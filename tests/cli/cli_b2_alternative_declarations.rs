//! Engine limits B2, wave 2 final review: one routine, object or table id
//! declared in more than one place that never compile together. The places
//! are two whole-object `#if` arms in one file (`#if A codeunit X {..} #else
//! codeunit X {..} #endif`, or two separate `#if` blocks), or two FILES (one
//! under `#if A`, one under `#if not A`). No configuration is chosen, so each
//! declaration is one possible build, and a fact one of them lacks is a
//! "may" (`workspace::join_alternative_declarations`, the program graph's
//! `dedup_objects` and `EventFacts::of_arms`).
//!
//! Every fixture states its precondition in its source: which declaration
//! carries the fact and which does not, and which file sorts first (the files
//! are read in name order, so `a.al` before `b.al`). Where the fact sits in
//! the FIRST file and where it sits in the SECOND, both orders are tested:
//! a reader that takes one row by id (the first, or the last) is wrong for
//! one of them.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b2b2b2b2-0000-4000-8000-0000000000f7","name":"B2AD","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(t.path().join("src").join(name), text).unwrap();
    }
    t
}

fn run(ws: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(ws)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// `prove . <routine> <question> --single-app` (JSON): the result object.
fn prove(ws: &Path, routine: &str, question: &str) -> Value {
    prove_with(ws, routine, question, &["--single-app"])
}

/// [`prove`] with `extra` flags instead (none: the cross-app model).
fn prove_with(ws: &Path, routine: &str, question: &str, extra: &[&str]) -> Value {
    let mut args = vec!["prove", ".", routine, question];
    args.extend_from_slice(extra);
    let out = run(ws, &args);
    let v: Value = serde_json::from_slice(&out).expect("json");
    v["payload"]["result"].clone()
}

// ---------------------------------------------------------------------------
// [TryFunction] in one declaration only
// ---------------------------------------------------------------------------

const TRY_CALLER: &str = r#"codeunit 50450 "AD Top"
{
    procedure CallT()
    var
        C: Codeunit "AD Try";
    begin
        if C.T() then;
    end;
}
"#;

/// The declaration of `"AD Try"` with `[TryFunction]` on `T` (`try`) or
/// without, wrapped in `open` and `close` (the `#if` lines).
fn try_decl(try_fn: bool, open: &str, close: &str) -> String {
    let attr = if try_fn { "    [TryFunction]\n" } else { "" };
    format!(
        "{open}codeunit 50451 \"AD Try\"\n{{\n{attr}    procedure T()\n    begin\n        Error('t');\n    end;\n}}\n{close}"
    )
}

/// The four places a second declaration can sit, each with `[TryFunction]`
/// on the first declaration (`first`) and on the second (`second`).
fn try_shapes(first: bool, second: bool) -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "one file, #if/#else",
            vec![(
                "s.al".to_string(),
                try_decl(first, "#if A\n", "#else\n") + &try_decl(second, "", "#endif\n"),
            )],
        ),
        (
            "one file, two #if blocks",
            vec![(
                "s.al".to_string(),
                try_decl(first, "#if A\n", "#endif\n")
                    + &try_decl(second, "#if not A\n", "#endif\n"),
            )],
        ),
        (
            "two files",
            vec![
                ("a.al".to_string(), try_decl(first, "#if A\n", "#endif\n")),
                (
                    "b.al".to_string(),
                    try_decl(second, "#if not A\n", "#endif\n"),
                ),
            ],
        ),
    ]
}

/// A `[TryFunction]` that only one declaration of `T` carries traps nothing
/// for certain: a build of the other declaration lets `Error('t')` escape
/// `CallT`, so `throws-error` is `yes` (before: a definite `no`, "caught on
/// every route by a trap", because `traps::is_try_function` read one row,
/// and the rows were compared only inside one object declaration). Each
/// place, with the attribute on the first declaration and on the second, in
/// the single-app model and the cross-app one (each assembly entry runs the
/// join). Control: on both declarations, the call is trapped in every build
/// (`no`).
#[test]
fn a_try_function_one_declaration_lacks_traps_nothing_for_certain() {
    for (first, second, want) in [
        (true, false, "yes"),
        (false, true, "yes"),
        (true, true, "no"),
    ] {
        for (shape, files) in try_shapes(first, second) {
            let mut all: Vec<(&str, &str)> = files
                .iter()
                .map(|(n, t)| (n.as_str(), t.as_str()))
                .collect();
            all.push(("t.al", TRY_CALLER));
            let t = workspace(&all);
            for extra in [&["--single-app"][..], &[]] {
                let r = prove_with(t.path(), "AD Top::CallT", "throws-error", extra);
                assert_eq!(
                    r["answer"], want,
                    "{shape} {extra:?}, [TryFunction] on first={first} second={second}: {r:#}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// One routine in two files: conditional sites, distinct site ids
// ---------------------------------------------------------------------------

const WORK_A: &str = r#"#if A
codeunit 50460 "AD Work"
{
    procedure P()
    var
        R: Record "AD A";
    begin
        R.Insert();
    end;
}
#endif
"#;

const WORK_B: &str = r#"#if not A
codeunit 50460 "AD Work"
{
    procedure P()
    var
        R: Record "AD B";
    begin
        R.Insert();
        Commit();
    end;
}
#endif
"#;

const WORK_T: &str = r#"table 50462 "AD A"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

table 50463 "AD B"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50461 "AD Caller"
{
    procedure Root()
    var
        W: Codeunit "AD Work";
    begin
        W.P();
    end;
}
"#;

/// `P` is declared once in `a.al` (`#if A`, inserts into `AD A`) and once in
/// `b.al` (`#if not A`, inserts into `AD B` and commits). A build runs one of
/// them, so each insert is `conditional` through `Root`, and `Root` does not
/// commit on every success path (a build with `A` never commits). Before:
/// both inserts `unconditional on success` and `commits-on-success-path: yes`
/// (`lift_arm_declarations` counted declarations per file). Each write also
/// cites its OWN file's line: the two files' rows shared their site ids
/// (`P/op0` in both), so `AD A`'s insert was cited at `b.al`'s line.
#[test]
fn a_routine_declared_in_two_files_is_conditional_and_cites_its_own_file() {
    let t = workspace(&[("a.al", WORK_A), ("b.al", WORK_B), ("t.al", WORK_T)]);
    for (table, file) in [("AD A", "ws:src/a.al"), ("AD B", "ws:src/b.al")] {
        let r = prove(
            t.path(),
            "AD Caller::Root",
            &format!("writes-table:{table}"),
        );
        assert_eq!(r["answer"], "yes", "{table}: {r:#}");
        let e = &r["evidence"][0];
        assert_eq!(e["conditionality"], "conditional", "{table}: {e:#}");
        assert_eq!(e["evidence"]["file"], file, "{table}: {e:#}");
        assert_eq!(e["evidence"]["line"], 8, "{table}: {e:#}");
    }
    let r = prove(t.path(), "AD Caller::Root", "commits-on-success-path");
    assert_eq!(r["answer"], "unknown", "{r:#}");
}

// ---------------------------------------------------------------------------
// TableType = Temporary in one declaration only
// ---------------------------------------------------------------------------

fn temp_table(temporary: bool, open: &str) -> String {
    let tt = if temporary {
        "    TableType = Temporary;\n"
    } else {
        ""
    };
    format!(
        "{open}table 50470 \"AD Temp\"\n{{\n{tt}    fields {{ field(1; \"No.\"; Integer) {{ }} }}\n    keys {{ key(PK; \"No.\") {{ }} }}\n}}\n#endif\n"
    )
}

const TEMP_WRITER: &str = r#"codeunit 50471 "AD TW"
{
    procedure W()
    var
        R: Record "AD Temp";
    begin
        R.Insert();
    end;
}
"#;

/// `TableType = Temporary` in one file's declaration of `AD Temp` only: a
/// build of the other file has a normal table, so `W`'s insert may write it.
/// Before: with the temporary declaration read last, a definite `no` ("no
/// write site ... every candidate decided"). Both file orders. Control: both
/// declarations temporary, no build writes the database (`no`).
#[test]
fn a_table_temporary_in_one_file_only_may_be_written() {
    for (a, b, want) in [
        (true, false, "yes"),
        (false, true, "yes"),
        (true, true, "no"),
    ] {
        let ta = temp_table(a, "#if A\n");
        let tb = temp_table(b, "#if not A\n");
        let t = workspace(&[("a.al", &ta), ("b.al", &tb), ("c.al", TEMP_WRITER)]);
        let r = prove(t.path(), "AD TW::W", "writes-table:AD Temp");
        assert_eq!(r["answer"], want, "temporary a={a} b={b}: {r:#}");
    }
}

// ---------------------------------------------------------------------------
// [EventSubscriber] in one file's declaration only
// ---------------------------------------------------------------------------

const PUB: &str = r#"codeunit 50480 "AD Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;

    procedure Raise()
    begin
        OnX();
    end;
}
"#;

fn sub(subscribes: bool, open: &str) -> String {
    let attr = if subscribes {
        "    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"AD Pub\", 'OnX', '', false, false)]\n"
    } else {
        ""
    };
    format!(
        "{open}codeunit 50481 \"AD Sub\"\n{{\n{attr}    procedure H()\n    begin\n        Commit();\n    end;\n}}\n#endif\n"
    )
}

/// `H` subscribes to `OnX` in one file's declaration only: a build of that
/// file commits when `Raise` raises the event. Before: the program graph kept
/// the FIRST file's copy's event facts (#103's per-file rule), so with the
/// subscriber in `b.al` no subscription existed in any build and `may-commit`
/// said `no`. Both file orders; control: neither declaration subscribes
/// (`no`).
#[test]
fn a_subscription_in_one_file_only_is_possible() {
    for (a, b, want) in [
        (false, true, "yes"),
        (true, false, "yes"),
        (false, false, "no"),
    ] {
        let sa = sub(a, "#if A\n");
        let sb = sub(b, "#if not A\n");
        let t = workspace(&[("a.al", &sa), ("b.al", &sb), ("p.al", PUB)]);
        let r = prove(t.path(), "AD Pub::Raise", "may-commit");
        assert_eq!(r["answer"], want, "subscriber a={a} b={b}: {r:#}");
    }
}

// ---------------------------------------------------------------------------
// XmlPort Direction in one file's declaration only
// ---------------------------------------------------------------------------

fn port(export: bool, open: &str) -> String {
    let dir = if export {
        "    Direction = Export;\n"
    } else {
        ""
    };
    format!(
        "{open}xmlport 50490 \"AD Port\"\n{{\n{dir}    schema {{ textelement(Root) {{ tableelement(El; \"AD Imp\") {{ }} }} }}\n}}\n#endif\n"
    )
}

const PORT_RUN: &str = r#"table 50491 "AD Imp"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50492 "AD PortRun"
{
    procedure RunIt()
    begin
        Xmlport.Run(Xmlport::"AD Port");
    end;
}
"#;

/// `Direction = Export` in one file's declaration of `AD Port` only: a build
/// of the other file keeps the default `Both`, so `Xmlport.Run` may import
/// and insert into `AD Imp`. Before: the program graph kept the first file's
/// copy (`dedup_objects`), so with `Export` in `a.al` a definite `no`. Both
/// file orders; control: `Export` in both, no build imports (`no`).
#[test]
fn a_direction_in_one_file_only_may_still_import() {
    for (a, b, want) in [
        (true, false, "yes"),
        (false, true, "yes"),
        (true, true, "no"),
    ] {
        let pa = port(a, "#if A\n");
        let pb = port(b, "#if not A\n");
        let t = workspace(&[("a.al", &pa), ("b.al", &pb), ("c.al", PORT_RUN)]);
        let r = prove(t.path(), "AD PortRun::RunIt", "writes-table:AD Imp");
        assert_eq!(r["answer"], want, "export a={a} b={b}: {r:#}");
    }
}

// ---------------------------------------------------------------------------
// digest: each site's own operation id; no dominance through a two-arm callee
// ---------------------------------------------------------------------------

const ORD: &str = r#"table 50495 "AD Log"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50496 "AD Ord"
{
#if A
    procedure TwoArm(k: Integer)
    var
        L: Record "AD Log";
    begin
        L.Insert();
        Commit();
    end;
#else
    procedure TwoArm(k: Integer)
    var
        L: Record "AD Log";
    begin
        L.Insert();
        if k = 1 then exit;
        Commit();
    end;
#endif

    procedure TwoCommits(k: Integer)
    var
        L: Record "AD Log";
    begin
        L.Insert();
        Commit();
        L.Modify();
        Commit();
    end;

    procedure OneArm()
    var
        L: Record "AD Log";
    begin
        L.Insert();
        Commit();
    end;

    procedure CallsTwoArm(k: Integer)
    begin
        TwoArm(k);
    end;

    procedure CallsTwoCommits(k: Integer)
    begin
        TwoCommits(k);
    end;

    procedure CallsOneArm()
    begin
        OneArm();
    end;
}
"#;

/// `digest . --routine <r> --single-app --detail --compact=false`: the
/// root's effects.
fn digest_effects(ws: &Path, routine: &str) -> Vec<Value> {
    let out = run(
        ws,
        &[
            "digest",
            ".",
            "--routine",
            routine,
            "--single-app",
            "--detail",
            "--compact=false",
        ],
    );
    let v: Value = serde_json::from_slice(&out).expect("json");
    v["payload"]["entries"][0]["effects"]
        .as_array()
        .unwrap_or_else(|| panic!("effects: {v:#}"))
        .clone()
}

/// Every terminal (`paths[].terminal`) of `effects`: (operation id, line).
fn terminals(effects: &[Value]) -> Vec<(String, u64)> {
    effects
        .iter()
        .flat_map(|e| e["paths"].as_array().cloned().unwrap_or_default())
        .map(|p| {
            let t = &p["terminal"];
            (
                t["operationId"].as_str().unwrap_or_default().to_string(),
                t["line"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

/// A caller's digest names each reached site by its OWN operation id. An
/// inherited effect enumerated per site took the operation id of the fact's
/// representative, so two sites of one callee (one `Commit` per `#if` arm of
/// `TwoArm`; or two `Commit`s in plain `TwoCommits`) printed one id at two
/// lines (the review's `ord` probe: `…/op1` at lines 128 and 137). Stated:
/// each fixture's callee holds two commits on two lines.
#[test]
fn a_callers_digest_names_each_site_by_its_own_operation() {
    let t = workspace(&[("o.al", ORD)]);
    for root in ["AD Ord::CallsTwoArm", "AD Ord::CallsTwoCommits"] {
        let ts = terminals(&digest_effects(t.path(), root));
        let mut by_op: std::collections::BTreeMap<&str, std::collections::BTreeSet<u64>> =
            std::collections::BTreeMap::new();
        for (op, line) in &ts {
            by_op.entry(op.as_str()).or_default().insert(*line);
        }
        let commits: std::collections::BTreeSet<u64> = ORD
            .lines()
            .enumerate()
            .filter(|(_, l)| l.trim() == "Commit();")
            .map(|(i, _)| (i + 1) as u64)
            .collect();
        let shown: std::collections::BTreeSet<u64> = ts
            .iter()
            .map(|(_, l)| *l)
            .filter(|l| commits.contains(l))
            .collect();
        assert_eq!(shown.len(), 2, "{root}: two commit sites: {ts:?}");
        for (op, lines) in &by_op {
            assert_eq!(lines.len(), 1, "{root}: {op} names {lines:?}: {ts:?}");
        }
    }
}

/// A two-arm callee's `Commit` does not dominate its caller's return: a build
/// of arm B may `exit` before its own commit, and arm A's commit does not
/// exist there. Before: arm A's commit carried a root-scope
/// `COMMIT_DOMINATES_RETURN`, valid for refutation (the review's non-blocking
/// item 1). Control: the same shape in a routine declared once (`OneArm`)
/// keeps it.
#[test]
fn a_two_arm_callees_commit_does_not_dominate_the_callers_return() {
    let t = workspace(&[("o.al", ORD)]);
    let root_dominance = |root: &str| {
        digest_effects(t.path(), root).iter().any(|e| {
            e["scopedGuarantees"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|s| {
                    s["label"] == "COMMIT_DOMINATES_RETURN"
                        && s["scope"] == "root"
                        && s["validForRefutation"] == true
                })
        })
    };
    assert!(!root_dominance("AD Ord::CallsTwoArm"));
    assert!(root_dominance("AD Ord::CallsOneArm"), "control");
}
