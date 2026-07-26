#!/bin/bash
# UserPromptSubmit hook：把輸入 JSON 原樣記下，驗證 session_id 等欄位。
BASE="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$BASE/logs"
cat >> "$BASE/logs/ups-input.jsonl"
exit 0
