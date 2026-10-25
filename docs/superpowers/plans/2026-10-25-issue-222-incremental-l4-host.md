# #222 incremental L4 host behind `--l4-host`: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An `IncrementalHost` that re-solves only the SCCs an edit can reach and seeds the rest from the last revision, so its L4 result equals a fresh one. It runs on a worker thread off `on_swap`, behind the hidden `--l4-host` flag, and publishes nothing to the editor.

**Architecture:** The seeded entry `compute_summaries_v2_bundle_seeded` adds one branch to today's SCC loop (`summary_runner.rs:1567`), seed or re-solve, and computes the row digests and Δ itself. The host (`src/engine/incremental.rs`) drives the step-12 builder and a `View` filled with `rel` and `l4` only. The worker (`src/lsp/l4_host.rs`) runs the host, with one lock over the request counter and the status. `server.rs` calls `request()` from `on_swap`.

**Tech Stack:** Rust; cargo-nextest; sha2 (already a dependency).

**Spec:** `docs/superpowers/specs/2026-10-25-issue-222-incremental-l4-host-design.md` at `f6ac24e4` (converged after 5 panel rounds, 43 register entries accepted). Every "§" below is that spec. Probes: `.agent/issue-222/ledger.md` `## Probes`.

## Global Constraints

- Every command keeps `control: None`. The default solve, every golden, every `--lib` outcome and every CLI byte stay unchanged (§5.7, I2).
- The host fills `rel` and `l4` only: no `det`, no cone, no runs (§2).
- No editor output (Q1). The host publishes to `HostStatus` and one `debug!` line, `l4-host: published generation <g>: <report>`.
- The universe is `ViewUniverse::CrossApp` under the flag (OQ1). Tests drive both universes through the library.
- R0 host half: universe, closure digest (`semantic_golden::dependency_closure_digest`, read before the build), `app.json` bytes, model-instance id. R0 solve half: `FieldIndex` sorted, `collided_site_callee` sorted. Never `driver_version` (§4.3).
- A row digest is sha256 of the `Debug` text of every model row of the id in model order, its `edges_by_from` list, its uncertainty edges, its call sites' upgraded bindings, its leaf summary, its fixed-role flag, and one shared-event bit per `event-dispatch` edge. It is computed inside the solve (§4.3, O4).
- Only `is_recomputed` members are seeded. Fixed leaves and ids with no model row are never touched (§4.4).
- Labels are `Object.[Member.]Routine(var T1; T2): R` (§4.1).
- Edit/Write only, including breaks and reverts. `rustfmt <file>` only, never `cargo fmt`. Stage only intended paths.
- Protected paths, never touched: `.github/` `scripts/` `.claude/` `CLAUDE.md` `.gitignore` `Cargo.toml` version fields, `tree-sitter-al`, `.agent/`.
- Non-speed (Decision 58): no bench and no CDO run in any task. The conductor runs the branch gate.
- Environment: `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
- Every task's gate is `scripts/ci-steps task-affected`, run to a log file and grepped (never piped through `tail`).
- Discrimination proofs: make each break with Edit, watch the named assertion fail, revert with Edit, watch it pass, and record `test | break | failed assertion | reverted green` in the task's commit body. A scripted break is never used. A break the spec marks **redundancy** is run as its stated combined break, and the single-rule run is recorded as "green, redundant (spec §5.2)".

## Review Focus

1. **A refresh whose build fails after a good baseline** (an `app.json` without an `id`, or an unreadable `.alpackages`) must return `RefreshError::Build`, publish nothing, and keep the baseline, so the next good refresh is Incremental. Test `build_error_keeps_baseline` in Task 3.
2. **Dropping the last `HostHandle`** must end the worker thread, with no leak and no hang. Test `worker_exits_when_handles_drop` in Task 7.
3. **A refresh with no routine at all** (the model build refuses an empty model, `workspace.rs:1934-1937`) is a `Build` error, not a panic. Test `empty_workspace_is_build_error` in Task 3.
4. **Two refreshes with no edit between them** must re-solve nothing and keep the projection byte-equal. This is T1m, with the same projection compared twice.
5. **`take_l4` and the cell rules:** after `take_l4`, `View::l4()` panics as a released cell does, and a second `prepare_l4_seeded` on the same view panics. Test `take_l4_releases_the_cell` in Task 2.

---

### Task 1: The seeded solve and the P3 gate (T0a, T2, T2b; C1-C4)

**Files:**
- Modify: `src/engine/l4/summary_runner.rs` (new types and entry; one loop branch at `:1567`)
- Modify: `src/engine/l4/db_effect_solver.rs:1651-1692` (factor the row seeder out of `seed_fixed_leaf_rows`)
- Modify: `src/engine/l4/analysis.rs:43-124` (`L4Request.control`, `build_l4_controlled`)
- Create: `src/engine/l4/summary_runner_seeded_tests.rs`, included as `#[cfg(test)] #[path] mod` from `summary_runner.rs`

**Interfaces (produces):**
```rust
// summary_runner.rs
pub struct Cancelled;
pub(crate) struct L4Control<'a> { pub cancel: &'a dyn Fn() -> bool, pub reuse: Option<L4Reuse<'a>> }
pub(crate) struct L4Reuse<'a> {
    pub solve: &'a SolveBaseline, pub bundle: &'a SummaryBundle,
    pub core: &'a HashMap<String, RoutineSummary>,
}
pub(crate) struct SolveBaseline {
    pub solve_global: [u8; 32], pub rows: HashMap<String, [u8; 32]>,
    pub diag_of: HashMap<Vec<String>, Option<SummarizeDiagnostic>>,
}
pub(crate) struct SolveOutcome {
    pub baseline: SolveBaseline,
    pub resolved: Vec<Vec<String>>,   // keys of re-solved SCCs, solve order
    pub seeded: usize, pub total: usize,
    pub full: bool,                   // reuse None, or the solve half of R0 differed
}
pub(crate) fn compute_summaries_v2_bundle_seeded(
    routines: &[ModelRoutine], graph: &PrimaryGraph, scc: &SccResult,
    upgraded_bindings: &HashMap<String, Vec<UpgradedBinding>>, fields: &FieldIndex,
    leaf_summaries: &HashMap<String, RoutineSummary>, fixed_roles: &HashSet<String>,
    control: &L4Control<'_>,
) -> Result<(SummaryBundle, HashMap<String, RoutineSummary>, Vec<SummarizeDiagnostic>, SolveOutcome), Cancelled>;
// db_effect_solver.rs
pub(crate) fn seed_row(ix: SolveIx, effects: impl Iterator<Item = (EffectIdentity, ViaRank)>,
    universe: &mut GrowingEffectUniverse, bundle: &mut SummaryBundleBuilder);
// analysis.rs
pub(crate) struct L4Request<'a> { pub extra_diagnostics: Vec<SummarizeDiagnostic>, pub control: Option<L4Control<'a>> }
pub(crate) fn build_l4_controlled(model: &Model, rel: &ExecRelation, req: L4Request<'_>,
    cross: Option<&CrossAppInputs<'_>>) -> Result<(L4Analysis, CoreSummaries, SolveOutcome), Cancelled>;
```
`build_l4` keeps its signature, asserts `req.control.is_none()`, and shares the body. Every existing `L4Request { .. }` literal gains `control: None`.

- [ ] **Step 1: Write the curated fixtures and the tests (red).** In `summary_runner_seeded_tests.rs`, build models with the existing `model(src)` helper pattern (`summary_runner.rs` `issue107_tests`), and write the four fixtures of §5.3:
  - C1: a recursive `{P,Q}` whose roles need more than one round, solved under `with_roles_iteration_cap(1, ..)`;
  - C2: `S(var R: Record Cust)` with a `Zeta` insert written before its `Cust` insert, and `C` calling `S` with a `temporary` record;
  - C3: hand-stated `leaf_summaries` and `fixed_roles` maps;
  - C4: a multi-member SCC with a db effect on each member.

  Each fixture asserts its §5.3 precondition before any comparison. Then write the tests:
  - `t0a_seeded_entry_without_reuse_equals_today` over every r0 corpus fixture (`tests/r0-corpus/*`, unrestricted) plus C1-C4;
  - `t2_all_seeded_equals_fresh`;
  - `t2b_mixed_equals_fresh_for_every_k`, which deletes `k`'s members' digests from the baseline. On C2 with `k = {C}` it asserts `S` seeded, `C` re-solved, and at least one identity whose `EffectId` differs between the mixed and fresh bundles.

  The projection is §4.7's: per id the core summary plus owned `db_effects`; the `terminal_base` partition as id groups; diagnostics, warnings, the SCC list and flags. Put the helper `l4_projection(&SummaryBundle, &HashMap<String, RoutineSummary>, &[SummarizeDiagnostic], &SccResult)` in the test module; Task 3 lifts it to `pub`.
- [ ] **Step 2: Run red.** `cargo test -p al-sem --lib summary_runner_seeded` fails to compile (no entry yet).
- [ ] **Step 3: Implement.**
  - Factor `seed_row` out of `seed_fixed_leaf_rows`, byte-neutral for the leaf path.
  - Add the types and `compute_summaries_v2_bundle_seeded` per §4.3 R0-R3 and §4.4:
    - the prologue (`:1371-1565`) is unchanged;
    - after it come the digests, Δ and the solve-global;
    - in the loop: `cancel()` at the top of every turn, then the R2/R3 decision. Seed only `is_recomputed` members, `expect` their baseline entry with the id in the message (O22), and re-push `diag_of[key]` in place. A re-solved SCC records whether it came out different: new member, core summary differs, or the `(op, table_id, operation_id, temp, via)` sets differ, with no rvid (O18). Record `diag_of` for every SCC.
  - Then add `L4Request.control` and `build_l4_controlled`.
- [ ] **Step 4: Run green.** The same filter passes, and `cargo test -p al-sem --lib l4::` stays green.
- [ ] **Step 5: Discrimination proofs.** Record each.
  - T0a: in the seeded entry, skip the diagnostic push of a re-solved SCC → C1 fails.
  - T2: B12 (`seed_row` pushes terminal effects only) → C2 fails.
  - T2b: push the seeded rows after the loop instead of in the branch → T2b fails on C2 with `k = {C}`, while T2 stays green. Record both outcomes; this pair is the proof.
- [ ] **Step 6: Gate** `scripts/ci-steps task-affected`, then commit: `#222 task 1: seeded L4 solve (P3 gate: T0a, T2, T2b)`.

**If T2 or T2b fails (the P3 gate):** stop and record the failing fixture and `k` in the ledger. Then F1 fires (§9, row F1):
- **F1(a):** seed only `v2_map` and run `solve_scc_db_effects` for EVERY SCC. Its gates are its own, not §5.3's:
  - KEPT: T0a, projection equality in T2/T2b (corpus, C1-C4), C1's capped diagnostics, and mixed core-summary reuse.
  - DROPPED: C2's differing `EffectId`, B12, and the "rows after the loop" break.
  - ADDED: (1) the set of ids given a row by `solve_scc_db_effects` equals the fresh solve's; (2) curated C5. In C5, `C(var R: Record Cust)` passes `R` as a `var` argument to `S(var R: Record Cust)`, whose body is `R.Reset();`, with `k = {C}`. Its precondition: `S`'s and `C`'s fresh `resets_filters_on_param` are `Yes`, with no db effect (`summary_runner.rs:498`, `:672`, `:795-851`, `:942-943`, `:464-483`, `:557-559`). Its break: skip inserting the reused core summary into `v2_map` → `C` becomes `Unknown`.
  - Later tasks then drop T1m's B12 and keep everything else.
- **F1(b):** revision-level reuse only. It drops acceptance A6-A8, so it **needs the panel's approval**: stop the claim, report to the conductor, and build no code on it before approval.

### Task 2: The `View` seam: `prepare_l4_seeded` and `take_l4`

**Files:**
- Modify: `src/engine/session.rs` (two methods beside `take_run`, `:1183`)

**Interfaces:**
- Consumes: `L4Control`, `build_l4_controlled`, `SolveOutcome`, `Cancelled` (Task 1).
- Produces:
  - `pub(crate) fn prepare_l4_seeded(&mut self, control: L4Control<'_>) -> Result<(CoreSummaries, SolveOutcome), Cancelled>`. It fills `rel` exactly as `prepare` does (`:801-809`), then `l4` through `build_l4_controlled`, and asserts `!in_detector_loop()` and that `L4` was never filled or released.
  - `pub(crate) fn take_l4(&mut self) -> L4Analysis`. It moves the cell out and runs `note_release(Cell::L4)`.

- [ ] **Step 1: Tests (red), in `session.rs` tests:**
  - `prepare_l4_seeded_without_reuse_equals_query_plan`: on an r0 fixture, a view with `prepare(&query_plan())` and a view with `prepare_l4_seeded` (reuse None, never-cancel) give equal bundle projections, diagnostics and SCC.
  - `take_l4_releases_the_cell` (Review Focus 5): after `take_l4`, `l4()` panics with the released-cell message, and a second `prepare_l4_seeded` panics.
- [ ] **Step 2: Run red**, then implement, then run green with `cargo test -p al-sem --lib session::`.
- [ ] **Step 3: Breaks.**
  - `prepare_l4_seeded` skips `rel` and builds a fresh relation → the equality test still passes; record it as redundant (same input).
  - Instead, break the SCC source: build `l4` over `ExecRelation::single_app` on a cross-app target → the cross-app fixture's equality fails.
  - Drop `note_release` in `take_l4` → the second-prepare panic is missing; the test fails.
- [ ] **Step 4: Gate** `scripts/ci-steps task-affected`; commit `#222 task 2: View::prepare_l4_seeded, take_l4`.

### Task 3: `IncrementalHost` and the oracle harness (T0b, T1a, T1b)

**Files:**
- Create: `src/engine/incremental.rs` (and `pub mod incremental;` in `src/engine/mod.rs`)
- Create: `tests/lsp/l4_host_oracle.rs`
- Modify: `tests/lsp/main.rs` (`mod l4_host_oracle;`)

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces, exactly as §4.1:
  - `pub struct IncrementalHost`;
  - `pub trait Refresher { fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError>; fn commit(&mut self); }`;
  - `IncrementalHost::new(&Path, ViewUniverse)`, `refresh_committed(&mut self, &dyn Fn() -> bool)`, `projection(&self) -> Option<L4Projection>`;
  - `#[doc(hidden)] pub fn baseline_fingerprint(&self) -> Option<[u8; 32]>` and `#[doc(hidden)] pub const PHASE_CHECKS: usize = 3`;
  - `pub enum RefreshError { Cancelled, Build(String) }`, `pub enum RefreshKind { Full(FullReason), Incremental }`, `pub enum FullReason { NoBaseline, HostGlobal, SolveGlobal }`;
  - `pub struct RefreshReport { kind, resolved: BTreeSet<Vec<String>>, seeded, total }`, with labels from the model before the view drops;
  - `pub struct L4Projection` (`Debug + PartialEq`, §4.7), lifted from Task 1's helper.

  `refresh` follows §4.1 steps 1-6 and checks `cancel` after the inputs, the target and the relation. `refresh` stages the result; `commit` moves `staged` into `last`.

- [ ] **Step 1: Tests (red)** in `l4_host_oracle.rs`. The harness function `case(r0: &[(&str, &str)], edit: impl Fn(&Path), want_kind, want_resolved: &[&[&str]])` follows §5.1: write the files, refresh committed, edit with an asserted single replacement, refresh committed, make a fresh host and `refresh_committed(&never)`, then assert the oracle, the kind and the minimality. Every fixture has one codeunit per file plus an unrelated `U`. Tests:
  - `t0b_fresh_host_equals_query_plan`;
  - `t1a_body`;
  - `t1b_body_no_l4_change`;
  - `build_error_keeps_baseline` (Review Focus 1): r0 good, then remove `app.json`'s `id`, giving `Err(Build)` and an unchanged projection; restore it, and the refresh is Incremental with nothing re-solved;
  - `empty_workspace_is_build_error` (Review Focus 3).
- [ ] **Step 2: Red, then implement, then green.** `cargo test --test lsp l4_host_oracle::`.
- [ ] **Step 3: Breaks (§5.1, §5.2).**
  - T0b: build the target with the other universe → cross-app differs. (T0b's cross-app half lands in Task 5; until then record it as pending.)
  - T1a, B1: drop R3's successor-came-out-different clause → oracle fails.
  - T1b, B2: always "different" → minimality fails.
  - Build-error test: replace `last` on `Err` → the second assertion fails.
- [ ] **Step 4: Gate** `scripts/ci-steps task-affected`; commit `#222 task 3: IncrementalHost and the oracle harness`.

### Task 4: Oracle cases, structure and rows (T1c-T1h, T1k-T1o, T1q, T1r)

**Files:**
- Modify: `tests/lsp/l4_host_oracle.rs`

**Interfaces:** Consumes Task 3's `case` harness. Produces no new API.

- [ ] **Step 1: Write each case exactly as its §5.2 row**, with its preconditions asserted first on the fresh hosts:
  - `t1c_signature`;
  - `t1d_edge_removal`;
  - `t1e_scc_split` (`{A,B,C}`, delete `C→A`; `{B,C}` equal at r0 and r1);
  - `t1f_scc_merge`;
  - `t1g_event_param_rename`;
  - `t1h_file_add`;
  - `t1k_table_field`;
  - `t1l_roots_config`;
  - `t1m_no_edit`: a PD effect in `L`; it also compares two consecutive no-edit projections (Review Focus 4);
  - `t1n_shared_event_bit`: `P1`'s row text, `edges_by_from` and `S`'s row are equal;
  - `t1o_collided_rows`: an `#if A`/`#else` pair, the first row edited on the same line, the last row's `Debug` text equal;
  - `t1q_app_json_only`;
  - `t1r_core_summary_half`: `L` gains `R.Reset();`; `L`'s, `B`'s and `A`'s `resets_filters_on_param` are `No` at r0 and `Yes` at r1; all db effects equal.
  Each single-app case runs in both universes.
- [ ] **Step 2: Run** `cargo test --test lsp l4_host_oracle::`. Each case must pass on Task 1-3's code. A failure is a design bug: stop, go to F2/F4 (§9), and record it in the ledger.
- [ ] **Step 3: Breaks, per row:**
  - T1c: the combined break (drop R2's Δ conditions, keep the key; R3 treats a no-baseline member as equal);
  - T1d: B3; the edges-only removal recorded as redundant;
  - T1e: B5;
  - T1f: R2 always true;
  - T1g: B6;
  - T1h: B7, kind only;
  - T1k: B10;
  - T1l: B11;
  - T1m: B12;
  - T1n: B13;
  - T1o: B14;
  - T1q: B15, kind only;
  - T1r: B16.
- [ ] **Step 4: Gate** `scripts/ci-steps task-affected`; commit `#222 task 4: oracle cases T1c-T1r`.

### Task 5: Cross-app cases (T1i, T1j; T0b cross-app half)

**Files:**
- Modify: `tests/lsp/main.rs`: add `#[path = "../common/symbol_app.rs"] mod symbol_app;` and `#[path = "../common/dep_dispatch_fixture.rs"] mod dep_dispatch_fixture;`, as `tests/cli/main.rs:12-22`.
- Modify: `tests/common/dep_dispatch_fixture.rs`: make `write_dependency_lib` `pub` (`:128`, needed by T1j(b)). Nothing else.
- Modify: `tests/lsp/l4_host_oracle.rs`

- [ ] **Step 1: Write `t1i_demand` per §5.2.**
  - Rewrite the workspace: `CallChain` alone in `Ws Chain` (`src/Chain.al`) calls `Lib.Inner();`; `Work.al` is `WS_AL` without `CallChain`. Assert each replacement applied once.
  - Preconditions: the r1 model has no `Inner` row; `CallChain`'s fresh projection differs between r0 and r1.
- [ ] **Step 2: Write `t1j_dependency_change`:** (a) `write_dependency(.., 1)`; (b) `Inner` `Insert` → `Modify` through `write_dependency_lib`. Both must be `Full(HostGlobal)` and pass the oracle.
- [ ] **Step 3: Add the cross-app fixtures to `t0b`.** Run `cargo test --test lsp l4_host_oracle::` green. Make sure no other `tests/cli` user of `dep_dispatch_fixture` breaks: `cargo test --test cli dep` compiles.
- [ ] **Step 4: Breaks.**
  - T1i: drop R2's "no member in Δ" → r1 oracle fails. Record the demand redundancy (§5.2).
  - T1j: B9 → kind fails in (a) and (b).
  - T0b: the other-universe break, now red on these fixtures.
- [ ] **Step 5: Gate** `scripts/ci-steps task-affected`; commit `#222 task 5: cross-app oracle cases`.

### Task 6: Cancellation keeps the baseline (T3)

**Files:**
- Modify: `tests/lsp/l4_host_oracle.rs`

- [ ] **Step 1: Write `t3_cancel_keeps_baseline` per §5.4** (all calls `refresh_committed`). Use fixture T1a.
  - Cancel at call `PHASE_CHECKS + report_r0.total`.
  - Assert `Err(Cancelled)`, an unchanged `projection()` and `baseline_fingerprint()`, and the exact call count.
  - The recovery refresh is Incremental with {L},{B},{A}, and the oracle holds.
- [ ] **Step 2: Run green** (Tasks 1 and 3 already cancel).
- [ ] **Step 3: Break** (O14): `self.last.take()` at the start of `refresh`, restored only on success → `projection()` is `None`; the first assertion fails.
- [ ] **Step 4: Gate**; commit `#222 task 6: T3 cancellation`.

### Task 7: The worker (T4a-T4c)

**Files:**
- Create: `src/lsp/l4_host.rs` (and `pub mod l4_host;` in `src/lsp/mod.rs`)

**Interfaces (produces, §4.2):**
```rust
pub(crate) struct Shared { requested: u64, status: Option<HostStatus> }
pub struct HostHandle { /* Arc<Mutex<Shared>>, mpsc::Sender<()> */ }
impl HostHandle { pub fn request(&self); pub fn status(&self) -> Option<HostStatus>; }
impl Clone for HostHandle;
#[derive(Clone, Debug)] pub struct HostStatus { pub generation: u64, pub report: RefreshReport }
pub(crate) fn run_worker(shared: Arc<Mutex<Shared>>, wake: mpsc::Receiver<()>,
    host: impl Refresher, after_drain: impl Fn());
pub fn spawn_l4_host(root: PathBuf, universe: ViewUniverse) -> (HostHandle, JoinHandle<()>);
```
The loop:
1. `recv`, then drain, then `after_drain()`, then read `g` under the lock.
2. `refresh(&|| requested != g)`.
3. On `Ok`, `commit()` unconditionally, then `lock; if requested == g { status = Some } ; unlock` and the `debug!` line.
4. On `Err(Build)`, `debug!`. On `Err(Cancelled)`, nothing.

`spawn_l4_host` calls `request()` once. `RefreshReport` derives `Clone, Debug`.

- [ ] **Step 1: Tests (red)** in `l4_host.rs` tests. Use a test `Refresher` with barriers; no fixed sleeps; every wait has a timeout.
  - `t4a_stale_complete_unpublished_retained`: wraps a real `IncrementalHost` on a T1a-shaped tempdir and uses the worker's own cancel closure, pausing after `host.refresh` returns (§5.5).
  - `t4b_no_stranded_request`.
  - `t4c_request_never_waits`.
  - `worker_exits_when_handles_drop` (Review Focus 2): drop every handle; the `JoinHandle` joins within the timeout.
- [ ] **Step 2: Red, then implement, then green.** `cargo test -p al-sem --lib lsp::l4_host::`.
- [ ] **Step 3: Breaks (all in `run_worker` or `HostHandle`).**
  - T4a: (i) delete the publish guard → generation 1 visible during #2's pause; (ii) `commit()` only when `requested == g` → #2 is `Full(NoBaseline)`.
  - T4b: swap read and drain → timeout.
  - T4c: `request()` takes a lock the refresh holds → timeout.
  - Drop test: keep a sender clone inside the worker → join times out.
- [ ] **Step 4: Gate**; commit `#222 task 7: the L4 host worker`.

### Task 8: Server wiring and the hidden flag (T5a-T5d)

**Files:**
- Modify: `src/main.rs`:
  - `Args`: `/// Experimental: run the incremental L4 host (no editor output). A crash in it ends the server.` on `#[arg(long, hide = true)] l4_host: bool`;
  - pass it to `run_server`;
  - test `t5d_flag_parses_and_is_hidden`.
- Modify: `src/server.rs`:
  - `run_server` gains `l4_host: bool`;
  - `RootBuild` (`:154-176`) gains `l4_host`;
  - `build_workspace` passes it on;
  - `build_server_state` (`:575`) spawns when set, and the `on_swap` closure (`:629`) calls `handle.request()` as its first line;
  - `ServerState` (`:117`) gains `l4_host: Option<(HostHandle, JoinHandle<()>)>`;
  - every test `RootBuild`/`ServerState` literal gains the field.

**Interfaces:** Consumes Task 7's `spawn_l4_host`, `HostHandle::{request, status}`.

- [ ] **Step 1: Tests (red), §5.6:**
  - `t5a_no_host_without_flag` (synchronous `is_none()`);
  - `t5b_didsave_requests_a_refresh` (status reaches generation 2);
  - `t5c_config_only_requests_a_refresh` (`ConfigChanged` on `state.tx`; the snapshot generation is unchanged and the host generation advances);
  - `t5d_flag_parses_and_is_hidden`.
- [ ] **Step 2: Red, then implement, then green.** Run `cargo test -p al-sem --bin al-call-hierarchy server::` and the `main` test. The existing server tests stay unchanged and green.
- [ ] **Step 3: Breaks.**
  - T5a: spawn unconditionally.
  - T5b: delete `request()` in `on_swap`.
  - T5c: request only when the snapshot generation moved.
  - T5d: drop `hide = true`.
- [ ] **Step 4: Gate**; commit `#222 task 8: --l4-host wiring`.

### Task 9: The spawned-binary test (T5e)

**Files:**
- Modify: `tests/lsp/l4_host_oracle.rs` (or a new member `tests/lsp/l4_host_binary.rs`, listed in `tests/lsp/main.rs`)

- [ ] **Step 1: Write `t5e_flag_on_sends_nothing_new` per §5.6, plus the O26 sync paragraph.**
  - Spawn `env!("CARGO_BIN_EXE_al-call-hierarchy")` with `--lsp --l4-host --no-watcher --no-telemetry --verbose` on a T1a tempdir.
  - Read stdout and stderr on two threads; kill the child in a drop guard.
  - Send `initialize` and `initialized`, then one `prepareCallHierarchy`, then wait for its response.
  - Flag-on only: wait for the stderr line `l4-host: published generation 1`.
  - Then `shutdown` and `exit`. Repeat on the same tempdir without the flag.
  - Assert: every frame is well-formed `Content-Length` JSON-RPC; the same `publishDiagnostics` count before the response; and the server-to-client messages are equal as multisets of normalized payloads (sorted keys).
- [ ] **Step 2: Run green.** `cargo test --test lsp l4_host`.
- [ ] **Step 3: Breaks.**
  - (i) Drop the `l4_host` pass-through in `main.rs` → the stderr wait times out.
  - (ii) Make the worker send one extra `textDocument/publishDiagnostics` through the server sink → the multisets differ. Revert both.
- [ ] **Step 4: Gate**; commit `#222 task 9: T5e spawned-binary guard`.

### Task 10: CHANGELOG, OUTSTANDING and the as-built check

**Files:**
- Modify: `CHANGELOG.md`: one `Added` entry under Unreleased. It covers the hidden `--l4-host` flag, the incremental host, the invalidation rule in one sentence, "no editor output, off by default", and the two discoveries (D-1 one-hop rows, D-2 file adds are Full).
- Modify: `docs/OUTSTANDING.md:705-712`: close "A future incremental L4 path" with a pointer to the spec.
- Modify: the spec, only by appending an "As built (2026-10-..)" section. Do not edit any accepted anchor range.

- [ ] **Step 1: As-built check.** Walk §10's file list and §8's acceptance matrix against the diff.
  - Every test id in the coverage map below exists by name: `cargo nextest list` or grep.
  - Every break was recorded in a task commit.
  - Write the "As built" section: any deviation (F1/F2/F4 fired or not), test names, and the redundancies recorded.
- [ ] **Step 2: Run** `scripts/check-goldens` (no regen) to a log and grep it: no golden moved. Then run `scripts/ci-steps task` to a log and grep for failures.
- [ ] **Step 3: Commit** `#222 task 10: CHANGELOG, OUTSTANDING, as-built`.

## Coverage map (spec test → task)

| Spec test | Task |
|---|---|
| T0a, T2, T2b; C1, C2, C3, C4 | 1 |
| C5 (F1(a) only) | 1 (on F1) |
| `prepare_l4_seeded` parity, `take_l4` (Review Focus 5) | 2 |
| T0b (single-app), T1a, T1b; Review Focus 1, 3 | 3 |
| T1c, T1d, T1e, T1f, T1g, T1h, T1k, T1l, T1m (+ Review Focus 4), T1n, T1o, T1q, T1r | 4 |
| T1i, T1j, T0b (cross-app) | 5 |
| T3 | 6 |
| T4a, T4b, T4c; Review Focus 2 | 7 |
| T5a, T5b, T5c, T5d | 8 |
| T5e (+ O26 sync) | 9 |
| §5.7 unchanged suites, goldens, as-built | 10 |

| Acceptance (§8) | Tasks |
|---|---|
| A1-A4 | 3, 4 |
| A5, A8 | 4 (T1n), 5 |
| A6, A7, A9 | 3, 4 |
| A10 | 1, 4 (T1m) |
| A11 | 7, 8 |
| A12 | 6 |
| A13 | 7, 8, 9 |
| A14 | 10 (+ the conductor's branch gate and CDO identity check) |
| A15, A16 | all (no bench); 4 (both universes) |
