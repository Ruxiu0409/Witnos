<p align="center">
  <img src=".github/assets/hero.png" alt="Witnos" width="150" />
</p>

<h1 align="center">Witnos</h1>

<p align="center">
  <strong>When your coding agent says "done", whose yardstick did it measure against?<br />
  Witnos puts that yardstick on the table — and lets you change it while the agent is still running.</strong>
</p>

<p align="center">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square" alt="License" /></a>
  <a href="https://github.com/Ruxiu0409/Witnos/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ruxiu0409/Witnos/ci.yml?style=flat-square&label=ci" alt="CI" /></a>
  <img src="https://img.shields.io/badge/status-v1%20WIP-orange?style=flat-square" alt="Status" />
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey?style=flat-square" alt="Platform" />
</p>

<p align="center">
  <strong>English</strong> ·
  <a href="README.zh-Hant.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <a href="#install-and-run">Install</a> ·
  <a href="#in-thirty-seconds">In 30 seconds</a> ·
  <a href="#what-it-actually-does">Features</a> ·
  <a href="#design-principles-this-is-the-project-not-the-ui">Design principles</a> ·
  <a href="#roadmap">Roadmap</a>
</p>

---

## In thirty seconds

Wherever there is room to interpret "how do we know it's done", the agent verifies against its own standard and reports success — and that may not be what you meant. The longer it runs unattended, the more a deviation nobody caught in step one becomes the foundation everything after it is built on.

Witnos is a thin layer over the coding agent you already use (Claude Code, today). It does three things:

1. **Makes the agent lay out its verification.** Every criterion is written as claim + how it's checked + the evidence it judged completion by. Evidence must carry a provenance pointer — a file, a command, a URL — and the UI opens the original in one click.
2. **Lets you change the yardstick mid-run.** Your edit reaches the agent through its own hooks on its next tool call. No interrupting it, no waiting for it to finish.
3. **Intercepts "I'm done".** When the agent tries to stop, a Stop hook compares its work against the current contract and blocks until it lines up.

It is **not** an agent and does not replace yours. You bring your own agent and credentials (BYOK); Witnos only makes the verification step visible and interruptible.

The name comes from **witness** — every action the agent takes is watched under your eyes.

> **This README is also the project's design document.** The first half is for people who want to run it. From [Design principles](#design-principles-this-is-the-project-not-the-ui) on is the full rationale and the bet the whole thing rests on — that part is the actual project.

**Status: v1 in progress (started 2026-07-26).** Landed: the hooks-behaviour spike, the contract schema, and a Rust workspace — `witnos-core` (types / store / release condition), `witnos-server` (the axum core as a lib, embedded by the GUI shell), the headless `witnos` bin (both hooks + the agent-facing subcommands + the `pty-serve` terminal daemon), the Tauri shell and frontend. Auto mode landed 2026-07-29: point the app at a project directory, and every agent session started **from the app's own terminal** gets its own goal from its first prompt. Not built yet: the subjective-judgement prompt hook.

---

## Install and run

**Read this first: macOS only today, and this is a tool its author is still dogfooding.** There are no prebuilt downloads — you build it yourself.

You need [Rust](https://rustup.rs) (via rustup, with `~/.cargo/bin` on PATH) and Node 20+.

```sh
git clone https://github.com/Ruxiu0409/Witnos && cd Witnos
cd ui && npm install && npm run build && cd ..   # frontendDist must exist before the Rust build
cargo build -p witnos-cli                        # terminals run inside this bin's daemon
cargo run -p witnos-app
```

To install it properly as `/Applications/Witnos.app`, run `scripts/install-app.sh` — it builds the frontend and the CLI, replaces any existing copy, and reopens the app.

Once it's running: click **watch a project (auto)** and pick a project folder. Witnos installs its hooks into that project's `.claude/settings.json`. **Claude Code must then trust that folder** (new hooks may need `/hooks` approval). Now start Claude Code **inside the app's own terminal** — your first prompt becomes a goal, and that session's verification contract goes live. Sessions started in any other terminal are out of scope: no goal, no injected protocol, never stalled.

To stop: click **stop watching** on that project. If the app crashed and left an agent stalled at the Stop gate, `witnos disarm` releases it in place (the message tells you the app will re-arm on restart).

### Current limits — all deliberate, none of them bugs awaiting a fix

- **macOS only.** Windows has no terminal daemon (ConPTY offers neither a readable foreground process group nor a filesystem socket), so terminals there die with the app; `scripts/install-app.sh` is POSIX-only. Linux is untested.
- **Unsigned, un-notarized.** Build it and run it yourself; shipping to other machines isn't the point yet.
- **Claude Code only.** Abstracting the format so a second agent can be swapped in is step 4 of the roadmap.
- **Nobody is watching while the app is closed.** A graceful quit disarms (no stray stalls), but a **crash** leaves the armed marker, and a surviving agent will stall at its next Stop against a core that isn't there.
- **The subjective-judgement prompt hook isn't built yet.**

Development commands — tests, lint, the various build loops — are in `CLAUDE.md`.

---

## What it actually does

**📋 A living contract, not an upfront spec.** The checklist stays editable for the whole run. Editing an item bumps the contract version; after the agent's next tool call, the changed items are injected into its conversation — **always a delta, never the whole list**, because re-feeding hundreds of items makes the agent re-litigate work it already finished and burns tokens doing it.

**🔒 "I'm done" gets intercepted, and it fails closed.** When the agent tries to stop, the gate checks: objective items passed, every subjective item carrying an interpretation and fresh evidence, reconciled against the latest version. If Witnos is unreachable, the gate **blocks rather than releases** — the interception *is* the product, so it cannot quietly disappear when the service is down. This applies only to watched projects; everything else is untouched.

**🔍 Evidence must carry its provenance.** Every piece of evidence names a file, a command, or a URL, and the UI opens the original in one click. It also logs "the human opened the original, then changed a criterion" — the signal that keeps human judgement from decaying into a rubber stamp, and the direct readout of this project's core bet.

**⚖️ The agent may never pass a subjective item itself.** Classification defaults to subjective; an item counts as objective only if it carries a machine-executable oracle (a command plus its expected output). Misclassification always errs toward showing the human more.

**🖥 Terminals outlive the app.** The agent runs in the app's own terminal, and those shells live in a separate daemon — quit Witnos, reopen it, and the session is still there with its conversation intact. (Not on Windows; see limits.)

**↩︎ Your correction can reach an agent that already stopped.** After-the-fact judgement is worthless against an agent that has finished, so Witnos types the change into the terminal the agent runs in — after reading Claude Code's own title state, so it stays quiet while the agent is working (the delivery channel carries the change on its next tool call anyway).

**🔑 BYOK.** Your tokens are never touched or proxied. Entirely local-first: no cloud, no data collection of any kind.

---

## Why it exists

Agents run a loop: **reason → act → observe → repeat**. The weak step is *observe*.

The real problem isn't whether a goal is objective or subjective — it's whether **your understanding of "how do we know it's done" can differ from the agent's.** Wherever it can, the agent verifies against its own standard, reports success, and hands you something else.

That line does not coincide with subjective-versus-objective. Subjective goals obviously leave room — "make it feel like an Apple app" means different things to different people. But **apparently objective goals leave room in the verification itself**: "does it build" sounds hard-edged until you ask which environment, which version, and whether the tests have to be green too. What matters isn't the nature of the goal; it's whether both sides agree on how it gets checked.

The gap itself is an old problem — the specification gap, the oracle problem, and Goodhart's law are all relatives. **Long unattended runs make it acute**: with nobody watching, a deviation missed in step one becomes the foundation of step two, compounding silently, so that by the time the agent says "done" what's bent is an entire chain of decisions rather than one file.

**The core claim: wherever "done" can mean two different things, the agent must not be the sole judge of its own completion. The human needs somewhere that yardstick is visible and steerable.**

### The loop

Three roles: **User** (a capable engineer), **Agent** (the AI writing the code), **Tool** (this project, the layer between them).

1. The user gives a goal.
2. The agent lays out "what I'll verify, how, and the result" onto the tool.
3. The agent starts running.
4. The user can add or edit a criterion at any time — **without interrupting the agent**.
5. On each check round the agent reads the **current latest version** of the contract. That is what "living" means: continuously re-read during execution, not a static document read once at the start.
6. When the agent finds its work no longer matches, it **fixes the code first, on its own reading**, then lays back: "I interpreted your criterion as X, here's what I changed, here's the evidence."
7. Next time the user looks, they judge whether the agent got it right.

**That loop is the project. Everything else is periphery.**

**Why judgement happens after the fact.** When the user edits a *subjective* criterion, deciding whether the agent now satisfies it is itself a subjective judgement — which by this project's own rules belongs to the human. But stopping to ask every time would interrupt constantly. So the agent never waits: it acts on its own reading and lays its interpretation back, and the human rules later. The cost of that choice, and what pays for it, is design principle 6.

**Why the value is in being early.** The loop already guarantees eventual convergence on "matches the latest list". The tool's value is not correctness — it is giving a human the chance to catch a deviation *before* it spreads into the foundation of downstream work, at the lowest rework cost available.

---

## Design principles (this is the project, not the UI)

These six are hard constraints. Every implementation decision gets checked against them.

### 1. Evidence over intent

Don't let the agent merely write down what it *intends* to verify. Intent can lie and can't be checked — "I confirmed it matches the HIG" is unfalsifiable. Make it surface **the evidence it judged completion by**: the screenshot it produced, the colour swatches it detected, the contrast numbers it measured.

The reason: people are bad at proactively recalling expectations they never voiced, and good at being triggered by evidence in front of them. When five colours show up in the swatch list, the user's unspoken "it should only be black, white and grey" gets *pulled out of them* — they never had to think of it in advance. The problem turns from "the human must remember" into "evidence triggers them passively."

### 2. For subjective items, the human is the final judge (the Goodhart safeguard)

The tool guides the agent to decompose fuzzy standards into checkable proxy metrics. **But proxy metrics are only communication scaffolding, never the judge.**

- **Objective items**: the agent may self-check and self-pass.
- **Subjective items**: proxy metrics only carry evidence to the human. **The agent may never declare one passed.**

*Revised 2026-07-29.* This used to read "passing requires a human nod", and there really was an approve button. It was removed: it changed nothing about the agent (the release condition already treated "laid" and "approved" alike), and the human's honest default is that the agent's work is presumed right. So subjective items have no pass state at all — the terminal state is "evidence laid, and the human either moved the yardstick or didn't." Agreement is silence; only disagreement needs an action.

*Revised again 2026-08-02.* Two more levers went. **Send-back** meant "the criterion stands, but your evidence doesn't pass" — except editing already reopens the item, bumps the version, stales the evidence and reaches the running agent, so re-saving even the same words says the same thing; and when the agent misread a criterion, writing it clearer beats telling it "again". **Waiving** parked an item in a state nobody checked; a contract accumulating tombstones is exactly the reading load principle 4 exists to cut, so ✕ now deletes, evidence and all. The human has two levers left: **edit the yardstick, or delete the item.**

Never let subjective items auto-pass on proxy metrics. That is the Goodhart trap: the agent satisfies the numbers and produces something where every metric is right and the whole is wrong. This line is held absolutely.

There is a side door to guard. **If the agent decides which items count as objective, mislabelling one silently bypasses the judge.** So the rule is fixed: default subjective; objective requires a machine-executable oracle; a human may explicitly promote an item and take responsibility for it. Classification errors always fall toward showing the human more.

### 3. A living contract, not an upfront spec

Don't depend on the user specifying requirements correctly at the start. Subjective and tacit things are inherently unspecifiable — if they could be spelled out, you wouldn't need the human. The list stays editable and re-compared throughout.

### 4. Triage what surfaces

More detail means more for the human to read, and that is a real conflict. A three-hour task may involve hundreds of verifications; dump them all and the user reads none, which is the same as not building it. The answer is not "hope they're patient" — it's **triage**: let the agent digest the safe items nobody needs to see, and surface only the few that genuinely need human judgement.

(Exactly which dimensions decide "needs a human" is deliberately left open until there is real data. See the roadmap.)

### 5. The unit of control is a single goal; monitoring is opt-in per goal

A project is a series of independently issued goals. Within one goal's execution the agent runs to completion — it **does not stop to wait for anyone**.

So "pay attention when I want to" is not a stop-gate inside a run. It is simply: **the user decides, per goal, whether to watch this one and live-edit its verification.** Watch goals 1–5, ignore 6–20, come back for the last five. Each goal is an independent issuance, so "start watching the next one" is always available and never has to be decided in advance.

### 6. After-the-fact judgement must be actively flagged, or it equals the agent deciding alone

Since the human's ruling happens later, the tool is obliged to make sure they don't miss the moment they should look.

If the agent quietly reinterprets a criterion, fixes the code and lays it back — and that gets buried among a hundred passing items — then the human holds judgement in name only, and the agent has effectively decided alone.

So the agent's **new interpretation of a subjective criterion** must be surfaced, not filed away with the passes. This is the same mechanism as principle 4: a reinterpretation is exactly the kind of thing that should rise to the top.

---

## The core bet

Everything rests on one claim:

> **Showing a human "evidence" lets them catch their own unspoken expectations better than showing them a "text checklist".**

If that is false, this tool is a prettier checklist.

**Decision: build it.** As a member of the target audience, the author judged the claim credible enough to validate by building a minimal version and using it, rather than running a formal study first. For a local-first, single-user, open-source tool, "build it and see whether you keep reaching for it" is cheap and honest validation.

**But be clear which version of the claim is being bet on:**

- **Weak version** (near-certain): more evidence helps.
- **Strong version** (what the product actually bets on): evidence **triggers people into remembering expectations they never articulated** — catching gaps, not just confirming knowns.

The weak version holding says nothing about the strong one. And being the target user makes the strong version *harder* to check from the inside, because your own mind fills the gaps automatically. So dogfooding watches for exactly this: did something get caught **because evidence was seen**, that was never written in the list — as opposed to merely feeling well-informed.

**Which is why dogfooding is instrumented, not vibes-based.** Every contract add or edit records its provenance: (a) written before the run, (b) added while looking at a specific piece of evidence (recording which one), (c) spontaneous mid-run. The count of (b) is the direct readout of the strong version. It costs nothing — edit events land in the store anyway.

**Cheap fallback if the built version doesn't resonate:** Wizard-of-Oz. Run a real task with an off-the-shelf agent, assemble its evidence by hand, split a few engineers into two groups — one sees a text-only list, one sees evidence — and compare whether the latter catches more gaps nobody wrote down in advance.

---

## Who it's for

Someone whose judgement is already adequate but who is locked outside the agent's black box. Typically a software engineer.

**The tool does not make you smarter; it gives judgement you already have somewhere to act.** It amplifies existing cognition, it does not manufacture cognition you lack. Surfacing evidence to someone who has never internalised "Apple only uses black, white and grey" does nothing — they don't know what they're looking at. The value ceiling is the user's own.

**Domain-neutral by construction.** Aesthetics, taste, and technical judgement (correctness, edge cases, codebase conventions, security) are all the same action as far as the tool is concerned: make the agent surface evidence, let the domain authority poke at it. The tool understands *process*, not *content* — and precisely because it knows nothing about your domain, any domain can use it. What evidence to surface is produced by the agent; no domain-specific extractor is built in.

---

## Scope and non-goals

- **Local-first, single-user, open source.** No cloud, no cross-user data collection.
- **No moat sought.** Being clean enough to understand and fork easily is a feature.
- **Not a replacement for your agent harness.** Only the verification-transparency and collaboration layer.

**Explicitly out of scope, recorded only:** learning "what people usually miss for this kind of task" from de-identified cross-user data. Powerful, and in direct conflict with the local-first, nothing-leaves-your-machine positioning.

---

## Roadmap

1. **Build the minimal viable prototype — that is itself the validation method.** On Claude Code hooks: the Stop (gate) + PostToolUse (delivery) pair, a verification core reading the living contract, and a minimal UI to edit it during execution. Agent-curated evidence with collaborative editing and after-the-fact judgement. Dogfood it instrumented, watching the strong version of the bet.
2. Present the evidence the agent surfaces well. The agent produces it; the tool displays and flags, and embeds no domain extractor of its own.
3. Add triage, plus per-goal opt-in monitoring, and implement principle 6's active flagging of reinterpretations.
4. Abstract the contract format into an agent-agnostic schema; try a second base agent.
5. *(Conditional)* Only if agent-curated evidence proves insufficient **and you can see where** — design the filtering rules for a raw-trace layer.

---

## Prior art and related ideas

- **Loop engineering** — the reason → act → observe → repeat framing this project's problem statement builds on.
- **Claude Code hooks** (Stop / http / agent) — the binding mechanism.
- **[open-design](https://github.com/nexu-io/open-design)** — reference for the BYOK integration model: local daemon, UI, per-agent adapters scanning PATH.
- **open-codesign** (OpenCoworkAI) — its live agent panel (todos, tool calls, interruptible generation) is the closest existing implementation of "executing under the user's eyes".
- **"A Field Guide to Fable: Finding Your Unknowns"** (Thariq Shihipar, Anthropic, 2026-07) — independently states the same problem model from the prompt side. Quality bottlenecks on clarifying your own unknowns, and **unknown knowns** — things you know but never articulated, visible only in front of results — are exactly what the core bet harvests. Evidence is their developing agent.

For the full technical decision log — the state machine, the arm/disarm protocol, the terminal daemon's design, and every dated decision behind them — see the [Traditional Chinese README](README.zh-Hant.md), which is the canonical design document.

---

## Repository layout

| Path | What it is |
|---|---|
| `crates/witnos-core` | Domain types, write-time rules, the per-goal JSON store, the gate's release condition. No I/O framework dependency |
| `crates/witnos-server` | The axum core **as a lib** — the Tauri shell embeds it, so what the human edits is the same in-process store the gate reads. `examples/serve.rs` runs it headless |
| `crates/witnos-cli` | The headless `witnos` bin: both hooks (Stop gate fail-closed / PostToolUse delivery fail-open), arm/disarm, the agent-facing subcommands, and the `pty-serve` terminal daemon. **Must never depend on the `tauri` crate** |
| `crates/witnos-app` | The Tauri shell. The human side goes over IPC, the agent side over HTTP — that split is a structural trust boundary |
| `ui/` | The webview frontend (React + TypeScript, no UI framework, icons are inlined Lucide paths) |
| `scripts/install-app.sh` | One-shot bundle and install to `/Applications/Witnos.app` |

Development commands are in `CLAUDE.md`.

---

## Contributing

Still at the dogfooding stage; issues and PRs are welcome, but read the [design principles](#design-principles-this-is-the-project-not-the-ui) first. Those six are hard constraints and every implementation decision gets checked against them — especially principle 2 (the agent may never pass its own subjective work) and the fail-closed gate. Those two are why the tool exists and won't be relaxed for convenience.

Think the direction is right but the details should work differently? This project deliberately seeks no moat — **being clean enough to understand and fork easily is one of its features.** Forking it outright is a perfectly good outcome.

---

## License

Copyright © 2026 CHENG YEH TSAI

**Dual licensed under MIT OR Apache-2.0** — take whichever you prefer. Full texts in [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE). (`LICENSE-APACHE` is the unmodified official text; the appendix at its end is a template for applying the licence to your own work, not a declaration by this project — the copyright holder is the line above and `LICENSE-MIT`.)

This is the Rust ecosystem's default, and it fits: "clean enough to fork" is a stated feature here, so the choice goes to whoever forks it — Apache-2.0 for its patent grant and retaliation clause, MIT for GPLv2 compatibility. Source files carry no licence headers; all four crates declare `license = "MIT OR Apache-2.0"` in their manifests, and repeating it atop every file is noise.

Unless you state otherwise, any contribution you intentionally submit for inclusion shall be dual licensed as above, with no additional terms.
