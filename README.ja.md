<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="136" />
</p>

<h1 align="center">Witnos</h1>

<p align="center">
  <strong>コーディングエージェントに、自分の仕事を自己採点させない。</strong>
</p>

<p align="center">
  Witnos は Claude Code に、生きた完了契約を与えます。<br />
  見える、証拠がある、そして実行中でも編集できます。
</p>

<p align="center">
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <a href="#ライセンス"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/data-local%20only-6f42c1?style=flat-square" alt="Local only" />
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-Hant.md">繁體中文</a> ·
  <strong>日本語</strong>
</p>

---

エージェントが最も危険なのは、派手に失敗したときではありません。もっともらしい基準を選び、それで自己検証し、自信たっぷりに止まるときです。長いタスクでは、最初の小さな誤解が 10 ステップ後にはアーキテクチャになります。

**Witnos は、修正コストがまだ安いうちに、完了基準を両者の目の前へ出します。**

## 仕組み

1. **ものさしを見せる。** Claude は各条件を、主張・確認方法・完了判断に使った証拠として並べます。
2. **実行中に動かす。** Claude が作業している間も条件を編集できます。差分は次の tool call の後に届きます。
3. **「完了」を止める。** Claude が停止しようとすると、Witnos が最新の契約を確認します。証拠が古い、または不足していれば作業へ戻します。

Witnos は別のエージェントではありません。Claude Code、自分の認証情報、今のワークフローはそのままです。Witnos は検証を見えるようにし、途中からでも方向を変えられるようにするだけです。

## インストール

> **Developer Preview** — 現在は macOS + Claude Code のみ。ソースからのビルドが必要で、署名・公証はまだありません。

必要なもの：[rustup で管理した Rust](https://rustup.rs)、Node.js <code>^20.19.0</code> または <code>>=22.12.0</code>、Claude Code。

~~~sh
git clone https://github.com/Ruxiu0409/Witnos.git
cd Witnos
./scripts/install-app.sh
~~~

インストーラーはすべてをビルドし、<code>/Applications/Witnos.app</code> にインストールし、同梱 CLI を検証してからアプリを開きます。開かずに終えるには <code>--no-open</code> を付けてください。

## 30 秒で始める

1. Witnos を開き、**watch a project (auto)** を押します。
2. プロジェクトを選び、Claude Code でフォルダを信頼します。求められた場合は <code>/hooks</code> で hooks を承認します。
3. Witnos 内のターミナルで <code>claude</code> を起動します。
4. 最初の prompt を送ります。それがこの session の目標になり、検証契約が自動で現れます。

あとは、証拠を見て「言い忘れた」と気づいた瞬間に契約を直すだけです。

### 具体例

あなたが **「auth をリファクタして。振る舞いは変えないで」** と頼む。Claude はテストを通して停止しようとする。そこで、キーボード操作が基準に入っていないと気づく。その条件を契約へ追加すると、Witnos が修正を届け、古い証拠を無効にし、Claude は以前の「完了」を使い回せません。

狙いは一つです。**間違った前提が積み上がる前に捕まえる。**

## 得られるもの

- **出所つきの証拠** — 主張を支えるファイル、URL、実行済みコマンドを一緒に保持します。
- **session ごとに一つの契約** — 監視対象の Claude session は、最初の prompt から固有の目標を持ちます。
- **中断しない修正** — 契約全体ではなく、変わった条件だけを送り返します。
- **アプリより長生きするターミナル** — Witnos を閉じて開き直しても、shell と会話は残ります。
- **ローカルが既定** — ローカル JSON と認証つき loopback 通信。クラウド、telemetry、認証情報の代理はありません。

## 影響範囲と退避方法

明示的に監視したプロジェクトだけが armed になります。自動目標は Witnos のターミナル内で起動した Claude session だけが対象です。ほかのターミナルやプロジェクトには触れません。

解除するには **stop watching** を選びます。クラッシュ後にエージェントが Stop gate で止まった場合は Witnos を開き直すか、プロジェクトルートで <code>/Applications/Witnos.app/Contents/Resources/bin/witnos disarm</code> を実行してください。armed でないプロジェクトでは、インストール済み hooks は何もしません。

## 現在の制限

現時点では macOS、Claude Code、ソースビルドのみで、署名もありません。Linux は未検証、Windows には永続ターミナル daemon がなく、主観判断用 prompt hook も未実装です。アプリ UI は英語と繁体字中国語に対応しています。

判断の背景は[設計ノート](docs/README.md)に残しています。

## 開発

~~~sh
npm ci --prefix ui
npm --prefix ui run build
cargo test --workspace --exclude witnos-app
cargo clippy --workspace --exclude witnos-app --all-targets -- -D warnings
~~~

Workspace は、フレームワーク非依存の domain core、Axum server、headless hook CLI、Tauri + React デスクトップアプリに分かれています。開発ループの全体は [CLAUDE.md](CLAUDE.md) を参照してください。

## コントリビュート

Issue と PR を歓迎します。大きな変更は先に issue を開き、[6 つの設計制約](docs/design.ja.md#設計原則これがこのプロジェクトの中身でありui-の話ではない)を読んでください。特に「エージェントは自分の主観的な仕事を自分で通してはならない」という規則は重要です。

## ライセンス

Copyright © 2026 CHENG YEH TSAI

[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE) のデュアルライセンスです。好きなほうを選べます。別途明記しない限り、コントリビューションも同じ条件です。
