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
    evaluate, Actor, Class, EventKind, GateDecisionKind, Goal, Item, ItemEdit, ItemStatus,
    NewEvidence, NewItem, Pointer, StoreError, Version,
};

use crate::{resync_dir, resync_goal_dir, AppState};

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
    /// Absent in auto mode when the session never got a goal bound — the
    /// gate then resolves by (project_dir, session_id) instead.
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub project_dir: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub stop_hook_active: Option<bool>,
}

pub async fn gate(State(state): State<Arc<AppState>>, Json(req): Json<GateReq>) -> Response {
    let goal = match (&req.goal_id, &req.project_dir, &req.session_id) {
        (Some(id), _, _) => match goal_or_404(&state, id) {
            Ok(g) => g,
            Err(r) => return r,
        },
        (None, Some(dir), Some(sid)) => match state.store.find_session_goal(dir, sid) {
            Some(g) if g.watching => {
                // The hook had no marker entry for a goal that exists and is
                // watched — the marker is stale; heal it for the next round.
                resync_dir(&state, dir);
                g
            }
            // The human deliberately unwatched/closed this session's goal:
            // per-goal opt-out wins over fail-closed (which protects against
            // silent failure, not against deliberate human choice).
            Some(_) => return Json(json!({"decision": "release"})).into_response(),
            None => {
                return Json(json!({"decision": "block", "reason": no_goal_reason()}))
                    .into_response()
            }
        },
        _ => {
            return err(StoreError::Invalid(
                "gate needs goal_id, or project_dir + session_id".into(),
            ))
        }
    };
    if let Some(sid) = &req.session_id {
        // No pane here: the Stop hook's env is the agent's, but the binding
        // hook already recorded it — `None` never erases what we know.
        let _ = state.store.bind_session(&goal.id, "claude-code", sid, None);
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

/// Auto project, session never got a goal. Which is all the core knows: it
/// cannot see why, so this must not name a cause. ("The human deleted this
/// session's goal" looks identical from here and is answered before it reaches
/// the core — the Stop hook releases those.) The reason string is the
/// escape-hatch docs, so a confident wrong diagnosis costs the user the debug.
fn no_goal_reason() -> String {
    "[witnos] This project is auto-watched, but this session never got a goal — most likely \
     Witnos was not running when your prompts were submitted, or the project started being \
     watched after this session did. Tell the user: with the Witnos app open, one more prompt \
     attaches a goal automatically; to stop watching instead, remove the project in the app or \
     run `witnos disarm` in the project root. This stall is fail-closed by design."
        .to_string()
}

fn block_reason(goal: &Goal, reasons: &[String]) -> String {
    format!(
        "[witnos] The verification contract (v{}) is not met:\n- {}\n\
         Fetch the latest contract with `witnos contract show --goal {} --since {}`; lay interpretations with \
         `witnos item interpret <item-id> <text>`; attach evidence with `witnos evidence add <item-id>` \
         (JSON on stdin: {{conclusion, basis, provenance:[{{kind:\"file\"|\"command\"|\"url\", …}}]}}); \
         report oracle runs with `witnos oracle report <item-id> --passed|--failed`; \
         then declare alignment with `witnos reconcile --to {}`. \
         All witnos commands accept `--goal {}`.",
        goal.contract_version,
        reasons.join("\n- "),
        goal.id,
        goal.agent_synced_version,
        goal.contract_version,
        goal.id,
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

#[derive(Deserialize)]
pub struct AutoGoalReq {
    pub title: String,
    pub project_dir: String,
    pub session_id: String,
    #[serde(default = "default_auto_agent")]
    pub agent: String,
    /// The Witnos terminal pane this session's shell runs in — how the human
    /// later gets a correction typed back into the right terminal.
    #[serde(default)]
    pub pane: Option<u32>,
}

fn default_auto_agent() -> String {
    "claude-code".to_string()
}

/// Auto mode: get-or-create the goal owned by one agent session (idempotent
/// via the store's write lock). A goal the human closed/unwatched comes back
/// with `watching: false` and is NOT re-watched — per-goal opt-out wins.
pub async fn create_auto_goal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AutoGoalReq>,
) -> Response {
    match state.store.create_auto_goal(
        &req.title,
        &req.project_dir,
        &req.session_id,
        &req.agent,
        req.pane,
    ) {
        Ok((goal, created)) => {
            resync_dir(&state, &req.project_dir);
            Json(json!({
                "id": goal.id,
                "title": goal.title,
                "created": created,
                "watching": goal.watching,
                "contract_version": goal.contract_version,
                "agent_synced_version": goal.agent_synced_version,
            }))
            .into_response()
        }
        Err(e) => err(e),
    }
}

/// A session ended while this goal was still running — account the turn
/// (SessionEnd hook; bookkeeping, deliberately not a gate).
pub async fn end_turn(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.end_turn(&id) {
        Ok(changed) => Json(json!({"turn_ended": changed})).into_response(),
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
            resync_goal_dir(&state, &goal);
            Json(json!({"watching": true, "contract_version": goal.contract_version}))
                .into_response()
        }
        Err(e) => err(e),
    }
}

pub async fn unwatch(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.store.set_watch(&id, None, false) {
        Ok(goal) => {
            resync_goal_dir(&state, &goal);
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
    /// Which Witnos terminal pane the session runs in, when the hook could
    /// tell (absent for sessions started anywhere else).
    #[serde(default)]
    pub pane: Option<u32>,
}

fn default_agent() -> String {
    "unknown".to_string()
}

pub async fn bind_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<BindReq>,
) -> Response {
    match state
        .store
        .bind_session(&id, &req.agent, &req.session_id, req.pane)
    {
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
        // Claims only: after a deletion the id names nothing, so it would be a
        // handle onto nowhere. This is "stop working on these", not a lookup.
        "removed": goal.deletions_since(since),
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
///
/// Two halves, because the human has two levers. Changed items come from
/// `items_since`; DELETED ones cannot — the row is gone — so they come off the
/// event log, and they have to be here: otherwise the agent spends the rest of
/// its turn on a criterion nobody holds and learns better only by hitting
/// `ItemNotFound`.
fn render_delta(goal: &Goal, since: Version) -> String {
    let mut lines: Vec<String> = goal
        .items_since(since)
        .iter()
        .map(|i| {
            format!(
                "- [{}] \"{}\" — check: {} (id: {}){}",
                class_word(&i.class),
                i.claim,
                i.check,
                i.id,
                delta_note(i),
            )
        })
        .collect();
    lines.extend(goal.deletions_since(since).iter().map(|claim| {
        format!("- REMOVED from the contract: \"{claim}\" — nothing needed from you anymore")
    }));
    if lines.is_empty() {
        return "(no items changed)".to_string();
    }
    lines.join("\n")
}

/// Why an item is in the delta, when the claim text alone wouldn't say. Only a
/// legacy waived item needs one (the opt-out is a deletion since 2026-08-02,
/// and deletions get their own line above): the required action is the opposite
/// of an edit's — stop, rather than address it.
///
/// An ordinary changed item needs no note: it is `Open` again, and "changed,
/// address it" is exactly the right reading. So is a re-saved item, which is how
/// the human disagrees now that send-back is gone: the claim is in the delta,
/// its evidence is stale, and the gate holds until fresh evidence answers the
/// current version.
fn delta_note(item: &Item) -> &'static str {
    match item.status {
        ItemStatus::Waived => "  ← WAIVED by the human: nothing needed from you",
        _ => "",
    }
}

// ---------- items / evidence ----------

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
                resync_goal_dir(&state, &goal);
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
    #[serde(flatten)]
    pub edit: ItemEdit,
}

pub async fn edit_item(
    State(state): State<Arc<AppState>>,
    Path((id, item_id)): Path<(String, String)>,
    Json(req): Json<EditReq>,
) -> Response {
    match state
        .store
        // `after_drill_down` is false over HTTP by construction: this surface is
        // the agent's, and the signal it records is a human opening an evidence
        // original. The human's edits come through IPC, which sets it.
        .edit_item(&id, &item_id, req.edit, req.actor, false)
    {
        Ok(()) => match state.store.get_goal(&id) {
            Some(goal) => {
                resync_goal_dir(&state, &goal);
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
                resync_goal_dir(&state, &goal); // keep the delivery baseline current
            }
            Json(json!({"agent_synced_version": req.to_version})).into_response()
        }
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
