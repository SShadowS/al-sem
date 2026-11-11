# Where `scripts/cdo-gate` spends its time (TLS-16)

2026-11-11. An investigation: nothing was changed. Sources: the logs of today's three complete gates
in this container (`/work/alsem-wt/orchestrator-gate/target/cdo-gate-logs/<run>/`, one log per phase,
every line stamped with the gate's clock in seconds), the scripts, and the tests' code. Two
independent reads went into the proposals: mine, and Fable's
(`.agent/tls16/fable-proposals.md`, `fable-product-candidates.md`); each proposal says whose it is
and what was checked. Section "Traced" is filled in after the traced runs.

## The gates measured

Serial mode (`CDO_GATE_SERIAL=1`, required here by memory). Seconds per phase:

| Run | build | speed_build | speed | harness | lsp | l4 | lib_cdo | output_cdo | Total |
|-----|-------|-------------|-------|---------|-----|----|---------|------------|-------|
| 003611 (TLS-01..10) | 31 | 204 | 247 | 51 | 9 | 20 | 292 | 798 | 27 min |
| 012547 (GRM-01) | 213 | 290 | 257 | 52 | 9 | 20 | 289 | 804 | 28 min |
| 052939 (TLS-11..13, busy host) | 419 | 596 | 541 | 68 | 12 | 32 | 452 | 1,032 | 45 min |

`build` (the `cdo`-profile test targets) and `speed_build` (the two `release-fast` `alsem`s) run at
the same time, so the longer one counts. `speed` runs alone, then the test phases one after another.

## Findings

**1. `output_cdo` is two impact ratchets.** In all three gates the other 22 tests finish 314-328 s
into the phase and `cdo_test_selection_ratchets` 347-364 s in; `cdo_impact_ratchets` ends 657-687 s
in and `cdo_impact_base_ratchets` 798-1,032 s in. So 470-700 s of the phase waits on those two.
They run one-shot `alsem impact` processes (`tests/cli/cli_impact.rs:547`, three at once: a
699-root diff as JSON and as human text, and one routine; `tests/cli/cli_impact_base.rs:1136`,
two at once: `--base bc3ccb18~5` as JSON without tests and as human text with tests). Each builds
the CDO model (and the base's, and the test universe) from scratch. CLAUDE.md's "the length is
output_cdo's impact ratchets" is confirmed.

**2. The cost is model builds, not answers.** Through the speed phase's resident server an impact
answer takes 1.4 s per round (`speed.log`, `impact` row); one-shot, 4-7 min. The `cdo` profile is
fully optimized (`opt-level` from `release`, no LTO), so this is close to what a CLI user pays.

**3. The phase starts three resident CDO servers** (`tests/cli/output_cdo.rs:283`, `:611`,
`tests/cli/output_invariants_w3.rs:1689`; found by Fable, checked), besides the five one-shot
impact processes and one-shot `tests` and `prove` calls: about 12-14 CDO model builds in one phase.

**4. `lib_cdo` rebuilds the same models.** 17 tests, 4 threads, 289-452 s. Read test by test:
six `build_cross_app_base` calls with identical arguments across four tests (two of them build it
twice), three single-app BASE models, six identical CDO snapshots; four tests need their own build
(they compare profiles, consume the model, install read hooks, or need another view). The gate runs
`cargo test`, one process per binary, so a `OnceLock` CDO fixture IS shared between tests here
(unlike nextest); `program_resolve_harness` already does it (`CDO_SHARED`: 213 tests in 51-68 s).

**5. The speed phase** (247-541 s): the warm-up is about half (the two model builds one after the
other, 66 + 83 s, and the two test universes, 31 s each); the three rounds 70-135 s each, prove's
first call per routine the largest part. Its doc gives the noise bands behind three rounds and the
warm-up; we both judged its rounds not worth trimming (one round is about 36 s, and two samples
leave no median).

**6. `speed_build` is usually the longer build.** The working tree's `release-fast` `alsem` takes
204-290 s after any change (thin LTO relinks the crate); the base binary (master's), built once per
master commit into `~/.cache/al-sem/cdo-speed/`, cost 595 s in run 052939, when master had moved.
`build` itself recompiles only the `al-sem` crate (31-419 s; 213 s on GRM-01, whose grammar bump
recompiled `al-syntax` too).

**7. The busy host cost 18 minutes.** Run 052939 ran at a load average of 40-97 (other projects
on server1) and took 45 min against 27-28.

## Gate proposals, ranked

Each: the expected saving, what it weakens, whose. These change the gate's scripts and tests (or
policy); the gate keeps proving what it proves today.

| # | Proposal | Saving (est.) | Weakens | From |
|---|----------|---------------|---------|------|
| G1 | Run the two impact ratchets as their own background lane, also in serial mode | ~6 min | nothing; peak memory needs one measured run | Fable |
| G2 | Never build the speed base inside the gate: build master's `release-fast` binary into the cache right after each landing | 4-10 min on gates after a master move | nothing | both |
| G3 | Skip the speed phase when no product source changed (a path rule; unknown paths run it) | 7.5-19 min on test- or script-only branches | nothing for those branches (no product change to time) | Fable |
| G4 | One shared CDO model per process in `lib_cdo` (groups above) and one resident server for `output_cdo` | 1-3 min; less peak memory (unmeasured) | the one-shot `tests`/`prove` paths would no longer run on CDO: keep one one-shot call per command | both |
| G5 | Time the `release` `alsem` that `ci-steps all` already built, instead of building `release-fast` | 200-290 s per gate | the noise bands must be re-measured on `release`; only after G2 | both |
| G6 | Keep other builds off the host while a gate runs (policy) | up to 18 min (run 052939) | nothing | Fable |

## Product speedups (asked for by the owner)

The owner asked, in the lane's session on 2026-11-11, for product-wide refactors of this kind too
("parallelize `alsem impact` across roots"): changes that make `alsem` itself faster for every user
and shorten the gate as a side effect. They are product changes, each its own task with a spec,
review and goldens; all are inferences from code and totals until traced (section "Traced").

| # | Candidate | Expected | Risk | From |
|---|-----------|----------|------|------|
| P1 | Run what follows the witnesses in parallel over roots (the literal-reach pass, the per-root finish and rollup: `digest_cli.rs:2135-2166`, `:2358-2370`, `impact/jobs.rs:441-456`; the witnesses already run on rayon, `digest.rs:4866`) | up to most of the ~200 s a 699-root impact adds; `--base` pays it twice; with P2, G1 becomes unnecessary | output order must stay pinned; the shared `SuffixCache` must become a concurrent memo | Fable (round 2) |
| P2 | Share one program build across a one-shot's parts (main, up, select, probes, base each rebuild today: `src/bin/alsem.rs:2649-2717`, `impact/run.rs:153-166`) | one-shot floor ~65 s -> perhaps 35-45 s, for CLI and CI users | peak memory rises | Fable (round 2) |
| P3 | Refresh the test universe incrementally after a body edit (today a `Package`-class rebuild: `alsem_tests` after an edit 13-14 s, warm 0.2 s; CHANGELOG `:5471-5476`) | the agents' edit loop: 13 s -> seconds | highest: must equal a cold build | Fable (round 2) |
| P4 | Cut prove's first call per routine (13-16 s, one thread; `digest.rs:5212`; `literal_reach.rs` has no rayon use) | depends on which span dominates | depends | Fable (round 2) |
| P5 | Trace the cold start's serial stages (`program_calls.rs`, `demand.rs`, `program/build.rs`, `impact_graph/` have no parallel code; up to ~30 s of a 35-41 s first ask may be one thread) | to be measured | - | Fable (round 2) |
| P6 | Keep each root's small result per revision for large asks | repeat asks fall to the render | low | Fable (round 2) |
| P7 | Walk impact's up part over a bounded number of roots at once (`src/engine/impact/up.rs:1164`, serial today by design: one visitor alive at a time) | 53-62 s of a 699-root impact (traced) | memory: more visitors alive; order of the output must stay pinned | traces (this note) |

Not proposed: trimming the speed phase's rounds or running its two sides at once (load noise in the
judged ratio); a parser per worker thread (creating one is tiny beside parsing; the `engine_stages`
bench can confirm). Fable's round 1 said impact's per-root work is serial; its round 2 corrected it
(the witnesses run on rayon, `digest.rs:4866`; checked), which moved the fix to P1 and P2. A claim
that prove's first call regressed from 5.9 s was a misread (that CHANGELOG line measures four
`query touches` answers) and is not used.

Estimate if all gate items land (Fable's): 11-15 min against 27-28 for a product branch, 6-8 min for
a test-only branch; G1-G4 give most of it.

## Limits

- No memory was measured: every peak-memory statement is a guess. Parallel lanes measured 51-58 GB
  on the owner's machine (CLAUDE.md), above this container's 48 GiB; G4 may bring that under it.
- The product candidates' savings (P1-P6) are inferences from totals and code until traced.
- One shared fixture keeps its model for the whole process (a `OnceLock` never drops); two lib tests
  drop theirs on purpose today (`no_path_tests`, `wave2_index_tests`).

## Traced

`ALSEM_TRACE=1` one-shot runs on the pinned CDO baseline, the `cdo`-profile `alsem` of master
`4a3caaaa`, one at a time, the engine lane building beside them (no other CDO job), summarized with
`scripts/trace_summary.py` (self time per span; the main thread's spans tile the wall time):

| Span (self, s) | impact `--diff` (699 roots, 315 s) | impact `--base`, JSON (520 s) | prove (39 s) | explain (36 s) |
|---|---|---|---|---|
| `prove.reach_effects` | 108.8 | 196.8 | 5.5 | 5.6 |
| `job.0.impact-up` (the up part) | 53.4 | 62.4 | - | - |
| `digest.roots` (witnesses, rayon) | 42.7 | 81.4 | 4.0 | 4.0 |
| `digest.unresolved_cones` | 33.2 | 66.9 | - | - |
| `cone.walk` | 9.5 | 15.7 | 5.5 | 3.1 |
| model build (`crossapp.resolve_dependency_bodies`, `preflight.*`, `l3.dependency_rows`, ...) | about 40-50, in 3 builds | about 50-60, in 3 builds | about 15-20 | about 15-20 |

What this settles:

- **P1 is confirmed and is the largest item.** `prove.reach_effects` is `literal_reach_entries`
  (`src/engine/l5/digest_cli.rs:2358`): `for entry in entries`, one root after another, sharing one
  `&mut SuffixCache`. With `digest.unresolved_cones` it is 142 s of the diff call's 315 s and 264 s
  of the base call's 520 s, on one thread. Even one root pays 5.5 s of it (prove, explain).
- **A new candidate, P7: the up part is serial too.** `up::up` (`src/engine/impact/up.rs:1164`) walks
  the roots one at a time, by design ("one is alive at a time": a memory bound on the visitors):
  53-62 s on 699 roots. Walking a bounded number at once trades some of that bound for time.
- **P2 is confirmed but smaller than estimated.** Each one-shot builds the model three times (the
  `n = 3` of `crossapp.resolve_dependency_bodies` and `preflight.parse_snapshot`), about 40-60 s in
  all, so sharing one build saves about 25-40 s of a 315-520 s call. Fable's "65 s floor ->
  35-45 s" is roughly right for the one-routine call.
- **The gate's impact ratchets are mostly P1 and P7.** About 200 of the diff call's 315 s and
  330 of the base call's 520 s are the per-root work after the witnesses plus the up walk; the
  witnesses themselves (already parallel) are 43-81 s.
- **P4 (prove's first call):** in a one-shot, about half the 36-39 s is the model build and the
  rest the answer: `reach_effects` 5.5 s, the witnesses 4 s, `cone.walk` 3-5.5 s. P1's change
  (parallel over a root's crossings, or a shared tree-level reach) is also P4's.
- **P5 (cold start):** the build's largest single spans are `crossapp.resolve_dependency_bodies`
  (5-5.2 s per build), `l3.dependency_rows` (3.1 s), `preflight.parse_snapshot` (2.3-2.4 s) and
  `preflight.dep_layer` (1.2-1.3 s); parallelism inside them was not measured (a trace shows
  wall time per span, not cores).

Ranking after the traces: P1, then P7, then P2 (product); G1 (impact ratchets as their own lane)
shrinks to whatever P1 and P7 leave. The four traces and the script that ran them are in
`cdo-gate-time/` next to this note (`scripts/trace_summary.py cdo-gate-time/trace-*.json`
reprints the table).

## TLS-17: the cones' indexes built once (measured)

The unresolved cones' four snapshot indexes, rebuilt for every root, are built once per read
(`ConeIndexes` in a `OnceLock` on `FinishCx`; spec `docs/superpowers/specs/2026-11-11-parallel-literal-reach.md`).
Measured on CDO, release-fast, one job at a time, alternating: before = the speed cache's binary of
`3f180c94` (master's product code), after = TLS-17. Every run was flagged noisy (host 37-80% idle).
Records: `cdo-gate-time/tls17-runs.jsonl`.

| Call | `digest.unresolved_cones` (self) | Wall | CPU | Peak RSS |
|------|----------------------------------|------|-----|----------|
| `impact --diff` (699 roots) | 30.4 -> 3.6 s | 318.0 -> 338.6 s | 729 -> 780 s | 3,283 -> 3,273 MiB |
| `impact --base`, JSON | 41.8 -> 4.0 s | 408.1 -> 371.0 s | 906 -> 866 s | 3,663 -> 3,680 MiB |

- The span falls by about 88% in both calls: the index builds were nearly all of it.
- The walls and CPU move with the host's load, not with the change: the diff call's "after" run had
  a host 37% idle, and its untouched `prove.reach_effects` grew from 87.6 s to 118.3 s with it; the
  base call's runs had comparable hosts and fell 37 s. The span is the clean number.
- Peak RSS is unchanged within noise (-0.3%, +0.5%).
- Output: the `--diff` answers are identical; the `--base` answers differ only in the base cache's
  temporary folder and the base build's `ms` timings, which change on every run.
- What is left of the span (3.6-4.0 s) includes `count_gaps_in_cone`; not worth a task now.

## TLS-19: the impact ratchets as their own lane (measured)

G1 as built: `output_cdo::cdo_impact*` run as the phase `output_cdo_impact`, started in the
background before the other test phases, also in serial mode (the mode used here). The other phases
still run one after another. Two gates on 2026-11-11, seconds:

| Run | impact lane | harness | lsp | l4 | lib_cdo | output_cdo (rest) | chain | test phases (wall) |
|-----|-------------|---------|-----|----|---------|-------------------|-------|--------------------|
| tb4 (TLS-17..20) | 874 | 99 | 12 | 36 | 362 | 247 | 756 | 874 |
| joint (TLS-19 fix + ENG-18, busy host) | 1,232 | 104 | 19 | 55 | 503 | 358 | 1,039 | 1,232 |

- Before, the same phases one after another took 1,170-1,174 s on a quiet host (003611, 012547
  above) and 1,594 s on a busy one (052939). tb4 cut that to 874 s (-25%); the joint run, on a host
  loaded like 052939 (its lib_cdo 503 s against 452 s), to 1,232 s (-23%).
- The cost: the phases beside the impact lane slow down (harness 51 -> 99 s, l4 20 -> 36 s in tb4),
  because the two lanes share the CPU. The impact lane is now the longer one, so the gate's test
  time is the impact calls' time; P1 (parallel work over roots) is what shortens it further.
- tb4's whole gate took 34.7 min, but 944 s of it was `speed_build` compiling the merge base cold
  (TLS-18's warm step was not yet on master); the test phases were 874 s of it.
- Memory. tb4 sampled `memory.current`, which counts page cache the kernel reclaims, and read
  49,151 of 49,152 MiB with no OOM: no information. The joint run sampled the unreclaimable part
  (`memory.stat` anon + shmem + kernel - slab_reclaimable): peak 42,822 of 49,152 MiB, headroom
  6,329 MiB, against the owner's rule of at least 4 GiB. Its `memory.events` added max +24,119
  (the limit reached and page cache reclaimed), oom +0, oom_kill +0.
- First verdict (superseded below): keep the lane. Both runs were faster than the serial phases
  they replaced, and the one run with a real memory reading kept 6.2 GiB of headroom.

## TLS-23: back to serial (measured)

Two more gates on 2026-11-11, with the timeline sampler (`memory.timeline`, `phases.times`):

| Gate | impact lane | chain | unreclaimable peak | headroom | gate's own then | oom |
|------|-------------|-------|--------------------|----------|-----------------|-----|
| ENG-18 + TLS-19 re-gate (both tests at once) | - | - | 46,315 MiB | 2,836 MiB | (not sampled) | 0 |
| TLS-23 (`--test-threads=1`) | 1,641 s | 859 s | 41,064 MiB | 8,087 MiB | 30,838 MiB | 0 |

- The re-gate broke the owner's rule (at least 4 GiB headroom).
- The timeline of the TLS-23 gate (run `20261111-144019-3962977`) shows where the memory goes:
  - `lib_cdo`'s single test process grows to 25-29 GB.
  - `output_cdo` holds about 24 GB of its own (three `alsem` at ~6 GB plus the test process).
  - The base impact test (2 calls) holds ~5.7 GB, the diff test (3 calls) ~8.5 GB.
  - The peak was during `lib_cdo` with the base test beside it; other agents held ~10 GB of it.
- Only the base test fits beside `lib_cdo` or `output_cdo`. One test at a time made the lane the
  bottleneck: the base test took 1,047 s beside the chain (428 s alone), then the diff test ~570 s.
  1,641 s is slower than the 1,170 s the phases took one after another on a quiet host.
- A lane staged to fit (the base test beside the chain, the diff test after it) was estimated at
  1,260-1,430 s with the same headroom: no clear gain on a shared host, for more script logic.
- Verdict (orchestrator, 2026-11-11): serial. The impact tests stay their own phase,
  `output_cdo_impact`, run after every other phase with both tests at once (~14 GB, nothing else of
  the gate beside them). The sampler, timeline and phase times stay.
