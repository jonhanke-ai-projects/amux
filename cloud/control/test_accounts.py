"""Checkout, webhooks, provisioning, tokens, refunds, disputes and the fleet API
(T-PAY-1, T-ACC-1/2/4, T-PROV-1/3/4, T-SSO-1, T-DIS-1, refund cap)."""
import os, sys, tempfile, threading, time, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import accounts, fleet, provision, tokens  # noqa: E402
from fakes import FakeBilling, FakeInfra  # noqa: E402
from price_check import load_plans  # noqa: E402
from store import Store  # noqa: E402

PLANS = load_plans()
T0 = 1_800_000_000.0


def session(sid, email, sub, customer="cus_1", amount=3900, plan="solo"):
    return {"id": sid, "status": "complete", "payment_status": "paid", "mode": "subscription",
            "customer_details": {"email": email}, "customer": customer, "subscription": sub,
            "amount_total": amount, "payment_intent": f"pi_{sid}", "metadata": {"plan": plan, "source": "test"}}


class Base(unittest.TestCase):
    def setUp(self):
        self.store = Store(os.path.join(tempfile.mkdtemp(), "c.db"))
        self.billing, self.infra = FakeBilling(), FakeInfra()
        self.billing.sessions = {}

    def pay(self, sid, email, sub, **kw):
        self.billing.sessions[sid] = session(sid, email, sub, **kw)
        self.billing.subs[sub] = {"id": sub, "status": "active", "current_period_end": T0 + 30 * 86400}
        return accounts.complete_checkout(self.store, self.billing, sid, now=T0)


class Checkout(Base):
    def test_T_ACC_1_new_email_gets_account_org_and_one_job(self):
        r = self.pay("cs_1", "New@Example.com", "sub_1")
        self.assertEqual(r["user"]["email"], "new@example.com")
        self.assertEqual(r["user"]["email_verified"], 0)
        self.assertEqual(r["server"]["status"], "provisioning")
        self.assertEqual(len(self.store.all("SELECT * FROM provision_jobs")), 1)
        self.assertEqual([e["template"] for e in self.store.all("SELECT template FROM outbox")], ["being_built"])

    def test_T_PAY_1_replay_and_race_make_one_job(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        accounts.complete_checkout(self.store, self.billing, "cs_1", now=T0 + 1)      # success URL again
        ev = {"id": "evt_1", "type": "checkout.session.completed", "data": {"object": {"id": "cs_1", "mode": "subscription"}}}
        accounts.handle_event(self.store, self.billing, ev, now=T0 + 2)
        self.assertEqual(accounts.handle_event(self.store, self.billing, ev, now=T0 + 3), "duplicate")
        self.assertEqual(len(self.store.all("SELECT * FROM provision_jobs")), 1)
        self.assertEqual(len(self.store.all("SELECT * FROM servers")), 1)

    def test_T_ACC_4_concurrent_success_url_and_webhook(self):
        self.billing.sessions["cs_9"] = session("cs_9", "race@example.com", "sub_9")
        self.billing.subs["sub_9"] = {"id": "sub_9", "status": "active", "current_period_end": T0 + 9e5}
        out, errs = [], []

        def go():
            try:
                out.append(accounts.complete_checkout(self.store, self.billing, "cs_9", now=T0)["server"]["id"])
            except Exception as e:  # pragma: no cover - surfaced below
                errs.append(e)
        ts = [threading.Thread(target=go) for _ in range(4)]
        [t.start() for t in ts]
        [t.join() for t in ts]
        self.assertEqual(errs, [])
        self.assertEqual(len(set(out)), 1)
        self.assertEqual(len(self.store.all("SELECT * FROM servers")), 1)

    def test_T_ACC_2_existing_account_payment_attaches(self):
        a = self.pay("cs_1", "same@example.com", "sub_1")
        b = self.pay("cs_2", "same@example.com", "sub_2")
        self.assertEqual(a["org"]["id"], b["org"]["id"])
        self.assertEqual(len(self.store.all("SELECT * FROM users")), 1)
        self.assertEqual(len(self.store.all("SELECT * FROM servers")), 2)        # a second server (J21 later)

    def test_unpaid_session_is_refused(self):
        self.billing.sessions["cs_x"] = dict(session("cs_x", "x@example.com", "sub_x"), payment_status="unpaid")
        with self.assertRaises(accounts.NotPaid):
            accounts.complete_checkout(self.store, self.billing, "cs_x", now=T0)
        self.assertEqual(self.store.all("SELECT * FROM servers"), [])

    def test_payment_failed_then_paid_webhooks(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        accounts.handle_event(self.store, self.billing, {"id": "e1", "type": "invoice.payment_failed",
                              "data": {"object": {"id": "in_2", "subscription": "sub_1"}}}, now=T0 + 10)
        sub = self.store.one("SELECT * FROM subscriptions WHERE id='sub_1'")
        self.assertEqual((sub["status"], sub["failure_started_at"]), ("past_due", T0 + 10))
        accounts.handle_event(self.store, self.billing, {"id": "e2", "type": "invoice.payment_failed",
                              "data": {"object": {"id": "in_2", "subscription": "sub_1"}}}, now=T0 + 86400 * 3)
        self.assertEqual(self.store.one("SELECT failure_started_at f FROM subscriptions")["f"], T0 + 10)   # first failure kept
        accounts.handle_event(self.store, self.billing, {"id": "e3", "type": "invoice.paid",
                              "data": {"object": {"id": "in_2", "subscription": "sub_1", "amount_paid": 3900}}}, now=T0 + 86400 * 4)
        self.assertIsNone(self.store.one("SELECT failure_started_at f FROM subscriptions")["f"])
        self.assertEqual(self.store.one("SELECT COUNT(*) n FROM ledger")["n"], 2)


class Provisioning(Base):
    def job(self):
        return self.store.one("SELECT * FROM provision_jobs")

    def tick(self, t):
        return provision.tick(self.store, self.infra, PLANS, "golden-1", "https://cloud.amux.io", now=t,
                              refund=accounts.provisioning_refund(self.billing))

    def test_T_PROV_1_create_dns_then_ready_on_heartbeat(self):
        r = self.pay("cs_1", "a@example.com", "sub_1")
        self.tick(T0 + 1)
        srv = self.store.one("SELECT * FROM servers")
        h = self.infra.servers[srv["hetzner_id"]]
        self.assertEqual(h["labels"]["amux-cloud"], "1")
        self.assertIn(srv["fleet_secret"], h["user_data"])
        self.tick(T0 + 2)
        self.assertEqual(self.job()["step"], "boot")
        self.assertEqual(len(self.infra.dns), 1)
        reply = fleet.heartbeat(self.store, self.infra, self.store.one("SELECT * FROM servers"),
                                {"version": "1.0", "csr": "CSR", "serving": True, "last_backup_at": T0 + 100}, now=T0 + 120)
        self.assertIn("BEGIN CERTIFICATE", reply["origin_certificate"])
        self.assertTrue(reply["ready"])
        self.assertEqual(self.store.one("SELECT status FROM servers")["status"], "ready")
        self.assertEqual(r["server"]["slug"], "a")

    def test_ready_needs_a_first_backup(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        self.tick(T0 + 1)
        self.tick(T0 + 2)
        reply = fleet.heartbeat(self.store, self.infra, self.store.one("SELECT * FROM servers"),
                                {"serving": True}, now=T0 + 60)
        self.assertNotIn("ready", reply)
        self.assertEqual(self.store.one("SELECT status FROM servers")["status"], "provisioning")

    def test_T_PROV_3_capacity_error_moves_to_next_location(self):
        self.infra.capacity_errors = {"fsn1"}
        self.pay("cs_1", "a@example.com", "sub_1")
        self.tick(T0 + 1)
        self.tick(T0 + 2)
        self.assertEqual(self.store.one("SELECT location FROM servers")["location"], "nbg1")
        self.assertEqual([e["props"] for e in self.store.all("SELECT props FROM events WHERE name='cloud.provision_capacity'")],
                         ['{"location": "fsn1"}'])

    def test_T_PROV_4_timeout_refunds_emails_and_cleans_up(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        self.store.run("INSERT INTO ledger(invoice_id,org_id,subscription_id,amount_cents,paid_at) "
                       "SELECT 'big','x','y',500000,?", T0)      # enough revenue that a $39 refund fits the 2% cap
        self.tick(T0 + 1)
        self.tick(T0 + 2)
        self.tick(T0 + 16 * 60)
        self.assertEqual(self.store.one("SELECT status FROM servers")["status"], "failed")
        self.assertEqual(self.infra.servers, {})
        self.assertEqual(self.infra.dns, {})
        self.assertEqual(len(self.billing.refunds), 1)
        self.assertIn("provision_failed", [r["template"] for r in self.store.all("SELECT template FROM outbox")])
        self.assertIn("provision_failed", [a["kind"] for a in self.store.all("SELECT kind FROM alerts")])

    def test_freeze_stops_provisioning(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        self.store.set_flag("provisioning_freeze", "1")
        self.assertEqual(self.tick(T0 + 1), 0)
        self.assertEqual(self.infra.servers, {})


class Refunds(Base):
    def test_over_cap_is_queued_then_paid_by_default(self):
        self.pay("cs_1", "a@example.com", "sub_1")                # $39 revenue -> cap is 78 cents
        self.assertFalse(accounts.refund_latest(self.store, self.billing, self.store.one("SELECT id FROM orgs")["id"],
                                                "test", now=T0 + 1))
        self.assertEqual(self.billing.refunds, [])
        accounts.settle_queued_refunds(self.store, self.billing, now=T0 + 6 * 86400)
        self.assertEqual(self.billing.refunds, [])
        accounts.settle_queued_refunds(self.store, self.billing, now=T0 + 8 * 86400)
        self.assertEqual(len(self.billing.refunds), 1)

    def test_room_is_two_percent_capped_at_500(self):
        self.store.run("INSERT INTO ledger(invoice_id,org_id,amount_cents,paid_at) VALUES('a','o',10000000,?)", T0)
        self.assertEqual(accounts.refund_room_cents(self.store, now=T0 + 1), 50000)


class Disputes(Base):
    def test_T_DIS_1_contest_with_evidence_or_accept_small(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        r = accounts.handle_event(self.store, self.billing, {"id": "d1", "type": "charge.dispute.created",
                                  "data": {"object": {"id": "dp_1", "amount": 3900, "payment_intent": "pi_cs_1"}}}, now=T0 + 9)
        self.assertEqual(r, "dispute_contested")
        self.assertIn("a@example.com", self.billing.disputes[0][1]["customer_email_address"])
        r = accounts.handle_event(self.store, self.billing, {"id": "d2", "type": "charge.dispute.created",
                                  "data": {"object": {"id": "dp_2", "amount": 500, "payment_intent": "pi_cs_1"}}}, now=T0 + 9)
        self.assertEqual(r, "dispute_accepted")


class Tokens(unittest.TestCase):
    def test_T_SSO_1_token_is_bound_to_its_server(self):
        a = {"slug": "alice", "fleet_secret": tokens.new_secret()}
        b = {"slug": "bob", "fleet_secret": tokens.new_secret()}
        t = tokens.door_token(a, "alice@example.com")
        self.assertEqual(tokens.verify(t, a["fleet_secret"], audience="alice")["sub"], "alice@example.com")
        with self.assertRaisesRegex(ValueError, "bad signature"):
            tokens.verify(t, b["fleet_secret"], audience="bob")
        forged = tokens.sign({"aud": "bob", "sub": "x"}, a["fleet_secret"], 60)   # right shape, wrong key
        with self.assertRaises(ValueError):
            tokens.verify(forged, b["fleet_secret"], audience="bob")
        with self.assertRaisesRegex(ValueError, "expired"):
            tokens.verify(tokens.door_token(a, "x", ttl=-1), a["fleet_secret"])
        with self.assertRaisesRegex(ValueError, "wrong audience"):
            tokens.verify(t, a["fleet_secret"], audience="bob")


class Fleet(Base):
    def test_auth_and_invite_email_limits(self):
        self.pay("cs_1", "a@example.com", "sub_1")
        srv = self.store.one("SELECT * FROM servers")
        self.assertIsNone(fleet.authenticate(self.store, srv["id"], "wrong"))
        self.assertIsNotNone(fleet.authenticate(self.store, srv["id"], srv["fleet_secret"]))
        code, _ = fleet.invite_email(self.store, srv, {"email": "x@example.com", "invite_url": "https://evil.example/x"}, now=T0)
        self.assertEqual(code, 400)
        for i in range(fleet.INVITE_EMAILS_PER_DAY):
            code, _ = fleet.invite_email(self.store, srv, {"email": f"m{i}@example.com",
                                         "invite_url": f"https://cloud.amux.io/invite/a/t{i}"}, now=T0)
            self.assertEqual(code, 200)
        code, _ = fleet.invite_email(self.store, srv, {"email": "late@example.com",
                                     "invite_url": "https://cloud.amux.io/invite/a/late"}, now=T0)
        self.assertEqual(code, 429)


if __name__ == "__main__":
    unittest.main()
