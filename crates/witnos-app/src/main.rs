//! The Witnos GUI core: a Tauri shell that spawns the axum server on startup
//! and exposes the HUMAN side of the store over IPC. The trust split is
//! structural: the webview (human) uses these in-process commands — including
//! `rule_item`, which the HTTP surface will never let an agent reach cleanly —
//! while agents go through the `witnos` CLI over HTTP.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod terminal;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{Manager, State};
use witnos_core::{Actor, Goal, GoalStatus, NewItem, Origin, Pointer};
use witnos_server::AppState;

struct App(Arc<AppState>);

fn witnos_home() -> PathBuf {
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

#[tauri::command]
fn edit_item(
    state: State<'_, App>,
    goal_id: String,
    item_id: String,
    claim: String,
    check: String,
) -> Result<Value, String> {
    state
        .0
        .store
        .edit_item(&goal_id, &item_id, Some(claim), Some(check), None, Actor::Human)
        .map_err(|e| e.to_string())?;
    resync(&state, &goal_id);
    Ok(json!({"ok": true}))
}

#[tauri::command]
fn rule_item(
    state: State<'_, App>,
    goal_id: String,
    item_id: String,
    approve: bool,
    after_drill_down: bool,
) -> Result<Value, String> {
    state
        .0
        .store
        .rule_item(&goal_id, &item_id, approve, after_drill_down)
        .map_err(|e| e.to_string())?;
    Ok(json!({"ok": true}))
}

/// Record the drill-down (that log is the requirements spec for the future
/// raw-trace layer), then open the original behind the pointer.
#[tauri::command]
fn drill_down(
    state: State<'_, App>,
    goal_id: String,
    evidence_id: String,
    pointer: Pointer,
) -> Result<Value, String> {
    state
        .0
        .store
        .record_drill_down(&goal_id, &evidence_id, pointer.clone())
        .map_err(|e| e.to_string())?;
    if let Some(goal) = state.0.store.get_goal(&goal_id) {
        open_original(&goal, &pointer);
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

#[tauri::command]
fn unwatch_goal(state: State<'_, App>, goal_id: String) -> Result<Value, String> {
    let goal = state
        .0
        .store
        .set_watch(&goal_id, None, false)
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

fn open_original(goal: &Goal, pointer: &Pointer) {
    let target = match pointer {
        Pointer::File { path, .. } => {
            let p = PathBuf::from(path);
            let p = if p.is_relative() {
                goal.project_dir
                    .as_deref()
                    .map(|d| PathBuf::from(d).join(&p))
                    .unwrap_or(p)
            } else {
                p
            };
            p.to_string_lossy().into_owned()
        }
        Pointer::Url { url } => url.clone(),
        Pointer::Command { .. } => return, // nothing to open; the recorded event is the point
    };
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&target).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &target])
            .spawn();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle =
                tauri::async_runtime::block_on(witnos_server::start(&witnos_home()))
                    .map_err(|e| e as Box<dyn std::error::Error>)?;
            app.manage(App(handle.state.clone()));
            app.manage(terminal::Terminals::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_goals,
            get_goal,
            create_goal,
            add_item,
            edit_item,
            rule_item,
            drill_down,
            close_goal,
            delete_goal,
            unwatch_goal,
            pick_project_dir,
            add_auto_project,
            remove_auto_project,
            list_auto_projects,
            terminal::term_spawn,
            terminal::term_write,
            terminal::term_resize,
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
