//! `ReverseEffectIndex` — the bidirectional (routine <-> effect/table) query
//! index over the compact db-effect store A3 built (spec Part A Step 4 —
//! `docs/superpowers/specs/2026-07-22-l4-dbeffect-store-and-retirement-design.md`,
//! lines 189-211). ADDITIVE: this module only READS a finished
//! [`SummaryBundle`] (via the accessors [`SummaryBundle::routines_with_rows`]/
//! [`SummaryBundle::routine_id`]/[`SummaryBundle::pd_delta_ids`] added
//! alongside it in `effect_store.rs`) — it changes NOTHING about db-effect
//! output, so the v2-vs-old differential stays byte-identical.
//!
//! ## Two SCC notions — kept DISTINCT (⟨rev⟩ spec:194-198)
//!
//! [`EffectClassIx`] is the effect-SHARING effective SCC: the grouping A3
//! already forms when it hash-conses routines' `terminal_base` into a shared
//! [`EffectSetId`] (fixed leaves/missing routines are removed from the
//! induced subgraph BEFORE Tarjan re-runs on it —
//! [`crate::engine::l4::db_effect_solver::effective_sccs`] — so ONE original
//! Tarjan SCC can SPLIT into several effect classes). [`GraphSccIx`] is the
//! ORIGINAL call-graph Tarjan condensation
//! ([`SccResult::scc_id_by_routine`]), UNCHANGED by leaf removal. The effect
//! transpose below (`down`/`touches_table`/`up_table`) uses `EffectClassIx`
//! exclusively; the ancestor-scoped query ("callers of R that touch X") MUST
//! use `GraphSccIx` for its reverse-DAG BFS instead — effective-SCC
//! leaf-removal changes REACHABILITY (an edge through the removed leaf
//! disappears), so the two are never interchangeable. That query is
//! IMPLEMENTED, in
//! [`crate::engine::l4::effect_query::DbEffectQuery::ancestors_touching`]
//! (Task 6) — it holds exactly this discipline: steps 1-2 (reverse
//! condensation + BFS) walk `GraphSccIx`, step 3 (does this ancestor touch T?)
//! goes through `EffectClassIx` via `touches_table`.
//! [`class_of`]/[`graph_scc_of`] are free functions, not `ReverseEffectIndex`
//! fields — each is a cheap live projection off `SummaryBundle`/`SccResult`,
//! so there is only ONE place either notion is computed (no second,
//! driftable copy baked into the index).
//!
//! ## Posting-list representation (spec Step 4: "adapt sorted-vec / dense-
//! bitmap / Roaring by cardinality")
//!
//! `roaring` was evaluated (arc constraint: optional, decide by measurement)
//! and NOT adopted as a new dependency — this module's posting-list
//! cardinalities (routines or classes touching one effect/table) are modest
//! relative to the domain sizes the spec's own worked numbers use (~12.5KB
//! for a FULL dense bitmap over a ~100k-routine domain), so a plain sorted
//! `Box<[u32]>` below [`POSTING_SPARSE_THRESHOLD`] and a dense `Box<[u64]>`
//! bitmap at/above it (the SAME shape as `effect_store::HybridEffectSet`, but
//! over the `RoutineIx`/`EffectClassIx` domains rather than the frozen
//! `EffectId` universe — see [`PostingList`] for why it is a separate, small
//! type rather than a reuse of `HybridEffectSet`) already gives O(1)-ish
//! membership with no new dependency. Revisit only if profiling on a real
//! workspace shows a measured win `roaring` would close.
//!
//! ## Build cost (spec Step 4: "One transpose pass over each SCC base")
//!
//! [`ReverseEffectIndex::build`] iterates DISTINCT classes (one hash-consed
//! [`EffectSetId`] each) to populate `effect_to_sccs`/`table_to_sccs`, NOT
//! the 7.1M routine memberships A3's sharing collapsed away — the same
//! complexity-class win A3 made for storage applies to the reverse build.
//!
//! The transpose passes key their working maps on `&str` BORROWED from the
//! frozen universe (which outlives the build), allocating a `String` only for
//! the `<= n_tables` surviving map keys. The pre-Task-6 shape cloned the table
//! id TWICE per (class, effect) membership — 2·B allocations, ~131k on DO and
//! an extrapolated ~4M (~190 MB of churn) at 8020 scale, landing in peak RSS
//! inside an arc whose point was killing a 24 GB allocation. `B` is
//! Σ-over-distinct-classes of base cardinality, so that cost scaled with the
//! transpose work, not with the (much smaller) table count it produced.
//!
//! ## Two rules every consumer must hold (they exist nowhere else)
//!
//! 1. **[`ReverseEffectIndex::class_members`] is an INTERNAL expansion vehicle
//!    — never render it, and never describe it as "the routines in this
//!    cycle".** [`EffectClassIx`] IS [`EffectSetId`], so two effective SCCs
//!    with byte-identical terminal bases hash-cons into ONE class and
//!    `class_members` returns the UNION of both SCCs' members — routines with
//!    no call relationship at all. Every query this module ships stays correct
//!    under that collapse (both SCCs genuinely carry the base, so every
//!    routine `up_table`/`up_effect` returns genuinely touches the table, and
//!    `touches_table` never flips); it is purely an expansion vehicle, and
//!    nothing here derives reachability or cycle membership from it.
//! 2. **Membership comes from the index; WITNESSES come from the bundle.** The
//!    posting lists discard [`crate::engine::l4::effect_store::ViaRank`], and
//!    98.8% of real memberships (DO: 219 727 / 222 483) are `via: inherited` —
//!    so a surface that answers "does R touch T?" from postings alone and
//!    stops there has answered the less useful half of the question ("does
//!    THIS routine do it, or something twelve frames down?").
//!    [`crate::engine::l4::effect_query::DbEffectQuery`] is the facade that
//!    holds both halves together; prefer it over calling this module directly.
//!
//! ## Two things this module deliberately does NOT own
//!
//! - **`table_id` is not always a table.** `summary_runner`'s base extraction
//!   substitutes the literal sentinel `"unknown"` when a record operation's
//!   target table could not be determined (`summary_runner.rs`, the
//!   `op.table_id.clone().unwrap_or_else(|| "unknown".to_string())` line), and
//!   on DO that bucket is the LARGEST posting of all (1 334 routines). The
//!   index stores it verbatim — it is a real, queryable effect population —
//!   but any surface that renders a table must state what it is rather than
//!   showing it as though it were a table id. See
//!   [`crate::engine::l4::effect_query::UNKNOWN_TABLE_ID`].
//! - **[`RoutineIx`] is not a user-facing identity.**
//!   [`SummaryBundle::routine_id`] yields the INTERNAL id
//!   (`<appGuid>:Codeunit:6175271#<bodyhash>`). Rendering needs a join to
//!   `ModelRoutine` (`name` / `object_type` / `stable_routine_id` /
//!   `source_anchor`); that join belongs to the CONSUMER, not here — see
//!   `effect_query_cli.rs`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::engine::l4::effect_store::{DbEffectRef, EffectSetId, SummaryBundle};
use crate::engine::l4::effect_universe::EffectId;
use crate::engine::l4::routine_interner::RoutineIx;
use crate::engine::l4::scc::SccResult;

// ---------------------------------------------------------------------------
// EffectClassIx / GraphSccIx — the two distinct SCC notions.
// ---------------------------------------------------------------------------

/// The effect-SHARING effective SCC a routine's `terminal_base` belongs to —
/// literally [`EffectSetId`]'s own dense index space (every hash-consed set
/// IS one class by construction: A3 pushes exactly one arena entry per
/// effective SCC / retained fixed leaf, and every such entry has >= 1 member
/// row referencing it — see `effect_store.rs`'s `SummaryBundleBuilder` doc).
/// Kept as a DISTINCT type from [`EffectSetId`] (rather than reusing it bare)
/// so a reverse-index call site can never accidentally pass a raw storage id
/// where the "class a routine belongs to" notion is meant, and — the
/// load-bearing distinction — so it is never confused with [`GraphSccIx`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectClassIx(pub u32);

impl From<EffectSetId> for EffectClassIx {
    fn from(id: EffectSetId) -> Self {
        EffectClassIx(id.0)
    }
}

/// The ORIGINAL call-graph Tarjan SCC condensation index
/// ([`SccResult::scc_id_by_routine`]'s value) — UNAFFECTED by fixed-leaf/
/// missing-routine removal. Ancestor-scoped hover BFS must walk THIS
/// condensation, never [`EffectClassIx`]'s effect-sharing DAG (spec:196-198).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GraphSccIx(pub u32);

/// A routine's effect class — the class whose shared base it draws
/// `db_effects`'s terminal half from. `None` for a routine with no compact
/// row (never solved, never a retained leaf — spec: "Missing routines get no
/// row"). A thin, stateless projection off [`SummaryBundle::terminal_base`] —
/// NOT cached inside [`ReverseEffectIndex`], so there is exactly one place
/// this classification is computed.
pub fn class_of(bundle: &SummaryBundle, r: RoutineIx) -> Option<EffectClassIx> {
    bundle.terminal_base(r).map(EffectClassIx::from)
}

/// A routine's ORIGINAL Tarjan SCC id, from the call-graph condensation
/// `scc` carries — independent of whether `r` has a compact db-effect row at
/// all (every graph node has a Tarjan SCC; not every node has a row).
pub fn graph_scc_of(scc: &SccResult, bundle: &SummaryBundle, r: RoutineIx) -> Option<GraphSccIx> {
    let name = bundle.routine_id(r);
    scc.scc_id_by_routine
        .get(name)
        .map(|&ix| GraphSccIx(ix as u32))
}

// ---------------------------------------------------------------------------
// PostingList — cardinality-adaptive id set (sorted-vec / dense-bitmap).
// ---------------------------------------------------------------------------

/// Below this many entries a posting list stores a sorted `Box<[u32]>`; at or
/// above it, a dense bitmap. Mirrors `effect_store`'s own sparse/dense
/// threshold value (256) — the same sparse/dense crossover reasoning applies
/// (a 256-entry sorted array and a full dense word-array cost about the
/// same; above that, dense wins) — but this module keeps its OWN constant
/// and impl rather than reusing `HybridEffectSet`: that type's dense repr is
/// sized to a FROZEN `EffectId` universe length with tail-bit masking
/// against it, a contract that does not apply to a posting list's ad hoc
/// `RoutineIx`/`EffectClassIx` domain (see module doc).
const POSTING_SPARSE_THRESHOLD: usize = 256;

/// One posting list: the set of raw `u32` ids (a [`RoutineIx`] or
/// [`EffectClassIx`] value, unwrapped) referencing one effect or table.
/// Always constructed sorted + deduplicated ([`PostingList::from_ids`]), so
/// iteration is always ascending and membership never decompresses the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PostingList {
    Sparse(Box<[u32]>),
    Dense(Box<[u64]>),
}

impl PostingList {
    /// Build from an UNSORTED, possibly-duplicated id list — sorts, dedups,
    /// then picks sparse/dense by the resulting cardinality.
    fn from_ids(mut ids: Vec<u32>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        if ids.len() < POSTING_SPARSE_THRESHOLD {
            PostingList::Sparse(ids.into_boxed_slice())
        } else {
            let max = *ids
                .last()
                .expect("dense branch only reached when non-empty");
            let mut words = vec![0u64; (max as usize / 64) + 1];
            for id in &ids {
                words[(*id / 64) as usize] |= 1u64 << (*id % 64);
            }
            PostingList::Dense(words.into_boxed_slice())
        }
    }

    /// O(1)-ish membership test — a dense word lookup or a sorted-vec binary
    /// search. Never expands/collects the list.
    fn contains(&self, id: u32) -> bool {
        match self {
            PostingList::Sparse(ids) => ids.binary_search(&id).is_ok(),
            PostingList::Dense(words) => {
                let w = (id / 64) as usize;
                w < words.len() && (words[w] & (1u64 << (id % 64))) != 0
            }
        }
    }

    /// Ascending iteration, cost proportional to the cardinality (bit-scan
    /// for dense, direct for sparse) — never proportional to the domain size.
    fn iter(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        match self {
            PostingList::Sparse(ids) => Box::new(ids.iter().copied()),
            PostingList::Dense(words) => Box::new(iter_bits(words)),
        }
    }
}

/// Bit-scan a `u32`-domain word array in ascending order. The SAME shape as
/// `effect_store::iter_set_bits`, but over raw `u32` (not `EffectId`) — this
/// module's own small helper (see module doc for why it does not share the
/// `EffectId`-typed primitives, which are specifically the frozen-universe
/// presence-set layout).
fn iter_bits(words: &[u64]) -> impl Iterator<Item = u32> + '_ {
    words.iter().enumerate().flat_map(|(word_idx, &word)| {
        let mut remaining = word;
        std::iter::from_fn(move || {
            if remaining == 0 {
                None
            } else {
                let bit = remaining.trailing_zeros();
                remaining &= remaining - 1;
                Some((word_idx as u32) * 64 + bit)
            }
        })
    })
}

/// OR one `u32` id into a growable word array — the temporary routine bitmap
/// [`ReverseEffectIndex::up_table`]/[`ReverseEffectIndex::up_effect`] build
/// (spec ⟨rev4 P1, DECIDED⟩).
fn set_bit_u32(bits: &mut Vec<u64>, id: u32) {
    let word = (id / 64) as usize;
    if bits.len() <= word {
        bits.resize(word + 1, 0);
    }
    bits[word] |= 1u64 << (id % 64);
}

// ---------------------------------------------------------------------------
// ReverseEffectIndex — the transpose (spec Part A Step 4).
// ---------------------------------------------------------------------------

/// The bidirectional effect/table <-> routine query index. Built ONCE from a
/// finished [`SummaryBundle`] (a read-only transpose pass — never mutates the
/// bundle, never changes db-effect output).
pub struct ReverseEffectIndex {
    /// `class_members_pool[class_members_ranges[c]]` = class `c`'s member
    /// routines, ascending `RoutineIx` (spec Step 4 CSR).
    class_members_pool: Vec<RoutineIx>,
    class_members_ranges: Vec<Range<u32>>,
    /// `EffectId.0` -> the classes whose shared base contains that effect.
    effect_to_sccs: Vec<PostingList>,
    /// `EffectId.0` -> the routines whose OWN delta (not their class's base)
    /// carries that effect.
    effect_to_delta_routines: Vec<PostingList>,
    /// table id -> the classes whose shared base touches that table.
    table_to_sccs: HashMap<String, PostingList>,
    /// table id -> the routines whose delta touches that table AND whose
    /// class's base does NOT (the ⟨rev3⟩ disjoint contract).
    table_to_delta_routines: HashMap<String, PostingList>,
}

impl ReverseEffectIndex {
    /// Build the index: ONE transpose pass over each distinct class's shared
    /// base (not the collapsed-away per-routine memberships), plus one pass
    /// over each routine's own (small) delta.
    pub fn build(bundle: &SummaryBundle) -> Self {
        let store = bundle.effects();
        let universe = store.universe();
        let n_effects = universe.len();
        let n_classes = store.set_count();

        // 1. Group routines-with-rows by class, ascending RoutineIx per class
        //    (deterministic regardless of the bundle's HashMap iteration
        //    order).
        let mut members_by_class: Vec<Vec<RoutineIx>> = vec![Vec::new(); n_classes];
        for r in bundle.routines_with_rows() {
            if let Some(set_id) = bundle.terminal_base(r) {
                members_by_class[set_id.0 as usize].push(r);
            }
        }
        for members in &mut members_by_class {
            members.sort_unstable();
        }

        // 2. CSR-pool the class members.
        let mut class_members_pool: Vec<RoutineIx> = Vec::new();
        let mut class_members_ranges: Vec<Range<u32>> = Vec::with_capacity(n_classes);
        for members in &members_by_class {
            let start = class_members_pool.len() as u32;
            class_members_pool.extend_from_slice(members);
            class_members_ranges.push(start..class_members_pool.len() as u32);
        }

        // 3. ONE transpose pass per distinct class's shared base:
        //    effect_to_sccs + table_to_sccs (+ each class's touched-table set,
        //    which the delta pass below needs for the ⟨rev3⟩ disjoint
        //    contract).
        //
        //    Both table maps are keyed on `&str` BORROWED from `universe`
        //    (which outlives this whole function — it is reached through the
        //    `&SummaryBundle` parameter), so this pass allocates NOTHING per
        //    membership; the only `String`s minted are the `<= n_tables`
        //    surviving map keys, at the finalize below. See the module doc for
        //    the 2·B-allocations cost this replaced.
        let mut effect_to_sccs_ids: Vec<Vec<u32>> = vec![Vec::new(); n_effects];
        let mut table_to_sccs_ids: HashMap<&str, Vec<u32>> = HashMap::new();
        let mut class_tables: Vec<HashSet<&str>> = vec![HashSet::new(); n_classes];

        for (class_ix, range) in class_members_ranges.iter().enumerate() {
            if range.is_empty() {
                continue; // an interned set with no live class member — should
                // not occur (every arena push pairs with >= 1 row), skipped
                // defensively rather than assumed.
            }
            let set = store.set(EffectSetId(class_ix as u32));
            for eid in set.iter() {
                effect_to_sccs_ids[eid.0 as usize].push(class_ix as u32);
                let table: &str = universe.identity(eid).table_id.as_str();
                table_to_sccs_ids
                    .entry(table)
                    .or_default()
                    .push(class_ix as u32);
                class_tables[class_ix].insert(table);
            }
        }

        // 4. Per-routine delta pass: effect_to_delta_routines + the disjoint
        //    table_to_delta_routines contract (⟨rev3⟩: R is posted under T
        //    iff delta(R) touches T AND base(class(R)) does not).
        let mut effect_to_delta_routines_ids: Vec<Vec<u32>> = vec![Vec::new(); n_effects];
        let mut table_to_delta_routines_ids: HashMap<&str, Vec<u32>> = HashMap::new();

        for r in bundle.routines_with_rows() {
            let Some(set_id) = bundle.terminal_base(r) else {
                continue;
            };
            let class_ix = set_id.0 as usize;
            let mut added_tables: HashSet<&str> = HashSet::new();
            for &eid in bundle.pd_delta_ids(r) {
                effect_to_delta_routines_ids[eid.0 as usize].push(r.0);
                let table: &str = universe.identity(eid).table_id.as_str();
                if !class_tables[class_ix].contains(table) && added_tables.insert(table) {
                    table_to_delta_routines_ids
                        .entry(table)
                        .or_default()
                        .push(r.0);
                }
            }
        }

        ReverseEffectIndex {
            class_members_pool,
            class_members_ranges,
            effect_to_sccs: effect_to_sccs_ids
                .into_iter()
                .map(PostingList::from_ids)
                .collect(),
            effect_to_delta_routines: effect_to_delta_routines_ids
                .into_iter()
                .map(PostingList::from_ids)
                .collect(),
            // The ONLY table-id `String` allocations in the whole build: one
            // per surviving map key (<= n_tables), not one per membership.
            table_to_sccs: table_to_sccs_ids
                .into_iter()
                .map(|(t, ids)| (t.to_string(), PostingList::from_ids(ids)))
                .collect(),
            table_to_delta_routines: table_to_delta_routines_ids
                .into_iter()
                .map(|(t, ids)| (t.to_string(), PostingList::from_ids(ids)))
                .collect(),
        }
    }

    /// A class's ascending-`RoutineIx` members (spec Step 4 CSR).
    pub fn class_members(&self, class: EffectClassIx) -> &[RoutineIx] {
        match self.class_members_ranges.get(class.0 as usize) {
            Some(range) => &self.class_members_pool[range.start as usize..range.end as usize],
            None => &[],
        }
    }

    /// The classes whose shared base contains `effect`.
    pub fn effect_classes(&self, effect: EffectId) -> impl Iterator<Item = EffectClassIx> + '_ {
        self.effect_to_sccs
            .get(effect.0 as usize)
            .into_iter()
            .flat_map(|pl| pl.iter())
            .map(EffectClassIx)
    }

    /// The routines whose OWN delta (not their class's base) carries `effect`.
    pub fn effect_delta_routines(&self, effect: EffectId) -> impl Iterator<Item = RoutineIx> + '_ {
        self.effect_to_delta_routines
            .get(effect.0 as usize)
            .into_iter()
            .flat_map(|pl| pl.iter())
            .map(RoutineIx)
    }

    /// True iff `table_id`'s posting includes `class` (its shared base
    /// touches the table).
    pub fn table_touches_via_base(&self, table_id: &str, class: EffectClassIx) -> bool {
        self.table_to_sccs
            .get(table_id)
            .is_some_and(|pl| pl.contains(class.0))
    }

    /// True iff `table_id`'s DELTA posting includes `r` (its own delta
    /// touches the table, disjoint from its class's base — ⟨rev3⟩).
    pub fn table_touches_via_delta_routine(&self, table_id: &str, r: RoutineIx) -> bool {
        self.table_to_delta_routines
            .get(table_id)
            .is_some_and(|pl| pl.contains(r.0))
    }

    /// Down: `r`'s full projected effect set — delegates VERBATIM to
    /// [`SummaryBundle::db_effects`] (the base∪delta ordered merge A3 already
    /// built); this module never re-derives that merge (spec: "down() should
    /// reuse the store's projection/merge, not fork it").
    pub fn down<'a>(
        &self,
        bundle: &'a SummaryBundle,
        r: RoutineIx,
    ) -> impl Iterator<Item = DbEffectRef<'a>> {
        bundle.db_effects(r)
    }

    /// "Does routine `r` touch table `table_id`?" — a pure posting-list
    /// membership check (base-class OR disjoint-delta); NEVER decompresses
    /// `r`'s full effect set (no call to [`Self::down`]/`db_effects` here).
    pub fn touches_table(&self, bundle: &SummaryBundle, r: RoutineIx, table_id: &str) -> bool {
        let Some(class) = class_of(bundle, r) else {
            return false;
        };
        self.table_touches_via_base(table_id, class)
            || self.table_touches_via_delta_routine(table_id, r)
    }

    /// True iff `effect`'s CLASS posting includes `class` — the effect-
    /// granularity counterpart of [`Self::table_touches_via_base`], and the
    /// membership primitive [`Self::touches_effect`] is built on.
    ///
    /// Private on purpose: unlike the table pair, the two effect postings carry
    /// NO disjointness contract between them (pass 4 posts every delta effect
    /// unconditionally, since a routine's own PD delta and its class's terminal
    /// base are disjoint EffectId sets by construction — spec Step 3 — so there
    /// is nothing for a caller to reason about across the two).
    fn effect_touches_via_base(&self, effect: EffectId, class: EffectClassIx) -> bool {
        self.effect_to_sccs
            .get(effect.0 as usize)
            .is_some_and(|pl| pl.contains(class.0))
    }

    /// True iff `effect`'s DELTA posting includes `r`. Private — see
    /// [`Self::effect_touches_via_base`].
    fn effect_touches_via_delta_routine(&self, effect: EffectId, r: RoutineIx) -> bool {
        self.effect_to_delta_routines
            .get(effect.0 as usize)
            .is_some_and(|pl| pl.contains(r.0))
    }

    /// "Does routine `r` touch effect `effect`?" — the effect-granularity
    /// sibling of [`Self::touches_table`], same no-decompression shape.
    ///
    /// Both arms go through [`PostingList::contains`] (binary search on a
    /// sparse posting, one word test on a dense one), NOT a linear `.any()`
    /// scan of the iterator: the pre-Task-6 body read
    /// `self.effect_classes(effect).any(|c| c == class)`, which walked a SORTED
    /// posting element-by-element (a full bit-scan in the dense case) and so
    /// contradicted this doc's own "same no-decompression shape" claim.
    /// Harmless at DO's mean posting of 25, wrong asymptotics as written.
    pub fn touches_effect(&self, bundle: &SummaryBundle, r: RoutineIx, effect: EffectId) -> bool {
        let Some(class) = class_of(bundle, r) else {
            return false;
        };
        self.effect_touches_via_base(effect, class)
            || self.effect_touches_via_delta_routine(effect, r)
    }

    /// Up: every routine that touches `table_id`, as an ASCENDING-`RoutineIx`
    /// result (spec ⟨rev4 P1, DECIDED⟩: expand class postings into a
    /// TEMPORARY routine bitmap — set bits while expanding classes + delta
    /// postings — then bit-scan ascending; expanding sorted class postings
    /// directly does NOT yield a globally-sorted `RoutineIx` sequence, since
    /// class order and routine order are independent).
    pub fn up_table(&self, table_id: &str) -> Vec<RoutineIx> {
        let mut bits: Vec<u64> = Vec::new();
        if let Some(pl) = self.table_to_sccs.get(table_id) {
            for class in pl.iter() {
                for &r in self.class_members(EffectClassIx(class)) {
                    set_bit_u32(&mut bits, r.0);
                }
            }
        }
        if let Some(pl) = self.table_to_delta_routines.get(table_id) {
            for r in pl.iter() {
                set_bit_u32(&mut bits, r);
            }
        }
        iter_bits(&bits).map(RoutineIx).collect()
    }

    /// Up: every routine that touches `effect`, ascending `RoutineIx` — the
    /// effect-granularity sibling of [`Self::up_table`], same temporary-
    /// bitmap construction.
    pub fn up_effect(&self, effect: EffectId) -> Vec<RoutineIx> {
        let mut bits: Vec<u64> = Vec::new();
        for class in self.effect_classes(effect) {
            for &r in self.class_members(class) {
                set_bit_u32(&mut bits, r.0);
            }
        }
        for r in self.effect_delta_routines(effect) {
            set_bit_u32(&mut bits, r.0);
        }
        iter_bits(&bits).map(RoutineIx).collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::l4::combined_graph::{CombinedEdge, CombinedGraph};
    use crate::engine::l4::db_effect_solver::effective_sccs;
    use crate::engine::l4::effect_lattice::TempStateKind;
    use crate::engine::l4::effect_store::{
        DbEffectRef, SummaryBundle, SummaryBundleBuilder, ViaRank, set_bit,
    };
    use crate::engine::l4::effect_universe::{EffectId, EffectIdentity, GrowingEffectUniverse};
    use crate::engine::l4::reverse_index::{
        POSTING_SPARSE_THRESHOLD, PostingList, ReverseEffectIndex, class_of, graph_scc_of,
    };
    use crate::engine::l4::routine_interner::{RoutineInterner, RoutineIx};
    use crate::engine::l4::scc::{SccInputGraph, tarjan_scc};
    use std::collections::HashMap;

    fn ident(op: &str, table: &str, opid: &str, temp: TempStateKind) -> EffectIdentity {
        EffectIdentity {
            op: op.into(),
            table_id: table.into(),
            operation_id: opid.into(),
            temp,
        }
    }

    fn bits_of(ids: &[EffectId]) -> Vec<u64> {
        let mut b = Vec::new();
        for &id in ids {
            set_bit(&mut b, id);
        }
        b
    }

    /// A 2-routine fixture: `r1`'s base touches `T1`; `r1`'s delta touches
    /// BOTH `T1` (already covered by base — the disjoint-contract case) and
    /// `T2` (base does NOT cover it — the genuine delta case). `r2`'s base
    /// touches only `T3`.
    struct Fixture {
        bundle: SummaryBundle,
        r1: RoutineIx,
        r2: RoutineIx,
        base_t1: EffectId,
        pd_t1: EffectId,
        pd_t2: EffectId,
        base_t3: EffectId,
    }

    fn build_fixture() -> Fixture {
        let mut u = GrowingEffectUniverse::new();
        let base_t1 = u.intern(&ident(
            "Insert",
            "T1",
            "op_base",
            TempStateKind::Known(true),
        ));
        let pd_t1 = u.intern(&ident(
            "Modify",
            "T1",
            "op_pd1",
            TempStateKind::ParameterDependent(0),
        ));
        let pd_t2 = u.intern(&ident(
            "Modify",
            "T2",
            "op_pd2",
            TempStateKind::ParameterDependent(0),
        ));
        let base_t3 = u.intern(&ident("Insert", "T3", "op_r2", TempStateKind::Known(true)));

        let mut interner = RoutineInterner::new();
        let r1 = interner.intern("r1");
        let r2 = interner.intern("r2");

        let mut b = SummaryBundleBuilder::new();
        let set1 = b.push_terminal_set(bits_of(&[base_t1]));
        b.push_row(
            r1,
            set1,
            vec![ViaRank::Direct],
            vec![pd_t1, pd_t2],
            vec![ViaRank::Dynamic, ViaRank::Dynamic],
        );
        let set2 = b.push_terminal_set(bits_of(&[base_t3]));
        b.push_row(r2, set2, vec![ViaRank::Direct], vec![], vec![]);

        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);
        Fixture {
            bundle,
            r1,
            r2,
            base_t1,
            pd_t1,
            pd_t2,
            base_t3,
        }
    }

    // ---- down(r) == the routine's computed set ---------------------------

    #[test]
    fn down_matches_db_effects_exactly() {
        let fx = build_fixture();
        let index = ReverseEffectIndex::build(&fx.bundle);
        let via_down: Vec<_> = index
            .down(&fx.bundle, fx.r1)
            .map(|e: DbEffectRef| e.to_owned())
            .collect();
        let via_bundle: Vec<_> = fx.bundle.db_effects(fx.r1).map(|e| e.to_owned()).collect();
        assert_eq!(via_down, via_bundle);
        assert_eq!(via_down.len(), 3, "base(T1) + pd(T1) + pd(T2)");
    }

    // ---- touches_table: true/false without decompression -----------------

    #[test]
    fn touches_table_true_and_false_cases() {
        let fx = build_fixture();
        let index = ReverseEffectIndex::build(&fx.bundle);

        assert!(
            index.touches_table(&fx.bundle, fx.r1, "T1"),
            "r1's base touches T1"
        );
        assert!(
            index.touches_table(&fx.bundle, fx.r1, "T2"),
            "r1's delta touches T2"
        );
        assert!(
            !index.touches_table(&fx.bundle, fx.r1, "T3"),
            "r1 never touches T3"
        );

        assert!(
            index.touches_table(&fx.bundle, fx.r2, "T3"),
            "r2's base touches T3"
        );
        assert!(
            !index.touches_table(&fx.bundle, fx.r2, "T1"),
            "r2 never touches T1"
        );
    }

    // ---- table-posting disjoint invariant (⟨rev3⟩) ------------------------

    #[test]
    fn table_posting_disjoint_invariant() {
        let fx = build_fixture();
        let index = ReverseEffectIndex::build(&fx.bundle);
        let r1_class = class_of(&fx.bundle, fx.r1).unwrap();

        // T1: base already covers it -> r1 must NOT be posted under the
        // delta table posting, even though its delta ALSO carries a T1 fact.
        assert!(index.table_touches_via_base("T1", r1_class));
        assert!(!index.table_touches_via_delta_routine("T1", fx.r1));

        // T2: base does NOT cover it -> the delta posting DOES carry r1.
        assert!(!index.table_touches_via_base("T2", r1_class));
        assert!(index.table_touches_via_delta_routine("T2", fx.r1));

        // General invariant, scanned over the built index: whenever a
        // routine is posted under a table's delta list, its class's base
        // must NOT also touch that table.
        for table in ["T1", "T2", "T3", "T4"] {
            for r in [fx.r1, fx.r2] {
                if index.table_touches_via_delta_routine(table, r) {
                    let c = class_of(&fx.bundle, r).unwrap();
                    assert!(
                        !index.table_touches_via_base(table, c),
                        "table {table}: r is in the delta posting AND the base posting — not disjoint"
                    );
                }
            }
        }
    }

    // ---- up_table: exact routine set, ascending RoutineIx -----------------

    #[test]
    fn up_table_returns_exactly_expected_routines_ascending() {
        let fx = build_fixture();
        let index = ReverseEffectIndex::build(&fx.bundle);

        assert_eq!(index.up_table("T1"), vec![fx.r1]);
        assert_eq!(index.up_table("T2"), vec![fx.r1]);
        assert_eq!(index.up_table("T3"), vec![fx.r2]);
        assert_eq!(index.up_table("T4"), Vec::<RoutineIx>::new());
    }

    /// `up_table` must yield ascending `RoutineIx` even when class-posting
    /// expansion order disagrees with routine order (⟨rev4 P1, DECIDED⟩: the
    /// temporary-bitmap normalization is load-bearing, not cosmetic).
    #[test]
    fn up_table_normalizes_to_ascending_routine_ix_even_when_class_order_disagrees() {
        let mut u = GrowingEffectUniverse::new();
        let e1 = u.intern(&ident("Insert", "TX", "op1", TempStateKind::Known(true)));
        let e2 = u.intern(&ident("Modify", "TX", "op2", TempStateKind::Known(true)));

        let mut interner = RoutineInterner::new();
        let a = interner.intern("a"); // RoutineIx(0)
        let b_ix = interner.intern("b"); // RoutineIx(1)

        let mut builder = SummaryBundleBuilder::new();
        // Push b's terminal set FIRST (lower EffectSetId / class) even though
        // b's RoutineIx is HIGHER than a's.
        let set_b = builder.push_terminal_set(bits_of(&[e2]));
        builder.push_row(b_ix, set_b, vec![ViaRank::Direct], vec![], vec![]);
        let set_a = builder.push_terminal_set(bits_of(&[e1]));
        builder.push_row(a, set_a, vec![ViaRank::Direct], vec![], vec![]);

        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = builder.finish(u.freeze(), interner, rvid);
        let index = ReverseEffectIndex::build(&bundle);

        assert!(
            class_of(&bundle, b_ix).unwrap().0 < class_of(&bundle, a).unwrap().0,
            "b's class was interned before a's"
        );
        assert_eq!(index.up_table("TX"), vec![a, b_ix]);
    }

    // ---- effect-granularity siblings ---------------------------------------

    #[test]
    fn up_effect_and_touches_effect_mirror_the_table_queries() {
        let fx = build_fixture();
        let index = ReverseEffectIndex::build(&fx.bundle);

        assert!(index.touches_effect(&fx.bundle, fx.r1, fx.base_t1));
        // `pd_t1` is r1's OWN delta fact, but it is a DIFFERENT EffectId from
        // `base_t1` even though both touch table T1 (distinct op/opid) — so
        // it must ALSO be independently answerable at effect granularity.
        assert!(index.touches_effect(&fx.bundle, fx.r1, fx.pd_t1));
        assert!(index.touches_effect(&fx.bundle, fx.r1, fx.pd_t2));
        assert!(!index.touches_effect(&fx.bundle, fx.r2, fx.base_t1));
        assert!(!index.touches_effect(&fx.bundle, fx.r2, fx.pd_t1));

        assert_eq!(index.up_effect(fx.base_t1), vec![fx.r1]);
        assert_eq!(index.up_effect(fx.pd_t1), vec![fx.r1]);
        assert_eq!(index.up_effect(fx.pd_t2), vec![fx.r1]);
        assert_eq!(index.up_effect(fx.base_t3), vec![fx.r2]);
    }

    // ---- PostingList: the DENSE branch -------------------------------------
    //
    // ⟨Task 6, scope §1.5⟩ Before this, all 7 tests used postings of 1-2
    // elements, so `Dense` construction, `Dense::contains`' word-bounds check
    // and `iter_bits` were exercised by production data and by NOTHING else —
    // on DO, 4 of 61 table postings cross `POSTING_SPARSE_THRESHOLD` (largest
    // 650). The first two tests below hand-state the precondition (a literal
    // >= 256-element id list) rather than depending on a bundle happening to
    // produce one; the third then proves the production build path REACHES the
    // dense branch, asserting that rather than assuming it.

    /// The dense branch, with the input stated literally: 300 ids in, sorted
    /// ascending out, membership exact in both directions, and the
    /// `w < words.len()` bounds guard exercised by an id past the last word.
    #[test]
    fn posting_list_dense_branch_contains_and_iterates_exactly() {
        // Deliberately UNSORTED and containing a duplicate, so `from_ids`'
        // sort+dedup is exercised on the dense path too.
        let mut ids: Vec<u32> = (0..300u32).map(|n| n * 2).collect();
        ids.reverse();
        ids.push(0);
        let pl = PostingList::from_ids(ids);

        assert!(
            matches!(pl, PostingList::Dense(_)),
            "300 unique ids >= POSTING_SPARSE_THRESHOLD ({POSTING_SPARSE_THRESHOLD}) must \
             pick the Dense repr"
        );
        let expected: Vec<u32> = (0..300u32).map(|n| n * 2).collect();
        assert_eq!(
            pl.iter().collect::<Vec<u32>>(),
            expected,
            "ascending + deduped"
        );
        for &id in &expected {
            assert!(pl.contains(id), "dense contains({id}) must be true");
            assert!(
                !pl.contains(id + 1),
                "dense contains({}) must be false (odd ids were never inserted)",
                id + 1
            );
        }
        // The word-bounds guard: max id is 598 -> 10 words (bits 0..639), so an
        // id in word 100 is past the end and must answer false, not panic.
        assert!(
            !pl.contains(6400),
            "an id past the last word is absent, not a panic"
        );
    }

    /// The sparse/dense crossover: 255 ids stay Sparse, 256 flip to Dense, and
    /// both reprs answer `contains`/`iter` identically over the SAME id set.
    /// Both cardinalities are stated literally.
    #[test]
    fn posting_list_reprs_agree_across_the_sparse_dense_threshold() {
        let below: Vec<u32> = (0..(POSTING_SPARSE_THRESHOLD as u32 - 1)).collect();
        let at: Vec<u32> = (0..(POSTING_SPARSE_THRESHOLD as u32)).collect();

        let sparse = PostingList::from_ids(below.clone());
        let dense = PostingList::from_ids(at.clone());
        assert!(
            matches!(sparse, PostingList::Sparse(_)),
            "255 ids stay sparse"
        );
        assert!(
            matches!(dense, PostingList::Dense(_)),
            "256 ids flip to dense"
        );

        assert_eq!(sparse.iter().collect::<Vec<u32>>(), below);
        assert_eq!(dense.iter().collect::<Vec<u32>>(), at);
        // The one id that differs between the two sets is the discriminator.
        let boundary = POSTING_SPARSE_THRESHOLD as u32 - 1;
        assert!(!sparse.contains(boundary));
        assert!(dense.contains(boundary));
    }

    /// The production BUILD path reaches the dense branch: 300 routines, each
    /// its own distinct effect class, all touching one table `TBIG` — so
    /// `table_to_sccs["TBIG"]` holds 300 class ids. The dense-ness is
    /// ASSERTED (reaching into the private field from this child module), not
    /// assumed, and `up_table` must still return all 300 routines ascending
    /// (which routes the answer through `Dense::iter`/`iter_bits`).
    #[test]
    fn build_produces_a_dense_table_posting_and_up_table_still_answers_exactly() {
        const N: u32 = 300;
        assert!(
            N as usize >= POSTING_SPARSE_THRESHOLD,
            "fixture must exceed the threshold for this test to mean anything"
        );

        let mut u = GrowingEffectUniverse::new();
        let eids: Vec<EffectId> = (0..N)
            .map(|n| {
                u.intern(&ident(
                    "Insert",
                    "TBIG",
                    &format!("op{n:04}"),
                    TempStateKind::Known(true),
                ))
            })
            .collect();

        let mut interner = RoutineInterner::new();
        let routines: Vec<RoutineIx> = (0..N)
            .map(|n| interner.intern(&format!("r{n:04}")))
            .collect();

        let mut b = SummaryBundleBuilder::new();
        for (i, &r) in routines.iter().enumerate() {
            // A DISTINCT singleton terminal set per routine => a distinct
            // hash-consed class per routine => 300 class ids in TBIG's posting.
            let set = b.push_terminal_set(bits_of(&[eids[i]]));
            b.push_row(r, set, vec![ViaRank::Direct], vec![], vec![]);
        }
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);
        assert_eq!(
            bundle.effects().set_count(),
            N as usize,
            "each routine's singleton set is distinct — 300 classes, not one shared class"
        );

        let index = ReverseEffectIndex::build(&bundle);
        assert!(
            matches!(index.table_to_sccs.get("TBIG"), Some(PostingList::Dense(_))),
            "the build path must have produced a DENSE table posting here — if this ever \
             flips to Sparse the fixture stopped covering the dense branch and the rest of \
             this test proves nothing"
        );

        assert_eq!(index.up_table("TBIG"), routines, "all 300, ascending");
        for (i, &r) in routines.iter().enumerate() {
            assert!(index.touches_table(&bundle, r, "TBIG"));
            assert!(index.touches_effect(&bundle, r, eids[i]));
            // Its NEIGHBOUR's effect is a different EffectId on the same table:
            // table granularity says yes, effect granularity says no.
            let other = eids[(i + 1) % N as usize];
            assert!(!index.touches_effect(&bundle, r, other));
        }
        assert!(index.up_table("TNOPE").is_empty());
    }

    // ---- the two SCC notions are genuinely distinct ------------------------

    /// A fixed leaf (`b`) excluded from a 3-node Tarjan cycle `a -> b -> c ->
    /// a` splits the induced subgraph into 2 singleton effective SCCs
    /// (`effective_sccs`, the REAL production function) — so `a` and `c`
    /// land in DIFFERENT `EffectClassIx`es even though Tarjan itself grouped
    /// all three into ONE `GraphSccIx`. This is the spec's binding
    /// distinction (⟨rev⟩:194-198): the effect-sharing DAG and the original
    /// call-graph condensation are NOT the same relation.
    #[test]
    fn effect_class_and_graph_scc_are_genuinely_distinct_when_a_leaf_splits_a_cycle() {
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        adjacency.insert("a".into(), vec!["b".into()]);
        adjacency.insert("b".into(), vec!["c".into()]);
        adjacency.insert("c".into(), vec!["a".into()]);
        let scc_result = tarjan_scc(&SccInputGraph {
            nodes: &nodes,
            edges_by_from: &adjacency,
        });
        assert_eq!(scc_result.sccs.len(), 1, "Tarjan sees ONE 3-member cycle");
        assert!(scc_result.sccs[0].recursive);

        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        for (from, to) in [("a", "b"), ("b", "c"), ("c", "a")] {
            edges_by_from
                .entry(from.to_string())
                .or_default()
                .push(CombinedEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                    kind: "direct".to_string(),
                    callsite_id: None,
                    operation_id: None,
                    event_id: None,
                    subscriber_app_id: None,
                    resolution: "resolved".to_string(),
                });
        }
        let graph = CombinedGraph {
            nodes: nodes.clone(),
            edges_by_from,
            edges_from_order: Vec::new(),
            uncertainty_edges: Vec::new(),
            typed_edges: Vec::new(),
        };
        // `b` is a fixed leaf — never recomputed.
        let eff = effective_sccs(&scc_result.sccs[0], &graph, &|id: &str| id != "b");
        assert_eq!(
            eff.len(),
            2,
            "removing the fixed leaf splits the cycle into 2 effective SCCs"
        );
        for e in &eff {
            assert_eq!(e.members.len(), 1);
            assert!(!e.recursive);
        }

        // Build a bundle mirroring what solve_scc_db_effects would produce:
        // ONE shared terminal set per effective SCC, so a and c land in
        // DIFFERENT classes. b has no row (excluded, no retained summary).
        let mut u = GrowingEffectUniverse::new();
        let e_a = u.intern(&ident("Insert", "TA", "op_a", TempStateKind::Known(true)));
        let e_c = u.intern(&ident("Insert", "TC", "op_c", TempStateKind::Known(true)));
        let mut interner = RoutineInterner::new();
        let ia = interner.intern("a");
        let _ib = interner.intern("b");
        let ic = interner.intern("c");
        let mut b = SummaryBundleBuilder::new();
        let set_a = b.push_terminal_set(bits_of(&[e_a]));
        b.push_row(ia, set_a, vec![ViaRank::Direct], vec![], vec![]);
        let set_c = b.push_terminal_set(bits_of(&[e_c]));
        b.push_row(ic, set_c, vec![ViaRank::Direct], vec![], vec![]);
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);

        assert_eq!(
            graph_scc_of(&scc_result, &bundle, ia),
            graph_scc_of(&scc_result, &bundle, ic),
            "a and c were in the SAME original Tarjan SCC"
        );
        assert_ne!(
            class_of(&bundle, ia),
            class_of(&bundle, ic),
            "a and c landed in DIFFERENT effect classes once the fixed leaf split the cycle"
        );
    }
}
