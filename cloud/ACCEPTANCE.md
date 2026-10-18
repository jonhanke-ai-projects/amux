# amux Cloud acceptance criteria

The goal, in the owner's words: "I never think about amux cloud again and its entirely set and
forget (users sign up -> get cloud can handle their account, delete, etc.) cloud instances get all
the amux updates as they come out etc."

amux Cloud is done when every item below passes as a real customer would experience it, with
evidence. An independent Codex thread ("#amux cloud e2e") runs the checks and reports failures to the
amux-cloud worker. Its brief and evidence are under `~/.amux/cloud/codex-e2e/`.

## Criteria

1. **Understand the offer.** From amux.io a visitor learns what amux is, that self-hosting is free,
   and what Cloud adds and costs. Works on desktop and a 390px phone with no overflow and no console
   errors.
2. **Buy without help.** A visitor gets from the homepage button through Stripe Checkout to a paid
   order.
3. **Told when ready.** On screen and by email, within a few minutes, even when the first attempt
   to create the server fails.
4. **Sign in to their own server.** Wrong credentials and other people are refused.
5. **Connect an AI account.** Claude, ChatGPT or Gemini subscription, or an API key, from the
   dashboard without help.
6. **First real work.** A first worker does real work, including on the customer's own GitHub repo.
7. **Phone.** The customer reaches the server from the iOS app or PWA and steers a worker.
8. **Team.** The customer gives a teammate their own login and can revoke it.
9. **Healthy with nobody touching it.** New amux releases reach existing servers automatically,
   the server recovers from a crash or reboot, and TLS renews.
10. **Work survives a failure.** Backups exist and a restore works.
11. **Billing alone.** Change card, see invoices, cancel, resubscribe.
12. **Delete the account.** The customer deletes their account and data themselves, and the
    server, DNS record, snapshots and subscription are really gone.
13. **Clean cancellation.** Service runs to the end of the paid period, data is kept 30 days, then
    removed, and billing stops.
14. **Failures are clear.** Every failure the customer can hit shows a clear message and a way to
    get help, and the operator hears about it without watching a dashboard.
15. **Abuse and edge cases.** Double submit, replayed and forged webhooks, name collisions,
    reserved names, Hetzner capacity errors, failed payments and a browser closed mid-flow leave no
    duplicate or orphaned server, no charge without a server and no server without a charge.
16. **Live mode is sane.** cloud.amux.io is healthy, live checkout is amux-branded, and the
    FIRSTMONTH code shows $0.00 due today. No live purchase is made in testing.

## Status (2026-10-18)

| # | Status | Evidence |
|---|---|---|
| 1 | Pass | Codex round 1 |
| 2 | Pass | Codex round 1 |
| 3 | Fixed, proven live | The Worker cron finishes any paid order without a server (c9f139760). Test order r2c1455 hit Hetzner capacity errors for about 4 minutes and was finished at 363 seconds. Readiness waits for the dashboard and first update. |
| 4 | Pass | Codex round 1 |
| 5 | Pass (API key) | Codex round 3: an OpenAI key saved in Vault drives real Codex turns. A saved key now also signs Codex in before its login picker (2f2f148f6) |
| 6 | Fixed, awaiting retest | Round 3 C6-3: Codex's bubblewrap sandbox cannot start in the container, so every command failed. Launch now falls back to the container as the sandbox (d52cec2ad) |
| 7 | Pass at 390px | Round 3: a phone-width Send started a model turn and got its ACK. A physical handset is not yet shown |
| 8 | Fixed, awaiting retest | Round 3: invite, join and lockout pass. Revoking had no button (C8-3); Settings > People now has Remove, e2e `settings_team_member_remove` (d52cec2ad, 9b1b3c21f) |
| 9 | Update path proven live | r2c1455 moved its checkout and its image (live_commit 17d4bef36bde) with no manual step (bbca9bcf7, 17d4bef36). Reboot recovery passed in round 1. TLS renewal relies on Caddy and is not yet observed. |
| 10 | Partial | Hetzner daily backups are enabled on every server; a restore is not yet shown |
| 11 | Pass | Round 3: deleted, then signed up again with the same email and name; the new server reached ready |
| 12 | Proven live | 2026-10-18 on r2c1455 and twice in round 3. A name bought again was listed twice (C12-3), now once (d52cec2ad) |
| 13 | Partial | Cancel keeps service to period end; 30-day snapshot expiry not yet observed |
| 14 | Fixed | Email failures return 502 with a message; stuck orders email the operator once. Cloud mail uses a service account that never needs re-approval (097a9380c). |
| 15 | Fixed | Duplicate checkouts are expired, webhook replays and DNS failures finish once. Round 3 C15-3: a paid name could open a second checkout before its server existed; now refused (d52cec2ad, offline suite `node cloud/start/test-worker.mjs`) |
| 16 | Pass in a real browser | FIRSTMONTH shows $0.00 in real Chrome; headless browsers are refused by Stripe |

Also found in the 2026-10-18 journey and fixed (7fac35d90): schedules never fired on a server without `AMUX_RS_SCHEDULER=1`, every Cloud server included (now on by default); `amux import` refused a real 239 MB export with 413 (now sent gzipped, 1 GiB limit), and imported schedules now arrive paused so a move cannot fire them twice.
