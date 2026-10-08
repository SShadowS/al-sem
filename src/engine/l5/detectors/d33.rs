//! D33 — Unfiltered bulk write (`DeleteAll` / `ModifyAll`). Port of al-sem
//! `src/detectors/d33-unfiltered-bulk-write.ts`.
//!
//! Flags `DeleteAll` / `ModifyAll` on a local record variable when no narrowing
//! filter (`SetRange` / `SetFilter`) has been applied on the same variable since
//! the last `Reset` (or the start of the routine).
//!
//! Skipped:
//!  - by-var parameter records (caller is responsible for filters).
//!  - temporary records (`tempState: { kind: "known", value: true }`).
//!  - operations whose tableId did not resolve.
//!  - parse-incomplete routines.
//!  - G-3: receivers provably filtered by a one-hop by-`var` helper call
//!    earlier in the routine (`record_filtered_by_call_before`, reusing the
//!    G-10 callee-summary machinery).
//!
//! Severity:
//!  - `DeleteAll` without filter → `critical`.
//!  - `ModifyAll` without filter → `high`.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, record_filter_applied_before, record_filtered_by_call_before,
};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d33-unfiltered-bulk-write";

const BULK_OPS: &[&str] = &["DeleteAll", "ModifyAll"];

pub fn detect_d33(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_filtered = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_parameter = 0u64;
    let mut skipped_unresolved_table = 0u64;
    let mut skipped_parse_incomplete = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" — source-only, every routine is primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }

        let param_record_names: std::collections::HashSet<String> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| rv.name.to_lowercase())
            .collect();

        for op in &routine.record_operations {
            if !BULK_OPS.contains(&op.op.as_str()) {
                continue;
            }
            candidates_considered += 1;

            let var_key = op.record_variable_name.to_lowercase();

            // Skip temporary records.
            if super::is_known_temp(op) {
                skipped_temp_record += 1;
                continue;
            }

            // Skip by-var parameter records.
            if param_record_names.contains(&var_key) {
                skipped_parameter += 1;
                continue;
            }

            // Skip if tableId is unresolved.
            let table_id = match &op.table_id {
                Some(id) => id.clone(),
                None => {
                    skipped_unresolved_table += 1;
                    continue;
                }
            };

            // Check whether a narrowing filter was applied before this op —
            // inline (SetRange/SetFilter record op), or G-3: by a one-hop
            // helper call that takes the receiver by-`var` and filters it.
            if record_filter_applied_before(&routine.record_operations, &var_key, op)
                || record_filtered_by_call_before(
                    routine,
                    ctx,
                    &op.record_variable_name,
                    &op.source_anchor,
                )
            {
                skipped_filtered += 1;
                continue;
            }

            // Resolve the table name for the finding text (fall back to table_id).
            let table_name = ctx
                .table_by_id
                .get(table_id.as_str())
                .map(|t| t.name.clone())
                .unwrap_or_else(|| table_id.clone());

            let severity = if op.op == "DeleteAll" {
                "critical"
            } else {
                "high"
            };

            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine),
                note: format!(
                    "{} on {} ({}) with no prior SetRange/SetFilter in this routine",
                    op.op, op.record_variable_name, table_name
                ),
            }];

            // id = d33/{routineId}/{op.id}
            let id = format!("d33/{}/{}", routine.id, op.id);
            let root_cause_key = id.clone();

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables = vec![table_id.clone()];

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} calls {} on {} ({}) with no SetRange/SetFilter applied since the last Reset \
                 — the operation affects every row in the table.",
                routine.name, op.op, op.record_variable_name, table_name
            );

            let fix_desc = format!(
                "Apply a SetRange / SetFilter on {} before calling {}, or confirm the \
                 unconditional whole-table operation is intentional and document it.",
                op.record_variable_name, op.op
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: format!("Unfiltered {}", op.op).into(),
                root_cause,
                severity: severity.to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description: fix_desc.into(),
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
    stats.add_skip("filtered", skipped_filtered);
    stats.add_skip("tempRecord", skipped_temp_record);
    stats.add_skip("parameter", skipped_parameter);
    stats.add_skip("unresolvedTable", skipped_unresolved_table);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    Ok(DetectorOutput::no_diag(findings, stats))
}
