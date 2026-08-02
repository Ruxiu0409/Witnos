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
    /// Agent released by the gate; the work is presumed correct and the human
    /// may still send items back. This is a NORMAL terminal state of a goal.
    /// (`ruled` in goals stored before approval was removed from the domain
    /// lands here: there is no longer a "fully ruled" state to reach.)
    #[serde(alias = "ruled")]
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
    /// Which terminal pane the session runs in, when Witnos spawned it (the
    /// app stamps `WITNOS_PANE` on every shell; the binding hook forwards it).
    /// This is what lets the human type a correction back into the shell their
    /// agent is actually sitting in. `None` = we don't know where it lives.
    #[serde(default)]
    pub pane: Option<u32>,
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
    /// (`rejected` in goals stored before send-back was removed lands here: a
    /// sent-back item was one the agent had to re-address with fresh evidence,
    /// which is exactly what `open` already means. See `edit_item`.)
    #[serde(alias = "rejected")]
    Open,
    /// Interpretation + evidence laid out — the terminal state of a subjective
    /// item. The agent's work is presumed correct from here; the human's lever
    /// is editing the yardstick or waiving the item, not blessing it.
    /// (`approved` in goals stored before approval was removed lands here — an
    /// approved item was a laid item the human had also nodded at.)
    #[serde(alias = "approved")]
    Laid,
    /// Objective only: oracle passed, agent self-passed.
    Passed,
    /// Historical only: the human's per-item opt-out, replaced by deletion on
    /// 2026-08-02. Nothing produces it anymore, but goals written before then
    /// carry waived items and the gate must go on ignoring them — aliasing this
    /// onto `Open` would silently re-arm the gate against a check its owner had
    /// already opted out of, which is the one thing a load must never do.
    Waived,
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
        /// Did the human open an evidence original before moving this
        /// yardstick? The anti-rubber-stamping signal (principle 6), and half
        /// of the drill-down log the raw-trace layer will be specified from.
        /// It used to hang off `Ruling`; editing is now the whole of
        /// disagreement, so it hangs here. Always false for an agent edit, and
        /// for goals stored before the field existed.
        #[serde(default)]
        after_drill_down: bool,
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
    /// following ContractEdited{after_drill_down: true} — or an item added with
    /// origin `UserViewingEvidence` — this log IS the requirements spec for the
    /// future raw-trace layer's filtering.
    DrillDown {
        evidence_id: EvidenceId,
        pointer: Pointer,
    },
    /// Historical only: the human sending an item back, removed from the domain
    /// on 2026-08-02. Nothing writes this anymore — it stays so goals recorded
    /// before then still deserialize, with their whole event log intact (that
    /// log is the core bet's readout; dropping the variant would throw away the
    /// very drill-down pairs it exists to count). `verdict` reads back as
    /// `open` via the alias on ItemStatus.
    Ruling {
        item_id: ItemId,
        verdict: ItemStatus,
        after_drill_down: bool,
    },
    /// The human took an item out of the contract. Carries the claim text and
    /// the version because the item is *gone*: afterwards the id names nothing,
    /// and `items_since` cannot report a row that no longer exists — so this
    /// event is the only way a running agent can be told to stop working on it
    /// (see [`Goal::deletions_since`]).
    ItemDeleted {
        item_id: ItemId,
        claim: String,
        version_after: Version,
    },
    /// Historical only: the human opting one item out, replaced by deletion on
    /// 2026-08-02. Kept so goals written before then still deserialize.
    Waiver {
        item_id: ItemId,
        waived: bool,
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
    /// The version at which a HUMAN last moved the yardstick — an add, an edit,
    /// a waiver. NOT moved by anything the agent does.
    ///
    /// It exists because `contract_version > agent_synced_version` cannot mean
    /// "the human changed something the agent hasn't read": the agent bumps the
    /// contract itself every time it lays an item, so that comparison is true
    /// through ordinary mid-run work. The UI's "send it to the agent now" offer
    /// compares against this instead, so it appears when there is actually
    /// something of the human's to deliver — an affordance that is always lit is
    /// noise, and principle 4 spends the human's attention nowhere else.
    #[serde(default)]
    pub last_human_edit_version: Version,
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
    /// Set when this goal was auto-created for one agent session (auto mode:
    /// one goal per session, titled from its first prompt). None = manual.
    #[serde(default)]
    pub auto_session: Option<SessionId>,
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

    /// The other half of the delta: what was taken OUT of the contract since
    /// `version`. A deleted item cannot show up in [`Self::items_since`] — the
    /// row is gone — so without this a running agent would keep producing
    /// evidence for a criterion nobody holds anymore, and only find out by
    /// hitting `ItemNotFound`. Read off the event log, which is why
    /// `ItemDeleted` carries the claim text and the version it happened at.
    pub fn deletions_since(&self, version: Version) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ItemDeleted {
                    claim,
                    version_after,
                    ..
                } if *version_after > version => Some(claim.as_str()),
                _ => None,
            })
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
