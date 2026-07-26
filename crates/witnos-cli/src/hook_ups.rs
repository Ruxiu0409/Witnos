//! The UserPromptSubmit hook: binds the session to the watched goal
//! (best-effort) and injects the contract-authoring protocol ONCE per
//! session. Hooks can only force the agent to stop; good contracts come
//! from the prompt side — this is that prompt.
//!
//! Fails OPEN everywhere: the injection decision is purely local (armed
//! marker + instructed file); the core is only consulted to bind the
//! session and fetch the goal title.

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
    let _ = try_run();
    ExitCode::SUCCESS
}

fn try_run() -> Option<()> {
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

    let instructed_path = root.join(paths::INSTRUCTED_REL);
    let mut instructed: HashMap<String, u64> = std::fs::read_to_string(&instructed_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if instructed.contains_key(&session) {
        return Some(()); // already instructed; deltas travel via PostToolUse
    }

    // Best-effort bind + title fetch; the injection below does not depend on it.
    let mut title = None;
    if let Ok(ep) = paths::read_endpoint() {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(600))
            .timeout(Duration::from_secs(2))
            .build();
        let auth = format!("Bearer {}", ep.token);
        let _ = agent
            .post(&format!(
                "http://127.0.0.1:{}/goals/{}/sessions",
                ep.port, marker.goal_id
            ))
            .set("Authorization", &auth)
            .send_json(json!({"session_id": session, "agent": "claude-code"}));
        if let Ok(resp) = agent
            .get(&format!(
                "http://127.0.0.1:{}/goals/{}",
                ep.port, marker.goal_id
            ))
            .set("Authorization", &auth)
            .call()
        {
            if let Ok(v) = resp.into_json::<Value>() {
                title = v.get("title").and_then(Value::as_str).map(String::from);
            }
        }
    }

    println!(
        "{}",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": protocol_text(&marker, title.as_deref()),
            }
        })
    );

    instructed.insert(session, witnos_core_now());
    let _ = std::fs::write(
        &instructed_path,
        serde_json::to_string_pretty(&instructed).ok()?,
    );
    Some(())
}

fn witnos_core_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The contract-authoring prompt. Injected once per session; kept terse —
/// it rides along with every later turn of the conversation.
fn protocol_text(marker: &paths::Marker, title: Option<&str>) -> String {
    let goal_line = match title {
        Some(t) => format!("goal: \"{t}\""),
        None => format!("goal {}", marker.goal_id),
    };
    format!(
        "[witnos] This project is watched — {goal_line}, contract v{}. Your work here runs under a \
         verification contract that the user can edit WHILE you work. Protocol (all via the `witnos` CLI in Bash):\n\
         1. Before implementing, lay out what you will verify: `witnos item lay` with a JSON array on stdin; \
         each item = {{\"claim\": what must hold, \"check\": how you verify it}}. Items are SUBJECTIVE by default; \
         add \"class\":{{\"kind\":\"objective\",\"oracle\":{{\"command\":\"…\",\"expected\":\"…\"}},\"promoted_by\":\"agent\"}} \
         ONLY when a machine-runnable command truly decides it.\n\
         2. For every subjective item, record how you read it: `witnos item interpret <item-id> <your interpretation>`.\n\
         3. When you verify something, attach the evidence you judged by: `witnos evidence add <item-id>` with JSON on stdin \
         {{\"conclusion\":\"…\",\"basis\":\"…\",\"provenance\":[{{\"kind\":\"file\",\"path\":\"…\"}} or {{\"kind\":\"command\",\"cmd\":\"…\"}} or {{\"kind\":\"url\",\"url\":\"…\"}}]}}. \
         Evidence without provenance is rejected.\n\
         4. Objective items: run the oracle, then `witnos oracle report <item-id> --passed|--failed`.\n\
         5. After laying the initial contract, do ONE blindspot pass: `witnos item lay --blindspot` with checks the user \
         likely didn't think to ask for.\n\
         6. The contract is alive: when it changes you'll see a delta after a tool call — address it, then \
         `witnos reconcile --to <version>`.\n\
         7. Stopping is gated: you are released only when objective items passed, every subjective item carries \
         interpretation + evidence, and you've reconciled to the latest contract version. Human rulings on subjective \
         items happen AFTER you stop — \"awaiting rulings\" is a normal way to finish; never wait for them.",
        marker.contract_version
    )
}
