//! G33: a routine that takes a record as a `Variant`, reads its table with
//! `RecRef.GetTable(Variant)` and branches on `case RecRef.Number of
//! Database::"T": …` runs a branch only for the table its caller passes, so
//! `prove` follows the table per call path (`literal_reach`'s table frame).
//! Base Application's `ApprovalsMgmt.PreventDeletingRecordWithOpenApprovalEntry`
//! has this shape: called from a Requisition Line's `OnDelete` with its own
//! `Rec`, its `Gen. Journal Batch` branch (into the workflow engine) cannot run,
//! and on Continia DO every chain from `Sales-Calc. Discount` to a `Cust. Ledger
//! Entry` write went through it.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50390 "TG Line"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    trigger OnDelete()
    var
        Approvals: Codeunit "TG Approvals";
    begin
        Approvals.Prevent(Rec);
    end;
}
table 50391 "TG Batch"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50392 "TG Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50393 "TG Approvals"
{
    procedure Prevent(Variant: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Variant);
        case RecRef.Number of
            Database::"TG Batch":
                WriteLog();
            else
                OnElseCase(RecRef, Variant);
        end;
    end;

    procedure PreventInLoop(Variant: Variant)
    var
        RecRef: RecordRef;
        i: Integer;
    begin
        RecRef.GetTable(Variant);
        for i := 1 to 2 do
            case RecRef.Number of
                Database::"TG Batch":
                    WriteLog();
                else
                    OnElseCase(RecRef, Variant);
            end;
    end;

    [IntegrationEvent(false, false)]
    local procedure OnElseCase(var RecRef: RecordRef; Variant: Variant)
    begin
    end;

    local procedure WriteLog()
    var
        Log: Record "TG Log";
    begin
        Log.Code := 'X';
        Log.Insert();
    end;
}
codeunit 50394 "TG Callers"
{
    procedure DeleteLine()
    var
        Line: Record "TG Line";
    begin
        Line.Get('A');
        Line.Delete(true);
    end;

    procedure PreventBatch()
    var
        Batch: Record "TG Batch";
        Approvals: Codeunit "TG Approvals";
    begin
        Approvals.Prevent(Batch);
    end;

    procedure PreventLineInLoop()
    var
        Line: Record "TG Line";
        Approvals: Codeunit "TG Approvals";
    begin
        Approvals.PreventInLoop(Line);
    end;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn a_branch_on_the_passed_records_table_runs_only_for_that_table() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"39393939-0000-4000-8000-000000000039","name":"TG","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let prove = |name: &str| -> serde_json::Value {
        let id = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string();
        alsem(
            t.path(),
            &["prove", "ws", &id, "writes-table:TG Log", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    // Precondition: the batch branch writes the log.
    let batch = prove("PreventBatch");
    assert_eq!(batch["answer"], "yes", "precondition: {batch}");

    // The line passes itself: the batch branch cannot run. The `else` branch
    // passes RecRef on (an event may rebind it), but only after the branch
    // is chosen.
    let line = prove("DeleteLine");
    assert_eq!(line["answer"], "no", "{line}");

    // In a loop, the event may rebind RecRef before the case runs again.
    let looped = prove("PreventLineInLoop");
    assert_eq!(looped["answer"], "yes", "{looped}");
}

/// The same inside a dependency, seen from the workspace: rule B groups the
/// dependency's writes at the boundary call, so a group resource is dropped
/// when every operation that writes it cannot run (`dead_group_resources`).
#[test]
fn a_dead_branch_inside_a_dependency_drops_its_group_resource() {
    let lib = |passed: &str| {
        format!(
            r#"codeunit 10 "Dep Lib"
{{
    procedure Outer()
    var
        Cust: Record Customer;
        Other: Record "Dep Batch";
    begin
        Prevent({passed});
    end;

    procedure Prevent(Variant: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Variant);
        case RecRef.Number of
            Database::"Dep Batch":
                WriteCust();
        end;
    end;

    local procedure WriteCust()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}}

table 12 "Dep Batch"
{{
    fields {{ field(1; Code; Code[20]) {{ }} }}
}}
"#
        )
    };
    let prove = |passed: &str| -> serde_json::Value {
        let t = tempfile::tempdir().unwrap();
        crate::dep_dispatch_fixture::write_own_lib(t.path(), "", &lib(passed));
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(t.path())
            .args(["CallChain", "writes-table:Customer", "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))["payload"]
            ["result"]
            .clone()
    };
    // Precondition: passing a Dep Batch record writes Customer, as a group.
    let live = prove("Other");
    assert_eq!(live["answer"], "yes", "precondition: {live}");
    assert!(
        live["evidence"][0]["detail"]["boundary"].is_string(),
        "precondition: a boundary group: {live}"
    );
    // Passing a Customer: the branch cannot run.
    let dead = prove("Cust");
    assert_eq!(dead["answer"], "no", "{dead}");
}

/// G48/G46/G44: a boundary group whose crossing into the dependency cannot
/// run is dropped whole, its event and COMMIT facts included, not only its
/// table resources. The workspace calls `Dep Lib.Outer()` (which raises
/// `OnDepEvent` and commits) only past `if RecRef.Number <> Database::Customer
/// then exit;` (Continia DO: `CDO File.CalcSalesDisc`'s `Sales-Calc. Discount`
/// run, reached with a posted service document, PRNT-QUEU-006.c8).
#[test]
fn a_boundary_group_behind_a_dead_crossing_is_dropped_whole() {
    const LIB: &str = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    begin
        OnDepEvent();
        Commit();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}
"#;
    const GUARDED: &str = r#"codeunit 50180 "Ws Guarded"
{
    procedure FromCust(var Cust: Record Customer)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Cust);
        Guarded(RecRef);
    end;

    procedure FromFlag(var Flag: Record "Ws Flag")
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Flag);
        Guarded(RecRef);
    end;

    local procedure Guarded(var RecRef: RecordRef)
    var
        Lib: Codeunit "Dep Lib";
    begin
        if RecRef.Number <> Database::Customer then
            exit;
        Lib.Outer();
    end;
}
"#;
    let t = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own_lib(t.path(), GUARDED, LIB);
    let prove = |routine: &str, question: &str| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(t.path())
            .args([routine, question, "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))["payload"]
            ["result"]
            .clone()
    };
    for question in ["publishes-event:OnDepEvent", "may-commit"] {
        // Precondition: through a Customer the group runs.
        let live = prove("FromCust", question);
        assert_eq!(live["answer"], "yes", "precondition: {question}: {live}");
        assert!(
            live["evidence"][0]["detail"]["boundary"].is_string(),
            "precondition: {question}: a boundary group: {live}"
        );
        // Through a Ws Flag the crossing cannot run.
        let dead = prove("FromFlag", question);
        assert_ne!(dead["answer"], "yes", "{question}: {dead}");
    }
}
