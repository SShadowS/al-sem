//! D12 — dead integration event. Port of al-sem
//! `src/detectors/d12-dead-integration-event.ts`.
//!
//! Flags an `[IntegrationEvent]` published from primary-app code that has ZERO
//! subscribers anywhere in the workspace + dependency closure — a dead extensibility
//! surface. Reads the raw event graph (`ctx.event_graph`): integration `EventSymbol`s
//! with a primary publisher routine + 0 edges naming that event's id.
//!
//! `id = d12/{event.id}` where `event.id` is the INTERNAL EventId
//! `{publisherObjectId}/event/{eventName_lc}` (appGuid-based). The R4 projection does
//! NOT stabilize this — only internal RoutineIds (`r0/...`) are rewritten — so the
//! Rust event id must byte-match al-sem's `encodeEventId`. Within-detector sort by
//! `compareStrings(a.id, b.id)`. severity info; confidence `to_confidence(&[],
//! "likely")` → "likely".

use std::collections::HashMap;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

use super::anchor_of;

const DETECTOR: &str = "d12-dead-integration-event";

pub fn detect_d12(
    _resolved: &crate::program::model::workspace::Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();

    // subsByEvent: count of edges per internal eventId.
    let mut subs_by_event: HashMap<&str, usize> = HashMap::new();
    for edge in &ctx.event_graph.edges {
        *subs_by_event.entry(edge.event_id.as_str()).or_insert(0) += 1;
    }

    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
    let mut skipped_dependency = 0u64;

    for ev in &ctx.event_graph.events {
        if ev.event_kind != "integration" {
            continue;
        }
        let Some(pub_routine_id) = &ev.publisher_routine_id else {
            continue;
        };
        let Some(pub_routine) = ctx.routine_by_id.get(pub_routine_id.as_str()) else {
            continue;
        };
        let pub_routine: &crate::program::model::workspace::ModelRoutine = pub_routine;
        // roleOf(routine) !== "primary" → skip (al-sem d12 primary gate). A
        // dependency app's dead integration event is NOT the user's to fix (its
        // source isn't in the workspace), and a workspace subscriber to it would
        // be an edge on the event anyway. dep_routine_ids is EMPTY for source-only
        // runs ⇒ no behavior change there (matches d13/d16/d17 gating).
        if ctx.dep_routine_ids.contains(pub_routine_id.as_str()) {
            skipped_dependency += 1;
            continue;
        }
        candidates_considered += 1;
        if subs_by_event.get(ev.id.as_str()).copied().unwrap_or(0) > 0 {
            skipped_other += 1;
            continue;
        }

        let confidence: FindingConfidence = to_confidence(&[], "likely");

        let id = format!("d12/{}", ev.id);
        let root_cause_key = id.clone();

        let root_cause = format!(
            "{} publishes an integration event that no subscriber across this workspace or its \
             dependencies handles — the extensibility point is dead.",
            pub_routine.name
        );

        let path = vec![EvidenceStep {
            routine_id: pub_routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_of(&pub_routine.source_anchor, pub_routine),
            note: format!("publishes {}", pub_routine.name),
        }];

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: "Integration event has no subscribers".into(),
            root_cause,
            severity: "info".to_string(),
            confidence,
            primary_location: anchor_of(&pub_routine.source_anchor, pub_routine),
            evidence_path: path,
            additional_paths: None,
            affected_objects: vec![pub_routine.object_id.as_str().into()],
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description:
                    "Either remove the event if it has no real extensibility purpose, or document \
                     why it exists for future subscribers."
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    stats.add_skip("dependency", skipped_dependency);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
