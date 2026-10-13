//! Keep the firehose search index (`search_firehose`) current.
//!
//! Every tick indexes up to a byte budget of new log and transcript bytes. The
//! budget bounds one tick's work: the first build on a machine with 1.6 GB of
//! logs takes several ticks instead of one long stall, and while it runs
//! `/api/log-search` still answers completely by scanning the unindexed tail.

use std::time::Duration;

/// `AMUX_SEARCH_INGEST=0` stops the job from being spawned.
pub fn enabled() -> bool {
    std::env::var("AMUX_SEARCH_INGEST")
        .map(|v| v.trim() != "0")
        .unwrap_or(true)
}

fn tick_secs() -> u64 {
    std::env::var("AMUX_SEARCH_INGEST_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10)
}

fn budget_bytes() -> u64 {
    std::env::var("AMUX_SEARCH_INGEST_BUDGET_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(64)
        * 1024
        * 1024
}

pub fn spawn() {
    if !enabled() {
        tracing::info!("search ingest: disabled by AMUX_SEARCH_INGEST=0");
        return;
    }
    let interval = Duration::from_secs(tick_secs());
    let id = crate::runtime_jobs::registry::ids::SEARCH_INGEST;
    let h = tokio::spawn(async move {
        let mut t = tokio::time::interval(interval);
        loop {
            t.tick().await;
            crate::runtime_jobs::registry::tick_start(id);
            let home = crate::config::amux_home();
            let budget = budget_bytes();
            let started = std::time::Instant::now();
            match tokio::task::spawn_blocking(move || crate::search_firehose::ingest(&home, budget))
                .await
            {
                Ok(Ok(r)) if r.bytes > 0 || r.reset > 0 || r.pruned > 0 => tracing::info!(
                    target: "amux::search",
                    bytes = r.bytes, docs = r.docs, reset = r.reset, pruned = r.pruned,
                    sources = r.sources, took_ms = started.elapsed().as_millis() as u64,
                    "search ingest: indexed new log/transcript bytes"
                ),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::warn!(target: "amux::search", error = %e, "search ingest failed")
                }
                Err(e) => {
                    tracing::warn!(target: "amux::search", error = %e, "search ingest panicked")
                }
            }
            crate::runtime_jobs::registry::tick_end(id);
        }
    });
    crate::runtime_jobs::registry::adopt(id, Some(interval), &h);
}
