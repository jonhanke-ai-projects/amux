#!/usr/bin/env python3
"""Make a Stripe TEST-mode account ready for the control plane (docs/design/cloud-self-service.md, T-PAY-1).

Idempotent. For each plan in plans.json: a product and a monthly price carrying the plan's
stripe_lookup_key (the control plane finds prices by lookup key). Then one webhook endpoint at
<control>/webhooks/stripe for the events accounts.py handles. Prints the webhook signing secret
the first time it creates the endpoint, which Stripe shows only then.

  stripe_setup.py --control https://cp-test.amux.io     (key from STRIPE_TEST_SECRET_KEY)

Refuses a live key: this script creates objects, and live objects are the owner's to create.
"""
import argparse, json, os, sys, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
EVENTS = ["checkout.session.completed", "customer.subscription.created", "customer.subscription.updated",
          "customer.subscription.deleted", "invoice.paid", "invoice.payment_failed", "charge.dispute.created"]


def form(d, prefix=""):
    out = []
    for k, v in d.items():
        key = f"{prefix}[{k}]" if prefix else k
        if isinstance(v, dict):
            out += form(v, key)
        elif isinstance(v, list):
            out += [(f"{key}[]", str(x)) for x in v]
        else:
            out.append((key, str(v)))
    return out


def call(key, method, path, params=None):
    data = urllib.parse.urlencode(form(params or {})).encode() if method == "POST" else None
    url = "https://api.stripe.com/v1" + path + ("?" + urllib.parse.urlencode(form(params or {})) if method == "GET" and params else "")
    req = urllib.request.Request(url, data=data, method=method, headers={"Authorization": f"Bearer {key}"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--control", required=True, help="control plane URL, e.g. https://cp-test.amux.io")
    a = ap.parse_args()
    key = os.environ.get("STRIPE_TEST_SECRET_KEY", "")
    if not key.startswith(("sk_test_", "rk_test_")):
        sys.exit("STRIPE_TEST_SECRET_KEY must be a test-mode key (sk_test_...); refusing to create live objects")
    plans = json.load(open(os.path.join(HERE, "plans.json")))
    for p in plans["plans"]:
        lk = p["stripe_lookup_key"]
        if call(key, "GET", "/prices", {"lookup_keys": [lk], "active": "true"})["data"]:
            print(f"price {lk}: present", file=sys.stderr)
            continue
        prod = call(key, "POST", "/products", {"name": f"amux Cloud {p['name']}", "description": p.get("specs", ""),
                                                "metadata": {"plan": p["id"]}})
        call(key, "POST", "/prices", {"product": prod["id"], "currency": plans.get("currency", "usd"),
                                      "unit_amount": int(round(p["price_usd"] * 100)), "recurring": {"interval": "month"},
                                      "lookup_key": lk, "tax_behavior": "exclusive"})
        print(f"price {lk}: created at ${p['price_usd']}/month", file=sys.stderr)
    url = a.control.rstrip("/") + "/webhooks/stripe"
    hooks = call(key, "GET", "/webhook_endpoints", {"limit": 100})["data"]
    if any(h["url"] == url for h in hooks):
        print(f"webhook {url}: present (its secret is only shown at creation)", file=sys.stderr)
        return 0
    h = call(key, "POST", "/webhook_endpoints", {"url": url, "enabled_events": EVENTS,
                                                  "description": "amux Cloud control plane"})
    print(f"webhook {url}: created", file=sys.stderr)
    print(h["secret"])          # stdout: the signing secret, for the caller to store
    return 0


if __name__ == "__main__":
    sys.exit(main())
