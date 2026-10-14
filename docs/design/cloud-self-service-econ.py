"""Unit economics for amux Cloud (docs/design/cloud-self-service.md, section 4).
Inputs are either measured (Hetzner pricing API 2026-10-14; Stripe/Paddle list
prices; PostHog and GitHub traffic 2026-10-14) or marked H (hypothesis).
Run: python3 docs/design/cloud-self-service-econ.py"""
import math
COST = {'solo': 9.99 + 0.60 + 2.00, 'team': 18.49 + 0.60 + 3.70, 'max': 34.99 + 0.60 + 7.00}   # server + IPv4 + backups
MIX = {'solo': 0.80, 'team': 0.16, 'max': 0.04}            # H; EU only at launch (US deferred)
LADDERS = {'$26 (2x cost)': {'solo': 26, 'team': 46, 'max': 86},
           '$39 entry':     {'solo': 39, 'team': 69, 'max': 129},
           '$49 entry':     {'solo': 49, 'team': 89, 'max': 159}}
EU_CONSUMER = 0.35     # H: share of customers who are EU consumers (pay VAT; prices shown VAT-inclusive)
VAT = 0.21             # H: average EU VAT rate
INTL = 0.75            # H: share of cards issued outside the US (EU default product)
REFUND, ABUSE, DISPUTE_RATE, DISPUTE_FEE = 0.01, 0.005, 0.003, 15.0   # H, H, H, Stripe list fee per dispute
SUPPORT_TOKENS = 0.40  # H: support-agent inference per customer per month
LAPSED_SNAPSHOT = 0.02 * 1.0   # H: 2% of customers lapsed at any time x ~$1 snapshot each (servers deleted, not stopped)
PAYMENTS = {
  'Stripe': dict(pct=0.029 + 0.007 + 0.005 + INTL * (0.015 + 0.01), fix=0.30, disputes=True),   # card, Billing, Tax, intl + FX
  'Merchant of record (Paddle)': dict(pct=0.05, fix=0.50, disputes=False),                      # handles VAT filing and disputes
}
def fixed(n):
    control = 7.09 * max(1, math.ceil(n / 1500))      # H: one cx23 control-plane node per 1500 servers
    operator_server = 12.59                            # the Operator's own amux server (cx33)
    synthetic = 0.0137 * 24 * 30 + 0.05 * 24 * 30     # one started cx33 hour + ~$0.05 tokens per hourly run (H)
    operator_tokens = 250                              # cap at scale; the plan scales it as $50 + 5% of contribution, max $250 (section 4.7)
    clerk = 0 if n <= 10000 else 0.02 * (n - 10000)    # H: check the current Clerk tier
    return control + operator_server + synthetic + operator_tokens + clerk
OWNER_RATE = 150                                       # H: owner time at $/hour
def owner_hours(n): return n / 1000                    # target: no routine work; ~1 h of exceptions per 1000 customers (H)
def month(n, ladder, pay):
    p = PAYMENTS[pay]; rev = net = var = 0.0
    for plan, share in MIX.items():
        c = n * share; price = LADDERS[ladder][plan]
        gross = c * price
        net_price = price * (1 - EU_CONSUMER) + price / (1 + VAT) * EU_CONSUMER   # VAT leaves with the tax authority
        rev += gross; net += c * net_price
        var += c * (COST[plan] + price * (p['pct'] + REFUND + ABUSE) + p['fix'] + SUPPORT_TOKENS + LAPSED_SNAPSHOT
                    + (DISPUTE_RATE * DISPUTE_FEE if p['disputes'] else 0))
    f = fixed(n); contrib = net - var - f
    return rev, net, var, f, contrib, contrib - owner_hours(n) * OWNER_RATE
if __name__ == '__main__':
    print('Monthly at steady state, USD. "Net" is after VAT.')
    print(f"{'payments':28} {'ladder':14} {'N':>5} {'gross':>9} {'net':>9} {'variable':>9} {'fixed':>6} {'contrib':>9} {'margin':>7} {'after owner':>11}")
    for pay in PAYMENTS:
        for L in LADDERS:
            for n in (100, 500, 1000, 5000):
                g, nt, v, f, c, co = month(n, L, pay)
                print(f"{pay:28} {L:14} {n:5d} {g:9,.0f} {nt:9,.0f} {v:9,.0f} {f:6,.0f} {c:9,.0f} {c/nt:7.0%} {co:11,.0f}")
    print('\nPrice and churn: per 1,000 buyers at the $26 conversion rate (H: $39 converts 0.80x, $49 0.65x), Stripe path')
    CONV = {'$26 (2x cost)': 1.0, '$39 entry': 0.80, '$49 entry': 0.65}
    for L in LADDERS:
        unit = (month(1000, L, 'Stripe')[4] + fixed(1000)) / 1000
        for churn in (0.05, 0.12):
            n0 = 1000 * CONV[L]; alive = [n0 * (1 - churn) ** m for m in range(3)]
            print(f"  {L:14} churn {churn:4.0%}: per-customer contrib ${unit:5.2f}, day-90 customers {alive[2]*(1-churn):5.0f}, months 1-3 ${sum(a*unit for a in alive):9,.0f}, lifetime ${n0*unit/churn:10,.0f}")
    print('\nVolume (measured 2026-10-14: amux.io 3,596 unique visitors/30d, 178 viewed /pricing; GitHub 459 uniques/14d)')
    for pay_rate in (0.01, 0.02, 0.05, 0.08):  # H: share of pricing-page viewers who buy
        for growth in (0.0, 0.10):              # H: monthly growth of pricing-page viewers
            n, m, viewers = 0.0, 0, 178.0
            while n < 100 and m < 60:
                n = n * (1 - 0.05) + viewers * pay_rate; viewers *= 1 + growth; m += 1
            print(f"  {pay_rate:4.0%} of pricing viewers buy, viewers grow {growth:4.0%}/month: 100 customers in {m if n >= 100 else '60+'} months")
        ceiling = 178 * pay_rate / 0.05
        print(f"    flat-traffic ceiling at 5% churn: {ceiling:4.0f} customers, about ${month(max(1,round(ceiling)), '$39 entry', 'Merchant of record (Paddle)')[4]:,.0f}/month contribution at $39")
