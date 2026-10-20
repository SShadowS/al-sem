# #160 step 7a: one inherited set per Cone-SCC — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** under `ConeOutput::DerivedOnly`, a recursive Cone-SCC member's derived row is
folded from one set per SCC (its fact cone) plus the member's own entries and self half,
with no per-member walk, and with every output byte unchanged.

**Architecture:** the change lives in `compose_inherited_cones`'s recursive branch
(`src/engine/l4/capability_cone.rs`). Task 1 adds the instrument (real `ComposeStats`, the
census `cone_scc:` line, a census-only `root_set_probe`) and changes no behaviour; the
conductor then runs the stop rule on CDO. Task 2 switches the derived fold to the SCC set.
Task 3 leaves the old fold reachable only from tests. Tasks 4-5 are the CDO oracle run
and the documents.

**Tech stack:** Rust; the crate's census atomics (`detector_context.rs` `cones_census`);
`scripts/bench_ab.py` (conductor only).

**Spec:** `docs/superpowers/specs/2026-10-20-issue-160-step7a-scc-membership-design.md`
(committed `af76001e`, pre-ratified from attempt 1; "spec §n" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.6, §9 row 7a.
Probes P1-P25 and the attempt-2 test record: `.agent/issue-160/ledger.md`.

Five implementation tasks, one commit each, plus one conductor checkpoint. Step 7a is
byte-neutral: **no golden may move in any commit** (spec A7a.3, epic I2).

## Global Constraints

- Edit with the Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script
  rewrites, including discrimination breaks (break with Edit, revert with Edit).
- `rustfmt <file>` per touched `.rs` file; never `cargo fmt`.
- No `#[allow(`. Dead code is deleted, not silenced.
- Protected paths, never edited: `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`, `.agent/` (except the
  ledger's proof table and Timeline lines a task names).
- Environment: `export TREE_SITTER_AL_PATH=/work/src/tree-sitter-al`. CDO only through a
  per-command prefix, `CDO_WS=/work/src/cdo-baseline/Cloud cargo test --profile cdo …`.
  Never export `CDO_WS`. Never quote a timing from the `cdo` profile.
- Per-task gate, before the commit: `scripts/ci-steps task-affected > <log> 2>&1`, then
  `grep` the log (never `| tail`). Then `cargo test -p al-sem --lib scc_set_tests` green
  except the tests the task names as still red. The pre-commit golden hook must pass
  unmodified (never `--no-verify`).
- Stage only the task's files (`git add <paths>`), never `git add -A`.
- **Discrimination proof** for every test a task turns green or changes: break with Edit
  (the Edit tool refuses an `old_string` that does not occur once, so the break cannot
  silently miss), run the test, record that it fails AT the assertion the spec names (the
  failing message, not a compile error or another test), revert with Edit, run, record
  the pass. One ledger row each: `test | break | fail output | pass output | commit`. The
  breaks are spec §5's; the spec section is named per test below.
- Names fixed by the stubs already in the tree (do not rename): `RecursiveMembership {
  PerScc, #[cfg(test)] PerMemberWalk }` (`capability_cone.rs:2402`), `ComposeStats`
  (`:2416`, 12 fields), `compose_cone_over_graph_with(.., mode, membership)` (`:2886`),
  `ConeDerivedStore::row_ids()` (`cone_derived.rs:452`, `#[cfg(test)]`), the
  `#[cfg(test)] test_facts` module (`cone_derived.rs:868`), `DetConeInputs` /
  `det_cone_inputs` (`detector_context.rs:1111`/`:1119`), `model_with_tables`
  (`capability_cone.rs:5843`), the `scc_set_tests` module (`capability_cone.rs:6382`).

## Review Focus

1. **A recursive member outside `nodes`** (Tarjan found it, the cone never emits it):
   it must get no row and must not be counted in `recursive_members_emitted`. T11 pins
   it; Task 2's code must keep the `routine_ids` filter before both the set fold and the
   own-entry count.
2. **A recursive SCC that is a root and has no member in `routine_ids`**: no set should be
   built for it (spec §4.2: "with at least one member in `routine_ids`"). Task 2 adds
   `scc_set_root_with_no_emitted_member_builds_nothing` (below). *As executed:* that
   fixture is impossible. Tarjan starts only from `nodes`, so an SCC with no member in
   `nodes` is always reached from a node outside it and is never a root. It was replaced
   by `scc_set_none_for_an_scc_with_no_emitted_member` (a NON-root SCC with no emitted
   member: no set; in `scc_set_tests`).
3. **`Both` mode** (policy's RAW build): the raw `Vec` must stay byte-identical while the
   derived row now comes from the set. T9's raw half pins it.
4. **Census `rest=` going negative** when a new timing is counted under two components:
   `census_rest_is_never_negative` (`tests/cli/cli_build_counters.rs:733`) must stay
   green in every task (spec §4.5 overlap rule).
5. **Interning order** (the store's `ResId`s) changes with the fold order; no output may
   read it. Goldens and the CDO SARIF identity (bench Part 1) are the guard; no new test.

---

## Task 1 — the instrument, no behaviour change

Spec §4.5 ("New, added by plan Task 1"), §5.9, §6. Not small.

**Files:** `src/engine/l4/capability_cone.rs` (`ComposeStats`, `compose_inherited_cones`,
`ConeOutcome`), `src/engine/l5/detector_context.rs` (`cones_census`: new atomics and the
`cone_scc:` line); the five switch-dependent tests are moved out of `scc_set_tests` (the
tests' own module, not `capability_cone::tests`) to a file outside the repo (Step 1).

**Interfaces:**
- Produces: `pub(crate) struct ComposeStats` (the 12 stub fields, now `pub(crate)`),
  `ConeOutcome.stats: ComposeStats` (`pub(crate)`; *as executed* `#[cfg(test)]
  pub(crate)` on `ConeOutcome`: production reads the counts only through
  the census), counted on every call;
  census atomics `CONE_SCC_RECURSIVE`, `CONE_SCC_MEMBERS_ALL`, `CONE_SCC_MEMBERS_EMITTED`,
  `CONE_SCC_ROOTS`, `CONE_SCC_SELF_LOOPS`, `CONE_SCC_MAX` (`max`), `CONE_SCC_SIZE_HIST:
  [AtomicU64; 5]`, `CONE_SCC_OWN_ENTRIES`, `CONE_SCC_SETS_BORROWED`,
  `CONE_SCC_SETS_BUILT`, `CONE_SCC_SET_ENTRIES`, and report-only timings `WALK_TOTAL_NANOS`,
  `ROOT_SET_PROBE_NANOS`, `ROOT_SET_ENTRIES` (none of them in `component_snapshot`).

Red tests it turns green: **T1 `scc_set_rows_hand_stated`, T10
`compose_stats_counts_scc_shapes`, T11 `scc_set_skips_members_outside_nodes`.** T2, T3b,
T5, T6 and T9 are parked outside the repo during this task (Step 1) and come back in Task 2.

- [ ] **Step 1: Park the five switch-dependent tests outside the repo.** They can only go
  green at the switch, and `#[ignore` is refused (`scripts/agentflow/protect.py:16`,
  `ignore-attribute`, checked by `check-diff` before every task commit). With Edit, cut
  each test, from its `#[test]` line through its closing `}`, out of `scc_set_tests` in
  `src/engine/l4/capability_cone.rs`. Write them, byte for byte and in this order, to
  `/work/alsem/.agent/runs/20261020-113707-ad56ee/parked-switch-tests.rs` (outside the
  repo): T2 `scc_set_root_and_self_loop`, T3b `scc_set_own_entries_scale`, T5
  `scc_set_479_cycle_scales`, T6 `scc_set_chain_and_fan_out`, T9
  `derived_only_runs_no_member_walk`. Their helpers (for example `t3b_src`, `t5_graph`,
  `raw_row`) stay in the module, and so does every other test. If a helper is left with
  no caller, dev clippy `-D warnings` fails; then the helper moves with its test, placed
  before the tests in the parked file. Record in the ledger Timeline the parked file's
  `sha256sum` and its `grep -c '#\[test\]'` count (5). The conductor holds the file between
  tasks. Task 1 commits without these five tests. *As executed:* done; the five were
  parked between Tasks 1 and 2 and restored byte for byte in Task 2 Step 1.
- [ ] **Step 2: Run red.** `cargo test -p al-sem --lib scc_set_tests` → T1 fails on
  `recursive_sccs` (0 ≠ 1), T10 on `recursive_sccs` (0 ≠ 2), T11 on
  `recursive_members_all` (0 ≠ 2); every other test in the module passes.
- [ ] **Step 3: Count the stats** in `compose_inherited_cones`, per SCC entry (spec §4.5
  definitions): `recursive_sccs`, `recursive_members_all` (every member of a recursive
  SCC), `recursive_members_emitted` (those passing `routine_ids`), `recursive_roots`
  (`remaining_uses[i] == 0`), `self_loops` (recursive with one member),
  `max_recursive_members`, `size_hist` buckets `[1, 2, 3-9, 10-99, ≥100]`,
  `own_entries_recursive` (Σ `own_entries(m).len()` over emitted recursive members).
  `member_walks` stays as the stub counts it. `scc_sets_*` and `scc_set_entries_folded`
  stay 0 (Task 2 counts them). Make the struct and its fields `pub(crate)` and add
  `pub(crate) stats` to `ConeOutcome`; the census reads every field, so dead-code clippy
  passes.
- [ ] **Step 4: Census.** Feed the atomics from `ComposeStats` once per
  `compose_inherited_cones` call, gated (`add_gated`/`max`). Under the census only:
  `WALK_TOTAL_NANOS` += each recursive member's walk ns (read from the same `Instant` as
  `BFS_WALK_NANOS`, a report, not a component); **`root_set_probe`**: for each ROOT
  recursive SCC with an emitted member, `fact_cone_for_scc(&members, succ_ids,
  &fact_cones, direct, None)`, add its `len()` to `ROOT_SET_ENTRIES`, time it into
  `ROOT_SET_PROBE_NANOS`, drop it. Print one line right after the `compose:` line
  (`detector_context.rs:566`), exact text from spec §4.5 at Task 1 (*as executed,* Task 2
  deletes the two `root_set_*` fields; the delivered line ends at `walk_total=`,
  is the `cone_scc:` format string in `cones_census::report`, and is pinned by
  `census_cone_scc_line_counts_the_recursive_sccs` in `tests/cli/cli_build_counters.rs`):
  `[cones-census]   cone_scc: recursive={} members_all={} members_emitted={} roots={} self_loops={} max={} sizes=1:{} 2:{} 3-9:{} 10-99:{} 100+:{} member_walks={} own_entries={} sets_borrowed={} sets_built={} set_entries={} walk_total={:.1}ms root_set_probe={:.1}ms root_set_entries={}`.
  No new timing is added to a component (spec §4.5 overlap rule); the probe adds to
  `rest=`, which stays non-negative.
- [ ] **Step 5: Run green.** `cargo test -p al-sem --lib scc_set_tests` → T1, T10, T11
  pass; T7 passes; T8 skips. `cargo test --test cli cli_build_counters::` green
  (`census_rest_is_never_negative` included). `scripts/check-goldens` green (no golden
  moves).
- [ ] **Step 6: Proofs.**
  - T10 (spec §5.9): break `recursive` → `recursive || members.len() == 1` in the
    shape count → fails on `recursive_sccs == 2`.
  - T11 (spec §5.5b): delete the `if !routine_ids.contains(m) { continue; }` at the
    member loop → fails on the `row_ids()` assertion.
  - T1: the spec's T1 break (§5.2) acts on the set, so it is proved in Task 2. Here only
    its new precondition is proved: skip the `recursive_sccs += 1` → fails on
    `recursive_sccs == 1`.
- [ ] **Step 7: Gate and commit.** `scripts/ci-steps task-affected`. Commit message:
  `#160 step 7a Task 1: Cone-SCC census and ComposeStats (no behaviour change)`. Record
  the commit sha in the ledger Timeline as **the Task-1 sha**.

## CHECKPOINT — the stop rule (conductor, not an implementer)

Spec §6. The conductor runs, under the exclusive bench slot:
`agentflow run --name bench --timeout 30 -- bash /work/bench/160-7a-t1/stoprule.sh <Task-1 sha>`
(the script is spec §6's block, copied verbatim to that path; it refuses any other commit
and a dirty `src crates Cargo.toml Cargo.lock`). From its 6 + 6 samples: `Pw`, `Bw`
(`bfs_walk`), `Bc` (`context.capability_cones` band), `R` (`root_set_probe` median).

- **S1** `members_emitted` = the `compose:` line's `x` count; `members_all ≥
  members_emitted`; `max = max_scc_members` when the latter > 1.
- **S2** `R ≤ 500 ms`.
- **S3** `Pw − 1000 − Bw ≥ 2 × Bc`.

Any rule false, or the script exits non-zero: **the step stops** here and goes back to
the panel (S3 false means the step is re-labelled non-speed before code, epic I6). The
conductor records the readings in the ledger. Task 2 starts only after S1-S3 hold.

## Task 2 — the switch: one set per Cone-SCC

Spec §4.2, §4.5 (timers, counters that move), §4.7. Not small. Depends on the checkpoint.

**Files:** `src/engine/l4/capability_cone.rs` (`compose_inherited_cones`'s recursive
branch; the `scc_set_tests` attributes; one new test).

**Interfaces:**
- Consumes: Task 1's `ComposeStats`, census atomics.
- Produces: production `PerScc` behaviour; `ComposeStats.scc_sets_borrowed`,
  `scc_sets_built`, `scc_set_entries_folded` counted. `compose_cone_over_graph` keeps its
  signature and passes `PerScc`; `project_r3a3`'s direct call (`:4097`) passes `PerScc`.

Red tests it turns green: **T2, T3b, T5, T6, T9** (and the Review Focus test below).

- [ ] **Step 1: Red.** Before any other edit: check that
  `sha256sum /work/alsem/.agent/runs/20261020-113707-ad56ee/parked-switch-tests.rs`
  equals the sum Task 1 recorded. Put its content back, unchanged, at the end of the
  `scc_set_tests` module with one Edit. Then prove the copy is byte-identical: write the
  inserted lines to a scratch file (a line-range read of `capability_cone.rs`, from the
  first inserted line to the last) and `cmp` it with the parked file. `cmp` must be
  silent, else stop. Then add `scc_set_root_with_no_emitted_member_builds_nothing`
  (*as executed:* impossible as written, since Tarjan starts only from `nodes`; replaced
  by `scc_set_none_for_an_scc_with_no_emitted_member`, `nodes = [A, C]`, edges A → X,
  X → Y, Y → X, X → C, asserting no set and `row_ids() == {A, C}`;
  in `scc_set_tests`). As first written: synthetic graph, `nodes = [C]`,
  edges X → Y, Y → X, X → C (X, Y not in `nodes`; the root SCC {X, Y} has no emitted
  member); assert `stats.recursive_roots == 1`, `stats.scc_sets_built == 0`,
  `row_ids() == {C}`. Run `scc_set_tests`: T2 fails on `scc_sets_built` (0 ≠ 1), T3b on
  `scc_set_entries_folded` (0 ≠ 8), T5 and T9 on `member_walks` (479 ≠ 0), T6 on the set
  count (0 ≠ 20). The new test passes on today's code (no set is built yet); its proof
  is in Step 5.
- [ ] **Step 2: Implement `PerScc`** in the recursive branch, exactly spec §4.2:
  1. once per recursive SCC with an emitted member, when `mode.wants_derived()`: borrow
     `fact_cones[i]` (non-root, `scc_sets_borrowed += 1`) or build
     `fact_cone_for_scc(&members, succ_ids, &fact_cones, direct, None)` into a local kept
     until the member loop ends (root, `scc_sets_built += 1`); timed under
     `BFS_WALK_NANOS` only. Delete Task 1's `root_set_probe` (this build replaces it;
     `ROOT_SET_PROBE_NANOS`/`ROOT_SET_ENTRIES` and their line fields go too; spec §4.5);
  2. per emitted member, after `begin_routine`: `fold_fact(&entry.rep)` for every set
     entry, then for every `own_entries(m)` entry; `scc_set_entries_folded += set.len()`;
     timed under `BFS_FOLD_NANOS`, which keeps the parent's coverage (spec §4.5: in raw
     modes it also wraps `emit_picks`);
  3. `inherited_facts_by_bfs` runs only when `mode.wants_raw()` (or under
     `PerMemberWalk`); under `PerMemberWalk` the derived row comes from the picks as
     today. In this task `emit_picks` keeps its `derived` argument and is called with a
     fold only under `PerMemberWalk` (Task 3 removes the argument).
- [ ] **Step 3: Green.** `scc_set_tests` all pass (T8 skips). `cargo test --test cli
  cli_build_counters::` green. `scripts/check-goldens` green. `cargo test -p al-sem --lib`
  green. Clippy dev `-D warnings` via the gate.
- [ ] **Step 4: Census.** Step 3's `cli_build_counters::` run includes
  `census_rest_is_never_negative` (it sets `ALSEM_CONES_CENSUS=1` itself). The CDO
  counters that move (spec §4.5: `BFS_PICKS` → 0, `MERGE_CALLS`/`MERGE_TIEBREAKS` rise by
  the root-set merges) are read at the bench, not here.
- [ ] **Step 5: Proofs** (spec §5; every break applied with Edit, failing at the named
  assertion):
  - T1 §5.2: skip the successor merge in `fact_cone_for_scc` (`:2254-2289`) for a
    recursive SCC → A's literal row fails on `t/C`.
  - T2 §5.3: use an empty set for a root → the literal rows fail on `t/S`; *as executed*
    it fails first on X (the `"{m}: writes"` assertion, `m = X`): the test checks X,
    Y, S in that order, and both X and Y lose `t/S`.
  - T3 §5.4: Break 1 `own_entries(&members[0])` → the literal row assertion fails;
    Break 2 skip the own fold → Q loses `[T]`.
  - T3b §5.4: skip the own fold → rows fail; `MAY_COMMIT` and set-count assertions
    still hold (record both).
  - T4 §5.5: delete `anchor.is_some() && ` at `:2272` → A's and B's literal rows fail.
  - T5 §5.6: route `DerivedOnly` through the walk → fails on `member_walks == 0`.
  - T6 §5.6: build the set per member → fails on the set count (40 / 3).
  - T9 §5.7: (a) `compose_cone_over_graph` passes `PerMemberWalk` → fails on
    `member_walks == 0`; (b) drop `sort_inherited` in `emit_picks` → fails on A's literal
    raw order.
  - `scc_set_root_with_no_emitted_member_builds_nothing` (*as executed:*
    `scc_set_none_for_an_scc_with_no_emitted_member`): drop the "has an emitted member"
    condition → fails on the no-set assertion.
- [ ] **Step 6: Gate and commit.** `scripts/ci-steps task-affected`. Commit:
  `#160 step 7a Task 2: fold recursive members from one set per Cone-SCC`.

## Task 3 — the old fold reachable only from tests (small)

Spec §4.2 step 3, §4.3. Depends on Task 2.

**Files:** `src/engine/l4/capability_cone.rs` (`emit_picks`, the recursive branch, doc
comments).

**Interfaces:** Produces `fn emit_picks(subject: &str, picks: Vec<(&CapabilityFact,
&TypedOutEdge)>, mode: ConeOutput) -> Vec<CapabilityFact>` (no `derived` parameter).

No test turns green, none is added or changed; every test stays green. (The tests that
pin this code, T7's oracle and T9's raw half, got their proofs in Task 2.)

- [ ] **Step 1:** Delete `emit_picks`' fold half and `derived` parameter. Move the
  oracle's fold of the picks to a loop at the call site inside the `PerMemberWalk` match
  arm, which is `#[cfg(test)]` with its variant. Doc comments: `RecursiveMembership` and
  the arm state the deletion point (spec §4.3: deleted by step 7b's last commit after its
  CDO oracle passes; if 7b is dropped, by the next step that edits
  `compose_inherited_cones`, step 6 at the latest). Remove the stub's `shortcut:` comment
  on `_membership` (the parameter is now used; rename it `membership`).
- [ ] **Step 2: Check non-test builds** see no dead code: `scripts/ci-steps clippy
  > <log> 2>&1` (release, `-D warnings`) exits 0.
- [ ] **Step 3: Green.** `cargo test -p al-sem --lib scc_set_tests` and `--lib` green;
  goldens green.
- [ ] **Step 4: Re-run T9's proof (a)** (production passes `PerMemberWalk` → fails on
  `member_walks == 0`), since this task moved the code it guards; record it in the
  ledger row of Task 2 with this commit's sha.
- [ ] **Step 5: Gate and commit.** `scripts/ci-steps task-affected`. Commit:
  `#160 step 7a Task 3: the per-member fold only under the test-only PerMemberWalk`.

## Task 4 — the oracle on CDO (small)

Spec §5.8 (T8), A7a.2. Depends on Task 3.

**Files:** none changed unless the run fails; the ledger Timeline line.

- [ ] **Step 1:** `CDO_WS=/work/src/cdo-baseline/Cloud cargo test --profile cdo -p al-sem --lib scc_set_matches_member_walk_on_cdo -- --nocapture > <log> 2>&1`;
  require BOTH lines `scc-set oracle on CDO (cross-app): ` and `scc-set oracle on CDO
  (single-app): ` (T8 prints one per universe, its `scc-set oracle on CDO ({mode}): …`
  format) and
  `test result: ok. 1 passed`, with the same three `grep -q` as spec §8 Part 1. Required:
  `recursive_members_emitted == 714` cross-app; the line's classes and wall recorded in
  the ledger (spec §5.8: T8's cost goes into the numbers file).
- [ ] **Step 2: Proof** (spec §5.8): the T3 Break 2 (skip own entries) on the CDO run →
  record red or green. If green on CDO, use the T1 break, and record the null result as a
  stated LIMIT. Revert with Edit; re-run green.
- [ ] **Step 3: T7 limit recorded.** From the attempt-2 run: over all 249 fixture dirs,
  both modes, only multi-member (4) and roots (4) are non-zero; T7 asserts those two as
  minimums. Copy the T8 class line next to it.
- [ ] **Step 4:** No commit unless a file changed (a failing run stops the task and is
  reported, never patched around).

## Task 5 — CHANGELOG and the numbers-file skeleton (small)

Gate-inert prose. Depends on Tasks 1-4.

**Files:** `CHANGELOG.md`, `docs/measurements/2026-10-20-issue-160-step7a.md` (new).

- [ ] **Step 1: CHANGELOG** `[Unreleased]` → `### Changed`, entry "#160 step 7a" (Part of
  #160, no closing keyword): recursive Cone-SCC members fold from one set per SCC under
  `DerivedOnly` and `Both`; raw output unchanged; no golden moves; the old path is
  test-only (`RecursiveMembership::PerMemberWalk`) and its deletion point; the census:
  new `cone_scc:` line, `BFS_PICKS` 0 in `DerivedOnly`, merge counters rise by the
  root-set merges, `DIRECT_NANOS` now also times the publisher-map build
  (`det_cone_inputs`) (*as executed:* that widening was reverted; `direct=` times the
  per-routine loop only, as at step 0, in `det_cone_inputs`, so the CHANGELOG does
  not state it); M3 deferred to step 6; `fold_row` not in 7a. **Coverage limit:**
  over the 249 fixture dirs only the multi-member (4) and root (4) classes are non-zero;
  self-loops, non-roots, successors, guarded successors and own entries are covered only
  by the hand tests (T1, T3, T4) and by T8 on CDO; the gap deviation (as step 3).
  Measured values as `<bench>` placeholders.
- [ ] **Step 2: Numbers file skeleton**, structured like
  `docs/measurements/2026-10-20-issue-160-step4.md`: protocol table (machine; parent
  `d37e8634`, the spec's bench parent, source-identical to this branch's base `68253a8e`:
  `git diff --stat d37e8634 68253a8e -- src crates tests Cargo.toml Cargo.lock` is empty;
  both shas from the bench's `$B/shas.txt`; profile, harness, commands);
  sections for the stop-rule reading (from the checkpoint), Part 1 (SARIF `cmp` both
  scopes, histogram SHA, retained buckets head ≤ parent, T8 line), Part 2 (A7a.4
  `bfs_walk` head median ≤ 1,000 ms; A7a.5 with its void check `Pw − 1000 − Bw ≥ 2 ×
  Bc` first, then `Pc − Hc ≥ Pw − 1000 − Bw − Bc`, and the literal form reported;
  `bfs_fold` reported), Part 3 (reported walls, polled RSS), every run listed, and a
  verdict. Every number is `<bench>` until the conductor's run fills it.
- [ ] **Step 3: Gate and commit.** `scripts/ci-steps task-affected` (docs only). Commit:
  `#160 step 7a Task 5: CHANGELOG and numbers-file skeleton`.

---

## The CDO bench (conductor only)

The 3-part CDO bench of spec §8 runs **once, after the final panel**, run by the
conductor through `agentflow run --name bench --timeout 45 -- bash <part>.sh` (Part 1
takes the head sha as its argument). Never by an implementer, never beside a build.
It depends on **Tasks 1-3** (the code it measures) and **Task 5** (the numbers file it
fills); Task 4 is independent of it. The conductor fills the `<bench>` placeholders in
the numbers file and the CHANGELOG from its outputs and commits them.

## Acceptance map (spec §7)

A7a.1 Task 2 (T5, T6, T9) · A7a.2 Tasks 3, 4 · A7a.3 every task (goldens), bench Part 1
· A7a.4 bench Part 2 · A7a.5 checkpoint S3 and bench Part 2 · A7a.6 bench Part 1 ·
A7a.7 Task 2 (`fold_row` absent), Task 5 · A7a.8 Tasks 2, 4 (T7, T8) · A7a.9 Task 2 (T3b,
T4, T5, T6) · A7a.10 Task 1 and the checkpoint · A7a.11 Task 5 (CHANGELOG) · A7a.12 the
branch gate (`scripts/cdo-gate`, T8 included).
