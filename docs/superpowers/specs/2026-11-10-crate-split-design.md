# Design: split the `al-sem` crate into a Cargo workspace

| | |
|---|---|
| **Status** | Design, ready for a new session to start from. Nothing is built, split or measured. |
| **Written** | 2026-11-10, against master `4bb454f7` and the wave-2 tree `integrate/w2` at `37c734e5`. |
| **Start when** | Engine-limits wave 2 and the tree-sitter-al 5.0.0 upgrade have landed, in the kraken-alsem container. |
| **Reviewed by** | Fable (verdict on v1: sound with changes) and gpt-6-astra via pi (verdict on v1: rework). This document folds in both. |
| **Third review** | Fable, 2026-11-10, against master `b8858256` (after wave 2 and grammar 5.0.0). Verdict: sound; gains re-weighted by where edits land (section 2), the rehearsal moved ahead of the cuts (Phase 1b), two-sided stop/go limits. It could not confirm the chain is the right shape: section 3.1 now lists three candidates and Phase 0 chooses. Folded in below. gpt-6-astra has not reviewed this version. |
| **Must also be reviewed** | By Fable and gpt-6-astra again before Phase 2, and on each phase's diff (owner's instruction). |
| **Who runs it** | A supervised session. The autonomous issue flow cannot: `scripts/agentflow/protect.py` blocks edits under `scripts/`. |

**Terms.**
- **Crate**: rustc's unit of compilation.
- **Back-edge**: an import that points up the planned layering.
- **Cut**: a code change that removes a back-edge.
- **Facade**: the root `al-sem` package. It re-exports the new crates, so existing `al_sem::…` paths keep working.
- **Front end** (of rustc): parsing, macro expansion, name resolution, type and borrow checking. Mostly single-threaded per crate.
- **Inner loop**: `cargo test -p <crate> --lib <filter>`. One test build of one library, and no test binaries.
- **Task gate**: `scripts/ci-steps task`, the whole suite.
- **Warm**: the build folder is already populated from an earlier build.

---

## 1. Why

Iterations are slow. On the 22-core machine, a cold dev build of the one `al-sem` crate takes about 3 minutes, and
the test build about 3 more (measured under load on 2026-11-10; no idle-machine baseline exists yet). Any edit
recompiles the whole crate.

Waves 0–2 showed where the time went. Most of it was not the tests themselves. It was many cold builds of one
large crate, run at the same time. The process rules that fix the concurrency half are already in force (at most
2–3 builders at once, reuse warm worktrees, `task-affected` while iterating, nothing beside `cdo-gate`). This
design addresses the other half: what has to recompile after an edit.

**Goal:** a shorter edit-to-test time for the common edits, without changing any answer the engine gives.

**Not goals:** a faster cold build (a small side effect at most); changing any output; changing any public
binary, wire string or golden.

---

## 2. The facts this design rests on

### Size (lines of Rust under `src/`, about 434k in total, on master)

| Group | Total | Of which test code (rough) |
|---|---|---|
| `src/engine/l5` | 143k | 31k (the detectors alone are 24k) |
| `src/program/resolve` | 61k | 33k |
| `src/engine/l4` | 44k | 15k |
| `src/engine/{resident,impact,test_select,deps,host,batch}` | 43k | small |
| `src/program/{body,impact_graph,session,pack}` and flat files | 45k | 11k |
| `src/engine/gate` | 19k | 4k |
| `src/program/model` | 16k | 3k |
| `src/lsp`, `src/mcp`, `src/wire` | 22k | 8k |
| `src/bin` (alsem 5.4k, aldump 2.4k, mint-goldens 0.3k) | 8k | |

`tests/` holds another 158k lines in 15 test binaries. `tests/cli/` alone is 81k. (Recount on master `b8858256`:
`src/` is 447k lines, and `ls tests/*.rs tests/*/main.rs` gives 14 test binaries. Redo both in Phase 0.)

### Which code changes most

Commits touching each area since 2026-09-10 (600 commits on master): `engine/l5` 254, `program/resolve` 178,
`engine/l4` 160, `bin` 107, `program/model` 100, `gate` 87, `mcp` 64, `lsp` 47. By file-touch counts on the
wave-2 tree: l5 1,366, program/resolve 459, l4 386, program/model 295, gate 210, mcp 109, lsp 81.

**The same count per planned crate** (third review; 710 commits touching `src/` since 2026-09-10, on master
`b8858256`; a commit counts once for each crate it touches). The per-directory numbers above hide this:

| Planned crate | Commits touching it |
|---|---|
| al-program (`program/` without `impact_graph`, `snapshot/`) | 317 |
| al-engine: upper engine (`gate`, `batch`, `session`, `resident`, `impact`, `test_select`, `host`, `wire`, `impact_graph`) | 232 |
| facade (`lsp`, `mcp`, `bin`, `analysis`) | 190 |
| al-engine-core: l5 substrate | 187 |
| al-engine: l5 command files (prove, explain, digest, …) | 184 |
| al-engine-core: l4 | 174 |
| al-detectors | 80 |

Commits that stay inside one layer: core plus engine only 179; al-program only 106; facade only 14; detectors
only fewer than 10. So:
- **Detector-only edits are about 1% of commits.** Of the 254 `l5` commits, 80 touch a detector at all.
- **About 45% of commits touch al-program**, the bottom crate, where the split gives nothing in the inner loop
  and may slow the task gate.
- **The common case that gains is an edit inside core or engine** (179 commits): it skips al-program entirely.

Commits are a stand-in for edit-test cycles, not the same thing. It is the only stand-in this design has.

### The module graph is one big cycle, with few upward edges

Counting production code only, 150 module groups (all of program, snapshot, engine, lsp, mcp, wire, telemetry,
config, dependencies…) form one strongly connected component. Only small leaves sit outside it (`types`,
`source_text`, `capped_io`, `record_ops`, `state_paths`, `git_env`, …). So no split is free. But the imports that
point upward are few and mostly small items. They are listed in section 6.

### What checks out already

- **Orphan rule** (an `impl` must live with its type or its trait): a name-based scan of 998 `impl` lines found no
  impl on a type from another planned crate. The two exceptions (`impl Model` in `l4/combined_graph.rs`, `impl
  Tally` in `engine/census.rs`) move with their types. The scan is approximate; the compiler has the final word.
- **Macros**: no `macro_rules!` crosses a planned boundary.
- **Global allocator**: in `lib.rs` under `cfg(test)`, and in `bin/alsem.rs`. Also in three leaf programs that the
  split does not touch: `tests/cone_memory_bound.rs`, `tests/resident_memory_bound.rs`, `tools/census-probe`.
- **Detectors below the upper engine**: no file under `l5/detectors/` (57 files, tests included) imports `gate`,
  `batch`, `session`, `resident`, `impact`, `test_select`, `host` or an l5 command file. al-detectors can sit
  below al-engine as drawn.
- **Edges into the facade**: production code below it imports `lsp` in only two files, `program/dep_cache.rs` and
  `test_quality.rs` (both in section 6). This is what makes the early rehearsal (Phase 1b) cheap.
- **`build.rs`**: matters only to `src/server.rs:376` (the telemetry connection string), so it stays in the root.
- **Declared-needs**: `Plan` fields are private (`engine/session.rs`). `BuildProfile` is built only at
  `program/resolved.rs:52`, `program/session/workspace.rs:110` and `resolve/full.rs`, all planned for al-program.

---

## 3. What to expect: honest gains

An earlier estimate counted "lines rebuilt". Both reviewers rejected that model, and so does this design.

- Cargo reruns rustc on **every** crate above an edit when an upstream crate's output changes. There is no skip
  for "only a function body changed". Incremental compilation makes those reruns cheaper, but each still pays a
  fixed cost (parse, expand, resolve, dependency-graph load and save, metadata), plus relinking.
- The 15 test binaries and 4 program binaries are a **floor** the split does not remove.
- For a change low in the graph, the front-end work becomes a **serial chain** of crates. Cargo overlaps only code
  generation (pipelining).

| Edit in | Share of commits (section 2) | Inner loop (a target to test, not a forecast) | Task gate |
|---|---|---|---|
| a detector only | about 1% | 40–60% shorter | a smaller gain; unknown until measured |
| core or engine only (l4, l5 substrate, prove, explain, digest, gate) | about 25% | skips al-program (about 140k lines); size of the gain unknown | a smaller gain; unknown until measured |
| MCP or LSP only | about 2% | large | limited by the test-binary floor; no 80–90% promise |
| al-program (the model, the resolver, the IR) | about 45% touch it | about the same | **may be slower** |
| cold build | | little (10–25% at best) | |

**Read the table by its share column.** The best row is the rarest edit. The case for the split rests on the
second row, and on al-program edits not getting worse. Phase 0 must time both, and the stop/go limits (section
10) are two-sided for that reason.

Every number above must be replaced by a measurement before it is quoted. That is what Phases 0, 1b, 3 and 4 are for.

### 3.1 Three candidate shapes: Phase 0 chooses

The third review found the design consistent, but could not say it is the right one. Two doubts:

- **The graph in section 4.1 is a chain, and the busiest code is at its bottom.** A chain is the worst shape for
  build time: an edit rebuilds every crate above it, one after another. The chain follows the engine's pipeline,
  not the edits (section 2: about 45% of commits touch al-program).
- **Section 1 names cold builds in fresh worktrees as where the time went, and a split barely helps a cold build.**

So sections 4–7 describe **candidate A**. Nothing after Phase 1 starts until Phase 0 has compared it with the other two.

| | Shape | Helps | Cost | Not known |
|---|---|---|---|---|
| **A. The chain** (sections 4–7) | syntax → program → core → detectors → engine → facade | warm edits in core and engine (about 25% of commits) | 4–6 weeks; the cuts are listed | the size of the gain; whether al-program edits get slower |
| **B. Wide and shallow** | A stable base (IR facts, ids, the model's types), with the busy parts as siblings above it: the resolver beside the model builder; prove, explain, digest, gate, impact and test selection side by side over the l4/l5 substrate | warm edits in most places, al-program's included; sibling crates compile in parallel | more cuts than A, because the modules form one cycle (section 2) and siblings must not import each other. Not costed. | whether the cycle can be cut this way at all; the cut list |
| **C. No split: never build cold** | One crate, as now. A new worktree starts from a warm build folder (a copy-on-write copy of a warm `target/`, or a compiler cache for the dependencies, which build at opt-level 2 and are not incremental) | cold builds, the stated pain | days, not weeks | whether cargo reuses a copied folder under another path (section 8 rules out a *shared* folder because paths are baked into binaries; a copy that is then rebuilt in place is a different thing and is untested); how much of a cold build the dependencies are |

A and C can be combined, and B can start as A's first step (al-program out) and then widen. They are not exclusive;
the question is which one the next week of work goes to.

**The three numbers that choose** (all in Phase 0):
1. **Cold or warm.** Of real build time in a working week, how much is cold builds? Sources: `/work/.al-sem/flow-timings.jsonl`
   and the run-timer records of the build commands. Mostly cold → C first. Mostly warm → A or B.
2. **The rerun cost** (the `al-syntax` test in Phase 0). If rerunning an unchanged large crate costs nearly as much as editing
   it, a crate above the edit saves nothing: only B's siblings and skipped lower crates help, and A's chain gains little.
3. **Where rustc spends the time** (`-Ztime-passes`). Mostly front end → crate size matters, so A or B. Mostly linking and
   test binaries → none of the three; go to the test-binary floor (section 8, "Split only the tests") instead.

**Stop/go 0.** The owner picks A, B, C or a mix from these numbers. Before choosing B, cost its cuts: one day with the
module-graph tool that produced section 2's cycle count.

---

## 4. The design (candidate A)

### 4.1 Target crate graph

```
al-syntax            (exists, 24k lines: the only crate that touches tree-sitter)
  └─ al-program      (about 140k total, 82k production)
       └─ al-engine-core   (about 115k, 67k)
            ├─ al-detectors  (about 24k, 19k)
            └─ al-engine     (about 120k, 92k; depends on core and detectors)
                 └─ al-sem   (facade, root package: lsp, mcp, analysis, every bin, tests/; about 34k)
```

The line counts are from master. Redo them after wave 2.

### 4.2 What goes where

**al-program**
- Small root modules: `types`, `source_text`, `capped_io`, `big_stack`, `record_ops`, `census_hook`, `git_env`,
  `state_paths`, `protocol`, `config`, `telemetry`, `app_image`, `app_package`, `dependencies`, `language`,
  `release_alloc`, and the IR half of `test_quality`.
- `snapshot`, and all of `program` except `impact_graph`. This includes wave 2's `program/split_decls.rs`,
  `program/resolve/dataset.rs` and `program/model/flow_fields.rs`, which use only al-program in production code.
- Low engine parts, kept at their old `engine::` paths:
  - `ids`, `perf_trace`, `census` (with the census counters), `root_classification`, `snapshot`;
  - `deps`, minus `dep_artifact_l4.rs` and `r3a4_projection.rs`;
  - `l4::{combined_graph, scc}`;
  - `gate::{version, model_instance_id, workspace_diagnostics, app_attribution}`;
  - the moved items in section 6;
  - `lsp/encoding.rs` (`LineIndex`, pure, 411 lines).

**al-engine-core**
- `l4`, minus `effect_query_cli.rs` (but see section 6: two of its helpers move down).
- The l5 substrate: `detector_context`, `registry` (with the substrate bits), `full_summary`, `d1_*`, `ordering*`,
  `literal_*`, `path_*`, `digest`, `snapshot`, `fingerprint`, `capability_query`, `rules_index`, `table_display`,
  `transaction_spans`, `confidence`, `pending_writes`, and from wave 2 `commit_scope` and `traps`.
- `group_path` (`digest.rs` holds its outcome type).
- Shared parts of `detectors/d1.rs`, `detectors/mod.rs` (`anchor_of`, `walk_paths`, `PathQuestion`, `LoadPoint`…) and `detectors/d65.rs`.
- Small vocabulary modules: `follow_up`, `result_header`, `result_index`, `line_base`, `joined_key`, `path_segments`,
  `source_excerpt`, `return_summary`, `vocab`, `gate::{cbor, format_json}`.

**al-detectors**
- The rest of `src/engine/l5/detectors/`, with `registered_detectors`, `default_detectors`, `opt_in_detectors`.

**al-engine**
- The command files in l5: `digest_cli`, `digest_v3`, `explain`, `prove`, `prove_slice`, `fingerprint_cli`,
  `fingerprint_query`, `callers_context`, `emit_policy`, `snapshot_full`, `test_coverage`, `selector_index`,
  `routine_rollup`, ….
- `l4::effect_query_cli`.
- The rest of `gate`; `batch`, `session`, `incremental`, `switch_dump`, `resident`, `impact`, `test_select`, `host`, `wire`.
- `program::impact_graph`, `engine::deps::{dep_artifact_l4, r3a4_projection}`.

**al-sem (the facade, root package)**
- `lsp`, `mcp`, `analysis`, the LSP half of `test_quality`, `cdo_gate_coverage_tests`, `build.rs`, every `[[bin]]`, and `tests/`.

### 4.3 Module ownership: one owner per module

Every production module has exactly one owning crate. Write this as an explicit map before Phase 4 and keep it
beside the layering test (section 7).

Several `mod.rs` files are each shared by 2–3 planned crates and must be split or replaced: `src/engine/mod.rs`,
`engine/l4/mod.rs`, `engine/l5/mod.rs`, `engine/gate/mod.rs`, `engine/deps/mod.rs`, `program/mod.rs`.
`src/engine/l5/detectors/mod.rs` (2,601 lines) mixes helpers with registration and needs a real file split.

### 4.4 Layout: leave the files where they are, during the migration

Each new crate gets `crates/<name>/Cargo.toml` with `[lib] path = "../../src/<name>.rs"`. That root file declares
the modules it owns, for example `pub mod program; pub mod snapshot; pub mod engine { pub mod ids; pub mod l4 { pub
mod combined_graph; pub mod scc; } … }`. Rust resolves child files relative to the root file's folder, so every
file stays at its current path.

This keeps working: doc, skill and spec paths; the path map in `scripts/ci-steps`; relative `include_str!` paths
(`engine/gate/run.rs:1641`, `engine/host/mod.rs:192`, `engine/session.rs:1899`); and test names, because each crate
rebuilds the same module tree.

Cargo allows a `[lib] path` outside the package folder for local builds, but `cargo package` rejects it, so every
new crate sets `publish = false`. State edition 2024 explicitly. Rustfmt, clippy, rustdoc and rust-analyzer should
cope because they follow cargo metadata, but nobody has tested this. The Phase 3 prototype checks it.

Keeping files in place is a migration tactic. A later move to `crates/*/src` is a separate choice; git follows
renames, so blame is not a strong reason to keep the unusual layout forever.

**Hazard:** with this layout, a module file no crate root declares is simply not built, and nothing reports it.
Test accounting (section 7) guards against this.

**The opposite hazard:** a file declared by two crate roots is compiled twice. Each crate gets its own copy of
the file's types, and nothing reports that either. The layering test (section 7) must check that every file
under `src/` is declared by exactly one root.

### 4.5 Keeping the old paths working

- **Facade.** `src/lib.rs` re-exports the lower crates, so tests, benches and bins keep `al_sem::…`.
  `src/main.rs:17` already uses this pattern.
- **A module split across crates.** The upper crate merges the lower part into its own module:
  `pub mod program { pub use al_program::program::*; pub mod impact_graph; }`.
  The facade must export the **composite upper module**. A plain `pub use al_program::program` would lose `impact_graph`.
- **Moved items** keep a `pub use` at their old path, so most upper-crate call sites do not change.
- Re-exports preserve type identity, but they do **not** redirect `crate::` or `super::` inside the defining crate.
  Code in a lower crate naming `crate::engine::l5::registry::Diagnostic` must change. `literal_values.rs` uses
  `super::Bindings`, so its logical parent must remain correct.

### 4.6 Visibility

There are about 1,650 `pub(crate)` and 119 `pub(super)` items on the wave-2 tree.

- Keep `pub(crate)` wherever the compiler allows.
- Each item that must cross a boundary gets a decision. Do not widen constructors, mutable fields or preparation
  methods by default.
- Costs of `pub`: such an item is never reported as dead code, and several clippy lints fire only on `pub` items
  (`new_without_default`, `len_without_is_empty`, `should_implement_trait`, …). CI uses `-D warnings`, so budget for this.

### 4.7 Invariants that must survive

- **Declared needs.** `BuildProfile` (`program/profile.rs`, no `Default`) and the view needs in `program/inputs.rs` stay
  together in al-program. `BuildProfile` has a public field, so `#[non_exhaustive]` alone does not stop mutation of a
  copy; use private fields and getters if construction control is intended. Substrate bits stay in `registry.rs`. `Plan`
  and `View` keep private fields. Keep the parity test `tests/gap/gap_detector_substrate_parity.rs` and the missing-needs tests.
- **`src/program/resolved.rs:66` is a compile-fail privacy doctest.** Update and keep it. Do not rename it away or mark it
  `ignore`, and do not leave a stale `al_sem` import that makes it fail for the wrong reason.
- **Thread-local state.** `engine/batch/output.rs`: `job_line`, `capture` and `JOB_LINES` share one slot. Move them together.
- **One copy of singletons.** Telemetry and allocator state must be re-exported, never compiled twice. Allocator installation
  stays in `src/bin/alsem.rs`. The counting allocator moves with `engine/session_phase_tests.rs`, under test configuration only.
- **The four things that keep the old `al-call-hierarchy` name** (CLAUDE.md) are untouched. All `[[bin]]` entries and `default-run` stay in the root package.

### 4.8 Detectors: pass a catalog, avoid a global

About 25 call sites use `default_detectors()` / `registered_detectors()`. Making al-detectors a true leaf needs those callers
to get the catalog from outside. Prefer passing an explicit catalog into command and session construction, keeping order,
defaults, opt-ins and `requires` together. A global `OnceLock` adds a startup requirement to every binary, test and bench.
Only do this if measurements justify it (section 9, Phase 4).

### 4.9 Features

Internal dependencies use `default-features = false`, and the facade forwards features. Forward `telemetry`,
`engine-test-hooks` and `test-runtime` (`telemetry/runtime.rs:56`). Without it, the release job's `cargo build
--no-default-features --bin alsem` would quietly keep telemetry. `cfg(test)` does not propagate into dependencies, so moving
telemetry changes what its `cfg!(test)` sees when an upper crate's unit test calls it.

Test the default, no-default, `test-runtime` and `engine-test-hooks` builds explicitly.

---

## 5. Tests: the largest piece of work

Moving a test to an upper crate means it sees the lower crate's **normal** build. `super::*` access and every `#[cfg(test)]`
helper disappear. This was underestimated by a factor of 2–3 in the first plan.

**The scope, as far as known (redo after wave 2):**
- Large inline test modules that use upper layers:
  - `engine/l4/capability_cone.rs`: 9,585 lines, about 6,900 are tests. They use `engine::batch`, `gate::run::AnalysisTarget`, `session`, `effect_query_cli`, `registered_detectors()`, `default_detectors()`.
  - `engine/l4/pd_solve.rs`: about 2,300 test lines, and a `#[cfg(test)]` serial-oracle hook in production code at line 1242.
  - `engine/l5/snapshot_view.rs`, lines 1045–1205.
- `detector_context_{11b,288,step5,arm}_tests.rs` and `transaction_spans_order_tests.rs`: they call detectors and `l5::explain`.
- `digest_rule_b_tests.rs`, `wave2_index_tests.rs`, `prove_slice_tests.rs`, `summary_runner_seeded_tests.rs`.
- In al-program: `program/model/{events,record_types,flow_fields,census,projection}.rs` tests, `model/rules_tests.rs`,
  `model/role.rs`, `program/impact_graph/*_tests.rs`, `resolve/demand.rs:104` (uses `impact_graph::project::tests::World`),
  `census_hook.rs` and `dep_cache.rs` tests.
- 191 `#[cfg(test)]` helper items in `src/` (38 in l5, 13 in detectors, 11 in l4, 5 in program/model).
- Six switches whose behaviour differs for a test in an upper crate, because they would see the production branch:
  `l5/full_summary.rs:121`, `telemetry/install_id.rs:115`, `telemetry/runtime.rs:56`, `release_alloc.rs:239`,
  `engine/host/output.rs:445`, `mcp/output.rs:448`.

**"Always compile `test_support`" does not work.** `l5/test_support.rs` calls `FullRoutineSummary::with_inherited` and
`inherited_raw`, and both, with the field they use, exist only under `cfg(test)` in `full_summary.rs`. Un-gating them changes
the production struct and restores an API that was deliberately removed. `program/session/workspace.rs` has similar test-only fields.

**The approach. Sort every helper three ways:**
1. Plain fixture constructors, which can be shared (`#[doc(hidden)] pub`, or a fixture crate that creates no cycle).
2. Raw-fact oracles, which stay with the owning crate's unit tests.
3. Cross-layer scenario tests, which move up or into `tests/` and are rewritten on explicit fixture data.

**Never** widen a production struct so that a moved test compiles.

**Rejected:** a `test-support` cargo feature (on for `cargo test`, off for `cargo build`: everything above rebuilds at each
switch), and dev-dependency cycles (cargo allows them, but it links two copies of the lower crate and the types no longer match).

---

## 6. Known cuts (back-edges to remove)

Each must be checked for its own dependency closure when moved. Each lands as its own small change, in the single crate (section 9, Phase 2).

**`program/model` → engine**
- `model/{calls,program_calls}.rs` → `l4::combined_graph::{TriggerFires, trigger_fires}`; `model/projection.rs` → `l4::combined_graph::PrimaryGraph`. (`combined_graph` moves down.)
- `model/{workspace,program_calls}.rs`, `resolve/full.rs` → `l5::registry::Diagnostic`. Move the struct down.
- `model/workspace.rs` → `l5::routine_rollup::infer_member_kind`, `root_classification::*`, `gate::app_attribution::App`.
- `model/program_calls.rs` → `gate::workspace_diagnostics::compute_workspace_diagnostics`.
- `model/workspace.rs` → `test_quality::{body_counts, is_test}`.

**`program/inputs.rs` → engine**: `l5::snapshot_full::{SnapshotInput, derive_inputs_from_parts}`, `gate::policy::pipeline::absolutize`, `gate::version::driver_version`, `gate::model_instance_id::*`, `root_classification::roots_config_bytes_load`.

**`program/impact_graph` → engine** (`impact_graph` moves up instead): `mod.rs` → `engine::batch::{job, output::job_line}`; `reverse.rs` → `l4::summary::build_routine_stable_map`; `sited.rs` → `l5::routine_rollup`; `facts.rs` → `root_classification::{RootShape, root_kind_mask}`. Needs `facts::property_object_ref` down (used at `resolve/full.rs:958`) and `vocab`'s `EntryKind`/`Tier` down.

**Other program and snapshot files**: `body/ir_walk.rs:1935` → `l5::literal_reach::values::VARIANT_TESTS`; `program/dep_cache.rs:105,117` → `lsp::encoding::LineIndex`, `lsp::snapshot::DepEventLinks` (move the plain data down rather than use `Any`); `test_quality.rs:21` → `lsp::snapshot::LspSnapshot` (split the file).

**Missed by the first plan, found by the reviews**
- `l4/combined_graph.rs:1404` → `l4::analysis::graph_scc`. Move **only** `graph_scc` (a small function) beside `scc`, with its counter. Do not move `analysis`, which imports the solver and `l5::detector_context`.
- `l4/combined_graph.rs:479` → `summary_runner::summaries_census::mixed_readings`. Move these counters to `engine::census`.
- `cones_census::add_compose_stats` takes `capability_cone::ComposeStats`. Move the counters and reporting down; keep this typed adapter above them, or move a neutral stats type down. `capability_cone.rs` has 126 `cones_census` call sites.
- `detectors/d1.rs:2085` → `detectors::d14::provably_dead_routine_ids` (a core-to-detectors edge).
- `l5/digest.rs:4155–4166` holds `group_path::GroupPathOutcome`. `group_path` goes in core.
- Wave 2: `l5/digest.rs:4376` → `l4::effect_query_cli::{owner_qualified, object_display}`. Move those two down.
- Wave 2: `commit_scope` and `traps` are used by core and engine. They go in core.

**Between core and engine (18 edges)**: move `digest_v3` and `field_guard` to engine; move `SUMMARY_CAP_BYTES`, `job_line` (with its state), `CallersQuestion`, `RECEIVER_TEMP_STATE`, `thousands`, `FindingLocation` / `FindingSummary` down.

**Smaller**: `config.rs:416` → `engine::resident::budget::parse_bytes`; `engine/perf_trace.rs:790` → `report_cones_census`; `engine/census.rs` → `l4::{combined_graph, scc}`; `engine/deps/dep_artifact_l4.rs` (moves up); `engine/vocab.rs:13` → `impact_graph::reverse::{EntryKind, Tier}`; `engine/resident/session.rs` → `release_alloc`.

---

## 7. Safety rails

Every step is a pure refactor. No golden moves; any golden diff is a bug.

1. **Layering test.** It fails when a file in the planned lower set references a module outside it. A scanner of about 150 lines
   can be a test or a `ci-steps` step. Start it as a ratchet: the list of remaining violations can only shrink.
   Once crate roots exist it also checks that every file under `src/` is declared by exactly one root (section 4.4).
2. **Test accounting, from the first change.** Compare package, target, test name, ignored status and doctests, against an explicit
   relocation map. Checking `.d` dependency files is **not** a proof: old files survive, `include_str!` inputs appear there, and a file's test module can vanish while its production module stays.
3. **Gates.** Every cut in Phase 2 passes `scripts/ci-steps task` (which runs every golden target) on its own. The branch gate
   (`scripts/ci-steps all`, `scripts/cdo-gate` including its speed phase) runs once per landed batch of cuts, on the exact tree
   about to merge. `cdo-gate` takes 15–20 minutes and must run alone, so one run for each of about 40 cuts is not affordable.
   Every extraction in Phases 1b and 4 gets the full branch gate on its own.
4. **Runtime.** `cdo-speed` covers release-fast only. Also measure the shipped fat-LTO build and the `cdo` workload (`output_cdo` is about 780 s).
   The `cdo` profile (no cross-crate LTO, 16 codegen units) can lose inlining of small `Model` accessors. Measure first; prefer `#[inline]` on hot accessors. Do not prescribe ThinLTO before measuring build plus run.

### What must keep working

| Thing | What to do |
|---|---|
| `LINT_PKGS` in `scripts/ci-steps` | Add each crate. A missing package is silently unlinted and unformatted. |
| `affected_filter` in `ci-steps` | It only maps `crates/al-syntax/*`. Teach it the new crates. |
| `package(al-sem) & kind(lib)` | Becomes `kind(lib)`. |
| `scripts/cdo-gate` | `-p al-sem --lib cdo` must cover every library crate. Prebuild new lib-test targets before the parallel lanes. Add expected test counts: a filter matching zero tests can pass silently. |
| `src/cdo_gate_coverage_tests.rs` | Asserts literal command text, including `-p al-sem --lib cdo -- `. |
| `on_the_cdo_gate` in `tests/common/cdo.rs` | Keyed on the crate name from `module_path!()`. Add one arm per new lib crate. New crate roots with CDO tests add their own `#[path]` include. |
| `CARGO_MANIFEST_DIR` (95 uses, 51 files) | A repo-root helper. |
| `CARGO_PKG_VERSION` (14 uses, including an impact cache key) | `[workspace.package] version`, inherited by every crate. |
| `tools/census-probe` | Depends on `al-sem`; works through the facade. |
| `tests/program_resolve_harness.rs:11536` | Uses core (`compose_callsite_resolutions`), so it cannot simply become an al-program test. |
| CI | `release.yml` and `build-and-deploy.yml` use `--no-default-features --bin alsem`. Needs feature forwarding. |
| CLAUDE.md | `cargo test -p al-sem --lib <filter>` needs the right `-p` afterwards. Update the Build Commands and Test tiers sections. |
| After a split | `cargo clean -p al-sem` no longer cleans the extracted code. Clean the whole package set. |

`[profile.dev.package."*"] opt-level = 2` applies only to non-workspace packages, so new crates stay at opt-level 0. That is the right behaviour, but it is a change worth knowing about.

---

## 8. Alternatives considered

| Option | Verdict |
|---|---|
| **`task-affected` selects what it compiles** | Do first. It passes `--workspace -E …`, which filters which tests RUN but still builds every test binary. |
| **`split-debuginfo`** | Measure first. `.cargo/config.toml` sets `packed` (meant for Windows); it also applies on Linux and may add a packing step to every link. Compare packed, unpacked, off. Test `debug = 0` for test targets separately. |
| **mold linker** | Measure first. Confirm the actual linker (Rust 1.96 uses rust-lld on x86_64 Linux). The claim that links are a minority is unmeasured. |
| **Dev profile tweaks** | Measure on total build-plus-test time. Dependency opt-level 2 also affects generic sharing. |
| **Split only the tests** | Moving public-API-heavy tests out can reduce the big lib-test compile. Splitting `tests/cli` trades more parallel compile for more links and repeated helpers; keep CDO tests that share expensive process-local state together. Keep as optional later work. |
| **sccache** | Little use for warm edits: it does not cache incremental builds, which all workspace crates are in dev. For cold builds in fresh worktrees it can cache the dependencies; measure under candidate C (section 3.1). |
| **A shared target folder across worktrees** | Ruled out: paths are baked into binaries (`scripts/agentflow/worktrees.py`). A *copied* warm folder is a different thing: candidate C. |
| **Cranelift backend, `-Zthreads` parallel front end** | Nightly only; `rust-toolchain.toml` pins stable 1.96 for clippy. `-Zthreads` targets the real bottleneck, so revisit if it stabilises. |
| **`codegen-units`** | About zero expected: dev already uses 256 units with incremental. |

The split is the only option that changes *what* recompiles. The others shave constant costs, and they are cheap, so they go first.

---

## 9. Migration plan

Each phase ends green, and the owner can stop at each stop/go point.

### Phase 0: baseline (½–1 day)

Record the exact tree, grammar, toolchain, flags and test inventory. Then, on an idle machine (check `uptime`), median of 3, in equally warmed target folders:
- Measure separately: dependency-cold, workspace-cold, warm no-op, warm edit-to-build, warm edit-to-test, and test-only iteration.
- Edits to time, each **output-preserving** (a changed message could fail a test and falsify the timing): one detector, one substrate file, the model, a renderer, MCP.
- Record peak memory, rustc invocation count, link time, build-to-test feature churn, `output_cdo` runtime and full cdo-gate time, and the per-unit durations from `cargo build --timings`.
- Once, for diagnosis: `RUSTC_BOOTSTRAP=1 RUSTFLAGS=-Ztime-passes` on a warm lib-test build. It shows the split between parse, type check, metadata, codegen and link, which every estimate here depends on.
- **The rerun-cost test.** Change one function body in `al-syntax` (output-preserving), then time the warm rebuild of the
  `al-sem` lib-test. Nothing in `al-sem` changed, so this is the price of rerunning rustc on an unchanged large crate: the
  price every upper crate pays after the split. Compare it with the warm edit-to-test time for an edit inside `al-sem`. If
  the two are close, the split gains little above the edited crate and only the skipped lower crates count.
- Redo the per-crate commit table of section 2, and weight every timing by it.
- **Cold or warm:** the share of a working week's build time that is cold builds (section 3.1, number 1).
- For candidate C: copy a warm `target/` into a fresh worktree (copy-on-write) and time the first build there against a
  cold one. Record what cargo rebuilds. Also time a dependency-only cold build, to size what a compiler cache could save.

**Stop/go 0.** Section 3.1: the owner picks the shape. Phase 1 goes ahead under every choice.

### Phase 1: cheap wins (1–2 days)

The rows of section 8 marked "do first" or "measure first". Keep the full merge gate unchanged. Fix the stale comment about "151 integration-test binaries" in the Windows config.

**Stop/go 1.** Re-measure. If the inner loop is acceptable, stop here.

### Phase 1b: rehearsal, the facade split (3–5 days)

Phases 1b to 4 are candidate A's plan. They run only if stop/go 0 chose A (or B, which shares Phase 1b).

Moved here from Phase 4 by the third review, so that the first real numbers arrive before the 2–3 weeks of cuts and not after.

Extract everything below `lsp`, `mcp` and `analysis` as one crate, using the in-place layout of section 4.4. It needs only the
two facade cuts (`program/dep_cache.rs`, `test_quality.rs`), the scaffolding (Phase 4 step 1) and test accounting. It gives:
- a test of the layout against cargo metadata, per-package rustfmt, clippy, rustdoc and rust-analyzer;
- a test of the scripts, the gates and CI on a real second crate;
- the first measured cost of a crate boundary, on this code: the task gate, the cold build, `cdo-gate` and its speed phase,
  before and after.

Its direct gain is small (about 2% of commits touch only the facade). It is kept if it is green and no slower; it is not
throwaway.

**Stop/go 1b.** With these numbers and Phase 0's rerun-cost test, forecast the gain for a core-or-engine edit and the cost
for an al-program edit. Stop here if the forecast misses the limits in section 10.

### Phase 2: cuts inside the single crate (about 2–3 weeks)

Small, green, independently reviewable changes. No further workspace change yet, and every cut stands as a cleanup even if the rest stops.

**Beside other work.** The cuts move `Diagnostic`, split `detectors/mod.rs` (2,601 lines) and move about 6,900 test lines out of
`capability_cone.rs`. An engine-limits wave on the other lane touches the same files. Rules: one cut per branch, landed the day
it is green; a large move (a file split, a test relocation) lands only while no wave branch is open on that file, and the
orchestrator orders the two; a wave is never rebased across a half-landed move.
1. Add the layering test and test accounting.
2. Inventory every inline test module and `cfg(test)` helper that crosses a planned boundary. Re-estimate.
3. Do the cuts in section 6.
4. Do the helper separation in section 5.

Fable and gpt-6-astra review the updated design before this phase starts.

### Phase 3: prototype (2–3 days, throwaway)

Build a compiling, test-enabled `al-program` with its wrapper modules. The tooling checks were done in Phase 1b; this one tests the
split modules (`engine::l4`, `program`) and the wrapper re-exports of section 4.5. Re-estimate from what it shows.

**Stop/go 2.** The owner approves the graph and the numbers.

### Phase 4: extraction (1–2 weeks; one mechanical change per crate; re-measure and stop/go after each)

The recommended order:
1. **Scaffolding** (`[workspace.package]`, `[workspace.dependencies]`, the repo-root helper, scripts taking a package list). Done in Phase 1b.
2. **Engine and program out of the facade.** Done in Phase 1b.
3. **al-program.** The biggest structural gain: every core-or-engine edit skips it.
4. **al-engine-core and al-engine.**
5. **al-detectors.** Last, and only if measured: detector-only edits are about 1% of commits (section 2), so a detector-first order is no longer recommended.

Optional afterwards: make al-detectors a leaf (section 4.8), and lower the test-binary floor (move suitable test targets into al-program's own tests; split `tests/cli`). Both need their own measurement.

### Effort (unmeasured)

| Phase | Effort |
|---|---|
| 0 baseline | ½–1 day |
| 1 cheap wins | 1–2 days |
| 1b rehearsal (facade split) | 3–5 days |
| 2 cuts and test separation | 2–3 weeks |
| 3 prototype | 2–3 days |
| 4 extraction (the rest) | 1–2 weeks |
| **Total, if run to the end** | **about 4–6 weeks** |

Stopping after Phase 1 costs about 2 days. Stopping after Phase 1b costs about a week and leaves a working two-crate workspace.

---

## 10. Open questions for the owner (at the start of the new session)

0. **The shape** (section 3.1), after Phase 0: the chain (A), wide and shallow (B), no split with warm worktrees (C), or a mix. Questions 1, 3 and 4 apply only to A and B.
1. **First extraction.** Recommended: the rehearsal (Phase 1b, before the cuts), then al-program. Alternative: al-program directly. Detector-first is dropped (section 2).
2. **Stop/go thresholds, two-sided.** Suggested:
   - Continue past Phase 1 only if a warm edit-to-test inner-loop run still takes over about 90 seconds on an idle machine.
   - Continue past Phase 1b, and past each later extraction, only if a core-or-engine edit gets (or is forecast to get) at least 20% shorter edit-to-test, **and** nothing gets worse than these limits: an al-program edit's inner loop and task gate 10% slower; the cold build 15% slower; `cdo-gate` and its speed phase within their existing limits.
   - Judge the total by the commit shares of section 2, not by the best row.
3. **File layout.** Keep files in `src/` during the migration (recommended). Decide later on moving them.
4. **Crate names.** al-program, al-engine-core, al-detectors, al-engine.

## 11. What nobody has checked yet

- Any timing in this document, including the 3-minute baseline on an idle machine and all gain estimates.
- That rustfmt, clippy, rustdoc and rust-analyzer cope with crate roots in `src/` and a `[lib] path` outside the package.
- Exact test counts and that every lib test keeps its name.
- Which `pub(crate)` items really cross a boundary; only the compiler can tell.
- Any effect on `cdo-gate`'s speed phase, memory or runtime.
- The line counts, edge counts and test lists after wave 2 and the grammar upgrade. Redo them in Phase 0. The third review spot-checked nine cited lines on master `b8858256` and corrected the two that had moved; the rest of section 6 is unchecked.
- Candidates B and C (section 3.1): neither is costed, prototyped or reviewed. B's sibling layout is a sketch, not a cut list.
- That commits per crate (section 2) match edit-test cycles per crate.
- The limits suggested in section 10 (10%, 15%, 20%): they are proposals with no measurement behind them.
