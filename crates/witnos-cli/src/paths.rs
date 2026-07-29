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
    let _ = write_atomic_checked(path, content);
}

/// The same write, for the callers that must know whether it landed: the PTY
/// daemon's session-id allocator cannot silently fail to persist, or the next
/// daemon would hand out an id a live pane already answers to.
pub fn write_atomic_checked(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Is this agent session running inside Witnos's own embedded terminal?
///
/// The app stamps `WITNOS_TERMINAL` on every shell it spawns; the agent
/// launched there inherits it, and so does every hook process the agent runs
/// (verified on Claude Code 2.1.220). A session without it was started
/// somewhere else — another terminal app, a CI runner — and Witnos leaves it
/// alone: no goal is created for it and it is never stalled. Exporting the
/// variable by hand is therefore a deliberate opt-in, not a leak.
pub fn in_witnos_terminal() -> bool {
    std::env::var("WITNOS_TERMINAL").is_ok_and(|v| !v.trim().is_empty())
}

/// Which Witnos terminal pane this hook is running under, stamped by the app
/// alongside `WITNOS_TERMINAL` and inherited the same way. The core records it
/// on the session binding so the human can type a correction back into the
/// shell their agent actually sits in.
///
/// Missing or unparseable is `None`, never an error: every caller of this is a
/// hook, and not knowing the pane must never cost anyone a goal.
pub fn witnos_pane() -> Option<u32> {
    std::env::var("WITNOS_PANE")
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Was this session ever handed a goal in this project? The hooks' own book
/// (`.witnos/instructed.json`) is the record, and only key PRESENCE is read
/// here, so both its shapes answer the question.
///
/// The gate needs this to tell two very different situations apart when the
/// marker names no goal for a session: one that never had one (silent failure —
/// exactly what fail-closed exists for) and one whose goal a human took away.
pub fn was_instructed(root: &Path, session: Option<&str>) -> bool {
    let Some(session) = session else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(root.join(INSTRUCTED_REL)) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get(session).cloned())
        .is_some()
}

pub fn read_endpoint() -> Result<Endpoint, String> {
    let p = witnos_home().join("endpoint.json");
    let s = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("malformed endpoint.json: {e}"))
}
