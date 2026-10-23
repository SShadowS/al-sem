//! #220 (#160 step 8): roles, snapshot schema 4, preflight — the integration
//! half of the spec's test plan
//! (`docs/superpowers/specs/2026-10-22-issue-220-roles-snapshot-schema-4-design.md`
//! §5). Cross-app models come from `AnalysisTarget::build(inputs, false, ..)`
//! and the composition from `compose_full_snapshot` with
//! `SnapshotUniverse::of(&target)`.
//!
//! Fixtures: DD = `tests/common/dep_dispatch_fixture.rs` (`write_own`;
//! embedded-source dependency `XBase`); R5B =
//! `tests/r2-5b-fixtures/cross-app-resolution` (two symbol-only apps, `dddd…`
//! reachable through `Host Mgt.DriveDeps`, `eeee…` not).
//!
//! "Hand-stated" = a field set by assignment on the assembled model before
//! composing (CLAUDE.md).

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use al_sem::engine::gate::cbor::CborValue;
use al_sem::engine::gate::diff::CoveragePolicy;
use al_sem::engine::gate::diff::cli::{DiffCliOptions, DiffRunOutcome, run_diff};
use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::engine::l4::capability_cone::R3a5CrossAppBase;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::engine::l5::snapshot_full::{
    FullSnapshotOptions, SnapshotUniverse, compose_full_snapshot, to_sorted_json,
};
use al_sem::program::inputs::InputSet;
use al_sem::program::model::workspace::Model;
use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const DRIVER: &str = "cli-b-v1";
const R5B_D: &str = "dddddddd-0000-0000-0000-000000000001";
const R5B_E: &str = "eeeeeeee-0000-0000-0000-000000000002";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn r5b() -> PathBuf {
    repo().join("tests/r2-5b-fixtures/cross-app-resolution")
}

struct Built {
    inputs: Arc<InputSet>,
    target: AnalysisTarget,
}

fn build(ws: &Path, single_app: bool) -> Built {
    let inputs = InputSet::read(ws, &crate::fingerprint_needs());
    let target = AnalysisTarget::build(&inputs, single_app, "t220").expect("analysis target");
    Built { inputs, target }
}

fn base(t: &AnalysisTarget) -> &R3a5CrossAppBase {
    match t {
        AnalysisTarget::CrossApp(b) => b,
        AnalysisTarget::SingleApp(_) => panic!("a cross-app target"),
    }
}

fn model_mut(t: &mut AnalysisTarget) -> &mut Model {
    match t {
        AnalysisTarget::CrossApp(b) => &mut b.resolved,
        AnalysisTarget::SingleApp(m) => m,
    }
}

fn compose_with(b: &Built, universe: SnapshotUniverse) -> CborValue {
    compose_full_snapshot(
        b.target.resolved(),
        &FullSnapshotOptions {
            inputs: &b.inputs,
            driver_version: DRIVER,
            deterministic: true,
            roots_config_ignored: false,
            universe,
        },
    )
}

/// The snapshot of `b` in its own universe.
fn compose(b: &Built) -> CborValue {
    compose_with(b, SnapshotUniverse::of(&b.target))
}

fn json(tree: &CborValue) -> Value {
    serde_json::from_str(&to_sorted_json(tree)).expect("snapshot JSON")
}

fn rows<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is an array"))
}

fn starts_ci(s: &str, prefix: &str) -> bool {
    s.to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

fn primary_guid(m: &Model) -> String {
    m.primary_app
        .as_ref()
        .expect("primary app")
        .app_guid
        .clone()
}

fn routine_named<'a>(
    m: &'a Model,
    name: &str,
) -> &'a al_sem::program::model::workspace::ModelRoutine {
    m.workspace
        .routines
        .iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("routine {name}"))
}

fn stable(m: &Model, name: &str) -> String {
    routine_named(m, name).stable_routine_id.clone()
}

fn hand_state_parse_incomplete(t: &mut AnalysisTarget, names: &[&str]) {
    let m = model_mut(t);
    for name in names {
        let r = m
            .workspace
            .routines
            .iter_mut()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("routine {name}"));
        r.parse_incomplete = true;
    }
}

/// Reach over the CONE (`rel.cone()`): BFS from every primary routine.
fn cone_reach(b: &R3a5CrossAppBase) -> HashSet<String> {
    let mut adj: std::collections::HashMap<&str, Vec<String>> = Default::default();
    for e in b.rel.cone() {
        if let Some(to) = &e.to {
            adj.entry(e.from.as_str()).or_default().push(to.clone());
        }
    }
    bfs(b, |from| adj.get(from).cloned().unwrap_or_default())
}

/// Reach over the CUT (`rel.primary()`: primary callers' calls plus event
/// dispatch).
fn cut_reach(b: &R3a5CrossAppBase) -> HashSet<String> {
    let g = b.rel.primary();
    bfs(b, |from| {
        g.edges_by_from
            .get(from)
            .map(|es| es.iter().map(|e| e.to.clone()).collect())
            .unwrap_or_default()
    })
}

fn bfs<F>(b: &R3a5CrossAppBase, next: F) -> HashSet<String>
where
    F: Fn(&str) -> Vec<String>,
{
    let primary = primary_guid(&b.resolved);
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for r in &b.resolved.workspace.routines {
        if r.app_guid.eq_ignore_ascii_case(&primary) && seen.insert(r.id.clone()) {
            queue.push_back(r.id.clone());
        }
    }
    while let Some(id) = queue.pop_front() {
        for to in next(&id) {
            if seen.insert(to.clone()) {
                queue.push_back(to);
            }
        }
    }
    seen
}

/// The serialized gap rows of `kind`.
fn gaps<'a>(snap: &'a Value, kind: &str) -> Vec<&'a Value> {
    rows(snap, "analysisGaps")
        .iter()
        .filter(|g| g["kind"] == kind)
        .collect()
}

/// The in-memory gap table (unfiltered, I8), as JSON rows.
fn memory_gaps(m: &Model) -> Vec<Value> {
    compose_snapshot(m)
        .analysis_gaps
        .iter()
        .map(|g| serde_json::to_value(g).unwrap())
        .collect()
}

fn dd_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    dd::write_own(dir.path(), "");
    dir
}

// ---------------------------------------------------------------------------
// Gaps (D2): T3, T4, T4c, T5
// ---------------------------------------------------------------------------

/// T3: in memory, `symbol-only-boundary` for BOTH R5B apps (each `role:
/// "dependency"`); `eeee…` rows are model rows and none is reachable.
/// Serialized cross-app: `dddd…` present, `eeee…` absent.
///
/// Break 1: skip the reach filter -> `eeee…` appears. Break 2: the stub back
/// to `false` -> `dddd…` missing. Both directions of Decision 46.
#[test]
fn gaps_reachable_only() {
    let b = build(&r5b(), false);
    let x = base(&b.target);
    let model = &x.resolved;
    let reach = cone_reach(x);
    let e_rows: Vec<_> = model
        .workspace
        .routines
        .iter()
        .filter(|r| r.app_guid.eq_ignore_ascii_case(R5B_E))
        .collect();
    assert!(
        !e_rows.is_empty(),
        "precondition: eeee… rows are model rows"
    );
    assert!(
        e_rows.iter().all(|r| !reach.contains(&r.id)),
        "precondition: no eeee… row is reachable"
    );
    let mem = memory_gaps(model);
    for app in [R5B_D, R5B_E] {
        let g = mem
            .iter()
            .find(|g| {
                g["kind"] == "symbol-only-boundary"
                    && starts_ci(g["subject"].as_str().unwrap(), app)
            })
            .unwrap_or_else(|| {
                panic!("precondition: in-memory symbol-only-boundary for {app}: {mem:?}")
            });
        assert_eq!(g["role"], "dependency", "in-memory gap role for {app}");
    }

    let snap = json(&compose(&b));
    let sob = gaps(&snap, "symbol-only-boundary");
    assert!(
        sob.iter()
            .any(|g| starts_ci(g["subject"].as_str().unwrap(), R5B_D) && g["role"] == "dependency"),
        "the reachable opaque app keeps its gap: {sob:?}"
    );
    assert!(
        sob.iter()
            .all(|g| !starts_ci(g["subject"].as_str().unwrap(), R5B_E)),
        "the unreachable opaque app has no serialized gap: {sob:?}"
    );
}

const T4_DEP: &str = "cccc0221-0000-0000-0000-000000000221";

/// T4's workspace: an embedded-source dependency `T4 Lib` with `A` (called by
/// the primary `W`), and a reverse-demand raiser `R` that raises `OnT4Event`,
/// which the primary `OnEv` subscribes to.
fn write_t4(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("app.json"),
        format!(
            r#"{{"id":"aaaa0221-0000-0000-0000-000000000221","name":"T4Ws","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{T4_DEP}","name":"T4Dep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/Ws.al"),
        r#"codeunit 50180 "T4 Ws"
{
    procedure W()
    var
        L: Codeunit "T4 Lib";
    begin
        L.A();
    end;
}

codeunit 50181 "T4 Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"T4 Lib", 'OnT4Event', '', false, false)]
    local procedure OnEv()
    begin
    end;
}
"#,
    )
    .unwrap();
    let lib = r#"codeunit 20 "T4 Lib"
{
    procedure A()
    begin
    end;

    procedure R()
    begin
        OnT4Event();
    end;

    [IntegrationEvent(false, false)]
    procedure OnT4Event()
    begin
    end;
}
"#;
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":20,"Name":"T4 Lib","Methods":[{{"Name":"A","Parameters":[]}},{{"Name":"R","Parameters":[]}},{{"Name":"OnT4Event","Parameters":[],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"False"}},{{"Value":"False"}}]}}]}}]}}],"AppId":"{T4_DEP}","Name":"T4Dep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_source_app(
        &dir.join(".alpackages").join("probe_T4Dep_1.0.0.0.app"),
        T4_DEP,
        "T4Dep",
        "1.0.0.0",
        &symbols,
        &[("src/Lib.al", lib)],
        "",
    );
}

/// T4: `R` is a model row NOT in reach; `A`, `R`, `W` hand-stated
/// `parse_incomplete` (F10). Serialized cross-app: `W` (role `primary`) and
/// `A` gapped, `R` not.
///
/// Break 1: no reach filter -> `R` appears. Break 2: hard-code `role:
/// "dependency"` -> `W`'s `role == "primary"` fails (S7: `W` is a BFS root, so
/// only the annotation discriminates the primary arm).
#[test]
fn parse_incomplete_gap_filter() {
    let dir = tempfile::tempdir().unwrap();
    write_t4(dir.path());
    let mut b = build(dir.path(), false);
    {
        let x = base(&b.target);
        let reach = cone_reach(x);
        let r = routine_named(&x.resolved, "R");
        assert!(
            r.app_guid.eq_ignore_ascii_case(T4_DEP),
            "precondition: R is a dependency model row"
        );
        assert!(!reach.contains(&r.id), "precondition: R is not reachable");
        assert!(
            reach.contains(&routine_named(&x.resolved, "A").id),
            "precondition: A is reachable"
        );
    }
    hand_state_parse_incomplete(&mut b.target, &["A", "R", "W"]);
    let model = b.target.resolved();
    let (w, a, r) = (stable(model, "W"), stable(model, "A"), stable(model, "R"));

    let snap = json(&compose(&b));
    let pi = gaps(&snap, "parse-incomplete");
    let row = |id: &str| pi.iter().find(|g| g["subject"] == id).copied();
    assert_eq!(
        row(&w).map(|g| g["role"].clone()),
        Some(Value::from("primary")),
        "W is gapped with role primary: {pi:?}"
    );
    assert_eq!(
        row(&a).map(|g| g["role"].clone()),
        Some(Value::from("dependency")),
        "A (reachable) is gapped with role dependency: {pi:?}"
    );
    assert!(
        row(&r).is_none(),
        "R (unreachable) has no serialized gap: {pi:?}"
    );
}

/// T4c (O2, S6): DD's `Inner` hand-stated `parse_incomplete`; `Inner` is
/// reachable over the CONE and NOT over the cut (both computed). Serialized
/// cross-app: a `parse-incomplete` gap for `Inner`.
///
/// Break: BFS over the cut (`rel.primary()` edges) -> the gap disappears (the
/// F9 shape P6 measured).
#[test]
fn gap_reach_follows_dependency_calls() {
    let dir = dd_dir();
    let mut b = build(dir.path(), false);
    {
        let x = base(&b.target);
        let inner = routine_named(&x.resolved, "Inner").id.clone();
        assert!(
            cone_reach(x).contains(&inner),
            "precondition: Inner is reachable over the cone"
        );
        assert!(
            !cut_reach(x).contains(&inner),
            "precondition: Inner is NOT reachable over the cut"
        );
    }
    hand_state_parse_incomplete(&mut b.target, &["Inner"]);
    let inner = stable(b.target.resolved(), "Inner");
    let snap = json(&compose(&b));
    let pi = gaps(&snap, "parse-incomplete");
    assert!(
        pi.iter()
            .any(|g| g["subject"] == inner.as_str() && g["role"] == "dependency"),
        "Inner's parse-incomplete gap is serialized: {pi:?}"
    );
}

/// T5: a typed edge from `DriveDeps` targets a `dddd…` body-less row
/// (asserted on `rel.cone()`). Serialized cross-app: `symbol-only-boundary`
/// for `dddd…`, `role: "dependency"`.
///
/// Break: the stub back to `false` -> the gap is missing. (The cone-vs-cut
/// discrimination is T4c's.) Step 6 adds the `unfetched-dependency` ledger row.
#[test]
fn symbol_only_callee_has_its_gap() {
    let b = build(&r5b(), false);
    let x = base(&b.target);
    let drive = routine_named(&x.resolved, "DriveDeps").id.clone();
    let bodyless_d: BTreeSet<&str> = x
        .resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.app_guid.eq_ignore_ascii_case(R5B_D) && !r.body_available)
        .map(|r| r.id.as_str())
        .collect();
    assert!(
        x.rel
            .cone()
            .iter()
            .any(|e| e.from == drive && e.to.as_deref().is_some_and(|t| bodyless_d.contains(t))),
        "precondition: DriveDeps has a typed edge into a body-less dddd… row"
    );
    let snap = json(&compose(&b));
    let sob = gaps(&snap, "symbol-only-boundary");
    assert!(
        sob.iter()
            .any(|g| starts_ci(g["subject"].as_str().unwrap(), R5B_D) && g["role"] == "dependency"),
        "dddd…'s symbol-only-boundary gap: {sob:?}"
    );
}

fn base_mut(t: &mut AnalysisTarget) -> &mut R3a5CrossAppBase {
    match t {
        AnalysisTarget::CrossApp(b) => b,
        AnalysisTarget::SingleApp(_) => panic!("a cross-app target"),
    }
}

/// Review F-3a: the app-subject match is case-insensitive. The REACHED
/// `dddd…` rows get an upper-case `object_id` guid (hand-stated), the
/// unreached ones keep the lower-case spelling, so the lower-case
/// `symbol-only-boundary` gap names an app whose reached rows are spelled
/// differently. It is still serialized.
///
/// Break: drop the case folding in `reachable_gaps` -> the gap is dropped.
#[test]
fn gap_app_subject_matches_case_insensitively() {
    let mut b = build(&r5b(), false);
    let reach = cone_reach(base(&b.target));
    let upper = R5B_D.to_ascii_uppercase();
    let x = base_mut(&mut b.target);
    let (mut reached, mut unreached_bodyless) = (0, 0);
    for r in x.resolved.workspace.routines.iter_mut() {
        if !r.app_guid.eq_ignore_ascii_case(R5B_D) {
            continue;
        }
        if reach.contains(&r.id) {
            r.object_id = r.object_id.replacen(R5B_D, &upper, 1);
            assert!(r.object_id.starts_with(&upper), "the hand-state applied");
            reached += 1;
        } else if !r.body_available {
            unreached_bodyless += 1;
        }
    }
    assert!(reached > 0, "precondition: a reached dddd… row");
    assert!(
        unreached_bodyless > 0,
        "precondition: an unreached body-less dddd… row keeps the lower-case spelling"
    );
    let mem = memory_gaps(b.target.resolved());
    assert!(
        mem.iter()
            .any(|g| g["kind"] == "symbol-only-boundary" && g["subject"] == R5B_D),
        "precondition: a lower-case dddd… gap in memory: {mem:?}"
    );
    let snap = json(&compose(&b));
    let sob = gaps(&snap, "symbol-only-boundary");
    assert!(
        sob.iter().any(|g| g["subject"] == R5B_D),
        "the lower-case dddd… gap is kept: {sob:?}"
    );
}

/// Review F-3b: the F21 assertion fires when an app WITH source owns a
/// reached body-less row (`dddd…`'s `has_source` hand-stated `true`).
///
/// Break: delete the `debug_assert!` -> no panic.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a symbol-only-boundary gap names an app with source")]
fn opaque_gap_of_an_app_with_source_fires() {
    let mut b = build(&r5b(), false);
    let app = base_mut(&mut b.target)
        .dependency_apps
        .iter_mut()
        .find(|a| a.guid.eq_ignore_ascii_case(R5B_D))
        .expect("precondition: dddd… is a dependency app");
    assert!(!app.has_source, "precondition: dddd… is symbol-only");
    app.has_source = true;
    let _ = compose(&b);
}

// ---------------------------------------------------------------------------
// Preflight (D5): T7
// ---------------------------------------------------------------------------

fn vendored(name: &str) -> PathBuf {
    repo().join("tests/cli-b-goldens/diff/snapshots").join(name)
}

/// A temp copy of a vendored diff input, upgraded to v4 (D6's three edits),
/// then `edit` applied.
fn v4_copy(dir: &Path, name: &str, out: &str, edit: impl Fn(&mut Value)) -> PathBuf {
    let mut v: Value = serde_json::from_slice(&std::fs::read(vendored(name)).unwrap()).unwrap();
    v["schemaVersion"] = Value::from(4);
    v["analysisUniverse"] = Value::from("single-app");
    for app in v["apps"].as_array_mut().unwrap() {
        app["role"] = Value::from("primary");
    }
    edit(&mut v);
    let p = dir.join(out);
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    p
}

fn diff(old: &Path, new: &Path) -> DiffRunOutcome {
    let (o, n) = (
        old.to_string_lossy().to_string(),
        new.to_string_lossy().to_string(),
    );
    run_diff(&DiffCliOptions {
        old_arg: &o,
        new_arg: &n,
        format: "json",
        out: None,
        coverage_policy: CoveragePolicy::Loose,
        renames_path: None,
        fail_on: None,
        strict: false,
        deterministic: true,
        driver_version: DRIVER,
    })
}

/// T7: a temp copy of a vendored input edited back to `schemaVersion: 3`,
/// diffed against a v4 file. stderr names 3, v4 and the remedy; exit 1.
///
/// Break: accept 3 -> loads, fails.
#[test]
fn diff_refuses_schema_3() {
    let dir = tempfile::tempdir().unwrap();
    let old = v4_copy(dir.path(), "event-added-old.snap.json", "v3.json", |v| {
        v["schemaVersion"] = Value::from(3);
    });
    let new = v4_copy(dir.path(), "event-added-new.snap.json", "v4.json", |_| {});
    let v: Value = serde_json::from_slice(&std::fs::read(&old).unwrap()).unwrap();
    assert_eq!(v["schemaVersion"], 3, "precondition: the copy carries 3");
    let outcome = diff(&old, &new);
    assert_eq!(outcome.exit_code, 1);
    let msg = outcome.error_message.expect("a load error");
    assert!(msg.contains("schemaVersion 3"), "{msg}");
    assert!(msg.contains("(v4)"), "{msg}");
    assert!(
        msg.contains("re-create it with 'alsem fingerprint'"),
        "{msg}"
    );
}

// ---------------------------------------------------------------------------
// Key classes (D4): T12
// ---------------------------------------------------------------------------

/// T12 (O18): two cross-app targets built from the same workspace (equal
/// routine ids, asserted). Composing target A's model with
/// `SnapshotUniverse::of(&target_b)` panics with the model-identity message;
/// A with A composes.
///
/// Break: drop the `ptr::eq` assert -> it composes silently.
#[test]
fn cross_app_universe_binds_its_model() {
    let dir = dd_dir();
    let a = build(dir.path(), false);
    let b = build(dir.path(), false);
    let ids = |x: &Built| -> Vec<String> {
        x.target
            .resolved()
            .workspace
            .routines
            .iter()
            .map(|r| r.id.clone())
            .collect()
    };
    assert_eq!(ids(&a), ids(&b), "precondition: equal routine ids");
    let _ = compose(&a);
    let wrong = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_with(&a, SnapshotUniverse::of(&b.target))
    }));
    let err = wrong.expect_err("composing A's model with B's universe must panic");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert_eq!(
        msg, "compose_full_snapshot: the model is not the cross-app universe's model",
        "the model-identity message"
    );
}
