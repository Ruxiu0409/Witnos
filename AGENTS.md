# Repository Guidelines

## Project Structure & Module Organization

Witnos is a Cargo workspace with four Rust crates:

- `crates/witnos-core`: domain types, JSON store, and gate rules.
- `crates/witnos-server`: Axum service; `examples/serve.rs` is the headless runner.
- `crates/witnos-cli`: the `witnos` binary and CLI integration tests.
- `crates/witnos-app`: Tauri desktop shell and bundled icons/configuration.

The React/TypeScript UI lives in `ui/src`, with assets in `ui/public`. Contract design is in `docs/schema-v1.md`; hook experiments belong under `spike/`. Keep `README.md` (Traditional Chinese) and `CLAUDE.md` (English) synchronized.

## Build, Test, and Development Commands

- `cargo test --workspace --exclude witnos-app`: fast Rust tests without the heavy Tauri build.
- `cargo test --workspace`: full workspace verification.
- `cargo test -p witnos-cli --test full_loop`: run the end-to-end v1 workflow.
- `cargo clippy --workspace --all-targets`: lint all Rust targets; keep zero warnings.
- `cargo fmt --all --check`: verify Rust formatting.
- `cargo run -p witnos-server --example serve`: start the local headless core.
- `cd ui && npm install && npm run build`: install and build the frontend.
- `cd ui && npm run dev`: run Vite locally; `npm run lint` runs Oxlint.
- `cargo run -p witnos-app`: launch the desktop shell after `ui/dist` exists.

## Coding Style & Naming Conventions

Use `rustfmt` and four-space Rust indentation. Name modules/functions in `snake_case`, types in `PascalCase`, and constants in `SCREAMING_SNAKE_CASE`. TypeScript uses two spaces; React components and files use `PascalCase` (for example, `TerminalPanel.tsx`), while helpers use `camelCase`. Keep the CLI independent of `tauri`; shared domain behavior belongs in `witnos-core`.

## Testing Guidelines

Rust uses `#[test]` plus integration tests in `crates/*/tests`. Use behavior names such as `evidence_requires_provenance`; add regression coverage near the affected crate. There is no numeric coverage threshold. Changes should pass the fast suite, focused tests, Clippy, and UI build/lint when applicable.

## Commit & Pull Request Guidelines

Recent commits use concise, imperative summaries, optionally scoped (`UI: ...`, `Dev commands: ...`). Keep each commit focused. Pull requests should explain behavior and design impact, list verification commands, link relevant issues, and include screenshots or recordings for UI changes. Call out schema or hook-protocol changes explicitly and update both design documents when decisions change.

## Security & Configuration

Never commit endpoint tokens, `.witnos` runtime state, credentials, or generated build output. Preserve the armed gate’s fail-closed behavior and the delivery hook’s fail-open behavior. Do not add license headers or a `LICENSE` file until the project chooses a license.
