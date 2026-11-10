# Crate split, Phase 0: baseline measurements

For the design `docs/superpowers/specs/2026-11-10-crate-split-design.md`, section 9 "Phase 0".
Task ENG-04 (kraken lane engine).

**Status.** Part 1 (no build: counts, commits, tests, cold-or-warm share) is done. Part 2 (the
timings on an idle machine) has not run yet. Its plan is in section 6.

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

## 7. Part 2: the timing plan (not run yet)

Every batch runs only on an idle machine (`uptime` load under 2, no other cargo or rustc
process), announced to the orchestrator first, which holds lane tools' builds and `cdo-gate`.
Each timing is the median of 3, in its own target folder (`CARGO_TARGET_DIR=/work/eng04-target/<name>`)
so no peer's folder is touched. Peak memory comes from a Python wrapper reading
`resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` (there is no `/usr/bin/time`). rustc invocation
counts come from `cargo … -v`. Every build passes `--timings` and the HTML report is kept.

| Batch | What | Commands | Expected wall time |
|---|---|---|---|
| T1 cold | dependency-cold, then workspace-cold (deps warm: `cargo clean -p al-sem -p al-syntax`), for `cargo test --no-run --workspace` (lib tests, the 14 test binaries, bins). The dependency share comes from `--timings` and from a dependency-only build (`cargo build` of the dependency list from `cargo metadata`) | 3 × 2 builds | ~35 min |
| T2 warm | no-op, then for each of 6 edits: edit-to-build (`cargo build --tests`), edit-to-test (`cargo test -p al-sem --lib <one test>`), and once each, the task gate (`scripts/ci-steps task`). Edits are output-preserving `let _ = 0;` lines added and removed with Edit, one each in: a detector (`detectors/d60.rs`), a substrate file (`l5/pending_writes.rs`), the model (`program/model/workspace.rs`), a renderer (`gate/format_terminal` or the like), MCP (`mcp/` tool dispatch), and **al-syntax** (`lower/mod.rs`, the rerun-cost test) | 6 edits × 3 + 6 task gates | ~60 min |
| T3 diagnosis | `RUSTC_BOOTSTRAP=1 RUSTFLAGS=-Ztime-passes` on one warm lib-test rebuild after a model edit: parse/expand, type check, borrow check, metadata, codegen, link | 1 build | ~10 min |
| T4 candidate C | reflink-copy (`cp -a --reflink=always`) a warm T2 folder into a fresh worktree at another path; time the first `cargo test --no-run` there against T1's cold build; record what cargo rebuilds and why (`CARGO_LOG=cargo::core::compiler::fingerprint=info`) | 3 builds | ~20 min |
| T5 churn | after a warm `cargo build --bins`, count the units `cargo test --no-run` recompiles (feature unification between build and test) | 1 | ~5 min |

`output_cdo` runtime and the full `cdo-gate` time are taken from the orchestrator's next real
`cdo-gate` run (its phase logs), not from an extra run: the gate needs the whole machine and its
memory question is open. CLAUDE.md's figures from 2026-11-09 (lanes 756–910 s, ~1,300 s with the
speed phase) stand until then.

Total machine time for T1–T5: about 2–2.5 hours of an idle machine.
