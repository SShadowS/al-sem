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
    FullReason, IncrementalHost, L4Projection, PHASE_CHECKS, RefreshError, RefreshKind,
    RefreshReport, Refresher,
};
use al_sem::engine::l4::effect_lattice::EffectPresence;
use al_sem::engine::l4::summary::{DbEffect, RoutineSummary, TempState};
use al_sem::engine::session::{View, query_plan};
use al_sem::program::inputs::WorkspaceInputs;
use al_sem::program::model::workspace::ModelRoutine;

use crate::dep_dispatch_fixture::{
    DEP_APP_FILE, WS_AL, write_dependency, write_own, write_own_modify,
};

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

/// A plain `View::new` + `prepare(&query_plan())` on `dir`: today's command
/// path.
fn query_view(dir: &Path, universe: ViewUniverse) -> View {
    let inputs = WorkspaceInputs::read(dir, &Default::default());
    let target =
        AnalysisTarget::build(&inputs, universe == ViewUniverse::SingleApp, "query").unwrap();
    let mut view = View::new(target);
    view.prepare(&query_plan());
    view
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
        Fresh {
            proj,
            view: query_view(dir, universe),
        }
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
        let mut host = host_r0(name, (dir, universe));
        step(
            name,
            (dir, universe),
            &mut host,
            &edit,
            want_kind,
            want_resolved,
            pre,
        );
    }
}

/// A host on `dir`, refreshed once (a Full refresh with no baseline).
fn host_r0(name: &str, (dir, universe): (&Path, ViewUniverse)) -> IncrementalHost {
    let mut host = IncrementalHost::new(dir, universe);
    assert_eq!(
        refresh(&mut host).kind,
        RefreshKind::Full(FullReason::NoBaseline),
        "{name} {universe:?}: r0"
    );
    host
}

/// One edit on a committed host: refresh, the preconditions (fresh before
/// against fresh after), then the oracle, the kind and the minimality.
fn step(
    name: &str,
    (dir, universe): (&Path, ViewUniverse),
    host: &mut IncrementalHost,
    edit: &dyn Fn(&Path),
    want_kind: RefreshKind,
    want_resolved: &[&[&str]],
    pre: Pre<'_>,
) {
    let before = pre.map(|_| Fresh::of(dir, universe));
    edit(dir);
    let report = refresh(host);
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

/// T0b: on each oracle fixture, in both universes, the fresh host's bundle
/// part (db effects per id, the terminal partition), diagnostics, warnings
/// and SCC equal what `View::new` + `prepare(&query_plan())` leaves in
/// `view.l4()` (whose core summaries `prepare` drops). Break: the host
/// builds its target in the other universe. On T1A that break stays green
/// (no dependency: both universes give the single-app answer); on the
/// cross-app fixtures (`write_own`, T1i's r0) it fails.
#[test]
fn t0b_fresh_host_equals_query_plan() {
    type Write = fn(&Path);
    let fixtures: [(&str, Write); 3] = [
        ("T1A", |d| write_ws(d, T1A)),
        ("write_own", |d| write_own(d, "")),
        ("T1i r0", t1i_r0),
    ];
    for (fixture, write) in fixtures {
        for universe in UNIVERSES {
            let tmp = tempfile::tempdir().unwrap();
            write(tmp.path());
            let host = fresh_projection(tmp.path(), universe).without_core();
            let view = query_view(tmp.path(), universe);
            assert!(
                view.l4().bundle.routines_with_rows().next().is_some(),
                "precondition: {fixture} has bundle rows"
            );
            let want = L4Projection::of(view.l4(), &HashMap::new()).without_core();
            assert_eq!(
                host, want,
                "{fixture} {universe:?}: fresh host vs the query plan"
            );
        }
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

/// T1s (review R4-1): `A` dispatches `G.Go()` over interface `IGo`. The
/// edit moves `implements IGo` from `X` (inserts `Cust`) to `Y` (inserts
/// `Item`): only object headers change, so no routine row does, no id is
/// new and Δ is empty; `A`'s out-edge moves from `X.Go` to `Y.Go`, both
/// existing and unchanged. Only the `edges_by_from` part of `A`'s row digest
/// sees it. Break: edges out of the row digest seeds `A` with `Cust`.
#[test]
fn t1s_interface_rebind() {
    let go = |no: u32, path: &'static str, name: &str, implements: &str, table: &str| {
        (
            path,
            format!(
                "codeunit {no} {name}{implements}\n{{\n    procedure Go()\n    var\n        R: Record {table};\n    begin\n        R.Insert();\n    end;\n}}\n"
            ),
        )
    };
    let ws = fixture(&[
        (
            "src/IGo.al",
            "interface IGo\n{\n    procedure Go();\n}\n".to_string(),
        ),
        (
            "src/A.al",
            cu(
                50100,
                "A",
                "    procedure A()\n    var\n        G: Interface IGo;\n    begin\n        G.Go();\n    end;\n",
            ),
        ),
        go(50103, "src/X.al", "X", " implements IGo", "Cust"),
        go(50104, "src/Y.al", "Y", "", "Item"),
    ]);
    // `X`'s and `Y`'s OBJECT rows, which carry `implements_interfaces`.
    let objects = |f: &Fresh| {
        let ws = &f.view.model().workspace;
        let rows: Vec<String> = ["X", "Y"]
            .iter()
            .map(|n| {
                let o: Vec<_> = ws.objects.iter().filter(|o| o.name == *n).collect();
                assert_eq!(o.len(), 1, "precondition: one object {n}");
                format!("{:?}", o[0])
            })
            .collect();
        rows
    };
    let pre = |r0: &Fresh, r1: &Fresh| {
        assert_eq!(
            r0.rows("Go", 0).len(),
            2,
            "precondition: X and Y each have a Go row (the interface has none)"
        );
        assert_eq!(r0.rows("A", 0).len(), 1, "precondition: one A row");
        let (o0, o1) = (objects(r0), objects(r1));
        assert!(
            o0[0] != o1[0] && o0[1] != o1[1],
            "precondition: X's and Y's object rows change (implements)"
        );
        for n in ["A", "Go"] {
            assert_eq!(
                format!("{:?}", r0.rows(n, 0)),
                format!("{:?}", r1.rows(n, 0)),
                "precondition: every {n} row is equal at r0 and r1"
            );
        }
        assert_ne!(
            r0.edges("A", 0),
            r1.edges("A", 0),
            "precondition: A's out-edges move"
        );
        assert_ne!(
            r0.routine("A", 0),
            r1.routine("A", 0),
            "precondition: A's projection differs between r0 and r1"
        );
    };
    case_pre(
        "t1s",
        &as_refs(&ws),
        |d| {
            replace_once(d, "src/X.al", "X implements IGo", "X");
            replace_once(d, "src/Y.al", "Y\n", "Y implements IGo\n");
        },
        RefreshKind::Incremental,
        &[&["A.A()"]],
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

/// `CallChain` as `WS_AL` writes it, with the blank line after it.
const CALL_CHAIN: &str = "    procedure CallChain()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    begin\n        Lib.Outer();\n    end;\n\n";

/// T1i's r0: `write_own`, with `CallChain` moved alone into codeunit
/// `Ws Chain` (`src/Chain.al`) and calling `Lib.Inner()` directly.
fn t1i_r0(dir: &Path) {
    write_own(dir, "");
    assert_eq!(WS_AL.matches(CALL_CHAIN).count(), 1, "fixture: CallChain");
    replace_once(dir, "src/Work.al", CALL_CHAIN, "");
    let chain = CALL_CHAIN.replacen("Lib.Outer();", "Lib.Inner();", 1);
    assert_ne!(chain, CALL_CHAIN, "fixture: CallChain calls Inner");
    std::fs::write(
        dir.join("src/Chain.al"),
        cu(50172, "\"Ws Chain\"", chain.strip_suffix('\n').unwrap()),
    )
    .unwrap();
}

/// T1i (cross-app): `CallChain` stops and then starts again calling the
/// dependency's `Inner` (the only dependency routine with an L4-visible
/// effect, its `Customer` insert). r1 drops `Inner` from the demand; r2
/// brings it back as a new id. Break: R2 without "no member in Δ" seeds
/// `CallChain` at r1 with the old inherited insert. REDUNDANCY (spec §5.2):
/// demand adds and removes reach the decision only through new SCC keys and
/// the caller's own row (T1c, T1d); the demand input is pinned by T1n.
#[test]
fn t1i_demand() {
    let at = (tempfile::tempdir().unwrap(), ViewUniverse::CrossApp);
    let dir = at.0.path();
    t1i_r0(dir);
    let mut host = host_r0("t1i", (dir, at.1));
    let has_inner = |f: &Fresh| !f.rows("Inner", 0).is_empty();
    let pre_r1 = |r0: &Fresh, r1: &Fresh| {
        assert!(has_inner(r0), "precondition: r0's model has an Inner row");
        assert!(!has_inner(r1), "precondition: r1's model has no Inner row");
        assert_ne!(
            r0.routine("CallChain", 0),
            r1.routine("CallChain", 0),
            "precondition: CallChain's projection differs between r0 and r1"
        );
    };
    step(
        "t1i r1",
        (dir, at.1),
        &mut host,
        &|d| replace_once(d, "src/Chain.al", "        Lib.Inner();\n", ""),
        RefreshKind::Incremental,
        &[&["Ws Chain.CallChain()"]],
        Some(&pre_r1),
    );
    let pre_r2 = |r1: &Fresh, r2: &Fresh| {
        assert!(!has_inner(r1), "precondition: r1's model has no Inner row");
        assert!(has_inner(r2), "precondition: r2's model has an Inner row");
    };
    step(
        "t1i r2",
        (dir, at.1),
        &mut host,
        &|d| {
            replace_once(
                d,
                "src/Chain.al",
                "    begin\n    end;",
                "    begin\n        Lib.Inner();\n    end;",
            )
        },
        RefreshKind::Incremental,
        &[&["Dep Lib.Inner()"], &["Ws Chain.CallChain()"]],
        Some(&pre_r2),
    );
}

/// T1j (cross-app): a dependency change is Full(HostGlobal) through the
/// closure digest (Q2), (a) bytes only (a new revision comment) and (b)
/// semantic (`Inner` modifies instead of inserting), and (c) a package the
/// workspace does not declare lands in `.alpackages` (nothing the analysis
/// reads changes). Break B9 (no closure digest in R0) makes all three
/// Incremental. REDUNDANCY for the oracle (spec
/// §5.2): in (b) `Inner`'s own row changes, so Δ alone also answers right.
fn t1j(name: &str, edit: &dyn Fn(&Path), pre: &dyn Fn(&Fresh, &Fresh)) {
    let universe = ViewUniverse::CrossApp;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_own(dir, "");
    let mut host = host_r0(name, (dir, universe));
    step(
        name,
        (dir, universe),
        &mut host,
        edit,
        RefreshKind::Full(FullReason::HostGlobal),
        &[
            &["Dep Lib.Inner()"],
            &["Dep Lib.OnDepEvent()"],
            &["Dep Lib.Outer()"],
            &["Dep Sub.HandleDep()"],
            &["Ws Sub.HandleInWs()"],
            &["Ws Work.CallChain()"],
            &["Ws Work.RaiseDep()"],
            &["Ws Work.TouchCust()"],
        ],
        Some(pre),
    );
}

fn same_projection(r0: &Fresh, r1: &Fresh) {
    assert_eq!(r0.proj, r1.proj, "precondition: no projection changes");
}

#[test]
fn t1j_a_dependency_bytes_only() {
    t1j(
        "t1j (a)",
        &|d| write_dependency(&d.join(".alpackages").join(DEP_APP_FILE), 1),
        &same_projection,
    );
}

#[test]
fn t1j_b_dependency_semantic() {
    t1j("t1j (b)", &|d| write_own_modify(d, ""), &|r0, r1| {
        assert_ne!(r0.proj, r1.proj, "precondition: the projection changes");
    });
}

#[test]
fn t1j_c_undeclared_package() {
    const OTHER_GUID: &str = "cccc2222-0000-0000-0000-000000000222";
    t1j(
        "t1j (c)",
        &|d| {
            crate::symbol_app::write_symbol_app(
                &d.join(".alpackages/probe_Other_1.0.0.0.app"),
                OTHER_GUID,
                "Other",
                "1.0.0.0",
                &format!(
                    r#"{{"RuntimeVersion":"13.0","AppId":"{OTHER_GUID}","Name":"Other","Publisher":"probe","Version":"1.0.0.0"}}"#
                ),
            )
        },
        &same_projection,
    );
}

/// T3 (spec §5.4): a refresh cancelled at the top of the LAST SCC turn (call
/// `PHASE_CHECKS + total`: every earlier SCC, `L`'s re-solved one included,
/// has completed) returns `Cancelled` and keeps the r0 baseline whole; the
/// next refresh is Incremental against it. The exact call counts (the
/// cancelled run's, and the next, uncancelled run's) pin the three phase
/// checks (inputs, target, relation) and one check per SCC turn, both ways.
/// Break (O14): `last.take()` at the start of `refresh` leaves no projection.
#[test]
fn t3_cancel_keeps_baseline() {
    for universe in UNIVERSES {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_ws(dir, T1A);
        let mut host = IncrementalHost::new(dir, universe);
        let r0 = refresh(&mut host);
        let proj = host.projection();
        let fingerprint = host.baseline_fingerprint();
        assert!(fingerprint.is_some(), "{universe:?}: r0 has a baseline");
        replace_once(dir, "src/L.al", "C.Insert();", "C.Insert(); C.Modify();");
        let at = PHASE_CHECKS + r0.total;
        let calls = std::cell::Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == at
        };
        let err = host.refresh_committed(&cancel);
        assert_eq!(host.projection(), proj, "{universe:?}: projection kept");
        assert_eq!(
            host.baseline_fingerprint(),
            fingerprint,
            "{universe:?}: baseline fingerprint kept"
        );
        assert!(
            matches!(err, Err(RefreshError::Cancelled)),
            "{universe:?}: cancelled: {err:?}"
        );
        assert_eq!(calls.get(), at, "{universe:?}: cancel calls");
        // Two-sided (review R5-1): a refresh that never cancels makes exactly
        // `PHASE_CHECKS + total` checks, so an ADDED check fails too.
        let counted = std::cell::Cell::new(0);
        let report = host
            .refresh_committed(&|| {
                counted.set(counted.get() + 1);
                false
            })
            .expect("a good refresh");
        assert_eq!(
            counted.get(),
            PHASE_CHECKS + report.total,
            "{universe:?}: cancel calls of a full run"
        );
        assert_eq!(report.total, r0.total, "{universe:?}: same SCC count");
        assert_eq!(report.kind, RefreshKind::Incremental, "{universe:?}");
        assert_eq!(
            report.resolved,
            labels(&[&["L.L()"], &["B.B()"], &["A.A()"]]),
            "{universe:?}: re-solved against the r0 baseline"
        );
        assert_eq!(
            host.projection().expect("committed"),
            fresh_projection(dir, universe),
            "{universe:?}: oracle"
        );
    }
}
