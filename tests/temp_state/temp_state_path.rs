//! Task 9 (temp-state-tracking, Component 3 / RV-6): the SHARED per-PATH temp
//! resolver `resolve_temp_along_path`.
//!
//! A path-walker terminal db-op may carry `temp_state = ParameterDependent(i)` —
//! its temporariness depends on parameter `i` of the routine it lives in. That
//! symbolic index is only resolvable along a CONCRETE caller chain, so the SAME op
//! reached from two different callers can resolve differently (per-finding truth):
//!   - caller A passes a TEMP local  → `Known(true)`  (would be suppressed by d1);
//!   - caller B passes a PHYSICAL var → `Known(false)` (would fire).
//!
//! These tests drive the resolver against HAND-BUILT `EvidenceStep` paths +
//! `ModelRoutine` structures rather than the full L5 pipeline. RATIONALE: the net-new
//! logic in this task is the per-PATH FRAME-STEPPING (walking hop callsites toward
//! the root and applying the substitution table at each frame). The end-to-end L4
//! substitution semantics are already locked by `tests/temp_state_substitution.rs`;
//! hand-built paths exercise the stepping directly, with full control over the
//! caller-chain shape (mixed callers, root PD, direct-Known) that a real-pipeline
//! WalkResult would otherwise make awkward to obtain in isolation. Path orientation
//! used here is ROOT→TERMINAL (verified against `path_walker::visit`): hop steps are
//! pushed while descending, terminal step is last; a hop's `routine_id` is the
//! PARENT (caller) and its `callsite_id` is the call site in that parent.

use std::collections::HashMap;

use al_sem::engine::l4::effect_lattice::TempStateKind;
use al_sem::engine::l5::finding::{EvidenceStep, SourceAnchor};
use al_sem::engine::l5::path_temp_resolve::resolve_temp_along_path;
use al_sem::program::body::features::{
    PAnchor, PCallArgumentBinding, PCallSite, PCallee, PTempState,
};
use al_sem::program::model::workspace::{ModelRoutine, RoutineVariables};

// --- builders ---------------------------------------------------------------

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

/// A bare `ModelRoutine` with just an id; callers push call_sites onto it.
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

/// A call site `id` whose single argument binds callee param `parameter_index`
/// with the given `source_temp_state`.
fn call_site(id: &str, parameter_index: u32, source_temp_state: Option<PTempState>) -> PCallSite {
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

fn ts_known(value: bool) -> PTempState {
    PTempState {
        kind: "known".to_string(),
        value: Some(value),
        parameter_index: None,
    }
}

/// A HOP step: `routine_id` = parent (caller), `callsite_id` = the call site in
/// that parent invoking the next-deeper routine.
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
fn terminal(routine_id: &str, op_id: &str) -> EvidenceStep {
    EvidenceStep {
        routine_id: routine_id.to_string(),
        operation_id: Some(op_id.to_string()),
        callsite_id: None,
        loop_id: None,
        source_anchor: anchor(routine_id),
        note: "Modify on Rec".to_string(),
    }
}

fn routine_map(routines: &[ModelRoutine]) -> HashMap<&str, &ModelRoutine> {
    routines.iter().map(|r| (r.id.as_str(), r)).collect()
}

/// An edge-kind lookup mapping every callsite in `routines` to a binding-carrying
/// `"direct"` edge — the common case for these stepping tests. Case (c) builds its
/// own map with a non-allowlisted kind to exercise the guard.
fn direct_edge_kinds(routines: &[ModelRoutine]) -> HashMap<&str, &str> {
    routines
        .iter()
        .flat_map(|r| r.call_sites.iter().map(|cs| (cs.id.as_str(), "direct")))
        .collect()
}

// --- (a) mixed callers per-path ---------------------------------------------

/// Helper `H(var Rec)` does `Rec.Modify()` → terminal op temp_state = PD(0).
/// Caller A passes a TEMP local (binding source Known(true)) → `Known(true)`.
/// Caller B passes a PHYSICAL local (binding source Known(false)) → `Known(false)`.
/// SAME op, SAME terminal state, DIFFERENT path → different resolution.
#[test]
fn mixed_callers_resolve_per_path() {
    // Caller A: H is entered via cs "A/cs0" passing a temporary (Known(true)).
    let mut caller_a = routine("A");
    caller_a
        .call_sites
        .push(call_site("A/cs0", 0, Some(ts_known(true))));

    // Caller B: H is entered via cs "B/cs0" passing a physical var (Known(false)).
    let mut caller_b = routine("B");
    caller_b
        .call_sites
        .push(call_site("B/cs0", 0, Some(ts_known(false))));

    let helper = routine("H");
    let routines = [caller_a, caller_b, helper];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    // Path A (root→terminal): hop(A→H), terminal(H op0).
    let path_a = vec![hop("A", "A/cs0"), terminal("H", "H/op0")];
    let path_b = vec![hop("B", "B/cs0"), terminal("H", "H/op0")];

    assert_eq!(
        resolve_temp_along_path(
            &path_a,
            TempStateKind::ParameterDependent(0),
            &map,
            &edge_kinds
        ),
        TempStateKind::Known(true),
        "caller A passes a temp local → PD(0) must resolve Known(true)"
    );
    assert_eq!(
        resolve_temp_along_path(
            &path_b,
            TempStateKind::ParameterDependent(0),
            &map,
            &edge_kinds
        ),
        TempStateKind::Known(false),
        "caller B passes a physical local → PD(0) must resolve Known(false)"
    );
}

/// PD(j) re-symbolization chains UPWARD through two frames: the terminal frame's
/// PD(0) forwards to the caller's own param (PD(1)), which the GRANDcaller binds
/// to a temp (Known(true)).
#[test]
fn pd_chains_upward_two_frames() {
    // Grandcaller G: enters M via "G/cs0", binding M's param 1 ← temp (Known(true)).
    let mut g = routine("G");
    g.call_sites
        .push(call_site("G/cs0", 1, Some(ts_known(true))));

    // Middle M: enters H via "M/cs0", forwarding its OWN by-var param 1 → PD(1).
    let mut m = routine("M");
    m.call_sites.push(call_site(
        "M/cs0",
        0,
        Some(PTempState {
            kind: "parameter-dependent".to_string(),
            value: None,
            parameter_index: Some(1),
        }),
    ));

    let h = routine("H");
    let routines = [g, m, h];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    // Path: hop(G→M), hop(M→H), terminal(H op0). Terminal op PD(0).
    let path = vec![hop("G", "G/cs0"), hop("M", "M/cs0"), terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(
            &path,
            TempStateKind::ParameterDependent(0),
            &map,
            &edge_kinds
        ),
        TempStateKind::Known(true),
        "PD(0)→PD(1)→Known(true) must chain up two frames"
    );
}

// --- (b) root PD → Unknown --------------------------------------------------

/// The terminal op is PD(i) and frame i is an ENTRY parameter (no caller hop in
/// the path) → resolves Unknown (conservative = fires).
#[test]
fn root_pd_resolves_unknown() {
    // Single-frame path: just the terminal op, no caller hop. The op's tempness
    // depends on its OWN entry param 0 with no caller → Unknown.
    let helper = routine("H");
    let routines = [helper];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    let path = vec![terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(
            &path,
            TempStateKind::ParameterDependent(0),
            &map,
            &edge_kinds
        ),
        TempStateKind::Unknown,
        "PD at the path root (entry param, no caller) → Unknown"
    );
}

/// PD that re-symbolizes to PD at the root caller (the root caller forwards its
/// OWN by-var param) → still-PD at root → Unknown.
#[test]
fn pd_resymbolized_to_root_param_is_unknown() {
    // Root caller R forwards its own by-var param 2 → PD(2). No caller above R.
    let mut r = routine("R");
    r.call_sites.push(call_site(
        "R/cs0",
        0,
        Some(PTempState {
            kind: "parameter-dependent".to_string(),
            value: None,
            parameter_index: Some(2),
        }),
    ));
    let h = routine("H");
    let routines = [r, h];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    let path = vec![hop("R", "R/cs0"), terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(
            &path,
            TempStateKind::ParameterDependent(0),
            &map,
            &edge_kinds
        ),
        TempStateKind::Unknown,
        "PD(0)→PD(2) with no caller above the root → Unknown"
    );
}

// --- (c) direct Known -------------------------------------------------------

/// A terminal op already `Known(true)` (e.g. a temp LOCAL var, not param-dependent)
/// resolves `Known(true)` with NO stepping (no caller hops consumed).
#[test]
fn direct_known_true_no_stepping() {
    let helper = routine("H");
    let routines = [helper];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    // Even with a caller hop present, a Known terminal short-circuits immediately.
    let mut caller = routine("A");
    caller
        .call_sites
        .push(call_site("A/cs0", 0, Some(ts_known(false))));
    let routines2 = [caller, routine("H")];
    let map2 = routine_map(&routines2);
    let edge_kinds2 = direct_edge_kinds(&routines2);

    let path_no_hop = vec![terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(&path_no_hop, TempStateKind::Known(true), &map, &edge_kinds),
        TempStateKind::Known(true),
        "a Known(true) terminal resolves Known(true) with no stepping"
    );

    let path_with_hop = vec![hop("A", "A/cs0"), terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(
            &path_with_hop,
            TempStateKind::Known(true),
            &map2,
            &edge_kinds2
        ),
        TempStateKind::Known(true),
        "Known(true) short-circuits even when caller hops exist (binding ignored)"
    );
}

// --- soundness: every uncertainty → Unknown ---------------------------------

/// Missing binding for the chased param index, `Some(Unknown)` source, and a
/// missing parent routine all collapse to Unknown.
#[test]
fn uncertainty_sources_resolve_unknown() {
    // (i) callsite exists but binds a DIFFERENT param index (no binding for 0).
    let mut caller_wrong = routine("A");
    caller_wrong
        .call_sites
        .push(call_site("A/cs0", 5, Some(ts_known(true))));
    // (ii) binding source is Some(Unknown).
    let mut caller_unknown = routine("B");
    caller_unknown.call_sites.push(call_site(
        "B/cs0",
        0,
        Some(PTempState {
            kind: "unknown".to_string(),
            value: None,
            parameter_index: None,
        }),
    ));
    // (iii) binding source is None.
    let mut caller_none = routine("C");
    caller_none.call_sites.push(call_site("C/cs0", 0, None));

    let routines = [caller_wrong, caller_unknown, caller_none, routine("H")];
    let map = routine_map(&routines);
    let edge_kinds = direct_edge_kinds(&routines);

    for parent in ["A", "B", "C"] {
        let cs = format!("{parent}/cs0");
        let path = vec![hop(parent, &cs), terminal("H", "H/op0")];
        assert_eq!(
            resolve_temp_along_path(
                &path,
                TempStateKind::ParameterDependent(0),
                &map,
                &edge_kinds
            ),
            TempStateKind::Unknown,
            "uncertain binding source from {parent} → Unknown"
        );
    }

    // see edge-kind-guard test below for (c).

    // (iv) parent routine not in the map at all → Unknown. The callsite IS in the
    // edge-kind map as an allowlisted `direct` edge, so the guard passes and the
    // Unknown comes from the missing parent routine (not the guard).
    let empty: Vec<ModelRoutine> = Vec::new();
    let empty_map = routine_map(&empty);
    let mut missing_edge_kinds: HashMap<&str, &str> = HashMap::new();
    missing_edge_kinds.insert("MISSING/cs0", "direct");
    let path = vec![hop("MISSING", "MISSING/cs0"), terminal("H", "H/op0")];
    assert_eq!(
        resolve_temp_along_path(
            &path,
            TempStateKind::ParameterDependent(0),
            &empty_map,
            &missing_edge_kinds
        ),
        TempStateKind::Unknown,
        "missing parent routine → Unknown"
    );
}

// --- (c) edge-kind allowlist guard ------------------------------------------

/// SOUNDNESS (RV-6 / Task 10): a PD chased down a NON-allowlisted hop
/// (`dynamic` / `interface` / a run-edge) must resolve `Unknown` — NOT `Known(true)`
/// — EVEN when the hop's binding source is concretely `Known(true)`. Such edges
/// carry no caller-frame binding semantics (L4's `substitute_pd_temp_state` only
/// substitutes `direct | method | implicit-trigger`); resolving `Known(true)` here
/// would let d1 SUPPRESS a real finding down a dynamic-dispatch hop.
#[test]
fn edge_kind_guard_dynamic_hop_resolves_unknown() {
    // Caller A enters H via "A/cs0" passing a TEMP local (binding source Known(true))
    // — the binding that WOULD resolve Known(true) over an allowlisted edge.
    let mut caller_a = routine("A");
    caller_a
        .call_sites
        .push(call_site("A/cs0", 0, Some(ts_known(true))));
    let helper = routine("H");
    let routines = [caller_a, helper];
    let map = routine_map(&routines);

    let path = vec![hop("A", "A/cs0"), terminal("H", "H/op0")];

    // Sanity: over a `direct` edge the SAME binding resolves Known(true).
    let direct = direct_edge_kinds(&routines);
    assert_eq!(
        resolve_temp_along_path(&path, TempStateKind::ParameterDependent(0), &map, &direct),
        TempStateKind::Known(true),
        "control: a Known(true) binding over a `direct` hop resolves Known(true)"
    );

    // Each non-allowlisted kind STOPS the chase → Unknown despite Known(true) source.
    // Exhaustive registry of the non-allowlisted edge kinds (everything outside
    // {direct, method, implicit-trigger}).
    for kind in [
        "dynamic",
        "interface",
        "codeunit-run",
        "report-run",
        "page-run",
        "event-dispatch",
    ] {
        let mut edge_kinds: HashMap<&str, &str> = HashMap::new();
        edge_kinds.insert("A/cs0", kind);
        assert_eq!(
            resolve_temp_along_path(
                &path,
                TempStateKind::ParameterDependent(0),
                &map,
                &edge_kinds
            ),
            TempStateKind::Unknown,
            "PD chased down a `{kind}` hop must be Unknown (NOT suppressed), even with a Known(true) source"
        );
    }

    // A callsite ABSENT from the edge-kind map (unknown kind) also stops the chase.
    let empty_kinds: HashMap<&str, &str> = HashMap::new();
    assert_eq!(
        resolve_temp_along_path(
            &path,
            TempStateKind::ParameterDependent(0),
            &map,
            &empty_kinds
        ),
        TempStateKind::Unknown,
        "PD chased down a hop with an unknown edge kind → Unknown"
    );
}
