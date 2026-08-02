<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="136" />
</p>

<h1 align="center">Witnos</h1>

<p align="center">
  <strong>別再讓 coding agent 自己訂標準、自己宣布過關。</strong>
</p>

<p align="center">
  Witnos 把 Claude Code 的完成條件列出來——<br />
  每一條都有證據，Claude 還在跑時你也能直接改。
</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#授權"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <strong>繁體中文</strong> ·
  <a href="README.ja.md">日本語</a>
</p>

---

Agent 最危險的時候，不是它明顯報錯，而是它選了一套看似合理的標準、自我驗證、然後很有自信地收工。一個長任務裡，第一步的小誤解，到了第十步就是整套架構。

**Witnos 讓你直接看到 Claude 的完成標準——趁修正還便宜。**

## 它怎麼運作

1. **把完成條件寫清楚。** Claude 把每一條寫成一個主張、一種驗法，以及它判定完成時用的證據。
2. **執行中直接改。** Claude 還在工作時就能編輯條件；下一次 tool call 後，它會收到差異。
3. **擋下「完成」。** Claude 想停時，Witnos 對照最新條件；證據過期或不完整，就把它送回去繼續做。

Witnos 不是另一個 agent。你照樣使用 Claude Code、自己的憑證與原本的工作流；Witnos 只讓驗證過程看得見，而且你能介入。

## 安裝

> **Developer Preview** — 目前只支援 macOS + Claude Code，需從 source build，尚未簽章或公證。

需要 [rustup 管理的 Rust](https://rustup.rs)、Node.js <code>^20.19.0</code> 或 <code>>=22.12.0</code>，以及 Claude Code。

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

安裝程式會 build 全部元件、安裝 <code>/Applications/Witnos.app</code>、驗證內附 CLI，然後打開 App。加上 <code>--no-open</code> 可以只安裝、不開啟。

## 30 秒開始使用

1. 打開 Witnos，按 **監看專案（自動）**。
2. 選一個專案資料夾；在 Claude Code 信任該資料夾，若有提示則用 <code>/hooks</code> 核准 hooks。
3. 在 Witnos 內建終端機執行 <code>claude</code>。
4. 送出第一個 prompt。它會自動成為這個 session 的目標，驗收清單也會出現。

接下來，只要證據讓你想到一件剛才忘了說的事，直接改完成條件。

### 一個實際例子

你說：**「重構 auth，但行為不能變。」** Claude 跑完測試，準備收工。你看到證據才發現「鍵盤操作不能壞」根本不在它的標準裡。把這條加進清單；Witnos 會送回修正、讓舊證據失效，Claude 不能拿舊的「完成」過關。

這就是全部的重點：**在錯誤假設一路帶歪後面的工作以前抓到它。**

## 你會得到什麼

- **附出處的證據** —— 支撐主張的檔案、URL 與已執行指令會留在一起。
- **每個 session 一份驗收清單** —— 每個受監看的 Claude session，都從第一個 prompt 建立自己的目標。
- **不打斷的修正** —— 只把有變動的條件送回去，不重送整份清單。
- **活得比 App 久的終端機** —— 關掉再打開 Witnos，shell 與對話都還在。
- **預設只在本機** —— 本機 JSON、帶驗證的 loopback 通訊；沒有雲端、telemetry，也不代理憑證。

## 範圍清楚，退路也清楚

只有你明確監看的專案會啟用 Stop gate。自動目標只套用在 Witnos 終端機裡啟動的 Claude session；其他終端機與專案完全不受影響。

按 **停止監看** 就能退出。若 App 崩潰，讓 agent 卡在 Stop gate，請重新打開 Witnos，或在專案根目錄執行 <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code>。沒有啟用監看時，已安裝的 hooks 不會做事。

## 目前限制

現階段只有 macOS、只有 Claude Code、只能從 source build，而且尚未簽章。Linux 未實測，Windows 還沒有持久化終端機 daemon；主觀判斷 prompt hook 也尚未完成。

想看這些取捨背後的完整推論，請讀[設計筆記](docs/README.md)。

## 開發

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

Workspace 分成無框架依賴的 domain core、Axum server、headless hook CLI，以及 Tauri + React 桌面 App。完整開發迴圈見 [CLAUDE.md](CLAUDE.md)。

## 參與

歡迎 issue 與 PR。大改動請先開 issue，並讀過[六條設計硬約束](docs/design.zh-Hant.md#設計原則這些是這個專案的真正內容不是-ui)——尤其是「agent 不得替自己的主觀工作判定通過」。

## 授權

Copyright © 2026 CHENG YEH TSAI

採 [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE) 雙授權，你選一個使用即可。除非另有聲明，貢獻內容也採相同授權。
