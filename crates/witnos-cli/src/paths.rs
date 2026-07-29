use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The armed marker, written into a watched project by the core when it
/// starts watching, mirrored on every contract bump, removed on graceful
/// stop. Its PRESENCE is what arms the fail-closed gate. Shape + resolution
/// rules are shared with the core via `witnos_core::marker`.
pub use witnos_core::marker::{ArmedMarker, GoalRef, Resolution};

pub const ARMED_REL: &str = ".witnos/armed.json";
pub const DELIVERED_REL: &str = ".witnos/delivered.json";
pub const INSTRUCTED_REL: &str = ".witnos/instructed.json";

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

/// Read + parse the marker. `None` means unreadable/unparseable content —
/// gate-path callers must still treat the file's presence as armed.
pub fn read_marker(marker_path: &Path) -> Option<ArmedMarker> {
    ArmedMarker::parse(&std::fs::read_to_string(marker_path).ok()?)
}

/// tmp+rename — with concurrent sessions the hook-side books (delivered /
/// instructed) get concurrent writers; a torn file would merely re-inject
/// once, but atomicity costs nothing.
pub fn write_atomic(path: &Path, content: &str) {
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, content).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

pub fn read_endpoint() -> Result<Endpoint, String> {
    let p = witnos_home().join("endpoint.json");
    let s = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("malformed endpoint.json: {e}"))
}
