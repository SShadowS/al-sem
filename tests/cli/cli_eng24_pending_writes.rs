//! ENG-24: d50 (a checked `Codeunit.Run` with a write pending) read only
//! writes to a known physical table, so a write whose table does not resolve,
//! a `LockTable`, unknown code and a sibling subscriber of a raised event were
//! not pending there. Probe: docs/superpowers/notes/2026-11-13-eng24-d50-pending-probe.md.

use std::process::Command;

const D50: &str = "d50-checked-run-implicit-commit";

const SOURCE: &str = r#"
table 53000 "E24 Row"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
}
codeunit 53001 "E24 Worker"
{
    trigger OnRun()
    begin
    end;
}
codeunit 53004 "E24 Locker"
{
    trigger OnRun()
    var
        R: Record "E24 Row";
    begin
        R.LockTable();
        R.FindFirst();
    end;
}
codeunit 53002 "E24 Ops"
{
    procedure PlainWrite()
    var
        R: Record "E24 Row";
    begin
        R.Get(1);
        R.Modify();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    procedure RecRefParam(var RR: RecordRef)
    begin
        RR.Modify();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    procedure LockThenRead()
    var
        R: Record "E24 Row";
    begin
        R.LockTable();
        R.FindFirst();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    procedure UnknownThenRun()
    var
        Missing: Codeunit "E24 Not Loaded";
    begin
        Missing.DoIt();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    local procedure LockHelper()
    var
        R: Record "E24 Row";
    begin
        R.LockTable();
        R.FindFirst();
    end;

    procedure ViaHelperLock()
    begin
        LockHelper();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    local procedure IsolatedLock()
    begin
        if Codeunit.Run(Codeunit::"E24 Locker") then;
    end;

    procedure ViaIsolated()
    begin
        IsolatedLock();
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    procedure Clean()
    begin
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;

    procedure Raise()
    begin
        OnSomething();
    end;

    [IntegrationEvent(false, false)]
    procedure OnSomething()
    begin
    end;
}
codeunit 53003 "E24 Subs"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops", 'OnSomething', '', false, false)]
    local procedure WritesFirst()
    var
        R: Record "E24 Row";
    begin
        R.Get(1);
        R.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops", 'OnSomething', '', false, false)]
    local procedure RunsCheck()
    begin
        if Codeunit.Run(Codeunit::"E24 Worker") then;
    end;
}
"#;

/// d50's findings: (routine, root cause).
fn d50(source: &str) -> Vec<(String, String)> {
    findings(source, D50)
}

/// `detector`'s findings: (routine, root cause).
fn findings(source: &str, detector: &str) -> Vec<(String, String)> {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e24e24e0-0000-4000-8000-0000000000aa","name":"E24","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "analyze",
            t.path().to_str().unwrap(),
            "--detector",
            detector,
            "--format",
            "json",
            "--deterministic",
            "--scope",
            "all",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    v["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == detector)
        .map(|f| {
            (
                f["primaryLocation"]["routineName"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                f["rootCause"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn cause<'a>(fs: &'a [(String, String)], routine: &str) -> Option<&'a str> {
    fs.iter()
        .find(|(r, _)| r == routine)
        .map(|(_, c)| c.as_str())
}

/// A write whose table does not resolve, a `LockTable` (direct or in a
/// callee's cone) and unknown code are pending at a checked run, each named;
/// a lock inside the helper's own checked run is not (ENG-22); a run with
/// nothing before it is clean.
/// Breaks (2026-11-13): drop the `lock` arm of
/// `cone_derived::op_may_leave_write_pending` -> `LockThenRead` and
/// `ViaHelperLock` lose their finding HERE, and the trap search's
/// `prove … throws-error` on `LockThenRead` answers `no` (test
/// `the_shared_rule_drives_the_trap_search_too`): one rule, two readers; drop
/// the unresolved-write label (`fact_other`) -> `RecRefParam` loses it; pass
/// no unknown-code sites -> `UnknownThenRun` loses it. Each fails; restored,
/// it passes.
#[test]
fn d50_counts_every_write_the_trap_search_counts() {
    let fs = d50(SOURCE);
    let want = |r: &str, label: &str| {
        let c = cause(&fs, r).unwrap_or_else(|| panic!("{r}: no d50 finding in {fs:?}"));
        assert!(c.contains(label), "{r}: {c}");
    };
    want("PlainWrite", "(E24 Row)");
    want("RecRefParam", "a write to a table that does not resolve");
    want("LockThenRead", "a LockTable");
    want("ViaHelperLock", "a LockTable");
    want("UnknownThenRun", "unknown code at Missing.DoIt");
    assert!(cause(&fs, "ViaIsolated").is_none(), "{fs:?}");
    assert!(cause(&fs, "Clean").is_none(), "{fs:?}");
}

/// Another subscriber of an event raised on the way to the run may run first
/// (their order is not known): pending, labeled so a reader can tell it apart.
/// Break (2026-11-13): skip the sibling loop in `span_template` ->
/// `RunsCheck` has no finding. Fails; restored, it passes.
#[test]
fn a_sibling_subscriber_is_possibly_pending() {
    let fs = d50(SOURCE);
    let c = cause(&fs, "RunsCheck").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(
        c.contains("possibly pending: subscriber WritesFirst of event OnSomething (order unknown)"),
        "{c}"
    );
}

/// `prove <routine> throws-error` on `source`: (answer, the verdicts of the
/// checked runs' own errors among the evidence).
fn prove_checked_runs(source: &str, routine: &str) -> (String, Vec<String>) {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e24e24e0-0000-4000-8000-0000000000aa","name":"E24","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            t.path().to_str().unwrap(),
            routine,
            "throws-error",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let r = &v["payload"]["result"];
    let verdicts = r["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["trap"]["kind"] == "checked-run-pending-write")
        .map(|e| e["trap"]["verdict"].as_str().unwrap_or("").to_string())
        .collect();
    (r["answer"].as_str().unwrap_or("").to_string(), verdicts)
}

/// The trap search reads the same rule: a `LockTable` then a read leaves a
/// write pending at the checked run, so the run's own error escapes; with
/// nothing before the run, `no`.
/// Break (2026-11-13, review round 1): drop the `lock` arm of
/// `op_may_leave_write_pending` -> `LockThenRead`'s run is no longer
/// `escapes-if-write-pending`. Fails; restored, it passes.
#[test]
fn the_shared_rule_drives_the_trap_search_too() {
    let (_, lock) = prove_checked_runs(SOURCE, "Codeunit \"E24 Ops\"::LockThenRead");
    assert_eq!(lock, ["escapes-if-write-pending"]);
    let (answer, clean) = prove_checked_runs(SOURCE, "Codeunit \"E24 Ops\"::Clean");
    assert_eq!((answer.as_str(), clean.len()), ("no", 0));
}

/// Review round 1 (astra): the cases the first walk answered wrong.
const ROUND2: &str = r#"
table 53010 "E24 T"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 53011 "E24 U"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 53012 "E24 V"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 53013 "E24 W"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 53011 "E24 Worker2"
{
    trigger OnRun()
    begin
    end;
}
codeunit 53012 "E24 Writer"
{
    trigger OnRun()
    var
        W: Record "E24 W";
    begin
        W.Get(1);
        W.Modify();
    end;
}
codeunit 53010 "E24 Ops2"
{
    procedure InsertT()
    var
        T: Record "E24 T";
    begin
        T.K := 1;
        T.Insert();
    end;

    procedure InsertU()
    var
        U: Record "E24 U";
    begin
        U.K := 1;
        U.Insert();
    end;

    procedure InsertV()
    var
        V: Record "E24 V";
    begin
        V.K := 1;
        V.Insert();
    end;

    procedure DynThenRun(Id: Integer)
    begin
        if Codeunit.Run(Id) then;
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    local procedure CallsMissing()
    var
        Missing: Codeunit "E24 Not Loaded";
    begin
        Missing.DoIt();
    end;

    procedure ViaMissingHelper()
    begin
        CallsMissing();
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    procedure Raise2()
    begin
        OnThing2();
    end;

    procedure Raise3()
    begin
        OnThing3();
    end;

    procedure Check3()
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    procedure Post4()
    begin
        OnThing4();
    end;

    [IntegrationEvent(false, false)]
    procedure OnThing2()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnThing3()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnThing4()
    begin
    end;
}
codeunit 53013 "E24 Subs2"
{
    [EventSubscriber(ObjectType::Table, Database::"E24 T", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsT(var Rec: Record "E24 T"; RunTrigger: Boolean)
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    [EventSubscriber(ObjectType::Table, Database::"E24 U", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsU(var Rec: Record "E24 U"; RunTrigger: Boolean)
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    [EventSubscriber(ObjectType::Table, Database::"E24 V", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsVWrites(var Rec: Record "E24 V"; RunTrigger: Boolean)
    var
        W: Record "E24 W";
    begin
        W.Get(1);
        W.Modify();
    end;

    [EventSubscriber(ObjectType::Table, Database::"E24 V", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsVRuns(var Rec: Record "E24 V"; RunTrigger: Boolean)
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing2', '', false, false)]
    local procedure OnlyIsolated()
    begin
        if Codeunit.Run(Codeunit::"E24 Writer") then;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing2', '', false, false)]
    local procedure RunsCheck2()
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing3', '', false, false)]
    local procedure SubA()
    var
        W: Record "E24 W";
        Ops: Codeunit "E24 Ops2";
    begin
        Ops.Check3();
        W.Get(1);
        W.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing3', '', false, false)]
    local procedure SubB()
    var
        Ops: Codeunit "E24 Ops2";
    begin
        Ops.Check3();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing4', '', false, false)]
    local procedure Sub4Writes()
    var
        T: Record "E24 T";
        W: Record "E24 W";
    begin
        T.Get(1);
        T.Modify();
        W.Get(1);
        W.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing4', '', false, false)]
    local procedure Sub4Commits()
    begin
        Commit();
    end;
}
"#;

/// Table events (astra round 1, finding 1): an OnAfter subscriber runs after
/// its operation's write (pending); an OnBefore one before it (not); another
/// subscriber of the same table event may run first (pending, labeled).
/// Breaks (2026-11-13): no `written` targets in `span_template` -> `AfterInsT`
/// has no finding; `run_phase` reads OnBefore as after the write ->
/// `BeforeInsU` gets one. Each fails; restored, it passes.
#[test]
fn table_events_order_the_write_and_their_siblings() {
    let fs = d50(ROUND2);
    let after = cause(&fs, "AfterInsT").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(after.contains("(E24 T)"), "{after}");
    assert!(cause(&fs, "BeforeInsU").is_none(), "{fs:?}");
    let sib = cause(&fs, "BeforeInsVRuns").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(
        sib.contains(
            "E24 W (order unknown), possibly pending: subscriber BeforeInsVWrites of event \
             onbeforeinsertevent (order unknown)"
        ),
        "{sib}"
    );
}

/// Findings 2, 3, 4, 6: a dynamic checked run is not unknown code; a
/// sibling's write inside its own checked run is not pending; a sibling that
/// is itself in the span (both call `Check3`) counts what it writes after
/// its call; unknown code behind a helper counts.
/// Breaks (2026-11-13): drop `!is_checked_run` from `is_unknown_code_call` ->
/// `DynThenRun` gets a finding (and the trap search answers
/// `escapes-if-write-pending` for its second run); `sibling_pending` reads
/// the whole cone -> `RunsCheck2` gets one; skip siblings in the span ->
/// `Check3` has none; drop the "unknown code behind" loop in `effects` ->
/// `ViaMissingHelper` has none. Each fails; restored, it passes.
#[test]
fn checked_runs_siblings_in_the_span_and_unknown_code_behind_a_call() {
    let fs = d50(ROUND2);
    assert!(cause(&fs, "DynThenRun").is_none(), "{fs:?}");
    assert!(cause(&fs, "RunsCheck2").is_none(), "{fs:?}");
    let c3 = cause(&fs, "Check3").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(
        c3.contains("possibly pending: subscriber SubA of event OnThing3 (order unknown)"),
        "{c3}"
    );
    let m = cause(&fs, "ViaMissingHelper").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(m.contains("unknown code behind CallsMissing"), "{m}");
    // The trap search reads the same rule for the dynamic run.
    let (answer, runs) = prove_checked_runs(ROUND2, "Codeunit \"E24 Ops2\"::DynThenRun");
    assert!(
        !runs.iter().any(|v| v == "escapes-if-write-pending"),
        "{answer} {runs:?}"
    );
}

/// Finding 5: a sibling's physical tables reach the explicit-commit spans
/// (d9 counts them), labeled order unknown.
/// Break (2026-11-13): drop `tables.extend` in `add_dispatch_siblings` ->
/// `Sub4Commits` has no d9 finding. Fails; restored, it passes.
#[test]
fn a_siblings_tables_reach_d9_labeled() {
    let fs = findings(ROUND2, "d9-transaction-span-summary");
    let c = cause(&fs, "Sub4Commits").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(
        c.contains(
            "writes 2 known table(s) before it (2 of them only in another event subscriber \
             or trigger, which may run before it: order unknown)"
        ),
        "{c}"
    );
}
