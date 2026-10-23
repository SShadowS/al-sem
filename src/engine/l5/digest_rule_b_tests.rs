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
use crate::engine::l5::snapshot::{compose_snapshot, compose_tree};
use crate::engine::l5::test_support::{
    write_fixture_dep_ambiguous, write_fixture_dep_uncertainty, write_fixture_rule_b,
    write_fixture_symbol_only_dep,
};
use crate::engine::session::digest_plan;
use std::path::Path;
use std::sync::Arc;

fn inputs(ws: &Path) -> Arc<crate::program::inputs::InputSet> {
    crate::program::inputs::InputSet::read(ws, &Default::default())
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
        .effects()
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
        vec!["DB_MODIFY XRec.Write Customer"],
        "the tree groups"
    );
    let model = view.model();
    let cut = compose_snapshot(model);
    let roots: Vec<String> = stable_roots(model, &["CallWrite"]).into_iter().collect();
    let entries = crate::engine::l5::digest::compute_digest_effects_cli(&cut, model, Some(&roots));
    let effects = &entries[0].effects;
    assert_eq!(
        shape(effects),
        vec!["DB_MODIFY - Customer"],
        "the cut keeps its per-resource effect: {effects:?}"
    );
    assert!(effects.iter().all(|e| !e.boundary_group));
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
    assert_eq!(inserts, vec!["DB_INSERT XRec.Raise Ws Log"], "{effects:?}");
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
        vec!["COMMIT XRec.MaybeCommit ", "DB_INSERT - Ws Log"],
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
    let groups: Vec<(&str, bool, usize)> = effects
        .iter()
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
        vec![("XRec.Fill", true, 0), ("XRec.Post", false, 0)],
        "{effects:?}"
    );
    assert!(effects.iter().all(|e| e.boundary_group));
}

/// The cross-app base of `ws`, its relation, and the tree `compose_tree`
/// composes over `rel.cone()` for `scope` (the view's path, unreleased).
fn tree_of(
    model: &Model,
    rel: &ExecRelation,
    scope: ConeScope,
) -> crate::engine::l5::snapshot::CapabilitySnapshot {
    let (mut caps, raw) =
        compose_capabilities(model, rel.cone(), det_cone_inputs(model, rel), scope);
    compose_tree(
        model,
        rel.cone(),
        rel.dependency_calls(),
        &mut caps,
        raw,
        None,
        &HashMap::new(),
    )
}

fn cross_base(ws: &Path) -> crate::engine::l4::capability_cone::R3a5CrossAppBase {
    crate::engine::l4::capability_cone::build_cross_app_base(
        &inputs(ws),
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
    )
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
        &base.rel,
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
        &base.rel,
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
        &crate::program::inputs::InputSet::read(
            tmp.path(),
            &crate::program::inputs::InputNeeds {
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
        view.effects()
            .identities
            .stable_ids
            .iter()
            .any(|id| id.starts_with("bbbb1604-0000-0000-0000-000000000160:")),
        "the table holds the dependency's rows"
    );
}

/// T3 (digest arm, Decision 14): the production scope against an untouched
/// `WitnessScope::All` on the same relation. For every reportable root of
/// every r0 dir (single-app) and of the dependency fixtures (cross-app), the
/// effects and unresolved cones of `Subjects(root)` equal `All`'s.
#[test]
fn t3_digest_scoped_equals_unscoped() {
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
    let (mut roots_seen, mut inherited_seen) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    for (name, dir, single) in &dirs {
        let Ok(target) = AnalysisTarget::build(&inputs(dir), *single, "digest") else {
            continue;
        };
        let model = target.resolved();
        let rel = match &target {
            AnalysisTarget::SingleApp(m) => Arc::new(ExecRelation::single_app(m)),
            AnalysisTarget::CrossApp(b) => Arc::clone(&b.rel),
        };
        let roots = crate::engine::l5::digest::reportable_roots(model);
        let mut all = tree_of(
            model,
            &rel,
            ConeScope {
                derived: false,
                raw: WitnessScope::All,
            },
        );
        let unscoped = digest_effects(&mut all, model, &roots, false);
        for root in &roots {
            let subjects: BTreeSet<String> = model
                .workspace
                .routines
                .iter()
                .filter(|r| &r.stable_routine_id == root)
                .map(|r| r.id.clone())
                .collect();
            let mut scoped_tree = tree_of(
                model,
                &rel,
                ConeScope {
                    derived: false,
                    raw: WitnessScope::Subjects(subjects),
                },
            );
            let scoped = digest_effects(&mut scoped_tree, model, std::slice::from_ref(root), false);
            let mut one = |e: &DigestEffects| {
                let entry = e.entries.iter().find(|x| &x.routine_id == root);
                inherited_seen += entry.map_or(0, |x| {
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
                failures.push(format!("{name}: {root}"));
            }
            roots_seen += 1;
        }
    }
    assert!(
        roots_seen > 100,
        "precondition: roots compared ({roots_seen})"
    );
    assert!(
        inherited_seen > 0,
        "precondition: transitive effects compared"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
