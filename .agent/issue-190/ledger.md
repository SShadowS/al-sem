# Issue #190 ledger: attempt 1 (perf_bounds magnitude gates fail on runner noise)

## Claim

```json
{"run_id":"20261021-143757-e143e4","issue":190,"attempt":1,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/190-agent-discovery-tests-perf-bound-a1","worktree":"/work/al-sem-issue-190-a1","body_hash":"8e3e348d1554d6bb80a3fe66e3449c721babe6dc7aa3320525df354d6d3756c5","reconciled":[]}
```

Issue body hash: `8e3e348d1554d6bb80a3fe66e3449c721babe6dc7aa3320525df354d6d3756c5`.
Base `617b402f853664cf1cec8d049d85561d66ac8aa2` (make-worktree, origin/master). Grammar `4d75846c`.
Picked by the operator (2026-10-21): "fix the CI so other tasks don't get blocked" — #190 was filed by #111's run after
`perf_bounds` failed PR #187's CI twice. Reviewers: the operator's standing stand-ins, gpt-6.1-sol (slot astra) and Opus
(slot flash); attest with `--substitute astra=sol --substitute flash=opus`. Loop budget: claim 2 of 10.

## Classification

**Bounded.** The change is confined to `tests/perf_bounds.rs`: the magnitude gates judge a noise-sensitive statistic.
No production code, no golden, no CDO number can move.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 The failure is noise on an unchanged code path | held | PR #187 (a `--diff` parser change that never reaches L4's summary solve) failed CI twice, only on `compute_summaries_v2_within_bound`: run 39611462366 `median=231.830322ms bound=230ms samples=[183.77, 266.94, 231.83, 409.95, 147.33]ms`; run 39612982728 `median=243.029519ms … samples=[151.57, 166.38, 243.03, 246.39, 319.29]ms`; the same head passed `perf_bounds` locally (`ci-steps all`) |
| P2 The noise is one-sided, and this file already says so | held | `tests/perf_bounds.rs:428-446` `min_of`'s doc: noise on a shared machine is ONE-SIDED (an interrupted run is slower, never faster), so the minimum is the maximum-likelihood estimate and a median of a noise-skewed set is itself skewed; the `compute_all` gates were moved to `min_of` for exactly this (`:224-282`, v1-v4 history) |
| P3 The minimum clears the bound by a wide margin on the failing runs | held | min of the two CI runs: 147.33 ms and 151.57 ms against 230 ms (1.5x headroom), while the medians missed by 1.8 and 13.0 ms |
| P4 Ten magnitude gates still use `median` | held | `grep -n "let m = median"`: lines 617, 639, 669, 707, 744, 947, 1007, 1071, 1143, 1261; only `compute_all` uses `min_of` (:777, :833-834) |
| P5 Local headroom, every gate (release, dev container, `--nocapture --test-threads=1`) | held | `perf-local.log`, 12 passed. `compute_summaries_v2_bundle(1000)` median 44.77 ms (samples 43.60-46.39), bound 230 ms; `recursive_scc_db_effects` median 108.58 ms / 650; `build_full_1000` 70.11 ms / 6 s; `build_full_100` 6.20 ms / 1.5 s; `incoming` 2.23 ms / 75; `outgoing` 7.9 µs / 3 ms; `prepare` 2.1 µs / 3 ms; `rung1` 5.74 ms / synthetic 100; `rung1_scoped` 1.23 ms / 65; `rung2` 46.49 ms / 750; `compute_all` (already `min_of`) 6.86 ms / 150, scaling ratio 4.479 / 7 |
| P6 How much slower the CI runner is for the failing gate | measured | runner min 147.33 / 151.57 ms vs local min 43.60 ms → observed elapsed-time ratio ~3.4x; runner medians 231.8 / 243.0 vs local median 44.8 → ~5.3x. The ratios mix machine speed with sibling contention (P7) and do not isolate either. The bound (230 ms = 3x a 2026-07 dev-machine median of ~76 ms) leaves ~1.5x headroom over the runner's MIN, ~0.95-1.0x over its MEDIAN |
| P7 CI runs the 12 tests of this binary IN PARALLEL, and the siblings' contention is sustained, not transient | held (panel round 1, both reviewers; measured) | `scripts/ci-steps:176` `cargo test --release --test perf_bounds` (default libtest parallelism) on a 4-vCPU `ubuntu-latest`. Local CI-shape A/B, binary pinned to 4 cores (`taskset -c 0-3`), 5 full unfiltered runs each (`perf-4core-par.log` / `perf-4core-ser.log`, run dir): `compute_summaries_v2` per-run min 54.1-70.8 ms / median 60.9-93.1 ms in parallel vs min 44.1-47.2 / median 44.8-47.7 serial; `build_full_1000` min 196-255 vs 108-121; `build_full_100` median 17.5-41.5 vs 8.3-11.7; `recursive_scc` min 121-137 vs 106-120. Parallel raises even the MINIMUM (every sample is under load), which `min_of` cannot undo (`tests/perf_bounds.rs:250-258`, the v3 note); serial removes it and collapses the median-min gap to a few % |
| P8 The same on the 22-core dev box without pinning | held | `perf-parallel.log`, 5 full runs, all green: `compute_summaries_v2` min 54.7-64.2 / median 56.8-74.5 ms vs 43.6 / 44.8 serial (P5) |

## Design

*(Revised after spec-panel round 1: the round-0 design blamed the median alone; P7 shows the main cause.)*

**Root cause.** CI runs this binary's 12 tests IN PARALLEL on a 4-vCPU runner (P7). The heavy tests (`build_full_*`,
`rung2`, `recursive_scc`, the L4 solves) each load every core for seconds, so one test's timed samples run while a
sibling is building or solving. That load is SUSTAINED: it covers all of a gate's samples, so it raises the minimum
as well as the median (P7: on 4 cores `compute_summaries_v2`'s per-run min goes from 44-47 ms serial to 54-71 ms
parallel, its median to 61-93 ms). The file's own v3 note (`tests/perf_bounds.rs:250-258`) says `min_of` cannot undo
such a window. On top of that, ten of the eleven magnitude gates judge the MEDIAN (P4), which also rises with any
brief one-sided interruption (P2). Both CI failures (P1) are this: medians 231.8 / 243.0 ms over a 230 ms bound, with
minima of ~150 ms.

**Change** (all in `tests/perf_bounds.rs` plus prose):
1. **No two timing tests overlap.** One file-wide `static TIMING_LOCK: Mutex<()>` and a `fn serial()` that locks it,
   recovering from poisoning (`unwrap_or_else(PoisonError::into_inner)`, so one red test cannot turn every later test
   red). Every timing test in `release_checks` (all eleven `*_within_bound` tests, `compute_all` included) takes
   `let _serial = serial();` as its FIRST line, so the lock covers its setup as well as its timed loop: a sibling's
   untimed setup (parsing, building) is exactly the load that skews another's samples. The CI step and `ci-steps` stay
   unchanged (`scripts/` is outside the flow's reach, and the fix belongs in the file that measures). Cost: the binary
   runs its timing tests one at a time, a few seconds (P5: serial is ~5 s locally).
2. **Every magnitude gate judges `min_of`.** At the ten `let m = median(samples.clone());` sites (P4) use
   `min_of(&samples)`; print and assert `min=` / "min … exceeds" instead of "median". `median` loses its last caller
   and is deleted. Doc references to it (`:233`, `:429`, `:437`, `:753`, `:878`) become plain code spans or are
   reworded, so no intra-doc link dangles.
3. **Docs say what is now true.**
   - The module doc (or the lock's doc) states the rule: timing tests hold `TIMING_LOCK` and judge `min_of`, and why
     (P7, the two CI failures, dated 2026-10-21, #190).
   - `min_of`'s doc: drop the claim that hyperfine and Criterion use the minimum. Say instead that the minimum rejects
     transient positive delays, and that it does nothing for a sustained window, which is what the lock is for.
   - One line at the bounds: they were calibrated as 3x a MEDIAN and are now compared with a minimum. That widens each
     bound by its gate's median-over-min gap, which depends on the gate: the worst per-run gap in the 4-core serial runs
     (`perf-4core-ser.log`) was 3-5% for `build_full_100`, `compute_summaries_v2`, `recursive_scc`, `rung1_scoped`;
     9-12% for `incoming`, `outgoing`, `prepare`, `rung2`; 16% for `build_full_1000` and 18% for `rung1`. All are small
     against the bounds' 3x margin. No bound value changes.
   - `COMPUTE_SUMMARIES_V2_BOUND` gains a dated note with the two runner failures and the numbers.
   - `docs/OUTSTANDING.md:645` (the `compute_all` CI flake item) is ticked with a pointer to #190.
   - CHANGELOG under `Fixed`.

**What the gates still catch, stated honestly.** Under the lock and `min_of`, a gate is a best-case magnitude tripwire
for a CONSISTENT slowdown (one that slows every call). A regression that slows only some calls can pass if one sample
is fast; that was never this file's job (it has no latency-distribution gates). Sustained load from OTHER tenants of
the runner, covering all of one gate's samples, is still not filtered; the lock removes only the load this binary
causes itself. Where a gate trips depends on the machine. On the dev machine `compute_summaries_v2` trips at ~5x its
min (230 / 44 ms), so a 3x regression passes there (proof D2, recorded as a limit). On the runner the PRE-lock min was
~150 ms (1.5x headroom); the lock should lower it (locally, 4-core: 54-71 ms → 44-47 ms), so the POST-lock runner
min, and with it the runner's trip factor, is UNMEASURED until D3. The claim "fails on a real 3x regression" on the
runner is therefore conditional on D3, decided in advance: if D3's exact 3x injection FAILS its bound on the runner,
the acceptance is met there; if it PASSES, the acceptance is NOT met and the issue goes back to design (re-baseline
that bound to 3x the post-lock runner min, or a ratio gate) instead of shipping with a reworded claim. The trip factor
stated in the code docs is written from D3's measured numbers, not from this estimate.

**Proofs** (each break made with Edit and grep-checked to have applied, then reverted with Edit):
- **D1a, every timing test takes the lock (deterministic).** A new test in this file, OUTSIDE `release_checks` (so it
  also runs in debug builds), reads the file's own source (`include_str!`) and asserts that EVERY `#[test]` fn inside
  `release_checks` (selected by the attribute, not by name, so a timing test named differently is still checked) has
  `let _serial = serial();` as its first statement, and that it found at least one (so the scan cannot pass empty).
  Matches are anchored to lines that start (after indentation) with `#[test]` / `fn `, so the scan's own string
  literals cannot match. It guards the future too: a new timing test without the lock goes red. Discrimination: delete the lock line from ONE test (Edit, grep-checked) →
  the scan fails naming that test; revert → green.
- **D1b, the lock removes sibling contention (measured, supporting).** After the change, run the full unfiltered
  binary in its CI shape (default threads) pinned to 4 cores (`taskset -c 0-3`), 5 runs. Judged only on the gates whose
  P7 per-run MINIMA separate by more than 10%: `build_full_1000` (serial max 121.3 ms, parallel 196-255),
  `compute_summaries_v2` (47.2 vs 54-71), `incoming` (2.61 vs 3.8-7.3), `compute_all` (7.07 vs 9.2-13.1). Each run's
  min must be at most the P7 serial maximum + 10%. The rest (`build_full_100` and `recursive_scc` separate on the
  median but not on the min; `prepare`/`outgoing` overlap) need only stay green. Discrimination:
  delete every `let _serial = serial();` line (Edit) and run the same 5 runs; the break counts as discriminating when,
  for EACH of the four gates, the LARGEST per-run min over the 5 runs exceeds that gate's limit (as every P7 parallel
  run did); revert. All runs and summaries recorded in the run dir.
- **D2, 3x locally.** Make `compute_summaries_v2_within_bound` solve 3 times per timed sample: it passes locally
  (~135 ms against 230 ms). Recorded as the dev-machine limit stated above, not as a pass.
- **D3, 3x on the runner.** One throwaway probe commit on the draft PR: `compute_summaries_v2_within_bound` solves 3x
  per sample and must fail by its own bound assertion; every other timing test panics AFTER printing its samples, so the
  runner's numbers for all gates under the lock become visible (CI shows only failing tests' output). Then a revert
  commit; the squash merge carries neither. Recorded: the 3x gate's outcome (see the decision rule above) and every
  gate's runner samples and min.
- **D4, repeat green on an unchanged head.** The final head's CI run plus 4 reruns of it (`gh run rerun`): 5/5 green.
  A rerun keeps the run id and bumps the attempt, so each execution is recorded as `(run_id, run_attempt, head_sha,
  conclusion)` with that attempt's own log. Locally, the full unfiltered binary 10/10 green in the CI shape (pinned to
  4 cores).
- Dropped from round 0: the sleep-3-of-5 injection (true by arithmetic, it proves nothing).

**Amendment after D3 (spec round 5; operator decision 2026-10-21: "Tighten that bound").** D3 fired the pre-decided
"back to design" branch: under the lock the runner's 3x `compute_summaries_v2` min was 179.87 ms, under 230 ms
(timeline, D3 result). The lock cut the runner's cost ~2.5x (implied 1x min ~60 ms, was ~150), so 230 ms now sits
~3.8x above the true cost.
- **Change 4:** `COMPUTE_SUMMARIES_V2_BOUND` 230 → **150 ms**. Margins: runner 1x ~60 ms → 2.5x headroom, runner 3x
  ~180 ms → fails by ~20%; dev machine 1x 47-49 ms → ~3.1x headroom (dev 3x 141-153 ms is at the bound, so on the dev
  machine 3x may pass: still stated as the dev limit). Under the lock the runner's samples spread only ~3.5%
  (179.9-186.3), so 2.5x is a wide noise margin. The bound's doc records the calibration (D3 numbers, the run id, the
  dates) and that it is calibrated to the CI runner, not the dev machine. **Scope of the 3x claim (S5-1, S5-3):** a
  3x regression fails on a runner of D3/D3b's speed (the CPU model D3b records); 150 ms rejects 3x only while the true
  1x cost is above 50 ms, so a runner ~1.2x faster than D3b's, or the dev machine, may let an exact 3x pass. The bound's
  doc and the CHANGELOG say exactly this, naming the CPU. No other bound changes, because the issue's check is this
  gate; the others are NOT uniformly "3x of product targets" (S5-2). Runner headroom under the lock (D3):
  `build_full_*`, `prepare`, `outgoing`, `incoming` are 3x CLAUDE.md product targets with 25x+ headroom (`incoming`
  75/2.81 = 27x); `compute_all`'s architectural bound 150/7.42 = 20x; the rung gates bind on SYNTHETIC bounds, 5x a
  synthetic-corpus baseline (`perf_bounds.rs:146`, `:154`): `rung1` 100/7.03 = 14x, `rung1_scoped` 65/1.47 = 44x,
  `rung2` 750/77.7 = 9.6x; `recursive_scc` is 3x a CONTENDED full-suite median
  (`perf_bounds.rs:384-393`), 650/133 = 4.9x, kept because its stated job is the ~22x re-materialization regression
  (133 x 22 ≈ 2.9 s ≫ 650 ms). None of these catches 3x today and none did before; re-baselining `recursive_scc` with
  the lock is filed as a discovery, not done here.
- **Change 5 (S1-6):** `min_of`'s doc drops "maximum-likelihood" (no noise model is stated); it says the minimum
  rejects transient positive delays.
- **D3b (confirm on the runner):** one more throwaway probe commit on the PR. `compute_summaries_v2_within_bound`
  prints the runner's CPU model (`/proc/cpuinfo` "model name") and times 5 samples of ONE solve, then 5 samples of
  THREE solves against the new bound; in both, every result is KEPT until `start.elapsed()` is taken and dropped after
  it, so the 3x sample is exactly three solves with the same drop placement as the 1x sample (S1-2). The measured
  multiplier (3x min / 1x min) is recorded. Required: the 3x assertion FAILS with the bound message, and the 1x min
  is <= 75 ms (2x below 150); else back to design again. Then a revert commit.
- D1-D4 otherwise unchanged; D4's 5/5 green runs on the final head (with the 150 ms bound) are the noise proof for it
  (superseded by Amendment 2: the bound stays 230 ms).

**Amendment 2 after D3b (operator decision 2026-10-21: "Keep 230 ms, ship flake fix").** D3b fired the "back to
design" branch: the runner it landed on (AMD EPYC 7763) gave 1x 86.52 ms, above the 75 ms rule; D3's runner did 3x in
179.87 ms against D3b's 248.21 ms, so the two runner runs differed ~1.4x. A bound that catches 3x on the fast runner
(needs < ~180 ms) and keeps a 2x noise margin on the slow one (needs > ~174 ms) exists only in a ~7 ms window (173-180 ms
fits both measured runs with ~1-3% margin): the two runs do not establish a robust portable bound.
- **Change 4 is withdrawn: `COMPUTE_SUMMARIES_V2_BOUND` stays 230 ms.** Headroom over the slow runner's 1x: 230 / 86.5
  = 2.7x; over the fast runner's (implied ~60 ms): ~3.8x. Change 5 (`min_of` doc) stays.
- **The 3x acceptance is PARTLY delivered, stated as such:** at 230 ms a 3x regression fails on a runner of the EPYC
  7763's speed (D3b: 3x min 248.2 ms) and passes on the faster D3 runner (179.9 ms) and on the dev machine (D2
  142-153 ms). The bound's doc and the CHANGELOG say exactly this with both runs' ids and numbers; the ledger's
  `## Not delivered` names it; a follow-up issue (a discovery, kind enhancement) asks for a machine-independent ratio
  gate (time `compute_summaries_v2` against a reference workload in the same process, as `compute_all`'s scaling
  ratio does).
- **Implementation:** a commit on top of the reverted D3b probe sets the bound back to 230 ms, rewrites the bound's doc
  (D3 and D3b numbers, CPUs, the partial-3x statement, the follow-up), fixes the Task-2 review's minors (the stale
  "bound left at 230ms" sentence becomes the history it is; the unreflowed `min_of` doc line), and rewords the
  CHANGELOG entry (no 150 ms change; the lock + `min_of` + what the runner data showed).
- **D4** runs on the final head with the 230 ms bound. **Operator decision (2026-10-21): "Cut D4 to two runs"** — the
  final head's PR CI run plus one rerun, 2/2 green, each recorded as `(run_id, run_attempt, head_sha, conclusion)`
  (was 5/5; the runner samples under the lock spread ~4% and the slow runner keeps 2.7x headroom).

**Non-goals.** Changing any other bound value or any sample count; the `compute_all` scaling ratio (#179); filtering
load from other runner tenants; making absolute bounds machine-independent (the file's history explains why it cannot
be done).

**Expected moves:** `tests/perf_bounds.rs`, `docs/OUTSTANDING.md`, `CHANGELOG.md`; no production code, no golden, no
CDO number.

## Base/Head SHAs

- Base `B` = `617b402f853664cf1cec8d049d85561d66ac8aa2` (origin/master; unmoved at step 11, no rebase needed).
- Gated head `H` = `280e6a9204d5bed925fde8dbb8b04ade7b95c8d7`.

## Timeline

- make-worktree: `{"worktree": "/work/al-sem-issue-190-a1", "branch": "issue/190-agent-discovery-tests-perf-bound-a1", "base": "617b402f853664cf1cec8d049d85561d66ac8aa2", "target": null}`
- classify: bounded
- probes P1-P6 (conductor; P5 a local release `perf_bounds` run, 12 passed): ten magnitude gates judge the median;
  the runner is ~3.4-5.3x slower than local for the failing gate. `## Design` written (the spec for a bounded issue).
- spec panel round 1 (spec_rounds 1/5, pi_calls 1, subagents 1; hash `60cc1313aea0b0f0`): sol E1 confirm-narrow / E2
  reject / E3 confirm / E4 reject; opus E1 reject-partly / E2 reject / E3 confirm-with-caveats / E4 reject. Both found
  the probes used the wrong run shape (serial vs CI's parallel). Conductor probes P7/P8 (4-core pinned CI shape, 5 runs
  parallel vs serial; dev box parallel): sibling contention is sustained and raises the minimum. Design revised (lock +
  `min_of`, exact-3x proofs on dev and runner, 5/5 CI reruns); register S1-1..S1-11 (9 fixed, 2 refuted); hash
  `019d3f91d8471df4`.
- spec panel round 2 (spec_rounds 2/5, pi_calls 2, subagents 2): opus accepted all 11; sol accepted 9, re-raised S1-2
  (runner 3x rests on the pre-lock min) and S1-8 (widening is gate-dependent). New: S2-1 (D1 criterion; sol N1 + opus
  N2) and S2-2 (rerun attempt ids). Fixed: a pre-decided D3 rule, per-gate widening numbers, D1a source-scan guard and
  D1b on the four min-separating gates, D4 records attempts. Hash `390ee02b003a8b86`.
- spec panel round 3 (spec_rounds 3/5, pi_calls 3, subagents 3): sol accepted all 13, no new; opus accepted all 13,
  new S3-1 (D1b break rule) and S3-2 (D1a selects by attribute, anchored). Both fixed. Hash `3ff6fdd3a57c4a0e`.
- spec panel round 4 (spec_rounds 4/5, pi_calls 4, subagents 4): both accepted all 15 against `3ff6fdd3a57c4a0e`, no
  new findings. **Converged**: no open entry; 13 fixed, 2 refuted; every entry accepted by both. Opus's non-finding
  note for implementation: D1a must fail LOUDLY if any other attribute sits between `#[test]` and `fn`, never skip.
- plan `docs/superpowers/plans/2026-10-21-issue-190-perf-bounds-serial-min.md`, 1 task (plan_tasks 1). Implement T1
  (opus, task_attempts T1 = 1): commit `4e5cce44`, task-affected exit 0, check-diff `[]`. **Deviation, recorded not
  tuned:** D1b's lock-run criterion (min <= P7 serial max + 10%) is NOT met on `build_full_1000` and `compute_all`;
  a same-session serial control lands on the same numbers, so the lock reaches serial equivalence and the miss is the
  machine's drift since P7 (conductor: load average 24 at 15:18, governor `powersave`, another session building). The
  no-lock break discriminates on all four gates. The final panel judges this deviation.
- sonnet review of `4e5cce44` (subagents): no critical/important; minor fixes sent back to the implementer → commit
  `1ea3dad4` (scan end must be the file's last item; any non-exact test attribute fails loudly; CHANGELOG/OUTSTANDING
  worded to what is shown; doc numbers labelled by session). Not taken: "thin separation" on `incoming` (7.77 ms
  no-lock vs 2.868 limit is 2.7x).
- task gate `ci-steps-task`: exit 0, supervised, 26.2 s (`logs/ci-steps-task/20261021T152335-ac78a620.log`).
- push-branch head `1ea3dad4`; draft PR #191.
- D3 probe commit `e83cdb12` (THROWAWAY): `serial()` returns a guard plus a `D3Probe` whose `Drop` panics unless
  already panicking (all 11 timing tests fail at their end and print samples); `compute_summaries_v2` solves 3x per
  timed sample (grep `D3-PROBE` = 4). Local check (`d3-local.log`, 4-core): 11 failed by the probe, 3x min 141.25 ms
  < 230 (dev-machine limit, as D2). Pushed; CI running.
- **D3 result** (run 39625785855, head `e83cdb12`, log `d3-ci.log`): all 11 timing tests failed by the probe only.
  `compute_summaries_v2` 3x: min **179.87 ms** < 230 ms (samples 179.9-186.3, ~3.5% spread) → the 3x gate PASSES on
  the runner. By the pre-decided rule the acceptance "still fails on a real 3x regression" is **NOT met**; back to
  design. Implied post-lock runner 1x min ~60 ms (was ~150 pre-lock): trip factor ~3.8x. Runner mins under the lock,
  every gate: build_full_1000 154.4, build_full_100 11.97, compute_all 7.42 (ratio 4.654), incoming 2.81, outgoing
  8.8 µs, prepare 2.18 µs, recursive_scc 133.1, rung1 7.03, rung1_scoped 1.47, rung2 77.7 ms.
- operator decision: "Tighten that bound" → Design amendment (change 4: 150 ms; D3b), hash `e373caf33b6ba209`.
- spec panel round 5 (spec_rounds 5/5 — cap reached; pi_calls 5, subagents 5): opus accepted all 16; sol accepted 13,
  re-raised S1-2 (probe drops inside the timed region), S1-6 (min_of doc still says maximum-likelihood), S5-1 (runner
  hardware). New minor S5-2 (other gates' headroom misstated) and S5-3 (runner CPU variance) — unreviewed. **Not
  converged; not handoff-ready** (S5-2/S5-3 are open and unreviewed). No sanctioned way past the cap
  (`scripts/agentflow/budget.py`, protected). Asked the operator.
- **Operator override (2026-10-21): "Allow one extra round"** — one verification-only spec round 6 beyond the
  `spec_rounds` cap of 5 (not chargeable: the executor's cap is exhausted; recorded here instead). Fixes: D3b keeps
  results until timing stops and records the multiplier and CPU model (S1-2); change 5 drops "maximum-likelihood"
  (S1-6); the 3x claim is scoped to a runner of the measured speed (S5-1, S5-3); other gates' calibration stated per
  group (S5-2). Hash `42f86dcf047458fe`.
- round 6 (pi_calls 6, subagents 6): sol accepted 17, re-raised S5-2 (minor: `rung1*` are 5x synthetic baselines,
  14x/44x); opus accepted 18 with the same slip plus `compute_all` unlisted. Sentence fixed; hash `8948f351efa94cd3`;
  both asked to re-confirm all 18 against it (same authorised round, the only change is that sentence).
- round 6 follow-up (pi_calls 7, subagents 7): both accepted all 18 against `8948f351efa94cd3`. **Converged** (under
  the operator's one-round override): no open entry; 16 fixed, 2 refuted. Opus's non-blocking note: cite the synthetic
  bounds by constant name (`RUNG2_SYNTHETIC_BOUND` :147, `RUNG1_SCOPED_SYNTHETIC_BOUND` :155), not `:154`.
- probe reverted (`6f9ab3ef`, tree = `1ea3dad4`); plan Task 2 (plan_tasks 2, task_attempts T2 = 1): implementer commits
  `b2a33e54` (bound 150 ms + docs) and THROWAWAY D3b probe `f96b087a` (pushed; CI running). Sonnet review of
  `b2a33e54`: no critical/important; minor: stale "bound left at 230ms" sentence (:415-416), dev-machine evidence
  mixed (D2 passed 142-153, D3b local failed 156.4), one unreflowed doc line — folded into the CPU-fill commit.
- **D3b result** (run 39635284889, head `f96b087a`, log `d3b-ci.log`): CPU `AMD EPYC 7763 64-Core Processor`; 1x min
  **86.52 ms** (86.5-90.2), 3x min 248.21 ms (248.2-264.8), multiplier 2.869; the 3x assertion FAILED with the bound
  message. The pre-decided rule needs 1x <= 75 ms: **NOT met → back to design**. D3 (unknown CPU) had 3x 179.87 ms; this
  runner is ~1.38x slower. Catching 3x on the fast runner (3x ~180) and keeping a 2x noise margin on the slow one
  (1x ~87 → needs > 173) leaves only a ~7 ms window, not a robust portable bound.
- **Operator decision (2026-10-21): "Keep 230 ms, ship flake fix"** → Amendment 2 (change 4 withdrawn; 3x partly
  delivered; ratio-gate follow-up), hash `ce4c719e3a118e4f`; register S5-4 added; every entry re-marked by the final
  panel. D3b probe reverted: `7627e2f0` (tree = `b2a33e54`).
- plan Task 3 (plan_tasks 3, task_attempts T3 = 1): implementer commit `5086d48c` (bound 230 ms, runner calibration in
  the doc and CHANGELOG, stale sentence marked history). Plan's grep expectation was wrong (`COMPUTE_ALL_BOUND` is
  150 ms, unrelated). Sonnet review: no critical/important; minor: OUTSTANDING says "confirmed by #190's CI runs"
  (true only once D4 is green; prose, revisit if D4 fails), stale ledger "150 ms" D4 line (fixed). Task gate exit 0,
  supervised, 24.3 s (`logs/ci-steps-task/20261021T165002-81889d5d.log`). Pushed `5086d48c`. Design hash
  `e89013fb6af5fc6a`.
- final panel round 1 (final_rounds 1/5, pi_calls 8, subagents 9): code-review skill (high, forked) 9 findings; sol 7
  findings, re-raised S1-3, S1-9, S2-2, S3-2, S5-1, S5-2, S5-3, S5-4; opus 1 important (D4 not yet recorded) + 5 minor,
  accepted all 19. Fix batch sent to the implementer (scan `{`-contains + `cfg_attr` rejection, nextest test-group,
  calibration/recursive_scc/1.4x/"cannot" wording, CHANGELOG, OUTSTANDING reflow, plan cleanup). Code-review #2
  (ratio-gate follow-up untracked), #5 (`--test-threads=1` in `ci-steps`), #8 (serial setup cost) to be refuted.
- **Operator decision: "Cut D4 to two runs"** (Design D4 updated); hash `01fc570aad8125b9`.
- final panel round 2 (final_rounds 2/5, pi_calls 9, subagents 10): opus accepted all 29, new minor N1 (window
  arithmetic) and N2 (S1-3 evidence) → F2-1, F2-2, fixed in `280e6a92` (doc/CHANGELOG/ledger wording) with check-diff
  `[]`; sol accepted 24, re-raised S5-4 (ledger impossibility prose — fixed) and S1-3, S2-2, F1-1, F1-7, which can only
  close with step 11's gates and D4's runs. Order decision: run step 11 (rebase + full gates) and D4 now, then final
  round 3 on the attested head with every record in place (no code change is pending; any later fix that touches
  more than prose goes back through step 11). Hash `c370b3c177626d54`.
- step 11 on `280e6a92` (no rebase: origin/master unmoved at `617b402f`): ci-steps-all, check-goldens-coverage,
  check-goldens, cdo-gate all exit 0, supervised (see Gate results). D4: run 39641883017 attempts 1 and 2 green.
- final panel round 3 (final_rounds 3/5, pi_calls 10, subagents 11): both accepted all 31 against
  `c370b3c177626d54`, no new findings. **Converged.**
- step 12: halt-check null; check-diff `617b402f..280e6a92` `{"reasons": []}`. The acceptance row "still fails on a
  real 3x regression" is PARTLY delivered; step 13 would make that `blocked acceptance-unmet`, but the operator chose
  "Keep 230 ms, ship flake fix" with the 3x catch "recorded honestly as partly delivered" and the remainder filed as a
  follow-up (the ratio-gate discovery), so the PR closes #190 under that operator decision, stated in its body.

## Acceptance matrix

| Acceptance item | Proof | Status |
|---|---|---|
| A perf bound that does not fail on runner noise alone | lock (D1a scan test + proofs; D1b lock/no-lock/serial-control) + `min_of` on every magnitude gate; runner samples under the lock spread ~4% (D3, D3b) | delivered |
| Stays green across repeated CI runs of an unchanged head | D4: run 39641883017 attempts 1 and 2 on `280e6a92`, 2/2 green (operator cut from 5); local 10/10 in the CI shape | delivered |
| Still fails on a real 3x regression | D3 (3x 179.87 ms passes 230), D3b (EPYC 7763: 3x 248.21 ms fails 230), D2 (dev 3x passes) | **partly delivered** (see Not delivered); ratio-gate follow-up filed |

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| `every_timing_test_holds_the_timing_lock` (TDD red) | test written before the lock existed | `scan-red.log`: panicked, missing list names all 11 `*_within_bound` fns | after Step 3: `scan-green-debug.log` / `scan-green-release.log` 1 passed | 4e5cce44 |
| D1a `every_timing_test_holds_the_timing_lock` | delete `let _serial = serial();` from `compute_summaries_v2_within_bound` (Edit; `grep -c '^        let _serial = serial();$'` = 10) | `d1a-break.log`: "timing tests without `let _serial = serial();` as their first statement: [\"fn compute_summaries_v2_within_bound() {\"]" | revert (grep 11), `d1a-revert.log` 1 passed | 4e5cce44 |
| D1a (attribute) | `#[ignore]` between `#[test]` and `fn prepare_within_bound` (grep `^    #\[ignore\]$` = 1) | `d1a-ignore-break.log`: "line 738: attribute between #[test] and fn (`#[ignore]`); the scan cannot check it" | revert (grep 0), `d1a-ignore-revert.log` 1 passed | 4e5cce44 |
| D1b lock removes sibling contention | delete all 11 `let _serial = serial();` lines (Edit replace_all; grep 0), 5 CI-shape runs | `perf-d1b-nolock.log`: largest per-run min `build_full_1000` 336.4 > 133.44, `compute_summaries_v2` 79.9 > 51.96, `incoming` 7.77 > 2.868, `compute_all` 12.89 > 7.776: all four discriminate; only the scan test red (5/5) | revert (grep 11, diff stat identical to pre-break 144+/47-); `perf-d1b-lock.log` 5/5 green, but see `## Implementation results`: 2 of 4 lock-run limits exceeded, equally by a same-session serial control | 4e5cce44 |
| scan end guard (review item 1) | `const SCAN_END_BREAK: () = ();` after `release_checks`'s closing `}` (grep `^const SCAN_END_BREAK` = 1) | `r2-end-break.log`: "line 1392: text after the column-0 `}` taken as the end of `release_checks` (line 1390); it must be the file's last item, or the scan stops early" | revert (grep 0), `r2-end-revert.log` 1 passed | 1ea3dad4 |
| scan attribute guard (review item 2) | first try, joining `#[test] fn outgoing_within_bound() {` on one line, did NOT apply: a formatter hook re-split it (grep 0, green run discarded). Used `#[tokio::test]` in place of `#[test]` on `outgoing_within_bound` (grep `^    #\[tokio::test\]$` = 1; `release_checks` is compiled out in debug, so it builds) | `r2-attr-break.log`: "line 827: test attribute not alone on its line as `#[test]` (`#[tokio::test]`); the scan cannot check it" | revert (anchored grep 0; the one remaining `tokio::test` is the scan's own comment outside the module), `r2-attr-revert.log` 1 passed | 1ea3dad4 |
| scan body-start (final panel 1a) | the specified break (`fn outgoing_within_bound() { // note`, lock moved into the `for` block) could not be staged: rustfmt (and the formatter hook) moves a comment after `{` onto the next line (grep 0). That red run came from the `// note` line, not the brace rule; discarded and reverted (grep 11). rustfmt-stable shape used: a one-line `#[test] fn scan_brace_break() {}` added before `outgoing_within_bound` (grep 1) | new rule: `r3-brace-break.log` red, missing list names "fn scan_brace_break() {}". Old rule (`ends_with('{')`) restored with the break in place: `r3-brace-oldrule.log` GREEN, i.e. the old scan silently took the next test's lock | rule restored (grep `contains('{')` 1, old rule 0), break removed (grep 0), `r3-brace-revert.log` 1 passed | 280ee98d |
| scan cfg_attr (final panel 1b) | `#[cfg_attr(not(debug_assertions), test)]` in place of `#[test]` on `outgoing_within_bound` (grep 1) | `r3-cfgattr-break.log`: "line 842: test attribute not alone on its line as `#[test]` (`#[cfg_attr(not(debug_assertions), test)]`); the scan cannot check it" | revert (grep 0), `r3-cfgattr-revert.log` 1 passed | 280ee98d |
| D2 3x locally | `compute_summaries_v2_within_bound` solves 3x per timed sample (grep `D2-BREAK` = 2), 3 CI-shape runs | (expected pass) min 141.8 / 153.1 / 144.9 ms vs bound 230 ms: PASS, dev-machine limit as the design states | revert (grep 0) | 4e5cce44 |

## Findings register summary

29 entries (spec panel S1-S5, proofs D3/D3b as S5-1/S5-4, final round 1 F1-1..F1-10): 24 fixed, 5 refuted
(S1-10, S1-11, F1-8, F1-9, F1-10). No open entry; no blocking entry deferred. Marks per final round below.

## Gate results

On `H` = `280e6a92`, all `"supervised": true`, queued 0 s; logs under the run dir `logs/`:

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 265.7 | `ci-steps-all/20261021T171316-3e8e7903.log` |
| check-goldens-coverage | 0 | 0.0 | `check-goldens-coverage/20261021T171741-e200963c.log` ("coverage OK (36 golden dirs declared)") |
| check-goldens | 0 | 0.1 | `check-goldens/20261021T171742-83ae0081.log` (tree stamp: this exact tree passed `ci-steps test` inside `ci-steps all`, goldens already verified) |
| cdo-gate | 0 | 265.6 | `cdo-gate/20261021T171742-57365914.log` |

**D4 (operator: two runs), on the gated head:**

| run_id | run_attempt | head_sha | conclusion |
|---|---|---|---|
| 39641883017 | 1 | 280e6a9204d5bed925fde8dbb8b04ade7b95c8d7 | success |
| 39641883017 | 2 | 280e6a9204d5bed925fde8dbb8b04ade7b95c8d7 | success |

Earlier green PR runs since the lock: 39638416458 (`5086d48c`), 39641237764 (`280ee98d`). The final head adds only
the committed ledger and register (`.agent/issue-190/`) to `280e6a92`; its own CI run is the merge gate's.

`git status --porcelain -- . ':!.agent'`: empty. Earlier task gates: `ci-steps-task` exit 0 on `1ea3dad4` (26.2 s),
`5086d48c` (24.3 s), `280ee98d` (24.8 s).

## Discoveries

```json
[
  {"subsystem": "tests / perf_bounds (compute_summaries_v2 sensitivity)", "locator": "tests/perf_bounds.rs release_checks::compute_summaries_v2_within_bound (COMPUTE_SUMMARIES_V2_BOUND = 230 ms, an absolute bound)", "symptom": "Two GitHub-hosted runner runs differed ~1.4x in speed for this gate, leaving only a ~7 ms window (173-180 ms) for an absolute bound that catches a 3x regression on the fast runner and keeps a 2x noise margin on the slow one (no robust portable bound): at 230 ms a 3x regression passes on the faster runner and on the dev machine.", "kind": "enhancement", "origin_issue": 190, "reproducer": "#190 CI probes under the timing lock: run 39625785855 3x min 179.87 ms (< 230, passes); run 39635284889 (AMD EPYC 7763) 1x min 86.52 ms, 3x min 248.21 ms; dev machine (Intel Core Ultra 9 185H) 3x min 141.8-156.4 ms.", "pre_existing": true, "capability": "a machine-independent check: time compute_summaries_v2 against a reference workload in the same process (a ratio, as compute_all's scaling check does)", "acceptance": "a real 3x slowdown of compute_summaries_v2 fails on every runner and on the dev machine, while an unchanged head stays green across repeated CI runs"},
  {"subsystem": "tests / perf_bounds (recursive_scc calibration)", "locator": "tests/perf_bounds.rs RECURSIVE_SCC_BOUND (650 ms, 3x a CONTENDED full-suite median of ~215 ms, :384-393)", "symptom": "The bound was calibrated on runs where sibling tests ran at the same time; since #190 the timing tests run one at a time, and the runner's min is 133 ms (4.9x headroom), so the bound reflects contention that no longer exists.", "kind": "enhancement", "origin_issue": 190, "reproducer": "#190 CI probe run 39625785855: recursive_scc_db_effects min 133.09 ms vs bound 650 ms; dev machine serial 106-121 ms.", "pre_existing": true, "capability": "RECURSIVE_SCC_BOUND recalibrated against serial (locked) runs, keeping its stated job of catching the ~22x re-materialization regression", "acceptance": "the bound's doc cites serial-run numbers and the ~22x regression still fails by a wide margin"}
]
```

## Implementation results

Task 1, commit `4e5cce44` (tests/perf_bounds.rs, docs/OUTSTANDING.md, CHANGELOG.md, the plan). Logs in
`/work/alsem/.agent/runs/20261021-143757-e143e4/`. All runs `taskset -c 0-3 cargo test --release --test perf_bounds
-- --nocapture` unless noted; summaries by `agg.py` (per-run min range over the runs).

- Step 6: `perf-green.log` 13 passed (12 + scan). Clippy dev and release on `--test perf_bounds`, `-D warnings`: clean.
- **D1b, with the lock (`perf-d1b-lock.log`, 5/5 green).** Per-run min ranges: `build_full_1000` 129.9-140.3 ms
  (limit 133.44: **exceeded** in some runs), `compute_summaries_v2` 47.5-49.0 (51.96: met), `incoming` 2.41-2.73
  (2.868: met), `compute_all` 7.14-8.14 (7.776: **exceeded**). The criterion as written is NOT met for 2 of 4 gates.
  Cause measured, not tuned away: a same-session serial control (`--test-threads=1`, same pinning, 5 runs,
  `perf-d1b-serial-control.log`) gives `build_full_1000` 130.2-141.7, `compute_all` 6.94-7.82, `incoming` 2.44-2.94,
  `compute_summaries_v2` 46.8-48.0: the same as the lock runs. The machine ran slower than at P7 (load avg ~5,
  governor `powersave`); the lock reaches serial-equivalence in this session; the P7-derived absolute limits did not
  hold even for true serial runs. Later, in the 10x run, `build_full_1000` min was 116.4-126.2 (inside the limit) and
  `compute_all` 6.96-8.60 (still above 7.776 in some runs).
- **D1b, no lock (`perf-d1b-nolock.log`).** Largest per-run min: `build_full_1000` 336.4, `compute_summaries_v2` 79.9,
  `incoming` 7.77, `compute_all` 12.89: each exceeds its limit, so the break discriminates on all four. Timing gates all
  still passed (5/5 runs failed only on the scan test).
- **D2** (`perf-d2-3x.log`, 3 runs): 3 solves per sample, min 141.8 / 153.1 / 144.9 ms vs 230 ms: passes. Dev-machine
  trip factor ~5x (230 / ~47 ms), as the design states.
- **Local repeat** (`perf-local-10x.log`): 10/10 green (13 passed each).
- `bash scripts/ci-steps task-affected` (`task-affected.log`): exit 0, 3590 passed, 8 skipped.
- `agentflow check-diff --base origin/master --head HEAD --issue 190`: `{"reasons": []}`.
- Runner trip factor left as "measured in #190's D3" in the `COMPUTE_SUMMARIES_V2_BOUND` note, for the conductor.
- Review follow-up, commit `1ea3dad4` (on top, no amend): scan end guard + attribute guard (proofs above); CHANGELOG
  reworded to what the change does; OUTSTANDING tick reads "addressed by #190 … confirmed on the runner by #190's CI
  runs"; dev trip factor "~4.7x (230 / ~48 ms)" with P5's 44 ms named; lock doc labels its numbers P7. Release
  `perf_bounds` 13 passed, clippy clean, `task-affected` exit 0 (3590 passed), check-diff `{"reasons": []}`.
- **Task 2**, commit `b2a33e54` (tests/perf_bounds.rs, CHANGELOG.md, the plan with Task 2): `COMPUTE_SUMMARIES_V2_BOUND`
  230 -> 150 ms, doc records the D3 calibration and the runner scope with the `<D3B_CPU>` placeholder (also in the
  CHANGELOG, for the conductor); `min_of`'s doc no longer says maximum-likelihood. Step 3: no doc cites other bounds'
  calibration by line number, nothing to change. `t2-5x.log`: 5/5 green (13 passed each); `compute_summaries_v2` min
  per run 49.39 / 50.16 / 47.87 / 52.81 / 52.28 ms vs 150. `task-affected` (`t2-task-affected.log`) exit 0, 3590
  passed; check-diff on `b2a33e54` `{"reasons": []}`.
- **D3b probe**, THROWAWAY commit `f96b087a` on top (not pushed). Local run (`d3b-local.log`, 4-core pinned, full
  binary): CPU `Intel(R) Core(TM) Ultra 9 185H`; 1x min 52.98 ms (samples 53.0-55.3); 3x min 156.42 ms (samples
  156.4-168.6); multiplier 2.952. On this dev box the 3x FAILED the 150 ms bound with the bound's own message
  ("compute_summaries_v2_bundle min 156.418615ms exceeds 3x-target bound 150ms …"), so the D3B-PROBE panic did not
  fire; 12 passed, 1 failed. Note: the dev machine's 1x was ~53 ms in this session (47-49 earlier), above the 50 ms
  break-even, which is why 3x failed here although the design expected it may pass. Clippy release clean.
- **Task 3**, commit `5086d48c` on `7627e2f0` (tests/perf_bounds.rs, CHANGELOG.md, the plan with Task 3):
  `COMPUTE_SUMMARIES_V2_BOUND` back to 230 ms; its doc rewritten (pre-lock CI failures; D3 3x 179.87 ms; D3b EPYC 7763
  1x 86.52 / 3x 248.21 ms; ~1.4x runner spread; 2.7x headroom; 3x fails only on the slow runner, passes on the fast one
  and the dev machine; ratio-gate follow-up); the old "bound left at 230ms" sentence marked as history; `min_of` doc
  reflowed; CHANGELOG reworded (no 150 ms change, runner data, partial 3x, follow-up). Net diff vs `b2a33e54~1` for
  `tests/perf_bounds.rs`: 22+/14-. Step 4 grep `D3B_CPU\|from_millis(150)` = 1 in perf_bounds.rs, 0 in CHANGELOG: the
  one hit is the pre-existing `COMPUTE_ALL_BOUND` (150 ms, line 285), so the plan's expected 0 cannot hold; no
  `<D3B_CPU>` remains and `COMPUTE_SUMMARIES_V2_BOUND` is `from_millis(230)`. Release `perf_bounds` (`t3-release.log`)
  13 passed (`compute_summaries_v2` min 44.13 ms); `task-affected` (`t3-task-affected.log`) exit 0, 3590 passed;
  check-diff `{"reasons": []}`.
- **Final-panel fixes**, commit `280ee98d` on `5086d48c` (tests/perf_bounds.rs, .config/nextest.toml, CHANGELOG.md,
  docs/OUTSTANDING.md, the plan). Scan 1a/1b (proofs in the table; 1a's specified shape is not rustfmt-stable, a
  one-line `fn x() {}` used instead, with an old-rule run showing the miss). nextest: `[test-groups] perf-bounds =
  { max-threads = 1 }` plus a default-profile override `binary(perf_bounds)`; `cargo nextest show-config test-groups
  --release` (`r3-nextest-groups.log`) lists all 13 perf_bounds tests under "group: perf-bounds (max threads = 1)",
  and `--profile ci` (`r3-nextest-groups-ci.log`) too (11 release_checks tests listed). A `cargo nextest run --release
  --test perf_bounds --no-capture` passed 13/13 (`r3-nextest-run.log`), but `--no-capture` serializes on its own, so
  that run is not evidence for the group. Docs: bound calibration summary per kind; RECURSIVE_SCC history + D3 133.09
  ms / ~4.9x; ~1.4x as "two runner runs"; "175 ms would satisfy both with ~1.7% margin"; CHANGELOG same + "a solve
  made 3x heavier"; OUTSTANDING reflowed, "repeat-green CI record is in #190's PR"; plan ticked, Task 2 superseded,
  session paths removed (the `TREE_SITTER_AL_PATH` and agentflow command paths kept: they are commands, not run
  artifacts). Clippy dev + release clean; release `perf_bounds` 13 passed; `task-affected` (`r3-task-affected.log`)
  exit 0, 3590 passed; check-diff `{"reasons": []}` (`.config/` accepted).

## Not delivered

- **"still fails on a real 3x regression" — PARTLY delivered.** At the 230 ms bound a 3x regression of
  `compute_summaries_v2` fails on a runner of the AMD EPYC 7763's speed (D3b: 3x min 248.21 ms) and passes on the faster
  D3 runner (179.87 ms) and on the dev machine (141.8-156.4 ms). The 150 ms alternative was withdrawn by the operator
  because the slow runner's 1x (86.52 ms) would leave only 1.7x noise headroom. Follow-up: the ratio-gate discovery.
