//! D19 — declared procedure parameter never referenced in the routine body.
//! Port of al-sem `src/detectors/d19-unused-parameter.ts`.
//!
//! Restricted to `procedure` (NOT triggers, NOT event subscribers — the latter's
//! signature is dictated by the publisher and must keep every parameter declared,
//! even unused — and NOT event publishers). bodyAvailable + !parseIncomplete; skip
//! when there are 0 params.
//!
//! The event-publisher exclusion (issue 25) is a CONSTRUCTION argument, not a
//! heuristic: an `[IntegrationEvent]`/`[BusinessEvent]` body is empty by language
//! definition — the compiler generates the dispatch — so a publisher parameter is
//! unreferenced in its own body *always*. Its readers are the SUBSCRIBERS, which
//! this body-local predicate cannot see, making d19 100% false-positive on that
//! population. This is NOT a claim that a publisher parameter can never deserve
//! removal; that question needs subscriber-use evidence and belongs to the
//! event-graph detectors (d12 already covers the dead-event case).
//!
//! `refs = set(routine.identifier_references)` (the L2 lowercased / sorted /
//! deduped set). A parameter whose lowercased name is absent from that set is
//! unreferenced. Unnamed slots (empty name) are never flagged.
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order).

use std::collections::HashSet;

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

const DETECTOR: &str = "d19-unused-parameter";

pub fn detect_d19(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_trigger = 0u64;
    let mut skipped_event_subscriber = 0u64;
    let mut skipped_event_publisher = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        // procedure only — skip triggers, event subscribers + event publishers.
        if routine.kind == "trigger" {
            skipped_trigger += 1;
            continue;
        }
        if routine.kind == "event-subscriber" {
            skipped_event_subscriber += 1;
            continue;
        }
        // A routine carrying BOTH a subscriber and a publisher attribute is booked
        // as a SUBSCRIBER — but that precedence is decided upstream, in
        // `ir_walk::ir_routine_kind`, not here: by the time d19 runs, `routine.kind`
        // is already one single string. These three checks are mutually-exclusive
        // equality tests on that one scalar, so REORDERING THEM CHANGES NOTHING —
        // measured, by swapping them and watching every test stay green (issue 25).
        // The guard that actually matters is `ir_routine_kind`'s branch order, and
        // `gap_d19_event_publisher_skip::dual_attribute_routine_books_the_subscriber_counter`
        // is what pins it (inverting that branch order fails the test).
        if routine.kind == "event-publisher" {
            skipped_event_publisher += 1;
            continue;
        }
        if routine.parameters.is_empty() {
            continue;
        }
        candidates_considered += 1;

        let refs: HashSet<&str> = routine
            .identifier_references
            .iter()
            .map(|s| s.as_str())
            .collect();

        for param in &routine.parameters {
            let lc = param.name.to_lowercase();
            if lc.is_empty() {
                continue; // unnamed slot — never flag
            }
            if refs.contains(lc.as_str()) {
                continue;
            }

            let path = vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&routine.source_anchor, routine),
                note: format!(
                    "parameter '{}: {}' declared but never referenced",
                    param.name, param.type_text
                ),
            }];

            let id = format!("d19/{}/p{}", routine.id, param.index);
            let root_cause_key = id.clone();

            let confidence: FindingConfidence = to_confidence(&[], "likely");

            let root_cause = format!(
                "{} declares parameter '{}' ({}) at position {} but the body never references it.",
                routine.name, param.name, param.type_text, param.index
            );

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Procedure parameter is never used".into(),
                root_cause,
                severity: "info".to_string(),
                confidence,
                primary_location: anchor_of(&routine.source_anchor, routine),
                evidence_path: path,
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description:
                        "Remove the parameter, or wire it into the procedure body. If callers \
                         must keep the existing signature, leave it and silence with an `_` \
                         prefix on the name."
                            .into(),
                    safety: "low".into(),
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
    stats.add_skip("trigger", skipped_trigger);
    stats.add_skip("eventSubscriber", skipped_event_subscriber);
    stats.add_skip("eventPublisher", skipped_event_publisher);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
