#!/usr/bin/env bash
# Keep a standalone amux host (amux Cloud, cloud/start) on the newest CI-built amux, with no one
# touching it. Run as root by amux-update.timer (bootstrap.sh installs it): hourly, and 5 minutes
# after boot.
#
#   1. Fast-forward the amux checkout (the bash CLI, this script, the unit templates).
#   2. Compare ghcr.io/mixpeek/amux:latest with the digest installed last time. The image is
#      rebuilt only after CI passes on main (.github/workflows/deploy-cloud.yml), so "latest" is a
#      green build.
#   3. New digest: copy its binaries in by rename, restart the server, and require /health to answer
#      within 120 s. Otherwise put the previous binaries back and restart again.
#   4. Make sure the server and Caddy are running even when nothing changed (crash recovery).
#
# The outcome is written to /var/lib/amux-update/status.json, which amux-door serves at
# /_door/status, so cloud.amux.io's monitor sees a failed or stuck update without logging in.
# Every run logs one line with verdict=... to the journal (journalctl -t amux-update).
set -uo pipefail
USER_NAME="${AMUX_USER:-amux}"
HOME_DIR="$(getent passwd "$USER_NAME" | cut -d: -f6)"
BIN_DIR="$HOME_DIR/.local/bin"
SRC="$HOME_DIR/Dev/amux"
IMAGE_REPO="${AMUX_IMAGE_REPO:-mixpeek/amux}"
STATE=/var/lib/amux-update
STATUS="$STATE/status.json"
install -d -m 755 "$STATE"
exec 9>"$STATE/lock"; flock -n 9 || { echo "another update is running"; exit 0; }

UID_N=$(id -u "$USER_NAME")
as_user() { runuser -u "$USER_NAME" -- env HOME="$HOME_DIR" "$@"; }
ucl() { as_user XDG_RUNTIME_DIR="/run/user/$UID_N" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$UID_N/bus" systemctl --user "$@"; }
say() { logger -t amux-update -- "$*"; echo "$*"; }
health() { curl -sk --max-time 5 https://127.0.0.1:8824/health; }

# Codex runs every worker command inside its own bubblewrap sandbox, which needs an unprivileged
# user namespace. Ubuntu 24.04 blocks those through AppArmor, so on a Cloud server every Codex
# command failed, even `pwd` ("bwrap: loopback: Failed RTM_NEWADDR", codex e2e round 3, C6-3).
# The server is single tenant, so allow them and let Codex's sandbox work.
USERNS=/proc/sys/kernel/apparmor_restrict_unprivileged_userns
if [[ -f "$USERNS" && "$(cat "$USERNS")" != 0 ]]; then
  echo 'kernel.apparmor_restrict_unprivileged_userns = 0' > /etc/sysctl.d/60-amux-codex-sandbox.conf
  if sysctl -q -p /etc/sysctl.d/60-amux-codex-sandbox.conf; then
    say "verdict=userns_allowed for the Codex sandbox"
  else
    say "verdict=userns_allow_failed Codex workers cannot run commands until user namespaces are allowed"
  fi
fi
commit_of() { health | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("commit",""))
except Exception: print("")'; }
wait_healthy() {
  for _ in $(seq 1 60); do [[ -n "$(commit_of)" ]] && return 0; sleep 2; done
  return 1
}
write_status() {  # verdict, detail
  python3 - "$STATUS" "$1" "$2" "$(cat "$STATE/digest" 2>/dev/null)" "$(commit_of)" "$(as_user git -C "$SRC" rev-parse --short HEAD 2>/dev/null)" <<'PY'
import json, os, sys, time
path, verdict, detail, digest, commit, checkout = sys.argv[1:]
old = {}
try: old = json.load(open(path))
except Exception: pass
now = int(time.time())
s = {"verdict": verdict, "detail": detail[:300], "checked_at": now, "digest": digest[:19],
     "commit": commit, "checkout": checkout,
     "updated_at": now if verdict == "updated" else old.get("updated_at", 0),
     "last_failure_at": now if verdict.startswith("failed") else old.get("last_failure_at", 0)}
tmp = path + ".tmp"; json.dump(s, open(tmp, "w")); os.chmod(tmp, 0o644); os.replace(tmp, path)
PY
  say "verdict=$1 commit=$(commit_of) $2"
}

# 1. Checkout (fast-forward only; a diverged checkout is reported, never forced).
SELF="$SRC/cloud/desktop/update.sh"
before=$(sha256sum "$SELF" 2>/dev/null | cut -c1-64)
if ! as_user git -C "$SRC" pull -q --ff-only 2>/dev/null; then
  say "WARN checkout $SRC did not fast-forward; keeping its current scripts"
fi
# This run is executing the bytes it started with. If the pull changed this script, run the new
# one now (once), so a fix in it lands in this run rather than an hour later.
if [[ -z "${AMUX_UPDATE_REEXEC:-}" && -f "$SELF" && "$(sha256sum "$SELF" | cut -c1-64)" != "$before" ]]; then
  say "update.sh changed in the pull; running the new version"
  exec 9>&-
  AMUX_UPDATE_REEXEC=1 exec bash "$SELF"
fi
as_user make -C "$SRC" install-cli BIN_DIR="$BIN_DIR" >/dev/null 2>&1 || say "WARN install-cli failed"

# The sign-in (door.py) ships from the checkout too, so its fixes arrive with the scripts.
if [[ -f "$SRC/cloud/desktop/door.py" ]] && ! cmp -s "$SRC/cloud/desktop/door.py" /opt/amux-door/door.py; then
  install -m 755 "$SRC/cloud/desktop/door.py" /opt/amux-door/door.py.new && mv /opt/amux-door/door.py.new /opt/amux-door/door.py
  systemctl restart amux-door && say "door.py updated from the checkout"
fi

# Config fixes that bootstrap.sh now writes, applied to servers bootstrapped before them. Each is
# idempotent, validated before it is used, and reverted if Caddy rejects it.
CF=/etc/caddy/Caddyfile
if [[ -f $CF ]] && grep -q 'header_up Cookie "amux_member=' $CF && ! grep -q 'header_up Host {host}' $CF; then
  cp -p $CF $CF.prev
  sed -i 's|^\( *\)header_up Cookie "amux_member=|\1header_up Host {host}\n\1header_up Cookie "amux_member=|' $CF
  if runuser -u caddy -- caddy validate --config $CF --adapter caddyfile >/dev/null 2>&1 && systemctl reload caddy; then
    say "caddy: amux now sees the public Host (team invite links)"
  else
    mv $CF.prev $CF; systemctl reload caddy; say "WARN caddy Host migration rejected; previous Caddyfile kept"
  fi
fi

# 2. Newest green image.
TOKEN=$(curl -fsS --max-time 20 "https://ghcr.io/token?scope=repository:$IMAGE_REPO:pull" | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])' 2>/dev/null)
LATEST=$(curl -fsSI --max-time 20 -H "Authorization: Bearer $TOKEN" \
  -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json' \
  "https://ghcr.io/v2/$IMAGE_REPO/manifests/latest" 2>/dev/null | awk 'tolower($1)=="docker-content-digest:"{print $2}' | tr -d '\r')
CURRENT=$(cat "$STATE/digest" 2>/dev/null || true)

if [[ -z "$LATEST" ]]; then
  VERDICT="failed_registry"; DETAIL="could not read the latest image digest from ghcr.io"
elif [[ "$LATEST" == "$CURRENT" ]]; then
  VERDICT="current"; DETAIL="already on the newest image"
else
  # 3. Install by rename, keep the previous binaries for rollback.
  CID=""
  cleanup() { [[ -n "$CID" ]] && docker rm -f "$CID" >/dev/null 2>&1; }
  trap cleanup EXIT
  if docker pull -q "ghcr.io/$IMAGE_REPO@$LATEST" >/dev/null && CID=$(docker create "ghcr.io/$IMAGE_REPO@$LATEST"); then
    ok=1
    for b in amux-server-rs amux-rs; do
      docker cp "$CID:/usr/local/bin/$b" "$BIN_DIR/$b.new" || ok=0
    done
    if [[ $ok == 1 ]]; then
      for b in amux-server-rs amux-rs; do
        [[ -x "$BIN_DIR/$b" ]] && cp -p "$BIN_DIR/$b" "$BIN_DIR/$b.prev"
        chown "$USER_NAME:$USER_NAME" "$BIN_DIR/$b.new"; mv "$BIN_DIR/$b.new" "$BIN_DIR/$b"
      done
      ucl restart amux-server.service
      if wait_healthy; then
        echo "$LATEST" > "$STATE/digest"
        VERDICT="updated"; DETAIL="installed ${LATEST:7:12}"
      else
        for b in amux-server-rs amux-rs; do [[ -x "$BIN_DIR/$b.prev" ]] && mv "$BIN_DIR/$b.prev" "$BIN_DIR/$b"; done
        ucl restart amux-server.service; wait_healthy
        VERDICT="failed_rolled_back"; DETAIL="${LATEST:7:12} did not pass /health in 120 s; previous binaries restored"
      fi
    else
      rm -f "$BIN_DIR"/amux-*.new
      VERDICT="failed_copy"; DETAIL="image ${LATEST:7:12} is missing a binary"
    fi
  else
    VERDICT="failed_pull"; DETAIL="docker could not pull ${LATEST:7:12}"
  fi
  docker image prune -af >/dev/null 2>&1 || true
fi

# 4. Recovery, whatever happened above.
if [[ -z "$(commit_of)" ]]; then
  ucl restart amux-server.service
  if wait_healthy; then DETAIL="$DETAIL; server was down and was restarted"
  else VERDICT="failed_server_down"; DETAIL="$DETAIL; amux-server does not answer /health after a restart"; fi
fi
systemctl is-active -q caddy || systemctl restart caddy
systemctl is-active -q amux-door 2>/dev/null || systemctl restart amux-door 2>/dev/null || true
write_status "$VERDICT" "$DETAIL"
