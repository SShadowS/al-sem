//! ENG-12 (engine limits L35, L36, ENG-06 review gap (b)): record operations
//! the engine did not see. Probe on master `bdb7aab5`:
//! docs/superpowers/notes/2026-11-12-eng12-record-ops-probe.md. Every
//! routine below answered a definite `no` there.

use std::process::Command;

const SOURCE: &str = r#"table 52600 "E12 Row"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; V; Integer) { }
        field(3; Cnt; Integer) { FieldClass = FlowField; CalcFormula = count("E12 Plain"); }
    }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
    procedure BareSysId(Id: Guid)
    begin
        if GetBySystemId(Id) then;
    end;
}
codeunit 52601 "E12 Ops"
{
    procedure SysIdMember(Id: Guid)
    var
        R: Record "E12 Row";
    begin
        if R.GetBySystemId(Id) then;
    end;
    procedure RecRefSysId(Id: Guid)
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"E12 Row");
        if RR.GetBySystemId(Id) then;
    end;
    procedure ArrMember()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        if Recs[1].FindFirst() then;
    end;
    procedure ArrWith()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        with Recs[1] do
            if FindFirst() then;
    end;
    procedure ArrModifyTrue()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[1].Modify(true);
    end;
    procedure RRArrModifyTrue()
    var
        RRs: array[2] of RecordRef;
    begin
        RRs[1].Open(Database::"E12 Row");
        RRs[1].Modify(true);
    end;
    procedure GetRefModifyTrue()
    begin
        GetRef().Modify(true);
    end;
    local procedure GetRef(): Record "E12 Row"
    var
        R: Record "E12 Row";
    begin
        exit(R);
    end;
    procedure GetPlainModifyTrue()
    begin
        GetPlain().Modify(true);
    end;
    local procedure GetPlain(): Record "E12 Plain"
    var
        P: Record "E12 Plain";
    begin
        exit(P);
    end;
    procedure GetRRModifyTrue()
    begin
        GetRR().Modify(true);
    end;
    local procedure GetRR(): RecordRef
    var
        RR: RecordRef;
    begin
        exit(RR);
    end;
}
table 52602 "E12 Plain"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 52603 "E12 Worker"
{
    procedure Modify()
    begin
        Commit();
    end;
}
codeunit 52604 "E12 More"
{
    var
        X: Record "E12 Plain";
        GRecs: array[2] of Record "E12 Row";
    procedure Shadow()
    var
        X: array[2] of Codeunit "E12 Worker";
    begin
        X[1].Modify();
    end;
    procedure WithModifyTrue()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        with Recs[1] do
            Modify(true);
    end;
    procedure GlobalArr()
    begin
        if GRecs[1].FindFirst() then;
    end;
    procedure ArrParam(var Recs: array[2] of Record "E12 Row")
    begin
        if Recs[1].FindFirst() then;
    end;
    procedure TempArr()
    var
        Temps: array[2] of Record "E12 Row" temporary;
    begin
        Temps[1].K := 1;
        Temps[1].Insert();
    end;
    procedure GetRefRead()
    begin
        if GetRef().FindFirst() then;
    end;
    local procedure GetRef(): Record "E12 Row"
    var
        R: Record "E12 Row";
    begin
        exit(R);
    end;
    procedure SpacedFlow()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[1].SetAutoCalcFields(Cnt);
        if Recs[ 1 ].FindFirst() then;
    end;
    procedure SpacedSetup()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[ 1 ].SetAutoCalcFields(Cnt);
        if Recs[1].FindFirst() then;
    end;
    procedure SpacedBoth()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[ 1 ].SetAutoCalcFields(Cnt);
        if Recs[ 1 ].FindFirst() then;
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e12e12e0-0000-4000-8000-0000000000aa","name":"E12","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
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

/// The `answer:` line of `query touches --table "E12 Row" --direction down`.
fn touches_down(ws: &str, from: &str) -> String {
    let h = run(&[
        "query",
        "touches",
        "--table",
        "E12 Row",
        "--from",
        from,
        "--direction",
        "down",
        ws,
    ]);
    h.lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_default()
        .to_string()
}

/// L35: `GetBySystemId` reads its table (on a record, bare in the table,
/// on a RecordRef opened on it).
/// Break (2026-11-12): drop `getbysystemid` from `record_ops::record_op_type`
/// -> each answers `down: no (every candidate decided)`; restored, passes.
#[test]
fn get_by_system_id_reads_its_table() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for from in [
        "\"E12 Row\".BareSysId",
        "\"E12 Ops\".SysIdMember",
        "\"E12 Ops\".RecRefSysId",
    ] {
        let a = touches_down(ws, from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
}

/// The `answer:` line of `prove <routine> may-commit`.
fn may_commit(ws: &str, routine: &str) -> String {
    let h = run(&[
        "prove",
        ws,
        &format!("\"E12 Ops\"::{routine}"),
        "may-commit",
        "--format",
        "human",
    ]);
    h.lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_default()
        .to_string()
}

/// L36: an array element is a record: its reads touch the table, member and
/// `with` alike.
/// Break (2026-11-12): `record_ops::receiver_variable` without its `Index`
/// arm -> `ArrMember` answers `down: no (every candidate decided)`; restored,
/// passes. (`Recs[1].Modify(true)` still answers `yes` then, through the
/// member-path trigger step, the second test's fallback.)
#[test]
fn an_array_element_is_a_record() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for from in ["\"E12 Ops\".ArrMember", "\"E12 Ops\".ArrWith"] {
        let a = touches_down(ws, from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
}

/// ENG-06 review gap (b): a trigger-firing operation on a receiver that is
/// not a plain variable runs the table's triggers (`OnModify` commits):
/// an array element of a known table -> yes; a RecordRef array element (any
/// table may be open) -> unknown; a call's result of a known table -> yes;
/// of a table with no trigger -> unknown (its table events are not followed),
/// a RecordRef call result -> unknown. Never a definite `no`.
/// Breaks (2026-11-12): drop the receiver id from `CalleeShape::RecordOp` in
/// `full.rs` (`None` to `infer_receiver_type`) -> `ArrModifyTrue` answers
/// `unknown` (no table, the open edge), not `yes`; drop the member-path
/// trigger step in `full.rs` -> `GetRefModifyTrue` answers `no`; keep the
/// step but not its unknown route -> `GetPlainModifyTrue` answers `no`. Each
/// fails; restored, it passes.
#[test]
fn a_trigger_firing_op_on_an_element_or_a_call_result_is_never_a_definite_no() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, want) in [
        ("ArrModifyTrue", "answer: yes"),
        ("RRArrModifyTrue", "answer: unknown"),
        ("GetRefModifyTrue", "answer: yes"),
        ("GetPlainModifyTrue", "answer: unknown"),
        ("GetRRModifyTrue", "answer: unknown"),
    ] {
        let a = may_commit(ws, routine);
        assert!(a.starts_with(want), "{routine}: {a}");
    }
}

/// The `answer:` line of a command's human output.
fn answer(args: &[&str]) -> String {
    run(args)
        .lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_default()
        .to_string()
}

/// The review's shapes (ENG-12 round 1):
/// - a local codeunit array shadowing a record global of its name is no
///   record: `X[1].Modify()` is the codeunit's call, which commits;
/// - `with Recs[1] do Modify(true)` runs the trigger;
/// - a global array and an array parameter are records through elements;
/// - a temporary array's element is temporary;
/// - a read on a call's result is never a definite `no`;
/// - `Recs[ 1 ]` is the element `Recs[1]` set its FlowField up on, so its
///   retrieval reads the FlowField's source table.
///
/// Breaks (2026-11-12): skip `routine_non_record_names` in both engines ->
/// `Shadow` answers `no`; `receiver_variable` without its `Index` arm ->
/// `GlobalArr` and `ArrParam` answer `no`; skip the temporary element in
/// pass 2b -> `TempArr` counts 0 temporary touches; `touches_table` false in
/// the member-path step -> `GetRefRead` answers `no`; `receiver_key_text`
/// returning its input -> `SpacedFlow` answers `no` for "E12 Plain". Each
/// fails; restored, it passes.
#[test]
fn the_review_shapes() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let mc = |r: &str| {
        answer(&[
            "prove",
            ws,
            &format!("\"E12 More\"::{r}"),
            "may-commit",
            "--format",
            "human",
        ])
    };
    let tc = |r: &str, table: &str| {
        answer(&[
            "query",
            "touches",
            "--table",
            table,
            "--from",
            &format!("\"E12 More\".{r}"),
            "--direction",
            "down",
            ws,
        ])
    };
    let a = mc("Shadow");
    assert!(a.starts_with("answer: yes"), "Shadow: {a}");
    let a = mc("WithModifyTrue");
    assert!(a.starts_with("answer: yes"), "WithModifyTrue: {a}");
    for r in ["GlobalArr", "ArrParam"] {
        let a = tc(r, "E12 Row");
        assert!(a.starts_with("answer: down: yes"), "{r}: {a}");
    }
    let a = tc("TempArr", "E12 Row");
    assert!(a.contains("1 on temporary records"), "TempArr: {a}");
    let a = tc("GetRefRead", "E12 Row");
    assert!(a.starts_with("answer: down: unknown"), "GetRefRead: {a}");
    // Round 2 (astra): either side spaced, or both. The setup side already
    // matches without normalizing `flow_fields::var_key` (a break normalizing
    // it changed nothing, 2026-11-12), so these pin the behaviour only.
    for r in ["SpacedFlow", "SpacedSetup", "SpacedBoth"] {
        let a = tc(r, "E12 Plain");
        assert!(a.starts_with("answer: down: yes"), "{r}: {a}");
    }
}
