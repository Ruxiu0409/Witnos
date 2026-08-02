<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="126" />
</p>

<h1 align="center">Witnos</h1>

<p align="center"><strong>コーディングエージェントに、自分の仕事を自己採点させない。</strong></p>
<p align="center">Claude が何を検証するかを見える化。実行中に変更。証拠が合わなければ停止させません。</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#ライセンス"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/status-developer%20preview-orange?style=flat-square" alt="Developer Preview" />
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-Hant.md">繁體中文</a> · <strong>日本語</strong></p>

<p align="center">
  <img src=".github/assets/workflow.svg" alt="証拠つきの完了条件を定義し、Claude の実行中に編集し、最後に Stop gate が最新版を確認する流れ。" width="100%" />
  <sub>Claude Code はそのままエージェントとして動き、Witnos は検証を見えるようにして完了を制御します。</sub>
</p>

## ひと目でわかる機能

| **証拠に出所を添付**<br>ファイル、URL、コマンド | **実行中に編集**<br>変更した条件だけを送信 | **Stop gate**<br>最新版を確認 |
|:---|:---|:---|
| **session ごとに一つの目標**<br>別の作業と混ざらない | **ターミナルを維持**<br>アプリ再起動後も会話が残る | **ローカルのみ／BYOK**<br>クラウドも token proxy もなし |

## ソースからインストール

**必要なもの：** macOS · Claude Code · [rustup で管理した Rust](https://rustup.rs) · Node.js 20.19.x または ≥22.12

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

<details>
<summary><strong>インストーラーの処理</strong></summary>

アプリと CLI をビルドし、<code>/Applications/Witnos.app</code> にインストールし、同梱 CLI を検証してから Witnos を開きます。開かずに終えるには <code>--no-open</code> を付けてください。
</details>

## 30 秒で始める

| **1 · 監視** | **2 · 承認** | **3 · 起動** | **4 · Prompt** |
|:---|:---|:---|:---|
| プロジェクトを選ぶ | フォルダを信頼し <code>/hooks</code> を承認 | Witnos 内で <code>claude</code> を起動 | 最初の prompt が目標になる |

> [!IMPORTANT]
> Stop gate が有効になるのは監視対象のプロジェクトだけです。ほかのターミナルで起動した Claude には触れません。クラッシュ後に止まった場合は Witnos を開き直すか、プロジェクトルートで <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code> を実行してください。

<details>
<summary><strong>互換性、対象範囲、復旧</strong></summary>

| | 現在 |
|---|---|
| 利用可能 | macOS + Claude Code |
| アプリ UI | English + 繁體中文 |
| 配布 | ソースビルド、未署名／未公証 |
| 未対応 | Linux テスト、Windows の永続ターミナル、主観判断用 prompt hook |

完全に解除するには **stop watching** を選びます。自動監視中のプロジェクトで <code>disarm</code> を実行しても一時的な解除にすぎず、Witnos の再起動で再び有効になります。監視していない間、インストール済み hooks は何もしません。
</details>

<details>
<summary><strong>開発とコントリビュート</strong></summary>

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

Issue と PR を歓迎します。大きな変更は先に issue を開き、[設計制約](docs/design.ja.md#設計原則これがこのプロジェクトの中身でありui-の話ではない)を読んでください。開発コマンドは [CLAUDE.md](CLAUDE.md)、背景は[設計ノート](docs/README.md)にあります。
</details>

## ライセンス

Copyright © 2026 CHENG YEH TSAI · [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)
