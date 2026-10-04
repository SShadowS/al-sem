# Compact graph core, step 2: lighter per-root state (2a-2c)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut what each workspace root keeps while idle — the LSP snapshot's route-less event links and the idle updater's whole-graph index — without changing a single LSP answer.

**Architecture:** Three independent, provably-identical reductions (spec §6 revised, 2a-2c): the object map indexes workspace objects only; the LSP snapshot stores only event links with routes; the resolver index stops carrying subscriber maps (moved to the one function that reads them) and, if the measurement favours it, stops carrying a per-routine map that duplicates the sorted routine list. Sharing between roots (2d) is NOT in this plan; it is decided by this plan's final measurement.

**Tech Stack:** Rust 2024; `NodeSet` (merged shared+own sorted sets), `ResolveIndex`, `LspSnapshot`, the census probe (`tools/census-probe`).

**Spec:** `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §6 (revised 2026-10-04). Code map with file:line evidence for every claim below: the controller passes its path to each task (session scratchpad `step2-code-map.md`).

## Global Constraints

- Plain English in docs and comments. Short sentences.
- `rustfmt --edition 2024 <file>` per touched file; never `cargo fmt`. Stage by name; never `git add -A`; never stage `src/engine/deps/mod.rs`.
- Edit/Write tools only — no inline python/sed edits.
- No golden may move. `FULL`/CLI output byte-identical to `master`. Every LSP answer identical to the previous commit.
- Real-unknown rate stays 0 (`scripts/cdo-gate`).
- Per task: `scripts/ci-steps task` rc=0 before the commit. Tasks that touch resolution or LSP state also run `scripts/cdo-gate U:/Git/DO-cdo-baseline/Cloud` (PASS; it includes the CDO LIGHT==FULL test). Never pipe a gate through `| tail`.
- Every new or changed test gets a discrimination proof (break → FAIL, revert → PASS), break asserted as applied, recorded in the commit message.
- `tools/census-probe` is compiled by no gate: run `cargo check --manifest-path tools/census-probe/Cargo.toml` (with `CARGO_TARGET_DIR=C:/lpt`) whenever a task changes a type the probe reads.
- Never delete a fact `FULL`/the program report keeps (spec §2). The LSP snapshot is the light view.
- Git Bash with Windows paths; never `2>nul`. IDE diagnostics in this repo are often stale; trust cargo.

## Review Focus

1. **A workspace routine subscribing to a dependency event** (the two_roots_one_alpackages fixture has one): its incoming list, the publisher's code-lens count and outgoing items must be identical after every task. The mixed link is the case CG never exercises.
2. **A publisher with no subscriber** must still show zero in code lens and must not be flagged differently by the unused-procedure diagnostic after 2b drops its link.
3. **Rung 1 then rung 2 then rung 1 again** on one root (the spawn loop rebuilds `Rung1Context` after rung 2): answers equal a fresh build at every step (`tests/lsp/lsp_incremental_parity.rs` scripts).
4. **Two routines with the same (object, name) — overloads and physical duplicate rows (aliased publishers)** must come back from `routines_in_object` in the same order and multiplicity after 2c's lookup change (dual-publisher skip guard tests in `tests/program_resolve_harness.rs`).
5. **An old snapshot kept alive across a rung-2 swap** keeps answering correctly (event refs still valid).

---

## Task 1: Object map holds workspace objects only (2a)

**Files:** Modify `src/lsp/snapshot.rs` (~521-522), `src/lsp/updater.rs` (~323-328, ~532-536, ~1150-1155), `src/program/resolve/full.rs` (~781-782); add the helper in `src/program/resolve/full.rs` next to `resolve_file_obligations`.

**Interfaces — produces:** `pub(crate) fn workspace_object_map(graph: &ProgramGraph, primary: AppRef) -> HashMap<ObjectNodeId, &ObjectNode>`.

- [ ] **Step 1: the helper, used at all five sites**

```rust
/// The object map `resolve_file_obligations` reads. Its only lookups use ids
/// built from the primary app (`full.rs` resolve_file_obligations), so it holds
/// workspace objects only — dependency objects would be dead weight, retained
/// per root by the idle updater.
pub(crate) fn workspace_object_map(graph: &ProgramGraph, primary: AppRef) -> HashMap<ObjectNodeId, &ObjectNode> {
    graph
        .objects
        .iter()
        .filter(|o| o.id.app == primary)
        .map(|o| (o.id.clone(), o))
        .collect()
}
```

Replace each of the five `graph.objects.iter().map(|o| (o.id.clone(), o)).collect()` builds with `workspace_object_map(&graph, primary_app_ref)` (use the primary `AppRef` each site already has; in the updater it is `self.workspace`'s app via `cur.graph.apps.find(..)` as rung 1 already does — read the site). Before replacing, grep every reader of each map: if any reader looks up a NON-primary id, STOP and report NEEDS_CONTEXT.

- [ ] **Step 2: test (pins the use).** In `src/program/resolve/full.rs` tests: build the multi-app or two_roots fixture context, call `resolve_file_obligations` for a workspace file with the full map and with `workspace_object_map`, assert identical edges; and assert `workspace_object_map(..).len()` equals the workspace object count. Discrimination: filter on `!= primary` → the edges differ (FAIL); revert → PASS.
- [ ] **Step 3: gates + commit.** `scripts/ci-steps task`, `scripts/cdo-gate`. `git commit -m "perf: the resolver's object map holds workspace objects only"`.

---

## Task 2: The LSP snapshot stores only event links with routes (2b)

**Files:** Modify `src/lsp/snapshot.rs` (`from_context` ~582-590; tests ~1121-1144), `src/lsp/updater.rs` (`apply_rung2` ~593-602), `src/program/dep_cache.rs` (`answers()` ~733-778).

- [ ] **Step 1: filter at both production sites**: after `emit_event_flow_edges`, keep only edges with `!edge.routes.is_empty()`, with a comment: "Links without routes have no LSP reader (no incoming ref, no fan-out, no outgoing item); the program report keeps them (spec §2, §6 2b)." Rung 1 forwards `event_edges` unchanged (no change needed).
- [ ] **Step 2: tests.** (a) Change `build_full_edges_match_resolve_full_program` to compare the LSP links with the CLI's links FILTERED to non-empty routes, and add an assertion that the CLI still has the empty ones (they are not lost). (b) New test: a fixture publisher with no subscriber — `publisher_fanout` has no entry, code lens count unchanged from before, the unused-procedure diagnostic result unchanged (Review Focus 2). (c) `answers()` keeps comparing `event_edges` (both sides are LSP snapshots built the same way), plus a new line: a doc comment that it compares answers of the light view. Discrimination for (a)/(b): remove the filter → the new tests' "no empty link stored" assertion FAILS; revert → PASS.
- [ ] **Step 3: parity suites unchanged:** `cargo test --test lsp lsp_incremental_parity::` (fresh vs incremental both filter), `perf_support_smoke` and `perf_bounds` counts (every perf publisher has a subscriber, so their counts must not move — if they do, STOP and report).
- [ ] **Step 4: probe.** `cargo check` the probe; adjust it only if it reads a removed thing (it reads `event_edges` — the count just shrinks).
- [ ] **Step 5: gates + commit.** `ci-steps task`, `cdo-gate`. `git commit -m "perf: the LSP snapshot stores only event links with routes"`.

---

## Task 3: Make the resolver index measurable, then measure (2c, measure first)

**Files:** Modify `src/program/resolve/index.rs` (add a census split); `tools/census-probe/src/main.rs` (new mode `--index-split`); create `docs/2026-10-04-step2-index-census.md`.

**Interfaces — produces:** `#[doc(hidden)] pub fn census_parts(self) -> Vec<(&'static str, Box<dyn std::any::Any + Send>)>` on `ResolveIndex`: one entry per field, by field name, each the field moved out and boxed — so the probe can drop parts one at a time and read the heap delta. No runtime cost; production never calls it. Doc it as census-only.

- [ ] **Step 1:** add `census_parts` (destructure `self`; box every field). Unit test: the returned names are exactly the struct's field names (a new field must be added to the census — the test fails until it is).
- [ ] **Step 2:** probe mode `--index-split`: for each corpus (CG 7 roots and CDO; embedded and symbols), build root 1, then `ResolveIndex::build(&graph)` and `workspace_object_map`, settle, and for each census part drop it and record the live-heap drop and allocation drop. Also record `ResolveIndex` total and the object map, and the same per-root totals across 7 CG roots.
- [ ] **Step 3:** write the report: per-field MiB and allocations, the share of the 57.45 MiB (CG) / 66.3 MiB (CDO) per-root updater cost each explains, and the residual. Conventions header like the step 0 report. Commit (`measure: what the idle updater's resolver index holds, by field`). The controller runs the measurement-auditor on it.
- [ ] **Decision (controller, recorded in the ledger):** Task 5 runs only if `routines_by_obj_name` is at least half of the per-root updater cost on CG embedded; otherwise Task 5 is replaced by a ledger ruling and the next lever is chosen from the table.

---

## Task 4: Subscriber maps leave the resolver index (2c)

Rung 1 never reads `subscribers_map` / `ambiguous_subscriptions` / `orphaned_subscriptions`; only `emit_event_flow_edges` (`resolver.rs` ~3036) does. Move them into their own index built where they are read.

**Files:** Modify `src/program/resolve/index.rs` (split build), `src/program/resolve/resolver.rs` (`emit_event_flow_edges`), callers/tests of `subscribers_of`, `ambiguous_subscriptions`, `orphaned_subscriptions` (grep; known: `abi_ingest.rs` ~1204-1213 tests).

**Interfaces — produces:** `pub struct SubscriberIndex { .. }` in `index.rs` with `pub fn build(graph: &ProgramGraph) -> SubscriberIndex` and the three methods moved verbatim (same signatures, same results); `ResolveIndex` loses the three fields and methods. `emit_event_flow_edges(graph, index, surface)` keeps its signature and builds a `SubscriberIndex` internally.

- [ ] **Step 1:** move the subscriber-building code (`index.rs` ~286-408, including the transient `routine_indices_by_obj_name` it needs) into `SubscriberIndex::build` with no logic change; same sort (`index.rs` ~405-408).
- [ ] **Step 2:** `emit_event_flow_edges` builds `SubscriberIndex::build(graph)` and reads it. Update the tests that call the moved methods.
- [ ] **Step 3: tests.** All event tests listed in the code map §C stay green unchanged (`event_flow_*`, `platform_*_event_subscriber_wires_via_synthetic_publisher`, the dual-publisher harness tests, `lsp_incremental_parity::event_subscriber_attribute_edit_stays_equivalent`). Add a test that `ResolveIndex` built over a fixture with subscribers and `SubscriberIndex::build` over the same graph give the same subscriber lists as before the move (compare against the values captured by a test written FIRST, before the move, and committed in the same commit). Discrimination: sort subscribers in reverse in `SubscriberIndex::build` → the event tests FAIL; revert → PASS.
- [ ] **Step 4: gates + commit.** `ci-steps task`, `check-goldens`, `cdo-gate`. `git commit -m "perf: subscriber maps move out of the resolver index into the one place that reads them"`.

---

## Task 5 (conditional on Task 3): `routines_in_object` reads the sorted routine list (2c)

`routines_by_obj_name` maps `(ObjectNodeId, name_lc)` to routine ids pushed in `graph.routines` order (`index.rs` ~210-217). `graph.routines` is a `NodeSet` sorted by `RoutineNodeId`, whose derived order starts `(object, name_lc, …)` (`node.rs` ~174-178), so every row for one `(object, name_lc)` is one contiguous run in merged order — the same ids, same multiplicity (physical duplicate rows included), same order (ties: shared before own, as `NodeSet` merges). The map is redundant.

**Files:** Modify `src/program/resolve/index.rs` (`routines_in_object`, ~430-435; remove `routines_by_obj_name`), `src/program/node_set.rs` (a range helper if `binary_search_by` cannot express a lower bound), every caller (~26 production sites listed in the code map §B.8 table: `resolver.rs`, `receiver.rs`, `applicability.rs`, `index.rs` internal).

**Interfaces — produces:** `pub fn routines_in_object<'g>(&self, graph: &'g ProgramGraph, obj: &ObjectNodeId, name_lc: &str) -> impl Iterator<Item = &'g RoutineNodeId> + 'g` — or, if `ResolveIndex` already holds a `&ProgramGraph`, keep the graph parameter out. Read the code first and pick the form that touches the fewest call sites; record the choice.

- [ ] **Step 1: write the equality test FIRST** (before changing the lookup): over the multi-app fixture, CDO-free fixtures with overloads and aliased publishers (Review Focus 4), and every `(object, name_lc)` key in `routines_by_obj_name`, record the old lists; commit the test with a helper that the new implementation must match exactly (same ids, order, multiplicity).
- [ ] **Step 2:** implement the run lookup on `NodeSet` (lower bound for `(obj, name_lc)`, take while the key matches, across the merged shared+own sequence); delete the map. Update callers.
- [ ] **Step 3: tests.** The Step 1 test passes; the dual-publisher harness tests, all resolver/receiver tests and `lsp_incremental_parity::` stay green. Discrimination: stop the run one element early → FAIL; revert → PASS.
- [ ] **Step 4: gates + commit.** `ci-steps task`, `check-goldens`, `cdo-gate`. `git commit -m "perf: routines_in_object reads the sorted routine list; drop the per-routine map"`.

---

## Task 6: Measure the result, docs, branch gate

**Files:** `docs/2026-10-04-step0-server-census.md` (append "After step 2"), `CHANGELOG.md` (`## [Unreleased]`), raw runs under `tools/census-probe/runs-step2/`.

- [ ] **Step 1:** `cargo check` the probe; re-run it exactly as step 0/1 did (CG 7 roots + CDO, embedded + symbols, with and without `--with-updaters`). Report before (the after-step-1 numbers) vs after, per root and in total, with the updaters idle. Name which task explains each change; attribute only what the probe can split.
- [ ] **Step 2:** CHANGELOG `### Changed`: the three reductions with measured numbers (corpus, mode, heap-not-RSS). Plain English.
- [ ] **Step 3: the 2d decision input.** One paragraph in the report: idle retained heap at 7 CG roots after step 2, and what is left per root that sharing (2d) could remove. The controller and user decide 2d from it.
- [ ] **Step 4: branch gate.** `scripts/ci-steps all` rc=0 and `scripts/cdo-gate` PASS (logs, grepped).
- [ ] **Step 5: commit** (`docs: step 2 measured — lighter per-root state`). The controller runs the measurement-auditor.
