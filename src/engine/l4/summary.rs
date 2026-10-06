//! L4 summary core types + projection (R3a-2).
//!
//! Ports al-sem's `src/model/summary.ts` (RoutineSummary / DbEffect /
//! Uncertainty / RecordRoleSummary) and `scripts/r3a2-projection.ts`
//! (projectR3a2 / stable-id mapping). The per-pass Jacobi fingerprint TRACE
//! projection retired with the old Jacobi solver (Part B); the closed-form v2
//! `EffectStore` solver has no per-pass trajectory to trace.
//!
//! HARD-FORBIDDEN in the R3a-2 projection: `capabilityFactsDirect` /
//! `capabilityFactsInherited` / `coverage` (R3a-3 cone), `fieldEffects`
//! (lazy/detector), the dep-hook output (R3a-4). These are never declared
//! on the projected types so they cannot appear.

use serde::{Deserialize, Serialize};

use super::effect_lattice::{EffectPresence, TempStateKind, effect_key_of};
use super::summary_runner::compute_summaries_v2_with_leaves_core;
use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l4::combined_graph::build_combined_graph;
use crate::engine::l4::scc::{SccInputGraph, tarjan_scc};

// ---------------------------------------------------------------------------
// Internal summary core types (NOT the serde projection shape). Internal ids.
// ---------------------------------------------------------------------------

/// The temp-state of a record operation (internal form — NOT the serde
/// projection shape). Mirrors al-sem `TempState`.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum TempState {
    Known(bool),
    ParameterDependent(u32),
    Unknown,
}

impl TempState {
    pub fn from_p(ts: &crate::engine::l2::features::PTempState) -> Self {
        match ts.kind.as_str() {
            "known" => TempState::Known(ts.value.unwrap_or(false)),
            "parameter-dependent" => TempState::ParameterDependent(ts.parameter_index.unwrap_or(0)),
            _ => TempState::Unknown,
        }
    }

    pub fn to_kind(&self) -> TempStateKind {
        match self {
            TempState::Known(v) => TempStateKind::Known(*v),
            TempState::ParameterDependent(i) => TempStateKind::ParameterDependent(*i),
            TempState::Unknown => TempStateKind::Unknown,
        }
    }
}

/// One de-duplicated DB effect (internal form). Mirrors al-sem `DbEffect`.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct DbEffect {
    /// `effectKeyOf(op, tableId, operationId, tempState)` — EXCLUDES via.
    pub effect_key: String,
    pub operation_id: String,
    pub op: String,
    pub table_id: String, // "unknown" or internal form
    pub record_variable_id: Option<String>,
    pub temp_state: TempState,
    /// "direct" | "inherited" | "implicit-trigger" | "event-subscriber" | "dynamic"
    pub via: String,
}

/// One uncertainty (internal form). Mirrors al-sem `Uncertainty`.
///
/// `Hash` is derived (field-wise, consistent with the derived `PartialEq`) so the
/// value can key an intern map — see
/// [`crate::engine::l5::d1_cohort::UncertaintyTable`], which stores one copy of
/// each distinct uncertainty for a whole d1 run and hands out [`u32`] ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct Uncertainty {
    pub kind: String,
    pub callsite_id: Option<String>,
    pub operation_id: Option<String>,
    pub routine_id: Option<String>,
    pub interface_name: Option<String>,
}

/// The combined-graph `UncertaintyEdge` carries its own structurally-identical
/// `Uncertainty` (`combined_graph::Uncertainty`). They are the SAME al-sem
/// `Uncertainty` shape modelled in two modules (the L4 summary core vs. the
/// to-less combined-graph edge). This converts an edge uncertainty into the
/// summary form so the path-walker (which consumes `summary::Uncertainty`) can
/// union the summary-carried and edge-carried sources with one type. Field-for-
/// field copy — no information is lost or gained.
impl From<&crate::engine::l4::combined_graph::Uncertainty> for Uncertainty {
    fn from(u: &crate::engine::l4::combined_graph::Uncertainty) -> Self {
        Uncertainty {
            kind: u.kind.clone(),
            callsite_id: u.callsite_id.clone(),
            operation_id: u.operation_id.clone(),
            routine_id: u.routine_id.clone(),
            interface_name: u.interface_name.clone(),
        }
    }
}

/// The descriptive id an [`Uncertainty`] is reported *at*: `callsiteId`, else
/// `operationId`, else `routineId`, else `""`. Mirrors al-sem `describe(u)`'s
/// id-precedence.
///
/// This is the SAME precedence [`uncertainty_key`] uses — enforced structurally,
/// because that function is now written in terms of this one rather than
/// repeating the `if let` chain. That identity,
/// `uncertainty_key(u) == format!("{}|{}", u.kind, uncertainty_at(u))`, is what
/// lets [`crate::engine::l5::d1_cohort::UncertaintyTable`] carry a cohort's
/// uncertainty union as ids: the de-dup key and the confidence mapper's
/// projection read the same pair of fields.
///
/// Five detectors (d1, d2, d3, d46, d48) previously each carried their own copy
/// of this chain, cloning both `String`s per uncertainty; they now all go through
/// [`crate::engine::l5::confidence::UncertaintyLite::of`], which borrows here.
pub fn uncertainty_at(u: &Uncertainty) -> &str {
    if let Some(cs) = &u.callsite_id {
        return cs;
    }
    if let Some(op) = &u.operation_id {
        return op;
    }
    u.routine_id.as_deref().unwrap_or("")
}

/// Stable key for an Uncertainty — mirrors al-sem `uncertaintyKey`.
pub fn uncertainty_key(u: &Uncertainty) -> String {
    format!("{}|{}", u.kind, uncertainty_at(u))
}

/// De-duplicate a list of [`Uncertainty`] values by key, then sort by key. Mirrors
/// al-sem `dedupeUncertainties` (uncertainty-util.ts) EXACTLY: al-sem builds a JS
/// `Map` with `byKey.set(key, u)` in order — so on a key collision the LAST value
/// wins — then emits `[...byKey.values()].sort(byKey)`. A `BTreeMap` reproduces both:
/// `insert` is last-write-wins and iteration is key-sorted (byte order == al-sem's
/// ASCII-key `compareStrings`). (Keep-first would diverge only for same-key
/// `interface-open-world` uncertainties with differing `interfaceName`, but matching
/// keep-last removes the reliance on that one-interface-per-callsite invariant.)
///
/// ## Why this is a sort, not a `BTreeMap<String, _>`
///
/// The `BTreeMap` form built ONE `format!("{}|{}", kind, at)` key `String` per
/// element. `ALSEM_SUMMARIES_CENSUS=1` measured 3,708,222 elements passing
/// through this function per BC Base App 8020 run (inside `solve_side_facts`
/// alone) — i.e. 3.7 M heap allocations whose only purpose was to be compared and
/// dropped. [`cmp_uncertainty_key`] compares the same key without materializing
/// it, so the map is replaced by a stable sort plus a keep-LAST pass. The
/// contract is unchanged and reproduced exactly:
///
/// - **Order**: the sort is on the CONCATENATED `"kind|at"` byte sequence (see
///   [`cmp_uncertainty_key`] — a `(kind, at)` TUPLE comparison would be a
///   different order, because `'|'` (0x7C) outranks most identifier bytes).
/// - **Last-write-wins**: `sort_by` is stable, so an equal-key run keeps its
///   original insertion order; taking the LAST element of each run is exactly
///   what `BTreeMap::insert`'s overwrite did.
pub(crate) fn dedupe_uncertainties(mut list: Vec<Uncertainty>) -> Vec<Uncertainty> {
    list.sort_by(cmp_uncertainty_key);
    // Keep the LAST element of each equal-key run. `dedup_by` removes the element
    // passed as `a` (the LATER one) when the closure returns true, so returning
    // `keys equal` would keep the FIRST — hence the swap, which makes the
    // survivor of each run the last-inserted one.
    list.dedup_by(|a, b| {
        if cmp_uncertainty_key(a, b) == std::cmp::Ordering::Equal {
            std::mem::swap(a, b);
            true
        } else {
            false
        }
    });
    list
}

/// Compare two uncertainties by [`uncertainty_key`] WITHOUT building it: the
/// key is `format!("{}|{}", u.kind, uncertainty_at(u))`, so its byte sequence is
/// `kind` bytes, then `b'|'`, then `at` bytes. Chaining the iterators compares
/// that exact sequence with no allocation.
///
/// Comparing the `(kind, at)` pair as a tuple instead would NOT be the same
/// order — `("a", "b")` < `("ab", "c")` as a tuple, while `"a|b"` > `"ab|c"` as
/// a string, because `'|'` (0x7C) > `'b'` (0x62). The goldens are on the string
/// order, so this reproduces the string order.
pub(crate) fn cmp_uncertainty_key(a: &Uncertainty, b: &Uncertainty) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ka, kb) = (a.kind.as_bytes(), b.kind.as_bytes());
    let n = ka.len().min(kb.len());
    // Slice compare lowers to `memcmp`; the byte-at-a-time iterator form this
    // replaced was measurably slower than the `format!` allocations it removed
    // (`side_facts`' assemble phase moved 21.8 % -> 24.6 % of the span on it).
    match ka[..n].cmp(&kb[..n]) {
        Ordering::Equal => {}
        ord => return ord,
    }
    // Common prefix. Whichever key's `kind` ran out continues with `'|'`; the
    // other continues with its next `kind` byte.
    match ka.len().cmp(&kb.len()) {
        Ordering::Equal => uncertainty_at(a)
            .as_bytes()
            .cmp(uncertainty_at(b).as_bytes()),
        Ordering::Less => match b'|'.cmp(&kb[n]) {
            Ordering::Equal => cmp_uncertainty_key_bytewise(a, b),
            ord => ord,
        },
        Ordering::Greater => match ka[n].cmp(&b'|') {
            Ordering::Equal => cmp_uncertainty_key_bytewise(a, b),
            ord => ord,
        },
    }
}

/// The fallback [`cmp_uncertainty_key`] defers to when a `kind` itself contains
/// the `'|'` separator at exactly the position where the shorter kind ends — the
/// one case the fast path cannot decide from a single byte. No AL uncertainty
/// kind contains `'|'` (they are fixed literals like `opaque-callee`), so this is
/// unreachable in practice; it exists so the fast path is an OPTIMIZATION of the
/// key order rather than a redefinition of it.
fn cmp_uncertainty_key_bytewise(a: &Uncertainty, b: &Uncertainty) -> std::cmp::Ordering {
    let seq = |u: &Uncertainty| {
        u.kind
            .as_bytes()
            .iter()
            .copied()
            .chain(std::iter::once(b'|'))
            .chain(uncertainty_at(u).as_bytes().iter().copied())
            .collect::<Vec<u8>>()
    };
    seq(a).cmp(&seq(b))
}

/// Per-record-parameter role summary (internal form). Mirrors al-sem
/// `RecordRoleSummary`.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct RecordRoleSummary {
    pub parameter_index: u32,
    pub table_id: String,
    pub reads_fields: FieldList,
    pub writes_fields: FieldList,
    pub may_reset_filters: bool,
    pub may_change_load_fields: bool,
    pub may_assign_record: bool,
    pub may_use_record_ref: bool,
    // Entry requirements
    pub requires_loaded_at_entry: EffectPresence,
    pub required_loaded_fields_at_entry: FieldList,
    pub mutates_before_load: EffectPresence,
    // Exit effects
    pub persists_current_record: EffectPresence,
    pub set_based_db_writes: EffectPresence,
    pub validates_param: EffectPresence,
    pub copies_into_param: EffectPresence,
    pub resets_filters_on_param: EffectPresence,
    pub dirty_at_exit: EffectPresence,
    pub current_loaded_fields_at_exit: FieldList,
    // Convenience derivations
    pub mutates_param: EffectPresence,
    pub loads_from_db_param: EffectPresence,
    pub initialises_param: EffectPresence,
}

impl RecordRoleSummary {
    /// The routine may leave this record in a loaded/initialised state: it loads
    /// it from the database, initialises it, or copies into it (the
    /// `loadsFromDb` / `initialises` / `copiesInto` record-flow roles). A MAY
    /// fact, which the walker and d40 read leniently as "loaded".
    pub fn puts_in_loaded_state(&self) -> bool {
        self.loads_from_db_param == EffectPresence::Yes
            || self.initialises_param == EffectPresence::Yes
            || self.copies_into_param == EffectPresence::Yes
    }
}

/// A field list value: a sorted list of field ids, or a sentinel.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum FieldList {
    Known(Vec<String>),
    Unknown,
    Full,
}

/// One routine summary core (internal form). Mirrors al-sem `RoutineSummary`
/// (CORE fields only — capabilityFacts/coverage/fieldEffects excluded).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct RoutineSummary {
    pub routine_id: String,
    pub db_effects: Vec<DbEffect>,
    pub in_recursive_cycle: bool,
    pub has_unresolved_calls: bool,
    pub uncertainties: Vec<Uncertainty>,
    pub parameter_roles: Vec<RecordRoleSummary>,
}

// ---------------------------------------------------------------------------
// Projected (stable-id) types — the R3a-2 comparison surface.
// Matches scripts/r3a2-projection.ts field-for-field.
// ---------------------------------------------------------------------------

/// Projected TempState (in stable-id serialization form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum PDbEffectTempState {
    #[serde(rename = "known")]
    Known { value: bool },
    #[serde(rename = "parameter-dependent")]
    ParameterDependent {
        #[serde(rename = "parameterIndex")]
        parameter_index: u32,
    },
    #[serde(rename = "unknown")]
    Unknown,
}

impl PDbEffectTempState {
    fn from_temp_state(ts: &TempState) -> Self {
        match ts {
            TempState::Known(v) => PDbEffectTempState::Known { value: *v },
            TempState::ParameterDependent(i) => PDbEffectTempState::ParameterDependent {
                parameter_index: *i,
            },
            TempState::Unknown => PDbEffectTempState::Unknown,
        }
    }
}

/// Projected field list (stable-id form).
pub type PFieldList = serde_json::Value;

/// Projected DbEffect (stable-id form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PDbEffect {
    #[serde(rename = "effectKey")]
    pub effect_key: String,
    pub op: String,
    #[serde(rename = "tableId")]
    pub table_id: String,
    #[serde(rename = "operationId")]
    pub operation_id: String,
    #[serde(rename = "tempState")]
    pub temp_state: PDbEffectTempState,
    pub via: String,
}

/// Projected Uncertainty (stable-id form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PUncertainty {
    pub kind: String,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "routineId", skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<String>,
    #[serde(rename = "interfaceName", skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
}

/// Stable key for a projected Uncertainty.
pub fn p_uncertainty_key(u: &PUncertainty) -> String {
    if let Some(cs) = &u.callsite_id {
        return format!("{}|{}", u.kind, cs);
    }
    if let Some(op) = &u.operation_id {
        return format!("{}|{}", u.kind, op);
    }
    format!("{}|{}", u.kind, u.routine_id.as_deref().unwrap_or(""))
}

/// Projected RecordRoleSummary (stable-id form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PRecordRoleSummary {
    #[serde(rename = "parameterIndex")]
    pub parameter_index: u32,
    #[serde(rename = "tableId")]
    pub table_id: String,
    #[serde(rename = "readsFields")]
    pub reads_fields: PFieldList,
    #[serde(rename = "writesFields")]
    pub writes_fields: PFieldList,
    #[serde(rename = "mayResetFilters")]
    pub may_reset_filters: bool,
    #[serde(rename = "mayChangeLoadFields")]
    pub may_change_load_fields: bool,
    #[serde(rename = "mayAssignRecord")]
    pub may_assign_record: bool,
    #[serde(rename = "mayUseRecordRef")]
    pub may_use_record_ref: bool,
    #[serde(rename = "requiresLoadedAtEntry")]
    pub requires_loaded_at_entry: String,
    #[serde(rename = "requiredLoadedFieldsAtEntry")]
    pub required_loaded_fields_at_entry: PFieldList,
    #[serde(rename = "mutatesBeforeLoad")]
    pub mutates_before_load: String,
    #[serde(rename = "persistsCurrentRecord")]
    pub persists_current_record: String,
    #[serde(rename = "setBasedDbWrites")]
    pub set_based_db_writes: String,
    #[serde(rename = "validatesParam")]
    pub validates_param: String,
    #[serde(rename = "copiesIntoParam")]
    pub copies_into_param: String,
    #[serde(rename = "resetsFiltersOnParam")]
    pub resets_filters_on_param: String,
    #[serde(rename = "dirtyAtExit")]
    pub dirty_at_exit: String,
    #[serde(rename = "currentLoadedFieldsAtExit")]
    pub current_loaded_fields_at_exit: PFieldList,
    #[serde(rename = "mutatesParam")]
    pub mutates_param: String,
    #[serde(rename = "loadsFromDbParam")]
    pub loads_from_db_param: String,
    #[serde(rename = "initialisesParam")]
    pub initialises_param: String,
}

/// Projected RoutineSummary CORE (stable-id form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, salsa::Update)]
pub struct PRoutineSummaryCore {
    #[serde(rename = "routineId")]
    pub routine_id: String,
    #[serde(rename = "dbEffects")]
    pub db_effects: Vec<PDbEffect>,
    pub uncertainties: Vec<PUncertainty>,
    #[serde(rename = "parameterRoles")]
    pub parameter_roles: Vec<PRecordRoleSummary>,
    #[serde(rename = "inRecursiveCycle")]
    pub in_recursive_cycle: bool,
    #[serde(rename = "hasUnresolvedCalls")]
    pub has_unresolved_calls: bool,
}

/// The R3a-2 stable projection of the post-computeSummaries model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct R3a2Projection {
    pub summaries: Vec<PRoutineSummaryCore>,
}

// ---------------------------------------------------------------------------
// Stable-id mapping helpers. Mirror scripts/r3a2-projection.ts exactly.
// ---------------------------------------------------------------------------

/// Build internal-RoutineId → StableRoutineId from the workspace routines.
pub(crate) fn build_routine_stable_map(
    routines: &[crate::engine::l3::l3_workspace::L3Routine],
) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    for r in routines {
        m.insert(r.id.clone(), r.stable_routine_id.clone());
    }
    m
}

/// Convert an internal RoutineId to StableRoutineId; pass through if unmapped.
pub(crate) fn stable_routine_id(
    internal: &str,
    map: &std::collections::HashMap<String, String>,
) -> String {
    map.get(internal)
        .cloned()
        .unwrap_or_else(|| internal.to_string())
}

/// Rewrite `${routineId}/<suffix>` (callsiteId `/csN`, operationId `/opN`) into
/// stable form. Internal RoutineId is `${modelInstanceId}/${hash}` (exactly two
/// `/`-separated parts), so the suffix is everything after the SECOND `/`.
/// Mirrors `stableSubId` in r3a2-projection.ts.
pub(crate) fn stable_sub_id(
    internal_sub_id: &str,
    map: &std::collections::HashMap<String, String>,
) -> String {
    // The internal sub-id looks like `<modelInstanceId>/<hash>/<suffix>`.
    // We need to split off the first two `/`-parts as the routineId.
    let first_slash = internal_sub_id.find('/');
    let second_slash =
        first_slash.and_then(|f| internal_sub_id[f + 1..].find('/').map(|s| f + 1 + s));
    match (first_slash, second_slash) {
        (Some(_), Some(sec)) => {
            let routine_id = &internal_sub_id[..sec];
            let suffix = &internal_sub_id[sec..]; // includes leading "/"
            match map.get(routine_id) {
                Some(stable) => format!("{stable}{suffix}"),
                None => internal_sub_id.to_string(),
            }
        }
        _ => internal_sub_id.to_string(),
    }
}

/// Project an internal TableId to stable form.
/// Internal: `${appGuid}/table/${N}` → `${appGuid}:Table:${N}`.
/// `"unknown"` passes through.
fn stable_table_id(internal: &str) -> String {
    if internal == "unknown" {
        return "unknown".to_string();
    }
    let parts: Vec<&str> = internal.split('/').collect();
    if parts.len() == 3 && parts[1] == "table" {
        format!("{}:Table:{}", parts[0], parts[2])
    } else {
        internal.to_string()
    }
}

/// Project an internal FieldId to stable form. Mirrors al-sem
/// `toStableFieldId` (src/model/stable-identity.ts): the internal FieldId is
/// `${tableId}/${fieldNumber}` (e.g. `${appGuid}/table/${N}/${M}`); split on
/// the LAST slash into the internal TableId + the field number, then convert
/// the table id to stable form: `${stableTableId}#${fieldNumber}`.
fn stable_field_id(internal: &str) -> String {
    match internal.rfind('/') {
        Some(last_slash) if last_slash > 0 => {
            let table_internal = &internal[..last_slash];
            let field_num = &internal[last_slash + 1..];
            format!("{}#{}", stable_table_id(table_internal), field_num)
        }
        _ => internal.to_string(),
    }
}

fn project_field_list_id(fl: &FieldList) -> PFieldList {
    match fl {
        FieldList::Unknown => serde_json::Value::String("unknown".to_string()),
        FieldList::Full => serde_json::Value::String("full".to_string()),
        FieldList::Known(fields) => {
            let mut stable: Vec<String> = fields.iter().map(|f| stable_field_id(f)).collect();
            stable.sort();
            serde_json::Value::Array(stable.into_iter().map(serde_json::Value::String).collect())
        }
    }
}

/// Project a field-name list (requiredLoadedFieldsAtEntry /
/// currentLoadedFieldsAtExit) — these are opaque strings, keep order.
fn project_field_name_list(fl: &FieldList) -> PFieldList {
    match fl {
        FieldList::Unknown => serde_json::Value::String("unknown".to_string()),
        FieldList::Full => serde_json::Value::String("full".to_string()),
        FieldList::Known(names) => serde_json::Value::Array(
            names
                .iter()
                .map(|n| serde_json::Value::String(n.clone()))
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Per-routine projection helpers.
// ---------------------------------------------------------------------------

fn project_db_effect(e: &DbEffect, map: &std::collections::HashMap<String, String>) -> PDbEffect {
    let table_id = stable_table_id(&e.table_id);
    let operation_id = stable_sub_id(&e.operation_id, map);
    let temp_state = e.temp_state.to_kind();
    let effect_key = effect_key_of(&e.op, &table_id, &operation_id, &temp_state);
    PDbEffect {
        effect_key,
        op: e.op.clone(),
        table_id,
        operation_id,
        temp_state: PDbEffectTempState::from_temp_state(&e.temp_state),
        via: e.via.clone(),
    }
}

fn project_uncertainty(
    u: &Uncertainty,
    map: &std::collections::HashMap<String, String>,
) -> PUncertainty {
    PUncertainty {
        kind: u.kind.clone(),
        callsite_id: u.callsite_id.as_ref().map(|c| stable_sub_id(c, map)),
        operation_id: u.operation_id.as_ref().map(|o| stable_sub_id(o, map)),
        routine_id: u.routine_id.as_ref().map(|r| stable_routine_id(r, map)),
        interface_name: u.interface_name.clone(),
    }
}

fn project_record_role(r: &RecordRoleSummary) -> PRecordRoleSummary {
    PRecordRoleSummary {
        parameter_index: r.parameter_index,
        table_id: stable_table_id(&r.table_id),
        reads_fields: project_field_list_id(&r.reads_fields),
        writes_fields: project_field_list_id(&r.writes_fields),
        may_reset_filters: r.may_reset_filters,
        may_change_load_fields: r.may_change_load_fields,
        may_assign_record: r.may_assign_record,
        may_use_record_ref: r.may_use_record_ref,
        requires_loaded_at_entry: r.requires_loaded_at_entry.as_str().to_string(),
        required_loaded_fields_at_entry: project_field_name_list(
            &r.required_loaded_fields_at_entry,
        ),
        mutates_before_load: r.mutates_before_load.as_str().to_string(),
        persists_current_record: r.persists_current_record.as_str().to_string(),
        set_based_db_writes: r.set_based_db_writes.as_str().to_string(),
        validates_param: r.validates_param.as_str().to_string(),
        copies_into_param: r.copies_into_param.as_str().to_string(),
        resets_filters_on_param: r.resets_filters_on_param.as_str().to_string(),
        dirty_at_exit: r.dirty_at_exit.as_str().to_string(),
        current_loaded_fields_at_exit: project_field_name_list(&r.current_loaded_fields_at_exit),
        mutates_param: r.mutates_param.as_str().to_string(),
        loads_from_db_param: r.loads_from_db_param.as_str().to_string(),
        initialises_param: r.initialises_param.as_str().to_string(),
    }
}

/// Public projector for one internal RoutineSummary CORE → the stable R3a-2 shape.
/// Used by the R3a-5 cross-app full-summary projection (which composes the R3a-2
/// core with the R3a-3 cone over the MERGED model). The `map` covers BOTH primary
/// and dep routine ids (every merged L3Routine carries `stable_routine_id`).
pub fn project_routine_summary_core_pub(
    s: &RoutineSummary,
    map: &std::collections::HashMap<String, String>,
) -> PRoutineSummaryCore {
    project_routine_summary_core(s, map)
}

/// Public alias used by `summary_runner` to project an internal summary to
/// stable form for the roles-only fixpoint's `roles_change_key` convergence
/// signal (the trace oracle this originally served retired with the old
/// Jacobi solver — see `summary_runner`'s module doc). The `routine_id` arg
/// is ignored (the id comes from `s.routine_id`); it exists only for
/// call-site symmetry with the internal helper.
pub fn project_routine_summary_core_internal(
    _routine_id: &str,
    s: &RoutineSummary,
    map: &std::collections::HashMap<String, String>,
) -> PRoutineSummaryCore {
    project_routine_summary_core(s, map)
}

fn project_routine_summary_core(
    s: &RoutineSummary,
    map: &std::collections::HashMap<String, String>,
) -> PRoutineSummaryCore {
    let mut db_effects: Vec<PDbEffect> = s
        .db_effects
        .iter()
        .map(|e| project_db_effect(e, map))
        .collect();
    db_effects.sort_by(|a, b| {
        a.effect_key
            .cmp(&b.effect_key)
            .then_with(|| a.operation_id.cmp(&b.operation_id))
    });

    let mut uncertainties: Vec<PUncertainty> = s
        .uncertainties
        .iter()
        .map(|u| project_uncertainty(u, map))
        .collect();
    uncertainties.sort_by_key(p_uncertainty_key);

    let mut parameter_roles: Vec<PRecordRoleSummary> =
        s.parameter_roles.iter().map(project_record_role).collect();
    parameter_roles.sort_by_key(|r| r.parameter_index);

    PRoutineSummaryCore {
        routine_id: stable_routine_id(&s.routine_id, map),
        db_effects,
        uncertainties,
        parameter_roles,
        in_recursive_cycle: s.in_recursive_cycle,
        has_unresolved_calls: s.has_unresolved_calls,
    }
}

// ---------------------------------------------------------------------------
// Stable fingerprint for the TRACE oracle (mirrors stableSummaryFingerprint).
// ---------------------------------------------------------------------------

pub fn stable_summary_fingerprint(s: &PRoutineSummaryCore) -> String {
    // Mirrors scripts/r3a2-projection.ts stableSummaryFingerprint EXACTLY:
    // JSON.stringify([
    //   s.dbEffects.map(e => `${e.effectKey}:${e.via}`),
    //   s.hasUnresolvedCalls,
    //   s.uncertainties.map(uncertaintyPKey),
    //   s.parameterRoles.map(r => [...]),
    // ])
    let db_effects_arr: Vec<serde_json::Value> = s
        .db_effects
        .iter()
        .map(|e| serde_json::Value::String(format!("{}:{}", e.effect_key, e.via)))
        .collect();

    let uncertainties_arr: Vec<serde_json::Value> = s
        .uncertainties
        .iter()
        .map(|u| serde_json::Value::String(p_uncertainty_key(u)))
        .collect();

    // FieldList helpers for fingerprint.
    fn field_list_fp(v: &serde_json::Value) -> serde_json::Value {
        // If it's an array, join with comma.
        if let serde_json::Value::Array(arr) = v {
            let joined: String = arr
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(",");
            serde_json::Value::String(joined)
        } else {
            v.clone()
        }
    }

    let param_roles_arr: Vec<serde_json::Value> = s
        .parameter_roles
        .iter()
        .map(|r| {
            serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(r.parameter_index)),
                serde_json::Value::String(r.loads_from_db_param.clone()),
                serde_json::Value::String(r.initialises_param.clone()),
                serde_json::Value::String(r.persists_current_record.clone()),
                serde_json::Value::String(r.set_based_db_writes.clone()),
                serde_json::Value::String(r.validates_param.clone()),
                serde_json::Value::String(r.copies_into_param.clone()),
                serde_json::Value::String(r.resets_filters_on_param.clone()),
                serde_json::Value::String(r.mutates_param.clone()),
                serde_json::Value::String(r.requires_loaded_at_entry.clone()),
                serde_json::Value::String(r.mutates_before_load.clone()),
                field_list_fp(&r.required_loaded_fields_at_entry),
                serde_json::Value::String(r.dirty_at_exit.clone()),
                field_list_fp(&r.current_loaded_fields_at_exit),
            ])
        })
        .collect();

    let arr = serde_json::Value::Array(vec![
        serde_json::Value::Array(db_effects_arr),
        serde_json::Value::Bool(s.has_unresolved_calls),
        serde_json::Value::Array(uncertainties_arr),
        serde_json::Value::Array(param_roles_arr),
    ]);
    serde_json::to_string(&arr).unwrap_or_default()
}

/// The EXACT information [`stable_summary_fingerprint`] encodes, as a comparable
/// struct instead of a serde_json string. Equality of two `SummaryChangeKey`s is
/// equivalent to equality of the two fingerprint strings: the fingerprint is
/// `JSON.stringify` over these same components in the same order, and JSON
/// serialization of (arrays of strings + bool + numbers) is injective. This lets
/// the JACOBI fixed-point compare change keys directly instead of allocating and
/// comparing a whole-summary JSON string per member per round, WITHOUT changing
/// the iteration trajectory. The equivalence is unit-tested exhaustively in
/// `change_key_equality_iff_fingerprint_equality`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryChangeKey {
    /// "{effect_key}:{via}" per db effect, in order.
    pub db_effects: Vec<String>,
    pub has_unresolved_calls: bool,
    /// `p_uncertainty_key` per uncertainty, in order.
    pub uncertainties: Vec<String>,
    /// The same 14 per-role fields the fingerprint encodes, stringified in the
    /// fingerprint's exact order (the two FieldList fields via `field_list_key`).
    pub parameter_roles: Vec<Vec<String>>,
}

/// Build the [`SummaryChangeKey`] for a projected summary. Its equality relation
/// is byte-identical to [`stable_summary_fingerprint`]'s.
pub fn summary_change_key(s: &PRoutineSummaryCore) -> SummaryChangeKey {
    // Mirror `stable_summary_fingerprint`'s `field_list_fp` EXACTLY, then
    // serialize the folded value the way the fingerprint's `serde_json::to_string`
    // does. Emitting the SERIALIZED form (a quoted JSON string) rather than a bare
    // join is load-bearing: `field_list_fp` folds an array to `Value::String(join)`
    // and serde quotes ALL strings, so the fingerprint does NOT distinguish
    // `Array(["unknown"])` from `String("unknown")` (both serialize to `"unknown"`).
    // The change key must reproduce that collision — a stricter key would flag a
    // phantom change and lengthen a recursive SCC's fixed-point trajectory (the
    // cap-hit path is load-bearing), so the two can NEVER be allowed to diverge.
    fn field_list_key(v: &serde_json::Value) -> String {
        let folded = if let serde_json::Value::Array(arr) = v {
            let joined: String = arr
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(",");
            serde_json::Value::String(joined)
        } else {
            v.clone()
        };
        serde_json::to_string(&folded).unwrap_or_default()
    }

    SummaryChangeKey {
        db_effects: s
            .db_effects
            .iter()
            .map(|e| format!("{}:{}", e.effect_key, e.via))
            .collect(),
        has_unresolved_calls: s.has_unresolved_calls,
        uncertainties: s.uncertainties.iter().map(p_uncertainty_key).collect(),
        parameter_roles: s
            .parameter_roles
            .iter()
            .map(|r| {
                vec![
                    r.parameter_index.to_string(),
                    r.loads_from_db_param.clone(),
                    r.initialises_param.clone(),
                    r.persists_current_record.clone(),
                    r.set_based_db_writes.clone(),
                    r.validates_param.clone(),
                    r.copies_into_param.clone(),
                    r.resets_filters_on_param.clone(),
                    r.mutates_param.clone(),
                    r.requires_loaded_at_entry.clone(),
                    r.mutates_before_load.clone(),
                    field_list_key(&r.required_loaded_fields_at_entry),
                    r.dirty_at_exit.clone(),
                    field_list_key(&r.current_loaded_fields_at_exit),
                ]
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Top-level projection entry points.
// ---------------------------------------------------------------------------

/// Run the full pipeline (assemble + call-resolve + combined-graph + the v2
/// closed-form `EffectStore` solver) and project the post-computeSummaries model
/// to the R3a-2 stable comparison surface.
pub fn project_r3a2(resolved: &L3Resolved) -> R3a2Projection {
    R3a2Projection {
        summaries: run_and_project(resolved),
    }
}

// ---------------------------------------------------------------------------
// Internal: run the pipeline and collect the projection.
// ---------------------------------------------------------------------------

fn run_and_project(resolved: &L3Resolved) -> Vec<PRoutineSummaryCore> {
    let ws = &resolved.workspace;
    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    // Engine-switch S6.8: the model's own calls and events (L3's when nothing is
    // attached, as for this projection's golden callers).
    let calls = crate::engine::l3::call_resolver::calls_for(resolved, &symbols);
    let event_graph = crate::engine::l3::event_graph::events_for(resolved, &symbols);
    let graph = build_combined_graph(ws, &calls, &event_graph);

    // Tarjan SCC over the combined graph.
    let mut adjacency: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (from, list) in &graph.edges_by_from {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &adjacency,
    });

    // Build the field-resolution index from the resolved tables (extension
    // fields are already merged into each base table's `fields` at L3) so the
    // parameterRoles readsFields/writesFields resolve to FieldIds — mirroring
    // al-sem's `resolveField` against `ctx.tableById`. Keyed (tableId,
    // lowercased field name).
    let mut field_index: crate::engine::l4::summary_runner::FieldIndex =
        std::collections::HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }

    // Run the v2 closed-form `EffectStore` solver (no fixed leaves — this is the
    // source-only R3a-2 projection). Byte-identical `RoutineSummary` output to
    // the retired Jacobi solver (proven by the frozen-baseline differential),
    // so this projection is unchanged. Not the production detect/gate envelope —
    // `detector_context::build_detector_context` is the path that threads cap-hit
    // diagnostics into `DetectorContext`.
    let no_leaves: std::collections::HashMap<String, RoutineSummary> =
        std::collections::HashMap::new();
    let (final_summaries, _cap_diagnostics) = compute_summaries_v2_with_leaves_core(
        &ws.routines,
        &graph,
        &scc,
        &calls.upgraded_bindings,
        &field_index,
        &no_leaves,
    );

    let map = build_routine_stable_map(&ws.routines);

    // Project summaries.
    let mut projected: Vec<PRoutineSummaryCore> = final_summaries
        .values()
        .map(|s| project_routine_summary_core(s, &map))
        .collect();
    projected.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    projected
}

#[cfg(test)]
mod dedupe_uncertainties_tests {
    use super::*;

    fn u(kind: &str, callsite: Option<&str>, iface: Option<&str>) -> Uncertainty {
        Uncertainty {
            kind: kind.to_string(),
            callsite_id: callsite.map(str::to_string),
            operation_id: None,
            routine_id: None,
            interface_name: iface.map(str::to_string),
        }
    }

    /// The ORDER contract, hand-stated so it survives any rewrite of the
    /// implementation: the sort is on the concatenated `"kind|at"` string, NOT on
    /// the `(kind, at)` tuple. These two inputs are the minimal pair that
    /// separates the two orders — `("a","b") < ("ab","c")` as a tuple, but
    /// `"a|b" > "ab|c"` as a string because `'|'` (0x7C) > `'b'` (0x62).
    ///
    /// DISCRIMINATION PROOF (recorded, both directions): replacing
    /// `cmp_uncertainty_key`'s chained-byte comparison with the tuple form
    /// `(a.kind.as_str(), uncertainty_at(a)).cmp(&(b.kind.as_str(), uncertainty_at(b)))`
    /// makes this test FAIL with `["a|b", "ab|c"]`; restoring it passes.
    #[test]
    fn sort_is_on_the_concatenated_key_not_the_field_tuple() {
        // Deliberately fed in the order the tuple sort would produce, so a tuple
        // comparator would leave it untouched and look correct.
        let out = dedupe_uncertainties(vec![u("a", Some("b"), None), u("ab", Some("c"), None)]);
        let keys: Vec<String> = out.iter().map(uncertainty_key).collect();
        assert_eq!(
            keys,
            vec!["ab|c".to_string(), "a|b".to_string()],
            "'|' (0x7C) outranks 'b' (0x62), so ab|c sorts BEFORE a|b"
        );
    }

    /// The COLLISION contract: on two records sharing a key, the LAST one in the
    /// input wins (al-sem `Map.set` / the retired `BTreeMap::insert` overwrite).
    /// The precondition is hand-stated — two records built to share a key while
    /// differing in `interface_name`, the one field `uncertainty_key` drops —
    /// rather than obtained by asking production code to produce a collision.
    ///
    /// DISCRIMINATION PROOF (recorded, both directions): deleting the
    /// `std::mem::swap(a, b)` from `dedupe_uncertainties`' `dedup_by` (which
    /// makes it keep-FIRST) makes this test FAIL with `Some("iface-first")`;
    /// restoring it passes.
    #[test]
    fn same_key_keeps_the_last_record_not_the_first() {
        let first = u("interface-open-world", Some("cs1"), Some("iface-first"));
        let last = u("interface-open-world", Some("cs1"), Some("iface-last"));
        assert_eq!(
            uncertainty_key(&first),
            uncertainty_key(&last),
            "precondition: these two records share a key by construction"
        );
        assert_ne!(first, last, "precondition: and are otherwise different");

        let out = dedupe_uncertainties(vec![first, last]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].interface_name.as_deref(), Some("iface-last"));
    }

    /// Keep-last must hold for a run of THREE, not just a pair — a keep-first
    /// implementation that happens to reverse pairs would still pass the test
    /// above.
    #[test]
    fn same_key_run_of_three_keeps_the_last() {
        let out = dedupe_uncertainties(vec![
            u("k", Some("cs"), Some("a")),
            u("k", Some("cs"), Some("b")),
            u("k", Some("cs"), Some("c")),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].interface_name.as_deref(), Some("c"));
    }

    /// Distinct keys all survive, in key-sorted order, and the whole thing is a
    /// no-op on an empty list.
    #[test]
    fn distinct_keys_survive_in_key_order() {
        assert!(dedupe_uncertainties(Vec::new()).is_empty());
        let out = dedupe_uncertainties(vec![
            u("zzz", Some("1"), None),
            u("aaa", Some("2"), None),
            u("aaa", Some("1"), None),
        ]);
        let keys: Vec<String> = out.iter().map(uncertainty_key).collect();
        assert_eq!(keys, vec!["aaa|1", "aaa|2", "zzz|1"]);
    }
}

#[cfg(test)]
mod change_key_tests {
    use super::*;

    fn empty_core() -> PRoutineSummaryCore {
        PRoutineSummaryCore {
            routine_id: "r".to_string(),
            db_effects: Vec::new(),
            uncertainties: Vec::new(),
            parameter_roles: Vec::new(),
            in_recursive_cycle: false,
            has_unresolved_calls: false,
        }
    }

    fn effect(effect_key: &str, via: &str) -> PDbEffect {
        PDbEffect {
            effect_key: effect_key.to_string(),
            op: "read".to_string(),
            table_id: "t".to_string(),
            operation_id: "o".to_string(),
            temp_state: PDbEffectTempState::Unknown,
            via: via.to_string(),
        }
    }

    fn uncertainty(kind: &str, callsite: &str) -> PUncertainty {
        PUncertainty {
            kind: kind.to_string(),
            callsite_id: Some(callsite.to_string()),
            operation_id: None,
            routine_id: None,
            interface_name: None,
        }
    }

    fn arr(items: &[&str]) -> serde_json::Value {
        serde_json::Value::Array(
            items
                .iter()
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect(),
        )
    }

    fn default_role() -> PRecordRoleSummary {
        PRecordRoleSummary {
            parameter_index: 0,
            table_id: "t".to_string(),
            reads_fields: serde_json::Value::String("unknown".to_string()),
            writes_fields: serde_json::Value::String("unknown".to_string()),
            may_reset_filters: false,
            may_change_load_fields: false,
            may_assign_record: false,
            may_use_record_ref: false,
            requires_loaded_at_entry: "no".to_string(),
            required_loaded_fields_at_entry: serde_json::Value::String("unknown".to_string()),
            mutates_before_load: "no".to_string(),
            persists_current_record: "no".to_string(),
            set_based_db_writes: "no".to_string(),
            validates_param: "no".to_string(),
            copies_into_param: "no".to_string(),
            resets_filters_on_param: "no".to_string(),
            dirty_at_exit: "no".to_string(),
            current_loaded_fields_at_exit: serde_json::Value::String("unknown".to_string()),
            mutates_param: "no".to_string(),
            loads_from_db_param: "no".to_string(),
            initialises_param: "no".to_string(),
        }
    }

    fn role_with_entry(v: serde_json::Value) -> PRecordRoleSummary {
        PRecordRoleSummary {
            required_loaded_fields_at_entry: v,
            ..default_role()
        }
    }

    /// The binding contract: `SummaryChangeKey` equality IFF stable-fingerprint
    /// string equality, over a fixture set exercising every component. Proves the
    /// struct key is a faithful drop-in for the JSON fingerprint in the JACOBI
    /// change test — same equality relation, INCLUDING the fingerprint's own
    /// Array/String fold collisions (a stricter key would change the trajectory).
    #[test]
    fn change_key_equality_iff_fingerprint_equality() {
        let mut fixtures: Vec<PRoutineSummaryCore> = Vec::new();

        // 1. empty / default core.
        fixtures.push(empty_core());

        // 2. one effect `k1:direct`.
        let mut c = empty_core();
        c.db_effects = vec![effect("k1", "direct")];
        fixtures.push(c);

        // 3. same effect key, different `via`.
        let mut c = empty_core();
        c.db_effects = vec![effect("k1", "indirect")];
        fixtures.push(c);

        // 4. `has_unresolved_calls` flipped.
        let mut c = empty_core();
        c.has_unresolved_calls = true;
        fixtures.push(c);

        // 5. one uncertainty.
        let mut c = empty_core();
        c.uncertainties = vec![uncertainty("opaque-callee", "cs1")];
        fixtures.push(c);

        // 6. two uncertainties in order [cs1, cs2].
        let mut c = empty_core();
        c.uncertainties = vec![
            uncertainty("opaque-callee", "cs1"),
            uncertainty("opaque-callee", "cs2"),
        ];
        fixtures.push(c);

        // 7. two uncertainties swapped [cs2, cs1] — fingerprint order matters, so
        //    the key must too.
        let mut c = empty_core();
        c.uncertainties = vec![
            uncertainty("opaque-callee", "cs2"),
            uncertainty("opaque-callee", "cs1"),
        ];
        fixtures.push(c);

        // 8. param role, required_loaded_fields_at_entry = Array(["a","b"]).
        let mut c = empty_core();
        c.parameter_roles = vec![role_with_entry(arr(&["a", "b"]))];
        fixtures.push(c);

        // 9. param role, required_loaded_fields_at_entry = String("a,b"). The
        //    fingerprint folds BOTH #8 and #9 to Value::String("a,b") and treats
        //    them as EQUAL; the change key MUST preserve that collision (a stricter
        //    key would change the JACOBI trajectory).
        let mut c = empty_core();
        c.parameter_roles = vec![role_with_entry(serde_json::Value::String(
            "a,b".to_string(),
        ))];
        fixtures.push(c);

        // 10. exact clone of #2 (identical → equal both ways).
        let c10 = fixtures[1].clone();
        fixtures.push(c10);

        // 11. collision probe: Array(["unknown"]) ...
        let mut c = empty_core();
        c.parameter_roles = vec![role_with_entry(arr(&["unknown"]))];
        fixtures.push(c);

        // 12. ... vs String("unknown") — the fingerprint collides them; key must too.
        let mut c = empty_core();
        c.parameter_roles = vec![role_with_entry(serde_json::Value::String(
            "unknown".to_string(),
        ))];
        fixtures.push(c);

        // 13. routine_id differs only — excluded from both fingerprint AND key.
        let mut c = empty_core();
        c.routine_id = "different-id".to_string();
        fixtures.push(c);

        // 14. in_recursive_cycle differs only — excluded from both.
        let mut c = empty_core();
        c.in_recursive_cycle = true;
        fixtures.push(c);

        // The contract, over every ordered pair (reflexive + cross).
        for (i, a) in fixtures.iter().enumerate() {
            for (j, b) in fixtures.iter().enumerate() {
                let key_eq = summary_change_key(a) == summary_change_key(b);
                let fp_eq = stable_summary_fingerprint(a) == stable_summary_fingerprint(b);
                assert_eq!(
                    key_eq, fp_eq,
                    "equivalence broken at ({i},{j}): key_eq={key_eq} fp_eq={fp_eq}\n a={a:?}\n b={b:?}"
                );
            }
        }

        // Pin the specific fold collisions the correct `field_list_key` protects:
        // the Array/String pairs compare EQUAL both ways (NOT unequal).
        assert_eq!(
            summary_change_key(&fixtures[7]),
            summary_change_key(&fixtures[8])
        );
        assert_eq!(
            stable_summary_fingerprint(&fixtures[7]),
            stable_summary_fingerprint(&fixtures[8])
        );
        assert_eq!(
            summary_change_key(&fixtures[10]),
            summary_change_key(&fixtures[11])
        );
        assert_eq!(
            stable_summary_fingerprint(&fixtures[10]),
            stable_summary_fingerprint(&fixtures[11])
        );

        // ... and a genuine difference IS detected (swapped uncertainty order).
        assert_ne!(
            summary_change_key(&fixtures[5]),
            summary_change_key(&fixtures[6])
        );
    }
}
