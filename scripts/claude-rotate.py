#!/usr/bin/env python3
"""Keep every Claude subscription account fully used by switching the one
machine-wide Claude CLI login to the account whose capacity expires soonest.

All workers share one login (the CLI's default credential), and a running
worker picks up a new login without a restart, so a switch disrupts nothing.

Subcommands:
  statusline     Claude Code status line command. Records the live rate_limits
                 reading under the logged-in account and prints a short line.
  pick [--json]  Rank accounts from the saved readings.
  tick           Switch when the policy says so. Run every few minutes.
  switch EMAIL   Log the CLI into EMAIL: `claude auth login --email` with the
                 browser page sent to the Chrome profile that holds the
                 accounts, email sign-in link read from the forwarded inbox.
  record EMAIL 5H 5H_RESET WK WK_RESET   Save a reading by hand (ISO times).
  status         Show the current account, its reading and the last switch.

State lives in ~/.amux/claude-usage (accounts.json, state.json). Every decision
is one JSON line in ~/.amux/logs/claude-rotate.log with a `verdict`.

Policy, readings for an account that is not logged in can only fall, so a
stale reading is a safe upper bound, and a window whose reset has passed counts
as 0. An account is usable while both windows have MIN_HEADROOM below
SWITCH_AT. Among usable accounts the one whose weekly window resets first
wins, since its headroom is lost at that reset. tick switches when the current
account stops being usable, or when another usable account's weekly reset is
earlier than the current one's (spend expiring capacity first). Unknown
accounts are used only after every known one.
"""
import datetime as dt
import fcntl
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.parse

HOME = os.path.expanduser("~")
DIR = os.environ.get("CLAUDE_ROTATE_DIR", os.path.join(HOME, ".amux", "claude-usage"))
ACCOUNTS = os.path.join(DIR, "accounts.json")
STATE = os.path.join(DIR, "state.json")
LOCK = os.path.join(DIR, "switch.lock")
LOG = os.environ.get("CLAUDE_ROTATE_LOG", os.path.join(HOME, ".amux", "logs", "claude-rotate.log"))
CLAUDE_JSON = os.environ.get("CLAUDE_ROTATE_CLAUDE_JSON", os.path.join(HOME, ".claude.json"))
SWITCH_AT = float(os.environ.get("CLAUDE_SWITCH_AT", "99"))
MIN_HEADROOM = float(os.environ.get("CLAUDE_MIN_HEADROOM", "2"))
# Readings arriving this soon after a switch may still describe the previous
# account (the CLI reports headers from its last response), so they are dropped.
SETTLE_S = int(os.environ.get("CLAUDE_ROTATE_SETTLE_S", "180"))
MIN_SWITCH_GAP_S = int(os.environ.get("CLAUDE_ROTATE_MIN_GAP_S", "900"))
CHROME_PROFILE = os.environ.get("CLAUDE_ROTATE_CHROME_PROFILE", "Profile 16")
CLAUDE = os.environ.get("CLAUDE_ROTATE_CLAUDE", "claude")
SWITCH_TIMEOUT_S = int(os.environ.get("CLAUDE_ROTATE_SWITCH_TIMEOUT_S", "240"))


def now():
    return dt.datetime.now().astimezone()


def log(verdict, **fields):
    os.makedirs(os.path.dirname(LOG), exist_ok=True)
    line = json.dumps({"ts": now().isoformat(timespec="seconds"), "verdict": verdict, **fields})
    with open(LOG, "a") as f:
        f.write(line + "\n")
    return line


def load(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def save(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".tmp-")
    with os.fdopen(fd, "w") as f:
        json.dump(data, f, indent=2)
    os.replace(tmp, path)


class locked:
    """Exclusive lock on a file; blocking unless nowait."""

    def __init__(self, path, nowait=False):
        self.path, self.nowait = path, nowait

    def __enter__(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        self.f = open(self.path, "a")
        try:
            fcntl.flock(self.f, fcntl.LOCK_EX | (fcntl.LOCK_NB if self.nowait else 0))
        except BlockingIOError:
            self.f.close()
            return False
        return True

    def __exit__(self, *exc):
        if not self.f.closed:
            fcntl.flock(self.f, fcntl.LOCK_UN)
            self.f.close()


def current_email():
    """The logged-in account, from the CLI's own config (no credential read)."""
    acct = (load(CLAUDE_JSON, {}) or {}).get("oauthAccount") or {}
    return acct.get("emailAddress")


def cli_email():
    """The logged-in account as the CLI reports it."""
    try:
        out = subprocess.run([CLAUDE, "auth", "status", "--json"], capture_output=True, text=True, timeout=30).stdout
        return json.loads(out).get("email")
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return None


def iso(epoch):
    return dt.datetime.fromtimestamp(float(epoch)).astimezone().isoformat(timespec="seconds")


# ---------------------------------------------------------------- readings

def record(email, five=None, five_reset=None, week=None, week_reset=None, source="statusline"):
    with locked(ACCOUNTS + ".lock"):
        data = load(ACCOUNTS, {"accounts": {}})
        acct = data["accounts"].setdefault(email, {})
        acct["observed_at"] = now().isoformat(timespec="seconds")
        acct["source"] = source
        if five is not None:
            acct["five_hour"] = {"used_percentage": five, "resets_at": five_reset}
        if week is not None:
            acct["seven_day"] = {"used_percentage": week, "resets_at": week_reset}
        save(ACCOUNTS, data)


def cmd_statusline():
    try:
        payload = json.load(sys.stdin)
    except ValueError:
        payload = {}
    email = current_email()
    limits = payload.get("rate_limits") or {}
    five, week = limits.get("five_hour") or {}, limits.get("seven_day") or {}
    state = load(STATE, {})
    settled = time.time() - float(state.get("switched_at") or 0) >= SETTLE_S
    if email and (five or week) and settled:
        # At most one write per account per 30 s across the whole fleet.
        stamp = os.path.join(DIR, "last-record")
        try:
            fresh = time.time() - os.path.getmtime(stamp) < 30
        except OSError:
            fresh = False
        if not fresh:
            try:
                record(email,
                       five.get("used_percentage"), iso(five["resets_at"]) if five.get("resets_at") else None,
                       week.get("used_percentage"), iso(week["resets_at"]) if week.get("resets_at") else None)
                open(stamp, "w").close()
            except Exception as e:  # a status line must never fail the CLI
                log("claude_rotate_record_failed", error=str(e)[:200])
    name = (email or "?").split("@")[-1].split(".")[0]
    parts = [name]
    if five.get("used_percentage") is not None:
        parts.append("5h %d%%" % round(five["used_percentage"]))
    if week.get("used_percentage") is not None:
        parts.append("wk %d%%" % round(week["used_percentage"]))
    print(" · ".join(parts))


# ---------------------------------------------------------------- policy

def window(w, at):
    w = w or {}
    reset = w.get("resets_at")
    reset = dt.datetime.fromisoformat(reset) if reset else None
    used = w.get("used_percentage")
    if used is None:
        return None, reset
    return (0.0 if reset and reset <= at else float(used)), reset


def rank(at=None):
    at = at or now()
    rows = []
    for email, a in load(ACCOUNTS, {"accounts": {}})["accounts"].items():
        if a.get("disabled"):
            continue
        u5, r5 = window(a.get("five_hour"), at)
        u7, r7 = window(a.get("seven_day"), at)
        known = u5 is not None or u7 is not None
        head = min(SWITCH_AT - (u5 or 0), SWITCH_AT - (u7 or 0))
        usable = head >= MIN_HEADROOM
        blockers = [r for u, r in ((u5, r5), (u7, r7)) if u is not None and SWITCH_AT - u < MIN_HEADROOM and r]
        rows.append({"email": email, "known": known, "usable": usable, "headroom": round(head, 1),
                     "five_hour": u5, "weekly": u7,
                     "weekly_resets": r7.isoformat() if r7 else None,
                     "available_at": "now" if usable else (max(blockers).isoformat() if blockers else None),
                     "observed_at": a.get("observed_at")})
    far = "9999"
    rows.sort(key=lambda r: (not r["usable"], not r["known"],
                             (r["weekly_resets"] or far) if r["usable"] else (r["available_at"] or far)))
    return rows


def decide(current, rows, state, at=None):
    """Return (target or None, reason)."""
    at = at or now()
    best = next((r for r in rows if r["usable"]), None)
    cur = next((r for r in rows if r["email"] == current), None)
    if best is None:
        return None, "no_usable_account"
    if best["email"] == current:
        return None, "current_is_best"
    if cur is None or not cur["usable"]:
        return best["email"], "current_exhausted"
    since = time.time() - float(state.get("switched_at") or 0)
    if since < MIN_SWITCH_GAP_S:
        return None, "recent_switch"
    if best["known"] and cur["weekly_resets"] and best["weekly_resets"] and best["weekly_resets"] < cur["weekly_resets"]:
        return best["email"], "expiring_capacity_first"
    return None, "current_is_fine"


def cmd_pick(argv):
    rows = rank()
    pick = rows[0]["email"] if rows and rows[0]["usable"] else None
    if "--json" in argv:
        print(json.dumps({"measured": True, "n_considered": len(rows), "pick": pick, "ranked": rows}, indent=2))
        return
    print("pick:", pick or "NONE usable (wait for the earliest available_at)")
    for r in rows:
        print("  %s %-28s 5h=%-5s wk=%-5s headroom=%-5s weekly_resets=%s available=%s" % (
            "OK" if r["usable"] else "--", r["email"], r["five_hour"], r["weekly"], r["headroom"],
            r["weekly_resets"], r["available_at"]))


def cmd_tick():
    if os.environ.get("CLAUDE_ROTATE_ENABLED", "1") == "0" or os.path.exists(os.path.join(DIR, "paused")):
        print(log("claude_rotate_paused", measured=False, why_unmeasured="paused"))
        return 0
    current = current_email()
    rows = rank()
    state = load(STATE, {})
    target, reason = decide(current, rows, state)
    if not target:
        print(log("claude_rotate_hold", measured=True, n_considered=len(rows), current=current, reason=reason))
        return 0
    print(log("claude_rotate_switch_planned", measured=True, n_considered=len(rows), current=current,
              target=target, reason=reason))
    return switch(target, reason)


# ---------------------------------------------------------------- switching

def cdp(*args, check=True):
    env = dict(os.environ, CDP_PROFILE_SYNC="0")
    p = subprocess.run(["amux", "cdp", *args], capture_output=True, text=True, timeout=60, env=env)
    out = "\n".join(l for l in p.stdout.splitlines() if not l.startswith("[amux] saved"))
    if check and p.returncode != 0:
        raise RuntimeError("amux cdp %s failed: %s" % (args[0], (p.stderr or out)[-300:]))
    return out.strip()


def find_tab(marker):
    for line in cdp("list").splitlines():
        parts = line.split(None, 1)
        if len(parts) == 2 and marker in line:
            return parts[0]
    return None


def page_text(tab):
    return cdp("eval", tab, 'document.body ? document.body.innerText.slice(0, 2000) : ""', check=False)


def click_text(tab, pattern):
    js = ('(() => { const re = new RegExp(%s); const el = [...document.querySelectorAll("button,a,[role=button]")]'
          '.find(e => re.test(e.innerText.trim())); if (!el) return "none"; el.scrollIntoView({block: "center"});'
          ' const r = el.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + " " + Math.round(r.y + r.height / 2) })()'
          % json.dumps(pattern))
    pos = cdp("eval", tab, js, check=False).splitlines()[-1:] or ["none"]
    if pos[0] == "none" or not re.match(r"^\d+ \d+$", pos[0]):
        return False
    x, y = pos[0].split()
    cdp("clickxy", tab, x, y)
    return True


_last_trace = {}


def trace(target, tab, text):
    """One line per distinct page the switch sees, for diagnosing a failed run."""
    url = cdp("eval", tab, "location.href", check=False).splitlines()[-1:] or [""]
    key = (url[0].split("?")[0], text[:120])
    if _last_trace.get(tab) == key:
        return
    _last_trace[tab] = key
    with open(LOG.replace(".log", "-trace.log"), "a") as f:
        f.write(json.dumps({"ts": now().isoformat(timespec="seconds"), "target": target,
                            "url": url[0][:160], "text": re.sub(r"\s+", " ", text)[:300]}) + "\n")


def fill_email(tab, email):
    """Type the address into an empty email field. Returns True if it typed."""
    js = ('(() => { const i = document.querySelector("input[type=email]"); if (!i || i.value) return "skip";'
          ' i.focus(); i.select(); return "ok" })()')
    if cdp("eval", tab, js, check=False).splitlines()[-1:] != ["ok"]:
        return False
    cdp("type", tab, email)
    return True


def amux_get(path):
    url = subprocess.run(["amux", "url"], capture_output=True, text=True).stdout.strip()
    p = subprocess.run(["curl", "-sk", "--max-time", "20", url + path], capture_output=True, text=True)
    return json.loads(p.stdout or "null")


def magic_link(email, since_epoch):
    msgs = amux_get("/api/email/search?q=%s&days=1&limit=20" % urllib.parse.quote("secure link to Claude")) or []
    for m in msgs:
        if (m.get("to") or "").lower() != email.lower():
            continue
        try:
            sent = dt.datetime.fromisoformat(m.get("date_iso") or m.get("received") or "").timestamp()
        except ValueError:
            sent = None
        if sent is None:
            try:
                from email.utils import parsedate_to_datetime
                sent = parsedate_to_datetime(m.get("date")).timestamp()
            except Exception:
                continue
        if sent < since_epoch - 5:
            continue
        body = json.dumps(amux_get("/api/email/message/" + urllib.parse.quote(m["message_id"], safe="")))
        found = re.findall(r"https://claude\.ai/magic-link#[A-Za-z0-9_\-:.]+", body)
        if found:
            return found[0]
    return None


def switch(target, reason="manual"):
    with locked(LOCK, nowait=True) as got:
        if not got:
            print(log("claude_rotate_switch_busy", target=target))
            return 1
        return _switch(target, reason)


def _switch(target, reason):
    before = cli_email()
    if before == target:
        print(log("claude_rotate_switch_noop", target=target))
        return 0
    started = time.time()
    with tempfile.TemporaryDirectory(prefix="claude-rotate-") as tmp:
        url_file = os.path.join(tmp, "url")
        opener = os.path.join(tmp, "open.sh")
        with open(opener, "w") as f:
            f.write('#!/bin/sh\nprintf %%s "$1" > %s\nexec open -na "Google Chrome" --args --profile-directory=%s "$1"\n'
                    % (json.dumps(url_file), json.dumps(CHROME_PROFILE)))
        os.chmod(opener, 0o700)
        out_path = os.path.join(tmp, "cli.log")
        # `script` gives the CLI the terminal it expects; the old login stays
        # valid until this process completes, so killing it is a clean abort.
        proc = subprocess.Popen(["script", "-q", out_path, CLAUDE, "auth", "login", "--email", target],
                                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                env=dict(os.environ, BROWSER=opener))
        steps, tab, state, link_used = [], None, None, False
        try:
            deadline = started + SWITCH_TIMEOUT_S
            while time.time() < deadline:
                if proc.poll() is not None:
                    break
                time.sleep(2)
                if tab is None:
                    if not os.path.exists(url_file):
                        continue
                    st = urllib.parse.parse_qs(urllib.parse.urlparse(open(url_file).read()).query).get("state", [""])[0]
                    tab = find_tab(st[:20]) if st else None
                    if tab is None:
                        continue
                    steps.append("tab")
                text = page_text(tab)
                trace(target, tab, text)
                if re.search(r"Authorize", text) and "Logged in as" in text:
                    if ("Logged in as " + target) in text:
                        if click_text(tab, r"^Authorize$"):
                            steps.append("authorize")
                            time.sleep(4)
                    elif click_text(tab, r"^Switch account$"):
                        steps.append("switch-account")
                elif "Sign in here instead" in text:
                    if click_text(tab, r"^Sign in here instead$"):
                        steps.append("sign-in-here")
                elif "click the link sent to" in text or "verification code" in text.lower():
                    if not link_used:
                        link = magic_link(target, started)
                        if link:
                            cdp("nav", tab, link)
                            link_used = True
                            steps.append("magic-link")
                elif "Continue with email" in text:
                    if steps.count("continue-email") >= 3:
                        raise RuntimeError("login page did not advance after 3 email submissions")
                    # "Switch account" clears the prefilled address.
                    if fill_email(tab, target):
                        steps.append("fill-email")
                    if click_text(tab, r"^Continue with email$"):
                        steps.append("continue-email")
                        time.sleep(3)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                pass
        except Exception as e:
            steps.append("error: " + str(e)[:160])
        finally:
            if proc.poll() is None:
                proc.kill()
            if tab:
                cdp("close", tab, check=False)
            cli_out = open(out_path, errors="replace").read() if os.path.exists(out_path) else ""
    after = cli_email()
    ok = after == target
    elapsed = round(time.time() - started, 1)
    if ok:
        state = load(STATE, {})
        state.update({"switched_at": time.time(), "from": before, "to": target, "reason": reason})
        save(STATE, state)
        print(log("claude_rotate_switched", measured=True, from_=before, to=target, reason=reason,
                  elapsed_s=elapsed, steps=steps))
        return 0
    tail = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\]8;;[^\x07\x1b]*(\x07|\x1b\\)?", "", cli_out)[-300:]
    print(log("claude_rotate_switch_failed", measured=True, target=target, still=after, reason=reason,
              elapsed_s=elapsed, steps=steps, cli_tail=tail))
    return 1


# ---------------------------------------------------------------- main

def main(argv):
    cmd = argv[1] if len(argv) > 1 else "status"
    if cmd == "statusline":
        cmd_statusline()
        return 0
    if cmd == "pick":
        cmd_pick(argv[2:])
        return 0
    if cmd == "tick":
        return cmd_tick()
    if cmd == "switch" and len(argv) == 3:
        return switch(argv[2])
    if cmd == "record" and len(argv) == 7:
        _, _, email, f, fr, w, wr = argv
        record(email, float(f), fr or None, float(w), wr or None, source="manual")
        return 0
    if cmd == "status":
        state = load(STATE, {})
        email = current_email()
        cur = load(ACCOUNTS, {"accounts": {}})["accounts"].get(email, {})
        print(json.dumps({"current": email, "reading": cur, "last_switch": state}, indent=2))
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
