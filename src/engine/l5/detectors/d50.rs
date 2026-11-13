//! D50 — opt-in. A checked `Codeunit.Run` (its Boolean result used) at which
//! a database write may be pending.
//!
//! **ENG-22 (meaning changed).** d50 used to report "a checked Codeunit.Run
//! performs an implicit commit on successful return" inside a posting span,
//! as if the run committed the caller's writes. BC does the opposite:
//! measured on the BC 28.4 Linux sidecar and confirmed on Windows BC
//! 28.0.54946.56477 (owner, q-20261112T021007-384a506a), a checked run with
//! a write pending raises "An error occurred and the transaction is
//! stopped." at the run itself, which the checked form does not catch (the
//! run does not return `false`), and nothing of the caller's is committed
//! (probe: docs/superpowers/notes/2026-11-11-eng09-checked-run-probe.md).
//! Microsoft Learn ("Codeunit.Run(Integer [, var Record]) Method"): "If
//! you're already in a transaction you must commit first before calling
//! Codeunit.Run". The owner kept the id and changed the finding to that
//! error (q-20261112T025612-62a3ca62); the finding id carries
//! `pending-write`, so a suppression or baseline of the old claim does not
//! carry over.
//!
//! Fires per checked-run seed of `transaction_spans` whose span leaves a
//! physical table write pending at the run
//! (`TransactionSpan::pending_physical_tables`, read in statement order by
//! [`crate::engine::l5::pending_writes`]). A routine-level approximation:
//! the span's callers are every caller in the transaction, not one route.
//! `prove … throws-error` rules the same error per route
//! (`LiteralReach::run_error_verdict`).

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::transaction_spans::SeedKind;
use crate::program::model::workspace::{Model, ModelRoutine};

pub(crate) const DETECTOR: &str = "d50-checked-run-implicit-commit";

/// Build a `SourceAnchor` from a routine's `PAnchor`.
fn routine_anchor(r: &ModelRoutine) -> SourceAnchor {
    anchor_of(&r.source_anchor, r)
}

pub fn detect_d50(
    _resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_clean = 0u64;

    for span in &ctx.transaction_spans {
        if span.seed_kind != SeedKind::CheckedRunImplicit {
            continue;
        }
        let Some(seed_routine) = ctx.routine_by_id.get(span.commit_routine_id.as_str()) else {
            continue;
        };
        candidates_considered += 1;
        // ENG-24: a write with no physical table to name (a LockTable, a write
        // whose table does not resolve) is pending too.
        if span.pending_physical_tables.is_empty() && span.pending_other.is_empty() {
            skipped_clean += 1;
            continue;
        }

        // Anchor: the run's call site when known, else the routine header.
        let callsite_anchor: SourceAnchor = span
            .seed_callsite_id
            .as_deref()
            .and_then(|cs_id| ctx.call_site_by_id.get(cs_id))
            .map_or_else(
                || routine_anchor(seed_routine),
                |cs| SourceAnchor {
                    source_unit_id: cs.source_anchor.source_unit_id.clone(),
                    start_line: cs.source_anchor.start_line,
                    start_column: cs.source_anchor.start_column,
                    end_line: cs.source_anchor.end_line,
                    end_column: cs.source_anchor.end_column,
                    enclosing_routine_id: seed_routine.id.clone(),
                    syntax_kind: cs.source_anchor.syntax_kind.clone(),
                    normalized_text_hash: None,
                    leading_context_hash: None,
                    trailing_context_hash: None,
                },
            );

        // The writer: a span member that writes before its call toward the
        // run (the first by id), else the run's own routine.
        let writer = span
            .pending_physical_by_routine
            .iter()
            .find(|&(_, &n)| n > 0)
            .and_then(|(id, _)| ctx.routine_by_id.get(id.as_str()))
            .copied()
            .unwrap_or(seed_routine);
        let tables = span
            .pending_physical_tables
            .iter()
            .map(|t| {
                ctx.table_by_id
                    .get(t.as_str())
                    .map_or(t.as_str(), |m| m.name.as_str())
            })
            .chain(span.pending_other.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(", ");
        let mut evidence_path = Vec::new();
        if writer.id != seed_routine.id {
            evidence_path.push(EvidenceStep {
                routine_id: writer.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: routine_anchor(writer),
                note: format!(
                    "{} may write before the run, and no Commit() ends it",
                    writer.name
                ),
            });
        }
        evidence_path.push(EvidenceStep {
            routine_id: seed_routine.id.clone(),
            operation_id: None,
            callsite_id: span.seed_callsite_id.clone(),
            loop_id: None,
            source_anchor: callsite_anchor.clone(),
            note: format!(
                "checked Codeunit.Run in {} with a write that may be pending ({tables}): \
                 BC raises an error at the run",
                seed_routine.name
            ),
        });

        let root_cause = format!(
            "A Codeunit.Run whose Boolean result is used in {} runs while a database write may be \
             pending ({tables}). A checked run needs a clean transaction: with a write pending, BC \
             raises \"An error occurred and the transaction is stopped.\" at the run itself, the \
             checked form does not catch it (the run does not return false), and nothing of the \
             caller's is committed (measured on BC 28). Routine-level approximation: the writes \
             may be pending on some callers' routes only.",
            seed_routine.name
        );

        let mut affected_objects = vec![seed_routine.object_id.clone(), writer.object_id.clone()];
        affected_objects.sort();
        affected_objects.dedup();

        // ENG-22: a new id (and so a new fingerprint) for the new meaning.
        let id = format!("d50/pending-write/{}", span.commit_operation_id);
        let mut finding = Finding {
            id: id.clone(),
            root_cause_key: id,
            detector: DETECTOR.to_string(),
            title: "Checked Codeunit.Run with a write pending raises an error".into(),
            root_cause,
            severity: "medium".into(),
            confidence: to_confidence(&[], "possible"),
            primary_location: callsite_anchor,
            evidence_path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: id_list(span.pending_physical_tables.clone()),
            fix_options: vec![FixOption {
                description: "Commit() before the checked run when the writes before it may be \
                              committed, or move the writes after the run."
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
            loose_subject: None,
        };
        finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
        findings.push(finding);
    }

    // Dedupe by id (same checked-run call site → one finding).
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: HashSet<String> = HashSet::new();
    let emitted: Vec<Finding> = findings
        .into_iter()
        .filter(|f| seen.insert(f.id.clone()))
        .collect();

    let count = emitted.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, count);
    stats.add_skip("no-write-pending", skipped_clean);
    Ok(DetectorOutput {
        findings: emitted,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::combined_graph::PrimaryGraph;
    use crate::engine::l5::detector_context::{DetectorArtifacts, detector_context};
    use crate::engine::l5::test_support::{empty_artifacts, routine};
    use crate::engine::l5::transaction_spans::TransactionSpan;
    use crate::program::model::workspace::ModelEntities;

    fn span(seed: &str, pending: &[&str]) -> TransactionSpan {
        TransactionSpan {
            seed_kind: SeedKind::CheckedRunImplicit,
            commit_operation_id: format!("{seed}/cs0"),
            seed_callsite_id: Some(format!("{seed}/cs0")),
            commit_routine_id: seed.to_string(),
            routines_in_span: vec![seed.to_string()],
            writes_tables: pending.iter().map(|s| s.to_string()).collect(),
            writes_physical_tables_count: pending.len(),
            publishes_events: vec![],
            span_roots: vec![seed.to_string()],
            coverage_complete: true,
            pending_physical_tables: pending.iter().map(|s| s.to_string()).collect(),
            pending_other: vec![],
            pending_events: vec![],
            pending_physical_by_routine: Default::default(),
        }
    }

    /// ENG-22: d50 fires on a checked run at which a write may be pending,
    /// with the error's meaning and a `pending-write` id; a run with no
    /// write pending (the old claim's other case) is no finding.
    ///
    /// Break: drop the `pending_physical_tables.is_empty()` skip → "Clean"
    /// is reported too.
    #[test]
    fn fires_only_where_a_write_may_be_pending() {
        let routines = vec![routine("dirty", "procedure"), routine("clean", "procedure")];
        let art = DetectorArtifacts {
            transaction_spans: vec![span("dirty", &["t/A"]), span("clean", &[])],
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
        let out = detect_d50(&resolved, &ctx).unwrap();
        assert_eq!(out.findings.len(), 1, "{:#?}", out.findings);
        let f = &out.findings[0];
        assert_eq!(f.id, "d50/pending-write/dirty/cs0");
        assert!(
            f.root_cause
                .contains("An error occurred and the transaction is stopped.")
        );
        assert!(!f.root_cause.contains("implicit commit"));
    }
}
