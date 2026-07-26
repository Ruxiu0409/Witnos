use serde::{Deserialize, Serialize};

pub type GoalId = String;
pub type ItemId = String;
pub type EvidenceId = String;
pub type SessionId = String;
pub type Version = u64;
pub type UnixSeconds = u64;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now() -> UnixSeconds {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Running,
    /// Agent released by the gate; subjective items await human rulings.
    /// This is a NORMAL terminal state of a goal.
    AwaitingRulings,
    /// Turn ended without meeting the release condition (consecutive-block
    /// cap, user interrupt). Re-issuing or resuming moves it back to Running.
    TurnEndedUnmet,
    /// No agent reads this anymore; to change the outcome, re-issue the goal.
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBinding {
    pub agent: String,
    pub session_id: SessionId,
    pub bound_at: UnixSeconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Oracle {
    pub command: String,
    pub expected: String,
}

/// Classification is NOT the agent's call (the Goodhart side door):
/// default subjective; objective requires a machine-executable oracle —
/// structurally mandatory here, even when a human promotes the item
/// (promotion decides who takes responsibility, it does not waive the oracle).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Class {
    Subjective,
    Objective { oracle: Oracle, promoted_by: Actor },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Open,
    /// Interpretation + evidence laid out (the release condition for
    /// subjective items — passing still requires a human nod).
    Laid,
    /// Objective only: oracle passed, agent self-passed.
    Passed,
    /// Subjective only: human nodded.
    Approved,
    /// Subjective only: human rejected; while the goal is running the agent
    /// must re-address it (new evidence moves it back to Laid).
    Rejected,
}

/// Where a contract item came from — the instrumentation for the core bet.
/// `UserViewingEvidence` is the direct readout of the strong version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Origin {
    UserPreRun,
    UserViewingEvidence { evidence_id: EvidenceId },
    UserMidRun,
    AgentInitial,
    AgentBlindspot,
}

impl Origin {
    pub fn is_user_authored(&self) -> bool {
        matches!(
            self,
            Origin::UserPreRun | Origin::UserViewingEvidence { .. } | Origin::UserMidRun
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interpretation {
    pub text: String,
    pub against_version: Version,
    pub at: UnixSeconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub claim: String,
    pub check: String,
    pub class: Class,
    /// Current interpretation. Mandatory for a subjective item to count as laid.
    pub interpretation: Option<String>,
    /// Every entry after the first is a reinterpretation — the raw material
    /// for principle 6's active flagging.
    pub interpretation_history: Vec<Interpretation>,
    pub status: ItemStatus,
    pub evidence_ids: Vec<EvidenceId>,
    pub origin: Origin,
    pub added_in_version: Version,
    /// Evidence with against_version < this is stale for this item.
    pub last_edited_version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Pointer {
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<String>,
    },
    Command {
        cmd: String,
    },
    Url {
        url: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceFingerprint {
    pub commit: Option<String>,
    pub dirty_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: EvidenceId,
    pub item_id: ItemId,
    /// What the agent concluded ("I judge this item to currently hold because …").
    pub conclusion: String,
    /// What it judged by (the detected palette, the measured numbers, …).
    pub basis: String,
    /// Never empty — every piece of evidence must carry its own
    /// insufficiency sensor (one-click open-the-original in the UI).
    pub provenance: Vec<Pointer>,
    pub workspace: WorkspaceFingerprint,
    /// "Stamped against contract version N" — the trust basis of
    /// after-the-fact judgement.
    pub against_version: Version,
    pub captured_at: UnixSeconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecisionKind {
    Block,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EventKind {
    ContractEdited {
        item_id: ItemId,
        by: Actor,
        version_after: Version,
    },
    EvidenceAdded {
        evidence_id: EvidenceId,
        item_id: ItemId,
    },
    Reconcile {
        session_id: SessionId,
        from_version: Version,
        to_version: Version,
        changed_items: Vec<ItemId>,
        reinterpreted_items: Vec<ItemId>,
    },
    GateDecision {
        decision: GateDecisionKind,
        reason: Option<String>,
        against_version: Version,
    },
    /// Human opened the original behind an evidence pointer. Together with a
    /// following Ruling{after_drill_down: true} this log IS the requirements
    /// spec for the future raw-trace layer's filtering.
    DrillDown {
        evidence_id: EvidenceId,
        pointer: Pointer,
    },
    Ruling {
        item_id: ItemId,
        verdict: ItemStatus,
        after_drill_down: bool,
    },
    TurnEnded {
        met: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub at: UnixSeconds,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: GoalId,
    pub title: String,
    pub status: GoalStatus,
    /// Monotonic. Bumps on every item add/edit (human or agent).
    /// Evidence does NOT bump it (evidence fills items in; it doesn't move the yardstick).
    pub contract_version: Version,
    /// Where the agent last reconciled to.
    pub agent_synced_version: Version,
    pub sessions: Vec<SessionBinding>,
    pub items: Vec<Item>,
    pub evidence: Vec<Evidence>,
    pub events: Vec<Event>,
    pub created_at: UnixSeconds,
    /// The project directory being watched (where the armed marker lives).
    #[serde(default)]
    pub project_dir: Option<String>,
    /// Watching = the armed marker is maintained in `project_dir` and the
    /// Stop gate fails closed there.
    #[serde(default)]
    pub watching: bool,
}

impl Goal {
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Delta since a version: items added or edited after it.
    pub fn items_since(&self, version: Version) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|i| i.last_edited_version > version)
            .collect()
    }

    /// The strong-bet readout: how many items were added while the user was
    /// looking at a specific piece of evidence.
    pub fn strong_bet_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.origin, Origin::UserViewingEvidence { .. }))
            .count()
    }
}
