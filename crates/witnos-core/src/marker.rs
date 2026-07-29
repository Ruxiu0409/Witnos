//! The armed marker's shape, parsing, and resolution rules — shared by the
//! core (which derives and writes `<project>/.witnos/armed.json`) and the
//! hook bin (which reads it). Pure data + logic; file I/O stays with the
//! writers/readers so this module has no opinion about paths.
//!
//! v2 shape (auto mode, one goal per agent session):
//! `{v, auto, default_goal?, sessions: {<session_id>: {goal_id, …}}}`.
//! The legacy v1 shape (`{goal_id, contract_version, agent_synced_version}`)
//! is still parsed and normalized — old markers on disk keep arming.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::{Goal, GoalId, SessionId, Version};

/// One goal's coordinates inside the marker. The mirrored versions let the
/// delivery channel run its "contract unchanged?" check with zero network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRef {
    pub goal_id: GoalId,
    pub contract_version: Version,
    #[serde(default)]
    pub agent_synced_version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmedMarker {
    #[serde(default = "marker_version")]
    pub v: u32,
    /// Auto mode: new agent sessions get a goal created from their first
    /// prompt. An auto marker with zero goals still ARMS the gate.
    #[serde(default)]
    pub auto: bool,
    /// The manually-watched goal (`witnos goal new` / legacy markers).
    /// Sessions without their own entry gate against this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_goal: Option<GoalRef>,
    /// Per-session goals (auto mode; also mirrors sessions bound to manual
    /// goals so every hook resolves through the same table).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sessions: BTreeMap<SessionId, GoalRef>,
}

fn marker_version() -> u32 {
    2
}

/// What the marker says about one hook invocation's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// Gate/deliver/instruct against this goal.
    Entry(&'a GoalRef),
    /// Auto project, session not (yet) bound to a goal: the UPS hook should
    /// create one; the Stop gate must NOT silently allow.
    NoGoalAuto,
    /// Manual marker that names no resolvable goal (hand-damaged) — the gate
    /// still arms on presence; delivery/instruction stay silent.
    NoGoalManual,
}

impl ArmedMarker {
    /// Parse either shape. `None` means the content is unusable — callers on
    /// the gate path must still treat the file's PRESENCE as armed.
    pub fn parse(raw: &str) -> Option<ArmedMarker> {
        let val: serde_json::Value = serde_json::from_str(raw).ok()?;
        if val.get("goal_id").is_some() {
            let legacy: GoalRef = serde_json::from_value(val).ok()?;
            return Some(ArmedMarker {
                v: 2,
                auto: false,
                default_goal: Some(legacy),
                sessions: BTreeMap::new(),
            });
        }
        serde_json::from_value(val).ok()
    }

    pub fn to_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("marker serializes")
    }

    pub fn resolve(&self, session_id: Option<&str>) -> Resolution<'_> {
        if let Some(entry) = session_id.and_then(|sid| self.sessions.get(sid)) {
            return Resolution::Entry(entry);
        }
        if let Some(default) = &self.default_goal {
            return Resolution::Entry(default);
        }
        if self.auto {
            Resolution::NoGoalAuto
        } else {
            Resolution::NoGoalManual
        }
    }
}

/// Derive a project dir's marker from ALL of its goals — the marker file is
/// always a pure function of (registry, store), never incrementally patched,
/// so two goals sharing a dir can no longer clobber each other's entries.
///
/// `None` means "no marker should exist" (manual project with nothing
/// watching). An auto project always keeps a marker, even with zero goals:
/// the human opted the directory in, so the gate stays fail-closed there.
pub fn compute(auto: bool, goals: &[Goal]) -> Option<ArmedMarker> {
    let mut watching: Vec<&Goal> = goals.iter().filter(|g| g.watching).collect();
    watching.sort_by_key(|g| (g.created_at, g.id.clone()));

    let default_goal = watching
        .iter()
        .rfind(|g| g.auto_session.is_none())
        .map(|g| goal_ref(g));

    let mut sessions: BTreeMap<SessionId, GoalRef> = BTreeMap::new();
    let mut owned: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for goal in &watching {
        for binding in &goal.sessions {
            let sid = binding.session_id.as_str();
            // The session's own auto goal always wins its slot; an
            // opportunistic bind to another goal must not shadow it.
            if goal.auto_session.as_deref() == Some(sid) {
                sessions.insert(sid.to_string(), goal_ref(goal));
                owned.insert(sid);
            } else if !owned.contains(sid) {
                sessions.insert(sid.to_string(), goal_ref(goal));
            }
        }
    }

    if !auto && default_goal.is_none() && sessions.is_empty() {
        return None;
    }
    Some(ArmedMarker {
        v: 2,
        auto,
        default_goal,
        sessions,
    })
}

fn goal_ref(goal: &Goal) -> GoalRef {
    GoalRef {
        goal_id: goal.id.clone(),
        contract_version: goal.contract_version,
        agent_synced_version: goal.agent_synced_version,
    }
}
