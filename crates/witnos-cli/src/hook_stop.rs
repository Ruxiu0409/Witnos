//! The Stop gate. The whole product is this block, so it FAILS CLOSED while
//! armed: on ANY error (no endpoint file, connection refused, timeout,
//! non-2xx, malformed response, even a panic in this binary) it prints
//! `{"decision":"block", ...}`. With no armed marker it allows — that
//! project simply isn't being watched.
//!
//! One session-level narrowing: a session is gated when it HAS a goal. When
//! it has none, only sessions launched from Witnos's own terminal get the
//! fail-closed stall — everything else is out of scope and released. Stalling
//! a session that owns no contract protects nothing; it is pure collateral
//! damage on whatever terminal the user happens to be working in.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::paths::{self, Resolution};

static ARMED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Deserialize)]
struct HookInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    stop_hook_active: Option<bool>,
}

pub fn run() -> ExitCode {
    // The hook runner treats a crashed hook as "continue" (fail open), so
    // fail-closed must be guaranteed here: even a panic prints the block.
    std::panic::set_hook(Box::new(|_| {
        if ARMED.load(Ordering::SeqCst) {
            println!("{}", block_json("witnos gate crashed while armed"));
        }
        std::process::exit(0);
    }));

    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let input: HookInput = serde_json::from_str(&raw).unwrap_or_default();

    let cwd = input
        .cwd
        .clone()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let Some((root, marker_path)) = paths::find_marker(&cwd) else {
        // Not watched: allow silently. Projects not using Witnos are never harmed.
        return ExitCode::SUCCESS;
    };
    ARMED.store(true, Ordering::SeqCst);

    // Out-of-scope sessions own no goal, so no branch below can protect
    // anything by stalling them. Sessions WITH a goal are gated regardless of
    // where they were started (see the module note).
    let in_scope = paths::in_witnos_terminal();

    // The marker's presence arms the gate even if its content is unreadable
    // (a torn write during an app crash must still stall, not slip through).
    let Some(marker) = paths::read_marker(&marker_path) else {
        if !in_scope {
            return ExitCode::SUCCESS; // can't be this session's goal — nothing to hold
        }
        println!(
            "{}",
            block_json(
                "Witnos is watching this project but its armed marker is unreadable. \
                 Ask the user to open the Witnos app (it rewrites the marker), or to run \
                 `witnos disarm` in the project root to stop watching. This stall is \
                 fail-closed by design; do not try to work around it."
            )
        );
        return ExitCode::SUCCESS;
    };

    // Resolve THIS session's goal; goal identity never crosses sessions.
    let body = match marker.resolve(input.session_id.as_deref()) {
        Resolution::Entry(entry) => json!({
            "goal_id": entry.goal_id,
            "session_id": input.session_id,
            "stop_hook_active": input.stop_hook_active,
        }),
        // Auto project, unbound session: only the core can tell apart
        // "goal exists but the marker is stale" / "human opted this goal
        // out" / "genuinely never bound" — ask it; on error, block. But a
        // session from another terminal never had a goal here to begin with,
        // so it is released without even asking.
        Resolution::NoGoalAuto => {
            if !in_scope {
                return ExitCode::SUCCESS;
            }
            json!({
                "project_dir": root.to_string_lossy(),
                "session_id": input.session_id,
                "stop_hook_active": input.stop_hook_active,
            })
        }
        // A manual marker that names no goal only exists hand-damaged;
        // presence arms, so block — unless this session is out of scope.
        Resolution::NoGoalManual => {
            if !in_scope {
                return ExitCode::SUCCESS;
            }
            println!(
                "{}",
                block_json(
                    "Witnos is watching this project but the armed marker names no goal. \
                     Ask the user to open the Witnos app, or to run `witnos disarm` in the \
                     project root to stop watching. This stall is fail-closed by design."
                )
            );
            return ExitCode::SUCCESS;
        }
    };

    match ask_core(body) {
        Ok(GateAnswer::Release) => ExitCode::SUCCESS,
        Ok(GateAnswer::Block(reason)) => {
            println!("{}", block_json(&reason));
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("{}", block_json(&unreachable_reason(&err)));
            ExitCode::SUCCESS
        }
    }
}

enum GateAnswer {
    Release,
    Block(String),
}

fn ask_core(body: Value) -> Result<GateAnswer, String> {
    let ep = paths::read_endpoint()?;
    // Internal timeouts far below the hook runner's own timeout: a hook
    // timeout would fail OPEN in the runner, ours fails CLOSED here.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(1500))
        .timeout(Duration::from_secs(4))
        .build();
    let resp = agent
        .post(&format!("http://127.0.0.1:{}/gate", ep.port))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .send_json(body)
        .map_err(|e| format!("core unreachable or refused: {e}"))?;

    let body: Value = resp
        .into_json()
        .map_err(|e| format!("malformed response from core: {e}"))?;
    match body.get("decision").and_then(Value::as_str) {
        Some("release") => Ok(GateAnswer::Release),
        Some("block") => Ok(GateAnswer::Block(
            body.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("blocked by witnos (no reason given)")
                .to_string(),
        )),
        _ => Err("core response carries no decision".into()),
    }
}

/// The block reason IS the escape-hatch documentation: when stalled, the
/// user's eyes are already on the transcript.
fn unreachable_reason(err: &str) -> String {
    format!(
        "Witnos is watching this project but its core is unreachable ({err}). \
         Ask the user to open the Witnos app, or to run `witnos disarm` in the project root to stop watching. \
         This stall is fail-closed by design; do not try to work around it."
    )
}

fn block_json(reason: &str) -> String {
    json!({"decision": "block", "reason": reason}).to_string()
}
