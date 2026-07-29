# Claude Code hooks 行為實測（spike）

- **日期**：2026-07-26
- **環境**：Claude Code 2.1.220 / macOS；以巢狀 `claude -p --model haiku` headless 執行（hook 行為與模型無關）
- **目的**：README〈v1 技術選型〉實作備忘裡 2026-07-03 的 hooks 查核，在動工前以現行版本重驗。這些行為是 `witnos-gate`（守門 fail-closed）與送信通道（PostToolUse）的承重牆。
- **方法**：見 `harness/README.md`（可重跑）。

## 結論總表

| # | 待驗行為 | 結果 |
|---|---------|------|
| 1 | Stop hook `{"decision":"block","reason":…}` 擋得住停止、reason 餵得回去 | ✅ 8 連擋全部有效；agent 每輪都照 reason 指示行動（reason 能引導 agent） |
| 2 | 連續 block 有上限 | ✅ **上限 = 8**：第 9 次連續 block 的 reason 進 transcript 但回合直接結束；headless 下 stdout 為空、stderr 無警告 |
| 3 | `stop_hook_active` 輸入欄位 | ✅ 仍存在（現行文件已不載）：首擋 `false`、其後 `true` |
| 4 | PostToolUse exit 2 + stderr 注入 | ✅ 模型看得到（codeword 測試通過） |
| 5 | PostToolUse `hookSpecificOutput.additionalContext` | ✅ **與 2026-07-03 記載相反，現已支援**且實測有效（文件：注入位置在 tool result 旁） |
| 6 | hook 崩潰（exit 1）fail open | ✅ Stop 與 PostToolUse 都被無聲放行，session 正常結束 |
| 7 | `session_id` 存在於 UserPromptSubmit / Stop 輸入 | ✅ 都有；Stop 輸入另有 `last_assistant_message`、`transcript_path`、`cwd`、`permission_mode`、`prompt_id` |
| 8 | command hook 預設逾時 | 文件：600 秒、可 per-hook `timeout` 覆寫（未實測） |
| 9 | http hook fail open | 文件再度證實：「HTTP hooks can't signal a blocking error through status codes alone」（未實測） |

## 對設計的影響

1. **送信通道改用 `additionalContext`**（較 exit 2 + stderr 乾淨；後者實測也通，留作 fallback）。
2. 舊記載的 `continueOnBlock` 已不在文件中——設計文件已刪除該句，不要依賴。
3. **連續 block 上限有具體數字（8）**：中心狀態機必須有「回合已結束、未達放行條件」的入帳路徑（原設計已預留，數字補上）；`witnos-gate` 的 block reason 應在接近上限時提醒 agent 先把已達成的部分 reconcile 回 store。
4. **fail-closed 必須由 `witnos-gate` 自己保證**的立論成立：hook runner 對崩潰、逾時、非 0 非 2 退出一律放行。
5. **目標↔session 綁定可行**：UserPromptSubmit 輸入有 `session_id`。
6. 新發現的**信任前提**：未受信任的資料夾會忽略專案 settings 的 `permissions.allow`（實測該情況下 hooks 仍會執行，但別依賴這行為）→ `witnos init` 流程要納入「資料夾必須先受信任」。

## 補測（2026-07-29，同版 2.1.220）

UserPromptSubmit 輸入**含 `prompt` 欄位**（使用者原文）——auto 模式「以首個 prompt 命名 goal」的依據。原始樣本：

```json
{"session_id":"3a43039f-…","transcript_path":"…","cwd":"…","prompt_id":"9a5535e4-…","permission_mode":"default","hook_event_name":"UserPromptSubmit","prompt":"hello witnos ups probe"}
```

方法：scratch 目錄裝一個 `cat >> ups-input.jsonl` 的 UPS hook，`claude -p "hello witnos ups probe" --model haiku` 一發即得。

## 殘留待驗（下次落地前）

- **prompt / agent 型 hook 未測**（主觀判斷要用它）——v1 動工前補測。
- settings 裡 `$CLAUDE_PROJECT_DIR` 的展開未實測（本輪 command 用絕對路徑）。
- 上限 8 與各欄位皆為 2.1.220 實測值，**版本相依**；hook API 演化快（現有 30 種事件，含 PostToolUseFailure / StopFailure 等新事件），落地前用本 harness 重跑。

## 附錄：關鍵原始證據

第 1 次 Stop 輸入（節錄）：

```json
{
  "session_id": "3ec73634-1f5b-4f6f-9c82-33f579873bcc",
  "hook_event_name": "Stop",
  "stop_hook_active": false,
  "last_assistant_message": "done",
  "permission_mode": "default"
}
```

第 9 次（連擋 8 次之後）：`stop_hook_active: true`、`last_assistant_message: "CONTINUE-8"`。

上限探測（`SPIKE_MAX_BLOCKS=25`）的 transcript 結尾——第 9 次 block 的 reason 已送達、但無後續 assistant 回應，回合就此結束：

```
TYPE: assistant | CONTINUE-8
TYPE: user      | Stop hook feedback:
                  SPIKE round 9: do not stop yet. Reply with exactly CONTINUE-9 …
TYPE: last-prompt
```

PostToolUse 注入測試的模型回答：stderr 通道回「codeword is: **MANGO**」、additionalContext 通道回「codeword: **PINEAPPLE**」、crash 通道回「NO-CODEWORD」（即無聲放行）。
