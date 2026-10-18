#!/usr/bin/env bash
# Deploy the amux cloud GATEWAY (cloud/gateway/) to the cloud host by hand and
# restart it. The normal path is a push to main (.github/workflows/deploy-cloud.yml);
# this is the manual one, for when CI cannot be used.
#
# Usage:
#   AMUX_CLOUD_HOST=<host> ./deploy-gateway.sh                      # deploy + restart
#   AMUX_CLOUD_HOST=<host> ./deploy-gateway.sh --mint-token <email> # also mint a tunnel token
#
# The host is a plain Linux box (Hetzner, bootstrapped by setup-cloud.sh) reached
# over ssh as root. There is no default host: the GCP VM this script used to
# name (`amux-dev`) is deleted, and a default that points at a dead or foreign
# address fails late and unclearly.
#   AMUX_CLOUD_HOST     address or ssh alias of the host (required)
#   AMUX_CLOUD_SSH_KEY  private key to offer (default ~/.ssh/amux_cloud, if it exists)
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC="$SCRIPT_DIR/gateway"
DEST="/opt/amux-cloud/gateway"
SVC="amux-gateway"
HOST="${AMUX_CLOUD_HOST:-}"
KEY="${AMUX_CLOUD_SSH_KEY:-$HOME/.ssh/amux_cloud}"

log() { echo "→ $*"; }
die() { echo "✗ $*" >&2; exit 1; }

[ -n "$HOST" ] || die "AMUX_CLOUD_HOST is not set. Name the cloud host, for example: AMUX_CLOUD_HOST=203.0.113.10 $0"
[ -f "$SRC/gateway.py" ] || die "gateway.py not found at $SRC"

SSH_OPTS=(-o ConnectTimeout=20 -o BatchMode=yes)
[ -f "$KEY" ] && SSH_OPTS+=(-i "$KEY")
SSH() { ssh "${SSH_OPTS[@]}" "root@$HOST" "$@"; }

log "Checking $HOST…"
SSH "test -d '$DEST'" || die "cannot reach root@$HOST, or $DEST is missing there (bootstrap the host with setup-cloud.sh first)"

# chatgpt_front.py first: gateway.py imports it.
STAMP="$(date +%s)"
for f in chatgpt_front.py gateway.py; do
  log "Copying $f…"
  SSH "cat > '/tmp/$f.new'" < "$SRC/$f"
  SSH "python3 -c 'import ast,sys; ast.parse(open(sys.argv[1]).read())' '/tmp/$f.new'" \
    || die "$f does not parse on the host; nothing was installed"
done
log "Backing up + installing…"
for f in chatgpt_front.py gateway.py; do
  SSH "[ ! -f '$DEST/$f' ] || cp '$DEST/$f' '$DEST/$f.bak.$STAMP'; mv '/tmp/$f.new' '$DEST/$f'"
done

log "Restarting $SVC…"
SSH "systemctl restart '$SVC' && sleep 2 && systemctl is-active '$SVC'"

log "Smoke test (tunnel routes should 402/404, not 500)…"
SSH "curl -s -o /dev/null -w 'POST /tunnel/register → %{http_code}\n' -X POST http://127.0.0.1:8080/tunnel/register || true"

if [ "${1:-}" = "--mint-token" ] && [ -n "${2:-}" ]; then
  log "Minting tunnel token for owner email: $2"
  # The host has no sqlite3 CLI. The email travels as an argument, never as SQL text.
  TOKEN="$(SSH "python3 - $(printf '%q' "$2")" <<'PY' | tr -d '\r' | tail -1
import secrets, sqlite3, sys, time
email = sys.argv[1]
db = sqlite3.connect("/var/amux/gateway.db")
row = db.execute("SELECT o.id FROM orgs o JOIN users u ON u.id=o.owner_id WHERE u.email=? LIMIT 1", (email,)).fetchone()
if not row:
    sys.exit("no org is owned by " + email)
tok = secrets.token_urlsafe(24)
db.execute("INSERT INTO tunnel_tokens (token,org_id,email,label,created_at) VALUES (?,?,?,?,?)",
           (tok, row[0], email, "cli", int(time.time())))
db.commit()
print(tok)
PY
)"
  [ -n "$TOKEN" ] || die "no token was minted"
  echo ""
  echo "  Tunnel token: $TOKEN"
  echo "  Configure locally: add AMUX_TUNNEL_TOKEN=<token> to ~/.amux/server.env, then restart the server."
fi

log "Done."
