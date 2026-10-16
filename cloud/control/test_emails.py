"""The outbox sender (emails.drain) against a recorded SMTP server: test and stale mail is
suppressed, real mail is sent once (2026-10-16, before SMTP was turned on for hello@amux.io)."""
import os, tempfile, time, unittest
from unittest import mock

import emails
from store import Store


class FakeSMTP:
    sent = []

    def __init__(self, host, port, timeout=0):
        self.host, self.port = host, port

    def starttls(self): pass
    def login(self, user, pw): self.user = user
    def send_message(self, m): FakeSMTP.sent.append((m["To"], m["Subject"]))
    def __enter__(self): return self
    def __exit__(self, *a): return False


class Drain(unittest.TestCase):
    def setUp(self):
        FakeSMTP.sent = []
        self.store = Store(os.path.join(tempfile.mkdtemp(), "e.db"))
        now = time.time()
        self.store.enqueue_email("alice@customer.com", "server_ready", dedupe="r1", slug="alice")
        self.store.enqueue_email("synthetic+123@amux.io", "being_built", dedupe="s1", slug="synth")
        self.store.enqueue_email("bob@example.com", "being_built", dedupe="e1", slug="bob")
        self.store.enqueue_email("carol@customer.com", "being_built", dedupe="o1", ts=now - 3 * 86400, slug="carol")

    def drain(self):
        with mock.patch.dict(os.environ, {"SMTP_URL": "smtp://hello%40amux.io:pw@smtp.gmail.com:587"}), \
             mock.patch.object(emails.smtplib, "SMTP", FakeSMTP):
            return emails.drain(self.store)

    def test_only_real_current_mail_is_sent(self):
        self.assertEqual(self.drain(), (1, 0))
        self.assertEqual(FakeSMTP.sent, [("alice@customer.com", "Your amux server is ready")])
        why = {r["to_addr"]: r["error"] for r in self.store.all("SELECT * FROM outbox WHERE to_addr!='alice@customer.com'")}
        self.assertEqual(why, {"synthetic+123@amux.io": "suppressed: test address",
                               "bob@example.com": "suppressed: test address",
                               "carol@customer.com": "suppressed: older than two days"})

    def test_a_second_drain_sends_nothing(self):
        self.drain()
        self.assertEqual(self.drain(), (0, 0))
        self.assertEqual(len(FakeSMTP.sent), 1)

    def test_without_smtp_nothing_is_touched(self):
        with mock.patch.dict(os.environ, {"SMTP_URL": ""}):
            self.assertEqual(emails.drain(self.store), (0, 0))
        self.assertEqual(len(self.store.all("SELECT * FROM outbox WHERE sent_at IS NULL")), 4)


class GmailTransport(unittest.TestCase):
    def test_gmail_oauth_sends_through_the_api_as_the_connected_account(self):
        import base64, json
        FakeSMTP.sent = []
        store = Store(os.path.join(tempfile.mkdtemp(), "g.db"))
        store.enqueue_email("alice@customer.com", "server_ready", dedupe="r1", slug="alice")
        blob = base64.b64encode(json.dumps({"client_id": "c", "client_secret": "s", "refresh_token": "r",
                                            "token_uri": "https://oauth2.googleapis.com/token"}).encode()).decode()
        calls = []
        def urlopen(req, timeout=0):
            calls.append(req.full_url)
            body = {"access_token": "at"} if "oauth2" in req.full_url else {"id": "m1"}
            if "messages/send" in req.full_url:
                self.assertEqual(req.headers["Authorization"], "Bearer at")
                raw = base64.urlsafe_b64decode(json.loads(req.data)["raw"]).decode()
                self.assertIn("To: alice@customer.com", raw)
                self.assertIn("Subject: Your amux server is ready", raw)
            class R:
                def __enter__(s): return s
                def __exit__(s, *a): return False
                def read(s): return json.dumps(body).encode()
            return R()
        with mock.patch.dict(os.environ, {"GMAIL_OAUTH": blob, "SMTP_URL": ""}), \
             mock.patch.object(emails.urllib.request, "urlopen", urlopen), \
             mock.patch.object(emails.smtplib, "SMTP", FakeSMTP):
            self.assertEqual(emails.drain(store), (1, 0))
        self.assertEqual(FakeSMTP.sent, [])
        self.assertEqual(calls, ["https://oauth2.googleapis.com/token",
                                 "https://gmail.googleapis.com/gmail/v1/users/me/messages/send"])


if __name__ == "__main__":
    unittest.main()
