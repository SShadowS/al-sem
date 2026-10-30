//! G34: `prove` follows the table a RecordRef holds into `if R.Number <>
//! Database::"T" then exit;`, through `GetTable`, `Duplicate()`, parameters and
//! a codeunit global of an instance the caller owns
//! (`literal_reach` + `recref_flow`).
//!
//! Continia DO's `VendorLedgerPrint` puts a Vendor Ledger Entry in a RecordRef,
//! hands it to the e-mail handler (`SetRecRefFilter` keeps it in a global) and
//! prints; the print path copies the global and reaches `CDO File.CalcSalesDisc`,
//! which exits unless the RecordRef holds a Sales Header. Its `Sales-Calc.
//! Discount` run cannot happen from there, but `prove … writes-table:Vendor`
//! answered `yes` through it (HAND-PSV-004.c5).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50400 "RG Doc"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50401 "RG Entry"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50402 "RG Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50403 "RG Handler"
{
    var
        RecRefFilter: RecordRef;

    procedure SetRecRefFilter(var RecRef: RecordRef)
    begin
        RecRefFilter := RecRef;
    end;

    procedure Print(VariantRec: Variant)
    begin
        RunIt(VariantRec);
    end;

    procedure PrintViaEvent(VariantRec: Variant)
    var
        RecRef: RecordRef;
        Output: Codeunit "RG Output";
    begin
        RecRef.GetTable(VariantRec);
        Output.CreateWithEvent(RecRef);
    end;

    procedure PrintFromSet(VariantRec: Variant)
    var
        RecRef: RecordRef;
        Output: Codeunit "RG Output";
    begin
        RecRef.GetTable(VariantRec);
        Output.CreateFromSet(RecRef);
    end;

    local procedure RunIt(VariantRec: Variant)
    var
        RecRef: RecordRef;
        Output: Codeunit "RG Output";
    begin
        if RecRefFilter.Number > 0 then
            RecRef := RecRefFilter.Duplicate()
        else
            RecRef.GetTable(VariantRec);
        Output.Create(RecRef);
    end;
}
codeunit 50404 "RG Output"
{
    procedure Create(var RecordFilter: RecordRef)
    begin
        CalcDisc(RecordFilter);
    end;

    procedure CreateWithEvent(var RecordFilter: RecordRef)
    begin
        OnBeforeCreate(RecordFilter);
        CalcDisc(RecordFilter);
    end;

    procedure CreateFromSet(var RecordFilter: RecordRef)
    var
        Log: Record "RG Log";
    begin
        // G43: the set form of the exit guard.
        if not (RecordFilter.Number in [Database::"RG Doc", Database::"RG Log"]) then
            exit;
        Log.Code := 'S';
        Log.Insert();
    end;

    local procedure CalcDisc(var RecRef: RecordRef)
    var
        Doc: Record "RG Doc";
        Log: Record "RG Log";
    begin
        if RecRef.Number <> Database::"RG Doc" then
            exit;
        RecRef.SetTable(Doc);
        Log.Code := Doc.Code;
        Log.Insert();
        RecRef.GetTable(Doc);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeCreate(var RecordFilter: RecordRef)
    begin
    end;
}
codeunit 50405 "RG Reopener"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RG Output", 'OnBeforeCreate', '', false, false)]
    local procedure Reopen(var RecordFilter: RecordRef)
    begin
        RecordFilter.Open(Database::"RG Doc");
    end;
}
codeunit 50406 "RG Pages"
{
    procedure PrintEntry(var Entry: Record "RG Entry")
    var
        Handler: Codeunit "RG Handler";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Entry);
        Handler.SetRecRefFilter(RecRef);
        Handler.Print(Entry);
    end;

    procedure PrintDoc(var Doc: Record "RG Doc")
    var
        Handler: Codeunit "RG Handler";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Doc);
        Handler.SetRecRefFilter(RecRef);
        Handler.Print(Doc);
    end;

    procedure PrintEntryReopened(var Entry: Record "RG Entry")
    var
        Handler: Codeunit "RG Handler";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Entry);
        Handler.SetRecRefFilter(RecRef);
        Reopen(RecRef);
        Handler.Print(Entry);
    end;

    procedure PrintEntryViaEvent(var Entry: Record "RG Entry")
    var
        Handler: Codeunit "RG Handler";
    begin
        Handler.PrintViaEvent(Entry);
    end;

    procedure PrintEntryFromSet(var Entry: Record "RG Entry")
    var
        Handler: Codeunit "RG Handler";
    begin
        Handler.PrintFromSet(Entry);
    end;

    procedure PrintDocFromSet(var Doc: Record "RG Doc")
    var
        Handler: Codeunit "RG Handler";
    begin
        Handler.PrintFromSet(Doc);
    end;

    procedure PrintEntryAliased(var Entry: Record "RG Entry")
    var
        Handler: Codeunit "RG Handler";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Entry);
        AliasReopen(RecRef);
        Handler.SetRecRefFilter(RecRef);
        Handler.Print(Entry);
    end;

    local procedure Reopen(var RecRef: RecordRef)
    begin
        RecRef.Open(Database::"RG Doc");
    end;

    local procedure AliasReopen(RecRef: RecordRef)
    var
        Alias: RecordRef;
    begin
        Alias := RecRef;
        Alias.Open(Database::"RG Doc");
    end;
}
"#;

/// G35: Base Application's workflow loop (`ExecuteResponses`) passes one
/// Variant to every response by `var`. `ReleaseDocument` swaps it for the
/// approved record only in its `Database::"Approval Entry"` branch, and
/// `PostDocument` posts only a document. From a requisition batch neither
/// branch can run, however often the loop goes round.
const LOOP_SOURCE: &str = r#"table 50410 "CF Batch"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50411 "CF Approval"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50412 "CF Doc"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50413 "CF Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50414 "CF Responses"
{
    procedure Responses(Variant: Variant)
    var
        Variant1: Variant;
        i: Integer;
    begin
        Variant1 := Variant;
        for i := 1 to 3 do
            Execute(Variant1);
    end;

    local procedure Execute(var Variant: Variant)
    begin
        Inspect(Variant);
        Release(Variant);
        Post(Variant);
    end;

    local procedure Inspect(Variant: Variant)
    var
        Approval: Record "CF Approval";
    begin
        // Its own copy: the caller's Variant keeps what it held.
        Variant := Approval;
    end;

    local procedure Release(var Variant: Variant)
    var
        Approval: Record "CF Approval";
        RecRef: RecordRef;
        TargetRecRef: RecordRef;
    begin
        RecRef.GetTable(Variant);
        case RecRef.Number of
            Database::"CF Approval":
                begin
                    Approval := Variant;
                    TargetRecRef.Open(Database::"CF Doc");
                    Variant := TargetRecRef;
                end;
        end;
    end;

    local procedure Post(Variant: Variant)
    var
        Log: Record "CF Log";
        RecRef: RecordRef;
    begin
        RecRef.GetTable(Variant);
        case RecRef.Number of
            Database::"CF Doc":
                begin
                    Log.Code := 'X';
                    Log.Insert();
                end;
        end;
    end;
}
codeunit 50415 "CF Roots"
{
    procedure FromBatch(var Batch: Record "CF Batch")
    var
        Responses: Codeunit "CF Responses";
    begin
        Responses.Responses(Batch);
    end;

    procedure FromApproval(var Approval: Record "CF Approval")
    var
        Responses: Codeunit "CF Responses";
    begin
        Responses.Responses(Approval);
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
fn a_recordref_number_guard_reads_the_table_the_root_put_in() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"34343434-0000-4000-8000-000000000034","name":"RG","publisher":"t","version":"1.0.0.0"}"#,
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
            &["prove", "ws", &id, "writes-table:RG Log", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    // Precondition: with the guard's own table the log is written. The handler
    // global holds it (set on the handler instance before printing).
    let doc = prove("PrintDoc");
    assert_eq!(doc["answer"], "yes", "precondition: {doc}");

    // An entry reaches the guard through the global's Duplicate() and the
    // Variant's GetTable alike: the write cannot run. `CalcDisc` putting its
    // own Doc back after the guard adds no table.
    let entry = prove("PrintEntry");
    assert_eq!(entry["answer"], "no", "{entry}");

    // Rebinding the RecordRef after it went into the global rebinds the
    // global too (`RecRefFilter := RecRef` shares it): it may hold a Doc.
    let reopened = prove("PrintEntryReopened");
    assert_eq!(reopened["answer"], "yes", "{reopened}");

    // `Alias := RecRef` shares the RecordRef, so reopening the alias reopens
    // the caller's (passed by value, still the same RecordRef).
    let aliased = prove("PrintEntryAliased");
    assert_eq!(aliased["answer"], "yes", "{aliased}");

    // A loaded subscriber of an event the RecordRef is passed to may rebind
    // it before the guard.
    let event = prove("PrintEntryViaEvent");
    assert_eq!(event["answer"], "yes", "{event}");

    // G43: `if not (R.Number in [Database::"RG Doc", Database::"RG Log"]) then
    // exit;` lets a Doc through and stops an Entry.
    let set_doc = prove("PrintDocFromSet");
    assert_eq!(set_doc["answer"], "yes", "precondition: {set_doc}");
    let set_entry = prove("PrintEntryFromSet");
    assert_eq!(set_entry["answer"], "no", "{set_entry}");
}

#[test]
fn a_branch_on_the_variants_table_rebinds_it_only_for_that_table() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"35353535-0000-4000-8000-000000000035","name":"CF","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), LOOP_SOURCE).unwrap();
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
            &["prove", "ws", &id, "writes-table:CF Log", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    // Precondition: an approval is swapped for a document on one round of
    // the loop and posted on the next.
    let approval = prove("FromApproval");
    assert_eq!(approval["answer"], "yes", "precondition: {approval}");

    // A batch takes neither branch on any round: the log is not written.
    let batch = prove("FromBatch");
    assert_eq!(batch["answer"], "no", "{batch}");
}
