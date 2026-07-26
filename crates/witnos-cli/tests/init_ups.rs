//! Tests for `witnos init` (idempotent settings merge) and the
//! UserPromptSubmit hook (once-per-session protocol injection + binding).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "witnos-iups-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_bin(
    args: &[&str],
    project: &Path,
    home: &Path,
    stdin: Option<&str>,
) -> (String, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_witnos"))
        .args(args)
        .current_dir(project)
        .env("WITNOS_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or("").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn read_settings(project: &Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(project.join(".claude/settings.json")).unwrap(),
    )
    .unwrap()
}

fn hook_commands(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|h| h["command"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ---------- init ----------

#[test]
fn init_installs_three_hooks_idempotently() {
    let project = temp_dir("init");
    let home = temp_dir("init-home");

    let (_, e, ok) = run_bin(&["init"], &project, &home, None);
    assert!(ok, "init failed: {e}");

    let s = read_settings(&project);
    assert_eq!(hook_commands(&s, "Stop").len(), 1);
    assert!(hook_commands(&s, "Stop")[0].ends_with("hook stop"));
    assert_eq!(hook_commands(&s, "PostToolUse").len(), 1);
    assert_eq!(s["hooks"]["PostToolUse"][0]["matcher"], "*");
    assert_eq!(hook_commands(&s, "UserPromptSubmit").len(), 1);

    // Second run: no duplicates.
    let (_, e, ok) = run_bin(&["init"], &project, &home, None);
    assert!(ok, "re-init failed: {e}");
    let s = read_settings(&project);
    assert_eq!(hook_commands(&s, "Stop").len(), 1, "must not duplicate");
    assert_eq!(hook_commands(&s, "PostToolUse").len(), 1);
    assert_eq!(hook_commands(&s, "UserPromptSubmit").len(), 1);
}

#[test]
fn init_merges_preserving_existing_settings() {
    let project = temp_dir("merge");
    let home = temp_dir("merge-home");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(
        project.join(".claude/settings.json"),
        serde_json::to_string_pretty(&json!({
            "permissions": {"allow": ["Bash(ls:*)"]},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo custom"}]}]}
        }))
        .unwrap(),
    )
    .unwrap();

    let (_, e, ok) = run_bin(&["init"], &project, &home, None);
    assert!(ok, "init failed: {e}");

    let s = read_settings(&project);
    assert_eq!(s["permissions"]["allow"][0], "Bash(ls:*)", "foreign keys preserved");
    let stops = hook_commands(&s, "Stop");
    assert_eq!(stops.len(), 2, "custom hook preserved alongside ours: {stops:?}");
    assert_eq!(stops[0], "echo custom");
    assert!(stops[1].ends_with("hook stop"));
}

// ---------- user-prompt-submit ----------

fn write_marker(project: &Path, goal: &str, version: u64) {
    let dir = project.join(".witnos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("armed.json"),
        format!(r#"{{"goal_id":"{goal}","contract_version":{version}}}"#),
    )
    .unwrap();
}

fn ups_stdin(project: &Path, session: &str) -> String {
    format!(
        r#"{{"session_id":"{session}","cwd":"{}","prompt":"do the thing"}}"#,
        project.display()
    )
}

#[test]
fn ups_injects_protocol_once_and_binds_session() {
    let project = temp_dir("ups");
    let home = temp_dir("ups-home");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(witnos_server::start(&home)).unwrap();
    let base = format!("http://127.0.0.1:{}", handle.port);
    let auth = format!("Bearer {}", handle.token);

    let goal: Value = ureq::post(&format!("{base}/goals"))
        .set("Authorization", &auth)
        .send_json(json!({"title": "ups demo"}))
        .unwrap()
        .into_json()
        .unwrap();
    let gid = goal["id"].as_str().unwrap().to_string();
    ureq::post(&format!("{base}/goals/{gid}/watch"))
        .set("Authorization", &auth)
        .send_json(json!({"project_dir": project.to_str().unwrap()}))
        .unwrap();

    // First prompt: protocol injected, session bound.
    let out = run_bin(
        &["hook", "user-prompt-submit"],
        &project,
        &home,
        Some(&ups_stdin(&project, "s-ups")),
    )
    .0;
    assert!(out.contains("additionalContext"), "got: {out}");
    assert!(out.contains("witnos item lay"), "got: {out}");
    assert!(out.contains("blindspot"), "got: {out}");
    assert!(out.contains("ups demo"), "should carry the goal title: {out}");

    let g: Value = ureq::get(&format!("{base}/goals/{gid}"))
        .set("Authorization", &auth)
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    let sessions: Vec<&str> = g["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert!(sessions.contains(&"s-ups"), "got: {sessions:?}");

    // Second prompt in the same session: silent.
    let out = run_bin(
        &["hook", "user-prompt-submit"],
        &project,
        &home,
        Some(&ups_stdin(&project, "s-ups")),
    )
    .0;
    assert_eq!(out.trim(), "", "must instruct only once per session: {out}");

    // A different session gets instructed again.
    let out = run_bin(
        &["hook", "user-prompt-submit"],
        &project,
        &home,
        Some(&ups_stdin(&project, "s-other")),
    )
    .0;
    assert!(out.contains("additionalContext"), "got: {out}");
}

#[test]
fn ups_silent_when_not_armed() {
    let project = temp_dir("ups-noarm");
    let home = temp_dir("ups-noarm-home");
    let out = run_bin(
        &["hook", "user-prompt-submit"],
        &project,
        &home,
        Some(&ups_stdin(&project, "s1")),
    )
    .0;
    assert_eq!(out.trim(), "", "unwatched project must stay untouched: {out}");
}

#[test]
fn ups_fails_open_but_still_injects_locally_when_core_down() {
    let project = temp_dir("ups-down");
    let home = temp_dir("ups-down-home"); // no endpoint.json
    write_marker(&project, "g-local", 4);
    let out = run_bin(
        &["hook", "user-prompt-submit"],
        &project,
        &home,
        Some(&ups_stdin(&project, "s1")),
    )
    .0;
    assert!(
        out.contains("additionalContext") && out.contains("contract v4"),
        "injection is locally decided and must survive a dead core: {out}"
    );
}
