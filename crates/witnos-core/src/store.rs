//! One serde-JSON file per goal behind an `RwLock` — thinnest, most forkable,
//! local-first single-user. The GUI core and the gate hit this same
//! in-process store, so what the human edits IS what the gate reads.
//!
//! Domain rules enforced here at write time (not in the UI, not in prompts):
//! - default subjective; objective is structurally impossible without an oracle
//! - an agent cannot claim a human promoted an item
//! - evidence must carry provenance
//! - agents must not edit human-authored claims (interpret instead)

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use serde::Deserialize;

use crate::types::*;

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    GoalNotFound(GoalId),
    ItemNotFound(ItemId),
    Invalid(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io error: {e}"),
            StoreError::Serde(e) => write!(f, "serialization error: {e}"),
            StoreError::GoalNotFound(id) => write!(f, "goal not found: {id}"),
            StoreError::ItemNotFound(id) => write!(f, "item not found: {id}"),
            StoreError::Invalid(msg) => write!(f, "invalid operation: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e)
    }
}

/// Input for laying a new contract item. `class: None` means subjective —
/// the default-subjective rule is the absence of a stated oracle.
#[derive(Debug, Clone, Deserialize)]
pub struct NewItem {
    pub claim: String,
    pub check: String,
    #[serde(default)]
    pub class: Option<Class>,
    #[serde(default)]
    pub interpretation: Option<String>,
    pub origin: Origin,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewEvidence {
    pub conclusion: String,
    pub basis: String,
    pub provenance: Vec<Pointer>,
    #[serde(default)]
    pub workspace: WorkspaceFingerprint,
}

pub struct Store {
    dir: PathBuf,
    goals: RwLock<HashMap<GoalId, Goal>>,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut goals = HashMap::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let mut goal: Goal = serde_json::from_str(&fs::read_to_string(&path)?)?;
                normalize_parked_status(&mut goal);
                goals.insert(goal.id.clone(), goal);
            }
        }
        Ok(Store {
            dir,
            goals: RwLock::new(goals),
        })
    }

    pub fn goal_ids(&self) -> Vec<GoalId> {
        self.read().keys().cloned().collect()
    }

    pub fn get_goal(&self, id: &str) -> Option<Goal> {
        self.read().get(id).cloned()
    }

    pub fn create_goal(&self, title: &str) -> Result<Goal, StoreError> {
        let goal = Goal {
            id: new_id(),
            title: title.to_string(),
            status: GoalStatus::Running,
            contract_version: 0,
            agent_synced_version: 0,
            sessions: Vec::new(),
            items: Vec::new(),
            evidence: Vec::new(),
            events: Vec::new(),
            created_at: now(),
            project_dir: None,
            watching: false,
            auto_session: None,
        };
        persist(&self.dir, &goal)?;
        self.write().insert(goal.id.clone(), goal.clone());
        Ok(goal)
    }

    /// Auto mode: get-or-create the goal owned by one agent session in one
    /// project. Idempotent inside the write lock — a double-fired hook is
    /// structurally unable to create two goals. Returns (goal, created).
    ///
    /// A returned goal may be closed or unwatched: the human's per-goal
    /// opt-out wins, and the caller must NOT re-watch it.
    pub fn create_auto_goal(
        &self,
        title: &str,
        project_dir: &str,
        session_id: &str,
        agent: &str,
    ) -> Result<(Goal, bool), StoreError> {
        let mut map = self.write();
        if let Some(existing) = map.values().find(|g| {
            g.auto_session.as_deref() == Some(session_id)
                && g.project_dir.as_deref() == Some(project_dir)
        }) {
            return Ok((existing.clone(), false));
        }
        let goal = Goal {
            id: new_id(),
            title: title.to_string(),
            status: GoalStatus::Running,
            contract_version: 0,
            agent_synced_version: 0,
            sessions: vec![SessionBinding {
                agent: agent.to_string(),
                session_id: session_id.to_string(),
                bound_at: now(),
            }],
            items: Vec::new(),
            evidence: Vec::new(),
            events: Vec::new(),
            created_at: now(),
            project_dir: Some(project_dir.to_string()),
            watching: true,
            auto_session: Some(session_id.to_string()),
        };
        persist(&self.dir, &goal)?;
        map.insert(goal.id.clone(), goal.clone());
        Ok((goal, true))
    }

    pub fn goals_for_dir(&self, dir: &str) -> Vec<Goal> {
        self.read()
            .values()
            .filter(|g| g.project_dir.as_deref() == Some(dir))
            .cloned()
            .collect()
    }

    /// Resolve which goal one session gates against in a project. Prefers
    /// the goal the session OWNS (auto_session) over goals it was merely
    /// bound to opportunistically. Includes non-watching goals — the gate
    /// needs to tell "human opted out" apart from "never bound".
    pub fn find_session_goal(&self, dir: &str, session_id: &str) -> Option<Goal> {
        let map = self.read();
        let in_dir = || {
            map.values()
                .filter(|g| g.project_dir.as_deref() == Some(dir))
        };
        in_dir()
            .find(|g| g.auto_session.as_deref() == Some(session_id))
            .or_else(|| in_dir().find(|g| g.sessions.iter().any(|s| s.session_id == session_id)))
            .cloned()
    }

    pub fn bind_session(&self, goal_id: &str, agent: &str, session_id: &str) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            if !goal.sessions.iter().any(|s| s.session_id == session_id) {
                goal.sessions.push(SessionBinding {
                    agent: agent.to_string(),
                    session_id: session_id.to_string(),
                    bound_at: now(),
                });
            }
            Ok(())
        })
    }

    /// Lay one or more items. Each add bumps the contract version by one so
    /// deltas stay per-item addressable.
    pub fn lay_items(
        &self,
        goal_id: &str,
        items: Vec<NewItem>,
        actor: Actor,
    ) -> Result<Vec<ItemId>, StoreError> {
        self.mutate(goal_id, |goal| {
            let mut ids = Vec::new();
            for new in items {
                let class = new.class.unwrap_or(Class::Subjective);
                validate_class(&class, actor)?;
                if actor == Actor::Agent && new.origin.is_user_authored() {
                    return Err(StoreError::Invalid(
                        "an agent cannot lay items with a user origin — that would corrupt the \
                         core-bet instrumentation"
                            .into(),
                    ));
                }
                goal.contract_version += 1;
                let v = goal.contract_version;
                let id = new_id();
                let history = new
                    .interpretation
                    .iter()
                    .map(|text| Interpretation {
                        text: text.clone(),
                        against_version: v,
                        at: now(),
                    })
                    .collect();
                goal.items.push(Item {
                    id: id.clone(),
                    claim: new.claim,
                    check: new.check,
                    class,
                    interpretation: new.interpretation,
                    interpretation_history: history,
                    status: ItemStatus::Open,
                    evidence_ids: Vec::new(),
                    origin: new.origin,
                    added_in_version: v,
                    last_edited_version: v,
                });
                goal.events.push(Event {
                    at: now(),
                    kind: EventKind::ContractEdited {
                        item_id: id.clone(),
                        by: actor,
                        version_after: v,
                    },
                });
                ids.push(id);
            }
            Ok(ids)
        })
    }

    /// Human (or agent, on its own items only) edits an item's yardstick.
    /// Any such edit reopens the item: prior evidence/passes were against the
    /// old yardstick.
    pub fn edit_item(
        &self,
        goal_id: &str,
        item_id: &str,
        claim: Option<String>,
        check: Option<String>,
        class: Option<Class>,
        actor: Actor,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            goal.contract_version += 1;
            let v = goal.contract_version;
            let item = find_item(&mut goal.items, item_id)?;
            if actor == Actor::Agent && item.origin.is_user_authored() {
                return Err(StoreError::Invalid(
                    "agents must not edit human-authored items; update your interpretation instead"
                        .into(),
                ));
            }
            if let Some(c) = class {
                validate_class(&c, actor)?;
                item.class = c;
            }
            if let Some(c) = claim {
                item.claim = c;
            }
            if let Some(c) = check {
                item.check = c;
            }
            item.status = ItemStatus::Open;
            item.last_edited_version = v;
            let id = item.id.clone();
            goal.events.push(Event {
                at: now(),
                kind: EventKind::ContractEdited {
                    item_id: id,
                    by: actor,
                    version_after: v,
                },
            });
            Ok(())
        })
    }

    /// Agent updates its interpretation of an item. Appending to a non-empty
    /// history is exactly the "agent reinterpreted a subjective criterion"
    /// signal that principle 6 requires surfacing.
    pub fn set_interpretation(
        &self,
        goal_id: &str,
        item_id: &str,
        text: &str,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            let version = goal.contract_version;
            let item = find_item(&mut goal.items, item_id)?;
            item.interpretation = Some(text.to_string());
            item.interpretation_history.push(Interpretation {
                text: text.to_string(),
                against_version: version,
                at: now(),
            });
            Ok(())
        })
    }

    pub fn add_evidence(
        &self,
        goal_id: &str,
        item_id: &str,
        new: NewEvidence,
    ) -> Result<EvidenceId, StoreError> {
        self.mutate(goal_id, |goal| {
            if new.provenance.is_empty() {
                return Err(StoreError::Invalid(
                    "evidence must carry at least one provenance pointer (file/command/url)".into(),
                ));
            }
            let version = goal.contract_version;
            let item = find_item(&mut goal.items, item_id)?;
            let id = new_id();
            item.evidence_ids.push(id.clone());
            // Laying evidence moves an open/rejected subjective item to Laid,
            // provided an interpretation exists. Objective items pass only
            // via report_oracle.
            if matches!(item.class, Class::Subjective)
                && matches!(item.status, ItemStatus::Open | ItemStatus::Rejected)
                && item.interpretation.is_some()
            {
                item.status = ItemStatus::Laid;
            }
            let item_id = item.id.clone();
            goal.evidence.push(Evidence {
                id: id.clone(),
                item_id: item_id.clone(),
                conclusion: new.conclusion,
                basis: new.basis,
                provenance: new.provenance,
                workspace: new.workspace,
                against_version: version,
                captured_at: now(),
            });
            goal.events.push(Event {
                at: now(),
                kind: EventKind::EvidenceAdded {
                    evidence_id: id.clone(),
                    item_id,
                },
            });
            Ok(id)
        })
    }

    /// Agent reports an oracle run for an objective item.
    pub fn report_oracle(
        &self,
        goal_id: &str,
        item_id: &str,
        passed: bool,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            let item = find_item(&mut goal.items, item_id)?;
            if !matches!(item.class, Class::Objective { .. }) {
                return Err(StoreError::Invalid(
                    "oracle results apply only to objective items; subjective items pass only on a human ruling".into(),
                ));
            }
            item.status = if passed {
                ItemStatus::Passed
            } else {
                ItemStatus::Open
            };
            Ok(())
        })
    }

    /// Human ruling on a subjective item. Never callable by the agent — the
    /// server must route agent identities away from this operation.
    pub fn rule_item(
        &self,
        goal_id: &str,
        item_id: &str,
        approve: bool,
        after_drill_down: bool,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            let item = find_item(&mut goal.items, item_id)?;
            if !matches!(item.class, Class::Subjective) {
                return Err(StoreError::Invalid(
                    "rulings apply only to subjective items".into(),
                ));
            }
            if !matches!(item.status, ItemStatus::Laid | ItemStatus::Approved | ItemStatus::Rejected) {
                return Err(StoreError::Invalid(
                    "item has nothing laid out to rule on yet".into(),
                ));
            }
            item.status = if approve {
                ItemStatus::Approved
            } else {
                ItemStatus::Rejected
            };
            let verdict = item.status;
            let id = item.id.clone();
            goal.events.push(Event {
                at: now(),
                kind: EventKind::Ruling {
                    item_id: id,
                    verdict,
                    after_drill_down,
                },
            });
            Ok(())
        })
    }

    /// Human opened the original behind an evidence pointer.
    pub fn record_drill_down(
        &self,
        goal_id: &str,
        evidence_id: &str,
        pointer: Pointer,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            goal.events.push(Event {
                at: now(),
                kind: EventKind::DrillDown {
                    evidence_id: evidence_id.to_string(),
                    pointer,
                },
            });
            Ok(())
        })
    }

    /// Agent declares it has read and aligned to `to_version`.
    pub fn reconcile(
        &self,
        goal_id: &str,
        session_id: &str,
        to_version: Version,
        reinterpreted_items: Vec<ItemId>,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            if to_version > goal.contract_version {
                return Err(StoreError::Invalid(format!(
                    "cannot reconcile to v{to_version}: latest is v{}",
                    goal.contract_version
                )));
            }
            if to_version < goal.agent_synced_version {
                return Err(StoreError::Invalid(format!(
                    "cannot reconcile backwards: already synced to v{}",
                    goal.agent_synced_version
                )));
            }
            let from = goal.agent_synced_version;
            let changed = goal
                .items_since(from)
                .iter()
                .map(|i| i.id.clone())
                .collect();
            goal.agent_synced_version = to_version;
            goal.events.push(Event {
                at: now(),
                kind: EventKind::Reconcile {
                    session_id: session_id.to_string(),
                    from_version: from,
                    to_version,
                    changed_items: changed,
                    reinterpreted_items,
                },
            });
            Ok(())
        })
    }

    pub fn record_gate_decision(
        &self,
        goal_id: &str,
        decision: GateDecisionKind,
        reason: Option<String>,
    ) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            let against = goal.contract_version;
            goal.events.push(Event {
                at: now(),
                kind: EventKind::GateDecision {
                    decision,
                    reason,
                    against_version: against,
                },
            });
            if decision == GateDecisionKind::Release {
                goal.status = GoalStatus::AwaitingRulings;
            }
            Ok(())
        })
    }

    pub fn set_watch(
        &self,
        goal_id: &str,
        project_dir: Option<String>,
        watching: bool,
    ) -> Result<Goal, StoreError> {
        self.mutate(goal_id, |goal| {
            if project_dir.is_some() {
                goal.project_dir = project_dir;
            }
            goal.watching = watching;
            Ok(goal.clone())
        })
    }

    pub fn set_status(&self, goal_id: &str, status: GoalStatus) -> Result<(), StoreError> {
        self.mutate(goal_id, |goal| {
            goal.status = status;
            Ok(())
        })
    }

    /// Permanently remove a goal — disk first, then memory, so a failed file
    /// removal leaves the store consistent. Returns the removed goal so the
    /// caller can clean up anything keyed on it (e.g. an armed marker).
    pub fn delete_goal(&self, goal_id: &str) -> Result<Goal, StoreError> {
        let mut map = self.write();
        let goal = map
            .get(goal_id)
            .cloned()
            .ok_or_else(|| StoreError::GoalNotFound(goal_id.to_string()))?;
        let path = self.dir.join(format!("{}.json", goal.id));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        map.remove(goal_id);
        Ok(goal)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<GoalId, Goal>> {
        self.goals.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<GoalId, Goal>> {
        self.goals.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// All mutations go through here: work on a clone, persist, then swap —
    /// a failed operation leaves neither memory nor disk half-changed.
    fn mutate<T>(
        &self,
        goal_id: &str,
        f: impl FnOnce(&mut Goal) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut map = self.write();
        let goal = map
            .get_mut(goal_id)
            .ok_or_else(|| StoreError::GoalNotFound(goal_id.to_string()))?;
        let mut draft = goal.clone();
        let out = f(&mut draft)?;
        normalize_parked_status(&mut draft);
        persist(&self.dir, &draft)?;
        *goal = draft;
        Ok(out)
    }
}

/// While a goal is parked after release, `AwaitingRulings` vs `Ruled` is a
/// pure derivation of item statuses — any subjective item still `Laid` keeps
/// the goal awaiting. Recomputed on every mutation (a ruling can settle it;
/// fresh evidence on a rejected item re-opens it) and on load, so goals
/// stored before the split heal on open.
fn normalize_parked_status(goal: &mut Goal) {
    if matches!(
        goal.status,
        GoalStatus::AwaitingRulings | GoalStatus::Ruled
    ) {
        let awaiting = goal
            .items
            .iter()
            .any(|i| matches!(i.class, Class::Subjective) && i.status == ItemStatus::Laid);
        goal.status = if awaiting {
            GoalStatus::AwaitingRulings
        } else {
            GoalStatus::Ruled
        };
    }
}

fn find_item<'a>(items: &'a mut [Item], id: &str) -> Result<&'a mut Item, StoreError> {
    items
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| StoreError::ItemNotFound(id.to_string()))
}

fn validate_class(class: &Class, actor: Actor) -> Result<(), StoreError> {
    if let Class::Objective { promoted_by, .. } = class {
        if actor == Actor::Agent && *promoted_by == Actor::Human {
            return Err(StoreError::Invalid(
                "an agent cannot claim a human promoted this item to objective".into(),
            ));
        }
    }
    Ok(())
}

/// Write-to-temp then rename, so a crash never leaves a torn goal file.
fn persist(dir: &Path, goal: &Goal) -> Result<(), StoreError> {
    let tmp = dir.join(format!("{}.json.tmp", goal.id));
    let dst = dir.join(format!("{}.json", goal.id));
    fs::write(&tmp, serde_json::to_string_pretty(goal)?)?;
    fs::rename(&tmp, &dst)?;
    Ok(())
}
