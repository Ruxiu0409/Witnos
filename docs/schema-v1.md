# 契約 Schema v1（設計）

- **日期**：2026-07-26；2026-07-29 增補 auto 模式（每 session 一目標）與 marker v2。狀態：v1 實作依據；欄位以 `crates/witnos-core` 的型別為準，本檔記「為什麼長這樣」。
- **根據**：README 設計原則 1–6、〈v1 技術選型〉的中心狀態機與上膛協議、〈核心假設〉的儀表化要求。
- **恆守**：schema 與 agent 無關（roadmap 第 4 步的前置）——Claude Code 專屬的東西（hook 名、session_id 來源）只准出現在 adapter 邊界（hook 子命令）內，不准進 schema。

## 實體

### Goal（目標）

| 欄位 | 型別 | 說明 |
|---|---|---|
| `id` | uuid | |
| `title` | string | 使用者下的目標原文（auto 模式：session 首個 prompt 壓空白、80 字元截斷——截斷住在 adapter 邊界，不進 schema） |
| `status` | enum | 見下方狀態機 |
| `contract_version` | u64 | **單調遞增**。任何 item 的新增／修改（不論人或 agent）都 bump；證據不 bump（證據是對既有條目的填充，不是尺的變動） |
| `agent_synced_version` | u64 | agent 最近一次 reconcile 完成時對齊到的版本 |
| `sessions` | list | 綁定的 agent session（`{agent, session_id, bound_at}`；由 UserPromptSubmit hook 建立） |
| `items` | list\<Item\> | |
| `evidence` | list\<Evidence\> | |
| `events` | list\<Event\> | append-only |
| `project_dir` | string? | 被監看的專案目錄（armed marker 的家） |
| `watching` | bool | true = core 在該目錄維護 marker、Stop 守門在那裡 fail closed |
| `auto_session` | string? | **auto 模式**：此目標由哪個 session 的首個 prompt 自動建立（一 session 一目標）。null = 人手動建立 |

**auto 模式（2026-07-29 定案）**：人把專案目錄設為「自動監看」（進 `~/.witnos/projects.json`，見檔案佈局）後，每個新 agent session 的第一個 prompt 由 UserPromptSubmit hook 自動 `POST /goals/auto` 建立該 session 專屬目標並上膛。**每個 session 只被自己目標的契約 gate**——「證明 agent 看過你的編輯」的信任基礎不允許 A session 被 B 的契約攔。goal 建立以 `auto_session` 在 store 寫鎖內去重（hook 重複觸發不會建兩個）；人對某目標 unwatch／close 後，該 session 的重試**不得**重新 watch（per-goal opt-out 優先，原則 5）。`/clear`、resume 產生新 session id → 新目標，是所選語意不是 bug。

**狀態機**（README「閘門的放行 ≠ 項目的通過」的落地）：

```
running ──(放行條件成立，Stop 放行)──► awaiting_rulings   ← 正常終態：agent 收工、主觀項待人裁決
   │
   ├──(回合被截斷：連續 block 上限 8、人中斷、session 結束時仍在跑
   │    ——SessionEnd hook 入帳，防「殭屍 running」)──► turn_ended_unmet
   │        └──(續跑：同 session 回來再觸發守門 → 自動復活；或重新下目標)──► running
   └──(人明確收攤)──► closed        ← UI 必須明說：不再有 agent 讀這裡；要變更就重新下目標
awaiting_rulings ──(最後一個 laid 主觀項獲人裁決)──► ruled   ← 已無任何項等人裁；仍是收攤狀態
ruled ──(rejected 項補上新證據、回到 laid)──► awaiting_rulings
awaiting_rulings / ruled ──(人收攤)──► closed
```

收攤期間 `awaiting_rulings` ⇄ `ruled` 是 item 狀態的**純導出**（有無 `laid` 主觀項），每次 store 寫入與載入時重算——`ruled` 引入前存檔的 goal 開檔即癒合，不需人重新裁決。

### Item（驗證項）

| 欄位 | 型別 | 說明 |
|---|---|---|
| `id` | uuid | |
| `claim` | string | 主張（「什麼算做到」） |
| `check` | string | 怎麼驗（給 agent 的操作性描述） |
| `class` | enum | `subjective`（**預設**）／ `objective { oracle: {command, expected}, promoted_by }` |
| `interpretation` | string? | agent 對這條的當前詮釋——**主觀項必填**，沒有詮釋的主觀項不算 laid |
| `interpretation_history` | list | `{text, against_version, at}`。**新增一筆 = 一次重新詮釋事件**，這是原則 6「主動標記」的資料來源 |
| `status` | enum | 見下 |
| `evidence_ids` | list | |
| `origin` | enum | **核心假設的儀表**，見下 |
| `added_in_version` / `last_edited_version` | u64 | 證據過期判定用：evidence.against_version < item.last_edited_version → 該證據對不上現在這條尺 |

**分類規則（畫死，Goodhart 側門）**：預設一律 `subjective`；只有帶機器可執行 oracle（一條指令＋預期輸出）才可為 `objective`；人可明確升級某條為 objective（`promoted_by: human`，人自己扛）。**agent 不得自行把無 oracle 的項標成 objective**——core 在寫入時拒絕。分類錯誤永遠往「多給人看」倒。

**status**：

```
open ──(agent 攤出詮釋＋證據)──► laid
laid ──(objective：oracle 通過，agent 自過)──► passed
laid ──(subjective：人點頭)──► approved
laid ──(subjective：人打槍)──► rejected ──(goal 仍 running → agent 下輪 reconcile 必須處理)──► open
```

`rejected` 發生在 goal 已收攤（`awaiting_rulings`）時不回 `open`——那是「重新下目標」的觸發器（原則 5 的邊界），不是本輪的工作。

**origin（每條驗證項的出身——強版假設的直接讀數）**：

| 值 | 對應 README 的 |
|---|---|
| `user_pre_run` | (a) 開跑前寫的 |
| `user_viewing_evidence { evidence_id }` | **(b) 看著某條證據時加的——強版假設的計數器，必記是哪條證據** |
| `user_mid_run` | (c) 執行中自發想到的 |
| `agent_initial` | agent 初版契約攤的 |
| `agent_blindspot` | blindspot pass 提的候選（預設主觀、待人裁決） |

### Evidence（證據）

| 欄位 | 說明 |
|---|---|
| `id`, `item_id` | |
| `conclusion` | agent 的結論（「我判定這條目前如何」） |
| `basis` | 據以判斷的內容（色票清單、量到的數字、diff 摘要…） |
| `provenance` | list：`file {path, lines?}` ／ `command {cmd}` ／ `url {url}` ——每條證據**必附至少一個**出處指標，UI 一鍵開原物（「（一）自帶不足感測器」的前提） |
| `workspace` | `{commit?, dirty_hash?}` 擷取當下的 workspace 指紋；代碼之後又動 → UI 標「證據已過期」 |
| `against_version` | 「對齊到第 N 版」的章——事後裁判的信任基礎 |
| `captured_at` | |

### Event（append-only 事件流）

`contract_edited {item_id, by, origin, version_after}`／`evidence_added`／`reconcile {session_id, from_version, to_version, changed_items, reinterpreted_items}`／`gate_decision {decision, reason, against_version}`／`drill_down {evidence_id, pointer}`（人點開原物）／`ruling {item_id, verdict, after_drill_down}`／`turn_ended {met}`

事件流就是三份儀表的原始資料：origin=(b) 的計數（強版假設讀數）、「drill_down 之後改裁決／加條目」的序列（（二）篩選規則的需求規格）、分流維度（原則 4）的觀察資料。

## 放行條件（Stop 守門的判定，住在 core）

```
release ⇔ ∀ objective item: status == passed
        ∧ ∀ subjective item: status ∈ {laid, approved}（詮釋＋至少一條證據已攤）
        ∧ agent_synced_version == contract_version（已對齊最新契約）
        ∧ 無 rejected-未處理項（goal running 時）
```

block 的 reason **永遠是 delta**：缺哪幾條、哪幾條證據 stale、版本落後多少。連續 block 接近上限（spike 實測 = 8）時，reason 要改口氣：「回合可能被截斷，先把已完成部分 reconcile 回 store」。

## 檔案佈局

| 路徑 | 內容 | 寫者 |
|---|---|---|
| `~/.witnos/goals/<goal-id>.json` | 一目標一檔（serde-JSON，core 行程內 `RwLock` 單寫者） | core |
| `~/.witnos/endpoint.json` | `{port, token}`，mode 0600 | core（啟動時） |
| `~/.witnos/projects.json` | `{v, projects: [dir…]}`——哪些目錄開了 auto 模式（canonicalize 去重）。**人專屬面**：只走 app 的 IPC，永不上 HTTP——agent 不得替目錄開關監看 | core（app IPC 觸發） |
| 專案 `.witnos/armed.json` | **marker v2**：`{v: 2, auto, default_goal?: {goal_id, contract_version, agent_synced_version}, sessions: {session_id: {…同左}}}`。整檔是 `(projects.json ∋ dir, store 內該 dir 的 goals)` 的**純推導**（只有 watching 的 goal 進得來；session 自己的 auto goal 佔自己的槽），tmp+rename 原子寫入——同目錄多 goal 不再互相蓋寫。**auto 專案零 goal 也保留 marker（照樣 fail closed）**。舊版單 goal 形狀仍可讀（正規化為 `default_goal`） | core：watch／每次 bump／每次 reconcile 重推導，優雅停止移除（`watching` 與 registry 留著，重啟時重新上膛） |
| 專案 `.witnos/delivered.json` | `{session_id: version}`（送信通道「上次注入到第幾版」） | `witnos hook post-tool-use` 自己寫（純本地，tmp+rename） |
| 專案 `.witnos/instructed.json` | `{session_id: unix_ts}`（協議已注入過的 session） | `witnos hook user-prompt-submit` 自己寫（純本地，tmp+rename）。**auto 模式下建目標失敗不寫**——下個 prompt 重試，短暫斷線自癒 |

送信通道的零成本判斷（純本地）：先以 session_id 解析 marker 條目（`sessions[sid]` → 沒有則 `default_goal` → 都沒有 → 靜默），delta 基準 = `max(delivered[session], 條目.agent_synced_version)`——agent 可證明已看過的最新版本；基準 ≥ `條目.contract_version` → 靜默放行，不碰網路。（若只用 delivered 起算，會把 agent 已 reconcile 過的條目整批重灌，違反「注入永遠是 delta」。）

Stop 守門的 session 解析：`sessions[sid]` 有條目 → 以該 goal 問 core；auto 且無條目 → 帶 `{project_dir, session_id}` 問 core（core 分三種：goal 存在且 watching → 正常判定並順手治癒 marker；人明確 opt-out → **release**（fail-closed 防的是靜默失效，不是人的明確決定）；真的沒 goal → block，理由文即逃生說明）；手動 marker 卻無 goal 可解析（只可能是手壞檔）→ block。core 不可達 → 一律 block（不變）。

## HTTP API（core；`127.0.0.1` 臨時 port ＋ bearer token）

| 路由 | 用途 |
|---|---|
| `POST /gate` | Stop 守門判定：`{goal_id?}` 或 `{project_dir, session_id}`（auto 模式 session 解析，見檔案佈局節）→ `{decision, reason?}`。判定邏輯全在 core；gate bin 只是 fail-closed 的信使 |
| `POST /goals` | 建目標（`{title}`；手動流） |
| `POST /goals/auto` | **auto 模式建目標**：`{title, project_dir, session_id, agent?}`，以 `auto_session` 冪等；已 opt-out 的目標原樣回傳（`watching: false`）、不重新 watch |
| `POST /goals/{id}/watch`／`DELETE 同路徑` | 盯／不盯（`{project_dir}`）；觸發該目錄 marker 重推導 |
| `GET /goals/{id}/contract?since=V` | 取 delta（送信、reconcile 用） |
| `POST /goals/{id}/items` | lay（batch；agent 或 UI；origin 必填） |
| `POST /goals/{id}/items/{iid}/edit` | 改尺（人；agent 只能改自己攤的） |
| `POST /goals/{id}/interpret`／`/evidence`／`/oracle` | 詮釋、附證據、oracle 結果回報 |
| `POST /goals/{id}/reconcile` | `{session_id, to_version, ...}` → 更新 `agent_synced_version` |
| `POST /goals/{id}/sessions` | 綁定 session（UserPromptSubmit hook 用，best-effort） |
| `POST /goals/{id}/turn-ended` | SessionEnd hook 的入帳：goal 仍 `running` → `turn_ended_unmet`（否則 no-op）。記帳不是守門，fail open |
| `POST /goals/{id}/rulings`／`/drilldown` | 人裁決、drill-down 記錄（UI 實際走 IPC；HTTP 面為完整性保留） |

（goal 刪除與 auto 專案註冊表**不在** HTTP 面上——那是人的動作，只走 app IPC。）

## Agent 寫入路徑（本次決定）

**走同一支 headless bin 的子命令，agent 用 Bash 呼叫。** 理由：能跑 shell 指令是所有 coding agent 的最大公約數（Codex 也只有 command hook）；endpoint／token 的處理封裝在 bin 內，prompt 端完全不碰憑證；與「單一語言、單一 repo 承重」一致。MCP 之類的整合等 roadmap 第 4 步（agent-agnostic schema 抽象）時再議。

Agent 面向（**全部吃 `--goal <id>`**；同專案多個 active goal 時必帶——Bash 呼叫不帶 session 身份，環境式解析會跨 session 汙染，故 goal 身份走 in-context：協議注入、delta、block reason 都印著它；marker 只解析得出唯一 goal 時可省略）：`witnos contract show [--since N]`／`witnos item lay [--blindspot]`（stdin JSON、batch；origin 由 CLI 蓋章，agent 不能自稱 user 出身）／`witnos item interpret <id>`／`witnos evidence add <item-id>`（stdin JSON，自動蓋 git workspace 指紋）／`witnos oracle report <id> --passed|--failed`／`witnos reconcile --to N`
Hook 入口：`witnos hook stop`／`witnos hook post-tool-use`／`witnos hook user-prompt-submit`（fail open；auto 模式下無條目的新 session → `POST /goals/auto` 建目標＋注入協議；失敗不記 instructed、下個 prompt 重試。協議文字帶 bin 絕對路徑，不依賴 PATH）／`witnos hook session-end`（fail open 的記帳：session 結束時自己名下的 goal 仍在 running → 入帳 `turn_ended_unmet`；只認 marker `sessions` 的精確條目，default goal 不受單一 session 結束影響）
人面向：`witnos init`（把四個 hook 冪等合併進專案 `.claude/settings.json`；app 的「監看專案」會用 bundle 內的 bin 代跑）／`witnos goal new <title>`（建目標＋盯當前專案；手動流）／`witnos arm <goal-id>`／`witnos disarm`（auto 專案會提示：registry 還在，app 重啟會重新上膛；要停用去 app 移除）／`witnos status`（渲染 marker v2：auto 旗標、default goal、各 session 條目）

### bin 命名修正（對設計文件的小修）

原文件同時出現「`witnos-gate` bin」與「`witnos disarm` 指令」——收斂成**單一 headless bin，名為 `witnos`**；「gate」是其中的 `hook` 子命令家族。原約束不變：這支 bin 不依賴 `tauri` crate、不連結 webview、毫秒級啟動、headless/CI 可跑。
