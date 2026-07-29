//! `witnos init` — install the four command hooks into the PROJECT-LEVEL
//! `.claude/settings.json` (not user-global: fail-closed must only ever arm
//! projects that opted in). Idempotent merge: existing settings and foreign
//! hooks are preserved; our entries are updated in place if the binary moved.

use std::process::ExitCode;

use serde_json::{json, Value};

const HOOKS: [(&str, Option<&str>, &str); 4] = [
    ("Stop", None, "stop"),
    ("PostToolUse", Some("*"), "post-tool-use"),
    ("UserPromptSubmit", None, "user-prompt-submit"),
    ("SessionEnd", None, "session-end"),
];

pub fn run() -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("witnos: cannot determine cwd: {e}");
            return ExitCode::FAILURE;
        }
    };
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("witnos: cannot resolve own binary path: {e}");
            return ExitCode::FAILURE;
        }
    };

    let settings_path = cwd.join(".claude").join("settings.json");
    let mut root: Value = match std::fs::read_to_string(&settings_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "witnos: {} exists but is not valid JSON ({e}); fix it first",
                    settings_path.display()
                );
                return ExitCode::FAILURE;
            }
        },
        Err(_) => json!({}),
    };

    let mut changed = Vec::new();
    for (event, matcher, sub) in HOOKS {
        let cmd = format!("\"{}\" hook {sub}", exe.display());
        if ensure_hook(&mut root, event, matcher, sub, &cmd) {
            changed.push(event);
        }
    }

    if changed.is_empty() {
        println!("witnos hooks: already installed");
    } else {
        let parent = settings_path.parent().expect("settings path has a parent");
        if let Err(e) = std::fs::create_dir_all(parent).and_then(|_| {
            std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&root).expect("settings serialize"),
            )
        }) {
            eprintln!("witnos: cannot write {}: {e}", settings_path.display());
            return ExitCode::FAILURE;
        }
        println!("witnos hooks: installed {}", changed.join(", "));
    }
    // Short lines on purpose: this renders in narrow embedded terminals.
    println!("  file: {}", settings_path.display());
    println!();
    println!("  - trust this folder in Claude Code (/hooks to approve)");
    println!("  - watch:  Witnos app → \"watch a project (auto)\"");
    println!("  - manual: witnos goal new \"<title>\"");
    ExitCode::SUCCESS
}

/// Returns true if the settings changed (added or updated).
fn ensure_hook(root: &mut Value, event: &str, matcher: Option<&str>, sub: &str, cmd: &str) -> bool {
    if !root.is_object() {
        *root = json!({});
    }
    let hooks = root
        .as_object_mut()
        .expect("root is object")
        .entry("hooks")
        .or_insert(json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let arr = hooks
        .as_object_mut()
        .expect("hooks is object")
        .entry(event)
        .or_insert(json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let list = arr.as_array_mut().expect("event entry is array");

    // Already installed? Update in place if the binary path changed.
    let suffix = format!("hook {sub}");
    for group in list.iter_mut() {
        if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for h in hs.iter_mut() {
                let Some(c) = h.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if c.contains("witnos") && c.trim_end().ends_with(&suffix) {
                    if c == cmd {
                        return false;
                    }
                    h["command"] = json!(cmd);
                    return true;
                }
            }
        }
    }

    let mut group = json!({"hooks": [{"type": "command", "command": cmd}]});
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    list.push(group);
    true
}
