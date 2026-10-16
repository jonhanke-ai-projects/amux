#!/usr/bin/env bash
# test-main-ci-watch.sh: the red-main watcher says WHY the board refused its
# card, and a failed fallback send is retried once with its error kept.
#
# 2026-10-16 13:23 (SCHED-713): two HTTP 400s and "could not message the amux
# lane either", and nothing said why. The 400s were the lane's WIP gate; the
# send went through from a bare environment minutes later. Every input here is
# a stub on PATH: no network, no board, no real lane.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$(mktemp -d "${TMPDIR:-/tmp}/mciw.XXXXXX")
trap 'rm -rf -- "${FIX:?}"' EXIT
fails=0
check() { if [ "$2" = "$3" ]; then echo "  ok   $1"; else echo "  FAIL $1: expected '$2', got '$3'"; fails=$((fails+1)); fi; }
has() { printf '%s' "$1" | grep -qF -- "$2" && echo yes || echo no; }

BIN="$FIX/bin"; mkdir -p "$BIN" "$FIX/home/.amux/github-app"
printf '#!/bin/sh\necho "export GH_TOKEN=stub"\n' > "$FIX/home/.amux/github-app/get-token.sh"
cat > "$BIN/gh" <<'EOF'
#!/bin/sh
case "$*" in
  *workflows/rust.yml/runs*) printf '101\tfailure\tabc123def456\thttps://example/run/101\n' ;;
  *workflows/checks.yml/runs*) printf '' ;;
  *) printf '' ;;
esac
EOF
cat > "$BIN/curl" <<'EOF'
#!/bin/sh
out=""; while [ $# -gt 0 ]; do [ "$1" = -o ] && { out=$2; shift; }; shift; done
printf '{"blocked":true,"error":"WIP limit: holding 348, limit 20"}' > "$out"; printf 400
EOF
cat > "$BIN/amux" <<'EOF'
#!/bin/sh
[ "$1" = url ] && { echo https://stub; exit 0; }
n=$(cat "$AMUX_SEND_COUNT" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$AMUX_SEND_COUNT"
cat > /dev/null
[ "$n" -ge "${AMUX_SEND_OK_AT:-2}" ] && { echo "sent"; exit 0; }
echo "send refused: lane busy (stub)" >&2; exit 1
EOF
cat > "$BIN/sleep" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$BIN"/* "$FIX/home/.amux/github-app/get-token.sh"
run() { HOME="$FIX/home" PATH="$BIN:$PATH" MAIN_CI_WATCH_STATE="$FIX/state.json" AMUX_SEND_COUNT="$FIX/sends" \
  AMUX_SEND_OK_AT="$1" bash "$HERE/main-ci-watch.sh" 2>&1; }

echo "1. a refused card names its reason; a failed send is retried and lands"
rm -f "$FIX/state.json" "$FIX/sends"
out=$(run 2 || true)
check "the refusal body is printed"            yes "$(has "$out" 'HTTP 400: {"blocked":true,"error":"WIP limit: holding 348')"
check "the first send's own error is kept"     yes "$(has "$out" 'could not message the amux lane (try 1): send refused: lane busy')"
check "the retry lands, so nothing is lost"    yes "$(has "$out" '"alerts_not_delivered":0')"
check "two sends were made"                    2   "$(cat "$FIX/sends")"

echo "2. two failed sends report the alert as not delivered"
rm -f "$FIX/state.json" "$FIX/sends"
out=$(run 9 || true)
check "both tries are named"                   yes "$(has "$out" '(try 2): send refused')"
check "it is counted undelivered"              yes "$(has "$out" '"alerts_not_delivered":1')"

if [ "$fails" = 0 ]; then echo "PASS: main-ci-watch — all checks passed"; else echo "main-ci-watch: $fails check(s) FAILED"; exit 1; fi
