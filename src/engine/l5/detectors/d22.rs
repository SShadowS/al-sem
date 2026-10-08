//! D22 — FlowField read without prior CalcFields. Port of al-sem
//! `src/detectors/d22-flowfield-without-calcfields.ts`.
//!
//! Flags `FieldAccess` where:
//!  1. The record variable resolves to a known table.
//!  2. The accessed field is a `FlowField`.
//!  3. No `CalcFields` op on the same variable, strictly BEFORE the access in
//!     source order, lists the field name in its `fieldArgumentInfos`.
//!
//! Skipped:
//!  - by-var parameter records (caller may have already called CalcFields).
//!  - record variables whose tableId did not resolve.
//!  - tables that are not in the workspace (tableById miss).
//!  - fields not found in the table model.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor, unquoted_field_name};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d22-flowfield-without-calcfields";

pub fn detect_d22(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_unresolved_table = 0u64;
    let mut skipped_parameter = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" — source-only, every routine is primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        candidates_considered += 1;

        // Build recordVarByNameLc and paramRecordNames.
        let record_var_by_name_lc: std::collections::HashMap<String, _> = routine
            .record_variables
            .iter()
            .map(|rv| (rv.name.to_lowercase(), rv))
            .collect();

        let param_record_names: std::collections::HashSet<String> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| rv.name.to_lowercase())
            .collect();

        for fa in &routine.field_accesses {
            let record_var_key = fa.record_variable_name.to_lowercase();

            // Skip by-var parameter records.
            if param_record_names.contains(&record_var_key) {
                skipped_parameter += 1;
                continue;
            }

            // Resolve the record variable to its table id.
            let table_id = match record_var_by_name_lc
                .get(&record_var_key)
                .and_then(|rv| rv.table_id.as_deref())
            {
                Some(id) => id,
                None => {
                    skipped_unresolved_table += 1;
                    continue;
                }
            };

            // Look up the table in the context index.
            let table = match ctx.table_by_id.get(table_id) {
                Some(t) => *t,
                None => {
                    skipped_unresolved_table += 1;
                    continue;
                }
            };

            // Find the field in the table (case-insensitive).
            let field_name_lc = fa.field_name.to_lowercase();
            let field = match table
                .fields
                .iter()
                .find(|f| f.name.to_lowercase() == field_name_lc)
            {
                Some(f) => f,
                None => continue,
            };

            // Only FlowFields are of interest.
            if field.field_class != "FlowField" {
                continue;
            }

            // Check if this access is covered by a CalcFields op earlier in the routine.
            if is_covered(
                &routine.record_operations,
                &record_var_key,
                &field_name_lc,
                &fa.source_anchor,
            ) {
                continue;
            }

            // Emit a finding.
            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&fa.source_anchor, routine),
                note: format!(
                    "reads FlowField {}.{} without a prior CalcFields({}) on {}",
                    table.name, field.name, field.name, fa.record_variable_name
                ),
            }];

            // id = d22/{routineId}/{recVarLower}/{fieldLower}/{startLine}:{startColumn}
            let id = format!(
                "d22/{}/{}/{}/{}:{}",
                routine.id,
                record_var_key,
                field_name_lc,
                fa.source_anchor.start_line,
                fa.source_anchor.start_column
            );
            // rootCauseKey does NOT have the line:col suffix
            let root_cause_key = format!("d22/{}/{}/{}", routine.id, record_var_key, field_name_lc);

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables = vec![table.id.clone()];

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} reads {}.{} (a FlowField) but never called CalcFields({}) on {} \
                 — the read returns the AL default (0 / empty), not the live value.",
                routine.name, table.name, field.name, field.name, fa.record_variable_name
            );

            let fix_desc = format!(
                "Call `{}.CalcFields({});` before reading the field. \
                 Hoist the CalcFields out of any tight loop to avoid an N+1.",
                fa.record_variable_name, field.name
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "FlowField read without prior CalcFields".into(),
                root_cause,
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&fa.source_anchor, routine),
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
    stats.add_skip("unresolvedTable", skipped_unresolved_table);
    stats.add_skip("parameter", skipped_parameter);
    Ok(DetectorOutput::no_diag(findings, stats))
}

/// Returns true if there is a `CalcFields` op on `record_var_key` strictly
/// BEFORE `access_anchor` that lists `field_name_lc` in its fieldArgumentInfos.
fn is_covered(
    ops: &[crate::program::model::workspace::ModelRecordOperation],
    record_var_key: &str,
    field_name_lc: &str,
    access_anchor: &crate::program::body::features::PAnchor,
) -> bool {
    for op in ops {
        if op.op != "CalcFields" {
            continue;
        }
        if op.record_variable_name.to_lowercase() != record_var_key {
            continue;
        }
        if !before_anchor(&op.source_anchor, access_anchor) {
            continue;
        }
        let infos = match &op.field_argument_infos {
            Some(v) => v,
            None => continue,
        };
        if infos
            .iter()
            .any(|info| unquoted_field_name(info).to_lowercase() == field_name_lc)
        {
            return true;
        }
    }
    false
}
