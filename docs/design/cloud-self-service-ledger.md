# amux Cloud execution ledger

Task state for executing [cloud-self-service.md](cloud-self-service.md). The
`amux-cloud` worker is isolated and has no board, so this file is the record.
Statuses: todo, doing, done (merged, with evidence), blocked (with the owner
question).

| # | Plan step | Status | Evidence or blocker |
|---|---|---|---|
| 0 | Demand check: waitlist form on /cloud | done | `site/cloud/index.html`; browser test of loaded and blocked PostHog at 1280 and 375 px |
| 1a | `plans.json` and the price check (M13, T-SITE-2) | done | `cloud/control/price_check.py`; 8 unit tests; live Hetzner margin check passes |
| 1b | Pricing page, cloud page and homepage CTA without trial or Calendly | blocked | Owner: approve launch prices (O1) and the offer to existing $20 subscribers (O3) |
| 2 | Control plane: accounts, Stripe billing, servers, provision jobs, redirect, emails, kill switches | todo | |
| 3 | Golden image, door token mode, model sign-in screen, templates | todo | |
| 4 | Fleet agent: backups, restore drill, signed releases, heartbeat | todo | |
| 5 | Safe lifecycle in shadow mode, dunning, cancel, deletion, abuse, disputes | todo | |
| 6 | Invites through cloud.amux.io; add to tailnet | todo | |
| 7 | iOS: Sign in with Apple and web sign-in | todo | |
| 8 | Journey every 6 hours, nightly capability suite, weekly report | todo | |

## Owner asks

| Ask | Why it blocks | Unblocks |
|---|---|---|
| Approve launch prices $39 / $69 / $129 (O1) | Publishing prices and creating live Stripe products | A yes, or other prices |
| Approve Hetzner spend: one cx23 for the control plane ($6.49/month) and up to $25/month of short-lived test servers | Running the control plane and the real provisioning tests | A yes with a cap |
| Stripe test-mode secret key in `~/.amux/server.env` as `STRIPE_TEST_SECRET_KEY` | Checkout, webhook and lifecycle tests against real Stripe test mode | The key present |
| Offer to existing $20 and $200 tunnel subscribers (O3) | Rewriting /cloud and /pricing for the new product | A decision; the email itself goes out only after the owner approves its text |
