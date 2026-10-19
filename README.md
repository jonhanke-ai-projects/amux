<img src="site/github-header.svg" alt="amux: the agent control plane" width="1280"/>

<p align="center">
  <a href="https://github.com/mixpeek/amux/stargazers"><img src="https://img.shields.io/github/stars/mixpeek/amux?style=flat-square&color=f5c518" alt="GitHub stars"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT%20%2B%20Commons%20Clause-blue?style=flat-square" alt="License"/></a>
  <a href="https://amux.io"><img src="https://img.shields.io/badge/site-amux.io-orange?style=flat-square" alt="Website"/></a>
</p>

**Run an AI engineering team.** amux runs Claude Code, Codex and Gemini CLI agents in parallel from one dashboard or your phone. They share a task board, message each other, recover from crashes, and keep shipping while you sleep.

## Install

```bash
curl -fsSL https://amux.io/install.sh | bash
```

Then open **https://localhost:8824**. It clones amux to `~/Dev/amux`, asks before installing anything (Rust, tmux), and keeps your data in `~/.amux`.

Already running amux? Move it to [amux Cloud](https://amux.io/start/): `amux export`, then `amux import <file> --to https://<you>.amux.io`.

You need tmux 3.2+ and at least one agent CLI (Claude Code, Codex or Gemini CLI). Runs on macOS and Linux. Prefer to read first? `git clone https://github.com/mixpeek/amux && cd amux && ./install.sh` does the same thing.

## Apps

Every app connects to your amux server (or your amux Cloud address).

| Device | Get it |
|---|---|
| **Mac** (macOS 12+, Apple silicon and Intel) | [Download Amux-macOS.zip](https://github.com/mixpeek/amux/releases/download/desktop-latest/Amux-macOS.zip), or install with the one-liner below |
| **iPhone and iPad** | [App Store](https://apps.apple.com/us/app/amux-agent-multiplexer/id6760410435) (one app for both) |
| **Android and any browser** | Open your dashboard URL and choose **Install app** or **Add to Home Screen** |

Mac, one command:

```bash
curl -fsSL https://github.com/mixpeek/amux/releases/download/desktop-latest/Amux-macOS.zip -o /tmp/Amux-macOS.zip && ditto -x -k /tmp/Amux-macOS.zip /Applications && open /Applications/Amux.app
```

The Mac app is a native window on the system WebKit (about 1.5 MB). It opens several windows and tabs, keeps pinned notes floating above other apps, shows a menu bar readout and a Dock badge when agents need you, and toggles with Control-Option-Space. It is not notarized yet: if you download the zip in a browser, macOS blocks the first open, so go to System Settings, Privacy & Security, and click **Open Anyway**. To build it yourself: `make desktop`.

<p align="center"><a href="https://amux.io"><img src="site/amux.gif" alt="amux dashboard running parallel agents" width="920"/></a></p>

<p align="center"><a href="https://amux.io/walkthrough/"><img src="site/walkthrough/amux-walkthrough-desktop-short.jpg" alt="Watch the amux walkthrough" width="920"/></a><br/><a href="https://amux.io/walkthrough/"><b>▶ Watch the walkthrough</b></a> (desktop and phone, with transcript)</p>

## What your agents get

- **A shared task board.** Claiming a card is atomic, so two agents never grab the same work.
- **Messages to each other.** Agents ask, hand off and unblock peers without you relaying.
- **Self-healing.** Crashed or stuck sessions restart and pick up where they left off.
- **Schedules and loops.** `daily at 9am`, `every 15m`, or overnight runs that pace themselves.
- **Your phone, tablet and Mac.** Watch, steer and approve from the [iPhone and iPad app, the Mac app](#apps) or the browser.
- **Any model.** Claude Code, Codex, Gemini CLI, OpenCode or Ollama, switchable per worker.

Local-first: one Rust binary and SQLite on your own machine.

## Don't want to run a server?

[amux Cloud](https://amux.io/start/) gives you your own always-on amux at `yourname.amux.io` from $39/mo, so agents keep working when your laptop sleeps. Same product, we run the box. [Pricing](https://amux.io/pricing/).

## Learn more

- [Getting started](https://amux.io/guides/getting-started/) · [Docs](https://amux.io/docs/) · [How amux compares](https://amux.io/compare/)
- [Self-hosting reference](docs/self-hosting.md): commands, Linux services, architecture, CLI, config, macOS permissions, security
- [Contributing](CONTRIBUTING.md) · [Roadmap (#46)](https://github.com/mixpeek/amux/issues/46) · [Security policy](SECURITY.md)

If amux saves you time, a ⭐ helps others find it.
