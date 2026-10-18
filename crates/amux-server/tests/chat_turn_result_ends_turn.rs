//! A chat turn ends when the provider reports its final result, even if the
//! provider process stays alive.
//!
//! Ethan, 2026-10-12: "looks like this chat is stuck". mxp-gs12@chat had
//! written its whole reply, but its `claude -p` was waiting on a background
//! retry loop it had started (up to 30 minutes), so the Chat tab sat on
//! "responding…" with a message queued behind it. The turn only ended on
//! process exit. It must be recorded once the result arrives, and the
//! lingering process must NOT be killed: it is work the turn started.
//!
//! Same setup as chat_turn_reattach.rs: a detached provider and the in-flight
//! markers, followed by recovery. Own process, because AMUX_HOME is global.
use amux_server::api::{router, AppState};
use amux_server::db::Store;
use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

// Writes its reply and its final result, then stays alive the way a provider
// waiting on a background command does.
const LINGERING_CLAUDE: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_ARGS_LOG"
input=$(cat)
python3 -c 'import json,sys; print(json.dumps({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"all done: "+sys.argv[1]}}}))' "$input"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"conv-1"}'
sleep 120
"#;

async fn history(app: &axum::Router) -> Value {
    let req = Request::builder().uri("/api/sessions/chatty/chat").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn a_turn_is_recorded_at_its_final_result_while_the_provider_lingers() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    let turns = home.join("chat-state/turns");
    std::fs::create_dir_all(&turns).unwrap();
    let fake = tmp.path().join("fake-claude");
    std::fs::write(&fake, LINGERING_CLAUDE).unwrap();
    std::process::Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
    let args_log = tmp.path().join("args.log");
    unsafe {
        std::env::set_var("AMUX_HOME", &home);
        std::env::set_var("AMUX_CHAT_CLAUDE_BIN", &fake);
        std::env::set_var("FAKE_ARGS_LOG", &args_log);
    }
    let dir = tmp.path().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        home.join("sessions/chatty.env"),
        format!("CC_DIR=\"{}\"\nCC_WORKER_TYPE=\"chat\"\n", dir.display()),
    )
    .unwrap();

    // What the previous server process did before it was replaced: started the
    // provider DETACHED (new process group, files for stdin/stdout/exit) ...
    let turn = "TURN-ACROSS-RESTART";
    let stem = turns.join(format!("{turn}.0"));
    let p = |ext: &str| format!("{}.{ext}", stem.display());
    std::fs::write(p("in"), "what is the status?").unwrap();
    let inner = format!(
        "{} --session-id conv-1 \"$@\"; ec=$?; printf '%s' \"$ec\" > '{tmp}' && mv '{tmp}' '{exit}'",
        fake.display(),
        tmp = p("exit.tmp"),
        exit = p("exit"),
    );
    let launcher = format!(
        "bash -c \"$0\" amux-chat < '{}' > '{}' 2> '{}' & echo $!",
        p("in"),
        p("out"),
        p("err")
    );
    let child = std::process::Command::new("bash")
        .arg("-c")
        .arg(&launcher)
        .arg(&inner)
        .process_group(0)
        .output()
        .unwrap();
    assert!(child.status.success());
    // ... and left its in-flight markers. The user message row exists already.
    let pgid = {
        // The launcher was the group leader; read the group from the turn's pid.
        let pid: i32 = String::from_utf8_lossy(&child.stdout).trim().parse().unwrap();
        unsafe { libc::getpgid(pid) }
    };
    std::fs::write(
        home.join("sessions/chatty.meta.json"),
        json!({
            "chat_running": true,
            "chat_turns": 1,
            "chat_conversation_id": "conv-1",
            "cc_conversation_id": "conv-1",
            "chat_inflight_turn": turn,
            "chat_inflight_text": "what is the status?",
            "chat_inflight_origin": "owner",
            "chat_inflight_files": stem.to_string_lossy(),
            "chat_inflight_pgid": pgid,
            "chat_inflight_provider": "claude",
            "chat_inflight_started": 1.0,
        })
        .to_string(),
    )
    .unwrap();

    // The restarted server.
    let store = Arc::new(Store::open(&tmp.path().join("t.db")).unwrap());
    let state = AppState {
        store,
        started: std::time::Instant::now(),
        build_hash: "test".into(),
        auth_token: None,
        reconciled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    let app = router(state.clone());
    let (workers, interrupted, _resumed) = amux_server::api::chat_worker::recover_all(&state).await;
    assert_eq!((workers, interrupted), (1, 0), "a re-attached turn is not reported as interrupted");

    let started = std::time::Instant::now();
    let mut h = history(&app).await;
    for _ in 0..400 {
        let done = h["messages"].as_array().unwrap().iter().any(|m| m["role"] == "assistant" && m["turn_id"] == turn);
        if done && h["busy"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        h = history(&app).await;
    }
    assert!(started.elapsed() < Duration::from_secs(15), "the turn ended on its result, not on the 120s process");
    assert_eq!(h["busy"], false, "the lane is free for the next message: {h}");
    let msgs = h["messages"].as_array().unwrap();
    let replies: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "assistant" && m["turn_id"] == turn).collect();
    assert_eq!(replies.len(), 1, "the reply is recorded exactly once: {msgs:?}");
    let r = replies[0];
    assert!(r["error"].is_null(), "a lingering provider is not an error: {r}");
    assert_eq!(r["text"], "all done: what is the status?");
    // The lingering process is the turn's own background work: still alive.
    let alive = unsafe { libc::killpg(pgid, 0) } == 0;
    assert!(alive, "the provider's process group must not be killed");
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
}
