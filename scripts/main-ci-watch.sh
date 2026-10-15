#!/usr/bin/env bash
# Main's CI is the full gate now that PRs run only affected tests (#317), and
# the builder deploys every main commit without waiting for it. So a red main
# must be noticed by someone who will fix it. Run as a shell schedule every few
# minutes: for each workflow, look at main's newest completed run and file ONE
# board card on the amux lane per newly failed run, naming the failing tests and
# the commits since the last green run. DRY=1 prints instead of filing.
set -euo pipefail
REPO=mixpeek/amux
STATE="${MAIN_CI_WATCH_STATE:-${AMUX_HOME:-$HOME/.amux}/state/main-ci-watch.json}"
mkdir -p "$(dirname "$STATE")"; [ -f "$STATE" ] || echo '{}' > "$STATE"
eval "$(~/.amux/github-app/get-token.sh --min-remaining 300)"
U="$(amux url)"
filed=0 considered=0
for wf in rust.yml checks.yml; do
  row=$(gh api "repos/$REPO/actions/workflows/$wf/runs?branch=main&status=completed&per_page=1" \
        --jq '.workflow_runs[0] | [.id, .conclusion, .head_sha, .html_url] | @tsv') || continue
  [ -n "$row" ] || continue
  considered=$((considered + 1))
  IFS=$'\t' read -r id conclusion sha url <<<"$row"
  seen=$(python3 -c "import json,sys; s=json.load(open('$STATE')); print(s.get('$wf',{}).get('seen',''))")
  [ "$seen" = "$id" ] && continue
  if [ "$conclusion" = success ]; then
    python3 - "$STATE" "$wf" "$id" "$sha" <<'PY'
import json,sys
p,wf,i,sha=sys.argv[1:]; s=json.load(open(p)); s[wf]={'seen':i,'green':sha}; json.dump(s,open(p,'w'))
PY
    continue
  fi
  [ "$conclusion" = failure ] || { python3 - "$STATE" "$wf" "$id" <<'PY'
import json,sys
p,wf,i=sys.argv[1:]; s=json.load(open(p)); s.setdefault(wf,{})['seen']=i; json.dump(s,open(p,'w'))
PY
    continue; }
  green=$(python3 -c "import json; print(json.load(open('$STATE')).get('$wf',{}).get('green',''))")
  tests=$(gh api "repos/$REPO/actions/runs/$id/jobs" --jq '.jobs[]|select(.conclusion=="failure")|.id' | head -6 | while read -r j; do
      gh api "repos/$REPO/actions/jobs/$j/logs" 2>/dev/null | grep -av WebServer \
        | grep -aE '^\S+\s+[0-9]+\) \[|^\S+ +test .* FAILED|^\S+ +not ok|  FAIL  ' | sed 's/^[0-9TZ:.-]* //' | head -3 || true
    done | sed 's/^ *//' | sort -u | head -10 || true)
  commits=""
  if [ -n "$green" ]; then
    commits=$(gh api "repos/$REPO/compare/$green...$sha" --jq '.commits[] | "\(.sha[0:9]) \(.commit.message | split("\n")[0])"' 2>/dev/null | tail -15 || true)
  fi
  first=$(printf '%s\n' "$tests" | head -1 | cut -c1-90)
  title="main CI red: ${wf%.yml} on ${sha:0:9}${first:+: $first}"
  desc=$(printf 'Run: %s\n\nFailing:\n%s\n\nCommits since the last green %s run (%s):\n%s\n\nPRs now run only affected tests (#317), so main is the full gate and the builder already deployed this commit. Fix forward or revert the culprit.' \
    "$url" "${tests:-see the run}" "${wf%.yml}" "${green:0:9}" "${commits:-unknown: no green run recorded yet}")
  if [ "${DRY:-0}" = 1 ]; then
    printf 'DRY card: %s\n%s\n' "$title" "$desc"
  else
    body=$(python3 -c "import json,sys; print(json.dumps({'title':sys.argv[1],'desc':sys.argv[2],'status':'todo','session':'amux'}))" "$title" "$desc")
    code=$(curl -sk --max-time 20 -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -H 'X-Amux-Session: amux' -d "$body" "$U/api/board")
    case "$code" in 2??) filed=$((filed + 1)) ;; *) echo "board refused: $code" >&2 ;; esac
  fi
  python3 - "$STATE" "$wf" "$id" <<'PY'
import json,sys
p,wf,i=sys.argv[1:]; s=json.load(open(p)); s.setdefault(wf,{})['seen']=i; json.dump(s,open(p,'w'))
PY
done
echo "{\"verdict\":\"main_ci_watch\",\"measured\":true,\"n_considered\":$considered,\"cards_filed\":$filed}"
