//! The SessionEnd hook: bookkeeping, deliberately NOT a gate. When a session
//! ends while its goal is still running (mid-run /clear, closed terminal),
//! tell the core so the goal is accounted as "turn ended, release condition
//! unmet" instead of sitting in the UI as a zombie "running" goal nothing
//! will come back to.
//!
//! Fails OPEN everywhere: worst case the status stays Running — exactly the
//! pre-hook behavior, and the human can still close the goal by hand.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::paths::{self, Resolution};

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

    let (_root, marker_path) = paths::find_marker(&cwd)?;
    let marker = paths::read_marker(&marker_path)?;
    // Only a session with its own goal has a turn to account. The default
    // goal is shared across sessions — one session ending says nothing
    // about it, so only exact session entries qualify.
    let Resolution::Entry(entry) = marker.resolve(Some(&session)) else {
        return Some(());
    };
    if !marker.sessions.contains_key(&session) {
        return Some(());
    }

    let ep = paths::read_endpoint().ok()?;
    let _ = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(600))
        .timeout(Duration::from_secs(2))
        .build()
        .post(&format!(
            "http://127.0.0.1:{}/goals/{}/turn-ended",
            ep.port, entry.goal_id
        ))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .send_json(json!({"session_id": session}));
    Some(())
}
