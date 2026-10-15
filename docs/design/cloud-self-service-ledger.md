# amux Cloud execution ledger

Task state for executing [cloud-self-service.md](cloud-self-service.md). The
`amux-cloud` worker is isolated and has no board, so this file is the record.
Statuses: done (committed, with evidence), blocked (with the owner question),
todo.

## Path to 10 (plan 13.2)

| # | Step | Status | Evidence or blocker |
|---|---|---|---|
| 0 | Demand check: waitlist form on /cloud | done | `site/cloud/index.html`; browser test with PostHog loaded and blocked at 1280 and 375 px; control-plane `/api/notify` for when it is live |
| 1a | `plans.json` and the price check (M13, T-SITE-2) | done | `cloud/control/price_check.py`; 8 tests; live Hetzner margin check passes |
| 1b | Pricing page, cloud page and homepage CTA without trial or Calendly | partly done | Prices approved 2026-10-14; `site/start/` plan cards switch to checkout only when `/health` reports live payments. The /cloud rewrite waits on O3 |
| 2 | Control plane: checkout-first accounts, Stripe, servers, provision jobs, redirect, emails, kill switches | done | `cloud/control/`; fake-mode Chromium journey PASS 11 checks |
| 3 | Golden image, door token mode, model sign-in screen, templates | done | Six real image builds on Hetzner; each failure is listed under real runs below; door tests against a real amux-server; CLI login fixtures for Claude Code 2.1.299, Codex 0.162.1, Gemini CLI 0.64.0 |
| 4 | Fleet agent: backups, restore drill, signed releases, heartbeat | done except the restore drill job | release install and rollback tested with real ssh-keygen signatures; per-server R2 bucket and scoped token |
| 5 | Safe lifecycle in shadow mode, dunning, cancel, deletion, abuse, disputes | done except abuse isolation (M17) | 17 lifecycle tests, every one of 11 safety-rule mutations reddens a named test; account deletion with retried steps; dispute evidence |
| 6 | Invites through cloud.amux.io; add to tailnet | done | door redeems invites against a real amux-server; `/api/tailnet/*`; tailnet-only switch refused without a tailnet |
| 7 | iOS: Sign in with Apple and web sign-in | done, not yet on a device | app builds for the simulator; control-plane Apple verification tested with generated keys; no-purchase string scan; device test needs the Apple capability (P3) |
| 8 | Journey every 6 hours, nightly capability suite, weekly report | journey scheduled and passing; capability suite todo | `deploy_control.sh` installs Node 22 and Chromium on the control plane and an `amux-synthetic` timer (every 6 hours) and runs it on every deploy; first scheduled run passed all five steps in 2 min 4 s and recorded `synthetic.run ok`. Checkout is still the fake one until a Stripe test key exists |
| - | Portability: amux.yaml, export/import, `amux cloud move/pull/apply/login` | done | export from one real amux-server and import into another: every kind, idempotent re-import, schedules disabled, workers not started, secret-shaped values refused (5 tests) |

## Real infrastructure runs (2026-10-14)

Approved spend: control plane cx23, image builds, up to $25/month of test servers.

| Run | What failed | Fix |
|---|---|---|
| Image build 1 | A typo'd git ref, and the builder powered off whether or not bootstrap passed, so a plain Ubuntu image was saved | Power off only after `IMAGE_OK`; image deleted |
| Image build 2 | dpkg failed on usbmuxd pulled in as a recommendation | `--no-install-recommends` plus one repair-and-retry |
| Image build 3 | `amux-server-rs --version` is not a flag, so the server started serving and the build never ended | Log a sha256 of the binary instead |
| Customer run 1 | 80 GB image cannot boot a cx23; root password expired so SSH refused | Drill uses cx33; `chage` in the image; ops key on creation |
| Customer run 1 | Cloudflare refused the Origin certificate (1016, token lacks permission) | ACME fallback: record goes DNS-only, Caddy gets Let's Encrypt, `tls_mode=acme` |
| Image build 5 | The builder's SSH key was baked into the image | Image cleanup removes `authorized_keys` |
| Customer run 2 | First boot ran `install` of `/opt/amux/bootstrap.sh` onto itself and exited 1 every minute; the control plane failed it at 15 minutes, deleted the server and queued the refund for review | Skip files already in place; agent keeps the end of stderr; reproduced and verified on a scratch server (`e09b3a17`) |
| Customer run 2 | Backup credentials failed on a missing `CF_ACCOUNT_ID` | `deploy_control.sh` reads it from the zone |
| Customer run 3 (image 443614609) | Everything up to the first backup works: ACME certificate, door sign-in page, API 401 without a login, door hand-off into the dashboard with `/api/sessions` 200. Backup credentials fail on the real permission (R2 403), so the server never reaches ready; the control plane failed it at 15 minutes and removed the server and its DNS record | Needs the Cloudflare token ask. The synthetic's dashboard check matched only a hidden modal; it now waits for `#brand-header` and an API call through the door |

| Customer run 4 | **Passed end to end**: Origin certificate issued, first backup, ready, door hand-off and dashboard 2 min 32 s after payment (fake checkout, real Hetzner and Cloudflare) | A scoped control-plane token, `AMUX_CLOUD_CF_TOKEN` (R2 Edit, API Tokens Edit, DNS, SSL and Zone Read on amux.io only), created through the owner's signed-in Cloudflare dashboard over CDP |
| Run 4 account deletion | Server, DNS, subscription and account went; the bucket retried forever because R2 refuses a non-empty bucket, and backup tokens were never revoked | `delete_bucket` empties through R2's S3 API with a one-hour bucket-scoped token, deletes, and revokes every `amux-backup-<bucket>` token; the stuck record then finished with every step done |

## Where the build departed from the plan, and why

| Plan said | Built | Why |
|---|---|---|
| Door verifies an Ed25519 token | HMAC-SHA256 with a per-server secret | Same isolation (a token for one server cannot open another, T-SSO-1) while the door stays on the Python standard library |
| iOS Sign in with Apple through Clerk's iOS SDK | The control plane verifies Apple's identity token itself and keeps the refresh token for revocation | No SDK in the app, works with the control plane's own session, and revocation on deletion does not depend on what Clerk keeps |
| The move bundle merges an allowlist of database tables through a staging copy | Export and import through amux's public API, matched on natural keys | No raw database ever leaves a machine, it works across server versions without migrations, and every write passes the target's own checks |
| Payments through Paddle | Stripe (owner decision, 2026-10-14) | Stripe Tax and automatic dispute evidence cover what Paddle would have |

## Owner asks

| Ask | Why it blocks | Unblocks |
|---|---|---|
| A Stripe test-mode secret key in `~/.amux/server.env` as `STRIPE_TEST_SECRET_KEY` | Checkout, webhook and dunning tests against real Stripe test mode | The key present. Tried 2026-10-14: no saved profile and no Chrome session is signed in to Stripe; the login is the owner's |
| Sign in with Apple enabled on the App ID, plus a Services ID and a Sign in with Apple key (P3) | The iOS sign-in on a device, and token revocation on deletion | The capability on, and the key in `server.env` as `APPLE_TEAM_ID`, `APPLE_KEY_ID`, `APPLE_PRIVATE_KEY`. Tried 2026-10-14: Chrome lands on Apple's sign-in page and no profile holds an Apple login |
| Offer to existing $20 and $200 tunnel subscribers (O3) | Rewriting /cloud and /pricing for the new product | A yes on the plan's recommendation (old price for 12 months) and on the drafted email, [cloud-o3-subscriber-email.md](cloud-o3-subscriber-email.md); the count needs a Stripe key |
