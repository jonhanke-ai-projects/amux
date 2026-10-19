//! `GET /api/palette?q=&limit=` — the Cmd-K palette's name index for the
//! two resource families too big to ship to every client: board cards and
//! schedules.
//!
//! The palette answers on every keystroke, so it cannot use `/api/search`:
//! that ranks full card text with bm25, and on the live 15.8 GB database a
//! common word ("deploy", 18k matching documents) took 1 to 14 s depending on
//! what the page cache held. A card's name, though, is 2.8 MB for all 28k
//! cards. So this module keeps (id, title, status, worker, archived, updated)
//! in memory and matches names there, in microseconds.
//!
//! Freshness: a request reads a fingerprint (row counts and the newest
//! `updated` of both tables), at most every 2 s. A changed fingerprint
//! rebuilds the index on a blocking thread while the previous index keeps
//! answering, so a hit can be seconds stale but a keystroke never waits. The
//! first request on a database builds synchronously. `index_age_ms`, `stale`
//! and `n_considered` ride on every answer, so an empty list says whether the
//! index existed (ethos rule 4).
//!
//! Workers, groups, projects, pins, connectors and the rest are small and the
//! dashboard already holds or fetches them, so the client matches those.

use super::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/palette", axum::routing::get(palette))
}

const DEFAULT_LIMIT: usize = 12;
const MAX_LIMIT: usize = 50;

#[derive(Clone, Debug)]
pub struct Entry {
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub status: String,
    pub session: String,
    pub archived: bool,
    pub updated: i64,
    /// lowercased "id title session", what tokens are matched against
    hay: String,
    title_lc: String,
    id_lc: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    cards: i64,
    cards_updated: i64,
    schedules: i64,
    schedules_updated: i64,
}

struct Index {
    entries: Arc<Vec<Entry>>,
    fp: Fingerprint,
    built: Instant,
    build_ms: u64,
}

#[derive(Default)]
struct Slot {
    index: Option<Index>,
    rebuilding: bool,
    /// The last fingerprint read, and when: typing fires a request per
    /// keystroke, and the fingerprint's COUNT took 152ms cold on the live
    /// 28k-card table, so it is re-read at most every FINGERPRINT_EVERY.
    checked: Option<(Instant, Fingerprint)>,
}

const FINGERPRINT_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// One index per database file (tests open several in one process).
fn slots() -> &'static Mutex<HashMap<usize, Arc<Mutex<Slot>>>> {
    static S: OnceLock<Mutex<HashMap<usize, Arc<Mutex<Slot>>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn fingerprint(conn: &Connection) -> rusqlite::Result<Fingerprint> {
    let (cards, cards_updated) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(updated), 0) FROM issues WHERE deleted IS NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (schedules, schedules_updated) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(updated), 0) FROM schedules WHERE deleted IS NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Fingerprint {
        cards,
        cards_updated,
        schedules,
        schedules_updated,
    })
}

fn entry(
    kind: &'static str,
    id: String,
    title: String,
    status: String,
    session: String,
    archived: bool,
    updated: i64,
) -> Entry {
    let id_lc = id.to_lowercase();
    let title_lc = title.to_lowercase();
    let hay = format!("{id_lc} {title_lc} {}", session.to_lowercase());
    Entry {
        kind,
        id,
        title,
        status,
        session,
        archived,
        updated,
        hay,
        title_lc,
        id_lc,
    }
}

pub fn build(conn: &Connection) -> rusqlite::Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut st = conn.prepare(
        "SELECT id, title, status, COALESCE(session, ''), COALESCE(archived, 0), updated
         FROM issues WHERE deleted IS NULL",
    )?;
    let rows = st.query_map([], |r| {
        Ok(entry(
            "card",
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get::<_, i64>(4)? != 0,
            r.get(5)?,
        ))
    })?;
    for e in rows {
        out.push(e?);
    }
    let mut st = conn.prepare(
        "SELECT id, title, COALESCE(enabled, 1), COALESCE(session, ''), COALESCE(updated, 0)
         FROM schedules WHERE deleted IS NULL",
    )?;
    let rows = st.query_map([], |r| {
        let enabled: i64 = r.get(2)?;
        Ok(entry(
            "schedule",
            r.get(0)?,
            r.get(1)?,
            if enabled != 0 {
                "enabled".into()
            } else {
                "paused".into()
            },
            r.get(3)?,
            false,
            r.get(4)?,
        ))
    })?;
    for e in rows {
        out.push(e?);
    }
    Ok(out)
}

fn is_open(status: &str) -> bool {
    !matches!(status, "done" | "verified" | "discarded")
}

fn word_start(hay: &str, tok: &str) -> bool {
    hay.match_indices(tok).any(|(i, _)| {
        i == 0
            || !hay[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
    })
}

/// Every token must appear in "id title worker". Ranked by: an exact or
/// prefix id hit, the title starting with the query, tokens at word starts,
/// open over closed, live over archived, then recency.
pub fn rank<'a>(entries: &'a [Entry], q: &str, now: i64) -> (Vec<(&'a Entry, i64)>, usize) {
    let q = q.trim().to_lowercase();
    let toks: Vec<&str> = q.split_whitespace().collect();
    if toks.is_empty() {
        return (Vec::new(), 0);
    }
    let mut hits: Vec<(&Entry, i64)> = entries
        .iter()
        .filter(|e| toks.iter().all(|t| e.hay.contains(t)))
        .map(|e| {
            let mut s: i64 = 0;
            if e.id_lc == q {
                s += 1000;
            } else if e.id_lc.starts_with(&q) {
                s += 300;
            }
            if e.title_lc.starts_with(&q) {
                s += 200;
            } else if e.title_lc.contains(&q) {
                s += 80;
            }
            for t in &toks {
                s += if word_start(&e.title_lc, t) { 40 } else { 10 };
            }
            if e.kind == "card" {
                if is_open(&e.status) {
                    s += 60;
                }
                if e.archived {
                    s -= 80;
                }
            } else if e.status == "enabled" {
                s += 30;
            }
            // Up to +30 for something touched today, fading over a month.
            let age_days = ((now - e.updated).max(0) as f64) / 86_400.0;
            s += (30.0 * (-age_days / 30.0).exp()) as i64;
            (e, s)
        })
        .collect();
    let total = hits.len();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.updated.cmp(&a.0.updated)));
    (hits, total)
}

#[derive(Deserialize)]
pub struct PaletteParams {
    #[serde(default)]
    q: String,
    limit: Option<usize>,
}

async fn palette(State(st): State<AppState>, Query(p): Query<PaletteParams>) -> Response {
    let started = Instant::now();
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let key = Arc::as_ptr(&st.store) as usize;
    let slot = slots().lock().unwrap().entry(key).or_default().clone();

    let store = st.store.clone();
    let recent = slot
        .lock()
        .unwrap()
        .checked
        .filter(|(at, _)| at.elapsed() < FINGERPRINT_EVERY);
    // How old the "has anything changed" check behind this answer is: within
    // that window a new card can be missing while `stale` still reads false.
    let fp_age_ms = recent
        .map(|(at, _)| at.elapsed().as_millis() as u64)
        .unwrap_or(0);
    let fp = if let Some((_, f)) = recent {
        f
    } else {
        let conn = match store.read() {
            Ok(c) => c,
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": e.to_string(), "measured": false, "why_unmeasured": "store unavailable"}))).into_response(),
        };
        match fingerprint(&conn) {
            Ok(f) => {
                slot.lock().unwrap().checked = Some((Instant::now(), f));
                f
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "measured": false, "why_unmeasured": "fingerprint query failed"}))).into_response(),
        }
    };

    let have_index = slot.lock().unwrap().index.is_some();
    if !have_index {
        // First request on this database: build now.
        let s2 = store.clone();
        let built = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<Entry>, u64)> {
            let t0 = Instant::now();
            let conn = s2.read()?;
            let e = build(&conn)?;
            Ok((e, t0.elapsed().as_millis() as u64))
        })
        .await;
        match built {
            Ok(Ok((entries, ms))) => {
                tracing::info!(target: "amux::palette", entries = entries.len(), build_ms = ms, "palette index built");
                slot.lock().unwrap().index = Some(Index { entries: Arc::new(entries), fp, built: Instant::now(), build_ms: ms });
            }
            Ok(Err(e)) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "measured": false, "why_unmeasured": "index build failed"}))).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string(), "measured": false, "why_unmeasured": "index build panicked"}))).into_response(),
        }
    }

    // Serve the index we have; refresh it behind this answer if it drifted.
    let (entries, stale, age_ms, build_ms) = {
        let mut g = slot.lock().unwrap();
        let idx = g.index.as_ref().expect("built above");
        let stale = idx.fp != fp;
        let out = (
            idx.entries.clone(),
            stale,
            idx.built.elapsed().as_millis() as u64,
            idx.build_ms,
        );
        if stale && !g.rebuilding {
            g.rebuilding = true;
            let (slot2, s2) = (slot.clone(), store.clone());
            tokio::task::spawn_blocking(move || {
                let t0 = Instant::now();
                let res = s2.read().map_err(|e| e.to_string()).and_then(|c| {
                    let f = fingerprint(&c).map_err(|e| e.to_string())?;
                    build(&c).map(|e| (e, f)).map_err(|e| e.to_string())
                });
                let mut g = slot2.lock().unwrap();
                g.rebuilding = false;
                match res {
                    Ok((entries, f)) => {
                        let ms = t0.elapsed().as_millis() as u64;
                        g.index = Some(Index {
                            entries: Arc::new(entries),
                            fp: f,
                            built: Instant::now(),
                            build_ms: ms,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(target: "amux::palette", error = %e, "verdict=palette_rebuild_failed (serving the previous index)")
                    }
                }
            });
        }
        out
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (ranked, total) = rank(&entries, &p.q, now);
    let hits: Vec<Value> = ranked
        .into_iter()
        .take(limit)
        .map(|(e, score)| {
            json!({
                "kind": e.kind, "id": e.id, "title": e.title, "status": e.status,
                "session": e.session, "archived": e.archived, "updated": e.updated, "score": score,
            })
        })
        .collect();
    Json(json!({
        "q": p.q,
        "hits": hits,
        "total": total,
        "limit": limit,
        "measured": true,
        "n_considered": entries.len(),
        "stale": stale,
        "fingerprint_age_ms": fp_age_ms,
        "index_age_ms": age_ms,
        "index_build_ms": build_ms,
        "took_ms": started.elapsed().as_millis() as u64,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(
        kind: &'static str,
        id: &str,
        title: &str,
        status: &str,
        archived: bool,
        updated: i64,
    ) -> Entry {
        entry(
            kind,
            id.into(),
            title.into(),
            status.into(),
            "lane".into(),
            archived,
            updated,
        )
    }

    #[test]
    fn every_token_must_match_and_ids_win() {
        let now = 1_790_000_000;
        let v = vec![
            e("card", "AMUX-12", "Fix the deploy gate", "todo", false, now),
            e(
                "card",
                "MG-9",
                "Deploy notes for amux",
                "done",
                false,
                now - 86_400 * 90,
            ),
            e("card", "AMUX-120", "Unrelated", "todo", false, now),
            e(
                "schedule",
                "SCHED-1",
                "nightly deploy check",
                "enabled",
                false,
                now,
            ),
        ];
        let (hits, total) = rank(&v, "deploy amux", now);
        assert_eq!(total, 2, "only the two with both words");
        assert_eq!(
            hits[0].0.id, "AMUX-12",
            "open beats a closed 90-day-old card"
        );
        let (hits, _) = rank(&v, "amux-12", now);
        assert_eq!(
            hits[0].0.id, "AMUX-12",
            "exact id first, before the AMUX-120 prefix"
        );
        assert_eq!(hits[1].0.id, "AMUX-120");
        let (hits, _) = rank(&v, "nightly", now);
        assert_eq!(hits[0].0.kind, "schedule");
        assert_eq!(rank(&v, "   ", now).1, 0, "blank query matches nothing");
    }

    #[test]
    fn archived_cards_sink_below_live_ones() {
        let now = 1_790_000_000;
        let v = vec![
            e("card", "A-1", "billing fix", "todo", true, now),
            e("card", "A-2", "billing fix", "todo", false, now),
        ];
        let (hits, _) = rank(&v, "billing", now);
        assert_eq!(hits[0].0.id, "A-2");
    }
}
