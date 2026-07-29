//! End-to-end tests of the real `witnos` binary: the fail-closed matrix for
//! the Stop gate and the fail-open behavior of the delivery channel, against
//! a hand-rolled mock core.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "witnos-e2e-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_marker(project: &Path, goal: &str, version: u64) {
    let dir = project.join(".witnos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("armed.json"),
        format!(r#"{{"goal_id":"{goal}","contract_version":{version}}}"#),
    )
    .unwrap();
}

fn write_endpoint(home: &Path, port: u16) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("endpoint.json"),
        format!(r#"{{"port":{port},"token":"test-token"}}"#),
    )
    .unwrap();
}

/// A session started in some other terminal (no Witnos scope stamp).
fn run_hook(sub: &str, project: &Path, home: &Path, stdin_json: &str) -> String {
    hook(sub, project, home, stdin_json, false)
}

/// A session started from Witnos's own embedded terminal.
fn run_hook_from_witnos(sub: &str, project: &Path, home: &Path, stdin_json: &str) -> String {
    hook(sub, project, home, stdin_json, true)
}

fn hook(
    sub: &str,
    project: &Path,
    home: &Path,
    stdin_json: &str,
    from_witnos: bool,
) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_witnos"));
    cmd.args(["hook", sub])
        .current_dir(project)
        .env("WITNOS_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Set or cleared explicitly, never inherited: the suite must mean the same
    // thing whether it runs inside Witnos's terminal or anywhere else.
    if from_witnos {
        cmd.env("WITNOS_TERMINAL", "1");
    } else {
        cmd.env_remove("WITNOS_TERMINAL");
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_json.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Minimal one-shot HTTP server: reads one request, sends the canned
/// response, closes.
fn spawn_server(status_line: &'static str, body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        data.extend_from_slice(&buf[..n]);
                        if let Some(pos) = find(&data, b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                            let cl: usize = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                            if data.len() >= pos + 4 + cl {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn dead_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn stdin_json(project: &Path) -> String {
    format!(
        r#"{{"session_id":"s1","cwd":"{}","stop_hook_active":false}}"#,
        project.display()
    )
}

// ---------- Stop gate: the fail-closed matrix ----------

#[test]
fn not_armed_allows_silently() {
    let project = temp_dir("noarm");
    let home = temp_dir("noarm-home");
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "unwatched project must not be touched");
}

#[test]
fn armed_but_core_unreachable_blocks_with_escape_hatch() {
    let project = temp_dir("unreach");
    let home = temp_dir("unreach-home");
    write_marker(&project, "g1", 3);
    write_endpoint(&home, dead_port());
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
    assert!(out.contains("witnos disarm"), "block reason must document the escape hatch: {out}");
}

#[test]
fn armed_but_no_endpoint_file_blocks() {
    let project = temp_dir("noep");
    let home = temp_dir("noep-home"); // empty: endpoint.json missing
    write_marker(&project, "g1", 1);
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
}

#[test]
fn armed_release_allows() {
    let project = temp_dir("release");
    let home = temp_dir("release-home");
    write_marker(&project, "g1", 3);
    write_endpoint(&home, spawn_server("200 OK", r#"{"decision":"release"}"#));
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "release must print nothing: {out}");
}

#[test]
fn armed_server_error_blocks() {
    let project = temp_dir("err500");
    let home = temp_dir("err500-home");
    write_marker(&project, "g1", 3);
    write_endpoint(&home, spawn_server("500 Internal Server Error", "{}"));
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
}

#[test]
fn armed_malformed_response_blocks() {
    let project = temp_dir("malformed");
    let home = temp_dir("malformed-home");
    write_marker(&project, "g1", 3);
    write_endpoint(&home, spawn_server("200 OK", "this is not json"));
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
}

#[test]
fn server_block_reason_is_forwarded_verbatim() {
    let project = temp_dir("fwd");
    let home = temp_dir("fwd-home");
    write_marker(&project, "g1", 3);
    write_endpoint(
        &home,
        spawn_server(
            "200 OK",
            r#"{"decision":"block","reason":"objective item not passed: cargo test"}"#,
        ),
    );
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains("objective item not passed: cargo test"), "got: {out}");
}

#[test]
fn garbage_stdin_still_fails_closed_via_process_cwd() {
    let project = temp_dir("garbage");
    let home = temp_dir("garbage-home");
    write_marker(&project, "g1", 1);
    let out = run_hook("stop", &project, &home, "not json at all");
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
}

// ---------- Delivery channel: fail-open + local version compare ----------

#[test]
fn delivery_silent_when_contract_unchanged() {
    let project = temp_dir("dlv-same");
    let home = temp_dir("dlv-same-home");
    write_marker(&project, "g1", 3);
    std::fs::write(project.join(".witnos/delivered.json"), r#"{"s1":3}"#).unwrap();
    let out = run_hook("post-tool-use", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "unchanged contract must cost nothing: {out}");
}

#[test]
fn delivery_injects_delta_and_records_version() {
    let project = temp_dir("dlv-delta");
    let home = temp_dir("dlv-delta-home");
    write_marker(&project, "g1", 5);
    std::fs::write(project.join(".witnos/delivered.json"), r#"{"s1":2}"#).unwrap();
    write_endpoint(
        &home,
        spawn_server("200 OK", r#"{"version":5,"summary":"- \"calm UI\" edited: no animation at all"}"#),
    );
    let out = run_hook("post-tool-use", &project, &home, &stdin_json(&project));
    assert!(out.contains("additionalContext"), "got: {out}");
    assert!(out.contains("no animation at all"), "got: {out}");
    let delivered = std::fs::read_to_string(project.join(".witnos/delivered.json")).unwrap();
    assert!(delivered.contains(r#""s1": 5"#) || delivered.contains(r#""s1":5"#), "got: {delivered}");
}

#[test]
fn delivery_fails_open_when_core_unreachable() {
    let project = temp_dir("dlv-open");
    let home = temp_dir("dlv-open-home"); // no endpoint.json
    write_marker(&project, "g1", 5);
    std::fs::write(project.join(".witnos/delivered.json"), r#"{"s1":2}"#).unwrap();
    let out = run_hook("post-tool-use", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "delivery must fail open silently: {out}");
}

// ---------- marker v2: session routing + auto-mode fail-closed ----------

fn write_raw_marker(project: &Path, content: &str) {
    let dir = project.join(".witnos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("armed.json"), content).unwrap();
}

/// Like spawn_server, but also hands back the raw request it served.
fn spawn_capture_server(
    status_line: &'static str,
    body: &'static str,
) -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        data.extend_from_slice(&buf[..n]);
                        if let Some(pos) = find(&data, b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                            let cl: usize = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                            if data.len() >= pos + 4 + cl {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&data).into_owned());
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    (port, rx)
}

#[test]
fn stop_routes_the_session_to_its_own_goal_not_the_default() {
    let project = temp_dir("v2-route");
    let home = temp_dir("v2-route-home");
    write_raw_marker(
        &project,
        r#"{"v":2,"auto":true,
           "default_goal":{"goal_id":"g-default","contract_version":9},
           "sessions":{"s1":{"goal_id":"g-mine","contract_version":4}}}"#,
    );
    let (port, rx) = spawn_capture_server("200 OK", r#"{"decision":"release"}"#);
    write_endpoint(&home, port);
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "release: {out}");
    let req = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(req.contains("g-mine"), "must gate against the session's goal: {req}");
    assert!(!req.contains("g-default"), "never a neighbor's goal: {req}");
}

#[test]
fn stop_unbound_session_in_auto_asks_core_with_project_dir() {
    let project = temp_dir("v2-unbound");
    let home = temp_dir("v2-unbound-home");
    write_raw_marker(&project, r#"{"v":2,"auto":true}"#);
    let (port, rx) = spawn_capture_server(
        "200 OK",
        r#"{"decision":"block","reason":"no goal for this session"}"#,
    );
    write_endpoint(&home, port);
    let out = run_hook_from_witnos("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains("no goal for this session"), "got: {out}");
    let req = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(req.contains("project_dir"), "resolution key must travel: {req}");
    assert!(req.contains(r#""session_id":"s1""#), "got: {req}");
}

#[test]
fn stop_auto_marker_blocks_when_core_unreachable() {
    let project = temp_dir("v2-dead");
    let home = temp_dir("v2-dead-home");
    write_raw_marker(&project, r#"{"v":2,"auto":true}"#);
    write_endpoint(&home, dead_port());
    let out = run_hook_from_witnos("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "got: {out}");
    assert!(out.contains("witnos disarm"), "escape hatch must be documented: {out}");
}

#[test]
fn stop_garbage_marker_content_still_blocks() {
    let project = temp_dir("v2-garbage");
    let home = temp_dir("v2-garbage-home");
    write_raw_marker(&project, "{{{ not json");
    let out = run_hook_from_witnos("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "torn marker must stall: {out}");
    assert!(out.contains("witnos disarm"), "got: {out}");
}

/// A session with no goal of its own, started outside Witnos's terminal, is
/// never stalled — not by a dead core, not by a torn marker. Stalling it could
/// not protect any contract, and it would land in whatever terminal the user
/// is actually working in.
#[test]
fn stop_unbound_outside_witnos_terminal_releases() {
    let project = temp_dir("v2-outside");
    let home = temp_dir("v2-outside-home");
    write_raw_marker(&project, r#"{"v":2,"auto":true}"#);
    write_endpoint(&home, dead_port());
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "out-of-scope session must not be stalled: {out}");

    write_raw_marker(&project, "{{{ not json");
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "not even a torn marker may stall it: {out}");
}

/// The flip side: owning a goal is what gets you gated. A session bound in the
/// marker is evaluated exactly as before even though it was started elsewhere —
/// silently dropping enforcement on a contract the human is editing would be
/// the worse failure.
#[test]
fn stop_bound_session_is_gated_even_outside_witnos_terminal() {
    let project = temp_dir("v2-bound-out");
    let home = temp_dir("v2-bound-out-home");
    write_raw_marker(
        &project,
        r#"{"v":2,"auto":true,"sessions":{"s1":{"goal_id":"g-mine","contract_version":4}}}"#,
    );
    write_endpoint(&home, dead_port());
    let out = run_hook("stop", &project, &home, &stdin_json(&project));
    assert!(out.contains(r#""decision":"block""#), "bound session keeps its gate: {out}");
}

#[test]
fn delivery_uses_the_sessions_own_entry() {
    let project = temp_dir("v2-dlv");
    let home = temp_dir("v2-dlv-home");
    write_raw_marker(
        &project,
        r#"{"v":2,"auto":true,
           "sessions":{"s1":{"goal_id":"g-mine","contract_version":5,"agent_synced_version":0}}}"#,
    );
    std::fs::write(project.join(".witnos/delivered.json"), r#"{"s1":2}"#).unwrap();
    let (port, rx) = spawn_capture_server(
        "200 OK",
        r#"{"version":5,"summary":"- \"calm UI\" edited"}"#,
    );
    write_endpoint(&home, port);
    let out = run_hook("post-tool-use", &project, &home, &stdin_json(&project));
    assert!(out.contains("additionalContext"), "got: {out}");
    let req = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(req.contains("/goals/g-mine/contract?since=2"), "got: {req}");
}

#[test]
fn delivery_silent_for_a_session_with_no_goal() {
    let project = temp_dir("v2-dlv-nogoal");
    let home = temp_dir("v2-dlv-nogoal-home"); // no endpoint: would fail loudly if it tried
    write_raw_marker(
        &project,
        r#"{"v":2,"auto":true,
           "sessions":{"other":{"goal_id":"g-x","contract_version":5}}}"#,
    );
    let out = run_hook("post-tool-use", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "no goal → nothing to deliver: {out}");
}

#[test]
fn session_end_fails_open_when_core_dead() {
    let project = temp_dir("se-open");
    let home = temp_dir("se-open-home"); // no endpoint.json: core down
    write_raw_marker(
        &project,
        r#"{"v":2,"auto":true,
           "sessions":{"s1":{"goal_id":"g-mine","contract_version":1}}}"#,
    );
    let out = run_hook("session-end", &project, &home, &stdin_json(&project));
    assert_eq!(out.trim(), "", "bookkeeping must fail open silently: {out}");
}
