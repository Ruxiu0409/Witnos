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

use crate::paths;

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
    let marker: paths::Marker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).ok()?).ok()?;

    let delivered_path = root.join(paths::DELIVERED_REL);
    let mut delivered: HashMap<String, u64> = std::fs::read_to_string(&delivered_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let since = delivered.get(&session).copied().unwrap_or(0);
    if since == marker.contract_version {
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
            ep.port, marker.goal_id, since
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
        "[witnos] The verification contract moved to v{version} (you were synced to v{since}). Changed items:\n{summary}\n\
         Address the delta, update interpretations/evidence through the `witnos` CLI, then run `witnos reconcile --to {version}`."
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
    std::fs::write(
        &delivered_path,
        serde_json::to_string_pretty(&delivered).ok()?,
    )
    .ok()?;
    Some(())
}
