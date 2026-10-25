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
use al_sem::engine::l4::effect_lattice::EffectPresence;
use al_sem::engine::l4::summary::{DbEffect, RoutineSummary, TempState};
use al_sem::engine::session::{View, query_plan};
use al_sem::program::inputs::WorkspaceInputs;
use al_sem::program::model::workspace::ModelRoutine;

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

/// One fresh revision: its projection, and a `query` plan view for the model
/// rows and the relation the preconditions read.
struct Fresh {
    proj: L4Projection,
    view: View,
}

impl Fresh {
    fn of(dir: &Path, universe: ViewUniverse) -> Fresh {
        let proj = fresh_projection(dir, universe);
        let inputs = WorkspaceInputs::read(dir, &Default::default());
        let target =
            AnalysisTarget::build(&inputs, universe == ViewUniverse::SingleApp, "query").unwrap();
        let mut view = View::new(target);
        view.prepare(&query_plan());
        Fresh { proj, view }
    }

    /// The model rows named `name` with `params` parameters, in model order.
    fn rows(&self, name: &str, params: usize) -> Vec<&ModelRoutine> {
        self.view
            .model()
            .workspace
            .routines
            .iter()
            .filter(|r| r.name == name && r.parameters.len() == params)
            .collect()
    }

    /// The one id of the rows named `name` with `params` parameters.
    fn id(&self, name: &str, params: usize) -> String {
        let rows = self.rows(name, params);
        assert!(!rows.is_empty(), "precondition: a routine {name}/{params}");
        assert!(
            rows.iter().all(|r| r.id == rows[0].id),
            "precondition: one id for {name}/{params}"
        );
        rows[0].id.clone()
    }

    /// The core summary and db effects of `name`/`params`.
    fn routine(
        &self,
        name: &str,
        params: usize,
    ) -> &(Option<RoutineSummary>, Option<Vec<DbEffect>>) {
        self.proj.routine(&self.id(name, params))
    }

    /// `Debug` text of `name`/`params`'s out-edges in the primary graph.
    fn edges(&self, name: &str, params: usize) -> String {
        let id = self.id(name, params);
        format!("{:?}", self.view.rel().primary().edges_by_from.get(&id))
    }
}

/// A case's preconditions, asserted on the fresh r0 and r1 before the oracle.
type Pre<'a> = Option<&'a dyn Fn(&Fresh, &Fresh)>;

/// §5.1's harness, in both universes. Returns nothing: every assertion is
/// here.
fn case(
    name: &str,
    r0: &[(&str, &str)],
    edit: impl Fn(&Path),
    want_kind: RefreshKind,
    want_resolved: &[&[&str]],
) {
    case_pre(name, r0, edit, want_kind, want_resolved, None)
}

/// [`case`] with preconditions, asserted before any other assertion on r1.
fn case_pre(
    name: &str,
    r0: &[(&str, &str)],
    edit: impl Fn(&Path),
    want_kind: RefreshKind,
    want_resolved: &[&[&str]],
    pre: Pre<'_>,
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
        let before = pre.map(|_| Fresh::of(dir, universe));
        edit(dir);
        let report = refresh(&mut host);
        if let (Some(pre), Some(before)) = (pre, &before) {
            pre(before, &Fresh::of(dir, universe));
        }
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

/// `codeunit <no> <name> { <body> }`.
fn cu(no: u32, name: &str, body: &str) -> String {
    format!("codeunit {no} {name}\n{{\n{body}}}\n")
}

/// A fixture: the tables, `U`, and `files`.
fn fixture(files: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("src/Tables.al", TABLES_AL.to_string()),
        ("src/U.al", U_AL.to_string()),
    ];
    v.extend(files.iter().cloned());
    v
}

fn as_refs<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(p, t)| (*p, t.as_str())).collect()
}

/// `A()` calls `B()`.
fn a_calls_b() -> (&'static str, String) {
    (
        "src/A.al",
        cu(
            50100,
            "A",
            "    procedure A()\n    var\n        CB: Codeunit B;\n    begin\n        CB.B();\n    end;\n",
        ),
    )
}

/// T1c: `L(x)` → `L(x; y: Integer)` and `B`'s call with it. `B` is dirty
/// three ways (own row, a new successor id, a new SCC). REDUNDANCY (spec
/// §5.2): the combined break (R2 without its two Δ conditions, the key match
/// kept, and R3 treating a member with no baseline entry as equal) seeds `B`
/// with the old `L` id in its inherited effects.
#[test]
fn t1c_signature() {
    let ws = fixture(&[
        a_calls_b(),
        (
            "src/B.al",
            cu(
                50101,
                "B",
                "    procedure B()\n    var\n        CL: Codeunit L;\n    begin\n        CL.L(1);\n    end;\n",
            ),
        ),
        (
            "src/L.al",
            cu(
                50102,
                "L",
                "    procedure L(x: Integer)\n    var\n        C: Record Cust;\n    begin\n        C.Insert();\n    end;\n",
            ),
        ),
    ]);
    case(
        "t1c",
        &as_refs(&ws),
        |d| {
            replace_once(d, "src/L.al", "L(x: Integer)", "L(x: Integer; y: Integer)");
            replace_once(d, "src/B.al", "CL.L(1);", "CL.L(1, 2);");
        },
        RefreshKind::Incremental,
        &[&["L.L(Integer; Integer)"], &["B.B()"], &["A.A()"]],
    );
}

/// T1d: `A` stops calling `L`. Break B3 (R2 without "no member in Δ") seeds
/// `A` with the `Cust` insert. REDUNDANCY: leaving `edges_by_from` out of the
/// row stays green, since `A`'s model row (`call_sites`) changes too.
#[test]
fn t1d_edge_removal() {
    let ws = fixture(&[
        (
            "src/A.al",
            cu(
                50100,
                "A",
                "    procedure A()\n    var\n        CL: Codeunit L;\n        CM: Codeunit M;\n    begin\n        CL.L();\n        CM.M();\n    end;\n",
            ),
        ),
        (
            "src/L.al",
            cu(
                50102,
                "L",
                "    procedure L()\n    var\n        C: Record Cust;\n    begin\n        C.Insert();\n    end;\n",
            ),
        ),
        (
            "src/M.al",
            cu(
                50103,
                "M",
                "    procedure M()\n    var\n        I: Record Item;\n    begin\n        I.Insert();\n    end;\n",
            ),
        ),
    ]);
    case(
        "t1d",
        &as_refs(&ws),
        |d| replace_once(d, "src/A.al", "        CL.L();\n", ""),
        RefreshKind::Incremental,
        &[&["A.A()"]],
    );
}

/// T1e: `{A,B,C}` splits into `{B,C}` and `{A}` when `C→A` goes. `A` is not
/// in Δ and its one successor `B` comes out equal, so only the key rule
/// re-solves it. Break B5 (no key condition) seeds `A` with
/// `in_recursive_cycle = true`.
#[test]
fn t1e_scc_split() {
    let ws = fixture(&[
        a_calls_b(),
        (
            "src/B.al",
            cu(
                50101,
                "B",
                "    procedure B()\n    var\n        CC: Codeunit C;\n        R: Record Cust;\n    begin\n        R.Insert();\n        CC.C();\n    end;\n",
            ),
        ),
        (
            "src/C.al",
            cu(
                50103,
                "C",
                "    procedure C()\n    var\n        CB: Codeunit B;\n        CA: Codeunit A;\n    begin\n        CB.B();\n        CA.A();\n    end;\n",
            ),
        ),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        for n in ["B", "C"] {
            assert_eq!(
                r0.routine(n, 0),
                r1.routine(n, 0),
                "precondition: {n} projects equal at r0 and r1"
            );
        }
        let cyc = |f: &Fresh| f.routine("A", 0).0.as_ref().unwrap().in_recursive_cycle;
        assert!(cyc(r0) && !cyc(r1), "precondition: A recursive at r0 only");
    };
    case_pre(
        "t1e",
        &as_refs(&ws),
        |d| replace_once(d, "src/C.al", "        CA.A();\n", ""),
        RefreshKind::Incremental,
        &[&["B.B()", "C.C()"], &["A.A()"]],
        Some(&pre),
    );
}

/// T1f: `B→A` merges `A` and `B` into one SCC. REDUNDANCY (spec §5.2): `B`
/// is in Δ, so the key rule never decides a merge; the combined break (R2
/// always true, R3 kept) seeds `{A,B}` with `in_recursive_cycle = false`.
#[test]
fn t1f_scc_merge() {
    let ws = fixture(&[
        a_calls_b(),
        (
            "src/B.al",
            cu(
                50101,
                "B",
                "    procedure B()\n    var\n        R: Record Cust;\n        CA: Codeunit A;\n    begin\n        R.Insert();\n    end;\n",
            ),
        ),
    ]);
    case(
        "t1f",
        &as_refs(&ws),
        |d| replace_once(d, "src/B.al", "R.Insert();", "R.Insert(); CA.A();"),
        RefreshKind::Incremental,
        &[&["A.A()", "B.B()"]],
    );
}

/// The publisher `P` of T1g: an integration event with one `var Rec`.
const P_AL: &str = "codeunit 50103 P\n{\n    [IntegrationEvent(false, false)]\n    procedure P(var Rec: Record Cust)\n    begin\n    end;\n}\n";

/// T1g (D-1): `S` renames the parameter `P`'s event binds by name. `S`'s own
/// result does not move; `P`'s does, through the dispatch binding read off
/// `S`'s model row. Break B6 (R2 without the one-hop rule) seeds `P`.
/// Reshaped from the spec's `Rec.Modify()`: a db effect carries the
/// parameter's name in its `record_variable_id`, so `S` would project
/// differently. `Rec.Reset()` makes no db effect and its role has no name.
#[test]
fn t1g_event_param_rename() {
    let ws = fixture(&[
        ("src/P.al", P_AL.to_string()),
        (
            "src/S.al",
            cu(
                50104,
                "S",
                "    [EventSubscriber(ObjectType::Codeunit, Codeunit::P, 'P', '', false, false)]\n    local procedure S(var Rec: Record Cust)\n    begin\n        Rec.Reset();\n    end;\n",
            ),
        ),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        assert_eq!(
            r0.routine("S", 1),
            r1.routine("S", 1),
            "precondition: S projects equal at r0 and r1"
        );
        assert_ne!(
            r0.routine("P", 1),
            r1.routine("P", 1),
            "precondition: P projects differently at r0 and r1"
        );
    };
    case_pre(
        "t1g",
        &as_refs(&ws),
        |d| {
            replace_once(
                d,
                "src/S.al",
                "S(var Rec: Record Cust)",
                "S(var RecX: Record Cust)",
            );
            replace_once(d, "src/S.al", "Rec.Reset();", "RecX.Reset();");
        },
        RefreshKind::Incremental,
        &[&["S.S(var Record Cust)"], &["P.P(var Record Cust)"]],
        Some(&pre),
    );
}

/// The T1a fixture's SCCs plus `U`, for the Full cases on it.
const T1A_ALL: &[&[&str]] = &[&["A.A()"], &["B.B()"], &["L.L()"], &["U.U()"]];

/// T1h: a new `.al` file renames every id (the model-instance id hashes the
/// file list). Break B7 (no instance id in R0) makes it Incremental; the
/// oracle holds either way (no key matches).
#[test]
fn t1h_file_add() {
    case(
        "t1h",
        T1A,
        |d| {
            std::fs::write(
                d.join("src/N.al"),
                cu(50110, "N", "    procedure N()\n    begin\n    end;\n"),
            )
            .unwrap()
        },
        RefreshKind::Full(FullReason::HostGlobal),
        &[&["A.A()"], &["B.B()"], &["L.L()"], &["N.N()"], &["U.U()"]],
    );
}

/// T1k: `A` reads `T."F"` before `F` exists. Adding the field moves
/// `FieldIndex` (R0 solve half) and no row. Break B10 (no `FieldIndex` in
/// R0) seeds `A` with the unresolved field.
#[test]
fn t1k_table_field() {
    let ws = fixture(&[
        (
            "src/T.al",
            "table 50102 T\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n".to_string(),
        ),
        (
            "src/A.al",
            cu(
                50100,
                "A",
                "    procedure A(var R: Record T)\n    var\n        x: Integer;\n    begin\n        x := R.\"F\";\n    end;\n",
            ),
        ),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        assert_ne!(
            r0.routine("A", 1).0,
            r1.routine("A", 1).0,
            "precondition: A's core summary differs between r0 and r1"
        );
    };
    case_pre(
        "t1k",
        &as_refs(&ws),
        |d| {
            replace_once(
                d,
                "src/T.al",
                "field(1; \"No.\"; Code[20])\n        {\n        }\n",
                "field(1; \"No.\"; Code[20])\n        {\n        }\n        field(2; \"F\"; Integer)\n        {\n        }\n",
            )
        },
        RefreshKind::Full(FullReason::SolveGlobal),
        &[&["A.A(var Record T)"], &["U.U()"]],
        Some(&pre),
    );
}

/// T1l: a new `roots.config.json` naming `A` changes no row and no global
/// input: nothing is re-solved. Break B11 (the config bytes in R0) makes it
/// Full.
#[test]
fn t1l_roots_config() {
    case(
        "t1l",
        T1A,
        |d| {
            std::fs::write(
                d.join("roots.config.json"),
                r#"{"version":1,"roots":[{"id":"a-root","target":{"objectId":"22222222-0000-0000-0000-000000000223/Codeunit/50100","routineName":"A"},"kinds":["job-queue-entrypoint"],"note":"T1l"}]}"#,
            )
            .unwrap()
        },
        RefreshKind::Incremental,
        &[],
    );
}

/// T1m's `L`: hands its `var Rec` to an insert (a parameter-dependent
/// effect).
fn t1m_fixture() -> Vec<(&'static str, String)> {
    fixture(&[(
        "src/L.al",
        cu(
            50102,
            "L",
            "    procedure L(var Rec: Record Cust)\n    begin\n        Rec.Insert();\n    end;\n",
        ),
    )])
}

/// T1m: no edit re-solves nothing, and the seeded rows keep `L`'s
/// parameter-dependent effect. Review Focus 4: two consecutive no-edit
/// refreshes keep the projection byte-equal. Break B12 (the shared seeding
/// helper pushes only terminal effects) drops it.
#[test]
fn t1m_no_edit() {
    let ws = t1m_fixture();
    let pre = |r0: &Fresh, _: &Fresh| {
        let effects = r0.routine("L", 1).1.as_ref().expect("L has a row");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e.temp_state, TempState::ParameterDependent(_))),
            "precondition: L has a parameter-dependent effect: {effects:?}"
        );
    };
    case_pre(
        "t1m",
        &as_refs(&ws),
        |_| {},
        RefreshKind::Incremental,
        &[],
        Some(&pre),
    );
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        write_ws(tmp.path(), &as_refs(&ws));
        let mut host = IncrementalHost::new(tmp.path(), universe);
        refresh(&mut host);
        let p0 = host.projection();
        for round in 1..=2 {
            let report = refresh(&mut host);
            assert_eq!(
                report.kind,
                RefreshKind::Incremental,
                "{universe:?} {round}"
            );
            assert!(
                report.resolved.is_empty(),
                "{universe:?} {round}: {report:?}"
            );
            assert_eq!(
                host.projection(),
                p0,
                "{universe:?} {round}: no-edit projection"
            );
        }
    }
}

/// T1n's subscriber `S`: binds `Rec` of `P.E` and modifies it.
const T1N_S: &str = "    [EventSubscriber(ObjectType::Codeunit, Codeunit::P, 'E', '', false, false)]\n    local procedure S(var Rec: Record Cust)\n    begin\n        Rec.Modify();\n    end;\n";

/// T1n (P6): an overload `E(var Rec; X)` after `E(var Rec)` makes the event
/// shared. `E(var Rec)`'s rows, out-edges and successor `S` do not change;
/// only its shared-event bit does. Break B13 (no shared bit in the row) seeds
/// it with its old dispatch.
#[test]
fn t1n_shared_event_bit() {
    let ws = fixture(&[
        (
            "src/P.al",
            cu(
                50103,
                "P",
                "    [IntegrationEvent(false, false)]\n    procedure E(var Rec: Record Cust)\n    begin\n    end;\n",
            ),
        ),
        ("src/S.al", cu(50104, "S", T1N_S)),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        assert_ne!(
            r0.routine("E", 1).0,
            r1.routine("E", 1).0,
            "precondition: E(var Rec)'s core summary differs"
        );
        assert_eq!(
            format!("{:?}", r0.rows("E", 1)),
            format!("{:?}", r1.rows("E", 1)),
            "precondition: E(var Rec)'s model row is equal"
        );
        assert_eq!(
            r0.edges("E", 1),
            r1.edges("E", 1),
            "precondition: E(var Rec)'s out-edges are equal"
        );
        assert_eq!(
            format!("{:?}", r0.rows("S", 1)),
            format!("{:?}", r1.rows("S", 1)),
            "precondition: S's model row is equal"
        );
    };
    case_pre(
        "t1n",
        &as_refs(&ws),
        |d| {
            replace_once(
                d,
                "src/P.al",
                "    end;\n}",
                "    end;\n\n    [IntegrationEvent(false, false)]\n    procedure E(var Rec: Record Cust; X: Integer)\n    begin\n    end;\n}",
            )
        },
        RefreshKind::Incremental,
        &[
            &["P.E(var Record Cust)"],
            &["P.E(var Record Cust; Integer)"],
        ],
        Some(&pre),
    );
}

/// T1o (step 11b): `K` is declared in an `#if A` / `#else` pair, two rows
/// with one id; the first row's insert becomes a modify on the same line.
/// Break B14 (digest only the last row of an id) leaves `K` out of Δ, so
/// `K` and `A` are seeded with the old union.
#[test]
fn t1o_collided_rows() {
    let ws = fixture(&[
        (
            "src/A.al",
            cu(
                50100,
                "A",
                "    procedure A()\n    var\n        CK: Codeunit K;\n    begin\n        CK.K();\n    end;\n",
            ),
        ),
        (
            "src/K.al",
            cu(
                50105,
                "K",
                "#if A\n    procedure K()\n    var\n        C: Record Cust;\n    begin\n        C.Insert();\n    end;\n#else\n    procedure K()\n    var\n        I: Record Item;\n    begin\n        I.Insert();\n    end;\n#endif\n",
            ),
        ),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        for f in [r0, r1] {
            assert_eq!(f.rows("K", 0).len(), 2, "precondition: two rows of K");
            f.id("K", 0);
        }
        assert_eq!(
            format!("{:?}", r0.rows("K", 0).last()),
            format!("{:?}", r1.rows("K", 0).last()),
            "precondition: K's last row is equal"
        );
        assert_ne!(
            r0.proj, r1.proj,
            "precondition: the fresh projections differ"
        );
    };
    case_pre(
        "t1o",
        &as_refs(&ws),
        |d| replace_once(d, "src/K.al", "C.Insert();", "C.Modify();"),
        RefreshKind::Incremental,
        &[&["K.K()"], &["A.A()"]],
        Some(&pre),
    );
}

/// T1q: a `description` in `app.json` changes no row, but `app.json` bytes
/// are in R0's host half. Break B15 (no `app.json` bytes in R0) makes it
/// Incremental; the oracle holds either way.
#[test]
fn t1q_app_json_only() {
    case(
        "t1q",
        T1A,
        |d| {
            replace_once(
                d,
                "app.json",
                r#""name":"Host222","#,
                r#""name":"Host222","description":"d","#,
            )
        },
        RefreshKind::Full(FullReason::HostGlobal),
        T1A_ALL,
    );
}

/// T1r (O21): `L` gains `R.Reset();`, which moves only
/// `resets_filters_on_param` (no db effect) up `A→B→L`'s `var` chain. Break
/// B16 (R3 compares db effects only) seeds `A` with its stale role.
#[test]
fn t1r_core_summary_half() {
    let pass_on = |no: u32, name: &'static str, callee: &str| {
        cu(
            no,
            name,
            &format!(
                "    procedure {name}(var R: Record Cust)\n    var\n        CX: Codeunit {callee};\n    begin\n        CX.{callee}(R);\n    end;\n"
            ),
        )
    };
    let ws = fixture(&[
        ("src/A.al", pass_on(50100, "A", "B")),
        ("src/B.al", pass_on(50101, "B", "L")),
        (
            "src/L.al",
            cu(
                50102,
                "L",
                "    procedure L(var R: Record Cust)\n    begin\n    end;\n",
            ),
        ),
    ]);
    let pre = |r0: &Fresh, r1: &Fresh| {
        for n in ["L", "B", "A"] {
            let role = |f: &Fresh| {
                f.routine(n, 1).0.as_ref().unwrap().parameter_roles[0].resets_filters_on_param
            };
            assert_eq!(role(r0), EffectPresence::No, "precondition: {n} at r0");
            assert_eq!(role(r1), EffectPresence::Yes, "precondition: {n} at r1");
            assert_eq!(
                r0.routine(n, 1).1,
                r1.routine(n, 1).1,
                "precondition: {n}'s db effects are equal"
            );
            assert_eq!(
                r0.routine(n, 1).1,
                Some(vec![]),
                "precondition: {n} has no db effect"
            );
        }
    };
    case_pre(
        "t1r",
        &as_refs(&ws),
        |d| {
            replace_once(
                d,
                "src/L.al",
                "    begin\n    end;",
                "    begin\n        R.Reset();\n    end;",
            )
        },
        RefreshKind::Incremental,
        &[
            &["L.L(var Record Cust)"],
            &["B.B(var Record Cust)"],
            &["A.A(var Record Cust)"],
        ],
        Some(&pre),
    );
}

/// Review R2-3: an unreadable `.alpackages` entry (a directory named
/// `Bad.app`) makes `dependency_closure_digest` fail; `refresh` returns
/// `Build` and keeps the baseline, and the next good refresh is Incremental.
/// Break: `expect` the digest.
#[test]
fn closure_error_keeps_baseline() {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_ws(dir, T1A);
        let mut host = IncrementalHost::new(dir, universe);
        refresh(&mut host);
        let before = host.projection().expect("committed");
        let bad = dir.join(".alpackages/Bad.app");
        std::fs::create_dir_all(&bad).unwrap();
        let err = host.refresh_committed(&never);
        assert!(
            matches!(&err, Err(RefreshError::Build(m)) if m.contains("Bad.app")),
            "{universe:?}: an unreadable .app is a build error: {err:?}"
        );
        assert_eq!(
            host.projection(),
            Some(before),
            "{universe:?}: the baseline is kept"
        );
        std::fs::remove_dir(&bad).unwrap();
        let report = refresh(&mut host);
        assert_eq!(report.kind, RefreshKind::Incremental, "{universe:?}");
        assert!(report.resolved.is_empty(), "{universe:?}: {report:?}");
    }
}
