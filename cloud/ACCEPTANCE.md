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
| 6 | Fixed, awaiting retest | Round 4 C6-4: a default Codex worker (no bypass) could not resolve github.com, because its workspace-write sandbox has network off and `-a never` cannot ask. Codex workers now launch with network access inside the sandbox (0aaa74e83). Earlier proof: 2026-10-18 on the e2e server: a new Codex worker cloned mixpeek/amux and wrote `~/work/report.txt` with CLOUD_E2E_OK, the commit sha and the four workspace crates. The fix was on the host: Ubuntu 24.04 blocked the user namespaces Codex's sandbox needs; bootstrap.sh and the hourly update.sh now allow them (fe9ac3b56) |
| 7 | Pass at 390px | 2026-10-18: steered that Codex worker from a 390px phone view by tapping Send; it appended STEER_OK_390 to its report. No horizontal overflow. A physical handset is not yet shown |
| 8 | Pass | Codex round 4: invite, join, Remove with Cancel and confirm, then the removed member's cookie and password are refused. Earlier: Round 3: invite, join and lockout pass. Revoking had no button (C8-3); Settings > People now has Remove, e2e `settings_team_member_remove` (d52cec2ad, 9b1b3c21f) |
| 9 | Update path proven live | r2c1455 moved its checkout and its image (live_commit 17d4bef36bde) with no manual step (bbca9bcf7, 17d4bef36). Reboot recovery passed in round 1. TLS renewal relies on Caddy and is not yet observed. |
| 10 | Partial | Hetzner daily backups are enabled on every server; a restore is not yet shown |
| 11 | Pass | Round 3: deleted, then signed up again with the same email and name; the new server reached ready |
| 12 | Pass | Codex round 4: hostname listed once, deletion email, Stripe canceled, server, DNS (NXDOMAIN on both nameservers) and images gone. Earlier: 2026-10-18 on r2c1455 and twice in round 3. A name bought again was listed twice (C12-3), now once (d52cec2ad). 2026-10-18 on e2efull-ba83: the Stripe webhook and the delete page raced and left the DNS record behind with no confirmation; erase now removes the address by name and treats 404 as done (595733e58). Re-confirming then reported success, sent the email, and the name returns NXDOMAIN |
| 13 | Partial | Cancel keeps service to period end; 30-day snapshot expiry not yet observed |
| 14 | Fixed | Email failures return 502 with a message; stuck orders email the operator once. Cloud mail uses a service account that never needs re-approval (097a9380c). |
| 15 | Pass | Codex round 4: a paid name returns 409 with no Checkout for the same and another email. Earlier: Duplicate checkouts are expired, webhook replays and DNS failures finish once. Round 3 C15-3: a paid name could open a second checkout before its server existed; now refused (d52cec2ad, offline suite `node cloud/start/test-worker.mjs`) |
| 16 | Pass in a real browser | FIRSTMONTH shows $0.00 in real Chrome; headless browsers are refused by Stripe |

Also found in the 2026-10-18 journey and fixed (7fac35d90): schedules never fired on a server without `AMUX_RS_SCHEDULER=1`, every Cloud server included (now on by default); `amux import` refused a real 239 MB export with 413 (now sent gzipped, 1 GiB limit), and imported schedules now arrive paused so a move cannot fire them twice.

Also found 2026-10-18 after the move, fixed:

- "Next run" was off by the time zone difference when the browser and server zones differ ("in 4h" for an every-minute schedule). The server now sends absolute instants (fe9ac3b56, e2e `scheduler-next-run-timezone`).
- A new worker's first task waited the full 10 minute steering deadline. Codex 0.163's braille welcome logo read as a busy spinner, and Claude workers on Cloud never had amux's status hooks (52b20f811). Codex hooks stay off on Cloud because Codex holds new hooks behind a review screen.
- Deleting an account could leave its DNS record behind when the webhook won the race (595733e58, offline tests C12-4).
- main's rust run was red for hours from unrelated tests, and the image only publishes from a green run, so Cloud servers stopped updating. Fixed the failing tests (8582c5714 by another lane, c01af610c). A red main stalls every Cloud server's updates; that is by design, and it means main must stay green.

Round 4 (Codex, 2026-10-18, image 48304e544): C6-3 bubblewrap, C7 at 390px, C8-3, C12, C15-3, the scheduler time zone and the Codex first-task delay (task running 7 s after login) pass. New C6-4, fixed above. The Claude first-task timing was blocked by the lack of an Anthropic credential in the tester's environment.

Also fixed after round 4: an OpenAI-only server read "No Anthropic API key set"; the banner now appears only when no model is connected (6709d041b). The worker header showed a guessed Codex model; it now reads the model from Codex's rollout (6709d041b). The site footer left a 200px gap on phones (0903f637b).
