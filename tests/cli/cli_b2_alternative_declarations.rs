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

// ---------------------------------------------------------------------------
// A dispatch reaches only the declarations that subscribe (review B1)
// ---------------------------------------------------------------------------

const PUB_XY: &str = r#"codeunit 50480 "AD Pub"
{
    procedure RaiseX()
    begin
        OnX();
    end;

    procedure RaiseY()
    begin
        OnY();
    end;

    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnY()
    begin
    end;
}
"#;

/// One declaration of `"AD Sub"` whose `H` subscribes to `event` and runs
/// `body`, wrapped in `open` and `close`.
fn sub_on(event: &str, body: &str, open: &str, close: &str) -> String {
    format!(
        "{open}codeunit 50481 \"AD Sub\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"AD Pub\", '{event}', '', false, false)]\n    procedure H()\n    begin\n{body}    end;\n}}\n{close}"
    )
}

/// `H` is declared twice: the declaration that subscribes to `OnX` commits,
/// the one that subscribes to `OnY` does nothing. No build runs the
/// committing body when `OnY` is raised, so `RaiseY may-commit` is `no`
/// (the review's probes e1 and e2: `yes`, citing the `OnX` declaration's
/// `Commit`, since #103 in one file and since the per-file rule was dropped
/// for two files). `RaiseX` still commits (control). Shapes: one file
/// `#if`/`#else` (the `OnX` declaration first, then second), two files (the
/// `OnX` declaration in `a.al`, then in `b.al`).
#[test]
fn a_dispatch_reaches_only_the_declarations_that_subscribe() {
    let commit = "        Commit();\n";
    for x_first in [true, false] {
        let (first, second) = if x_first {
            (("OnX", commit), ("OnY", ""))
        } else {
            (("OnY", ""), ("OnX", commit))
        };
        let one_file = sub_on(first.0, first.1, "#if A\n", "#else\n")
            + &sub_on(second.0, second.1, "", "#endif\n");
        let a = sub_on(first.0, first.1, "#if A\n", "#endif\n");
        let b = sub_on(second.0, second.1, "#if not A\n", "#endif\n");
        let shapes: [(&str, Vec<(&str, &str)>); 2] = [
            (
                "one file",
                vec![("s.al", one_file.as_str()), ("p.al", PUB_XY)],
            ),
            (
                "two files",
                vec![("a.al", a.as_str()), ("b.al", b.as_str()), ("p.al", PUB_XY)],
            ),
        ];
        for (shape, files) in shapes {
            let t = workspace(&files);
            for extra in [&["--single-app"][..], &[]] {
                let y = prove_with(t.path(), "AD Pub::RaiseY", "may-commit", extra);
                assert_eq!(
                    y["answer"], "no",
                    "{shape} x_first={x_first} {extra:?}: {y:#}"
                );
                let x = prove_with(t.path(), "AD Pub::RaiseX", "may-commit", extra);
                assert_eq!(
                    x["answer"], "yes",
                    "{shape} x_first={x_first} {extra:?}: {x:#}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A declaration's own [CommitBehavior] is definite for its own commits
// ---------------------------------------------------------------------------

const CB_TOP: &str = r#"codeunit 50501 "CBTop"
{
    procedure Run()
    var
        C: Codeunit "CB";
    begin
        C.P();
    end;
}
"#;

/// One declaration of `CB::P` (the routine only), with
/// `[CommitBehavior(Ignore)]` (`ignore`) or not, its body `body`.
fn cb_decl(ignore: bool, body: &str) -> String {
    let attr = if ignore {
        "    [CommitBehavior(CommitBehavior::Ignore)]\n"
    } else {
        ""
    };
    format!("{attr}    procedure P()\n    begin\n{body}    end;\n")
}

/// The review's probes c1, c2 and c3: `[CommitBehavior(Ignore)]` on ONE
/// declaration of `P`, and the `Commit` in that same declaration. No build
/// commits (the build that has the commit ignores it), so `may-commit` is
/// `no` for `P` and its caller (before: `yes (conditional)`: the attribute
/// was read as "may" on its own row, and the id's rows were one node of the
/// commit-scope graph). Control: the `Commit` in the OTHER declaration, which
/// no attribute covers: `yes`. Shapes: member-list arms (c1), two files (c2),
/// whole-object `#if`/`#else` (c3).
#[test]
fn an_own_commit_behavior_is_definite_for_its_own_commit() {
    let commit = "        Commit();\n";
    for (commit_in_ignoring, want) in [(true, "no"), (false, "yes")] {
        let (a_body, b_body) = if commit_in_ignoring {
            (commit, "")
        } else {
            ("", commit)
        };
        let obj = |inner: &str| format!("codeunit 50500 \"CB\"\n{{\n{inner}}}\n");
        let c1 = obj(&format!(
            "#if A\n{}#else\n{}#endif\n",
            cb_decl(true, a_body),
            cb_decl(false, b_body)
        ));
        let c2a = format!("#if A\n{}#endif\n", obj(&cb_decl(true, a_body)));
        let c2b = format!("#if not A\n{}#endif\n", obj(&cb_decl(false, b_body)));
        let c3 = format!(
            "#if A\n{}#else\n{}#endif\n",
            obj(&cb_decl(true, a_body)),
            obj(&cb_decl(false, b_body))
        );
        let shapes: [(&str, Vec<(&str, &str)>); 3] = [
            (
                "c1 member arms",
                vec![("a.al", c1.as_str()), ("t.al", CB_TOP)],
            ),
            (
                "c2 two files",
                vec![
                    ("a.al", c2a.as_str()),
                    ("b.al", c2b.as_str()),
                    ("t.al", CB_TOP),
                ],
            ),
            (
                "c3 whole objects",
                vec![("s.al", c3.as_str()), ("t.al", CB_TOP)],
            ),
        ];
        for (shape, files) in shapes {
            let t = workspace(&files);
            for routine in ["CBTop::Run", "CB::P"] {
                let r = prove(t.path(), routine, "may-commit");
                assert_eq!(
                    r["answer"], want,
                    "{shape} {routine} commit_in_ignoring={commit_in_ignoring}: {r:#}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A declaration's site ids do not depend on another file's content or order
// ---------------------------------------------------------------------------

/// The operation id of every terminal `(file, line)` in `--routine`'s
/// digest.
fn op_ids_by_place(ws: &Path, routine: &str) -> std::collections::BTreeMap<(String, u64), String> {
    let mut out = std::collections::BTreeMap::new();
    for e in digest_effects(ws, routine) {
        for p in e["paths"].as_array().cloned().unwrap_or_default() {
            let t = &p["terminal"];
            if let (Some(f), Some(l), Some(op)) = (
                t["file"].as_str(),
                t["line"].as_u64(),
                t["operationId"].as_str(),
            ) {
                out.insert((f.to_string(), l), op.to_string());
            }
        }
    }
    out
}

/// `P` is declared in two files. `b.al`'s sites keep their ids when `a.al`
/// gains sites (the review: `op5` became `op7`) and when `a.al` is renamed
/// so it sorts after `b.al`: each file's numbers sit in a block picked from
/// its own path. Precondition stated: `b.al` is byte-identical in all three
/// workspaces; the two `a.al` variants differ in their number of sites.
#[test]
fn a_files_site_ids_do_not_move_when_another_file_changes() {
    let more_a = WORK_A.replace(
        "        R.Insert();\n",
        "        R.Insert();\n        R.Modify();\n        R.Delete();\n",
    );
    assert_ne!(more_a, WORK_A, "precondition: a.al gains sites");
    let b_ids = |files: &[(&str, &str)]| {
        let t = workspace(files);
        op_ids_by_place(t.path(), "AD Work::P")
            .into_iter()
            .filter(|((f, _), _)| f == "ws:src/b.al")
            .collect::<Vec<_>>()
    };
    let base = b_ids(&[("a.al", WORK_A), ("b.al", WORK_B), ("t.al", WORK_T)]);
    assert!(!base.is_empty(), "precondition: b.al's sites are listed");
    let grown = b_ids(&[("a.al", &more_a), ("b.al", WORK_B), ("t.al", WORK_T)]);
    assert_eq!(base, grown, "a.al gained sites");
    let renamed = b_ids(&[("z.al", WORK_A), ("b.al", WORK_B), ("t.al", WORK_T)]);
    assert_eq!(base, renamed, "a.al renamed to sort after b.al");
}

// ---------------------------------------------------------------------------
// One routine per declaration of a subscriber (review of 600e6943: N1-N3)
// ---------------------------------------------------------------------------

/// One declaration of `"AD Sub"::H` subscribing to `publisher`'s `event`,
/// with `extra` attribute lines, `vars` and `body`.
fn sub_decl(publisher: &str, event: &str, extra: &str, vars: &str, body: &str) -> String {
    format!(
        "codeunit 50481 \"AD Sub\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"{publisher}\", '{event}', '', false, false)]\n{extra}    procedure H()\n{vars}    begin\n{body}    end;\n}}\n"
    )
}

/// The three places two declarations `first` and `second` can sit (one
/// file `#if`/`#else`, two files), each in both orders: (shape, files).
fn decl_shapes(first: &str, second: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for (tag, x, y) in [("first", first, second), ("second", second, first)] {
        out.push((
            format!("one file, the {tag} declaration first"),
            vec![("s.al".to_string(), format!("#if A\n{x}#else\n{y}#endif\n"))],
        ));
        out.push((
            format!("two files, the {tag} declaration in a.al"),
            vec![
                ("a.al".to_string(), format!("#if A\n{x}#endif\n")),
                ("b.al".to_string(), format!("#if not A\n{y}#endif\n")),
            ],
        ));
    }
    out
}

/// Runs `check` on every shape of `first`/`second` plus `extra_files`, in the
/// single-app and the cross-app model.
fn each_shape(
    first: &str,
    second: &str,
    extra_files: &[(&str, &str)],
    check: impl Fn(&Path, &[&str], &str),
) {
    for (shape, files) in decl_shapes(first, second) {
        let mut all: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        all.extend_from_slice(extra_files);
        let t = workspace(&all);
        for extra in [&["--single-app"][..], &[]] {
            check(t.path(), extra, &format!("{shape} {extra:?}"));
        }
    }
}

const PUB_TWO: &str = r#"codeunit 50480 "AD Pub"
{
    procedure RaiseX()
    begin
        OnX();
    end;

    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;
}

codeunit 50482 "AD Pub2"
{
    procedure RaiseX2()
    begin
        OnX();
    end;

    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;
}
"#;

/// N1 (probe e3): the two declarations subscribe to events of the SAME name
/// on two publishers: `"AD Pub".OnX` (commits) and `"AD Pub2".OnX` (does
/// nothing). A subscription is matched on its full identity (publisher kind
/// and object, event, element), so raising `"AD Pub2".OnX` runs only the
/// empty declaration: `RaiseX2 may-commit` is `no` (it was `yes`, citing the
/// other declaration's `Commit`: the match read the event name only).
/// Control: `RaiseX` commits.
#[test]
fn a_subscription_is_matched_on_its_publisher_too() {
    let commits = sub_decl("AD Pub", "OnX", "", "", "        Commit();\n");
    let empty = sub_decl("AD Pub2", "OnX", "", "", "");
    each_shape(&commits, &empty, &[("p.al", PUB_TWO)], |ws, extra, at| {
        let r = prove_with(ws, "AD Pub2::RaiseX2", "may-commit", extra);
        assert_eq!(r["answer"], "no", "{at}: {r:#}");
        let r = prove_with(ws, "AD Pub::RaiseX", "may-commit", extra);
        assert_eq!(r["answer"], "yes", "{at}: {r:#}");
    });
}

const Q_COMMITS: &str = r#"codeunit 50490 "AD Q"
{
    procedure C()
    begin
        Commit();
    end;
}
"#;

/// N2 (probes ce, ci): the `OnX` declaration carries
/// `[CommitBehavior(Error|Ignore)]`, the `OnY` one none; both call `Q.C()`,
/// which commits. The commit-scope graph reached every declaration through
/// a dispatch, so `RaiseY throws-error` said `yes` ("CommitBehavior Error on
/// H") and `RaiseX may-commit` `yes`. Each dispatch reaches only its own
/// declaration: under `Error`, `RaiseX` raises the error and never commits,
/// `RaiseY` commits and raises nothing; under `Ignore`, `RaiseX` commits
/// nothing.
#[test]
fn a_dispatch_runs_under_its_own_declarations_commit_behavior() {
    let vars = "    var\n        Q: Codeunit \"AD Q\";\n";
    let call = "        Q.C();\n";
    for behavior in ["Error", "Ignore"] {
        let attr = format!("    [CommitBehavior(CommitBehavior::{behavior})]\n");
        let x = sub_decl("AD Pub", "OnX", &attr, vars, call);
        let y = sub_decl("AD Pub", "OnY", "", vars, call);
        each_shape(
            &x,
            &y,
            &[("p.al", PUB_XY), ("q.al", Q_COMMITS)],
            |ws, extra, at| {
                let ask = |r: &str, q: &str| prove_with(ws, r, q, extra)["answer"].clone();
                assert_eq!(ask("AD Pub::RaiseX", "may-commit"), "no", "{behavior} {at}");
                assert_eq!(
                    ask("AD Pub::RaiseY", "may-commit"),
                    "yes",
                    "{behavior} {at}"
                );
                if behavior == "Error" {
                    assert_eq!(ask("AD Pub::RaiseY", "throws-error"), "no", "{at}");
                    assert_eq!(ask("AD Pub::RaiseX", "throws-error"), "yes", "{at}");
                }
            },
        );
    }
}

const W_TABLE: &str = r#"table 50495 "AD W"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}
"#;

/// N3 (probe e4): the `OnX` declaration inserts into `AD W`, the `OnY` one
/// writes nothing. The L4 summary solver entered every declaration through
/// a dispatch, so `query touches --table "AD W"` listed `RaiseY` and `OnY`,
/// and `query effects --routine RaiseY` counted the insert. Now neither:
/// `RaiseX` and `OnX` touch it, `RaiseY` reaches no operation.
#[test]
fn the_table_queries_follow_only_the_subscribing_declaration() {
    let vars = "    var\n        W: Record \"AD W\";\n";
    let x = sub_decl("AD Pub", "OnX", "", vars, "        W.Insert();\n");
    let y = sub_decl("AD Pub", "OnY", "", "", "");
    each_shape(
        &x,
        &y,
        &[("p.al", PUB_XY), ("w.al", W_TABLE)],
        |ws, extra, at| {
            let mut args = vec!["query", "touches", ".", "--table", "AD W", "--all"];
            args.extend_from_slice(extra);
            let touches = String::from_utf8(run(ws, &args)).unwrap();
            assert!(touches.contains("\"AD Pub\"::RaiseX"), "{at}: {touches}");
            assert!(!touches.contains("RaiseY"), "{at}: {touches}");
            assert!(!touches.contains("::OnY"), "{at}: {touches}");
            let mut args = vec!["query", "effects", "--routine", "AD Pub::RaiseY", "."];
            args.extend_from_slice(extra);
            let effects = String::from_utf8(run(ws, &args)).unwrap();
            assert!(effects.contains("answer: 0 operations"), "{at}: {effects}");
        },
    );
}

/// A table event's declaration of `"AD Sub"::H`: subscribes to `event` of
/// table `AD W`, runs `body`.
fn table_sub_decl(event: &str, body: &str) -> String {
    format!(
        "codeunit 50481 \"AD Sub\"\n{{\n    [EventSubscriber(ObjectType::Table, Database::\"AD W\", '{event}', '', false, false)]\n    procedure H(var Rec: Record \"AD W\"; RunTrigger: Boolean)\n    begin\n{body}    end;\n}}\n"
    )
}

const TABLE_WRITERS: &str = r#"codeunit 50496 "AD TW"
{
    procedure Ins()
    var
        W: Record "AD W";
    begin
        W.Insert(true);
    end;

    procedure Mod()
    var
        W: Record "AD W";
    begin
        W.Modify(true);
    end;
}
"#;

/// A table event subscribed by only one declaration of a split subscriber
/// reaches only that one (the table-event edges, `program_calls`): the
/// `OnAfterInsertEvent` declaration commits, the `OnAfterModifyEvent` one
/// does nothing. `Ins` may commit; `Mod` may not (before the split, the
/// dispatch reached both bodies). Both orders, both shapes, single-app and
/// cross-app.
#[test]
fn a_table_event_reaches_only_the_declaration_that_subscribes() {
    let ins = table_sub_decl("OnAfterInsertEvent", "        Commit();\n");
    let modify = table_sub_decl("OnAfterModifyEvent", "");
    each_shape(
        &ins,
        &modify,
        &[("w.al", W_TABLE), ("t.al", TABLE_WRITERS)],
        |ws, extra, at| {
            let r = prove_with(ws, "AD TW::Ins", "may-commit", extra);
            assert_eq!(r["answer"], "yes", "{at}: {r:#}");
            let r = prove_with(ws, "AD TW::Mod", "may-commit", extra);
            assert_eq!(r["answer"], "no", "{at}: {r:#}");
        },
    );
}

const H_CALLER: &str = r#"codeunit 50497 "AD Caller"
{
    procedure Go()
    var
        S: Codeunit "AD Sub";
    begin
        S.H();
    end;
}
"#;

/// A DIRECT call to a split subscriber reaches every declaration (each is a
/// possible build): the declaration that commits is the first, then the
/// second (the conversion names one of them; that one alone was the empty
/// declaration in one of the orders). `Go may-commit` is `yes` in every
/// shape and order.
#[test]
fn a_direct_call_reaches_every_declaration_of_a_split_subscriber() {
    let commits = sub_decl("AD Pub", "OnX", "", "", "        Commit();\n");
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    each_shape(
        &commits,
        &empty,
        &[("p.al", PUB_XY), ("c.al", H_CALLER)],
        |ws, extra, at| {
            let r = prove_with(ws, "AD Caller::Go", "may-commit", extra);
            assert_eq!(r["answer"], "yes", "{at}: {r:#}");
        },
    );
}

// ---------------------------------------------------------------------------
// An [EventSubscriber] the engine cannot read is a gap, never a definite `no`
// ---------------------------------------------------------------------------

/// An `AD Sub`.`H` under `[EventSubscriber(<attr>)]` running `body`.
fn sub_with_attr(attr: &str, body: &str) -> String {
    format!(
        "codeunit 50481 \"AD Sub\"\n{{\n    [EventSubscriber({attr})]\n    procedure H()\n    begin\n{body}    end;\n}}\n"
    )
}

const PARENS_ON_X: &str = "ObjectType::Codeunit, (Codeunit::\"AD Pub\"), 'OnX', '', false, false";

const FOO_ON_X: &str = "ObjectType::Foo, Codeunit::\"AD Pub\", 'OnX', '', false, false";

/// Whether a `prove` result's blockers name the unreadable attribute.
fn blocked_by_unreadable(r: &Value) -> bool {
    r["blockedBy"].as_array().is_some_and(|b| {
        b.iter().any(|x| {
            x["detail"]
                .as_str()
                .is_some_and(|d| d.contains("[EventSubscriber] the engine could not read"))
        })
    })
}

/// P1, one declaration: each attribute shape the parser cannot read left the
/// routine with no subscription, so `RaiseX may-commit` said a definite `no`
/// although `H` may subscribe to `OnX` and commits. Now `unknown`, naming the
/// reason. The readable parts narrow it (`RaiseY`).
#[test]
fn an_unreadable_subscriber_never_gives_a_definite_no() {
    // (shape, attribute, `RaiseY`'s answer).
    let shapes = [
        (
            "an integer object type",
            "5, Codeunit::\"AD Pub\", 'OnX', '', false, false",
            "no",
        ),
        (
            "two arguments only",
            "ObjectType::Codeunit, Codeunit::\"AD Pub\"",
            "unknown",
        ),
        ("a parenthesised publisher", PARENS_ON_X, "no"),
        (
            "a computed event name",
            "ObjectType::Codeunit, Codeunit::\"AD Pub\", 'On' + 'X', '', false, false",
            "unknown",
        ),
        (
            "an object type no event is published on",
            "ObjectType::Foo, Codeunit::\"AD Pub\", 'OnX', '', false, false",
            "no",
        ),
        (
            "a bare identifier as the publisher",
            "ObjectType::Codeunit, Foo, 'OnX', '', false, false",
            "no",
        ),
    ];
    for (shape, attr, raise_y) in shapes {
        let sub = sub_with_attr(attr, "        Commit();\n");
        let t = workspace(&[("s.al", &sub), ("p.al", PUB_XY)]);
        for extra in [&["--single-app"][..], &[]] {
            let r = prove_with(t.path(), "AD Pub::RaiseX", "may-commit", extra);
            assert_eq!(r["answer"], "unknown", "{shape} {extra:?}: {r:#}");
            assert!(blocked_by_unreadable(&r), "{shape} {extra:?}: {r:#}");
            let y = prove_with(t.path(), "AD Pub::RaiseY", "may-commit", extra);
            assert_eq!(y["answer"], raise_y, "{shape} {extra:?}: {y:#}");
        }
    }
}

/// The gap reaches explain, digest and the queries too: `RaiseX` shows no
/// effect there, and none of them says the answer is complete.
#[test]
fn an_unreadable_subscriber_shows_in_explain_digest_and_query() {
    let sub = sub_with_attr(PARENS_ON_X, "        Commit();\n");
    let t = workspace(&[("s.al", &sub), ("p.al", PUB_XY), ("w.al", W_TABLE)]);
    let text = |args: &[&str]| String::from_utf8(run(t.path(), args)).unwrap();
    let explain = text(&[
        "explain",
        ".",
        "--routine",
        "AD Pub::RaiseX",
        "--format",
        "human",
    ]);
    assert!(explain.contains("analysis partial"), "{explain}");
    let digest = text(&["digest", ".", "--routine", "AD Pub::RaiseX"]);
    assert!(digest.contains("unreadable-event-subscriber"), "{digest}");
    let effects = text(&["query", "effects", "--routine", "AD Pub::RaiseX", "."]);
    assert!(effects.contains("analysis partial"), "{effects}");
    let touches = text(&["query", "touches", ".", "--table", "AD W"]);
    assert!(touches.contains("could not read"), "{touches}");
}

/// A table event is raised by a record operation, with no publisher routine
/// to carry the gap. `(Database::"AD W")` hides the table, `OnAfterInsertEvent`
/// is readable: `Insert` is `unknown`, `Modify` raises another event and stays
/// `no`.
#[test]
fn an_unreadable_table_subscriber_is_a_gap_at_the_record_operation() {
    let sub = sub_with_attr(
        "ObjectType::Table, (Database::\"AD W\"), 'OnAfterInsertEvent', '', false, false",
        "        Commit();\n",
    );
    let writer = "codeunit 50492 \"AD Writer\"\n{\n    procedure Ins()\n    var\n        W: Record \"AD W\";\n    begin\n        W.Insert();\n    end;\n\n    procedure Mod()\n    var\n        W: Record \"AD W\";\n    begin\n        W.Modify();\n    end;\n}\n";
    let t = workspace(&[("s.al", &sub), ("w.al", W_TABLE), ("v.al", writer)]);
    for extra in [&["--single-app"][..], &[]] {
        let r = prove_with(t.path(), "AD Writer::Ins", "may-commit", extra);
        assert_eq!(r["answer"], "unknown", "{extra:?}: {r:#}");
        let m = prove_with(t.path(), "AD Writer::Mod", "may-commit", extra);
        assert_eq!(m["answer"], "no", "{extra:?}: {m:#}");
    }
}

/// P1, the split (probe u2): one declaration's subscription reads `OnX`, the
/// other's `(Codeunit::"AD Pub")` cannot be read and it commits. The two
/// subscription lists differed, so the routine split and the committing body
/// was reachable from no dispatch: `RaiseX may-commit` said `no`. An unreadable
/// declaration does not count as subscribing differently, so the routine stays
/// whole and `RaiseX` reaches the commit.
#[test]
fn an_unreadable_declaration_does_not_split_its_routine() {
    let readable = sub_decl("AD Pub", "OnX", "", "", "");
    let unreadable = sub_with_attr(PARENS_ON_X, "        Commit();\n");
    each_shape(
        &readable,
        &unreadable,
        &[("p.al", PUB_XY)],
        |ws, extra, at| {
            let r = prove_with(ws, "AD Pub::RaiseX", "may-commit", extra);
            assert_eq!(r["answer"], "yes", "{at}: {r:#}");
            // Not split: one routine, without a declaration suffix.
            let out = String::from_utf8(run(ws, &["graph", "."])).unwrap();
            let ids: Vec<String> = out
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .filter(|v| v["t"] == "node" && v["v"]["objectName"] == "AD Sub")
                .filter_map(|v| v["v"]["stableRoutineId"].as_str().map(str::to_string))
                .collect();
            assert_eq!(ids.len(), 1, "{at}: {ids:?}");
            assert!(!ids[0].contains('~'), "{at}: {ids:?}");
        },
    );
}

/// P1, a split that happens for other declarations: two readable declarations
/// subscribe differently, so they split; the third cannot be read and commits.
/// It stays reachable from both readable events.
#[test]
fn an_unreadable_declaration_stays_reachable_when_the_others_split() {
    let on_x = format!("#if A\n{}#endif\n", sub_decl("AD Pub", "OnX", "", "", ""));
    let on_y = format!("#if B\n{}#endif\n", sub_decl("AD Pub", "OnY", "", "", ""));
    let odd = format!(
        "#if C\n{}#endif\n",
        sub_with_attr(PARENS_ON_X, "        Commit();\n")
    );
    let t = workspace(&[
        ("a.al", &on_x),
        ("b.al", &on_y),
        ("c.al", &odd),
        ("p.al", PUB_XY),
    ]);
    for (raise, at) in [("AD Pub::RaiseX", "OnX"), ("AD Pub::RaiseY", "OnY")] {
        let r = prove_with(t.path(), raise, "may-commit", &[]);
        assert_eq!(r["answer"], "yes", "{at}: {r:#}");
    }
}

/// The callers' walker starts at a routine and goes up: a subscriber whose
/// attribute cannot be read has dispatchers the graph does not show, so a
/// `[TryFunction]` above one of them cannot be ruled out. `AD Q::C` runs
/// under it: not a definite `no`.
#[test]
fn an_unreadable_subscribers_callers_are_not_all_known() {
    // An `ObjectType::Foo`: it parses, so no parse recovery hides the question.
    let sub = sub_with_attr(FOO_ON_X, "        Q.C();\n").replace(
        "    begin\n",
        "    var\n        Q: Codeunit \"AD Q\";\n    begin\n",
    );
    let t = workspace(&[("s.al", &sub), ("p.al", PUB_XY), ("q.al", Q_ERRORS)]);
    let r = prove_with(
        t.path(),
        "AD Q::C",
        "callers-intercept-errors",
        &["--no-dependents"],
    );
    assert_eq!(r["answer"], "unknown", "{r:#}");
}

/// The same, when the routine also has a readable subscription: the node has
/// a bound event, and still dispatchers the graph does not show.
#[test]
fn a_subscriber_with_a_bound_event_and_an_unreadable_one_has_hidden_callers() {
    let readable = sub_decl("AD Pub", "OnX", "", "", "");
    let unreadable = sub_with_attr(FOO_ON_X, "        Q.C();\n").replace(
        "    begin\n",
        "    var\n        Q: Codeunit \"AD Q\";\n    begin\n",
    );
    each_shape(
        &readable,
        &unreadable,
        &[("p.al", PUB_XY), ("q.al", Q_ERRORS)],
        |ws, _, at| {
            let r = prove_with(
                ws,
                "AD Q::C",
                "callers-intercept-errors",
                &["--no-dependents"],
            );
            assert_eq!(r["answer"], "unknown", "{at}: {r:#}");
        },
    );
}

// ---------------------------------------------------------------------------
// The program graph splits too: one node per subscriber declaration
// ---------------------------------------------------------------------------

const Q_ERRORS: &str = r#"codeunit 50490 "AD Q"
{
    procedure C()
    begin
        Error('boom');
    end;
}
"#;

/// The `OnX` declaration, which calls `AD Q::C` (and only it does).
fn calls_q_on_x() -> String {
    sub_decl(
        "AD Pub",
        "OnX",
        "",
        "    var\n        Q: Codeunit \"AD Q\";\n",
        "        Q.C();\n",
    )
}

/// A `[TryFunction]` around raising event `raise` (`RaiseX` or `RaiseY`).
fn trap(raise: &str) -> String {
    format!(
        "codeunit 50491 \"AD Wrap\"\n{{\n    [TryFunction]\n    procedure TryIt()\n    var\n        P: Codeunit \"AD Pub\";\n    begin\n        P.{raise}();\n    end;\n\n    procedure Go()\n    begin\n        if TryIt() then;\n    end;\n}}\n"
    )
}

/// M1 (probe s5): `AD Q::C` runs only under the `OnX` declaration, and the
/// `[TryFunction]` raises `OnY`, which the other declaration subscribes to.
/// No build has a route from the trap to `C`, so nothing intercepts it. The
/// callers' walker reads the program graph, which had one node per subscriber
/// holding both subscriptions, so it found Go → TryIt → RaiseY → OnY → H → C
/// and said `yes`. The control: raising `OnX` under the trap IS an
/// interception, so the walker did not just stop walking.
#[test]
fn callers_intercept_errors_follows_only_the_declaration_that_subscribes() {
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    let calls_q = calls_q_on_x();
    // A callers' question always walks the cross-app program.
    let question = |ws: &Path, _: &[&str]| {
        prove_with(
            ws,
            "AD Q::C",
            "callers-intercept-errors",
            &["--no-dependents"],
        )
    };
    let wrong = trap("RaiseY");
    each_shape(
        &calls_q,
        &empty,
        &[("p.al", PUB_XY), ("q.al", Q_ERRORS), ("w.al", &wrong)],
        |ws, extra, at| {
            let r = question(ws, extra);
            assert_eq!(r["answer"], "no", "{at}: {r:#}");
        },
    );
    let right = trap("RaiseX");
    each_shape(
        &calls_q,
        &empty,
        &[("p.al", PUB_XY), ("q.al", Q_ERRORS), ("w.al", &right)],
        |ws, extra, at| {
            let r = question(ws, extra);
            assert_eq!(r["answer"], "yes", "control, {at}: {r:#}");
        },
    );
}

/// Inserts into `AD W`, then raises `raise`.
fn writes_then_raises(raise: &str) -> String {
    format!(
        "codeunit 50492 \"AD Writer\"\n{{\n    procedure Go()\n    var\n        W: Record \"AD W\";\n        P: Codeunit \"AD Pub\";\n    begin\n        W.Insert();\n        P.{raise}();\n    end;\n}}\n"
    )
}

/// M1's sibling: `callers-have-pending-writes` uses the same walker. The
/// write happens before raising `OnY`; `C` runs only under `OnX`.
#[test]
fn callers_have_pending_writes_follows_only_the_declaration_that_subscribes() {
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    let calls_q = calls_q_on_x();
    let question = |ws: &Path, _: &[&str]| {
        prove_with(
            ws,
            "AD Q::C",
            "callers-have-pending-writes",
            &["--no-dependents"],
        )
    };
    let wrong = writes_then_raises("RaiseY");
    each_shape(
        &calls_q,
        &empty,
        &[
            ("p.al", PUB_XY),
            ("q.al", Q_ERRORS),
            ("w.al", W_TABLE),
            ("v.al", &wrong),
        ],
        |ws, extra, at| {
            let r = question(ws, extra);
            assert_eq!(r["answer"], "no", "{at}: {r:#}");
        },
    );
    let right = writes_then_raises("RaiseX");
    each_shape(
        &calls_q,
        &empty,
        &[
            ("p.al", PUB_XY),
            ("q.al", Q_ERRORS),
            ("w.al", W_TABLE),
            ("v.al", &right),
        ],
        |ws, extra, at| {
            let r = question(ws, extra);
            assert_eq!(r["answer"], "yes", "control, {at}: {r:#}");
        },
    );
}

const SPLIT_TESTS: &str = r#"codeunit 50470 "AD Top"
{
    procedure Root()
    var
        S: Codeunit "AD Sub";
    begin
        S.H();
    end;
}

codeunit 50471 "AD Tests"
{
    Subtype = Test;

    [Test]
    procedure TestRoot()
    var
        T: Codeunit "AD Top";
    begin
        T.Root();
    end;
}
"#;

/// The two split declarations' stable ids, read from `alsem graph`.
fn split_ids(ws: &Path) -> Vec<String> {
    let out = String::from_utf8(run(ws, &["graph", "."])).unwrap();
    let mut ids: Vec<String> = out
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["t"] == "node" && v["v"]["objectName"] == "AD Sub")
        .filter_map(|v| v["v"]["stableRoutineId"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    assert_eq!(ids.len(), 2, "precondition: two declaration nodes: {ids:?}");
    assert!(ids.iter().all(|i| i.contains('~')), "{ids:?}");
    ids
}

/// M2 (probe s1): `alsem tests` could not find a split subscriber. Its `~hex`
/// id had no node in the sited graph, so the run reported "not in projection"
/// and fell back to the whole suite; a name was ambiguous and resolved
/// nothing. By name, by a declaration's id and by the routine's base id it
/// now resolves, and each declaration has its modeled test path. Both file
/// orders by the shapes.
#[test]
fn alsem_tests_finds_a_split_subscriber_by_name_and_by_id() {
    let commits = sub_decl("AD Pub", "OnX", "", "", "        Commit();\n");
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    each_shape(
        &commits,
        &empty,
        &[("p.al", PUB_XY), ("t.al", SPLIT_TESTS)],
        |ws, extra, at| {
            let tests = |selector: &str| -> Value {
                let mut args = vec!["tests", ".", "--routine", selector, "--format", "json"];
                args.extend_from_slice(extra);
                serde_json::from_slice(&run(ws, &args)).expect("json")
            };
            let check = |v: &Value, n: usize, what: &str| {
                let changed = v["changed"].as_array().expect("changed");
                assert_eq!(changed.len(), n, "{at}, {what}: {v:#}");
                for c in changed {
                    assert_eq!(
                        c["testStatus"], "tests with a modeled path",
                        "{at}, {what}: {c:#}"
                    );
                    assert_eq!(c["nearest"]["test"], "Codeunit \"AD Tests\"::TestRoot");
                }
            };
            check(&tests("AD Sub::H"), 2, "by name");
            let ids = split_ids(ws);
            check(&tests(&ids[0]), 1, "by id");
            let base = ids[0].rsplit_once('~').unwrap().0.to_string();
            check(&tests(&base), 2, "by the routine's id");
        },
    );
}

/// M2: `alsem impact` too. A selector naming a split subscriber resolves its
/// declarations (`rootsResolved`).
#[test]
fn alsem_impact_finds_a_split_subscriber_by_name_and_by_id() {
    let commits = sub_decl("AD Pub", "OnX", "", "", "        Commit();\n");
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    each_shape(
        &commits,
        &empty,
        &[("p.al", PUB_XY), ("t.al", SPLIT_TESTS)],
        |ws, extra, at| {
            let impact = |selector: &str| -> Value {
                let mut args = vec!["impact", ".", "--routine", selector, "--format", "json"];
                args.extend_from_slice(extra);
                serde_json::from_slice(&run(ws, &args)).expect("json")
            };
            let by_name = impact("AD Sub::H");
            assert_eq!(by_name["input"]["rootsResolved"], 2, "{at}: {by_name:#}");
            let ids = split_ids(ws);
            let by_id = impact(&ids[1]);
            assert_eq!(by_id["input"]["rootsResolved"], 1, "{at}: {by_id:#}");
        },
    );
}

/// M2: the callers' questions can name a split subscriber, by name and by a
/// declaration's id. `Holder::Go` writes, then calls `H` directly: both
/// declarations are callees of that call, so `H` has a caller with a pending
/// write either way.
#[test]
fn the_callers_questions_can_name_a_split_subscriber() {
    let commits = sub_decl("AD Pub", "OnX", "", "", "        Commit();\n");
    let empty = sub_decl("AD Pub", "OnY", "", "", "");
    let holder = "codeunit 50493 \"AD Holder\"\n{\n    procedure Go()\n    var\n        W: Record \"AD W\";\n        S: Codeunit \"AD Sub\";\n    begin\n        W.Insert();\n        S.H();\n    end;\n}\n";
    each_shape(
        &commits,
        &empty,
        &[("p.al", PUB_XY), ("w.al", W_TABLE), ("h.al", holder)],
        |ws, _, at| {
            let args = ["--no-dependents"];
            let by_name = prove_with(ws, "AD Sub::H", "callers-have-pending-writes", &args);
            assert_eq!(by_name["answer"], "yes", "{at}: {by_name:#}");
            let ids = split_ids(ws);
            for id in &ids {
                let r = prove_with(ws, id, "callers-have-pending-writes", &args);
                assert_eq!(r["answer"], "yes", "{at}, {id}: {r:#}");
            }
        },
    );
}

/// The callgraph contract (spec 2026-10-13, the join): a change at a
/// declaration's lines maps to a node. A split subscriber's declarations are
/// a node each, each with its own lines. An unsplit routine declared twice
/// keeps one node, which now also lists the other declaration's place
/// (`otherDeclarations`, an additive field): before, a change at the place the
/// node did not carry matched no node.
#[test]
fn the_call_graph_carries_every_declarations_lines() {
    let graph_nodes = |files: &[(&str, &str)]| -> Vec<Value> {
        let t = workspace(files);
        let out = String::from_utf8(run(t.path(), &["graph", "."])).unwrap();
        let mut nodes: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|v| v["t"] == "node" && v["v"]["objectName"] == "AD Sub")
            .map(|v| v["v"].clone())
            .collect();
        nodes.sort_by_key(|v| v["sourcePath"].as_str().unwrap().to_string());
        nodes
    };
    let a_split = format!(
        "#if A\n{}#endif\n",
        sub_decl("AD Pub", "OnX", "", "", "        Commit();\n")
    );
    let b_split = format!(
        "#if not A\n{}#endif\n",
        sub_decl("AD Pub", "OnY", "", "", "")
    );
    for (first, second) in [("a.al", "b.al"), ("b.al", "a.al")] {
        let nodes = graph_nodes(&[(first, &a_split), (second, &b_split), ("p.al", PUB_XY)]);
        assert_eq!(nodes.len(), 2, "one node per declaration: {nodes:#?}");
        let want: Vec<(String, u64, u64)> = [("a.al", 5, 8), ("b.al", 5, 7)]
            .iter()
            .map(|(f, l, e)| {
                let f = if *f == "a.al" { first } else { second };
                (format!("src/{f}"), *l, *e)
            })
            .collect();
        let mut got: Vec<(String, u64, u64)> = nodes
            .iter()
            .map(|n| {
                (
                    n["sourcePath"].as_str().unwrap().to_string(),
                    n["line"].as_u64().unwrap(),
                    n["endLine"].as_u64().unwrap(),
                )
            })
            .collect();
        got.sort();
        let mut want = want;
        want.sort();
        assert_eq!(got, want, "each node is one declaration's own place");
        assert!(
            nodes.iter().all(|n| n.get("otherDeclarations").is_none()),
            "{nodes:#?}"
        );
    }
    // The same two declarations subscribing alike stay one node.
    let a_alike = format!(
        "#if A\n{}#endif\n",
        sub_decl("AD Pub", "OnX", "", "", "        Commit();\n")
    );
    let b_alike = format!(
        "#if not A\n{}#endif\n",
        sub_decl("AD Pub", "OnX", "", "", "")
    );
    let nodes = graph_nodes(&[("a.al", &a_alike), ("b.al", &b_alike), ("p.al", PUB_XY)]);
    assert_eq!(nodes.len(), 1, "precondition: one node: {nodes:#?}");
    let n = &nodes[0];
    let mut places = vec![(
        n["sourcePath"].as_str().unwrap().to_string(),
        n["line"].as_u64().unwrap(),
    )];
    let others = n["otherDeclarations"]
        .as_array()
        .expect("otherDeclarations");
    assert_eq!(others.len(), 1, "{n:#}");
    places.push((
        others[0]["sourcePath"].as_str().unwrap().to_string(),
        others[0]["line"].as_u64().unwrap(),
    ));
    places.sort();
    assert_eq!(
        places,
        [("src/a.al".to_string(), 5), ("src/b.al".to_string(), 5)]
    );
}
