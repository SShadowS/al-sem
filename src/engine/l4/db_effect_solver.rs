//! L4 summary-fixpoint redesign (Phase 1) — the interned-bitvector db-effect solver.
//!
//! Step 0: effective-SCC re-decomposition. The retired `run_one_scc` (the old
//! Jacobi solver, deleted at `b4181d8`) excluded fixed leaves AND routines missing
//! from `routines_by_id` when it built a Tarjan SCC's per-member equation graph. Removing
//! those nodes from a strongly-connected component's induced subgraph can SPLIT one
//! cycle into several DAG-shaped pieces — e.g. `a -> b -> c -> a` with `b` excluded
//! degrades to `c -> a`, no cycle at all. The old solver never re-ran Tarjan on that
//! induced subgraph; the new solver does, via [`effective_sccs`].
//!
//! Step A: PD product-graph reachability ([`solve_pd_reachability`]). The ONLY
//! genuinely per-member db-effect flow is `ParameterDependent` substitution —
//! `Known`/`Unknown` effects transfer by identity and are handled by the later
//! closed-form union task (Task 5). Step A solves PD substitution as semi-naive
//! reachability over `(base_effect_id, routine, PD(param_index))` product nodes,
//! reusing `summary_runner::substitute_pd_temp_state` (the SAME per-edge
//! transition the retired `compose_routine` JACOBI fold used) so this solver
//! inherits the old PD semantics by construction, not by re-derivation.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::engine::l3::call_resolver::UpgradedBinding;
use crate::engine::l3::l3_workspace::L3Routine;
use crate::engine::l4::combined_graph::{CombinedEdge, CombinedGraph};
use crate::engine::l4::effect_lattice::{TempStateKind, via_for_edge_kind};
use crate::engine::l4::effect_store::{
    SetRef, SummaryBundleBuilder, ViaRank, has_bit, iter_set_bits, or_bits, set_bit,
};
use crate::engine::l4::effect_universe::{EffectId, EffectIdentity, GrowingEffectUniverse};
use crate::engine::l4::routine_interner::{RoutineInterner, RoutineIx};
use crate::engine::l4::scc::{Scc, SccInputGraph, tarjan_scc};
use crate::engine::l4::summary::{
    DbEffect, RoutineSummary, TempState, Uncertainty, dedupe_uncertainties, uncertainty_at,
};
use crate::engine::l4::summary_runner::summaries_census;
use crate::engine::l4::summary_runner::{
    FieldIndex, base_intraprocedural_summary, substitute_pd_temp_state,
};

/// Re-decompose one Tarjan `Scc` into its *effective* SCCs: the strongly-connected
/// components of the subgraph induced by keeping only members for which
/// `is_recomputed` is true (neither a fixed leaf nor a routine missing from
/// `routines_by_id`) and only edges between two such members.
///
/// A member excluded by `is_recomputed` contributes no node and no edge to the
/// induced subgraph — its outgoing/incoming edges are external inputs to whichever
/// effective SCC touches it, not intra-component dependencies, so the caller must
/// account for them separately (fixed-leaf substitution, not re-decomposition).
///
/// Returned in reverse-topological order (callees first), matching `tarjan_scc`'s
/// own contract — `effective_sccs` re-runs `tarjan_scc` and returns its `.sccs`
/// verbatim, so callers can fold over the result exactly like a normal SCC list.
pub fn effective_sccs(
    scc_entry: &Scc,
    graph: &CombinedGraph,
    is_recomputed: &dyn Fn(&str) -> bool,
) -> Vec<Scc> {
    // 1. Filter to recomputed members only. Keep `scc_entry.members`' own order (it
    //    is already sorted — `tarjan_scc` sorts every member list — so the induced
    //    node list is sorted too, matching `SccInputGraph::nodes`'s deterministic-DFS-
    //    roots contract).
    let nodes: Vec<String> = scc_entry
        .members
        .iter()
        .filter(|m| is_recomputed(m))
        .cloned()
        .collect();

    if nodes.is_empty() {
        return Vec::new();
    }

    let recomputed: std::collections::HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();

    // 2. Project edges: for each recomputed member, keep only `to`s that are ALSO
    //    recomputed members of THIS Scc. Edges to fixed leaves, missing routines, or
    //    nodes outside this Scc entirely are dropped — they are external inputs, not
    //    intra-component dependencies.
    let mut edges_by_from: HashMap<String, Vec<String>> = HashMap::new();
    for m in &nodes {
        let tos: Vec<String> = graph
            .edges_by_from
            .get(m)
            .into_iter()
            .flatten()
            .map(|e| e.to.as_str())
            .filter(|to| recomputed.contains(to))
            .map(|to| to.to_string())
            .collect();
        edges_by_from.insert(m.clone(), tos);
    }

    // 3. Re-run Tarjan on the induced subgraph and hand back its SCCs verbatim —
    //    already reverse-topological, already deterministic-sorted per component.
    let input = SccInputGraph {
        nodes: &nodes,
        edges_by_from: &edges_by_from,
    };
    tarjan_scc(&input).sccs
}

// ---------------------------------------------------------------------------
// Step A: PD product-graph reachability.
// ---------------------------------------------------------------------------

/// A terminal (`Known`/`Unknown`) db-effect discovered while solving `PD`
/// substitution for one effective SCC. Keyed by the member routine it landed
/// on plus the BASE effect identity — `(op, table_id, operation_id)`, the same
/// via-free/temp-state-free triple `effect_key_of` folds in as its first three
/// fragments.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TerminalEmission {
    pub routine_id: String,
    /// `(op, table_id, operation_id)`.
    pub base: (String, String, String),
    /// `Known(_)` or `Unknown` only — a `ParameterDependent` outcome never
    /// reaches here (it inserts a new product node instead; see
    /// [`solve_pd_reachability`]).
    pub temp: TempStateKind,
}

/// A `ParameterDependent` fact retained on a member: the base effect identity,
/// carried at THIS member's own caller-frame parameter index — reached after
/// zero or more re-symbolizing substitution hops from wherever the base effect
/// originated (a member's own record op, or an external successor's PD effect
/// substituted at this member's out-edge).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PdFact {
    pub routine_id: String,
    /// `(op, table_id, operation_id)`.
    pub base: (String, String, String),
    pub param_index: u32,
}

/// One product-graph state: `(routine, base identity, param_index)` — a
/// `PdFact` pre-materialization. Plain tuple internally so the worklist/visited
/// set never allocates a `PdFact` just to check membership. Task A1: `routine`
/// is a [`RoutineIx`] (a `Copy` `u32`), not a `String` — every member of an
/// effective SCC is already interned (see [`RoutineInterner::build_canonical`]),
/// so this worklist never hashes/clones a routine id string mid-solve.
/// `PdFact`/`TerminalEmission` (the OUTPUT of this module) stay `String`-keyed —
/// only the internal solve state changes representation.
type PdState = (RoutineIx, (String, String, String), u32);

/// Apply one `substitute_pd_temp_state` outcome to the worklist: a
/// `ParameterDependent(j)` result inserts (and, if unseen, enqueues) a new
/// state at `at_routine`; a `Known`/`Unknown` result emits a [`TerminalEmission`]
/// and stops (terminal states transfer by identity thereafter — Task 5).
fn apply_pd_transition(
    at_routine: RoutineIx,
    base: &(String, String, String),
    outcome: TempState,
    interner: &RoutineInterner,
    visited: &mut HashSet<PdState>,
    worklist: &mut VecDeque<PdState>,
    terminals: &mut HashSet<TerminalEmission>,
) {
    match outcome {
        TempState::ParameterDependent(j) => {
            let state: PdState = (at_routine, base.clone(), j);
            if visited.insert(state.clone()) {
                worklist.push_back(state);
            }
        }
        TempState::Known(v) => {
            terminals.insert(TerminalEmission {
                routine_id: interner.name(at_routine).to_string(),
                base: base.clone(),
                temp: TempStateKind::Known(v),
            });
        }
        TempState::Unknown => {
            terminals.insert(TerminalEmission {
                routine_id: interner.name(at_routine).to_string(),
                base: base.clone(),
                temp: TempStateKind::Unknown,
            });
        }
    }
}

/// Solve `ParameterDependent` substitution as semi-naive reachability over
/// `(base_effect_id, routine, PD(param_index))` product nodes, for ONE
/// effective SCC (`eff` — see [`effective_sccs`]).
///
/// Seeds:
///   1. Each member's OWN base (intraprocedural) PD effects — a record op on
///      the member itself whose temp-state is `ParameterDependent(i)`.
///   2. Edge-substituted images of external-callee PD effects at each
///      member's OUT-edges: an edge to a routine NOT in `eff` is resolved
///      through `settled` (the predecessor final map — already-processed
///      successor SCCs AND fixed leaves both live there, exactly like
///      the retired `compose_routine`'s `lookup` falling through to
///      `final_map`), and any
///      `ParameterDependent` effect on that settled summary is substituted
///      through the edge immediately (it can only be seeded once — the
///      external callee's summary never changes while this SCC is solved).
///
/// Transition: popping a discovered state `(base, w, i)`, for every
/// intra-effective-SCC edge `caller -> w` (caller ALSO a member of `eff`),
/// `substitute_pd_temp_state` is applied using the CALLER's own binding for
/// callee-param `i`. A `ParameterDependent(j)` result inserts the new state
/// `(base, caller, j)` (continuing the worklist if unseen); a `Known`/
/// `Unknown` result emits a [`TerminalEmission`] and stops.
///
/// The visited set is bounded by the OBSERVED `(base, routine, param_index)`
/// triples actually reached — never by an assumed `param_index <= #params`
/// (malformed/recovered input can carry an arbitrary `u32`), so a PD chasing
/// itself around a cycle re-hits an already-visited state and the worklist
/// terminates (the same monotone-fixed-point argument
/// `substitute_pd_temp_state`'s own doc makes for the JACOBI fold).
///
/// `_upgraded_bindings` is accepted (not read): `substitute_pd_temp_state`
/// resolves purely from `PCallArgumentBinding.source_temp_state` on the
/// caller's own call site, never consulting the upgraded-bindings side table
/// (that table only feeds the SEPARATE cross-call `parameter_roles`
/// composition, today in `summary_runner::compose_roles_only`) — kept in the
/// signature for parity with
/// the plan's other Step functions / Task 8's uniform per-SCC ctx wiring.
pub fn solve_pd_reachability(
    eff: &Scc,
    graph: &CombinedGraph,
    routines_by_id: &HashMap<String, &L3Routine>,
    settled_db: &SummaryBundleBuilder,
    _upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    universe: &GrowingEffectUniverse,
    interner: &RoutineInterner,
) -> (Vec<PdFact>, Vec<TerminalEmission>) {
    let member_set: HashSet<&str> = eff.members.iter().map(|s| s.as_str()).collect();

    // Intra-effective-SCC caller edges, indexed by CALLEE: for member `w`, the
    // `(caller, edge)` pairs where `caller` is ALSO an effective-SCC member and
    // `caller -> w` is a real combined-graph edge. Multiple edges for the same
    // (caller, w) pair (distinct callsites) are each kept independently — they
    // can carry different bindings and so substitute differently (the
    // multi-callsite-same-callee shape the retired `compose_routine` handled).
    // Stays keyed by `&str` (the graph's own id shape) — `w` is converted via
    // `interner.name(w)` at the ONE lookup site below (the worklist pop),
    // rather than rebuilding this whole index as `RoutineIx`-keyed.
    let mut callers_by_callee: HashMap<&str, Vec<(&str, &CombinedEdge)>> = HashMap::new();
    for v in &eff.members {
        for e in graph.edges_by_from.get(v).into_iter().flatten() {
            if member_set.contains(e.to.as_str()) {
                callers_by_callee
                    .entry(e.to.as_str())
                    .or_default()
                    .push((v.as_str(), e));
            }
        }
    }

    let mut visited: HashSet<PdState> = HashSet::new();
    let mut worklist: VecDeque<PdState> = VecDeque::new();
    let mut terminals: HashSet<TerminalEmission> = HashSet::new();

    // Seed 1: each member's OWN base PD effects. `fields` never affects
    // `db_effects` (only `parameter_roles` reads it), so deriving the base
    // summary here with an EMPTY `FieldIndex` reproduces byte-identical
    // `db_effects` to whatever the real workspace-wide `base_summaries` map
    // would carry for this member — no separate derivation logic to drift.
    let empty_fields = FieldIndex::new();
    for m in &eff.members {
        let Some(routine) = routines_by_id.get(m) else {
            continue; // effective_sccs already excludes missing members; defensive only.
        };
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        let base = base_intraprocedural_summary(routine, routines_by_id, &empty_fields);
        for e in &base.db_effects {
            if let TempState::ParameterDependent(i) = &e.temp_state {
                let base_id = (e.op.clone(), e.table_id.clone(), e.operation_id.clone());
                let state: PdState = (m_ix, base_id, *i);
                if visited.insert(state.clone()) {
                    worklist.push_back(state);
                }
            }
        }
    }

    // Seed 2: edge-substituted images of external-callee (already-settled
    // successor / fixed-leaf) PD effects at each member's OUT-edges. Task A3:
    // the callee's PD facts are read as interned `EffectId`s straight off the
    // feed-forward builder (`settled_db.pd_ids`), NOT from a materialized
    // `Vec<DbEffect>` — the identity `(op, table, operation, PD(j))` is decoded
    // from the (still-growing) universe. No String re-intern, no `Vec<DbEffect>`.
    for m in &eff.members {
        let Some(caller_routine) = routines_by_id.get(m) else {
            continue;
        };
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        for e in graph.edges_by_from.get(m).into_iter().flatten() {
            if member_set.contains(e.to.as_str()) {
                continue; // intra-effective-SCC; handled by the worklist below.
            }
            let Some(to_ix) = interner.get(&e.to) else {
                continue; // not interned (missing routine) — no PD image to seed.
            };
            if !settled_db.has_row(to_ix) {
                continue; // unresolved / not (yet) settled — no PD image to seed.
            }
            for &callee_id in settled_db.pd_ids(to_ix) {
                let identity = universe.identity(callee_id);
                let TempStateKind::ParameterDependent(j) = identity.temp else {
                    continue; // pd_ids are PD by construction; defensive.
                };
                let base_id = (
                    identity.op.clone(),
                    identity.table_id.clone(),
                    identity.operation_id.clone(),
                );
                let outcome = substitute_pd_temp_state(
                    e,
                    j,
                    caller_routine,
                    routines_by_id.get(e.to.as_str()).copied(),
                );
                apply_pd_transition(
                    m_ix,
                    &base_id,
                    outcome,
                    interner,
                    &mut visited,
                    &mut worklist,
                    &mut terminals,
                );
            }
        }
    }

    // Semi-naive worklist: pop a discovered state; for every intra-effective-
    // SCC caller edge INTO its routine, substitute through the CALLER's own
    // binding for the callee param this state's index refers to.
    while let Some((w, base_id, idx)) = worklist.pop_front() {
        if let Some(callers) = callers_by_callee.get(interner.name(w)) {
            for (v, edge) in callers {
                let Some(caller_routine) = routines_by_id.get(*v) else {
                    continue;
                };
                let v_ix = interner
                    .get(v)
                    .expect("every effective-SCC member is interned at workspace setup");
                let outcome = substitute_pd_temp_state(
                    edge,
                    idx,
                    caller_routine,
                    routines_by_id.get(edge.to.as_str()).copied(),
                );
                apply_pd_transition(
                    v_ix,
                    &base_id,
                    outcome,
                    interner,
                    &mut visited,
                    &mut worklist,
                    &mut terminals,
                );
            }
        }
    }

    // Materialize + sort for determinism. `PdFact`'s derived `Ord` compares
    // fields in declaration order — (routine_id, base, param_index) — exactly
    // the requested sort key, so a plain `.sort()` suffices.
    let mut pd_facts: Vec<PdFact> = visited
        .into_iter()
        .map(|(routine_ix, base, param_index)| PdFact {
            routine_id: interner.name(routine_ix).to_string(),
            base,
            param_index,
        })
        .collect();
    pd_facts.sort();

    // `TerminalEmission` can't derive `Ord` (`TempStateKind` doesn't), so sort
    // explicitly by (routine_id, base, temp's key fragment).
    let mut terminal_emissions: Vec<TerminalEmission> = terminals.into_iter().collect();
    terminal_emissions.sort_by(|a, b| {
        (&a.routine_id, &a.base, a.temp.key_fragment()).cmp(&(
            &b.routine_id,
            &b.base,
            b.temp.key_fragment(),
        ))
    });

    (pd_facts, terminal_emissions)
}

// ---------------------------------------------------------------------------
// Steps B/C: closed-form terminal union + per-member presence assembly.
//
// The `EffectId`-keyed bitset primitives (`set_bit`/`has_bit`/`or_bits`/
// `iter_set_bits`) live in `effect_store` now — the ONE home for the set
// representation the solve-time bitsets and the freeze-time hash-cons share
// (imported at the top of this module).
// ---------------------------------------------------------------------------

/// Per-member db-effect PRESENCE for one effective SCC (Task A3: SCC-shared).
/// The TERMINAL union `C` is stored ONCE in `terminal_union` (never cloned per
/// member — the memory win); only each member's OWN `ParameterDependent` facts
/// are per-member (`pd_by_member`), because PD re-symbolizes along the specific
/// caller-binding chain reaching that member. A member's FULL presence is the
/// (disjoint) union `terminal_union ∪ pd_by_member[m]` — see
/// [`SccPresence::member_present`] / [`SccPresence::member_present_ids`].
pub struct SccPresence {
    /// The shared closed-form TERMINAL union `C` — the terminal-typed
    /// (`Known`/`Unknown`) portion of EVERY member's presence, guaranteed
    /// PD-free (built only via [`intern_terminal_db_effect`] which skips
    /// `ParameterDependent`, plus [`TerminalEmission`]s which are terminal by
    /// construction). Recorded once per effective SCC as the shared
    /// [`crate::engine::l4::effect_store::EffectSetId`].
    pub terminal_union: Vec<u64>,
    /// Per-member `ParameterDependent`-only bits (`delta \ base`, since PD and
    /// terminal identities are disjoint EffectIds). The `pd_delta` of each
    /// member's compact row.
    pub pd_by_member: HashMap<RoutineIx, Vec<u64>>,
}

impl SccPresence {
    /// True iff `id` is present for member `m` (in the shared terminal union OR
    /// that member's own PD facts).
    fn member_present(&self, m: RoutineIx, id: EffectId) -> bool {
        has_bit(&self.terminal_union, id)
            || self.pd_by_member.get(&m).is_some_and(|pd| has_bit(pd, id))
    }

    /// Iterate member `m`'s full present ids (terminal union + its PD facts),
    /// ascending EffectId within each half. The two halves are disjoint.
    fn member_present_ids(&self, m: RoutineIx) -> impl Iterator<Item = EffectId> + '_ {
        let pd = self.pd_by_member.get(&m);
        iter_set_bits(&self.terminal_union).chain(pd.into_iter().flat_map(|b| iter_set_bits(b)))
    }
}

/// Intern a TERMINAL (`Known`/`Unknown` only) [`DbEffect`](crate::engine::l4::summary::DbEffect)
/// and OR its id into `bits`. A `ParameterDependent` effect is silently
/// skipped — Step A already accounts for it (as a retained [`PdFact`] or a
/// [`TerminalEmission`]); folding it here under its UNSUBSTITUTED index would
/// double-count under the wrong identity.
fn intern_terminal_db_effect(
    universe: &mut GrowingEffectUniverse,
    bits: &mut Vec<u64>,
    op: &str,
    table_id: &str,
    operation_id: &str,
    temp_state: &TempState,
) {
    let temp = match temp_state {
        TempState::Known(v) => TempStateKind::Known(*v),
        TempState::Unknown => TempStateKind::Unknown,
        TempState::ParameterDependent(_) => return,
    };
    let identity = EffectIdentity {
        op: op.to_string(),
        table_id: table_id.to_string(),
        operation_id: operation_id.to_string(),
        temp,
    };
    let id = universe.intern(&identity);
    set_bit(bits, id);
}

/// Compute per-member db-effect PRESENCE sets for one effective SCC (`eff`):
///
/// ```text
/// C =  union(member terminal (Known/Unknown) base effects)
///    ∪ union(ACTUAL outgoing-edge successor terminal sets)   // successors in `settled`
///    ∪ union(terminal emissions from Step A / solve_pd_reachability)
/// effects[v] = C ∪ (member-v PD facts from Step A)   // PD facts are per-member, NOT shared
/// ```
///
/// The exploit: within a strongly-connected effective SCC every member
/// reaches every other, so the TERMINAL (identity-transferred) union `C` is
/// IDENTICAL for every member — computed ONCE, over the whole membership, as
/// a single bitset — rather than folded edge-by-edge per member the way the
/// retired JACOBI `compose_routine` fold did it. Only the `ParameterDependent`
/// facts differ per member (Step A already computed them per-member, because
/// PD re-symbolizes along the SPECIFIC chain of caller bindings reaching that
/// member — it does not transfer by identity).
///
/// `C`'s three sources (all TERMINAL — `Known`/`Unknown` — effects; a `PD`
/// effect is skipped wherever it's seen, whether in a member's own base
/// summary or a settled successor's, because Step A already turned every PD
/// outcome into either a per-member [`PdFact`] or a shared [`TerminalEmission`]):
///   1. Each member's OWN base ([`base_summaries`], falling back to
///      [`base_intraprocedural_summary`] if a member is missing from it —
///      mirrors Step A's own Seed 1 fallback).
///   2. TERMINAL effects of successors reached over an ACTUAL out-edge
///      (`graph.edges_by_from[member]`) from ANY member of `eff`, where the
///      successor is already `settled` (a previously-processed effective SCC
///      or a fixed leaf) — an edge to another member of `eff` itself is
///      intra-component, not yet settled, and is skipped here (source 1 +
///      Step A already cover it via the strong-connectivity closure).
///   3. Every `terminal_emissions` entry from Step A, regardless of which
///      member it landed on — by the same strong-connectivity argument, once
///      any member's PD chain resolves to a terminal value the whole SCC
///      shares it.
#[allow(clippy::too_many_arguments)]
pub fn closed_form_union(
    eff: &Scc,
    graph: &CombinedGraph,
    routines_by_id: &HashMap<String, &L3Routine>,
    settled_db: &SummaryBundleBuilder,
    base_summaries: &HashMap<String, RoutineSummary>,
    pd_facts: &[PdFact],
    terminal_emissions: &[TerminalEmission],
    universe: &mut GrowingEffectUniverse,
    interner: &RoutineInterner,
) -> SccPresence {
    let member_set: HashSet<&str> = eff.members.iter().map(|s| s.as_str()).collect();
    let empty_fields = FieldIndex::new();

    let mut c: Vec<u64> = Vec::new();

    // Source 1: each member's own base terminal effects. `computed_base`
    // only gets initialized (and only lives) on the fallback path — the
    // common case (`base_summaries` has the member) borrows straight from
    // the map, never cloning a whole `RoutineSummary` just to read its
    // `db_effects`.
    for m in &eff.members {
        let computed_base;
        let db_effects: &[DbEffect] = if let Some(b) = base_summaries.get(m) {
            &b.db_effects
        } else if let Some(r) = routines_by_id.get(m.as_str()) {
            computed_base = base_intraprocedural_summary(r, routines_by_id, &empty_fields);
            &computed_base.db_effects
        } else {
            continue; // effective_sccs already excludes missing members; defensive only.
        };
        for e in db_effects {
            intern_terminal_db_effect(
                universe,
                &mut c,
                &e.op,
                &e.table_id,
                &e.operation_id,
                &e.temp_state,
            );
        }
    }

    // Source 2: terminal effects of settled successors, reached over an
    // actual out-edge from any member of `eff`. Task A3: the settled callee's
    // TERMINAL set already IS a bitset of interned `EffectId`s in the feed-
    // forward builder (`settled_db.terminal_bits`) — bulk word-OR it wholesale
    // into `C` (no per-effect re-intern, no `Vec<DbEffect>` walk). A callee's
    // PD facts are deliberately NOT folded here (Step A already turned them
    // into per-member `PdFact`s / shared `TerminalEmission`s), and the builder
    // keeps terminal (`terminal_bits`) and PD (`pd_ids`) apart, so folding
    // `terminal_bits` is exactly the old `intern_terminal_db_effect` filter.
    for m in &eff.members {
        for e in graph.edges_by_from.get(m).into_iter().flatten() {
            if member_set.contains(e.to.as_str()) {
                continue; // intra-effective-SCC; not settled yet.
            }
            let Some(to_ix) = interner.get(&e.to) else {
                continue; // not interned (missing routine).
            };
            if settled_db.has_row(to_ix) {
                or_bits(&mut c, settled_db.terminal_bits(to_ix));
            }
        }
    }

    // Source 3: Step A's terminal emissions, shared across the whole SCC.
    for t in terminal_emissions {
        let identity = EffectIdentity {
            op: t.base.0.clone(),
            table_id: t.base.1.clone(),
            operation_id: t.base.2.clone(),
            temp: t.temp.clone(),
        };
        let id = universe.intern(&identity);
        set_bit(&mut c, id);
    }

    // effects[v] = C ∪ member-v's own retained PD facts. Task A3: `C` is
    // recorded ONCE (in `terminal_union`) and NEVER cloned per member — only
    // each member's PD-only bits are stored per-member.
    let mut pd_by_member: HashMap<RoutineIx, Vec<u64>> = HashMap::with_capacity(eff.members.len());
    for m in &eff.members {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        let mut pd_bits: Vec<u64> = Vec::new();
        for f in pd_facts.iter().filter(|f| &f.routine_id == m) {
            let identity = EffectIdentity {
                op: f.base.0.clone(),
                table_id: f.base.1.clone(),
                operation_id: f.base.2.clone(),
                temp: TempStateKind::ParameterDependent(f.param_index),
            };
            let id = universe.intern(&identity);
            set_bit(&mut pd_bits, id);
        }
        pd_by_member.insert(m_ix, pd_bits);
    }

    SccPresence {
        terminal_union: c,
        pd_by_member,
    }
}

// ---------------------------------------------------------------------------
// Step D: via reconstruction post-pass.
// ---------------------------------------------------------------------------

/// Merge `via` into `via_map[(member, id)]` using [`ViaRank`]'s max-rank,
/// first-wins-on-tie semantics (mirrors `effect_lattice::merge_via`'s
/// `via_rank(a) >= via_rank(b) ? a : b`, `a` = the EXISTING entry). Task A2:
/// `via` is now a [`ViaRank`] (a closed 5-variant enum) rather than an
/// arbitrary `&str`, so the OLD canonicalization `debug_assert!` (guarding
/// against a bogus non-canonical string winning a rank-0 tie against a real
/// `"inherited"`) is now a COMPILE-TIME guarantee — the type system makes a
/// non-canonical via unrepresentable, so the runtime guard is no longer
/// needed (a strict improvement: compile-time > runtime-assert, per this
/// repo's "engine never throws in production" rule taken one step further).
fn merge_via_into(
    via_map: &mut HashMap<(RoutineIx, EffectId), ViaRank>,
    member: RoutineIx,
    id: EffectId,
    via: ViaRank,
) {
    let key = (member, id);
    let merged = match via_map.get(&key).copied() {
        Some(existing) if existing >= via => existing,
        _ => via,
    };
    via_map.insert(key, merged);
}

/// Reconstruct the per-`(member, EffectId)` `via` provenance in ONE
/// post-pass over the FINAL presence sets, for one effective SCC (`eff`).
///
/// The retired JACOBI fold (`compose_routine`, in the pre-`b4181d8` tree) never
/// let `via` propagate transitively: every callee effect's `via` was
/// REPLACED with `via_for_edge_kind(edge.kind)` the moment it was folded into
/// the caller, and a same-`effect_key` collision resolved by `merge_via`
/// (max rank, first-wins-on-tie). Because of that replacement rule, the
/// FINAL `via` for a present effect can be reconstructed with only ONE hop
/// per contributing edge — no multi-hop trajectory tracking needed:
///
/// ```text
/// via(m,k) = max( base_via(m,k)               if k ∈ base(m)  [always "direct"],
///                  via_for_edge_kind(e.kind)   for every ACTUAL edge e=(m,c)
///                                              and every effect k' ∈ set(c)
///                                              with T_e(k') = k )
/// ```
///
/// ## Coverage
///
/// COMPLETE for:
///   - base-seeded via (`"direct"`, for every present `EffectId` that is one
///     of `m`'s own base `db_effects` — terminal OR `ParameterDependent`;
///     both always carry `via="direct"` — see `base_intraprocedural_summary`).
///   - inherited via for TERMINAL (`Known`/`Unknown`) effects, over EVERY
///     actual out-edge, whether the callee is another member of `eff` (read
///     from `presence`) or an already-`settled` successor/fixed leaf (read
///     from its `RoutineSummary.db_effects`) — a terminal effect transfers
///     by IDENTITY (`T_e` is the identity function), so no per-edge
///     argument-binding data is needed to know which target `EffectId` an
///     edge contributes to.
///
/// DEFERRED (documented, not silently wrong — see task-6-report.md) for:
///   - inherited via on a `ParameterDependent`-typed present effect that
///     arose from an out-edge SUBSTITUTION (re-symbolizing through the
///     caller's own argument binding, `substitute_pd_temp_state`) rather
///     than from `m`'s own base. `T_e` for a PD-typed callee fact IS that
///     substitution table, which needs the CALLER'S OWN `L3Routine`
///     (specifically the call site matching `edge.callsite_id`) to resolve —
///     data this function's signature (per the plan/brief, verbatim) does
///     not carry. Task 8's `solve_scc_db_effects` DOES have `routines_by_id`
///     and `upgraded_bindings` in scope; it must either extend this function
///     or run an equivalent substitution-aware pass over PD-typed presence
///     bits before materializing `DbEffect.via`.
///
/// ## Implementation: rank-group mask-OR (Task A2 review fix, spec lines 136-139)
///
/// Rather than a per-`(member, EffectId)` `HashMap` upsert for every
/// contribution (`merge_via_into`'s original strategy here), this fn
/// accumulates FIVE per-member presence bitsets — one per [`ViaRank`] —
/// then derives each present effect's `via` as the highest rank whose
/// bitset has that effect's bit set. Two properties make this exact:
///   - `via_for_edge_kind` never returns `"direct"`, so the `Direct` group is
///     populated ONLY by the base seed (below) — never by an edge fold — and
///     `Inherited` (rank 0) is `materialize_member_db_effects`'s own default
///     for an ABSENT `via_map` entry, so that group is never even built:
///     skipping it is behaviourally identical to populating and querying it.
///   - Every intra-effective-SCC callee's TERMINAL presence is, by
///     construction, EXACTLY the shared closed-form union `C`
///     ([`SccPresence::terminal_union`] — `closed_form_union` clones `c`
///     into every member's `by_member` entry, so `C` never varies by which
///     member is asked) — so an intra-SCC edge's contribution is a single
///     bulk word-at-a-time OR of `C` into that edge's rank group
///     ([`or_bits`]), not a per-bit scan filtered by temp-state kind. A
///     settled successor's own terminal set is typically small and NOT
///     necessarily all of `C`, so that path still walks its `db_effects` by
///     identity (same cost as before).
///
/// Gating happens ONCE, in bulk, at derivation time (`has_bit` against the
/// member's FINAL presence) rather than per contribution — equivalent
/// because presence never changes between accumulation and derivation.
/// Because the 5 ranks are a bijection with the 5 canonical via strings (no
/// two distinct strings share a rank), an equal-rank tie is an identical
/// value — "first-wins" is vacuous — so max-rank reproduces `merge_via`
/// exactly, matching `merge_via_into`'s own max-rank semantics byte for
/// byte.
pub fn reconstruct_via(
    eff: &Scc,
    graph: &CombinedGraph,
    presence: &SccPresence,
    base_summaries: &HashMap<String, RoutineSummary>,
    settled_db: &SummaryBundleBuilder,
    universe: &GrowingEffectUniverse,
    interner: &RoutineInterner,
) -> HashMap<(RoutineIx, EffectId), ViaRank> {
    let member_set: HashSet<&str> = eff.members.iter().map(|s| s.as_str()).collect();
    let mut via_map: HashMap<(RoutineIx, EffectId), ViaRank> = HashMap::new();

    // Descending-rank order, `Inherited` excluded — see this fn's own doc
    // for why the floor rank is never built or queried.
    const RANKED: [ViaRank; 4] = [
        ViaRank::Direct,
        ViaRank::ImplicitTrigger,
        ViaRank::EventSubscriber,
        ViaRank::Dynamic,
    ];

    for m in &eff.members {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");

        // The 5 rank-group presence contributions, indexed by `ViaRank as
        // usize` (index 0, `Inherited`, is never populated). Accumulated
        // UNCONDITIONALLY — the presence gate is applied once, below.
        let mut rank_bits: [Vec<u64>; 5] = Default::default();

        // Base seed: every one of `m`'s own base db_effects — terminal OR
        // PD, both always carry via="direct" (base_intraprocedural_summary)
        // — contributes to the `Direct` group.
        // Every real routine has a precomputed base summary — see the retired
        // `compose_routine`'s own "this fallback is dead" note (in the
        // pre-`b4181d8` tree); reconstruct_via's signature carries
        // no routines_by_id to recompute one on the fly, so a missing entry
        // contributes nothing here, same as before.
        if let Some(base) = base_summaries.get(m) {
            for e in &base.db_effects {
                let identity = EffectIdentity {
                    op: e.op.clone(),
                    table_id: e.table_id.clone(),
                    operation_id: e.operation_id.clone(),
                    temp: e.temp_state.to_kind(),
                };
                if let Some(id) = universe.get(&identity) {
                    set_bit(&mut rank_bits[ViaRank::Direct as usize], id);
                }
            }
        }

        // Fold: every actual out-edge contributes via_for_edge_kind(edge.kind)
        // to every TERMINAL effect it carries from the callee (identity
        // transfer — see the "DEFERRED" doc section above for why PD-typed
        // callee facts are skipped here).
        for edge in graph.edges_by_from.get(m).into_iter().flatten() {
            let via = ViaRank::from_str(via_for_edge_kind(&edge.kind));
            if via == ViaRank::Inherited {
                continue; // the floor default; never queried at derivation.
            }
            if member_set.contains(edge.to.as_str()) {
                // Intra-effective-SCC callee: bulk-OR the shared terminal
                // union wholesale — see this fn's own doc for why this is
                // exact, not an approximation.
                or_bits(&mut rank_bits[via as usize], &presence.terminal_union);
            } else if let Some(to_ix) = interner.get(&edge.to)
                && settled_db.has_row(to_ix)
            {
                // Settled successor/leaf: its TERMINAL set is already a bitset
                // of interned ids in the feed-forward builder — bulk-OR it
                // (identity transfer). PD-typed callee facts are excluded by
                // construction (the builder keeps `terminal_bits` PD-free) —
                // they are the DEFERRED case handled by
                // `attribute_pd_substituted_via`.
                or_bits(
                    &mut rank_bits[via as usize],
                    settled_db.terminal_bits(to_ix),
                );
            }
            // `else`: edge.to is neither an eff member nor settled — the
            // Step-B/C closed-form union already treats it as unresolved
            // (no contribution to presence), so there is nothing to fold.
        }

        // Derive: for every effect PRESENT for `m` (the shared terminal union
        // ∪ `m`'s own PD facts — the `has_bit` presence gate applied once
        // here), take the highest rank whose group has that effect's bit set.
        for id in presence.member_present_ids(m_ix) {
            if let Some(&via) = RANKED
                .iter()
                .find(|&&r| has_bit(&rank_bits[r as usize], id))
            {
                via_map.insert((m_ix, id), via);
            }
        }
    }

    via_map
}

// ---------------------------------------------------------------------------
// Side-facts solvers: uncertainties (per-effective-SCC set union, respecting
// the callsite-local kind filter) + has_unresolved_calls (boolean-OR
// reachability).
// ---------------------------------------------------------------------------

/// The 4 uncertainty kinds the retired `compose_routine`'s inherited-union fold
/// SKIPPED (in the pre-`b4181d8` tree) when folding a CALLEE's uncertainties
/// into a CALLER — each describes a resolution failure AT one specific callsite (a
/// `member-not-found`/`external-target`/`ambiguous-overload` dispatch
/// failure, or a zero/multi-impl `interface-open-world` dispatch), so it must
/// never be attributed to a caller of the routine that owns that callsite.
/// Every real producer of these 4 kinds is a to-less
/// [`UncertaintyEdge`](crate::engine::l4::combined_graph::UncertaintyEdge)
/// (`combined_graph.rs:260,272,299,304`) — folded into its OWN routine's
/// uncertainties unconditionally (no kind filter on the way IN, in that same
/// retired fold) — so a routine that owns one of these always keeps
/// it in its own final `uncertainties`; it simply never propagates further
/// when some OTHER routine inherits from it. Checked generically by KIND
/// here (matching that retired fold's own `matches!`), not by assuming only
/// uncertainty-edges ever produce these kinds, so a future producer is still
/// filtered correctly on the inherited side without touching this function.
fn is_callsite_local_kind(kind: &str) -> bool {
    matches!(
        kind,
        "member-not-found" | "external-target" | "ambiguous-overload" | "interface-open-world"
    )
}

/// Per-member side-facts for one effective SCC: `uncertainties` (deduped +
/// sorted by [`uncertainty_key`], via [`dedupe_uncertainties`]) and
/// `has_unresolved_calls`.
pub struct SideFacts {
    pub uncertainties: HashMap<RoutineIx, Vec<Uncertainty>>,
    pub has_unresolved: HashMap<RoutineIx, bool>,
}

/// Fold `u` into the SCC-shared uncertainty map under its [`uncertainty_key`],
/// last-write-wins — WITHOUT allocating the key and WITHOUT deep-cloning the
/// record on the overwhelmingly common repeat.
///
/// `ALSEM_SUMMARIES_CENSUS=1` measured **4,397,866** calls to this fold per BC
/// Base App 8020 run over only 150,211 edges (29.3 per edge — every external
/// edge re-folds the settled callee's ENTIRE `uncertainties` vector). The
/// straightforward `shared.insert(uncertainty_key(u), u.clone())` therefore paid
/// 4.4 M `format!` allocations plus 4.4 M five-`Option<String>` deep clones, for
/// a map that ends up holding only the distinct keys.
///
/// Two changes, neither of which weakens the contract:
///
/// 1. The key is formed into a REUSED `String` buffer and hashed as a `&str`, so
///    an allocation happens only when a genuinely NEW key is inserted.
/// 2. On a repeat, the stored record is compared instead of overwritten. Last-
///    write-wins is preserved BY CONSTRUCTION: when the two records are equal,
///    overwriting is a no-op, so skipping it cannot change the result; when they
///    differ, the overwrite still happens. This is not a corpus-conditional
///    argument — it holds for any input.
///
/// `conflicts` counts the differing case, which is the ONLY case in which
/// `dedupe_uncertainties`' last-write-wins is observable at all (the key is
/// `kind|at`, so it drops `interface_name` and the losing id fields). It measured
/// **0** on both 8020 and DO, and is now reported by the census permanently and
/// for free — the counter increments only on the branch that is never taken.
#[inline]
fn fold_shared(
    shared: &mut HashMap<String, Uncertainty>,
    keybuf: &mut String,
    u: &Uncertainty,
    conflicts: &mut u64,
) {
    keybuf.clear();
    keybuf.push_str(&u.kind);
    keybuf.push('|');
    keybuf.push_str(uncertainty_at(u));
    match shared.get_mut(keybuf.as_str()) {
        Some(slot) => {
            if slot != u {
                *conflicts += 1;
                *slot = u.clone();
            }
        }
        None => {
            shared.insert(keybuf.clone(), u.clone());
        }
    }
}

/// Solve BOTH side-facts for one effective SCC (`eff`) in a single closed-form
/// pass, reproducing the retired JACOBI `compose_routine` fold (in the
/// pre-`b4181d8` tree) EXACTLY at its FIXED POINT — not its iteration.
///
/// ## Why one shared value converges for the WHOLE effective SCC
///
/// `has_unresolved_calls` propagates from callee to caller UNCONDITIONALLY —
/// no kind filter at all (that retired fold had none).
/// `uncertainties` propagates too, but only for kinds that are NOT one of the
/// 4 [`is_callsite_local_kind`] kinds. Both are
/// otherwise the same closure argument: an effective SCC is, by
/// `tarjan_scc`'s own contract, STRONGLY CONNECTED — every member can reach
/// every other member via a path of intra-effective-SCC edges (verified at
/// the JACOBI level too: the retired `run_one_scc` seeded `in_progress` with
/// EVERY non-leaf member's BASE summary before the first pass, so an intra-SCC
/// `lookup` never actually saw "no summary yet"
/// at any pass — only a truly external, never-settled target can trigger the
/// unresolved-lookup branch). Unrolling the JACOBI fixed point along an
/// intra-SCC path shows that whatever a member `w` produces LOCALLY (its own
/// base facts, its own opaque-callee/uncertainty-edge entries) — filtered to
/// the propagatable (non-callsite-local) part for uncertainties, unfiltered
/// for the boolean — eventually reaches every member `v` that can reach `w`,
/// which by strong connectivity is every member of `eff`, `v` included. So
/// at the fixed point:
///
/// ```text
/// shared_uncertainties  = ⋃_{w ∈ eff} NF(produced_at(w))
///                        ∪ ⋃_{w ∈ eff, edge w->ext, ext settled} NF(ext.uncertainties)
/// shared_has_unresolved = ⋃_{w ∈ eff} local_trigger(w)
///                        ∪ ⋃_{w ∈ eff, edge w->ext, ext settled} ext.has_unresolved_calls
/// ```
///
/// (`NF` = filter OUT the 4 callsite-local kinds; `produced_at(w)` = `w`'s own
/// base uncertainties + its own opaque-callee entries + its own
/// uncertainty-edge entries; `local_trigger(w)` = `w`'s base
/// `has_unresolved_calls`, OR an edge to a target that is NEITHER an
/// intra-`eff` member NOR settled, OR an opaque-callee-triggering edge, OR a
/// nonempty `uncertainty_edges_by_from[w]`) — identical for EVERY member of
/// `eff`, so this function computes each union ONCE rather than folding it
/// separately per member the way the old JACOBI fold does.
///
/// A member's FINAL `uncertainties` is then
/// `dedupe_uncertainties(shared_uncertainties ++ produced_at(v))` —
/// `produced_at(v)`'s own callsite-local entries (excluded from
/// `shared_uncertainties`) survive only on `v` itself, exactly like the old
/// fold never propagating them past their owner; `produced_at(v)` is placed
/// LAST so it wins any same-key tie against `shared_uncertainties` (last
/// write wins — [`dedupe_uncertainties`]'s own semantics), keeping a
/// member's OWN payload authoritative for a key it itself produced.
#[allow(clippy::too_many_arguments)]
pub fn solve_side_facts(
    eff: &Scc,
    graph: &CombinedGraph,
    routines_by_id: &HashMap<String, &L3Routine>,
    settled: &HashMap<String, RoutineSummary>,
    base_summaries: &HashMap<String, RoutineSummary>,
    uncertainty_edges_by_from: &HashMap<String, Vec<usize>>,
    body_avail_by_id: &HashMap<String, bool>,
    interner: &RoutineInterner,
) -> SideFacts {
    let member_set: HashSet<&str> = eff.members.iter().map(|s| s.as_str()).collect();
    let empty_fields = FieldIndex::new();

    // Workspace-wide body-availability, for the opaque-callee guard — mirrors
    // `compute_summaries_v2_bundle_with_leaves`'s own `body_avail_by_id`
    // construction. Threaded in by the caller
    // (`compute_summaries_v2_with_leaves_core` -> `solve_scc_db_effects` ->
    // `solve_one_effective_scc`), built ONCE for the whole run rather than
    // recomputed from `routines_by_id` on every call — this function used to
    // rebuild its own `HashMap<&str, bool>` copy of the SAME workspace-wide
    // data on EVERY invocation (once per effective SCC, i.e. once per Tarjan
    // SCC in the common case), an O(total-routines) cost paid O(N) times —
    // the second O(N²) cost found alongside `build_rvid_by_opid`'s (see that
    // fn's doc). `routines_by_id` is still threaded through for the
    // `base_intraprocedural_summary` fallback below (a missing `base_summaries`
    // entry), which needs the actual `&L3Routine`, not just its `body_available`
    // flag.

    // Per-member entries PRODUCED AT that member (base + its own opaque-callee
    // + its own uncertainty-edge entries, regardless of kind) — `produced_at(m)`
    // in the doc above. Task A1: keyed by `RoutineIx`, killing the per-member
    // `m.clone()` String allocation this map used to pay on every insert.
    let mut own_by_member: HashMap<RoutineIx, Vec<Uncertainty>> = HashMap::new();
    // The SCC-wide shared union of the PROPAGATABLE (non-callsite-local) part
    // — `shared_uncertainties` in the doc above — keyed by `uncertainty_key`
    // so a legitimate duplicate (the same source reached via two different
    // members' edges) collapses for free.
    let mut shared: HashMap<String, Uncertainty> = HashMap::new();
    // Reused across every fold in this call — see `fold_shared`.
    let mut keybuf = String::new();
    // The SCC-wide has_unresolved_calls OR — `shared_has_unresolved` above.
    let mut shared_has_unresolved = false;

    // Census populations: plain locals, folded into the atomics ONCE at the end
    // of this call, so a per-edge counter never touches an atomic on the hot path.
    let mut c_edges: u64 = 0;
    let mut c_shared_inserts: u64 = 0;
    let mut c_opaque: u64 = 0;
    let mut c_dedupe_elems: u64 = 0;
    let mut c_shared_clone_elems: u64 = 0;
    let mut c_shared_moved_elems: u64 = 0;
    let mut c_key_conflicts: u64 = 0;
    let mut c_output_elems: u64 = 0;

    for m in &eff.members {
        let _t = summaries_census::start();
        let computed_base;
        let base: &RoutineSummary = if let Some(b) = base_summaries.get(m) {
            b
        } else if let Some(r) = routines_by_id.get(m.as_str()) {
            computed_base = base_intraprocedural_summary(r, routines_by_id, &empty_fields);
            &computed_base
        } else {
            continue; // effective_sccs already excludes missing members; defensive only.
        };

        let mut own: Vec<Uncertainty> = base.uncertainties.clone();
        let mut local_hu = base.has_unresolved_calls;
        for u in &own {
            if !is_callsite_local_kind(&u.kind) {
                fold_shared(&mut shared, &mut keybuf, u, &mut c_key_conflicts);
                c_shared_inserts += 1;
            }
        }
        summaries_census::add_since(&summaries_census::SF_BASE_NANOS, _t);

        let _t = summaries_census::start();
        for edge in graph.edges_by_from.get(m).into_iter().flatten() {
            c_edges += 1;
            if !member_set.contains(edge.to.as_str()) {
                match settled.get(&edge.to) {
                    None => {
                        // Genuinely unresolved target (neither an intra-eff
                        // member nor a settled successor/leaf) — matches the
                        // old fold's `continue`: no opaque-callee check runs
                        // for THIS edge either.
                        local_hu = true;
                        continue;
                    }
                    Some(callee) => {
                        if callee.has_unresolved_calls {
                            shared_has_unresolved = true;
                        }
                        for u in &callee.uncertainties {
                            if !is_callsite_local_kind(&u.kind) {
                                fold_shared(&mut shared, &mut keybuf, u, &mut c_key_conflicts);
                                c_shared_inserts += 1;
                            }
                        }
                    }
                }
            }
            // Reaches here for: an intra-eff edge (always "resolved" at the
            // fixed point — see the doc above) OR a resolved external edge.
            // An intra-eff `to`'s own production is already folded into
            // `shared`/`own_by_member` when `to` itself is visited as `m` by
            // this same outer loop, so nothing further is needed for that
            // case beyond the opaque-callee check below, which applies
            // uniformly to every edge that reaches here (exactly like
            // the retired `compose_routine`, which never special-cased an
            // intra-SCC target for this check either).
            let callee_opaque = !body_avail_by_id
                .get(edge.to.as_str())
                .copied()
                .unwrap_or(false);
            let add_opaque = edge.kind == "interface" || edge.kind == "dynamic" || callee_opaque;
            if add_opaque {
                if let Some(cs_id) = &edge.callsite_id {
                    let u = Uncertainty {
                        kind: "opaque-callee".to_string(),
                        callsite_id: Some(cs_id.clone()),
                        operation_id: None,
                        routine_id: None,
                        interface_name: None,
                    };
                    fold_shared(&mut shared, &mut keybuf, &u, &mut c_key_conflicts);
                    c_shared_inserts += 1;
                    c_opaque += 1;
                    own.push(u);
                }
                local_hu = true;
            }
        }
        summaries_census::add_since(&summaries_census::SF_EDGE_NANOS, _t);

        let _t = summaries_census::start();
        if let Some(idxs) = uncertainty_edges_by_from.get(m) {
            if !idxs.is_empty() {
                local_hu = true;
            }
            for &i in idxs {
                let ue = &graph.uncertainty_edges[i];
                let u = Uncertainty::from(&ue.uncertainty);
                // Empirically always one of the 4 filtered kinds (every real
                // `UncertaintyEdge` producer is — see this fn's own doc), but
                // apply the SAME generic filter check here rather than
                // hardcoding that fact.
                if !is_callsite_local_kind(&u.kind) {
                    fold_shared(&mut shared, &mut keybuf, &u, &mut c_key_conflicts);
                    c_shared_inserts += 1;
                }
                own.push(u);
            }
        }
        summaries_census::add_since(&summaries_census::SF_UNCEDGE_NANOS, _t);

        if local_hu {
            shared_has_unresolved = true;
        }
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        own_by_member.insert(m_ix, own);
    }

    let _t = summaries_census::start();
    let mut shared_vec: Vec<Uncertainty> = shared.into_values().collect();
    let mut uncertainties: HashMap<RoutineIx, Vec<Uncertainty>> = HashMap::new();
    let mut has_unresolved: HashMap<RoutineIx, bool> = HashMap::new();
    let member_count = eff.members.len();
    for (i, m) in eff.members.iter().enumerate() {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        // The LAST member takes `shared_vec` by move rather than by clone: it is
        // the final reader, so the copy it would make is the copy that is then
        // dropped. The contents `all` starts from are identical either way.
        // 100,010 of the 100,922 members on 8020 are the sole member of their
        // effective SCC, so this alone removes ~3.65 M of the 3,687,409
        // `Uncertainty` deep clones the census counted here.
        let mut all = if i + 1 == member_count {
            c_shared_moved_elems += shared_vec.len() as u64;
            std::mem::take(&mut shared_vec)
        } else {
            c_shared_clone_elems += shared_vec.len() as u64;
            shared_vec.clone()
        };
        all.extend(own_by_member.remove(&m_ix).unwrap_or_default());
        c_dedupe_elems += all.len() as u64;
        let deduped = dedupe_uncertainties(all);
        c_output_elems += deduped.len() as u64;
        uncertainties.insert(m_ix, deduped);
        has_unresolved.insert(m_ix, shared_has_unresolved);
    }
    summaries_census::add_since(&summaries_census::SF_ASSEMBLE_NANOS, _t);

    summaries_census::add(&summaries_census::SF_EDGES, c_edges);
    summaries_census::add(&summaries_census::SF_SHARED_INSERTS, c_shared_inserts);
    summaries_census::add(&summaries_census::SF_OPAQUE_PUSHES, c_opaque);
    summaries_census::add(&summaries_census::SF_DEDUPE_ELEMS, c_dedupe_elems);
    summaries_census::add(
        &summaries_census::SF_SHARED_CLONE_ELEMS,
        c_shared_clone_elems,
    );
    summaries_census::add(&summaries_census::SF_OUTPUT_ELEMS, c_output_elems);
    summaries_census::add(
        &summaries_census::SF_SHARED_MOVED_ELEMS,
        c_shared_moved_elems,
    );
    summaries_census::add(&summaries_census::SF_KEY_CONFLICTS, c_key_conflicts);

    SideFacts {
        uncertainties,
        has_unresolved,
    }
}

// ---------------------------------------------------------------------------
// Task 8: one-shot per-SCC assembly — build effective SCCs, run PD → union →
// via → side-facts, materialize each member's `Vec<DbEffect>`, and return the
// per-member (db_effects, uncertainties, has_unresolved_calls) triple.
// ---------------------------------------------------------------------------

/// Convert a `TempStateKind` (universe identity form) back to the `TempState`
/// carried on a materialized [`DbEffect`]. The two enums are field-for-field
/// identical; this is the inverse of [`TempState::to_kind`]. `pub(crate)`:
/// also reused by `effect_store::DbEffectRef::to_owned` (DRY — one
/// implementation of the conversion, not a re-derived copy).
pub(crate) fn kind_to_temp_state(kind: &TempStateKind) -> TempState {
    match kind {
        TempStateKind::Known(v) => TempState::Known(*v),
        TempStateKind::ParameterDependent(i) => TempState::ParameterDependent(*i),
        TempStateKind::Unknown => TempState::Unknown,
    }
}

/// Build the `operation_id -> record_variable_id` map used to attribute a
/// materialized effect's `record_variable_id`.
///
/// `record_variable_id` is a NON-KEY payload (excluded from `effect_key` /
/// `EffectIdentity`) that the retired `compose_routine` fold carried UNCHANGED
/// from the effect's originating record operation: a base effect keeps its own
/// `op.record_variable_id`, and every inherited/PD-substituted effect keeps the
/// callee effect's payload (its `..e.clone()`, in the pre-`b4181d8` tree). Since
/// `operation_id` is part of `effect_key` (`effect_lattice.rs:133`), any two
/// effects that collide on `effect_key` share an `operation_id` and therefore
/// trace back to the SAME originating record operation — so they carry the SAME
/// `record_variable_id`. A PD-substituted effect re-keys only its temp fragment,
/// leaving `op`/`table_id`/`operation_id`/`record_variable_id` untouched, so it
/// too maps back to its origin's payload by `operation_id`. Keying the payload
/// by `operation_id` therefore reproduces the old fold's winner EXACTLY (base
/// last-write / inherited first-wins collapse to one value per `operation_id`),
/// without needing to replay the fold's iteration order.
///
/// Built from `base_summaries ∪ leaf_summaries` — **not** the growing `settled`
/// map — and, critically, built ONCE for the whole workspace by the caller
/// (`compute_summaries_v2_with_leaves_core`), never per-Tarjan-SCC. Every
/// `operation_id` that can ever appear on ANY assembled `RoutineSummary`
/// (leaf, singleton, or SCC member) originates at some routine's OWN base
/// intraprocedural record operation: `base_summaries` holds that base summary
/// for every NON-leaf routine, and `leaf_summaries` holds the already-final
/// summary for every leaf (a leaf has no separate "base" — its own db_effects
/// ARE its base, by construction). Every routine in the workspace is in
/// exactly one of those two maps (`base_summaries` is built by filtering OUT
/// `leaf_summaries.contains_key`), so their union already covers every
/// `operation_id` origin that will ever exist — a later-settled SCC's
/// materialized effects only ever re-key an operation_id already covered
/// here (identity transfer, or a PD substitution that leaves
/// `operation_id`/`record_variable_id` untouched); they never mint a new one.
/// This map therefore never needs rebuilding once `base_summaries` and
/// `leaf_summaries` are fixed for the run.
pub(crate) fn build_rvid_by_opid(
    base_summaries: &HashMap<String, RoutineSummary>,
    leaf_summaries: &HashMap<String, RoutineSummary>,
) -> HashMap<String, Option<String>> {
    let mut map: HashMap<String, Option<String>> = HashMap::new();
    for s in base_summaries.values().chain(leaf_summaries.values()) {
        for e in &s.db_effects {
            map.entry(e.operation_id.clone())
                .or_insert_with(|| e.record_variable_id.clone());
        }
    }
    map
}

/// Attribute `via` to the present effects [`reconstruct_via`] deliberately
/// leaves UNMAPPED — the PD-substitution image of a callee's
/// `ParameterDependent` effect (its "DEFERRED" case, see that fn's doc). The
/// retired JACOBI fold (`compose_routine`, in the pre-`b4181d8` tree) gave EVERY
/// inherited effect — terminal-by-identity OR PD-substituted — the SAME
/// `via_for_edge_kind(edge.kind)`, max-merged on a same-key collision. This pass
/// closes the PD-substituted half: for each member's ACTUAL out-edge, re-apply
/// `substitute_pd_temp_state` to every `ParameterDependent` callee effect (read
/// from an intra-effective-SCC callee's presence set, or from a settled
/// successor/leaf's `db_effects`), and — if the produced identity is present in
/// the caller — merge the edge-kind `via` in.
///
/// Combined with `reconstruct_via`'s base-`"direct"` seed and its
/// terminal-by-identity fold, the resulting `via_map` covers the EXACT set of
/// contributors the old fold folded (base + every callee effect over every
/// out-edge). Because `merge_via`'s 5 ranks are a bijection with its 5 canonical
/// strings (`effect_lattice.rs:161-170`; no two distinct strings share a rank),
/// the max-merge is order-INDEPENDENT — so splitting the contributor set across
/// two passes yields byte-identical `via` to the old single fold.
#[allow(clippy::too_many_arguments)]
fn attribute_pd_substituted_via(
    eff: &Scc,
    graph: &CombinedGraph,
    presence: &SccPresence,
    settled_db: &SummaryBundleBuilder,
    routines_by_id: &HashMap<String, &L3Routine>,
    universe: &GrowingEffectUniverse,
    interner: &RoutineInterner,
    via_map: &mut HashMap<(RoutineIx, EffectId), ViaRank>,
) {
    let member_set: HashSet<&str> = eff.members.iter().map(|s| s.as_str()).collect();

    for m in &eff.members {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        let Some(caller_routine) = routines_by_id.get(m.as_str()) else {
            continue;
        };
        for edge in graph.edges_by_from.get(m).into_iter().flatten() {
            let via = ViaRank::from_str(via_for_edge_kind(&edge.kind));

            // The callee's PD effects, as `(op, table_id, operation_id,
            // pd_index)`. Intra-effective-SCC callee → read from presence (its
            // PD-only bits, Task A3); settled successor/leaf → read from the
            // feed-forward builder's PD ids (Task A3 — no `Vec<DbEffect>`).
            let mut callee_pd: Vec<(String, String, String, u32)> = Vec::new();
            let mut push_pd = |cid: EffectId| {
                let identity = universe.identity(cid);
                if let TempStateKind::ParameterDependent(i) = &identity.temp {
                    callee_pd.push((
                        identity.op.clone(),
                        identity.table_id.clone(),
                        identity.operation_id.clone(),
                        *i,
                    ));
                }
            };
            if member_set.contains(edge.to.as_str()) {
                let to_ix = interner
                    .get(&edge.to)
                    .expect("every intra-effective-SCC callee is interned at workspace setup");
                if let Some(callee_pd_bits) = presence.pd_by_member.get(&to_ix) {
                    for cid in iter_set_bits(callee_pd_bits) {
                        push_pd(cid);
                    }
                }
            } else if let Some(to_ix) = interner.get(&edge.to)
                && settled_db.has_row(to_ix)
            {
                for &cid in settled_db.pd_ids(to_ix) {
                    push_pd(cid);
                }
            }

            for (op, table_id, operation_id, pd_index) in callee_pd {
                let outcome = substitute_pd_temp_state(
                    edge,
                    pd_index,
                    caller_routine,
                    routines_by_id.get(edge.to.as_str()).copied(),
                );
                let produced = EffectIdentity {
                    op,
                    table_id,
                    operation_id,
                    temp: outcome.to_kind(),
                };
                // The produced identity is present iff Step A / the closed-form
                // union already interned it (a PD fact or a terminal emission);
                // `get` (never `intern`) keeps this pass read-only over the
                // universe, and an absent id simply means the effect never
                // survived into `m`'s presence, so there is nothing to attribute.
                if let Some(pid) = universe.get(&produced)
                    && presence.member_present(m_ix, pid)
                {
                    merge_via_into(via_map, m_ix, pid, via);
                }
            }
        }
    }
}

/// Record ONE member's compact db-effect row (spec Part A Step 2 —
/// `effect_store::CompactRoutineSummary`, via [`SummaryBundleBuilder::push_row`])
/// from its presence bitset. Returns nothing: NO `Vec<DbEffect>` is
/// materialized — feed-forward is on ids (the bundle) and the owned projection
/// is lazy ([`SummaryBundle::db_effects`](crate::engine::l4::effect_store::SummaryBundle::db_effects)),
/// which resolves `record_variable_id` (from the bundle's `rvid_by_opid`),
/// `effect_key`, and `temp_state` from the frozen universe at that time. Each
/// effect's `via` comes from `via_map` (base-`direct` / terminal-inherited from
/// [`reconstruct_via`], PD-substituted from [`attribute_pd_substituted_via`]).
///
/// Task A3: `via` is carried as a `Copy` [`ViaRank`] (`u8`); the terminal half
/// is the SHARED `terminal_set` (never re-listed per member), and only this
/// member's PD delta ids/vias are per-member. `base_via` is built parallel to
/// `iter_set_bits(terminal_union)` (ascending EffectId storage order) and
/// `finish` KEEPS it in that storage order (it is the shared set's
/// `ordinal_of`-parallel via column — spec:175); only the per-row PD delta is
/// reordered to `key_rank` order.
fn materialize_member_row(
    member_ix: RoutineIx,
    presence: &SccPresence,
    via_map: &HashMap<(RoutineIx, EffectId), ViaRank>,
    terminal_set: SetRef,
    bundle: &mut SummaryBundleBuilder,
) {
    // The `Inherited` fallback (rank 0, the old fold's default
    // `via_for_edge_kind` value) is the defensive floor for a present effect
    // with no attributed via — the differential surfaces any genuine gap.
    let via_of = |id: EffectId| {
        via_map
            .get(&(member_ix, id))
            .copied()
            .unwrap_or(ViaRank::Inherited)
    };
    // base_via parallel to the shared terminal union's storage order.
    let base_via: Vec<ViaRank> = iter_set_bits(&presence.terminal_union)
        .map(via_of)
        .collect();
    // pd_delta + delta_via: this member's OWN PD facts (ascending EffectId).
    let mut pd_delta: Vec<EffectId> = Vec::new();
    let mut delta_via: Vec<ViaRank> = Vec::new();
    if let Some(pd_bits) = presence.pd_by_member.get(&member_ix) {
        for id in iter_set_bits(pd_bits) {
            pd_delta.push(id);
            delta_via.push(via_of(id));
        }
    }
    bundle.push_row(member_ix, terminal_set, base_via, pd_delta, delta_via);
}

/// Solve ONE effective SCC end-to-end (PD → union → via → side-facts → compact
/// rows). `settled` (a `RoutineSummary` map) supplies settled callees'
/// `uncertainties`/`has_unresolved_calls` to [`solve_side_facts`]; the db-effect
/// feed-forward (settled callees' terminal bits + PD ids) comes from `bundle`
/// itself (Task A3 — no materialized `Vec<DbEffect>`). Returns each member's
/// `(uncertainties, has_unresolved_calls)`; db_effects are recorded as compact
/// rows in `bundle` (projected lazily post-freeze).
#[allow(clippy::too_many_arguments)]
fn solve_one_effective_scc(
    eff: &Scc,
    graph: &CombinedGraph,
    routines_by_id: &HashMap<String, &L3Routine>,
    settled: &HashMap<String, RoutineSummary>,
    base_summaries: &HashMap<String, RoutineSummary>,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    uncertainty_edges_by_from: &HashMap<String, Vec<usize>>,
    body_avail_by_id: &HashMap<String, bool>,
    universe: &mut GrowingEffectUniverse,
    interner: &RoutineInterner,
    bundle: &mut SummaryBundleBuilder,
) -> HashMap<String, (Vec<Uncertainty>, bool)> {
    // --- Read phase: everything below reads `bundle` (the db-effect feed-
    // forward source) immutably; the shared terminal set + rows are pushed in
    // the write phase after `presence`/`via_map` are fully owned. ---

    // Step A: PD substitution as reachability.
    summaries_census::add(&summaries_census::DB_EFF_SCC_SOLVES, 1);
    let _t = summaries_census::start();
    let (pd_facts, terminal_emissions) = solve_pd_reachability(
        eff,
        graph,
        routines_by_id,
        &*bundle,
        upgraded_bindings,
        &*universe,
        interner,
    );
    summaries_census::add_since(&summaries_census::DB_PD_NANOS, _t);

    // Steps B/C: closed-form terminal union + per-member PD presence.
    let _t = summaries_census::start();
    let presence = closed_form_union(
        eff,
        graph,
        routines_by_id,
        &*bundle,
        base_summaries,
        &pd_facts,
        &terminal_emissions,
        universe,
        interner,
    );
    summaries_census::add_since(&summaries_census::DB_UNION_NANOS, _t);

    // Step D: via reconstruction (base + terminal-inherited) then the
    // PD-substituted via attribution.
    let _t = summaries_census::start();
    let mut via_map = reconstruct_via(
        eff,
        graph,
        &presence,
        base_summaries,
        &*bundle,
        &*universe,
        interner,
    );
    summaries_census::add_since(&summaries_census::DB_VIA_NANOS, _t);
    let _t = summaries_census::start();
    attribute_pd_substituted_via(
        eff,
        graph,
        &presence,
        &*bundle,
        routines_by_id,
        &*universe,
        interner,
        &mut via_map,
    );
    summaries_census::add_since(&summaries_census::DB_PDVIA_NANOS, _t);

    // Side-facts: uncertainties + has_unresolved_calls (read from the
    // `RoutineSummary` settled map, not the db-effect feed-forward).
    let _t = summaries_census::start();
    let side = solve_side_facts(
        eff,
        graph,
        routines_by_id,
        settled,
        base_summaries,
        uncertainty_edges_by_from,
        body_avail_by_id,
        interner,
    );
    summaries_census::add_since(&summaries_census::DB_SIDE_NANOS, _t);

    // --- Write phase: record the shared terminal set ONCE, then every
    // member's compact row against it (Task A3 SCC-sharing). ---
    let _t = summaries_census::start();
    let terminal_set = bundle.push_terminal_set(presence.terminal_union.clone());
    for m in &eff.members {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        materialize_member_row(m_ix, &presence, &via_map, terminal_set, bundle);
    }
    summaries_census::add_since(&summaries_census::DB_WRITE_NANOS, _t);

    let _t = summaries_census::start();
    let mut side = side;
    let mut out: HashMap<String, (Vec<Uncertainty>, bool)> = HashMap::new();
    for m in &eff.members {
        let m_ix = interner
            .get(m)
            .expect("every effective-SCC member is interned at workspace setup");
        // MOVE the member's vector out rather than deep-cloning it: `side` is
        // local and dead after this loop, and an SCC's members are distinct
        // (Tarjan), so each key is taken at most once. The census counted
        // 3,700,433 `Uncertainty` records passing through here per 8020 run —
        // every one of them was being cloned and the original immediately
        // dropped.
        let uncertainties = side.uncertainties.remove(&m_ix).unwrap_or_default();
        let has_unresolved = side.has_unresolved.get(&m_ix).copied().unwrap_or(false);
        out.insert(m.clone(), (uncertainties, has_unresolved));
    }
    summaries_census::add_since(&summaries_census::DB_OUT_NANOS, _t);
    out
}

/// One-shot per-Tarjan-SCC db-effect solve. Re-decomposes `scc_entry` into its
/// effective SCCs (Task 3), then — in `tarjan_scc`'s reverse-topological order —
/// solves each with PD reachability (Task 4) → closed-form union (Task 5) → via
/// (Task 6) + PD-substituted via attribution → side-facts (Task 7), and records
/// every member's compact row in `bundle` (projected lazily post-freeze).
/// Returns the per-member `(uncertainties, has_unresolved_calls)` pair.
///
/// ## Inter-effective-SCC feed-forward (the redesign's INTEGRATION crux)
///
/// `effective_sccs` can split ONE recursive Tarjan SCC into several sibling
/// effective SCCs (a fixed leaf / missing routine severing a cycle into DAG
/// pieces). A LATER sibling can call an EARLIER-processed one:
///
/// - **db-effect** feed-forward (a settled callee's terminal ids + PD ids) is
///   read straight from `bundle` — a GLOBAL, monotonically-growing structure —
///   so a just-solved earlier sibling's compact row is visible to a later
///   sibling with NO clone and NO materialized `Vec<DbEffect>` (Task A3, the
///   ~40GB win). Every settled routine's terminal set is an `EffectSetId` ref +
///   tiny PD delta, not a private Vec of Strings.
/// - **side-facts** feed-forward (a settled callee's `uncertainties` /
///   `has_unresolved_calls`) still reads the `RoutineSummary` settled map;
///   multi-effective-SCC siblings clone it into a LOCAL view. That clone no
///   longer carries db_effects (empty here — the db feed-forward is on
///   `bundle`), so it is cheap; multi-effective-SCC Tarjan SCCs are rare.
///
/// The common case (one effective SCC — every singleton and simple self-loop)
/// skips the clone entirely.
///
/// `body_avail_by_id` is a workspace-wide, run-invariant map built ONCE by the
/// caller (`compute_summaries_v2_with_leaves_core`) BEFORE its per-Tarjan-SCC
/// loop, not rebuilt here.
#[allow(clippy::too_many_arguments)]
pub fn solve_scc_db_effects(
    scc_entry: &Scc,
    graph: &CombinedGraph,
    routines_by_id: &HashMap<String, &L3Routine>,
    settled: &HashMap<String, RoutineSummary>,
    base_summaries: &HashMap<String, RoutineSummary>,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>,
    uncertainty_edges_by_from: &HashMap<String, Vec<usize>>,
    body_avail_by_id: &HashMap<String, bool>,
    universe: &mut GrowingEffectUniverse,
    is_recomputed: &dyn Fn(&str) -> bool,
    interner: &RoutineInterner,
    bundle: &mut SummaryBundleBuilder,
) -> HashMap<String, (Vec<Uncertainty>, bool)> {
    let _t = summaries_census::start();
    let eff_sccs = effective_sccs(scc_entry, graph, is_recomputed);
    summaries_census::add_since(&summaries_census::DB_EFFSCC_NANOS, _t);
    let mut results: HashMap<String, (Vec<Uncertainty>, bool)> = HashMap::new();
    if eff_sccs.is_empty() {
        return results;
    }

    if eff_sccs.len() == 1 {
        // Fast path: no inter-effective-SCC dependency, read `settled` directly.
        let solved = solve_one_effective_scc(
            &eff_sccs[0],
            graph,
            routines_by_id,
            settled,
            base_summaries,
            upgraded_bindings,
            uncertainty_edges_by_from,
            body_avail_by_id,
            universe,
            interner,
            bundle,
        );
        results.extend(solved);
        return results;
    }

    // General path: feed each solved sibling forward (reverse-topo order). The
    // db-effect feed-forward is on `bundle` (global); `local_settled` carries
    // ONLY the side-facts (uncertainties/has_unresolved) — its db_effects are
    // deliberately empty (never read by the sub-solvers post-A3).
    summaries_census::add(&summaries_census::DB_MULTI_EFF_SCCS, 1);
    let _t = summaries_census::start();
    let mut local_settled: HashMap<String, RoutineSummary> = settled.clone();
    summaries_census::add_since(&summaries_census::DB_LOCALSETTLED_NANOS, _t);
    for eff in &eff_sccs {
        let solved = solve_one_effective_scc(
            eff,
            graph,
            routines_by_id,
            &local_settled,
            base_summaries,
            upgraded_bindings,
            uncertainty_edges_by_from,
            body_avail_by_id,
            universe,
            interner,
            bundle,
        );
        let _t = summaries_census::start();
        for (id, (uncertainties, has_unresolved)) in &solved {
            local_settled.insert(
                id.clone(),
                RoutineSummary {
                    routine_id: id.clone(),
                    db_effects: Vec::new(),
                    in_recursive_cycle: scc_entry.recursive,
                    has_unresolved_calls: *has_unresolved,
                    uncertainties: uncertainties.clone(),
                    parameter_roles: Vec::new(),
                },
            );
        }
        summaries_census::add_since(&summaries_census::DB_LOCALSETTLED_NANOS, _t);
        results.extend(solved);
    }
    results
}

/// Seed the compact store's shared arena + rows with the RETAINED fixed leaves'
/// singleton effect classes (spec Step 3, ⟨rev3⟩), BEFORE the per-Tarjan-SCC
/// solve loop. A fixed leaf is excluded from every effective SCC
/// (`is_recomputed` is false for it), so its OWN settled summary must be
/// normalized into the compact store itself, where it serves TWO roles:
///   1. the db-effect FEED-FORWARD the solver reads when a member calls the
///      leaf (terminal ids via [`SummaryBundleBuilder::terminal_bits`], PD ids
///      via [`SummaryBundleBuilder::pd_ids`]);
///   2. a singleton `terminal_base` + `pd_delta` ROW that projects/queries like
///      any routine, preserving the leaf's OWN `via` values.
///
/// Interning every leaf effect identity here is part of the complete pre-freeze
/// identity discovery (lifecycle step 2b — "every retained fixed-leaf identity,
/// terminal AND PD"). A leaf not present in the workspace routine set (never
/// interned) gets NO row — matching "missing routines get no row".
pub fn seed_fixed_leaf_rows(
    leaf_summaries: &HashMap<String, RoutineSummary>,
    universe: &mut GrowingEffectUniverse,
    interner: &RoutineInterner,
    bundle: &mut SummaryBundleBuilder,
) {
    for (id, summary) in leaf_summaries {
        let Some(ix) = interner.get(id) else {
            continue; // a leaf not in the workspace routine set — no row.
        };
        // Intern the leaf's effects, splitting terminal (`C`) vs PD (delta),
        // remembering each id's OWN via.
        let mut terminal_bits: Vec<u64> = Vec::new();
        let mut pd_bits: Vec<u64> = Vec::new();
        let mut via_of: HashMap<EffectId, ViaRank> = HashMap::new();
        for e in &summary.db_effects {
            let identity = EffectIdentity {
                op: e.op.clone(),
                table_id: e.table_id.clone(),
                operation_id: e.operation_id.clone(),
                temp: e.temp_state.to_kind(),
            };
            let eid = universe.intern(&identity);
            via_of.insert(eid, ViaRank::from_str(&e.via));
            match e.temp_state {
                TempState::Known(_) | TempState::Unknown => set_bit(&mut terminal_bits, eid),
                TempState::ParameterDependent(_) => set_bit(&mut pd_bits, eid),
            }
        }
        let via_lookup = |eid: EffectId| via_of.get(&eid).copied().unwrap_or(ViaRank::Inherited);
        // Parallel to `iter_set_bits(terminal_bits)` (ascending EffectId).
        let base_via: Vec<ViaRank> = iter_set_bits(&terminal_bits).map(via_lookup).collect();
        let mut pd_delta: Vec<EffectId> = Vec::new();
        let mut delta_via: Vec<ViaRank> = Vec::new();
        for eid in iter_set_bits(&pd_bits) {
            pd_delta.push(eid);
            delta_via.push(via_lookup(eid));
        }
        let set_ref = bundle.push_terminal_set(terminal_bits);
        bundle.push_row(ix, set_ref, base_via, pd_delta, delta_via);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;

    /// Build a minimal `CombinedGraph` for SCC-decomposition tests: every edge is a
    /// plain "direct" call with a synthetic callsite id, no other L4 machinery
    /// (uncertainty/typed edges, event dispatch) attached.
    fn build_cycle_graph(nodes: &[&str], edges: &[(&str, &str)]) -> CombinedGraph {
        let mut sorted_nodes: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
        sorted_nodes.sort();

        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut edges_from_order: Vec<String> = Vec::new();
        for (from, to) in edges {
            let from = from.to_string();
            let to = to.to_string();
            if !edges_by_from.contains_key(&from) {
                edges_from_order.push(from.clone());
            }
            edges_by_from
                .entry(from.clone())
                .or_default()
                .push(CombinedEdge {
                    from,
                    to,
                    kind: "direct".to_string(),
                    callsite_id: Some("cs".to_string()),
                    operation_id: None,
                    event_id: None,
                    subscriber_app_id: None,
                    resolution: "resolved".to_string(),
                });
        }

        CombinedGraph {
            nodes: sorted_nodes,
            edges_by_from,
            edges_from_order,
            uncertainty_edges: Vec::new(),
            typed_edges: Vec::new(),
        }
    }

    #[test]
    fn fixed_leaf_splits_cycle_into_dag_parts() {
        // Tarjan SCC {a,b,c} with edges a->b->c->a. Mark `b` as NOT recomputed (fixed leaf).
        // Induced graph over {a,c}: a-> (b removed), c->a  => edges: c->a only. No cycle.
        let graph = build_cycle_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
        let scc = Scc {
            members: vec!["a".into(), "b".into(), "c".into()],
            recursive: true,
        };
        let eff = effective_sccs(&scc, &graph, &|id| id != "b");
        // b excluded; a and c are now acyclic (c->a). Two non-recursive singletons.
        let members: Vec<Vec<String>> = eff.iter().map(|s| s.members.clone()).collect();
        assert_eq!(eff.len(), 2, "leaf removal splits the cycle");
        assert!(eff.iter().all(|s| !s.recursive));
        // reverse-topo (callees before callers): c calls a, so a (the callee) settles
        // and is emitted BEFORE c (the caller) — this is the opposite pairing from the
        // brief's inline comment, which had caller/callee backwards; verified against
        // `tarjan_scc`'s actual output (see task-3-report.md) and invariant under DFS
        // root order (Tarjan settles a node's SCC only after every SCC it can reach).
        assert_eq!(members, vec![vec!["a".to_string()], vec!["c".to_string()]]);
    }

    #[test]
    fn missing_routine_excluded_same_as_leaf() {
        let graph = build_cycle_graph(&["a", "b"], &[("a", "b"), ("b", "a")]);
        let scc = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let eff = effective_sccs(&scc, &graph, &|id| id != "b"); // b missing
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].members, vec!["a".to_string()]);
        assert!(!eff[0].recursive);
    }

    // -----------------------------------------------------------------------
    // Step A: PD product-graph reachability (Task 4).
    //
    // Local fixture builders — NOT `crate::engine::l5::test_support` (that
    // module is `mod test_support;`, private to `l5`; `l4` is a sibling, not a
    // descendant, so it is unreachable even under `#[cfg(test)]` — the SAME
    // constraint `cfg_walker.rs`'s own test module already documents on its
    // `minimal_routine` helper).
    // -----------------------------------------------------------------------

    use crate::engine::l2::features::{
        PAnchor, PCallArgumentBinding, PCallSite, PCallee, PTempState,
    };
    use crate::engine::l3::call_resolver::UpgradedBinding;
    use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Routine, RoutineVariables};
    use crate::engine::l4::combined_graph::Uncertainty as CgUncertainty;
    use crate::engine::l4::combined_graph::UncertaintyEdge;
    use crate::engine::l4::effect_lattice::effect_key_of;
    use crate::engine::l4::scc::SccResult;
    use crate::engine::l4::summary::{DbEffect, RoutineSummary, TempState};
    use crate::engine::l4::summary_runner::compute_summaries_v2_with_leaves_core;
    use std::collections::HashSet;

    fn pd_anchor() -> PAnchor {
        PAnchor {
            source_unit_id: "ws:test".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: "test".to_string(),
        }
    }

    /// A minimal, body-available `L3Routine` with just an id; callers push
    /// `record_operations`/`call_sites` onto it (mirrors
    /// `cfg_walker::tests::minimal_routine` / the differential harness's own
    /// `routine` ctor — the SAME from-scratch pattern, unavoidable here since
    /// `l5::test_support` is private to `l5`).
    fn pd_routine(id: &str) -> L3Routine {
        L3Routine {
            id: id.to_string(),
            stable_routine_id: format!("stable::{id}"),
            object_id: "app/Codeunit/1".to_string(),
            object_type: "Codeunit".to_string(),
            name: id.to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: Vec::new(),
            app_guid: "app".to_string(),
            object_number: 1,
            normalized_signature_hash: String::new(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            source_anchor: pd_anchor(),
            identifier_references: Vec::new(),
            unreachable_statements: Vec::new(),
            has_branching: false,
            var_assignments: Vec::new(),
            condition_references: Vec::new(),
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    fn pd_ts_known(value: bool) -> PTempState {
        PTempState {
            kind: "known".to_string(),
            value: Some(value),
            parameter_index: None,
        }
    }

    fn pd_ts_pd(idx: u32) -> PTempState {
        PTempState {
            kind: "parameter-dependent".to_string(),
            value: None,
            parameter_index: Some(idx),
        }
    }

    /// One db-touching record operation with the given (always-PD-or-known)
    /// temp state; `record_variable_name` is always `"Rec"` — no fixture below
    /// needs `RecordRoleSummary` field precision.
    fn pd_record_op(
        id: &str,
        op: &str,
        table_id: &str,
        temp_state: PTempState,
    ) -> L3RecordOperation {
        L3RecordOperation {
            id: id.to_string(),
            op: op.to_string(),
            record_variable_name: "Rec".to_string(),
            record_variable_id: None,
            table_id: Some(table_id.to_string()),
            temp_state: Some(temp_state),
            field_arguments: None,
            source_anchor: pd_anchor(),
            loop_stack: Vec::new(),
            field_argument_infos: None,
            in_until_condition: false,
            run_trigger: None,
        }
    }

    /// One argument binding of callee param `parameter_index` to
    /// `source_temp_state` — the shape `substitute_pd_temp_state` reads.
    fn pd_arg_binding(
        parameter_index: u32,
        source_temp_state: Option<PTempState>,
    ) -> PCallArgumentBinding {
        PCallArgumentBinding {
            parameter_index,
            source_kind: "variable".to_string(),
            source_variable_name: Some("arg".to_string()),
            source_record_variable_id: None,
            source_parameter_index: None,
            caller_source_parameter_is_var: None,
            source_temp_state,
            argument_anchor: pd_anchor(),
        }
    }

    /// A bare call site `id` calling `callee_name`, with the given argument
    /// bindings.
    fn pd_call_site(
        id: &str,
        callee_name: &str,
        argument_bindings: Vec<PCallArgumentBinding>,
    ) -> PCallSite {
        PCallSite {
            id: id.to_string(),
            operation_id: format!("{id}/op"),
            callee_text: callee_name.to_string(),
            callee: PCallee::Bare {
                name: callee_name.to_string(),
            },
            argument_texts: Vec::new(),
            argument_infos: Vec::new(),
            argument_bindings,
            loop_stack: Vec::new(),
            source_anchor: pd_anchor(),
            result_consumed: None,
            object_run_return_used: None,
            under_asserterror: None,
            control_context: None,
            order: None,
            in_statement_position: false,
            receiver_temp_state: None,
        }
    }

    /// A resolved "direct" combined edge `from -> to` with a real callsite id
    /// (every Task-4 fixture below uses only direct, binding-carrying edges).
    fn pd_edge(from: &str, to: &str, callsite_id: &str) -> CombinedEdge {
        CombinedEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: "direct".to_string(),
            callsite_id: Some(callsite_id.to_string()),
            operation_id: None,
            event_id: None,
            subscriber_app_id: None,
            resolution: "resolved".to_string(),
        }
    }

    /// A `CombinedGraph` from a node list + flat edge list, grouped by `from`.
    fn pd_graph(nodes: &[&str], edges: Vec<CombinedEdge>) -> CombinedGraph {
        let mut node_vec: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
        node_vec.sort();
        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut edges_from_order: Vec<String> = Vec::new();
        for e in edges {
            if !edges_by_from.contains_key(&e.from) {
                edges_from_order.push(e.from.clone());
            }
            edges_by_from.entry(e.from.clone()).or_default().push(e);
        }
        CombinedGraph {
            nodes: node_vec,
            edges_by_from,
            edges_from_order,
            uncertainty_edges: Vec::new(),
            typed_edges: Vec::new(),
        }
    }

    fn pd_routines_by_id(routines: &[L3Routine]) -> HashMap<String, &L3Routine> {
        routines.iter().map(|r| (r.id.clone(), r)).collect()
    }

    /// A `RoutineInterner` covering every fixture routine, built the SAME way
    /// `compute_summaries_v2_with_leaves_core` builds the workspace-wide one
    /// (canonical `stable_routine_id` order — Task A1).
    fn pd_interner(routines: &[L3Routine]) -> RoutineInterner {
        RoutineInterner::build_canonical(
            routines
                .iter()
                .map(|r| (r.id.as_str(), r.stable_routine_id.as_str())),
        )
    }

    /// Workspace-wide `body_available` map, as
    /// `compute_summaries_v2_with_leaves_core` builds it once for the whole
    /// run and threads down to [`solve_side_facts`] — see that fn's doc for
    /// why it is no longer built internally per call.
    fn pd_body_avail(routines_by_id: &HashMap<String, &L3Routine>) -> HashMap<String, bool> {
        routines_by_id
            .iter()
            .map(|(id, r)| (id.clone(), r.body_available))
            .collect()
    }

    /// Task A3: build a settled `SummaryBundleBuilder` (the db-effect feed-
    /// forward source) carrying one routine's effects — interns them into
    /// `universe`, splits terminal (`C`) vs PD, and pushes a compact row. The
    /// routine id must already be interned in `interner`. Vias are irrelevant
    /// to the callers that read this (they read terminal ids / PD ids only), so
    /// a floor `Inherited` via is used.
    fn settled_db_with(
        universe: &mut GrowingEffectUniverse,
        interner: &RoutineInterner,
        id: &str,
        effects: &[(&str, &str, &str, TempStateKind)],
    ) -> SummaryBundleBuilder {
        let mut b = SummaryBundleBuilder::new();
        let ix = interner.get(id).expect("settled routine must be interned");
        let mut terminal: Vec<u64> = Vec::new();
        let mut pd: Vec<EffectId> = Vec::new();
        for (op, table, opid, temp) in effects {
            let eid = universe.intern(&EffectIdentity {
                op: op.to_string(),
                table_id: table.to_string(),
                operation_id: opid.to_string(),
                temp: temp.clone(),
            });
            match temp {
                TempStateKind::ParameterDependent(_) => pd.push(eid),
                _ => set_bit(&mut terminal, eid),
            }
        }
        let n_term = iter_set_bits(&terminal).count();
        let set = b.push_terminal_set(terminal);
        b.push_row(
            ix,
            set,
            vec![ViaRank::Inherited; n_term],
            pd.clone(),
            vec![ViaRank::Inherited; pd.len()],
        );
        b
    }

    /// Reconstruct member `ix`'s FULL presence bitset (the shared terminal
    /// union ∪ its own PD facts) — Task A3 split `by_member` into
    /// `terminal_union` + `pd_by_member`, so tests that assert on the complete
    /// per-member presence rebuild it here.
    fn member_full_bits(presence: &SccPresence, ix: RoutineIx) -> Vec<u64> {
        let mut bits = presence.terminal_union.clone();
        if let Some(pd) = presence.pd_by_member.get(&ix) {
            or_bits(&mut bits, pd);
        }
        bits
    }

    #[test]
    fn pd_to_pd_chain_two_hops_then_known() {
        // c (base PD(0)) <- b (forwards its own param1, PD(1)) <- a (binds
        // param1 to Known(true)): a two-hop re-symbolizing chain that
        // terminates at `a` as a Known(true) TerminalEmission.
        let mut c = pd_routine("c");
        c.record_operations
            .push(pd_record_op("c_op1", "Insert", "t1", pd_ts_pd(0)));

        let mut b = pd_routine("b");
        b.call_sites.push(pd_call_site(
            "b_cs1",
            "C",
            vec![pd_arg_binding(0, Some(pd_ts_pd(1)))],
        ));

        let mut a = pd_routine("a");
        a.call_sites.push(pd_call_site(
            "a_cs1",
            "B",
            vec![pd_arg_binding(1, Some(pd_ts_known(true)))],
        ));

        let routines = vec![a, b, c];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["a", "b", "c"],
            vec![pd_edge("b", "c", "b_cs1"), pd_edge("a", "b", "a_cs1")],
        );
        let eff = Scc {
            members: vec!["a".into(), "b".into(), "c".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let universe = GrowingEffectUniverse::new();
        let ub: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

        let (pd_facts, terminals) =
            solve_pd_reachability(&eff, &graph, &rbid, &settled_db, &ub, &universe, &interner);

        let base = ("Insert".to_string(), "t1".to_string(), "c_op1".to_string());
        let expected_facts: HashSet<PdFact> = [
            PdFact {
                routine_id: "c".to_string(),
                base: base.clone(),
                param_index: 0,
            },
            PdFact {
                routine_id: "b".to_string(),
                base: base.clone(),
                param_index: 1,
            },
        ]
        .into_iter()
        .collect();
        let expected_terminals: HashSet<TerminalEmission> = [TerminalEmission {
            routine_id: "a".to_string(),
            base: base.clone(),
            temp: TempStateKind::Known(true),
        }]
        .into_iter()
        .collect();

        assert_eq!(
            pd_facts.iter().cloned().collect::<HashSet<_>>(),
            expected_facts
        );
        assert_eq!(
            terminals.iter().cloned().collect::<HashSet<_>>(),
            expected_terminals
        );
    }

    #[test]
    fn pd_to_known_emission() {
        let mut callee = pd_routine("callee");
        callee
            .record_operations
            .push(pd_record_op("callee_op1", "Insert", "t1", pd_ts_pd(0)));

        let mut caller = pd_routine("caller");
        caller.call_sites.push(pd_call_site(
            "caller_cs1",
            "Callee",
            vec![pd_arg_binding(0, Some(pd_ts_known(true)))],
        ));

        let routines = vec![callee, caller];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["callee", "caller"],
            vec![pd_edge("caller", "callee", "caller_cs1")],
        );
        let eff = Scc {
            members: vec!["callee".into(), "caller".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let universe = GrowingEffectUniverse::new();
        let ub: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

        let (pd_facts, terminals) =
            solve_pd_reachability(&eff, &graph, &rbid, &settled_db, &ub, &universe, &interner);

        let base = (
            "Insert".to_string(),
            "t1".to_string(),
            "callee_op1".to_string(),
        );
        let expected_facts: HashSet<PdFact> = [PdFact {
            routine_id: "callee".to_string(),
            base: base.clone(),
            param_index: 0,
        }]
        .into_iter()
        .collect();
        let expected_terminals: HashSet<TerminalEmission> = [TerminalEmission {
            routine_id: "caller".to_string(),
            base: base.clone(),
            temp: TempStateKind::Known(true),
        }]
        .into_iter()
        .collect();

        assert_eq!(
            pd_facts.iter().cloned().collect::<HashSet<_>>(),
            expected_facts
        );
        assert_eq!(
            terminals.iter().cloned().collect::<HashSet<_>>(),
            expected_terminals
        );
    }

    #[test]
    fn pd_to_unknown_emission() {
        // Same shape as `pd_to_known_emission`, but the binding carries no
        // captured source temp state (e.g. an unresolved/non-record argument)
        // — substitutes to `Unknown`.
        let mut callee = pd_routine("callee");
        callee
            .record_operations
            .push(pd_record_op("callee_op1", "Insert", "t1", pd_ts_pd(0)));

        let mut caller = pd_routine("caller");
        caller.call_sites.push(pd_call_site(
            "caller_cs1",
            "Callee",
            vec![pd_arg_binding(0, None)],
        ));

        let routines = vec![callee, caller];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["callee", "caller"],
            vec![pd_edge("caller", "callee", "caller_cs1")],
        );
        let eff = Scc {
            members: vec!["callee".into(), "caller".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let universe = GrowingEffectUniverse::new();
        let ub: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

        let (pd_facts, terminals) =
            solve_pd_reachability(&eff, &graph, &rbid, &settled_db, &ub, &universe, &interner);

        let base = (
            "Insert".to_string(),
            "t1".to_string(),
            "callee_op1".to_string(),
        );
        let expected_facts: HashSet<PdFact> = [PdFact {
            routine_id: "callee".to_string(),
            base: base.clone(),
            param_index: 0,
        }]
        .into_iter()
        .collect();
        let expected_terminals: HashSet<TerminalEmission> = [TerminalEmission {
            routine_id: "caller".to_string(),
            base: base.clone(),
            temp: TempStateKind::Unknown,
        }]
        .into_iter()
        .collect();

        assert_eq!(
            pd_facts.iter().cloned().collect::<HashSet<_>>(),
            expected_facts
        );
        assert_eq!(
            terminals.iter().cloned().collect::<HashSet<_>>(),
            expected_terminals
        );
    }

    #[test]
    fn external_successor_pd_seed_resymbolizes() {
        // `ext` is an already-settled successor (NOT a member of `eff`)
        // carrying a ParameterDependent(0) effect. `a`'s out-edge to `ext`
        // forwards its OWN param 2 — the seed re-symbolizes to a NEW PdFact
        // retained on `a`, not a terminal (demonstrating a settled-derived
        // seed feeds into the same worklist machinery as an intra-eff hop).
        let mut a = pd_routine("a");
        a.call_sites.push(pd_call_site(
            "a_ext_cs",
            "Ext",
            vec![pd_arg_binding(0, Some(pd_ts_pd(2)))],
        ));

        let routines = vec![a];
        let rbid = pd_routines_by_id(&routines);
        // `ext` is a settled successor, NOT a workspace routine here — intern
        // it so the feed-forward builder can key its row.
        let mut interner = pd_interner(&routines);
        interner.intern("ext");
        let graph = pd_graph(&["a", "ext"], vec![pd_edge("a", "ext", "a_ext_cs")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: false,
        };

        let base = (
            "Insert".to_string(),
            "t1".to_string(),
            "ext_op1".to_string(),
        );
        // `ext` settled with a ParameterDependent(0) effect — supplied to the
        // feed-forward builder as an interned PD id (Task A3).
        let mut universe = GrowingEffectUniverse::new();
        let settled_db = settled_db_with(
            &mut universe,
            &interner,
            "ext",
            &[(
                "Insert",
                "t1",
                "ext_op1",
                TempStateKind::ParameterDependent(0),
            )],
        );
        let ub: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

        let (pd_facts, terminals) =
            solve_pd_reachability(&eff, &graph, &rbid, &settled_db, &ub, &universe, &interner);

        let expected_facts: HashSet<PdFact> = [PdFact {
            routine_id: "a".to_string(),
            base: base.clone(),
            param_index: 2,
        }]
        .into_iter()
        .collect();

        assert_eq!(
            pd_facts.iter().cloned().collect::<HashSet<_>>(),
            expected_facts
        );
        assert!(
            terminals.is_empty(),
            "expected no terminal emissions, got {terminals:?}"
        );
    }

    #[test]
    fn self_loop_pd_to_known() {
        // `a` calls itself; its own base op is PD(0), and its self-callsite
        // binds param 0 to Known(true) — the seed IS `a`'s own retained
        // PdFact (the retired compose_routine seeded base db_effects verbatim)
        // AND the self-edge substitution independently emits a Known terminal.
        let mut a = pd_routine("a");
        a.record_operations
            .push(pd_record_op("a_op1", "Insert", "t1", pd_ts_pd(0)));
        a.call_sites.push(pd_call_site(
            "a_cs1",
            "A",
            vec![pd_arg_binding(0, Some(pd_ts_known(true)))],
        ));

        let routines = vec![a];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(&["a"], vec![pd_edge("a", "a", "a_cs1")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let universe = GrowingEffectUniverse::new();
        let ub: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();

        let (pd_facts, terminals) =
            solve_pd_reachability(&eff, &graph, &rbid, &settled_db, &ub, &universe, &interner);

        let base = ("Insert".to_string(), "t1".to_string(), "a_op1".to_string());
        let expected_facts: HashSet<PdFact> = [PdFact {
            routine_id: "a".to_string(),
            base: base.clone(),
            param_index: 0,
        }]
        .into_iter()
        .collect();
        let expected_terminals: HashSet<TerminalEmission> = [TerminalEmission {
            routine_id: "a".to_string(),
            base: base.clone(),
            temp: TempStateKind::Known(true),
        }]
        .into_iter()
        .collect();

        assert_eq!(
            pd_facts.iter().cloned().collect::<HashSet<_>>(),
            expected_facts
        );
        assert_eq!(
            terminals.iter().cloned().collect::<HashSet<_>>(),
            expected_terminals
        );
    }

    // -----------------------------------------------------------------------
    // Steps B/C: closed-form terminal union + per-member presence (Task 5).
    // -----------------------------------------------------------------------

    #[test]
    fn recursive_members_share_terminal_union_plus_own_pd() {
        // A<->B; A has base Known(true) effect e1; B has base Unknown effect
        // e2; B has a PD fact (from Step A) on b only. Expect: A and B both
        // contain {e1,e2}; only B additionally carries its PD-keyed effect.
        let a_effect_key = effect_key_of("Insert", "t1", "a_op1", &TempStateKind::Known(true));
        let b_effect_key = effect_key_of("Modify", "t2", "b_op1", &TempStateKind::Unknown);

        let mut base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        base_summaries.insert(
            "a".to_string(),
            RoutineSummary {
                routine_id: "a".to_string(),
                db_effects: vec![DbEffect {
                    effect_key: a_effect_key,
                    operation_id: "a_op1".to_string(),
                    op: "Insert".to_string(),
                    table_id: "t1".to_string(),
                    record_variable_id: None,
                    temp_state: TempState::Known(true),
                    via: "direct".to_string(),
                }],
                in_recursive_cycle: true,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );
        base_summaries.insert(
            "b".to_string(),
            RoutineSummary {
                routine_id: "b".to_string(),
                db_effects: vec![DbEffect {
                    effect_key: b_effect_key,
                    operation_id: "b_op1".to_string(),
                    op: "Modify".to_string(),
                    table_id: "t2".to_string(),
                    record_variable_id: None,
                    temp_state: TempState::Unknown,
                    via: "direct".to_string(),
                }],
                in_recursive_cycle: true,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a"), pd_routine("b")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["a", "b"],
            vec![pd_edge("a", "b", "a_cs1"), pd_edge("b", "a", "b_cs1")],
        );
        let eff = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let pd_facts = vec![PdFact {
            routine_id: "b".to_string(),
            base: ("Delete".to_string(), "t3".to_string(), "b_op3".to_string()),
            param_index: 0,
        }];
        let terminal_emissions: Vec<TerminalEmission> = Vec::new();
        let mut universe = GrowingEffectUniverse::new();

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let e1 = universe.intern(&EffectIdentity {
            op: "Insert".to_string(),
            table_id: "t1".to_string(),
            operation_id: "a_op1".to_string(),
            temp: TempStateKind::Known(true),
        });
        let e2 = universe.intern(&EffectIdentity {
            op: "Modify".to_string(),
            table_id: "t2".to_string(),
            operation_id: "b_op1".to_string(),
            temp: TempStateKind::Unknown,
        });
        let epd = universe.intern(&EffectIdentity {
            op: "Delete".to_string(),
            table_id: "t3".to_string(),
            operation_id: "b_op3".to_string(),
            temp: TempStateKind::ParameterDependent(0),
        });

        let a_bits = member_full_bits(&presence, interner.get("a").unwrap());
        let b_bits = member_full_bits(&presence, interner.get("b").unwrap());

        assert!(has_bit(&a_bits, e1), "a must carry e1 (own base)");
        assert!(has_bit(&a_bits, e2), "a must carry e2 (shared SCC closure)");
        assert!(!has_bit(&a_bits, epd), "a must NOT carry b's own PD fact");

        assert!(has_bit(&b_bits, e1), "b must carry e1 (shared SCC closure)");
        assert!(has_bit(&b_bits, e2), "b must carry e2 (own base)");
        assert!(has_bit(&b_bits, epd), "b must carry its own PD fact");
    }

    #[test]
    fn settled_successor_terminal_contributes_to_c_but_its_pd_effect_does_not() {
        // `a` (a singleton effective SCC) calls an already-settled successor
        // `ext` over an actual out-edge. `ext`'s Known effect must join `a`'s
        // presence set (transfers by identity); `ext`'s ParameterDependent
        // effect must NOT — Step A already owns PD outcomes, and re-adding it
        // here (under its unsubstituted callee-frame index) would be wrong.
        let routines = vec![pd_routine("a")];
        let rbid = pd_routines_by_id(&routines);
        let mut interner = pd_interner(&routines);
        interner.intern("ext"); // settled successor, not a workspace routine here.
        let graph = pd_graph(&["a", "ext"], vec![pd_edge("a", "ext", "a_cs1")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: false,
        };
        let base_summaries: HashMap<String, RoutineSummary> = HashMap::new(); // "a" falls back to base_intraprocedural_summary (no own effects).
        let pd_facts: Vec<PdFact> = Vec::new();
        let terminal_emissions: Vec<TerminalEmission> = Vec::new();
        // `ext` settled with a Known + a PD effect — both interned into the
        // feed-forward builder (Task A3); the builder keeps them in SEPARATE
        // channels (`terminal_bits` vs `pd_ids`).
        let mut universe = GrowingEffectUniverse::new();
        let settled_db = settled_db_with(
            &mut universe,
            &interner,
            "ext",
            &[
                ("Insert", "t4", "ext_op1", TempStateKind::Known(true)),
                (
                    "Delete",
                    "t5",
                    "ext_op2",
                    TempStateKind::ParameterDependent(0),
                ),
            ],
        );

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let known_id = universe
            .get(&EffectIdentity {
                op: "Insert".to_string(),
                table_id: "t4".to_string(),
                operation_id: "ext_op1".to_string(),
                temp: TempStateKind::Known(true),
            })
            .expect("ext's Known effect is interned");
        // "a" has no own PD facts, so its full presence IS the shared C.
        let c = &presence.terminal_union;
        assert!(
            has_bit(c, known_id),
            "a must carry ext's settled terminal effect"
        );

        // Task A3: ext's PD id IS interned (it lives in the feed-forward
        // builder), but `closed_form_union` folds ONLY `terminal_bits` into
        // `C` — so the PD id must NOT be present in `C`.
        let pd_id = universe
            .get(&EffectIdentity {
                op: "Delete".to_string(),
                table_id: "t5".to_string(),
                operation_id: "ext_op2".to_string(),
                temp: TempStateKind::ParameterDependent(0),
            })
            .expect("ext's PD effect is interned in the feed-forward builder");
        assert!(
            !has_bit(c, pd_id),
            "ext's PD effect must NOT be folded into C (Step A owns PD outcomes)"
        );
    }

    #[test]
    fn base_pd_effect_is_skipped_from_c() {
        // A member's OWN base summary carrying a raw ParameterDependent
        // db_effect entry (as opposed to a Step-A-retained PdFact) must NOT
        // contribute to `C` — only its Known/Unknown siblings do.
        let known_key = effect_key_of("Insert", "t1", "a_op1", &TempStateKind::Known(true));
        let mut base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        base_summaries.insert(
            "a".to_string(),
            RoutineSummary {
                routine_id: "a".to_string(),
                db_effects: vec![
                    DbEffect {
                        effect_key: known_key,
                        operation_id: "a_op1".to_string(),
                        op: "Insert".to_string(),
                        table_id: "t1".to_string(),
                        record_variable_id: None,
                        temp_state: TempState::Known(true),
                        via: "direct".to_string(),
                    },
                    DbEffect {
                        effect_key: effect_key_of(
                            "Delete",
                            "t9",
                            "a_op9",
                            &TempStateKind::ParameterDependent(3),
                        ),
                        operation_id: "a_op9".to_string(),
                        op: "Delete".to_string(),
                        table_id: "t9".to_string(),
                        record_variable_id: None,
                        temp_state: TempState::ParameterDependent(3),
                        via: "direct".to_string(),
                    },
                ],
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(&["a"], Vec::new());
        let eff = Scc {
            members: vec!["a".into()],
            recursive: false,
        };
        let settled_db = SummaryBundleBuilder::new();
        let pd_facts: Vec<PdFact> = Vec::new();
        let terminal_emissions: Vec<TerminalEmission> = Vec::new();
        let mut universe = GrowingEffectUniverse::new();

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let known_id = universe.intern(&EffectIdentity {
            op: "Insert".to_string(),
            table_id: "t1".to_string(),
            operation_id: "a_op1".to_string(),
            temp: TempStateKind::Known(true),
        });
        let a_bits = member_full_bits(&presence, interner.get("a").unwrap());
        assert!(
            has_bit(&a_bits, known_id),
            "a must carry its own Known effect"
        );

        let pd_identity = EffectIdentity {
            op: "Delete".to_string(),
            table_id: "t9".to_string(),
            operation_id: "a_op9".to_string(),
            temp: TempStateKind::ParameterDependent(3),
        };
        assert_eq!(
            universe.get(&pd_identity),
            None,
            "the member's own base PD effect must never be interned by closed_form_union"
        );
    }

    #[test]
    fn terminal_emission_lands_in_every_member_presence() {
        // A<->B; no base effects, no settled successors, no PD facts — a
        // single `TerminalEmission` (Step A's output) is the ONLY source
        // contributing to `C`. Its `EffectId` bit must be set in EVERY
        // member's presence bitset (source 3's "shared across the whole
        // SCC" contract), even though it's recorded against just one
        // `routine_id` ("a") and its base identity matches nothing else in
        // scope (no base effect, no settled-successor effect).
        let routines = vec![pd_routine("a"), pd_routine("b")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["a", "b"],
            vec![pd_edge("a", "b", "a_cs1"), pd_edge("b", "a", "b_cs1")],
        );
        let eff = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        let settled_db = SummaryBundleBuilder::new();
        let pd_facts: Vec<PdFact> = Vec::new();
        let terminal_emissions = vec![TerminalEmission {
            routine_id: "a".to_string(),
            base: ("Insert".to_string(), "t7".to_string(), "op7".to_string()),
            temp: TempStateKind::Known(true),
        }];
        let mut universe = GrowingEffectUniverse::new();

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let emitted_id = universe.intern(&EffectIdentity {
            op: "Insert".to_string(),
            table_id: "t7".to_string(),
            operation_id: "op7".to_string(),
            temp: TempStateKind::Known(true),
        });

        let a_bits = member_full_bits(&presence, interner.get("a").unwrap());
        let b_bits = member_full_bits(&presence, interner.get("b").unwrap());
        assert!(
            has_bit(&a_bits, emitted_id),
            "terminal emission must land on the member it was recorded against"
        );
        assert!(
            has_bit(&b_bits, emitted_id),
            "terminal emission must be shared into EVERY member's presence, not just the one it landed on"
        );
    }

    // -----------------------------------------------------------------------
    // Step D: via reconstruction post-pass (Task 6).
    // -----------------------------------------------------------------------

    /// An edge with an explicit `kind` (unlike [`pd_edge`], which always
    /// hardcodes `"direct"`) — Task 6's fixtures need to control edge kind
    /// directly since `via_for_edge_kind` dispatches on it.
    fn kind_edge(from: &str, to: &str, kind: &str) -> CombinedEdge {
        CombinedEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.to_string(),
            callsite_id: None,
            operation_id: None,
            event_id: Some("evt".to_string()),
            subscriber_app_id: None,
            resolution: "resolved".to_string(),
        }
    }

    #[test]
    fn base_effect_seeds_direct_and_beats_a_colliding_event_dispatch_self_loop() {
        // `a` is a singleton recursive SCC (self-loop, kind="event-dispatch").
        // Its own base effect X is ALSO reachable via the self-loop (the
        // whole SCC's closed-form union is shared with itself) — via
        // reconstruction must max-merge base's "direct" (rank 4) against the
        // self-loop's "event-subscriber" (rank 2, via_for_edge_kind of
        // "event-dispatch") and keep "direct".
        let x_key = effect_key_of("Insert", "t1", "op1", &TempStateKind::Known(true));
        let mut base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        base_summaries.insert(
            "a".to_string(),
            RoutineSummary {
                routine_id: "a".to_string(),
                db_effects: vec![DbEffect {
                    effect_key: x_key,
                    operation_id: "op1".to_string(),
                    op: "Insert".to_string(),
                    table_id: "t1".to_string(),
                    record_variable_id: None,
                    temp_state: TempState::Known(true),
                    via: "direct".to_string(),
                }],
                in_recursive_cycle: true,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(&["a"], vec![kind_edge("a", "a", "event-dispatch")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let pd_facts: Vec<PdFact> = Vec::new();
        let terminal_emissions: Vec<TerminalEmission> = Vec::new();
        let mut universe = GrowingEffectUniverse::new();

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let via_map = reconstruct_via(
            &eff,
            &graph,
            &presence,
            &base_summaries,
            &settled_db,
            &universe,
            &interner,
        );

        let x_id = universe
            .get(&EffectIdentity {
                op: "Insert".to_string(),
                table_id: "t1".to_string(),
                operation_id: "op1".to_string(),
                temp: TempStateKind::Known(true),
            })
            .expect("X must already be interned by closed_form_union");

        assert_eq!(
            via_map.get(&(interner.get("a").unwrap(), x_id)).copied(),
            Some(ViaRank::Direct),
            "a base-owned effect must win over a colliding event-dispatch self-loop"
        );
    }

    #[test]
    fn non_owner_member_inherits_via_edge_kind_not_direct() {
        // a<->b recursive SCC. `a` owns base effect X. `b` has no base
        // effects of its own — it only sees X via the closed-form union
        // shared across the SCC. `a`'s via(X) must be "direct" (its own
        // base); `b`'s via(X) must be "event-subscriber" (via_for_edge_kind
        // of the b->a "event-dispatch" edge that actually carries X into
        // `b`'s presence) — `b` never owns X, so it can never earn "direct".
        let x_key = effect_key_of("Insert", "t1", "op1", &TempStateKind::Known(true));
        let mut base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        base_summaries.insert(
            "a".to_string(),
            RoutineSummary {
                routine_id: "a".to_string(),
                db_effects: vec![DbEffect {
                    effect_key: x_key,
                    operation_id: "op1".to_string(),
                    op: "Insert".to_string(),
                    table_id: "t1".to_string(),
                    record_variable_id: None,
                    temp_state: TempState::Known(true),
                    via: "direct".to_string(),
                }],
                in_recursive_cycle: true,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );
        base_summaries.insert(
            "b".to_string(),
            RoutineSummary {
                routine_id: "b".to_string(),
                db_effects: Vec::new(),
                in_recursive_cycle: true,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a"), pd_routine("b")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(
            &["a", "b"],
            vec![
                pd_edge("a", "b", "a_cs1"),
                kind_edge("b", "a", "event-dispatch"),
            ],
        );
        let eff = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let settled_db = SummaryBundleBuilder::new();
        let pd_facts: Vec<PdFact> = Vec::new();
        let terminal_emissions: Vec<TerminalEmission> = Vec::new();
        let mut universe = GrowingEffectUniverse::new();

        let presence = closed_form_union(
            &eff,
            &graph,
            &rbid,
            &settled_db,
            &base_summaries,
            &pd_facts,
            &terminal_emissions,
            &mut universe,
            &interner,
        );

        let via_map = reconstruct_via(
            &eff,
            &graph,
            &presence,
            &base_summaries,
            &settled_db,
            &universe,
            &interner,
        );

        let x_id = universe
            .get(&EffectIdentity {
                op: "Insert".to_string(),
                table_id: "t1".to_string(),
                operation_id: "op1".to_string(),
                temp: TempStateKind::Known(true),
            })
            .expect("X must already be interned by closed_form_union");

        assert_eq!(
            via_map.get(&(interner.get("a").unwrap(), x_id)).copied(),
            Some(ViaRank::Direct),
            "a owns X in its own base"
        );
        assert_eq!(
            via_map.get(&(interner.get("b").unwrap(), x_id)).copied(),
            Some(ViaRank::EventSubscriber),
            "b only inherits X via the b->a event-dispatch edge, never direct"
        );
    }

    // `canonicalization_guard_rejects_bogus_via_string` (the OLD
    // `debug_assert!`-based guard test) is REMOVED, not weakened: Task A2
    // makes `via` a `ViaRank` (a closed 5-variant enum) rather than an
    // arbitrary `&str`, so "pass a non-canonical via string" is no longer a
    // representable call — the invariant the old runtime guard checked is
    // now a COMPILE-TIME guarantee (see `merge_via_into`'s own updated doc).
    // A test asserting a debug_assert panic for an input the type system no
    // longer accepts would not compile; there is nothing left to guard.

    /// Task A3: `materialize_member_row` records a compact row (shared
    /// terminal set + per-member via) into the bundle; the frozen bundle's
    /// lazy `db_effects` then emits ids in `key_rank` (== cached `effect_key`,
    /// `operation_id` tie-break) order. Interns two identities in an order that
    /// DELIBERATELY does not match key order (Zeta before Alpha —
    /// EffectId(0)=Zeta, EffectId(1)=Alpha) so a correct emit must actually
    /// reorder rather than happening to agree with intern order.
    #[test]
    fn materialize_member_row_emits_key_rank_order_via_bundle() {
        let mut universe = GrowingEffectUniverse::new();
        let zeta_id = universe.intern(&EffectIdentity {
            op: "Zeta".to_string(),
            table_id: "t1".to_string(),
            operation_id: "op1".to_string(),
            temp: TempStateKind::Known(true),
        });
        let alpha_id = universe.intern(&EffectIdentity {
            op: "Alpha".to_string(),
            table_id: "t1".to_string(),
            operation_id: "op2".to_string(),
            temp: TempStateKind::Known(true),
        });

        let mut interner = RoutineInterner::new();
        let m_ix: RoutineIx = interner.intern("m");

        let mut via_map: HashMap<(RoutineIx, EffectId), ViaRank> = HashMap::new();
        via_map.insert((m_ix, zeta_id), ViaRank::Direct);
        via_map.insert((m_ix, alpha_id), ViaRank::Direct);

        // Build a presence with both ids terminal (shared C), no PD.
        let mut terminal_union: Vec<u64> = Vec::new();
        set_bit(&mut terminal_union, zeta_id);
        set_bit(&mut terminal_union, alpha_id);
        let presence = SccPresence {
            terminal_union: terminal_union.clone(),
            pd_by_member: HashMap::new(),
        };

        let mut bundle = SummaryBundleBuilder::new();
        let set_ref = bundle.push_terminal_set(terminal_union);
        materialize_member_row(m_ix, &presence, &via_map, set_ref, &mut bundle);

        let rvid_by_opid: HashMap<String, Option<String>> = HashMap::new();
        let frozen = bundle.finish(universe.freeze(), interner, rvid_by_opid);
        let out: Vec<_> = frozen.db_effects(m_ix).map(|e| e.to_owned()).collect();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].op, "Alpha", "Alpha sorts before Zeta by effect_key");
        assert_eq!(out[1].op, "Zeta");
    }

    // -----------------------------------------------------------------------
    // Side-facts solvers: uncertainties + has_unresolved_calls (Task 7).
    // -----------------------------------------------------------------------

    /// Build the `from`-indexed uncertainty-edge lookup exactly like
    /// `compute_summaries_v2_bundle_with_leaves` does, so
    /// a test's hand-built `graph.uncertainty_edges` and its
    /// `uncertainty_edges_by_from` argument stay consistent with each other.
    fn index_uncertainty_edges(graph: &CombinedGraph) -> HashMap<String, Vec<usize>> {
        let mut by_from: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, ue) in graph.uncertainty_edges.iter().enumerate() {
            by_from.entry(ue.from.clone()).or_default().push(i);
        }
        by_from
    }

    #[test]
    fn inherited_callsite_local_kind_filtered_generic_kind_propagates() {
        // `a` (singleton effective SCC, no self-loop) has a resolved "direct"
        // edge to an already-settled `ext` carrying BOTH a callsite-local
        // kind (`member-not-found` — must be filtered from the inherited
        // union) and a generic non-filtered kind (`parse-incomplete` — must
        // propagate). `ext` is body-available so this edge never ALSO
        // triggers the (separately tested) opaque-callee path.
        let member_not_found = Uncertainty {
            kind: "member-not-found".to_string(),
            callsite_id: Some("ext_cs1".to_string()),
            operation_id: None,
            routine_id: None,
            interface_name: None,
        };
        let generic = Uncertainty {
            kind: "parse-incomplete".to_string(),
            callsite_id: None,
            operation_id: None,
            routine_id: Some("ext".to_string()),
            interface_name: None,
        };
        let mut settled: HashMap<String, RoutineSummary> = HashMap::new();
        settled.insert(
            "ext".to_string(),
            RoutineSummary {
                routine_id: "ext".to_string(),
                db_effects: Vec::new(),
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: vec![member_not_found, generic],
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a"), pd_routine("ext")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(&["a", "ext"], vec![pd_edge("a", "ext", "a_cs1")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: false,
        };
        let base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        let uncertainty_edges_by_from = index_uncertainty_edges(&graph);
        let body_avail_by_id = pd_body_avail(&rbid);

        let facts = solve_side_facts(
            &eff,
            &graph,
            &rbid,
            &settled,
            &base_summaries,
            &uncertainty_edges_by_from,
            &body_avail_by_id,
            &interner,
        );

        let a_uncertainties = facts
            .uncertainties
            .get(&interner.get("a").unwrap())
            .expect("a present");
        assert!(
            a_uncertainties.iter().any(|u| u.kind == "parse-incomplete"),
            "generic non-filtered kind must propagate: {a_uncertainties:?}"
        );
        assert!(
            !a_uncertainties.iter().any(|u| u.kind == "member-not-found"),
            "callsite-local kind must be filtered from the inherited union: {a_uncertainties:?}"
        );
        assert_eq!(
            facts
                .has_unresolved
                .get(&interner.get("a").unwrap())
                .copied(),
            Some(false),
            "no local/opaque/edge trigger fired; ext itself is not unresolved"
        );
    }

    #[test]
    fn opaque_callee_edge_adds_uncertainty_and_sets_has_unresolved() {
        // `a` has a resolved "direct" edge to `ext`, a routine that IS
        // present in `settled` (a resolved call) but carries
        // `body_available: false` (an ABI-only stub) — FIX 3's scenario: a
        // body-available caller with a resolved DIRECT edge to a bodyless
        // callee. Must add an `opaque-callee` uncertainty keyed by the
        // edge's OWN callsite_id and set `has_unresolved_calls`.
        let mut ext = pd_routine("ext");
        ext.body_available = false;

        let mut settled: HashMap<String, RoutineSummary> = HashMap::new();
        settled.insert(
            "ext".to_string(),
            RoutineSummary {
                routine_id: "ext".to_string(),
                db_effects: Vec::new(),
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a"), ext];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let graph = pd_graph(&["a", "ext"], vec![pd_edge("a", "ext", "a_cs1")]);
        let eff = Scc {
            members: vec!["a".into()],
            recursive: false,
        };
        let base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        let uncertainty_edges_by_from = index_uncertainty_edges(&graph);
        let body_avail_by_id = pd_body_avail(&rbid);

        let facts = solve_side_facts(
            &eff,
            &graph,
            &rbid,
            &settled,
            &base_summaries,
            &uncertainty_edges_by_from,
            &body_avail_by_id,
            &interner,
        );

        let a_uncertainties = facts
            .uncertainties
            .get(&interner.get("a").unwrap())
            .expect("a present");
        assert!(
            a_uncertainties
                .iter()
                .any(|u| u.kind == "opaque-callee" && u.callsite_id.as_deref() == Some("a_cs1")),
            "expected an opaque-callee uncertainty keyed by the edge's callsite_id: {a_uncertainties:?}"
        );
        assert_eq!(
            facts
                .has_unresolved
                .get(&interner.get("a").unwrap())
                .copied(),
            Some(true)
        );
    }

    #[test]
    fn recursive_members_share_uncertainty_union_but_not_filtered_kind() {
        // A<->B (recursive effective SCC). `a` has an out-edge to a settled
        // `ext` carrying a generic (`parse-incomplete`) uncertainty — `b`
        // never touches `ext` directly, but must still inherit it via the
        // SCC closure. `a` ALSO owns a `member-not-found` uncertainty-edge —
        // that one must stay on `a` only; `b` must NOT see it.
        let generic = Uncertainty {
            kind: "parse-incomplete".to_string(),
            callsite_id: None,
            operation_id: None,
            routine_id: Some("ext".to_string()),
            interface_name: None,
        };
        let mut settled: HashMap<String, RoutineSummary> = HashMap::new();
        settled.insert(
            "ext".to_string(),
            RoutineSummary {
                routine_id: "ext".to_string(),
                db_effects: Vec::new(),
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: vec![generic],
                parameter_roles: Vec::new(),
            },
        );

        let routines = vec![pd_routine("a"), pd_routine("b"), pd_routine("ext")];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);
        let mut graph = pd_graph(
            &["a", "b", "ext"],
            vec![
                pd_edge("a", "b", "a_cs1"),
                pd_edge("b", "a", "b_cs1"),
                pd_edge("a", "ext", "a_cs2"),
            ],
        );
        graph.uncertainty_edges.push(UncertaintyEdge {
            from: "a".to_string(),
            uncertainty: CgUncertainty {
                kind: "member-not-found".to_string(),
                callsite_id: Some("a_mnf_cs".to_string()),
                operation_id: None,
                routine_id: None,
                interface_name: None,
            },
        });
        let uncertainty_edges_by_from = index_uncertainty_edges(&graph);

        let eff = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        let body_avail_by_id = pd_body_avail(&rbid);

        let facts = solve_side_facts(
            &eff,
            &graph,
            &rbid,
            &settled,
            &base_summaries,
            &uncertainty_edges_by_from,
            &body_avail_by_id,
            &interner,
        );

        let a_u = facts
            .uncertainties
            .get(&interner.get("a").unwrap())
            .expect("a present");
        let b_u = facts
            .uncertainties
            .get(&interner.get("b").unwrap())
            .expect("b present");

        assert!(
            a_u.iter().any(|u| u.kind == "parse-incomplete"),
            "a: {a_u:?}"
        );
        assert!(
            b_u.iter().any(|u| u.kind == "parse-incomplete"),
            "b must share the SCC-wide generic uncertainty: {b_u:?}"
        );
        assert!(
            a_u.iter().any(|u| u.kind == "member-not-found"),
            "a keeps its own callsite-local uncertainty: {a_u:?}"
        );
        assert!(
            !b_u.iter().any(|u| u.kind == "member-not-found"),
            "b must NOT inherit a's callsite-local uncertainty: {b_u:?}"
        );
        // `a`'s uncertainty-edge (regardless of its FILTERED kind) sets
        // has_unresolved_calls unconditionally (the retired Jacobi fold set
        // it for EVERY uncertainty edge, with no kind check at all — unlike
        // the uncertainties filter, this boolean is never callsite-local),
        // and that boolean is OR-shared across the whole strongly-connected
        // effective SCC, so `b` inherits `true` too even though `b` owns no
        // uncertainty-edge of its own.
        assert_eq!(
            facts
                .has_unresolved
                .get(&interner.get("a").unwrap())
                .copied(),
            Some(true)
        );
        assert_eq!(
            facts
                .has_unresolved
                .get(&interner.get("b").unwrap())
                .copied(),
            Some(true)
        );
    }

    #[test]
    fn oracle_parity_with_compose_routine_for_filter_opaque_and_uncertainty_edge() {
        // Build a REAL fixture — {a,b} recursive effective SCC, `ext` an
        // external bodyless (opaque) successor, `a` parse-incomplete (a
        // generic base uncertainty) and owning a `member-not-found`
        // uncertainty-edge — run the v2 solver
        // (`compute_summaries_v2_with_leaves_core`) over the WHOLE workspace
        // (ext's singleton SCC settles first, exactly the ordering the real
        // assembly uses) purely to obtain `ext`'s settled successor summary,
        // then assert `solve_side_facts` (the KEPT v2 uncertainty/roles
        // side-solver) reproduces the converged
        // `uncertainties`/`has_unresolved_calls` BIT FOR BIT. Was originally
        // seeded from the old Jacobi `compute_summaries_with_leaves`; since v2
        // is byte-identical for `ext` (a trivial bodyless-opaque summary) the
        // seed source moved to v2 when the old solver was retired (Part B) —
        // `solve_side_facts` remains the subject under test.
        let mut a = pd_routine("a");
        a.parse_incomplete = true;
        let b = pd_routine("b");
        let mut ext = pd_routine("ext");
        ext.body_available = false;

        let routines = vec![a, b, ext];
        let rbid = pd_routines_by_id(&routines);
        let interner = pd_interner(&routines);

        let mut graph = pd_graph(
            &["a", "b", "ext"],
            vec![
                pd_edge("a", "b", "a_cs1"),
                pd_edge("b", "a", "b_cs1"),
                pd_edge("a", "ext", "a_cs2"),
            ],
        );
        graph.uncertainty_edges.push(UncertaintyEdge {
            from: "a".to_string(),
            uncertainty: CgUncertainty {
                kind: "member-not-found".to_string(),
                callsite_id: Some("a_mnf_cs".to_string()),
                operation_id: None,
                routine_id: None,
                interface_name: None,
            },
        });
        let uncertainty_edges_by_from = index_uncertainty_edges(&graph);

        let scc = SccResult {
            sccs: vec![
                Scc {
                    members: vec!["ext".into()],
                    recursive: false,
                },
                Scc {
                    members: vec!["a".into(), "b".into()],
                    recursive: true,
                },
            ],
            scc_id_by_routine: HashMap::new(),
        };
        let upgraded_bindings: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();
        let fields = FieldIndex::new();
        let leaf_summaries: HashMap<String, RoutineSummary> = HashMap::new();

        let (v2_map, _diag) = compute_summaries_v2_with_leaves_core(
            &routines,
            &graph,
            &scc,
            &upgraded_bindings,
            &fields,
            &leaf_summaries,
        );

        // Feed solve_side_facts the same inputs the surrounding Task-8
        // assembly would have in scope once `ext`'s effective SCC has
        // already settled.
        let eff = Scc {
            members: vec!["a".into(), "b".into()],
            recursive: true,
        };
        let mut settled: HashMap<String, RoutineSummary> = HashMap::new();
        settled.insert(
            "ext".to_string(),
            v2_map.get("ext").expect("ext settled").clone(),
        );
        let mut base_summaries: HashMap<String, RoutineSummary> = HashMap::new();
        for id in ["a", "b"] {
            let r = *rbid.get(id).expect("routine present");
            base_summaries.insert(
                id.to_string(),
                base_intraprocedural_summary(r, &rbid, &fields),
            );
        }
        let body_avail_by_id = pd_body_avail(&rbid);

        let facts = solve_side_facts(
            &eff,
            &graph,
            &rbid,
            &settled,
            &base_summaries,
            &uncertainty_edges_by_from,
            &body_avail_by_id,
            &interner,
        );

        for m in ["a", "b"] {
            let full_summary = v2_map
                .get(m)
                .unwrap_or_else(|| panic!("v2 solver missing {m}"));
            let m_ix = interner.get(m).expect("routine present");
            assert_eq!(
                facts.uncertainties.get(&m_ix).cloned().unwrap_or_default(),
                full_summary.uncertainties,
                "[{m}] uncertainties mismatch vs full v2 solver oracle"
            );
            assert_eq!(
                facts.has_unresolved.get(&m_ix).copied(),
                Some(full_summary.has_unresolved_calls),
                "[{m}] has_unresolved_calls mismatch vs full v2 solver oracle"
            );
        }
    }
}
