#!/usr/bin/env bash
# Tests for scripts/disk-reclaim-tick.sh against a fake /api/reclaim.
# The fake records every quarantine POST and purge DELETE, so each cell asserts
# what the tick ASKED the server to move, which is the only thing it decides.
set -uo pipefail
cd "$(dirname "$0")/.."
TICK=scripts/disk-reclaim-tick.sh
T=$(mktemp -d "${TMPDIR:-/tmp}/test-reclaim.XXXXXX")
trap 'kill "${SRV:-0}" 2>/dev/null; rm -rf -- "${T:?}"' EXIT
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS $1"; }
bad() { fail=$((fail+1)); echo "FAIL $1"; }

now=$(date +%s); old=$((now - 5*86400)); recent=$((now - 3600))
mkdir -p "$T/dev/a/target" "$T/dev/b/target" "$T/dev/c/target" "$T/dev/d/node_modules" \
         "$T/dev/e/target" "$T/dev/f/.venv" "$T/global/node_modules"
oldstamp=$(python3 -c 'import sys,time; print(time.strftime("%Y%m%d%H%M.%S", time.localtime(int(sys.argv[1]))))' "$old")  # BSD and GNU date disagree on -r
# A dir's own mtime moves when an entry is created in it, so age the file AND
# its dir: the recheck reads both, which is what makes a fresh target stay.
for d in "$T"/dev/{a,b,c,e}/target "$T/dev/d/node_modules" "$T/dev/f/.venv" "$T/global/node_modules"; do echo x > "$d/f"; touch -t "$oldstamp" "$d/f" "$d"; done
echo fresh > "$T/dev/e/target/new.o"   # scan says idle, disk says written now

finding() { # path category regenerable mtime protected
  printf '{"path":"%s","category":"%s","regenerable":%s,"bytes":1073741824,"mtime":%s,"protected":%s}' "$@"; }
mkscan() { # <fleet_measured> -> writes the fake's scan file
  cat > "$T/scan.json" <<EOF
{"scan":{"id":"S1","status":"done"},"ownership":{"fleet_measured":$1},"findings":[
$(finding "$T/dev/a/target" build true $old null),
$(finding "$T/dev/b/target" build true $recent null),
$(finding "$T/dev/c/target" build true $old '{"class":"worker","reason":"running"}'),
$(finding "$T/global/node_modules" build true $old null),
$(finding "$T/dev/d/node_modules" large false $old null),
$(finding "$T/dev/e/target" build true $old null),
$(finding "$T/dev/f/.venv" build true $old null)]}
EOF
}
cat > "$T/batches.json" <<EOF
{"batches":[
 {"id":"OWNOLD","session":"disk-reclaim-tick","status":"staged","created_at":$((now - 4*86400)),"bytes":1},
 {"id":"OWNNEW","session":"disk-reclaim-tick","status":"staged","created_at":$recent,"bytes":1},
 {"id":"PEEROLD","session":"amux-admin","status":"staged","created_at":$((now - 9*86400)),"bytes":1},
 {"id":"OWNGONE","session":"disk-reclaim-tick","status":"purged","created_at":$((now - 9*86400)),"bytes":1}]}
EOF

cat > "$T/fake.py" <<'PY'
import http.server, json, sys, time, urllib.parse
T = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, obj, code=200):
        b = json.dumps(obj).encode(); self.send_response(code)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(b)))
        self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        if u.path == "/api/reclaim/scan":
            return self.send([{"id": "S1", "status": "done", "finished_at": time.time() - 60}])
        if u.path == "/api/reclaim/scan/S1":
            return self.send(json.load(open(T + "/scan.json")))
        if u.path == "/api/reclaim/quarantine":
            return self.send(json.load(open(T + "/batches.json")))
        self.send({"error": "nf"}, 404)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0); body = self.rfile.read(n).decode()
        open(T + "/posts.log", "a").write(self.headers.get("X-Amux-Session", "") + "\t" + body + "\n")
        paths = json.loads(body).get("paths", [])
        self.send({"batch_id": "B1", "moved": paths[:-1], "refused": [{"path": paths[-1], "class": "owned", "reason": "test"}] if paths else []})
    def do_DELETE(self):
        open(T + "/deletes.log", "a").write(self.path + "\n"); self.send({"ok": True})
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(T + "/port", "w").write(str(s.server_port)); s.serve_forever()
PY
python3 "$T/fake.py" "$T" & SRV=$!
for _ in $(seq 50); do [ -s "$T/port" ] && break; sleep 0.1; done
API="http://127.0.0.1:$(cat "$T/port")"

run() { : > "$T/posts.log"; : > "$T/deletes.log"
  RECLAIM_API=$API RECLAIM_ROOTS="$T/dev" RECLAIM_TOOLS=0 RECLAIM_LOG="$T/tick.log" \
    bash "$TICK" "$@" > "$T/out.txt" 2>&1; echo $? > "$T/rc"; }
posted() { grep -c -F "\"$1\"" "$T/posts.log"; }

# ── act ──
mkscan true; run
[ "$(cat "$T/rc")" = 0 ] && ok "exits 0" || { bad "exits 0 (rc=$(cat "$T/rc"))"; cat "$T/out.txt"; }
[ "$(posted "$T/dev/a/target")" = 1 ] && ok "idle unowned build output in a root is quarantined" || bad "idle build output quarantined"
[ "$(posted "$T/dev/f/.venv")" = 1 ] && ok "a .venv in a root is quarantined" || bad ".venv quarantined"
[ "$(posted "$T/dev/b/target")" = 0 ] && ok "scan-recent finding is kept" || bad "scan-recent kept"
[ "$(posted "$T/dev/c/target")" = 0 ] && ok "protected finding is kept" || bad "protected kept"
[ "$(posted "$T/global/node_modules")" = 0 ] && ok "build-named dir outside the roots is kept" || bad "outside roots kept"
[ "$(posted "$T/dev/d/node_modules")" = 0 ] && ok "non-regenerable category is kept" || bad "non-regenerable kept"
[ "$(posted "$T/dev/e/target")" = 0 ] && ok "on-disk recheck keeps a dir written inside the idle window" || bad "on-disk recheck"
grep -q "written inside 48h" "$T/out.txt" && ok "recheck says why it kept" || bad "recheck reason printed"
cut -f1 "$T/posts.log" | grep -qx disk-reclaim-tick && ok "POST carries the schedule's session" || bad "session header"
grep -q 'moved 1, refused 1' "$T/out.txt" && ok "server refusals are counted, not hidden" || bad "refusal count"
grep -q 'REFUSED.*owned' "$T/out.txt" && ok "a refusal prints its class" || bad "refusal class"
grep -q 'OWNOLD?confirm=OWNOLD' "$T/deletes.log" && ok "own expired batch purged with confirm" || bad "own expired purge"
grep -q 'OWNNEW' "$T/deletes.log" && bad "own recent batch must stay restorable" || ok "own recent batch stays restorable"
grep -q 'PEEROLD' "$T/deletes.log" && bad "peer batch must never be purged" || ok "peer's batch is never purged"
grep -q 'OWNGONE' "$T/deletes.log" && bad "already purged batch re-purged" || ok "already-purged batch is skipped"
grep -q 'done dry=0 picked=2 moved=1 refused=1 purged=1' "$T/out.txt" && ok "summary line is computed from the run" || { bad "summary line"; tail -2 "$T/out.txt"; }

# ── dry run ──
run --dry-run
[ ! -s "$T/posts.log" ] && [ ! -s "$T/deletes.log" ] && ok "dry run posts and deletes nothing" || bad "dry run is inert"
grep -q 'would move 2 path' "$T/out.txt" && grep -q 'would purge batch OWNOLD' "$T/out.txt" && ok "dry run says what it would do" || bad "dry run report"

# ── ownership not measured ──
mkscan false; run
[ ! -s "$T/posts.log" ] && grep -q 'verdict=reclaim_tick_unmeasured' "$T/out.txt" && ok "unmeasured ownership moves nothing and says so" || bad "unmeasured ownership"

# ── no rm on a candidate, ever ──
grep -nE '(^|[^a-z_])rm ' "$TICK" | grep -v -- '"${WORK:?}"' | grep -v '^[0-9]*:#' > "$T/rm.txt"
[ ! -s "$T/rm.txt" ] && ok "the tick has no rm except its own scratch dir" || { bad "rm found"; cat "$T/rm.txt"; }

echo "disk-reclaim-tick: $pass passed, $fail failed"
[ "$fail" = 0 ]
