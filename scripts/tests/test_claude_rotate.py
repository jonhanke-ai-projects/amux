"""Policy tests for scripts/claude-rotate.py. Run: python3 scripts/tests/test_claude_rotate.py"""
import datetime as dt, importlib.util, io, json, os, sys, tempfile, time, unittest
from contextlib import redirect_stdout

TMP = tempfile.mkdtemp()
os.environ.update(CLAUDE_ROTATE_DIR=TMP, CLAUDE_ROTATE_LOG=os.path.join(TMP, "log"),
                  CLAUDE_ROTATE_CLAUDE_JSON=os.path.join(TMP, "claude.json"))
spec = importlib.util.spec_from_file_location("cr", os.path.join(os.path.dirname(__file__), "..", "claude-rotate.py"))
cr = importlib.util.module_from_spec(spec); spec.loader.exec_module(cr)
NOW = dt.datetime(2026, 10, 15, 11, 30).astimezone()
h = lambda hours: (NOW + dt.timedelta(hours=hours)).isoformat()

def acct(five, five_r, week, week_r):
    return {"five_hour": {"used_percentage": five, "resets_at": five_r}, "seven_day": {"used_percentage": week, "resets_at": week_r}}

def setup(accounts, current=None):
    cr.save(cr.ACCOUNTS, {"accounts": accounts})
    json.dump({"oauthAccount": {"emailAddress": current}}, open(cr.CLAUDE_JSON, "w"))

class Policy(unittest.TestCase):
    def test_expiring_capacity_first(self):
        setup({"run": acct(10, h(4), 29, h(57)), "join": acct(0, None, 89, h(15)), "use": acct(0, None, 100, h(104))})
        rows = cr.rank(NOW)
        self.assertEqual([r["email"] for r in rows], ["join", "run", "use"])
        self.assertEqual(cr.decide("run", rows, {}, NOW), ("join", "expiring_capacity_first"))

    def test_exhausted_switches_even_right_after_a_switch(self):
        setup({"join": acct(99.5, h(2), 90, h(15)), "run": acct(10, h(4), 29, h(57))})
        self.assertEqual(cr.decide("join", cr.rank(NOW), {"switched_at": time.time()}, NOW), ("run", "current_exhausted"))

    def test_no_flap_inside_gap(self):
        setup({"run": acct(10, h(4), 29, h(57)), "join": acct(0, None, 89, h(15))})
        self.assertEqual(cr.decide("run", cr.rank(NOW), {"switched_at": time.time()}, NOW), (None, "recent_switch"))

    def test_reset_passed_counts_as_zero(self):
        setup({"amux": acct(0, None, 100, h(-1))})
        r = cr.rank(NOW)[0]
        self.assertTrue(r["usable"]); self.assertEqual(r["weekly"], 0.0)

    def test_all_exhausted_holds_and_names_earliest(self):
        setup({"a": acct(0, None, 100, h(20)), "b": acct(100, h(1), 50, h(90))})
        rows = cr.rank(NOW)
        self.assertEqual(cr.decide("a", rows, {}, NOW), (None, "no_usable_account"))
        self.assertEqual(rows[0]["email"], "b")  # back first, in one hour

    def test_unknown_after_known(self):
        setup({"new": {}, "run": acct(10, h(4), 29, h(57))})
        self.assertEqual([r["email"] for r in cr.rank(NOW)], ["run", "new"])

    def test_five_hour_block_is_temporary(self):
        setup({"vec": acct(99, h(1), 93, h(140))})
        r = cr.rank(NOW)[0]
        self.assertFalse(r["usable"]); self.assertEqual(r["available_at"], h(1))

    def test_statusline_records_current_account_and_prints(self):
        setup({}, current="ethan@joinmixpeek.com")
        cr.save(cr.STATE, {"switched_at": 0})
        payload = {"rate_limits": {"five_hour": {"used_percentage": 12.4, "resets_at": 1792090000},
                                   "seven_day": {"used_percentage": 90, "resets_at": 1792130400}}}
        sys.stdin = io.StringIO(json.dumps(payload)); out = io.StringIO()
        with redirect_stdout(out): cr.cmd_statusline()
        self.assertEqual(out.getvalue().strip(), "joinmixpeek · 5h 12% · wk 90%")
        saved = cr.load(cr.ACCOUNTS, {})["accounts"]["ethan@joinmixpeek.com"]
        self.assertEqual(saved["seven_day"]["used_percentage"], 90)

    def test_statusline_ignores_readings_right_after_switch(self):
        setup({}, current="ethan@x.com")
        cr.save(cr.STATE, {"switched_at": time.time()})
        try: os.remove(os.path.join(cr.DIR, "last-record"))
        except OSError: pass
        sys.stdin = io.StringIO(json.dumps({"rate_limits": {"five_hour": {"used_percentage": 50, "resets_at": 1}}}))
        with redirect_stdout(io.StringIO()): cr.cmd_statusline()
        self.assertNotIn("ethan@x.com", cr.load(cr.ACCOUNTS, {"accounts": {}})["accounts"])

if __name__ == "__main__":
    unittest.main(verbosity=1)
