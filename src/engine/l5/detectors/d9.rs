//! D9 — Transaction span summary. Port of al-sem
//! `src/detectors/d9-transaction-span-summary.ts`.
//!
//! For each non-trivial ExplicitCommit transaction span (≥2 routines AND (≥2 PHYSICAL tables
//! written before the Commit OR !coverage_complete)), emit an info-level finding describing
//! what the transaction holds when it commits. The tables and events are the span's PENDING
//! ones (engine-switch S8 gap 3, `pending_writes`), not everything the span can reach.
//! Aimed at code review / agent context, not a bug to fix.
//!
//! Within-detector sort by `a.id.cmp(&b.id)` (byte order).

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::transaction_spans::SeedKind;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d9-transaction-span-summary";
const MIN_INTERESTING_ROUTINES: usize = 2;
const MIN_INTERESTING_TABLES: usize = 2;

pub fn detect_d9(
    _resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;

    for span in &ctx.transaction_spans {
        // §B: checked-run-implicit seeds are for D50 only — D9 ignores them.
        if span.seed_kind != SeedKind::ExplicitCommit {
            continue;
        }
        let Some(commit_routine) = ctx.routine_by_id.get(span.commit_routine_id.as_str()) else {
            continue;
        };
        // roleOf(routine) === "primary": source-only ⇒ every routine is primary.
        candidates_considered += 1;

        if span.routines_in_span.len() < MIN_INTERESTING_ROUTINES {
            skipped_other += 1;
            continue;
        }

        // ⟨issue 23 rule⟩ The gate and the count read PHYSICAL tables only:
        // writes to temporary records are not part of any transaction. Only the
        // writes pending at the Commit are this transaction's (S8 gap 3).
        let table_count = span.pending_physical_tables.len();
        let effects_are_interesting =
            table_count >= MIN_INTERESTING_TABLES || !span.coverage_complete;
        if !effects_are_interesting {
            skipped_other += 1;
            continue;
        }

        // Single evidence step: commit routine with operationId.
        let commit_anchor = anchor_of(&commit_routine.source_anchor, commit_routine);
        let path = vec![EvidenceStep {
            routine_id: commit_routine.id.clone(),
            operation_id: Some(span.commit_operation_id.clone()),
            callsite_id: None,
            loop_id: None,
            source_anchor: commit_anchor.clone(),
            note: "Commit at end of span".to_string(),
        }];

        // tableDesc: "writes {n} known table(s)" if n>0 else ("writes tables (effect scope unknown)"
        // if !coverage_complete else "writes tables").
        let table_desc = if table_count > 0 {
            format!("writes {table_count} known table(s) before it")
        } else if !span.coverage_complete {
            "writes tables (effect scope unknown)".to_string()
        } else {
            "writes tables".to_string()
        };

        let confidence: FindingConfidence = to_confidence(&[], "possible");
        let id = format!("d9/{}", span.commit_operation_id);
        let root_cause_key = id.clone();

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: "Transaction span summary".into(),
            root_cause: format!(
                "Transaction ending at {}'s Commit spans {} routines, {}, publishes {} event(s) \
                 before it. Consider whether all of this needs to be atomic.",
                commit_routine.name,
                span.routines_in_span.len(),
                table_desc,
                span.pending_events.len()
            ),
            severity: "info".to_string(),
            confidence,
            primary_location: commit_anchor,
            evidence_path: path,
            additional_paths: None,
            affected_objects: vec![commit_routine.object_id.as_str().into()],
            affected_tables: id_list(span.pending_physical_tables.clone()),
            fix_options: vec![FixOption {
                description: "If the span includes operations that are logically independent, \
                              split them into separate transactions with their own Commit boundaries."
                    .to_string().into(),
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
