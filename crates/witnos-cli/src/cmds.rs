//! Agent-facing subcommands. Failures print a self-explanatory message to
//! stderr and exit non-zero — the agent reads that from its Bash call.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::client;

fn fail(msg: &str) -> ExitCode {
    eprintln!("witnos: {msg}");
    ExitCode::FAILURE
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn read_stdin() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    Ok(s)
}

pub fn contract_show(args: &[String]) -> ExitCode {
    let since: u64 = flag_value(args, "--since")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    match ctx.get(&format!(
        "/goals/{}/contract?since={since}",
        ctx.marker.goal_id
    )) {
        Ok(v) => {
            println!(
                "contract v{} (you are synced to v{})",
                v["version"], v["agent_synced_version"]
            );
            println!("{}", v["summary"].as_str().unwrap_or(""));
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

pub fn item_lay(args: &[String]) -> ExitCode {
    let origin = if args.iter().any(|a| a == "--blindspot") {
        "agent_blindspot"
    } else {
        "agent_initial"
    };
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let raw = match read_stdin() {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return fail(&format!("stdin must be a JSON array of items: {e}")),
    };
    let Some(arr) = parsed.as_array() else {
        return fail("stdin must be a JSON array of items ({claim, check, class?, interpretation?})");
    };
    // Origin is stamped HERE, not taken from input: an agent must never be
    // able to write user-origin items (core-bet instrumentation integrity).
    let wrapped: Vec<Value> = arr
        .iter()
        .map(|it| {
            let mut o = it.clone();
            if let Some(m) = o.as_object_mut() {
                m.insert("origin".into(), json!({"kind": origin}));
            }
            o
        })
        .collect();
    match ctx.post(
        &format!("/goals/{}/items", ctx.marker.goal_id),
        json!({"actor": "agent", "items": wrapped}),
    ) {
        Ok(v) => {
            let ids = v["ids"].as_array().cloned().unwrap_or_default();
            for (id, item) in ids.iter().zip(arr) {
                println!(
                    "laid {}  {}",
                    id.as_str().unwrap_or("?"),
                    item["claim"].as_str().unwrap_or("")
                );
            }
            println!("contract now v{}", v["version"]);
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

pub fn item_interpret(args: &[String]) -> ExitCode {
    let Some(item_id) = args.first() else {
        return fail("usage: witnos item interpret <item-id> <text…>   (or text on stdin)");
    };
    let text = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        match read_stdin() {
            Ok(s) => s.trim().to_string(),
            Err(e) => return fail(&e),
        }
    };
    if text.is_empty() {
        return fail("interpretation text required (as arguments or on stdin)");
    }
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    match ctx.post(
        &format!("/goals/{}/interpret", ctx.marker.goal_id),
        json!({"item_id": item_id, "text": text}),
    ) {
        Ok(_) => {
            println!("interpretation recorded for {item_id}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

pub fn evidence_add(args: &[String]) -> ExitCode {
    let Some(item_id) = args.first() else {
        return fail(
            "usage: witnos evidence add <item-id>   with JSON on stdin: \
             {conclusion, basis, provenance:[{kind:\"file\"|\"command\"|\"url\", …}]}",
        );
    };
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let raw = match read_stdin() {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let mut body: Value = match serde_json::from_str(&raw) {
        Ok(Value::Object(o)) => Value::Object(o),
        Ok(_) => return fail("stdin must be a JSON object"),
        Err(e) => return fail(&format!("stdin must be a JSON object: {e}")),
    };
    // Stamp the workspace fingerprint at capture time unless the caller
    // supplied one — this is what lets the UI flag stale evidence.
    if body.get("workspace").is_none() {
        body["workspace"] = fingerprint(&ctx.root);
    }
    body["item_id"] = json!(item_id);
    match ctx.post(&format!("/goals/{}/evidence", ctx.marker.goal_id), body) {
        Ok(v) => {
            println!(
                "evidence {} attached to {item_id}",
                v["evidence_id"].as_str().unwrap_or("?")
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

pub fn oracle_report(args: &[String]) -> ExitCode {
    let Some(item_id) = args.first() else {
        return fail("usage: witnos oracle report <item-id> --passed|--failed");
    };
    let passed = match (
        args.iter().any(|a| a == "--passed"),
        args.iter().any(|a| a == "--failed"),
    ) {
        (true, false) => true,
        (false, true) => false,
        _ => return fail("exactly one of --passed / --failed is required"),
    };
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    match ctx.post(
        &format!("/goals/{}/oracle", ctx.marker.goal_id),
        json!({"item_id": item_id, "passed": passed}),
    ) {
        Ok(_) => {
            println!(
                "oracle result recorded for {item_id}: {}",
                if passed { "passed" } else { "failed" }
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

pub fn reconcile(args: &[String]) -> ExitCode {
    let Some(to) = flag_value(args, "--to").and_then(|v| v.parse::<u64>().ok()) else {
        return fail("usage: witnos reconcile --to <version> [--reinterpreted id,id,…]");
    };
    let reinterpreted: Vec<String> = flag_value(args, "--reinterpreted")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let ctx = match client::ctx() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    match ctx.post(
        &format!("/goals/{}/reconcile", ctx.marker.goal_id),
        json!({
            "session_id": "agent-cli",
            "to_version": to,
            "reinterpreted_items": reinterpreted,
        }),
    ) {
        Ok(_) => {
            println!("reconciled to v{to}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

/// Workspace fingerprint via git. `DefaultHasher` is not stable across Rust
/// releases — fine here: fingerprints are only ever compared within the same
/// install to detect "code moved after evidence was captured".
fn fingerprint(root: &Path) -> Value {
    let commit = git(root, &["rev-parse", "HEAD"]);
    let dirty_hash = git(root, &["status", "--porcelain"])
        .filter(|s| !s.is_empty())
        .map(|status| {
            use std::hash::{Hash, Hasher};
            let diff = git(root, &["diff"]).unwrap_or_default();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            status.hash(&mut h);
            diff.hash(&mut h);
            format!("{:016x}", h.finish())
        });
    json!({"commit": commit, "dirty_hash": dirty_hash})
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
