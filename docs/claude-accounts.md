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

## Per-account usage

```bash
amux claude-accounts track on     # once
amux claude-accounts list         # 5h / weekly %, reset times, pace, and where to spend surplus
```

Claude Code gives its status-line command each session's `rate_limits`: the 5-hour and
weekly used percentage and reset time for the account that session is signed in to.
`track on` writes `scripts/claude-account-usage-tap.py` (compiled into the server) to
`~/.amux/bin/` and puts it in front of your existing status line:

```text
statusLine.command = '~/.amux/bin/claude-account-usage-tap.py' -- <your previous command>
```

Every account shares `settings.json`, so the tap runs for every account's sessions.
`CLAUDE_CONFIG_DIR` tells it which account a session belongs to. It posts at most one
reading per account every 30 s to `POST /api/usage/accounts/readings` (the
`claude_account_usage` history the upstream rotator also uses), then runs your own status
line with the same input. `track on` changes only `statusLine`, after a timestamped backup
(`settings.json.bak-amux-tap-*`). `track off` puts your command back.

The tap reads no credential and calls no Anthropic endpoint. The numbers are the ones
Claude Code already shows. An account only gets readings while one of its sessions is
running, and `list` shows how old each reading is.

`GET /api/claude-accounts` adds `usage` per account (latest reading, burn rate, hours
to full, hours to the weekly reset) and `spend_first`. That is the account with weekly
and 5-hour headroom whose weekly window resets soonest. Its unused share is lost at that
reset, so it is where surplus work should go:

```bash
amux claude-accounts assign --group surplus <that account>
```

## Not yet

- Moving the surplus group to `spend_first` is still a manual `assign`. It could run on a
  schedule, but that is a policy decision for the owner.
- No dashboard panel. The CLI, the API and the Scope env editor cover it for now.
