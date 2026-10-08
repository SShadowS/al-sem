//! Gap G-15 (docs/engine-gaps.md): d3/d42 fire on field WRITES, post-Init
//! accesses, and PK fields.
//!
//! `d3-missing-setloadfields` must NOT fire when:
//!   (a) the only fields touched after the retrieval are assignment WRITE
//!       targets (`Rec.Field := ...` — construction/update writes need no
//!       SetLoadFields), including the `if not Rec.Get then begin Init; ...;
//!       Insert end` construct shape,
//!   (b) an intervening `Init()` / `Clear(Rec)` resets the buffer between the
//!       retrieval and the access — the access reads the constructed buffer,
//!       not the loaded row.
//!
//! `d42-cross-call-wrong-setloadfields` must NOT fire when the only fields the
//! callee requires loaded at entry are PRIMARY-KEY fields (the PK is always
//! loaded regardless of SetLoadFields — the same exclusion G-12 added to d3).
//!
//! Suppression signals (exact, structural):
//!   (a) a field access whose (position, member name) matches a recorded
//!       assignment LHS is the write target — excluded from the witness set;
//!   (b) `Init` record ops and `Clear(<var>)` bare calls close the post-
//!       retrieval access window (like Reset/Copy/TransferFields);
//!   (c) the callee's required-at-entry set minus the PK (first key) fields.
//! Everything else keeps firing (control cases below).
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g12_d3_refinements.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g15abc";

/// Run ONLY the named detector over an inline workspace.
fn run_detector(name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == name)
        .collect();
    assert_eq!(detectors.len(), 1, "{name} must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn run_d3(files: &[(String, String)]) -> Vec<Finding> {
    run_detector("d3-missing-setloadfields", files)
}

fn run_d42(files: &[(String, String)]) -> Vec<Finding> {
    run_detector("d42-cross-call-wrong-setloadfields", files)
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// Fixture table: PK `"No."` (key 0), normal `Description` / `"Unit Price"`.
const TABLE_SRC: &str = r#"
table 50160 "G15 Item"
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

fn findings_on(findings: &[Finding], routine: &str) -> Vec<Finding> {
    findings
        .iter()
        .filter(|f| f.root_cause.contains(routine))
        .cloned()
        .collect()
}

// --- (a) field WRITES after the retrieval → no d3 -------------------------------

/// The G-15 evidence shape: `if not Get then begin Init; <writes>; Insert end`.
/// Description is WRITTEN (construction), never read — no SetLoadFields needed.
#[test]
fn write_after_failed_get_then_construct_is_suppressed() {
    let cu_src = r#"
codeunit 50160 "G15 Ensure"
{
    procedure EnsureExists(ItemNo: Code[20])
    var Item: Record "G15 Item";
    begin
        if not Item.Get(ItemNo) then begin
            Item.Init();
            Item.Description := 'New item';
            Item.Insert();
        end;
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15Ensure", cu_src)]);
    assert!(
        findings.is_empty(),
        "fields WRITTEN after a failed Get (construct-and-Insert) are not reads — \
         no SetLoadFields needed, no d3. findings: {findings:#?}"
    );
}

/// Pure write signal, no Init: Get then assign-and-Modify. The only post-Get
/// field touch is the assignment TARGET — a write, not a read.
#[test]
fn field_write_without_init_is_suppressed() {
    let cu_src = r#"
codeunit 50161 "G15 Update"
{
    procedure RenameIt(ItemNo: Code[20])
    var Item: Record "G15 Item";
    begin
        Item.Get(ItemNo);
        Item.Description := 'Updated';
        Item.Modify();
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15Update", cu_src)]);
    assert!(
        findings.is_empty(),
        "an assignment WRITE target after a Get is not a read — no d3. \
         findings: {findings:#?}"
    );
}

// --- (b) Init / Clear resets the buffer → no d3 ----------------------------------

/// FindLast-then-construct: `Init()` resets the buffer, the writes build a new
/// row. Nothing reads the loaded row.
#[test]
fn init_then_writes_after_findlast_is_suppressed() {
    let cu_src = r#"
codeunit 50162 "G15 Append"
{
    procedure AppendItem()
    var Item: Record "G15 Item";
    begin
        Item.FindLast();
        Item.Init();
        Item.Description := 'Appended';
        Item.Insert();
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15Append", cu_src)]);
    assert!(
        findings.is_empty(),
        "Init resets the buffer and the later fields are writes — no d3. \
         findings: {findings:#?}"
    );
}

/// Isolates the Init signal: a READ after `Init()` reads the initialised
/// in-memory buffer, not the loaded row — the prior FindLast load is irrelevant.
#[test]
fn read_after_init_is_suppressed() {
    let cu_src = r#"
codeunit 50163 "G15 ReadInit"
{
    procedure ReadAfterInit(): Text
    var Item: Record "G15 Item";
    begin
        Item.FindLast();
        Item.Init();
        exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15ReadInit", cu_src)]);
    assert!(
        findings.is_empty(),
        "a read after Init() reads the initialised buffer, not the loaded row — \
         no d3. findings: {findings:#?}"
    );
}

/// `Clear(Rec)` resets the variable entirely — same buffer-reset semantics.
#[test]
fn read_after_clear_is_suppressed() {
    let cu_src = r#"
codeunit 50164 "G15 ReadClear"
{
    procedure ReadAfterClear(ItemNo: Code[20]): Text
    var Item: Record "G15 Item";
    begin
        Item.Get(ItemNo);
        Clear(Item);
        exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15ReadClear", cu_src)]);
    assert!(
        findings.is_empty(),
        "a read after Clear(Rec) reads a cleared buffer, not the loaded row — \
         no d3. findings: {findings:#?}"
    );
}

// --- (c) d42: callee requires only PK fields → no d42 ---------------------------

#[test]
fn d42_pk_only_cross_call_is_suppressed() {
    let cu_src = r#"
codeunit 50165 "G15 D42 Pk"
{
    procedure ForwardNarrowed(): Code[20]
    var Item: Record "G15 Item";
    begin
        Item.SetLoadFields(Description);
        if Item.FindFirst() then
            exit(ReadPk(Item));
    end;

    local procedure ReadPk(var Item: Record "G15 Item"): Code[20]
    begin
        exit(Item."No.");
    end;
}
"#;
    let findings = run_d42(&[al("G15Item", TABLE_SRC), al("G15D42Pk", cu_src)]);
    assert!(
        findings.is_empty(),
        "the PK is always loaded regardless of SetLoadFields — a callee requiring \
         only PK fields causes no extra round-trip, no d42. findings: {findings:#?}"
    );
}

// --- CONTROLS: genuine reads must STILL fire -------------------------------------

/// A genuine READ of a non-PK normal field with no SetLoadFields → d3 fires.
#[test]
fn control_normal_field_read_still_fires_d3() {
    let cu_src = r#"
codeunit 50166 "G15 Control"
{
    procedure ReadDesc(ItemNo: Code[20]): Text
    var Item: Record "G15 Item";
    begin
        Item.Get(ItemNo);
        exit(Item.Description);
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15Control", cu_src)]);
    let hits = findings_on(&findings, "ReadDesc");
    assert!(
        !hits.is_empty(),
        "a genuine read of a normal non-PK field with no SetLoadFields must still \
         fire d3 (suppression-direction guard). findings: {findings:#?}"
    );
}

/// A READ on the assignment's RHS keeps firing even though the LHS is a write:
/// `Item.Description := Format(Item."Unit Price")` READS "Unit Price".
#[test]
fn control_read_on_assignment_rhs_still_fires_d3() {
    let cu_src = r#"
codeunit 50167 "G15 RhsRead"
{
    procedure CopyPrice(ItemNo: Code[20])
    var Item: Record "G15 Item";
    begin
        Item.Get(ItemNo);
        Item.Description := Format(Item."Unit Price");
        Item.Modify();
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15RhsRead", cu_src)]);
    let hits = findings_on(&findings, "CopyPrice");
    assert!(
        !hits.is_empty(),
        "the RHS read of \"Unit Price\" is a genuine read — the LHS write \
         exclusion must not mask it. findings: {findings:#?}"
    );
    assert!(
        hits.iter()
            .any(|f| f.root_cause.contains("unit price") && !f.root_cause.contains("description")),
        "the missing-field list must name the READ field only (the written field \
         is excluded). findings: {hits:#?}"
    );
}

/// A read BETWEEN the retrieval and the Init is still a read of the loaded row.
#[test]
fn control_read_before_init_still_fires_d3() {
    let cu_src = r#"
codeunit 50168 "G15 PreInit"
{
    procedure ReadThenInit(): Text
    var
        Item: Record "G15 Item";
        Desc: Text;
    begin
        Item.FindLast();
        Desc := Item.Description;
        Item.Init();
        exit(Desc);
    end;
}
"#;
    let findings = run_d3(&[al("G15Item", TABLE_SRC), al("G15PreInit", cu_src)]);
    let hits = findings_on(&findings, "ReadThenInit");
    assert!(
        !hits.is_empty(),
        "a read BEFORE the Init still reads the loaded row — the Init window \
         close must not suppress it. findings: {findings:#?}"
    );
}

/// d42 control: a callee requiring a NORMAL non-PK field outside the caller's
/// narrow must still fire.
#[test]
fn control_d42_normal_field_still_fires() {
    let cu_src = r#"
codeunit 50169 "G15 D42 Ctrl"
{
    procedure ForwardNarrowed(): Decimal
    var Item: Record "G15 Item";
    begin
        Item.SetLoadFields(Description);
        if Item.FindFirst() then
            exit(ReadPrice(Item));
    end;

    local procedure ReadPrice(var Item: Record "G15 Item"): Decimal
    begin
        exit(Item."Unit Price");
    end;
}
"#;
    let findings = run_d42(&[al("G15Item", TABLE_SRC), al("G15D42Ctrl", cu_src)]);
    let hits = findings_on(&findings, "ReadPrice");
    assert!(
        !hits.is_empty(),
        "a callee requiring a normal non-PK field outside the caller's narrow \
         must still fire d42 (suppression-direction guard). findings: {findings:#?}"
    );
}
