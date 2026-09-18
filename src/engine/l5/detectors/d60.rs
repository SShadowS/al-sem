//! D60 — repeat…Modify…until loop in an Upgrade/Install codeunit. BCQuality
//! `datatransfer-for-bulk-init`: upgrade code rewriting rows one-by-one should
//! use DataTransfer (set-based SQL, no per-row trigger cost) — on large tables
//! the difference is hours vs seconds.
//!
//! Join: object_subtype ∈ {Upgrade, Install} (Codeunit), a Modify record op with
//! non-empty loop_stack whose receiver is a live cursor (FindSet/Find/FindFirst/
//! Next on the same var in the routine), AND a DataTransfer-shaped loop body: NO
//! per-row call, NO op on another record var, NO if/case computing the value.
//! A body that does any of those legitimately needs the row-by-row loop and is
//! not a DataTransfer candidate — inspecting the body (rather than firing on
//! every upgrade repeat…Modify) is what keeps this precise on real upgrade code.
//! One finding per (routine, loop, var) — first op wins. Severity: medium.
//! Confidence: likely.

use std::collections::{HashMap, HashSet};

use crate::engine::l2::features::{PAnchor, PCFNNode, PLoop};
use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Resolved};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::detectors::{advance_discipline_holds, whole_set_break};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d60-upgrade-loop-should-be-datatransfer";

const CURSOR_OPS: &[&str] = &["FindSet", "Find", "FindFirst", "Next"];

/// A CFG node's `source_range` sits fully inside `outer` (same 0-based/utf16
/// basis as PAnchor).
fn range_within(r: (u32, u32, u32, u32), outer: &PAnchor) -> bool {
    let (sl, sc, el, ec) = r;
    let starts_ok = outer.start_line < sl || (outer.start_line == sl && outer.start_column <= sc);
    let ends_ok = el < outer.end_line || (el == outer.end_line && ec <= outer.end_column);
    starts_ok && ends_ok
}

/// True if the statement tree contains an `if`/`case` branch node whose source
/// range is within `loop_anchor` — i.e. the loop body branches. Structural, so
/// it catches conditions of ANY shape (parenthesized, quoted-field scrutinee)
/// that the identifier-only `condition_references` collection misses. Recurses
/// every child group; the loop's own enclosing `if Rec.FindSet()` guard is NOT
/// within the loop and so never matches.
fn tree_has_branch_within(node: &PCFNNode, loop_anchor: &PAnchor) -> bool {
    if (node.kind == "if" || node.kind == "case")
        && node
            .source_range
            .is_some_and(|r| range_within(r, loop_anchor))
    {
        return true;
    }
    [&node.children, &node.else_children, &node.condition_leaves]
        .into_iter()
        .flatten()
        .flatten()
        .any(|k| tree_has_branch_within(k, loop_anchor))
}

pub fn detect_d60(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_not_cursor = 0u64;
    let mut skipped_body_has_call = 0u64;
    let mut skipped_body_other_record = 0u64;
    let mut skipped_body_conditional = 0u64;
    let mut skipped_traversal = 0u64;

    let lifecycle_objects: HashSet<&str> = ws
        .objects
        .iter()
        .filter(|o| {
            o.object_type == "Codeunit"
                && o.object_subtype.as_deref().is_some_and(|s| {
                    s.eq_ignore_ascii_case("Upgrade") || s.eq_ignore_ascii_case("Install")
                })
        })
        .map(|o| o.id.as_str())
        .collect();
    if lifecycle_objects.is_empty() {
        let stats = DetectorStats::new(DETECTOR, 0, 0);
        return Ok(DetectorOutput::no_diag(findings, stats));
    }

    for routine in &ws.routines {
        if !lifecycle_objects.contains(routine.object_id.as_str()) {
            continue;
        }
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        let loop_by_id: HashMap<&str, &PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();
        let cursor_vars: HashSet<String> = routine
            .record_operations
            .iter()
            .filter(|op| CURSOR_OPS.contains(&op.op.as_str()))
            .map(|op| op.record_variable_name.to_lowercase())
            .collect();

        // One finding per (loop, var): first Modify wins.
        let mut reported: HashSet<(String, String)> = HashSet::new();
        for op in &routine.record_operations {
            if op.op != "Modify" {
                continue;
            }
            let Some(rep_loop_id) = op.loop_stack.last() else {
                continue;
            };
            candidates_considered += 1;
            let var_lc = op.record_variable_name.to_lowercase();
            if !cursor_vars.contains(&var_lc) {
                skipped_not_cursor += 1;
                continue;
            }
            if !reported.insert((rep_loop_id.clone(), var_lc.clone())) {
                continue;
            }
            let Some(loop_info) = loop_by_id.get(rep_loop_id.as_str()) else {
                continue;
            };

            // DataTransfer can only express a set-based constant / same-record
            // copy init. A loop body that does per-row WORK — calls a routine,
            // touches ANOTHER record, or computes the value under an if/case —
            // legitimately needs the row-by-row loop and is NOT a DataTransfer
            // candidate. Inspecting the body (rather than firing on every
            // upgrade repeat…Modify) is what keeps this precise on real upgrade
            // code, where such bodies are the norm.
            let loop_id = loop_info.id.as_str();
            if routine
                .call_sites
                .iter()
                .any(|cs| cs.loop_stack.iter().any(|id| id == loop_id))
            {
                skipped_body_has_call += 1;
                continue;
            }
            if routine.record_operations.iter().any(|o| {
                o.loop_stack.iter().any(|id| id == loop_id)
                    && o.record_variable_name.to_lowercase() != var_lc
            }) {
                skipped_body_other_record += 1;
                continue;
            }
            if routine
                .statement_tree
                .as_ref()
                .is_some_and(|t| tree_has_branch_within(t, &loop_info.source_anchor))
            {
                skipped_body_conditional += 1;
                continue;
            }

            // issue #21: the three gates above say nothing about ops on the
            // DRIVER variable itself — the guard above is `!= var_lc` — so an
            // in-loop `SetRange`/`SetCurrentKey`, or a `Next(2)`, or an extra
            // body advance all reached here untouched and were reported as
            // DataTransfer candidates. Pinned by `tests/r0-corpus/ws-d60`:
            // remove this pass and all SIX unsound shapes there return, while
            // OnUpgradePerCompany and UpgradeUnitStepAdvance stay reported.
            let driver_ops_in_loop: Vec<&L3RecordOperation> = routine
                .record_operations
                .iter()
                .filter(|o| {
                    o.loop_stack.iter().any(|id| id == loop_id)
                        && o.record_variable_name.to_lowercase() == var_lc
                })
                .collect();
            if !advance_discipline_holds(&driver_ops_in_loop, &var_lc, loop_id) {
                skipped_traversal += 1;
                continue;
            }
            let traversal_broken = driver_ops_in_loop
                .iter()
                .any(|o| o.id != op.id && whole_set_break(o, loop_id).is_some());
            if traversal_broken {
                skipped_traversal += 1;
                continue;
            }

            let table_name = op
                .table_id
                .as_deref()
                .and_then(|tid| ctx.table_by_id.get(tid).map(|t| t.name.clone()))
                .unwrap_or_else(|| op.record_variable_name.clone());

            let confidence: FindingConfidence = to_confidence(&[], "likely");
            let mut finding = Finding {
                id: format!("d60/{}/{}/{}", routine.id, loop_info.id, op.id),
                root_cause_key: format!("d60/{}/{}/{}", routine.id, loop_info.id, var_lc),
                detector: DETECTOR.to_string(),
                title: "Row-by-row upgrade loop (use DataTransfer)".into(),
                root_cause: format!(
                    "{} (upgrade/install codeunit) rewrites {} row-by-row in a {} loop — \
                     DataTransfer performs the same bulk init/copy set-based, without \
                     per-row trigger cost.",
                    routine.name, table_name, loop_info.loop_type
                ),
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&op.source_anchor, routine),
                evidence_path: vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: Some(loop_info.id.clone()),
                        source_anchor: anchor_of(&loop_info.source_anchor, routine),
                        note: format!("{} loop over {}", loop_info.loop_type, table_name),
                    },
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(op.id.clone()),
                        callsite_id: None,
                        loop_id: Some(loop_info.id.clone()),
                        source_anchor: anchor_of(&op.source_anchor, routine),
                        note: "per-row Modify".to_string(),
                    },
                ],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: id_list(op.table_id.iter().cloned()),
                fix_options: vec![FixOption {
                    description: "Replace the loop with a DataTransfer (SourceTable/\
                                  DestinationTable + CopyFields/ConstantValue), or ModifyAll \
                                  when a single field gets a constant."
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
    stats.add_skip("notCursorVar", skipped_not_cursor);
    stats.add_skip("bodyHasCall", skipped_body_has_call);
    stats.add_skip("bodyOtherRecordOp", skipped_body_other_record);
    stats.add_skip("bodyConditional", skipped_body_conditional);
    stats.add_skip("traversal", skipped_traversal);
    Ok(DetectorOutput::no_diag(findings, stats))
}
