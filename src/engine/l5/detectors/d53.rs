//! D53 — Ignored `[TryFunction]` return value. BCQuality: a TryFunction called
//! in STATEMENT position discards its implicit Boolean — the error it caught is
//! silently swallowed and execution continues on a failed step.
//!
//! Join: statement-position call site (Task-1 `in_statement_position`) whose
//! RESOLVED callee carries `[TryFunction]`. Skips: `asserterror` scopes
//! (deliberate negative-path assertions); a callee that is ALSO consumed
//! (result read) elsewhere in the same routine — a deliberate best-effort
//! fallback (`if not TryX(a) then TryX(b);`), not an accidental swallow.
//! Unresolved callees skip (fail-quiet, advisory precision-first). Inert on the
//! cross-app context (`resolved_call_edge_by_callsite` is empty there).
//!
//! Severity: high. Confidence: likely.

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::has_attribute;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d53-ignored-tryfunction-result";

pub fn detect_d53(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_asserterror = 0u64;
    let mut skipped_result_consumed = 0u64;
    let mut skipped_sibling_consumed = 0u64;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        // Callee ids whose result IS consumed somewhere in THIS routine (an
        // expression-position call: if-condition, assignment RHS, argument). A
        // statement-position call to the SAME try is then a deliberate
        // best-effort fallback — `try X; on failure retry X with different args
        // and accept the result` — not an accidental swallow. Skip it.
        let consumed_callees: std::collections::HashSet<&str> = routine
            .call_sites
            .iter()
            .filter(|cs| !cs.in_statement_position)
            .filter_map(|cs| ctx.resolved_call_edge_by_callsite.get(&cs.id))
            .filter_map(|edge| edge.to.as_deref())
            .collect();

        for cs in &routine.call_sites {
            let Some(edge) = ctx.resolved_call_edge_by_callsite.get(&cs.id) else {
                continue;
            };
            let Some(to) = edge.to.as_deref() else {
                continue;
            };
            let Some(callee) = ctx.routine_by_id.get(to) else {
                continue;
            };
            if !has_attribute(&callee.attributes_parsed, "TryFunction") {
                continue;
            }
            candidates_considered += 1;
            if !cs.in_statement_position {
                skipped_result_consumed += 1;
                continue;
            }
            if cs.under_asserterror == Some(true) {
                skipped_asserterror += 1;
                continue;
            }
            if consumed_callees.contains(to) {
                skipped_sibling_consumed += 1;
                continue;
            }

            let confidence: FindingConfidence = to_confidence(&[], "likely");
            let id = format!("d53/{}/{}", routine.id, cs.id);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: "Ignored TryFunction result".into(),
                root_cause: format!(
                    "{} calls the TryFunction {} in statement position — the Boolean result \
                     is discarded, so a caught error is silently swallowed and execution \
                     continues past the failed step.",
                    routine.name, callee.name
                ),
                severity: "high".to_string(),
                confidence,
                primary_location: anchor_of(&cs.source_anchor, routine),
                evidence_path: vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&cs.source_anchor, routine),
                        note: format!("statement-position call to [TryFunction] {}", callee.name),
                    },
                    EvidenceStep {
                        routine_id: callee.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&callee.source_anchor, callee),
                        note: format!("[TryFunction] {}", callee.name),
                    },
                ],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: format!(
                        "Consume the result: `if not {}(...) then` handle/surface the failure \
                         (GetLastErrorText), or drop the [TryFunction] attribute if errors \
                         must propagate.",
                        callee.name
                    )
                    .into(),
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
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("resultConsumed", skipped_result_consumed);
    stats.add_skip("asserterror", skipped_asserterror);
    stats.add_skip("siblingConsumed", skipped_sibling_consumed);
    Ok(DetectorOutput::no_diag(findings, stats))
}
