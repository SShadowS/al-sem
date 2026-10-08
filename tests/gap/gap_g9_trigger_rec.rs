//! Gap G-9 (docs/engine-gaps.md): page/table trigger `Rec` is platform-loaded.
//!
//! `d11-modify-without-get`, `d21-read-without-load`, and
//! `d37-validate-without-persist` must NOT fire on the implicit `Rec` inside
//! page triggers (`OnValidate`, `OnAction`, `OnAfterGetRecord`, `OnDrillDown`,
//! `OnAfterGetCurrRecord`) or table field `OnValidate` triggers — the platform
//! has already loaded `Rec` before the trigger runs, and a field `OnValidate`
//! calling `Validate(...)` on a sibling field is normal field-chain validation
//! whose persistence is the caller's responsibility.
//!
//! Suppression signal (exact, structural): `routine.kind == "trigger"` AND the
//! owning object is a Page/PageExtension (trigger name in the page set) or a
//! Table/TableExtension (`OnValidate`), AND the op's receiver is `Rec`.
//! Everything else keeps firing (control cases below).
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/temp_state_calcfields.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g9abc";

/// Run d11 + d21 + d37 over an inline workspace and return all emitted findings.
fn run_g9_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let wanted = [
        "d11-modify-without-get",
        "d21-read-without-load",
        "d37-validate-without-persist",
    ];
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        3,
        "d11/d21/d37 must each be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50140 "G9 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
        field(3; "Unit Price"; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) page triggers on Rec → no d11/d21/d37 --------------------------------

/// A Page whose field `OnValidate` does `Rec.Validate(...)`, whose action
/// `OnAction` does `Rec.Modify()`, whose `OnAfterGetRecord` does
/// `Rec.TestField(...)`, and whose field `OnDrillDown` does `Rec.Validate` +
/// `Rec.TestField`. The platform loaded `Rec` before every one of these
/// triggers ran → NO d11/d21/d37 finding may be emitted.
#[test]
fn page_triggers_on_rec_are_suppressed() {
    let page_src = r#"
page 50140 "G9 Item Card"
{
    PageType = Card;
    SourceTable = "G9 Item";

    layout
    {
        area(content)
        {
            field(Description; Rec.Description)
            {
                trigger OnValidate()
                begin
                    Rec.Validate("Unit Price", 10);
                end;
            }
            field("Unit Price"; Rec."Unit Price")
            {
                trigger OnDrillDown()
                begin
                    Rec.TestField(Description);
                    Rec.Validate(Description, 'drilled');
                end;
            }
        }
    }

    actions
    {
        area(processing)
        {
            action(Approve)
            {
                trigger OnAction()
                begin
                    Rec.TestField("No.");
                    Rec.Modify();
                end;
            }
        }
    }

    trigger OnAfterGetRecord()
    begin
        Rec.TestField(Description);
    end;

    trigger OnAfterGetCurrRecord()
    begin
        Rec.Validate("Unit Price", 1);
    end;
}
"#;
    let findings = run_g9_detectors(&[al("G9Item", TABLE_SRC), al("G9ItemCard", page_src)]);
    assert!(
        findings.is_empty(),
        "page-trigger ops on the platform-loaded Rec must not fire d11/d21/d37. \
         findings: {findings:#?}"
    );
}

// --- (b) table field OnValidate on Rec → no d11/d37 ----------------------------

/// A Table field `OnValidate` calling `Validate(SiblingField, x)` — the bare
/// call binds to the implicit `Rec`. Field-chain validation is normal; the
/// persistence is the (platform) caller's job → NO d11/d37. The explicit
/// `Rec.TestField` covers d21 too.
#[test]
fn table_field_onvalidate_on_rec_is_suppressed() {
    let table_src = r#"
table 50141 "G9 Line"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Quantity; Decimal)
        {
            trigger OnValidate()
            begin
                Rec.TestField("No.");
                Validate("Unit Price", Quantity * 2);
            end;
        }
        field(3; "Unit Price"; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}
"#;
    let findings = run_g9_detectors(&[al("G9Line", table_src)]);
    assert!(
        findings.is_empty(),
        "table field OnValidate ops on the implicit Rec must not fire d11/d21/d37. \
         findings: {findings:#?}"
    );
}

// --- (c) CONTROL: non-trigger procedure keeps firing ---------------------------

/// A plain codeunit procedure doing `Item.Modify()` / `Item.TestField` /
/// `Item.Validate` with no prior Get — NOT a trigger, NOT Rec → d11, d21 and
/// d37 must all STILL fire (suppression-direction guard).
#[test]
fn control_non_trigger_procedure_still_fires() {
    let cu_src = r#"
codeunit 50140 "G9 Control"
{
    procedure MutateBlind()
    var Item: Record "G9 Item";
    begin
        Item.Modify();
    end;

    procedure ReadBlind()
    var Item: Record "G9 Item";
    begin
        Item.TestField("No.");
    end;

    procedure ValidateBlind()
    var Item: Record "G9 Item";
    begin
        Item.Get('X');
        Item.Validate(Description, 'y');
    end;
}
"#;
    let findings = run_g9_detectors(&[al("G9Item", TABLE_SRC), al("G9Control", cu_src)]);
    let d11: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d11-modify-without-get")
        .collect();
    let d21: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d21-read-without-load")
        .collect();
    let d37: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d37-validate-without-persist")
        .collect();
    assert!(
        d11.iter().any(|f| f.root_cause.contains("MutateBlind")),
        "d11 must still fire on a non-trigger Modify without Get. findings: {findings:#?}"
    );
    assert!(
        d21.iter().any(|f| f.root_cause.contains("ReadBlind")),
        "d21 must still fire on a non-trigger TestField without load. findings: {findings:#?}"
    );
    assert!(
        d37.iter().any(|f| f.root_cause.contains("ValidateBlind")),
        "d37 must still fire on a non-trigger Validate without persist. findings: {findings:#?}"
    );
}

// --- (d) CONTROL: trigger op on a NON-Rec record keeps firing -------------------

/// Inside a page `OnAction`, mutating a LOCAL record variable (not Rec) with no
/// prior Get is still a real problem — the suppression must be receiver-exact.
#[test]
fn control_trigger_op_on_non_rec_record_still_fires() {
    let page_src = r#"
page 50141 "G9 Item List"
{
    PageType = List;
    SourceTable = "G9 Item";

    actions
    {
        area(processing)
        {
            action(TouchOther)
            {
                trigger OnAction()
                var Other: Record "G9 Item";
                begin
                    Other.Modify();
                end;
            }
        }
    }
}
"#;
    let findings = run_g9_detectors(&[al("G9Item", TABLE_SRC), al("G9ItemList", page_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get" && f.root_cause.contains("Other")),
        "d11 must still fire on a non-Rec record inside a page trigger. \
         findings: {findings:#?}"
    );
}
