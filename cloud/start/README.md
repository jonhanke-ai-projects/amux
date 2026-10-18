# amux Cloud signup (serverless)

Owner design, 2026-10-17: a static signup page, Stripe Checkout, and one serverless function. No
control-plane server and no database. Each customer gets a standalone amux server.

| Piece | Where |
|---|---|
| Signup, welcome, billing pages | `site/start/` on main (GitHub Pages at amux.io/start) |
| The function | `worker.mjs`, Cloudflare Worker `amux-cloud` at https://cloud.amux.io (live Stripe) and `amux-cloud-test` on workers.dev (Stripe test mode, needs `x-test-key`) |
| Deploy | `deploy.sh live` or `deploy.sh test` |
| Server image | the newest Hetzner snapshot labelled `amux-golden` (`cloud/control/build_image.py --ref <sha>`) |
| End-to-end test | `e2e.mjs` (see below) |
| Offline tests | `node cloud/start/test-worker.mjs`: the real module against fake Hetzner, Cloudflare and Stripe, with injected faults |

Flow: the page hashes the password in the browser (scrypt, the door's format) and posts plan,
name, email and hash to `/api/checkout`. Stripe Checkout takes payment. On
`checkout.session.completed` the function creates the server from the golden image with
`/etc/amux/standalone.env` (domain and the one login) and the DNS record. The server's bootstrap
gets its own Let's Encrypt certificate and runs its first self-update. A cron every minute
sends the ready email as hello@amux.io once the server's `/_door/status` shows a commit, a
dashboard that loads and an update verdict. On `customer.subscription.deleted` it snapshots the
server (kept 30 days), then deletes the server and the record. Deleting the account from the
billing page (an emailed one-hour link) ends the subscription and erases everything at once.

Every step of provisioning is idempotent. If the webhook fails part way (no Hetzner capacity, a
DNS error), Stripe redelivers it and the cron's `reconcile()` finishes any paid subscription
whose signup is unfinished (its `pw_hash` is still in Stripe metadata). An order still
unfinished after 10 minutes emails hello@amux.io once. The cron also checks each ready server
every 10 minutes (reachable, updater healthy, backups on, DNS record right) and emails after two
failed checks in a row.

State lives in Hetzner labels (`amux-sub`, `amux-slug`, `amux-mode`, `amux-ready`, `amux-ending`)
and Stripe metadata. The password hash is removed from Stripe once the server exists.

## Moving a local amux in

The customer is the server's owner (`AMUX_WEB_USERS` entry marked `owner`, no member token), so
the door's Basic auth with their email and password is all an import needs:

```bash
amux export                                    # on the local machine: amux-<host>-<date>.amuxmove.json
amux import amux-*.amuxmove.json --to https://<slug>.amux.io --user <email>   # dry run, then y
```

The password comes from `AMUX_IMPORT_PASSWORD` or a prompt, never argv. Import adds rows and
never overwrites; teammates (org members) are refused with 403. Format and rules:
`docs/design/cloud-self-service.md` section 7.1, `crates/amux-server/src/api/move_bundle.rs`.

## Testing it

Stripe runs an invisible hCaptcha on Checkout that holds back automated browsers (Playwright,
headed or headless, test or live mode). The visitor journey therefore runs in a real Chrome
through `amux cdp` (2026-10-17 run: paid to ready in 69 s, door sign-in, dashboard, portal
cancel, deletion in 97 s). `e2e.mjs` covers everything up to Checkout in Playwright and the rest
when Stripe lets it through.
