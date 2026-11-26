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
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, LooseSubject,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

use super::anchor_of;

pub(crate) const DETECTOR: &str = "d12-dead-integration-event";

pub fn detect_d12(
    resolved: &crate::program::model::workspace::Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    use al_syntax::IdentifierFoldExt as _;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();

    // subsByEvent: count of edges per internal eventId.
    let mut subs_by_event: HashMap<&str, usize> = HashMap::new();
    // A subscription whose publisher object did not resolve (a name several apps'
    // objects carry, ENG-193) sits on a stand-in event `unknown/{type}/0:{name or
    // number}/event/{event}` (`model::events`): it may subscribe to any event of
    // that type, object reference and name, so such an event is not dead. Only a
    // WORKSPACE subscriber's stand-in counts: a dependency cannot see the
    // workspace's objects (sol r1 on ENG-193). The ids are matched whole, as
    // `model::events` writes them, never parsed (a quoted name may hold
    // `/event/`).
    let mut maybe_subscribed: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for edge in &ctx.event_graph.edges {
        *subs_by_event.entry(edge.event_id.as_str()).or_insert(0) += 1;
        if edge.event_id.starts_with("unknown/")
            && !ctx
                .dep_routine_ids
                .contains(edge.subscriber_routine_id.as_str())
        {
            maybe_subscribed.insert(edge.event_id.as_str());
        }
    }
    let objects: HashMap<&str, &crate::program::model::workspace::ModelObject> = resolved
        .workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o))
        .collect();

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
        let kind = pub_routine.object_type.fold_identifier();
        let object = objects.get(pub_routine.object_id.as_str());
        // How a subscription may name the publisher: its object by name or number,
        // or, for an event an extension publishes, the base object by name
        // (`ObjectType::Table, Database::T` for a tableextension's event).
        let mut refs: Vec<(String, String)> =
            vec![(kind.clone(), pub_routine.object_number.to_string())];
        if let Some(o) = object {
            refs.push((kind.clone(), o.name.fold_identifier()));
            if let (Some(base_kind), Some(base)) =
                (kind.strip_suffix("extension"), &o.extends_target_name)
            {
                refs.push((base_kind.to_string(), base.fold_identifier()));
            }
        }
        if refs.into_iter().any(|(k, r)| {
            maybe_subscribed.contains(
                crate::program::model::events::encode_event_id(
                    &format!("unknown/{k}/0:{r}"),
                    &ev.event_name,
                )
                .as_str(),
            )
        }) {
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
            // #252: the event (the anchor IS the event); equals `root_cause_key`.
            loose_subject: Some(LooseSubject {
                object_id: None,
                key: format!("d12/{}", ev.id),
            }),
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::detect_d12;
    use crate::engine::l4::combined_graph::PrimaryGraph;
    use crate::engine::l5::detector_context::detector_context;
    use crate::engine::l5::test_support::{empty_artifacts, empty_resolved, routine};
    use crate::program::model::events::{EventEdge, EventGraph, EventSymbol, encode_event_id};

    /// The names of the events d12 reports dead, over a workspace codeunit
    /// (number 50) publishing `event`, subscribed through the stand-in ids
    /// `subs` (`(subscriber routine id, stand-in object)`); `r_dep` is a
    /// dependency routine.
    fn dead(event: &str, subs: &[(&str, &str)]) -> Vec<String> {
        let mut publisher = routine("r_pub", "procedure");
        publisher.object_id = "ws/Codeunit/50".to_string();
        publisher.object_number = 50;
        publisher.name = event.to_string();
        let routines = vec![
            publisher,
            routine("r_ws", "procedure"),
            routine("r_dep", "procedure"),
        ];
        let symbol = EventSymbol {
            id: encode_event_id("ws/Codeunit/50", event),
            publisher_object_id: "ws/Codeunit/50".to_string(),
            publisher_routine_id: Some("r_pub".to_string()),
            publisher_stable_routine_id: Some("stable::r_pub".to_string()),
            event_name: event.to_string(),
            event_kind: "integration".to_string(),
            element_name: None,
            signature_hash: String::new(),
            parameters: Vec::new(),
            isolated: None,
            provenance: Vec::new(),
            publisher_ref: None,
        };
        let edges = subs
            .iter()
            .map(|(sub, object)| EventEdge {
                event_id: encode_event_id(object, event),
                subscriber_routine_id: (*sub).to_string(),
                subscriber_stable_routine_id: format!("stable::{sub}"),
                subscriber_app_id: "app".to_string(),
                resolution: "unknown".to_string(),
                provenance: Vec::new(),
                conditional: false,
                mixed_readings: false,
            })
            .collect();
        let mut m = empty_resolved();
        m.workspace.routines = routines.clone();
        let mut art = empty_artifacts(
            PrimaryGraph {
                nodes: Vec::new(),
                edges_by_from: HashMap::new(),
                edges_from_order: Vec::new(),
                uncertainty_edges: Vec::new(),
                collided_site_callee: HashMap::new(),
            },
            HashMap::new(),
        );
        art.dep_routine_ids = ["r_dep".to_string()].into();
        // The context reads the artifacts' event graph (the combined-graph
        // build's), not the model's.
        art.event_graph = EventGraph {
            events: vec![symbol],
            edges,
        };
        let ctx = detector_context(&m, &art, None, None);
        detect_d12(&m, &ctx)
            .unwrap()
            .findings
            .iter()
            .map(|f| f.id.clone())
            .collect()
    }

    /// ENG-193 (sol r1): a subscription whose publisher object did not resolve
    /// (a stand-in id) keeps a workspace event alive only when WORKSPACE code
    /// makes it (a dependency cannot see the workspace's objects), and the id
    /// is matched whole, so an event name holding `/event/` still matches.
    /// Hand-stated: the stand-in ids are built as `model::events` writes them.
    ///
    /// DISCRIMINATION (2026-11-25, each break confirmed applied, then
    /// reverted): counting a dependency subscriber's stand-in again fails "a
    /// dependency's stand-in keeps nothing alive"; dropping the stand-in check
    /// fails "a workspace stand-in keeps it alive".
    #[test]
    fn only_a_workspace_stand_in_keeps_an_event_alive() {
        let by_number = "unknown/codeunit/0:50";
        assert_eq!(
            dead("OnY", &[("r_dep", by_number)]).len(),
            1,
            "a dependency's stand-in keeps nothing alive"
        );
        assert!(
            dead("OnY", &[("r_ws", by_number)]).is_empty(),
            "a workspace stand-in keeps it alive"
        );
        assert!(
            dead("A/event/B", &[("r_ws", by_number)]).is_empty(),
            "an event name holding /event/ matches whole"
        );
        assert_eq!(dead("OnY", &[]).len(), 1, "control: no subscriber, dead");
    }
}
