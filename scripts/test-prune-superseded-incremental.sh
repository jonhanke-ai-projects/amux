#!/usr/bin/env bash
# Proves prune-superseded-incremental.sh removes only superseded fingerprints.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
I="$T/incremental"
ALL="amux_server-aaaaaaaaaaaa amux_server-bbbbbbbbbbbb amux_server-eeeeeeeeeeee amux_core-cccccccccccc amux_core-dddddddddddd amux_core-ffffffffffff build_script_build-111111111111 build_script_build-222222222222 build_script_build-333333333333"
for f in $ALL; do mkdir -p "$I/$f/s-1"; done
for f in $ALL; do echo x > "$I/$f/s-1/dep-graph.bin"; touch -t 202001010000 "$I/$f/s-1/dep-graph.bin"; done
SINCE=$(date +%s); sleep 1
touch -t 202101010000 "$I/amux_server-eeeeeeeeeeee/s-1/dep-graph.bin"   # the lib: idle, but 2nd newest
echo y > "$I/amux_server-bbbbbbbbbbbb/s-1/dep-graph.bin"   # this build wrote amux_server only
echo y > "$I/build_script_build-111111111111/s-1/dep-graph.bin"
out=$(bash "$HERE/prune-superseded-incremental.sh" "$I" "$SINCE")
fail=0
[ ! -e "$I/amux_server-aaaaaaaaaaaa" ] || { echo "FAIL: superseded amux_server tree kept"; fail=1; }
[ -e "$I/amux_server-bbbbbbbbbbbb" ] || { echo "FAIL: the tree this build wrote was removed"; fail=1; }
[ -e "$I/amux_server-eeeeeeeeeeee" ] || { echo "FAIL: the second-newest tree (lib beside bin) was pruned"; fail=1; }
[ -e "$I/amux_core-cccccccccccc" ] && [ -e "$I/amux_core-dddddddddddd" ] && [ -e "$I/amux_core-ffffffffffff" ] || { echo "FAIL: a crate this build did not compile was pruned"; fail=1; }
[ -e "$I/build_script_build-222222222222" ] && [ -e "$I/build_script_build-333333333333" ] || { echo "FAIL: build_script_build dirs of other packages were pruned"; fail=1; }
printf '%s' "$out" | grep -q "verdict=incremental_pruned" || { echo "FAIL: no verdict: $out"; fail=1; }
# A third tree used minutes ago (before this build, so neither fresh nor in
# the newest two) survives on the idle window alone.
mkdir -p "$I/amux_server-gggggggggggg/s-1"; echo n > "$I/amux_server-gggggggggggg/s-1/dep-graph.bin"
sleep 1; SINCE3=$(date +%s); sleep 1
echo y > "$I/amux_server-bbbbbbbbbbbb/s-1/dep-graph.bin"; echo y > "$I/amux_server-eeeeeeeeeeee/s-1/dep-graph.bin"
bash "$HERE/prune-superseded-incremental.sh" "$I" "$SINCE3" >/dev/null
[ -e "$I/amux_server-gggggggggggg" ] || { echo "FAIL: a recently used tree was pruned before the idle window"; fail=1; }
rm -rf "$I/amux_server-gggggggggggg"
out2=$(bash "$HERE/prune-superseded-incremental.sh" "$I" "$SINCE")
printf '%s' "$out2" | grep -q "verdict=incremental_none" || { echo "FAIL: second run: $out2"; fail=1; }
[ "$fail" = 0 ] && echo "PASS: prune-superseded-incremental"
exit "$fail"
