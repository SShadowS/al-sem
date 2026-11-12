//! Engine limits B7/L23 + L30: the forward error answers apply traps.
//!
//! An error behind a trap does not leave the routine: a checked
//! `Codeunit.Run` (`if Codeunit.Run(…)`, `Ok := Codeunit.Run(…)`,
//! `exit(Codeunit.Run(…))`), a `[TryFunction]` whose result is used, a call
//! under `asserterror`. Before, `prove … throws-error` answered `yes` for
//! each of them, with or without `--match`, and explain and digest listed the
//! error as the routine's own. An isolated event's subscriber is caught too,
//! but only while no write is pending (Microsoft Learn, "Isolated events in
//! AL"): its error stays a `yes` and says so. A route trapped on some paths
//! and not others escapes, and says which trap the other route crosses.
//! `[ErrorBehavior(ErrorBehavior::Collect)]` collects a collectible error and
//! raises it when the collecting procedure ends: it stays a `yes` and says so.
//!
//! Every routine of one temp workspace is asked through the real binary; the
//! unchecked twins are the controls (each trap's precondition is stated by
//! its own source shape, not by a helper).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50301 "B7 Thrower"
{
    trigger OnRun()
    begin
        Error('thrower failed');
    end;

    procedure Fail()
    begin
        Error('direct failed');
    end;
}

codeunit 50302 "B7 Callers"
{
    procedure CheckedRun(): Boolean
    begin
        if not Codeunit.Run(Codeunit::"B7 Thrower") then
            exit(false);
        exit(true);
    end;

    procedure AssignedRun(): Boolean
    var
        Ok: Boolean;
    begin
        Ok := Codeunit.Run(Codeunit::"B7 Thrower");
        exit(Ok);
    end;

    procedure ExitRun(): Boolean
    begin
        exit(Codeunit.Run(Codeunit::"B7 Thrower"));
    end;

    procedure CheckedVariableRun(): Boolean
    var
        T: Codeunit "B7 Thrower";
    begin
        exit(T.Run());
    end;

    procedure UncheckedRun()
    begin
        Codeunit.Run(Codeunit::"B7 Thrower");
    end;

    procedure UncheckedVariableRun()
    var
        T: Codeunit "B7 Thrower";
    begin
        T.Run();
    end;

    procedure CheckedTry(): Boolean
    begin
        if not TryIt() then
            exit(false);
        exit(true);
    end;

    procedure UncheckedTry()
    begin
        TryIt();
    end;

    [TryFunction]
    local procedure TryIt()
    var
        T: Codeunit "B7 Thrower";
    begin
        T.Fail();
    end;

    procedure UnderAsserterror()
    var
        T: Codeunit "B7 Thrower";
    begin
        asserterror T.Fail();
    end;

    // One route trapped, one not: the error escapes.
    procedure Both()
    var
        T: Codeunit "B7 Thrower";
    begin
        if not Codeunit.Run(Codeunit::"B7 Thrower") then
            exit;
        T.Run();
    end;

    [ErrorBehavior(ErrorBehavior::Collect)]
    procedure Collecting()
    begin
        Raise();
    end;

    local procedure Raise()
    var
        Info: ErrorInfo;
    begin
        Info.Message('collect failed');
        Info.Collectible(true);
        Error(Info);
    end;
}

codeunit 50303 "B7 Iso Publisher"
{
    procedure DoIsoWork()
    begin
        OnIsoEvent();
    end;

    [IntegrationEvent(false, false, true)]
    local procedure OnIsoEvent()
    begin
    end;
}

codeunit 50304 "B7 Iso Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"B7 Iso Publisher", 'OnIsoEvent', '', false, false)]
    local procedure HandleIso()
    begin
        Error('iso failed');
    end;
}

table 50300 "B7 Rec"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; V; Integer) { }
    }
    keys
    {
        key(PK; "No.") { }
    }
}

codeunit 50305 "B7 More"
{
    // B7 review: a TryFunction traps only in the shapes Learn names.
    procedure TryAsArgument()
    begin
        Use(TryIt2());
    end;

    procedure TryInExit(): Boolean
    begin
        exit(TryIt2());
    end;

    procedure TryAssigned(): Boolean
    var
        Ok: Boolean;
    begin
        Ok := TryIt2();
        exit(Ok);
    end;

    procedure TryInWhile()
    begin
        while not TryIt2() do
            exit;
    end;

    local procedure Use(B: Boolean)
    begin
    end;

    [TryFunction]
    local procedure TryIt2()
    begin
        Error('try2 failed');
    end;

    // B7 review: a checked run inside an open write transaction is itself
    // a run-time error.
    procedure WriteThenRun()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    procedure WriteCommitRun()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    procedure TempWriteThenRun()
    var
        R: Record "B7 Rec" temporary;
    begin
        R.ModifyAll(V, 1);
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    procedure CalleeWritesThenRun()
    begin
        DoWrite();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    procedure WriteThenHelperRuns()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        HelperRuns();
    end;

    procedure HelperRuns()
    begin
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    local procedure DoWrite()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
    end;

    // B7 review: the trap-free route goes through deeper callees.
    procedure DeepBoth()
    begin
        if not Codeunit.Run(Codeunit::"B7 Thrower") then
            exit;
        Middle();
    end;

    local procedure Middle()
    begin
        Deeper();
    end;

    local procedure Deeper()
    var
        T: Codeunit "B7 Thrower";
    begin
        T.Run();
    end;

    // B7 review: a Commit before the run in the same loop iteration ends
    // the writes of earlier iterations too (CDO's SendQueue shape).
    procedure LoopCommitRun()
    var
        R: Record "B7 Rec";
        I: Integer;
    begin
        for I := 1 to 2 do begin
            R.ModifyAll(V, I);
            Commit();
            if Codeunit.Run(Codeunit::"B7 Thrower") then;
        end;
    end;

    // B7 review: a caught error beside one that escapes.
    procedure RunAndFail()
    begin
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
        Error('other failed');
    end;
}

codeunit 50306 "B7 Commit Scopes"
{
    // B7 review round 2 (B4 x B7): under Ignore the Commit() is ignored, so
    // the write is still pending at the checked run.
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoredCommitRun()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    // Under Error the Commit() raises an error and commits nothing.
    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorCommitRun()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    // The Ignore scope is set by the caller.
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreOuter()
    begin
        CommitThenRun();
    end;

    // Asked on its own: entered normally, its Commit() ends the write.
    procedure CommitThenRun()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
    end;

    // As CommitThenRun, with an error that escapes beside the caught one:
    // the unfiltered answer is `yes` and states nothing.
    procedure CommitRunAndFail()
    var
        R: Record "B7 Rec";
    begin
        R.ModifyAll(V, 1);
        Commit();
        if Codeunit.Run(Codeunit::"B7 Thrower") then;
        Error('other failed');
    end;
}
"#;

fn run(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    out.stdout
}

fn json(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(cwd, args);
    serde_json::from_slice(&out)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out)))
}

fn human(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(run(cwd, args)).unwrap()
}

struct Ws {
    _t: tempfile::TempDir,
    root: std::path::PathBuf,
    ids: std::collections::HashMap<String, String>,
}

impl Ws {
    fn new() -> Ws {
        let t = tempfile::tempdir().unwrap();
        let ws = t.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            r#"{"id":"b7b7b7b7-0000-4000-8000-000000000023","name":"B7","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(ws.join("src/Traps.al"), SOURCE).unwrap();
        let inv = json(
            t.path(),
            &["fingerprint", "ws", "--inventory-only", "--format", "json"],
        );
        let ids = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["routineName"].as_str().unwrap().to_string(),
                    r["stableRoutineId"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let root = t.path().to_path_buf();
        Ws { _t: t, root, ids }
    }

    fn id(&self, name: &str) -> &str {
        self.ids
            .get(name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
    }

    fn prove(&self, name: &str, extra: &[&str]) -> serde_json::Value {
        let mut args = vec![
            "prove",
            "ws",
            self.id(name),
            "throws-error",
            "--deterministic",
        ];
        args.extend_from_slice(extra);
        json(&self.root, &args)["payload"].clone()
    }
}

/// The error rows of an answer's evidence, as (type, trap verdict).
fn error_rows(result: &serde_json::Value) -> Vec<(String, Option<String>)> {
    result["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| matches!(e["type"].as_str(), Some("ERROR_THROW" | "ERROR_IMPLICIT")))
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["trap"]["verdict"].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[test]
fn an_error_behind_a_definite_trap_does_not_escape() {
    let ws = Ws::new();
    // Each trap kind, unfiltered and with `--match`: `no`, with the caught
    // site named (the trap's kind and place).
    for (routine, kind) in [
        ("CheckedRun", "checked-run"),
        ("AssignedRun", "checked-run"),
        ("ExitRun", "checked-run"),
        ("CheckedVariableRun", "checked-run"),
        ("CheckedTry", "try-function"),
        ("UnderAsserterror", "asserterror"),
    ] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "no", "{routine}: {}", p["result"]);
        let p = ws.prove(routine, &["--match", "failed"]);
        assert_eq!(
            p["result"]["answer"], "no",
            "{routine} --match: {}",
            p["result"]
        );
        let caught = p["slice"]["caught"].as_array().unwrap_or_else(|| {
            panic!("{routine}: the slice names the caught site: {}", p["slice"])
        });
        // ENG-22: a clean checked run's own error is left out too, named by
        // its own kind; the callee's error is the other one.
        let (own, caught): (Vec<&serde_json::Value>, Vec<&serde_json::Value>) = caught
            .iter()
            .partition(|c| c["trap"]["kind"] == "checked-run-pending-write");
        assert_eq!(
            own.len(),
            usize::from(kind == "checked-run"),
            "{routine}: {own:?}"
        );
        assert_eq!(caught.len(), 1, "{routine}: {caught:?}");
        assert_eq!(caught[0]["trap"]["kind"], kind, "{routine}: {caught:?}");
        let h = human(
            &ws.root,
            &[
                "prove",
                "ws",
                ws.id(routine),
                "throws-error",
                "--format",
                "human",
            ],
        );
        assert!(
            h.contains("caught on every route by a trap: 1 error site"),
            "{routine}: {h}"
        );
    }
}

#[test]
fn an_error_with_no_trap_on_its_route_still_escapes() {
    let ws = Ws::new();
    // Controls: the unchecked twins. A TryFunction called as a statement is
    // an ordinary call (Learn: "If a try method call doesn't use the return
    // value, the try method operates like an ordinary method").
    for routine in ["UncheckedRun", "UncheckedVariableRun", "UncheckedTry"] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "yes", "{routine}: {}", p["result"]);
        let rows = error_rows(&p["result"]);
        assert!(
            !rows.is_empty() && rows.iter().all(|(_, v)| v.is_none()),
            "{routine}: escaping rows, no trap: {rows:?}"
        );
        let p = ws.prove(routine, &["--match", "failed"]);
        assert_eq!(
            p["result"]["answer"], "yes",
            "{routine} --match: {}",
            p["result"]
        );
    }
}

#[test]
fn a_route_trapped_on_some_paths_escapes_and_says_which() {
    let ws = Ws::new();
    let p = ws.prove("Both", &[]);
    assert_eq!(p["result"]["answer"], "yes", "{}", p["result"]);
    let rows = error_rows(&p["result"]);
    assert!(
        rows.iter()
            .any(|(_, v)| v.as_deref() == Some("trapped-on-some-routes")),
        "the row names the trap on its other route: {rows:?}"
    );
    // The filtered answer prints the route that crosses no trap: its call
    // site is the unchecked `T.Run()` (line 89), not the checked run (87).
    let p = ws.prove("Both", &["--match", "failed"]);
    assert_eq!(p["result"]["answer"], "yes", "{}", p["result"]);
    let site = p["result"]["evidence"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| p["slice"].clone());
    let text = site.to_string();
    assert!(
        text.contains("trapped-on-some-routes"),
        "the matching site names the trap: {text}"
    );
    let first_route_line = site["route"]["sites"][0]["line"].as_u64();
    assert_eq!(
        first_route_line,
        Some(89),
        "the printed route leaves at the unchecked T.Run(): {text}"
    );
}

#[test]
fn an_isolated_event_catches_unless_a_write_is_pending() {
    let ws = Ws::new();
    // A may-answer: the error escapes when a write is pending as the event
    // is raised (Learn), so it stays `yes`, and says the condition.
    let p = ws.prove("DoIsoWork", &[]);
    assert_eq!(p["result"]["answer"], "yes", "{}", p["result"]);
    let rows = error_rows(&p["result"]);
    assert!(
        rows.iter()
            .any(|(_, v)| v.as_deref() == Some("escapes-if-write-pending")),
        "{rows:?}"
    );
    let h = human(
        &ws.root,
        &[
            "prove",
            "ws",
            ws.id("DoIsoWork"),
            "throws-error",
            "--format",
            "human",
        ],
    );
    assert!(
        h.contains("escapes only if a write is pending when the event is raised"),
        "{h}"
    );
}

#[test]
fn error_collection_is_stated_and_the_error_kept() {
    let ws = Ws::new();
    let p = ws.prove("Collecting", &[]);
    assert_eq!(p["result"]["answer"], "yes", "{}", p["result"]);
    let collected: Vec<&serde_json::Value> = p["result"]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["collectedBy"].is_array())
        .collect();
    assert_eq!(collected.len(), 1, "{}", p["result"]);
    assert!(
        collected[0]["collectedBy"][0]
            .as_str()
            .is_some_and(|s| s.starts_with("Collecting (")),
        "{collected:?}"
    );
}

/// Errors beyond a call into dependency code are one boundary group per
/// crossing (cross-app mode): the group is caught when every route to its
/// crossing crosses a trap. CDO's `SendQueue` had 67 such groups behind its
/// checked `Codeunit.Run`.
const DEP_TRAP_AL: &str = r#"codeunit 50190 "Ws Trap Runner"
{
    trigger OnRun()
    var
        Lib: Codeunit "Dep Lib";
    begin
        Lib.Outer();
    end;
}

codeunit 50191 "Ws Trap"
{
    procedure Guarded(): Boolean
    begin
        exit(Codeunit.Run(Codeunit::"Ws Trap Runner"));
    end;

    procedure Open()
    begin
        Codeunit.Run(Codeunit::"Ws Trap Runner");
    end;
}
"#;

#[test]
fn errors_beyond_a_dependency_call_behind_a_trap_are_caught() {
    let t = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own(t.path(), DEP_TRAP_AL);
    let prove = |routine: &str| -> serde_json::Value {
        json(
            t.path(),
            &[
                "prove",
                ".",
                &format!("Codeunit \"Ws Trap\"::{routine}"),
                "throws-error",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };
    // The control states the precondition: the error is a group row beyond
    // the dependency call (the dependency's unguarded `Cust.Insert()`).
    let open = prove("Open");
    assert_eq!(open["answer"], "yes", "{open}");
    assert!(
        open["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["detail"]["boundary"].is_string()
                && matches!(e["type"].as_str(), Some("ERROR_IMPLICIT" | "ERROR_THROW"))),
        "precondition: an error row beyond the dependency call: {open}"
    );
    let guarded = prove("Guarded");
    assert_eq!(guarded["answer"], "no", "{guarded}");
}

#[test]
fn explain_and_digest_do_not_list_a_caught_error_as_the_routines_own() {
    let ws = Ws::new();
    let id = ws.id("CheckedRun");
    let explain = human(
        &ws.root,
        &["explain", "ws", "--routine", id, "--format", "human"],
    );
    assert!(
        explain.contains("errors caught on every route by a trap"),
        "{explain}"
    );
    assert!(
        !explain.contains("errors at 1 place"),
        "a caught error is no error place: {explain}"
    );
    let digest = json(
        &ws.root,
        &["digest", "ws", "--routine", id, "--format", "json"],
    );
    let entry = &digest["payload"]["entries"][0];
    // One error place (its dialog row, `UI_ERROR`, is no error place), and
    // (ENG-22) the clean checked run's own error.
    assert_eq!(entry["summary"]["caughtErrors"], 2, "{}", entry["summary"]);
    let effects = entry["effects"].as_array().unwrap();
    // ENG-09: the checked run's own commit is a live row of the routine;
    // every other row is the caught error.
    let is_commit = |e: &serde_json::Value| {
        digest["payload"]["effects"][e["effect"].as_str().unwrap()]["type"] == "COMMIT"
    };
    let commits: Vec<&serde_json::Value> = effects.iter().filter(|e| is_commit(e)).collect();
    assert!(
        commits.len() == 1 && commits[0]["status"] == "live",
        "the checked run's commit: {digest}"
    );
    let statuses: Vec<&str> = effects
        .iter()
        .filter(|e| !is_commit(e))
        .filter_map(|e| e["status"].as_str())
        .collect();
    assert!(
        !statuses.is_empty() && statuses.iter().all(|s| *s == "caught"),
        "{statuses:?}"
    );
    // The control: the unchecked twin's error is its own.
    let explain = human(
        &ws.root,
        &[
            "explain",
            "ws",
            "--routine",
            ws.id("UncheckedRun"),
            "--format",
            "human",
        ],
    );
    assert!(explain.contains("errors at 1 place"), "{explain}");
}

fn prove_human(ws: &Ws, routine: &str, extra: &[&str]) -> String {
    let mut args = vec![
        "prove",
        "ws",
        ws.id(routine),
        "throws-error",
        "--format",
        "human",
    ];
    args.extend_from_slice(extra);
    human(&ws.root, &args)
}

#[test]
fn a_tryfunction_traps_only_in_the_shapes_learn_names() {
    // Microsoft Learn names `OK :=` and a condition (`if`) as a try call;
    // its argument example (`DoWork(DoInsert(Rec))`) does not establish
    // that a try call used as an argument traps. So an argument and
    // `exit(…)` are ordinary calls: the error is reported as escaping.
    let ws = Ws::new();
    for routine in ["TryAsArgument", "TryInExit"] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "yes", "{routine}: {}", p["result"]);
        let rows = error_rows(&p["result"]);
        assert!(
            !rows.is_empty() && rows.iter().all(|(_, v)| v.is_none()),
            "{routine}: escaping rows, no trap: {rows:?}"
        );
        let p = ws.prove(routine, &["--match", "try2"]);
        assert_eq!(
            p["result"]["answer"], "yes",
            "{routine} --match: {}",
            p["result"]
        );
    }
    // The named shapes trap (assignment; a `while not …` condition).
    for routine in ["TryAssigned", "TryInWhile"] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "no", "{routine}: {}", p["result"]);
    }
}

#[test]
fn a_checked_run_with_a_write_pending_is_no_definite_trap() {
    // Learn ("Codeunit.Run", Transaction semantics): "If you're already in
    // a transaction you must commit first before calling Codeunit.Run"; a
    // checked run inside a write transaction is itself a run-time error.
    let ws = Ws::new();
    for routine in ["WriteThenRun", "CalleeWritesThenRun", "WriteThenHelperRuns"] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "yes", "{routine}: {}", p["result"]);
        let ev = p["result"]["evidence"].as_array().unwrap();
        assert!(
            ev.iter()
                .any(|e| e["trap"]["verdict"] == "escapes-if-write-pending"
                    && e["trap"]["kind"] == "checked-run"),
            "{routine}: {}",
            p["result"]
        );
        let p = ws.prove(routine, &["--match", "thrower"]);
        assert_eq!(
            p["result"]["answer"], "yes",
            "{routine} --match: {}",
            p["result"]
        );
        let h = prove_human(&ws, routine, &[]);
        assert!(
            h.contains(
                "a checked Codeunit.Run inside a write transaction is itself a run-time error"
            ),
            "{routine}: {h}"
        );
    }
    // Controls: a Commit() before the run ends the transaction, a write to a
    // temporary record is none, and the helper asked on its own has no write
    // before its run. Each is `no`, and states the callers' precondition.
    for routine in [
        "WriteCommitRun",
        "LoopCommitRun",
        "TempWriteThenRun",
        "HelperRuns",
        "CheckedRun",
    ] {
        let p = ws.prove(routine, &[]);
        assert_eq!(p["result"]["answer"], "no", "{routine}: {}", p["result"]);
        assert_eq!(
            p["result"]["obligations"]["assumesNoPendingWriteAtEntry"], true,
            "{routine}: {}",
            p["result"]
        );
        let h = prove_human(&ws, routine, &[]);
        assert!(
            h.contains("assumes the routine's callers have no write pending when they call it"),
            "{routine}: {h}"
        );
        assert!(
            h.contains("no error site escapes the routine; 1 error site caught on every route"),
            "{routine}: {h}"
        );
        let h = prove_human(&ws, routine, &["--match", "thrower"]);
        assert!(
            h.contains("assumes the routine's callers have no write pending when they call it"),
            "{routine} --match: {h}"
        );
    }
    // The filtered question states it too when the unfiltered answer is a
    // `yes` (another error escapes) and so states nothing.
    let p = ws.prove("RunAndFail", &[]);
    assert_eq!(
        p["result"]["answer"], "yes",
        "precondition: {}",
        p["result"]
    );
    let p = ws.prove("RunAndFail", &["--match", "thrower"]);
    assert_eq!(p["result"]["answer"], "no", "{}", p["result"]);
    assert_eq!(
        p["result"]["obligations"]["assumesNoPendingWriteAtEntry"], true,
        "{}",
        p["result"]
    );
    // A caught ruling that rests on no checked run states no such thing.
    let p = ws.prove("CheckedTry", &[]);
    assert_eq!(p["result"]["answer"], "no", "{}", p["result"]);
    assert!(
        p["result"]["obligations"]["assumesNoPendingWriteAtEntry"].is_null(),
        "{}",
        p["result"]
    );
}

#[test]
fn a_trap_free_route_through_deeper_callees_is_printed() {
    let ws = Ws::new();
    let p = ws.prove("DeepBoth", &[]);
    assert_eq!(p["result"]["answer"], "yes", "{}", p["result"]);
    assert!(
        error_rows(&p["result"])
            .iter()
            .any(|(_, v)| v.as_deref() == Some("trapped-on-some-routes")),
        "{}",
        p["result"]
    );
    let h = prove_human(&ws, "DeepBoth", &["--match", "thrower"]);
    assert!(
        h.contains("escapes on the route below (no trap on it)"),
        "{h}"
    );
    assert!(h.contains("Deeper"), "the route goes through Deeper: {h}");
}

#[test]
fn a_commit_that_commitbehavior_stops_does_not_end_the_write() {
    // B7 review round 2 (B4 x B7). Microsoft Learn, "CommitBehavior
    // attribute": under `Ignore` "this commit is ignored", under `Error`
    // "this commit causes an error"; either way the write before it is still
    // pending at the checked run, which is then itself a run-time error
    // ("Codeunit.Run", Transaction semantics). Precondition by source shape:
    // each routine writes, commits, then runs the thrower checked.
    let ws = Ws::new();
    for routine in ["IgnoredCommitRun", "ErrorCommitRun", "IgnoreOuter"] {
        for extra in [&[][..], &["--match", "thrower"][..]] {
            let p = ws.prove(routine, extra);
            assert_eq!(
                p["result"]["answer"], "yes",
                "{routine} {extra:?}: {}",
                p["result"]
            );
        }
        let p = ws.prove(routine, &[]);
        assert!(
            p["result"]["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["trap"]["verdict"] == "escapes-if-write-pending"
                    && e["trap"]["kind"] == "checked-run"),
            "{routine}: {}",
            p["result"]
        );
        // explain and digest agree: the error is not listed as caught.
        let id = ws.id(routine);
        let d = json(
            &ws.root,
            &["digest", "ws", "--routine", id, "--format", "json"],
        );
        assert!(
            d["payload"]["entries"][0]["summary"]["caughtErrors"].is_null(),
            "{routine}: {}",
            d["payload"]["entries"][0]["summary"]
        );
    }
    // The control: the same shape entered normally. Its Commit() ends the
    // write, so the error is caught, and the answer states that it rests on
    // the callers calling it under no such behavior.
    let p = ws.prove("CommitThenRun", &[]);
    assert_eq!(p["result"]["answer"], "no", "{}", p["result"]);
    let o = &p["result"]["obligations"];
    assert_eq!(o["assumesNoPendingWriteAtEntry"], true, "{o}");
    assert_eq!(o["assumesNoCommitBehaviorAtEntry"], true, "{o}");
    let h = prove_human(&ws, "CommitThenRun", &["--match", "thrower"]);
    assert!(
        h.contains("do not call it under [CommitBehavior(Ignore)] or [CommitBehavior(Error)]"),
        "{h}"
    );
    // The filtered question states it on its own: the unfiltered answer is
    // a `yes` (another error escapes) and states nothing.
    let p = ws.prove("CommitRunAndFail", &[]);
    assert_eq!(
        p["result"]["answer"], "yes",
        "precondition: {}",
        p["result"]
    );
    assert!(
        p["result"]["obligations"]["assumesNoCommitBehaviorAtEntry"].is_null(),
        "precondition: {}",
        p["result"]
    );
    let p = ws.prove("CommitRunAndFail", &["--match", "thrower"]);
    assert_eq!(p["result"]["answer"], "no", "{}", p["result"]);
    assert_eq!(
        p["result"]["obligations"]["assumesNoCommitBehaviorAtEntry"], true,
        "{}",
        p["result"]
    );
    // A ruling that needs no commit states only the pending-write part.
    let p = ws.prove("HelperRuns", &[]);
    assert_eq!(p["result"]["answer"], "no", "{}", p["result"]);
    let o = &p["result"]["obligations"];
    assert_eq!(o["assumesNoPendingWriteAtEntry"], true, "{o}");
    assert!(o["assumesNoCommitBehaviorAtEntry"].is_null(), "{o}");
}

#[test]
fn explain_and_digest_state_what_a_caught_error_assumes() {
    // B7 review round 2: an error left out as caught behind a checked run
    // leaves the routine when a caller has a write pending. prove stated
    // that; explain and digest dropped the error with a definite "they do
    // not leave the routine". Now every forward error answer states it.
    let ws = Ws::new();
    let sentence = "assumes the routine's callers have no write pending when they call it";
    for (routine, commit) in [("HelperRuns", false), ("CommitThenRun", true)] {
        let id = ws.id(routine);
        for tool in ["explain", "digest"] {
            let h = human(
                &ws.root,
                &[tool, "ws", "--routine", id, "--format", "human"],
            );
            let header = h
                .lines()
                .find(|l| l.contains("assumptions:"))
                .unwrap_or_else(|| panic!("{routine} {tool}: a header: {h}"));
            assert!(header.contains(sentence), "{routine} {tool}: {header}");
            assert_eq!(
                header.contains("[CommitBehavior(Ignore)]"),
                commit,
                "{routine} {tool}: {header}"
            );
            assert!(
                h.contains(
                    "errors caught on every route by a trap (they do not leave the routine if \
                     its callers have no write pending when they call it"
                ),
                "{routine} {tool}: {h}"
            );
            let j = json(&ws.root, &[tool, "ws", "--routine", id, "--format", "json"]);
            let s = j.to_string();
            assert!(
                s.contains("\"assumesNoPendingWriteAtEntry\":true"),
                "{routine} {tool}: {s}"
            );
            assert_eq!(
                s.contains("\"assumesNoCommitBehaviorAtEntry\":true"),
                commit,
                "{routine} {tool}: {s}"
            );
            assert!(
                s.contains("\"no-pending-write-at-entry\""),
                "{routine} {tool}: {s}"
            );
        }
        let d = json(
            &ws.root,
            &["digest", "ws", "--routine", id, "--format", "json"],
        );
        let summary = &d["payload"]["entries"][0]["summary"];
        // The callee's error and (ENG-22) the clean run's own.
        assert_eq!(summary["caughtErrors"], 2, "{routine}: {summary}");
        assert_eq!(
            summary["assumesNoPendingWriteAtEntry"], true,
            "{routine}: {summary}"
        );
    }
    // A caught error behind no checked run (a used TryFunction) rests on no
    // caller: the plain wording, no such field.
    let id = ws.id("CheckedTry");
    let h = human(
        &ws.root,
        &["digest", "ws", "--routine", id, "--format", "human"],
    );
    assert!(
        h.contains("errors caught on every route by a trap (they do not leave the routine):"),
        "{h}"
    );
    assert!(!h.contains(sentence), "{h}");
    let d = json(
        &ws.root,
        &["digest", "ws", "--routine", id, "--format", "json"],
    );
    assert!(
        d["payload"]["entries"][0]["summary"]["assumesNoPendingWriteAtEntry"].is_null(),
        "{}",
        d["payload"]["entries"][0]["summary"]
    );
}
