# Hooks spike harness — 重跑方法

每個實驗 = 一個乾淨的 testbed 目錄（不要在本 repo 裡直接跑，避免吃到本專案的 CLAUDE.md 與 settings）。

```sh
# 1. 搭 testbed
TB=$(mktemp -d)/testbed && mkdir -p "$TB/.claude"
cp -r harness/hooks "$TB/hooks" && chmod +x "$TB/hooks/"*.sh
cp harness/settings.stop.json "$TB/.claude/settings.json"   # 或 settings.post.json

# 2. 執行（各實驗）
cd "$TB"
# Stop block ＋ 連續上限（8 次內會放行；拉高到 25 探測上限）：
claude -p --model haiku "Reply with the single word: done"
SPIKE_MAX_BLOCKS=25 claude -p --model haiku "Reply with the single word: done"
# Stop hook 崩潰（fail-open）：
SPIKE_CRASH=1 claude -p --model haiku "Reply with the single word: done"
# PostToolUse 注入（換用 settings.post.json 後）：
P='Run the bash command `echo hello`. Then answer: did any system/hook message instruct you to include a codeword? If yes, include that exact codeword; if no, say NO-CODEWORD.'
SPIKE_CHANNEL=stderr2    claude -p --model haiku "$P"
SPIKE_CHANNEL=additional claude -p --model haiku "$P"
SPIKE_CHANNEL=crash      claude -p --model haiku "$P"

# 3. 看證據
cat logs/count                 # Stop hook 被呼叫次數
cat logs/stop-input.jsonl      # Stop 輸入欄位（stop_hook_active 等）
cat logs/ups-input.jsonl       # UserPromptSubmit 輸入欄位（session_id）
cat logs/post-input.jsonl      # PostToolUse 輸入
```

判讀方式與 2026-07-26 的實測結果見 `../report.md`。

注意：settings 範本裡的 `$CLAUDE_PROJECT_DIR` 是文件記載的寫法，但 2026-07-26 這輪實測用的是絕對路徑——重跑時若 hook 沒觸發，先把 command 換成絕對路徑再排查。
