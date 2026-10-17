"""Checkout and the synthetic customer's live subscription (2026-10-17)."""
import os, tempfile, unittest

from billing import StripeBilling, FIRST_MONTH_COUPON

PLANS = {"plans": [{"id": "solo", "stripe_lookup_key": "amux_cloud_solo_monthly"}]}


def billing_recording():
    b = StripeBilling("sk_test_x", PLANS)
    b._price_ids["solo"] = "price_1"
    calls = []
    def req(method, path, params=None, idempotency_key=None):
        calls.append((method, path, params or {}))
        return {"url": "u", "id": {"/customers": "cus_1", "/subscriptions": "sub_1"}.get(path, "cs_1")}
    b._req = req
    return b, calls


class Checkout(unittest.TestCase):
    def test_customers_can_enter_a_promotion_code_and_give_a_card(self):
        b, calls = billing_recording()
        b.create_checkout("solo", "su_1", "direct", "https://s", "https://c", customer_email="a@b.co")
        p = calls[0][2]
        self.assertTrue(p["allow_promotion_codes"])               # FIRSTMONTH is entered here
        self.assertNotIn("payment_method_collection", p)          # Stripe's default: always take a card
        self.assertEqual(p["branding_settings"]["display_name"], "amux Cloud")   # never Mixpeek's branding


class SyntheticSubscription(unittest.TestCase):
    def test_free_first_month_subscription_completes_like_a_checkout(self):
        b, calls = billing_recording()
        sess = b.subscribe_free_first_month("synthetic+1@amux.io", "solo",
                                            {"signup_id": "su_9", "plan": "solo", "source": "synthetic"})
        sub = next(c for c in calls if c[1] == "/subscriptions")[2]
        self.assertEqual(sub["discounts"], [{"coupon": FIRST_MONTH_COUPON}])
        self.assertEqual(sub["items"], [{"price": "price_1"}])
        self.assertEqual((sess["status"], sess["payment_status"], sess["amount_total"]), ("complete", "no_payment_required", 0))


class SyntheticRoute(unittest.TestCase):
    """/synthetic/subscribe is admin-only and synthetic+ addresses only."""

    def setUp(self):
        import app
        from fakes import FakeBilling, FakeInfra
        from store import Store
        app.STORE = Store(os.path.join(tempfile.mkdtemp(), "c.db"))
        app.BILLING, app.INFRA = FakeBilling(), FakeInfra()
        os.environ["CONTROL_ADMIN_TOKEN"] = "adm"
        self.app = app

    def call(self, query, admin=True):
        app = self.app
        class H:
            headers = {"Authorization": "Bearer adm"} if admin else {}
            command = "GET"
            out = {}
            def query(self): return query
            def send(self, code, body, *a, **k): H.out["code"] = code
            def redirect(self, to, *a): H.out["code"] = 302; H.out["to"] = to
            def session_cookie(self, uid): return "c"
        h = H()
        app.r_synthetic_subscribe(h, None)
        return H.out

    def test_refuses_without_admin_and_for_real_addresses(self):
        self.assertEqual(self.call({"email": "synthetic+1@amux.io"}, admin=False)["code"], 403)
        self.assertEqual(self.call({"email": "someone@customer.com"})["code"], 403)

    def test_admin_synthetic_gets_a_server(self):
        out = self.call({"email": "synthetic+2@amux.io", "plan": "solo"})
        self.assertEqual((out["code"], out.get("to")), (302, "/progress"))
        srv = self.app.STORE.one("SELECT s.* FROM servers s JOIN orgs o ON o.id=s.org_id WHERE o.email='synthetic+2@amux.io'")
        self.assertEqual((srv["status"], srv["synthetic"]), ("provisioning", 1))


if __name__ == "__main__":
    unittest.main()
