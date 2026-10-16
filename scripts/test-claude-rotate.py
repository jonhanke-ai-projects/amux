#!/usr/bin/env python3
"""claude-rotate switches BEFORE the current account hits its limit, and takes
workers parked on Claude's limit prompt back to work (Ethan, 2026-10-15:
"workers shouldn't never be rate limited, u needs to switch login before then";
amux-app sat on the weekly-limit prompt while three switches failed). Loads the
shipped script as a module; no network, no CLI."""
import importlib.util
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.environ["CLAUDE_ROTATE_DIR"] = tempfile.mkdtemp()
os.environ["CLAUDE_ROTATE_LOG"] = os.path.join(os.environ["CLAUDE_ROTATE_DIR"], "log")
spec = importlib.util.spec_from_file_location("rot", os.path.join(ROOT, "scripts", "claude-rotate.py"))
rot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rot)

PASS = FAIL = 0


def expect(cond, label):
    global PASS, FAIL
    if cond:
        PASS += 1
    else:
        FAIL += 1
        print("FAIL:", label)


def row(email, five, week, resets="2026-10-20T00:00:00-04:00"):
    head = min(rot.SWITCH_AT - five, rot.SWITCH_AT - week)
    return {"email": email, "known": True, "usable": head >= rot.MIN_HEADROOM, "headroom": head,
            "five_hour": five, "weekly": week, "weekly_resets": resets}


old = {"switched_at": 0}
# Under the preempt line: stay.
t, why = rot.decide("a", [row("a", 10, 70), row("b", 0, 20, "2026-10-25T00:00:00-04:00")], old)
expect(t is None, f"70% weekly holds ({why})")
# Over the preempt line, another account has more headroom: switch now, before the limit.
rows = [row("a", 10, 88, "2026-10-17T00:00:00-04:00"), row("b", 0, 20, "2026-10-25T00:00:00-04:00")]
rows.sort(key=lambda r: (not r["usable"], r["weekly_resets"]))
t, why = rot.decide("a", rows, old)
expect((t, why) == ("b", "preempt_limit"), f"88% weekly switches early, got {(t, why)}")
# The five-hour window counts too.
rows = [row("a", 90, 30, "2026-10-17T00:00:00-04:00"), row("b", 0, 20, "2026-10-25T00:00:00-04:00")]
t, why = rot.decide("a", rows, old)
expect((t, why) == ("b", "preempt_limit"), f"90% five-hour switches early, got {(t, why)}")
# Over the line but nothing better: hold rather than flap to a worse account.
rows = [row("a", 10, 88, "2026-10-17T00:00:00-04:00"), row("b", 0, 95, "2026-10-25T00:00:00-04:00")]
t, why = rot.decide("a", rows, old)
expect(t is None, f"no account with more headroom: hold, got {(t, why)}")

# Stranded workers: running, rate limited, not isolated.
names, iso = rot.stranded([
    {"name": "w1", "running": True, "status": "rate_limited"},
    {"name": "w2", "running": True, "status": "active"},
    {"name": "w3", "running": True, "status": "rate_limited", "isolated": True},
    {"name": "w4", "running": False, "status": "rate_limited"},
])
expect(names == ["w1"] and iso == ["w3"], f"stranded {names} isolated {iso}")
# Only Claude's own limit prompt gets a keypress.
expect(rot.resume_action("What do you want to do?\n 1. Stop and wait for limit to reset\n 2. Wait here") == "1",
       "the limit prompt is answered with 1")
expect(rot.resume_action("❯ fix the deploy gate") is None, "a composer gets no stray keypress")

print(f"test-claude-rotate: {PASS} passed, {FAIL} failed")
sys.exit(1 if FAIL else 0)
