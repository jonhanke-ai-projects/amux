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

Flow: the page hashes the password in the browser (scrypt, the door's format) and posts plan,
name, email and hash to `/api/checkout`. Stripe Checkout takes payment. On
`checkout.session.completed` the function creates the server from the golden image with
`/etc/amux/standalone.env` (domain and the one login) and the DNS record. The server's bootstrap
gets its own Let's Encrypt certificate. A cron every minute sends the ready email as
hello@amux.io. On `customer.subscription.deleted` it snapshots the server (kept 30 days), then
deletes the server and the record.

State lives in Hetzner labels (`amux-sub`, `amux-slug`, `amux-mode`, `amux-ready`, `amux-ending`)
and Stripe metadata. The password hash is removed from Stripe once the server exists.

## Testing it

Stripe runs an invisible hCaptcha on Checkout that holds back automated browsers (Playwright,
headed or headless, test or live mode). The visitor journey therefore runs in a real Chrome
through `amux cdp` (2026-10-17 run: paid to ready in 69 s, door sign-in, dashboard, portal
cancel, deletion in 97 s). `e2e.mjs` covers everything up to Checkout in Playwright and the rest
when Stripe lets it through.
