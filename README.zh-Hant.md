<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="126" />
</p>

<h1 align="center">Witnos</h1>

<p align="center"><strong>別再讓 coding agent 自己訂標準、自己宣布過關。</strong></p>
<p align="center">看清楚 Claude 準備怎麼驗。執行中直接改。證據對不上，就不讓它收工。</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#授權"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/status-developer%20preview-orange?style=flat-square" alt="Developer Preview" />
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center"><a href="README.md">English</a> · <strong>繁體中文</strong> · <a href="README.ja.md">日本語</a></p>

<p align="center">
  <img src=".github/assets/workflow.svg" alt="先列出附證據的完成條件，Claude 執行中仍可修改，最後由 Stop gate 對照最新版。" width="100%" />
  <sub>Claude Code 還是原本的 agent；Witnos 只讓驗證過程看得見，而且真的會攔住它。</sub>
</p>

## 一眼看懂

| **證據附出處**<br>檔案、URL、指令 | **執行中修改**<br>只送有變動的條件 | **Stop gate**<br>檢查最新版本 |
|:---|:---|:---|
| **每個 session 一個目標**<br>不同工作不會混在一起 | **終端機持續存在**<br>重開 App 對話還在 | **只在本機／BYOK**<br>沒有雲端或 token proxy |

## 從 source 安裝

**需要：** macOS · Claude Code · [rustup 管理的 Rust](https://rustup.rs) · Node.js 20.19.x 或 ≥22.12

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

<details>
<summary><strong>安裝程式會做什麼</strong></summary>

Build App 與 CLI、安裝到 <code>/Applications/Witnos.app</code>、驗證內附 CLI，然後打開 Witnos。加上 <code>--no-open</code> 可以只安裝、不開啟。
</details>

## 30 秒開始使用

| **1 · 監看** | **2 · 核准** | **3 · 啟動** | **4 · 下 prompt** |
|:---|:---|:---|:---|
| 選一個專案 | 信任資料夾；核准 <code>/hooks</code> | 在 Witnos 裡執行 <code>claude</code> | 第一個 prompt 自動成為目標 |

> [!IMPORTANT]
> 只有受監看的專案會啟用 Stop gate；在其他終端機啟動的 Claude 完全不受影響。App 崩潰後卡住？重新打開 Witnos，或在專案根目錄執行 <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code>。

<details>
<summary><strong>相容性、作用範圍與恢復方式</strong></summary>

| | 目前狀態 |
|---|---|
| 可使用 | macOS + Claude Code |
| App 語言 | English + 繁體中文 |
| 發佈方式 | 從 source build；尚未簽章／公證 |
| 尚未完成 | Linux 測試、Windows 持久終端機、主觀判斷 prompt hook |

按 **停止監看** 才是永久退出。自動監看的專案執行 <code>disarm</code> 只是暫時解除；重開 Witnos 後會再次啟用。沒有監看時，已安裝的 hooks 不會做事。
</details>

<details>
<summary><strong>開發與參與</strong></summary>

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

歡迎 issue 與 PR。大改動請先開 issue，並讀過[設計硬約束](docs/design.zh-Hant.md#設計原則這些是這個專案的真正內容不是-ui)。更多開發指令在 [CLAUDE.md](CLAUDE.md)，完整推論在[設計筆記](docs/README.md)。
</details>

## 授權

Copyright © 2026 CHENG YEH TSAI · [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)
