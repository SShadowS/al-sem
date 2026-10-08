//! D34 — Commit inside a loop (direct or transitive). Port of al-sem
//! `src/detectors/d34-commit-in-loop.ts`.
//!
//! (a) DIRECT: operation_sites with kind=="commit" and non-empty loop_stack.
//!     Loop = loops indexed by last loop_stack entry.
//!     Severity: depth >= 2 → critical; else high. Confidence: likely.
//!     id = `d34/{routineId}/{loopId}/{site.id}`.
//!
//! (b) TRANSITIVE: in-loop call_sites where the resolved edge's callee summary
//!     has may_commit == Yes. Suppress if THIS routine also has a direct in-loop
//!     commit on the SAME loop. edge.kind == "event-dispatch" → skip.
//!     Severity: medium. Confidence: possible.
//!     id = `d34/{routineId}/{loopId}/{callsiteId}`.
//!
//! Within-detector sort by `a.id.cmp(&b.id)` (byte order).

use std::collections::HashMap;

use crate::engine::l5::capability_query::{EffectPresence, may_commit_derived};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PLoop;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d34-commit-in-loop";

pub fn detect_d34(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_suppressed_by_direct = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) === "primary": source-only ⇒ every routine is primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }
        candidates_considered += 1;

        // Build loop_by_id from routine.loops.
        let loop_by_id: HashMap<&str, &PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        // (a) Direct: operation_sites kind=="commit" with non-empty loop_stack.
        for site in &routine.operation_sites {
            if site.kind != "commit" {
                continue;
            }
            if site.loop_stack.is_empty() {
                continue;
            }
            let rep_id = match site.loop_stack.last() {
                Some(id) => id.as_str(),
                None => continue,
            };
            let Some(loop_info) = loop_by_id.get(rep_id) else {
                continue;
            };
            emit_direct(
                routine.id.as_str(),
                routine.name.as_str(),
                routine.object_id.as_str(),
                loop_info,
                site,
                fp_index,
                &mut findings,
            );
        }

        // (b) Transitive: in-loop call_sites.
        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue;
            }
            let rep_id = match cs.loop_stack.last() {
                Some(id) => id.as_str(),
                None => continue,
            };
            let Some(loop_info) = loop_by_id.get(rep_id) else {
                continue;
            };

            // Resolve edge from graph.edges_by_from by callsite_id.
            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges
                    .iter()
                    .find(|e| e.callsite_id.as_deref() == Some(&cs.id))
            });
            let Some(edge) = edge else {
                continue;
            };
            if edge.kind == "event-dispatch" {
                continue;
            }

            // Callee summary may_commit == Yes?
            let callee_id = &edge.to;
            let Some(callee_summary) = ctx.summaries.get(callee_id) else {
                continue;
            };
            if may_commit_derived(&ctx.cone_derived, callee_summary) != EffectPresence::Yes {
                continue;
            }

            // Suppress if this routine ALSO has a direct in-loop commit on the SAME loop.
            let has_direct_on_same_loop = routine
                .operation_sites
                .iter()
                .any(|s| s.kind == "commit" && s.loop_stack.iter().any(|l| l == &loop_info.id));
            if has_direct_on_same_loop {
                skipped_suppressed_by_direct += 1;
                continue;
            }

            // Get callee name.
            let callee_name = ctx
                .routine_by_id
                .get(callee_id.as_str())
                .map(|r| r.name.as_str())
                .unwrap_or(callee_id.as_str());

            emit_transitive(
                routine.id.as_str(),
                routine.name.as_str(),
                routine.object_id.as_str(),
                loop_info,
                cs.id.as_str(),
                anchor_of(&cs.source_anchor, routine),
                callee_name,
                fp_index,
                &mut findings,
                resolved,
            );
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    stats.add_skip("suppressedByDirect", skipped_suppressed_by_direct);
    Ok(DetectorOutput::no_diag(findings, stats))
}

fn emit_direct(
    routine_id: &str,
    routine_name: &str,
    object_id: &str,
    loop_info: &PLoop,
    site: &crate::program::body::features::POperationSite,
    fp_index: &crate::engine::l5::fingerprint::FingerprintIndex,
    findings: &mut Vec<Finding>,
) {
    // We need the enclosing_routine_id for anchors — build from scratch using routine_id.
    // The anchor_of helper needs an ModelRoutine; since we have the ids, build the anchors manually.
    // Use the routine's id as the enclosing_routine_id for both anchors.
    let loop_anchor = SourceAnchor {
        source_unit_id: loop_info.source_anchor.source_unit_id.clone(),
        start_line: loop_info.source_anchor.start_line,
        start_column: loop_info.source_anchor.start_column,
        end_line: loop_info.source_anchor.end_line,
        end_column: loop_info.source_anchor.end_column,
        enclosing_routine_id: routine_id.to_string(),
        syntax_kind: loop_info.source_anchor.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    };
    let site_anchor = SourceAnchor {
        source_unit_id: site.source_anchor.source_unit_id.clone(),
        start_line: site.source_anchor.start_line,
        start_column: site.source_anchor.start_column,
        end_line: site.source_anchor.end_line,
        end_column: site.source_anchor.end_column,
        enclosing_routine_id: routine_id.to_string(),
        syntax_kind: site.source_anchor.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    };

    let depth = site.loop_stack.len();
    let severity = if depth >= 2 { "critical" } else { "high" };
    let title = if depth >= 2 {
        "Commit inside a nested loop"
    } else {
        "Commit inside a loop"
    };
    let commit_note = if depth >= 2 {
        format!("Commit (loop depth {})", depth)
    } else {
        "Commit".to_string()
    };

    let path = vec![
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: Some(loop_info.id.clone()),
            source_anchor: loop_anchor,
            note: format!("{} loop", loop_info.loop_type),
        },
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: Some(site.id.clone()),
            callsite_id: None,
            loop_id: None,
            source_anchor: site_anchor.clone(),
            note: commit_note,
        },
    ];

    let confidence: FindingConfidence = to_confidence(&[], "likely");
    let id = format!("d34/{}/{}/{}", routine_id, loop_info.id, site.id);
    let root_cause_key = id.clone();

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: title.to_string().into(),
        root_cause: format!(
            "{} calls Commit inside a {} loop \u{2014} per-iteration commits break atomicity \
             and prevent the job from being retried safely.",
            routine_name, loop_info.loop_type
        ),
        severity: severity.to_string(),
        confidence,
        primary_location: site_anchor,
        evidence_path: path,
        additional_paths: None,
        affected_objects: vec![object_id.to_string().into()],
        affected_tables: Vec::new(),
        fix_options: vec![FixOption {
            description: "Move the Commit outside the loop. If progress-saving is genuinely \
                          required, document a chunking strategy and consider a job queue."
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

#[allow(clippy::too_many_arguments)]
fn emit_transitive(
    routine_id: &str,
    routine_name: &str,
    object_id: &str,
    loop_info: &PLoop,
    callsite_id: &str,
    callsite_anchor: SourceAnchor,
    callee_name: &str,
    fp_index: &crate::engine::l5::fingerprint::FingerprintIndex,
    findings: &mut Vec<Finding>,
    _resolved: &Model,
) {
    let loop_anchor = SourceAnchor {
        source_unit_id: loop_info.source_anchor.source_unit_id.clone(),
        start_line: loop_info.source_anchor.start_line,
        start_column: loop_info.source_anchor.start_column,
        end_line: loop_info.source_anchor.end_line,
        end_column: loop_info.source_anchor.end_column,
        enclosing_routine_id: routine_id.to_string(),
        syntax_kind: loop_info.source_anchor.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    };

    let path = vec![
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: Some(loop_info.id.clone()),
            source_anchor: loop_anchor,
            note: format!("{} loop", loop_info.loop_type),
        },
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: None,
            callsite_id: Some(callsite_id.to_string()),
            loop_id: None,
            source_anchor: callsite_anchor.clone(),
            note: format!("calls {} (transitively commits)", callee_name),
        },
    ];

    let confidence: FindingConfidence = to_confidence(&[], "possible");
    let id = format!("d34/{}/{}/{}", routine_id, loop_info.id, callsite_id);
    let root_cause_key = id.clone();

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: "Loop reaches a Commit through a callee".into(),
        root_cause: format!(
            "{}'s {} loop calls {}, which commits \u{2014} per-iteration commits break atomicity \
             even when the Commit isn't visible at the loop site.",
            routine_name, loop_info.loop_type, callee_name
        ),
        severity: "medium".to_string(),
        confidence,
        primary_location: callsite_anchor,
        evidence_path: path,
        additional_paths: None,
        affected_objects: vec![object_id.to_string().into()],
        affected_tables: Vec::new(),
        fix_options: vec![FixOption {
            description: "Verify the callee really needs to Commit. If the loop is correct as \
                          written, hoist the work that requires a Commit (or the Commit itself) \
                          outside the loop."
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
