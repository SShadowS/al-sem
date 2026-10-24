//! `DetectorContext` — port of al-sem `src/detectors/detector-context.ts`.
//!
//! The shared, eager indexes + derived graphs detectors read from, built once at
//! the top of `run_detectors`. This R4-A wave builds the EAGER indexes the task
//! enumerates: routine_by_id / objects_by_id / table_by_id / reverse_call_graph /
//! entry_points / transaction_spans / resolved_call_edge_by_callsite /
//! uncertainty_edges_by_from / call_site_by_id, plus the combined graph + the
//! per-routine `FullRoutineSummary` map (transaction_spans needs it).
//!
//! DEFERRED to later waves (TODO):
//!   - the lazy `get_event_flow_indexes()` hook (D43/D44/D45)
//!   - the ordering facts (D47; since #160 step 2 a slot filled before the
//!     detectors run, `get_ordering_facts()`)
//!
//! d4 reads none of these; later detector waves add them as they land.
//!
//! The R4-G wave wired `reachable_roots` + `internal_reachable_externally` (D14):
//! `reachable_roots` is built via `entry_points::find_reachable_roots` over the
//! `access_modifiers` map harvested from `ModelRoutine.access_modifier`;
//! `internal_reachable_externally` DEFAULTS to `false` (see field doc).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::engine::l4::analysis::{CoreSummaries, L4Analysis, L4Request, build_l4};
use crate::engine::l4::capability_cone::{
    CapabilityFact, callsite_local_coverage_reasons, compose_cone_over_graph,
    direct_facts_for_routine, fold_uncertainty_reasons,
};
use crate::engine::l4::combined_graph::{PrimaryGraph, TypedEdge};
use crate::engine::l4::cone_derived::{ConeDerivedStore, ConeScope, WitnessScope};
use crate::engine::l4::exec_relation::ExecRelation;
use crate::engine::l4::reverse_index::ReverseEffectIndex;
use crate::engine::l4::summary::{
    RecordRoleSummary, Uncertainty, dedupe_uncertainties, uncertainty_key,
};
use crate::engine::l5::confidence::UncertaintyLite;
use crate::engine::l5::entry_points::AccessModifier;
use crate::engine::l5::event_flow::{EventFlowIndexes, build_event_flow_indexes};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::reverse_call_graph::{ReverseCallGraph, build_reverse_call_graph};
use crate::engine::l5::transaction_spans::{TransactionSpan, compute_transaction_spans};
use crate::engine::perf_trace as pt;
use crate::program::body::features::PCallSite;
use crate::program::model::calls::{CallEdge, UpgradedBinding};
use crate::program::model::events::{EventGraph, EventSymbol};
use crate::program::model::projection;
use crate::program::model::workspace::{Model, ModelObject, ModelRoutine, ModelTable};

/// A declared workspace dependency (`model.identity.primaryDependencies[]`): the
/// `appGuid` / `name` / `minVersion` triple d17 iterates. Mirrors al-sem's
/// `ManifestDependency` (the d17-relevant subset). Source-only runs leave this empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDep {
    pub app_guid: String,
    pub name: String,
    pub min_version: String,
}

/// An OWNED uncertainty substrate — the index plus the node map — for callers that
/// build one outside a [`DetectorContext`] (fixtures, and any walker driven
/// directly). Hands out a [`UncertaintyView`] over itself.
#[derive(Default)]
pub struct OwnedUncertainties {
    pub index: UncertaintyIndex,
    pub by_node: HashMap<String, UncertaintySetId>,
}

impl OwnedUncertainties {
    pub fn view(&self) -> UncertaintyView<'_> {
        UncertaintyView {
            index: &self.index,
            by_node: &self.by_node,
        }
    }

    /// Intern `set` and attach it to `node`, replacing any previous set.
    pub fn insert(&mut self, node: &str, set: Vec<Uncertainty>) {
        let sid = self.index.intern_set(set);
        self.by_node.insert(node.to_string(), sid);
    }
}

/// Hash-cons pool for [`DetectorContext::uncertainties_by_node`]'s per-node sets.
///
/// The L4 summary solver broadcasts an SCC's whole uncertainty union to each of its
/// members, so on a large workspace thousands of nodes hold the byte-identical
/// deduped set. This pool keys a set by its own CONTENT — `Arc<[Uncertainty]>`
/// hashes and compares as `[Uncertainty]` does, and `Arc<T>: Borrow<T>` lets the
/// lookup take a plain `&[Uncertainty]` with no probe allocation — so the second and
/// every later node carrying an equal set gets a refcount bump instead of its own
/// copy.
///
/// **Content, not identity.** Which node's `Vec` becomes the canonical allocation is
/// therefore unobservable: the pool only ever hands back a slice `Eq` to the one it
/// was asked for, so the map's VALUES are byte-identical to the un-pooled `Vec`s
/// they replace, element for element and in the same order.
/// A distinct uncertainty VALUE, interned run-globally at context build.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct UncertaintyId(pub u32);

/// A distinct uncertainty SET — the per-node set the pool already collapses by
/// content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct UncertaintySetId(pub u32);

/// The uncertainty identity substrate: every distinct VALUE and every distinct
/// SET, each named once.
///
/// **Why this type exists.** [`UncertaintySetPool`] (which it replaces) already
/// computed exactly this identity — it keys per-node sets by CONTENT, collapsing
/// 27,037 nodes onto 10,112 distinct allocations over a 19,311-value vocabulary on
/// BC Base App — and then discarded it, returning only `Arc<[Uncertainty]>`. Every
/// consumer downstream then re-derived it: `d1` re-interned 2,229,391 elements per
/// run into a detector-local table and memoized per-set work behind a raw POINTER
/// key, and `d1_reach` answered "does this node carry uncertainty" with a
/// `HashMap<String, _>` lookup. Naming the identity once, here, removes all of it.
///
/// **Order is part of set identity, deliberately.** The union these sets feed is
/// last-write-wins by `uncertainty_key`, so two sets holding the same values in a
/// different order are NOT interchangeable — collapsing them would silently change
/// which value wins. `intern_set` therefore keys on the id sequence as given, and
/// does NOT dedupe or sort: its input is already `dedupe_uncertainties`' output,
/// and re-deduping here would change the sequence the caller round-trips back.
#[derive(Default)]
pub struct UncertaintyIndex {
    values: Vec<Uncertainty>,
    /// `uncertainty_key(values[i])`, precomputed at intern time: `dedupe` needs it
    /// once per id per union, and recomputing the `format!` there would reintroduce
    /// exactly the per-record allocation this index exists to remove.
    keys: Vec<Box<str>>,
    /// The confidence mapper's view of `values[i]` — its kind plus the
    /// materialised `"{kind} at {at}"` note text. d1 builds one `Evidence` per id
    /// per winning cohort, so this is allocated once per DISTINCT value rather
    /// than once per record. Note it is a DIFFERENT string from `keys[i]`
    /// (`"{kind} at {at}"` vs `"{kind}|{at}"`) and they do NOT sort alike, so
    /// neither can stand in for the other.
    lites: Vec<UncertaintyLite>,
    by_value: HashMap<Uncertainty, UncertaintyId>,
    /// Flat element pool; a set's elements are a window into it.
    set_elems: Vec<UncertaintyId>,
    set_span: Vec<(u32, u32)>,
    by_set: HashMap<Box<[UncertaintyId]>, UncertaintySetId>,
}

impl UncertaintyId {
    /// Test-only: a stand-in id that never came from an index.
    #[cfg(test)]
    pub(crate) fn for_test(n: u32) -> Self {
        UncertaintyId(n)
    }
}

impl UncertaintyIndex {
    /// Intern one VALUE.
    pub fn intern_value(&mut self, u: &Uncertainty) -> UncertaintyId {
        if let Some(&id) = self.by_value.get(u) {
            return id;
        }
        // `try_from`, not `as`: past `u32::MAX` an `as` cast WRAPS and a new value
        // would silently alias an existing id — a wrong answer rather than a crash.
        let id = UncertaintyId(
            u32::try_from(self.values.len())
                .expect("UncertaintyIndex exceeded u32::MAX distinct uncertainties"),
        );
        self.values.push(u.clone());
        self.keys.push(uncertainty_key(u).into_boxed_str());
        self.lites.push(UncertaintyLite::of(u));
        self.by_value.insert(u.clone(), id);
        id
    }

    /// `uncertainty_key(value(id))`, precomputed.
    pub fn key(&self, id: UncertaintyId) -> &str {
        &self.keys[id.0 as usize]
    }

    /// The confidence mapper's view of `id`, precomputed.
    pub fn lite(&self, id: UncertaintyId) -> &UncertaintyLite {
        &self.lites[id.0 as usize]
    }

    /// Dedupe by `uncertainty_key`, LAST-WRITE-WINS, emitted in byte-sorted key
    /// order — the same contract as `dedupe_uncertainties` on values, and the
    /// reason a union's input order is load-bearing (two `interface-open-world`
    /// values differing only in `interfaceName` share a key, and the later one
    /// wins).
    pub fn dedupe(&self, ids: &[UncertaintyId]) -> Vec<UncertaintyId> {
        let mut seen: std::collections::BTreeMap<&str, UncertaintyId> =
            std::collections::BTreeMap::new();
        for &id in ids {
            seen.insert(self.key(id), id);
        }
        seen.into_values().collect()
    }

    /// Intern one SET, returning the id of an equal (same values, same ORDER) set
    /// if one was already interned.
    pub fn intern_set(&mut self, set: Vec<Uncertainty>) -> UncertaintySetId {
        let ids: Box<[UncertaintyId]> = set.iter().map(|u| self.intern_value(u)).collect();
        if let Some(&sid) = self.by_set.get(&ids) {
            return sid;
        }
        let start = u32::try_from(self.set_elems.len()).expect("set element pool exceeded u32");
        self.set_elems.extend_from_slice(&ids);
        let end = u32::try_from(self.set_elems.len()).expect("set element pool exceeded u32");
        let sid = UncertaintySetId(
            u32::try_from(self.set_span.len()).expect("UncertaintyIndex exceeded u32::MAX sets"),
        );
        self.set_span.push((start, end));
        self.by_set.insert(ids, sid);
        sid
    }

    pub fn elements(&self, s: UncertaintySetId) -> &[UncertaintyId] {
        let (a, b) = self.set_span[s.0 as usize];
        &self.set_elems[a as usize..b as usize]
    }

    /// O(1), no map lookup and no allocation — the replacement for
    /// `uncertainties_by_node.get(id).is_none_or(|v| v.is_empty())`.
    pub fn is_empty_set(&self, s: UncertaintySetId) -> bool {
        let (a, b) = self.set_span[s.0 as usize];
        a == b
    }

    pub fn value(&self, id: UncertaintyId) -> &Uncertainty {
        &self.values[id.0 as usize]
    }

    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    pub fn set_count(&self) -> usize {
        self.set_span.len()
    }

    /// The retained census's shallow walk (#160 §4.1.5): every buffer, the
    /// precomputed keys and each interned set's boxed id slice.
    pub(crate) fn census_tally(&self) -> crate::engine::census::Tally {
        let mut t = crate::engine::census::Tally::default();
        t.buf::<Uncertainty>(self.values.len())
            .buf::<Box<str>>(self.keys.len())
            .strs(&self.keys)
            .buf::<UncertaintyLite>(self.lites.len())
            .buf::<(Uncertainty, UncertaintyId)>(self.by_value.len())
            .buf::<UncertaintyId>(self.set_elems.len())
            .buf::<(u32, u32)>(self.set_span.len())
            .buf::<(Box<[UncertaintyId]>, UncertaintySetId)>(self.by_set.len());
        for ids in self.by_set.keys() {
            t.buf::<UncertaintyId>(ids.len());
        }
        t
    }
}

impl UncertaintyIndex {
    /// `ALSEM_UNCERTAINTY_CENSUS=1` — live bytes of the uncertainty substrate,
    /// priced BOTH ways from the same run: as it is now (per-set windows of 4-byte
    /// ids into one value table) and as the pre-substrate shape would have cost
    /// (each distinct set a hash-consed `Arc<[Uncertainty]>` of full records).
    ///
    /// Pricing both from one measurement is deliberate: the old shape's code is
    /// deleted, so a "before" number could otherwise only be quoted from a
    /// previous arc's census — a different probe, on a different build, counting
    /// slightly differently. The distinct-set population is identical either way,
    /// so charging it twice from the same data is an apples-to-apples comparison
    /// rather than a cross-run one.
    ///
    /// Heap bytes only — the `String`/`Arc<str>` payloads and the backing buffers.
    /// `HashMap` control-byte overhead and allocator rounding are NOT modelled, so
    /// both sides UNDERSTATE by the same convention; the DELTA is the claim, not
    /// the absolute.
    pub fn emit_census(&self, by_node: &HashMap<String, UncertaintySetId>) {
        if std::env::var("ALSEM_UNCERTAINTY_CENSUS").as_deref() != Ok("1") {
            return;
        }
        fn rec_bytes(u: &Uncertainty) -> u64 {
            let opt = |o: &Option<String>| o.as_ref().map_or(0, |s| s.len() as u64 + 24);
            // 24 = String's own ptr/len/cap words, charged per field.
            u.kind.len() as u64
                + 24
                + opt(&u.callsite_id)
                + opt(&u.operation_id)
                + opt(&u.routine_id)
                + opt(&u.interface_name)
        }

        let values_bytes: u64 = self.values.iter().map(rec_bytes).sum();
        let keys_bytes: u64 = self.keys.iter().map(|k| k.len() as u64 + 16).sum();
        let lites_bytes: u64 = self.lites.iter().map(|l| l.census_heap_bytes()).sum();
        // `by_value` stores a SECOND copy of every value as its key.
        let by_value_bytes: u64 = values_bytes + (self.by_value.len() as u64 * 4);
        let set_elems_bytes = self.set_elems.len() as u64 * 4;
        let set_span_bytes = self.set_span.len() as u64 * 8;
        // `by_set` stores a second copy of every set's id slice as its key.
        let by_set_bytes: u64 = set_elems_bytes + (self.by_set.len() as u64 * 4);
        let node_map_bytes: u64 = by_node.keys().map(|k| k.len() as u64 + 24 + 4).sum();

        let now = values_bytes
            + keys_bytes
            + lites_bytes
            + by_value_bytes
            + set_elems_bytes
            + set_span_bytes
            + by_set_bytes
            + node_map_bytes;

        // The pre-substrate shape: one `Arc<[Uncertainty]>` per DISTINCT set,
        // holding full records, plus a node map of 8-byte `Arc` pointers. It had
        // no value table, no key/lite precompute and no id pools.
        let as_records: u64 = (0..self.set_span.len())
            .map(|i| {
                self.elements(UncertaintySetId(i as u32))
                    .iter()
                    .map(|id| rec_bytes(self.value(*id)))
                    .sum::<u64>()
            })
            .sum();
        let then = as_records + by_node.keys().map(|k| k.len() as u64 + 24 + 8).sum::<u64>();

        let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
        eprintln!(
            "[uncertainty-census] nodes={} distinct_values={} distinct_sets={} set_elements={}",
            by_node.len(),
            self.values.len(),
            self.set_span.len(),
            self.set_elems.len(),
        );
        eprintln!(
            "[uncertainty-census] now={:.1}MiB = values {:.1} + keys {:.1} + lites {:.1} + by_value {:.1} + set_elems {:.1} + set_span {:.1} + by_set {:.1} + node_map {:.1}",
            mib(now),
            mib(values_bytes),
            mib(keys_bytes),
            mib(lites_bytes),
            mib(by_value_bytes),
            mib(set_elems_bytes),
            mib(set_span_bytes),
            mib(by_set_bytes),
            mib(node_map_bytes),
        );
        eprintln!(
            "[uncertainty-census] same sets priced as RECORDS (pre-substrate shape) = {:.1}MiB; delta = {:+.1}MiB",
            mib(then),
            mib(then) - mib(now),
        );
    }
}

/// `ALSEM_CONES_CENSUS=1` — attribution inside the `context.capability_cones`
/// span, which the post-substrate profile shows as the largest single item in an
/// `alsem analyze` run (~15.8 s of ~66 s on BC Base App 8020) but which a span
/// cannot break down: it measures a region, not which of the three phases inside
/// it costs.
///
/// The three phases are the whole span: per-routine DIRECT fact extraction
/// (`direct_facts_for_routine` over every routine), the SCC cone WALK
/// (`compose_cone_over_graph`), and per-routine summary ASSEMBLY (draining the
/// cones into `FullRoutineSummary`). Counters plus three `Instant` pairs taken
/// once each — nothing per-routine, so the probe cannot distort what it measures.
pub(crate) mod cones_census {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    pub(crate) static ROUTINES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DIRECT_FACTS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static DIRECT_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WALK_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static ASSEMBLE_NANOS: AtomicU64 = AtomicU64::new(0);
    // Inside the walk: typed-edge graph build, Tarjan, direct-fact dedup, and the
    // SCC-order cone composition itself.
    pub(crate) static WALK_GRAPH_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WALK_SCC_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WALK_DEDUP_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WALK_COMPOSE_NANOS: AtomicU64 = AtomicU64::new(0);
    // Inside compose: the two per-member inherited-fact walks. The recursive
    // arm splits into the WALK and the FOLD (#160 §4.1.1). Since step 7a
    // (§4.5) `BFS_WALK_NANOS` holds each recursive SCC's set build (root) or
    // borrow (non-root) plus the per-member walks that still run (raw modes);
    // `BFS_FOLD_NANOS` the derived fold plus `emit_picks` in raw modes. Since
    // step 7b the set is folded ONCE per SCC (its row built, each member's
    // `fold_row` + own entries, and the row's destruction all charge it).
    // `BFS_PICKS` counts picks of walks that ran (0 under `raw: WitnessScope::None`).
    pub(crate) static BFS_WALK_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BFS_FOLD_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BFS_PICKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BFS_CALLS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SINGLETON_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SINGLETON_CALLS: AtomicU64 = AtomicU64::new(0);
    /// Inside `inherited_facts_for_singleton`, which the first census round left
    /// at ONE number (4.8 s over 100,419 calls) with nothing saying what inside
    /// it costs — and where precomputing `edge_sort_key` then bought only 7 %,
    /// proving the cost is elsewhere. Split: the per-edge SUBST of frame-bound
    /// entries, the candidate scan over every out-edge's callee cone
    /// (`SCAN_CONE`), the scan over the subject's own producer-solve entries
    /// (`SCAN_OWN`), and the raw materialization (`retag` + `sort_inherited`,
    /// skipped for a member with no raw half). Since #160 step 7b the scans
    /// run for the raw half only (0 in `analyze`), and the derived row is
    /// folded from memoized closed-set rows: memo-row builds and destruction
    /// (`MEMO`; the destruction runs in the refcount-free loop and is added to
    /// `SINGLETON_NANOS` and `MEMO` from one reading, so it is counted once),
    /// `fold_row` (`ROW`), the substituted open entries (`OPEN`) and the own
    /// entries (`OWN_FOLD`). All count inside `SINGLETON_NANOS`; none is a
    /// [`component_snapshot`] component.
    pub(crate) static SGL_SUBST_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_SCAN_CONE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_SCAN_OWN_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_MEMO_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_ROW_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_OPEN_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_OWN_FOLD_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_RAW_NANOS: AtomicU64 = AtomicU64::new(0);
    /// #160 step 7b §4.8: the closed-set memo, summed from each compose call's
    /// `ComposeStats`, plus the most memo rows alive at once in one compose
    /// (set only under the census).
    pub(crate) static CLOSED_MEMO_BUILDS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CLOSED_MEMO_READS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CLOSED_MEMO_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SCC_SET_FOLDS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SINGLETON_SCANS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CLOSED_MEMO_LIVE_MAX: AtomicU64 = AtomicU64::new(0);
    /// Populations the scan time is per (since #160 step 7b, the raw-half scan
    /// only: these and the `SGL_CALLS_*` / `SGL_ENTRIES_N_CONES` counts stay 0
    /// under `raw: WitnessScope::None`, and the report prints their two lines only when a
    /// singleton scanned). Accumulated in plain locals inside the
    /// walk and folded in with ONE `fetch_add` per call — the inner loop runs
    /// millions of times and must not touch an atomic (the distortion this
    /// module's `add_gated` doc records).
    pub(crate) static SGL_EDGES: AtomicU64 = AtomicU64::new(0);
    /// The `(key, entry)` pairs scanned — the REAL population of the walk, as
    /// opposed to its call count — by class: CLOSED cone entries (read from the
    /// callee cone as-is), OPEN entries (frame-bound, carried per edge through
    /// `substitute_entry`), and the subject's OWN producer-solve entries.
    /// Until #160 one `cone_entries=` figure counted all three; it is now
    /// `closed_entries + open_entries + own_entries`.
    pub(crate) static SGL_CLOSED_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_OPEN_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_OWN_ENTRIES: AtomicU64 = AtomicU64::new(0);
    /// `best` insertions — each one a `String` key clone today.
    pub(crate) static SGL_WINS: AtomicU64 = AtomicU64::new(0);
    /// Surviving `best` entries summed over the calls that scanned.
    pub(crate) static SGL_BESTS: AtomicU64 = AtomicU64::new(0);
    /// How the walk's calls distribute over the number of CONE-BEARING out-edges
    /// they actually merge. A call with one cone-bearing edge needs no merge at
    /// all — `best` would be a key-for-key copy of that single callee cone, which
    /// is already a `BTreeMap` in the same key order.
    pub(crate) static SGL_CALLS_0_CONES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_CALLS_1_CONE: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_CALLS_N_CONES: AtomicU64 = AtomicU64::new(0);
    /// Cone entries scanned by calls that merge two or more cones — the entries a
    /// single-cone fast path would NOT remove.
    pub(crate) static SGL_ENTRIES_N_CONES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static MAX_SCC_MEMBERS: AtomicU64 = AtomicU64::new(0);
    /// The rest of compose: the per-SCC coverage cone, and the per-member
    /// `CoverageRecord` build + `out.insert` that follows the inherited walk.
    pub(crate) static COVERAGE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static EMIT_NANOS: AtomicU64 = AtomicU64::new(0);
    /// `ConeDerivedBuilder` folding: begin/fold_fact/finish per routine.
    pub(crate) static DERIVED_NANOS: AtomicU64 = AtomicU64::new(0);
    /// The per-SCC FACT cone: union of the successors' cones + this SCC's own
    /// direct facts, built once per non-root SCC.
    pub(crate) static FACTCONE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FACTCONE_CALLS: AtomicU64 = AtomicU64::new(0);
    /// `merge_cone` traffic: total merges, and how many hit the tie-break path
    /// that calls `rep_key` TWICE (each call JSON-serializes two fields).
    pub(crate) static MERGE_CALLS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static MERGE_TIEBREAKS: AtomicU64 = AtomicU64::new(0);
    /// #69 D1.4: the producer solve (`l4::pd_solve::solve`), summed over every
    /// cone composition: producers, states, links, resolved (non-seed) entries,
    /// the distinct edges the links cross.
    pub(crate) static PD_PRODUCERS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_STATES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_LINKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_EDGES: AtomicU64 = AtomicU64::new(0);
    /// The solve's wall time and call count (a second solve is a second call).
    pub(crate) static PD_SOLVE_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_SOLVE_CALLS: AtomicU64 = AtomicU64::new(0);
    /// #160 G-1: the bytes each solve retains, split in two (spec §4.1.1).
    /// `_ARENA` (`pd_solve::arena_tally`): producers, states, links, edges,
    /// `by_routine`, the part step 6 may retain. `_WORKING`
    /// (`pd_solve::working_tally`): resolved own entries, interned names,
    /// reps, the part step 6 retains or re-solves. Charged only by
    /// `pd_solve::census`, never by a snapshot.
    pub(crate) static PD_SOLVE_BYTES_ARENA: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PD_SOLVE_BYTES_WORKING: AtomicU64 = AtomicU64::new(0);
    /// The bytes of every snapshot `PdArena` derived from a solve
    /// (`snapshot::derive_pd_arena`). The pre-#160 `solve_bytes=` figure was
    /// `_ARENA + _WORKING + PD_SNAPSHOT_BYTES`, printed as
    /// `solve_bytes_old_equiv=`.
    pub(crate) static PD_SNAPSHOT_BYTES: AtomicU64 = AtomicU64::new(0);
    /// #69 D2: solve links whose cone edge has no snapshot typed edge (dropped).
    pub(crate) static PD_EDGE_UNMAPPED: AtomicU64 = AtomicU64::new(0);
    /// #160 step 6 (R6-4): the distinct cone edges behind those dropped links
    /// (anchorless rows that carry a link). Both are counted ungated, so the
    /// step-6 probe reads them without the census.
    pub(crate) static PD_EDGE_UNMAPPED_KEYS: AtomicU64 = AtomicU64::new(0);
    /// #160 step 6 D8: the witness DAGs built (one per root and first hop),
    /// their routines and their predecessor entries (spec §14.9's census line).
    pub(crate) static WITNESS_DAGS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WITNESS_DAG_NODES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WITNESS_DAG_PREDS: AtomicU64 = AtomicU64::new(0);
    /// What `compose_inherited_cones` spends outside every timed component,
    /// charged PER CALL as `elapsed - Σ(component delta)` in `i64`. The one
    /// signed counter: a negative total means two component timers overlap.
    pub(crate) static COMPOSE_REST_NANOS: AtomicI64 = AtomicI64::new(0);
    /// #160 step 7a §4.5: the Cone-SCC census, summed from each compose call's
    /// [`ComposeStats`](crate::engine::l4::capability_cone::ComposeStats).
    pub(crate) static CONE_SCC_RECURSIVE: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_MEMBERS_ALL: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_MEMBERS_EMITTED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_ROOTS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_SELF_LOOPS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_MAX: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_SIZE_HIST: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    pub(crate) static CONE_SCC_MEMBER_WALKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_OWN_ENTRIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_SETS_BORROWED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_SETS_BUILT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_SET_ENTRIES: AtomicU64 = AtomicU64::new(0);
    /// The oracle classes (spec §5.8): members with an own entry, SCCs with a
    /// successor, SCCs with a guarded (frame-bound) successor entry.
    pub(crate) static CONE_SCC_OWN_ENTRY_MEMBERS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_WITH_SUCCESSOR: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CONE_SCC_WITH_GUARDED_SUCCESSOR: AtomicU64 = AtomicU64::new(0);
    /// A REPORT value, never a component (the §4.5 overlap rule): the time of
    /// the per-member walks only, from the same readings `BFS_WALK_NANOS`
    /// adds. `BFS_WALK_NANOS` also holds the set builds and borrows, so
    /// `walk_total <= bfs_walk`; under `raw: WitnessScope::None` `walk_total` is 0.
    pub(crate) static WALK_TOTAL_NANOS: AtomicU64 = AtomicU64::new(0);

    /// Fold one compose call's stats into the Cone-SCC census, if it is on.
    pub(crate) fn add_compose_stats(s: &crate::engine::l4::capability_cone::ComposeStats) {
        if !enabled() {
            return;
        }
        add(&CONE_SCC_RECURSIVE, s.recursive_sccs);
        add(&CONE_SCC_MEMBERS_ALL, s.recursive_members_all);
        add(&CONE_SCC_MEMBERS_EMITTED, s.recursive_members_emitted);
        add(&CONE_SCC_ROOTS, s.recursive_roots);
        add(&CONE_SCC_SELF_LOOPS, s.self_loops);
        max(&CONE_SCC_MAX, s.max_recursive_members);
        for (c, v) in CONE_SCC_SIZE_HIST.iter().zip(s.size_hist) {
            add(c, v);
        }
        add(&CONE_SCC_MEMBER_WALKS, s.member_walks);
        add(&CONE_SCC_OWN_ENTRIES, s.own_entries_recursive);
        add(&CONE_SCC_SETS_BORROWED, s.scc_sets_borrowed);
        add(&CONE_SCC_SETS_BUILT, s.scc_sets_built);
        add(&CONE_SCC_SET_ENTRIES, s.scc_set_entries_folded);
        add(&CONE_SCC_OWN_ENTRY_MEMBERS, s.members_with_own_entry);
        add(&CONE_SCC_WITH_SUCCESSOR, s.with_successor);
        add(&CONE_SCC_WITH_GUARDED_SUCCESSOR, s.with_guarded_successor);
        add(&CLOSED_MEMO_BUILDS, s.closed_memo_builds);
        add(&CLOSED_MEMO_READS, s.closed_memo_reads);
        add(&CLOSED_MEMO_ENTRIES, s.closed_memo_entries);
        add(&SCC_SET_FOLDS, s.scc_set_folds);
        add(&SINGLETON_SCANS, s.singleton_scans);
    }

    /// The eight components of compose that [`COMPOSE_REST_NANOS`] subtracts.
    /// Read only under [`enabled`].
    pub(crate) fn component_snapshot() -> [u64; 8] {
        [
            &FACTCONE_NANOS,
            &COVERAGE_NANOS,
            &BFS_WALK_NANOS,
            &BFS_FOLD_NANOS,
            &SINGLETON_NANOS,
            &DERIVED_NANOS,
            &EMIT_NANOS,
            &PD_SOLVE_NANOS,
        ]
        .map(|c| c.load(Ordering::Relaxed))
    }

    pub(crate) fn enabled() -> bool {
        static E: OnceLock<bool> = OnceLock::new();
        *E.get_or_init(|| std::env::var("ALSEM_CONES_CENSUS").as_deref() == Ok("1"))
    }

    pub(crate) fn add(c: &AtomicU64, v: u64) {
        c.fetch_add(v, Ordering::Relaxed);
    }

    /// `add`, but only when the census is on. For counters on paths that run
    /// millions of times per run (`merge_cone`: 6,556,465), an unconditional
    /// `fetch_add` is not free — it inflated `compose` from ~11.0 s to ~12.8 s,
    /// i.e. the probe was distorting the very number it reported.
    pub(crate) fn add_gated(c: &AtomicU64, v: u64) {
        if enabled() {
            c.fetch_add(v, Ordering::Relaxed);
        }
    }

    pub(crate) fn max(c: &AtomicU64, v: u64) {
        c.fetch_max(v, Ordering::Relaxed);
    }

    /// A timer ONLY when the census is enabled. With it off this is a bool load
    /// and a `None`, so the ~1 M `Instant::now()` pairs these probe sites would
    /// otherwise take per BC Base App run — three per routine plus two per SCC —
    /// are not taken at all. A diagnostic that costs production time is a
    /// diagnostic that eventually gets deleted for the wrong reason.
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
        let d = DIRECT_NANOS.load(Ordering::Relaxed);
        let w = WALK_NANOS.load(Ordering::Relaxed);
        let a = ASSEMBLE_NANOS.load(Ordering::Relaxed);
        let total = d + w + a;
        let ms = |n: u64| n as f64 / 1e6;
        let pct = |n: u64| {
            if total == 0 {
                0.0
            } else {
                100.0 * n as f64 / total as f64
            }
        };
        eprintln!(
            "[cones-census] routines={} direct_facts={} phases_total={:.1}ms",
            ROUTINES.load(Ordering::Relaxed),
            DIRECT_FACTS.load(Ordering::Relaxed),
            ms(total),
        );
        eprintln!(
            "[cones-census] direct={:.1}ms ({:.1}%) walk={:.1}ms ({:.1}%) assemble={:.1}ms ({:.1}%)",
            ms(d),
            pct(d),
            ms(w),
            pct(w),
            ms(a),
            pct(a),
        );
        eprintln!(
            "[cones-census]   walk: graph={:.1}ms scc={:.1}ms dedup={:.1}ms compose={:.1}ms",
            ms(WALK_GRAPH_NANOS.load(Ordering::Relaxed)),
            ms(WALK_SCC_NANOS.load(Ordering::Relaxed)),
            ms(WALK_DEDUP_NANOS.load(Ordering::Relaxed)),
            ms(WALK_COMPOSE_NANOS.load(Ordering::Relaxed)),
        );
        eprintln!(
            "[cones-census]   compose: bfs_walk={:.1}ms bfs_fold={:.1}ms x{} picks={} singleton={:.1}ms x{} max_scc_members={}",
            ms(BFS_WALK_NANOS.load(Ordering::Relaxed)),
            ms(BFS_FOLD_NANOS.load(Ordering::Relaxed)),
            BFS_CALLS.load(Ordering::Relaxed),
            BFS_PICKS.load(Ordering::Relaxed),
            ms(SINGLETON_NANOS.load(Ordering::Relaxed)),
            SINGLETON_CALLS.load(Ordering::Relaxed),
            MAX_SCC_MEMBERS.load(Ordering::Relaxed),
        );
        let h = CONE_SCC_SIZE_HIST
            .each_ref()
            .map(|c| c.load(Ordering::Relaxed));
        eprintln!(
            "[cones-census]   cone_scc: recursive={} members_all={} members_emitted={} roots={} self_loops={} max={} sizes=1:{} 2:{} 3-9:{} 10-99:{} 100+:{} member_walks={} own_entries={} sets_borrowed={} sets_built={} set_entries={} walk_total={:.1}ms own_entry_members={} with_successor={} with_guarded_successor={}",
            CONE_SCC_RECURSIVE.load(Ordering::Relaxed),
            CONE_SCC_MEMBERS_ALL.load(Ordering::Relaxed),
            CONE_SCC_MEMBERS_EMITTED.load(Ordering::Relaxed),
            CONE_SCC_ROOTS.load(Ordering::Relaxed),
            CONE_SCC_SELF_LOOPS.load(Ordering::Relaxed),
            CONE_SCC_MAX.load(Ordering::Relaxed),
            h[0],
            h[1],
            h[2],
            h[3],
            h[4],
            CONE_SCC_MEMBER_WALKS.load(Ordering::Relaxed),
            CONE_SCC_OWN_ENTRIES.load(Ordering::Relaxed),
            CONE_SCC_SETS_BORROWED.load(Ordering::Relaxed),
            CONE_SCC_SETS_BUILT.load(Ordering::Relaxed),
            CONE_SCC_SET_ENTRIES.load(Ordering::Relaxed),
            ms(WALK_TOTAL_NANOS.load(Ordering::Relaxed)),
            CONE_SCC_OWN_ENTRY_MEMBERS.load(Ordering::Relaxed),
            CONE_SCC_WITH_SUCCESSOR.load(Ordering::Relaxed),
            CONE_SCC_WITH_GUARDED_SUCCESSOR.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   singleton: subst={:.1}ms scan_cone={:.1}ms scan_own={:.1}ms memo={:.1}ms row={:.1}ms open={:.1}ms own={:.1}ms raw={:.1}ms",
            ms(SGL_SUBST_NANOS.load(Ordering::Relaxed)),
            ms(SGL_SCAN_CONE_NANOS.load(Ordering::Relaxed)),
            ms(SGL_SCAN_OWN_NANOS.load(Ordering::Relaxed)),
            ms(SGL_MEMO_NANOS.load(Ordering::Relaxed)),
            ms(SGL_ROW_NANOS.load(Ordering::Relaxed)),
            ms(SGL_OPEN_NANOS.load(Ordering::Relaxed)),
            ms(SGL_OWN_FOLD_NANOS.load(Ordering::Relaxed)),
            ms(SGL_RAW_NANOS.load(Ordering::Relaxed)),
        );
        // #160 step 7b: the scan populations exist only where a singleton
        // scanned `best` (the raw half); under `raw: WitnessScope::None` they would be zeros.
        if SINGLETON_SCANS.load(Ordering::Relaxed) > 0 {
            eprintln!(
                "[cones-census]   singleton calls: 0_cones={} 1_cone={} n_cones={} entries_under_n_cones={}",
                SGL_CALLS_0_CONES.load(Ordering::Relaxed),
                SGL_CALLS_1_CONE.load(Ordering::Relaxed),
                SGL_CALLS_N_CONES.load(Ordering::Relaxed),
                SGL_ENTRIES_N_CONES.load(Ordering::Relaxed),
            );
            eprintln!(
                "[cones-census]   singleton pop: edges={} closed_entries={} open_entries={} own_entries={} wins={} bests={}",
                SGL_EDGES.load(Ordering::Relaxed),
                SGL_CLOSED_ENTRIES.load(Ordering::Relaxed),
                SGL_OPEN_ENTRIES.load(Ordering::Relaxed),
                SGL_OWN_ENTRIES.load(Ordering::Relaxed),
                SGL_WINS.load(Ordering::Relaxed),
                SGL_BESTS.load(Ordering::Relaxed),
            );
        }
        eprintln!(
            "[cones-census]   closed: memo_builds={} memo_reads={} memo_entries={} scc_set_folds={} singleton_scans={} memo_live_max={}",
            CLOSED_MEMO_BUILDS.load(Ordering::Relaxed),
            CLOSED_MEMO_READS.load(Ordering::Relaxed),
            CLOSED_MEMO_ENTRIES.load(Ordering::Relaxed),
            SCC_SET_FOLDS.load(Ordering::Relaxed),
            SINGLETON_SCANS.load(Ordering::Relaxed),
            CLOSED_MEMO_LIVE_MAX.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   compose rest: coverage_cone={:.1}ms emit_record={:.1}ms",
            ms(COVERAGE_NANOS.load(Ordering::Relaxed)),
            ms(EMIT_NANOS.load(Ordering::Relaxed)),
        );
        eprintln!(
            "[cones-census]   compose rest: derived_fold={:.1}ms fact_cone={:.1}ms x{}",
            ms(DERIVED_NANOS.load(Ordering::Relaxed)),
            ms(FACTCONE_NANOS.load(Ordering::Relaxed)),
            FACTCONE_CALLS.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   compose untimed: rest={:.3}ms pd_solve={:.1}ms x{}",
            COMPOSE_REST_NANOS.load(Ordering::Relaxed) as f64 / 1e6,
            ms(PD_SOLVE_NANOS.load(Ordering::Relaxed)),
            PD_SOLVE_CALLS.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   merge_cone: calls={} tiebreaks={} (a tiebreak = 2x rep_key, each JSON-serializing)",
            MERGE_CALLS.load(Ordering::Relaxed),
            MERGE_TIEBREAKS.load(Ordering::Relaxed),
        );
        let arena = PD_SOLVE_BYTES_ARENA.load(Ordering::Relaxed);
        let working = PD_SOLVE_BYTES_WORKING.load(Ordering::Relaxed);
        let snapshot = PD_SNAPSHOT_BYTES.load(Ordering::Relaxed);
        eprintln!(
            "[cones-census]   pd_solve: producers={} states={} links={} entries={} edges={} arena={} working={} snapshot={} solve_bytes_old_equiv={} edge_unmapped={}",
            PD_PRODUCERS.load(Ordering::Relaxed),
            PD_STATES.load(Ordering::Relaxed),
            PD_LINKS.load(Ordering::Relaxed),
            PD_ENTRIES.load(Ordering::Relaxed),
            PD_EDGES.load(Ordering::Relaxed),
            arena,
            working,
            snapshot,
            arena + working + snapshot,
            PD_EDGE_UNMAPPED.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   digest.witness_dags={} nodes={} preds={}",
            WITNESS_DAGS.load(Ordering::Relaxed),
            WITNESS_DAG_NODES.load(Ordering::Relaxed),
            WITNESS_DAG_PREDS.load(Ordering::Relaxed),
        );
    }
}

/// Print the process-cumulative cone census once (spec #160 §4.1.7). One
/// OnceLock read when off. Called by `alsem`'s and `aldump`'s `main` after the
/// command returns, and by `perf_trace::maybe_exit_after` before its exit, so
/// every cone the process composed (the snapshot's included) reaches the line.
pub fn report_cones_census() {
    if cones_census::enabled() {
        cones_census::report();
    }
}

/// A borrowed view of the uncertainty substrate: the node → set map plus the
/// index its ids resolve against.
///
/// The two are meaningless apart — a `UncertaintySetId` names nothing without the
/// index that minted it — so they travel together rather than being threaded as
/// two parameters that a caller could mismatch. Walkers take this instead of the
/// whole [`DetectorContext`], which is why `path_walker` still has no dependency
/// on the context type.
#[derive(Clone, Copy)]
pub struct UncertaintyView<'a> {
    pub index: &'a UncertaintyIndex,
    pub by_node: &'a HashMap<String, UncertaintySetId>,
}

impl<'a> UncertaintyView<'a> {
    /// This node's interned uncertainty ids; EMPTY for a node with no entry.
    ///
    /// A node with no entry and a node carrying an empty set are deliberately
    /// indistinguishable here, exactly as they were when this was a
    /// `HashMap<String, Arc<[Uncertainty]>>` read through
    /// `get(..).is_some_and(|v| !v.is_empty())`. `ContextKey.unc` depends on that
    /// equivalence.
    pub fn ids_of(self, node_id: &str) -> &'a [UncertaintyId] {
        match self.by_node.get(node_id) {
            Some(&sid) => self.index.elements(sid),
            None => &[],
        }
    }

    /// The same set as values, resolved on demand.
    pub fn values_of(self, node_id: &str) -> impl Iterator<Item = &'a Uncertainty> + 'a {
        let index = self.index;
        self.ids_of(node_id).iter().map(move |id| index.value(*id))
    }

    /// `true` iff this node carries at least one uncertainty. O(1): a window
    /// length check, no map-value deref and no allocation.
    pub fn has_any(self, node_id: &str) -> bool {
        match self.by_node.get(node_id) {
            Some(&sid) => !self.index.is_empty_set(sid),
            None => false,
        }
    }
}

/// #160 step 2 (spec §4.4.2): the OWNED substrates every detector reads, built
/// once by [`build_detector_artifacts`] at the union of the runs' masks. A
/// [`DetectorContext`] borrows them (`Deref`), so `ctx.<field>` reads compile
/// unchanged.
pub struct DetectorArtifacts {
    /// The substrate bits this was built with (`registry::substrate`).
    pub demanded: u32,
    /// The view's `ExecRelation` Primary payload (`rel.primary()`, shared by
    /// pointer; #160 step 4): detectors read `edges_by_from`, … here (al-sem
    /// passed it as the detector's `graph` arg).
    pub graph: Arc<PrimaryGraph>,
    /// The view's `ExecRelation` Cone payload (`rel.cone()`, shared by
    /// pointer; #160 step 11b §4.4): d43 reads it.
    pub cone: Arc<Vec<TypedEdge>>,
    /// The raw L3 event graph (al-sem `model.eventGraph`). d12/d38 read its
    /// `events`/`edges`; the combined-graph build already constructs it, so it is
    /// captured here rather than recomputed.
    pub event_graph: EventGraph,
    pub reverse_call_graph: ReverseCallGraph,
    /// Trigger + event-subscriber roots — transaction-span boundaries.
    pub entry_points: BTreeSet<String>,
    pub transaction_spans: Vec<TransactionSpan>,
    /// Resolved CallEdges keyed by callsiteId (first edge per callsite wins).
    pub resolved_call_edge_by_callsite: HashMap<String, CallEdge>,
    /// Uncertainty edges grouped by source routine.
    pub uncertainty_edges_by_from:
        HashMap<String, Vec<crate::engine::l4::combined_graph::Uncertainty>>,
    /// Per-node merged uncertainty set the path-walker accumulates per branch.
    /// `uncertaintiesAt(node) = core_summary.uncertainties ∪
    /// uncertainty_edges_by_from.get(node)`, deduped+sorted by `uncertainty_key`.
    /// Mirrors al-sem `walkEvidence`'s `uncertaintiesAt` (path-walker.ts:103-106).
    /// The UNION ORDER is `[...fromSummary, ...fromEdges]` — the CORE
    /// `RoutineSummary.uncertainties` first, then the combined-graph edge
    /// uncertainties — matching al-sem exactly before the dedupe. Keyed by
    /// internal routine id; `walk_evidence` reads it via this exact field.
    ///
    /// **HASH-CONSED.** The value is an `Arc<[Uncertainty]>`, and every node whose
    /// deduped set is byte-equal to another node's SHARES one allocation (see
    /// [`UncertaintySetPool`]). This is not an optimisation of the reader surface —
    /// `Arc<[T]>` derefs to `&[T]`, so every consumer (`walk_evidence`, d1's
    /// `path_uncertainty_ids`, d2's `sub_summary_uncertainties`,
    /// `d1_reach::node_has_uncertainty`) reads it exactly as it read the `Vec`.
    /// It is a MEMORY fix: the L4 solver broadcasts an SCC's whole uncertainty union
    /// to every member (`db_effect_solver.rs`'s per-member `shared_vec.clone()`), so
    /// on BC Base App 8020 3,700,433 records across 27,037 nodes collapse to 507,437
    /// records in 10,112 distinct sets — 729.1 MiB in 7,428,267 allocations becomes
    /// **102.0 MiB in 1,025,350** (measured; see the CHANGELOG entry). (On a customer
    /// workspace the whole structure is ~2.8 MiB; this is a "survive BC Base App" fix,
    /// not a customer-workspace one.)
    ///
    /// Sharing is sound with no new invariant: nothing mutates a node's set after
    /// this map is built (the only `insert`s outside the two builders are
    /// `#[cfg(test)]`), and `dedupe_uncertainties` already makes each set canonical
    /// — deduped by `uncertainty_key`, emitted in byte-sorted key order — so two
    /// nodes with the same uncertainty set produce byte-identical slices. Sharing
    /// changes lifetime and aliasing, never content or order.
    pub uncertainties_by_node: HashMap<String, UncertaintySetId>,
    /// The identity substrate `uncertainties_by_node`'s ids name. Read through
    /// [`DetectorContext::uncertainty_view`] rather than directly — an id is
    /// meaningless without this index.
    pub uncertainties: UncertaintyIndex,
    /// Per-routine `FullRoutineSummary` (direct + inherited facts + coverage).
    pub summaries: HashMap<String, FullRoutineSummary>,
    /// ⟨C1⟩ The compact DERIVED capability-cone substrate — per-routine presence
    /// flags + interned table/event id-sets folded during the same cone walk that
    /// produces `summaries`, with zero `retag` clones. Every analyze-path consumer
    /// of the old `capability_facts_inherited` Vec reads only a derived predicate
    /// off it, so this row REPLACES that Vec outright: since C1 Task 3 no
    /// summary carries a raw Vec (see `FullRoutineSummary`). Parked here because the rows are `Range<u32>` windows into pools this store owns; a row alone
    /// is meaningless. EMPTY when the `SUMMARIES` substrate was not demanded.
    pub cone_derived: ConeDerivedStore,
    /// The shared event-flow indexes (publisher/subscriber lookup tables) the
    /// d43/d44/d45 event-flow detectors consume. al-sem builds this LAZILY
    /// (`ctx.getEventFlowIndexes()`, memoized); the Rust port builds it EAGERLY
    /// here — deterministic, one pass over `event_graph.events`/`.edges`, matching
    /// how `event_graph`/`transaction_spans` are already eager.
    pub event_flow_indexes: EventFlowIndexes,
    /// The CORE `RoutineSummary.parameter_roles` (`RecordRoleSummary[]`) per
    /// routine, keyed by internal RoutineId. al-sem detectors read this as
    /// `routine.summary.parameterRoles`; the Rust `FullRoutineSummary`
    /// (`ctx.summaries`) DROPPED parameter_roles, so d37/d39 read them here.
    /// Harvested from the SAME recomputed core summaries the `uncertainties_by_node`
    /// harvest uses — NOT recomputed. Absent ⇒ no record-parameter roles.
    pub parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>>,
    /// The post-upgrade argument bindings per callsite (the resolver's
    /// `upgradeBindings` side table). The L3 `PCallArgumentBinding` carries the
    /// SOURCE-side fields (sourceKind / sourceVariableName / sourceRecordVariableId
    /// / callerSourceParameterIsVar / argumentAnchor / parameterIndex), but NOT the
    /// upgraded `bindingResolution` / `calleeParameterIsVar` — those live here,
    /// index-aligned with `call_site.argument_bindings`. d37/d39 join the two by
    /// position to read `binding.bindingResolution` / `binding.calleeParameterIsVar`.
    pub upgraded_bindings_by_callsite: HashMap<String, Vec<UpgradedBinding>>,
    /// The D14 forward-reachability root set — entry points (trigger /
    /// event-subscriber) PLUS the procedures al-sem cannot prove app-scoped
    /// (non-`local`; `internal` only when `internal_reachable_externally`). Built
    /// by `entry_points::find_reachable_roots` over the `access_modifiers` map
    /// harvested from `ModelRoutine.access_modifier`. Sorted; d14 BFS-seeds from it.
    pub reachable_roots: BTreeSet<String>,
    /// al-sem `(model.identity.primaryInternalsVisibleTo?.length ?? 0) > 0` — true
    /// when some other app is granted `internal` access (so `internal` procedures
    /// stay external API surface and are NOT flaggable as dead).
    ///
    /// DEFAULTS to `false`: the Rust model does NOT carry `primaryInternalsVisibleTo`
    /// and the source-only fixtures never set `internalsVisibleTo`. This is the
    /// source-only common case (no granted consumer ⇒ `internal` is app-scoped ⇒
    /// flaggable).
    /// TODO(R4-G+): if any fixture ever sets `internalsVisibleTo`, forward
    /// `primaryInternalsVisibleTo` from the L3 identity and replace this default.
    pub internal_reachable_externally: bool,
    /// R4-F root classifications (`model.rootClassifications`), keyed by INTERNAL
    /// RoutineId — d50/d51 look these up exactly like al-sem's
    /// `model.rootClassifications.find(r => r.routineId === routine.id)`. Carried
    /// verbatim from the resolved workspace (AST classifier + roots.config
    /// overlay). Empty when the resolve path produced no classifications.
    pub root_classifications_by_routine:
        HashMap<String, crate::engine::root_classification::RootClassification>,
    /// The routines whose app_guid ∈ the fetched dependency set (cross-app runs).
    /// `roleOf(caller)` = `dep_routine_ids.contains(caller.id) ? "dependency" :
    /// "primary"` — the d13/d16/d17 cross-app gate. EMPTY for source-only runs (every
    /// routine primary), matching al-sem's source-only `analysisRole` default.
    pub dep_routine_ids: BTreeSet<String>,
    /// The DECLARED workspace dependencies (`model.identity.primaryDependencies`),
    /// `{appGuid, name, minVersion}` per the primary app.json `dependencies[]`. d17
    /// iterates these. EMPTY for source-only runs (no deps declared / read).
    pub declared_dependencies: Vec<DeclaredDep>,
    /// Resolved dependency `.app` versions keyed by appGuid (`model.apps[].version`).
    /// d17 looks up the resolved version to compare against the declared minVersion.
    /// EMPTY for source-only runs (no dep .app parsed).
    pub app_versions: HashMap<String, String>,
    /// `internalsVisibleTo`, lower-case app guids: exposing app -> its friend apps
    /// (engine-switch S8.5). Empty single-app.
    pub friends: HashMap<String, BTreeSet<String>>,
    /// G-19 — the closed-world proven-temp `(routineId, paramIndex)` set: a
    /// keyword-less by-var record param of a `local` procedure ALL of whose
    /// resolved callers (and the routine's complete, fully-resolved same-object
    /// call surface) prove a `Known(true)` temporary argument. The d1/d3/d10
    /// temp gates treat such a param exactly like a `Known(true)` temp record.
    /// Built by `closed_world_temp::prove_closed_world_temp_params`; EVERY
    /// uncertainty fails the proof (the firing direction) — see module docs.
    pub closed_world_temp_params: crate::engine::l5::closed_world_temp::ClosedWorldTempParams,
    /// L4 summarize-stage diagnostics — presently just the roles-fixpoint
    /// cap-hit raised by `summary_runner::run_one_scc_roles` (the roles-only
    /// fixpoint that replaced the retired Jacobi `run_one_scc` at `b4181d8`).
    /// Cloned from `L4Analysis::summarize_diagnostics`, the SAME `build_l4` solve
    /// whose core summaries fill `uncertainties_by_node`/`parameter_roles_by_routine`
    /// — not recomputed.
    /// Empty for every workspace whose SCCs converge, which is the overwhelming
    /// common case (additive: `run_detectors` folds this into the "summarize"
    /// slot of the analyze/detect diagnostics envelope).
    pub summarize_diagnostics: Vec<crate::engine::l4::summary_runner::SummarizeDiagnostic>,
    /// ⟨Task 6⟩ The [`ReverseEffectIndex`] transpose over the solve's compact
    /// db-effect store (`L4Analysis::bundle`, which [`build_detector_artifacts`]
    /// returns beside these artifacts) — the routine <-> table/effect inverted
    /// index a db-effect-reading detector or the planned LSP hover would query.
    ///
    /// `Some` **only** when `demanded` carried
    /// [`substrate::DB_EFFECT_REVERSE_INDEX`], which is deliberately outside
    /// [`substrate::ALL`] and which no detector may declare (see that
    /// constant's doc for the `gap_detector_substrate_parity` trap and the
    /// one-line unlock). Left `None` costs a run exactly one `Option` field: no
    /// allocation, no transpose pass, no perf span.
    ///
    /// Today's shipped consumer — `alsem query` — does NOT come through here at
    /// all; it owns its own pipeline (`l4::effect_query_cli`), so it cannot
    /// charge `analyze` even accidentally. This field is the seam for the
    /// eventual in-context consumer.
    pub reverse_effect_index: Option<ReverseEffectIndex>,
    /// event id → cross-extension subscriber routine ids (subscribers living in a
    /// DIFFERENT app than the publisher object). Previously rebuilt identically by
    /// d43, d44 AND d45 each run; built once here (same sharing pattern as
    /// `event_flow_indexes`).
    pub cross_extension_subscribers: std::collections::BTreeMap<String, Vec<String>>,
}

impl DetectorArtifacts {
    /// Test-only: intern `set` and attach it to `node`.
    #[cfg(test)]
    pub(crate) fn set_uncertainties_for_test(&mut self, node: &str, set: Vec<Uncertainty>) {
        let sid = self.uncertainties.intern_set(set);
        self.uncertainties_by_node.insert(node.to_string(), sid);
    }

    /// The uncertainty substrate as one borrowed view — the id map and the index
    /// its ids resolve against, which are meaningless apart.
    pub fn uncertainty_view(&self) -> UncertaintyView<'_> {
        UncertaintyView {
            index: &self.uncertainties,
            by_node: &self.uncertainties_by_node,
        }
    }
}

/// The ordering facts (R4-F Stage-5b), keyed by `StableRoutineId`.
pub type OrderingFactsMap = HashMap<String, crate::engine::l5::ordering_facts::OrderingFacts>;

/// Shared context threaded into every detector: the owned [`DetectorArtifacts`]
/// (through `Deref`), the filled ordering slot, and the maps that borrow the
/// model's rows. Cheap to build; one per command.
pub struct DetectorContext<'v> {
    pub art: &'v DetectorArtifacts,
    /// The L4.5 ordering facts d47/d49/d51 consume. `Some` only when the
    /// builder demanded `substrate::ORDERING_FACTS` and filled it BEFORE the
    /// parallel detector loop (filling inside it can deadlock, see that bit).
    pub ordering: Option<&'v OrderingFactsMap>,
    pub routine_by_id: HashMap<&'v str, &'v ModelRoutine>,
    pub objects_by_id: HashMap<&'v str, &'v ModelObject>,
    pub table_by_id: HashMap<&'v str, &'v ModelTable>,
    /// Every call site indexed by id.
    pub call_site_by_id: HashMap<&'v str, &'v PCallSite>,
    /// The shared finding-fingerprint index (routine/object id maps + the
    /// internal→stable routine-id substitution map). Built ONCE per run —
    /// previously every detector rebuilt it (54 × ~2 String clones per routine).
    pub fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex<'v>,
}

impl std::ops::Deref for DetectorContext<'_> {
    type Target = DetectorArtifacts;
    fn deref(&self) -> &DetectorArtifacts {
        self.art
    }
}

impl<'v> DetectorContext<'v> {
    /// The borrowed maps over the rows given. Takes slices, not a `Model`, so
    /// a test with routines alone can build one.
    pub fn new(
        routines: &'v [ModelRoutine],
        objects: &'v [ModelObject],
        tables: &'v [ModelTable],
        art: &'v DetectorArtifacts,
        ordering: Option<&'v OrderingFactsMap>,
    ) -> Self {
        let _s = pt::span("context", "context.final_indexes");
        // #160 step 11b §4.6: the routine lookup is the projection's class P
        // pick (the LAST row of the id).
        let routine_by_id = projection::last_per_id_borrowed(
            &projection::RowsOfId::build(routines),
            routines.iter().map(Some).collect(),
        );
        let objects_by_id = objects.iter().map(|o| (o.id.as_str(), o)).collect();
        // G-5: REAL table wins an id collision with a tableextension stub (the stub's
        // id reuses the extension's own object number) — otherwise rootCause text
        // renders the EXTENSION's name for ops on the real table.
        let table_by_id = crate::program::model::workspace::table_by_id_preferring_real(tables);
        // Class P per call-site id: the LAST owning row's site (a collided id's
        // rows share site ids).
        let call_site_by_id: HashMap<&str, &PCallSite> =
            projection::last_per_key(routines.iter().flat_map(|r| &r.call_sites), |cs| {
                cs.id.as_str()
            });
        let fingerprint_index =
            crate::engine::l5::fingerprint::FingerprintIndex::build(routines, objects);
        let ctx = DetectorContext {
            art,
            ordering,
            routine_by_id,
            objects_by_id,
            table_by_id,
            call_site_by_id,
            fingerprint_index,
        };
        // #160 §4.1.5: the borrowed maps print as `context.*` here, under the
        // same names as step 0, so parent and head line up one to one.
        if crate::engine::census::enabled() {
            crate::census_hook::mark("census.detector_context");
            census_borrowed_maps(&ctx);
        }
        ctx
    }

    /// The L4.5 ordering facts, keyed by `StableRoutineId`. d47/d49/d51 look up
    /// their reportable routine's facts here exactly as al-sem's
    /// `ctx.getOrderingFacts()`. The slot is filled before the detectors run;
    /// a detector that reads it without declaring `substrate::ORDERING_FACTS`
    /// panics here, in every build.
    pub fn get_ordering_facts(&self) -> &'v OrderingFactsMap {
        self.ordering
            .expect("ordering cell not filled: declare substrate::ORDERING_FACTS")
    }
}

/// The context over `model`'s rows.
pub fn detector_context<'v>(
    model: &'v Model,
    art: &'v DetectorArtifacts,
    ordering: Option<&'v OrderingFactsMap>,
) -> DetectorContext<'v> {
    let ws = &model.workspace;
    DetectorContext::new(&ws.routines, &ws.objects, &ws.tables, art, ordering)
}

/// The ordering facts of `model`, computed on the big-stack pool (the fill
/// recurses over statement trees, `compute_return_summaries`). Call it BEFORE
/// the parallel detector loop, never inside it (`substrate::ORDERING_FACTS`).
pub(crate) fn fill_ordering(model: &Model) -> OrderingFactsMap {
    assert!(
        !crate::engine::l5::registry::in_detector_loop(),
        "the ordering slot is filled before the detector loop, never inside it"
    );
    let _s = pt::span("context", "context.ordering_facts");
    let facts = crate::big_stack::big_stack_pool()
        .install(|| crate::engine::l5::ordering_facts::compute_ordering_facts(model));
    // #160 §4.1.5: the cell's bucket is printed at the fill.
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.ordering_facts");
        census_ordering_facts(&facts);
    }
    facts
}

/// Owner for the shims below (tests and `aldump`; no `alsem` command): a
/// [`DetectorContext`] borrows its artifacts, so a function that builds them
/// cannot also return the context. Call sites do
/// `let owned = build_detector_context(..); let ctx = owned.ctx();`.
pub struct OwnedContext<'m> {
    model: &'m Model,
    art: DetectorArtifacts,
    ordering: Option<OrderingFactsMap>,
    /// The solve the shim ran (`Some` iff the mask held `CORE_SUMMARIES`).
    l4: Option<L4Analysis>,
}

impl OwnedContext<'_> {
    pub fn ctx(&self) -> DetectorContext<'_> {
        detector_context(self.model, &self.art, self.ordering.as_ref())
    }

    /// The `L4Analysis` this shim built, if any.
    pub fn l4(&self) -> Option<&L4Analysis> {
        self.l4.as_ref()
    }
}

/// Build the shared context. Runs the L4 substrate over the model's calls and
/// event graph (combined_graph → cone) to assemble the combined
/// graph + the always-built eager indexes, then builds ONLY the expensive substrates
/// named in `demanded` (see `registry::substrate`).
///
/// `demanded` is the union of every selected detector's `requires` bits (folded by
/// `run_detectors`). Skipped substrates leave their ctx fields EMPTY — the field types
/// are unchanged. Passing `substrate::ALL` reproduces the pre-W1.0 eager build exactly
/// (byte-identical context), which every non-registry caller does. The four gated
/// substrates are:
///   - `SUMMARIES` — capability cones + `summaries` (also built for `TRANSACTION_SPANS`,
///     which folds over the summaries map internally).
///   - `CORE_SUMMARIES` — the closed-form v2 core summaries over the relation's SCC
///     (`db_effects`/`uncertainties` via `compute_summaries_v2`, `parameter_roles`
///     via its own JACOBI-disciplined roles-only fixpoint) →
///     `uncertainties_by_node` / `parameter_roles_by_routine` / `summarize_diagnostics`.
///   - `TRANSACTION_SPANS` — `transaction_spans`.
///   - `CLOSED_WORLD_TEMP` — `closed_world_temp_params`.
///
/// ⟨C1 Task 3⟩ The cone composes with no raw half: the per-routine raw
/// `Vec<CapabilityFact>` is never allocated (it cost ~10.9 GB on the 8020
/// corpus). Raw facts come only from a `View`'s witness request (#160 step 6).
///
/// #160 step 2: a SHIM (tests and `aldump`; no `alsem` command) over
/// [`compose_detector_artifacts`]. It fills the ordering slot FIRST when the mask
/// has `ORDERING_FACTS`, builds the relation, and runs the solve iff the mask has
/// `CORE_SUMMARIES`.
pub fn build_detector_context(resolved: &Model, demanded: u32) -> OwnedContext<'_> {
    build_detector_context_with(resolved, demanded, None)
}

/// What a CROSS-APP context adds to the model (engine-switch S8.1): the dependency
/// routines (never primary roots or entry points), their fixed solver leaves, and
/// d17's declared dependencies and resolved versions. The model is the cross-app
/// base's (`R3a5CrossAppBase::resolved`), its calls cut to workspace callers. The
/// dependency-internal edges the cone follows are the Cone payload of the base's
/// relation (`R3a5CrossAppBase::rel`), which a cross-app context takes.
pub(crate) struct CrossAppInputs<'b> {
    pub dep_routine_ids: &'b BTreeSet<String>,
    pub leaf_summaries: &'b HashMap<String, crate::engine::l4::summary::RoutineSummary>,
    /// Dependency publishers solved for rows only (`R3a5CrossAppBase::fixed_roles`).
    pub fixed_roles: &'b std::collections::HashSet<String>,
    pub declared_dependencies: &'b [crate::program::model::workspace::DeclaredDependencyDecl],
    pub app_versions: &'b HashMap<String, String>,
    pub friends: &'b HashMap<String, BTreeSet<String>>,
    /// The base's solver field index: `build_l4` reads it instead of
    /// rebuilding it.
    pub field_index: &'b crate::engine::l4::summary_runner::FieldIndex,
}

/// [`build_detector_context`], single-app (`base: None`) or cross-app (`resolved`
/// is `base.resolved`; the base's relation and cross-app inputs). One builder for
/// both since engine-switch S8.1; the cross-app one (which built every
/// substrate whatever the detectors asked for) is gone.
pub(crate) fn build_detector_context_with<'a>(
    resolved: &'a Model,
    demanded: u32,
    base: Option<&crate::engine::l4::capability_cone::R3a5CrossAppBase>,
) -> OwnedContext<'a> {
    use crate::engine::l5::registry::substrate;
    let ordering = (demanded & substrate::ORDERING_FACTS != 0).then(|| fill_ordering(resolved));
    // Cross-app the base's relation (its SCC included, no second Tarjan);
    // single-app one built here over every call.
    let rel = match base {
        Some(b) => Arc::clone(&b.rel),
        None => Arc::new(ExecRelation::single_app(resolved)),
    };
    let l4 = (demanded & substrate::CORE_SUMMARIES != 0).then(|| L4Request {
        extra_diagnostics: vec![],
    });
    let cross = base.map(|b| b.cross_app_inputs());
    let (art, l4) = compose_detector_artifacts(resolved, demanded, &rel, l4, cross);
    OwnedContext {
        model: resolved,
        art,
        ordering,
        l4,
    }
}

/// The det cone's inputs over `resolved` (#160 step 7a §5.8): the routine
/// universe, every routine's raw direct facts and its direct coverage. The
/// cone's typed edges are `rel.cone()` and its routines
/// `resolved.workspace.routines`.
pub(crate) struct DetConeInputs {
    pub nodes: Vec<String>,
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,
    pub coverage_in: HashMap<String, (String, Vec<String>)>,
}

/// Builds [`DetConeInputs`]: the one assembly [`build_detector_artifacts`]
/// and the step 7a oracle both compose over. Each row's direct coverage folds
/// the call-site-local uncertainty of both halves of `rel`
/// ([`ExecRelation::uncertainty_all`]; #160 step 5), the same fold, per row,
/// as the cross-app base. The r3a3 projection and the snapshot base fold their
/// own graph's edges; cross-app that is the cut graph (Primary callers only)
/// until step 6 (spec 5-A3).
pub(crate) fn det_cone_inputs(resolved: &Model, rel: &ExecRelation) -> DetConeInputs {
    det_cone_inputs_over(resolved, rel.uncertainty_all())
}

/// [`det_cone_inputs`] over any uncertainty edges (#160 step 6 §4.4): the
/// standalone single-app composition passes its own graph's, which single-app
/// are the relation's.
pub(crate) fn det_cone_inputs_over<'e>(
    resolved: &Model,
    uncertainty: impl IntoIterator<Item = &'e crate::engine::l4::combined_graph::UncertaintyEdge>,
) -> DetConeInputs {
    let ws = &resolved.workspace;
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &resolved.events.graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    // `direct=` times the per-routine extraction only, as at step 0.
    let t_direct = cones_census::start();
    // #160 step 11b §4.6: per row, then the projection (R2a): a collided id's
    // direct facts are the full-value union, its coverage the least complete.
    let mut facts_per_row = Vec::with_capacity(ws.routines.len());
    let mut coverage_per_row = Vec::with_capacity(ws.routines.len());
    let uncertainty_reasons = callsite_local_coverage_reasons(uncertainty);
    for r in &ws.routines {
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, status, reasons) = direct_facts_for_routine(r, pubs);
        let coverage =
            fold_uncertainty_reasons(status, reasons, uncertainty_reasons.get(r.id.as_str()));
        coverage_per_row.push(Some(coverage));
        facts_per_row.push(Some(facts));
    }
    let rows = projection::RowsOfId::build(&ws.routines);
    let coverage_in = projection::merge_per_id(&rows, coverage_per_row, projection::least_complete);
    let direct_full = projection::merge_per_id(&rows, facts_per_row, |v| {
        projection::union_by_value(v, |_| ())
    });
    cones_census::add_since(&cones_census::DIRECT_NANOS, t_direct);
    DetConeInputs {
        nodes,
        direct_full,
        coverage_in,
    }
}

/// What ONE composition over the view leaves for its readers (#160 step 6
/// D2): every routine's direct facts and coverage, the derived rows (empty
/// unless the scope derived them) and the producer arena, which the tree takes.
/// The subjects' raw facts are returned APART by [`compose_capabilities`].
pub(crate) struct CapabilityAnalysis {
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,
    pub coverage: HashMap<String, crate::engine::l4::capability_cone::CoverageRecord>,
    pub derived: ConeDerivedStore,
    pub pd: Option<crate::engine::l4::pd_solve::ConeArena>,
}

/// How many materialized raw entries belong to a non-primary routine (I5: must
/// be 0). An id is non-primary when any of its rows has `Role::Dependency`.
pub(crate) fn witness_non_primary_members(
    model: &Model,
    raw: &HashMap<String, Vec<CapabilityFact>>,
) -> usize {
    use crate::program::model::role::Role;
    let dependency: std::collections::HashSet<&str> = model
        .workspace
        .routines
        .iter()
        .filter(|r| model.routine_role(r) == Role::Dependency)
        .map(|r| r.id.as_str())
        .collect();
    raw.keys()
        .filter(|id| dependency.contains(id.as_str()))
        .count()
}

/// The one composition (#160 step 6 D1, D2): `compose_cone_over_graph` over
/// `typed` with `inputs` (from [`det_cone_inputs`]) and `scope`. Returns the
/// analysis and the raw facts of the members `scope.raw` contains, and prints
/// the `witness.*` census lines (what was materialized, by role).
pub(crate) fn compose_capabilities(
    model: &Model,
    typed: &[crate::engine::l4::combined_graph::TypedEdge],
    inputs: DetConeInputs,
    scope: ConeScope,
) -> (CapabilityAnalysis, HashMap<String, Vec<CapabilityFact>>) {
    let ws = &model.workspace;
    let DetConeInputs {
        nodes,
        direct_full,
        coverage_in,
    } = inputs;
    cones_census::add(&cones_census::ROUTINES, ws.routines.len() as u64);
    cones_census::add(
        &cones_census::DIRECT_FACTS,
        direct_full.values().map(|v| v.len() as u64).sum::<u64>(),
    );
    let _t_walk = cones_census::start();
    let outcome = compose_cone_over_graph(
        typed,
        &nodes,
        &direct_full,
        &coverage_in,
        &ws.routines,
        scope,
    );
    cones_census::add_since(&cones_census::WALK_NANOS, _t_walk);
    // ⟨C1 Task 4⟩ Both cone inputs are dead here (`nodes` is one more full
    // copy of every routine id, `coverage_in` one more copy of every routine's
    // direct status + reasons).
    drop(nodes);
    drop(coverage_in);
    if crate::engine::census::enabled() {
        let non_primary = witness_non_primary_members(model, &outcome.raw);
        // Bytes of the pass's `raw` map (#277 P-N9): key heap + each fact
        // vector's capacity + every fact's owned heap. Map slots not counted.
        let raw_bytes: u64 = outcome
            .raw
            .iter()
            .map(|(k, v)| {
                (k.len() + v.capacity() * std::mem::size_of::<CapabilityFact>()) as u64
                    + v.iter()
                        .map(crate::engine::l4::cone_census::capability_fact_heap_bytes)
                        .sum::<u64>()
            })
            .sum();
        eprintln!(
            "[retained-census] witness.members={} witness.non_primary_members={non_primary} witness.raw_bytes={raw_bytes}",
            outcome.raw.len()
        );
    }
    (
        CapabilityAnalysis {
            direct_full,
            coverage: outcome.coverage,
            derived: outcome.derived,
            pd: Some(outcome.pd_solution),
        },
        outcome.raw,
    )
}

/// The composing builder (#160 step 6 Task 2b-1, spec §14.5): ONE composition
/// over `rel.cone()`, then [`build_detector_artifacts`] over it. The test shim
/// composes here; a `View` composes in `prepare`.
pub(crate) fn compose_detector_artifacts(
    resolved: &Model,
    demanded: u32,
    rel: &ExecRelation,
    l4: Option<L4Request>,
    cross: Option<CrossAppInputs<'_>>,
) -> (DetectorArtifacts, Option<L4Analysis>) {
    use crate::engine::l5::registry::substrate;
    let need_summaries = demanded & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) != 0;
    // This composition repeats `View::prepare`'s (the scope, the span, the
    // inputs) for the shim, with no witness request.
    let caps = need_summaries.then(|| {
        let scope = ConeScope {
            derived: true,
            raw: WitnessScope::None,
        };
        let _s = pt::span("context", "context.capability_cones");
        compose_capabilities(resolved, rel.cone(), det_cone_inputs(resolved, rel), scope).0
    });
    build_detector_artifacts(resolved, demanded, rel, l4, cross, caps)
}

/// The owned detector substrates over the relation `rel`, at mask `demanded`:
/// the summaries from `caps`, the caller's ONE composition (#160 step 6 D2;
/// `Some` whenever the mask has `SUMMARIES` or `TRANSACTION_SPANS`), then
/// (when `l4` is `Some`) ONE [`build_l4`] solve over `rel`, then the harvest
/// of its core summaries. It composes nothing and never sees raw facts.
/// Returns the solve's [`L4Analysis`] beside the artifacts. `l4: None` = no
/// solve.
pub(crate) fn build_detector_artifacts(
    resolved: &Model,
    demanded: u32,
    rel: &ExecRelation,
    l4: Option<L4Request>,
    cross: Option<CrossAppInputs<'_>>,
    caps: Option<CapabilityAnalysis>,
) -> (DetectorArtifacts, Option<L4Analysis>) {
    // The Primary payload; `DetectorArtifacts.graph` shares it by pointer.
    let graph: Arc<PrimaryGraph> = Arc::clone(rel.primary());
    let cone = Arc::clone(rel.cone_shared());
    use crate::engine::l5::registry::substrate;
    assert!(
        demanded & substrate::CORE_SUMMARIES == 0 || l4.is_some(),
        "CORE_SUMMARIES demanded without an l4 request: the uncertainty/roles harvest would be silently empty"
    );
    pt::counter_delta("build.detector_context", 1);
    let ws = &resolved.workspace;
    let no_deps: BTreeSet<String> = BTreeSet::new();
    let dep_routine_ids = cross.as_ref().map_or(&no_deps, |c| c.dep_routine_ids);
    // TRANSACTION_SPANS folds over the summaries map, so demand summaries whenever
    // either bit is set (see `compute_transaction_spans`).
    let need_summaries = demanded & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) != 0;

    // --- L3→L4 substrate: the model's calls and events (the program engine's) ---
    let _symbols_span = pt::span("context", "context.symbols_resolve_calls");
    // Owned: the body below drains `calls.edges`/`upgraded_bindings` by move.
    let mut calls = (*resolved.calls).clone();
    drop(_symbols_span);
    let event_graph = resolved.events.graph.clone();

    // The summaries over the caller's composition (direct facts, coverage and
    // derived rows of every routine). SUBSTRATE-GATED: built only when some
    // selected detector demands SUMMARIES (or TRANSACTION_SPANS, which folds
    // over the summaries map). Skipped ⇒ empty `summaries` map.
    // Not `context.capability_cones` (final panel F1-C7): that span is the
    // composition's, opened by the caller (`View::prepare`, the shim above).
    let _cones_span = pt::span("context", "context.summaries_assemble");
    // ⟨C1⟩ Assigned inside the block below (kept a separate binding so the whole
    // cone-assembly block stays at its original indentation).
    let mut cone_derived = ConeDerivedStore::default();
    let summaries: HashMap<String, FullRoutineSummary> = if need_summaries {
        let CapabilityAnalysis {
            mut direct_full,
            coverage: mut cone_coverage,
            derived,
            pd,
        } = caps.expect(
            "SUMMARIES/TRANSACTION_SPANS demanded without a composition: the caller composes \
             once and passes it (#160 step 6 D2)",
        );
        cone_derived = derived;
        // #69 D1.4: the detectors read the solve only through the cone; its
        // arena (a snapshot input) is dead here.
        drop(pd);

        // `cone_coverage` and `direct_full` are owned and dead after this
        // loop, so move their payloads into the summaries instead of cloning
        // them out.
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let _t_assemble = cones_census::start();
        for r in &ws.routines {
            let coverage = cone_coverage.remove(&r.id);
            let direct = direct_full.remove(&r.id);
            // One summary per id (#160 step 11b §4.6): `direct_full` is already
            // the projection's per-id input (a collided id's full-value union),
            // and the cone is per id, so a collided id's first row files it and
            // every later row of the id finds the drain empty and is skipped.
            // (A coverage of `None` is the fail-soft for an id the cone did not
            // walk.)
            let Some(direct) = direct else {
                continue;
            };
            // ⟨C1 Task 3⟩ No inherited half (R6): raw facts live in the view's
            // witness pass, never on a summary.
            summaries.insert(
                r.id.clone(),
                FullRoutineSummary::new(r.id.clone(), direct, coverage),
            );
        }
        cones_census::add_since(&cones_census::ASSEMBLE_NANOS, _t_assemble);
        summaries
    } else {
        HashMap::new()
    };
    drop(_cones_span);

    // --- Eager indexes -----------------------------------------------------
    let reverse_call_graph = build_reverse_call_graph(&graph);

    let entry_points: BTreeSet<String> =
        crate::engine::l5::entry_points::find_entry_points(&ws.routines, dep_routine_ids)
            .into_iter()
            .collect();

    // D14 reachable-roots wiring. Build the RoutineId → AccessModifier map from
    // `ModelRoutine.access_modifier` ("local"/"internal"/"protected"/None). al-sem maps
    // "local" → Local, "internal" → Internal, "protected"/None/anything-else →
    // Public (default-access). A routine with NO entry is treated as Public by
    // `find_reachable_roots`, so we only need to insert the non-Public cases — but we
    // insert all parsed modifiers explicitly for clarity.
    // #160 step 11b §4.6: per row, then the projection (#11, class P: the LAST
    // row of the id).
    let access_per_row = ws
        .routines
        .iter()
        .map(|r| {
            Some(match r.access_modifier.as_deref() {
                Some("local") => AccessModifier::Local,
                Some("internal") => AccessModifier::Internal,
                // "protected" / None / any other value → public (al-sem default-access).
                _ => AccessModifier::Public,
            })
        })
        .collect();
    let access_modifiers: HashMap<String, AccessModifier> =
        projection::last_per_id(&projection::RowsOfId::build(&ws.routines), access_per_row);
    // See `DetectorContext::internal_reachable_externally` doc: defaults to false
    // (the Rust model carries no `primaryInternalsVisibleTo`; source-only fixtures
    // never set `internalsVisibleTo`).
    let internal_reachable_externally = false;
    let reachable_roots: BTreeSet<String> = crate::engine::l5::entry_points::find_reachable_roots(
        &ws.routines,
        dep_routine_ids,
        &access_modifiers,
        internal_reachable_externally,
    )
    .into_iter()
    .collect();

    // G-19 — closed-world proven-temp params for `local` procedures (consumed
    // by the d1/d3/d10 temp gates). Pure lookup-table build over the routines +
    // combined graph + reverse graph; entry points are proof-disqualifying.
    // SUBSTRATE-GATED on CLOSED_WORLD_TEMP.
    let closed_world_temp_params = if demanded & substrate::CLOSED_WORLD_TEMP != 0 {
        crate::engine::l5::closed_world_temp::prove_closed_world_temp_params(
            &ws.routines,
            &graph,
            &reverse_call_graph,
            &entry_points,
        )
    } else {
        Default::default()
    };

    // Transaction spans — SUBSTRATE-GATED on TRANSACTION_SPANS (which also forced
    // `need_summaries`, so the `summaries` map above is populated here).
    let transaction_spans = {
        let _s = pt::span("context", "context.transaction_spans");
        if demanded & substrate::TRANSACTION_SPANS != 0 {
            compute_transaction_spans(
                &ws.routines,
                dep_routine_ids,
                &reverse_call_graph,
                &summaries,
                &cone_derived,
            )
        } else {
            Vec::new()
        }
    };

    // Event-flow indexes — built eagerly from the L3 event graph + routine set +
    // dep set (source-only ⇒ empty dep set ⇒ every routine primary). Consumes
    // `event_graph` by reference before it is moved into the struct.
    let event_flow_indexes = build_event_flow_indexes(&event_graph, &ws.routines, dep_routine_ids);

    // Cross-extension subscriber lookup, shared by d43/d44/d45 — previously each
    // rebuilt this identically from `ctx.event_graph` + `ws.objects` per run.
    let cross_extension_subscribers =
        crate::engine::l5::event_flow::build_cross_extension_subscribers(&event_graph, &ws.objects);

    // `calls.edges` is not read after this point (`calls.upgraded_bindings` still
    // is), so take the edges by value instead of cloning each retained one.
    let resolved_call_edge_by_callsite =
        first_resolved_edge_per_callsite(std::mem::take(&mut calls.edges), &ws.routines);

    let mut uncertainty_edges_by_from: HashMap<
        String,
        Vec<crate::engine::l4::combined_graph::Uncertainty>,
    > = HashMap::new();
    for ue in &graph.uncertainty_edges {
        uncertainty_edges_by_from
            .entry(ue.from.clone())
            .or_default()
            .push(ue.uncertainty.clone());
    }

    // --- Per-node uncertainty wiring (the path-walker source) --------------
    // al-sem `walkEvidence` computes
    //   uncertaintiesAt(node) = routine.summary.uncertainties ∪ uncertaintyEdgesByFrom.get(node)
    // The CORE `RoutineSummary.uncertainties` is dropped by `FullRoutineSummary`
    // (the cone path keeps only facts + coverage), so we recompute the core
    // summaries here from the SAME relation the cone used: the closed-form v2
    // solver (`compute_summaries_v2`) over its Primary payload and its SCC
    // (computed in `ExecRelation::new`, #160 step 4).
    // This is the only place that needs the core uncertainties; the union is
    // assembled once and exposed on `uncertainties_by_node`.
    //
    // The v2-solver pass — the most expensive substrate — is ONE
    // `build_l4` call, here, after the cone (#160 §4.4.1), when the caller asked
    // for it. Its core summaries (lean: their `db_effects` are empty, the rows
    // live in `l4.bundle`) are harvested below iff the mask has CORE_SUMMARIES.
    // Skipped ⇒ `uncertainties_by_node` / `parameter_roles_by_routine` /
    // `summarize_diagnostics` are all empty, which by decision (a) means a
    // substrate-skipping run emits no summarize cap-hit diagnostics.
    let (l4, core_summaries) = match l4.map(|req| build_l4(resolved, rel, req, cross.as_ref())) {
        Some((l4, core)) => (Some(l4), Some(core)),
        None => (None, None),
    };
    let summarize_diagnostics = match &l4 {
        Some(l4) if demanded & substrate::CORE_SUMMARIES != 0 => l4.summarize_diagnostics.clone(),
        _ => Vec::new(),
    };
    let (uncertainties_by_node, uncertainties, parameter_roles_by_routine): (
        HashMap<String, UncertaintySetId>,
        UncertaintyIndex,
        HashMap<String, Vec<RecordRoleSummary>>,
    ) = if let Some(CoreSummaries(mut core_summaries)) =
        core_summaries.filter(|_| demanded & substrate::CORE_SUMMARIES != 0)
    {
        // uncertaintiesAt(node) per routine: [...fromSummary, ...fromEdges], deduped.
        // Union ORDER mirrors al-sem `[...fromSummary, ...fromEdges]` — core summary
        // uncertainties FIRST, then the combined-graph edge uncertainties (converted
        // to the summary `Uncertainty` form). `dedupe_uncertainties` keeps LAST-WRITE-WINS
        // per key then sorts by key, matching al-sem's `dedupeUncertainties` (see that
        // function's own doc — a same-key `interface-open-world` divergence is the one
        // case keep-first vs. keep-last would differ, and keep-last is what both engines do).
        //
        // ONE pass now DRAINS `core_summaries` for BOTH harvests: the uncertainty
        // union (which used to borrow-and-clone here, then hand the map to a second
        // loop that drained it for roles) and `parameter_roles`. The clone was pure
        // waste — `core_summaries` is dead after these two harvests, so the summary's
        // uncertainty `Vec` can be MOVED into the union instead of deep-copied (on
        // 8020 the clone was of `s.uncertainties` alone, bounded above by the ≤3.7 M
        // records / ≤730 MiB of the summary∪edges union — no probe separates the
        // summary-side share of that from the edge-side share).
        //
        // Draining makes the loop id-SENSITIVE where the old borrowing form was not,
        // and internal routine ids are NOT unique (15 collision groups / 19 routines
        // survive the member discriminator on 8020 — preproc `#if` alternatives and
        // XMLport same-name elements; see `docs/OUTSTANDING.md`). Under a naive drain
        // the second routine of a colliding pair would find `None`, recompute an
        // edges-ONLY set, and overwrite the first routine's full union with that
        // strict subset. `processed` closes that: both harvests are pure functions of
        // `r.id` (`core_summaries[r.id]`, `uncertainty_edges_by_from[r.id]`), so
        // running each DISTINCT id exactly once is exactly what the old
        // recompute-and-overwrite form converged to — same keys, same values. Pinned
        // by `colliding_ids_keep_the_full_summary_union_not_just_the_edges`.
        let mut uncertainties_by_node: HashMap<String, UncertaintySetId> = HashMap::new();
        let mut parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>> =
            HashMap::new();
        let mut uncertainties = UncertaintyIndex::default();
        let mut processed: HashSet<&str> = HashSet::new();
        for r in &ws.routines {
            if !processed.insert(r.id.as_str()) {
                continue;
            }
            // Harvest the CORE parameter_roles per routine from the SAME recomputed
            // core summaries (d37/d39 read these as `routine.summary.parameterRoles`),
            // moved out with the uncertainties in this one `remove`.
            let (from_summary, roles) = match core_summaries.remove(&r.id) {
                Some(s) => (s.uncertainties, s.parameter_roles),
                None => (Vec::new(), Vec::new()),
            };
            if !roles.is_empty() {
                parameter_roles_by_routine.insert(r.id.clone(), roles);
            }
            let from_edges: Vec<Uncertainty> = uncertainty_edges_by_from
                .get(&r.id)
                .map(|edges| edges.iter().map(Uncertainty::from).collect())
                .unwrap_or_default();
            if from_summary.is_empty() && from_edges.is_empty() {
                continue;
            }
            let combined: Vec<Uncertainty> = from_summary.into_iter().chain(from_edges).collect();
            uncertainties_by_node.insert(
                r.id.clone(),
                uncertainties.intern_set(dedupe_uncertainties(combined)),
            );
        }
        // `parameter_roles_by_routine`'s membership was previously the WHOLE of
        // `core_summaries`, not `ws.routines` — so anything the loop above did not
        // reach still belongs in it. Empty in practice (the solver is driven by
        // `ws.routines`), but membership is preserved by construction rather than by
        // an assumption about the solver's key set.
        for (rid, s) in core_summaries {
            if !s.parameter_roles.is_empty() {
                parameter_roles_by_routine.insert(rid, s.parameter_roles);
            }
        }
        (
            uncertainties_by_node,
            uncertainties,
            parameter_roles_by_routine,
        )
    } else {
        (HashMap::new(), UncertaintyIndex::default(), HashMap::new())
    };

    // ⟨Task 6⟩ The reverse transpose — built ONLY on explicit demand. Not in
    // `substrate::ALL`, so an `ALL` caller (the parity tests, census-probe)
    // never pays it, and a run (which passes `demanded`, the fold of the selected
    // detectors' `requires`) can never reach it either, since no detector may
    // declare the bit. Unset ⇒ this is one `None` write: no allocation, no pass,
    // and — because the span is INSIDE the branch — not even a trace entry.
    // The named span is deliberate: when it IS built, its cost lands in the perf
    // trace beside every other substrate rather than being guessed at.
    let reverse_effect_index = if demanded & substrate::DB_EFFECT_REVERSE_INDEX != 0 {
        let _s = pt::span("context", "context.reverse_effect_index");
        l4.as_ref().map(|l4| ReverseEffectIndex::build(&l4.bundle))
    } else {
        None
    };

    // Expose the resolver's post-upgrade bindings (the `upgradeBindings` side
    // table) keyed by callsite id — the join target for d37/d39 which read
    // `binding.bindingResolution` / `binding.calleeParameterIsVar`. `build_l4` reads
    // `resolved.calls.upgraded_bindings`, not this local clone, and nothing else reads
    // the clone's bindings, so move them out here.
    let upgraded_bindings_by_callsite: HashMap<String, Vec<UpgradedBinding>> =
        std::mem::take(&mut calls.upgraded_bindings);

    // R4-F root classifications — keyed by internal RoutineId for d50/d51 lookup.
    let root_classifications_by_routine: HashMap<
        String,
        crate::engine::root_classification::RootClassification,
    > = crate::engine::root_classification::classification_per_id(
        &resolved.root_classifications,
        &resolved.workspace.routines,
        &resolved.workspace.objects,
    )
    .into_iter()
    .map(|(id, rc)| (id.to_string(), rc.clone()))
    .collect();

    // ⟨substrate census⟩ `ALSEM_UNCERTAINTY_CENSUS=1` — see `UncertaintyIndex::emit_census`.
    uncertainties.emit_census(&uncertainties_by_node);

    let art = DetectorArtifacts {
        demanded,
        graph,
        cone,
        event_graph,
        reverse_call_graph,
        entry_points,
        transaction_spans,
        resolved_call_edge_by_callsite,
        uncertainty_edges_by_from,
        uncertainties_by_node,
        uncertainties,
        summaries,
        cone_derived,
        event_flow_indexes,
        parameter_roles_by_routine,
        upgraded_bindings_by_callsite,
        reachable_roots,
        internal_reachable_externally,
        dep_routine_ids: dep_routine_ids.clone(),
        declared_dependencies: cross
            .as_ref()
            .map(|c| {
                c.declared_dependencies
                    .iter()
                    .map(|d| DeclaredDep {
                        app_guid: d.app_guid.clone(),
                        name: d.name.clone(),
                        min_version: d.min_version.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        app_versions: cross
            .as_ref()
            .map(|c| c.app_versions.clone())
            .unwrap_or_default(),
        friends: cross
            .as_ref()
            .map(|c| c.friends.clone())
            .unwrap_or_default(),
        root_classifications_by_routine,
        closed_world_temp_params,
        summarize_diagnostics,
        reverse_effect_index,
        cross_extension_subscribers,
    };
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.context");
        census_context(&art);
    }
    (art, l4)
}

/// #160 §4.1.5: one `context.<field>` bucket per heap-owning field of
/// [`DetectorArtifacts`] (it replaced the old `[C1_CONE_CENSUS]` report of
/// `summaries` + `cone_derived`). `summaries` uses the per-fact accounting
/// (exact); the rest is shallow. `graph` is counted by whoever built it
/// (#160 A4), the solve by `build_l4`'s `l4.*` buckets, and the borrowed maps
/// of a [`DetectorContext`] by [`census_borrowed_maps`] when it is built.
fn census_context(c: &DetectorArtifacts) {
    use crate::engine::census::{Kind, Tally, event_graph};
    use crate::engine::l4::combined_graph::{CombinedEdge, Uncertainty};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit("context.event_graph", &mut event_graph(&c.event_graph));
    emit(
        "context.reverse_call_graph",
        Tally::default()
            .buf::<(String, Vec<CombinedEdge>)>(c.reverse_call_graph.len())
            .strs(c.reverse_call_graph.keys())
            .bufs(c.reverse_call_graph.values()),
    );
    emit(
        "context.entry_points",
        Tally::default()
            .buf::<String>(c.entry_points.len())
            .strs(&c.entry_points),
    );
    emit(
        "context.transaction_spans",
        Tally::default().buf::<TransactionSpan>(c.transaction_spans.len()),
    );
    emit(
        "context.resolved_call_edge_by_callsite",
        Tally::default()
            .buf::<(String, CallEdge)>(c.resolved_call_edge_by_callsite.len())
            .strs(c.resolved_call_edge_by_callsite.keys()),
    );
    emit(
        "context.uncertainty_edges_by_from",
        Tally::default()
            .buf::<(String, Vec<Uncertainty>)>(c.uncertainty_edges_by_from.len())
            .strs(c.uncertainty_edges_by_from.keys())
            .bufs(c.uncertainty_edges_by_from.values()),
    );
    emit(
        "context.uncertainties_by_node",
        Tally::default()
            .buf::<(String, UncertaintySetId)>(c.uncertainties_by_node.len())
            .strs(c.uncertainties_by_node.keys()),
    );
    emit("context.uncertainties", &mut c.uncertainties.census_tally());
    crate::engine::l4::cone_census::summaries_tally(&c.summaries)
        .emit("context.summaries", Kind::Exact);
    let cd = c.cone_derived.census();
    emit(
        "context.cone_derived",
        &mut Tally {
            bytes: (cd.rows_key_heap_bytes
                + cd.rows_struct_bytes
                + cd.interner_heap_bytes
                + cd.writes_all_bytes
                + cd.phys_writes_bytes
                + cd.phys_reads_bytes
                + cd.events_bytes) as usize,
            allocs: cd.rows + cd.interner_strings,
        },
    );
    let f = &c.event_flow_indexes;
    let mut flow = Tally::default();
    flow.buf::<(String, String)>(f.publisher_by_event.len())
        .strs(f.publisher_by_event.iter().flat_map(|(k, v)| [k, v]))
        .buf::<(String, String)>(f.event_name_by_event.len())
        .strs(f.event_name_by_event.iter().flat_map(|(k, v)| [k, v]))
        .buf::<String>(f.primary_routines.len())
        .strs(&f.primary_routines);
    for m in [
        &f.events_by_publisher,
        &f.subscribers_by_event,
        &f.publishers_by_subscriber,
    ] {
        flow.buf::<(String, Vec<String>)>(m.len())
            .strs(m.keys())
            .bufs(m.values())
            .strs(m.values().flatten());
    }
    emit("context.event_flow_indexes", &mut flow);
    emit(
        "context.parameter_roles_by_routine",
        Tally::default()
            .buf::<(String, Vec<RecordRoleSummary>)>(c.parameter_roles_by_routine.len())
            .strs(c.parameter_roles_by_routine.keys())
            .bufs(c.parameter_roles_by_routine.values()),
    );
    emit(
        "context.upgraded_bindings_by_callsite",
        Tally::default()
            .buf::<(String, Vec<UpgradedBinding>)>(c.upgraded_bindings_by_callsite.len())
            .strs(c.upgraded_bindings_by_callsite.keys())
            .bufs(c.upgraded_bindings_by_callsite.values()),
    );
    emit(
        "context.reachable_roots",
        Tally::default()
            .buf::<String>(c.reachable_roots.len())
            .strs(&c.reachable_roots),
    );
    emit(
        "context.internal_reachable_externally",
        &mut Tally::default(),
    );
    emit(
        "context.root_classifications_by_routine",
        Tally::default()
            .buf::<(
                String,
                crate::engine::root_classification::RootClassification,
            )>(c.root_classifications_by_routine.len())
            .strs(c.root_classifications_by_routine.keys()),
    );
    emit(
        "context.dep_routine_ids",
        Tally::default()
            .buf::<String>(c.dep_routine_ids.len())
            .strs(&c.dep_routine_ids),
    );
    emit(
        "context.declared_dependencies",
        Tally::default().buf::<DeclaredDep>(c.declared_dependencies.len()),
    );
    emit(
        "context.app_versions",
        Tally::default()
            .buf::<(String, String)>(c.app_versions.len())
            .strs(c.app_versions.iter().flat_map(|(k, v)| [k, v])),
    );
    emit(
        "context.friends",
        Tally::default()
            .buf::<(String, BTreeSet<String>)>(c.friends.len())
            .strs(c.friends.keys())
            .strs(c.friends.values().flatten()),
    );
    // `context.ordering_facts` is printed when the slot is filled
    // (`fill_ordering`).
    emit(
        "context.closed_world_temp_params",
        Tally::default()
            .buf::<(String, u32)>(c.closed_world_temp_params.len())
            .strs(c.closed_world_temp_params.iter().map(|(s, _)| s)),
    );
    emit(
        "context.summarize_diagnostics",
        Tally::default().buf::<crate::engine::l4::summary_runner::SummarizeDiagnostic>(
            c.summarize_diagnostics.len(),
        ),
    );
    emit(
        "context.reverse_effect_index",
        &mut c
            .reverse_effect_index
            .as_ref()
            .map(ReverseEffectIndex::census_tally)
            .unwrap_or_default(),
    );
    emit(
        "context.cross_extension_subscribers",
        Tally::default()
            .buf::<(String, Vec<String>)>(c.cross_extension_subscribers.len())
            .strs(c.cross_extension_subscribers.keys())
            .bufs(c.cross_extension_subscribers.values())
            .strs(c.cross_extension_subscribers.values().flatten()),
    );
}

/// The borrowed maps of a [`DetectorContext`], shallow, under step 0's names.
fn census_borrowed_maps(c: &DetectorContext<'_>) {
    use crate::engine::census::{Kind, Tally};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit(
        "context.routine_by_id",
        Tally::default().buf::<(&str, &ModelRoutine)>(c.routine_by_id.len()),
    );
    emit(
        "context.objects_by_id",
        Tally::default().buf::<(&str, &ModelObject)>(c.objects_by_id.len()),
    );
    emit(
        "context.table_by_id",
        Tally::default().buf::<(&str, &ModelTable)>(c.table_by_id.len()),
    );
    emit(
        "context.call_site_by_id",
        Tally::default().buf::<(&str, &PCallSite)>(c.call_site_by_id.len()),
    );
    emit(
        "context.fingerprint_index",
        &mut c.fingerprint_index.census_tally(),
    );
}

/// The filled ordering cell's `context.ordering_facts` bucket, shallow.
fn census_ordering_facts(m: &HashMap<String, crate::engine::l5::ordering_facts::OrderingFacts>) {
    use crate::engine::census::{Kind, Tally};
    use crate::engine::l5::ordering_facts::OrderingFacts;
    let mut t = Tally::default();
    t.buf::<(String, OrderingFacts)>(m.len())
        .strs(m.keys())
        .strs(m.values().map(|f| &f.routine_id))
        .bufs(m.values().map(|f| &f.facts));
    t.emit("context.ordering_facts", Kind::Shallow);
}

/// `resolved_call_edge_by_callsite`: per call site, the FIRST edge that has a
/// target, in `calls.edges` order.
///
/// Deliberate detector policy (engine-switch S3.6 audit). The only call sites
/// with several targeted edges are interface dispatches with several workspace
/// implementers; the adapter emits those sorted by target id, as L3 did, so the
/// detectors that read this map (d40/d41/d42/d53/d55/d61 and the shared
/// by-var-argument helpers) analyse the implementer with the smallest id. They
/// read the callee's parameters and parameter roles; implementers share the
/// interface's signature, but not their bodies. Analysing every implementer is
/// a detector change, not a resolution one: see `docs/OUTSTANDING.md`.
///
/// #160 step 11b §4.6 (R9, class P): a collided id's call site takes its LAST
/// OWNING row's edges only (`CallEdge.from_ix`), so the edge comes from the row
/// whose bindings R5 holds; inside that row the first targeted edge, as above.
fn first_resolved_edge_per_callsite(
    edges: Vec<CallEdge>,
    routines: &[ModelRoutine],
) -> HashMap<String, CallEdge> {
    let owners = projection::site_owners(&projection::RowsOfId::build(routines), routines);
    projection::first_per_key(
        edges.into_iter().filter(|ce| {
            ce.to.is_some()
                && owners
                    .get(ce.callsite_id.as_str())
                    .is_none_or(|&o| o == ce.from_ix)
        }),
        |ce| ce.callsite_id.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::model::ix::RoutineIx;

    /// States a routine-id collision across the WHOLE model, by assignment: every
    /// routine named `name` gets the id `shared`, and so does every precomputed
    /// call and event edge end that named one of their old ids. The program-backed
    /// model resolves its calls and events once, at build time (the L3 builder
    /// re-resolved them from the routines on every read), so re-keying only the
    /// routines would leave their edges pointing at ids no routine carries.
    /// Returns how many routines were re-keyed.
    fn force_shared_id(resolved: &mut Model, name: &str, shared: &str) -> usize {
        let mut old = std::collections::HashSet::new();
        let mut forced = 0;
        for r in resolved.workspace.routines.iter_mut() {
            if r.name.eq_ignore_ascii_case(name) {
                old.insert(std::mem::replace(&mut r.id, shared.to_string()));
                forced += 1;
            }
        }
        let rekey = |id: &mut String| {
            if old.contains(id.as_str()) {
                *id = shared.to_string();
            }
        };
        for e in &mut std::sync::Arc::make_mut(&mut resolved.calls).edges {
            rekey(&mut e.from);
            e.to.as_mut().map(rekey);
            e.candidates.iter_mut().flatten().for_each(rekey);
        }
        let graph = &mut std::sync::Arc::make_mut(&mut resolved.events).graph;
        for e in &mut graph.edges {
            rekey(&mut e.subscriber_routine_id);
        }
        for s in &mut graph.events {
            s.publisher_routine_id.as_mut().map(rekey);
        }
        forced
    }

    /// S3.6 audit, precondition by assignment: one call site with two targeted
    /// edges (an interface site with two implementers) and a to-less one first.
    /// The map keeps the first TARGETED edge, never the to-less one or the last.
    #[test]
    fn the_first_targeted_edge_represents_a_call_site() {
        let mk = |to: Option<&str>| {
            let mut e = CallEdge::base(RoutineIx(0), "caller", "cs1", "op1");
            e.to = to.map(str::to_string);
            e
        };
        let map =
            first_resolved_edge_per_callsite(vec![mk(None), mk(Some("a")), mk(Some("b"))], &[]);
        assert_eq!(map.len(), 1);
        assert_eq!(map["cs1"].to.as_deref(), Some("a"));
    }

    /// The model's calls are USED, not just present: a hand-made `ResolvedCalls` on
    /// an empty workspace must show up in the detector context, the ordering-facts
    /// substrate (`compose_snapshot` → `build_r3a3_source_only_base`) and coverage.
    /// (Until engine-switch S9.6 the calls were optional; reverting a site to L3's
    /// own resolver failed its assertion.)
    #[test]
    fn model_calls_reach_detector_context_and_ordering_base() {
        use crate::program::model::calls::{CallEdge, ResolvedCalls};
        use crate::program::model::taxonomy::{DispatchKind, Resolution};
        let mut edge = CallEdge::base(RoutineIx(0), "from-r", "distinct-callsite", "distinct-op");
        edge.to = Some("to-r".to_string());
        // Dynamic so the coverage path (`dynamic_dispatch_sites`) shows it too.
        edge.dispatch_kind = DispatchKind::Dynamic;
        edge.resolution = Resolution::Resolved;
        let resolved = crate::program::model::workspace::Model {
            workspace: crate::program::model::workspace::ModelEntities {
                objects: Vec::new(),
                tables: Vec::new(),
                routines: Vec::new(),
            },
            root_classifications: Vec::new(),
            primary_app: None,
            infra_diagnostics: Vec::new(),
            workspace_diagnostics: Vec::new(),
            dependency_diagnostics: Vec::new(),
            calls: std::sync::Arc::new(ResolvedCalls {
                edges: vec![edge],
                upgraded_bindings: HashMap::new(),
                diagnostics: Vec::new(),
                external_targets: Vec::new(),
            }),
            events: Default::default(),
        };
        let owned = build_detector_context(&resolved, crate::engine::l5::registry::substrate::ALL);
        let ctx = owned.ctx();
        assert!(
            ctx.resolved_call_edge_by_callsite
                .contains_key("distinct-callsite"),
            "build_detector_context ignored the model's calls"
        );
        let base = crate::engine::l4::capability_cone::build_r3a3_source_only_base(&resolved);
        assert!(
            base.calls
                .edges
                .iter()
                .any(|e| e.callsite_id == "distinct-callsite"),
            "the ordering-facts base ignored the model's calls"
        );
        let coverage = resolved.project_coverage(&[], &[]);
        assert!(
            coverage
                .dynamic_dispatch_sites
                .iter()
                .any(|s| s.contains("distinct-op")),
            "project_coverage ignored the model's calls"
        );
    }

    /// ⟨T1⟩ The CONTRACT the non-destructive drain buys, stated so it survives the
    /// later id-schema change.
    ///
    /// Two page actions each declaring `trigger OnAction()` USED to collide on one
    /// internal routine id (gap G-18); ⟨T3⟩ gave `compute_routine_id` a conditional
    /// enclosing-member discriminator, so as of that change they do not. Whether they
    /// collide or not, the observable requirement is the same and
    /// is what this asserts: **every `OnAction` trigger in this fixture has a real
    /// summary (coverage present) whose derived row reaches the `Setup.Insert()` it
    /// calls through `Touch()`** — i.e. no routine is silently erased.
    ///
    /// Before the fix this FAILED: `build_detector_context` drained `cones`/`direct_full`
    /// with `remove()`, so the second occurrence of the colliding id got `None/None`,
    /// called `cone_derived.forget()` and overwrote the real summary with a degenerate
    /// one — no direct facts, no inherited facts, no coverage, no derived row. Every
    /// cone-derived detector reading that id then saw an empty cone and produced nothing.
    ///
    /// After the T3 id-schema change the two triggers get DISTINCT ids and each
    /// carries its own real summary — the assertions below held unchanged across it,
    /// which is why they are phrased over `ws.routines` rather than over "the
    /// colliding id". The still-honest collision pin is
    /// [`hand_stated_id_collision_keeps_a_real_summary_and_derived_row`] below.
    #[test]
    fn colliding_routine_ids_keep_a_real_summary_and_derived_row() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50811 "CP Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

page 50811 "CP Wizard"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action(First)
            {
                trigger OnAction()
                begin
                    Touch();
                end;
            }
            action(Second)
            {
                trigger OnAction()
                begin
                    Touch();
                end;
            }
        }
    }

    local procedure Touch()
    var
        Setup: Record "CP Setup";
    begin
        Setup.Insert();
    end;
}
"#;
        let files = vec![("src/CPWizard.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_inline_program_default(
            &files,
            "11111111-0000-0000-0000-0000000cp001",
        );
        let owned = build_detector_context(&resolved, substrate::SUMMARIES);
        let ctx = owned.ctx();

        // Without this the whole test would pass identically on an EMPTY store —
        // every `flags_of` would read the empty row for the trivial reason.
        assert!(
            !ctx.cone_derived.is_empty(),
            "precondition: the derived store must actually hold rows, or the \
             assertions below are vacuous"
        );

        let on_actions: Vec<&crate::program::model::workspace::ModelRoutine> = resolved
            .workspace
            .routines
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case("OnAction"))
            .collect();
        assert_eq!(
            on_actions.len(),
            2,
            "fixture precondition: both OnAction trigger bodies must be in the model"
        );

        for r in &on_actions {
            let s = ctx.summaries.get(&r.id).unwrap_or_else(|| {
                panic!(
                    "{}: every routine must have a summary (member {:?})",
                    r.id, r.enclosing_member
                )
            });
            assert!(
                s.coverage.is_some(),
                "{}: summary must not be degenerate — coverage was dropped (member {:?})",
                r.id,
                r.enclosing_member
            );
            assert!(
                ctx.cone_derived.touches_table(&r.id),
                "{}: the OnAction cone must still reach Touch()'s Setup.Insert() \
                 (member {:?})",
                r.id,
                r.enclosing_member
            );
        }

        // No routine anywhere in this fixture may end up with a degenerate summary.
        // `coverage_in` is inserted for EVERY routine in the first loop and
        // `compose_cone_over_graph` returns an entry for every node, so the only way
        // a `None` coverage can appear is the drain accident this test guards.
        let degenerate: Vec<&String> = ctx
            .summaries
            .iter()
            .filter(|(_, s)| s.coverage.is_none())
            .map(|(id, _)| id)
            .collect();
        assert!(
            degenerate.is_empty(),
            "no summary may be degenerate; got {degenerate:?}"
        );
    }

    /// ⟨task-1-review.md finding I-2⟩ Schema-independent pin for the SAME defect the
    /// test above pins — and, as of ⟨T3⟩, the ONLY one of the two that still exercises
    /// the drain path. That test asserts only `on_actions.len() == 2` — no collision
    /// PRECONDITION — so now that the id schema HAS a member discriminator, its two
    /// `OnAction` triggers get distinct natural ids, the drain path is never entered,
    /// and it keeps passing for a reason unrelated to its name. That the drain path
    /// still matters is measured, not hypothetical: 15 collision groups / 19 routines
    /// on 8020 survive the member discriminator (preproc `#if` alternatives and
    /// XMLport same-name elements at different nesting paths).
    ///
    /// This test never asks `compute_routine_id` for a collision — it STATES one:
    /// two `ModelRoutine`s built from ordinary, non-colliding source are forced to carry
    /// the literal same `id` by direct field assignment after assembly, before
    /// `build_detector_context` runs. That holds under ANY id schema, forever,
    /// because it does not depend on what the schema would have produced.
    #[test]
    fn hand_stated_id_collision_keeps_a_real_summary_and_derived_row() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50812 "CP2 Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

page 50812 "CP2 Wizard"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action(Alpha)
            {
                trigger OnAction()
                begin
                    Touch();
                end;
            }
            action(Beta)
            {
                trigger OnAction()
                begin
                    Touch();
                end;
            }
        }
    }

    local procedure Touch()
    var
        Setup: Record "CP2 Setup";
    begin
        Setup.Insert();
    end;
}
"#;
        let files = vec![("src/CP2Wizard.al".to_string(), src.to_string())];
        let mut resolved = assemble_and_resolve_inline_program_default(
            &files,
            "22222222-0000-0000-0000-0000000cp002",
        );

        // State the collision by hand: force the two `OnAction` triggers to carry the
        // literal SAME id via direct field assignment — never by relying on
        // `compute_routine_id` happening to agree (it does today; it need not
        // tomorrow). The model's calls and events were resolved at build time from
        // the original ids, so `force_shared_id` re-keys their edges too; the rest
        // (symbol table, combined graph) `build_detector_context` derives from the
        // routines at call time.
        const SHARED_ID: &str = "hand-stated-collision-id";
        let forced = force_shared_id(&mut resolved, "OnAction", SHARED_ID);
        assert_eq!(
            forced, 2,
            "fixture precondition: both OnAction trigger bodies must be in the model"
        );

        let owned = build_detector_context(&resolved, substrate::SUMMARIES);
        let ctx = owned.ctx();

        let summary = ctx
            .summaries
            .get(SHARED_ID)
            .expect("the hand-stated collision id must have a summary at all");
        assert!(
            summary.coverage.is_some(),
            "the surviving summary must not be degenerate — coverage was dropped"
        );
        assert!(
            ctx.cone_derived.touches_table(SHARED_ID),
            "the surviving derived row must still reach Touch()'s Setup.Insert()"
        );
    }

    /// #160 step 11a (spec §5.9), the epic's 11a proof: today's three rules for
    /// rows that share one string id, hand-stated (the technique of
    /// `hand_stated_id_collision_keeps_a_real_summary_and_derived_row`): two
    /// `OnAction` triggers with DIFFERENT bodies, B's `id` set to A's by
    /// assignment. R1: the combined graph files both rows' out-edges under the
    /// one id (union). R2: since 11b stage B the direct facts are the rows'
    /// full-value union and the coverage the least complete (it was the LAST
    /// row's). R3: one summary per id, carrying R2's direct facts. The natural
    /// `#if` form is `tests_11b::collision_rules_hand_stated` (T1).
    #[test]
    fn collision_rules_hand_stated() {
        use crate::engine::l4::capability_cone::direct_facts_for_routine;
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50813 "CR A"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50814 "CR B"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

page 50813 "CR Wizard"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action(Alpha)
            {
                trigger OnAction()
                var
                    A: Record "CR A";
                begin
                    A.Insert();
                    TouchA();
                end;
            }
            action(Beta)
            {
                trigger OnAction()
                var
                    B: Record "CR B";
                begin
                    B.Modify();
                    TouchB();
                end;
            }
        }
    }

    local procedure TouchA()
    begin
    end;

    local procedure TouchB()
    begin
    end;
}
"#;
        let files = vec![("src/CRWizard.al".to_string(), src.to_string())];
        let mut resolved = assemble_and_resolve_inline_program_default(
            &files,
            "22222222-0000-0000-0000-0000000cr011",
        );
        let id_of = |m: &Model, name: &str| {
            m.workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .id
                .clone()
        };
        let (touch_a, touch_b) = (id_of(&resolved, "TouchA"), id_of(&resolved, "TouchB"));
        let on_actions: Vec<usize> = resolved
            .workspace
            .routines
            .iter()
            .enumerate()
            .filter(|(_, r)| r.name == "OnAction")
            .map(|(ri, _)| ri)
            .collect();
        assert_eq!(on_actions.len(), 2, "precondition: two OnAction rows");

        const SHARED_ID: &str = "hand-stated-collision-id";
        let forced = force_shared_id(&mut resolved, "OnAction", SHARED_ID);
        assert_eq!(forced, 2, "precondition: both rows carry the shared id");
        // FP-14: the FIRST row is marked parse-incomplete by assignment, so the
        // two rows' coverage differs too and R2's coverage half can tell first
        // from last.
        resolved.workspace.routines[on_actions[0]].parse_incomplete = true;
        // Each row's own direct facts, under the shared id (the facts' subject).
        let facts = |ri: usize| direct_facts_for_routine(&resolved.workspace.routines[ri], &[]);
        let (a_direct, b_direct) = (facts(on_actions[0]), facts(on_actions[1]));
        assert_ne!(
            a_direct.0, b_direct.0,
            "precondition: the two bodies have different direct facts"
        );
        assert_ne!(
            (&a_direct.1, &a_direct.2),
            (&b_direct.1, &b_direct.2),
            "precondition: the two rows have different coverage"
        );

        // R2 (11b, the one rule): direct facts are the full-value union of both
        // rows' (here arm A is parse-incomplete, so it adds none), coverage the
        // least complete (arm A's `unknown`) with the sorted union of reasons.
        let inputs = det_cone_inputs(&resolved, &ExecRelation::single_app(&resolved));
        let mut union = [a_direct.0.clone(), b_direct.0.clone()].concat();
        union.sort();
        union.dedup();
        assert_eq!(
            inputs.direct_full.get(SHARED_ID),
            Some(&union),
            "R2: the direct facts are the union of the rows'"
        );
        let mut reasons = [a_direct.2.clone(), b_direct.2.clone()].concat();
        reasons.sort();
        reasons.dedup();
        assert_eq!(
            inputs.coverage_in.get(SHARED_ID),
            Some(&("unknown".to_string(), reasons)),
            "R2: the direct coverage is the least complete row's, reasons unioned"
        );
        assert_eq!(a_direct.1, "unknown", "precondition: arm A is unknown");

        let owned = build_detector_context(&resolved, substrate::SUMMARIES);
        let ctx = owned.ctx();
        // R1: the edges are the union of both rows'.
        let to: std::collections::BTreeSet<&str> = ctx
            .graph
            .edges_by_from
            .get(SHARED_ID)
            .map(|v| v.iter().map(|e| e.to.as_str()).collect())
            .unwrap_or_default();
        assert!(
            to.contains(touch_a.as_str()) && to.contains(touch_b.as_str()),
            "R1: both rows' callees under the one id: {to:?}"
        );
        // R3: one summary per id, holding R2's direct facts.
        let summary = ctx.summaries.get(SHARED_ID).expect("R3: a summary");
        assert!(summary.coverage.is_some(), "R3: not degenerate");
        assert_eq!(
            summary.capability_facts_direct, union,
            "R3: the summary holds the union of the rows' direct facts"
        );
    }

    /// ⟨fix wave FIX 1, final-branch-review finding 2⟩ Discriminates the call-site
    /// regression the review flagged: both `perf_bounds` L4 gates call
    /// `compute_summaries_v2_bundle` directly, so neither would notice if
    /// `build_detector_context` stopped calling it. On a fixture guaranteed to
    /// produce a REAL db-effect population (`Setup.Insert()` — `Insert` is
    /// db-touching, `summary_runner::is_db_touching`), this test proves the bundle
    /// side of the B1 invariant is populated; the production `debug_assert!` right
    /// after the solve in `build_l4` (which the shim calls) proves the
    /// `core_summaries` side stays empty — and is exercised for REAL here (not
    /// vacuously, the way it would be against an empty workspace). If the call
    /// site ever regresses to a materializing entry point, that debug_assert
    /// panics and this test fails (verified by temporarily re-pointing the call
    /// site — see `.superpowers/sdd/minors-report.md`'s Fix wave section for the
    /// before/after run).
    #[test]
    fn core_summaries_stay_lean_while_the_bundle_carries_the_db_effect_rows() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50900 "FX1 Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50900 "FX1 Touch"
{
    procedure Touch()
    var
        Setup: Record "FX1 Setup";
    begin
        Setup.Insert();
    end;
}
"#;
        let files = vec![("src/FX1Touch.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_inline_program_default(
            &files,
            "11111111-0000-0000-0000-0000000fx001",
        );
        // CORE_SUMMARIES alone is enough to reach the `compute_summaries_v2_bundle`
        // call (gated independently of SUMMARIES — see `build_detector_context`'s
        // own doc), so this test does not need the cone/SUMMARIES substrate at all.
        let owned = build_detector_context(&resolved, substrate::CORE_SUMMARIES);

        let bundle = &owned
            .l4()
            .expect("CORE_SUMMARIES was demanded — the bundle must exist")
            .bundle;
        let any_row_has_effects = bundle
            .routines_with_rows()
            .any(|rix| bundle.db_effects(rix).next().is_some());
        assert!(
            any_row_has_effects,
            "fixture precondition: `Setup.Insert()` must produce at least one compact \
             db_effects row in the bundle, or the debug_assert in \
             `build_detector_context` is never exercised against a real effect"
        );
    }

    /// ⟨Task 6⟩ The demand gate on `DB_EFFECT_REVERSE_INDEX`, in BOTH
    /// directions, on a workspace that really does produce db effects.
    ///
    /// The direction that matters is the negative one: wiring the index was
    /// declined before precisely because "does `analyze` pay for it?" had no
    /// answer. `analyze` reaches this function through `run_detectors`, which
    /// passes `demanded` — the fold of every selected detector's `requires` —
    /// and no detector may declare this bit (see the constant's doc), so the
    /// production analyze path cannot set it. `substrate::ALL` is asserted here
    /// as the standing proxy for that path: if someone ever folds the bit into
    /// `ALL`, this test fails rather than the four `ALL`-passing CLI callers
    /// silently starting to pay for a transpose none of them reads.
    ///
    /// The fixture's effect population is asserted as a PRECONDITION, not
    /// assumed — an empty bundle would make the positive direction pass
    /// vacuously.
    #[test]
    fn reverse_effect_index_is_built_only_when_its_bit_is_demanded() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50901 "FX2 Ledger"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50901 "FX2 Touch"
{
    procedure Touch()
    var
        Ledger: Record "FX2 Ledger";
    begin
        Ledger.Insert();
    end;
}
"#;
        let files = vec![("src/FX2Touch.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_inline_program_default(
            &files,
            "11111111-0000-0000-0000-0000000fx002",
        );

        // The bit is NOT in `ALL` — asserted directly, so folding it in there
        // fails here first.
        assert_eq!(
            substrate::ALL & substrate::DB_EFFECT_REVERSE_INDEX,
            0,
            "DB_EFFECT_REVERSE_INDEX must stay OUT of substrate::ALL — see its doc \
             for the four ALL-passing callers this protects and the parity trap"
        );

        // NEGATIVE: the full `ALL` substrate — everything analyze can ever ask
        // for — leaves the index unbuilt.
        let owned_all = build_detector_context(&resolved, substrate::ALL);
        let ctx_all = owned_all.ctx();
        assert!(
            owned_all.l4().is_some(),
            "ALL includes CORE_SUMMARIES, so the bundle IS built — which is what \
             makes the next assertion meaningful rather than vacuous"
        );
        assert!(
            ctx_all.reverse_effect_index.is_none(),
            "substrate::ALL must NOT build the reverse index"
        );

        // POSITIVE: ask for it explicitly and it appears, correct and populated.
        let owned = build_detector_context(
            &resolved,
            substrate::CORE_SUMMARIES | substrate::DB_EFFECT_REVERSE_INDEX,
        );
        let ctx = owned.ctx();
        let bundle = &owned.l4().expect("CORE_SUMMARIES demanded").bundle;
        let index = ctx
            .reverse_effect_index
            .as_ref()
            .expect("DB_EFFECT_REVERSE_INDEX demanded — the transpose must exist");

        // Fixture precondition, hand-stated: there IS a table with effects to
        // transpose. Without this the assertions below could all pass on an
        // empty index.
        let table_id = bundle
            .routines_with_rows()
            .flat_map(|rix| bundle.db_effects(rix))
            .map(|e| e.table_id.to_string())
            .next()
            .expect(
                "fixture precondition: `Ledger.Insert()` must produce at least one \
                 db effect, or this test proves nothing about the transpose",
            );

        let up = index.up_table(&table_id);
        assert!(
            !up.is_empty(),
            "the transpose must answer for a table the bundle really carries"
        );
        for &rix in &up {
            assert!(
                index.touches_table(bundle, rix, &table_id),
                "up_table and touches_table must agree"
            );
        }
    }

    /// The hash-consing CONTRACT, pinned at the USE: two nodes whose deduped
    /// uncertainty sets are equal must share ONE allocation on the built context,
    /// not hold two copies.
    ///
    /// This is the whole point of [`UncertaintySetPool`] and the only reason
    /// `uncertainties_by_node` is an `Arc<[Uncertainty]>` — on BC Base App 8020 the
    /// L4 solver broadcasts each SCC's uncertainty union to every member, so
    /// 3,700,433 records over 27,037 nodes are only 507,437 records in 10,112
    /// distinct sets. A regression to a per-node copy (`Arc::from(...)` without the
    /// pool) is INVISIBLE to every golden — the bytes are identical either way —
    /// so a pointer assertion is the only thing that can catch it.
    ///
    /// The fixture is the smallest shape that produces one: `A`/`B` are mutually
    /// recursive (one SCC) and `B` makes an unresolved call, so the solver hands
    /// BOTH members the same one-record union `[unresolved-call at B/cs1]`. Value
    /// equality is asserted first so a failure reads as "not shared" rather than
    /// "not equal", and the set is asserted non-empty so the test cannot pass on two
    /// empty slices.
    #[test]
    fn equal_uncertainty_sets_are_hash_consed_to_one_allocation() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
codeunit 50914 "HC Ring"
{
    procedure A()
    begin
        B();
    end;

    procedure B()
    begin
        A();
        MissingRing();
    end;
}
"#;
        let files = vec![("src/HCRing.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_inline_program_default(
            &files,
            "44444444-0000-0000-0000-0000000hc001",
        );
        let owned = build_detector_context(&resolved, substrate::CORE_SUMMARIES);
        let ctx = owned.ctx();

        let id_of = |name: &str| -> String {
            resolved
                .workspace
                .routines
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(name))
                .unwrap_or_else(|| panic!("fixture precondition: routine {name} must exist"))
                .id
                .clone()
        };
        let (a, b) = (id_of("A"), id_of("B"));

        let &sa = ctx.uncertainties_by_node.get(&a).unwrap_or_else(|| {
            panic!(
                "fixture precondition: the SCC broadcast must give A a non-empty                  uncertainty set, or this test proves nothing"
            )
        });
        let &sb = ctx
            .uncertainties_by_node
            .get(&b)
            .expect("fixture precondition: B must carry the same set");
        assert!(
            ctx.uncertainty_view().has_any(&a),
            "fixture precondition: the shared set must be NON-empty — two empty              sets would compare equal for the wrong reason"
        );
        assert_eq!(
            ctx.uncertainties.elements(sa),
            ctx.uncertainties.elements(sb),
            "fixture precondition: both SCC members must hold the SAME uncertainty              set (the solver per-member broadcast); if this diverges the identity              assertion below is testing the wrong thing"
        );

        assert_eq!(
            sa, sb,
            "equal per-node uncertainty sets must intern to ONE UncertaintySetId —              two distinct ids mean UncertaintyIndex is no longer collapsing them, and              the 729 MiB -> 102 MiB reduction on BC Base App is gone (no golden can              see this; only this assertion can)"
        );
    }

    /// The drain's CONTRACT under colliding internal routine ids: the surviving
    /// entry must be the FULL `summary ∪ edges` union, never the edges-only subset.
    ///
    /// `build_detector_context` now takes each routine's core summary out of
    /// `core_summaries` by `remove` (moving the uncertainty vector instead of deep-
    /// copying it). That makes the loop id-sensitive, and internal routine ids are
    /// NOT unique — 15 collision groups / 19 routines survive the member
    /// discriminator on BC Base App 8020 (`docs/OUTSTANDING.md`). Without the
    /// `processed` guard the SECOND routine of a colliding pair finds `None`,
    /// recomputes an edges-only set, and OVERWRITES the first routine's full union
    /// with that strict subset — silently dropping every INHERITED uncertainty from
    /// the node the whole path-walker substrate is built on.
    ///
    /// Neither net that guards this arc could see that: BC Base App is not a golden
    /// corpus, and the DO byte-identity workspace has ZERO collision groups. So this
    /// test STATES the collision by hand — the technique
    /// [`hand_stated_id_collision_keeps_a_real_summary_and_derived_row`] introduced,
    /// for the same reason: it holds under any future id schema because it never
    /// asks `compute_routine_id` to produce a collision.
    ///
    /// The fixture is built so the two harvests are distinguishable: each `OnAction`
    /// makes its OWN unresolved call (⇒ a non-empty `uncertainty_edges_by_from`
    /// entry, without which the naive drain would `continue` instead of overwriting,
    /// and the bug would not manifest) AND calls `Touch()`, which runs a codeunit
    /// chosen at run time (⇒ a `dynamic-dispatch` uncertainty, an INHERITED one that
    /// lives ONLY in the core summary). The union must therefore carry a
    /// `Touch`-owned operation. (Until S9.5 `Touch` called a missing procedure,
    /// which L3 labelled `unknown`; the program engine labels it
    /// `member-not-found`, a callsite-local kind that is by design never inherited
    /// — `db_effect_solver::is_callsite_local_kind` — so it stated nothing.)
    #[test]
    fn colliding_ids_keep_the_full_summary_union_not_just_the_edges() {
        use crate::engine::l5::registry::substrate;
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
table 50813 "CP3 Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

page 50813 "CP3 Wizard"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action(Alpha)
            {
                trigger OnAction()
                begin
                    Touch();
                    MissingHere();
                end;
            }
            action(Beta)
            {
                trigger OnAction()
                begin
                    Touch();
                    MissingHere();
                end;
            }
        }
    }

    local procedure Touch()
    var
        Setup: Record "CP3 Setup";
        N: Integer;
    begin
        Setup.Insert();
        Codeunit.Run(N);
    end;
}
"#;
        let files = vec![("src/CP3Wizard.al".to_string(), src.to_string())];
        let mut resolved = assemble_and_resolve_inline_program_default(
            &files,
            "33333333-0000-0000-0000-0000000cp003",
        );

        const SHARED_ID: &str = "hand-stated-uncertainty-collision-id";
        let forced = force_shared_id(&mut resolved, "OnAction", SHARED_ID);
        assert_eq!(
            forced, 2,
            "fixture precondition: both OnAction trigger bodies must be in the model"
        );
        // Call-site ids were minted at assembly time from the ORIGINAL routine ids,
        // so the two triggers keep DISTINCT callsite ids across the forced collision
        // — which is what makes the merged edge set and the merged summary set
        // distinguishable below.
        let touch_id = resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("Touch"))
            .expect("fixture precondition: Touch must be in the model")
            .id
            .clone();

        let owned = build_detector_context(&resolved, substrate::CORE_SUMMARIES);
        let ctx = owned.ctx();

        let edges = ctx
            .uncertainty_edges_by_from
            .get(SHARED_ID)
            .map(|v| v.len())
            .unwrap_or(0);
        assert!(
            edges > 0,
            "fixture precondition: the colliding id must own a NON-empty \
             uncertainty-edge set — with an empty one a naive drain would `continue` \
             rather than overwrite, and this test would pass vacuously"
        );

        let &union_sid = ctx
            .uncertainties_by_node
            .get(SHARED_ID)
            .expect("the colliding id must have a per-node uncertainty set at all");
        let union: Vec<&Uncertainty> = ctx
            .uncertainties
            .elements(union_sid)
            .iter()
            .map(|id| ctx.uncertainties.value(*id))
            .collect();
        let inherited: Vec<&&Uncertainty> = union
            .iter()
            .filter(|u| {
                u.kind == "dynamic-dispatch"
                    && u.operation_id
                        .as_deref()
                        .is_some_and(|op| op.starts_with(&format!("{touch_id}/")))
            })
            .collect();
        assert!(
            !inherited.is_empty(),
            "the surviving union must still carry Touch()'s INHERITED uncertainty —              it exists ONLY in the core summary, never in `uncertainty_edges_by_from`              for this node, so losing it means the drain overwrote the full union              with the edges-only subset. union={union:?}"
        );
        assert!(
            union.len() > edges,
            "the union ({}) must be strictly larger than the edge set ({edges}) —              equality is exactly the edges-only overwrite this guards",
            union.len()
        );
    }

    // -- UncertaintyIndex ------------------------------------------------------

    /// Two `interface-open-world` uncertainties differing ONLY in `interface_name`
    /// share an `uncertainty_key` (`"{kind}|{at}"` does not read it) while being
    /// DIFFERENT values. Every ordering assertion below is hand-stated on that
    /// shape, because it is the only one where last-write-wins is observable.
    fn u_iow(interface: &str) -> Uncertainty {
        Uncertainty {
            kind: "interface-open-world".to_string(),
            callsite_id: Some("cs/1".to_string()),
            operation_id: None,
            routine_id: None,
            interface_name: Some(interface.to_string()),
        }
    }

    #[test]
    fn interning_equal_sets_yields_one_set_id_and_one_element_window() {
        let mut ix = UncertaintyIndex::default();
        let a = ix.intern_set(vec![u_iow("IAlpha"), u_iow("IBeta")]);
        let b = ix.intern_set(vec![u_iow("IAlpha"), u_iow("IBeta")]);
        assert_eq!(a, b, "equal sets are ONE set id");
        assert_eq!(ix.elements(a), ix.elements(b));
        assert_eq!(
            ix.value_count(),
            2,
            "two distinct values, interned once each"
        );
        assert_eq!(ix.set_count(), 1, "…and one distinct set");
    }

    /// Set identity is by content AND ORDER. A reordered set is a DIFFERENT set,
    /// because the union that consumes it is last-write-wins by key and therefore
    /// order-sensitive — collapsing the two would silently change which value wins.
    #[test]
    fn a_reordered_set_is_a_distinct_set_id() {
        let mut ix = UncertaintyIndex::default();
        let a = ix.intern_set(vec![u_iow("IAlpha"), u_iow("IBeta")]);
        let b = ix.intern_set(vec![u_iow("IBeta"), u_iow("IAlpha")]);
        assert_ne!(a, b);
        assert_eq!(
            ix.value_count(),
            2,
            "…while still interning only two values"
        );
    }

    /// Round-trip: the elements resolve back to the EXACT input sequence. This is
    /// what lets `share()` hand out an `Arc<[Uncertainty]>` byte-identical to the
    /// one it used to build directly.
    #[test]
    fn elements_round_trip_to_the_input_sequence() {
        let mut ix = UncertaintyIndex::default();
        // A CONSECUTIVE duplicate and a non-consecutive one. The consecutive pair
        // is deliberate: `Vec::dedup` only collapses adjacent equals, so an input
        // without one cannot detect a stray dedupe in `intern_set` at all.
        let input = vec![
            u_iow("IBeta"),
            u_iow("IBeta"),
            u_iow("IAlpha"),
            u_iow("IBeta"),
        ];
        let sid = ix.intern_set(input.clone());
        let back: Vec<Uncertainty> = ix
            .elements(sid)
            .iter()
            .map(|id| ix.value(*id).clone())
            .collect();
        assert_eq!(back, input, "same values, same order, duplicates preserved");
    }

    /// The three cases `node_has_uncertainty` must keep distinguishing — or
    /// rather, must keep NOT distinguishing. Hand-stated because the migration
    /// from `HashMap<String, Arc<[Uncertainty]>>` to `HashMap<String,
    /// UncertaintySetId>` changes what "absent" means at the type level: a node
    /// with NO ENTRY and a node with an EMPTY SET were both `false` before
    /// (`get(..).is_some_and(|v| !v.is_empty())`) and must both stay `false`.
    /// This predicate feeds `ContextKey.unc`, so a change here re-partitions d1's
    /// cohorts and moves output.
    #[test]
    fn absent_and_empty_are_both_no_uncertainty_and_nonempty_is_yes() {
        let mut ix = UncertaintyIndex::default();
        let empty = ix.intern_set(Vec::new());
        let full = ix.intern_set(vec![u_iow("IAlpha")]);
        let mut by_node: HashMap<String, UncertaintySetId> = HashMap::new();
        by_node.insert("has-empty".to_string(), empty);
        by_node.insert("has-one".to_string(), full);
        // "absent" is deliberately never inserted.
        let view = UncertaintyView {
            index: &ix,
            by_node: &by_node,
        };

        assert!(!view.has_any("absent"), "no entry ⇒ no uncertainty");
        assert!(!view.has_any("has-empty"), "empty set ⇒ no uncertainty");
        assert!(view.has_any("has-one"));

        assert_eq!(view.ids_of("absent"), &[] as &[UncertaintyId]);
        assert_eq!(view.ids_of("has-empty"), &[] as &[UncertaintyId]);
        assert_eq!(view.ids_of("has-one").len(), 1);

        // The value view agrees with the id view, element for element.
        let vals: Vec<&Uncertainty> = view.values_of("has-one").collect();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0], ix.value(view.ids_of("has-one")[0]));
        assert_eq!(view.values_of("absent").count(), 0);
    }

    /// An empty set is a real set id, and `is_empty_set` answers O(1) — this is
    /// what replaces `HashMap::get(..).is_none()` as the "does this node carry
    /// uncertainty" test.
    #[test]
    fn the_empty_set_is_interned_and_reported_empty() {
        let mut ix = UncertaintyIndex::default();
        let e = ix.intern_set(Vec::new());
        assert!(ix.is_empty_set(e));
        assert_eq!(ix.elements(e), &[] as &[UncertaintyId]);
        let n = ix.intern_set(vec![u_iow("IAlpha")]);
        assert!(!ix.is_empty_set(n));
    }

    /// #160 task 21 review I-2: a `CORE_SUMMARIES` mask with `l4: None` would
    /// leave the uncertainty/roles harvest silently empty. It must panic, in
    /// every build.
    #[test]
    fn core_summaries_without_an_l4_request_panics() {
        use crate::engine::l5::registry::substrate;
        let model = crate::engine::l5::test_support::empty_resolved();
        let rel = ExecRelation::single_app(&model);
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_detector_artifacts(&model, substrate::CORE_SUMMARIES, &rel, None, None, None);
        }))
        .expect_err("CORE_SUMMARIES with l4: None must panic");
        let msg = err
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| err.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(
            msg.contains("CORE_SUMMARIES demanded without an l4 request"),
            "wrong panic: {msg}"
        );
    }
}

// #160 step 11b acceptance tests: a child module, for private access.
#[cfg(test)]
#[path = "detector_context_11b_tests.rs"]
mod tests_11b;

// #160 step 5 acceptance tests (one coverage policy): a child module, for
// private access to `det_cone_inputs`.
#[cfg(test)]
#[path = "detector_context_step5_tests.rs"]
mod tests_step5;
