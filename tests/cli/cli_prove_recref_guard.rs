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
}
