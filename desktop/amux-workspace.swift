import Cocoa
import ServiceManagement

// The workspace: where every amux window lives and what it shows, kept across
// quits, crashes, reboots and display changes.
//
// - Each window's place is its display (by the monitor's UUID), its frame,
//   that frame as fractions of the display (for a resolution change), its
//   macOS Desktop (by the Space's UUID, then by its number on that display),
//   its tab group, and whether it is minimized or full screen.
// - The whole set autosaves within a few seconds of any change, so a crash or
//   power loss restores what was on screen moments before.
// - A window remembers its "home": where you last put it. A display that goes
//   away moves its windows to another one (macOS does that); when it comes
//   back the windows go home.
// - Snap to grid while dragging (with a preview of where the window lands),
//   and Window > Arrange for halves, thirds, quarters, tiling and stacking.

// ── Spaces (macOS Desktops) ──
// No public API names a window's Desktop or moves one there. The window
// server's own calls (the ones Mission Control and the Dock use) do both. They
// are looked up at run time, so a macOS without them restores on the current
// Desktop and logs why instead of failing to start.
enum Spaces {
    struct Info {
        let id: Int
        let uuid: String
        let display: String      // display UUID, or "Main" when Displays have separate Spaces is off
        let index: Int           // 1-based Desktop number on that display; 0 for a full-screen Space
        let fullscreen: Bool
        let current: Bool
    }

    private typealias MainConn = @convention(c) () -> Int32
    private typealias CopyDisplaySpaces = @convention(c) (Int32) -> Unmanaged<CFArray>?
    private typealias CopyWindowSpaces = @convention(c) (Int32, Int32, CFArray) -> Unmanaged<CFArray>?
    private typealias MoveWindows = @convention(c) (Int32, CFArray, Int) -> Void

    private static func sym<T>(_ name: String, _: T.Type) -> T? {
        guard let p = dlsym(UnsafeMutableRawPointer(bitPattern: -2), name) else { return nil }   // RTLD_DEFAULT
        return unsafeBitCast(p, to: T.self)
    }
    private static let mainConn = sym("CGSMainConnectionID", MainConn.self)
    private static let copyDisplaySpaces = sym("CGSCopyManagedDisplaySpaces", CopyDisplaySpaces.self)
    private static let copyWindowSpaces = sym("CGSCopySpacesForWindows", CopyWindowSpaces.self)
    private static let moveWindows = sym("CGSMoveWindowsToManagedSpace", MoveWindows.self)

    static var canRead: Bool { mainConn != nil && copyDisplaySpaces != nil && copyWindowSpaces != nil }
    static var canMove: Bool { canRead && moveWindows != nil }

    static func all() -> [Info] {
        guard let conn = mainConn?(), let arr = copyDisplaySpaces?(conn)?.takeRetainedValue() as? [[String: Any]] else { return [] }
        var out: [Info] = []
        for d in arr {
            let display = (d["Display Identifier"] as? String) ?? "Main"
            let cur = ((d["Current Space"] as? [String: Any])?["ManagedSpaceID"] as? Int) ?? -1
            var n = 0
            for s in (d["Spaces"] as? [[String: Any]]) ?? [] {
                guard let id = s["ManagedSpaceID"] as? Int else { continue }
                let fs = ((s["type"] as? Int) ?? 0) == 4
                if !fs { n += 1 }
                out.append(Info(id: id, uuid: (s["uuid"] as? String) ?? "", display: display,
                                index: fs ? 0 : n, fullscreen: fs, current: id == cur))
            }
        }
        return out
    }

    /// The Spaces a window is on: one normally, every Desktop for a window
    /// assigned to all of them, none while it is minimized.
    static func of(window number: Int) -> [Int] {
        guard number > 0, let conn = mainConn?(),
              let arr = copyWindowSpaces?(conn, 7, [number] as CFArray)?.takeRetainedValue() as? [Int] else { return [] }
        return arr
    }

    /// Moves the windows to a Space and reports whether the window server agrees it happened.
    @discardableResult
    static func move(windows numbers: [Int], to space: Int) -> Bool {
        let ids = numbers.filter { $0 > 0 }
        guard !ids.isEmpty, let conn = mainConn?(), let move = moveWindows else { return false }
        move(conn, ids as CFArray, space)
        return ids.allSatisfy { of(window: $0).contains(space) }
    }
}

// ── Displays ──
extension NSScreen {
    var displayID: CGDirectDisplayID {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
    }
    /// Stable across reboots and reconnects for the same monitor.
    var uuid: String {
        guard let u = CGDisplayCreateUUIDFromDisplayID(displayID)?.takeRetainedValue() else { return "" }
        return (CFUUIDCreateString(nil, u) as String?) ?? ""
    }
    static func with(uuid: String?) -> NSScreen? {
        guard let uuid = uuid, !uuid.isEmpty else { return nil }
        return screens.first { $0.uuid == uuid }
    }
}

func relRect(_ r: NSRect, in vf: NSRect) -> String {
    guard vf.width > 0, vf.height > 0 else { return "" }
    let f = { (v: CGFloat) in String(format: "%.4f", Double(v)) }
    return [f((r.minX - vf.minX) / vf.width), f((r.minY - vf.minY) / vf.height),
            f(r.width / vf.width), f(r.height / vf.height)].joined(separator: ",")
}

func fromRel(_ s: String?, in vf: NSRect) -> NSRect? {
    guard let p = s?.split(separator: ",").compactMap({ Double($0) }), p.count == 4 else { return nil }
    return NSRect(x: vf.minX + CGFloat(p[0]) * vf.width, y: vf.minY + CGFloat(p[1]) * vf.height,
                  width: CGFloat(p[2]) * vf.width, height: CGFloat(p[3]) * vf.height)
}

/// Where a saved frame goes on the displays connected now: its own display
/// at the same spot when it still fits there; scaled from its fractions when
/// that display's resolution changed; on the main display, scaled, when its
/// display is not connected.
func resolveFrame(_ s: SavedWindow) -> (frame: NSRect?, screen: NSScreen?) {
    let abs = NSRectFromString(s.frame)
    let home = NSScreen.with(uuid: s.display)
    let screen = home
        ?? (s.display == nil ? NSScreen.screens.max { $0.frame.intersection(abs).area < $1.frame.intersection(abs).area } : nil)
        ?? NSScreen.main ?? NSScreen.screens.first
    guard let scr = screen, abs.width > 100 else { return (nil, screen) }
    let vf = scr.visibleFrame
    if home != nil || s.display == nil, vf.insetBy(dx: -2, dy: -2).contains(abs) { return (abs, scr) }
    if let r = fromRel(s.rel, in: vf) { return (r.clamped(to: vf), scr) }
    return (abs.clamped(to: vf), scr)
}

extension NSRect {
    var area: CGFloat { isNull ? 0 : width * height }
    func clamped(to vf: NSRect) -> NSRect {
        var r = self
        r.size.width = min(max(r.width, 480), vf.width)
        r.size.height = min(max(r.height, 400), vf.height)
        r.origin.x = min(max(r.minX, vf.minX), vf.maxX - r.width)
        r.origin.y = min(max(r.minY, vf.minY), vf.maxY - r.height)
        return r
    }
}

// ── Capture ──
extension AppDelegate {
    /// Where the window is now, or where it belongs when that is a display
    /// that is not connected at the moment.
    func placement(of c: DashboardWindowController, spaces: [Spaces.Info], live: Bool = false) -> SavedWindow {
        if !live, let h = c.home, NSScreen.with(uuid: h.display) == nil, h.display != nil { return h }
        var p = SavedWindow(url: "", frame: "")
        guard let w = c.window else { return p }
        let fullscreen = w.styleMask.contains(.fullScreen)
        let frame = fullscreen ? (c.normalFrame ?? w.frame) : w.frame
        p.frame = NSStringFromRect(frame)
        let scr = w.screen ?? NSScreen.screens.max { $0.frame.intersection(frame).area < $1.frame.intersection(frame).area }
        if let scr = scr {
            p.display = scr.uuid
            p.displayName = scr.localizedName
            p.rel = relRect(frame, in: scr.visibleFrame)
        }
        if fullscreen { p.fullscreen = true }
        if w.isMiniaturized { p.minimized = true }
        let ids = Spaces.of(window: w.windowNumber)
        if ids.count > 1 {
            p.allDesktops = true
        } else if let id = ids.first, let sp = spaces.first(where: { $0.id == id }), !sp.fullscreen {
            c.lastSpace = sp
        }
        if !(p.allDesktops ?? false), let sp = c.lastSpace {
            p.space = sp.uuid
            p.spaceIndex = sp.index
        }
        return p
    }

    /// Every window, back to front, tab groups together in tab order.
    func snapshotNow() -> [SavedWindow] {
        let spaces = Spaces.all()
        let z = NSApp.orderedWindows
        var seen = Set<ObjectIdentifier>()
        var units: [(z: Int, items: [SavedWindow])] = []
        var groupN = 0
        for c in controllers {
            guard let w = c.window, !seen.contains(ObjectIdentifier(w)) else { continue }
            let tabWindows = w.tabbedWindows ?? [w]
            let members = tabWindows.compactMap { tw in controllers.first { $0.window === tw } }
            members.forEach { if let mw = $0.window { seen.insert(ObjectIdentifier(mw)) } }
            guard members.contains(where: { $0.window?.isVisible == true || $0.window?.isMiniaturized == true }) else { continue }
            let selected = w.tabGroup?.selectedWindow ?? w
            let lead = members.first { $0.window === selected } ?? members[0]
            let place = placement(of: lead, spaces: spaces)
            var group: Int? = nil
            if members.count > 1 { groupN += 1; group = groupN }
            let items = members.compactMap { m -> SavedWindow? in
                guard let url = m.savedURL() else { return nil }
                var s = place
                s.url = url
                s.tabGroup = group
                if group != nil { s.selected = m.window === selected }
                return s
            }
            if items.isEmpty { continue }
            let front = members.compactMap { m in m.window.flatMap { z.firstIndex(of: $0) } }.min() ?? Int.max
            units.append((front, items))
        }
        return units.sorted { $0.z > $1.z }.flatMap { $0.items }
    }
}

extension DashboardWindowController {
    /// The deep link this window reopens: the page it is on (or was on, while
    /// the server restarts and it shows the connect page) and its view.
    func savedURL() -> String? {
        let page = webView.url.flatMap { app?.isServerURL($0) == true ? $0 : nil } ?? lastServerURL
        guard let base = page else { return nil }
        if !viewHash.isEmpty, let u = app?.url(withHash: viewHash) { return u.absoluteString }
        return base.absoluteString
    }
}

// ── Restore ──
extension AppDelegate {
    /// Opens saved windows where they were: display, frame, Desktop, tabs,
    /// minimized and full screen. Windows already showing the same page are
    /// reused rather than opened again, so switching workspaces keeps their
    /// state (scroll, terminal, a half-typed message).
    func openWindows(_ saved: [SavedWindow], reuse: [DashboardWindowController] = []) {
        guard serverURL != nil, !saved.isEmpty else {
            if reuse.isEmpty { openWindow(url: nil, frame: nil) }
            return
        }
        restoring = true
        defer { restoring = false }
        let spaces = Spaces.all()
        var pool = reuse
        var lastInGroup: [Int: DashboardWindowController] = [:]
        var selected: [Int: DashboardWindowController] = [:]
        var placed: [(DashboardWindowController, SavedWindow)] = []
        for s in saved {
            let (frame, screen) = resolveFrame(s)
            let c: DashboardWindowController
            if let i = pool.firstIndex(where: { $0.savedURL() == s.url }) {
                c = pool.remove(at: i)
                if let w = c.window {
                    if w.isMiniaturized { w.deminiaturize(nil) }
                    if w.styleMask.contains(.fullScreen) { w.toggleFullScreen(nil) }
                    if w.tabbedWindows != nil { w.moveTabToNewWindow(nil) }
                    if let f = frame { w.setFrame(f, display: false) }
                    w.orderFront(nil)
                }
            } else {
                c = openWindow(url: URL(string: s.url), frame: frame, activate: false)
            }
            c.home = s
            c.home?.display = s.display ?? screen?.uuid
            if let g = s.tabGroup {
                if let prev = lastInGroup[g], let pw = prev.window, let cw = c.window {
                    pw.addTabbedWindow(cw, ordered: .above)
                }
                lastInGroup[g] = c
                if s.selected == true { selected[g] = c }
            }
            placed.append((c, s))
        }
        for c in pool { c.window?.close() }
        for (_, c) in selected { c.window?.tabGroup?.selectedWindow = c.window }

        // Desktops: move each window (a tab group as one) to its Space.
        var moved = 0, stayed = 0, missing = 0
        var done = Set<ObjectIdentifier>()
        for (c, s) in placed {
            guard let w = c.window, !done.contains(ObjectIdentifier(w)) else { continue }
            let group = (w.tabbedWindows ?? [w])
            group.forEach { done.insert(ObjectIdentifier($0)) }
            if s.allDesktops == true {
                group.forEach { $0.collectionBehavior.insert(.canJoinAllSpaces) }
                continue
            }
            guard s.space != nil || s.spaceIndex != nil else { continue }
            guard let target = resolveSpace(s, screen: w.screen, spaces: spaces) else { missing += 1; continue }
            if target.current { stayed += 1; c.lastSpace = target; continue }
            if Spaces.move(windows: group.map { $0.windowNumber }, to: target.id) {
                moved += 1
                c.lastSpace = target
            } else {
                missing += 1
            }
        }
        for (c, s) in placed {
            if s.minimized == true { c.window?.miniaturize(nil) }
            if s.fullscreen == true, let w = c.window, !w.styleMask.contains(.fullScreen) {
                c.normalFrame = w.frame
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { w.toggleFullScreen(nil) }
            }
        }
        // Focus the front-most window on the Desktop you are on, so restoring
        // does not pull you to another Desktop.
        if let front = placed.reversed().first(where: { $0.0.window?.isOnActiveSpace == true && $0.1.minimized != true })?.0 {
            front.window?.makeKeyAndOrderFront(nil)
        }
        NSLog("amux-desktop: verdict=windows_restored n_considered=%d desktops_moved=%d desktops_current=%d desktops_unavailable=%d spaces_readable=%d spaces_movable=%d displays=%d",
              saved.count, moved, stayed, missing, Spaces.canRead ? 1 : 0, Spaces.canMove ? 1 : 0, NSScreen.screens.count)
    }

    /// The Desktop a saved window belongs on: the same Space if it still
    /// exists, else the Desktop with the same number on the window's display.
    func resolveSpace(_ s: SavedWindow, screen: NSScreen?, spaces: [Spaces.Info]) -> Spaces.Info? {
        if let u = s.space, !u.isEmpty, let hit = spaces.first(where: { $0.uuid == u && !$0.fullscreen }) { return hit }
        guard let n = s.spaceIndex, n > 0 else { return nil }
        let disp = screen?.uuid ?? ""
        let onDisplay = spaces.filter { !$0.fullscreen && ($0.display == disp || $0.display == "Main") }
        return onDisplay.first { $0.index == n }
    }
}

// ── Autosave, crash and display-change handling ──
extension AppDelegate {
    func startWorkspaceTracking() {
        let nc = NotificationCenter.default
        nc.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            self?.screensChanged()
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
            self?.workspaceChanged()
        }
        // A window dragged to another Desktop in Mission Control sends no
        // window notification; a cheap signature check catches it.
        placementTimer = Timer.scheduledTimer(withTimeInterval: 4, repeats: true) { [weak self] _ in
            guard let self = self, !self.restoring, !self.terminating else { return }
            let sig = self.placementSignature()
            if sig != self.lastSignature { self.lastSignature = sig; self.saveWindowsSoon() }
        }
        // SIGTERM (a logout, a reboot, `kill`) quits the normal way, saving first.
        signal(SIGTERM, SIG_IGN)
        let src = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        src.setEventHandler { NSLog("amux-desktop: verdict=sigterm_quit"); NSApp.terminate(nil) }
        src.resume()
        sigtermSource = src
        // Another copy (tests, a second server) is driven by its pid, since an
        // amux:// link reaches whichever copy macOS picks.
        DistributedNotificationCenter.default().addObserver(forName: Notification.Name("io.amux.desktop.url.\(getpid())"), object: nil, queue: .main) { [weak self] n in
            if let s = n.object as? String, let u = URL(string: s) { self?.handleAmuxURL(u) }
        }
    }

    func placementSignature() -> String {
        controllers.compactMap { c -> String? in
            guard let w = c.window else { return nil }
            return "\(w.windowNumber):\(NSStringFromRect(w.frame)):\(Spaces.of(window: w.windowNumber)):\(w.isMiniaturized):\(c.viewHash)"
        }.joined(separator: "|")
    }

    func workspaceChanged() {
        if !restoring { saveWindowsSoon() }
    }

    /// A move or resize sets the window's home, except one macOS makes because
    /// a display came or went. While the home display is unplugged only you
    /// (a drag, a resize, Arrange) can give the window a new home: macOS can
    /// report the move off an unplugged display before it reports the display
    /// change, so the grace period alone let that move overwrite the home.
    func windowPlaced(_ c: DashboardWindowController, user: Bool = false) {
        guard !restoring, !terminating else { return }
        let homeGone = c.home.map { $0.display != nil && NSScreen.with(uuid: $0.display) == nil } ?? false
        // Onto another display only by you: the app's own screen list can
        // still name a display macOS is already moving windows off.
        let sameDisplay = c.home?.display == nil || c.window?.screen?.uuid == c.home?.display
        if user || (Date() > screenGrace && !homeGone && sameDisplay), let w = c.window, !w.styleMask.contains(.fullScreen) {
            let before = c.home?.display
            c.home = placement(of: c, spaces: Spaces.all(), live: true)
            if before != nil, before != c.home?.display {
                NSLog("amux-desktop: verdict=window_home_display_changed window=%d from=%@ to=%@ by=%@ screens=%d",
                      w.windowNumber, before ?? "-", c.home?.display ?? "-", user ? "you" : "macos", NSScreen.screens.count)
            }
        }
        saveWindowsSoon()
    }

    func screensChanged() {
        screenGrace = Date().addingTimeInterval(3)
        let now = NSScreen.screens.map { "\($0.localizedName) \(NSStringFromRect($0.frame))" }
        NSLog("amux-desktop: verdict=displays_changed displays=%d [%@]", now.count, now.joined(separator: "; "))
        // Let macOS finish moving windows off a removed display first.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.sendWindowsHome() }
    }

    /// Windows whose home display is connected again go back to it.
    func sendWindowsHome() {
        var n = 0
        for c in controllers {
            guard let h = c.home, let w = c.window, !w.styleMask.contains(.fullScreen),
                  let home = NSScreen.with(uuid: h.display) else { continue }
            let (frame, _) = resolveFrame(h)
            guard let f = frame else { continue }
            if w.screen?.uuid != home.uuid || abs(w.frame.minX - f.minX) > 2 || abs(w.frame.minY - f.minY) > 2 || abs(w.frame.width - f.width) > 2 {
                w.setFrame(f, display: true, animate: false)
                n += 1
            }
        }
        if n > 0 { NSLog("amux-desktop: verdict=display_windows_returned n=%d n_considered=%d", n, controllers.count) }
        saveWindowsSoon()
    }

    // ── Launch at login, so a reboot comes back to the workspace ──
    func applyOpenAtLogin() {
        guard #available(macOS 13.0, *) else { return }
        let want = config.openAtLogin ?? true
        let svc = SMAppService.mainApp
        guard Bundle.main.bundleURL.pathExtension == "app" else { return }
        do {
            if want, svc.status != .enabled { try svc.register() }
            if !want, svc.status == .enabled { try svc.unregister() }
        } catch {
            NSLog("amux-desktop: verdict=login_item_failed want=%d error=%@", want ? 1 : 0, error.localizedDescription)
        }
        NSLog("amux-desktop: login_item want=%d status=%d", want ? 1 : 0, svc.status.rawValue)
    }

    var openAtLoginStatus: String {
        guard #available(macOS 13.0, *) else { return "needs macOS 13" }
        switch SMAppService.mainApp.status {
        case .enabled: return "on"
        case .requiresApproval: return "waiting for approval in System Settings > General > Login Items"
        case .notRegistered: return "off"
        default: return "unavailable"
        }
    }
}

// ── Snap to grid and Arrange ──
// The grid is 12 columns by 8 rows of the display's usable area. A window
// dragged near a grid line or another amux window's edge snaps to it when you
// let go, and a translucent outline shows where while you drag. Hold Option
// while letting go to place it freely.
let gridColumns: CGFloat = 12
let gridRows: CGFloat = 8
let snapDistance: CGFloat = 18

final class SnapPreview {
    let panel: NSPanel
    init() {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.level = .floating
        panel.collectionBehavior = [.moveToActiveSpace, .transient, .ignoresCycle]
        let v = NSView()
        v.wantsLayer = true
        v.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.16).cgColor
        v.layer?.borderColor = NSColor.controlAccentColor.withAlphaComponent(0.75).cgColor
        v.layer?.borderWidth = 2
        v.layer?.cornerRadius = 10
        panel.contentView = v
    }
    func show(_ r: NSRect, below w: NSWindow) {
        panel.setFrame(r, display: true)
        if !panel.isVisible { panel.order(.below, relativeTo: w.windowNumber) }
    }
    func hide() { panel.orderOut(nil) }
}

extension AppDelegate {
    var snapEnabled: Bool { config.snapToGrid ?? true }

    func gridLines(_ vf: NSRect, excluding w: NSWindow) -> (xs: [CGFloat], ys: [CGFloat]) {
        var xs = (0...Int(gridColumns)).map { vf.minX + vf.width * CGFloat($0) / gridColumns }
        var ys = (0...Int(gridRows)).map { vf.minY + vf.height * CGFloat($0) / gridRows }
        for c in controllers {
            guard let o = c.window, o !== w, o.isVisible, o.isOnActiveSpace, !o.isMiniaturized,
                  !(w.tabbedWindows?.contains(o) ?? false) else { continue }
            xs += [o.frame.minX, o.frame.maxX]
            ys += [o.frame.minY, o.frame.maxY]
        }
        return (xs, ys)
    }

    /// The nearest snap for a moved window: whichever edge is closest to a line, size unchanged.
    func snapMove(_ f: NSRect, _ w: NSWindow) -> NSRect {
        guard let vf = w.screen?.visibleFrame else { return f }
        let (xs, ys) = gridLines(vf, excluding: w)
        func best(_ a: CGFloat, _ b: CGFloat, _ lines: [CGFloat]) -> CGFloat {
            var d: CGFloat = .infinity
            for l in lines {
                if abs(l - a) < abs(d) { d = l - a }
                if abs(l - b) < abs(d) { d = l - b }
            }
            return abs(d) <= snapDistance ? d : 0
        }
        return f.offsetBy(dx: best(f.minX, f.maxX, xs), dy: best(f.minY, f.maxY, ys))
    }

    /// A resized window: each edge snaps on its own.
    func snapResize(_ f: NSRect, _ w: NSWindow) -> NSRect {
        guard let vf = w.screen?.visibleFrame else { return f }
        let (xs, ys) = gridLines(vf, excluding: w)
        func near(_ v: CGFloat, _ lines: [CGFloat]) -> CGFloat {
            let l = lines.min { abs($0 - v) < abs($1 - v) } ?? v
            return abs(l - v) <= snapDistance ? l : v
        }
        let x0 = near(f.minX, xs), x1 = near(f.maxX, xs), y0 = near(f.minY, ys), y1 = near(f.maxY, ys)
        let r = NSRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
        return r.width >= w.minSize.width && r.height >= w.minSize.height ? r : f
    }

    func dragBegan(_ c: DashboardWindowController) {
        // windowWillMove also fires for a programmatic move (Arrange, a
        // restore); only a drag with the button down snaps.
        guard NSEvent.pressedMouseButtons & 1 != 0, !restoring,
              let w = c.window, !w.styleMask.contains(.fullScreen) else { return }
        dragTimer?.invalidate()
        let t = Timer(timeInterval: 1.0 / 30, repeats: true) { [weak self, weak c] t in
            guard let self = self, let c = c, let w = c.window else { t.invalidate(); return }
            let free = !self.snapEnabled || NSEvent.modifierFlags.contains(.option)
            let target = self.snapMove(w.frame, w)
            if NSEvent.pressedMouseButtons & 1 != 0 {
                if !free, target != w.frame { self.snapPreview.show(target, below: w) } else { self.snapPreview.hide() }
                return
            }
            t.invalidate()
            self.snapPreview.hide()
            if !free, target != w.frame {
                w.setFrame(target, display: true, animate: true)
                self.arranged(w, to: target)
            } else {
                self.windowPlaced(c, user: true)
            }
        }
        RunLoop.main.add(t, forMode: .common)
        dragTimer = t
    }

    func resizeEnded(_ c: DashboardWindowController) {
        guard let w = c.window else { return }
        if restoring { return }
        if snapEnabled, !NSEvent.modifierFlags.contains(.option), !w.styleMask.contains(.fullScreen) {
            let r = snapResize(w.frame, w)
            if r != w.frame { w.setFrame(r, display: true, animate: true); arranged(w, to: r); return }
        }
        windowPlaced(c, user: true)
    }

    // ── Window > Arrange ──
    /// The window Arrange acts on: the key window, or with amux in the
    /// background (an amux:// link), its front-most window on this Desktop.
    var arrangeTarget: NSWindow? {
        if let k = NSApp.keyWindow, controllers.contains(where: { $0.window === k }) { return k }
        return NSApp.orderedWindows.first { w in
            controllers.contains { $0.window === w } && w.isVisible && w.isOnActiveSpace && !w.isMiniaturized
        }
    }

    /// x, y, width, height as fractions of the display's usable area.
    func place(_ fx: CGFloat, _ fy: CGFloat, _ fw: CGFloat, _ fh: CGFloat) {
        guard let w = arrangeTarget, let vf = w.screen?.visibleFrame else { return }
        if w.styleMask.contains(.fullScreen) { return }
        let r = NSRect(x: vf.minX + vf.width * fx, y: vf.minY + vf.height * fy, width: vf.width * fw, height: vf.height * fh).integral
        w.setFrame(r, display: true, animate: true)
        arranged(w, to: r)
    }

    @objc func arrangeLeftHalf(_ s: Any?) { place(0, 0, 0.5, 1) }
    @objc func arrangeRightHalf(_ s: Any?) { place(0.5, 0, 0.5, 1) }
    @objc func arrangeTopHalf(_ s: Any?) { place(0, 0.5, 1, 0.5) }
    @objc func arrangeBottomHalf(_ s: Any?) { place(0, 0, 1, 0.5) }
    @objc func arrangeTopLeft(_ s: Any?) { place(0, 0.5, 0.5, 0.5) }
    @objc func arrangeTopRight(_ s: Any?) { place(0.5, 0.5, 0.5, 0.5) }
    @objc func arrangeBottomLeft(_ s: Any?) { place(0, 0, 0.5, 0.5) }
    @objc func arrangeBottomRight(_ s: Any?) { place(0.5, 0, 0.5, 0.5) }
    @objc func arrangeLeftThird(_ s: Any?) { place(0, 0, 1.0 / 3, 1) }
    @objc func arrangeCenterThird(_ s: Any?) { place(1.0 / 3, 0, 1.0 / 3, 1) }
    @objc func arrangeRightThird(_ s: Any?) { place(2.0 / 3, 0, 1.0 / 3, 1) }
    @objc func arrangeFill(_ s: Any?) { place(0, 0, 1, 1) }
    @objc func arrangeCenter(_ s: Any?) { place(0.15, 0.1, 0.7, 0.8) }

    /// The key window onto the next display, at the same relative spot.
    @objc func arrangeNextDisplay(_ s: Any?) {
        guard let w = arrangeTarget, let cur = w.screen, NSScreen.screens.count > 1,
              let i = NSScreen.screens.firstIndex(of: cur) else { return }
        let next = NSScreen.screens[(i + 1) % NSScreen.screens.count]
        if let r = fromRel(relRect(w.frame, in: cur.visibleFrame), in: next.visibleFrame) {
            let f = r.clamped(to: next.visibleFrame)
            w.setFrame(f, display: true, animate: true)
            arranged(w, to: f)
        }
    }

    /// Arrange is you placing the window: it becomes its home. Read once the
    /// move has finished: an animated setFrame returns before the window has
    /// moved, and the home it recorded was the display it was leaving.
    func arranged(_ w: NSWindow, to target: NSRect? = nil) {
        let delay = target.map { w.animationResizeTime($0) + 0.1 } ?? 0.4
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self, weak w] in
            guard let self = self, let w = w else { return }
            for c in self.controllers where c.window === w || (w.tabbedWindows?.contains { $0 === c.window } ?? false) {
                self.windowPlaced(c, user: true)
            }
        }
    }

    /// The windows on this Desktop and display (a tab group counts once).
    func windowsHere(_ scr: NSScreen) -> [NSWindow] {
        var seen = Set<ObjectIdentifier>()
        var out: [NSWindow] = []
        let z = NSApp.orderedWindows
        for w in z where controllers.contains(where: { $0.window === w }) {
            guard w.isVisible, w.isOnActiveSpace, !w.isMiniaturized, !w.styleMask.contains(.fullScreen),
                  w.screen == scr, !seen.contains(ObjectIdentifier(w)) else { continue }
            (w.tabbedWindows ?? [w]).forEach { seen.insert(ObjectIdentifier($0)) }
            out.append(w)
        }
        return out
    }

    /// Tiles every window on this Desktop and display into an even grid,
    /// front-most window top left.
    @objc func arrangeTile(_ s: Any?) {
        guard let scr = arrangeTarget?.screen ?? NSScreen.main else { return }
        let ws = windowsHere(scr)
        guard !ws.isEmpty else { return }
        let vf = scr.visibleFrame
        let n = ws.count
        var cols = Int(ceil(sqrt(Double(n) * Double(vf.width / vf.height) / 1.6)))
        cols = max(1, min(n, cols))
        let rows = Int(ceil(Double(n) / Double(cols)))
        let gap: CGFloat = 6
        let cw = (vf.width - gap * CGFloat(cols + 1)) / CGFloat(cols)
        let ch = (vf.height - gap * CGFloat(rows + 1)) / CGFloat(rows)
        for (i, w) in ws.enumerated() {
            let r = i / cols, col = i % cols
            // The last row's windows share its width.
            let inRow = r == rows - 1 ? n - cols * (rows - 1) : cols
            let rw = (vf.width - gap * CGFloat(inRow + 1)) / CGFloat(inRow)
            let width = r == rows - 1 ? rw : cw
            let x = vf.minX + gap + CGFloat(col) * (width + gap)
            let y = vf.maxY - gap - CGFloat(r + 1) * ch - CGFloat(r) * gap
            let f = NSRect(x: x, y: y, width: width, height: ch).integral
            w.setFrame(f, display: true, animate: true)
            arranged(w, to: f)
        }
        NSLog("amux-desktop: verdict=windows_tiled n=%d cols=%d rows=%d", n, cols, rows)
    }

    /// Every window on this Desktop and display as tabs of the key window.
    @objc func arrangeStack(_ s: Any?) {
        guard let lead = arrangeTarget, let scr = lead.screen else { return }
        var last = lead
        for w in windowsHere(scr) where w !== lead && !(lead.tabbedWindows?.contains(w) ?? false) {
            last.addTabbedWindow(w, ordered: .above)
            last = w
        }
        lead.makeKeyAndOrderFront(nil)
    }

    @objc func toggleSnap(_ s: Any?) {
        config = loadConfig()
        config.snapToGrid = !(config.snapToGrid ?? true)
        saveConfig(config)
    }
}

func buildArrangeMenu(_ d: AppDelegate) -> NSMenu {
    let m = NSMenu(title: "Arrange")
    let ctlOptCmd: NSEvent.ModifierFlags = [.control, .option, .command]
    func key(_ c: Int) -> String { String(Character(UnicodeScalar(c)!)) }
    let items: [(String, Selector, String)] = [
        ("Left Half", #selector(AppDelegate.arrangeLeftHalf(_:)), key(NSLeftArrowFunctionKey)),
        ("Right Half", #selector(AppDelegate.arrangeRightHalf(_:)), key(NSRightArrowFunctionKey)),
        ("Top Half", #selector(AppDelegate.arrangeTopHalf(_:)), key(NSUpArrowFunctionKey)),
        ("Bottom Half", #selector(AppDelegate.arrangeBottomHalf(_:)), key(NSDownArrowFunctionKey)),
        ("-", #selector(AppDelegate.arrangeFill(_:)), ""),
        ("Top Left", #selector(AppDelegate.arrangeTopLeft(_:)), "u"),
        ("Top Right", #selector(AppDelegate.arrangeTopRight(_:)), "i"),
        ("Bottom Left", #selector(AppDelegate.arrangeBottomLeft(_:)), "j"),
        ("Bottom Right", #selector(AppDelegate.arrangeBottomRight(_:)), "k"),
        ("-", #selector(AppDelegate.arrangeFill(_:)), ""),
        ("Left Third", #selector(AppDelegate.arrangeLeftThird(_:)), "d"),
        ("Center Third", #selector(AppDelegate.arrangeCenterThird(_:)), "f"),
        ("Right Third", #selector(AppDelegate.arrangeRightThird(_:)), "g"),
        ("-", #selector(AppDelegate.arrangeFill(_:)), ""),
        ("Fill", #selector(AppDelegate.arrangeFill(_:)), "\r"),
        ("Center", #selector(AppDelegate.arrangeCenter(_:)), "c"),
        ("Move to Next Display", #selector(AppDelegate.arrangeNextDisplay(_:)), "n"),
        ("-", #selector(AppDelegate.arrangeFill(_:)), ""),
        ("Tile Windows on This Desktop", #selector(AppDelegate.arrangeTile(_:)), "t"),
        ("Stack Windows on This Desktop as Tabs", #selector(AppDelegate.arrangeStack(_:)), "s"),
    ]
    for (title, sel, k) in items {
        if title == "-" { m.addItem(.separator()); continue }
        let it = m.addItem(withTitle: title, action: sel, keyEquivalent: k)
        it.keyEquivalentModifierMask = ctlOptCmd
        it.target = d
    }
    m.addItem(.separator())
    let snap = m.addItem(withTitle: "Snap to Grid While Dragging", action: #selector(AppDelegate.toggleSnap(_:)), keyEquivalent: "")
    snap.target = d
    return m
}
