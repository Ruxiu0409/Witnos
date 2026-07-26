# 契約 Schema v1（設計）

- **日期**：2026-07-26。狀態：v1 實作依據；欄位以 `crates/witnos-core` 的型別為準，本檔記「為什麼長這樣」。
- **根據**：README 設計原則 1–6、〈v1 技術選型〉的中心狀態機與上膛協議、〈核心假設〉的儀表化要求。
- **恆守**：schema 與 agent 無關（roadmap 第 4 步的前置）——Claude Code 專屬的東西（hook 名、session_id 來源）只准出現在 adapter 邊界（hook 子命令）內，不准進 schema。

## 實體

### Goal（目標）

| 欄位 | 型別 | 說明 |
|---|---|---|
| `id` | uuid | |
| `title` | string | 使用者下的目標原文 |
| `status` | enum | 見下方狀態機 |
| `contract_version` | u64 | **單調遞增**。任何 item 的新增／修改（不論人或 agent）都 bump；證據不 bump（證據是對既有條目的填充，不是尺的變動） |
| `agent_synced_version` | u64 | agent 最近一次 reconcile 完成時對齊到的版本 |
| `sessions` | list | 綁定的 agent session（`{agent, session_id, bound_at}`；由 UserPromptSubmit hook 建立） |
| `items` | list\<Item\> | |
| `evidence` | list\<Evidence\> | |
| `events` | list\<Event\> | append-only |

**狀態機**（README「閘門的放行 ≠ 項目的通過」的落地）：

```
running ──(放行條件成立，Stop 放行)──► awaiting_rulings   ← 正常終態：agent 收工、主觀項待人裁決
   │
   ├──(回合被截斷：連續 block 上限 8、人中斷)──► turn_ended_unmet
   │        └──(重新下目標／續跑)──► running
   └──(人明確收攤)──► closed        ← UI 必須明說：不再有 agent 讀這裡；要變更就重新下目標
awaiting_rulings ──(人裁決完／收攤)──► closed
```

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
| 專案 `.witnos/armed.json` | `{goal_id, contract_version, agent_synced_version}` | core：開始盯時寫入、每次 bump 與每次 reconcile 鏡寫、優雅停止移除（`watching` 留 true，重啟時重新上膛） |
| 專案 `.witnos/delivered.json` | `{session_id: version}`（送信通道「上次注入到第幾版」） | `witnos hook post-tool-use` 自己寫（純本地，不經網路） |

送信通道的零成本判斷（純本地）：delta 基準 = `max(delivered[session], armed.agent_synced_version)`——agent 可證明已看過的最新版本；基準 ≥ `armed.contract_version` → 靜默放行，不碰網路。（若只用 delivered 起算，會把 agent 已 reconcile 過的條目整批重灌，違反「注入永遠是 delta」。）

## HTTP API（core；`127.0.0.1` 臨時 port ＋ bearer token）

| 路由 | 用途 |
|---|---|
| `POST /gate` | Stop 守門判定：`{session_id, cwd}` → `{decision, reason?, against_version}`。判定邏輯全在 core；gate bin 只是 fail-closed 的信使 |
| `GET /goals/{id}/contract?since=V` | 取 delta（送信、reconcile 用） |
| `POST /goals/{id}/items` | lay（batch；agent 或 UI；origin 必填） |
| `PATCH /goals/{id}/items/{iid}` | 更新詮釋、oracle 結果回報等 |
| `POST /goals/{id}/evidence` | 附證據 |
| `POST /goals/{id}/reconcile` | `{session_id, to_version, ...}` → 更新 `agent_synced_version` |
| `POST /goals/{id}/events` | UI 回報 drill_down、ruling |

## Agent 寫入路徑（本次決定）

**走同一支 headless bin 的子命令，agent 用 Bash 呼叫。** 理由：能跑 shell 指令是所有 coding agent 的最大公約數（Codex 也只有 command hook）；endpoint／token 的處理封裝在 bin 內，prompt 端完全不碰憑證；與「單一語言、單一 repo 承重」一致。MCP 之類的整合等 roadmap 第 4 步（agent-agnostic schema 抽象）時再議。

Agent 面向：`witnos contract show [--since N]`／`witnos item lay`（stdin JSON、batch）／`witnos item interpret <id>`／`witnos evidence add <item-id>`（stdin JSON）／`witnos reconcile --to N`
Hook 入口：`witnos hook stop`／`witnos hook post-tool-use`
人面向：`witnos init`／`witnos arm <goal-id>`／`witnos disarm`／`witnos status`

### bin 命名修正（對設計文件的小修）

原文件同時出現「`witnos-gate` bin」與「`witnos disarm` 指令」——收斂成**單一 headless bin，名為 `witnos`**；「gate」是其中的 `hook` 子命令家族。原約束不變：這支 bin 不依賴 `tauri` crate、不連結 webview、毫秒級啟動、headless/CI 可跑。
