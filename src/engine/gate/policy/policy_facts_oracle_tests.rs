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

fn render(mut facts: Vec<&CapabilityFact>) -> Vec<String> {
    facts.sort_by_cached_key(|f| fact_sort_key(f));
    facts.iter().map(|f| format!("{f:?}")).collect()
}

/// A view prepared as `run_policy_check` prepares it, its run taken.
fn view_of(ws: &Path, single_app: bool) -> Result<View, String> {
    let inputs = WorkspaceInputs::read(ws, &Default::default());
    let target = AnalysisTarget::build(&inputs, single_app, "oracle")?;
    let mut view = View::new(target);
    let plan = policy_plan(default_detectors(), true);
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
        let inherited = raw.get(r.id.as_str()).map_or(&[][..], Vec::as_slice);
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
        raw.keys().map(String::as_str).collect::<BTreeSet<_>>(),
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
    raw.remove(&id);
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
