//! D38 — primary-app `[EventSubscriber(...)]` bound to a publisher routine carrying
//! `[Obsolete(...)]`. Port of al-sem `src/detectors/d38-subscriber-to-obsolete-event.ts`.
//!
//! Reads the raw event graph (`ctx.event_graph`): for every `resolved` edge, resolve
//! the subscriber (primary + !parseIncomplete) and the event's publisher routine, then
//! parse the publisher's `[Obsolete]` state. severity Removed→high / Pending→info.
//!
//! `id = d38/{subscriberId}/{event.id}` — the subscriberId is an INTERNAL RoutineId
//! (the projection rewrites it to stable form); the `event.id` is the INTERNAL EventId
//! (appGuid-based) which passes through the projection verbatim. Two evidence steps
//! (subscriber anchor, publisher anchor). Within-detector sort by
//! `compareStrings(a.id, b.id)`. confidence `to_confidence(&[], "confirmed")`.

use std::collections::HashMap;

use crate::engine::l3::event_graph::EventSymbol;
use crate::engine::l3::l3_workspace::{L3Resolved, L3Routine};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::{ObsoleteState, parse_routine_attributes};

use super::anchor_of;

const DETECTOR: &str = "d38-subscriber-to-obsolete-event";

pub fn detect_d38(
    _resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();

    // eventById: internal eventId → EventSymbol.
    let event_by_id: HashMap<&str, &EventSymbol> = ctx
        .event_graph
        .events
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();

    let mut candidates_considered = 0usize;
    let mut skipped_unresolved = 0u64;
    let mut skipped_no_publisher_routine = 0u64;
    let mut skipped_publisher_missing = 0u64;
    let mut skipped_publisher_not_obsolete = 0u64;

    for edge in &ctx.event_graph.edges {
        if edge.resolution != "resolved" {
            skipped_unresolved += 1;
            continue;
        }
        let Some(subscriber) = ctx.routine_by_id.get(edge.subscriber_routine_id.as_str()) else {
            continue;
        };
        let subscriber: &L3Routine = subscriber;
        // roleOf(subscriber) !== "primary" → skip. Source-only ⇒ always primary.
        if subscriber.parse_incomplete {
            continue;
        }

        let Some(event) = event_by_id.get(edge.event_id.as_str()) else {
            continue;
        };
        let Some(publisher_routine_id) = &event.publisher_routine_id else {
            skipped_no_publisher_routine += 1;
            continue;
        };
        let Some(publisher) = ctx.routine_by_id.get(publisher_routine_id.as_str()) else {
            skipped_publisher_missing += 1;
            continue;
        };
        let publisher: &L3Routine = publisher;
        candidates_considered += 1;

        let attrs = parse_routine_attributes(&publisher.attributes_parsed);
        let Some(obsolete_state) = attrs.obsolete_state else {
            skipped_publisher_not_obsolete += 1;
            continue;
        };

        let (state_label, severity): (&str, &str) = match obsolete_state {
            ObsoleteState::Removed => ("Removed", "high"),
            ObsoleteState::Pending => ("Pending", "info"),
        };

        let publisher_note = match &attrs.obsolete_reason {
            Some(reason) => format!(
                "publisher {} is [Obsolete({})] — {}",
                publisher.name, state_label, reason
            ),
            None => format!(
                "publisher {} is [Obsolete({})]",
                publisher.name, state_label
            ),
        };

        let path = vec![
            EvidenceStep {
                routine_id: subscriber.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&subscriber.source_anchor, subscriber),
                note: format!("[EventSubscriber] subscribes to '{}'", event.event_name),
            },
            EvidenceStep {
                routine_id: publisher.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&publisher.source_anchor, publisher),
                note: publisher_note,
            },
        ];

        let root_cause = match obsolete_state {
            ObsoleteState::Removed => format!(
                "{} subscribes to '{}', whose publisher {} is [Obsolete(Removed)] — the \
                 subscriber will stop firing once the publisher is removed.",
                subscriber.name, event.event_name, publisher.name
            ),
            ObsoleteState::Pending => format!(
                "{} subscribes to '{}', whose publisher {} is [Obsolete(Pending)] — plan a \
                 migration to the successor before the publisher is removed.",
                subscriber.name, event.event_name, publisher.name
            ),
        };

        let mut affected_objects = vec![subscriber.object_id.clone(), publisher.object_id.clone()];
        affected_objects.sort();

        let confidence: FindingConfidence = to_confidence(&[], "confirmed");

        let id = format!("d38/{}/{}", subscriber.id, event.id);
        let root_cause_key = id.clone();

        let fix_description = attrs.obsolete_reason.clone().unwrap_or_else(|| {
            "Migrate the subscriber to the documented successor event; if none exists, remove the \
             subscription once the publisher is gone."
                .to_string()
        });

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: format!("Subscriber bound to obsolete event ({})", state_label).into(),
            root_cause,
            severity: severity.to_string(),
            confidence,
            primary_location: anchor_of(&subscriber.source_anchor, subscriber),
            evidence_path: path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description: fix_description.into(),
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("unresolved", skipped_unresolved);
    stats.add_skip("noPublisherRoutine", skipped_no_publisher_routine);
    stats.add_skip("publisherMissing", skipped_publisher_missing);
    stats.add_skip("publisherNotObsolete", skipped_publisher_not_obsolete);
    Ok(DetectorOutput::no_diag(findings, stats))
}
