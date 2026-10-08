//! D40 — transitive load missing. Port of al-sem
//! `src/detectors/d40-transitive-load-missing.ts`.
//!
//! For each resolved call edge where the callee requires its parameter loaded at
//! entry (`callee.parameterRoles[Q].requiresLoadedAtEntry === "yes"`), verify the
//! caller has loaded the forwarded record before the callsite; otherwise emit at
//! the caller's callsite.
//!
//! "Loaded before the callsite" is lexical and lenient (any earlier load, even on
//! one branch): the routine's own Get/Find*/Next/Init/Copy/TransferFields on the
//! record, or an earlier call that hands the record `var` to a helper whose
//! parameter role loads it (`record_load_points`, shared with d39). A routine
//! that forwards its own parameter is not blamed when that parameter's role
//! requires it loaded at entry AND its callers are visible (`owner_is_judged`):
//! the caller owns the load, and this detector judges the caller through that
//! role instead. A public routine with no workspace caller keeps the finding.
//!
//! Severity `medium`, escalating to `high` when the callee mutates the unloaded
//! record (`mutatesBeforeLoad === "yes"`).
//!
//! OPT-IN: al-sem keeps D40 out of the default registry (the straight-line walker
//! over-flags loop-loaded records). The Rust port registers it; `project_r4_findings`
//! filters by name, so it only contributes when explicitly requested.
//!
//! Reads the CORE `RoutineSummary.parameterRoles` via `ctx.parameter_roles_by_routine`
//! and the resolved per-callsite edge via `ctx.resolved_call_edge_by_callsite`. Joins
//! to the L3 source-side bindings (`cs.argument_bindings`) — the source-side fields
//! (sourceKind / sourceTempState / sourceVariableName / sourceRecordVariableId) live
//! on the L3 binding directly; the post-upgrade `bindingResolution` lives on
//! `ctx.upgraded_bindings_by_callsite` joined POSITIONALLY by index.

use crate::engine::l4::effect_lattice::EffectPresence;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor, owner_is_judged, record_load_points};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d40-transitive-load-missing";

pub fn detect_d40(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_unresolved = 0u64;
    let mut skipped_implicit_rec = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_caller_loaded = 0u64;
    let mut skipped_callee_unknown = 0u64;
    let mut skipped_caller_owns_load = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only ⇒ all primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }

        // Every point where a source variable becomes loaded (own load ops and
        // `var` helpers whose role loads it — the shared definition).
        let loads_by_source_lc = record_load_points(routine, ctx);

        for cs in &routine.call_sites {
            let edge = match ctx.resolved_call_edge_by_callsite.get(&cs.id) {
                Some(e) => e,
                None => {
                    skipped_unresolved += 1;
                    continue;
                }
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
                // C2(a): implicit-rec narrowing — checked BEFORE bindingResolution.
                if binding.source_kind == "implicit-rec" {
                    skipped_implicit_rec += 1;
                    continue;
                }
                let binding_resolution = upgraded
                    .and_then(|u| u.get(i))
                    .map(|u| u.binding_resolution.as_str());
                if binding_resolution != Some("resolved") {
                    skipped_unresolved += 1;
                    continue;
                }
                // sourceTempState known/true → temp record, no DB load concept.
                if crate::program::body::features::temp_state_suppresses(
                    binding.source_temp_state.as_ref(),
                ) {
                    skipped_temp_record += 1;
                    continue;
                }
                let callee_role =
                    match ctx
                        .parameter_roles_by_routine
                        .get(&callee.id)
                        .and_then(|roles| {
                            roles
                                .iter()
                                .find(|r| r.parameter_index == binding.parameter_index)
                        }) {
                        Some(r) => r,
                        None => {
                            skipped_callee_unknown += 1;
                            continue;
                        }
                    };
                if callee_role.requires_loaded_at_entry != EffectPresence::Yes {
                    continue;
                }
                candidates_considered += 1;

                let source_name_lc = match &binding.source_variable_name {
                    Some(n) => n.clone(),
                    None => continue,
                };
                let bucket = loads_by_source_lc
                    .get(&source_name_lc)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let source_id = binding.source_record_variable_id.as_deref();
                let loaded_before = bucket.iter().any(|load| {
                    before_anchor(load.anchor, &cs.source_anchor) && load.is_variable(source_id)
                });
                if loaded_before {
                    skipped_caller_loaded += 1;
                    continue;
                }

                // The routine forwards its OWN parameter. The L4 walker composes
                // the callee's entry requirement into that parameter's role (any
                // var-ness: a by-value copy carries the caller's loaded state), so
                // when the role says "requires loaded" the record's owner is one
                // level up and this same loop judges the routine's callers — but
                // only when those callers are visible (`owner_is_judged`): a public
                // routine with no workspace caller keeps the finding.
                if binding.source_kind == "parameter"
                    && owner_is_judged(routine, ctx)
                    && let Some(src_ix) = binding.source_parameter_index
                    && ctx
                        .parameter_roles_by_routine
                        .get(&routine.id)
                        .and_then(|rs| rs.iter().find(|r| r.parameter_index == src_ix))
                        .is_some_and(|r| r.requires_loaded_at_entry == EffectPresence::Yes)
                {
                    skipped_caller_owns_load += 1;
                    continue;
                }

                let mutates = callee_role.mutates_before_load == EffectPresence::Yes;
                let severity = if mutates { "high" } else { "medium" };
                let verb = if mutates { "mutates" } else { "reads" };
                let verb_ing = if mutates { "mutating" } else { "reading" };

                let path = vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&binding.argument_anchor, routine),
                        note: format!(
                            "forwards {} to {} (param[{}])",
                            binding.source_variable_name.as_deref().unwrap_or(""),
                            callee.name,
                            binding.parameter_index
                        ),
                    },
                    EvidenceStep {
                        routine_id: callee.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&callee.source_anchor, callee),
                        note: format!("{} {} this record before loading it", callee.name, verb),
                    },
                ];

                let id = format!("d40/{}/{}/{}", routine.id, cs.id, binding.parameter_index);
                let mut affected_objects =
                    vec![routine.object_id.clone(), callee.object_id.clone()];
                affected_objects.sort();

                let mut finding = Finding {
                    id: id.clone(),
                    root_cause_key: id,
                    detector: DETECTOR.to_string(),
                    title: format!("Forwarded record not loaded before {verb_ing} helper").into(),
                    root_cause: format!(
                        "{} forwards {} to {}, which {} the record without loading it — the caller must Get/Find the record before the call.",
                        routine.name,
                        binding.source_variable_name.as_deref().unwrap_or(""),
                        callee.name,
                        verb
                    ),
                    severity: severity.to_string(),
                    confidence: to_confidence(&[], "likely"),
                    primary_location: anchor_of(&binding.argument_anchor, routine),
                    evidence_path: path,
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Load {} with Get / FindFirst before forwarding to {}, or have {} load its parameter internally.",
                            binding.source_variable_name.as_deref().unwrap_or(""),
                            callee.name,
                            callee.name
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
    stats.add_skip("unresolved", skipped_unresolved);
    stats.add_skip("implicitRec", skipped_implicit_rec);
    stats.add_skip("tempRecord", skipped_temp_record);
    stats.add_skip("callerLoaded", skipped_caller_loaded);
    stats.add_skip("calleeUnknown", skipped_callee_unknown);
    stats.add_skip("callerOwnsLoad", skipped_caller_owns_load);
    Ok(DetectorOutput::no_diag(findings, stats))
}
