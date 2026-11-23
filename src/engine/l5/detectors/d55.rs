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

pub(crate) const DETECTOR: &str = "d55-event-publish-in-loop";

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
        // ENG-101: the n-th publish of one event in one loop, in source order.
        // `call_sites` come in the body walk's order, which is source order for
        // the statement calls a publish is (a publisher returns nothing, so it is
        // never an argument); the sort makes `#n` not depend on that walk.
        let mut sites: Vec<_> = routine.call_sites.iter().collect();
        sites.sort_by(|a, b| {
            (
                a.source_anchor.start_line,
                a.source_anchor.start_column,
                &a.id,
            )
                .cmp(&(
                    b.source_anchor.start_line,
                    b.source_anchor.start_column,
                    &b.id,
                ))
        });
        let mut nth: HashMap<(&str, &str), usize> = HashMap::new();

        for cs in sites {
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
            // ENG-101 (owner decision 2026-11-18): the published event, and its
            // repeat number in this loop, are part of the root cause. Each event is
            // its own hazard; keyed by the loop alone, every publish in one loop
            // shared one fingerprint (suppressing one suppressed all, and a new
            // event in the loop was invisible to a baseline). `#n` tells apart two
            // publishes of the SAME event in one loop (CDO has one such loop).
            let n = nth
                .entry((loop_info.id.as_str(), callee.id.as_str()))
                .or_insert(0);
            *n += 1;
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: format!("d55/{}/{}/{}#{}", routine.id, loop_info.id, callee.id, n),
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
                loose_subject: None,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::combined_graph::PrimaryGraph;
    use crate::engine::l5::detector_context::{DetectorArtifacts, detector_context};
    use crate::engine::l5::test_support::{call_site, empty_artifacts, loop_def, routine};
    use crate::program::model::calls::CallEdge;
    use crate::program::model::ix::RoutineIx;
    use crate::program::model::workspace::ModelEntities;

    /// ENG-101: `#n` counts the publishes of one event in one loop in SOURCE
    /// order, not in `call_sites` order. Precondition, by assignment: `call_sites`
    /// lists `caller/cs0` (line 10) before `caller/cs1` (line 5), both publishing
    /// `pub` in `caller/loop0`. So `cs1` is `#1` and `cs0` is `#2`.
    ///
    /// Discrimination (2026-11-23): the `sites.sort_by` removed (walk order) ->
    /// "cs1, first in source, is #1" fails (`cs1` got `#2`). Restored, green.
    #[test]
    fn repeat_number_follows_source_order() {
        let mut caller = routine("caller", "procedure");
        caller.loops = vec![loop_def("caller/loop0")];
        let site = |id: &str, line: u32| {
            let mut cs = call_site(id, "Pub", vec!["caller/loop0".to_string()]);
            cs.source_anchor.start_line = line;
            cs
        };
        caller.call_sites = vec![site("caller/cs0", 10), site("caller/cs1", 5)];
        let routines = vec![caller, routine("pub", "event-publisher")];
        let edge = |cs: &str| {
            let mut e = CallEdge::base(RoutineIx(0), "caller", cs, &format!("{cs}/op"));
            e.to = Some("pub".to_string());
            (cs.to_string(), e)
        };
        let art = DetectorArtifacts {
            resolved_call_edge_by_callsite: HashMap::from([edge("caller/cs0"), edge("caller/cs1")]),
            ..empty_artifacts(
                PrimaryGraph {
                    nodes: vec![],
                    edges_by_from: HashMap::new(),
                    edges_from_order: vec![],
                    uncertainty_edges: vec![],
                    collided_site_callee: HashMap::new(),
                },
                HashMap::new(),
            )
        };
        let resolved = Model {
            workspace: ModelEntities {
                objects: vec![],
                tables: vec![],
                routines,
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: Vec::new(),
            workspace_diagnostics: Vec::new(),
            dependency_diagnostics: Vec::new(),
            calls: Default::default(),
            events: Default::default(),
            page_ui_blocked: Default::default(),
        };
        let ctx = detector_context(&resolved, &art, None, None);
        let out = detect_d55(&resolved, &ctx).unwrap();
        let key_of = |cs: &str| {
            out.findings
                .iter()
                .find(|f| f.id.ends_with(&format!("/{cs}")))
                .map(|f| f.root_cause_key.clone())
                .unwrap_or_else(|| panic!("no finding for {cs}: {:#?}", out.findings))
        };
        assert!(
            key_of("caller/cs1").ends_with("#1"),
            "cs1, first in source, is #1"
        );
        assert!(key_of("caller/cs0").ends_with("#2"), "cs0 is #2");
    }
}
