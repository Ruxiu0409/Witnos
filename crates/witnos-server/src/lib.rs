//! witnos-server — the core's HTTP surface. Bound to 127.0.0.1 on an
//! ephemeral port; `{port, token}` written to `<home>/endpoint.json` (0600).
//! The GUI core and the gate hit the same in-process store, so what the
//! human edits IS what the gate reads each round.

mod api;
mod marker;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use witnos_core::{ProjectRegistry, Store};

pub struct AppState {
    pub store: Store,
    pub token: String,
    /// Auto-watch project registry — human-only surface (IPC), never HTTP.
    pub registry: ProjectRegistry,
}

/// Recompute one project dir's armed marker from the registry + store.
/// Every mutation that can change what a hook should see routes through
/// here; per-goal incremental marker writes no longer exist.
pub fn resync_dir(state: &AppState, dir: &str) {
    marker::sync_dir(dir, state.registry.contains(dir), &state.store.goals_for_dir(dir));
}

/// Convenience for callers holding a goal: resync the dir it lives in.
pub fn resync_goal_dir(state: &AppState, goal: &witnos_core::Goal) {
    if let Some(dir) = goal.project_dir.as_deref() {
        resync_dir(state, dir);
    }
}

/// Register a directory for auto mode: every new agent session there gets a
/// goal created from its first prompt. Arms the marker immediately.
pub fn register_project(state: &AppState, dir: &str) -> std::io::Result<bool> {
    let added = state.registry.add(dir)?;
    resync_dir(state, dir);
    Ok(added)
}

/// Unregister a directory from auto mode and unwatch its auto goals (so a
/// restart cannot re-arm them). Manual watched goals in the dir survive.
pub fn unregister_project(state: &AppState, dir: &str) -> std::io::Result<bool> {
    let removed = state.registry.remove(dir)?;
    for goal in state.store.goals_for_dir(dir) {
        if goal.watching && goal.auto_session.is_some() {
            let _ = state.store.set_watch(&goal.id, None, false);
        }
    }
    resync_dir(state, dir);
    Ok(removed)
}

/// Every dir that should carry a marker while the core runs: registered
/// auto projects plus any dir with a watching goal.
fn marker_dirs(state: &AppState) -> BTreeSet<String> {
    let mut dirs: BTreeSet<String> = state.registry.list().into_iter().collect();
    for id in state.store.goal_ids() {
        if let Some(goal) = state.store.get_goal(&id) {
            if goal.watching {
                if let Some(dir) = goal.project_dir {
                    dirs.insert(dir);
                }
            }
        }
    }
    dirs
}

pub struct ServerHandle {
    pub port: u16,
    pub token: String,
    pub state: Arc<AppState>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(api::health))
        .route("/gate", post(api::gate))
        .route("/goals", get(api::list_goals).post(api::create_goal))
        .route("/goals/auto", post(api::create_auto_goal))
        .route("/goals/{id}", get(api::get_goal))
        .route("/goals/{id}/watch", post(api::watch).delete(api::unwatch))
        .route("/goals/{id}/sessions", post(api::bind_session))
        .route("/goals/{id}/turn-ended", post(api::end_turn))
        .route("/goals/{id}/contract", get(api::contract))
        .route("/goals/{id}/items", post(api::lay_items))
        .route("/goals/{id}/items/{item_id}/edit", post(api::edit_item))
        .route("/goals/{id}/interpret", post(api::interpret))
        .route("/goals/{id}/evidence", post(api::add_evidence))
        .route("/goals/{id}/oracle", post(api::report_oracle))
        .route("/goals/{id}/reconcile", post(api::reconcile))
        .route("/goals/{id}/rulings", post(api::reject))
        .route("/goals/{id}/drilldown", post(api::drill_down))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == state.token);
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing or wrong bearer token"})),
        )
            .into_response();
    }
    next.run(req).await
}

/// Start the core: open the store at `<home>/goals`, bind an ephemeral
/// 127.0.0.1 port, write `<home>/endpoint.json` (mode 0600), serve in a
/// background task.
pub async fn start(home: &Path) -> Result<ServerHandle, Box<dyn std::error::Error + Send + Sync>> {
    let store = Store::open(home.join("goals"))?;
    let token = uuid::Uuid::new_v4().to_string();
    let state = Arc::new(AppState {
        store,
        token: token.clone(),
        registry: ProjectRegistry::load(home),
    });

    // Nothing has a pane yet, so every goal whose session ran in one of Witnos's
    // own terminals lost its agent when the previous run ended — account those
    // turns now, before a hook can read a `running` status that would never
    // change again. Sessions with no pane recorded are left alone (see the store
    // method): Witnos didn't spawn those shells and can't know they died.
    state.store.account_ended_panes();

    // Re-arm watched dirs: an app restart must restore their markers
    // (a crash left them in place — correctly — and a graceful stop removed
    // them while keeping `watching` / the registry entry durable).
    for dir in marker_dirs(&state) {
        resync_dir(&state, &dir);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    write_endpoint(home, port, &token)?;

    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Ok(ServerHandle { port, token, state })
}

/// Graceful stop: remove armed markers (so no project stalls while the app
/// is deliberately closed) but keep `watching: true` / the registry entry
/// durable, so the next start re-arms them. An app CRASH never reaches
/// this — the marker stays and the gate stalls, which is the designed
/// fail-closed behavior.
pub fn graceful_stop(state: &AppState) {
    for dir in marker_dirs(state) {
        marker::remove_for_dir(&dir);
    }
}

fn write_endpoint(home: &Path, port: u16, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path: PathBuf = home.join("endpoint.json");
    std::fs::write(&path, json!({"port": port, "token": token}).to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
