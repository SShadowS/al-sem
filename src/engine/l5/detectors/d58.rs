//! D58 — Query filter set after `Open()`. BCQuality `set-query-filters-before-open`:
//! the running dataset snapshots filters at Open; a later SetFilter/SetRange is
//! silently ignored until the next Open. `Close()` re-arms filtering.
//!
//! Intraprocedural, straight-line source order (branching ignored — the same
//! convention d33's filter scan uses). Per query-typed variable, walk its
//! member calls in anchor order tracking open-state; flag each
//! SetFilter/SetRange while open. Severity: medium. Confidence: likely.

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PCallee;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d58-query-filter-after-open";

pub fn detect_d58(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_before_open = 0u64;

    for routine in &ws.routines {
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        // Query-typed variables (params/locals/globals).
        let query_vars: Vec<String> = routine
            .variables
            .iter()
            .filter(|v| {
                v.declared_type
                    .trim_start()
                    .to_lowercase()
                    .starts_with("query")
            })
            .map(|v| v.name.to_lowercase())
            .collect();
        if query_vars.is_empty() {
            continue;
        }

        for qv in &query_vars {
            // (anchor-ordered) events on this receiver: Open / Close / SetFilter/SetRange.
            let mut events: Vec<(&crate::program::body::features::PCallSite, &'static str)> =
                Vec::new();
            for cs in &routine.call_sites {
                let PCallee::Member { receiver, method } = &cs.callee else {
                    continue;
                };
                if receiver.to_lowercase() != *qv {
                    continue;
                }
                let ev = if method.eq_fold_identifier("Open") {
                    "open"
                } else if method.eq_fold_identifier("Close") {
                    "close"
                } else if method.eq_fold_identifier("SetFilter")
                    || method.eq_fold_identifier("SetRange")
                {
                    "filter"
                } else {
                    continue;
                };
                events.push((cs, ev));
            }
            events.sort_by_key(|(cs, _)| {
                (cs.source_anchor.start_line, cs.source_anchor.start_column)
            });

            let mut is_open = false;
            for (cs, ev) in events {
                match ev {
                    "open" => is_open = true,
                    "close" => is_open = false,
                    "filter" => {
                        candidates_considered += 1;
                        if !is_open {
                            skipped_before_open += 1;
                            continue;
                        }
                        let confidence: FindingConfidence = to_confidence(&[], "likely");
                        let id = format!("d58/{}/{}", routine.id, cs.id);
                        let mut finding = Finding {
                            id: id.clone(),
                            root_cause_key: id,
                            detector: DETECTOR.to_string(),
                            title: "Query filter set after Open".into(),
                            root_cause: format!(
                                "{} sets a filter on the query variable {} AFTER Open() — \
                                 the open dataset ignores it; the filter only applies on \
                                 the next Open.",
                                routine.name, cs.callee_text
                            ),
                            severity: "medium".to_string(),
                            confidence,
                            primary_location: anchor_of(&cs.source_anchor, routine),
                            evidence_path: vec![EvidenceStep {
                                routine_id: routine.id.clone(),
                                operation_id: None,
                                callsite_id: Some(cs.id.clone()),
                                loop_id: None,
                                source_anchor: anchor_of(&cs.source_anchor, routine),
                                note: "filter after Open (ignored by the open dataset)".to_string(),
                            }],
                            additional_paths: None,
                            affected_objects: vec![routine.object_id.as_str().into()],
                            affected_tables: Vec::new(),
                            fix_options: vec![FixOption {
                                description: "Move the SetFilter/SetRange before Open(), or \
                                              Close() and re-Open() after changing filters."
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
                    _ => {}
                }
            }
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("filterBeforeOpen", skipped_before_open);
    Ok(DetectorOutput::no_diag(findings, stats))
}
