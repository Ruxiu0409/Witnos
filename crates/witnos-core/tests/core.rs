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
fn full_loop_release_block_and_rejection() {
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

    // The human sends the item back. That is a move on the yardstick, so the
    // version bumps and the item lands in the delta the delivery channel
    // computes from what the agent had already seen — a running agent hears
    // about it, instead of the news waiting at the gate.
    let seen = store.get_goal(&goal.id).unwrap().contract_version;
    store.reject_item(&goal.id, subj, true).unwrap();
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.contract_version, seen + 1, "a rejection moves the yardstick");
    assert_eq!(g.item(subj).unwrap().last_edited_version, g.contract_version);
    assert!(
        g.items_since(seen).iter().any(|i| i.id == *subj),
        "the rejection must be in the delta"
    );
    let out = evaluate(&g);
    assert!(out.reasons.iter().any(|r| r.contains("rejected")));

    // Fresh evidence re-lays it; reconciling to the new version releases again.
    store.add_evidence(&goal.id, subj, some_evidence()).unwrap();
    assert_eq!(
        store.get_goal(&goal.id).unwrap().item(subj).unwrap().status,
        ItemStatus::Laid
    );
    store
        .reconcile(&goal.id, "sess-1", seen + 1, vec![subj.clone()])
        .unwrap();
    assert!(evaluate(&store.get_goal(&goal.id).unwrap()).release);

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

    let err = store.reject_item(&goal.id, &ids[1], false).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)));
    // A refused ruling must not have moved the contract version either.
    assert_eq!(store.get_goal(&goal.id).unwrap().contract_version, 2);

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
        .create_auto_goal("fix login", "/proj", "sA", "claude-code", None)
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
        .create_auto_goal("fix login retry", "/proj", "sA", "claude-code", None)
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, a.id);
    assert!(!again.watching, "opt-out must survive");

    // A different session gets its own goal.
    let (b, created) = store
        .create_auto_goal("refactor payments", "/proj", "sB", "claude-code", None)
        .unwrap();
    assert!(created);
    assert_ne!(b.id, a.id);
}

#[test]
fn find_session_goal_prefers_the_owned_goal() {
    let (store, _dir) = temp_store();
    let (a, _) = store
        .create_auto_goal("own goal", "/proj", "sA", "claude-code", None)
        .unwrap();
    let manual = store.create_goal("manual").unwrap();
    store
        .set_watch(&manual.id, Some("/proj".into()), true)
        .unwrap();
    // Opportunistic bind of sA to the manual goal must not shadow ownership.
    store.bind_session(&manual.id, "claude-code", "sA", None).unwrap();

    let found = store.find_session_goal("/proj", "sA").unwrap();
    assert_eq!(found.id, a.id);
    // A session only bound (not owning) resolves to what it's bound to.
    store.bind_session(&manual.id, "claude-code", "sB", None).unwrap();
    assert_eq!(store.find_session_goal("/proj", "sB").unwrap().id, manual.id);
    assert!(store.find_session_goal("/proj", "sC").is_none());
    assert!(store.find_session_goal("/elsewhere", "sA").is_none());
}

#[test]
fn marker_compute_derives_sessions_and_default() {
    let (store, _dir) = temp_store();
    let (a, _) = store
        .create_auto_goal("auto A", "/proj", "sA", "claude-code", None)
        .unwrap();
    let manual = store.create_goal("manual").unwrap();
    store
        .set_watch(&manual.id, Some("/proj".into()), true)
        .unwrap();
    store.bind_session(&manual.id, "claude-code", "sB", None).unwrap();
    // Opportunistic cross-bind: sA also lands on the manual goal.
    store.bind_session(&manual.id, "claude-code", "sA", None).unwrap();

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

/// A goal stays parked in `AwaitingRulings` after release: the human may still
/// send items back, so "you can still intervene" never stops being true.
#[test]
fn a_released_goal_parks_awaiting_rulings_and_stays_there() {
    let (store, _dir) = temp_store();
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

    // Sending one back does not change the parked state — only the item's.
    store.reject_item(&goal.id, &ids[0], false).unwrap();
    assert_eq!(status(&store), GoalStatus::AwaitingRulings);
    assert_eq!(
        store.get_goal(&goal.id).unwrap().item(&ids[0]).unwrap().status,
        ItemStatus::Rejected
    );
}

/// On-disk back-compat: real goals under `~/.witnos/` were written while
/// approval existed. `ruled` was a goal state, `approved` an item state; both
/// must load, and an approved item must come back as what it always was
/// underneath — laid, with its evidence still counting.
#[test]
fn legacy_ruled_and_approved_statuses_still_load() {
    let (store, dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![subjective("calm UI", Origin::AgentInitial)],
            Actor::Agent,
        )
        .unwrap();
    store
        .set_interpretation(&goal.id, &ids[0], "calm = no motion")
        .unwrap();
    store.add_evidence(&goal.id, &ids[0], some_evidence()).unwrap();
    store.reconcile(&goal.id, "s", 1, vec![]).unwrap();
    store
        .record_gate_decision(&goal.id, GateDecisionKind::Release, None)
        .unwrap();

    // Rewrite the file the way the old domain would have left it.
    let path = dir.join(format!("{}.json", goal.id));
    let legacy = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"status\": \"awaiting_rulings\"", "\"status\": \"ruled\"")
        .replace("\"status\": \"laid\"", "\"status\": \"approved\"");
    assert!(legacy.contains("\"ruled\"") && legacy.contains("\"approved\""));
    std::fs::write(&path, legacy).unwrap();

    drop(store);
    let store = Store::open(&dir).unwrap();
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.status, GoalStatus::AwaitingRulings);
    assert_eq!(g.item(&ids[0]).unwrap().status, ItemStatus::Laid);
    // And the loaded item is treated as laid, not as unfinished work.
    assert!(evaluate(&g).release, "{:?}", evaluate(&g).reasons);
}

/// Per-item opt-out: the gate neither blocks on a waived item nor demands
/// evidence for it, and putting it back in scope makes it live again.
#[test]
fn the_gate_ignores_a_waived_item() {
    let (store, _dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    let ids = store
        .lay_items(
            &goal.id,
            vec![
                subjective("the part the user cares about", Origin::UserPreRun),
                subjective("the part they don't", Origin::UserPreRun),
            ],
            Actor::Human,
        )
        .unwrap();
    store.set_interpretation(&goal.id, &ids[0], "how I read it").unwrap();
    store.add_evidence(&goal.id, &ids[0], some_evidence()).unwrap();
    store.reconcile(&goal.id, "s", 2, vec![]).unwrap();

    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(!out.release);
    assert_eq!(out.reasons.len(), 1, "{:?}", out.reasons);

    // Waived: the item stops existing as far as the gate is concerned…
    store.waive_item(&goal.id, &ids[1], true).unwrap();
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.item(&ids[1]).unwrap().status, ItemStatus::Waived);
    let out = evaluate(&g);
    assert!(
        !out.reasons.iter().any(|r| r.contains("the part they don't")),
        "a waived item must not be mentioned at all: {:?}",
        out.reasons
    );
    // …but it IS news for a running agent — otherwise it keeps producing
    // evidence nobody will read — so it moves the yardstick like an edit and
    // lands in the delta the delivery channel computes.
    assert_eq!(g.contract_version, 3);
    assert_eq!(g.last_human_edit_version, 3);
    assert_eq!(g.item(&ids[1]).unwrap().last_edited_version, 3);
    let delta = g.items_since(2);
    assert_eq!(delta.len(), 1, "only the waived item changed");
    assert_eq!(delta[0].id, ids[1]);
    // The one round the bump costs: reconcile, and nothing else is outstanding.
    assert_eq!(out.reasons.len(), 1, "{:?}", out.reasons);
    assert!(out.reasons[0].contains("contract moved"), "{:?}", out.reasons);
    store.reconcile(&goal.id, "s", 3, vec![]).unwrap();
    let out = evaluate(&store.get_goal(&goal.id).unwrap());
    assert!(out.release, "{:?}", out.reasons);

    // Idempotent (a double-clicked toggle is not an error), and a no-op must
    // not creep the version — that would stall the gate for nothing.
    store.waive_item(&goal.id, &ids[1], true).unwrap();
    assert_eq!(store.get_goal(&goal.id).unwrap().contract_version, 3);

    // …and un-waiving puts it back in scope, equally as news.
    store.waive_item(&goal.id, &ids[1], false).unwrap();
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.item(&ids[1]).unwrap().status, ItemStatus::Open);
    assert_eq!(g.contract_version, 4);
    assert_eq!(g.item(&ids[1]).unwrap().last_edited_version, 4);
    assert!(g.items_since(3).iter().any(|i| i.id == ids[1]));
    let out = evaluate(&g);
    assert!(!out.release);
    assert!(
        out.reasons.iter().any(|r| r.contains("the part they don't")),
        "back in scope means back to being blocked on: {:?}",
        out.reasons
    );

    // Un-waiving something that was never waived must not reset laid work, and
    // must not move the version either.
    store.waive_item(&goal.id, &ids[0], false).unwrap();
    let g = store.get_goal(&goal.id).unwrap();
    assert_eq!(g.item(&ids[0]).unwrap().status, ItemStatus::Laid);
    assert_eq!(g.contract_version, 4);

    // Both directions are on the record (the human's own trail).
    let waivers = g
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Waiver { .. }))
        .count();
    assert_eq!(waivers, 2, "one waive, one un-waive");
}

/// Fix 2's whole point: the UI's "the agent hasn't seen your change" signal must
/// answer for the HUMAN's moves only. The agent bumps `contract_version` itself
/// on every item it lays, so a comparison against that number is lit through
/// ordinary mid-run work — an affordance that is always on says nothing.
#[test]
fn only_human_moves_bump_last_human_edit_version() {
    let (store, dir) = temp_store();
    let goal = store.create_goal("demo").unwrap();
    assert_eq!(goal.last_human_edit_version, 0);
    let seen = |s: &Store| {
        let g = s.get_goal(&goal.id).unwrap();
        (g.contract_version, g.last_human_edit_version)
    };

    // The agent lays its initial contract: the yardstick moves, but none of it
    // is news to the agent — it wrote it.
    let agent_ids = store
        .lay_items(
            &goal.id,
            vec![
                subjective("calm UI", Origin::AgentInitial),
                objective("it builds", Actor::Agent),
            ],
            Actor::Agent,
        )
        .unwrap();
    assert_eq!(seen(&store), (2, 0));

    // The agent's own edit of its own item: still not the human's doing.
    store
        .edit_item(
            &goal.id,
            &agent_ids[0],
            Some("calm UI, no motion".into()),
            None,
            None,
            Actor::Agent,
        )
        .unwrap();
    assert_eq!(seen(&store), (3, 0));

    // Everything the agent does to FILL IN an item leaves both alone or moves
    // only the contract — never this.
    store
        .set_interpretation(&goal.id, &agent_ids[0], "calm = nothing animates")
        .unwrap();
    store
        .add_evidence(&goal.id, &agent_ids[0], some_evidence())
        .unwrap();
    store.report_oracle(&goal.id, &agent_ids[1], true).unwrap();
    store
        .add_evidence(&goal.id, &agent_ids[1], some_evidence())
        .unwrap();
    store.reconcile(&goal.id, "s", 3, vec![]).unwrap();
    assert_eq!(seen(&store), (3, 0));

    // The human adds an item: news.
    let mine = store
        .lay_items(
            &goal.id,
            vec![subjective("nothing shifts on hover", Origin::UserMidRun)],
            Actor::Human,
        )
        .unwrap();
    assert_eq!(seen(&store), (4, 4));

    // The human edits one: news.
    store
        .edit_item(
            &goal.id,
            &mine[0],
            Some("nothing shifts on hover, anywhere".into()),
            None,
            None,
            Actor::Human,
        )
        .unwrap();
    assert_eq!(seen(&store), (5, 5));

    // Sending an item back: news (the agent has to re-address it).
    store.reject_item(&goal.id, &agent_ids[0], true).unwrap();
    assert_eq!(seen(&store), (6, 6));

    // Waiving one: news too (stop working on it), both directions.
    store.waive_item(&goal.id, &mine[0], true).unwrap();
    assert_eq!(seen(&store), (7, 7));
    store.waive_item(&goal.id, &mine[0], false).unwrap();
    assert_eq!(seen(&store), (8, 8));

    // A rejected write must leave neither number moved (`mutate` discards the
    // draft) — an agent cannot make the UI claim the human edited something.
    store
        .edit_item(
            &goal.id,
            &mine[0],
            Some("agents must not touch this".into()),
            None,
            None,
            Actor::Agent,
        )
        .unwrap_err();
    assert_eq!(seen(&store), (8, 8));

    // Durable: it is what the UI reads on next open, not a live-only flag.
    drop(store);
    let store = Store::open(&dir).unwrap();
    assert_eq!(seen(&store), (8, 8));
}

/// The pane a session's shell runs in is durable and never clobbered: it is the
/// address a human correction gets typed back to.
#[test]
fn session_pane_survives_a_bind_round_trip() {
    let (store, dir) = temp_store();
    let (g, _) = store
        .create_auto_goal("typed back", "/proj", "sP", "claude-code", Some(7))
        .unwrap();
    assert_eq!(g.sessions[0].pane, Some(7));

    let pane_of = |s: &Store, sid: &str| {
        s.get_goal(&g.id)
            .unwrap()
            .sessions
            .iter()
            .find(|b| b.session_id == sid)
            .expect("binding exists")
            .pane
    };

    // A later bind that doesn't know the pane must not erase it (the gate
    // route binds from the agent's own env, which carries no pane).
    store.bind_session(&g.id, "claude-code", "sP", None).unwrap();
    assert_eq!(pane_of(&store, "sP"), Some(7));

    // A pane learned after the first bind lands on the existing binding.
    store.bind_session(&g.id, "claude-code", "sQ", None).unwrap();
    assert_eq!(pane_of(&store, "sQ"), None);
    store.bind_session(&g.id, "claude-code", "sQ", Some(9)).unwrap();
    assert_eq!(pane_of(&store, "sQ"), Some(9));

    // Durable across a reload…
    drop(store);
    let store = Store::open(&dir).unwrap();
    assert_eq!(pane_of(&store, "sP"), Some(7));

    // …and a goal file written before panes existed still loads.
    std::fs::write(
        dir.join("g-old.json"),
        r#"{"id":"g-old","title":"older than panes","status":"running",
            "contract_version":0,"agent_synced_version":0,
            "sessions":[{"agent":"claude-code","session_id":"s1","bound_at":1}],
            "items":[],"evidence":[],"events":[],"created_at":1}"#,
    )
    .unwrap();
    drop(store);
    let store = Store::open(&dir).unwrap();
    let old = store.get_goal("g-old").expect("legacy goal must load");
    assert_eq!(old.sessions[0].pane, None);
}

#[test]
fn end_turn_accounts_only_running_goals_and_the_gate_revives() {
    let (store, _dir) = temp_store();
    let (g, _) = store
        .create_auto_goal("mid-run clear", "/proj", "sE", "claude-code", None)
        .unwrap();

    // Session ended mid-run → honest accounting.
    assert!(store.end_turn(&g.id).unwrap());
    assert_eq!(store.get_goal(&g.id).unwrap().status, GoalStatus::TurnEndedUnmet);
    // Second end is a no-op.
    assert!(!store.end_turn(&g.id).unwrap());

    // The gate firing again (resume) proves the session is back.
    store
        .record_gate_decision(&g.id, GateDecisionKind::Block, Some("delta".into()))
        .unwrap();
    assert_eq!(store.get_goal(&g.id).unwrap().status, GoalStatus::Running);

    // A parked goal has nothing to account — never touched.
    store
        .record_gate_decision(&g.id, GateDecisionKind::Release, None)
        .unwrap();
    let parked = store.get_goal(&g.id).unwrap().status;
    assert_ne!(parked, GoalStatus::Running);
    assert!(!store.end_turn(&g.id).unwrap());
    assert_eq!(store.get_goal(&g.id).unwrap().status, parked);
}

/// A fresh start means no pane exists, so a goal whose session ran in one of
/// Witnos's own panes has definitely lost its agent — account it rather than
/// leave a goal `running` that nothing will ever come back to.
#[test]
fn startup_accounts_goals_whose_pane_died_with_the_app() {
    let (store, dir) = temp_store();
    let (ours, _) = store
        .create_auto_goal("ran in our pane", "/proj", "s-ours", "claude-code", Some(4))
        .unwrap();
    // Already released: the turn is accounted, there is nothing to end.
    let (parked, _) = store
        .create_auto_goal("released", "/proj", "s-parked", "claude-code", Some(5))
        .unwrap();
    store
        .record_gate_decision(&parked.id, GateDecisionKind::Release, None)
        .unwrap();

    store.account_ended_panes();
    let status = |id: &str| store.get_goal(id).unwrap().status;
    assert_eq!(status(&ours.id), GoalStatus::TurnEndedUnmet);
    assert_eq!(status(&parked.id), GoalStatus::AwaitingRulings);

    // Accounted exactly once, however many times the app is restarted.
    let turn_ends = |id: &str| {
        store
            .get_goal(id)
            .unwrap()
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::TurnEnded { met: false }))
            .count()
    };
    assert_eq!(turn_ends(&ours.id), 1);
    store.account_ended_panes();
    assert_eq!(turn_ends(&ours.id), 1, "a second start must not re-account it");

    // Durable, and still not a one-way door: a resumed session's gate revives it.
    drop(store);
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get_goal(&ours.id).unwrap().status, GoalStatus::TurnEndedUnmet);
    store
        .record_gate_decision(&ours.id, GateDecisionKind::Block, Some("delta".into()))
        .unwrap();
    assert_eq!(store.get_goal(&ours.id).unwrap().status, GoalStatus::Running);
}

/// The exclusion, pinned: a session binding with no pane came from a shell
/// Witnos never spawned (a manual `witnos goal new` in the human's own
/// terminal), and it may still be running right now. Declaring that one over
/// would report a live run as ended — the one error this sweep must not make.
#[test]
fn startup_spares_sessions_witnos_did_not_spawn() {
    let (store, _dir) = temp_store();
    let (theirs, _) = store
        .create_auto_goal("their own terminal", "/proj", "s-theirs", "claude-code", None)
        .unwrap();
    // Two bindings, one of them ours: our pane is gone, so the goal is stale
    // whatever the other session is doing.
    let (mixed, _) = store
        .create_auto_goal("resumed here", "/proj", "s-elsewhere", "claude-code", None)
        .unwrap();
    store
        .bind_session(&mixed.id, "claude-code", "s-in-pane", Some(3))
        .unwrap();

    store.account_ended_panes();

    assert_eq!(
        store.get_goal(&theirs.id).unwrap().status,
        GoalStatus::Running,
        "a session Witnos did not spawn must never be declared dead"
    );
    assert_eq!(
        store.get_goal(&mixed.id).unwrap().status,
        GoalStatus::TurnEndedUnmet,
        "one recorded pane of ours is enough: that pane is gone"
    );
}
