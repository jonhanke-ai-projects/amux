//! Background work and recently touched cards for the chat companion's
//! `[context]` block.
//!
//! The 2026-10-12 report (mixpeek-frustrations' Chat tab): the owner asked
//! "what is this worker working on?", the block showed 0 doing cards and an
//! idle prompt, and the companion answered that it had no active task. The
//! worker had a background shell watching a CI run for MF-4026 plus 5
//! background agents, all visible in Claude Code's status bar
//! (`1 shell · ← 5 agents`), and its last reply said it was waiting for the
//! run. An idle prompt describes the MAIN turn, not the worker.
//!
//! Every fact here says where it came from, and a count the probe could not
//! take reads "not measured" rather than 0 (ethos rule 4).

use std::sync::LazyLock;

use regex::Regex;

/// Counts read from Claude Code's footer status bar. `None` from the parser
/// means no footer was found, so nothing was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FooterCounts {
    pub shells: u32,
    pub agents: u32,
}

/// Read `N shell(s)` and `N agent(s)` from the footer under the newest
/// composer prompt. Only lines below the last `❯` count, so a worker quoting
/// "1 shell" in its prose cannot match. Returns `None` when there is no
/// provider status bar (`⏵⏵` / `⏸` row) and no count, so the caller can say
/// the pane did not measure it.
pub(crate) fn claude_footer_counts(raw: &str) -> Option<FooterCounts> {
    let clean = crate::backend::adapter::strip_ansi(raw);
    let (_, footer) = clean.rsplit_once('❯')?;
    let mut bar_seen = false;
    let mut counted = false;
    let mut out = FooterCounts::default();
    for line in footer.lines() {
        let line = line.trim();
        let is_bar = line.starts_with('⏵') || line.starts_with('⏸');
        bar_seen |= is_bar;
        if !is_bar && !line.contains('·') {
            continue;
        }
        for part in line.split('·') {
            let part = part.trim().trim_start_matches(['←', '↓', '→', '↑']).trim();
            let mut words = part.split_whitespace();
            let (Some(n), Some(unit)) = (words.next(), words.next()) else { continue };
            if words.next().is_some() {
                continue;
            }
            let Ok(n) = n.parse::<u32>() else { continue };
            match unit {
                "shell" | "shells" => {
                    out.shells = n;
                    counted = true;
                }
                "agent" | "agents" => {
                    out.agents = n;
                    counted = true;
                }
                _ => {}
            }
        }
    }
    (bar_seen || counted).then_some(out)
}

static RE_WAIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(waiting (for|on|until)|wait(s|ing)? for|will wait|watch(ing)? armed|watch is armed|armed a (watch|wakeup|monitor)|polling|until (it|the run|ci|the job)|once (it|the run|ci|the job) (completes|finishes|is done|lands)|when (it|the run|ci|the job) (completes|finishes|lands)|in the background|background (shell|job|agent|task)s?|wake ?up)\b",
    )
    .expect("wait regex")
});

/// The wait the last reply announced: its last one or two sentences that say
/// it is waiting on something, in their original order. Empty when it
/// announced none.
pub(crate) fn announced_wait(last_reply: &str) -> String {
    let flat = last_reply.split_whitespace().collect::<Vec<_>>().join(" ");
    // Split on sentence ends that are followed by a space, so a run id, a
    // version number or a file name keeps its dots.
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0;
    let bytes = flat.as_bytes();
    for (i, ch) in flat.char_indices() {
        if matches!(ch, '.' | '!' | '?') && bytes.get(i + 1) == Some(&b' ') {
            sentences.push(flat[start..=i].trim());
            start = i + 1;
        }
    }
    if start < flat.len() {
        sentences.push(flat[start..].trim());
    }
    let hits: Vec<&str> = sentences.iter().copied().filter(|s| RE_WAIT.is_match(s)).collect();
    let tail = &hits[hits.len().saturating_sub(2)..];
    let out = tail.join(" ");
    if out.chars().count() > 360 {
        out.chars().take(360).collect::<String>() + "…"
    } else {
        out
    }
}

static RE_CARD_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]{1,11}-\d{1,6}\b").expect("card id regex"));

/// Card ids (`MF-4026`) named in the given texts, first mention first,
/// deduplicated. Only ids that exist on the board are shown, so a token that
/// merely looks like one (`UTF-8`) drops out at the lookup.
pub(crate) fn referenced_card_ids<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        for m in RE_CARD_ID.find_iter(t) {
            let id = m.as_str().to_string();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// Everything the background line is built from. Each field is `None` when
/// its probe did not run, which is different from a measured zero.
#[derive(Debug, Default, Clone)]
pub(crate) struct BackgroundInputs {
    /// Footer counts from the pane capture; `None` = no pane or no footer.
    pub footer: Option<FooterCounts>,
    /// Live subagent count from the SubagentStart/SubagentStop hook report.
    pub reported_agents: Option<i64>,
    /// Descriptions of subagents whose transcripts were written recently.
    pub agent_descriptions: Vec<String>,
    /// The worker's last reply (may be empty).
    pub last_reply: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BackgroundSummary {
    pub shells: Option<u32>,
    pub agents: Option<i64>,
    pub wait: String,
    pub measured: bool,
    pub lines: String,
}

impl BackgroundSummary {
    pub fn present(&self) -> bool {
        self.shells.unwrap_or(0) > 0 || self.agents.unwrap_or(0) > 0
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Compose the background lines of the context block.
pub(crate) fn background_section(i: &BackgroundInputs) -> BackgroundSummary {
    let shells = i.footer.map(|f| f.shells);
    let pane_agents = i.footer.map(|f| f.agents as i64);
    // The hook count is the reported state (AMUX-4024); the pane bar is the
    // fallback (D1). When both exist and disagree, the larger one is shown
    // and the other is named, so a lost SubagentStop or a stale bar is visible.
    let agents = match (i.reported_agents, pane_agents) {
        (Some(r), Some(p)) => Some(r.max(p)),
        (r, p) => r.or(p),
    };
    let measured = shells.is_some() || agents.is_some();
    let wait = announced_wait(&i.last_reply);
    let mut lines = String::new();
    if !measured {
        lines.push_str("- background: not measured (no hook-reported agent count and no provider status bar in the pane)\n");
    } else if shells.unwrap_or(0) == 0 && agents.unwrap_or(0) == 0 {
        let src = match (shells.is_some(), i.reported_agents.is_some()) {
            (true, true) => "pane status bar and hook-reported agent count",
            (true, false) => "pane status bar",
            _ => "hook-reported agent count; shells not measured",
        };
        lines.push_str(&format!("- background: none ({src})\n"));
    } else {
        let mut parts = Vec::new();
        match shells {
            Some(n) => parts.push(plural(n as i64, "shell", "shells")),
            None => parts.push("shells not measured".to_string()),
        }
        if let Some(n) = agents {
            parts.push(plural(n, "agent", "agents"));
        }
        let mut src = Vec::new();
        if shells.is_some() {
            src.push("shells from the pane status bar".to_string());
        }
        match (i.reported_agents, pane_agents) {
            (Some(r), Some(p)) if r != p => {
                src.push(format!("agents: hook report {r}, pane status bar {p}"))
            }
            (Some(_), _) => src.push("agents from the hook report".to_string()),
            (None, Some(_)) => src.push("agents from the pane status bar".to_string()),
            (None, None) => {}
        }
        lines.push_str(&format!("- background: {} ({})\n", parts.join(", "), src.join("; ")));
        if !i.agent_descriptions.is_empty() {
            let d: Vec<String> = i.agent_descriptions.iter().take(6).map(|d| super::chat_worker::clip(d, 80)).collect();
            lines.push_str(&format!("- background agents (recent subagent transcripts): {}\n", d.join("; ")));
        }
    }
    if !wait.is_empty() {
        let note = if measured && shells.unwrap_or(0) == 0 && agents.unwrap_or(0) == 0 {
            " (no background shell or agent is visible now, so it may have finished)"
        } else {
            ""
        };
        lines.push_str(&format!("- pending wait announced in its last reply{note}: {wait}\n"));
    }
    BackgroundSummary { shells, agents, wait, measured, lines }
}

/// A card the worker is touching that the doing/needsyou list does not show.
#[derive(Debug, Clone)]
pub(crate) struct TouchedCard {
    pub id: String,
    pub status: String,
    pub session: String,
    pub updated: f64,
    pub why: &'static str,
}

pub(crate) fn touched_line(worker: &str, cards: &[TouchedCard], now: f64, n_considered: usize) -> String {
    if cards.is_empty() {
        return format!(
            "- recently touched (not in doing): none ({n_considered} card id(s) named in its last reply and recent log, plus its own cards updated in the last 6h)\n"
        );
    }
    let items: Vec<String> = cards
        .iter()
        .map(|c| {
            let mins = ((now - c.updated).max(0.0) / 60.0).round() as i64;
            let ago = if mins < 90 { format!("{mins}m ago") } else { format!("{}h ago", mins / 60) };
            let owner = if c.session == worker || c.session.is_empty() {
                String::new()
            } else {
                format!(", owned by {}", c.session)
            };
            format!("{} ({}{owner}, updated {ago}; {})", c.id, c.status, c.why)
        })
        .collect();
    format!("- recently touched (not in doing): {}\n", items.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORTED_PANE: &str = "\u{25cf} Watch armed on the manually-dispatched, isolated CI run.\n\n\
        \u{276f} \n\
        \u{2500}\u{2500}\u{2500}\u{2500}\n  \
        \u{23f5}\u{23f5} bypass permissions on \u{b7} 1 shell \u{b7} \u{2190} 5 agents \u{b7} \u{2193} to manage";

    #[test]
    fn the_reported_status_bar_reads_one_shell_and_five_agents() {
        assert_eq!(claude_footer_counts(REPORTED_PANE), Some(FooterCounts { shells: 1, agents: 5 }));
    }

    #[test]
    fn a_bar_with_no_background_reads_a_measured_zero_and_no_bar_reads_unmeasured() {
        let idle = "\u{276f} \n  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle) \u{b7} \u{2190} for agents";
        assert_eq!(claude_footer_counts(idle), Some(FooterCounts::default()));
        assert_eq!(claude_footer_counts("$ ls\nfoo"), None);
        // Prose above the composer does not count.
        let quoted = "I see 3 shells \u{b7} 2 agents in my notes\n\u{276f} \n  \u{23f5}\u{23f5} bypass permissions on";
        assert_eq!(claude_footer_counts(quoted), Some(FooterCounts::default()));
    }

    #[test]
    fn the_wait_the_last_reply_announced_is_extracted() {
        let reply = "Pushed the fix as abc123. Watch armed on the manually-dispatched, isolated CI run 38306792417 for MF-4026. Waiting for it to complete.";
        let w = announced_wait(reply);
        assert!(w.starts_with("Watch armed on the manually-dispatched"), "{w}");
        assert!(w.contains("38306792417"), "{w}");
        assert!(w.ends_with("Waiting for it to complete."), "{w}");
        assert!(!w.contains("Pushed the fix"), "{w}");
        assert_eq!(announced_wait("Done. All tests pass."), "");
    }

    #[test]
    fn the_reported_state_produces_the_background_line_and_the_wait() {
        let s = background_section(&BackgroundInputs {
            footer: claude_footer_counts(REPORTED_PANE),
            reported_agents: Some(5),
            agent_descriptions: vec!["Audit retry path".into()],
            last_reply: "Watch armed on CI run 38306792417. Waiting for it to complete.".into(),
        });
        assert!(s.present());
        assert!(s.lines.contains("- background: 1 shell, 5 agents (shells from the pane status bar; agents from the hook report)"), "{}", s.lines);
        assert!(s.lines.contains("- background agents (recent subagent transcripts): Audit retry path"), "{}", s.lines);
        assert!(s.lines.contains("- pending wait announced in its last reply: Watch armed on CI run 38306792417. Waiting for it to complete."), "{}", s.lines);
    }

    #[test]
    fn no_background_is_a_measured_none_and_no_probe_is_not_measured() {
        let none = background_section(&BackgroundInputs {
            footer: Some(FooterCounts::default()),
            reported_agents: Some(0),
            ..Default::default()
        });
        assert!(!none.present());
        assert!(none.lines.contains("- background: none (pane status bar and hook-reported agent count)"), "{}", none.lines);
        let unmeasured = background_section(&BackgroundInputs::default());
        assert!(!unmeasured.measured);
        assert!(unmeasured.lines.contains("- background: not measured"), "{}", unmeasured.lines);
    }

    #[test]
    fn a_hook_count_and_a_pane_count_that_disagree_are_both_named() {
        let s = background_section(&BackgroundInputs {
            footer: Some(FooterCounts { shells: 0, agents: 5 }),
            reported_agents: Some(0),
            ..Default::default()
        });
        assert!(s.lines.contains("- background: 0 shells, 5 agents (shells from the pane status bar; agents: hook report 0, pane status bar 5)"), "{}", s.lines);
    }

    #[test]
    fn card_ids_are_collected_once_in_order() {
        let ids = referenced_card_ids(["Waiting on MF-4026 and MF-4026.", "see AMUX-12 and UTF-8"]);
        assert_eq!(ids, vec!["MF-4026", "AMUX-12", "UTF-8"]);
    }
}
