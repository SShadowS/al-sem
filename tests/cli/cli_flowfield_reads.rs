//! Engine limits B5 / L19: a FlowField's `CalcFormula` table is read where the
//! FlowField is calculated (`CalcFields`, a filter on the FlowField, a read
//! after `SetAutoCalcFields`). Before the fix `query touches --direction down`
//! answered a definite "no" for that table and `query effects` listed only the
//! record's own table.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem`, so the user-visible answer is
//! pinned, not a helper. Each fixture states its precondition in the AL text:
//! the table read is reachable ONLY through the FlowField (no routine of the
//! cone names it as a record).

use std::path::Path;
use std::process::Command;

const WS_GUID: &str = "aaaa0b05-0000-0000-0000-0000000000b5";
const DEP_GUID: &str = "bbbb0b05-0000-0000-0000-0000000000b5";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `alsem <args> --deterministic` -> stdout (exit 0 asserted).
fn alsem(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--deterministic")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", "cli-b5-v1")
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "alsem {args:?} failed: {text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    text
}

fn answer_line(text: &str) -> &str {
    text.lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_else(|| panic!("no answer line: {text}"))
}

fn app_json(dir: &Path, deps: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"WsB5","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{deps}]}}"#
        ),
    );
}

/// Workspace tables: "B5 Header" has a sum FlowField and a count FlowField
/// over "B5 Line", a FlowFilter and a Blob. No codeunit names "B5 Line" as a
/// record: every read of it below goes through a FlowField.
const TABLES: &str = r#"
table 50100 "B5 Header"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Total Amount"; Decimal)
        {
            FieldClass = FlowField;
            CalcFormula = sum("B5 Line".Amount where("Header No." = field("No."), "Date" = field("Date Filter")));
        }
        field(3; "Date Filter"; Date) { FieldClass = FlowFilter; }
        field(4; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Line" where("Header No." = field("No.")));
        }
        field(5; Picture; Blob) { }
        field(6; Amount; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

table 50101 "B5 Line"
{
    fields
    {
        field(1; "Header No."; Code[20]) { }
        field(2; "Line No."; Integer) { }
        field(3; Amount; Decimal) { }
        field(4; "Date"; Date) { }
    }
    keys { key(PK; "Header No.", "Line No.") { Clustered = true; } }
}

table 50102 "B5 Node"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Parent No."; Code[20]) { }
        field(3; "Child Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Node" where("Parent No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}
"#;

const LOGIC: &str = r#"
codeunit 50100 "B5 Logic"
{
    procedure ReadTotals(No: Code[20]): Decimal
    var
        H: Record "B5 Header";
    begin
        H.Get(No);
        H.SetRange("Date Filter", 20200101D, 20201231D);
        H.CalcFields("Total Amount");
        exit(H."Total Amount");
    end;

    procedure AutoTotals(): Decimal
    var
        H: Record "B5 Header";
    begin
        H.SetAutoCalcFields("Line Count");
        if H.FindSet() then
            exit(H."Line Count");
    end;

    procedure FilterOnFlowField(): Boolean
    var
        H: Record "B5 Header";
    begin
        H.SetFilter("Total Amount", '>%1', 0);
        exit(H.IsEmpty());
    end;

    procedure ReadBlobAndSums()
    var
        H: Record "B5 Header";
    begin
        H.Get('A');
        H.SetRange("Date Filter", 20200101D, 20201231D);
        H.CalcFields(Picture);
        H.CalcSums(Amount);
    end;

    procedure CountChildren(): Integer
    var
        N: Record "B5 Node";
    begin
        N.Get('A');
        N.CalcFields("Child Count");
        exit(N."Child Count");
    end;
}
"#;

fn workspace_only(dir: &Path) {
    app_json(dir, "");
    write(&dir.join("src/Tables.al"), TABLES);
    write(&dir.join("src/Logic.al"), LOGIC);
}

fn touches_down(ws: &str, table: &str, from: &str) -> String {
    alsem(&[
        "query",
        "touches",
        ws,
        "--table",
        table,
        "--from",
        from,
        "--direction",
        "down",
    ])
}

/// The user-visible answer of B5: a table read only through a FlowField is
/// touched, for each way BC calculates one.
#[test]
fn flowfield_destination_table_is_touched_down() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    workspace_only(&ws);
    let ws = ws.to_str().unwrap();

    for from in ["ReadTotals", "AutoTotals", "FilterOnFlowField"] {
        let out = touches_down(ws, "B5 Line", from);
        assert!(
            answer_line(&out).starts_with("answer: down: yes"),
            "{from}: a FlowField read of B5 Line must be a touch: {out}"
        );
    }

    // A Blob `CalcFields`, `CalcSums` of an ordinary field and a filter on a
    // FlowFilter read the record's own table only.
    let out = touches_down(ws, "B5 Line", "ReadBlobAndSums");
    assert!(
        answer_line(&out).starts_with("answer: down: no"),
        "no FlowField is calculated in ReadBlobAndSums: {out}"
    );
}

/// `query effects` lists the destination read with its FlowField, and
/// `query touches --table` (table mode) counts the site.
#[test]
fn flowfield_read_is_listed_by_effects_and_table_mode() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    workspace_only(&ws);
    let ws = ws.to_str().unwrap();

    let out = alsem(&["query", "effects", ws, "--routine", "ReadTotals", "--all"]);
    assert!(
        out.contains(
            "[CalcFields] on B5 Line through FlowField \"Total Amount\" via direct, not temporary"
        ),
        "{out}"
    );
    assert!(
        out.contains("[CalcFields] on B5 Header via direct"),
        "the host read stays: {out}"
    );

    let out = alsem(&["query", "touches", ws, "--table", "B5 Line"]);
    assert!(
        answer_line(&out).contains("workspace sites: 3 in 3 routines — by op: CalcFields 3"),
        "ReadTotals' CalcFields, AutoTotals' FindSet and FilterOnFlowField's SetFilter: {out}"
    );

    // `digest --include-reads`: a DB_READ of B5 Line at the CalcFields.
    let out = alsem(&[
        "digest",
        ws,
        "--routine",
        "ReadTotals",
        "--include-reads",
        "--schema",
        "2",
        "--format",
        "json",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let line_reads: Vec<&serde_json::Value> = doc["payload"]["entries"][0]["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["type"] == "DB_READ" && f["detail"]["resourceDisplay"] == "B5 Line")
        .collect();
    assert_eq!(line_reads.len(), 1, "{out}");
    assert_eq!(
        line_reads[0]["evidence"]["excerpt"], "H.CalcFields",
        "{out}"
    );

    // `fingerprint`: reading B5 Line needs its read right.
    let out = alsem(&["fingerprint", ws, "--routine", "ReadTotals"]);
    assert!(
        out.contains(&format!("TableData \"{WS_GUID}/table/50101\" R")),
        "{out}"
    );
}

/// A FlowField of a symbol-only dependency: its `CalcFormula` comes from
/// `SymbolReference.json`. A FlowField whose symbols carry no formula is not
/// decided: the answer is never a definite "no".
#[test]
fn dependency_flowfield_reads_its_symbol_formula() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    app_json(
        &ws,
        &format!(
            r#"{{"id":"{DEP_GUID}","name":"B5Base","publisher":"probe","version":"1.0.0.0"}}"#
        ),
    );
    let customer = r#"{"Id":18,"Name":"Customer","Fields":[
        {"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}},
        {"Id":55,"Name":"Date Filter","TypeDefinition":{"Name":"Date"},"Properties":[{"Name":"FieldClass","Value":"FlowFilter"}]},
        {"Id":61,"Name":"Net Change (LCY)","TypeDefinition":{"Name":"Decimal"},"Properties":[{"Name":"CalcFormula","Value":"sum(\"Detailed Cust. Ledg. Entry\".\"Amount (LCY)\" where(\"Customer No.\" = field(\"No.\"),\r\n \"Posting Date\" = field(\"Date Filter\")))"},{"Name":"FieldClass","Value":"FlowField"}]},
        {"Id":62,"Name":"Opaque Total","TypeDefinition":{"Name":"Decimal"},"Properties":[{"Name":"FieldClass","Value":"FlowField"}]}
    ]}"#;
    let entry = r#"{"Id":379,"Name":"Detailed Cust. Ledg. Entry","Fields":[{"Id":1,"Name":"Entry No.","TypeDefinition":{"Name":"Integer"}}]}"#;
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_B5Base_1.0.0.0.app"),
        DEP_GUID,
        "B5Base",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{customer},{entry}],"AppId":"{DEP_GUID}","Name":"B5Base","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    write(
        &ws.join("src/Setup.al"),
        r#"
codeunit 50101 "B5 Setup"
{
    procedure Balance(Cust: Record Customer; EndDate: Date): Boolean
    begin
        Cust.SetFilter("Date Filter", '..%1', EndDate);
        Cust.CalcFields("Net Change (LCY)");
        exit(Cust."Net Change (LCY)" <> 0);
    end;

    procedure Opaque(Cust: Record Customer): Decimal
    begin
        Cust.CalcFields("Opaque Total");
        exit(Cust."Opaque Total");
    end;
}
"#,
    );
    let ws = ws.to_str().unwrap();

    let out = touches_down(ws, "Detailed Cust. Ledg. Entry", "Balance");
    assert!(
        answer_line(&out).starts_with("answer: down: yes"),
        "the symbols' CalcFormula names the table: {out}"
    );

    let out = touches_down(ws, "Detailed Cust. Ledg. Entry", "Opaque");
    assert!(
        !answer_line(&out).starts_with("answer: down: no"),
        "a FlowField with no formula in the symbols must not answer no: {out}"
    );
    let out = alsem(&["query", "effects", ws, "--routine", "Opaque", "--all"]);
    assert!(
        out.contains("[CalcFields] on table not decided (the FlowField's CalcFormula is not in the loaded source or symbols) through FlowField \"Opaque Total\""),
        "{out}"
    );
}

/// More ways a `SetAutoCalcFields` reaches a retrieval: a bare call on a
/// table's implicit `Rec`, and an object global set in one procedure and
/// retrieved in another. In a separate workspace so the counts above stay.
const AUTO_MORE: &str = r#"
table 50103 "B5 Owner"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Line" where("Header No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { Clustered = true; } }

    procedure BareAuto(): Integer
    begin
        SetAutoCalcFields("Line Count");
        if FindSet() then
            exit("Line Count");
    end;
}

codeunit 50104 "B5 Globals"
{
    var
        G: Record "B5 Header";

    procedure Prepare()
    begin
        G.SetAutoCalcFields("Line Count");
    end;

    procedure Fetch(): Integer
    begin
        if G.FindSet() then
            exit(G."Line Count");
    end;

    procedure Shadowed(): Integer
    var
        G: Record "B5 Header";
    begin
        if G.FindSet() then
            exit(G."Line Count");
    end;
}

codeunit 50105 "B5 Self"
{
    procedure AutoSelf(): Integer
    var
        N: Record "B5 Node";
    begin
        N.SetAutoCalcFields("Child Count");
        if N.FindSet() then
            exit(N."Child Count");
    end;
}
"#;

/// Precondition stated in `AUTO_MORE`: `BareAuto` reads "B5 Line" only
/// through the bare `SetAutoCalcFields` on its implicit `Rec`; `Fetch` only
/// through the global `G` that `Prepare` set up; `Shadowed` retrieves a LOCAL
/// `G` nothing set up.
#[test]
fn set_auto_calc_fields_on_implicit_rec_and_globals_is_a_read() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    workspace_only(&ws);
    write(&ws.join("src/AutoMore.al"), AUTO_MORE);
    let ws = ws.to_str().unwrap();

    for from in ["BareAuto", "Fetch"] {
        let out = touches_down(ws, "B5 Line", from);
        assert!(
            answer_line(&out).starts_with("answer: down: yes"),
            "{from}: the FindSet calculates \"Line Count\": {out}"
        );
    }
    let out = touches_down(ws, "B5 Line", "Shadowed");
    assert!(
        answer_line(&out).starts_with("answer: down: no"),
        "a local G is not the global G: {out}"
    );

    // The `FindSet` row is the host read; only the derived row names the
    // FlowField.
    let out = alsem(&["query", "effects", ws, "--routine", "AutoSelf", "--all"]);
    assert!(
        out.contains("[CalcFields] on B5 Node through FlowField \"Child Count\" via direct"),
        "{out}"
    );
    assert!(out.contains("[FindSet] on B5 Node via direct"), "{out}");
}

/// A formula naming a table that two loaded tables (different apps) are
/// named: the model keeps no namespace, so the read is not decided, never a
/// read of one of them chosen by load order.
#[test]
fn flowfield_over_an_ambiguous_table_name_is_not_decided() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    workspace_only(&ws);
    app_json(
        &ws,
        &format!(
            r#"{{"id":"{DEP_GUID}","name":"B5Base","publisher":"probe","version":"1.0.0.0"}}"#
        ),
    );
    let line = r#"{"Id":60000,"Name":"B5 Line","Fields":[{"Id":1,"Name":"Header No.","TypeDefinition":{"Name":"Code"}}]}"#;
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_B5Base_1.0.0.0.app"),
        DEP_GUID,
        "B5Base",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{line}],"AppId":"{DEP_GUID}","Name":"B5Base","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    let ws = ws.to_str().unwrap();

    let out = alsem(&["query", "effects", ws, "--routine", "ReadTotals", "--all"]);
    assert!(
        out.contains("[CalcFields] on table not decided (the CalcFormula's table name matches more than one loaded table) through FlowField \"Total Amount\""),
        "{out}"
    );
    assert!(
        !out.contains("on B5 Line through FlowField"),
        "no B5 Line is chosen: {out}"
    );
}

/// A FlowField over its own table (`count("B5 Node" ...)` on "B5 Node") is
/// the same read as the `CalcFields` of the record: one row and one site, not
/// two.
#[test]
fn flowfield_over_its_own_table_is_one_read() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    workspace_only(&ws);
    let ws = ws.to_str().unwrap();

    let out = alsem(&[
        "query",
        "effects",
        ws,
        "--routine",
        "CountChildren",
        "--all",
    ]);
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("[CalcFields] on B5 Node"))
        .collect();
    assert_eq!(rows.len(), 1, "{out}");
    assert!(!rows[0].contains("through FlowField"), "{out}");
    assert!(
        answer_line(&out).starts_with("answer: 2 operations"),
        "{out}"
    );

    let out = alsem(&["query", "touches", ws, "--table", "B5 Node"]);
    assert!(
        answer_line(&out).contains("workspace sites: 2 in 1 routine — by op: CalcFields 1, Get 1"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// Round 2 (second review of the integrated tree): which record a bare
// `SetAutoCalcFields` is on, the implicit `Rec` across an object and its
// extensions, a qualified field argument, and RecordRef / FieldRef shapes.
// Built from the reviewer's probes (`w2rr-b5/ws1`, `ws2`).
// ---------------------------------------------------------------------------

/// Precondition stated in the AL: "B5 Line" is read only through the
/// FlowFields of "B5 Header" ("Total Amount", field 2; "Line Count", field 4)
/// and "B5 Owner2" ("Line Count"). Nothing else names "B5 Line".
const ROUND2: &str = r#"
table 50106 "B5 Owner2"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Line" where("Header No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { Clustered = true; } }

    // The bare call is on THIS table's Rec; Other is another record.
    procedure NoWithOther(): Integer
    var
        Other: Record "B5 Owner2";
    begin
        SetAutoCalcFields("Line Count");
        Other.SetRange("No.", 'A');
        if Other.FindFirst() then
            exit(1);
    end;

    // The implicit Rec set up in one procedure, retrieved bare in another.
    procedure TabPrep()
    begin
        SetAutoCalcFields("Line Count");
    end;

    procedure TabFetchBare(): Integer
    begin
        if FindSet() then
            exit("Line Count");
    end;
}

table 50107 "B5 Plain"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Line" where("Header No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

// The table's Rec is shared with its extension's procedures.
tableextension 50108 "B5 Plain Ext" extends "B5 Plain"
{
    procedure ExtFetch(): Integer
    begin
        if FindSet() then
            exit("Line Count");
    end;
}

table 50109 "B5 Plain2"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("B5 Line" where("Header No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { Clustered = true; } }

    procedure BasePrep()
    begin
        SetAutoCalcFields("Line Count");
    end;
}

tableextension 50110 "B5 Plain2 Ext" extends "B5 Plain2"
{
    procedure Ext2Fetch(): Integer
    begin
        if FindSet() then
            exit("Line Count");
    end;
}

page 50111 "B5 Card"
{
    SourceTable = "B5 Plain";
    layout { area(Content) { field(No; Rec."No.") { } } }
    trigger OnOpenPage()
    begin
        Rec.SetAutoCalcFields("Line Count");
    end;
}

// The action runs on the extended page's Rec.
pageextension 50112 "B5 Card Ext" extends "B5 Card"
{
    actions
    {
        addlast(Processing)
        {
            action(ExtFetchAction)
            {
                trigger OnAction()
                begin
                    if Rec.FindSet() then;
                end;
            }
        }
    }
}

codeunit 50113 "B5 Round2"
{
    procedure WithAuto(): Integer
    var
        X: Record "B5 Header";
    begin
        with X do begin
            SetAutoCalcFields("Line Count");
            if FindFirst() then
                exit("Line Count");
        end;
    end;

    procedure QualifiedFilter(): Boolean
    var
        X: Record "B5 Header";
    begin
        X.SetFilter(X."Total Amount", '>0');
        exit(X.FindFirst());
    end;

    procedure QualifiedCalc(): Decimal
    var
        X: Record "B5 Header";
    begin
        X.Get('A');
        X.CalcFields(X."Total Amount");
        exit(X."Total Amount");
    end;

    // An argument naming no field of a table that has FlowFields.
    procedure MissingFilter(): Boolean
    var
        X: Record "B5 Header";
    begin
        X.SetFilter(X.Missing, '>0');
        exit(X.FindFirst());
    end;

    procedure RecRefCalc(): Boolean
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B5 Header");
        RR.FindFirst();
        FR := RR.Field(4);
        FR.CalcField();
        exit(true);
    end;

    procedure RecRefAuto(): Boolean
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"B5 Header");
        RR.SetAutoCalcFields(4);
        exit(RR.FindFirst());
    end;

    procedure FieldRefFilter(): Boolean
    var
        RR: RecordRef;
        FR: FieldRef;
        H: Record "B5 Header";
    begin
        RR.Open(Database::"B5 Header");
        FR := RR.Field(H.FieldNo("Total Amount"));
        FR.SetFilter('>0');
        exit(RR.IsEmpty());
    end;

    procedure FieldRefParam(FR: FieldRef)
    begin
        FR.CalcField();
    end;

    procedure FieldRefByIndex(): Boolean
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B5 Header");
        RR.FindFirst();
        FR := RR.FieldIndex(2);
        FR.CalcField();
        exit(true);
    end;

    // FR is bound to "No." here, but Bind's `var` parameter rebinds it to
    // "Line Count" before the CalcField.
    procedure FieldRefPassed(): Boolean
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B5 Header");
        RR.FindFirst();
        FR := RR.Field(1);
        Bind(FR);
        FR.CalcField();
        exit(true);
    end;

    local procedure Bind(var FR: FieldRef)
    var
        Other: RecordRef;
    begin
        Other.Open(Database::"B5 Header");
        FR := Other.Field(4);
    end;

    // Two assignments to different fields: the first ("No.") is not the one
    // calculated.
    procedure FieldRefTwoFields(): Boolean
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B5 Header");
        RR.FindFirst();
        FR := RR.Field(1);
        FR := RR.Field(4);
        FR.CalcField();
        exit(true);
    end;

    // Two assignments of field 2 on different tables: on "B5 Node" field 2
    // is the Normal field "Parent No.", on "B5 Header" it is the FlowField
    // "Total Amount" (a sum over "B5 Line").
    procedure FieldRefTwoTables(): Boolean
    var
        NodeRR: RecordRef;
        HeaderRR: RecordRef;
        FR: FieldRef;
    begin
        NodeRR.Open(Database::"B5 Node");
        HeaderRR.Open(Database::"B5 Header");
        HeaderRR.FindFirst();
        FR := NodeRR.Field(2);
        FR := HeaderRR.Field(2);
        FR.CalcField();
        exit(true);
    end;
}
"#;

fn round2(dir: &Path) {
    workspace_only(dir);
    write(&dir.join("src/Round2.al"), ROUND2);
}

fn assert_down(ws: &str, from: &str, want: &str, why: &str) {
    let out = touches_down(ws, "B5 Line", from);
    assert!(
        answer_line(&out).starts_with(&format!("answer: down: {want}")),
        "{from}: {why}: {out}"
    );
}

/// A bare `SetAutoCalcFields` is a call on the record of its implicit frame:
/// this table's `Rec`, or the `X` of `with X do`. It never reaches another
/// record's retrieval (the first round's blanket fallback did).
#[test]
fn bare_set_auto_calc_fields_is_on_its_implicit_record() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    round2(&ws);
    let ws = ws.to_str().unwrap();
    assert_down(
        ws,
        "NoWithOther",
        "no",
        "Rec's SetAutoCalcFields does not apply to Other",
    );
    assert_down(
        ws,
        "WithAuto",
        "yes",
        "inside `with X do`, the bare call is on X",
    );
}

/// The implicit `Rec` is one record across the routines of a table, of a
/// page, and of every extension of the same base object.
#[test]
fn implicit_rec_is_shared_across_an_object_and_its_extensions() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    round2(&ws);
    let ws = ws.to_str().unwrap();
    assert_down(ws, "TabFetchBare", "yes", "TabPrep set up the same Rec");
    // The fixture's one action trigger (the extension's ExtFetchAction).
    let out = touches_down(ws, "B5 Line", "OnAction");
    assert!(
        out.contains("PageExtension \"B5 Card Ext\".action(ExtFetchAction)::OnAction"),
        "{out}"
    );
    assert!(
        answer_line(&out).starts_with("answer: down: yes"),
        "the page's OnOpenPage set up the Rec the extension's action retrieves: {out}"
    );
    assert_down(
        ws,
        "Ext2Fetch",
        "yes",
        "the table's BasePrep set up the Rec the extension's procedure retrieves",
    );
    // The table "B5 Plain" itself does nothing, but its page's OnOpenPage
    // sets up a PAGE's Rec, not every "B5 Plain" record: the table
    // extension's procedure is a different record.
    assert_down(
        ws,
        "ExtFetch",
        "no",
        "no SetAutoCalcFields on the table's own Rec",
    );
}

/// A field argument written with its record names the field after the dot.
/// An argument that names no field of a table with FlowFields is not decided.
#[test]
fn qualified_and_unknown_field_arguments() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    round2(&ws);
    let ws = ws.to_str().unwrap();
    assert_down(
        ws,
        "QualifiedFilter",
        "yes",
        "X.\"Total Amount\" is the FlowField",
    );
    let out = alsem(&[
        "query",
        "effects",
        ws,
        "--routine",
        "QualifiedCalc",
        "--all",
    ]);
    assert!(
        out.contains("[CalcFields] on B5 Line through FlowField \"Total Amount\""),
        "X.CalcFields(X.\"Total Amount\") reads B5 Line, decided: {out}"
    );
    assert_down(
        ws,
        "MissingFilter",
        "unknown",
        "a filter on no known field of a table with FlowFields is not decided",
    );
}

/// RecordRef and FieldRef shapes: `RecRef.SetAutoCalcFields(<number>)`,
/// `FieldRef.CalcField()` and a FieldRef filter on a FlowField.
#[test]
fn record_ref_and_field_ref_flowfield_reads() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    round2(&ws);
    let ws = ws.to_str().unwrap();

    assert_down(
        ws,
        "RecRefCalc",
        "yes",
        "FR := RR.Field(4); FR.CalcField() calculates \"Line Count\"",
    );
    assert_down(
        ws,
        "FieldRefFilter",
        "yes",
        "a FieldRef filter on \"Total Amount\"",
    );
    // A FieldRef whose table is not known: a read of a table not resolved.
    assert_down(
        ws,
        "FieldRefParam",
        "unknown",
        "a FieldRef parameter's table is decided by the caller",
    );
    // A FieldRef by index on a table with FlowFields: not decided.
    assert_down(
        ws,
        "FieldRefByIndex",
        "unknown",
        "FieldIndex(2) does not say which field",
    );
    // A local FieldRef passed to a call: a `var` parameter can rebind it, so
    // its own assignment does not decide the field.
    assert_down(
        ws,
        "FieldRefPassed",
        "unknown",
        "Bind(FR) can rebind FR to \"Line Count\"",
    );
    // Assignments that disagree decide neither the field nor the table.
    assert_down(
        ws,
        "FieldRefTwoFields",
        "unknown",
        "FR is \"No.\" or \"Line Count\"",
    );
    assert_down(
        ws,
        "FieldRefTwoTables",
        "unknown",
        "field 2 of \"B5 Node\" or of \"B5 Header\"",
    );

    // Field 4 of "B5 Header" is "Line Count": found by its number.
    let out = alsem(&["query", "effects", ws, "--routine", "RecRefAuto", "--all"]);
    assert!(
        out.contains("[CalcFields] on B5 Line through FlowField \"Line Count\""),
        "{out}"
    );

    // The FieldRef read reaches the capability cone and table mode too.
    let out = alsem(&[
        "digest",
        ws,
        "--routine",
        "RecRefCalc",
        "--include-reads",
        "--schema",
        "2",
        "--format",
        "json",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let entry = &doc["payload"]["entries"][0];
    let line_reads: Vec<&serde_json::Value> = entry["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["type"] == "DB_READ" && f["detail"]["resourceDisplay"] == "B5 Line")
        .collect();
    assert_eq!(line_reads.len(), 1, "{out}");
    // Its evidence cites the CalcField's own line (`FR.CalcField();`, line
    // 166 of Round2.al), not `unavailable`.
    let ev = &line_reads[0]["evidence"];
    assert_eq!(ev["sourceKind"], "source", "{out}");
    assert_eq!(ev["file"], "ws:src/Round2.al", "{out}");
    assert_eq!(ev["line"], 166, "{out}");
    assert_eq!(entry["entryDiagnostics"], serde_json::json!([]), "{out}");
    let out = alsem(&["query", "touches", ws, "--table", "B5 Line"]);
    assert!(
        out.contains("RecRefCalc"),
        "table mode lists the FieldRef CalcField site: {out}"
    );
}
