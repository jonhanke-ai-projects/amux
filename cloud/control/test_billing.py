"""Checkout parameters (2026-10-17): customers can enter FIRSTMONTH and always give a card; only the
authenticated synthetic customer in live mode gets the free first invoice without one."""
import unittest

from billing import StripeBilling, FIRST_MONTH_COUPON

PLANS = {"plans": [{"id": "solo", "stripe_lookup_key": "amux_cloud_solo_monthly"}]}


class Checkout(unittest.TestCase):
    def params(self, **kw):
        b = StripeBilling("sk_test_x", PLANS)
        b._price_ids["solo"] = "price_1"
        seen = {}
        b._req = lambda method, path, params=None, idempotency_key=None: seen.update(params) or {"url": "u", "id": "cs"}
        b.create_checkout("solo", "su_1", "direct", "https://s", "https://c", customer_email="a@b.co", **kw)
        return seen

    def test_customers_can_enter_a_promotion_code_and_give_a_card(self):
        p = self.params()
        self.assertTrue(p["allow_promotion_codes"])
        self.assertNotIn("discounts", p)
        self.assertNotIn("payment_method_collection", p)        # Stripe's default: always collect

    def test_synthetic_gets_the_free_first_invoice_without_a_card(self):
        p = self.params(synthetic_free=True)
        self.assertEqual(p["discounts"], [{"coupon": FIRST_MONTH_COUPON}])
        self.assertEqual(p["payment_method_collection"], "if_required")
        self.assertNotIn("allow_promotion_codes", p)            # Stripe refuses both together


if __name__ == "__main__":
    unittest.main()
