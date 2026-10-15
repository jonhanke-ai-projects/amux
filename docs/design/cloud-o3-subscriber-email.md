# O3: the email to existing amux Cloud subscribers (draft for owner approval)

Plan reference: [cloud-self-service.md](cloud-self-service.md) section O3 and P9.

Status: **not needed today (no current subscribers); draft kept, not sent.** It goes to customers, so the owner approves the text and
sends it. Nothing in the control plane sends it automatically.

## Who gets it

Nobody, as of 2026-10-15. Counted read-only against Mixpeek, Inc.'s live Stripe account
(the key in the `mixpeek-ops-secrets` GKE secret):

- Amux Cloud Pro Monthly ($20, `price_1TKPz7…`): no subscription ever.
- Amux Cloud Pro Annual ($200, `price_1TKPzr…`): one, canceled. None active, trialing or past due.

The query was checked against the same account's other products, which do have active
subscriptions, so the zero is a real count. The draft below stays for the case where
someone subscribes on the old prices before launch.

## The offer

- The new plan of their choice at their old price for 12 months: Solo at $20 a month
  (or $200 a year) instead of $39.
- Their current subscription ends at the end of the period they already paid for.
  Nobody is charged twice and nobody loses time they paid for.
- The link is a Stripe Checkout session with a 12-month coupon, one per customer,
  made when the owner approves this draft.

## Subject

Your amux Cloud server is ready to move

## Body

Hi {first_name},

You've been on amux Cloud since {start_month}. We've rebuilt it: every account
now gets its own dedicated server at {name}.amux.io, with daily backups, your own
model key, and sign-in with Claude Code, Codex or Gemini CLI.

You keep your current price for the next 12 months. This link moves you to the
new plan at {old_price}:

{checkout_link}

Your current subscription runs until {period_end} and then ends on its own. You
won't be charged twice.

If you want to bring your existing workers, board and schedules with you, run
`amux cloud move` from your current setup after the new server is ready.

Questions: reply to this email.

Ethan
