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
table 52401 "WT Trig"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
    trigger OnInsert()
    begin
        Commit();
    end;
}
codeunit 52412 "WT Helper Runner"
{
    trigger OnRun()
    var
        C: Codeunit "WT Caller";
    begin
        C.Helper();
    end;
}
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
    procedure TwoRoutes()
    begin
        if Codeunit.Run(Codeunit::"WT Helper Runner") then;
        if TryHelper() then;
    end;
    procedure Helper()
    var
        R: Record "WT Row";
    begin
        R.K := 8;
        R.Insert();
    end;
    [TryFunction]
    local procedure TryHelper()
    begin
        HelperVia(); // one hop longer: the shortest witness takes the run
    end;
    local procedure HelperVia()
    begin
        Helper();
    end;
    procedure ArgTrigTry()
    var
        T: Record "WT Trig" temporary;
        Ok: Boolean;
    begin
        T.K := 1;
        Ok := TryTakes(T.Insert(true)); // OnInsert commits inside the try call
    end;
    procedure ArgRecTry()
    var
        R: Record "WT Row";
        Ok: Boolean;
    begin
        R.K := 6;
        Ok := TryTakes(R.Insert()); // T13, the write in the argument itself
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
/// Break (2026-11-12): skip the operation-site scan of the argument list in
/// `try_refusal_verdict` -> `ArgRecTry` answers `no`; restored, it passes.
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
        ("ArgRecTry", "Ok := TryTakes(R.Insert());"),
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

/// A filtered answer on the refused operation itself: a `Commit()` (no
/// error of its own) and a tested `Insert` in a try call's argument list
/// raise the refusal (T09, T13); an `Insert` behind a checked run inside
/// the try call does not (T10); a try call where nothing may be refused is
/// "not raised", not "caught".
/// Breaks (2026-11-12): drop the added sites for refused operations with no
/// error of their own -> `--op Commit` and `ArgRecTry --op Insert` answer
/// `no`; restore the first-trap override (any `Caught(TryFunction)` on a
/// refused operation escapes) -> `CheckedRunTry --op Insert` answers `yes`;
/// require the witness's trap to be the TryFunction -> `TwoRoutes --op
/// Insert` answers `no`; leave argument operation sites out of `try_sites`
/// -> `ArgTrigTry --op Commit` answers `no`; leave their edges out of
/// `try_refusal_verdict`'s argument scan -> the unfiltered `ArgTrigTry`
/// answers `no`. Each fails; restored, it passes.
#[test]
fn the_op_slice_rules_refused_operations_by_their_route() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "WT Caller::CommitTryCaller", &["--op", "Commit"]);
    assert_eq!(r["answer"], "yes", "{r}");
    let r = ask(ws, "WT Caller::ArgRecTry", &["--op", "Insert"]);
    assert_eq!(r["answer"], "yes", "{r}");
    let r = ask(ws, "WT Caller::CheckedRunTry", &["--op", "Insert"]);
    assert_eq!(r["answer"], "no", "{r}");
    // One route through a checked run, another through a try call: the
    // insert is refused on the second whatever trap the first route names.
    let r = ask(ws, "WT Caller::TwoRoutes", &["--op", "Insert"]);
    assert_eq!(r["answer"], "yes", "{r}");
    // A temporary insert in the argument list: allowed, but its OnInsert
    // trigger commits inside the try call.
    let r = ask(ws, "WT Caller::ArgTrigTry", &["--op", "Commit"]);
    assert_eq!(r["answer"], "yes", "{r}");
    let r = ask(ws, "WT Caller::ArgTrigTry", &[]);
    let want = line_of("Ok := TryTakes(T.Insert(true));");
    assert!(
        refusals(&r)
            .iter()
            .any(|(l, v, _)| *l == want && v == "escapes-if-write-refused"),
        "{:?} {r}",
        refusals(&r)
    );
    let site = format!("src/wt.al:{}", line_of("Ok := TryRead();"));
    let h = run(&[
        "prove",
        ws,
        "WT Caller::ReadTry",
        "throws-error",
        "--site",
        &site,
        "--format",
        "human",
    ]);
    assert!(
        h.contains("not raised: 1 try call's write-refusal error")
            && !h.contains("caught on every"),
        "{h}"
    );
}

const DEP_GUID: &str = "e10e10e0-0000-4000-8000-0000000000dd";

/// A try function known only from a dependency's symbols: no body to read,
/// so what it runs is unknown code and its refusal may be raised.
/// Break (2026-11-12): drop the `!body_available` seed in
/// `build_try_refusal` -> the answer is `unknown` with no refusal row
/// (never a wrong `no`; the fix adds the row); restored, it passes.
#[test]
fn a_symbol_only_try_function_may_be_refused() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"e10e10e0-0000-4000-8000-0000000000ab","name":"ENG10S","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/c.al"),
        "codeunit 52430 \"WS Caller\"\n{\n    procedure UseDepTry()\n    var\n        L: Codeunit \"XDep Lib\";\n        Ok: Boolean;\n    begin\n        Ok := L.TryDo();\n    end;\n}\n",
    )
    .unwrap();
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":70,"Name":"XDep Lib","Methods":[{{"Name":"TryDo","Parameters":[],"Attributes":[{{"Name":"TryFunction"}}]}}]}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    let r = ask(ws.to_str().unwrap(), "WS Caller::UseDepTry", &[]);
    assert_eq!(r["answer"], "yes", "{r}");
    assert!(
        refusals(&r)
            .iter()
            .any(|(l, v, _)| *l == 8 && v == "escapes-if-write-refused"),
        "{:?} {r}",
        refusals(&r)
    );
}

const SRC_DEP_GUID: &str = "e10e10e0-0000-4000-8000-0000000000de";

/// A try function that calls into dependency code that writes: the errors
/// beyond the crossing form one boundary group, which the try function
/// would catch, but a refused write's error leaves it (the group rule in
/// `digest_cli::judge_traps`).
/// Break (2026-11-12): turn the group rule off -> the group is caught and
/// not listed; restored, it passes.
#[test]
fn a_group_beyond_a_try_call_stays_escaping_when_it_may_refuse() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"e10e10e0-0000-4000-8000-0000000000ac","name":"ENG10G","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{SRC_DEP_GUID}","name":"YDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/c.al"),
        "codeunit 52440 \"WG Caller\"\n{\n    procedure UseTry()\n    var\n        Ok: Boolean;\n    begin\n        Ok := TryDep();\n    end;\n\n    [TryFunction]\n    local procedure TryDep()\n    var\n        L: Codeunit \"YDep Lib\";\n    begin\n        L.InsertRow();\n    end;\n\n    procedure UseTryRun()\n    var\n        Ok: Boolean;\n    begin\n        Ok := TryRun();\n    end;\n\n    [TryFunction]\n    local procedure TryRun()\n    begin\n        if Codeunit.Run(Codeunit::\"WG Runner\") then;\n    end;\n\n    procedure UseTryRunDep()\n    var\n        Ok: Boolean;\n    begin\n        Ok := TryRunDep();\n    end;\n\n    [TryFunction]\n    local procedure TryRunDep()\n    begin\n        if Codeunit.Run(Codeunit::\"YDep Runner\") then;\n    end;\n}\ncodeunit 52441 \"WG Runner\"\n{\n    trigger OnRun()\n    var\n        L: Codeunit \"YDep Lib\";\n    begin\n        L.InsertRow();\n    end;\n}\n",
    )
    .unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_YDep_1.0.0.0.app"),
        SRC_DEP_GUID,
        "YDep",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":52450,"Name":"YDep Row","Fields":[{{"Id":1,"Name":"K","TypeDefinition":{{"Name":"Integer"}}}}]}}],"Codeunits":[{{"Id":52451,"Name":"YDep Lib","Methods":[{{"Name":"InsertRow","Parameters":[]}}]}},{{"Id":52452,"Name":"YDep Runner","Methods":[]}}],"AppId":"{SRC_DEP_GUID}","Name":"YDep","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
        &[
            (
                "src/Row.al",
                "table 52450 \"YDep Row\"\n{\n    fields\n    {\n        field(1; K; Integer)\n        {\n        }\n    }\n}\n",
            ),
            (
                "src/Lib.al",
                "codeunit 52451 \"YDep Lib\"\n{\n    procedure InsertRow()\n    var\n        R: Record \"YDep Row\";\n    begin\n        R.Insert();\n    end;\n}\n",
            ),
            (
                "src/Runner.al",
                "codeunit 52452 \"YDep Runner\"\n{\n    trigger OnRun()\n    var\n        R: Record \"YDep Row\";\n    begin\n        R.Insert();\n    end;\n}\n",
            ),
        ],
        "",
    );
    let r = ask(ws.to_str().unwrap(), "WG Caller::UseTry", &[]);
    assert_eq!(r["answer"], "yes", "{r}");
    let group = r["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["detail"]["boundary"] == "Codeunit YDep Lib::InsertRow")
        // A caught group is not listed: missing means the try function was
        // taken to catch it.
        .unwrap_or_else(|| panic!("the group beyond InsertRow is not escaping: {r}"))
        .clone();
    assert_eq!(
        group["trap"]["verdict"], "escapes-if-write-refused",
        "{group}"
    );
    assert_eq!(group["trap"]["kind"], "try-write-refused", "{group}");
    // T10: the same write behind a checked run inside the try call runs
    // outside it: the group stays caught (not listed), nothing is refused.
    // Break (2026-11-12): rule the group by its first trap only (any
    // crossing target that may refuse) -> it is listed as escaping.
    let r = ask(ws.to_str().unwrap(), "WG Caller::UseTryRun", &[]);
    assert_eq!(r["answer"], "no", "{r}");
    // The crossing edge is itself the checked run (into dependency code):
    // its group stays caught.
    // Break (2026-11-12): skip the crossing edge's own trap check in
    // `crossing_in_try` -> listed as escaping.
    let r = ask(ws.to_str().unwrap(), "WG Caller::UseTryRunDep", &[]);
    assert_eq!(r["answer"], "no", "{r}");
}

/// A root that is itself a try function and may run a refused operation:
/// its answer holds only for callers that call it as a statement, and says
/// so (prove's assumptions and obligations, explain's header). One that
/// writes only a temporary record states nothing.
/// Breaks (2026-11-12): never set `try_root_refusal` -> no assumption;
/// drop `may_refuse` from `try_root_may_refuse` -> `TryTemp` states it.
/// Each fails; restored, it passes.
#[test]
fn a_try_function_root_states_its_callers_assumption() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["TryCommit", "TryInsert", "TryLock"] {
        let r = ask(ws, &format!("WT Caller::{routine}"), &[]);
        assert_eq!(
            r["obligations"]["assumesStatementCallsOnly"], true,
            "{routine}: {r}"
        );
        let h = human(ws, &format!("WT Caller::{routine}"));
        assert!(
            h.contains("assumes the routine's callers call it as a statement"),
            "{routine}: {h}"
        );
        let e = run(&["explain", ws, "--routine", &format!("WT Caller::{routine}")]);
        assert!(
            e.contains("assumes the routine's callers call it as a statement"),
            "{routine}: {e}"
        );
    }
    let r = ask(ws, "WT Caller::TryTemp", &[]);
    assert!(
        r["obligations"].get("assumesStatementCallsOnly").is_none(),
        "{r}"
    );
}

/// A library whose try function no loaded routine calls: its callers are
/// all elsewhere, so its answer still states the assumption.
/// Break (2026-11-12): build the refusal facts only when a try call exists
/// -> no assumption; restored, it passes.
#[test]
fn a_try_function_with_no_loaded_caller_states_the_assumption() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e10e10e0-0000-4000-8000-0000000000ad","name":"ENG10L","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/l.al"),
        "table 52460 \"WL Row\" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }\ncodeunit 52461 \"WL Lib\"\n{\n    [TryFunction]\n    procedure TryPost()\n    var\n        R: Record \"WL Row\";\n    begin\n        if not R.Insert() then;\n    end;\n}\n",
    )
    .unwrap();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "WL Lib::TryPost", &[]);
    assert_eq!(r["obligations"]["assumesStatementCallsOnly"], true, "{r}");
}
