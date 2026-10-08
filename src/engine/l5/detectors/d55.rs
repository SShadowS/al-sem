//! D55 — Direct event publish inside a loop. BCQuality
//! `do-not-publish-events-inside-loops`: each iteration dispatches EVERY
//! subscriber — cost is subscribers × iterations and grows as third parties
//! subscribe. d2 covers transitive fan-out-in-loop; d55 is the direct,
//! declaration-independent form (fires even with zero current subscribers —
//! the publish point itself is the hazard).
//!
//! Join: call site with non-empty loop_stack whose RESOLVED callee has
//! kind == "event-publisher". Severity: high when loop depth ≥ 2, else medium.
//! Confidence: likely. Inert on the cross-app context (resolver join empty).

use std::collections::HashMap;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PLoop;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d55-event-publish-in-loop";

pub fn detect_d55(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_not_in_loop = 0u64;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        let loop_by_id: HashMap<&str, &PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        for cs in &routine.call_sites {
            let Some(edge) = ctx.resolved_call_edge_by_callsite.get(&cs.id) else {
                continue;
            };
            let Some(to) = edge.to.as_deref() else {
                continue;
            };
            let Some(callee) = ctx.routine_by_id.get(to) else {
                continue;
            };
            if callee.kind != "event-publisher" {
                continue;
            }
            candidates_considered += 1;
            let Some(rep_loop_id) = cs.loop_stack.last() else {
                skipped_not_in_loop += 1;
                continue;
            };
            let Some(loop_info) = loop_by_id.get(rep_loop_id.as_str()) else {
                skipped_not_in_loop += 1;
                continue;
            };

            let severity = if cs.loop_stack.len() >= 2 {
                "high"
            } else {
                "medium"
            };
            let confidence: FindingConfidence = to_confidence(&[], "likely");
            let id = format!("d55/{}/{}/{}", routine.id, loop_info.id, cs.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: format!("d55/{}/{}", routine.id, loop_info.id),
                detector: DETECTOR.to_string(),
                title: "Event published inside loop".into(),
                root_cause: format!(
                    "{} publishes {} inside a {} loop — every subscriber runs once per \
                     iteration, and the cost grows as third parties subscribe.",
                    routine.name, callee.name, loop_info.loop_type
                ),
                severity: severity.to_string(),
                confidence,
                primary_location: anchor_of(&cs.source_anchor, routine),
                evidence_path: vec![
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
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: Some(loop_info.id.clone()),
                        source_anchor: anchor_of(&cs.source_anchor, routine),
                        note: format!("publishes {}", callee.name),
                    },
                ],
                additional_paths: None,
                affected_objects: id_list([routine.object_id.as_str(), callee.object_id.as_str()]),
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Accumulate the per-row data and publish ONE event after the \
                                  loop (pass a collection/buffer), or document why per-row \
                                  dispatch is required."
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
    stats.add_skip("notInLoop", skipped_not_in_loop);
    Ok(DetectorOutput::no_diag(findings, stats))
}
