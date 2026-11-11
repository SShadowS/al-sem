# Where the task gate's test time goes (TLS-07)

2026-11-10, master `4abbed93`, server1 (22 logical CPUs), the task gate's own command:
`cargo nextest run --workspace` (as `scripts/ci-steps test` runs it), dev profile, warm build.
Every run went through `crate-split-phase0/timeit.py`, which waits up to 3 minutes for a host at
least 90% idle; no run was flagged noisy. Another project on server1 loaded the host between
and during runs (the 1-minute load reached 320), so absolute walls swing by about 20 s. The
comparisons below therefore use alternating runs and the tail, which host load does not move.
Data: `task-gate-test-time/` (timing records `runs.jsonl`, the per-test report `report.txt`,
two timelines, the scripts that made them).

## The answer in short

- **The run is limited by its tail, not by its total.** With the current config every one of
  the 22 slots is busy until about 88-109 s, and then the run waits about **20 s** (19.8-23.3 s
  in 4 runs) for 3 long tests that started late: the `al-syntax` test
  `attribute_flow_scales_linearly` (22 s, started at 88 s, because `al-syntax` is scheduled
  last) and two `tests_rule_b` lib tests (about 31 s each, started at 62 s). The lib binary and
  `al-syntax` have no nextest priority; `cli`, `r3` and `r4` do.
- **Giving the 14 lib and `al-syntax` tests of at least 15 s a priority removes the tail**
  (3.1-3.9 s left in 6 runs). Median wall **118.8 s -> 98.6 s** (7 runs against 6); alternating
  pairs: -12.0, -13.3 and -23.3 s. After it the run ends at its floor (the summed test time over
  22 slots), so further savings need less total work.
- **The total is spread out.** 5,754 tests, about 2,300 s of test time (median per test over the
  first 3 runs). The top 20 tests are 28% of it, the 44 tests of 10 s or more 41.5%, the 95 of
  5 s or more 57%. By binary: `cli` 48%, the lib 36%, `r3` 7%, `r4` 3%, everything else under 2%
  each.

## Measurements

| Runs | Config | Wall (s) | Median |
|------|--------|----------|--------|
| run1-3, A1-A4 | current `.config/nextest.toml` | 130.2, 117.9, 129.5, 108.0, 129.2, 109.8, 118.3 | 118.8 |
| B1-B6 | + priority 60 for 14 tests (`task-gate-test-time/b_override.toml`) | 97.3, 98.9, 113.6, 117.2, 96.5, 95.1 | 98.6 |

Order: run1-3, A1, B1-B3, then A2 B4 A3 B5 A4 B6 alternating. Each test's start and end come from
nextest's JUnit output (a scratch copy of the config with a `junit` path; the repository's config
was not changed). The tail is the time after the last second in which every slot was busy:

| Run | Wall | Floor (summed / 22) | Tail |
|-----|------|---------------------|------|
| A1 | 108.0 | 90.2 | 20.0 |
| A2 | 129.2 | 110.6 | 20.2 |
| A3 | 109.8 | 92.3 | 19.8 |
| A4 | 118.3 | 97.5 | 23.3 |
| B2 | 98.9 | 95.9 | 3.9 |
| B4 | 117.2 | 114.9 | 3.2 |
| B5 | 96.5 | 93.8 | 3.5 |
| B6 | 95.1 | 92.9 | 3.1 |

The floor moves with host load (summed test time 1,984-2,529 s); the tail does not. Children's
CPU time was 1,656-1,719 s per run, against about 2,000-2,500 s of summed test wall time, so
tests spend a quarter or more of their time waiting for a CPU (our own oversubscription, see
proposal 4, and the other project's load).

### Top 20 tests (median of run1-3)

| s | Binary | Test |
|---|--------|------|
| 87.4 | cli | `cli_fingerprint_routine_cross_app::no_dependency_parity_fingerprint_routine` |
| 47.1 | cli | `cli_prove_cross_app::no_dependency_parity_prove` |
| 44.9 | lib | `engine::gate::run::builders_tests::single_app_builders_agree` |
| 41.3 | lib | `engine::l5::digest_cli::tests_rule_b::prove_scoped_equals_unscoped` |
| 39.9 | lib | `engine::l5::digest_cli::tests_rule_b::fingerprint_query_scoped_equals_unscoped` |
| 35.0 | r4 | `r4_cross_app_program::without_dependencies_cross_app_mode_reports_what_single_app_mode_does` |
| 31.0 | cli | `cli_inventory_cross_app::no_dependency_parity_inventory_only` |
| 30.3 | lib | `engine::l5::digest_cli::tests_rule_b::t3_digest_scoped_equals_unscoped` |
| 28.8 | lib | `engine::l4::capability_cone::scc_set_tests::derived_only_matches_raw_fold_on_fixtures` |
| 27.5 | lib | `engine::gate::policy::policy_engine::facts_oracle_tests::policy_selects_the_parents_facts_per_primary_routine` |
| 27.4 | cli | `ids_parent_baseline::parent_baseline_fingerprints_and_stable_ids_are_unchanged` |
| 26.9 | lib | `engine::l5::detector_context::tests_l5::parallel_direct_facts_equal_serial_on_fixtures` |
| 26.6 | lib | `engine::l5::snapshot::step0_sort_tests::t6_every_corpus_app_gives_the_old_table_and_pd_arena` |
| 26.3 | lib | `engine::l4::exec_relation::tests::relation_equals_the_legacy_base_per_field` |
| 25.9 | lib | `engine::l5::snapshot::step6_tree_tests::callsite_resolutions_alone_equal_the_snapshot_rows` |
| 22.1 | al-syntax | `lower::tests::attribute_flow_scales_linearly` |
| 21.5 | cli | `cli_tests_selection::explain_callers_equal_the_cones_direct_callers` |
| 20.3 | cli | `output_invariants::explain_digest_and_prove_give_one_table_answer` |
| 20.1 | cli | `gate_prsummary_differential::gate_exit_code_matrix_matches` |
| 20.0 | lib | `engine::l4::pd_solve::tests::parallel_solve_equals_the_serial_walk_on_every_fixture` |

Most of them walk the whole r0 corpus (220 fixture folders), several in both universes
(cross-app and `--single-app`), building a model per folder, root or format. Under nextest each
test is its own process, so a model can only be shared inside one test function.

## Proposals (no test was changed in this task)

Savings in wall time are for the whole run on 22 slots; a test's own saving counts in wall time
only while the run is tail-bound, and otherwise at about its saved test time / 22.

1. **Priority for the long lib and `al-syntax` tests** (`.config/nextest.toml`, measured).
   Add the override in `task-gate-test-time/b_override.toml` (priority 60, the 14 tests of at
   least 15 s). Measured: tail 20 s -> 3.5 s, median wall -20 s (118.8 -> 98.6). Also give
   `no_dependency_parity_fingerprint_routine` the top priority: with the tail gone it is the
   longest single test (70-87 s; it started at 29 s in B2 and ended last). A name list goes
   stale as tests change; the alternative that needs no list is proposal 3 (shorter tests).
   Expected: **about -20 s**. Cheapest by far: a config change, no test change.
2. **Stop doing the same work twice** (less total, so a lower floor once 1 is in):
   - `no_dependency_parity_fingerprint_routine` runs the whole fingerprint pipeline per folder,
     root, format (json and human) and universe (`tests/cli/cli_fingerprint_routine_cross_app.rs:494`,
     `:522`): the human run only renders the same answer again. Compare json for every root and
     human on a sample. About -40 s of test time.
   - The three `tests_rule_b` tests (`src/engine/l5/digest_rule_b_tests.rs:762`, `:1182`,
     `:1296`) collect the same folders and build the same `AnalysisTarget` per folder and
     universe three times. One loop that builds each target once and runs the three checks
     saves about two thirds of their builds: about -50 to -70 s of test time.
   - `parent_baseline_fingerprints_and_stable_ids_are_unchanged`
     (`tests/cli/ids_parent_baseline.rs:238`) analyzes each fixture three times (SARIF, JSON,
     digest); one analysis rendered twice saves about a third: about -9 s.
   Together about -100 to -120 s of test time, **about -5 s of wall**. Worth doing for the CPU it
   frees (the CDO gate and other lanes run beside the task gate), not for the gate's wall alone.
3. **Shard the long serial tests** (`single_app_builders_agree`, the `tests_rule_b` tests,
   `without_dependencies_cross_app_mode_reports_what_single_app_mode_does`,
   `no_dependency_parity_inventory_only`, `derived_only_matches_raw_fold_on_fixtures`,
   `policy_selects_the_parents_facts_per_primary_routine`): each loops over the corpus on one
   thread, so splitting the folder list into 3-4 `#[test]`s cuts each to about a quarter. It
   saves no total, but it removes the risk proposal 1 handles with a list: no test is long
   enough to make a tail. Expected: no wall saving on top of 1; it makes 1's list unnecessary.
   Care: `derived_only_matches_raw_fold_on_fixtures` asserts corpus-wide totals
   (`src/engine/l4/capability_cone.rs` near `:8419`); a shard needs its own.
4. **Stop the oversubscription.** The two cli parity tests spawn up to 8 threads each
   (`available_parallelism().min(8)`) and `ids_parent_baseline` uses a rayon pool as wide as
   the machine, while nextest already runs 22 processes. Mark them `threads-required` (nextest)
   so their slots are reserved. Expected: small and unmeasured (it trades their speed for
   everyone else's); measure before adopting.
5. **`attribute_flow_scales_linearly`** (`crates/al-syntax/src/lower/mod.rs:8787`, 22 s in a debug
   build): it proves linear scaling on 20,000 attributes and a 16,000-symbol condition. A
   quarter of the sizes still separates linear from quadratic: about -15 s of test time. With
   proposal 1 it no longer sets the tail, so this is a total-work saving only.

Not proposed: moving the parity tests to the CDO gate (they are the only no-dependency parity
checks at the pipeline level, and the CDO gate is already the longest gate).

## Limits

- The host was shared: absolute walls carry about +-15 s of noise; the tail and the alternating
  pairs are the robust numbers.
- Savings for proposals 2-5 are estimates from reading the tests (the call counts and loops cited
  above), not measured. The per-call cost of `run_fingerprint_pipeline` and `run_prove_pipeline`
  was not profiled.
- The task gate's other ~47 s (fmt, clippy, gen-syntax, `cargo test --doc`) was not measured here.

## TLS-11: proposal 1 applied, re-measured (2026-11-11)

Applied in `.config/nextest.toml`: priority 70 for the cli fingerprint parity test, priority 60
for the 14 lib and `al-syntax` tests of `b_override.toml` (15 tests in all; `cargo nextest list`
with the two filters selects exactly them). Every run also writes a JUnit report, and
`scripts/ci-steps test` then runs `scripts/nextest-priority-check`, a warning only.

Measured on master `761581b3` (grammar v5.1.0, `abc77b25`), 6 alternating runs through
`timeit.py`, "before" = master's config, "after" = this branch's (both with a JUnit path; the
engine lane held idle by the orchestrator). Every run was still flagged noisy: the host was 62-74%
idle at each start, from other projects on server1. Records: `task-gate-test-time/tls11-runs.jsonl`.

| Pair | Before wall | After wall | Difference | Before tail | After tail |
|------|-------------|------------|------------|-------------|------------|
| 4 | 149.1 | 129.4 | -19.7 | 22.6 | 3.6 |
| 5 | 150.7 | 122.0 | -28.6 | 27.0 | 3.5 |
| 6 | 138.3 | 122.6 | -15.7 | 24.8 | 3.1 |
| median | 149.1 | 122.6 | -19.7 (of the pairs; -26.5 between the medians) | 24.8 | 3.5 |

An earlier set of 6 the same day, beside the engine lane's builds (host 0-69% idle), gave walls
too noisy to use (one run 243 s) but the same tails: before 26.0, 24.1, 25.7 s; after 8.2, 3.7,
3.0 s. The tail is the robust number: about 21 s of it is gone in every pair, as TLS-07 measured
(20 s to 3.5 s). The run now ends at its floor (the summed test time over 22 slots), so the next
saving is less total work (proposal 2, TLS-12).

Walls on this master are about 30 s longer than TLS-07's on `4abbed93` for both configs (summed
test time 2,550-2,900 s against 1,980-2,530 s). TLS-07's per-test medians against these show a
uniform slowdown (median per-test ratio x1.23; no test over x1.48) while children's CPU rose
only 4% (1,656-1,680 s -> 1,714-1,748 s): host load (TLS-07's runs at least 90% idle, these
62-74%), plus 35 new tests (17 s).

## TLS-12: proposal 2 applied, re-measured (2026-11-11)

Each change shares a build inside one test (nextest runs every test in its own process, so a
`OnceLock` is never shared between tests), keeps every assertion and failure message, and has
its discrimination proofs in its commit message:

- The four `tests_rule_b` corpus tests (TLS-07 named three; `streamed_effects_equal_one_read`
  walks the same corpus) are arms of one test, `scoped_reads_equal_unscoped_on_the_corpus`:
  each target built once per universe, the trees two arms build alike composed once, the dirs
  on 4 threads. 11.5 s alone against about 127 s for the four.
- `no_dependency_parity_fingerprint_routine`: each dir's cases run as one in-process batch (one
  model per universe instead of one per case); every root of every 5th dir also runs each job
  alone through `standalone_output`, the path `alsem fingerprint` runs, because a batch merges
  its jobs' plans and a cross-app-only fault of a one-job view could pass it. 87 s -> 21 s alone.
  Limit: the other four dirs in five are compared through the batch only.
- `ids_parent_baseline`: the SARIF and JSON analyses of each fixture run as one batch; the
  digest call is unchanged.

Reviewed by both panel reviewers (the fingerprint change: sol DO NOT APPROVE then APPROVE, Opus
APPROVE twice; `.agent/tls12/`). Measured on 3 alternating pairs from `cargo nextest archive`
builds of TLS-11's head (before) and TLS-12's first version (after; the standalone sample of
every 5th dir came after the timing and adds about 6 s to one test), the engine lane held,
every run flagged noisy (host 0-65% idle). Records: `task-gate-test-time/tls12-runs.jsonl`.

| Pair | Before CPU | After CPU | Before wall | After wall | Difference |
|------|------------|-----------|-------------|------------|------------|
| 1 | 1,766 | 1,546 | 154.5 | 123.0 | -31.5 |
| 2 | 1,750 | 1,547 | 136.5 | 122.5 | -14.0 |
| 3 | 1,778 | 1,684 | 189.7 | 157.2 | -32.5 |

Children's CPU per run is down about 200 s (12%; pair 3 ran on a saturated host), the summed test
time x0.91, the median wall 154.5 -> 123.0 s. The tail stays at its floor on both sides
(3.0-4.3 s): the run is still bound by its total work.

**Proposals 3-5 after TLS-12.** The longest tests are now `attribute_flow_scales_linearly`,
`single_app_builders_agree` and `no_dependency_parity_prove` (22-49 s on a quiet host). The
last is the fingerprint test's twin (`run_prove_pipeline` per dir, root and universe) and takes
the same batch treatment: the best next step (about -35 s of test CPU). Proposal 5 (shrink
`attribute_flow_scales_linearly`, one debug-build test of 22 s) still saves about 15 s of CPU.
Proposal 3 (shard the serial corpus tests) saves no CPU; with the tail at its floor it buys
nothing until a test sets the tail again, which `scripts/nextest-priority-check` reports.
Proposal 4 (`threads-required`) stays unmeasured and small.
