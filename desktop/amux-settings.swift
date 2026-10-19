import Cocoa
import WebKit

// Settings (amux > Settings…, Cmd-,) and saved window layouts.
//
// A layout is the set of dashboard windows (what each shows, where it sits,
// which ones are tabs together) plus the floating pins, under a name. Opening
// one replaces the current windows, so a "morning triage" or "two workers side
// by side" arrangement comes back exactly as it was saved.

struct SavedPin: Codable {
    var paths: [String]
    var frame: String
    var opacity: Double
}

struct Layout: Codable {
    var name: String
    var windows: [SavedWindow]
    var pins: [SavedPin]?
    var savedAt: Double
}

// ── Layouts ──
extension AppDelegate {
    func snapshotPins() -> [SavedPin] {
        overlays.map { o in
            SavedPin(paths: o.files.map { $0.path },
                     frame: NSStringFromRect(o.contentWindow.frame),
                     opacity: Double(o.contentWindow.alphaValue))
        }
    }

    func saveLayout(named name: String, _ done: (() -> Void)? = nil) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        snapshotWindows { [weak self] windows in
            guard let self = self else { return }
            var c = loadConfig()
            var layouts = c.layouts ?? []
            let l = Layout(name: trimmed, windows: windows, pins: self.snapshotPins(), savedAt: Date().timeIntervalSince1970)
            if let i = layouts.firstIndex(where: { $0.name == trimmed }) { layouts[i] = l } else { layouts.append(l) }
            c.layouts = layouts
            saveConfig(c)
            self.config = c
            NSLog("amux-desktop: verdict=layout_saved name=%@ windows=%d pins=%d", trimmed, windows.count, l.pins?.count ?? 0)
            self.rebuildLayoutsMenu()
            done?()
        }
    }

    /// Replace the open windows and pins with the layout's.
    func openLayout(named name: String) {
        guard let l = (loadConfig().layouts ?? []).first(where: { $0.name == name }) else { return }
        for c in controllers { c.window?.close() }
        closeAllPins()
        openWindows(l.windows)
        for p in l.pins ?? [] where !p.paths.isEmpty {
            let f = NSRectFromString(p.frame)
            let o = OverlayController(filePaths: p.paths, x: Int(f.minX), y: Int(f.minY),
                                      w: Int(max(f.width, 200)), h: Int(max(f.height, 150)), opacity: p.opacity)
            overlays.append(o)
        }
        NSLog("amux-desktop: verdict=layout_opened name=%@ windows=%d pins=%d", name, l.windows.count, l.pins?.count ?? 0)
        NSApp.activate(ignoringOtherApps: true)
    }

    func openWindows(_ saved: [SavedWindow]) {
        guard serverURL != nil, !saved.isEmpty else {
            openWindow(url: nil, frame: nil)
            return
        }
        var groups: [Int: DashboardWindowController] = [:]
        for s in saved {
            let frame = NSRectFromString(s.frame)
            let c = openWindow(url: URL(string: s.url), frame: frame.width > 100 ? frame : nil)
            if let g = s.tabGroup {
                if let lead = groups[g], let lw = lead.window, let cw = c.window {
                    lw.addTabbedWindow(cw, ordered: .above)
                } else {
                    groups[g] = c
                }
            }
        }
    }

    func deleteLayout(named name: String) {
        var c = loadConfig()
        c.layouts = (c.layouts ?? []).filter { $0.name != name }
        if c.launch == "layout:" + name { c.launch = nil }
        saveConfig(c)
        config = c
        rebuildLayoutsMenu()
    }

    func renameLayout(_ old: String, to new: String) {
        let n = new.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty, n != old else { return }
        var c = loadConfig()
        var layouts = (c.layouts ?? []).filter { $0.name != n }
        if let i = layouts.firstIndex(where: { $0.name == old }) { layouts[i].name = n }
        c.layouts = layouts
        if c.launch == "layout:" + old { c.launch = "layout:" + n }
        saveConfig(c)
        config = c
        rebuildLayoutsMenu()
    }

    /// What launch opens: the last windows (default), a saved layout, or one window.
    func launchWindows() {
        let launch = config.launch ?? "last"
        if launch.hasPrefix("layout:"), (config.layouts ?? []).contains(where: { "layout:" + $0.name == launch }) {
            openLayout(named: String(launch.dropFirst("layout:".count)))
        } else if launch == "new" {
            openWindow(url: nil, frame: nil)
        } else {
            openWindows(config.windows ?? [])
        }
    }

    @objc func saveLayoutPrompt(_ sender: Any?) {
        let existing = (loadConfig().layouts ?? []).map { $0.name }
        var n = existing.count + 1
        while existing.contains("Layout \(n)") { n += 1 }
        guard let name = askText(title: "Save Window Layout",
                                 message: "Saves every open amux window (what it shows and where it sits) and the floating pins. A name that already exists is replaced.",
                                 value: "Layout \(n)", ok: "Save") else { return }
        saveLayout(named: name)
    }

    @objc func openLayoutItem(_ sender: NSMenuItem) {
        openLayout(named: sender.representedObject as? String ?? sender.title)
    }

    func askText(title: String, message: String, value: String, ok: String) -> String? {
        NSApp.activate(ignoringOtherApps: true)
        let a = NSAlert()
        a.messageText = title
        a.informativeText = message
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 300, height: 24))
        field.stringValue = value
        a.accessoryView = field
        a.addButton(withTitle: ok)
        a.addButton(withTitle: "Cancel")
        a.window.initialFirstResponder = field
        return a.runModal() == .alertFirstButtonReturn ? field.stringValue : nil
    }

    func rebuildLayoutsMenu() {
        layoutsMenu.removeAllItems()
        let save = NSMenuItem(title: "Save Window Layout…", action: #selector(saveLayoutPrompt(_:)), keyEquivalent: "s")
        save.keyEquivalentModifierMask = [.command, .option]
        save.target = self
        layoutsMenu.addItem(save)
        let layouts = loadConfig().layouts ?? []
        if !layouts.isEmpty { layoutsMenu.addItem(.separator()) }
        for l in layouts {
            let item = NSMenuItem(title: l.name, action: #selector(openLayoutItem(_:)), keyEquivalent: "")
            item.representedObject = l.name
            item.target = self
            item.toolTip = "\(l.windows.count) window\(l.windows.count == 1 ? "" : "s"), \(l.pins?.count ?? 0) pin\((l.pins?.count ?? 0) == 1 ? "" : "s")"
            layoutsMenu.addItem(item)
        }
        layoutsMenu.addItem(.separator())
        let manage = NSMenuItem(title: "Manage Layouts…", action: #selector(showSettings(_:)), keyEquivalent: "")
        manage.target = self
        layoutsMenu.addItem(manage)
    }

    @objc func showSettings(_ sender: Any?) {
        if settings == nil { settings = SettingsWindowController(app: self) }
        settings?.reload()
        settings?.showWindow(nil)
        settings?.window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}

// ── Settings window ──
final class SettingsWindowController: NSWindowController, WKScriptMessageHandler, WKNavigationDelegate {
    weak var app: AppDelegate?
    let webView: WKWebView

    init(app: AppDelegate) {
        self.app = app
        let cfg = WKWebViewConfiguration()
        webView = WKWebView(frame: .zero, configuration: cfg)
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 620, height: 680),
                         styleMask: [.titled, .closable, .miniaturizable, .resizable],
                         backing: .buffered, defer: false)
        w.title = "amux Settings"
        w.isReleasedWhenClosed = false
        w.minSize = NSSize(width: 480, height: 420)
        w.contentView = webView
        w.center()
        w.setFrameAutosaveName("amuxSettings")
        super.init(window: w)
        cfg.userContentController.add(WeakScriptHandler(self), name: "amuxSettings")
        webView.navigationDelegate = self
        webView.setValue(false, forKey: "drawsBackground")
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func reload() {
        guard let app = app else { return }
        webView.loadHTMLString(settingsHTML(app), baseURL: nil)
    }

    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let app = app, let m = message.body as? [String: Any], let action = m["action"] as? String else { return }
        let name = m["name"] as? String ?? ""
        var cfg = loadConfig()
        switch action {
        case "open": app.openLayout(named: name)
        case "update": app.saveLayout(named: name) { [weak self] in self?.reload() }; return
        case "save": app.saveLayout(named: name) { [weak self] in self?.reload() }; return
        case "delete": app.deleteLayout(named: name)
        case "rename":
            if let new = app.askText(title: "Rename Layout", message: "", value: name, ok: "Rename") {
                app.renameLayout(name, to: new)
            }
        case "launch":
            cfg.launch = m["value"] as? String
            saveConfig(cfg)
            app.config = cfg
        case "toggle":
            let on = m["value"] as? Bool ?? true
            switch name {
            case "menuBar": cfg.menuBar = on
            case "notifications": cfg.notifications = on
            case "dockBadge": cfg.dockBadge = on
            default: break
            }
            saveConfig(cfg)
            app.config = cfg
            app.applyBadge()
            if name == "menuBar" { app.startMenuBarItem() }
        case "server":
            (app.keyController ?? app.openWindow(url: nil, frame: nil)).showConnectPage()
        default: break
        }
        reload()
    }
}

func htmlEscape(_ s: String) -> String {
    s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;").replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&#39;")
}

func jsString(_ s: String) -> String {
    let data = (try? JSONSerialization.data(withJSONObject: [s])) ?? Data("[\"\"]".utf8)
    let arr = String(data: data, encoding: .utf8) ?? "[\"\"]"
    return htmlEscape(String(arr.dropFirst().dropLast()))
}

func settingsHTML(_ app: AppDelegate) -> String {
    let c = loadConfig()
    let layouts = c.layouts ?? []
    let launch = c.launch ?? "last"
    let df = DateFormatter()
    df.dateStyle = .medium
    df.timeStyle = .short
    let rows = layouts.isEmpty
        ? "<div class=\"empty\">No saved layouts yet. Arrange your windows, then save them below.</div>"
        : layouts.map { l -> String in
            let n = jsString(l.name)
            let pins = l.pins?.count ?? 0
            let what = "\(l.windows.count) window\(l.windows.count == 1 ? "" : "s")" + (pins > 0 ? ", \(pins) pin\(pins == 1 ? "" : "s")" : "")
            let shows = l.windows.compactMap { w -> String? in
                guard let r = w.url.range(of: "#") else { return nil }
                let h = String(w.url[r.upperBound...])
                let v = h.split(separator: "=", maxSplits: 1).last.map(String.init) ?? h
                return v.removingPercentEncoding ?? v
            }.joined(separator: ", ")
            return """
            <div class="row">
              <div class="info"><div class="name">\(htmlEscape(l.name))</div>
                <div class="meta">\(what) · saved \(df.string(from: Date(timeIntervalSince1970: l.savedAt)))\(shows.isEmpty ? "" : "<br>" + htmlEscape(shows))</div></div>
              <button class="primary" onclick="send('open',\(n))">Open</button>
              <button onclick="send('update',\(n))" title="Replace with the windows open now">Update</button>
              <button onclick="send('rename',\(n))">Rename</button>
              <button class="danger" onclick="send('delete',\(n))">Delete</button>
            </div>
            """
        }.joined()
    let launchOptions = [("last", "Reopen the windows from last time"), ("new", "Open one new window")]
        .map { "<option value=\"\($0.0)\"\(launch == $0.0 ? " selected" : "")>\($0.1)</option>" }.joined()
        + layouts.map { l in
            let v = "layout:" + l.name
            return "<option value=\"\(htmlEscape(v))\"\(launch == v ? " selected" : "")>Open layout: \(htmlEscape(l.name))</option>"
        }.joined()
    func toggle(_ key: String, _ on: Bool, _ label: String, _ hint: String) -> String {
        """
        <label class="toggle"><input type="checkbox" \(on ? "checked" : "") onchange="send('toggle','\(key)',this.checked)">
          <span><b>\(label)</b><br><small>\(hint)</small></span></label>
        """
    }
    let server = htmlEscape(app.serverURL?.absoluteString ?? "not connected")
    return """
    <!DOCTYPE html><html><head><meta charset="utf-8">
    <style>
      :root { color-scheme: light dark; --bg:#f6f8fa; --card:#fff; --text:#1f2328; --dim:#59636e; --border:#d1d9e0; --accent:#0969da; --danger:#cf222e; }
      @media (prefers-color-scheme: dark) { :root { --bg:#0d1117; --card:#161b22; --text:#e6edf3; --dim:#8b949e; --border:#30363d; --accent:#2f81f7; --danger:#f85149; } }
      * { box-sizing: border-box; }
      body { margin:0; padding:24px; font: 13px -apple-system, BlinkMacSystemFont, sans-serif; background:var(--bg); color:var(--text); }
      h2 { font-size: 13px; text-transform: uppercase; letter-spacing: .04em; color: var(--dim); margin: 22px 0 8px; }
      h2:first-child { margin-top: 0; }
      .card { background: var(--card); border: 1px solid var(--border); border-radius: 10px; padding: 4px 14px; }
      .row { display:flex; align-items:center; gap:8px; padding:10px 0; border-bottom:1px solid var(--border); }
      .row:last-child { border-bottom: none; }
      .info { flex:1; min-width:0; }
      .name { font-weight:600; font-size:14px; }
      .meta { color: var(--dim); font-size: 12px; margin-top: 2px; overflow-wrap: anywhere; }
      .empty { color: var(--dim); padding: 12px 0; }
      button { font: inherit; padding: 5px 11px; border-radius: 6px; border: 1px solid var(--border); background: var(--card); color: var(--text); cursor: pointer; }
      button:hover { border-color: var(--accent); }
      button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
      button.danger { color: var(--danger); }
      .save { display:flex; gap:8px; padding: 12px 0; }
      input[type=text], select { flex:1; font: inherit; padding: 6px 10px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
      .toggle { display:flex; gap:10px; align-items:flex-start; padding: 10px 0; border-bottom: 1px solid var(--border); cursor: pointer; }
      .toggle:last-child { border-bottom: none; }
      .toggle input { margin-top: 3px; }
      small { color: var(--dim); }
      kbd { font: 12px ui-monospace, monospace; border: 1px solid var(--border); border-radius: 4px; padding: 1px 5px; }
    </style></head><body>
    <h2>Window layouts</h2>
    <div class="card">
      \(rows)
      <div class="save"><input type="text" id="lname" placeholder="Name this layout, e.g. Morning triage"
        onkeydown="if(event.key==='Enter')saveNew()"><button class="primary" onclick="saveNew()">Save current windows</button></div>
    </div>
    <h2>When amux opens</h2>
    <div class="card"><div class="save"><select onchange="send('launch','',this.value)">\(launchOptions)</select></div></div>
    <h2>General</h2>
    <div class="card">
      \(toggle("menuBar", c.menuBar ?? true, "Show in menu bar", "CPU and memory readout, with your pinned notes one click away."))
      \(toggle("notifications", c.notifications ?? true, "Notify when amux needs you", "A notification for each new item waiting on you."))
      \(toggle("dockBadge", c.dockBadge ?? true, "Show count on Dock icon", "The number of items waiting on you."))
    </div>
    <h2>Server</h2>
    <div class="card"><div class="row"><div class="info"><div class="name">\(server)</div>
      <div class="meta">Every window connects here.</div></div><button onclick="send('server','')">Change…</button></div></div>
    <h2>Shortcuts</h2>
    <div class="card"><div class="row"><div class="info">
      <kbd>⌃⌥Space</kbd> show or hide amux · <kbd>⌘N</kbd> new window · <kbd>⌘T</kbd> new tab ·
      <kbd>⇧⌘N</kbd> current view in a new window · <kbd>⌥⌘S</kbd> save window layout</div></div></div>
    <script>
      function send(action, name, value) { window.webkit.messageHandlers.amuxSettings.postMessage({action, name, value}); }
      function saveNew() { const v = document.getElementById('lname').value.trim(); if (v) send('save', v); }
    </script>
    </body></html>
    """
}
