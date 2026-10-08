//! The call-resolution SHAPE the detector model carries: edges, upgraded argument
//! bindings, diagnostics, declared dependencies.
//!
//! Moved verbatim out of `program::model::calls` in engine-switch S2b.2: the
//! model ([`super::workspace::Model::calls`]) holds a [`ResolvedCalls`], and
//! the program engine fills it (`program_calls`, the B3 adapter). The legacy L3
//! resolver that also produced this shape was deleted in S9.6.

use super::taxonomy::{DispatchKind, Resolution};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Edge model (the resolver's internal-id shape).
// ---------------------------------------------------------------------------

/// Interface-dispatch metadata, attached to ONLY the first emitted edge (after
/// the sort-by-`to`) or the single unknown edge. The dump lifts this to the
/// callsite-group level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchMeta {
    pub interface_name: String,
    pub total_impls: usize,
    /// (internal objectId, reason) for each impl that did not resolve.
    pub unresolved_impls: Vec<(String, String)>,
    /// internal object ids of enum implementers (metadata only).
    pub enum_implementers: Vec<String>,
}

/// An external (out-of-index) type reference on a member edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTypeRef {
    pub kind: String,
    pub name: String,
}

/// Why a `resolution == "unknown"` edge could not be resolved. DIAGNOSTIC-only
/// metadata (never projected to a golden — `CallEdge` is not `Serialize`); it lets
/// `aldump --l3-unknown-breakdown` attribute the residual real-`unknown` rate to
/// its causes, which is the work-list for the later typed-resolution phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    /// Bare call, not own-object, not a global builtin.
    BareUnresolved,
    /// Member call whose receiver is a compound expression (`a.b.M()`, `(x).M()`,
    /// indexed) — `simple_receiver_name` declined it.
    CompoundReceiver,
    /// Member call whose receiver name is not a local/param/global in the routine
    /// (object globals not captured, `CurrPage`/`CurrReport`, return-value chains).
    UntrackedReceiver,
    /// Member call on a `Record`-typed receiver whose method is NOT a builtin — a
    /// real table procedure (resolvable by the later Record-dispatch phase).
    RecordTableProcedure,
    /// Member call on a RecordRef/FieldRef/KeyRef/framework receiver whose method is
    /// not in the intrinsic catalog (a catalog gap to fill).
    FrameworkMethodNotInCatalog,
    /// Member call whose declared receiver type is a primitive / Variant /
    /// unrecognized type (no object, no catalog kind).
    NonObjectReceiverType,
    /// Member call on an enum-typed receiver (enum statics are not callable here).
    EnumStatic,
    /// The L2 callee itself could not be parsed (`PCallee::Unknown`).
    CalleeUnknown,
    /// Interface dispatch where NO implementer resolved (open-world / no impls).
    InterfaceNoImpl,
    /// Object-run whose target is a dynamic variable (not a static ref) -- the
    /// dispatch kind is `dynamic`; target is unknowable without runtime info.
    DynamicObjectRunTarget,
    /// Member call on a `Variant`-typed receiver -- the held type (and the dispatch)
    /// is RUNTIME-determined. Emitted with `dispatch_kind == Dynamic` so it classifies
    /// `dynamic` (NOT real-`unknown`): genuinely indeterminate, not a failure.
    DynamicReceiver,
    /// The program engine produced no usable edge for this call site (no edge at
    /// its span, a shape/callee/caller mismatch, or a workspace callee the model
    /// has no routine for). Engine-switch S3: these used to fall back to the legacy
    /// resolver; now they are an honest unknown.
    NoProgramSite,
}

impl UnknownReason {
    /// Stable kebab-case label for the diagnostic breakdown histogram.
    pub fn label(self) -> &'static str {
        match self {
            UnknownReason::BareUnresolved => "bare-unresolved",
            UnknownReason::CompoundReceiver => "compound-receiver",
            UnknownReason::UntrackedReceiver => "untracked-receiver",
            UnknownReason::RecordTableProcedure => "record-table-procedure",
            UnknownReason::FrameworkMethodNotInCatalog => "framework-method-not-in-catalog",
            UnknownReason::NonObjectReceiverType => "non-object-receiver-type",
            UnknownReason::EnumStatic => "enum-static",
            UnknownReason::CalleeUnknown => "callee-unknown",
            UnknownReason::InterfaceNoImpl => "interface-no-impl",
            UnknownReason::DynamicObjectRunTarget => "dynamic-objectrun-target",
            UnknownReason::DynamicReceiver => "dynamic-receiver",
            UnknownReason::NoProgramSite => "no-program-site",
        }
    }
}

/// A resolved (or unresolved) call edge. Ids are INTERNAL until projected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEdge {
    pub from: String,
    pub to: Option<String>,
    pub callsite_id: String,
    pub operation_id: String,
    pub dispatch_kind: DispatchKind,
    pub resolution: Resolution,
    /// candidates (internal routine ids), for ambiguous / member-not-found.
    pub candidates: Option<Vec<String>>,
    pub external_type_ref: Option<ExternalTypeRef>,
    /// method-dispatch receiver's declared type.
    pub receiver_type: Option<String>,
    pub dispatch_meta: Option<DispatchMeta>,
    /// For `FrameworkMethodNotInCatalog` unknown edges: the `"Kind::method_lc"`
    /// detail string that identifies the catalog gap. `None` on all other edges.
    pub unknown_method_name: Option<String>,
    /// DIAGNOSTIC-only receiver shape tag for `UntrackedReceiver` /
    /// `CompoundReceiver` / `RecordTableProcedure` edges — sub-characterizes the
    /// bucket so `--l3-unknown-breakdown` can attribute `implicit-rec`, `currpage`,
    /// `currreport`, `member-of-member`, `call-result`, `indexed`, and (for record
    /// table procedures) `table-unresolved::<declType>::<method>` vs
    /// `proc-not-found::<declType>::<method>`. `None` on all other edges.
    pub receiver_shape: Option<String>,
}

impl CallEdge {
    pub(crate) fn base(from: &str, callsite_id: &str, operation_id: &str) -> CallEdge {
        CallEdge {
            from: from.to_string(),
            to: None,
            callsite_id: callsite_id.to_string(),
            operation_id: operation_id.to_string(),
            dispatch_kind: DispatchKind::Unresolved,
            resolution: Resolution::Unknown(UnknownReason::CalleeUnknown),
            candidates: None,
            external_type_ref: None,
            receiver_type: None,
            dispatch_meta: None,
            unknown_method_name: None,
            receiver_shape: None,
        }
    }
}

/// The post-upgrade state of one argument binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradedBinding {
    pub parameter_index: u32,
    pub callee_parameter_is_var: bool,
    /// "non-record-arg" | "unresolved-callee" | "resolved" | "ambiguous".
    pub binding_resolution: String,
}

/// A diagnostic (the resolver only emits the double-upgrade warning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
}

/// A declared dependency (the L3-relevant subset of al-sem's ManifestDependency).
#[derive(Debug, Clone)]
pub struct DeclaredDependency {
    pub app_guid: String,
}

/// Which dependency routine a site reaches, and the state of its body
/// (engine-switch S3.3). For a call site the edge itself stays to-less (the model
/// holds workspace routines only); this keeps the target's identity for S7/S8 and
/// says whether an empty fact set for it could ever mean "no effects" (it cannot,
/// unless `AnalyzedClean`). Since S3.6 it also holds every dependency candidate of
/// an ambiguous overload, and every dependency table trigger a record operation
/// reaches; those operations have no edge (`callsite_id` is then the operation's
/// id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTargetRef {
    pub callsite_id: String,
    /// `"{app guid}/{type}/{number}::{routine name, folded}/{arity}"`; a field
    /// trigger's name is `"{field, folded}::{trigger}"`.
    pub target: String,
    /// `None` when the program graph has no routine node with that identity. In
    /// practice that is the resolver's placeholder entry-trigger key of a run into
    /// a dependency object that declares no entry trigger (on CDO/DO: only
    /// `OnOpenPage` of Base/System Application pages): no routine exists there.
    pub body: Option<crate::program::registry::BodyState>,
}

/// The full call-resolution result: every edge + the per-callsite upgraded
/// bindings (keyed by internal callsite id) + diagnostics.
#[derive(Clone, Default)]
pub struct ResolvedCalls {
    pub edges: Vec<CallEdge>,
    /// internal callsite id → upgraded argument bindings (in argument order).
    pub upgraded_bindings: HashMap<String, Vec<UpgradedBinding>>,
    pub diagnostics: Vec<Diagnostic>,
    /// The dependency routines this resolution's to-less dependency edges reach
    /// (S3.3), in edge order. Empty from the legacy resolver.
    pub external_targets: Vec<ExternalTargetRef>,
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
// Upgraded-binding side table (the `upgradeBindings` mutation, captured out of
// band because the model's PCallArgumentBinding does not carry the upgrade
// fields). Moved from `program::model::calls` in engine-switch S9.4.
// ---------------------------------------------------------------------------

/// Per-callsite upgraded bindings. `upgraded` guards `upgrade_bindings` so it
/// runs EXACTLY once per callsite (reproducing al-sem's double-upgrade guard).
pub(crate) struct BindingState {
    pub(crate) bindings: Vec<UpgradedBinding>,
}

/// Derive the INITIAL bindingResolution for a callsite's bindings, matching
/// al-sem's `intraprocedural-body.ts` construction:
///   - non-identifier arg (sourceKind "expression") → "non-record-arg"
///   - identifier bound to a record variable → "unresolved-callee" (upgradable)
///   - any other identifier (param / implicit-rec / unknown) → "non-record-arg"
///
/// `calleeParameterIsVar` starts `false` (upgraded later).
pub(crate) fn initial_binding_state(
    call_site: &crate::program::body::features::PCallSite,
) -> BindingState {
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
    callee: &super::workspace::ModelRoutine,
    callsite_id: &str,
) -> Option<Diagnostic> {
    upgrade_bindings_with(
        state,
        |i| callee.parameters.get(i).map(|p| p.is_var),
        callsite_id,
    )
}

/// [`upgrade_bindings`] from the callee's per-parameter `var` flags alone
/// (`None` past its last parameter), for a callee with no model routine (a
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
