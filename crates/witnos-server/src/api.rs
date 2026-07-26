//! Route handlers. All domain rules live in witnos-core's store; this layer
//! only translates HTTP ⇄ store calls, renders block reasons / deltas, and
//! mirrors the armed marker after contract bumps.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use witnos_core::{
    evaluate, Actor, Class, EventKind, GateDecisionKind, Goal, Item, NewEvidence, NewItem,
    Pointer, StoreError, Version,
};

use crate::{marker, AppState};

fn err(e: StoreError) -> Response {
    let code = match &e {
        StoreError::GoalNotFound(_) | StoreError::ItemNotFound(_) => StatusCode::NOT_FOUND,
        StoreError::Invalid(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, Json(json!({"error": e.to_string()}))).into_response()
}

// A ~128-byte Err on a local single-user server is noise, not a cost.
#[allow(clippy::result_large_err)]
fn goal_or_404(state: &AppState, id: &str) -> Result<Goal, Response> {
    state
        .store
        .get_goal(id)
        .ok_or_else(|| err(StoreError::GoalNotFound(id.to_string())))
}

pub async fn health() -> Json<Value> {
    Json(json!({"ok": true}))
}

// ---------- gate ----------

#[derive(Deserialize)]
pub struct GateReq {
    pub goal_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
}

pub async fn gate(State(state): State<Arc<AppState>>, Json(req): Json<GateReq>) -> Response {
    let goal = match goal_or_404(&state, &req.goal_id) {
        Ok(g) => g,
        Err(r) => return r,
    };
    if let Some(sid) = &req.session_id {
        let _ = state.store.bind_session(&goal.id, "claude-code", sid);
    }
    let outcome = evaluate(&goal);
    if outcome.release {
        if let Err(e) = state
            .store
            .record_gate_decision(&goal.id, GateDecisionKind::Release, None)
        {
            return err(e);
        }
        Json(json!({"decision": "release"})).into_response()
    } else {
        let mut reason = block_reason(&goal, &outcome.reasons);
        // Claude Code caps consecutive Stop blocks (measured: 8 on 2.1.220,
        // see spike/hooks-2026-07-26). Near the cap, tell the agent to save
        // its state so "turn ended, release condition unmet" loses nothing.
        let trailing_blocks = goal
            .events
            .iter()
            .rev()
            .take_while(|e| {
                matches!(
                    e.kind,
                    EventKind::GateDecision {
                        decision: GateDecisionKind::Block,
                        ..
                    }
                )
            })
            .count();
        if req.stop_hook_active.unwrap_or(false) && trailing_blocks >= 5 {
            reason.push_str(
                "\nNOTE: the harness caps consecutive stop-blocks and may end this turn soon. \
                 FIRST persist what is already done (`witnos evidence add`, `witnos reconcile`), \
                 so the next turn resumes instead of redoing.",
            );
        }
        let _ = state
            .store
            .record_gate_decision(&goal.id, GateDecisionKind::Block, Some(reason.clone()));
        Json(json!({"decision": "block", "reason": reason})).into_response()
    }
}

fn block_reason(goal: &Goal, reasons: &[String]) -> String {
    format!(
        "[witnos] The verification contract (v{}) is not met:\n- {}\n\
         Fetch the latest contract with `witnos contract show --since {}`; lay interpretations with \
         `witnos item interpret <item-id> <text>`; attach evidence with `witnos evidence add <item-id>` \
         (JSON on stdin: {{conclusion, basis, provenance:[{{kind:\"file\"|\"command\"|\"url\", …}}]}}); \
         report oracle runs with `witnos oracle report <item-id> --passed|--failed`; \
         then declare alignment with `witnos reconcile --to {}`.",
        goal.contract_version,
        reasons.join("\n- "),
        goal.agent_synced_version,
        goal.contract_version,
    )
}

// ---------- goals ----------

#[derive(Deserialize)]
pub struct CreateGoalReq {
    pub title: String,
}

pub async fn create_goal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateGoalReq>,
) -> Response {
    match state.store.create_goal(&req.title) {
        Ok(g) => Json(serde_json::to_value(&g).expect("goal serializes")).into_response(),
        Err(e) => err(e),
    }
}

pub async fn list_goals(State(state): State<Arc<AppState>>) -> Response {
    let mut out = Vec::new();
    for id in state.store.goal_ids() {
        if let Some(g) = state.store.get_goal(&id) {
            out.push(json!({
                "id": g.id,
                "title": g.title,
                "status": g.status,
                "contract_version": g.contract_version,
                "watching": g.watching,
                "strong_bet_count": g.strong_bet_count(),
            }));
        }
    }
    Json(Value::Array(out)).into_response()
}

pub async fn get_goal(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match goal_or_404(&state, &id) {
        Ok(g) => Json(serde_json::to_value(&g).expect("goal serializes")).into_response(),
        Err(r) => r,
    }
}

// ---------- watch (arm/disarm) ----------

#[derive(Deserialize)]
pub struct WatchReq {
    pub project_dir: String,
}

pub async fn watch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<WatchReq>,
) -> Response {
    match state.store.set_watch(&id, Some(req.project_dir), true) {
        Ok(goal) => {
            marker::sync(&goal);
            Json(json!({"watching": true, "contract_version": goal.contract_version}))
                .into_response()
        }
        Err(e) => err(e),
    }
}

pub async fn unwatch(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.set_watch(&id, None, false) {
        Ok(goal) => {
            marker::remove(&goal);
            Json(json!({"watching": false})).into_response()
        }
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct BindReq {
    pub session_id: String,
    #[serde(default = "default_agent")]
    pub agent: String,
}

fn default_agent() -> String {
    "unknown".to_string()
}

pub async fn bind_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<BindReq>,
) -> Response {
    match state.store.bind_session(&id, &req.agent, &req.session_id) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

// ---------- contract ----------

pub async fn contract(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let goal = match goal_or_404(&state, &id) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let since: Version = q.get("since").and_then(|v| v.parse().ok()).unwrap_or(0);
    let items: Vec<Value> = goal.items_since(since).iter().map(|i| brief(i)).collect();
    Json(json!({
        "version": goal.contract_version,
        "agent_synced_version": goal.agent_synced_version,
        "summary": render_delta(&goal, since),
        "items": items,
    }))
    .into_response()
}

fn class_word(c: &Class) -> &'static str {
    match c {
        Class::Subjective => "subjective",
        Class::Objective { .. } => "objective",
    }
}

fn brief(i: &Item) -> Value {
    json!({
        "id": i.id,
        "claim": i.claim,
        "check": i.check,
        "class": class_word(&i.class),
        "status": i.status,
        "interpretation": i.interpretation,
        "last_edited_version": i.last_edited_version,
    })
}

/// Always a delta, never the full list (unless asked from v0).
fn render_delta(goal: &Goal, since: Version) -> String {
    let items = goal.items_since(since);
    if items.is_empty() {
        return "(no items changed)".to_string();
    }
    items
        .iter()
        .map(|i| {
            format!(
                "- [{}] \"{}\" — check: {} (id: {})",
                class_word(&i.class),
                i.claim,
                i.check,
                i.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- items / evidence / rulings ----------

#[derive(Deserialize)]
pub struct LayReq {
    pub actor: Actor,
    pub items: Vec<NewItem>,
}

pub async fn lay_items(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<LayReq>,
) -> Response {
    match state.store.lay_items(&id, req.items, req.actor) {
        Ok(ids) => match state.store.get_goal(&id) {
            Some(goal) => {
                marker::sync(&goal);
                Json(json!({"ids": ids, "version": goal.contract_version})).into_response()
            }
            None => err(StoreError::GoalNotFound(id)),
        },
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct EditReq {
    pub actor: Actor,
    #[serde(default)]
    pub claim: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub class: Option<Class>,
}

pub async fn edit_item(
    State(state): State<Arc<AppState>>,
    Path((id, item_id)): Path<(String, String)>,
    Json(req): Json<EditReq>,
) -> Response {
    match state
        .store
        .edit_item(&id, &item_id, req.claim, req.check, req.class, req.actor)
    {
        Ok(()) => match state.store.get_goal(&id) {
            Some(goal) => {
                marker::sync(&goal);
                Json(json!({"version": goal.contract_version})).into_response()
            }
            None => err(StoreError::GoalNotFound(id)),
        },
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct InterpretReq {
    pub item_id: String,
    pub text: String,
}

pub async fn interpret(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<InterpretReq>,
) -> Response {
    match state.store.set_interpretation(&id, &req.item_id, &req.text) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct EvidenceReq {
    pub item_id: String,
    #[serde(flatten)]
    pub evidence: NewEvidence,
}

pub async fn add_evidence(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<EvidenceReq>,
) -> Response {
    match state.store.add_evidence(&id, &req.item_id, req.evidence) {
        Ok(evidence_id) => Json(json!({"evidence_id": evidence_id})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct OracleReq {
    pub item_id: String,
    pub passed: bool,
}

pub async fn report_oracle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<OracleReq>,
) -> Response {
    match state.store.report_oracle(&id, &req.item_id, req.passed) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct ReconcileReq {
    #[serde(default)]
    pub session_id: Option<String>,
    pub to_version: Version,
    #[serde(default)]
    pub reinterpreted_items: Vec<String>,
}

pub async fn reconcile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ReconcileReq>,
) -> Response {
    let session = req.session_id.unwrap_or_else(|| "agent-cli".to_string());
    match state
        .store
        .reconcile(&id, &session, req.to_version, req.reinterpreted_items)
    {
        Ok(()) => {
            if let Some(goal) = state.store.get_goal(&id) {
                marker::sync(&goal); // keep the delivery baseline current
            }
            Json(json!({"agent_synced_version": req.to_version})).into_response()
        }
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct RuleReq {
    pub item_id: String,
    pub approve: bool,
    #[serde(default)]
    pub after_drill_down: bool,
}

/// Human-only by contract. The HTTP layer cannot distinguish callers yet
/// (one shared local token); the agent-facing CLI simply does not expose
/// this. Proper separation lands when the UI moves in-process (Tauri).
pub async fn rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<RuleReq>,
) -> Response {
    match state
        .store
        .rule_item(&id, &req.item_id, req.approve, req.after_drill_down)
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
pub struct DrillDownReq {
    pub evidence_id: String,
    pub pointer: Pointer,
}

pub async fn drill_down(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<DrillDownReq>,
) -> Response {
    match state
        .store
        .record_drill_down(&id, &req.evidence_id, req.pointer)
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}
