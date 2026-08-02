<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="126" />
</p>

<h1 align="center">Witnos</h1>

<p align="center"><strong>Stop letting your coding agent grade its own homework.</strong></p>
<p align="center">See what Claude plans to verify. Change it while it works. Block “done” when the evidence no longer matches.</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/status-developer%20preview-orange?style=flat-square" alt="Developer Preview" />
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center"><strong>English</strong> · <a href="README.zh-Hant.md">繁體中文</a> · <a href="README.ja.md">日本語</a></p>

<p align="center">
  <img src=".github/assets/workflow.svg" alt="Define evidence-backed completion criteria, edit them while Claude works, then gate done against the latest version." width="100%" />
  <sub>Claude Code stays the agent. Witnos only exposes and gates its verification.</sub>
</p>

## At a glance

| **Evidence attached**<br>Files, URLs, and commands | **Live edits**<br>Only changed criteria are sent | **Stop gate**<br>Checks the latest version |
|:---|:---|:---|
| **One goal per session**<br>No mixed-up contracts | **Persistent terminal**<br>Shell survives app restarts | **Local-only BYOK**<br>No cloud or token proxy |

## Install from source

**Requires:** macOS · Claude Code · [Rust via rustup](https://rustup.rs) · Node.js 20.19.x or ≥22.12

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

<details>
<summary><strong>What the installer does</strong></summary>

Builds the app and CLI, installs <code>/Applications/Witnos.app</code>, verifies the bundled CLI, and opens Witnos. Pass <code>--no-open</code> to keep it closed.
</details>

## Start in 30 seconds

| **1 · Watch** | **2 · Approve** | **3 · Launch** | **4 · Prompt** |
|:---|:---|:---|:---|
| Pick a project | Trust the folder; approve <code>/hooks</code> | Run <code>claude</code> inside Witnos | First prompt becomes the goal |

> [!IMPORTANT]
> Only watched projects use the Stop gate; Claude sessions started in other terminals are untouched. Stuck after a crash? Reopen Witnos or run <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code> from the project root.

<details>
<summary><strong>Compatibility, scope, and recovery</strong></summary>

| | Today |
|---|---|
| Works with | macOS + Claude Code |
| App UI | English + Traditional Chinese |
| Distribution | Source build; unsigned / unnotarized |
| Not yet | Linux testing, Windows persistent terminals, subjective-judgement prompt hook |

Use **stop watching** to opt out permanently. <code>disarm</code> is temporary for an auto-watched project; restarting Witnos re-enables it. Installed hooks remain inert while watching is off.
</details>

<details>
<summary><strong>Build and contribute</strong></summary>

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

Issues and PRs are welcome. For large changes, open an issue first and read the [design constraints](docs/design.md#design-principles-this-is-the-project-not-the-ui). More loops live in [CLAUDE.md](CLAUDE.md); the full rationale is in the [design notes](docs/README.md).
</details>

## License

Copyright © 2026 CHENG YEH TSAI · [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)
