#!/usr/bin/env python3
"""Load and verify a dedicated amux host from another machine.

The host is the one cloud/desktop/bootstrap.sh builds: amux behind Caddy with
HTTP basic auth, where Caddy stamps the authenticated user as
X-Amux-User-Email. This script is the "from here" half.

  remote.py load   ENV.yaml   apply an EnvSpec (groups, workers, files,
                              schedules, columns, cards, prompts) through the
                              server's own /api/env/apply, upload binary files
                              listed under x-uploads, then start the workers
  remote.py verify [ENV.yaml] prove the acceptance criteria and print one
                              PASS/FAIL line per check

Connection, from the environment:
  AMUX_REMOTE        https://wexus.amux.io
  AMUX_REMOTE_USER   basic-auth user (becomes the identity amux records)
  AMUX_REMOTE_PASS   its password
  AMUX_SESSION       this machine's lane, sent as X-Amux-Session so the remote
                     board records which local worker acted (already set in
                     every amux pane)

Exit status is the number of failed checks (0 = all passed).
"""
import base64, json, os, ssl, sys, time, urllib.error, urllib.parse, urllib.request, uuid

try:
    import yaml
except ImportError:
    sys.exit("pyyaml required: pip3 install pyyaml")

BASE = os.environ.get("AMUX_REMOTE", "").rstrip("/")
USER = os.environ.get("AMUX_REMOTE_USER", "")
PASS = os.environ.get("AMUX_REMOTE_PASS", "")
LANE = os.environ.get("AMUX_SESSION", "") or "remote-cli"

FAILS = 0
PASSES = 0


def check(ok, what, detail=""):
    global FAILS, PASSES
    if ok:
        PASSES += 1
    else:
        FAILS += 1
    print(f"{'PASS' if ok else 'FAIL'}  {what}" + (f"  ({detail})" if detail else ""), flush=True)
    return ok


def req(method, path, body=None, raw=None, ctype="application/json", auth=True,
        user=None, pw=None, extra=None, timeout=60):
    """Returns (status, text). Never raises on HTTP errors: a status is data."""
    h = {"X-Amux-Session": LANE}
    if auth:
        tok = base64.b64encode(f"{user or USER}:{pw if pw is not None else PASS}".encode()).decode()
        h["Authorization"] = f"Basic {tok}"
    if extra:
        h.update(extra)
    data = raw
    if body is not None:
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
    if data is not None:
        h["Content-Type"] = ctype
    r = urllib.request.Request(BASE + path, data=data, method=method, headers=h)
    ctx = ssl.create_default_context()
    if os.environ.get("AMUX_REMOTE_INSECURE") == "1":  # rehearsal against a self-signed local server only
        ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=ctx) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")
    except Exception as e:  # DNS, TLS, refused: report as 000 like curl
        return 0, f"{type(e).__name__}: {e}"


def jget(path, **kw):
    code, txt = req("GET", path, **kw)
    try:
        return code, json.loads(txt)
    except Exception:
        return code, None


def load_spec(path):
    spec = yaml.safe_load(open(path)) or {}
    if not isinstance(spec, dict):
        sys.exit(f"{path}: not a YAML mapping")
    return spec


def upload(path_on_host, local_file):
    directory, _, fname = path_on_host.rpartition("/")
    req("POST", "/api/fs/mkdir", body={"path": directory})
    blob = open(local_file, "rb").read()
    b = "----amux" + uuid.uuid4().hex
    payload = b"".join([
        f"--{b}\r\nContent-Disposition: form-data; name=\"dir\"\r\n\r\n{directory}\r\n".encode(),
        f"--{b}\r\nContent-Disposition: form-data; name=\"overwrite\"\r\n\r\n1\r\n".encode(),
        (f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{fname}\"\r\n"
         "Content-Type: application/octet-stream\r\n\r\n").encode() + blob + b"\r\n",
        f"--{b}--\r\n".encode(),
    ])
    code, txt = req("POST", "/api/fs/upload", raw=payload,
                    ctype=f"multipart/form-data; boundary={b}", timeout=120)
    return code, len(blob), txt


def running_workers():
    code, rows = jget("/api/sessions")
    if code != 200 or not isinstance(rows, list):
        return None
    return {s.get("name"): bool(s.get("running")) for s in rows}


def cmd_load(env_path):
    spec = load_spec(env_path)
    here = os.path.dirname(os.path.abspath(env_path))
    uploads = spec.pop("x-uploads", []) or []
    body = json.dumps({k: v for k, v in spec.items() if not k.startswith(("x-", "_"))}).encode()

    code, txt = req("POST", "/api/env/apply?dry_run=1", body=body, timeout=180)
    if not check(code == 200, "dry-run apply accepted", f"HTTP {code} {txt[:200] if code != 200 else ''}"):
        return
    code, txt = req("POST", "/api/env/apply", body=body, timeout=180)
    try:
        resp = json.loads(txt)
    except Exception:
        resp = {}
    check(code == 200, "env apply", f"HTTP {code}")
    report = resp.get("report", [])
    tally = {}
    for r in report:
        tally.setdefault(r.get("kind"), {}).setdefault(r.get("action"), 0)
        tally[r.get("kind")][r.get("action")] += 1
    for kind, acts in sorted(tally.items(), key=lambda kv: kv[0] or ""):
        print(f"      {kind}: " + ", ".join(f"{a} x{n}" for a, n in sorted(acts.items())))
    errs = [r for r in report if r.get("action") == "error"] + [{"detail": e} for e in resp.get("errors", [])]
    check(not errs, "apply reported no item errors", "; ".join(str(e.get("detail")) for e in errs)[:300])

    # Board columns: /api/env/apply still reports them "not-yet-applied"
    # (env_config.rs phase 2), so create the missing ones through the board API,
    # matching by label so a re-run adds nothing.
    want_cols = spec.get("columns") or []
    if want_cols:
        c, cols = jget("/api/board/statuses")
        have = {str(x.get("label", "")).lower() for x in (cols or [])}
        for label in want_cols:
            if label.lower() in have:
                check(True, f"column {label!r} exists")
                continue
            c, t = req("POST", "/api/board/statuses", body={"label": label})
            check(c in (200, 201), f"create column {label!r}", f"HTTP {c} {t[:120] if c not in (200, 201) else ''}")

    for u in uploads:
        src = os.path.normpath(os.path.join(here, u["source"]))
        if not os.path.exists(src):
            check(False, f"upload {u['path']}", f"local source missing: {src}")
            continue
        c, n, t = upload(u["path"], src)
        check(c == 200, f"upload {u['path']}", f"{n} bytes, HTTP {c}")

    # apply declares workers; starting a pane is the separate, non-idempotent
    # half (env_config.rs). Start only the ones not already running.
    if os.environ.get("AMUX_REMOTE_NO_START") == "1":
        print("      AMUX_REMOTE_NO_START=1: workers declared, not started")
        return
    live = running_workers() or {}
    for w in spec.get("workers", []):
        name = w["name"]
        if live.get(name):
            check(True, f"worker {name} already running")
            continue
        c, t = req("POST", f"/api/sessions/{urllib.parse.quote(name)}/start", timeout=120)
        check(c in (200, 201, 202), f"start worker {name}", f"HTTP {c}")
    time.sleep(15)
    live = running_workers() or {}
    for w in spec.get("workers", []):
        check(bool(live.get(w["name"])), f"worker {w['name']} running after start")


def cmd_verify(env_path=None):
    # 1. Reachable, and the door is shut without credentials.
    code, _ = req("GET", "/health", auth=False)
    check(code == 401, "no credentials -> 401", f"HTTP {code}")
    code, _ = req("GET", "/api/sessions", auth=True, pw=PASS + "-wrong")
    check(code == 401, "wrong password -> 401", f"HTTP {code}")
    code, h = jget("/health")
    check(code == 200 and isinstance(h, dict), "authenticated /health", f"HTTP {code}, commit {(h or {}).get('commit')}")

    # 2. Who the server says we are. Behind bootstrap.sh's Caddy each web user
    # is an amux invited member, verified from a cookie Caddy attaches after
    # basic auth, so this must be a member even when we forge every override.
    code, ident = jget("/api/identity", extra={"Cookie": "amux_member=forged", "X-Amux-Worker": "impostor",
                                               "X-Amux-User-Email": "impostor@example.com"})
    ident = ident or {}
    email = ident.get("email") or ""
    check(bool(ident.get("is_local_member")) and "@" in email,
          "authenticated user is a verified amux member (forged cookie/headers ignored)",
          f"email={email!r} scope={ident.get('access_scope')}")
    check(bool(ident.get("has_api_key")), "Claude API key configured on host", f"has_api_key={ident.get('has_api_key')}")
    actor = f"member:{email}"

    # 3. A write from here is attributed to that member, even when the client
    # claims to be someone else, and so is an edit.
    title = f"remote attribution probe {uuid.uuid4().hex[:8]}"
    code, txt = req("POST", "/api/board", body={"title": title, "status": "backlog"},
                    extra={"X-Amux-Worker": "impostor", "X-Amux-User-Email": "impostor@example.com"})
    try:
        card = json.loads(txt)
    except Exception:
        card = {}
    cid = card.get("id")
    check(code in (200, 201) and cid and card.get("creator") == actor,
          "card created from here is attributed to the member, not the claimed impostor",
          f"HTTP {code} id={cid} creator={card.get('creator')!r}")
    if cid:
        code, _ = req("PATCH", f"/api/board/{cid}", body={"desc_append": "attribution probe edit"})
        code, full = jget(f"/api/board/{cid}")
        log = str((full or {}).get("log") or "")
        check(f"{actor}: desc" in log or actor in log, "card edit is attributed to the member in the card log",
              log.strip().splitlines()[-1][:160] if log.strip() else "empty log")
        code, _ = req("DELETE", f"/api/board/{cid}")
        check(code in (200, 204), "remove probe card", f"HTTP {code}")

    # 4. The loaded environment is all there.
    if env_path:
        spec = load_spec(env_path)
        live = running_workers()
        check(live is not None, "session list readable")
        live = live or {}
        for w in spec.get("workers", []):
            check(bool(live.get(w["name"])), f"worker {w['name']} running",
                  "declared, not running" if w["name"] in live else "not declared")
        code, cols = jget("/api/board/statuses")
        have = {str(x.get("label", "")).lower() for x in (cols or [])}
        missing = [c for c in spec.get("columns") or [] if c.lower() not in have]
        check(not missing, f"board columns present ({len(spec.get('columns') or []) - len(missing)}/{len(spec.get('columns') or [])})",
              str(missing))
        code, scheds = jget("/api/schedules")
        rows = scheds if isinstance(scheds, list) else (scheds or {}).get("schedules", [])
        have = {(s.get("session"), s.get("title")) for s in rows}
        want = [(s["worker"], s["title"]) for s in spec.get("schedules", [])]
        missing = [t for t in want if t not in have]
        check(not missing, f"schedules present ({len(want) - len(missing)}/{len(want)})", str(missing)[:200])
        code, cards = jget("/api/board?all=1")
        titles = {c.get("title") for c in (cards or []) if not c.get("archived")}
        want = [c["title"] for c in spec.get("cards", [])]
        missing = [t for t in want if t not in titles]
        check(not missing, f"cards present ({len(want) - len(missing)}/{len(want)})", str(missing)[:200])
        files = [f["path"] for f in spec.get("files", [])] + [u["path"] for u in spec.get("x-uploads", [])]
        bad = []
        for p in files:
            c, _ = req("GET", "/api/fs/read?path=" + urllib.parse.quote(p))
            if c != 200:
                bad.append(f"{p}:{c}")
        check(not bad, f"files readable ({len(files) - len(bad)}/{len(files)})", ", ".join(bad)[:200])


def main():
    if not (BASE and USER and PASS):
        sys.exit("set AMUX_REMOTE, AMUX_REMOTE_USER, AMUX_REMOTE_PASS")
    if len(sys.argv) < 2 or sys.argv[1] not in ("load", "verify"):
        sys.exit(__doc__)
    print(f"remote {BASE} as {USER}, lane {LANE}")
    if sys.argv[1] == "load":
        if len(sys.argv) < 3:
            sys.exit("usage: remote.py load ENV.yaml")
        cmd_load(sys.argv[2])
    else:
        cmd_verify(sys.argv[2] if len(sys.argv) > 2 else None)
    print(f"{PASSES} passed, {FAILS} failed")
    sys.exit(min(FAILS, 100))


if __name__ == "__main__":
    main()
