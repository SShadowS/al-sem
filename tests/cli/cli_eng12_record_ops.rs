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
    procedure GetRefModifyAll()
    begin
        GetRef().ModifyAll(V, 1);
    end;
    procedure GetRefCalc()
    begin
        GetRef().CalcFields(Cnt);
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
    procedure GetSubModifyTrue()
    begin
        GetSub().Modify(true);
    end;
    local procedure GetSub(): Record "E12 Sub"
    var
        S: Record "E12 Sub";
    begin
        exit(S);
    end;
}
table 52605 "E12 Sub"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 52606 "E12 Sub Listener"
{
    [EventSubscriber(ObjectType::Table, Database::"E12 Sub", 'OnAfterModifyEvent', '', false, false)]
    local procedure OnAfterModifySub(var Rec: Record "E12 Sub"; var xRec: Record "E12 Sub"; RunTrigger: Boolean)
    begin
        Commit();
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
        G: Record "E12 Row";
        TempG: Record "E12 Row" temporary;
        "My G": Record "E12 Row";
        "Temp Q": Record "E12 Row" temporary;
        XC: Codeunit "E12 Worker";
        GB: Record "E12 Sub";
        GT: Record "E12 Row";
        GRR: RecordRef;
    procedure ThisRRModify()
    begin
        this.GRR.Modify(true);
    end;
    procedure ThisQuoted()
    begin
        this."My G".Modify(true);
    end;
    procedure ThisQuotedTemp()
    begin
        this."Temp Q".Insert();
    end;
    procedure ThisShadowCodeunit()
    var
        XC: Record "E12 Plain";
    begin
        this.XC.Modify();
    end;
    procedure ThisShadowRecord()
    var
        GB: Record "E12 Plain";
    begin
        this.GB.Modify(true);
    end;
    procedure ThisShadowInteger()
    var
        GT: Integer;
    begin
        GT := 1;
        this.GT.Modify(true);
    end;
    procedure ThisAndBareFlow()
    begin
        G.SetAutoCalcFields(Cnt);
        if this.G.FindFirst() then;
    end;
    procedure GetRRRead()
    begin
        if GetRRX().FindFirst() then;
    end;
    local procedure GetRRX(): RecordRef
    var
        RR: RecordRef;
    begin
        exit(RR);
    end;
    procedure ThisRead()
    begin
        if this.G.FindFirst() then;
    end;
    procedure ThisModifyTrue()
    begin
        this.G.Modify(true);
    end;
    procedure ThisTemp()
    begin
        this.TempG.K := 1;
        this.TempG.Insert();
    end;
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
}
codeunit 52607 "E12 Spaced Flow"
{
    procedure SpacedFlow()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[1].SetAutoCalcFields(Cnt);
        if Recs[ 1 ].FindFirst() then;
    end;
}
codeunit 52608 "E12 Spaced Setup"
{
    procedure SpacedSetup()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[ 1 ].SetAutoCalcFields(Cnt);
        if Recs[1].FindFirst() then;
    end;
}
codeunit 52609 "E12 Spaced Both"
{
    procedure SpacedBoth()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[ 1 ].SetAutoCalcFields(Cnt);
        if Recs[ 1 ].FindFirst() then;
    end;
}
codeunit 52611 "E12 This Flow"
{
    var
        G: Record "E12 Row";
    procedure ShadowBoth()
    var
        G: Integer;
    begin
        G := 1;
        this.G.SetAutoCalcFields(Cnt);
        if this.G.FindFirst() then;
    end;
}
codeunit 52612 "E12 This Cross"
{
    var
        G: Record "E12 Row";
    procedure Setup()
    begin
        G.SetAutoCalcFields(Cnt);
    end;
    procedure ShadowRead()
    var
        G: Integer;
    begin
        G := 1;
        if this.G.FindFirst() then;
    end;
}
codeunit 52614 "E12 This Local"
{
    var
        G: Record "E12 Row";
    procedure LocalSetupGlobalRead()
    var
        G: Record "E12 Row";
    begin
        G.SetAutoCalcFields(Cnt);
        if this.G.FindFirst() then;
    end;
}
codeunit 52613 "E12 Bracket"
{
    procedure Setup()
    var
        "Line [Temp]": Record "E12 Row";
    begin
        "Line [Temp]".SetAutoCalcFields(Cnt);
    end;
    procedure Read()
    var
        "Line [Temp]": Record "E12 Row";
    begin
        if "Line [Temp]".FindFirst() then;
    end;
}
codeunit 52610 "E12 Local Setup"
{
    procedure Setup()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        Recs[1].SetAutoCalcFields(Cnt);
    end;
    procedure Retrieve()
    var
        Recs: array[2] of Record "E12 Row";
    begin
        if Recs[1].FindFirst() then;
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
/// not a plain variable runs the table's triggers (`OnModify` commits) and
/// its table events: an array element of a known table -> yes; a RecordRef
/// array element (any table may be open) -> unknown; a call's result of a
/// known table, a record operation of its own (`CallResultRecord`) -> yes; of
/// a table with no trigger and no subscriber -> no (a real `no`); of a table
/// whose `OnAfterModifyEvent` subscriber commits -> yes; a RecordRef call
/// result (runtime-typed) -> unknown.
/// Breaks (2026-11-12): drop the receiver id from `CalleeShape::RecordOp` in
/// `full.rs` (`None` to `infer_receiver_type`) -> `ArrModifyTrue` answers
/// `unknown` (no table, the open edge), not `yes`; never set
/// `SiteFacts::call_result_record` -> `GetRefModifyTrue` answers `no` (and the
/// review test's `GetRefRead`); drop the table-event edges in
/// `program_calls::call_result_operation` -> `GetSubModifyTrue` answers `no`.
/// Each fails; restored, it passes.
#[test]
fn a_trigger_firing_op_on_an_element_or_a_call_result_is_never_a_definite_no() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, want) in [
        ("ArrModifyTrue", "answer: yes"),
        ("RRArrModifyTrue", "answer: unknown"),
        ("GetRefModifyTrue", "answer: yes"),
        // No trigger and no subscriber on its table: a real `no`.
        ("GetPlainModifyTrue", "answer: no"),
        // A table-event subscriber commits: reached through the call result.
        ("GetSubModifyTrue", "answer: yes"),
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
/// - a read on a known table's call result reads it (a record operation);
/// - an object global through `this` is a record operation with the global's
///   own temp state (ENG-12 gate).
///
/// Breaks (2026-11-12): skip `routine_non_record_names` in both engines ->
/// `Shadow` answers `no`; `receiver_variable` without its `Index` arm ->
/// `GlobalArr` and `ArrParam` answer `no`; skip the temporary element in
/// pass 2b -> `TempArr` counts 0 temporary touches; `touches_table` false in
/// the member-path step -> `GetRefRead` answers `no`; `record_ops::is_this` false -> `ThisTemp` counts 0 temporary touches (the
/// call-result path still answers `ThisRead`/`ThisModifyTrue`, with an
/// unknown temp state). Review round 3: keep the quotes of `this."X"` ->
/// `ThisQuotedTemp` counts 0 temporary touches; let `this_receivers` keep a
/// global a local shadows -> `ThisShadowRecord` answers `no` (the local's
/// table); send a shadowed `this.X` past the call-result path (the guard back
/// to `is_none`) -> `ThisShadowRecord` answers `no`; keep `this.` in
/// `receiver_key_text` -> `ThisAndBareFlow` answers `no` (the FlowField set
/// up on `G` is not `this.G`'s). Each fails; restored, it passes.
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
    // ENG-12 gate: an object global through `this` is a record operation (CDO's
    // dependencies: `this.TempOnboardingCompany.FindSet(false)`).
    let a = mc("ThisModifyTrue");
    assert!(a.starts_with("answer: yes"), "ThisModifyTrue: {a}");
    let a = tc("ThisRead", "E12 Row");
    assert!(a.starts_with("answer: down: yes"), "ThisRead: {a}");
    // ... with the global's own temp state (CDO's are `this.TempX`).
    let a = tc("ThisTemp", "E12 Row");
    assert!(a.contains("1 on temporary records"), "ThisTemp: {a}");
    // Review round 3: a quoted global; a same-named local of any type (the
    // global's codeunit call, its own table's subscriber, its own trigger);
    // `this.G` and `G` are one record (the FlowField set up on `G`).
    for r in [
        "ThisQuoted",
        "ThisShadowCodeunit",
        "ThisShadowRecord",
        "ThisShadowInteger",
    ] {
        let a = mc(r);
        assert!(a.starts_with("answer: yes"), "{r}: {a}");
    }
    // A quoted global is the variable too, with its own temp state.
    // The shadowed global's own table (the call-result op), not the local's.
    let a = tc("ThisShadowRecord", "E12 Sub");
    assert!(a.starts_with("answer: down: yes"), "ThisShadowRecord: {a}");
    let a = tc("ThisQuotedTemp", "E12 Row");
    assert!(a.contains("1 on temporary records"), "ThisQuotedTemp: {a}");
    let a = tc("ThisAndBareFlow", "E12 Plain");
    assert!(a.starts_with("answer: down: yes"), "ThisAndBareFlow: {a}");
    // A read on a RecordRef call result: runtime-typed, never a definite `no`.
    let a = tc("GetRRRead", "E12 Row");
    assert!(a.starts_with("answer: down: unknown"), "GetRRRead: {a}");
    for r in ["GlobalArr", "ArrParam"] {
        let a = tc(r, "E12 Row");
        assert!(a.starts_with("answer: down: yes"), "{r}: {a}");
    }
    let a = tc("TempArr", "E12 Row");
    assert!(a.contains("1 on temporary records"), "TempArr: {a}");
    // A read on a known table's call result is a record operation of its own.
    let a = tc("GetRefRead", "E12 Row");
    assert!(a.starts_with("answer: down: yes"), "GetRefRead: {a}");
}

/// The `answer:` line of `query touches --table <table> --from <from> --direction down`.
fn touches(ws: &str, table: &str, from: &str) -> String {
    answer(&[
        "query",
        "touches",
        "--table",
        table,
        "--from",
        from,
        "--direction",
        "down",
        ws,
    ])
}

/// Rounds 2 and 3 (astra): `Recs[ 1 ]` is the element `Recs[1]`, on either
/// side of a FlowField's setup, so the retrieval reads the FlowField's source
/// table. Each shape in its own codeunit: a setup in another routine of the
/// same object once supplied the missing key (a local array's element was
/// shared as an undeclared name, the implicit `Rec`); it is the routine's own
/// local, shared with no other routine (`Retrieve`).
/// Round 4: a `this.G` under a same-named local is the global on both sides
/// (its retrieval is a call-result op, appended after the FlowField pass:
/// `ShadowBoth`; the setup in another routine: `ShadowRead`); a variable
/// whose quoted name holds `[` is looked up whole (`"Line [Temp]"`, Fable):
/// a local, so another routine's setup is not its own.
/// Breaks (2026-11-12, re-run 2026-11-13): `flow_fields::receiver_key` without
/// `receiver_key_text`
/// -> `SpacedSetup` and `SpacedBoth` answer `no`; `Sharing::scopes` looking
/// the whole key up again (not its array variable) -> `Retrieve` answers
/// `yes`. (2026-11-13): no retrievals in
/// `attach_appended_flow_field_reads` (`CalcFields` only) -> `ShadowBoth`
/// answers `no`; `scopes` ignoring `this` -> `ShadowRead` answers `no`; the
/// own-routine match ignoring the `this` flag -> `LocalSetupGlobalRead`
/// answers `yes`;
/// `scopes` without the whole-key lookup -> `Bracket` `Read` answers `yes`
/// (the name falls to the object's scope; a global there shares alike, so
/// only a local tells).
/// Each fails; restored, it passes.
#[test]
fn a_spaced_element_is_the_element_its_flow_field_was_set_up_on() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (from, want) in [
        ("\"E12 Spaced Flow\".SpacedFlow", "answer: down: yes"),
        ("\"E12 Spaced Setup\".SpacedSetup", "answer: down: yes"),
        ("\"E12 Spaced Both\".SpacedBoth", "answer: down: yes"),
        // Another routine's local array: its setup is not this one's.
        ("\"E12 Local Setup\".Retrieve", "answer: down: no"),
        ("\"E12 This Flow\".ShadowBoth", "answer: down: yes"),
        ("\"E12 This Cross\".ShadowRead", "answer: down: yes"),
        // A local `G`'s setup is not the global `this.G`'s (astra round 5).
        (
            "\"E12 This Local\".LocalSetupGlobalRead",
            "answer: down: no",
        ),
        // Another routine's local whose quoted name holds `[`: not shared.
        ("\"E12 Bracket\".Read", "answer: down: no"),
    ] {
        let a = touches(ws, "E12 Plain", from);
        assert!(a.starts_with(want), "{from}: {a}");
    }
}

/// Round 3 (astra): a call result's `CalcFields` reads its FlowFields'
/// source tables, and its `ModifyAll` writes its field.
/// Breaks (2026-11-12): no `CalcFields` field arguments in
/// `program_calls::call_result_operation` -> `GetRefCalc` answers `no` for
/// "E12 Plain"; `modify_all_field: None` there -> `GetRefModifyAll` is not a
/// writer of `V`. Each fails; restored, it passes.
#[test]
fn a_call_result_calc_fields_reads_and_modify_all_writes_its_field() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let a = touches(ws, "E12 Plain", "\"E12 Ops\".GetRefCalc");
    assert!(a.starts_with("answer: down: yes"), "GetRefCalc: {a}");
    let h = run(&[
        "explain", "--table", "E12 Row", "--field", "V", "--format", "json", ws,
    ]);
    assert!(h.contains("GetRefModifyAll"), "{h}");
}

/// ENG-12 gate (`dependency_body_unknown_ceiling_on_cdo`): no site here is an
/// `unknown` edge. The two RecordRef call results (`GetRR().Modify(true)`,
/// `GetRRX().FindFirst()`, their tables runtime-typed) are the two
/// honest-dynamic edges; a known table's call result is a record operation of
/// its own; an array element and `this.X` are record operations.
/// Break (2026-11-12): the RecordRef result keeps an unknown route instead of
/// `DynamicOpen` -> unknown 1, honestDynamic 0 (one RecordRef site then);
/// restored, passes.
#[test]
fn no_site_is_an_unknown_edge() {
    let t = workspace();
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(t.path())
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn aldump");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    let p = &v["primaryScoped"];
    assert_eq!(
        (p["unknown"].as_u64(), p["honestDynamic"].as_u64()),
        (Some(0), Some(2)),
        "{p}"
    );
}

/// Fable round 4: a global RecordRef through `this` keeps its RecordRef flag
/// (`receiver_is_record_ref`), which the effects label and the RecordRef
/// origin analysis read; the `this.` key is the unshadowed global's.
/// Break (2026-11-12): look `this.grr` up unstripped in `record_ref_vars` ->
/// the label is "table not resolved", not "on a RecordRef"; restored, passes.
#[test]
fn a_global_record_ref_through_this_stays_a_record_ref() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let h = run(&[
        "query",
        "effects",
        "--routine",
        "\"E12 More\".ThisRRModify",
        ws,
    ]);
    assert!(h.contains("on a RecordRef (table not resolved"), "{h}");
}
