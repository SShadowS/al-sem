//! Detector-audit class A (temp gate) + Singleton BUG-5 (duplicate finding id)
//! for d4 (docs/detector-audit.md):
//!
//! - **Temp gate (class A)**: repeated identical lookups on a `temporary`
//!   record (`temp_state` Known(true)) are in-memory — no SQL round-trip to
//!   hoist → NO d4. Suppression-direction control: the SAME shape on a
//!   PHYSICAL record must STILL fire.
//! - **BUG-5 (duplicate id)**: the finding id omitted the literal lookup key,
//!   so TWO distinct keys each repeated 2+ times in the same
//!   (routine, loop, record variable) produced the SAME id. The key is now
//!   appended ONLY when multiple key groups collide, so single-key findings
//!   keep their pre-fix ids (existing goldens stay stable).
//!
//! Drives the REAL d4 detector over inline AL workspaces (mirrors
//! `tests/r0-corpus/ws-d4-repeated-get` and `tests/gap_audit_d2_guards.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000audd04";

const TABLE_SRC: &str = r#"
table 50180 "AuditD4 Customer"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// Run ONLY d4 over an inline workspace and return its findings.
fn run_d4(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d4-repeated-lookup-in-loop")
        .collect();
    assert_eq!(detectors.len(), 1, "d4 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// --- (1) Temp gate: repeated identical lookups on a `temporary` record
// inside a loop → NO d4 (in-memory, nothing to hoist) ---------------------------

#[test]
fn temp_record_repeated_lookup_is_suppressed() {
    let codeunit = r#"
codeunit 50181 "AuditD4 Temp"
{
    procedure LookupInLoop()
    var
        TempCust: Record "AuditD4 Customer" temporary;
        i: Integer;
    begin
        for i := 1 to 5 do begin
            TempCust.Get('CUST001');
            TempCust.Get('CUST001');
        end;
    end;
}
"#;
    let findings = run_d4(&[
        al("AuditD4Customer", TABLE_SRC),
        al("AuditD4Temp", codeunit),
    ]);
    assert!(
        findings.is_empty(),
        "repeated identical lookups on a Known(true) temporary record must not \
         make d4 fire. findings: {findings:#?}"
    );
}

// --- CONTROL: the SAME repeated-lookup shape on a PHYSICAL record → d4 must
// STILL fire (suppression-direction safety) -------------------------------------

#[test]
fn control_physical_record_repeated_lookup_still_fires() {
    let codeunit = r#"
codeunit 50182 "AuditD4 Physical"
{
    procedure LookupInLoop()
    var
        Cust: Record "AuditD4 Customer";
        i: Integer;
    begin
        for i := 1 to 5 do begin
            Cust.Get('CUST001');
            Cust.Get('CUST001');
        end;
    end;
}
"#;
    let findings = run_d4(&[
        al("AuditD4Customer", TABLE_SRC),
        al("AuditD4Physical", codeunit),
    ]);
    assert_eq!(
        findings.len(),
        1,
        "repeated identical lookups on a PHYSICAL record must still make d4 \
         fire. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(f.detector, "d4-repeated-lookup-in-loop");
    // BUG-5 guardrail: a SINGLE-key group keeps the pre-fix id shape
    // `d4/{routine}/{loop}/{varLower}` — no key suffix → existing single-key
    // goldens do not move.
    assert!(
        f.id.ends_with("/cust"),
        "single-key d4 finding id must keep the pre-fix `…/{{varLower}}` shape \
         (no key suffix). id: {}",
        f.id
    );
}

// --- (2) BUG-5: TWO distinct literal keys, each repeated 2+ times, on the
// SAME (routine, loop, record variable) → the two findings must have DISTINCT
// ids (pre-fix both got `d4/{routine}/{loop}/{varLower}`) -----------------------

#[test]
fn two_distinct_keys_in_same_loop_get_distinct_ids() {
    let codeunit = r#"
codeunit 50183 "AuditD4 TwoKeys"
{
    procedure LookupInLoop()
    var
        Cust: Record "AuditD4 Customer";
        i: Integer;
    begin
        for i := 1 to 5 do begin
            Cust.Get('ALPHA');
            Cust.Get('ALPHA');
            Cust.Get('BETA');
            Cust.Get('BETA');
        end;
    end;
}
"#;
    let findings = run_d4(&[
        al("AuditD4Customer", TABLE_SRC),
        al("AuditD4TwoKeys", codeunit),
    ]);
    assert_eq!(
        findings.len(),
        2,
        "two distinct repeated literal keys must produce two findings. \
         findings: {findings:#?}"
    );
    assert_ne!(
        findings[0].id, findings[1].id,
        "two distinct literal-key groups in the same (routine, loop, variable) \
         must get DISTINCT finding ids (BUG-5). findings: {findings:#?}"
    );
    assert_ne!(
        findings[0].root_cause_key, findings[1].root_cause_key,
        "rootCauseKey must also be distinct across the two key groups. \
         findings: {findings:#?}"
    );
}
