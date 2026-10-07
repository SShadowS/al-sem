//! Per-PATH temp-state resolution (Component 3, RV-6).
//!
//! A path-walker [`WalkResult`] terminates at a db-operation that may carry
//! `temp_state = ParameterDependent(i)` — its temporariness depends on parameter
//! `i` of the routine the op LIVES IN. That symbolic index is only resolvable in
//! the context of a CONCRETE caller chain: the SAME op reached from two different
//! callers can resolve differently (caller-A passes a temp var → `Known(true)`;
//! caller-B passes a physical var → `Known(false)`). This is *per-finding* truth,
//! and it is what [`resolve_temp_along_path`] computes.
//!
//! ## Path orientation (verified against `path_walker::visit`)
//!
//! `WalkResult.path` is in **ROOT → TERMINAL** order. The walker descends from
//! `start` into each `edge.to`, PUSHING a hop step (`build_hop_step`) as it goes,
//! and appends the terminal step LAST. So:
//!   - `path.last()` is the TERMINAL step; its `routine_id` is the routine that
//!     OWNS the terminal op (frame T).
//!   - A HOP step at index `k` has `routine_id == edge.from` (the PARENT / UPSTREAM
//!     routine, closer to the root) and `callsite_id == edge.callsite_id` (the call
//!     site IN THAT PARENT that invokes the next-deeper routine `edge.to`).
//!   - The hop that ENTERS the terminal frame T is therefore the LAST hop step in
//!     the path — the step immediately before the terminal whose `callsite_id` is
//!     `Some` and whose `routine_id` is T's caller. Stepping "toward the path root"
//!     means walking the hop steps from the END of the vec toward the FRONT.
//!
//! Detector-supplied prefix steps (d1 seeds `[loopStep, callStep]`) are part of the
//! same vec; the `loopStep` carries `callsite_id == None` and the `callStep` carries
//! the in-loop callsite that enters the FIRST walked routine. The resolver only ever
//! consumes hop steps that carry a `Some(callsite_id)`, and it stops the moment it
//! runs out of caller hops — so seed steps that lack a callsite are simply the path
//! root for resolution purposes (still-PD there → `Unknown`).
//!
//! ## Callee-param index — DERIVED, not a new serialized field (RV-6 decision)
//!
//! RV-6 asks the walker to expose, per hop, the callee-param index needed to step
//! frames. We DERIVE it at resolve time from the L3 routine map instead of adding a
//! field to a serialized walker/`EvidenceStep` struct: given a hop's `callsite_id`,
//! the parent routine's `call_sites[*].argument_bindings` already carry
//! `parameter_index` (= callee param index) and `source_temp_state` /
//! `source_parameter_index`. Deriving avoids touching any serialized struct, so NO
//! R3a/trace/R4 golden can move (lower golden impact — the explicitly preferred
//! option). The resolver receives the routine map it needs as an explicit argument.
//!
//! ## Edge-kind allowlist guard (Component 3 / RV-6 soundness, Task 10)
//!
//! L4's `substitute_pd_temp_state` only substitutes bindings across the
//! `direct | method | implicit-trigger` edge kinds; ANY other kind
//! (`dynamic | interface | codeunit-run | report-run | page-run | event-dispatch`,
//! or an unknown/missing kind) carries NO usable binding semantics and falls to
//! `Unknown`. d1's path expansion FOLLOWS `dynamic`/`interface`/run hops, and those
//! hops DO carry a `callsite_id`, so a naive resolver would chase a binding down
//! such a hop and could resolve `Known(true)` where L4 returns `Unknown` — an
//! UNSOUND divergence that would SUPPRESS a real finding. The resolver therefore
//! takes an `edge_kind_by_callsite` lookup and, before stepping ANY hop, checks the
//! hop's edge kind against the allowlist; a non-allowlisted (or unknown) kind stops
//! the chase and returns `Unknown`.
//!
//! ## Soundness
//!
//! Resolution only ever yields `Known(true)` when a concrete binding source ON THE
//! PATH is itself `Known(true)` AND every hop chased to reach it is an
//! allowlisted binding-carrying edge. EVERY uncertainty — a missing caller hop, a
//! missing callsite, a missing binding, a non-allowlisted edge kind, a
//! `Some(Unknown)` / `None` source, or a still-`PD` state at the path root —
//! collapses to `Unknown` (the conservative, FIRING direction). This mirrors the L4
//! per-callsite substitution table (`summary_runner::substitute_pd_temp_state`)
//! applied frame-by-frame.

use std::collections::HashMap;

use crate::engine::l3::l3_workspace::L3Routine;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
use crate::engine::l5::finding::EvidenceStep;

/// Resolve a terminal op's `temp_state` ALONG ONE WALK PATH to a concrete
/// `Known(_)` / `Unknown` (Component 3, RV-6).
///
/// - `path` is a [`WalkResult::path`](crate::engine::l5::path_walker::WalkResult)
///   in ROOT→TERMINAL order (see module docs).
/// - `terminal_state` is the terminal op's `temp_state` as a [`TempStateKind`]
///   (the caller maps `op.temp_state` via `TempStateKind::from_p_temp_state`, with
///   a `None` temp_state → `Unknown`).
/// - `routine_by_id` maps each routine's INTERNAL id to its `L3Routine` (so a hop's
///   `callsite_id` can be resolved against the parent routine's call sites). This is
///   the same `ctx.routine_by_id` index d1 already builds.
/// - `edge_kind_by_callsite` maps a hop's `callsite_id` to the edge KIND of the
///   resolved call edge for that callsite (derivable from the `CombinedGraph` d1
///   already holds). Used to enforce the edge-kind allowlist (see module docs):
///   only `direct | method | implicit-trigger` hops carry usable binding semantics;
///   ANY other kind — or a callsite missing from the map — stops the chase and
///   returns `Unknown` (sound = fires).
///
/// Steps one frame toward the path root per `ParameterDependent` level, applying the
/// L4 substitution table at each hop; terminates because each step consumes one more
/// caller hop and the path is finite.
///
/// Visibility: `pub` (not `pub(crate)`) SOLELY so the `tests/temp_state_path.rs`
/// integration test — a separate crate — can drive it directly per this task's TDD
/// mandate. It is otherwise an internal L5 helper; Task 10 wires d1 to it in-crate.
pub fn resolve_temp_along_path(
    path: &[EvidenceStep],
    terminal_state: TempStateKind,
    routine_by_id: &HashMap<&str, &L3Routine>,
    edge_kind_by_callsite: &HashMap<&str, &str>,
) -> TempStateKind {
    resolve_temp_along_path_closed_world(
        path,
        terminal_state,
        routine_by_id,
        edge_kind_by_callsite,
        &ClosedWorldTempParams::new(),
    )
}

/// [`resolve_temp_along_path`] PLUS the G-19 closed-world proven set: whenever
/// the chase holds `ParameterDependent(i)` anchored to a frame `F` with
/// `(F, i)` closed-world proven (a `local` routine ALL of whose resolved
/// callers prove a temp argument — see `closed_world_temp`), the state is
/// `Known(true)` for EVERY possible caller, so the per-path answer is too.
/// The check fires for any frame on the chase (terminal or upstream); a frame
/// NOT in the proven set behaves exactly as before (root-PD → `Unknown`).
///
/// SOUNDNESS: the proven set quantifies over ALL resolved callers of a closed
/// (`local`, fully-resolved-call-surface) routine, so it can never contradict a
/// concrete path: a path whose caller passes a physical record implies the
/// proof failed for that frame, and the chase proceeds per-path as before.
pub fn resolve_temp_along_path_closed_world(
    path: &[EvidenceStep],
    terminal_state: TempStateKind,
    routine_by_id: &HashMap<&str, &L3Routine>,
    edge_kind_by_callsite: &HashMap<&str, &str>,
    closed_world_temp_params: &ClosedWorldTempParams,
) -> TempStateKind {
    // The hop steps that carry a real caller callsite, in ROOT→TERMINAL order. The
    // terminal step (last, callsite_id == None) and any seed loop step (callsite_id
    // == None) are naturally excluded. We consume these from the END (the hop that
    // enters the terminal frame) toward the FRONT (the root) as we chase PD levels.
    let caller_hops: Vec<&EvidenceStep> = path.iter().filter(|s| s.callsite_id.is_some()).collect();

    let mut state = terminal_state;
    // `hop_idx` indexes the caller_hops vec; we start at the LAST hop (the one
    // entering the terminal frame) and walk backward (toward root) per PD level.
    let mut hop_idx = caller_hops.len();
    // The routine OWNING the frame the current PD state is anchored to. Starts at
    // the TERMINAL frame (the last step's routine — the op's owner); each consumed
    // hop re-anchors to that hop's parent routine (`hop.routine_id`).
    let mut frame_routine: Option<&str> = path.last().map(|s| s.routine_id.as_str());

    loop {
        let i = match &state {
            // Concrete — done.
            TempStateKind::Known(_) | TempStateKind::Unknown => return state,
            TempStateKind::ParameterDependent(i) => *i,
        };

        // G-19: PD(i) anchored to a closed-world PROVEN frame is Known(true) for
        // every possible caller — no path context needed.
        if let Some(f) = frame_routine
            && closed_world_temp_params.contains(&(f.to_string(), i))
        {
            return TempStateKind::Known(true);
        }

        if hop_idx == 0 {
            // Reached the path ROOT while still PD: the op's tempness depends on an
            // ENTRY parameter with no caller in this path. Conservative → Unknown.
            return TempStateKind::Unknown;
        }
        hop_idx -= 1;
        let hop = caller_hops[hop_idx];

        // EDGE-KIND ALLOWLIST GUARD (RV-6 soundness). The hop carries a
        // `callsite_id`; resolve its edge kind. Only `direct | method |
        // implicit-trigger` carry usable binding semantics. ANY other kind — or a
        // callsite absent from the lookup — has no caller-frame binding semantics,
        // so the chase STOPS here → Unknown (sound = fires). Mirrors L4's positive
        // allowlist in `substitute_pd_temp_state`.
        let edge_kind = hop
            .callsite_id
            .as_deref()
            .and_then(|cs| edge_kind_by_callsite.get(cs).copied());
        if !matches!(edge_kind, Some("direct" | "method" | "implicit-trigger")) {
            return TempStateKind::Unknown;
        }

        // The hop's parent routine (it OWNS the callsite). `routine_id` is the
        // caller (edge.from); `callsite_id` is the call site in that caller.
        let parent = routine_by_id.get(hop.routine_id.as_str()).copied();
        let cs_id = hop.callsite_id.as_deref();
        state = step_one_frame(parent, cs_id, i);
        // A PD result of the substitution is re-anchored to the PARENT frame.
        frame_routine = Some(hop.routine_id.as_str());
    }
}

/// Apply the L4 per-callsite substitution table for one caller frame: resolve
/// `ParameterDependent(callee_param_index)` through the parent routine's argument
/// binding for that callee param. Mirrors
/// `summary_runner::substitute_pd_temp_state`'s table, but threaded for the
/// per-PATH walk (the parent routine + callsite are derived from the hop here,
/// not from a `CombinedEdge`).
///
/// Any missing piece → `Unknown` (sound = fires):
///   - no parent routine in the map, or no callsite id on the hop;
///   - no callsite with that id in the parent;
///   - no binding whose `parameter_index == callee_param_index`;
///   - `source_temp_state` is `Some(Unknown)` or `None`.
///
/// `Some(Known(v))` → `Known(v)`; `Some(PD(j))` → `ParameterDependent(j)` (re-anchored
/// to the PARENT frame at L2 — the same UPWARD re-symbolization Task 8 does — which
/// the next loop turn then chases through the parent's own caller hop).
fn step_one_frame(
    parent: Option<&L3Routine>,
    callsite_id: Option<&str>,
    callee_param_index: u32,
) -> TempStateKind {
    let (Some(parent), Some(cs_id)) = (parent, callsite_id) else {
        return TempStateKind::Unknown;
    };
    let Some(cs) = parent.call_sites.iter().find(|c| c.id == cs_id) else {
        return TempStateKind::Unknown;
    };
    match cs.source_temp_state_for(callee_param_index) {
        Some(ts) => match TempStateKind::from_p_temp_state(ts) {
            TempStateKind::Known(v) => TempStateKind::Known(v),
            // Forwarded keyword-less by-var param: re-symbolize to the caller's own
            // param index (chains upward; chased on the next loop turn).
            TempStateKind::ParameterDependent(j) => TempStateKind::ParameterDependent(j),
            TempStateKind::Unknown => TempStateKind::Unknown,
        },
        None => TempStateKind::Unknown,
    }
}
