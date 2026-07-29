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

// ---------- auto mode: goal creation from the first prompt ----------

struct AutoCore {
    base: String,
    auth: String,
    state: std::sync::Arc<witnos_server::AppState>,
    _rt: tokio::runtime::Runtime,
}

fn start_auto_core(home: &Path, project: &Path) -> AutoCore {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(witnos_server::start(home)).unwrap();
    witnos_server::register_project(&handle.state, project.to_str().unwrap()).unwrap();
    AutoCore {
        base: format!("http://127.0.0.1:{}", handle.port),
        auth: format!("Bearer {}", handle.token),
        state: handle.state,
        _rt: rt,
    }
}

fn goals_list(core: &AutoCore) -> Vec<Value> {
    ureq::get(&format!("{}/goals", core.base))
        .set("Authorization", &core.auth)
        .call()
        .unwrap()
        .into_json::<Value>()
        .unwrap()
        .as_array()
        .cloned()
        .unwrap()
}

fn ups(project: &Path, home: &Path, session: &str, prompt: &str) -> String {
    let stdin = format!(
        r#"{{"session_id":"{session}","cwd":"{}","prompt":"{prompt}"}}"#,
        project.display()
    );
    run_bin(&["hook", "user-prompt-submit"], project, home, Some(&stdin)).0
}

#[test]
fn ups_auto_creates_one_goal_per_session_from_the_prompt() {
    let project = temp_dir("aups");
    let home = temp_dir("aups-home");
    let core = start_auto_core(&home, &project);

    let out = ups(&project, &home, "s-auto", "Fix the login bug");
    assert!(out.contains("additionalContext"), "got: {out}");
    assert!(out.contains("auto-created"), "got: {out}");
    assert!(out.contains("--goal"), "protocol must pin the goal id: {out}");
    let goals = goals_list(&core);
    assert_eq!(goals.len(), 1, "got: {goals:?}");
    assert_eq!(goals[0]["title"], "Fix the login bug", "title = the user's words");

    // Marker carries the session entry.
    let m: Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".witnos/armed.json")).unwrap(),
    )
    .unwrap();
    assert!(m["sessions"]["s-auto"]["goal_id"].is_string(), "got: {m}");

    // Same session again: silent, no second goal.
    let out = ups(&project, &home, "s-auto", "and another thing");
    assert_eq!(out.trim(), "", "must instruct only once per session: {out}");
    assert_eq!(goals_list(&core).len(), 1);

    // A new session gets its own goal.
    let out = ups(&project, &home, "s-two", "Refactor payments");
    assert!(out.contains("Refactor payments"), "got: {out}");
    assert_eq!(goals_list(&core).len(), 2);
}

#[test]
fn ups_auto_truncates_long_titles_and_collapses_whitespace() {
    let project = temp_dir("aups-trunc");
    let home = temp_dir("aups-trunc-home");
    let core = start_auto_core(&home, &project);

    let long = "word ".repeat(30); // 150 chars, with a trailing space
    let out = ups(&project, &home, "s-long", long.trim());
    assert!(out.contains("additionalContext"), "got: {out}");
    let goals = goals_list(&core);
    let title = goals[0]["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 81, "80 chars + ellipsis: {title:?}");
    assert!(title.ends_with('…'), "got: {title:?}");

    // Newlines collapse into single spaces (JSON \n in the prompt).
    let out = ups(&project, &home, "s-nl", r"first line\nsecond line");
    assert!(out.contains("additionalContext"), "got: {out}");
    let goals = goals_list(&core);
    let t = goals
        .iter()
        .find(|g| g["title"].as_str().unwrap().starts_with("first"))
        .unwrap();
    assert_eq!(t["title"], "first line second line");
}

#[test]
fn ups_auto_retries_until_the_core_is_back() {
    let project = temp_dir("aups-retry");
    let home = temp_dir("aups-retry-home"); // no endpoint.json yet: core down
    let dir = project.join(".witnos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("armed.json"), r#"{"v":2,"auto":true}"#).unwrap();

    // Core down: silent, and crucially NOT marked as instructed.
    let out = ups(&project, &home, "s-r", "do the thing");
    assert_eq!(out.trim(), "", "fail open silently: {out}");
    assert!(
        !project.join(".witnos/instructed.json").exists(),
        "a failed creation must not consume the once-per-session injection"
    );

    // Core comes back: the same session's next prompt heals everything.
    let core = start_auto_core(&home, &project);
    let out = ups(&project, &home, "s-r", "do the thing");
    assert!(out.contains("additionalContext"), "retry must inject: {out}");
    assert_eq!(goals_list(&core).len(), 1);
}

#[test]
fn ups_auto_respects_a_human_opt_out() {
    let project = temp_dir("aups-opt");
    let home = temp_dir("aups-opt-home");
    let core = start_auto_core(&home, &project);

    // The session's goal exists (created via HTTP, as the hook would)…
    let goal: Value = ureq::post(&format!("{}/goals/auto", core.base))
        .set("Authorization", &core.auth)
        .send_json(json!({
            "title": "opted out",
            "project_dir": project.to_str().unwrap(),
            "session_id": "s-opt",
        }))
        .unwrap()
        .into_json()
        .unwrap();
    let gid = goal["id"].as_str().unwrap();
    // …and the human unwatches it.
    ureq::delete(&format!("{}/goals/{gid}/watch", core.base))
        .set("Authorization", &core.auth)
        .call()
        .unwrap();

    // The hook must stay silent and must NOT re-watch or duplicate it.
    let out = ups(&project, &home, "s-opt", "try again anyway");
    assert_eq!(out.trim(), "", "opt-out wins: {out}");
    let goals = goals_list(&core);
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0]["watching"], false, "must not re-watch: {goals:?}");
    // The store keeps the goal's session ownership, so the gate can tell
    // "opted out" from "never bound".
    let owned = core
        .state
        .store
        .find_session_goal(project.to_str().unwrap(), "s-opt")
        .expect("auto_session ownership must survive the opt-out");
    assert_eq!(owned.id, gid);
}
