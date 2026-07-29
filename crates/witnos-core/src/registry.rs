//! The auto-watch project registry: which directories are in auto mode
//! (every new agent session there gets a goal created from its first
//! prompt). Persisted at `<WITNOS_HOME>/projects.json`.
//!
//! Human-only surface: mutated via the GUI's IPC commands, never exposed
//! over HTTP — an agent must not be able to opt a directory into or out of
//! being watched.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileShape {
    v: u32,
    projects: BTreeSet<String>,
}

pub struct ProjectRegistry {
    path: PathBuf,
    dirs: RwLock<BTreeSet<String>>,
}

impl ProjectRegistry {
    /// Missing or unreadable file loads as empty — the registry only ever
    /// gains meaning through explicit human adds.
    pub fn load(home: &Path) -> Self {
        let path = home.join("projects.json");
        let dirs = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<FileShape>(&s).ok())
            .map(|f| f.projects)
            .unwrap_or_default();
        ProjectRegistry {
            path,
            dirs: RwLock::new(dirs),
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.read().iter().cloned().collect()
    }

    pub fn contains(&self, dir: &str) -> bool {
        self.read().contains(&canon(dir))
    }

    /// Returns true if the directory was newly added.
    pub fn add(&self, dir: &str) -> std::io::Result<bool> {
        let mut dirs = self.write();
        if !dirs.insert(canon(dir)) {
            return Ok(false);
        }
        persist(&self.path, &dirs)?;
        Ok(true)
    }

    /// Returns true if the directory was present.
    pub fn remove(&self, dir: &str) -> std::io::Result<bool> {
        let mut dirs = self.write();
        if !dirs.remove(&canon(dir)) {
            return Ok(false);
        }
        persist(&self.path, &dirs)?;
        Ok(true)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeSet<String>> {
        self.dirs.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeSet<String>> {
        self.dirs.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Registry entries are canonicalized so "add via picker" and "look up via
/// a goal's project_dir" agree on symlinked/`..`-laden paths. A path that no
/// longer resolves keeps its literal form (still removable).
fn canon(dir: &str) -> String {
    Path::new(dir)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_string())
}

fn persist(path: &Path, dirs: &BTreeSet<String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let shape = FileShape {
        v: 1,
        projects: dirs.clone(),
    };
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&shape).expect("registry serializes"))?;
    fs::rename(&tmp, path)?;
    Ok(())
}
