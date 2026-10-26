# Pull request handling for mixpeek/amux

Ethan asked on 2026-10-26 for a scheduler that looks at the repo's open pull
requests and handles them, and is allowed to merge.

## How it runs

- **Watcher** (`shell` schedule, every 15 minutes): `scripts/pr-watch.sh`.
  It reads the open PRs and any fork CI runs waiting for approval. When that
  set changes, it runs the handler. A quiet tick costs no model turn. Each run
  prints a `verdict=` line (`woke`, `unchanged`, `no_prs`, `unmeasured`,
  `wake_refused`), which shows in the schedule's run history.
- **Handler** (`tmux` schedule on amux-frustrations, run only by the
  watcher): the prompt below. It reviews each PR, then merges it, closes it
  or leaves it open with a reason.
- **Log**: the handler appends one block per run to
  `docs/pr-handling/<YYYY-MM>.md` and commits it.

Test: `scripts/test-pr-watch.sh` (in `checks`).

## Handler prompt

Handle the open pull requests on mixpeek/amux. The watcher woke you because
the set changed. Owner authority (Ethan, 2026-10-26): you may merge.

1. List them: `gh pr list -R mixpeek/amux --state open`. Read with the App
   token (`eval "$(~/.amux/github-app/get-token.sh)"`, then confirm a limit
   above 5000 with `gh api rate_limit`). Merge and approve as the admin
   identity: `env -u GH_TOKEN -u GITHUB_TOKEN gh ...`. The App token cannot.
   Recipe and pitfalls: memory `upstream-pr-landing-recipe`.
2. For each PR, read the diff (`gh pr diff N`) before deciding anything.
   - A fork's CI waits in `action_required`. Approve the run
     (`gh api -X POST repos/mixpeek/amux/actions/runs/<id>/approve`) only
     after reading the diff, including `package.json`, workflows and scripts,
     because `checks` runs the fork's tree.
   - Verify it: fetch `pull/N/head`, cherry-pick it onto `origin/main` in a
     detached worktree, and run the tests it touches. When it adds a guard,
     mutate the guard out with `scripts/mutate.sh run` and require a test to
     fail. If main's `rust` is red, compare with main before blaming the PR.
3. Decide, and record what you read:
   - **Merge** when `checks` is green on the PR, your own run passes and you
     can say what its tests would catch. Use `--squash` when the commits are
     authored by a model, otherwise `--rebase`. Never add a Claude
     co-author. Then confirm the commit is on `origin/main` and check its
     author line.
   - **Close** only as a duplicate, superseded or obsolete, and name the
     commit or PR that replaces it.
   - **Leave open** with one line saying what it waits on and who owns that.
     A product decision, a paid feature or removal of a user-facing feature
     is Ethan's: list the options and leave it.
   - A PR touching a fleet-wide path (the `amux` CLI, `runtime_jobs`, the
     launch path, `spawn_periodic`) needs your full suite run before merge.
4. Anything the contributor reads (a review, comment or request for changes)
   is a draft for Ethan. Put it in the log and in your final message. Do not
   post it.
5. Append a block to `docs/pr-handling/<YYYY-MM>.md` with one line per PR:
   number, verdict, the evidence (commands and results), and any draft.
   Commit it with `Amux-Request: MSG-97357`, then land it on
   main with fetch, rebase and push.
