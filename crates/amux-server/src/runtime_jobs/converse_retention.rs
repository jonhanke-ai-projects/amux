//! `converse-retention`: deletes Converse-tab conversations past
//! `AMUX_CONVERSE_RETENTION_DAYS` (default 30). The work list is the folder
//! itself (`api::converse`), so a restart or a changed retention resolves on
//! the next tick with nothing to reconcile. Each tick logs one `verdict=` line.

/// Hourly by default; `AMUX_CONVERSE_RETENTION_SECS=0` disables it. Expired
/// conversations are already hidden from every read the moment they expire,
/// so the interval only bounds how long their files outlive that.
pub fn spawn() -> super::PeriodicTask {
    let name = super::registry::ids::CONVERSE_RETENTION;
    let secs = std::env::var(super::per_job_disable_var(name))
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(3600);
    super::spawn_periodic(name, secs, || async {
        crate::api::converse::sweep("tick").await;
    })
}
