//! D56 — Record cloned before Modify/Delete inside a loop. BCQuality
//! `avoid-cloning-records-before-modify-delete-in-loops`: `Copy := Cursor;
//! Copy.Modify();` per iteration re-reads/re-writes the row the cursor already
//! holds — an extra SQL round-trip per row. Modify the cursor directly (or use
//! ModifyAll / a temp buffer).
//!
//! Join (all intraprocedural, exact):
//!  - a whole-record copy `lhs := rhs` (PVarAssignment.rhs_identifier — set ONLY
//!    for bare-identifier-to-bare-identifier copies) between two RECORD vars,
//!  - the assignment sits inside a loop (innermost containing PLoop by anchor),
//!  - a Modify/Delete on the CLONE, in the SAME loop (op.loop_stack), AFTER the copy,
//!  - the SOURCE is a live cursor (has FindSet/Find/FindFirst/Next ops in the routine),
//!  - the SOURCE is NOT a `temporary` record. A temp source is an in-memory buffer
//!    being MATERIALIZED into a persisted target (`Persisted := Temp; Persisted.
//!    Insert()/Modify()`) — a genuinely different row, not the redundant re-write
//!    the premise describes; the `Copy := Cursor` struct copy is itself SQL-free.
//!
//! Key-field-reassignment skip (closes the prior opt-in residual): a
//! PERSISTED-source clone is NOT flagged when, between the clone assignment and
//! the write, a field WRITE on the clone targets a KEY field — the target
//! table's PRIMARY KEY (`ModelTable.keys` first entry) or a field named in a
//! `SetCurrentKey` call on the SOURCE cursor in the SAME routine (the current
//! key, which need not be the PK). Such a write retargets the clone at a
//! DIFFERENT physical row, so the clone is functionally required — the
//! real-world case: Continia's MoveEmailLog (`EmailLog2 := EmailLog;
//! EmailLog2."Record ID" := ...; EmailLog2.Modify()` inside a loop where
//! "Record ID" is the SetCurrentKey field, not the table's declared PK). The
//! write signal is derived structurally (never a raw field read): a
//! `field_accesses` entry counts as a write only when its exact
//! `(start_line, start_column, field_name)` matches a recorded
//! `var_assignments` LHS — the same G-15(a) proof d3 uses (a plain read of a
//! key field, e.g. in a condition, never matches an assignment LHS position
//! and so never triggers the skip). This closes the residual that kept d56
//! OPT-IN; it now ships DEFAULT.
//!
//! Severity: medium. Confidence: likely.

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, anchor_within, before_anchor, is_known_temp_var, normalize_load_field_arg,
    primary_key_field_names_lc,
};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::{PAnchor, PCFNNode};
use crate::program::model::workspace::Model;

pub(crate) const DETECTOR: &str = "d56-clone-before-write-in-loop";

/// A source range `(start_line, start_col, end_line, end_col)` holds `a`'s start.
fn range_holds(r: (u32, u32, u32, u32), a: &PAnchor) -> bool {
    let p = (a.start_line, a.start_column);
    (r.0, r.1) <= p && p <= (r.2, r.3)
}

/// ENG-27: every `if` / `case` around `a` in the statement tree, with the
/// branch that holds it (`if`: 0 then, 1 else; `case`: the branch's index;
/// `None` when no branch holds it, e.g. the condition, or a range is missing).
fn branch_path(
    nodes: &[PCFNNode],
    a: &PAnchor,
    out: &mut Vec<((u32, u32, u32, u32), Option<usize>)>,
) {
    let held = |list: &Option<Vec<PCFNNode>>| {
        list.iter()
            .flatten()
            .any(|n| n.source_range.is_some_and(|r| range_holds(r, a)))
    };
    for n in nodes {
        if (n.kind == "if" || n.kind == "case")
            && let Some(r) = n.source_range
            && range_holds(r, a)
        {
            let branch = if n.kind == "if" {
                if held(&n.children) {
                    Some(0)
                } else if held(&n.else_children) {
                    Some(1)
                } else {
                    None
                }
            } else {
                n.children
                    .iter()
                    .flatten()
                    .position(|b| b.source_range.is_some_and(|br| range_holds(br, a)))
            };
            out.push((r, branch));
        }
        for list in [&n.children, &n.else_children] {
            branch_path(list.as_deref().unwrap_or(&[]), a, out);
        }
    }
}

/// ENG-27: `other` runs on every path that reaches `write` from where both
/// start: each `if` / `case` branch around `other` is a branch around `write`
/// too. Anything less, or unknown, is conditional: then `other` overwrites
/// nothing, and the earlier copy's finding stands.
fn unconditional_before(
    routine: &crate::program::model::workspace::ModelRoutine,
    other: &PAnchor,
    write: &PAnchor,
) -> bool {
    let Some(tree) = routine.statement_tree.as_deref() else {
        return false;
    };
    let (mut o, mut w) = (Vec::new(), Vec::new());
    branch_path(std::slice::from_ref(tree), other, &mut o);
    branch_path(std::slice::from_ref(tree), write, &mut w);
    o.iter()
        .all(|(r, b)| b.is_some() && w.iter().any(|(wr, wb)| wr == r && wb == b))
}

const CURSOR_OPS: &[&str] = &["FindSet", "Find", "FindFirst", "Next"];
const WRITE_BACK_OPS: &[&str] = &["Modify", "Delete"];

pub fn detect_d56(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_no_write_back = 0u64;
    let mut skipped_source_not_cursor = 0u64;
    let mut skipped_source_temp = 0u64;
    let mut skipped_target_temp = 0u64;
    let mut skipped_overwritten = 0u64;
    let mut skipped_key_remapped_clone = 0u64;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        if routine.loops.is_empty() || routine.var_assignments.is_empty() {
            continue;
        }
        let record_names: HashSet<String> = routine
            .record_variables
            .iter()
            .map(|rv| rv.name.to_lowercase())
            .collect();

        // G-15(a) write-target proof (shared with d3): a `field_accesses` entry
        // is a WRITE iff its exact (start_line, start_column, field_name) matches
        // a recorded `var_assignments` LHS — the assignment statement's anchor IS
        // the LHS member expression's start. A plain read of the same field
        // elsewhere in the routine sits at a different position and never matches.
        let write_targets: HashSet<(u32, u32, String)> = routine
            .var_assignments
            .iter()
            .map(|va| {
                (
                    va.source_anchor.start_line,
                    va.source_anchor.start_column,
                    normalize_load_field_arg(&va.lhs_name),
                )
            })
            .collect();

        // Innermost loop containing an assignment (assignments carry no
        // loop_stack; containment is by anchor).
        let innermost = |a: &crate::program::body::features::PAnchor| {
            routine
                .loops
                .iter()
                .filter(|l| anchor_within(a, &l.source_anchor))
                .max_by_key(|l| (l.source_anchor.start_line, l.source_anchor.start_column))
        };
        for asg in &routine.var_assignments {
            let Some(rhs_lc) = asg.rhs_identifier.as_deref() else {
                continue;
            };
            // Whole-record copy between two record vars only.
            if !record_names.contains(rhs_lc) || !record_names.contains(&asg.lhs_name) {
                continue;
            }
            let Some(lp) = innermost(&asg.source_anchor) else {
                continue;
            };
            candidates_considered += 1;

            // Written back inside the SAME loop, after the copy.
            let write = routine.record_operations.iter().find(|op| {
                WRITE_BACK_OPS.contains(&op.op.as_str())
                    && op.record_variable_name.to_lowercase() == asg.lhs_name
                    && op.loop_stack.iter().any(|id| id == &lp.id)
                    && before_anchor(&asg.source_anchor, &op.source_anchor)
            });
            let Some(write) = write else {
                skipped_no_write_back += 1;
                continue;
            };
            // ENG-27: only the LAST whole-record assignment to the clone before
            // the write can be what it writes (in source order; a branch is not
            // followed). An earlier copy is overwritten, so the checks below
            // must judge the later one (`Copy := Cursor; Copy := Temp;
            // Copy.Modify()` writes the temp copy). This also makes it at most
            // one finding per write: two copies before one write gave one id
            // twice. Only a whole-variable assignment overwrites (`Other.Copy
            // := 1` names a field), and only one whose innermost loop also holds
            // the write: an inner loop's copy before an OUTER write cannot pair
            // with it (letting it overwrite would leave no finding), while one
            // before an INNER write does, and then it is the copy that counts.
            // And only one that runs whenever the write does (no `if` / `case`
            // branch around it that the write is outside of): `if X then Copy :=
            // Temp;` leaves the cursor copy on the path where X is false.
            let overwritten = routine.var_assignments.iter().any(|other| {
                !other.lhs_is_member
                    && other.lhs_name == asg.lhs_name
                    && before_anchor(&asg.source_anchor, &other.source_anchor)
                    && before_anchor(&other.source_anchor, &write.source_anchor)
                    && innermost(&other.source_anchor)
                        .is_some_and(|l| write.loop_stack.iter().any(|id| id == &l.id))
                    && unconditional_before(routine, &other.source_anchor, &write.source_anchor)
            });
            if overwritten {
                skipped_overwritten += 1;
                continue;
            }
            // The copy SOURCE must be a live cursor.
            let src_is_cursor = routine.record_operations.iter().any(|op| {
                CURSOR_OPS.contains(&op.op.as_str())
                    && op.record_variable_name.to_lowercase() == rhs_lc
            });
            if !src_is_cursor {
                skipped_source_not_cursor += 1;
                continue;
            }
            // The SOURCE must be a PHYSICAL cursor. A `temporary` source record is
            // an in-memory buffer being MATERIALIZED into a persisted target
            // (`Persisted := Temp; Persisted.Insert()/Modify()`) — a genuinely
            // different row, not the redundant re-write of the cursor's own row
            // the premise describes, and the `Copy := Cursor` assignment itself is
            // an in-memory struct copy with no SQL round-trip. Skip.
            let src_is_temp = routine
                .record_variables
                .iter()
                .any(|rv| rv.name.to_lowercase() == rhs_lc && is_known_temp_var(rv));
            if src_is_temp {
                skipped_source_temp += 1;
                continue;
            }
            // ENG-27: nor a temporary TARGET. Copying the cursor into a temp
            // buffer and writing the buffer is the remedy, not the problem, and
            // the write is no SQL round-trip (a `SourceTableTemporary` page's
            // `Rec`, a `temporary` local). The write op carries the temp state
            // (the model forces the page's implicit `Rec` ops to known-temp).
            if crate::program::body::features::temp_state_suppresses(write.temp_state.as_ref()) {
                skipped_target_temp += 1;
                continue;
            }

            // Key-field-reassignment skip: the clone is functionally required
            // (targets a DIFFERENT physical row) when, between the clone and the
            // write, a field WRITE on the CLONE targets a key field — the
            // target table's PRIMARY KEY (first `keys` entry) or a field named
            // in a `SetCurrentKey` call on the SOURCE cursor in this routine
            // (the current key, which need not be the PK).
            let pk_fields: HashSet<String> = write
                .table_id
                .as_deref()
                .and_then(|tid| ctx.table_by_id.get(tid))
                .copied()
                .map(primary_key_field_names_lc)
                .unwrap_or_default();
            let current_key_fields: HashSet<String> = routine
                .record_operations
                .iter()
                .filter(|op| {
                    op.op == "SetCurrentKey" && op.record_variable_name.to_lowercase() == rhs_lc
                })
                .filter_map(|op| op.field_arguments.as_ref())
                .flatten()
                .map(|f| normalize_load_field_arg(f))
                .collect();
            let key_field_reassigned = routine.field_accesses.iter().any(|fa| {
                fa.record_variable_name.to_lowercase() == asg.lhs_name
                    && before_anchor(&asg.source_anchor, &fa.source_anchor)
                    && before_anchor(&fa.source_anchor, &write.source_anchor)
                    && write_targets.contains(&(
                        fa.source_anchor.start_line,
                        fa.source_anchor.start_column,
                        fa.field_name.to_lowercase(),
                    ))
                    && (pk_fields.contains(&fa.field_name.to_lowercase())
                        || current_key_fields.contains(&fa.field_name.to_lowercase()))
            });
            if key_field_reassigned {
                skipped_key_remapped_clone += 1;
                continue;
            }

            let confidence: FindingConfidence = to_confidence(&[], "likely");
            let id = format!("d56/{}/{}/{}", routine.id, lp.id, write.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: format!("d56/{}/{}", routine.id, lp.id),
                detector: DETECTOR.to_string(),
                title: format!("Record cloned before {} in loop", write.op).into(),
                root_cause: format!(
                    "{} copies the loop cursor {} into {} and calls {} on the copy inside \
                     the loop — an extra SQL round-trip per row; the cursor already holds \
                     the row.",
                    routine.name, rhs_lc, asg.lhs_name, write.op
                ),
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&write.source_anchor, routine),
                evidence_path: vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: Some(lp.id.clone()),
                        source_anchor: anchor_of(&asg.source_anchor, routine),
                        note: format!(
                            "clone {} := {} inside {} loop",
                            asg.lhs_name, rhs_lc, lp.loop_type
                        ),
                    },
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(write.id.clone()),
                        callsite_id: None,
                        loop_id: Some(lp.id.clone()),
                        source_anchor: anchor_of(&write.source_anchor, routine),
                        note: format!("{} on the clone", write.op),
                    },
                ],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: id_list(write.table_id.iter().cloned()),
                fix_options: vec![FixOption {
                    description: format!(
                        "Call {} on the cursor ({}) directly, or restructure to a set-based \
                         write (ModifyAll/DeleteAll) outside the loop.",
                        write.op, rhs_lc
                    )
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
                loose_subject: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("noWriteBack", skipped_no_write_back);
    stats.add_skip("sourceNotCursor", skipped_source_not_cursor);
    stats.add_skip("sourceTemp", skipped_source_temp);
    stats.add_skip("targetTemp", skipped_target_temp);
    stats.add_skip("overwrittenCopy", skipped_overwritten);
    stats.add_skip("keyRemappedClone", skipped_key_remapped_clone);
    Ok(DetectorOutput::no_diag(findings, stats))
}
