"""Fleet API: what a customer server tells the control plane, and what it gets back
(docs/design/cloud-self-service.md, 3.1, J7, 8.2, 8.5).

Each server authenticates with its own fleet secret, so it can only ever speak
for itself. Everything here is outbound from the server, which is why updates,
backups and lifecycle keep working when a customer turns on tailnet-only.
"""
import hmac, time

import provision, releases
from tokens import new_secret

INVITE_EMAILS_PER_DAY = 20


def authenticate(store, server_id, bearer):
    srv = store.one("SELECT * FROM servers WHERE id=?", server_id)
    if not srv or not bearer or not hmac.compare_digest(srv["fleet_secret"], bearer):
        return None
    if srv["status"] in ("deleted", "failed"):
        return None
    return srv


def heartbeat(store, infra, srv, body, now=None):
    """body: {version, last_backup_at, disk_used_pct, csr?, serving: bool, door_ok, amux_ok}."""
    now = now or time.time()
    store.run("UPDATE servers SET last_heartbeat=?, version=?, last_backup_at=COALESCE(?, last_backup_at) WHERE id=?",
              now, body.get("version"), body.get("last_backup_at"), srv["id"])
    reply = {"commands": [], "tailnet_only": bool(srv["tailnet_only"])}
    if body.get("csr"):
        try:
            reply["origin_certificate"] = infra.origin_certificate(srv["slug"], body["csr"])
            store.event("cloud.origin_cert", srv["org_id"], server=srv["id"])
        except Exception as e:
            store.event("cloud.origin_cert_failed", srv["org_id"], server=srv["id"], error=str(e)[:200])
    if srv["status"] == "provisioning" and body.get("serving") and body.get("last_backup_at"):
        provision.mark_ready(store, srv["id"], now)
        reply["ready"] = True
    if body.get("need_backup_creds") or (srv["status"] == "provisioning" and not srv["backup_bucket"]):
        try:
            creds = infra.backup_bucket(srv["slug"], srv["id"])
            restic_pw = srv["restic_password"] or new_secret()
            store.run("UPDATE servers SET backup_bucket=?, restic_password=? WHERE id=?", creds["bucket"], restic_pw, srv["id"])
            reply["backup_credentials"] = dict(creds, restic_password=restic_pw)
        except Exception as e:
            store.event("cloud.backup_creds_failed", srv["org_id"], server=srv["id"], error=str(e)[:200])
    releases.report(store, srv, body.get("release_report"), now)
    offer = releases.offer_for(store, dict(srv, version=body.get("version")), now)
    if offer:
        reply["release"] = offer
    return reply


def invite_email(store, srv, body, now=None):
    """8.2: the customer server asks us to send email 7. Only to the invite's own address, 20 a day."""
    now = now or time.time()
    to, invite_url = (body.get("email") or "").strip().lower(), body.get("invite_url") or ""
    if "@" not in to or not invite_url.startswith(f"https://cloud.amux.io/invite/"):
        return 400, {"error": "email and a cloud.amux.io invite_url are required"}
    sent = store.one("SELECT COUNT(*) n FROM outbox WHERE template='invited' AND ts>? AND params LIKE ?",
                     now - 86400, f'%"server": "{srv["id"]}"%')["n"]
    if sent >= INVITE_EMAILS_PER_DAY:
        return 429, {"error": "invite email limit reached for today"}
    store.enqueue_email(to, "invited", dedupe=f"invite:{invite_url}", ts=now, server=srv["id"], slug=srv["slug"],
                        invite_url=invite_url, inviter=body.get("inviter") or "")
    store.event("cloud.invite_sent", srv["org_id"], server=srv["id"])
    return 200, {"queued": True}
