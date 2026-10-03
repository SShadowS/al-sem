# Share the Embedded-Mode Dependency Tier Across Roots — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In `--dependency-source embedded` (the default), a multi-root LSP session holds each dependency's source text and declaration metadata once per process, and only the first root that loads a given dependency set parses its source.

**Architecture:** Extend the process-level `DepCache` (src/program/dep_cache.rs, added by the 2026-10-03 shared-dependency-tier plan) with (1) a per-`.app` map of extracted embedded source (`SourceRoot`), keyed like the package map on `(path, pre-read AppFileStamp)`, so every root's `AppUnit.source` and `dep_texts` point at the same `Arc<str>`s; and (2) the derived dependency tier products — `dep_meta: Arc<DepMetaMap>` and `dep_texts: Arc<DepTexts>` — stored with the shared `DepNodes` under the existing `DepKey`. On a `DepKey` hit, the context build parses only the workspace unit, takes nodes/meta/texts from the cache, and builds the resolver's declaration surface as workspace-only `.with_frozen(dep_meta)` — the construction the updater's rung 1 already uses and the incremental-parity harness already checks.

**Tech Stack:** Rust (crate `al-sem`).

**Spec:** this document; the design follows the user's request of 2026-10-03 ("share the embedded-mode dependency parse across roots too") and the measurements below. Predecessor: `docs/superpowers/plans/2026-10-03-share-dependency-tier-across-roots.md` (merged as al-sem v1.3.2).

## Why (measured 2026-10-03, refapp/Core + BC 28.4 Microsoft packages, embedded, al-sem v1.3.2)

Byte census of one root (live heap, counting allocator, after the background drop of dependency parse trees settles):

| field | MiB | shared across roots in v1.3.2? |
|---|---:|---|
| `dep_texts` | 109.8 | no |
| `dep_meta` | 55.1 | no |
| `dep_layer` (nodes) | 44.1 | yes |
| `snap.apps[].abi` | 24.0 | yes |
| `event_edges` | 15.5 | no |
| rest | ~4 | |
| **retained total** | **252.3** (1,335,423 allocs) | |

Right after the build returns, live heap is 1,056 MiB: ~800 MiB of dependency parse trees still being freed by the `dep-arena-drop` thread. Every root re-extracts the source (`cached_source` hashes the whole `.app` and deserialises its JSON cache) and re-parses it. Process peak, 7 roots embedded: 2,895 MB (v1.3.2 measurement, one run).

Expected after: shared ≈ 110 + 55 + 44 + 24 MiB once; per root ≈ `event_edges` + small; only the first root of a dependency set pays the ~800 MiB transient parse. 7 roots embedded: roughly 1.3–1.5 GB peak.

## Global Constraints

- **Behaviour-preserving.** Every golden family (`scripts/check-goldens`) and the CDO gate (`CDO_WS=U:/Git/DO-cdo-baseline/Cloud scripts/cdo-gate`) byte-identical. LSP answers (decls, edges, incoming, event edges, custom requests that read `dep_texts`) for a root built on a cache HIT must equal a cache-less build of the same root.
- Share only under the existing `DepKey` (ordered `snap.apps[1..]`, path + pre-read stamp + `has_source`) and a `(path, pre-read stamp)` key for source; never a stat taken after the read; an unstamped app is never shared.
- CLI/aldump/alsem paths keep a throwaway `DepCache::default()` → always a miss → unchanged behaviour (they still parse everything; whole-program resolution needs dependency bodies).
- Edit with Edit/Write tools only; `rustfmt <file>` per file, never `cargo fmt`; clippy clean (`scripts/ci-steps clippy`); every new test has a discrimination proof in its commit message; `df -h /u` before builds.

## Review Focus

1. **A hit must not change any LSP answer.** The surface on a hit is `DeclSurface::build(workspace).with_frozen(cached dep_meta)` instead of `build_split(full)`; pin equality of decls/edges/incoming/event_edges/dep_texts between a hit build and a cache-less build of the same root (Task 3).
2. **Something besides `dep_meta`/`dep_texts`/nodes still reads dependency parse trees in the LSP path** (e.g. `ResolveIndex::build`, event-edge emission spans via `surface.get_with_path`, `build_dep_texts` virtual paths, `ProgramContext.parsed` consumers). Audit every reader of `ctx.parsed` / `parsed` in `lsp::snapshot::from_context` and `build_context_from_snapshot_cached` before skipping the parse; any reader that needs dependency units blocks the skip and must be listed (Task 3 Step 1).
3. **Rung 2 / rung 3** on a hit: rung 2 reuses `cur.dep_layer`/`dep_meta`/`dep_texts` by `Arc` (unchanged); rung 3 rebuilds through the cache — pin that a rung-3 rebuild of a root whose dependency set is unchanged is a hit and answers identically (Task 3).
4. **The first root's build must keep producing the shared entry even if it is later dropped** — entries are weak; a second root after the first root is gone re-parses (correct, just not shared). Pin with a test (Task 2).
5. **`dep_texts` keys embed `AppRef`** (`(AppRef, String)`), so they are only shareable under `DepKey` (same numbering) — never under the per-`.app` source key alone (Task 2).

---

## File Structure

- Modify `src/program/dep_cache.rs` — source map `(path, stamp) → Weak<SourceRoot>`-like entry (Task 1); `DepNodes` gains `dep_meta`/`dep_texts` slots (Task 2).
- Modify `src/snapshot/snapshot.rs` + `src/snapshot/provider.rs` — `EmbeddedAppProvider` gets the source through `DepCache` when a cache is given (Task 1).
- Modify `src/program/resolve/full.rs` (`build_context_from_snapshot_cached`) — on a `DepKey` hit, parse only the workspace unit (Task 3).
- Modify `src/lsp/snapshot.rs` (`from_context`) — on a hit use cached `dep_meta`/`dep_texts` and the frozen-surface construction; on a miss compute them as today and publish them into the cache entry (Tasks 2–3).
- Tests next to each change; measurement with the scratch census probe and `multiroot.ps1` (Task 4).

---

### Task 1: share extracted embedded source per `.app`

**Interfaces:** Produces `DepCache::source(&self, path: &Path, stamp: Option<&AppFileStamp>, load: impl FnOnce() -> Result<Option<SourceRoot>>) -> Result<Option<Arc<SourceRoot>>>` (or the equivalent shape that lets `AppUnit.source` hold shared `Arc<str>` texts — `SourceFile.text` is already `Arc<str>`, so cloning a cached `SourceRoot` shares every text; choose the cheapest representation and document it). `None` stamp → load directly, never cached. `SourceRoot` must stay usable by `parse_snapshot` unchanged.

- [ ] **Step 1: Failing test** (in dep_cache.rs tests, reusing the two-roots fixture with a dependency `.app` that ships `src/*.al`): with one `DepCache`, root A's and root B's dependency `AppUnit.source` file texts are `Arc::ptr_eq`; with a rewritten `.app` (new stamp) they are not; with no cache (`DepCache::default()` per build) they are not.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement: thread the cache into the provider chain in `SnapshotBuilder::build_with_options` (the `EmbeddedAppProvider` is built there with `rd.app_path`; pass `rd.stamp`), route `cached_source` through `DepCache::source`.
- [ ] **Step 4:** Run → PASS; `cargo test -p al-sem --lib` green.
- [ ] **Step 5:** Discrimination proof: bypass the cache in the provider → test FAILS; revert.
- [ ] **Step 6:** Commit `perf(snapshot): share extracted dependency source across roots`.

### Task 2: cache `dep_meta` and `dep_texts` with the shared dependency nodes

**Interfaces:** `DepNodes` gains a write-once slot for the dependency-derived LSP products, e.g. `pub lsp: OnceLock<Arc<DepLspTier>>` with `pub struct DepLspTier { pub dep_meta: Arc<DepMetaMap>, pub dep_texts: Arc<DepTexts> }` (or an equivalent the implementer justifies). `LspSnapshot::from_context`: if the context's `dep_layer.dep_nodes.lsp` is set, use it; else compute `dep_meta`/`dep_texts` as today and `set` them. `DepTexts`' `Arc<str>` values must be the same allocations as the shared `SourceRoot` texts from Task 1 (assert in the test).

- [ ] **Step 1: Failing tests:** (a) two roots, same cache, embedded: `Arc::ptr_eq` on `dep_meta` and on `dep_texts`; a `dep_texts` value is `Arc::ptr_eq` with the corresponding shared `AppUnit.source` text. (b) Review Focus 4: build root A, drop it, build root B with the same cache → B still correct (equal to a cache-less build) and the entry is fresh.
- [ ] **Step 2:** Run → FAIL. **Step 3:** Implement. **Step 4:** Run → PASS; lib + bin tests green.
- [ ] **Step 5:** Discrimination proof: always recompute (ignore the slot) → (a) FAILS; revert.
- [ ] **Step 6:** Commit `perf(lsp): share dep_meta and dep_texts across roots`.

### Task 3: skip parsing dependency source on a cache hit

**Interfaces:** `build_context_from_snapshot_cached` looks up `DepKey::of(&snap)` in the cache *before* parsing (a new non-building lookup, e.g. `DepCache::get(&DepKey) -> Option<Arc<DepNodes>>`, that only returns entries whose `lsp` slot is set). On a hit: `parse_snapshot` on a snapshot view containing only the workspace unit (or a `parse_unit` helper), `DepLayer` built from the cached `DepNodes` without touching dependency parse trees, `ProgramContext.parsed` holds the workspace unit only; `from_context` builds `DeclSurface::build(&graph, &[workspace]).with_frozen(dep_meta)` (rung-1 construction) and skips `build_dep_texts`. On a miss: unchanged.

- [ ] **Step 1: Audit (Review Focus 2).** List every reader of `parsed`/`ctx.parsed` on the LSP path (`build_context_from_snapshot_cached`, `from_context`, `ResolveIndex::build`, `emit_event_flow_edges`, `build_dep_texts`, `DeclSurface::*`). For each: does it read dependency units? Record the list in the report. If any production LSP reader needs dependency units beyond `dep_meta`/`dep_texts`/nodes, STOP and report it (it blocks the skip as designed).
- [ ] **Step 2: Failing tests:** (a) **parity** — for the two-roots fixture (embedded, dependency with source and at least one event publisher/subscriber pair and a call into the dependency), root B built on a hit equals root B built cache-less: `decls_by_file`, `edges_by_file`, `incoming`, `event_edges`, `publisher_fanout`, `decl_by_id`, `dep_texts` keys (compare via stable debug/projection); (b) **no dependency parse on a hit** — expose a test-only counter of parsed units (e.g. a `#[cfg(test)]` atomic incremented in `parse_snapshot`'s per-unit closure) and assert root B parsed exactly the workspace unit; (c) **rung 3 on a hit** — spawn an updater for root B with the shared cache, force rung 3 (as `rung3_rebuild_stays_in_symbols_mode` does), assert the rebuild was a hit (parse counter) and answers equal the cache-less build.
- [ ] **Step 3:** Run → FAIL. **Step 4:** Implement. **Step 5:** Run → PASS; `cargo test -p al-sem --lib`, `cargo test --bin al-call-hierarchy`, and the incremental-parity integration tests (`cargo test --test lsp`) green.
- [ ] **Step 6:** Discrimination proofs: (i) on a hit, still parse everything → (b) FAILS; (ii) on a hit, use an EMPTY `dep_meta` instead of the cached one → (a) FAILS (proves the frozen surface is what answers dependency lookups). Revert each.
- [ ] **Step 7:** Gates: `scripts/check-goldens` (no golden moved), cdo-gate PASS, clippy. Commit `perf(lsp): skip re-parsing dependency source when the dependency tier is shared`.

### Task 4: measure, document, gate

- [ ] **Step 1:** `df -h /u`, `df -h /c`.
- [ ] **Step 2:** Census (one root, embedded, probe waits for background drops): before = the table above; after = this branch. `multiroot.ps1` 1/3/7 roots × embedded/symbols, before = al-sem v1.3.2 release exe (`scratchpad/rel1184/al-call-hierarchy.exe`), after = release-fast build; alternate before/after per cell; note one run per cell. Also record wall time of the 7-root embedded run (CPU saving from skipped parses).
- [ ] **Step 3:** CHANGELOG `[Unreleased]` `### Changed`: what is now shared, the numbers with their caveats, what stays per root.
- [ ] **Step 4:** `scripts/ci-steps all`, `scripts/check-goldens`, cdo-gate — green.
- [ ] **Step 5:** Commit. Remove scratch build dirs (`cargo clean --target-dir U:/lpt-census` when the census is done; `cargo clean --profile release-fast`).

## Out of scope

- The first root's transient parse (~800 MiB): streaming "parse → extract declarations → drop body" per file, or an on-disk summary cache, are separate levers (see the prior-art notes).
- `event_edges` per root (~15.5 MiB embedded): needs a dependency-only/overlay split of publisher edges.
