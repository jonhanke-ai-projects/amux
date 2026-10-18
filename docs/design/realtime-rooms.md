# Realtime rooms

A small multiplayer protocol any amux feature can use: open the same thing in
two tabs (or on two devices) and see each other's changes as they happen. The
Draw tab is the first adopter: one person draws, the other watches the stroke
appear at frame rate.

Server: `crates/amux-server/src/api/rt.rs`. Client: the "Realtime rooms" and
"Draw adapter for realtime rooms" sections at the end of
`crates/amux-dashboard/static/app.js`. Tests: the `api::rt` unit tests,
`crates/amux-server/tests/rt_rooms.rs` (real sockets) and
`e2e/draw-realtime.spec.ts`.

## Shape

- **Transport.** One WebSocket per browser tab at `GET /api/rt`, on the same
  origin, TLS and auth as the rest of the API (the bearer rides `?_token=` the
  way EventSource URLs do). It multiplexes any number of rooms.
- **Fallback.** If the socket never opens (a proxy that refuses upgrades), the
  tab switches to `GET /api/rt/events` (SSE, server to client) plus
  `POST /api/rt/send` (`{cid, msgs: [...]}`, batched every ~30 ms). Same hub,
  same messages, same guarantees. Measured on this host the fallback held
  p95 under 30 ms; the cost is a POST per batch instead of a frame.
- **Rooms** are keyed by resource. Today only `file:<absolute path>` exists;
  the server resolves the path the way the file API does, so two spellings of
  one file share a room. `worker:<name>` and `card:<id>` are the intended next
  kinds: add a branch to `normalize_room` and `room_access`.

## Messages

Client to server (`t` is the type):

| `t` | Fields | Notes |
|---|---|---|
| `join` | `room` | Read permission required. |
| `leave` | `room` | |
| `op` | `room`, `data`, optional `ck`, `id` | Write permission required. `data` is opaque to the server. |
| `presence` | `room`, `data` | Cursor, selection, anything ephemeral. |
| `snapshot_request` | `room`, `why` | Sent on a detected gap. |
| `snapshot` | `room`, `for`, `data` | A writer answering a request for client `for`. |
| `stats` | `lat_p50`, `lat_p95`, `fps` | Client-measured, every 10 s, feeds `/api/debug/rt`. |
| `ping` | | |

Server to client (`type`):

| `type` | Fields | Notes |
|---|---|---|
| `hello` | `cid`, `actor`, `label`, `color`, `transport`, `protocol` | First message on every connection. |
| `joined` | `room`, `asked`, `seq`, `you`, `can_write`, `peers` | `room` is the resolved key; `asked` is what the client sent. |
| `members` | `room`, `members[]`, `saver`, `seq` | State message: the newest replaces any queued one. |
| `op` | `room`, `seq`, `prev`, `from`, `data` | Never echoed to the sender. |
| `ack` | `room`, `seq`, `id` | To the sender of an op. |
| `presence` | `room`, `from`, `data` | |
| `snapshot_request` | `room`, `for`, `why` | Sent to one writer peer, the saver first. |
| `snapshot` | `room`, `source` (`peer` or `disk`), `base`, `from`, `data` | `disk` means nobody else holds it: the file is the truth. |
| `gap` | `room` | The server dropped ops for this client; resync. |
| `error` | `room`, `code`, `error` | `forbidden`, `read_only`, `not_joined`, `bad_room`, ... |

`from` is always `{cid, actor, label, color}` written by the server. Any
identity a client puts in its own message is ignored: the actor comes from the
same verified headers as everything else (`typing::actor`).

## Guarantees

- **Order.** Every op in a room gets the next `seq`. Each recipient also gets
  `prev`, the seq of the previous op it was due in that room. If `prev` does
  not match the last seq the client saw, something was lost: it applies the op
  anyway and asks a peer for a snapshot.
- **Bounded queues.** Each client's outbound queue is bounded. A message with a
  coalesce key (`presence` per sender, `members`, `ack`, and any op that sets
  `ck`) replaces an older queued one with the same key, and the `prev` chain is
  spliced so a superseded op never reads as a gap. Past the hard cap (2048
  messages or 24 MB) the oldest ops are dropped, never the newest, and the
  client is sent `gap` so it resyncs. A stalled real socket received 7 of 2500
  keyed ops in `rt_rooms.rs`, ending on the newest.
- **Saver.** The first writer to join a room is its saver; when it leaves or
  disconnects the role passes to the next writer in join order and `members`
  announces it. Only the saver writes the file. Alone in a room you are the
  saver, so solo behaviour is unchanged.
- **Permission.** Join needs read access under the file API's rules
  (`fs::is_path_allowed`, plus a scoped member's worker folders). Ops and
  snapshots need write access: not a dangerous-write target, and not a scoped
  (non-global) member, who can watch but not change.
- **Late joiners** load the file from disk and get a snapshot from a writer
  peer; the joiner also re-offers what it holds, and the version rules decide.

## What it is not

- **Not a store.** Rooms live in process memory, like typing presence. A
  deploy drops every room; clients reconnect, rejoin and resync from a peer or
  from disk. The durable truth is the file.
- **Not a CRDT.** The server never merges anything. Convergence is the
  adopter's job, which for Draw is Excalidraw's own rule (higher element
  `version` wins, `versionNonce` breaks ties). An adopter whose data has no
  such rule should send whole state with a `ck`, not deltas.
- **Not the board.** The board's property is that every mutation is attributed
  and gated; rooms are for ephemeral, high-rate collaboration on a resource
  whose truth lives elsewhere.

## How Draw uses it

- Joins `file:<path>` when a drawing opens (or is created or renamed), leaves
  when the tab is left or another drawing opens.
- Each animation frame sends at most one op: elements whose `version` rose
  since they were last sent. A freehand stroke still being drawn is sent as
  append-only point deltas from index N, then as the whole element when it ends.
- Remote ops queue and apply once per frame through
  `reconcileElements(restoreElements(...))` and
  `updateScene({captureUpdate: CaptureUpdateAction.NEVER})`, so they never
  enter local undo.
- Cursors are Excalidraw `collaborators`; presence is sent at most every 30 ms
  and interpolated toward the newest position each frame.
- The saver autosaves and polls the file as before, then sends a `saved` op
  (its new ETag) and, when a worker rewrites the file on disk, a `reload` op
  with the new contents. Other tabs skip their own saves and polls and show
  "Live: <saver> saves this file".

## Adopting a room in a new feature

```js
const room = rtJoin('file:' + path, {
  onOp: m => apply(m.data),                      // someone else's change
  onPresence: m => showCursor(m.from, m.data),
  onSnapshotRequest: m => room.snapshot(m.for, currentState()),
  onSnapshot: m => { if (m.source === 'peer') replaceWith(m.data); },
  onMembers: m => setSaver(room.isSaver()),
});
room.op(change, { ck: 'el:' + id });            // ck only when the op is whole state for that key
room.presence({ x, y });
room.leave();
```

Keep ops idempotent and ordered by something the data carries (a version), so
that a replayed or reordered op is harmless.

## Observability

`GET /api/debug/rt` (behind the normal API auth; scoped members are refused) reports live rooms and clients, ops per
second, coalesced and dropped counts, saver hand-offs, resyncs, refusals, the
client-reported latency and fps percentiles, and the client beacons of kind
`rt` (`ws-failed`, `fallback`, `resync`, `reconnect`), with `measured` and
`n_considered`. Refusals log `verdict="rt_refused"`, overflow drops
`rt_queue_overflow_dropped`, and saver changes `rt_saver_handoff`.

## Measured (this branch, local host)

From `e2e/draw-realtime.spec.ts`, receiver side, a 180-frame stroke driven
one pointer move per animation frame:

| target | latency p50 | latency p95 | remote fps |
|---|---|---|---|
| desktop (Chromium) | 1-16 ms | 17-23 ms | 54-58 |
| mobile 375 (Chromium) | 16 ms | 17 ms | 60 |
| ios-safari (WebKit) | 16 ms | 26 ms | 60 |

Latency is measured from the sender's timestamp to the animation frame that
applied the update on the receiver, so it includes up to one frame of waiting.
The desktop budget (p95 under 50 ms, at least 50 fps) is asserted only on a
local desktop run; CI and the phone targets assert p95 under 150 ms and at
least 25 fps, because shared CI runners cannot promise frame timing.
