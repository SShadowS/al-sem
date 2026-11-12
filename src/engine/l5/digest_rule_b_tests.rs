//! #160 step 6 Task 2b-2: digest on the view's composition over the cone
//! graph (spec §4.2-§4.7 for digest, §14.3.5 rule B), end to end on written
//! workspaces. The hand-stated rule-B trees (B1-B3, B5, W4b's rule-B arms) are
//! in `digest.rs`'s tests; the CLI readers (T1, T5, the checkpoint, Review
//! Focus 2) are in `tests/cli/`.

use super::*;
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
use crate::engine::l4::exec_relation::ExecRelation;
use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};
use crate::engine::l5::digest::DigestEffectResult;
use crate::engine::l5::digest_cli::ReachWorkers;
use crate::engine::l5::snapshot::{compose_snapshot, compose_tree, source_less_apps};
use crate::engine::l5::test_support::{
    write_fixture_dep_ambiguous, write_fixture_dep_uncertainty, write_fixture_rule_b,
    write_fixture_symbol_only_dep,
};
use crate::engine::session::{JobId, View, digest_plan};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

fn inputs(ws: &Path) -> Arc<crate::program::inputs::WorkspaceInputs> {
    crate::program::inputs::WorkspaceInputs::read(ws, &Default::default())
}

/// The internal → stable ids of the roots named `displays` (workspace rows).
fn stable_roots(model: &Model, displays: &[&str]) -> BTreeSet<String> {
    let roots: BTreeSet<String> = model
        .workspace
        .routines
        .iter()
        .filter(|r| displays.contains(&r.name.as_str()))
        .filter(|r| model.routine_role(r) == crate::program::model::role::Role::Primary)
        .map(|r| r.stable_routine_id.clone())
        .collect();
    assert_eq!(roots.len(), displays.len(), "precondition: {displays:?}");
    roots
}

/// A cross-app digest view of `ws` prepared for the roots `displays`.
fn digest_view(ws: &Path, displays: &[&str]) -> View {
    let target = AnalysisTarget::build(&inputs(ws), false, "digest").expect("cross-app base");
    let mut view = View::new(target);
    let roots = stable_roots(view.model(), displays);
    view.prepare(&digest_plan(
        crate::engine::l5::detectors::default_detectors(),
        roots,
        false,
    ));
    view
}

/// `display`'s digest effects off the view's effects cell.
fn effects_of<'v>(view: &'v View, display: &str) -> &'v [DigestEffectResult] {
    let model = view.model();
    let id = &model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == display)
        .expect("the root")
        .stable_routine_id;
    &view
        .effects(JobId(0))
        .entries
        .iter()
        .find(|e| &e.routine_id == id)
        .unwrap_or_else(|| panic!("no entry for {display}"))
        .effects
}

fn detail<'e>(e: &'e DigestEffectResult, key: &str) -> Option<&'e str> {
    e.detail
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `type boundary resources` per effect (boundary `-` for a per-resource one).
fn shape(effects: &[DigestEffectResult]) -> Vec<String> {
    let mut v: Vec<String> = effects
        .iter()
        .map(|e| {
            format!(
                "{} {} {}",
                e.effect_type,
                detail(e, "boundary").unwrap_or("-"),
                detail(e, "resources")
                    .or(detail(e, "resourceDisplay"))
                    .unwrap_or("-")
            )
        })
        .collect();
    v.sort();
    v
}

/// B4: rule B is off on the cut. The same root reads per-resource effects
/// over `compose_snapshot` (the cut, the ordering cell's tree) and a group
/// over digest's tree.
#[test]
fn b4_rule_b_is_off_on_the_cut() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let view = digest_view(tmp.path(), &["CallWrite"]);
    assert_eq!(
        shape(effects_of(&view, "CallWrite")),
        // `C.Modify();` is a statement: its implicit error (G20) groups too.
        vec![
            "DB_MODIFY Codeunit Dep Rec Lib::Write Customer",
            "ERROR_IMPLICIT Codeunit Dep Rec Lib::Write -"
        ],
        "the tree groups"
    );
    let model = view.model();
    let cut = compose_snapshot(model);
    let roots: Vec<String> = stable_roots(model, &["CallWrite"]).into_iter().collect();
    let entries =
        crate::engine::l5::digest::compute_digest_effects_cli(&cut, model, Some(&roots), false);
    let effects = &entries[0].effects;
    assert_eq!(
        shape(effects),
        vec!["DB_MODIFY - Customer", "ERROR_IMPLICIT - -"],
        "the cut keeps its per-resource effect: {effects:?}"
    );
    assert!(effects.iter().all(|e| !e.boundary_group));
}

/// Context-server design r3 §13 item 3: a read of a tree never changes it.
/// Two reads of ONE tree answer the same, each sharing the tree's coverage
/// and identity table (`Arc` clones, never taken) and, with `tests`, one
/// coverage graph; rule B is a parameter of the read (`BoundaryMode::Off`
/// reads per-resource effects, as the cut does) and the tree keeps its rule.
#[test]
fn a_tree_read_is_a_pure_read() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let target = AnalysisTarget::build(&inputs(tmp.path()), false, "digest").expect("cross-app");
    let model = target.resolved();
    let AnalysisTarget::CrossApp(b) = &target else {
        panic!("cross-app")
    };
    let calls = b.dependency_calls_or_empty();
    let roots: Vec<String> = stable_roots(model, &["CallWrite"]).into_iter().collect();
    let subjects: BTreeSet<String> = model
        .workspace
        .routines
        .iter()
        .filter(|r| roots.contains(&r.stable_routine_id))
        .map(|r| r.id.clone())
        .collect();
    let tree = tree_of(
        model,
        b.rel(),
        calls,
        &source_less_of(&target),
        ConeScope {
            derived: false,
            raw: WitnessScope::Subjects(subjects),
        },
    );
    assert!(
        tree.boundary.is_some(),
        "precondition: digest's tree has rule B"
    );
    assert!(!tree.coverage.is_empty() && !tree.identities.stable_ids.is_empty());
    let read = |mode| {
        digest_effects_each(
            &tree,
            model,
            &[&roots],
            true,
            false,
            false,
            mode,
            ReachWorkers::SERIAL,
        )
    };
    let (first, second) = (read(BoundaryMode::On), read(BoundaryMode::On));
    for e in [&first[0], &second[0]] {
        assert!(
            Arc::ptr_eq(&e.coverage, &tree.coverage),
            "the tree's coverage, shared"
        );
        assert!(Arc::ptr_eq(&e.identities, &tree.identities));
        assert!(e.coverage_graph.is_some(), "tests: the coverage graph");
    }
    assert_eq!(
        shape(&first[0].entries[0].effects),
        shape(&second[0].entries[0].effects),
        "a second read of one tree answers the same"
    );
    let grouped = shape(&first[0].entries[0].effects);
    assert!(
        first[0].entries[0].effects.iter().any(|e| e.boundary_group),
        "rule B groups: {grouped:?}"
    );
    let off = read(BoundaryMode::Off);
    let effects = &off[0].entries[0].effects;
    assert_eq!(
        shape(effects),
        vec!["DB_MODIFY - Customer", "ERROR_IMPLICIT - -"],
        "rule B off"
    );
    assert!(effects.iter().all(|e| !e.boundary_group));
    assert!(tree.boundary.is_some(), "the tree keeps its rule");
    assert_eq!(
        shape(&read(BoundaryMode::On)[0].entries[0].effects),
        grouped,
        "an Off read changes no later On read"
    );
}

/// B6: a primary carrier behind dependency code. `CallRaise` calls the
/// dependency's `Raise`, which raises `OnRaised`; the workspace's
/// `OnRaisedWs` handles it with an insert into `Ws Log`. The cut wins over
/// the carrier: one group at the call into `Raise` with `Ws Log`, and no
/// per-resource effect at the subscriber's operation.
#[test]
fn b6_a_primary_carrier_behind_dependency_code_is_in_the_group() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let view = digest_view(tmp.path(), &["CallRaise"]);
    let effects = effects_of(&view, "CallRaise");
    let inserts: Vec<String> = shape(effects)
        .into_iter()
        .filter(|s| s.starts_with("DB_INSERT"))
        .collect();
    assert_eq!(
        inserts,
        vec!["DB_INSERT Codeunit Dep Rec Lib::Raise Ws Log"],
        "{effects:?}"
    );
}

/// B7: conservative ordering (A2-03). `WriteThenCommit` inserts into `Ws
/// Log`, then calls the dependency's `MaybeCommit` unconditionally, which
/// commits inside an `if`. The `COMMIT` group is `unknown` and carries none of
/// the MUST labels, although the call dominates the return and the write
/// dominates the call.
#[test]
fn b7_a_boundary_commit_carries_no_must_label() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let view = digest_view(tmp.path(), &["WriteThenCommit"]);
    let effects = effects_of(&view, "WriteThenCommit");
    assert_eq!(
        shape(effects),
        // A COMMIT group states no resources (MCP output plan 1d).
        // The statement `L.Insert();` also raises an implicit error (G20).
        vec![
            "COMMIT Codeunit Dep Rec Lib::MaybeCommit -",
            "DB_INSERT - Ws Log",
            "ERROR_IMPLICIT - -"
        ],
        "precondition: the own write and the commit group: {effects:?}"
    );
    let commit = effects
        .iter()
        .find(|e| e.effect_type == "COMMIT")
        .expect("the group");
    assert!(commit.boundary_group);
    assert_eq!(
        commit.conditionality,
        crate::engine::l5::conditionality::UNKNOWN
    );
    let labels: Vec<&str> = commit.scoped_guarantees.iter().map(|g| g.label).collect();
    for must in [
        "COMMIT_DOMINATES_RETURN",
        "WRITE_COMMITTED_BEFORE_RETURN",
        "COMMIT_ON_SUCCESS_PATH",
    ] {
        assert!(!labels.contains(&must), "{must} on the group: {labels:?}");
    }
}

/// B7, the MUST sets (A2-03): a boundary group satisfies no MUST condition of a
/// PRIMARY commit either. `WriteCallCommit` inserts into `Ws Log`, calls
/// `MaybeCommit` (a boundary COMMIT group) and then commits itself: the nearest
/// MUST commit after the write is its own, so its commit is
/// `WRITE_COMMITTED_BEFORE_RETURN` (the boundary commit in the commit set would
/// take that place). `DepWriteThenCommit` calls `Write` (a boundary DB_MODIFY
/// group), then commits: no MUST write precedes its commit, so it is not
/// `WRITE_COMMITTED_BEFORE_RETURN` (the boundary write in the write set would
/// make it so).
#[test]
fn b7_boundary_groups_stay_out_of_a_primary_commits_must_sets() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let view = digest_view(tmp.path(), &["WriteCallCommit", "DepWriteThenCommit"]);
    let own_commit_labels = |root: &str, group: &str| -> Vec<&'static str> {
        let effects = effects_of(&view, root);
        assert!(
            effects
                .iter()
                .any(|e| e.boundary_group && e.effect_type == group),
            "precondition: {root} has a {group} group: {effects:?}"
        );
        let own: Vec<&DigestEffectResult> = effects
            .iter()
            .filter(|e| e.effect_type == "COMMIT" && !e.boundary_group)
            .collect();
        assert_eq!(
            own.len(),
            1,
            "precondition: {root}'s own commit: {effects:?}"
        );
        own[0].scoped_guarantees.iter().map(|g| g.label).collect()
    };
    let w = own_commit_labels("WriteCallCommit", "COMMIT");
    assert!(w.contains(&"WRITE_COMMITTED_BEFORE_RETURN"), "{w:?}");
    let d = own_commit_labels("DepWriteThenCommit", "DB_MODIFY");
    assert!(!d.contains(&"WRITE_COMMITTED_BEFORE_RETURN"), "{d:?}");
}

/// B8: proofs are cut at the boundary and keep their class (A2-04).
/// `FillAndPost` passes a `temporary` Customer into `Fill`, which inserts into
/// it (a producer: only a proof witnesses it), and calls `Post`, which inserts
/// into the physical table. Two `DB_INSERT` groups, the known-temp one at
/// `Fill` and the physical one at `Post`, each with the empty via path.
#[test]
fn b8_proofs_are_cut_at_the_boundary_and_keep_their_class() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let view = digest_view(tmp.path(), &["FillAndPost"]);
    let effects = effects_of(&view, "FillAndPost");
    // The insert groups (each statement insert's `ERROR_IMPLICIT`, G20, is a
    // group of its own at the same crossing).
    let groups: Vec<(&str, bool, usize)> = effects
        .iter()
        .filter(|e| e.effect_type == "DB_INSERT")
        .map(|e| {
            (
                detail(e, "boundary").unwrap_or("-"),
                crate::engine::l5::digest::is_known_temp_state(e.temp_state.as_ref()),
                e.via_paths.iter().map(Vec::len).sum(),
            )
        })
        .collect();
    let mut got = groups.clone();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("Codeunit Dep Rec Lib::Fill", true, 0),
            ("Codeunit Dep Rec Lib::Post", false, 0),
        ],
        "{effects:?}"
    );
    assert!(effects.iter().all(|e| e.boundary_group));
}

/// Final panel F1-S2 (spec §14.3.5 item 4): an event-dispatch boundary group
/// stays in the MAY relations. `WriteThenRaise` inserts into `Ws Log`, then
/// raises `OnWsEvent`. Hand-stated precondition (no compilable AL dispatches
/// from the workspace into a dependency subscriber): on the real cross-app
/// tree, an event-dispatch edge `OnWsEvent → DoHttp` (a dependency routine
/// whose HTTP call is a real cone fact) and `WriteThenRaise`'s inherited HTTP
/// fact through its `OnWsEvent()` call. The `HTTP` group (`ev:` identity: no
/// call site or operation of its own) is placed at that call, in
/// `WriteThenRaise`'s real order frames, so the insert is pending at it:
/// `WRITE_PENDING_AT_EXTERNAL_IO`, never `ORDERING_UNKNOWN`.
#[test]
fn an_event_boundary_group_is_in_the_may_relations() {
    use crate::engine::l5::snapshot::SnapshotGraphEdge;
    let tmp = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_rule_b_event(tmp.path());
    let base = cross_base(tmp.path());
    let model = &base.resolved;
    let id = |name: &str| -> String {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("precondition: {name}"))
            .stable_routine_id
            .clone()
    };
    let (root, publisher, dep, call_http) = (
        id("WriteThenRaise"),
        id("OnWsEvent"),
        id("DoHttp"),
        id("CallHttp"),
    );
    let mut tree = tree_of(
        model,
        base.rel(),
        base.dependency_calls_or_empty(),
        &source_less_apps(&base.dependency_apps),
        ConeScope {
            derived: false,
            raw: WitnessScope::All,
        },
    );
    let (cs, anchor) = tree
        .typed_edges
        .iter()
        .find_map(|e| match e {
            SnapshotGraphEdge::DirectCall {
                from,
                to,
                callsite_id,
                source_anchor,
                ..
            } if *from == root && *to == publisher => {
                Some((callsite_id.clone(), source_anchor.clone()))
            }
            _ => None,
        })
        .expect("precondition: WriteThenRaise calls OnWsEvent");
    std::sync::Arc::make_mut(&mut tree.typed_edges).push(SnapshotGraphEdge::EventDispatch {
        kind: "event-dispatch",
        from: publisher,
        to: dep,
        event_id: "test/Codeunit/50191/event/onwsevent".to_string(),
        publish_anchor: anchor.clone(),
        subscriber_anchor: anchor,
        edge_id: "f1-s2-dispatch".to_string(),
    });
    let mut fact = tree
        .capability_facts
        .iter()
        .find(|f| f.subject() == call_http && f.provenance() == "inherited" && f.op() == "send")
        .expect("precondition: CallHttp's inherited HTTP send fact")
        .fields();
    fact.subject = root.clone();
    fact.witness_callsite_id = Some(cs);
    tree.capability_facts.push(fact.into());
    let entries = crate::engine::l5::digest::compute_digest_effects_cli(
        &tree,
        model,
        Some(std::slice::from_ref(&root)),
        false,
    );
    let effects = &entries[0].effects;
    let http: Vec<&DigestEffectResult> =
        effects.iter().filter(|e| e.effect_type == "HTTP").collect();
    assert_eq!(http.len(), 1, "precondition: one HTTP effect: {effects:?}");
    let g = http[0];
    assert!(g.boundary_group, "precondition: a group: {g:?}");
    // Owner-qualified (MCP output plan 1d).
    assert!(
        detail(g, "boundary").is_some_and(|b| b.ends_with("::DoHttp")),
        "precondition: {g:?}"
    );
    assert!(
        g.evidence_callsite_id.is_none() && g.evidence_operation_id.is_none(),
        "precondition: the ev: boundary has no site of its own: {g:?}"
    );
    let labels: Vec<&str> = g.scoped_guarantees.iter().map(|s| s.label).collect();
    assert!(!labels.contains(&"ORDERING_UNKNOWN"), "{labels:?}");
    assert!(
        labels.contains(&"WRITE_PENDING_AT_EXTERNAL_IO"),
        "{labels:?}"
    );
}

/// Final panel F1-C2: the dependency calls live on the base only until the
/// view's first prepare. Digest's tree reads them there (its unresolved cone
/// for `Go` lists the dependency's to-less call `R.NoSuchMethod`, a ledger row
/// only the dependency calls give), and every plan, with a tree or not, leaves
/// the base without them.
#[test]
fn the_dependency_calls_leave_the_base_in_the_first_prepare() {
    use crate::engine::session::analyze_plan;
    let tmp = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_dep_ledger(tmp.path());
    let calls_of = |v: &View| match v.target() {
        AnalysisTarget::CrossApp(b) => b.dependency_calls.as_ref().map(|c| c.len()),
        AnalysisTarget::SingleApp(_) => panic!("precondition: cross-app"),
    };
    let target =
        AnalysisTarget::build(&inputs(tmp.path()), false, "digest").expect("cross-app base");
    let mut view = View::new(target);
    assert!(
        calls_of(&view).is_some_and(|n| n > 0),
        "precondition: the base holds dependency calls"
    );
    let roots = stable_roots(view.model(), &["Go"]);
    let go = roots.iter().next().unwrap().clone();
    view.prepare(&digest_plan(
        crate::engine::l5::detectors::default_detectors(),
        roots,
        false,
    ));
    assert_eq!(calls_of(&view), None, "digest: released");
    let items: Vec<&str> = view.effects(JobId(0)).unresolved[&go]
        .items
        .iter()
        .map(|i| i.callee_display.as_str())
        .collect();
    assert!(items.contains(&"R.NoSuchMethod"), "{items:?}");

    let target =
        AnalysisTarget::build(&inputs(tmp.path()), false, "analyze").expect("cross-app base");
    let mut view = View::new(target);
    view.prepare(&analyze_plan(
        crate::engine::l5::detectors::default_detectors(),
        true,
    ));
    assert_eq!(calls_of(&view), None, "analyze: released");
}

/// The cross-app base of `ws`, its relation, and the tree `compose_tree`
/// composes over `rel.cone()` for `scope` (the view's path, unreleased), with
/// the dependency calls `calls` (the base's; empty single-app).
fn tree_of(
    model: &Model,
    rel: &ExecRelation,
    calls: &[crate::program::model::calls::CallEdge],
    source_less: &HashSet<String>,
    scope: ConeScope,
) -> crate::engine::l5::snapshot::CapabilitySnapshot {
    let (mut caps, raw) =
        compose_capabilities(model, rel.cone(), det_cone_inputs(model, rel), scope);
    let pd = std::borrow::Cow::Owned(caps.pd.take().unwrap_or_default());
    compose_tree(
        model,
        rel.cone(),
        calls,
        &caps,
        crate::engine::l5::snapshot::TreeRaw::Drain(raw),
        pd,
        &crate::engine::l5::snapshot::TreeApps {
            names: HashMap::new(),
            source_less: source_less.clone(),
        },
    )
}

/// D9's source-less dependency apps of `target` (#160 step 9); none single-app.
fn source_less_of(target: &AnalysisTarget) -> HashSet<String> {
    match target {
        AnalysisTarget::SingleApp(_) => HashSet::new(),
        AnalysisTarget::CrossApp(b) => source_less_apps(&b.dependency_apps),
    }
}

fn cross_base(ws: &Path) -> crate::engine::l4::capability_cone::R3a5CrossAppBase {
    crate::engine::l4::capability_cone::build_cross_app_base(&inputs(ws))
        .unwrap_or_else(|_| panic!("cross-app base"))
}

/// T7 (a), D6: the ledger reads every caller. The dependency's `DepCaller`
/// makes a to-less call (`R.NoSuchMethod()`) and reads the result of a
/// resolved object run (`Ok := Codeunit.Run(..)`). In the tree's
/// `callsiteResolutions` the first has the ledger's own (non-`resolved`)
/// status and the second its dispatch kind and `result_consumed`; the leftover
/// pass would write neither (no row for the first, `direct` with no flag for
/// the second).
#[test]
fn t7a_a_dependency_callers_rows_are_the_ledgers() {
    let tmp = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_dep_ledger(tmp.path());
    let base = cross_base(tmp.path());
    let model = &base.resolved;
    let dep_caller = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "DepCaller")
        .expect("precondition: DepCaller is demanded");
    let tree = tree_of(
        model,
        base.rel(),
        base.dependency_calls_or_empty(),
        &source_less_apps(&base.dependency_apps),
        ConeScope {
            derived: true,
            raw: WitnessScope::None,
        },
    );
    let mut rows: Vec<(String, String, String, Option<bool>)> = tree
        .callsite_resolutions
        .iter()
        .filter(|r| r.from == dep_caller.stable_routine_id)
        .map(|r| {
            (
                r.callee_display.clone(),
                r.dispatch_kind.clone(),
                r.status.clone(),
                r.result_consumed,
            )
        })
        .collect();
    rows.sort();
    let s = |v: &str| v.to_string();
    assert_eq!(
        rows,
        vec![
            (
                s("Codeunit.Run(Dep Rec Lib)"),
                s("codeunit-run"),
                s("resolved"),
                Some(true)
            ),
            (
                s("R.NoSuchMethod"),
                s("method"),
                s("unresolved-receiver-type"),
                None
            ),
        ],
        "{:?}",
        tree.callsite_resolutions
    );
}

/// T7 (b), D6: a resolved call into a body-less (symbol-only) dependency
/// routine is `unfetched-dependency` in the tree's ledger.
#[test]
fn t7b_a_call_into_a_body_less_dependency_row_is_unfetched() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_symbol_only_dep(tmp.path());
    let base = cross_base(tmp.path());
    let model = &base.resolved;
    let go = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Go")
        .expect("Go");
    let write = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Write")
        .expect("precondition: Write is a model row");
    assert!(!write.body_available, "precondition: Write has no body");
    let tree = tree_of(
        model,
        base.rel(),
        base.dependency_calls_or_empty(),
        &source_less_apps(&base.dependency_apps),
        ConeScope {
            derived: true,
            raw: WitnessScope::None,
        },
    );
    let rows: Vec<&str> = tree
        .callsite_resolutions
        .iter()
        .filter(|r| r.from == go.stable_routine_id)
        .map(|r| r.status.as_str())
        .collect();
    assert_eq!(
        rows,
        vec!["unfetched-dependency"],
        "{:?}",
        tree.callsite_resolutions
    );
}

/// T8 (coverage half, A6.2): on the dependency fixture whose only
/// call-site-local uncertainty is in dependency code, the tree's coverage
/// equals the det cell's for every routine (step 5's fold over
/// `rel.uncertainty_all()`), and `Go` is `partial` there.
#[test]
fn t8_the_trees_coverage_is_the_det_cells() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_dep_ambiguous(tmp.path());
    let target =
        AnalysisTarget::build(&inputs(tmp.path()), false, "digest").expect("cross-app base");
    let mut view = View::new(target);
    view.prepare(&crate::engine::session::analyze_plan(
        crate::engine::l5::detectors::default_detectors(),
        true,
    ));
    let model = view.model();
    let tree = tree_of(
        model,
        view.rel(),
        &[],
        &HashSet::new(),
        ConeScope {
            derived: false,
            raw: WitnessScope::All,
        },
    );
    let stable: HashMap<&str, &str> = model
        .workspace
        .routines
        .iter()
        .map(|r| (r.id.as_str(), r.stable_routine_id.as_str()))
        .collect();
    let by_subject: HashMap<&str, &crate::engine::l5::snapshot::SnapshotCoverageRecord> = tree
        .coverage
        .iter()
        .map(|c| (c.subject.as_str(), c))
        .collect();
    let mut compared = 0;
    let mut go_partial = false;
    for (id, s) in &view.det().summaries {
        let Some(c) = s.coverage.as_ref() else {
            continue;
        };
        let t = by_subject
            .get(stable[id.as_str()])
            .unwrap_or_else(|| panic!("no tree coverage for {id}"));
        assert_eq!(
            (t.inherited_status.as_str(), &t.reasons),
            (c.inherited_status.as_str(), &c.reasons),
            "{id}"
        );
        go_partial |= model
            .workspace
            .routines
            .iter()
            .any(|r| &r.id == id && r.name == "Go")
            && c.inherited_status == "partial";
        compared += 1;
    }
    assert!(compared > 2, "precondition: routines compared");
    assert!(
        go_partial,
        "precondition: Go is partial through dependency code"
    );
}

/// T9 (a): digest resolves `--routine` from the identity table it builds
/// from the model before the composition; the root's effects come off the
/// view. The table is unfiltered: the effects' table holds the dependency
/// routines too.
#[test]
fn t9a_a_routine_selector_reads_the_table_digest_builds() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let r = run_digest_pipeline(
        &crate::program::inputs::WorkspaceInputs::read(
            tmp.path(),
            &crate::program::inputs::WorkspaceNeeds {
                fingerprint_rows: true,
                ..Default::default()
            },
        ),
        None,
        Some(vec!["CallWrite".to_string()]),
        None,
        "t",
        true,
        None,
        false,
        false,
    )
    .expect("digest");
    assert_eq!(r.exit_code, 0, "{}", r.json_text);
    let doc: serde_json::Value = serde_json::from_str(&r.json_text).unwrap();
    let roots: Vec<&str> = doc["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["routine"]["display"].as_str())
        .collect();
    assert_eq!(roots, vec!["CallWrite"]);
    let view = digest_view(tmp.path(), &["CallWrite"]);
    assert!(
        view.effects(JobId(0))
            .identities
            .stable_ids
            .iter()
            .any(|id| id.starts_with("bbbb1604-0000-0000-0000-000000000160:")),
        "the table holds the dependency's rows"
    );
}

/// One corpus case of the scoped-read parity arms below (TLS-12): a built
/// target, read by every arm that wants it, and the trees two arms share,
/// each composed at most once. Under nextest every test is its own process,
/// so a build is shared only inside one test: the four arms (each formerly its
/// own test) run in [`scoped_reads_equal_unscoped_on_the_corpus`].
struct Case<'a> {
    name: &'a str,
    single: bool,
    model: &'a Model,
    rel: Arc<ExecRelation>,
    calls: &'a [crate::program::model::calls::CallEdge],
    sl: HashSet<String>,
    /// `WitnessScope::All` (T3 and the streamed read).
    all: std::cell::OnceCell<crate::engine::l5::snapshot::CapabilitySnapshot>,
    /// `WitnessRequest::primary()` (the prove and query arms).
    primary: std::cell::OnceCell<crate::engine::l5::snapshot::CapabilitySnapshot>,
    /// `WitnessRequest::roots({root})`, per root (the prove and query arms).
    roots:
        std::cell::RefCell<HashMap<String, Arc<crate::engine::l5::snapshot::CapabilitySnapshot>>>,
}

impl Case<'_> {
    fn tree(&self, raw: WitnessScope) -> crate::engine::l5::snapshot::CapabilitySnapshot {
        tree_of(
            self.model,
            &self.rel,
            self.calls,
            &self.sl,
            ConeScope {
                derived: false,
                raw,
            },
        )
    }

    fn all(&self) -> &crate::engine::l5::snapshot::CapabilitySnapshot {
        self.all.get_or_init(|| self.tree(WitnessScope::All))
    }

    fn primary(&self) -> &crate::engine::l5::snapshot::CapabilitySnapshot {
        use crate::engine::session::WitnessRequest;
        self.primary
            .get_or_init(|| self.tree(WitnessRequest::primary().scope(self.model)))
    }

    fn scoped_to(&self, root: &str) -> Arc<crate::engine::l5::snapshot::CapabilitySnapshot> {
        use crate::engine::session::WitnessRequest;
        let tree = self.roots.borrow().get(root).cloned();
        tree.unwrap_or_else(|| {
            let t = Arc::new(
                self.tree(WitnessRequest::roots([root.to_string()].into()).scope(self.model)),
            );
            self.roots
                .borrow_mut()
                .insert(root.to_string(), Arc::clone(&t));
            t
        })
    }
}

/// What the arms counted and the failures they found, summed over cases.
#[derive(Default)]
struct Seen {
    t3_roots: usize,
    t3_inherited: usize,
    t3_failures: Vec<String>,
    streamed_roots: usize,
    streamed_dependency_roots: usize,
    streamed_passed: usize,
    prove_roots: usize,
    prove_inherited: usize,
    prove_gaps: usize,
    prove_failures: Vec<String>,
    query_roots: usize,
    query_inherited: usize,
    query_totals: usize,
    query_failures: Vec<String>,
}

impl Seen {
    fn add(&mut self, o: Seen) {
        self.t3_roots += o.t3_roots;
        self.t3_inherited += o.t3_inherited;
        self.t3_failures.extend(o.t3_failures);
        self.streamed_roots += o.streamed_roots;
        self.streamed_dependency_roots += o.streamed_dependency_roots;
        self.streamed_passed += o.streamed_passed;
        self.prove_roots += o.prove_roots;
        self.prove_inherited += o.prove_inherited;
        self.prove_gaps += o.prove_gaps;
        self.prove_failures.extend(o.prove_failures);
        self.query_roots += o.query_roots;
        self.query_inherited += o.query_inherited;
        self.query_totals += o.query_totals;
        self.query_failures.extend(o.query_failures);
    }
}

/// The scoped-read parity arms over one corpus (TLS-12; each arm was its own
/// test, and each still states its own cases, preconditions and failures):
/// every r0 dir with an `app.json` and the four dependency fixtures, each
/// built once per universe. T3 and the streamed read take the r0 dirs
/// single-app and the first three fixtures cross-app, as they always did; the
/// prove and query arms take every dir in both universes. The dirs run on 4
/// threads (the 4 slots the four tests took).
#[test]
fn scoped_reads_equal_unscoped_on_the_corpus() {
    let mut dirs: Vec<(String, std::path::PathBuf, Option<usize>)> = Vec::new();
    let r0 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut r0_dirs: Vec<_> = walkdir::WalkDir::new(&r0)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    r0_dirs.sort();
    for d in r0_dirs {
        dirs.push((d.display().to_string(), d, None));
    }
    let tmps: Vec<tempfile::TempDir> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    write_fixture_rule_b(tmps[0].path());
    write_fixture_dep_ambiguous(tmps[1].path());
    write_fixture_dep_uncertainty(tmps[2].path());
    write_fixture_symbol_only_dep(tmps[3].path());
    for (i, t) in tmps.iter().enumerate() {
        dirs.push((
            format!("dependency fixture {i}"),
            t.path().to_path_buf(),
            Some(i),
        ));
    }
    let seen = std::sync::Mutex::new(Seen::default());
    let chunks: Vec<_> = dirs.chunks(dirs.len().div_ceil(4)).collect();
    std::thread::scope(|s| {
        for chunk in chunks {
            let seen = &seen;
            s.spawn(move || {
                let mut mine = Seen::default();
                for (name, dir, fixture) in chunk {
                    for single in [true, false] {
                        let Ok(target) = AnalysisTarget::build(&inputs(dir), single, "digest")
                        else {
                            continue;
                        };
                        let (rel, calls) = match &target {
                            AnalysisTarget::SingleApp(m) => {
                                (Arc::new(ExecRelation::single_app(m)), &[][..])
                            }
                            AnalysisTarget::CrossApp(b) => {
                                (Arc::clone(b.rel()), b.dependency_calls_or_empty())
                            }
                        };
                        let case = Case {
                            name,
                            single,
                            model: target.resolved(),
                            rel,
                            calls,
                            sl: source_less_of(&target),
                            all: std::cell::OnceCell::new(),
                            primary: std::cell::OnceCell::new(),
                            roots: std::cell::RefCell::new(HashMap::new()),
                        };
                        // T3 and the streamed read: r0 single-app, fixtures 0-2 cross-app.
                        if match fixture {
                            None => single,
                            Some(i) => *i < 3 && !single,
                        } {
                            t3_digest_scoped_equals_unscoped(&case, &mut mine);
                            streamed_effects_equal_one_read(&case, &mut mine);
                        }
                        prove_scoped_equals_unscoped(&case, &mut mine);
                        fingerprint_query_scoped_equals_unscoped(&case, &mut mine);
                    }
                }
                seen.lock().unwrap().add(mine);
            });
        }
    });
    let seen = seen.into_inner().unwrap();
    // T3.
    assert!(
        seen.t3_roots > 100,
        "precondition: roots compared ({})",
        seen.t3_roots
    );
    assert!(
        seen.t3_inherited > 0,
        "precondition: transitive effects compared"
    );
    assert!(
        seen.t3_failures.is_empty(),
        "{}",
        seen.t3_failures.join("\n")
    );
    // The streamed read.
    assert!(
        seen.streamed_roots > 100,
        "precondition: roots compared ({})",
        seen.streamed_roots
    );
    assert!(
        seen.streamed_dependency_roots > 0,
        "precondition: a dependency root (rule B off) is read"
    );
    assert!(
        seen.streamed_passed > 0,
        "precondition: the literal pass marks an entry"
    );
    // Prove.
    assert!(
        seen.prove_roots > 200,
        "precondition: roots compared ({})",
        seen.prove_roots
    );
    assert!(
        seen.prove_inherited > 0,
        "precondition: transitive effects compared"
    );
    assert!(seen.prove_gaps > 0, "precondition: gap counts compared");
    assert!(
        seen.prove_failures.is_empty(),
        "{}",
        seen.prove_failures.join("\n")
    );
    // The fingerprint query.
    assert!(
        seen.query_roots > 200,
        "precondition: roots compared ({})",
        seen.query_roots
    );
    assert!(
        seen.query_inherited > 0,
        "precondition: inherited facts compared"
    );
    assert!(
        seen.query_totals > 0,
        "precondition: classification counts compared"
    );
    assert!(
        seen.query_failures.is_empty(),
        "{}",
        seen.query_failures.join("\n")
    );
}

/// T3 (digest arm, Decision 14): the production scope against an untouched
/// `WitnessScope::All` on the same relation. For every reportable root of
/// every r0 dir (single-app) and of the dependency fixtures (cross-app), the
/// effects and unresolved cones of `Subjects(root)` equal `All`'s.
fn t3_digest_scoped_equals_unscoped(case: &Case, seen: &mut Seen) {
    let (model, name) = (case.model, case.name);
    let roots = crate::engine::l5::digest::reportable_roots(model);
    let unscoped = digest_effects(case.all(), model, &roots, false, false, false);
    for root in &roots {
        let subjects: BTreeSet<String> = model
            .workspace
            .routines
            .iter()
            .filter(|r| &r.stable_routine_id == root)
            .map(|r| r.id.clone())
            .collect();
        let scoped_tree = case.tree(WitnessScope::Subjects(subjects));
        let scoped = digest_effects(
            &scoped_tree,
            model,
            std::slice::from_ref(root),
            false,
            false,
            false,
        );
        let mut one = |e: &DigestEffects| {
            let entry = e.entries.iter().find(|x| &x.routine_id == root);
            seen.t3_inherited += entry.map_or(0, |x| {
                x.effects
                    .iter()
                    .filter(|f| f.provenance == "transitive")
                    .count()
            });
            let cone = &e.unresolved[root];
            format!(
                "{:?}\n{:?}\n{:?}",
                entry.map(|x| &x.effects),
                cone.items,
                cone.traversal
            )
        };
        if one(&scoped) != one(&unscoped) {
            seen.t3_failures.push(format!("{name}: {root}"));
        }
        seen.t3_roots += 1;
    }
}

/// A streamed read (`digest_effects_chunks`, agent-tasks impact follow-up:
/// the main part of a large impact) gives each root exactly what one read
/// over every root gives: its entry after the literal pass and its
/// unresolved cone with the gap and table-event counts, in root order, over
/// batches of 1 and of 3 that share the tree's indexes, carrier cache and
/// suffix trees. Every r0 dir (single-app) and the dependency fixtures
/// (cross-app), every reportable root plus every dependency routine with a
/// body (rule B off for those, G31).
///
/// Break: a batch read with rule B on for every root (`any_dependency`
/// forced false) → the dependency roots' entries differ.
fn streamed_effects_equal_one_read(case: &Case, seen: &mut Seen) {
    let (model, name) = (case.model, case.name);
    let shown = |e: &DigestEffects| -> Vec<String> {
        let mut v: Vec<String> = e.entries.iter().map(|x| format!("{x:?}")).collect();
        v.extend(e.unresolved.iter().map(|(r, c)| {
            format!(
                "{r}\n{:?}\n{:?}\n{:?}\n{:?}",
                c.items, c.traversal, c.gaps_in_cone, c.fires_table_events
            )
        }));
        v
    };
    {
        let mut roots: Vec<String> = crate::engine::l5::digest::root_set(model)
            .into_iter()
            .collect();
        roots.sort();
        let all = case.all();
        seen.streamed_dependency_roots += roots
            .iter()
            .filter(|r| {
                all.boundary
                    .as_ref()
                    .is_some_and(|b| b.dependency_app.contains_key(*r))
            })
            .count();
        let one = digest_effects_each(
            all,
            model,
            &[&roots],
            false,
            true,
            false,
            BoundaryMode::On,
            ReachWorkers::SERIAL,
        )
        .pop()
        .unwrap();
        seen.streamed_passed += one
            .entries
            .iter()
            .filter(|e| {
                e.effects
                    .iter()
                    .any(|f| f.literal_unreachable || f.own_path.is_some())
            })
            .count();
        let whole = shown(&one);
        for chunk in [1, 3] {
            let (mut entries, mut unresolved, mut batches) =
                (Vec::new(), BTreeMap::new(), Vec::new());
            digest_effects_chunks(
                all,
                model,
                &roots,
                chunk,
                false,
                true,
                false,
                ReachWorkers::SERIAL,
                |b, e| {
                    assert!(b.len() <= chunk);
                    batches.extend(b.iter().cloned());
                    entries.extend(e.entries);
                    unresolved.extend(e.unresolved);
                },
            );
            assert_eq!(batches, roots, "{name}: every root once, in order");
            let streamed = DigestEffects {
                entries,
                unresolved,
                ..DigestEffects::default()
            };
            assert_eq!(shown(&streamed), whole, "{name}, batches of {chunk}");
        }
        seen.streamed_roots += roots.len();
    }
}

/// #160 step 9 T25 (trigger and event arms, spec §5, S9-2): the fingerprint
/// query's witness path names its rule-B boundary per kind. No compilable AL
/// gives either edge cross-app (a workspace record operation's dependency
/// trigger is a to-less external target, and a dependency cannot subscribe to a
/// workspace event), so each arm adds the edge to the tree by hand, as
/// `an_event_boundary_group_is_in_the_may_relations` does: `WriteThenRaise` →
/// the dependency's `DoHttp` by an implicit trigger of its `Ws Log` insert
/// (`op:`), or by an event dispatch from its `OnWsEvent` (`ev:`), and
/// `CallHttp`'s inherited HTTP send moved onto `WriteThenRaise`.
/// Assertion: the send's path ends at a `trigger` / `event` boundary with that
/// identity and the operation's / publish site's anchor, and the witness's
/// evidence is that boundary. DISCRIMINATION: B3, `path_boundary` filling
/// call-site edges only, fails both arms.
#[test]
fn fingerprint_witness_boundary_trigger_and_event() {
    use crate::engine::l5::fingerprint_query::{
        FactEvidence, FingerprintFilters, WitnessLimit, fingerprint_query, primary_stable_ids,
    };
    use crate::engine::l5::snapshot::SnapshotGraphEdge;
    for arm in ["trigger", "event"] {
        let tmp = tempfile::tempdir().unwrap();
        crate::engine::l5::test_support::write_fixture_rule_b_event(tmp.path());
        let base = cross_base(tmp.path());
        let model = &base.resolved;
        let id = |name: &str| -> String {
            model
                .workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("precondition: {name}"))
                .stable_routine_id
                .clone()
        };
        let (root, publisher, dep, call_http) = (
            id("WriteThenRaise"),
            id("OnWsEvent"),
            id("DoHttp"),
            id("CallHttp"),
        );
        let mut tree = tree_of(
            model,
            base.rel(),
            base.dependency_calls_or_empty(),
            &source_less_apps(&base.dependency_apps),
            ConeScope {
                derived: false,
                raw: WitnessScope::All,
            },
        );
        let mut fact = tree
            .capability_facts
            .iter()
            .find(|f| f.subject() == call_http && f.provenance() == "inherited" && f.op() == "send")
            .expect("precondition: CallHttp's inherited HTTP send fact")
            .fields();
        fact.subject = root.clone();
        // (the boundary identity, its anchor file and line)
        let (want_id, want_file, want_line) = match arm {
            "trigger" => {
                let op = tree
                    .operation_index
                    .iter()
                    .find(|o| o.routine == root)
                    .expect("precondition: WriteThenRaise's Ws Log insert")
                    .clone();
                std::sync::Arc::make_mut(&mut tree.typed_edges).push(
                    SnapshotGraphEdge::ImplicitTrigger {
                        kind: "implicit-trigger",
                        from: root.clone(),
                        to: dep.clone(),
                        operation_id: op.operation_id.clone(),
                        trigger_kind: "OnInsert".to_string(),
                        fires: crate::engine::l4::combined_graph::TriggerFires::Always,
                        anchor: None,
                        edge_id: "t25-trigger".to_string(),
                    },
                );
                fact.witness_callsite_id = None;
                fact.witness_first_hop_operation_id = Some(op.operation_id.clone());
                (
                    format!("op:{}", op.operation_id),
                    op.source_file.clone(),
                    op.start_line,
                )
            }
            _ => {
                let (cs, anchor) = tree
                    .typed_edges
                    .iter()
                    .find_map(|e| match e {
                        SnapshotGraphEdge::DirectCall {
                            from,
                            to,
                            callsite_id,
                            source_anchor,
                            ..
                        } if *from == root && *to == publisher => {
                            Some((callsite_id.clone(), source_anchor.clone()))
                        }
                        _ => None,
                    })
                    .expect("precondition: WriteThenRaise calls OnWsEvent");
                let event_id = "test/Codeunit/50191/event/onwsevent".to_string();
                std::sync::Arc::make_mut(&mut tree.typed_edges).push(
                    SnapshotGraphEdge::EventDispatch {
                        kind: "event-dispatch",
                        from: publisher.clone(),
                        to: dep.clone(),
                        event_id: event_id.clone(),
                        publish_anchor: anchor.clone(),
                        subscriber_anchor: anchor.clone(),
                        edge_id: "t25-dispatch".to_string(),
                    },
                );
                fact.witness_callsite_id = Some(cs);
                (
                    format!("ev:{event_id}"),
                    anchor.source_unit_id.clone(),
                    anchor.range.start_line,
                )
            }
        };
        tree.capability_facts.push(fact.into());
        let primary = primary_stable_ids(model);
        let result = fingerprint_query(
            &tree,
            &FingerprintFilters {
                roots: None,
                routine_selectors: vec![root.clone()],
                include_inherited: true,
                witness_limit: WitnessLimit::Capped(3),
            },
            Some(&BTreeSet::from([root.clone()])),
            false,
            &crate::engine::l4::effect_query_cli::OwnerIndex::build(model),
            &|id| primary.contains(id),
        );
        assert!(
            result.diagnostics.is_empty(),
            "{arm}: {:?}",
            result.diagnostics
        );
        let block = &result.blocks[0];
        let (fi, _) = block
            .rendered_facts
            .iter()
            .enumerate()
            .find(|(_, f)| f.op() == "send" && f.provenance() == "inherited")
            .unwrap_or_else(|| panic!("{arm}: precondition: the send fact is rendered"));
        let w = block
            .witnesses
            .iter()
            .find(|w| w.fact_index == fi)
            .expect("its witness");
        let b = w
            .paths
            .iter()
            .find_map(|p| p.boundary.as_ref())
            .unwrap_or_else(|| {
                panic!(
                    "{arm}: a path names its boundary: {:?} {:?}",
                    w.paths, w.diagnostics
                )
            });
        assert_eq!(b.kind.as_str(), arm, "{arm}: {b:?}");
        assert_eq!(b.id, want_id, "{arm}");
        assert_eq!(
            (b.file.as_deref(), b.line),
            (Some(want_file.as_str()), Some(want_line)),
            "{arm}: the anchor"
        );
        assert!(
            matches!(&w.evidence, Some(FactEvidence::Boundary(e)) if e == b),
            "{arm}: the evidence is the boundary: {:?}",
            w.evidence
        );
    }
}

/// #160 step 9 T17 (prove arm, 6A-9): prove reads one root's effects,
/// unresolved cone and gap count off a tree whose witness request is
/// `WitnessRequest::roots({root})` (`ProveJob`'s `prove_plan`). For every
/// reportable root of every r0 dir and of the dependency fixtures, in both
/// universes, those equal the same root's under `WitnessRequest::primary()`
/// (every primary routine a subject). The subjects come from
/// `WitnessRequest::scope`, the view's own mapping. DISCRIMINATION:
/// a roots request's scope dropping its subjects (`.take(0)` in `scope`) loses the
/// root's transitive effects and fails it.
fn prove_scoped_equals_unscoped(case: &Case, seen: &mut Seen) {
    let (model, name, single) = (case.model, case.name, case.single);
    let roots = crate::engine::l5::digest::reportable_roots(model);
    let unscoped = digest_effects(case.primary(), model, &roots, false, true, false);
    for root in &roots {
        let scoped_tree = case.scoped_to(root);
        let scoped = digest_effects(
            &scoped_tree,
            model,
            std::slice::from_ref(root),
            false,
            true,
            false,
        );
        let mut one = |e: &DigestEffects| {
            let entry = e.entries.iter().find(|x| &x.routine_id == root);
            seen.prove_inherited += entry.map_or(0, |x| {
                x.effects
                    .iter()
                    .filter(|f| f.provenance == "transitive")
                    .count()
            });
            let cone = &e.unresolved[root];
            seen.prove_gaps += cone.gaps_in_cone.expect("prove's plan counts the gaps");
            format!(
                "{:?}\n{:?}\n{:?}\n{:?}",
                entry.map(|x| &x.effects),
                cone.items,
                cone.traversal,
                cone.gaps_in_cone
            )
        };
        if one(&scoped) != one(&unscoped) {
            seen.prove_failures
                .push(format!("{name} (single-app {single}): {root}"));
        }
        seen.prove_roots += 1;
    }
}

/// #160 step 9 T17 (query arm, 6A-9): `fingerprint --routine R` reads a tree
/// whose witness request is `WitnessRequest::roots({R})` (the pool). For every
/// root classification of every r0 dir and of the dependency fixtures, in both
/// universes, the query's block for `--routine R` (its facts, coverage,
/// witnesses and evidence) equals the same query's over a tree with
/// `WitnessRequest::primary()`. DISCRIMINATION: a roots request's scope dropping its
/// subjects (`.take(0)` in `scope`) loses the inherited facts and fails it.
/// Final panel O4: in each universe the model's `totalClassifications`
/// (`selector_failure_result`, the count a failing selector prints with no
/// tree) equals the tree's. DISCRIMINATION: dropping its primary filter
/// fails it.
fn fingerprint_query_scoped_equals_unscoped(case: &Case, seen: &mut Seen) {
    use crate::engine::l5::fingerprint_query::{
        FingerprintFilters, WitnessLimit, fingerprint_query, primary_stable_ids,
    };
    let (model, name, single) = (case.model, case.name, case.single);
    let owners = crate::engine::l4::effect_query_cli::OwnerIndex::build(model);
    let primary = primary_stable_ids(model);
    let query = |tree: &crate::engine::l5::snapshot::CapabilitySnapshot, r: &str| {
        let res = fingerprint_query(
            tree,
            &FingerprintFilters {
                roots: None,
                routine_selectors: vec![r.to_string()],
                include_inherited: true,
                witness_limit: WitnessLimit::Capped(3),
            },
            Some(&BTreeSet::from([r.to_string()])),
            false,
            &owners,
            &|id| primary.contains(id),
        );
        (format!("{:?}\n{:?}", res.blocks, res.diagnostics), res)
    };
    let all = case.primary();
    // Final panel O4: a failing selector's count, read off the model
    // with no tree, is the count the tree gives.
    let tree_total = query(all, "").1.total_classifications;
    let model_total =
        crate::engine::l5::fingerprint_query::selector_failure_result(model, vec![], false)
            .total_classifications;
    seen.query_totals += tree_total;
    if tree_total != model_total {
        seen.query_failures.push(format!(
            "{name} (single-app {single}): totalClassifications {model_total} off the \
             model, {tree_total} off the tree"
        ));
    }
    let roots: BTreeSet<String> =
        model
            .root_classifications
            .iter()
            .filter_map(|rc| {
                model.workspace.routines.iter().find(|r| {
                    r.id == rc.routine_id && primary.contains(r.stable_routine_id.as_str())
                })
            })
            .map(|r| r.stable_routine_id.clone())
            .collect();
    for root in &roots {
        let scoped_tree = case.scoped_to(root);
        let (got, _) = query(&scoped_tree, root);
        // Counted on the unscoped side, so a scope that drops the
        // subject reaches the comparison.
        let (want, res) = query(all, root);
        seen.query_inherited += res
            .blocks
            .iter()
            .flat_map(|b| &b.rendered_facts)
            .filter(|f| f.provenance() == "inherited")
            .count();
        if got != want {
            seen.query_failures
                .push(format!("{name} (single-app {single}): {root}"));
        }
        seen.query_roots += 1;
    }
}

/// #160 step 9 T11 (spec §5): `fingerprint --inventory-only` (cross-app, on the
/// rule-B fixture, whose source dependency `Dep Rec Lib` writes `Customer`)
/// is the inventory envelope of `assemble_full_snapshot`'s five keys over a
/// tree composed over an empty request. Precondition: that tree holds no
/// inherited fact. DISCRIMINATION: giving the plan every primary routine keeps
/// the dependency identities its inherited facts reference, and fails it (its
/// counter half, `build.witness_pass` 0, is `cli_build_counters`'
/// `fingerprint_inventory_only`).
#[test]
fn inventory_reads_a_tree_with_no_raw_facts() {
    use crate::engine::l5::fingerprint_cli::{
        FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
    };
    use crate::engine::l5::snapshot_full::{
        FullSnapshotOptions, SnapshotKeys, SnapshotUniverse, assemble_full_snapshot,
        build_inventory_envelope,
    };
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_rule_b(tmp.path());
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        tmp.path(),
        &crate::program::inputs::WorkspaceNeeds {
            fingerprint_rows: true,
            ..Default::default()
        },
    );
    let got = match run_fingerprint_pipeline(&FingerprintOptions {
        inputs: &inputs,
        driver_version: "t",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: None,
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: false,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: true,
        no_roots_config: false,
        single_app: false,
    })
    .expect("inventory runs")
    .output
    {
        FingerprintOutput::Text(t) => t,
        _ => panic!("inventory output is text"),
    };
    let target = AnalysisTarget::build(&inputs, false, "fingerprint").expect("cross-app");
    let AnalysisTarget::CrossApp(b) = &target else {
        unreachable!()
    };
    let model = &b.resolved;
    let tree = tree_of(
        model,
        b.rel(),
        b.dependency_calls_or_empty(),
        &source_less_apps(&b.dependency_apps),
        ConeScope {
            derived: false,
            // Hand-stated: no subject (what an empty request must mean).
            raw: WitnessScope::Subjects(BTreeSet::new()),
        },
    );
    assert!(
        tree.capability_facts
            .iter()
            .all(|f| f.provenance() != "inherited"),
        "precondition: no inherited fact in a tree of an empty request"
    );
    let keys = assemble_full_snapshot(
        model,
        tree,
        crate::engine::gate::cbor::CborValue::Array(vec![]),
        &FullSnapshotOptions {
            inputs: &inputs,
            driver_version: "t",
            deterministic: true,
            roots_config_ignored: false,
            universe: SnapshotUniverse::of(&target),
        },
        SnapshotKeys::Inventory,
    );
    assert_eq!(keys.len(), 5, "the five inventory keys");
    let want = build_inventory_envelope(
        &crate::engine::gate::cbor::CborValue::Map(keys),
        model,
        "t",
        true,
    );
    assert_eq!(got, want);
}

/// TLS-17: the unresolved cones read the snapshot's indexes built once per
/// read (`ConeIndexes`, in `FinishCx`), and give what the verbatim pre-TLS-17
/// search (`unresolved_cone_legacy`) gives. Every r0 dir (single-app) and the
/// dependency fixtures (cross-app), every root:
/// - the raw search: one `ConeIndexes` per tree reused across its roots in
///   root order, against the oracle, every field (`visited_ids` as a set);
/// - through the real caller: `FinishCx::finish`'s cones (after placement,
///   the literal filtering and the counts) against the same `finish` taking
///   each raw cone from the oracle (`LEGACY_CONES`);
/// - built once: one read, and one streamed read of several batches, each
///   build the indexes once (`INDEX_BUILDS`).
///
/// Preconditions: more than 100 roots; a cone with items; a tree with two
/// roots whose cones differ.
/// DISCRIMINATION: `FinishCx::raw_cone` calling `unresolved_cone` per root
/// (the pre-TLS-17 path) fails "built once"; answering every root with the
/// first root's raw cone fails both comparisons.
#[test]
fn cones_off_shared_indexes_equal_the_legacy_search() {
    use crate::engine::l5::digest_cli::LEGACY_CONES;
    use crate::engine::l5::unresolved_cone::{
        ConeIndexes, DEFAULT_MAX_DEPTH, DEFAULT_MAX_VISITED, INDEX_BUILDS, unresolved_cone_in,
        unresolved_cone_legacy,
    };
    let mut dirs: Vec<(String, std::path::PathBuf, bool)> = Vec::new();
    let r0 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut r0_dirs: Vec<_> = walkdir::WalkDir::new(&r0)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    r0_dirs.sort();
    for d in r0_dirs {
        dirs.push((d.display().to_string(), d, true));
    }
    let tmps: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    write_fixture_rule_b(tmps[0].path());
    write_fixture_dep_ambiguous(tmps[1].path());
    write_fixture_dep_uncertainty(tmps[2].path());
    for (i, t) in tmps.iter().enumerate() {
        dirs.push((
            format!("dependency fixture {i}"),
            t.path().to_path_buf(),
            false,
        ));
    }
    let shown = |c: &UnresolvedCone| {
        format!(
            "{:?}\n{}\n{}\n{:?}\n{:?}\n{:?}",
            c.items,
            c.literal_dead,
            c.literal_narrowed,
            c.traversal,
            c.gaps_in_cone,
            c.fires_table_events
        )
    };
    let builds = || INDEX_BUILDS.with(std::cell::Cell::get);
    let (mut roots_seen, mut with_items, mut differing_pair, mut streamed) =
        (0usize, 0usize, false, false);
    let mut failures: Vec<String> = Vec::new();
    for (name, dir, single) in &dirs {
        let Ok(target) = AnalysisTarget::build(&inputs(dir), *single, "digest") else {
            continue;
        };
        let model = target.resolved();
        let sl = source_less_of(&target);
        let (rel, calls) = match &target {
            AnalysisTarget::SingleApp(m) => (Arc::new(ExecRelation::single_app(m)), &[][..]),
            AnalysisTarget::CrossApp(b) => (Arc::clone(b.rel()), b.dependency_calls_or_empty()),
        };
        let tree = tree_of(
            model,
            &rel,
            calls,
            &sl,
            ConeScope {
                derived: false,
                raw: WitnessScope::All,
            },
        );
        let mut roots: Vec<String> = crate::engine::l5::digest::root_set(model)
            .into_iter()
            .collect();
        roots.sort();
        if roots.is_empty() {
            continue;
        }
        // The raw search, one index set reused across the tree's roots.
        let idx = ConeIndexes::new(&tree);
        let mut raw_items: Vec<String> = Vec::new();
        for r in &roots {
            let got = unresolved_cone_in(&tree, &idx, r, DEFAULT_MAX_DEPTH, DEFAULT_MAX_VISITED);
            let want = unresolved_cone_legacy(&tree, r, DEFAULT_MAX_DEPTH, DEFAULT_MAX_VISITED);
            if got.visited_ids != want.visited_ids
                || format!("{:?}\n{:?}", got.items, got.traversal)
                    != format!("{:?}\n{:?}", want.items, want.traversal)
            {
                failures.push(format!("{name}: raw cone of {r}"));
            }
            with_items += usize::from(!want.items.is_empty());
            raw_items.push(format!("{:?}", want.items));
        }
        differing_pair |= raw_items.iter().any(|x| *x != raw_items[0]);
        // Through the real caller: one read builds the indexes once.
        INDEX_BUILDS.with(|n| n.set(0));
        let prod = digest_effects(&tree, model, &roots, false, true, false);
        if builds() != 1 {
            failures.push(format!("{name}: {} index builds in one read", builds()));
        }
        LEGACY_CONES.with(|c| c.set(true));
        let legacy = digest_effects(&tree, model, &roots, false, true, false);
        LEGACY_CONES.with(|c| c.set(false));
        let all = |e: &DigestEffects| -> Vec<String> {
            e.unresolved
                .iter()
                .map(|(r, c)| format!("{r}\n{}", shown(c)))
                .collect()
        };
        if all(&prod) != all(&legacy) {
            failures.push(format!("{name}: finished cones differ"));
        }
        // A streamed read of several batches builds them once too.
        if roots.len() >= 3 {
            INDEX_BUILDS.with(|n| n.set(0));
            digest_effects_chunks(
                &tree,
                model,
                &roots,
                1,
                false,
                true,
                false,
                ReachWorkers::SERIAL,
                |_, _| {},
            );
            if builds() != 1 {
                failures.push(format!(
                    "{name}: {} index builds in one streamed read of {} batches",
                    builds(),
                    roots.len()
                ));
            }
            streamed = true;
        }
        roots_seen += roots.len();
    }
    assert!(
        roots_seen > 100,
        "precondition: roots compared ({roots_seen})"
    );
    assert!(with_items > 0, "precondition: a cone with items");
    assert!(
        differing_pair,
        "precondition: a tree with two roots whose cones differ"
    );
    assert!(streamed, "precondition: a streamed read of several batches");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The corpus's trees (r0 dirs single-app, the dependency fixtures
/// cross-app), each with its roots, sorted.
fn corpus_trees() -> Vec<(
    String,
    AnalysisTarget,
    crate::engine::l5::snapshot::CapabilitySnapshot,
    Vec<String>,
)> {
    let r0 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<(String, std::path::PathBuf, bool)> = walkdir::WalkDir::new(&r0)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .map(|p| (p.display().to_string(), p, true))
        .collect();
    dirs.sort();
    let tmps: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    write_fixture_rule_b(tmps[0].path());
    write_fixture_dep_ambiguous(tmps[1].path());
    write_fixture_dep_uncertainty(tmps[2].path());
    for (i, t) in tmps.iter().enumerate() {
        dirs.push((
            format!("dependency fixture {i}"),
            t.path().to_path_buf(),
            false,
        ));
    }
    let mut out = Vec::new();
    for (name, dir, single) in dirs {
        let Ok(target) = AnalysisTarget::build(&inputs(&dir), single, "digest") else {
            continue;
        };
        let model = target.resolved();
        let sl = source_less_of(&target);
        let (rel, calls) = match &target {
            AnalysisTarget::SingleApp(m) => (Arc::new(ExecRelation::single_app(m)), &[][..]),
            AnalysisTarget::CrossApp(b) => (Arc::clone(b.rel()), b.dependency_calls_or_empty()),
        };
        let tree = tree_of(
            model,
            &rel,
            calls,
            &sl,
            ConeScope {
                derived: false,
                raw: WitnessScope::All,
            },
        );
        let mut roots: Vec<String> = crate::engine::l5::digest::root_set(model)
            .into_iter()
            .collect();
        roots.sort();
        if !roots.is_empty() {
            out.push((name, target, tree, roots));
        }
    }
    out
}

/// TLS-22: the literal pass answers the same at any worker count, over the
/// corpus: every passed entry, in its `Debug` form (no hash fields), with 1,
/// 2 and 8 workers, and a one-root read. Threads hash with different seeds,
/// so a dependence on a `HashMap`'s order that a serial run hides fails here
/// too. The serial run is the new code as well: the goldens are the oracle
/// for what is wrong in every order.
///
/// Each probe-holding test in this file uses pool sizes no other test uses
/// (5, 6; 3; 9; the resident route test 7): a pool is shared by every pass
/// of its size in the process, so two tests holding a first wave in one pool
/// under a plain `cargo test` could starve each other (review, Opus I1).
#[test]
fn the_literal_pass_answers_the_same_at_any_worker_count() {
    let (mut boundary, mut wide) = (false, false);
    let probe = Arc::new(PassProbe::default());
    let mut failures: Vec<String> = Vec::new();
    for (name, target, tree, roots) in corpus_trees() {
        let model = target.resolved();
        let read = |roots: &[String], n: usize| -> Vec<String> {
            passed_root_entries(
                &tree,
                model,
                roots,
                false,
                BoundaryMode::On,
                ReachWorkers::new(n).with_probe(Arc::clone(&probe)),
            )
            .iter()
            .map(|e| format!("{e:?}"))
            .collect()
        };
        let serial = read(&roots, 1);
        boundary |= serial.iter().any(|e| e.contains("boundary_group: true"));
        wide |= roots.len() > 8;
        for n in [2, 8] {
            if read(&roots, n) != serial {
                failures.push(format!("{name}: {n} workers"));
            }
        }
        if read(&roots[..1], 8) != read(&roots[..1], 1) {
            failures.push(format!("{name}: one root"));
        }
    }
    assert!(boundary, "precondition: an entry with a boundary group");
    assert!(wide, "precondition: a tree with more roots than workers");
    assert!(
        probe.suffix_hits.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "precondition: a suffix tree served again (shared, not rebuilt)"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// TLS-22: the default worker count is capped by the cores: a one-core host
/// runs every pass serially (no pool), a large one runs `REACH_WORKERS`.
///
/// DISCRIMINATION (2026-11-12): `capped_by_cores` without `.min(cores)`
/// fails the one-core case (8 against 1); restored, passes.
#[test]
fn the_default_worker_count_is_capped_by_the_cores() {
    assert_eq!(capped_by_cores(1), 1, "one core: serial");
    assert_eq!(capped_by_cores(4), 4);
    assert_eq!(capped_by_cores(22), REACH_WORKERS);
    assert_eq!(capped_by_cores(0), 1, "an unknown count: serial");
}

/// TLS-22, owner decision 2026-11-12 (q-20261112T022816-833a1bd9): while
/// another pass sharing the counter runs (another ask), a pass runs one root
/// at a time. Started beside another, it is serial; when another starts
/// while it runs (the probe adds one to the counter as its third root
/// starts, the first wave of 3 in flight), every root it starts after that
/// runs alone.
///
/// DISCRIMINATION (2026-11-12): without the workers' stop (`w > 0 &&
/// others()`) it fails (3 roots at once after the other pass started,
/// against 1); restored, passes.
#[test]
fn beside_another_pass_the_pass_runs_one_root_at_a_time() {
    let (name, target, tree, roots) = corpus_trees()
        .into_iter()
        .max_by_key(|(_, _, _, r)| r.len())
        .expect("a corpus tree");
    assert!(
        roots.len() > 8,
        "precondition: {name} has more than 8 roots"
    );
    let run = |w: ReachWorkers| {
        passed_root_entries(&tree, target.resolved(), &roots, false, BoundaryMode::On, w);
    };
    let held = |n| Some((n, std::time::Duration::from_secs(10)));
    // Another pass starts while this one runs.
    let w = ReachWorkers::new(3);
    let counter = Arc::clone(w.passes.as_ref().unwrap());
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = held(3);
    *probe.another_at.lock().unwrap() = Some((2, Arc::clone(&counter)));
    run(w.with_probe(Arc::clone(&probe)));
    assert_eq!(probe.max(), 3, "precondition: alone, 3 roots at once");
    assert!(
        probe.started.load(std::sync::atomic::Ordering::SeqCst) > 4,
        "precondition: roots started after the other pass"
    );
    assert_eq!(
        probe.max_after.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "after the other pass started: one root at a time"
    );
    // Started while another runs (the counter still counts it).
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = Some((3, std::time::Duration::from_millis(200)));
    run(w.with_probe(Arc::clone(&probe)));
    assert_eq!(probe.max(), 1, "started beside another: serial");
    // The default setting shares one counter for the process.
    let (a, b) = (default_reach_workers(), default_reach_workers());
    assert!(Arc::ptr_eq(
        a.passes.as_ref().unwrap(),
        b.passes.as_ref().unwrap()
    ));
}

/// TLS-22: the pass runs exactly `n` roots at once with `n` workers (the
/// first wave held until `n` are in flight, at most 10 s), on a pool of `n`
/// threads, and one at a time serially.
///
/// DISCRIMINATION (2026-11-11): `reach_pool(n)` building `REACH_WORKERS`
/// threads whatever `n` fails (the pool's size, and with 2 workers 8 roots
/// at once); restored, passes.
#[test]
fn the_literal_pass_runs_exactly_its_worker_count_at_once() {
    let (name, target, tree, roots) = corpus_trees()
        .into_iter()
        .max_by_key(|(_, _, _, r)| r.len())
        .expect("a corpus tree");
    let model = target.resolved();
    assert!(
        roots.len() > 8,
        "precondition: {name} has more than 8 roots"
    );
    for n in [1, 5, 6] {
        let probe = Arc::new(PassProbe::default());
        *probe.first_wave.lock().unwrap() = Some((n, std::time::Duration::from_secs(10)));
        let entries = passed_root_entries(
            &tree,
            model,
            &roots,
            false,
            BoundaryMode::On,
            ReachWorkers::new(n).with_probe(Arc::clone(&probe)),
        );
        assert!(
            entries.iter().flatten().count() > n,
            "precondition: more entries than workers"
        );
        assert_eq!(probe.max(), n, "{n} workers: roots in flight at once");
        if n > 1 {
            assert_eq!(reach_pool(n).current_num_threads(), n, "the pool's size");
        }
    }
}

/// TLS-22: two passes running at once on separate counters (so neither
/// throttles the other) share the ONE pool of their worker count: their
/// roots in flight together never pass the pool's 9 threads (two pools
/// would reach 18 while both first waves are held), both overlap, and each
/// answers as serially.
///
/// DISCRIMINATION (2026-11-12): `reach_pool` building a fresh pool on every
/// call fails (18 at once); restored, passes.
#[test]
fn two_passes_at_once_share_the_pool() {
    let trees: Vec<_> = corpus_trees()
        .into_iter()
        .filter(|(_, _, _, r)| r.len() > 9)
        .take(2)
        .collect();
    assert_eq!(
        trees.len(),
        2,
        "precondition: two trees with more than 9 roots"
    );
    let read = |(_, target, tree, roots): &(
        String,
        AnalysisTarget,
        crate::engine::l5::snapshot::CapabilitySnapshot,
        Vec<String>,
    ),
                w: ReachWorkers| {
        format!(
            "{:?}",
            passed_root_entries(tree, target.resolved(), roots, false, BoundaryMode::On, w)
        )
    };
    let serial: Vec<String> = trees
        .iter()
        .map(|t| read(t, ReachWorkers::SERIAL))
        .collect();
    // One probe for both passes: it counts their roots together; each holds
    // its first roots until 18 are in flight, at most 300 ms.
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = Some((18, std::time::Duration::from_millis(300)));
    let together: Vec<String> = std::thread::scope(|s| {
        let hs: Vec<_> = trees
            .iter()
            .map(|t| {
                let w = ReachWorkers::new(9).with_probe(Arc::clone(&probe));
                s.spawn(move || read(t, w))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(together, serial);
    assert!(
        probe.max() <= 9,
        "both passes in one pool of 9 threads: {} at once",
        probe.max()
    );
    assert!(probe.max() >= 2, "precondition: roots ran at once");
}

/// TLS-22: from inside a rayon worker the pass runs serially on that thread
/// (`install` there would let the blocked worker run other jobs, a deadlock
/// when one takes a lock the thread holds), and answers the same.
///
/// DISCRIMINATION (2026-11-11): without the rayon-worker check the pass
/// runs 2 roots at once there: fails; restored, passes.
#[test]
fn inside_a_rayon_worker_the_pass_runs_serially() {
    let (_, target, tree, roots) = corpus_trees()
        .into_iter()
        .find(|(_, _, _, r)| r.len() > 2)
        .expect("a tree with three roots");
    let read = |w: ReachWorkers| {
        format!(
            "{:?}",
            passed_root_entries(&tree, target.resolved(), &roots, false, BoundaryMode::On, w)
        )
    };
    let serial = read(ReachWorkers::SERIAL);
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = Some((2, std::time::Duration::from_millis(200)));
    let outer = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let inside = outer.install(|| read(ReachWorkers::new(2).with_probe(Arc::clone(&probe))));
    assert_eq!(inside, serial);
    assert_eq!(probe.max(), 1, "one root at a time inside a rayon worker");
}
