//! D39 — record left dirty across helper chain. Port of al-sem
//! `src/detectors/d39-record-left-dirty-across-chain.ts`.
//!
//! For each var-param P of every primary callee where the path-aware walker PROVED
//! `dirtyAtExit[P] === "yes"`, walk the reverse call graph. Every primary caller that
//! forwards a record to P by-var, does NOT persist that source after the callsite, and
//! does NOT pass it from a by-value parameter, is flagged: the Validate's field write
//! is silently discarded across the chain.
//!
//! A caller that forwards its OWN `var` parameter, and whose parameter is
//! therefore dirty at exit too, discards nothing: the record goes back to its
//! caller, which this same walk judges through that parameter's role. Only the
//! routine that owns the record (or drops the dirt itself) is flagged.
//! "Drops the dirt itself" is judged at the call site: a load / init / copy into
//! the record after the call, by the caller or by a `var` helper whose role
//! loads it (`record_load_points`, shared with d40), keeps the caller flagged.
//! Unlike d40, the skip does NOT need visible callers (`owner_is_judged`): d39's
//! claim is that the write is discarded IN this routine, and a `var` parameter
//! returns the dirt to whoever called it, so the claim is false here even when
//! that caller is outside the workspace. "After the call" is source order, minus
//! a reset in the OTHER arm of an `if` / `case` that also holds the call (outside
//! any loop; `branch_exclusive`): only one of the two runs. Known limits: a
//! reload BEFORE the call on a loop back-edge (it runs after the call on the next
//! iteration) is missed, a reload after an `exit` on the call's path still counts,
//! and a whole-record assignment (`Cust := Other`) is not a record op, so it does
//! not count as a reset.
//!
//! Reads the CORE `RoutineSummary.parameterRoles` via `ctx.parameter_roles_by_routine`
//! (the `dirtyAtExit` fact), `ctx.reverse_call_graph`, and the post-upgrade per-callsite
//! bindings via `ctx.upgraded_bindings_by_callsite` joined positionally with
//! `call_site.argument_bindings`.
//!
//! G-13: bindings whose caller-side source record is `Known(true)` TEMPORARY
//! are skipped — discarding in-memory state has no SQL consequence. Mirrors
//! the d40 `source_temp_state` gate. Physical/Unknown keep firing
//! (suppression-direction safe).

use crate::engine::l4::effect_lattice::EffectPresence;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, before_anchor, branch_exclusive, is_auto_persist_trigger_rec, record_load_points,
};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d39-record-left-dirty-across-chain";

const PERSIST_OPS: &[&str] = &["Modify", "Insert", "Rename"];

pub fn detect_d39(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_caller_persists = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_auto_persist_trigger = 0u64;
    let mut skipped_dirt_returned_to_caller = 0u64;

    for callee in &ws.routines {
        if !callee.body_available {
            continue;
        }
        let roles = match ctx.parameter_roles_by_routine.get(&callee.id) {
            Some(r) => r,
            None => continue,
        };
        for role in roles {
            match role.dirty_at_exit {
                EffectPresence::Unknown => continue, // unknownDirtyCallee
                EffectPresence::Yes => {}
                EffectPresence::No => continue,
            }

            // All resolved callers that forward a record to this var-parameter.
            let caller_edges = match ctx.reverse_call_graph.get(&callee.id) {
                Some(e) => e,
                None => continue,
            };
            for edge in caller_edges {
                let callsite_id = match &edge.callsite_id {
                    Some(c) => c,
                    None => continue,
                };
                let caller = match ctx.routine_by_id.get(edge.from.as_str()) {
                    Some(c) => *c,
                    None => continue,
                };
                // roleOf(caller) !== "primary" → skip. Source-only ⇒ all primary.
                if !caller.body_available {
                    continue;
                }

                let cs = match caller.call_sites.iter().find(|c| &c.id == callsite_id) {
                    Some(c) => c,
                    None => continue,
                };

                // binding = cs.argumentBindings.find(parameterIndex == role.parameterIndex
                //   && bindingResolution === "resolved" && calleeParameterIsVar)
                let upgraded = ctx.upgraded_bindings_by_callsite.get(&cs.id);
                let binding_idx = cs.argument_bindings.iter().enumerate().find(|(i, b)| {
                    if b.parameter_index != role.parameter_index {
                        return false;
                    }
                    let up = upgraded.and_then(|u| u.get(*i));
                    up.map(|u| u.binding_resolution == "resolved" && u.callee_parameter_is_var)
                        .unwrap_or(false)
                });
                let (i, binding) = match binding_idx {
                    Some((i, b)) => (i, b),
                    None => continue,
                };
                let _ = i;

                // Only source kinds the caller can actually persist. A promoted
                // GLOBAL (RV-8: `sourceKind == "global"`) is a real caller var,
                // persistable exactly like a "local"; include it so the RV-8
                // relabel stays behavior-preserving here (the persist-after check
                // below matches by name regardless of scope).
                if binding.source_kind != "parameter"
                    && binding.source_kind != "local"
                    && binding.source_kind != "global"
                    && binding.source_kind != "implicit-rec"
                {
                    continue;
                }

                // For parameter sources, require the caller-side parameter to be var.
                if binding.source_kind == "parameter"
                    && !binding.caller_source_parameter_is_var.unwrap_or(false)
                {
                    continue;
                }

                // G-13: skip Known(true) TEMPORARY source records — a temp
                // record left dirty has no SQL consequence (same gate as d40).
                if crate::program::body::features::temp_state_suppresses(
                    binding.source_temp_state.as_ref(),
                ) {
                    skipped_temp_record += 1;
                    continue;
                }

                let source_name_lc = match &binding.source_variable_name {
                    Some(n) => n.clone(),
                    None => continue,
                };

                candidates_considered += 1;

                // Class B (docs/detector-audit.md): the caller is a table-level
                // OnInsert/OnModify/OnDelete/OnRename trigger forwarding its
                // implicit `Rec` — the platform persists `Rec` after the
                // trigger returns (OnDelete deletes it), so the dirty state is
                // not discarded.
                if is_auto_persist_trigger_rec(caller, &source_name_lc) {
                    skipped_auto_persist_trigger += 1;
                    continue;
                }

                // A `var` parameter hands the record back to the caller's caller.
                // When the caller's OWN parameter is dirty at exit (the L4 walker
                // composes the callee's dirt into it), nothing is discarded here:
                // this loop judges that parameter's callers instead. A caller that
                // drops the dirt itself (e.g. reloads after the call) is not
                // dirty at exit and still falls through to be flagged. The role is
                // a whole-routine fact, so a caller that reloads after THIS call
                // and dirties the record again itself is dirty at exit too, yet it
                // lost the callee's write: a load/init/copy into the source after
                // the call keeps it flagged. Source order, like the persist check
                // below, except that a reload in the other arm of a branch holding
                // the call does not count (`branch_exclusive`). A reset is the shared
                // "loaded" definition: own load ops and `var` helpers that load.
                // No caller-visibility gate (see the module doc): the dirt goes back
                // to the caller whether or not that caller is in the workspace.
                if binding.source_kind == "parameter"
                    && let Some(src_ix) = binding.source_parameter_index
                    && ctx
                        .parameter_roles_by_routine
                        .get(&caller.id)
                        .and_then(|rs| rs.iter().find(|r| r.parameter_index == src_ix))
                        .is_some_and(|r| r.dirty_at_exit == EffectPresence::Yes)
                    && !record_load_points(caller, ctx)
                        .get(&source_name_lc)
                        .is_some_and(|pts| {
                            pts.iter().any(|p| {
                                before_anchor(&cs.source_anchor, p.anchor)
                                    && p.is_variable(binding.source_record_variable_id.as_deref())
                                    && !branch_exclusive(
                                        caller.statement_tree.as_ref(),
                                        &cs.id,
                                        p.node_id,
                                    )
                            })
                        })
                {
                    skipped_dirt_returned_to_caller += 1;
                    continue;
                }

                // Did caller persist the source variable after the callsite?
                let persisted_after = caller.record_operations.iter().any(|op| {
                    PERSIST_OPS.contains(&op.op.as_str())
                        && op.record_variable_name.to_lowercase() == source_name_lc
                        && before_anchor(&cs.source_anchor, &op.source_anchor)
                });
                if persisted_after {
                    skipped_caller_persists += 1;
                    continue; // callerPersists
                }

                // Emit.
                let path = vec![
                    EvidenceStep {
                        routine_id: caller.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&binding.argument_anchor, caller),
                        note: format!(
                            "forwards {} to {}; never persists after the call",
                            binding.source_variable_name.as_deref().unwrap_or(""),
                            callee.name
                        ),
                    },
                    EvidenceStep {
                        routine_id: callee.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&callee.source_anchor, callee),
                        note: format!(
                            "{} validates and exits dirty on at least one path",
                            callee.name
                        ),
                    },
                ];

                let id = format!("d39/{}/{}/{}", caller.id, cs.id, role.parameter_index);
                let mut affected_objects = vec![caller.object_id.clone(), callee.object_id.clone()];
                affected_objects.sort();

                let mut finding = Finding {
                    id: id.clone(),
                    root_cause_key: id,
                    detector: DETECTOR.to_string(),
                    title: "Record left dirty across helper chain".into(),
                    root_cause: format!(
                        "{} forwards {} to {}, which leaves the record in a Validate-dirty state on at least one exit path. {} never persists after the call — the field write is silently discarded.",
                        caller.name,
                        binding.source_variable_name.as_deref().unwrap_or(""),
                        callee.name,
                        caller.name
                    ),
                    severity: "medium".to_string(),
                    confidence: to_confidence(&[], "likely"),
                    primary_location: anchor_of(&binding.argument_anchor, caller),
                    evidence_path: path,
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Add {}.Modify() in {} after the call to {}, or have {} persist before returning.",
                            binding.source_variable_name.as_deref().unwrap_or(""),
                            caller.name,
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
    stats.add_skip("callerPersists", skipped_caller_persists);
    stats.add_skip("tempRecord", skipped_temp_record);
    stats.add_skip("autoPersistTriggerRec", skipped_auto_persist_trigger);
    stats.add_skip("dirtReturnedToCaller", skipped_dirt_returned_to_caller);
    Ok(DetectorOutput::no_diag(findings, stats))
}
