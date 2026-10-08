//! Task 5 (temp-state) — page `SourceTableTemporary = true` capture + implicit
//! `Rec`/`xRec` `Known(true)` override (G4, RV-8).
//!
//! # What is tested
//!
//! A page declared `SourceTableTemporary = true;` has an implicit `Rec` and
//! `xRec` that are structurally known to be temporary — the SourceTable is always
//! loaded as a temporary copy in such pages. After L3 resolution:
//!
//! - `ModelObject.source_table_temporary == Some(true)` (Part A: property capture)
//! - The page trigger's `Rec.DeleteAll()` resolves `Known(true)` (Part B)
//! - The page trigger's `xRec.Get(...)` resolves `Known(true)` (Part B: RV-8)
//!
//! Control test: a page WITHOUT `SourceTableTemporary` (or with `= false`) does
//! NOT have its implicit Rec/xRec force-upgraded — the override is strictly
//! additive toward `Known(true)`.

use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "2a000000-0000-0000-0000-0000000005aa";

/// (a) A page with `SourceTableTemporary = true` has:
///   - `ModelObject.source_table_temporary == Some(true)`
///   - implicit `Rec` op resolves `Known(true)`
///   - implicit `xRec` op resolves `Known(true)` (RV-8)
#[test]
fn source_table_temporary_true_rec_and_xrec_resolve_known_true() {
    let source = r#"
table 50700 "Some Table"
{
    fields { field(1; Id; Integer) { } }
}

page 50700 "TmpPage"
{
    SourceTable = "Some Table";
    SourceTableTemporary = true;

    layout
    {
        area(content)
        {
            field(Id; Rec.Id) { }
        }
    }

    trigger OnOpenPage()
    begin
        Rec.DeleteAll();
        xRec.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // Part A: the page object must carry source_table_temporary == Some(true).
    let obj = resolved
        .workspace
        .objects
        .iter()
        .find(|o| o.object_type == "Page" && o.name == "TmpPage")
        .expect("TmpPage Page object must be indexed");
    assert_eq!(
        obj.source_table_temporary,
        Some(true),
        "SourceTableTemporary = true must set ModelObject.source_table_temporary == Some(true)",
    );

    // Part B: OnOpenPage's implicit Rec.DeleteAll() must resolve Known(true).
    let routine = resolved
        .routine_by_name("OnOpenPage")
        .expect("OnOpenPage trigger must be resolved");
    assert_eq!(
        routine.first_record_op_temp_known("Rec"),
        Some(true),
        "`Rec.DeleteAll()` in a SourceTableTemporary=true page must resolve Known(true)",
    );

    // Part B: xRec.DeleteAll() must also resolve Known(true) (RV-8 — xRec alongside Rec).
    assert_eq!(
        routine.first_record_op_temp_known("xRec"),
        Some(true),
        "`xRec.DeleteAll()` in a SourceTableTemporary=true page must resolve Known(true) \
         (RV-8: xRec is implicitly temporary alongside Rec in such pages)",
    );
}

/// (b) Control: a page WITHOUT `SourceTableTemporary` does NOT have its Rec/xRec
/// force-upgraded to Known(true). The implicit Rec of a physical-table page stays
/// Known(false) as before — the override is additive-only.
#[test]
fn page_without_source_table_temporary_rec_not_upgraded() {
    let source = r#"
table 50710 "Phys Table"
{
    fields { field(1; Id; Integer) { } }
}

page 50710 "PhysPage"
{
    SourceTable = "Phys Table";

    layout
    {
        area(content)
        {
            field(Id; Rec.Id) { }
        }
    }

    trigger OnOpenPage()
    begin
        Rec.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // The page object must carry source_table_temporary == None (absent).
    let obj = resolved
        .workspace
        .objects
        .iter()
        .find(|o| o.object_type == "Page" && o.name == "PhysPage")
        .expect("PhysPage Page object must be indexed");
    assert_eq!(
        obj.source_table_temporary, None,
        "a page without SourceTableTemporary must have source_table_temporary == None",
    );

    // The Rec op must NOT be upgraded to Known(true). The implicit Rec of a
    // physical-table page has no temp_state forced — it remains not-known-true.
    let routine = resolved
        .routine_by_name("OnOpenPage")
        .expect("OnOpenPage trigger must be resolved");
    assert_ne!(
        routine.first_record_op_temp_known("Rec"),
        Some(true),
        "`Rec.DeleteAll()` in a non-temporary page must NOT be upgraded to Known(true); \
         the additive-only rule must not false-upgrade",
    );
}

/// (c) A page with `SourceTableTemporary = false` is explicitly non-temporary.
/// Its `source_table_temporary` is `Some(false)`, and Rec is NOT upgraded.
#[test]
fn source_table_temporary_false_rec_not_upgraded() {
    let source = r#"
table 50720 "Another Table"
{
    fields { field(1; Id; Integer) { } }
}

page 50720 "FalseTmpPage"
{
    SourceTable = "Another Table";
    SourceTableTemporary = false;

    layout
    {
        area(content)
        {
            field(Id; Rec.Id) { }
        }
    }

    trigger OnOpenPage()
    begin
        Rec.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // source_table_temporary == Some(false) when present but false.
    let obj = resolved
        .workspace
        .objects
        .iter()
        .find(|o| o.object_type == "Page" && o.name == "FalseTmpPage")
        .expect("FalseTmpPage Page object must be indexed");
    assert_eq!(
        obj.source_table_temporary,
        Some(false),
        "SourceTableTemporary = false must set ModelObject.source_table_temporary == Some(false)",
    );

    // Rec op must NOT be upgraded to Known(true).
    let routine = resolved
        .routine_by_name("OnOpenPage")
        .expect("OnOpenPage trigger must be resolved");
    assert_ne!(
        routine.first_record_op_temp_known("Rec"),
        Some(true),
        "`Rec.DeleteAll()` in a SourceTableTemporary=false page must NOT be upgraded to Known(true)",
    );
}
