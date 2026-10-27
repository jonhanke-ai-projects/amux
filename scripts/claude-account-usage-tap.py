#!/usr/bin/env python3
"""Claude Code status-line tap: record each account's live usage, then hand off.

Installed by `amux claude-accounts track on` (POST /api/claude-accounts/track),
which puts this file at ~/.amux/bin/claude-account-usage-tap.py and sets
~/.claude/settings.json `statusLine.command` to

    python3 ~/.amux/bin/claude-account-usage-tap.py -- <your previous command>

Claude Code gives its status-line command a JSON payload on stdin that
includes `rate_limits` (5-hour and 7-day used percentage and reset time) for
the account that session is signed in to. Every account shares settings.json
(claude_accounts.rs links it), so this runs for every account's workers, and
CLAUDE_CONFIG_DIR says which account it is. No credential is read or stored:
the numbers are the ones Claude Code already shows.

Each reading goes to POST /api/usage/accounts/readings (table
claude_account_usage, the same history the upstream rotator writes), at most
once per account per 30 s, in a detached curl so the status line never waits.
Then the previous command runs with the same stdin and its output is the
status line. Without one, a short "acct · 5h n% · wk n%" line is printed.

A status line must never fail, so every error is swallowed.
"""
import datetime as dt
import hashlib
import json
import os
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
AMUX_HOME = os.environ.get("AMUX_HOME") or os.path.join(HOME, ".amux")
MIN_GAP_S = 30


def iso(v):
    if v is None or v == "":
        return None
    if isinstance(v, (int, float)):
        return dt.datetime.fromtimestamp(v, dt.timezone.utc).isoformat()
    return str(v)


def account_email():
    cfg = os.environ.get("CLAUDE_CONFIG_DIR")
    path = os.path.join(cfg, ".claude.json") if cfg else os.path.join(HOME, ".claude.json")
    try:
        with open(path) as f:
            return ((json.load(f).get("oauthAccount") or {}).get("emailAddress")) or None
    except (OSError, ValueError, AttributeError):
        return None


def amux_base():
    url = os.environ.get("AMUX_URL")
    if url:
        return url.rstrip("/")
    try:
        with open(os.path.join(AMUX_HOME, "endpoint.json")) as f:
            return json.load(f)["canonical_url"].rstrip("/")
    except (OSError, ValueError, KeyError, TypeError):
        return "https://localhost:8824"


def due(email):
    stamp_dir = os.path.join(AMUX_HOME, "claude-usage")
    stamp = os.path.join(stamp_dir, "tap-%s.stamp" % hashlib.sha256(email.encode()).hexdigest()[:12])
    try:
        if time.time() - os.path.getmtime(stamp) < MIN_GAP_S:
            return False
    except OSError:
        pass
    try:
        os.makedirs(stamp_dir, exist_ok=True)
        open(stamp, "w").close()
    except OSError:
        pass
    return True


def record(payload):
    limits = payload.get("rate_limits") or {}
    five, week = limits.get("five_hour") or {}, limits.get("seven_day") or {}
    email = account_email()
    if not email or (five.get("used_percentage") is None and week.get("used_percentage") is None):
        return email, five, week
    if due(email):
        body = {
            "account": email,
            "source": "statusline-tap",
            "at": time.time(),
            "five_hour_pct": five.get("used_percentage"),
            "five_hour_resets_at": iso(five.get("resets_at")),
            "seven_day_pct": week.get("used_percentage"),
            "seven_day_resets_at": iso(week.get("resets_at")),
        }
        subprocess.Popen(
            ["curl", "-sk", "--max-time", "5", "-X", "POST", "-H", "Content-Type: application/json",
             "-d", json.dumps(body), amux_base() + "/api/usage/accounts/readings"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    return email, five, week


def main(argv):
    raw = sys.stdin.buffer.read()
    email, five, week = None, {}, {}
    try:
        email, five, week = record(json.loads(raw or b"{}"))
    except Exception:
        pass
    nxt = argv[1:]
    if nxt and nxt[0] == "--":
        nxt = nxt[1:]
    if nxt:
        try:
            out = subprocess.run(" ".join(nxt), shell=True, input=raw, capture_output=True, timeout=10)
            sys.stdout.buffer.write(out.stdout)
            return 0
        except Exception:
            pass
    parts = [(email or "?").split("@")[0]]
    if five.get("used_percentage") is not None:
        parts.append("5h %d%%" % round(five["used_percentage"]))
    if week.get("used_percentage") is not None:
        parts.append("wk %d%%" % round(week["used_percentage"]))
    print(" · ".join(parts))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception:
        sys.exit(0)
