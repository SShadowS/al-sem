//! Gap G-12 (docs/engine-gaps.md): d3 over-fires on PK-only / FlowField /
//! existence-check Gets and misses a pre-Get `SetLoadFields`.
//!
//! `d3-missing-setloadfields` must NOT fire when:
//!   (a) the ONLY field accessed after a `Get` is a PRIMARY-KEY field (the PK is
//!       always loaded regardless of SetLoadFields),
//!   (b) the only field accessed is a FlowField (`CalcFields` territory — d22's
//!       domain, not d3's),
//!   (c) the `Get` is an existence check (unconditional `exit` / create-with-PK
//!       pattern) with no normal field read after it,
//!   (d) a covering `SetLoadFields` appears BEFORE the `Get` in the routine —
//!       including with QUOTED field-name arguments (`SetLoadFields("Unit Price")`
//!       must match a later `Rec."Unit Price"` access).
//!
//! Suppression signal (exact, structural): the accessed-field set, minus the
//! table's first-key (PK) fields and minus `field_class == "FlowField"` fields,
//! is empty — or the (quote-normalized) pre-Get load set covers every access.
//! Everything else keeps firing (control cases below).
//!
//! Drives the REAL detector over inline AL workspaces (mirrors
//! `tests/gap_g9_trigger_rec.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g12abc";

/// Run ONLY d3 over an inline workspace and return all emitted findings.
fn run_d3(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d3-missing-setloadfields")
        .collect();
    assert_eq!(detectors.len(), 1, "d3 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// Fixture table: PK `"No."` (key 0), normal `Description` / `"Unit Price"`,
/// FlowField `Balance`, Blob `Picture`.
const TABLE_SRC: &str = r#"
table 50150 "G12 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
        field(3; "Unit Price"; Decimal) { }
        field(4; Balance; Decimal)
        {
            FieldClass = FlowField;
            CalcFormula = sum("G12 Entry".Amount where("Item No." = field("No.")));
        }
        field(5; Picture; Blob) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

fn d3_on(findings: &[Finding], routine: &str) -> Vec<Finding> {
    findings
        .iter()
        .filter(|f| f.root_cause.contains(routine))
        .cloned()
        .collect()
}

// --- (a) PK-only read after Get → no d3 ----------------------------------------

#[test]
fn pk_only_read_after_get_is_suppressed() {
    let cu_src = r#"
codeunit 50150 "G12 Pk Only"
{
    procedure ReadPkOnly(ItemNo: Code[20]): Code[20]
    var Item: Record "G12 Item";
    begin
        if Item.Get(ItemNo) then
            exit(Item."No.");
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12PkOnly", cu_src)]);
    assert!(
        findings.is_empty(),
        "the PK is always loaded — a Get whose only later access is the primary key \
         must not fire d3. findings: {findings:#?}"
    );
}

// --- (b) FlowField-only access after Get → no d3 (d22's domain) -----------------

#[test]
fn flowfield_only_access_after_get_is_suppressed() {
    let cu_src = r#"
codeunit 50151 "G12 FlowField"
{
    procedure ReadFlowOnly(ItemNo: Code[20]): Decimal
    var Item: Record "G12 Item";
    begin
        Item.Get(ItemNo);
        Item.CalcFields(Balance);
        exit(Item.Balance);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12FlowField", cu_src)]);
    assert!(
        findings.is_empty(),
        "a FlowField needs CalcFields, not SetLoadFields — a Get whose only later \
         access is a FlowField must not fire d3. findings: {findings:#?}"
    );
}

// --- (c) Get as existence check → no d3 -----------------------------------------

/// Pure existence check: `exit(Item.Get(...))` / Get-then-exit — no field touched
/// after the retrieval at all.
#[test]
fn existence_check_get_with_no_field_read_is_suppressed() {
    let cu_src = r#"
codeunit 50152 "G12 Exists"
{
    procedure HasItem(ItemNo: Code[20]): Boolean
    var Item: Record "G12 Item";
    begin
        exit(Item.Get(ItemNo));
    end;

    procedure BailIfMissing(ItemNo: Code[20])
    var Item: Record "G12 Item";
    begin
        if not Item.Get(ItemNo) then
            exit;
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12Exists", cu_src)]);
    assert!(
        findings.is_empty(),
        "an existence-check Get with no field read loads nothing wasted — no d3. \
         findings: {findings:#?}"
    );
}

/// Existence check + create pattern: `if Get then exit;` followed by Init /
/// PK-write / Insert. The only post-Get field touch is the PK — no normal field
/// is read → no d3.
#[test]
fn existence_check_then_create_with_pk_write_is_suppressed() {
    let cu_src = r#"
codeunit 50153 "G12 Ensure"
{
    procedure EnsureExists(ItemNo: Code[20])
    var Item: Record "G12 Item";
    begin
        if Item.Get(ItemNo) then
            exit;
        Item.Init();
        Item."No." := ItemNo;
        Item.Insert(true);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12Ensure", cu_src)]);
    assert!(
        findings.is_empty(),
        "an existence-check Get followed by create-with-PK-assignment reads no \
         normal field — no d3. findings: {findings:#?}"
    );
}

// --- (d) SetLoadFields BEFORE the Get covers the access → no d3 -----------------

#[test]
fn setloadfields_before_get_is_recognized() {
    let cu_src = r#"
codeunit 50154 "G12 PreLoad"
{
    procedure LoadQuotedBefore(ItemNo: Code[20]): Decimal
    var Item: Record "G12 Item";
    begin
        Item.SetLoadFields("Unit Price");
        if Item.Get(ItemNo) then
            exit(Item."Unit Price");
    end;

    procedure LoadPlainBefore(ItemNo: Code[20]): Text
    var Item: Record "G12 Item";
    begin
        Item.SetLoadFields(Description);
        if Item.Get(ItemNo) then
            exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12PreLoad", cu_src)]);
    assert!(
        findings.is_empty(),
        "a SetLoadFields set before the Get (quoted or plain field names) covers \
         the later access — no d3. findings: {findings:#?}"
    );
}

// --- CONTROL: uncovered normal field read must STILL fire ----------------------

#[test]
fn control_uncovered_normal_field_read_still_fires() {
    let cu_src = r#"
codeunit 50155 "G12 Control"
{
    procedure ReadUncovered(ItemNo: Code[20]): Text
    var Item: Record "G12 Item";
    begin
        if Item.Get(ItemNo) then
            exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12Control", cu_src)]);
    let hits = d3_on(&findings, "ReadUncovered");
    assert!(
        !hits.is_empty(),
        "a normal non-PK field read after a Get with no SetLoadFields must still \
         fire d3 (suppression-direction guard). findings: {findings:#?}"
    );
    assert!(
        hits.iter()
            .all(|f| f.detector == "d3-missing-setloadfields"),
        "control finding must be d3. findings: {hits:#?}"
    );
}

/// PK + normal field both read: the normal field is uncovered → STILL fires,
/// and the missing list names ONLY the normal field (the PK is excluded).
#[test]
fn control_pk_plus_normal_field_read_still_fires() {
    let cu_src = r#"
codeunit 50156 "G12 Mixed"
{
    procedure ReadPkAndNormal(ItemNo: Code[20]): Text
    var
        Item: Record "G12 Item";
        Key2: Code[20];
    begin
        Item.Get(ItemNo);
        Key2 := Item."No.";
        exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12Mixed", cu_src)]);
    let hits = d3_on(&findings, "ReadPkAndNormal");
    assert!(
        !hits.is_empty(),
        "a Get reading BOTH the PK and an uncovered normal field must still fire d3. \
         findings: {findings:#?}"
    );
    assert!(
        hits.iter()
            .any(|f| f.root_cause.contains("description") && !f.root_cause.contains("no.")),
        "the missing-field list must name the normal field only (PK excluded). \
         findings: {hits:#?}"
    );
}

/// FlowField + normal field both read: the FlowField exclusion must not mask the
/// uncovered normal read.
#[test]
fn control_flowfield_plus_normal_field_read_still_fires() {
    let cu_src = r#"
codeunit 50157 "G12 FlowMixed"
{
    procedure ReadFlowAndNormal(ItemNo: Code[20]): Decimal
    var Item: Record "G12 Item";
    begin
        Item.Get(ItemNo);
        Item.CalcFields(Balance);
        if Item.Description <> '' then
            exit(Item.Balance);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12FlowMixed", cu_src)]);
    let hits = d3_on(&findings, "ReadFlowAndNormal");
    assert!(
        !hits.is_empty(),
        "a Get reading a FlowField AND an uncovered normal field must still fire d3. \
         findings: {findings:#?}"
    );
    assert!(
        hits.iter()
            .any(|f| f.root_cause.contains("description") && !f.root_cause.contains("balance")),
        "the missing-field list must name the normal field only (FlowField excluded). \
         findings: {hits:#?}"
    );
}

/// A pre-Get SetLoadFields that does NOT cover the accessed field must still
/// fire (incomplete).
#[test]
fn control_incomplete_pre_get_setloadfields_still_fires() {
    let cu_src = r#"
codeunit 50158 "G12 Incomplete"
{
    procedure LoadWrongField(ItemNo: Code[20]): Text
    var Item: Record "G12 Item";
    begin
        Item.SetLoadFields("Unit Price");
        if Item.Get(ItemNo) then
            exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G12Item", TABLE_SRC), al("G12Incomplete", cu_src)]);
    let hits = d3_on(&findings, "LoadWrongField");
    assert!(
        !hits.is_empty(),
        "a pre-Get SetLoadFields that misses the accessed field must still fire d3 \
         (incomplete). findings: {findings:#?}"
    );
}
