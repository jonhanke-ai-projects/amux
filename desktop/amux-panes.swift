import Cocoa
import WebKit

// Panes: a tab dragged from the tab bar into an amux window splits THAT window
// into a grid, and the tab becomes a pane in it (Ethan, 2026-10-23: "i want it
// to be all within the same amux window rather than the mac desktop"). Desktop
// snap zones (amux-snapzones.swift) still place a tab dropped outside every
// window.
//
// A pane is the dragged tab's own controller and web view: the web view moves
// into the grid and the tab's window is hidden, so the page keeps its state
// (scroll, terminal, a half-typed message). The window the grid lives in is its
// host. Each pane has a header: its name, a button that turns it back into a
// tab, and a close button. Dragging the header moves the pane into another
// pane's zone (this window or another), or out of every window to become a tab
// again. Dividers resize. The layout is amux-panegrid.swift's tree, saved with
// the workspace.

final class PaneGrid {
    var tree: PaneTree
    var members: [Int: DashboardWindowController]
    var nextId: Int
    weak var host: DashboardWindowController?
    let container = NSView()
    var views: [Int: PaneView] = [:]

    init(host: DashboardWindowController) {
        self.host = host
        tree = .leaf(0)
        members = [0: host]
        nextId = 1
        container.autoresizingMask = [.width, .height]
    }

    func id(of c: DashboardWindowController) -> Int? { members.first { $0.value === c }?.key }

    /// The tree with each split's fraction as the dividers have it now.
    func syncFractions() {
        guard let root = container.subviews.first else { return }
        func walk(_ t: PaneTree, _ v: NSView) -> PaneTree {
            guard case .split(let row, let f, let a, let b) = t, let sv = v as? GridSplitView, sv.subviews.count == 2 else { return t }
            return .split(row: row, fraction: sv.currentFraction ?? f, walk(a, sv.subviews[0]), walk(b, sv.subviews[1]))
        }
        tree = walk(tree, root)
    }

    /// Rebuilds the split views for the tree. Web views move into their panes;
    /// WebKit keeps the pages running while they do.
    func rebuild() {
        container.subviews.forEach { $0.removeFromSuperview() }
        func make(_ t: PaneTree) -> NSView {
            switch t {
            case .leaf(let id):
                let v = views[id] ?? PaneView(grid: self, id: id)
                views[id] = v
                if let c = members[id] { v.attach(c.webView); v.title = c.pageTitle }
                return v
            case .split(let row, let f, let a, let b):
                let s = GridSplitView(row: row, fraction: f)
                s.onResize = { [weak self] in self?.host?.app?.saveWindowsSoon() }
                s.addSubview(make(a))
                s.addSubview(make(b))
                return s
            }
        }
        let root = make(tree)
        root.frame = container.bounds
        root.autoresizingMask = [.width, .height]
        container.addSubview(root)
        let live = Set(tree.leaves)
        views = views.filter { live.contains($0.key) }

    }

    /// Each pane's frame on screen, for drop zones.
    func paneFrames() -> [(id: Int, frame: NSRect)] {
        views.compactMap { id, v in
            guard let w = v.window else { return nil }
            return (id, w.convertToScreen(v.convert(v.bounds, to: nil)))
        }
    }

    var signature: String { syncFractions(); return tree.encoded }
    /// The layout with panes numbered in reading order, as it is saved.
    var readingOrder: String { syncFractions(); let l = tree.leaves; return tree.mapped { l.firstIndex(of: $0) ?? -1 }.encoded }
}

/// One split. Holds the fraction it was built with until it has a size to apply it to.
final class GridSplitView: NSSplitView, NSSplitViewDelegate {
    var pending: Double?
    var onResize: (() -> Void)?
    init(row: Bool, fraction: Double) {
        pending = fraction
        super.init(frame: .zero)
        isVertical = row
        dividerStyle = .thin
        delegate = self
    }
    required init?(coder: NSCoder) { fatalError("not used") }

    var length: CGFloat { isVertical ? bounds.width : bounds.height }
    var currentFraction: Double? {
        guard pending == nil, subviews.count == 2, length > dividerThickness else { return pending }
        let first = isVertical ? subviews[0].frame.width : subviews[0].frame.height
        return Double(first / (length - dividerThickness))
    }

    override func layout() {
        super.layout()
        if let f = pending, subviews.count == 2, length > dividerThickness {
            pending = nil
            setPosition((length - dividerThickness) * CGFloat(f), ofDividerAt: 0)
        }
    }
    func splitView(_ s: NSSplitView, constrainMinCoordinate p: CGFloat, ofSubviewAt i: Int) -> CGFloat { 160 }
    func splitView(_ s: NSSplitView, constrainMaxCoordinate p: CGFloat, ofSubviewAt i: Int) -> CGFloat { length - 160 }
    func splitViewDidResizeSubviews(_ n: Notification) {
        if pending == nil, NSEvent.pressedMouseButtons & 1 != 0 { onResize?() }
    }
}

/// A pane: a header strip and the page under it.
final class PaneView: NSView {
    weak var grid: PaneGrid?
    let id: Int
    let header = PaneHeader()
    private let label = NSTextField(labelWithString: "")
    private let tabButton = NSButton()
    private let closeButton = NSButton()
    static let headerHeight: CGFloat = 24
    var title: String { get { label.stringValue } set { label.stringValue = newValue; label.toolTip = newValue } }

    init(grid: PaneGrid, id: Int) {
        self.grid = grid
        self.id = id
        super.init(frame: .zero)
        header.wantsLayer = true
        header.layer?.backgroundColor = NSColor.black.withAlphaComponent(0.28).cgColor
        label.font = .systemFont(ofSize: 11, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.lineBreakMode = .byTruncatingTail
        for (b, sym, tip, act) in [(tabButton, "rectangle.on.rectangle", "Move to a tab", #selector(toTab)),
                                   (closeButton, "xmark", "Close pane", #selector(closePane))] {
            b.isBordered = false
            b.image = NSImage(systemSymbolName: sym, accessibilityDescription: tip)
            b.imagePosition = .imageOnly
            b.contentTintColor = .secondaryLabelColor
            b.toolTip = tip
            b.target = self
            b.action = act
            header.addSubview(b)
        }
        header.addSubview(label)
        addSubview(header)
        header.onDrag = { [weak self] p in self?.dragged(to: p) }
        header.onDrop = { [weak self] p in self?.dropped(at: p) }
    }
    required init?(coder: NSCoder) { fatalError("not used") }

    func attach(_ wv: WKWebView) {
        if wv.superview !== self { wv.removeFromSuperview(); addSubview(wv) }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let h = PaneView.headerHeight
        header.frame = NSRect(x: 0, y: bounds.height - h, width: bounds.width, height: h)
        closeButton.frame = NSRect(x: bounds.width - 26, y: 2, width: 22, height: 20)
        tabButton.frame = NSRect(x: bounds.width - 50, y: 2, width: 22, height: 20)
        label.frame = NSRect(x: 8, y: 4, width: max(0, bounds.width - 64), height: 16)
        for v in subviews where v is WKWebView { v.frame = NSRect(x: 0, y: 0, width: bounds.width, height: max(0, bounds.height - h)) }
    }

    var controller: DashboardWindowController? { grid?.members[id] }

    @objc func toTab() { if let c = controller { c.app?.undockPane(c, to: .tab) } }
    @objc func closePane() { if let c = controller { c.app?.undockPane(c, to: .close) } }

    func dragged(to p: NSPoint) {
        guard let app = controller?.app else { return }
        if let t = app.paneDropTarget(at: p, moving: controller), !(t.host === grid?.host && t.paneId == id) {
            app.snapPreview.showFront(t.preview)
        } else {
            app.snapPreview.hide()
        }
    }

    func dropped(at p: NSPoint) {
        guard let c = controller, let app = c.app, let hostWindow = grid?.host?.window else { return }
        app.snapPreview.hide()
        if let t = app.paneDropTarget(at: p, moving: c) {
            if t.host === grid?.host && t.paneId == id { return }
            app.undockPane(c, to: .move(t))
            return
        }
        // Out of every window, or onto this window's tab bar: a tab again.
        let content = hostWindow.convertToScreen(hostWindow.contentLayoutRect)
        if !hostWindow.frame.contains(p) || p.y > content.maxY { app.undockPane(c, to: .tab) }
    }
}

/// The header's drag: a pane follows the pointer until the button comes up.
final class PaneHeader: NSView {
    var onDrag: ((NSPoint) -> Void)?
    var onDrop: ((NSPoint) -> Void)?
    override func mouseDown(with e: NSEvent) {
        guard let w = window else { return }
        let start = NSEvent.mouseLocation
        var dragging = false
        while let ev = w.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            let p = NSEvent.mouseLocation
            if ev.type == .leftMouseUp { break }
            if !dragging, hypot(p.x - start.x, p.y - start.y) > 6 { dragging = true; NSCursor.closedHand.push() }
            if dragging { onDrag?(p) }
        }
        if dragging { NSCursor.pop(); onDrop?(NSEvent.mouseLocation) }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
}

struct SavedPane: Codable {
    var url: String
    var title: String?
}

struct SavedGrid: Codable {
    var tree: String          // PaneTree.encoded; 0 is the window's own page
    var panes: [SavedPane]    // 1, 2, ... in reading order
}

struct PaneDropTarget {
    let host: DashboardWindowController
    let paneId: Int?          // nil: the window shows one page, no grid yet
    let edge: PaneEdge
    let name: String
    let preview: NSRect
}

enum PaneDestination {
    case tab, close
    case move(PaneDropTarget)
}

extension DashboardWindowController {
    var isGridHost: Bool { grid != nil }
    /// Where sheets (alerts, file pickers) attach: a pane's own window is hidden.
    var sheetWindow: NSWindow { paneHost?.window ?? window! }
}

extension AppDelegate {
    /// The amux window at `p`, front-most, skipping `excluding` (a tab being
    /// dragged, which follows the pointer) and the drop preview. nil when another
    /// app's window is on top there. Read from the window server's front-to-back
    /// list, skipping its own layers above the screen saver's: a locked or
    /// shielded display put a full-screen "Display Shield" window on top, and
    /// windowNumber(at:) answered that for every point (2026-10-23).
    func amuxWindow(at p: NSPoint, excluding: [NSWindow] = []) -> DashboardWindowController? {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]],
              let top = NSScreen.screens.first?.frame.maxY else { return nil }
        let q = CGPoint(x: p.x, y: top - p.y)   // the window server's y runs down from the main display's top
        let skip = Set(excluding.map { $0.windowNumber } + [snapPreview.panel.windowNumber])
        for w in list {
            guard let n = w[kCGWindowNumber as String] as? Int, !skip.contains(n),
                  (w[kCGWindowLayer as String] as? Int ?? 0) < Int(CGWindowLevelForKey(.screenSaverWindow)),
                  let b = w[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b), r.contains(q) else { continue }
            if (w[kCGWindowAlpha as String] as? Double ?? 1) == 0 { continue }
            return controllers.first { $0.window?.windowNumber == n && $0.window?.isVisible == true }
        }
        return nil
    }

    /// Where a tab (or a pane, `moving`) dropped at `p` would go: the pane under
    /// it in an amux window's content, and which half. nil over a tab bar, away
    /// from every amux window, or on a window that already has `maxPanes`.
    func paneDropTarget(at p: NSPoint, excluding: [NSWindow] = [], moving: DashboardWindowController? = nil) -> PaneDropTarget? {
        guard snapEnabled, !NSEvent.modifierFlags.contains(.option),
              let host = amuxWindow(at: p, excluding: excluding), let w = host.window, !w.styleMask.contains(.fullScreen) else { return nil }
        let content = w.convertToScreen(w.contentLayoutRect)
        guard content.contains(p) else { return nil }
        var paneId: Int? = nil
        var rect = content
        if let g = host.grid {
            let samePane = moving.flatMap { g.id(of: $0) }
            let full = g.tree.leaves.count >= maxPanes && samePane == nil
            guard !full, let hit = g.paneFrames().first(where: { $0.frame.contains(p) }) else { return nil }
            paneId = hit.id
            rect = hit.frame
        }
        guard rect.width > 0, rect.height > 0 else { return nil }
        let d = paneDropEdge(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height, width: rect.width, height: rect.height)
        return PaneDropTarget(host: host, paneId: paneId, edge: d.edge, name: d.name, preview: paneDropRect(rect, edge: d.edge).integral)
    }

    /// `c` becomes a pane of `host`'s window, beside `paneId` on `edge`.
    @discardableResult
    func dockPane(_ c: DashboardWindowController, into host: DashboardWindowController, at paneId: Int?, edge: PaneEdge, zone: String = "") -> Bool {
        guard c !== host, c.grid == nil, let hw = host.window, let cw = c.window else {
            dlog("verdict=pane_dock_refused reason=\(c === host ? "same_window" : c.grid != nil ? "dragged_tab_has_grid" : "no_window")")
            return false
        }
        if host.grid == nil {
            let g = PaneGrid(host: host)
            host.grid = g
            g.container.frame = hw.contentView?.bounds ?? .zero
            hw.contentView = g.container
        }
        guard let g = host.grid else { return false }
        if g.tree.leaves.count >= maxPanes {
            dlog("verdict=pane_dock_refused reason=grid_full panes=\(g.tree.leaves.count)")
            return false
        }
        // Out of its tab group and off screen; its page moves into the grid.
        // Tab groups change here by the app, not a drag: the tear watcher must not read it as one.
        lastTabPlaced = Date()
        if (cw.tabbedWindows?.count ?? 1) > 1 { cw.tabGroup?.removeWindow(cw) }
        cw.orderOut(nil)
        cw.contentView = NSView()
        c.paneHost = host
        g.syncFractions()
        let id = g.nextId
        g.nextId += 1
        g.members[id] = c
        g.tree = g.tree.inserting(id, at: paneId ?? g.tree.leaves.first ?? 0, edge: edge)
        g.rebuild()
        host.updateTitle()
        hw.tabGroup?.selectedWindow = hw
        hw.makeKeyAndOrderFront(nil)
        hw.makeFirstResponder(c.webView)
        dlog("verdict=tab_docked_as_pane panes=\(g.tree.leaves.count) edge=\(edge.rawValue) zone=\(zone.isEmpty ? edge.rawValue : zone) grid=\(g.tree.encoded)")
        saveWindowsSoon()
        return true
    }

    /// Takes `c` out of its grid: back to a tab of that window, closed, or into
    /// another pane's zone. A window left with one page is a plain window again.
    func undockPane(_ c: DashboardWindowController, to dest: PaneDestination) {
        guard let g = c.paneHost?.grid ?? c.grid, let cid = g.id(of: c) else { return }
        // A move's target, by its page: removing this pane can renumber the
        // grid or collapse it to a plain window.
        var targetPage: DashboardWindowController? = nil
        if case .move(let t) = dest { targetPage = t.paneId.flatMap { t.host.grid?.members[$0] } ?? t.host }
        // The host's own page: another pane's controller takes over the window
        // first, so the window, its place and its tab stay.
        if g.host === c {
            guard let heir = g.members.first(where: { $0.key != cid })?.value else { return }
            rehost(g, to: heir)
        }
        guard let host = g.host, let hw = host.window else { return }
        lastTabPlaced = Date()
        g.syncFractions()
        g.members[cid] = nil
        g.views[cid]?.removeFromSuperview()
        g.tree = g.tree.removing(cid) ?? .leaf(g.members.keys.first ?? 0)
        c.paneHost = nil
        c.webView.removeFromSuperview()
        c.window?.contentView = c.webView
        if g.tree.leaves.count <= 1 {
            host.grid = nil
            g.container.removeFromSuperview()
            host.webView.removeFromSuperview()
            hw.contentView = host.webView
        } else {
            g.rebuild()
        }
        host.updateTitle()
        let left = host.grid?.tree.leaves.count ?? 1
        switch dest {
        case .tab:
            if let cw = c.window, cw !== hw, !(hw.tabbedWindows?.contains(cw) ?? false) { hw.addTabbedWindow(cw, ordered: .above) }
            c.window?.makeKeyAndOrderFront(nil)
            dlog("verdict=pane_to_tab panes_left=\(left)")
        case .close:
            c.window?.close()
            dlog("verdict=pane_closed panes_left=\(left)")
        case .move(let t):
            let page = targetPage ?? t.host
            let into = page.paneHost ?? page
            if !dockPane(c, into: into, at: into.grid?.id(of: page), edge: t.edge, zone: t.name) {
                if let cw = c.window, !(hw.tabbedWindows?.contains(cw) ?? false) { hw.addTabbedWindow(cw, ordered: .above) }
                c.window?.makeKeyAndOrderFront(nil)
            }
        }
        saveWindowsSoon()
    }

    /// `heir` (a pane) takes over the grid's window: its hidden window takes the
    /// host window's place and tab, and the grid moves into it. The old host
    /// stays a pane until the caller removes it.
    func rehost(_ g: PaneGrid, to heir: DashboardWindowController) {
        guard let old = g.host, let ow = old.window, let nw = heir.window else { return }
        nw.setFrame(ow.frame, display: false)
        nw.contentView = g.container
        ow.contentView = NSView()
        ow.addTabbedWindow(nw, ordered: .above)
        ow.tabGroup?.selectedWindow = nw
        nw.makeKeyAndOrderFront(nil)
        heir.paneHost = nil
        heir.grid = g
        old.grid = nil
        old.paneHost = heir
        g.host = heir
        for (_, m) in g.members where m !== heir { m.paneHost = heir }
        heir.home = old.home
        dlog("verdict=pane_grid_rehosted panes=\(g.tree.leaves.count)")
    }

    /// A grid's window closed: its panes close with it.
    func gridHostClosed(_ c: DashboardWindowController) {
        guard let g = c.grid else { return }
        c.grid = nil
        for (_, m) in g.members where m !== c {
            m.paneHost = nil
            m.window?.close()
        }
        dlog("verdict=pane_grid_closed panes=\(g.members.count)")
    }

    /// The window a tab tore off into, finished by a drop on a pane zone.
    func finishPaneDrop(_ drop: PaneDropTarget, at p: NSPoint, from group: [NSWindow], source: NSRect) {
        let alone = group.filter { w in controllers.contains { $0.window === w } && w.isVisible && (w.tabbedWindows?.count ?? 1) == 1 }
        guard let tw = alone.first(where: { abs($0.frame.minX - source.minX) > 1 || abs($0.frame.maxY - source.maxY) > 1 }),
              let torn = controllers.first(where: { $0.window === tw }) else {
            dlog("verdict=tab_drag_not_torn_off zone=pane")
            return
        }
        // Dropped on the window it came from: that window is now whichever of
        // its tabs is showing.
        var host = drop.host
        if host === torn {
            guard let w = group.first(where: { $0 !== tw && $0.isVisible && ($0.tabGroup?.selectedWindow === $0 || $0.tabbedWindows == nil) }),
                  let h = controllers.first(where: { $0.window === w }) else { dlog("verdict=pane_dock_refused reason=no_host"); return }
            host = h
        }
        let again = paneDropTarget(at: p, excluding: [tw])
        let t = again?.host === host ? again! : drop
        dockPane(torn, into: host, at: t.host === host ? t.paneId : nil, edge: t.edge, zone: t.name)
    }

    /// Saved with the workspace: the grid's tree (the host's page is 0, the
    /// panes 1... in reading order) and each pane's page.
    func savedGrid(_ c: DashboardWindowController) -> SavedGrid? {
        guard let g = c.grid, g.host === c else { return nil }
        g.syncFractions()
        let others = g.tree.leaves.filter { g.members[$0] !== c }
        var index: [Int: Int] = [:]
        if let hid = g.id(of: c) { index[hid] = 0 }
        for (i, id) in others.enumerated() { index[id] = i + 1 }
        let panes = others.compactMap { id -> SavedPane? in
            guard let m = g.members[id], let url = m.savedURL() else { return nil }
            let t = m.window?.title ?? ""
            return SavedPane(url: url, title: t.isEmpty || t == "amux" ? nil : t)
        }
        guard panes.count == others.count else { return nil }
        return SavedGrid(tree: g.tree.mapped { index[$0] ?? 0 }.encoded, panes: panes)
    }

    /// Rebuilds a saved grid in `host`'s window, reusing open controllers
    /// showing the same pages.
    func restoreGrid(_ s: SavedGrid, in host: DashboardWindowController, pool: inout [DashboardWindowController]) {
        guard let tree = PaneTree.decode(s.tree), let hw = host.window, tree.leaves.contains(0),
              Set(tree.leaves) == Set(0...s.panes.count) else {
            dlog("verdict=pane_grid_restore_skipped reason=bad_tree tree=\(s.tree)")
            return
        }
        let g = PaneGrid(host: host)
        for (i, p) in s.panes.enumerated() {
            let c: DashboardWindowController
            if let j = pool.firstIndex(where: { $0.savedURL() == p.url }) {
                c = pool.remove(at: j)
            } else {
                c = openWindow(url: URL(string: p.url), frame: hw.frame, activate: false)
                if let t = p.title, !t.isEmpty { c.pageLabel = t; c.updateTitle() }
            }
            if let cw = c.window {
                if (cw.tabbedWindows?.count ?? 1) > 1 { cw.tabGroup?.removeWindow(cw) }
                cw.orderOut(nil)
                cw.contentView = NSView()
            }
            c.paneHost = host
            g.members[i + 1] = c
        }
        g.tree = tree
        g.nextId = s.panes.count + 1
        host.grid = g
        g.container.frame = hw.contentView?.bounds ?? .zero
        hw.contentView = g.container
        g.rebuild()
        host.updateTitle()
        dlog("verdict=pane_grid_restored panes=\(tree.leaves.count) grid=\(s.tree)")
    }

    /// Before a workspace switch: every grid back to plain hidden windows, so
    /// their pages can be matched by URL like any other window.
    func dissolveGrids(_ cs: [DashboardWindowController]) {
        for h in cs {
            guard let g = h.grid, g.host === h, let hw = h.window else { continue }
            for (_, m) in g.members where m !== h {
                m.webView.removeFromSuperview()
                m.window?.contentView = m.webView
                m.paneHost = nil
            }
            h.webView.removeFromSuperview()
            hw.contentView = h.webView
            h.grid = nil
        }
    }
}
