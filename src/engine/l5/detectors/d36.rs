//! D36 — SetLoadFields placed after the load. Port of al-sem
//! `src/detectors/d36-late-setloadfields.ts`.
//!
//! Detects `SetLoadFields` / `AddLoadFields` placed AFTER a load
//! (`Get`/`Find*`/`Next`) with no later load on the same record variable. The
//! partial-record optimisation doesn't apply to the row that was already loaded.
//!
//! Skipped:
//!  - temporary records (loadfields semantics don't apply);
//!  - by-var parameter records (caller may issue the next load);
//!  - record-vars with no preceding load (D3's domain — too-early is not D36).
//!  - record-vars WITH a later load (the SetLoadFields prepares for the next load).
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d36-late-setloadfields";

const LOAD_OPS: &[&str] = &["Get", "FindFirst", "FindLast", "FindSet", "Find", "Next"];

const LOAD_FIELDS_OPS: &[&str] = &["SetLoadFields", "AddLoadFields"];

pub fn detect_d36(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_has_later_load = 0u64;
    let mut skipped_no_prior_load = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_parameter = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }

        // Collect by-var parameter record variable names (lowercased).
        let param_record_names: std::collections::HashSet<String> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| rv.name.to_lowercase())
            .collect();

        for op in &routine.record_operations {
            if !LOAD_FIELDS_OPS.contains(&op.op.as_str()) {
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

            // Must have at least one prior load.
            let has_prior_load = routine.record_operations.iter().any(|other| {
                LOAD_OPS.contains(&other.op.as_str())
                    && other.record_variable_name.to_lowercase() == var_key
                    && before_anchor(&other.source_anchor, &op.source_anchor)
            });
            if !has_prior_load {
                skipped_no_prior_load += 1;
                continue;
            }

            // If there IS a later load, the SetLoadFields is "preparing" for the next
            // iteration — skip.
            let has_later_load = routine.record_operations.iter().any(|other| {
                LOAD_OPS.contains(&other.op.as_str())
                    && other.record_variable_name.to_lowercase() == var_key
                    && before_anchor(&op.source_anchor, &other.source_anchor)
            });
            if has_later_load {
                skipped_has_later_load += 1;
                continue;
            }

            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine),
                note: format!(
                    "{} on {} after the record was already loaded — the call has no effect",
                    op.op, op.record_variable_name
                ),
            }];

            let id = format!("d36/{}/{}", routine.id, op.id);
            let root_cause_key = id.clone();

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables: Vec<String> = match &op.table_id {
                Some(t) => vec![t.clone()],
                None => Vec::new(),
            };

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} calls {} on {} after the record was already loaded and never loads it again — \
                 the partial-record optimisation cannot apply.",
                routine.name, op.op, op.record_variable_name
            );

            let fix_description = format!(
                "Move the {} call to BEFORE the preceding Get / Find on {}, so the loader can \
                 fetch only the listed fields.",
                op.op, op.record_variable_name
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "SetLoadFields placed after the load".into(),
                root_cause,
                severity: "low".to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description: fix_description.into(),
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
    stats.add_skip("hasLaterLoad", skipped_has_later_load);
    stats.add_skip("noPriorLoad", skipped_no_prior_load);
    stats.add_skip("tempRecord", skipped_temp_record);
    stats.add_skip("parameter", skipped_parameter);
    Ok(DetectorOutput::no_diag(findings, stats))
}
