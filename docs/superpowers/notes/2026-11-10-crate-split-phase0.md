# Crate split, Phase 0: baseline measurements

For the design `docs/superpowers/specs/2026-11-10-crate-split-design.md`, section 9 "Phase 0".
Task ENG-04 (kraken lane engine).

**Status.** Done. Part 1 (no build: counts, commits, tests, the cold-or-warm share) is sections
2–6; part 2 (the timings, 2026-11-10 17:54–20:45) is sections 7–8; the answer for stop/go 0 is
section 9. Raw records: `/work/eng04-target/results.jsonl` (one JSON line per run, with its
`uptime`, idle share and log) — outside the repository, so the numbers below copy them.

## 1. What was measured on

| | |
|---|---|
| Tree | master `bc6ed1c0` (after wave 2, grammar 5.0.0 and ENG-01) |
| Grammar | tree-sitter-al `main` at `18c988e8`; `node-types.sha256` `e9005d07…` |
| Toolchain | rustc 1.96.0 (`ac68faa20`, 2026-05-25), cargo 1.96.0; pinned in `rust-toolchain.toml` |
| Flags | `[profile.dev] debug = "line-tables-only"`, dependencies `debug = false` and `opt-level = 2`; `.cargo/config.toml` sets `split-debuginfo = "packed"` for dev on every target, not only Windows; `release` fat LTO with `codegen-units = 1`; `release-fast` thin LTO with 4 units; `cdo` no LTO, 16 units, incremental |
| Machine | kraken container on server1: 22 cores, no CPU quota, 32 GiB cgroup memory limit (host 93 GB), `/work` on btrfs (so `cp --reflink` works) |
| Tools | `cargo-nextest` installed; no `hyperfine`, `mold`, `sccache` or `/usr/bin/time` (peak memory has to come from a wrapper; see section 6) |

The scripts behind every count are in `docs/superpowers/notes/crate-split-phase0/`. Each one says
in its header what it counts and how.

## 2. Size

`lines.py`, run from the repository root. Test lines are `*_tests.rs` files, `tests.rs` files,
and everything from the first `#[cfg(test)] mod …` to the end of a file. It is rough, as it was
in the design.

`src/` is **447,288 lines**. The design's figure of 434k was from master before wave 2.

| Group | Total | Production | Test |
|---|---|---|---|
| `src/engine/l5` without detectors | 124.7k | 84.0k | 40.7k |
| `src/engine/l5/detectors` | 24.6k | 20.2k | 4.4k |
| `src/program/resolve` | 62.0k | 28.5k | 33.5k |
| `src/program/` other, with flat files | 45.8k | 31.0k | 14.8k |
| `src/engine/l4` | 44.2k | 28.3k | 15.8k |
| `src/engine/{resident,impact,test_select,deps,host,batch}` | 43.2k | 30.0k | 13.3k |
| `src/engine/gate` | 19.2k | 14.7k | 4.6k |
| `src/program/model` | 18.9k | 13.4k | 5.5k |
| `src/lsp`, `src/mcp`, `src/wire` | 21.8k | 13.3k | 8.5k |
| `src/` flat files | 15.4k | 9.5k | 5.9k |
| `src/engine/` flat files | 12.4k | 8.1k | 4.3k |
| `src/bin` | 8.1k | 7.5k | 0.5k |
| `src/snapshot` | 6.9k | 4.9k | 2.1k |
| `crates/al-syntax` | 26.2k | 20.1k | 6.1k |
| `tests/` | 149.6k | | 149.6k |

Large single files the cuts touch: `engine/l4/capability_cone.rs` 9,591 lines (126 references to
`cones_census`), `engine/l4/pd_solve.rs` 2,964, `engine/l5/detectors/mod.rs` 2,601.

Other recounts for sections 4–7: 1,654 `pub(crate)` and 119 `pub(super)` in `src/` (the design
said about 1,650 and 119). `CARGO_MANIFEST_DIR` appears 96 times in `src/`, and 290 times in 167
files across `src`, `tests`, `benches` and `tools` (the design said 95 uses in 51 files, which
matches `src/` only). `CARGO_PKG_VERSION` appears 16 times. The package has 5 `[[bin]]` entries.

## 3. Module edges (design sections 2 and 6)

`edges.py`. It reads production code only: comments, the inline-test tails and `*_tests.rs`
files are skipped. It expands every `use crate::…` tree and every inline `crate::a::b` path,
resolves each to the deepest module file that exists, and maps both ends to the planned crates of
section 4.2 (the same map as `crates.py`). **Paths through `super::` and through re-exports are
not seen.** For example, `combined_graph.rs:479` reaches `super::summary_runner::summaries_census`
and is missing from the list below. So these counts are a floor.

- **One big cycle.** With modules grouped to three path components, there are 333 groups. The
  largest strongly connected component (a set of modules that all reach each other) holds 185 of
  them. The design said 150 groups at a coarser grouping.
- **Path references between planned crates:**

| From | To | References |
|---|---|---|
| al-engine-core | al-program | 568 |
| al-detectors | al-program / al-engine-core | 172 / 798 |
| al-engine | al-program / al-engine-core | 634 / 865 |
| facade | al-program / al-engine-core / al-engine | 160 / 14 / 76 |
| **back** al-program | al-engine-core / al-engine / facade | 12 / 6 / 3 |
| **back** al-engine-core | al-detectors / al-engine | 1 / 12 |

- **Back-edges: 24 distinct file pairs** (from file A to file B). Every one is in section 6 except
  the ones marked *new*:
  - al-program → al-engine-core:
    - `gate/workspace_diagnostics.rs`, `model/program_calls.rs`, `model/workspace.rs` and
      `resolve/full.rs` → `l5/registry.rs` (`Diagnostic`)
    - `l4/combined_graph.rs` → `l4/analysis.rs` (`graph_scc`)
    - `engine/perf_trace.rs` → `l5/detector_context.rs` (`report_cones_census`)
    - `body/ir_walk.rs` → `l5/literal_reach.rs`
    - *new*: `engine/root_classification.rs:844` → `l4::summary::build_routine_stable_map`
  - al-program → al-engine:
    - `config.rs` → `resident/budget.rs`
    - `program/inputs.rs` → `gate/policy/pipeline.rs` and `l5/snapshot_full.rs`
    - `model/workspace.rs` → `l5/routine_rollup.rs`
    - `resolve/full.rs` → `impact_graph/facts.rs`
  - al-program → facade: `program/dep_cache.rs` and `test_quality.rs` → `lsp/snapshot.rs`
  - al-engine-core → al-detectors: `detectors/d1.rs` → `detectors/d14.rs`
  - al-engine-core → al-engine:
    - `l4/analysis.rs` and `result_index.rs` → `batch/output.rs` (`job_line`)
    - `l5/digest.rs` and `l5/rollup.rs` → `l4/effect_query_cli.rs` (`rollup.rs`, *new*: `RECEIVER_TEMP_STATE`)
    - `vocab.rs` → `impact_graph/reverse.rs`
    - *new*: `follow_up.rs` → `l5/callers_context.rs`
    - *new*: `gate/format_json.rs` → `gate/projection.rs`
    - *new*: `l4/field_touches.rs:214` → `l5/routine_rollup::thousands` (section 6 already moves `thousands` down)
- Doc-comment links (`[`crate::…`]`) that point up are not compile edges and are not counted. They
  still break `cargo doc` after a split: `page_ui.rs`, `node_extract.rs`, `resolved.rs`,
  `resolve/full.rs:2307`, `engine/host/output.rs` and `model/flow_fields.rs` have such links.

## 4. Commits per planned crate (design section 2)

`crates.py`, fed with
`git log --since=2026-09-10 --no-merges --format='@%H' --name-only master -- src crates/al-syntax`.
A commit counts once for each planned crate it touches. Paths of deleted modules (`engine/l2`,
`engine/l3`) are left out. Master `bc6ed1c0` has the same 628 commits as `b8858256`, because
nothing since then touched `src/`.

| Planned crate | Commits touching it | Share of 628 |
|---|---|---|
| al-program | 350 | 56% |
| al-engine (upper: gate, batch, session, resident, impact, test_select, host, wire, impact_graph) | 238 | 38% |
| al-engine (l5 command files) | 207 | 33% |
| al-engine-core (l5 substrate) | 202 | 32% |
| facade | 188 | 30% |
| al-engine-core (l4) | 136 | 22% |
| al-detectors | 62 | 10% |
| al-syntax | 50 | 8% |

The third review counted 317 for al-program over 710 commits. The difference is the map: this one
puts `engine/mod.rs` and the low engine files of section 4.2 in al-program, keeps al-syntax
separate, and leaves out the deleted `l2`/`l3` paths.

**What decides the gain under candidate A is the lowest crate a commit touches.** An edit rebuilds
that crate and every crate above it.

| Lowest crate touched | Commits | Share |
|---|---|---|
| al-syntax | 50 | 8% |
| al-program | 319 | **51%** |
| al-engine-core | 118 | 19% |
| al-detectors | 4 | 1% |
| al-engine | 124 | 20% |
| facade | 13 | 2% |

So **59%** of commits rebuild from al-program or below, where A saves nothing in the inner loop.
**41%** skip al-program. The design said about 45% touch al-program and about 25% gain; by the
lowest-crate count the share that gains is larger, but the busy bottom is larger too.

Commits that stay inside one layer: al-program only 128, al-engine only 80, al-engine-core only
29, facade only 13.

## 5. Tests (inventory from source)

Counted as `#[test]` / `#[tokio::test]` attributes. Listing the tests exactly needs a build
(`cargo nextest list`), which is part 2.

| Target | Tests | Ignored |
|---|---|---|
| lib `al-sem` | 3,286 | 4 |
| lib `al-syntax` | 159 | 0 |
| `tests/cli` (89,997 lines) | 1,384 | 1 |
| `tests/gap` | 266 | 0 |
| `program_resolve_harness` (11,557 lines) | 208 | 3 |
| `l2_ir` | 72 | 0 |
| `lsp`, `temp_state` | 64 each | 0 |
| `r4`, `r3` | 61, 60 | 0 |
| `l4_summary_differential` | 20 | 0 |
| `perf_bounds` | 14 | 0 |
| `differential` | 8 | 0 |
| `r25_abi` | 4 | 0 |
| `cone_memory_bound`, `resident_memory_bound` | 1 each | 0 |

That is 14 integration-test binaries (8 umbrellas plus 6 standalone files). `tests/common` holds
5 more tests, compiled into each binary that includes it. There are 4 bench files and 1
compile-fail doctest (`program/resolved.rs`).

## 6. Number 1 of section 3.1: cold or warm

**Source.** The agentflow gate logs, `/work/alsem/.agent/runs/*/logs/<gate>/*.log`: 74 runs,
2026-10-20 to 2026-10-26, with cargo's own output. `coldwarm.py` splits each log into cargo
invocations (one per `Finished … in T` line) and classes each by the `Compiling` lines before it:
- **cold-deps**: more than 5 third-party crates compiled;
- **warm-edit**: only our own crates compiled;
- **no-op**: nothing compiled.

**The run-timer records (`/work/.al-sem/timings.jsonl`, 3,196 rows) cannot be classed.** They
store a command and its seconds, not the folder or cargo's output. Their totals are given
separately below.

| Class | Invocations | Build time | Share |
|---|---|---|---|
| cold-deps | 119 | 4.43 h | **42%** |
| warm-edit | 481 | 5.86 h | 55% |
| no-op | 559 | 0.31 h | 3% |
| total | 1,159 | 10.6 h | |

By profile, which matters more:

| Profile | Total | cold-deps | warm-edit |
|---|---|---|---|
| `release` (fat LTO, `ci-steps all`) | 6.60 h | 3.19 h | 3.25 h |
| `cdo` (`cdo-gate`) | 2.64 h | 0.81 h | 1.83 h |
| `test` + `dev` (task gate, inner loop) | 0.84 h | 0.21 h | 0.48 h |
| `release-fast` (bench) | 0.52 h | 0.21 h | 0.31 h |

Reading:
- **62% of gate build time is the fat-LTO `release` build**, and **25% is `cdo`**. A crate split
  does not shorten a fat-LTO link; the `cdo` profile has no cross-crate LTO, so it may gain a
  little. Of the dev and test time the split is aimed at, the gate logs hold only 0.84 h. Of that,
  25% is cold, 57% warm, and 19% no-op checks.
- **40 of the 74 runs did at least one cold dependency build.** Each fresh issue worktree started
  cold, once per profile.
- **The inner loop is mostly outside these logs.** The run-timer has `ci-steps task` 299 times,
  8.2 h in total (median 84 s, 90th percentile 170 s, max 436 s), and `check-goldens` 119 times,
  1.3 h. It also has release-fast builds: `--bin alsem` 61 times, 1.5 h (median 79 s); with
  `aldump`, 40 more. A median task gate of 84 s against a cold dev build of about 3 minutes
  suggests most task gates were warm. That is an inference, not a measurement.

**Number 1, as far as the logs can say it:**
- In the gates, cold builds are 42% of build time.
- In the dev/test profiles the split targets, they are 25% of the gate share.
- The task gates, which the logs cannot class, look mostly warm.
- The largest single block of build time is the fat-LTO release build, which no shape changes.

## 7. Part 2: how the timings were taken

Every batch runs only on an idle machine (`uptime` load under 2, no other cargo or rustc
process), announced to the orchestrator first, which holds lane tools' builds and `cdo-gate`.
server1 is shared with other kraken projects that nobody here can hold, so `timeit.py` records
`uptime` before every run and marks a run `noisy` when the machine was busy before it. The first
guard (a 1-minute load under 2) never passed: the host's background load sat at 3–13, and the
load average trails a build by minutes. From the second T1 pair on, the guard samples host-wide
idle CPU over 5 seconds (`/proc/stat`), waits up to 3 minutes for 90%, and runs anyway with the
flag. From 19:08 to 19:35 another project held the host at 0% idle; the runs were paused. Below,
a median is of the quiet runs where there are three; otherwise every value is given and the
noisy ones are named. CPU seconds (user + system of all children) are given beside wall time:
they are less sensitive to a busy host, though not immune (contention on shared cores inflates
them too, see the model rows).
Every run has its own target folder under `/work/eng04-target/`, so no peer's folder is touched.
Peak memory is the largest single child process (`resource.getrusage(RUSAGE_CHILDREN).ru_maxrss`
in `timeit.py`; there is no `/usr/bin/time`). The T1 builds passed `--timings`; their reports are in
`/work/eng04-target/timings-t1-*`.

**What ran, against the plan:**
- **T1 (cold):** as planned, for `cargo test --no-run -p al-sem -p al-syntax` (both libraries'
  tests, the 14 test binaries and the bins). There was no separate dependency-only build: the
  dependencies' share is the dependency-cold time minus the workspace-cold time (same command,
  same folder).
- **T2 (warm), in T1's third folder:**
  - Each edit adds or removes `std::hint::black_box(());` as the first statement of a function,
    with the Edit tool. The function body changes, the output does not, and clippy stays clean.
    Every edit was reverted afterwards.
  - Sites: `detectors/d60.rs::range_within`, `l5/pending_writes.rs::forward_sites`,
    `program/model/workspace.rs::member_detail_of`, and `al-syntax` `lower/mod.rs::collect_comments`.
  - **The renderer and MCP sites were dropped.** They are in the same crate, and the detector,
    substrate and model sites already showed that every edit inside `al-sem` costs about the
    same.
  - Edit-to-test runs `cargo test -p al-sem --lib rollup_multi_contributor_group` (one test,
    3,277 filtered out), so every site pays the same test run.
  - Edit-to-build runs `cargo test --no-run -p al-sem -p al-syntax`.
  - The task gate ran once, after the detector edit, following an untimed warm-up gate (clippy
    had never run in that folder).
- **T3:** `RUSTC_BOOTSTRAP=1 cargo rustc -p al-sem --lib --profile test -- -Ztime-passes` after a
  model edit, in a second warm folder. The variant was warmed by one run first.
- **T4:** a reflink copy of the warm folder, then the same build in a fresh worktree
  (`/work/eng04-wt`, the same commit) at another path. Three runs, each from a fresh copy, with
  cargo's fingerprint log.
- **T5:** in the warm folder: `cargo build --bins`, `cargo test --no-run`, then single packages
  and the whole workspace.

`output_cdo` and the full `cdo-gate` time are **still open**: they are to come from the
orchestrator's next real `cdo-gate` run, and there has been none. CLAUDE.md's figures from
2026-11-09 (the lanes 756–910 s, about 1,300 s with the speed phase) are the only ones.

## 8. Part 2: results

Commit `bc6ed1c0` plus this note's docs (no source change). Wall time in seconds; CPU = user +
system seconds of all child processes; peak = the largest single process.

**Cold (T1).**

| Build | Runs (wall) | CPU | Peak | Compiled |
|---|---|---|---|---|
| Dependency-cold: empty folder | 143.8, 141.3, **143.1** | ~1,270 | 5.0 GiB | 255 crates |
| Workspace-cold: deps warm, `al-sem` + `al-syntax` cleaned | 91.0, **91.3**, 92.6 | ~375 | 5.0 GiB | 2 packages |

Runs 1 of each (and depcold-2) started under the first guard at a 1-minute load of 3–21, so they
are flagged noisy. They agree with the quiet runs to within 2%. **So the dependencies are about
51 s of a 142 s cold build (36% of the wall time, 70% of the CPU; they compile in parallel),
and our own two crates are about 91 s.** The design guessed "about 3 minutes, plus about 3 for
the tests" for a cold build.

**Warm (T2).** Median of the quiet runs; in brackets, runs taken while the host was busy.

| After | Edit-to-test (one lib test) | Edit-to-build (every test target) |
|---|---|---|
| nothing (no-op) | 0.14 | 0.16 |
| a detector edit | **14.8** (14.8, 14.8; 15.4 noisy) | **18.5** (18.5, 18.4; 19.6 noisy) |
| a substrate edit | 15.0 (one quiet; 14.1, 15.3 noisy) | 17.6–18.8 (all three noisy) |
| a model edit | **17.0** (one quiet run of 17.0; 17.8, 19.9 at 65–85% idle; 33–37 at 1–2% idle) | **19.3** (one quiet run; 20.8–47.6 noisy) |
| an `al-syntax` edit, its own test (`-p al-syntax`) | **1.0–1.4** (after one 19.6 s run that recompiled 5 dependencies, see T5) | |

The model's two busiest runs also doubled the CPU seconds (54–57 against 27–28 on a quiet
host), so contention, not the model, made them slow. The peak memory of a warm edit-to-test is
3.1–3.8 GiB (the `al-sem` lib-test rustc).

**The task gate** (`scripts/ci-steps task`, after the detector edit, quiet): **159.9 s**, 1,814 CPU s,
peak 3.7 GiB. nextest's own summary: **112.8 s for 5,754 tests** (5,754 passed, 8 skipped). So
building, fmt, clippy and `gen-syntax` are about 47 s of the gate, and running the tests is about
113 s. This is the exact test inventory that section 5 could only estimate.

**The rerun-cost test (T2, al-syntax edit, then the `al-sem` lib test).** `al-syntax` and the
unchanged `al-sem` recompile:

| Run | Wall | CPU | Idle before |
|---|---|---|---|
| 1 | 21.8 | 23.2 | 33% |
| 2 | 17.5 | 19.3 | 51% |
| 3 | 12.5 | 14.6 | 84% |
| 4 | 12.2 | 13.8 | 79% |

All four are flagged noisy. The two quietest give **about 12.3 s against 14.8 s** for an edit
inside `al-sem` (CPU about 14 against 22–25). **So recompiling an unchanged large crate above an
edit costs about 75–85% of the wall time of editing it (55–65% of its CPU).** A crate above an
edit saves about 15–25% of its own rebuild; a crate an edit does not reach at all saves 100%.

**Where rustc spends the time (T3)**, a warm `al-sem` lib-test rebuild after a model edit, 18.7 s
inside rustc (19.0 s wall, at 55% idle):

| Part | Seconds | Share |
|---|---|---|
| Code generation (`codegen_crate`: LLVM IR 4.0, LLVM passes 4.1, with monomorphization and partitioning) | 7.1 | 38% |
| Front end: macro expansion 1.9, name resolution 0.8, type check 2.1, borrow check 0.5, other checks 1.1 | ~6.3 | ~34% |
| Saving incremental state (dependency graph 1.6, result cache 0.9) | ~2.5 | ~13% |
| Link (`rust-lld`, including the packed debug info step `run_thorin` 0.17) | 0.9 | 5% |
| Other | ~1.9 | ~10% |

**Candidate C (T4).** A reflink copy of the 20 GB warm folder takes **1.0 s**. The first build in
the fresh worktree with that copy:

| Run | Wall | CPU | Compiled |
|---|---|---|---|
| 1 | 118.2 | 461 | 2 packages |
| 2 | 98.1 | 395 | 2 packages |
| 3 | 93.3 | 379 | 2 packages |

Every dependency stayed fresh. `al-sem` and `al-syntax` rebuilt in full. Cargo's reason:
`stale: changed "/work/eng04-wt/build.rs"`, so the source paths and file times differ. rustc then
reused nothing from the incremental cache: the CPU time equals a workspace-cold build (about
375 s), because the cache belongs to the crate's identity, which includes the package path. **So
C, as designed (copy a warm folder into a new worktree), saves exactly the dependencies: about
51 s of a 142 s cold build, never the 91 s of our own crates.** A worktree built at the path the
folder was built at (for example, one fixed path per lane) might keep the rest; that was not tested.

**Build-to-test churn (T5).**
- `cargo build --bins` and `cargo test --no-run` share every artifact: 0 units recompiled either
  way.
- `cargo test --no-run -p al-sem` alone also recompiled nothing.
- **`cargo test -p al-syntax` alone resolves dependency features differently.** The first time,
  it recompiled `regex`, `regex-syntax`, `regex-automata`, `aho-corasick`, `cc` and `tree-sitter`
  (19.6 s). After that it was warm (both variants are kept).
- `--workspace` adds `al-sem-benches` (42 s, once).

A split's per-crate test runs (`-p <crate>`) would meet that churn once per folder.

## 9. Stop/go 0: the three numbers, and a recommendation

**The bound.** On this machine, today, a warm edit-to-test in the single crate already takes
**15–19 s**. The task gate takes **160 s**, of which **113 s is running the tests**. These two
numbers bound what any split can gain:
- In the inner loop, a split can save at most part of 15–19 s.
- In the task gate, it can save at most part of the 47 s that is not test running.

The design's own suggested limit (section 10) was to continue past Phase 1 only if a warm
edit-to-test still took over about 90 s. **It takes 15–19 s before Phase 1.**

**The three numbers of section 3.1:**
1. **Cold or warm.** Of 10.6 h of cargo time in 74 gate runs, 42% was cold dependency builds and
   55% warm rebuilds. 62% of all gate build time was the fat-LTO `release` build, which no shape
   changes. In the dev and test profiles a split targets, 25% was cold and 57% warm. A cold build
   is 142 s, and only 51 s of it is dependencies.
2. **The rerun cost.** Recompiling an unchanged `al-sem` above an `al-syntax` edit costs about
   75–85% of the wall time of editing it. A crate above an edit saves about 15–25%. So candidate
   A's chain gains only where an edit skips crates entirely: commits that touch neither
   al-program nor al-syntax, 41% of them (section 4). The other 59% gain nothing in the inner
   loop.
3. **Where rustc spends the time.** About 34% front end, 38% code generation, 13% saving
   incremental state, and 5% linking. Crate size matters (front end and codegen), so this number
   alone does not rule out A or B. Linking does not matter.

**Recommendation: none of A, B or C as designed. Stop the split after Phase 0, and put the next
week into the costs these numbers show are larger.**
- **A (the chain):** the best case is an edit in core or engine (41% of commits) that skips
  al-program. That saves part of a 15 s loop: by section 2's line counts, perhaps 5–8 s, unmeasured
  until a prototype. The al-program edits (59%) get nothing, may get slower, and meet per-crate
  feature churn. 4–6 weeks for that is poor value.
- **B (wide and shallow):** it has the same rerun cost above each edit and more cuts. Not costed,
  and the numbers do not invite costing it.
- **C (a warm copy):** as designed it saves only the dependencies, about 51 s per fresh worktree
  per profile, and the copy costs 1 s. It is worth doing in that reduced form, which takes days,
  not weeks: seed each new worktree's folder with a reflink copy, or a compiler cache for the
  dependencies. A fixed build path per lane might also keep our own crates warm. That is a
  separate, small test.
- **What the numbers point at instead:**
  - The task gate's 113 s of test running.
  - The fat-LTO `release` build, 62% of gate build time, run by `ci-steps all` and CI.
  - Section 8's cheap rows: `split-debuginfo = "packed"` applies on Linux too (`run_thorin` is
    small, so expect little); `task-affected` still builds every test binary.
  - Phase 1 (ENG-05) stays useful as that measurement, and goes ahead under every choice.
