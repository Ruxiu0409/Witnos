<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="136" />
</p>

<h1 align="center">Witnos</h1>

<p align="center">
  <strong>Stop letting your coding agent grade its own homework.</strong>
</p>

<p align="center">
  Witnos gives Claude Code a living definition of done —<br />
  visible, evidence-backed, and editable while it works.
</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center">
  <strong>English</strong> ·
  <a href="README.zh-Hant.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

---

Agents rarely fail loudly. They choose a plausible standard, verify against it, and confidently stop. On a long run, a wrong assumption in step one becomes architecture by step ten.

**Witnos makes the definition of done visible while correction is still cheap.**

## How it works

1. **Make the completion criteria explicit.** Claude lays out each criterion as a claim, a check, and the evidence behind it.
2. **Change it live.** Edit a criterion while Claude is working. The delta reaches it after its next tool call.
3. **Gate “done.”** When Claude tries to stop, Witnos checks the latest contract and sends it back if the evidence is stale or incomplete.

Witnos is not another agent. You keep Claude Code, your credentials, and your workflow. Witnos only makes verification visible and steerable.

## Install

> **Developer Preview** — macOS + Claude Code only. Witnos is currently built from source and is unsigned / unnotarized.

Requirements: [Rust via rustup](https://rustup.rs), Node.js <code>^20.19.0</code> or <code>>=22.12.0</code>, and Claude Code.

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

The installer builds everything, installs <code>/Applications/Witnos.app</code>, verifies the bundled CLI, and opens the app. Pass <code>--no-open</code> to keep it closed.

## Start in 30 seconds

1. Open Witnos and click **watch a project (auto)**.
2. Pick a project folder; trust it in Claude Code and approve the hooks with <code>/hooks</code> if prompted.
3. Start <code>claude</code> inside Witnos's terminal.
4. Send your first prompt. It becomes that session's goal and its verification contract appears automatically.

Now edit the contract whenever the evidence reveals something you forgot to say.

### A concrete example

You ask: **“Refactor auth without changing behavior.”** Claude runs the tests and prepares to stop. Then you notice keyboard navigation was never part of its standard. Add it to the contract. Witnos delivers the correction, makes the old evidence stale, and Claude cannot reuse the old “done.”

That is the whole point: **catch the wrong assumption before it compounds.**

## What you get

- **Evidence with provenance** — files, URLs, and recorded commands stay attached to the claim they support.
- **One contract per session** — every watched Claude session gets its own goal from its first prompt.
- **Corrections without interruption** — only the changed criteria are delivered, not the entire contract again.
- **Terminals that survive the app** — quit and reopen Witnos without losing the shell or its conversation.
- **Local by default** — local JSON, authenticated loopback traffic, no cloud, no telemetry, no credential proxy.

## Safe by scope

Only projects you explicitly watch enable the Stop gate. Auto-created goals only apply to Claude sessions started inside Witnos's terminal; your other terminals and projects are untouched.

Use **stop watching** to opt out. If a crash leaves a watched agent at the Stop gate, reopen Witnos or run <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code> from the project root. Installed hooks remain inert when watching is off.

## Current limits

Today Witnos is macOS-only, Claude Code-only, source-built, and unsigned. Linux is untested, Windows does not yet have the persistent terminal daemon, and the subjective-judgement prompt hook is still pending.

For the reasoning behind those choices, read the [design notes](docs/README.md).

## Development

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

The workspace is split into a framework-free domain core, an Axum server, a headless hook CLI, and a Tauri + React desktop app. See [CLAUDE.md](CLAUDE.md) for the full development loops.

## Contributing

Issues and PRs are welcome. For a large change, open an issue first and read the [six design constraints](docs/design.md#design-principles-this-is-the-project-not-the-ui) — especially the rule that an agent may never pass its own subjective work.

## License

Copyright © 2026 CHENG YEH TSAI

Dual licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE). Use whichever you prefer. Unless stated otherwise, contributions are licensed the same way.
