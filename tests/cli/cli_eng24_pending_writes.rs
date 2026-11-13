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
table 53014 "E24 X"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }

    trigger OnInsert()
    var
        Missing: Codeunit "E24 Not Loaded";
    begin
        Missing.DoIt();
    end;
}
table 53015 "E24 Y"
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

    procedure InsertXThenRun(var X: Record "E24 X" temporary)
    begin
        X.K := 1;
        X.Insert(true);
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    procedure InsertY()
    var
        Y: Record "E24 Y";
    begin
        Y.K := 1;
        Y.Insert();
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

    procedure Post5()
    begin
        OnThing5();
    end;

    [IntegrationEvent(false, false)]
    procedure OnThing5()
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

    [EventSubscriber(ObjectType::Table, Database::"E24 Y", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsYRuns(var Rec: Record "E24 Y"; RunTrigger: Boolean)
    begin
        if Codeunit.Run(Codeunit::"E24 Worker2") then;
    end;

    [EventSubscriber(ObjectType::Table, Database::"E24 Y", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsYWrites(var Rec: Record "E24 Y"; RunTrigger: Boolean)
    var
        W: Record "E24 W";
    begin
        W.Get(1);
        W.Modify();
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

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing5', '', false, false)]
    local procedure Sub5Writes()
    var
        Y: Record "E24 Y";
    begin
        Y.Get(1);
        Y.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"E24 Ops2", 'OnThing5', '', false, false)]
    local procedure Sub5Commits()
    var
        T: Record "E24 T";
        W: Record "E24 W";
    begin
        T.Get(1);
        T.Modify();
        W.Get(1);
        W.Modify();
        Commit();
    end;
}
"#;

/// Table events (astra round 1, finding 1): an OnAfter subscriber runs after
/// its operation's write (pending); an OnBefore one before it (not); another
/// subscriber of the same table event may run first (pending, labeled).
/// Breaks (2026-11-13): no `written` targets in `span_template` -> `AfterInsT`
/// has no finding; `run_phase` reads OnBefore as after the write ->
/// `BeforeInsU` gets one; `add_dispatch_siblings` ignores the phase order
/// (`ph <= m || true`) -> `BeforeInsYRuns` gets one; the "unknown code
/// behind" loop skips a record operation's site -> `InsertXThenRun` has
/// none (Fable round 2). Each fails; restored, it passes.
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
    // An OnAfter subscriber runs after an OnBefore one: its write is not
    // pending there.
    assert!(cause(&fs, "BeforeInsYRuns").is_none(), "{fs:?}");
    // Unknown code in the table's trigger, behind a temporary record's
    // insert (the insert itself writes nothing).
    let x = cause(&fs, "InsertXThenRun").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(x.contains("unknown code behind X."), "{x}");
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

/// Finding 5, as the orchestrator narrowed it: a sibling's physical tables
/// reach the explicit-commit spans (`pending_order_unknown_tables`), but the
/// default d9 gates on the tables whose order is known. Sibling-only tables
/// make no finding (`Sub4Commits`); next to known ones they are named as
/// possibly also pending (`Sub5Commits`).
/// Breaks (2026-11-13): d9 gates on `pending_physical_tables.len()` ->
/// `Sub4Commits` gets a finding; drop `tables.extend` in
/// `add_dispatch_siblings` -> `Sub5Commits` loses the "possibly also pending"
/// part. Each fails; restored, it passes.
#[test]
fn a_siblings_tables_reach_d9_text_not_its_gate() {
    let fs = findings(ROUND2, "d9-transaction-span-summary");
    assert!(cause(&fs, "Sub4Commits").is_none(), "{fs:?}");
    let c = cause(&fs, "Sub5Commits").unwrap_or_else(|| panic!("{fs:?}"));
    assert!(
        c.contains(
            "writes 2 known table(s) before it (possibly also pending (order unknown): 1 more \
             in another event subscriber or trigger)"
        ),
        "{c}"
    );
}

/// The digest's transaction context of each effect of `routine` (sorted, deduped).
fn digest_contexts(source: &str, routine: &str) -> Vec<String> {
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
            "digest",
            t.path().to_str().unwrap(),
            "--routine",
            routine,
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(s) = m.get("transactionContext").and_then(|s| s.as_str()) {
                    out.insert(s.to_string());
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut set = std::collections::BTreeSet::new();
    walk(&v, &mut set);
    set.into_iter().collect()
}

/// The digest's transaction context (orchestrator ruling): a Commit whose only
/// pending tables are another subscriber's is neither "known writes" nor "no
/// known writes" (`Sub4Commits`); one with its own writes keeps "known".
/// Break (2026-11-13): drop the `TX_SPAN_MAY_WRITES` branch in `digest_cli`'s
/// `tx_context_map` -> `Sub4Commits` reads `span-has-no-known-writes`. Fails;
/// restored, it passes.
#[test]
fn the_digest_says_may_have_writes_for_order_unknown_tables() {
    let sib = digest_contexts(ROUND2, "Sub4Commits");
    assert_eq!(sib, ["span-may-have-writes-order-unknown"]);
    let own = digest_contexts(ROUND2, "Sub5Commits");
    assert!(own.iter().any(|c| c == "span-has-known-writes"), "{own:?}");
    assert!(!own.iter().any(|c| c.starts_with("span-may")), "{own:?}");
}

/// Review round 1 (gpt-6.1-sol): checked runs that are no unknown code.
const ROUND3: &str = r#"
codeunit 53020 "E24 EmptyCu"
{
    procedure Foo()
    begin
    end;
}
codeunit 53021 "E24 Worker3"
{
    trigger OnRun()
    begin
    end;
}
codeunit 53022 "E24 MissingRunner"
{
    trigger OnRun()
    var
        Missing: Codeunit "E24 Not Loaded";
    begin
        Missing.DoIt();
    end;
}
codeunit 53023 "E24 Ops3"
{
    procedure VarRunNoOnRun()
    var
        E: Codeunit "E24 EmptyCu";
    begin
        if E.Run() then;
        if Codeunit.Run(Codeunit::"E24 Worker3") then;
    end;

    procedure VarRunSymbolOnly()
    var
        S: Codeunit "E24 Sym Cu";
    begin
        if S.Run() then;
        if Codeunit.Run(Codeunit::"E24 Worker3") then;
    end;

    local procedure IsolatedUnknownHelper()
    begin
        if Codeunit.Run(Codeunit::"E24 MissingRunner") then;
    end;

    procedure ViaIsolatedUnknown()
    begin
        IsolatedUnknownHelper();
        if Codeunit.Run(Codeunit::"E24 Worker3") then;
    end;
}
"#;

/// ROUND3 in a workspace whose `"E24 Sym Cu"` comes from a dependency loaded
/// from symbols only (its `OnRun` is not listed: no edge reaches it).
fn round3_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e24e24e0-0000-4000-8000-0000000000ab","name":"E24 R3","publisher":"t","version":"1.0.0.0","dependencies":[{"id":"22222222-0000-0000-0000-0000000000e2","name":"E24 Sym","publisher":"t","version":"1.0.0.0"}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), ROUND3).unwrap();
    let symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000e2","Name":"E24 Sym","Publisher":"t","Version":"1.0.0.0","Codeunits":[{"Id":70970,"Name":"E24 Sym Cu","Methods":[{"Name":"Foo","Parameters":[]}]}]}"#;
    crate::symbol_app::write_symbol_app(
        &t.path().join(".alpackages/t_E24 Sym_1.0.0.0.app"),
        "22222222-0000-0000-0000-0000000000e2",
        "E24 Sym",
        "1.0.0.0",
        symbols,
    );
    t
}

/// `alsem <args…> <ws> …` with `--format json`, parsed.
fn run_json(ws: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut all: Vec<&str> = vec![args[0], ws.to_str().unwrap()];
    all.extend_from_slice(&args[1..]);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&all)
        .args(["--format", "json"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// A codeunit variable's checked `Run()` (no `OnRun` declared, or one of a
/// symbol-only dependency, whose run reaches no edge) is a checked run, not
/// unknown code; unknown code reachable only through a helper's checked run
/// ends with that run. None leaves a write pending at the next checked run,
/// for d50 and the trap search alike.
/// Breaks (2026-11-13): drop `cs.checked_run ||` from `traps::is_checked_run`
/// (the one predicate every reader uses, sol round 2) ->
/// `VarRunSymbolOnly` gets a d50 finding ("unknown code at S.Run"; the local
/// `VarRunNoOnRun` stays clean either way, its run resolves); drop the
/// `!is_run_edge` filter in `UnknownCode::build` -> `ViaIsolatedUnknown` gets
/// one ("unknown code behind IsolatedUnknownHelper"). Each fails; restored, it
/// passes.
#[test]
fn checked_runs_end_unknown_code() {
    let t = round3_workspace();
    let v = run_json(
        t.path(),
        &[
            "analyze",
            "--detector",
            D50,
            "--deterministic",
            "--scope",
            "all",
        ],
    );
    let fs: Vec<(String, String)> = v["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["primaryLocation"]["routineName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                f["rootCause"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    for routine in ["VarRunNoOnRun", "VarRunSymbolOnly", "ViaIsolatedUnknown"] {
        assert!(cause(&fs, routine).is_none(), "{routine}: {fs:?}");
        let p = run_json(
            t.path(),
            &[
                "prove",
                &format!("Codeunit \"E24 Ops3\"::{routine}"),
                "throws-error",
            ],
        );
        let escapes = p["payload"]["result"]["evidence"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|e| {
                e["trap"]["kind"] == "checked-run-pending-write"
                    && e["trap"]["verdict"] == "escapes-if-write-pending"
            });
        assert!(!escapes, "{routine}: {p}");
    }
}
