//! L4 summary runner (R3a-2).
//!
//! Originally ported al-sem's `src/engine/summary-runner.ts` (`computeSummaries` /
//! `runSummaries` — the SCC walk, the per-SCC JACOBI fixed-point loop,
//! the fingerprint, the `composeRoutineCtx`, the parameterRoles cross-call
//! composition) and `src/engine/summary-engine.ts`
//! (`baseIntraproceduralSummaryCtx` / `computeRecordRolesCtx`). Since Task B1
//! (l4-dbeffect-store-and-retirement), `db_effects` / `uncertainties` /
//! `has_unresolved_calls` are computed by the closed-form v2 `EffectStore`
//! solver ([`compute_summaries_v2`]) — that JACOBI transfer function and its
//! per-pass fingerprint TRACE oracle (R3a-2 Rev 2 #3) are retired along with it.
//!
//! ## JACOBI discipline still governs the roles-ONLY fixpoint
//!
//! `parameter_roles` is still computed by a per-SCC JACOBI fixed-point loop
//! ([`run_one_scc_roles`]) — the ONE fixpoint remaining in this file. Each pass
//! FREEZES the entire prior-pass roles map; ALL reads within a pass see the
//! frozen snapshot; writes go to a NEW map; maps swap at end of pass. This is
//! JACOBI, NOT Gauss-Seidel — the `snapshot` inside the loop MUST be the frozen
//! PRIOR-pass state (taken by `mem::take`, so reads cannot see this pass's
//! writes); the next-pass accumulator must ONLY be written, never read, during
//! a pass. There is no trace oracle for this fixpoint (roles convergence is
//! checked by [`roles_change_key`], not a per-pass fingerprint sequence).

use std::collections::HashMap;

use super::combined_graph::CombinedGraph;
use super::db_effect_solver::{build_rvid_by_opid, seed_fixed_leaf_rows, solve_scc_db_effects};
use super::effect_lattice::{EffectPresence, effect_key_of, join_presence};
use super::effect_store::{SummaryBundle, SummaryBundleBuilder};
use super::effect_universe::GrowingEffectUniverse;
use super::routine_interner::RoutineInterner;
use super::scc::SccResult;
use super::summary::{
    DbEffect, FieldList, PRoutineSummaryCore, RecordRoleSummary, RoutineSummary, TempState,
    Uncertainty, project_routine_summary_core_internal, summary_change_key,
};
use crate::engine::l3::call_resolver::UpgradedBinding;
use crate::engine::l3::l3_workspace::L3Routine;
use crate::engine::perf_trace as pt;
use serde_json::json;

const MAX_FIXED_POINT_ITERATIONS: usize = 1000;

/// `ALSEM_SUMMARIES_CENSUS=1` — attribution inside the
/// `context.compute_summaries` span (~9.9 s of ~66 s on BC Base App 8020, and
/// the largest span in the run with NO attribution at all).
///
/// A span measures a REGION, not which line inside it costs. This splits the
/// region into the phases it is actually made of: the caller's `field_index`
/// build, then — inside [`compute_summaries_v2_bundle_with_leaves`] — the
/// per-routine scaffolding prologue (six separate maps, each its own counter),
/// the per-Tarjan-SCC loop (roles fixpoint / closed-form db-effect solve /
/// member assembly), and the freeze+finish epilogue.
///
/// Every timer goes through [`start`]/[`add_since`], which take no `Instant` at
/// all when the census is off — the per-SCC probes would otherwise be ~4 clock
/// reads per SCC on a corpus with tens of thousands of SCCs, i.e. a probe that
/// distorts the number it reports (`cones_census`'s own doc records the 1.8 s
/// this cost there).
pub(crate) mod summaries_census {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Phase 0 — the caller's own work inside the span (`detector_context`'s
    /// `FieldIndex` build, which lowercases every field name in the workspace).
    pub(crate) static FIELD_INDEX_NANOS: AtomicU64 = AtomicU64::new(0);

    // --- Prologue: the per-routine scaffolding maps, one counter each. ---
    pub(crate) static ROUTINES_BY_ID_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BODY_AVAIL_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BASE_SUMMARIES_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static STABLE_MAP_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static INTERNER_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_EDGES_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SEED_LEAVES_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static RVID_NANOS: AtomicU64 = AtomicU64::new(0);

    // --- The per-Tarjan-SCC loop. ---
    pub(crate) static ROLES_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_NANOS: AtomicU64 = AtomicU64::new(0);
    /// The member-assembly tail of each SCC iteration (`RoutineSummary` build +
    /// `v2_map.insert`) — inside the loop but covered by neither timer above.
    pub(crate) static ASSEMBLE_NANOS: AtomicU64 = AtomicU64::new(0);

    // --- Inside `db_solver`: the per-effective-SCC pipeline
    // (`solve_scc_db_effects` -> `solve_one_effective_scc`). ---
    pub(crate) static DB_EFFSCC_NANOS: AtomicU64 = AtomicU64::new(0);
    /// The multi-sibling general path's `settled.clone()` + its per-solved-member
    /// re-insert — the only place a whole workspace-sized map is copied per SCC.
    pub(crate) static DB_LOCALSETTLED_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_PD_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_UNION_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_VIA_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_PDVIA_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_SIDE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_WRITE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_OUT_NANOS: AtomicU64 = AtomicU64::new(0);
    /// How many Tarjan SCCs took the multi-sibling general path (the one that
    /// clones `settled`), vs the single-effective-SCC fast path.
    pub(crate) static DB_MULTI_EFF_SCCS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DB_EFF_SCC_SOLVES: AtomicU64 = AtomicU64::new(0);

    // --- Inside `side_facts` (the dominant db_solver phase). ---
    /// The per-member production loop, split three ways.
    pub(crate) static SF_BASE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_EDGE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_UNCEDGE_NANOS: AtomicU64 = AtomicU64::new(0);
    /// The final per-member assembly: `shared_vec.clone()` + extend + dedupe.
    pub(crate) static SF_ASSEMBLE_NANOS: AtomicU64 = AtomicU64::new(0);
    /// Populations these times are per. Accumulated in plain locals inside
    /// `solve_side_facts` and folded in with ONE `fetch_add` per call, so a
    /// per-edge / per-uncertainty counter never touches an atomic on the hot path.
    pub(crate) static SF_EDGES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_SHARED_INSERTS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_OPAQUE_PUSHES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_DEDUPE_ELEMS: AtomicU64 = AtomicU64::new(0);
    /// Elements COPIED by `shared_vec.clone()`. Since the last member of each
    /// effective SCC takes `shared_vec` by MOVE, this counts only the copies
    /// that are still made; `SF_SHARED_MOVED_ELEMS` counts the ones that are
    /// not. The two together are the population the pre-move code cloned, so a
    /// drop here reads as a saving rather than as a vanished counter.
    pub(crate) static SF_SHARED_CLONE_ELEMS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SF_SHARED_MOVED_ELEMS: AtomicU64 = AtomicU64::new(0);
    /// Elements ACTUALLY emitted by `solve_side_facts` (post-dedupe), the
    /// materialization a record-carrying design has to pay no matter what.
    pub(crate) static SF_OUTPUT_ELEMS: AtomicU64 = AtomicU64::new(0);
    /// `uncertainty_key` collisions where the two records DIFFER — the only case
    /// `dedupe_uncertainties`' last-write-wins is observable (the key is
    /// `kind|at`, dropping `interface_name` and the losing id fields). Measured
    /// **0** on both 8020 and DO. NOT gated and free: `fold_shared` compares the
    /// stored record instead of overwriting it, so this increments only on the
    /// branch that comparison already has to take.
    pub(crate) static SF_KEY_CONFLICTS: AtomicU64 = AtomicU64::new(0);

    // --- Epilogue. ---
    pub(crate) static FINISH_NANOS: AtomicU64 = AtomicU64::new(0);

    // --- Populations (what the times are per). ---
    pub(crate) static ROUTINES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SCCS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static MEMBERS_ASSEMBLED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BASE_SUMMARIES_BUILT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static RVID_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FIELD_INDEX_ENTRIES: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn enabled() -> bool {
        static E: OnceLock<bool> = OnceLock::new();
        *E.get_or_init(|| std::env::var("ALSEM_SUMMARIES_CENSUS").as_deref() == Ok("1"))
    }

    pub(crate) fn add(c: &AtomicU64, v: u64) {
        c.fetch_add(v, Ordering::Relaxed);
    }

    /// A timer ONLY when the census is enabled — off-census this is a bool load
    /// and a `None`, so no clock is read on any of the per-SCC probe sites.
    pub(crate) fn start() -> Option<std::time::Instant> {
        enabled().then(std::time::Instant::now)
    }

    /// Charge `t`'s elapsed time to `c`, if the census is on.
    pub(crate) fn add_since(c: &AtomicU64, t: Option<std::time::Instant>) {
        if let Some(t) = t {
            c.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }

    pub(crate) fn report() {
        if !enabled() {
            return;
        }
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let ms = |n: u64| n as f64 / 1e6;

        let field_index = g(&FIELD_INDEX_NANOS);
        let routines_by_id = g(&ROUTINES_BY_ID_NANOS);
        let body_avail = g(&BODY_AVAIL_NANOS);
        let base = g(&BASE_SUMMARIES_NANOS);
        let stable = g(&STABLE_MAP_NANOS);
        let interner = g(&INTERNER_NANOS);
        let unc_edges = g(&UNC_EDGES_NANOS);
        let seed = g(&SEED_LEAVES_NANOS);
        let rvid = g(&RVID_NANOS);
        let roles = g(&ROLES_NANOS);
        let db = g(&DB_NANOS);
        let assemble = g(&ASSEMBLE_NANOS);
        let finish = g(&FINISH_NANOS);

        let prologue =
            routines_by_id + body_avail + base + stable + interner + unc_edges + seed + rvid;
        let loop_total = roles + db + assemble;
        let total = field_index + prologue + loop_total + finish;
        let pct = |n: u64| {
            if total == 0 {
                0.0
            } else {
                100.0 * n as f64 / total as f64
            }
        };

        eprintln!(
            "[summaries-census] routines={} sccs={} members_assembled={} phases_total={:.1}ms",
            g(&ROUTINES),
            g(&SCCS),
            g(&MEMBERS_ASSEMBLED),
            ms(total),
        );
        eprintln!(
            "[summaries-census] field_index={:.1}ms ({:.1}%) prologue={:.1}ms ({:.1}%) scc_loop={:.1}ms ({:.1}%) finish={:.1}ms ({:.1}%)",
            ms(field_index),
            pct(field_index),
            ms(prologue),
            pct(prologue),
            ms(loop_total),
            pct(loop_total),
            ms(finish),
            pct(finish),
        );
        eprintln!(
            "[summaries-census]   prologue: routines_by_id={:.1}ms body_avail={:.1}ms base_summaries={:.1}ms x{} stable_map={:.1}ms",
            ms(routines_by_id),
            ms(body_avail),
            ms(base),
            g(&BASE_SUMMARIES_BUILT),
            ms(stable),
        );
        eprintln!(
            "[summaries-census]   prologue: interner={:.1}ms unc_edges={:.1}ms seed_leaves={:.1}ms rvid_by_opid={:.1}ms (entries={})",
            ms(interner),
            ms(unc_edges),
            ms(seed),
            ms(rvid),
            g(&RVID_ENTRIES),
        );
        eprintln!(
            "[summaries-census]   scc_loop: roles={:.1}ms ({:.1}%) db_solver={:.1}ms ({:.1}%) assemble={:.1}ms ({:.1}%)",
            ms(roles),
            pct(roles),
            ms(db),
            pct(db),
            ms(assemble),
            pct(assemble),
        );
        eprintln!(
            "[summaries-census]   db_solver: eff_sccs={:.1}ms local_settled={:.1}ms (multi_eff_sccs={}) solves={}",
            ms(g(&DB_EFFSCC_NANOS)),
            ms(g(&DB_LOCALSETTLED_NANOS)),
            g(&DB_MULTI_EFF_SCCS),
            g(&DB_EFF_SCC_SOLVES),
        );
        eprintln!(
            "[summaries-census]   db_solver: pd_reach={:.1}ms ({:.1}%) union={:.1}ms ({:.1}%) via={:.1}ms ({:.1}%) pd_via={:.1}ms ({:.1}%)",
            ms(g(&DB_PD_NANOS)),
            pct(g(&DB_PD_NANOS)),
            ms(g(&DB_UNION_NANOS)),
            pct(g(&DB_UNION_NANOS)),
            ms(g(&DB_VIA_NANOS)),
            pct(g(&DB_VIA_NANOS)),
            ms(g(&DB_PDVIA_NANOS)),
            pct(g(&DB_PDVIA_NANOS)),
        );
        eprintln!(
            "[summaries-census]   db_solver: side_facts={:.1}ms ({:.1}%) write_rows={:.1}ms ({:.1}%) out_assemble={:.1}ms ({:.1}%)",
            ms(g(&DB_SIDE_NANOS)),
            pct(g(&DB_SIDE_NANOS)),
            ms(g(&DB_WRITE_NANOS)),
            pct(g(&DB_WRITE_NANOS)),
            ms(g(&DB_OUT_NANOS)),
            pct(g(&DB_OUT_NANOS)),
        );
        eprintln!(
            "[summaries-census]   side_facts: base={:.1}ms ({:.1}%) edges={:.1}ms ({:.1}%) unc_edges={:.1}ms ({:.1}%) assemble={:.1}ms ({:.1}%)",
            ms(g(&SF_BASE_NANOS)),
            pct(g(&SF_BASE_NANOS)),
            ms(g(&SF_EDGE_NANOS)),
            pct(g(&SF_EDGE_NANOS)),
            ms(g(&SF_UNCEDGE_NANOS)),
            pct(g(&SF_UNCEDGE_NANOS)),
            ms(g(&SF_ASSEMBLE_NANOS)),
            pct(g(&SF_ASSEMBLE_NANOS)),
        );
        eprintln!(
            "[summaries-census]   side_facts pop: edges={} shared_inserts={} opaque_pushes={} dedupe_elems={} shared_clone_elems={}",
            g(&SF_EDGES),
            g(&SF_SHARED_INSERTS),
            g(&SF_OPAQUE_PUSHES),
            g(&SF_DEDUPE_ELEMS),
            g(&SF_SHARED_CLONE_ELEMS),
        );
        eprintln!(
            "[summaries-census]   side_facts pop: output_elems={} shared_moved_elems={} key_conflicts={} (a conflict = one uncertainty_key, two DIFFERENT records)",
            g(&SF_OUTPUT_ELEMS),
            g(&SF_SHARED_MOVED_ELEMS),
            g(&SF_KEY_CONFLICTS),
        );
        eprintln!(
            "[summaries-census]   field_index entries={}",
            g(&FIELD_INDEX_ENTRIES),
        );
    }
}

// ---------------------------------------------------------------------------
// Op classification (ports src/engine/op-classification.ts).
// ---------------------------------------------------------------------------

fn is_db_touching(op: &str) -> bool {
    matches!(
        op,
        "FindSet"
            | "FindFirst"
            | "FindLast"
            | "Find"
            | "Get"
            | "Next"
            | "Count"
            | "CountApprox"
            | "IsEmpty"
            | "CalcFields"
            | "CalcSums"
            | "Modify"
            | "ModifyAll"
            | "Insert"
            | "Delete"
            | "DeleteAll"
            | "Rename"
            | "LockTable"
    )
}

pub(crate) fn record_flow_role(op: &str) -> &'static str {
    match op {
        "Get" | "FindFirst" | "FindLast" | "FindSet" | "Find" | "Next" => "loadsFromDb",
        "Init" => "initialises",
        // Rename writes the current record back under its new key (d39 already
        // counts it as a persist op).
        "Modify" | "Insert" | "Rename" => "persistsCurrent",
        "ModifyAll" | "DeleteAll" => "setBasedWrite",
        "Validate" => "validates",
        "Copy" | "TransferFields" => "copiesInto",
        "Reset" => "resetsFilter",
        _ => "neutral",
    }
}

// ---------------------------------------------------------------------------
// Base intraprocedural summary (ports baseIntraproceduralSummaryCtx).
// ---------------------------------------------------------------------------

/// Build a routine's summary from its OWN intraprocedural features only — no
/// callee composition. Mirrors al-sem `baseIntraproceduralSummaryCtx`.
pub fn base_intraprocedural_summary(
    routine: &L3Routine,
    _routines_by_id: &HashMap<String, &L3Routine>,
    fields: &FieldIndex,
) -> RoutineSummary {
    let parameter_roles = compute_record_roles(routine, fields);

    // Opaque (.app symbol, no body). #22: the empty `db_effects` means "not
    // seen", not "proven not to write" — the `opaque-body` self-uncertainty
    // says so, exactly as `parse-incomplete` does for a body we cannot read.
    // It propagates to every ancestor like `parse-incomplete`.
    if !routine.body_available {
        return RoutineSummary {
            routine_id: routine.id.clone(),
            db_effects: Vec::new(),
            in_recursive_cycle: false,
            has_unresolved_calls: true,
            uncertainties: vec![Uncertainty {
                kind: "opaque-body".to_string(),
                callsite_id: None,
                operation_id: None,
                routine_id: Some(routine.id.clone()),
                interface_name: None,
            }],
            parameter_roles,
        };
    }

    // Parse-incomplete — body present but unparseable.
    if routine.parse_incomplete {
        return RoutineSummary {
            routine_id: routine.id.clone(),
            db_effects: Vec::new(),
            in_recursive_cycle: false,
            has_unresolved_calls: true,
            uncertainties: vec![Uncertainty {
                kind: "parse-incomplete".to_string(),
                callsite_id: None,
                operation_id: None,
                routine_id: Some(routine.id.clone()),
                interface_name: None,
            }],
            parameter_roles,
        };
    }

    // Body available + parsed — derive direct facts from the operation stream.
    let mut db_effects: Vec<DbEffect> = Vec::new();
    for op in &routine.record_operations {
        if !is_db_touching(&op.op) {
            continue;
        }
        let table_id = op.table_id.clone().unwrap_or_else(|| "unknown".to_string());
        let temp_state = op
            .temp_state
            .as_ref()
            .map(TempState::from_p)
            .unwrap_or(TempState::Unknown);
        let temp_kind = temp_state.to_kind();
        let effect_key = effect_key_of(&op.op, &table_id, &op.id, &temp_kind);
        db_effects.push(DbEffect {
            effect_key,
            operation_id: op.id.clone(),
            op: op.op.clone(),
            table_id,
            record_variable_id: op.record_variable_id.clone(),
            temp_state,
            via: "direct".to_string(),
        });
    }

    // Sort by effect_key for determinism (matches al-sem sort).
    db_effects.sort_by(|a, b| a.effect_key.cmp(&b.effect_key));

    RoutineSummary {
        routine_id: routine.id.clone(),
        db_effects,
        in_recursive_cycle: false,
        has_unresolved_calls: false,
        uncertainties: Vec::new(),
        parameter_roles,
    }
}

/// Compute RecordRoleSummary per record parameter. Mirrors al-sem
/// `computeRecordRolesCtx`. Path-aware facts (requiresLoadedAtEntry etc.) are
/// populated as "unknown" here; [`compose_roles_only`] overwrites them with the
/// flat-walker facts (which need the current fixpoint `lookup`) — the role the
/// retired `compose_routine` played in the pre-`b4181d8` tree.
fn compute_record_roles(routine: &L3Routine, fields: &FieldIndex) -> Vec<RecordRoleSummary> {
    let mut out: Vec<RecordRoleSummary> = Vec::new();
    for param in &routine.parameters {
        if !param.is_record {
            continue;
        }
        let rec_var = routine
            .record_variables
            .iter()
            .find(|rv| rv.is_parameter && rv.parameter_index == Some(param.index));
        let rec_var = match rec_var {
            Some(rv) => rv,
            None => continue,
        };
        let table_id = rec_var
            .table_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let mut reads_fields: Vec<String> = Vec::new();
        let mut writes_fields: Vec<String> = Vec::new();
        let mut may_reset_filters = false;
        let mut may_change_load_fields = false;
        let mut may_assign_record = false;
        let mut loads_from_db_param = EffectPresence::No;
        let mut initialises_param = EffectPresence::No;
        let mut persists_current_record = EffectPresence::No;
        let mut set_based_db_writes = EffectPresence::No;
        let mut validates_param = EffectPresence::No;
        let mut copies_into_param = EffectPresence::No;
        let mut resets_filters_on_param = EffectPresence::No;

        let rec_var_name_lc = rec_var.name.to_lowercase();

        // Field accesses — readsFields.
        for fa in &routine.field_accesses {
            if fa.record_variable_name.to_lowercase() != rec_var_name_lc {
                continue;
            }
            if let Some(fid) = resolve_field(&table_id, &fa.field_name, fields) {
                reads_fields.push(fid);
            }
        }

        // Record operations — may-fact bootstrap.
        for op in &routine.record_operations {
            if op.record_variable_name.to_lowercase() != rec_var_name_lc {
                continue;
            }
            if op.op == "Validate"
                && let Some(args) = &op.field_arguments
            {
                for arg in args {
                    if let Some(fid) = resolve_field(&table_id, arg, fields) {
                        writes_fields.push(fid);
                    }
                }
            }
            if op.op == "Reset" || op.op == "Copy" {
                may_reset_filters = true;
            }
            if op.op == "SetLoadFields" || op.op == "AddLoadFields" || op.op == "Reset" {
                may_change_load_fields = true;
            }
            if op.op == "Copy" || op.op == "TransferFields" {
                may_assign_record = true;
            }
            match record_flow_role(&op.op) {
                "loadsFromDb" => loads_from_db_param = EffectPresence::Yes,
                "initialises" => initialises_param = EffectPresence::Yes,
                "persistsCurrent" => persists_current_record = EffectPresence::Yes,
                "setBasedWrite" => set_based_db_writes = EffectPresence::Yes,
                "validates" => validates_param = EffectPresence::Yes,
                "copiesInto" => copies_into_param = EffectPresence::Yes,
                "resetsFilter" => resets_filters_on_param = EffectPresence::Yes,
                _ => {}
            }
        }

        let may_use_record_ref = param.type_text.to_lowercase().contains("recordref")
            || param.type_text.to_lowercase().contains("fieldref")
            || param.type_text.to_lowercase().contains("variant");

        let (reads_fields_fl, writes_fields_fl) = if may_use_record_ref {
            (FieldList::Unknown, FieldList::Unknown)
        } else {
            let rf: Vec<String> = reads_fields
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let wf: Vec<String> = writes_fields
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            (FieldList::Known(rf), FieldList::Known(wf))
        };

        let mutates_param = join_presence(
            join_presence(persists_current_record, validates_param),
            copies_into_param,
        );

        out.push(RecordRoleSummary {
            parameter_index: param.index,
            table_id,
            reads_fields: reads_fields_fl,
            writes_fields: writes_fields_fl,
            may_reset_filters,
            may_change_load_fields,
            may_assign_record,
            may_use_record_ref,
            // Path-aware entry-req + exit-effect facts are "unknown" until
            // `compose_roles_only` runs the flat walker with the current lookup.
            requires_loaded_at_entry: EffectPresence::Unknown,
            required_loaded_fields_at_entry: FieldList::Unknown,
            mutates_before_load: EffectPresence::Unknown,
            persists_current_record,
            set_based_db_writes,
            validates_param,
            copies_into_param,
            resets_filters_on_param,
            dirty_at_exit: EffectPresence::Unknown,
            current_loaded_fields_at_exit: FieldList::Unknown,
            mutates_param,
            loads_from_db_param,
            initialises_param,
        });
    }
    out.sort_by_key(|r| r.parameter_index);
    out
}

/// A field-resolution index: `(internal tableId, lowercased field name) →
/// internal FieldId`. Built once per workspace from the resolved tables (and
/// their merged extension fields). Mirrors the case-insensitive
/// `resolveField` lookup al-sem performs against `ctx.tableById` in
/// `src/engine/summary-engine.ts`.
pub type FieldIndex = HashMap<(String, String), String>;

/// Resolve a field name to its internal FieldId by table, case-insensitively.
/// Mirrors al-sem `resolveField` (summary-engine.ts):
///   `table?.fields.find(f => f.name.toLowerCase() === fieldName.toLowerCase())?.id`.
/// Returns None when the table is unresolved (`"unknown"`) or the field is not
/// found on the table.
fn resolve_field(table_id: &str, field_name: &str, fields: &FieldIndex) -> Option<String> {
    if table_id == "unknown" {
        return None;
    }
    fields
        .get(&(table_id.to_string(), field_name.to_lowercase()))
        .cloned()
}

/// Compute ONLY a routine's `parameter_roles` — the cross-call exit-effect
/// composition (§c1b) plus the branch-aware `walk_param` path analysis
/// (§c1a/§c1b). This is the roles slice that the roles-only fixpoint
/// ([`run_one_scc_roles`]) folds — extracted from the retired full Jacobi
/// transfer function so the v2 role harvest reproduces its byte-for-byte output
/// (now pinned to the frozen baseline).
///
/// **db_effects-INDEPENDENT by construction.** The seed is `base_roles`; the
/// cross-call fold reads only callee `parameter_roles`; and
/// `cfg_walker::walk_param` reads only callee `parameter_roles` (verified:
/// `cfg_walker.rs` contains ZERO `db_effects` references). This function
/// therefore never constructs a `db_effects_by_key` set — dropping that fold
/// from the v2 role harvest (so the ~729s db_effects Jacobi no longer runs there)
/// is exactly Task 8b's perf win.
///
/// `snapshot` is the FROZEN prior-pass map (JACOBI reads go here); `final_map` is
/// the settled summaries for already-processed SCCs. Callee `parameter_roles` are
/// read via `snapshot.get(id).or_else(|| final_map.get(id))`.
#[allow(clippy::too_many_arguments)]
fn compose_roles_only(
    routine: &L3Routine,
    base_roles: &[RecordRoleSummary],
    snapshot: &HashMap<String, RoutineSummary>,
    final_map: &HashMap<String, RoutineSummary>,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    graph: &CombinedGraph,
    body_avail_by_id: &HashMap<String, bool>,
) -> Vec<RecordRoleSummary> {
    // For non-recursive SCCs `snapshot` is empty; reads fall through to `final_map`.
    let lookup =
        |id: &str| -> Option<&RoutineSummary> { snapshot.get(id).or_else(|| final_map.get(id)) };

    // Deep-copy the base parameterRoles so we can mutate them independently.
    let mut parameter_roles: Vec<RecordRoleSummary> = base_roles.iter().map(clone_role).collect();

    // Cross-call exit-effect composition (spec §(c1b)).
    // `binding_resolution` and `callee_parameter_is_var` live in the upgraded-
    // bindings side table (from the call resolver), NOT on PCallArgumentBinding.
    for cs in &routine.call_sites {
        // Get the upgraded bindings for this callsite (if any).
        let upgraded = upgraded_bindings.get(&cs.id);

        // Find the resolved callee edge for this callsite.
        let edge = graph.edges_by_from.get(&routine.id).and_then(|edges| {
            edges
                .iter()
                .find(|e| e.callsite_id.as_deref() == Some(&cs.id))
        });

        for (arg_idx, binding) in cs.argument_bindings.iter().enumerate() {
            // Get the upgraded state for this binding position.
            let upgraded_b = upgraded.and_then(|ub| ub.get(arg_idx));

            // Only proceed if the binding is resolved.
            let resolution = upgraded_b
                .map(|ub| ub.binding_resolution.as_str())
                .unwrap_or("unresolved-callee");
            if resolution != "resolved" {
                continue;
            }
            let source_param_idx = match binding.source_parameter_index {
                Some(i) => i,
                None => continue,
            };
            if binding.caller_source_parameter_is_var != Some(true) {
                continue;
            }
            let callee_param_is_var = upgraded_b
                .map(|ub| ub.callee_parameter_is_var)
                .unwrap_or(false);
            if !callee_param_is_var {
                continue;
            }

            let callee_id = match edge.map(|e| e.to.as_str()) {
                Some(id) => id,
                None => continue,
            };
            let callee_summary = lookup(callee_id);
            // FIX 2: the opaque guard takes the "unknown" branch on ANY of the three
            // al-sem reasons (summary-runner.ts:267-270): no callee summary/role, OR
            // `callee.bodyAvailable === false`. A bodyless callee carries a role with
            // all-`No` facts; without this guard we would join "no" (an unsound flip).
            let callee_body_available = body_avail_by_id.get(callee_id).copied().unwrap_or(false);
            let callee_role = callee_summary.and_then(|s| {
                s.parameter_roles
                    .iter()
                    .find(|r| r.parameter_index == binding.parameter_index)
            });
            let opaque =
                callee_summary.is_none() || callee_role.is_none() || !callee_body_available;

            let p = parameter_roles
                .iter_mut()
                .find(|r| r.parameter_index == source_param_idx);
            let p = match p {
                Some(p) => p,
                None => continue,
            };
            if opaque {
                p.persists_current_record =
                    join_presence(p.persists_current_record, EffectPresence::Unknown);
                p.set_based_db_writes =
                    join_presence(p.set_based_db_writes, EffectPresence::Unknown);
                p.validates_param = join_presence(p.validates_param, EffectPresence::Unknown);
                p.copies_into_param = join_presence(p.copies_into_param, EffectPresence::Unknown);
                p.resets_filters_on_param =
                    join_presence(p.resets_filters_on_param, EffectPresence::Unknown);
                p.loads_from_db_param =
                    join_presence(p.loads_from_db_param, EffectPresence::Unknown);
                p.initialises_param = join_presence(p.initialises_param, EffectPresence::Unknown);
            } else {
                let cr = callee_role.unwrap();
                p.persists_current_record =
                    join_presence(p.persists_current_record, cr.persists_current_record);
                p.set_based_db_writes =
                    join_presence(p.set_based_db_writes, cr.set_based_db_writes);
                p.validates_param = join_presence(p.validates_param, cr.validates_param);
                p.copies_into_param = join_presence(p.copies_into_param, cr.copies_into_param);
                p.resets_filters_on_param =
                    join_presence(p.resets_filters_on_param, cr.resets_filters_on_param);
                // A helper that loads its `var` record loads the forwarder's too.
                // A MAY fact (loads on some path), but its readers take it as MUST
                // ("loaded after the call"): the walker's c1b (entry requirements,
                // read by d40 and d42) and d39 (a load point resets the dirt, via
                // `record_load_points`). That is lenient for all of them: it can
                // only drop an entry requirement or a d39 finding, never invent one.
                p.loads_from_db_param =
                    join_presence(p.loads_from_db_param, cr.loads_from_db_param);
                p.initialises_param = join_presence(p.initialises_param, cr.initialises_param);
            }
            p.mutates_param = join_presence(
                join_presence(p.persists_current_record, p.validates_param),
                p.copies_into_param,
            );
        }
    }

    // Path-aware entry-requirement + exit-effect composition (spec §(c1a)/(c1b)).
    // Mirrors al-sem summary-runner.ts lines 310-325: after cross-call c1b, run
    // the BRANCH-AWARE walker (`cfg_walker::walk_param`, the port of `walkRoutine`
    // → `walkCFG`) with the current JACOBI `lookup` so callee summaries are from
    // the current iteration. The walker overwrites the "unknown" entry-req +
    // exit-effect placeholders from the base summary with PATH-PROVEN facts:
    // a Validate/Modify/field-access INSIDE a conditional yields a branch-joined
    // (often "unknown") result, not the straight-line "yes"/"no".
    // Only runs when the body is available + parsed (opaque/parse-incomplete stay
    // "unknown" as set by the base summary).
    if routine.body_available && !routine.parse_incomplete {
        // Built ONCE per routine, not once per parameter: the op/call/fa index is
        // identical across every parameter's walk of the SAME routine.
        let walk_indexes = crate::engine::l4::cfg_walker::build_indexes(routine);
        for param_role in &mut parameter_roles {
            let rec_var = routine.record_variables.iter().find(|rv| {
                rv.is_parameter && rv.parameter_index == Some(param_role.parameter_index)
            });
            let (rec_var_name_lc, rec_var_id) = match rec_var {
                Some(rv) => (rv.name.to_lowercase(), Some(rv.id.as_str())),
                None => continue,
            };
            let f = crate::engine::l4::cfg_walker::walk_param(
                routine,
                &rec_var_name_lc,
                rec_var_id,
                snapshot,
                final_map,
                upgraded_bindings,
                graph,
                body_avail_by_id,
                &walk_indexes,
            );
            param_role.requires_loaded_at_entry = f.requires_loaded_at_entry;
            param_role.mutates_before_load = f.mutates_before_load;
            param_role.dirty_at_exit = f.dirty_at_exit;
            param_role.current_loaded_fields_at_exit = f.current_loaded_fields_at_exit;
            param_role.required_loaded_fields_at_entry = f.required_loaded_fields_at_entry;
        }
    }

    parameter_roles
}

/// Substitute a callee effect's `ParameterDependent(callee_param_index)` temp
/// state through the caller's per-callsite argument binding (G5 / RV-7).
///
/// Resolution (all uncertainty → `Unknown`, which FIRES — the sound direction):
///   1. event-dispatch edge (no `callsite_id`) → `Unknown`.
///   2. edge kinds with no binding semantics modeled
///      (`interface | codeunit-run | report-run | page-run | dynamic`) → `Unknown`.
///      Only `direct | method | implicit-trigger` carry usable bindings.
///   3. no binding whose `parameter_index == callee_param_index` → `Unknown`.
///   4. apply the SUBSTITUTION TABLE on the binding's `source_temp_state`:
///
/// ```text
/// Some(Known(true))  → Known(true)
/// Some(Known(false)) → Known(false)
/// Some(PD(j))        → PD(j)   (RE-SYMBOLIZE upward — TASK 8 / RV-7)
/// Some(Unknown)      → Unknown
/// None               → Unknown
/// ```
///
/// SOUNDNESS: only NARROWS symbolic → binding-derived, or RE-SYMBOLIZES a
/// forwarded caller param's PD to the caller's own param index (propagating the
/// symbolic dependency, never inventing it); never yields `Known(true)` unless
/// the binding source is itself `Known(true)`. A PD chasing itself around a
/// recursive cycle stays PD (monotone) and the fixed point converges.
///
/// `pub(crate)` (not module-private): `db_effect_solver::solve_pd_reachability`
/// is its only caller now (three call sites, ALL in `db_effect_solver.rs` —
/// this module itself has zero), which walks THIS per-edge transition as a
/// semi-naive product-graph reachability worklist in place of the retired
/// `compose_routine`'s JACOBI fold. Carried over unchanged from that retired
/// fold so the closed-form solver matches the old PD semantics exactly
/// (l4-summary-fixpoint-redesign Task 4); `pub(crate)` exists solely for that
/// one cross-module caller, not because two solvers share it — moving this
/// function to sit next to its only caller is a reasonable follow-up, out of
/// scope here.
pub(crate) fn substitute_pd_temp_state(
    edge: &super::combined_graph::CombinedEdge,
    callee_param_index: u32,
    routine: &L3Routine,
) -> TempState {
    // (1) event-dispatch / any to-less edge: no caller-frame binding.
    let cs_id = match &edge.callsite_id {
        Some(id) => id,
        None => return TempState::Unknown,
    };
    // (2) only binding-carrying edge kinds substitute. This is intentionally a
    // POSITIVE allowlist: only `direct | method | implicit-trigger` carry usable
    // bindings; ANY other kind — including future edge kinds — falls to Unknown
    // (sound = fires). event-dispatch is already excluded by the `callsite_id:
    // None` guard above.
    if !matches!(edge.kind.as_str(), "direct" | "method" | "implicit-trigger") {
        return TempState::Unknown;
    }
    pd_temp_state_at_callsite(routine, cs_id, callee_param_index)
}

/// Steps (3)-(4) of [`substitute_pd_temp_state`], without the edge-kind gate:
/// the temp state the caller's `routine` hands to callee parameter
/// `callee_param_index` at its call site `cs_id`. The capability cone
/// (`capability_cone.rs`) applies the same substitution at its own call edges,
/// behind its own edge-kind allowlist.
pub(crate) fn pd_temp_state_at_callsite(
    routine: &L3Routine,
    cs_id: &str,
    callee_param_index: u32,
) -> TempState {
    // Find THIS edge's callsite among the caller's call sites.
    let cs = match routine.call_sites.iter().find(|cs| cs.id == cs_id) {
        Some(cs) => cs,
        None => return TempState::Unknown,
    };
    // (3) the binding for the callee param the PD refers to (the call's
    // receiver for a table method's `Rec`, `RECEIVER_PARAM_INDEX`).
    let source_temp_state = cs.source_temp_state_for(callee_param_index);
    // (4) substitution table over the binding's captured source temp state.
    //
    // A record-typed PARAMETER is present in the caller's
    // `enclosing_record_variables` at L2, so a forwarded-param arg's binding
    // ALREADY carries `source_temp_state` = that caller param's OWN temp_state
    // (verified — see `extract_record_variables` / `extract_argument_bindings`):
    //   keyword `temporary`  -> Known(true)
    //   keyword-less by-var  -> ParameterDependent(caller_param_index)
    //   by-value             -> Known(false)
    //
    // TASK 8 (RV-7 binding gap): RE-SYMBOLIZE the PD case. When the caller
    // forwards its OWN keyword-less by-var record param onward, the inherited
    // effect's tempness depends on the CALLER's param `j`, not a concrete var —
    // so it becomes `ParameterDependent(j)`, chaining the symbolic dependency
    // UPWARD instead of collapsing to Unknown. The substituted PD index is the
    // CALLER-frame index carried in `source_temp_state` (the binding already
    // re-anchored it from the callee frame to the caller frame at L2).
    //
    // SOUNDNESS: re-symbolizing PD->PD only PROPAGATES a symbolic dependency; it
    // never invents Known(true). A forwarded keyword param yields Known(true)
    // ONLY because its source param IS Known(true). Around a recursive cycle a
    // PD chasing itself stays PD (monotone) and the fixed point converges — the
    // effect_key includes the PD index, so the state space stays finite.
    match source_temp_state {
        Some(ts) => match TempState::from_p(ts) {
            TempState::Known(v) => TempState::Known(v),
            // Caller's-own-param source (forwarded keyword-less by-var param):
            // re-symbolize to the caller's own param index (chains upward).
            TempState::ParameterDependent(j) => TempState::ParameterDependent(j),
            // Genuinely unknown source → Unknown (conservative = fires).
            TempState::Unknown => TempState::Unknown,
        },
        // No captured source temp state (arg is not a record var/param the
        // caller declares — e.g. an implicit-rec or unresolved name): Unknown.
        None => TempState::Unknown,
    }
}

fn clone_role(r: &RecordRoleSummary) -> RecordRoleSummary {
    RecordRoleSummary {
        parameter_index: r.parameter_index,
        table_id: r.table_id.clone(),
        reads_fields: r.reads_fields.clone(),
        writes_fields: r.writes_fields.clone(),
        may_reset_filters: r.may_reset_filters,
        may_change_load_fields: r.may_change_load_fields,
        may_assign_record: r.may_assign_record,
        may_use_record_ref: r.may_use_record_ref,
        requires_loaded_at_entry: r.requires_loaded_at_entry,
        required_loaded_fields_at_entry: r.required_loaded_fields_at_entry.clone(),
        mutates_before_load: r.mutates_before_load,
        persists_current_record: r.persists_current_record,
        set_based_db_writes: r.set_based_db_writes,
        validates_param: r.validates_param,
        copies_into_param: r.copies_into_param,
        resets_filters_on_param: r.resets_filters_on_param,
        dirty_at_exit: r.dirty_at_exit,
        current_loaded_fields_at_exit: r.current_loaded_fields_at_exit.clone(),
        mutates_param: r.mutates_param,
        loads_from_db_param: r.loads_from_db_param,
        initialises_param: r.initialises_param,
    }
}

// ---------------------------------------------------------------------------
// compute_summaries_v2 — the new-solver seam (l4-summary-fixpoint-redesign).
// ---------------------------------------------------------------------------

/// The v2 db-effect solver core (the ONLY summary path since the old Jacobi
/// solver was retired). Computes `db_effects` / `uncertainties` /
/// `has_unresolved_calls` via the closed-form `db_effect_solver` (effective-SCC
/// re-decomposition + PD reachability + closed-form union + via + side-facts),
/// and `parameter_roles` via the roles-ONLY fixpoint [`run_one_scc_roles`].
/// `in_recursive_cycle` is `scc_entry.recursive` (`true` for every member of a
/// recursive Tarjan SCC). The two are assembled into one `RoutineSummary` per
/// routine.
///
/// REGRESSION ANCHOR: the result is pinned byte-for-byte (over the complete
/// `RoutineSummary`) to the FROZEN complete-internal baseline in
/// `tests/l4-summary-baseline/` — captured from this solver at parity with the
/// old Jacobi solver before that solver was deleted (see
/// `tests/l4_summary_differential.rs`).
///
/// ⟨Task A2⟩ This is the REAL v2 core (spec Part A Step 2 — "v2 returns the
/// bundle"): it returns the workspace-complete [`SummaryBundle`] (the compact
/// db-effect rows) ALONGSIDE the settled map and any summarize-stage
/// `SummarizeDiagnostic` raised by the roles fixpoint's convergence backstop
/// (empty on every real SCC — roles converge). It is closed-form: there is no
/// per-pass trajectory to emit.
///
/// [`compute_summaries_v2_with_leaves_core`] is a THIN shim over this fn (see its
/// own doc): it drops the bundle after using it to rebuild `db_effects` from the
/// lazy view, returning the `(HashMap<String, RoutineSummary>,
/// Vec<SummarizeDiagnostic>)` shape every caller (incl. the no-leaves convenience
/// [`compute_summaries_v2`] and the differential harness) consumes.
///
/// ## Assembly discipline
///
/// ONE settled map (`v2_map`) is threaded through the reverse-topological SCC
/// walk, pre-seeded with the fixed leaves, and fed as the predecessor view to
/// BOTH [`run_one_scc_roles`] (which reads settled callee `parameter_roles`) and
/// `solve_scc_db_effects` (which reads settled successor `db_effects` /
/// `uncertainties`). For each Tarjan SCC, the roles fixpoint and the db-effect
/// solver BOTH read `v2_map` as their predecessor view (before it is updated with
/// this SCC), then this SCC's members are assembled member-by-member. ⟨Task A2⟩
/// A `SummaryBundleBuilder` is threaded `&mut` alongside `universe` through the
/// SAME per-SCC db-effect solve call and frozen into the returned
/// [`SummaryBundle`] once the loop completes — `v2_map`'s own feed-forward
/// (`settled`-successor reads) is UNCHANGED by this (see `effect_store.rs`'s
/// module doc for why materialized `Vec<DbEffect>` feed-forward survives
/// through A2).
pub fn compute_summaries_v2_bundle_with_leaves(
    routines: &[L3Routine],
    graph: &CombinedGraph,
    scc: &SccResult,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    fields: &FieldIndex,
    leaf_summaries: &HashMap<String, RoutineSummary>,
) -> (
    SummaryBundle,
    HashMap<String, RoutineSummary>,
    Vec<SummarizeDiagnostic>,
) {
    // --- Scaffolding: mirrors what the retired `compute_summaries_with_leaves`
    // built (in the pre-`b4181d8` tree). ---
    summaries_census::add(&summaries_census::ROUTINES, routines.len() as u64);
    summaries_census::add(&summaries_census::SCCS, scc.sccs.len() as u64);

    let _t = summaries_census::start();
    let routines_by_id: HashMap<String, &L3Routine> =
        routines.iter().map(|r| (r.id.clone(), r)).collect();
    summaries_census::add_since(&summaries_census::ROUTINES_BY_ID_NANOS, _t);

    let _t = summaries_census::start();
    let body_avail_by_id: HashMap<String, bool> = routines
        .iter()
        .map(|r| (r.id.clone(), r.body_available))
        .collect();
    summaries_census::add_since(&summaries_census::BODY_AVAIL_NANOS, _t);

    // Base intraprocedural summaries for NON-LEAF routines only (a leaf carries
    // its own summary; its EMPTY merged features must never overwrite it).
    let _t = summaries_census::start();
    let base_summaries: HashMap<String, RoutineSummary> = routines
        .iter()
        .filter(|r| !leaf_summaries.contains_key(&r.id))
        .map(|r| {
            (
                r.id.clone(),
                base_intraprocedural_summary(r, &routines_by_id, fields),
            )
        })
        .collect();
    summaries_census::add_since(&summaries_census::BASE_SUMMARIES_NANOS, _t);
    summaries_census::add(
        &summaries_census::BASE_SUMMARIES_BUILT,
        base_summaries.len() as u64,
    );

    let _t = summaries_census::start();
    let stable_map: HashMap<String, String> = routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();
    summaries_census::add_since(&summaries_census::STABLE_MAP_NANOS, _t);

    // Task A1/A3: intern every workspace routine id once, up front, in the
    // CANONICAL `stable_routine_id`-sorted order (spec rev4) — so ascending
    // `RoutineIx` is stable across repeated builds of the SAME workspace, not
    // merely self-consistent within one run. Covers every routine (leaf AND
    // non-leaf), the db-effect solver's per-member maps, AND — Task A3 — every
    // fixed-leaf id, INCLUDING retained cross-app dependency leaves that are
    // NOT in `routines` (`build_detector_context_cross_app` passes exactly
    // this). A leaf must be interned so the db-effect FEED-FORWARD can key its
    // row; a leaf outside `routines` has no `stable_routine_id` here, so its
    // own id is its canonical sort key (deterministic, and dedups against the
    // `routines` entry when the leaf is also a workspace routine).
    let _t = summaries_census::start();
    let routine_interner = RoutineInterner::build_canonical(
        routines
            .iter()
            .map(|r| (r.id.as_str(), r.stable_routine_id.as_str()))
            .chain(leaf_summaries.keys().map(|id| (id.as_str(), id.as_str()))),
    );
    summaries_census::add_since(&summaries_census::INTERNER_NANOS, _t);

    let _t = summaries_census::start();
    let mut uncertainty_edges_by_from: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, ue) in graph.uncertainty_edges.iter().enumerate() {
        uncertainty_edges_by_from
            .entry(ue.from.clone())
            .or_default()
            .push(i);
    }
    summaries_census::add_since(&summaries_census::UNC_EDGES_NANOS, _t);

    // A member is RECOMPUTED (contributes to an effective SCC) iff it is neither
    // a fixed leaf NOR missing from the workspace — the exact predicate the
    // retired `run_one_scc` used when it seeded `in_progress` from
    // `base_summaries`.
    let is_recomputed =
        |id: &str| -> bool { !leaf_summaries.contains_key(id) && routines_by_id.contains_key(id) };

    // ONE settled map — the assembled v2 output — fed forward as the predecessor
    // view to BOTH the roles fixpoint and the db-effect solver. v2's db_effects /
    // uncertainties / roles are byte-identical to the old solver's (Phase-1
    // parity), so feeding v2 forward equals the retired two-map design that fed
    // the old JACOBI's summaries forward — without running that JACOBI. Pre-seed
    // the fixed leaves so the key set matches the old solver's `final_map` exactly.
    let mut v2_map: HashMap<String, RoutineSummary> = HashMap::new();
    for (id, summary) in leaf_summaries {
        v2_map.insert(id.clone(), summary.clone());
    }

    // The loop-invariant per-SCC context (roles fixpoint reads it).
    let ctx = SccComputeCtx {
        routines_by_id: &routines_by_id,
        base_summaries: &base_summaries,
        upgraded_bindings,
        graph,
        body_avail_by_id: &body_avail_by_id,
        stable_map: &stable_map,
        leaf_summaries,
        uncertainty_edges_by_from: &uncertainty_edges_by_from,
    };

    // One workspace-wide interned effect universe, threaded across every SCC
    // (GROWING form — `intern()` mints new PD/terminal-emission variants as
    // SCCs settle; frozen ONCE after the loop, spec lifecycle steps 1-3).
    let mut universe = GrowingEffectUniverse::new();

    // Task A2/A3: the compact db-effect row accumulator + the shared terminal-
    // set arena that DOUBLES as the db-effect feed-forward source, threaded
    // `&mut` through the per-SCC solve exactly like `universe` — frozen into a
    // `SummaryBundle` once the loop below completes.
    let mut bundle_builder = SummaryBundleBuilder::new();

    // Task A3 (spec Step 3 ⟨rev3⟩ + lifecycle step 2b): seed the RETAINED
    // fixed leaves' singleton effect classes BEFORE the solve loop — they are
    // read as settled callees' db-effect feed-forward AND project like any
    // routine (preserving their own via). This interns every leaf effect
    // identity into `universe` up front (complete pre-freeze identity
    // discovery).
    let _t = summaries_census::start();
    seed_fixed_leaf_rows(
        leaf_summaries,
        &mut universe,
        &routine_interner,
        &mut bundle_builder,
    );
    summaries_census::add_since(&summaries_census::SEED_LEAVES_NANOS, _t);

    // Summarize-stage diagnostics raised by the roles fixpoint's convergence
    // backstop. Empty on every real SCC (roles converge) — see [`RolesSccOut`].
    let mut diagnostics: Vec<SummarizeDiagnostic> = Vec::new();

    // `record_variable_id` origins for every `operation_id` that can ever
    // appear on ANY assembled summary live in `base_summaries` (all non-leaf
    // routine bases) ∪ `leaf_summaries` (every leaf's own final summary) —
    // see `build_rvid_by_opid`'s doc for the completeness argument. Built
    // ONCE, here, BEFORE the per-Tarjan-SCC loop below — NOT per SCC. This
    // used to be rebuilt inside `solve_scc_db_effects` on every call (once
    // per Tarjan SCC), an O(total-workspace-db_effects) cost paid N times —
    // O(N^2) in routine count. Hoisting it here makes it O(total) ONCE.
    let _t = summaries_census::start();
    let rvid_by_opid = build_rvid_by_opid(&base_summaries, leaf_summaries);
    summaries_census::add_since(&summaries_census::RVID_NANOS, _t);
    summaries_census::add(&summaries_census::RVID_ENTRIES, rvid_by_opid.len() as u64);

    // Phase-split attribution (observability only): accumulate wall time spent in
    // the roles-only fixpoint vs the closed-form db-effect solver across all SCCs,
    // emitted once after the loop. Instant::now() is ~20ns; the per-SCC overhead is
    // negligible and the aggregate is only serialized when a tracer is active.
    //
    // Accumulated in NANOseconds, not micros: with tens of thousands of SCCs a
    // per-SCC `as_micros()` truncation loses up to one whole microsecond EACH,
    // i.e. tens of milliseconds of systematic under-count across the loop.
    let mut roles_ns: u128 = 0;
    let mut db_ns: u128 = 0;

    for scc_entry in &scc.sccs {
        // parameter_roles from the roles-ONLY fixpoint (the old db_effects JACOBI
        // is NOT run). Reads `v2_map` as its predecessor view (this SCC not yet
        // inserted).
        let _roles_t = std::time::Instant::now();
        let RolesSccOut {
            roles,
            cap_hit_stable_members,
        } = run_one_scc_roles(scc_entry, &v2_map, &ctx);
        roles_ns += _roles_t.elapsed().as_nanos();

        // Surface the roles fixpoint's cap-hit as a summarize-stage diagnostic,
        // byte-identical to the OLD solver's message shape (severity/stage/text).
        // Never fires on the corpus (roles converge); preserves the honesty signal.
        if let Some(members) = &cap_hit_stable_members {
            diagnostics.push(SummarizeDiagnostic {
                severity: "warning".to_string(),
                stage: "summarize".to_string(),
                message: format!(
                    "Summary fixed-point did not converge for SCC [{}]; its facts are lower-confidence",
                    members.join(", ")
                ),
            });
        }

        // New db-effect solve. Reads the SAME predecessor view (`v2_map`) for
        // side-facts; the db-effect feed-forward is on `bundle_builder` (Task
        // A3 — compact ids, no materialized `Vec<DbEffect>`). Records every
        // member's compact ROW in `bundle_builder` and returns only the
        // per-member `(uncertainties, has_unresolved_calls)` — db_effects are
        // projected lazily from the frozen bundle after the loop.
        // `body_avail_by_id` is the workspace-wide, run-invariant map built
        // once above — NOT rebuilt per SCC.
        let _db_t = std::time::Instant::now();
        let mut side_facts = solve_scc_db_effects(
            scc_entry,
            graph,
            &routines_by_id,
            &v2_map,
            &base_summaries,
            upgraded_bindings,
            &uncertainty_edges_by_from,
            &body_avail_by_id,
            &mut universe,
            &is_recomputed,
            &routine_interner,
            &mut bundle_builder,
        );
        db_ns += _db_t.elapsed().as_nanos();

        // Assemble each member: uncertainties/has_unresolved from the solver
        // (db_effects left EMPTY here — filled from the frozen bundle's lazy
        // projection in `compute_summaries_v2_with_leaves_core`), roles from
        // the roles-only fixpoint, in_recursive_cycle from the Tarjan SCC's own
        // `recursive` flag. `roles_out` defines the exact member set (== the
        // solver's recomputed set); a missing side-facts entry falls back to
        // empty (which fails the differential loudly rather than silently
        // masking a dropped member).
        let _asm_t = summaries_census::start();
        summaries_census::add(&summaries_census::MEMBERS_ASSEMBLED, roles.len() as u64);
        for (id, parameter_roles) in roles {
            // MOVE, not clone: `side_facts` is this iteration's own map and is
            // dropped at the end of it, and `roles` is a map so `id` is distinct
            // per member — so each entry is taken at most once. Cloning here deep-
            // copied all 3,700,433 `Uncertainty` records the census counts per
            // 8020 run, a second time, purely to drop the originals.
            let (mut uncertainties, has_unresolved_calls) =
                side_facts.remove(&id).unwrap_or_default();
            // On a roles cap-hit, attach the per-member `fixpoint-capped`
            // Uncertainty EXACTLY as the OLD solver did (appended AFTER the
            // solver's dedup+sorted uncertainties, mirroring old's post-fixpoint
            // push) so v2 stays byte-identical to old on a cap. Never fires on the
            // corpus (roles converge), so the golden path is untouched.
            if cap_hit_stable_members.is_some() {
                let stable_id = stable_map.get(&id).cloned().unwrap_or_else(|| id.clone());
                uncertainties.push(Uncertainty {
                    kind: "fixpoint-capped".to_string(),
                    callsite_id: None,
                    operation_id: None,
                    routine_id: Some(stable_id),
                    interface_name: None,
                });
            }
            let assembled = RoutineSummary {
                routine_id: id.clone(),
                db_effects: Vec::new(),
                in_recursive_cycle: scc_entry.recursive,
                has_unresolved_calls,
                uncertainties,
                parameter_roles,
            };
            v2_map.insert(id, assembled);
        }
        summaries_census::add_since(&summaries_census::ASSEMBLE_NANOS, _asm_t);
    }
    summaries_census::add(&summaries_census::ROLES_NANOS, roles_ns as u64);
    summaries_census::add(&summaries_census::DB_NANOS, db_ns as u64);

    // Phase-split attribution: roles-only fixpoint vs closed-form db-effect solver.
    // Emitted once (tracer-gated; no-op off-trace) so `context.compute_summaries`'s
    // cost can be attributed without per-SCC span spam.
    pt::instant_lazy("l4", "compute_summaries_v2_phase_split", || {
        json!({
            "roles_ms": (roles_ns / 1_000_000) as u64,
            "db_solver_ms": (db_ns / 1_000_000) as u64,
            "sccs": scc.sccs.len(),
        })
    });

    // Freeze the universe (spec lifecycle steps 3-7): computes `key_rank`,
    // then `finish` hash-conses the shared terminal-set arena into the
    // `EffectStore` and reorders each row's via/PD arrays to `key_rank` output
    // order. After this no new identity can be minted (compile-enforced).
    let _t = summaries_census::start();
    let bundle = bundle_builder.finish(universe.freeze(), routine_interner, rvid_by_opid);
    summaries_census::add_since(&summaries_census::FINISH_NANOS, _t);
    (bundle, v2_map, diagnostics)
}

/// Compat shim (spec Part A, "Public API"): wraps
/// [`compute_summaries_v2_bundle_with_leaves`] and rebuilds the legacy
/// `HashMap<String, RoutineSummary>` shape FROM the bundle's lazy `db_effects`
/// view, for every routine that has a compact row — proving the lazy view
/// reproduces the SAME `db_effects` the per-SCC solve recorded. A row exists for
/// both RECOMPUTED routines AND every RETAINED fixed leaf: `seed_fixed_leaf_rows`
/// normalizes each fixed leaf into a singleton-class compact row (spec ⟨rev3⟩)
/// before the solve loop, so its `db_effects` here are re-projected through the
/// SAME `db_effects` path (one projection impl, `SummaryBundle::project_row`),
/// preserving the leaf's own per-effect `via`. Only a routine with NO compact
/// row (e.g. a routine present in an SCC but absent from the workspace routine
/// set — never interned, so no row) keeps its already-assembled `db_effects`
/// untouched (see `SummaryBundle::has_row`'s doc).
pub fn compute_summaries_v2_with_leaves_core(
    routines: &[L3Routine],
    graph: &CombinedGraph,
    scc: &SccResult,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    fields: &FieldIndex,
    leaf_summaries: &HashMap<String, RoutineSummary>,
) -> (HashMap<String, RoutineSummary>, Vec<SummarizeDiagnostic>) {
    let (bundle, mut map, diagnostics) = compute_summaries_v2_bundle_with_leaves(
        routines,
        graph,
        scc,
        upgraded_bindings,
        fields,
        leaf_summaries,
    );
    for (id, summary) in map.iter_mut() {
        if let Some(rix) = bundle.routine_ix(id)
            && bundle.has_row(rix)
        {
            summary.db_effects = bundle.db_effects(rix).map(|e| e.to_owned()).collect();
        }
    }
    (map, diagnostics)
}

// ---------------------------------------------------------------------------
// compute_summaries_v2 no-leaves convenience wrapper.
// ---------------------------------------------------------------------------

/// Tuple-returning v2 entry point (no fixed leaves) — a thin convenience over
/// [`compute_summaries_v2_with_leaves_core`] with an empty leaf map, returning
/// `(map, Vec<SummarizeDiagnostic>)`. `summarize_diagnostics` is the roles
/// fixpoint's cap-hit backstop, empty on every real SCC (roles converge; the
/// db_effects path is closed-form and never caps). Callers that already have a
/// fixed-leaf map call [`compute_summaries_v2_with_leaves_core`] directly.
pub fn compute_summaries_v2(
    routines: &[L3Routine],
    graph: &CombinedGraph,
    scc: &SccResult,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    fields: &FieldIndex,
) -> (HashMap<String, RoutineSummary>, Vec<SummarizeDiagnostic>) {
    let no_leaves: HashMap<String, RoutineSummary> = HashMap::new();
    compute_summaries_v2_with_leaves_core(
        routines,
        graph,
        scc,
        upgraded_bindings,
        fields,
        &no_leaves,
    )
}

/// LEAN no-leaves entry point (⟨Task B1⟩ — the analyze-path RSS win). Returns the
/// workspace-complete [`SummaryBundle`] alongside the settled map WITHOUT the
/// per-member `Vec<DbEffect>` re-materialization the compat shim
/// [`compute_summaries_v2_with_leaves_core`] performs. The returned map's
/// `db_effects` stay EMPTY (as assembled by
/// [`compute_summaries_v2_bundle_with_leaves`], see its member-assembly loop) while
/// `uncertainties` / `parameter_roles` / `has_unresolved_calls` / `in_recursive_cycle`
/// are fully populated — exactly the fields the analyze path
/// ([`crate::engine::l5::detector_context::build_detector_context`]) consumes. The
/// db-effect rows remain QUERYABLE through the returned bundle (lazy `db_effects(rix)`
/// projection / an on-demand `ReverseEffectIndex`), so no per-routine owned
/// `Vec<DbEffect>` is ever expanded on this path. Callers that DO read
/// `RoutineSummary.db_effects` (the R3a-5 projection, the differential harness) keep
/// using the materializing [`compute_summaries_v2`] / `_with_leaves_core` shims.
pub fn compute_summaries_v2_bundle(
    routines: &[L3Routine],
    graph: &CombinedGraph,
    scc: &SccResult,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    fields: &FieldIndex,
) -> (
    SummaryBundle,
    HashMap<String, RoutineSummary>,
    Vec<SummarizeDiagnostic>,
) {
    let no_leaves: HashMap<String, RoutineSummary> = HashMap::new();
    compute_summaries_v2_bundle_with_leaves(
        routines,
        graph,
        scc,
        upgraded_bindings,
        fields,
        &no_leaves,
    )
}

/// The SHARED per-SCC compute context — the workspace-wide lookup structures the
/// v2 roles fixpoint reads (all keyed by internal RoutineId, so a single SCC's
/// loop reads only the entries it needs). Built by
/// [`compute_summaries_v2_bundle_with_leaves`] and passed to
/// [`run_one_scc_roles`].
pub struct SccComputeCtx<'a> {
    pub routines_by_id: &'a HashMap<String, &'a L3Routine>,
    pub base_summaries: &'a HashMap<String, RoutineSummary>,
    pub upgraded_bindings: &'a HashMap<String, Vec<UpgradedBinding>>,
    pub graph: &'a CombinedGraph,
    pub body_avail_by_id: &'a HashMap<String, bool>,
    pub stable_map: &'a HashMap<String, String>,
    pub leaf_summaries: &'a HashMap<String, RoutineSummary>,
    /// `graph.uncertainty_edges` indexed by source routine id (indices into
    /// `graph.uncertainty_edges`, pushed in GLOBAL order) — an O(1) per-routine
    /// lookup of "this routine's uncertainty edges" instead of scanning the whole
    /// workspace list per routine. Same edges, same order, byte-identical.
    pub uncertainty_edges_by_from: &'a HashMap<String, Vec<usize>>,
}

/// A diagnostic surfaced by the L4 summarize stage — presently just the JACOBI
/// fixed-point cap-hit (below). Structurally identical to
/// `root_classification::InfraDiagnostic` / `l5::registry::Diagnostic`
/// (severity/stage/message), the shape every engine layer uses for its own
/// diagnostics; kept local to `l4` rather than importing `l5::registry::Diagnostic`
/// so this module does not gain an upward dependency. `gate/run.rs` converts it
/// into the shared `Diagnostic` at the TS-order "summarizeDiagnostics" slot
/// exactly like it already does for `InfraDiagnostic` at the "overlay" slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummarizeDiagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// run_one_scc_roles — the roles-ONLY per-SCC fixpoint (v2 role harvest).
// ---------------------------------------------------------------------------

/// The roles-only convergence signal: EXACTLY the `parameter_roles` component of
/// [`summary_change_key`] for a stable-projected summary carrying these roles.
/// `db_effects` / `uncertainties` are irrelevant to that component, so a
/// roles-only summary (both empty) yields the identical roles slice. Routing
/// through the SAME `summary_change_key` the old full fixpoint used GUARANTEES the
/// roles change signal is byte-identical to what the retired `run_one_scc`
/// compared on for roles — this is not a re-invented key.
fn roles_change_key(
    routine_id: &str,
    roles: &[RecordRoleSummary],
    stable_map: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let summary = RoutineSummary {
        routine_id: routine_id.to_string(),
        db_effects: Vec::new(),
        in_recursive_cycle: false,
        has_unresolved_calls: false,
        uncertainties: Vec::new(),
        parameter_roles: roles.iter().map(clone_role).collect(),
    };
    let proj = project_summary_to_stable(routine_id, &summary, stable_map);
    summary_change_key(&proj).parameter_roles
}

/// The result of the roles-ONLY per-SCC fixpoint ([`run_one_scc_roles`]): each
/// member's settled `parameter_roles`, PLUS a cap-hit witness.
/// `cap_hit_stable_members` is `Some(sorted stable ids)` iff the roles JACOBI hit
/// `MAX_FIXED_POINT_ITERATIONS` without converging — the SAME honesty signal the
/// old whole-summary fixpoint raised (a `SummarizeDiagnostic` at the summarize
/// stage + a per-member `fixpoint-capped` `Uncertainty`). It is `None` on
/// convergence, which is EVERY real corpus/fixture SCC (roles converge — the old
/// fixpoint never cap-hit there, verified by the 10-fixture + CDO whole-program
/// differential), so the witness stays inert on the byte-identical golden path.
///
/// The db_effects path in v2 is closed-form and CANNOT cap, so this roles cap is
/// the only convergence backstop v2 can raise. When the OLD whole-summary fixpoint
/// caps for a db_effects-only reason (roles converged), v2 cannot reproduce it —
/// but that never happens on the corpus (old fully converges), and the goldens
/// gate would catch it as a divergence if it ever did.
struct RolesSccOut {
    roles: Vec<(String, Vec<RecordRoleSummary>)>,
    cap_hit_stable_members: Option<Vec<String>>,
}

impl RolesSccOut {
    /// Constructor for the convergent (no cap) case — every real SCC.
    fn converged(roles: Vec<(String, Vec<RecordRoleSummary>)>) -> Self {
        Self {
            roles,
            cap_hit_stable_members: None,
        }
    }
}

/// Compute ONE SCC's settled `parameter_roles` per member — the roles-ONLY
/// fixpoint that replaced the retired full Jacobi solver (`run_one_scc`) on the
/// v2 path. It runs [`compose_roles_only`] (roles slice only), so the old
/// db_effects JACOBI — which materialized ~9k-effect string sets every pass — is
/// never run during the v2 role harvest. Convergence is driven by the roles change signal
/// ONLY ([`roles_change_key`], == the old `summary_change_key`'s `parameter_roles`
/// slice).
///
/// ## Why this converges to the old fixpoint's EXACT roles
///
/// `parameter_roles` are db_effects-INDEPENDENT: [`compose_roles_only`] seeds from
/// `base_roles`, and both the cross-call fold and `cfg_walker::walk_param` read
/// ONLY callee `parameter_roles` — never callee `db_effects` / `uncertainties` /
/// `has_unresolved_calls` (verified: `cfg_walker.rs` has zero `db_effects`
/// references). So a member's roles are a pure function of its callees' roles. The
/// old full fixpoint's roles trajectory (round-by-round) is therefore IDENTICAL to
/// a roles-only Jacobi's: the extra recomputes the old fixpoint does when a
/// callee's db_effects churn (with roles unchanged) are provable no-ops for roles.
/// Dirtying a caller on the roles-slice change (rather than the whole-summary
/// change) reproduces the same per-round roles as full-Jacobi-on-roles, which
/// equals the roles subsystem of full-Jacobi-on-the-whole-summary. Cap parity: the
/// same `MAX_FIXED_POINT_ITERATIONS` bound — if roles never settle, the old
/// whole-summary fixpoint cannot settle either (roles are one of its convergence
/// components), so both stop at round `MAX` with the same round-`MAX` roles.
///
/// Mirrors the retired `run_one_scc`'s structure (JACOBI freeze/dirty-frontier
/// discipline) exactly, but carries roles-only summaries and drops the trace / cap-diagnostic
/// / cap-hit-uncertainty machinery (those belong to the db/uncertainty facts the
/// v2 solver owns, not to roles).
fn run_one_scc_roles(
    scc_entry: &super::scc::Scc,
    predecessor_final_map: &HashMap<String, RoutineSummary>,
    ctx: &SccComputeCtx,
) -> RolesSccOut {
    let leaf_summaries = ctx.leaf_summaries;

    // A roles-carrying, db_effects-EMPTY RoutineSummary — the only shape the
    // snapshot / in_progress maps need to feed callee `parameter_roles` to callers.
    fn roles_summary(id: &str, roles: Vec<RecordRoleSummary>) -> RoutineSummary {
        RoutineSummary {
            routine_id: id.to_string(),
            db_effects: Vec::new(),
            in_recursive_cycle: false,
            has_unresolved_calls: false,
            uncertainties: Vec::new(),
            parameter_roles: roles,
        }
    }

    if !scc_entry.recursive {
        // Non-recursive SCC: single pass (mirrors the retired run_one_scc's
        // early return set).
        let id = match scc_entry.members.first() {
            Some(id) => id,
            None => return RolesSccOut::converged(Vec::new()),
        };
        if leaf_summaries.contains_key(id) {
            return RolesSccOut::converged(Vec::new());
        }
        let routine = match ctx.routines_by_id.get(id) {
            Some(r) => r,
            None => return RolesSccOut::converged(Vec::new()),
        };
        let base_roles = ctx
            .base_summaries
            .get(id)
            .map(|b| b.parameter_roles.as_slice())
            .unwrap_or(&[]);
        let empty_snapshot: HashMap<String, RoutineSummary> = HashMap::new();
        let roles = compose_roles_only(
            routine,
            base_roles,
            &empty_snapshot,
            predecessor_final_map,
            ctx.upgraded_bindings,
            ctx.graph,
            ctx.body_avail_by_id,
        );
        return RolesSccOut::converged(vec![(id.clone(), roles)]);
    }

    // Recursive SCC — JACOBI fixed-point on roles only.
    //
    // Seed in_progress with base roles (leaves excluded: read from predecessor).
    let mut in_progress: HashMap<String, RoutineSummary> = HashMap::new();
    for id in &scc_entry.members {
        if leaf_summaries.contains_key(id) {
            continue;
        }
        if let Some(base) = ctx.base_summaries.get(id) {
            in_progress.insert(
                id.clone(),
                roles_summary(id, base.parameter_roles.iter().map(clone_role).collect()),
            );
        }
    }

    // Per-member cached roles change key of the CURRENT in_progress value. Seeded
    // from the base roles so the first round compares composed-vs-base exactly as
    // run_one_scc's `fp(snapshot == base)` did — but on the roles slice only.
    let mut key_cache: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for id in &scc_entry.members {
        if leaf_summaries.contains_key(id) {
            continue;
        }
        if let Some(s) = in_progress.get(id) {
            key_cache.insert(
                id.clone(),
                roles_change_key(id, &s.parameter_roles, ctx.stable_map),
            );
        }
    }

    // Intra-SCC dependents: for each member m, the members that CALL it. Roles read
    // callees EXCLUSIVELY through `edges_by_from[from].to` (the cross-call fold AND
    // the cfg walker both resolve callees that way), so these edges capture every
    // intra-SCC roles dependency — identical to the retired run_one_scc's
    // dependents graph.
    let member_set: std::collections::HashSet<&String> = scc_entry.members.iter().collect();
    let mut dependents: HashMap<&String, Vec<&String>> = HashMap::new();
    for m in &scc_entry.members {
        if let Some(edges) = ctx.graph.edges_by_from.get(m) {
            for e in edges {
                if member_set.contains(&e.to) {
                    dependents.entry(&e.to).or_default().push(m);
                }
            }
        }
    }

    // Dirty frontier. First round: every non-leaf member dirty.
    let mut dirty: std::collections::BTreeSet<String> = scc_entry
        .members
        .iter()
        .filter(|m| !leaf_summaries.contains_key(*m))
        .cloned()
        .collect();

    let mut iterations = 0usize;
    let mut changed = true;
    // Cap-hit witness — sorted stable member ids when the roles fixpoint fails to
    // converge within MAX_FIXED_POINT_ITERATIONS. `None` (converged) on every real
    // SCC; see [`RolesSccOut`].
    let mut cap_hit_stable_members: Option<Vec<String>> = None;

    while changed {
        changed = false;
        iterations += 1;

        // JACOBI: freeze prior-pass state via mem::take; recomputed members
        // accumulate in next_pass; unchanged members carried back by move.
        let snapshot: HashMap<String, RoutineSummary> = std::mem::take(&mut in_progress);
        let mut next_pass: HashMap<String, RoutineSummary> = HashMap::new();
        let mut next_dirty: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for id in &scc_entry.members {
            if leaf_summaries.contains_key(id) {
                continue;
            }
            if !dirty.contains(id) {
                // No callee's ROLES changed last round ⇒ compose_roles_only(m,
                // snapshot) is a deterministic function of the same role inputs ⇒
                // bit-identical roles. Skip; the value is carried in the merge below.
                continue;
            }
            let routine = match ctx.routines_by_id.get(id) {
                Some(r) => r,
                None => continue,
            };
            let base_roles = ctx
                .base_summaries
                .get(id)
                .map(|b| b.parameter_roles.as_slice())
                .unwrap_or(&[]);
            let next_roles = compose_roles_only(
                routine,
                base_roles,
                &snapshot,             // FROZEN: all reads from the prior pass
                predecessor_final_map, // settled summaries for already-processed SCCs
                ctx.upgraded_bindings,
                ctx.graph,
                ctx.body_avail_by_id,
            );
            let next_key = roles_change_key(id, &next_roles, ctx.stable_map);
            let member_changed = key_cache.get(id) != Some(&next_key);
            if member_changed {
                changed = true;
                key_cache.insert(id.clone(), next_key);
                if let Some(deps) = dependents.get(id) {
                    for dep in deps {
                        next_dirty.insert((*dep).clone());
                    }
                }
            }
            next_pass.insert(id.clone(), roles_summary(id, next_roles));
        }

        // Carry over members not recomputed this round; recomputed members
        // overwrite. Bit-identical to a full-Jacobi `in_progress = next_pass`.
        let mut merged = snapshot;
        for (k, v) in next_pass {
            merged.insert(k, v);
        }
        in_progress = merged;
        dirty = next_dirty;

        if iterations >= MAX_FIXED_POINT_ITERATIONS {
            // Same cap the retired run_one_scc used. Roles-only convergence
            // cannot outlast the
            // old whole-summary fixpoint (roles are one of its components), so a
            // cap-hit here mirrors a cap-hit there with the same round-MAX roles.
            // Record the honesty signal EXACTLY as the old `run_one_scc` did
            // (summary_runner.rs FIX 4): a deterministic, modelInstanceId-
            // independent, sorted stable-id member list, plus the same stderr
            // warning. The caller (`compute_summaries_v2_with_leaves_core`) turns
            // this into the `SummarizeDiagnostic` + per-member `fixpoint-capped`
            // uncertainty so the v2 path is byte-identical to old on a cap.
            let mut members: Vec<&str> = scc_entry
                .members
                .iter()
                .map(|m| {
                    ctx.stable_map
                        .get(m)
                        .map(|s| s.as_str())
                        .unwrap_or(m.as_str())
                })
                .collect();
            members.sort_unstable();
            eprintln!(
                "warning: summarize: Summary fixed-point did not converge for SCC [{}]",
                members.join(", ")
            );
            cap_hit_stable_members = Some(members.into_iter().map(str::to_string).collect());
            break;
        }
    }

    let mut out: Vec<(String, Vec<RecordRoleSummary>)> = Vec::new();
    for id in &scc_entry.members {
        if let Some(s) = in_progress.remove(id) {
            out.push((id.clone(), s.parameter_roles));
        }
    }
    RolesSccOut {
        roles: out,
        cap_hit_stable_members,
    }
}

// ---------------------------------------------------------------------------
// Project one internal RoutineSummary to stable form (for the roles-only
// fixpoint's `roles_change_key` convergence signal — see that fn above).
// ---------------------------------------------------------------------------

fn project_summary_to_stable(
    routine_id: &str,
    s: &RoutineSummary,
    stable_map: &HashMap<String, String>,
) -> PRoutineSummaryCore {
    project_routine_summary_core_internal(routine_id, s, stable_map)
}
