//! B6 (engine limits L20): the platform's own dataset operations. A report's
//! dataitems read their tables when it runs, a query's when it opens, and an
//! XmlPort import inserts (and with `AutoUpdate`/`AutoReplace`, modifies) the
//! records of its table elements, with no AL code at all. `prove writes-table`
//! used to answer a definite `no` for a table written only by an XmlPort's
//! automatic insert, and `query effects` listed no read for a query.
//!
//! Each case states its precondition in the source below: the written table
//! has no code write anywhere, so only the platform's operation can make the
//! answer `yes`, and every answer is read through the CLI (the production
//! call site).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50600 "DS Line"
{
    fields { field(1; K; Integer) { } field(2; Amount; Decimal) { } }
    keys { key(PK; K) { } }
    trigger OnInsert()
    var
        A: Record "DS Audit";
    begin
        A.Init();
        A.Insert();
    end;
}
table 50601 "DS Audit"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50602 "DS Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50603 "DS Kept"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50604 "DS Price"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50605 "DS Read"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
xmlport 50610 "DS Import"
{
    schema
    {
        textelement(Root)
        {
            tableelement(LineEl; "DS Line")
            {
                fieldelement(Amt; LineEl.Amount) { }
            }
        }
    }
}
xmlport 50611 "DS No Save"
{
    schema
    {
        textelement(Root)
        {
            tableelement(KeptEl; "DS Kept")
            {
                AutoSave = false;
            }
        }
    }
}
xmlport 50612 "DS Update"
{
    schema
    {
        textelement(Root)
        {
            tableelement(PriceEl; "DS Price")
            {
                AutoUpdate = true;
            }
        }
    }
}
xmlport 50613 "DS Export Only"
{
    Direction = Export;
    schema
    {
        textelement(Root)
        {
            tableelement(KeptEl; "DS Kept") { }
        }
    }
}
report 50620 "DS Report"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(ReadItem; "DS Read") { }
        dataitem(TempItem; "DS Kept") { UseTemporary = true; }
    }
}
query 50630 "DS Query"
{
    elements
    {
        dataitem(Q; "DS Read")
        {
            column(K; K) { }
        }
    }
}
codeunit 50640 "DS Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"DS Line", 'OnAfterInsertEvent', '', false, false)]
    local procedure LogInsert(var Rec: Record "DS Line"; RunTrigger: Boolean)
    var
        L: Record "DS Log";
    begin
        L.Init();
        L.Insert();
    end;
}
codeunit 50650 "DS Logic"
{
    procedure ImportStatic()
    begin
        Xmlport.Import(Xmlport::"DS Import");
    end;

    procedure ImportVariable()
    var
        Xp: XmlPort "DS Import";
    begin
        Xp.Import();
    end;

    procedure ImportNoSave()
    begin
        Xmlport.Import(Xmlport::"DS No Save");
    end;

    procedure ImportUpdate()
    begin
        Xmlport.Import(Xmlport::"DS Update");
    end;

    procedure RunExportOnly()
    begin
        Xmlport.Run(Xmlport::"DS Export Only");
    end;

    procedure SaveReport()
    begin
        Report.SaveAsPdf(Report::"DS Report", 'x.pdf');
    end;

    procedure RequestPageOnly(): Text
    var
        R: Report "DS Report";
    begin
        exit(R.RunRequestPage());
    end;

    procedure OpenQuery()
    var
        Q: Query "DS Query";
    begin
        Q.Open();
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"60606060-0000-4000-8000-000000000060","name":"DS","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50600,"to":50699}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/Probe.al"), SOURCE).unwrap();
    t
}

/// `alsem <command> <ws> <rest>`: the command is one word, or two after `query`.
fn alsem(ws: &Path, args: &[&str]) -> String {
    let lead = if args[0] == "query" { 2 } else { 1 };
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args[..lead])
        .arg(ws)
        .args(&args[lead..])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !text.is_empty(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    text
}

/// The `answer:` line of a human prove.
fn prove(ws: &Path, routine: &str, question: &str) -> String {
    let text = alsem(
        ws,
        &[
            "prove",
            &format!("DS Logic::{routine}"),
            question,
            "--format",
            "human",
        ],
    );
    text.lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_else(|| panic!("no answer line: {text}"))
        .to_string()
}

fn effects(ws: &Path, routine: &str) -> String {
    alsem(
        ws,
        &[
            "query",
            "effects",
            "--routine",
            &format!("DS Logic::{routine}"),
            "--all",
        ],
    )
}

#[test]
fn an_xmlport_import_writes_its_table_elements() {
    let t = workspace();
    let ws = t.path();
    // The static and the variable form both insert: no code writes DS Line.
    for routine in ["ImportStatic", "ImportVariable"] {
        let a = prove(ws, routine, "writes-table:DS Line");
        assert!(a.starts_with("answer: yes"), "{routine}: {a}");
        let e = effects(ws, routine);
        assert!(
            e.contains(
                "(platform: XMLport \"DS Import\" imports into table \"DS Line\" \
                 (tableelement LineEl, AutoSave))"
            ),
            "{routine}: {e}"
        );
        // The insert may run the table's OnInsert trigger (Learn does not say
        // it does not), and it raises the platform table events: their writes
        // are in the cone.
        for table in ["DS Audit", "DS Log"] {
            let a = prove(ws, routine, &format!("writes-table:{table}"));
            assert!(a.starts_with("answer: yes"), "{routine} {table}: {a}");
        }
    }
    // AutoSave = false: the platform writes nothing (the trigger code would).
    let a = prove(ws, "ImportNoSave", "writes-table:DS Kept");
    assert!(a.starts_with("answer: no"), "{a}");
    // AutoUpdate = true: an existing record is modified too.
    let e = effects(ws, "ImportUpdate");
    assert!(e.contains("[Insert] on DS Price"), "{e}");
    assert!(
        e.contains("[Modify] on DS Price") && e.contains("(tableelement PriceEl, AutoUpdate)"),
        "{e}"
    );
    // `Direction = Export`: a run exports, it reads and writes nothing.
    let a = prove(ws, "RunExportOnly", "writes-table:DS Kept");
    assert!(a.starts_with("answer: no"), "{a}");
    let e = effects(ws, "RunExportOnly");
    assert!(e.contains("[FindSet] on DS Kept"), "{e}");
}

#[test]
fn a_report_and_a_query_read_their_dataitems() {
    let t = workspace();
    let ws = t.path();
    let e = effects(ws, "SaveReport");
    assert!(
        e.contains(
            "[FindSet] on DS Read via direct, not temporary — Codeunit \"DS Logic\"::SaveReport"
        ) && e.contains(
            "(platform: Report \"DS Report\" reads table \"DS Read\" (dataitem ReadItem))"
        ),
        "{e}"
    );
    // `UseTemporary = true`: the dataitem's record is temporary.
    assert!(
        e.contains("[FindSet] on DS Kept via direct, temporary"),
        "{e}"
    );
    // A request page alone runs no dataset.
    let e = effects(ws, "RequestPageOnly");
    assert!(!e.contains("platform:"), "{e}");
    let e = effects(ws, "OpenQuery");
    assert!(
        e.contains("(platform: Query \"DS Query\" reads table \"DS Read\" (dataitem Q))"),
        "{e}"
    );
    // `query touches --direction down` sees the read.
    let touches = alsem(
        ws,
        &[
            "query",
            "touches",
            "--table",
            "DS Read",
            "--from",
            "DS Logic::OpenQuery",
            "--direction",
            "down",
        ],
    );
    assert!(touches.contains("FindSet"), "{touches}");
}

const DEP_GUID: &str = "62626262-0000-4000-8000-000000000062";

/// A workspace that runs a symbol-only dependency's XmlPort and query: the
/// symbol data has no dataitems and no `AutoSave`.
fn dependency_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"61616161-0000-4000-8000-000000000061","name":"DW","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"DBase","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_DBase_1.0.0.0.app"),
        DEP_GUID,
        "DBase",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":50500,"Name":"Dep T","Fields":[{{"Id":1,"Name":"K","TypeDefinition":{{"Name":"Integer"}}}}]}}],"XmlPorts":[{{"Id":50501,"Name":"Dep Port"}}],"Queries":[{{"Id":50502,"Name":"Dep Query"}}],"AppId":"{DEP_GUID}","Name":"DBase","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    std::fs::write(
        ws.join("src/W.al"),
        r#"table 50100 "W Line"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50101 "W Logic"
{
    procedure ImportDep()
    begin
        Xmlport.Import(Xmlport::"Dep Port");
    end;

    procedure OpenDep()
    var
        Q: Query "Dep Query";
    begin
        Q.Open();
    end;
}
"#,
    )
    .unwrap();
    t
}

#[test]
fn a_symbol_only_dataset_is_not_known_never_empty() {
    let t = dependency_workspace();
    let ws = t.path();
    for single in [false, true] {
        let mode: &[&str] = if single { &["--single-app"] } else { &[] };
        // An import of an XmlPort whose table elements are not known may write
        // any table: `unknown`, with the not-resolved writes named.
        let mut args = vec![
            "prove",
            "W Logic::ImportDep",
            "writes-table:W Line",
            "--format",
            "human",
        ];
        args.extend(mode);
        let text = alsem(ws, &args);
        assert!(text.contains("answer: unknown"), "single={single}: {text}");
        assert!(
            text.contains("DB_INSERT on a table that is not known @ ws:src/W.al:10"),
            "single={single}: {text}"
        );
        // A query whose dataitems are not known reads tables nobody can name:
        // the read is listed as not resolved, with its reason, never dropped.
        let mut args = vec!["query", "effects", "--routine", "W Logic::OpenDep", "--all"];
        args.extend(mode);
        let text = alsem(ws, &args);
        assert!(
            text.contains("table not resolved: 1 operation in 1 routine (FindSet 1)")
                && text.contains(
                    "(platform: Query \"Dep Query\" reads tables its dataset names, and its \
                     dataset is not known (the symbol data has no dataitems))"
                ),
            "single={single}: {text}"
        );
    }
}
