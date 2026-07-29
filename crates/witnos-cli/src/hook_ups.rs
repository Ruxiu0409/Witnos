//! The UserPromptSubmit hook: in auto mode it CREATES this session's goal
//! from the first prompt (one goal per session, titled from the user's own
//! words), binds the session, and injects the contract-authoring protocol
//! ONCE per session. Hooks can only force the agent to stop; good contracts
//! come from the prompt side — this is that prompt.
//!
//! Fails OPEN everywhere. Crucially, a failed goal-creation does NOT mark
//! the session as instructed — the next prompt retries, healing transient
//! core outages. The Stop gate is the only fail-closed point.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
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
    /// The user's prompt text (verified present on Claude Code 2.1.220 —
    /// spike/hooks-2026-07-26). Absence falls back to a session-based title.
    #[serde(default)]
    prompt: Option<String>,
}

pub fn run() -> ExitCode {
    let _ = try_run();
    ExitCode::SUCCESS
}

fn try_run() -> Option<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok()?;
    let input: HookInput = serde_json::from_str(&raw).ok()?;
    let session = input.session_id.clone()?;
    let cwd = input
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;

    let (root, marker_path) = paths::find_marker(&cwd)?;
    let marker = paths::read_marker(&marker_path)?;

    let instructed_path = root.join(paths::INSTRUCTED_REL);
    let mut instructed: HashMap<String, u64> = std::fs::read_to_string(&instructed_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if instructed.contains_key(&session) {
        return Some(()); // already instructed; deltas travel via PostToolUse
    }

    let injection = match marker.resolve(Some(&session)) {
        Resolution::Entry(entry) => existing_goal(&entry.goal_id, entry.contract_version, &session),
        // Auto mode covers Witnos's own terminal only: a session the user
        // started in some other terminal gets no goal and no protocol, so
        // nothing about their work shows up in the app uninvited. No
        // instructed mark either — if they later relaunch inside Witnos, the
        // first prompt there still binds.
        Resolution::NoGoalAuto if !paths::in_witnos_terminal() => return Some(()),
        Resolution::NoGoalAuto => auto_create_goal(&root, &session, input.prompt.as_deref())?,
        Resolution::NoGoalManual => return Some(()),
    };
    let Some(text) = injection else {
        return Some(()); // deliberate silence (e.g. opted-out goal) — no instructed mark
    };

    println!(
        "{}",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": text,
            }
        })
    );

    instructed.insert(session, witnos_core::now());
    paths::write_atomic(
        &instructed_path,
        &serde_json::to_string_pretty(&instructed).ok()?,
    );
    Some(())
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(600))
        .timeout(Duration::from_secs(2))
        .build()
}

/// Manual / already-bound path: best-effort bind + title fetch; the
/// injection does not depend on the core being up.
fn existing_goal(goal_id: &str, contract_version: u64, session: &str) -> Option<String> {
    let mut title = None;
    if let Ok(ep) = paths::read_endpoint() {
        let agent = http_agent();
        let auth = format!("Bearer {}", ep.token);
        let _ = agent
            .post(&format!(
                "http://127.0.0.1:{}/goals/{}/sessions",
                ep.port, goal_id
            ))
            .set("Authorization", &auth)
            .send_json(json!({"session_id": session, "agent": "claude-code"}));
        if let Ok(resp) = agent
            .get(&format!("http://127.0.0.1:{}/goals/{}", ep.port, goal_id))
            .set("Authorization", &auth)
            .call()
        {
            if let Ok(v) = resp.into_json::<Value>() {
                title = v.get("title").and_then(Value::as_str).map(String::from);
            }
        }
    }
    let goal_line = match title {
        Some(t) => format!("goal: \"{t}\""),
        None => format!("goal {goal_id}"),
    };
    Some(protocol_text(&goal_line, goal_id, contract_version))
}

/// Auto mode, unbound session: create this session's goal from the prompt.
/// Any failure returns None WITHOUT an instructed mark → the next prompt
/// retries. A goal that came back unwatched was deliberately opted out by
/// the human — stay silent and never re-watch it.
fn auto_create_goal(root: &Path, session: &str, prompt: Option<&str>) -> Option<Option<String>> {
    let ep = paths::read_endpoint().ok()?;
    let title = title_from_prompt(prompt, session);
    let resp: Value = http_agent()
        .post(&format!("http://127.0.0.1:{}/goals/auto", ep.port))
        .set("Authorization", &format!("Bearer {}", ep.token))
        .send_json(json!({
            "title": title,
            "project_dir": root.to_string_lossy(),
            "session_id": session,
            "agent": "claude-code",
        }))
        .ok()?
        .into_json()
        .ok()?;
    if !resp.get("watching").and_then(Value::as_bool).unwrap_or(false) {
        return Some(None);
    }
    let goal_id = resp.get("id").and_then(Value::as_str)?;
    let version = resp
        .get("contract_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let goal_line = format!(
        "goal: \"{}\" (auto-created from this prompt)",
        resp.get("title").and_then(Value::as_str).unwrap_or(&title)
    );
    Some(Some(protocol_text(&goal_line, goal_id, version)))
}

/// The goal title is the user's own words: first prompt, whitespace
/// collapsed, cut at 80 chars. The truncation lives here at the adapter
/// boundary — the schema stays agent-agnostic.
fn title_from_prompt(prompt: Option<&str>, session: &str) -> String {
    let collapsed = prompt
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        let sid8: String = session.chars().take(8).collect();
        return format!("session {sid8}");
    }
    let mut title: String = collapsed.chars().take(80).collect();
    if title.chars().count() < collapsed.chars().count() {
        title.push('…');
    }
    title
}

/// The contract-authoring prompt. Injected once per session; kept terse —
/// it rides along with every later turn of the conversation. Every command
/// carries the bin's absolute path (no PATH assumption) and the goal id
/// (goal identity travels in-context, never ambiently).
fn protocol_text(goal_line: &str, goal_id: &str, contract_version: u64) -> String {
    let bin = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "witnos".to_string());
    format!(
        "[witnos] This project is watched — {goal_line}, contract v{contract_version}. Your work here runs under a \
         verification contract that the user can edit WHILE you work. Protocol (all via the `witnos` CLI in Bash; \
         if `witnos` is not on PATH call it as \"{bin}\"; your goal id is {goal_id} — pass `--goal {goal_id}` \
         to every command):\n\
         1. Before implementing, lay out what you will verify: `witnos item lay --goal {goal_id}` with a JSON array on stdin; \
         each item = {{\"claim\": what must hold, \"check\": how you verify it}}. Items are SUBJECTIVE by default; \
         add \"class\":{{\"kind\":\"objective\",\"oracle\":{{\"command\":\"…\",\"expected\":\"…\"}},\"promoted_by\":\"agent\"}} \
         ONLY when a machine-runnable command truly decides it.\n\
         2. For every subjective item, record how you read it: `witnos item interpret <item-id> <your interpretation>`.\n\
         3. When you verify something, attach the evidence you judged by: `witnos evidence add <item-id>` with JSON on stdin \
         {{\"conclusion\":\"…\",\"basis\":\"…\",\"provenance\":[{{\"kind\":\"file\",\"path\":\"…\"}} or {{\"kind\":\"command\",\"cmd\":\"…\"}} or {{\"kind\":\"url\",\"url\":\"…\"}}]}}. \
         Evidence without provenance is rejected.\n\
         4. Objective items: run the oracle, then `witnos oracle report <item-id> --passed|--failed`.\n\
         5. After laying the initial contract, do ONE blindspot pass: `witnos item lay --goal {goal_id} --blindspot` with checks the user \
         likely didn't think to ask for.\n\
         6. The contract is alive: when it changes you'll see a delta after a tool call — address it, then \
         `witnos reconcile --goal {goal_id} --to <version>`.\n\
         7. Stopping is gated: you are released only when objective items passed, every subjective item carries \
         interpretation + evidence, and you've reconciled to the latest contract version. Human rulings on subjective \
         items happen AFTER you stop — \"awaiting rulings\" is a normal way to finish; never wait for them."
    )
}
