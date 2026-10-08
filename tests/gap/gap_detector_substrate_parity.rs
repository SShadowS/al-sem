//! W1.0 demand-driven detector substrate — per-detector full-vs-minimal-ctx parity.
//!
//! This is the LICENSE for the substrate audit (Task 10). For EVERY registered
//! detector, the findings computed against the FULL context (`substrate::ALL`) MUST
//! equal the findings computed against a context built from ONLY that detector's
//! declared `requires` bits. If a detector reads a substrate field it did not declare,
//! the minimal context leaves that field empty and the detector's findings diverge —
//! failing this test and pinpointing the under-declared `requires`.
//!
//! Fixture choice matters: the run must actually POPULATE the gated substrate AND fire
//! its consuming detector, or the parity is vacuous (an empty substrate is empty in
//! both contexts). The set below was chosen from a substrate-population sweep so that
//! every gated substrate is populated — and exercised by a firing detector — on at
//! least one input:
//!   - SUMMARIES              — every fixture (d1/d2/d34/d43/d45 fire).
//!   - CORE_SUMMARIES (roles) — ws-d3/ws-d40/ws-d41/ws-d42/ws-d52 (parameter_roles).
//!   - CORE_SUMMARIES (uncert)— ws-txn-d48-pos (uncertainties_by_node, d48 fires).
//!   - TRANSACTION_SPANS      — ws-d8-commit-in-tx/ws-d34/ws-txn-d46-pos/ws-d50-pos.
//!   - CLOSED_WORLD_TEMP      — the INLINE closed-world-temp workspace below (no corpus
//!     fixture proves a closed-world temp param; it needs a `local` by-var record proc
//!     called only from all-temp callers — see `gap_g19_temp_param`). There d10/d1 are
//!     suppressed by the proof, so dropping CLOSED_WORLD_TEMP would make them fire.
//!
//! A detector whose findings are empty on ALL inputs for both contexts contributes only
//! weak coverage — acceptable for this wave (noted in the Task 10 commit message).

use al_sem::engine::l5::detector_context::build_detector_context;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::registry::{Detector, substrate};
use al_sem::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
};
use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;
use std::path::PathBuf;

const PARITY_FIXTURES: &[&str] = &[
    "ws-d8-commit-in-tx",    // SUMMARIES + TRANSACTION_SPANS + parameter_roles
    "ws-d34",                // SUMMARIES + TRANSACTION_SPANS (d34 fires)
    "ws-txn-d46-pos",        // TRANSACTION_SPANS (d46 fires)
    "ws-d50-pos",            // SUMMARIES + TRANSACTION_SPANS (d50 fires)
    "ws-d3",                 // parameter_roles (d3/d42 fire)
    "ws-d40",                // parameter_roles (d40 fires)
    "ws-d41",                // parameter_roles (d41 fires)
    "ws-d42",                // parameter_roles (d42 fires)
    "ws-d52",                // parameter_roles (d52 fires)
    "ws-txn-d48-pos",        // uncertainties_by_node (d48 fires)
    "ws-event-d45-deep",     // SUMMARIES (d45 fires)
    "ws-d1",                 // SUMMARIES (d1 fires)
    "ws-d1-setup-singleton", // SUMMARIES (d1 fires)
    "ws-d2",                 // SUMMARIES (d2/d55 fire)
    "ws-d10-self-mod",       // SUMMARIES (d1/d10 fire)
];

const CWT_APP_GUID: &str = "11111111-0000-0000-0000-00000cwtpar";

const CWT_TABLE: &str = r#"
table 50190 "CWT Parity Line"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// A `local` by-var record proc called ONLY from all-temp callers ⇒ its param is a
/// closed-world proven temp ⇒ `closed_world_temp_params` is NON-EMPTY, and d1/d10 are
/// suppressed on the in-loop self-Delete. Dropping CLOSED_WORLD_TEMP from either would
/// let the proof-set go empty and make them fire — the teeth for that substrate.
const CWT_CODEUNIT: &str = r#"
codeunit 50190 "CWT Parity Prune"
{
    local procedure BulkPrune(var Buf: Record "CWT Parity Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "CWT Parity Line" temporary;
    begin
        BulkPrune(TempLine);
    end;

    procedure RunB()
    var
        TempOther: Record "CWT Parity Line" temporary;
    begin
        BulkPrune(TempOther);
    end;
}
"#;

fn corpus_dir(fixture: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(fixture)
}

/// Assert full-vs-minimal parity for every detector over one resolved workspace.
fn assert_parity(
    resolved: &al_sem::engine::l3::l3_workspace::L3Resolved,
    detectors: &[Detector],
    label: &str,
) {
    // Build the full context ONCE; each detector's minimal context is rebuilt from
    // its own declared `requires`.
    let full_ctx = build_detector_context(resolved, substrate::ALL);
    for det in detectors {
        let min_ctx = build_detector_context(resolved, det.requires);
        let full = (det.run)(resolved, &full_ctx)
            .unwrap_or_else(|e| panic!("detector {} failed on full ctx ({label}): {e}", det.name));
        let minimal = (det.run)(resolved, &min_ctx).unwrap_or_else(|e| {
            panic!("detector {} failed on minimal ctx ({label}): {e}", det.name)
        });
        assert_eq!(
            full.findings, minimal.findings,
            "detector {} under-declares its substrate requirements ({label})",
            det.name
        );
    }
}

#[test]
fn every_detector_parity_between_full_and_minimal_ctx() {
    let detectors = registered_detectors();

    for fixture in PARITY_FIXTURES {
        let dir = corpus_dir(fixture);
        let resolved =
            assemble_and_resolve_workspace_program(&dir, MODEL_INSTANCE_ID_DEFAULT, false)
                .unwrap_or_else(|| panic!("fixture {fixture} must assemble"));
        assert_parity(&resolved, &detectors, fixture);
    }

    // Closed-world-temp teeth: an inline workspace whose proven-temp param populates
    // `closed_world_temp_params` (corpus fixtures never do).
    let cwt = assemble_and_resolve_inline_program_default(
        &[
            ("src/CwtLine.al".to_string(), CWT_TABLE.to_string()),
            ("src/CwtPrune.al".to_string(), CWT_CODEUNIT.to_string()),
        ],
        CWT_APP_GUID,
    );
    assert_parity(&cwt, &detectors, "inline-closed-world-temp");
}
