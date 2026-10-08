//! D52 — Bulk write on a `var` record PARAMETER without a temp proof.
//! BCQuality community rule `guard-bulk-operations-with-istemporary`.
//!
//! d33 skips parameter receivers (caller-responsible); d52 is the parameter-side
//! complement: `DeleteAll`/`ModifyAll` on a record parameter with NO temp proof
//! (declared `temporary`, the G-2 `IsTemporary` entry guard, or the G-19
//! closed-world proof) and NO routine-local narrowing filter. Such helpers are
//! written for temp buffers; called with a real record they bulk-write the table.
//!
//! Severity: DeleteAll → high, ModifyAll → medium. Confidence: possible
//! (advisory — caller-side filters travel with the record var, so an unfiltered
//! callee op is not PROOF of an unfiltered write).
//!
//! Inert on cross-app contexts only via its normal skips (no resolver join used).

use std::collections::HashMap;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, is_known_temp, record_filter_applied_before, record_filtered_by_call_before,
};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d52-bulk-write-param-no-temp-guard";

const BULK_OPS: &[&str] = &["DeleteAll", "ModifyAll"];

pub fn detect_d52(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_known_temp = 0u64;
    let mut skipped_entry_guard = 0u64;
    let mut skipped_closed_world_temp = 0u64;
    let mut skipped_filtered = 0u64;
    let mut skipped_parse_incomplete = 0u64;

    for routine in &ws.routines {
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }

        // by-name → parameter index for the routine's record PARAMETERS.
        let param_records: HashMap<String, Option<u32>> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| (rv.name.to_lowercase(), rv.parameter_index))
            .collect();

        for op in &routine.record_operations {
            if !BULK_OPS.contains(&op.op.as_str()) {
                continue;
            }
            let var_key = op.record_variable_name.to_lowercase();
            let Some(&param_index) = param_records.get(&var_key) else {
                continue; // non-parameter receivers are d33's territory
            };
            candidates_considered += 1;

            // Temp proofs — a proven-temp buffer bulk-op is the pattern's
            // INTENDED use, never flagged.
            if is_known_temp(op) {
                skipped_known_temp += 1;
                continue;
            }
            if routine.entry_temp_guard_receiver.as_deref() == Some(var_key.as_str()) {
                skipped_entry_guard += 1;
                continue;
            }
            if param_index.is_some_and(|pi| {
                ctx.closed_world_temp_params
                    .contains(&(routine.id.clone(), pi))
            }) {
                skipped_closed_world_temp += 1;
                continue;
            }
            // A routine-local narrowing filter makes this a scoped cleanup.
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

            let table_name = op
                .table_id
                .as_deref()
                .and_then(|tid| ctx.table_by_id.get(tid).map(|t| t.name.clone()))
                .or_else(|| {
                    routine
                        .record_variables
                        .iter()
                        .find(|rv| rv.name.to_lowercase() == var_key)
                        .and_then(|rv| rv.table_name.clone())
                })
                .unwrap_or_else(|| "unknown table".to_string());

            let severity = if op.op == "DeleteAll" {
                "high"
            } else {
                "medium"
            };
            let confidence: FindingConfidence = to_confidence(&[], "possible");

            let id = format!("d52/{}/{}", routine.id, op.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: format!("{} on unguarded record parameter", op.op).into(),
                root_cause: format!(
                    "{} calls {} on the var record parameter {} ({}) without proving it \
                     temporary (no `temporary` declaration, no IsTemporary entry guard) and \
                     without a local filter — called with a real record this bulk-writes the \
                     whole table.",
                    routine.name, op.op, op.record_variable_name, table_name
                ),
                severity: severity.to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: vec![EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: Some(op.id.clone()),
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&op.source_anchor, routine),
                    note: format!(
                        "{} on parameter {} with no temp proof and no prior filter",
                        op.op, op.record_variable_name
                    ),
                }],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: id_list(op.table_id.iter().cloned()),
                fix_options: vec![FixOption {
                    description: format!(
                        "Add `if not {0}.IsTemporary() then Error(...)` as the first statement \
                         (or declare the parameter `temporary`), or apply a SetRange/SetFilter \
                         before {1}.",
                        op.record_variable_name, op.op
                    )
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
    stats.add_skip("knownTemp", skipped_known_temp);
    stats.add_skip("entryGuard", skipped_entry_guard);
    stats.add_skip("closedWorldTemp", skipped_closed_world_temp);
    stats.add_skip("filtered", skipped_filtered);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    Ok(DetectorOutput::no_diag(findings, stats))
}
