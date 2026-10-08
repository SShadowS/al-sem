//! G-19 (docs/engine-gaps.md) — closed-world temp inference for `local` routines.
//!
//! A keyword-less `var Record X` parameter carries `temp_state =
//! ParameterDependent(i)`: its temporariness is the CALLER's choice, so an
//! intra-callee finding (no caller frame on the path) collapses to `Unknown`
//! and fires. That is OPEN-WORLD CORRECT for a `public`/`internal` routine —
//! some other (or future) caller could pass a physical record. But there is a
//! provably-sound CLOSED-WORLD subset, and this module computes exactly it:
//!
//! `(routine R, param i)` is proven `Known(true)` temporary iff ALL of:
//!   1. R is a `local` **procedure** (AL: callable ONLY within its owning
//!      object — a language rule, not an app-wide heuristic). Triggers,
//!      event subscribers and publishers (runtime-invoked with args the source
//!      cannot see) are excluded by `kind`, and entry points by id.
//!   2. The closed world is COMPLETE: no same-object call site that could name
//!      R is unresolved. Concretely, every call site in every routine of R's
//!      object whose callee name (bare or member-method, quotes stripped,
//!      case-insensitive) matches R's name — or whose callee shape is
//!      `unknown` — must appear on a resolved combined-graph edge; and no
//!      same-object routine is `parse_incomplete` (a broken body could hide a
//!      call). Any gap → no proof.
//!   3. R has at least ONE resolved caller (a dead `local` routine is NOT
//!      vacuously proven — suppression-direction discipline: keep firing).
//!   4. EVERY resolved caller edge into R is a binding-carrying kind
//!      (`direct` | `method` — the same positive allowlist as
//!      `summary_runner::substitute_pd_temp_state`), and its caller's argument
//!      binding for param `i` has `source_temp_state` `Known(true)` — or
//!      `ParameterDependent(j)` where `(caller, j)` is ITSELF closed-world
//!      proven (recursive; a cycle grounds to NOT-proven).
//!   5. Neither R's id nor any caller's id is DUPLICATED in the workspace (the
//!      RE-11 same-name-trigger id collision conflates sibling bodies' edges —
//!      caller attribution would be unreliable).
//!
//! EVERY uncertainty — non-`local`, unresolved or unclassifiable same-object
//! call, non-allowlisted edge kind, missing caller/callsite/binding, physical
//! or unknown argument, recursion cycle, id collision — fails the proof, which
//! leaves the param `ParameterDependent`/`Unknown` (the FIRING direction).
//! `Known(false)` is deliberately NOT inferred: only the suppression-useful
//! `Known(true)` requires a proof; everything else already fires.

use std::collections::{BTreeSet, HashMap, HashSet};

use al_syntax::IdentifierFoldExt;

use crate::engine::l4::combined_graph::CombinedGraph;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l5::reverse_call_graph::{ReverseCallGraph, callers_of};
use crate::program::body::features::{PCallee, PTempState};
use crate::program::model::workspace::ModelRoutine;

/// The proven set: `(internal RoutineId, parameter index)` pairs whose
/// keyword-less by-var record param is closed-world proven `Known(true)`.
pub type ClosedWorldTempParams = HashSet<(String, u32)>;

/// `true` iff `ts` is `ParameterDependent(i)` AND `(routine_id, i)` is in the
/// closed-world proven set — i.e. the op/variable may be treated EXACTLY like a
/// `Known(true)` temporary. Any other state (including `None`) → `false`.
pub fn pd_state_proven_temp(
    ts: Option<&PTempState>,
    routine_id: &str,
    proven: &ClosedWorldTempParams,
) -> bool {
    let Some(ts) = ts else {
        return false;
    };
    if ts.kind != "parameter-dependent" {
        return false;
    }
    let Some(i) = ts.parameter_index else {
        return false;
    };
    proven.contains(&(routine_id.to_string(), i))
}

fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
}

fn name_matches(callee_name: &str, routine_name: &str) -> bool {
    strip_quotes(callee_name).eq_fold_identifier(strip_quotes(routine_name))
}

struct ProofEnv<'a> {
    routine_by_id: HashMap<&'a str, &'a ModelRoutine>,
    /// Internal routine ids that occur MORE THAN ONCE in the workspace (the
    /// RE-11 trigger-id collision) — proof-disqualifying on either side.
    dup_ids: HashSet<&'a str>,
    /// Callsite ids that appear on ANY resolved combined-graph edge.
    resolved_callsites: HashSet<&'a str>,
    routines_by_object: HashMap<&'a str, Vec<&'a ModelRoutine>>,
    reverse: &'a ReverseCallGraph,
    entry_points: &'a BTreeSet<String>,
}

/// Compute the full closed-world proven set for a workspace (see module docs).
/// Pure + deterministic: a set lookup table built once in the detector context.
pub fn prove_closed_world_temp_params(
    routines: &[ModelRoutine],
    graph: &CombinedGraph,
    reverse: &ReverseCallGraph,
    entry_points: &BTreeSet<String>,
) -> ClosedWorldTempParams {
    let mut routine_by_id: HashMap<&str, &ModelRoutine> = HashMap::new();
    let mut dup_ids: HashSet<&str> = HashSet::new();
    for r in routines {
        if routine_by_id.insert(r.id.as_str(), r).is_some() {
            dup_ids.insert(r.id.as_str());
        }
    }

    let mut resolved_callsites: HashSet<&str> = HashSet::new();
    for edges in graph.edges_by_from.values() {
        for e in edges {
            if let Some(cs) = e.callsite_id.as_deref() {
                resolved_callsites.insert(cs);
            }
        }
    }

    let mut routines_by_object: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
    for r in routines {
        routines_by_object
            .entry(r.object_id.as_str())
            .or_default()
            .push(r);
    }

    // Candidate queries: every PD param record variable on a `local` procedure.
    let mut queries: Vec<(String, u32)> = Vec::new();
    for r in routines {
        if r.kind != "procedure" || r.access_modifier.as_deref() != Some("local") {
            continue;
        }
        for rv in &r.record_variables {
            if rv.is_parameter
                && rv.temp_state.kind == "parameter-dependent"
                && let Some(i) = rv.temp_state.parameter_index
            {
                queries.push((r.id.clone(), i));
            }
        }
    }

    let env = ProofEnv {
        routine_by_id,
        dup_ids,
        resolved_callsites,
        routines_by_object,
        reverse,
        entry_points,
    };
    let mut memo: HashMap<(String, u32), bool> = HashMap::new();
    let mut proven: ClosedWorldTempParams = HashSet::new();
    for q in queries {
        let mut visiting: HashSet<(String, u32)> = HashSet::new();
        if prove(&env, &q.0, q.1, &mut memo, &mut visiting) {
            proven.insert(q);
        }
    }
    proven
}

/// Memoized + cycle-grounded driver around [`prove_inner`]. A recursion cycle
/// (`A` forwards to `B` forwards back to `A`) has no temp ground truth → NOT
/// proven (fires). Memoizing the conservative `false` is sound: it can only
/// ever under-prove, never suppress.
fn prove(
    env: &ProofEnv,
    rid: &str,
    i: u32,
    memo: &mut HashMap<(String, u32), bool>,
    visiting: &mut HashSet<(String, u32)>,
) -> bool {
    let key = (rid.to_string(), i);
    if let Some(&v) = memo.get(&key) {
        return v;
    }
    if visiting.contains(&key) {
        return false; // cycle — ungrounded, not proven
    }
    visiting.insert(key.clone());
    let ok = prove_inner(env, rid, i, memo, visiting);
    visiting.remove(&key);
    memo.insert(key, ok);
    ok
}

fn prove_inner(
    env: &ProofEnv,
    rid: &str,
    i: u32,
    memo: &mut HashMap<(String, u32), bool>,
    visiting: &mut HashSet<(String, u32)>,
) -> bool {
    // (5) id collision — caller attribution unreliable.
    if env.dup_ids.contains(rid) {
        return false;
    }
    let Some(&r) = env.routine_by_id.get(rid) else {
        return false;
    };
    // (1) `local` PROCEDURE only. Triggers / event subscribers / publishers are
    // runtime-invoked; entry points doubly excluded by id.
    if r.kind != "procedure" {
        return false;
    }
    if r.access_modifier.as_deref() != Some("local") {
        return false;
    }
    if env.entry_points.contains(rid) {
        return false;
    }
    // The param must exist (sanity — a stale PD index proves nothing).
    if !r.parameters.iter().any(|p| p.index == i) {
        return false;
    }

    // (2) Closed-world completeness: `local` ⇒ callable only within the owning
    // object. Every same-object call site that could name R must be resolved.
    let Some(siblings) = env.routines_by_object.get(r.object_id.as_str()) else {
        return false;
    };
    for q in siblings {
        if q.parse_incomplete {
            return false; // a broken sibling body could hide a call to R
        }
        for cs in &q.call_sites {
            if env.resolved_callsites.contains(cs.id.as_str()) {
                continue; // resolved — if it targets R it shows up as an edge
            }
            match &cs.callee {
                PCallee::Bare { name } if name_matches(name, &r.name) => return false,
                PCallee::Member { method, .. } if name_matches(method, &r.name) => return false,
                // An unclassifiable callee could be anything — incl. R.
                PCallee::Unknown => return false,
                _ => {}
            }
        }
    }

    // (3) + (4) Every resolved caller edge proves its argument temp.
    let edges = callers_of(env.reverse, rid);
    if edges.is_empty() {
        return false; // dead local routine — refuse the vacuous proof
    }
    for e in edges {
        // Positive edge-kind allowlist (mirrors substitute_pd_temp_state's
        // binding-carrying kinds for procedure calls).
        if e.kind != "direct" && e.kind != "method" {
            return false;
        }
        if env.dup_ids.contains(e.from.as_str()) {
            return false; // colliding caller id — callsite attribution unreliable
        }
        let Some(cs_id) = e.callsite_id.as_deref() else {
            return false;
        };
        let Some(&caller) = env.routine_by_id.get(e.from.as_str()) else {
            return false;
        };
        if caller.parse_incomplete {
            return false;
        }
        let Some(cs) = caller.call_sites.iter().find(|c| c.id == cs_id) else {
            return false;
        };
        let Some(binding) = cs.argument_bindings.iter().find(|b| b.parameter_index == i) else {
            return false;
        };
        let Some(src) = &binding.source_temp_state else {
            return false;
        };
        match TempStateKind::from_p_temp_state(src) {
            TempStateKind::Known(true) => {}
            // The caller forwards its OWN keyword-less by-var param — proven
            // only if the caller's param is itself closed-world proven.
            TempStateKind::ParameterDependent(j) => {
                if !prove(env, e.from.as_str(), j, memo, visiting) {
                    return false;
                }
            }
            _ => return false, // Known(false) / Unknown — no proof
        }
    }
    true
}
