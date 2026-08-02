//! The Witnos GUI core: a Tauri shell that spawns the axum server on startup
//! and exposes the HUMAN side of the store over IPC. The trust split is
//! structural: the webview (human) uses these in-process commands — including
//! `delete_item`, which the HTTP surface will never let an agent reach cleanly —
//! while agents go through the `witnos` CLI over HTTP.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod terminal;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{Manager, State};
use witnos_core::{Actor, Goal, GoalStatus, ItemEdit, NewItem, Origin, Pointer};
use witnos_server::AppState;

struct App(Arc<AppState>);

pub(crate) fn witnos_home() -> PathBuf {
    if let Ok(h) = std::env::var("WITNOS_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".witnos")
}

fn resync(state: &App, goal_id: &str) {
    if let Some(goal) = state.0.store.get_goal(goal_id) {
        witnos_server::resync_goal_dir(&state.0, &goal);
    }
}

/// The bundled headless CLI: the hooks it installs point at its own
/// absolute path (`current_exe` inside `witnos init`), so resolving it here
/// is the only PATH-free link the app needs.
pub(crate) fn bundled_cli() -> Option<PathBuf> {
    let name = if cfg!(windows) { "witnos.exe" } else { "witnos" };
    if let Ok(p) = std::env::var("WITNOS_CLI_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // Dev loop: cargo puts the witnos bin next to witnos-app in target/.
    let sibling = dir.join(name);
    if sibling.is_file() {
        return Some(sibling);
    }
    // macOS bundle: Contents/MacOS/witnos-app → Contents/Resources/bin/witnos.
    let resource = dir.parent()?.join("Resources").join("bin").join(name);
    if resource.is_file() {
        // Resource copying can strip the exec bit — restore it defensively.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&resource, std::fs::Permissions::from_mode(0o755));
        }
        return Some(resource);
    }
    None
}

#[tauri::command]
fn list_goals(state: State<'_, App>) -> Vec<Value> {
    let store = &state.0.store;
    let mut goals: Vec<Goal> = store
        .goal_ids()
        .into_iter()
        .filter_map(|id| store.get_goal(&id))
        .collect();
    goals.sort_by_key(|g| std::cmp::Reverse(g.created_at));
    goals
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "title": g.title,
                "status": g.status,
                "contract_version": g.contract_version,
                "watching": g.watching,
                "strong_bet_count": g.strong_bet_count(),
                "project_dir": g.project_dir,
            })
        })
        .collect()
}

#[tauri::command]
fn get_goal(state: State<'_, App>, id: String) -> Result<Value, String> {
    let goal = state.0.store.get_goal(&id).ok_or("goal not found")?;
    serde_json::to_value(&goal).map_err(|e| e.to_string())
}

#[tauri::command]
fn create_goal(state: State<'_, App>, title: String) -> Result<Value, String> {
    let goal = state.0.store.create_goal(&title).map_err(|e| e.to_string())?;
    serde_json::to_value(&goal).map_err(|e| e.to_string())
}

/// Adding an item from the UI records its origin honestly — this is the
/// core-bet instrumentation. `viewing_evidence` set = the (b) signal.
#[tauri::command]
fn add_item(
    state: State<'_, App>,
    goal_id: String,
    claim: String,
    check: String,
    viewing_evidence: Option<String>,
) -> Result<Value, String> {
    let goal = state.0.store.get_goal(&goal_id).ok_or("goal not found")?;
    let origin = match viewing_evidence {
        Some(evidence_id) => Origin::UserViewingEvidence { evidence_id },
        None if goal.sessions.is_empty() => Origin::UserPreRun,
        None => Origin::UserMidRun,
    };
    state
        .0
        .store
        .lay_items(
            &goal_id,
            vec![NewItem {
                claim,
                check,
                class: None, // default subjective; promotion is a separate, deliberate act
                interpretation: None,
                origin,
            }],
            Actor::Human,
        )
        .map_err(|e| e.to_string())?;
    resync(&state, &goal_id);
    Ok(json!({"ok": true}))
}

/// Editing is the human's whole disagreement lever (send-back was removed on
/// 2026-08-02): it reopens the item and bumps the contract version, so it
/// reaches a still-running agent through the delivery channel instead of
/// waiting for the gate. `after_drill_down` records whether they opened an
/// evidence original before moving the yardstick — the anti-rubber-stamping
/// signal (principle 6).
#[tauri::command]
fn edit_item(
    state: State<'_, App>,
    goal_id: String,
    item_id: String,
    claim: String,
    check: String,
    after_drill_down: bool,
) -> Result<Value, String> {
    state
        .0
        .store
        .edit_item(
            &goal_id,
            &item_id,
            ItemEdit {
                claim: Some(claim),
                check: Some(check),
                class: None,
            },
            Actor::Human,
            after_drill_down,
        )
        .map_err(|e| e.to_string())?;
    resync(&state, &goal_id); // the marker mirrors the bump for the delivery channel
    Ok(json!({"ok": true}))
}

/// Per-item opt-out: "I don't want this one checked". Takes the item out of the
/// contract along with its evidence — irreversible, which is why the UI asks
/// twice. Human-only, exactly like editing: an agent must never be able to
/// excuse itself from a check, let alone delete one.
#[tauri::command]
fn delete_item(state: State<'_, App>, goal_id: String, item_id: String) -> Result<Value, String> {
    state
        .0
        .store
        .delete_item(&goal_id, &item_id)
        .map_err(|e| e.to_string())?;
    // Deleting moves the contract version too (an agent still checking a
    // deleted item is wasted work), so the marker has to mirror the bump or the
    // delivery channel's zero-network check would never notice.
    resync(&state, &goal_id);
    Ok(json!({"ok": true}))
}

/// Type a nudge into the pane where this goal's agent session is running, so a
/// contract change reaches it now instead of at its next tool call. Returns
/// `"sent"`, `"no_agent"` (the pane is there but sitting at a bare shell prompt:
/// nothing is running to be prompted, and the text would have been run as a
/// shell command), or `"unbound"` (no session, no pane recorded, or that pane is
/// gone: `/clear` moved the session, or the human closed the terminal).
///
/// Note what is NOT a case here: an agent that is mid-turn. Typing at it is
/// exactly what this is for — the keystrokes land in its input box and Enter
/// queues the message. Only "there is no program there" is a refusal.
#[tauri::command]
fn send_to_agent(
    state: State<'_, App>,
    terminals: State<'_, terminal::Terminals>,
    goal_id: String,
    note: String,
) -> Result<String, String> {
    let goal = state.0.store.get_goal(&goal_id).ok_or("goal not found")?;
    // Newest binding first: after a resume the same goal can carry several
    // sessions, and the live one is the last we heard from.
    let Some(pane) = goal.sessions.iter().rev().find_map(|s| s.pane) else {
        return Ok("unbound".to_string());
    };
    let text = agent_note(
        &goal.id,
        goal.contract_version,
        goal.agent_synced_version,
        &note,
    );
    Ok(match terminal::prompt_pane(&terminals, pane, &text)? {
        Some(true) => "sent",
        Some(false) => "no_agent",
        None => "unbound",
    }
    .to_string())
}

/// The nudge itself. Composed here — one testable place — and deliberately a
/// SINGLE line: it is typed into a shell, where a newline would submit.
/// The commands must stay identical to what the injected protocol and the gate's
/// block reasons tell the agent to run, `--goal` included; three different
/// spellings of the same instruction is how an agent ends up guessing.
fn agent_note(goal_id: &str, contract_version: u64, synced_version: u64, note: &str) -> String {
    let mut text = format!("[witnos] The verification contract moved to v{contract_version}.");
    // The human's own words come before the mechanics: that is the part the
    // agent has to act on, and a long note must not bury it.
    let collapsed = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        text.push_str(&format!(" From the user: {collapsed}"));
    }
    text.push_str(&format!(
        " Run `witnos contract show --goal {goal_id} --since {synced_version}` to see what changed, \
         address it, then `witnos reconcile --goal {goal_id} --to {contract_version}`."
    ));
    text
}

/// Record the drill-down (that log is the requirements spec for the future
/// raw-trace layer), then open the original behind the pointer. `editor` is
/// the UI's "open files with" setting — an editor id from the settings
/// picker, or "system"/absent for the OS default app.
#[tauri::command]
fn drill_down(
    state: State<'_, App>,
    goal_id: String,
    evidence_id: String,
    pointer: Pointer,
    editor: Option<String>,
) -> Result<Value, String> {
    state
        .0
        .store
        .record_drill_down(&goal_id, &evidence_id, pointer.clone())
        .map_err(|e| e.to_string())?;
    let goal = state.0.store.get_goal(&goal_id).ok_or("goal not found")?;
    if let Some(target) = resolve_target(&goal, &pointer)? {
        // Opening blocks for as long as the OS opener runs — xdg-open's
        // generic fallback doesn't return until the browser it started exits —
        // and this command runs on the UI thread.
        std::thread::spawn(move || open_target(&target, editor.as_deref()));
    }
    Ok(json!({"ok": true}))
}

#[tauri::command]
fn close_goal(state: State<'_, App>, goal_id: String) -> Result<Value, String> {
    state
        .0
        .store
        .set_watch(&goal_id, None, false)
        .map_err(|e| e.to_string())?;
    state
        .0
        .store
        .set_status(&goal_id, GoalStatus::Closed)
        .map_err(|e| e.to_string())?;
    resync(&state, &goal_id); // other goals in the same dir keep their marker
    Ok(json!({"ok": true}))
}

/// Deletion is a human act: exposed only over IPC, never on the HTTP
/// surface the agent talks to.
#[tauri::command]
fn delete_goal(state: State<'_, App>, goal_id: String) -> Result<Value, String> {
    let goal = state
        .0
        .store
        .delete_goal(&goal_id)
        .map_err(|e| e.to_string())?;
    witnos_server::resync_goal_dir(&state.0, &goal);
    Ok(json!({"ok": true}))
}

// ---------- auto-watch projects (human-only surface: IPC, never HTTP) ----------

#[tauri::command]
async fn pick_project_dir(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Watch a project in auto mode: install the hooks (via the bundled CLI, so
/// they point at its absolute path), register the dir, arm the marker.
#[tauri::command]
async fn add_auto_project(state: State<'_, App>, dir: String) -> Result<Value, String> {
    let path = PathBuf::from(&dir);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    let cli = bundled_cli()
        .ok_or("witnos CLI not found — reinstall the app (or set WITNOS_CLI_BIN in dev)")?;
    let out = std::process::Command::new(&cli)
        .arg("init")
        .current_dir(&path)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", cli.display()))?;
    if !out.status.success() {
        return Err(format!(
            "witnos init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    witnos_server::register_project(&state.0, &dir).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

/// Stop auto-watching: unregister + unwatch its auto goals (so a restart
/// cannot re-arm them). The installed hooks stay — inert without a marker.
#[tauri::command]
fn remove_auto_project(state: State<'_, App>, dir: String) -> Result<Value, String> {
    witnos_server::unregister_project(&state.0, &dir).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

#[tauri::command]
fn list_auto_projects(state: State<'_, App>) -> Vec<Value> {
    state
        .0
        .registry
        .list()
        .into_iter()
        .map(|dir| {
            let goals = state.0.store.goals_for_dir(&dir);
            json!({
                "dir": dir,
                "goal_count": goals.len(),
                "watching_count": goals.iter().filter(|g| g.watching).count(),
            })
        })
        .collect()
}

/// Where a pointer lands on this machine.
enum Target {
    File { path: String, line: Option<u32> },
    Url(String),
}

/// Resolve a pointer against the goal's workspace; `None` = nothing to open.
/// A relative path with no project directory to anchor it is an error rather
/// than a guess: the bundled app's cwd is `/`, so both the editor URL and the
/// OS-default fallback would resolve it from the filesystem root and the human
/// would be told nothing.
fn resolve_target(goal: &Goal, pointer: &Pointer) -> Result<Option<Target>, String> {
    match pointer {
        Pointer::File { path, lines } => {
            let p = PathBuf::from(path);
            let p = if p.is_relative() {
                let dir = goal.project_dir.as_deref().ok_or_else(|| {
                    format!("cannot open {path}: relative path, and this goal has no project directory to resolve it against")
                })?;
                PathBuf::from(dir).join(&p)
            } else {
                p
            };
            Ok(Some(Target::File {
                path: p.to_string_lossy().into_owned(),
                line: lines.as_deref().and_then(first_number),
            }))
        }
        // URLs always go to whatever owns their scheme (usually the browser).
        Pointer::Url { url } => Ok(Some(Target::Url(url.clone()))),
        Pointer::Command { .. } => Ok(None), // the recorded event is the point
    }
}

/// Open a resolved target with the UI's "open files with" editor. Every branch
/// here can block, so this must not run on the UI thread.
fn open_target(target: &Target, editor: Option<&str>) {
    let (path, line) = match target {
        Target::Url(url) => {
            launch(url);
            return;
        }
        Target::File { path, line } => (path.as_str(), *line),
    };
    match editor {
        // Editors registering a `<scheme>://file/<path>[:line]` URL scheme
        // (VS Code and its forks, Zed). A failed launch means the scheme is
        // unregistered — the editor isn't installed — so fall through to
        // the OS default rather than opening nothing.
        Some(s @ ("vscode" | "cursor" | "windsurf" | "zed")) => {
            let sep = if path.starts_with('/') { "" } else { "/" };
            let mut url = format!("{s}://file{sep}{}", percent_encode(path));
            if let Some(n) = line {
                url.push_str(&format!(":{n}"));
            }
            if launch(&url) {
                return;
            }
        }
        // Xcode has no file-open URL scheme; `xed` ships in /usr/bin — but as
        // an xcode-select shim that spawns fine with no Xcode installed, so
        // only its exit status tells us the file actually opened.
        Some("xcode") => {
            let mut cmd = std::process::Command::new("xed");
            if let Some(n) = line {
                cmd.arg("--line").arg(n.to_string());
            }
            if cmd.arg(path).status().map(|s| s.success()).unwrap_or(false) {
                return;
            }
        }
        _ => {} // "system" or unknown → OS default
    }
    launch(path);
}

/// Hand the target to the OS opener; true = the opener accepted it. The status
/// check is what detects an unregistered editor URL scheme. Blocks until the
/// opener exits.
fn launch(target: &str) -> bool {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(target).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(target).status();
    #[cfg(target_os = "windows")]
    let status = {
        use std::os::windows::process::CommandExt;
        match windows_open_cmdline(target) {
            // raw_arg, not arg: the quoting below has to reach cmd verbatim.
            Some(line) => std::process::Command::new("cmd").raw_arg(line).status(),
            None => return false,
        }
    };
    status.map(|s| s.success()).unwrap_or(false)
}

/// The `cmd` command line that opens `target`, or `None` if it can't be handed
/// over safely. `start` is a cmd builtin, so the target crosses cmd's parser:
/// an agent-written URL carrying `&` or `|` would split the command line and
/// run the tail as its own command. Quoting neutralises that; a target holding
/// a double quote of its own could break back out, so it is refused instead.
#[cfg(any(target_os = "windows", test))]
fn windows_open_cmdline(target: &str) -> Option<String> {
    if target.contains('"') {
        return None;
    }
    Some(format!("/C start \"\" \"{target}\""))
}

/// First run of digits in an evidence `lines` field ("120", "120-140", "L12").
fn first_number(s: &str) -> Option<u32> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    s[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Percent-encode a filesystem path for a `scheme://file/...` URL, keeping
/// `/` so the path stays readable in logs and error messages.
fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let terminals = terminal::Terminals::default();
            // Asked BEFORE the core starts, so the sweep below can follow it
            // immediately: a `running` goal whose pane is gone should be readable
            // by a hook for as short a window as possible. Cheap on purpose — it
            // never starts a daemon, because a daemon that is not running has no
            // surviving panes anyway, which is also the answer on a first launch.
            let surviving = terminals.surviving();
            let handle =
                tauri::async_runtime::block_on(witnos_server::start(&witnos_home()))
                    .map_err(|e| e as Box<dyn std::error::Error>)?;
            // The startup sweep the core used to do unconditionally: account
            // every goal whose agent was running in a pane of ours that is no
            // longer there. Only the app can say which those are — the panes now
            // live in a daemon that outlives it, so "we just started" no longer
            // means "our panes are dead". A terminal layer that cannot answer
            // sweeps nothing at all: unknown must never be reported as ended.
            if let Some(panes) = &surviving {
                let ids: Vec<u32> = panes.iter().map(|p| p.id).collect();
                handle.state.store.account_ended_panes(&ids);
            }
            app.manage(App(handle.state.clone()));
            app.manage(terminals);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_goals,
            get_goal,
            create_goal,
            add_item,
            edit_item,
            delete_item,
            send_to_agent,
            drill_down,
            close_goal,
            delete_goal,
            pick_project_dir,
            add_auto_project,
            remove_auto_project,
            list_auto_projects,
            terminal::term_spawn,
            terminal::term_attach,
            terminal::term_detach,
            terminal::term_list,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_try_cd,
            terminal::term_try_prompt,
            terminal::term_kill
        ])
        .build(tauri::generate_context!())
        .expect("witnos app failed to build")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                witnos_server::graceful_stop(&app.state::<App>().0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{agent_note, first_number, windows_open_cmdline};

    /// The nudge is typed into a shell, so "one line" is a hard property, not a
    /// preference — including when the human's note is a multi-line paste.
    #[test]
    fn the_agent_note_is_one_line_and_names_the_goal() {
        let note = agent_note("g-7", 5, 3, "the palette\nis  still\r\nwrong");
        assert!(
            !note.contains('\n') && !note.contains('\r'),
            "must be a single line: {note:?}"
        );
        assert!(note.contains("From the user: the palette is still wrong"), "{note}");
        assert!(note.contains("moved to v5"), "{note}");
        // Same commands, same flags as the protocol text and the block reasons.
        assert!(note.contains("witnos contract show --goal g-7 --since 3"), "{note}");
        assert!(note.contains("witnos reconcile --goal g-7 --to 5"), "{note}");
    }

    /// An empty note is the common case (the human just edited the contract and
    /// wants the agent to look now) and must not leave a dangling label.
    #[test]
    fn the_agent_note_omits_an_empty_human_note() {
        let note = agent_note("g-7", 2, 0, "   ");
        assert!(!note.contains("From the user"), "{note}");
        assert!(note.contains("witnos contract show --goal g-7 --since 0"), "{note}");
    }

    #[test]
    fn cmd_metacharacters_cannot_leave_the_quoted_target() {
        assert_eq!(
            windows_open_cmdline("https://x.test/run?id=1&calc.exe").unwrap(),
            "/C start \"\" \"https://x.test/run?id=1&calc.exe\""
        );
        assert_eq!(windows_open_cmdline("x\"&calc.exe"), None);
    }

    #[test]
    fn evidence_line_fields_yield_their_first_number() {
        assert_eq!(first_number("120-140"), Some(120));
        assert_eq!(first_number("L12"), Some(12));
        assert_eq!(first_number("none"), None);
    }
}
