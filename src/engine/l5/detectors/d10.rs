//! D10 — self-modifying loop. Port of al-sem
//! `src/detectors/d10-self-modifying-loop.ts`.
//!
//! Detects a mutating op (`Modify`, `ModifyAll`, `Validate`, `Delete`,
//! `DeleteAll`, `Rename`) on the SAME record variable that is driving the loop cursor
//! (identified via `Next()` inside the loop body). The cursor's snapshot may
//! be corrupted when the iterating record is mutated inside its own loop.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).
//!
//! G-13: ops whose record is `Known(true)` TEMPORARY are skipped — an
//! in-memory cursor self-modify is safe (cursor corruption only applies to
//! physical SQL cursors). Mirrors the d33 temp gate. Physical/Unknown keep
//! firing (suppression-direction safe).

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d10-self-modifying-loop";

// Rename moves the driving record to a new key, the strongest self-modify.
const MUTATING_OPS: &[&str] = &[
    "Modify",
    "ModifyAll",
    "Validate",
    "Delete",
    "DeleteAll",
    "Rename",
];

pub fn detect_d10(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_temp_record = 0u64;

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

        // Map loopId → record variable that drives the loop.
        // We use Next() as the signal: it is always emitted inside the repeat/until body
        // (loopStack is non-empty), whereas FindSet/FindFirst appear in the `if` guard
        // before the `repeat` keyword and therefore have loopStack === [].
        let mut loop_driver: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for op in &routine.record_operations {
            if op.op != "Next" {
                continue;
            }
            let loop_id = op.loop_stack.last().cloned();
            let Some(loop_id) = loop_id else { continue };
            loop_driver
                .entry(loop_id)
                .or_insert_with(|| op.record_variable_name.to_lowercase());
        }

        for op in &routine.record_operations {
            if !MUTATING_OPS.contains(&op.op.as_str()) {
                continue;
            }
            let loop_id = op.loop_stack.last().cloned();
            let Some(loop_id) = loop_id else { continue };
            let driver = match loop_driver.get(&loop_id) {
                Some(d) => d.clone(),
                None => continue,
            };
            if op.record_variable_name.to_lowercase() != driver {
                continue;
            }

            // G-13: skip Known(true) TEMPORARY records — no SQL cursor to
            // corrupt when the iterating record is in-memory (same gate as d33).
            if let Some(ts) = &op.temp_state
                && ts.kind == "known"
                && ts.value == Some(true)
            {
                skipped_temp_record += 1;
                continue;
            }
            // G-19: a ParameterDependent record (keyword-less by-var param)
            // whose `(routine, param)` is CLOSED-WORLD PROVEN temp — a `local`
            // routine all of whose resolved callers pass a Known(true) temp —
            // is treated exactly like Known(true). Any unproven PD keeps firing.
            if crate::engine::l5::closed_world_temp::pd_state_proven_temp(
                op.temp_state.as_ref(),
                &routine.id,
                &ctx.closed_world_temp_params,
            ) {
                skipped_temp_record += 1;
                continue;
            }

            let loop_node = routine.loops.iter().find(|l| l.id == loop_id);

            let mut path: Vec<EvidenceStep> = Vec::new();
            if let Some(l) = loop_node {
                path.push(EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: Some(l.id.clone()),
                    source_anchor: anchor_of(&l.source_anchor, routine),
                    note: format!("{} loop iterating {}", l.loop_type, op.record_variable_name),
                });
            }
            path.push(EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine),
                note: format!("{} on iterating record {}", op.op, op.record_variable_name),
            });

            let id = format!("d10/{}/{}", routine.id, op.id);
            let root_cause_key = id.clone();

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables: Vec<String> = match &op.table_id {
                Some(t) => vec![t.clone()],
                None => Vec::new(),
            };

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} runs {} on the iterating record {} inside its own loop — \
                 the cursor's snapshot may be corrupted.",
                routine.name, op.op, op.record_variable_name
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Self-modifying loop".into(),
                root_cause,
                severity: "high".to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description: "Collect the keys first, then iterate a fresh recordset to \
                                  perform the modifications; or use ModifyAll with a filter."
                        .into(),
                    safety: "medium".into(),
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
    stats.add_skip("tempRecord", skipped_temp_record);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
