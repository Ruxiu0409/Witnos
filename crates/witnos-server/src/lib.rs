//! witnos-server — the core's HTTP surface. Bound to 127.0.0.1 on an
//! ephemeral port; `{port, token}` written to `<home>/endpoint.json` (0600).
//! The GUI core and the gate hit the same in-process store, so what the
//! human edits IS what the gate reads each round.

mod api;
mod marker;

pub use marker::{remove as remove_marker, sync as sync_marker};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use witnos_core::Store;

pub struct AppState {
    pub store: Store,
    pub token: String,
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
        .route("/goals/{id}", get(api::get_goal))
        .route("/goals/{id}/watch", post(api::watch).delete(api::unwatch))
        .route("/goals/{id}/sessions", post(api::bind_session))
        .route("/goals/{id}/contract", get(api::contract))
        .route("/goals/{id}/items", post(api::lay_items))
        .route("/goals/{id}/items/{item_id}/edit", post(api::edit_item))
        .route("/goals/{id}/interpret", post(api::interpret))
        .route("/goals/{id}/evidence", post(api::add_evidence))
        .route("/goals/{id}/oracle", post(api::report_oracle))
        .route("/goals/{id}/reconcile", post(api::reconcile))
        .route("/goals/{id}/rulings", post(api::rule))
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
    });

    // Re-arm watched goals: an app restart must restore their markers
    // (a crash left them in place — correctly — and a graceful stop removed
    // them while keeping `watching` true).
    for id in state.store.goal_ids() {
        if let Some(goal) = state.store.get_goal(&id) {
            marker::sync(&goal);
        }
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
/// is deliberately closed) but keep `watching: true` in the store, so the
/// next start re-arms them. An app CRASH never reaches this — the marker
/// stays and the gate stalls, which is the designed fail-closed behavior.
pub fn graceful_stop(state: &AppState) {
    for id in state.store.goal_ids() {
        if let Some(goal) = state.store.get_goal(&id) {
            if goal.watching {
                marker::remove(&goal);
            }
        }
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
