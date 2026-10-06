//! Task B1 (l4-dbeffect-store-and-retirement, Part B): the FROZEN-BASELINE
//! differential.
//!
//! Part A proved the v2 interned columnar `EffectStore` solver
//! (`compute_summaries_v2_with_leaves_core`) byte-identical — over the COMPLETE
//! internal `RoutineSummary` (effect sequence + order, `effect_key`,
//! op/table/operation, temp, via, `record_variable_id`, `uncertainties`,
//! `has_unresolved_calls`, `parameter_roles`, `in_recursive_cycle`) — to the
//! old Jacobi solver, per routine, on these fixtures + the CDO whole-program
//! case. Part B has now retired the old solver, so this harness asserts
//! `v2 == frozen-baseline`: a committed complete-internal snapshot captured from
//! v2 while old still existed (`tests/l4-summary-baseline/`, see its `README.md`
//! for provenance + the pre-deletion tag `l4-pre-jacobi-deletion` (`f295ef8`) for
//! forensic re-differencing).
//!
//! ## Capture provenance (spec Part B.1)
//!
//! The frozen baseline was captured from v2 with `baseline == old == v2` proven
//! in the same working tree. Through the R3b-delete and aldump-cut steps that
//! proof stayed live and CONTINUOUS — a `v2 == old` cross-check over every
//! fixture + the CDO whole-program parity test kept asserting it — until the
//! FINAL commit deleted the old solver and those two cross-checks together. What
//! remains is `v2 == frozen-baseline` ([`assert_parity`] /
//! [`cdo_whole_program_v2_matches_frozen_digest`]) as the permanent anchor.
//!
//! Regenerate (a deliberate, engine-intended re-freeze — a MEASUREMENT, never a
//! blind bless) with `REGEN_TEMP_GOLDENS=1 cargo test -p al-call-hierarchy
//! --test l4_summary_differential` (CDO digest additionally needs `CDO_WS`).

use std::collections::HashMap;
use std::path::PathBuf;

use al_sem::engine::l4::summary::RoutineSummary;
use al_sem::engine::l4::summary_runner::{FieldIndex, compute_summaries_v2_with_leaves_core};

// Task 9 (l4-summary-fixpoint-redesign): the CDO_WS/ENFORCE_CDO_WS gating helper
// lives in `tests/common/cdo.rs` (shared with `program_resolve_harness.rs` and
// siblings) — separate test-binary crates can't `use` each other's `mod`s, so
// every CDO-gated test file includes it via `#[path]`. See that file's doc
// comment for the skip/panic contract.
#[path = "common/cdo.rs"]
mod cdo;
use cdo::cdo_ws_or_enforce;

fn baseline_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("l4-summary-baseline")
}

fn regen() -> bool {
    std::env::var("REGEN_TEMP_GOLDENS").as_deref() == Ok("1")
}

/// Line-ending-insensitive compare (autocrlf=true checkouts; the baseline files
/// are `eol=lf`-pinned in `.gitattributes`, but strip `\r` defensively).
fn strip_cr(s: &str) -> String {
    s.replace('\r', "")
}

/// The canonical complete-internal serialization of a v2 output: sort the outer
/// map by `routine_id` (HashMap order is nondeterministic), then pretty-`Debug`.
/// `RoutineSummary` and every nested type derive `Debug`, so this captures ALL
/// internal fields (the complete surface, NOT `stable_summary_fingerprint`).
fn canonical(map: &HashMap<String, RoutineSummary>) -> String {
    let mut v: Vec<&RoutineSummary> = map.values().collect();
    v.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    format!("{v:#?}")
}

/// Assert v2 reproduces the frozen complete-internal `RoutineSummary` baseline,
/// with no fixed leaves.
fn assert_parity(name: &str) {
    assert_parity_with_leaves(name, &HashMap::new());
}

/// Like [`assert_parity`], but with an explicit `leaf_summaries` map — needed by
/// `fixed_leaf_in_scc`, whose whole point is a routine that arrives as a
/// pre-settled leaf rather than being recomputed.
fn assert_parity_with_leaves(name: &str, leaves: &HashMap<String, RoutineSummary>) {
    let (routines, graph, scc, fields, ub) = fixtures::build(name);
    // The `_core` fn also returns the roles cap-hit diagnostics, which are
    // irrelevant here (and empty on every fixture — roles converge).
    let (new, _diags) =
        compute_summaries_v2_with_leaves_core(&routines, &graph, &scc, &ub, &fields, leaves);
    // `canonical()` serializes `new.values()` sorted by `routine_id`, dropping the
    // map key — so before relying on it, check the key IS the value's
    // `routine_id` for every entry. This makes the value-only comparison below
    // provably no weaker than the retired `assert_eq!(v2_map, old_map)` (which
    // compared keys AND values): the key is recoverable from the value, so
    // nothing the old whole-map equality checked is lost.
    for (k, v) in &new {
        assert_eq!(
            &v.routine_id, k,
            "[{name}] map key {k:?} must equal the summary's routine_id {:?} — canonical() \
             keys identity on routine_id, so this invariant is what makes it no weaker than \
             the old HashMap equality",
            v.routine_id
        );
    }
    let actual = canonical(&new);
    let path = baseline_dir().join(format!("{name}.baseline.txt"));

    if regen() {
        std::fs::create_dir_all(baseline_dir()).expect("create l4-summary-baseline dir");
        std::fs::write(&path, actual.as_bytes())
            .unwrap_or_else(|e| panic!("[{name}] regen write {}: {e}", path.display()));
        eprintln!("REGEN l4-summary baseline: {}", path.display());
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "[{name}] read frozen baseline {} failed: {e} — \
             run REGEN_TEMP_GOLDENS=1 to (re)capture",
            path.display()
        )
    });
    assert_eq!(
        strip_cr(&actual),
        strip_cr(&expected),
        "[{name}] v2 diverged from the frozen complete-internal RoutineSummary baseline"
    );
}

#[test]
fn parity_linear_two_routine_known_effect() {
    assert_parity("linear_known");
}

#[test]
fn parity_recursive_self_loop() {
    assert_parity("recursive_self_loop");
}

#[test]
fn parity_recursive_pair_pd() {
    assert_parity("recursive_pair_pd");
}

#[test]
fn parity_pd_to_known() {
    assert_parity("pd_to_known");
}

#[test]
fn parity_pd_to_unknown() {
    assert_parity("pd_to_unknown");
}

#[test]
fn parity_multi_callsite_same_callee() {
    assert_parity("multi_callsite_same_callee");
}

#[test]
fn parity_via_collision() {
    assert_parity("via_collision");
}

// ---------------------------------------------------------------------------
// Task A2 via-rank-merge guards (spec Step 3 test list) — see the fixture
// doc comments in `mod fixtures` for what each one exercises and why the
// differential (byte-identical to the untouched OLD solver) is the oracle.
// ---------------------------------------------------------------------------

#[test]
fn parity_via_collision_edges_reversed() {
    assert_parity("via_collision_edges_reversed");
}

#[test]
fn parity_pd_substituted_via_collision() {
    assert_parity("pd_substituted_via_collision");
}

#[test]
fn parity_pd_substituted_via_collision_reversed() {
    assert_parity("pd_substituted_via_collision_reversed");
}

#[test]
fn parity_direct_terminal_beats_colliding_pd_substitution() {
    assert_parity("direct_terminal_beats_colliding_pd_substitution");
}

#[test]
fn parity_direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition() {
    assert_parity("direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition");
}

#[test]
fn parity_external_successor_pd() {
    assert_parity("external_successor_pd");
}

#[test]
fn parity_fixed_leaf_in_scc() {
    let leaves = fixtures::fixed_leaf_in_scc_leaves();
    assert_parity_with_leaves("fixed_leaf_in_scc", &leaves);
}

#[test]
fn parity_missing_routine_in_scc() {
    assert_parity("missing_routine_in_scc");
}

/// Item-8 (A3 store fix wave): a fixed leaf in an SCC carrying ≥2 OWN effects
/// at DIFFERENT vias — structurally exercises the multi-effect fixed-leaf
/// seed→project round-trip (`seed_fixed_leaf_rows` interns the leaf's own
/// effects + vias, `SummaryBundle::project_row` re-emits them), so the
/// per-effect via mapping through the compact store is tested independent of
/// the data-dependent single-effect `fixed_leaf_in_scc` above. The differential
/// (byte-identical to the untouched OLD solver) is the oracle.
#[test]
fn parity_multi_effect_fixed_leaf_in_scc() {
    let leaves = fixtures::multi_effect_fixed_leaf_in_scc_leaves();
    assert_parity_with_leaves("multi_effect_fixed_leaf_in_scc", &leaves);
}

/// SHA-256 (lowercase hex) of a string.
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// The PERMANENT CDO whole-program regression anchor (spec Part B.1): the CDO v2
/// output is too large (the source-only workspace's ~3685-routine population) to
/// commit as a readable baseline, so it is frozen as a **SHA-256 digest** over
/// the canonical complete-`RoutineSummary`
/// serialization (`tests/l4-summary-baseline/cdo-whole-program-digest.txt`).
///
/// The digest was captured (`REGEN_TEMP_GOLDENS=1`) while the old solver still
/// existed, with the `v2 == old` cross-assert green — so `digest == old == v2`
/// at capture (see this file's header + `tests/l4-summary-baseline/README.md`
/// for the pre-deletion commit). The old solver and that regen-time cross-assert
/// are now deleted; a re-freeze measures v2 alone.
///
/// Assembly is the source-only detector-context substrate (`no_deps`, empty leaf
/// map). Skips when `CDO_WS` is unset; panics under `ENFORCE_CDO_WS=1`. Run with:
/// ```text
/// CDO_WS=<path> cargo test -p al-call-hierarchy --test l4_summary_differential cdo_whole_program_v2_matches_frozen_digest -- --nocapture
/// ```
#[test]
fn cdo_whole_program_v2_matches_frozen_digest() {
    use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
    use al_sem::engine::l3::event_graph::build_event_graph;
    use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
    use al_sem::engine::l3::symbol_table::SymbolTable;
    use al_sem::engine::l4::combined_graph::build_combined_graph;
    use al_sem::engine::l4::scc::{SccInputGraph, tarjan_scc};

    let Some(ws_path) = cdo_ws_or_enforce() else {
        return;
    };

    let resolved = assemble_and_resolve_workspace_default(&ws_path)
        .expect("assemble_and_resolve_workspace_default must succeed on CDO_WS");
    let ws = &resolved.workspace;

    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let no_deps: Vec<DeclaredDependency> = Vec::new();
    let no_fetched: Vec<String> = Vec::new();
    let calls = resolve_calls(ws, &symbols, &no_deps, &no_fetched);
    let event_graph = build_event_graph(&ws.routines, &symbols);
    let graph = build_combined_graph(ws, &calls, &event_graph);

    let mut scc_adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        scc_adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &scc_adjacency,
    });

    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }
    let leaf_summaries: HashMap<String, RoutineSummary> = HashMap::new();

    let (new, _diags) = compute_summaries_v2_with_leaves_core(
        &ws.routines,
        &graph,
        &scc,
        &calls.upgraded_bindings,
        &field_index,
        &leaf_summaries,
    );

    // Same key↔routine_id invariant as `assert_parity_with_leaves` — see its
    // comment for why this makes `canonical()`'s value-only serialization no
    // weaker than a full `HashMap` equality over keys AND values.
    for (k, v) in &new {
        assert_eq!(
            &v.routine_id, k,
            "map key {k:?} must equal the summary's routine_id {:?} — canonical() keys \
             identity on routine_id, so this invariant is what makes it no weaker than the \
             old HashMap equality",
            v.routine_id
        );
    }

    let digest = sha256_hex(&canonical(&new));
    let path = baseline_dir().join("cdo-whole-program-digest.txt");

    if regen() {
        // The old-solver `v2 == old` cross-assert that guarded this capture was
        // removed with the old Jacobi solver (spec Part B). The digest was frozen
        // at parity — see this file's header + tests/l4-summary-baseline/README.md
        // (pre-deletion commit for forensic re-differencing). A fresh re-freeze
        // now measures v2 alone.
        std::fs::create_dir_all(baseline_dir()).expect("create l4-summary-baseline dir");
        std::fs::write(&path, digest.as_bytes())
            .unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
        eprintln!(
            "REGEN cdo-whole-program-digest ({} routines): {} -> {}",
            new.len(),
            digest,
            path.display()
        );
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read frozen CDO digest {} failed: {e} — run CDO_WS=<path> \
             REGEN_TEMP_GOLDENS=1 to (re)capture",
            path.display()
        )
    });
    assert_eq!(
        digest,
        strip_cr(&expected).trim(),
        "CDO whole-program v2 output ({} routines) diverged from the frozen digest",
        new.len()
    );
}

// ---------------------------------------------------------------------------
// Task 6 — the `ReverseEffectIndex` / `DbEffectQuery` differential.
// ---------------------------------------------------------------------------

/// An INDEPENDENTLY computed answer to every query the reverse index and its
/// facade ship, worked out the slow way: straight off [`SummaryBundle`], with
/// no postings, no effect classes, no bitmaps and no condensation.
///
/// ## Why this file, and why an oracle rather than a golden
///
/// `ReverseEffectIndex` shipped 7 self-consistency unit tests, all of which
/// hand-build a 2-3 routine bundle through `SummaryBundleBuilder`. **None ran
/// `compute_summaries_v2*`, the only real producer**, so nothing exercised the
/// dense `PostingList` branch, a class with more than one member, the real
/// `key_rank` / `ordered_storage_ord` ordering plumbing, fixed-leaf singleton
/// classes as `seed_fixed_leaf_rows` actually emits them, or any scale effect.
/// The module had never executed against real data at all.
///
/// A golden over the index's own dump would not have fixed that: it freezes
/// whatever the index currently does, bug included — which is exactly how
/// 7 comfortable self-consistency tests came to exist. So the oracle below is
/// deliberately naive and shares no code with the implementation:
///
/// - membership: iterate `db_effects(r)` and compare strings;
/// - the up-queries: filter every routine-with-a-row by that membership;
/// - ancestors: a whole-graph brute force (for each candidate A, BFS FORWARD
///   over `graph.edges_by_from` and see whether it reaches R). O(V·(V+E)) —
///   unusable in production, perfect as an oracle, and it never touches the
///   condensation, so it cannot share a bug with the implementation;
/// - ancestor DEPTH: a 0-1 BFS on the REVERSED ROUTINE graph counting SCC
///   boundary crossings, which is a different computation from the
///   implementation's plain BFS over a prebuilt condensation and must
///   nonetheless agree exactly.
///
/// Effect granularity needs no `effect_key`-injectivity assumption: a
/// `DbEffectRef` carries the whole `(op, table_id, operation_id, temp)` tuple,
/// which IS `EffectIdentity`, so the oracle maps it back through the frozen
/// universe's own `get()` — the interning map itself is the injection.
mod reverse_index_differential {
    use std::collections::{HashMap, HashSet, VecDeque};

    use al_sem::engine::l4::combined_graph::CombinedGraph;
    use al_sem::engine::l4::effect_query::DbEffectQuery;
    use al_sem::engine::l4::effect_store::SummaryBundle;
    use al_sem::engine::l4::effect_universe::{EffectId, EffectIdentity};
    use al_sem::engine::l4::reverse_index::{ReverseEffectIndex, class_of};
    use al_sem::engine::l4::routine_interner::RoutineIx;
    use al_sem::engine::l4::scc::SccResult;
    use al_sem::engine::l4::summary::RoutineSummary;
    use al_sem::engine::l4::summary_runner::compute_summaries_v2_bundle_with_leaves;

    // -----------------------------------------------------------------------
    // The oracle.
    // -----------------------------------------------------------------------

    /// Every answer, worked out the slow way and MEMOIZED once per workspace.
    ///
    /// Memoization is not a shortcut into the implementation's world: each set
    /// below is built by iterating `bundle.db_effects(r)` and reading strings —
    /// the same thing a caller with no index at all would do. Without it the
    /// exhaustive `up_effect` comparison would be
    /// `n_effects x n_routines x effects_per_routine` (~1e9 on a real
    /// workspace) and the differential would be untestable at the only scale
    /// that matters.
    pub struct Oracle<'a> {
        bundle: &'a SummaryBundle,
        graph: &'a CombinedGraph,
        /// Per routine-with-a-row: the exact `EffectId` set of its projected
        /// effects, resolved through the frozen universe's OWN identity map (so
        /// no `effect_key` injectivity is assumed — the interning map IS the
        /// injection).
        pub effect_ids: HashMap<RoutineIx, HashSet<EffectId>>,
        /// Per routine-with-a-row: the exact `table_id` string set.
        pub table_ids: HashMap<RoutineIx, HashSet<String>>,
        /// Reversed ROUTINE adjacency `to -> [(from, cost)]`, cost 1 when the
        /// edge crosses an SCC boundary and 0 when it does not. Deliberately
        /// over raw routine ids: the implementation walks a prebuilt
        /// CONDENSATION, so a 0-1 BFS here is a structurally different
        /// computation that must nonetheless agree.
        rev_routine: HashMap<&'a str, Vec<(&'a str, u32)>>,
        pub routines: Vec<RoutineIx>,
        pub tables: Vec<String>,
    }

    impl<'a> Oracle<'a> {
        pub fn build(bundle: &'a SummaryBundle, graph: &'a CombinedGraph, scc: &SccResult) -> Self {
            let universe = bundle.effects().universe();
            let mut routines: Vec<RoutineIx> = bundle.routines_with_rows().collect();
            routines.sort_unstable();

            let mut effect_ids: HashMap<RoutineIx, HashSet<EffectId>> = HashMap::new();
            let mut table_ids: HashMap<RoutineIx, HashSet<String>> = HashMap::new();
            for &r in &routines {
                let mut eids: HashSet<EffectId> = HashSet::new();
                let mut tids: HashSet<String> = HashSet::new();
                for e in bundle.db_effects(r) {
                    let identity = EffectIdentity {
                        op: e.op.to_string(),
                        table_id: e.table_id.to_string(),
                        operation_id: e.operation_id.to_string(),
                        temp: e.temp_state.clone(),
                    };
                    let eid = universe.get(&identity).unwrap_or_else(|| {
                        panic!(
                            "every projected effect must round-trip to its EffectId \
                             (op={:?} table={:?} opid={:?})",
                            e.op, e.table_id, e.operation_id
                        )
                    });
                    eids.insert(eid);
                    tids.insert(e.table_id.to_string());
                }
                effect_ids.insert(r, eids);
                table_ids.insert(r, tids);
            }

            let mut tables: Vec<String> = (0..universe.len() as u32)
                .map(|i| universe.identity(EffectId(i)).table_id.clone())
                .collect();
            tables.sort();
            tables.dedup();

            let mut rev_routine: HashMap<&'a str, Vec<(&'a str, u32)>> = HashMap::new();
            for (from, edges) in &graph.edges_by_from {
                for e in edges {
                    let cost = match (
                        scc.scc_id_by_routine.get(from),
                        scc.scc_id_by_routine.get(&e.to),
                    ) {
                        (Some(a), Some(b)) if a == b => 0,
                        _ => 1,
                    };
                    rev_routine
                        .entry(e.to.as_str())
                        .or_default()
                        .push((from.as_str(), cost));
                }
            }

            Oracle {
                bundle,
                graph,
                effect_ids,
                table_ids,
                rev_routine,
                routines,
                tables,
            }
        }

        pub fn touches_table(&self, r: RoutineIx, t: &str) -> bool {
            self.table_ids.get(&r).is_some_and(|s| s.contains(t))
        }

        pub fn touches_effect(&self, r: RoutineIx, e: EffectId) -> bool {
            self.effect_ids.get(&r).is_some_and(|s| s.contains(&e))
        }

        pub fn up_table(&self, t: &str) -> Vec<RoutineIx> {
            self.routines
                .iter()
                .copied()
                .filter(|&r| self.touches_table(r, t))
                .collect()
        }

        pub fn up_effect(&self, e: EffectId) -> Vec<RoutineIx> {
            self.routines
                .iter()
                .copied()
                .filter(|&r| self.touches_effect(r, e))
                .collect()
        }

        /// Ancestor DEPTH by 0-1 BFS on the reversed ROUTINE graph: the minimum
        /// number of SCC boundaries any path `A -> ... -> r` crosses. That
        /// equals BFS depth in the condensation, so it must match the
        /// implementation exactly — without this oracle ever building one.
        /// `r` itself is excluded.
        pub fn ancestor_depths(&self, r: RoutineIx) -> HashMap<RoutineIx, u32> {
            let start = self.bundle.routine_id(r);
            let mut dist: HashMap<&str, u32> = HashMap::new();
            let mut dq: VecDeque<&str> = VecDeque::new();
            dist.insert(start, 0);
            dq.push_back(start);
            let empty: Vec<(&str, u32)> = Vec::new();
            while let Some(cur) = dq.pop_front() {
                let d = dist[cur];
                for &(pred, cost) in self.rev_routine.get(cur).unwrap_or(&empty) {
                    let nd = d + cost;
                    if dist.get(pred).is_none_or(|&old| nd < old) {
                        dist.insert(pred, nd);
                        if cost == 0 {
                            dq.push_front(pred);
                        } else {
                            dq.push_back(pred);
                        }
                    }
                }
            }
            dist.into_iter()
                .filter_map(|(id, d)| self.bundle.routine_ix(id).map(|ix| (ix, d)))
                .filter(|&(ix, _)| ix != r)
                .collect()
        }

        /// The ancestor answer used everywhere: transitive callers of `r`
        /// (excluding `r`) that have a row and touch `t`, ascending.
        pub fn ancestors_touching(&self, r: RoutineIx, t: &str) -> Vec<RoutineIx> {
            let mut v: Vec<RoutineIx> = self
                .ancestor_depths(r)
                .into_keys()
                .filter(|&a| self.table_ids.contains_key(&a) && self.touches_table(a, t))
                .collect();
            v.sort_unstable();
            v
        }

        /// The MAXIMALLY naive ancestor answer: for each candidate, a fresh
        /// forward BFS over `graph.edges_by_from` asking whether it reaches `r`.
        /// O(V·(V+E)) — unusable past fixture scale, which is why
        /// [`Self::ancestors_touching`] exists; the fixture test asserts the two
        /// agree, so the cheaper one is itself under oracle.
        pub fn ancestors_touching_bruteforce(&self, r: RoutineIx, t: &str) -> Vec<RoutineIx> {
            let target = self.bundle.routine_id(r).to_string();
            let mut v: Vec<RoutineIx> = self
                .routines
                .iter()
                .copied()
                .filter(|&a| a != r)
                .filter(|&a| self.touches_table(a, t))
                .filter(|&a| reaches(self.graph, self.bundle.routine_id(a), &target))
                .collect();
            v.sort_unstable();
            v
        }
    }

    /// Brute force: is there a directed path of length >= 1 from `from` to `to`?
    fn reaches(graph: &CombinedGraph, from: &str, to: &str) -> bool {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut q: VecDeque<&str> = VecDeque::new();
        q.push_back(from);
        let mut first = true;
        while let Some(cur) = q.pop_front() {
            if !first && cur == to {
                return true;
            }
            first = false;
            if let Some(edges) = graph.edges_by_from.get(cur) {
                for e in edges {
                    if e.to == to {
                        return true;
                    }
                    if seen.insert(e.to.as_str()) {
                        q.push_back(e.to.as_str());
                    }
                }
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // The exhaustive comparison.
    // -----------------------------------------------------------------------

    fn is_strictly_ascending(v: &[RoutineIx]) -> bool {
        v.windows(2).all(|w| w[0] < w[1])
    }

    /// Every assertion of scope §4.2 level 1, run exhaustively over one bundle.
    /// Shared verbatim by the fixture cases and the real-workspace case, so the
    /// two can never drift apart.
    pub fn assert_index_matches_oracle(
        label: &str,
        bundle: &SummaryBundle,
        graph: &CombinedGraph,
        scc: &SccResult,
        oracle: &Oracle<'_>,
    ) {
        let index = ReverseEffectIndex::build(bundle);
        let query = DbEffectQuery::build(bundle, scc, graph);
        let universe_len = bundle.effects().universe().len() as u32;

        // 1. up_table, exhaustively + an id that is definitely not a table.
        for t in &oracle.tables {
            let got = index.up_table(t);
            assert_eq!(
                got,
                oracle.up_table(t),
                "[{label}] up_table({t:?}) diverged from the oracle"
            );
            assert!(
                is_strictly_ascending(&got),
                "[{label}] up_table({t:?}) must be strictly ascending"
            );
            assert_eq!(
                query.routines_touching(t),
                got,
                "[{label}] the facade must delegate routines_touching verbatim"
            );
        }
        assert!(
            index.up_table("::absolutely-not-a-table::").is_empty(),
            "[{label}] an absent table id answers empty, never panics"
        );

        // 2. touches_table over every (routine-with-row x table), and the
        //    facade's boolean/witness-list agreement.
        for &r in &oracle.routines {
            for t in &oracle.tables {
                let got = index.touches_table(bundle, r, t);
                assert_eq!(
                    got,
                    oracle.touches_table(r, t),
                    "[{label}] touches_table({}, {t:?}) diverged",
                    bundle.routine_id(r)
                );
                assert_eq!(
                    query.touches_table(r, t),
                    got,
                    "[{label}] facade touches_table must match the index"
                );
                assert_eq!(
                    !query.touches(r, t).is_empty(),
                    got,
                    "[{label}] touches()/touches_table() disagreement for {t:?}"
                );
            }
            assert!(
                !index.touches_table(bundle, r, "::absolutely-not-a-table::"),
                "[{label}] absent table is never touched"
            );
        }

        // 3. up_effect / touches_effect over every EffectId in the universe.
        for i in 0..universe_len {
            let eid = EffectId(i);
            let got = index.up_effect(eid);
            assert_eq!(
                got,
                oracle.up_effect(eid),
                "[{label}] up_effect({i}) diverged from the oracle"
            );
            assert!(
                is_strictly_ascending(&got),
                "[{label}] up_effect({i}) must be strictly ascending"
            );
        }
        for &r in &oracle.routines {
            for i in 0..universe_len {
                let eid = EffectId(i);
                assert_eq!(
                    index.touches_effect(bundle, r, eid),
                    oracle.touches_effect(r, eid),
                    "[{label}] touches_effect({}, {i}) diverged",
                    bundle.routine_id(r)
                );
            }
        }

        // 4. down(r) is byte-identical to db_effects(r), ORDER included, and the
        //    facade's unfiltered list agrees row for row.
        for &r in &oracle.routines {
            let via_down: Vec<_> = index.down(bundle, r).map(|e| e.to_owned()).collect();
            let via_bundle: Vec<_> = bundle.db_effects(r).map(|e| e.to_owned()).collect();
            assert_eq!(
                via_down,
                via_bundle,
                "[{label}] down({}) must delegate verbatim",
                bundle.routine_id(r)
            );
            let facade = query.all_effects(r);
            assert_eq!(facade.len(), via_bundle.len());
            for (a, b) in facade.iter().zip(via_bundle.iter()) {
                assert_eq!(a.op, b.op);
                assert_eq!(a.table_id, b.table_id);
                assert_eq!(a.operation_id, b.operation_id);
                assert_eq!(a.via.as_str(), b.via);
            }
        }

        // 5. The ⟨rev3⟩ disjointness invariant over the WHOLE index — not the
        //    4 hand-picked pairs the unit test checks — plus the stronger
        //    statement that the two arms UNION to exactly the oracle's answer.
        for &r in &oracle.routines {
            let Some(c) = class_of(bundle, r) else {
                continue;
            };
            for t in &oracle.tables {
                let base = index.table_touches_via_base(t, c);
                let delta = index.table_touches_via_delta_routine(t, r);
                if delta {
                    assert!(
                        !base,
                        "[{label}] {t:?}: routine {} is in BOTH the delta posting and its \
                         class's base posting — the disjoint contract is broken",
                        bundle.routine_id(r)
                    );
                }
                assert_eq!(
                    base || delta,
                    oracle.touches_table(r, t),
                    "[{label}] base|delta arms must reconstruct the oracle answer"
                );
            }
        }

        // 6. class_members: ascending, self-inclusive, and internally coherent.
        for &r in &oracle.routines {
            let Some(c) = class_of(bundle, r) else {
                continue;
            };
            let members = index.class_members(c);
            assert!(
                members.windows(2).all(|w| w[0] < w[1]),
                "[{label}] class_members must be strictly ascending"
            );
            assert!(
                members.contains(&r),
                "[{label}] a routine must be a member of its own class"
            );
            for &m in members {
                assert_eq!(
                    class_of(bundle, m),
                    Some(c),
                    "[{label}] every class member must report that class"
                );
            }
        }
    }

    /// The ancestor comparison. Split out because the real-workspace case runs
    /// it over a stated deterministic SAMPLE while fixtures run it exhaustively.
    pub fn assert_ancestors_match_oracle(
        label: &str,
        bundle: &SummaryBundle,
        graph: &CombinedGraph,
        scc: &SccResult,
        oracle: &Oracle<'_>,
        routines: &[RoutineIx],
        tables: &[String],
    ) {
        let query = DbEffectQuery::build(bundle, scc, graph);
        for &r in routines {
            let depths = oracle.ancestor_depths(r);

            // The ancestor set and its depth, independent of any table.
            let ancestors = query.ancestors(r);
            for &(depth, ix) in &ancestors {
                assert_eq!(
                    depths.get(&ix).copied(),
                    Some(depth),
                    "[{label}] ancestor depth for {} -> {} diverged from the 0-1-BFS oracle",
                    bundle.routine_id(ix),
                    bundle.routine_id(r)
                );
            }
            assert!(
                !ancestors.iter().any(|&(_, ix)| ix == r),
                "[{label}] a routine must never be its own ancestor"
            );
            // `ancestors` returns `(depth, routine)`, so the tuple's own `Ord`
            // IS the documented nearest-first order — no re-derived sort key,
            // and no way to accidentally check the wrong field.
            assert!(
                ancestors.windows(2).all(|w| w[0] < w[1]),
                "[{label}] ancestors must be strictly ascending by (depth, routine)"
            );

            for t in tables {
                let ups = query.ancestors_touching(r, t);
                let mut got: Vec<RoutineIx> = ups.iter().map(|a| a.touch.routine).collect();
                got.sort_unstable();
                got.dedup();
                assert_eq!(
                    got,
                    oracle.ancestors_touching(r, t),
                    "[{label}] ancestors_touching({}, {t:?}) diverged from the oracle",
                    bundle.routine_id(r)
                );
                assert!(
                    ups.windows(2).all(|w| w[0].depth <= w[1].depth),
                    "[{label}] ancestors_touching must be nearest-first"
                );
                for a in &ups {
                    assert_eq!(a.touch.table_id, t);
                    assert!(query.touches_table(a.touch.routine, t));
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Level 1: the fixture cases, exhaustive.
    // -----------------------------------------------------------------------

    /// Every fixture `fixtures::build` knows how to make — including
    /// `fixed_leaf_in_scc` (a fixed leaf inside a cycle, where the effect-class
    /// DAG and the Tarjan condensation genuinely diverge),
    /// `missing_routine_in_scc` (a graph node with no row) and the whole
    /// via-collision family.
    const FIXTURES: &[&str] = &[
        "linear_known",
        "recursive_self_loop",
        "recursive_pair_pd",
        "pd_to_known",
        "pd_to_unknown",
        "multi_callsite_same_callee",
        "via_collision",
        "via_collision_edges_reversed",
        "pd_substituted_via_collision",
        "pd_substituted_via_collision_reversed",
        "direct_terminal_beats_colliding_pd_substitution",
        "direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition",
        "external_successor_pd",
        "fixed_leaf_in_scc",
        "multi_effect_fixed_leaf_in_scc",
        "missing_routine_in_scc",
    ];

    fn leaves_for(name: &str) -> HashMap<String, RoutineSummary> {
        match name {
            "fixed_leaf_in_scc" => super::fixtures::fixed_leaf_in_scc_leaves(),
            "multi_effect_fixed_leaf_in_scc" => {
                super::fixtures::multi_effect_fixed_leaf_in_scc_leaves()
            }
            _ => HashMap::new(),
        }
    }

    #[test]
    fn every_fixture_index_matches_the_slow_oracle_exhaustively() {
        let mut with_effects = 0usize;
        for name in FIXTURES {
            let (routines, graph, scc, fields, ub) = super::fixtures::build(name);
            let leaves = leaves_for(name);
            // The BUNDLE entry point, not the materializing `_core` shim — the
            // compact rows are what the index transposes.
            let (bundle, _map, _diags) = compute_summaries_v2_bundle_with_leaves(
                &routines, &graph, &scc, &ub, &fields, &leaves,
            );
            let oracle = Oracle::build(&bundle, &graph, &scc);

            assert_index_matches_oracle(name, &bundle, &graph, &scc, &oracle);
            let rs = oracle.routines.clone();
            let ts = oracle.tables.clone();
            assert_ancestors_match_oracle(name, &bundle, &graph, &scc, &oracle, &rs, &ts);

            // The cheap ancestor oracle is itself under oracle: at fixture scale
            // the maximally-naive per-candidate forward BFS must agree with the
            // 0-1-BFS derivation used at real-workspace scale.
            for &r in &rs {
                for t in &ts {
                    assert_eq!(
                        oracle.ancestors_touching(r, t),
                        oracle.ancestors_touching_bruteforce(r, t),
                        "[{name}] the two oracles disagree for ({}, {t:?}) — the cheaper \
                         one is not a faithful stand-in",
                        bundle.routine_id(r)
                    );
                }
            }

            if !ts.is_empty() {
                with_effects += 1;
            }
        }
        // Precondition, hand-stated: nearly every fixture must actually carry db
        // effects, or the comparison above ran over empty universes and proved
        // nothing. (`missing_routine_in_scc` legitimately has none.)
        assert!(
            with_effects >= FIXTURES.len() - 1,
            "expected all but the effect-free fixture to carry db effects, got \
             {with_effects} of {}",
            FIXTURES.len()
        );
    }

    /// The scope's §2.4 notion-discipline case, driven through the REAL solver
    /// rather than a hand-built bundle: `fixed_leaf_in_scc` is a 3-member cycle
    /// `a -> b -> c -> a` whose `c` arrives as a pre-settled fixed leaf, so
    /// `effective_sccs` splits the cycle in the EFFECT-class DAG while Tarjan
    /// still sees one component. An ancestor walk computed on the effect DAG
    /// would therefore return a strictly SMALLER set than the truth.
    #[test]
    fn fixed_leaf_cycle_ancestors_match_the_brute_force_oracle() {
        let name = "fixed_leaf_in_scc";
        let (routines, graph, scc, fields, ub) = super::fixtures::build(name);
        let leaves = super::fixtures::fixed_leaf_in_scc_leaves();
        let (bundle, _map, _diags) =
            compute_summaries_v2_bundle_with_leaves(&routines, &graph, &scc, &ub, &fields, &leaves);

        // Premise: Tarjan really did see ONE cycle here.
        assert_eq!(scc.sccs.len(), 1, "the fixture is a single 3-member cycle");

        let oracle = Oracle::build(&bundle, &graph, &scc);
        let query = DbEffectQuery::build(&bundle, &scc, &graph);
        let rs = oracle.routines.clone();
        assert!(
            rs.len() >= 2,
            "precondition: at least two cycle members must have rows"
        );

        // Every member is every other member's ancestor at depth 0 — the answer
        // an effect-class-DAG walk could not give.
        for &r in &rs {
            let ancestors: Vec<RoutineIx> =
                query.ancestors(r).into_iter().map(|(_, ix)| ix).collect();
            for &other in &rs {
                if other == r {
                    continue;
                }
                assert!(
                    ancestors.contains(&other),
                    "{} must be an ancestor of {} — they are cycle-mates in the TARJAN \
                     condensation, which is the notion this walk uses",
                    bundle.routine_id(other),
                    bundle.routine_id(r)
                );
            }
            assert!(
                query.ancestors(r).iter().all(|&(d, _)| d == 0),
                "all cycle-mates sit at depth 0"
            );
        }

        assert!(
            oracle.tables.contains(&"t3".to_string()),
            "precondition: the fixed leaf's own table must be in the universe"
        );
        let ts = oracle.tables.clone();
        assert_ancestors_match_oracle(name, &bundle, &graph, &scc, &oracle, &rs, &ts);
    }
}

/// The real-workspace differential (scope §4.2 level 2) — the one that closes
/// "this module has never executed against real data".
///
/// Reuses `cdo_whole_program_v2_matches_frozen_digest`'s assembly verbatim,
/// swapping `compute_summaries_v2_with_leaves_core` for
/// `compute_summaries_v2_bundle_with_leaves` so the compact bundle survives, and
/// then runs the SAME oracle comparison the fixtures use — so the two can never
/// drift apart.
///
/// Coverage, stated rather than hidden:
///
/// - **Exhaustive** — `up_table` / `up_effect` / `touches_table` /
///   `touches_effect` / `down` / the ⟨rev3⟩ disjointness invariant / ascending
///   order, over EVERY table, EVERY `EffectId` and EVERY routine-with-a-row.
/// - **Sampled, deterministically** — `ancestors_touching`, whose oracle is
///   O(V·(V+E)): every 50th routine-with-a-row (ascending `RoutineIx`, so the
///   sample is reproducible), against its own touched tables PLUS two tables it
///   does NOT touch. The negatives are the interesting half — they are where
///   "a caller reaches it through a sibling branch" actually shows up.
///
/// **Then, only once the exhaustive comparison above has already asserted
/// agreement in THIS SAME RUN, a frozen digest** (scope §4.2 level 2's third
/// bullet, `tests/l4-summary-baseline/cdo-reverse-index-digest.txt`) — the
/// canonical serialization the spec names verbatim: for each table id in
/// sorted order, the sorted `bundle.routine_id(ix)` list `up_table` returns.
/// The oracle proves the index correct TODAY; the digest proves it stays
/// UNCHANGED tomorrow. Sequencing the digest after the oracle assertions
/// inside one function (rather than as a separate test, which Rust may run in
/// any order/in parallel) is what makes "capture mismatch-guarded" — a digest
/// can never be minted from a state the differential has not already
/// vetted — an actual guarantee rather than a convention, mirroring
/// `tests/l4-summary-baseline/README.md`'s `baseline == old == v2`-at-capture
/// discipline as a same-run check instead of a historical one (the oracle
/// here, unlike the now-deleted old Jacobi solver, is still live to ask).
///
/// Skips when `CDO_WS` is unset; panics under `ENFORCE_CDO_WS=1`, so
/// `scripts/cdo-gate` runs it for real. Regenerate the digest with:
/// ```text
/// CDO_WS=<path> ENFORCE_CDO_WS=1 REGEN_TEMP_GOLDENS=1 cargo test -p al-call-hierarchy \
///   --test l4_summary_differential cdo_reverse_index_matches_slow_oracle -- --nocapture
/// ```
#[test]
fn cdo_reverse_index_matches_slow_oracle() {
    use std::collections::BTreeMap;

    use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
    use al_sem::engine::l3::event_graph::build_event_graph;
    use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
    use al_sem::engine::l3::symbol_table::SymbolTable;
    use al_sem::engine::l4::combined_graph::build_combined_graph;
    use al_sem::engine::l4::reverse_index::ReverseEffectIndex;
    use al_sem::engine::l4::routine_interner::RoutineIx;
    use al_sem::engine::l4::scc::{SccInputGraph, tarjan_scc};
    use al_sem::engine::l4::summary_runner::compute_summaries_v2_bundle_with_leaves;
    use reverse_index_differential as diff;

    let Some(ws_path) = cdo_ws_or_enforce() else {
        return;
    };

    let resolved = assemble_and_resolve_workspace_default(&ws_path)
        .expect("assemble_and_resolve_workspace_default must succeed on CDO_WS");
    let ws = &resolved.workspace;

    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let no_deps: Vec<DeclaredDependency> = Vec::new();
    let no_fetched: Vec<String> = Vec::new();
    let calls = resolve_calls(ws, &symbols, &no_deps, &no_fetched);
    let event_graph = build_event_graph(&ws.routines, &symbols);
    let graph = build_combined_graph(ws, &calls, &event_graph);

    let mut scc_adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        scc_adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &scc_adjacency,
    });

    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }
    let leaf_summaries: HashMap<String, RoutineSummary> = HashMap::new();
    let (bundle, _map, _diags) = compute_summaries_v2_bundle_with_leaves(
        &ws.routines,
        &graph,
        &scc,
        &calls.upgraded_bindings,
        &field_index,
        &leaf_summaries,
    );

    let oracle = diff::Oracle::build(&bundle, &graph, &scc);
    let routines = oracle.routines.clone();
    let tables = oracle.tables.clone();

    // PRECONDITIONS, hand-stated: a real workspace must produce a real
    // population, or every assertion below passes vacuously. These numbers are
    // deliberately loose floors, not pinned values — this is a differential,
    // not a ratchet.
    assert!(
        routines.len() > 100,
        "a real BC workspace must yield >100 routines with db-effect rows, got {}",
        routines.len()
    );
    assert!(
        tables.len() > 5,
        "a real BC workspace must yield >5 distinct effect tables, got {}",
        tables.len()
    );
    eprintln!(
        "cdo_reverse_index_matches_slow_oracle: {} routines-with-rows, {} tables, \
         {} effects in the frozen universe",
        routines.len(),
        tables.len(),
        bundle.effects().universe().len()
    );

    // Exhaustive.
    diff::assert_index_matches_oracle("CDO_WS", &bundle, &graph, &scc, &oracle);

    // Sampled — the rule is stated in this test's doc, and printed, not hidden.
    let sample: Vec<RoutineIx> = routines.iter().copied().step_by(50).collect();
    assert!(
        !sample.is_empty(),
        "the every-50th sample must not be empty"
    );
    for &r in &sample {
        let touched: Vec<String> = {
            let mut t: Vec<String> = bundle
                .db_effects(r)
                .map(|e| e.table_id.to_string())
                .collect();
            t.sort();
            t.dedup();
            t
        };
        // Two tables this routine does NOT touch — the informative half.
        let untouched: Vec<String> = tables
            .iter()
            .filter(|t| !touched.contains(t))
            .take(2)
            .cloned()
            .collect();
        let mut probe = touched;
        probe.extend(untouched);
        diff::assert_ancestors_match_oracle("CDO_WS", &bundle, &graph, &scc, &oracle, &[r], &probe);
    }
    eprintln!(
        "cdo_reverse_index_matches_slow_oracle: ancestor oracle ran over {} sampled \
         routines (every 50th)",
        sample.len()
    );

    // ---- Frozen digest (scope §4.2 level 2, third bullet) --------------------
    //
    // Reached only after every assertion above has already passed — see this
    // fn's doc comment for why that sequencing, not a separate test, is what
    // makes the mismatch guard real. `tables` is `oracle.tables` (sorted,
    // deduped) captured above; `index` is rebuilt here (cheap relative to the
    // CDO workspace assembly already paid for) so this section reads
    // standalone.
    let index = ReverseEffectIndex::build(&bundle);
    let mut canonical: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in &tables {
        let ids: Vec<String> = index
            .up_table(t)
            .into_iter()
            .map(|ix| bundle.routine_id(ix).to_string())
            .collect();
        canonical.insert(t.clone(), ids);
    }
    let digest = sha256_hex(&format!("{canonical:#?}"));
    let digest_path = baseline_dir().join("cdo-reverse-index-digest.txt");

    if regen() {
        std::fs::create_dir_all(baseline_dir()).expect("create l4-summary-baseline dir");
        std::fs::write(&digest_path, digest.as_bytes())
            .unwrap_or_else(|e| panic!("regen write {}: {e}", digest_path.display()));
        eprintln!(
            "REGEN cdo-reverse-index-digest ({} tables): {} -> {}",
            tables.len(),
            digest,
            digest_path.display()
        );
        return;
    }

    let expected = std::fs::read_to_string(&digest_path).unwrap_or_else(|e| {
        panic!(
            "read frozen CDO reverse-index digest {} failed: {e} — run CDO_WS=<path> \
             ENFORCE_CDO_WS=1 REGEN_TEMP_GOLDENS=1 to (re)capture",
            digest_path.display()
        )
    });
    assert_eq!(
        digest,
        strip_cr(&expected).trim(),
        "CDO reverse index ({} tables) diverged from the frozen digest",
        tables.len()
    );
}

/// Fixture builders for the differential harness above. Each named fixture
/// builds real `L3Routine` / `CombinedGraph` / `SccResult` / `FieldIndex` /
/// `upgraded_bindings` inputs the OLD solver can process without panicking; the
/// differential itself is the oracle, so these are structurally-valid inputs
/// exercising a specific solver code path rather than hand-verified outputs.
///
/// NOTE: `src/engine/l5/test_support.rs` has similar `L3Routine`/`CombinedGraph`
/// constructors, but that module is `#![cfg(test)]`-gated (unit-test-only) so it
/// is not linkable from this integration-test binary — every builder below is a
/// local, from-scratch copy of that same pattern (mirroring how
/// `tests/temp_state/temp_state_path.rs` and siblings already do this).
mod fixtures {
    use std::collections::HashMap;

    use al_sem::engine::l2::features::{
        PAnchor, PCallArgumentBinding, PCallSite, PCallee, PTempState,
    };
    use al_sem::engine::l3::call_resolver::UpgradedBinding;
    use al_sem::engine::l3::l3_workspace::{L3RecordOperation, L3Routine, RoutineVariables};
    use al_sem::engine::l4::combined_graph::{CombinedEdge, CombinedGraph};
    use al_sem::engine::l4::effect_lattice::{TempStateKind, effect_key_of};
    use al_sem::engine::l4::scc::{Scc, SccResult};
    use al_sem::engine::l4::summary::{DbEffect, RoutineSummary, TempState};
    use al_sem::engine::l4::summary_runner::FieldIndex;

    type FixtureOut = (
        Vec<L3Routine>,
        CombinedGraph,
        SccResult,
        FieldIndex,
        HashMap<String, Vec<UpgradedBinding>>,
    );

    /// Build the named fixture: `(routines, graph, scc, fields, upgraded_bindings)`.
    pub fn build(name: &str) -> FixtureOut {
        match name {
            "linear_known" => linear_known(),
            "recursive_self_loop" => recursive_self_loop(),
            "recursive_pair_pd" => recursive_pair_pd(),
            "pd_to_known" => pd_to_known(),
            "pd_to_unknown" => pd_to_unknown(),
            "multi_callsite_same_callee" => multi_callsite_same_callee(),
            "via_collision" => via_collision(),
            "via_collision_edges_reversed" => via_collision_edges_reversed(),
            "pd_substituted_via_collision" => pd_substituted_via_collision(false),
            "pd_substituted_via_collision_reversed" => pd_substituted_via_collision(true),
            "direct_terminal_beats_colliding_pd_substitution" => {
                direct_terminal_beats_colliding_pd_substitution()
            }
            "direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition" => {
                direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition()
            }
            "external_successor_pd" => external_successor_pd(),
            "fixed_leaf_in_scc" => fixed_leaf_in_scc(),
            "multi_effect_fixed_leaf_in_scc" => multi_effect_fixed_leaf_in_scc(),
            "missing_routine_in_scc" => missing_routine_in_scc(),
            other => panic!("fixtures::build: unknown fixture {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Shared low-level builders.
    // -----------------------------------------------------------------------

    fn anchor() -> PAnchor {
        PAnchor {
            source_unit_id: "ws:test.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: "test".to_string(),
        }
    }

    /// A bare, body-available `L3Routine` with just an id; callers push
    /// `record_operations`/`call_sites` onto it.
    fn routine(id: &str) -> L3Routine {
        L3Routine {
            id: id.to_string(),
            stable_routine_id: format!("stable::{id}"),
            object_id: "app/Codeunit/1".to_string(),
            object_type: "Codeunit".to_string(),
            name: id.to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: Vec::new(),
            app_guid: "app".to_string(),
            object_number: 1,
            normalized_signature_hash: String::new(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            source_anchor: anchor(),
            identifier_references: Vec::new(),
            unreachable_statements: Vec::new(),
            has_branching: false,
            var_assignments: Vec::new(),
            condition_references: Vec::new(),
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    fn ts_known(value: bool) -> PTempState {
        PTempState {
            kind: "known".to_string(),
            value: Some(value),
            parameter_index: None,
        }
    }

    fn ts_pd(idx: u32) -> PTempState {
        PTempState {
            kind: "parameter-dependent".to_string(),
            value: None,
            parameter_index: Some(idx),
        }
    }

    /// One db-touching record operation (`record_variable_name` is always
    /// `"Rec"` — no fixture below needs `RecordRoleSummary` field precision).
    fn record_op(
        id: &str,
        op: &str,
        table_id: &str,
        temp_state: Option<PTempState>,
    ) -> L3RecordOperation {
        L3RecordOperation {
            id: id.to_string(),
            op: op.to_string(),
            record_variable_name: "Rec".to_string(),
            record_variable_id: None,
            table_id: Some(table_id.to_string()),
            temp_state,
            field_arguments: None,
            source_anchor: anchor(),
            loop_stack: Vec::new(),
            field_argument_infos: None,
            in_until_condition: false,
            run_trigger: None,
        }
    }

    /// One argument binding of callee param `parameter_index` to
    /// `source_temp_state` (the shape `substitute_pd_temp_state` reads).
    fn arg_binding(
        parameter_index: u32,
        source_temp_state: Option<PTempState>,
    ) -> PCallArgumentBinding {
        PCallArgumentBinding {
            parameter_index,
            source_kind: "variable".to_string(),
            source_variable_name: Some("arg".to_string()),
            source_record_variable_id: None,
            source_parameter_index: None,
            caller_source_parameter_is_var: None,
            source_temp_state,
            argument_anchor: anchor(),
        }
    }

    /// A bare call site `id` calling `callee_name`, with the given argument
    /// bindings.
    fn call_site(
        id: &str,
        callee_name: &str,
        argument_bindings: Vec<PCallArgumentBinding>,
    ) -> PCallSite {
        PCallSite {
            id: id.to_string(),
            operation_id: format!("{id}/op"),
            callee_text: callee_name.to_string(),
            callee: PCallee::Bare {
                name: callee_name.to_string(),
            },
            argument_texts: Vec::new(),
            argument_infos: Vec::new(),
            argument_bindings,
            loop_stack: Vec::new(),
            source_anchor: anchor(),
            result_consumed: None,
            object_run_return_used: None,
            under_asserterror: None,
            control_context: None,
            order: None,
            in_statement_position: false,
            receiver_temp_state: None,
        }
    }

    /// A resolved combined edge `from -> to` with the given `kind` + optional
    /// callsite id (event-dispatch edges carry none).
    fn edge(from: &str, to: &str, kind: &str, callsite_id: Option<&str>) -> CombinedEdge {
        CombinedEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.to_string(),
            callsite_id: callsite_id.map(|s| s.to_string()),
            operation_id: None,
            event_id: None,
            subscriber_app_id: None,
            resolution: "resolved".to_string(),
        }
    }

    /// A `CombinedGraph` from a node list + flat edge list, grouped by `from`
    /// (mirrors `src/engine/l5/test_support.rs::graph_from_edges` — that helper
    /// is `#[cfg(test)]`-gated so it is unavailable to this integration test).
    fn graph(nodes: &[&str], edges: Vec<CombinedEdge>) -> CombinedGraph {
        let mut node_vec: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
        node_vec.sort();
        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut edges_from_order: Vec<String> = Vec::new();
        for e in edges {
            if !edges_by_from.contains_key(&e.from) {
                edges_from_order.push(e.from.clone());
            }
            edges_by_from.entry(e.from.clone()).or_default().push(e);
        }
        CombinedGraph {
            nodes: node_vec,
            edges_by_from,
            edges_from_order,
            uncertainty_edges: Vec::new(),
            typed_edges: Vec::new(),
        }
    }

    /// An `SccResult` from an ordered list of `(members, recursive)` — the order
    /// IS the reverse-topological surface (callees before callers).
    fn scc(entries: Vec<(Vec<&str>, bool)>) -> SccResult {
        let mut sccs = Vec::new();
        let mut scc_id_by_routine = HashMap::new();
        for (i, (members, recursive)) in entries.into_iter().enumerate() {
            let members: Vec<String> = members.into_iter().map(|m| m.to_string()).collect();
            for m in &members {
                scc_id_by_routine.insert(m.clone(), i);
            }
            sccs.push(Scc { members, recursive });
        }
        SccResult {
            sccs,
            scc_id_by_routine,
        }
    }

    // -----------------------------------------------------------------------
    // Fixtures.
    // -----------------------------------------------------------------------

    /// `b` calls `a`; `a` has one `Insert` op on table `t1` (`Known(true)` temp).
    /// Two singleton non-recursive SCCs in reverse-topo order `[a, b]`.
    fn linear_known() -> FixtureOut {
        let mut a = routine("a");
        a.record_operations
            .push(record_op("a_op1", "Insert", "t1", Some(ts_known(true))));

        let mut b = routine("b");
        b.call_sites.push(call_site("b_cs1", "A", Vec::new()));

        let g = graph(&["a", "b"], vec![edge("b", "a", "direct", Some("b_cs1"))]);
        let s = scc(vec![(vec!["a"], false), (vec!["b"], false)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `a` calls itself; one `ParameterDependent(0)` op whose self-call binding
    /// is `Known(true)` (a temp-record arg) — a single-member recursive SCC.
    fn recursive_self_loop() -> FixtureOut {
        let mut a = routine("a");
        a.record_operations
            .push(record_op("a_op1", "Insert", "t1", Some(ts_pd(0))));
        a.call_sites.push(call_site(
            "a_cs1",
            "A",
            vec![arg_binding(0, Some(ts_known(true)))],
        ));

        let g = graph(&["a"], vec![edge("a", "a", "direct", Some("a_cs1"))]);
        let s = scc(vec![(vec!["a"], true)]);

        (vec![a], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `a` <-> `b` mutual recursion; `b` has a `ParameterDependent(0)` op that
    /// `a`'s callsite forwards as its OWN `ParameterDependent(0)` binding — the
    /// re-symbolize-upward substitution path (RV-7).
    fn recursive_pair_pd() -> FixtureOut {
        let mut a = routine("a");
        a.call_sites.push(call_site(
            "a_cs1",
            "B",
            vec![arg_binding(0, Some(ts_pd(0)))],
        ));

        let mut b = routine("b");
        b.record_operations
            .push(record_op("b_op1", "Insert", "t2", Some(ts_pd(0))));
        b.call_sites.push(call_site("b_cs1", "A", Vec::new()));

        let g = graph(
            &["a", "b"],
            vec![
                edge("a", "b", "direct", Some("a_cs1")),
                edge("b", "a", "direct", Some("b_cs1")),
            ],
        );
        let s = scc(vec![(vec!["a", "b"], true)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `caller` passes a `temporary`-keyword record into `callee`, whose one op
    /// is `ParameterDependent(0)` — substitutes to `Known(true)`.
    fn pd_to_known() -> FixtureOut {
        let mut callee = routine("callee");
        callee
            .record_operations
            .push(record_op("callee_op1", "Insert", "t1", Some(ts_pd(0))));

        let mut caller = routine("caller");
        caller.call_sites.push(call_site(
            "caller_cs1",
            "Callee",
            vec![arg_binding(0, Some(ts_known(true)))],
        ));

        let g = graph(
            &["callee", "caller"],
            vec![edge("caller", "callee", "direct", Some("caller_cs1"))],
        );
        let s = scc(vec![(vec!["callee"], false), (vec!["caller"], false)]);

        (
            vec![callee, caller],
            g,
            s,
            FieldIndex::new(),
            HashMap::new(),
        )
    }

    /// Same shape as `pd_to_known`, but the binding carries no captured source
    /// temp state (e.g. an unresolved/non-record argument) — substitutes to
    /// `Unknown`.
    fn pd_to_unknown() -> FixtureOut {
        let mut callee = routine("callee");
        callee
            .record_operations
            .push(record_op("callee_op1", "Insert", "t1", Some(ts_pd(0))));

        let mut caller = routine("caller");
        caller.call_sites.push(call_site(
            "caller_cs1",
            "Callee",
            vec![arg_binding(0, None)],
        ));

        let g = graph(
            &["callee", "caller"],
            vec![edge("caller", "callee", "direct", Some("caller_cs1"))],
        );
        let s = scc(vec![(vec!["callee"], false), (vec!["caller"], false)]);

        (
            vec![callee, caller],
            g,
            s,
            FieldIndex::new(),
            HashMap::new(),
        )
    }

    /// `a` calls `b` twice via two DIFFERENT call sites with different bindings
    /// for the same `ParameterDependent(0)` op — divergent substitutions stay
    /// two DISTINCT folded effects (different re-keyed `effect_key`s), never
    /// merged.
    fn multi_callsite_same_callee() -> FixtureOut {
        let mut b = routine("b");
        b.record_operations
            .push(record_op("b_op1", "Insert", "t1", Some(ts_pd(0))));

        let mut a = routine("a");
        a.call_sites.push(call_site(
            "a_cs1",
            "B",
            vec![arg_binding(0, Some(ts_known(true)))],
        ));
        a.call_sites.push(call_site(
            "a_cs2",
            "B",
            vec![arg_binding(0, Some(ts_known(false)))],
        ));

        let g = graph(
            &["a", "b"],
            vec![
                edge("a", "b", "direct", Some("a_cs1")),
                edge("a", "b", "direct", Some("a_cs2")),
            ],
        );
        let s = scc(vec![(vec!["b"], false), (vec!["a"], false)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `a` reaches the SAME `callee` effect (a `Known(true)` op, non-PD) via a
    /// `direct` edge AND an `event-dispatch` edge — the folded `via` must be the
    /// max-rank of the two (`event-subscriber`, rank 2, beats `direct`'s
    /// `inherited`, rank 0 — see `effect_lattice::via_for_edge_kind`/`merge_via`).
    fn via_collision() -> FixtureOut {
        let mut callee = routine("callee");
        callee.record_operations.push(record_op(
            "callee_op1",
            "Insert",
            "t1",
            Some(ts_known(true)),
        ));

        let mut a = routine("a");
        a.call_sites.push(call_site("a_cs1", "Callee", Vec::new()));

        let g = graph(
            &["a", "callee"],
            vec![
                edge("a", "callee", "direct", Some("a_cs1")),
                edge("a", "callee", "event-dispatch", None),
            ],
        );
        let s = scc(vec![(vec!["callee"], false), (vec!["a"], false)]);

        (vec![a, callee], g, s, FieldIndex::new(), HashMap::new())
    }

    // -----------------------------------------------------------------------
    // Task A2 via-rank-merge guard fixtures (spec Step 3 test list, lines
    // 149-152): the "simplest safe v1" chosen for A2 keeps `reconstruct_via`/
    // `attribute_pd_substituted_via`'s EXISTING traversal structure, only
    // retyping the via map's VALUE from `String` to `ViaRank` — so these
    // fixtures pin that the retyped max-rank merge stays byte-identical to
    // the old solver across the specific collision/order shapes the spec's
    // Step 2 write-up calls out, not a NEW algorithm discovery (the
    // differential's byte-for-byte comparison against the untouched OLD
    // solver is the correctness oracle for every one of them).
    // -----------------------------------------------------------------------

    /// `via_collision`, but with its two edges added in the OPPOSITE order
    /// (`event-dispatch` first, `direct` second) — pins that the max-rank
    /// merge for a TERMINAL effect is order-INDEPENDENT (spec: "first-
    /// transition rank-0 then a later-transition rank-2/3" / vice versa),
    /// not an artifact of `graph.edges_by_from`'s insertion order.
    fn via_collision_edges_reversed() -> FixtureOut {
        let mut callee = routine("callee");
        callee.record_operations.push(record_op(
            "callee_op1",
            "Insert",
            "t1",
            Some(ts_known(true)),
        ));

        let mut a = routine("a");
        a.call_sites.push(call_site("a_cs1", "Callee", Vec::new()));

        let g = graph(
            &["a", "callee"],
            vec![
                edge("a", "callee", "event-dispatch", None),
                edge("a", "callee", "direct", Some("a_cs1")),
            ],
        );
        let s = scc(vec![(vec!["callee"], false), (vec!["a"], false)]);

        (vec![a, callee], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `a` reaches `b`'s SAME `ParameterDependent(0)` effect via TWO
    /// callsites of DIFFERENT edge kinds — `"direct"` (`via_for_edge_kind`
    /// -> `"inherited"`, rank 0) and `"implicit-trigger"` (rank 3) — both
    /// forwarding `b`'s param 0 to `a`'s own param 0, so BOTH substitute to
    /// the IDENTICAL produced identity on `a`. Exercises
    /// `attribute_pd_substituted_via`'s max-rank merge on the PD-substitution
    /// path specifically (spec item: "two callsites producing the same PD
    /// state at different ranks"). `reversed` controls edge insertion order
    /// — both orderings must reach the SAME old-solver-verified result (spec
    /// item: "first-transition rank-0 then a later-transition rank-2/3").
    fn pd_substituted_via_collision(reversed: bool) -> FixtureOut {
        let mut b = routine("b");
        b.record_operations
            .push(record_op("b_op1", "Insert", "t2", Some(ts_pd(0))));

        let mut a = routine("a");
        a.call_sites.push(call_site(
            "a_cs_direct",
            "B",
            vec![arg_binding(0, Some(ts_pd(0)))],
        ));
        a.call_sites.push(call_site(
            "a_cs_trigger",
            "B",
            vec![arg_binding(0, Some(ts_pd(0)))],
        ));

        let mut edges = vec![
            edge("a", "b", "direct", Some("a_cs_direct")),
            edge("a", "b", "implicit-trigger", Some("a_cs_trigger")),
        ];
        if reversed {
            edges.reverse();
        }
        let g = graph(&["a", "b"], edges);
        let s = scc(vec![(vec!["b"], false), (vec!["a"], false)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `a` owns a base `Known(true)` effect at the SAME `(op, table_id,
    /// operation_id)` triple as `b`'s `ParameterDependent(0)` effect; `a`
    /// calls `b` over a `"direct"` edge (`via_for_edge_kind` -> `"inherited"`,
    /// rank 0) binding param 0 to `Known(true)` — the substitution's produced
    /// identity COLLIDES exactly with `a`'s own base effect. `reconstruct_via`'s
    /// init seeds `"direct"` (rank 4) for `a`'s own effect BEFORE
    /// `attribute_pd_substituted_via` runs; the rank-0 `"inherited"`
    /// contribution from the colliding edge must NOT downgrade it (spec item:
    /// "a PD→Known colliding with a direct terminal").
    fn direct_terminal_beats_colliding_pd_substitution() -> FixtureOut {
        let mut b = routine("b");
        b.record_operations
            .push(record_op("shared_op1", "Insert", "t5", Some(ts_pd(0))));

        let mut a = routine("a");
        a.record_operations.push(record_op(
            "shared_op1",
            "Insert",
            "t5",
            Some(ts_known(true)),
        ));
        a.call_sites.push(call_site(
            "a_cs1",
            "B",
            vec![arg_binding(0, Some(ts_known(true)))],
        ));

        let g = graph(&["a", "b"], vec![edge("a", "b", "direct", Some("a_cs1"))]);
        let s = scc(vec![(vec!["b"], false), (vec!["a"], false)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// Same shape as [`direct_terminal_beats_colliding_pd_substitution`], but
    /// `a`'s own base effect STAYS `ParameterDependent(2)` (rather than
    /// resolving to `Known`) and the substitution's produced identity is ALSO
    /// `ParameterDependent(2)` — exercising the collision on the PD/delta
    /// storage path (spec item: "a PD→PD colliding with a direct PD").
    /// Because `a`'s own base PD(2) fact (Step A Seed 1) and the
    /// edge-substituted PD(2) fact (Step A Seed 2, forwarding `a`'s own
    /// param 2 through the `a -> b` edge) are the IDENTICAL `PdState`, this
    /// ALSO exercises the spec's "a duplicate transition where
    /// `visited.insert` is false" item — Step A's worklist dedup must not
    /// lose either the fact or (independently) its via attribution.
    fn direct_pd_base_beats_colliding_pd_substitution_and_dedup_transition() -> FixtureOut {
        let mut b = routine("b");
        b.record_operations
            .push(record_op("shared_op2", "Insert", "t6", Some(ts_pd(0))));

        let mut a = routine("a");
        a.record_operations
            .push(record_op("shared_op2", "Insert", "t6", Some(ts_pd(2))));
        a.call_sites.push(call_site(
            "a_cs1",
            "B",
            vec![arg_binding(0, Some(ts_pd(2)))],
        ));

        let g = graph(&["a", "b"], vec![edge("a", "b", "direct", Some("a_cs1"))]);
        let s = scc(vec![(vec!["b"], false), (vec!["a"], false)]);

        (vec![a, b], g, s, FieldIndex::new(), HashMap::new())
    }

    /// `ext` is an already-settled successor SCC (processed BEFORE the recursive
    /// SCC below it) carrying a `ParameterDependent(0)` op; the self-recursive
    /// `a` calls both itself and `ext` — the `ext` edge substitutes through `a`'s
    /// own binding while `ext`'s summary is read from the PREDECESSOR final map,
    /// not the in-SCC snapshot (the retired `compose_routine`'s `lookup`
    /// fallback, in the pre-`b4181d8` tree).
    fn external_successor_pd() -> FixtureOut {
        let mut ext = routine("ext");
        ext.record_operations
            .push(record_op("ext_op1", "Insert", "t1", Some(ts_pd(0))));

        let mut a = routine("a");
        a.call_sites.push(call_site("a_self_cs", "A", Vec::new()));
        a.call_sites.push(call_site(
            "a_ext_cs",
            "Ext",
            vec![arg_binding(0, Some(ts_known(true)))],
        ));

        let g = graph(
            &["a", "ext"],
            vec![
                edge("a", "a", "direct", Some("a_self_cs")),
                edge("a", "ext", "direct", Some("a_ext_cs")),
            ],
        );
        let s = scc(vec![(vec!["ext"], false), (vec!["a"], true)]);

        (vec![a, ext], g, s, FieldIndex::new(), HashMap::new())
    }

    /// A 3-member cycle `a -> b -> c -> a`; `c` is a FIXED LEAF (its summary
    /// comes from `leaf_summaries` — see [`fixed_leaf_in_scc_leaves`] — and is
    /// never recomputed). `b`'s composition must read `c`'s db effect from the
    /// pre-seeded final map rather than `c`'s (nonexistent) base summary.
    fn fixed_leaf_in_scc() -> FixtureOut {
        let mut a = routine("a");
        a.call_sites.push(call_site("a_cs1", "B", Vec::new()));

        let mut b = routine("b");
        b.call_sites.push(call_site("b_cs1", "C", Vec::new()));

        let mut c = routine("c");
        c.call_sites.push(call_site("c_cs1", "A", Vec::new()));

        let g = graph(
            &["a", "b", "c"],
            vec![
                edge("a", "b", "direct", Some("a_cs1")),
                edge("b", "c", "direct", Some("b_cs1")),
                edge("c", "a", "direct", Some("c_cs1")),
            ],
        );
        let s = scc(vec![(vec!["a", "b", "c"], true)]);

        (vec![a, b, c], g, s, FieldIndex::new(), HashMap::new())
    }

    /// The fixed-leaf summary for `fixed_leaf_in_scc`'s `c` member: one settled
    /// `Insert`/`Known(true)` effect on table `t3`, via `"direct"` (a retained
    /// dependency-routine summary, R3a-5-shaped).
    pub fn fixed_leaf_in_scc_leaves() -> HashMap<String, RoutineSummary> {
        let table_id = "t3";
        let effect_key = effect_key_of("Insert", table_id, "c_op1", &TempStateKind::Known(true));
        let mut leaves = HashMap::new();
        leaves.insert(
            "c".to_string(),
            RoutineSummary {
                routine_id: "c".to_string(),
                db_effects: vec![DbEffect {
                    effect_key,
                    operation_id: "c_op1".to_string(),
                    op: "Insert".to_string(),
                    table_id: table_id.to_string(),
                    record_variable_id: None,
                    temp_state: TempState::Known(true),
                    via: "direct".to_string(),
                }],
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );
        leaves
    }

    /// Same 3-member cycle `a -> b -> c -> a` as [`fixed_leaf_in_scc`], with `c`
    /// a FIXED LEAF — but `c`'s settled summary carries TWO own effects at
    /// DIFFERENT vias (see [`multi_effect_fixed_leaf_in_scc_leaves`]): an
    /// `Insert`/`direct` and a `Modify`/`event-subscriber`. `a`/`b` inherit both
    /// through the feed-forward, and `c` itself projects its multi-effect
    /// singleton row — so the per-effect via round-trip is exercised for a
    /// non-trivial leaf, not just the single-effect case.
    fn multi_effect_fixed_leaf_in_scc() -> FixtureOut {
        let mut a = routine("a");
        a.call_sites.push(call_site("a_cs1", "B", Vec::new()));

        let mut b = routine("b");
        b.call_sites.push(call_site("b_cs1", "C", Vec::new()));

        let mut c = routine("c");
        c.call_sites.push(call_site("c_cs1", "A", Vec::new()));

        let g = graph(
            &["a", "b", "c"],
            vec![
                edge("a", "b", "direct", Some("a_cs1")),
                edge("b", "c", "direct", Some("b_cs1")),
                edge("c", "a", "direct", Some("c_cs1")),
            ],
        );
        let s = scc(vec![(vec!["a", "b", "c"], true)]);

        (vec![a, b, c], g, s, FieldIndex::new(), HashMap::new())
    }

    /// The fixed-leaf summary for `multi_effect_fixed_leaf_in_scc`'s `c`: TWO
    /// settled effects at DIFFERENT vias — `Insert`/`Known(true)` on `t3` via
    /// `"direct"` and `Modify`/`Known(false)` on `t4` via `"event-subscriber"`.
    /// Listed in `(effect_key, operation_id)` order (`Insert|..` < `Modify|..`)
    /// so a pass-through OLD solver and the round-tripping v2 agree on emit
    /// order; the point of the fixture is that the two DISTINCT vias survive the
    /// compact-store round-trip attached to the correct effect.
    pub fn multi_effect_fixed_leaf_in_scc_leaves() -> HashMap<String, RoutineSummary> {
        let insert_key = effect_key_of("Insert", "t3", "c_op1", &TempStateKind::Known(true));
        let modify_key = effect_key_of("Modify", "t4", "c_op2", &TempStateKind::Known(false));
        let mut leaves = HashMap::new();
        leaves.insert(
            "c".to_string(),
            RoutineSummary {
                routine_id: "c".to_string(),
                db_effects: vec![
                    DbEffect {
                        effect_key: insert_key,
                        operation_id: "c_op1".to_string(),
                        op: "Insert".to_string(),
                        table_id: "t3".to_string(),
                        record_variable_id: None,
                        temp_state: TempState::Known(true),
                        via: "direct".to_string(),
                    },
                    DbEffect {
                        effect_key: modify_key,
                        operation_id: "c_op2".to_string(),
                        op: "Modify".to_string(),
                        table_id: "t4".to_string(),
                        record_variable_id: None,
                        temp_state: TempState::Known(false),
                        via: "event-subscriber".to_string(),
                    },
                ],
                in_recursive_cycle: false,
                has_unresolved_calls: false,
                uncertainties: Vec::new(),
                parameter_roles: Vec::new(),
            },
        );
        leaves
    }

    /// `ghost` is present in `scc.members` (a 2-member recursive SCC alongside
    /// `a`) but ABSENT from `routines` — the solver must skip it gracefully
    /// (no panic) while `a`'s own composition records an unresolved call to it.
    fn missing_routine_in_scc() -> FixtureOut {
        let mut a = routine("a");
        a.call_sites.push(call_site("a_cs1", "Ghost", Vec::new()));

        let g = graph(
            &["a", "ghost"],
            vec![edge("a", "ghost", "direct", Some("a_cs1"))],
        );
        let s = scc(vec![(vec!["a", "ghost"], true)]);

        (vec![a], g, s, FieldIndex::new(), HashMap::new())
    }
}
