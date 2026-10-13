//! Background work and recently touched cards for the chat companion's
//! `[context]` block.
//!
//! The 2026-10-12 report (mixpeek-frustrations' Chat tab): the owner asked
//! "what is this worker working on?", the block showed 0 doing cards and an
//! idle prompt, and the companion answered that it had no active task. The
//! worker had a background shell watching a CI run for MF-4026, its status
//! bar read `1 shell · ← 5 agents`, and its last reply said it was waiting
//! for the run. An idle prompt describes the MAIN turn, not the worker.
//!
//! Every fact here says where it came from, and a count the probe could not
//! take reads "not measured" rather than 0 (ethos rule 4).
//!
//! The status bar's `← N agents` is the agent SWITCHER: finished agents stay
//! listed there (measured 2026-10-12: 14 fleet panes showed `← 5 agents`
//! while every one of those lanes had a hook-reported live count of 0). So
//! it is reported as "listed", never as live work. Live agents come from the
//! SubagentStart/SubagentStop hook count, or from Claude Code's own
//! "Waiting for N background agents to finish" row.

use std::sync::LazyLock;

use regex::Regex;

/// Counts read from Claude Code's footer status bar. `None` from the parser
/// means no footer was found, so nothing was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FooterCounts {
    /// `N shell(s)`: background shells still running.
    pub shells: u32,
    /// `← N agents`: agents listed in the switcher, finished ones included.
    pub listed_agents: u32,
}

/// Read `N shell(s)` and `← N agent(s)` from the footer under the newest
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
        if !is_bar {
            continue;
        }
        for part in line.split('·') {
            // Claude right-aligns hints ("new task? /clear to save ...") on
            // the same row after a run of spaces; they are not part of the item.
            let part = part.trim().split("  ").next().unwrap_or("");
            let part = part.trim_start_matches(['←', '↓', '→', '↑']).trim();
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
                    out.listed_agents = n;
                    counted = true;
                }
                _ => {}
            }
        }
    }
    (bar_seen || counted).then_some(out)
}

static RE_WAITING_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)waiting for ([1-9]\d*) background agents? to finish").expect("waiting regex"));

/// N from Claude Code's own "Waiting for N background agents to finish" row,
/// when that row is the newest turn marker (a later completed turn supersedes
/// it). `None` when there is no such live row.
pub(crate) fn claude_waiting_agents(raw: &str) -> Option<u32> {
    let clean = crate::backend::adapter::strip_ansi(raw);
    if !crate::backend::adapter::claude_background_wait_verdict(&clean).0 {
        return None;
    }
    clean
        .lines()
        .rev()
        .find(|l| crate::backend::adapter::claude_background_agents_waiting(l))
        .and_then(|l| RE_WAITING_N.captures(l))
        .and_then(|c| c[1].parse().ok())
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
    /// N from a live "Waiting for N background agents to finish" row.
    pub waiting_agents: Option<u32>,
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
    /// Live background agents; `None` when no probe could count them.
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
    let listed = i.footer.map(|f| f.listed_agents).unwrap_or(0);
    let waiting = i.waiting_agents.map(i64::from);
    // The hook count is the reported state (AMUX-4024); Claude Code's own
    // waiting row is provider chrome that only shows while agents run (D1
    // fallback). Both are live evidence, so the larger one wins and a
    // disagreement is named.
    let agents = match (i.reported_agents, waiting) {
        (Some(r), Some(w)) => Some(r.max(w)),
        (r, w) => r.or(w),
    };
    let measured = shells.is_some() || agents.is_some();
    let wait = announced_wait(&i.last_reply);
    let mut lines = String::new();
    let listed_note = if listed > 0 {
        format!("; the status bar lists {} (finished agents stay listed there)", plural(listed as i64, "agent", "agents"))
    } else {
        String::new()
    };
    if !measured {
        lines.push_str("- background: not measured (no hook-reported agent count and no provider status bar in the pane)\n");
    } else {
        let mut parts = Vec::new();
        match shells {
            Some(n) => parts.push(plural(n as i64, "shell running", "shells running")),
            None => parts.push("shells not measured".to_string()),
        }
        match agents {
            Some(n) => parts.push(plural(n, "live agent", "live agents")),
            None => parts.push("live agents not measured".to_string()),
        }
        let mut src = Vec::new();
        if shells.is_some() {
            src.push("shells from the pane status bar".to_string());
        }
        match (i.reported_agents, waiting) {
            (Some(r), Some(w)) if r != w => {
                src.push(format!("agents: hook report {r}, pane waiting row {w}"))
            }
            (Some(_), _) => src.push("agents from the hook report".to_string()),
            (None, Some(_)) => src.push("agents from the pane's waiting row".to_string()),
            (None, None) => {}
        }
        let head = if shells.unwrap_or(0) == 0 && agents.unwrap_or(0) == 0 {
            "none".to_string()
        } else {
            parts.join(", ")
        };
        lines.push_str(&format!("- background: {head} ({}{listed_note})\n", src.join("; ")));
        if !i.agent_descriptions.is_empty() {
            let d: Vec<String> =
                i.agent_descriptions.iter().take(6).map(|d| super::chat_worker::clip(d, 80)).collect();
            lines.push_str(&format!("- subagents with transcript writes in the last 15m: {}\n", d.join("; ")));
        }
    }
    if !wait.is_empty() {
        let note = if measured && shells.unwrap_or(0) == 0 && agents.unwrap_or(0) == 0 {
            " (no background shell or live agent now, so it may have finished)"
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
            "- recently touched (not in doing): none ({n_considered} card(s) considered: ids named in its last reply and recent log, plus its own cards updated in the last 6h)\n"
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

    /// The reported pane, as the fleet drew it on 2026-10-12, including the
    /// right-aligned hint Claude puts on the status row.
    const REPORTED_PANE: &str = "\u{25cf} Watch armed on the manually-dispatched, isolated CI run.\n\n\
        \u{276f} \n\
        \u{2500}\u{2500}\u{2500}\u{2500}\n  \
        \u{23f5}\u{23f5} bypass permissions on \u{b7} 1 shell \u{b7} \u{2190} 5 agents \u{b7} \u{2193} to manage\n                                        new task? /clear to save 325.7k tokens";

    #[test]
    fn the_reported_status_bar_reads_one_shell_and_five_listed_agents() {
        assert_eq!(claude_footer_counts(REPORTED_PANE), Some(FooterCounts { shells: 1, listed_agents: 5 }));
        // The hint can share the row with the last item.
        let hinted = "\u{276f} \n  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle) \u{b7} \u{2190} 5 agents                    new task? /clear to save 164.7k tokens";
        assert_eq!(claude_footer_counts(hinted), Some(FooterCounts { shells: 0, listed_agents: 5 }));
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
    fn claude_waiting_row_counts_live_agents_until_a_turn_completes() {
        let waiting = "\u{2733} Waiting for 3 background agents to finish\n\n\u{276f}\n\n\
                       \u{23f5}\u{23f5} bypass permissions on \u{b7} \u{2190} 5 agents";
        assert_eq!(claude_waiting_agents(waiting), Some(3));
        assert_eq!(claude_waiting_agents("\u{276f}\n\u{23f5}\u{23f5} bypass permissions on \u{b7} \u{2190} 5 agents"), None);
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

    /// The reported state as measured: the hook said 0 live agents while the
    /// bar listed 5. The shell is the live work; the 5 are named as listed.
    #[test]
    fn the_reported_state_names_the_shell_the_listed_agents_and_the_wait() {
        let s = background_section(&BackgroundInputs {
            footer: claude_footer_counts(REPORTED_PANE),
            reported_agents: Some(0),
            last_reply: "Watch armed on CI run 38306792417. Waiting for it to complete.".into(),
            ..Default::default()
        });
        assert!(s.present());
        assert!(s.lines.contains("- background: 1 shell running, 0 live agents (shells from the pane status bar; agents from the hook report; the status bar lists 5 agents (finished agents stay listed there))"), "{}", s.lines);
        assert!(s.lines.contains("- pending wait announced in its last reply: Watch armed on CI run 38306792417. Waiting for it to complete."), "{}", s.lines);
    }

    #[test]
    fn live_agents_from_the_hook_are_counted_with_their_descriptions() {
        let s = background_section(&BackgroundInputs {
            footer: claude_footer_counts(REPORTED_PANE),
            reported_agents: Some(5),
            agent_descriptions: vec!["Audit retry path".into()],
            ..Default::default()
        });
        assert!(s.lines.contains("- background: 1 shell running, 5 live agents (shells from the pane status bar; agents from the hook report;"), "{}", s.lines);
        assert!(s.lines.contains("- subagents with transcript writes in the last 15m: Audit retry path"), "{}", s.lines);
    }

    #[test]
    fn no_background_is_a_measured_none_and_no_probe_is_not_measured() {
        let none = background_section(&BackgroundInputs {
            footer: Some(FooterCounts::default()),
            reported_agents: Some(0),
            ..Default::default()
        });
        assert!(!none.present());
        assert!(none.lines.contains("- background: none (shells from the pane status bar; agents from the hook report)"), "{}", none.lines);
        let unmeasured = background_section(&BackgroundInputs::default());
        assert!(!unmeasured.measured);
        assert!(unmeasured.lines.contains("- background: not measured"), "{}", unmeasured.lines);
    }

    #[test]
    fn a_hook_count_and_a_waiting_row_that_disagree_are_both_named() {
        let s = background_section(&BackgroundInputs {
            footer: Some(FooterCounts { shells: 0, listed_agents: 0 }),
            waiting_agents: Some(3),
            reported_agents: Some(0),
            ..Default::default()
        });
        assert!(s.present());
        assert!(s.lines.contains("- background: 0 shells running, 3 live agents (shells from the pane status bar; agents: hook report 0, pane waiting row 3)"), "{}", s.lines);
    }

    /// A hookless lane (no reported count) still gets live agents from
    /// Claude Code's waiting row rather than "not measured".
    #[test]
    fn without_a_hook_count_the_waiting_row_counts_live_agents() {
        let s = background_section(&BackgroundInputs {
            footer: Some(FooterCounts { shells: 0, listed_agents: 5 }),
            waiting_agents: Some(2),
            reported_agents: None,
            ..Default::default()
        });
        assert!(s.present());
        assert!(s.lines.contains("- background: 0 shells running, 2 live agents (shells from the pane status bar; agents from the pane's waiting row;"), "{}", s.lines);
    }

    #[test]
    fn card_ids_are_collected_once_in_order() {
        let ids = referenced_card_ids(["Waiting on MF-4026 and MF-4026.", "see AMUX-12 and UTF-8"]);
        assert_eq!(ids, vec!["MF-4026", "AMUX-12", "UTF-8"]);
    }
}
