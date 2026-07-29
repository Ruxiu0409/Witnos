//! The milestone test: the entire v1 loop, headless, through the REAL
//! `witnos` binary against the REAL core server.
//!
//! empty contract blocks → agent lays items → gate blocks with the delta →
//! interpret/evidence/oracle/reconcile → gate releases (goal parks awaiting
//! rulings) → human edits mid-run (marker mirrors the bump) → delivery
//! injects the delta → gate blocks again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "witnos-loop-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Core {
    base: String,
    token: String,
    state: std::sync::Arc<witnos_server::AppState>,
    _rt: tokio::runtime::Runtime,
}

fn start_core(home: &Path) -> Core {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(witnos_server::start(home)).unwrap();
    Core {
        base: format!("http://127.0.0.1:{}", handle.port),
        token: handle.token,
        state: handle.state,
        _rt: rt,
    }
}

impl Core {
    fn get(&self, path: &str) -> Value {
        ureq::get(&format!("{}{}", self.base, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .unwrap()
            .into_json()
            .unwrap()
    }
    fn post(&self, path: &str, body: Value) -> Value {
        ureq::post(&format!("{}{}", self.base, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
            .unwrap()
            .into_json()
            .unwrap()
    }
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

fn hook(kind: &str, project: &Path, home: &Path) -> String {
    hook_as(kind, "s1", project, home)
}

fn hook_as(kind: &str, session: &str, project: &Path, home: &Path) -> String {
    let stdin = format!(
        r#"{{"session_id":"{session}","cwd":"{}","stop_hook_active":false}}"#,
        project.display()
    );
    run_bin(&["hook", kind], project, home, Some(&stdin)).0
}

fn ups_as(session: &str, prompt: &str, project: &Path, home: &Path) -> String {
    let stdin = format!(
        r#"{{"session_id":"{session}","cwd":"{}","prompt":"{prompt}"}}"#,
        project.display()
    );
    run_bin(&["hook", "user-prompt-submit"], project, home, Some(&stdin)).0
}

#[test]
fn whole_v1_loop_headless() {
    let home = temp_dir("home");
    let project = temp_dir("project");
    let core = start_core(&home);

    // -- goal created and watched: the armed marker appears in the project.
    let goal = core.post("/goals", json!({"title": "demo goal"}));
    let gid = goal["id"].as_str().unwrap().to_string();
    core.post(
        &format!("/goals/{gid}/watch"),
        json!({"project_dir": project.to_str().unwrap()}),
    );
    let marker_path = project.join(".witnos/armed.json");
    assert!(marker_path.exists(), "watch must write the armed marker");

    // -- 1. empty contract → the gate blocks.
    let out = hook("stop", &project, &home);
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
    assert!(out.contains("contract is empty"), "got: {out}");

    // -- 2. the agent lays the contract through the CLI.
    let lay_input = r#"[
        {"claim":"cargo test passes","check":"run cargo test",
         "class":{"kind":"objective","oracle":{"command":"cargo test","expected":"exit 0"},"promoted_by":"agent"}},
        {"claim":"UI feels calm","check":"open the app and look at it"}
    ]"#;
    let (stdout, stderr, ok) = run_bin(&["item", "lay"], &project, &home, Some(lay_input));
    assert!(ok, "lay failed: {stderr}");
    assert!(stdout.contains("contract now v2"), "got: {stdout}");

    let g = core.get(&format!("/goals/{gid}"));
    let items = g["items"].as_array().unwrap().clone();
    let id_of = |claim: &str| {
        items
            .iter()
            .find(|i| i["claim"] == claim)
            .unwrap_or_else(|| panic!("item {claim} not found"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let obj = id_of("cargo test passes");
    let subj = id_of("UI feels calm");

    // Marker mirrored the bump — the delivery channel's no-network check.
    // (v2 shape: the manually-watched goal is the default_goal.)
    let m: Value =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(
        m["default_goal"]["contract_version"], 2,
        "marker must mirror the contract version: {m}"
    );

    // -- 3. gate blocks with a per-item delta, never a bare "no".
    let out = hook("stop", &project, &home);
    assert!(out.contains("cargo test passes"), "got: {out}");
    assert!(out.contains("UI feels calm"), "got: {out}");
    assert!(out.contains("contract moved"), "got: {out}");

    // -- 4. the agent completes its side.
    let (_, e, ok) = run_bin(
        &["item", "interpret", &subj, "calm = generous whitespace, zero motion"],
        &project,
        &home,
        None,
    );
    assert!(ok, "interpret failed: {e}");
    let evidence = r#"{"conclusion":"holds","basis":"screenshot shows static layout, 3 colors",
        "provenance":[{"kind":"command","cmd":"screencapture -x shot.png"}]}"#;
    let (_, e, ok) = run_bin(&["evidence", "add", &subj], &project, &home, Some(evidence));
    assert!(ok, "evidence add failed: {e}");
    let (_, e, ok) = run_bin(&["oracle", "report", &obj, "--passed"], &project, &home, None);
    assert!(ok, "oracle report failed: {e}");
    let (_, e, ok) = run_bin(&["evidence", "add", &obj], &project, &home, Some(evidence));
    assert!(ok, "evidence add failed: {e}");
    let (_, e, ok) = run_bin(&["reconcile", "--to", "2"], &project, &home, None);
    assert!(ok, "reconcile failed: {e}");

    // -- 5. gate releases; goal parks in awaiting_rulings (normal terminal).
    let out = hook("stop", &project, &home);
    assert_eq!(out.trim(), "", "release must print nothing, got: {out}");
    let g = core.get(&format!("/goals/{gid}"));
    assert_eq!(g["status"], "awaiting_rulings");

    // -- 6. the human edits the subjective yardstick mid-run.
    core.post(
        &format!("/goals/{gid}/items/{subj}/edit"),
        json!({"actor": "human", "claim": "UI feels calm (no animation AT ALL)"}),
    );
    let m: Value =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(
        m["default_goal"]["contract_version"], 3,
        "marker must mirror the human edit: {m}"
    );

    // -- 7. the delivery channel injects ONLY the delta into the session.
    let out = hook("post-tool-use", &project, &home);
    assert!(out.contains("additionalContext"), "got: {out}");
    assert!(out.contains("no animation AT ALL"), "got: {out}");
    assert!(
        !out.contains("cargo test passes"),
        "delta must not re-feed unchanged items: {out}"
    );
    let delivered =
        std::fs::read_to_string(project.join(".witnos/delivered.json")).unwrap();
    assert!(delivered.contains("s1"), "got: {delivered}");

    // -- 8. and the gate holds again until the agent re-addresses it.
    let out = hook("stop", &project, &home);
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
    assert!(out.contains("no animation AT ALL"), "got: {out}");

    // -- 9. approaching the consecutive-block cap, the gate tells the agent
    //       to persist state before the turn gets cut off.
    let stalled_stdin = format!(
        r#"{{"session_id":"s1","cwd":"{}","stop_hook_active":true}}"#,
        project.display()
    );
    let mut last = String::new();
    for _ in 0..6 {
        last = run_bin(&["hook", "stop"], &project, &home, Some(&stalled_stdin)).0;
    }
    assert!(
        last.contains("persist what is already done"),
        "near-cap blocks must warn the agent to save state: {last}"
    );

    // The instrumentation trail is in place: events recorded the whole loop.
    let g = core.get(&format!("/goals/{gid}"));
    let kinds: Vec<&str> = g["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"contract_edited"));
    assert!(kinds.contains(&"evidence_added"));
    assert!(kinds.contains(&"reconcile"));
    assert!(kinds.contains(&"gate_decision"));
}

/// The auto-mode correctness kernel: one goal per session, each session
/// gated against ITS OWN contract — never a neighbor's.
#[test]
fn auto_mode_gates_each_session_against_its_own_goal() {
    let home = temp_dir("auto-home");
    let project = temp_dir("auto-project");
    let core = start_core(&home);

    // -- the human watches the project (what the app's IPC does).
    witnos_server::register_project(&core.state, project.to_str().unwrap()).unwrap();
    let marker_path = project.join(".witnos/armed.json");
    let m: Value = serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(m["auto"], true, "registering must arm an auto marker: {m}");

    // -- pre-goal, the gate already fails closed (the dir was opted in).
    let out = hook_as("stop", "sA", &project, &home);
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
    assert!(out.contains("no goal"), "reason must explain the unbound session: {out}");

    // -- session A's first prompt creates goal A; protocol carries --goal.
    let out = ups_as("sA", "Fix the login bug", &project, &home);
    assert!(out.contains("additionalContext"), "got: {out}");
    assert!(out.contains("auto-created"), "got: {out}");
    assert!(out.contains("Fix the login bug"), "title = the user's words: {out}");
    assert!(out.contains("--goal"), "protocol must pin the goal id: {out}");

    // -- session B gets its OWN goal.
    let out = ups_as("sB", "Refactor payments", &project, &home);
    assert!(out.contains("Refactor payments"), "got: {out}");

    let m: Value = serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    let ga = m["sessions"]["sA"]["goal_id"].as_str().unwrap().to_string();
    let gb = m["sessions"]["sB"]["goal_id"].as_str().unwrap().to_string();
    assert_ne!(ga, gb, "one goal per session: {m}");

    // -- with two active goals, ambient goal resolution must refuse.
    let (_, stderr, ok) = run_bin(&["contract", "show"], &project, &home, None);
    assert!(!ok);
    assert!(stderr.contains("--goal"), "ambiguity must demand --goal: {stderr}");

    // -- a human edit on goal A reaches session A only.
    core.post(
        &format!("/goals/{ga}/items"),
        json!({"actor": "human", "items": [
            {"claim": "login works on Safari", "check": "manual check", "origin": {"kind": "user_pre_run"}}
        ]}),
    );
    let out = hook_as("post-tool-use", "sA", &project, &home);
    assert!(out.contains("login works on Safari"), "delta must reach A: {out}");
    let out = hook_as("post-tool-use", "sB", &project, &home);
    assert_eq!(out.trim(), "", "B has no delta — must stay silent: {out}");

    // -- session A completes ITS contract (via --goal), B does nothing.
    let g = core.get(&format!("/goals/{ga}"));
    let item = g["items"][0]["id"].as_str().unwrap().to_string();
    let (_, e, ok) = run_bin(
        &["item", "interpret", "--goal", &ga, &item, "works = no console errors on Safari 18"],
        &project, &home, None,
    );
    assert!(ok, "interpret failed: {e}");
    let evidence = r#"{"conclusion":"holds","basis":"manual run, no errors",
        "provenance":[{"kind":"command","cmd":"open -a Safari http://localhost"}]}"#;
    let (_, e, ok) = run_bin(&["evidence", "add", "--goal", &ga, &item], &project, &home, Some(evidence));
    assert!(ok, "evidence add failed: {e}");
    let (_, e, ok) = run_bin(&["reconcile", "--goal", &ga, "--to", "1"], &project, &home, None);
    assert!(ok, "reconcile failed: {e}");

    // -- the kernel: A releases, B still blocks.
    let out = hook_as("stop", "sA", &project, &home);
    assert_eq!(out.trim(), "", "A met its own contract and must release: {out}");
    let out = hook_as("stop", "sB", &project, &home);
    assert!(out.contains(r#""decision":"block""#), "B's contract is empty → block: {out}");

    // -- opt-out: the human unwatches B's goal; B's gate releases.
    ureq::delete(&format!("{}/goals/{gb}/watch", core.base))
        .set("Authorization", &format!("Bearer {}", core.token))
        .call()
        .unwrap();
    let out = hook_as("stop", "sB", &project, &home);
    assert_eq!(out.trim(), "", "deliberate opt-out must release: {out}");

    // -- graceful stop removes the marker; restart re-arms it (registry).
    witnos_server::graceful_stop(&core.state);
    assert!(!marker_path.exists(), "graceful stop must disarm");
    drop(core);
    let _core2 = start_core(&home);
    let m: Value = serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(m["auto"], true, "restart must re-arm the auto project: {m}");
    assert_eq!(
        m["sessions"]["sA"]["goal_id"].as_str().unwrap(),
        ga,
        "watching session goals must survive the restart: {m}"
    );
}
