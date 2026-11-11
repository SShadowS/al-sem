# TLS-22 Parallel Literal Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** run the literal pass's per-root loop (`literal_reach_entries`, span `prove.reach_effects`) on
a fixed-size thread pool, output byte-identical at any worker count, memory bounded and measured.

**Architecture:** make every cache the loop touches `Sync` (evaluator state local to an evaluation,
pure memos behind short locks, one shared suffix cache), pass an explicit `ReachWorkers` setting down
every route into the loop, and run the loop with `par_iter_mut` inside `reach_pool(n)`.

**Tech Stack:** Rust, rayon (already a dependency), `std::sync::{Mutex, OnceLock, Arc}`.

**Spec:** `docs/superpowers/specs/2026-11-11-parallel-literal-reach.md` section 8 (approved with
changes, round 3, sol + Opus). Read 8.3-8.7 before each task; this plan names only the decisions.

## Global Constraints

- Output byte-identical at any worker count (every golden, the CDO output tests, ratchets).
- `ReachWorkers` is a parameter on every route, never a process-global; only the binary reads
  `AL_SEM_REACH_WORKERS`.
- 1 worker = serial on the calling thread, no pool. `--low-mem` resident sessions pass 1.
- `reach_pool(n)`: exactly `n` threads, `big_stack::BIG_STACK_SIZE` stacks, one per distinct `n`
  per process.
- `per_tree` / `STATE_CAP` / `SUFFIX_CACHE_STATES` / `EVAL_BUDGET` / `EVAL_DEPTH` unchanged.
- A cancelled search is never cached.
- Edit with Edit/Write; `rustfmt <file>`; stage named paths only; discrimination breaks by Edit.
- No build while the orchestrator holds builds (cdo-gate running).

## Review Focus

1. Two host requests running passes at once share one pool: neither deadlocks, outputs equal serial
   (test in Task 4).
2. A pass called from inside a rayon worker: `debug_assert!` fires instead of a silent deadlock
   (Task 4, a `#[should_panic]` test in debug).
3. A root whose search is capped or cancelled inside a worker: same `Stop` as serial, cancelled not
   cached (Task 2's eviction test covers capped; Task 2 adds a cancelled case).
4. A batch with fewer roots than workers (1 root): runs, equals serial (Task 4 equality test with a
   one-root read).
5. Panics in a worker propagate to the caller as today (rayon re-raises); no poisoned-lock cascade
   on the next request: locks recover with `unwrap_or_else(|p| p.into_inner())` as
   `resident/revision.rs:896` does (Task 2 and 3 code steps).

---

### Task 0: Can it be shared? (compile probe, no commit)

**Files:** none committed.

- [ ] **Step 1:** add a temporary `fn _probe(r: &LiteralReach<'_>) { fn s<T: Sync>(_: &T) {} s(r); }`
  in `literal_reach.rs`, `cargo check -p al-sem --lib`; record every non-`Sync` field the compiler
  names (expected: `RecRefFlow::eval`, `results`, `LiveTree::by_routine`; anything else is a gap in
  spec 8.3: stop and report).
- [ ] **Step 2:** run the grep of spec 8.3 (files listed there) for `Cell|RefCell|Rc<|Mutex|thread_local|static mut|Atomic`; paste the hits into the commit message of Task 1.
- [ ] **Step 3:** remove the probe.

### Task 1: RecRefFlow without interior evaluator state

**Files:** Modify `src/engine/l5/recref_flow.rs` (`eval` field `:203`, `results` `:207`, `new`
`:389-390`, `evaluate` `:584-617`, `eval_routine` `:639` and every `self.eval` use: `:663`, `:679`,
`:727`, recursion `:890`). Test: `src/engine/l5/literal_flow_tests.rs`.

**Interfaces:** Produces `RecRefFlow: Sync`; `results: Mutex<HashMap<EvalKey, Option<HashMap<String, Val>>>>`;
`fn eval_routine(&self, st: &mut EvalState, i, inputs, globals, depth)`; `#[cfg(test)] pub(crate) fn set_evaluate_hook(&self, f: Box<dyn Fn() + Send + Sync>)`
(runs after the `results` miss, lock released, before the fresh `EvalState`).

- [ ] **Step 1: test `evaluate_race_keeps_one_answer`**: two scoped threads call `arg_tables` on the
  same sensitive routine/key of an existing literal-flow fixture; the hook makes each wait on a
  `Barrier(2)` (first two calls only), so both miss. Assert both answers equal a serial answer from a
  fresh `LiteralReach`, and `results.lock().len()` grew by exactly 1.
- [ ] **Step 2:** `cargo test -p al-sem --lib literal_flow_tests` → fails to compile (no hook).
- [ ] **Step 3:** implement: delete the `eval` field; `evaluate` builds `let mut st = EvalState { budget: EVAL_BUDGET, ..Default::default() }` and threads `&mut st`; `results` lookup and `entry(key).or_insert` each under a short lock.
- [ ] **Step 4:** the test and all `literal_flow_tests`, `recref_flow` tests pass.
- [ ] **Step 5: discrimination:** the test also asserts the hook ran exactly twice during the race and
  0 times for a third call after it (the memo is consulted). Break: skip the `results` lookup →
  the third call runs the hook, the test fails; revert → passes. Record both.
- [ ] **Step 6:** commit `refactor(engine): TLS-22 evaluator state local to each evaluation`.

### Task 2: A shared, `Sync` suffix cache

**Files:** Modify `src/engine/l5/literal_reach.rs` (`SuffixCache` `:535-630`, `LiveTree::by_routine`
`:1522`, `:2874`, users of `get`/`plain` taking `&mut SuffixCache`), `src/engine/l5/group_path.rs`
(`paths(.., suffixes: &mut SuffixCache, ..)` → `&SuffixCache`), `src/engine/l5/digest_cli.rs`
(`LiteralPass::suffixes`, `literal_reach_entries`). Tests: `src/engine/l5/wave2_index_tests.rs`
(existing `SuffixCache` tests adapt to `&self`).

**Interfaces:** Produces `SuffixCache<'a>` with `fn get(&self, reach, start, cancel) -> Result<Arc<LiveTree<'a>>, Stop>`,
`fn plain(&self, reach, from) -> Arc<HashSet<&'a str>>`, counters read through
`fn stats(&self) -> SuffixStats { built, evicted, peak_held, held, duplicate }`, and
`#[cfg(test)] fn set_miss_hook(&self, f: Box<dyn Fn(&State<'a>) + Send + Sync>)` (after the miss,
lock released, before `tree_from`). `LiveTree.by_routine: OnceLock<..>`.

- [ ] **Step 1: test `shared_suffix_tree_opposing_bindings`** (hand-built fixture per spec 8.6: two
  roots into one dependency routine under different bindings whose trees differ — assert that
  precondition — and two roots under the same entry state). Workers via `std::thread::scope` (the
  pool comes in Task 4); the miss hook holds the first two same-state misses on a `Barrier(2)`.
  Assert: equal trees, `stats().duplicate == 1`, `held` counts that tree once, every
  `group_resource_paths`/`group_resource_status` equals a serial run.
- [ ] **Step 2: test `eviction_under_threads_equals_serial`**: `with_limits(per_tree, per_tree)`
  (same `per_tree` as the oracle), 4 scoped threads over the fixture's roots; outputs equal the
  serial default-budget run; preconditions `evicted > 0` and some key built twice. Plus a cancelled
  case: a `cancel` that returns true → `Err(Stop::Cancelled)`, `stats().built` counted, key not held.
- [ ] **Step 3:** both fail to compile; implement (`Mutex<Inner>`, build outside the lock,
  insert-if-absent, `duplicate += 1` when present, FIFO eviction unchanged, `plain` built outside the
  lock). Poisoned locks recover (`into_inner`).
- [ ] **Step 4:** new tests and every existing `wave2_index_tests` suffix test pass.
- [ ] **Step 5: discrimination (record each):** (a) key by target alone (drop bindings) →
  shared-tree test fails; (b) insert unconditionally, adding to `held` twice → shared-tree test
  fails on `held`; (c) a rebuilt tree gets `per_tree / 2` (assert first the fixture has a tree
  larger than that whose capping changes output) → eviction test fails. Revert each → passes.
- [ ] **Step 6:** commit `refactor(engine): TLS-22 one shared suffix cache, Arc trees`.

### Task 3: `ReachWorkers` on every route

**Files:** Modify `src/engine/l5/digest_cli.rs` (`LiteralPass::new`, `literal_reach_pass` `:2223`,
`passed_root_entries` `:1786`, `digest_effects_each` `:1753`, `digest_effects_chunks` `:2144`),
`src/engine/session.rs` (`EffectsStream` `:1039-1088` gets a `reach_workers` field; `:1167`,
`:1521`), `src/engine/resident/revision.rs` (`Revision` `:301` gets `reach_workers`; `:927`,
`:1361`), `src/engine/resident/session.rs` (set from `low_mem()` `:400`), the binary's CLI entry
(`src/bin/alsem.rs`: `REACH_WORKERS` or `AL_SEM_REACH_WORKERS`). Every test caller passes `1`
unless it tests parallelism.

**Interfaces:** Produces `pub(crate) type ReachWorkers = usize;` and
`pub(crate) const REACH_WORKERS: usize = 8;` in `digest_cli.rs` (8 is a placeholder the measurement
in Task 5 replaces; it must be `min(.., available_parallelism)` at the binary), every route function
above taking `workers: ReachWorkers` as its last parameter.

- [ ] **Step 1:** thread the parameter (no behavior change yet: the loop stays serial).
- [ ] **Step 2:** `cargo check --all-targets`; `scripts/ci-steps task-affected` green.
- [ ] **Step 3:** commit `refactor(engine): TLS-22 the reach worker count reaches the pass`.

### Task 4: The pool and the parallel loop

**Files:** Modify `src/engine/l5/digest_cli.rs` (`literal_reach_entries` `:2396`; new
`fn reach_pool(n: usize) -> &'static rayon::ThreadPool`). Tests: new
`src/engine/l5/literal_pass_parallel_tests.rs` (registered next to `literal_flow_tests` in
`src/engine/l5/mod.rs`), `src/engine/resident/` session tests for the low-mem route.

**Interfaces:** Consumes Tasks 1-3. Produces `reach_pool`, and
`#[cfg(test)] pub(crate) struct PassProbe { max_in_flight: AtomicUsize, first_wave: Option<(usize, Duration)> }`
held by `LiteralPass` and exposed by `Revision` for the resident tests.

- [ ] **Step 1: tests:**
  - `thread_count_equality_over_fixtures`: every r0 dir + dependency fixtures, passed entries at
    workers 1, 2, 8 equal by `format!("{:?}")`; preconditions (spec 8.6) asserted; also a one-root read.
  - `workers_are_obeyed`: `n` = 2 and 4, more roots than `n`, first-wave hook with a 10 s timeout;
    `max_in_flight == n`, `reach_pool(n).current_num_threads() == n`; workers 1 → 1.
  - `low_mem_session_runs_serially`: a `ResidentSession::with_low_mem(true)` witness fill and a
    streamed read: `max_in_flight == 1`; default session: `== REACH_WORKERS` (capped by roots).
  - `two_requests_share_the_pool`: two scoped threads run passes on two trees at once at
    `REACH_WORKERS`; both equal serial; finishes (test timeout 60 s).
  - `pass_inside_rayon_worker_asserts` (`#[cfg(debug_assertions)]`, `#[should_panic]`).
- [ ] **Step 2:** they fail (loop serial). Implement: workers 1 → today's loop; else
  `debug_assert!(rayon::current_thread_index().is_none())`, `reach_pool(n).install(|| entries.par_iter_mut().with_max_len(1).for_each(..))`,
  the `prove.reach_effects` span on the calling thread around it.
- [ ] **Step 3:** all pass; then `scripts/ci-steps task` green (every golden).
- [ ] **Step 4: discrimination:** (a) `reach_pool(n)` always builds `REACH_WORKERS` threads →
  `workers_are_obeyed` fails; (b) the resident route ignores `low_mem` → `low_mem_session_runs_serially`
  fails. Revert → pass. Record both.
- [ ] **Step 5:** commit `feat(engine): TLS-22 the literal pass runs roots in parallel`.

### Task 5: Measure, choose `REACH_WORKERS`, admission estimates (needs the orchestrator's window)

**Files:** Modify `src/engine/l5/digest_cli.rs` (`REACH_WORKERS`), `src/engine/resident/budget.rs`
(`Case::Fill("witness")`, `Case::Trees` in `table_mib` only if measured higher), new note
`docs/superpowers/notes/2026-11-1x-parallel-literal-pass.md`, `CHANGELOG.md` (`[Unreleased]`).

- [ ] **Step 1:** temporary trace fields (per-root allocation peak at workers 1, `flow_cache` lock
  wait, duplicate builds); release-fast build to `target2` style dir (never beside a gate).
- [ ] **Step 2:** spec 8.7 runs, one job at a time with the orchestrator's go: `impact --diff` and
  `impact --base bc3ccb18~5 --no-tests --format json` at `AL_SEM_REACH_WORKERS` 1/2/4/8/16; resident
  `alsem mcp` default and `--low-mem`, `Trees` and `Fill("witness")`, one and two concurrent.
  Byte-compare the `--diff` JSON at 1 and the chosen count.
- [ ] **Step 3:** set `REACH_WORKERS` per spec 8.7; raise the two `table_mib` keys if measured; if
  `flow_cache` wait > 5 % of the span, report (sharding is a follow-up, spec 8.3).
- [ ] **Step 4:** remove the temporary trace fields; note + CHANGELOG (numbers, the 32-root batch
  tail if it dominates); `scripts/ci-steps task` green; commit; ask the orchestrator for the
  branch gate (`cdo-speed` must pass).
