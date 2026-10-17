---
description: Send a message to a Codex chat by its name, or start a new named one, e.g. /codex #mxp onboarding flow <message> or /codex new thread <name>. Use when asked to send, relay or hand context to a Codex chat or thread, or to create one. Scripts: ~/.amux/scripts/codex-send.py <chat name> - and ~/.amux/scripts/codex-new.py <name> - (message on stdin).
argument-hint: <chat name> [message]
allowed-tools: Bash
---

# Send to a Codex chat

The request: **$ARGUMENTS**

The sender is `~/.amux/scripts/codex-send.py`. It finds the Codex thread by name
and runs `codex queue`. Do not look up thread ids by hand. Source of both scripts:
`skills/codex.md` and `skills/codex/scripts/` in the amux repo; install with
`install -m 755 skills/codex/scripts/*.py ~/.amux/scripts/`.

## 0. A NEW thread: always named, never `codex exec`

When the request asks for a new chat or thread, create it with
`~/.amux/scripts/codex-new.py`, never with `codex exec`. An exec run is headless:
it gets a session id and no name, the Codex app does not list it, and
`/codex <name>` cannot reach it (2026-10-17: a tester started that way had to be
forked and restarted).

```bash
~/.amux/scripts/codex-new.py '<name>' - [--cwd DIR] [--full-access] [--fork <thread>] <<'MSG'
<the first message: a self-contained brief, same rules as section 2>
MSG
```

- Pick a short name the owner will recognise, `#<area> <topic>` (`#amux cloud e2e`).
  Exit 2 means the name is taken: send to that thread instead.
- `--full-access` gives the thread danger-full-access with no approvals, for
  unattended testers and builders that need the network or a browser. Without
  it the thread uses the owner's `~/.codex/config.toml`.
- `--fork <thread>` copies an existing thread's whole history into the new named
  one (use it to rescue a headless `codex exec` run: stop the run, then fork it).
- It prints `Started thread <id> named "<name>"`. Report the name and id. Later
  messages go through codex-send.py by that name.
- Want replies back in this worker? Tell the thread how: a JSON POST of
  `{"text": ...}` to `$(amux url)/api/sessions/$AMUX_SESSION/send` arrives as
  owner input, even for an isolated worker.

## 1. Find the chat name

The chat name is usually the first words of the request, often starting with `#`
(`#mxp onboarding flow`). Everything after it is the message. If you are unsure
where the name ends, list the names and pick the one that fits:

```bash
~/.amux/scripts/codex-send.py --list | grep -i '<a word from the name>'
```

The name match ignores case, a leading `#` and punctuation. A fragment works if it
fits exactly one chat.

## 2. Write the message

- If the request gives the message text, send that text as written.
- If the request says to send "this", "all this context" or "what we found", write
  it yourself from the conversation. The Codex chat cannot see this session, so the
  message must stand alone: the goal, what was done, the findings with file paths,
  commit shas or links, open questions, and what you want the chat to do. Lead with
  the ask.
- Never include secrets, tokens, passwords or card numbers.
- Run the anti-slop pass before sending: no em-dashes, no "not X, it's Y".

## 3. Send it

Always pass the message on stdin with a quoted heredoc, so backticks and `$(...)`
stay literal:

```bash
~/.amux/scripts/codex-send.py '<chat name>' - <<'MSG'
<message>
MSG
```

The script adds `From amux worker $AMUX_SESSION:` at the top. Add `--no-origin` to
leave it out, or `--dry-run` to resolve the name without sending.

## 4. Report

- Exit 0 prints `Queued message <id> for thread <id>`. Tell the user which chat it
  went to (name and thread id) and the queued message id.
- Exit 2 means the name matched no chat or several. The script lists the
  candidates and sent nothing. If one candidate is clearly the one meant, retry
  with its full name. Otherwise show the candidates and ask.
- Any other failure: show the output. Do not report it as sent.

Every send is logged to `~/.amux/logs/codex-send.log`.
