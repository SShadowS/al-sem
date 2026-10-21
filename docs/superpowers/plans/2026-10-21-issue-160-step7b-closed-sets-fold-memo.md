# #160 step 7b: closed-class sets, fold memo — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** under `ConeOutput::DerivedOnly`, build a singleton's derived row from memoized
rows of its callees' CLOSED sets (one row per callee Cone-SCC, built at first read). Build a
recursive member's row from ONE folded row of its SCC set. Change no output byte.

**Architecture:**
- `ConeDerivedBuilder` gains one classifier (`Scratch::add`) and two entry points:
  - `fold_set` builds a `FoldedRow`;
  - `fold_row` ORs a `FoldedRow` into the routine in progress.
- `compose_inherited_cones` owns a build-local `ClosedRowMemo`. A memo row is freed with
  its cone.
- The old derived paths stay reachable only through the `cfg(test)`
  `RecursiveMembership::PerMemberWalk`. They are the oracle until the CDO run is recorded;
  then the last commit deletes them.
- Task 1 is the switch together with the acceptance tests. Task 2 is the documents.
  Task 3 is the recorded CDO oracle run. Task 4, the LAST commit, deletes the old path and
  installs the oracle that survives step 6.

**Tech stack:** Rust; the crate's census atomics (`detector_context.rs` `cones_census`);
`scripts/agentflow` (`cdo-gate`); `scripts/bench_ab.py` (conductor only).

**Spec:** `docs/superpowers/specs/2026-10-21-issue-160-step7b-closed-sets-fold-memo-design.md`
(converged at panel round 3; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.6, §9 row 7b.
Probes P1-P24: `.agent/issue-160/ledger.md`.

There are four tasks: three with a commit, and Task 3, which commits nothing unless its run
fails. The CDO bench is run by the conductor after the final panel, not in a task. Step 7b
is byte-neutral: **no golden may move in any commit** (spec A7b.5, epic I2).

## Global Constraints

- **Editing.** Use the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit). Before
  running a break, re-read the edited line to confirm the break applied.
- **Formatting.** `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** No `#[allow(`. Dead code is deleted, not silenced.
- **Protected paths, never edited:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, `.agent/` (except the
  ledger rows and Timeline lines a task names).
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
  - CDO only through a per-command prefix, with the path read from the main checkout's
    config:
    `CDO_WS=$(jq -r .cdo_ws /work/alsem/.agent/config.json) cargo test --profile cdo …`.
  - Never export `CDO_WS`, and never write its path into a committed file or the ledger.
  - Never quote a timing from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `scripts/ci-steps task-affected > <log> 2>&1`, then `grep` the log (never `| tail`).
  2. `cargo test -p al-sem --lib capability_cone` and `cargo test -p al-sem --lib
     cone_derived` must be green.
  3. The pre-commit golden hook must pass unmodified (never `--no-verify`).
- **Staging.** Stage only the task's files (`git add <paths>`), never `git add -A`.
- **Discrimination proof** for every test a task turns green or changes:
  1. Break the code with Edit.
  2. Run the test and record that it fails AT the assertion the spec names: the failing
     message, not a compile error or another test.
  3. Revert with Edit, run, and record the pass.
  Record one ledger row per proof under `## Discrimination proofs`:
  `test | break | fail output | pass output | commit`.
  The breaks are spec §5's, cited per test below. A break that comes back green is
  evidence about the test (CLAUDE.md): stop and report it, unless the spec already names
  it as a stated LIMIT.
- **The acceptance tests come from a separate agent.** They are in the worktree,
  uncommitted, and red on their own:
  - T1-T5 in `src/engine/l4/capability_cone.rs` (the `scc_set_tests` module or a sibling
    `closed_memo_tests`);
  - the T7/T8 widening;
  - T9 in `tests/cli/cli_build_counters.rs`.
  Task 1 commits them with the code that turns them green. Do not rename them. Change a
  test only if it contradicts the spec, and record that change in the ledger with the
  spec clause.

## Review Focus

1. **The old side must not share the memo** (spec §4.7, opus I1). Under `PerMemberWalk` a
   singleton builds no memo row and calls no `fold_row`; its derived row is the fold of
   `best.values()` alone. T7/T8 assert `closed_memo_builds == 0 && closed_memo_reads == 0`
   on that side.
2. **Open entries never enter a memo row.** `fold_set` reads only `!frame_bound` entries;
   substituted open entries are folded one by one (T2).
3. **The memo's lifetime is the cone's lifetime.** The memo row is removed at the same
   `remaining_uses[y] == 0` point as `fact_cones[y]`, and the leak `debug_assert!`
   includes the memo (T6).
4. **Timer windows** (spec §4.5 M4, §4.8):
   - the per-SCC `fold_set` sits after the `add_since(BFS_WALK_NANOS, _t_set)` and adds
     to `BFS_FOLD_NANOS` only;
   - the new `SGL_*` timers nest inside `SINGLETON_NANOS`, as today's `SGL_*` timers do;
   - `census_rest_is_never_negative` stays green.
5. **`Both` mode** (policy's RAW build): the raw `Vec` stays byte-identical, while the
   derived row now comes from the memo path in every mode. T5's `Both` half pins this.
6. **Interning order** changes (spec §4.6); no output may read it. The goldens and bench
   Part 1 (SARIF identity) are the guard.

## Task 1 — the switch, with the acceptance tests

Spec §4.2-§4.5, §4.7 (widening only), §4.8. Not small. It is one commit, because the
acceptance tests are red without the code.

**Files:**
- `src/engine/l4/cone_derived.rs`: `Scratch`, `FoldedRow`, `fold_set`, `fold_row`, the
  `debug_assert!`;
- `src/engine/l4/capability_cone.rs`:
  - `inherited_facts_for_singleton`;
  - the `compose_inherited_cones` member loop and refcount-free loop;
  - `ComposeStats`;
  - the `singleton()` test helper;
  - the acceptance tests;
- `src/engine/l5/detector_context.rs`: `cones_census` timers, counters and the `closed:`
  line;
- `tests/cli/cli_build_counters.rs`: T9, from the test agent.

**Interfaces** (spec §4.2, §4.3, §4.8):
- `pub(crate) struct FoldedRow { flags: u8, writes_all: Box<[ResId]>, phys_writes:
  Box<[(ResId, u8)]>, phys_reads: Box<[ResId]>, events: Box<[ResId]> }`.
  Rows are sorted, deduped, and their masks are OR-merged.
- `ConeDerivedBuilder::fold_set<'f>(&mut self, facts: impl IntoIterator<Item = &'f
  CapabilityFact>) -> FoldedRow`. It uses the builder's second scratch, so it can run
  while a routine is in progress, and it interns through `store.interner`.
- `ConeDerivedBuilder::fold_row(&mut self, row: &FoldedRow)`.
- `fold_fact` keeps its signature. Its body becomes `Scratch::add(&mut self, interner, f)`,
  the ONE classifier that `fold_set` also uses.
- `type ClosedRowMemo = HashMap<usize, FoldedRow>`, a local of `compose_inherited_cones`.
- `inherited_facts_for_singleton` gains `memo: &mut ClosedRowMemo`, `stats: &mut
  ComposeStats` and `membership: RecursiveMembership`.
- `ComposeStats` gains `closed_memo_builds`, `closed_memo_reads`, `closed_memo_entries`,
  `scc_set_folds` and `singleton_scans`.
- Census:
  - new atomics `SGL_MEMO_NANOS`, `SGL_ROW_NANOS`, `SGL_OPEN_NANOS`,
    `SGL_OWN_FOLD_NANOS`, plus a gated `max` atomic `CLOSED_MEMO_LIVE_MAX`;
  - `SGL_FOLD_NANOS` is deleted;
  - the `singleton:` line drops `fold=` and gains `memo= row= open= own=`;
  - one new line after `singleton pop:`:
    `[cones-census]   closed: memo_builds=… memo_reads=… memo_entries=… scc_set_folds=… singleton_scans=… memo_live_max=…`.

Red tests it turns green:
- T1 `closed_memo_folds_a_shared_callee_once`;
- T2 `closed_memo_excludes_frame_bound_entries`;
- T3 `duplicate_edges_fold_one_row`;
- T4 `scc_set_folds_once_per_scc`;
- T5 `derived_only_builds_no_best`;
- T9 `census_closed_line_counts_the_memo`;
- the widened T7 `scc_set_matches_member_walk_on_fixtures`.
T6 is the extended leak `debug_assert!` (no named test). The widened T8
`scc_set_matches_member_walk_on_cdo` skips without `CDO_WS`; Task 3 runs it.

- [ ] **Step 1: Red.**
  1. `git status --short -- src tests` lists the test agent's files.
  2. `git diff --stat -- src tests` shows changes inside test code only (`#[cfg(test)]`
     modules and `tests/`). If any non-test production line changed, stop and report.
  3. Run `cargo test -p al-sem --lib capability_cone` and
     `cargo test --test cli cli_build_counters::` and record which tests fail and how. A
     compile error is the expected red for tests that name the new stats or functions.
     Record it as such.
- [ ] **Step 2: The builder** (spec §4.2), `cone_derived.rs`:
  1. Move `fold_fact`'s body into `Scratch::add`.
  2. Give `ConeDerivedBuilder` two scratches, `cur` (today's `flags` and `s_*`) and `set`.
  3. Add `fold_set`: it `debug_assert!(f.resource_id.as_deref() != Some(""))` per fact
     (the Words domain condition, P20/P23), then freezes `set` into a `FoldedRow` with
     `freeze_ids`/`freeze_masked` semantics.
  4. Add `fold_row`.
  `finish_routine` is unchanged.
- [ ] **Step 3: The singleton path** (spec §4.3).
  - With `mode.wants_derived()`, after `subst`:
    1. Collect J(m): distinct callee SCCs of the cone-bearing out-edges, in out-edge
       order, in a `Vec` with `contains` before push.
    2. For each j: `memo.entry(j).or_insert_with(|| derived.fold_set(<closed entries of
       cones[j]>))`, counting `closed_memo_builds` and `closed_memo_entries`; then
       `fold_row`, counting `closed_memo_reads`.
    3. `fold_fact` each substituted open entry, then each own entry.
  - Under `!mode.wants_raw()`, return with no `own_keys`, no `best` and no scan.
  - Under `mode.wants_raw()` the `best` scan runs as today (`singleton_scans += 1`) and
    builds the raw `Vec` only. Its fold of `best.values()` is deleted.
  - Under `membership == PerMemberWalk` (`#[cfg(test)]` arm only):
    - the singleton builds NO memo row and calls no `fold_row`;
    - it runs the scan even in `DerivedOnly` (`singleton_scans += 1`);
    - its derived row is the fold of `best.values()` alone (spec §4.7, opus I1).
  - Time the new work: memo builds under `SGL_MEMO_NANOS`, `fold_row` under
    `SGL_ROW_NANOS`, the open fold under `SGL_OPEN_NANOS`, the own fold under
    `SGL_OWN_FOLD_NANOS`. All of them sit inside `SINGLETON_NANOS`.
- [ ] **Step 4: The recursive branch and the lifetime** (spec §4.4, §4.5):
  - For each recursive SCC with `use_set`, AFTER the `_t_set` add:
    1. `let set_row = derived.fold_set(set.values().map(|e| &*e.rep))`, then
       `scc_set_folds += 1`, all under `BFS_FOLD_NANOS`.
    2. Per member: `fold_row(&set_row)`, then `fold_fact` each own entry.
    3. Delete the per-member `for entry in set.values()` loop.
  - `scc_set_entries_folded` keeps its member-weighted count. Only its doc is reworded
    (§4.5).
  - In the refcount-free loop, remove the memo row together with `fact_cones.remove(y)`.
  - Extend the leak `debug_assert!` with `&& memo.is_empty()`; its message names the
    memo.
  - `PerMemberWalk`'s doc states the widened meaning and the deletion point (spec §4.7).
- [ ] **Step 5: Census** (spec §4.8): the atomics, the `singleton:` line change, the
  `closed:` line, and `add_compose_stats` for the new fields. The new timers are NOT
  `component_snapshot` components.
- [ ] **Step 6: Callers and helpers.**
  - Production callers pass `PerScc` where the parameter is new.
  - `singleton()` (`capability_cone.rs:4805`) passes a fresh memo, stats and `PerScc`.
    Its two tests' assertions are unchanged (spec §4.7 table).
- [ ] **Step 7: Green.** All of these must pass:
  - `cargo test -p al-sem --lib` (T1-T5, the widened T7, 7a's T1-T6/T10/T11 unchanged,
    the naive-union oracles);
  - `cargo test --test cli cli_build_counters::` (T9, `census_rest_is_never_negative`,
    `census_cone_scc_line_counts_the_recursive_sccs` with `set_entries=18`);
  - `scripts/check-goldens` (no golden moves).
- [ ] **Step 8: Proofs** (spec §5; every break applied with Edit):
  - **T1:** never insert into the memo (rebuild on every read) → fails on
    `closed_memo_builds == 2` (got 3).
  - **T2:** `fold_set` over every cone entry, frame-bound ones included → fails on A's row
    (gains `t/G`).
  - **T3:** drop the J(m) `contains` check → fails on `closed_memo_reads == 1` (got 2).
  - **T4:**
    - Break 1: fold the set once per member → fails on `scc_set_folds == 1` (got 2).
    - Break 2: fold every member's own entries into the set row → fails on P's row
      (gains `Cust`).
  - **T5:** route `DerivedOnly` through the scan → fails on `singleton_scans == 0`.
  - **T6:** skip `memo.remove(y)` → T1 panics on the leak `debug_assert!` ("cone leak …
    memo …"). Name that assertion in the row.
  - **T7 (fixtures):**
    - skip the own-entry fold in the singleton path → `rows_diff` failure on the first
      fixture with a singleton own entry. If no fixture reaches it, the break stays
      green; record it as not reaching T7 (T8 carries it in Task 3).
    - let `PerMemberWalk` also `fold_row` → fails on the oracle-side
      `closed_memo_reads == 0`.
  - **T9:** never count `closed_memo_builds` → fails on `memo_builds > 0`.
- [ ] **Step 9: Gate and commit.**
  - Run `scripts/ci-steps task-affected`.
  - Stage the production files above and the test agent's test files.
  - Commit message: `#160 step 7b Task 1: fold singletons from per-callee closed-set rows,
    recursive sets once per SCC`.
  - Ledger Timeline: one line naming the commit and the proofs.

## Task 2 — CHANGELOG and the numbers-file skeleton (small)

These are gate-inert prose. Depends on Task 1. The commit it makes is the commit Task 3 runs
on, so it must not touch `src/` or `tests/`.

**Files:** `CHANGELOG.md`, `docs/measurements/2026-10-21-issue-160-step7b.md` (new).

- [ ] **Step 1: CHANGELOG.** Under `[Unreleased]` → `### Changed`, add the entry "#160
  step 7b" (Part of #160, no closing keyword). It states:
  - singletons fold per-callee memoized closed-set rows, and recursive members fold one
    set row per SCC, in every mode that wants derived rows;
  - the raw output is unchanged and no golden moves;
  - the census changes (`fold=` gone; `memo= row= open= own=`; the `closed:` line);
  - amendments 7B-1 to 7B-4;
  - the old path is test-only until the last commit deletes it, and the final oracle
    (T7'/T8') survives step 6.
  Measured values are `<bench>` placeholders.
- [ ] **Step 2: Numbers file skeleton**, structured like
  `docs/measurements/2026-10-20-issue-160-step7a.md`:
  - a protocol table: machine; parent `fd77939d` (the rebased base, code-identical to
    `afd5d88f`, where X was derived), head from `$B/shas.txt`; profile,
    harness, commands;
  - Part 1: SARIF `cmp` in both scopes, histogram SHA, retained buckets head ≤ parent,
    the T8' lines;
  - Part 2: B1 and B2, each with its void check first (B1: `4,382.1 ≥ 2 × Bq`; B2:
    `4,382.1 − Bq ≥ 2 × Bc`). Then `Pq − Hq ≥ 4,382.1 − Bq` and
    `Pc − Hc ≥ 4,382.1 − Bq − Bc`, with the literal form `Pc − Hc ≥ 4,382.1 − Bc`
    reported. The sub-timers and the `closed:` line are reported;
  - Part 3: `analyze --single-app` and cross-app `policy check`, 3 rounds per side
    (epic Decision 58), reported, with the spec §8 `Both` fallback rule;
  - the re-run rule (one re-run of Part 2 if void; void again = FAIL = drop), every run
    listed, and a verdict.
  Every number is `<bench>` until the conductor fills it.
- [ ] **Step 3: Gate and commit.**
  - Run `scripts/ci-steps task-affected` (docs only).
  - Commit message: `#160 step 7b Task 2: CHANGELOG and numbers-file skeleton`.

## Task 3 — the recorded CDO oracle run (licenses Task 4)

Spec §4.7 ("The run that licenses the deletion"), §5 T7/T8, A7b.6. Depends on Task 2. It
runs on the commit just BEFORE the deletion commit, which is Task 2's HEAD. It commits
nothing unless the run fails; a failure stops the plan and is reported, never patched
around.

- [ ] **Step 1: The gate.** Run
  `python scripts/agentflow run --name cdo-gate --timeout 45 --cwd /work/al-sem-issue-160-a3 -- bash scripts/cdo-gate`
  at Task 2's HEAD (`git status --porcelain -- src tests crates` empty). Required:
  - exit 0;
  - both lines `scc-set oracle on CDO (cross-app): ` and `scc-set oracle on CDO
    (single-app): `;
  - `recursive_members_emitted == 714` cross-app;
  - on the `PerScc` side, `singleton_scans == 0`, and `closed_memo_builds > 0` cross-app;
  - on the oracle side, `closed_memo_builds == 0` and `closed_memo_reads == 0`.
- [ ] **Step 2: Record it.** Under the ledger's `## Gate results`, write:
  - the HEAD sha;
  - the exit status;
  - the log path;
  - both oracle lines, with their `closed_memo_builds` and `singleton_scans`.
  **Task 4 may start only when this record exists and is green.**
- [ ] **Step 3: T8's RSS** (spec §4.7 memory plan): run T8 alone,
  `CDO_WS=… /usr/bin/time -v cargo test --profile cdo -p al-sem --lib scc_set_matches_member_walk_on_cdo -- --nocapture > <log> 2>&1`.
  Record `Maximum resident set size` in the ledger. This is the T8 figure that Task 4's T8'
  is compared against.
- [ ] **Step 4: Proofs on CDO** (spec §5 T7/T8), each a break with Edit, then T8 alone,
  then revert:
  - Skip the own-entry fold in the singleton path → T8 fails on its row diff.
  - T2's break (`fold_set` over frame-bound entries too) → record red or green. If green,
    record it as the stated LIMIT (CDO has no contradicted guard that reaches a memo row;
    T2 carries the class).
  After the reverts, `git status --porcelain -- src` must be empty.
- [ ] **Step 5:** No commit (ledger rows only).

## Task 4 — delete `PerMemberWalk`; install T7'/T8' (the LAST commit)

Spec §4.7 (deletion, final oracle, memory plan, migration table), §5 T7/T8 → T7'/T8'.
Depends on Task 3's green record. **It is the step's last code commit.**

**Files:** `src/engine/l4/capability_cone.rs`.

**Interfaces:**
- Deleted:
  - `RecursiveMembership` and its `PerMemberWalk` arms in `compose_inherited_cones` and
    `inherited_facts_for_singleton`;
  - the `membership` parameter, wherever only the variant used it.
  Since `PerScc` is then the only value, the enum and the parameter both go; spec §4.7
  says "wherever only the variant used it".
- Added (`#[cfg(test)]`; superseded in final panel round 1 by the `#[cfg(test)]` module
  `raw_fold_hook` with a thread-local oracle builder, so no `raw_sink` parameter or branch
  reaches non-test builds — spec §4.7):
  - `compose_raw_streaming(…, sink: &mut dyn FnMut(&str, Vec<CapabilityFact>))`;
  - a `raw_sink: Option<&mut dyn FnMut(&str, Vec<CapabilityFact>)>` parameter on
    `compose_inherited_cones`. Production callers pass `None`. With `Some`, the member's
    raw `Vec` goes to the sink and its `InheritedConeResult` keeps an empty `Vec`.
- The migration table of spec §4.7, every row:
  - `compose` and `compose_model` lose `how`;
  - T10 drops its `PerScc` argument;
  - `both_paths` → `derived_and_raw_fold`, with its 6 callers;
  - `both_paths_model`, with its 3 callers;
  - `check_model` → T7'/T8';
  - T5 (7a)'s `member_walks == 479` is read off the `RawOnly` outcome.

- [ ] **Step 1: Red.** Rewrite T7/T8 as T7'/T8' (spec §4.7). Per universe:
  1. Compose `DerivedOnly` and assert `member_walks == 0` and `singleton_scans == 0` per
     composition.
  2. Run `compose_raw_streaming` with a sink that folds `raw ∪ direct_raw[m]` into an
     oracle builder (`begin_routine`, `fold_fact` each, `finish_routine(m)`) and drops the
     `Vec`.
  3. Compare with `rows_diff`.
  4. Assert `closed_memo_builds > 0` summed over T7''s corpus, and on CDO cross-app in
     T8'.
  The suite does not compile until Step 2 adds `compose_raw_streaming`. That is the red.
- [ ] **Step 2: Implement.**
  1. Add the sink parameter and `compose_raw_streaming`.
  2. Delete the variant, its arms and the parameter.
  3. Migrate every helper and caller in the spec §4.7 table. The hand-stated row
     assertions stay byte-unchanged; only the oracle line changes, from new-vs-old to
     new-vs-raw-fold.
  No test may become a comparison of the new path against itself.
- [ ] **Step 3: Green.**
  - `cargo test -p al-sem --lib`;
  - `scripts/check-goldens`;
  - `scripts/ci-steps clippy > <log> 2>&1`: release, `-D warnings`, with no dead
    `cfg(test)` remnants in non-test builds.
- [ ] **Step 4: T8' on CDO and its RSS.** Run
  `CDO_WS=… /usr/bin/time -v cargo test --profile cdo -p al-sem --lib <T8' name> -- --nocapture > <log> 2>&1`.
  - Required: both oracle lines, pass.
  - Rule: max RSS ≤ Task 3's T8 figure + 256 MB.
  - If it exceeds that, the streaming is wrong: fix it in this task and re-measure.
  - Record both figures in the ledger.
- [ ] **Step 5: Proofs** (spec §5, re-run against T7'/T8'):
  - **Skip the own-entry fold** in the singleton path → T8' fails on its row diff; T7'
    too if a fixture reaches it.
  - **T2's break** → T8' red or the LIMIT recorded in Task 3, unchanged.
  - **Third break:** let the `DerivedOnly` side take the scan → fails on
    `singleton_scans == 0`.
  - **Migrated 7a tests:** re-run the 7a T5 break ("route `DerivedOnly` through the
    walk") against `scc_set_479_cycle_scales` → fails on `member_walks == 0`. This shows
    the migrated assertions still discriminate.
- [ ] **Step 6: Gate and commit.**
  - Run `scripts/ci-steps task-affected`, then `scripts/check-goldens`.
  - Commit message: `#160 step 7b Task 4: delete the test-only PerMemberWalk; the oracle
    compares DerivedOnly with a streamed RawOnly fold`.
  - Ledger: Timeline line, and the proof rows with this commit's sha.

---

## The CDO bench (conductor only)

The three-part CDO bench of spec §8 runs **once, after the final panel**, on the branch
head, which is Task 4's commit. The conductor runs it through
`agentflow run --name bench --timeout 45 -- bash <part>.sh`, never an implementer and never
beside a build.

- Bands come only from that bench's parent runs (`fd77939d`, code-identical to
  `afd5d88f`, X's base). Part 2 runs 5 rounds per side; Part 3 (reported) runs 3
  (epic Decision 58).
- Void checks are read first. A void bound gets one re-run of Part 2; void again = FAIL =
  drop.
- A missed bound = drop: revert the speed commits; `PerMemberWalk` then goes to step 6.
- The conductor fills the `<bench>` placeholders of Task 2's files in a docs-only commit
  after the bench. This is the one commit allowed after Task 4. It touches no `src/`.

## Acceptance map (spec §6)

| Acceptance | Where |
|---|---|
| A7b.1 | Task 2 (CHANGELOG states 7B-1) |
| A7b.2 | Task 1 (T2) |
| A7b.3 | Task 1 (T1, T3, T6) |
| A7b.4 | Task 1 (T1, T4) |
| A7b.5 | every task (goldens), bench Part 1 |
| A7b.6 | Tasks 1 and 3 (T7/T8), Task 4 (T7'/T8') |
| A7b.7 | spec "Feasibility first" (done) |
| A7b.8, A7b.9 | bench Part 2 |
| A7b.10 | Task 4, licensed by Task 3 |
| A7b.11 | Task 3 (`cdo-gate`) and the branch gate |
| A7b.12 | bench Part 1 |
