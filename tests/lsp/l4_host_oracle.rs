//! #222 (spec §5.1, §5.2): the incremental L4 host against a fresh one.
//!
//! Each case writes its r0 sources into a tempdir (one codeunit per `.al`
//! file, plus an unrelated `U`), refreshes a host, applies one edit, refreshes
//! it again, then builds a SECOND host on the same directory and refreshes it
//! once. It asserts the oracle (`host.projection() == fresh.projection()`),
//! the refresh kind and the hand-stated re-solved set (minimality), in both
//! universes: a cross-app host on a workspace with no dependency is the
//! single-app answer.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use al_sem::engine::batch::job::ViewUniverse;
use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::engine::incremental::{
    FullReason, IncrementalHost, L4Projection, RefreshError, RefreshKind, RefreshReport, Refresher,
};
use al_sem::engine::session::{View, query_plan};
use al_sem::program::inputs::WorkspaceInputs;

const UNIVERSES: [ViewUniverse; 2] = [ViewUniverse::CrossApp, ViewUniverse::SingleApp];

const APP_JSON: &str = r#"{"id":"22222222-0000-0000-0000-000000000223","name":"Host222","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#;

const TABLES_AL: &str = "table 50100 Cust\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n\ntable 50101 Item\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";

const U_AL: &str = "codeunit 50109 U\n{\n    procedure U()\n    var\n        I: Record Item;\n    begin\n        I.Insert();\n    end;\n}\n";

/// T1a/T1b's r0: `A→B→L`; `L` inserts `Cust`; `U` inserts `Item`.
const T1A: &[(&str, &str)] = &[
    ("src/Tables.al", TABLES_AL),
    ("src/U.al", U_AL),
    (
        "src/A.al",
        "codeunit 50100 A\n{\n    procedure A()\n    var\n        CB: Codeunit B;\n    begin\n        CB.B();\n    end;\n}\n",
    ),
    (
        "src/B.al",
        "codeunit 50101 B\n{\n    procedure B()\n    var\n        CL: Codeunit L;\n    begin\n        CL.L();\n    end;\n}\n",
    ),
    (
        "src/L.al",
        "codeunit 50102 L\n{\n    procedure L()\n    var\n        C: Record Cust;\n        i: Integer;\n    begin\n        C.Insert();\n    end;\n}\n",
    ),
];

fn never() -> bool {
    false
}

fn write_ws(dir: &Path, files: &[(&str, &str)]) {
    std::fs::write(dir.join("app.json"), APP_JSON).unwrap();
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// Replace `old` with `new` in `dir/rel`, after asserting `old` occurs once.
fn replace_once(dir: &Path, rel: &str, old: &str, new: &str) {
    let path = dir.join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text.matches(old).count(),
        1,
        "{rel}: {old:?} must occur once"
    );
    std::fs::write(path, text.replacen(old, new, 1)).unwrap();
}

fn refresh(host: &mut IncrementalHost) -> RefreshReport {
    host.refresh_committed(&never).expect("a good refresh")
}

fn labels(want: &[&[&str]]) -> BTreeSet<Vec<String>> {
    want.iter()
        .map(|scc| {
            let mut l: Vec<String> = scc.iter().map(|s| s.to_string()).collect();
            l.sort();
            l
        })
        .collect()
}

/// The fresh side: a second host with no baseline, refreshed once.
fn fresh_projection(dir: &Path, universe: ViewUniverse) -> L4Projection {
    let mut fresh = IncrementalHost::new(dir, universe);
    let r = refresh(&mut fresh);
    assert_eq!(r.kind, RefreshKind::Full(FullReason::NoBaseline));
    fresh
        .projection()
        .expect("a committed refresh has a projection")
}

/// §5.1's harness, in both universes. Returns nothing: every assertion is
/// here.
fn case(
    name: &str,
    r0: &[(&str, &str)],
    edit: impl Fn(&Path),
    want_kind: RefreshKind,
    want_resolved: &[&[&str]],
) {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_ws(dir, r0);
        let mut host = IncrementalHost::new(dir, universe);
        assert_eq!(
            refresh(&mut host).kind,
            RefreshKind::Full(FullReason::NoBaseline),
            "{name} {universe:?}: r0"
        );
        edit(dir);
        let report = refresh(&mut host);
        assert_eq!(
            host.projection().expect("committed"),
            fresh_projection(dir, universe),
            "{name} {universe:?}: oracle"
        );
        assert_eq!(report.kind, want_kind, "{name} {universe:?}: kind");
        assert_eq!(
            report.resolved,
            labels(want_resolved),
            "{name} {universe:?}: re-solved SCCs"
        );
        if report.kind == RefreshKind::Incremental {
            assert!(
                report.resolved.len() < report.total,
                "{name} {universe:?}: an incremental refresh re-solves a strict subset"
            );
        }
    }
}

/// T0b: on each oracle fixture, in both universes, the fresh host's bundle
/// part (db effects per id, the terminal partition), diagnostics, warnings
/// and SCC equal what `View::new` + `prepare(&query_plan())` leaves in
/// `view.l4()` (whose core summaries `prepare` drops). Break: the host
/// builds its target in the other universe. On these no-dependency fixtures
/// that break stays green (both universes give the single-app answer); it
/// fails on `dep_dispatch_fixture::write_own` (checked in Task 3 with a
/// reverted trial; the fixture lands with Task 5).
#[test]
fn t0b_fresh_host_equals_query_plan() {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        write_ws(tmp.path(), T1A);
        let host = fresh_projection(tmp.path(), universe).without_core();
        let inputs = WorkspaceInputs::read(tmp.path(), &Default::default());
        let target =
            AnalysisTarget::build(&inputs, universe == ViewUniverse::SingleApp, "query").unwrap();
        let mut view = View::new(target);
        view.prepare(&query_plan());
        assert!(
            view.l4().bundle.routines_with_rows().next().is_some(),
            "precondition: T1a has bundle rows"
        );
        let want = L4Projection::of(view.l4(), &HashMap::new()).without_core();
        assert_eq!(host, want, "{universe:?}: fresh host vs the query plan");
    }
}

/// T1a: `L` also modifies `Cust`. `L` is in Δ, `B` is dirty by the one-hop
/// rule, and `A` only by R3 (`B` came out different: it inherits the
/// modify). Break B1 (`came_out_different` always false) seeds `A`.
#[test]
fn t1a_body() {
    case(
        "t1a",
        T1A,
        |d| replace_once(d, "src/L.al", "C.Insert();", "C.Insert(); C.Modify();"),
        RefreshKind::Incremental,
        &[&["L.L()"], &["B.B()"], &["A.A()"]],
    );
}

/// T1b: `L` gains `i := 1;`, which changes its row but not its L4 result.
/// `B` is re-solved by the one-hop rule and comes out equal, so `A` and `U`
/// stay seeded. Break B2 (always different) re-solves `A` too.
#[test]
fn t1b_body_no_l4_change() {
    case(
        "t1b",
        T1A,
        |d| replace_once(d, "src/L.al", "C.Insert();", "C.Insert(); i := 1;"),
        RefreshKind::Incremental,
        &[&["L.L()"], &["B.B()"]],
    );
}

/// Review Focus 1: a build that fails after a good baseline (an `app.json`
/// with no `id`) returns `Build`, keeps the projection, and the next good
/// refresh is Incremental with nothing re-solved. Break: clear `last` on an
/// error.
#[test]
fn build_error_keeps_baseline() {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_ws(dir, T1A);
        let mut host = IncrementalHost::new(dir, universe);
        refresh(&mut host);
        let before = host.projection().expect("committed");
        let id = r#""id":"22222222-0000-0000-0000-000000000223","#;
        replace_once(dir, "app.json", id, "");
        let err = host.refresh_committed(&never);
        assert!(
            matches!(err, Err(RefreshError::Build(_))),
            "{universe:?}: no app id is a build error: {err:?}"
        );
        assert_eq!(
            host.projection(),
            Some(before),
            "{universe:?}: the baseline is kept"
        );
        std::fs::write(dir.join("app.json"), APP_JSON).unwrap();
        let report = refresh(&mut host);
        assert_eq!(report.kind, RefreshKind::Incremental, "{universe:?}");
        assert!(report.resolved.is_empty(), "{universe:?}: {report:?}");
    }
}

/// Review Focus 3: a workspace with no routine is a `Build` error, not a
/// panic. Break: `expect` the target build.
#[test]
fn empty_workspace_is_build_error() {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        write_ws(tmp.path(), &[]);
        let mut host = IncrementalHost::new(tmp.path(), universe);
        let err = host.refresh(&never);
        assert!(
            matches!(err, Err(RefreshError::Build(_))),
            "{universe:?}: {err:?}"
        );
        assert!(host.projection().is_none(), "{universe:?}");
    }
}
