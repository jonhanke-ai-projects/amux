"""In-memory providers for control-plane tests. Each can be made unhealthy or
contradictory so the safety rules are exercised, not just the happy path."""
import itertools
from lifecycle import ProviderError


class FakeBilling:
    def __init__(self):
        self.subs = {}          # sub_id -> {"status", "current_period_end", "cancel_at_period_end"}
        self.is_healthy = True
        self.fail_reads = False
        self.reads = 0
        self.refunds = []
        self.disputes = []

    def healthy(self):
        return self.is_healthy

    def get_subscription(self, sub_id):
        self.reads += 1
        if self.fail_reads:
            raise ProviderError("stripe unreachable")
        if sub_id not in self.subs:
            raise ProviderError(f"no such subscription {sub_id}")
        return dict(self.subs[sub_id])

    sessions = None

    def get_checkout_session(self, session_id):
        self.reads += 1
        if self.fail_reads:
            raise ProviderError("stripe unreachable")
        return dict((self.sessions or {})[session_id])

    def close_dispute(self, dispute_id):
        self.disputes.append((dispute_id, "accepted"))
        return {"id": dispute_id, "status": "lost"}

    def refund(self, payment_intent, amount_cents, reason):
        self.refunds.append((payment_intent, amount_cents, reason))
        return {"id": f"re_{len(self.refunds)}"}

    def submit_dispute_evidence(self, dispute_id, evidence):
        self.disputes.append((dispute_id, evidence))
        return {"id": dispute_id, "status": "under_review"}


class FakeInfra:
    def __init__(self):
        self._ids = itertools.count(1000)
        self.servers = {}       # hetzner_id -> {"id", "name", "labels", "location", "server_type", "ipv4"}
        self.images = {}        # image_id -> {"status", "image_size"}
        self.dns = {}           # record_id -> {"name", "content"}
        self.is_healthy = True
        self.snapshot_status = "available"
        self.snapshot_size = 2.5
        self.capacity_errors = set()   # locations that refuse new servers
        self.deleted = []

    def healthy(self):
        return self.is_healthy

    def list_servers(self, label_selector=None):
        out = list(self.servers.values())
        if label_selector:
            k, v = label_selector.split("=")
            out = [s for s in out if s["labels"].get(k) == v]
        return out

    def create_server(self, name, server_type, location, image, labels, user_data):
        if location in self.capacity_errors:
            raise CapacityError(f"resource_unavailable in {location}")
        sid = next(self._ids)
        self.servers[sid] = {"id": sid, "name": name, "labels": dict(labels), "location": location,
                             "server_type": server_type, "ipv4": f"192.0.2.{sid % 250}", "image": image,
                             "user_data": user_data}
        return self.servers[sid]

    def create_snapshot(self, server_id, description):
        if server_id not in self.servers:
            raise RuntimeError(f"server {server_id} not found")
        iid = next(self._ids)
        self.images[iid] = {"id": iid, "status": self.snapshot_status, "image_size": self.snapshot_size,
                            "description": description}
        return iid

    def get_image(self, image_id):
        return self.images.get(int(image_id))

    def delete_server(self, server_id):
        self.servers.pop(server_id)
        self.deleted.append(server_id)

    def delete_image(self, image_id):
        self.images.pop(int(image_id), None)

    def create_dns(self, name, ipv4):
        rid = f"rec{next(self._ids)}"
        self.dns[rid] = {"name": name, "content": ipv4, "proxied": True}
        return rid

    def delete_dns(self, record_id):
        self.dns.pop(record_id, None)

    def origin_certificate(self, hostname, csr_pem):
        return f"-----BEGIN CERTIFICATE-----\nfake for {hostname}\n-----END CERTIFICATE-----\n"


class CapacityError(Exception):
    pass
