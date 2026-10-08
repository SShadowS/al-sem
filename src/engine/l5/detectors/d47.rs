//! D47 — external IO (HTTP/FILE) executed in an UNSAFE transaction context.
//! Port of al-sem `src/detectors/d47-io-unsafe-txn.ts`.
//!
//! Consumes the L4.5 ordering facts (`ctx.get_ordering_facts()`) and grades each
//! fact via `grade_guarantee`. Owns the three external-IO labels:
//!   - WRITE_PENDING_AT_EXTERNAL_IO + HTTP  → critical
//!   - WRITE_PENDING_AT_EXTERNAL_IO + FILE  → high (FILE direction unknown today)
//!   - EXTERNAL_IO_BEFORE_COMMIT + write    → info advisory (deduped vs WRITE_PENDING)
//!   - EXTERNAL_IO_BEFORE_COMMIT + read/unknown → suppressed
//!   - EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN  → info advisory (with event hop)
//!   - none / suppressed                    → NO finding
//!
//! Within-detector sort by `compareStrings(id)` (= `str::cmp`), then dedup by id.

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, SourceAnchor};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::ordering_facts::{
    OrderingFact, OrderingFacts, grade_guarantee, is_reportable_routine,
    stable_routine_id_for_routine, to_severity,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d47-io-unsafe-txn";

/// `ioDetail.method` lookup helper.
fn detail_method(detail: &[(String, String)]) -> Option<&str> {
    detail
        .iter()
        .find(|(k, _)| k == "method")
        .map(|(_, v)| v.as_str())
}

fn build_d47_finding(
    fp: &FingerprintIndex,
    routine: &ModelRoutine,
    of: &OrderingFacts,
    fact: &OrderingFact,
    sev: &str,
) -> Finding {
    let is_advisory = fact.guarantee.label == "EXTERNAL_IO_BEFORE_COMMIT" && sev == "info";
    let io_anchor = io_source_anchor(fact, routine);
    let write_anchor = to_src(fact.write_anchor.as_ref(), routine);
    let commit_anchor = to_src(fact.commit_anchor.as_ref(), routine);

    let method = detail_method(&fact.io_detail);
    let io_label = match method {
        Some(m) => format!("{} {}", fact.io_type, m),
        None => fact.io_type.clone(),
    };

    let mut evidence_path: Vec<EvidenceStep> = Vec::new();
    if let Some(wa) = &write_anchor {
        evidence_path.push(EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: wa.clone(),
            note: "DB write — transaction now dirty".to_string(),
        });
    }
    let io_note = if fact.guarantee.label == "WRITE_PENDING_AT_EXTERNAL_IO" {
        format!("{io_label} call inside open write transaction")
    } else {
        format!("{io_label} call before commit (advisory)")
    };
    evidence_path.push(EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: io_anchor
            .clone()
            .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine)),
        note: io_note,
    });
    if let Some(ca) = &commit_anchor {
        evidence_path.push(EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: ca.clone(),
            note: "commit after external IO".to_string(),
        });
    }

    let primary_location = io_anchor
        .clone()
        .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine));

    let title = if is_advisory {
        "External IO before commit (advisory)".to_string()
    } else {
        "External IO inside an open write transaction".to_string()
    };

    let root_cause = if is_advisory {
        format!(
            "{} performs {io_label} ordered before a Commit(); if the commit's write is the \
             durable record of that IO, a rollback would orphan the external effect. Commit \
             effectiveness and orphan relatedness are NOT proven — review manually.",
            routine.name
        )
    } else if fact.guarantee.label == "WRITE_PENDING_AT_EXTERNAL_IO" {
        format!(
            "{} performs {io_label} while a database write is still pending (uncommitted). The \
             external call happens inside an open write transaction — if it blocks or fails the \
             transaction is held open across the network round-trip, and BC's runtime forbids {} \
             during a write transaction.",
            routine.name, fact.io_type
        )
    } else {
        format!(
            "{} performs {io_label} before the write transaction is durably committed — if the \
             process rolls back after the IO, the external side effect is orphaned.",
            routine.name
        )
    };

    let fix_options = if is_advisory {
        vec![FixOption {
            description:
                "Review whether a rollback after this external call would leave the remote \
                 side-effect without a corresponding DB record. If so, move the Commit() before \
                 the external IO, or ensure the IO is idempotent and the DB record is \
                 re-reconciled on retry."
                    .into(),
            safety: "medium".into(),
        }]
    } else {
        vec![FixOption {
            description:
                "Commit (or complete) the write transaction before making the external call, or \
                 move the external IO out of the transactional path entirely. Holding a write \
                 transaction open across a network call risks deadlocks and orphaned side effects."
                    .into(),
            safety: "medium".into(),
        }]
    };

    let mut finding = Finding {
        id: format!("d47/{}/{}", of.routine_id, fact.key),
        root_cause_key: format!("d47/{}", of.routine_id),
        detector: DETECTOR.to_string(),
        title: title.into(),
        root_cause,
        severity: sev.to_string(),
        confidence: to_confidence(&[], "likely"),
        primary_location,
        evidence_path,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options,
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
    finding.fingerprint = Some(fp.fingerprint_of(&finding));
    finding
}

/// Minimal event-hop context for the event-crossed advisory.
struct EventHopContext {
    event_names: Vec<String>,
    subscriber_routine_ids: Vec<String>,
}

/// Build the event-hop context for a publisher routine; `None` when the routine
/// raises no known events.
fn build_event_hop_context(
    ctx: &DetectorContext,
    publisher_routine_id: &str,
) -> Option<EventHopContext> {
    let mut event_ids: HashSet<String> = HashSet::new();
    let mut event_names: Vec<String> = Vec::new();
    for ev in &ctx.event_graph.events {
        if ev.publisher_routine_id.as_deref() == Some(publisher_routine_id) {
            event_ids.insert(ev.id.clone());
            event_names.push(ev.event_name.clone());
        }
    }
    if event_ids.is_empty() {
        return None;
    }
    event_names.sort();

    let mut sub_set: HashSet<String> = HashSet::new();
    for edge in &ctx.event_graph.edges {
        if edge.resolution != "resolved" {
            continue;
        }
        if event_ids.contains(&edge.event_id) {
            sub_set.insert(edge.subscriber_routine_id.clone());
        }
    }
    let mut subscriber_routine_ids: Vec<String> = sub_set.into_iter().collect();
    subscriber_routine_ids.sort();

    Some(EventHopContext {
        event_names,
        subscriber_routine_ids,
    })
}

fn routine_name_by_id(ctx: &DetectorContext, id: &str) -> String {
    ctx.routine_by_id
        .get(id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn build_d47_event_advisory_finding(
    fp: &FingerprintIndex,
    ctx: &DetectorContext,
    routine: &ModelRoutine,
    of: &OrderingFacts,
    fact: &OrderingFact,
    sev: &str,
    event_ctx: Option<&EventHopContext>,
) -> Finding {
    let io_anchor = io_source_anchor(fact, routine);
    let write_anchor = to_src(fact.write_anchor.as_ref(), routine);

    let method = detail_method(&fact.io_detail);
    let io_label = match method {
        Some(m) => format!("{} {}", fact.io_type, m),
        None => fact.io_type.clone(),
    };

    let event_name_display = match event_ctx {
        Some(c) if !c.event_names.is_empty() => c.event_names.join(", "),
        _ => "integration event".to_string(),
    };

    let subscriber_display = match event_ctx {
        Some(c) if !c.subscriber_routine_ids.is_empty() => {
            let names: Vec<String> = c
                .subscriber_routine_ids
                .iter()
                .map(|id| routine_name_by_id(ctx, id))
                .collect();
            if names.len() <= 3 {
                names.join(", ")
            } else {
                format!("{} (+{} more)", names[..3].join(", "), names.len() - 3)
            }
        }
        _ => "one or more subscribers".to_string(),
    };

    let mut evidence_path: Vec<EvidenceStep> = Vec::new();
    if let Some(wa) = &write_anchor {
        evidence_path.push(EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: wa.clone(),
            note: "DB write — transaction now dirty".to_string(),
        });
    }
    evidence_path.push(EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: anchor_of(&routine.source_anchor, routine),
        note: format!(
            "event dispatch — {event_name_display} fires synchronously; a bound subscriber would \
             execute inside this uncommitted transaction (binding not proven)"
        ),
    });
    evidence_path.push(EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: io_anchor
            .clone()
            .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine)),
        note: format!(
            "{io_label} call in subscriber ({subscriber_display}) — inside publisher's open write \
             transaction"
        ),
    });

    let primary_location = io_anchor
        .clone()
        .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine));

    let title = "External IO in event subscriber inside publisher's write transaction (advisory)"
        .to_string();

    let root_cause = format!(
        "{} dirties the database transaction (DB write) then raises {event_name_display}. A \
         synchronous event subscriber ({subscriber_display}) may run inside the publisher's \
         uncommitted transaction and issue an external request ({io_label}). If the subscriber is \
         bound and the event is non-isolated, the external call executes before the publisher's \
         transaction commits or rolls back. Subscriber binding and execution order are NOT proven \
         — review manually.",
        routine.name
    );

    let fix_options = vec![FixOption {
        description: "Commit the write transaction before raising the event, or declare the event \
             Isolated=true ([IntegrationEvent(false, false, true)]) so subscribers run in a \
             separate transaction. Alternatively, move external IO out of the subscriber or \
             ensure it is idempotent."
            .into(),
        safety: "medium".into(),
    }];

    let mut finding = Finding {
        id: format!("d47/{}/{}", of.routine_id, fact.key),
        root_cause_key: format!("d47/{}", of.routine_id),
        detector: DETECTOR.to_string(),
        title: title.into(),
        root_cause,
        severity: sev.to_string(),
        confidence: to_confidence(&[], "likely"),
        primary_location,
        evidence_path,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options,
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
    finding.fingerprint = Some(fp.fingerprint_of(&finding));
    finding
}

/// `toSourceAnchor(fact.ioAnchor, routine.id)` — resolve the IO contract anchor.
fn io_source_anchor(fact: &OrderingFact, routine: &ModelRoutine) -> Option<SourceAnchor> {
    crate::engine::l5::ordering_facts::to_source_anchor(Some(&fact.io_anchor), &routine.id)
}

/// `toSourceAnchor(opt, routine.id)` for write/commit contracts.
fn to_src(
    contract: Option<&crate::engine::l5::digest::ProjectedEvidence>,
    routine: &ModelRoutine,
) -> Option<SourceAnchor> {
    crate::engine::l5::ordering_facts::to_source_anchor(contract, &routine.id)
}

/// The ioId is the 3rd pipe-delimited segment (index 2) of `fact.key`.
fn io_id_from_key(key: &str) -> &str {
    key.split('|').nth(2).unwrap_or("")
}

pub fn detect_d47(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp = &ctx.fingerprint_index;
    let ordering_facts = ctx.get_ordering_facts();

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;

    for routine in &ws.routines {
        if !is_reportable_routine(routine) {
            continue;
        }
        let Some(of) = ordering_facts.get(&stable_routine_id_for_routine(routine)) else {
            continue;
        };
        candidates_considered += 1;

        // Dedupe advisory vs WRITE_PENDING on same IO occurrence.
        let mut write_pending_io_ids: HashSet<String> = HashSet::new();
        for fact in &of.facts {
            if fact.guarantee.label != "WRITE_PENDING_AT_EXTERNAL_IO" {
                continue;
            }
            let graded = grade_guarantee(&fact.guarantee, &fact.io_type, &fact.io_detail);
            if to_severity(graded.grade).is_some() {
                let io_id = io_id_from_key(&fact.key);
                if !io_id.is_empty() {
                    write_pending_io_ids.insert(io_id.to_string());
                }
            }
        }

        for fact in &of.facts {
            // D47 owns ONLY the external-IO labels.
            if fact.guarantee.label != "WRITE_PENDING_AT_EXTERNAL_IO"
                && fact.guarantee.label != "EXTERNAL_IO_BEFORE_COMMIT"
                && fact.guarantee.label != "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN"
            {
                continue;
            }

            if fact.guarantee.label == "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN" {
                let graded = grade_guarantee(&fact.guarantee, &fact.io_type, &fact.io_detail);
                let Some(sev) = to_severity(graded.grade) else {
                    continue;
                };
                let event_ctx = build_event_hop_context(ctx, &routine.id);
                findings.push(build_d47_event_advisory_finding(
                    fp,
                    ctx,
                    routine,
                    of,
                    fact,
                    sev,
                    event_ctx.as_ref(),
                ));
                continue;
            }

            let graded = grade_guarantee(&fact.guarantee, &fact.io_type, &fact.io_detail);
            let Some(sev) = to_severity(graded.grade) else {
                continue;
            };
            // Suppress advisory when a WRITE_PENDING finding already covers this IO occ.
            if fact.guarantee.label == "EXTERNAL_IO_BEFORE_COMMIT" && sev == "info" {
                let io_id = io_id_from_key(&fact.key);
                if !io_id.is_empty() && write_pending_io_ids.contains(io_id) {
                    continue;
                }
            }
            findings.push(build_d47_finding(fp, routine, of, fact, sev));
        }
    }

    // Sort + dedupe by id.
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: HashSet<String> = HashSet::new();
    let mut emitted: Vec<Finding> = Vec::new();
    for f in findings {
        if seen.insert(f.id.clone()) {
            emitted.push(f);
        }
    }

    let count = emitted.len();
    Ok(DetectorOutput {
        findings: emitted,
        stats: DetectorStats::new(DETECTOR, candidates_considered, count),
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
