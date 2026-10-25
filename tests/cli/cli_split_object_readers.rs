//! #279: every reader of "this routine's object" reads the routine's OWN
//! declaring row. An object split across `#if` arms has two `ModelObject` rows
//! under one id (a numbered object's id is its number), each with its own name,
//! subtype, SourceTable, Editable, SingleInstance. A last-wins id -> object map
//! gives one arm's properties to the other arm's routines.
//!
//! The workspace holds ten split objects, each an `#if A … #else … #endif` with
//! an "interesting" arm and an "other" arm. Every test runs both arm orders:
//! interesting arm first ([`Order::InterestingFirst`]), then second, so a lookup
//! that picks the LAST row and one that picks the FIRST row are each caught in
//! one order. Each test asserts its arm's positive and the other arm's negative.
//!
//! Tempdir fixtures (the #241 style of `cli_object_display.rs`), not r0-corpus
//! dirs, so no golden family moves. Each order's workspace is written once per
//! test process and each command spawned once ([`fixture`]); every spawn's exit
//! code is asserted.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::Value;

const VERSION_OVERRIDE: &str = "cli-split-object-readers-v1";

#[derive(Clone, Copy, Debug)]
enum Order {
    InterestingFirst,
    InterestingSecond,
}

const ORDERS: [Order; 2] = [Order::InterestingFirst, Order::InterestingSecond];

const APP_JSON: &str = r#"{"id":"27927927-0000-0000-0000-000000000279","name":"Probe279","publisher":"probe","version":"1.0.0.0","idRanges":[{"from":50100,"to":50199}],"runtime":"14.0","dependencies":[]}"#;

const POLICY: &str = r##"version: 1
description: "#279 probe: routine-scope object fields on an #if-split object."
defaults:
  onUnknown: fail-open
  requireCoverage: any

rules:
  - id: foo-deletes
    title: "Foo deletes"
    severity: low
    when:
      object.name: "Foo"
      capability.op: delete
    facts: any

  - id: bar-deletes
    title: "Bar deletes"
    severity: low
    when:
      object.name: "Bar"
      capability.op: delete
    facts: any

  - id: install-inserts
    title: "Install codeunit inserts"
    severity: low
    when:
      root.kinds: install-codeunit
      capability.op: insert
    facts: any
"##;

/// Files that are not split.
const PLAIN_FILES: &[(&str, &str)] = &[
    (
        "src/Common.Codeunit.al",
        r#"codeunit 50180 "Probe Events"
{
    [IntegrationEvent(false, false)]
    procedure OnThing()
    begin
    end;
}

codeunit 50181 "Probe H"
{
    procedure Dirty(var R: Record "Probe Rec")
    begin
        R.Validate("No.", 'x');
    end;
}

codeunit 50182 "Probe Prod"
{
    procedure Prod()
    var
        R: Record "Probe Rec";
    begin
        R.Insert();
    end;
}

codeunit 50183 "Probe Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub A", 'OnPub', '', false, false)]
    local procedure OnPubSub()
    begin
    end;

    [EventSubscriber(ObjectType::Table, Database::"Probe Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnProbeInsert(var Rec: Record "Probe Rec"; RunTrigger: Boolean)
    begin
    end;
}
"#,
    ),
    (
        "src/T.Table.al",
        r#"table 50190 "Probe Rec"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}

table 50191 "Other Rec"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}
"#,
    ),
];

/// (file, interesting arm, other arm). Each arm ends with a newline.
const SPLIT_FILES: &[(&str, &str, &str)] = &[
    (
        "src/Split.Codeunit.al",
        r#"codeunit 50100 "Foo"
{
    procedure P()
    var
        R: Record "Probe Rec";
    begin
        R.DeleteAll();
    end;
}
"#,
        r#"codeunit 50100 "Bar"
{
    procedure Q()
    var
        R: Record "Probe Rec";
    begin
        R.ModifyAll("No.", 'X');
    end;
}
"#,
    ),
    (
        "src/SplitDead.Codeunit.al",
        r#"codeunit 50101 "Live"
{
    local procedure DeadLive()
    begin
    end;
}
"#,
        r#"codeunit 50101 "Live Test"
{
    local procedure DeadInTest()
    begin
    end;
}
"#,
    ),
    (
        "src/SplitHandler.Codeunit.al",
        r#"codeunit 50110 "Probe T"
{
    Subtype = Test;

    [Test]
    [HandlerFunctions('H')]
    procedure T()
    begin
    end;

    [MessageHandler]
    procedure H(Msg: Text[1024])
    begin
    end;
}
"#,
        r#"codeunit 50110 "Probe T2"
{
    procedure H()
    var
        P: Codeunit "Probe Prod";
    begin
        P.Prod();
    end;
}
"#,
    ),
    (
        "src/SplitInstall.Codeunit.al",
        r#"codeunit 50103 "Probe Install"
{
    Subtype = Install;

    trigger OnInstallAppPerCompany()
    var
        R: Record "Probe Rec";
    begin
        R.Insert();
    end;
}
"#,
        r#"codeunit 50103 "Probe Plain"
{
    procedure Plain()
    var
        R: Record "Probe Rec";
    begin
        R.Insert();
    end;
}
"#,
    ),
    (
        "src/SplitPage.Page.al",
        r#"page 50105 "Probe Page"
{
    SourceTable = "Probe Rec";

    trigger OnOpenPage()
    begin
        Rec.DeleteAll();
    end;
}
"#,
        r#"page 50105 "Other Page"
{
    SourceTable = "Other Rec";

    procedure Wipe()
    begin
        Rec.DeleteAll();
    end;
}
"#,
    ),
    (
        "src/SplitPub.Codeunit.al",
        r#"codeunit 50109 "Pub A"
{
    [IntegrationEvent(false, false)]
    procedure OnPub()
    begin
    end;
}
"#,
        r#"codeunit 50109 "Pub B"
{
    procedure Nothing()
    begin
    end;
}
"#,
    ),
    (
        "src/SplitRO.Page.al",
        r#"page 50108 "Probe RO"
{
    PageType = Card;
    SourceTable = "Probe Rec";
    Editable = false;
    layout { area(Content) { field(No; Rec."No.") { } } }
    actions
    {
        area(Processing)
        {
            action(A1)
            {
                trigger OnAction()
                var
                    H: Codeunit "Probe H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}
"#,
        r#"page 50108 "Probe Edit"
{
    PageType = Card;
    SourceTable = "Probe Rec";
    layout { area(Content) { field(No; Rec."No.") { } } }

    procedure Touch()
    var
        H: Codeunit "Probe H";
    begin
        H.Dirty(Rec);
    end;
}
"#,
    ),
    (
        "src/SplitSI.Codeunit.al",
        r#"codeunit 50107 "SI Keep"
{
    SingleInstance = true;

    var
        Seen: List of [Text];

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Probe Events", 'OnThing', '', false, false)]
    local procedure KeepSub()
    begin
        Seen.Add('x');
    end;
}
"#,
        r#"codeunit 50107 "SI Plain"
{
    var
        Names: List of [Text];

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Probe Events", 'OnThing', '', false, false)]
    local procedure PlainSub()
    begin
        Names.Add('x');
    end;
}
"#,
    ),
    (
        "src/SplitSubtype.Codeunit.al",
        r#"codeunit 50102 "Probe Tests"
{
    Subtype = Test;

    procedure Helper()
    var
        R: Record "Probe Rec";
    begin
        R.DeleteAll();
    end;
}
"#,
        r#"codeunit 50102 "Probe Main"
{
    procedure Main()
    var
        R: Record "Probe Rec";
    begin
        R.Insert();
    end;
}
"#,
    ),
    (
        "src/SplitUpgrade.Codeunit.al",
        r#"codeunit 50106 "Probe Upgrade"
{
    Subtype = Upgrade;

    procedure UpgradeRows()
    var
        R: Record "Probe Rec";
    begin
        if R.FindSet() then
            repeat
                R."No." := 'U';
                R.Modify();
            until R.Next() = 0;
    end;
}
"#,
        r#"codeunit 50106 "Probe Batch"
{
    procedure Rewrite()
    var
        R: Record "Probe Rec";
    begin
        if R.FindSet() then
            repeat
                R."No." := 'X';
                R.Modify();
            until R.Next() = 0;
    end;
}
"#,
    ),
];

/// The text of split file `file` in `order`.
fn split_text(file: &str, order: Order) -> String {
    let (_, interesting, other) = SPLIT_FILES
        .iter()
        .find(|(f, _, _)| *f == file)
        .unwrap_or_else(|| panic!("no split file {file}"));
    let (first, second) = match order {
        Order::InterestingFirst => (interesting, other),
        Order::InterestingSecond => (other, interesting),
    };
    format!("#if A\n{first}#else\n{second}#endif\n")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The whole workspace in `order`, in a fresh tempdir.
fn workspace(order: Order) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(&ws.join("app.json"), APP_JSON);
    write(&ws.join("al-sem.policy.yaml"), POLICY);
    for (file, text) in PLAIN_FILES {
        write(&ws.join(file), text);
    }
    for (file, _, _) in SPLIT_FILES {
        write(&ws.join(file), &split_text(file, order));
    }
    dir
}

/// The 0-based line of the one line of `file` (in `order`) that contains `needle`.
fn line_of(file: &str, order: Order, needle: &str) -> u64 {
    let text = split_text(file, order);
    let hits: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "precondition: one `{needle}` in {file}");
    hits[0] as u64
}

/// One spawn's stdout, stderr and exit code.
#[derive(Clone)]
struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// One arm order's workspace, written once per test process, and every command
/// run on it, each spawned once (the tests share them under `cargo test`).
struct Fixture {
    dir: tempfile::TempDir,
    runs: Mutex<HashMap<Vec<String>, Run>>,
}

fn fixture(order: Order) -> &'static Fixture {
    static FIRST: OnceLock<Fixture> = OnceLock::new();
    static SECOND: OnceLock<Fixture> = OnceLock::new();
    let cell = match order {
        Order::InterestingFirst => &FIRST,
        Order::InterestingSecond => &SECOND,
    };
    cell.get_or_init(|| Fixture {
        dir: workspace(order),
        runs: Mutex::default(),
    })
}

/// `alsem <args>` on `order`'s workspace (`{ws}` in an arg is its path), spawned
/// once per process: its stdout, after asserting the exit code is `code`.
fn alsem_raw(order: Order, args: &[&str], code: i32) -> String {
    let fx = fixture(order);
    let ws = fx.dir.path().to_str().unwrap();
    let args: Vec<String> = args.iter().map(|a| a.replace("{ws}", ws)).collect();
    let mut runs = fx.runs.lock().unwrap_or_else(PoisonError::into_inner);
    let run = runs
        .entry(args.clone())
        .or_insert_with(|| {
            let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
                .args(&args)
                .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
                .output()
                .expect("spawn alsem");
            Run {
                stdout: String::from_utf8_lossy(&out.stdout).replace('\r', ""),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                code: out.status.code(),
            }
        })
        .clone();
    drop(runs);
    assert_eq!(
        run.code,
        Some(code),
        "exit code of {args:?} in {order:?}: stderr={}",
        run.stderr
    );
    run.stdout
}

/// `alsem <args>` on `order`'s workspace -> stdout parsed as JSON, after
/// asserting the exit code is `code`.
fn alsem(order: Order, args: &[&str], code: i32) -> Value {
    let stdout = alsem_raw(order, args, code);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{e}: args={args:?} in {order:?} stdout={stdout}"))
}

/// `policy check` with the fixture's policy (exit 0: the rules are `low` and
/// nothing gates): every finding of rule `rule` as `(file, startLine)`.
fn policy_hits(order: Order, rule: &str) -> Vec<(String, u64)> {
    let doc = alsem(
        order,
        &[
            "policy",
            "check",
            "{ws}",
            "--policy",
            "{ws}/al-sem.policy.yaml",
            "--format",
            "json",
            "--deterministic",
        ],
        0,
    );
    let detector = format!("policy-{rule}");
    let mut hits: Vec<(String, u64)> = doc["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {doc}"))
        .iter()
        .filter(|f| f["detector"] == detector.as_str())
        .map(|f| {
            let loc = &f["primaryLocation"];
            (
                loc["sourceUnitId"].as_str().unwrap().to_string(),
                loc["range"]["startLine"].as_u64().unwrap(),
            )
        })
        .collect();
    hits.sort();
    hits
}

/// `analyze --format json` (exit 0): every finding of the detector whose id
/// starts with `prefix` as `(routineName, objectName, rootCause)`.
fn analyze_hits(order: Order, prefix: &str) -> Vec<(String, String, String)> {
    let doc = alsem(
        order,
        &["analyze", "{ws}", "--format", "json", "--deterministic"],
        0,
    );
    let mut hits: Vec<(String, String, String)> = doc["payload"]["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {doc}"))
        .iter()
        .filter(|f| f["detector"].as_str().unwrap().starts_with(prefix))
        .map(|f| {
            let loc = &f["primaryLocation"];
            (
                loc["routineName"].as_str().unwrap().to_string(),
                loc["objectName"].as_str().unwrap().to_string(),
                f["rootCause"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    hits.sort();
    hits
}

/// The one `(routineName, objectName)` the detector `prefix` reports, in both
/// orders: `routine` in `object`, and nothing on the other arm.
fn assert_one_finding(prefix: &str, routine: &str, object: &str) {
    for order in ORDERS {
        let got: Vec<(String, String)> = analyze_hits(order, prefix)
            .into_iter()
            .map(|(r, o, _)| (r, o))
            .collect();
        assert_eq!(
            got,
            vec![(routine.to_string(), object.to_string())],
            "{prefix} in {order:?}"
        );
    }
}

/// `digest --routine <routine> --tests` (exit 0): the payload's `testCoverage`.
fn test_coverage(order: Order, routine: &str) -> Value {
    let doc = alsem(
        order,
        &[
            "digest",
            "{ws}",
            "--routine",
            routine,
            "--tests",
            "--format",
            "json",
            "--deterministic",
        ],
        0,
    );
    let tc = doc["payload"]["testCoverage"].clone();
    assert!(tc.is_object(), "no testCoverage: {doc}");
    tc
}

/// R1. Policy `object.name` is the routine's own arm: `foo-deletes` matches P
/// (in `Foo`, deletes) and only P; `bar-deletes` matches nothing.
///
/// Discrimination: `object_of` back to the id -> object map (last row) ->
/// interesting-first `bar-deletes` matches P; a first-row pick ->
/// interesting-second `bar-deletes` matches P.
#[test]
fn r1_policy_object_name_is_the_declaring_arm() {
    for order in ORDERS {
        let p = line_of("src/Split.Codeunit.al", order, "procedure P()");
        assert_eq!(
            policy_hits(order, "foo-deletes"),
            vec![("ws:src/Split.Codeunit.al".to_string(), p)],
            "foo-deletes in {order:?}"
        );
        assert_eq!(
            policy_hits(order, "bar-deletes"),
            Vec::<(String, u64)>::new(),
            "bar-deletes in {order:?}"
        );
    }
}

/// R2. Policy `root.kinds` comes from the routine's own arm: exactly one
/// `install-inserts`, on the Install arm's `OnInstallAppPerCompany`; none on
/// the plain arm's `Plain`.
///
/// Discrimination: `classify_roots` back to the id -> object map ->
/// interesting-first no match, interesting-second an extra match on `Plain`.
#[test]
fn r2_policy_root_kinds_are_the_declaring_arms() {
    for order in ORDERS {
        let line = line_of(
            "src/SplitInstall.Codeunit.al",
            order,
            "trigger OnInstallAppPerCompany()",
        );
        assert_eq!(
            policy_hits(order, "install-inserts"),
            vec![("ws:src/SplitInstall.Codeunit.al".to_string(), line)],
            "install-inserts in {order:?}"
        );
    }
}

/// R3. d14 reads the routine's own arm at both sites: `DeadLive` in `Live` is
/// dead code; `DeadInTest` in `Live Test` is test code, never flagged; the
/// root cause names `Live`, not `Live Test`.
///
/// Discrimination: `classify_routine`'s object lookup (`d14.rs`, the test-name
/// check) to the last row -> interesting-first no finding; to the first row ->
/// an extra `DeadInTest`. The message lookup alone to the last row ->
/// interesting-first root cause `DeadLive on Live Test …`.
#[test]
fn r3_d14_reads_the_declaring_arm() {
    for order in ORDERS {
        let hits = analyze_hits(order, "d14-");
        assert_eq!(hits.len(), 1, "one d14 in {order:?}: {hits:?}");
        let (routine, object, root_cause) = &hits[0];
        assert_eq!(
            (routine.as_str(), object.as_str()),
            ("DeadLive", "Live"),
            "{order:?}"
        );
        assert!(
            root_cause.starts_with("DeadLive on Live is not called"),
            "{order:?}: {root_cause}"
        );
    }
}

/// R4. The Test-subtype check reads the routine's own arm: `Helper` (in the
/// `Subtype = Test` arm) is excluded test code, `Main` (normal arm) is a
/// changed routine.
///
/// Discrimination: `test_coverage`'s Test-subtype check back to the id ->
/// object map -> interesting-first `Helper` reads 0/1; a first-row pick ->
/// interesting-first `Main` reads 1/0.
#[test]
fn r4_test_subtype_is_the_declaring_arms() {
    for order in ORDERS {
        let helper = test_coverage(order, "Helper");
        assert_eq!(
            (&helper["excludedTestCode"], &helper["changedRoutines"]),
            (&Value::from(1), &Value::from(0)),
            "Helper in {order:?}: {helper}"
        );
        let main = test_coverage(order, "Main");
        assert_eq!(
            (&main["excludedTestCode"], &main["changedRoutines"]),
            (&Value::from(0), &Value::from(1)),
            "Main in {order:?}: {main}"
        );
    }
}

/// R5. The implicit `Rec` binds through the routine's own arm's SourceTable:
/// `OnOpenPage` deletes from `Probe Rec`, `Wipe` from `Other Rec`.
///
/// Discrimination: `workspace.rs resolve` back to the id -> object map ->
/// interesting-first `OnOpenPage` reads `Other Rec`; a first-row pick ->
/// `Wipe` reads `Probe Rec`.
#[test]
fn r5_implicit_rec_binds_the_declaring_arms_source_table() {
    for order in ORDERS {
        for (routine, table) in [("OnOpenPage", "Probe Rec"), ("Wipe", "Other Rec")] {
            let doc = alsem(
                order,
                &[
                    "query",
                    "effects",
                    "{ws}",
                    "--routine",
                    routine,
                    "--format",
                    "json",
                    "--deterministic",
                ],
                0,
            );
            let tables: Vec<&str> = doc["payload"]["effects"]
                .as_array()
                .unwrap_or_else(|| panic!("no effects: {doc}"))
                .iter()
                .map(|e| e["tableName"].as_str().unwrap())
                .collect();
            assert_eq!(tables, vec![table], "{routine} in {order:?}: {doc}");
        }
    }
}

/// R6. d60 fires only in the Upgrade arm: one finding, on `UpgradeRows` in
/// `Probe Upgrade`; none on the normal arm's `Rewrite` (same loop).
///
/// Discrimination: the lifecycle membership back to "any row of the id" ->
/// an extra `Rewrite` in both orders; to the last row -> interesting-first no
/// finding.
#[test]
fn r6_d60_reads_the_declaring_arms_subtype() {
    assert_one_finding("d60-", "UpgradeRows", "Probe Upgrade");
}

/// R7. d57 fires only in the SingleInstance arm: one finding, on `KeepSub` in
/// `SI Keep`; none on the plain arm's `PlainSub` (same growing global).
///
/// Discrimination: the SingleInstance membership back to "any row of the id"
/// -> an extra `PlainSub` in both orders.
#[test]
fn r7_d57_reads_the_declaring_arms_single_instance() {
    assert_one_finding("d57-", "KeepSub", "SI Keep");
}

/// R8. d39's `page_may_modify` reads the routine's own page: one finding, on
/// `OnAction` in the read-only `Probe RO`; none on `Touch` in the editable
/// `Probe Edit`.
///
/// Discrimination: `page_may_modify`'s lookup to the last row ->
/// interesting-first `OnAction` lost, interesting-second `Touch` reported.
#[test]
fn r8_d39_page_may_modify_reads_the_declaring_page() {
    assert_one_finding("d39-", "OnAction", "Probe RO");
}

/// Every `<title>` of `analyze --format html`'s event-graph nodes (exit 0).
fn html_titles(order: Order) -> Vec<String> {
    let html = alsem_raw(
        order,
        &["analyze", "{ws}", "--format", "html", "--deterministic"],
        0,
    );
    let titles: Vec<String> = html
        .split("<title>")
        .skip(1)
        .filter_map(|s| s.split_once("</title>").map(|(t, _)| t.to_string()))
        .collect();
    assert!(titles.len() > 1, "no node titles in {order:?}");
    titles
}

/// R9. The HTML report's event graph labels the publisher by its routine's
/// own arm (`Pub A`, never `Pub B`), each SingleInstance subscriber by its own
/// arm, and a table event (no publisher routine) by the object row
/// (`Probe Rec`, the fallback).
///
/// Discrimination: `routine_label` / `pub_label` back to the id -> object map
/// -> interesting-first `Pub B` and `SI Plain :: KeepSub`; the object fallback
/// removed -> the node reads `…/Table/50190` instead of `Probe Rec`.
#[test]
fn r9_html_labels_name_the_declaring_arm() {
    for order in ORDERS {
        let titles = html_titles(order);
        for want in [
            "Pub A",
            "SI Keep :: KeepSub",
            "SI Plain :: PlainSub",
            "Probe Rec",
        ] {
            assert!(
                titles.iter().any(|t| t == want),
                "`{want}` in {order:?}: {titles:?}"
            );
        }
        for never in ["Pub B", "SI Plain :: KeepSub", "SI Keep :: PlainSub"] {
            assert!(
                !titles.iter().any(|t| t == never),
                "no `{never}` in {order:?}: {titles:?}"
            );
        }
        assert!(
            !titles.iter().any(|t| t.contains("/Table/")),
            "no raw table id in {order:?}: {titles:?}"
        );
    }
}

/// R10. `[HandlerFunctions('H')]` names a handler in the test's OWN arm: the
/// other arm's `H` (which calls `Prod`) is not this test's handler, so `Prod`
/// stays uncovered with `no-path`.
///
/// Discrimination: the handler key back to the object id alone -> `Prod`
/// covered in both orders.
#[test]
fn r10_handler_functions_resolve_in_the_declaring_arm() {
    for order in ORDERS {
        let tc = test_coverage(order, "Prod");
        assert_eq!(tc["covered"], Value::Array(vec![]), "{order:?}: {tc}");
        let uncovered: Vec<(&str, &str)> = tc["uncovered"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| {
                (
                    u["display"].as_str().unwrap(),
                    u["reason"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(uncovered, vec![("Prod", "no-path")], "{order:?}: {tc}");
    }
}
