# CDO gate: how much wall time do repeated model builds cost? (TLS-37 census)

TLS-37 asked whether building each CDO model once per test process would cut the serial
gate's wall time. The orchestrator set a stop line: under ~60 s of wall time saved on the
critical path, stop. **Result: under the line. TLS-37 stops at the census.**

## Method

- Each of the eight longest lib CDO tests ran alone (`--exact`, `--test-threads=1`), with
  `ALSEM_TRACE=1`, on the `cdo` profile binary from master `af21566b` (before ENG-27). Run
  on server1 on 2026-11-13.
- The traces give each test's top-level spans, which split model-build time from check time.
- Wall times are noisy. Another lane was building during the first three tests (host load
  87-233). The build/check split is what decides, and it holds up under noise because the
  builds are the same code in every test.
- The gate's own timing comes from the TLS-36 landing gate's `lib_cdo` log
  (`cdo-gate-logs/20261113-114730-216514`): the five long tests at 4 threads took 204 s, and
  the other 12 took 57 s.

## Builds per test (alone)

| test | wall alone | build spans | what it builds |
|---|---:|---:|---|
| `derived_only_matches_raw_fold_on_cdo` | 131 s | ~19 s + ~6 s | cross-app, then single-app; the rest is a 70 s `cone.walk` |
| `parallel_solve_equals_the_serial_walk_on_cdo` | 110 s (42 s in the TLS-32 census) | 48 s `crossapp.base_total` | cross-app base, then the cones |
| `cdo_i369_no_path_rows_and_fact_ids` | 62 s | ~37 s | cross-app **twice** (once to find ids, once scoped to them) |
| `cdo_crossing_census` | 130 s | ~38 s | cross-app **twice**, same shape |
| `retained_emission_equals_a_composition_on_cdo` | 127 s | 18 s `crossapp.base_total` | through `Revision` (the resident path), then fold, emit and cones |
| `relation_equals_the_legacy_base_on_cdo` | 27 s | ~22 s | single-app and cross-app |
| `parallel_direct_facts_equal_serial_on_cdo` | 27 s | ~22 s | single-app and cross-app |
| `cdo_light_and_full_profiles_give_the_same_report_and_answers` | 25 s | ~11 s | three preflight builds (the profiles it compares) |

A cross-app build alone is ~19 s (program build 5, dependency bodies 6, model assembly 5,
calls 2). A single-app build is ~4-6 s.

## Why sharing gains little wall time

- **The long run (204 s).** libtest starts four tests together at t = 0. With a shared
  build, all four would still wait for that one build before their checks start, so none
  starts its check earlier. What sharing removes is the CPU of three extra concurrent
  builds, a contention gain of perhaps 10-20 s. The critical path is `parallel_solve`
  (74 s in the gate), then `retained_emission` (~130 s), which starts when the first
  slot frees. `retained_emission` builds through `Revision`, the path it exists to test,
  so it cannot take a shared model.
- **The second run (57 s).** `relation_equals` and `parallel_direct_facts` run side by side
  and spend most of their time building. Sharing might take this run to ~40 s, a gain of
  ~15-20 s.
- **Total:** ~25-40 s at best, under the 60 s line. A quiet re-run would make the numbers
  cleaner, but it cannot move the decision: the structure (everyone starts at t = 0) caps
  the gain, not the noise.

## What else the census shows (not done)

1. **Scheduling, not sharing.** The long run's 204 s is `parallel_solve` + `retained_emission`
   one after the other. If `retained_emission` started at t = 0 (for example in its own
   invocation running alongside the other four at 3 threads, holding the same memory as now),
   the run would end near the longest single test (~161 s): ~40 s, with no Rust change. Below
   the line alone, and the gate's speed is noisy at that scale. **Tried as TLS-40 and dropped**
   (see below).
2. **The double build in `cdo_i369` and `cdo_crossing_census`.** Each builds the cross-app
   model once to find two routine ids, drops it, then builds it again scoped to those
   routines (`tree_over`): ~19 s of CPU each. They are not on the critical path, so the gate's
   wall time would not move.
3. `parallel_solve`'s alone time moved from 42 s (TLS-32 census) to 110 s here under load 233;
   its cones phase is the most load-sensitive part of the run.

## TLS-40: `retained_emission` first, tried and dropped

**What was tried.** In `scripts/cdo-gate`'s `lib_tests`, at 2 or more threads,
`retained_emission` ran alone in its own `cargo test` invocation from t = 0, beside the other
four long tests at one thread fewer. The same number of tests ran at once, so memory was
unchanged. It came with a `gate_scripts` test and three discrimination proofs. The change is on
the unmerged branch `tools/tls-40-retained-first` (`fc1799f2`).

**The A/B.** `lib_tests` was sourced from master's script (A, `db06b145`) and from the branch's
(B), with one `cdo`-profile test binary (only the script differed), 4 threads, no pin, in ABBA
order. Run 2026-11-13 on server1, with engine's builds held. It started at load 8.6 with 50 GiB
free, then outside load (another project) rose to 26-30 for the last three runs.

| run | lib_cdo wall | long run | load start → end |
|---|---:|---|---|
| A1 | 289.5 s | 231.6 s (5 tests, 4 threads) | 8.6 → 26.1 |
| B1 | 255.6 s | `retained_emission` 159.0 s; the other four 196.1 s | 26.1 → 29.7 |
| B2 | 257.9 s | `retained_emission` 156.6 s; the other four 198.2 s | 29.7 → 29.9 |
| A2 | 260.0 s | 201.5 s | 29.9 → 27.8 |

The rest of the phase (12 tests) took 56.6-58.4 s in every run.

**Result.** A averaged 274.8 s and B 256.8 s: −18 s. But A2 against B is only −2 to −4 s, so
most of the gap is A1, which began on a quiet host. That is within the noise and below the 40 s
bar.

**Why it fell short of ~40 s.** Giving `retained_emission` a slot of its own leaves the other
four on 3 threads, and they become the new critical path at ~197 s (`derived_only` alone is
~131-161 s, plus the others queued behind it). `retained_emission` alone also took ~157 s here,
not the ~130 s it took in the gate.

**Decision (orchestrator, 2026-11-13; mantra 13, 18):** dropped. The gain is within the noise
and the script grows.
