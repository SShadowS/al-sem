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
table 50606 "DS Side"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50607 "DS QSide"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
report 50621 "DS Trig Report"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(TrigItem; "DS Read")
        {
            trigger OnAfterGetRecord()
            var
                S: Record "DS Side";
            begin
                S.Init();
                S.Insert();
            end;
        }
    }
}
report 50622 "DS Own Exec"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(OwnItem; "DS Read") { }
    }
    procedure Execute(N: Integer)
    begin
    end;
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
query 50631 "DS Trig Query"
{
    elements
    {
        dataitem(TQ; "DS Read")
        {
            column(K; K) { }
        }
    }
    trigger OnBeforeOpen()
    var
        S: Record "DS QSide";
    begin
        S.Init();
        S.Insert();
    end;
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

    procedure SaveTrig()
    begin
        Report.SaveAsPdf(Report::"DS Trig Report", 'x.pdf');
    end;

    procedure ExecTrig()
    begin
        Report.Execute(Report::"DS Trig Report", '');
    end;

    procedure SaveTrigVariable()
    var
        R: Report "DS Trig Report";
    begin
        R.SaveAsPdf('x.pdf');
    end;

    procedure ExecOwnProcedure()
    var
        R: Report "DS Own Exec";
    begin
        R.Execute(5);
    end;

    procedure OpenTrigQuery()
    var
        Q: Query "DS Trig Query";
    begin
        Q.Open();
    end;

    procedure SaveTrigQuery(var O: OutStream)
    begin
        Query.SaveAsCsv(Query::"DS Trig Query", O);
    end;

    procedure DynSave(Id: Integer)
    begin
        Report.SaveAsPdf(Id, 'x.pdf');
    end;

    procedure DynPrint(Id: Integer)
    begin
        Report.Print(Id, '');
    end;

    procedure RunExportByArgument()
    begin
        Xmlport.Run(Xmlport::"DS Import", false, false);
    end;

    procedure RunWithRequestPage()
    begin
        Xmlport.Run(Xmlport::"DS Import", true, false);
    end;

    procedure DynQuery(Id: Integer; var O: OutStream)
    begin
        Query.SaveAsCsv(Id, O);
    end;

    procedure SaveFiltered(Id: Integer)
    var
        Line: Record "DS Line";
    begin
        SaveWithFilter(Line, Id);
    end;

    local procedure SaveWithFilter(V: Variant; Id: Integer)
    var
        RR: RecordRef;
        OS: OutStream;
    begin
        RR.GetTable(V);
        Report.SaveAs(Id, '', ReportFormat::Pdf, OS, RR);
        OnlyForKept(RR);
    end;

    local procedure OnlyForKept(var R: RecordRef)
    var
        K: Record "DS Kept";
    begin
        if R.Number <> Database::"DS Kept" then
            exit;
        K.Init();
        K.Insert();
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

/// The rows of `alsem graph <ws>` (JSONL).
fn graph_rows(ws: &Path) -> Vec<serde_json::Value> {
    alsem(ws, &["graph"])
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The id of the graph node of `object`'s `member`.
fn graph_node(rows: &[serde_json::Value], object: &str, member: &str) -> String {
    rows.iter()
        .find(|r| {
            r["t"] == "node" && r["v"]["objectName"] == object && r["v"]["memberName"] == member
        })
        .unwrap_or_else(|| panic!("no node {object}::{member}"))["v"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// B6 round 2 (the call graph bc-dev-mcp's test selection reads): an XmlPort
/// import's own insert may run the table's `OnInsert` and raises its
/// `OnAfterInsertEvent`, so `alsem graph` has those edges from the importing
/// routine. Before, `ImportStatic` had no edge to either, and a test that
/// imports into "DS Line" was not selected for a change to its `OnInsert` or
/// to "DS Subscriber"'s `LogInsert`. "DS Line" is written by no code, so
/// only the platform's insert can lead there.
#[test]
fn the_graph_follows_an_import_into_its_tables_trigger_and_events() {
    let t = workspace();
    let rows = graph_rows(t.path());
    let caller = graph_node(&rows, "DS Logic", "ImportStatic");
    let on_insert = graph_node(&rows, "DS Line", "OnInsert");
    let event = graph_node(&rows, "DS Line", "OnAfterInsertEvent");
    let out: Vec<(&str, &str)> = rows
        .iter()
        .filter(|r| r["t"] == "edge" && r["v"]["caller"] == caller.as_str())
        .map(|r| {
            (
                r["v"]["callee"].as_str().unwrap(),
                r["v"]["via"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(
        out.contains(&(on_insert.as_str(), "implicitTrigger"))
            && out.contains(&(event.as_str(), "tableEvent")),
        "{out:?}"
    );
    // An import with AutoSave off inserts nothing: no trigger edge.
    let no_save = graph_node(&rows, "DS Logic", "ImportNoSave");
    assert!(
        !rows.iter().any(|r| r["t"] == "edge"
            && r["v"]["caller"] == no_save.as_str()
            && r["v"]["via"] == "implicitTrigger"),
        "ImportNoSave"
    );
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
    // `Xmlport.Run(X, false, false)`: no request page, `Import = false`
    // exports (Learn, "Xmlport.Run method"), so DS Line is only read. With a
    // request page the user may choose import: still a write.
    let a = prove(ws, "RunExportByArgument", "writes-table:DS Line");
    assert!(a.starts_with("answer: no"), "{a}");
    let e = effects(ws, "RunExportByArgument");
    assert!(e.contains("[FindSet] on DS Line"), "{e}");
    let a = prove(ws, "RunWithRequestPage", "writes-table:DS Line");
    assert!(a.starts_with("answer: yes"), "{a}");
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

/// A report's `SaveAs*`/`Execute` (static or on a variable) and a query's
/// `Open`/`SaveAs*` run the object as `Report.Run` does: its triggers too.
/// Precondition: "DS Side" is written only by "DS Trig Report"'s
/// `OnAfterGetRecord`, "DS QSide" only by "DS Trig Query"'s `OnBeforeOpen`.
/// These calls used to resolve as catalog builtins that reach no trigger, so
/// `prove` answered a definite `no`, and a dynamic object id
/// (`Report.SaveAsPdf(Id, …)`) answered `no` for every table.
#[test]
fn a_run_by_another_name_runs_the_triggers_and_a_dynamic_one_is_unknown() {
    let t = workspace();
    let ws = t.path();
    for routine in ["SaveTrig", "ExecTrig", "SaveTrigVariable"] {
        let a = prove(ws, routine, "writes-table:DS Side");
        assert!(a.starts_with("answer: yes"), "{routine}: {a}");
    }
    for routine in ["OpenTrigQuery", "SaveTrigQuery"] {
        let a = prove(ws, routine, "writes-table:DS QSide");
        assert!(a.starts_with("answer: yes"), "{routine}: {a}");
    }
    // A procedure the report declares with that name takes the call: no run.
    let e = effects(ws, "ExecOwnProcedure");
    assert!(
        e.contains("every operation (0)") && !e.contains("platform:"),
        "{e}"
    );
    // A dynamic id may name any report or query: never a definite `no`.
    for routine in ["DynSave", "DynPrint"] {
        let a = prove(ws, routine, "writes-table:DS Side");
        assert!(
            a.starts_with("answer: unknown (blocked by: call not resolved 1)"),
            "{routine}: {a}"
        );
    }
    for routine in ["DynSave", "DynPrint", "DynQuery"] {
        let touches = alsem(
            ws,
            &[
                "query",
                "touches",
                "--table",
                "DS Read",
                "--from",
                &format!("DS Logic::{routine}"),
                "--direction",
                "down",
            ],
        );
        assert!(
            touches.contains("answer: down: unknown"),
            "{routine}: {touches}"
        );
    }
    // A report save is still a platform method for the RecordRef flow: it
    // cannot rebind the RecordRef passed to it, so `R` still holds "DS Line"
    // at `OnlyForKept`'s guard, which rules out its insert into "DS Kept".
    // (Read as a call that may rebind it, the save cost Continia DO's
    // `IssueReminder` most of its literal pruning: 418 tables, not 198.)
    let text = alsem(ws, &["explain", "--routine", "DS Logic::SaveFiltered"]);
    assert!(
        text.contains("literal arguments rule out: 1 table (2 effect rows)"),
        "{text}"
    );
}

/// The reason reaches `explain` (a `platform:` line under the write's anchor)
/// and `prove --format json` (the evidence excerpt).
#[test]
fn the_platform_reason_is_in_explain_and_prove_json() {
    let t = workspace();
    let ws = t.path();
    let reason =
        "XMLport \"DS Import\" imports into table \"DS Line\" (tableelement LineEl, AutoSave)";
    let text = alsem(ws, &["explain", "--routine", "DS Logic::ImportStatic"]);
    assert!(text.contains(&format!("platform: {reason}")), "{text}");
    let json = alsem(
        ws,
        &[
            "prove",
            "DS Logic::ImportStatic",
            "writes-table:DS Line",
            "--format",
            "json",
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let excerpts: Vec<&str> = v["payload"]["result"]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["evidence"]["excerpt"].as_str())
        .collect();
    assert!(excerpts.contains(&reason), "{excerpts:?}");
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

/// B6 round 2. Every write below is the ONLY write of its table:
/// - "BO Own" is written only by the objects' own procedures named like a
///   built-in run (`Print()`, `Execute()`, `SaveAsPdf(Integer)`,
///   `SaveAsWord(Code[250])`, the query's `Open(Integer)`);
/// - "BO Pre" only by object-level triggers that every run reaches
///   (`OnPreReport`, `OnBeforeOpen`, `OnPreXmlPort`, `OnOpenPage`);
/// - "BO Row" only by per-record triggers (a report dataitem's and a page's
///   `OnAfterGetRecord`, an XmlPort table element's `OnBeforeInsertRecord`),
///   which also hold the fixture's one `Commit`;
/// - "BO IfD" only by the automatic insert of an XmlPort whose `Direction =
///   Export` sits in an `#if` arm alone;
/// - "BO Out" only by `ViaPre`/`ViaRows`, right after a save of a report whose
///   one trigger rejects a blocked record: `OnPreReport` in "BO GuardPre",
///   a dataitem's `OnAfterGetRecord` in "BO GuardRows".
///
/// "BO Zero"'s `local procedure SaveAsHtml(Code[250])` cannot be called from
/// "BO Logic", so `R.SaveAsHtml('x')` there can only be the built-in.
///
/// The fixture compiles with alc 18.0.43.1464 (target OnPrem, for `SaveAsPdf`).
const ROUND2: &str = r#"table 50700 "BO Read" { fields { field(1; K; Integer) { } field(2; Blocked; Boolean) { } } keys { key(PK; K) { } } }
table 50701 "BO Own" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50702 "BO Pre" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50703 "BO Row" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50704 "BO IfD" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50705 "BO Out" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
report 50722 "BO GuardPre"
{
    ProcessingOnly = true;
    dataset { dataitem(I; "BO Read") { } }
    trigger OnPreReport()
    var
        C: Record "BO Read";
    begin
        C.Get(1);
        if C.Blocked then
            Error('blocked');
    end;
}
report 50723 "BO GuardRows"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(I; "BO Read")
        {
            trigger OnAfterGetRecord()
            begin
                if I.Blocked then
                    Error('blocked');
            end;
        }
    }
}
report 50720 "BO Zero"
{
    ProcessingOnly = true;
    dataset { dataitem(I; "BO Read") { } }
    trigger OnPreReport()
    var
        P: Record "BO Pre";
    begin
        P.Insert();
    end;

    procedure Print()
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;

    procedure Execute()
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;

    procedure SaveAsPdf(N: Integer)
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;

    procedure SaveAsWord(T: Code[250])
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;

    local procedure SaveAsHtml(T: Code[250])
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;
}
report 50721 "BO Rows"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(I; "BO Read")
        {
            trigger OnAfterGetRecord()
            var
                W: Record "BO Row";
            begin
                W.Insert();
                Commit();
            end;
        }
    }
    trigger OnPreReport()
    var
        P: Record "BO Pre";
    begin
        P.Insert();
    end;
}
query 50730 "BO Query"
{
    elements { dataitem(Q; "BO Read") { column(K; K) { } } }
    trigger OnBeforeOpen()
    var
        P: Record "BO Pre";
    begin
        P.Insert();
    end;

    procedure Open(N: Integer)
    var
        O: Record "BO Own";
    begin
        O.Insert();
    end;
}
xmlport 50740 "BO Xml"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(RowEl; "BO Read")
            {
                AutoSave = false;
                trigger OnBeforeInsertRecord()
                var
                    W: Record "BO Row";
                begin
                    W.Insert();
                end;
            }
        }
    }
    trigger OnPreXmlPort()
    var
        P: Record "BO Pre";
    begin
        P.Insert();
    end;
}
xmlport 50741 "BO IfDir"
{
#if CLEAN
    Direction = Export;
#endif
    schema { textelement(Root) { tableelement(IfEl; "BO IfD") { } } }
}
page 50750 "BO Page"
{
    SourceTable = "BO Read";
    trigger OnOpenPage()
    var
        P: Record "BO Pre";
    begin
        P.Insert();
    end;

    trigger OnAfterGetRecord()
    var
        W: Record "BO Row";
    begin
        W.Insert();
    end;
}
codeunit 50760 "BO Logic"
{
    procedure CallPrint()
    var
        R: Report "BO Zero";
    begin
        R.Print();
    end;

    procedure CallExec()
    var
        R: Report "BO Zero";
    begin
        R.Execute();
    end;

    procedure CallBuiltinSave()
    var
        R: Report "BO Zero";
    begin
        R.SaveAsPdf('x.pdf');
    end;

    procedure CallOwnSave()
    var
        R: Report "BO Zero";
    begin
        R.SaveAsPdf(5);
    end;

    procedure CallUndecided()
    var
        R: Report "BO Zero";
    begin
        R.SaveAsWord('x.docx');
    end;

    procedure OpenBuiltin()
    var
        Q: Query "BO Query";
    begin
        Q.Open();
    end;

    procedure OpenOwn()
    var
        Q: Query "BO Query";
    begin
        Q.Open(5);
    end;

    procedure SaveRows()
    begin
        Report.SaveAsPdf(Report::"BO Rows", 'x.pdf');
    end;

    procedure SaveRowsVariable()
    var
        R: Report "BO Rows";
    begin
        R.SaveAsPdf('x.pdf');
    end;

    procedure RunRows()
    begin
        Report.Run(Report::"BO Rows", false);
    end;

    procedure ImportRows(var S: InStream)
    begin
        Xmlport.Import(Xmlport::"BO Xml", S);
    end;

    procedure ShowPage()
    begin
        Page.RunModal(Page::"BO Page");
    end;

    procedure RunIfDir()
    begin
        Xmlport.Run(Xmlport::"BO IfDir");
    end;

    procedure CallLocalHidden()
    var
        R: Report "BO Zero";
    begin
        R.SaveAsHtml('x.html');
    end;

    procedure ViaPre()
    var
        R: Report "BO GuardPre";
        O: Record "BO Out";
    begin
        R.SaveAsPdf('x.pdf');
        O.Insert();
    end;

    procedure ViaRows()
    var
        R: Report "BO GuardRows";
        O: Record "BO Out";
    begin
        R.SaveAsPdf('x.pdf');
        O.Insert();
    end;
}
"#;

fn round2_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"60606060-0000-4000-8000-000000000062","name":"BO","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50700,"to":50799}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/Round2.al"), ROUND2).unwrap();
    t
}

/// The `answer:` line of a human prove on "BO Logic".
fn prove_bo(ws: &Path, routine: &str, question: &str) -> String {
    let text = alsem(
        ws,
        &[
            "prove",
            &format!("BO Logic::{routine}"),
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

/// A call binds to the object's own procedure when its arguments fit that
/// procedure and no built-in, to the built-in run when they fit only a
/// built-in, and is unknown when both fit and the types do not decide (alc
/// binds the Text literal to the built-in `SaveAsWord(Text)` over the
/// procedure's `Code[250]`; the engine's fail-closed pick does not claim it).
#[test]
fn a_call_binds_the_reports_own_procedure_when_its_arguments_fit_it() {
    let t = round2_workspace();
    let ws = t.path();
    // The built-in `Print`/`Execute`/`SaveAsPdf`(Text) need an argument the
    // procedure does not take, or of another type: the procedure runs, the
    // report does not.
    for routine in ["CallPrint", "CallExec", "CallOwnSave", "OpenOwn"] {
        let own = prove_bo(ws, routine, "writes-table:BO Own");
        assert!(own.starts_with("answer: yes"), "{routine}: {own}");
        let pre = prove_bo(ws, routine, "writes-table:BO Pre");
        assert!(pre.starts_with("answer: no"), "{routine}: {pre}");
    }
    // A Text argument fits only the built-in `SaveAsPdf(Text)`; no argument
    // fits only the built-in `Query.Open()`; a `local` procedure the caller
    // cannot see takes no part (AL leaves it out of overload resolution):
    // the object runs.
    for routine in ["CallBuiltinSave", "OpenBuiltin", "CallLocalHidden"] {
        let own = prove_bo(ws, routine, "writes-table:BO Own");
        assert!(own.starts_with("answer: no"), "{routine}: {own}");
        let pre = prove_bo(ws, routine, "writes-table:BO Pre");
        assert!(pre.starts_with("answer: yes"), "{routine}: {pre}");
    }
    for table in ["BO Own", "BO Pre"] {
        let a = prove_bo(ws, "CallUndecided", &format!("writes-table:{table}"));
        assert!(
            a.starts_with("answer: unknown (blocked by: call not resolved 1)"),
            "{table}: {a}"
        );
    }
    // The report's dataset runs only with the built-in.
    let effects_of = |routine: &str| {
        alsem(
            ws,
            &[
                "query",
                "effects",
                "--routine",
                &format!("BO Logic::{routine}"),
                "--all",
            ],
        )
    };
    let own = effects_of("CallPrint");
    assert!(!own.contains("platform:"), "{own}");
    let builtin = effects_of("CallBuiltinSave");
    assert!(
        builtin.contains("(platform: Report \"BO Zero\" reads table \"BO Read\""),
        "{builtin}"
    );
    // The call graph bc-dev-mcp's test selection reads: `R.Print()` is a call
    // of the procedure, so a change to it selects the tests reaching
    // `CallPrint`; it is no run of the report.
    let rows = graph_rows(ws);
    let caller = graph_node(&rows, "BO Logic", "CallPrint");
    let print = graph_node(&rows, "BO Zero", "Print");
    let out: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["t"] == "edge" && r["v"]["caller"] == caller.as_str())
        .collect();
    assert!(
        out.len() == 1 && out[0]["v"]["callee"] == print.as_str() && out[0]["v"]["via"] == "call",
        "{out:?}"
    );
}

/// A run may finish without running a per-record trigger (Learn, "Report
/// Triggers and Runtime Operations": `OnAfterGetRecord` runs once per record
/// retrieved), so what it does is conditional on the run, never
/// `unconditional on success`; the object-level trigger every run reaches
/// keeps its `unconditional`.
#[test]
fn a_per_record_trigger_is_conditional_on_the_run() {
    let t = round2_workspace();
    let ws = t.path();
    for routine in ["SaveRows", "SaveRowsVariable", "RunRows"] {
        let row = prove_bo(ws, routine, "writes-table:BO Row");
        assert!(
            row.starts_with("answer: yes") && row.ends_with("(conditional)"),
            "{routine}: {row}"
        );
        let pre = prove_bo(ws, routine, "writes-table:BO Pre");
        assert!(
            pre.ends_with("(unconditional on success)"),
            "{routine}: {pre}"
        );
        let commit = prove_bo(ws, routine, "commits-on-success-path");
        assert!(
            commit.starts_with("answer: unknown (blocked by: conditional effect exists 1)"),
            "{routine}: {commit}"
        );
    }
    for routine in ["ImportRows", "ShowPage"] {
        let row = prove_bo(ws, routine, "writes-table:BO Row");
        assert!(
            row.starts_with("answer: yes") && row.ends_with("(conditional)"),
            "{routine}: {row}"
        );
        let pre = prove_bo(ws, routine, "writes-table:BO Pre");
        assert!(
            pre.ends_with("(unconditional on success)"),
            "{routine}: {pre}"
        );
    }
    // explain's rules across calls: the rule in `OnPreReport` holds before
    // `ViaPre`'s insert; the one in `OnAfterGetRecord` may never run, so it
    // protects nothing.
    let explain = |routine: &str| {
        alsem(
            ws,
            &["explain", "--routine", &format!("BO Logic::{routine}")],
        )
    };
    let pre = explain("ViaPre");
    assert!(
        pre.contains("inherits via OnPreReport: rejects when C.Blocked: \"blocked\""),
        "{pre}"
    );
    let rows = explain("ViaRows");
    assert!(
        rows.contains("calls: Report \"BO GuardRows\".dataitem(I)::OnAfterGetRecord")
            && !rows.contains("inherits via"),
        "{rows}"
    );
}

/// A `Direction` written in one `#if` arm alone: a build that takes no arm
/// keeps the default `Both`, so `Xmlport.Run` may import.
#[test]
fn a_direction_in_an_if_arm_alone_may_still_import() {
    let t = round2_workspace();
    let a = prove_bo(t.path(), "RunIfDir", "writes-table:BO IfD");
    assert!(a.starts_with("answer: yes"), "{a}");
}
