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
