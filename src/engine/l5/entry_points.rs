//! Entry points / reachable roots — faithful port of al-sem
//! `src/engine/entry-points.ts`.
//!
//! `find_entry_points` — primary-app routines whose `kind` is `trigger` or
//! `event-subscriber` (the BC-dispatched root set), sorted. Used by D8
//! (transaction-span roots).
//!
//! `find_reachable_roots` — the D14 dead-routine root set: entry points PLUS
//! non-`local` procedures (public callable from any dependent app;
//! `internal` only when some app is granted access via `internalsVisibleTo`).
//!
//! ## Role / access-modifier threading (read this)
//!
//! al-sem reads `roleOf(r)` and `r.accessModifier` off the model. The Rust
//! `ModelRoutine` has NEITHER:
//!   - **Role** is `is_dep = dep_routine_ids.contains(&r.id)` (see
//!     `capability_cone.rs` dep universe). For source-only this set is EMPTY (all
//!     routines primary). We thread it as an explicit `&BTreeSet<String>` so the
//!     function is correct for both source-only (empty set ⇒ all primary) and
//!     cross-app.
//!   - **Access modifier** has NO field on `ModelRoutine` yet. al-sem's
//!     `findReachableRoots` needs it ("local"/"internal"/public) plus
//!     `internalReachableExternally` (al-sem `model.identity.
//!     primaryInternalsVisibleTo`). For Task 2a we take BOTH as EXPLICIT INPUTS:
//!     `access_modifiers: &HashMap<RoutineId, AccessModifier>` (absent ⇒ public,
//!     the al-sem default-access case) and a `internal_reachable_externally: bool`.
//!     So `find_reachable_roots` is fully correct and testable NOW; WIRING the
//!     access modifier from the model is a known follow-up for the D14 / R4-G wave
//!     (do NOT add a model field in this task — keep it additive-later).

use std::collections::{BTreeSet, HashMap};

use crate::program::model::workspace::ModelRoutine;

/// Routine access modifier (al-sem `accessModifier`). `Public` is the default
/// when the model carries no entry (al-sem default-access procedures).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModifier {
    /// `local` — app-scoped; never a reachable root.
    Local,
    /// `internal` — a root ONLY when `internal_reachable_externally` is set.
    Internal,
    /// default-access (public) or `protected` — always a reachable root.
    Public,
}

/// `roleOf(r) === "primary"` — true when the routine is NOT in the dependency
/// universe. Source-only: `dep_routine_ids` is empty ⇒ always primary.
fn is_primary(routine: &ModelRoutine, dep_routine_ids: &BTreeSet<String>) -> bool {
    !dep_routine_ids.contains(&routine.id)
}

/// Identify primary-app routines BC dispatches to without an in-app caller — the
/// root set for reachability analysis. An entry point is a primary-app routine
/// whose `kind` is `trigger` or `event-subscriber`. Sorted by internal id.
///
/// `dep_routine_ids` is the role oracle (empty ⇒ all primary). Mirrors al-sem
/// `findEntryPoints(model)`.
pub fn find_entry_points(
    routines: &[ModelRoutine],
    dep_routine_ids: &BTreeSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in routines {
        if !is_primary(r, dep_routine_ids) {
            continue;
        }
        if r.kind == "event-subscriber" || r.kind == "trigger" {
            out.push(r.id.clone());
        }
    }
    out.sort();
    out
}

/// Root set for "is this routine ever invoked?" reachability (D14). Adds to
/// `find_entry_points` the procedures we cannot prove are app-scoped: non-`local`
/// procedures, with `internal` included only when `internal_reachable_externally`
/// is true. Sorted by internal id.
///
/// `dep_routine_ids` — the role oracle (empty ⇒ all primary).
/// `access_modifiers` — internal RoutineId → its `AccessModifier`; a routine with
/// NO entry is treated as `Public` (al-sem default-access). See module docs for
/// why this is an explicit input rather than a model field.
/// `internal_reachable_externally` — al-sem
/// `model.identity.primaryInternalsVisibleTo` non-empty (some app granted
/// `internal` access).
///
/// Mirrors al-sem `findReachableRoots(model, { internalReachableExternally })`.
pub fn find_reachable_roots(
    routines: &[ModelRoutine],
    dep_routine_ids: &BTreeSet<String>,
    access_modifiers: &HashMap<String, AccessModifier>,
    internal_reachable_externally: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in routines {
        if !is_primary(r, dep_routine_ids) {
            continue;
        }
        if r.kind == "event-subscriber" || r.kind == "trigger" {
            out.push(r.id.clone());
            continue;
        }
        if r.kind != "procedure" {
            continue;
        }
        let access = access_modifiers
            .get(&r.id)
            .copied()
            .unwrap_or(AccessModifier::Public);
        match access {
            AccessModifier::Local => continue,
            AccessModifier::Internal if !internal_reachable_externally => continue,
            _ => out.push(r.id.clone()),
        }
    }
    out.sort();
    out
}

// ===========================================================================
// Native oracles — ground-truth-free invariants on synthetic inputs.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::test_support::routine;

    #[test]
    fn entry_points_are_only_primary_triggers_and_subscribers_sorted() {
        let routines = vec![
            routine("z", "trigger"),
            routine("a", "event-subscriber"),
            routine("p", "procedure"),
            routine("e", "event-publisher"),
            routine("m", "trigger"),
        ];
        let no_deps = BTreeSet::new();
        let eps = find_entry_points(&routines, &no_deps);
        // procedure + event-publisher excluded; trigger/subscriber kept; sorted.
        assert_eq!(eps, vec!["a", "m", "z"]);
    }

    #[test]
    fn dep_routines_are_never_entry_points() {
        let routines = vec![routine("a", "trigger"), routine("b", "event-subscriber")];
        let deps: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        let eps = find_entry_points(&routines, &deps);
        assert_eq!(eps, vec!["b"]); // a is a dep routine → excluded
    }

    #[test]
    fn reachable_roots_add_non_local_procedures() {
        let routines = vec![
            routine("trig", "trigger"),
            routine("pub_proc", "procedure"),
            routine("loc_proc", "procedure"),
            routine("int_proc", "procedure"),
        ];
        let no_deps = BTreeSet::new();
        let mut access = HashMap::new();
        access.insert("pub_proc".to_string(), AccessModifier::Public);
        access.insert("loc_proc".to_string(), AccessModifier::Local);
        access.insert("int_proc".to_string(), AccessModifier::Internal);

        // internalReachableExternally = false: internal collapses to local-case.
        let roots = find_reachable_roots(&routines, &no_deps, &access, false);
        assert_eq!(roots, vec!["pub_proc", "trig"]);

        // internalReachableExternally = true: internal now a root.
        let roots = find_reachable_roots(&routines, &no_deps, &access, true);
        assert_eq!(roots, vec!["int_proc", "pub_proc", "trig"]);
    }

    #[test]
    fn reachable_roots_default_access_is_public() {
        // A procedure with no access-modifier entry is treated as public.
        let routines = vec![routine("p", "procedure")];
        let no_deps = BTreeSet::new();
        let access = HashMap::new();
        let roots = find_reachable_roots(&routines, &no_deps, &access, false);
        assert_eq!(roots, vec!["p"]);
    }

    #[test]
    fn reachable_roots_exclude_dep_routines() {
        let routines = vec![routine("a", "procedure"), routine("b", "trigger")];
        let deps: BTreeSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        let access = HashMap::new();
        let roots = find_reachable_roots(&routines, &deps, &access, true);
        assert!(roots.is_empty());
    }
}
