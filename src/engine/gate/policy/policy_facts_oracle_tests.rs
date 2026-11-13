//! #160 step 6 Task 3 (spec §5 T3's policy arm, Decision 41, A6.12; T8's
//! materialized half, A6.6): policy reads the view's witness pass.
//!
//! The oracle `tests/fixtures/policy-facts-oracle/selected-facts.json` was
//! recorded on Task 2b-1's head (`86d24beb`), where policy composed its own
//! `{ derived, raw: All }` cone and read `FullRoutineSummary::inherited_raw()`.
//! Per case (every `tests/r0-corpus/` app dir and every `test_support`
//! dependency fixture, cross-app and `--single-app`) it holds the facts each
//! `facts:` mode selects per primary routine, as counts and the sha256 of the
//! rendering below. The parent path no longer exists, so the oracle cannot be
//! re-minted: a mismatch is a change in what policy rules see.
//!
//! Amended by hand (2026-10-28, G10): `r0/ws-r0-canon-stress` (cross and single)
//! gained one direct fact, the `publish` of its `[InternalEvent]` routine, which
//! is now an event publisher like the integration and business ones (`direct`
//! 4 → 5, `any` 5 → 6, new sha256). No other case moved.
//!
//! Re-recorded (2026-10-28, G20) with `REMINT_POLICY_FACTS_ORACLE=1`: 118 cases
//! gained the `error-implicit` facts (unguarded record lookups, `TestField`,
//! `FieldError`) of their routines, which policy sees as it sees `error-throw`.
//! Re-record only for such an intended change, after reading the diff.
//!
//! Re-recorded (2026-11-01, routine-id migration) with
//! `REMINT_POLICY_FACTS_ORACLE=1`: the rendering hashed here holds internal
//! routine ids, whose prefix became the app scope. Checked before re-recording:
//! on the parent (004f0ee4) and after the change, all 454 cases have equal
//! counts and an equal sha256 once every `<prefix>/<64 hex>` is reduced to its
//! hash part; 448 raw sha256 values moved, the other 6 hold no internal id.
//!
//! Re-recorded (2026-11-06, G20 widened) with `REMINT_POLICY_FACTS_ORACLE=1`:
//! a statement `Insert` / `Modify` / `Delete` / `Rename` / `Find` now also
//! gives an `error-implicit` fact. 175 of 455 cases moved; checked against the
//! old file, no case's `any` / `direct` / `inherited` / `routines` count went
//! down (facts only added).
//! Re-recorded (2026-11-06, F1: record operations raise their table's platform
//! events) with `REMINT_POLICY_FACTS_ORACLE=1`: only `r0/ws-d29` (cross and
//! single) moved, `inherited` 0 → 7 (`any` 9 → 16, `direct` unchanged). Its
//! subscribers call `Rec.Modify()`/`Rec.Delete()` on the Customer they are
//! handed, which raises the subscribed events again, and `OnAfterPostMutates`'s
//! `Rec.Modify()` raises them too; those subscribers' facts are now inherited.
//! Both re-recordings were made on their own branch; the file was recorded
//! again on the integration of the two (`integrate/outval-r1-fixes`).
//!
//! Re-recorded (2026-11-09, engine limits B5: a FlowField's `CalcFormula`
//! table is read where the FlowField is calculated) with
//! `REMINT_POLICY_FACTS_ORACLE=1`: only `r0/ws-d21` and `r0/ws-d22` (cross and
//! single) moved, `direct` and `any` +1 each: the `CalcFields` of
//! `Customer."Balance (LCY)"` now also reads `Ledger Entry`.
//!
//! Re-recorded (2026-11-11, ENG-09: a checked `Codeunit.Run` commits) with
//! `REMINT_POLICY_FACTS_ORACLE=1`: 9 of 454 cases moved, each by its checked
//! runs' commit facts (`direct` +1 per run in the case's own code, `inherited`
//! plus their copies in callers), no count down: `r0/ws-d50-medium` (3 runs),
//! `r0/ws-d50-temp-gate` (4), `r0/ws-d50-pos` (1),
//! `r0/ws-txn-d49-neg-run-boundary` (1), cross and single each, and
//! `dep/dep_ledger` cross (its dependency's `Ok := Codeunit.Run(..)`, inherited).
//!
//! Re-recorded (2026-11-12, ENG-22: a checked `Codeunit.Run` raises its own
//! error when a write is pending at it) with `REMINT_POLICY_FACTS_ORACLE=1`:
//! the same 9 cases moved by the same amounts, each checked run's
//! `error-implicit` fact (resource `checked-run`) beside its commit: `direct`
//! +1 per run, `inherited` plus the copies; no count down.
//!
//! Re-recorded (2026-11-12, ENG-10: a database write inside a used try call is
//! refused, an error no try call catches) with `REMINT_POLICY_FACTS_ORACLE=1`:
//! only `r0/ws-d53` (single and cross) moved, `direct` 4 -> 6 and `any`
//! 12 -> 14: the `error-implicit` fact (resource `write-in-try`) of its two
//! `if not TryStep()` calls; its statement calls (`TryStep();`,
//! `asserterror TryStep();`) have none. No count down.
//!
//! Re-recorded (2026-11-12, ENG-25: a parens-less record operation in
//! expression position is a record operation) with
//! `REMINT_POLICY_FACTS_ORACLE=1`: only `r0/ws-d3-method-stmt` (single and
//! cross) moved, `direct` and `any` 5 -> 6: the read of `if Customer.FindSet
//! then`. No count down.
//!
//! Re-recorded (2026-11-13, TLS-41: the new fixture `ws-grammar-v4-shapes`) with
//! `REMINT_POLICY_FACTS_ORACLE=1`: two cases added (`r0/ws-grammar-v4-shapes`
//! single and cross, 5 routines, no facts); no other case moved.

use super::*;
use crate::engine::gate::policy::policy_loader::{LoadResult, load_policy_from_string};
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l5::detector_context::witness_non_primary_members;
use crate::engine::l5::detectors::default_detectors;
use crate::engine::l5::test_support as ts;
use crate::engine::session::{View, policy_plan};
use crate::program::inputs::WorkspaceInputs;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One rule per `facts:` mode; `select_facts` reads only the mode.
const MODES_POLICY: &str = "version: 1
rules:
  - id: m-direct
    severity: high
    when:
      capability.op: insert
    facts: direct
  - id: m-inherited
    severity: high
    when:
      capability.op: insert
    facts: inherited
  - id: m-any
    severity: high
    when:
      capability.op: insert
    facts: any
";

/// Routine id -> mode -> its selected facts (`Debug`), in the engine's order.
type Selected = BTreeMap<String, BTreeMap<String, Vec<String>>>;

fn mode_rules() -> Vec<Rule> {
    match load_policy_from_string(MODES_POLICY) {
        LoadResult::Ok { policy, .. } => policy.rules,
        LoadResult::Err { errors, .. } => panic!("{errors:?}"),
    }
}

/// Each selected fact as the owned `CapabilityFact`'s `Debug` (an inherited
/// one materialized here, in the test, never by policy), in policy's order.
fn render(mut facts: Vec<FactRef<'_>>) -> Vec<String> {
    sort_facts(&mut facts);
    facts
        .iter()
        .map(|f| match f {
            FactRef::Direct(f) => format!("{f:?}"),
            FactRef::Inherited(v) => format!("{:?}", v.materialize()),
        })
        .collect()
}

/// A view prepared as `run_policy_check` prepares it, its run taken.
fn view_of(ws: &Path, single_app: bool) -> Result<View, String> {
    let inputs = WorkspaceInputs::read(ws, &Default::default());
    let target = AnalysisTarget::build(&inputs, single_app, "oracle")?;
    let mut view = View::new(target);
    let plan = policy_plan(default_detectors(), true, false);
    let key = plan.keys().remove(0);
    view.prepare(&plan);
    drop(view.take_run(&key));
    Ok(view)
}

/// What each mode selects per primary routine, off `det.summaries` and
/// `view.raw()` as `run_policy_engine` reads them.
fn selected(ws: &Path, single_app: bool) -> Result<Selected, String> {
    let view = view_of(ws, single_app)?;
    let (summaries, raw) = (&view.det().summaries, view.raw());
    let rules = mode_rules();
    let mut out = Selected::new();
    for r in &view.model().workspace.routines {
        if !view.target().is_primary_routine(r) {
            continue;
        }
        let direct = summaries
            .get(r.id.as_str())
            .map_or(&[][..], |s| &s.capability_facts_direct);
        let inherited = raw.get(r.id.as_str());
        let per = rules
            .iter()
            .map(|rule| {
                (
                    rule.facts.clone().expect("mode"),
                    render(select_facts(rule, direct, inherited)),
                )
            })
            .collect();
        out.insert(r.id.clone(), per);
    }
    Ok(out)
}

fn digest(s: &Selected) -> serde_json::Value {
    let total = |mode: &str| s.values().map(|m| m[mode].len()).sum::<usize>();
    serde_json::json!({
        "routines": s.len(),
        "direct": total("direct"),
        "inherited": total("inherited"),
        "any": total("any"),
        "sha256": crate::engine::ids::sha256_hex(&serde_json::to_string(s).unwrap()),
    })
}

fn app_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// A `test_support` fixture writer.
type WriteFixture = fn(&Path);

const DEP_FIXTURES: [(&str, WriteFixture); 6] = [
    ("step4", ts::write_fixture_step4),
    ("dep_uncertainty", ts::write_fixture_dep_uncertainty),
    ("dep_ledger", ts::write_fixture_dep_ledger),
    ("symbol_only_dep", ts::write_fixture_symbol_only_dep),
    ("rule_b", ts::write_fixture_rule_b),
    ("dep_ambiguous", ts::write_fixture_dep_ambiguous),
];

fn cases() -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    let mut add = |name: String, ws: &Path| {
        for (universe, single) in [("cross", false), ("single", true)] {
            let v = match selected(ws, single) {
                Ok(s) => digest(&s),
                Err(e) => serde_json::Value::String(format!("error: {e}")),
            };
            out.insert(format!("{name} {universe}"), v);
        }
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    for dir in app_dirs(&root) {
        let name = dir.strip_prefix(&root).unwrap().display().to_string();
        add(format!("r0/{}", name.replace('\\', "/")), &dir);
    }
    for (name, write) in DEP_FIXTURES {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write(&ws);
        add(format!("dep/{name}"), &ws);
    }
    out
}

/// T3's policy arm: every case selects exactly the oracle's facts. The
/// precondition (the oracle has inherited facts, cross-app dependency ones
/// among them) keeps it from passing on empty selections. Break: drop `raw`
/// from `select_facts`'s `inherited` arm -> every case with inherited facts
/// differs -> fails.
#[test]
fn policy_selects_the_parents_facts_per_primary_routine() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/policy-facts-oracle/selected-facts.json");
    let oracle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let want = oracle["cases"].as_object().expect("oracle cases");
    let inherited = |key: &str| want[key]["inherited"].as_u64().unwrap_or(0);
    assert!(
        want.keys().map(|k| inherited(k)).sum::<u64>() > 0
            && inherited("dep/rule_b cross") > 0
            && inherited("dep/rule_b single") == 0,
        "precondition: the oracle selects inherited facts, dependency ones cross-app only"
    );
    let got = cases();
    // The recorder is gone (module doc), so an INTENDED change of what policy
    // sees re-records the cases from this run, explicitly, and the module doc
    // says why. The precondition above was checked on the oracle as it was.
    if std::env::var_os("REMINT_POLICY_FACTS_ORACLE").is_some() {
        let mut doc = oracle.clone();
        doc["cases"] = serde_json::Value::Object(got.clone());
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
        return;
    }
    let mut bad: Vec<String> = want
        .iter()
        .filter(|(k, v)| got.get(*k) != Some(v))
        .map(|(k, v)| format!("{k}: want {v}, got {:?}", got.get(k)))
        .collect();
    bad.extend(
        got.keys()
            .filter(|k| !want.contains_key(*k))
            .map(|k| format!("{k}: not in the oracle")),
    );
    assert!(
        bad.is_empty(),
        "{} case(s) differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// T8, the materialized half (I5, A6.6): a cross-app policy view keeps raw for
/// no dependency routine, and for every primary one. Preconditions: the
/// model has dependency routines and the subject `Go` inherits the dependency
/// `Inner`'s write. Break 1: give policy's request the scope `All` -> the
/// dependency members get raw -> the count is > 0 -> fails.
#[test]
fn policy_view_materializes_no_dependency_member() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    ts::write_fixture_dep_ambiguous(&ws);
    let view = view_of(&ws, false).expect("cross-app target");
    let model = view.model();
    let primary: BTreeSet<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| view.target().is_primary_routine(r))
        .map(|r| r.id.as_str())
        .collect();
    let id_of = |name: &str| {
        let ids: Vec<&str> = model
            .workspace
            .routines
            .iter()
            .filter(|r| r.name == name)
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids.len(), 1, "precondition: one {name}");
        ids[0]
    };
    let (go, inner) = (id_of("Go"), id_of("Inner"));
    assert!(
        primary.contains(go) && !primary.contains(inner),
        "precondition: Go is primary, Inner is dependency code"
    );
    let raw = view.raw();
    assert!(
        raw.get(go).is_some_and(|v| !v.is_empty()),
        "precondition: Go inherits Inner's write"
    );
    assert_eq!(
        witness_non_primary_members(model, raw),
        0,
        "no dependency member materialized: {:?}",
        raw.keys().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        raw.keys().collect::<BTreeSet<_>>(),
        primary,
        "every primary routine, and only those, has raw"
    );
}

/// The rule engine on `ws`'s policy view, with `drop` removed from the raw
/// map (the hand-stated precondition: a witness scope that missed an
/// evaluated routine), for `policy`.
fn run_without_raw_of(drop: &str, policy: &str) -> RunOutput {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    ts::write_fixture_dep_ambiguous(&ws);
    let view = view_of(&ws, false).expect("cross-app target");
    let model = view.model();
    let routines: Vec<ModelRoutine> = model
        .workspace
        .routines
        .iter()
        .filter(|r| view.target().is_primary_routine(r))
        .cloned()
        .collect();
    let id = routines
        .iter()
        .find(|r| r.name == drop)
        .expect("precondition: a primary routine")
        .id
        .clone();
    let det = view.det();
    assert!(
        det.summaries.contains_key(&id) && view.raw().get(&id).is_some_and(|v| !v.is_empty()),
        "precondition: {drop} has a summary and inherited facts"
    );
    let mut raw = view.raw().clone();
    let _removed = raw.take_subjects([id.as_str()]);
    let declaring =
        crate::program::model::declaring::DeclaringObjects::new(&model.workspace.objects);
    let classes = crate::engine::root_classification::classify_roots(&model.workspace, &declaring);
    let pm = PolicyModel::new(
        &routines,
        &model.workspace,
        declaring,
        &det.event_graph.events,
        &classes,
        &det.summaries,
        &raw,
    );
    let doc = match load_policy_from_string(policy) {
        LoadResult::Ok { policy, .. } => policy,
        LoadResult::Err { errors, .. } => panic!("{errors:?}"),
    };
    run_policy_engine(&pm, &doc)
}

/// Final panel F1-O6 / F1-C1 (R6): an evaluated routine with a summary but no
/// raw entry FAILS LOUDLY when a rule reads inherited facts. Read as "no
/// inherited facts", a `facts: inherited|any` rule would pass for it unseen.
#[test]
#[should_panic(expected = "has a summary but no raw inherited facts")]
fn a_routine_missing_from_raw_fails_loudly() {
    run_without_raw_of("Go", MODES_POLICY);
}

/// The same precondition under a `facts: direct` rule: it never reads the
/// inherited half, so it runs.
#[test]
fn a_direct_rule_never_reads_raw() {
    let out = run_without_raw_of(
        "Go",
        "version: 1
rules:
  - id: m-direct
    severity: high
    when:
      capability.op: insert
    facts: direct
",
    );
    assert_eq!(out.rule_summaries.len(), 1);
}

/// The rule engine over `view`'s primary routines and its raw cell, as
/// `pipeline.rs` builds it.
fn run_on_view(view: &View, policy: &str) -> RunOutput {
    let model = view.model();
    let routines: Vec<ModelRoutine> = model
        .workspace
        .routines
        .iter()
        .filter(|r| view.target().is_primary_routine(r))
        .cloned()
        .collect();
    let det = view.det();
    let declaring =
        crate::program::model::declaring::DeclaringObjects::new(&model.workspace.objects);
    let classes = crate::engine::root_classification::classify_roots(&model.workspace, &declaring);
    let pm = PolicyModel::new(
        &routines,
        &model.workspace,
        declaring,
        &det.event_graph.events,
        &classes,
        &det.summaries,
        view.raw(),
    );
    let doc = match load_policy_from_string(policy) {
        LoadResult::Ok { policy, .. } => policy,
        LoadResult::Err { errors, .. } => panic!("{errors:?}"),
    };
    run_policy_engine(&pm, &doc)
}

fn routine_named<'v>(view: &'v View, name: &str) -> &'v ModelRoutine {
    let rs: Vec<&ModelRoutine> = view
        .model()
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rs.len(), 1, "precondition: one routine {name}");
    rs[0]
}

/// The evidence steps after the first (one per matched fact) of the finding
/// of `rule` on `routine`.
fn matched_steps<'o>(
    out: &'o RunOutput,
    rule: &str,
    routine: &ModelRoutine,
) -> &'o [crate::engine::l5::finding::EvidenceStep] {
    let id = format!("policy-{rule}/{}", routine.id);
    let f = out
        .findings
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("a finding {id}"));
    &f.evidence_path[1..]
}

/// Cone-memory step 1, T4 (policy half): policy reads each inherited fact
/// through the compact view, and a finding's evidence carries the witnesses
/// `retag` gave it. (a) call hop: `Caller`'s inherited `execute` cites
/// CALLER's call site, not the one `Runner`'s own direct fact carries. (b)
/// trigger hop: `Inserter`'s inherited `modify` cites no call site and the
/// trigger's own operation. (c) event hop: `OnT4`'s inherited `modify` cites
/// no call site and the subscriber's operation.
/// DISCRIMINATION: `InheritedFactRef::witness_callsite_id()` returns the
/// rep's call site → (a) fails.
#[test]
fn t4_policy_evidence_carries_the_first_hop_witnesses() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    ts::write_single_app(&ws, &ts::T4_FILES);
    let view = view_of(&ws, true).expect("single-app target");
    let out = run_on_view(
        &view,
        "version: 1
rules:
  - id: t4-exec
    severity: high
    when:
      capability.op: execute
    facts: inherited
  - id: t4-modify
    severity: high
    when:
      capability.op: modify
    facts: inherited
",
    );
    let det = view.det();

    // (a)
    let (caller, runner) = (
        routine_named(&view, "Caller"),
        routine_named(&view, "Runner"),
    );
    let runner_cs = det.summaries[runner.id.as_str()]
        .capability_facts_direct
        .iter()
        .find(|f| f.op == "execute")
        .and_then(|f| f.witness_callsite_id.clone())
        .expect("precondition: Runner's direct execute has its own call site");
    let caller_cs = caller
        .call_sites
        .iter()
        .find(|c| {
            matches!(&c.callee, crate::program::body::features::PCallee::Bare { name } if name == "Runner")
        })
        .expect("precondition: Caller's call site to Runner")
        .id
        .clone();
    let a = matched_steps(&out, "t4-exec", caller);
    assert_eq!(a.len(), 1, "{a:?}");
    assert_eq!(a[0].callsite_id.as_deref(), Some(caller_cs.as_str()));
    assert_ne!(a[0].callsite_id.as_deref(), Some(runner_cs.as_str()));

    // (b)
    let ins = routine_named(&view, "Inserter");
    let b = matched_steps(&out, "t4-modify", ins);
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].callsite_id, None, "a trigger hop has no call site");
    let trig_op = b[0].operation_id.clone().expect("the trigger's operation");
    assert!(
        !ins.record_operations.iter().any(|o| o.id == trig_op),
        "the operation is the trigger's, not the inserter's"
    );

    // (c)
    let publisher = routine_named(&view, "OnT4");
    let handle = routine_named(&view, "Handle");
    let c = matched_steps(&out, "t4-modify", publisher);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].callsite_id, None, "an event hop has no call site");
    assert!(
        handle
            .record_operations
            .iter()
            .any(|o| Some(&o.id) == c[0].operation_id.as_ref()),
        "the operation is the subscriber's Modify"
    );
}

/// Cone-memory step 1, T8 (policy half): `Writer` is an explicitly composed
/// subject with NO inherited facts (its raw entry is `Some(empty)`) and a
/// summary; a `facts: inherited` rule evaluates it and passes it.
/// DISCRIMINATION: drop empty entries at emission → policy's "summary but no
/// raw inherited facts" panic fires here.
#[test]
fn t8_an_empty_subject_is_composed_not_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    ts::write_single_app(&ws, &ts::T8_FILES);
    let view = view_of(&ws, true).expect("single-app target");
    let out = run_on_view(
        &view,
        "version: 1
rules:
  - id: t8-inherited
    severity: high
    when:
      capability.op: insert
    facts: inherited
",
    );
    let (writer, caller) = (
        routine_named(&view, "Writer"),
        routine_named(&view, "Caller"),
    );
    assert!(
        view.det().summaries.contains_key(writer.id.as_str()),
        "precondition: Writer has a summary"
    );
    assert!(
        view.raw().get(&writer.id).is_some_and(|s| s.is_empty()),
        "Writer's entry is composed and empty"
    );
    assert_eq!(out.rule_summaries[0].routines_evaluated, 2);
    assert!(
        !out.findings
            .iter()
            .any(|f| f.id == format!("policy-t8-inherited/{}", writer.id)),
        "Writer inherits nothing"
    );
    assert_eq!(
        matched_steps(&out, "t8-inherited", caller).len(),
        1,
        "precondition: Caller inherits the producer's insert"
    );
}
