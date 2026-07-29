//! HTTP client context for the agent-facing subcommands: goal identity comes
//! from `--goal <id>` (in-context — the protocol message and every delta
//! carry it) or, unambiguously, from the armed marker; endpoint + token from
//! `$WITNOS_HOME/endpoint.json`. Prompts never touch credentials — this bin
//! encapsulates them.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::paths::{self, Endpoint};

pub struct Ctx {
    pub root: PathBuf,
    pub goal_id: String,
    pub ep: Endpoint,
}

pub fn ctx(goal_override: Option<&str>) -> Result<Ctx, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot determine cwd: {e}"))?;
    let ep = paths::read_endpoint()?;
    let found = paths::find_marker(&cwd);
    let root = found
        .as_ref()
        .map(|(r, _)| r.clone())
        .unwrap_or_else(|| cwd.clone());

    if let Some(id) = goal_override {
        return Ok(Ctx {
            root,
            goal_id: id.to_string(),
            ep,
        });
    }

    let (_, marker_path) = found.ok_or_else(|| {
        "this project is not being watched by Witnos (no .witnos/armed.json found); \
         ask the user to start watching a goal here first, or pass --goal <id>"
            .to_string()
    })?;
    let marker = paths::read_marker(&marker_path)
        .ok_or_else(|| format!("armed marker at {} is unreadable", marker_path.display()))?;

    // Without --goal the marker must resolve to exactly ONE goal — with
    // several concurrent session-goals, a Bash call carries no session
    // identity, so ambient resolution would be a cross-session bug.
    let mut ids: BTreeSet<String> = marker
        .sessions
        .values()
        .map(|g| g.goal_id.clone())
        .collect();
    if let Some(d) = &marker.default_goal {
        ids.insert(d.goal_id.clone());
    }
    match ids.len() {
        1 => Ok(Ctx {
            root,
            goal_id: ids.into_iter().next().expect("one id"),
            ep,
        }),
        0 => Err(
            "no goal is bound in this project yet; pass --goal <id> (it is in your \
             [witnos] protocol message)"
                .to_string(),
        ),
        _ => Err(format!(
            "multiple goals are active in this project ({}); pass --goal <id> — yours \
             is in your [witnos] protocol message",
            ids.into_iter().collect::<Vec<_>>().join(", ")
        )),
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(1500))
        .timeout(Duration::from_secs(5))
        .build()
}

impl Ctx {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.ep.port, path)
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.ep.token)
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        finish(
            agent()
                .get(&self.url(path))
                .set("Authorization", &self.bearer())
                .call(),
        )
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        finish(
            agent()
                .post(&self.url(path))
                .set("Authorization", &self.bearer())
                .send_json(body),
        )
    }
}

/// For commands that must work without an armed marker (e.g. `goal new`).
pub fn post_raw(ep: &Endpoint, path: &str, body: Value) -> Result<Value, String> {
    finish(
        agent()
            .post(&format!("http://127.0.0.1:{}{}", ep.port, path))
            .set("Authorization", &format!("Bearer {}", ep.token))
            .send_json(body),
    )
}

fn finish(resp: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    match resp {
        Ok(r) => r
            .into_json()
            .map_err(|e| format!("malformed response from core: {e}")),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r
                .into_json::<Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| "no detail".to_string());
            Err(format!("core rejected the request ({code}): {msg}"))
        }
        Err(e) => Err(format!("witnos core unreachable: {e}")),
    }
}
