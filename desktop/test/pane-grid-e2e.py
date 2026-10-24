#!/usr/bin/env python3
"""A tab dragged into an amux window's content splits THAT window into panes
(Ethan, 2026-10-23: "me dragging a tab into a section in the amux mac os app
window should create the grid").

    desktop/test/pane-grid-e2e.py [path/to/Amux.app]

Runs a second copy of the app (its own config, AMUX_DESKTOP_TEST=1) against
the local amux server, with one window of five tabs. One run walks the grid
through its life: two, three, four panes, a full grid refusing a fifth, a
divider moved, a quit and relaunch, a pane back to a tab, a pane dragged out,
a pane closed, the window's own page closed.

Real: the tab-bar press reaching tabPressed, the window under the pointer
(the window server's own z-order), the drop preview panel, finding the window
the tab became, the web views moving into split views, and the frames the
window server reports for each pane.

Simulated, as in tab-snap-e2e.py: the pointer (amux://test/pointer) and the
tear (moveTabToNewWindow, what AppKit does with a tab dropped outside every
tab bar), because macOS refuses this agent synthetic mouse events. A pane's
header drag and its buttons go through the same handlers a click reaches
(amux://test/pane-*), not through a real mouse.
"""
import json, os, signal, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
APP = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "..", "Amux.app")
BIN = os.path.join(APP, "Contents", "MacOS", "amux")
TMP = tempfile.mkdtemp(prefix="amux-panegrid-e2e-")
CFG, LOG, STATE = (os.path.join(TMP, n) for n in ("config.json", "app.log", "state.json"))
fails = 0


def sh(*a):
    return subprocess.run(a, capture_output=True, text=True)


def check(label, ok, detail=""):
    global fails
    print(("  ok   " if ok else "  FAIL ") + label + ("" if ok else f": {detail}"))
    fails += 0 if ok else 1


def rect(s):
    x, y, w, h = (float(v) for v in s.replace("{", "").replace("}", "").split(","))
    return x, y, w, h


def near(a, b, tol=4):
    return all(abs(p - q) <= tol for p, q in zip(a, b))


def log():
    try:
        return open(LOG, errors="replace").read()
    except FileNotFoundError:
        return ""


assert sh("xcrun", "swiftc", "-O", "-o", os.path.join(TMP, "wsctl"), os.path.join(HERE, "wsctl.swift")).returncode == 0
url = sh("amux", "url").stdout.strip() or "https://localhost:8824"
d0 = json.loads(sh(os.path.join(TMP, "wsctl"), "displays").stdout.splitlines()[0])
vx, vy, vw, vh = rect(d0["visible"])
src = (vx + vw * 0.08, vy + vh * 0.08, vw * 0.7, vh * 0.8)
src_s = "{{%d, %d}, {%d, %d}}" % src
TABS = ["board", "calendar", "map", "sessions", "messages"]


class App:
    def __init__(self, fresh=True):
        if fresh:
            with open(CFG, "w") as f:
                json.dump({"connections": [{"name": "localhost", "url": url}], "lastUrl": url, "cleanExit": True,
                           "windows": [{"url": f"{url}/#view={v}", "frame": src_s, "tabGroup": 1, "selected": v == "messages"}
                                       for v in TABS]}, f)
        n = log().count("verdict=windows_restored")
        self.p = subprocess.Popen([BIN], env=dict(os.environ, AMUX_DESKTOP_CONFIG=CFG, AMUX_DESKTOP_TEST="1",
                                  AMUX_DESKTOP_TEST_STATE=STATE), stdout=open(LOG, "a"), stderr=subprocess.STDOUT)
        end = time.time() + 25
        while log().count("verdict=windows_restored") <= n and time.time() < end:
            time.sleep(0.3)
        time.sleep(3)   # pages load and post their names
        self.send("activate", 0.5)

    def send(self, u, settle=0.3):
        sh(os.path.join(TMP, "wsctl"), "send", str(self.p.pid), "amux://test/" + u)
        time.sleep(settle)

    def state(self):
        before = os.path.getmtime(STATE) if os.path.exists(STATE) else 0
        self.send("state", 0)
        end = time.time() + 5
        while time.time() < end and (not os.path.exists(STATE) or os.path.getmtime(STATE) == before):
            time.sleep(0.05)
        return json.load(open(STATE))

    def stop(self):
        self.p.send_signal(signal.SIGTERM)
        try:
            self.p.wait(15)
        except subprocess.TimeoutExpired:
            self.p.kill()


def grid_window(st):
    g = [w for w in st["windows"] if "grid" in w]
    return g[0] if g else None


def drag_tab(app, title, x, y):
    """Press in the tab bar, move to (x, y) with the button down, drop the tab
    called `title` there. Returns the state seen mid-drag."""
    app.send("activate", 0.3)
    app.send(f"pointer?x={src[0] + src[2] / 2}&y={src[1] + src[3] - 10}&down=1", 0.1)
    app.send("press-tab", 0.1)
    app.send(f"pointer?x={x}&y={y}&down=1", 0.5)
    mid = app.state()
    app.send(f"drop-tab?title={title}", 1.5)
    app.send("pointer-clear", 0.2)
    return mid


app = App()
try:
    st = app.state()
    titles = sorted(w["title"] for w in st["windows"])
    check("starts as one window of five tabs", len(st["windows"]) == 5 and all(w["tabs"] == 5 for w in st["windows"]), str(st["windows"]))
    cx, cy, cw, ch = rect(st["content_frame"])

    print("1. the showing tab dragged into its own window's right half: two panes")
    mid = drag_tab(app, "Messages", cx + cw * 0.9, cy + ch * 0.5)
    want = (cx + cw / 2, cy, cw / 2, ch)
    check("the preview shows the right half of the content", mid["preview"] and near(rect(mid["preview_frame"]), want), f"{mid['preview_frame']} want {want}")
    st = app.state()
    g = grid_window(st)
    check("one window holds a grid of two", g is not None and g["grid"] == "r0.500(0,1)", str(st["windows"]))
    check("Messages is the right pane", g is not None and g["panes"][1] == "Messages", str(g))
    check("the right pane fills the right half", g is not None and near(rect(g["pane_frames"][1]), want, 6), str(g and g["pane_frames"]))
    check("the tab left the tab bar (4 tabs, no window for it)", g is not None and g["tabs"] == 4 and len(st["windows"]) == 4, str(st["windows"]))
    check("its own window is hidden, its page kept", st["hidden_panes"] == 1, str(st["hidden_panes"]))
    check("no preview after the drop", not st["preview"])
    check("the dock is logged", "verdict=tab_docked_as_pane panes=2 edge=right" in log())

    print("2. a tab dropped in the right pane's top right corner: that pane splits, new one on top")
    right = rect(g["pane_frames"][1])
    drag_tab(app, "Calendar", right[0] + right[2] * 0.92, right[1] + right[3] * 0.92)
    st = app.state()
    g = grid_window(st)
    check("three panes, the right column split", g is not None and g["grid"] == "r0.500(0,c0.500(1,2))", str(g))
    check("Calendar on top of Messages", g is not None and g["panes"][1:] == ["Calendar", "Messages"], str(g and g["panes"]))
    check("top right quarter", g is not None and near(rect(g["pane_frames"][1]), (cx + cw / 2, cy + ch / 2, cw / 2, ch / 2), 30), str(g and g["pane_frames"]))

    print("3. a tab dropped on the left pane's lower part: four panes")
    left = rect(g["pane_frames"][0])
    drag_tab(app, "Map", left[0] + left[2] * 0.5, left[1] + left[3] * 0.1)
    st = app.state()
    g = grid_window(st)
    check("four panes, two columns of two", g is not None and g["grid"] == "r0.500(c0.500(0,1),c0.500(2,3))", str(g))

    print("4. a full grid: a fifth tab gets no pane zone and tears off as a window")
    before = g["grid"] if g else ""
    # The one tab still in the tab bar (AppKit showed the next tab after each tear, so which it is varies).
    rest = [w["title"] for w in st["windows"] if "grid" not in w and w["tabs"] > 1]
    mid = drag_tab(app, rest[0] if rest else "Board", cx + cw * 0.3, cy + ch * 0.6)
    st = app.state()
    g = grid_window(st)
    check("no pane preview over a full grid", not mid["preview"] or not near(rect(mid["preview_frame"]), (cx, cy, cw / 2, ch), 30), mid["preview_frame"])
    check("the grid is unchanged", g is not None and g["grid"] == before, str(g))
    check("the fifth tab is its own window", any("grid" not in w and w["tabs"] == 1 for w in st["windows"]) and g is not None and g["tabs"] == 1, str(st["windows"]))

    print("5. a divider moved, then a quit and relaunch: the grid comes back as left")
    app.send("pane-divider?f=0.3", 1.5)
    st = app.state()
    g = grid_window(st)
    check("the split moved to 0.3", g is not None and g["grid"].startswith("r0.300("), str(g))
    panes_before = g["panes"] if g else []
    grid_before = g["grid"] if g else ""
    time.sleep(2)
finally:
    app.stop()

saved = json.load(open(CFG))
sg = [w for w in saved.get("windows", []) if w.get("grid")]
check("the grid is saved with the window", len(sg) == 1 and sg[0]["grid"]["tree"].startswith("r0.300(") and len(sg[0]["grid"]["panes"]) == 3, str(sg))

app = App(fresh=False)
try:
    st = app.state()
    g = grid_window(st)
    print("    after relaunch:", [(w["title"], w["tabs"], w.get("grid")) for w in st["windows"]])
    check("relaunched with the same grid", g is not None and g["grid"] == grid_before, f"{g and g['grid']} want {grid_before}")
    check("the same pages in the same panes", g is not None and g["panes"] == panes_before, f"{g and g['panes']} want {panes_before}")
    check("the panes' own windows stay hidden", st["hidden_panes"] == 3, str(st["hidden_panes"]))
    check("the restore is logged", "verdict=pane_grid_restored panes=4" in log())

    app.send("activate", 0.3)
    print("6. a pane's tab button: back to a tab")
    tabs0 = g["tabs"] if g else 0
    app.send("pane-tab?i=1", 1)
    st = app.state()
    g = grid_window(st)
    check("three panes left", g is not None and len(g["panes"]) == 3, str(g))
    check("one more tab in the window", any(w["tabs"] == tabs0 + 1 for w in st["windows"]), str(st["windows"]))

    print("7. a pane's header dragged out of every window: a tab again")
    x0, y0, w0, h0 = rect(st["content_frame"])
    app.send(f"pane-drag?i=1&x={vx + vw - 5}&y={vy + 5}", 1)
    st = app.state()
    g = grid_window(st)
    check("two panes left", g is not None and len(g["panes"]) == 2, str(g))

    print("8. a pane's header dragged onto the other pane's top: the grid rearranges")
    g0 = rect(g["pane_frames"][0]) if g else (0, 0, 0, 0)
    app.send(f"pane-drag?i=1&x={g0[0] + g0[2] * 0.5}&y={g0[1] + g0[3] * 0.92}", 1)
    st = app.state()
    g = grid_window(st)
    check("still two panes, now one above the other", g is not None and g["grid"].startswith("c0.500("), str(g))

    print("9. a pane closed: one page left, a plain window again")
    n_ctrl = len(st["windows"])
    app.send("pane-close?i=1", 1)
    st = app.state()
    check("no grid left", grid_window(st) is None, str(st["windows"]))
    check("the closed pane's page is gone, not a tab", len(st["windows"]) == n_ctrl, str(st["windows"]))
    check("the close is logged", "verdict=pane_closed panes_left=1" in log())
finally:
    app.stop()

print("10. the window's own page closed: the next pane takes the window")
app = App()
try:
    st = app.state()
    cx, cy, cw, ch = rect(st["content_frame"])
    drag_tab(app, "Messages", cx + cw * 0.9, cy + ch * 0.5)
    st = app.state()
    g = grid_window(st)
    frame0 = g["frame"] if g else ""
    host_page = g["panes"][0] if g else ""
    app.send("pane-close?i=0", 1)
    st = app.state()
    w = [w for w in st["windows"] if w["title"] == "Messages"]
    check("Messages now has the window, at the same place", len(w) == 1 and near(rect(w[0]["frame"]), rect(frame0)), f"{st['windows']} frame {frame0}")
    check(f"{host_page} is gone", not any(x["title"] == host_page for x in st["windows"]), str(st["windows"]))
    check("the takeover is logged", "verdict=pane_grid_rehosted" in log())
finally:
    app.stop()

print(f"PASS: pane-grid, all checks passed" if fails == 0 else f"pane-grid: {fails} check(s) FAILED (log {LOG})")
sys.exit(1 if fails else 0)
