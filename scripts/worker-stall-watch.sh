#!/usr/bin/env bash
# worker-stall-watch.sh: notice when an orchestrating worker has stopped acting
# while work is still arriving for it, and do something about it.
#
# Why (2026-10-24/25): amux-helper asked the owner a question with Claude Code's
# AskUserQuestion tool, which blocks the whole turn until answered. The owner was
# asleep, so from ~04:2xZ to 12:15Z peer messages queued unread, its ScheduleWakeup
# chain could not fire (the turn never ended), and nothing steered while the
# TubeScience chain failed three times and aborted. Nothing in amux noticed.
#
# Detection is by symptom, so it covers every cause (a blocking prompt, a turn that
# ended without re-arming, a usage limit): the worker's last_progress_at is older
# than STALL_MIN minutes AND it has inbound messages newer than that progress.
#   - turn idle    -> send it a wake message (it will read its queue).
#   - turn running -> it is blocked inside a turn; a message cannot reach it, so
#                     alert the owner, once per stall.
# Every check writes one line to ~/.amux/logs/worker-stall-watch.log; a stall also
# writes a WARN line, so a log sweep finds it.
#
# Usage: worker-stall-watch.sh <worker> [<worker>...]
# Env:   STALL_MIN (default 20), STALL_FIXTURE_SESSION / STALL_FIXTURE_HISTORY
#        (JSON files used instead of the API, for tests), STALL_DRY_RUN=1 (decide only).
set -uo pipefail
STALL_MIN="${STALL_MIN:-20}"
LOG="${STALL_LOG:-$HOME/.amux/logs/worker-stall-watch.log}"
STATE_DIR="${STALL_STATE_DIR:-$HOME/.amux/tmp/worker-stall-watch}"
mkdir -p "$(dirname "$LOG")" "$STATE_DIR"
U="$(amux url 2>/dev/null)"
now=$(date -u +%s)
ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
rc=0

for w in "$@"; do
  if [ -n "${STALL_FIXTURE_SESSION:-}" ]; then
    sess=$(cat "$STALL_FIXTURE_SESSION"); hist=$(cat "$STALL_FIXTURE_HISTORY")
  else
    sess=$(curl -sk --max-time 15 "$U/api/sessions/$w") || sess=""
    hist=$(curl -sk --max-time 15 "$U/api/history?session=$w&limit=50") || hist=""
  fi
  verdict=$(python3 -c '
import json,sys
w,now,stall=sys.argv[1],float(sys.argv[2]),float(sys.argv[3])*60
try:
    s=json.loads(sys.argv[4]); h=json.loads(sys.argv[5])
except Exception as e:
    print("unmeasured unreadable_api", e.__class__.__name__); sys.exit()
if not isinstance(s,dict) or "name" not in s:
    print("unmeasured no_session"); sys.exit()
prog=float(s.get("last_progress_at") or s.get("last_activity") or 0)
turn=s.get("turn_state") or ""
inbound=[int(r["ts"])/1000 for r in (h if isinstance(h,list) else []) if r.get("ts")]
unread=sum(1 for t in inbound if t>prog)
age=now-prog
if prog==0:
    print("unmeasured no_progress_field"); sys.exit()
if age>stall and unread>0:
    print("STALLED", turn or "unknown", int(age//60), unread)
else:
    print("ok", turn or "unknown", int(age//60), unread)
' "$w" "$now" "$STALL_MIN" "$sess" "$hist")
  read -r state turn age_min unread <<<"$verdict"
  echo "$ts worker=$w verdict=$state turn=$turn idle_min=${age_min:-?} unread=${unread:-?} stall_min=$STALL_MIN" >> "$LOG"
  stamp="$STATE_DIR/$w.alerted"
  if [ "$state" = STALLED ]; then
    rc=1
    echo "$ts WARN worker=$w STALLED turn=$turn no progress for ${age_min} min with $unread unread inbound message(s)" >> "$LOG"
    if [ "$turn" = running ]; then
      # Blocked inside a turn (a prompt, a question): only the owner can unblock it.
      if [ ! -f "$stamp" ]; then
        [ "${STALL_DRY_RUN:-}" = 1 ] || amux alert "Worker $w is stuck inside a turn: no progress for ${age_min} min while $unread message(s) wait. It is probably blocked on a prompt or question in its pane; answer or dismiss it." "orchestrator stalled" >/dev/null 2>&1
        echo "$ts worker=$w action=owner_alert" >> "$LOG"; date -u +%s > "$stamp"
      fi
    else
      [ "${STALL_DRY_RUN:-}" = 1 ] || printf '%s\n' "Stall watchdog: no progress for ${age_min} min and $unread unread message(s). Read your queue and act on whatever is blocked; re-arm your wake." | amux send "$w" --stdin >/dev/null 2>&1
      echo "$ts worker=$w action=wake_sent" >> "$LOG"
    fi
  else
    rm -f "$stamp"   # recovered or healthy: the next stall alerts again
  fi
done
exit $rc
