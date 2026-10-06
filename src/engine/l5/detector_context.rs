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
//!   - the lazy `get_ordering_facts()` hook (D47)
//!
//! d4 reads none of these; later detector waves add them as they land.
//!
//! The R4-G wave wired `reachable_roots` + `internal_reachable_externally` (D14):
//! `reachable_roots` is built via `entry_points::find_reachable_roots` over the
//! `access_modifiers` map harvested from `L3Routine.access_modifier`;
//! `internal_reachable_externally` DEFAULTS to `false` (see field doc).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::engine::l2::features::PCallSite;
use crate::engine::l3::call_resolver::{CallEdge, UpgradedBinding, calls_for};
use crate::engine::l3::event_graph::build_event_graph;
use crate::engine::l3::event_graph::{EventGraph, EventSymbol};
use crate::engine::l3::l3_workspace::{L3Object, L3Resolved, L3Routine, L3Table};
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l4::capability_cone::{
    CapabilityFact, compose_cone_over_graph, direct_facts_for_routine,
};
use crate::engine::l4::combined_graph::{CombinedGraph, build_combined_graph};
use crate::engine::l4::cone_derived::{ConeDerivedStore, ConeOutput};
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::reverse_index::ReverseEffectIndex;
use crate::engine::l4::scc::{SccInputGraph, tarjan_scc};
use crate::engine::l4::summary::{
    RecordRoleSummary, Uncertainty, dedupe_uncertainties, uncertainty_key,
};
use crate::engine::l4::summary_runner::{
    FieldIndex, compute_summaries_v2_bundle, summaries_census,
};
use crate::engine::l5::confidence::UncertaintyLite;
use crate::engine::l5::entry_points::AccessModifier;
use crate::engine::l5::event_flow::{EventFlowIndexes, build_event_flow_indexes};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::reverse_call_graph::{ReverseCallGraph, build_reverse_call_graph};
use crate::engine::l5::transaction_spans::{TransactionSpan, compute_transaction_spans};
use crate::engine::perf_trace as pt;
use serde_json::json;

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
    use std::sync::atomic::{AtomicU64, Ordering};

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
    // Inside compose: the two per-member inherited-fact walks.
    pub(crate) static BFS_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BFS_CALLS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SINGLETON_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SINGLETON_CALLS: AtomicU64 = AtomicU64::new(0);
    /// Inside `inherited_facts_for_singleton`, which the first census round left
    /// at ONE number (4.8 s over 100,419 calls) with nothing saying what inside
    /// it costs — and where precomputing `edge_sort_key` then bought only 7 %,
    /// proving the cost is elsewhere. Split three ways: the candidate SCAN over
    /// every out-edge's callee cone, the derived FOLD over the surviving bests,
    /// and the raw materialization (`retag` + `sort_inherited`, skipped under
    /// `ConeOutput::DerivedOnly`).
    pub(crate) static SGL_SCAN_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_FOLD_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SGL_RAW_NANOS: AtomicU64 = AtomicU64::new(0);
    /// Populations the scan time is per. Accumulated in plain locals inside the
    /// walk and folded in with ONE `fetch_add` per call — the inner loop runs
    /// millions of times and must not touch an atomic (the distortion this
    /// module's `add_gated` doc records).
    pub(crate) static SGL_EDGES: AtomicU64 = AtomicU64::new(0);
    /// Every `(key, entry)` pair scanned — the REAL population of the walk, as
    /// opposed to its call count.
    pub(crate) static SGL_CONE_ENTRIES: AtomicU64 = AtomicU64::new(0);
    /// `best` insertions — each one a `String` key clone today.
    pub(crate) static SGL_WINS: AtomicU64 = AtomicU64::new(0);
    /// Surviving `best` entries summed over all calls (= `fold_fact` calls).
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
            "[cones-census]   compose: bfs={:.1}ms x{} singleton={:.1}ms x{} max_scc_members={}",
            ms(BFS_NANOS.load(Ordering::Relaxed)),
            BFS_CALLS.load(Ordering::Relaxed),
            ms(SINGLETON_NANOS.load(Ordering::Relaxed)),
            SINGLETON_CALLS.load(Ordering::Relaxed),
            MAX_SCC_MEMBERS.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   singleton: scan={:.1}ms fold={:.1}ms raw={:.1}ms",
            ms(SGL_SCAN_NANOS.load(Ordering::Relaxed)),
            ms(SGL_FOLD_NANOS.load(Ordering::Relaxed)),
            ms(SGL_RAW_NANOS.load(Ordering::Relaxed)),
        );
        eprintln!(
            "[cones-census]   singleton calls: 0_cones={} 1_cone={} n_cones={} entries_under_n_cones={}",
            SGL_CALLS_0_CONES.load(Ordering::Relaxed),
            SGL_CALLS_1_CONE.load(Ordering::Relaxed),
            SGL_CALLS_N_CONES.load(Ordering::Relaxed),
            SGL_ENTRIES_N_CONES.load(Ordering::Relaxed),
        );
        eprintln!(
            "[cones-census]   singleton pop: edges={} cone_entries={} wins={} bests={}",
            SGL_EDGES.load(Ordering::Relaxed),
            SGL_CONE_ENTRIES.load(Ordering::Relaxed),
            SGL_WINS.load(Ordering::Relaxed),
            SGL_BESTS.load(Ordering::Relaxed),
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
            "[cones-census]   merge_cone: calls={} tiebreaks={} (a tiebreak = 2x rep_key, each JSON-serializing)",
            MERGE_CALLS.load(Ordering::Relaxed),
            MERGE_TIEBREAKS.load(Ordering::Relaxed),
        );
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

/// Shared context threaded into every detector.
pub struct DetectorContext<'a> {
    /// The combined graph (al-sem passes this as the detector's `graph` arg;
    /// detectors read it from the ctx here).
    pub graph: CombinedGraph,
    /// The raw L3 event graph (al-sem `model.eventGraph`). d12/d38 read its
    /// `events`/`edges`; the combined-graph build already constructs it, so it is
    /// captured here rather than recomputed.
    pub event_graph: EventGraph,
    pub routine_by_id: HashMap<&'a str, &'a L3Routine>,
    pub objects_by_id: HashMap<&'a str, &'a L3Object>,
    pub table_by_id: HashMap<&'a str, &'a L3Table>,
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
    /// Every call site indexed by id.
    pub call_site_by_id: HashMap<&'a str, &'a PCallSite>,
    /// Per-routine `FullRoutineSummary` (direct + inherited facts + coverage).
    pub summaries: HashMap<String, FullRoutineSummary>,
    /// ⟨C1⟩ The compact DERIVED capability-cone substrate — per-routine presence
    /// flags + interned table/event id-sets folded during the same cone walk that
    /// produces `summaries`, with zero `retag` clones. Every analyze-path consumer
    /// of the old `capability_facts_inherited` Vec reads only a derived predicate
    /// off it, so this row REPLACES that Vec outright: since C1 Task 3 the analyze
    /// path composes under `ConeOutput::DerivedOnly` and the raw Vec is never
    /// built (`summaries[*].inherited_raw()` panics there — see
    /// `FullRoutineSummary`). Parked here — next to `db_effect_bundle` — because
    /// the rows are `Range<u32>` windows into pools this store owns; a row alone
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
    /// harvested from `L3Routine.access_modifier`. Sorted; d14 BFS-seeds from it.
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
    /// R4-F Stage-5b — the L4.5 ordering facts the d47/d49/d51 detectors consume,
    /// keyed by `StableRoutineId`. Computed LAZILY on first `get_ordering_facts()`
    /// access and memoized — exactly al-sem's `ctx.getOrderingFacts()` semantics.
    /// Only d47/d49/d51 (opt-in detectors) read it, so a default `analyze` run
    /// never pays the snapshot→digest→ordering cost (measured 43.6 s+ on CDO —
    /// the "alsem never completes" hang; see
    /// `.superpowers/sdd/alsem-parallel/investigation.md`).
    pub ordering_facts:
        std::sync::OnceLock<HashMap<String, crate::engine::l5::ordering_facts::OrderingFacts>>,
    /// The resolved model `get_ordering_facts()` computes from. `None` for the
    /// cross-app context (whose ordering facts are ALWAYS empty — d13/d16/d17
    /// never read them; matches the previous eager `HashMap::new()`).
    pub ordering_source: Option<&'a L3Resolved>,
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
    /// Harvested from the SAME `compute_summaries*` call this module already makes for
    /// `uncertainties_by_node`/`parameter_roles_by_routine` — not recomputed.
    /// Empty for every workspace whose SCCs converge, which is the overwhelming
    /// common case (additive: `run_detectors` folds this into the "summarize"
    /// slot of the analyze/detect diagnostics envelope).
    pub summarize_diagnostics: Vec<crate::engine::l4::summary_runner::SummarizeDiagnostic>,
    /// ⟨Task B1⟩ The workspace-complete compact db-effect store the analyze path's
    /// v2 solve produced, held HERE instead of eagerly re-materialized into
    /// per-routine `Vec<DbEffect>`. No current detector reads `RoutineSummary.db_effects`
    /// (the analyze path consumes only `.uncertainties` / `.parameter_roles`), so the
    /// old compat-shim materialization was pure waste (~24 GB / ~74 s on 8020). The
    /// rows stay QUERYABLE on demand — `bundle.db_effects(rix)` (lazy projection) or an
    /// `l4::reverse_index::ReverseEffectIndex` built from this bundle — so a future
    /// db-effect-reading detector/hover can query them lazily WITHOUT resurrecting the
    /// eager expansion. `None` when the `CORE_SUMMARIES` substrate was not demanded
    /// (the summary solve is skipped entirely — no bundle exists to hold).
    pub db_effect_bundle: Option<SummaryBundle>,
    /// ⟨Task 6⟩ The [`ReverseEffectIndex`] transpose over [`Self::db_effect_bundle`]
    /// — the routine <-> table/effect inverted index a db-effect-reading detector
    /// or the planned LSP hover would query.
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
    /// The shared finding-fingerprint index (routine/object id maps + the
    /// internal→stable routine-id substitution map). Built ONCE per run —
    /// previously every detector rebuilt it (54 × ~2 String clones per routine).
    pub fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex<'a>,
    /// event id → cross-extension subscriber routine ids (subscribers living in a
    /// DIFFERENT app than the publisher object). Previously rebuilt identically by
    /// d43, d44 AND d45 each run; built once here (same sharing pattern as
    /// `event_flow_indexes`).
    pub cross_extension_subscribers: std::collections::BTreeMap<String, Vec<String>>,
}

impl DetectorContext<'_> {
    /// The uncertainty substrate as one borrowed view — the id map and the index
    /// its ids resolve against, which are meaningless apart.
    /// Test-only: intern `set` and attach it to `node` on this context.
    #[cfg(test)]
    pub(crate) fn set_uncertainties_for_test(&mut self, node: &str, set: Vec<Uncertainty>) {
        let sid = self.uncertainties.intern_set(set);
        self.uncertainties_by_node.insert(node.to_string(), sid);
    }

    pub fn uncertainty_view(&self) -> UncertaintyView<'_> {
        UncertaintyView {
            index: &self.uncertainties,
            by_node: &self.uncertainties_by_node,
        }
    }

    /// The L4.5 ordering facts, keyed by `StableRoutineId`. Lazily computed on
    /// first access and memoized via `OnceLock`. d47/d49/d51 look up their
    /// reportable routine's facts here exactly as al-sem's `ctx.getOrderingFacts()`.
    ///
    /// Thread-safe is NOT the same as safe inside the parallel detector loop:
    /// filling the slot there can deadlock (see `substrate::ORDERING_FACTS`). A
    /// detector that reads this must declare that bit, so `run_each` fills the
    /// slot first; in debug builds an undeclared read panics here instead of
    /// hanging sometimes.
    pub fn get_ordering_facts(
        &self,
    ) -> &HashMap<String, crate::engine::l5::ordering_facts::OrderingFacts> {
        debug_assert!(
            self.ordering_facts.get().is_some() || !crate::engine::l5::registry::in_detector_loop(),
            "a detector read ctx.get_ordering_facts() inside the parallel detector \
             loop without declaring substrate::ORDERING_FACTS in its `requires`"
        );
        self.ordering_facts
            .get_or_init(|| match self.ordering_source {
                Some(resolved) => {
                    crate::engine::l5::ordering_facts::compute_ordering_facts(resolved)
                }
                None => HashMap::new(),
            })
    }
}

/// Build the shared context. Runs the SOURCE-ONLY L3→L4 substrate (symbols →
/// resolve_calls → event_graph → combined_graph → cone) to assemble the combined
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
///   - `CORE_SUMMARIES` — second Tarjan + the closed-form v2 core summaries
///     (`db_effects`/`uncertainties` via `compute_summaries_v2`, `parameter_roles`
///     via its own JACOBI-disciplined roles-only fixpoint) →
///     `uncertainties_by_node` / `parameter_roles_by_routine` / `summarize_diagnostics`.
///   - `TRANSACTION_SPANS` — `transaction_spans`.
///   - `CLOSED_WORLD_TEMP` — `closed_world_temp_params`.
///
/// ⟨C1 Task 3⟩ `demanded` additionally carries the policy-only
/// `RAW_INHERITED_FACTS` bit (NOT part of `substrate::ALL` — see its doc). Without
/// it the cone composes under [`ConeOutput::DerivedOnly`]: the per-routine raw
/// `Vec<CapabilityFact>` is never allocated (it cost ~10.9 GB on the 8020 corpus)
/// and every summary carries `capability_facts_inherited: None`. With it the cone
/// composes under [`ConeOutput::Both`] — the derived substrate AND the raw Vecs,
/// byte-identical to the pre-Task-3 build.
pub fn build_detector_context(resolved: &L3Resolved, demanded: u32) -> DetectorContext<'_> {
    use crate::engine::l5::registry::substrate;
    let ws = &resolved.workspace;
    // TRANSACTION_SPANS folds over the summaries map, so demand summaries whenever
    // either bit is set (see `compute_transaction_spans`).
    let need_summaries = demanded & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) != 0;
    // ⟨C1 Task 3 — R1⟩ The gate is a MODE threaded into the cone walk, not a
    // post-hoc check: the raw Vec is allocated INSIDE `compose_inherited_cones`,
    // so a check here could only discard it — zero memory win.
    let want_raw_inherited = demanded & substrate::RAW_INHERITED_FACTS != 0;
    // ⟨C1 Task 3 review fix M-1⟩ `RAW_INHERITED_FACTS` without `SUMMARIES` (nor
    // `TRANSACTION_SPANS`) is a SILENT no-op: `need_summaries` would be false, the
    // block below never runs, `summaries` stays empty, and `select_facts` (which
    // reads through `summaries`) then returns an empty fact list for every
    // routine — the exact silent-empty outcome R6 was designed to foreclose,
    // reached through a different door. No current caller does this (`R2`'s table
    // above), but a future one could; fail loudly instead of degrading quietly.
    debug_assert!(
        !want_raw_inherited || need_summaries,
        "RAW_INHERITED_FACTS demanded without SUMMARIES/TRANSACTION_SPANS — the raw \
         cone would silently build empty; OR in substrate::SUMMARIES alongside it"
    );

    // --- L3→L4 substrate (source-only: no deps) ----------------------------
    // `symbols` feeds BOTH spans below (resolve_calls here, build_event_graph in
    // the next stage), so it is built at this outer scope instead of inside either
    // span's own block — the two spans are closed explicitly (`drop`) at their
    // semantic stage ends rather than by a block boundary (same pattern as
    // `gate/run.rs`'s `gate.project_filter_scope_baseline_suppress`).
    let _symbols_span = pt::span("context", "context.symbols_resolve_calls");
    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    // Owned: the body below drains `calls.edges`/`upgraded_bindings` by move.
    let mut calls = calls_for(resolved, &symbols).into_owned();
    drop(_symbols_span);

    let _graph_span = pt::span("context", "context.event_combined_graph");
    let event_graph = build_event_graph(&ws.routines, &symbols);
    let graph = build_combined_graph(ws, &calls, &event_graph);
    drop(_graph_span);

    // Per-routine direct facts + direct coverage, then the inherited cone over
    // the combined graph — the same assembly project_r3a3 does inline, here via
    // the reusable `compose_cone_over_graph` seam. SUBSTRATE-GATED: built only when
    // some selected detector demands SUMMARIES (or TRANSACTION_SPANS, which folds
    // over the summaries map). Skipped ⇒ empty `summaries` map.
    let _cones_span = pt::span("context", "context.capability_cones");
    // ⟨C1⟩ Assigned inside the block below (kept a separate binding so the whole
    // cone-assembly block stays at its original indentation).
    let mut cone_derived = ConeDerivedStore::default();
    let summaries: HashMap<String, FullRoutineSummary> = if need_summaries {
        let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
        for evt in &event_graph.events {
            if let Some(pr) = &evt.publisher_routine_id {
                publisher_events_by_routine
                    .entry(pr.clone())
                    .or_default()
                    .push(evt);
            }
        }
        let empty_pub: Vec<&EventSymbol> = Vec::new();
        // ⟨C1 Task 4⟩ ONE direct-facts map, not two. This used to build a
        // second, byte-identical `direct_in` (a `facts.clone()` per routine)
        // purely to hand to the cone walk, which takes it by shared reference
        // and only reads it — 79.66 MB of pure duplicate on the 8020 corpus,
        // held for the rest of this block because Rust frees a local at its
        // block's end, not at its last use. `direct_full` is not drained until
        // the summary loop below, well after the walk's borrow ends, so the
        // walk can simply read it. (Every OTHER `compose_cone_over_graph` call
        // site — `project_r3a3`, `project_r3a5_cross_app`,
        // `build_detector_context_cross_app` — already passed its `direct_full`
        // directly; this was the one straggler.)
        let mut direct_full: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        let mut coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();
        let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
        let _t_direct = cones_census::start();
        for r in &ws.routines {
            let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
            let (facts, status, reasons) = direct_facts_for_routine(r, pubs);
            coverage_in.insert(r.id.clone(), (status, reasons));
            direct_full.insert(r.id.clone(), facts);
        }
        // ⟨C1 Task 3⟩ `DerivedOnly` — the compact substrate only; the per-routine
        // raw inherited `Vec<CapabilityFact>` is never allocated. `Both` only when
        // the policy-only `RAW_INHERITED_FACTS` bit is demanded.
        let mode = if want_raw_inherited {
            ConeOutput::Both
        } else {
            ConeOutput::DerivedOnly
        };
        cones_census::add_since(&cones_census::DIRECT_NANOS, _t_direct);
        cones_census::add(&cones_census::ROUTINES, ws.routines.len() as u64);
        cones_census::add(
            &cones_census::DIRECT_FACTS,
            direct_full.values().map(|v| v.len() as u64).sum::<u64>(),
        );

        let _t_walk = cones_census::start();
        let outcome = compose_cone_over_graph(
            &graph,
            &nodes,
            &direct_full,
            &coverage_in,
            &ws.routines,
            mode,
        );
        cones_census::add_since(&cones_census::WALK_NANOS, _t_walk);
        let mut cones = outcome.cones;
        cone_derived = outcome.derived;
        // ⟨C1 Task 4⟩ Both cone inputs are dead here; free them before the
        // summary assembly below rather than at this block's closing brace
        // (`nodes` is one more full copy of every routine id, `coverage_in` one
        // more copy of every routine's direct status + reasons).
        drop(nodes);
        drop(coverage_in);

        // `cones` and `direct_full` are locally owned and dead after this loop, so
        // move their payloads into the summaries instead of cloning them out.
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        let _t_assemble = cones_census::start();
        for r in &ws.routines {
            let cone_entry = cones.remove(&r.id);
            let direct = direct_full.remove(&r.id);
            // ⟨T1, corrected by the T3 fix wave (review M-3)⟩ Two AL routines can
            // still COLLIDE on one internal routine id — gap G-18. Historically the
            // usual shape was two same-name triggers in one object, because
            // `compute_routine_id` had no member discriminator; ⟨T3⟩ added a
            // conditional one, so TODAY the surviving shapes are the ones a flat
            // member name cannot separate (XMLport same-name elements at different
            // nesting paths; preproc `#if`/`#else` alternatives) plus any
            // hand-stated id. See the ⟨T3⟩ paragraph below for the measured
            // residual. Both `remove`s above are then consumed by the FIRST
            // occurrence, so every LATER occurrence sees `None/None`.
            //
            // This used to write a fully degenerate summary for that later
            // occurrence — no direct facts, no inherited facts, no coverage —
            // and, because the map is keyed by id, that degenerate row is what
            // SURVIVED: `cone_derived.forget(&r.id)` then dropped the real
            // derived row to match, so the whole cone of an id shared by N
            // routines was erased and every cone-derived detector on it went
            // silent. That was never a decision — `build_detector_context_cross_app`
            // reads its cone with `get()` and has never had the accident; the
            // two builders simply disagreed. Measured, the collapse is not rare:
            // 1 157 of 4 842 DO routines (23.9 %) and 16 906 of 100 941 8020
            // routines (16.7 %) are erased by it (see
            // `.superpowers/sdd/scope-routine-id-collision.md`).
            //
            // Now: the FIRST occurrence's real summary is written and every later
            // occurrence is skipped, so the surviving summary is the real one.
            // No clone is paid for this (the `remove()` drain the C1 arc chose
            // over `get()` is kept — a `get()` would clone every routine's direct
            // facts, including the ~80 % that never collide, 79.66 MB of pure
            // duplicate on 8020).
            //
            // This is a PARTIAL fix and deliberately so. Loop 1 above builds
            // `direct_full` with `insert()`, so a colliding id's DIRECT facts are
            // already last-sibling-wins before the cone walk runs; the surviving
            // summary's direct half therefore carries ONE arbitrary (deterministic,
            // but arbitrary) sibling's facts attributed to all N. The INHERITED
            // half is NOT one sibling's view: the combined graph files every
            // sibling's out-edges under the one shared `from` key, so the cone walk
            // consumes their union — the surviving cone is (last sibling's direct
            // facts) ∪ (cone over the union of ALL siblings' callees), an
            // over-approximation rather than one body's picked view. What this
            // fixes is the strictly worse state of holding NO answer at all.
            //
            // ⟨T3⟩ The id schema itself now carries a CONDITIONAL enclosing-member
            // discriminator (`ids::encode_canonical_routine_key`), so on real
            // workspaces this path is very nearly unreachable: DO went from 262
            // colliding groups to 0, 8020 from 3 058 to 15. It is NOT dead code —
            // those 15 (XMLport same-name elements at different nesting paths;
            // preproc `#if` alternatives, which the union-read design makes
            // genuinely indistinguishable) still reach it, and it is the
            // fail-closed behaviour for any hand-stated or future collision.
            //
            // ⟨fix M1⟩ `cone_entry.is_none() <=> direct.is_none()` today: both maps
            // are built from the same `ws.routines` iteration and drained by the
            // same per-id `remove()`, so a collision empties them together, never
            // just one. Assert that invariant instead of merely relying on it —
            // if a future change ever filters `nodes` (dep routines, bodyless
            // routines, …) while `direct_full` kept every routine, the OLD `||`
            // would zero a row whose surviving summary still carries direct
            // facts, a silent findings loss once the parity oracle is retired.
            // Keying on `direct.is_none()` alone is the correct long-term
            // condition either way, since `direct` is what the surviving summary
            // actually stores.
            //
            // ⟨final-branch-review M-2, corrected per fix wave finding 1⟩ Task 2 had
            // added a second, reverse guard — the parity oracle's "the store cannot
            // hold a row `summaries` does not" check — and Task 3 deleted it along
            // with `cone_parity.rs` itself, which is also what returned
            // `ConeDerivedStore`'s `routine_ids()`/`interner()`/`len()` to zero
            // callers (deleted at M-2). That extra-row direction has NO guard today:
            // this assert only sees ids present in `ws.routines` (it lives inside the
            // `for r in &ws.routines` loop above), so a superset `nodes` — e.g. a
            // future widening of the cone input to dep routines — would produce rows
            // this loop never visits, and this assert would never fire against them,
            // in debug or otherwise.
            debug_assert_eq!(
                cone_entry.is_none(),
                direct.is_none(),
                "cones and direct_full must collide identically — a mismatch means \
                 `nodes` (cone input) and `ws.routines` (direct_full's source) have \
                 silently diverged"
            );
            // ⟨T1⟩ The drain came back empty: this is a LATER occurrence of an id
            // the first occurrence already consumed. That first occurrence wrote
            // the real summary and `cone_derived` already holds the matching
            // derived row, so the only correct action is to leave both alone.
            // Falling through would overwrite the real summary with a degenerate
            // one and then `forget()` the real derived row to match it.
            let Some(direct) = direct else {
                continue;
            };
            let (inherited, coverage) = match cone_entry {
                Some(c) => (c.inherited, Some(c.coverage)),
                // Unreachable while `nodes` is exactly `ws.routines`' ids (the
                // `debug_assert_eq!` above pins that); kept fail-soft rather than
                // `unwrap()` so a future divergence degrades one routine's coverage
                // instead of panicking a release build.
                None => (Vec::new(), None),
            };
            // ⟨C1 Task 3⟩ `Some(inherited)` ONLY under `RAW_INHERITED_FACTS`;
            // `None` records "never materialized" so `inherited_raw()` panics
            // instead of answering "empty cone" (R6). The `Some(Vec::new())` case
            // is still REAL and must stay distinct from `None` — a leaf routine
            // with no callees has a materialized-but-empty cone. (It is no longer
            // ALSO produced by the G-18 collision arm, which now `continue`s
            // above rather than composing a drained entry.)
            summaries.insert(
                r.id.clone(),
                FullRoutineSummary::new(
                    r.id.clone(),
                    direct,
                    want_raw_inherited.then_some(inherited),
                    coverage,
                ),
            );
        }
        cones_census::add_since(&cones_census::ASSEMBLE_NANOS, _t_assemble);
        cones_census::report();
        summaries
    } else {
        HashMap::new()
    };
    drop(_cones_span);

    // ⟨C1 census⟩ `C1_CONE_CENSUS=1` — one-shot byte census of what the cone
    // build just left resident (`summaries` + `cone_derived`), emitted after
    // the span closes so the census's own (modest) bookkeeping never pollutes
    // the span's own `rss_delta` measurement. No-op when the env var is unset.
    crate::engine::l4::cone_census::emit_full_census(&summaries, &cone_derived);

    // --- Eager indexes -----------------------------------------------------
    let routine_by_id: HashMap<&str, &L3Routine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let objects_by_id: HashMap<&str, &L3Object> =
        ws.objects.iter().map(|o| (o.id.as_str(), o)).collect();
    // G-5: REAL table wins an id collision with a tableextension stub (the stub's
    // id reuses the extension's own object number) — otherwise rootCause text
    // renders the EXTENSION's name for ops on the real table.
    let table_by_id: HashMap<&str, &L3Table> =
        crate::engine::l3::l3_workspace::table_by_id_preferring_real(&ws.tables);

    let reverse_call_graph = build_reverse_call_graph(&graph);

    // Source-only: no dep routines.
    let dep_routine_ids: BTreeSet<String> = BTreeSet::new();
    let entry_points: BTreeSet<String> =
        crate::engine::l5::entry_points::find_entry_points(&ws.routines, &dep_routine_ids)
            .into_iter()
            .collect();

    // D14 reachable-roots wiring. Build the RoutineId → AccessModifier map from
    // `L3Routine.access_modifier` ("local"/"internal"/"protected"/None). al-sem maps
    // "local" → Local, "internal" → Internal, "protected"/None/anything-else →
    // Public (default-access). A routine with NO entry is treated as Public by
    // `find_reachable_roots`, so we only need to insert the non-Public cases — but we
    // insert all parsed modifiers explicitly for clarity.
    let mut access_modifiers: HashMap<String, AccessModifier> = HashMap::new();
    for r in &ws.routines {
        let access = match r.access_modifier.as_deref() {
            Some("local") => AccessModifier::Local,
            Some("internal") => AccessModifier::Internal,
            // "protected" / None / any other value → public (al-sem default-access).
            _ => AccessModifier::Public,
        };
        access_modifiers.insert(r.id.clone(), access);
    }
    // See `DetectorContext::internal_reachable_externally` doc: defaults to false
    // (the Rust model carries no `primaryInternalsVisibleTo`; source-only fixtures
    // never set `internalsVisibleTo`).
    let internal_reachable_externally = false;
    let reachable_roots: BTreeSet<String> = crate::engine::l5::entry_points::find_reachable_roots(
        &ws.routines,
        &dep_routine_ids,
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
                &dep_routine_ids,
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
    let event_flow_indexes = build_event_flow_indexes(&event_graph, &ws.routines, &dep_routine_ids);

    // Cross-extension subscriber lookup, shared by d43/d44/d45 — previously each
    // rebuilt this identically from `ctx.event_graph` + `ws.objects` per run.
    let cross_extension_subscribers =
        crate::engine::l5::event_flow::build_cross_extension_subscribers(&event_graph, &ws.objects);

    // `calls.edges` is not read after this point (`calls.upgraded_bindings` still
    // is), so take the edges by value instead of cloning each retained one.
    let resolved_call_edge_by_callsite =
        first_resolved_edge_per_callsite(std::mem::take(&mut calls.edges));

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
    // summaries here from the SAME combined graph the cone used: Tarjan SCC over
    // `graph.edges_by_from`, then the closed-form v2 solver (`compute_summaries_v2`).
    // This is the only place that needs the core uncertainties; the union is
    // assembled once and exposed on `uncertainties_by_node`.
    //
    // SUBSTRATE-GATED on CORE_SUMMARIES. This is the second Tarjan + v2-solver
    // pass — the most expensive substrate. Skipped ⇒ `uncertainties_by_node` /
    // `parameter_roles_by_routine` / `summarize_diagnostics` are all empty, which by
    // decision (a) means a substrate-skipping run emits no summarize cap-hit
    // diagnostics (they are only ever produced by this `compute_summaries_v2` call).
    #[allow(clippy::type_complexity)]
    let (
        uncertainties_by_node,
        uncertainties,
        parameter_roles_by_routine,
        summarize_diagnostics,
        db_effect_bundle,
    ): (
        HashMap<String, UncertaintySetId>,
        UncertaintyIndex,
        HashMap<String, Vec<RecordRoleSummary>>,
        Vec<crate::engine::l4::summary_runner::SummarizeDiagnostic>,
        Option<SummaryBundle>,
    ) = if demanded & substrate::CORE_SUMMARIES != 0 {
        let scc = {
            let _s = pt::span("context", "context.core_scc_tarjan");
            let mut scc_adjacency: HashMap<String, Vec<String>> = HashMap::new();
            for (from, list) in &graph.edges_by_from {
                scc_adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
            }
            tarjan_scc(&SccInputGraph {
                nodes: &graph.nodes,
                edges_by_from: &scc_adjacency,
            })
        };

        // STRUCTS one-shot: SCC population stats + the largest-SCC's intra-edge
        // anatomy (which edge KIND fuses the biggest component, member outdegree).
        // Reimplements the historical `SCCSTATS`/`SCCANATOMY` eprintln probe
        // (`git show c8836e7^:src/engine/l5/detector_context.rs`) as one lazy JSON
        // payload — `build` below only runs when tracing is actually enabled.
        pt::instant_lazy("l4", "scc_stats", || {
            let max_scc = scc.sccs.iter().map(|s| s.members.len()).max().unwrap_or(0);
            let recursive_sccs = scc.sccs.iter().filter(|s| s.recursive).count();
            let recursive_members: usize = scc
                .sccs
                .iter()
                .filter(|s| s.recursive)
                .map(|s| s.members.len())
                .sum();
            let largest_scc = scc.sccs.iter().max_by_key(|s| s.members.len()).map(|big| {
                let member_set: std::collections::HashSet<&str> =
                    big.members.iter().map(|s| s.as_str()).collect();
                let mut kind_counts: std::collections::BTreeMap<&str, usize> =
                    std::collections::BTreeMap::new();
                let mut intra_edges = 0usize;
                let mut max_intra_outdegree = 0usize;
                for m in &big.members {
                    let mut out_here = 0usize;
                    if let Some(edges) = graph.edges_by_from.get(m) {
                        for e in edges {
                            if member_set.contains(e.to.as_str()) {
                                intra_edges += 1;
                                out_here += 1;
                                *kind_counts.entry(e.kind.as_str()).or_insert(0) += 1;
                            }
                        }
                    }
                    max_intra_outdegree = max_intra_outdegree.max(out_here);
                }
                json!({
                    "members": big.members.len(),
                    "intra_edges": intra_edges,
                    "max_intra_outdegree": max_intra_outdegree,
                    "kinds": kind_counts,
                })
            });
            json!({
                "nodes": graph.nodes.len(),
                "sccs": scc.sccs.len(),
                "recursive_sccs": recursive_sccs,
                "recursive_members": recursive_members,
                "max_scc": max_scc,
                "largest_scc": largest_scc,
            })
        });

        let _summaries_span = pt::span("context", "context.compute_summaries");
        // Field-resolution index (keyed (tableId, lowercased field name)) — mirrors
        // summary.rs `run_and_project`; parameterRoles need it, uncertainties don't,
        // but `compute_summaries_v2` takes it.
        let _fi_t = summaries_census::start();
        let mut field_index: FieldIndex = HashMap::new();
        for table in &ws.tables {
            for field in &table.fields {
                field_index
                    .entry((table.id.clone(), field.name.to_lowercase()))
                    .or_insert_with(|| field.id.clone());
            }
        }
        summaries_census::add_since(&summaries_census::FIELD_INDEX_NANOS, _fi_t);
        summaries_census::add(
            &summaries_census::FIELD_INDEX_ENTRIES,
            field_index.len() as u64,
        );
        // v2 db-effect solver — ⟨Task B1⟩ the LEAN bundle entry point: it returns the
        // compact `SummaryBundle` PLUS a `core_summaries` map whose `db_effects` are
        // EMPTY (never re-materialized) while `.uncertainties` / `.parameter_roles` are
        // fully populated — the only fields this path reads below. The db-effect rows
        // stay queryable on demand via `db_effect_bundle` (held on the ctx). This drops
        // the compat shim's ~24 GB / ~74 s per-routine `Vec<DbEffect>` expansion that no
        // detector consumed. `summarize_diagnostics` carries the ROLES fixpoint's
        // cap-hit backstop (empty on the corpus — roles converge); the db_effects path
        // is closed-form and never caps.
        let (db_effect_bundle, mut core_summaries, summarize_diagnostics) =
            compute_summaries_v2_bundle(
                &ws.routines,
                &graph,
                &scc,
                &calls.upgraded_bindings,
                &field_index,
            );
        drop(_summaries_span);
        // ⟨substrate census⟩ `ALSEM_SUMMARIES_CENSUS=1` — the phase split inside the
        // span that just closed. Emitted here, at the span boundary, so the report
        // and the span it attributes are read together.
        summaries_census::report();

        // ⟨fix wave finding 2⟩ `compute_summaries_v2_bundle` is the LEAN entry point
        // (⟨Task B1⟩ — see its own doc): `core_summaries`' `db_effects` field stays
        // EMPTY, with the real rows living compactly in `db_effect_bundle` instead.
        // Neither `perf_bounds.rs` L4 gate would notice this call site regressing to
        // a materializing entry point (`compute_summaries_v2` /
        // `compute_summaries_v2_with_leaves_core`, or an inline rebuild of their
        // db_effects-filling loop) — both gates call `compute_summaries_v2_bundle`
        // directly, bypassing this call site entirely. This assert is what catches
        // THAT regression: deterministic, machine-independent, no timing involved.
        // See `core_summaries_stay_lean_while_the_bundle_carries_the_db_effect_rows`
        // (this module's test module) for the corpus that exercises it for real.
        debug_assert!(
            core_summaries.values().all(|s| s.db_effects.is_empty()),
            "build_detector_context's core_summaries must carry EMPTY db_effects — a \
             non-empty row means this call site regressed to a materializing summary \
             entry point, reintroducing the ~24 GB / ~74 s per-routine Vec<DbEffect> \
             re-materialization ⟨Task B1⟩ (commit a0cd348) removed"
        );

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
            summarize_diagnostics,
            Some(db_effect_bundle),
        )
    } else {
        (
            HashMap::new(),
            UncertaintyIndex::default(),
            HashMap::new(),
            Vec::new(),
            None,
        )
    };

    // ⟨Task 6⟩ The reverse transpose — built ONLY on explicit demand. Not in
    // `substrate::ALL`, so the four `ALL`-passing non-registry callers never pay
    // it, and `run_detectors` (which passes `demanded`, the fold of the selected
    // detectors' `requires`) can never reach it either, since no detector may
    // declare the bit. Unset ⇒ this is one `None` write: no allocation, no pass,
    // and — because the span is INSIDE the branch — not even a trace entry.
    // The named span is deliberate: when it IS built, its cost lands in the perf
    // trace beside every other substrate rather than being guessed at.
    let reverse_effect_index = if demanded & substrate::DB_EFFECT_REVERSE_INDEX != 0 {
        let _s = pt::span("context", "context.reverse_effect_index");
        db_effect_bundle.as_ref().map(ReverseEffectIndex::build)
    } else {
        None
    };

    let _final_indexes_span = pt::span("context", "context.final_indexes");
    let mut call_site_by_id: HashMap<&str, &PCallSite> = HashMap::new();
    for r in &ws.routines {
        for cs in &r.call_sites {
            call_site_by_id.insert(cs.id.as_str(), cs);
        }
    }

    // Expose the resolver's post-upgrade bindings (the `upgradeBindings` side
    // table) keyed by callsite id — the join target for d37/d39 which read
    // `binding.bindingResolution` / `binding.calleeParameterIsVar`. `compute_summaries_v2`
    // above was the last reader of `calls.upgraded_bindings`, so move it out here.
    let upgraded_bindings_by_callsite: HashMap<String, Vec<UpgradedBinding>> =
        std::mem::take(&mut calls.upgraded_bindings);

    // R4-F root classifications — keyed by internal RoutineId for d50/d51 lookup.
    let root_classifications_by_routine: HashMap<
        String,
        crate::engine::root_classification::RootClassification,
    > = resolved
        .root_classifications
        .iter()
        .map(|rc| (rc.routine_id.clone(), rc.clone()))
        .collect();

    // R4-F Stage-5b ordering facts — computed lazily on first
    // `get_ordering_facts()` access (see field doc). Keyed by StableRoutineId;
    // d47/d49/d51 read it via `get_ordering_facts()`.

    let fingerprint_index =
        crate::engine::l5::fingerprint::FingerprintIndex::build(&ws.routines, &ws.objects);
    drop(_final_indexes_span);

    // ⟨substrate census⟩ `ALSEM_UNCERTAINTY_CENSUS=1` — see `UncertaintyIndex::emit_census`.
    uncertainties.emit_census(&uncertainties_by_node);

    DetectorContext {
        graph,
        event_graph,
        routine_by_id,
        objects_by_id,
        table_by_id,
        reverse_call_graph,
        entry_points,
        transaction_spans,
        resolved_call_edge_by_callsite,
        uncertainty_edges_by_from,
        uncertainties_by_node,
        uncertainties,
        call_site_by_id,
        summaries,
        cone_derived,
        event_flow_indexes,
        parameter_roles_by_routine,
        upgraded_bindings_by_callsite,
        reachable_roots,
        internal_reachable_externally,
        // Source-only: no deps → every routine primary, no declared deps, no versions.
        dep_routine_ids: BTreeSet::new(),
        declared_dependencies: Vec::new(),
        app_versions: HashMap::new(),
        root_classifications_by_routine,
        ordering_facts: std::sync::OnceLock::new(),
        ordering_source: Some(resolved),
        closed_world_temp_params,
        summarize_diagnostics,
        db_effect_bundle,
        reverse_effect_index,
        fingerprint_index,
        cross_extension_subscribers,
    }
}

/// Build the shared context for a CROSS-APP run from a pre-assembled
/// `R3a5CrossAppBase` (the merged workspace+dep model + cross-app combined graph +
/// `dep_routine_ids`). Mirrors `build_detector_context` but reads every substrate
/// from `base` instead of recomputing source-only, and threads `dep_routine_ids`
/// into the entry-point / reachable-root / transaction-span / event-flow builders so
/// dep routines are NOT treated as primary roots. d13/d16/d17 read
/// `dep_routine_ids` (the roleOf gate), `declared_dependencies` + `app_versions`
/// (d17), and the eager indexes; the path-walker substrate (uncertainties /
/// summaries) is built identically for any future cross-app detector.
///
/// `root_classifications` are EMPTY here; `ordering_source` is `None` here (ordering
/// facts lazily resolve to EMPTY — d13/d16/d17 never read them; the base does not
/// carry the resolved-model classifier inputs). A future cross-app ordering detector
/// would thread them additively.
pub(crate) fn build_detector_context_cross_app(
    base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
) -> DetectorContext<'_> {
    use crate::engine::l4::summary_runner::compute_summaries_v2_bundle_with_leaves;

    let ws_routines = &base.ws_routines;
    let dep_routine_ids = &base.dep_routine_ids;
    let graph = base.graph.clone();

    // Cone over the merged graph (direct facts/coverage already assembled in `base`).
    // ⟨C1 Task 3⟩ `DerivedOnly`, unconditionally. Unlike the source-only builder
    // there is no mode choice to make here: this context is reachable ONLY from
    // `registry::run_detectors_cross_app` (its single caller), i.e. from the
    // detector path, and no detector reads raw inherited facts. The one consumer
    // that does — `gate::policy` — builds its context through the SOURCE-ONLY
    // `build_detector_context` (`gate/policy/pipeline.rs`), never this one. Adding
    // a `demanded` parameter here would therefore only add a branch that no caller
    // can ever take.
    let outcome = compose_cone_over_graph(
        &base.graph,
        &base.nodes,
        &base.direct_full,
        &base.direct_coverage,
        &base.ws_routines,
        ConeOutput::DerivedOnly,
    );
    let cones = outcome.cones;
    let cone_derived = outcome.derived;
    let empty_facts: Vec<CapabilityFact> = Vec::new();
    let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
    for r in ws_routines {
        // ⟨C1 Task 3, carry #2⟩ The `inherited` FIELD POPULATION is gone; the
        // `cones.get()` is NOT. Switching this to `cones.remove()` would import
        // the G-18 routine-id-collision degeneracy that the source-only builder
        // accepts and this builder does not — a real output change, not a
        // refactor. `coverage` still comes off the same borrowed entry.
        let cone = cones.get(&r.id);
        let coverage = cone.map(|c| c.coverage.clone());
        summaries.insert(
            r.id.clone(),
            FullRoutineSummary::new(
                r.id.clone(),
                base.direct_full.get(&r.id).unwrap_or(&empty_facts).clone(),
                None,
                coverage,
            ),
        );
    }

    // --- Eager indexes (over the merged routine/object/table sets) ---------
    let routine_by_id: HashMap<&str, &L3Routine> =
        ws_routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let objects_by_id: HashMap<&str, &L3Object> =
        base.objects.iter().map(|o| (o.id.as_str(), o)).collect();
    // G-5: REAL table wins an id collision with a tableextension stub.
    let table_by_id: HashMap<&str, &L3Table> =
        crate::engine::l3::l3_workspace::table_by_id_preferring_real(&base.tables);

    let reverse_call_graph = build_reverse_call_graph(&graph);

    let entry_points: BTreeSet<String> =
        crate::engine::l5::entry_points::find_entry_points(ws_routines, dep_routine_ids)
            .into_iter()
            .collect();

    let mut access_modifiers: HashMap<String, AccessModifier> = HashMap::new();
    for r in ws_routines {
        let access = match r.access_modifier.as_deref() {
            Some("local") => AccessModifier::Local,
            Some("internal") => AccessModifier::Internal,
            _ => AccessModifier::Public,
        };
        access_modifiers.insert(r.id.clone(), access);
    }
    let internal_reachable_externally = false;
    let reachable_roots: BTreeSet<String> = crate::engine::l5::entry_points::find_reachable_roots(
        ws_routines,
        dep_routine_ids,
        &access_modifiers,
        internal_reachable_externally,
    )
    .into_iter()
    .collect();

    // G-19 — closed-world proven-temp params (see the source-only builder).
    // Dep routines carry `access_modifier: None` (the ABI does not expose it),
    // so they can never be proven; primary `local` procedures still can.
    let closed_world_temp_params =
        crate::engine::l5::closed_world_temp::prove_closed_world_temp_params(
            ws_routines,
            &graph,
            &reverse_call_graph,
            &entry_points,
        );

    let transaction_spans = compute_transaction_spans(
        ws_routines,
        dep_routine_ids,
        &reverse_call_graph,
        &summaries,
        &cone_derived,
    );

    let event_flow_indexes =
        build_event_flow_indexes(&base.event_graph, ws_routines, dep_routine_ids);

    // Cross-extension subscriber lookup, from the SAME inputs as `event_flow_indexes`
    // above — `base.event_graph` + `base.objects` (the merged cross-app event graph
    // + object set), consistent with how `fingerprint_index` below anchors to `base`.
    let cross_extension_subscribers =
        crate::engine::l5::event_flow::build_cross_extension_subscribers(
            &base.event_graph,
            &base.objects,
        );

    // Resolved-call-edge-by-callsite index: EMPTY for the cross-app context. The
    // cross-app build does not retain the raw resolver `calls.edges`, and d13/d16/d17
    // read edges directly off `ctx.graph` (the combined graph). Future cross-app
    // detectors that need this index would thread `calls` through `R3a5CrossAppBase`.
    let resolved_call_edge_by_callsite: HashMap<
        String,
        crate::engine::l3::call_resolver::CallEdge,
    > = HashMap::new();

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

    // Core summaries (v2 db-effect solver WITH dep leaves) for the path-walker
    // uncertainty union + parameter roles — same as project_r3a5_cross_app's core.
    // ⟨Task B1⟩ the LEAN bundle entry point: `core_summaries` carries EMPTY
    // `db_effects` (never re-materialized) with `.uncertainties` / `.parameter_roles`
    // populated — the only fields read below — while the compact rows stay queryable
    // via `db_effect_bundle` (held on the ctx). `summarize_diagnostics` carries the
    // roles fixpoint's cap-hit backstop (empty on the corpus).
    let (db_effect_bundle, mut core_summaries, summarize_diagnostics) =
        compute_summaries_v2_bundle_with_leaves(
            ws_routines,
            &graph,
            &base.combined_scc,
            &base.upgraded_bindings,
            &base.field_index,
            &base.leaf_summaries,
        );

    // One drained pass for both harvests, hash-consing the per-node uncertainty sets
    // — the exact shape `build_detector_context` uses; see its own comment for why
    // `processed` is required (colliding internal routine ids) and why draining is
    // equivalent to the old borrow-and-clone form. Membership here stays
    // `ws_routines`-driven for BOTH maps, matching what the two loops this replaces
    // produced: the cross-app `parameter_roles_by_routine` never covered
    // `core_summaries` keys outside `ws_routines`, so there is no trailing drain.
    let mut parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>> = HashMap::new();
    let mut uncertainties_by_node: HashMap<String, UncertaintySetId> = HashMap::new();
    let mut uncertainties = UncertaintyIndex::default();
    let mut processed: HashSet<&str> = HashSet::new();
    for r in ws_routines {
        if !processed.insert(r.id.as_str()) {
            continue;
        }
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

    let mut call_site_by_id: HashMap<&str, &PCallSite> = HashMap::new();
    for r in ws_routines {
        for cs in &r.call_sites {
            call_site_by_id.insert(cs.id.as_str(), cs);
        }
    }

    let upgraded_bindings_by_callsite: HashMap<String, Vec<UpgradedBinding>> =
        base.upgraded_bindings.clone();

    // Build the fingerprint index from `base.ws_routines`/`base.objects` — NOT from
    // the throwaway `merged_workspace_view` (registry.rs's `run_detectors_cross_app`
    // clones the merged sets into a local `L3Resolved` it builds AFTER calling this
    // function, so that clone doesn't exist yet at this point and can't be borrowed
    // from here anyway). `base: &'a R3a5CrossAppBase` is already the ctx's own
    // borrow source for every other eager index above, so anchoring the fingerprint
    // index to it too keeps the lifetime honest — the same 'a the whole ctx uses.
    let fingerprint_index =
        crate::engine::l5::fingerprint::FingerprintIndex::build(&base.ws_routines, &base.objects);

    let app_versions: HashMap<String, String> = base.resolved_app_versions.clone();
    let declared_dependencies: Vec<DeclaredDep> = base
        .declared_dependencies
        .iter()
        .map(|d| DeclaredDep {
            app_guid: d.app_guid.clone(),
            name: d.name.clone(),
            min_version: d.min_version.clone(),
        })
        .collect();

    // ⟨substrate census⟩ `ALSEM_UNCERTAINTY_CENSUS=1` — see `UncertaintyIndex::emit_census`.
    uncertainties.emit_census(&uncertainties_by_node);

    DetectorContext {
        graph,
        event_graph: base.event_graph.clone(),
        routine_by_id,
        objects_by_id,
        table_by_id,
        reverse_call_graph,
        entry_points,
        transaction_spans,
        resolved_call_edge_by_callsite,
        uncertainty_edges_by_from,
        uncertainties_by_node,
        uncertainties,
        call_site_by_id,
        summaries,
        cone_derived,
        event_flow_indexes,
        parameter_roles_by_routine,
        upgraded_bindings_by_callsite,
        reachable_roots,
        internal_reachable_externally,
        dep_routine_ids: dep_routine_ids.clone(),
        declared_dependencies,
        app_versions,
        root_classifications_by_routine: HashMap::new(),
        ordering_facts: std::sync::OnceLock::new(),
        ordering_source: None,
        closed_world_temp_params,
        summarize_diagnostics,
        db_effect_bundle: Some(db_effect_bundle),
        // ⟨Task 6⟩ The cross-app context takes no `demanded` mask, so there is
        // no way to ask it for the transpose — and no cross-app consumer wants
        // one (d13/d16/d17 read no db effects). `None` keeps this path free.
        reverse_effect_index: None,
        fingerprint_index,
        cross_extension_subscribers,
    }
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
fn first_resolved_edge_per_callsite(edges: Vec<CallEdge>) -> HashMap<String, CallEdge> {
    let mut by_callsite: HashMap<String, CallEdge> = HashMap::new();
    for ce in edges {
        if ce.to.is_none() {
            continue;
        }
        by_callsite.entry(ce.callsite_id.clone()).or_insert(ce);
    }
    by_callsite
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S3.6 audit, precondition by assignment: one call site with two targeted
    /// edges (an interface site with two implementers) and a to-less one first.
    /// The map keeps the first TARGETED edge, never the to-less one or the last.
    #[test]
    fn the_first_targeted_edge_represents_a_call_site() {
        let mk = |to: Option<&str>| {
            let mut e = CallEdge::base("caller", "cs1", "op1");
            e.to = to.map(str::to_string);
            e
        };
        let map = first_resolved_edge_per_callsite(vec![mk(None), mk(Some("a")), mk(Some("b"))]);
        assert_eq!(map.len(), 1);
        assert_eq!(map["cs1"].to.as_deref(), Some("a"));
    }

    /// Laziness contract: `build_detector_context` must NOT compute ordering facts
    /// (the OnceLock starts empty); first `get_ordering_facts()` call computes and
    /// memoizes a map EQUAL to a direct `compute_ordering_facts(resolved)` run.
    #[test]
    fn ordering_facts_are_lazy_and_parity_with_direct_compute() {
        // Empty workspace: cheap, and exercises the full lazy path end-to-end.
        let resolved = crate::engine::l3::l3_workspace::L3Resolved {
            workspace: crate::engine::l3::l3_workspace::L3Workspace {
                objects: Vec::new(),
                tables: Vec::new(),
                routines: Vec::new(),
            },
            root_classifications: Vec::new(),
            primary_app: None,
            infra_diagnostics: Vec::new(),
            precomputed_calls: None,
        };
        let ctx = build_detector_context(&resolved, crate::engine::l5::registry::substrate::ALL);
        assert!(
            ctx.ordering_facts.get().is_none(),
            "ordering facts must not be computed eagerly"
        );
        let via_ctx = ctx.get_ordering_facts();
        let direct = crate::engine::l5::ordering_facts::compute_ordering_facts(&resolved);
        assert_eq!(via_ctx.len(), direct.len());
        assert!(
            ctx.ordering_facts.get().is_some(),
            "first access must memoize"
        );
    }

    /// The seam is USED, not just present: a hand-made `ResolvedCalls` attached to
    /// an empty workspace (which on its own resolves ZERO edges) must show up in
    /// BOTH the detector context and the ordering-facts substrate
    /// (`compose_snapshot` → `build_r3a3_source_only_base`). Reverting either site
    /// to `resolve_calls` makes its assertion fail.
    #[test]
    fn precomputed_calls_reach_detector_context_and_ordering_base() {
        use crate::engine::l3::call_resolver::{CallEdge, ResolvedCalls};
        use crate::engine::l3::taxonomy::{DispatchKind, Resolution};
        let mut edge = CallEdge::base("from-r", "distinct-callsite", "distinct-op");
        edge.to = Some("to-r".to_string());
        // Dynamic so the coverage path (`dynamic_dispatch_sites`) shows it too.
        edge.dispatch_kind = DispatchKind::Dynamic;
        edge.resolution = Resolution::Resolved;
        let resolved = crate::engine::l3::l3_workspace::L3Resolved {
            workspace: crate::engine::l3::l3_workspace::L3Workspace {
                objects: Vec::new(),
                tables: Vec::new(),
                routines: Vec::new(),
            },
            root_classifications: Vec::new(),
            primary_app: None,
            infra_diagnostics: Vec::new(),
            precomputed_calls: Some(std::sync::Arc::new(ResolvedCalls {
                edges: vec![edge],
                upgraded_bindings: HashMap::new(),
                diagnostics: Vec::new(),
                external_targets: Vec::new(),
            })),
        };
        let ctx = build_detector_context(&resolved, crate::engine::l5::registry::substrate::ALL);
        assert!(
            ctx.resolved_call_edge_by_callsite
                .contains_key("distinct-callsite"),
            "build_detector_context ignored precomputed_calls"
        );
        let base = crate::engine::l4::capability_cone::build_r3a3_source_only_base(&resolved);
        assert!(
            base.calls
                .edges
                .iter()
                .any(|e| e.callsite_id == "distinct-callsite"),
            "the ordering-facts base ignored precomputed_calls"
        );
        let coverage = resolved.project_coverage(&[], &[]);
        assert!(
            coverage
                .dynamic_dispatch_sites
                .iter()
                .any(|s| s.contains("distinct-op")),
            "project_coverage ignored precomputed_calls"
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
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
        let resolved = assemble_and_resolve_default(&files, "11111111-0000-0000-0000-0000000cp001");
        let ctx = build_detector_context(&resolved, substrate::SUMMARIES);

        // Without this the whole test would pass identically on an EMPTY store —
        // every `flags_of` would read the empty row for the trivial reason.
        assert!(
            !ctx.cone_derived.is_empty(),
            "precondition: the derived store must actually hold rows, or the \
             assertions below are vacuous"
        );

        let on_actions: Vec<&crate::engine::l3::l3_workspace::L3Routine> = resolved
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
    /// two `L3Routine`s built from ordinary, non-colliding source are forced to carry
    /// the literal same `id` by direct field assignment after assembly, before
    /// `build_detector_context` runs. That holds under ANY id schema, forever,
    /// because it does not depend on what the schema would have produced.
    #[test]
    fn hand_stated_id_collision_keeps_a_real_summary_and_derived_row() {
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
        let mut resolved =
            assemble_and_resolve_default(&files, "22222222-0000-0000-0000-0000000cp002");

        // State the collision by hand: force the two `OnAction` triggers to carry the
        // literal SAME id via direct field assignment — never by relying on
        // `compute_routine_id` happening to agree (it does today; it need not
        // tomorrow). `build_detector_context` re-derives everything else (symbol
        // table, call resolution, combined graph) from `resolved.workspace.routines`
        // fresh on every call, reading whatever id is on each routine AT CALL TIME —
        // so overwriting here, before that call, is sufficient to force the collision
        // all the way through the pipeline it exercises.
        const SHARED_ID: &str = "hand-stated-collision-id";
        let mut forced = 0usize;
        for r in resolved.workspace.routines.iter_mut() {
            if r.name.eq_ignore_ascii_case("OnAction") {
                r.id = SHARED_ID.to_string();
                forced += 1;
            }
        }
        assert_eq!(
            forced, 2,
            "fixture precondition: both OnAction trigger bodies must be in the model"
        );

        let ctx = build_detector_context(&resolved, substrate::SUMMARIES);

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

    /// ⟨fix wave FIX 1, final-branch-review finding 2⟩ Discriminates the call-site
    /// regression the review flagged: both `perf_bounds` L4 gates call
    /// `compute_summaries_v2_bundle` directly, so neither would notice if
    /// `build_detector_context` stopped calling it. On a fixture guaranteed to
    /// produce a REAL db-effect population (`Setup.Insert()` — `Insert` is
    /// db-touching, `summary_runner::is_db_touching`), this test proves the bundle
    /// side of the B1 invariant is populated; the production `debug_assert!` right
    /// after `compute_summaries_v2_bundle` in `build_detector_context` proves the
    /// `core_summaries` side stays empty — and is exercised for REAL here (not
    /// vacuously, the way it would be against an empty workspace). If the call
    /// site ever regresses to a materializing entry point, that debug_assert
    /// panics and this test fails (verified by temporarily re-pointing the call
    /// site — see `.superpowers/sdd/minors-report.md`'s Fix wave section for the
    /// before/after run).
    #[test]
    fn core_summaries_stay_lean_while_the_bundle_carries_the_db_effect_rows() {
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
        let resolved = assemble_and_resolve_default(&files, "11111111-0000-0000-0000-0000000fx001");
        // CORE_SUMMARIES alone is enough to reach the `compute_summaries_v2_bundle`
        // call (gated independently of SUMMARIES — see `build_detector_context`'s
        // own doc), so this test does not need the cone/SUMMARIES substrate at all.
        let ctx = build_detector_context(&resolved, substrate::CORE_SUMMARIES);

        let bundle = ctx
            .db_effect_bundle
            .as_ref()
            .expect("CORE_SUMMARIES was demanded — the bundle must exist");
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
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
        let resolved = assemble_and_resolve_default(&files, "11111111-0000-0000-0000-0000000fx002");

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
        let ctx_all = build_detector_context(&resolved, substrate::ALL);
        assert!(
            ctx_all.db_effect_bundle.is_some(),
            "ALL includes CORE_SUMMARIES, so the bundle IS built — which is what \
             makes the next assertion meaningful rather than vacuous"
        );
        assert!(
            ctx_all.reverse_effect_index.is_none(),
            "substrate::ALL must NOT build the reverse index"
        );

        // POSITIVE: ask for it explicitly and it appears, correct and populated.
        let ctx = build_detector_context(
            &resolved,
            substrate::CORE_SUMMARIES | substrate::DB_EFFECT_REVERSE_INDEX,
        );
        let bundle = ctx
            .db_effect_bundle
            .as_ref()
            .expect("CORE_SUMMARIES demanded");
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
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
        let resolved = assemble_and_resolve_default(&files, "44444444-0000-0000-0000-0000000hc001");
        let ctx = build_detector_context(&resolved, substrate::CORE_SUMMARIES);

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
    /// and the bug would not manifest) AND calls `Touch()`, which makes a further
    /// unresolved call (⇒ an INHERITED uncertainty that lives ONLY in the core
    /// summary). The union must therefore carry a `Touch`-owned callsite.
    #[test]
    fn colliding_ids_keep_the_full_summary_union_not_just_the_edges() {
        use crate::engine::l3::l3_workspace::assemble_and_resolve_default;
        use crate::engine::l5::registry::substrate;

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
    begin
        Setup.Insert();
        MissingDeeper();
    end;
}
"#;
        let files = vec![("src/CP3Wizard.al".to_string(), src.to_string())];
        let mut resolved =
            assemble_and_resolve_default(&files, "33333333-0000-0000-0000-0000000cp003");

        const SHARED_ID: &str = "hand-stated-uncertainty-collision-id";
        let mut forced = 0usize;
        for r in resolved.workspace.routines.iter_mut() {
            if r.name.eq_ignore_ascii_case("OnAction") {
                r.id = SHARED_ID.to_string();
                forced += 1;
            }
        }
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

        let ctx = build_detector_context(&resolved, substrate::CORE_SUMMARIES);

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
                u.callsite_id
                    .as_deref()
                    .is_some_and(|cs| cs.starts_with(&format!("{touch_id}/")))
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
}
