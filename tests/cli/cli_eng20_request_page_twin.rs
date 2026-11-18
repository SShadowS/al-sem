//! ENG-20 (engine limits L20): a report procedure with a request-page trigger's
//! name and parameters (`procedure OnOpenPage()` beside the request page's
//! `trigger OnOpenPage()`, which alc accepts) used to share the trigger's routine
//! id, and the build kept one of the two bodies. Re-probed on master `a73b4bf6`,
//! both declaration orders: a run answered `writes-table: no` for the trigger's
//! write, and a call of the procedure read the trigger's body (procedure first:
//! its own write `no`, the trigger's `yes`) or answered `unknown` (trigger first).
//!
//! Since ENG-20 a request page's own triggers have the request page as their
//! member, so the two routines have distinct ids. Each table below is written in
//! exactly one of the two routines; every answer is read through the CLI.

use serde_json::Value;
use std::process::Command;

const TABLES: &str = r#"table 50950 "RP Trig Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50951 "RP Proc Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50953 "RP Caller"
{
    procedure RunIt()
    begin
        Report.Run(Report::"RP Twin");
    end;
    procedure CallProc()
    var
        R: Report "RP Twin";
    begin
        R.OnOpenPage();
    end;
}
"#;

const PROCEDURE: &str = r#"    procedure OnOpenPage()
    var
        L: Record "RP Proc Log";
    begin
        L.Insert();
    end;
"#;

const REQUEST_PAGE: &str = r#"    requestpage
    {
        trigger OnOpenPage()
        var
            L: Record "RP Trig Log";
        begin
            L.Insert();
        end;
    }
"#;

/// The report with the procedure declared before (`procedure_first`) or after
/// its request page.
fn workspace(procedure_first: bool) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e20e20e2-0000-4000-8000-0000000000aa","name":"ENG20","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    let (a, b) = if procedure_first {
        (PROCEDURE, REQUEST_PAGE)
    } else {
        (REQUEST_PAGE, PROCEDURE)
    };
    let report = format!(
        "report 50952 \"RP Twin\"\n{{\n    ProcessingOnly = true;\n    dataset {{ dataitem(Num; Integer) {{ }} }}\n{a}{b}}}\n"
    );
    std::fs::write(t.path().join("src/rp.al"), format!("{TABLES}{report}")).unwrap();
    t
}

/// `prove <caller> writes-table:<table>`: the answer and its evidence's
/// conditionalities.
fn writes(ws: &std::path::Path, routine: &str, table: &str) -> (String, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            ws.to_str().unwrap(),
            &format!("RP Caller::{routine}"),
            &format!("writes-table:{table}"),
            "--single-app",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let r = &v["payload"]["result"];
    let conds = r["evidence"]
        .as_array()
        .map(|ev| {
            ev.iter()
                .map(|e| e["conditionality"].as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default();
    (r["answer"].as_str().unwrap_or("?").to_string(), conds)
}

/// ENG-20 option C: a request page's own trigger is told apart by a flag, not by its
/// member's text, so a dataitem NAMED `RequestPage` (alc 18.0.41 compiles this report)
/// keeps its own `OnAfterGetRecord` beside the request page's (a request page with a
/// `SourceTable`). A run reaches both writes; under a text-only member the two shared one
/// id and one write was lost.
#[test]
fn a_dataitem_named_request_page_stays_apart_from_the_request_page() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e20e20e2-0000-4000-8000-0000000000bb","name":"ENG20b","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/rp.al"),
        r#"table 50960 "RP Item Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50961 "RP Page Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50962 "RP Src" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
report 50963 "RP Collide"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(RequestPage; Integer)
        {
            trigger OnAfterGetRecord()
            var
                L: Record "RP Item Log";
            begin
                L.Insert();
            end;
        }
    }
    requestpage
    {
        SourceTable = "RP Src";
        trigger OnAfterGetRecord()
        var
            L: Record "RP Page Log";
        begin
            L.Insert();
        end;
    }
}
codeunit 50953 "RP Caller"
{
    procedure RunIt()
    begin
        Report.Run(Report::"RP Collide");
    end;
}
"#,
    )
    .unwrap();
    for table in ["RP Item Log", "RP Page Log"] {
        assert_eq!(
            writes(t.path(), "RunIt", table),
            ("yes".to_string(), vec!["conditional".to_string()]),
            "a run reaches the {table} write"
        );
    }
}

/// The collide report (a dataitem NAMED `RequestPage` beside the request page): its two
/// `OnAfterGetRecord` triggers, as `alsem graph` writes them, keep apart in the node id
/// (the wire id bc-dev-mcp joins edges and selects tests by) and the stable id (Fable and
/// the id-shape audit, ENG-20 review: the wire id ignored the flag).
#[test]
fn graph_keeps_a_dataitem_named_request_page_apart_from_the_request_page() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e20e20e2-0000-4000-8000-0000000000bb","name":"ENG20b","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/rp.al"),
        r#"report 50963 "RP Collide"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(RequestPage; Integer)
        {
            trigger OnAfterGetRecord() begin end;
        }
    }
    requestpage
    {
        trigger OnAfterGetRecord() begin end;
    }
}
"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("graph")
        .arg(t.path())
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let nodes: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("jsonl"))
        .filter(|v| v["t"] == "node" && v["v"]["memberName"] == "OnAfterGetRecord")
        .map(|v| v["v"].clone())
        .collect();
    assert_eq!(
        nodes.len(),
        2,
        "precondition: both triggers are nodes: {nodes:#?}"
    );
    assert_ne!(nodes[0]["id"], nodes[1]["id"], "wire ids: {nodes:#?}");
    // sol: both stable ids present (a missing one is not "different").
    assert!(
        nodes.iter().all(|n| n["stableRoutineId"].is_string()),
        "precondition: both stable ids: {nodes:#?}"
    );
    assert_ne!(
        nodes[0]["stableRoutineId"], nodes[1]["stableRoutineId"],
        "stable ids: {nodes:#?}"
    );
}

/// A table field NAMED `RequestPage`: the unquoted selector form still names its trigger
/// (no request page's trigger matches, so `.RequestPage` falls back to the field form),
/// and so does the quoted form the display prints (Fable, ENG-20 review).
#[test]
fn a_field_named_request_page_is_still_selected() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e20e20e2-0000-4000-8000-0000000000cc","name":"ENG20c","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/t.al"),
        r#"table 50970 "RP Field Table"
{
    fields
    {
        field(1; RequestPage; Integer)
        {
            trigger OnValidate() begin end;
        }
    }
    keys { key(PK; RequestPage) { } }
}
"#,
    )
    .unwrap();
    for selector in [
        r#"Table "RP Field Table".RequestPage::OnValidate"#,
        r#"Table "RP Field Table"."RequestPage"::OnValidate"#,
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("explain")
            .arg(format!("--routine={selector}"))
            .args(["--single-app", "--format=json", "--"])
            .arg(t.path())
            .env("RUST_LOG", "off")
            .output()
            .expect("spawn alsem");
        assert!(
            out.status.success(),
            "{selector}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(
            v["cards"].as_array().map(Vec::len),
            Some(1),
            "{selector} names the field's trigger: {v:#}"
        );
    }
}

/// ENG-20: explain names a request page's own trigger as the display prints it
/// (`Report "RP Twin".requestpage::OnOpenPage`), and a page action whose `RunObject` runs
/// the report reaches it (explain's `reachedFrom`, which reads the trigger's level).
#[test]
fn explain_selects_a_request_page_trigger_reached_from_a_page_action() {
    let t = workspace(true);
    std::fs::write(
        t.path().join("src/launcher.al"),
        r#"page 50965 "RP Launcher"
{
    PageType = Card;
    actions
    {
        area(Processing)
        {
            action(RunTwin)
            {
                RunObject = report "RP Twin";
            }
        }
    }
}
"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("explain")
        .arg(r#"--routine=Report "RP Twin".requestpage::OnOpenPage"#)
        .args(["--single-app", "--format=json", "--"])
        .arg(t.path())
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let cards = v["cards"].as_array().expect("cards");
    assert_eq!(cards.len(), 1, "one routine named: {v:#}");
    assert!(
        cards[0]["routine"]["display"]
            .as_str()
            .is_some_and(|d| d.starts_with(r#"Report "RP Twin".requestpage::OnOpenPage"#)),
        "{:#}",
        cards[0]["routine"]
    );
    assert_eq!(
        cards[0]["reachedFromSummary"]["actions"], 1,
        "the RunTwin action reaches it: {:#}",
        cards[0]
    );
}

/// In either declaration order: a run reaches the request page's trigger (and its
/// write is conditional: `UseRequestPage` false skips `OnOpenPage`), never the
/// procedure; a call of the procedure reaches the procedure's own body, never the
/// trigger's.
#[test]
fn a_report_procedure_and_its_request_pages_trigger_stay_apart() {
    for procedure_first in [true, false] {
        let t = workspace(procedure_first);
        let order = if procedure_first {
            "procedure first"
        } else {
            "trigger first"
        };
        let ws = t.path();
        assert_eq!(
            writes(ws, "RunIt", "RP Trig Log"),
            ("yes".to_string(), vec!["conditional".to_string()]),
            "{order}: a run reaches the request page's OnOpenPage"
        );
        assert_eq!(
            writes(ws, "RunIt", "RP Proc Log").0,
            "no",
            "{order}: a run does not call the procedure"
        );
        assert_eq!(
            writes(ws, "CallProc", "RP Proc Log"),
            (
                "yes".to_string(),
                vec!["unconditional-on-success".to_string()]
            ),
            "{order}: a call of the procedure runs its own body"
        );
        assert_eq!(
            writes(ws, "CallProc", "RP Trig Log").0,
            "no",
            "{order}: a call of the procedure does not run the trigger"
        );
    }
}
