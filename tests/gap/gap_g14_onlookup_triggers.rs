//! Gap G-14 (docs/engine-gaps.md): extend the G-9 trigger set with the
//! field-level lookup triggers `OnLookup` and `OnAssistEdit`.
//!
//! The AL platform loads the implicit `Rec` before a page field's `OnLookup` /
//! `OnAssistEdit` trigger runs, and a `Validate` performed inside `OnLookup` is
//! persisted by the page framework — so `d11-modify-without-get`,
//! `d21-read-without-load`, and `d37-validate-without-persist` must NOT fire on
//! `Rec` inside those triggers (G-9's set missed them).
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g9_trigger_rec.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g14abc";

/// Run d11 + d21 + d37 over an inline workspace and return all emitted findings.
fn run_g14_detectors(files: &[(String, String)]) -> Vec<Finding> {
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
table 50150 "G14 Item"
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

// --- (a) OnLookup / OnAssistEdit on Rec → no d11/d21/d37 -----------------------

/// A Page whose field `OnLookup` does `Rec.Validate` / `Rec.TestField` /
/// `Rec.Modify`, and whose field `OnAssistEdit` does the same. The platform
/// loaded `Rec` before both triggers ran, and the page framework persists the
/// result → NO d11/d21/d37 finding may be emitted.
#[test]
fn onlookup_and_onassistedit_on_rec_are_suppressed() {
    let page_src = r#"
page 50150 "G14 Item Card"
{
    PageType = Card;
    SourceTable = "G14 Item";

    layout
    {
        area(content)
        {
            field(Description; Rec.Description)
            {
                trigger OnLookup(var Text: Text): Boolean
                begin
                    Rec.Validate("Unit Price", 10);
                    Rec.TestField("No.");
                    Rec.Modify();
                end;
            }
            field("Unit Price"; Rec."Unit Price")
            {
                trigger OnAssistEdit()
                begin
                    Rec.Validate(Description, 'assisted');
                    Rec.TestField(Description);
                    Rec.Modify();
                end;
            }
        }
    }
}
"#;
    let findings = run_g14_detectors(&[al("G14Item", TABLE_SRC), al("G14ItemCard", page_src)]);
    assert!(
        findings.is_empty(),
        "OnLookup/OnAssistEdit ops on the platform-loaded Rec must not fire d11/d21/d37. \
         findings: {findings:#?}"
    );
}

// --- (b) CONTROL: non-trigger procedure keeps firing ---------------------------

/// A plain codeunit procedure doing the same Rec-style ops on a local record
/// with no prior Get — NOT a trigger → d11/d21/d37 must all STILL fire
/// (suppression-direction guard, same pattern as the G-9 control).
#[test]
fn control_non_trigger_procedure_still_fires() {
    let cu_src = r#"
codeunit 50150 "G14 Control"
{
    procedure MutateBlind()
    var Item: Record "G14 Item";
    begin
        Item.Modify();
    end;

    procedure ReadBlind()
    var Item: Record "G14 Item";
    begin
        Item.TestField("No.");
    end;

    procedure ValidateBlind()
    var Item: Record "G14 Item";
    begin
        Item.Get('X');
        Item.Validate(Description, 'y');
    end;
}
"#;
    let findings = run_g14_detectors(&[al("G14Item", TABLE_SRC), al("G14Control", cu_src)]);
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

// --- (c) CONTROL: OnLookup op on a NON-Rec record keeps firing ------------------

/// Inside a field `OnLookup`, mutating a LOCAL record variable (not Rec) with
/// no prior Get is still a real problem — the suppression must stay
/// receiver-exact.
#[test]
fn control_onlookup_op_on_non_rec_record_still_fires() {
    let page_src = r#"
page 50151 "G14 Item List"
{
    PageType = List;
    SourceTable = "G14 Item";

    layout
    {
        area(content)
        {
            field(Description; Rec.Description)
            {
                trigger OnLookup(var Text: Text): Boolean
                var Other: Record "G14 Item";
                begin
                    Other.Modify();
                end;
            }
        }
    }
}
"#;
    let findings = run_g14_detectors(&[al("G14Item", TABLE_SRC), al("G14ItemList", page_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get" && f.root_cause.contains("Other")),
        "d11 must still fire on a non-Rec record inside OnLookup. findings: {findings:#?}"
    );
}
