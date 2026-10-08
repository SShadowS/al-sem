//! Task 11 (temp-state-tracking, RV-1): the CalcFields/FlowField gate in d1.
//!
//! A TEMPORARY record's FlowField is still computed by evaluating its CalcFormula
//! against the (physical) flow-target tables — a REAL SQL query, host tempness
//! irrelevant. Blob/Normal field loads on a temp record ARE in-memory. So d1's
//! blanket "temp record ⇒ downgrade to info" is WRONG for `CalcFields` /
//! `SetAutoCalcFields` when a FlowField is involved.
//!
//! RV-1 policy: for a `CalcFields`/`SetAutoCalcFields` op on a record d1 resolved
//! to Temporary, downgrade to info ONLY when EVERY named field argument resolves
//! (via the table model) to `field_class != "FlowField"`. If ANY field arg is a
//! FlowField, OR any field arg is unresolvable (name not in table / table not
//! resolved / no field args) ⇒ KEEP FIRING at normal severity with the honest
//! FlowField note.
//!
//! These drive the REAL d1 detector over inline AL workspaces (mirrors
//! `tests/temp_state_d1_path.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::{DetectorStats, run_detectors};
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000d1abc";

/// Run d1 in isolation over an inline workspace and return its emitted findings.
fn run_d1(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    run_detectors(&resolved, &d1).findings
}

/// Run d1 and return its `DetectorStats` (for the `downgradedToInfo` stat assertions).
fn run_d1_stats(files: &[(String, String)]) -> DetectorStats {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    let out = run_detectors(&resolved, &d1);
    out.detector_stats
        .into_iter()
        .find(|s| s.detector == "d1-db-op-in-loop")
        .expect("d1 stats must be present")
}

/// The `downgradedToInfo` skip counter (absent ⇒ 0).
fn downgraded_to_info(stats: &DetectorStats) -> u64 {
    stats.skipped.get("downgradedToInfo").copied().unwrap_or(0)
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const FLOWFIELD_NOTE: &str = "temporary record, but FlowField calculation queries the flow targets";

// --- (a) CalcFields on a Blob field of a temp record → info (in-memory) -------

/// The CDO LoadFiles case: a TEMPORARY record, `CalcFields("File Blob")` where
/// "File Blob" is a Blob field (field_class "Normal", is_blob_like). The Blob load
/// is in-memory, so d1 downgrades to info exactly as before.
#[test]
fn calcfields_blob_on_temp_downgrades_to_info() {
    let src = r#"
table 50121 "CF Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("CF Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50122 "CF Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50121 "CF D1 Blob"
{
    procedure LoadFiles()
    var TempFiles: Record "CF Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("File Blob");
    end;
}
"#;
    let findings = run_d1(&[al("CFD1Blob", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "info",
        "CalcFields on a Blob (Normal) field of a temp record is in-memory → info. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause
            .contains("temporary record — not a SQL round-trip"),
        "Blob-field temp CalcFields keeps the in-memory temporary note. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains(FLOWFIELD_NOTE),
        "a pure-Normal-field temp CalcFields must NOT carry the FlowField note. rootCause: {}",
        f.root_cause
    );
}

// --- (b) CalcFields on a FlowField of a temp record → keeps firing ------------

/// A TEMPORARY record, `CalcFields("Amount")` where "Amount" is a FlowField. The
/// FlowField is computed against the physical flow targets (a real SQL query), so
/// d1 must KEEP FIRING at normal severity with the FlowField note — the RV-1 fix
/// (was wrongly downgraded to info before).
#[test]
fn calcfields_flowfield_on_temp_keeps_firing() {
    let src = r#"
table 50123 "FF Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50124 "FF Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50123 "FF D1 Flow"
{
    procedure SumFiles()
    var TempFiles: Record "FF Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("Amount");
    end;
}
"#;
    let findings = run_d1(&[al("FFD1Flow", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_ne!(
        f.severity, "info",
        "CalcFields on a FlowField of a temp record queries the flow targets → must fire. \
         rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains(FLOWFIELD_NOTE),
        "FlowField temp CalcFields must carry the honest FlowField note. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains("not a SQL round-trip"),
        "FlowField temp CalcFields must NOT carry the in-memory temporary note. rootCause: {}",
        f.root_cause
    );
}

// --- (c) unresolvable field arg → keeps firing (conservative) -----------------

/// A TEMPORARY record CalcFields naming a field NOT in the table. Unresolvable ⇒
/// conservative ⇒ keep firing with the FlowField note.
#[test]
fn calcfields_unresolvable_field_keeps_firing() {
    let src = r#"
table 50125 "UR Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50125 "UR D1 Unres"
{
    procedure LoadFiles()
    var TempFiles: Record "UR Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("Nonexistent Field");
    end;
}
"#;
    let findings = run_d1(&[al("URD1Unres", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_ne!(
        f.severity, "info",
        "an unresolvable field arg is conservative → keep firing. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains(FLOWFIELD_NOTE),
        "unresolvable temp CalcFields keeps the FlowField note. rootCause: {}",
        f.root_cause
    );
}

// --- (d) mixed field args: any FlowField → keeps firing -----------------------

/// `CalcFields("File Blob", "Amount")` on a temp record: "File Blob" is Normal but
/// "Amount" is a FlowField. ANY FlowField ⇒ keep firing.
#[test]
fn calcfields_mixed_args_any_flowfield_keeps_firing() {
    let src = r#"
table 50127 "MX Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("MX Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50128 "MX Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50127 "MX D1 Mixed"
{
    procedure LoadFiles()
    var TempFiles: Record "MX Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("File Blob", "Amount");
    end;
}
"#;
    let findings = run_d1(&[al("MXD1Mixed", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_ne!(
        f.severity, "info",
        "mixed args with ANY FlowField → keep firing. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains(FLOWFIELD_NOTE),
        "mixed-args temp CalcFields with a FlowField keeps the FlowField note. rootCause: {}",
        f.root_cause
    );
}

// --- (e) non-CalcFields temp op unchanged (regression guard) ------------------

/// A TEMPORARY record DeleteAll inside a loop. The RV-1 gate only touches
/// CalcFields/SetAutoCalcFields; a DeleteAll on a temp record still downgrades to
/// info exactly as Task 10 (regression guard).
#[test]
fn non_calcfields_temp_op_still_downgrades_to_info() {
    let src = r#"
table 50129 "NC Files"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50129 "NC D1 NonCalc"
{
    procedure Purge()
    var TempFiles: Record "NC Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.DeleteAll();
    end;
}
"#;
    let findings = run_d1(&[al("NCD1NonCalc", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "info",
        "a non-CalcFields temp op still downgrades to info (gate only affects CalcFields). \
         rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause
            .contains("temporary record — not a SQL round-trip"),
        "non-CalcFields temp op keeps the in-memory temporary note. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains(FLOWFIELD_NOTE),
        "non-CalcFields temp op must NOT carry the FlowField note. rootCause: {}",
        f.root_cause
    );
}

// --- (f) downgradedToInfo stat excludes a gated FlowField CalcFields ----------

/// The `downgradedToInfo` stat counts a direct in-loop known-temp op ONLY when it
/// genuinely downgrades. A Blob CalcFields on a temp record downgrades → counted.
/// A FlowField CalcFields on a temp record is GATED (fires, not info) → NOT counted.
#[test]
fn downgraded_to_info_stat_excludes_gated_flowfield_calcfields() {
    // Blob CalcFields on a temp record → genuinely downgrades → stat counts it.
    let blob_src = r#"
table 50131 "ST Blob Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50131 "ST D1 Blob"
{
    procedure LoadFiles()
    var TempFiles: Record "ST Blob Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("File Blob");
    end;
}
"#;
    let blob_stats = run_d1_stats(&[al("STD1Blob", blob_src)]);
    assert_eq!(
        downgraded_to_info(&blob_stats),
        1,
        "a Blob CalcFields on a temp record genuinely downgrades → counted. stats: {blob_stats:#?}"
    );

    // FlowField CalcFields on a temp record → GATED (fires) → NOT counted.
    let flow_src = r#"
table 50133 "ST Flow Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("ST Flow Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50134 "ST Flow Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50133 "ST D1 Flow"
{
    procedure SumFiles()
    var TempFiles: Record "ST Flow Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempFiles.CalcFields("Amount");
    end;
}
"#;
    let flow_stats = run_d1_stats(&[al("STD1Flow", flow_src)]);
    assert_eq!(
        downgraded_to_info(&flow_stats),
        0,
        "a GATED FlowField CalcFields fires (not info) → must NOT increment downgradedToInfo. \
         stats: {flow_stats:#?}"
    );
}

// --- (g) merge: FlowFieldGated path + physical path on the same op ------------

/// A helper `H(var Files)` does `Files.CalcFields("Amount")` (PD temp_state, "Amount"
/// is a FlowField). Caller A loops calling `H(TempFiles)` (a TEMPORARY local) → that
/// path resolves Temporary, gate trips → `FlowFieldGated` (fires). Caller B loops
/// calling `H(PhysFiles)` (a PHYSICAL local) → that path resolves `Physical` (fires).
/// Both share the SAME terminal op so `merge_by_terminal` collapses them to ONE
/// finding.
///
/// Both paths FIRE at the SAME op-based severity (CalcFields → high). The
/// terminal-centric assembly collapses them to ONE finding with one `LoopContext`
/// per caller; the FlowField fact must SURVIVE — carried structurally by the
/// `flowfield-on-temp` verdict on the temp caller's context (the redesign
/// replaced the old dual-verdict prose note). This is the defect the dedicated
/// `FlowFieldGated` variant fixes: faking `Physical` would have lost the FlowField
/// fact entirely.
#[test]
fn merge_flowfield_gated_with_physical_preserves_flowfield_note() {
    let src = r#"
table 50135 "MF Files"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("MF Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50136 "MF Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50135 "MF D1 Merge"
{
    procedure CalcHelper(var Files: Record "MF Files")
    begin
        Files.CalcFields("Amount");
    end;

    procedure CallerTemp()
    var TempFiles: Record "MF Files" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            CalcHelper(TempFiles);
    end;

    procedure CallerPhysical()
    var PhysFiles: Record "MF Files"; i: Integer;
    begin
        for i := 1 to 10 do
            CalcHelper(PhysFiles);
    end;
}
"#;
    let findings = run_d1(&[al("MFD1Merge", src)]);
    assert_eq!(
        findings.len(),
        1,
        "the two callers' paths must merge to one finding. findings: {findings:#?}"
    );
    let f = &findings[0];
    // Both paths FIRE (FlowFieldGated + Physical), so the finding is NOT info.
    assert_ne!(
        f.severity, "info",
        "a merged FlowFieldGated + physical finding must fire. rootCause: {}",
        f.root_cause
    );
    // Both verdicts are surfaced per VERDICT CLASS in cohortContexts[] — the
    // FlowField fact is preserved structurally by the dedicated `flowfield-on-temp`
    // verdict on the temp caller's cohort (the C6 cutover: per-class cohorts
    // replaced the old per-loop contexts + the dropped dual-verdict prose).
    let ccs = f
        .cohort_contexts
        .as_ref()
        .expect("d1 findings carry cohort_contexts");
    assert_eq!(
        ccs.len(),
        2,
        "one cohort per verdict class. cohortContexts: {ccs:#?}"
    );
    let verdicts: std::collections::BTreeSet<&str> =
        ccs.iter().map(|c| c.verdict.as_str()).collect();
    assert_eq!(
        verdicts,
        ["flowfield-on-temp", "physical"].into_iter().collect(),
        "the FlowField fact (flowfield-on-temp) + physical are both surfaced across \
         cohorts. cohortContexts: {ccs:#?}"
    );
    assert!(
        !f.root_cause.contains("temp state varies by caller"),
        "the dual-verdict prose note is gone (verdicts live in contexts). rootCause: {}",
        f.root_cause
    );
}
