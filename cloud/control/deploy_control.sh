#!/usr/bin/env bash
# Deploy the amux Cloud control plane to its own small Hetzner server
# (docs/design/cloud-self-service.md section 3). Idempotent: run again to ship new code.
#
#   CONTROL_HOST=cp-test.amux.io cloud/control/deploy_control.sh
#
# Reads HCLOUD_TOKEN and CLOUDFLARE_API_TOKEN from ~/.amux/server.env. Keeps the control
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
HC="$(envv HCLOUD_TOKEN)"; CF="$(envv CLOUDFLARE_API_TOKEN)"
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
ZID=$(capi "https://api.cloudflare.com/client/v4/zones?name=${HOST#*.}" | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"][0]["id"])')
RID=$(capi "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records?name=$HOST" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')
REC="{\"type\":\"A\",\"name\":\"$HOST\",\"content\":\"$IP\",\"proxied\":false,\"ttl\":60,\"comment\":\"amux control plane\"}"
if [[ -z "$RID" ]]; then capi -X POST "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records" -d "$REC" >/dev/null
else capi -X PUT "https://api.cloudflare.com/client/v4/zones/$ZID/dns_records/$RID" -d "$REC" >/dev/null; fi
log "DNS $HOST -> $IP"

# ── code and config ──
COPYFILE_DISABLE=1 tar --no-mac-metadata -C "$REPO" -czf - cloud/control | "${SSH[@]}" "mkdir -p /opt/amux && tar -C /opt/amux -xzf - && id amux-control >/dev/null 2>&1 || useradd --system --home /var/lib/amux-control --create-home amux-control"
GOLDEN="${AMUX_GOLDEN_IMAGE:-$(hapi 'https://api.hetzner.cloud/v1/images?type=snapshot&label_selector=amux-golden&sort=created:desc' | python3 -c 'import json,sys;i=json.load(sys.stdin)["images"];print(i[0]["id"] if i else "")')}"
{
  echo "CONTROL_DB=/var/lib/amux-control/control.db"
  echo "CONTROL_PORT=8090"
  echo "CONTROL_PUBLIC_URL=https://$HOST"
  echo "CONTROL_SECRET=$(cat "$HOME/.amux/cloud/control-secret")"
  echo "CONTROL_ADMIN_TOKEN=$(cat "$HOME/.amux/cloud/admin-token")"
  echo "CONTROL_PUBLIC_IPS=$IP"
  echo "CONTROL_FAKE_BILLING=${CONTROL_FAKE_BILLING:-1}"
  echo "HCLOUD_TOKEN=$HC"
  echo "CLOUDFLARE_API_TOKEN=$CF"
  echo "CF_ZONE_ID=$ZID"
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
systemctl daemon-reload
systemctl enable amux-control >/dev/null 2>&1
systemctl restart amux-control
systemctl reload caddy 2>/dev/null || systemctl restart caddy
REMOTE
for _ in $(seq 1 60); do curl -fsS "https://$HOST/health" >/dev/null 2>&1 && break; sleep 5; done
log "health: $(curl -fsS "https://$HOST/health" || echo UNREACHABLE)"
log "admin: https://$HOST/admin (token in ~/.amux/cloud/admin-token)"
