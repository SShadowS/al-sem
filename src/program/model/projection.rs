//! #160 step 11b (spec §4.3, §4.5, §4.6): the projection that turns per-row
//! producer outputs into the per-id inputs the cone, the solve, the event graph
//! and the lookups take.
//!
//! Rows that share one string id (a collision group) each produce their own
//! value, indexed by the row's position ([`RoutineIx`]); the projection then
//! makes the ONE per-id value a computation reads. In stage A every function
//! here reproduces its point's CURRENT rule (LAST row, FIRST row, per-row copies
//! kept), so the output is byte-identical to the per-id producers it replaced.
//! Stage B replaces the set-valued rules with the full-value union and the
//! single-valued ones with the LAST OWNING row (spec §4.6).
//!
//! [`RowsOfId`] is transient (R1-12): built from the CURRENT rows each time a
//! projection runs, dropped after. Building it runs the role check (§4.3, T9).

use std::cell::Cell;
use std::collections::HashMap;

use smallvec::SmallVec;

use super::ix::RoutineIx;
use super::workspace::ModelRoutine;

thread_local! {
    static GROUPS_MIXED_ROLE: Cell<usize> = const { Cell::new(0) };
}

/// The rows of each string id, in row order (spec §4.3). Transient: built from
/// the slice a producer iterates, read by the compat functions, then dropped.
pub struct RowsOfId<'a> {
    map: HashMap<&'a str, SmallVec<[RoutineIx; 1]>>,
}

impl<'a> RowsOfId<'a> {
    /// The rows of every id of `routines`, by position. Runs the role check:
    /// the rows of every collision group must share one role (all primary, or
    /// all of one dependency app). A mixed group is counted in
    /// [`groups_mixed_role`] in every build and fails a `debug_assert!`.
    #[must_use]
    pub fn build(routines: &'a [ModelRoutine]) -> Self {
        let mut map: HashMap<&'a str, SmallVec<[RoutineIx; 1]>> = HashMap::new();
        for (ri, r) in routines.iter().enumerate() {
            map.entry(r.id.as_str())
                .or_default()
                .push(RoutineIx::of(ri));
        }
        let rows = RowsOfId { map };
        let mixed = rows.mixed_role_groups(routines);
        GROUPS_MIXED_ROLE.with(|c| c.set(mixed));
        debug_assert!(mixed == 0, "collision group mixes roles ({mixed} groups)");
        rows
    }

    /// The rows of `id`, in row order (empty for an unknown id).
    #[must_use]
    pub fn rows(&self, id: &str) -> &[RoutineIx] {
        self.map.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Every id with its rows (map order: callers must not depend on it).
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &[RoutineIx])> + '_ {
        self.map.iter().map(|(id, rows)| (*id, rows.as_slice()))
    }

    /// Collision groups whose rows do not share one role.
    fn mixed_role_groups(&self, routines: &[ModelRoutine]) -> usize {
        self.map
            .values()
            .filter(|rows| rows.len() > 1)
            .filter(|rows| {
                let first = role_of(&routines[rows[0].get()]);
                rows[1..]
                    .iter()
                    .any(|ix| role_of(&routines[ix.get()]) != first)
            })
            .count()
    }
}

/// A row's role (§4.3): `None` for a primary (workspace) row, else the
/// dependency app it belongs to (from its `dep:<guid>:` unit, or a symbol-only
/// row's `app_guid`), lowercase.
fn role_of(r: &ModelRoutine) -> Option<String> {
    let unit = &r.source_anchor.source_unit_id;
    if unit.starts_with("ws:") {
        return None;
    }
    let guid = unit
        .strip_prefix("dep:")
        .and_then(|rest| rest.split_once(':'))
        .map_or(r.app_guid.as_str(), |(guid, _)| guid);
    Some(guid.to_ascii_lowercase())
}

/// The collision groups of `routines` that mix roles: the count the role check
/// of [`RowsOfId::build`] makes, without the check (the census line reads it).
#[must_use]
pub fn mixed_role_groups(routines: &[ModelRoutine]) -> usize {
    let mut map: HashMap<&str, SmallVec<[RoutineIx; 1]>> = HashMap::new();
    for (ri, r) in routines.iter().enumerate() {
        map.entry(r.id.as_str())
            .or_default()
            .push(RoutineIx::of(ri));
    }
    RowsOfId { map }.mixed_role_groups(routines)
}

/// The census counter `groups_mixed_role` (spec §4.3, T9): the collision groups
/// whose rows mix roles, as counted by the last [`RowsOfId::build`] on this
/// thread. Natural fixtures and CDO read 0.
#[must_use]
pub fn groups_mixed_role() -> usize {
    GROUPS_MIXED_ROLE.with(Cell::get)
}

/// One value per row (`per_row[ix]` is row `ix`'s; `None` = the row produces
/// nothing) taken out by row.
fn take<T>(per_row: &mut [Option<T>], ix: RoutineIx) -> Option<T> {
    per_row.get_mut(ix.get()).and_then(Option::take)
}

/// Compat rule LAST (stage A): each id's value is that of its LAST row that
/// produced one; an id no row produced one for is absent. This is today's
/// `collect()` / `insert`-per-row answer. Points: R2a `det_cone_inputs`; R2b the
/// cross-app base's `direct_coverage` / `direct_full` / `leaf_summaries`; R2c
/// the r3a3 builders; #13 `base_summaries`, `dispatch_by_id`, `routines_by_id`,
/// `body_avail_by_id`, `stable_map`; R6 the adapter's `routine_by_id`; the
/// detector context's `routine_by_id`, `access_modifiers`,
/// `root_classifications_by_routine`; `return_summary`, `ordering_facts`; the
/// unresolved cone's lookup; `dep_artifact_l4`'s scope frames.
#[must_use]
pub fn last_per_id<T>(rows: &RowsOfId<'_>, mut per_row: Vec<Option<T>>) -> HashMap<String, T> {
    let mut out = HashMap::with_capacity(rows.map.len());
    for (id, ixs) in rows.iter() {
        if let Some(v) = ixs.iter().rev().find_map(|&ix| take(&mut per_row, ix)) {
            out.insert(id.to_string(), v);
        }
    }
    out
}

/// [`last_per_id`] keyed by the borrowed id (`&str` maps).
#[must_use]
pub fn last_per_id_borrowed<'a, T>(
    rows: &RowsOfId<'a>,
    mut per_row: Vec<Option<T>>,
) -> HashMap<&'a str, T> {
    let mut out = HashMap::with_capacity(rows.map.len());
    for (id, ixs) in rows.iter() {
        if let Some(v) = ixs.iter().rev().find_map(|&ix| take(&mut per_row, ix)) {
            out.insert(id, v);
        }
    }
    out
}

/// Compat rule FIRST (stage A): each id's value is that of its FIRST row that
/// produced one: today's `entry().or_insert` answer. Points:
/// `dep_artifact_l4`'s operation orders (paired with LAST frames, today's mix).
#[must_use]
pub fn first_per_id<T>(rows: &RowsOfId<'_>, mut per_row: Vec<Option<T>>) -> HashMap<String, T> {
    let mut out = HashMap::with_capacity(rows.map.len());
    for (id, ixs) in rows.iter() {
        if let Some(v) = ixs.iter().find_map(|&ix| take(&mut per_row, ix)) {
            out.insert(id.to_string(), v);
        }
    }
    out
}

/// Compat rule for a per-key pick FIRST (stage A): over items emitted in row
/// order, the FIRST item per key wins (today's `entry().or_insert`). Points:
/// R9 `first_resolved_edge_per_callsite` (per call-site id), #22 the query's
/// `OperationJoin` (per operation id).
pub fn first_per_key<K: std::hash::Hash + Eq, T>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
) -> HashMap<K, T> {
    let mut out: HashMap<K, T> = HashMap::new();
    for item in items {
        out.entry(key(&item)).or_insert(item);
    }
    out
}

/// Compat rule for a per-key pick LAST (stage A): over items emitted in row
/// order, the LAST item per key wins (today's `insert`). Points: the detector
/// context's `call_site_by_id` and `root_classifications_by_routine`, the
/// snapshot's per-site anchors.
pub fn last_per_key<K: std::hash::Hash + Eq, T>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
) -> HashMap<K, T> {
    let mut out: HashMap<K, T> = HashMap::new();
    for item in items {
        out.insert(key(&item), item);
    }
    out
}

/// [`last_per_key`] over `(key, value)` pairs emitted in row order. Points: R5
/// `upgraded_bindings` (per call-site id).
pub fn last_per_key_pairs<K: std::hash::Hash + Eq, T>(
    items: impl IntoIterator<Item = (K, T)>,
) -> HashMap<K, T> {
    let mut out: HashMap<K, T> = HashMap::new();
    for (k, v) in items {
        out.insert(k, v);
    }
    out
}

/// Compat rule KEPT (stage A): the per-row copies pass through unchanged, a
/// collided id's rows each keeping their own item (class K, R4b). Points: the
/// event graph's publisher symbols, root classification, the policy
/// evaluation, the d1 seeds, the snapshot's per-row reads.
#[must_use]
pub fn kept_per_row<T>(per_row: Vec<T>) -> Vec<T> {
    per_row
}

/// The full-value union across the rows of ONE collided id (spec §4.6), in
/// sort-key order. STAGE A: today's rule for class U, the rows' items
/// concatenated in row order and stable-sorted by `sort_key` only (equal keys
/// keep emission order, nothing deduplicated). Stage B makes it the full-value
/// union (every payload type is `Ord` since stage A).
#[must_use]
pub fn union_by_value<T: Clone>(per_row: Vec<Vec<T>>, sort_key: impl Fn(&T) -> String) -> Vec<T> {
    let mut out: Vec<T> = per_row.into_iter().flatten().collect();
    out.sort_by_key(|t| sort_key(t));
    out
}

/// One call site of a routine, as the per-site consumers (d37,
/// `compose_roles_only`) read it (spec §4.6 `site_pick`).
pub struct SitePick<'a, E, B> {
    /// The resolved edge for the site.
    pub edge: Option<&'a E>,
    /// The site's argument bindings.
    pub bindings: Option<&'a B>,
}

/// `site_pick` (spec §4.6), COMPAT form (stage A): today's mix for the call site
/// `callsite_id` of a row of `from`: the FIRST edge with that call-site id in
/// the id's unioned edge list `edges` (whichever row emitted it), and the
/// bindings `bindings_by_callsite` holds for the id (R5: the LAST row's).
/// Stage B returns the LAST OWNING row's edge and bindings instead.
pub fn site_pick<'a, E, B>(
    edges: Option<&'a [E]>,
    edge_callsite: impl Fn(&E) -> Option<&str>,
    callsite_id: &str,
    bindings_by_callsite: &'a HashMap<String, B>,
) -> SitePick<'a, E, B> {
    SitePick {
        edge: edges.and_then(|es| es.iter().find(|e| edge_callsite(e) == Some(callsite_id))),
        bindings: bindings_by_callsite.get(callsite_id),
    }
}

#[cfg(test)]
mod tests {
    /// Every payload type a stage-B union sorts by full value has a TOTAL order
    /// (`Ord`), never a serialization or `Debug` tie-break. Deriving `Ord` is the
    /// check that no field is hash-ordered or a float: `HashMap`/`HashSet` and
    /// `f32`/`f64` have no `Ord`, so the derive (and this test) fails to compile.
    #[test]
    fn every_union_payload_type_has_a_total_order() {
        fn total<T: Ord>() {}
        total::<crate::engine::l4::combined_graph::CombinedEdge>();
        total::<crate::engine::l4::combined_graph::TypedEdge>();
        total::<crate::engine::l4::combined_graph::UncertaintyEdge>();
        total::<crate::engine::l4::summary::Uncertainty>();
        total::<crate::engine::l4::summary::DbEffect>();
        total::<crate::program::body::capability::CapabilityFact>();
        total::<crate::engine::root_classification::RootClassification>();
        total::<crate::engine::l5::snapshot::SnapshotCallsiteEvidence>();
        total::<crate::program::model::events::EventSymbol>();
        total::<crate::program::model::events::EventEdge>();
    }
}
