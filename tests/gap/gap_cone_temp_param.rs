//! The capability cone resolves a callee's PARAMETER-DEPENDENT record writes
//! against the caller's argument (B3 triage-E finding 2).
//!
//! A callee that writes through a keyword-less `var Record X` parameter records
//! the write as `parameter-dependent(i)`: it is temporary exactly when the
//! caller passes a temporary record. The L4 db-effect solver already
//! substitutes that at each call edge (`substitute_pd_temp_state`). The
//! capability cone did not: it copied the callee's fact up unchanged, so
//! `writes_physical_tables_of(caller)` — what d8/d43/d44/d45/d50 read — counted
//! a write into the caller's `temporary` record as a physical table write. On
//! CDO that produced three false d45 findings once `Page.Run` correctly reached
//! `OnOpenPage`, whose page-global `temporary` records go by `var` into
//! `GetUpgradeData`.
//!
//! Every case reads the cone of the CALLER (the callee's own physical set is
//! unchanged: its param could be physical from another caller).

use al_sem::engine::l3::l3_workspace::{L3Resolved, assemble_and_resolve_default};
use al_sem::engine::l5::detector_context::build_detector_context;
use al_sem::engine::l5::registry::substrate;

const APP_GUID: &str = "11111111-0000-0000-0000-00000000c0e1";

const TABLES: &str = r#"
table 50101 "CT One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50102 "CT Two" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50103 "CT Three" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
"#;

const WRITER: &str = r#"
codeunit 50110 "CT Writer"
{
    procedure WriteOne(var R: Record "CT One")
    begin
        R.Init();
        R.Insert();
    end;

    procedure WriteOneByValue(R: Record "CT One")
    begin
        R.Insert();
    end;

    procedure Forward(var R: Record "CT One")
    begin
        WriteOne(R);
    end;

    procedure WriteTwoMixed(var R: Record "CT Two")
    var
        Phys: Record "CT Two";
    begin
        R.Insert();
        Phys.Insert();
    end;

    procedure WriteThree(var R: Record "CT Three")
    begin
        R.Insert();
    end;
}
"#;

const CALLERS: &str = r#"
codeunit 50111 "CT Callers"
{
    var
        Writer: Codeunit "CT Writer";
        GlobalTemp: Record "CT One" temporary;

    procedure TempByVar()
    var
        T: Record "CT One" temporary;
    begin
        Writer.WriteOne(T);
    end;

    procedure PhysicalByVar()
    var
        P: Record "CT One";
    begin
        Writer.WriteOne(P);
    end;

    procedure TempTwoLevels()
    var
        T: Record "CT One" temporary;
    begin
        Writer.Forward(T);
    end;

    procedure GlobalTempByVar()
    begin
        Writer.WriteOne(GlobalTemp);
    end;

    procedure TempByValue()
    var
        T: Record "CT One" temporary;
    begin
        Writer.WriteOneByValue(T);
    end;

    procedure TempIntoMixedCallee()
    var
        T: Record "CT Two" temporary;
    begin
        Writer.WriteTwoMixed(T);
    end;

    procedure TempAndPhysicalSameCallee()
    var
        T: Record "CT Three" temporary;
        P: Record "CT Three";
    begin
        Writer.WriteThree(T);
        Writer.WriteThree(P);
    end;

    procedure PhysicalThenForward(var R: Record "CT Three")
    var
        Phys: Record "CT Three";
    begin
        Phys.Insert();
        Writer.WriteThree(R);
    end;

    procedure TempIntoPhysicalThenForward()
    var
        T: Record "CT Three" temporary;
    begin
        PhysicalThenForward(T);
    end;

    procedure RecursiveWrite(var R: Record "CT One"; N: Integer)
    begin
        R.Insert();
        if N > 0 then
            RecursiveWrite(R, N - 1);
    end;

    procedure TempIntoRecursive()
    var
        T: Record "CT One" temporary;
    begin
        RecursiveWrite(T, 3);
    end;

    procedure OuterOfTempTwoLevels()
    begin
        TempTwoLevels();
    end;

    procedure ForwardsOwnParam(var R: Record "CT One")
    begin
        Writer.WriteOne(R);
    end;

    procedure PassesTempToForwarder()
    var
        T: Record "CT One" temporary;
    begin
        ForwardsOwnParam(T);
    end;
}
"#;

const PAGES: &str = r#"
page 50120 "CT Upgrade Page"
{
    PageType = Card;

    trigger OnOpenPage()
    var
        Writer: Codeunit "CT Writer";
    begin
        Writer.WriteOne(TempOne);
    end;

    var
        TempOne: Record "CT One" temporary;
}

page 50121 "CT Temp Source Page"
{
    PageType = List;
    SourceTable = "CT One";
    SourceTableTemporary = true;

    trigger OnOpenPage()
    var
        Writer: Codeunit "CT Writer";
    begin
        Writer.WriteOne(Rec);
    end;
}
"#;

fn resolve() -> L3Resolved {
    let files = vec![
        ("src/Tables.al".to_string(), TABLES.to_string()),
        ("src/Writer.al".to_string(), WRITER.to_string()),
        ("src/Callers.al".to_string(), CALLERS.to_string()),
        ("src/Pages.al".to_string(), PAGES.to_string()),
    ];
    assemble_and_resolve_default(&files, APP_GUID)
}

/// The unique routine id for `object_name` + `routine_name`.
fn rid(resolved: &L3Resolved, object_name: &str, routine_name: &str) -> String {
    let ws = &resolved.workspace;
    let hits: Vec<&str> = ws
        .routines
        .iter()
        .filter(|r| {
            r.name == routine_name
                && ws
                    .objects
                    .iter()
                    .any(|o| o.id == r.object_id && o.name == object_name)
        })
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one routine {object_name}.{routine_name}, got {hits:?}"
    );
    hits[0].to_string()
}

/// Table names (not ids) of `routine`'s physical write set.
fn physical_writes(resolved: &L3Resolved, object_name: &str, routine_name: &str) -> Vec<String> {
    let ctx = build_detector_context(resolved, substrate::ALL);
    let id = rid(resolved, object_name, routine_name);
    let names = |set: Vec<String>| -> Vec<String> {
        set.iter()
            .map(|t| {
                resolved
                    .workspace
                    .tables
                    .iter()
                    .find(|tb| &tb.id == t)
                    .map(|tb| tb.name.clone())
                    .unwrap_or_else(|| t.clone())
            })
            .collect()
    };
    // Non-vacuity: the temp-INCLUSIVE set must see the write, so an empty
    // physical set below is the temp gate's decision, not a missing edge.
    assert!(
        !ctx.cone_derived.writes_tables_of(&id).is_empty(),
        "{object_name}.{routine_name}: no table write reached the cone at all"
    );
    names(ctx.cone_derived.writes_physical_tables_of(&id))
}

#[test]
fn temp_record_by_var_is_not_a_physical_write_of_the_caller() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempByVar"),
        Vec::<String>::new()
    );
}

#[test]
fn physical_record_by_var_stays_a_physical_write() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "PhysicalByVar"),
        vec!["CT One"]
    );
}

#[test]
fn temp_record_through_two_var_levels_is_not_physical() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempTwoLevels"),
        Vec::<String>::new()
    );
    // ...and one more caller above it inherits the resolved (temporary) fact.
    assert_eq!(
        physical_writes(&r, "CT Callers", "OuterOfTempTwoLevels"),
        Vec::<String>::new()
    );
}

#[test]
fn caller_param_forwarded_stays_dependent_until_a_temp_caller() {
    let r = resolve();
    // Its own `var` param: could be physical from another caller → counts.
    assert_eq!(
        physical_writes(&r, "CT Callers", "ForwardsOwnParam"),
        vec!["CT One"]
    );
    assert_eq!(
        physical_writes(&r, "CT Callers", "PassesTempToForwarder"),
        Vec::<String>::new()
    );
}

#[test]
fn object_global_temp_record_by_var_is_not_physical() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "GlobalTempByVar"),
        Vec::<String>::new()
    );
}

#[test]
fn page_global_temp_record_from_onopenpage_is_not_physical() {
    // The CDO shape (CDOAssistAutStatementUpg.Page.al OnOpenPage).
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Upgrade Page", "OnOpenPage"),
        Vec::<String>::new()
    );
}

#[test]
fn source_table_temporary_rec_by_var_is_not_physical() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Temp Source Page", "OnOpenPage"),
        Vec::<String>::new()
    );
}

#[test]
fn temp_record_by_value_still_counts_physical() {
    // L2 types a keyword-less BY-VALUE record param `known(false)`; this fix
    // does not change that (only `var` aliasing is modelled), so it counts.
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempByValue"),
        vec!["CT One"]
    );
}

#[test]
fn callee_with_its_own_physical_write_of_the_same_table_still_counts() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempIntoMixedCallee"),
        vec!["CT Two"]
    );
}

#[test]
fn same_callee_with_temp_and_physical_arguments_still_counts() {
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempAndPhysicalSameCallee"),
        vec!["CT Three"]
    );
}

#[test]
fn forwarded_param_merged_with_a_physical_write_of_the_same_table_still_counts() {
    // `PhysicalThenForward`'s cone holds ONE key for "insert CT Three": its own
    // physical write and the forwarded PD write merge there. The merged entry
    // must not stay substitutable, or the temp caller below resolves the
    // physical write away with it.
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempIntoPhysicalThenForward"),
        vec!["CT Three"]
    );
}

#[test]
fn recursive_callee_write_through_var_param_stays_physical() {
    // Inside a recursive SCC a PD fact has no single frame (the recursive call
    // may bind the param to anything), so the cone does not resolve it — a
    // stated, conservative limit. Pins the non-recursive-singleton anchor gate.
    let r = resolve();
    assert_eq!(
        physical_writes(&r, "CT Callers", "TempIntoRecursive"),
        vec!["CT One"]
    );
}

#[test]
fn callee_own_param_write_still_counts() {
    let r = resolve();
    assert_eq!(physical_writes(&r, "CT Writer", "WriteOne"), vec!["CT One"]);
}
