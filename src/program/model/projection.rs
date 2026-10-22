//! #160 step 11b (spec §4.3, §4.5, §4.6): the projection that turns per-row
//! producer outputs into the per-id inputs the cone, the solve, the event graph
//! and the lookups take.
//!
//! Rows that share one string id (a collision group) each produce their own
//! value, indexed by the row's position ([`RoutineIx`]); the projection then
//! makes the ONE per-id value a computation reads. An id with one row passes
//! its value through untouched (moved; no dedup, no re-sort). Only collided ids
//! are merged, by the one rule (spec §4.6):
//! - set-valued inputs (classes S, K, and class U's collided lists): the
//!   full-value union across the id's rows ([`merge_per_id`] with
//!   [`union_by_value`] / [`least_complete`]);
//! - single-valued lookups (class P): the LAST OWNING row per key
//!   ([`last_per_id`], [`last_per_key`], [`site_owners`], [`site_pick`]).
//!
//! [`RowsOfId`] is transient (R1-12): built from the CURRENT rows each time a
//! projection runs, dropped after. Building it runs the role check (§4.3, T9).

use std::cell::Cell;
use std::collections::HashMap;

use smallvec::SmallVec;

use super::ix::RoutineIx;
use super::workspace::ModelRoutine;
use crate::program::body::features::PCallSite;

thread_local! {
    static GROUPS_MIXED_ROLE: Cell<usize> = const { Cell::new(0) };
}

/// The rows of each string id, in row order (spec §4.3). Transient: built from
/// the slice a producer iterates, read by the projection, then dropped.
pub struct RowsOfId<'a> {
    map: HashMap<&'a str, SmallVec<[RoutineIx; 1]>>,
}

impl<'a> RowsOfId<'a> {
    /// The rows of every id of `routines`, by position. Runs the role check:
    /// the rows of every collision group must share one role (all primary, or
    /// all of one dependency app). A mixed group fails a `debug_assert!`; the
    /// count is readable through [`groups_mixed_role`].
    #[must_use]
    pub fn build(routines: &'a [ModelRoutine]) -> Self {
        let rows = Self::index(routines);
        let mixed = rows.mixed_role_groups(routines);
        GROUPS_MIXED_ROLE.with(|c| c.set(mixed));
        debug_assert!(mixed == 0, "collision group mixes roles ({mixed} groups)");
        rows
    }

    /// The map alone, without the role check.
    fn index(routines: &'a [ModelRoutine]) -> Self {
        let mut map: HashMap<&'a str, SmallVec<[RoutineIx; 1]>> = HashMap::new();
        for (ri, r) in routines.iter().enumerate() {
            map.entry(r.id.as_str())
                .or_default()
                .push(RoutineIx::of(ri));
        }
        RowsOfId { map }
    }

    /// The rows of `id`, in row order (empty for an unknown id).
    #[must_use]
    pub fn rows(&self, id: &str) -> &[RoutineIx] {
        self.map.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Whether `id` has two or more rows.
    #[must_use]
    pub fn is_collided(&self, id: &str) -> bool {
        self.rows(id).len() > 1
    }

    /// Every id with its rows (map order: callers must not depend on it).
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &[RoutineIx])> + '_ {
        self.map.iter().map(|(id, rows)| (*id, rows.as_slice()))
    }

    /// Every collided id with its rows (map order).
    pub fn collided(&self) -> impl Iterator<Item = (&'a str, &[RoutineIx])> + '_ {
        self.iter().filter(|(_, rows)| rows.len() > 1)
    }

    /// Collision groups whose rows do not share one role.
    fn mixed_role_groups(&self, routines: &[ModelRoutine]) -> usize {
        self.collided()
            .filter(|(_, rows)| {
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
    RowsOfId::index(routines).mixed_role_groups(routines)
}

/// The mixed-role count of the LAST [`RowsOfId::build`] on this thread (spec
/// §4.3, T9). A test observation only: every build overwrites it, so a later
/// clean build reads 0 again; the census line reads [`mixed_role_groups`].
#[must_use]
pub fn groups_mixed_role() -> usize {
    GROUPS_MIXED_ROLE.with(Cell::get)
}

/// One value per row (`per_row[ix]` is row `ix`'s; `None` = the row produces
/// nothing) taken out by row.
fn take<T>(per_row: &mut [Option<T>], ix: RoutineIx) -> Option<T> {
    per_row.get_mut(ix.get()).and_then(Option::take)
}

/// Class P, per id: each id's value is that of its LAST row that produced one
/// (every row of an id owns the id, so that is the last owning row); an id no
/// row produced one for is absent. Points: #13 `routines_by_id`,
/// `body_avail_by_id`, `dispatch_by_id` (per parameter), `stable_map`; R2b's
/// `routines_by_id`; the detector context's `access_modifiers`.
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

/// [`last_per_id`] keyed by the borrowed id (`&str` maps). Points: R6 the
/// adapter's routine pick, the detector context's `routine_by_id`, #14's seed
/// lookup, #17's subscriber index.
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

/// Classes S and K, per id: an id with one producing row gets that row's
/// value, moved; a collided id's produced values (in row order) are merged by
/// `merge` (a full-value union: [`union_by_value`], [`least_complete`]). An id
/// no row produced one for is absent. Points: R2a `det_cone_inputs`; R2b the
/// cross-app base's `direct_coverage` / `direct_full` / `leaf_summaries`; R2c
/// the r3a3 builders; #13 `base_summaries`; #14 `commits_by_routine`.
pub fn merge_per_id<T>(
    rows: &RowsOfId<'_>,
    mut per_row: Vec<Option<T>>,
    mut merge: impl FnMut(Vec<T>) -> T,
) -> HashMap<String, T> {
    let mut out = HashMap::with_capacity(rows.map.len());
    for (id, ixs) in rows.iter() {
        let mut vals: Vec<T> = ixs
            .iter()
            .filter_map(|&ix| take(&mut per_row, ix))
            .collect();
        let v = match vals.len() {
            0 => continue,
            1 => vals.pop().expect("one value"),
            _ => merge(vals),
        };
        out.insert(id.to_string(), v);
    }
    out
}

/// The full-value union of ONE collided id's items (spec §4.6): equal items
/// (every field equal) collapse to one, unequal items are all kept, ordered by
/// `sort_key` (today's sort key for the payload), then by the payload's total
/// order (`Ord`), never by row order. Never called for a single-row id.
#[must_use]
pub fn union_by_value<T: Ord, K: Ord>(per_row: Vec<Vec<T>>, sort_key: impl Fn(&T) -> K) -> Vec<T> {
    let mut out: Vec<T> = per_row.into_iter().flatten().collect();
    out.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

/// The least complete of a collided id's coverage records (spec §4.6, decision
/// 7): the status is the least complete (`complete` < `partial` < anything
/// else, e.g. `unknown`), the reasons a sorted union.
#[must_use]
pub fn least_complete(per_row: Vec<(String, Vec<String>)>) -> (String, Vec<String>) {
    let rank = |s: &str| match s {
        "complete" => 0,
        "partial" => 1,
        _ => 2,
    };
    let mut status = String::new();
    let mut reasons: Vec<String> = Vec::new();
    for (s, r) in per_row {
        if status.is_empty() || rank(&s) > rank(&status) {
            status = s;
        }
        reasons.extend(r);
    }
    reasons.sort();
    reasons.dedup();
    (status, reasons)
}

/// Class P, per key: over items emitted in row order, the LAST item per key
/// wins, i.e. the LAST OWNING row's. Points: the detector context's
/// `call_site_by_id` and `root_classifications_by_routine`.
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
/// `upgraded_bindings` (per call-site id; every row's site pushes its bindings,
/// so the last pair is the LAST OWNING row's).
pub fn last_per_key_pairs<K: std::hash::Hash + Eq, T>(
    items: impl IntoIterator<Item = (K, T)>,
) -> HashMap<K, T> {
    let mut out: HashMap<K, T> = HashMap::new();
    for (k, v) in items {
        out.insert(k, v);
    }
    out
}

/// Within one key, the FIRST item in emission order (the within-row candidate
/// policy, spec §4.6). Points: R9 `first_resolved_edge_per_callsite`, after its
/// edges are restricted to each collided site's LAST owning row.
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

/// The LAST OWNING row of every call site of a COLLIDED id (spec §4.6): the
/// last row of the id, in row order, whose `call_sites` hold that call-site id.
/// A single-row id's sites are absent (their only row owns them).
#[must_use]
pub fn site_owners<'a>(
    rows: &RowsOfId<'_>,
    routines: &'a [ModelRoutine],
) -> HashMap<&'a str, RoutineIx> {
    let mut out = HashMap::new();
    for (_, ixs) in rows.collided() {
        for &ix in ixs {
            for cs in &routines[ix.get()].call_sites {
                out.insert(cs.id.as_str(), ix);
            }
        }
    }
    out
}

/// The owned call sites of every COLLIDED id (spec §4.6, the per-site
/// consumers): the union of its rows' call-site keys, in first-appearance order
/// over the rows, each the LAST OWNING row's `PCallSite`. A single-row id is
/// absent: its consumers read its own `call_sites`.
#[must_use]
pub fn owned_sites<'a>(
    rows: &RowsOfId<'_>,
    routines: &'a [ModelRoutine],
) -> HashMap<&'a str, Vec<&'a PCallSite>> {
    let mut out: HashMap<&'a str, Vec<&'a PCallSite>> = HashMap::new();
    for (_, ixs) in rows.collided() {
        let mut keys: Vec<&'a str> = Vec::new();
        let mut pick: HashMap<&'a str, &'a PCallSite> = HashMap::new();
        for &ix in ixs {
            for cs in &routines[ix.get()].call_sites {
                if pick.insert(cs.id.as_str(), cs).is_none() {
                    keys.push(cs.id.as_str());
                }
            }
        }
        let id = routines[ixs[0].get()].id.as_str();
        out.insert(id, keys.into_iter().map(|k| pick[k]).collect());
    }
    out
}

/// The call sites a per-site consumer visits for row `routine`: its id's owned
/// sites ([`owned_sites`]) when the id is collided, else its own.
#[must_use]
pub fn sites_of<'a>(
    owned: &HashMap<&str, Vec<&'a PCallSite>>,
    routine: &'a ModelRoutine,
) -> Vec<&'a PCallSite> {
    owned
        .get(routine.id.as_str())
        .cloned()
        .unwrap_or_else(|| routine.call_sites.iter().collect())
}

/// One call site, as the per-site consumers (d37, `compose_roles_only`) read it
/// (spec §4.6 `site_pick`).
pub struct SitePick<'a, B> {
    /// The callee of the site's resolved edge.
    pub callee: Option<&'a str>,
    /// The site's argument bindings.
    pub bindings: Option<&'a B>,
}

/// `site_pick` (spec §4.6): the callee and the bindings of the call site
/// `callsite_id` of `from`, both from ONE row. A collided id's site reads the
/// LAST OWNING row's first targeted edge (`graph.collided_site_callee`, built
/// through `CallEdge.from_ix`) and that row's bindings (R5, the last owning
/// row's). A single-row id is untouched: the first edge with the call-site id
/// in `edges_by_from[from]` and R5's bindings.
pub fn site_pick<'a, B>(
    graph: &'a crate::engine::l4::combined_graph::PrimaryGraph,
    from: &str,
    callsite_id: &str,
    bindings_by_callsite: &'a HashMap<String, B>,
) -> SitePick<'a, B> {
    let callee = match graph.collided_site_callee.get(callsite_id) {
        Some(pick) => pick.as_deref(),
        None => graph.edges_by_from.get(from).and_then(|es| {
            es.iter()
                .find(|e| e.callsite_id.as_deref() == Some(callsite_id))
                .map(|e| e.to.as_str())
        }),
    };
    SitePick {
        callee,
        bindings: bindings_by_callsite.get(callsite_id),
    }
}

#[cfg(test)]
mod tests {
    /// T1-cov's second case (spec §5.1), on the helper `merge_per_id` calls at
    /// R2a/R2b/R2c (T1-cov pins that production use through the reasons): a
    /// `partial` row and a `complete` row make `partial`, in either row order,
    /// and the reasons are the sorted union. Break: LAST status -> the
    /// `complete`-last order reads `complete`.
    #[test]
    fn least_complete_takes_the_least_complete_status() {
        let partial = (
            "partial".to_string(),
            vec!["b".to_string(), "a".to_string()],
        );
        let complete = ("complete".to_string(), vec!["a".to_string()]);
        let want = (
            "partial".to_string(),
            vec!["a".to_string(), "b".to_string()],
        );
        assert_eq!(
            super::least_complete(vec![partial.clone(), complete.clone()]),
            want
        );
        assert_eq!(super::least_complete(vec![complete, partial]), want);
    }

    /// Every payload type a collided id's union sorts by full value has a TOTAL order
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
        // The cone's own fact type: R2a/R2b/R2c union it.
        total::<crate::engine::l4::capability_cone::CapabilityFact>();
        total::<crate::engine::root_classification::RootClassification>();
        total::<crate::engine::l5::snapshot::SnapshotCallsiteEvidence>();
        total::<crate::program::model::events::EventSymbol>();
        total::<crate::program::model::events::EventEdge>();
    }
}
