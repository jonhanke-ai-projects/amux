"""The HTTP layer must at least import and route: a syntax error in an f-string page once
passed every other unit test because nothing imported app.py."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


class Imports(unittest.TestCase):
    def test_app_imports_and_every_route_has_a_handler(self):
        import app
        paths = {(m, p) for m, p, _ in app.ROUTES}
        for need in [("GET", "/"), ("GET", "/start"), ("GET", "/welcome"), ("POST", "/webhooks/stripe"),
                     ("POST", "/api/fleet/"), ("GET", "/invite/"), ("POST", "/api/session"), ("POST", "/api/notify")]:
            self.assertIn(need, paths)
        self.assertTrue(all(callable(fn) for _, _, fn in app.ROUTES))

    def test_clerk_module_imports(self):
        import clerk  # noqa: F401


if __name__ == "__main__":
    unittest.main()


class PaymentsState(unittest.TestCase):
    """A Stripe test key accepts card 4242; if it opened /start to the public, anyone could
    provision real servers at our cost. Only a live key may report payments as live."""

    def test_test_key_is_not_live(self):
        from unittest import mock
        import app
        with mock.patch.object(app, "FAKE_BILLING", False), mock.patch.dict(app.PLANS, {"status": "approved"}):
            with mock.patch.dict(os.environ, {"STRIPE_SECRET_KEY": "sk_test_x"}):
                self.assertEqual(app.payments_state(), "test")
            with mock.patch.dict(os.environ, {"STRIPE_SECRET_KEY": "sk_live_x"}):
                self.assertEqual(app.payments_state(), "live")
        with mock.patch.object(app, "FAKE_BILLING", True):
            self.assertEqual(app.payments_state(), "not live")
