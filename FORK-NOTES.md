# Fork notes (jonhanke-ai-projects/amux)

Notes specific to this fork. Upstream is [mixpeek/amux](https://github.com/mixpeek/amux).

## Where it lives locally

This machine keeps most repos under `~/Documents/Code_Repositories/<name>`. This one lives under `~/Dev` instead:

| What | Path |
|---|---|
| Project folder | `~/Dev/amux-for-multiple-accounts/` |
| Main checkout (`main`) | `~/Dev/amux-for-multiple-accounts/repo/` |
| Worktree for multi-account work (`feat/multi-claude-accounts`) | `~/Dev/amux-for-multiple-accounts/wt-multi-accounts/` |
| Build cache shared by both | `~/Dev/amux-for-multiple-accounts/repo/target/` (`CARGO_TARGET_DIR`) |
| Archived worktrees, once merged | `~/Documents/Code_Repositories/_archive/amux-worktrees/` |

Remotes in the checkout: `origin` is this fork, `upstream` is mixpeek/amux.

This is a source checkout only. The amux you run day to day (`~/.amux`, `~/.local/bin/amux-server-rs`) is installed separately from upstream and does **not** run this fork's code.

## What the fork is for

Running Claude Code workers on several Claude accounts at once, and moving workers or whole projects between accounts. Design and usage: `docs/claude-accounts.md` ([on the PR branch](https://github.com/jonhanke-ai-projects/amux/blob/feat/multi-claude-accounts/docs/claude-accounts.md) until PR #2 merges).

- Issue: [#1](https://github.com/jonhanke-ai-projects/amux/issues/1)
- PR: [#2](https://github.com/jonhanke-ai-projects/amux/pull/2)
- Terms-of-service analysis (two accounts, one person): [docs/fork/claude-accounts-terms.md](docs/fork/claude-accounts-terms.md)
- Status 2026-10-27: PR #2 is a draft, not yet compiled (the laptop was overloaded). Next step: `cargo test -p amux-server --lib claude_accounts`, then a live test with both accounts.

## Workflow differences from upstream

Upstream's `CLAUDE.md` says to commit straight to `main` with no branches or forks. That rule is Mixpeek's, for their own machine. In this fork, work goes issue → worktree branch → PR, as everywhere else.

GitHub Actions is **not** enabled on the fork. Upstream's workflows include scheduled cloud and deploy jobs, so enable Actions only on purpose.
