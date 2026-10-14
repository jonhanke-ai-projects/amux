#!/usr/bin/env python3
"""amux Cloud fleet agent: runs on every customer server (docs/design/cloud-self-service.md, 3.1, J7, J14).

Everything it does is outbound, so it keeps working when the customer turns on
tailnet-only access.

  first boot   make this host's key and CSR, trade the CSR for a Cloudflare Origin
               CA certificate in the first heartbeat, run bootstrap.sh in fleet mode,
               write the backup config, take the first backup, then report serving
  every minute heartbeat: version, last backup, disk; apply what the reply says
               (backup credentials once, a release to install, tailnet-only)
  backups      litestream streams amux.db to this server's own R2 bucket every
               60 seconds; restic snapshots the home directory nightly
  releases     download, verify with `ssh-keygen -Y verify` against the pinned
               release key, swap by rename, health-check 5 minutes, roll back alone

Standard library plus tools already on the image (openssl, ssh-keygen,
litestream, restic, systemctl). State lives in /var/lib/amux-fleet.

  fleet_agent.py run        the service loop
  fleet_agent.py once       one heartbeat, for debugging
"""
import hashlib, json, os, shutil, subprocess, sys, time, urllib.error, urllib.request

FLEET_ENV = os.environ.get("FLEET_ENV", "/etc/amux/fleet.env")
STATE = os.environ.get("FLEET_STATE", "/var/lib/amux-fleet")
ETC = os.environ.get("FLEET_ETC", "/etc/amux")
BOOTSTRAP = os.environ.get("FLEET_BOOTSTRAP", "/opt/amux/bootstrap.sh")
SERVER_BIN = os.environ.get("FLEET_SERVER_BIN", "/usr/local/bin/amux-server")
SERVER_UNIT = os.environ.get("FLEET_SERVER_UNIT", "amux-server.service")
HEALTH = os.environ.get("FLEET_HEALTH", "https://127.0.0.1:8824/health")
RELEASE_SIGNERS = os.environ.get("FLEET_RELEASE_SIGNERS", "/etc/amux/release_signers")
INTERVAL = float(os.environ.get("FLEET_INTERVAL", "60"))
HEALTH_SOAK = float(os.environ.get("FLEET_HEALTH_SOAK", "300"))


def log(**kv):
    kv["ts"] = round(time.time(), 3)
    print(json.dumps(kv, sort_keys=True), flush=True)


def env():
    out = {}
    with open(FLEET_ENV) as f:
        for line in f:
            k, _, v = line.strip().partition("=")
            if k:
                out[k] = json.loads(v) if v.startswith('"') else v
    return out


def sh(*cmd, check=True, timeout=600, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, **kw)
    if check and r.returncode != 0:
        raise RuntimeError(f"{cmd[0]} exited {r.returncode}: {r.stderr.strip()[:300]}")
    return r


def state(name, value=None):
    os.makedirs(STATE, exist_ok=True)
    p = os.path.join(STATE, name)
    if value is None:
        try:
            with open(p) as f:
                return json.load(f)
        except (FileNotFoundError, ValueError):
            return None
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(value, f)
    os.replace(tmp, p)
    return value


# ------------------------------------------------------------------ control plane
def post(cfg, verb, body, timeout=30):
    url = f"{cfg['AMUX_CONTROL_URL'].rstrip('/')}/api/fleet/{cfg['AMUX_SERVER_ID']}/{verb}"
    req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST",
                                 headers={"Authorization": f"Bearer {cfg['AMUX_FLEET_SECRET']}",
                                          "Content-Type": "application/json", "User-Agent": "amux-fleet/1"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


# ------------------------------------------------------------------ certificate
def ensure_csr():
    key, csr = os.path.join(ETC, "origin.key"), os.path.join(STATE, "origin.csr")
    os.makedirs(STATE, exist_ok=True)
    if not os.path.exists(key):
        sh("openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", key)
        os.chmod(key, 0o600)
    if not os.path.exists(csr):
        sh("openssl", "req", "-new", "-key", key, "-subj", "/CN=amux-cloud", "-out", csr)
    with open(csr) as f:
        return f.read()


def have_cert():
    return os.path.exists(os.path.join(ETC, "origin.crt")) and os.path.getsize(os.path.join(ETC, "origin.crt")) > 0


# ------------------------------------------------------------------ backups
def litestream_last():
    """End time of the newest replicated WAL segment, from `litestream generations` (last column, ISO time)."""
    cfgp = os.path.join(ETC, "litestream.yml")
    if not os.path.exists(cfgp):
        return None
    r = sh("litestream", "generations", "-config", cfgp, "/home/amux/.amux/amux.db", check=False, timeout=30)
    newest = None
    for line in r.stdout.splitlines()[1:]:
        parts = line.split()
        if parts:
            try:
                t = time.mktime(time.strptime(parts[-1].replace("Z", ""), "%Y-%m-%dT%H:%M:%S"))
                newest = max(newest or 0, t - time.timezone)
            except ValueError:
                continue
    return newest


def last_backup_at():
    """The database's last replicated write (continuous) once litestream runs, else the last file snapshot.
    The control plane needs this under an hour old before it may delete anything (plan 3.2 rule 6)."""
    files = (state("backup") or {}).get("last_ok")
    db = litestream_last()
    return db if db and files else files


def write_backup_config(creds):
    """creds: {endpoint, bucket, access_key_id, secret_access_key, restic_password} for this server only."""
    with open(os.path.join(ETC, "litestream.yml"), "w") as f:
        f.write(f"""# written by fleet_agent.py; this server's own bucket
dbs:
  - path: /home/amux/.amux/amux.db
    replicas:
      - type: s3
        endpoint: {creds['endpoint']}
        bucket: {creds['bucket']}
        path: litestream/amux.db
        access-key-id: {creds['access_key_id']}
        secret-access-key: {creds['secret_access_key']}
        sync-interval: 60s
        snapshot-interval: 24h
        retention: 168h
""")
    os.chmod(os.path.join(ETC, "litestream.yml"), 0o600)
    with open(os.path.join(ETC, "restic.env"), "w") as f:
        f.write(f"RESTIC_REPOSITORY=s3:{creds['endpoint']}/{creds['bucket']}/restic\n"
                f"AWS_ACCESS_KEY_ID={creds['access_key_id']}\nAWS_SECRET_ACCESS_KEY={creds['secret_access_key']}\n"
                f"RESTIC_PASSWORD={creds['restic_password']}\n")
    os.chmod(os.path.join(ETC, "restic.env"), 0o600)
    state("backup_configured", {"at": time.time(), "bucket": creds["bucket"]})


def run_backup(full=False):
    """Snapshot files with restic (and the database through litestream's own snapshot). Records success time."""
    envf = os.path.join(ETC, "restic.env")
    if not os.path.exists(envf):
        return False
    e = dict(os.environ)
    with open(envf) as f:
        e.update(dict(l.strip().split("=", 1) for l in f if "=" in l))
    if sh("restic", "cat", "config", env=e, check=False).returncode != 0:
        sh("restic", "init", env=e)
    sh("restic", "backup", "--exclude", ".cache", "--exclude", "node_modules", "--exclude", "target",
       "/home/amux", env=e, timeout=3600)
    if full:
        sh("litestream", "snapshots", "-config", os.path.join(ETC, "litestream.yml"), "/home/amux/.amux/amux.db", check=False)
    state("backup", {"last_ok": time.time()})
    log(event="backup", verdict="fleet_backup_ok")
    return True


# ------------------------------------------------------------------ releases
def server_version():
    try:
        import ssl
        ctx = ssl.create_default_context(); ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE
        with urllib.request.urlopen(HEALTH, timeout=5, context=ctx) as r:
            d = json.load(r)
        return d.get("version") or d.get("commit")
    except Exception:
        return None


def healthy_for(seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if server_version() is None:
            return False
        time.sleep(min(15, max(1, seconds / 20)))
    return True


def install_release(rel):
    """rel: {version, url, sha256, sig_url}. Verified before anything changes; old binary kept for rollback."""
    stage = os.path.join(STATE, "release")
    os.makedirs(stage, exist_ok=True)
    binp, sigp = os.path.join(stage, "amux-server.new"), os.path.join(stage, "amux-server.new.sig")
    urllib.request.urlretrieve(rel["url"], binp)
    urllib.request.urlretrieve(rel["sig_url"], sigp)
    with open(binp, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    if digest != rel["sha256"]:
        raise RuntimeError(f"release {rel['version']}: sha256 {digest} does not match {rel['sha256']}")
    with open(binp, "rb") as data:
        r = subprocess.run(["ssh-keygen", "-Y", "verify", "-f", RELEASE_SIGNERS, "-I", "release@amux.io",
                            "-n", "amux-release", "-s", sigp], stdin=data, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"release {rel['version']}: signature did not verify: {r.stderr.strip()[:200]}")
    os.chmod(binp, 0o755)
    prev = SERVER_BIN + ".prev"
    shutil.copy2(SERVER_BIN, prev)                      # rollback target
    os.replace(binp, SERVER_BIN)                        # rename: a running process keeps its old image
    sh("systemctl", "restart", SERVER_UNIT)
    if healthy_for(HEALTH_SOAK):
        state("release_result", {"version": rel["version"], "result": "installed", "at": time.time()})
        log(event="release", version=rel["version"], verdict="fleet_release_installed")
        return "installed"
    os.replace(prev, SERVER_BIN)
    sh("systemctl", "restart", SERVER_UNIT)
    state("release_result", {"version": rel["version"], "result": "rolled_back", "at": time.time()})
    log(event="release", version=rel["version"], verdict="fleet_release_rolled_back")
    return "rolled_back"


# ------------------------------------------------------------------ the loop
def first_boot(cfg):
    """Idempotent: each step records itself in STATE and is skipped once done."""
    if not have_cert():
        reply = post(cfg, "heartbeat", {"version": None, "csr": ensure_csr(), "serving": False})
        if reply.get("origin_certificate"):
            with open(os.path.join(ETC, "origin.crt"), "w") as f:
                f.write(reply["origin_certificate"])
            log(event="first_boot", step="certificate", verdict="fleet_cert_ok")
        apply_reply(cfg, reply)
        if not have_cert():
            return False
    if not state("bootstrapped"):
        sh("bash", BOOTSTRAP, env=dict(os.environ, AMUX_SKIP_PACKAGES="1"), timeout=1800)
        state("bootstrapped", {"at": time.time()})
        log(event="first_boot", step="bootstrap", verdict="fleet_bootstrap_ok")
    if not state("backup_configured"):
        apply_reply(cfg, post(cfg, "heartbeat", {"version": server_version(), "serving": False, "need_backup_creds": True}))
        if not state("backup_configured"):
            return False
        sh("systemctl", "enable", "--now", "litestream.service", check=False)
    if not last_backup_at():
        run_backup(full=True)
    return True


def apply_reply(cfg, reply):
    if reply.get("backup_credentials") and not state("backup_configured"):
        write_backup_config(reply["backup_credentials"])
    if reply.get("release") and reply["release"].get("version") != server_version():
        try:
            outcome = install_release(reply["release"])
        except Exception as e:
            outcome = f"refused: {e}"
            log(event="release", verdict="fleet_release_refused", error=str(e)[:200])
        state("release_report", {"version": reply["release"].get("version"), "outcome": outcome})
    want_tailnet_only = bool(reply.get("tailnet_only"))
    if (state("tailnet_only") or {}).get("on") != want_tailnet_only:
        state("tailnet_only", {"on": want_tailnet_only})


def heartbeat(cfg):
    du = shutil.disk_usage("/")
    body = {"version": server_version(), "last_backup_at": last_backup_at(),
            "disk_used_pct": round(100 * du.used / du.total, 1), "serving": server_version() is not None,
            "release_report": state("release_report")}
    reply = post(cfg, "heartbeat", body)
    apply_reply(cfg, reply)
    return reply


def run():
    cfg = env()
    log(event="start", server=cfg.get("AMUX_SERVER_ID"), verdict="fleet_agent_started")
    while True:
        try:
            if first_boot(cfg):
                heartbeat(cfg)
                last = (state("backup") or {}).get("last_ok") or 0
                if time.time() - last > 86400:
                    run_backup()
        except urllib.error.URLError as e:
            log(event="heartbeat", verdict="fleet_control_unreachable", error=str(e)[:200])
        except Exception as e:
            log(event="loop", verdict="fleet_loop_error", error=str(e)[:300])
        time.sleep(INTERVAL)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "run":
        run()
    elif cmd == "once":
        print(json.dumps(heartbeat(env()), indent=1))
    else:
        sys.exit(__doc__)
