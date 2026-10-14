#!/usr/bin/env python3
"""amux Cloud control plane: cloud.amux.io (docs/design/cloud-self-service.md, section 3).

Plain code, no agents. Billing, sign-in, provisioning, lifecycle, the redirect
and kill switches all live here and keep working with the Operator stopped.

Env:
  CONTROL_DB, CONTROL_SECRET, CONTROL_PUBLIC_URL, CONTROL_PORT
  STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET
  HCLOUD_TOKEN, CLOUDFLARE_API_TOKEN, CF_ZONE_ID_AMUX_CLOUD, AMUX_GOLDEN_IMAGE
  CLERK_PUBLISHABLE_KEY, CLERK_SECRET_KEY
  CONTROL_ADMIN_TOKEN       kill switches and alerts
  SMTP_URL, CONTROL_MAIL_FROM
  CONTROL_FAKE=1            fake Stripe, Hetzner and Cloudflare, plus a fake checkout
                            page and dev sign-in, for local end-to-end tests only
"""
import html, json, os, sys, threading, time, traceback, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import accounts, emails, fleet, lifecycle, provision, tokens  # noqa: E402
from price_check import load_plans  # noqa: E402
from store import new_id, open_store  # noqa: E402

PLANS = load_plans()
PLAN_IDS = {p["id"] for p in PLANS["plans"]}
PUBLIC = os.environ.get("CONTROL_PUBLIC_URL", "https://cloud.amux.io").rstrip("/")
SITE = os.environ.get("AMUX_SITE_URL", "https://amux.io").rstrip("/")
CUSTOMER_DOMAIN = os.environ.get("AMUX_CUSTOMER_DOMAIN", "amux.cloud")
SECRET = os.environ.get("CONTROL_SECRET") or ""
FAKE = os.environ.get("CONTROL_FAKE") == "1"
COOKIE = "__Host-amux_cp" if PUBLIC.startswith("https://") else "amux_cp"
SESSION_TTL = 7 * 86400


# ------------------------------------------------------------------ providers
def make_providers():
    if FAKE:
        from fakes import FakeBilling, FakeInfra
        b, i = FakeBilling(), FakeInfra()
        b.sessions = {}
        return b, i
    from billing import StripeBilling
    from infra import Infra
    return (StripeBilling(os.environ["STRIPE_SECRET_KEY"], PLANS),
            Infra(os.environ["HCLOUD_TOKEN"], os.environ["CLOUDFLARE_API_TOKEN"], os.environ["CF_ZONE_ID_AMUX_CLOUD"],
                  CUSTOMER_DOMAIN))


STORE = None
BILLING = INFRA = None


def server_url(srv):
    return f"https://{srv['slug']}.{CUSTOMER_DOMAIN}"


# ------------------------------------------------------------------ pages
STYLE = """<style>
:root{color-scheme:dark light;--fg:#e8e6e1;--bg:#14161a;--dim:#9a988f;--accent:#f0c040}
@media (prefers-color-scheme: light){:root{--fg:#1d1f23;--bg:#f7f6f2;--dim:#55575c;--accent:#7a5a00}}
body{font:16px/1.55 system-ui,sans-serif;background:var(--bg);color:var(--fg);margin:0;padding:max(24px,env(safe-area-inset-top)) 20px}
main{max-width:560px;margin:0 auto}h1{font-size:1.5rem}a{color:var(--accent)}.dim{color:var(--dim)}
.btn{display:inline-flex;align-items:center;min-height:44px;padding:0 18px;border-radius:8px;border:0;
background:var(--accent);color:#14161a;font:inherit;font-weight:600;text-decoration:none;cursor:pointer}
ol.steps li{margin:6px 0}ol.steps li.done{color:var(--dim)}ol.steps li.now{font-weight:600}
input{min-height:44px;padding:0 12px;font:inherit;border-radius:8px;border:1px solid var(--dim);background:transparent;color:inherit}
</style>"""


def page(title, body):
    return (f"<!doctype html><html lang=en><head><meta charset=utf-8>"
            f"<meta name=viewport content='width=device-width,initial-scale=1,viewport-fit=cover'>"
            f"<title>{html.escape(title)} · amux Cloud</title>{STYLE}</head><body><main>{body}</main></body></html>")


PROGRESS_JS = """<script>
const steps=['create','dns','boot','ready'];
async function poll(){
  const r=await fetch('/api/me',{credentials:'same-origin'}); if(!r.ok){location='/';return}
  const me=await r.json(); const s=(me.servers||[])[0]; if(!s){setTimeout(poll,3000);return}
  document.getElementById('host').textContent=s.url;
  const i=Math.max(0,steps.indexOf(s.step));
  document.querySelectorAll('ol.steps li').forEach((li,n)=>{li.className=n<i?'done':(n===i?'now':'')});
  if(s.status==='ready'){location='/go/'+s.id;return}
  if(s.status==='failed'){document.getElementById('msg').textContent='This server could not be built. Your payment is being refunded and we have been told.';return}
  setTimeout(poll,3000)}
poll();
</script>"""


# ------------------------------------------------------------------ handler
class Handler(BaseHTTPRequestHandler):
    server_version = "amux-control"

    def log_message(self, fmt, *a):
        sys.stderr.write("[control] %s %s\n" % (self.address_string(), fmt % a))

    # -- helpers
    def send(self, code, body=b"", ctype="text/html; charset=utf-8", headers=None):
        if isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "same-origin")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def json(self, code, obj, headers=None):
        self.send(code, json.dumps(obj), "application/json", headers)

    def redirect(self, url, headers=None):
        self.send(303, b"", "text/plain", dict({"Location": url}, **(headers or {})))

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(min(n, 2_000_000)) if n else b""

    def query(self):
        return {k: v[0] for k, v in urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).items()}

    def user(self):
        raw = self.headers.get("Cookie") or ""
        for part in raw.split(";"):
            k, _, v = part.strip().partition("=")
            if k == COOKIE:
                try:
                    claims = tokens.verify(v, SECRET)
                except ValueError:
                    return None
                return STORE.one("SELECT * FROM users WHERE id=?", claims["sub"])
        return None

    def session_cookie(self, user_id):
        t = tokens.sign({"sub": user_id}, SECRET, SESSION_TTL)
        secure = "; Secure" if COOKIE.startswith("__Host-") else ""
        return {"Set-Cookie": f"{COOKIE}={t}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL}{secure}"}

    # -- routing
    def do_GET(self):
        self._route("GET")

    def do_POST(self):
        self._route("POST")

    def _route(self, method):
        path = urllib.parse.urlparse(self.path).path
        try:
            for m, prefix, fn in ROUTES:
                if m == method and (path == prefix or (prefix != "/" and prefix.endswith("/") and path.startswith(prefix))):
                    return fn(self, path)
            self.send(404, page("Not found", "<h1>Not found</h1>"))
        except Exception as e:
            traceback.print_exc()
            STORE.event("control.error", path=path, error=str(e)[:300])
            self.send(500, page("Something went wrong", "<h1>Something went wrong</h1><p>We have been told.</p>"))


# ------------------------------------------------------------------ routes
def r_health(h, _):
    h.json(200, {"ok": True, "measured": True, "fake": FAKE, "flags": {r["name"]: r["value"] for r in STORE.all("SELECT * FROM flags")},
                 "email": "configured" if emails.configured() else "not configured: mail is queued, not sent",
                 "prices": PLANS["status"],
                 "n_considered": STORE.one("SELECT COUNT(*) n FROM servers")["n"]})


def r_root(h, _):
    """The redirect table (plan 8.1): every visitor goes to exactly one place."""
    q = h.query()
    u = h.user()
    if not u:
        if q.get("plan") in PLAN_IDS:
            return h.redirect(f"/start?plan={q['plan']}&source={urllib.parse.quote(q.get('source', 'cloud'))}")
        if q.get("from") == "ios" or q.get("signin"):
            return h.redirect("/signin" + ("?from=ios" if q.get("from") == "ios" else ""))
        return h.redirect(f"{SITE}/pricing/")
    servers = visible_servers(u)
    live = [s for s in servers if s["status"] in ("ready", "provisioning")]
    if not live:
        snap = STORE.one("SELECT a.* FROM lifecycle_actions a JOIN servers s ON s.id=a.server_id WHERE s.org_id=? "
                         "AND a.state='done' ORDER BY a.decided_at DESC LIMIT 1", u["org_id"])
        if snap:
            return h.send(200, page("Restore", f"<h1>Your server was saved and turned off</h1><p>Restore it from its snapshot "
                                    f"by resubscribing.</p><p><a class=btn href='/billing'>Restore my server</a></p>"))
        return h.redirect(f"{SITE}/pricing/")
    if len(live) > 1 and not q.get("server"):
        items = "".join(f"<li><a href='/go/{s['id']}'>{html.escape(server_url(s))}</a> <span class=dim>{s['plan']}, "
                        f"{s['status']}</span></li>" for s in live)
        return h.send(200, page("Your servers", f"<h1>Your servers</h1><ul>{items}</ul>"))
    srv = live[0]
    if srv["status"] == "provisioning":
        return h.redirect("/progress")
    h.redirect(f"/go/{srv['id']}")


def visible_servers(u):
    """Servers this person may enter: their own org's (owner) plus any they joined by invite."""
    own = STORE.all("SELECT *, 'owner' AS my_role FROM servers WHERE org_id=? ORDER BY created_at DESC", u["org_id"]) \
        if u["role"] == "owner" else []
    joined = STORE.all("SELECT s.*, 'member' AS my_role FROM servers s JOIN memberships m ON m.server_id=s.id "
                       "WHERE m.user_id=? ORDER BY s.created_at DESC", u["id"])
    seen, out = set(), []
    for srv in own + joined:
        if srv["id"] not in seen:
            seen.add(srv["id"])
            out.append(srv)
    return out


def r_start(h, _):
    q = h.query()
    plan = q.get("plan") if q.get("plan") in PLAN_IDS else "solo"
    if q.get("plan") and q.get("plan") not in PLAN_IDS:
        STORE.event("cloud.unknown_plan", plan=q.get("plan"))
    if STORE.flag("provisioning_freeze") == "1":
        return h.send(503, page("Back soon", "<h1>Back soon</h1><p>New servers are paused for a moment. Existing "
                                "servers are unaffected. Please try again shortly.</p>"))
    if PLANS["status"] != "approved" and not FAKE:
        return h.redirect(f"{SITE}/cloud/#notify")
    signup = new_id("su")
    u = h.user()
    org = STORE.one("SELECT * FROM orgs WHERE id=?", u["org_id"]) if u else None
    sess = BILLING.create_checkout(plan, signup, q.get("source") or "direct",
                                   f"{PUBLIC}/welcome?session_id={{CHECKOUT_SESSION_ID}}", f"{SITE}/pricing/",
                                   customer_email=None if org else q.get("email"),
                                   customer_id=org["stripe_customer_id"] if org else None)
    STORE.event("cloud.checkout_started", org["id"] if org else None, plan=plan, source=q.get("source"))
    h.redirect(sess["url"])


def r_welcome(h, _):
    sid = h.query().get("session_id") or ""
    try:
        r = accounts.complete_checkout(STORE, BILLING, sid)
    except accounts.NotPaid:
        return h.send(402, page("Payment not complete", "<h1>Payment not complete</h1><p>Your checkout did not finish. "
                                f"<a href='{SITE}/pricing/'>Back to pricing</a></p>"))
    # One-time sign-in for the browser that paid (J5); other devices sign in through Clerk.
    h.redirect("/progress", h.session_cookie(r["user"]["id"]))


def r_progress(h, _):
    if not h.user():
        return h.redirect("/signin")
    h.send(200, page("Building your server", "<h1>Building your server</h1><p class=dim id=host></p><ol class=steps>"
                     "<li>Creating your server</li><li>Setting up its address</li><li>Starting amux and taking the "
                     "first backup</li><li>Ready</li></ol><p id=msg class=dim>Usually about three minutes. While you "
                     "wait: have an Anthropic or OpenAI key ready, or your Claude, ChatGPT or Google sign-in.</p>" + PROGRESS_JS))


def r_me(h, _):
    u = h.user()
    if not u:
        return h.json(401, {"error": "signed out"})
    out = []
    steps = {r["server_id"]: r["step"] for r in STORE.all("SELECT server_id, step FROM provision_jobs")}
    for s in [dict(x, step=steps.get(x["id"])) for x in visible_servers(u)]:
        out.append({"id": s["id"], "slug": s["slug"], "url": server_url(s), "status": s["status"], "plan": s["plan"],
                    "role": s["my_role"],
                    "step": "ready" if s["status"] == "ready" else s["step"]})
    h.json(200, {"email": u["email"], "email_verified": bool(u["email_verified"]), "servers": out})


def r_go(h, path):
    u = h.user()
    if not u:
        return h.redirect("/signin")
    sid = path.rsplit("/", 1)[-1]
    srv = next((s for s in visible_servers(u) if s["id"] == sid and s["status"] == "ready"), None)
    if not srv:
        return h.redirect("/")
    STORE.event("cloud.enter", srv["org_id"], server=srv["id"], role=srv["my_role"])
    h.redirect(f"{server_url(srv)}/_door/enter?t={tokens.door_token(srv, u['email'], role=srv['my_role'])}")


def r_webhook(h, _):
    from billing import verify_webhook
    payload = h.body()
    if FAKE:
        event = json.loads(payload)
    else:
        try:
            event = verify_webhook(payload, h.headers.get("Stripe-Signature"), os.environ["STRIPE_WEBHOOK_SECRET"])
        except ValueError as e:
            STORE.event("stripe.webhook_rejected", reason=str(e))
            return h.json(400, {"error": str(e)})
    h.json(200, {"handled": accounts.handle_event(STORE, BILLING, event)})


def _fleet_server(h, path):
    parts = path.strip("/").split("/")          # api/fleet/<server_id>/<verb>
    auth = (h.headers.get("Authorization") or "").removeprefix("Bearer ").strip()
    return fleet.authenticate(STORE, parts[2], auth) if len(parts) == 4 else None, parts[-1]


def r_fleet(h, path):
    srv, verb = _fleet_server(h, path)
    if not srv:
        return h.json(401, {"error": "unknown server or bad fleet token"})
    body = json.loads(h.body() or b"{}")
    if verb == "heartbeat":
        return h.json(200, fleet.heartbeat(STORE, INFRA, srv, body))
    if verb == "invite-email":
        code, out = fleet.invite_email(STORE, srv, body)
        return h.json(code, out)
    h.json(404, {"error": f"unknown fleet verb {verb}"})


def r_notify(h, _):
    """Waitlist (plan step 0). CORS-open to amux.io so the site form can post here once this is live."""
    cors = {"Access-Control-Allow-Origin": SITE}
    try:
        body = json.loads(h.body() or b"{}")
    except ValueError:
        return h.json(400, {"error": "bad json"}, cors)
    email = (body.get("email") or "").strip().lower()
    if "@" not in email or len(email) > 200:
        return h.json(400, {"error": "email required"}, cors)
    STORE.run("INSERT OR IGNORE INTO waitlist(email,source,created_at) VALUES(?,?,?)", email,
              (body.get("source") or "site")[:60], time.time())
    STORE.event("cloud.notify_signup", source=body.get("source"))
    h.json(200, {"ok": True}, cors)


def r_notify_options(h, _):
    h.send(204, b"", "text/plain", {"Access-Control-Allow-Origin": SITE, "Access-Control-Allow-Methods": "POST",
                                    "Access-Control-Allow-Headers": "Content-Type"})


INVITE_COOKIE = "amux_invite"


def r_invite(h, path):
    """8.2: cloud.amux.io/invite/<slug>/<invite>. Sign in first; the door redeems the invite."""
    parts = path.strip("/").split("/")
    if len(parts) != 3:
        return h.send(404, page("Invite", "<h1>This invite link is incomplete</h1>"))
    _, slug, invite = parts
    srv = STORE.one("SELECT * FROM servers WHERE slug=? AND status='ready'", slug)
    if not srv:
        return h.send(404, page("Invite", "<h1>This invite is for a server that is not running</h1>"
                                "<p>Ask the person who invited you for a new link.</p>"))
    u = h.user()
    if not u:
        pending = f"{INVITE_COOKIE}={slug}/{invite}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600" + \
                  ("; Secure" if PUBLIC.startswith("https://") else "")
        return h.redirect("/signin", {"Set-Cookie": pending})
    STORE.run("INSERT OR IGNORE INTO memberships(user_id,server_id,created_at) VALUES(?,?,?)", u["id"], srv["id"], time.time())
    STORE.event("cloud.invite_followed", srv["org_id"], server=srv["id"])
    # The door refuses the invite if this email is not the one it was sent to (T-TEAM-4).
    h.redirect(f"{server_url(srv)}/_door/enter?t={tokens.door_token(srv, u['email'], role='member', invite=invite)}",
               {"Set-Cookie": f"{INVITE_COOKIE}=; Path=/; Max-Age=0"})


def pending_invite(h):
    for part in (h.headers.get("Cookie") or "").split(";"):
        k, _, v = part.strip().partition("=")
        if k == INVITE_COOKIE and "/" in v:
            return v.split("/", 1)
    return None


def r_signin(h, _):
    if FAKE:
        return h.send(200, page("Sign in", "<h1>Sign in (fake mode)</h1><form method=post action=/api/session>"
                                "<input name=email type=email required placeholder='you@example.com'> "
                                "<button class=btn>Sign in</button></form>"))
    pk = os.environ.get("CLERK_PUBLISHABLE_KEY", "")
    h.send(200, page("Sign in", f"""<h1>Sign in to amux Cloud</h1><div id=clerk></div>
<script async crossorigin data-clerk-publishable-key="{html.escape(pk)}"
 src="https://cdn.jsdelivr.net/npm/@clerk/clerk-js@5/dist/clerk.browser.js" onload="go()"></script>
<script>async function go(){{await window.Clerk.load();
 if(window.Clerk.user){{const t=await window.Clerk.session.getToken();
  const r=await fetch('/api/session',{{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{token:t}})}});
  const j=await r.json().catch(()=>({{}}));location=r.ok?(j.next||'/'):'/signin?error=1';return}}
 window.Clerk.mountSignIn(document.getElementById('clerk'),{{afterSignInUrl:'/signin',afterSignUpUrl:'/signin'}})}}</script>"""))


def r_session(h, _):
    raw = h.body()
    if FAKE:
        form = urllib.parse.parse_qs(raw.decode())
        email = (form.get("email") or [""])[0].strip().lower()
        verified = True
    else:
        from clerk import verify_session
        try:
            email, verified = verify_session(json.loads(raw)["token"])
        except Exception as e:
            STORE.event("control.signin_rejected", reason=str(e)[:200])
            return h.json(401, {"error": "sign-in could not be verified"})
    u = STORE.one("SELECT * FROM users WHERE lower(email)=?", email)
    inv = pending_invite(h)
    if not u and inv and verified:
        srv = STORE.one("SELECT * FROM servers WHERE slug=? AND status='ready'", inv[0])
        if srv:
            uid = new_id("usr")
            STORE.run("INSERT INTO users(id,email,email_verified,org_id,created_at,role) VALUES(?,?,1,?,?,'member')",
                      uid, email, srv["org_id"], time.time())
            STORE.event("cloud.member_joined", srv["org_id"], server=srv["id"])
            u = STORE.one("SELECT * FROM users WHERE id=?", uid)
    if u and inv:
        cookie = h.session_cookie(u["id"])
        target = f"/invite/{inv[0]}/{inv[1]}"
        return h.redirect(target, cookie) if FAKE else h.json(200, {"ok": True, "next": target}, cookie)
    if not u:
        # Signing in creates nothing: an account comes from paying (D6) or an invite.
        return (h.redirect(f"{SITE}/pricing/?no_account=1") if FAKE else h.json(404, {"error": "no account", "pricing": f"{SITE}/pricing/"}))
    if verified and not u["email_verified"]:
        STORE.run("UPDATE users SET email_verified=1 WHERE id=?", u["id"])
    cookie = h.session_cookie(u["id"])
    return h.redirect("/", cookie) if FAKE else h.json(200, {"ok": True}, cookie)


def r_signout(h, _):
    h.redirect(f"{SITE}/", {"Set-Cookie": f"{COOKIE}=; Path=/; Max-Age=0" + ("; Secure" if COOKIE.startswith("__Host-") else "")})


def r_billing(h, _):
    u = h.user()
    if not u:
        return h.redirect("/signin")
    if u["role"] != "owner":
        return h.send(403, page("Billing", "<h1>Billing</h1><p>Only the account owner can change billing.</p>"))
    org = STORE.one("SELECT * FROM orgs WHERE id=?", u["org_id"])
    if FAKE or not org["stripe_customer_id"]:
        return h.send(200, page("Billing", "<h1>Billing</h1><p class=dim>No billing portal in this mode.</p>"))
    h.redirect(BILLING.portal_url(org["stripe_customer_id"], f"{PUBLIC}/"))


def r_admin(h, path):
    """Kill switches (plan 11.2) and alerts, behind CONTROL_ADMIN_TOKEN."""
    tok = os.environ.get("CONTROL_ADMIN_TOKEN")
    if not tok or (h.headers.get("Authorization") or "") != f"Bearer {tok}":
        return h.json(401, {"error": "admin token required"})
    if path == "/api/admin/flags" and h.command == "POST":
        body = json.loads(h.body() or b"{}")
        for k, v in body.items():
            if k not in ("lifecycle_freeze", "deploy_freeze", "provisioning_freeze", "spend_freeze", "lifecycle_shadow"):
                return h.json(400, {"error": f"unknown flag {k}"})
            STORE.set_flag(k, "1" if v in (1, "1", True, "true") else "0")
            STORE.event("control.flag", flag=k, value=str(v))
    h.json(200, {"flags": {r["name"]: r["value"] for r in STORE.all("SELECT * FROM flags")},
                 "alerts": STORE.all("SELECT * FROM alerts WHERE acknowledged=0 ORDER BY id DESC LIMIT 50")})


# fake mode only: a stand-in for Stripe Checkout so the journey runs end to end locally
def r_fake_checkout(h, path):
    if not FAKE:
        return h.send(404, "")
    sid = path.rsplit("/", 1)[-1]
    sess = BILLING.sessions.get(sid)
    if not sess:
        return h.send(404, "")
    if h.command == "POST":
        email = urllib.parse.parse_qs(h.body().decode()).get("email", ["buyer@example.com"])[0]
        sub = "sub_" + sid[3:]
        sess.update(status="complete", payment_status="paid", subscription=sub, customer="cus_" + sid[3:],
                    customer_details={"email": email}, amount_total=sess["amount"], payment_intent="pi_" + sid[3:])
        BILLING.subs[sub] = {"id": sub, "status": "active", "current_period_end": time.time() + 30 * 86400}
        return h.redirect(sess["success_url"].replace("{CHECKOUT_SESSION_ID}", sid))
    plan = sess["metadata"]["plan"]
    h.send(200, page("Fake checkout", f"<h1>Fake checkout</h1><p>{plan}: ${sess['amount'] // 100}/month</p><form method=post>"
                     "<input name=email type=email required placeholder=email> <button class=btn id=pay>Pay</button></form>"))


ROUTES = [
    ("GET", "/health", r_health), ("GET", "/", r_root), ("GET", "/start", r_start), ("GET", "/welcome", r_welcome),
    ("GET", "/progress", r_progress), ("GET", "/api/me", r_me), ("GET", "/go/", r_go),
    ("POST", "/webhooks/stripe", r_webhook), ("POST", "/api/fleet/", r_fleet),
    ("POST", "/api/notify", r_notify), ("OPTIONS", "/api/notify", r_notify_options),
    ("GET", "/invite/", r_invite), ("GET", "/signin", r_signin), ("POST", "/api/session", r_session), ("GET", "/signout", r_signout),
    ("GET", "/billing", r_billing), ("GET", "/api/admin/", r_admin), ("POST", "/api/admin/", r_admin),
    ("GET", "/fake/checkout/", r_fake_checkout), ("POST", "/fake/checkout/", r_fake_checkout),
]
Handler.do_OPTIONS = lambda self: self._route("OPTIONS")


def install_fake_checkout(billing):
    def create_checkout(plan_id, signup_id, source, success_url, cancel_url, customer_email=None, customer_id=None):
        sid = "cs_" + signup_id[3:]
        amount = next(p["price_usd"] for p in PLANS["plans"] if p["id"] == plan_id) * 100
        billing.sessions[sid] = {"id": sid, "status": "open", "payment_status": "unpaid", "mode": "subscription",
                                 "metadata": {"plan": plan_id, "source": source}, "amount": amount,
                                 "success_url": success_url, "url": f"{PUBLIC}/fake/checkout/{sid}"}
        return billing.sessions[sid]
    billing.create_checkout = create_checkout


# ------------------------------------------------------------------ background loops
def loop(name, every, fn):
    def run():
        while True:
            try:
                fn()
            except Exception as e:
                traceback.print_exc()
                STORE.event("control.loop_error", loop=name, error=str(e)[:300])
            time.sleep(every)
    threading.Thread(target=run, name=name, daemon=True).start()


def main():
    global STORE, BILLING, INFRA
    if not SECRET and not FAKE:
        sys.exit("CONTROL_SECRET is required")
    STORE = open_store()
    BILLING, INFRA = make_providers()
    if FAKE:
        globals()["SECRET"] = SECRET or "fake-secret"
        install_fake_checkout(BILLING)
    golden = os.environ.get("AMUX_GOLDEN_IMAGE", "debian-12")
    refund = accounts.provisioning_refund(BILLING)
    loop("provision", 5 if FAKE else 15, lambda: provision.tick(STORE, INFRA, PLANS, golden, PUBLIC, refund=refund))
    loop("lifecycle", 3600, lambda: lifecycle.sweep(STORE, BILLING, INFRA))
    loop("refunds", 3600, lambda: accounts.settle_queued_refunds(STORE, BILLING))
    loop("email", 30, lambda: emails.drain(STORE))
    port = int(os.environ.get("CONTROL_PORT", "8090"))
    print(f"[control] listening on :{port} public={PUBLIC} fake={FAKE} prices={PLANS['status']} "
          f"email={'configured' if emails.configured() else 'NOT configured'}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
