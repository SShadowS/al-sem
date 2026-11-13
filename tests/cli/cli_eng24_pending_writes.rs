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
            D50,
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
        .filter(|f| f["detector"] == D50)
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

/// The trap search reads the same rule: a `LockTable` then a read leaves a
/// write pending at the checked run, so the run's own error is raised.
#[test]
fn the_shared_rule_drives_the_trap_search_too() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e24e24e0-0000-4000-8000-0000000000aa","name":"E24","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            t.path().to_str().unwrap(),
            "Codeunit \"E24 Ops\"::LockThenRead",
            "throws-error",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("\"checked-run-pending-write\""),
        "the checked run's own error should be among the error sites: {text}"
    );
}
