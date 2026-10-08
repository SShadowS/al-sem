//! Task 14 (temp-state-tracking): the **metamorphic soundness oracle** (RV-2).
//!
//! ## The governing property (RV-2)
//!
//! Adding the `temporary` modifier to a record declaration can only make that record
//! MORE temporary, never less. Temp-ness is a *suppression* signal in this epoch, so
//! the metamorphic edit "add ` temporary`" can only ever cause findings to be REMOVED
//! or DOWNGRADED — never ADDED, never UPGRADED — with ONE carve-out (RV-1):
//!
//!   FlowField CalcFields/SetAutoCalcFields findings are INVARIANT under the edit. A
//!   temporary record's FlowField still evaluates its CalcFormula against the physical
//!   flow targets (a real SQL round-trip), so adding `temporary` must NOT suppress
//!   them — they keep firing at the SAME severity.
//!
//! ## What the oracle enforces (BOTH directions, strict)
//!
//! For each fixture, the analyzer is run over the ORIGINAL source and over the
//! `temporary`-edited source, and the two `Finding` sets are compared by a stable key
//! `(detector, file, line, col, op-anchor)`:
//!
//!   1. Non-carve-out findings — the edited set is a SUBSET of the original under
//!      "removed or downgraded": every key present in the edited set was present in the
//!      original set at a severity >= the edited severity. No NEW key, no UPGRADE.
//!
//!   2. Carve-out findings (FlowField CalcFields) — the set is INVARIANT: identical keys,
//!      identical severities, in BOTH the original and the edited run.
//!
//! ## Why a metamorphic oracle (vs. a golden)
//!
//! The per-fixture goldens pin the analyzer's OUTPUT; this oracle pins a *relation
//! between two outputs* that must hold no matter how the detectors evolve. It is the
//! mechanical guard for the whole epoch's suppression direction: if any detector ever
//! starts ADDING or UPGRADING a finding when a record is made temporary (outside the
//! FlowField carve-out), or starts SUPPRESSING a FlowField CalcFields, this oracle goes
//! red — a genuine product-soundness signal, NOT a golden to refresh.
//!
//! These drive the REAL default detector set over inline AL workspaces via
//! `assemble_and_resolve_inline_program_default` + `run_detectors` (same in-process entry as
//! `tests/temp_state_d1_path.rs` / `tests/temp_state_calcfields.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000014ec70";

// ===========================================================================
// In-process finding extraction (mirrors tests/temp_state_d1_path.rs)
// ===========================================================================

/// Run the FULL default detector set over an inline workspace and return the raw
/// internal `Finding`s. We use the internal model (not the projected/filtered gate
/// output) because the oracle needs the raw `detector` + `primary_location` +
/// `severity` triple, with NO min-severity / scope / suppression filtering — the
/// property is about the analyzer's RAW finding production, before any presentation
/// gate that could itself drop a finding for orthogonal reasons.
fn run_all_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors = registered_detectors();
    assert!(
        !detectors.is_empty(),
        "the default detector registry must be non-empty"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// ===========================================================================
// Severity ordering — matches src/engine/gate/filter.rs::sev_rank semantics.
// ===========================================================================

/// Total order on severity (higher = worse). Mirrors the gate's `sev_rank`
/// (critical > high > medium > low > info). An unrecognized severity ranks ABOVE
/// critical so an unexpected label can never silently satisfy a "<=" downgrade check.
fn sev_rank(sev: &str) -> u8 {
    match sev {
        "info" => 0,
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        "critical" => 4,
        _ => u8::MAX,
    }
}

// ===========================================================================
// Stable finding key — detector + precise location. Location is the ANCHOR the
// edit must not move (the op site in the loop body): we append ` temporary` to the
// END of the record-var declaration LINE, which does not shift any later line/column.
// ===========================================================================

/// A location- and detector-stable key for a finding. Severity is intentionally
/// NOT part of the key (we compare severities as the *value* keyed by this), so a
/// DOWNGRADE is recognized as "same finding, lower severity" rather than as a removal
/// plus an addition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct FindingKey {
    detector: String,
    file: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

fn key_of(f: &Finding) -> FindingKey {
    let loc = f.actionable_anchor.as_ref().unwrap_or(&f.primary_location);
    FindingKey {
        detector: f.detector.clone(),
        file: loc.source_unit_id.clone(),
        start_line: loc.start_line,
        start_column: loc.start_column,
        end_line: loc.end_line,
        end_column: loc.end_column,
    }
}

// ===========================================================================
// The metamorphic edit: add ` temporary` to a chosen record declaration.
// ===========================================================================

/// Mechanically transform `source` by appending ` temporary` to the record-variable
/// declaration whose target table is `table_name`. The ONLY edit is inserting the
/// keyword; nothing else moves. Targets a declaration of the form:
///
///   `<Var>: Record "<table_name>";`           →  `<Var>: Record "<table_name>" temporary;`
///   `<Var>: Record "<table_name>"; i: Integer;`→ `<Var>: Record "<table_name>" temporary; i: Integer;`
///
/// We find the literal `Record "<table_name>"` not already followed by ` temporary`
/// and insert ` temporary` immediately after the closing quote. Deterministic and
/// obvious; panics if the target is absent or already temporary (a fixture-authoring
/// guard — the oracle must always actually perform the edit).
fn add_temporary(source: &str, table_name: &str) -> String {
    let needle = format!("Record \"{table_name}\"");
    let already = format!("Record \"{table_name}\" temporary");
    assert!(
        !source.contains(&already),
        "add_temporary: `{table_name}` is already temporary — the edit would be a no-op"
    );
    let pos = source
        .find(&needle)
        .unwrap_or_else(|| panic!("add_temporary: `{needle}` not found in source"));
    let insert_at = pos + needle.len();
    let mut edited = String::with_capacity(source.len() + " temporary".len());
    edited.push_str(&source[..insert_at]);
    edited.push_str(" temporary");
    edited.push_str(&source[insert_at..]);
    edited
}

// ===========================================================================
// The comparison: assert the RV-2 relation between original & edited finding sets.
// ===========================================================================

/// How a fixture's record op behaves under the edit. Lets one harness assert both
/// the "shrink/downgrade" direction and the FlowField "invariant" carve-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carveout {
    /// Ordinary suppression direction: edited ⊆ original under "removed or downgraded".
    SuppressionAllowed,
    /// RV-1 FlowField carve-out: the finding set must be INVARIANT (identical) under the edit.
    Invariant,
}

/// Assert the RV-2 property for one fixture.
///
/// `class` selects the direction:
///   - `SuppressionAllowed`: every edited key must have existed in the original at a
///     severity >= the edited severity (no new key, no upgrade). Removals are allowed.
///   - `Invariant`: the original and edited key→severity maps must be EQUAL.
fn assert_rv2(fixture: &str, original: &str, edited: &str, class: Carveout) {
    use std::collections::BTreeMap;

    let orig_findings = run_all_detectors(&[al(fixture, original)]);
    let edit_findings = run_all_detectors(&[al(fixture, edited)]);

    // Build key → severity maps. A duplicate key with differing severity within ONE
    // run would itself be a bug; assert uniqueness to keep the relation well-defined.
    let to_map = |fs: &[Finding], label: &str| -> BTreeMap<FindingKey, String> {
        let mut m: BTreeMap<FindingKey, String> = BTreeMap::new();
        for f in fs {
            let k = key_of(f);
            if let Some(prev) = m.insert(k.clone(), f.severity.clone()) {
                assert_eq!(
                    prev, f.severity,
                    "[{fixture}/{label}] duplicate finding key {k:?} with differing severities \
                     ({prev} vs {}) — the stable key is not unique within a run",
                    f.severity
                );
            }
        }
        m
    };

    let orig = to_map(&orig_findings, "original");
    let edit = to_map(&edit_findings, "edited");

    // Sanity: the ORIGINAL run must actually produce a finding, else the fixture is
    // degenerate and the oracle would pass vacuously.
    assert!(
        !orig.is_empty(),
        "[{fixture}] the ORIGINAL (physical) source produced NO findings — the fixture is \
         degenerate; the oracle cannot prove anything. original findings: {orig_findings:#?}"
    );

    match class {
        Carveout::SuppressionAllowed => {
            // Direction 1: edited ⊆ original under "removed or downgraded".
            for (k, edit_sev) in &edit {
                match orig.get(k) {
                    None => panic!(
                        "[{fixture}] RV-2 VIOLATION: adding `temporary` ADDED a new finding that \
                         did not exist in the physical original.\n  added key: {k:?}\n  severity: \
                         {edit_sev}\n  This violates the suppression-direction rule (findings may \
                         only be removed or downgraded). ESCALATE.\n  original keys: {:#?}\n  \
                         edited keys: {:#?}",
                        orig.keys().collect::<Vec<_>>(),
                        edit.keys().collect::<Vec<_>>()
                    ),
                    Some(orig_sev) => assert!(
                        sev_rank(edit_sev) <= sev_rank(orig_sev),
                        "[{fixture}] RV-2 VIOLATION: adding `temporary` UPGRADED a finding's \
                         severity ({orig_sev} → {edit_sev}).\n  key: {k:?}\n  Findings may only be \
                         downgraded, never upgraded. ESCALATE."
                    ),
                }
            }
            // The fixture is designed to SUPPRESS or DOWNGRADE: assert the edit actually
            // had an observable softening effect (else it is not exercising the property).
            let softened = edit.len() < orig.len()
                || orig
                    .iter()
                    .any(|(k, ov)| edit.get(k).is_some_and(|ev| sev_rank(ev) < sev_rank(ov)));
            assert!(
                softened,
                "[{fixture}] the suppressing fixture showed NO softening (no removal, no \
                 downgrade) when `temporary` was added — the fixture is not exercising the \
                 suppression direction.\n  original: {orig:#?}\n  edited: {edit:#?}"
            );
        }
        Carveout::Invariant => {
            // Direction 2 (RV-1 carve-out): identical key→severity maps.
            assert_eq!(
                orig, edit,
                "[{fixture}] RV-2 CARVE-OUT VIOLATION: a FlowField CalcFields/SetAutoCalcFields \
                 finding set CHANGED under the `temporary` edit. It MUST be invariant (a temp \
                 record's FlowField still hits SQL).\n  original: {orig:#?}\n  edited: {edit:#?}\n  \
                 ESCALATE."
            );
        }
    }
}

// ===========================================================================
// Fixtures
//
// Each is a standalone single-codeunit + table(s) AL source. The chosen record var is
// declared against a named table so `add_temporary` can target it deterministically.
// ===========================================================================

/// (1) SUPPRESS: a DeleteAll on a buffer record inside a loop. Physical → d33/d1 fire;
/// adding `temporary` makes the record in-memory ⇒ findings removed or downgraded to
/// info. (Non-carve-out: ordinary suppression direction.)
const FX_DELETEALL: &str = r#"
table 50901 "ORC DelAll Tab"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50901 "ORC DelAll"
{
    procedure Purge()
    var Buf: Record "ORC DelAll Tab"; i: Integer;
    begin
        for i := 1 to 10 do
            Buf.DeleteAll();
    end;
}
"#;
const FX_DELETEALL_TABLE: &str = "ORC DelAll Tab";

/// (2) SUPPRESS: a Modify in a loop. Physical → d1 high; temp → info. (Non-carve-out.)
const FX_MODIFY_LOOP: &str = r#"
table 50903 "ORC Mod Tab"
{
    fields { field(1; "No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50903 "ORC Mod"
{
    procedure Touch()
    var Rec: Record "ORC Mod Tab"; i: Integer;
    begin
        for i := 1 to 10 do
            Rec.Modify();
    end;
}
"#;
const FX_MODIFY_LOOP_TABLE: &str = "ORC Mod Tab";

/// (3) SUPPRESS: CalcFields on a BLOB (Normal) field in a loop. Physical CalcFields is a
/// SQL round-trip; on a temp record the Blob load is in-memory ⇒ downgrade to info.
/// This is the NON-FlowField CalcFields case — it IS subject to suppression. (Non-carve-out.)
const FX_CALCFIELDS_BLOB: &str = r#"
table 50905 "ORC Blob Tab"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "File Blob"; Blob) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50905 "ORC Blob"
{
    procedure LoadFiles()
    var Rec: Record "ORC Blob Tab"; i: Integer;
    begin
        for i := 1 to 10 do
            Rec.CalcFields("File Blob");
    end;
}
"#;
const FX_CALCFIELDS_BLOB_TABLE: &str = "ORC Blob Tab";

/// (4) INVARIANT (RV-1 carve-out): CalcFields on a FLOWFIELD in a loop. A temp record's
/// FlowField still evaluates its CalcFormula against the physical flow targets — a real
/// SQL query — so adding `temporary` must NOT suppress or downgrade it. The finding set
/// must be byte-identical (key + severity) in both runs.
const FX_CALCFIELDS_FLOWFIELD: &str = r#"
table 50907 "ORC Flow Tab"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(3; "Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("ORC Flow Ledger".Amount where("File No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50908 "ORC Flow Ledger"
{
    fields { field(1; "File No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "File No.") { } }
}

codeunit 50907 "ORC Flow"
{
    procedure SumFiles()
    var Rec: Record "ORC Flow Tab"; i: Integer;
    begin
        for i := 1 to 10 do
            Rec.CalcFields("Amount");
    end;
}
"#;
const FX_CALCFIELDS_FLOWFIELD_TABLE: &str = "ORC Flow Tab";

/// (5) CONTROL: a physical record op that is NOT affected by temp-ness in a way that
/// would add/upgrade. The edit may only remove/downgrade — same suppression direction
/// as the other ops, here exercised as a generic Modify-without-Get + db-op pattern via
/// a Get/Modify in a loop. This is the "physical record op control": whatever fires must
/// not be made WORSE by adding `temporary`.
const FX_CONTROL_GET_MODIFY: &str = r#"
table 50909 "ORC Ctl Tab"
{
    fields { field(1; "No."; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50909 "ORC Ctl"
{
    procedure Recompute()
    var Rec: Record "ORC Ctl Tab"; i: Integer;
    begin
        for i := 1 to 10 do begin
            Rec.Get(Format(i));
            Rec.Amount := i;
            Rec.Modify();
        end;
    end;
}
"#;
const FX_CONTROL_GET_MODIFY_TABLE: &str = "ORC Ctl Tab";

// ===========================================================================
// Tests
// ===========================================================================

/// The metamorphic edit helper is correct: it inserts ` temporary` exactly once,
/// immediately after the targeted `Record "Name"`, and shifts nothing before it.
#[test]
fn add_temporary_edit_is_surgical() {
    let edited = add_temporary(FX_DELETEALL, FX_DELETEALL_TABLE);
    assert!(
        edited.contains(r#"Record "ORC DelAll Tab" temporary;"#),
        "edit must produce `Record \"ORC DelAll Tab\" temporary;`. got:\n{edited}"
    );
    // Exactly one ` temporary` keyword introduced.
    assert_eq!(
        edited.matches(" temporary").count(),
        1,
        "exactly one ` temporary` keyword must be inserted"
    );
    // Everything up to the insertion point is unchanged (the loop body, which carries
    // the finding anchors, sits AFTER the declaration → its byte offsets are preserved
    // relative to the start, so line/column anchors of ops do not move within the line).
    let pos = FX_DELETEALL.find(r#"Record "ORC DelAll Tab""#).unwrap();
    let prefix_len = pos + r#"Record "ORC DelAll Tab""#.len();
    assert_eq!(&edited[..prefix_len], &FX_DELETEALL[..prefix_len]);
}

/// SUPPRESS fixtures: adding `temporary` may only REMOVE or DOWNGRADE findings.
#[test]
fn deleteall_buffer_suppresses_or_downgrades() {
    let edited = add_temporary(FX_DELETEALL, FX_DELETEALL_TABLE);
    assert_rv2(
        "ORCDelAll",
        FX_DELETEALL,
        &edited,
        Carveout::SuppressionAllowed,
    );
}

#[test]
fn modify_in_loop_suppresses_or_downgrades() {
    let edited = add_temporary(FX_MODIFY_LOOP, FX_MODIFY_LOOP_TABLE);
    assert_rv2(
        "ORCMod",
        FX_MODIFY_LOOP,
        &edited,
        Carveout::SuppressionAllowed,
    );
}

#[test]
fn calcfields_blob_suppresses_or_downgrades() {
    let edited = add_temporary(FX_CALCFIELDS_BLOB, FX_CALCFIELDS_BLOB_TABLE);
    assert_rv2(
        "ORCBlob",
        FX_CALCFIELDS_BLOB,
        &edited,
        Carveout::SuppressionAllowed,
    );
}

/// INVARIANT fixture (RV-1 carve-out): a FlowField CalcFields finding set must be
/// IDENTICAL (key + severity) before and after the `temporary` edit.
#[test]
fn calcfields_flowfield_is_invariant() {
    let edited = add_temporary(FX_CALCFIELDS_FLOWFIELD, FX_CALCFIELDS_FLOWFIELD_TABLE);
    assert_rv2(
        "ORCFlow",
        FX_CALCFIELDS_FLOWFIELD,
        &edited,
        Carveout::Invariant,
    );
}

/// CONTROL physical record op: the edit may only soften, never harden.
#[test]
fn control_physical_op_only_softens() {
    let edited = add_temporary(FX_CONTROL_GET_MODIFY, FX_CONTROL_GET_MODIFY_TABLE);
    assert_rv2(
        "ORCCtl",
        FX_CONTROL_GET_MODIFY,
        &edited,
        Carveout::SuppressionAllowed,
    );
}

/// Whole-corpus guard: across EVERY fixture, the *global* relation holds — no finding
/// key that is absent from the original ever appears in the edited run at a HIGHER
/// severity than it (if present) had originally. This is the broad anti-regression net
/// over the full default detector set (it would catch a NEW detector that violates RV-2
/// on any of these shapes, even one not anticipated per-fixture). FlowField findings are
/// allowed to persist unchanged (they satisfy "<= original" trivially).
#[test]
fn corpus_wide_no_addition_no_upgrade() {
    use std::collections::BTreeMap;
    let cases: &[(&str, &str, &str)] = &[
        ("ORCDelAll", FX_DELETEALL, FX_DELETEALL_TABLE),
        ("ORCMod", FX_MODIFY_LOOP, FX_MODIFY_LOOP_TABLE),
        ("ORCBlob", FX_CALCFIELDS_BLOB, FX_CALCFIELDS_BLOB_TABLE),
        (
            "ORCFlow",
            FX_CALCFIELDS_FLOWFIELD,
            FX_CALCFIELDS_FLOWFIELD_TABLE,
        ),
        ("ORCCtl", FX_CONTROL_GET_MODIFY, FX_CONTROL_GET_MODIFY_TABLE),
    ];
    for (name, src, table) in cases {
        let edited = add_temporary(src, table);
        let orig: BTreeMap<FindingKey, String> = run_all_detectors(&[al(name, src)])
            .iter()
            .map(|f| (key_of(f), f.severity.clone()))
            .collect();
        let edit: BTreeMap<FindingKey, String> = run_all_detectors(&[al(name, &edited)])
            .iter()
            .map(|f| (key_of(f), f.severity.clone()))
            .collect();
        for (k, edit_sev) in &edit {
            match orig.get(k) {
                None => panic!(
                    "[corpus/{name}] RV-2 VIOLATION: `temporary` ADDED finding {k:?} (severity \
                     {edit_sev}) absent from the physical original. ESCALATE."
                ),
                Some(orig_sev) => assert!(
                    sev_rank(edit_sev) <= sev_rank(orig_sev),
                    "[corpus/{name}] RV-2 VIOLATION: `temporary` UPGRADED {k:?} from {orig_sev} \
                     to {edit_sev}. ESCALATE."
                ),
            }
        }
    }
}
