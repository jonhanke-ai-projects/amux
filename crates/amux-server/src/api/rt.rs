//! `/api/rt`: realtime rooms, the reusable multiplayer protocol.
//!
//! One WebSocket per browser tab multiplexes any number of ROOMS, each keyed by
//! the resource it is about (`file:<absolute path>` today; `worker:<name>` and
//! `card:<id>` are the shapes the key leaves room for). A feature that wants
//! live collaboration joins a room, sends opaque `op` payloads and `presence`,
//! and receives everyone else's. Design: `docs/design/realtime-rooms.md`.
//!
//! WHAT THE SERVER DOES, and only this:
//! - relays, stamping a server-verified actor on every message (the client's
//!   own claim of who it is never survives: [`super::typing::actor`] decides);
//! - orders: a per-room monotonic `seq` on every op, and a per-recipient
//!   `prev` link so a client can tell a gap from a coalesced skip;
//! - checks permission on join (read) and on op/snapshot (write) with the file
//!   API's own path rules (`fs::is_path_allowed`, `fs::is_dangerous_write`,
//!   and a scoped member's grant);
//! - elects one SAVER per room (the first writer to join; handed off when it
//!   leaves), so only one tab autosaves a shared file;
//! - bounds every client's outbound queue, coalescing keyed messages (newest
//!   wins) and, past a hard cap, dropping the oldest ops and telling the client
//!   so it resyncs from a peer's snapshot.
//!
//! WHAT IT DOES NOT DO: own document content. Rooms live in process memory
//! only, like typing presence (api/typing.rs, ethos rule 5): a deploy drops
//! every room, clients reconnect and resync, and the durable truth stays the
//! file on disk. This is not a CRDT and not a store.
//!
//! Fallback transport: `GET /api/rt/events` (SSE down) + `POST /api/rt/send`
//! (batched up) carry the same messages for a network that blocks WebSocket
//! upgrades. Same hub, same guarantees, lower rate.
//!
//! Observability: `GET /api/debug/rt` (rooms, clients, ops/s, coalesced and
//! dropped counts, resyncs, client-reported latency and fps percentiles, and
//! the client beacons of kind `rt`), plus WARN lines on refusals and drops.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};

use super::org::MemberScope;
use super::AppState;

/// Wire protocol version, sent in `hello`.
pub const PROTOCOL: u32 = 1;
/// Queued messages a client may have before the oldest ops are dropped.
const HARD_CAP_ITEMS: usize = 2048;
/// Queued bytes a client may have before the oldest ops are dropped.
const HARD_CAP_BYTES: usize = 24 * 1024 * 1024;
/// Largest single inbound message (a snapshot of a big drawing with images).
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ROOMS_PER_CLIENT: usize = 32;
const MAX_ROOM_KEY: usize = 4096;
const LATENCY_RING: usize = 512;

const COLORS: &[&str] = &[
    "#e03131", "#1971c2", "#2f9e44", "#f08c00", "#9c36b5", "#0c8599", "#d6336c", "#5c940d",
];

// ───────────────────────────── outbound queue ─────────────────────────────

/// One message waiting to go to one client.
#[derive(Debug, Clone)]
struct Item {
    room: Option<String>,
    /// Coalesce key: a newer item with the same (room, key) replaces this one
    /// while it is still queued.
    ck: Option<String>,
    /// Room seq for ops (the gap-detection chain). None for state messages.
    seq: Option<u64>,
    /// The seq of the previous chain message THIS client was due in this room.
    prev: Option<u64>,
    body: Map<String, Value>,
    bytes: usize,
}

impl Item {
    fn state(room: Option<&str>, ck: Option<String>, body: Value) -> Self {
        Self::new(room, ck, None, body)
    }
    fn new(room: Option<&str>, ck: Option<String>, seq: Option<u64>, body: Value) -> Self {
        let body = match body {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("data".into(), other);
                m
            }
        };
        let bytes = body.get("data").map(|d| d.to_string().len()).unwrap_or(0) + 128;
        Self { room: room.map(str::to_string), ck, seq, prev: None, body, bytes }
    }
    fn render(&self) -> String {
        let mut m = self.body.clone();
        if let Some(s) = self.seq {
            m.insert("seq".into(), json!(s));
        }
        if let Some(p) = self.prev {
            m.insert("prev".into(), json!(p));
        }
        Value::Object(m).to_string()
    }
}

#[derive(Default)]
struct OutQ {
    items: VecDeque<Item>,
    bytes: usize,
    /// Per room: the seq of the last chain item enqueued for this client.
    last_chain: HashMap<String, u64>,
}

/// A client's bounded outbound queue. Producers push under the hub lock; the
/// transport task drains it and writes to the socket. A slow socket leaves
/// items queued, which is exactly when coalescing has something to do.
pub(crate) struct Outbox {
    q: Mutex<OutQ>,
    notify: tokio::sync::Notify,
    closed: AtomicBool,
    coalesced: AtomicU64,
    dropped: AtomicU64,
    sent: AtomicU64,
}

/// What one push did, for counters and tests.
#[derive(Debug, Default, PartialEq, Eq)]
struct PushOutcome {
    coalesced: u64,
    dropped: u64,
}

impl Outbox {
    fn new() -> Self {
        Self {
            q: Mutex::new(OutQ::default()),
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            coalesced: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            sent: AtomicU64::new(0),
        }
    }

    /// Start (or restart) a room's chain at `seq`: the next op this client
    /// receives in the room carries `prev = seq`.
    fn reset_chain(&self, room: &str, seq: u64) {
        let mut q = self.q.lock().unwrap_or_else(|p| p.into_inner());
        q.last_chain.insert(room.to_string(), seq);
    }

    fn forget_room(&self, room: &str) {
        let mut q = self.q.lock().unwrap_or_else(|p| p.into_inner());
        q.last_chain.remove(room);
        let before = q.items.len();
        q.items.retain(|i| i.room.as_deref() != Some(room) || i.body.get("type") == Some(&json!("left")));
        if q.items.len() != before {
            q.bytes = q.items.iter().map(|i| i.bytes).sum();
        }
    }

    fn push(&self, mut item: Item) -> PushOutcome {
        let mut out = PushOutcome::default();
        if self.closed.load(Ordering::Relaxed) {
            return out;
        }
        {
            let mut q = self.q.lock().unwrap_or_else(|p| p.into_inner());
            if let (Some(room), Some(seq)) = (item.room.clone(), item.seq) {
                item.prev = q.last_chain.get(&room).copied();
                q.last_chain.insert(room, seq);
            }
            q.bytes += item.bytes;
            let new_room = item.room.clone();
            let new_ck = item.ck.clone();
            q.items.push_back(item);
            // Coalesce: drop an OLDER queued item with the same (room, key).
            if let Some(ck) = new_ck {
                let last = q.items.len() - 1;
                if let Some(i) = q
                    .items
                    .iter()
                    .take(last)
                    .position(|x| x.room == new_room && x.ck.as_deref() == Some(ck.as_str()))
                {
                    let removed = q.items.remove(i).expect("index in range");
                    q.bytes -= removed.bytes;
                    out.coalesced += 1;
                    // Splice the chain: the next chain item of that room now
                    // follows whatever the removed one followed, so a skipped
                    // (superseded) op never reads as a gap.
                    if removed.seq.is_some() {
                        if let Some(next) = q
                            .items
                            .iter_mut()
                            .skip(i)
                            .find(|x| x.room == removed.room && x.seq.is_some())
                        {
                            next.prev = removed.prev;
                        }
                    }
                }
            }
            // Hard cap: drop the OLDEST ops (never a state message if an op can
            // go instead). The chain is deliberately NOT spliced: the client
            // sees the gap and asks for a snapshot, and a `gap` notice tells it
            // even if no further op arrives.
            let mut gapped: HashSet<String> = HashSet::new();
            while q.items.len() > HARD_CAP_ITEMS || q.bytes > HARD_CAP_BYTES {
                let idx = q.items.iter().position(|x| x.seq.is_some()).unwrap_or(0);
                let Some(gone) = q.items.remove(idx) else { break };
                q.bytes -= gone.bytes;
                out.dropped += 1;
                if let (Some(room), true) = (gone.room.clone(), gone.seq.is_some()) {
                    gapped.insert(room);
                }
            }
            for room in gapped {
                let notice = Item::state(
                    Some(&room),
                    Some("gap".into()),
                    json!({"type": "gap", "room": room}),
                );
                q.bytes += notice.bytes;
                q.items.retain(|x| !(x.room.as_deref() == Some(room.as_str()) && x.ck.as_deref() == Some("gap")));
                q.items.push_back(notice);
            }
        }
        if out.coalesced > 0 {
            self.coalesced.fetch_add(out.coalesced, Ordering::Relaxed);
        }
        if out.dropped > 0 {
            self.dropped.fetch_add(out.dropped, Ordering::Relaxed);
        }
        self.notify.notify_one();
        out
    }

    fn drain(&self) -> Vec<String> {
        let mut q = self.q.lock().unwrap_or_else(|p| p.into_inner());
        q.bytes = 0;
        let out: Vec<String> = q.items.drain(..).map(|i| i.render()).collect();
        self.sent.fetch_add(out.len() as u64, Ordering::Relaxed);
        out
    }

    fn queued(&self) -> usize {
        self.q.lock().map(|q| q.items.len()).unwrap_or(0)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// Wait until there is something to send (or the outbox closed).
    async fn next_batch(&self) -> Option<Vec<String>> {
        loop {
            if self.closed.load(Ordering::Relaxed) {
                return None;
            }
            let batch = self.drain();
            if !batch.is_empty() {
                return Some(batch);
            }
            self.notify.notified().await;
        }
    }
}

// ───────────────────────────── identity + permission ─────────────────────────────

/// Who a connection is, decided from server-verified headers at connect time.
#[derive(Debug, Clone)]
pub(crate) struct Ident {
    pub actor: String,
    /// A scoped invited member's grant; None for the owner, a worker lane, or
    /// any caller the auth layer already admitted as unscoped.
    pub scope: Option<MemberScope>,
}

impl Ident {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        Self { actor: super::typing::actor(headers), scope: super::org::local_member_scope(headers) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
}

/// Parse and normalise a room key. Only `file:` rooms exist today; the path
/// is resolved the way the file API resolves it, so two spellings of one file
/// land in one room.
pub(crate) fn normalize_room(room: &str) -> Result<String, &'static str> {
    if room.len() > MAX_ROOM_KEY {
        return Err("room key too long");
    }
    let Some(path) = room.strip_prefix("file:") else {
        return Err("unknown room kind (only file:<absolute path> is supported)");
    };
    if !path.starts_with('/') {
        return Err("file rooms need an absolute path");
    }
    let resolved = super::fs::resolve_nonstrict(Path::new(path));
    Ok(format!("file:{}", resolved.to_string_lossy()))
}

/// The file API's rules applied to a room: read to watch, write to change.
pub(crate) fn room_access(ident: &Ident, room: &str) -> Result<Access, &'static str> {
    let path = room.strip_prefix("file:").ok_or("unknown room kind")?;
    let p = Path::new(path);
    if !super::fs::is_path_allowed(p) {
        return Err("access denied");
    }
    match &ident.scope {
        None => {}
        Some(scope) if scope.is_global() => {}
        Some(scope) => {
            // A scoped member reads files under their workers' folders and
            // writes none (the same rule /api/file applies to them).
            let dirs = super::org::scoped_worker_dirs(scope);
            let target = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            if !dirs.iter().any(|d| target.starts_with(d)) {
                return Err("outside your workers' folders");
            }
            return Ok(Access::Read);
        }
    }
    if super::fs::is_dangerous_write(p) {
        return Ok(Access::Read);
    }
    Ok(Access::Write)
}

// ───────────────────────────── the hub ─────────────────────────────

struct Client {
    ident: Ident,
    label: String,
    color: &'static str,
    transport: &'static str,
    out: Arc<Outbox>,
    rooms: HashMap<String, Access>,
    opened: Instant,
}

#[derive(Default)]
struct Room {
    seq: u64,
    /// Join order; the saver is the first writer in it.
    members: Vec<u64>,
    saver: Option<u64>,
    ops: u64,
}

#[derive(Default)]
struct Stats {
    opened_ws: u64,
    opened_sse: u64,
    ops: u64,
    presence: u64,
    snapshots: u64,
    resyncs: u64,
    refused: u64,
    coalesced: u64,
    dropped: u64,
    saver_handoffs: u64,
    op_times: VecDeque<Instant>,
    latency_p50: VecDeque<f64>,
    latency_p95: VecDeque<f64>,
    fps: VecDeque<f64>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    clients: HashMap<u64, Client>,
    rooms: HashMap<String, Room>,
    stats: Stats,
}

#[derive(Default)]
pub(crate) struct Hub {
    inner: Mutex<Inner>,
}

pub(crate) fn hub() -> &'static Hub {
    static H: OnceLock<Hub> = OnceLock::new();
    H.get_or_init(Hub::default)
}

fn from_of(cid: u64, c: &Client) -> Value {
    json!({"cid": cid, "actor": c.ident.actor, "label": c.label, "color": c.color})
}

impl Inner {
    fn send(&mut self, cid: u64, item: Item) {
        if let Some(c) = self.clients.get(&cid) {
            let o = c.out.push(item);
            self.stats.coalesced += o.coalesced;
            if o.dropped > 0 {
                self.stats.dropped += o.dropped;
                tracing::warn!(cid, dropped = o.dropped, verdict = "rt_queue_overflow_dropped",
                    "realtime client fell behind its queue cap: oldest ops dropped, client told to resync");
            }
        }
    }

    fn error(&mut self, cid: u64, room: Option<&str>, code: &str, msg: &str) {
        self.stats.refused += 1;
        let actor = self.clients.get(&cid).map(|c| c.ident.actor.clone()).unwrap_or_default();
        tracing::warn!(cid, actor = %actor, room = room.unwrap_or(""), code, verdict = "rt_refused", "realtime message refused: {msg}");
        self.send(cid, Item::state(room, None, json!({"type": "error", "room": room, "code": code, "error": msg})));
    }

    fn members_msg(&self, room: &str) -> Value {
        let r = &self.rooms[room];
        let members: Vec<Value> = r
            .members
            .iter()
            .filter_map(|cid| {
                let c = self.clients.get(cid)?;
                let mut v = from_of(*cid, c);
                v["can_write"] = json!(c.rooms.get(room) == Some(&Access::Write));
                Some(v)
            })
            .collect();
        json!({"type": "members", "room": room, "saver": r.saver, "seq": r.seq, "members": members})
    }

    fn broadcast_members(&mut self, room: &str) {
        let msg = self.members_msg(room);
        let targets = self.rooms[room].members.clone();
        for t in targets {
            self.send(t, Item::state(Some(room), Some("members".into()), msg.clone()));
        }
    }

    /// Pick a writer peer (the saver first) to answer a snapshot request.
    fn snapshot_source(&self, room: &str, not: u64) -> Option<u64> {
        let r = self.rooms.get(room)?;
        let is_writer = |cid: &u64| {
            self.clients.get(cid).and_then(|c| c.rooms.get(room)) == Some(&Access::Write)
        };
        r.saver
            .filter(|s| *s != not && is_writer(s))
            .or_else(|| r.members.iter().copied().find(|m| *m != not && is_writer(m)))
    }

    fn request_snapshot(&mut self, room: &str, for_cid: u64, why: &str) {
        self.stats.resyncs += 1;
        match self.snapshot_source(room, for_cid) {
            Some(src) => {
                let msg = json!({"type": "snapshot_request", "room": room, "for": for_cid, "why": why});
                self.send(src, Item::state(Some(room), Some(format!("snapreq:{for_cid}")), msg));
            }
            None => {
                // Nobody else holds the document: the durable truth is the file.
                let seq = self.rooms.get(room).map(|r| r.seq).unwrap_or(0);
                if let Some(c) = self.clients.get(&for_cid) {
                    c.out.reset_chain(room, seq);
                }
                let msg = json!({"type": "snapshot", "room": room, "source": "disk", "base": seq, "data": null});
                self.send(for_cid, Item::state(Some(room), Some("snapshot".into()), msg));
            }
        }
    }

    fn leave(&mut self, cid: u64, room: &str, why: &str) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if c.rooms.remove(room).is_none() {
            return;
        }
        c.out.forget_room(room);
        let Some(r) = self.rooms.get_mut(room) else { return };
        r.members.retain(|m| *m != cid);
        if r.saver == Some(cid) {
            let clients = &self.clients;
            r.saver = r
                .members
                .iter()
                .copied()
                .find(|m| clients.get(m).and_then(|c| c.rooms.get(room)) == Some(&Access::Write));
            self.stats.saver_handoffs += 1;
            tracing::info!(room, from = cid, to = ?r.saver, why, verdict = "rt_saver_handoff", "realtime room saver handed off");
        }
        if r.members.is_empty() {
            self.rooms.remove(room);
        } else {
            self.broadcast_members(room);
        }
    }
}

impl Hub {
    pub(crate) fn connect(&self, ident: Ident, transport: &'static str) -> (u64, Arc<Outbox>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.next += 1;
        let cid = g.next;
        let out = Arc::new(Outbox::new());
        let label = super::typing::label(&ident.actor);
        let color = COLORS[(cid as usize) % COLORS.len()];
        match transport {
            "ws" => g.stats.opened_ws += 1,
            _ => g.stats.opened_sse += 1,
        }
        let hello = json!({"type": "hello", "protocol": PROTOCOL, "cid": cid, "actor": ident.actor,
            "label": label, "color": color, "transport": transport});
        g.clients.insert(cid, Client { ident, label, color, transport, out: out.clone(), rooms: HashMap::new(), opened: Instant::now() });
        g.send(cid, Item::state(None, None, hello));
        (cid, out)
    }

    pub(crate) fn disconnect(&self, cid: u64) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let rooms: Vec<String> = g.clients.get(&cid).map(|c| c.rooms.keys().cloned().collect()).unwrap_or_default();
        for room in rooms {
            g.leave(cid, &room, "disconnect");
        }
        if let Some(c) = g.clients.remove(&cid) {
            c.out.close();
        }
    }

    pub(crate) fn actor_of(&self, cid: u64) -> Option<String> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.clients.get(&cid).map(|c| c.ident.actor.clone())
    }

    /// Handle one inbound message from client `cid`.
    pub(crate) fn handle(&self, cid: u64, text: &str) {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
                g.error(cid, None, "bad_json", "message was not JSON");
                return;
            }
        };
        self.handle_value(cid, msg);
    }

    pub(crate) fn handle_value(&self, cid: u64, msg: Value) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !g.clients.contains_key(&cid) {
            return;
        }
        let kind = msg.get("t").or_else(|| msg.get("type")).and_then(Value::as_str).unwrap_or("").to_string();
        if kind == "ping" {
            g.send(cid, Item::state(None, Some("pong".into()), json!({"type": "pong", "t": msg.get("ts")})));
            return;
        }
        if kind == "stats" {
            let push = |ring: &mut VecDeque<f64>, v: Option<f64>| {
                if let Some(v) = v.filter(|v| v.is_finite() && *v >= 0.0) {
                    ring.push_back(v);
                    while ring.len() > LATENCY_RING {
                        ring.pop_front();
                    }
                }
            };
            push(&mut g.stats.latency_p50, msg.get("lat_p50").and_then(Value::as_f64));
            push(&mut g.stats.latency_p95, msg.get("lat_p95").and_then(Value::as_f64));
            push(&mut g.stats.fps, msg.get("fps").and_then(Value::as_f64));
            return;
        }
        let raw_room = msg.get("room").and_then(Value::as_str).unwrap_or("");
        let room = match normalize_room(raw_room) {
            Ok(r) => r,
            Err(e) => {
                g.error(cid, Some(raw_room), "bad_room", e);
                return;
            }
        };
        match kind.as_str() {
            "join" => {
                let ident = g.clients[&cid].ident.clone();
                let access = match room_access(&ident, &room) {
                    Ok(a) => a,
                    Err(e) => {
                        g.error(cid, Some(&room), "forbidden", e);
                        return;
                    }
                };
                if g.clients[&cid].rooms.len() >= MAX_ROOMS_PER_CLIENT && !g.clients[&cid].rooms.contains_key(&room) {
                    g.error(cid, Some(&room), "too_many_rooms", "a client may join at most 32 rooms");
                    return;
                }
                if g.clients[&cid].rooms.contains_key(&room) {
                    // A re-join resyncs in place.
                    g.leave(cid, &room, "rejoin");
                }
                g.clients.get_mut(&cid).expect("present").rooms.insert(room.clone(), access);
                let r = g.rooms.entry(room.clone()).or_default();
                r.members.push(cid);
                if r.saver.is_none() && access == Access::Write {
                    r.saver = Some(cid);
                }
                let seq = r.seq;
                let peers = r.members.len() - 1;
                g.clients[&cid].out.reset_chain(&room, seq);
                let you = from_of(cid, &g.clients[&cid]);
                g.send(cid, Item::state(Some(&room), None, json!({"type": "joined", "room": room, "asked": raw_room, "seq": seq,
                    "you": you, "can_write": access == Access::Write, "peers": peers})));
                g.broadcast_members(&room);
                if peers > 0 {
                    g.request_snapshot(&room, cid, "join");
                }
            }
            "leave" => {
                g.leave(cid, &room, "leave");
                g.send(cid, Item::state(Some(&room), None, json!({"type": "left", "room": room})));
            }
            "presence" | "op" | "snapshot" | "snapshot_request" => {
                let Some(access) = g.clients[&cid].rooms.get(&room).copied() else {
                    g.error(cid, Some(&room), "not_joined", "join the room first");
                    return;
                };
                let from = from_of(cid, &g.clients[&cid]);
                let data = msg.get("data").cloned().unwrap_or(Value::Null);
                match kind.as_str() {
                    "presence" => {
                        g.stats.presence += 1;
                        let body = json!({"type": "presence", "room": room, "from": from, "data": data});
                        let peers: Vec<u64> = g.rooms[&room].members.iter().copied().filter(|m| *m != cid).collect();
                        for p in peers {
                            g.send(p, Item::state(Some(&room), Some(format!("p:{cid}")), body.clone()));
                        }
                    }
                    "snapshot_request" => {
                        g.request_snapshot(&room, cid, msg.get("why").and_then(Value::as_str).unwrap_or("gap"));
                    }
                    _ if access != Access::Write => {
                        g.error(cid, Some(&room), "read_only", "you can watch this file but not change it");
                    }
                    "op" => {
                        let r = g.rooms.get_mut(&room).expect("joined room exists");
                        r.seq += 1;
                        r.ops += 1;
                        let seq = r.seq;
                        let peers: Vec<u64> = r.members.iter().copied().filter(|m| *m != cid).collect();
                        g.stats.ops += 1;
                        let now = Instant::now();
                        g.stats.op_times.push_back(now);
                        while g.stats.op_times.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(10)) || g.stats.op_times.len() > 20_000 {
                            g.stats.op_times.pop_front();
                        }
                        let ck = msg.get("ck").and_then(Value::as_str).map(|k| format!("op:{cid}:{k}"));
                        let body = json!({"type": "op", "room": room, "from": from, "data": data});
                        for p in peers {
                            g.send(p, Item::new(Some(&room), ck.clone(), Some(seq), body.clone()));
                        }
                        let id = msg.get("id").cloned().unwrap_or(Value::Null);
                        g.send(cid, Item::state(Some(&room), Some("ack".into()), json!({"type": "ack", "room": room, "seq": seq, "id": id})));
                    }
                    _ => {
                        // snapshot: a writer answering a request for client `for`.
                        let Some(target) = msg.get("for").and_then(Value::as_u64) else {
                            g.error(cid, Some(&room), "bad_snapshot", "snapshot needs a `for` client id");
                            return;
                        };
                        if !g.rooms[&room].members.contains(&target) {
                            return; // the requester left meanwhile
                        }
                        g.stats.snapshots += 1;
                        let base = g.rooms[&room].seq;
                        // The snapshot restarts the target's chain at `base`:
                        // ops after it link to it, ops before it were already
                        // queued ahead of it.
                        g.clients[&target].out.reset_chain(&room, base);
                        let body = json!({"type": "snapshot", "room": room, "source": "peer", "from": from, "base": base, "data": data});
                        g.send(target, Item::state(Some(&room), Some("snapshot".into()), body));
                    }
                }
            }
            other => {
                let msg = format!("unknown message type {other:?}");
                g.error(cid, Some(&room), "bad_type", &msg);
            }
        }
    }

    fn debug(&self, beacons: Option<Vec<Value>>) -> Value {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        let ops_10s = g.stats.op_times.iter().filter(|t| now.duration_since(**t) <= Duration::from_secs(10)).count();
        let rooms: Vec<Value> = g
            .rooms
            .iter()
            .map(|(k, r)| json!({"room": k, "members": r.members, "saver": r.saver, "seq": r.seq, "ops": r.ops}))
            .collect();
        let mut coalesced_live = 0u64;
        let mut dropped_live = 0u64;
        let clients: Vec<Value> = g
            .clients
            .iter()
            .map(|(cid, c)| {
                let co = c.out.coalesced.load(Ordering::Relaxed);
                let dr = c.out.dropped.load(Ordering::Relaxed);
                coalesced_live += co;
                dropped_live += dr;
                json!({"cid": cid, "actor": c.ident.actor, "transport": c.transport,
                    "rooms": c.rooms.keys().collect::<Vec<_>>(), "queued": c.out.queued(),
                    "sent": c.out.sent.load(Ordering::Relaxed), "coalesced": co, "dropped": dr,
                    "age_s": c.opened.elapsed().as_secs()})
            })
            .collect();
        let pct = |ring: &VecDeque<f64>, q: f64| -> Value {
            if ring.is_empty() {
                return Value::Null;
            }
            let mut v: Vec<f64> = ring.iter().copied().collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let i = ((v.len() as f64 - 1.0) * q).round() as usize;
            json!((v[i] * 10.0).round() / 10.0)
        };
        let mut by_event: HashMap<String, u64> = HashMap::new();
        let beacons_measured = beacons.is_some();
        let beacon_n = beacons.as_ref().map(Vec::len).unwrap_or(0);
        for b in beacons.unwrap_or_default() {
            let ev = b.pointer("/body/event").and_then(Value::as_str).unwrap_or("?").to_string();
            *by_event.entry(ev).or_default() += 1;
        }
        json!({
            "measured": true,
            "n_considered": g.clients.len(),
            "protocol": PROTOCOL,
            "clients_live": g.clients.len(),
            "rooms_live": g.rooms.len(),
            "rooms": rooms,
            "clients": clients,
            "totals": {
                "opened_ws": g.stats.opened_ws,
                "opened_sse": g.stats.opened_sse,
                "ops": g.stats.ops,
                "ops_per_s_10s": (ops_10s as f64) / 10.0,
                "presence": g.stats.presence,
                "snapshots_relayed": g.stats.snapshots,
                "resyncs": g.stats.resyncs,
                "refused": g.stats.refused,
                "coalesced": g.stats.coalesced,
                "dropped": g.stats.dropped,
                "saver_handoffs": g.stats.saver_handoffs,
                "coalesced_live_clients": coalesced_live,
                "dropped_live_clients": dropped_live,
            },
            // Client-reported, one sample per client per report window. Null
            // means no client has reported yet, not zero latency.
            "client_reported": {
                "n_samples": g.stats.latency_p95.len(),
                "latency_ms_p50": pct(&g.stats.latency_p50, 0.5),
                "latency_ms_p95": pct(&g.stats.latency_p95, 0.95),
                "fps_p50": pct(&g.stats.fps, 0.5),
                "fps_p5": pct(&g.stats.fps, 0.05),
            },
            "client_beacons": {"measured": beacons_measured, "n_considered": beacon_n, "by_event": by_event},
            "note": "In-memory and per-process: a deploy drops every room and resets these counters; clients reconnect and resync.",
        })
    }
}

// ───────────────────────────── transports ─────────────────────────────

/// A browser page from another origin must not reach a room (cross-site
/// WebSocket hijacking). Browsers send `Origin` on every WebSocket handshake
/// and cross-origin POST, and do not apply CORS to a WebSocket, so a same-site
/// sibling page carrying the SameSite=Lax member cookie could otherwise open
/// one. A request with no `Origin` is not a browser page (CLI, tests) and is
/// left to the normal auth layer.
pub(crate) fn cross_origin(headers: &HeaderMap, uri: &axum::http::Uri) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin).trim_end_matches('/');
    // HTTP/2 carries the host as `:authority`, which reaches a handler as the
    // request URI's authority, not as a Host header.
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| uri.authority().map(|a| a.as_str().to_string()))
        .unwrap_or_default();
    !origin_host.eq_ignore_ascii_case(&host)
}

fn refuse_cross_origin(headers: &HeaderMap, path: &str) -> Response {
    let origin = headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("");
    tracing::warn!(path, origin, verdict = "rt_cross_origin_refused", measured = true, n_considered = 1,
        "realtime request from another origin refused");
    (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "cross-origin realtime request refused"}))).into_response()
}

async fn ws_upgrade(ws: WebSocketUpgrade, uri: axum::http::Uri, headers: HeaderMap) -> Response {
    if cross_origin(&headers, &uri) {
        return refuse_cross_origin(&headers, "/api/rt");
    }
    let ident = Ident::from_headers(&headers);
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run_ws(socket, ident))
}

async fn run_ws(socket: WebSocket, ident: Ident) {
    let (cid, out) = hub().connect(ident, "ws");
    let (mut sink, mut stream) = socket.split();
    let writer_out = out.clone();
    let writer = async move {
        while let Some(batch) = writer_out.next_batch().await {
            for s in batch {
                if sink.send(Message::Text(s.into())).await.is_err() {
                    return;
                }
            }
        }
        let _ = sink.close().await;
    };
    let reader = async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Text(t) => hub().handle(cid, t.as_str()),
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
    }
    hub().disconnect(cid);
}

/// Disconnects the SSE client when axum drops the response stream.
struct SseGuard(u64);
impl Drop for SseGuard {
    fn drop(&mut self) {
        hub().disconnect(self.0);
    }
}

async fn sse_events(headers: HeaderMap) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let (cid, out) = hub().connect(Ident::from_headers(&headers), "sse");
    let guard = SseGuard(cid);
    let stream = futures::stream::unfold((out, guard, VecDeque::<String>::new()), |(out, guard, mut pending)| async move {
        loop {
            if let Some(s) = pending.pop_front() {
                return Some((Ok(Event::default().data(s)), (out, guard, pending)));
            }
            match out.next_batch().await {
                Some(batch) => pending.extend(batch),
                None => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10)).event(Event::default().data("{\"type\":\"ping\"}")))
}

/// `POST /api/rt/send`: the fallback's upstream: `{cid, msgs: [...]}`. The
/// cid must belong to the same verified actor as this request.
async fn sse_send(uri: axum::http::Uri, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if cross_origin(&headers, &uri) {
        return refuse_cross_origin(&headers, "/api/rt/send");
    }
    let Some(cid) = body.get("cid").and_then(Value::as_u64) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing cid"}))).into_response();
    };
    let actor = super::typing::actor(&headers);
    match hub().actor_of(cid) {
        None => return (StatusCode::GONE, Json(json!({"error": "unknown or closed realtime client", "code": "rt_client_gone"}))).into_response(),
        Some(a) if a != actor => {
            tracing::warn!(cid, actor = %actor, owner = %a, verdict = "rt_cid_actor_mismatch", "realtime fallback send refused: cid belongs to another actor");
            return (StatusCode::FORBIDDEN, Json(json!({"error": "that realtime client belongs to someone else"}))).into_response();
        }
        Some(_) => {}
    }
    let msgs = body.get("msgs").and_then(Value::as_array).cloned().unwrap_or_default();
    let n = msgs.len();
    for m in msgs {
        hub().handle_value(cid, m);
    }
    Json(json!({"ok": true, "n": n})).into_response()
}

async fn debug_rt() -> Json<Value> {
    Json(hub().debug(super::client_debug_of_kind("rt")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/rt", get(ws_upgrade))
        .route("/api/rt/events", get(sse_events))
        .route("/api/rt/send", post(sse_send).layer(axum::extract::DefaultBodyLimit::max(MAX_MESSAGE_BYTES)))
        .route("/api/debug/rt", get(debug_rt))
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn a_page_from_another_origin_is_refused_and_same_origin_or_no_origin_is_not() {
        let u = axum::http::Uri::from_static("/api/rt");
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, "sandbox.amux.io".parse().unwrap());
        assert!(!cross_origin(&h, &u), "no Origin: a CLI or test client, left to auth");
        h.insert(axum::http::header::ORIGIN, "https://sandbox.amux.io".parse().unwrap());
        assert!(!cross_origin(&h, &u), "same origin");
        h.insert(axum::http::header::ORIGIN, "https://evil.amux.io".parse().unwrap());
        assert!(cross_origin(&h, &u), "same-site sibling is still another origin");
        h.insert(axum::http::header::ORIGIN, "https://example.com".parse().unwrap());
        assert!(cross_origin(&h, &u));
        let mut t = HeaderMap::new();
        t.insert(axum::http::header::HOST, "desktop.tailnet.ts.net:8824".parse().unwrap());
        t.insert(axum::http::header::ORIGIN, "https://desktop.tailnet.ts.net:8824".parse().unwrap());
        assert!(!cross_origin(&t, &u), "a port is part of the origin and matches");
        // HTTP/2: no Host header, the host is the URI authority.
        let h2 = axum::http::Uri::from_static("https://localhost:18823/api/rt/send");
        let mut o = HeaderMap::new();
        o.insert(axum::http::header::ORIGIN, "https://localhost:18823".parse().unwrap());
        assert!(!cross_origin(&o, &h2), "same origin over HTTP/2 (no Host header)");
        o.insert(axum::http::header::ORIGIN, "https://example.com".parse().unwrap());
        assert!(cross_origin(&o, &h2));
    }

    use super::*;

    fn owner() -> Ident {
        Ident { actor: "owner".into(), scope: None }
    }

    fn tmpfile(name: &str) -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        std::fs::write(&p, "{}").unwrap();
        let key = format!("file:{}", super::super::fs::resolve_nonstrict(&p).display());
        (d, key)
    }

    fn msgs(out: &Outbox) -> Vec<Value> {
        out.drain().iter().map(|s| serde_json::from_str(s).unwrap()).collect()
    }

    fn of_type<'a>(v: &'a [Value], t: &str) -> Vec<&'a Value> {
        v.iter().filter(|m| m["type"] == t).collect()
    }

    fn send(h: &Hub, cid: u64, v: Value) {
        h.handle(cid, &v.to_string());
    }

    #[test]
    fn join_and_leave_update_members_and_drop_empty_rooms() {
        let h = Hub::default();
        let (_d, room) = tmpfile("a.excalidraw");
        let (a, ao) = h.connect(owner(), "ws");
        let (b, bo) = h.connect(owner(), "ws");
        send(&h, a, json!({"t": "join", "room": room}));
        send(&h, b, json!({"t": "join", "room": room}));
        let am = msgs(&ao);
        assert_eq!(am[0]["type"], "hello");
        assert_eq!(of_type(&am, "joined")[0]["can_write"], true);
        let last_members = of_type(&am, "members").last().cloned().cloned().unwrap();
        assert_eq!(last_members["members"].as_array().unwrap().len(), 2, "{am:?}");
        let bm = msgs(&bo);
        assert_eq!(of_type(&bm, "joined")[0]["peers"], 1);
        send(&h, b, json!({"t": "leave", "room": room}));
        let am = msgs(&ao);
        let m = of_type(&am, "members");
        assert_eq!(m.last().unwrap()["members"].as_array().unwrap().len(), 1);
        send(&h, a, json!({"t": "leave", "room": room}));
        assert!(h.inner.lock().unwrap().rooms.is_empty(), "an empty room is dropped");
    }

    #[test]
    fn ops_carry_monotonic_seq_and_a_prev_chain_and_are_not_echoed() {
        let h = Hub::default();
        let (_d, room) = tmpfile("b.excalidraw");
        let (a, ao) = h.connect(owner(), "ws");
        let (b, bo) = h.connect(owner(), "ws");
        send(&h, a, json!({"t": "join", "room": room}));
        send(&h, b, json!({"t": "join", "room": room}));
        msgs(&ao);
        msgs(&bo);
        for i in 0..5 {
            send(&h, a, json!({"t": "op", "room": room, "data": {"i": i}, "id": i}));
        }
        let bm = msgs(&bo);
        let ops = of_type(&bm, "op");
        let seqs: Vec<u64> = ops.iter().map(|o| o["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
        let prevs: Vec<u64> = ops.iter().map(|o| o["prev"].as_u64().unwrap()).collect();
        assert_eq!(prevs, vec![0, 1, 2, 3, 4], "each op links to the previous one this client was due");
        let am = msgs(&ao);
        assert!(of_type(&am, "op").is_empty(), "the sender does not get its own op back");
        assert_eq!(of_type(&am, "ack").last().unwrap()["seq"], 5, "acks coalesce to the newest");
    }

    #[test]
    fn the_server_stamps_the_actor_and_ignores_a_client_claim() {
        let h = Hub::default();
        let (_d, room) = tmpfile("c.excalidraw");
        let (a, _ao) = h.connect(Ident { actor: "member:alice@x.com".into(), scope: None }, "ws");
        let (b, bo) = h.connect(owner(), "ws");
        send(&h, a, json!({"t": "join", "room": room}));
        send(&h, b, json!({"t": "join", "room": room}));
        msgs(&bo);
        send(&h, a, json!({"t": "op", "room": room, "from": {"actor": "owner", "cid": 999}, "actor": "owner",
            "data": {"x": 1}}));
        send(&h, a, json!({"t": "presence", "room": room, "from": {"actor": "owner"}, "data": {"x": 3}}));
        let bm = msgs(&bo);
        let op = of_type(&bm, "op")[0];
        assert_eq!(op["from"]["actor"], "member:alice@x.com");
        assert_eq!(op["from"]["cid"], a);
        assert_eq!(op["from"]["label"], "alice");
        assert!(op.get("actor").is_none(), "a top-level client claim is not relayed");
        assert_eq!(of_type(&bm, "presence")[0]["from"]["actor"], "member:alice@x.com");
    }

    #[test]
    fn permission_refuses_blocked_paths_scoped_writes_and_unknown_rooms() {
        let h = Hub::default();
        let (a, ao) = h.connect(owner(), "ws");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        send(&h, a, json!({"t": "join", "room": format!("file:{home}/.ssh/id_rsa")}));
        send(&h, a, json!({"t": "join", "room": "worker:x"}));
        send(&h, a, json!({"t": "join", "room": "file:relative/path"}));
        let am = msgs(&ao);
        let errs = of_type(&am, "error");
        assert_eq!(errs.len(), 3, "{am:?}");
        assert_eq!(errs[0]["code"], "forbidden");
        assert_eq!(errs[1]["code"], "bad_room");
        assert!(of_type(&am, "joined").is_empty());

        // A scoped member outside any worker folder cannot even watch.
        let (_d, room) = tmpfile("d.excalidraw");
        let (m, mo) = h.connect(Ident { actor: "member:e@x.com".into(), scope: Some(MemberScope::Worker("no-such-worker-zz".into())) }, "ws");
        send(&h, m, json!({"t": "join", "room": room}));
        assert_eq!(of_type(&msgs(&mo), "error")[0]["code"], "forbidden");

        // A dangerous write target is read-only: join works, ops are refused.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("evil.command");
        std::fs::write(&p, "x").unwrap();
        let rk = format!("file:{}", p.display());
        send(&h, a, json!({"t": "join", "room": rk}));
        send(&h, a, json!({"t": "op", "room": rk, "data": 1}));
        let am = msgs(&ao);
        assert_eq!(of_type(&am, "joined")[0]["can_write"], false);
        assert_eq!(of_type(&am, "error")[0]["code"], "read_only");
        // And an op to a room never joined is refused.
        send(&h, a, json!({"t": "op", "room": room, "data": 1}));
        assert_eq!(of_type(&msgs(&ao), "error")[0]["code"], "not_joined");
    }

    #[test]
    fn coalescing_keeps_the_newest_and_splices_the_chain() {
        let out = Outbox::new();
        out.reset_chain("r", 0);
        let op = |seq: u64, ck: Option<&str>, v: u64| Item::new(Some("r"), ck.map(String::from), Some(seq), json!({"type": "op", "data": v}));
        out.push(op(1, None, 1));
        out.push(op(2, Some("el"), 2));
        out.push(op(3, None, 3));
        let o = out.push(op(4, Some("el"), 4));
        assert_eq!(o.coalesced, 1);
        let got = msgs(&out);
        let seqs: Vec<u64> = got.iter().map(|m| m["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, vec![1, 3, 4], "the older keyed op is gone, the newest kept");
        let prevs: Vec<u64> = got.iter().map(|m| m["prev"].as_u64().unwrap()).collect();
        assert_eq!(prevs, vec![0, 1, 3], "op 3 now follows op 1, so the skip is not a gap");
        assert_eq!(got[2]["data"], 4);

        // Presence-style state items: newest wins, nothing else touched.
        let out = Outbox::new();
        for i in 0..100 {
            out.push(Item::state(Some("r"), Some("p:1".into()), json!({"type": "presence", "data": i})));
        }
        let got = msgs(&out);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["data"], 99);
    }

    #[test]
    fn overflow_drops_oldest_ops_and_tells_the_client_to_resync() {
        let out = Outbox::new();
        out.reset_chain("r", 0);
        out.push(Item::state(Some("r"), Some("members".into()), json!({"type": "members"})));
        for s in 1..=(HARD_CAP_ITEMS as u64 + 10) {
            out.push(Item::new(Some("r"), None, Some(s), json!({"type": "op", "data": s})));
        }
        let got = msgs(&out);
        assert!(got.len() <= HARD_CAP_ITEMS + 1);
        assert_eq!(got[0]["type"], "members", "state messages survive op drops");
        let ops = of_type(&got, "op");
        assert_eq!(ops.last().unwrap()["seq"].as_u64().unwrap(), HARD_CAP_ITEMS as u64 + 10, "the latest op is never the one dropped");
        let first = ops[0];
        assert_ne!(
            first["prev"].as_u64().unwrap(),
            0,
            "the client last saw seq 0, so a first op whose prev is not 0 is a visible gap"
        );
        assert_eq!(of_type(&got, "gap").len(), 1, "and a gap notice is queued");
        assert!(out.dropped.load(Ordering::Relaxed) >= 10);
    }

    #[test]
    fn saver_is_the_first_writer_and_hands_off_on_disconnect() {
        let h = Hub::default();
        let (_d, room) = tmpfile("e.excalidraw");
        let (a, _ao) = h.connect(owner(), "ws");
        let (b, _bo) = h.connect(owner(), "ws");
        let (c, co) = h.connect(owner(), "ws");
        for x in [a, b, c] {
            send(&h, x, json!({"t": "join", "room": room}));
        }
        let saver = |h: &Hub| h.inner.lock().unwrap().rooms[&room].saver;
        assert_eq!(saver(&h), Some(a));
        msgs(&co);
        h.disconnect(a);
        assert_eq!(saver(&h), Some(b), "the next writer in join order takes over");
        let cm = msgs(&co);
        assert_eq!(of_type(&cm, "members").last().unwrap()["saver"], b, "and every member is told");
        send(&h, b, json!({"t": "leave", "room": room}));
        assert_eq!(saver(&h), Some(c));
    }

    #[test]
    fn a_gap_or_a_late_join_gets_a_snapshot_from_a_peer_or_from_disk() {
        let h = Hub::default();
        let (_d, room) = tmpfile("f.excalidraw");
        let (a, ao) = h.connect(owner(), "ws");
        send(&h, a, json!({"t": "join", "room": room}));
        // Alone: a snapshot request answers "from disk".
        send(&h, a, json!({"t": "snapshot_request", "room": room}));
        let am = msgs(&ao);
        assert_eq!(of_type(&am, "snapshot")[0]["source"], "disk");
        for i in 0..3 {
            send(&h, a, json!({"t": "op", "room": room, "data": i}));
        }
        // A late joiner triggers a request to the saver...
        let (b, bo) = h.connect(owner(), "ws");
        send(&h, b, json!({"t": "join", "room": room}));
        let am = msgs(&ao);
        let req = of_type(&am, "snapshot_request");
        assert_eq!(req.len(), 1);
        assert_eq!(req[0]["for"], b);
        // ...whose answer reaches only the joiner, with the base seq.
        send(&h, a, json!({"t": "snapshot", "room": room, "for": b, "data": {"elements": [1, 2, 3]}}));
        send(&h, a, json!({"t": "op", "room": room, "data": 9}));
        let bm = msgs(&bo);
        let snap = of_type(&bm, "snapshot")[0];
        assert_eq!(snap["base"], 3);
        assert_eq!(snap["data"]["elements"], json!([1, 2, 3]));
        assert_eq!(snap["from"]["cid"], a);
        let op = of_type(&bm, "op")[0];
        assert_eq!((op["seq"].as_u64(), op["prev"].as_u64()), (Some(4), Some(3)), "the chain continues from the snapshot");
        // A gap reported by b is answered the same way.
        send(&h, b, json!({"t": "snapshot_request", "room": room, "why": "gap"}));
        assert_eq!(of_type(&msgs(&ao), "snapshot_request")[0]["why"], "gap");
        assert!(h.inner.lock().unwrap().stats.resyncs >= 3);
    }

    #[test]
    fn room_keys_normalise_so_two_spellings_share_a_room() {
        let (_d, room) = tmpfile("g.excalidraw");
        let path = room.strip_prefix("file:").unwrap();
        let dir = Path::new(path).parent().unwrap();
        let alt = format!("file:{}/./sub/../g.excalidraw", dir.display());
        assert_eq!(normalize_room(&alt).unwrap(), room);
    }

    #[test]
    fn debug_reports_measured_and_population() {
        let h = Hub::default();
        let (a, _) = h.connect(owner(), "sse");
        send(&h, a, json!({"t": "stats", "lat_p50": 4.0, "lat_p95": 9.0, "fps": 60.0}));
        let d = h.debug(Some(vec![]));
        assert_eq!(d["measured"], true);
        assert_eq!(d["n_considered"], 1);
        assert_eq!(d["totals"]["opened_sse"], 1);
        assert_eq!(d["client_reported"]["latency_ms_p95"], 9.0);
    }
}
