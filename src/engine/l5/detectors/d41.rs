//! D41 — transitive filter loss. Port of al-sem
//! `src/detectors/d41-transitive-filter-loss.ts`.
//!
//! Predicate (all four must hold):
//!  1. Caller called SetRange / SetFilter on R before a callsite forwarding R by-var;
//!  2. Callee's `parameterRoles[Q].resetsFiltersOnParam === "yes"`;
//!  3. Caller performs a filter-sensitive op on R AFTER the callsite;
//!  4. Caller did NOT re-filter R between the callsite and that sensitive op.
//!
//! Severity high. Anchor: caller's argumentAnchor.
//!
//! Reads the CORE `RoutineSummary.parameterRoles` via `ctx.parameter_roles_by_routine`
//! and `ctx.resolved_call_edge_by_callsite`. The post-upgrade `bindingResolution` /
//! `calleeParameterIsVar` live on `ctx.upgraded_bindings_by_callsite`, joined
//! POSITIONALLY with `cs.argument_bindings` by index.

use crate::engine::l4::effect_lattice::EffectPresence;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::calls::UpgradedBinding;
use crate::program::model::workspace::{Model, ModelRecordOperation};

const DETECTOR: &str = "d41-transitive-filter-loss";

const FILTER_SET_OPS: &[&str] = &["SetRange", "SetFilter"];
const FILTER_SENSITIVE_OPS: &[&str] = &[
    "FindFirst",
    "FindLast",
    "FindSet",
    "Find",
    "Next",
    "CalcSums",
    "DeleteAll",
    "ModifyAll",
    "Count",
    "IsEmpty",
];

pub fn detect_d41(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_no_prior_filter = 0u64;
    let mut skipped_no_post_use = 0u64;
    let mut skipped_re_filtered = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only ⇒ all primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }

        for cs in &routine.call_sites {
            let edge = match ctx.resolved_call_edge_by_callsite.get(&cs.id) {
                Some(e) => e,
                None => continue,
            };
            let to = match &edge.to {
                Some(t) => t,
                None => continue,
            };
            let callee = match ctx.routine_by_id.get(to.as_str()) {
                Some(c) => *c,
                None => continue,
            };

            let upgraded = ctx.upgraded_bindings_by_callsite.get(&cs.id);
            for (i, binding) in cs.argument_bindings.iter().enumerate() {
                let up: Option<&UpgradedBinding> = upgraded.and_then(|u| u.get(i));
                if up.map(|u| u.binding_resolution.as_str()) != Some("resolved") {
                    continue;
                }
                if !up.map(|u| u.callee_parameter_is_var).unwrap_or(false) {
                    continue;
                }

                let callee_role =
                    ctx.parameter_roles_by_routine
                        .get(&callee.id)
                        .and_then(|roles| {
                            roles
                                .iter()
                                .find(|r| r.parameter_index == binding.parameter_index)
                        });
                if callee_role.map(|r| r.resets_filters_on_param) != Some(EffectPresence::Yes) {
                    continue;
                }

                let source_name_lc = match &binding.source_variable_name {
                    Some(n) => n.clone(),
                    None => continue,
                };
                candidates_considered += 1;

                // Ops on this variable, in source order (record_operations preserves it).
                let ops_on_var: Vec<&ModelRecordOperation> = routine
                    .record_operations
                    .iter()
                    .filter(|op| op.record_variable_name.to_lowercase() == source_name_lc)
                    .collect();

                // (1) Was a filter ACTIVE on R at the callsite? Net-effect scan
                // in source order (mirrors d33 `was_filtered_before`):
                // SetRange/SetFilter → filtered, Reset → cleared, last-wins. A
                // filter the caller itself cleared with its OWN Reset before the
                // call is not "lost across the helper" — the callee's Reset wipes
                // nothing. Using the naive first-prior-SetRange (ignoring an
                // intervening Reset) fires when no filter was active (Gap-T FP).
                // The witness is the LAST filter-set op still in effect.
                let mut filtered = false;
                let mut witness_filter: Option<&ModelRecordOperation> = None;
                for op in &ops_on_var {
                    if !before_anchor(&op.source_anchor, &cs.source_anchor) {
                        continue;
                    }
                    if FILTER_SET_OPS.contains(&op.op.as_str()) {
                        filtered = true;
                        witness_filter = Some(*op);
                    } else if op.op == "Reset" {
                        filtered = false;
                        witness_filter = None;
                    }
                }
                let first_prior_filter = match (filtered, witness_filter) {
                    (true, Some(op)) => op,
                    _ => {
                        skipped_no_prior_filter += 1;
                        continue;
                    }
                };

                // (3) Any filter-sensitive op AFTER the callsite?
                let first_sensitive = ops_on_var.iter().find(|op| {
                    FILTER_SENSITIVE_OPS.contains(&op.op.as_str())
                        && before_anchor(&cs.source_anchor, &op.source_anchor)
                });
                let first_sensitive = match first_sensitive {
                    Some(op) => *op,
                    None => {
                        skipped_no_post_use += 1;
                        continue;
                    }
                };

                // (4) Re-filter between callsite and the sensitive op?
                let re_filtered = ops_on_var.iter().any(|op| {
                    FILTER_SET_OPS.contains(&op.op.as_str())
                        && before_anchor(&cs.source_anchor, &op.source_anchor)
                        && before_anchor(&op.source_anchor, &first_sensitive.source_anchor)
                });
                if re_filtered {
                    skipped_re_filtered += 1;
                    continue;
                }

                let path = vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(first_prior_filter.id.clone()),
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&first_prior_filter.source_anchor, routine),
                        note: format!("{} on {}", first_prior_filter.op, source_name_lc),
                    },
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&binding.argument_anchor, routine),
                        note: format!(
                            "forwards {} to {}, which calls Reset",
                            source_name_lc, callee.name
                        ),
                    },
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(first_sensitive.id.clone()),
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&first_sensitive.source_anchor, routine),
                        note: format!(
                            "{} on {} — operates on the now-unfiltered set",
                            first_sensitive.op, source_name_lc
                        ),
                    },
                ];

                let id = format!("d41/{}/{}/{}", routine.id, cs.id, binding.parameter_index);
                let mut affected_objects =
                    vec![routine.object_id.clone(), callee.object_id.clone()];
                affected_objects.sort();

                let mut finding = Finding {
                    id: id.clone(),
                    root_cause_key: id,
                    detector: DETECTOR.to_string(),
                    title: "Filter silently lost across helper call".into(),
                    root_cause: format!(
                        "{} filters {} before calling {}, which calls Reset; the subsequent {} operates on the unfiltered set.",
                        routine.name, source_name_lc, callee.name, first_sensitive.op
                    ),
                    severity: "high".to_string(),
                    confidence: to_confidence(&[], "likely"),
                    primary_location: anchor_of(&binding.argument_anchor, routine),
                    evidence_path: path,
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Re-apply the SetRange/SetFilter on {} after the call to {}, or restructure to avoid the call inside the filtered scope.",
                            source_name_lc, callee.name
                        ).into(),
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
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("noPriorFilter", skipped_no_prior_filter);
    stats.add_skip("noPostUse", skipped_no_post_use);
    stats.add_skip("reFiltered", skipped_re_filtered);
    Ok(DetectorOutput::no_diag(findings, stats))
}
