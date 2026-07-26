//! The Stop gate. The whole product is this block, so it FAILS CLOSED while
//! armed: on ANY error (no endpoint file, connection refused, timeout,
//! non-2xx, malformed response, even a panic in this binary) it prints
//! `{"decision":"block", ...}`. With no armed marker it allows — that
//! project simply isn't being watched.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::paths;

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

    let Some((_root, marker_path)) = paths::find_marker(&cwd) else {
        // Not watched: allow silently. Projects not using Witnos are never harmed.
        return ExitCode::SUCCESS;
    };
    ARMED.store(true, Ordering::SeqCst);

    // The marker's presence arms the gate even if its content is unreadable
    // (a torn write during an app crash must still stall, not slip through).
    let marker: paths::Marker = std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(paths::Marker {
            goal_id: "unknown".into(),
            contract_version: 0,
        });

    match ask_core(&marker, &input) {
        Ok(GateAnswer::Release) => ExitCode::SUCCESS,
        Ok(GateAnswer::Block(reason)) => {
            println!("{}", block_json(&reason));
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("{}", block_json(&unreachable_reason(&marker.goal_id, &err)));
            ExitCode::SUCCESS
        }
    }
}

enum GateAnswer {
    Release,
    Block(String),
}

fn ask_core(marker: &paths::Marker, input: &HookInput) -> Result<GateAnswer, String> {
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
        .send_json(json!({
            "goal_id": marker.goal_id,
            "contract_version": marker.contract_version,
            "session_id": input.session_id,
            "cwd": input.cwd,
            "stop_hook_active": input.stop_hook_active,
        }))
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
fn unreachable_reason(goal_id: &str, err: &str) -> String {
    format!(
        "Witnos is watching this project (goal {goal_id}) but its core is unreachable ({err}). \
         Ask the user to open the Witnos app, or to run `witnos disarm` in the project root to stop watching. \
         This stall is fail-closed by design; do not try to work around it."
    )
}

fn block_json(reason: &str) -> String {
    json!({"decision": "block", "reason": reason}).to_string()
}
