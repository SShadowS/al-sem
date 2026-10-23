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
//! reachable through `Host Mgt.DriveDeps`, `eeee…` not); R5B-F = a temp copy of
//! R5B whose primary guid sorts LAST.
//!
//! "Hand-stated" = a field set by assignment on the assembled model before
//! composing (CLAUDE.md).

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use al_sem::engine::gate::cbor::CborValue;
use al_sem::engine::gate::diff::cli::{DiffCliOptions, DiffRunOutcome, run_diff};
use al_sem::engine::gate::diff::{CoveragePolicy, Severity};
use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::engine::gate::snapshot_deserialize::deserialize_snapshot;
use al_sem::engine::l4::capability_cone::R3a5CrossAppBase;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::engine::l5::snapshot_full::{
    Domain, FullSnapshotOptions, SNAPSHOT_KEYS, SnapshotUniverse, build_inventory_envelope,
    closure_violations, compose_full_snapshot, is_classified, is_reference, to_sorted_json,
};
use al_sem::program::inputs::InputSet;
use al_sem::program::model::workspace::Model;
use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const DRIVER: &str = "cli-b-v1";
const R5B_PRIMARY: &str = "11111111-0000-0000-0000-0000000000aa";
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

/// The E6 multi-hop shape (final panel G1): DD, but `XBase.Inner` calls
/// `Z Deep.Work` of a third, symbol-only app `ZDeep` that `XBase` depends on.
const ZDEEP_GUID: &str = "cccc7070-0000-0000-0000-000000000070";

fn write_multi_hop(dir: &Path) {
    dd::write_workspace(dir, "");
    let lib = "codeunit 10 \"Dep Lib\"\n{\n    procedure Outer()\n    begin\n        Inner();\n    end;\n\n    procedure Inner()\n    var\n        Deep: Codeunit \"Z Deep\";\n    begin\n        Deep.Work();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnDepEvent()\n    begin\n    end;\n}\n";
    let customer = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
    let base_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#,
        dd::DEP_GUID
    );
    crate::symbol_app::write_source_app(
        &dir.join(".alpackages").join(dd::DEP_APP_FILE),
        dd::DEP_GUID,
        "XBase",
        "1.0.0.0",
        &base_symbols,
        &[("src/Customer.al", customer), ("src/Lib.al", lib)],
        &format!(
            r#"<Dependencies><Dependency Id="{ZDEEP_GUID}" Name="ZDeep" Publisher="probe" MinVersion="1.0.0.0" /></Dependencies>"#
        ),
    );
    let deep_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":20,"Name":"Z Deep","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"AppId":"{ZDEEP_GUID}","Name":"ZDeep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_symbol_app(
        &dir.join(".alpackages/probe_ZDeep_1.0.0.0.app"),
        ZDEEP_GUID,
        "ZDeep",
        "1.0.0.0",
        &deep_symbols,
    );
}

/// Final panel G1: spec §4.4 step (6)'s "an app a kept gap references" arm.
/// Multi-hop (`write_multi_hop`): `ZDeep` owns no serialized identity (no
/// kept row on the cut names its routine) and its `symbol-only-boundary` gap
/// is kept (reachable over the cone through `Inner`), both asserted. Its
/// `apps[]` row exists with `role: "dependency"`, and the closure holds.
///
/// Break: drop `|| referenced_apps.contains(..)` from `project_apps` -> no
/// `ZDeep` row, and the gap's app reference dangles.
#[test]
fn gap_app_gets_its_apps_row() {
    let dir = tempfile::tempdir().unwrap();
    write_multi_hop(dir.path());
    let b = build(dir.path(), false);
    let tree = compose(&b);
    let snap = json(&tree);
    let ids: Vec<&str> = snap["identities"]["stableIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        !ids.iter().any(|s| starts_ci(s, ZDEEP_GUID)),
        "precondition: ZDeep owns no serialized identity: {ids:?}"
    );
    let sob = gaps(&snap, "symbol-only-boundary");
    assert!(
        sob.iter().any(|g| g["subject"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(ZDEEP_GUID))),
        "precondition: ZDeep's symbol-only-boundary gap is kept: {sob:?}"
    );
    let apps = rows(&snap, "apps");
    assert!(
        apps.iter().any(|a| a["appGuid"]
            .as_str()
            .is_some_and(|g| g.eq_ignore_ascii_case(ZDEEP_GUID))
            && a["role"] == "dependency"),
        "ZDeep's apps[] row: {apps:?}"
    );
    let universe = SnapshotUniverse::of(&b.target);
    let violations = closure_violations(&tree, b.target.resolved(), &universe);
    assert!(violations.is_empty(), "{violations:?}");
}

/// Final panel C1: a dependency routine whose parse error left it with no
/// body (hand-stated on DD's `Inner`: `parse_incomplete`, `body_available`
/// false) does not make its app, which ships source, a `symbol-only-boundary`.
/// Serialized cross-app: `Inner`'s `parse-incomplete` gap, and no app-level
/// gap for DD's dependency.
///
/// Break: count `parse_incomplete` rows into the opaque apps -> the app gap
/// appears (or the F21 `has_source` assert fires).
#[test]
fn parse_error_body_is_not_symbol_only() {
    let dir = dd_dir();
    let mut b = build(dir.path(), false);
    {
        let x = base(&b.target);
        assert!(
            x.dependency_apps
                .iter()
                .any(|a| a.guid.eq_ignore_ascii_case(dd::DEP_GUID) && a.has_source),
            "precondition: DD's dependency ships source"
        );
    }
    let m = model_mut(&mut b.target);
    let r = m
        .workspace
        .routines
        .iter_mut()
        .find(|r| r.name == "Inner")
        .expect("routine Inner");
    r.parse_incomplete = true;
    r.body_available = false;
    let inner = stable(b.target.resolved(), "Inner");
    let snap = json(&compose(&b));
    assert!(
        gaps(&snap, "parse-incomplete")
            .iter()
            .any(|g| g["subject"] == inner.as_str()),
        "Inner's parse-incomplete gap: {:?}",
        snap["analysisGaps"]
    );
    let sob = gaps(&snap, "symbol-only-boundary");
    assert!(
        !sob.iter()
            .any(|g| starts_ci(g["subject"].as_str().unwrap_or(""), dd::DEP_GUID)),
        "a parse error is not a symbol-only app: {sob:?}"
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

/// F22 (review F4): a serialized identity's dependency app must be in
/// `dependency_apps`; `project_apps` never guesses a row. DD with its
/// dependency app hand-removed from `dependency_apps` (its identities, e.g.
/// the referenced `Outer`, stay) fires the assertion.
///
/// Break: delete the `debug_assert!` -> no panic.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a serialized identity's app is missing from dependency_apps")]
fn identity_app_missing_from_dependency_apps_fires() {
    let dir = dd_dir();
    let mut b = build(dir.path(), false);
    let apps = &mut base_mut(&mut b.target).dependency_apps;
    let before = apps.len();
    apps.retain(|a| !a.guid.eq_ignore_ascii_case(dd::DEP_GUID));
    assert_eq!(apps.len() + 1, before, "precondition: DD's dependency app");
    let _ = compose(&b);
}

// ---------------------------------------------------------------------------
// Preflight (D5): T6, T6b, T7, T7b
// ---------------------------------------------------------------------------

fn vendored(name: &str) -> PathBuf {
    repo().join("tests/cli-b-goldens/diff/snapshots").join(name)
}

/// A temp copy of a vendored diff input, upgraded to v4 (D6's three edits),
/// then `edit` applied.
fn v4_copy(dir: &Path, name: &str, out: &str, edit: impl Fn(&mut Value)) -> PathBuf {
    let mut v: Value = serde_json::from_slice(&std::fs::read(vendored(name)).unwrap()).unwrap();
    // The vendored inputs carry D6's three edits themselves (final panel F5:
    // asserted, never re-applied here, so a stale input header fails).
    assert_eq!(v["schemaVersion"], 4, "precondition: {name} is v4");
    assert_eq!(
        v["analysisUniverse"], "single-app",
        "precondition: {name} states its universe"
    );
    let apps = v["apps"].as_array().expect("precondition: apps[]");
    assert!(
        apps.len() == 1 && apps[0]["role"] == "primary",
        "precondition: {name} has one primary apps[] row: {apps:?}"
    );
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

fn loads(p: &Path) -> Result<CborValue, String> {
    deserialize_snapshot(&std::fs::read(p).unwrap(), None)
}

fn diag_kinds(outcome: &DiffRunOutcome) -> (Vec<String>, usize) {
    let out: Value = serde_json::from_str(outcome.output.as_deref().expect("diff output")).unwrap();
    let kinds = out["payload"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap().to_string())
        .collect();
    (kinds, out["payload"]["findings"].as_array().unwrap().len())
}

/// T6: two v4 files of the same app, `single-app` vs `cross-app` (both load).
/// `analysis-universe-mismatch` in the diagnostics, no findings, exit 1.
///
/// Break: drop the check -> findings computed, exit 0.
#[test]
fn diff_refuses_mixed_universes() {
    let dir = tempfile::tempdir().unwrap();
    let old = v4_copy(dir.path(), "event-added-old.snap.json", "old.json", |_| {});
    let new = v4_copy(dir.path(), "event-added-new.snap.json", "new.json", |v| {
        v["analysisUniverse"] = Value::from("cross-app");
    });
    for p in [&old, &new] {
        if let Err(e) = loads(p) {
            panic!("precondition: {} loads: {e}", p.display());
        }
    }
    let outcome = diff(&old, &new);
    let (kinds, findings) = diag_kinds(&outcome);
    assert!(
        kinds.iter().any(|k| k == "analysis-universe-mismatch"),
        "{kinds:?}"
    );
    assert_eq!(findings, 0, "a fatal preflight computes no findings");
    assert_eq!(outcome.exit_code, 1);
}

/// T6b (P9): two vendored inputs of DIFFERENT apps (both load): exit 1 with
/// `app-identity-mismatch`, also under `--fail-on low`; stdout is the same
/// with and without the threshold.
///
/// Break: drop the `fatal` exit gate -> exit 0.
#[test]
fn fatal_preflight_exits_one() {
    let old = vendored("event-added-old.snap.json");
    let new = vendored("schema-field-removed-new.snap.json");
    for p in [&old, &new] {
        if let Err(e) = loads(p) {
            panic!("precondition: {} loads: {e}", p.display());
        }
    }
    let app = |p: &Path| -> String {
        let v: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        v["apps"][0]["appGuid"].as_str().unwrap().to_string()
    };
    assert_ne!(app(&old), app(&new), "precondition: different apps");
    let outcome = diff(&old, &new);
    let (kinds, _) = diag_kinds(&outcome);
    assert!(
        kinds.iter().any(|k| k == "app-identity-mismatch"),
        "{kinds:?}"
    );
    assert_eq!(outcome.exit_code, 1, "a fatal preflight exits 1");

    let (o, n) = (
        old.to_string_lossy().to_string(),
        new.to_string_lossy().to_string(),
    );
    let fail_on_low = run_diff(&DiffCliOptions {
        old_arg: &o,
        new_arg: &n,
        format: "json",
        out: None,
        coverage_policy: CoveragePolicy::Loose,
        renames_path: None,
        fail_on: Some(Severity::Low),
        strict: false,
        deterministic: true,
        driver_version: DRIVER,
    });
    assert_eq!(fail_on_low.exit_code, 1, "--fail-on low: still exit 1");
    assert_eq!(fail_on_low.output, outcome.output, "stdout unchanged");
}

/// T7b (S14, O6): five temp copies, each differing from a valid v4 file in ONE
/// field (asserted): no `analysisUniverse`; `analysisUniverse: "both"`; an
/// `apps[]` row without `role`; two `primary` rows; a dependency shard. Each is
/// refused at load, exit 1, and the message names the field (the shard case
/// names the hint). Final panel: an unknown role beside the primary (F4), and
/// a missing or non-array `apps` (F3, G6), are refused too. A v4 file with an
/// empty `apps[]` (no `app.json` id) loads.
///
/// Break: drop each check in turn -> that copy loads.
#[test]
fn diff_refuses_bad_universe_or_roles() {
    let dir = tempfile::tempdir().unwrap();
    let src = "event-added-old.snap.json";
    let valid = v4_copy(dir.path(), src, "valid.json", |_| {});
    let other = v4_copy(
        dir.path(),
        "event-added-new.snap.json",
        "other.json",
        |_| {},
    );
    let valid_v: Value = serde_json::from_slice(&std::fs::read(&valid).unwrap()).unwrap();
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit, &str)> = vec![
        (
            "no-universe",
            Box::new(|v: &mut Value| {
                v.as_object_mut().unwrap().remove("analysisUniverse");
            }),
            "analysisUniverse",
        ),
        (
            "both",
            Box::new(|v: &mut Value| v["analysisUniverse"] = Value::from("both")),
            "analysisUniverse",
        ),
        (
            "no-role",
            Box::new(|v: &mut Value| {
                v["apps"][0].as_object_mut().unwrap().remove("role");
            }),
            // The row's own field, not the exactly-one-primary rule that a
            // single role-less row also breaks.
            "apps[0].role",
        ),
        (
            "two-primaries",
            Box::new(|v: &mut Value| {
                let mut second = v["apps"][0].clone();
                second["appGuid"] = Value::from("dd000000-0000-0000-0000-0000000000ff");
                v["apps"].as_array_mut().unwrap().push(second);
            }),
            "primary",
        ),
        (
            "dependency-shard",
            Box::new(|v: &mut Value| v["apps"][0]["role"] = Value::from("dependency")),
            "diff the full snapshot or the primary shard",
        ),
        (
            // Final panel F4: a second row with an unknown role, beside the
            // one primary, so only the role-value check can refuse it.
            "unknown-role",
            Box::new(|v: &mut Value| {
                let mut second = v["apps"][0].clone();
                second["appGuid"] = Value::from("dd000000-0000-0000-0000-0000000000ee");
                second["role"] = Value::from("owner");
                v["apps"].as_array_mut().unwrap().push(second);
            }),
            "apps[1].role",
        ),
        (
            // Final panel F3/G6: no `apps` key at all is not an empty apps[].
            "no-apps-key",
            Box::new(|v: &mut Value| {
                v.as_object_mut().unwrap().remove("apps");
            }),
            "apps must be an array",
        ),
        (
            "apps-not-array",
            Box::new(|v: &mut Value| v["apps"] = Value::Null),
            "apps must be an array",
        ),
    ];
    for (name, edit, needle) in &cases {
        let p = v4_copy(dir.path(), src, &format!("{name}.json"), |v| edit(v));
        let v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        // Precondition: exactly one top-level field differs from the valid file.
        let changed: Vec<&String> = valid_v
            .as_object()
            .unwrap()
            .keys()
            .chain(v.as_object().unwrap().keys())
            .filter(|k| valid_v.get(k.as_str()) != v.get(k.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            changed.len(),
            1,
            "precondition {name}: one field differs: {changed:?}"
        );
        let outcome = diff(&p, &other);
        assert_eq!(outcome.exit_code, 1, "{name}: refused");
        let msg = outcome.error_message.unwrap_or_default();
        assert!(
            msg.contains(needle),
            "{name}: the message names {needle:?}: {msg}"
        );
    }
    // The valid file itself loads, and so does one with an empty `apps[]`.
    if let Err(e) = loads(&valid) {
        panic!("the valid v4 file loads: {e}");
    }
    let no_apps = v4_copy(dir.path(), src, "no-apps.json", |v| {
        v["apps"] = Value::Array(Vec::new());
    });
    if let Err(e) = loads(&no_apps) {
        panic!("an empty apps[] v4 file loads: {e}");
    }
}

// ---------------------------------------------------------------------------
// Key classes (D4): T12
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Key classes (D4): T9a, T9b, T9d, T9-closure, T9e, T9f, T9-keys, T10, T12
// ---------------------------------------------------------------------------

/// T9a Inventory (DD cross-app). In memory: the model holds the dependency
/// objects `Dep Lib`/`Dep Sub` (contractFacts' source) and the dependency
/// table `Customer` (18) (schemaFacts', S15); the unfiltered core holds
/// dependency `rootClassifications` and `eventDeclarations` rows; the model
/// holds `Outer` (the inventory's source). Serialized cross-app: none of
/// them, and the inventory doc's `routineInventory` is primary only.
///
/// Break: identity filter -> they appear.
#[test]
fn key_class_inventory() {
    let dir = dd_dir();
    let b = build(dir.path(), false);
    let model = b.target.resolved();
    let dep = dd::DEP_GUID;
    for name in ["Dep Lib", "Dep Sub"] {
        assert!(
            model
                .workspace
                .objects
                .iter()
                .any(|o| o.name == name && o.app_guid.eq_ignore_ascii_case(dep)),
            "precondition: dependency object {name}"
        );
    }
    assert!(
        model
            .workspace
            .tables
            .iter()
            .any(|t| t.table_number == 18 && t.app_guid.eq_ignore_ascii_case(dep)),
        "precondition: dependency table Customer (18)"
    );
    let core = compose_snapshot(model);
    assert!(
        core.root_classifications.iter().any(|r| starts_ci(
            serde_json::to_value(r).unwrap()["routineId"]
                .as_str()
                .unwrap_or(""),
            dep
        )),
        "precondition: a dependency rootClassifications row"
    );
    assert!(
        core.event_declarations.iter().any(|e| starts_ci(
            serde_json::to_value(e).unwrap()["routine"]
                .as_str()
                .unwrap_or(""),
            dep
        )),
        "precondition: a dependency eventDeclarations row"
    );
    let outer = stable(model, "Outer");

    let tree = compose(&b);
    let snap = json(&tree);
    let dep_rows = |key: &str, field: &str| -> Vec<Value> {
        rows(&snap, key)
            .iter()
            .filter(|r| starts_ci(r[field].as_str().unwrap_or(""), dep))
            .cloned()
            .collect()
    };
    for (key, field) in [
        ("contractFacts", "stableId"),
        ("schemaFacts", "stableId"),
        ("permissionFacts", "subject"),
        ("rootClassifications", "routineId"),
        ("eventDeclarations", "routine"),
    ] {
        assert_eq!(
            dep_rows(key, field),
            Vec::<Value>::new(),
            "{key}: no dependency row"
        );
    }
    let inv: Value =
        serde_json::from_str(&build_inventory_envelope(&tree, model, DRIVER, true)).unwrap();
    let ids: Vec<&str> = inv["payload"]["routineInventory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["stableRoutineId"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&outer.as_str()),
        "routineInventory lists no Outer"
    );
    assert!(
        ids.iter().all(|id| starts_ci(id, dd::WS_GUID)),
        "routineInventory is primary only: {ids:?}"
    );
}

/// T9b Behaviour (DD cross-app). In memory `Inner` has direct capability
/// facts and coverage. Serialized: no `capabilityFacts`/`coverage`/
/// `typedEdges`/`callsiteResolutions` row with a dependency subject;
/// `CallChain`'s rows present, and every in-memory primary fact.
///
/// Spec deviation (as T9d's): `CallChain` has NO capability fact in memory at
/// this base (its facts come through `Outer -> Inner`, cone-only), so its
/// present rows are its `typedEdges` and `callsiteResolutions` rows; the
/// fact arm is carried by every primary fact.
///
/// Break: identity filter -> `Inner` appears.
#[test]
fn key_class_behaviour() {
    let dir = dd_dir();
    let b = build(dir.path(), false);
    let model = b.target.resolved();
    let inner = stable(model, "Inner");
    let call_chain = stable(model, "CallChain");
    let core = compose_snapshot(model);
    assert!(
        core.capability_facts.iter().any(|f| f.subject == inner),
        "precondition: Inner has capability facts in memory"
    );
    assert!(
        core.coverage.iter().any(|c| c.subject == inner),
        "precondition: Inner has coverage in memory"
    );
    assert!(
        !core
            .capability_facts
            .iter()
            .any(|f| f.subject == call_chain),
        "spec deviation recorded: CallChain has no in-memory fact on the cut; \
         if it now has one, assert CallChain's facts stay"
    );
    let primary_facts = core
        .capability_facts
        .iter()
        .filter(|f| starts_ci(&f.subject, dd::WS_GUID))
        .count();
    assert!(primary_facts > 0, "precondition: primary facts in memory");
    let snap = json(&compose(&b));
    assert_eq!(
        rows(&snap, "capabilityFacts").len(),
        primary_facts,
        "every primary fact stays"
    );
    for key in ["typedEdges", "callsiteResolutions"] {
        assert!(
            rows(&snap, key)
                .iter()
                .any(|r| r["from"] == call_chain.as_str()),
            "{key}: CallChain's rows stay"
        );
    }
    for (key, field) in [
        ("capabilityFacts", "subject"),
        ("coverage", "subject"),
        ("typedEdges", "from"),
        ("callsiteResolutions", "from"),
    ] {
        let dep: Vec<&Value> = rows(&snap, key)
            .iter()
            .filter(|r| starts_ci(r[field].as_str().unwrap_or(""), dd::DEP_GUID))
            .collect();
        assert!(dep.is_empty(), "{key}: no dependency {field}: {dep:?}");
    }
}

/// T9d Evidence (DD + R5B cross-app; S8, O3). DD: `Outer` is referenced by
/// `CallChain`'s typed edge; `TouchCust`'s fact names `bbbb…/table/18`; in
/// memory `Inner`'s operationIndex row exists. `Outer` has NO in-memory
/// callsiteIndex row at this base (recorded deviation 1: `Outer -> Inner` is
/// cone-only), so the "NOT `Outer`'s call-site row" arm is vacuous until
/// step 6, and the test asserts that base truth.
/// Serialized: `identities` holds `Outer`, `bbbb…:Table:18` and every primary
/// id; `operationIndex` holds the primary rows (TouchCust's) and NOT
/// `Inner`'s; `callsiteIndex` holds `CallChain`'s. R5B: the in-memory
/// `identities` hold `eeee…` ids that nothing references, and no `eeee…` id
/// is serialized.
///
/// Break 1: emit every row -> `eeee…` ids and `Inner`'s operation appear.
/// Break 2: primary only -> `Outer` missing. (Break 3 is T9-closure's. Break
/// 4, drop primary Evidence rows, cannot fail on a composed tree, where every
/// primary Evidence row is also referenced; it is pinned on hand-built rows
/// by `snapshot_full`'s `evidence_keeps_unreferenced_primary_rows`.)
#[test]
fn key_class_evidence() {
    let dir = dd_dir();
    let b = build(dir.path(), false);
    let model = b.target.resolved();
    let (outer, inner, call_chain, touch) = (
        stable(model, "Outer"),
        stable(model, "Inner"),
        stable(model, "CallChain"),
        stable(model, "TouchCust"),
    );
    let core = compose_snapshot(model);
    let dep_table = format!("{}/table/18", dd::DEP_GUID);
    assert!(
        core.typed_edges.iter().any(|e| {
            let v = serde_json::to_value(e).unwrap();
            v["from"] == call_chain.as_str() && v["to"] == outer.as_str()
        }),
        "precondition: CallChain's typed edge references Outer"
    );
    assert!(
        core.capability_facts.iter().any(|f| f.subject == touch
            && f.resource_id
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(&dep_table))),
        "precondition: TouchCust's fact names {dep_table}"
    );
    assert!(
        core.operation_index.iter().any(|o| o.routine == inner),
        "precondition: Inner's operationIndex row exists in memory"
    );
    // The spec's precondition "in memory, `Outer`'s callsiteIndex row exists"
    // does NOT hold at this base: `derive_callsite_evidence` keeps only call
    // sites a cut fact or cut typed edge references, and `Outer -> Inner` is a
    // dependency caller's call (cone-only). So the "NOT Outer's" arm below is
    // vacuous until step 6 composes on the cone (ledger, acceptance tests).
    assert!(
        !core.callsite_index.iter().any(|c| c.routine == outer),
        "spec deviation recorded: Outer has no in-memory call-site row on the cut; \
         if it now has one, restore the spec's precondition here"
    );
    assert!(
        core.operation_index.iter().any(|o| o.routine == touch),
        "precondition: TouchCust has an operation row"
    );
    let primary_ids: Vec<&String> = core
        .identities
        .stable_ids
        .iter()
        .filter(|s| starts_ci(s, dd::WS_GUID))
        .collect();

    let snap = json(&compose(&b));
    let ids: BTreeSet<&str> = snap["identities"]["stableIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        ids.contains(outer.as_str()),
        "Outer (referenced) is an identity"
    );
    let stable_table = format!("{}:Table:18", dd::DEP_GUID);
    assert!(
        ids.iter().any(|s| s.eq_ignore_ascii_case(&stable_table)),
        "{stable_table} (referenced) is an identity"
    );
    for p in &primary_ids {
        assert!(ids.contains(p.as_str()), "every primary id is kept: {p}");
    }
    let op_routines: Vec<&str> = rows(&snap, "operationIndex")
        .iter()
        .map(|o| o["routine"].as_str().unwrap())
        .collect();
    assert!(
        op_routines.contains(&touch.as_str()),
        "TouchCust's operation row"
    );
    assert!(
        !op_routines.contains(&inner.as_str()),
        "no Inner operation row"
    );
    let cs_routines: Vec<&str> = rows(&snap, "callsiteIndex")
        .iter()
        .map(|o| o["routine"].as_str().unwrap())
        .collect();
    assert!(
        cs_routines.contains(&call_chain.as_str()),
        "CallChain's call-site rows"
    );
    assert!(
        !cs_routines.contains(&outer.as_str()),
        "no Outer call-site row"
    );

    // R5B: the eeee… rows are unreferenced; none is serialized.
    let r = build(&r5b(), false);
    let core = compose_snapshot(r.target.resolved());
    let referencing = format!(
        "{}{}{}",
        serde_json::to_string(&core.typed_edges).unwrap(),
        serde_json::to_string(&core.callsite_resolutions).unwrap(),
        serde_json::to_string(&core.operation_index).unwrap(),
    )
    .to_ascii_lowercase();
    // Final panel G12: the in-memory identities themselves (not only the
    // model's routines) hold an eeee… id, so "none serialized" can fail.
    assert!(
        core.identities
            .stable_ids
            .iter()
            .any(|s| starts_ci(s, R5B_E)),
        "precondition: the in-memory identities hold an eeee… id"
    );
    assert!(
        !referencing.contains(R5B_E),
        "precondition: no edge, resolution or operation row references eeee…"
    );
    let snap = json(&compose(&r));
    let e_ids: Vec<&str> = snap["identities"]["stableIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .filter(|s| starts_ci(s, R5B_E))
        .collect();
    assert!(e_ids.is_empty(), "no eeee… identity: {e_ids:?}");
}

/// Every scalar leaf of a row (text, integer, float, bool, null) with its
/// row-relative path (array indices dropped); a non-text value rendered.
fn leaves(v: &CborValue, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, String)>) {
    match v {
        CborValue::Text(s) => out.push((path.clone(), s.clone())),
        CborValue::Int(n) => out.push((path.clone(), n.to_string())),
        CborValue::Float(f) => out.push((path.clone(), f.to_string())),
        CborValue::Bool(b) => out.push((path.clone(), b.to_string())),
        CborValue::Null | CborValue::Undefined => out.push((path.clone(), "null".into())),
        CborValue::Array(a) => a.iter().for_each(|x| leaves(x, path, out)),
        CborValue::Map(m) => {
            for (k, x) in m {
                path.push(k.clone());
                leaves(x, path, out);
                path.pop();
            }
        }
    }
}

/// Every string leaf of `tree`, by top-level key. `routineOrderFrames` is
/// walked per frame (its map key is the row's defining field).
fn leaves_by_key(tree: &CborValue) -> Vec<(String, Vec<String>, String)> {
    let CborValue::Map(m) = tree else {
        panic!("a snapshot is a map")
    };
    let mut out = Vec::new();
    for (key, v) in m {
        let mut found = Vec::new();
        match (key.as_str(), v) {
            ("routineOrderFrames", CborValue::Map(frames)) => {
                for f in frames.values() {
                    leaves(f, &mut Vec::new(), &mut found);
                }
            }
            _ => leaves(v, &mut Vec::new(), &mut found),
        }
        out.extend(found.into_iter().map(|(p, s)| (key.clone(), p, s)));
    }
    out
}

/// The id-shaped test of T9-closure: rooted in a model app guid, or equal to
/// a model routine id or to an Evidence row id.
fn id_shaped(model: &Model, evidence_ids: HashSet<String>) -> impl Fn(&str) -> bool + use<> {
    let mut apps: HashSet<String> = model
        .workspace
        .routines
        .iter()
        .map(|r| r.app_guid.to_ascii_lowercase())
        .chain(
            model
                .workspace
                .objects
                .iter()
                .map(|o| o.app_guid.to_ascii_lowercase()),
        )
        .collect();
    apps.insert(primary_guid(model).to_ascii_lowercase());
    let routine_ids: HashSet<String> = model
        .workspace
        .routines
        .iter()
        .map(|r| r.id.clone())
        .collect();
    move |s: &str| {
        let l = s.to_ascii_lowercase();
        apps.iter().any(|a| l.starts_with(a.as_str()))
            || routine_ids.contains(s)
            || evidence_ids.contains(s)
    }
}

fn evidence_ids(snap: &Value) -> HashSet<String> {
    rows(snap, "operationIndex")
        .iter()
        .map(|o| o["operationId"].as_str().unwrap().to_string())
        .chain(
            rows(snap, "callsiteIndex")
                .iter()
                .map(|c| c["callsiteId"].as_str().unwrap().to_string()),
        )
        .collect()
}

/// T9-closure, field classification and Closure (DD + R5B cross-app; S9,
/// SN2, SN4, ON3, ON4; the hand-stated literal, value-source and nested arms
/// run on typed rows in `snapshot_full`'s lib tests, F30). DD's `TouchCust`
/// fact carries `resourceId` `bbbb…/table/18` (asserted). Every id-shaped
/// string sits under a field of `REFERENCES` or `NON_REFERENCE_FIELDS`, and
/// the per-domain Closure of §4.4 holds (no violation), including no frame
/// violation for a root frame's `parentFrameId = -1`.
///
/// Break 2: drop `tableId` from `REFERENCES` -> it is in neither list.
/// Break 3: map internal tables by a `/`->`:` rewrite -> `bbbb…:table:18` is
/// model-rooted and resolves to nothing, a violation. Break 4: treat
/// `stableId` as a reference -> T10's unreferenced-table precondition fails.
#[test]
fn closure_fields_are_classified_and_closed() {
    let dir = dd_dir();
    for (label, b) in [
        ("DD", build(dir.path(), false)),
        ("R5B", build(&r5b(), false)),
    ] {
        let model = b.target.resolved();
        let mut tree = compose(&b);
        if label == "DD" {
            let touch = stable(model, "TouchCust");
            let dep_table = format!("{}/table/18", dd::DEP_GUID);
            assert!(
                compose_snapshot(model)
                    .capability_facts
                    .iter()
                    .any(|f| f.subject == touch
                        && f.resource_id
                            .as_deref()
                            .is_some_and(|r| r.eq_ignore_ascii_case(&dep_table))),
                "precondition: TouchCust's fact carries resourceId {dep_table}"
            );
            // F28: neither DD nor R5B serializes a `table-field` value source,
            // so one is hand-stated on a kept primary fact (TouchCust's).
            let CborValue::Map(m) = &mut tree else {
                unreachable!()
            };
            let Some(CborValue::Array(facts)) = m.get_mut("capabilityFacts") else {
                panic!("capabilityFacts is an array")
            };
            let Some(CborValue::Map(fact)) = facts.iter_mut().find(|f| {
                matches!(f, CborValue::Map(r)
                    if matches!(r.get("subject"), Some(CborValue::Text(s)) if *s == touch))
            }) else {
                panic!("precondition: a kept TouchCust fact")
            };
            let mut source = indexmap::IndexMap::new();
            source.insert("kind".to_string(), CborValue::Text("table-field".into()));
            source.insert("tableId".to_string(), CborValue::Text(dep_table.clone()));
            source.insert("fieldName".to_string(), CborValue::Text("No.".into()));
            fact.insert("resourceArgSource".to_string(), CborValue::Map(source));
            assert!(
                leaves_by_key(&tree)
                    .iter()
                    .any(|(_, path, _)| path.last().is_some_and(|l| l == "tableId")),
                "precondition: a kept fact carries a table-field source"
            );
        }
        if label == "R5B" {
            // #236: R5B's one publisher (`OnHostStarted()`) has no parameters and
            // DD's is a dependency row, so no tree carries a `parameters` leaf;
            // one is hand-stated on a kept contract fact.
            let CborValue::Map(m) = &mut tree else {
                unreachable!()
            };
            let Some(CborValue::Array(facts)) = m.get_mut("contractFacts") else {
                panic!("contractFacts is an array")
            };
            let Some(CborValue::Map(fact)) = facts.first_mut() else {
                panic!("precondition: a kept contract fact")
            };
            fact.insert(
                "parameters".to_string(),
                CborValue::Array(vec![CborValue::Text("x: integer".into())]),
            );
            assert!(
                leaves_by_key(&tree)
                    .iter()
                    .any(|(key, path, _)| key == "contractFacts" && path == &["parameters"]),
                "precondition: a kept contract fact carries a parameters leaf"
            );
        }
        let tree = tree;
        let snap = json(&tree);
        let shaped = id_shaped(model, evidence_ids(&snap));
        let unclassified: BTreeSet<String> = leaves_by_key(&tree)
            .into_iter()
            .filter(|(_, _, s)| shaped(s))
            .filter(|(key, path, _)| !is_classified(key, path))
            .map(|(key, path, _)| format!("{key}.{}", path.join(".")))
            .collect();
        assert!(
            unclassified.is_empty(),
            "{label}: id-shaped values under fields in neither REFERENCES nor NON_REFERENCE_FIELDS: {unclassified:?}"
        );
        // Final panel F6: EVERY field of every row is classified, whatever
        // its value (numbers and booleans included), not only id-shaped ones.
        // A Header or Input key's own scalar (empty path) is not a row field.
        let unlisted: BTreeSet<String> = leaves_by_key(&tree)
            .into_iter()
            .filter(|(_, path, _)| !path.is_empty())
            .filter(|(key, path, _)| !is_classified(key, path))
            .map(|(key, path, _)| format!("{key}.{}", path.join(".")))
            .collect();
        assert!(
            unlisted.is_empty(),
            "{label}: fields in neither REFERENCES nor NON_REFERENCE_FIELDS: {unlisted:?}"
        );
        let universe = SnapshotUniverse::of(&b.target);
        let violations = closure_violations(&tree, model, &universe);
        assert!(violations.is_empty(), "{label}: {violations:?}");
    }
}

/// T9-closure, per-domain resolution (S3N3): one hand-stated dangling value
/// per domain in a kept row of DD's cross-app tree (an unknown `operationId`,
/// `callsiteId`, `edgeId`, an `order.frameId` absent from its table, a gap
/// subject guid that owns no row): each is reported as a violation of its
/// domain.
///
/// Break 8: allow any unresolved value in that domain as external -> that arm
/// passes silently, fails.
#[test]
fn closure_reports_each_dangling_domain() {
    let dir = dd_dir();
    let b = build(dir.path(), false);
    let model = b.target.resolved();
    let universe = SnapshotUniverse::of(&b.target);
    let clean = compose(&b);
    let set = |tree: &mut CborValue, key: &str, edit: &dyn Fn(&mut CborValue)| {
        let CborValue::Map(m) = tree else {
            unreachable!()
        };
        let Some(CborValue::Array(rows)) = m.get_mut(key) else {
            panic!("{key} is an array")
        };
        let row = rows
            .first_mut()
            .unwrap_or_else(|| panic!("precondition: a kept {key} row"));
        edit(row);
    };
    let put = |row: &mut CborValue, field: &str, v: CborValue| {
        let CborValue::Map(m) = row else {
            unreachable!()
        };
        m.insert(field.to_string(), v);
    };
    let txt = |s: &str| CborValue::Text(s.to_string());
    type Arm<'a> = (Domain, &'a str, Box<dyn Fn(&mut CborValue) + 'a>);
    let arms: Vec<Arm> = vec![
        (
            Domain::Operation,
            "mi/nohash/op99",
            Box::new(|t: &mut CborValue| {
                set(t, "capabilityFacts", &|r| {
                    put(r, "witnessOperationId", txt("mi/nohash/op99"))
                })
            }),
        ),
        (
            Domain::Callsite,
            "mi/nohash/cs99",
            Box::new(|t: &mut CborValue| {
                set(t, "callsiteResolutions", &|r| {
                    put(r, "callsiteId", txt("mi/nohash/cs99"))
                })
            }),
        ),
        (
            Domain::Edge,
            "nosuchedge00000",
            Box::new(|t: &mut CborValue| {
                set(t, "callsiteResolutions", &|r| {
                    put(
                        r,
                        "resolvedEdges",
                        CborValue::Array(vec![txt("nosuchedge00000")]),
                    )
                })
            }),
        ),
        (
            Domain::Frame,
            "999",
            Box::new(|t: &mut CborValue| {
                set(t, "operationIndex", &|r| {
                    let CborValue::Map(m) = r else { unreachable!() };
                    let mut order = indexmap::IndexMap::new();
                    order.insert("orderId".to_string(), CborValue::Int(0));
                    order.insert("frameId".to_string(), CborValue::Int(999));
                    order.insert("onSuccessPath".to_string(), CborValue::Bool(true));
                    order.insert("dominatesSuccessReturn".to_string(), CborValue::Bool(false));
                    m.insert("order".to_string(), CborValue::Map(order));
                })
            }),
        ),
        (
            Domain::App,
            "99999999-0000-0000-0000-000000000099",
            Box::new(|t: &mut CborValue| {
                let CborValue::Map(m) = t else { unreachable!() };
                let Some(CborValue::Array(g)) = m.get_mut("analysisGaps") else {
                    panic!("analysisGaps")
                };
                let mut row = indexmap::IndexMap::new();
                row.insert("kind".to_string(), txt("symbol-only-boundary"));
                row.insert(
                    "subject".to_string(),
                    txt("99999999-0000-0000-0000-000000000099"),
                );
                row.insert("detail".to_string(), txt("hand-stated"));
                row.insert("role".to_string(), txt("dependency"));
                g.push(CborValue::Map(row));
            }),
        ),
    ];
    // Identity (final panel F1): only "unknown" or an id rooted in a guid the
    // model does not hold is external; an empty, a malformed and an internal
    // id rooted in no guid must resolve, so each is reported.
    let identity_arms: Vec<Arm> = ["", "garbage", "e09e92b394f6df1a/nosuchroutine"]
        .into_iter()
        .map(|bad| -> Arm {
            (
                Domain::Identity,
                bad,
                Box::new(move |t: &mut CborValue| {
                    set(t, "capabilityFacts", &|r| put(r, "resourceId", txt(bad)))
                }),
            )
        })
        .collect();
    let arms: Vec<Arm> = arms.into_iter().chain(identity_arms).collect();
    let clean_violations = closure_violations(&clean, model, &universe);
    assert!(
        !clean_violations
            .iter()
            .any(|v| v.domain == Domain::Frame && v.value == "-1"),
        "parentFrameId = -1 is no reference: {clean_violations:?}"
    );
    for (domain, value, arm) in &arms {
        let mut tree = clean.clone();
        arm(&mut tree);
        let got = closure_violations(&tree, model, &universe);
        assert!(
            got.iter().any(|v| v.domain == *domain && v.value == *value),
            "{domain:?}: the dangling {value} is reported: {got:?}"
        );
    }
    // The external shape itself still passes: a guid no model app holds.
    let external = "12345678-0000-0000-0000-000000000220:Table:9";
    let mut tree = clean.clone();
    set(&mut tree, "capabilityFacts", &|r| {
        put(r, "resourceId", txt(external))
    });
    let got = closure_violations(&tree, model, &universe);
    assert!(
        !got.iter().any(|v| v.value == external),
        "an id rooted in an unknown guid is external: {got:?}"
    );
}

/// F25 is exactly "unanchored" (review fix 3): a resolution row with no
/// `resolvedEdges` whose call site a kept typed edge still names, and whose
/// `callsiteIndex` row is gone (hand-stated on DD's tree), IS a violation.
///
/// Break: drop the "no kept edge or witness names it" condition from F25 ->
/// the anchored site passes as external, fails.
#[test]
fn closure_anchored_unresolved_site_is_a_violation() {
    let dir = dd_dir();
    let b = build(dir.path(), false);
    let model = b.target.resolved();
    let mut tree = compose(&b);
    let CborValue::Map(m) = &mut tree else {
        unreachable!()
    };
    let text = |row: &CborValue, f: &str| match row {
        CborValue::Map(r) => match r.get(f) {
            Some(CborValue::Text(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };
    let edge_sites: HashSet<String> = match m.get("typedEdges") {
        Some(CborValue::Array(a)) => a.iter().filter_map(|e| text(e, "callsiteId")).collect(),
        _ => HashSet::new(),
    };
    let Some(CborValue::Array(res)) = m.get_mut("callsiteResolutions") else {
        panic!("callsiteResolutions")
    };
    let row = res
        .iter_mut()
        .find(|r| text(r, "callsiteId").is_some_and(|c| edge_sites.contains(&c)))
        .expect("precondition: a resolution whose site a kept typed edge names");
    let site = text(row, "callsiteId").unwrap();
    let CborValue::Map(r) = row else {
        unreachable!()
    };
    r.insert("resolvedEdges".to_string(), CborValue::Array(Vec::new()));
    let Some(CborValue::Array(index)) = m.get_mut("callsiteIndex") else {
        panic!("callsiteIndex")
    };
    index.retain(|c| text(c, "callsiteId").as_deref() != Some(site.as_str()));
    let got = closure_violations(&tree, model, &SnapshotUniverse::of(&b.target));
    assert!(
        got.iter().any(|v| v.domain == Domain::Callsite
            && v.key == "callsiteResolutions"
            && v.value == site),
        "the anchored site {site} is reported: {got:?}"
    );
}

/// T9e Input (DD single vs cross): `workspaceFingerprint` and `inputs` are
/// equal across universes. The cross-app side states `analysisUniverse:
/// "cross-app"` (else the arm would compare two unfiltered trees).
///
/// Break: filter `inputs` as Inventory -> the dependency `.app` rows vanish.
#[test]
fn key_class_input() {
    let dir = dd_dir();
    let single = json(&compose(&build(dir.path(), true)));
    let cross = json(&compose(&build(dir.path(), false)));
    assert_eq!(
        cross["analysisUniverse"], "cross-app",
        "precondition: the cross-app filter ran"
    );
    assert!(
        rows(&single, "inputs")
            .iter()
            .any(|i| i["kind"] == "dep-package"),
        "precondition: a dependency package input row"
    );
    assert_eq!(
        single["workspaceFingerprint"],
        cross["workspaceFingerprint"]
    );
    assert_eq!(single["inputs"], cross["inputs"]);
}

/// T9f Header and order (DD both universes; R5B-F, S8). `analysisUniverse`
/// per universe; DD cross-app `apps` holds `bbbb…` with `role: "dependency"`,
/// publisher `probe`; R5B-F (its primary guid sorts after `dddd…`, asserted)
/// `apps[0]` is `ffff…` with `role: "primary"`, `dddd…` second, no `eeee…`.
///
/// Break: sort `apps[]` by guid only -> `dddd…` first.
#[test]
fn key_class_header_and_apps_order() {
    let dir = dd_dir();
    let single = json(&compose(&build(dir.path(), true)));
    let cross = json(&compose(&build(dir.path(), false)));
    assert_eq!(single["analysisUniverse"], "single-app");
    assert_eq!(cross["analysisUniverse"], "cross-app");
    assert_eq!(single["apps"][0]["role"], "primary");
    let dep_app = rows(&cross, "apps")
        .iter()
        .find(|a| {
            a["appGuid"]
                .as_str()
                .is_some_and(|g| g.eq_ignore_ascii_case(dd::DEP_GUID))
        })
        .unwrap_or_else(|| panic!("the dependency app row: {:?}", cross["apps"]));
    assert_eq!(dep_app["role"], "dependency");
    assert_eq!(dep_app["publisher"], "probe");
    assert_eq!(cross["apps"][0]["role"], "primary");

    // O13 (review F4): an upper-case manifest guid owns the lower-case
    // identities. DD's manifest is lower-case, so the spelling is hand-stated
    // on the dependency app; the row is matched case-insensitively and
    // written as the manifest spells it.
    let mut upper = build(dir.path(), false);
    let upper_guid = dd::DEP_GUID.to_ascii_uppercase();
    base_mut(&mut upper.target)
        .dependency_apps
        .iter_mut()
        .find(|a| a.guid == dd::DEP_GUID)
        .expect("precondition: the lower-case dependency app")
        .guid = upper_guid.clone();
    let snap = json(&compose(&upper));
    assert!(
        snap["identities"]["stableIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap_or("").starts_with(dd::DEP_GUID)),
        "precondition: a lower-case dependency identity is serialized"
    );
    assert!(
        rows(&snap, "apps")
            .iter()
            .any(|a| a["appGuid"] == upper_guid.as_str() && a["role"] == "dependency"),
        "the upper-case dependency app owns its identities: {:?}",
        snap["apps"]
    );

    let f = tempfile::tempdir().unwrap();
    crate::copy_dir(&r5b(), f.path());
    let app_json = std::fs::read_to_string(f.path().join("app.json")).unwrap();
    let ffff = "ffffffff-0000-0000-0000-0000000000aa";
    assert_eq!(app_json.matches(R5B_PRIMARY).count(), 1, "precondition");
    std::fs::write(
        f.path().join("app.json"),
        app_json.replace(R5B_PRIMARY, ffff),
    )
    .unwrap();
    assert!(ffff > R5B_D, "precondition: the primary sorts after dddd…");
    let snap = json(&compose(&build(f.path(), false)));
    let apps = rows(&snap, "apps");
    assert_eq!(apps[0]["appGuid"], ffff, "{apps:?}");
    assert_eq!(apps[0]["role"], "primary");
    assert!(
        apps.get(1)
            .and_then(|a| a["appGuid"].as_str())
            .is_some_and(|g| g.eq_ignore_ascii_case(R5B_D)),
        "dddd… second: {apps:?}"
    );
    assert!(
        apps.iter().all(|a| !a["appGuid"]
            .as_str()
            .unwrap_or("")
            .eq_ignore_ascii_case(R5B_E)),
        "no eeee… row: {apps:?}"
    );
}

/// T9-keys (both universes, DD; O4): the composed keys are a subset of
/// `SNAPSHOT_KEYS` and every required key is present. (The shard half is
/// `snapshot_full`'s `slice_for_app_handles_every_classed_key`.)
///
/// Break: insert a new key without a class -> fails.
#[test]
fn composed_keys_are_classed() {
    let dir = dd_dir();
    let classed: Vec<&str> = SNAPSHOT_KEYS.iter().map(|(k, _)| *k).collect();
    let optional = ["inputsMetadata", "routineOrderFrames"];
    for single in [true, false] {
        let tree = compose(&build(dir.path(), single));
        let CborValue::Map(m) = &tree else {
            unreachable!()
        };
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        for k in &keys {
            assert!(
                classed.contains(k),
                "single_app={single}: {k} has no class ({classed:?})"
            );
        }
        for k in &classed {
            assert!(
                optional.contains(k) || keys.contains(k),
                "single_app={single}: required key {k} missing"
            );
        }
    }
}

/// T10 (S7, O5, S10, SN2): a temp copy of `ws-d8-commit-in-tx` plus
/// `src/Unused.al` (table 50199 no code touches). The cross-app base builds
/// with no dependency; one primary routine hand-stated `parse_incomplete` in
/// both models, so `analysisGaps` is non-empty; table `Unused` is an
/// `identities` row no other field names (asserted). The full trees are equal
/// except `analysisUniverse`.
///
/// Break 1: write the gap `role` only cross-app -> the single-app gap row
/// lacks it. Break 2: Evidence keeps referenced ids only -> the unreferenced
/// table vanishes cross-app.
#[test]
fn no_dependency_cross_equals_single() {
    let dir = tempfile::tempdir().unwrap();
    crate::copy_dir(
        &repo().join("tests/r0-corpus/ws-d8-commit-in-tx"),
        dir.path(),
    );
    std::fs::write(
        dir.path().join("src/Unused.al"),
        "table 50199 Unused\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
    )
    .unwrap();
    let mut single = build(dir.path(), true);
    let mut cross = build(dir.path(), false);
    assert!(
        base(&cross.target).dep_routine_ids.is_empty(),
        "precondition: no dependency routine"
    );
    let victim = single.target.resolved().workspace.routines[0].name.clone();
    hand_state_parse_incomplete(&mut single.target, &[&victim]);
    hand_state_parse_incomplete(&mut cross.target, &[&victim]);
    let s_tree = compose(&single);
    let s = json(&s_tree);
    let c = json(&compose(&cross));
    assert!(
        !rows(&s, "analysisGaps").is_empty(),
        "precondition: a non-empty analysisGaps"
    );
    let guid = primary_guid(single.target.resolved());
    let unused_stable = format!("{guid}:Table:50199");
    let unused_internal = format!("{guid}/table/50199");
    let ids = s["identities"]["stableIds"].as_array().unwrap();
    assert!(
        ids.iter().any(|v| v == unused_stable.as_str()),
        "precondition: Unused is an identities row"
    );
    // No REFERENCES field (a defining field is no reference, SN2) of any row
    // names Unused; in the committed fixture every table is a permission
    // target, so none would do.
    let naming: Vec<String> = leaves_by_key(&s_tree)
        .into_iter()
        .filter(|(_, _, v)| v.starts_with(&unused_stable) || v.starts_with(&unused_internal))
        .filter(|(key, path, _)| is_reference(key, path))
        .map(|(key, path, v)| format!("{key}.{} = {v}", path.join(".")))
        .collect();
    assert!(
        naming.is_empty(),
        "precondition: no reference field names Unused: {naming:?}"
    );

    assert_eq!(s["analysisUniverse"], "single-app");
    assert_eq!(c["analysisUniverse"], "cross-app");
    assert!(
        rows(&s, "analysisGaps")
            .iter()
            .all(|g| g["role"] == "primary"),
        "the single-app gap rows carry role primary: {:?}",
        s["analysisGaps"]
    );
    let strip = |mut v: Value| {
        v.as_object_mut().unwrap().remove("analysisUniverse");
        v
    };
    assert_eq!(
        strip(s),
        strip(c),
        "the trees differ only in analysisUniverse"
    );
}

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

/// The panic message of composing `b`'s model as `SingleApp`.
fn single_app_refusal(b: &Built) -> String {
    let wrong = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_with(b, SnapshotUniverse::SingleApp)
    }));
    let err = wrong.expect_err("a cross-app model composed as SingleApp must panic");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Final panel G5: `SingleApp` binds a single-app model. DD's cross-app model
/// (it holds dependency routines, asserted) composed as `SingleApp` panics;
/// so does it with every dependency routine removed (round 2: dependency
/// objects and tables alone, asserted, are serialized as identities too);
/// DD's single-app model composes.
///
/// Break: check routines only -> the second arm composes, writing dependency
/// identities under `"single-app"`.
#[test]
fn single_app_universe_binds_a_single_app_model() {
    const REFUSAL: &str =
        "compose_full_snapshot: SingleApp with a model that holds dependency rows";
    let dir = dd_dir();
    let mut cross = build(dir.path(), false);
    let dep = |g: &str| g.eq_ignore_ascii_case(dd::DEP_GUID);
    assert!(
        cross
            .target
            .resolved()
            .workspace
            .routines
            .iter()
            .any(|r| dep(&r.app_guid)),
        "precondition: the cross-app model holds dependency routines"
    );
    let _ = compose(&build(dir.path(), true));
    assert_eq!(single_app_refusal(&cross), REFUSAL);

    let ws = &mut model_mut(&mut cross.target).workspace;
    ws.routines.retain(|r| !dep(&r.app_guid));
    assert!(
        ws.objects.iter().any(|o| dep(&o.app_guid)) || ws.tables.iter().any(|t| dep(&t.app_guid)),
        "precondition: dependency objects or tables remain with no dependency routine"
    );
    assert_eq!(
        single_app_refusal(&cross),
        REFUSAL,
        "objects and tables count"
    );
}
