//! G40 / G41: `prove … writes-table:<name>` answered a decided `no` for writes
//! plainly in the code.
//!
//! - G40: a table extension numbered like a real table of the same app took the
//!   table's display name in the snapshot's identity table, so the table's name
//!   matched none of its writes (Continia DO: tableextension 6175291
//!   `CDOReturnReceiptHeaderExt` against table 6175291 "CDO Cust.Template
//!   Setup"). A name or number no table has now fails the question instead of
//!   answering `no`.
//! - G41: a bare `Modify(true)` / `Validate(…)` in a report dataitem trigger
//!   operates on the dataitem's record (DO's "CDO Update Customer Setup").

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50700 "TN Cust.Template Setup"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50701 "TN Other"
{
    fields
    {
        field(1; Code; Code[20]) { }
        field(2; Flag; Boolean)
        {
            trigger OnValidate()
            var
                Setup: Record "TN Cust.Template Setup";
            begin
                Setup.DeleteAll();
            end;
        }
    }
    keys { key(PK; Code) { } }
}
tableextension 50700 "TN Other Ext" extends "TN Other"
{
    fields { field(50700; Extra; Code[20]) { } }
}
codeunit 50702 "TN Writers"
{
    procedure ClearSetup()
    var
        Setup: Record "TN Cust.Template Setup";
    begin
        Setup.DeleteAll();
    end;
}
report 50703 "TN Update Other"
{
    ProcessingOnly = true;

    dataset
    {
        dataitem(Other; "TN Other")
        {
            trigger OnAfterGetRecord()
            begin
                Validate(Flag, true);
                Modify(true);
            end;
        }
    }
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

fn json(out: &std::process::Output, args: &str) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn writes_table_names_resolve_and_an_unknown_name_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"40404040-0000-4000-8000-000000000040","name":"TN","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = json(
        &alsem(
            t.path(),
            &["fingerprint", "ws", "--inventory-only", "--format", "json"],
        ),
        "fingerprint",
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |routine: &str, question: &str| -> std::process::Output {
        alsem(
            t.path(),
            &["prove", "ws", &id_of(routine), question, "--deterministic"],
        )
    };
    let answer = |routine: &str, question: &str| -> String {
        let out = prove(routine, question);
        json(&out, question)["payload"]["result"]["answer"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // G40: the table's own name and number both find the DeleteAll, though
    // tableextension 50700 shares the table's number.
    assert_eq!(answer("ClearSetup", "writes-table:50700"), "yes");
    assert_eq!(
        answer("ClearSetup", "writes-table:TN Cust.Template Setup"),
        "yes"
    );

    // G40: no table of that name: an error, never a `no`.
    let unknown = prove("ClearSetup", "writes-table:TN No Such Table");
    assert_eq!(unknown.status.code(), Some(1), "{unknown:?}");
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("unknown table 'TN No Such Table'"),
        "{unknown:?}"
    );

    // G41: the dataitem trigger's bare Modify writes its record, and its bare
    // Validate runs the field trigger (which deletes the setup).
    assert_eq!(answer("OnAfterGetRecord", "writes-table:TN Other"), "yes");
    assert_eq!(
        answer("OnAfterGetRecord", "writes-table:TN Cust.Template Setup"),
        "yes"
    );
}
