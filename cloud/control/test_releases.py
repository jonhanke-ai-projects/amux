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


class WeeklyReport(unittest.TestCase):
    def test_quiet_week_and_blind_week_read_differently(self):
        import report
        store = Store(os.path.join(tempfile.mkdtemp(), "r.db"))
        lines, waiting = report.build(store, now=T0)
        self.assertEqual(lines["Synthetic journey"], "not measured: no synthetic runs recorded this week")
        self.assertEqual(lines["Owner time"], "not measured: no owner actions logged")
        self.assertEqual(waiting, [])
        store.run("INSERT INTO servers(id,org_id,slug,plan,status,fleet_secret,created_at,last_backup_at) VALUES('s','o','alice','solo','ready','x',?,?)", T0, T0 - 7200)
        store.alert("lifecycle_breaker", "4 lapsed at once")
        lines, waiting = report.build(store, now=T0 + 10)
        self.assertIn("alice", lines["Backups"])
        self.assertEqual(len(waiting), 1)
        text = report.render(lines, waiting)
        self.assertIn("\n\n- Backups:", text)          # blank line between blocks survives mail reflow


class RestoreDrill(unittest.TestCase):
    def setUp(self):
        import drill
        self.drill = drill
        self.store = Store(os.path.join(tempfile.mkdtemp(), "d.db"))
        self.infra = FakeInfra()
        self.store.run("INSERT INTO orgs(id,email,created_at) VALUES('o','o@example.com',?)", T0)
        self.store.run("INSERT INTO servers(id,org_id,slug,plan,status,fleet_secret,created_at,backup_bucket,restic_password,location) "
                       "VALUES('srv1','o','alice','solo','ready','x',?,'amux-alice-1','pw','fsn1')", T0)
        fleet.heartbeat(self.store, self.infra, self.store.one("SELECT * FROM servers"),
                        {"backup_fingerprint": {"tables": {"board_items": 12, "schedules": 3}, "files_sha256": "abc", "files": 40}}, now=T0)

    def run_drill(self, got, at):
        self.drill.sweep(self.store, self.infra, "golden", "https://cloud.example", now=at)
        d = self.store.one("SELECT * FROM drills WHERE finished_at IS NULL")
        ds = fleet.authenticate(self.store, d["id"], d["fleet_secret"])
        self.assertIsNotNone(ds)
        self.assertIsNone(fleet.authenticate(self.store, d["id"], "wrong"))
        first = fleet.heartbeat(self.store, self.infra, ds, {}, now=at + 60)
        self.assertEqual(first["restore_from"]["bucket"], "amux-alice-srv1")
        return fleet.heartbeat(self.store, self.infra, ds, {"drill_result": got}, now=at + 600)["result"]

    def test_T_OPS_3_pass_fail_cleanup_and_escalation(self):
        good = {"tables": {"board_items": 12, "schedules": 3}, "files_sha256": "abc", "files": 40}
        self.assertEqual(self.run_drill(good, T0 + 10), "pass")
        self.assertEqual([s for s in self.infra.servers.values() if s["labels"].get("purpose") == "restore-drill"], [],
                         "the drill server is deleted")
        self.assertEqual(self.drill.sweep(self.store, self.infra, "golden", "x", now=T0 + 3 * DAY), 0, "weekly, not daily")
        bad = {"tables": {"board_items": 7}, "files_sha256": "zzz", "files": 39}
        self.assertEqual(self.run_drill(bad, T0 + 8 * DAY), "fail")
        self.assertEqual(self.store.all("SELECT * FROM alerts"), [], "one failure is retried, not escalated")
        self.assertEqual(self.run_drill(bad, T0 + 16 * DAY), "fail")
        self.assertIn("restore_drill_failed", [a["kind"] for a in self.store.all("SELECT kind FROM alerts")])
        detail = self.store.one("SELECT detail FROM drills ORDER BY started_at DESC LIMIT 1")["detail"]
        self.assertIn("board_items: 7 rows restored, 12 expected", detail)
        self.assertIn("schedules missing", detail)

    def test_a_silent_drill_times_out_as_a_failure(self):
        self.drill.sweep(self.store, self.infra, "golden", "x", now=T0 + 10)
        self.drill.sweep(self.store, self.infra, "golden", "x", now=T0 + 10 + 46 * 60)
        d = self.store.one("SELECT * FROM drills")
        self.assertEqual(d["result"], "fail")
        self.assertIn("did not report", d["detail"])


class SyntheticServers(unittest.TestCase):
    """Found on the real control plane, 2026-10-15: nothing set servers.synthetic, so releases could
    never leave stage 0, and every journey server got a restore drill of its own."""

    def setUp(self):
        import provision
        self.provision = provision
        self.store = Store(os.path.join(tempfile.mkdtemp(), "s.db"))

    def org(self, oid, source):
        self.store.run("INSERT INTO orgs(id,email,source,created_at) VALUES(?,?,?,?)", oid, f"{oid}@example.com", source, T0)

    def test_synthetic_checkout_marks_the_server(self):
        self.org("syn", "synthetic")
        self.org("cust", "homepage")
        self.assertEqual(self.provision.start(self.store, "syn", "sub1", "solo", "syn@example.com", now=T0)["synthetic"], 1)
        self.assertEqual(self.provision.start(self.store, "cust", "sub2", "solo", "cust@example.com", now=T0)["synthetic"], 0)

    def test_drills_skip_synthetic_servers(self):
        import drill
        self.org("syn", "synthetic")
        srv = self.provision.start(self.store, "syn", "sub1", "solo", "syn@example.com", now=T0)
        self.store.run("UPDATE servers SET status='ready', backup_bucket='b', backup_fingerprint='{}' WHERE id=?", srv["id"])
        self.assertEqual(drill.due(self.store, T0 + 10), [])
        self.store.run("UPDATE servers SET synthetic=0 WHERE id=?", srv["id"])
        self.assertEqual(len(drill.due(self.store, T0 + 10)), 1)

    def test_failed_job_keeps_its_last_error(self):
        self.org("cust", "homepage")
        srv = self.provision.start(self.store, "cust", "sub1", "solo", "cust@example.com", now=T0)
        self.store.run("UPDATE provision_jobs SET error='403: resource_limit_exceeded' WHERE server_id=?", srv["id"])
        job = self.store.one("SELECT j.*, s.org_id FROM provision_jobs j JOIN servers s ON s.id=j.server_id WHERE j.server_id=?", srv["id"])
        self.provision.fail(self.store, FakeInfra(), job, "not ready within 15 minutes", T0 + 901)
        err = self.store.one("SELECT error FROM provision_jobs WHERE server_id=?", srv["id"])["error"]
        self.assertIn("not ready within 15 minutes", err)
        self.assertIn("resource_limit_exceeded", err)


class DrillCredentials(RestoreDrill):
    """2026-10-15: every drill heartbeat minted a backup token, and a drill whose source was deleted
    re-created the deleted customer's bucket each minute."""

    def start_drill(self, at):
        self.drill.sweep(self.store, self.infra, "golden", "https://cloud.example", now=at)
        d = self.store.one("SELECT * FROM drills WHERE finished_at IS NULL")
        return fleet.authenticate(self.store, d["id"], d["fleet_secret"])

    def test_one_token_per_drill_revoked_at_the_end(self):
        ds = self.start_drill(T0 + 10)
        for i in range(5):
            fleet.heartbeat(self.store, self.infra, fleet.authenticate(self.store, ds["id"], ds["fleet_secret"]), {}, now=T0 + 60 * (i + 1))
        drill_tokens = [t for t in self.infra.tokens if t.startswith("amux-drill-")]
        self.assertEqual(drill_tokens, [f"amux-drill-{ds['id']}"], "one token, however many heartbeats")
        good = {"tables": {"board_items": 12, "schedules": 3}, "files_sha256": "abc", "files": 40}
        fleet.heartbeat(self.store, self.infra, fleet.authenticate(self.store, ds["id"], ds["fleet_secret"]), {"drill_result": good}, now=T0 + 600)
        self.assertNotIn(f"amux-drill-{ds['id']}", self.infra.tokens)
        self.assertIsNone(self.store.one("SELECT creds FROM drills WHERE id=?", ds["id"])["creds"], "no secret kept after the drill")

    def test_deleted_source_cancels_the_drill_without_recreating_its_bucket(self):
        ds = self.start_drill(T0 + 10)
        self.store.run("UPDATE servers SET status='deleted' WHERE id='srv1'")
        self.infra.buckets = {}
        r = fleet.heartbeat(self.store, self.infra, ds, {}, now=T0 + 60)
        self.assertEqual(r, {"stop": True})
        self.assertEqual(self.infra.buckets, {}, "the deleted customer's bucket is not re-created")
        d = self.store.one("SELECT * FROM drills WHERE id=?", ds["id"])
        self.assertEqual(d["result"], "cancelled")
        self.assertNotIn(d["hetzner_id"], self.infra.servers)

    def test_sweep_every_few_minutes_still_starts_at_most_five_an_hour(self):
        for i in range(2, 9):
            self.store.run("INSERT INTO servers(id,org_id,slug,plan,status,fleet_secret,created_at,backup_bucket,backup_fingerprint) "
                           "VALUES(?,?,?,?,?,?,?,?,?)", f"srv{i}", "o", f"s{i}", "solo", "ready", "x", T0, f"b{i}", "{}")
        for k in range(12):                           # an hour of 5-minute sweeps
            self.drill.sweep(self.store, self.infra, "golden", "x", now=T0 + 10 + 300 * k)
        self.assertEqual(self.store.one("SELECT COUNT(*) n FROM drills")["n"], 5)


class SyntheticFailure(unittest.TestCase):
    def test_failed_synthetic_server_queues_no_refund_email_or_alert(self):
        import provision
        store = Store(os.path.join(tempfile.mkdtemp(), "f.db"))
        store.run("INSERT INTO orgs(id,email,source,created_at) VALUES('syn','s@amux.io','synthetic',?)", T0)
        srv = provision.start(store, "syn", "sub1", "solo", "s@amux.io", now=T0)
        job = store.one("SELECT j.*, s.org_id FROM provision_jobs j JOIN servers s ON s.id=j.server_id WHERE j.server_id=?", srv["id"])
        refunds = []
        provision.fail(store, FakeInfra(), job, "not ready within 15 minutes", T0 + 901, refund=lambda st, sv: refunds.append(sv) or True)
        self.assertEqual(refunds, [])
        self.assertEqual(store.one("SELECT COUNT(*) n FROM alerts")["n"], 0)
        self.assertEqual(store.one("SELECT COUNT(*) n FROM outbox")["n"], 0)
        self.assertEqual(store.one("SELECT status FROM servers WHERE id=?", srv["id"])["status"], "failed")
