//! Detector-audit d37 FN-1 (docs/detector-audit.md): `ModifyAll` does NOT persist
//! the CURRENT record's `Validate` result — `R.ModifyAll(Field, Value)` issues a
//! set-based UPDATE across R's filtered set with a literal value and never writes
//! R's in-memory Validate'd fields. So a `Validate` followed only by `ModifyAll`
//! silently discards the change and MUST fire d37; a `Validate` followed by a real
//! `Modify` / `Insert` persists it and stays suppressed.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000d37abcd";

fn run_d37(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d37: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d37-validate-without-persist")
        .collect();
    assert_eq!(d37.len(), 1);
    run_detectors(&resolved, &d37).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50210 "D37 Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
        field(3; Blocked; Boolean) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// `Validate` then `ModifyAll` — the bulk update does not write R's Validate'd
/// field, so the change is discarded → d37 fires.
#[test]
fn validate_then_modifyall_fires() {
    let cu = r#"
codeunit 50210 "D37 MA"
{
    procedure ValidateThenModifyAll()
    var
        Cust: Record "D37 Cust";
    begin
        Cust.Get('C1');
        Cust.Validate(Name, 'New');
        Cust.ModifyAll(Blocked, true);
    end;
}
"#;
    let findings = run_d37(&[al("D37Cust", TABLE_SRC), al("D37MA", cu)]);
    assert_eq!(
        findings.len(),
        1,
        "ModifyAll does not persist R's Validate → d37 fires. findings: {findings:#?}"
    );
    assert!(findings[0].root_cause.contains("ValidateThenModifyAll"));
}

/// Control: `Validate` then a real `Modify` persists R → suppressed.
#[test]
fn validate_then_modify_is_suppressed() {
    let cu = r#"
codeunit 50211 "D37 Mod"
{
    procedure ValidateThenModify()
    var
        Cust: Record "D37 Cust";
    begin
        Cust.Get('C1');
        Cust.Validate(Name, 'New');
        Cust.Modify();
    end;
}
"#;
    let findings = run_d37(&[al("D37Cust", TABLE_SRC), al("D37Mod", cu)]);
    assert!(
        findings.is_empty(),
        "a real Modify after Validate persists the change → no d37. findings: {findings:#?}"
    );
}
