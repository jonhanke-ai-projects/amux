#!/usr/bin/env python3
"""amux-door: the sign-in in front of a dedicated amux host.

Caddy asks this service about every request (`forward_auth`). A request passes
with either a signed `amux_door` cookie (browsers, after the sign-in form) or
HTTP Basic credentials (the amux CLI, remote.py, scripts). Either way the
answer carries `X-Door-User: <username>`, which Caddy maps to that person's
amux member cookie, so the amux identity is the account that signed in.

Why a form at all: a browser can carry Basic auth only through its own modal,
and a URL with embedded credentials breaks the dashboard (Chromium and WebKit
both refuse every fetch from such a page; tested 2026-10-06). The form lets
another amux's Settings > Connect > Connections sign in with a stored
username and password by posting here, top level, then landing on the
dashboard.

Stdlib only. Users live in USERS_FILE as `user scrypt$<salt>$<hash>` lines,
written by `door.py hash`. The cookie is `user.expiry.hmac` under a random
per-host secret; rotating the secret signs everyone out.

  door.py serve                 listen on 127.0.0.1:8890
  door.py hash <user>           read a password on stdin, print a users line
"""
import base64
import hashlib
import hmac
import html
import json
import os
import secrets
import sys
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

USERS_FILE = os.environ.get("DOOR_USERS", "/etc/amux-door/users")
SECRET_FILE = os.environ.get("DOOR_SECRET", "/etc/amux-door/secret")
COOKIE = "amux_door"
# File previews run in a sandboxed iframe (an opaque origin), so their link
# clicks are cross-site and never carry a SameSite=Lax cookie. This second,
# read-only cookie is SameSite=None and valid ONLY for GET under VIEW_PATH,
# the path-style file route the dashboard points previews at.
VIEW_COOKIE = "amux_door_view"
VIEW_PATH = "/api/file/raw-path/"
# The view cookie is scoped to VIEW_PATH, so it is never sent for "/". This
# Path=/ marker records that it was issued; without it the refresh hop below
# would fire on every page load.
VIEW_MARK = "amux_door_vm"
MAX_AGE = 30 * 86400
PORT = int(os.environ.get("DOOR_PORT", "8890"))


def log(**kv):
    kv["ts"] = round(time.time(), 3)
    print(json.dumps(kv), flush=True)


def scrypt_hash(password, salt=None):
    salt = salt or secrets.token_bytes(16)
    h = hashlib.scrypt(password.encode(), salt=salt, n=2**14, r=8, p=1, dklen=32)
    return "scrypt$%s$%s" % (base64.b64encode(salt).decode(), base64.b64encode(h).decode())


def users():
    out = {}
    try:
        for line in open(USERS_FILE):
            parts = line.split()
            if len(parts) == 2:
                out[parts[0]] = parts[1]
    except FileNotFoundError:
        pass
    return out


def check_password(user, password):
    stored = users().get(user)
    if not stored or not stored.startswith("scrypt$"):
        # Same work either way so a timing difference does not reveal users.
        scrypt_hash(password)
        return False
    _, salt, _ = stored.split("$", 2)
    return hmac.compare_digest(scrypt_hash(password, base64.b64decode(salt)), stored)


def secret():
    with open(SECRET_FILE, "rb") as f:
        return f.read().strip()


def sign(user, expiry):
    msg = ("%s.%d" % (user, expiry)).encode()
    return hmac.new(secret(), msg, hashlib.sha256).hexdigest()


def make_cookie(user):
    exp = int(time.time()) + MAX_AGE
    return "%s.%d.%s" % (user, exp, sign(user, exp))


def cookie_user(header, name=COOKIE):
    for part in (header or "").split(";"):
        k, _, v = part.strip().partition("=")
        if k == name and v:
            try:
                user, exp, mac = v.rsplit(".", 2)
                exp = int(exp)
            except ValueError:
                return None
            if exp > time.time() and hmac.compare_digest(sign(user, exp), mac) and user in users():
                return user
    return None


def basic_user(header):
    if not header or not header.startswith("Basic "):
        return None
    try:
        user, _, pw = base64.b64decode(header[6:]).decode().partition(":")
    except Exception:
        return None
    return user if check_password(user, pw) else None


def session_cookies(user):
    v = make_cookie(user)
    return [("Set-Cookie", "%s=%s; Path=/; Max-Age=%d; HttpOnly; Secure; SameSite=Lax" % (COOKIE, v, MAX_AGE)),
            ("Set-Cookie", "%s=%s; Path=%s; Max-Age=%d; HttpOnly; Secure; SameSite=None" % (VIEW_COOKIE, v, VIEW_PATH, MAX_AGE)),
            ("Set-Cookie", "%s=1; Path=/; Max-Age=%d; HttpOnly; Secure; SameSite=Lax" % (VIEW_MARK, MAX_AGE))]


def has_cookie(header, name):
    return any(p.strip().partition("=")[0] == name for p in (header or "").split(";"))


def safe_next(n):
    # Only a same-host path: never an open redirect.
    n = n or "/"
    return n if n.startswith("/") and not n.startswith("//") else "/"


PAGE = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>Sign in to amux</title>
<style>
body{margin:0;font:16px -apple-system,system-ui,sans-serif;background:#0d1117;color:#e6edf3;display:flex;min-height:100vh;align-items:center;justify-content:center}
form{background:#161b22;border:1px solid #30363d;border-radius:12px;padding:28px;width:min(340px,90vw)}
h1{font-size:20px;margin:0 0 4px}p{color:#8b949e;font-size:13px;margin:0 0 18px}
label{display:block;font-size:13px;color:#8b949e;margin:12px 0 4px}
input{width:100%%;box-sizing:border-box;min-height:44px;padding:10px;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#e6edf3;font-size:16px}
button{margin-top:18px;width:100%%;min-height:44px;border:0;border-radius:8px;background:#58a6ff;color:#0d1117;font-weight:600;font-size:16px}
.err{color:#f85149;font-size:13px;margin-top:10px}
</style></head><body>
<form method="post" action="/_door/login">
<h1>amux</h1><p>%(host)s</p>
<label for="u">Username</label><input id="u" name="username" autocomplete="username" autocapitalize="none" required value="%(user)s">
<label for="p">Password</label><input id="p" name="password" type="password" autocomplete="current-password" required>
<input type="hidden" name="next" value="%(next)s">
<button type="submit">Sign in</button>%(err)s
</form></body></html>"""


class Door(BaseHTTPRequestHandler):
    server_version = "amux-door"

    def log_message(self, *a):
        pass

    def send(self, code, body=b"", headers=()):
        self.send_response(code)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def page(self, nxt, user="", err=""):
        host = html.escape(self.headers.get("X-Forwarded-Host") or self.headers.get("Host") or "")
        body = PAGE % {"host": host, "next": html.escape(safe_next(nxt)), "user": html.escape(user),
                       "err": '<div class="err">%s</div>' % html.escape(err) if err else ""}
        self.send(401 if err else 200, body.encode(), [("Content-Type", "text/html; charset=utf-8"),
                                                       ("Cache-Control", "no-store")])

    def do_GET(self):
        path = urllib.parse.urlsplit(self.path)
        if path.path == "/_door/check":
            # forward_auth: the original request's method/URI arrive as headers.
            uri = self.headers.get("X-Forwarded-Uri", "/")
            method = self.headers.get("X-Forwarded-Method", "GET")
            cookies = self.headers.get("Cookie")
            user = cookie_user(cookies) or basic_user(self.headers.get("Authorization"))
            if not user and method == "GET" and uri.startswith(VIEW_PATH):
                user = cookie_user(cookies, VIEW_COOKIE)
            if user:
                # Signed in before the view cookie existed: one hop to add it.
                if (method == "GET" and "text/html" in self.headers.get("Accept", "")
                        and not has_cookie(cookies, VIEW_MARK) and not uri.startswith(("/_door/", VIEW_PATH))):
                    loc = "/_door/refresh?next=" + urllib.parse.quote(safe_next(uri), safe="")
                    return self.send(302, headers=[("Location", loc), ("Cache-Control", "no-store")])
                return self.send(200, headers=[("X-Door-User", user)])
            accept = self.headers.get("Accept", "")
            if self.headers.get("X-Forwarded-Method", "GET") == "GET" and "text/html" in accept:
                loc = "/_door/login?next=" + urllib.parse.quote(safe_next(uri), safe="")
                return self.send(302, headers=[("Location", loc)])
            return self.send(401, b'{"error":"sign in required"}', [("Content-Type", "application/json"),
                             ("WWW-Authenticate", 'Basic realm="amux"')])
        if path.path == "/_door/login":
            q = urllib.parse.parse_qs(path.query)
            return self.page(q.get("next", ["/"])[0])
        if path.path == "/_door/refresh":
            q = urllib.parse.parse_qs(path.query)
            nxt = safe_next(q.get("next", ["/"])[0])
            user = cookie_user(self.headers.get("Cookie"))
            if not user:
                return self.send(302, headers=[("Location", "/_door/login?next=" + urllib.parse.quote(nxt, safe=""))])
            return self.send(302, headers=[("Location", nxt), ("Cache-Control", "no-store")] + session_cookies(user))
        if path.path == "/_door/logout":
            return self.send(302, headers=[("Location", "/_door/login"),
                             ("Set-Cookie", "%s=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax" % COOKIE),
                             ("Set-Cookie", "%s=; Path=%s; Max-Age=0; HttpOnly; Secure; SameSite=None" % (VIEW_COOKIE, VIEW_PATH)),
                             ("Set-Cookie", "%s=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax" % VIEW_MARK)])
        self.send(404)

    def do_POST(self):
        if urllib.parse.urlsplit(self.path).path != "/_door/login":
            return self.send(404)
        n = int(self.headers.get("Content-Length") or 0)
        form = urllib.parse.parse_qs(self.rfile.read(min(n, 65536)).decode(errors="replace"))
        user = (form.get("username") or [""])[0].strip()
        pw = (form.get("password") or [""])[0]
        nxt = safe_next((form.get("next") or ["/"])[0])
        ip = self.headers.get("X-Forwarded-For", self.client_address[0])
        if check_password(user, pw):
            log(event="signin", user=user, ip=ip, verdict="door_signin_ok")
            return self.send(303, headers=[("Location", nxt), ("Cache-Control", "no-store")] + session_cookies(user))
        log(event="signin", user=user, ip=ip, verdict="door_signin_refused")
        time.sleep(1.0)  # slows guessing without a lockout a person could trip
        return self.page(nxt, user, "Wrong username or password.")


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "hash":
        print("%s %s" % (sys.argv[2], scrypt_hash(sys.stdin.readline().rstrip("\n"))))
        return
    if len(sys.argv) >= 2 and sys.argv[1] == "serve":
        log(event="start", port=PORT, users=len(users()), verdict="door_started")
        ThreadingHTTPServer(("127.0.0.1", PORT), Door).serve_forever()
        return
    sys.exit(__doc__)


if __name__ == "__main__":
    main()
