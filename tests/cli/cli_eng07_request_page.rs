//! ENG-07 (engine limits L20): a report's request-page triggers. A run of a
//! report loads its request page, so the page's `OnInit` runs, and its
//! `OnOpenPage`/`OnQueryClosePage`/`OnClosePage` run when the page shows (and
//! `OnOpenPage`/`OnClosePage` under a head-less `Execute`). Before the fix no
//! request-page trigger was followed, so `prove writes-table` answered a
//! definite `no` for a write in one of them through `Report.Run`.
//!
//! Which trigger runs in which run was measured on the BC 28.4 Linux sidecar
//! (Windows confirmation pending): `OnInit` in every run; `OnOpenPage` and
//! `OnClosePage` not with `UseRequestPage` false; `OnQueryClosePage` not under
//! `Execute`. So `OnInit`'s write is `unconditional-on-success`, the others'
//! `conditional`.
//!
//! Each table below is written in exactly one routine, so only following that
//! routine can make the answer `yes`; every answer is read through the CLI.

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 50900 "RP Init Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50901 "RP Open Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50902 "RP Query Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50903 "RP Close Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50904 "RP Rec Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50905 "RP Control Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50906 "RP Proc Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50907 "RP Ext Log" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
report 50910 "RP Probe"
{
    ProcessingOnly = true;
    dataset { dataitem(Num; Integer) { } }
    requestpage
    {
        SourceTable = "RP Rec Log";
        layout
        {
            area(Content)
            {
                field(Opt; Opt)
                {
                    trigger OnValidate()
                    var
                        L: Record "RP Control Log";
                    begin
                        L.Insert();
                    end;
                }
            }
        }
        trigger OnInit()
        var
            L: Record "RP Init Log";
        begin
            L.Insert();
        end;
        trigger OnOpenPage()
        var
            L: Record "RP Open Log";
        begin
            L.Insert();
        end;
        trigger OnQueryClosePage(CloseAction: Action): Boolean
        var
            L: Record "RP Query Log";
        begin
            L.Insert();
            exit(true);
        end;
        trigger OnClosePage()
        var
            L: Record "RP Close Log";
        begin
            L.Insert();
        end;
        // Not `Rec.Insert()`: the request page's `SourceTable` does not type
        // `Rec` yet, so that write is a table not known (`unknown`, never `no`).
        trigger OnAfterGetRecord()
        var
            L: Record "RP Rec Log";
        begin
            L.Insert();
        end;
    }
    var
        Opt: Boolean;
}
report 50911 "RP Proc Probe"
{
    ProcessingOnly = true;
    dataset { dataitem(Num; Integer) { } }
    // A procedure, not a trigger: nothing calls it, so a run never runs it.
    procedure OnOpenPage()
    var
        L: Record "RP Proc Log";
    begin
        L.Insert();
    end;
}
report 50912 "RP Ext Base"
{
    ProcessingOnly = true;
    dataset { dataitem(Num; Integer) { } }
}
reportextension 50913 "RP Ext" extends "RP Ext Base"
{
    requestpage
    {
        trigger OnOpenPage()
        var
            L: Record "RP Ext Log";
        begin
            L.Insert();
        end;
    }
}
codeunit 50920 "RP Caller"
{
    procedure ViaRun()
    begin
        Report.Run(Report::"RP Probe");
    end;
    procedure ViaRunModal()
    begin
        Report.RunModal(Report::"RP Probe", false);
    end;
    procedure ViaExecute(P: Text)
    begin
        Report.Execute(Report::"RP Probe", P);
    end;
    procedure ViaVar()
    var
        R: Report "RP Probe";
    begin
        R.RunModal();
    end;
    procedure RunProc()
    begin
        Report.Run(Report::"RP Proc Probe");
    end;
    procedure RunExtBase()
    begin
        Report.Run(Report::"RP Ext Base");
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e07e07e0-0000-4000-8000-0000000000aa","name":"ENG07","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/rp.al"), SOURCE).unwrap();
    t
}

fn writes(ws: &str, routine: &str, table: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            ws,
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
    v["payload"]["result"].clone()
}

/// The one write's conditionality, after asserting the answer is `yes`.
fn yes_with(r: &Value) -> String {
    assert_eq!(r["answer"], "yes", "{r:#}");
    let ev = r["evidence"].as_array().expect("evidence");
    assert_eq!(ev.len(), 1, "{r:#}");
    ev[0]["conditionality"].as_str().unwrap().to_string()
}

/// Every run form reaches every request-page lifecycle trigger: `OnInit`
/// unconditionally, the rest conditionally.
#[test]
fn a_run_reaches_the_request_pages_triggers() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["ViaRun", "ViaRunModal", "ViaExecute", "ViaVar"] {
        let got: Vec<(&str, String)> = [
            "RP Init Log",
            "RP Open Log",
            "RP Query Log",
            "RP Close Log",
            "RP Rec Log",
        ]
        .into_iter()
        .map(|table| (table, yes_with(&writes(ws, routine, table))))
        .collect();
        assert_eq!(
            got,
            vec![
                ("RP Init Log", "unconditional-on-success".to_string()),
                ("RP Open Log", "conditional".to_string()),
                ("RP Query Log", "conditional".to_string()),
                ("RP Close Log", "conditional".to_string()),
                ("RP Rec Log", "conditional".to_string()),
            ],
            "{routine}"
        );
    }
}

/// A request-page control's `OnValidate` runs only when a user edits the
/// field: a run does not reach it.
#[test]
fn a_run_does_not_reach_a_request_page_controls_trigger() {
    let t = workspace();
    let r = writes(t.path().to_str().unwrap(), "ViaRun", "RP Control Log");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// A report procedure named like a request-page trigger is not one: a run
/// does not call it.
#[test]
fn a_report_procedure_named_like_a_trigger_is_not_run() {
    let t = workspace();
    let r = writes(t.path().to_str().unwrap(), "RunProc", "RP Proc Log");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// A report extension's request-page trigger runs with the base report.
#[test]
fn a_run_reaches_a_report_extensions_request_page_trigger() {
    let t = workspace();
    let r = writes(t.path().to_str().unwrap(), "RunExtBase", "RP Ext Log");
    assert_eq!(yes_with(&r), "conditional");
}
