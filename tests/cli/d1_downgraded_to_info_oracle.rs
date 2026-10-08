//! D1 `downgradedToInfo` native oracle.
//!
//! The cli-a stats differential corpus never exercises d1's `downgradedToInfo`
//! predicate (no fixture has a temporary-record DB op in a loop), so a port bug in
//! that counter would slip through. This oracle constructs an inline workspace with
//! known-temp in-loop DB ops and asserts the counter is computed PER-DIRECT-IN-LOOP-OP
//! (pre-merge, direct branch only) — mirroring al-sem `d1-db-op-in-loop.ts:320-322`.
//!
//! It also locks the other corpus-invisible d1 predicates (`opaqueCallee`,
//! `dynamicDispatch`, `parseIncomplete`) against regression by asserting they are
//! ABSENT (present-iff-nonzero) on a clean workspace.
//!
//! Before the post-merge-text-filter → real-counter fix, the over-count branch of
//! this test FAILED (the old filter counted a transitive merged info finding that
//! TS never counts, yielding 2 instead of 1).

use std::collections::BTreeMap;

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000d1abc";

/// Run d1 in isolation over an inline workspace and return its `skipped` map.
fn run_d1_skipped(files: &[(String, String)]) -> (BTreeMap<String, u64>, usize) {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    let out = run_detectors(&resolved, &d1);
    assert_eq!(
        out.detector_stats.len(),
        1,
        "exactly one DetectorStats entry expected"
    );
    let stats = &out.detector_stats[0];
    assert_eq!(stats.detector, "d1-db-op-in-loop");
    (stats.skipped.clone(), stats.findings_emitted)
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// One direct in-loop temp op (TS: downgradedToInfo = 1) PLUS a helper whose temp op
/// is NOT itself in a loop (so the direct branch never counts it) but IS reached
/// transitively (path b) from TWO in-loop callers. Those two transitive paths share
/// a terminal so `merge_by_terminal` collapses them to ONE info finding.
///
/// Expected `downgradedToInfo` = 1 (the single DIRECT in-loop temp op).
///   - The OLD post-merge text filter counted 1 (direct) + 1 (merged transitive info
///     finding whose rootCause contains "temporary record") = 2 → FAILED here.
///   - The NEW per-direct-op counter counts only the direct op = 1. The helper's
///     temp op is not in-loop in the helper, and the callers' in-loop CALLS are calls
///     (not direct ops), so neither feeds downgradedToInfo.
#[test]
fn downgraded_to_info_counts_direct_ops_only_not_merged_transitive() {
    let src = r#"
table 50101 "T1 Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50101 "T1 D1 Temp"
{
    // DIRECT in-loop temp op — the ONLY downgradedToInfo the detector should count.
    procedure DirectTemp()
    var TempCust: Record "T1 Cust" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempCust.Modify();
    end;

    // Helper with a temp op that is NOT in a loop here — only becomes in-loop via the
    // callers below. The direct branch never counts it; the transitive path (b) does
    // produce an info finding TS NEVER counts toward downgradedToInfo.
    procedure TempHelper(var TempCust: Record "T1 Cust" temporary)
    begin
        TempCust.Modify();
    end;

    procedure Caller1()
    var TempCust: Record "T1 Cust" temporary; i: Integer;
    begin
        for i := 1 to 3 do
            TempHelper(TempCust);
    end;

    procedure Caller2()
    var TempCust: Record "T1 Cust" temporary; i: Integer;
    begin
        for i := 1 to 7 do
            TempHelper(TempCust);
    end;
}
"#;
    let files = vec![al("T1D1Temp", src)];
    let (skipped, _emitted) = run_d1_skipped(&files);

    assert_eq!(
        skipped.get("downgradedToInfo").copied(),
        Some(1),
        "downgradedToInfo must count only the single DIRECT in-loop temp op (NOT \
         transitive/merged findings). Full skipped map: {skipped:?}"
    );
    // The other corpus-invisible predicates must be absent on this clean workspace.
    assert_eq!(skipped.get("opaqueCallee"), None, "skipped: {skipped:?}");
    assert_eq!(skipped.get("dynamicDispatch"), None, "skipped: {skipped:?}");
    assert_eq!(skipped.get("parseIncomplete"), None, "skipped: {skipped:?}");
}

/// TWO direct in-loop temp ops in the SAME routine ⇒ downgradedToInfo = 2.
/// (Distinct ops ⇒ distinct findings; this guards the per-op increment count.)
#[test]
fn downgraded_to_info_counts_each_direct_temp_op() {
    let src = r#"
table 50102 "T2 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50102 "T2 D1 Temp"
{
    procedure TwoTempOps()
    var TempCust: Record "T2 Cust" temporary; i: Integer;
    begin
        for i := 1 to 10 do begin
            TempCust.Insert();
            TempCust.Modify();
        end;
    end;
}
"#;
    let files = vec![al("T2D1Temp", src)];
    let (skipped, _emitted) = run_d1_skipped(&files);
    assert_eq!(
        skipped.get("downgradedToInfo").copied(),
        Some(2),
        "two distinct DIRECT in-loop temp ops ⇒ downgradedToInfo = 2. skipped: {skipped:?}"
    );
}

/// A workspace with NO temp in-loop ops ⇒ downgradedToInfo absent (present-iff-nonzero).
#[test]
fn downgraded_to_info_absent_when_no_temp_ops() {
    let src = r#"
table 50103 "T3 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50103 "T3 D1 NonTemp"
{
    procedure NonTemp()
    var Cust: Record "T3 Cust"; i: Integer;
    begin
        for i := 1 to 10 do
            Cust.Modify();
    end;
}
"#;
    let files = vec![al("T3D1NonTemp", src)];
    let (skipped, _emitted) = run_d1_skipped(&files);
    assert_eq!(
        skipped.get("downgradedToInfo"),
        None,
        "no temp in-loop ops ⇒ downgradedToInfo must be absent. skipped: {skipped:?}"
    );
}
