//! D5 — loop-and-Modify could be ModifyAll. Port of al-sem
//! `src/detectors/d5-set-based-opportunity.ts`.
//!
//! Detects a repeat/until loop whose only DB write is exactly one `Modify` on
//! the iterating record variable (identified via `Next()` inside the loop), no
//! callSites inside the loop, and all other ops are filter/load-state setters.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l2::features::PLoop;
use crate::engine::l3::l3_workspace::{L3Resolved, L3Routine};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::detectors::{advance_discipline_holds, whole_set_break};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d5-set-based-opportunity";

/// Ops that are allowed alongside a Modify without disqualifying the pattern.
const ALLOWED_OTHER_OPS: &[&str] = &[
    "SetRange",
    "SetFilter",
    "SetLoadFields",
    "AddLoadFields",
    "SetCurrentKey",
    "Next",
];

fn anchor_of_loop(l: &PLoop, routine: &L3Routine) -> crate::engine::l5::finding::SourceAnchor {
    // Delegate to the shared helper — PLoop.source_anchor is a PAnchor.
    anchor_of(&l.source_anchor, routine)
}

pub fn detect_d5(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
    let mut skipped_traversal = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        candidates_considered += 1;
        let findings_before = findings.len();

        // Map loopId → record variable that drives the loop (identified via Next()).
        // Next() is always emitted inside the loop body (loopStack non-empty);
        // FindSet/FindFirst appear in the `if` guard before `repeat` (loopStack=[]).
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

        for loop_info in &routine.loops {
            let driver = match loop_driver.get(&loop_info.id) {
                Some(d) => d.clone(),
                None => continue,
            };

            // All record ops inside this loop.
            let ops_in_loop: Vec<_> = routine
                .record_operations
                .iter()
                .filter(|op| op.loop_stack.contains(&loop_info.id))
                .collect();

            // No callSites inside the loop — interprocedural calls could hide DB ops.
            let callsites_in_loop = routine
                .call_sites
                .iter()
                .any(|cs| cs.loop_stack.contains(&loop_info.id));
            if callsites_in_loop {
                continue;
            }

            // Must have exactly one Modify on the iterating record variable inside the loop.
            let modify_ops: Vec<_> = ops_in_loop
                .iter()
                .filter(|op| op.op == "Modify" && op.record_variable_name.to_lowercase() == driver)
                .collect();
            if modify_ops.len() != 1 {
                continue;
            }
            let modify = modify_ops[0];

            // All other ops must be filter/load-state setters or Next.
            //
            // This gate is by op NAME and stays exactly as it was: it is what
            // rejects `Delete`/`Insert`/`Validate`/`Get`, and its scope is ALL
            // ops in the loop, not only the driver's.
            let all_allowed = ops_in_loop
                .iter()
                .filter(|op| op.id != modify.id)
                .all(|op| ALLOWED_OTHER_OPS.contains(&op.op.as_str()));
            if !all_allowed {
                continue;
            }

            // issue #21: a name is not enough. `Next(1)` and `Next(2)` share
            // one, and a `SetRange` on a literal differs from one on the
            // current row — so the ops that PASSED the list above still have
            // to be judged by what they do to the traversal. These checks only
            // ever remove candidates.
            //
            // Scoped to the DRIVER's ops: a filter on some other record
            // variable does not change this loop's selected set.
            if !advance_discipline_holds(&ops_in_loop, &driver, &loop_info.id) {
                skipped_traversal += 1;
                continue;
            }
            let traversal_broken = ops_in_loop.iter().any(|op| {
                op.id != modify.id
                    && op.record_variable_name.to_lowercase() == driver
                    && whole_set_break(op, &loop_info.id).is_some()
            });
            if traversal_broken {
                skipped_traversal += 1;
                continue;
            }

            // Find the associated retrieval op for this record variable OUTSIDE the loop
            // (FindSet/FindFirst/FindLast/Find — they live before the repeat with loopStack=[]).
            let retrieval_op = routine.record_operations.iter().find(|op| {
                (op.op == "FindSet"
                    || op.op == "FindFirst"
                    || op.op == "FindLast"
                    || op.op == "Find")
                    && op.record_variable_name.to_lowercase() == driver
                    && op.loop_stack.is_empty()
            });

            // Display name: first op's original casing for this record var.
            let record_var_display = routine
                .record_operations
                .iter()
                .find(|op| op.record_variable_name.to_lowercase() == driver)
                .map(|op| op.record_variable_name.clone())
                .unwrap_or_else(|| driver.clone());

            let id = format!("d5/{}/{}", routine.id, loop_info.id);
            let root_cause_key = id.clone();

            let mut path: Vec<EvidenceStep> = Vec::new();
            if let Some(ret) = retrieval_op {
                path.push(EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: Some(ret.id.clone()),
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&ret.source_anchor, routine),
                    note: format!("{} on {} — loop entry", ret.op, record_var_display),
                });
            }
            path.push(EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: Some(loop_info.id.clone()),
                source_anchor: anchor_of_loop(loop_info, routine),
                note: format!("{} loop on {}", loop_info.loop_type, record_var_display),
            });
            path.push(EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(modify.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&modify.source_anchor, routine),
                note: format!("Modify on {} — consider ModifyAll", record_var_display),
            });

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables: Vec<String> = match &modify.table_id {
                Some(t) => vec![t.clone()],
                None => Vec::new(),
            };

            let confidence: FindingConfidence = to_confidence(&[], "possible");

            let root_cause = format!(
                "{} loops over {} and Modifies each row with no conditional branches or \
                 inter-record DB calls — ModifyAll on the same filter would issue one SQL statement.",
                routine.name, record_var_display
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Loop-and-Modify could be ModifyAll".into(),
                root_cause,
                severity: "info".to_string(),
                confidence,
                primary_location: anchor_of(&modify.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description:
                        "Replace the FindSet+repeat+Modify pattern with ModifyAll on the same filter."
                            .to_string().into(),
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
        if findings.len() == findings_before {
            skipped_other += 1;
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    stats.add_skip("traversal", skipped_traversal);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
