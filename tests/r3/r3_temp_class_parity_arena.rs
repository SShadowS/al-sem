//! #69 A5 Compare 1 (unreduced, per producer) — the arena half of the cone/L4
//! temp-class parity oracle (see `r3_temp_class_parity.rs` for the matched
//! graph, the supported ops and the guarded-fixture rule).
//!
//! For every routine r and every producer p that r reaches: the set of
//! `class(t)` over p's solve states at r (the seed included when r = owner(p))
//! equals the set of classes of L4's rows for p's `operation_id` at r.
//! Routines are enumerated from L4's side AND the solve's side, so a routine
//! missing from either side is a failure, not a skip.
//!
//! Reads the snapshot's `pd_solution` (spec D2): a state's producer operation
//! id is its producer fact's `witness_operation_id`, and its `t` is a
//! `TempStateKind`.
//!
//! Mutation (spec A5): skip transitions across intra-SCC edges in the solve →
//! "Compare 1: RecUi2 ∅ != L4 {nt}". RecUi2 reaches the producer in RecUi1
//! only through the intra-SCC edge.

use std::collections::{BTreeMap, BTreeSet};

use al_sem::engine::l5::snapshot::{SnapCapabilityExtra, SnapTempState, compose_snapshot};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::program::model::workspace::Model;

use crate::r3_temp_class_parity::{
    GUARDED, class_of, corpus_fixtures, display_by_id, l4_verb, matched_l4,
};
use crate::temp_state_probe;

/// routine stable id -> producer operation id -> classes.
type Classes = BTreeMap<String, BTreeMap<String, BTreeSet<&'static str>>>;

/// Stated limit (spec A5): producers whose op is not insert/modify/delete
/// (`Get` reads) are real solve producers, but L4 has no write row for them, so
/// neither compare looks at them. `skipped` counts those states by verb, so the
/// excluded population is visible and the filter cannot start swallowing
/// writes unnoticed.
fn compare_producers(
    label: &str,
    resolved: &Model,
    guarded: &[&str],
    f: &mut Vec<String>,
    skipped: &mut BTreeMap<String, usize>,
) {
    let snap = compose_snapshot(resolved);
    let arena = &snap.pd_solution;
    let names = display_by_id(resolved);

    let mut solve: Classes = BTreeMap::new();
    let mut producers: BTreeSet<String> = BTreeSet::new();
    for (routine, ixs) in &arena.by_routine {
        for &ix in ixs {
            let state = &arena.states[ix];
            // Supported ops on BOTH sides: a producer can be a read (`Get`).
            let fact = &snap.capability_facts[state.producer_fact];
            let verb = fact.op.as_str();
            if !matches!(verb, "insert" | "modify" | "delete") {
                *skipped.entry(format!("{label}:{verb}")).or_default() += 1;
                continue;
            }
            let op = fact
                .witness_operation_id
                .clone()
                .expect("a producer fact has its operation id");
            let t = &state.temp_state;
            producers.insert(op.clone());
            solve
                .entry(routine.clone())
                .or_default()
                .entry(op)
                .or_default()
                .insert(class_of(t));
        }
    }

    // The producer set comes from the snapshot's direct facts, not from the
    // solve: a write producer the solve dropped entirely has no states and no
    // L4 rows read, and would otherwise compare equal (both empty).
    for fact in &snap.capability_facts {
        if fact.provenance == "direct"
            && matches!(fact.op.as_str(), "insert" | "modify" | "delete")
            && matches!(
                &fact.extra,
                Some(SnapCapabilityExtra::Table {
                    temp_state: Some(SnapTempState::ParameterDependent { .. }),
                    ..
                })
            )
            && let Some(op) = &fact.witness_operation_id
            && !producers.contains(op)
        {
            f.push(format!(
                "[{label}] producer {op} ({} in {}) has no solve state",
                fact.op, fact.subject
            ));
        }
    }

    let (bundle, _dropped) = matched_l4(resolved);
    let mut l4: Classes = BTreeMap::new();
    for r in &resolved.workspace.routines {
        let Some(ix) = bundle.routine_ix(&r.id) else {
            continue;
        };
        for row in bundle.db_effects(ix) {
            if l4_verb(row.op).is_none() || !producers.contains(row.operation_id) {
                continue;
            }
            l4.entry(r.stable_routine_id.clone())
                .or_default()
                .entry(row.operation_id.to_string())
                .or_default()
                .insert(class_of(row.temp_state));
        }
    }

    let routines: BTreeSet<&String> = solve.keys().chain(l4.keys()).collect();
    for rid in routines {
        let name = names.get(rid).map(String::as_str).unwrap_or(rid);
        let empty = BTreeMap::new();
        let s = solve.get(rid).unwrap_or(&empty);
        let l = l4.get(rid).unwrap_or(&empty);
        let ops: BTreeSet<&String> = s.keys().chain(l.keys()).collect();
        for op in ops {
            let none = BTreeSet::new();
            let ss = s.get(op).unwrap_or(&none);
            let ls = l.get(op).unwrap_or(&none);
            let ok = if guarded.contains(&name) {
                ss.is_subset(ls)
            } else {
                ss == ls
            };
            if !ok {
                let fmt = |c: &BTreeSet<&str>| {
                    if c.is_empty() {
                        "∅".to_string()
                    } else {
                        format!("{{{}}}", c.iter().copied().collect::<Vec<_>>().join(","))
                    }
                };
                f.push(format!(
                    "[{label}] Compare 1: {name} {} != L4 {} (op {op})",
                    fmt(ss),
                    fmt(ls)
                ));
            }
        }
    }
}

/// A5 Compare 1 (unreduced, per producer), on every `tests/r0-corpus` fixture
/// and the probe.
#[test]
fn a5_cone_l4_class_parity_per_producer() {
    let mut f = Vec::new();
    let mut skipped = BTreeMap::new();
    let tmp = temp_state_probe::write_workspace();
    let probe = assemble_and_resolve_workspace_with_program_calls(tmp.path()).expect("probe");
    compare_producers("probe", &probe, GUARDED, &mut f, &mut skipped);
    for dir in corpus_fixtures() {
        let label = dir.file_name().unwrap().to_string_lossy().to_string();
        if let Some(m) = assemble_and_resolve_workspace_with_program_calls(&dir) {
            compare_producers(&label, &m, &[], &mut f, &mut skipped);
        }
    }
    assert!(f.is_empty(), "{} mismatches:\n{}", f.len(), f.join("\n"));
    // The excluded read producers are known to exist in these fixtures; if the
    // count is zero the verb filter is no longer reaching them. ws-d21 left the
    // list in #100: its only one was a `TestField`, which is no longer a read.
    for ws in ["ws-d40", "ws-d42", "ws-path-facts"] {
        assert!(
            skipped.keys().any(|k| k.starts_with(&format!("{ws}:"))),
            "no read producer skipped in {ws}: {skipped:?}"
        );
    }
}

/// #69 D2: every arena link `(callee state, edge)` of a state names a snapshot
/// typed edge FROM the state's routine TO the callee state's routine, and every
/// non-seed state keeps at least one link. On the probe and every
/// `tests/r0-corpus` fixture. Limit: the edge's call site is pinned only
/// through the links staying distinct; the arena carries no cone edge to
/// compare it with.
#[test]
fn pd_arena_links_are_snapshot_edges() {
    let tmp = temp_state_probe::write_workspace();
    let probe = assemble_and_resolve_workspace_with_program_calls(tmp.path()).expect("probe");
    let links = check_links("probe", &probe);
    assert!(links > 0, "the probe solve has links");
    for dir in corpus_fixtures() {
        let label = dir.file_name().unwrap().to_string_lossy().to_string();
        if let Some(m) = assemble_and_resolve_workspace_with_program_calls(&dir) {
            check_links(&label, &m);
        }
    }
}

/// The link checks of [`pd_arena_links_are_snapshot_edges`]; returns the
/// arena's link count.
fn check_links(label: &str, resolved: &Model) -> usize {
    let snap = compose_snapshot(resolved);
    let arena = &snap.pd_solution;
    for (ix, state) in arena.states.iter().enumerate() {
        assert!(
            state.seed || !state.links.is_empty(),
            "[{label}] state {ix} at {} has no link",
            state.routine
        );
        for &(callee, e) in &arena.links[state.links.clone()] {
            let edge = &snap.typed_edges[e];
            let (from, to) = (edge.edge_from(), edge.edge_to());
            let want = (
                state.routine.as_str(),
                Some(arena.states[callee].routine.as_str()),
            );
            assert_eq!(
                (from, to),
                want,
                "[{label}] state {ix}: link edge {e} is not its hop"
            );
        }
        // The solve's links are distinct (callee state, edge) pairs, so two
        // call sites into one callee must stay two snapshot edges.
        let links = &arena.links[state.links.clone()];
        let distinct: BTreeSet<&(usize, usize)> = links.iter().collect();
        assert_eq!(
            distinct.len(),
            links.len(),
            "[{label}] state {ix}: links collapsed"
        );
    }
    arena.links.len()
}
