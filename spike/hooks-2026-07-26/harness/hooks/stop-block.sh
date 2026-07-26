#!/bin/bash
# Stop hook：記錄 stdin、連續 block 最多 SPIKE_MAX_BLOCKS 次後放行。
# SPIKE_CRASH=1 → 模擬 hook 崩潰（exit 1），測 fail-open。
BASE="$(cd "$(dirname "$0")/.." && pwd)"
LOG="$BASE/logs"
MAX_BLOCKS="${SPIKE_MAX_BLOCKS:-8}"
mkdir -p "$LOG"
INPUT=$(cat)
if [ -n "$SPIKE_CRASH" ]; then
  echo "SPIKE: simulated stop-hook crash" >&2
  date >> "$LOG/crash-fired"
  exit 1
fi
N=$(cat "$LOG/count" 2>/dev/null || echo 0)
N=$((N + 1))
echo "$N" > "$LOG/count"
printf '%s\n' "$INPUT" >> "$LOG/stop-input.jsonl"
if [ "$N" -le "$MAX_BLOCKS" ]; then
  printf '{"decision":"block","reason":"SPIKE round %s: do not stop yet. Reply with exactly CONTINUE-%s and then try to finish again."}\n' "$N" "$N"
fi
exit 0
