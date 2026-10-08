//! D51 — opt-in advisory: external write-direction IO before an escaping error.
//! Port of al-sem `src/detectors/d51-retry-side-effect-duplication.ts`.
//!
//! Consumes the L4.5 ordering facts; filters to IO_BEFORE_ESCAPING_ERROR, grades
//! via `grade_guarantee` (validated + no proven-effective commit → low; validated +
//! proven-effective commit → medium; !validForRefutation → suppressed). A
//! config-asserted `job-queue-entrypoint` root raises confidence to "confirmed" and
//! changes the rootCause wording. OPT-IN (registered but filtered by the differential).

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, SourceAnchor};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::ordering_facts::{
    OrderingFact, OrderingFacts, TxnFact, grade_guarantee, is_reportable_routine,
    stable_routine_id_for_routine, to_severity,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d51-retry-side-effect-duplication";

fn detail_method(detail: &[(String, String)]) -> Option<&str> {
    detail
        .iter()
        .find(|(k, _)| k == "method")
        .map(|(_, v)| v.as_str())
}

fn to_src(
    contract: Option<&crate::engine::l5::digest::ProjectedEvidence>,
    routine: &ModelRoutine,
) -> Option<SourceAnchor> {
    crate::engine::l5::ordering_facts::to_source_anchor(contract, &routine.id)
}

/// True when the routine is asserted a `job-queue-entrypoint` root.
fn is_retryable_entrypoint(ctx: &DetectorContext, routine: &ModelRoutine) -> bool {
    ctx.root_classifications_by_routine
        .get(&routine.id)
        .map(|rc| rc.kinds.iter().any(|k| k == "job-queue-entrypoint"))
        .unwrap_or(false)
}

fn build_d51_finding(
    fp: &FingerprintIndex,
    routine: &ModelRoutine,
    of: &OrderingFacts,
    fact: &OrderingFact,
    sev: &str,
    retryable: bool,
) -> Finding {
    let io_anchor = to_src(Some(&fact.io_anchor), routine);

    let method = detail_method(&fact.io_detail);
    let io_label = match method {
        Some(m) => format!("{} {}", fact.io_type, m),
        None => "write-direction".to_string(),
    };
    let request_phrase = match method {
        Some(m) => format!("an {} {} request", fact.io_type, m),
        None => "a write-direction external request".to_string(),
    };

    let mut evidence_path: Vec<EvidenceStep> = Vec::new();
    evidence_path.push(EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: io_anchor
            .clone()
            .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine)),
        note: format!(
            "{io_label} request — if this routine is retried the request may be re-issued"
        ),
    });
    let commit_anchor = to_src(fact.commit_anchor.as_ref(), routine);
    if let Some(ca) = &commit_anchor {
        evidence_path.push(EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: ca.clone(),
            note: "commit between request and error — partial state may persist across retry"
                .to_string(),
        });
    }

    let primary_location = io_anchor
        .clone()
        .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine));

    let title = "External request may be duplicated on retry".to_string();

    let mut root_cause = if retryable {
        format!(
            "{} is configured as a job-queue (retryable) entrypoint and issues {request_phrase} \
             that can then raise an uncaught error on the same path. When the platform retries a \
             failed attempt, the request path is reached again; unless the endpoint is idempotent \
             or guarded by durable state, the external side effect may be duplicated.",
            routine.name
        )
    } else {
        format!(
            "{} issues {request_phrase} and can then raise an uncaught error on the same path. If \
             this routine is retried (e.g. as a job-queue entry, up to its configured attempts), \
             the request path may be reached again; unless the endpoint is idempotent or guarded \
             by durable state, the external side effect may be duplicated.",
            routine.name
        )
    };

    if sev == "medium" {
        root_cause.push_str(
            " A committed write between the request and the error may persist partial state \
             across the retry.",
        );
    }

    let mut finding = Finding {
        id: format!("d51/{}/{}", of.routine_id, fact.key),
        root_cause_key: format!("d51/{}", of.routine_id),
        detector: DETECTOR.to_string(),
        title: title.into(),
        root_cause,
        severity: sev.to_string(),
        confidence: to_confidence(&[], if retryable { "confirmed" } else { "likely" }),
        primary_location,
        evidence_path,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: vec![FixOption {
            description:
                "Guard the external call with a durable idempotency key stored in the database \
                 before the call, or ensure the external endpoint is idempotent. Alternatively, \
                 restructure the routine so errors are raised before the external call, or move \
                 the external call to an after-commit step."
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
    finding.fingerprint = Some(fp.fingerprint_of(&finding));
    finding
}

pub fn detect_d51(
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

        for fact in &of.facts {
            if fact.guarantee.label != "IO_BEFORE_ESCAPING_ERROR" {
                continue;
            }
            let graded = grade_guarantee(&fact.guarantee, &fact.io_type, &fact.io_detail);
            if graded.txn_fact != TxnFact::IoBeforeEscapingError {
                continue;
            }
            let Some(sev) = to_severity(graded.grade) else {
                continue;
            };
            findings.push(build_d51_finding(
                fp,
                routine,
                of,
                fact,
                sev,
                is_retryable_entrypoint(ctx, routine),
            ));
        }
    }

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
