//! D49 — uncommitted write before a window-opening UI call.
//! Port of al-sem `src/detectors/d49-uncommitted-write-before-ui.ts`.
//!
//! Consumes the L4.5 ordering facts, grades each WRITE_PENDING_AT_UI fact via
//! `grade_guarantee`, emits a `high` finding for non-none/suppressed grades.
//! Witness path: write anchor → UI-sink anchor. Sort by `compareStrings(id)`,
//! dedup by id.

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

const DETECTOR: &str = "d49-uncommitted-write-before-ui";

fn to_src(
    contract: Option<&crate::engine::l5::digest::ProjectedEvidence>,
    routine: &ModelRoutine,
) -> Option<SourceAnchor> {
    crate::engine::l5::ordering_facts::to_source_anchor(contract, &routine.id)
}

fn build_d49_finding(
    fp: &FingerprintIndex,
    routine: &ModelRoutine,
    of: &OrderingFacts,
    fact: &OrderingFact,
    sev: &str,
) -> Finding {
    let ui_anchor = to_src(Some(&fact.io_anchor), routine);
    let write_anchor = to_src(fact.write_anchor.as_ref(), routine);

    let ui_label = &fact.io_type;

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
        source_anchor: ui_anchor
            .clone()
            .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine)),
        note: format!("{ui_label} call inside open write transaction (window-opening UI)"),
    });

    let primary_location = ui_anchor
        .clone()
        .unwrap_or_else(|| anchor_of(&routine.source_anchor, routine));

    let title = "Uncommitted write before window-opening UI".to_string();

    let root_cause = format!(
        "{} opens a window-opening UI ({ui_label}) while a database write is still pending \
         (uncommitted). BC's runtime throws \"you cannot open a window after modifying the \
         database\" — the write transaction must be committed or rolled back before opening a \
         window.",
        routine.name
    );

    let fix_options = vec![FixOption {
        description:
            "Add a Commit() call before the window-opening UI call, or restructure the code so \
             the UI interaction happens outside the write transaction. Alternatively, consider \
             whether the write can be deferred until after the user interaction."
                .into(),
        safety: "medium".into(),
    }];

    let mut finding = Finding {
        id: format!("d49/{}/{}", of.routine_id, fact.key),
        root_cause_key: format!("d49/{}", of.routine_id),
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

pub fn detect_d49(
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
            if fact.guarantee.label != "WRITE_PENDING_AT_UI" {
                continue;
            }
            let graded = grade_guarantee(&fact.guarantee, &fact.io_type, &fact.io_detail);
            let Some(sev) = to_severity(graded.grade) else {
                continue;
            };
            findings.push(build_d49_finding(fp, routine, of, fact, sev));
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
