//! The armed-marker side of the arm/disarm protocol. The core derives each
//! watched project's marker from ALL of its goals (shape + rules live in
//! `witnos_core::marker`), writes it on watch/bump/reconcile, and removes it
//! on graceful stop. tmp+rename so hooks never read a torn multi-entry file.

use std::path::Path;

use witnos_core::Goal;

/// Recompute and write (or remove) one project dir's marker. The file is a
/// pure function of (auto?, goals-of-dir) — never incrementally patched, so
/// goals sharing a dir cannot clobber each other's entries.
pub fn sync_dir(dir: &str, auto: bool, goals: &[Goal]) {
    let witnos_dir = Path::new(dir).join(".witnos");
    let dst = witnos_dir.join("armed.json");
    match witnos_core::marker::compute(auto, goals) {
        Some(marker) => {
            if std::fs::create_dir_all(&witnos_dir).is_ok() {
                let tmp = witnos_dir.join("armed.json.tmp");
                if std::fs::write(&tmp, marker.to_pretty()).is_ok() {
                    let _ = std::fs::rename(&tmp, &dst);
                }
            }
        }
        None => {
            let _ = std::fs::remove_file(&dst);
        }
    }
}

/// Graceful-stop removal: no derivation, just take the marker down.
pub fn remove_for_dir(dir: &str) {
    let _ = std::fs::remove_file(Path::new(dir).join(".witnos").join("armed.json"));
}
