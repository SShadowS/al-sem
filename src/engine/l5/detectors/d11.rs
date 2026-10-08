//! D11 — Modify without Get. Port of al-sem
//! `src/detectors/d11-modify-without-get.ts`.
//!
//! Flags `Modify` / `Validate` on a record variable that was never loaded
//! earlier in the same routine (no prior Get/FindFirst/FindLast/FindSet/Find/
//! Next/Init/Insert/Copy before the mutating op). Skips by-var parameter records
//! (the caller is responsible for loading them). G-10/G-16: a prior call that
//! loads the record also counts — the platform built-in `GetBySystemId` and any
//! resolved callee chain that loads the by-var argument (bounded multi-hop
//! callee summary, `detectors::record_loaded_by_call_before`) — as does a
//! whole-record assignment from a provably-loaded var
//! (`detectors::record_loaded_by_assignment_before`).
//!
//! `ModifyAll` is intentionally excluded — it operates on the filtered set,
//! not the current record, and the standard pattern is `SetRange(…);
//! ModifyAll(field, value)` with no prior Get/Find.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    RECORD_LOAD_OPS, anchor_of, before_anchor, is_platform_loaded_trigger_rec,
    record_loaded_by_assignment_before, record_loaded_by_call_before,
};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d11-modify-without-get";

/// The recognized load ops — shared with the G-10 one-hop callee summary
/// (`detectors::RECORD_LOAD_OPS`), so both stay in lockstep.
const LOAD_OPS: &[&str] = RECORD_LOAD_OPS;

const MUTATING_OPS: &[&str] = &["Modify", "Validate"];

pub fn detect_d11(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_parameter = 0u64;
    let mut skipped_trigger_rec = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }
        candidates_considered += 1;

        // Collect by-var parameter record variable names (lowercased).
        let param_record_names: std::collections::HashSet<String> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| rv.name.to_lowercase())
            .collect();

        for op in &routine.record_operations {
            if !MUTATING_OPS.contains(&op.op.as_str()) {
                continue;
            }
            let var_key = op.record_variable_name.to_lowercase();
            if param_record_names.contains(&var_key) {
                skipped_parameter += 1;
                continue;
            }
            // G-9: the platform loaded the implicit `Rec` before a page/table
            // field trigger runs — Modify/Validate on it is not "without Get".
            if is_platform_loaded_trigger_rec(routine, &op.record_variable_name) {
                skipped_trigger_rec += 1;
                continue;
            }

            // Is there any LOAD_OP on the same record variable STRICTLY BEFORE this op?
            let loaded_before = routine.record_operations.iter().any(|other| {
                LOAD_OPS.contains(&other.op.as_str())
                    && other.record_variable_name.to_lowercase() == var_key
                    && before_anchor(&other.source_anchor, &op.source_anchor)
            });
            if loaded_before {
                continue;
            }
            // G-10/G-16(a): the record was loaded by a CALL strictly before
            // this op — a platform built-in (`GetBySystemId`) or a resolved
            // callee chain that loads the by-var argument (bounded multi-hop
            // callee summary).
            if record_loaded_by_call_before(
                routine,
                ctx,
                &op.record_variable_name,
                &op.source_anchor,
            ) {
                continue;
            }
            // G-16(b): the record was ASSIGNED from a provably-loaded record
            // var strictly before this op (`RecB := RecA` with RecA loaded).
            if record_loaded_by_assignment_before(
                routine,
                ctx,
                &op.record_variable_name,
                &op.source_anchor,
            ) {
                continue;
            }

            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine),
                note: format!(
                    "{} on {} with no prior Get/Find in this routine",
                    op.op, op.record_variable_name
                ),
            }];

            let id = format!("d11/{}/{}", routine.id, op.id);
            let root_cause_key = id.clone();

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables: Vec<String> = match &op.table_id {
                Some(t) => vec![t.clone()],
                None => Vec::new(),
            };

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} calls {} on {} but never loaded it — the record's state may be stale or partial.",
                routine.name, op.op, op.record_variable_name
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Modify without Get".into(),
                root_cause,
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description:
                        "Load the record with Get / FindFirst before mutating, or pass it in \
                         as a var parameter from a caller that loaded it."
                            .into(),
                    safety: "high".into(),
                }],
                provenance: vec![Evidence {
                    source: "tree-sitter",
                    note: None,
                }],
                actionable_anchor: None,
                fingerprint: None,
                event_kind: None,
                cross_extension_subscribers: None,
                cohort_contexts: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    stats.add_skip("other", skipped_parameter);
    stats.add_skip("triggerRec", skipped_trigger_rec);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
