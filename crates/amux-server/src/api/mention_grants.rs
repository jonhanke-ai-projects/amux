//! Owner @-mention grants (Ethan, 2026-10-13: "make a way where i can bypass
//! intra worker communication (grouped up, etc.) but only via doing an @ with
//! them in the input/chat/terminal. workers can't do it automatically it needs
//! to be explicitly done via a human via the input").
//!
//! When the OWNER sends worker A a message that @-mentions worker B, A may
//! message B for a while even if they share no group. Nothing else creates a
//! grant:
//!
//! - The send must be owner input (no worker origin) AND carry the dashboard's
//!   UI token (`X-Amux-UI-Token`), the same human proof session delete uses. A
//!   bare API call that leaves the worker header off is not enough.
//! - A worker's own sends never create one, whatever they mention.
//! - A grant is one direction (A to B), names one receiver, and expires.
//! - It only lifts the group boundary. A receiver's own CC_RECEIVE_DENY,
//!   isolation and lifecycle refusals still apply.
//!
//! Grants live in `~/.amux/mention-grants.json` so a deploy restart keeps them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) const GRANT_TTL_SECS: i64 = 2 * 3600;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Grant {
    pub from: String,
    pub to: String,
    pub created: i64,
    pub expires: i64,
}

fn path(home: &Path) -> PathBuf {
    home.join("mention-grants.json")
}

fn lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

fn load(home: &Path) -> Vec<Grant> {
    std::fs::read(path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(home: &Path, grants: &[Grant]) -> std::io::Result<()> {
    let p = path(home);
    let tmp = p.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(grants).unwrap_or_default())?;
    std::fs::rename(&tmp, &p)
}

/// Worker names @-mentioned in `text`, in order, without duplicates. `exists`
/// says whether a name is a registered worker; `exclude` is the receiver.
pub(crate) fn mentioned_workers(text: &str, exclude: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let re = regex::Regex::new(r"(^|[^\w@/.])@([A-Za-z0-9][\w.-]*)").expect("mention regex");
    let mut out: Vec<String> = Vec::new();
    for c in re.captures_iter(text) {
        let name = c[2].trim_end_matches(['.', '-']).to_string();
        if name.is_empty() || name == exclude || name.contains('@') || out.contains(&name) {
            continue;
        }
        if exists(&name) {
            out.push(name);
        }
    }
    out
}

/// Record `from` -> each of `to`, replacing any older grant for the same pair.
pub(crate) fn record(home: &Path, from: &str, to: &[String], now: i64) -> Vec<Grant> {
    if from.is_empty() || to.is_empty() {
        return vec![];
    }
    let _g = lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut all: Vec<Grant> = load(home).into_iter().filter(|g| g.expires > now).collect();
    let mut made = Vec::new();
    for t in to {
        all.retain(|g| !(g.from == from && &g.to == t));
        let g = Grant { from: from.to_string(), to: t.clone(), created: now, expires: now + GRANT_TTL_SECS };
        all.push(g.clone());
        made.push(g);
    }
    if let Err(e) = save(home, &all) {
        tracing::warn!(error = %e, verdict = "owner_mention_grant_unsaved", measured = true, n_considered = made.len(),
            "owner @-mention grants could not be saved; they will not be honoured");
        return vec![];
    }
    for g in &made {
        tracing::info!(from = %g.from, to = %g.to, expires = g.expires, verdict = "owner_mention_grant_created",
            measured = true, n_considered = 1, "the owner @-mentioned a worker; the receiver may message it until expiry");
    }
    made
}

/// The live grant letting `from` message `to`, if any.
pub(crate) fn active(home: &Path, from: &str, to: &str, now: i64) -> Option<Grant> {
    if from.is_empty() || to.is_empty() {
        return None;
    }
    load(home).into_iter().find(|g| g.from == from && g.to == to && g.expires > now)
}

/// The note appended to the message the receiver gets, so it knows the grant
/// exists and when it ends.
pub(crate) fn delivery_note(made: &[Grant]) -> String {
    if made.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = made.iter().map(|g| g.to.as_str()).collect();
    format!(
        "\n\n[amux: the owner @-mentioned {}. You may message {} for the next 2 hours even outside your groups \
         (owner grant, one way; it does not let them message you).]",
        names.join(", "),
        if names.len() == 1 { "them" } else { "each of them" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(n: &str) -> bool {
        matches!(n, "gtm-engine" | "mxp-gs12" | "amux")
    }

    #[test]
    fn mentions_pick_real_workers_once_and_skip_the_receiver_and_addresses() {
        let t = "@gtm-engine please, and `@mxp-gs12` too; mail me at x@gtm-engine.com, @gtm-engine again, @nobody, @amux";
        assert_eq!(mentioned_workers(t, "amux", known), vec!["gtm-engine".to_string(), "mxp-gs12".to_string()]);
        assert!(mentioned_workers("path /tmp/@gtm-engine/x", "amux", known).is_empty());
        assert_eq!(mentioned_workers("ask @gtm-engine.", "amux", known), vec!["gtm-engine".to_string()]);
    }

    #[test]
    fn a_grant_is_one_way_one_receiver_and_expires() {
        let home = tempfile::tempdir().unwrap();
        let made = record(home.path(), "amux", &["gtm-engine".to_string()], 1000);
        assert_eq!(made.len(), 1);
        assert!(active(home.path(), "amux", "gtm-engine", 1001).is_some());
        assert!(active(home.path(), "gtm-engine", "amux", 1001).is_none(), "one way");
        assert!(active(home.path(), "amux", "mxp-gs12", 1001).is_none(), "only the mentioned worker");
        assert!(active(home.path(), "amux", "gtm-engine", 1000 + GRANT_TTL_SECS).is_none(), "expires");
        assert!(record(home.path(), "", &["gtm-engine".to_string()], 1000).is_empty());
        assert!(delivery_note(&made).contains("gtm-engine"));
    }
}
