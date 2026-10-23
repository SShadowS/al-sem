//! #160 step 5 acceptance tests (spec
//! `docs/superpowers/specs/2026-10-22-issue-160-step5-coverage-policy-design.md`
//! §5): T1 (spans and d9), T3 (context == snapshot on r0), T4 (the dependency
//! fixture against its own Cone), T5 (one fold, two cross-app producers) and
//! T9 (d2's use and its budget cost). The CLI readers (T1b, T2, T6-T8) are in
//! `tests/cli/cli_step5_coverage_fold.rs`.
//!
//! Every test asserts its hand-stated preconditions first. They read inputs
//! the fold does not touch (uncertainty edges, spans, edge order, cone facts),
//! so on the base they hold and the test is red only for the missing fold.

use super::*;
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::capability_cone::{
    CoverageRecord, R3a5CrossAppBase, build_cross_app_base, build_r3a3_source_only_base,
};
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::registry::substrate;
use crate::engine::l5::test_support::write_fixture_dep_ambiguous;
use crate::engine::l5::transaction_spans::SeedKind;
use crate::engine::session::{View, analyze_plan};
use crate::program::body::features::PCallee;
use crate::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

const APP: &str = "55555555-0000-0000-0000-000000160005";

/// The four call-site-local kinds the fold maps, written out (not read from
/// `is_callsite_local_kind`).
const FOUR_KINDS: [&str; 4] = [
    "ambiguous-overload",
    "external-target",
    "interface-open-world",
    "member-not-found",
];

/// Spec §5 fixture A: `Helper` makes the ambiguous `O(L)` call (§0 item 7)
/// and one `Modify` on a physical table, no Commit; `Root` calls `Helper`,
/// then `Commit()`; `Entry` calls `Root`. NOT compilable AL (a Codeunit is
/// neither a `Text` nor an `S5 ILog`); it works because our overload resolver
/// is lenient, and T1's precondition asserts the ambiguous edge exists.
const FIXTURE_A: &str = r#"table 50100 "S5 Order"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

interface "S5 ILog"
{
    procedure Touch();
}

codeunit 50100 "S5 Target"
{
    procedure O(X: Interface "S5 ILog"): Integer
    begin
        exit(0);
    end;

    procedure O(X: Text): Integer
    begin
        exit(1);
    end;
}

codeunit 50101 "S5 Work"
{
    procedure Helper()
    var
        L: Codeunit "S5 Target";
        T: Codeunit "S5 Target";
        R: Record "S5 Order";
    begin
        T.O(L);
        R.Modify();
    end;

    procedure Root()
    begin
        Helper();
        Commit();
    end;

    procedure Entry()
    begin
        Root();
    end;
}
"#;

fn build(src: &str) -> Model {
    assemble_and_resolve_inline_program_default(&[("src/S5.al".to_string(), src.to_string())], APP)
}

/// The id of the one routine named `name` among `routines`.
fn id_in(routines: &[ModelRoutine], name: &str) -> String {
    let ids: BTreeSet<&str> = routines
        .iter()
        .filter(|r| r.name == name)
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "precondition: one routine {name}, got {ids:?}"
    );
    ids.into_iter().next().unwrap().to_string()
}

/// `(from, kind)` of every four-kind edge in `edges`.
fn four_kind_edges<'e>(
    edges: impl IntoIterator<Item = &'e crate::engine::l4::combined_graph::UncertaintyEdge>,
) -> Vec<(&'e str, &'e str)> {
    edges
        .into_iter()
        .filter(|u| FOUR_KINDS.contains(&u.uncertainty.kind.as_str()))
        .map(|u| (u.from.as_str(), u.uncertainty.kind.as_str()))
        .collect()
}

fn coverage_of<'a>(
    summaries: &'a HashMap<String, FullRoutineSummary>,
    id: &str,
) -> &'a CoverageRecord {
    summaries
        .get(id)
        .and_then(|s| s.coverage.as_ref())
        .unwrap_or_else(|| panic!("a coverage record for {id}"))
}

// ---------------------------------------------------------------------------
// T1: spans and d9
// ---------------------------------------------------------------------------

/// T1 (spec §5): the context's fold reaches the transaction spans and d9. On
/// fixture A the explicit-Commit span of `Root` is `{Entry, Root}` with one
/// pending physical table (`Helper`'s `Modify`); `Root` reaches `Helper`'s
/// ambiguous call, so the span is incomplete and d9 reports it, admitted by
/// the coverage gate alone (1 table < 2), with the count wording ("writes 1
/// known table(s) before it", `d9.rs:75-80`). Break: delete the fold call in
/// `det_cone_inputs` -> `Helper` is `complete`, `coverage_complete` is `true`,
/// and d9 emits nothing.
#[test]
fn step5_t1_spans_and_d9_read_the_fold() {
    let m = build(FIXTURE_A);
    let [helper, root, entry] =
        ["Helper", "Root", "Entry"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();

    // Preconditions, hand-stated.
    assert_eq!(
        four_kind_edges(&ctx.graph.uncertainty_edges),
        vec![(helper.as_str(), "ambiguous-overload")],
        "precondition: the Primary graph holds exactly one four-kind edge, \
         ambiguous-overload from Helper"
    );
    let spans: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| s.seed_kind == SeedKind::ExplicitCommit && s.commit_routine_id == root)
        .collect();
    assert_eq!(
        spans.len(),
        1,
        "precondition: one explicit-Commit span at Root"
    );
    let span = spans[0];
    let mut want_span = vec![entry.clone(), root.clone()];
    want_span.sort();
    assert_eq!(
        span.routines_in_span, want_span,
        "precondition: the span is {{Entry, Root}}"
    );
    assert_eq!(
        span.pending_physical_tables.len(),
        1,
        "precondition: one pending physical table: {:?}",
        span.pending_physical_tables
    );

    // The fold and its readers.
    let h = coverage_of(&ctx.summaries, &helper);
    assert_eq!(
        (h.direct_status.as_str(), h.reasons.clone()),
        ("partial", vec!["ambiguous-overload".to_string()]),
        "Helper's direct coverage folds its ambiguous call: {h:?}"
    );
    let r = coverage_of(&ctx.summaries, &root);
    assert_eq!(
        r.inherited_status, "partial",
        "Root inherits partial: {r:?}"
    );
    assert!(
        !span.coverage_complete,
        "the span reaches Helper's ambiguous call, so it is incomplete"
    );
    let out = crate::engine::l5::detectors::d9::detect_d9(&m, &ctx).expect("d9 runs");
    let got: Vec<(&str, &str)> = out
        .findings
        .iter()
        .map(|f| (f.id.as_str(), f.root_cause.as_str()))
        .collect();
    assert_eq!(
        got.len(),
        1,
        "d9 emits exactly one finding, for Root's span: {got:?}"
    );
    assert_eq!(got[0].0, format!("d9/{}", span.commit_operation_id));
    assert!(
        got[0].1.contains("writes 1 known table(s) before it"),
        "the count wording: {}",
        got[0].1
    );
}

// ---------------------------------------------------------------------------
// T3: context == snapshot per routine on r0
// ---------------------------------------------------------------------------

/// Sorted dirs under `root`, at any depth, holding an `app.json`.
fn app_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// `(direct_status, inherited_status, sorted reasons, sorted unknown_targets)`.
type CovKey = (String, String, Vec<String>, Vec<String>);

fn cov_key(c: &CoverageRecord) -> CovKey {
    let mut reasons = c.reasons.clone();
    reasons.sort();
    let mut targets = c.unknown_targets.clone();
    targets.sort();
    (
        c.direct_status.clone(),
        c.inherited_status.clone(),
        reasons,
        targets,
    )
}

/// T3 (spec §5, A1, #161 part 2): on every `tests/r0-corpus/` app dir,
/// single-app, every routine's coverage from the detector context
/// (`build_detector_context`: `det_cone_inputs` + the cone over `rel.cone()`)
/// equals the snapshot base's (`build_r3a3_source_only_base`, which already
/// folds). Precondition: at least 11 dirs have a routine with a four-kind
/// reason on the snapshot side (P5), so the equality is not vacuous. Break:
/// delete the fold in `det_cone_inputs` -> fails on those dirs (as on the base:
/// this test is the bug's witness).
#[test]
fn step5_t3_context_coverage_equals_the_snapshot_on_r0() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let dirs = app_dirs(&root);
    assert!(!dirs.is_empty(), "precondition: r0 dirs under {root:?}");
    let mut failures: Vec<String> = Vec::new();
    let mut dirs_with_kind = 0usize;
    for dir in &dirs {
        let where_ = dir.strip_prefix(&root).unwrap_or(dir).display().to_string();
        let inputs = crate::program::inputs::InputSet::read(dir, &Default::default());
        let Some(m) = assemble_and_resolve_workspace_program(
            &inputs,
            crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
            false,
        ) else {
            failures.push(format!("{where_}: no single-app model built"));
            continue;
        };
        let snapshot = build_r3a3_source_only_base(&m);
        let owned = build_detector_context(&m, substrate::SUMMARIES);
        let ctx = owned.ctx();
        let ids: BTreeSet<&str> = m.workspace.routines.iter().map(|r| r.id.as_str()).collect();
        let mut has_kind = false;
        for id in ids {
            let s = snapshot.coverage.get(id).map(cov_key);
            let c = ctx
                .summaries
                .get(id)
                .and_then(|s| s.coverage.as_ref())
                .map(cov_key);
            if s.as_ref()
                .is_some_and(|k| k.2.iter().any(|r| FOUR_KINDS.contains(&r.as_str())))
            {
                has_kind = true;
            }
            if s != c {
                failures.push(format!("{where_} {id}: snapshot {s:?}, context {c:?}"));
            }
        }
        if has_kind {
            dirs_with_kind += 1;
        }
    }
    assert!(
        dirs_with_kind >= 11,
        "precondition: at least 11 r0 dirs carry a four-kind reason (P5), got {dirs_with_kind}"
    );
    let shown: Vec<&String> = failures.iter().take(20).collect();
    assert!(
        failures.is_empty(),
        "{} routine(s) differ over {} dirs, first {}:\n{}",
        failures.len(),
        dirs.len(),
        shown.len(),
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---------------------------------------------------------------------------
// T4, T5: the dependency fixture
// ---------------------------------------------------------------------------

fn cross_app_base(ws: &Path) -> R3a5CrossAppBase {
    let inputs = crate::program::inputs::InputSet::read(ws, &Default::default());
    build_cross_app_base(&inputs, "r0").expect("cross-app base")
}

/// T4/T5's preconditions on the base: the dependency half holds exactly one
/// four-kind edge, `ambiguous-overload` from `Inner`; the Primary half holds
/// none; no routine makes an object run (so no unresolved object-run source).
/// Returns `(Go, Inner)`.
fn dep_ambiguous_preconditions(base: &R3a5CrossAppBase) -> (String, String) {
    let [go, inner] = ["Go", "Inner"].map(|n| id_in(base.ws_routines(), n));
    assert!(
        base.dep_routine_ids.contains(&inner) && !base.dep_routine_ids.contains(&go),
        "precondition: Inner is dependency code, Go is the workspace's"
    );
    assert_eq!(
        four_kind_edges(base.rel.dependency_uncertainty()),
        vec![(inner.as_str(), "ambiguous-overload")],
        "precondition: the dependency half holds exactly one four-kind edge, from Inner"
    );
    assert_eq!(
        four_kind_edges(&base.rel.primary().uncertainty_edges),
        Vec::<(&str, &str)>::new(),
        "precondition: the Primary half holds no four-kind edge"
    );
    let runs: Vec<&str> = base
        .ws_routines()
        .iter()
        .flat_map(|r| &r.call_sites)
        .filter(|c| matches!(c.callee, PCallee::ObjectRun { .. }))
        .map(|c| c.id.as_str())
        .collect();
    assert!(runs.is_empty(), "precondition: no object run: {runs:?}");
    (go, inner)
}

/// T4 (spec §5, A2): cross-app, the context folds the dependency half and the
/// Cone carries it. `Inner` (dependency code) is `partial` by its own
/// ambiguous call, and the workspace's `Go`, which reaches it through `Outer`,
/// inherits `partial` with `Inner` in its `unknown_targets`. Go's status also
/// equals a BFS over `rel.cone()` (partial iff a reached row's direct status is
/// not `complete`). That BFS reads the same folded direct statuses, so it
/// checks the Cone's propagation, not the fold: the explicit `Inner` and `Go`
/// assertions carry the fold. Break: fold `rel.primary().uncertainty_edges`
/// only -> `Inner` and `Go` are `complete`.
#[test]
fn step5_t4_dependency_half_folds_into_the_cross_app_context() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    write_fixture_dep_ambiguous(&ws);
    let base = cross_app_base(&ws);
    let (go, inner) = dep_ambiguous_preconditions(&base);
    let rel = Arc::clone(&base.rel);

    let d35 = registered_detectors()
        .into_iter()
        .find(|d| d.name == crate::engine::l5::detectors::d35::DETECTOR)
        .expect("d35 is registered");
    let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
    view.prepare(&analyze_plan(vec![d35], true));
    let summaries = &view.det().summaries;

    let i = coverage_of(summaries, &inner);
    assert_eq!(
        (i.direct_status.as_str(), i.reasons.clone()),
        ("partial", vec!["ambiguous-overload".to_string()]),
        "Inner's direct coverage folds its dependency-half edge: {i:?}"
    );
    let g = coverage_of(summaries, &go);
    assert_eq!(g.inherited_status, "partial", "Go inherits partial: {g:?}");
    assert!(
        g.unknown_targets.contains(&inner),
        "Inner is in Go's unknown_targets: {g:?}"
    );

    // A propagation check: a BFS over the Cone's typed edges from Go, over the
    // same folded direct statuses (not independent of the fold).
    let mut succ: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in rel.cone() {
        if let Some(to) = &e.to {
            succ.entry(e.from.as_str()).or_default().push(to.as_str());
        }
    }
    let mut seen: BTreeSet<&str> = BTreeSet::from([go.as_str()]);
    let mut queue: VecDeque<&str> = VecDeque::from([go.as_str()]);
    while let Some(n) = queue.pop_front() {
        for &t in succ.get(n).map(Vec::as_slice).unwrap_or_default() {
            if seen.insert(t) {
                queue.push_back(t);
            }
        }
    }
    assert!(
        seen.contains(inner.as_str()),
        "precondition: Go reaches Inner over the Cone: {seen:?}"
    );
    let oracle_partial = seen.iter().any(|id| {
        summaries
            .get(*id)
            .and_then(|s| s.coverage.as_ref())
            .is_some_and(|c| c.direct_status != "complete")
    });
    assert_eq!(
        g.inherited_status,
        if oracle_partial {
            "partial"
        } else {
            "complete"
        },
        "Go's inherited status equals the BFS oracle over rel.cone() (reached {seen:?})"
    );
}

/// T5 (spec §5, A2): one fold, two cross-app producers. On T4's workspace,
/// P-a's direct coverage (`det_cone_inputs`) equals P-b's
/// (`build_cross_app_base`'s `direct_coverage`), and both hold `Inner`'s
/// folded row, so the equality is not two unfolded maps agreeing. Break: drop
/// the fold call in `build_cross_app_base` -> `Inner` differs.
#[test]
fn step5_t5_both_cross_app_producers_fold_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    write_fixture_dep_ambiguous(&ws);
    let base = cross_app_base(&ws);
    let (_go, inner) = dep_ambiguous_preconditions(&base);
    let pa = det_cone_inputs(&base.resolved, &base.rel).coverage_in;
    let pb = &base.direct_coverage;
    let want = (
        "partial".to_string(),
        vec!["ambiguous-overload".to_string()],
    );
    let sorted =
        |m: &HashMap<String, (String, Vec<String>)>| -> BTreeMap<String, (String, Vec<String>)> {
            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
    assert_eq!(
        (pa.get(&inner), pb.get(&inner)),
        (Some(&want), Some(&want)),
        "(P-a, P-b) Inner's row is folded in both producers"
    );
    assert_eq!(sorted(&pa), sorted(pb), "P-a's map equals P-b's");
}

/// #160 step 6 T8 (I5, the materialized half) on T4's workspace: a view
/// prepared with policy's request (`WitnessRequest::Primary`) materializes raw
/// for no dependency routine, and `Go`'s raw (which reaches `Inner`'s write)
/// equals a `WitnessScope::All` composition's. Break 1: resolve `Primary` as
/// `All` -> the dependency members get raw -> the count is > 0 -> fails.
#[test]
fn step6_t8_a_primary_witness_request_materializes_no_dependency_member() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    write_fixture_dep_ambiguous(&ws);
    let base = cross_app_base(&ws);
    let (go, inner) = dep_ambiguous_preconditions(&base);
    let d35 = registered_detectors()
        .into_iter()
        .find(|d| d.name == crate::engine::l5::detectors::d35::DETECTOR)
        .expect("d35 is registered");
    let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
    view.prepare(
        &analyze_plan(vec![d35], true)
            .with_witness(crate::engine::session::WitnessRequest::Primary),
    );
    let raw = view.raw();
    assert_eq!(
        witness_non_primary_members(view.model(), raw),
        0,
        "no dependency member materialized: {:?}",
        raw.keys().collect::<BTreeSet<_>>()
    );
    let (_, all) = compose_capabilities(
        view.model(),
        view.rel().cone(),
        det_cone_inputs(view.model(), view.rel()),
        ConeScope {
            derived: true,
            raw: WitnessScope::All,
        },
    );
    assert!(
        all.contains_key(&inner),
        "precondition: the All composition gives Inner raw"
    );
    let go_raw = raw.get(&go).expect("Go is a subject");
    assert!(!go_raw.is_empty(), "Go inherits Inner's write");
    assert_eq!(Some(go_raw), all.get(&go), "Go's raw equals the All run's");
}

// ---------------------------------------------------------------------------
// T9: d2's use of the fold, and its budget cost
// ---------------------------------------------------------------------------

/// T9's workspace: a loop in `L` raises `OnE`; its subscriber `S` calls `Wide`,
/// then `Db`. `Wide` calls `A1..A<n>`, each making only the ambiguous `O(L)`
/// call (no record operation under `Wide`); `Db` does one `Modify` on a
/// physical table.
fn t9_src(n: usize) -> String {
    let wide_calls: String = (1..=n).map(|i| format!("        A{i}();\n")).collect();
    let callees: String = (1..=n)
        .map(|i| {
            format!(
                "\n    procedure A{i}()\n    var\n        L: Codeunit \"T9 Target\";\n        T: Codeunit \"T9 Target\";\n    begin\n        T.O(L);\n    end;\n"
            )
        })
        .collect();
    format!(
        r#"table 50120 "T9 Order"
{{
    fields
    {{
        field(1; "No."; Code[20]) {{ }}
    }}
    keys
    {{
        key(PK; "No.") {{ Clustered = true; }}
    }}
}}

interface "T9 ILog"
{{
    procedure Touch();
}}

codeunit 50120 "T9 Target"
{{
    procedure O(X: Interface "T9 ILog"): Integer
    begin
        exit(0);
    end;

    procedure O(X: Text): Integer
    begin
        exit(1);
    end;
}}

codeunit 50121 "T9 Loop"
{{
    [IntegrationEvent(false, false)]
    procedure OnE()
    begin
    end;

    procedure L()
    var
        I: Integer;
    begin
        for I := 1 to 10 do
            OnE();
    end;
}}

codeunit 50122 "T9 Sub"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"T9 Loop", 'OnE', '', false, false)]
    local procedure S()
    begin
        Wide();
        Db();
    end;

    procedure Wide()
    begin
{wide_calls}    end;

    procedure Db()
    var
        R: Record "T9 Order";
    begin
        R.Modify();
    end;
{callees}}}
"#
    )
}

/// What [`t9_run`] returns: the d2 findings' ids, `Wide`'s inherited status,
/// and each `Ai`'s direct `(status, reasons)`.
type T9Out = (Vec<String>, String, Vec<(String, Vec<String>)>);

/// Builds T9's workspace with `n` callees under `Wide`, asserts the
/// preconditions, and returns [`T9Out`].
fn t9_run(n: usize) -> T9Out {
    let m = build(&t9_src(n));
    let routines = &m.workspace.routines;
    let [s, wide, db] = ["S", "Wide", "Db"].map(|name| id_in(routines, name));
    let callees: Vec<String> = (1..=n).map(|i| id_in(routines, &format!("A{i}"))).collect();
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();

    // Preconditions, hand-stated: no physical-table fact under Wide, S's own
    // edges in the order Wide, Db, and Wide calls every Ai.
    for id in callees.iter().chain([&wide]) {
        assert!(
            !ctx.cone_derived.touches_physical_table(id),
            "precondition: {id} has no physical-table fact"
        );
    }
    assert!(
        ctx.cone_derived.touches_physical_table(&db),
        "precondition: Db touches a physical table"
    );
    let s_out: Vec<&str> = ctx.graph.edges_by_from[&s]
        .iter()
        .filter(|e| e.kind != "event-dispatch")
        .map(|e| e.to.as_str())
        .collect();
    assert_eq!(
        s_out,
        vec![wide.as_str(), db.as_str()],
        "precondition: S's edges are Wide, then Db"
    );
    let wide_out: BTreeSet<&str> = ctx.graph.edges_by_from[&wide]
        .iter()
        .map(|e| e.to.as_str())
        .collect();
    assert_eq!(
        wide_out,
        callees.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        "precondition: Wide calls A1..A{n}"
    );
    // Each Ai makes exactly one uncertain call, the ambiguous `O(L)`.
    for id in &callees {
        let kinds: Vec<&str> = ctx
            .graph
            .uncertainty_edges
            .iter()
            .filter(|e| &e.from == id)
            .map(|e| e.uncertainty.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec!["ambiguous-overload"],
            "precondition: {id}'s one uncertain call is the ambiguous O(L)"
        );
    }
    let out = crate::engine::l5::detectors::d2::detect_d2(&m, &ctx).expect("d2 runs");
    let ids = out.findings.iter().map(|f| f.id.clone()).collect();
    let wide_status = coverage_of(&ctx.summaries, &wide).inherited_status.clone();
    let callee_direct = callees
        .iter()
        .map(|id| {
            let c = coverage_of(&ctx.summaries, id);
            (c.direct_status.clone(), c.reasons.clone())
        })
        .collect();
    (ids, wide_status, callee_direct)
}

/// T9 main variant (spec §5, §3.2): pins d2's use of the fold AND a known
/// precision cost. `Wide` is `partial` (its callees' ambiguous calls), so
/// d2's `expand` admits it as `Unknown`; the walk enters it, spends the shared
/// 500-node budget on the 500 terminal-free callees, and cuts the `Db` branch
/// (`NodeBudgetCut`): d2 emits NO finding for `L`'s loop. Break: delete the
/// fold call in `det_cone_inputs` -> `Wide` is `No`, pruned, and the finding
/// is emitted. A head loss on CDO is fallback F6.
#[test]
fn step5_t9_d2_budget_cost_main() {
    let (ids, wide_status, callee_direct) = t9_run(500);
    assert_eq!(
        ids,
        Vec::<String>::new(),
        "the walk spends its budget under the partial Wide and finds no Complete path \
         (Wide's inherited status: {wide_status})"
    );
    assert_eq!(wide_status, "partial", "Wide is partial by the fold");
    let folded = (
        "partial".to_string(),
        vec!["ambiguous-overload".to_string()],
    );
    for (i, got) in callee_direct.iter().enumerate() {
        assert_eq!(got, &folded, "A{}'s direct coverage folds its call", i + 1);
    }
}

/// T9 control (spec §5): `Wide` calls one `A1`; the walk enters it and still
/// reaches `Db`, so d2 emits its finding. Green on the base and on the head.
#[test]
fn step5_t9_d2_budget_cost_control() {
    let (ids, _, _) = t9_run(1);
    assert_eq!(ids.len(), 1, "one d2 finding for L's loop: {ids:?}");
}
