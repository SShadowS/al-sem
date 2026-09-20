//! C1 — the compact **derived** capability-cone substrate.
//!
//! `capability_cone`'s per-routine `capability_facts_inherited: Vec<CapabilityFact>`
//! is produced by `retag`-cloning one full `CapabilityFact` struct per (routine ×
//! reachable representative). On a 100,941-routine corpus that is ~27M struct
//! clones held live for the whole detector run (~10.9 GB). Every analyze-path
//! consumer of that Vec reads only a derived predicate off it — a presence
//! boolean, a table/event id-set, or (d44) an id-set tagged with which write ops
//! occurred. This module is that derived view, computed by folding the SAME
//! reachable representatives the `retag` sites already visit, with zero clones.
//!
//! ⟨Task 3⟩ It is no longer a parallel view: the analyze path composes under
//! [`ConeOutput::DerivedOnly`] and the raw Vec is not built at all. The only
//! survivors of the raw path are the projection/`prove`/`digest`/`policy`
//! surfaces, which ask for it explicitly.
//!
//! ## What one row summarizes
//!
//! A row is a fold over the routine's REACHABLE cone — `own direct ∪ inherited
//! representatives`, i.e. the sequence the retired `FullRoutineSummary::
//! reachable_iter` used to yield:
//!   - `flags` — presence bits OR-merged over the cone (table / commit / http / file).
//!   - `table_writes_all` — insert|modify|delete on `resource_kind == "table"`
//!     with a `resource_id`, INCLUDING known-temp. Backs `writes_tables_of`.
//!   - `physical_table_writes` — the same, EXCLUDING [`fact_is_known_temp`], each
//!     id carrying a `u8` op-mask. Backs `writes_physical_tables_of` and d44's
//!     per-table op-union.
//!   - `physical_table_reads` — `op == "read"` on a non-known-temp table with a
//!     `resource_id`. Backs d44's read set.
//!   - `event_publishes` — `op == "publish"` on `resource_kind == "event"` with a
//!     `resource_id`. Backs `publishes_events_of`.
//!
//! ## Why the fold is byte-identical to the raw path
//!
//! `retag` (`capability_cone.rs`) rewrites ONLY `subject` / `provenance` / `via` /
//! `witness_callsite_id`. No derived predicate above reads any of those four, so
//! folding the PRE-retag representative is equivalent by construction. The fold
//! therefore runs at the exact `retag` call sites, over the exact same
//! representatives (the singleton path's key-winner `best.values()`, the BFS
//! path's `seen`-deduped reps), plus the routine's own RAW direct facts.
//!
//! **The dedup asymmetry.** The cone dedups inherited facts by
//! `inherited_fact_key = op|resource_kind|resource_id|confidence|tempClass`, so
//! the fold runs over ONE representative per key, not over every reachable
//! fact. The self half is the mirror image: `capability_facts_direct` is stored
//! RAW (un-deduped) and the reachable sequence scans every one of them, so the
//! self half folds the raw Vec, not the key-deduped `direct` map.
//!
//! ⟨issue 33⟩ **That asymmetry is NO LONGER load-bearing, and this doc used to
//! claim it was.** It said so for a real reason: before the temp class went into
//! the key, two facts writing table T with identical `(op, kind, rid,
//! confidence)` but different `temp_state` became ONE representative; whichever
//! won decided whether the ancestor "writes a physical table"; so a nearer — or
//! lexicographically smaller — temp fact could hide a physical one outright, and
//! folding raw facts instead of key-winners would have given a DIFFERENT answer
//! from the cone's own.
//!
//! With the class in the key that is over. [`ConeDerivedBuilder::fold_fact`]
//! discriminates only on `(resource_kind, resource_id, op,
//! fact_is_known_temp)`, and all four are now IN the key — so two facts sharing
//! a key contribute identically, the fold is dedup-INVARIANT on both halves, and
//! folding the raw Vec or the deduped map gives the same derived row either way.
//! The asymmetry is now purely a COST choice (one representative is cheaper than
//! every reachable fact), not a correctness one, and nothing guards it because
//! nothing can go wrong there any more. `writes_physical_tables_of` is exactly
//! the union over the reachable non-known-temp writes, pinned by the
//! `assert_physical_writes_equal_naive_union` oracle in this module's tests.
//!
//! Downstream, the two classes survive as two DISTINCT obligations and are
//! graded separately — the outcome the r4 fixture asserts. That is NOT quite
//! "the conservative merges finally see both facts": `digest::merge_temp_state`
//! fires only for facts sharing a `dedupe_key`, and `dedupe_key` is anchored on
//! the witness TERMINAL's operation id, so with issue 32's paired terminal guard
//! the two classes terminate at DIFFERENT operations and never meet there;
//! `ordering_engine`'s `known_temp_only` likewise keys on `occurrence_id` and
//! grades them apart. Those merges are the FALLBACK for the case where the two
//! DO share an anchor (the synthetic-anchor path, when no terminal was found),
//! where they conservatively degrade to non-temp — graded physical, which is the
//! right answer when a physical fact is present.
//!
//! ## Storage (pooled, not per-routine trees)
//!
//! Four `BTreeSet`/`BTreeMap` per routine would be ~404k tree allocations. Instead
//! the fold runs into reusable scratch `Vec`s, and each routine is frozen into
//! sorted slices appended to four shared pools; the per-routine [`ConeDerivedRow`]
//! holds four `Range<u32>` + `flags` (~40 B). Resource ids are interned once
//! workspace-wide ([`ResInterner`]) — the same CSR playbook `l4::effect_store`
//! uses for db-effects. Interning ORDER never reaches output: every query resolves
//! ids back to `String` and sorts by the resolved string, reproducing the
//! `BTreeSet<String>` ordering the raw helpers produce.

use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;
use std::ops::Range;

use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact};

// ---------------------------------------------------------------------------
// Vocabularies — the closed sets the fold discriminates on.
// ---------------------------------------------------------------------------

/// Any reachable fact with `resource_kind == "table"` (regardless of op) — backs
/// `touches_db_of`.
pub const TOUCHES_TABLE: u8 = 1 << 0;
/// `op == "commit" && resource_kind == "transaction"` — backs `may_commit`.
pub const MAY_COMMIT: u8 = 1 << 1;
/// `resource_kind == "http"` — half of d48's `routine_touches_external_io`.
pub const TOUCHES_HTTP: u8 = 1 << 2;
/// `resource_kind == "file"` — the other half. `{http, file}` is the COMPLETE IO
/// vocabulary — see [`io_kind_bit`], the ONE definition; `d48::is_io_resource_kind`
/// routes through it too.
pub const TOUCHES_FILE: u8 = 1 << 3;

/// `op == "insert"`.
pub const OP_INSERT: u8 = 1 << 0;
/// `op == "modify"`.
pub const OP_MODIFY: u8 = 1 << 1;
/// `op == "delete"`.
pub const OP_DELETE: u8 = 1 << 2;

/// The table-write op bit for `op`, or `None` when `op` is not a write
/// (al-sem `TABLE_WRITE_OPS = {insert, modify, delete}`; identical to d44's
/// `is_write_op`). The ONE definition of "is this a table write" — `capability_query`
/// and the fold both route through it.
pub fn write_op_bit(op: &str) -> Option<u8> {
    match op {
        "insert" => Some(OP_INSERT),
        "modify" => Some(OP_MODIFY),
        "delete" => Some(OP_DELETE),
        _ => None,
    }
}

/// The IO-presence flag bit for `resource_kind`, or `None` when `kind` is not
/// an IO resource kind. `{http, file}` is the COMPLETE IO vocabulary — the ONE
/// definition of "is this IO". ⟨C1 Task 2 fix I1⟩ Before this fix, the fold's
/// `"http"`/`"file"` arms and `d48::is_io_resource_kind` each hardcoded the same
/// two-kind set independently; a fourth IO kind added to only one of them would
/// silently desync the pruning gate from the terminal producer. Both now route
/// through this function.
pub fn io_kind_bit(kind: &str) -> Option<u8> {
    match kind {
        "http" => Some(TOUCHES_HTTP),
        "file" => Some(TOUCHES_FILE),
        _ => None,
    }
}

/// Decode an op-mask into op literals in **LEXICAL** order — `delete, insert,
/// modify`. d44 unions its ops into a `BTreeSet<&str>` and renders
/// `op_union.join(", ")`, so the decoder must emit the `BTreeSet` iteration
/// order, NOT the mask's bit order.
pub fn decode_op_mask(mask: u8) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if mask & OP_DELETE != 0 {
        out.push("delete");
    }
    if mask & OP_INSERT != 0 {
        out.push("insert");
    }
    if mask & OP_MODIFY != 0 {
        out.push("modify");
    }
    out
}

/// True when a capability fact is a write/read on a PROVABLY temporary record
/// (`extra == Table { temp_state: known/true }`). Such ops are in-memory — they
/// never touch the physical database, so they cannot create cross-routine /
/// cross-extension table conflicts or exposure. Suppression-direction safe: only
/// the exact `known/true` signal qualifies; `Unknown` / parameter-dependent /
/// absent temp_state keep counting.
///
/// This is the ONE implementation **over an L4 [`CapabilityFact`]** —
/// `l5::capability_query::fact_is_known_temp` re-exports it, and
/// `capability_cone::inherited_fact_key` calls it, so the key, the fold and the
/// raw helpers cannot drift apart.
///
/// ⟨issue 33⟩ It is NOT the only implementation of the `known/true` predicate
/// in the repo, and the old "this is the ONE implementation" claim is why that
/// was easy to miss. Do not read the list below as exhaustive either — there are
/// **at least a dozen** copies, over four different carrier types. The L5
/// snapshot layer has `digest::is_known_temp_state` over
/// `Option<&SnapTempState>` and `digest::is_known_temp_snap` over a snapshot
/// `Fact`, plus `ordering_engine`'s own `matches!` over an effect's temp state;
/// the L5 detectors have `detectors::is_known_temp` /
/// `detectors::is_known_temp_rv` over an OPERATION SITE's `PTempState` and
/// inline re-spellings of the same test in d10 / d18 / d33 / d36 / d37 / d39 /
/// d40; and `l3_workspace` tests `kind == "known"` in two more places. If this
/// rule ever changes, find them with `grep -rn 'kind == "known"' src/` plus
/// `grep -rn 'Known { value: true }' src/` — do not trust this paragraph to
/// still be complete.
pub fn fact_is_known_temp(f: &CapabilityFact) -> bool {
    matches!(
        &f.extra,
        Some(CapabilityExtra::Table { temp_state: Some(ts), .. })
            if ts.kind == "known" && ts.value == Some(true)
    )
}

// ---------------------------------------------------------------------------
// Output mode — the cone seam's gate.
// ---------------------------------------------------------------------------

/// What a cone composition should PRODUCE. The raw inherited `Vec<CapabilityFact>`
/// is allocated *inside* the cone walk, so a post-hoc check could only discard it;
/// the gate has to be threaded into the walk itself.
///
/// [`ConeOutput::DerivedOnly`] is a real skip, not build-then-drop: neither the
/// per-representative `retag` clone nor the `sort_inherited` allocation happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConeOutput {
    /// Only the [`ConeDerivedStore`]. Every routine's raw inherited Vec is empty.
    DerivedOnly,
    /// Only the raw inherited Vecs (today's behaviour). The store stays empty.
    RawOnly,
    /// Both — the dual-run mode the C1 parity oracle asserts over.
    Both,
}

impl ConeOutput {
    /// True when the raw inherited `Vec<CapabilityFact>` must be materialized.
    pub fn wants_raw(self) -> bool {
        matches!(self, ConeOutput::RawOnly | ConeOutput::Both)
    }

    /// True when the derived substrate must be folded.
    pub fn wants_derived(self) -> bool {
        matches!(self, ConeOutput::DerivedOnly | ConeOutput::Both)
    }
}

// ---------------------------------------------------------------------------
// The resource-id interner.
// ---------------------------------------------------------------------------

/// An interned resource-id handle (TableId / EventId). Lossless — `resolve`
/// returns the exact original string.
pub type ResId = u32;

/// A dense set of [`ResId`]s over one store's resource-id universe.
///
/// Exists so a caller unioning many routines' id windows pays one word-OR per id
/// and no allocation per element. The alternative — accumulating the windows into
/// a `Vec<ResId>` and deduping at the end — would hold 261,772,789 `u32`s (1.05 GB)
/// for a single BC Base App run of `transaction_spans`, trading CPU for RSS on a
/// span that retains 73 MB today. This type is the reason that trade is not made.
///
/// [`Self::iter_ids`] yields ascending ID order, which is INTERN order, not
/// lexicographic. A caller reproducing a `BTreeSet<String>`'s output resolves and
/// then sorts by string.
#[derive(Debug, Default)]
pub struct ResBitset {
    words: Vec<u64>,
}

impl ResBitset {
    /// A bitset wide enough for `universe_len` ids (see
    /// [`ConeDerivedStore::res_universe_len`]). `insert_all` grows it anyway, so
    /// the width is an optimization, not a bound.
    pub fn new(universe_len: usize) -> Self {
        ResBitset {
            words: vec![0u64; universe_len.div_ceil(64)],
        }
    }

    pub fn insert_all(&mut self, ids: &[ResId]) {
        for &id in ids {
            let w = (id as usize) / 64;
            if w >= self.words.len() {
                self.words.resize(w + 1, 0);
            }
            self.words[w] |= 1u64 << ((id as usize) % 64);
        }
    }

    /// Empty the set, KEEPING the allocation — the caller reuses one bitset across
    /// every span template in a run.
    pub fn clear(&mut self) {
        self.words.iter_mut().for_each(|w| *w = 0);
    }

    pub fn iter_ids(&self) -> impl Iterator<Item = ResId> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            let mut bits = w;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let b = bits.trailing_zeros();
                bits &= bits - 1;
                Some((wi as u32) * 64 + b)
            })
        })
    }
}

/// Workspace-global, lossless `String → u32` interner for cone resource ids.
/// Serial (the cone walk is serial). Interning ORDER is irrelevant to any output:
/// every query resolves to `String` and sorts by the resolved string.
#[derive(Debug, Default)]
pub struct ResInterner {
    by_str: HashMap<String, ResId>,
    strings: Vec<String>,
}

impl ResInterner {
    /// Intern `s`, returning its stable handle. Allocates only on first sight.
    pub fn intern(&mut self, s: &str) -> ResId {
        if let Some(id) = self.by_str.get(s) {
            return *id;
        }
        let id = self.strings.len();
        debug_assert!(id < u32::MAX as usize, "ResInterner exhausted");
        let id = id as ResId;
        self.strings.push(s.to_string());
        self.by_str.insert(s.to_string(), id);
        id
    }

    /// The exact original string for `id`.
    ///
    /// # Panics
    /// Panics when `id` was not produced by this interner.
    pub fn resolve(&self, id: ResId) -> &str {
        &self.strings[id as usize]
    }

    /// Number of distinct interned ids.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// True when nothing has been interned.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// ⟨C1 census⟩ Total HEAP bytes owned by the interner's string storage —
    /// BOTH copies: the canonical `strings` Vec (index → string) AND the
    /// `by_str` reverse-lookup `HashMap`'s own `String` keys. Each interned
    /// resource id's text is therefore stored TWICE; this reports the true
    /// total, not an idealized single-copy count. `C1_CONE_CENSUS`-only —
    /// not read by any production code path.
    pub fn census_heap_bytes(&self) -> u64 {
        let strings_bytes: u64 = self.strings.iter().map(|s| s.len() as u64).sum();
        let by_str_key_bytes: u64 = self.by_str.keys().map(|s| s.len() as u64).sum();
        strings_bytes + by_str_key_bytes
    }
}

// ---------------------------------------------------------------------------
// The per-routine row + the pooled store.
// ---------------------------------------------------------------------------

/// One routine's derived cone: presence flags + four `Range<u32>` windows into
/// the owning [`ConeDerivedStore`]'s pools. Meaningless without its store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConeDerivedRow {
    /// OR-merged presence bits ([`TOUCHES_TABLE`] / [`MAY_COMMIT`] /
    /// [`TOUCHES_HTTP`] / [`TOUCHES_FILE`]).
    pub flags: u8,
    /// Window into `writes_all_pool` — sorted, deduped [`ResId`]s.
    pub table_writes_all: Range<u32>,
    /// Window into `phys_writes_pool` — sorted, deduped `(ResId, op-mask)`.
    pub physical_table_writes: Range<u32>,
    /// Window into `phys_reads_pool` — sorted, deduped [`ResId`]s.
    pub physical_table_reads: Range<u32>,
    /// Window into `events_pool` — sorted, deduped [`ResId`]s.
    pub event_publishes: Range<u32>,
}

/// The row every routine WITHOUT a folded row reads as: no flags, no ids. A
/// routine absent from the store is indistinguishable from one whose cone is
/// empty, which is exactly the raw path's behaviour (no facts ⇒ every presence
/// predicate falls through to the coverage arm).
static EMPTY_ROW: ConeDerivedRow = ConeDerivedRow {
    flags: 0,
    table_writes_all: 0..0,
    physical_table_writes: 0..0,
    physical_table_reads: 0..0,
    event_publishes: 0..0,
};

/// The workspace-wide derived cone substrate: one row per routine plus the shared
/// id pools and the interner. Parked on `DetectorContext` next to
/// `db_effect_bundle` — never referenced from inside a row.
#[derive(Debug, Default)]
pub struct ConeDerivedStore {
    interner: ResInterner,
    writes_all_pool: Vec<ResId>,
    phys_writes_pool: Vec<(ResId, u8)>,
    phys_reads_pool: Vec<ResId>,
    events_pool: Vec<ResId>,
    rows: HashMap<String, ConeDerivedRow>,
}

fn window<'p, T>(pool: &'p [T], r: &Range<u32>) -> &'p [T] {
    &pool[r.start as usize..r.end as usize]
}

impl ConeDerivedStore {
    /// This routine's row, or the empty row when it has none.
    pub fn row(&self, routine_id: &str) -> &ConeDerivedRow {
        self.rows.get(routine_id).unwrap_or(&EMPTY_ROW)
    }

    /// True when no row was folded (e.g. a `RawOnly` composition).
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    // ⟨T1⟩ `forget(routine_id)` — DELETED, not merely unused.
    //
    // It existed for exactly one caller and one reason: `build_detector_context`
    // drained its cone maps with `remove()`, so a LATER occurrence of a colliding
    // internal routine id (two same-name triggers in one object — gap G-18)
    // overwrote the real summary with a degenerate one, and the derived row then
    // had to be dropped to match. T1 made that builder skip the later occurrence
    // instead, so the real summary survives and the row it must match is the FULL
    // fold — the row `forget` used to delete.
    //
    // No path can reach it again as written. A store is produced in exactly three
    // places — `ConeDerivedBuilder::finish` (i.e. `compose_cone_over_graph`),
    // `Default`, and `l5::test_support::cone_store_of` (which folds the store FROM
    // the summaries, so it is consistent by construction) — and after T1 nothing
    // mutates a store after it is built. `build_detector_context_cross_app` reads
    // its cone with `get()` and never had the degeneracy at all.
    //
    // Its doc also carried the claim that the collapse hit "a handful of routines
    // per workspace". That was measured and is wrong by three orders of magnitude:
    // 1 157 of 4 842 DO routines (23.9 %) and 16 906 of 100 941 8020 routines
    // (16.7 %). See `.superpowers/sdd/scope-routine-id-collision.md`.

    /// This routine's presence flags.
    pub fn flags_of(&self, routine_id: &str) -> u8 {
        self.row(routine_id).flags
    }

    /// Any reachable `resource_kind == "table"` fact (the Yes arm of
    /// `touches_db_of`).
    pub fn touches_table(&self, routine_id: &str) -> bool {
        self.flags_of(routine_id) & TOUCHES_TABLE != 0
    }

    /// Any reachable `commit` on `transaction` (the Yes arm of `may_commit`).
    pub fn may_commit_flag(&self, routine_id: &str) -> bool {
        self.flags_of(routine_id) & MAY_COMMIT != 0
    }

    /// Any reachable http/file fact — d48's `routine_touches_external_io`.
    pub fn touches_io(&self, routine_id: &str) -> bool {
        self.flags_of(routine_id) & (TOUCHES_HTTP | TOUCHES_FILE) != 0
    }

    /// Sorted + deduped written TableIds, temp-INCLUSIVE (`writes_tables_of`).
    pub fn writes_tables_of(&self, routine_id: &str) -> Vec<String> {
        self.resolve_sorted(window(
            &self.writes_all_pool,
            &self.row(routine_id).table_writes_all,
        ))
    }

    /// `writes_tables_of(routine_id).len()` without resolving or allocating a
    /// single `String` — the window is already sorted-and-deduped by
    /// `freeze_ids`, so its length IS the distinct-table count.
    /// ⟨C1 Task 2 fix M2⟩ For callers that only need the count.
    ///
    /// ⟨issue 23⟩ NO production caller remains: d50 was the last one and now
    /// reads `writes_physical_tables_count_of`, like d8. The invariant that
    /// replaced it — GATES read the PHYSICAL count (`d8.rs:41`, `d50.rs:82`),
    /// WITNESS sets read the INCLUSIVE one (`d2.rs:425`, and `transaction_spans`
    /// :160, which backs d50's `affectedTables`).
    pub fn writes_tables_count_of(&self, routine_id: &str) -> usize {
        let r = &self.row(routine_id).table_writes_all;
        (r.end - r.start) as usize
    }

    /// Sorted + deduped PHYSICAL (non-known-temp) written TableIds
    /// (`writes_physical_tables_of`).
    pub fn writes_physical_tables_of(&self, routine_id: &str) -> Vec<String> {
        let ids = window(
            &self.phys_writes_pool,
            &self.row(routine_id).physical_table_writes,
        );
        let mut out: Vec<String> = ids
            .iter()
            .map(|(id, _)| self.interner.resolve(*id).to_string())
            .collect();
        out.sort();
        out
    }

    /// `writes_physical_tables_of(routine_id).len()` without resolving or
    /// allocating a single `String` — the window is already sorted-and-deduped
    /// (`freeze_masked` merges masks for equal ids), so its length IS the
    /// distinct-table count. ⟨C1 Task 2 fix M2⟩ For callers (d8) that only
    /// need the count.
    pub fn writes_physical_tables_count_of(&self, routine_id: &str) -> usize {
        let r = &self.row(routine_id).physical_table_writes;
        (r.end - r.start) as usize
    }

    /// Sorted + deduped PHYSICAL (non-known-temp) READ TableIds — d44's read set.
    pub fn physical_table_reads_of(&self, routine_id: &str) -> Vec<String> {
        self.resolve_sorted(window(
            &self.phys_reads_pool,
            &self.row(routine_id).physical_table_reads,
        ))
    }

    /// Sorted + deduped published EventIds (`publishes_events_of`).
    pub fn publishes_events_of(&self, routine_id: &str) -> Vec<String> {
        self.resolve_sorted(window(
            &self.events_pool,
            &self.row(routine_id).event_publishes,
        ))
    }

    /// The routine's written-TableId window as interned ids, BORROWED — the
    /// allocation-free half of [`Self::writes_tables_of`].
    ///
    /// The window is sorted and deduped BY ID (`freeze_ids`), which is NOT the
    /// order of the resolved strings, so a caller that needs string order must
    /// resolve and sort. A caller that unions MANY routines' windows should union
    /// the ids (see [`ResBitset`]) and resolve once — that is what this accessor
    /// exists for: `transaction_spans` was allocating 261,772,789 `String`s per BC
    /// Base App run through the `String` variant, one fresh `Vec<String>` per
    /// visited routine, every element of which went straight into a
    /// `BTreeSet<String>` and was dropped.
    pub fn writes_table_ids_of(&self, routine_id: &str) -> &[ResId] {
        window(
            &self.writes_all_pool,
            &self.row(routine_id).table_writes_all,
        )
    }

    /// The routine's published-EventId window as interned ids, BORROWED. Same
    /// contract as [`Self::writes_table_ids_of`].
    pub fn event_ids_of(&self, routine_id: &str) -> &[ResId] {
        window(&self.events_pool, &self.row(routine_id).event_publishes)
    }

    /// Resolve one interned resource id — for callers that union borrowed id
    /// windows and materialize the result themselves.
    pub fn resolve_res(&self, id: ResId) -> &str {
        self.interner.resolve(id)
    }

    /// Number of distinct interned resource ids — the width a [`ResBitset`] needs
    /// in order to hold any set this store can produce.
    pub fn res_universe_len(&self) -> usize {
        self.interner.len()
    }

    /// PHYSICAL written TableId → its op set, in d44's `BTreeSet<&str>` order
    /// (`delete, insert, modify`) — d44's per-table op-union.
    pub fn physical_table_write_ops_of(
        &self,
        routine_id: &str,
    ) -> BTreeMap<String, Vec<&'static str>> {
        let ids = window(
            &self.phys_writes_pool,
            &self.row(routine_id).physical_table_writes,
        );
        ids.iter()
            .map(|(id, mask)| {
                (
                    self.interner.resolve(*id).to_string(),
                    decode_op_mask(*mask),
                )
            })
            .collect()
    }

    /// Resolve a pooled id run to sorted `String`s. The run is already deduped by
    /// [`ResId`] and interning is injective, so the resolved strings are distinct
    /// — identical to the raw helpers' `BTreeSet<String>` collect.
    fn resolve_sorted(&self, ids: &[ResId]) -> Vec<String> {
        let mut out: Vec<String> = ids
            .iter()
            .map(|id| self.interner.resolve(*id).to_string())
            .collect();
        out.sort();
        out
    }

    /// ⟨C1 census⟩ Pool/row sizes for the `C1_CONE_CENSUS` diagnostic (see
    /// `cone_census.rs`'s module doc for the accounting convention). Every
    /// pool here stores plain `ResId`/`(ResId, u8)` values — no owned strings
    /// — so its `_bytes` fields are pure backing-buffer footprint
    /// (`len() * size_of::<T>()`); the interner is the ONLY owner of the
    /// actual id text, reported separately via [`ResInterner::census_heap_bytes`].
    pub fn census(&self) -> ConeDerivedCensus {
        ConeDerivedCensus {
            rows: self.rows.len(),
            rows_key_heap_bytes: self.rows.keys().map(|k| k.len() as u64).sum(),
            rows_struct_bytes: (self.rows.len() * size_of::<ConeDerivedRow>()) as u64,
            interner_strings: self.interner.len(),
            interner_heap_bytes: self.interner.census_heap_bytes(),
            writes_all_len: self.writes_all_pool.len(),
            writes_all_bytes: (self.writes_all_pool.len() * size_of::<ResId>()) as u64,
            phys_writes_len: self.phys_writes_pool.len(),
            phys_writes_bytes: (self.phys_writes_pool.len() * size_of::<(ResId, u8)>()) as u64,
            phys_reads_len: self.phys_reads_pool.len(),
            phys_reads_bytes: (self.phys_reads_pool.len() * size_of::<ResId>()) as u64,
            events_len: self.events_pool.len(),
            events_bytes: (self.events_pool.len() * size_of::<ResId>()) as u64,
        }
    }
}

/// ⟨C1 census⟩ Byte/row accounting for [`ConeDerivedStore::census`] — the
/// `C1_CONE_CENSUS` diagnostic only, not used by any production query.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConeDerivedCensus {
    pub rows: usize,
    /// Heap bytes of the `rows` HashMap's owned `String` keys (routine ids).
    pub rows_key_heap_bytes: u64,
    /// `rows.len() * size_of::<ConeDerivedRow>()` — the rows table's own
    /// backing-buffer footprint (the routine-id keys' heap bytes are counted
    /// separately above).
    pub rows_struct_bytes: u64,
    pub interner_strings: usize,
    pub interner_heap_bytes: u64,
    pub writes_all_len: usize,
    pub writes_all_bytes: u64,
    pub phys_writes_len: usize,
    pub phys_writes_bytes: u64,
    pub phys_reads_len: usize,
    pub phys_reads_bytes: u64,
    pub events_len: usize,
    pub events_bytes: u64,
}

// ---------------------------------------------------------------------------
// The fold.
// ---------------------------------------------------------------------------

/// Folds reachable facts into per-routine [`ConeDerivedRow`]s. One builder per
/// cone composition; the scratch buffers are reused across routines so a
/// 100k-routine walk allocates the four pools and nothing else.
///
/// Protocol per routine: [`begin_routine`](Self::begin_routine) →
/// [`fold_fact`](Self::fold_fact)* → [`finish_routine`](Self::finish_routine).
#[derive(Debug, Default)]
pub struct ConeDerivedBuilder {
    store: ConeDerivedStore,
    flags: u8,
    s_writes_all: Vec<ResId>,
    s_phys_writes: Vec<(ResId, u8)>,
    s_phys_reads: Vec<ResId>,
    s_events: Vec<ResId>,
}

impl ConeDerivedBuilder {
    /// Start a fresh routine (clears the scratch buffers, keeping their capacity).
    pub fn begin_routine(&mut self) {
        self.flags = 0;
        self.s_writes_all.clear();
        self.s_phys_writes.clear();
        self.s_phys_reads.clear();
        self.s_events.clear();
    }

    /// Fold ONE reachable fact into the routine in progress. Reads only fields
    /// `retag` does not rewrite (`op` / `resource_kind` / `resource_id` /
    /// `extra`), so a pre-retag representative folds identically to its retagged
    /// clone. Idempotent per distinct `(kind, op, resource_id, temp)` — folding a
    /// fact twice is a no-op after the freeze dedup.
    pub fn fold_fact(&mut self, f: &CapabilityFact) {
        match f.resource_kind {
            "table" => {
                self.flags |= TOUCHES_TABLE;
                let Some(rid) = f.resource_id.as_deref() else {
                    return;
                };
                let is_temp = fact_is_known_temp(f);
                if let Some(bit) = write_op_bit(f.op) {
                    let id = self.store.interner.intern(rid);
                    self.s_writes_all.push(id);
                    if !is_temp {
                        self.s_phys_writes.push((id, bit));
                    }
                } else if f.op == "read" && !is_temp {
                    let id = self.store.interner.intern(rid);
                    self.s_phys_reads.push(id);
                }
            }
            "transaction" => {
                if f.op == "commit" {
                    self.flags |= MAY_COMMIT;
                }
            }
            "event" => {
                if f.op == "publish"
                    && let Some(rid) = f.resource_id.as_deref()
                {
                    let id = self.store.interner.intern(rid);
                    self.s_events.push(id);
                }
            }
            // ⟨C1 Task 2 fix I1⟩ Routed through `io_kind_bit` — the ONE
            // definition of "is this IO" — instead of hardcoding `"http"` /
            // `"file"` arms here, so a future IO kind added there is
            // automatically visible to the fold too.
            kind => {
                if let Some(bit) = io_kind_bit(kind) {
                    self.flags |= bit;
                }
            }
        }
    }

    /// Freeze the routine in progress into the pools and record its row.
    pub fn finish_routine(&mut self, routine_id: &str) {
        let table_writes_all = freeze_ids(&mut self.s_writes_all, &mut self.store.writes_all_pool);
        let physical_table_writes =
            freeze_masked(&mut self.s_phys_writes, &mut self.store.phys_writes_pool);
        let physical_table_reads =
            freeze_ids(&mut self.s_phys_reads, &mut self.store.phys_reads_pool);
        let event_publishes = freeze_ids(&mut self.s_events, &mut self.store.events_pool);
        self.store.rows.insert(
            routine_id.to_string(),
            ConeDerivedRow {
                flags: self.flags,
                table_writes_all,
                physical_table_writes,
                physical_table_reads,
                event_publishes,
            },
        );
    }

    /// Fold ONE routine's complete reachable sequence in a single call
    /// (begin → fold each → finish).
    ///
    /// The production cone does NOT use this: its inherited half folds
    /// key-deduped representatives, not a flat reachable list (see the module
    /// docs' dedup-asymmetry note). It exists for callers that already hold the
    /// literal reachable sequence — hand-built fixture summaries, whose inherited
    /// facts ARE the input rather than a cone output.
    ///
    /// ⟨fix M3⟩ `#[cfg(test)]` + `pub(crate)` keep that a STRUCTURAL guarantee
    /// rather than just a doc warning. ⟨issue 33⟩ Its original justification —
    /// that folding a flat reachable list in the production cone would reproduce
    /// the R3 dedup hazard — no longer holds: with the temp class in
    /// `inherited_fact_key` the fold is dedup-invariant, so raw and deduped
    /// inputs now agree. The confinement stays as a cost and clarity boundary
    /// (the production path must not pay for a flat reachable list), not as a
    /// correctness guard. Its only caller (`l5::test_support::cone_store_of`) is
    /// itself `#[cfg(test)]`-only.
    #[cfg(test)]
    pub(crate) fn fold_routine<'f>(
        &mut self,
        routine_id: &str,
        reachable: impl IntoIterator<Item = &'f CapabilityFact>,
    ) {
        self.begin_routine();
        for f in reachable {
            self.fold_fact(f);
        }
        self.finish_routine(routine_id);
    }

    /// Consume the builder, yielding the frozen store.
    pub fn finish(self) -> ConeDerivedStore {
        self.store
    }
}

/// Sort + dedup `scratch` and append it to `pool`, returning its window.
fn freeze_ids(scratch: &mut Vec<ResId>, pool: &mut Vec<ResId>) -> Range<u32> {
    scratch.sort_unstable();
    scratch.dedup();
    let start = pool.len() as u32;
    pool.extend_from_slice(scratch);
    start..pool.len() as u32
}

/// Sort `scratch` by id, OR-merge the masks of equal ids, and append to `pool`.
fn freeze_masked(scratch: &mut [(ResId, u8)], pool: &mut Vec<(ResId, u8)>) -> Range<u32> {
    scratch.sort_unstable();
    let start = pool.len();
    for &(id, mask) in scratch.iter() {
        // Merge only within THIS routine's window — never into the previous
        // routine's last entry.
        if pool.len() > start
            && let Some(last) = pool.last_mut()
            && last.0 == id
        {
            last.1 |= mask;
            continue;
        }
        pool.push((id, mask));
    }
    start as u32..pool.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l2::features::PTempState;
    use crate::engine::l4::capability_cone::compose_cone_over_graph;
    use crate::engine::l4::combined_graph::{CombinedGraph, TypedEdge};
    use std::collections::HashMap;

    // -- fixture constructors -------------------------------------------------

    fn fact(
        subject: &str,
        op: &'static str,
        kind: &'static str,
        rid: Option<&str>,
    ) -> CapabilityFact {
        CapabilityFact {
            subject: subject.to_string(),
            op,
            resource_kind: kind,
            resource_id: rid.map(|s| s.to_string()),
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: None,
            extra: None,
        }
    }

    /// A table fact carrying an explicit `temp_state` (the physical/temp gate).
    fn temp_fact(
        subject: &str,
        op: &'static str,
        rid: &str,
        kind: &str,
        value: Option<bool>,
    ) -> CapabilityFact {
        let mut f = fact(subject, op, "table", Some(rid));
        f.extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(PTempState {
                kind: kind.to_string(),
                value,
                parameter_index: None,
            }),
            op_subtype: None,
        });
        f
    }

    fn call_edge(from: &str, to: &str, callsite: &str) -> TypedEdge {
        TypedEdge {
            kind: "direct-call".to_string(),
            from: from.to_string(),
            to: Some(to.to_string()),
            callsite_id: Some(callsite.to_string()),
            operation_id: None,
            event_id: None,
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
            target_object: None,
            object_type: None,
            target_id_source: None,
        }
    }

    fn graph_of(edges: Vec<TypedEdge>) -> CombinedGraph {
        CombinedGraph {
            nodes: Vec::new(),
            edges_by_from: HashMap::new(),
            edges_from_order: Vec::new(),
            uncertainty_edges: Vec::new(),
            typed_edges: edges,
        }
    }

    // -- ⟨issue 33⟩ mixed temp/physical keys ----------------------------------

    /// Every node reachable from `start` over `graph.typed_edges`, including
    /// `start` itself.
    fn reachable_from(graph: &CombinedGraph, start: &str) -> std::collections::BTreeSet<String> {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut queue = std::collections::VecDeque::new();
        seen.insert(start.to_string());
        queue.push_back(start.to_string());
        while let Some(cur) = queue.pop_front() {
            for e in &graph.typed_edges {
                if e.from != cur {
                    continue;
                }
                if let Some(to) = &e.to
                    && seen.insert(to.clone())
                {
                    queue.push_back(to.clone());
                }
            }
        }
        seen
    }

    /// ⟨issue 33 — A4b⟩ The oracle that replaces the two retired dedup-trap tests:
    /// for every routine in the graph handed to it, `writes_physical_tables_of` must
    /// equal the NAIVE union of physical (non-known-temp) table-write ids over the
    /// routine's whole reachable set — itself plus everything it can call,
    /// transitively.
    ///
    /// Before issue 33 this was FALSE by construction: `inherited_fact_key` omitted
    /// the temp class, so a known-temp representative could win a key outright and
    /// the physical fact hiding behind it never reached the fold. The derived answer
    /// was then a strict SUBSET of the naive union (the retired
    /// `temp_physical_dedup_trap_follows_the_winning_representative` pinned exactly
    /// that subsetting as intended behaviour). With the temp class in the key the two
    /// classes no longer collide, and the two answers coincide.
    ///
    /// **Stated limit on its reach.** The acceptance row this discharges is worded
    /// "for every routine in every fixture"; the delivery is narrower. It is called
    /// from THREE hand-built unit graphs in this module and never over
    /// `tests/r0-corpus/`. It is a genuine guard — all three fail on the un-widened
    /// key — but it is not a corpus-wide invariant. Widening it into the r3a3
    /// differential (compose with `ConeOutput::Both`, compare `out.derived` against
    /// the reachable-direct union) is cheap and is the honest way to earn that
    /// wording.
    ///
    /// It also cannot detect a raw↔key-deduped input swap any more, because after
    /// the key widening the two answers coincide by construction (see this module's
    /// docs). That is acceptable — it is coverage of something that can no longer go
    /// wrong — but it is not coverage the two retired tests used to provide.
    fn assert_physical_writes_equal_naive_union(
        label: &str,
        graph: &CombinedGraph,
        nodes: &[String],
        direct_in: &HashMap<String, Vec<CapabilityFact>>,
        derived: &ConeDerivedStore,
    ) {
        for n in nodes {
            let mut naive: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for r in reachable_from(graph, n) {
                for f in direct_in.get(&r).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if f.resource_kind != "table"
                        || write_op_bit(f.op).is_none()
                        || fact_is_known_temp(f)
                    {
                        continue;
                    }
                    if let Some(rid) = &f.resource_id {
                        naive.insert(rid.clone());
                    }
                }
            }
            let naive: Vec<String> = naive.into_iter().collect();
            assert_eq!(
                derived.writes_physical_tables_of(n),
                naive,
                "[{label}] writes_physical_tables_of({n}) != the naive union over {n}'s \
                 reachable direct facts — a physical obligation is being hidden by the \
                 cone's key collapse"
            );
        }
    }

    /// ⟨issue 33 — A1 + A8⟩ Two callees write table T with the same `(op,
    /// resource_kind, resource_id, confidence)` — one known-temp, one physical. The
    /// temp class is part of `inherited_fact_key`, so they no longer collide: BOTH
    /// survive as separate inherited facts and the ancestor's physical-write set
    /// contains T.
    ///
    /// A8 (additivity) is asserted here too: the representative the PRE-issue-33
    /// engine produced — the known-temp fact retagged through `cs/1`, which won the
    /// singleton path's `edge_sort_key` tie-break — is still present, field for
    /// field. The change only ADDS the physical fact beside it.
    #[test]
    fn a_mixed_temp_key_keeps_both_facts() {
        let graph = graph_of(vec![
            call_edge("r/root", "r/tempWriter", "cs/1"),
            call_edge("r/root", "r/physWriter", "cs/2"),
        ]);
        let nodes: Vec<String> = ["r/root", "r/tempWriter", "r/physWriter"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert("r/root".to_string(), Vec::new());
        direct_in.insert(
            "r/tempWriter".to_string(),
            vec![temp_fact(
                "r/tempWriter",
                "insert",
                "t/T",
                "known",
                Some(true),
            )],
        );
        direct_in.insert(
            "r/physWriter".to_string(),
            vec![temp_fact(
                "r/physWriter",
                "insert",
                "t/T",
                "known",
                Some(false),
            )],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        let root_inherited = &out.cones.get("r/root").expect("root cone").inherited;
        assert_eq!(
            root_inherited.len(),
            2,
            "a known-temp and a physical write of the same table are DIFFERENT \
             obligations and must both survive the key collapse, got {root_inherited:#?}"
        );
        let temps: Vec<&CapabilityFact> = root_inherited
            .iter()
            .filter(|f| fact_is_known_temp(f))
            .collect();
        let phys: Vec<&CapabilityFact> = root_inherited
            .iter()
            .filter(|f| !fact_is_known_temp(f))
            .collect();
        assert_eq!(temps.len(), 1, "exactly one known-temp survivor");
        assert_eq!(phys.len(), 1, "exactly one physical survivor");

        // A8 — the PRE-change winner is unchanged, field for field. (`cs/1` is the
        // edge_sort_key winner, so this is the fact the old engine emitted alone.)
        let kept = temps[0];
        assert_eq!(kept.subject, "r/root");
        assert_eq!(kept.op, "insert");
        assert_eq!(kept.resource_kind, "table");
        assert_eq!(kept.resource_id.as_deref(), Some("t/T"));
        assert_eq!(kept.resource_arg_source, None);
        assert_eq!(kept.confidence, "static");
        assert_eq!(kept.provenance, "inherited");
        assert_eq!(kept.via, "call");
        assert_eq!(kept.witness_operation_id, None);
        assert_eq!(kept.witness_callsite_id.as_deref(), Some("cs/1"));
        assert_eq!(
            kept.extra,
            Some(CapabilityExtra::Table {
                record_variable_id: None,
                temp_state: Some(PTempState {
                    kind: "known".to_string(),
                    value: Some(true),
                    parameter_index: None,
                }),
                op_subtype: None,
            })
        );

        // The physical obligation is no longer hidden.
        assert_eq!(out.derived.writes_physical_tables_of("r/root"), vec!["t/T"]);
        // ...and temp-INCLUSIVE writes are untouched.
        assert_eq!(out.derived.writes_tables_of("r/root"), vec!["t/T"]);

        assert_physical_writes_equal_naive_union(
            "mixed-key",
            &graph,
            &nodes,
            &direct_in,
            &out.derived,
        );
    }

    /// ⟨issue 33 — A2⟩ The preference is NOT a distance tie-break. A known-temp
    /// write at distance 1 and a physical write of the same table at distance 2 are
    /// different keys, so the nearer temp fact cannot hide the farther physical one:
    /// `writes_physical_tables_of(root)` contains the table.
    ///
    /// Pre-issue-33 the shared key resolved by MIN DISTANCE, the temp fact at
    /// distance 1 won outright, and the root's physical-write set was EMPTY.
    #[test]
    fn a_physical_write_at_distance_2_survives_a_temp_write_at_distance_1() {
        let graph = graph_of(vec![
            call_edge("r/root", "r/tempWriter", "cs/1"),
            call_edge("r/root", "r/mid", "cs/2"),
            call_edge("r/mid", "r/physWriter", "cs/3"),
        ]);
        let nodes: Vec<String> = ["r/root", "r/tempWriter", "r/mid", "r/physWriter"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert("r/root".to_string(), Vec::new());
        direct_in.insert("r/mid".to_string(), Vec::new());
        direct_in.insert(
            "r/tempWriter".to_string(),
            vec![temp_fact(
                "r/tempWriter",
                "insert",
                "t/T",
                "known",
                Some(true),
            )],
        );
        direct_in.insert(
            "r/physWriter".to_string(),
            vec![temp_fact(
                "r/physWriter",
                "insert",
                "t/T",
                "known",
                Some(false),
            )],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        let root_inherited = &out.cones.get("r/root").expect("root cone").inherited;
        assert_eq!(
            root_inherited.len(),
            2,
            "the nearer temp fact must not displace the farther physical one, \
             got {root_inherited:#?}"
        );
        assert_eq!(
            out.derived.writes_physical_tables_of("r/root"),
            vec!["t/T"],
            "the physical write at distance 2 must reach the root's physical-write set"
        );

        assert_physical_writes_equal_naive_union(
            "distance",
            &graph,
            &nodes,
            &direct_in,
            &out.derived,
        );
    }

    /// ⟨issue 33 — A4b⟩ The same oracle over a RECURSIVE SCC, which reaches the BFS
    /// reduction site (`inherited_facts_by_bfs`) rather than the singleton one. This
    /// is the fixture the retired `bfs_sibling_facts_use_the_key_deduped_direct_map`
    /// used; under the widened key both of each pair's classes survive, so the
    /// sibling half and the subject's own half agree with the naive union.
    #[test]
    fn a_mixed_temp_keys_in_a_recursive_scc_keep_both_classes() {
        let graph = graph_of(vec![
            call_edge("r/a", "r/b", "cs/ab"),
            call_edge("r/b", "r/a", "cs/ba"),
        ]);
        let nodes: Vec<String> = ["r/a", "r/b"].iter().map(|s| s.to_string()).collect();

        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert(
            "r/a".to_string(),
            vec![
                temp_fact("r/a", "modify", "t/Own", "known", Some(true)),
                temp_fact("r/a", "modify", "t/Own", "unknown", None),
            ],
        );
        direct_in.insert(
            "r/b".to_string(),
            vec![
                temp_fact("r/b", "insert", "t/Sib", "known", Some(true)),
                temp_fact("r/b", "insert", "t/Sib", "unknown", None),
            ],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        let a_inherited = &out.cones.get("r/a").expect("a cone").inherited;
        assert_eq!(
            a_inherited.len(),
            2,
            "the sibling's known-temp and unknown-temp facts are different keys and \
             must both arrive, got {a_inherited:#?}"
        );
        assert_eq!(
            a_inherited.iter().filter(|f| fact_is_known_temp(f)).count(),
            1
        );
        assert_eq!(
            out.derived.writes_physical_tables_of("r/a"),
            vec!["t/Own", "t/Sib"],
            "the sibling's non-known-temp write must no longer be hidden by its \
             known-temp twin"
        );
        assert_eq!(
            out.derived.writes_tables_of("r/a"),
            vec!["t/Own", "t/Sib"],
            "temp-INCLUSIVE writes are unaffected by the key widening"
        );

        assert_physical_writes_equal_naive_union(
            "recursive-scc",
            &graph,
            &nodes,
            &direct_in,
            &out.derived,
        );
    }

    // -- R7: d44's op order is lexical ----------------------------------------

    /// One routine writing ONE table with all three ops: the mask decoder must
    /// emit `delete, insert, modify` — the order d44's `BTreeSet<&str>` yields.
    #[test]
    fn d44_op_mask_decodes_in_lexical_order() {
        assert_eq!(
            decode_op_mask(OP_INSERT | OP_MODIFY | OP_DELETE),
            vec!["delete", "insert", "modify"]
        );

        let graph = graph_of(Vec::new());
        let nodes: Vec<String> = vec!["r/w".to_string()];
        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert(
            "r/w".to_string(),
            vec![
                fact("r/w", "insert", "table", Some("t/A")),
                fact("r/w", "modify", "table", Some("t/A")),
                fact("r/w", "delete", "table", Some("t/A")),
            ],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);
        let ops = out.derived.physical_table_write_ops_of("r/w");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops["t/A"], vec!["delete", "insert", "modify"]);
    }

    // -- flags + the coverage tri-state arms -----------------------------------

    /// Flag presence is op/kind-exact, and the absent-fact tri-state is decided
    /// by `coverage.inherited_status`: complete ⇒ No, partial ⇒ Unknown.
    #[test]
    fn flags_and_coverage_tristate_arms() {
        let graph = graph_of(Vec::new());
        let nodes: Vec<String> = ["r/complete", "r/partial", "r/io"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert("r/complete".to_string(), Vec::new());
        direct_in.insert("r/partial".to_string(), Vec::new());
        direct_in.insert(
            "r/io".to_string(),
            vec![
                fact("r/io", "send", "http", None),
                fact("r/io", "write", "file", None),
                fact("r/io", "commit", "transaction", None),
                // A non-commit transaction op must NOT set MAY_COMMIT.
                fact("r/io", "rollback", "transaction", None),
            ],
        );
        let mut coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();
        coverage_in.insert("r/complete".to_string(), ("complete".to_string(), vec![]));
        coverage_in.insert("r/partial".to_string(), ("partial".to_string(), vec![]));

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        // No facts ⇒ no flags; the tri-state then reads coverage.
        assert!(!out.derived.touches_table("r/complete"));
        assert!(!out.derived.may_commit_flag("r/complete"));
        assert_eq!(
            out.cones["r/complete"].coverage.inherited_status, "complete",
            "absent fact + complete ⇒ the No arm"
        );
        assert_eq!(
            out.cones["r/partial"].coverage.inherited_status, "partial",
            "absent fact + partial ⇒ the Unknown arm"
        );

        // ⟨fix I1⟩ The two asserts above only pin the INGREDIENTS (absent flags,
        // the coverage string) — never an actual `EffectPresence`. Swapping the
        // two arms of `l5::capability_query::presence` (absent+complete ⇒
        // Unknown, everything else ⇒ No) would leave this test green without
        // these. Build a `FullRoutineSummary` per routine and call the derived
        // tri-state helpers directly so a swap is caught here.
        {
            use crate::engine::l4::capability_cone::CoverageRecord;
            use crate::engine::l5::capability_query::{
                EffectPresence, may_commit_derived, touches_db_derived,
            };
            use crate::engine::l5::full_summary::FullRoutineSummary;

            let cov = |status: &str| {
                Some(CoverageRecord {
                    subject: "r".to_string(),
                    direct_status: status.to_string(),
                    inherited_status: status.to_string(),
                    reasons: Vec::new(),
                    unknown_targets: Vec::new(),
                })
            };
            // ⟨C1 Task 3⟩ `None` inherited — the DERIVED-ONLY shape the analyze
            // path now produces. The two tri-states must resolve entirely off the
            // store's flags + the summary's coverage, never touching a raw Vec
            // (`inherited_raw()` would panic if they did).
            let complete_summary = FullRoutineSummary::new(
                "r/complete".to_string(),
                Vec::new(),
                None,
                cov("complete"),
            );
            let partial_summary =
                FullRoutineSummary::new("r/partial".to_string(), Vec::new(), None, cov("partial"));
            assert_eq!(
                touches_db_derived(&out.derived, &complete_summary),
                EffectPresence::No,
                "absent fact + complete coverage must resolve to the No arm"
            );
            assert_eq!(
                may_commit_derived(&out.derived, &complete_summary),
                EffectPresence::No,
                "absent fact + complete coverage must resolve to the No arm"
            );
            assert_eq!(
                touches_db_derived(&out.derived, &partial_summary),
                EffectPresence::Unknown,
                "absent fact + partial coverage must resolve to the Unknown arm"
            );
            assert_eq!(
                may_commit_derived(&out.derived, &partial_summary),
                EffectPresence::Unknown,
                "absent fact + partial coverage must resolve to the Unknown arm"
            );
        }

        assert!(out.derived.touches_io("r/io"));
        assert_eq!(
            out.derived.flags_of("r/io"),
            TOUCHES_HTTP | TOUCHES_FILE | MAY_COMMIT
        );
        assert!(!out.derived.touches_table("r/io"));
    }

    // -- the mode is a real skip ----------------------------------------------

    /// `DerivedOnly` must produce a fully-populated derived row AND an empty raw
    /// Vec; `RawOnly` the mirror image; and the derived rows must be identical
    /// under `DerivedOnly` and `Both` (one traversal, two guarded emissions).
    #[test]
    fn cone_output_mode_skips_the_raw_vec_without_perturbing_the_fold() {
        let graph = graph_of(vec![call_edge("r/root", "r/callee", "cs/1")]);
        let nodes: Vec<String> = ["r/root", "r/callee"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert(
            "r/root".to_string(),
            vec![fact("r/root", "publish", "event", Some("e/E"))],
        );
        direct_in.insert(
            "r/callee".to_string(),
            vec![
                fact("r/callee", "insert", "table", Some("t/A")),
                fact("r/callee", "read", "table", Some("t/B")),
            ],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let both =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);
        let derived_only = compose_cone_over_graph(
            &graph,
            &nodes,
            &direct_in,
            &coverage_in,
            ConeOutput::DerivedOnly,
        );
        let raw_only = compose_cone_over_graph(
            &graph,
            &nodes,
            &direct_in,
            &coverage_in,
            ConeOutput::RawOnly,
        );

        // DerivedOnly: the raw Vec is never built...
        assert!(
            derived_only.cones.values().all(|c| c.inherited.is_empty()),
            "DerivedOnly must not materialize the raw inherited Vec"
        );
        // ...but the row is fully populated, and identical to `Both`'s.
        assert_eq!(derived_only.derived.writes_tables_of("r/root"), vec!["t/A"]);
        assert_eq!(
            derived_only.derived.physical_table_reads_of("r/root"),
            vec!["t/B"]
        );
        assert_eq!(
            derived_only.derived.publishes_events_of("r/root"),
            vec!["e/E"]
        );
        for id in &nodes {
            assert_eq!(
                derived_only.derived.writes_tables_of(id),
                both.derived.writes_tables_of(id),
                "{id}: DerivedOnly and Both must fold identically"
            );
            assert_eq!(
                derived_only.derived.flags_of(id),
                both.derived.flags_of(id),
                "{id}: flags diverged between modes"
            );
            assert_eq!(
                derived_only.derived.physical_table_write_ops_of(id),
                both.derived.physical_table_write_ops_of(id),
                "{id}: op masks diverged between modes"
            );
        }
        // `Both` still returns the raw Vec unchanged.
        assert_eq!(both.cones["r/root"].inherited.len(), 2);

        // RawOnly: the raw Vec is byte-identical to `Both`'s; no row is folded.
        assert_eq!(
            raw_only.cones["r/root"].inherited,
            both.cones["r/root"].inherited
        );
        assert!(raw_only.derived.is_empty());
    }

    // -- N-A: the count helpers must equal the resolved Vec's length ----------

    /// `writes_tables_count_of`/`writes_physical_tables_count_of` return the
    /// frozen window's length directly, without resolving a single `String` —
    /// that only holds because `freeze_ids`/`freeze_masked` dedup by [`ResId`]
    /// and interning is injective, so the window's length IS the resolved
    /// Vec's length. Nothing else asserts that equality, and it now carries
    /// d8's and d50's `>= 3`-distinct-table gates: a future change to
    /// `freeze_masked` that ever left a duplicate id in a window would move
    /// those gates with no named cause. The retired `cone_parity` oracle would
    /// NOT have caught it either (it compared the resolved Vec against the raw
    /// helper, and both would have carried the same duplicate) — which is why
    /// this test, not that one, was always the right home for the pin. ⟨Task 3⟩
    /// The oracle is now gone; this invariant is not.
    ///
    /// The fixture writes THREE distinct tables and repeats one exact
    /// `(table, op)` pair (`insert t/A` twice, straight in the RAW direct
    /// Vec — the self half is un-deduped by construction, see the module
    /// docs' dedup-asymmetry note) so a window of length 0 or 1 can never
    /// accidentally satisfy this.
    #[test]
    fn count_helpers_equal_the_resolved_vecs_length() {
        let graph = graph_of(Vec::new());
        let nodes: Vec<String> = vec!["r/multi".to_string()];
        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert(
            "r/multi".to_string(),
            vec![
                fact("r/multi", "insert", "table", Some("t/A")),
                // An exact duplicate of the fact above — pins that a repeated
                // (table, op) pair collapses to ONE window entry, not two.
                fact("r/multi", "insert", "table", Some("t/A")),
                fact("r/multi", "modify", "table", Some("t/B")),
                // A known-temp write: counts toward the temp-INCLUSIVE window
                // but not the physical one, so the two counts genuinely differ.
                temp_fact("r/multi", "insert", "t/C", "known", Some(true)),
            ],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        // Fixture preconditions: multiple distinct tables, and the two windows
        // are of different (non-0/1) lengths.
        assert_eq!(
            out.derived.writes_tables_of("r/multi"),
            vec!["t/A", "t/B", "t/C"]
        );
        assert_eq!(
            out.derived.writes_physical_tables_of("r/multi"),
            vec!["t/A", "t/B"]
        );

        // The discriminating assertions.
        assert_eq!(
            out.derived.writes_tables_count_of("r/multi"),
            out.derived.writes_tables_of("r/multi").len(),
            "writes_tables_count_of must equal writes_tables_of(...).len()"
        );
        assert_eq!(
            out.derived.writes_physical_tables_count_of("r/multi"),
            out.derived.writes_physical_tables_of("r/multi").len(),
            "writes_physical_tables_count_of must equal writes_physical_tables_of(...).len()"
        );

        // Also hold for the absent-row case (an id the store never folded).
        assert_eq!(out.derived.writes_tables_count_of("r/absent"), 0);
        assert_eq!(out.derived.writes_physical_tables_count_of("r/absent"), 0);
    }

    // -- the drop rules -------------------------------------------------------

    /// ⟨C1 Task 3⟩ RELOCATED PINS. `l5::capability_query`'s raw
    /// `writes_tables_of` / `writes_physical_tables_of` / `publishes_events_of`
    /// are deleted with the raw Vec (R6), and their unit tests went with them.
    /// The drop rules those tests pinned are properties of the FOLD now, so they
    /// are re-pinned here against the store:
    ///   - a fact with NO `resource_id` is dropped from every id-set (its
    ///     resource identity is unresolved);
    ///   - a table READ is not a table WRITE;
    ///   - a foreign `resource_kind` never enters the table sets, and a
    ///     non-`publish` op never enters the event set;
    ///   - a known-temp write counts in the temp-INCLUSIVE set but not the
    ///     physical one, while `unknown`/absent temp_state stays physical
    ///     (suppression-direction safe).
    #[test]
    fn fold_drops_unresolved_ids_foreign_kinds_and_known_temp_writes() {
        let graph = graph_of(Vec::new());
        let nodes: Vec<String> = vec!["r/x".to_string()];
        let mut direct_in: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
        direct_in.insert(
            "r/x".to_string(),
            vec![
                fact("r/x", "insert", "table", Some("t/B")), // write, kept
                fact("r/x", "modify", "table", Some("t/A")), // write, kept
                fact("r/x", "modify", "table", Some("t/A")), // dup → deduped
                fact("r/x", "delete", "table", None),        // no resource_id → dropped
                fact("r/x", "read", "table", Some("t/C")),   // read → not a write
                fact("r/x", "insert", "event", Some("e/X")), // foreign kind → no table
                fact("r/x", "publish", "event", Some("e/B")),
                fact("r/x", "publish", "event", Some("e/A")),
                fact("r/x", "publish", "event", Some("e/A")), // dup
                fact("r/x", "publish", "event", None),        // no resource_id → dropped
                fact("r/x", "subscribe", "event", Some("e/Z")), // wrong op → dropped
                fact("r/x", "publish", "table", Some("t/Q")), // publish on a table: not an event
                temp_fact("r/x", "insert", "t/Temp", "known", Some(true)), // temp
                temp_fact("r/x", "insert", "t/Phys", "known", Some(false)), // physical
                temp_fact("r/x", "insert", "t/Unk", "unknown", None), // physical (unknown)
            ],
        );
        let coverage_in: HashMap<String, (String, Vec<String>)> = HashMap::new();

        let out =
            compose_cone_over_graph(&graph, &nodes, &direct_in, &coverage_in, ConeOutput::Both);

        // `t/C` is a read, `t/Q`'s op is `publish` (not a write op) — neither is a
        // write. `e/X`'s kind is `event`, so it never reaches the table sets.
        assert_eq!(
            out.derived.writes_tables_of("r/x"),
            vec!["t/A", "t/B", "t/Phys", "t/Temp", "t/Unk"]
        );
        // The known-temp write is the ONLY one dropped from the physical set.
        assert_eq!(
            out.derived.writes_physical_tables_of("r/x"),
            vec!["t/A", "t/B", "t/Phys", "t/Unk"]
        );
        // Reads: only the genuine `read` on a non-temp table.
        assert_eq!(out.derived.physical_table_reads_of("r/x"), vec!["t/C"]);
        // Events: `publish` + `event` + a resource_id, deduped and sorted.
        assert_eq!(out.derived.publishes_events_of("r/x"), vec!["e/A", "e/B"]);
    }

    // -- interner invariants ---------------------------------------------------

    #[test]
    fn interner_is_lossless_and_stable() {
        let mut i = ResInterner::default();
        assert!(i.is_empty());
        let a = i.intern("t/A");
        let b = i.intern("t/B");
        assert_eq!(i.intern("t/A"), a, "re-interning must be stable");
        assert_ne!(a, b);
        assert_eq!(i.resolve(a), "t/A");
        assert_eq!(i.resolve(b), "t/B");
        assert_eq!(i.len(), 2);
    }

    // -- borrowed id windows (the allocation-free half of the query surface) ---

    /// The id accessors expose exactly the ids the `String` accessors resolve.
    /// Stated over TWO routines with DIFFERENT windows so an accessor that
    /// ignores the row and returns the whole pool cannot pass.
    #[test]
    fn id_accessors_agree_with_the_string_accessors() {
        let mut b = ConeDerivedBuilder::default();
        // `t/B` is declared BEFORE `t/A`, so intern order is not string order —
        // an accessor that skipped the resolve-then-sort would show here too.
        b.fold_routine(
            "r/one",
            [
                fact("r/one", "insert", "table", Some("t/B")),
                fact("r/one", "insert", "table", Some("t/A")),
                fact("r/one", "publish", "event", Some("e/Z")),
            ]
            .iter(),
        );
        b.fold_routine(
            "r/two",
            [fact("r/two", "insert", "table", Some("t/A"))].iter(),
        );
        let store = b.finish();

        for rid in ["r/one", "r/two"] {
            let mut via_ids: Vec<String> = store
                .writes_table_ids_of(rid)
                .iter()
                .map(|id| store.resolve_res(*id).to_string())
                .collect();
            via_ids.sort();
            assert_eq!(
                via_ids,
                store.writes_tables_of(rid),
                "writes mismatch for {rid}"
            );

            let mut ev: Vec<String> = store
                .event_ids_of(rid)
                .iter()
                .map(|id| store.resolve_res(*id).to_string())
                .collect();
            ev.sort();
            assert_eq!(
                ev,
                store.publishes_events_of(rid),
                "events mismatch for {rid}"
            );
        }
        // The two rows are genuinely different — otherwise the loop above proves
        // nothing about windowing.
        assert_ne!(
            store.writes_table_ids_of("r/one").len(),
            store.writes_table_ids_of("r/two").len()
        );
    }

    #[test]
    fn res_bitset_dedupes_and_iterates_ascending() {
        let mut bs = ResBitset::new(8);
        bs.insert_all(&[5, 1, 5, 3]);
        bs.insert_all(&[3, 7]);
        assert_eq!(bs.iter_ids().collect::<Vec<ResId>>(), vec![1, 3, 5, 7]);
        bs.clear();
        assert_eq!(bs.iter_ids().collect::<Vec<ResId>>(), Vec::<ResId>::new());
    }

    /// A set wider than one 64-bit word — the boundary an off-by-one word index
    /// would silently cross.
    #[test]
    fn res_bitset_spans_word_boundaries() {
        let mut bs = ResBitset::new(200);
        bs.insert_all(&[0, 63, 64, 65, 127, 128, 199]);
        assert_eq!(
            bs.iter_ids().collect::<Vec<ResId>>(),
            vec![0, 63, 64, 65, 127, 128, 199]
        );
    }
}
