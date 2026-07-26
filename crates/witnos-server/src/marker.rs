//! The armed-marker side of the arm/disarm protocol. The core writes the
//! marker when it starts watching, mirrors the contract version into it on
//! every bump (so the delivery channel's "unchanged" check touches no
//! network), and removes it on graceful stop.

use std::path::Path;

use serde_json::json;
use witnos_core::Goal;

pub fn sync(goal: &Goal) {
    let Some(dir) = goal.project_dir.as_deref().filter(|_| goal.watching) else {
        return;
    };
    let witnos_dir = Path::new(dir).join(".witnos");
    if std::fs::create_dir_all(&witnos_dir).is_ok() {
        let _ = std::fs::write(
            witnos_dir.join("armed.json"),
            serde_json::to_string_pretty(&json!({
                "goal_id": goal.id,
                "contract_version": goal.contract_version,
                "agent_synced_version": goal.agent_synced_version,
            }))
            .expect("marker serializes"),
        );
    }
}

pub fn remove(goal: &Goal) {
    if let Some(dir) = goal.project_dir.as_deref() {
        let _ = std::fs::remove_file(Path::new(dir).join(".witnos").join("armed.json"));
    }
}
