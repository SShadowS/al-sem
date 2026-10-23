# digest --tests: test coverage of changed routines — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest … --tests` adds a `testCoverage` block saying which changed production routines are reachable from a `[Test]` procedure through the resolved call graph (issue #254).

**Architecture:** A new module `src/engine/l5/test_coverage.rs` with a read-only input `CoverageGraph` built by one adapter from the digest's snapshot + model, and a pure `compute_test_coverage` over it. `digest_cli.rs` builds it before `drop(snap)` when the flag is set and hands the JSON block to `project_digest_document`, which inserts it after the null drop.

**Tech Stack:** Rust, serde / serde_json, clap (derive) in `src/bin/alsem.rs`, cargo-nextest via `scripts/ci-steps`.

**Spec:** `.agent/issue-254/ledger.md`, sections `## Design` and `## Acceptance matrix` (the bounded issue's spec; converged after 4 spec-panel rounds). Every rule below is stated there; this plan only fixes names, files and order.

## Global Constraints

- Flag spelling `--tests` (the issue's own). Without it the digest document is byte-identical to today; `schemaVersion` stays `1.6.0`; add one history-comment line naming `payload.testCoverage` at `src/engine/l5/digest_cli.rs:1407-1413`.
- Do not edit `src/engine/l5/digest.rs` effect/witness code, `ordering_engine.rs`, `fingerprint_query.rs` (another session, #160). Keep the `digest_cli.rs` diff to a few lines.
- Only the adapter `CoverageGraph::from_snapshot` reads the snapshot / model shape.
- Determinism: no `HashMap`/`HashSet` iteration order reaches the output; use `BTreeMap`/`BTreeSet` or sort.
- `rustfmt <file>` only; protected paths `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/` untouched; `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
- Every new or changed test gets a discrimination proof (break, fail, revert, pass) recorded in the ledger's proof table.

## Review Focus

1. A workspace with tests but a changed set that is ALL test code → `changedRoutines: 0`, `ratio: null` present, `covered`/`uncovered` empty (T2 CLI test; T1 unit `ratio_null_when_no_production_changes`).
2. A test that calls itself or two tests in a mutual cycle → terminates, each counted once (T1 unit `cycle_terminates`).
3. The same stable id appearing in two digest entries (collided ids) → counted once (T1 unit `duplicate_changed_ids_counted_once`).
4. A routine that is both a test root AND in a non-Test codeunit (a `[Test]` attribute outside a test codeunit) → still a root, excluded from the count (T1 unit `test_attribute_outside_test_codeunit_is_root`).
5. `--tests` with `--format` human (no JSON) → one summary line, no panic (T2 CLI `human_line_with_tests`).

---

### Task 1: the coverage core (`test_coverage.rs`)

**Files:**
- Create: `src/engine/l5/test_coverage.rs`
- Modify: `src/engine/l5/mod.rs` (add `pub mod test_coverage;`)

**Interfaces:**
- Produces (used by Task 2):
  - `pub struct CoverageGraph` — fields (all `pub(crate)` so unit tests build it by hand): `roots: BTreeSet<String>` (test stable ids), `test_code: BTreeSet<String>` (roots + every routine of a primary-app `Subtype = Test` codeunit), `edges: BTreeMap<String, Vec<(String, bool)>>` (from → sorted `(to, is_event)`), `incomplete: BTreeMap<String, BTreeSet<&'static str>>` (routine → reason codes of the spec's "incomplete" bucket), `boundary: BTreeMap<String, BTreeSet<&'static str>>` (routine → dependency-boundary reason codes), `uncovered_reason_facts: BTreeMap<String, UncoveredReason>` (subscriber/declaration-only facts; absent = `no-path`).
  - `pub enum UncoveredReason { DeclarationOnly, PlatformEventSubscriber, DependencyEventSubscriber, NoPath }` serialising as the spec's kebab-case words. `DependencyEventSubscriber` is only reported when no test reaches the publisher, so it is decided in `compute_test_coverage`, not in the adapter: the adapter records `DependencyEvent { publisher: String }` facts and the core checks reachability. Shape it as you see fit, but keep the four output words.
  - `pub struct ChangedRoutine { pub stable_id: String, pub display: String, pub object_display: String, pub anchor: Option<(String, u32)>, pub declaration_only: bool }`
  - `pub struct TestCoverage` (serde `Serialize`, `rename_all = "camelCase"`): `changed_routines: usize`, `covered_changed_routines: usize`, `ratio: Option<f64>`, `covered: Vec<CoveredItem>`, `uncovered: Vec<UncoveredItem>`, `test_procedures_considered: usize`, `excluded_test_code: usize`, `coverage: CoverageStatus`. `CoveredItem { stable_id, display, reached_from: Vec<String> /* exactly one */, via_event: bool }`; `UncoveredItem { stable_id, display, object_display, reason: UncoveredReason, anchor: Option<Anchor{file, line}> /* skip if None */ }`; `CoverageStatus { status: &'static str /* "complete" | "partial" */, reasons: Vec<&'static str> /* sorted distinct */, tests_with_unresolved_traversal: usize, tests_calling_dependency_code: usize }`.
  - `pub fn compute_test_coverage(g: &CoverageGraph, changed: &[ChangedRoutine]) -> TestCoverage`
  - `impl CoverageGraph { pub fn from_snapshot(snap: &CapabilitySnapshot, resolved: &Model) -> Self }`
  - `impl TestCoverage { pub fn to_json_value(&self) -> serde_json::Value }` — `ratio` emitted as JSON `null` when `None` (do NOT drop it).

- [ ] **Step 1: Write the failing unit tests** in `#[cfg(test)] mod tests` of `test_coverage.rs`, each building a `CoverageGraph` by hand. Names and assertions:
  - `direct_call_is_covered` — root T → A; A covered, `reached_from == [T]`, `via_event == false`.
  - `two_hop_is_covered` — T → A → B; B covered.
  - `subscriber_via_publisher_is_covered` — T → Raiser → Pub, Pub →(event) Sub; Sub covered, `via_event == true`.
  - `equal_length_event_free_path_wins` — two 2-hop paths to X, one through an event edge; `via_event == false`.
  - `shorter_event_path_sets_via_event` — event path length 1, plain path length 2; `via_event == true`.
  - `lowest_id_witness_wins_over_shorter_path` — tests "t:a" (3 hops) and "t:b" (1 hop) reach X; `reached_from == ["t:a"]`.
  - `cycle_terminates` — T → A → T and A → A; terminates, A covered once.
  - `shuffled_adjacency_same_output` — same graph built with edge vectors in two orders → equal `TestCoverage` (compare serialised JSON).
  - `unresolved_row_flips_status` — complete graph, then one `incomplete` reason on a reached routine → `status` `"partial"`, reason listed, `tests_with_unresolved_traversal == 1`; without it `"complete"` and `reasons` empty.
  - `incomplete_on_unreached_routine_does_not_flip` — the reason on a routine no test reaches → `"complete"`.
  - `dependency_boundary_counted_apart` — `boundary["X"] = {"dependency-code"}` on a reached X → `partial`, `tests_calling_dependency_code == 1`, `tests_with_unresolved_traversal == 0`.
  - `test_code_excluded_from_denominator` — changed = [T, Helper(in test_code), A] → `changed_routines == 1`, `excluded_test_code == 2`.
  - `test_attribute_outside_test_codeunit_is_root` — a root that is not in a test codeunit is still excluded and still reaches.
  - `declaration_only_stays_uncovered` — changed entry with `declaration_only: true` → in `uncovered`, reason `declaration-only`, counted.
  - `platform_and_dependency_subscriber_reasons` — facts give `platform-event-subscriber`; a dependency-event subscriber whose publisher no test reaches → `dependency-event-subscriber`; same subscriber when a test reaches the publisher → covered.
  - `ratio_null_when_no_production_changes` — only test code changed → `ratio == None`, and `to_json_value()["ratio"].is_null()` with the key present.
  - `partition_is_exact_disjoint_sorted` — covered ∪ uncovered ids == distinct non-test changed ids, no overlap, each list sorted.
  - `duplicate_changed_ids_counted_once` — two `ChangedRoutine` with one stable id → counted once.
  - `no_tests_all_uncovered` — empty roots, two changed → `test_procedures_considered == 0`, both uncovered `no-path`, `ratio == Some(0.0)`.

- [ ] **Step 2: Run** `cargo test -p al-sem --lib test_coverage::` — expected: compile errors / FAIL (types missing).

- [ ] **Step 3: Implement the types and `compute_test_coverage`.** Search per spec §Search: for each root in ascending order, BFS over `(routine, crossed_event: bool)` states with the sorted adjacency; a routine's witness is the first root that reaches it; `via_event = dist_no_event(witness, n) > dist(witness, n)` (unreachable without an event = true). Per root, union `incomplete` / `boundary` reasons of every routine it reaches (itself included) for the counters and `reasons`.

- [ ] **Step 4: Implement `CoverageGraph::from_snapshot`** following spec §Test roots, §Changed routines (test codeunits), §Followed edges (typed edges with a routine `to`; plus `[HandlerFunctions]` edges, reading the rule in `src/program/impact_graph/facts.rs:210`), §Incomplete traversal (the status → bucket table over `snap.callsite_resolutions`; `object-run-unresolved` typed edges; TestPage/TestRequestPage params, locals and object globals; coverage-record `parse-incomplete` / missing record / `opaque-dependency`; non-primary routine → `dependency-code`), and §Why `uncovered` (from `resolved.events`). Use `primary_guid_of(resolved)` (`digest_cli.rs:364`) / `Model::routine_role` for primary-app membership. Add adapter unit tests that use `crate::engine::l5::test_support` builders if they fit; otherwise the CLI tests of Task 2 are the adapter's proof — say which in the ledger.

- [ ] **Step 5: Run** `cargo test -p al-sem --lib test_coverage::` → PASS; then `scripts/ci-steps task-affected`.

- [ ] **Step 6: Discrimination proofs** for at least: `equal_length_event_free_path_wins` (flip the tie rule), `lowest_id_witness_wins_over_shorter_path` (iterate roots descending), `unresolved_row_flips_status` (ignore `incomplete`), `ratio_null_when_no_production_changes` (emit `Some(0.0)`), `test_code_excluded_from_denominator` (drop the test-code filter). Break with Edit, run, revert with Edit, run; record each in the ledger's proof table.

- [ ] **Step 7: Commit** (`check-diff` first, per the issue pipeline): `git add src/engine/l5/test_coverage.rs src/engine/l5/mod.rs` — message `digest: test-coverage core (#254)`. The CLI acceptance tests stay uncommitted until Task 2 (the pre-commit hook runs the suite).

### Task 2: wire `--tests` into `alsem digest`

**Files:**
- Modify: `src/bin/alsem.rs` (digest args: `#[arg(long)] tests: bool`, help "Add a testCoverage block: which changed routines a [Test] procedure reaches"), the call to `run_digest_pipeline`.
- Modify: `src/engine/l5/digest_cli.rs` (`run_digest_pipeline` / `DigestRequest` gain `tests: bool`; in `digest_pipeline_with`, before `drop(snap)`, when `tests`: build `ChangedRoutine`s from `entries` (stable id, display, object display, `routine_anchor`, declaration-only flag), `CoverageGraph::from_snapshot(&snap, resolved)`, `compute_test_coverage`; pass `Option<serde_json::Value>` to `project_digest_document`; human text gets the spec's one line; the schemaVersion history comment line).
- Modify: `src/engine/gate/format_json.rs` — `pub fn serialize_normalized_value(v: &serde_json::Value) -> String` (pretty-print an already-normalised value, same formatting as `serialize_document_value`), used by `project_digest_document` after inserting the block into the payload of the `sort_and_drop_nulls`'d document.
- Every other caller of `run_digest_pipeline` / `project_digest_document` (grep them: tests, `aldump`) passes `false` / `None`.
- Test: `tests/cli/cli_digest_test_coverage.rs` (already written in step 6 of the issue pipeline; fixtures `tests/fixtures/digest-test-coverage/`, `tests/fixtures/digest-test-coverage-no-tests/`) — plus `human_line_with_tests` from Review Focus 5 if it is not there.
- Modify: `CHANGELOG.md` `## [Unreleased]` → `### Added`: one entry for `digest --tests` (block shape, the rules in one sentence each, the stated limits: dependency-internal raises, platform events, TestPage).

**Interfaces:**
- Consumes: Task 1's `CoverageGraph::from_snapshot`, `compute_test_coverage`, `ChangedRoutine`, `TestCoverage::to_json_value`.

- [ ] **Step 1:** Run `cargo test --test cli cli_digest_test_coverage::` — expected FAIL (clap rejects `--tests`).
- [ ] **Step 2:** Implement the wiring above.
- [ ] **Step 3:** Run `cargo test --test cli cli_digest_test_coverage::` → PASS; `cargo test --test cli cli_b_digest_` (existing digest goldens) → PASS unchanged; `scripts/ci-steps task-affected`.
- [ ] **Step 4:** Discrimination proofs for the CLI tests whose wiring could silently break: drop the block insertion (key absent) → the `--tests` tests fail; insert the block before the null drop → the `ratio: null` test fails; pass `tests` always true → the "no flag, no key" test fails. Record in the ledger.
- [ ] **Step 5:** Commit (`check-diff` first): the source files, `tests/cli/main.rs`, `tests/cli/cli_digest_test_coverage.rs`, the two fixture dirs, `CHANGELOG.md` — message `digest: --tests reports test coverage of changed routines (#254)`.
