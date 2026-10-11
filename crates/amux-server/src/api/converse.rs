//! Converse: hands-free voice conversations with a worker (the worker view's
//! Converse tab).
//!
//! THE WORKER ANSWERS. The browser turns speech into text, sends each finished
//! utterance through the ordinary `POST /api/sessions/{name}/send` path and
//! reads the reply from `GET /api/sessions/{name}/last-message`, the same two
//! routes the composer and read-aloud use. Nothing here talks to a model; this
//! module only keeps the LOG of those conversations so any of them can be
//! replayed, and expires it.
//!
//! THE FOLDER IS THE STORE, like `recordings.rs`. One conversation is up to two
//! files under `<AMUX_HOME>/converse/<worker>/`:
//!
//! - `<id>.json`: title, engine, start/end, duration and every transcript turn
//!   (`role`, `text`, `t_ms` offset from the start).
//! - `<id>.<ext>`: the session audio the browser recorded with MediaRecorder,
//!   when it could. Optional; replay falls back to speaking the transcript.
//!
//! EXPIRY is computed, never stored: `started_at + AMUX_CONVERSE_RETENTION_DAYS`
//! (default 30), read on every request, so changing the setting re-dates every
//! conversation at once. An expired conversation stops being listed the moment
//! it expires and the `converse-retention` job deletes its files on its next
//! tick, logging one `verdict=` line per sweep.
//!
//! ENGINES. `browser` (Web Speech in, speechSynthesis out) always works.
//! `personaplex` (NVIDIA PersonaPlex, a 7B full-duplex speech model) needs an
//! NVIDIA GPU server; it is offered only when `AMUX_PERSONAPLEX_URL` resolves
//! (server.env, then the worker > group > global scope) AND that server
//! answers. `/api/converse/config` says which, and why, every time; it never
//! reports a connection it did not make.

use super::AppState;
use axum::body::Bytes;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const RETENTION_KEY: &str = "AMUX_CONVERSE_RETENTION_DAYS";
pub const PERSONAPLEX_KEY: &str = "AMUX_PERSONAPLEX_URL";
const DEFAULT_RETENTION_DAYS: u32 = 30;
const DAY_MS: i64 = 86_400_000;
/// A long conversation at opus bitrates is a few MB per 10 minutes; 200 MB is
/// hours of audio and still bounded.
const MAX_AUDIO_BYTES: usize = 200 * 1024 * 1024;
const MAX_TURNS: usize = 5_000;
const MAX_TURN_CHARS: usize = 40_000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/converse/config", get(config))
        .route("/api/converse/sweep", post(sweep_now))
        .route(
            "/api/converse/{worker}/conversations",
            get(list).post(upsert),
        )
        .route(
            "/api/converse/{worker}/conversations/{id}",
            get(get_one).delete(delete_one),
        )
        .route(
            "/api/converse/{worker}/conversations/{id}/audio",
            get(get_audio)
                .put(put_audio)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
}

// ---- paths and validation ---------------------------------------------------

pub(crate) fn store_root(home: &Path) -> PathBuf {
    home.join("converse")
}

fn valid_worker(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= 128
        && !w.contains("..")
        && w.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && w
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
}

fn valid_id(id: &str) -> bool {
    (6..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

/// Retention in days, read live: server.env first, then the process env.
pub(crate) fn retention_days(home: &Path) -> (u32, &'static str) {
    match super::settings::effective_env(home, RETENTION_KEY)
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        Some(d) if d > 0 => (d.min(3650), "env"),
        _ => (DEFAULT_RETENTION_DAYS, "default"),
    }
}

fn expires_at(started_at: i64, days: u32) -> i64 {
    started_at.saturating_add(i64::from(days) * DAY_MS)
}

/// Whole days left, rounded up, so a conversation is "Expires in 1 day" until
/// the moment it is gone, and never "in 0 days" while still listed.
fn expires_in_days(expires: i64, now: i64) -> i64 {
    let left = expires - now;
    if left <= 0 {
        0
    } else {
        (left + DAY_MS - 1) / DAY_MS
    }
}

fn ext_for_mime(mime: &str) -> &'static str {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "audio/webm" | "video/webm" => "webm",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" | "video/mp4" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" => "mp3",
        _ => "bin",
    }
}

fn read_record(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.is_object().then_some(v)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Add the computed fields every reader needs (expiry, audio URL).
fn decorate(mut v: Value, worker: &str, days: u32, now: i64) -> Value {
    let started = v["started_at"].as_i64().unwrap_or(0);
    let exp = expires_at(started, days);
    v["expires_at"] = json!(exp);
    v["expires_in_days"] = json!(expires_in_days(exp, now));
    v["retention_days"] = json!(days);
    if v["audio"].is_object() {
        let id = v["id"].as_str().unwrap_or("").to_string();
        v["audio"]["url"] = json!(format!(
            "/api/converse/{}/conversations/{}/audio",
            urlencode(worker),
            id
        ));
    }
    v
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn summary(v: &Value) -> Value {
    let turns = v["turns"].as_array().cloned().unwrap_or_default();
    let preview = turns
        .iter()
        .find_map(|t| t["text"].as_str().filter(|s| !s.trim().is_empty()))
        .unwrap_or("")
        .chars()
        .take(160)
        .collect::<String>();
    let mut s = v.clone();
    if let Some(o) = s.as_object_mut() {
        o.remove("turns");
    }
    s["turn_count"] = json!(turns.len());
    s["preview"] = json!(preview);
    s
}

// ---- config -----------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ConfigQuery {
    worker: Option<String>,
}

fn personaplex_url(home: &Path, worker: Option<&str>) -> (Option<String>, &'static str) {
    if let Some(v) = super::settings::effective_env(home, PERSONAPLEX_KEY) {
        return (Some(v.trim().to_string()), "server_env");
    }
    if let Some(w) = worker.filter(|w| valid_worker(w)) {
        if let Some(v) = super::session_verbs::scoped_setting_in(home, w, PERSONAPLEX_KEY) {
            return (Some(v), "scope");
        }
    }
    (None, "unset")
}

async fn probe(url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        // moshi/PersonaPlex servers commonly run on a self-signed cert.
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;
    let r = client.get(url).send().await.map_err(|e| e.to_string())?;
    Ok(r.status().as_u16())
}

async fn config(Query(q): Query<ConfigQuery>) -> Response {
    let home = crate::config::amux_home();
    let (days, days_source) = retention_days(&home);
    let (url, url_source) = personaplex_url(&home, q.worker.as_deref());
    let personaplex = match url.as_deref() {
        None => json!({
            "id": "personaplex",
            "label": "NVIDIA PersonaPlex",
            "available": false,
            "configured": false,
            "probed": false,
            "reason": format!("Needs a PersonaPlex server on an NVIDIA GPU. Set {PERSONAPLEX_KEY} to its address."),
        }),
        Some(u) if !(u.starts_with("http://") || u.starts_with("https://")) => json!({
            "id": "personaplex",
            "label": "NVIDIA PersonaPlex",
            "available": false,
            "configured": true,
            "probed": false,
            "url": u,
            "url_source": url_source,
            "reason": format!("{PERSONAPLEX_KEY} must be an http(s) URL; it is {u:?}."),
        }),
        Some(u) => {
            let res = probe(u).await;
            let (ok, reason) = match &res {
                Ok(code) if *code < 500 => (true, format!("PersonaPlex server answered HTTP {code}.")),
                Ok(code) => (false, format!("{PERSONAPLEX_KEY} is set but the server answered HTTP {code}.")),
                Err(e) => (false, format!("{PERSONAPLEX_KEY} is set but the server did not answer: {e}")),
            };
            if !ok {
                tracing::warn!(url = %u, verdict = "personaplex_unreachable", measured = true,
                    n_considered = 1, "[converse] {reason}");
            }
            json!({
                "id": "personaplex",
                "label": "NVIDIA PersonaPlex",
                "available": ok,
                "configured": true,
                "probed": true,
                "url": u,
                "url_source": url_source,
                "reason": reason,
            })
        }
    };
    Json(json!({
        "measured": true,
        "n_considered": 2,
        "engines": [
            {"id": "browser", "label": "Browser speech", "available": true,
             "reason": "Speech recognition and synthesis built into this browser."},
            personaplex,
        ],
        "retention_days": days,
        "retention_source": days_source,
        "retention_key": RETENTION_KEY,
        "personaplex_key": PERSONAPLEX_KEY,
    }))
    .into_response()
}

// ---- list / read ------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ListQuery {
    limit: Option<usize>,
}

struct Listing {
    rows: Vec<Value>,
    n_considered: usize,
    expired: usize,
    unreadable: usize,
}

fn list_dir(dir: &Path, worker: &str, days: u32, now: i64) -> std::io::Result<Listing> {
    let mut out = Listing { rows: vec![], n_considered: 0, expired: 0, unreadable: 0 };
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        out.n_considered += 1;
        let Some(v) = read_record(&p) else {
            out.unreadable += 1;
            continue;
        };
        let v = decorate(v, worker, days, now);
        if v["expires_at"].as_i64().unwrap_or(0) <= now {
            out.expired += 1;
            continue;
        }
        out.rows.push(v);
    }
    out.rows
        .sort_by_key(|v| std::cmp::Reverse(v["started_at"].as_i64().unwrap_or(0)));
    Ok(out)
}

async fn list(AxPath(worker): AxPath<String>, Query(q): Query<ListQuery>) -> Response {
    if !valid_worker(&worker) {
        return err(StatusCode::BAD_REQUEST, "invalid worker name");
    }
    let home = crate::config::amux_home();
    let (days, days_source) = retention_days(&home);
    let dir = store_root(&home).join(&worker);
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let w = worker.clone();
    let now = now_ms();
    let res = tokio::task::spawn_blocking(move || list_dir(&dir, &w, days, now)).await;
    match res {
        Ok(Ok(l)) => {
            let total = l.rows.len();
            Json(json!({
                "worker": worker,
                "measured": true,
                "n_considered": l.n_considered,
                "n": total,
                "expired_pending_sweep": l.expired,
                "unreadable": l.unreadable,
                "retention_days": days,
                "retention_source": days_source,
                "conversations": l.rows.iter().take(limit).map(summary).collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "worker": worker, "measured": false, "n_considered": 0,
                "why_unmeasured": format!("could not read the conversation folder: {e}"),
                "conversations": [],
            })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_one(AxPath((worker, id)): AxPath<(String, String)>) -> Response {
    if !valid_worker(&worker) || !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    }
    let home = crate::config::amux_home();
    let (days, _) = retention_days(&home);
    let path = store_root(&home).join(&worker).join(format!("{id}.json"));
    let now = now_ms();
    match read_record(&path).map(|v| decorate(v, &worker, days, now)) {
        Some(v) if v["expires_at"].as_i64().unwrap_or(0) > now => Json(v).into_response(),
        Some(_) => err(StatusCode::GONE, "this conversation has expired"),
        None => err(StatusCode::NOT_FOUND, "no such conversation"),
    }
}

// ---- write ------------------------------------------------------------------

fn clean_turns(v: &Value) -> Vec<Value> {
    v.as_array()
        .map(|a| {
            a.iter()
                .take(MAX_TURNS)
                .filter_map(|t| {
                    let text = t["text"].as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let role = match t["role"].as_str().unwrap_or("") {
                        "user" | "you" => "user",
                        "worker" | "assistant" => "worker",
                        _ => "system",
                    };
                    Some(json!({
                        "role": role,
                        "text": text.chars().take(MAX_TURN_CHARS).collect::<String>(),
                        "t_ms": t["t_ms"].as_i64().unwrap_or(0).max(0),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn short(s: Option<&str>, max: usize) -> String {
    s.unwrap_or("").trim().chars().take(max).collect()
}

/// Build the stored record from a client body, keeping fields the server owns
/// (`created_at`, `audio`) from any previous version.
fn build_record(worker: &str, id: &str, body: &Value, prev: Option<&Value>, now: i64) -> Value {
    let turns = clean_turns(&body["turns"]);
    // A client may name its own start (it knows when the mic opened), but not
    // a future one: expiry is derived from it.
    let started_at = body["started_at"]
        .as_i64()
        .or_else(|| prev.and_then(|p| p["started_at"].as_i64()))
        .unwrap_or(now)
        .min(now);
    let ended_at = body["ended_at"].as_i64().map(|e| e.clamp(started_at, now));
    let duration_ms = body["duration_ms"]
        .as_i64()
        .or_else(|| ended_at.map(|e| e - started_at))
        .unwrap_or_else(|| turns.last().and_then(|t| t["t_ms"].as_i64()).unwrap_or(0))
        .max(0);
    let mut title = short(body["title"].as_str(), 120);
    if title.is_empty() {
        title = turns
            .iter()
            .find(|t| t["role"] == "user")
            .and_then(|t| t["text"].as_str())
            .map(|s| s.chars().take(60).collect::<String>())
            .unwrap_or_else(|| "Conversation".to_string());
    }
    json!({
        "id": id,
        "worker": worker,
        "title": title,
        "engine": short(body["engine"].as_str(), 32).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>(),
        "voice": short(body["voice"].as_str(), 120),
        "personality": short(body["personality"].as_str(), 40),
        "started_at": started_at,
        "ended_at": ended_at,
        "duration_ms": duration_ms,
        "turns": turns,
        "audio": prev.map(|p| p["audio"].clone()).unwrap_or(Value::Null),
        "created_at": prev.and_then(|p| p["created_at"].as_i64()).unwrap_or(now),
        "updated_at": now,
    })
}

async fn upsert(AxPath(worker): AxPath<String>, Json(body): Json<Value>) -> Response {
    if !valid_worker(&worker) {
        return err(StatusCode::BAD_REQUEST, "invalid worker name");
    }
    let id = match body["id"].as_str() {
        Some(s) if valid_id(s) => s.to_string(),
        Some(_) => return err(StatusCode::BAD_REQUEST, "id must be 6-64 of [A-Za-z0-9_-]"),
        None => format!("cv-{}", ulid::Ulid::new().to_string().to_ascii_lowercase()),
    };
    let home = crate::config::amux_home();
    let (days, _) = retention_days(&home);
    let path = store_root(&home).join(&worker).join(format!("{id}.json"));
    let now = now_ms();
    let w = worker.clone();
    let res = tokio::task::spawn_blocking(move || {
        let prev = read_record(&path);
        let rec = build_record(&w, &id, &body, prev.as_ref(), now);
        let bytes = serde_json::to_vec_pretty(&rec).map_err(std::io::Error::other)?;
        write_atomic(&path, &bytes)?;
        Ok::<_, std::io::Error>((rec, prev.is_none()))
    })
    .await;
    match res {
        Ok(Ok((rec, created))) => {
            if created {
                tracing::info!(worker = %worker, id = %rec["id"].as_str().unwrap_or(""),
                    engine = %rec["engine"].as_str().unwrap_or(""), "[converse] conversation started");
            }
            let mut out = decorate(rec, &worker, days, now);
            out["created"] = json!(created);
            Json(out).into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(worker = %worker, "[converse] could not store conversation: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, format!("could not store the conversation: {e}"))
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn audio_files(dir: &Path, id: &str) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_stem().and_then(|s| s.to_str()) == Some(id)
                && p.extension().and_then(|e| e.to_str()) != Some("json")
        })
        .collect()
}

async fn delete_one(AxPath((worker, id)): AxPath<(String, String)>) -> Response {
    if !valid_worker(&worker) || !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    }
    let dir = store_root(&crate::config::amux_home()).join(&worker);
    let key = id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        let json_path = dir.join(format!("{key}.json"));
        let mut n = 0usize;
        if std::fs::remove_file(&json_path).is_ok() {
            n += 1;
        }
        for p in audio_files(&dir, &key) {
            if std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
        n
    })
    .await
    .unwrap_or(0);
    if removed == 0 {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    }
    tracing::info!(worker = %worker, id = %id, files = removed, "[converse] conversation deleted");
    Json(json!({"ok": true, "id": id, "files_removed": removed})).into_response()
}

async fn put_audio(
    AxPath((worker, id)): AxPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !valid_worker(&worker) || !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    }
    if body.is_empty() {
        return err(StatusCode::BAD_REQUEST, "the recording is empty");
    }
    if body.len() > MAX_AUDIO_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, format!("audio is over {} MB", MAX_AUDIO_BYTES >> 20));
    }
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let has_worker_voice = headers
        .get("x-converse-worker-voice")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "1");
    let home = crate::config::amux_home();
    let (days, _) = retention_days(&home);
    let dir = store_root(&home).join(&worker);
    let w = worker.clone();
    let now = now_ms();
    let res = tokio::task::spawn_blocking(move || {
        let json_path = dir.join(format!("{id}.json"));
        let Some(mut rec) = read_record(&json_path) else {
            return Err((StatusCode::NOT_FOUND, "save the conversation before its audio".to_string()));
        };
        for p in audio_files(&dir, &id) {
            let _ = std::fs::remove_file(p);
        }
        let ext = ext_for_mime(&mime);
        let file = format!("{id}.{ext}");
        write_atomic(&dir.join(&file), &body)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("could not store audio: {e}")))?;
        rec["audio"] = json!({
            "file": file,
            "mime": mime,
            "bytes": body.len(),
            "has_worker_voice": has_worker_voice,
        });
        rec["updated_at"] = json!(now);
        let bytes = serde_json::to_vec_pretty(&rec)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        write_atomic(&json_path, &bytes)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(rec)
    })
    .await;
    match res {
        Ok(Ok(rec)) => {
            tracing::info!(worker = %w, id = %rec["id"].as_str().unwrap_or(""),
                bytes = rec["audio"]["bytes"].as_u64().unwrap_or(0), "[converse] audio stored");
            Json(decorate(rec, &w, days, now)).into_response()
        }
        Ok(Err((s, m))) => err(s, m),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_audio(AxPath((worker, id)): AxPath<(String, String)>) -> Response {
    if !valid_worker(&worker) || !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    }
    let home = crate::config::amux_home();
    let (days, _) = retention_days(&home);
    let dir = store_root(&home).join(&worker);
    let Some(rec) = read_record(&dir.join(format!("{id}.json"))) else {
        return err(StatusCode::NOT_FOUND, "no such conversation");
    };
    if expires_at(rec["started_at"].as_i64().unwrap_or(0), days) <= now_ms() {
        return err(StatusCode::GONE, "this conversation has expired");
    }
    let Some(file) = rec["audio"]["file"].as_str().filter(|f| !f.contains('/') && !f.contains("..")) else {
        return err(StatusCode::NOT_FOUND, "this conversation has no recorded audio");
    };
    let mime = rec["audio"]["mime"].as_str().unwrap_or("application/octet-stream").to_string();
    match tokio::fs::read(dir.join(file)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "private, max-age=3600".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("inline; filename=\"{file}\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, format!("audio file missing: {e}")),
    }
}

// ---- retention sweep --------------------------------------------------------

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SweepReport {
    pub measured: bool,
    pub n_considered: usize,
    pub removed: usize,
    pub files_removed: usize,
    pub bytes_freed: u64,
    pub kept: usize,
    pub unreadable: usize,
    pub retention_days: u32,
    pub verdict: &'static str,
    pub why_unmeasured: Option<String>,
}

/// Delete every conversation past retention under `root`. Pure filesystem, so
/// it is testable against a temp dir with an injected clock.
pub(crate) fn sweep_dir(root: &Path, days: u32, now: i64) -> SweepReport {
    let mut rep = SweepReport { retention_days: days, ..Default::default() };
    let workers = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            rep.measured = true;
            rep.verdict = "no_store";
            return rep;
        }
        Err(e) => {
            rep.verdict = "unmeasured";
            rep.why_unmeasured = Some(format!("could not read {}: {e}", root.display()));
            return rep;
        }
    };
    rep.measured = true;
    let cutoff = std::time::SystemTime::now()
        .checked_sub(Duration::from_millis((i64::from(days) * DAY_MS) as u64));
    for wdir in workers.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(rd) = std::fs::read_dir(&wdir) else { continue };
        let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        let mut live_ids = std::collections::HashSet::new();
        for p in entries.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json")) {
            rep.n_considered += 1;
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let Some(rec) = read_record(p) else {
                rep.unreadable += 1;
                live_ids.insert(id);
                continue;
            };
            if expires_at(rec["started_at"].as_i64().unwrap_or(0), days) > now {
                rep.kept += 1;
                live_ids.insert(id);
                continue;
            }
            let mut freed = 0u64;
            let mut files = 0usize;
            for f in std::iter::once(p.clone()).chain(audio_files(&wdir, &id)) {
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&f).is_ok() {
                    freed += size;
                    files += 1;
                }
            }
            rep.removed += 1;
            rep.files_removed += files;
            rep.bytes_freed += freed;
        }
        // Audio whose record is gone (a crash between the two writes) and is
        // itself older than retention.
        for p in entries.iter().filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            ext != "json" && !ext.starts_with("tmp-")
        }) {
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if live_ids.contains(id) {
                continue;
            }
            let old = match (std::fs::metadata(p).and_then(|m| m.modified()), cutoff) {
                (Ok(m), Some(c)) => m < c,
                _ => false,
            };
            if old {
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(p).is_ok() {
                    rep.files_removed += 1;
                    rep.bytes_freed += size;
                }
            }
        }
    }
    rep.verdict = if rep.removed > 0 || rep.files_removed > 0 { "removed" } else { "nothing_expired" };
    rep
}

/// One sweep with its log line. The `converse-retention` job and the
/// "Delete expired now" button both come through here, so both are logged.
pub async fn sweep(trigger: &'static str) -> SweepReport {
    let home = crate::config::amux_home();
    let (days, _) = retention_days(&home);
    let root = store_root(&home);
    let now = now_ms();
    let rep = tokio::task::spawn_blocking(move || sweep_dir(&root, days, now))
        .await
        .unwrap_or_else(|e| SweepReport {
            verdict: "unmeasured",
            why_unmeasured: Some(format!("sweep task failed: {e}")),
            retention_days: days,
            ..Default::default()
        });
    if rep.measured {
        tracing::info!(
            verdict = rep.verdict, measured = true, n_considered = rep.n_considered,
            removed = rep.removed, files_removed = rep.files_removed, bytes_freed = rep.bytes_freed,
            kept = rep.kept, unreadable = rep.unreadable, retention_days = days, trigger,
            "[converse] retention sweep verdict={} removed={} of {}", rep.verdict, rep.removed, rep.n_considered
        );
    } else {
        tracing::warn!(
            verdict = rep.verdict, measured = false, trigger,
            why = rep.why_unmeasured.as_deref().unwrap_or(""),
            "[converse] retention sweep could not run"
        );
    }
    rep
}

async fn sweep_now(State(_state): State<AppState>) -> Response {
    let rep = sweep("manual").await;
    let status = if rep.measured { StatusCode::OK } else { StatusCode::INTERNAL_SERVER_ERROR };
    (status, Json(json!(rep))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(dir: &Path, worker: &str, id: &str, started: i64, audio: bool) {
        let wdir = dir.join(worker);
        std::fs::create_dir_all(&wdir).unwrap();
        let mut v = json!({"id": id, "worker": worker, "started_at": started, "turns": []});
        if audio {
            std::fs::write(wdir.join(format!("{id}.webm")), b"abcd").unwrap();
            v["audio"] = json!({"file": format!("{id}.webm"), "mime": "audio/webm", "bytes": 4});
        }
        std::fs::write(wdir.join(format!("{id}.json")), v.to_string()).unwrap();
    }

    #[test]
    fn sweep_removes_only_expired_conversations_and_their_audio() {
        let tmp = tempfile::tempdir().unwrap();
        let now = 1_800_000_000_000i64;
        rec(tmp.path(), "w1", "old-conv-1", now - 31 * DAY_MS, true);
        rec(tmp.path(), "w1", "new-conv-1", now - 2 * DAY_MS, true);
        rec(tmp.path(), "w2", "edge-conv-1", now - 30 * DAY_MS + 1000, false);
        let rep = sweep_dir(tmp.path(), 30, now);
        assert!(rep.measured);
        assert_eq!(rep.n_considered, 3);
        assert_eq!(rep.removed, 1, "{rep:?}");
        assert_eq!(rep.files_removed, 2, "json and audio of the expired one");
        assert_eq!(rep.kept, 2);
        assert_eq!(rep.verdict, "removed");
        assert!(!tmp.path().join("w1/old-conv-1.json").exists());
        assert!(!tmp.path().join("w1/old-conv-1.webm").exists());
        assert!(tmp.path().join("w1/new-conv-1.webm").exists());
        // A second sweep has nothing to do and says so.
        let again = sweep_dir(tmp.path(), 30, now);
        assert_eq!(again.removed, 0);
        assert_eq!(again.verdict, "nothing_expired");
    }

    #[test]
    fn a_missing_store_is_measured_empty_not_unmeasured() {
        let tmp = tempfile::tempdir().unwrap();
        let rep = sweep_dir(&tmp.path().join("nope"), 30, now_ms());
        assert!(rep.measured);
        assert_eq!(rep.n_considered, 0);
        assert_eq!(rep.verdict, "no_store");
    }

    #[test]
    fn expiry_rounds_up_and_listing_hides_expired() {
        assert_eq!(expires_in_days(DAY_MS * 3, DAY_MS * 3 - 1), 1);
        assert_eq!(expires_in_days(DAY_MS * 3, DAY_MS), 2);
        assert_eq!(expires_in_days(DAY_MS, DAY_MS), 0);
        let tmp = tempfile::tempdir().unwrap();
        let now = 1_800_000_000_000i64;
        rec(tmp.path(), "w", "old-conv-2", now - 40 * DAY_MS, false);
        rec(tmp.path(), "w", "new-conv-2", now - DAY_MS, true);
        let l = list_dir(&tmp.path().join("w"), "w", 30, now).unwrap();
        assert_eq!(l.n_considered, 2);
        assert_eq!(l.expired, 1);
        assert_eq!(l.rows.len(), 1);
        assert_eq!(l.rows[0]["expires_in_days"], json!(29));
        assert_eq!(
            l.rows[0]["audio"]["url"],
            json!("/api/converse/w/conversations/new-conv-2/audio")
        );
    }

    #[test]
    fn records_are_cleaned_and_server_fields_are_kept() {
        let now = 1_800_000_000_000i64;
        let body = json!({
            "title": "", "engine": "browser", "started_at": now + 99_999,
            "turns": [
                {"role": "you", "text": "  hello there ", "t_ms": 0},
                {"role": "assistant", "text": "hi", "t_ms": 1200},
                {"role": "user", "text": "   ", "t_ms": 1300},
            ],
        });
        let prev = json!({"created_at": 5, "audio": {"file": "x.webm"}});
        let r = build_record("w", "abcdef", &body, Some(&prev), now);
        assert_eq!(r["started_at"], json!(now), "a future start is clamped to now");
        assert_eq!(r["title"], json!("hello there"));
        assert_eq!(r["turns"].as_array().unwrap().len(), 2);
        assert_eq!(r["turns"][0]["role"], json!("user"));
        assert_eq!(r["turns"][1]["role"], json!("worker"));
        assert_eq!(r["created_at"], json!(5));
        assert_eq!(r["audio"]["file"], json!("x.webm"));
        assert_eq!(r["duration_ms"], json!(1200));
    }

    #[test]
    fn names_are_validated() {
        assert!(valid_worker("amux-frustrations"));
        assert!(valid_worker("w@chat"));
        assert!(!valid_worker("../etc"));
        assert!(!valid_worker("a/b"));
        assert!(!valid_worker(""));
        assert!(valid_id("cv-123456"));
        assert!(!valid_id("short"));
        assert!(!valid_id("has.dot.inside"));
        assert_eq!(ext_for_mime("audio/webm;codecs=opus"), "webm");
        assert_eq!(ext_for_mime("audio/mp4"), "m4a");
    }
}
