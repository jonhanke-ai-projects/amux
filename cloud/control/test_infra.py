"""Bucket deletion against a recorded Cloudflare: the first real account deletion found R2
refuses to delete a non-empty bucket, and that backup tokens were never revoked (2026-10-14)."""
import os, unittest
from unittest import mock

import infra


class FakeCF:
    def __init__(self, objects):
        self.objects = list(objects)
        self.tokens = [{"id": "t1", "name": "amux-backup-b1"}, {"id": "t2", "name": "amux-backup-b1"},
                       {"id": "t3", "name": "amux-backup-other"}]
        self.calls = []

    def call(self, url, token, method="GET", body=None, **kw):
        self.calls.append((method, url.split("/client/v4")[-1]))
        if url.endswith("/r2/buckets/b1") and method == "DELETE":
            if self.objects:
                raise infra.InfraError(409, "bucket not empty")
            return {}
        if "/user/tokens/permission_groups" in url:
            return {"result": [{"id": "w", "name": "Workers R2 Storage Bucket Item Write"},
                               {"id": "r", "name": "Workers R2 Storage Bucket Item Read"}]}
        if url.endswith("/user/tokens") and method == "POST":
            self.tokens.append({"id": "p1", "name": body["name"]})
            return {"result": {"id": "p1", "value": "secret"}}
        if "/user/tokens?" in url:
            return {"result": list(self.tokens)}
        if "/user/tokens/" in url and method == "DELETE":
            self.tokens = [t for t in self.tokens if t["id"] != url.rsplit("/", 1)[1]]
            return {}
        raise AssertionError(f"unexpected call {method} {url}")


class DeleteBucket(unittest.TestCase):
    def run_delete(self, cf, list_keys=None, delete_keys=None):
        i = infra.Infra("", "tok", "")
        with mock.patch.dict(os.environ, {"CF_ACCOUNT_ID": "acct"}), \
             mock.patch.object(infra, "_call", cf.call), \
             mock.patch.object(infra.S3, "list_keys", list_keys or (lambda s, b: list(cf.objects))), \
             mock.patch.object(infra.S3, "delete_keys", delete_keys or (lambda s, b, k: cf.objects.clear())):
            i.delete_bucket("b1")

    def test_non_empty_bucket_is_emptied_then_deleted_and_tokens_revoked(self):
        cf = FakeCF(["restic/data/aa", "litestream/db"])
        self.run_delete(cf)
        self.assertEqual(cf.objects, [])
        self.assertEqual([c for c in cf.calls if c == ("DELETE", "/accounts/acct/r2/buckets/b1")], [("DELETE", "/accounts/acct/r2/buckets/b1")] * 2)
        self.assertEqual([t["name"] for t in cf.tokens], ["amux-backup-other"])   # both backup tokens and the purge token gone

    def test_empty_bucket_deletes_without_a_purge_token(self):
        cf = FakeCF([])
        self.run_delete(cf)
        self.assertNotIn(("POST", "/user/tokens"), cf.calls)
        self.assertEqual([t["name"] for t in cf.tokens], ["amux-backup-other"])

    def test_purge_token_is_revoked_even_when_emptying_fails(self):
        cf = FakeCF(["x"])
        def boom(s, b, k):
            raise infra.InfraError(500, "s3 down")
        with self.assertRaises(infra.InfraError):
            self.run_delete(cf, delete_keys=boom)
        self.assertNotIn("amux-purge-b1", [t["name"] for t in cf.tokens])


class RevokeTokens(unittest.TestCase):
    def test_every_page_is_searched(self):
        pages = {1: [{"id": f"o{i}", "name": "owner-token"} for i in range(50)],
                 2: [{"id": "d1", "name": "amux-drill-x"}, {"id": "d2", "name": "amux-drill-x"}]}
        deleted = []
        def call(url, token, method="GET", body=None, **kw):
            if method == "DELETE":
                deleted.append(url.rsplit("/", 1)[1]); return {}
            page = int(infra.urllib.parse.parse_qs(infra.urllib.parse.urlparse(url).query)["page"][0])
            return {"result": pages[page], "result_info": {"page": page, "total_pages": 2}}
        with mock.patch.object(infra, "_call", call):
            self.assertEqual(infra.Infra("", "tok", "").revoke_tokens("amux-drill-x"), 2)
        self.assertEqual(deleted, ["d1", "d2"])


class S3Signing(unittest.TestCase):
    def test_signature_is_stable_and_scoped(self):
        s3 = infra.S3("acct.r2.cloudflarestorage.com", "KEY", "SECRET")
        seen = {}
        def fake_open(req, timeout=0):
            seen.update(req.headers)
            class R:
                def __enter__(self): return self
                def __exit__(self, *a): return False
                def read(self): return b"<ListBucketResult><Key>a&amp;b</Key></ListBucketResult>"
            return R()
        with mock.patch.object(infra.urllib.request, "urlopen", fake_open):
            self.assertEqual(s3.list_keys("b1"), ["a&b"])
        auth = seen["Authorization"]
        self.assertIn("Credential=KEY/", auth)
        self.assertIn("/auto/s3/aws4_request", auth)
        self.assertRegex(auth, r"Signature=[0-9a-f]{64}$")


if __name__ == "__main__":
    unittest.main()
