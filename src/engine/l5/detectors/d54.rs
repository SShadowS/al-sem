//! D54 — Event published inside a `[TryFunction]` cone. A try boundary swallows
//! errors raised by SUBSCRIBERS of any event published under it — third-party
//! subscriber failures are silenced and their partial writes survive.
//! BCQuality-adjacent; the transitive form is unique to this engine's call graph.
//!
//! For each [TryFunction] routine: BFS over the combined graph's CALL edges
//! (never `event-dispatch` — the cone is the routine's own synchronous closure;
//! never `dynamic` — unresolved). Every reachable `event-publisher` routine is
//! one finding; the BFS parent chain is the evidence path. Publishers are not
//! traversed THROUGH (their bodies are empty declarations).
//!
//! Severity: medium. Confidence: likely when the publisher is called directly
//! from the try body (2-node chain), possible otherwise. Capped at 5 findings
//! per try routine via group_and_cap (skip bucket `cappedPerTryRoutine`).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, group_and_cap};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::has_attribute;

const DETECTOR: &str = "d54-publish-in-tryfunction-cone";
const MAX_PER_TRY_ROUTINE: usize = 5;

pub fn detect_d54(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_no_publisher_reached = 0u64;

    for routine in &ws.routines {
        if !has_attribute(&routine.attributes_parsed, "TryFunction") {
            continue;
        }
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        candidates_considered += 1;

        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        seen.insert(routine.id.as_str());
        queue.push_back(routine.id.as_str());
        let mut reached: Vec<&crate::engine::l3::l3_workspace::L3Routine> = Vec::new();

        while let Some(cur) = queue.pop_front() {
            let Some(edges) = ctx.graph.edges_by_from.get(cur) else {
                continue;
            };
            for e in edges {
                // Never cross INTO subscribers (event-dispatch) or through
                // unresolved dynamic edges.
                if e.kind == "event-dispatch" || e.kind == "dynamic" {
                    continue;
                }
                if seen.contains(e.to.as_str()) {
                    continue;
                }
                seen.insert(e.to.as_str());
                parent.insert(e.to.as_str(), cur);
                let Some(target) = ctx.routine_by_id.get(e.to.as_str()) else {
                    continue;
                };
                if target.kind == "event-publisher" {
                    reached.push(target); // do not traverse THROUGH a publisher
                } else {
                    queue.push_back(e.to.as_str());
                }
            }
        }

        if reached.is_empty() {
            skipped_no_publisher_reached += 1;
            continue;
        }

        for publisher in reached {
            // Chain: try routine -> ... -> publisher (via BFS parents).
            let mut chain: Vec<&str> = vec![publisher.id.as_str()];
            let mut cur = publisher.id.as_str();
            while let Some(&p) = parent.get(cur) {
                chain.push(p);
                cur = p;
            }
            chain.reverse();
            let direct = chain.len() == 2;

            let path: Vec<EvidenceStep> = chain
                .iter()
                .filter_map(|rid| ctx.routine_by_id.get(rid))
                .map(|r| EvidenceStep {
                    routine_id: r.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&r.source_anchor, r),
                    note: if r.kind == "event-publisher" {
                        format!("event publisher {}", r.name)
                    } else if has_attribute(&r.attributes_parsed, "TryFunction") {
                        format!("[TryFunction] {}", r.name)
                    } else {
                        r.name.clone()
                    },
                })
                .collect();

            let confidence: FindingConfidence =
                to_confidence(&[], if direct { "likely" } else { "possible" });
            let mut finding = Finding {
                id: format!("d54/{}/{}", routine.id, publisher.id),
                root_cause_key: format!("d54/{}", routine.id),
                detector: DETECTOR.to_string(),
                title: format!(
                    "Event published inside TryFunction cone{}",
                    if direct { "" } else { " (via callee)" }
                )
                .into(),
                root_cause: format!(
                    "{} is a TryFunction that {} the event publisher {} — errors raised by \
                     subscribers are swallowed by the try boundary, silencing third-party \
                     failures.",
                    routine.name,
                    if direct {
                        "directly calls"
                    } else {
                        "transitively reaches"
                    },
                    publisher.name
                ),
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&routine.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: id_list([
                    routine.object_id.as_str(),
                    publisher.object_id.as_str(),
                ]),
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Move the event publish outside the TryFunction boundary, or \
                                  document that subscriber errors are intentionally suppressed \
                                  on this path."
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

    let (mut findings, truncated) =
        group_and_cap(findings, |f| f.root_cause_key.clone(), MAX_PER_TRY_ROUTINE);
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("noPublisherReached", skipped_no_publisher_reached);
    stats.add_skip("cappedPerTryRoutine", truncated as u64);
    Ok(DetectorOutput::no_diag(findings, stats))
}
