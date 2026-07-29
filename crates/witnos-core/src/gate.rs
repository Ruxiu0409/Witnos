//! The central state machine's release condition.
//!
//! Gate release ≠ the human agreeing: the agent never waits for a human, so
//! release requires only that every subjective item is LAID (interpretation +
//! fresh evidence), every objective item is PASSED, and the agent has
//! reconciled against the latest contract. The human's lever is editing the
//! contract or sending an item back — both of which move the version and land
//! right back here.

use crate::types::*;

#[derive(Debug, Clone)]
pub struct GateOutcome {
    pub release: bool,
    /// Always a delta — the specific unmet things, never the full list.
    pub reasons: Vec<String>,
}

pub fn evaluate(goal: &Goal) -> GateOutcome {
    let mut reasons = Vec::new();

    if goal.items.is_empty() {
        reasons.push("contract is empty — lay out verification items first".to_string());
    }

    for item in &goal.items {
        // The human opted this one out: not blocked on, no evidence demanded,
        // not even mentioned — per-goal opt-out narrowed to a single item.
        if item.status == ItemStatus::Waived {
            continue;
        }

        let fresh = goal
            .evidence
            .iter()
            .any(|e| e.item_id == item.id && e.against_version >= item.last_edited_version);

        match &item.class {
            Class::Objective { .. } => {
                if item.status != ItemStatus::Passed {
                    reasons.push(format!("objective item not passed: \"{}\"", item.claim));
                } else if !fresh {
                    reasons.push(format!(
                        "objective item passed but its evidence predates the latest edit: \"{}\"",
                        item.claim
                    ));
                }
            }
            Class::Subjective => match item.status {
                ItemStatus::Laid => {
                    if item.interpretation.is_none() {
                        reasons.push(format!(
                            "subjective item has no interpretation: \"{}\"",
                            item.claim
                        ));
                    }
                    if !fresh {
                        reasons.push(format!(
                            "subjective item's evidence predates the latest edit: \"{}\"",
                            item.claim
                        ));
                    }
                }
                ItemStatus::Rejected => reasons.push(format!(
                    "item was rejected by the human and not yet re-addressed: \"{}\"",
                    item.claim
                )),
                _ => reasons.push(format!(
                    "subjective item not laid (needs interpretation + evidence): \"{}\"",
                    item.claim
                )),
            },
        }
    }

    if goal.agent_synced_version != goal.contract_version {
        reasons.push(format!(
            "contract moved: you are synced to v{}, latest is v{} — fetch the delta and reconcile",
            goal.agent_synced_version, goal.contract_version
        ));
    }

    GateOutcome {
        release: reasons.is_empty(),
        reasons,
    }
}
