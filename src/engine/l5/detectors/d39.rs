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
//! #343 (`helperPersists`): a candidate is also skipped when, on EVERY path in
//! the caller from the call to an exit, the record is persisted, by the caller
//! or by a later call that hands it `var` (or as the receiver of a table
//! procedure) to a helper that itself persists it on every path, with nothing
//! reloading, initing or copying into it first. A path needs no persist when it
//! runs only on the `if C(..)` / `if not C(..)` arm where the callee returned
//! the Boolean literal on which it provably leaves the record clean. The walk is
//! `walk_paths` over `statement_tree` and fails closed (keeps the finding): an
//! `exit` before the persist, `try`, `asserterror`, an incomplete parse, an
//! unresolved or interface call taking the record, a ternary alternative, an
//! earlier loop pass's unsaved write, more than 3 helper levels, and any `var`
//! call between the call and the persist whose own body is not proven to leave
//! the record be (no L4 role is trusted for it; an event publisher never is,
//! since its subscribers are not read). The clean-return proof likewise trusts
//! only bodies proven to make no dirt. A `var` event publisher anywhere between
//! the call and the persist keeps the finding (#343 F5): a subscriber may reload
//! or replace the record. Known false negatives: an `IsHandled` exit or an
//! `OnBefore..` publisher before a helper's persist, a compound guard, a
//! non-literal return.
//!
//! An implicit `Rec` (#272) is reported only where nobody else persists it
//! (`implicit_rec_persisted_by_owner`): a table / tableext procedure or field
//! trigger hands `Rec` back to its caller, the platform writes it after the table
//! triggers, the tableext OnBefore* triggers, and the listed triggers and the
//! procedures of a modifiable page; page OnDeleteRecord is moot. It reports in a
//! tableext OnAfterInsert / OnAfterModify / OnAfterRename, OnAfterGetRecord or any
//! other page trigger, on an `Editable = false` / `ModifyAllowed = false` page, and
//! wherever the routine reloads `Rec` after the call. `xRec` always reports. Known
//! false negative: a modifiable page opened in view mode (a runtime fact).
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
    LoadPointCache, PathQuestion, PathStates, anchor_of, before_anchor, callee_param_table_subject,
    implicit_rec_persisted_by_owner, is_table_receiver_rec, load_point_after, walk_paths,
};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::{PAnchor, PCFNNode, PCallSite, PCallee};
use crate::program::model::symbol_table::SymbolTable;
use crate::program::model::workspace::{Model, ModelRoutine};

pub(crate) const DETECTOR: &str = "d39-record-left-dirty-across-chain";

const PERSIST_OPS: &[&str] = &["Modify", "Insert", "Rename"];

pub fn detect_d39(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    // The owner rule finds a pageext's base page by name (#272).
    let symbols = SymbolTable::build(&ws.objects, &[]);
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_caller_persists = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_implicit_rec_owner_persists = 0u64;
    let mut skipped_dirt_returned_to_caller = 0u64;
    let mut skipped_helper_persists = 0u64;
    // Each caller's load points, built once however many calls it makes.
    let mut load_points = LoadPointCache::default();

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

                // A load / init / copy into the source after the call drops the
                // dirt in this routine: source order, except a reset in the other
                // arm of a branch holding the call (`branch_exclusive`). A reset is
                // the shared "loaded" definition: own load ops and `var` helpers
                // that load.
                let mut reset_after_call = || {
                    load_point_after(
                        load_points.get(caller, ctx),
                        caller.statement_tree.as_ref(),
                        &source_name_lc,
                        binding.source_record_variable_id.as_deref(),
                        &cs.source_anchor,
                        &cs.id,
                    )
                };

                // #272 (absorbs detector-audit class B): an implicit `Rec` that the
                // caller or the platform persists, unless this routine drops the
                // dirt itself. `xRec` is persisted by nobody, so it always reports.
                // `is_variable` inside `reset_after_call` matches any point here:
                // an implicit binding has no record-variable id, so the `rec` name
                // bucket is the match.
                if binding.source_kind == "implicit-rec"
                    && source_name_lc == "rec"
                    && implicit_rec_persisted_by_owner(caller, ctx, &symbols)
                    && !reset_after_call()
                {
                    skipped_implicit_rec_owner_persists += 1;
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
                // lost the callee's write: a reset after the call
                // (`reset_after_call`) keeps it flagged.
                // No caller-visibility gate (see the module doc): the dirt goes back
                // to the caller whether or not that caller is in the workspace.
                if binding.source_kind == "parameter"
                    && let Some(src_ix) = binding.source_parameter_index
                    && ctx
                        .parameter_roles_by_routine
                        .get(&caller.id)
                        .and_then(|rs| rs.iter().find(|r| r.parameter_index == src_ix))
                        .is_some_and(|r| r.dirty_at_exit == EffectPresence::Yes)
                    && !reset_after_call()
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
                // #343: on every path from the call to an exit the record is
                // persisted, by the caller or by a later call to a helper that
                // persists it on every path, with no reload / init / copy first;
                // or that path runs only when the callee returned the value on
                // which it leaves the record clean.
                if !reset_after_call()
                    && persisted_on_every_path_after(
                        caller,
                        ctx,
                        cs,
                        &source_name_lc,
                        binding.source_record_variable_id.as_deref(),
                        clean_return(callee, role.parameter_index, ctx),
                        // Body-proven, not role-trusted (#343 round 2).
                        !callee
                            .parameters
                            .iter()
                            .find(|p| p.index == role.parameter_index)
                            .is_some_and(|p| helper_preserves(callee, ctx, &p.name, 1)),
                    )
                {
                    skipped_helper_persists += 1;
                    continue; // helperPersists
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
                    // #252: the callee, its parameter and the caller's table.
                    loose_subject: Some(callee_param_table_subject(
                        "d39",
                        &callee.id,
                        role.parameter_index,
                        &caller.record_variables,
                        binding,
                    )),
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
    stats.add_skip(
        "implicitRecOwnerPersists",
        skipped_implicit_rec_owner_persists,
    );
    stats.add_skip("dirtReturnedToCaller", skipped_dirt_returned_to_caller);
    stats.add_skip("helperPersists", skipped_helper_persists);
    Ok(DetectorOutput::no_diag(findings, stats))
}

// ---------------------------------------------------------------------------
// #343: the record is persisted on every path after the call.
// ---------------------------------------------------------------------------

/// Path states: before the call; after the latest run of the call with its
/// dirt unsaved (`PENDING`); dirt of an EARLIER run of the call (a loop) still
/// unsaved (`CARRIED`: a clean return of a later run does not clear it); the
/// dirt saved, or never made (`DONE`).
const BEFORE: PathStates = 1;
const PENDING: PathStates = 2;
const DONE: PathStates = 4;
const CARRIED: PathStates = 8;
const UNSAVED: PathStates = PENDING | CARRIED;

/// The unsaved states (`mask`) saved.
fn settle(s: PathStates, mask: PathStates) -> PathStates {
    if s & mask != 0 { (s & !mask) | DONE } else { s }
}

/// How deep a helper's own persisting helpers are followed.
const MAX_HELPER_DEPTH: usize = 3;

fn names_var(name: &str, id: Option<&str>, var_lc: &str, var_id: Option<&str>) -> bool {
    name.eq_ignore_ascii_case(var_lc)
        && match (id, var_id) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// The body a must-path proof may read: present and fully parsed. An
/// incomplete parse lowers unknown statements to transparent `other` nodes.
fn proof_tree(r: &ModelRoutine) -> Option<&PCFNNode> {
    r.statement_tree
        .as_ref()
        .filter(|_| r.body_available && !r.parse_incomplete)
}

/// The routine a call site certainly runs: a resolved direct or method call
/// (not an interface dispatch, whose other implementers are not read).
fn certain_callee<'c>(ctx: &'c DetectorContext, cs: &PCallSite) -> Option<&'c ModelRoutine> {
    use crate::program::model::taxonomy::{DispatchKind, Resolution};
    let e = ctx.resolved_call_edge_by_callsite.get(&cs.id)?;
    if e.resolution != Resolution::Resolved
        || !matches!(e.dispatch_kind, DispatchKind::Direct | DispatchKind::Method)
    {
        return None;
    }
    ctx.routine_by_id.get(e.to.as_deref()?).copied()
}

/// A call with no target that the resolver did not flag as unresolved: a
/// platform builtin.
fn is_builtin_site(ctx: &DetectorContext, routine: &ModelRoutine, cs: &PCallSite) -> bool {
    !ctx.resolved_call_edge_by_callsite.contains_key(&cs.id)
        && !ctx
            .own_uncertainty_edges(&routine.id)
            .any(|u| u.callsite_id.as_deref() == Some(cs.id.as_str()))
}

fn is_event_publisher(r: &ModelRoutine) -> bool {
    use crate::program::attributes::has_attribute;
    ["IntegrationEvent", "BusinessEvent", "InternalEvent"]
        .iter()
        .any(|a| has_attribute(&r.attributes_parsed, a))
}

/// A whole-record argument (not a field or an expression).
fn whole_record_arg(b: &crate::program::body::features::PCallArgumentBinding) -> bool {
    matches!(
        b.source_kind.as_str(),
        "local" | "parameter" | "global" | "implicit-rec"
    )
}

/// What a leaf does to record `var_lc` for a persist proof.
#[derive(PartialEq)]
enum VarEffect {
    None,
    Persists,
    /// Reloads, inits or copies into it, or may: the unsaved write is lost.
    Resets,
    /// Cannot tell (an unresolved or interface call, a builtin taking the record).
    Unknown,
}

/// What leaf `leaf` of `routine` does to record `var_lc` (`var_id` narrows by
/// id when both sides carry one): its own ops ([`op_effect`]), or a call that
/// takes the record ([`call_effect`]).
fn var_effect(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    leaf: &PCFNNode,
    var_lc: &str,
    var_id: Option<&str>,
    depth: usize,
) -> VarEffect {
    if let Some(op_id) = &leaf.operation_id {
        return routine
            .record_operations
            .iter()
            .find(|op| &op.id == op_id)
            .map_or(VarEffect::None, |op| op_effect(op, var_lc, var_id));
    }
    leaf.callsite_id
        .as_ref()
        .and_then(|id| routine.call_sites.iter().find(|c| &c.id == id))
        .map_or(VarEffect::None, |cs| {
            call_effect(routine, ctx, cs, var_lc, var_id, depth)
        })
}

fn op_effect(
    op: &crate::program::model::workspace::ModelRecordOperation,
    var_lc: &str,
    var_id: Option<&str>,
) -> VarEffect {
    use crate::engine::l4::summary_runner::record_flow_role;
    if !names_var(
        &op.record_variable_name,
        op.record_variable_id.as_deref(),
        var_lc,
        var_id,
    ) {
        return VarEffect::None;
    }
    match record_flow_role(&op.op) {
        "persistsCurrent" => VarEffect::Persists,
        "loadsFromDb" | "initialises" | "copiesInto" => VarEffect::Resets,
        _ => VarEffect::None,
    }
}

/// What call `cs` of `routine` does to record `var_lc`, taken as a
/// whole-record argument or as the receiver. Fails closed: a `var` helper
/// persists it only when it persists it on every path ([`helper_persists`]),
/// and leaves it be only when its own body provably does
/// ([`helper_preserves`]); no L4 role is trusted for this, because a role does
/// not see a whole-record assignment or a receiver call. Anything else that
/// takes the record is `Unknown`, an event publisher included (#343 F5).
fn call_effect(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    cs: &PCallSite,
    var_lc: &str,
    var_id: Option<&str>,
    depth: usize,
) -> VarEffect {
    let on_receiver = matches!(&cs.callee, PCallee::Member { receiver, .. }
        if receiver.trim_matches('"').eq_ignore_ascii_case(var_lc));
    let args: Vec<usize> = cs
        .argument_bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            whole_record_arg(b)
                && b.source_variable_name.as_deref().is_some_and(|n| {
                    names_var(n, b.source_record_variable_id.as_deref(), var_lc, var_id)
                })
        })
        .map(|(i, _)| i)
        .collect();
    if !on_receiver && args.is_empty() {
        return VarEffect::None;
    }
    let Some(callee) = certain_callee(ctx, cs) else {
        // A builtin record method (`Job.TableCaption()`) leaves the record be;
        // a builtin taking the record (`Clear(Job)`) or an unresolved call may not.
        return if args.is_empty() && is_builtin_site(ctx, routine, cs) {
            VarEffect::None
        } else {
            VarEffect::Unknown
        };
    };
    let deeper = depth < MAX_HELPER_DEPTH;
    if on_receiver {
        // A table procedure on the record: no receiver role on master, so only a
        // proven persist on every path counts; anything else may reload `Rec`.
        return if callee.kind != "trigger"
            && is_table_receiver_rec(callee, "rec")
            && deeper
            && helper_persists(callee, ctx, "rec", depth + 1)
        {
            VarEffect::Persists
        } else {
            VarEffect::Unknown
        };
    }
    let upgraded = ctx.upgraded_bindings_by_callsite.get(&cs.id);
    let mut out = VarEffect::None;
    for i in args {
        let b = &cs.argument_bindings[i];
        let Some(u) = upgraded.and_then(|u| u.get(i)) else {
            return VarEffect::Unknown;
        };
        if u.binding_resolution != "resolved" {
            return VarEffect::Unknown;
        }
        if !u.callee_parameter_is_var {
            continue; // a copy: the callee cannot touch the caller's record
        }
        let Some(p) = callee
            .parameters
            .iter()
            .find(|p| p.index == b.parameter_index)
            .filter(|_| deeper)
        else {
            return VarEffect::Unknown;
        };
        if helper_persists(callee, ctx, &p.name, depth + 1) {
            out = VarEffect::Persists;
        } else if !helper_preserves(callee, ctx, &p.name, depth + 1) {
            return VarEffect::Unknown;
        }
    }
    out
}

/// Does `routine` leave the record it was handed as `var_lc` as it got it:
/// no reload / init / copy into it, no `Var := Other`, and every call that
/// takes it provably persists or preserves it? Read from the body, path-
/// insensitively (any such point anywhere fails). An event publisher never
/// does: its subscribers are not read (#343 F5).
fn helper_preserves(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    var_lc: &str,
    depth: usize,
) -> bool {
    !is_event_publisher(routine)
        && proof_tree(routine).is_some()
        && !record_reassigned(routine, var_lc, None)
        && routine
            .record_operations
            .iter()
            .all(|op| op_effect(op, var_lc, None) != VarEffect::Resets)
        && routine.call_sites.iter().all(|cs| {
            !matches!(
                call_effect(routine, ctx, cs, var_lc, None, depth),
                VarEffect::Resets | VarEffect::Unknown
            )
        })
}

/// A bare whole-record assignment `Var := Other` in `routine`, after `after`
/// when given. Assignments are not leaves of the tree, so this is by source
/// order and fails closed.
fn record_reassigned(routine: &ModelRoutine, var_lc: &str, after: Option<&PAnchor>) -> bool {
    routine.var_assignments.iter().any(|a| {
        a.lhs_name.eq_ignore_ascii_case(var_lc)
            && a.rhs_identifier.is_some()
            && after.is_none_or(|x| before_anchor(x, &a.source_anchor))
    })
}

/// The walk of [`persisted_on_every_path_after`].
struct PersistAfter<'a> {
    routine: &'a ModelRoutine,
    ctx: &'a DetectorContext<'a>,
    call_id: &'a str,
    var_lc: &'a str,
    var_id: Option<&'a str>,
    clean_value: Option<bool>,
    /// The dirtying callee may reload / init / copy into the record: a second
    /// run of the call (a loop) would drop an earlier run's unsaved write.
    callee_resets: bool,
    seen: bool,
    failed: bool,
}

impl PathQuestion for PersistAfter<'_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        if leaf.callsite_id.as_deref() == Some(self.call_id) {
            self.seen = true;
            if s & UNSAVED == 0 {
                return PENDING;
            }
            // A re-run (a loop): the earlier write is lost if the callee may
            // reset the record, or if a `Var := Other` (not a tree leaf, so
            // anywhere in the routine) may run on the back edge.
            self.failed |= self.callee_resets || record_reassigned(self.routine, self.var_lc, None);
            return PENDING | CARRIED;
        }
        if s & UNSAVED == 0 {
            return s;
        }
        match var_effect(self.routine, self.ctx, leaf, self.var_lc, self.var_id, 0) {
            VarEffect::None => s,
            VarEffect::Persists => settle(s, UNSAVED),
            VarEffect::Resets | VarEffect::Unknown => {
                self.failed = true;
                s
            }
        }
    }
    fn arm(&mut self, cs_id: &str, value: bool, s: PathStates) -> PathStates {
        // Only this run's dirt: an earlier run's (`CARRIED`) stays unsaved.
        if cs_id == self.call_id && self.clean_value == Some(value) {
            settle(s, PENDING)
        } else {
            s
        }
    }
    fn exit(&mut self, _value: Option<bool>, s: PathStates) {
        self.failed |= s & UNSAVED != 0;
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}

/// Is record `var_lc` of `caller` persisted on every path from call `cs` to an
/// exit, with nothing reloading it, initing it or copying into it first? A
/// path that runs only when the callee returned `clean_value` (an arm of
/// `if C(..)` / `if not C(..)`) carries no dirt of that run. Path-sensitive:
/// an `exit` reached before the persist fails it. Fails closed on anything
/// the walk cannot see through.
fn persisted_on_every_path_after(
    caller: &ModelRoutine,
    ctx: &DetectorContext,
    cs: &PCallSite,
    var_lc: &str,
    var_id: Option<&str>,
    clean_value: Option<bool>,
    callee_resets: bool,
) -> bool {
    let Some(tree) = proof_tree(caller) else {
        return false;
    };
    if record_reassigned(caller, var_lc, Some(&cs.source_anchor)) {
        return false;
    }
    let mut q = PersistAfter {
        routine: caller,
        ctx,
        call_id: &cs.id,
        var_lc,
        var_id,
        clean_value,
        callee_resets,
        seen: false,
        failed: false,
    };
    walk_paths(tree, BEFORE, &mut q);
    q.seen && !q.failed
}

/// The walk of [`helper_persists`].
struct HelperPersists<'a> {
    routine: &'a ModelRoutine,
    ctx: &'a DetectorContext<'a>,
    var_lc: &'a str,
    depth: usize,
    failed: bool,
}

impl PathQuestion for HelperPersists<'_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        if s & PENDING == 0 {
            return s;
        }
        match var_effect(self.routine, self.ctx, leaf, self.var_lc, None, self.depth) {
            VarEffect::None => s,
            VarEffect::Persists => settle(s, PENDING),
            VarEffect::Resets | VarEffect::Unknown => {
                self.failed = true;
                s
            }
        }
    }
    fn exit(&mut self, _value: Option<bool>, s: PathStates) {
        self.failed |= s & PENDING != 0;
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}

/// Does `routine` persist the record it was handed as `var_lc` on every path
/// from entry to exit, with nothing reloading, initing or copying into it
/// before the persist? `depth` is the helper level, bounded by
/// [`MAX_HELPER_DEPTH`]; a memo would key on it too.
fn helper_persists(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    var_lc: &str,
    depth: usize,
) -> bool {
    let Some(tree) = proof_tree(routine) else {
        return false;
    };
    if record_reassigned(routine, var_lc, None) {
        return false;
    }
    let mut q = HelperPersists {
        routine,
        ctx,
        var_lc,
        depth,
        failed: false,
    };
    walk_paths(tree, PENDING, &mut q);
    !q.failed
}

const CLEAN: PathStates = 1;
const DIRTY: PathStates = 2;

/// Can call `cs` of `routine` leave record `var_lc` dirty? Fails closed: it
/// keeps the record clean only when it does not take it, is a builtin record
/// method on it, takes it by value, or is a resolved direct / method `var`
/// call whose own body provably makes no dirt ([`helper_clean`]). No L4 role
/// is trusted: a role does not compose a receiver call. A call on the record
/// to a table procedure, an unresolved call, an interface dispatch and an
/// event publisher (its subscribers are not read) may dirty it.
fn call_keeps_clean(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    cs: &PCallSite,
    var_lc: &str,
    depth: usize,
) -> bool {
    let on_receiver = matches!(&cs.callee, PCallee::Member { receiver, .. }
        if receiver.trim_matches('"').eq_ignore_ascii_case(var_lc));
    let args: Vec<usize> = cs
        .argument_bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            whole_record_arg(b)
                && b.source_variable_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(var_lc))
        })
        .map(|(i, _)| i)
        .collect();
    if !on_receiver && args.is_empty() {
        return true;
    }
    let Some(callee) = certain_callee(ctx, cs) else {
        return args.is_empty() && is_builtin_site(ctx, routine, cs);
    };
    if on_receiver || depth >= MAX_HELPER_DEPTH {
        return false;
    }
    let upgraded = ctx.upgraded_bindings_by_callsite.get(&cs.id);
    args.into_iter().all(|i| {
        let b = &cs.argument_bindings[i];
        match upgraded.and_then(|u| u.get(i)) {
            Some(u) if u.binding_resolution == "resolved" && !u.callee_parameter_is_var => true,
            Some(u) if u.binding_resolution == "resolved" => callee
                .parameters
                .iter()
                .find(|p| p.index == b.parameter_index)
                .is_some_and(|p| helper_clean(callee, ctx, &p.name, depth + 1)),
            _ => false,
        }
    })
}

/// Does `routine` make no dirt on the record it was handed as `var_lc`: no
/// `Validate` or copy into it anywhere, and every call that takes it keeps it
/// clean ([`call_keeps_clean`])? Path-insensitive. Never for an event
/// publisher.
fn helper_clean(routine: &ModelRoutine, ctx: &DetectorContext, var_lc: &str, depth: usize) -> bool {
    use crate::engine::l4::summary_runner::record_flow_role;
    !is_event_publisher(routine)
        && proof_tree(routine).is_some()
        && routine.record_operations.iter().all(|op| {
            // A copy counts as dirt here, though `CleanReturn` (path-sensitive, on
            // the callee's own body) treats it as a reset: path-insensitively we
            // cannot tell whether a Validate follows it.
            !op.record_variable_name.eq_ignore_ascii_case(var_lc)
                || !matches!(record_flow_role(&op.op), "validates" | "copiesInto")
        })
        && routine
            .call_sites
            .iter()
            .all(|cs| call_keeps_clean(routine, ctx, cs, var_lc, depth))
}

/// The walk of [`clean_return`].
struct CleanReturn<'a> {
    routine: &'a ModelRoutine,
    ctx: &'a DetectorContext<'a>,
    var_lc: &'a str,
    /// Dirty at an exit returning `false` / `true`.
    dirty_on: [bool; 2],
    failed: bool,
}

impl PathQuestion for CleanReturn<'_> {
    fn leaf(&mut self, leaf: &PCFNNode, s: PathStates) -> PathStates {
        use crate::engine::l4::summary_runner::record_flow_role;
        let r = self.routine;
        if let Some(op_id) = &leaf.operation_id {
            let Some(op) = r.record_operations.iter().find(|op| &op.id == op_id) else {
                return s;
            };
            if !op.record_variable_name.eq_ignore_ascii_case(self.var_lc) {
                return s;
            }
            return match record_flow_role(&op.op) {
                "validates" => DIRTY,
                "persistsCurrent" | "loadsFromDb" | "initialises" | "copiesInto" => CLEAN,
                _ => s,
            };
        }
        // A call that takes the parameter: it may dirty it unless
        // `call_keeps_clean` proves otherwise.
        match leaf
            .callsite_id
            .as_ref()
            .and_then(|id| r.call_sites.iter().find(|c| &c.id == id))
        {
            Some(cs) if !call_keeps_clean(r, self.ctx, cs, self.var_lc, 0) => s | DIRTY,
            _ => s,
        }
    }
    fn exit(&mut self, value: Option<bool>, s: PathStates) {
        if s & DIRTY != 0 {
            match value {
                Some(v) => self.dirty_on[usize::from(v)] = true,
                None => self.dirty_on = [true, true],
            }
        }
    }
    fn opaque(&mut self) {
        self.failed = true;
    }
}

/// The value `callee` returns on every path where it leaves its parameter
/// `param_index` clean, when its dirty exits all return the other Boolean
/// literal (`exit(false)` before the `Validate`, `exit(true)` after it).
fn clean_return(callee: &ModelRoutine, param_index: u32, ctx: &DetectorContext) -> Option<bool> {
    let tree = proof_tree(callee)?;
    let p = callee.parameters.iter().find(|p| p.index == param_index)?;
    let mut q = CleanReturn {
        routine: callee,
        ctx,
        var_lc: &p.name,
        dirty_on: [false, false],
        failed: false,
    };
    walk_paths(tree, CLEAN, &mut q);
    match (q.failed, q.dirty_on) {
        (false, [false, true]) => Some(false),
        (false, [true, false]) => Some(true),
        _ => None,
    }
}
