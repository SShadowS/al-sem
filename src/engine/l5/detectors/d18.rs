//! D18 — constant filter in loop. Port of al-sem
//! `src/detectors/d18-constant-filter-in-loop.ts`.
//!
//! Flags `SetRange` / `SetFilter` inside a loop whose every argument AFTER the
//! field name is a literal. The same filter is applied every iteration with no
//! dependency on the iterating variable; the call can be hoisted outside.
//!
//! Skipped:
//!  - temporary records (`tempState: { kind: "known", value: true }`);
//!  - any non-literal value argument.
//!
//! Dedup key: (routineId, loopId, recordVar, fieldName).
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, unquoted_field_name};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PExpressionInfo;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d18-constant-filter-in-loop";

const FILTER_OPS: &[&str] = &["SetRange", "SetFilter"];

/// `isLiteralExpression` from `model/expression.ts`:
/// A loop-invariant literal — string, number, boolean, or qualified enum value.
/// Unary +/- over a numeric literal is also literal (value is set on unary_expression).
fn is_literal_expression(info: &PExpressionInfo) -> bool {
    match info.kind.as_str() {
        "string_literal"
        | "quoted_identifier"
        | "integer"
        | "decimal"
        | "boolean"
        | "qualified_enum_value" => true,
        "unary_expression" => info.value.is_some(),
        _ => false,
    }
}

pub fn detect_d18(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut candidates_considered = 0usize;
    let mut skipped_non_literal = 0u64;
    let mut skipped_temp_record = 0u64;

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

        // Build loopId → PLoop map.
        let loop_by_id: std::collections::HashMap<&str, _> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        for op in &routine.record_operations {
            if !FILTER_OPS.contains(&op.op.as_str()) {
                continue;
            }
            if op.loop_stack.is_empty() {
                continue;
            }

            // Skip temporary records.
            if super::is_known_temp(op) {
                skipped_temp_record += 1;
                continue;
            }

            let infos = match &op.field_argument_infos {
                Some(v) => v,
                None => continue,
            };
            if infos.len() < 2 {
                continue; // unrenderable — bail
            }

            // Value arguments are everything after the field name (index 1+).
            let value_args = &infos[1..];
            if !value_args.iter().all(is_literal_expression) {
                skipped_non_literal += 1;
                continue;
            }

            // The "representative" loop is the innermost (last) loop in the stack.
            let representative_loop_id = match op.loop_stack.last() {
                Some(id) => id.as_str(),
                None => continue,
            };
            let loop_info = match loop_by_id.get(representative_loop_id) {
                Some(l) => *l,
                None => continue,
            };

            let record_var = op.record_variable_name.to_lowercase();
            let field_info = match infos.first() {
                Some(f) => f,
                None => continue,
            };
            let field_name = field_info.text.trim().to_string();

            let dedup_key = format!(
                "{}|{}|{}|{}",
                routine.id,
                loop_info.id,
                record_var,
                unquoted_field_name(field_info).to_lowercase()
            );
            if seen.contains(&dedup_key) {
                continue;
            }
            seen.insert(dedup_key);

            // Evidence path: loop step, then op step.
            let path = vec![
                EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: Some(loop_info.id.clone()),
                    source_anchor: anchor_of(&loop_info.source_anchor, routine),
                    note: format!("{} loop", loop_info.loop_type),
                },
                EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: Some(op.id.clone()),
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&op.source_anchor, routine),
                    note: format!(
                        "{}({}) on {}",
                        op.op,
                        op.field_arguments
                            .as_ref()
                            .map(|args| args.join(", "))
                            .unwrap_or_default(),
                        op.record_variable_name
                    ),
                },
            ];

            // id = d18/{routineId}/{loopId}/{recordVar}/{fieldName.lower}
            // (mirrors al-sem: `d18/${routine.id}/${loop.id}/${recordVar}/${fieldName.toLowerCase()}`)
            let id = format!(
                "d18/{}/{}/{}/{}",
                routine.id,
                loop_info.id,
                op.record_variable_name.to_lowercase(),
                field_name.to_lowercase()
            );
            let root_cause_key = id.clone();

            let affected_objects = vec![routine.object_id.clone()];
            let affected_tables: Vec<String> = match &op.table_id {
                Some(t) => vec![t.clone()],
                None => Vec::new(),
            };

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} calls {} on {}.{} with literal arguments inside a {} loop — \
                 the filter is identical every iteration and can be hoisted outside.",
                routine.name, op.op, op.record_variable_name, field_name, loop_info.loop_type
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Constant filter applied inside a loop".into(),
                root_cause,
                severity: "low".to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description:
                        "Move the SetRange/SetFilter call outside the loop. The filter state \
                         persists across iterations until reset or cleared."
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
    stats.add_skip("nonLiteralArgs", skipped_non_literal);
    stats.add_skip("tempRecord", skipped_temp_record);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
