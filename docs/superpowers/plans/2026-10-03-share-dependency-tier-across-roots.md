# Share the Dependency Tier Across Workspace Roots — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A multi-root LSP session holds each dependency's nodes and parsed symbols ONCE per process, and no root's program graph copies them, so 7 roots fit well under 1 GB.

**Architecture:** (A) `ProgramGraph.objects`/`routines` become a `NodeSet<T>`: a shared, `Arc`-held sorted dependency part plus a small per-root sorted part (workspace nodes and synthetic platform publishers), read as one sorted list. `assemble_program_graph` stops cloning the dependency nodes. (B) A process-level `DepCache`, owned by the LSP server, hands every root that loads the SAME dependency set the same `Arc`s (dependency nodes; per-`.app` parsed symbol packages), held weakly so entries die with their last root.

**Tech Stack:** Rust (crate `al-sem`), existing engine types (`ProgramGraph`, `DepLayer`, `AppSetSnapshot`).

**Spec:** this document plus the measurements below (no separate spec; design agreed with the user 2026-10-03: levers A + B, not the LRU cap). Evidence: `byte-census` of one root and a 7-root wrapper-shaped driver (scratch `root-census` probe, `multiroot.mjs`).

## Why (measured, 2026-10-03, refapp + BC 28.4 Microsoft packages, `--dependency-source symbols`)

One built root retains **106.6 MiB live heap in 1.05 M allocations** (≈131 MiB with ~24 B/alloc allocator overhead):

| field | MiB | allocs |
|---|---:|---:|
| `graph` (`ProgramGraph`) | 36.0 | 430k |
| `dep_layer` (`DepLayer`) | 34.3 | 420k |
| `snap.apps[].abi` Base Application (`ParsedAppPackage`) | 21.5 | 107k |
| `event_edges` | 11.8 | 72k |
| `snap.apps[].abi` System Application | 2.5 | 14k |
| everything else (incl. the workspace itself) | ~0.5 | |

7 roots: **1,214 MB** process peak, ~170 MB per root, linear; `--no-diagnostics` 1,218 MB (no effect). `graph` ≈ `dep_layer` because `assemble_program_graph` (`src/program/build.rs:179-180`) clones `dep_objects`/`dep_routines`. All roots of the harness workspace load the same `.alpackages`, so the dependency data is identical across roots.

Expected after A + B (7 roots): shared ≈ dep nodes 34 + packages 24 MiB once; per root ≈ `event_edges` 12 MiB + small. ≈ 160 MiB live.

## Global Constraints

- **Behaviour-preserving.** Every golden family (`scripts/check-goldens`) and the CDO gate (`scripts/cdo-gate`, `CDO_WS=U:/Git/DO-cdo-baseline/Cloud`) must stay byte-identical. Iteration order of `graph.objects`/`graph.routines` must equal the old sorted-`Vec` order exactly, ties included.
- Format with `rustfmt <file>` per file, never `cargo fmt`. Lint bar: `scripts/ci-steps clippy`.
- Every new test gets a discrimination proof (break the code, see it fail, revert) recorded in the commit message.
- Run `df -h /u` before each full build/gate run; clean stale `target/` profiles (user's standing rule, 2026-10-03).
- CLI/one-shot paths (aldump, alsem, CLI index) keep working with a throwaway `DepCache::default()`; only the LSP server shares one across roots.

## Review Focus

1. **Two roots, different dependency sets** (a sibling root's compiled `.app` sits in the shared `.alpackages`, so the self-dependency guard drops it for one root only): AppRef numbering differs, so the roots must NOT share. Pinned in Task 4.
2. **A `.app` replaced on disk mid-session, then a rung-3 rebuild**: must not reuse the stale cached nodes. The key carries each `.app`'s length and modified time. Pinned in Task 4.
3. **A synthetic platform publisher that ties on id with a real dependency routine** (inject only checks for a real *publisher*): the old stable sort kept the shared (real) routine first. `NodeSet` must too. Pinned in Task 1.
4. **Rung 2 after sharing**: must still reuse the same `Arc`s (no re-clone) and stay `Arc::ptr_eq` on the dependency part. Pinned in Task 3.
5. **`--dependency-source embedded` vs `symbols`** for the same root are different dependency tiers; they must never share. Pinned in Task 4.

---

## File Structure

- Create `src/program/node_set.rs` — `NodeSet<T>` + `SortKey` (Task 1).
- Modify `src/program/mod.rs` — `pub mod node_set; pub mod dep_cache;`.
- Modify `src/program/graph.rs` — `ProgramGraph.objects/routines: NodeSet<_>`; `ObjectIndex` reads through `NodeSet::get` (Task 2).
- Modify every compile-error site the type change surfaces (Task 2; mostly tests: `vec![..]` → `vec![..].into()`; slice-taking helpers → `&NodeSet<_>`).
- Modify `src/program/build.rs` — `DepLayer` holds `Arc<Vec<_>>`; `assemble_program_graph` builds `NodeSet::layered` without cloning (Task 3); `build_dep_layer` consults `DepCache` (Task 4).
- Create `src/program/dep_cache.rs` — `DepCache`, `DepKey` (Task 4).
- Modify `src/dependencies.rs`, `src/snapshot/snapshot.rs` — `ParsedAppPackage` shared as `Arc` through `DepCache` (Task 5).
- Modify `src/program/resolve/full.rs`, `src/lsp/snapshot.rs`, `src/lsp/updater.rs`, `src/server.rs` — thread `&DepCache` (Task 4).

---

### Task 1: `NodeSet<T>` — a sorted list with a shared part and an own part

**Files:**
- Create: `src/program/node_set.rs`
- Modify: `src/program/mod.rs` (add `pub mod node_set;`)

**Interfaces:**
- Produces: `pub trait SortKey { type Key: Ord; fn sort_key(&self) -> &Self::Key; }` implemented for `ObjectNode` (key `ObjectNodeId`) and `RoutineNode` (key `RoutineNodeId`); `pub struct NodeSet<T>` with `layered(shared: Arc<Vec<T>>, own: Vec<T>) -> Self`, `len`, `is_empty`, `get(usize) -> Option<&T>`, `iter() -> NodeSetIter<'_, T>`, `binary_search_by(FnMut(&T) -> Ordering) -> Result<usize, usize>`, `push`, `extend`, `sort_by`, `shared() -> &Arc<Vec<T>>`, `own() -> &[T]`; `Index<usize>`, `From<Vec<T>>`, `Default`, `IntoIterator for &NodeSet<T>`.

- [ ] **Step 1: Write the failing tests** (in `src/program/node_set.rs`, `#[cfg(test)] mod tests`). Use a minimal test element so the merge logic is tested on its own:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct N(u32, &'static str);
    impl SortKey for N {
        type Key = u32;
        fn sort_key(&self) -> &u32 {
            &self.0
        }
    }

    fn flat(set: &NodeSet<N>) -> Vec<N> {
        set.iter().cloned().collect()
    }

    #[test]
    fn layered_reads_as_one_sorted_list() {
        // own = workspace prefix (0, 1) + one interleaved synthetic (5).
        let set = NodeSet::layered(
            Arc::new(vec![N(3, "s"), N(4, "s"), N(7, "s")]),
            vec![N(0, "o"), N(1, "o"), N(5, "o")],
        );
        let want = vec![N(0, "o"), N(1, "o"), N(3, "s"), N(4, "s"), N(5, "o"), N(7, "s")];
        assert_eq!(flat(&set), want);
        assert_eq!(set.len(), 6);
        for (i, w) in want.iter().enumerate() {
            assert_eq!(&set[i], w, "index {i}");
        }
        assert_eq!(set.get(6), None);
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&5)), Ok(4));
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&6)), Err(5));
        assert_eq!(set.binary_search_by(|n| n.0.cmp(&9)), Err(6));
    }

    /// Review Focus 3: on an id tie the SHARED element comes first — what
    /// the old `extend` + stable `sort_by` produced for a synthetic that
    /// ties with an existing dependency routine.
    #[test]
    fn tie_puts_shared_first() {
        let set = NodeSet::layered(Arc::new(vec![N(2, "s")]), vec![N(2, "o")]);
        assert_eq!(flat(&set), vec![N(2, "s"), N(2, "o")]);
        assert_eq!(set[0], N(2, "s"));
        assert_eq!(set[1], N(2, "o"));
    }

    #[test]
    fn push_then_sort_matches_vec_semantics() {
        let mut set: NodeSet<N> = vec![N(1, "a"), N(4, "a")].into();
        set.push(N(2, "b"));
        set.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(flat(&set), vec![N(1, "a"), N(2, "b"), N(4, "a")]);
        assert_eq!(set[1], N(2, "b"));
    }

    #[test]
    fn empty_parts_are_fine() {
        let empty: NodeSet<N> = NodeSet::default();
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);
        let only_shared = NodeSet::layered(Arc::new(vec![N(1, "s")]), Vec::new());
        assert_eq!(flat(&only_shared), vec![N(1, "s")]);
    }
}
```

- [ ] **Step 2: Run to verify it fails.** `cargo test -p al-sem --lib node_set` → FAIL (module/type not defined).

- [ ] **Step 3: Implement `src/program/node_set.rs`:**

```rust
//! A sorted node list in two sorted parts, read as ONE sorted list.
//!
//! `shared` is the dependency tier: built once, `Arc`-held, identical for
//! every workspace root that loads the same dependencies (see
//! `program::dep_cache`). `own` is this root's part: its workspace nodes
//! (always `AppRef(0)`, so a prefix) plus synthetic platform publishers,
//! which carry a dependency `AppRef` and so interleave with `shared`.
//! Iteration, indexing and binary search all see the merged order, and on an
//! id tie the `shared` element comes first — exactly what the old
//! `Vec::extend` + stable `sort_by` produced. Nothing is copied.

use std::cmp::Ordering;
use std::ops::Index;
use std::sync::Arc;

use crate::program::node::{ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{ObjectNode, RoutineNode};

/// The key a node list is sorted by.
pub trait SortKey {
    type Key: Ord;
    fn sort_key(&self) -> &Self::Key;
}

impl SortKey for ObjectNode {
    type Key = ObjectNodeId;
    fn sort_key(&self) -> &ObjectNodeId {
        &self.id
    }
}

impl SortKey for RoutineNode {
    type Key = RoutineNodeId;
    fn sort_key(&self) -> &RoutineNodeId {
        &self.id
    }
}

#[derive(Debug)]
pub struct NodeSet<T> {
    shared: Arc<Vec<T>>,
    own: Vec<T>,
    /// Merged position of each `own` element, ascending (one per element).
    own_pos: Vec<usize>,
}

impl<T> Default for NodeSet<T> {
    fn default() -> Self {
        NodeSet {
            shared: Arc::new(Vec::new()),
            own: Vec::new(),
            own_pos: Vec::new(),
        }
    }
}

impl<T: SortKey> NodeSet<T> {
    /// `shared` and `own` must each already be sorted by `sort_key`.
    pub fn layered(shared: Arc<Vec<T>>, own: Vec<T>) -> Self {
        let mut set = NodeSet {
            shared,
            own,
            own_pos: Vec::new(),
        };
        set.reindex();
        set
    }

    /// Recompute `own_pos`: an own element lands after every shared element
    /// whose key is <= its own (shared first on ties).
    fn reindex(&mut self) {
        let mut pos = Vec::with_capacity(self.own.len());
        let mut k = 0;
        for (i, o) in self.own.iter().enumerate() {
            while k < self.shared.len() && self.shared[k].sort_key() <= o.sort_key() {
                k += 1;
            }
            pos.push(i + k);
        }
        self.own_pos = pos;
    }

    pub fn len(&self) -> usize {
        self.shared.len() + self.own.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        match self.own_pos.binary_search(&i) {
            Ok(j) => self.own.get(j),
            Err(j) => self.shared.get(i - j),
        }
    }

    pub fn iter(&self) -> NodeSetIter<'_, T> {
        NodeSetIter {
            shared: &self.shared,
            own: &self.own,
            s: 0,
            o: 0,
        }
    }

    /// Same contract as `slice::binary_search_by`, over the merged order.
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, mut f: F) -> Result<usize, usize> {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match f(&self[mid]) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
    }

    /// Append to the own part. Like `Vec::push` on a sorted list, the caller
    /// re-sorts afterwards (`sort_by`) before relying on the order.
    pub fn push(&mut self, item: T) {
        self.own.push(item);
        self.reindex();
    }

    pub fn extend<I: IntoIterator<Item = T>>(&mut self, items: I) {
        self.own.extend(items);
        self.reindex();
    }

    /// Stable-sort the own part (the shared part is sorted by construction).
    pub fn sort_by<F: FnMut(&T, &T) -> Ordering>(&mut self, f: F) {
        self.own.sort_by(f);
        self.reindex();
    }

    pub fn shared(&self) -> &Arc<Vec<T>> {
        &self.shared
    }

    pub fn own(&self) -> &[T] {
        &self.own
    }
}

impl<T: SortKey> Index<usize> for NodeSet<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        self.get(i).expect("NodeSet index out of bounds")
    }
}

impl<T: SortKey> From<Vec<T>> for NodeSet<T> {
    fn from(own: Vec<T>) -> Self {
        NodeSet::layered(Arc::new(Vec::new()), own)
    }
}

impl<'a, T: SortKey> IntoIterator for &'a NodeSet<T> {
    type Item = &'a T;
    type IntoIter = NodeSetIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Merged in-order iterator: shared first on ties.
#[derive(Clone)]
pub struct NodeSetIter<'a, T> {
    shared: &'a [T],
    own: &'a [T],
    s: usize,
    o: usize,
}

impl<'a, T: SortKey> Iterator for NodeSetIter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        let take_shared = match (self.shared.get(self.s), self.own.get(self.o)) {
            (Some(s), Some(o)) => s.sort_key() <= o.sort_key(),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        if take_shared {
            self.s += 1;
            Some(&self.shared[self.s - 1])
        } else {
            self.o += 1;
            Some(&self.own[self.o - 1])
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.shared.len() - self.s + self.own.len() - self.o;
        (n, Some(n))
    }
}

impl<T: SortKey> ExactSizeIterator for NodeSetIter<'_, T> {}
```

Note: `push` reindexes on an unsorted own part; `own_pos` is only meaningful once sorted, matching the old "push then sort" contract on `Vec`. `reindex` on an unsorted own still terminates (positions may be non-monotone until `sort_by`).

- [ ] **Step 4: Run** `cargo test -p al-sem --lib node_set` → all 4 PASS.
- [ ] **Step 5: Discrimination proofs.** (a) In `reindex` change `<=` to `<`: `tie_puts_shared_first` FAILS. (b) In `NodeSetIter::next` change `<=` to `<`: `tie_puts_shared_first` FAILS. (c) In `get` swap `Ok(j) => self.own.get(j)` to `self.shared.get(j)`: `layered_reads_as_one_sorted_list` FAILS. Revert each; record all three in the commit message.
- [ ] **Step 6: Commit** `feat(program): NodeSet — a sorted node list with a shared and an own part`.

---

### Task 2: `ProgramGraph` stores its nodes in `NodeSet` (no behaviour change)

**Files:**
- Modify: `src/program/graph.rs` (fields `objects`, `routines`; `ObjectIndex::build` takes `&NodeSet<ObjectNode>`; `resolve_object` reads `self.objects[idx]` — unchanged text, now `Index` on `NodeSet`).
- Modify: every site `cargo check --all-targets` reports.

**Interfaces:**
- Consumes: `NodeSet`, `SortKey` (Task 1).
- Produces: `ProgramGraph { objects: NodeSet<ObjectNode>, routines: NodeSet<RoutineNode>, .. }`; `ObjectIndex::build(&NodeSet<ObjectNode>)`.

- [ ] **Step 1: Change the field types** in `graph.rs`:

```rust
use crate::program::node_set::NodeSet;
// ...
    /// All object nodes, sorted by `ObjectNodeId` for determinism. A
    /// `NodeSet`: the dependency part is shared, never copied (see
    /// `program::node_set`).
    pub objects: NodeSet<ObjectNode>,
    /// All routine nodes, sorted by `RoutineNodeId` for determinism.
    pub routines: NodeSet<RoutineNode>,
```

and `ObjectIndex::build(objects: &NodeSet<ObjectNode>)` (body unchanged: `objects.iter().enumerate()` yields merged positions, which `resolve_object`'s `self.objects[idx]` reads back through the same order).

- [ ] **Step 2: In `assemble_program_graph`** (build.rs:179-207) wrap the existing sorted Vecs unchanged for now: `objects: objects.into()`, `routines: routines.into()`, and `ObjectIndex::build(&objects)` after the `into()`. (Task 3 removes the clone.) In `inject_platform_event_publishers` the existing `graph.routines.extend(synth); graph.routines.sort_by(...)` compiles as-is against `NodeSet`.

- [ ] **Step 3: Fix every compile error** from `cargo check --all-targets` with these mechanical rules only:
  - A struct literal `objects: vec![...]` / `routines: vec![...]` / `objects: some_vec` → append `.into()`.
  - A function taking `&[RoutineNode]` or `&[ObjectNode]` that is passed `&graph.routines`/`&graph.objects` (e.g. `dual_publisher_alias_skip_count`, `dual_publisher_alias_ids` in `resolve/resolver.rs`, called from `full.rs:983`) → change the parameter to `impl IntoIterator<Item = &RoutineNode>` when it only iterates, else to `&NodeSet<RoutineNode>`. Its slice-based test callers pass `&nodes` (a `Vec`) — `&Vec<T>` implements `IntoIterator<Item=&T>`, so they keep compiling.
  - `.iter().position(..)`, `.first()`, `.last()`, `.windows(..)`, `.to_vec()` on a graph node list → `.iter().position(..)`, `.iter().next()`, `.iter().last()`, explicit pairs via `iter().zip(iter().skip(1))`, `.iter().cloned().collect::<Vec<_>>()`.
  Do not change any logic. If a site needs more than these rules, stop and report it.

- [ ] **Step 4: Prove it is behaviour-preserving.** `df -h /u`; then `scripts/check-goldens` → all 9 targets green, no golden file modified (`git status --short tests/` empty); `cargo test -p al-sem --lib` → green (includes `assemble_program_graph_matches_build_program_graph_field_by_field`, build.rs:676); `scripts/ci-steps clippy` → green.
- [ ] **Step 5: Commit** `refactor(program): ProgramGraph nodes live in NodeSet (no behaviour change)`.

---

### Task 3 (lever A): assemble the graph without copying the dependency nodes

**Files:**
- Modify: `src/program/build.rs` (`DepLayer.dep_objects/dep_routines: Arc<Vec<_>>`; `assemble_program_graph`).
- Test: `src/program/build.rs` tests (extend `assemble_program_graph_reuses_dep_layer_across_two_workspace_edits`, build.rs:824).

**Interfaces:**
- Produces: `DepLayer { dep_objects: Arc<Vec<ObjectNode>>, dep_routines: Arc<Vec<RoutineNode>>, .. }`; `assemble_program_graph` returns a graph whose `objects.shared()`/`routines.shared()` are `Arc::ptr_eq` to the `DepLayer`'s.

- [ ] **Step 1: Write the failing test** (Review Focus 4) — add to build.rs tests, next to the existing reuse test, using its fixture helpers:

```rust
/// Lever A: the assembled graph SHARES the dep layer's node lists — no
/// copy — and a second assembly (rung 2) shares the very same allocation.
#[test]
fn assemble_program_graph_shares_dep_nodes_without_copying() {
    let (snap, parsed) = two_app_fixture(); // same helper the reuse test above uses
    let dep = build_dep_layer(&snap, &AbiCache::new(), &parsed);
    let ws = parsed.iter().find(|u| u.app == snap.workspace_app).unwrap();
    let g1 = assemble_program_graph(&dep, ws, &snap);
    let g2 = assemble_program_graph(&dep, ws, &snap);
    assert!(Arc::ptr_eq(g1.objects.shared(), &dep.dep_objects));
    assert!(Arc::ptr_eq(g1.routines.shared(), &dep.dep_routines));
    assert!(Arc::ptr_eq(g1.routines.shared(), g2.routines.shared()));
    // Own part = workspace nodes + synthetics only.
    assert!(g1.objects.own().iter().all(|o| Some(o.id.app) == dep.apps.find(&snap.workspace_app)));
}
```

(If the reuse test's fixture is inline rather than a helper, extract it into `fn two_app_fixture() -> (AppSetSnapshot, Vec<ParsedUnit>)` in the same `mod tests` as part of this step.)

- [ ] **Step 2: Run** `cargo test -p al-sem --lib assemble_program_graph_shares` → FAIL (`DepLayer` fields are `Vec`).
- [ ] **Step 3: Implement.** In `DepLayer`: `pub dep_objects: Arc<Vec<ObjectNode>>`, `pub dep_routines: Arc<Vec<RoutineNode>>` (doc: "shared, never cloned into a graph"); `build_dep_layer` ends with `dep_objects: Arc::new(objects), dep_routines: Arc::new(routines)`. Replace `assemble_program_graph`'s body lines 179-207 with:

```rust
    // Workspace nodes only; the dependency part is shared, not copied.
    let mut objects: Vec<ObjectNode> = Vec::new();
    let mut routines: Vec<RoutineNode> = Vec::new();
    for pf in &ws_unit.files {
        extract_nodes(ws_app_ref, &pf.file, pf.provenance.tier, &mut objects, &mut routines);
    }
    // Dedup the workspace population exactly as before. Dependency entries
    // can never collide with these: the primary's AppRef is disjoint.
    objects.sort_by(|a, b| a.id.cmp(&b.id));
    objects.dedup_by(|a, b| a.id == b.id);
    routines.sort_by(|a, b| a.id.cmp(&b.id));
    dedup_routines_preserving_genuine_overloads(&mut routines);

    let objects = NodeSet::layered(Arc::clone(&dep.dep_objects), objects);
    let routines = NodeSet::layered(Arc::clone(&dep.dep_routines), routines);
    let obj_index = ObjectIndex::build(&objects);
```

and the `ProgramGraph { .. }` literal takes `objects, routines` directly. `inject_platform_event_publishers` is unchanged: its `extend` + `sort_by` land in the own part, interleaved by `NodeSet`'s merge. Fix the remaining compile errors (`dep.dep_objects.clone()` call sites in tests → `.as_ref().clone()` where a `Vec` is genuinely needed; `DepLayer { dep_objects: vec![..] }` literals → `Arc::new(vec![..])`).

- [ ] **Step 4: Run** the new test → PASS; `cargo test -p al-sem --lib` green; `cargo test -p al-sem --lib updater` green (rung-2 `Arc::ptr_eq` tests, updater.rs:2051-2096).
- [ ] **Step 5: Discrimination proof:** in `assemble_program_graph` replace `Arc::clone(&dep.dep_routines)` with `Arc::new(dep.dep_routines.as_ref().clone())` → the new test FAILS on the `ptr_eq` asserts. Revert.
- [ ] **Step 6: Behaviour gate.** `df -h /u`; `scripts/check-goldens` (no golden moves), `CDO_WS=U:/Git/DO-cdo-baseline/Cloud scripts/cdo-gate` (byte-identical metric JSON SHA in its output).
- [ ] **Step 7: Measure** with the census probe (scratch `probe/`, `CARGO_TARGET_DIR` outside the repo): one root, symbols. Expected: `graph` ≈ workspace-only (≪ 36 MiB); record before/after in the commit message.
- [ ] **Step 8: Commit** `perf(program): the program graph shares the dependency tier instead of copying it`.

---

### Task 4 (lever B): share the dependency tier across roots with `DepCache`

**Files:**
- Create: `src/program/dep_cache.rs`; Modify: `src/program/mod.rs`.
- Modify: `src/program/build.rs` (`build_dep_layer` takes `&DepCache`), `src/program/resolve/full.rs` (`build_context_with`, `build_context_from_snapshot`), `src/lsp/snapshot.rs` (`build_full_with`, `build_full_with_parsed_with`), `src/lsp/updater.rs` (`Updater` holds `Arc<DepCache>`), `src/server.rs` (`RootBuild` holds `Arc<DepCache>`; `run_server` creates ONE).

**Interfaces:**
- Produces:

```rust
/// Process-level dependency tier, shared by every workspace root that loads
/// the SAME dependency set. Entries are held weakly: one lives exactly as
/// long as some root's snapshot still uses it.
#[derive(Default)]
pub struct DepCache {
    nodes: Mutex<HashMap<DepKey, Weak<DepNodes>>>,
}

/// The shareable part of a `DepLayer` — independent of which root built it
/// (its AppRefs are 1..n in `snap.apps` order; the root's own app is always
/// AppRef(0) and never appears here).
pub struct DepNodes {
    pub objects: Arc<Vec<ObjectNode>>,
    pub routines: Arc<Vec<RoutineNode>>,
    pub abi_ingest_errors: Vec<AbiIngestError>,
}

impl DepCache {
    pub fn get_or_build(&self, key: DepKey, build: impl FnOnce() -> DepNodes) -> Arc<DepNodes>;
}

/// Everything the dependency nodes depend on: the dependency apps in
/// `snap.apps[1..]` order (that order IS their AppRef numbering), each with
/// its on-disk identity and whether its source was indexed.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DepKey { apps: Vec<DepAppKey> }
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct DepAppKey { id: AppId, path: Option<PathBuf>, len: u64, modified: Option<SystemTime>, has_source: bool }

impl DepKey {
    pub fn of(snap: &AppSetSnapshot) -> DepKey;
}
```

  `has_source` encodes `--dependency-source` per app (Review Focus 5); `len` + `modified` catch a replaced `.app` (Review Focus 2); the exact ordered list catches a different dependency set (Review Focus 1). `get_or_build` holds the mutex only for lookup/insert, never while building (two roots building the same key concurrently both build; the second insert keeps the first live `Arc`).
- `build_dep_layer(snap, abi_cache, parsed, dep_cache: &DepCache) -> DepLayer` — `apps`/`topology`/`friends` still built per root (small, primary-dependent); `dep_objects`/`dep_routines`/`abi_ingest_errors` come from `dep_cache.get_or_build(DepKey::of(snap), || <existing steps 2, 2b, 4>)`.
- Old signatures stay as wrappers passing `&DepCache::default()` (tests, aldump, alsem, CLI).

- [ ] **Step 1: Write failing tests** in `src/program/dep_cache.rs`, building real snapshots with the `.app` writers in `crate::engine::deps::app_package_zip::test_apps` (as `snapshot.rs`'s `ready_to_run_package_loads_as_a_dependency` does): two workspace roots side by side under one parent whose `.alpackages` holds one dependency app.

```rust
#[test]
fn roots_with_the_same_dependencies_share_one_dep_tier() {
    let fx = two_roots_one_alpackages(); // helper in this test module
    let cache = DepCache::default();
    let a = LspSnapshot::build_full_with_cache(&fx.root_a, DependencySource::Symbols, &cache).unwrap();
    let b = LspSnapshot::build_full_with_cache(&fx.root_b, DependencySource::Symbols, &cache).unwrap();
    assert!(Arc::ptr_eq(a.graph.routines.shared(), b.graph.routines.shared()));
}

#[test]
fn different_dependency_source_does_not_share() { /* same fixture, Embedded vs Symbols → !ptr_eq */ }

#[test]
fn a_sibling_compiled_app_in_alpackages_splits_the_sets() {
    // Put root_a's own compiled .app (same GUID as root_a's app.json) into the
    // shared .alpackages: root_a drops it (self-dependency guard), root_b loads
    // it, so AppRef numbering differs → must NOT share, and both graphs must
    // still resolve their own calls (compare to a fresh, cache-less build).
}

#[test]
fn a_replaced_app_is_not_served_stale() {
    // Build root_a, keep it alive, rewrite the dependency .app with one more
    // codeunit (new len/mtime), build root_a again with the same cache:
    // the new graph contains the new codeunit; !ptr_eq with the old one.
}

#[test]
fn entries_die_with_their_last_root() {
    // Build, drop the snapshot, build again: a fresh Arc (Weak expired),
    // and the map holds no live entry in between (expose `fn live_entries(&self) -> usize` under #[cfg(test)]).
}
```

  Write the fixture helper `two_roots_one_alpackages()` concretely in this step (tempdir; `root_a/app.json`, `root_b/app.json` with distinct GUIDs, each one `src/X.Codeunit.al` calling `Sales-Post.Run`; parent `.alpackages/Dep.app` from `test_apps::build_app` with a `NavxManifest.xml` and a `SymbolReference.json` declaring codeunit 80 "Sales-Post" with method `Run`).

- [ ] **Step 2: Run** → FAIL (no `DepCache`).
- [ ] **Step 3: Implement** `dep_cache.rs` per the interface above; `DepKey::of` reads `snap.apps[1..]` with `std::fs::metadata(path)` (`len`, `modified().ok()`) when `app_path` is `Some`. Thread `&DepCache`: `build_dep_layer` (+ wrapper), `build_program_graph_from_parsed` (+ wrapper), `build_context_from_snapshot` (+ wrapper), `build_context_with(root, source, cache)`, `LspSnapshot::build_full_with_cache` / `build_full_with_parsed_with_cache` (the existing `_with` variants call these with `&DepCache::default()`). `Updater` gets `dep_cache: Arc<DepCache>` via `with_dep_cache(..)` and uses it in `apply_rung3`. `spawn_updater` takes `Arc<DepCache>`. `server.rs`: `run_server` creates `let dep_cache = Arc::new(DepCache::default());`, `build_workspace` stores `Arc::clone(&dep_cache)` in each `RootBuild`, `build_server_state` passes it to the build and to `spawn_updater`.
- [ ] **Step 4: Run** the new tests → PASS; full `cargo test -p al-sem --lib` and `cargo test --bin al-call-hierarchy` green.
- [ ] **Step 5: Discrimination proofs:** (a) make `DepKey::of` drop `has_source` → `different_dependency_source_does_not_share` FAILS; (b) drop `len`/`modified` → `a_replaced_app_is_not_served_stale` FAILS; (c) build the key from the `has_source` flags only, dropping each app's identity and path → the two roots wrongly share → `a_sibling_compiled_app_in_alpackages_splits_the_sets` FAILS (its graph disagrees with the cache-less build); (d) store `Arc` instead of `Weak` → `entries_die_with_their_last_root` FAILS. Revert each; record all four.
- [ ] **Step 6: Behaviour gate** (as Task 3 Step 6) + **measure** with `multiroot.mjs` (7 roots, symbols): record peak; expected well under the 1,214 MB baseline.
- [ ] **Step 7: Commit** `perf(lsp): share the dependency tier across workspace roots (DepCache)`.

---

### Task 5: share each dependency's parsed symbol package (`AppUnit.abi`) across roots

**Files:**
- Modify: `src/program/dep_cache.rs` (a second weak map keyed by `.app` path + len + mtime), `src/dependencies.rs` (`ResolvedDependency.package: Arc<ParsedAppPackage>`; `load_all_apps_with(root, cache)`), `src/snapshot/snapshot.rs` (`AppUnit.abi: Option<Arc<ParsedAppPackage>>`; `SnapshotBuilder::build_with_options` takes `&DepCache`).
- Readers compile unchanged through `Deref` (`lsp/custom.rs:371, 528`, `full.rs:1270`); test literals `abi: Some(pkg)` → `Some(Arc::new(pkg))`.

**Interfaces:**
- Produces: `DepCache::package(&self, path: &Path, load: impl FnOnce() -> Result<ParsedAppPackage>) -> Result<Arc<ParsedAppPackage>>`.

- [ ] **Step 1: Failing test** in `dep_cache.rs`: with the Task 4 fixture, `Arc::ptr_eq` on `a.snap.apps[i].abi` vs `b.snap.apps[j].abi` for the dependency app (find by GUID).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the package map and use it at the two `extract_app_symbols`/`extract_app_package` sites in `dependencies.rs` (lines ~510, ~639).
- [ ] **Step 4: Run** → PASS; full lib + bin tests green.
- [ ] **Step 5: Discrimination proof:** bypass the cache in `load_all_apps_with` → test FAILS. Revert.
- [ ] **Step 6: Commit** `perf(deps): share parsed dependency packages across roots`.

---

### Task 6: measure, document, gate, hand off

- [ ] **Step 1:** `df -h /u`; clean stale profiles (`target/release-fast` if unused).
- [ ] **Step 2:** Census (one root) and `multiroot.mjs` (1, 3, 7 roots; symbols AND embedded) before vs after, same machine, same binary profile (release-fast), back-to-back. Report live MiB + allocation counts per bucket and process peak.
- [ ] **Step 3:** CHANGELOG `[Unreleased]` → `Changed`: what is shared, the numbers, the sharing conditions (same dependency set, same `--dependency-source`), and what stays per root (`event_edges`, embedded dependency source text and `dep_meta` in `embedded` mode).
- [ ] **Step 4:** `scripts/ci-steps all`, `scripts/check-goldens`, `scripts/cdo-gate` — all green.
- [ ] **Step 5:** Commit, then build a test exe for the harness (lane-ops) to run a full 7-root cell at 3 GB; release only after that passes AND the user approves.

## Out of scope (recorded, not built)

- Sharing `event_edges` across roots (~12 MiB/root): edges mix dependency publishers with workspace subscribers; needs a dependency-only/overlay split.
- `--dependency-source embedded`: dependency source text, its parse and `dep_meta` stay per root (~1.4 GB transient for Base Application). A per-process parsed-dependency cache is the follow-up.
- The LRU cap on built roots (lever C) — not chosen.
