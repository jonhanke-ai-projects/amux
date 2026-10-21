#!/usr/bin/env bash
# Tests for scripts/disk-reclaim-tick.sh against a fake /api/reclaim.
# The fake records every quarantine POST and purge DELETE, so each cell asserts
# what the tick ASKED the server to move, which is the only thing it decides.
set -euo pipefail
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

# Stale-data fixtures: a fake $HOME with big files in every shape the class must judge.
H="$T/home"; ancient=$((now - 200*86400)); young=$((now - 10*86400))
ancstamp=$(python3 -c 'import sys,time; print(time.strftime("%Y%m%d%H%M.%S", time.localtime(int(sys.argv[1]))))' "$ancient")
mkdir -p "$H/Downloads" "$H/Movies" "$H/Documents" "$H/Library/Caches" "$H/.ollama/models" "$H/Pictures/Photos Library.photoslibrary/originals" "$H/Music/iTunes/iTunes Media/Music"
for f in "Downloads/old installer.dmg" Downloads/opened.iso Downloads/atime.zip Library/Caches/big.bin .ollama/models/blob \
         "Pictures/Photos Library.photoslibrary/originals/x.heic" Movies/open.mov "Music/iTunes/iTunes Media/Music/track.mp3"; do echo x > "$H/$f"; touch -t "$ancstamp" "$H/$f"; done
touch -a "$H/Downloads/atime.zip"            # read today: atime is newer than mtime
echo x > "$H/Documents/young.mov"
cat > "$T/mdls" <<'SH'
#!/bin/sh
# Fake Spotlight: opened.iso was opened two days ago, nothing else ever.
case "$4" in *opened.iso) date -u -v-2d '+%Y-%m-%d %H:%M:%S +0000' 2>/dev/null || date -u -d '-2 days' '+%Y-%m-%d %H:%M:%S +0000' ;; *) echo '(null)' ;; esac
SH
cat > "$T/lsof" <<'SH'
#!/bin/sh
case "$3" in *open.mov) echo 4242 ;; esac
SH
chmod +x "$T/mdls" "$T/lsof"

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
$(finding "$T/dev/f/.venv" build true $old null),
$(finding "$H/Downloads/old installer.dmg" stale false $ancient null),
$(finding "$H/Downloads/opened.iso" stale false $ancient null),
$(finding "$H/Downloads/atime.zip" stale false $ancient null),
$(finding "$H/Library/Caches/big.bin" stale false $ancient null),
$(finding "$H/.ollama/models/blob" stale false $ancient null),
$(finding "$H/Pictures/Photos Library.photoslibrary/originals/x.heic" stale false $ancient null),
$(finding "$H/Movies/open.mov" stale false $ancient null),
$(finding "$H/Music/iTunes/iTunes Media/Music/track.mp3" stale false $ancient null),
$(finding "$H/Documents/young.mov" large false $young null)]}
EOF
}
cat > "$T/batches.json" <<EOF
{"batches":[
 {"id":"OWNOLD","session":"disk-reclaim-tick","status":"staged","created_at":$((now - 4*86400)),"bytes":1},
 {"id":"OWNNEW","session":"disk-reclaim-tick","status":"staged","created_at":$recent,"bytes":1},
 {"id":"PEEROLD","session":"amux-admin","status":"staged","created_at":$((now - 9*86400)),"bytes":1},
 {"id":"OWNGONE","session":"disk-reclaim-tick","status":"purged","created_at":$((now - 9*86400)),"bytes":1},
 {"id":"STALEOLD","session":"disk-reclaim-stale","status":"staged","created_at":$((now - 31*86400)),"bytes":1},
 {"id":"STALENEW","session":"disk-reclaim-stale","status":"staged","created_at":$((now - 4*86400)),"bytes":1}]}
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
        # The real reply's shape (reclaim.rs create_quarantine): moved is a COUNT.
        self.send({"batch_id": "B1", "moved": len(paths[:-1]), "refused": [{"path": paths[-1], "class": "owned", "reason": "test"}] if paths else []})
    def do_DELETE(self):
        open(T + "/deletes.log", "a").write(self.path + "\n"); self.send({"ok": True})
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(T + "/port", "w").write(str(s.server_port)); s.serve_forever()
PY
python3 "$T/fake.py" "$T" & SRV=$!
for _ in $(seq 50); do [ -s "$T/port" ] && break; sleep 0.1; done
API="http://127.0.0.1:$(cat "$T/port")"

run() { : > "$T/posts.log"; : > "$T/deletes.log"; rm -f -- "$T/status.json"
  HOME="$H" RECLAIM_API=$API RECLAIM_ROOTS="$T/dev" RECLAIM_TOOLS=0 RECLAIM_LOG="$T/tick.log" \
    RECLAIM_STATUS_FILE="$T/status.json" RECLAIM_SCOPE_FILE="${SCOPE:-$T/no-scope}" \
    RECLAIM_MDLS="$T/mdls" RECLAIM_LSOF="$T/lsof" \
    bash "$TICK" "$@" > "$T/out.txt" 2>&1 && echo 0 > "$T/rc" || echo $? > "$T/rc"; }
stale_posted() { grep '^disk-reclaim-stale' "$T/posts.log" | grep -c -F "$1"; }
status() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); c={x["key"]:x for x in d["classes"]}; print(eval(sys.argv[2]))' "$T/status.json" "$1" 2>/dev/null; }
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
grep -q 'done dry=0 picked=2 moved=1 refused=1 stale_picked=1 stale_moved=0 purged=2' "$T/out.txt" && ok "summary line is computed from the run" || { bad "summary line"; tail -2 "$T/out.txt"; }

# ── stale data ──
[ "$(stale_posted "$H/Downloads/old installer.dmg")" = 1 ] && ok "stale: a file unopened 200d is quarantined under disk-reclaim-stale" || bad "stale: unopened file quarantined"
grep '^disk-reclaim-tick' "$T/posts.log" | grep -q -F "$H/" && bad "stale file must not ride the build batch" || ok "stale: files go in their own batch, never the build one"
[ "$(stale_posted opened.iso)" = 0 ] && ok "stale: Spotlight last-used inside the window keeps it" || bad "stale: Spotlight last-used"
[ "$(stale_posted atime.zip)" = 0 ] && ok "stale: atime inside the window keeps it" || bad "stale: atime"
[ "$(stale_posted big.bin)" = 0 ] && ok "stale: ~/Library is never entered" || bad "stale: Library excluded"
[ "$(stale_posted .ollama)" = 0 ] && ok "stale: dot-dirs are never entered" || bad "stale: dot-dir excluded"
[ "$(stale_posted track.mp3)" = 0 ] && ok "stale: an app-indexed media folder (iTunes Media) is never entered" || bad "stale: iTunes Media excluded"
[ "$(stale_posted x.heic)" = 0 ] && ok "stale: nothing is pulled out of a bundle" || bad "stale: bundle excluded"
[ "$(stale_posted open.mov)" = 0 ] && grep -q 'open.mov (open in a running process)' "$T/out.txt" && ok "stale: a file open in a process stays" || bad "stale: open file kept"
[ "$(stale_posted young.mov)" = 0 ] && ok "stale: a big file modified 10d ago stays" || bad "stale: young file kept"
grep -q 'STALEOLD?confirm=STALEOLD' "$T/deletes.log" && ok "stale: a batch past its 30d window is purged" || bad "stale: expired purge"
grep -q 'STALENEW' "$T/deletes.log" && bad "stale batch inside 30d must stay restorable" || ok "stale: a 4d-old batch outlives the build class's 72h"
[ "$(status 'c["stale"]["picked"], c["stale"]["kept_open"], c["stale"]["retain_s"], c["build"]["picked"]')" = "(1, 1, 2592000, 2)" ] \
  && ok "status file carries each class's counts and window" || bad "status file: $(status 'c')"

# ── knobs from the global scope; an explicit env value wins ──
printf 'OTHER=1\nRECLAIM_STALE_DAYS="400"\n' > "$T/scope.env"
SCOPE="$T/scope.env" run
[ "$(stale_posted "old installer.dmg")" = 0 ] && grep -q 'unused>=400d' "$T/out.txt" && ok "scope: RECLAIM_STALE_DAYS from the Scope tab applies" || bad "scope knob applies"
SCOPE="$T/scope.env" RECLAIM_STALE_DAYS=90 run
[ "$(stale_posted "old installer.dmg")" = 1 ] && ok "scope: an explicit env value beats the scope file" || bad "env beats scope"
RECLAIM_STALE=0 run
[ "$(grep -c '^disk-reclaim-stale' "$T/posts.log")" = 0 ] && [ "$(status 'c["stale"]["enabled"]')" = False ] && ok "RECLAIM_STALE=0 turns the class off and says so" || bad "stale off switch"

# ── dry run ──
run --dry-run
[ ! -s "$T/posts.log" ] && [ ! -s "$T/deletes.log" ] && ok "dry run posts and deletes nothing" || bad "dry run is inert"
grep -q 'would move 2 path' "$T/out.txt" && grep -q 'would move 1 file' "$T/out.txt" && grep -q 'would purge batch OWNOLD' "$T/out.txt" && ok "dry run says what it would do" || bad "dry run report"

# ── ownership not measured ──
mkscan false; run
[ ! -s "$T/posts.log" ] && grep -q 'verdict=reclaim_tick_unmeasured' "$T/out.txt" && ok "unmeasured ownership moves nothing and says so" || bad "unmeasured ownership"

# ── no rm on a candidate, ever ──
grep -nE '(^|[^a-z_])rm ' "$TICK" | grep -v -- '"${WORK:?}"' | grep -v '^[0-9]*:#' > "$T/rm.txt" || true
[ ! -s "$T/rm.txt" ] && ok "the tick has no rm except its own scratch dir" || { bad "rm found"; cat "$T/rm.txt"; }

echo "disk-reclaim-tick: $pass passed, $fail failed"
[ "$fail" = 0 ]
