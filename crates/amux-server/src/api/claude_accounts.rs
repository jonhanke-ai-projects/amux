//! Several Claude Code accounts on one amux, at the same time (fork issue #1).
//!
//! Upstream runs every Claude worker on the ONE machine-wide login
//! (`~/.claude.json` + the default credential); `scripts/claude-rotate.py`
//! switches that one login between accounts, so only one account is ever in
//! use. Here each named account is its own `CLAUDE_CONFIG_DIR`:
//!
//! ```text
//! ~/.amux/claude-accounts/<name>/
//!     .claude.json          this account's login + preferences (private)
//!     .credentials.json     (Linux) this account's token; macOS keeps it in a
//!                           keychain item keyed by the config dir (private)
//!     projects -> ~/.claude/projects      shared
//!     settings.json -> ~/.claude/settings.json   shared (amux hooks live here)
//!     skills, agents, commands, plugins, CLAUDE.md, ...  shared
//! ```
//!
//! Sharing `projects/` is what makes a move between accounts cheap: every
//! account sees every transcript, so a worker relaunched on another account
//! resumes the same conversation, and amux's own transcript readers
//! (`claude_home()`) keep finding it in `~/.claude/projects`.
//!
//! # Binding
//!
//! `AMUX_CLAUDE_ACCOUNT=<name>` in the scope env files: a worker's
//! `sessions/<w>.env`, or a group's `env/<group>.env` so a whole project runs
//! on one account. Resolution is [`super::session_verbs::scoped_setting_in`]
//! (worker > group > global), the same rule every other scoped key uses. No
//! binding, or a name with no account directory, launches exactly as upstream.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::session_verbs::{self, EnvFile};
use super::AppState;

/// The scope key that binds a worker or group to an account.
pub const KEY: &str = "AMUX_CLAUDE_ACCOUNT";

/// Entries of `~/.claude` every account shares. Everything else in an account
/// directory (login, caches, daemon state) stays private to that account.
const SHARED_DIRS: &[&str] = &[
    "projects",
    "file-history",
    "todos",
    "plans",
    "session-env",
    "shell-snapshots",
    "skills",
    "agents",
    "commands",
    "plugins",
    "hooks",
    "output-styles",
    "agent-memory",
];
const SHARED_FILES: &[&str] = &["settings.json", "settings.local.json", "CLAUDE.md", "history.jsonl"];

/// Login fields never copied from the default `~/.claude.json` into a new
/// account, so the new account starts signed out.
const PRIVATE_KEYS: &[&str] = &["oauthAccount", "userID", "primaryApiKey"];

pub fn root(amux_home: &Path) -> PathBuf {
    amux_home.join("claude-accounts")
}

/// Lower-case letters, digits, `-` and `_`, 1 to 40 characters. The name
/// becomes a directory and is exported into a shell, so nothing else passes.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn account_dir(amux_home: &Path, name: &str) -> Option<PathBuf> {
    valid_name(name).then(|| root(amux_home).join(name))
}

/// The config dir a lane's Claude runs under, when it is bound to an account
/// that exists. `None` means the default login, exactly as upstream.
pub fn bound_dir_in(amux_home: &Path, lane: &str) -> Option<PathBuf> {
    let name = session_verbs::scoped_setting_in(amux_home, lane, KEY)?;
    let dir = account_dir(amux_home, name.trim())?;
    if dir.is_dir() {
        Some(dir)
    } else {
        tracing::warn!(session = lane, account = %name, measured = true, n_considered = 1,
            verdict = "claude_account_missing",
            "worker is bound to a Claude account with no directory; it runs on the default login");
        None
    }
}

pub fn bound_dir(lane: &str) -> Option<PathBuf> {
    bound_dir_in(&session_verbs::home(), lane)
}

/// The account's own `.claude.json` (Claude Code keeps it inside
/// `CLAUDE_CONFIG_DIR` when that is set).
pub fn claude_json(dir: &Path) -> PathBuf {
    dir.join(".claude.json")
}

/// (signed in, email) for an account directory, from its `.claude.json`.
pub fn login_of(dir: &Path) -> (bool, Option<String>) {
    let v = std::fs::read_to_string(claude_json(dir))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(Value::Null);
    let acct = &v["oauthAccount"];
    let signed_in = super::py_truthy(acct);
    let email = acct["emailAddress"].as_str().map(str::to_string);
    (signed_in, email)
}

/// Create (or repair) an account directory. Idempotent: an entry that already
/// exists is left alone, so re-running never replaces a login or a file the
/// user put there.
pub fn ensure(amux_home: &Path, user_home: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = account_dir(amux_home, name)
        .ok_or_else(|| format!("invalid account name {name:?}: use a-z, 0-9, - and _ (max 40)"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let shared = user_home.join(".claude");
    for d in SHARED_DIRS {
        let _ = std::fs::create_dir_all(shared.join(d));
        link_shared(&shared.join(d), &dir.join(d))?;
    }
    for f in SHARED_FILES {
        link_shared(&shared.join(f), &dir.join(f))?;
    }
    seed_claude_json(&user_home.join(".claude.json"), &claude_json(&dir))?;
    Ok(dir)
}

fn link_shared(target: &Path, link: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(link).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("link {} -> {}: {e}", link.display(), target.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Ok(())
    }
}

/// A new account's `.claude.json` starts as the default one (MCP servers,
/// folder trust, onboarding done) minus the login, so its first worker starts
/// at the composer once the account is signed in.
fn seed_claude_json(default: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        return Ok(());
    }
    let mut doc = std::fs::read_to_string(default)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    strip_login(&mut doc);
    let body = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(dst, body).map_err(|e| format!("write {}: {e}", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn strip_login(doc: &mut Value) {
    if let Some(o) = doc.as_object_mut() {
        for k in PRIVATE_KEYS {
            o.remove(*k);
        }
    }
}

/// Every account directory, with its login and who is bound to it.
pub fn list(amux_home: &Path) -> Vec<Value> {
    let mut names: Vec<String> = std::fs::read_dir(root(amux_home))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| valid_name(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let bindings = bindings(amux_home);
    names
        .into_iter()
        .map(|n| {
            let dir = root(amux_home).join(&n);
            let (signed_in, email) = login_of(&dir);
            let (workers, groups): (Vec<_>, Vec<_>) = bindings
                .iter()
                .filter(|(_, _, a)| *a == n)
                .partition(|(level, _, _)| *level == "worker");
            json!({
                "name": n,
                "config_dir": dir.to_string_lossy(),
                "signed_in": signed_in,
                "email": email,
                "workers": workers.iter().map(|(_, w, _)| w).collect::<Vec<_>>(),
                "groups": groups.iter().map(|(_, g, _)| g).collect::<Vec<_>>(),
                "login_command": login_command(&dir),
            })
        })
        .collect()
}

/// (level, name, account) for every worker and group env file that sets [`KEY`].
fn bindings(amux_home: &Path) -> Vec<(&'static str, String, String)> {
    let mut out = Vec::new();
    for (level, sub) in [("worker", "sessions"), ("group", "env")] {
        let Ok(rd) = std::fs::read_dir(amux_home.join(sub)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Some(stem) = p.file_name().and_then(|s| s.to_str()).and_then(|s| s.strip_suffix(".env")) else {
                continue;
            };
            if let Some(a) = EnvFile::load(&p).get(KEY).map(str::trim).filter(|a| !a.is_empty()) {
                out.push((level, stem.to_string(), a.to_string()));
            }
        }
    }
    out.sort();
    out
}

/// The command a person runs once to sign an account in.
pub fn login_command(dir: &Path) -> String {
    format!(
        "CLAUDE_CONFIG_DIR={} claude auth login",
        session_verbs::sh_quote(&dir.to_string_lossy())
    )
}

/// Undo an earlier [`launch_exports`] in a shell that survives a restart. Only
/// a `CLAUDE_CONFIG_DIR` amux set is removed (the marker says so); one the
/// owner exports in their own profile is left alone.
pub const UNDO_EXPORTS: &str =
    "if [ \"${AMUX_CLAUDE_CONFIG_DERIVED:-}\" = 1 ]; then unset CLAUDE_CONFIG_DIR AMUX_CLAUDE_CONFIG_DERIVED; fi; ";

/// Shell for a lane bound to `dir`: route Claude to the account, and drop the
/// one credential that would override it (an env token beats the config dir).
pub fn launch_exports(dir: &Path) -> String {
    format!(
        "export CLAUDE_CONFIG_DIR={} AMUX_CLAUDE_CONFIG_DERIVED=1; unset CLAUDE_CODE_OAUTH_TOKEN; ",
        session_verbs::sh_quote(&dir.to_string_lossy())
    )
}

// ---------------------------------------------------------------------------
// Per-account usage (the status-line tap)
// ---------------------------------------------------------------------------

/// The tap Claude Code runs as its status line: records `rate_limits` for the
/// account that session is signed in to, then runs the owner's own status line.
/// One copy, compiled in, written out by [`track_on`]. It reads no credential.
pub const TAP_SOURCE: &str = include_str!("../../../../scripts/claude-account-usage-tap.py");
const TAP_FILE: &str = "claude-account-usage-tap.py";

pub fn tap_path(amux_home: &Path) -> PathBuf {
    amux_home.join("bin").join(TAP_FILE)
}

fn settings_path(user_home: &Path) -> PathBuf {
    let p = user_home.join(".claude").join("settings.json");
    // Write the real file, never replace a symlink with a copy.
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn load_settings(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str::<Value>(&t)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| format!("{} is not a JSON object; left untouched", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Only `statusLine` ever changes; every other key (hooks included) is written
/// back exactly as read, after a timestamped backup.
fn write_settings(path: &Path, doc: &Value) -> Result<(), String> {
    if path.exists() {
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let bak = path.with_file_name(format!("settings.json.bak-amux-tap-{stamp}"));
        std::fs::copy(path, &bak).map_err(|e| format!("backup {}: {e}", bak.display()))?;
    }
    let body = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    let tmp = session_verbs::unique_tmp_path(path);
    std::fs::write(&tmp, body)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// The status-line command, split into (tap installed, the owner's own command).
fn split_command(cmd: &str, tap: &Path) -> (bool, String) {
    let prefix = format!("{} --", session_verbs::sh_quote(&tap.to_string_lossy()));
    match cmd.trim().strip_prefix(&prefix) {
        Some(rest) => (true, rest.trim().to_string()),
        None => (false, cmd.trim().to_string()),
    }
}

/// (tap installed, the owner's own status-line command) as the settings have it now.
pub fn tracking(amux_home: &Path, user_home: &Path) -> (bool, String) {
    let doc = load_settings(&settings_path(user_home)).unwrap_or_else(|_| json!({}));
    split_command(doc["statusLine"]["command"].as_str().unwrap_or(""), &tap_path(amux_home))
}

/// Install the tap in front of the existing status line. Idempotent; the
/// script is rewritten each time so it tracks the server's copy.
pub fn track_on(amux_home: &Path, user_home: &Path) -> Result<(), String> {
    let path = settings_path(user_home);
    let mut doc = load_settings(&path)?;
    let tap = tap_path(amux_home);
    if let Some(dir) = tap.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(&tap, TAP_SOURCE).map_err(|e| format!("write {}: {e}", tap.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tap, std::fs::Permissions::from_mode(0o755));
    }
    let (on, own) = split_command(doc["statusLine"]["command"].as_str().unwrap_or(""), &tap);
    if on {
        return Ok(());
    }
    let cmd = format!("{} -- {own}", session_verbs::sh_quote(&tap.to_string_lossy()));
    let line = doc
        .as_object_mut()
        .expect("load_settings returns an object")
        .entry("statusLine")
        .or_insert_with(|| json!({}));
    if !line.is_object() {
        *line = json!({});
    }
    line["type"] = json!("command");
    line["command"] = json!(cmd.trim());
    write_settings(&path, &doc)
}

/// Put the owner's own status line back (or none, if there was none).
pub fn track_off(amux_home: &Path, user_home: &Path) -> Result<(), String> {
    let path = settings_path(user_home);
    let mut doc = load_settings(&path)?;
    let (on, own) = split_command(doc["statusLine"]["command"].as_str().unwrap_or(""), &tap_path(amux_home));
    if !on {
        return Ok(());
    }
    let o = doc.as_object_mut().expect("load_settings returns an object");
    if own.is_empty() {
        o.remove("statusLine");
    } else {
        o["statusLine"]["command"] = json!(own);
    }
    write_settings(&path, &doc)
}

fn epoch_of(ts: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts.as_str()?.trim()).ok().map(|d| d.timestamp())
}

/// Which account's spare capacity expires first: among accounts whose latest
/// reading has weekly headroom (under `full_pct`), a usable 5-hour window and a
/// weekly reset still ahead, the one that resets soonest. Its unused share is
/// lost at that reset, so it is the one to spend on surplus work. `usage` rows
/// are `usage_accounts::summary` accounts. Returns the account email.
pub fn spend_first(usage: &[Value], now: i64, full_pct: f64) -> Option<String> {
    usage
        .iter()
        .filter_map(|a| {
            let latest = &a["latest"];
            let reset = epoch_of(&latest["seven_day_resets_at"]).filter(|r| *r > now)?;
            let pct = latest["seven_day_pct"].as_f64()?;
            let five_ok = latest["five_hour_pct"].as_f64().is_none_or(|p| {
                p < full_pct || epoch_of(&latest["five_hour_resets_at"]).is_some_and(|r| r <= now)
            });
            (pct < full_pct && five_ok).then(|| (reset, a["account"].as_str().map(str::to_string)))
        })
        .filter_map(|(reset, email)| Some((reset, email?)))
        .min()
        .map(|(_, email)| email)
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn user_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"ok": false, "error": msg.into()}))).into_response()
}

/// `GET /api/claude-accounts`
pub async fn get_accounts(State(state): State<AppState>) -> Response {
    let home = session_verbs::home();
    let now = super::usage_accounts::now();
    let usage = state
        .store
        .read_async(move |c| Ok(super::usage_accounts::summary(c, 8.0 * 86400.0, None, 0, now)?))
        .await;
    let (usage_measured, usage_rows, usage_why) = match usage {
        Ok(v) => (true, v["accounts"].as_array().cloned().unwrap_or_default(), None),
        Err(e) => (false, vec![], Some(e.to_string())),
    };
    let usage_of = |email: Option<&str>| -> Value {
        email
            .and_then(|e| usage_rows.iter().find(|a| a["account"] == e))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let (default_signed_in, default_email) = {
        let v = std::fs::read_to_string(user_home().join(".claude.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        (super::py_truthy(&v["oauthAccount"]), v["oauthAccount"]["emailAddress"].as_str().map(str::to_string))
    };
    let mut accounts = list(&home);
    for a in &mut accounts {
        a["usage"] = usage_of(a["email"].as_str());
    }
    let (tracking_on, _) = tracking(&home, &user_home());
    Json(json!({
        "ok": true,
        "key": KEY,
        "default": {"signed_in": default_signed_in, "email": default_email,
                    "usage": usage_of(default_email.as_deref())},
        "accounts": accounts,
        "spend_first": spend_first(&usage_rows, now as i64, 95.0),
        "tracking": tracking_on,
        "measured": usage_measured,
        "n_considered": usage_rows.len(),
        "why_unmeasured": usage_why,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct TrackBody {
    pub on: bool,
}

/// `POST /api/claude-accounts/track {on}`: put the usage tap in front of the
/// Claude Code status line (or take it out). The owner's explicit step: it
/// changes `statusLine` in ~/.claude/settings.json, after a timestamped backup.
pub async fn post_track(Json(b): Json<TrackBody>) -> Response {
    let (home, user) = (session_verbs::home(), user_home());
    let res = if b.on { track_on(&home, &user) } else { track_off(&home, &user) };
    match res {
        Ok(()) => {
            let (on, own) = tracking(&home, &user);
            tracing::info!(on, measured = true, n_considered = 1, verdict = "claude_usage_tap_set",
                "Claude account usage tap changed");
            Json(json!({"ok": true, "tracking": on, "your_status_line": own,
                        "tap": tap_path(&home).to_string_lossy()}))
            .into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Debug, Deserialize)]
pub struct AddBody {
    pub name: String,
}

/// `POST /api/claude-accounts {name}`: create the account directory. Signing
/// in is the owner's own step (`login_command` in the reply): it opens a
/// browser for that account, which the server must not do on its own.
pub async fn post_account(Json(b): Json<AddBody>) -> Response {
    let name = b.name.trim().to_string();
    match ensure(&session_verbs::home(), &user_home(), &name) {
        Ok(dir) => {
            let (signed_in, email) = login_of(&dir);
            Json(json!({
                "ok": true,
                "name": name,
                "config_dir": dir.to_string_lossy(),
                "signed_in": signed_in,
                "email": email,
                "login_command": login_command(&dir),
            }))
            .into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Debug, Deserialize)]
pub struct AssignBody {
    /// A worker name, or a group name with `level: "group"`.
    pub target: String,
    /// `worker` (default) or `group`.
    #[serde(default)]
    pub level: Option<String>,
    /// Account name; empty or absent clears the binding (back to the default login).
    #[serde(default)]
    pub account: Option<String>,
    /// Relaunch affected running workers now (at an idle boundary). Default true.
    #[serde(default)]
    pub relaunch: Option<bool>,
}

/// `POST /api/claude-accounts/assign`: bind a worker or a group to an account
/// (or clear it), then relaunch the running Claude workers it affects so they
/// continue the same conversation on the new account.
pub async fn post_assign(State(state): State<AppState>, Json(b): Json<AssignBody>) -> Response {
    let home = session_verbs::home();
    let level = b.level.as_deref().unwrap_or("worker");
    let target = b.target.trim().to_lowercase();
    if target.is_empty() || !target.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) {
        return err(StatusCode::BAD_REQUEST, format!("invalid target {:?}", b.target));
    }
    let account = b.account.as_deref().map(str::trim).filter(|a| !a.is_empty());
    if let Some(a) = account {
        match account_dir(&home, a) {
            Some(d) if d.is_dir() => {}
            _ => return err(StatusCode::NOT_FOUND, format!("no Claude account {a:?}; add it first")),
        }
    }
    let file = match level {
        "worker" => {
            let f = session_verbs::env_path(&target);
            if !f.exists() {
                return err(StatusCode::NOT_FOUND, format!("no worker {target:?}"));
            }
            f
        }
        "group" => home.join("env").join(format!("{target}.env")),
        _ => return err(StatusCode::BAD_REQUEST, "level must be worker or group"),
    };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut env = EnvFile::load(&file);
    match account {
        Some(a) => env.set(KEY, a),
        None => env.remove(KEY),
    }
    if let Err(e) = env.write(&file) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("write {}: {e}", file.display()));
    }
    let affected: Vec<String> = match level {
        "worker" => vec![target.clone()],
        _ => session_verbs::running_workers()
            .await
            .into_iter()
            .filter(|w| session_verbs::lane_groups(w).contains(&target))
            .collect(),
    };
    let mut moved = Vec::new();
    if b.relaunch.unwrap_or(true) {
        for w in &affected {
            moved.push(relaunch(&state, w).await);
        }
    }
    tracing::info!(target = %target, level, account = ?account, measured = true,
        n_considered = affected.len(), verdict = "claude_account_assigned",
        "Claude account binding changed");
    Json(json!({"ok": true, "target": target, "level": level, "account": account, "relaunched": moved}))
        .into_response()
}

/// Stop a running Claude worker at an idle boundary and start it again; the
/// start resumes the same conversation, now under the new binding.
async fn relaunch(state: &AppState, name: &str) -> Value {
    let cfg = session_verbs::parse_env(name);
    if session_verbs::provider_of(&cfg) != "claude" {
        return json!({"worker": name, "relaunched": false, "why": "not a Claude worker"});
    }
    if !session_verbs::is_running(name).await {
        return json!({"worker": name, "relaunched": false, "why": "not running; the next start uses the new account"});
    }
    let stopped = {
        let lock = session_verbs::session_op_lock(name);
        let _held = lock.lock().await;
        session_verbs::stop_verified_worker(state, name).await
    };
    if let Err(why) = stopped {
        return json!({"worker": name, "relaunched": false,
            "why": format!("not at an idle boundary ({why}); the next restart uses the new account")});
    }
    let (ok, detail) = session_verbs::start_session_as("claude-account-move", state, name, "", false).await;
    json!({"worker": name, "relaunched": ok, "detail": detail})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("amux-ca-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_are_shell_and_path_safe() {
        assert!(valid_name("work"));
        assert!(valid_name("max-2_b"));
        for bad in ["", "Work", "a b", "../x", "a;rm", "a/b", &"x".repeat(41)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn ensure_shares_state_and_starts_signed_out() {
        let base = tmp("ensure");
        let (amux, user) = (base.join("amux"), base.join("user"));
        std::fs::create_dir_all(user.join(".claude/projects")).unwrap();
        std::fs::write(user.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(
            user.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@x"},"userID":"u","hasCompletedOnboarding":true,"mcpServers":{"m":{}}}"#,
        )
        .unwrap();
        let dir = ensure(&amux, &user, "second").unwrap();
        assert_eq!(std::fs::read_link(dir.join("projects")).unwrap(), user.join(".claude/projects"));
        assert_eq!(std::fs::read_link(dir.join("settings.json")).unwrap(), user.join(".claude/settings.json"));
        let seeded: Value = serde_json::from_str(&std::fs::read_to_string(claude_json(&dir)).unwrap()).unwrap();
        assert!(seeded.get("oauthAccount").is_none() && seeded.get("userID").is_none());
        assert_eq!(seeded["hasCompletedOnboarding"], true);
        assert!(seeded["mcpServers"]["m"].is_object());
        assert_eq!(login_of(&dir), (false, None));

        // Idempotent: a later login survives a re-run.
        std::fs::write(claude_json(&dir), r#"{"oauthAccount":{"emailAddress":"two@x"}}"#).unwrap();
        ensure(&amux, &user, "second").unwrap();
        assert_eq!(login_of(&dir), (true, Some("two@x".into())));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn binding_resolves_worker_over_group_and_needs_a_directory() {
        let base = tmp("bind");
        let amux = base.join("amux");
        std::fs::create_dir_all(amux.join("sessions")).unwrap();
        std::fs::create_dir_all(amux.join("env")).unwrap();
        std::fs::write(amux.join("sessions/w1.env"), "CC_TAGS=\"proj\"\n").unwrap();
        std::fs::write(amux.join("env/proj.env"), format!("{KEY}=\"team\"\n")).unwrap();
        // Bound by group, but no such account yet: default login.
        assert_eq!(bound_dir_in(&amux, "w1"), None);
        std::fs::create_dir_all(root(&amux).join("team")).unwrap();
        assert_eq!(bound_dir_in(&amux, "w1"), Some(root(&amux).join("team")));
        // The worker's own binding wins over its group's.
        std::fs::create_dir_all(root(&amux).join("solo")).unwrap();
        std::fs::write(amux.join("sessions/w1.env"), format!("CC_TAGS=\"proj\"\n{KEY}=\"solo\"\n")).unwrap();
        assert_eq!(bound_dir_in(&amux, "w1"), Some(root(&amux).join("solo")));
        let listed = list(&amux);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "solo");
        assert_eq!(listed[0]["workers"], json!(["w1"]));
        assert_eq!(listed[1]["groups"], json!(["proj"]));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn track_wraps_the_owners_status_line_and_restores_it() {
        let base = tmp("track");
        let (amux, user) = (base.join("amux"), base.join("user"));
        std::fs::create_dir_all(user.join(".claude")).unwrap();
        let settings = user.join(".claude").join("settings.json");
        let own = "python3 /x/overhead.py statusline";
        std::fs::write(
            &settings,
            json!({"statusLine": {"type": "command", "command": own, "padding": 1}, "hooks": {"a": 1}}).to_string(),
        )
        .unwrap();
        track_on(&amux, &user).unwrap();
        track_on(&amux, &user).unwrap(); // idempotent: wrapped once
        assert_eq!(tracking(&amux, &user), (true, own.to_string()));
        assert_eq!(std::fs::read_to_string(tap_path(&amux)).unwrap(), TAP_SOURCE);
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(doc["statusLine"]["padding"], 1);
        assert_eq!(doc["hooks"]["a"], 1, "every other key survives");
        assert_eq!(doc["statusLine"]["command"].as_str().unwrap().matches(" -- ").count(), 1);
        track_off(&amux, &user).unwrap();
        assert_eq!(tracking(&amux, &user), (false, own.to_string()));
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["a"], 1);
        let backups = std::fs::read_dir(user.join(".claude"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("settings.json.bak-amux-tap-"))
            .count();
        assert!(backups >= 1);

        // No status line before: none after.
        std::fs::write(&settings, "{}").unwrap();
        track_on(&amux, &user).unwrap();
        track_off(&amux, &user).unwrap();
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(doc.get("statusLine").is_none());

        // A settings file that is not JSON is never overwritten.
        std::fs::write(&settings, "not json").unwrap();
        assert!(track_on(&amux, &user).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn spend_first_picks_the_soonest_reset_with_headroom() {
        let now = 1_793_000_000_i64;
        let at = |h: i64| chrono::DateTime::from_timestamp(now + h * 3600, 0).unwrap().to_rfc3339();
        let acct = |email: &str, wk: f64, wk_reset_h: i64, five: f64, five_reset_h: i64| {
            json!({"account": email, "latest": {
                "seven_day_pct": wk, "seven_day_resets_at": at(wk_reset_h),
                "five_hour_pct": five, "five_hour_resets_at": at(five_reset_h)}})
        };
        let rows = vec![
            acct("late@x", 10.0, 120, 0.0, 2),
            acct("soon@x", 40.0, 20, 10.0, 2),
            acct("full@x", 99.0, 5, 0.0, 2),     // no weekly headroom
            acct("past@x", 10.0, -1, 0.0, 2),    // weekly reset already passed: stale, skipped
            acct("burst@x", 10.0, 10, 100.0, 3), // 5-hour window full until it resets
        ];
        assert_eq!(spend_first(&rows, now, 95.0).as_deref(), Some("soon@x"));
        // Once its 5-hour window has reset, it is usable again and resets first.
        let rows2 = vec![acct("soon@x", 40.0, 20, 0.0, 2), acct("burst@x", 10.0, 10, 100.0, -1)];
        assert_eq!(spend_first(&rows2, now, 95.0).as_deref(), Some("burst@x"));
        assert_eq!(spend_first(&[], now, 95.0), None);
    }

    #[test]
    fn launch_exports_route_and_drop_the_env_token() {
        let s = launch_exports(Path::new("/h/.amux/claude-accounts/a"));
        assert!(s.contains("export CLAUDE_CONFIG_DIR="));
        assert!(s.contains("unset CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(s.contains("AMUX_CLAUDE_CONFIG_DERIVED=1"));
    }

    /// The undo really undoes our export, and only ours (run in sh).
    #[test]
    fn undo_clears_only_an_amux_export() {
        let run = |pre: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{pre}{UNDO_EXPORTS}printf %s \"${{CLAUDE_CONFIG_DIR:-none}}\""))
                .env_remove("CLAUDE_CONFIG_DIR")
                .env_remove("AMUX_CLAUDE_CONFIG_DERIVED")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        assert_eq!(run(&launch_exports(Path::new("/a"))), "none");
        assert_eq!(run("export CLAUDE_CONFIG_DIR=/mine; "), "/mine");
    }
}
