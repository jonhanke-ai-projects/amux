#!/usr/bin/env bash
# Deploy the amux Cloud control plane to its own small Hetzner server
# (docs/design/cloud-self-service.md section 3). Idempotent: run again to ship new code.
#
#   CONTROL_HOST=cp-test.amux.io cloud/control/deploy_control.sh
#
# Reads HCLOUD_TOKEN and AMUX_CLOUD_CF_TOKEN (else CLOUDFLARE_API_TOKEN) from ~/.amux/server.env. Keeps the control
# plane's own secrets in ~/.amux/cloud/ (mode 600): control-secret, admin-token.
# Secrets reach the server over SSH, never through cloud-init metadata.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
HOST="${CONTROL_HOST:-cp-test.amux.io}"
NAME="amux-control-${HOST%%.*}"
KEY="${CONTROL_SSH_KEY:-$HOME/.ssh/amux_wexus}"
KEY_NAME="${CONTROL_SSH_KEY_NAME:-amux-wexus}"
envv() { grep -E "^(export )?$1=" "$HOME/.amux/server.env" | head -1 | cut -d= -f2- | tr -d '"'; }
# AMUX_CLOUD_CF_TOKEN is the control plane's own token: R2, API tokens, and DNS plus SSL on the
# customer zone only. The shared CLOUDFLARE_API_TOKEN is the fallback and lacks R2 and Origin CA.
HC="$(envv HCLOUD_TOKEN)"; CF="$(envv AMUX_CLOUD_CF_TOKEN)"; [[ -n "$CF" ]] || CF="$(envv CLOUDFLARE_API_TOKEN)"
[[ -n "$HC" && -n "$CF" ]] || { echo "HCLOUD_TOKEN and CLOUDFLARE_API_TOKEN must be in ~/.amux/server.env" >&2; exit 1; }
mkdir -p "$HOME/.amux/cloud"; chmod 700 "$HOME/.amux/cloud"
for f in control-secret admin-token; do
  [[ -s "$HOME/.amux/cloud/$f" ]] || { python3 -c 'import secrets;print(secrets.token_urlsafe(32))' > "$HOME/.amux/cloud/$f"; chmod 600 "$HOME/.amux/cloud/$f"; }
done
hapi() { curl -fsS -H "Authorization: Bearer $HC" -H 'Content-Type: application/json' "$@"; }
capi() { curl -fsS -H "Authorization: Bearer $CF" -H 'Content-Type: application/json' "$@"; }
log() { echo "[deploy-control] $*"; }

# ── server ──
SID=$(hapi "https://api.hetzner.cloud/v1/servers?name=$NAME" | python3 -c 'import json,sys;s=json.load(sys.stdin)["servers"];print(s[0]["id"] if s else "")')
if [[ -z "$SID" ]]; then
  log "creating $NAME (cx23, fsn1)"
  SID=$(hapi -X POST https://api.hetzner.cloud/v1/servers -d "$(python3 - "$NAME" "$KEY_NAME" <<'PY'
import json, sys
print(json.dumps({"name": sys.argv[1], "server_type": "cx23", "location": "fsn1", "image": "ubuntu-24.04",
                  "ssh_keys": [sys.argv[2]], "labels": {"amux-control": "1"},
                  "user_data": "#cloud-config\npackage_update: true\npackages: [python3-jwt, python3-cryptography, python3-yaml, sqlite3, debian-keyring, debian-archive-keyring, apt-transport-https, curl, gnupg]\nruncmd:\n  - [bash, -c, \"curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/gpg.key | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg && curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt > /etc/apt/sources.list.d/caddy-stable.list && apt-get update -qq && apt-get install -y -qq caddy && touch /var/lib/amux-control-ready\"]\n"}))
PY
)" | python3 -c 'import json,sys;print(json.load(sys.stdin)["server"]["id"])')
fi
IP=""
for _ in $(seq 1 60); do
  IP=$(hapi "https://api.hetzner.cloud/v1/servers/$SID" | python3 -c 'import json,sys;s=json.load(sys.stdin)["server"];print(s["public_net"]["ipv4"]["ip"] if s["status"]=="running" else "")')
  [[ -n "$IP" ]] && break; sleep 5
done
[[ -n "$IP" ]] || { log "FATAL: server $SID never reached running"; exit 1; }
log "server $SID at $IP"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$HOME/.amux/cloud/known_hosts" -o ConnectTimeout=10 "root@$IP")
for _ in $(seq 1 90); do "${SSH[@]}" test -f /var/lib/amux-control-ready 2>/dev/null && break; sleep 10; done
"${SSH[@]}" test -f /var/lib/amux-control-ready || { log "FATAL: cloud-init did not finish on $IP"; exit 1; }

# ── DNS: DNS-only so Caddy gets its own certificate ──
read -r ZID ACCT < <(capi "https://api.cloudflare.com/client/v4/zones?name=${HOST#*.}" | python3 -c 'import json,sys;z=json.load(sys.stdin)["result"][0];print(z["id"], z["account"]["id"])')
RID=$(capi "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records?name=$HOST" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')
REC="{\"type\":\"A\",\"name\":\"$HOST\",\"content\":\"$IP\",\"proxied\":false,\"ttl\":60,\"comment\":\"amux control plane\"}"
if [[ -z "$RID" ]]; then capi -X POST "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records" -d "$REC" >/dev/null
else capi -X PUT "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records/$RID" -d "$REC" >/dev/null; fi
log "DNS $HOST -> $IP"

# ── code and config ──
COPYFILE_DISABLE=1 tar --no-mac-metadata -C "$REPO" -czf - cloud/control | "${SSH[@]}" "mkdir -p /opt/amux && tar -C /opt/amux -xzf - && id amux-control >/dev/null 2>&1 || useradd --system --home /var/lib/amux-control --create-home amux-control"
# Stripe test mode (T-PAY-1): with STRIPE_TEST_SECRET_KEY in server.env the control plane takes real
# test-mode checkouts. stripe_setup.py makes the products, prices and webhook; the webhook secret
# is shown once, so it is kept in ~/.amux/cloud/stripe-test-webhook-secret.
STK="$(envv STRIPE_TEST_SECRET_KEY)"; WHS=""
if [[ -n "$STK" ]]; then
  WF="$HOME/.amux/cloud/stripe-test-webhook-secret"
  NEW=$(STRIPE_TEST_SECRET_KEY="$STK" python3 "$HERE/stripe_setup.py" --control "https://$HOST")
  if [[ -n "$NEW" ]]; then ( umask 077; printf '%s\n' "$NEW" > "$WF" ); fi
  WHS="$(cat "$WF" 2>/dev/null || true)"
  [[ -n "$WHS" ]] || { log "FATAL: the Stripe test webhook exists but its secret is not in $WF; delete the endpoint in Stripe and rerun"; exit 1; }
fi
# Sign in with Apple: the .p8 key travels base64-encoded. Its PEM has spaces and newlines, and
# systemd reads a backslash in an unquoted value as an escape, so '\\n' arrived as 'n'.
APK=""; if [[ -s "$HOME/.amux/cloud/apple-signin.p8" ]]; then APK="$(base64 < "$HOME/.amux/cloud/apple-signin.p8" | tr -d '\n')"; fi
GOLDEN="${AMUX_GOLDEN_IMAGE:-$(hapi 'https://api.hetzner.cloud/v1/images?type=snapshot&label_selector=amux-golden&sort=created:desc' | python3 -c 'import json,sys;i=json.load(sys.stdin)["images"];print(i[0]["id"] if i else "")')}"
{
  echo "CONTROL_DB=/var/lib/amux-control/control.db"
  echo "CONTROL_PORT=8090"
  echo "CONTROL_PUBLIC_URL=https://$HOST"
  echo "CONTROL_SECRET=$(cat "$HOME/.amux/cloud/control-secret")"
  echo "CONTROL_ADMIN_TOKEN=$(cat "$HOME/.amux/cloud/admin-token")"
  echo "CONTROL_PUBLIC_IPS=$IP"
  if [[ -n "$STK" ]]; then
    echo "CONTROL_FAKE_BILLING=0"; echo "STRIPE_SECRET_KEY=$STK"; echo "STRIPE_WEBHOOK_SECRET=$WHS"
  else
    echo "CONTROL_FAKE_BILLING=${CONTROL_FAKE_BILLING:-1}"
  fi
  # if, not &&: a missing optional value must not be this block's exit status. Under set -e and
  # pipefail that aborted the first Stripe deploy after writing the env file and before the restart.
  for k in APPLE_TEAM_ID APPLE_KEY_ID APPLE_BUNDLE_ID; do v="$(envv $k)"; if [[ -n "$v" ]]; then echo "$k=$v"; fi; done
  if [[ -n "$APK" ]]; then echo "APPLE_PRIVATE_KEY=$APK"; fi
  # Mail goes out as hello@amux.io through Google Workspace (amux.io's MX). Google refuses the
  # account password over SMTP, so this is an app password: AMUX_CLOUD_SMTP_PASSWORD from the
  # environment (a vault secret scoped to this worker arrives there) or server.env.
  SMP="${AMUX_CLOUD_SMTP_PASSWORD:-$(envv AMUX_CLOUD_SMTP_PASSWORD)}"
  if [[ -n "$SMP" ]]; then
    echo "SMTP_URL=smtp://hello%40amux.io:$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1].replace(" ",""),safe=""))' "$SMP")@smtp.gmail.com:587"
  fi
  echo "HCLOUD_TOKEN=$HC"
  echo "CLOUDFLARE_API_TOKEN=$CF"
  echo "CF_ZONE_ID=$ZID"
  echo "CF_ACCOUNT_ID=$ACCT"   # R2 buckets and per-server tokens live on the account
  echo "AMUX_CUSTOMER_DOMAIN=${HOST#*.}"
  echo "AMUX_GOLDEN_IMAGE=$GOLDEN"
  echo "CONTROL_OWNER_EMAIL=${CONTROL_OWNER_EMAIL:-ethan@mixpeek.com}"
  echo "CONTROL_SSH_KEY_NAME=${CONTROL_SSH_KEY_NAME:-$KEY_NAME}"
} | "${SSH[@]}" "umask 077; cat > /etc/amux-control.env; chown root:amux-control /etc/amux-control.env; chmod 640 /etc/amux-control.env"
"${SSH[@]}" bash -s "$HOST" <<'REMOTE'
set -euo pipefail
HOST="$1"
cat > /etc/systemd/system/amux-control.service <<UNIT
[Unit]
Description=amux Cloud control plane
After=network-online.target
[Service]
User=amux-control
EnvironmentFile=/etc/amux-control.env
ExecStart=/usr/bin/python3 /opt/amux/cloud/control/app.py
Restart=always
RestartSec=3
[Install]
WantedBy=multi-user.target
UNIT
cat > /etc/caddy/Caddyfile <<CADDY
$HOST {
  reverse_proxy 127.0.0.1:8090
}
CADDY
ufw allow 22/tcp >/dev/null; ufw allow 80/tcp >/dev/null; ufw allow 443/tcp >/dev/null; ufw --force enable >/dev/null
# The synthetic customer (plan section 12): every 6 hours and after every deploy, a real
# browser pays, waits for its server, enters through the door and deletes itself. Node and
# Chromium are installed once; synthetic.mjs resolves playwright from /opt/amux/node_modules.
# Ubuntu 24.04 ships Node 18 and Playwright needs 20 or newer (first scheduled run, 2026-10-15).
if [[ "$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)" -lt 20 ]]; then
  apt-get remove -y -qq nodejs npm >/dev/null 2>&1 || true
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
  apt-get install -y -qq nodejs >/dev/null
fi
# Checked on the browser, not on node_modules: a failed first install leaves node_modules behind.
if ! ls /opt/amux/pw/chromium* >/dev/null 2>&1; then
  (cd /opt/amux && npm install -s --no-audit --no-fund playwright@1 >/dev/null)
  PLAYWRIGHT_BROWSERS_PATH=/opt/amux/pw /opt/amux/node_modules/.bin/playwright install --with-deps chromium >/dev/null
fi
echo "synthetic runtime: node $(node -v), $(ls -d /opt/amux/pw/chromium* | head -1)"
install -d -o amux-control -g amux-control /var/lib/amux-control/synthetic
cat > /etc/systemd/system/amux-synthetic.service <<UNIT
[Unit]
Description=amux Cloud synthetic customer (pays, waits for a server, enters, deletes itself)
After=amux-control.service
[Service]
Type=oneshot
User=amux-control
EnvironmentFile=/etc/amux-control.env
Environment=CONTROL_URL=https://$HOST PLAYWRIGHT_BROWSERS_PATH=/opt/amux/pw
WorkingDirectory=/var/lib/amux-control/synthetic
ExecStart=/usr/bin/node /opt/amux/cloud/control/synthetic.mjs
TimeoutStartSec=25min
UNIT
cat > /etc/systemd/system/amux-synthetic.timer <<UNIT
[Unit]
Description=amux Cloud synthetic customer every 6 hours
[Timer]
OnCalendar=00/6:17
Persistent=true
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable amux-control >/dev/null 2>&1
systemctl restart amux-control
systemctl reload caddy 2>/dev/null || systemctl restart caddy
systemctl enable --now amux-synthetic.timer >/dev/null 2>&1
REMOTE
# Every deploy runs the journey once (plan section 12); its result lands in /api/admin/synthetic.
for _ in $(seq 1 60); do curl -fsS "https://$HOST/health" >/dev/null 2>&1 && break; sleep 5; done
"${SSH[@]}" systemctl start --no-block amux-synthetic.service
log "synthetic customer started (journalctl -u amux-synthetic on the control plane)"
for _ in $(seq 1 60); do curl -fsS "https://$HOST/health" >/dev/null 2>&1 && break; sleep 5; done
log "health: $(curl -fsS "https://$HOST/health" || echo UNREACHABLE)"
log "admin: https://$HOST/admin (token in ~/.amux/cloud/admin-token)"
