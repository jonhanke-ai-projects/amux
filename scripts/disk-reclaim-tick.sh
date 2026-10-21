#!/usr/bin/env bash
# disk-reclaim-tick.sh — the scheduled disk cleanup, driven through /api/reclaim.
#
# Run daily by the amux scheduler as a `shell` schedule (zero model cost):
#   scripts/disk-reclaim-tick.sh             # act
#   scripts/disk-reclaim-tick.sh --dry-run   # say what it would do, act on nothing
#
# WHY IT GOES THROUGH THE SERVER. On 2026-10-20 two hand-rolled cleanup scripts
# rm -rf'd a running worker's ~/.amux/tmp, git-tracked customer data and live
# worktrees (crates/amux-server/src/api/reclaim_owner.rs has the incidents). The
# server now refuses those paths at the moment of the move: guard_path (system,
# amux state, git metadata, live session scratch), the ownership gate (running
# workers, tracked files, dirty or unpushed worktrees) and the builder's cargo
# lease. A script that deletes with rm skips all three. This one never calls rm:
#
#   1. Use the newest finished Disk scan (or start one and wait for it).
#   2. Pick regenerable findings (build output, orphaned tmp snapshots) that are
#      unowned, big enough to matter, and untouched for RECLAIM_IDLE_H on disk.
#   3. POST them to /api/reclaim/quarantine. The server moves them aside and
#      refuses anything owned; refusals are printed with their class.
#   4. Purge THIS schedule's own batches once they are RECLAIM_RETAIN_H old,
#      so a wrong move can still be restored in the dashboard until then.
#      Batches anyone else staged are never purged here.
#   5. Run the owning tool's own prune for caches (uv, npm, pnpm, brew,
#      simulators, docker build cache and dangling images). Each tool knows
#      what is still referenced; rm does not.
#
# Reclaim is reported as a df delta, never a sum of sizes (APFS snapshots can
# hold freed blocks; reclaim.rs explains why the two disagree).
set -uo pipefail

DRY=0
for a in "$@"; do
  case "$a" in
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "disk-reclaim: unknown argument $a" >&2; exit 2 ;;
  esac
done

API=${RECLAIM_API:-$(amux url 2>/dev/null)}
SESSION=${RECLAIM_SESSION:-disk-reclaim-tick}
IDLE_H=${RECLAIM_IDLE_H:-48}             # untouched this long on disk before it moves
RETAIN_H=${RECLAIM_RETAIN_H:-72}         # our batches stay restorable this long
MIN_MB=${RECLAIM_MIN_MB:-256}            # smaller findings are not worth a rebuild
CATS=${RECLAIM_CATS:-build tmp-orphan}   # finding categories eligible for auto-move
# Only these trees are moved unattended. A build-dir NAME is not enough: the first
# dry run picked ~/.nvm/.../lib/node_modules (every global CLI) and an app's own
# runtime under ~/Library/Application Support. The dashboard still lists those
# for a person to judge.
ROOTS=${RECLAIM_ROOTS:-$HOME/Dev $HOME/.amux /private/tmp /private/var/folders}
SCAN_MAX_AGE_H=${RECLAIM_SCAN_MAX_AGE_H:-20}
SCAN_WAIT_S=${RECLAIM_SCAN_WAIT_S:-2700}
RECHECK_S=${RECLAIM_RECHECK_S:-60}       # per-path budget for the on-disk idle recheck
TOOLS=${RECLAIM_TOOLS:-1}                # 0 skips step 5 (the test suite sets it)
LOG=${RECLAIM_LOG:-$HOME/.amux/logs/disk-reclaim-tick.log}

mkdir -p "$(dirname "$LOG")" 2>/dev/null
exec > >(tee -a "$LOG") 2>&1
say() { printf '%s disk-reclaim: %s\n' "$(date '+%F %T')" "$*"; }
api() { local path=$1 t=${2:-30}; shift; [ $# -gt 0 ] && shift; curl -sk --max-time "$t" -H "X-Amux-Session: $SESSION" "$@" "$API$path"; }
free_kb() { df -k "$HOME" | awk 'NR==2{print $4}'; }
gb() { awk -v k="${1:-0}" 'BEGIN{printf "%.1fG", k/1048576}'; }

if [ -z "$API" ]; then say "verdict=reclaim_tick_unmeasured why=no amux url"; exit 1; fi
WORK=$(mktemp -d "${TMPDIR:-/tmp}/disk-reclaim.XXXXXX")
trap 'rm -rf -- "${WORK:?}"' EXIT
free0=$(free_kb)
say "start dry=$DRY free=$(gb "$free0") idle>=${IDLE_H}h retain=${RETAIN_H}h min=${MIN_MB}M cats=[$CATS] roots=[$ROOTS]"

# ── 1. a scan ──────────────────────────────────────────────────────────────
scan_id=$(api /api/reclaim/scan | python3 -c '
import json, sys, time
max_age = float(sys.argv[1]) * 3600
try: rows = json.load(sys.stdin)
except Exception: sys.exit(0)
rows = rows if isinstance(rows, list) else rows.get("scans", [])
done = [r for r in rows if r.get("status") == "done" and time.time() - (r.get("finished_at") or 0) <= max_age]
if done: print(max(done, key=lambda r: r["finished_at"])["id"])' "$SCAN_MAX_AGE_H")
if [ -n "$scan_id" ]; then
  say "scan: reusing $scan_id (finished within ${SCAN_MAX_AGE_H}h)"
elif [ "$DRY" = 1 ]; then
  say "scan: none finished within ${SCAN_MAX_AGE_H}h, and a dry run starts none"
else
  scan_id=$(api /api/reclaim/scan 30 -X POST -H 'Content-Type: application/json' -d '{}' \
    | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("scan_id") or "")
except Exception: pass')
  if [ -n "$scan_id" ]; then
    say "scan: started $scan_id, waiting up to ${SCAN_WAIT_S}s"
    t0=$SECONDS; st=running
    while [ "$st" = running ] && [ $((SECONDS - t0)) -lt "$SCAN_WAIT_S" ]; do
      sleep 30
      st=$(api "/api/reclaim/scan/$scan_id?limit=1" 60 | python3 -c 'import json,sys
try: print(json.load(sys.stdin)["scan"]["status"])
except Exception: print("running")')
    done
    [ "$st" = done ] || { say "scan: $scan_id ended as '$st' after $((SECONDS - t0))s, not used"; scan_id=; }
  else
    say "scan: could not start one (another scan running, or the API refused)"
  fi
fi

# ── 2. selection ──────────────────────────────────────────────────────────
moved=0; refused=0; picked=0
if [ -z "$scan_id" ]; then
  say "verdict=reclaim_tick_no_scan quarantine step skipped: no usable scan"
else
  api "/api/reclaim/scan/$scan_id?limit=5000" 120 > "$WORK/scan.json"
  python3 - "$WORK/scan.json" "$IDLE_H" "$MIN_MB" "$CATS" "$ROOTS" > "$WORK/candidates.tsv" <<'PY'
import json, os, sys, time
d = json.load(open(sys.argv[1]))
idle_s, min_b, cats = float(sys.argv[2]) * 3600, int(sys.argv[3]) << 20, set(sys.argv[4].split())
roots = [r.rstrip("/") + "/" for r in sys.argv[5].split()]
own = d.get("ownership") or {}
if not own.get("fleet_measured", False):
    print("UNMEASURED\tfleet ownership probe did not run")
    sys.exit(0)
now = time.time()
for f in d.get("findings") or []:
    p = f.get("path") or ""
    if f.get("protected") or not f.get("regenerable") or f.get("category") not in cats:
        continue
    if not any(p.startswith(r) for r in roots):
        continue
    if (f.get("bytes") or 0) < min_b or now - (f.get("mtime") or now) < idle_s:
        continue
    if os.path.lexists(p):
        print(f"{f['bytes']}\t{p}")
PY
  if head -1 "$WORK/candidates.tsv" | grep -q '^UNMEASURED'; then
    say "verdict=reclaim_tick_unmeasured scan ownership was not measured; nothing moved"
    : > "$WORK/candidates.tsv"
  fi
  # The scan is up to SCAN_MAX_AGE_H old. Re-check on disk now: any file written
  # inside the idle window keeps the path. A recheck that runs out of time keeps
  # it too, since the expensive direction to be wrong in is the move.
  touch -t "$(date -v-"${IDLE_H}"H '+%Y%m%d%H%M.%S' 2>/dev/null || date -d "-${IDLE_H} hours" '+%Y%m%d%H%M.%S')" "$WORK/idle-stamp"
  : > "$WORK/paths.txt"
  n_recent=0; n_slow=0
  while IFS=$'\t' read -r bytes p; do
    [ -n "$p" ] || continue
    hit=$(timeout "$RECHECK_S" find "$p" -newer "$WORK/idle-stamp" -print -quit 2>/dev/null); rc=$?
    if [ "$rc" = 124 ]; then n_slow=$((n_slow+1)); say "  keep $p (idle recheck ran out of ${RECHECK_S}s)"; continue; fi
    if [ -n "$hit" ]; then n_recent=$((n_recent+1)); say "  keep $p (written inside ${IDLE_H}h: $hit)"; continue; fi
    printf '%s\n' "$p" >> "$WORK/paths.txt"; picked=$((picked+1))
    say "  pick $(gb $((bytes/1024))) $p"
  done < "$WORK/candidates.tsv"
  say "selection: $picked picked from scan $scan_id; kept $n_recent recently written, $n_slow unrechecked"

  # ── 3. quarantine ───────────────────────────────────────────────────────
  if [ "$picked" -gt 0 ] && [ "$DRY" = 0 ]; then
    python3 -c 'import json,sys; print(json.dumps({"paths":[l.strip() for l in open(sys.argv[1]) if l.strip()],"scan_id":sys.argv[2]}))' \
      "$WORK/paths.txt" "$scan_id" > "$WORK/req.json"
    api /api/reclaim/quarantine 1800 -X POST -H 'Content-Type: application/json' --data-binary @"$WORK/req.json" > "$WORK/q.json"
    cat > "$WORK/reply.py" <<'PY'
import json, sys
# The server answers {"batch_id", "moved": <count>, "refused": [{path, reason, class}]}.
try: d = json.load(open(sys.argv[1]))
except Exception as e:
    print(f"verdict=reclaim_tick_bad_reply quarantine reply was not JSON: {e}", file=sys.stderr)
    print("? ? -"); sys.exit(0)
ref = d.get("refused") or []
for r in ref:
    print(f"REFUSED\t{r.get('class','?')}\t{r.get('path')}\t{r.get('reason')}", file=sys.stderr)
m = d.get("moved")
print(m if isinstance(m, int) else len(m or []), len(ref), d.get("batch_id") or "-")
PY
    read -r moved refused batch < <(python3 "$WORK/reply.py" "$WORK/q.json")
    say "quarantine: batch $batch moved $moved, refused $refused (refusals above name their class)"
  elif [ "$picked" -gt 0 ]; then
    say "quarantine: dry run, would move $picked path(s)"
  fi
fi

# ── 4. purge our own expired batches ───────────────────────────────────────
purged=0
api /api/reclaim/quarantine 60 | python3 -c '
import json, sys, time
retain, me = float(sys.argv[1]) * 3600, sys.argv[2]
try: d = json.load(sys.stdin)
except Exception: sys.exit(0)
for b in d.get("batches") or []:
    if b.get("session") == me and b.get("status") == "staged" and time.time() - (b.get("created_at") or time.time()) >= retain:
        print(b["id"], b.get("bytes") or 0)' "$RETAIN_H" "$SESSION" > "$WORK/expired.txt"
while read -r bid bbytes; do
  [ -n "$bid" ] || continue
  if [ "$DRY" = 1 ]; then say "purge: dry run, would purge batch $bid ($(gb $((bbytes/1024))))"; continue; fi
  code=$(curl -sk --max-time 1800 -o "$WORK/purge.json" -w '%{http_code}' -X DELETE \
    -H "X-Amux-Session: $SESSION" "$API/api/reclaim/quarantine/$bid?confirm=$bid")
  case "$code" in
    2??) purged=$((purged+1)); say "purge: batch $bid ($(gb $((bbytes/1024))))" ;;
    *) say "purge: batch $bid refused http=$code $(head -c 200 "$WORK/purge.json")" ;;
  esac
done < "$WORK/expired.txt"

# ── 5. tool-owned caches ───────────────────────────────────────────────────
tool() { # <label> <cmd...>: run if installed, bounded, report the df delta
  local label=$1; shift
  command -v "$1" >/dev/null 2>&1 || { say "tool: $label not installed"; return 0; }
  if [ "$DRY" = 1 ]; then say "tool: dry run, would run: $*"; return 0; fi
  local b a rc=0
  b=$(free_kb); timeout 900 "$@" >/dev/null 2>&1 || rc=$?; a=$(free_kb)
  say "tool: $label rc=$rc freed $(gb $((a - b)))"
}
if [ "$TOOLS" = 1 ]; then
  tool "uv cache prune" uv cache prune
  tool "npm cache verify" npm cache verify
  tool "pnpm store prune" pnpm store prune
  tool "brew cleanup" brew cleanup -s --prune=30
  tool "unavailable simulators" xcrun simctl delete unavailable
  if docker info >/dev/null 2>&1; then
    tool "docker build cache >72h" docker builder prune -f --filter until=72h
    tool "docker dangling images" docker image prune -f
  else
    say "tool: docker not running"
  fi
fi

free1=$(free_kb)
say "done dry=$DRY picked=$picked moved=$moved refused=$refused purged=$purged free $(gb "$free0") -> $(gb "$free1") (df delta $(gb $((free1 - free0))))"
