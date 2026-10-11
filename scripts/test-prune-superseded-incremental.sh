#!/usr/bin/env bash
# Proves prune-superseded-incremental.sh removes only superseded fingerprints.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
I="$T/incremental"
mkdir -p "$I/amux_server-aaaaaaaaaaaa/s-1" "$I/amux_server-bbbbbbbbbbbb/s-1" "$I/amux_core-cccccccccccc/s-1" "$I/amux_core-dddddddddddd/s-1"
for f in amux_server-aaaaaaaaaaaa amux_server-bbbbbbbbbbbb amux_core-cccccccccccc amux_core-dddddddddddd; do echo x > "$I/$f/s-1/dep-graph.bin"; touch -t 202001010000 "$I/$f/s-1/dep-graph.bin"; done
SINCE=$(date +%s); sleep 1
echo y > "$I/amux_server-bbbbbbbbbbbb/s-1/dep-graph.bin"   # this build wrote amux_server only
out=$(bash "$HERE/prune-superseded-incremental.sh" "$I" "$SINCE")
fail=0
[ ! -e "$I/amux_server-aaaaaaaaaaaa" ] || { echo "FAIL: superseded amux_server tree kept"; fail=1; }
[ -e "$I/amux_server-bbbbbbbbbbbb" ] || { echo "FAIL: the tree this build wrote was removed"; fail=1; }
[ -e "$I/amux_core-cccccccccccc" ] && [ -e "$I/amux_core-dddddddddddd" ] || { echo "FAIL: a crate this build did not compile was pruned"; fail=1; }
printf '%s' "$out" | grep -q "verdict=incremental_pruned" || { echo "FAIL: no verdict: $out"; fail=1; }
out2=$(bash "$HERE/prune-superseded-incremental.sh" "$I" "$SINCE")
printf '%s' "$out2" | grep -q "verdict=incremental_none" || { echo "FAIL: second run: $out2"; fail=1; }
[ "$fail" = 0 ] && echo "PASS: prune-superseded-incremental"
exit "$fail"
