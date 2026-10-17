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
| 8 | Journey every 6 hours, nightly capability suite, weekly report | journey on real Stripe test checkout, passing | Every 6 hours and on deploy: T-PAY-1 (test checkout, EU tax, webhook replay), T-DUN-1 (Stripe half), T-PROV-1, T-SSO-1, door locks; last run 9 passed, T-PROV-1 238 s. Not run yet: model and CLI sign-ins, email, tailnet, team invites |
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

| Scheduled journeys (2026-10-15) | Ubuntu's Node 18 is too old for Playwright; two runs failed at a check before the self-delete and leaked their servers until the Hetzner core quota refused the next; that quota error reached only the log; nothing set `servers.synthetic`, so releases could never leave stage 0 and each journey server got its own restore drill | Node 22 from NodeSource; the synthetic deletes its account whenever payment went through; a failed job keeps its last error and a provider limit alerts at once; a synthetic checkout flags its server and drills skip it (`b4318cb2`) |

| Restore drills and deletion (2026-10-15) | Every drill heartbeat minted a backup token and re-created the bucket if missing, so a drill whose source was deleted re-created that customer's bucket and minted 31 tokens; the hourly sweep let a stuck drill live up to 105 minutes; token revocation read only the first page; an account deleted mid-provision left its bucket | Drills mint one `amux-drill-<id>` token without creating anything and revoke it at the end, cancel when the source is gone, and sweep every 5 minutes; revocation reads every page; deletion always plans the bucket step. After the fixes a journey passed in 133 s and Hetzner, R2 and the token list held nothing of ours |

| O3 resolved by count (2026-10-15) | Read-only against the live Stripe account: no active, trialing or past-due subscription on either amux Cloud price (one canceled annual) | No email to send; the draft is kept in case someone subscribes before launch |

| Stripe test mode and Apple key (2026-10-15) | The owner added a test key and the Apple key. Found on the way: the key line had no trailing newline, so the next append glued onto it; a test key would have reported payments live and opened /start to the public (card 4242 provisions real servers); the first Stripe deploy stopped silently before restarting the service; the Apple PEM did not survive systemd's environment file | Appends check for a newline; payments has a third state, test, which keeps checkout admin-only; optional deploy lines cannot abort the deploy; the Apple key travels base64. Proven: a journey paid with a Stripe test card, Stripe's webhooks arrived, deletion cancelled the subscription in Stripe; T-DUN-1's Stripe half (failed charge seen as past_due with its start time, fixed card back to active); Apple answered `invalid_grant` to a bogus code signed with our key, so key, Key ID, team and bundle are accepted |

| EU tax, webhook replay, speed (2026-10-15) | Test mode had no tax registrations, so Stripe charged tax nowhere; the paid-to-ready target missed on two of three runs (217, 145, 238 s) | A test-mode EU OSS registration (no legal effect; live registration is the owner's, with an accountant). Journeys now check EU tax (741 cents on 3900 for Berlin, 19%) and webhook replay (accepted, still one server). Timing: boot is 11 s and cloud-init under 3 s, while Hetzner took 175 s to turn the 80 GB-disk snapshot into a running server. An image built on a 40 GB disk (443929900) restored in 50 s against 43 s for the 80 GB one, side by side, both far below the 175 s seen earlier: the spread is Hetzner's, not the image's. The remaining lever is a warm pool, which costs money every month and is the owner's call |
| Customer email live (2026-10-16) | Mail was queued and never sent: no provider. Turning one on would also have sent 56 queued test emails | Sends as `amux Cloud <hello@amux.io>` through the Gmail API with the account's existing amux Gmail grant (Google refuses the account password over SMTP). Test addresses and mail queued over two days ago are marked suppressed (56 on cp-test). A `server_ready` email to hello@amux.io was sent and found in its inbox. amux.io's SPF gained `include:_spf.google.com`; DKIM was already set. The deploy also stopped on an optional setting lookup under `set -e`, fixed |
| No EU VAT (owner, 2026-10-17) | The plan assumed an EU OSS registration and VAT returns | Test-mode IE registration expired (a Berlin Solo month now carries 0 tax, as live will); the journey's EU tax check removed; plan rows for VAT filing removed. Stripe Tax stays on with no registrations |
| Sign in with Apple on a device (2026-10-17) | Reuses the existing iOS app, team and App Store Connect pipeline (owner: "re-use that"). The app was hard-coded to cloud.amux.io, which still serves the old cloud | `AMUX_CLOUD_URL` build setting; `ios.yml` manual run with `cloud_url=https://cp-test.amux.io` uploaded TestFlight 1.1.25 (run 39035368503); signing with the Sign in with Apple entitlement succeeded. Left: one tap on a real phone. The nightly from main (06:00 UTC) will supersede it with a build that has no cloud sign-in until this branch merges |

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
| Paid-to-ready target versus Hetzner's restore spread (43 to 175 s measured) | T-PROV-1 misses 3 minutes on slow restores; nothing in our boot path can recover it (boot 11 s) | **Decided 2026-10-15 by the owner:** no warm pool. Three minutes is the typical time, not a ceiling. The progress page and the first email say the customer can leave, and a `server_ready` email goes to the owner of every non-synthetic server when it comes up |
| Sign in with Apple on a real iPhone (T-ACC-3, iOS half) | The control plane side is proven; the app's native sheet only runs on a device signed into an Apple ID | A device run from Xcode, or a TestFlight build |
