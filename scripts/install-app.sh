#!/usr/bin/env bash
# Build the frontend, bundle Witnos.app with the tauri CLI, and install it
# into /Applications (replacing the previous copy; a running instance is
# quit first, and the fresh copy is opened at the end — skip with --no-open).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="$ROOT/target/release/bundle/macos/Witnos.app"
DEST="/Applications/Witnos.app"

open_after=1
[[ "${1:-}" == "--no-open" ]] && open_after=0

cd "$ROOT/ui"
[[ -d node_modules ]] || npm install

# The frontend build, the CLI build, and staging the CLI into the bundle
# (Resources/bin/witnos — the hooks it installs point at that absolute path,
# so users never touch PATH) all live in tauri.conf.json's beforeBuildCommand
# now, so a bare `tauri build` produces a correct bundle too. This script used
# to own them, which meant `tauri build` on its own silently bundled whatever
# stale ui/dist and binaries/witnos happened to be lying around.
cd "$ROOT/crates/witnos-app"
"$ROOT/ui/node_modules/.bin/tauri" build

if pgrep -f "Witnos.app/Contents/MacOS/witnos-app" >/dev/null; then
  echo "Quitting running Witnos…"
  osascript -e 'quit app "Witnos"' >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do
    pgrep -f "Witnos.app/Contents/MacOS/witnos-app" >/dev/null || break
    sleep 0.25
  done
fi

rm -rf "$DEST"
ditto "$BUNDLE" "$DEST"

# Verify the bundled CLI is the one just built and actually runs. Checking the
# exec bit alone was worthless: build.rs's old empty-file placeholder runs as an
# empty shell script and exits 0, so "it executed" proved nothing. `cmp` is the
# check that has teeth — it catches the placeholder, a stale copy, and a
# resource that didn't get written, in one comparison.
CLI="$DEST/Contents/Resources/bin/witnos"
[[ -f "$CLI" ]] || { echo "bundled CLI missing at $CLI" >&2; exit 1; }
chmod +x "$CLI"  # resource copying can strip it
cmp -s "$ROOT/target/release/witnos" "$CLI" || {
  echo "bundled CLI is not the one just built: $CLI" >&2
  exit 1
}
# `status` is the one subcommand that answers without a running core (there is
# no --version/--help: the CLI's hand-rolled parser exits 64 on an unknown flag).
"$CLI" status >/dev/null || {
  echo "bundled CLI does not run: $CLI" >&2
  exit 1
}
echo "Installed $DEST (bundled CLI: $CLI)"

if [[ $open_after == 1 ]]; then
  open "$DEST"
fi
