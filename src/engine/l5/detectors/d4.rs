//! D4 — repeated identical lookup inside a loop. Port of al-sem
//! `src/detectors/d4-repeated-lookup-in-loop.ts`.
//!
//! Detects `Get` / `FindFirst` / `FindLast` called 2+ times inside the SAME loop
//! with the same STRING-LIKE LITERAL key argument on the same record variable.
//! v1 only matches string-literal arguments (`'...'` / `"..."`) — the
//! conservative correct case (the key is known at compile time → trivially
//! hoistable). The dedup key uses the unquoted `.value` (falling back to `.text`)
//! so `'X'` and `"X"` group together.
//!
//! Lookups on a provably `temporary` record (`temp_state` Known(true)) are
//! skipped — in-memory, no SQL round-trip to hoist (same gate as d1/d3/d33).
//! When the same (routine, loop, variable) has MULTIPLE distinct repeated
//! literal keys, each finding id gets the key appended (BUG-5); single-key
//! groups keep the plain `d4/{routine}/{loop}/{varLower}` id.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, is_known_temp, op_targets_virtual_system_table};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PExpressionInfo;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d4-repeated-lookup-in-loop";

/// `isStringLikeLiteral` (`model/expression.ts`): a `string_literal` or
/// `quoted_identifier`.
fn is_string_like_literal(info: &PExpressionInfo) -> bool {
    info.kind == "string_literal" || info.kind == "quoted_identifier"
}

pub fn detect_d4(resolved: &Model, ctx: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
    const LOOKUP_OPS: [&str; 3] = ["Get", "FindFirst", "FindLast"];

    let ws = &resolved.workspace;
    // The fingerprint index (routine-by-id + object-by-id) over INTERNAL ids —
    // the fingerprint hashes the internal rootCauseKey + affectedTables.
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
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
        let findings_before = findings.len();

        for loop_info in &routine.loops {
            // Collect lookup ops inside THIS loop with a string-like literal key.
            // candidates: (op-index, key) so we keep op order + group later.
            let mut candidates: Vec<(usize, String)> = Vec::new();
            for (op_idx, op) in routine.record_operations.iter().enumerate() {
                if !LOOKUP_OPS.contains(&op.op.as_str()) {
                    continue;
                }
                if !op.loop_stack.iter().any(|l| l == &loop_info.id) {
                    continue;
                }
                // G-6: a lookup on a BC virtual/system table is an in-memory
                // metadata read — no SQL round-trip to hoist (docs/engine-gaps.md
                // G-6, same gate as d1).
                if op_targets_virtual_system_table(op, routine, &ctx.table_by_id) {
                    continue;
                }
                // Temp gate (detector-audit class A): a lookup on a provably
                // `temporary` record (temp_state Known(true)) is in-memory —
                // no SQL round-trip to hoist (same gate as d1/d3/d33).
                // Physical/Unknown keep firing (suppression-direction safe).
                if is_known_temp(op) {
                    skipped_temp_record += 1;
                    continue;
                }
                let key_info = op.field_argument_infos.as_ref().and_then(|v| v.first());
                let Some(key_info) = key_info else {
                    continue;
                };
                if !is_string_like_literal(key_info) {
                    continue;
                }
                let key = key_info
                    .value
                    .clone()
                    .unwrap_or_else(|| key_info.text.clone());
                candidates.push((op_idx, key));
            }

            if candidates.len() < 2 {
                continue;
            }

            // Group by (recordVariableName.toLowerCase, key), preserving first-seen
            // group order (al-sem `Map` insertion order). The group VALUE keeps the
            // literal key so the id can disambiguate multi-key collisions (BUG-5).
            let mut group_order: Vec<String> = Vec::new();
            let mut groups: std::collections::HashMap<String, (String, Vec<usize>)> =
                std::collections::HashMap::new();
            for (op_idx, key) in &candidates {
                let op = &routine.record_operations[*op_idx];
                let group_key = format!("{}|{}", op.record_variable_name.to_lowercase(), key);
                let entry = groups.entry(group_key.clone()).or_insert_with(|| {
                    group_order.push(group_key.clone());
                    (key.clone(), Vec::new())
                });
                entry.1.push(*op_idx);
            }

            // BUG-5 (docs/detector-audit.md): the id `d4/{routine}/{loop}/{varLower}`
            // omits the literal key, so TWO distinct keys each repeated 2+ times on
            // the same variable collide. Count the QUALIFYING (2+ ops) key groups
            // per variable; only when a variable has multiple does the id get the
            // key appended — single-key groups keep the pre-fix id (goldens stable).
            let mut qualifying_keys_per_var: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for group_key in &group_order {
                let (_, op_idxs) = &groups[group_key];
                if op_idxs.len() < 2 {
                    continue;
                }
                let var = routine.record_operations[op_idxs[0]]
                    .record_variable_name
                    .to_lowercase();
                *qualifying_keys_per_var.entry(var).or_insert(0) += 1;
            }

            for group_key in &group_order {
                let (literal_key, op_idxs) = &groups[group_key];
                if op_idxs.len() < 2 {
                    continue;
                }
                let first = &routine.record_operations[op_idxs[0]];
                let first_rec_var_lower = first.record_variable_name.to_lowercase();

                let multi_key = qualifying_keys_per_var
                    .get(&first_rec_var_lower)
                    .copied()
                    .unwrap_or(0)
                    > 1;
                let id = if multi_key {
                    format!(
                        "d4/{}/{}/{}/{}",
                        routine.id, loop_info.id, first_rec_var_lower, literal_key
                    )
                } else {
                    format!("d4/{}/{}/{}", routine.id, loop_info.id, first_rec_var_lower)
                };
                let root_cause_key = id.clone();

                // Evidence path: the loop step, then one step per op.
                let mut path: Vec<EvidenceStep> = Vec::with_capacity(op_idxs.len() + 1);
                path.push(EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: Some(loop_info.id.clone()),
                    source_anchor: anchor_of(&loop_info.source_anchor, routine),
                    note: format!("{} loop", loop_info.loop_type),
                });
                for op_idx in op_idxs {
                    let o = &routine.record_operations[*op_idx];
                    path.push(EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(o.id.clone()),
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&o.source_anchor, routine),
                        note: format!("{} on {} with literal key", o.op, o.record_variable_name),
                    });
                }

                let affected_objects = vec![routine.object_id.clone()];
                let affected_tables: Vec<String> = match &first.table_id {
                    Some(t) => vec![t.clone()],
                    None => Vec::new(),
                };

                let confidence: FindingConfidence = to_confidence(&[], "likely");

                let root_cause = format!(
                    "{} calls {} on {} {} times inside a loop with the same literal key \
                     — cache the result once before the loop.",
                    routine.name,
                    first.op,
                    first.record_variable_name,
                    op_idxs.len()
                );

                let mut finding = Finding {
                    id,
                    root_cause_key,
                    detector: DETECTOR.to_string(),
                    title: "Repeated identical lookup inside a loop".into(),
                    root_cause,
                    severity: "medium".to_string(),
                    confidence,
                    primary_location: anchor_of(&first.source_anchor, routine),
                    evidence_path: path,
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: id_list(affected_tables),
                    fix_options: vec![FixOption {
                        description:
                            "Move the lookup out of the loop into a local variable, then read \
                             fields from that variable inside the loop."
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
        // skippedOther: routines where no findings were emitted.
        if findings.len() == findings_before {
            skipped_other += 1;
        }
    }

    // Within-detector sort by compareStrings(a.id, b.id) (byte order).
    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    stats.add_skip("tempRecord", skipped_temp_record);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
