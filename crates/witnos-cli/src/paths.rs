use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The armed marker, written into a watched project by the core when it
/// starts watching a goal, mirrored on every contract bump, removed on
/// graceful stop. Its PRESENCE is what arms the fail-closed gate.
pub const ARMED_REL: &str = ".witnos/armed.json";
pub const DELIVERED_REL: &str = ".witnos/delivered.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub goal_id: String,
    pub contract_version: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

pub fn witnos_home() -> PathBuf {
    if let Ok(h) = std::env::var("WITNOS_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".witnos")
}

/// Walk up from `start` looking for the armed marker (like git does for
/// `.git`). Returns (project_root, marker_path).
pub fn find_marker(start: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join(ARMED_REL);
        if candidate.exists() {
            return Some((d.clone(), candidate));
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

pub fn read_endpoint() -> Result<Endpoint, String> {
    let p = witnos_home().join("endpoint.json");
    let s = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("malformed endpoint.json: {e}"))
}
