//! `d1_temp` — Task 2 of the d1-reachability redesign
//! (`.superpowers/sdd/task-2-brief.md`): a FORWARD, per-node param-temp-state
//! vector, differentially proven equivalent to the backward per-path resolver
//! `resolve_temp_along_path_closed_world` (`path_temp_resolve.rs`).
//! `cross_hop`/`resolve_terminal`/`root_state` are LIVE — threaded through
//! `d1_graph`'s compact graph by the production
//! `d1_reach::search_loops_cohorts` -> `d1_dataflow::score_batch_to_sink`
//! path — instead of re-walking a `Vec<EvidenceStep>` per terminal.
//!
//! ## Why forward, not backward
//!
//! The backward resolver re-derives a terminal op's temp-ness by chasing ONE
//! path from the terminal toward the root, one hop at a time — correct, but
//! O(path length) PER TERMINAL, replayed from scratch for every terminal
//! reachable from a seed. A forward composition instead carries, per graph
//! NODE reached during the (Task 3) search, a [`TempVec`]: the resolved
//! temp-ness of every one of that node's OWN parameters GIVEN the specific
//! caller chain taken to reach it. Reaching a new node is then one
//! [`cross_hop`] call (caller's `TempVec` + this hop's binding table -> the
//! callee's `TempVec`), and resolving a terminal op is one [`resolve_terminal`]
//! lookup against the frame it lives in — no path replay.
//!
//! ## Order of checks — the one subtlety that must match EXACTLY
//!
//! `resolve_temp_along_path_closed_world` (`path_temp_resolve.rs:150-194`)
//! checks closed-world `proven(frame, i)` BEFORE it ever looks at a binding,
//! and BEFORE the edge-kind allowlist guard — a proof about `(routine, param)`
//! is a fact about the CALLEE alone, unconditionally true for every possible
//! caller, so it must win even over a non-allowlisted
//! (`dynamic`/`interface`/run-edge) hop or a missing/contradicting binding.
//! [`root_state`], [`cross_hop`], and [`resolve_terminal`] each therefore check
//! `cw.contains(&(frame, i))` FIRST:
//!   - [`root_state`] — proven params of the path ROOT are `Temp`; a
//!     non-proven root param reproduces the backward resolver's
//!     "root-PD -> Unknown" rule (`path_temp_resolve.rs:165-169`) simply by
//!     staying absent (the sparse vector's absent-key default IS `Unknown`).
//!   - [`cross_hop`] — the CALLEE's proven params are seeded `Temp` before the
//!     binding table is even consulted; `binding_ok=false` (mirrors the
//!     allowlist guard, `path_temp_resolve.rs:173-185`) then short-circuits to
//!     that proven-only baseline — exactly "all-Unknown-except-proven".
//!   - [`resolve_terminal`] — mirrors the chase's FIRST iteration, which checks
//!     `proven(frame_routine, i)` where `frame_routine` starts at the TERMINAL
//!     step's own routine (`path_temp_resolve.rs:148,159-163`) before any hop
//!     is popped; so a terminal PD index proven on its OWN owning routine
//!     resolves `Temp` even with zero callers on the path.
//!
//! The per-hop table itself (`no binding -> Unknown`, `Known(v) -> Temp/Physical`,
//! `PD(j) -> caller_state[j]`, `None/Unknown source -> Unknown`) is a direct
//! transcription of `step_one_frame` (`path_temp_resolve.rs:213-241`).
//!
//! ## Sparse-vector convention
//!
//! [`TempVec`] holds ONLY the indices some binding or op actually queries,
//! sorted by index; a missing index means `Unknown` (never stored explicitly —
//! this keeps two independently-built, semantically-equal states literally
//! `==` as plain sorted-pair vectors).
//!
//! Proven equivalent by `forward_vec_equals_backward_resolver_on_all_simple_paths`
//! (below): for every simple root->terminal path over a hand-built fixture
//! graph, the forward composition equals the backward resolver's
//! `TempStateKind` (mapped `Known(true)->Temp` / `Known(false)->Physical` /
//! `Unknown|ParameterDependent(_)->Unknown`).
#![allow(dead_code)]

use smallvec::SmallVec;

use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};

/// Concrete resolved temp-ness of one callee parameter, forward-composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ParamTemp {
    Temp,
    Physical,
    Unknown,
}

/// Sorted-by-index sparse vector; params absent => Unknown.
pub(crate) type TempVec = SmallVec<[(u32, ParamTemp); 4]>;

/// Look up `idx` in a sorted sparse [`TempVec`]; absent -> `Unknown` (the
/// sparse-vector default — see module docs). `pub(crate)` so the sparse-read
/// is defined ONCE and reused by the D2 fact solver
/// ([`crate::engine::l5::d1_dataflow`]) + the D1 liveness differential test.
pub(crate) fn lookup(v: &TempVec, idx: u32) -> ParamTemp {
    v.binary_search_by_key(&idx, |&(i, _)| i)
        .map(|pos| v[pos].1)
        .unwrap_or(ParamTemp::Unknown)
}

/// Insert/overwrite `idx -> val` in a sorted sparse [`TempVec`], keeping it
/// sorted (callers only ever insert non-`Unknown` values — see [`cross_hop`]).
fn insert_sorted(v: &mut TempVec, idx: u32, val: ParamTemp) {
    match v.binary_search_by_key(&idx, |&(i, _)| i) {
        Ok(pos) => v[pos].1 = val,
        Err(pos) => v.insert(pos, (idx, val)),
    }
}

/// Every `(routine_id, p)` pair in the closed-world proven set, as `Temp`
/// entries, sorted by index. Shared by [`root_state`] (the whole answer for a
/// path root) and [`cross_hop`] (the callee-frame baseline seeded BEFORE the
/// binding table runs — see module docs on check order).
fn proven_entries(routine_id: &str, cw: &ClosedWorldTempParams) -> TempVec {
    let mut v: TempVec = cw
        .iter()
        .filter(|(rid, _)| rid == routine_id)
        .map(|(_, i)| (*i, ParamTemp::Temp))
        .collect();
    v.sort_by_key(|&(i, _)| i);
    v
}

/// Root frame state: every param `p` is `Temp` iff closed-world proven
/// `(routine_id, p)`, else `Unknown` (absent — the root-PD rule; see module
/// docs). No caller exists above the path root, so the closed-world proof is
/// the ONLY possible source of non-`Unknown` truth for a root's own params.
pub(crate) fn root_state(routine_id: &str, cw: &ClosedWorldTempParams) -> TempVec {
    proven_entries(routine_id, cw)
}

/// Cross one hop caller -> callee, producing the callee frame's [`TempVec`].
/// `binding_ok=false` (non-allowlisted edge kind) yields all-Unknown-except-
/// proven. See module docs for the exact check order this reproduces.
pub(crate) fn cross_hop(
    caller_state: &TempVec,
    caller: &ModelRoutine,
    callsite_id: &str,
    callee_id: &str,
    binding_ok: bool,
    cw: &ClosedWorldTempParams,
) -> TempVec {
    // proven(callee, p) checked FIRST, unconditionally — a fact about the
    // callee alone that wins over both the edge-kind guard and the binding
    // table (module docs).
    let mut out = proven_entries(callee_id, cw);

    if !binding_ok {
        // Non-allowlisted hop kind: no caller-frame binding semantics at all.
        return out;
    }

    let Some(cs) = caller.call_sites.iter().find(|c| c.id == callsite_id) else {
        // Callsite missing from the caller — same Unknown fallback
        // `step_one_frame` takes for a missing callsite.
        return out;
    };

    // Every argument binding, plus the call's receiver (a table method's `Rec`).
    let bound = cs
        .argument_bindings
        .iter()
        .map(|b| (b.parameter_index, b.source_temp_state.as_ref()))
        .chain(std::iter::once((
            crate::program::body::features::RECEIVER_PARAM_INDEX,
            cs.receiver_temp_state.as_ref(),
        )));
    for (p, source_temp_state) in bound {
        if out.binary_search_by_key(&p, |&(i, _)| i).is_ok() {
            continue; // already proven Temp — the proof wins, binding ignored
        }
        let val = match source_temp_state {
            Some(ts) => match TempStateKind::from_p_temp_state(ts) {
                TempStateKind::Known(true) => ParamTemp::Temp,
                TempStateKind::Known(false) => ParamTemp::Physical,
                TempStateKind::ParameterDependent(j) => lookup(caller_state, j),
                TempStateKind::Unknown => ParamTemp::Unknown,
            },
            None => ParamTemp::Unknown,
        };
        // `Unknown` is never stored explicitly — it is the sparse vector's
        // absent-key default (see module docs), so skipping keeps two
        // semantically-equal states structurally `==`.
        if val != ParamTemp::Unknown {
            insert_sorted(&mut out, p, val);
        }
    }

    out
}

/// Terminal answer for an op given the state of its OWNING frame.
/// `owner_id` is checked for closed-world proof FIRST (mirrors the backward
/// chase's very first iteration, anchored at the terminal step's own routine
/// before any hop is popped — see module docs); only a non-proven
/// `ParameterDependent(i)` falls back to `frame_state[i]`.
pub(crate) fn resolve_terminal(
    op: &ModelRecordOperation,
    frame_state: &TempVec,
    owner_id: &str,
    cw: &ClosedWorldTempParams,
) -> ParamTemp {
    let Some(ts) = &op.temp_state else {
        return ParamTemp::Unknown;
    };
    match TempStateKind::from_p_temp_state(ts) {
        TempStateKind::Known(true) => ParamTemp::Temp,
        TempStateKind::Known(false) => ParamTemp::Physical,
        TempStateKind::Unknown => ParamTemp::Unknown,
        TempStateKind::ParameterDependent(i) => {
            if cw.contains(&(owner_id.to_string(), i)) {
                ParamTemp::Temp
            } else {
                lookup(frame_state, i)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::engine::l5::finding::{EvidenceStep, SourceAnchor};
    use crate::engine::l5::path_temp_resolve::resolve_temp_along_path_closed_world;
    use crate::program::body::features::{
        PAnchor, PCallArgumentBinding, PCallSite, PCallee, PTempState,
    };
    use crate::program::model::workspace::RoutineVariables;

    // --- fixture builders (mirrors tests/temp_state/temp_state_path.rs) -----

    fn p_anchor() -> PAnchor {
        PAnchor {
            source_unit_id: "ws:x.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: "test".to_string(),
        }
    }

    fn anchor(routine_id: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: "ws:x.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            enclosing_routine_id: routine_id.to_string(),
            syntax_kind: "test".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    /// A bare `ModelRoutine` with just an id; callers push call_sites/record_operations.
    fn routine(id: &str) -> ModelRoutine {
        ModelRoutine {
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
            source_anchor: p_anchor(),
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

    fn ts_known(value: bool) -> PTempState {
        PTempState {
            kind: "known".to_string(),
            value: Some(value),
            parameter_index: None,
        }
    }

    fn ts_pd(idx: u32) -> PTempState {
        PTempState {
            kind: "parameter-dependent".to_string(),
            value: None,
            parameter_index: Some(idx),
        }
    }

    /// A call site `id` binding callee param `parameter_index` to `source_temp_state`.
    fn call_site(
        id: &str,
        parameter_index: u32,
        source_temp_state: Option<PTempState>,
    ) -> PCallSite {
        PCallSite {
            id: id.to_string(),
            operation_id: format!("{id}/op"),
            callee_text: "Helper".to_string(),
            callee: PCallee::Bare {
                name: "Helper".to_string(),
            },
            argument_texts: vec!["arg".to_string()],
            argument_infos: Vec::new(),
            argument_bindings: vec![PCallArgumentBinding {
                parameter_index,
                source_kind: "variable".to_string(),
                source_variable_name: Some("arg".to_string()),
                source_record_variable_id: None,
                source_parameter_index: None,
                caller_source_parameter_is_var: None,
                source_temp_state,
                argument_anchor: p_anchor(),
            }],
            loop_stack: Vec::new(),
            source_anchor: p_anchor(),
            result_consumed: None,
            object_run_return_used: None,
            under_asserterror: None,
            control_context: None,
            order: None,
            in_statement_position: false,
            receiver_temp_state: None,
        }
    }

    /// A call site carrying NO argument bindings at all (the "missing binding" case).
    fn call_site_no_bindings(id: &str) -> PCallSite {
        let mut cs = call_site(id, 0, None);
        cs.argument_bindings.clear();
        cs
    }

    fn record_op(id: &str, temp_state: Option<PTempState>) -> ModelRecordOperation {
        ModelRecordOperation {
            id: id.to_string(),
            op: "Modify".to_string(),
            record_variable_name: "Rec".to_string(),
            record_variable_id: None,
            table_id: None,
            temp_state,
            field_arguments: None,
            source_anchor: p_anchor(),
            loop_stack: Vec::new(),
            field_argument_infos: None,
            in_until_condition: false,
            run_trigger: None,
        }
    }

    /// A HOP step: `routine_id` = parent (caller), `callsite_id` = the call
    /// site in that parent invoking the next-deeper routine.
    fn hop(parent_routine_id: &str, callsite_id: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: parent_routine_id.to_string(),
            operation_id: None,
            callsite_id: Some(callsite_id.to_string()),
            loop_id: None,
            source_anchor: anchor(parent_routine_id),
            note: "calls Helper".to_string(),
        }
    }

    /// The TERMINAL step (last in the path): the op routine, no callsite.
    fn terminal_step(routine_id: &str, op_id: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: routine_id.to_string(),
            operation_id: Some(op_id.to_string()),
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(routine_id),
            note: "Modify on Rec".to_string(),
        }
    }

    /// `TempStateKind` -> `ParamTemp`, per the task's equality contract:
    /// `Known(true)->Temp`, `Known(false)->Physical`, `Unknown|PD(_)->Unknown`.
    /// (The backward resolver never actually RETURNS a `ParameterDependent` —
    /// its loop only returns on `Known`/`Unknown` — the PD arm is included only
    /// so this mapping function is total.)
    fn classify(ts: TempStateKind) -> ParamTemp {
        match ts {
            TempStateKind::Known(true) => ParamTemp::Temp,
            TempStateKind::Known(false) => ParamTemp::Physical,
            TempStateKind::Unknown | TempStateKind::ParameterDependent(_) => ParamTemp::Unknown,
        }
    }

    /// `direct | method | implicit-trigger` — the same binding-carrying
    /// allowlist `D1Edge.binding_ok` (`d1_graph.rs`) and
    /// `resolve_temp_along_path_closed_world`'s guard both apply.
    fn binding_ok_for_kind(kind: &str) -> bool {
        matches!(kind, "direct" | "method" | "implicit-trigger")
    }

    /// One directed edge in a hand-built fixture graph: `from` calls `to` via
    /// `callsite_id` (owned by `from`), with the given resolved edge `kind`.
    struct FixtureEdge {
        from: &'static str,
        callsite_id: &'static str,
        to: &'static str,
        kind: &'static str,
    }

    /// DFS-enumerate every simple path from `node` to `terminal_owner` over
    /// `adj`. A node equal to `terminal_owner` ends the walk (our fixtures
    /// query exactly one op per terminal-owning routine, so paths never need
    /// to continue past it).
    fn enumerate_paths<'a>(
        root: &'a str,
        terminal_owner: &'a str,
        adj: &HashMap<&'a str, Vec<&'a FixtureEdge>>,
    ) -> Vec<Vec<&'a FixtureEdge>> {
        fn walk<'a>(
            node: &'a str,
            terminal_owner: &'a str,
            adj: &HashMap<&'a str, Vec<&'a FixtureEdge>>,
            current: &mut Vec<&'a FixtureEdge>,
            out: &mut Vec<Vec<&'a FixtureEdge>>,
        ) {
            if node == terminal_owner {
                out.push(current.clone());
                return;
            }
            if let Some(edges) = adj.get(node) {
                for e in edges {
                    current.push(e);
                    walk(e.to, terminal_owner, adj, current, out);
                    current.pop();
                }
            }
        }
        let mut out = Vec::new();
        let mut current = Vec::new();
        walk(root, terminal_owner, adj, &mut current, &mut out);
        out
    }

    /// Build `from -> [edges]` adjacency + the set of root ids (ids that never
    /// appear as an edge target) for [`enumerate_paths`] to fan out from.
    fn adjacency_and_roots<'a>(
        routines: &'a [ModelRoutine],
        edges: &'a [FixtureEdge],
    ) -> (HashMap<&'a str, Vec<&'a FixtureEdge>>, Vec<&'a str>) {
        let mut adj: HashMap<&str, Vec<&FixtureEdge>> = HashMap::new();
        for e in edges {
            adj.entry(e.from).or_default().push(e);
        }
        let targets: HashSet<&str> = edges.iter().map(|e| e.to).collect();
        let roots: Vec<&str> = routines
            .iter()
            .map(|r| r.id.as_str())
            .filter(|id| !targets.contains(id))
            .collect();
        (adj, roots)
    }

    /// For every simple root->terminal path over `routines`/`edges`, assert
    /// the forward composition (`root_state` -> `cross_hop`* ->
    /// `resolve_terminal`) equals the backward oracle
    /// (`resolve_temp_along_path_closed_world`), classified via [`classify`].
    /// Panics if the fixture yields zero paths (a silently-empty fixture would
    /// otherwise pass vacuously).
    fn assert_all_paths_agree(
        routines: &[ModelRoutine],
        edges: &[FixtureEdge],
        terminal_owner: &str,
        op_id: &str,
        cw: &ClosedWorldTempParams,
    ) {
        let routine_by_id: HashMap<&str, &ModelRoutine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let edge_kind_by_callsite: HashMap<&str, &str> =
            edges.iter().map(|e| (e.callsite_id, e.kind)).collect();
        let (adj, roots) = adjacency_and_roots(routines, edges);

        let op = routine_by_id[terminal_owner]
            .record_operations
            .iter()
            .find(|o| o.id == op_id)
            .unwrap_or_else(|| panic!("fixture must declare op {op_id} on {terminal_owner}"));
        let terminal_ts = match &op.temp_state {
            Some(ts) => TempStateKind::from_p_temp_state(ts),
            None => TempStateKind::Unknown,
        };

        let mut checked = 0usize;
        for root in roots {
            for chain in enumerate_paths(root, terminal_owner, &adj) {
                checked += 1;

                // --- backward oracle ---
                let mut steps: Vec<EvidenceStep> =
                    chain.iter().map(|e| hop(e.from, e.callsite_id)).collect();
                steps.push(terminal_step(terminal_owner, op_id));
                let backward = resolve_temp_along_path_closed_world(
                    &steps,
                    terminal_ts.clone(),
                    &routine_by_id,
                    &edge_kind_by_callsite,
                    cw,
                );
                let backward_pt = classify(backward);

                // --- forward composition ---
                let mut state = root_state(root, cw);
                for e in &chain {
                    state = cross_hop(
                        &state,
                        routine_by_id[e.from],
                        e.callsite_id,
                        e.to,
                        binding_ok_for_kind(e.kind),
                        cw,
                    );
                }
                let forward_pt = resolve_terminal(op, &state, terminal_owner, cw);

                assert_eq!(
                    forward_pt,
                    backward_pt,
                    "path {:?} into {terminal_owner}/{op_id}: forward {:?} != backward {:?}",
                    chain
                        .iter()
                        .map(|e| (e.from, e.callsite_id, e.to))
                        .collect::<Vec<_>>(),
                    forward_pt,
                    backward_pt,
                );
            }
        }
        assert!(
            checked > 0,
            "fixture for {terminal_owner}/{op_id} must exercise at least one root->terminal path"
        );
    }

    /// The load-bearing differential oracle: for every simple path root ->
    /// terminal in each fixture graph below, the forward composition must
    /// equal `TempVerdict::classify` of `resolve_temp_along_path_closed_world`
    /// over the equivalent `EvidenceStep` path. One block per required case:
    /// Known(true)/Known(false) (a single mixed-caller fixture), PD-chain-to-
    /// root, PD-chain-to-Known, missing binding, non-allowlisted hop
    /// mid-chain, closed-world proven at the terminal frame, proven at a mid
    /// frame, and an op with `None` temp_state.
    #[test]
    fn forward_vec_equals_backward_resolver_on_all_simple_paths() {
        let empty_cw = ClosedWorldTempParams::new();

        // --- Known(true) / Known(false): two callers into the same terminal,
        // same op, different per-path answers. ---
        {
            let mut a = routine("MC_A");
            a.call_sites
                .push(call_site("MC_A/cs0", 0, Some(ts_known(true))));
            let mut b = routine("MC_B");
            b.call_sites
                .push(call_site("MC_B/cs0", 0, Some(ts_known(false))));
            let mut h = routine("MC_H");
            h.record_operations
                .push(record_op("MC_H/op0", Some(ts_pd(0))));
            let routines = vec![a, b, h];
            let edges = vec![
                FixtureEdge {
                    from: "MC_A",
                    callsite_id: "MC_A/cs0",
                    to: "MC_H",
                    kind: "direct",
                },
                FixtureEdge {
                    from: "MC_B",
                    callsite_id: "MC_B/cs0",
                    to: "MC_H",
                    kind: "direct",
                },
            ];
            assert_all_paths_agree(&routines, &edges, "MC_H", "MC_H/op0", &empty_cw);
        }

        // --- PD chain to root: the root itself forwards its OWN by-var param
        // (still-PD at the root, no caller above it) -> Unknown. ---
        {
            let mut r = routine("PDR_R");
            r.call_sites.push(call_site("PDR_R/cs0", 0, Some(ts_pd(2))));
            let mut h = routine("PDR_H");
            h.record_operations
                .push(record_op("PDR_H/op0", Some(ts_pd(0))));
            let routines = vec![r, h];
            let edges = vec![FixtureEdge {
                from: "PDR_R",
                callsite_id: "PDR_R/cs0",
                to: "PDR_H",
                kind: "direct",
            }];
            assert_all_paths_agree(&routines, &edges, "PDR_H", "PDR_H/op0", &empty_cw);
        }

        // --- PD chain to Known: re-symbolizes UPWARD through two frames to a
        // grandcaller's concrete Known(true) binding. ---
        {
            let mut g = routine("PDK_G");
            g.call_sites
                .push(call_site("PDK_G/cs0", 1, Some(ts_known(true))));
            let mut m = routine("PDK_M");
            m.call_sites.push(call_site("PDK_M/cs0", 0, Some(ts_pd(1))));
            let mut h = routine("PDK_H");
            h.record_operations
                .push(record_op("PDK_H/op0", Some(ts_pd(0))));
            let routines = vec![g, m, h];
            let edges = vec![
                FixtureEdge {
                    from: "PDK_G",
                    callsite_id: "PDK_G/cs0",
                    to: "PDK_M",
                    kind: "direct",
                },
                FixtureEdge {
                    from: "PDK_M",
                    callsite_id: "PDK_M/cs0",
                    to: "PDK_H",
                    kind: "direct",
                },
            ];
            assert_all_paths_agree(&routines, &edges, "PDK_H", "PDK_H/op0", &empty_cw);
        }

        // --- missing binding: the hop's callsite carries no argument binding
        // at all for the queried index -> Unknown. ---
        {
            let mut a = routine("MB_A");
            a.call_sites.push(call_site_no_bindings("MB_A/cs0"));
            let mut h = routine("MB_H");
            h.record_operations
                .push(record_op("MB_H/op0", Some(ts_pd(0))));
            let routines = vec![a, h];
            let edges = vec![FixtureEdge {
                from: "MB_A",
                callsite_id: "MB_A/cs0",
                to: "MB_H",
                kind: "direct",
            }];
            assert_all_paths_agree(&routines, &edges, "MB_H", "MB_H/op0", &empty_cw);
        }

        // --- non-allowlisted hop MID-chain: the bad edge is the SECOND hop
        // consumed (G->M), not the one entering the terminal (M->H). Even
        // though G would bind Known(true) if ever reached, the chase must
        // stop at the bad hop and never get there -> Unknown. ---
        {
            let mut g = routine("BM_G");
            g.call_sites
                .push(call_site("BM_G/cs0", 1, Some(ts_known(true))));
            let mut m = routine("BM_M");
            m.call_sites.push(call_site("BM_M/cs0", 0, Some(ts_pd(1))));
            let mut h = routine("BM_H");
            h.record_operations
                .push(record_op("BM_H/op0", Some(ts_pd(0))));
            let routines = vec![g, m, h];
            let edges = vec![
                FixtureEdge {
                    from: "BM_G",
                    callsite_id: "BM_G/cs0",
                    to: "BM_M",
                    kind: "dynamic", // NOT allowlisted
                },
                FixtureEdge {
                    from: "BM_M",
                    callsite_id: "BM_M/cs0",
                    to: "BM_H",
                    kind: "direct",
                },
            ];
            assert_all_paths_agree(&routines, &edges, "BM_H", "BM_H/op0", &empty_cw);
        }

        // --- closed-world proven at the TERMINAL frame: zero callers at all,
        // but the op's own (routine, index) is proven -> Temp despite the
        // root-PD rule that would otherwise apply. ---
        {
            let mut h = routine("PVT_H");
            h.record_operations
                .push(record_op("PVT_H/op0", Some(ts_pd(0))));
            let routines = vec![h];
            let edges: Vec<FixtureEdge> = vec![];
            let mut cw = ClosedWorldTempParams::new();
            cw.insert(("PVT_H".to_string(), 0));
            assert_all_paths_agree(&routines, &edges, "PVT_H", "PVT_H/op0", &cw);
        }

        // --- closed-world proven at a MID frame: the root's own binding for
        // the forwarded param is deliberately Known(false) (would resolve
        // Physical if honored) — the closed-world proof at Mid must override
        // it unconditionally, without the root's binding ever mattering. ---
        {
            let mut root = routine("PVM_ROOT");
            root.call_sites
                .push(call_site("PVM_ROOT/cs0", 1, Some(ts_known(false))));
            let mut mid = routine("PVM_MID");
            mid.call_sites
                .push(call_site("PVM_MID/cs0", 0, Some(ts_pd(1))));
            let mut h = routine("PVM_H");
            h.record_operations
                .push(record_op("PVM_H/op0", Some(ts_pd(0))));
            let routines = vec![root, mid, h];
            let edges = vec![
                FixtureEdge {
                    from: "PVM_ROOT",
                    callsite_id: "PVM_ROOT/cs0",
                    to: "PVM_MID",
                    kind: "direct",
                },
                FixtureEdge {
                    from: "PVM_MID",
                    callsite_id: "PVM_MID/cs0",
                    to: "PVM_H",
                    kind: "direct",
                },
            ];
            let mut cw = ClosedWorldTempParams::new();
            cw.insert(("PVM_MID".to_string(), 1));
            assert_all_paths_agree(&routines, &edges, "PVM_H", "PVM_H/op0", &cw);
        }

        // --- op with None temp_state -> Unknown, no path needed. ---
        {
            let mut h = routine("NONE_H");
            h.record_operations.push(record_op("NONE_H/op0", None));
            let routines = vec![h];
            let edges: Vec<FixtureEdge> = vec![];
            assert_all_paths_agree(&routines, &edges, "NONE_H", "NONE_H/op0", &empty_cw);
        }
    }
}
