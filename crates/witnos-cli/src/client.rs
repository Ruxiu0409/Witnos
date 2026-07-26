//! HTTP client context for the agent-facing subcommands: goal identity comes
//! from the armed marker (walked up from cwd), endpoint + token from
//! `$WITNOS_HOME/endpoint.json`. Prompts never touch credentials — this bin
//! encapsulates them.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::paths::{self, Endpoint, Marker};

pub struct Ctx {
    pub root: PathBuf,
    pub marker: Marker,
    pub ep: Endpoint,
}

pub fn ctx() -> Result<Ctx, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot determine cwd: {e}"))?;
    let (root, marker_path) = paths::find_marker(&cwd).ok_or_else(|| {
        "this project is not being watched by Witnos (no .witnos/armed.json found); \
         ask the user to start watching a goal here first"
            .to_string()
    })?;
    let marker: Marker = std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| format!("armed marker at {} is unreadable", marker_path.display()))?;
    let ep = paths::read_endpoint()?;
    Ok(Ctx { root, marker, ep })
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
