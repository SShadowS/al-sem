//! L3 call resolver (R2b Task 3) — faithful port of al-sem's `resolveCalls` /
//! `resolveCallSite` / `resolveByNameAndArity` / `resolveInterfaceDispatch` /
//! `upgradeBindings` from `src/resolve/call-resolver.ts`.
//!
//! Resolves every call site in the assembled L3 workspace into one or more
//! `CallEdge`s (interface dispatch is MULTI-edge), mutating each callsite's
//! argument bindings exactly ONCE (`upgrade_bindings`) once the callee is known.
//! Unresolved calls are DATA (an edge with no `to` and a non-"resolved"
//! resolution), never a silent gap.
//!
//! All ids on the produced edges are INTERNAL ids (routine id / callsite id /
//! operation id). The dump / vector test projects them to StableRoutineId and
//! groups multi-edge callsites + lifts `dispatchMeta` to the group level.

use super::al_builtins::global_builtin_disposition;
use super::implicit_edges::build_implicit_trigger_edges;
use super::l3_workspace::{L3Routine, L3Workspace};
use super::receiver_type::{DispatchCtx, dispatch, infer_receiver_type};
use super::static_arg::static_arg_type;
use super::symbol_table::SymbolTable;
use super::type_rel::{TypeRelation, type_relation};
use crate::engine::l2::features::PCallSite;
use crate::engine::l3::taxonomy::{DispatchKind, Resolution};
use std::collections::HashMap;

// The call-resolution SHAPE (edges, upgraded bindings, diagnostics, declared
// dependencies) moved to `program::model::calls` in engine-switch S2b.2; it is
// re-exported here so this resolver and its users are unchanged.
pub use crate::program::model::calls::{
    CallEdge, DeclaredDependency, Diagnostic, DispatchMeta, ExternalTypeRef, ResolvedCalls,
    UnknownReason, UpgradedBinding,
};

// ---------------------------------------------------------------------------
// Upgraded-binding side table (the `upgradeBindings` mutation, captured out of
// band because the L3 PCallArgumentBinding does not carry the upgrade fields).
// ---------------------------------------------------------------------------

/// Per-callsite upgraded bindings. `upgraded` guards `upgrade_bindings` so it
/// runs EXACTLY once per callsite (reproducing al-sem's double-upgrade guard).
pub(crate) struct BindingState {
    pub(crate) bindings: Vec<UpgradedBinding>,
}

/// Derive the INITIAL bindingResolution for an L3 callsite's bindings, matching
/// al-sem's `intraprocedural-body.ts` construction:
///   - non-identifier arg (sourceKind "expression") → "non-record-arg"
///   - identifier bound to a record variable → "unresolved-callee" (upgradable)
///   - any other identifier (param / implicit-rec / unknown) → "non-record-arg"
///
/// `calleeParameterIsVar` starts `false` (upgraded later).
pub(crate) fn initial_binding_state(call_site: &PCallSite) -> BindingState {
    let bindings = call_site
        .argument_bindings
        .iter()
        .map(|b| {
            let resolution = if b.source_kind == "expression" {
                "non-record-arg"
            } else if b.source_record_variable_id.is_some() {
                "unresolved-callee"
            } else {
                "non-record-arg"
            };
            UpgradedBinding {
                parameter_index: b.parameter_index,
                callee_parameter_is_var: false,
                binding_resolution: resolution.to_string(),
            }
        })
        .collect();
    BindingState { bindings }
}

/// Upgrade a callsite's bindings with callee-side var-ness once the callee is
/// known. Sets `bindingResolution = "resolved"` + `calleeParameterIsVar` for any
/// binding not already "non-record-arg". Returns a diagnostic on double-upgrade
/// (and skips), reproducing al-sem's non-idempotence guard.
pub(crate) fn upgrade_bindings(
    state: &mut BindingState,
    callee: &L3Routine,
    callsite_id: &str,
) -> Option<Diagnostic> {
    upgrade_bindings_with(
        state,
        |i| callee.parameters.get(i).map(|p| p.is_var),
        callsite_id,
    )
}

/// [`upgrade_bindings`] from the callee's per-parameter `var` flags alone
/// (`None` past its last parameter), for a callee with no `L3Routine` (a
/// dependency routine, B3 adapter).
pub(crate) fn upgrade_bindings_with(
    state: &mut BindingState,
    param_is_var: impl Fn(usize) -> Option<bool>,
    callsite_id: &str,
) -> Option<Diagnostic> {
    for b in &state.bindings {
        if b.binding_resolution == "resolved" || b.binding_resolution == "ambiguous" {
            return Some(Diagnostic {
                severity: "warning".to_string(),
                stage: "resolve".to_string(),
                message: format!(
                    "call-resolver: argumentBindings for callsite {callsite_id} already upgraded (double-upgrade); skipping re-entrant resolution"
                ),
            });
        }
    }
    for (i, b) in state.bindings.iter_mut().enumerate() {
        if b.binding_resolution == "non-record-arg" {
            continue;
        }
        let Some(is_var) = param_is_var(i) else {
            continue; // arity mismatch — leave defaults
        };
        b.callee_parameter_is_var = is_var;
        b.binding_resolution = "resolved".to_string();
    }
    None
}

/// Mark all record-arg bindings "ambiguous" (leave "non-record-arg" untouched).
pub(crate) fn mark_bindings_ambiguous(state: &mut BindingState) {
    for b in &mut state.bindings {
        if b.binding_resolution == "non-record-arg" {
            continue;
        }
        b.binding_resolution = "ambiguous".to_string();
    }
}

// ---------------------------------------------------------------------------
// Arity-aware overload resolution.
// ---------------------------------------------------------------------------

pub(crate) enum ArityResolution<'a> {
    Resolved(&'a L3Routine),
    NotFound,
    NoArityMatch(Vec<&'a L3Routine>),
    Ambiguous(Vec<&'a L3Routine>),
}

/// Argument-type-aware overload tiebreak: drop ONLY candidates an inferred arg
/// type proves incompatible; resolve iff exactly one survives. Faithful port of
/// `disambiguateByArgTypes`.
fn disambiguate_by_arg_types<'a>(
    candidates: &[&'a L3Routine],
    caller: &L3Routine,
    call_site: &PCallSite,
    symbols: &SymbolTable,
) -> Option<&'a L3Routine> {
    let arg_types: Vec<Option<String>> = (0..call_site.argument_bindings.len())
        .map(|i| static_arg_type(caller, call_site, i, symbols))
        .collect();
    let survivors: Vec<&L3Routine> = candidates
        .iter()
        .copied()
        .filter(|cand| {
            arg_types.iter().enumerate().all(|(i, arg_type)| {
                let Some(arg_type) = arg_type else {
                    return true; // unknown position eliminates nothing
                };
                let Some(param) = cand.parameters.get(i) else {
                    return true;
                };
                type_relation(arg_type, &param.type_text) != TypeRelation::DefinitelyIncompatible
            })
        })
        .collect();
    if survivors.len() == 1 {
        Some(survivors[0])
    } else {
        None
    }
}

/// Resolve a call to `method_name` in `object_id` by name + exact arity, with
/// arg-type disambiguation when >1 same-arity candidates. Faithful port of
/// `resolveByNameAndArity`.
pub(crate) fn resolve_by_name_and_arity<'a>(
    symbols: &'a SymbolTable,
    object_id: &str,
    method_name: &str,
    caller: &L3Routine,
    call_site: &PCallSite,
) -> ArityResolution<'a> {
    resolve_by_name_and_arity_multi(symbols, &[object_id], method_name, caller, call_site)
}

/// Like [`resolve_by_name_and_arity`] but searches a SET of object ids as one
/// candidate pool — used for Record dispatch over a base table UNION its
/// `TableExtension`s (a `TableExtension` procedure is globally callable on the base
/// record). Name+arity matching, arg-type disambiguation, and the
/// Resolved/NoArityMatch/Ambiguous/NotFound contract are identical to the
/// single-object case; candidate lists are sorted by id downstream (`sorted_ids`),
/// so the cross-object union order does not affect determinism.
pub(crate) fn resolve_by_name_and_arity_multi<'a>(
    symbols: &'a SymbolTable,
    object_ids: &[&str],
    method_name: &str,
    caller: &L3Routine,
    call_site: &PCallSite,
) -> ArityResolution<'a> {
    let arg_count = call_site.argument_bindings.len();
    let mut matches: Vec<&L3Routine> = Vec::new();
    for oid in object_ids {
        matches.extend(symbols.routines_in_object_by_name(oid, method_name));
    }
    if matches.is_empty() {
        return ArityResolution::NotFound;
    }
    let arity_matches: Vec<&L3Routine> = matches
        .iter()
        .copied()
        .filter(|m| m.parameters.len() == arg_count)
        .collect();
    if arity_matches.len() == 1 {
        return ArityResolution::Resolved(arity_matches[0]);
    }
    if arity_matches.is_empty() {
        return ArityResolution::NoArityMatch(matches);
    }
    if let Some(narrowed) = disambiguate_by_arg_types(&arity_matches, caller, call_site, symbols) {
        return ArityResolution::Resolved(narrowed);
    }
    ArityResolution::Ambiguous(arity_matches)
}

/// Map an object-run objectKind to its dispatch kind.
pub(crate) fn object_run_dispatch_kind(object_kind: &str) -> DispatchKind {
    match object_kind {
        "Page" => DispatchKind::PageRun,
        "Report" => DispatchKind::ReportRun,
        _ => DispatchKind::CodeunitRun,
    }
}

// ---------------------------------------------------------------------------
// Dependency classification (computed ONCE per resolve_calls).
// ---------------------------------------------------------------------------

/// True when at least one declared dep's appGuid is absent from the fetched-app
/// set. Faithful port of `hasUnfetchedDeclaredDependency`. In the source-only
/// path `primary_dependencies` is empty → false.
fn has_unfetched_declared_dependency(
    primary_dependencies: &[DeclaredDependency],
    fetched_app_guids: &[String],
) -> bool {
    if primary_dependencies.is_empty() {
        return false;
    }
    let fetched: std::collections::HashSet<String> =
        fetched_app_guids.iter().map(|g| g.to_lowercase()).collect();
    primary_dependencies
        .iter()
        .any(|d| !fetched.contains(&d.app_guid.to_lowercase()))
}

// ---------------------------------------------------------------------------
// Interface dispatch.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_interface_dispatch(
    from: &str,
    callsite_id: &str,
    operation_id: &str,
    interface_name: &str,
    method_name: &str,
    caller: &L3Routine,
    call_site: &PCallSite,
    symbols: &SymbolTable,
    state: &mut BindingState,
) -> Vec<CallEdge> {
    let impls = symbols.objects_implementing(interface_name); // codeunits only, sorted by id
    let enum_impls = symbols.enum_implementers(interface_name);

    // Interface dispatch is polymorphic — bindings are ambiguous, never upgraded.
    mark_bindings_ambiguous(state);

    let mut resolved_edges: Vec<CallEdge> = Vec::new();
    let mut unresolved_impls: Vec<(String, String)> = Vec::new();

    for impl_obj in &impls {
        match resolve_by_name_and_arity(symbols, &impl_obj.id, method_name, caller, call_site) {
            ArityResolution::Resolved(r) => {
                let mut e = CallEdge::base(from, callsite_id, operation_id);
                e.to = Some(r.id.clone());
                e.dispatch_kind = DispatchKind::Interface;
                e.resolution = Resolution::Maybe;
                resolved_edges.push(e);
            }
            ArityResolution::NotFound => {
                unresolved_impls.push((impl_obj.id.clone(), "not-found".to_string()));
            }
            ArityResolution::NoArityMatch(_) => {
                unresolved_impls.push((impl_obj.id.clone(), "no-arity-match".to_string()));
            }
            ArityResolution::Ambiguous(_) => {
                unresolved_impls.push((impl_obj.id.clone(), "ambiguous".to_string()));
            }
        }
    }

    let enum_implementer_ids: Vec<String> = enum_impls.iter().map(|e| e.id.clone()).collect();

    let dispatch_meta = DispatchMeta {
        interface_name: interface_name.to_string(),
        total_impls: impls.len(),
        unresolved_impls,
        enum_implementers: enum_implementer_ids,
    };

    if resolved_edges.is_empty() {
        let mut e = CallEdge::base(from, callsite_id, operation_id);
        e.dispatch_kind = DispatchKind::Interface;
        e.resolution = Resolution::Unknown(UnknownReason::InterfaceNoImpl);
        e.dispatch_meta = Some(dispatch_meta);
        return vec![e];
    }

    // Sort resolved edges by `to` (byte-order on the internal routine id),
    // matching al-sem `(a.to ?? "") < (b.to ?? "")`.
    resolved_edges.sort_by(|a, b| {
        a.to.clone()
            .unwrap_or_default()
            .cmp(&b.to.clone().unwrap_or_default())
    });
    // dispatchMeta on the FIRST edge only.
    resolved_edges[0].dispatch_meta = Some(dispatch_meta);
    resolved_edges
}

// ---------------------------------------------------------------------------
// Per-callsite resolver.
// ---------------------------------------------------------------------------

fn resolve_call_site(
    routine: &L3Routine,
    call_site: &PCallSite,
    symbols: &SymbolTable,
    diagnostics: &mut Vec<Diagnostic>,
    unfetched_declared_dependency: bool,
    state: &mut BindingState,
) -> Vec<CallEdge> {
    let from = routine.id.as_str();
    let callsite_id = call_site.id.as_str();
    let operation_id = call_site.operation_id.as_str();

    use crate::engine::l2::features::PCallee;
    match &call_site.callee {
        PCallee::Bare { name } => {
            match resolve_by_name_and_arity(symbols, &routine.object_id, name, routine, call_site) {
                ArityResolution::Resolved(r) => {
                    if let Some(d) = upgrade_bindings(state, r, callsite_id) {
                        diagnostics.push(d);
                    }
                    let mut e = CallEdge::base(from, callsite_id, operation_id);
                    e.to = Some(r.id.clone());
                    e.dispatch_kind = DispatchKind::Direct;
                    e.resolution = Resolution::Resolved;
                    vec![e]
                }
                ArityResolution::NotFound => {
                    // Fallback 1: if the caller is in an extension object, try the
                    // EXTENDS-TARGET base object's procedures (e.g. a PageExtension
                    // bare-calling a procedure defined on its base Page).
                    if let Some(caller_obj) = symbols.object_by_id(&routine.object_id)
                        && let Some(base_obj) = extends_base_object(caller_obj, symbols)
                    {
                        match resolve_by_name_and_arity(
                            symbols,
                            &base_obj.id,
                            name,
                            routine,
                            call_site,
                        ) {
                            ArityResolution::Resolved(r) => {
                                if let Some(d) = upgrade_bindings(state, r, callsite_id) {
                                    diagnostics.push(d);
                                }
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.to = Some(r.id.clone());
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::Resolved;
                                return vec![e];
                            }
                            ArityResolution::NoArityMatch(candidates) => {
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::MemberNotFound;
                                e.candidates = Some(sorted_ids(&candidates));
                                return vec![e];
                            }
                            ArityResolution::Ambiguous(candidates) => {
                                mark_bindings_ambiguous(state);
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::Ambiguous;
                                e.candidates = Some(sorted_ids(&candidates));
                                return vec![e];
                            }
                            ArityResolution::NotFound => {
                                // Fall through to global-builtin / BareUnresolved below.
                            }
                        }
                    }
                    // Fallback 1.5: implicit `Rec` — a bare call in a Page/Table to
                    // the source record's procedure (AL resolves an unqualified call
                    // in page/table code as `Rec.<proc>()`). Search the implicit table
                    // UNION its TableExtensions (a TableExt proc is callable on the
                    // base record). Own-object procedures were already tried FIRST
                    // (above), so they correctly shadow a same-named table procedure.
                    if let Some(caller_obj) = symbols.object_by_id(&routine.object_id)
                        && let Some(tbl_obj_id) = implicit_rec_table_object_id(caller_obj, symbols)
                    {
                        let mut ids: Vec<&str> = vec![tbl_obj_id.as_str()];
                        if let Some(tbl_obj) = symbols.object_by_id(&tbl_obj_id) {
                            ids.extend(
                                symbols.table_extension_object_ids(
                                    &tbl_obj.name,
                                    tbl_obj.object_number,
                                ),
                            );
                        }
                        match resolve_by_name_and_arity_multi(
                            symbols, &ids, name, routine, call_site,
                        ) {
                            ArityResolution::Resolved(r) => {
                                if let Some(d) = upgrade_bindings(state, r, callsite_id) {
                                    diagnostics.push(d);
                                }
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.to = Some(r.id.clone());
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::Resolved;
                                return vec![e];
                            }
                            ArityResolution::NoArityMatch(candidates) => {
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::MemberNotFound;
                                e.candidates = Some(sorted_ids(&candidates));
                                return vec![e];
                            }
                            ArityResolution::Ambiguous(candidates) => {
                                mark_bindings_ambiguous(state);
                                let mut e = CallEdge::base(from, callsite_id, operation_id);
                                e.dispatch_kind = DispatchKind::Direct;
                                e.resolution = Resolution::Ambiguous;
                                e.candidates = Some(sorted_ids(&candidates));
                                return vec![e];
                            }
                            ArityResolution::NotFound => {
                                // Fall through to global-builtin / BareUnresolved.
                            }
                        }
                    }
                    // Fallback 2: global builtins, then BareUnresolved.
                    let mut e = CallEdge::base(from, callsite_id, operation_id);
                    if global_builtin_disposition(name).is_some() {
                        e.dispatch_kind = DispatchKind::Builtin;
                        e.resolution = Resolution::Builtin;
                    } else {
                        e.dispatch_kind = DispatchKind::Unresolved;
                        e.resolution = Resolution::Unknown(UnknownReason::BareUnresolved);
                        // DIAGNOSTIC: thread the bare call name (lowercased) so
                        // `--l3-unknown-breakdown` can name the residual bare-unresolved
                        // bucket and identify genuine catalog gaps.
                        e.unknown_method_name = Some(name.to_lowercase());
                    }
                    vec![e]
                }
                ArityResolution::NoArityMatch(candidates) => {
                    let mut e = CallEdge::base(from, callsite_id, operation_id);
                    e.dispatch_kind = DispatchKind::Direct;
                    e.resolution = Resolution::MemberNotFound;
                    e.candidates = Some(sorted_ids(&candidates));
                    vec![e]
                }
                ArityResolution::Ambiguous(candidates) => {
                    mark_bindings_ambiguous(state);
                    let mut e = CallEdge::base(from, callsite_id, operation_id);
                    e.dispatch_kind = DispatchKind::Direct;
                    e.resolution = Resolution::Ambiguous;
                    e.candidates = Some(sorted_ids(&candidates));
                    vec![e]
                }
            }
        }
        PCallee::ObjectRun {
            object_kind,
            target_type,
            target_ref,
            target_is_name,
        } => {
            let dispatch_kind = object_run_dispatch_kind(object_kind);
            let Some(target_ref) = target_ref else {
                // Dynamic target (a variable) — known shape, unknown target.
                let mut e = CallEdge::base(from, callsite_id, operation_id);
                e.dispatch_kind = DispatchKind::Dynamic;
                e.resolution = Resolution::Unknown(UnknownReason::DynamicObjectRunTarget);
                return vec![e];
            };
            let target_object = if *target_is_name {
                symbols.object_by_type_name(target_type, target_ref)
            } else {
                match target_ref.parse::<i64>() {
                    Ok(n) => symbols.object_by_type_number(target_type, n),
                    Err(_) => None,
                }
            };
            let Some(target_object) = target_object else {
                // Target named/numbered but not in indexed source.
                let mut e = CallEdge::base(from, callsite_id, operation_id);
                e.dispatch_kind = dispatch_kind;
                e.resolution = Resolution::Opaque;
                return vec![e];
            };
            // Entry routine: OnRun trigger, else the first routine in document order.
            let entry = symbols
                .routine_in_object(&target_object.id, "OnRun")
                .or_else(|| {
                    symbols
                        .routines_in_object(&target_object.id)
                        .into_iter()
                        .next()
                });
            if let Some(entry) = entry {
                if let Some(d) = upgrade_bindings(state, entry, callsite_id) {
                    diagnostics.push(d);
                }
                let mut e = CallEdge::base(from, callsite_id, operation_id);
                e.to = Some(entry.id.clone());
                e.dispatch_kind = dispatch_kind;
                e.resolution = Resolution::Resolved;
                return vec![e];
            }
            let mut e = CallEdge::base(from, callsite_id, operation_id);
            e.dispatch_kind = dispatch_kind;
            e.resolution = Resolution::Opaque;
            vec![e]
        }
        PCallee::Member { receiver, method } => {
            // Two-phase typed resolution (the ReceiverType lattice):
            //   Phase A — infer the receiver's type (`infer_receiver_type`).
            //   Phase B — dispatch the method against that type (`dispatch`).
            // The fail-closed invariant lives in the lattice: any receiver Phase A
            // cannot positively type becomes `ReceiverType::Unknown { reason }`,
            // which Phase B turns into an honest `unknown` edge.
            let inferred = infer_receiver_type(receiver, routine, symbols);
            let mut ctx = DispatchCtx {
                from,
                callsite_id,
                operation_id,
                routine,
                call_site,
                symbols,
                state,
                diagnostics,
                unfetched_declared_dependency,
            };
            dispatch(&inferred, method, &mut ctx)
        }
        PCallee::Unknown => {
            let mut e = CallEdge::base(from, callsite_id, operation_id);
            e.dispatch_kind = DispatchKind::Unresolved;
            e.resolution = Resolution::Unknown(UnknownReason::CalleeUnknown);
            vec![e]
        }
    }
}

pub(crate) fn unknown_method(
    from: &str,
    callsite_id: &str,
    operation_id: &str,
    reason: UnknownReason,
) -> Vec<CallEdge> {
    let mut e = CallEdge::base(from, callsite_id, operation_id);
    e.dispatch_kind = DispatchKind::Method;
    e.resolution = Resolution::Unknown(reason);
    vec![e]
}

/// A member call on a runtime-typed (`Variant`) receiver: `dispatch_kind == Dynamic`
/// so it classifies `dynamic` (genuinely indeterminate), NOT real-`unknown`.
pub(crate) fn dynamic_method(from: &str, callsite_id: &str, operation_id: &str) -> Vec<CallEdge> {
    let mut e = CallEdge::base(from, callsite_id, operation_id);
    e.dispatch_kind = DispatchKind::Dynamic;
    e.resolution = Resolution::Unknown(UnknownReason::DynamicReceiver);
    vec![e]
}

/// Map an extension object type to its corresponding base object type.
/// Returns `None` for non-extension types.
fn extension_base_type(object_type: &str) -> Option<&'static str> {
    match object_type {
        "PageExtension" => Some("Page"),
        "TableExtension" => Some("Table"),
        "ReportExtension" => Some("Report"),
        "EnumExtension" => Some("Enum"),
        _ => None,
    }
}

/// Given an extension object, find the base object it extends in the symbol table.
/// Returns `None` if the object is not an extension, has no extends target, or the
/// target cannot be found in the symbol table.
fn extends_base_object<'a>(
    obj: &super::l3_workspace::L3Object,
    symbols: &'a SymbolTable,
) -> Option<&'a super::l3_workspace::L3Object> {
    let base_type = extension_base_type(&obj.object_type)?;
    let target_name = obj.extends_target_name.as_deref()?;
    symbols.object_by_type_name(base_type, target_name)
}

/// The `Table` OBJECT id of the implicit `Rec` for an object that has one — a Table
/// (itself), a Page (its `SourceTable`), a TableExtension (the extended table), or a
/// PageExtension (the base page's `SourceTable`). The table reference may be a NAME
/// (native source) or a NUMBER (dep symbols emit the table's object number), so both
/// are resolved. `None` for objects with no implicit record. Used so a BARE call in a
/// Page/Table resolves against the source record's procedures — AL treats an
/// unqualified call in page/table code as an implicit `Rec.<proc>()`.
fn implicit_rec_table_object_id(
    obj: &super::l3_workspace::L3Object,
    symbols: &SymbolTable,
) -> Option<String> {
    let table_ref: String = match obj.object_type.as_str() {
        "Table" => obj.name.clone(),
        "Page" => obj.source_table_name.clone()?,
        "TableExtension" => obj.extends_target_name.clone()?,
        "PageExtension" => {
            let base = symbols.object_by_type_name("Page", obj.extends_target_name.as_deref()?)?;
            base.source_table_name.clone()?
        }
        _ => return None,
    };
    if let Ok(number) = table_ref.trim().parse::<i64>() {
        symbols
            .object_by_type_number("Table", number)
            .map(|o| o.id.clone())
    } else {
        symbols
            .object_by_type_name("Table", &table_ref)
            .map(|o| o.id.clone())
    }
}

/// `routines.map(r => r.id).sort()` — byte-order sort of internal routine ids.
pub(crate) fn sorted_ids(routines: &[&L3Routine]) -> Vec<String> {
    let mut ids: Vec<String> = routines.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// Top-level resolve.
// ---------------------------------------------------------------------------

/// The call resolution every source-only consumer on the analyze path reads:
/// `resolved.precomputed_calls` when set, else a fresh `resolve_calls` over the
/// empty-dependency inputs those sites all use. The `None` path is today's code
/// exactly (an owned value, no clone). A caller that must mutate the result
/// calls `.into_owned()`, which clones only the `Some` case.
pub fn calls_for<'a>(
    resolved: &'a super::l3_workspace::L3Resolved,
    symbols: &SymbolTable,
) -> std::borrow::Cow<'a, ResolvedCalls> {
    match &resolved.precomputed_calls {
        Some(pre) => std::borrow::Cow::Borrowed(pre.as_ref()),
        None => std::borrow::Cow::Owned(resolve_calls(&resolved.workspace, symbols, &[], &[])),
    }
}

/// Resolve ONE call site: its edges and its (possibly upgraded) bindings.
/// The per-site body of [`resolve_calls`], shared with the program adapter
/// (`program_calls`), which uses it for the sites it cannot take from the
/// program engine.
pub(crate) fn resolve_one_call_site(
    routine: &L3Routine,
    call_site: &PCallSite,
    symbols: &SymbolTable,
    diagnostics: &mut Vec<Diagnostic>,
    unfetched_declared_dependency: bool,
) -> (Vec<CallEdge>, Vec<UpgradedBinding>) {
    let mut state = initial_binding_state(call_site);
    let edges = resolve_call_site(
        routine,
        call_site,
        symbols,
        diagnostics,
        unfetched_declared_dependency,
        &mut state,
    );
    (edges, state.bindings)
}

/// Resolve every call site in the workspace into CallEdges (+ implicit-trigger
/// edges), upgrading argument bindings exactly once per callsite. Faithful port
/// of `resolveCalls` + the merge of `buildImplicitTriggerEdges`.
///
/// `primary_dependencies` / `fetched_app_guids` feed the one-time
/// `has_unfetched_declared_dependency` evaluation. In the source-only path pass
/// empty slices → the boolean is false.
pub fn resolve_calls(
    workspace: &L3Workspace,
    symbols: &SymbolTable,
    primary_dependencies: &[DeclaredDependency],
    fetched_app_guids: &[String],
) -> ResolvedCalls {
    let mut edges: Vec<CallEdge> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut upgraded_bindings: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

    // Evaluate the unfetched-dep boolean ONCE.
    let unfetched = has_unfetched_declared_dependency(primary_dependencies, fetched_app_guids);

    for routine in &workspace.routines {
        for call_site in &routine.call_sites {
            let (result, bindings) =
                resolve_one_call_site(routine, call_site, symbols, &mut diagnostics, unfetched);
            edges.extend(result);
            // Capture the (possibly upgraded) bindings for this callsite. Only
            // callsites with ≥1 binding are meaningful; store all so the dump can
            // decide whether to emit.
            upgraded_bindings.insert(call_site.id.clone(), bindings);
        }
    }

    // Merge implicit-trigger edges (read-only; same internal-id shape).
    let implicit = build_implicit_trigger_edges(workspace, symbols);
    for e in implicit {
        edges.push(e);
    }

    ResolvedCalls {
        edges,
        upgraded_bindings,
        diagnostics,
    }
}
