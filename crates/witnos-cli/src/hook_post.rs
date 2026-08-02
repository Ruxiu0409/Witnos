//! The delivery channel (PostToolUse). Fails OPEN by design: any problem →
//! exit silently; the gate still catches the final result. The
//! contract-unchanged check is purely local (marker file vs delivered file)
//! so an unchanged contract costs zero network round-trips.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::paths::{self, Resolution};

#[derive(Debug, Default, Deserialize)]
struct HookInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

pub fn run() -> ExitCode {
    let _ = try_deliver();
    ExitCode::SUCCESS
}

fn try_deliver() -> Option<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok()?;
    let input: HookInput = serde_json::from_str(&raw).ok()?;
    let session = input.session_id?;
    let cwd = input
        .cwd
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;

    let (root, marker_path) = paths::find_marker(&cwd)?;
    let marker = paths::read_marker(&marker_path)?;
    // A session without a goal has no contract to deliver — the UPS hook
    // creates one on the next prompt; the gate catches the turn's end.
    let Resolution::Entry(entry) = marker.resolve(Some(&session)) else {
        return Some(());
    };

    let delivered_path = root.join(paths::DELIVERED_REL);
    let mut delivered: HashMap<String, u64> = std::fs::read_to_string(&delivered_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Baseline = the newest version the agent has provably seen: what this
    // channel already injected, or what the agent declared via reconcile.
    let since = delivered
        .get(&session)
        .copied()
        .unwrap_or(0)
        .max(entry.agent_synced_version);
    if since >= entry.contract_version {
        return Some(()); // unchanged: zero-cost silent pass
    }

    let ep = paths::read_endpoint().ok()?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(800))
        .timeout(Duration::from_secs(2))
        .build();
    let body: Value = agent
        .get(&format!(
            "http://127.0.0.1:{}/goals/{}/contract?since={}",
            ep.port, entry.goal_id, since
        ))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .call()
        .ok()?
        .into_json()
        .ok()?;
    let version = body.get("version").and_then(Value::as_u64)?;
    let summary = body.get("summary").and_then(Value::as_str)?;

    // Always a delta, never the full list — re-feeding hundreds of items
    // makes the agent re-litigate passed work and burns tokens.
    let ctx = format!(
        "[witnos] The verification contract moved to v{version} (you were synced to v{since}). The delta \
         (items changed, and items REMOVED — stop working on those):\n{summary}\n\
         Address the delta, update interpretations/evidence through the `witnos` CLI, then run \
         `witnos reconcile --goal {} --to {version}`.",
        entry.goal_id
    );
    println!(
        "{}",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": ctx
            }
        })
    );

    delivered.insert(session, version);
    paths::write_atomic(
        &delivered_path,
        &serde_json::to_string_pretty(&delivered).ok()?,
    );
    Some(())
}
