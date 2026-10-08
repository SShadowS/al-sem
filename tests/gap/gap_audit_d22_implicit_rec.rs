//! Detector-audit d22 FN (docs/detector-audit.md): a FlowField read on the
//! IMPLICIT trigger/page `Rec` (`Rec."Balance (LCY)"` in OnAfterGetRecord) must be
//! visible to d22. The implicit Rec is registered as a record variable so the
//! access is captured + its table resolves.
//!
//! Companion gate: d3 (missing-SetLoadFields) must NOT fire on the same implicit
//! Rec field reads — the platform already loads Rec in those triggers.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000d22abcd";

fn run_detector(name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let dets: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == name)
        .collect();
    assert_eq!(dets.len(), 1, "{name} registered once");
    run_detectors(&resolved, &dets).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50220 "D22 Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
        field(3; "Balance (LCY)"; Decimal) { FieldClass = FlowField; CalcFormula = Count("D22 Cust"); }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// Page OnAfterGetRecord reads the FlowField on the implicit Rec with no prior
/// CalcFields → d22 fires.
#[test]
fn d22_implicit_rec_flowfield_read_fires() {
    let page = r#"
page 50220 "D22 List"
{
    PageType = List;
    SourceTable = "D22 Cust";

    layout
    {
        area(content)
        {
            repeater(grp)
            {
                field(Name; Rec.Name) { }
            }
        }
    }

    trigger OnAfterGetRecord()
    begin
        if Rec."Balance (LCY)" > 0 then;
    end;
}
"#;
    let findings = run_detector(
        "d22-flowfield-without-calcfields",
        &[al("D22Cust", TABLE_SRC), al("D22List", page)],
    );
    assert_eq!(
        findings.len(),
        1,
        "FlowField read on the implicit Rec must fire d22. findings: {findings:#?}"
    );
}

/// Control: a prior CalcFields on the implicit Rec covers the read → suppressed.
#[test]
fn d22_implicit_rec_flowfield_with_calcfields_suppressed() {
    let page = r#"
page 50221 "D22 List Calc"
{
    PageType = List;
    SourceTable = "D22 Cust";

    trigger OnAfterGetRecord()
    begin
        Rec.CalcFields("Balance (LCY)");
        if Rec."Balance (LCY)" > 0 then;
    end;
}
"#;
    let findings = run_detector(
        "d22-flowfield-without-calcfields",
        &[al("D22Cust", TABLE_SRC), al("D22ListCalc", page)],
    );
    assert!(
        findings.is_empty(),
        "a prior CalcFields on the implicit Rec covers the read → no d22. findings: {findings:#?}"
    );
}

/// Control: a NORMAL field read on the implicit Rec does not fire d22.
#[test]
fn d22_implicit_rec_normal_field_does_not_fire() {
    let page = r#"
page 50222 "D22 List Norm"
{
    PageType = List;
    SourceTable = "D22 Cust";

    trigger OnAfterGetRecord()
    begin
        if Rec.Name <> '' then;
    end;
}
"#;
    let findings = run_detector(
        "d22-flowfield-without-calcfields",
        &[al("D22Cust", TABLE_SRC), al("D22ListNorm", page)],
    );
    assert!(
        findings.is_empty(),
        "a normal-field read on the implicit Rec is not a FlowField → no d22. findings: {findings:#?}"
    );
}

/// Companion: d3 (missing-SetLoadFields) must NOT fire on the implicit Rec field
/// read inside a platform-loaded trigger (the platform loads Rec there).
#[test]
fn d3_does_not_fire_on_implicit_rec_in_trigger() {
    let page = r#"
page 50223 "D22 List D3"
{
    PageType = List;
    SourceTable = "D22 Cust";

    trigger OnAfterGetRecord()
    begin
        if Rec.Name <> '' then;
    end;
}
"#;
    let findings = run_detector(
        "d3-missing-setloadfields",
        &[al("D22Cust", TABLE_SRC), al("D22ListD3", page)],
    );
    assert!(
        findings.is_empty(),
        "d3 must not flag implicit-Rec reads in a platform-loaded trigger. findings: {findings:#?}"
    );
}
