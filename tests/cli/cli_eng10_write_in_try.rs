//! ENG-10 (engine limits L23, item 6): on premises with
//! `DisableWriteInsideTryFunctions` on (the default), a database write inside
//! a try call raises an error no try call catches. Before the fix
//! `Ok := TryInsert()` answered `throws-error: no` (the Insert's error "caught
//! by the TryFunction").
//!
//! Measured on BC 28 on premises, the Linux sidecar and Windows
//! 28.0.54946.56477 (q-20261112T104134-338e9d4a, q-20261112T110129-63d6683c):
//! probe docs/superpowers/notes/2026-11-12-eng10-write-in-try-probe.md. Each
//! routine below is one of its shapes (T-number in the comment).

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 52400 "WT Row" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 52410 "WT Writer"
{
    trigger OnRun()
    var
        R: Record "WT Row";
    begin
        R.K := 9;
        R.Insert();
    end;
}
codeunit 52411 "WT Try Runner"
{
    trigger OnRun()
    var
        C: Codeunit "WT Caller";
    begin
        C.UsedTry();
    end;
}
codeunit 52420 "WT Caller"
{
    procedure UsedTry()
    var
        Ok: Boolean;
    begin
        Ok := TryInsert(); // T01
    end;
    procedure IfNotTry()
    begin
        if not TryInsert() then; // T12
    end;
    procedure StatementTry()
    begin
        TryInsert(); // T02
    end;
    procedure NestedTry()
    var
        Ok: Boolean;
    begin
        Ok := TryNested(); // T03
    end;
    procedure TempTry()
    var
        Ok: Boolean;
    begin
        Ok := TryTemp(); // T08
    end;
    procedure ReadTry()
    var
        Ok: Boolean;
    begin
        Ok := TryRead();
    end;
    procedure CheckedRunTry()
    var
        Ok: Boolean;
    begin
        Ok := TryCheckedRun(); // T10
    end;
    procedure UncheckedRunTry()
    var
        Ok: Boolean;
    begin
        Ok := TryUncheckedRun(); // T11
    end;
    procedure ArgTry()
    var
        Ok: Boolean;
    begin
        Ok := TryTakes(InsertReturning()); // T13
    end;
    procedure OuterTry()
    var
        Ok: Boolean;
    begin
        Ok := TryOuter(); // T14
    end;
    procedure AssertTry()
    var
        Ok: Boolean;
    begin
        Ok := TryAssertWrite(); // T15
    end;
    procedure ExitTryCaller()
    var
        Ok: Boolean;
    begin
        Ok := ExitTry(); // T18
    end;
    procedure AndTry()
    var
        Ok: Boolean;
    begin
        Ok := AlwaysTrue() and TryInsert(); // T20
    end;
    procedure CondTryCaller()
    var
        Ok: Boolean;
    begin
        Ok := CondTry(); // only one #if arm makes it a try function
    end;
    procedure DynTryCaller(Id: Integer)
    var
        Ok: Boolean;
    begin
        Ok := TryDynamic(Id); // unknown code inside
    end;
#if ONPREM
    [TryFunction]
#endif
    local procedure CondTry()
    var
        R: Record "WT Row";
    begin
        R.K := 7;
        R.Insert();
    end;
    [TryFunction]
    local procedure TryDynamic(Id: Integer)
    begin
        Codeunit.Run(Id);
    end;
    procedure LockTryCaller()
    var
        Ok: Boolean;
    begin
        Ok := TryLock(); // T07
    end;
    procedure CommitTryCaller()
    var
        Ok: Boolean;
    begin
        Ok := TryCommit(); // T09
    end;
    [TryFunction]
    local procedure TryLock()
    var
        R: Record "WT Row";
    begin
        R.LockTable();
    end;
    [TryFunction]
    local procedure TryCommit()
    begin
        Commit();
    end;
    procedure CheckedRunAround()
    begin
        if Codeunit.Run(Codeunit::"WT Try Runner") then; // T16
    end;
    [TryFunction]
    procedure TryInsert()
    var
        R: Record "WT Row";
    begin
        R.K := 2;
        R.Insert();
    end;
    [TryFunction]
    local procedure TryNested()
    begin
        DoInsert();
    end;
    local procedure DoInsert()
    var
        R: Record "WT Row";
    begin
        R.K := 3;
        R.Insert();
    end;
    [TryFunction]
    local procedure TryTemp()
    var
        T: Record "WT Row" temporary;
    begin
        T.K := 4;
        T.Insert();
    end;
    [TryFunction]
    local procedure TryRead()
    var
        R: Record "WT Row";
    begin
        if R.Get(1) then;
    end;
    [TryFunction]
    local procedure TryCheckedRun()
    begin
        if Codeunit.Run(Codeunit::"WT Writer") then;
    end;
    [TryFunction]
    local procedure TryUncheckedRun()
    begin
        Codeunit.Run(Codeunit::"WT Writer");
    end;
    local procedure InsertReturning(): Boolean
    var
        R: Record "WT Row";
    begin
        R.K := 5;
        exit(R.Insert());
    end;
    [TryFunction]
    local procedure TryTakes(B: Boolean)
    begin
    end;
    [TryFunction]
    local procedure TryOuter()
    begin
        if TryInsert() then;
    end;
    [TryFunction]
    local procedure TryAssertWrite()
    begin
        asserterror DoInsert();
    end;
    local procedure ExitTry(): Boolean
    begin
        exit(TryInsert());
    end;
    local procedure AlwaysTrue(): Boolean
    begin
        exit(true);
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e10e10e0-0000-4000-8000-0000000000aa","name":"ENG10","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/wt.al"), SOURCE).unwrap();
    t
}

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

fn ask(ws: &str, routine: &str, extra: &[&str]) -> Value {
    let mut args = vec!["prove", ws, routine, "throws-error"];
    args.extend_from_slice(extra);
    let v: Value = serde_json::from_str(&run(&args)).expect("json");
    v["payload"]["result"].clone()
}

/// The 1-based line of the first line holding `text`.
fn line_of(text: &str) -> u64 {
    (SOURCE.lines().position(|l| l.contains(text)).expect(text) + 1) as u64
}

/// A prove answer's refusal rows (an implicit error with resource
/// `write-in-try`), with their trap verdicts, as (line, verdict, kind).
fn refusals(r: &Value) -> Vec<(u64, String, String)> {
    let rows = r["evidence"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<(u64, String, String)> = rows
        .iter()
        .filter(|e| e["type"] == "ERROR_IMPLICIT" && e["detail"]["resourceId"] == "write-in-try")
        .map(|e| {
            (
                e["evidence"]["line"].as_u64().unwrap_or(0),
                e["trap"]["verdict"].as_str().unwrap_or("").to_string(),
                e["trap"]["kind"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

/// A write the try call may run is refused, and no try call catches it:
/// raised by the try call, conditional on the server (never unconditional).
#[test]
fn a_write_inside_a_try_call_raises_an_error_no_try_catches() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, call) in [
        ("UsedTry", "Ok := TryInsert(); // T01"),
        ("IfNotTry", "if not TryInsert() then; // T12"),
        ("NestedTry", "Ok := TryNested(); // T03"),
        ("UncheckedRunTry", "Ok := TryUncheckedRun(); // T11"),
        ("ArgTry", "Ok := TryTakes(InsertReturning()); // T13"),
        ("ExitTryCaller", "exit(TryInsert());"),
        ("AndTry", "Ok := AlwaysTrue() and TryInsert(); // T20"),
        ("CondTryCaller", "Ok := CondTry();"),
        ("DynTryCaller", "Ok := TryDynamic(Id);"),
        ("LockTryCaller", "Ok := TryLock(); // T07"),
        ("CommitTryCaller", "Ok := TryCommit(); // T09"),
    ] {
        let r = ask(ws, &format!("WT Caller::{routine}"), &[]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let rows = refusals(&r);
        let want = line_of(call);
        assert!(
            rows.iter().any(|(l, v, k)| *l == want
                && v == "escapes-if-write-refused"
                && k == "try-write-refused"),
            "{routine}: want a refusal at line {want}: {rows:?} {r}"
        );
        for e in r["evidence"].as_array().unwrap() {
            if e["detail"]["resourceId"] == "write-in-try" {
                assert!(
                    !e["conditionality"]
                        .as_str()
                        .is_some_and(|c| c.starts_with("unconditional")),
                    "{routine}: {e}"
                );
            }
        }
    }
}

/// T14: the inner try call's refusal is not caught by the outer try call.
#[test]
fn an_outer_try_call_does_not_catch_the_refusal() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "WT Caller::OuterTry", &[]);
    let inner = line_of("if TryInsert() then;");
    assert!(
        refusals(&r)
            .iter()
            .any(|(l, v, _)| *l == inner && v == "escapes-if-write-refused"),
        "{:?} {r}",
        refusals(&r)
    );
}

/// Nothing inside the try call may be refused: no refusal error (T08 a
/// temporary record, a read only, T10 a checked run, T15 asserterror), and
/// a call statement is no try call (T02).
#[test]
fn no_refusal_where_nothing_inside_may_be_refused() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["TempTry", "ReadTry", "CheckedRunTry", "AssertTry"] {
        let r = ask(ws, &format!("WT Caller::{routine}"), &[]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        let h = human(ws, &format!("WT Caller::{routine}"));
        assert!(
            h.contains("not raised: 1 try call's write-refusal error"),
            "{routine}: {h}"
        );
    }
    let r = ask(ws, "WT Caller::StatementTry", &[]);
    assert!(refusals(&r).is_empty(), "{r}");
}

/// T16: a checked run above the try call catches its refusal.
#[test]
fn a_checked_run_above_catches_the_refusal() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "WT Caller::CheckedRunAround", &[]);
    assert_eq!(r["answer"], "no", "{r}");
    // The Insert's own error and the try call's refusal, both behind the run.
    let h = human(ws, "WT Caller::CheckedRunAround");
    assert!(
        h.contains("caught on every route by a trap: 2 error sites"),
        "{h}"
    );
}

fn human(ws: &str, routine: &str) -> String {
    run(&["prove", ws, routine, "throws-error", "--format", "human"])
}

/// The `--op`-less site slice rules it the same way.
#[test]
fn the_site_slice_rules_the_refusal() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let site = format!("src/wt.al:{}", line_of("Ok := TryInsert(); // T01"));
    let r = ask(ws, "WT Caller::UsedTry", &["--site", &site]);
    assert_eq!(r["answer"], "yes", "{r}");
}
