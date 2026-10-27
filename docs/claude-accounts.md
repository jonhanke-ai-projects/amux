# Several Claude Code accounts at once

Fork feature ([jonhanke-ai-projects/amux#1](https://github.com/jonhanke-ai-projects/amux/issues/1)).
Server: `crates/amux-server/src/api/claude_accounts.rs`. CLI: `amux claude-accounts`.

## What upstream amux does

Upstream runs every Claude worker on **one** machine-wide login (`~/.claude.json`
plus the default credential). `scripts/claude-rotate.py` can switch that one login
between accounts when usage runs low, but only one account is in use at any time.

The site guide `/guides/claude-code-account-rotation/` and the changelog also describe
`amux accounts add`, `AMUX_ACCOUNT_ROTATION`, `AMUX_ROTATION_*` settings, a credential
vault and per-session account assignment. None of those exist in the code as of
upstream `8ef8456` (2026-10-27).

## What this adds

Each named account is its own Claude Code config directory:

```text
~/.amux/claude-accounts/<name>/
    .claude.json        this account's login and preferences (private)
    credentials         macOS keychain item keyed by this directory, or .credentials.json (private)
    projects  -> ~/.claude/projects        shared transcripts
    settings.json -> ~/.claude/settings.json   shared (amux hooks live here)
    skills, agents, commands, plugins, hooks, CLAUDE.md, todos, plans, ...  shared
```

A worker bound to an account launches with `CLAUDE_CONFIG_DIR` set to that directory.
Everything else (hooks, skills, MCP servers, memory, transcripts) is the same as on the
default login. Only the login differs.

Because `projects/` is shared, moving a worker to another account is a relaunch that
resumes the same conversation. amux's own transcript readers keep working unchanged.

## Use

```bash
amux claude-accounts add work            # creates ~/.amux/claude-accounts/work
amux claude-accounts login work          # browser sign-in for that account
amux claude-accounts list                # accounts, logins, and what is bound to each

amux claude-accounts assign my-worker work           # one worker; relaunches at an idle boundary
amux claude-accounts assign --group client-x work    # a whole group/project
amux claude-accounts assign my-worker --default      # back to ~/.claude
```

Binding is the scope key `AMUX_CLAUDE_ACCOUNT=<name>`, so you can also set it in the
Scope UI or by hand in `~/.amux/sessions/<worker>.env` (worker) or
`~/.amux/env/<group>.env` (group). The worker setting wins over the group setting,
the same precedence as every other scoped key.

API:

| Route | Body | Does |
|---|---|---|
| `GET /api/claude-accounts` | | default login + every account with `signed_in`, `email`, `workers`, `groups` |
| `POST /api/claude-accounts` | `{"name"}` | create or repair an account directory |
| `POST /api/claude-accounts/assign` | `{"target","level":"worker"\|"group","account","relaunch"}` | bind or clear (empty `account`), then relaunch the affected running Claude workers |

## Details

- **Credentials.** A bound worker never receives the dashboard's saved
  `CLAUDE_CODE_OAUTH_TOKEN`, since an env token would override the account. If the
  account is signed in, `ANTHROPIC_API_KEY` is suppressed as for the default login.
- **First run.** A new account's `.claude.json` is copied from `~/.claude.json`
  without the login (`oauthAccount`, `userID`, `primaryApiKey`), so MCP servers,
  folder trust and onboarding carry over. Launch-time trust and onboarding seeding
  write to the bound account's file.
- **Restarts.** A shell that survives a restart drops an earlier amux-set
  `CLAUDE_CONFIG_DIR` (marked by `AMUX_CLAUDE_CONFIG_DERIVED=1`) before applying the
  current binding. A `CLAUDE_CONFIG_DIR` you export yourself is left alone.
- **Missing account.** A binding that names an account with no directory launches on
  the default login and logs `verdict=claude_account_missing`. A bound launch logs
  `verdict=claude_account_bound`.
- **Relaunch is bounded.** `assign` relaunches only at an idle boundary (the same
  `stop_verified_worker` gate the subscription relaunch job uses). A busy worker keeps
  running and picks up the new account at its next start.

## Not yet

- Usage readings (`/api/usage`, `claude-rotate.py`) still probe the default login only.
- No automatic per-worker rotation yet. The binding is the primitive it would drive:
  pick the account with headroom, then `assign`.
- No dashboard panel. The CLI, the API and the Scope env editor cover it for now.
