//! ENG-22 (engine limits L28): a checked `Codeunit.Run` with a write pending
//! raises an error at the run itself. Before the fix
//! `R.ModifyAll(V, 1); if Codeunit.Run(Codeunit::"Clean") then;` answered
//! `throws-error: no (every candidate decided)`.
//!
//! Measured on the BC 28.4 Linux sidecar and confirmed on Windows BC
//! 28.0.54946.56477 by the owner (q-20261112T021007-384a506a): with an
//! `Insert` pending, `Ok := Codeunit.Run(C)` raises "An error occurred and the
//! transaction is stopped." at the run, which the checked form does not
//! catch (no `false`), whether `C` succeeds or fails; the caller's write is
//! not committed. Probe: docs/superpowers/notes/2026-11-11-eng09-checked-run-probe.md.
//!
//! The error is ruled per root: a run at which a write may be pending on
//! some route raises it (conditional: only if the write is pending); a run
//! at which none may is clean (not raised), resting on the stated
//! assumption that the root's callers have no write pending.

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 52200 "PW Row" { fields { field(1; K; Integer) { } field(2; V; Integer) { } } keys { key(PK; K) { } } }
codeunit 52210 "PW Clean"
{
    trigger OnRun()
    begin
    end;
}
codeunit 52220 "PW Caller"
{
    procedure PendingThenRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure PendingThenVarRun()
    var
        R: Record "PW Row";
        C: Codeunit "PW Clean";
        Ok: Boolean;
    begin
        R.ModifyAll(V, 1);
        Ok := C.Run();
    end;
    procedure PendingThenDynamicRun(Id: Integer)
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if Codeunit.Run(Id) then;
    end;
    procedure CommitThenRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure DynamicOnly(Id: Integer)
    begin
        if Codeunit.Run(Id) then;
    end;
    procedure DynamicInLoop(Id: Integer; N: Integer)
    var
        I: Integer;
    begin
        for I := 1 to N do
            if Codeunit.Run(Id) then;
    end;
    procedure RunOnly()
    begin
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure WriteThenCallRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        RunOnly();
    end;
    procedure LoopRun()
    var
        R: Record "PW Row";
    begin
        while Codeunit.Run(Codeunit::"PW Clean") do
            R.ModifyAll(V, 1);
    end;
    procedure LoopRunClean()
    var
        I: Integer;
    begin
        while Codeunit.Run(Codeunit::"PW Clean") do
            I += 1;
    end;
    procedure GuardedCommitThenRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if IsInWriteTransaction() then
            Commit();
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure OtherGuardThenRun(B: Boolean)
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if B then
            Commit();
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure NegatedGuardThenRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if not IsInWriteTransaction() then
            Commit();
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure LockThenRun()
    var
        R: Record "PW Row";
    begin
        R.LockTable();
        if R.FindFirst() then;
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
    procedure LoopCallsRunThenWrite()
    var
        I: Integer;
    begin
        for I := 1 to 2 do
            RunThenWrite();
    end;
    local procedure RunThenWrite()
    var
        R: Record "PW Row";
    begin
        if Codeunit.Run(Codeunit::"PW Clean") then;
        R.ModifyAll(V, 1);
    end;
    procedure WhileCallsRunThenWrite()
    begin
        while RunThenWriteTrue() do;
    end;
    local procedure RunThenWriteTrue(): Boolean
    var
        R: Record "PW Row";
    begin
        if Codeunit.Run(Codeunit::"PW Clean") then;
        R.ModifyAll(V, 1);
        exit(true);
    end;
    procedure InsertFiresSubscriber()
    var
        R: Record "PW Row";
    begin
        R.K := 7;
        R.Insert();
    end;
    procedure TryCaught()
    begin
        if TryPendingRun() then;
    end;
    [TryFunction]
    local procedure TryPendingRun()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 1);
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
}
codeunit 52230 "PW Pub"
{
    procedure RaiseIt()
    begin
        OnThing();
    end;
    [IntegrationEvent(false, false)]
    local procedure OnThing()
    begin
    end;
}
codeunit 52231 "PW Sub A"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"PW Pub", 'OnThing', '', false, false)]
    local procedure WriteIt()
    var
        R: Record "PW Row";
    begin
        R.ModifyAll(V, 2);
    end;
}
codeunit 52233 "PW Row Sub"
{
    [EventSubscriber(ObjectType::Table, Database::"PW Row", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsert(var Rec: Record "PW Row"; RunTrigger: Boolean)
    begin
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
}
codeunit 52232 "PW Sub B"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"PW Pub", 'OnThing', '', false, false)]
    local procedure RunIt()
    begin
        if Codeunit.Run(Codeunit::"PW Clean") then;
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e22e22e0-0000-4000-8000-0000000000aa","name":"ENG22","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/pw.al"), SOURCE).unwrap();
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

fn ask(ws: &str, routine: &str, question: &str, extra: &[&str]) -> Value {
    let mut args = vec!["prove", ws, routine, question];
    args.extend_from_slice(extra);
    let v: Value = serde_json::from_str(&run(&args)).expect("json");
    v["payload"]["result"].clone()
}

/// The 1-based line of the first line holding `text` after the line holding
/// `after`.
fn line_of(text: &str, after: &str) -> u64 {
    let start = SOURCE.lines().position(|l| l.contains(after)).expect(after);
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.contains(text))
        .expect(text);
    (start + at + 1) as u64
}

/// The run-error rows of a prove answer (an implicit error with resource
/// `checked-run`).
fn run_errors(r: &Value) -> Vec<&Value> {
    r["evidence"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| {
                    e["type"] == "ERROR_IMPLICIT" && e["detail"]["resourceId"] == "checked-run"
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A write pending at a checked run: the run's own error, at the run, raised
/// only if the write is pending (never unconditional). Keyword, variable and
/// dynamic forms; a run in a callee entered with the caller's write pending.
#[test]
fn a_checked_run_with_a_write_pending_raises_its_own_error() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, call, run_in) in [
        (
            "PendingThenRun",
            "Codeunit.Run(",
            "procedure PendingThenRun(",
        ),
        (
            "PendingThenVarRun",
            "C.Run()",
            "procedure PendingThenVarRun(",
        ),
        (
            "PendingThenDynamicRun",
            "Codeunit.Run(Id)",
            "procedure PendingThenDynamicRun(",
        ),
        ("WriteThenCallRun", "Codeunit.Run(", "procedure RunOnly("),
    ] {
        let r = ask(ws, &format!("PW Caller::{routine}"), "throws-error", &[]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let rows = run_errors(&r);
        assert_eq!(rows.len(), 1, "{routine}: {r}");
        let e = rows[0];
        assert_eq!(
            e["evidence"]["line"].as_u64(),
            Some(line_of(call, run_in)),
            "{routine}: {e}"
        );
        assert_eq!(
            e["trap"]["verdict"], "escapes-if-write-pending",
            "{routine}: {e}"
        );
        assert_eq!(
            e["trap"]["kind"], "checked-run-pending-write",
            "{routine}: {e}"
        );
        assert!(
            !e["conditionality"]
                .as_str()
                .is_some_and(|c| c.starts_with("unconditional")),
            "{routine}: {e}"
        );
    }
}

/// No write may be pending at the run: its error is not raised, and the `no`
/// states what it rests on (the callers' entry, and the `Commit()` that
/// ends the write).
#[test]
fn a_clean_checked_run_raises_nothing_and_says_what_it_assumes() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    // A run with no edge (a run-time id) is ruled by its call site all the
    // same: the answer is `unknown` (the target is), but the run's own error
    // is not among its rows.
    let r = ask(ws, "PW Caller::DynamicOnly", "throws-error", &[]);
    assert!(run_errors(&r).is_empty(), "DynamicOnly: {r}");
    // Review round 2: in a loop, its earlier pass leaves nothing (a checked
    // run's writes end with it), though it has no edge to say so.
    let r = ask(ws, "PW Caller::DynamicInLoop", "throws-error", &[]);
    assert!(run_errors(&r).is_empty(), "DynamicInLoop: {r}");
    for routine in ["CommitThenRun", "RunOnly", "LoopRunClean"] {
        let r = ask(ws, &format!("PW Caller::{routine}"), "throws-error", &[]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        let h = run(&[
            "prove",
            ws,
            &format!("PW Caller::{routine}"),
            "throws-error",
            "--format",
            "human",
        ]);
        assert!(
            h.contains("assumes the routine's callers have no write pending when they call it"),
            "{routine}: {h}"
        );
    }
    let human = run(&[
        "prove",
        ws,
        "PW Caller::CommitThenRun",
        "throws-error",
        "--format",
        "human",
    ]);
    assert!(
        human.contains("not raised (no write may be pending at the run)"),
        "{human}"
    );
    assert!(human.contains("[CommitBehavior(Ignore)]"), "{human}");
}

/// ENG-22 review: the pending-write search's two holes. A run in a `while`
/// condition follows an earlier pass of the body; another subscriber of the
/// same event may run before the one that runs.
#[test]
fn a_loop_body_and_a_sibling_subscriber_may_leave_a_write_pending() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["PW Caller::LoopRun", "PW Pub::RaiseIt"] {
        let r = ask(ws, routine, "throws-error", &[]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        assert_eq!(run_errors(&r).len(), 1, "{routine}: {r}");
    }
}

/// A trap outside the run still catches the run's own error: a used
/// TryFunction around it.
#[test]
fn a_try_function_around_the_run_catches_its_error() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "PW Caller::TryCaught", "throws-error", &[]);
    assert_eq!(r["answer"], "no", "{r}");
}

/// The `--site` slice and explain rule it the same way and name it.
#[test]
fn the_site_slice_and_explain_name_the_runs_error() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let line = line_of("Codeunit.Run(", "procedure PendingThenRun(");
    let site = format!("src/pw.al:{line}");
    let r = ask(
        ws,
        "PW Caller::PendingThenRun",
        "throws-error",
        &["--site", &site],
    );
    assert_eq!(r["answer"], "yes", "{r}");
    let clean = format!(
        "src/pw.al:{}",
        line_of("Codeunit.Run(", "procedure CommitThenRun(")
    );
    let r = ask(
        ws,
        "PW Caller::CommitThenRun",
        "throws-error",
        &["--site", &clean],
    );
    assert_eq!(r["answer"], "no", "{r}");
    let card = run(&["explain", ws, "--routine", "PW Caller::PendingThenRun"]);
    assert!(
        card.contains(&format!(
            "ERROR (checked Codeunit.Run with a write pending) at ws:src/pw.al:{line}"
        )),
        "{card}"
    );
}

/// The run-error rows of `routine`'s `throws-error` answer that escape (not
/// ruled "not raised").
fn escaping_run_errors(ws: &str, routine: &str) -> usize {
    let r = ask(ws, routine, "throws-error", &[]);
    run_errors(&r)
        .iter()
        .filter(|e| e["trap"]["verdict"] == "escapes-if-write-pending")
        .count()
}

/// ENG-22 (measured on the BC 28.4 sidecar: `IsInWriteTransaction()` is true
/// exactly when a checked run fails): `if IsInWriteTransaction() then
/// Commit();` ends the write on both paths, so the run after it is clean.
/// Only that shape: a commit under another condition, or under the negated
/// call, ends nothing.
#[test]
fn a_commit_guarded_by_is_in_write_transaction_ends_the_write() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "PW Caller::GuardedCommitThenRun", "throws-error", &[]);
    assert_eq!(r["answer"], "no", "{r}");
    for routine in [
        "PW Caller::OtherGuardThenRun",
        "PW Caller::NegatedGuardThenRun",
    ] {
        assert_eq!(escaping_run_errors(ws, routine), 1, "{routine}");
    }
}

/// ENG-22 (measured on the BC 28.4 sidecar): `LockTable` then a read opens a
/// write transaction, and a checked run after it fails.
#[test]
fn a_lock_table_leaves_the_transaction_open() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    assert_eq!(escaping_run_errors(ws, "PW Caller::LockThenRun"), 1);
}

/// ENG-22 review: a call in a loop runs again after its own earlier pass
/// wrote; a subscriber of an `OnAfterInsert` event runs with the insert
/// pending.
#[test]
fn a_repeated_call_and_a_table_event_leave_a_write_pending() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in [
        "PW Caller::LoopCallsRunThenWrite",
        // Review round 2: the call is the loop's own condition.
        "PW Caller::WhileCallsRunThenWrite",
        "PW Caller::InsertFiresSubscriber",
    ] {
        assert_eq!(escaping_run_errors(ws, routine), 1, "{routine}");
    }
}

/// d50 reads the same facts: the variable form `Ok := C.Run()` is a seed
/// (ENG-22 review), and a run after `if IsInWriteTransaction() then
/// Commit();` has nothing pending.
#[test]
fn d50_sees_the_variable_form_and_the_guarded_commit() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let out = run(&[
        "analyze",
        ws,
        "--detector",
        "d50-checked-run-implicit-commit",
        "--format",
        "json",
        "--scope",
        "all",
    ]);
    let v: Value = serde_json::from_str(&out).expect("json");
    let mut lines: Vec<u64> = Vec::new();
    let mut stack = vec![&v];
    while let Some(x) = stack.pop() {
        match x {
            Value::Object(m) => {
                if m.get("detector").and_then(Value::as_str)
                    == Some("d50-checked-run-implicit-commit")
                    && let Some(l) = m.get("primaryLocation").and_then(|p| p["line"].as_u64())
                {
                    lines.push(l);
                }
                stack.extend(m.values());
            }
            Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
    let var_run = line_of("C.Run()", "procedure PendingThenVarRun(");
    let guarded = line_of("Codeunit.Run(", "procedure GuardedCommitThenRun(");
    assert!(lines.contains(&var_run), "{lines:?} want {var_run}: {out}");
    assert!(!lines.contains(&guarded), "{lines:?} not {guarded}");
}
