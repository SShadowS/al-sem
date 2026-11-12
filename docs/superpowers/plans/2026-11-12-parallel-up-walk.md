# TLS-21: impact's up walk over several roots at once (implementation plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The one-shot CLI walks impact's up part over 8 roots at once, with output byte-identical
to master's; the resident engine stays serial.

**Architecture:** The route that runs a sited-graph job passes a `ReachWorkers` count (batch
runner: `default_reach_workers()`; resident revisions: serial). `up::up` runs each walked root as
one task in TLS-22's `reach_pool`, collects rows by index, shares a lock-guarded caption cache and
a scratch stash, then walks the union serially.

**Tech Stack:** Rust, rayon (`reach_pool`), serde_json.

**Spec:** `docs/superpowers/specs/2026-11-12-parallel-up-walk.md` (commit `03d899dc`).

## Global Constraints

- Output byte-identical at any worker count, with `cost.walkMs`, `cost.walksUs` and
  `cost.scratchBytes` blanked (spec 3.4).
- Resident engine serial: `Revision` passes `self.pass_workers()`, `GraphRevision` passes
  `ReachWorkers::SERIAL`; no resident knob (spec top, 3.1).
- One knob for both passes: `AL_SEM_REACH_WORKERS`; no new environment variable (3.1).
- Every correctness guard gets one discrimination proof, broken with Edit, run with the narrowest
  command, reverted with Edit, both outcomes recorded in the test's doc (CLAUDE.md).
- `rustfmt <file>` per file; stage only named paths; commits end with the Co-Authored-By line.
- No CDO run without the orchestrator's slot. Stop my own rust-analyzer after `.rs` edits.

## Review Focus

1. A root with no nodes between walked roots: its row stays in place (test 1 fixture, middle).
2. Two roots whose summaries need the same caption file at once: both rows carry the caption
   (test 2).
3. `up` called on a rayon worker (a test reading many fixtures on rayon): serial, no deadlock
   (test 4).
4. More roots than workers: scratches made ≤ workers (test 5).
5. One-core host: `default_reach_workers()` is 1, the pass runs serially; test 3's parallel claim
   prints a skip note instead of failing.

---

### Task 1: The outside reference (fixture + master golden), before any refactor

**Files:**
- Create: `tests/fixtures/up-parallel/` (an AL workspace: `app.json`, `src/*.al`)
- Create: `tests/fixtures/up-parallel/up.master.golden.json`
- Create: `tests/cli/cli_up_parallel.rs`; Modify: `tests/cli/main.rs` (add `mod cli_up_parallel;`)

**Interfaces:** Produces the fixture path and root list used by Tasks 3-4.

- [ ] **Step 1:** Check that nothing walks `tests/fixtures/` as a whole (`grep -rn "tests/fixtures\"" tests src`); if something does, put the fixture where nothing walks.
- [ ] **Step 2:** Write the fixture: 6 roots in this `--root-id` order —
  `R1` (plain codeunit procedure with 2 callers), `R2` (a procedure declared in both arms of an
  `#if`/`#else`, so 2 nodes), `R3` (a stable id that does not exist: not-in-graph, MIDDLE),
  `R4` (a procedure called from a page action's `OnAction` with a `Caption`), `R5` (reached over
  that action's `RunObject` page), `R6` (a procedure with no callers). Root ids are the
  StableRoutineIds `alsem` prints; take them from `alsem impact --part main --routine ... --format json`.
- [ ] **Step 3:** Mint the golden with `target3/release-fast/alsem` built from `4f2459a6` plus the
  step-0 spans (trace-only, output unchanged):
  `alsem impact --part up --root-id=R1 ... --root-id=R6 -- tests/fixtures/up-parallel > up.master.golden.json`,
  then blank `cost.walkMs`, `cost.walksUs`, `cost.scratchBytes` to `null` with `jq`. Check by eye:
  6 rows, row 3 `not-in-graph`, row 4 or 5 carries the caption, row 2 `"nodes": 2`.
- [ ] **Step 4:** Test `up_part_matches_master_at_1_and_8_workers`: runs the built `alsem` binary
  (as the other `cli` members do) with `AL_SEM_REACH_WORKERS=1`, then `=8`; blanks the three
  fields; `assert_eq!` against the golden; asserts `roots[i].stableId == R(i+1)` for every i. The
  golden is read only, never written: a doc comment names its source (`4f2459a6`) and that
  `REGEN_TEMP_GOLDENS` must not touch it.
- [ ] **Step 5:** `cargo test --test cli cli_up_parallel::` → PASS on today's code.
- [ ] **Step 6:** Commit `test(TLS-21): master's up output on a fixture, the outside reference`.

### Task 2: The route states the worker count

**Files:**
- Modify: `src/engine/batch/job.rs` (`Job::sited`), `src/engine/batch/mod.rs` (caller),
  `src/engine/resident/revision.rs` (`answer_sited`, both revision kinds),
  `src/engine/impact/jobs.rs`, `src/engine/test_select/jobs.rs`,
  `src/engine/resident/session_tests.rs` (impls), `src/engine/l5/digest_cli.rs`

**Interfaces:**
- Produces: `fn sited(&self, side: SideIx, graph: Result<&SitedGraph, &str>, workers: &ReachWorkers) -> Result<SideDoc, JobFailure>`;
  `fn answer_sited(job: &dyn Job, side: SideIx, g: &SitedGraph, workers: &ReachWorkers) -> JobOutput`;
  `pub(crate) fn reach_pool(n: usize) -> &'static rayon::ThreadPool`;
  `UpArgs { roots, assume_no_dependents, keep, cancel: Option<&'a (dyn Fn() -> bool + Sync)>, workers: ReachWorkers }`.

- [ ] **Step 1:** Add the parameter; batch runner passes `&default_reach_workers()`; `Revision`
  passes `&self.pass_workers()`; `GraphRevision` passes `&ReachWorkers::SERIAL`; `UpJob` puts a
  clone in `UpArgs.workers`; the two other impls ignore it.
- [ ] **Step 2:** `cargo build` and `cargo test --test cli cli_up_parallel::` → PASS (no behaviour change yet).
- [ ] **Step 3:** Commit `refactor(TLS-21): the route passes the up walk's worker count`.

### Task 3: The parallel per-root pass

**Files:** Modify `src/engine/impact/up.rs`, `src/engine/l5/digest_cli.rs` (`PassProbe` fields).

**Interfaces:**
- Consumes: Task 2's `UpArgs.workers`, `reach_pool`.
- Produces: `pub fn up(g: &SitedGraph, args: &UpArgs<'_>, captions: &Captions) -> Value`
  (captions now `&`); `Captions::actions(&self, unit: &str) -> Arc<Vec<ObjectActions>>`.
  `PassProbe` gains (cfg(test)): `up_tasks`, `scratches_made`, `pool_threads_seen: Mutex<Vec<usize>>`,
  `caption_parses: Mutex<HashMap<String, usize>>`, and two holds: `hold_root0_until_others: bool`,
  `caption_race: Option<(String, Duration)>` (task A held between miss and insert until B looked up).

- [ ] **Step 1:** `Captions`: `parsed: Mutex<HashMap<String, Arc<Vec<ObjectActions>>>>`; parse outside
  the lock; `entry(unit).or_insert(parsed)` and return the map's `Arc`. `of` and `summary` take `&Captions`.
- [ ] **Step 2:** A `fn walk_root(...) -> RootOut { row: Value, states: usize, visitor_bytes: usize, walk_us: u64 }`
  that builds its own `WalkOptions` from `UpArgs`, takes a scratch from `Mutex<Vec<WalkScratch>>`
  (or makes one, counting `scratches_made` in tests), walks, puts the scratch back, builds the row.
- [ ] **Step 3:** In `up`: serial prelude (Shared, per-root nodes, `all_nodes`, not-in-graph rows as
  `RootOut` with zeros); then `if workers.count() <= 1 { loop } else if rayon::current_thread_index().is_some() { counter "impact.up.serial_in_rayon"; loop } else { reach_pool(n).install(|| roots.par_iter().with_max_len(1).map(task).collect()) }`;
  fold sums/max in index order; union walk on a stash scratch; `scratchBytes` = sum of stash
  capacities + trace counter `impact.up.scratch_bytes`.
- [ ] **Step 4:** Spans: keep `impact.up.roots`, `impact.up.union_walk`, `impact.up.union_summary`,
  `impact.up.caption_parse` and `impact.up.caption_files`; drop the step-0 per-root time counters.
  Walk-window memory (Linux, tracing on only): at `up`'s start write `5` to `/proc/self/clear_refs`;
  on success read `VmRSS`, after the union summary read `VmHWM`, emit `impact.up.rss_start_mb` and
  `impact.up.peak_mb`; on failure emit nothing.
- [ ] **Step 5:** Docs on `walksUs` (sum, exceeds wall in parallel) and `scratchBytes` (sum of the
  stash; varies with workers); fix the stale comment at the old loop ("one is alive at a time").
- [ ] **Step 6:** `cargo test --test cli cli_up_parallel::` → PASS at 1 and 8.
- [ ] **Step 7:** `tests/cli/engine_host.rs`: also remove `scratchBytes` in the `part` closure, with
  the reason in its comment. `cargo test --test cli engine_host::` → PASS.
- [ ] **Step 8:** Commit `perf(impact): TLS-21 the up walk runs roots in parallel (one-shot CLI)`.

### Task 4: Guards and their proofs

**Files:** Create `src/engine/impact/up_par_tests.rs` (`#[cfg(test)] mod up_par_tests;` in `up.rs`);
Modify `src/engine/resident/revision_tests.rs` (test 3).

Fixture: the Task 1 workspace, built in-process (`WorkspaceInputs::read`, `ResolvedProgram::build`
cross-app, `sited_from_program`), as `session_tests.rs`' `sited_cold` does. Every hold has a 10 s
timeout.

- [ ] **Test 1b** `parallel_rows_keep_root_order_when_root_0_finishes_last` (`ReachWorkers::new(8)` +
  `hold_root0_until_others`): rows equal the golden (three fields blanked); probe max in flight > 1.
  Proofs: (a) rows pushed in completion order → FAIL; (b) not-in-graph rows pushed after the
  walked ones → FAIL here and in `cli_up_parallel::`.
- [ ] **Test 2** `racing_tasks_both_get_the_caption` (`ReachWorkers::new(2)`, roots R4 and R5,
  `caption_race`): both rows carry the caption; the map holds one entry for the file.
  Proof: insert an empty "being parsed" entry before parsing and return it to a second reader → FAIL.
- [ ] **Test 3** (revision_tests.rs) `the_resident_up_walk_runs_on_the_revisions_count`: a revision
  `with_reach_workers(ReachWorkers::new(3))`, first wave of 3 held: probe max in flight == 3,
  `up_tasks` == walked roots (5), every `pool_threads_seen` == 3. A production revision: max in
  flight ≤ 1. On one core: print a note and skip the parallel claim.
  Proof: build the revision's `reach_workers` from `default_reach_workers()` → the production
  case shows > 1 → FAIL.
- [ ] **Test 4** `inside_a_rayon_worker_the_up_walk_is_serial`: `ReachWorkers::new(4)`, a first-wave hold
  of 2, called inside `ThreadPoolBuilder::new().num_threads(2)` pool's `install`: max in flight == 1.
  Proof: delete the `current_thread_index` branch → max in flight ≥ 2 → FAIL.
- [ ] **Test 5** `scratches_are_reused_across_roots`: `ReachWorkers::new(2)`, 6 roots: `scratches_made` ≤ 2.
  Proof: a fresh scratch per task → 5 → FAIL.
- [ ] **Step:** run each with `cargo test -p al-sem --lib up_par_tests::` / `revision_tests::the_resident_up`;
  record each proof's two outcomes in the test's doc comment.
- [ ] **Step:** Commit `test(TLS-21): the up walk's order, captions, resident, rayon and scratch guards`.

### Task 5: Docs and the task gate

- [ ] CHANGELOG `## [Unreleased]`: `### Changed (TLS-21: the up walk runs roots in parallel)` —
  one-shot only; resident serial; `cost.scratchBytes` now the sum of the scratches (varies with
  the worker count, blanked in the resident-vs-CLI comparison); `walksUs` exceeds wall. Numbers
  filled after Task 6.
- [ ] `scripts/ci-steps task` → green (log to a file, grep the summary). Commit, push the branch.

### Task 6: Measure (CDO slot from the orchestrator) and the note

- [ ] Ask for the slot; MemAvailable ≥ 35 GB at start; nothing else running.
- [ ] Build `release-fast` of the branch head.
- [ ] Composite `impact --diff` (TLS-16 input) and `--base bc3ccb18~5` at 1, 2, 4, 8, 16: traced
  run (spans, walk window) and untraced run (`timeit.py`: wall, CPU, peak); stdout equal to
  count 1's.
- [ ] `--part up` with the 699 `--root-id`s from count 1's output, at 1, 2, 4, 8, 16, traced +
  untraced; stdout (three fields blanked) equal to `4f2459a6`'s `--part up` on the same roots.
- [ ] `scripts/cdo-speed` head vs master: resident ratio ~1.00.
- [ ] Note `docs/superpowers/notes/2026-11-12-parallel-up-walk.md` with the scripts and raw rows in
  `docs/superpowers/notes/parallel-up-walk/`; CHANGELOG numbers; keep/drop per spec section 5.
- [ ] Task gate, commit, push; tell the orchestrator the head.
