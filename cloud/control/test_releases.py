"""Staged rollout (M12, T-OPS-1/2) and per-server backup credentials in the heartbeat."""
import os, sys, tempfile, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fleet, releases  # noqa: E402
from fakes import FakeInfra  # noqa: E402
from store import Store  # noqa: E402

T0 = 1_800_000_000.0
DAY = 86400


class Rollout(unittest.TestCase):
    def setUp(self):
        self.store = Store(os.path.join(tempfile.mkdtemp(), "c.db"))
        self.infra = FakeInfra()
        for i in range(200):
            self.store.run("INSERT INTO servers(id,org_id,slug,plan,status,fleet_secret,created_at,synthetic,version) "
                           "VALUES(?,?,?,?,?,?,?,?,?)", f"srv{i}", f"o{i}", f"s{i}", "solo", "ready", "x", T0, int(i == 0), "v1")
        releases.publish(self.store, "v2", "https://r/v2", "abc", "https://r/v2.sig", now=T0)

    def srv(self, i):
        return self.store.one("SELECT * FROM servers WHERE id=?", f"srv{i}")

    def offered(self, now):
        return [i for i in range(200) if releases.offer_for(self.store, self.srv(i), now)]

    def test_T_OPS_1_synthetic_then_canary_then_all(self):
        self.assertEqual(self.offered(T0), [0])
        self.assertIsNone(releases.advance(self.store, now=T0 + DAY + 1))      # synthetic has not installed it
        releases.report(self.store, self.srv(0), {"version": "v2", "outcome": "installed"}, now=T0 + 60)
        self.assertEqual(releases.advance(self.store, now=T0 + DAY + 1), 1)
        canary = self.offered(T0 + DAY + 2)
        self.assertTrue(5 <= len(canary) - 1 <= 40, len(canary))               # about 10% plus the synthetic one
        self.assertIsNone(releases.advance(self.store, now=T0 + DAY + 3))      # stage 1 must soak 24 hours
        self.assertEqual(releases.advance(self.store, now=T0 + 2 * DAY + 2), 2)
        self.assertEqual(len(self.offered(T0 + 2 * DAY + 3)), 200)

    def test_T_OPS_2_two_rollbacks_halt_and_alert(self):
        releases.report(self.store, self.srv(0), {"version": "v2", "outcome": "rolled_back"}, now=T0 + 60)
        self.assertEqual(self.offered(T0 + 61), [0])                            # one rollback: still offered
        self.assertIsNone(releases.advance(self.store, now=T0 + 2 * DAY))     # a rollback blocks advancing
        releases.report(self.store, self.srv(0), {"version": "v2", "outcome": "rolled_back"}, now=T0 + 120)
        self.assertEqual(self.store.one("SELECT rollbacks FROM releases")["rollbacks"], 1)   # same server twice counts once
        releases.report(self.store, self.srv(1), {"version": "v2", "outcome": "refused"}, now=T0 + 180)
        self.assertEqual(self.offered(T0 + 181), [])
        self.assertIn("release_halted", [a["kind"] for a in self.store.all("SELECT kind FROM alerts")])

    def test_deploy_freeze_offers_nothing(self):
        self.store.set_flag("deploy_freeze", "1")
        self.assertEqual(self.offered(T0), [])

    def test_heartbeat_carries_offer_and_backup_creds_once(self):
        srv = self.srv(0)
        reply = fleet.heartbeat(self.store, self.infra, srv, {"version": "v1", "need_backup_creds": True}, now=T0)
        self.assertEqual(reply["release"]["version"], "v2")
        creds = reply["backup_credentials"]
        self.assertEqual(creds["bucket"], "amux-s0-srv0")
        self.assertTrue(creds["restic_password"])
        again = fleet.heartbeat(self.store, self.infra, self.srv(0), {"version": "v2"}, now=T0 + 60)
        self.assertNotIn("backup_credentials", again)
        self.assertNotIn("release", again)
        other = fleet.heartbeat(self.store, self.infra, self.srv(5), {"need_backup_creds": True}, now=T0)
        self.assertNotEqual(other["backup_credentials"]["bucket"], creds["bucket"])


if __name__ == "__main__":
    unittest.main()
