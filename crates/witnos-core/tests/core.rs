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

#[test]
fn delete_goal_removes_memory_and_disk() {
    let (store, dir) = temp_store();
    let goal = store.create_goal("doomed").unwrap();
    let file = dir.join(format!("{}.json", goal.id));
    assert!(file.exists());

    let removed = store.delete_goal(&goal.id).unwrap();
    assert_eq!(removed.id, goal.id);
    assert!(!file.exists());
    assert!(store.get_goal(&goal.id).is_none());
    // a reopened store must not resurrect it
    drop(store);
    let store = Store::open(&dir).unwrap();
    assert!(store.get_goal(&goal.id).is_none());

    // deleting twice reports GoalNotFound
    assert!(matches!(
        store.delete_goal(&goal.id),
        Err(StoreError::GoalNotFound(_))
    ));
}

// ---------- auto mode: session goals, marker derivation, registry ----------

#[test]
fn create_auto_goal_is_idempotent_per_session() {
    let (store, _dir) = temp_store();
    let (a, created) = store
        .create_auto_goal("fix login", "/proj", "sA", "claude-code")
        .unwrap();
    assert!(created);
    assert!(a.watching);
    assert_eq!(a.auto_session.as_deref(), Some("sA"));
    assert_eq!(a.project_dir.as_deref(), Some("/proj"));
    assert_eq!(a.sessions.len(), 1, "creation binds the owning session");

    // Same session: returns the SAME goal, even after opt-out — the human's
    // per-goal decision must never be undone by a re-fired hook.
    store.set_watch(&a.id, None, false).unwrap();
    let (again, created) = store
        .create_auto_goal("fix login retry", "/proj", "sA", "claude-code")
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, a.id);
    assert!(!again.watching, "opt-out must survive");

    // A different session gets its own goal.
    let (b, created) = store
        .create_auto_goal("refactor payments", "/proj", "sB", "claude-code")
        .unwrap();
    assert!(created);
    assert_ne!(b.id, a.id);
}

#[test]
fn find_session_goal_prefers_the_owned_goal() {
    let (store, _dir) = temp_store();
    let (a, _) = store
        .create_auto_goal("own goal", "/proj", "sA", "claude-code")
        .unwrap();
    let manual = store.create_goal("manual").unwrap();
    store
        .set_watch(&manual.id, Some("/proj".into()), true)
        .unwrap();
    // Opportunistic bind of sA to the manual goal must not shadow ownership.
    store.bind_session(&manual.id, "claude-code", "sA").unwrap();

    let found = store.find_session_goal("/proj", "sA").unwrap();
    assert_eq!(found.id, a.id);
    // A session only bound (not owning) resolves to what it's bound to.
    store.bind_session(&manual.id, "claude-code", "sB").unwrap();
    assert_eq!(store.find_session_goal("/proj", "sB").unwrap().id, manual.id);
    assert!(store.find_session_goal("/proj", "sC").is_none());
    assert!(store.find_session_goal("/elsewhere", "sA").is_none());
}

#[test]
fn marker_compute_derives_sessions_and_default() {
    let (store, _dir) = temp_store();
    let (a, _) = store
        .create_auto_goal("auto A", "/proj", "sA", "claude-code")
        .unwrap();
    let manual = store.create_goal("manual").unwrap();
    store
        .set_watch(&manual.id, Some("/proj".into()), true)
        .unwrap();
    store.bind_session(&manual.id, "claude-code", "sB").unwrap();
    // Opportunistic cross-bind: sA also lands on the manual goal.
    store.bind_session(&manual.id, "claude-code", "sA").unwrap();

    let goals = store.goals_for_dir("/proj");
    let m = marker::compute(true, &goals).unwrap();
    assert!(m.auto);
    assert_eq!(
        m.default_goal.as_ref().unwrap().goal_id,
        manual.id,
        "newest watching manual goal is the default"
    );
    assert_eq!(m.sessions["sA"].goal_id, a.id, "owner wins its slot");
    assert_eq!(m.sessions["sB"].goal_id, manual.id);

    // Unwatching drops a goal out of the derivation entirely.
    store.set_watch(&a.id, None, false).unwrap();
    let m = marker::compute(true, &store.goals_for_dir("/proj")).unwrap();
    assert_eq!(m.sessions["sA"].goal_id, manual.id, "falls back to the bound goal");

    // Manual project with nothing watching → no marker; auto keeps one.
    store.set_watch(&manual.id, None, false).unwrap();
    let goals = store.goals_for_dir("/proj");
    assert!(marker::compute(false, &goals).is_none());
    let m = marker::compute(true, &goals).unwrap();
    assert!(m.sessions.is_empty() && m.default_goal.is_none());
}

#[test]
fn marker_parse_handles_both_shapes_and_resolves() {
    use witnos_core::marker::{ArmedMarker, Resolution};

    // Legacy v1 normalizes into a manual default goal.
    let legacy = ArmedMarker::parse(r#"{"goal_id":"g1","contract_version":3}"#).unwrap();
    assert!(!legacy.auto);
    let d = legacy.default_goal.as_ref().unwrap();
    assert_eq!((d.goal_id.as_str(), d.contract_version, d.agent_synced_version), ("g1", 3, 0));
    assert!(matches!(legacy.resolve(Some("any")), Resolution::Entry(e) if e.goal_id == "g1"));

    // v2 auto with no goals: unbound sessions are NoGoalAuto.
    let auto = ArmedMarker::parse(r#"{"v":2,"auto":true}"#).unwrap();
    assert!(matches!(auto.resolve(Some("sX")), Resolution::NoGoalAuto));
    assert!(matches!(auto.resolve(None), Resolution::NoGoalAuto));

    // v2 with a session entry: that session resolves to its own goal.
    let m = ArmedMarker::parse(
        r#"{"v":2,"auto":true,"sessions":{"sA":{"goal_id":"gA","contract_version":5,"agent_synced_version":2}}}"#,
    )
    .unwrap();
    assert!(matches!(m.resolve(Some("sA")), Resolution::Entry(e) if e.goal_id == "gA"));
    assert!(matches!(m.resolve(Some("sB")), Resolution::NoGoalAuto));

    // Unusable content is None (gate still arms on presence).
    assert!(ArmedMarker::parse("not json").is_none());
    // An unrecognized object parses as an empty MANUAL marker → NoGoalManual
    // (fail-closed on the gate path).
    let odd = ArmedMarker::parse(r#"{"weird":1}"#).unwrap();
    assert!(matches!(odd.resolve(Some("s")), Resolution::NoGoalManual));

    // Round-trip.
    let again = ArmedMarker::parse(&m.to_pretty()).unwrap();
    assert_eq!(again, m);
}

#[test]
fn registry_round_trips_and_canonicalizes() {
    let home = std::env::temp_dir().join(format!(
        "witnos-core-reg-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&home);
    let proj = home.join("some-project");
    std::fs::create_dir_all(&proj).unwrap();

    let reg = ProjectRegistry::load(&home);
    assert!(reg.list().is_empty());
    assert!(reg.add(proj.to_str().unwrap()).unwrap());
    assert!(!reg.add(proj.to_str().unwrap()).unwrap(), "dedupe");
    // A messy-but-equivalent path resolves to the same entry.
    let messy = format!("{}/../some-project", proj.display());
    assert!(reg.contains(&messy));
    assert_eq!(reg.list().len(), 1);

    // Persisted: a fresh load sees it.
    let reloaded = ProjectRegistry::load(&home);
    assert_eq!(reloaded.list().len(), 1);
    assert!(reloaded.remove(proj.to_str().unwrap()).unwrap());
    assert!(!reloaded.remove(proj.to_str().unwrap()).unwrap());
    assert!(ProjectRegistry::load(&home).list().is_empty());
}

#[test]
fn parked_goal_status_derives_ruled_from_rulings() {
    let (store, dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![
                subjective("calm UI", Origin::AgentInitial),
                subjective("readable copy", Origin::AgentInitial),
            ],
            Actor::Agent,
        )
        .unwrap();
    for id in &ids {
        store.set_interpretation(&goal.id, id, "how I read it").unwrap();
        store.add_evidence(&goal.id, id, some_evidence()).unwrap();
    }
    store
        .record_gate_decision(&goal.id, GateDecisionKind::Release, None)
        .unwrap();
    let status = |s: &Store| s.get_goal(&goal.id).unwrap().status;
    assert_eq!(status(&store), GoalStatus::AwaitingRulings);

    // Half ruled → still awaiting.
    store.rule_item(&goal.id, &ids[0], true, false).unwrap();
    assert_eq!(status(&store), GoalStatus::AwaitingRulings);

    // Last laid item ruled (a rejection is a ruling too) → ruled.
    store.rule_item(&goal.id, &ids[1], false, false).unwrap();
    assert_eq!(status(&store), GoalStatus::Ruled);

    // Re-ruling flips a verdict, not the parked state.
    store.rule_item(&goal.id, &ids[0], false, false).unwrap();
    assert_eq!(status(&store), GoalStatus::Ruled);

    // Fresh evidence re-lays a rejected item → back to awaiting.
    store.add_evidence(&goal.id, &ids[1], some_evidence()).unwrap();
    assert_eq!(status(&store), GoalStatus::AwaitingRulings);
    store.rule_item(&goal.id, &ids[1], true, false).unwrap();
    store.rule_item(&goal.id, &ids[0], true, false).unwrap();
    assert_eq!(status(&store), GoalStatus::Ruled);

    // A goal persisted before the split (parked as awaiting_rulings though
    // fully ruled) heals when the store loads it.
    let path = dir.join(format!("{}.json", goal.id));
    let stale = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"ruled\"", "\"awaiting_rulings\"");
    std::fs::write(&path, stale).unwrap();
    drop(store);
    let store = Store::open(&dir).unwrap();
    assert_eq!(status(&store), GoalStatus::Ruled);
}
