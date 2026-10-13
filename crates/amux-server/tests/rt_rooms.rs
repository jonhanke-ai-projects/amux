//! Realtime rooms over a REAL socket (api/rt.rs, docs/design/realtime-rooms.md).
//!
//! The unit tests in api/rt.rs drive the hub directly. This file drives the
//! shipped router through `GET /api/rt` with a WebSocket client, so the
//! upgrade, the per-client writer task and TCP backpressure are all real: a
//! receiver that stops reading fills its socket, its writer blocks, and the
//! queue behind it must coalesce rather than grow or lose the newest state.

use amux_server::api::{router, AppState};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

async fn serve() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = amux_server::db::Store::open(&dir.path().join("rt.db")).unwrap();
    let app = router(AppState {
        store: std::sync::Arc::new(store),
        started: std::time::Instant::now(),
        build_hash: "test".into(),
        auth_token: None,
        reconciled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("ws://{addr}/api/rt"), dir)
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .expect("a message within 20s")
            .expect("stream open")
            .expect("frame ok");
        if let Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

async fn until(ws: &mut Ws, ty: &str) -> Value {
    loop {
        let v = next_json(ws).await;
        if v["type"] == ty {
            return v;
        }
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string())).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_receiver_gets_the_newest_state_coalesced_over_a_real_socket() {
    let (url, dir) = serve().await;
    let file = dir.path().join("slow.excalidraw");
    std::fs::write(&file, "{}").unwrap();
    let room = format!("file:{}", file.display());

    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let a_hello = until(&mut a, "hello").await;
    until(&mut b, "hello").await;
    send(&mut a, json!({"t": "join", "room": room})).await;
    let joined = until(&mut a, "joined").await;
    let canonical = joined["room"].as_str().unwrap().to_string();
    send(&mut b, json!({"t": "join", "room": room})).await;
    let b_joined = until(&mut b, "joined").await;
    assert_eq!(b_joined["peers"], 1);
    let base = b_joined["seq"].as_u64().unwrap();

    // B now stops reading. A sends N keyed ops (the same element, ever newer),
    // large enough that B's socket fills and its writer blocks.
    const N: u64 = 2500;
    let pad = "x".repeat(16 * 1024);
    for i in 0..N {
        send(&mut a, json!({"t": "op", "room": canonical, "ck": "el-1", "data": {"i": i, "pad": pad},
            // A client claim of identity, which the server must ignore.
            "from": {"actor": "someone-else", "cid": 999}})).await;
    }
    // A's last ack proves the server processed every op before B reads.
    loop {
        let v = next_json(&mut a).await;
        if v["type"] == "ack" && v["seq"].as_u64() == Some(base + N) {
            break;
        }
    }

    // Now B drains. It must end on the newest op, see far fewer than N, and
    // see an unbroken prev chain (a coalesced skip is not a gap).
    let mut ops: Vec<Value> = Vec::new();
    loop {
        let v = next_json(&mut b).await;
        if v["type"] == "gap" {
            panic!("keyed ops must coalesce, not overflow: {v}");
        }
        if v["type"] == "op" {
            let last = v["data"]["i"].as_u64() == Some(N - 1);
            ops.push(v);
            if last {
                break;
            }
        }
    }
    let mut prev = base;
    for o in &ops {
        assert_eq!(o["prev"].as_u64(), Some(prev), "chain broken at {}", o["seq"]);
        prev = o["seq"].as_u64().unwrap();
        assert_eq!(o["from"]["cid"], a_hello["cid"], "the server's stamp, not the client's claim");
        assert_ne!(o["from"]["actor"], "someone-else");
    }
    assert_eq!(prev, base + N, "the newest op arrived");
    assert!(
        (ops.len() as u64) < N / 2,
        "a stalled receiver got {} of {N} ops; coalescing should have collapsed most of them",
        ops.len()
    );
    eprintln!("[rt] stalled receiver got {} of {N} keyed ops, ending on the newest", ops.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_hands_the_saver_role_to_the_next_writer_over_a_real_socket() {
    let (url, dir) = serve().await;
    let file = dir.path().join("saver.excalidraw");
    std::fs::write(&file, "{}").unwrap();
    let room = format!("file:{}", file.display());
    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let a_cid = until(&mut a, "hello").await["cid"].clone();
    let b_cid = until(&mut b, "hello").await["cid"].clone();
    send(&mut a, json!({"t": "join", "room": room})).await;
    until(&mut a, "joined").await;
    send(&mut b, json!({"t": "join", "room": room})).await;
    let m = until(&mut b, "members").await;
    assert_eq!(m["saver"], a_cid);
    drop(a);
    loop {
        let m = until(&mut b, "members").await;
        if m["members"].as_array().unwrap().len() == 1 {
            assert_eq!(m["saver"], b_cid, "B inherits the saver role when A's socket closes");
            break;
        }
    }
}
