# Witnos

> 名字取自 **witness**（見證）——agent 做的每件事都在使用者眼皮底下被看著。

讓 AI coding agent 的「驗證」這一步從黑盒變透明、且能由人**即時協作編輯**的工具。

**狀態：v1 實作進行中（2026-07-26 動工）。** 本檔記錄設計脈絡與待驗證假設。已落地：hooks 行為 spike（`spike/hooks-2026-07-26/`）、契約 schema（`docs/schema-v1.md`）、Rust workspace——`witnos-core`（型別／store／放行條件）、`witnos-server`（axum 核心，lib 形式、GUI 外殼內嵌）、headless bin `witnos`（雙 hook＋agent 子命令）、Tauri 外殼與前端。未動工：主觀判斷 prompt hook。（`witnos init`、UserPromptSubmit 綁定＋協議注入 hook、`witnos goal new` 已落地；契約書寫規範由 UserPromptSubmit hook 每 session 注入一次，不寫進任何檔案。**2026-07-29 落地 auto 模式**：在 app 選一個專案目錄開「自動監看」後，**在 app 內建終端機裡啟動的** agent session，第一個 prompt 自動建立該 session 專屬的目標並上膛；在其他終端機裡啟動的 session 不在範圍內——不建目標、不注入協議、也不會被攔停（範圍記號 `WITNOS_TERMINAL`，app 開的每個 shell 都蓋上，agent 與它跑的 hook 都繼承得到）。marker 升級為 session-keyed 的 v2、`witnos` bin 隨 app 打包、hooks 與協議全用絕對路徑，使用者不必碰 PATH。）開發指令見 `CLAUDE.md`。

---

## 這是什麼

一個 local-first、開源的薄層工具。它掛在你既有的 coding agent（Claude Code / Codex 等）之上，把 agent 在長時間自動執行任務時，**它打算怎麼驗證「自己有沒有做到」這件事攤開給你看，並且讓你能在過程中補上、修正它的驗證標準。**

它不是一個 agent，也不取代你的 agent。你自帶 agent，它只負責讓那個 agent 的驗證環節變得可被人看見、可被人介入。

---

## 為什麼需要它

現在的 agent 大致跑一個迴圈：**推理 → 行動 → 觀察 → 重複**（近期被稱為 loop engineering）。問題出在「觀察／驗證」這一步。

真正的問題不是「目標客觀還是主觀」，而是：**使用者跟 agent 對「怎麼算做到了」的認知，有沒有可能不一樣。** 只要有這個落差的空間，agent 就會拿它自己那套標準去自我驗證、然後回報「完成了」，而那不是使用者要的東西。

這條線跟「主客觀」不重合。主觀目標當然容易有落差——「做一個 Apple 風格的 App」，雙方對「Apple 風格」的認知本來就不同。但**看似客觀的目標，驗證方式本身也可能有歧義**：「能不能 build」聽起來很硬，可是「在哪個環境、哪個版本、要不要連測試一起綠才算過」——光是「怎麼算 build 過了」就有解讀空間。所以重點不在目標的性質，在於**雙方對驗證方式的理解是否一致**。

這個落差本身是個老問題（specification gap、oracle problem、Goodhart's law 都是它的親戚），但**長時間自動執行**讓它變嚴重：在人不在場、agent 連續跑很久的情境下，第一步沒被抓到的偏差會變成下一步的地基，一路無聲地複利累積，等 agent 說「完成」時，歪掉的已經是一整條決策鏈，而不是一個檔案。

**核心觀點：只要「怎麼算完成」存在使用者與 agent 認知不一致的空間，agent 就不該獨自決定自己過關。人需要一個能看見、且能介入那把尺的地方。**

---

第一版的完整迴圈（用三個角色講：**使用者** = 那位有判斷力的工程師；**Agent** = Claude Code 那類實際寫 code 的 AI；**工具** = 本專案，中間那層介面）：

1. **使用者**給目標。
2. **Agent** 列出「我要驗什麼、怎麼驗、結果是什麼」，攤在**工具**上。
3. **Agent** 開始跑。
4. **使用者**隨時能在**工具**上加一條、改一條驗證——**不打斷 Agent**。
5. **Agent** 每一輪檢查時，讀的是當下最新版的驗證內容（這就是「活契約」：執行中持續被重讀比對，不是執行前讀一次的死文件）。
6. 若 Agent 發現現在做的跟最新版不符，它**照自己的理解先回去改 code**，然後把「我把你這條理解成 XXX、我改了這些、現在的證據長這樣」攤回**工具**上。
7. **使用者**下次看的時候，判斷 Agent 改得對不對。

**這個迴圈就是本專案，其他都是外圍。**

### 收尾為什麼是「事後裁判」
第 6、7 步是個刻意的選擇。當使用者改的是一條**主觀**驗證（例如「只能黑白灰」），「Agent 算不算符合這條」本身又是一次主觀判斷——照本專案原則該由使用者裁判。但若每次都停下來等使用者確認，又會不斷打斷。

折衷是讓裁判**發生在事後**：如第 6、7 步，Agent 不停下、照自己的理解先改、並把它的理解攤回面板，使用者在下次看的時候才裁判。這樣不打斷流程，又守住「主觀的事使用者說了算」。代價與配套見設計原則 6。

### 為什麼價值在「趁早」
迴圈本身保證最後會收斂到「符合最新版清單」；本工具的價值不在「保證正確」，而在**讓人有機會趁偏差還沒擴散成下游地基時、用最低的返工成本就攔截它**。改得越早，Agent 回頭要動的東西越少。

---

## 設計原則（這些是這個專案的真正內容，不是 UI）

### 1. 證據優先於意圖（Evidence over intent）
不要只讓 agent 寫下「我打算驗證什麼」——意圖是會說謊的，「我確認過符合 HIG」這種話沒法檢查。要讓 agent 攤出**它據以判定完成的證據**：它做出來的截圖、偵測到的色票清單、量到的對比度數字。

理由：人很不會「主動回憶起自己沒講的期待」，但人很會「被眼前的證據戳到」。當色票清單上跳出五種顏色，使用者「只要黑白灰」那個默會期待會被**勾出來**，他不需要事先想到它。把問題從「靠人主動回憶」變成「靠證據被動觸發」。

### 2. 主觀項目，人是最終裁判（Goodhart 護身符）
本工具會引導 agent 把模糊標準拆解成可檢查的代理指標（把「Apple 風格」拆成色票數、字體、圓角半徑……）。**但代理指標永遠只是「溝通用的鷹架」，不是裁判。**

- **客觀項目**：agent 可以自己打勾、自己決定通過與否。
- **主觀項目**：代理指標只負責「把證據端到人面前」，**通過與否一定要人點頭**。

一旦為了「更自動」而讓主觀項目也能靠代理指標自動通過，就會掉進 Goodhart 陷阱——agent 去滿足那些數字，做出一個「每項指標都對、但整體就是很怪」的東西。這條線必須守死。

這條線還有一個側門：**「哪條算客觀」的分類若由 agent 自己決定，它把主觀項誤標成客觀，就等於繞過裁判。** 所以分類規則畫死：**預設一律主觀；只有掛著機器可執行 oracle（一條指令＋預期輸出）的項目才算客觀**；人可以明確把某條升級為客觀，那是人自己扛。分類錯誤的方向，永遠往「多給人看」倒。

### 3. 活契約，不是事前規格（Living contract, not upfront spec）
不依賴「使用者一開始就把需求講清楚」——主觀、默會的東西本來就講不清楚，講得清楚就不需要人了。驗證清單在整個迴圈過程中持續可編輯、持續被重新比對。

### 4. 分流，只讓需要人腦的浮上來（Triage what surfaces）
寫得越詳細，人要看的越多——這是個真實矛盾。三小時的任務可能驗幾百項，若全部攤出來，使用者根本看不完，於是乾脆不看，做了等於沒做。解法不是「希望人有耐心讀完」，而是**分流**：讓 agent 自己消化掉那些安全、不需要人管的項目，只把真正需要人判斷的少數浮上來。

（具體用什麼維度來判斷「哪些需要人判斷」，刻意先不寫死，留待實作時看真實資料再定。見 Roadmap。）

### 5. 控制的單位是「單一目標」，監控可逐目標自由加入（Per-goal monitoring）
先澄清執行模型：一個專案通常是一連串**獨立下達**的目標（例如 20 個），每次下一個目標就是一次獨立的執行，彼此不重疊。在**單一目標的執行之內**，Agent 一路跑到完成，過程中持續回頭比對活契約（即〈怎麼運作〉的核心迴圈）——它**不會停下來等人**。

所以「想掌握時才掌握、平常去忙別的」**不是**「一次執行裡要不要設一道停下等人的閘門」，而是更單純的一件事：**使用者逐個目標決定，這一個我要不要在旁邊看著、即時編輯它的驗證。**

- **想盯的目標** → 在旁監控，邊跑邊編輯驗證（核心迴圈第 4–7 步），趁早對齊。
- **沒空的目標** → 不在場，它自己跑完；若結果不對，就重新下這個目標。

監控是**逐目標、隨時可加入**的：可以盯前 5 個、放掉中間 15 個、等突然有空再回來盯最後 5 個。因為每個目標都是獨立的一次下達，「現在開始盯下一個」隨時成立，不需要事先決定。

### 6. 事後裁判要主動標記，否則等於 Agent 自己拍板
「事後裁判」（見〈怎麼運作〉）有個內建責任：既然使用者的裁判發生在事後，工具就有義務確保使用者**不會錯過該裁判的那一刻**。

如果 Agent 默默照自己的理解改了、攤回面板，但這件事被埋在一堆「已通過」的項目裡、使用者沒注意到「它對我這條的理解跟我想的不一樣」——那使用者就只是**名義上**有裁判權、實際上沒行使，結果等於 Agent 自己拍板。

所以工具必須把「**Agent 對某條主觀驗證的理解或改動**」主動標記出來、讓使用者一眼看到，而不是混在已通過項裡。這其實就是原則 4「分流」的同一個機制——Agent 對主觀項的新理解，正是那種「該浮上來給人看」的東西。

---

## 技術路線

### 整合模型：自帶 agent（BYOK）
使用者自帶他自己的 coding agent 與認證憑證。本工具不碰使用者的 token、不代理憑證。憑證與費用都留在使用者那邊。（參考 Open Design 的做法：本機 daemon + web 介面，掃描 PATH 找出可用的 agent CLI，用 per-agent adapter 接上去。）UI 的形式則已定案：**跨平台桌面應用程式，不是瀏覽器網頁**——見下方〈v1 技術選型〉。

**注意：不要走「挖出訂閱登入 token 自己驅動」這條路**——第三方 agent 以訂閱額度計費已被禁止（2026/02），且 OAuth token 會過期、技術上脆弱。正路是 API key，或（自用情境下）官方的 Agent SDK credit。

### 綁定機制：薄層掛在既有 agent 的 hook 上（Path A）
不自建 agent runtime；蓋在既有 agent 的 lifecycle hook 之上。

目前最適合原型的基座是 **Claude Code**：

- **Stop hook**：在 agent 自認為「完成、要停」時觸發，可回傳 block 強制它繼續做，直到滿足放行條件（見〈v1 技術選型〉的中心狀態機）。這就是「攔下『我完成了』」的機制核心。
- **http hook**：可帶 Authorization header 呼叫外部服務——讓驗證清單與判決邏輯住在本工具自己的後端。**注意：http hook 是 fail-open 的**（逾時、非 2xx、連不上 → agent 不會被擋、照樣繼續）。整個產品的核心就是那個 block，所以 **Stop 閘門不能用 http hook 做**——必須用一支 **fail closed 的 command hook 小程式**（上膛狀態下，連不到 Witnos 一律輸出 `{"decision":"block"}`），見下方〈v1 技術選型〉的 `witnos-gate` 與上膛／退膛協議。
- **agent / prompt hook**：用（子）模型去做主觀驗證的判斷。

（Codex 目前 prompt / agent 型 hook 會被解析但跳過，只有 command hook 能用，故主觀驗證的成熟度落後。但 hooks 正在收斂成跨工具標準，因此**驗證清單格式應設計成與 agent 無關的中立格式**，方便日後換基座。）

「活契約」不能只靠 Stop hook 實現——Stop 在 agent 自認完成時才觸發，對單一目標往往就是最後一刻；若中途的編輯全部堆到收尾才生效，那是收尾攔截，不是「趁早攔截」。所以契約要走兩條通道：

- **Stop hook＝守門**：agent 自認完成時，對照契約最新版決定放行或 block（放行條件見〈v1 技術選型〉的中心狀態機）。
- **高頻 hook（PostToolUse）＝送信**：每次工具呼叫後做一次**純本地**的版本比對——契約版本沒變，零成本靜默放行；變了才向核心取回**差異的那幾條**、注入對話。

兩條通道共用同一個 store、同一支 `witnos-gate`（以子命令區分），但故障方向相反：**送信 fail open**（取不到差異就靜默跳過——反正最終結果有守門攔著）、**守門 fail closed**。注入與 block 的內容永遠是 delta、不是整份清單——幾百條每輪整份灌回 context，agent 會重新翻案已通過的項目，token 也燒不完。

### 證據的範圍（第一版做哪些）
攤給使用者看的證據，有兩種可能的強度：

- **（一）Agent curate 的證據** ——Agent 主動報告「我驗了什麼、結論、依據」。使用者看到的是 Agent 選擇要給他看的那些。
- **（二）Agent 不能篩選的原始軌跡** ——Agent 實際做了什麼的被動紀錄：完整 diff、跑了哪些指令、輸出了什麼。工具被動記錄，非 Agent 挑選。

**第一版只做（一），且包含協作編輯**——這才是核心迴圈。（二）**降為條件性的下一步**，刻意先不做，理由有二：

1. 把（一）做出來，才會第一次拿到真實資料回答「Agent 自己 curate 出來的，到底夠不夠一個工程師施展判斷」——這個問題現在只能用猜的。
2.（二）若不經篩選地把三小時任務的全部軌跡傾倒給人，會變成「看不完 → 不會看 → 等於沒做」，反而違反原則 4（分流）。所以（二）真要做，形態也不能是「傾倒原始軌跡」，而得是「被篩成兩三條、人掃一眼就能判斷」。

**（二）的觸發條件：**（一）被驗證為不足、**且能看出不足在哪**。那個「不足在哪」會直接告訴你（二）該用什麼規則去篩，而不是現在憑空設計一個篩選器。

**（一）要自帶「發現自己不足」的感測器。** agent-curated 的證據若不帶出處，「（一）不足」這件事永遠不可觀測，上面的觸發條件形同虛設。所以每條證據必附**出處指標**（檔案路徑、跑過的指令、URL），UI 一鍵打開原物比對；並記錄「人點開原物之後，改了裁決或加了新條目」的事件——這份 log 累積起來，就是（二）該用什麼規則篩的需求規格，也是原則 4 分流維度的原始資料。證據另蓋**擷取當下的 workspace 指紋**（commit／dirty hash），代碼之後又動過就標「證據已過期」，免得事後裁判對著過時的截圖點頭。

### v1 技術選型（已定）

**形態已定案：跨平台桌面應用程式，不是網頁。** 以下是 v1 的具體形狀，每一條都是對著本專案的價值挑的（薄層、容易被 fork、fail-closed 的閘門、與 agent 無關）。目前仍未動工——這些是實作必須遵守的約束。

- **外殼：Tauri 2**（Rust 原生核心 + 作業系統內建 webview），跨平台（macOS + Windows + Linux），可安裝的原生 app、不是瀏覽器分頁。排除的選項：全原生 Swift（只剩 macOS、失去 TS 生態重用）；Electron／Node sidecar（前者整包扛一個 JS runtime，後者踩到 Tauri 已拒修的 `externalBin` 公證 bug）。

- **佈局：一個 Cargo workspace、兩個 Rust bin、一個 TS 前端。**
  1. **GUI 核心**（Tauri app 本體）：啟動時 `tokio::spawn` 一個 **axum** server，綁在 `127.0.0.1` 的**臨時 port**，把 `{port, token}` 寫進 `~/.witnos/endpoint.json`（權限 `0600`）。提供 `POST /gate`（外加閘門所需的 CRUD）。
  2. **`witnos-gate`**：同一個 workspace 裡**另一支不依賴 `tauri` crate 的無 GUI bin**（因此不連結任何 webview runtime——幾百 KB、毫秒級啟動、headless／CI 也能跑），以子命令同時擔任 **Stop（守門）與 PostToolUse（送信）** 兩個 command hook（見〈綁定機制〉）。守門流程：從 stdin 讀 hook JSON → 讀上膛標記（見下方上膛／退膛協議）與 `endpoint.json` → 帶 bearer token POST 給核心 → **上膛狀態下 fail closed**：碰到任何錯誤（連線被拒、逾時、非 2xx、回應格式不對、endpoint 檔不存在）一律輸出 `{"decision":"block", ...}` 後退出；**沒有上膛標記則放行**（這個專案根本沒在被盯）。重點就在把承重路徑維持在單一語言、單一 repo。

- **前端：webview 裡的 TS SPA（React 或 Svelte）。** 參考專案（nexu-io/open-design、OpenCoworkAI/open-codesign）真正搬得動的是 **live 面板的 UI**，不是它們的 Node daemon；daemon 那一半改用 Rust 重寫——是一次**有邊界的翻譯**（一條 gate 路由 + 一個 JSON store + 一次 PATH 掃描），不是重新設計。這是選擇桌面原生要誠實付的代價。

- **儲存：每個目標一個 serde-JSON 檔，包在 `RwLock` 後面**（最薄、最好 fork、符合 local-first 單人定位）。等真的出現「webview 端編輯」與「閘門端讀取」的並發爭用再換 `rusqlite`。GUI 核心與閘門打的是**同一個 in-process store**——人改到的就是閘門每一輪讀到的，「活契約」不需要任何跨行程同步。

- **中心狀態機：閘門的放行 ≠ 項目的通過。** 主觀項要人點頭才算過，但 agent 永不等人——所以 Stop 的放行條件是「客觀項全過 ∧ 主觀項都已攤出詮釋＋證據 ∧ 已對齊契約最新版」，**不是**「全部通過」；「agent 已收工、主觀項待裁決」是目標的正常結束態。契約帶單調遞增的版本號（同時鏡射進上膛標記檔，讓送信通道的「沒變化」判斷不必碰網路）；每條證據、每次 reconcile 都蓋「對齊到第 N 版」的章——事後裁判的信任基礎，是能證明「agent 看過我那條修改」。目標收攤後清單仍可查看，但 UI 要明說「不再有 agent 讀這裡；要變更就重新下目標」（原則 5 的邊界）。

- **主觀項的判斷 = Claude Code 自己的 prompt／agent hook**（裝在 `settings.json`），由 Claude Code 拿**使用者自己的憑證**去跑。v1 的 Witnos 行程內**完全不必接 LLM**——「Rust 沒有 Anthropic Agent SDK」因此不構成問題。Rust 核心只負責儲存 agent 的理解與證據，並主動標記重新詮釋（原則 6）。

- **`witnos init`**：裝進**專案層級**的 `.claude/settings.json`（不是使用者全域——見下面的上膛／退膛協議），共四樣：Stop 與 PostToolUse 兩個 command hook（都指向 app 內打包的 `witnos` bin，絕對路徑）、主觀判斷用的 prompt hook，外加一小段**契約書寫規範**的 prompt（每條＝主張＋怎麼驗＋附什麼證據；主觀條必附詮釋；初版契約攤完後，agent 須再做一輪 **blindspot pass**——提出「使用者可能沒想到要驗」的候選項，預設主觀、待人裁決）——hook 只能逼 agent 停下，好契約要靠 prompt 端寫出來。**App 的「監看專案（自動）」會代跑 `witnos init`**（shell out 到 bundle 內的 bin，實作只有一份；bin 內的 `current_exe()` 就是正確的 hook 絕對路徑），再把目錄記進 registry 並上膛；「資料夾要先受 Claude Code 信任」的前提以 UI 提示浮出。

- **契約 schema 與 Agent 寫入路徑（2026-07-26 定，詳見 `docs/schema-v1.md`）：** Goal／Item／Evidence／Event 的欄位、放行條件的形式化、origin 儀表（強版假設讀數）都定在該檔。Agent 對 store 的讀寫走**同一支 headless bin 的子命令**（用 Bash 呼叫）——能跑 shell 指令是所有 coding agent 的最大公約數，endpoint／token 封裝在 bin 內、prompt 端不碰憑證。bin 命名同時收斂：**單一 headless bin 名為 `witnos`**（上文 `witnos-gate` 的角色成為它的 `hook` 子命令家族；不依賴 `tauri` crate 的約束不變）。

- **發佈：** 單一簽章 `.app`／`.exe`，跑在作業系統 webview 上（十幾二十 MB 的量級，不是 Electron 的重量）。自用 dogfood 階段直接跑**未簽章／ad-hoc 簽章**——macOS 公證與 Windows 簽章延後到要裝到別人機器時再說，不擋 v1 驗證。

- **fail-closed 只在「上膛」時生效（上膛／退膛協議）：** 「連不上一律 block」若無條件成立，哪天沒開 app、在無關的專案跑 Claude Code，每個 session 都會卡死在 Stop。所以：被監看的專案帶一個**上膛標記檔（armed marker）**，優雅停止時移除；gate 只在「有標記且連不上」時 block。App 崩潰會留下標記 → 正確卡住；沒在用 Witnos 的專案 → 永不誤傷。刻意卡住仍是已知的 UX 代價，必須讓人看得見：app 要有「正在盯 N 個目標」指示，且 **block 的 reason 字串本身就是逃生門文件**（「Witnos unreachable——開啟 app，或執行 `witnos disarm`」）——使用者卡住的當下，眼睛正好就在 transcript 上。
  - **marker v2（2026-07-29，auto 模式）：** `{v: 2, auto, default_goal?, sessions: {session_id → {goal_id, contract_version, agent_synced_version}}}`——整檔是「(registry 有沒有這個目錄, store 裡該目錄的 goals)」的**純推導**（只有 watching 的 goal 進得來；session 自己的 auto goal 佔自己的槽；最新的 watching 手動 goal 當 `default_goal`），tmp+rename 原子寫入；舊版單 goal 形狀仍可讀。**一個 session 一個目標、各自被自己的契約 gate**——「證明 agent 看過我那條修改」的信任基礎不允許 A session 被 B 的契約攔。auto 專案裡未綁定的 session 停下來時，gate 帶 `(project_dir, session_id)` 問 core：goal 在且 watching → 正常判定；人明確 opt-out → 放行（fail-closed 防的是靜默失效，不是人的決定，原則 5）；真的沒 goal → block（等於「core 從頭到尾都不在」的靜默失效窗口，正是 fail-closed 要堵的）。**範圍收窄（2026-07-29）：有目標才會被攔。** 有 marker 條目的 session 不管在哪裡啟動都照原樣判定；沒有目標的 session 只有在帶著 `WITNOS_TERMINAL` 記號時才會被卡住——否則一律放行，連 core 不在或 marker 撕裂也放行，因為卡住一個沒有契約的 session 保護不了任何東西，卻會卡在使用者真正在工作的那個終端機裡。**auto 專案零 goal 也保留 marker、照樣卡自己終端機裡的 session**。auto 模式的 registry 在 `~/.witnos/projects.json`，人專屬面（IPC，不上 HTTP）；`witnos disarm` 移除 marker 但 registry 還在、app 重啟會重新上膛（訊息會明說：要停用去 app 移除專案）。
  - **agent 子命令全部吃 `--goal <id>`**（同專案多個 active goal 時必帶）：Bash 呼叫不帶 session 身份，goal 身份因此走 in-context——協議注入、每個 delta、每個 block reason 都印著它；不帶旗標時只在 marker 能解析出唯一 goal 時才通。

- **何時重新考慮：** 出現第一個真正「TS 形狀」的需求（roadmap 第 4 步——接 Codex 且想**原封不動**搬 open-design 的 adapter；或第 5 步要做原始軌跡的篩選呈現）才值得抽一個**由 Rust 核心監督的 TS sidecar** 出來。v1 不為此預付成本。

- **實作備忘：**
  - 「目標」與 Claude Code session 的綁定：用 UserPromptSubmit hook 建立／續接 goal（hook 拿得到 `session_id` 與 `prompt`——後者 2026-07-29 同版實測補證，原始樣本見 spike 報告）。auto 模式下這支 hook get-or-create 該 session 的目標（標題＝首個 prompt 壓空白截 80 字；失敗**不記** instructed → 下個 prompt 重試，短暫斷線自癒；人 opt-out 過的目標原樣回傳、永不重新 watch）。`/clear`／resume 產生新 session id → 新目標，是所選語意不是 bug。
  - **SessionEnd hook（第四個裝進 settings 的 hook；2.1.220 實測存在，輸入含 `session_id`／`cwd`／`reason`）**：session 結束時自己名下的 goal 仍在 `running` → 入帳 `turn_ended_unmet`，側欄不再出現「沒有 agent 會回來完成」的殭屍 running 目標。純記帳、fail open；只認 marker `sessions` 的精確條目（共用的 default goal 不因單一 session 結束被翻）；同 session 續跑再觸發守門 → 自動復活成 `running`。
  - Stop hook 輸入的 `stop_hook_active` 旗標要處理，把「fail-closed 的刻意 stall」與「無限 block 迴圈」在語意上分開。
  - 長任務會經歷 context 壓縮：reconcile 時讓 agent 重讀 store 裡自己上次的詮釋，避免壓縮後重新發明一套理解。
  - 已對過官方 hooks 文件，並以 Claude Code 2.1.220 實測（2026-07-26；方法、原始記錄與可重跑的 harness 見 `spike/hooks-2026-07-26/`）：
    - **送信兩條通道實測都通**：PostToolUse 現已支援 `hookSpecificOutput.additionalContext`（與 2026-07-03 的舊記載相反；注入位置在 tool result 旁），**exit code 2＋stderr** 也有效（「Shows stderr to Claude」；工具已跑完，不會擋到任何東西）。送信用 additionalContext，stderr 留作 fallback。matcher 用 `"*"`；command hook 預設逾時 600 秒，可用 `timeout` 欄位覆寫。（舊記載的 `continueOnBlock` 已不在文件中，不要依賴。）
    - **Stop 的 `{"decision":"block","reason":…}` 實測有效**，且 reason 字串能實際引導 agent 的下一步。輸入中的 `stop_hook_active` 文件已不載但實際仍在（首擋 false、其後 true）；輸入另有 `last_assistant_message`、`transcript_path`；UserPromptSubmit 輸入也有 `session_id`——目標↔session 綁定可行。
    - **Stop 連續 block 上限實測＝8**：第 9 次連續 block 的 reason 會進 transcript，但回合就此結束（headless 下 stdout 為空、無警告）。上膛卡住因此不是無限的——目標要以「回合已結束、未達放行條件」入帳，不能假設 block 能永遠拖住 agent。
    - **fail open 實測證實**：Stop 與 PostToolUse hook 崩潰（exit 1）都被無聲放行；hook 執行器對「非 0 非 2 退出、逾時、hook 自身崩潰」一律繼續——所以 fail closed 必須由 `witnos-gate` 自己保證：內部逾時遠短於 hook 逾時，panic handler 裡也要輸出 block JSON。
    - http hook 的 fail open 有文件背書：「HTTP hooks can't signal a blocking error through status codes alone」——只有成功的 2xx＋正確 JSON 才能 block；連不上、逾時、非 2xx 都等於放行。
    - **信任前提**：未受信任的資料夾會忽略專案 settings 的 `permissions.allow`（2.1.220 headless 實測該情況下 hooks 仍會執行，但別依賴）——`witnos init` 的流程要把「資料夾必須先受信任」算進去。
  - hook API 仍在快速演化（事件已達 30 種，另有 PostToolUseFailure／StopFailure 等新事件可留意）；prompt／agent 型 hook 這輪未測，v1 動工前用同一套 harness 補測、重驗上述欄位。

---

## 核心假設（最重要的一節）

整個專案的地基是這一條假設：

> **給人看「證據」，會比給人看「文字清單」，更能讓人抓到自己沒講出口的期待。**

如果這條是假的，本工具就只是個比較好看的 checklist。

**決定：直接做。** 作為目標使用者本人（軟體工程師），判斷這條夠可信，因此選擇用「做一個最小版本、自己實際用」來驗證，而不是先跑一輪正式對照測試。對一個 local-first、單人開源的工具，「做出來、看自己會不會一直用它」本身就是便宜又誠實的驗證。

**但要清楚自己押的是哪個版本的假設：**

- **弱版（幾乎一定成立）：** 給人看更多證據，有幫助。
- **強版（產品真正押的）：** 證據能讓人**被觸發、想起自己當初沒講出口的期待**——也就是「抓缺漏」，而不只是確認已知。

弱版成立不代表強版成立。而且身為目標使用者本人，反而不容易從內部檢查強版——因為自己的認知會自動填補缺漏。所以做最小版本、實際使用時，要盯著看的正是強版：有沒有真的**因為看到證據而抓到事先沒寫進清單的東西**，而不只是覺得「資訊很齊全」。

**所以 dogfood 要帶儀表，不靠感覺。** 每次新增／修改一條驗證，store 順手記下它的出身：(a) 開跑前寫的、(b) 看著某條證據時加的（記下是哪一條證據）、(c) 執行中自發想到的。(b) 的計數就是強版假設的直接讀數；萬一要退回下面的對照實驗，兩邊還能用同一套度量。成本近乎零——編輯事件本來就進 store。

**便宜的回頭驗證法（萬一做出來發現沒共鳴）：** Wizard-of-Oz——挑一個真實任務用現成 agent 跑一遍、手動整理出它的證據、找幾名工程師分兩組（一組只看文字清單、一組看證據），比對後者是否更容易抓到沒人事先寫進清單的缺漏。Token 成本可忽略：唯一花 token 的是「跑一次 agent」那一步，其餘全是人工。

---

## 目標使用者

「認知已經足夠、但平常被擋在 agent 黑盒外面」的人——典型是軟體工程師。

**本工具不負責讓使用者變聰明，它負責讓既有的判斷力有地方施展。** 它放大使用者本來就有的認知，不替使用者生出他沒有的認知。對一個認知裡沒有「Apple 只用黑白灰」的人，工具攤再多證據也沒用，因為他不知道自己在看什麼——這種人不是它的使用者。工具的價值上限，就是使用者本人的認知上限。

**領域中立。** 被驗的是美感、是主觀品味、還是技術判斷（抽象對不對、邊界情況、codebase 慣例、安全性），對工具來說是同一個動作——逼 Agent 攤出證據、讓那個領域的權威者去戳。工具本身**什麼領域知識都不懂，它只懂流程**；正因為它不懂內容，所以任何領域都能用。「該攤什麼證據」由 Agent 負責產生（見〈技術路線〉的證據範圍），工具不內建任何領域的證據擷取器。

---

## 範圍與非目標（目前）

- **本地優先、單人使用、開源。** 不做雲端、不蒐集跨使用者資料。
- **不追求護城河。** 開源專案不靠不可複製性活；設計乾淨到能被輕易理解與 fork，是優點。
- **不取代 agent harness。** 只做「驗證透明化與協作」這一層。

### 可能的未來方向（明確排除於當前範圍）
「從跨使用者的去識別化資料學會『這類任務大家通常漏掉什麼』、主動幫使用者補上沒想到的驗證項」——這是個強大的方向，但它與 local-first / 資料不外流的定位直接衝突，且需要使用者同意交出資料。**目前刻意不做**，僅記錄於此。

---

## 粗略 Roadmap

1. **做最小可行原型——這就是驗證手段。** 於 Claude Code hooks 上：Stop（守門）＋PostToolUse（送信）這對 hook + 讀取「活契約」的驗證核心 + 一個可在執行中編輯的最簡介面。先做（一），含協作編輯，採事後裁判。自己實際使用時帶著儀表（見〈核心假設〉），盯著看強版假設成立與否。
2. 把「Agent 交出的證據」呈現好（Agent 負責產生證據——例如它自己附的截圖、它偵測到的色票、它量的數字；工具只負責呈現與標記，不內建任何領域的擷取器）。
3. 加入分流機制（用什麼維度判斷「需要人判斷」，看真實資料再定）與逐目標的監控（可自由加入／退出）；並實作原則 6 的「主動標記 Agent 對主觀項的新理解」。
4. 把驗證清單格式抽成 agent-agnostic schema，嘗試接第二個基座。
5.（條件性）若（一）被驗證為不足且看得出不足在哪，才設計原始軌跡層（二）的篩選規則。

---

## 相關概念與先行者

- **Loop engineering** — 本專案問題意識的來源脈絡。
- **Claude Code hooks**（尤其 Stop / http / agent hook）— 預定的綁定機制。
- **Open Design**（nexu-io/open-design）— BYOK、自帶 agent 的整合模型參考（〈技術路線〉的 daemon + adapter 做法借鑒於此）。
- **open-codesign**（OpenCoworkAI）— 其「live agent 面板：todos + tool calls + 可中斷生成」是「在使用者眼皮底下執行」這個體驗最接近的現成實作，值得拆解參考。
- **「A Field Guide to Fable: Finding Your Unknowns」**（Thariq Shihipar，Anthropic，2026-07，x.com/trq212/article/2073100352921215386）— 從 prompt 端獨立寫出同一個問題模型：品質瓶頸在釐清自己的 unknowns，其中 **unknown knowns**（知道但沒說出口、只有在結果面前才顯形）正是本專案核心假設要收割的東西——證據就是它的顯影劑。他的五個手動技巧（blindspot pass、brainstorming、interviews、references、implementation plan）集中在事前／事後；Witnos 等於把這套手動紀律機制化成掛在執行迴圈上的層。其 post-implementation 的「quiz／報告」（確保人真的理解了才 merge）對原則 6「裁判不淪為橡皮圖章」是可參考的遠期想法。

---

## 授權

開源（待定；建議 Apache-2.0 或 MIT，依你對專利條款的需求決定）。
