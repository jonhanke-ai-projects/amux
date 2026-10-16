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


if __name__ == "__main__":
    unittest.main()
