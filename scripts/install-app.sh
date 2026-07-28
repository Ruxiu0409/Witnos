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
npm run build

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
echo "Installed $DEST"

if [[ $open_after == 1 ]]; then
  open "$DEST"
fi
