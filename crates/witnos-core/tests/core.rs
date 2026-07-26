use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use witnos_core::*;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_store() -> (Store, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "witnos-core-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::open(&dir).unwrap(), dir)
}

fn subjective(claim: &str, origin: Origin) -> NewItem {
    NewItem {
        claim: claim.to_string(),
        check: "look at it".to_string(),
        class: None,
        interpretation: None,
        origin,
    }
}

fn objective(claim: &str, promoted_by: Actor) -> NewItem {
    NewItem {
        claim: claim.to_string(),
        check: "run the oracle".to_string(),
        class: Some(Class::Objective {
            oracle: Oracle {
                command: "cargo test".into(),
                expected: "exit 0".into(),
            },
            promoted_by,
        }),
        interpretation: None,
        origin: Origin::AgentInitial,
    }
}

fn some_evidence() -> NewEvidence {
    NewEvidence {
        conclusion: "holds".into(),
        basis: "palette has exactly 3 colors: #000, #fff, #888".into(),
        provenance: vec![Pointer::Command {
            cmd: "extract-palette ./src".into(),
        }],
        workspace: WorkspaceFingerprint::default(),
    }
}

#[test]
fn defaults_to_subjective_and_bumps_version_per_item() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    assert_eq!(goal.contract_version, 0);

    let ids = store
        .lay_items(
            &goal.id,
            vec![
                subjective("only black/white/grey", Origin::AgentInitial),
                subjective("feels Apple-like", Origin::AgentBlindspot),
            ],
            Actor::Agent,
        )
        .unwrap();
    assert_eq!(ids.len(), 2);

    let goal = store.get_goal(&goal.id).unwrap();
    assert_eq!(goal.contract_version, 2);
    assert!(goal
        .items
        .iter()
        .all(|i| matches!(i.class, Class::Subjective)));
    assert_eq!(goal.items_since(1).len(), 1);
    assert_eq!(goal.items_since(0).len(), 2);
}

#[test]
fn agent_cannot_claim_human_promotion() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let err = store
        .lay_items(&goal.id, vec![objective("builds", Actor::Human)], Actor::Agent)
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "got: {err}");
}

#[test]
fn evidence_requires_provenance() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![subjective("x", Origin::AgentInitial)],
            Actor::Agent,
        )
        .unwrap();
    let mut ev = some_evidence();
    ev.provenance.clear();
    let err = store.add_evidence(&goal.id, &ids[0], ev).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)));
}

#[test]
fn subjective_needs_interpretation_before_it_counts_as_laid() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![subjective("tasteful", Origin::UserPreRun)],
            Actor::Human,
        )
        .unwrap();

    store.add_evidence(&goal.id, &ids[0], some_evidence()).unwrap();
    let goal_now = store.get_goal(&goal.id).unwrap();
    assert_eq!(goal_now.items[0].status, ItemStatus::Open, "no interpretation yet");

    store
        .set_interpretation(&goal.id, &ids[0], "I read 'tasteful' as: max 3 colors, no gradients")
        .unwrap();
    store.add_evidence(&goal.id, &ids[0], some_evidence()).unwrap();
    let goal_now = store.get_goal(&goal.id).unwrap();
    assert_eq!(goal_now.items[0].status, ItemStatus::Laid);
}

#[test]
fn full_loop_release_block_and_rulings() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();

    // Empty contract blocks.
    assert!(!evaluate(&store.get_goal(&goal.id).unwrap()).release);

    let ids = store
        .lay_items(
            &goal.id,
            vec![
                objective("cargo test passes", Actor::Agent),
                subjective("UI feels calm", Origin::AgentInitial),
            ],
            Actor::Agent,
        )
        .unwrap();
    let (obj, subj) = (&ids[0], &ids[1]);

    // Nothing laid yet → block with per-item reasons + sync reason.
    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(!out.release);
    assert_eq!(out.reasons.len(), 3, "{:?}", out.reasons);

    // Lay everything out.
    store.report_oracle(&goal.id, obj, true).unwrap();
    store.add_evidence(&goal.id, obj, some_evidence()).unwrap();
    store
        .set_interpretation(&goal.id, subj, "calm = whitespace, no motion")
        .unwrap();
    store.add_evidence(&goal.id, subj, some_evidence()).unwrap();
    store.reconcile(&goal.id, "sess-1", 2, vec![]).unwrap();

    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(out.release, "{:?}", out.reasons);

    // Human edits the subjective claim mid-run → reopened + version moved → block again.
    store
        .edit_item(
            &goal.id,
            subj,
            Some("UI feels calm (and NO animation at all)".into()),
            None,
            None,
            Actor::Human,
        )
        .unwrap();
    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(!out.release);
    assert!(out.reasons.iter().any(|r| r.contains("not laid")));
    assert!(out.reasons.iter().any(|r| r.contains("contract moved")));

    // Agent re-lays: interpretation + fresh evidence + reconcile → release.
    store
        .set_interpretation(&goal.id, subj, "no animation: no CSS transitions either")
        .unwrap();
    store.add_evidence(&goal.id, subj, some_evidence()).unwrap();
    store.reconcile(&goal.id, "sess-1", 3, vec![subj.clone()]).unwrap();
    assert!(evaluate(&store.get_goal(&goal.id).unwrap()).release);

    // Gate releases → goal parks in AwaitingRulings (normal terminal state).
    store
        .record_gate_decision(&goal.id, GateDecisionKind::Release, None)
        .unwrap();
    assert_eq!(
        store.get_goal(&goal.id).unwrap().status,
        GoalStatus::AwaitingRulings
    );

    // Human rejects → blocked again until re-addressed; approve after re-lay.
    store.rule_item(&goal.id, subj, false, true).unwrap();
    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(out.reasons.iter().any(|r| r.contains("rejected")));
    store.add_evidence(&goal.id, subj, some_evidence()).unwrap();
    assert_eq!(
        store.get_goal(&goal.id).unwrap().item(subj).unwrap().status,
        ItemStatus::Laid
    );
    store.rule_item(&goal.id, subj, true, false).unwrap();
    assert_eq!(
        store.get_goal(&goal.id).unwrap().item(subj).unwrap().status,
        ItemStatus::Approved
    );

    // The reinterpretation trail exists (principle 6's raw material).
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.item(subj).unwrap().interpretation_history.len(), 2);
}

#[test]
fn agent_must_not_edit_human_items_and_rulings_are_subjective_only() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![
                subjective("human's bar", Origin::UserMidRun),
                objective("builds", Actor::Agent),
            ],
            Actor::Human,
        )
        .unwrap();

    let err = store
        .edit_item(&goal.id, &ids[0], Some("weaker bar".into()), None, None, Actor::Agent)
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)));

    let err = store.rule_item(&goal.id, &ids[1], true, false).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)));

    let err = store.report_oracle(&goal.id, &ids[0], true).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)));
}

#[test]
fn reconcile_bounds() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    store
        .lay_items(
            &goal.id,
            vec![subjective("x", Origin::AgentInitial)],
            Actor::Agent,
        )
        .unwrap();
    assert!(store.reconcile(&goal.id, "s", 5, vec![]).is_err());
    store.reconcile(&goal.id, "s", 1, vec![]).unwrap();
    assert!(store.reconcile(&goal.id, "s", 0, vec![]).is_err());
}

#[test]
fn strong_bet_readout_counts_evidence_triggered_items() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![subjective("base", Origin::UserPreRun)],
            Actor::Human,
        )
        .unwrap();
    store
        .set_interpretation(&goal.id, &ids[0], "whatever")
        .unwrap();
    let ev = store.add_evidence(&goal.id, &ids[0], some_evidence()).unwrap();

    // The user saw the palette evidence and remembered an unspoken expectation.
    store
        .lay_items(
            &goal.id,
            vec![NewItem {
                claim: "only black/white/grey".into(),
                check: "count distinct colors".into(),
                class: None,
                interpretation: None,
                origin: Origin::UserViewingEvidence { evidence_id: ev },
            }],
            Actor::Human,
        )
        .unwrap();

    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.strong_bet_count(), 1);
}

#[test]
fn persistence_roundtrip() {
    let (store, dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    store
        .lay_items(
            &goal.id,
            vec![subjective("x", Origin::AgentInitial)],
            Actor::Agent,
        )
        .unwrap();

    let reopened = Store::open(&dir).unwrap();
    let g = reopened.get_goal(&goal.id).unwrap();
    assert_eq!(g.title, "demo");
    assert_eq!(g.contract_version, 1);
    assert_eq!(g.items.len(), 1);
    assert!(matches!(g.items[0].origin, Origin::AgentInitial));
}
