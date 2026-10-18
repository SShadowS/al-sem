# #160 child spec: steps 0, 1, 1b, 2 — measurement, frozen inputs, retained program, owned artifacts

Status: implementable design, pre-implementation. Child of
`2026-10-18-issue-160-analysis-session-design.md` (the epic). The epic holds the goal,
the architecture, every Decision, the invariants (I1–I11) and the delivery order. This
document holds the exact types, functions, call sites, tests and measurements for the
first four steps. Where the two disagree, this document is wrong.

Base: master `1e54624a`, worktree `U:/Git/al-sem-issue-160-a1`. Every `file:line` is in
that tree and was read before it was cited. Section numbers are the ones the epic cites
(§4.1 counters, §4.3 step 1b, §4.4 step 2, §6 bands, §8 discrimination proofs).

Words: **cell** = one owned artifact of a `View`, filled once by the driver, released
after its last reader. **mask** = the `substrate::*` bit set a detector run demands
(`src/engine/l5/registry.rs:45-141`). **context** = today's `DetectorContext<'a>`
(`src/engine/l5/detector_context.rs:607-803`). **band** = the noise band of one timer:
max − min over its parent-commit runs (epic I6).

## 1. Goal

1. **Step 0.** Every number a later step is bounded on exists before that step's spec is
   written: timers that split what the critiques showed was lumped together (G-1, G-3),
   build counters that make a duplicate build fail a test (I1), both peak numbers (§5.12),
   a byte census of every retained structure, and per-timer medians AND bands (G-5).
2. **Step 1.** Every read of a workspace file that a command makes happens once, before
   the semantic build, into one frozen `InputSet`. No command re-reads the disk after
   the build. The one stated exception is the loader's own read of each `.alpackages`
   `.app` (§4.2.2): `InputSet` hashes it, the loader parses it, as today. The second
   workspace parse (`src/engine/gate/workspace_diagnostics.rs:119-129`) is gone. One
   stdout writer with one failure rule.
3. **Step 1b.** The program build's result is a value (`ResolvedProgram`) that outlives
   the model's assembly; `alsem graph` reads it instead of running a private build
   (`src/program/impact_graph/mod.rs:191-193`). The body is released where it is
   released today (`src/program/model/program_calls.rs:669-676`).
4. **Step 2.** The detector context splits into an owned `DetectorArtifacts` cell plus a
   borrowed `DetectorContext<'v>`; the L4 solve becomes its own cell (`L4Analysis`);
   a driver-owned `View` fills cells in a fixed order; every command builds ONE solve
   and ONE context — except `policy check`, whose RAW-facts build stays a second,
   command-local `DetectorArtifacts` outside the `View` until step 6 deletes
   `RAW_INHERITED_FACTS` (epic I5, Decision 50; §4.4.4). digest's duplicate context
   (`src/engine/l5/digest_cli.rs:534-560` against `:1655-1660`) and prove's
   (`src/engine/l5/prove.rs:838-859` against `:902`) are deleted.

Steps 0, 1, 1b, 2 are byte-neutral (epic I2): no golden target moves, no `--lib` test
outcome moves, the CDO `aldump` histogram SHA is unchanged. The stderr and exit-code
changes are listed in §7, by step.

## 2. Non-goals

- No `ExecRelation` (step 4), no cone work (7a, 7b), no identity work (10, 11), no
  snapshot on `Transfer::Cone` (6), no batch (12). Step 2 keeps today's cross-app base
  (`R3a5CrossAppBase`, `src/engine/l4/capability_cone.rs:3329-3370`) as the thing a
  `View` holds; `rel` replaces it at step 4.
- No witness scope, no `ViewTree`: digest, prove and fingerprint keep composing today's
  snapshot (`src/engine/l5/snapshot.rs:1150-1156`), only later in the command (§4.4.4).
- No persistent cache; `InputSet` is a value, not a key (epic §2).
- No change to the LSP binary's build path beyond the `SnapshotBuilder` signature it
  shares (§4.2.6).
- No new output. The numbers file is documentation, not a product surface.
- The `pd_solve` retain-or-re-solve choice (G-1's design half), the `ViewTree` retention
  shape (G-2's design half), the 7a bound (G-3's design half), the arena's step (G-4) and
  the serialization filter (R6-4) stay with steps 6 and 7a. §9 says what this document
  hands them.

## 3. Acceptance matrix

Each row is one test or one measurement. "Measurement" rows are recorded in the step's
numbers file and compared under §6.

| # | Claim | Evidence | Step |
|---|---|---|---|
| A0.1 | Every timer in §4.1.1 exists and is zero-cost off | `cargo test -p al-sem --lib cones_census`: with the census off, `start()` returns `None` (existing contract, `detector_context.rs:454-456`); CDO SARIF byte-identical census on vs off (§6.2) | 0 |
| A0.2 | Every build counter in §4.1.2 reads from the trace | `tests/cli/cli_build_counters.rs`: per command and universe, the counter table of §4.1.3 exactly | 0 |
| A0.3 | A duplicate build fails A0.2 | one discrimination per counter (§8.1) | 0 |
| A0.4 | Both peaks are reported | the trace's last `peak.process_mb` and `peak.post_attach_mb` counters; the stderr mirror prints both | 0 |
| A0.5 | The byte census covers §4.1.5's buckets and changes no output | CDO SARIF byte-identical census on vs off; the census prints a per-bucket line and a residual | 0 |
| A0.6 | The full-graph timings exist | `aldump --issue160-full-graph-timings <ws>` prints the four numbers of §4.1.6 | 0 |
| A0.7 | Medians and bands are recorded per timer | the numbers file §4.1.7, one row per timer, ≥ 3 interleaved runs | 0 |
| A1.1 | `InputSet` is read once | the `inputs.read` counter (§4.1.2) is 1 per command in the trace (A0.2's reader) | 1 |
| A1.2 | No reader touches the disk after the build | one disk-edit-after-read test per input class (§4.2.3, §8.2), including a `.al` file ADDED after the read (`inputs::al_file_added_after_read`: today `select_program_files` walks again and fails closed on it, `workspace.rs:1644`, `:1657`) | 1 |
| A1.3 | The second parse is gone | `workspace_diagnostics.rs` no longer calls `al_syntax::parse` (the index rows come from the build's `files_without_object`, §4.2.4); `inputs::objectless_file_removed_after_read` (§8.2) is the executable pin: the row survives the file's deletion, so it came from the build, not from a walk. The one corner whose bytes move is stated in §7 | 1 |
| A1.4 | One writer, one failure rule | closed-pipe test: `analyze` exits 2 with `al-sem: write error: stdout: <OS message>`; `digest --out` into an unwritable path exits 1 with the `<path>` line (§4.2.5, §8.2) | 1 |
| A1.5 | Byte-neutral | 8 golden targets + `cargo test -p al-sem --lib`; CDO histogram SHA unchanged | 1 |
| A1.6 | `--update-baseline` keeps today's output | `baseline::update_baseline_applies_the_set_just_written`: stdout and exit code equal today's (every current finding dropped); a malformed old file is overwritten, never read (§4.2.3, §8.2) | 1 |
| A1.7 | A failed `.al` walk still fails closed | `inputs::walk_error_fails_the_build`: the build `Err` text, `model_instance_id == None` and the diagnostics equal today's (§4.2.1, §8.2) | 1 |
| A1b.1 | `alsem graph` output unchanged | `tests/cli-query-goldens/graph.test-selection-graph.jsonl` byte golden (`tests/cli/cli_graph_export.rs:702-716`) | 1b |
| A1b.2 | The body is released at today's point | the `b3.ctx_drop` span is still where it is (`program_calls.rs:674-676`) and A1b.4's post-attach peak is flat; no unit test (`body.take()` makes `is_none()` true by construction, see §8.3) | 1b |
| A1b.3 | The two edge sets reach the projection in today's order | `project::add_keeps_first_via_on_conf_tie` + `resolved::all_edges_is_edges_then_dependency_bodies` (§8.3); dropping `dependency_body_edges` or `site_facts` from the projection input → `graph_export_covers_dependencies` fails (`cli_graph_export.rs:597-650`) | 1b |
| A1b.4 | No new peak | both peaks ≤ parent + band on CDO analyze and digest (§6.3) | 1b |
| A1b.5 | `InputSet` bytes are in the census | the `inputs` bucket prints a number (5.09 MB expected on CDO) | 1b |
| A2.1 | One solve per command; one context per command (policy: two, its RAW build is the stated second, §4.4.4) | A0.2's table at its §4.4.8 values | 2 |
| A2.2 | Byte-neutral | 8 golden targets + lib; CDO detector baseline identical (`cdo-measure`) | 2 |
| A2.3 | digest is faster | CDO digest wall saving ≥ 21 s − band(digest wall) against the parent (§6.3) | 2 |
| A2.4 | No new co-residency | both peaks ≤ parent + band on CDO analyze and digest | 2 |
| A2.5 | Output never depends on fill state | `RunKey` carries the mask; the superset-mask parity test compares the whole `DetectorOutput` (§4.4.6) | 2 |
| A2.6 | prove's `Some(root)` moves nothing | cli-b prove goldens unchanged; wrong root set → "entry missing" (§8.4) | 2 |
| A2.7 | The failure classification keeps one build | `build.program` count on the no-id and build-failed rows is 1; the no-id Err text is pinned (§4.4.5) | 2 |
| A2.8 | The cell census runs at the first detector run | `det` and `l4` buckets print under the census flag | 2 |
| A2.9 | No lazy fill anywhere in `View` | `session::no_lazy_fill_in_this_file` (`--lib`): `include_str!("session.rs")` contains neither `OnceLock` nor `Mutex` (the needles are spelled `concat!("Once", "Lock")` so the test's own text does not match); `every_detector_alone_declares_what_it_reads` stays green (`registry.rs:998-1016`). Not a `scripts/ci-steps` grep: the issue flow may not edit `scripts/` (CLAUDE.md:873) | 2 |
| A2.10 | What a post-run projection reads off `det` is in the plan | `session::after_run_bits_must_be_in_the_union`: `prepare` on a `Plan { after_run: TRANSACTION_SPANS }` whose union mask lacks the bit panics, naming the bit (§4.4.3, §4.4.4, §8.4); digest/prove declare `TRANSACTION_SPANS` (the tx map), events `SUMMARIES` (the fanout's coverage read) | 2 |

## 4. Design

### 4.1 Step 0 — the measurement harness

Everything here is diagnostic. The rule of `perf_trace` and `cones_census` holds: with
the flag off, a probe is one `OnceLock` read and nothing else (`src/engine/perf_trace.rs:9-12`,
`src/engine/l5/detector_context.rs:449-463`). A probe that costs production time gets
deleted for the wrong reason.

#### 4.1.1 Timers

All in `cones_census` (`detector_context.rs:344-559`), gated by `ALSEM_CONES_CENSUS=1`,
read through the existing `start()`/`add_since()`/`add()` helpers. A renamed counter is
a new counter: the old name is deleted, never left reading zero (perf-probe law 2).

| Timer / counter | Where it is taken | What it answers |
|---|---|---|
| `BFS_WALK_NANOS`, `BFS_FOLD_NANOS`, `BFS_PICKS` (replace `BFS_NANOS`) | `capability_cone.rs:2552-2561`: walk = `inherited_facts_by_bfs` (`:2553-2560`), fold = `emit_picks` (`:2561`); `BFS_PICKS` += `picks.len()` before the fold | G-3: how much of the 10.0–10.5 s is the walk 7a removes and how much is the per-pick fold only `fold_row` (7b) removes |
| `SGL_SCAN_CONE_NANOS`, `SGL_SCAN_OWN_NANOS`, `SGL_SUBST_NANOS` (replace `SGL_SCAN_NANOS`); `SGL_CONE_ENTRIES`, `SGL_OWN_ENTRIES`, `SGL_OPEN_ENTRIES` (replace `SGL_CONE_ENTRIES`) | `capability_cone.rs:1776-1798` (`subst`: the frame-bound entries carried per edge), `:1812-1852` (the cone scan; count closed entries and open entries from `edge_subst` apart), `:1856-1873` (the own-entry loop) | the singleton cost per entry class: closed cone entries (7b's memo target), open (frame-bound) entries (stay on `substitute_entry`), own entries (G-1's population) |
| `PD_SOLVE_NANOS`, `PD_SOLVE_CALLS` | around `super::pd_solve::solve(..)` at `capability_cone.rs:2447` | G-1: the solve's share of the untimed 4.0–4.8 s compose remainder; a second solve is a second call |
| `PD_SOLVE_BYTES_ARENA`, `PD_SOLVE_BYTES_WORKING` (replace `PD_SOLVE_BYTES`) | `pd_solve.rs:465-500`: `solve_bytes` splits into `arena_bytes(arena)` (producers, states, links, edges, `by_routine`: what step 6 may retain) and `working_bytes(resolved, names, reps)` (the resolved own entries, the interned names, the reps: what step 6 decides to retain or re-solve) | G-1: the two budgets step 6 chooses between |
| `COMPOSE_REST_NANOS` | `compose_inherited_cones`, charged PER CALL: at entry read the eight component atomics (`FACTCONE`, `COVERAGE`, `BFS_WALK`, `BFS_FOLD`, `SINGLETON`, `DERIVED`, `EMIT`, `PD_SOLVE`) into locals; at exit add `elapsed − Σ(component now − component at entry)`. The atomics accumulate over the process (digest composes three times, §4.1.3), so a whole-process subtraction would be meaningless; the per-call delta is not | what is still unexplained after the split (the report prints it; a negative value is a timer overlap bug) |
| spans `b3.adapter.join`, `b3.adapter.by_decl`, `b3.adapter.convert` | inside `adapter` (`program_calls.rs:922-970`): `join(..)` (`:929`; the span join J1), the `by_decl` map (`:933-943`; J2), the conversion loop (`:972` onward) | steps 10 and 11 read `join` and `by_decl` (epic Decisions 45, 46) |

The census report (`detector_context.rs:465-558`) prints the new lines in place of the
old ones. `pd_solve::census` (`pd_solve.rs:435-457`) charges the two byte counters.

#### 4.1.2 Build counters

A counter is `pt::counter_delta("build.<name>", 1)` (`perf_trace.rs:599-603`), which
emits a Chrome `C` event with the running total. With tracing off it is one `OnceLock`
read. The reader takes the LAST `C` event per name. Counters are taken at the function
entry when the function is the thing counted, and at the call site when the function is
also called from inside another counted thing (Decision 21: the SCC counter never sits
inside `tarjan_scc`, which the db-effect solver also calls, `src/engine/l4/db_effect_solver.rs:93-99`).

| Counter | Sites |
|---|---|
| `build.program` | `build_program_with_coverage_profiled` entry (`src/program/resolve/full.rs:2136`) |
| `build.combined_graph` | `build_combined_graph` entry (`src/engine/l4/combined_graph.rs:247`) |
| `build.cone` | `compose_cone_over_graph` entry (`capability_cone.rs:2764`) |
| `build.summaries_v2` | `compute_summaries_v2_bundle_with_leaves` entry (`src/engine/l4/summary_runner.rs:1071`); the other three entries funnel into it (`:1374-1383`, `:1412-1420`, `:1445-1457`) |
| `build.graph_scc` | the top-level `tarjan_scc` call sites: `capability_cone.rs:2783` (cone), `:3198` (snapshot fold), `:3477` (base), `:3824` (`project_r3a3`, aldump), `detector_context.rs:1303`, `src/engine/l4/effect_query_cli.rs:237`, `src/engine/l4/summary.rs:859` (`project_r3a2`, aldump), `src/engine/l4/combined_graph.rs:1128` (`project_r3a1_combined_graph`, aldump). Not: `db_effect_solver.rs:99` (inside the solve), d7's own private Tarjan (`src/engine/l5/detectors/d7.rs:144`, the call; `:94` is its `fn`), the two test helpers (`effect_query.rs:424`, `reverse_index.rs:946`) |
| `build.pd_solve` | `pd_solve::solve` entry (`pd_solve.rs:253`) |
| `build.detector_context` | `build_detector_context_with` entry (`detector_context.rs:899`) |

After step 2 two sites move with the code they count: `build.detector_context` sits at
`build_detector_artifacts`' entry (§4.4.2; the test-only shim calls it, so a test counts
it too, which is what A0.2's reader wants), and `detector_context.rs:1303`'s
`build.graph_scc` sits at the single-app Tarjan call inside `build_l4` (§4.4.1; `build_l4`
is never called from inside another counted thing, so Decision 21 holds). The deleted
sites (`effect_query_cli.rs:237`, the context's own graph) are deleted with their code.
A2.1's table is the check.
| `build.snapshot` | `compose_snapshot_from_base` entry (`src/engine/l5/snapshot.rs:1161`) |
| `build.ordering_facts` | `compute_ordering_facts` entry (`src/engine/l5/ordering_facts.rs:218`) |
| `inputs.read` (step 1) | `InputSet::read` entry (§4.2.2) |

The reader is `tests/cli/cli_build_counters.rs`, built on the pattern of
`tests/cli/cli_envelope_detector_set.rs:35-76` (spawn `CARGO_BIN_EXE_alsem` with
`ALSEM_TRACE=1`, `ALSEM_TRACE_FILE=<tempfile>`, every other `ALSEM_TRACE_*` removed).
One function `trace_counters(args) -> BTreeMap<String, u64>` (last value per name, 0
for an absent name). One test per row of §4.1.3 on `tests/r0-corpus/ws-d1-multi-caller`
(cross-app rows need a fixture with a dependency: the workspace
`tests/cli/cli_digest_query_dependencies.rs:54-100` writes, a workspace plus a
source-bearing dependency `.app` from `tests/common/symbol_app.rs`; the spec calls it
"the dependency workspace" below). The table is the test's
literal expectation; a step that moves a count edits the table in the same commit.

#### 4.1.3 Today's counts (what the step 0 test pins)

Per command and universe. `scc` counts only the sites of §4.1.2. `ctx` =
`build.detector_context`, `snap` = `build.snapshot`, `v2` = `build.summaries_v2`,
`bcg` = `build.combined_graph`.

| Command | program | bcg | cone | v2 | scc | pd_solve | ctx | snap | Where the duplicates are |
|---|--:|--:|--:|--:|--:|--:|--:|--:|---|
| analyze, cross-app, default set | 1 | 3 | 1 | 1 | 3 | 1 | 1 | 0 | base builds 2 graphs (`capability_cone.rs:3470`, `:3512`) and the context a third (`detector_context.rs:940`); base SCC (`:3477`) and context SCC (`:1303`) are the same graph |
| analyze, cross-app, `--detector d47-…` | 1 | 4 | 2 | 2 | 5 | 2 | 1 | 1 | plus the ordering cell's own snapshot (`digest.rs:4016-4019`) |
| analyze `--single-app` | 1 | 1 | 1 | 1 | 2 | 1 | 1 | 0 | none |
| digest, cross-app | 1 | 5 | 3 | 4 | 7 | 3 | 2 | 1 | snapshot (`digest_cli.rs:1580`), tx map's context (`:1616` → `:534-560`), bundle (`:1627`), diagnostics' context (`:1655-1660`) |
| digest `--single-app` | 1 | 4 | 3 | 4 | 7 | 3 | 2 | 1 | the same, with `build_db_effect_bundle` (`effect_query_cli.rs:226-261`) |
| prove | 1 | 4 | 3 | 4 | 7 | 3 | 2 | 1 | snapshot (`prove.rs:897`), diagnostics' context (`:902`), tx map's context (`:958`), bundle (`:964`) |
| fingerprint (json, query, `--inventory-only`) | 1 | 2 | 2 | 2 | 4 | 2 | 1 | 1 | diagnostics' context (`fingerprint_cli.rs:364`) + the snapshot's own cone (`snapshot_full.rs:71`); no duplicate |
| diff ws ws (per side; `--strict` adds ctx 1, bcg 1, cone 1, v2 1, scc 2, pd 1) | 1 | 1 | 1 | 1 | 2 | 1 | 0 | 1 | none |
| events fanout / chains, cross-app | 1 | 4 | 2 | 2 | 5 | 2 | 2 | 0 | diagnostics' context (`events.rs:406`) + its own (`:422-426`, `:567-571`) |
| events, `--single-app` | 1 | 2 | 2 | 2 | 4 | 2 | 2 | 0 | the same |
| policy check, cross-app, policy loaded | 1 | 4 | 2 | 2 | 5 | 2 | 2 | 0 | diagnostics' context (`pipeline.rs:216`) + its own at `ALL \| RAW_INHERITED_FACTS` (`:261-266`) |
| policy check, `--single-app`, loaded | 1 | 2 | 2 | 2 | 4 | 2 | 2 | 0 | the same |
| query touches / effects, cross-app | 1 | 2 | 0 | 1 | 1 | 0 | 0 | 0 | none (`effect_query_cli.rs:207-220` reuses the base's graph and SCC) |
| query, `--single-app` | 1 | 1 | 0 | 1 | 1 | 0 | 0 | 0 | none |
| graph | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | none |

These match the epic's counts for digest (5 graphs, 4 solves, 3 cones; epic §3).

#### 4.1.4 The post-attach phase peak

The OS peak (`peak_working_set`, `perf_trace.rs:121-171`) is a lifetime maximum set in
attach (2,847 MB on CDO, epic §3), so it cannot see a later retention. The phase peak is
sampled:

- `pt::mark_phase_start("post_attach")` is called right after the `b3.ctx_drop` span
  closes (`program_calls.rs:674-676`). With tracing on it takes one sample
  synchronously, then spawns one sampler thread that reads `read_rss().working_set`
  every 10 ms and keeps the maximum; the stop takes one more sample synchronously. So
  the phase peak is never 0 on a run that attached, however short (the
  `ws-d1-multi-caller` fixture finishes in well under 10 ms after attach, and
  `phase_peak_is_reported`, §8.1, asserts `0 < post_attach`). With tracing off it is a
  `OnceLock` read.
- `alsem`'s `main` (`src/bin/alsem.rs:667`) opens `pt::run("alsem")` (`perf_trace.rs:569-577`,
  unused by the binary today). `RunGuard::drop` (`:453-461`) stops the sampler and emits
  two counters: `peak.process_mb` (the OS peak at exit) and `peak.post_attach_mb` (the
  sampler's maximum; 0 when attach never ran). `ALSEM_TRACE_STDERR=1` prints both on one
  line at exit.
- A sample interval of 10 ms misses spikes shorter than that. The band absorbs what it
  misses; the instrument is the same on both sides of every comparison (§6.1).
- A second `mark_phase_start` in the same process (diff attaches once per side,
  `diff/cli.rs:103` → `program_calls.rs:727`) RESTARTS the sampler: the maximum is reset
  and `peak.post_attach_mb` reports the last phase's maximum. For diff that is the
  second side; the numbers file says so where it quotes a diff peak.

#### 4.1.5 The byte census

`ALSEM_RETAINED_CENSUS=1` (value-tested). It REPLACES `C1_CONE_CENSUS=1`
(`src/engine/l4/cone_census.rs:77-80`): `cone_census::enabled()` reads the new name, the
two residual-census sites that key on it (`capability_cone.rs:2675`, `:2851`) print under
the new flag, and `emit_full_census` (`cone_census.rs:201`, the `[C1_CONE_CENSUS]` report
of `summaries` + `cone_derived`) is deleted in favour of the `context.<field>` buckets
below, which cover the same two structures per field. The per-fact byte accounting
(`cone_census.rs:86-190`) is kept and called by the new walk. One flag, one report;
the old report's name is gone, never left printing nothing (§7, step 0).
Accounting follows `cone_census.rs:17-63` and the byte-census skill: heap bytes by
`len`, shared allocations once, allocator overhead excluded, every bucket prints its
share and the residual is named. One `census.rs` module under `src/engine/` with one
`fn bucket(name, bytes, allocs)` printer; each site walks its own structure.

| Bucket | Walked where | What it holds |
|---|---|---|
| `program.snapshot_texts` | just before `b3.report_drop` (`program_calls.rs:670`) | every `SourceFile.text` of every app in `ctx.snapshot()` (`src/snapshot/snapshot.rs:34-69`) |
| `program.parsed_ir` | same point | the workspace unit's and the dependency units' `ParsedFile.file` (`src/snapshot/parse.rs:17-29`) |
| `program.graph` | same point | `ctx.graph()` (`full.rs:1538-1540`) |
| `program.report` | same point | `report.edges`, `site_facts`, `parenless_calls`, the histograms |
| `inputs` (step 1) | same point | the `InputSet` (§4.2.1); `al_files[].text` counted once even though the snapshot shares the `Arc` |
| `model` | end of `attach_program_calls_with` (`:689`) | `Model` (`src/program/model/workspace.rs:1890-1914`): rows, `calls`, `events` |
| `base` | end of `build_cross_app_base` (`capability_cone.rs:3565-3585`) | per field of `R3a5CrossAppBase` (`:3329-3370`): `graph`, `direct_full`, `leaf_summaries`, `injected_typed_edges`, … |
| `context.<field>` | end of `build_detector_context_with` (`detector_context.rs:1564-1616`), replacing `emit_full_census` (`:1161`) | one line per field of `DetectorContext` |
| `snapshot.<field>` | end of `compose_snapshot_from_base` (`snapshot.rs:1161-1190`) | one line per field of `CapabilitySnapshot` (`:881-899`): this is the `ViewTree`-equivalent G-2 asked for (`capability_facts`, `typed_edges`, `operation_index`, `callsite_index`, `coverage`, …) and the derived `pd_solution` |
| `arena` | `pd_solve::census` (`pd_solve.rs:435-457`) | the two halves of §4.1.1 |
| `det`, `l4` (step 2) | the first `View::run` | the owned artifacts and the solve |

A walk over 15 M cone entries is not free; it runs only under the flag, and A0.5 proves
the flag changes no byte of output.

#### 4.1.6 Full-graph timings for step 6's stop rule

Step 6 moves the snapshot from the cut graph (`resolved.calls` as the base cuts it,
`capability_cone.rs:3457-3467`, `:3564`) to the full one. Its stop rule (Decision 52)
needs today's cost of the snapshot over the FULL graph, measured before any step 6 code
exists. `aldump --issue160-full-graph-timings <ws>`:

1. builds the cross-app base;
2. runs `compose_cone_over_graph(&base.graph, &base.nodes, &base.direct_full,
   &base.direct_coverage, base.ws_routines(), ConeOutput::RawOnly)` — the base's graph
   already carries the injected dependency edges (`:3513-3515`), so this is the cone the
   context walks, with raw output for every routine;
3. assembles an `R3a3SourceBase` from those parts and runs `compose_snapshot_from_base`;
4. prints: the cone's wall, `PD_SOLVE_NANOS` (G-1: the solve is inside the cone's wall,
   and is printed on its own), the snapshot derivation's wall, and the raw population
   (inherited facts summed, bytes by the census).

This is a measurement-only path: it ships behind the aldump flag, is not called by any
command, and is deleted when step 6 lands.

#### 4.1.7 The numbers file

`docs/measurements/2026-10-<dd>-issue-160-step0.md`. Sections: machine and binary
(commit, profile `release-fast`, `RAYON_NUM_THREADS`); per command and universe the
§4.1.3 counts as read from the trace; per timer of §4.1.1 and per span the epic's
bounds read (`context.capability_cones`, `b3.adapter`, `b3.adapter.join`,
`b3.adapter.by_decl`, `analyze.total`, digest's wall, `crossapp.attach_program_calls`)
the median and the band over ≥ 3 interleaved runs (§6.1). A `cones_census` timer is a
process-cumulative atomic (`detector_context.rs:348-424`) and `report()` (`:465-558`)
prints the running totals at the end of every cone build, so the numbers file quotes
the LAST report line of the process: the process total. That is the number a later
step is compared on (digest's total today sums its three cone builds; after step 2 it
sums two, and the difference is the deleted context's cone). The counters are never
reset (epic §5.9 rule 4, as amended: a reset would make the step 0 and step 2 lines
measure different things). Both peaks per command;
the census buckets; the §4.1.6 numbers; the existing ordering counters
(`ordering.roots_all`, `ordering.roots_kept`, `digest.rs:4060-4062`) on a d47 run.
Each later step's numbers file has the same layout, so a diff of two files is the
comparison.

### 4.2 Step 1 — the frozen `InputSet`

#### 4.2.1 Types (`src/program/inputs.rs`)

```rust
pub struct InputSet {
    /// The workspace root as the command line spelled it (never canonicalized:
    /// `ws:` unit ids and messages use it as given).
    pub workspace: PathBuf,
    /// `<workspace>/app.json`, the raw bytes: `Err(io)` when it cannot be read.
    pub app_json: FileInput,
    /// The parse of `app_json.text()`: `None` when the bytes are `Err` OR not
    /// UTF-8 (a read error, see `FileInput::text`); `Some(Err(serde message))` when
    /// the file is readable UTF-8 but not JSON. Readers tell the two apart
    /// (§4.2.3): the build's `read <path>: <io>` / `parse <path>: <serde>`
    /// (`src/snapshot/snapshot.rs:193-197`), `derive_inputs` hashing the bytes of a
    /// malformed file (`snapshot_full.rs:819-827`), the diagnostics treating a
    /// malformed file as unreadable (`workspace_diagnostics.rs:145-151`).
    pub app_json_value: Option<Result<serde_json::Value, String>>,
    /// Every `app.json` under the root except under `node_modules`/`.alpackages`
    /// (`source_text::count_app_json_paths`, `src/source_text.rs:145`), sorted.
    pub app_json_paths: Vec<PathBuf>,
    /// `<workspace>/roots.config.json`: `None` = no file; `Some(Err)` = unreadable.
    pub roots_config: Option<Result<Vec<u8>, String>>,
    /// `<workspace>/al-sem.coverage.yaml` (a fingerprint input only).
    pub coverage_yaml: Option<Vec<u8>>,
    /// The policy file the command asked for (`--policy`, or the auto-detected
    /// `al-sem.policy.yaml` / `.yml`, `src/engine/gate/policy/pipeline.rs:72`);
    /// `None` when the command takes none. Read here, VALIDATED where it is today.
    pub policy_file: Option<FileInput>,
    /// `--baseline <path>` (analyze): `None` = no flag; `Some(Err(NotFound))` is
    /// the "no baseline yet" case `load_baseline` treats as empty
    /// (`src/engine/gate/baseline.rs:40-43`).
    pub baseline: Option<FileInput>,
    /// The dependency closure the loader reads: `discover_app_files`'s two lists
    /// (`src/dependencies.rs:411-413`): the `.app` files, and the `.alpackages`
    /// folders whose listing failed. `load_all_apps` takes this instead of
    /// discovering again; `unreadable` feeds `reduce_fresh_coverage` as
    /// `load.unreadable` does today (`full.rs:2150`).
    pub closure: ClosureEntries,
    /// `derive_inputs()`'s rows (`src/engine/l5/snapshot_full.rs:815-887`), present
    /// only when `InputNeeds::fingerprint_rows` was asked for (hashing every `.app`
    /// is work analyze never did; `fingerprint`, `digest`, `prove`, `diff` did).
    pub snapshot_inputs: Option<Vec<SnapshotInput>>,
    /// The `.al` walk itself (`discover_al_files`, `source_text.rs:66-103`):
    /// `Err(os message)` on an unreadable folder or a link loop (`:63-65`, `:85`).
    /// Then `al_files` is EMPTY and every reader fails closed as today: the build
    /// `Err`s with `walking source <root>: <os>` (`provider.rs:34-35`),
    /// `model_instance_id` is `None` (`model_instance_id.rs:83`, `.ok()?`), the
    /// diagnostics return the provider rows alone (`workspace_diagnostics.rs:93-95`).
    /// A partial list must never reach the build as if it were the whole app.
    pub al_walk: Result<(), String>,
    /// Every `.al` file of the `NestedApps::Walk` list, sorted by `rel_posix`.
    pub al_files: Vec<WsFile>,
    /// `driver_version()` read once (`src/engine/gate/version.rs:37-39`).
    pub driver_version: String,
    /// `compute_gate_model_instance_id`'s value (`src/engine/gate/model_instance_id.rs:75-96`),
    /// computed from the fields above with the same algorithm; `None` = fail-closed.
    pub model_instance_id: Option<String>,
}
pub struct FileInput { pub path: PathBuf, pub bytes: Result<Vec<u8>, std::io::Error> }
impl FileInput {
    /// The bytes as UTF-8. Invalid UTF-8 is an `io::Error` of kind `InvalidData`
    /// with the text `stream did not contain valid UTF-8` — the error
    /// `std::fs::read_to_string` returns, byte for byte (std's `INVALID_UTF8`
    /// const). Every reader that used `read_to_string` today (`app.json`,
    /// `snapshot.rs:194`; the baseline, `baseline.rs:38`) reads through this, so
    /// `read <path>: stream did not contain valid UTF-8` and `cannot read baseline
    /// '<path>': stream did not contain valid UTF-8` keep their text and stay READ
    /// errors, never parse errors. The hash rows (`derive_inputs`) read `bytes`.
    pub fn text(&self) -> Result<&str, std::io::Error>;
}
pub struct ClosureEntries { pub files: Vec<DiscoveredAppFile>, pub unreadable: Vec<(PathBuf, std::io::Error)> }
pub struct WsFile {
    pub rel_posix: String, pub abs_path: PathBuf,
    /// Under a child directory that holds its own `app.json`: the file the
    /// `NestedApps::Skip` walk would not list (`source_text.rs:70-75`).
    pub nested_app: bool,
    /// `Err(os message)`: the slot stays so the diagnostics can warn from it.
    pub text: Result<Arc<str>, String>,
}
pub struct InputNeeds { pub fingerprint_rows: bool, pub policy: PolicyRequest, pub baseline: Option<PathBuf> }
pub enum PolicyRequest { None, AutoDetect, Path(PathBuf), Disabled }
impl InputSet {
    pub fn read(workspace: &Path, needs: &InputNeeds) -> InputSet;   // the ONE disk reader; charges counter `inputs.read` (§4.1.2)
    pub fn file(&self, rel_posix: &str) -> Option<&WsFile>;          // index built in `read`
    pub fn app_scoped_files(&self) -> impl Iterator<Item = &WsFile>; // `!nested_app`: today's `NestedApps::Skip` list
    pub fn app_id(&self) -> Option<&str>;                            // `app_json_value` `Some(Ok)` with a non-empty string `id`, verbatim
    pub fn roots_config_loaded(&self) -> bool;                       // validates the bytes, as `roots_config_was_loaded`
}
```

`read` never fails: every read error is stored in its slot, the walk's in `al_walk`.
The build decides what a missing or unreadable input means, exactly as today.

#### 4.2.2 Reading

`InputSet::read` does, in this order: `app.json` (one read), the `app.json` walk, the
`.al` walk with `NestedApps::Walk` (one walk; `nested_app` is set per file by checking
each ancestor directory below the root with `dir_has_app_json`, cached per directory,
`source_text.rs:106-113`), one `read_al_source` per file (`:122-124`; the same lossy
decode, BOM dropped), `roots.config.json`, `al-sem.coverage.yaml`, the policy and
baseline files the command asked for, `discover_app_files` (`src/dependencies.rs:411`)
for the closure, and — when `fingerprint_rows` is asked — one `hash_file` per closure
entry (`snapshot_full.rs:907-910`). `driver_version()` once. The model-instance id from
`app_id()`, the `version` field and `al_files` (the `ws:<rel_posix>` list, sorted),
the same length-prefixed digest (`model_instance_id.rs:36-45`, `:79-95`).

The `.alpackages` `.app` files themselves are still opened by the loader
(`src/snapshot/snapshot.rs:182-260` via `dependencies::load_all_apps`), as today: the
loader reads a `.app` through `app_package` with a stamp taken before the read. A
second read of a `.app` for hashing is what `derive_inputs` does today on the commands
that need it; `InputSet` does it at most once instead of per `derive_inputs` call
(`snapshot_full.rs:63`, `:74`: two calls on a fingerprint run).

Nothing here special-cases a folder name (epic I11).

#### 4.2.3 Readers rewired

Every row is a disk read that happens after the semantic build today, or a read the
build itself makes of a workspace file. Each becomes a read of `&InputSet`.

| Today | File:line | After |
|---|---|---|
| `SnapshotBuilder::build_inner` reads and parses `app.json`; `WorkspaceProvider` walks and reads every `.al` | `src/snapshot/snapshot.rs:193-197`, `:236-239`; `src/snapshot/provider.rs:34-56` | `SnapshotBuilder { inputs: Arc<InputSet>, local_providers }`; `app_json.text()` `Err` (unreadable, or not UTF-8) fails with `read <path>: <io>`, `app_json_value` `Some(Err)` with `parse <path>: <serde>` (today's two texts, with today's split: invalid UTF-8 is a read error, `FileInput::text`); `al_walk` `Err` fails with `workspace source provider: walking source <root>: <os>`; the workspace `SourceRoot.files` are built from `al_files` with the SAME `Arc<str>` (no copy); a `WsFile.text` `Err` fails the build with today's message (`workspace source provider: reading source <abs>: <os>`), so an unreadable file still fails the program build |
| `compute_gate_model_instance_id(ws)` walks and reads | `model_instance_id.rs:75-96`; every command's first call | `inputs.model_instance_id`: `None` when `app_id()` is `None` or `al_walk` is `Err` (`:76`, `:83`) |
| `finish_resolved` reads `roots.config.json` and `app.json` | `src/program/model/workspace.rs:1767` (→ `root_classification.rs:450-463`), `:1775` (→ `:1789-1800`) | `compute_root_classifications(&ws, inputs.roots_config.as_ref())` validates from the bytes; `primary_app` from `inputs.app_json` |
| `compute_workspace_diagnostics(ws)` reads `app.json`, counts `app.json` files, walks, reads and PARSES every `.al` | `workspace_diagnostics.rs:44-47`, `:93-106`, `:119-129` | `compute_workspace_diagnostics(inputs, files_without_object: Option<&[String]>)`: the provider rows from `app_json_paths` / `app_id()` (readable = `app_json_value` is `Some(Ok)`, `:145-151`) / each `WsFile.text` `Err` (same texts, same order); `al_walk` `Err` returns after the provider rows (`:93-95`); the index rows from the bit (§4.2.4) |
| `project_coverage_disk(ws)` walks with `NestedApps::Skip` | `src/program/model/coverage.rs:298-323`; `run.rs:432` | `project_coverage_inputs(inputs)`: `app_scoped_files()` in the same sorted order |
| `select_program_files(ws, ctx)` reads the root `app.json` id and walks with `NestedApps::Skip` DURING model assembly, after the program build; a `.al` file added between the build's walk and this one is `ProgramFiles::Missing` and the model fails closed (`"program parse lacks"`) | `src/program/model/workspace.rs:1636-1661` (`:1643` `read_root_app_guid`, `:1644` `discover_al_files_app_scoped`, `:1657` `Missing`); called at `:1671` from `assemble_l3_workspace_from_program`, which `assemble_and_resolve_workspace_from_program` reaches | `select_program_files(inputs, ctx)`: `inputs.app_id()` and `inputs.app_scoped_files()`. The build parsed exactly that list (same `InputSet`), so `Missing` can no longer come from a disk change; the arm stays as the fail-closed guard against a parse that dropped a file. `inputs::al_file_added_after_read` (§8.2) pins it |
| `build_analysis_model`'s single-app arm walks with `NestedApps::Skip` to tell `ProgramBuildFailed` from `AssemblyFailed` | `run.rs:312-313` | `inputs.app_scoped_files().next().is_some()`: the same rule, no walk. (The cross-app arm's rerun of this walk goes with the rerun, §4.4.5) |
| `build_suppression_map(ws_path, unit_ids)` reads each `.al` again | `src/engine/gate/inline_suppression.rs:197-209` (→ `:37-38`); `run.rs:615` | parses suppressions from `inputs.file(rel).text`; a slot with `Err` yields no suppressions (today: a read error yields none) |
| `read_workspace_apps(ws)` reads `app.json` for pr-summary | `run.rs:122-157`; `:648` | from `inputs.app_json` with the same defaults |
| `derive_inputs(ws)` hashes `app.json`, every closure `.app`, `roots.config.json`, the coverage yaml | `snapshot_full.rs:815-887`; `:63`, `:74` | `inputs.snapshot_inputs.as_ref().expect("fingerprint rows were not requested")`: the command declares the need (`fingerprint`, `digest`, `prove`, `diff`). The `app-json` row hashes `app_json.bytes` whenever they are `Ok`, parse or no parse (`:819-827` hashes the file, never the JSON) |
| `roots_config_was_loaded(ws)` re-reads and validates | `root_classification.rs:443-445`; `fingerprint_cli.rs:359`, `snapshot_full.rs:858` | `inputs.roots_config_loaded()` |
| `resolve_policy_check(ws, path, no_policy)` reads the policy file | `pipeline.rs:47-80`; `:230` | takes `inputs.policy_file`; validation stays at `:230`, after the `--strict` check (`:219-226`), at step 1 AND at step 2 (the RAW build that needs the policy's presence is built after the run, where `:261` builds it today, §4.4.4), so no precedence changes. The reporting-order move is Decision 7, step 12 |
| `load_baseline(path)` reads after the run | `baseline.rs:37-60`; `run.rs:598` | without `--update-baseline`: `load_baseline_bytes(&inputs.baseline)` reads `FileInput::text()`, so a non-UTF-8 file is still `cannot read baseline '<path>': <io>` (`:44`), not `is not valid JSON` (`:47`); the NotFound → empty rule kept; validation stays at `:598` (a malformed baseline is still "analysis failure", exit 2). With `--update-baseline`: the set applied is the set `save_baseline` just wrote (`:584-591`, the current fingerprints), so every current finding is dropped, as today; `inputs.baseline` is NOT read or validated (today the old file is overwritten before the read, so a malformed old file never errors). A1.6 pins stdout and the exit code |
| `driver_version()` reads the env per call | `version.rs:37-39`; each `run_*_cmd` in `alsem.rs`; `impact_graph/mod.rs:237` | `inputs.driver_version` |
| `empty_output_result` reads workspace diagnostics from disk | `run.rs:787` | from `inputs` (§4.2.4) |

Not rewired, by design: `digest --diff <path>` / stdin (`alsem.rs:1062-1079`), `diff`'s
snapshot files and `--renames` — request inputs read before the build today, not
workspace inputs. `aldump`'s identity snapshot (`src/engine/snapshot.rs:282`) and the
LSP's `dependencies::resolve_all` (`src/dependencies.rs:790`) are other binaries' paths.

#### 4.2.4 The `has_object` bit

The index diagnostic "No object declaration found in <rel>" needs to know which
workspace files parsed to zero objects. Today that is a second full parse
(`workspace_diagnostics.rs:119-129`). The program build already parsed every file:
`ParsedFile.file.objects` (`parse.rs:17-29`).

- `FreshCoverage` (`full.rs:1892-1946`) gains `files_without_object: Vec<String>`
  (`rel_posix`, sorted): computed in `reduce_fresh_coverage` (`:2285`) from
  `ctx.parsed()` (`:1545-1547`), the workspace unit. This is the bit the epic calls
  `has_object`; it lives in `FreshCoverage` from step 1 on, and `ProgramMeta` carries
  `FreshCoverage` from step 1b on.
- `Model` (`workspace.rs:1890-1914`) gains `workspace_diagnostics: Vec<Diagnostic>`,
  built in `attach_program_calls_with` (`program_calls.rs:677-685`) from
  `ctx.snapshot().inputs` and `coverage.files_without_object`, next to
  `dependency_diagnostics` (the #91 precedent). `analyzer_diagnostics_of_run`
  (`run.rs:1085-1117`) reads it as source (1) and loses its `ws_path` parameter.
- `empty_output_result` (`run.rs:776-898`) calls `compute_workspace_diagnostics(inputs,
  fresh.as_ref().ok().map(|f| f.files_without_object.as_slice()))`. With `fresh` `Err`
  the build parsed nothing and the function emits NO index rows: there is no
  fallback parse. The corner where that moves a byte is stated here and in §7.
  `fresh` is `Err` on a missing, unreadable or malformed `app.json`
  (`snapshot.rs:193-197`), an unreadable `.al` (`provider.rs:38-39`) or a failed walk
  (`:34-35`). Today's function parses on none of the first group (it returns at the
  provider rows, `workspace_diagnostics.rs:49-87`, whenever the id is `None`) and not
  on a failed walk (`:93-95`). It parses on an unreadable `.al` only when
  `empty_output_result` runs at all, i.e. the model is `AssemblyFailed` (`run.rs:308-326`,
  `:491`: a `ProgramBuildFailed` returns the exit-2 line at `:494-498` with no
  diagnostics). `AssemblyFailed` with the id present means the `NestedApps::Skip` list
  is empty. A nested app (a second `app.json` under the root) does not reach it either:
  `count_app_json_paths` (`source_text.rs:145-169`) sees both files, `is_multi_app`
  is true and the function returns at `:86`. The one input left is a nested app
  reached through a DIRECTORY SYMLINK: the walk follows links (`follow_links(true)`,
  `:67-68`) and lists its files, but `count_app_json_paths` reads `entry.file_type()`
  off `read_dir` (`:153-156`), which does not follow a link, so it never descends
  and counts one `app.json`. With no `.al` file at the root and an unreadable file
  inside the link, today prints `No object declaration found in <rel>` for the
  link's readable objectless files; after step 1 it prints the provider rows alone
  (the `Could not read` warning included). That is the whole byte change: a
  symlinked nested app, an unreadable file inside it, and no root `.al` file, in
  either universe (cross-app reruns the single-app build today, `run.rs:387`). No
  fixture has the shape and no golden carries it.

The test `analyzer_diagnostics_of_run_keeps_every_source_in_order` (`run.rs:1129`)
constructs its `Model` by hand; it sets `workspace_diagnostics` literally (a hand-stated
precondition, CLAUDE.md).

#### 4.2.5 One stdout writer

`src/bin/alsem.rs` gains three functions and every command uses them:

```rust
fn write_stdout(bytes: &[u8], fail: ExitCode) -> Result<(), ExitCode>;      // stderr `al-sem: write error: stdout: <OS message>`, exit `fail`
fn write_file(path: &str, bytes: &[u8], fail: ExitCode) -> Result<(), ExitCode>; // stderr `al-sem: write error: <path>: <OS message>`, exit `fail`
fn write_stderr(bytes: &[u8], fail: ExitCode) -> Result<(), ExitCode>;      // no message (the stream is gone), exit `fail`
```

**One text, one rule, the command's own failure code.** `fail` is `ANALYSIS_FAILURE`
(2) for analyze and 1 for every other command — the code each of those commands
already exits with on a write failure and on every other failure today. Exit 2 is NOT
free on them: digest exits 2 for "no changed roots" (`digest_cli.rs:1652`) and prove for
"routine not resolved" (`prove.rs:950`), so a uniform 2 would make a lost report
indistinguishable from a legitimate result. The epic's Decision 49 and §5.10 are
amended to this rule (review log, row 4); the epic's "two sites exit 1" was a
miscount — the grep below finds eleven.

Sites replaced, with today's behaviour: analyze `println!("{out}")` (`alsem.rs:1616`;
under `panic = "abort"`, `Cargo.toml:268`, a closed pipe aborts the process); graph
(`:719-734`, exit 1); query (`:740-759`, exit 1); policy check (`:860-875`, `failed to
write: <e>`, exit 1); policy explain (`:907-912`, `failed to write`, no message, exit 1);
diff (`:985-994`, exit 1); digest (`:1122-1134`, exit 1); prove (`:1192-1202`, exit 1);
fingerprint (`write_fingerprint_output`, `src/engine/l5/fingerprint_cli.rs:558`, via
`:1330-1337`, exit 1); events fanout and chains (`:1403-1413` and the chains twin, exit
1); cache prune (`:1495-1498`, no message, exit 1). The trailing newline analyze's
`println!` adds is kept by appending it to the bytes. Stderr lines that follow the
output today (policy `:878-880`, diff, fingerprint `:1340-1342`, events) keep following
it and go through `write_stderr`; a library `eprintln!` is not rerouted (§5). The
exit-code table gains the row "write failure → 2" for analyze only
(`src/engine/gate/exit_code.rs:3-9`); no other command's exit code moves.

#### 4.2.6 Signatures that change

`InputSet::read` is called once per command in `alsem.rs`, before the pipeline, and
passed down: `run_analyze_with_exit(inputs: &InputSet, args)`; `run_digest_pipeline(inputs, …)`;
`run_prove_pipeline(inputs, …)`; `run_fingerprint_pipeline` (through `FingerprintOptions`);
`run_diff`'s `load_snapshot_from_workspace` (`src/engine/gate/diff/cli.rs:88-118`, one
`InputSet` per workspace side); `run_events_fanout`/`run_events_chains` (through their
options); `run_policy_check`; the two `QuerySubstrate::build` callers
(`effect_query_cli.rs:735-738`, `:1106-1109`); `impact_graph::export_workspace(inputs)`.
`AnalysisTarget::build(inputs, single_app, command)` (`run.rs:191-215`) and
`build_analysis_model(inputs, single_app)` (`:281`) take it instead of a path. The
model builders `assemble_and_resolve_workspace_program` (`program_calls.rs:713-728`),
`assemble_and_resolve_cross_app_program` (`:816-908`) and the program builders
`build_program_with_coverage[_profiled]` / `build_cross_app_program` (`full.rs:2123-2180`)
take `&Arc<InputSet>`; `SnapshotBuilder` keeps a `from_disk(root)` constructor that
calls `InputSet::read` with `InputNeeds::default()` for the LSP and `aldump` paths
(`src/lsp/snapshot.rs:599-604`).

`switch_dump.rs:437-451` greps `build_analysis_model`'s source body for exactly one
`assemble_and_resolve_workspace_from_program(` call: the function keeps its name and
that shape.

The in-process golden harnesses (`tests/cli/cli_b_*`, `cli_a_*`, `cli_c_*`, every file
the grep in §8.2 lists) call the pipeline functions; each gains one `InputSet::read`
line. No golden moves.

### 4.3 Step 1b — `ResolvedProgram`

#### 4.3.1 Types (`src/program/resolved.rs`)

```rust
pub struct ResolvedProgram { pub meta: ProgramMeta, pub body: Option<ProgramBody> }
pub struct ProgramMeta {
    pub inputs: Arc<InputSet>,
    pub profile: BuildProfile,
    /// `AppSetSnapshot` minus its source texts: ids, provenance, tiers, `app_path`,
    /// `declared_deps`, `internals_visible_to`, compilation context, ABI presence.
    pub apps: Vec<AppMeta>, pub workspace_app: AppId,
    pub primary_app_ref: AppRef,
    pub recovered_files: Vec<String>,
    /// Carries `files_without_object` (§4.2.4).
    pub coverage: FreshCoverage,
    pub histogram: Histogram, pub primary_histogram: Histogram,   // over `edges` only (`full.rs:1403-1418`)
    pub abi_integrity: AbiIntegrity, pub coverage_contract: Coverage, // `report.coverage` (`full.rs:262-263`)
    pub loaded_apps: Vec<serde_json::Value>,                       // `full.rs:2190`
}
pub struct ProgramBody {
    pub ctx: ProgramContext,            // graph, parsed units, dependency trees (per profile)
    /// `report.edges` as `resolve_full_program_with` returns them (`full.rs:1434-1445`):
    /// every workspace routine's edges (units filtered to the primary app, `:1231`)
    /// PLUS the event-flow edges of publishers in every app (`resolver.rs:3293`).
    pub edges: Vec<ClassifiedEdge>,
    /// `resolve_dependency_bodies().edges` (`full.rs:1629-1672`): every dependency
    /// file's call-site edges. Empty under `DependencyBodies::Summary`.
    pub dependency_body_edges: Vec<ClassifiedEdge>,
    pub site_facts: HashMap<ObligationId, SiteFacts>, pub parenless_calls: ParenlessCalls,  // both already merged (`:2177-2178`)
    pub builtin_dispatch_audit: …, pub event_flow_dual_publisher_alias_skips: usize,
}
impl ProgramBody {
    /// Today's merged order (`full.rs:2176`): `edges`, then `dependency_body_edges`.
    pub fn all_edges(&self) -> impl Iterator<Item = &ClassifiedEdge> + Clone;
}
impl ResolvedProgram {
    pub fn build(inputs: &Arc<InputSet>, profile: BuildProfile) -> Result<ResolvedProgram, String>;
}
```

`ResolvedProgram::build` is `build_program_with_coverage_profiled` (`full.rs:2136-2152`)
plus, under `BuildProfile::FULL`, `resolve_dependency_bodies` (`:2172-2178`), with the
two edge sets kept apart. `build_cross_app_program` (`:2165-2180`) becomes
`ResolvedProgram::build(inputs, FULL)`; `build_program_with_coverage` (`:2123-2132`)
becomes `build(inputs, Summary)`. `ProgramReport` stays as the internal shape the
resolver returns; `ProgramBody` is its retained form.

#### 4.3.2 The two edge sets (R6-5)

The epic's earlier justification "no caller sits in both sets" was false. What is true:

- `edges` holds the primary app's routines' edges (`resolve_full_program_from_parts`
  filters units to `primary_app_ref`, `full.rs:1231`) and the event-flow edges of every
  publisher in every app (`emit_event_flow_edges` loops over `graph.routines`,
  `src/program/resolve/resolver.rs:3293`).
- `dependency_body_edges` holds one edge per dependency call site (`full.rs:1677-1684`).
- So a dependency publisher appears in `edges` (its event-flow edge) and in
  `dependency_body_edges` (its call sites). The two are disjoint as edges, not as
  callers.
- The `graph` projection merges on `(from, to, kind)` and on a `conf` tie keeps the
  first edge's `via` (`src/program/impact_graph/project.rs:277-294`). The order `edges`
  then `dependency_body_edges` is therefore a contract (Decision 55), and `all_edges`
  is the only way the projection and the adapter read them.
- The `dependencyBodies` histogram (`impact_graph/mod.rs:222-230`, `:259`) filters the
  merged list by `from.app != primary && CallSite`. Over `all_edges` that is the same set
  as over `dependency_body_edges` alone: no `CallSite` edge in `edges` has a dependency
  caller, and no event-flow edge is a `CallSite`. The projection keeps today's filter
  over `all_edges`; A1b.1 pins the bytes.
- The histograms describe `edges` only (`full.rs:1403-1418` run before `:2176`), as today.

#### 4.3.3 Builder and consumers

- `impact_graph::export_workspace(inputs)` → `export_from_program(&ResolvedProgram)`;
  it reads `body.ctx` (`graph()`, `all_units()`, `decl_surface()`, `snapshot()`),
  `body.all_edges()`, `body.site_facts`, `meta.coverage_contract.missing`,
  `meta.recovered_files`, `meta.primary_app_ref`, `meta.loaded_apps`, the histograms
  and `inputs.driver_version`. The id check (`mod.rs:199-204`) reads `inputs.app_id()`.
  `alsem graph` builds `ResolvedProgram::build(inputs, FULL)` and exports; the
  private build at `mod.rs:192-193` is gone.
- `assemble_and_resolve_cross_app_program` (`program_calls.rs:816-908`) takes a
  `&mut ResolvedProgram`: demand (`:838-845`) reads `body.ctx.graph()` and
  `body.all_edges()`; assembly (`:846-856`) reads `body.ctx` and `body.parenless_calls`;
  `loaded_apps` (`:895`) is `meta.loaded_apps`; `attach_program_calls_with` takes
  `program.body.take().expect("built once")` and `&program.meta.coverage`.
- `assemble_and_resolve_workspace_program` (`:713-728`) and `build_analysis_model`'s
  single-app arm (`run.rs:285-357`) the same way, with `Summary`.
- The adapter (`program_calls.rs:922-970`) reads `body.all_edges()` where it read
  `report.edges` (through `SiteLinks::build`, `:429`) and `body.site_facts`.
- `AnalysisModel.fresh` (`run.rs:251-261`) stays a `Result<FreshCoverage, String>`; it
  is `program.meta.coverage.clone()` on success.

#### 4.3.4 Release point and sizes (G-6)

`attach_program_calls_with` drops the body at `program_calls.rs:669-676` (`b3.report_drop`,
`b3.ctx_drop`) — unchanged, except that `ProgramBody` is what is dropped. `ProgramMeta`
lives as long as its holder (the `AnalysisModel` / `AnalysisTarget` / the `View` at
step 2). Its size is NOT "a few KB": `inputs.al_files[].text` is the whole workspace
source, 5.09 MB on CDO (582 files), and grows with the workspace. On CDO this is inside
the 85 MB band; the census prints it (A1b.5) so a larger workspace is a number, not a
surprise. Everything else in `ProgramMeta` is small (histograms, ledger rows, app ids).

The parsed IR, the graph and the dependency trees are NOT retained; `graph` runs before
the body is released (rule 3 item 1).

### 4.4 Step 2 — `DetectorArtifacts`, `L4Analysis`, `View`

#### 4.4.1 `L4Analysis` (`src/engine/l4/analysis.rs`)

```rust
pub struct L4Analysis {                       // what outlives `det` in digest and prove: small
    /// The ONE SCC owner. Tarjan over `graph.edges_by_from` (`scc.rs:39-46`): single-app
    /// `build_l4` runs it (counter `build.graph_scc` at that call); cross-app it is the
    /// base's `Arc` (`R3a5CrossAppBase.combined_scc: Arc<SccResult>` from step 2, the
    /// r3a5 projections read through the `Arc`), cloned by pointer, never by value.
    pub scc: Arc<SccResult>,
    pub field_index: FieldIndex,
    pub bundle: SummaryBundle,                // the compact store (`effect_store.rs:709`)
    pub summarize_diagnostics: Vec<SummarizeDiagnostic>,
    /// The non-convergence line(s) the solver used to print (§4.4.7).
    pub warnings: Vec<String>,
}
pub struct L4Harvest {                        // the per-routine harvest (`:1454-1495`); moved INTO `det`
    pub uncertainties: UncertaintyIndex,
    pub uncertainties_by_node: HashMap<String, UncertaintySetId>,
    pub parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>>,
}
pub fn build_l4(model: &Model, graph: &CombinedGraph, scc: Option<Arc<SccResult>>,
                cross: Option<&CrossAppInputs<'_>>) -> (L4Analysis, L4Harvest);
```

`build_l4` is today's inline block (`detector_context.rs:1296-1511`) lifted out: the
SCC (`:1297-1307`, run when `scc` is `None`; a cross-app caller passes
`Arc::clone(&base.combined_scc)`, which is the same Tarjan over the same
`edges_by_from`, `capability_cone.rs:3473-3480`; no third copy exists, the `View`
holds no `scc` of its own), the field index
(`:1365-1372`), `compute_summaries_v2_bundle_with_leaves` (`:1387-1396`) with the leaves
and fixed roles from `cross`, and the harvest (`:1454-1495`). The harvest is NOT part of
`L4Analysis`: the uncertainty union alone is up to ~730 MiB on 8020 (`:1440`), and
today's digest and prove hold only the bundle during the query phase
(`effect_query_cli.rs:219` returns `.0`; `prove.rs:964` discards graph and scc). The
`View` holds `L4Harvest` only between the `l4` fill and the `det` fill, which takes it
by value; a plan with no `det` run (query) drops it at once. It replaces
`cross_app_db_effect_bundle` (`effect_query_cli.rs:207-220`) and `build_db_effect_bundle`
(`:226-261`): both build the same inputs (same graph, same SCC, same field index, same
upgraded bindings, same leaves), so the bundle is the same object. `QuerySubstrate`
(`:167-193`) becomes a `View` with an `l4` cell; `DbEffectQuery::build(&l4.bundle,
&l4.scc, &graph)` (`:196-198`) is unchanged (`&Arc<SccResult>` derefs to the
`&SccResult` it takes). The two deleted builders both discard the solver's diagnostics
(`:219` `.0`, `:251` `_diags`); today query's non-convergence line is the solver's
own `eprintln!`, after step 2 it is `l4.warnings` (§4.4.7).

The solver reads `edges_by_from` and `uncertainty_edges`, never `typed_edges`
(`summary_runner.rs`, `db_effect_solver.rs`: no reader), so a graph with the injected
dependency typed edges folded in gives the same bundle as one without.

#### 4.4.2 `DetectorArtifacts` and the borrowed `DetectorContext<'v>`

`DetectorContext<'a>` (`detector_context.rs:607-803`) splits by what it owns:

```rust
pub struct DetectorArtifacts {                 // owned; one per View, at the union of the plan's masks
    pub demanded: u32,
    pub graph: Arc<CombinedGraph>,             // the view's graph cell (§4.4.3); `rel.combined` at step 4
    pub event_graph: EventGraph,
    pub reverse_call_graph: ReverseCallGraph, pub entry_points: BTreeSet<String>,
    pub transaction_spans: Vec<TransactionSpan>,
    pub resolved_call_edge_by_callsite: HashMap<String, CallEdge>,
    pub uncertainty_edges_by_from: HashMap<String, Vec<Uncertainty>>,
    pub uncertainties: UncertaintyIndex,                                   // from `L4Harvest`, by value
    pub uncertainties_by_node: HashMap<String, UncertaintySetId>,          // from `L4Harvest`
    pub parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>>, // from `L4Harvest`
    pub summaries: HashMap<String, FullRoutineSummary>, pub cone_derived: ConeDerivedStore,
    pub event_flow_indexes: EventFlowIndexes, pub upgraded_bindings_by_callsite: …,
    pub reachable_roots: BTreeSet<String>, pub internal_reachable_externally: bool,
    pub root_classifications_by_routine: …, pub dep_routine_ids: BTreeSet<String>,
    pub declared_dependencies: Vec<DeclaredDep>, pub app_versions: …, pub friends: …,
    pub closed_world_temp_params: ClosedWorldTempParams,
    pub summarize_diagnostics: Vec<SummarizeDiagnostic>,   // from `l4`, or empty
    pub reverse_effect_index: Option<ReverseEffectIndex>,
    pub cross_extension_subscribers: BTreeMap<String, Vec<String>>,
}
pub struct DetectorContext<'v> {               // borrowed; built once per command, cheap
    pub art: &'v DetectorArtifacts,
    pub ordering: Option<&'v HashMap<String, OrderingFacts>>,   // the view's ordering cell
    pub routine_by_id: HashMap<&'v str, &'v ModelRoutine>, pub objects_by_id: …,
    pub table_by_id: …, pub call_site_by_id: …,
    pub fingerprint_index: FingerprintIndex<'v>,
}
impl<'v> std::ops::Deref for DetectorContext<'v> { type Target = DetectorArtifacts; fn deref(&self) -> &DetectorArtifacts { self.art } }
pub fn build_detector_artifacts(model: &Model, demanded: u32, graph: Arc<CombinedGraph>,
                                l4: Option<(&L4Analysis, L4Harvest)>, cross: Option<CrossAppInputs<'_>>) -> DetectorArtifacts;
impl<'v> DetectorContext<'v> {
    /// The borrowed maps over the rows given (`routine_by_id`, `call_site_by_id`,
    /// `objects_by_id`, `table_by_id` via `table_by_id_preferring_real`, the
    /// fingerprint index over `routines`): today's `:1164-1172`, `:1529-1534`,
    /// `:1557-1558`. Takes slices, not a `Model`, so a test with routines alone
    /// can build one.
    pub fn new(routines: &'v [ModelRoutine], objects: &'v [ModelObject], tables: &'v [ModelTable],
               art: &'v DetectorArtifacts, ordering: Option<&'v HashMap<String, OrderingFacts>>) -> Self;
}
pub fn detector_context<'v>(model: &'v Model, art: &'v DetectorArtifacts,
                            ordering: Option<&'v HashMap<String, OrderingFacts>>) -> DetectorContext<'v>;
                            // = `DetectorContext::new(&ws.routines, &ws.objects, &ws.tables, art, ordering)`
/// Test-only owner for the shims below: a `DetectorContext` borrows its artifacts, so
/// a function that builds them cannot also return the context (no owner, no
/// self-borrow). Call sites do `let owned = build_detector_context(..); let ctx = owned.ctx();`.
pub struct OwnedContext<'m> { model: &'m Model, art: DetectorArtifacts, ordering: Option<HashMap<String, OrderingFacts>> }
impl<'m> OwnedContext<'m> { pub fn ctx(&self) -> DetectorContext<'_>; }
/// `test_support::minimal_ctx` (`src/engine/l5/test_support.rs:367-424`, 63 call sites
/// in 8 files: the d1 substrate tests, d35, d50) builds today's context as a struct
/// literal from a `&[ModelRoutine]`, an OWNED graph and owned summaries. After the
/// split it is the same problem as `OwnedContext` with a routine slice in place of a
/// `Model`: it returns this owner, and each call site gains the `.ctx()` line.
pub struct MinimalCtx<'a> { routines: &'a [ModelRoutine], art: DetectorArtifacts }
impl<'a> MinimalCtx<'a> { pub fn ctx(&self) -> DetectorContext<'_>; }   // `DetectorContext::new(self.routines, &[], &[], &self.art, None)`
pub fn minimal_ctx<'a>(routines: &'a [ModelRoutine], graph_edges: HashMap<String, Vec<CombinedEdge>>,
                       summaries: HashMap<String, FullRoutineSummary>) -> MinimalCtx<'a>;   // `art.graph` = `Arc::new(graph)`
```

- `Deref` keeps every `ctx.<field>` read in the 54 detectors compiling unchanged: the
  owned fields keep their names AND types on `DetectorArtifacts` (the harvest moves in
  by value, so no field becomes an `Arc` and no reader changes), the borrowed maps keep
  theirs on `DetectorContext` (epic open question 9, closed).
- `db_effect_bundle: Option<SummaryBundle>` (`:777`) leaves the context: no detector
  reads it (`grep ctx.db_effect_bundle` has no detector site); `reverse_effect_index`
  (`:1521-1526`) is built from `l4.bundle` when `DB_EFFECT_REVERSE_INDEX` is demanded.
- `ordering_facts: OnceLock<…>` (`:744-749`, `:1608-1609`) and the lazy
  `get_ordering_facts` (`:831-846`) are gone. `get_ordering_facts(&self)` returns
  `self.ordering.expect("ordering cell not filled: declare substrate::ORDERING_FACTS")`
  in every build. The fill moves from `run_each` (`registry.rs:613-621`) to
  `View::prepare` (§4.4.3). The registry test `every_detector_alone_declares_what_it_reads`
  (`registry.rs:998-1016`) keeps catching an undeclared reader: `run_each` turns the
  panic into the same `Detector "<name>" threw` diagnostic under `panic = "unwind"`
  (`:693-703`), and under `abort` an undeclared read is a crash in debug AND release,
  which is the stated outcome of rule 1 (a read asserts only "filled").
- `build_detector_artifacts` is `build_detector_context_with` (`:899-1617`) minus the
  graph build (`:938-946`: it takes the view's graph), minus the inline solve
  (`:1296-1511`: it takes `l4`), minus the borrowed maps (`:1164-1172`, `:1529-1534`,
  `:1557-1558`: they move to `detector_context`). `summarize_diagnostics` is
  `l4.summarize_diagnostics.clone()` when `demanded & CORE_SUMMARIES != 0` and `l4` is
  `Some`, else empty — the decision (a) rule of `registry.rs:40-44`, now per run (§4.4.3).
- `build_detector_context` / `build_detector_context_with` stay as test-only shims that
  return an `OwnedContext` (above): graph + Tarjan + `build_l4` when the mask has
  `CORE_SUMMARIES` + `build_detector_artifacts`, **plus the ordering fill
  (`compute_ordering_facts(model)`) whenever the mask has `ORDERING_FACTS`**. That last
  clause is what keeps the two registry tests meaning what they mean today: the fill
  leaves `run_each` (`registry.rs:613-621`), so `every_detector_alone_declares_what_it_reads`
  (`:1002`, a context at `detector.requires`) gets its ordering from the shim, and a
  detector that reads without declaring finds `None`, panics in `get_ordering_facts`
  with the `expect` text above (it contains `substrate::ORDERING_FACTS`, the substring
  `:1007` greps), and `run_each` turns that into the diagnostic the test counts. The 47
  call sites (17 files, `grep build_detector_context`) each become two lines; the three
  tests that write the context as a struct literal (`d17.rs:504`, `d50.rs:619`, `:784`)
  write a `DetectorArtifacts` literal and call `detector_context`; the 63
  `minimal_ctx` sites get `MinimalCtx` (above) (§7). No `alsem` command calls the
  shims; `aldump` and the tests reach them through `run_detectors` /
  `run_detectors_cross_app` (§4.4.3).

#### 4.4.3 `View`, `Plan`, `RunKey` (`src/engine/session.rs`)

```rust
pub struct View {
    pub target: AnalysisTarget,                 // `SingleApp(Model)` | `CrossApp(R3a5CrossAppBase)` (`run.rs:173-176`); `rel` at step 4
    graph: Option<Arc<CombinedGraph>>,          // one graph per view; cross-app: `Arc::clone(&base.graph)` (`:3470-3480`)
    l4: Option<L4Analysis>, harvest: Option<L4Harvest>,            // `harvest` lives only until the `det` fill takes it
    det: Option<DetectorArtifacts>,
    ordering: Option<HashMap<String, OrderingFacts>>,
    runs: BTreeMap<RunKey, Arc<RunOutput>>,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RunKey { pub detectors: Vec<String>, pub demanded: u32, pub scope_primary: bool }
pub struct Plan { pub runs: Vec<Vec<Detector>>, pub scope_primary: bool, pub need_l4: bool,
                  /// The substrate bits a post-run projection reads off `det` (epic rule 3 item 8):
                  /// digest and prove `TRANSACTION_SPANS` (`tx_context_map`), events `SUMMARIES`
                  /// (`compute_fanout_report`'s coverage read, `events.rs:429`). `prepare` asserts
                  /// `union & after_run == after_run` before the `det` fill, naming the missing bit;
                  /// the artifact a projection reads is then in the plan, not an accident of the
                  /// detector set (A2.10).
                  pub after_run: u32 }
pub enum Cell { Det, L4, Ordering, Graph }
impl View {
    pub fn new(target: AnalysisTarget) -> View;       // nothing else: no step-2 reader of `InputSet` or `ResolvedProgram` goes through the view
    pub fn prepare(&mut self, plan: &Plan);           // fills every `None` cell the plan needs, in order: graph, l4, det, ordering, runs
    pub fn release(&mut self, cell: Cell);
    pub fn model(&self) -> &Model;                    // `target.resolved()`
    pub fn graph(&self) -> &Arc<CombinedGraph>;
    pub fn l4(&self) -> &L4Analysis;  pub fn det(&self) -> &DetectorArtifacts;
    pub fn detector_context(&self) -> DetectorContext<'_>;
    pub fn run(&self, key: &RunKey) -> &RunOutput;    // the stored output; panics if `prepare` did not run it
}
```

`View::new` takes the target alone. The commands hold `&InputSet` (passed down from
`alsem.rs`, §4.2.6) and the program's `ProgramMeta` through `AnalysisModel.fresh`
(§4.3.3) as they do after step 1b; nothing at step 2 reads either through the view,
and `AnalysisTarget::build` returns no `ResolvedProgram` (the cross-app path builds
and consumes it inside `build_cross_app_base`, `capability_cone.rs:3455-3585`). The
epic's `View.program` joins at the first step that reads it from the view (epic §5.9,
as annotated). `Universe` is the target's variant, not a field.

Rules (epic §5.9), as code:

1. **Serial fills from built inputs.** `prepare` fills every cell the plan needs that
   is still `None` (a second `prepare` after a `release` refills that cell at the new
   plan's mask and keeps the others; nothing is refilled while it is filled), in
   order: `graph` (single-app: `build_combined_graph` over `model.calls`/`events`,
   today's `detector_context.rs:938-946`; cross-app: `Arc::clone(&base.graph)` — the
   base's `graph` becomes `Arc<CombinedGraph>` at step 2 and STAYS on the base, since
   the r3a5 projections (`capability_cone.rs:3613`) and `QuerySubstrate::from_cross_app_base`
   (`effect_query_cli.rs:171`, until §4.4.1 replaces it) read it there; §10 item 3),
   then — when `plan.need_l4 || any mask has CORE_SUMMARIES` — `l4`
   (`build_l4(model, &graph, scc, cross)` → `l4`, `harvest`; single-app `scc` is `None`
   and `build_l4` runs today's Tarjan `:1297-1307`, which today also runs only under
   `CORE_SUMMARIES` at `:1296`, so a `--single-app --detector <non-core>` run pays no
   extra Tarjan; cross-app `scc` is `Some(Arc::clone(&base.combined_scc))`; the
   `l4.warnings` print happens here, §4.4.7), then `det` at `union(masks)` — never
   with `RAW_INHERITED_FACTS` (epic I5, Decision 50; policy's RAW build is not a cell,
   §4.4.4) — (`build_detector_artifacts(model, mask, Arc::clone(&graph), l4.zip(harvest.take()), cross)`;
   `l4` is passed only when `mask & CORE_SUMMARIES != 0`, as `:1296` gates it today;
   `prepare` asserts `mask & plan.after_run == plan.after_run` first, naming the
   missing bit — for `TRANSACTION_SPANS` that is the bit that forces the summaries the
   tx map is built from, `registry.rs:53-56`), then `ordering` when
   any run demands `ORDERING_FACTS` (`compute_ordering_facts(model)` on
   `big_stack_pool().install`), then each run, stored in `runs`. Every builder takes
   `&T`; the one shared owner is the `Arc<CombinedGraph>` held by the view and `det`
   (its last holder is `det`, released before the view's own `Arc`).
2. **No fill from inside a detector.** The types already forbid it: a detector is
   `fn(&Model, &DetectorContext)` (`registry.rs:318`) and every fill is on
   `prepare(&mut self)`, so no detector can reach a fill while a context borrowed from
   the view is alive. One assertion remains, at `prepare`'s entry, release builds too:
   `!in_detector_loop()` (`registry.rs:564-566`), the guard against a future caller
   that builds a second `View` from inside `run_each`'s loop. No `IN_CELL_BUILD`
   thread-local: it would be set by `prepare` around the fill and asserted clear by
   the fill, which contradicts itself, and a rayon worker inside a fill would not see
   it anyway. A read asserts only "filled" (`expect`).
3. **A run's diagnostics come from its own mask.** Each run EXECUTES inside `prepare`
   (rule 1's last fill): `run_each(model, &ctx, detectors)` (`registry.rs:571-708`) and
   the role-scoping tail of `run_detectors` (`:420-438`) or `run_detectors_cross_app`
   (`:476-537`), stored under its `RunKey`. `run(&self, key)` only returns the stored
   output (no interior mutability, rule 4). A run's `summarize_diagnostics` are
   `l4.summarize_diagnostics` iff `key.demanded & CORE_SUMMARIES != 0` (rule 2,
   Decision 48's `RunKey`). The context that serves the run was built at a
   superset of `key.demanded`; §4.4.6 licenses that.
4. **No `OnceLock`, `Mutex` or lazy fill** in `session.rs` (A2.9's `--lib` test reads
   the file's own source for the two names).
5. **Census counters are never reset.** They stay process-cumulative, as today
   (§4.1.7: the numbers file quotes the process total, so step 0 and step 2 lines
   measure the same thing). The epic's §5.9 rule 4 ("reset at the start of each cell
   build") is amended to this (review log, D6).

`run_detectors` and `run_detectors_cross_app` (`registry.rs:384-538`) keep their
signatures (`&Model` / `&R3a5CrossAppBase`): they build an `OwnedContext` through the
shim (§4.4.2) and call the same `run_each` + tail the view's fill calls (one shared
`fn finish_run`, so the two paths cannot drift). Their callers are `aldump`
(`switch_dump.rs:126`, `finding.rs:952`/`:1049`, the r4 projections) and tests
(`registry.rs:845`, `:858`, `:1193`, `:1236`, `d1.rs:4352`, the integration suites);
every `alsem` command goes through `View` (`run.rs:508`/`:512`, `:1032`/`:1057` are
rewired). A throwaway `View` is not possible there: `View` owns its `AnalysisTarget`,
and those callers hold a `&Model`.

#### 4.4.4 Per-command order

The fixed order of epic rule 3, as it applies before steps 3–6 exist. `det ⊥ snapshot`
means the two are never alive together, which is today's state for digest and prove
(`digest_cli.rs:1646-1650` drops the snapshot before the diagnostics run; the tx map's
context and the snapshot ARE alive together today, `:1580`–`:1650`, so step 2 holds
less at that point, not more).

| Command | Order on the `View` | Deleted |
|---|---|---|
| analyze | `prepare(Plan { runs: [selected], scope_primary: args.scope == Primary })`; `run(selected)`; `release(Det)`, `release(L4)`, `release(Ordering)` — today the context dies inside the runner, before any projection (`registry.rs:413-416`, `:526-529`), and the `View` must not hold it longer; `analyzer_diagnostics_of_run` over that run; projection, filter, baseline, suppression, coverage, format (`run.rs:522-700`, unchanged) | nothing (analyze had no duplicate); the failure path's second build (§4.4.5) |
| digest | `prepare(Plan { runs: [default], scope_primary: true, need_l4: true, after_run: TRANSACTION_SPANS })`; `run(default)` → diagnostics JSON; `tx_context_map(&det.transaction_spans)` (the loop of `digest_cli.rs:546-558` as a pure function; today's map is built at `ALL`, `:543-544`, and the default set's union has `TRANSACTION_SPANS` because d9 is a default detector, `detectors/mod.rs:1294` — `prepare`'s `after_run` assertion keeps that true, A2.10); `release(Det)`; `compose_snapshot(model)`; `resolve_changed_roots`; `compute_digest_effects_cli(&snap, model, Some(&roots))`; `DbTempStates::new(model, &l4.bundle)`; `run_digest_query_full_from_entries`; drop snapshot, entries, temp states; `release(L4)`; project JSON (with the diagnostics computed first); serialize | the second context (`:534-560`), the separate bundle (`:1625-1628`) |
| prove | the same plan; `run(default)` → diagnostics; `tx_context_map(&det.transaction_spans)` (today at `ALL`, `prove.rs:843`); `release(Det)`; `compose_snapshot`; roots; `compute_digest_effects_cli(&snap, model, Some(from_ref(root)))` (today `None`, `prove.rs:961`; each root's pass is independent, `digest.rs:4094-4112`); `DbTempStates::new(model, &l4.bundle)`; query; `unresolved_cone`; `release(L4)`; project | the second context (`prove.rs:838-859`), the separate bundle (`:964`) |
| fingerprint | `prepare(Plan { runs: [default] })`; `run(default)` → `analyzer_diags_for` (`fingerprint_cli.rs:547-555`); `--strict` gate; `release(Det)`, `release(L4)`; `compose_full_snapshot` / `compose_snapshot` as today (`:403`, `:464`) | nothing |
| diff | per side: a `View`; `--strict` → `run(default)`; release both; `compose_full_snapshot` (`diff/cli.rs:117`); the `View` dropped before the next side loads | nothing |
| events fanout / chains | `prepare(Plan { runs: [default], after_run: SUMMARIES })`; `run(default)` → `analyzer_diagnostic_lines`; `--strict`; the report from `det.event_graph`, `det.event_flow_indexes`, `det.summaries` (`events.rs:427-429`, `:572`; today from an `ALL` context, `:422-426`; `event_graph` and `event_flow_indexes` are built under every mask, `detector_context.rs:939`, `:1245`, and `summaries` under `SUMMARIES \| TRANSACTION_SPANS`, `:914` — the plan says so instead of relying on d9); release | the second context (`:422-426`, `:567-571`) |
| policy check | `prepare(Plan { runs: [default] })`; `run(default)` → diag lines; `--strict`; `resolve_policy_check` from `inputs.policy_file`, reported where `:230` reports it today (§4.2.3); `release(Det)`; then, with a policy loaded, the RAW build where `:261-266` builds it today: `let raw = build_detector_artifacts(model, SUMMARIES \| RAW_INHERITED_FACTS, Arc::clone(view.graph()), None, target.cross_app_inputs())` — a command LOCAL, not a cell, so no `det` ever carries `RAW_INHERITED_FACTS` (epic I5, Decisions 12 and 50) and the ~10.9 GB raw cone (`registry.rs:63-70`) is alive only after the detector loop, as today (`pipeline.rs:216` then `:261`); `PolicyModel::new(…, &raw.event_graph.events, …, &raw.summaries)` and `run_policy_engine` (`:281-289`); drop `raw`; release. The mask is `SUMMARIES \| RAW` and not today's `ALL \| RAW`: `PolicyModel` reads `event_graph` (built under every mask, `detector_context.rs:939`) and `summaries` (the `need_summaries` block, `:914`, `:1141-1146`, which `CORE_SUMMARIES` and `CLOSED_WORLD_TEMP` do not touch; `select_facts`, `policy_engine.rs:110-125`, reads the raw half), so the two maps are equal; `:926-930`'s assertion (`RAW` needs `SUMMARIES`) holds. The RAW build shares the view's graph (its own `build_combined_graph` and the v2 solve it ran under `ALL` are gone), so §4.4.8 counts it as `ctx` 2, `cone` 2, `pd_solve` 2, `v2` 1. It goes with step 6 (Decision 16) | the RAW context's own graph, Tarjan and solve (`:261-266` at `ALL`) |
| query | `prepare(Plan { runs: [], need_l4: true })` (`l4.warnings` printed by the fill, §4.4.7); `DbEffectQuery::build(&l4.bundle, &l4.scc, &graph)` | the two bundle builders (`effect_query_cli.rs:207-261`) |
| graph | no `View`: `export_from_program(&program)` | the private build (§4.3.3) |

Byte-neutrality of the reorders: every artifact is a pure function of the model, so
computing the diagnostics run before the snapshot changes no byte of stdout. What it
changes on stderr is in §4.4.7 and §7.

#### 4.4.5 analyze's failure classification (R6-3, Decision 29)

Today `build_cross_app_analysis_model` (`run.rs:370-402`) classifies a failed cross-app
build by RUNNING THE SINGLE-APP BUILD (`:387`), whose program build (`:285-291`) comes
before its id check (`:296-307`); `fresh` is that second build's result and
`empty_output_result` uses it as reason precedence 1 (`:884-890`). The second build is
deleted. The rows, with `fresh` defined on each:

| Row | Today | After step 2 |
|---|---|---|
| `inputs.model_instance_id` is `None` (no readable `app.json` with a non-empty string `id`, or a failed walk) | no cross-app build; one single-app build; `NoModelInstanceId`; `fresh` = that build's `Result` | ONE build, under today's profile: `ResolvedProgram::build(inputs, Summary)` — the id is known before any build, so this row never pays `FULL`'s `resolve_dependency_bodies` for an empty output; `NoModelInstanceId`; `fresh` = its `Result`. The `Err` text of a snapshot failure is the same for both profiles (`full.rs:2140-2148`: the snapshot build fails before the profile matters); a test pins it on an id-less `app.json` plus an unreadable `.al` (A2.7) |
| id present; the build returns `Err(e)` | single-app rerun (`:387`), which fails the same way; its arm walks (`:312-313`) and classifies `ProgramBuildFailed(why)` with `why` = the RERUN's own error (`:315`) if the `NestedApps::Skip` list is non-empty, else `AssemblyFailed`; `:392` passes that `Err` through; `fresh` = the rerun's `Err`. (The fixed `"the cross-app model could not be built"`, `:389-391`, appears only when the cross-app build fails but the rerun SUCCEEDS; then `fresh` is `Ok` today) | `ProgramBuildFailed(e)` if `inputs.app_scoped_files()` is non-empty, else `AssemblyFailed`; `fresh = Err(e)`. The `why` text is the cross-app build's own error: the same text as today when the rerun fails the same way (the usual case: one program build fails twice for one reason), and the real error instead of the fixed string in the cross-app-only failure, where `fresh` turns from `Ok` into `Err(e)`. The string reached the user only through `analysis failure — program engine build failed: <why>` (`:494-498`), exit 2; no test pins it (grep: none) |
| build `Ok`, cross-app assembly returns `None` (empty model, `workspace.rs:1781-1784`) | single-app rerun also assembles to nothing; `AssemblyFailed`; `fresh = Ok(single-app coverage)` | `AssemblyFailed`; `fresh = Ok(cross-app coverage)`. `empty_output_result` reads `fresh` only as `Err` text (`:884-890`), so the coverage value never reaches output |

`build.program` is 1 on every row (A2.7); today it is 1 on the first row and 2 on the
others. `build_analysis_model`'s single-app arm (`:285-364`) keeps its rows; its own
walk at `:312` reads `inputs` from step 1 (§4.2.3).

#### 4.4.6 The superset-mask parity test (R6-2, Decisions 12, 48)

`tests/gap/gap_detector_substrate_parity.rs:107-129` compares `findings` only, between
`ctx(ALL)` and `ctx(det.requires)`, calling `(det.run)` directly (`:117-119`). Step 2
widens it twice, and changes its masks once:

- The full context's mask becomes `ALL | ORDERING_FACTS`: `ALL` has no ordering bit
  (`registry.rs:141`), and d47/d49/d51 read `get_ordering_facts` (`d47.rs:379`,
  `d49.rs:123`, `d51.rs:167`), which today fills lazily and after step 2 `expect`s. The
  shim fills ordering for any mask carrying the bit (§4.4.2), so each detector's
  minimal context has it exactly when declared. Both contexts are `OwnedContext`s. No
  context carries `RAW_INHERITED_FACTS`: no detector ever runs against one (§4.4.4,
  policy's RAW build runs no detector), so there is nothing to license.
- The comparison is the whole `DetectorOutput` (`registry.rs:255-271`): `findings`,
  `stats`, `diagnostics` AND `d1_cohort_index`. `DetectorOutput`, `D1CohortIndex`
  (`finding.rs:213-217`) and `LoopSetRegistry` (`src/engine/l5/d1_cohort.rs:246-247`)
  derive `PartialEq, Eq` (their fields already do: `LoopCatalogEntry` at `finding.rs:230-231`,
  `Finding` at `:274`, `DetectorStats` at `registry.rs:165`).

`run_each` lifts `d1_cohort_index` into `RunOutput` (`registry.rs:682-684`) and the R4
projection serializes it, which is why a superset-mask dependence there would have
changed batch output under a green test.

#### 4.4.7 The non-convergence warning (Decision 30)

`run_one_scc_roles` prints `warning: summarize: Summary fixed-point did not converge for
SCC [...]` with `eprintln!` (`summary_runner.rs:1811-1814`) and ALSO returns the cap
members (`:1815`, `:1828`) as a `SummarizeDiagnostic`. Step 2 deletes the `eprintln!`
and makes the line a value:

- `l4::render_warnings(&[SummarizeDiagnostic]) -> Vec<String>` renders today's text;
  `build_l4` stores the result as `warnings`.
- ONE print site: `View::prepare` prints `l4.warnings` with `eprintln!` right after the
  `l4` fill, so the line is printed iff the cell was filled (`plan.need_l4 || some run's
  mask has CORE_SUMMARIES`) — the same place in the stream the solver printed from
  (during the build, before any stdout write), and the same kind of site (a library
  `eprintln!`, §5). One site, not one per command: with eight commands each printing
  for itself, a command that forgot would lose the line with no test failing (no
  fixture caps). "Filled", not "some run demands it": query's plan has no run
  (`runs: []`, `need_l4: true`) and today prints the line from its bundle builders
  (`effect_query_cli.rs:207-261` → `summary_runner.rs:1811-1814`); the line must not
  vanish there. Step 12 decides per-job emission from the stored value (epic §5.11).
- The snapshot's own fold solve (`capability_cone.rs:3215` inside `:3185-3247`)
  DISCARDS the diagnostics today (`let (summaries, _) = compute_summaries_v2(`); its
  line is the solver's `eprintln!`. Step 2 adds the print there — `for w in
  render_warnings(&diags) { eprintln!("{w}") }` at `:3215` — so the snapshot's line
  stays (the digest 2 / fingerprint 2 counts below depend on it) until step 3 deletes
  that solve and takes the line with it (§10 item 2).
- Two `aldump` callers also discard the diagnostics and LOSE the line: `project_r3a2`
  (`summary.rs:887`) and `project_r3a5_cross_app` (`capability_cone.rs:3619`). Stated
  in §7 as an `aldump` stderr change (epic I2); no golden carries it (no fixture caps).
  `db_effect_solver.rs:3328` is a unit test.

Effect on a capped SCC: digest and prove print 2 lines instead of 4, events and policy 1
instead of 2, analyze, fingerprint and query 1, 2 and 1 as today (§7). No fixture caps (the
corpus converges; `detector_context.rs:758-765`), so no golden carries the line; a unit
test on `render_warnings` from a hand-stated `SummarizeDiagnostic` pins the text. Stated
limit: the print site itself (`prepare`, `:3215`) is pinned by no executable test, since
no fixture produces a `SummarizeDiagnostic`; one site is the mitigation.

#### 4.4.8 Counts after step 2 (what A2.1 pins)

| Command | program | bcg | cone | v2 | scc | pd_solve | ctx | snap |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze, cross-app, default | 1 | 2 | 1 | 1 | 2 | 1 | 1 | 0 |
| analyze, cross-app, `--detector d47-…` | 1 | 3 | 2 | 2 | 4 | 2 | 1 | 1 |
| analyze `--single-app` | 1 | 1 | 1 | 1 | 2 | 1 | 1 | 0 |
| digest, cross-app | 1 | 3 | 2 | 2 | 4 | 2 | 1 | 1 |
| digest `--single-app` | 1 | 2 | 2 | 2 | 4 | 2 | 1 | 1 |
| prove | 1 | 2 | 2 | 2 | 4 | 2 | 1 | 1 |
| fingerprint (all modes) | 1 | 2 | 2 | 2 | 4 | 2 | 1 | 1 |
| diff ws ws, per side (`--strict`: ctx 1, bcg 2, cone 2, v2 2, scc 4, pd 2) | 1 | 1 | 1 | 1 | 2 | 1 | 0 | 1 |
| events, cross-app / single-app | 1 | 2 / 1 | 1 | 1 | 2 | 1 | 1 | 0 |
| policy check, cross-app / single-app, policy loaded | 1 | 2 / 1 | 2 | 1 | 3 | 2 | 2 | 0 |
| query, cross-app / single-app | 1 | 2 / 1 | 0 | 1 | 1 | 0 | 0 | 0 |
| graph | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

What remains above 1 is the snapshot's own graph, SCC, fold solve, cone and
`pd_solve` (steps 3 and 6), the cross-app base's second graph (the injected
dependency edges, step 4), and policy's RAW build (its own cone, `pd_solve` and the
cone's Tarjan at `capability_cone.rs:2783`; step 6, Decision 16). `ctx` counts
`build_detector_artifacts` entries (§4.1.2).

## 5. False-positive and failure shapes

- **A probe that changes bytes.** The census and the counters run under flags; A0.5 and
  A1.5 compare SARIF with the flag on and off. A timer on a per-entry path must go
  through `start()`/`add_since()`; a bare `Instant::now()` there would distort the number
  it reports (`detector_context.rs:435-443`).
- **A counter that counts a test helper.** `build.graph_scc` sits at named call sites;
  `tarjan_scc` itself is untouched, so the solver's inner Tarjan and d7's own never
  count.
- **A read that still hits the disk.** The disk-edit-after-read tests (§8.2) cover one
  class each. A new reader added later is caught by none of them; the rule is that
  every function taking `&Path` under `src/engine/` and `src/program/model/` is a
  review flag from step 1 on.
- **A stale `InputSet`.** The LSP keeps its own file reading; `SnapshotBuilder::from_disk`
  builds a fresh `InputSet` per build. Nothing caches an `InputSet` across builds.
- **A fail path that parses.** None: `compute_workspace_diagnostics` never parses
  after step 1 (§4.2.4); the one corner whose rows it drops is stated in §7.
- **A policy RAW build that becomes a cell.** It is a local of `run_policy_check`
  (§4.4.4); A2.5's parity test builds no RAW context, so a detector that read a raw
  fact would panic in `inherited_raw` (`registry.rs:72-74`) under every mask.
- **A post-run projection reading an artifact its plan did not ask for.**
  `Plan.after_run` plus `prepare`'s assertion (§4.4.3); A2.10.
- **A write failure that still aborts.** Only `println!`/`eprintln!` on stdout abort
  under `panic = "abort"`; §4.2.5 removes the last stdout `println!`. `eprintln!` to a
  closed stderr still aborts; out of scope (Decision 49 names stdout, stderr and files;
  the binary's own stderr lines go through `write_stderr`, §4.2.5, but a library
  `eprintln!` is not rerouted at this step).
- **An edge set in the wrong order.** `all_edges` is the only reader; A1b.3's two unit
  tests pin the tie rule and the order (§8.3), `graph_export_covers_dependencies` pins
  that the projection reads both sets, the graph golden covers the bytes. The fixture
  has no known `conf` tie, so the golden alone would pass an order swap.
- **A cell read before its fill.** `expect` with a message naming the cell; no lazy
  fill can hide it.
- **A rayon deadlock from a fill inside the loop.** The types rule it out (rule 2); the
  `!in_detector_loop()` assertion at `prepare`'s entry catches a second `View` built
  from inside the loop, in release builds.
- **Output depending on which run filled `det`.** `RunKey.demanded` decides
  `summarize_diagnostics`; §4.4.6 licenses the rest.
- **A second `pd_solve` hidden in a snapshot.** `build.pd_solve` counts it (§4.4.8).
- **The base's graph and the context's graph differing.** Both are
  `build_combined_graph` over the same cut calls and event graph plus the same injected
  edges (`capability_cone.rs:3470`, `:3513-3515`; `detector_context.rs:935-945`);
  `CombinedGraph` derives `PartialEq` (`combined_graph.rs:140`), and §8.4's equality
  test is TRANSIENT: it lands in the commit that adds the view's graph cell beside the
  context's own build, and the commit that deletes the context's build deletes the
  test (it would have nothing to compare). The commit history is the proof. The same
  rule covers `l4::bundle_equals_the_old_builders` (§8.4): `build_l4` lands beside
  `build_db_effect_bundle` / `cross_app_db_effect_bundle`, the test runs, then the
  deletion commit removes builders and test together. Neither keeps a test-only copy:
  a copy would pin the copy, not the code.
- **A profile-dependent `Err` text on the no-id row.** A2.7's test pins the text on the
  one input that produces it.

## 6. Measurement plan

### 6.1 Protocol (every step)

CDO `U:/Git/DO-cdo-baseline/Cloud` at `bc3ccb18`, clean, pinned `.alpackages`;
`release-fast` binaries copied out of `target/` (cdo-measure skill §1); the parent commit
and the step's head built from the same `tree-sitter-al` commit; no concurrent cargo
build. Commands: `analyze --format sarif`, `digest --diff c.diff` (the P5 diff, 48 roots),
`fingerprint --format json --out f.json`, and for step 2 also `prove` on one P5 root.
Runs interleave parent and head, ≥ 3 each, `ALSEM_TRACE=1 ALSEM_TRACE_STDERR=1`, the
process wall from the harness clock (`scratchpad/meas.py` pattern), both peaks from the
trace (§4.1.4). `RAYON_NUM_THREADS` unset on both sides (Decision 36).

Per timer: median and band = max − min over the parent's runs (G-5, Decision 54). A
bound is written as "head median ≤ / ≥ parent median ± band". The numbers file records
every run, not only the medians.

### 6.2 Step 0

No bound. Records: §4.1.7's table, including whether each timer's median is ≥ 2 × its
band (I6's rule). From the P5 numbers the expectation is: `context.capability_cones`
(20.9 s) and digest's wall (63–67 s) comfortably; `b3.adapter` (4.8 s) likely;
`b3.adapter.join` and `b3.adapter.by_decl` unknown until measured — if either's median
is < 2 × its band, step 10 / 11 is re-labelled non-speed before its spec is written
(epic I6). Census on/off byte check on SARIF (A0.5).

### 6.3 Steps 1, 1b, 2

| Step | Bound | Expected |
|---|---|---|
| 1 | none; both peaks and `analyze.total` reported against the parent | peak +5 MB (the retained texts), inside the band |
| 1b | process peak and post-attach peak ≤ parent median + band(peak) for analyze and digest | flat within the band: `ProgramMeta` holds the workspace texts (5.09 MB on CDO, §4.3.4) and nothing else of size; the body drops where it did |
| 2 | digest wall: parent median − head median ≥ 21 s − band(digest wall); both peaks ≤ parent + band for analyze and digest; cell census printed at the first run | ≈ −21 s on digest (the deleted context), ≈ −0.3 s on prove and events; analyze unchanged |

A miss on a peak walks the epic's fallback ladder (§5.12). A miss on the digest saving
is a stop: the deleted context is the whole claim.

## 7. Golden families each step may move

Byte-neutral means none of the 8 targets of `scripts/check-goldens` moves and no
`--lib` outcome moves. Per step, what is allowed to change outside the goldens:

| Step | Goldens | Stated non-golden changes |
|---|---|---|
| 0 | none | the `[cones-census]` stderr lines are renamed/split (a diagnostic, off by default); the `[C1_CONE_CENSUS]` report and its `C1_CONE_CENSUS=1` flag are replaced by the `context.<field>` buckets under `ALSEM_RETAINED_CENSUS=1` (§4.1.5; off by default); two new trace counter names |
| 1 | none; the in-process harnesses gain an `InputSet::read` line each (`tests/cli/cli_a_*`, `cli_b_*`, `cli_c_*`, `cli_d35_unknown_commit`, `cli_digest_*`, `cli_p1_inventory`, `cli_s6_consumers`, `cli_workspace_source`, `cli_analyze_*`, `cli_attribute_trivia`, `cli_event_dispatch_first_hop`, `cli_implicit_trigger_cone`) | a stdout/file write failure: one text, `al-sem: write error: <stream\|path>: <OS message>`, on every command; analyze exits 2 (today: abort), every other command keeps its exit 1 (today: exit 1 with four different texts); one stdout corner (§4.2.4): a nested app reached through a directory symlink, with an unreadable file inside it and no root `.al` file, loses its `No object declaration found in <rel>` info rows (the build parsed nothing; no fixture, no golden); a `.al` file added after the read no longer fails the model closed (§4.2.3, `select_program_files`); the `inputs.read` counter |
| 1b | `tests/cli-query-goldens/graph.test-selection-graph.jsonl` must NOT move (A1b.1) | none |
| 2 | none; `tests/gap/gap_detector_substrate_parity.rs` widened; `--lib` tests that build a `DetectorContext` by hand (`d17.rs:504`, `d50.rs:619`, `:784`, `detector_context.rs` tests) and the 63 `minimal_ctx` sites (`test_support.rs:367`) are rewritten to the split, same assertions | on a capped SCC: fewer duplicate non-convergence lines on `alsem` (§4.4.7), and `aldump --r3a2` / `--r3a5-cross-app-summary` lose the line (their solves discard the diagnostics, `summary.rs:887`, `capability_cone.rs:3619`); analyze's build-failure message carries the real error where today's fixed string stood (§4.4.5, row 2: cross-app build fails, single-app rerun would have succeeded) |

The CDO `aldump --program-call-graph-stats` histogram SHA and the `cdo-measure` detector
baseline are checked at every step (epic I10, E14).

## 8. Discrimination-proof plan

Every new test lists its break, the failing assertion, and the revert (epic I9). Breaks
are made with the Edit tool, run, then reverted with Edit; both outcomes go in the
ledger. A scripted break is never used.

### 8.1 Step 0

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `cli_build_counters::<command>_<universe>` (one per §4.1.3 row) | per counter: add a second call of the counted builder at its site (e.g. a second `build_combined_graph` in `build_cross_app_base`); for `build.graph_scc`: add a `pt::counter_delta` to the solver's inner Tarjan | the table cell for that counter | remove the call / the counter |
| `cli_build_counters::census_rest_is_never_negative` (spawned `alsem analyze` on `ws-d1-multi-caller` with `ALSEM_CONES_CENSUS=1`; parse the `[cones-census]` `rest` line from stderr). Spawned, not `--lib`: `cones_census::enabled()` is a `OnceLock` read once from the environment (`detector_context.rs:426-429`) and its counters are process-global atomics (`:348-424`), so another lib test in the same process can fix the flag off or add to the counters first | charge `SINGLETON_NANOS` (`:362`) twice. Not `BFS_FOLD_NANOS`: the fixture has no recursive SCC (`tests/r0-corpus/ws-d1-multi-caller/src/D1MultiCaller.al`: three callers of one helper, no cycle), so the `recursive` arm (`capability_cone.rs:2552-2561`) never runs there and a double charge of a counter that stays 0 changes nothing — the break would pass. Every routine of the fixture is a singleton (`:2563`), so a double `SINGLETON` charge makes `rest` negative | `rest >= 0` | once |
| `pd_solve::bytes_split_sums_to_the_old_total` (unit, hand-stated small arena) | drop `reps` from `working_bytes` | the sum equals the pre-split `solve_bytes` value computed in the test | restore |
| `cli_build_counters::phase_peak_is_reported` (spawned `alsem analyze` on `ws-d1-multi-caller` with `ALSEM_TRACE=1`; read both `peak.*` counters). Spawned for the same reason: the tracer is a `OnceLock` from the environment (`perf_trace.rs:402-407`). An absolute "≥ 64 MB" bound is not used — a shared test process may already sit above it | stop the sampler before attach runs | `0 < peak.post_attach_mb <= peak.process_mb` (the sampler never ran → 0) | restore |
| A0.5 (measurement, not a test): SARIF census on == off on CDO | — | — | — |

### 8.2 Step 1

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `inputs::read_once` (`inputs.read` count 1 per command, via the trace reader) | call `InputSet::read` a second time in `AnalysisTarget::build` | count == 1 | remove |
| `inputs::app_json_edit_after_read` (`analyze --format pr-summary`: read, rewrite `app.json`'s `name`, run; output equals the untouched run) | restore `read_workspace_apps`'s disk read at `run.rs:122-157` | the pr-summary bytes | remove |
| `inputs::roots_config_edit_after_read` (fingerprint json: read, delete `roots.config.json`, run; `inputs[]` rows and `rootClassifications` equal the untouched run) | restore the disk read in `load_roots_config` | the snapshot bytes | remove |
| `inputs::al_text_edit_after_read` (analyze json on `ws-d14-dead-routine`: read, append `// al-sem-ignore` above the finding's line on disk, run; the finding is still reported) | restore `load_inline_suppressions`'s disk read | `findings.len()` | remove |
| `inputs::objectless_file_removed_after_read` (analyze json: a workspace with one objectless `.al`; read, delete that file, run; the `DIAG-index` line is still emitted) | restore the second parse / disk walk in `compute_workspace_diagnostics` | the diagnostic's presence | remove |
| `inputs::al_file_added_after_read` (analyze json on `ws-d1-multi-caller` copied to a tempdir: read, write a new `src/Added.al` with one codeunit, run; the output equals the untouched run's). Today's `select_program_files` would walk again, list the new file, miss it in the parse and return `ProgramFiles::Missing` (`workspace.rs:1644`, `:1657`): no model | restore the `discover_al_files_app_scoped` walk at `workspace.rs:1644` | the output equality (the model fails closed) | remove |
| `inputs::closure_app_replaced_after_read` (fingerprint json on the dependency workspace: read, overwrite the dependency `.app` bytes, run; `workspaceFingerprint` unchanged). Stated limit: it pins the HASH rows only; the loader still parses the `.app` after the hash (§4.2.2, Goal 2's exception), so a swap between the two reads gives a model from the new bytes under the old hash — today's race, unchanged | restore `hash_file` reads in `derive_inputs` | the fingerprint string | remove |
| `inputs::policy_edit_after_read` (policy check json: read, rewrite the yaml on disk, run; findings unchanged) | restore the read in `resolve_policy_check` | the policy output | remove |
| `inputs::baseline_edit_after_read` (analyze `--baseline`: read, rewrite the baseline file to suppress everything, run; findings unchanged) | restore `load_baseline`'s read | `findings.len()` | remove |
| `baseline::update_baseline_applies_the_set_just_written` (A1.6; in-process, `ws-d8-commit-in-tx` like `update_baseline_round_trips_committed_file`, `tests/cli/gate_suppress_baseline_differential.rs:345-362`, which checks only the written file): `--baseline <tmp> --update-baseline` with (a) no file, (b) a malformed file; stdout has zero findings and the exit is clean in both, as today (`run.rs:584-598`: written, then read back) | apply `inputs.baseline` instead of the written set | (a) `findings.len()`; (b) the exit code (2, from the malformed read) | restore |
| `inputs::walk_error_fails_the_build` (A1.7; a workspace whose root holds a directory symlink loop, built in a tempdir — skipped where the platform cannot create one): `al_walk` is `Err`; `ResolvedProgram::build` `Err`s with the `walking source` text, `inputs.model_instance_id` is `None`, `compute_workspace_diagnostics` returns the provider rows alone | make `read` store `Ok(())` and the partial list | the three assertions | restore |
| `cli_write_failure::analyze_closed_pipe_exits_2` (spawn with piped stdout, drop the read end before the child writes, wait) | restore `println!("{out}")` | exit code 2 and the stderr line | remove |
| `cli_write_failure::digest_out_unwritable_exits_1` (`--out` into a directory that is a file) | restore the `al-sem: digest: write error:` text | the `al-sem: write error: <path>:` line (exit stays 1) | remove |
| A1.3: no test of its own. There is no fallback to pin (§4.2.4); `inputs::objectless_file_removed_after_read` above is the pin that the index rows come from the build | — | — | — |

### 8.3 Step 1b

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `graph_export_golden` (existing, `cli_graph_export.rs:702-716`) | — (the fixture has no known `conf` tie, so an order swap passes it; the two rows below pin the order) | — | — |
| `project::add_keeps_first_via_on_conf_tie` (unit in `project.rs`'s tests: two hand-built `E`s with the same `(from, to, kind)`, equal `conf`, different `via`, added in order) | change `new.conf < old.conf` to `<=` (`project.rs:287`) | the kept `via` is the first one's | restore |
| `resolved::all_edges_is_edges_then_dependency_bodies` (unit: a `ProgramBody` with one hand-stated edge in each set) | swap the two sets in `all_edges` | the yielded order | restore |
| `graph_export_covers_dependencies` (existing, `:597-650`; checks presence and `sourcePath`, not order) | feed `project` `edges` only (drop `dependency_body_edges`); separately, pass an empty `site_facts` | the dependency node's `sourcePath`/edges; the resolution-dependent fields | restore |
| A1b.2: no unit test. `attach_program_calls_with` takes `program.body.take()`, so `body.is_none()` afterwards is true by construction and pins nothing; the break "keep a clone" does not compile (`ProgramContext` derives no `Clone`, `full.rs:1489`). The claim is that the body's memory is released at `b3.ctx_drop` (`program_calls.rs:674-676`); A1b.4's post-attach peak is the measurement of that | — | — | — |
| `resolved::dependency_bodies_histogram_equals_merged_filter` (unit on the dependency workspace: the §4.3.2 claim, both computations) | add a `CallSite` filter bug (drop the `from.app` test) | the two histograms | restore |

### 8.4 Step 2

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `cli_build_counters` at the §4.4.8 values (existing test, table edited) | re-add `compute_tx_context_map`'s context build in digest | `ctx == 1` | remove |
| `l4::bundle_equals_the_old_builders` (TRANSIENT, §5: unit on `ws-d8-commit-in-tx` and the dependency workspace: `build_l4` vs `build_db_effect_bundle` / `cross_app_db_effect_bundle` compared through a test-only `SummaryBundle::rows_for_test()` rendering every routine's `db_effects`; lands with `build_l4` beside the old builders, deleted in the commit that deletes them) | pass empty `leaf_summaries` to `build_l4` on the cross-app case | the rendered rows | restore |
| `session::base_graph_equals_context_graph` (TRANSIENT, §5: unit, cross-app: `base.graph == build_combined_graph(..) + injected`, `PartialEq`; deleted with the context's own graph build) | drop the injected extend on one side | equality | restore |
| `session::after_run_bits_must_be_in_the_union` (A2.10, unit: `prepare` on `Plan { runs: [[a detector with requires: 0]], after_run: TRANSACTION_SPANS }` over `View::new(AnalysisTarget::SingleApp(Box::new(empty_resolved())))`) | remove the assertion | the expected panic | restore |
| `session::no_lazy_fill_in_this_file` (A2.9, unit: `include_str!("session.rs")`, needles built with `concat!`) | add an `OnceLock` field to `View` | the `contains` assertion | remove |
| (no `fill_inside_detector_loop_is_refused`: a detector cannot reach a fill, rule 2, so such a test could only call the fill helper directly, which pins the helper and not a use) | — | — | — |
| `session::run_diagnostics_follow_the_run_mask` (unit: two runs on one view, masks with and without `CORE_SUMMARIES`, a hand-stated `l4.summarize_diagnostics`; the second run's `summarize_diagnostics` is empty) | take the diagnostics from `det.demanded` instead of the key | the empty assertion | restore |
| `gap_detector_substrate_parity` widened (§4.4.6) | make d1 return `d1_cohort_index: None` when `RAW_INHERITED_FACTS` is set | the `DetectorOutput` equality on `ws-d1` | restore |
| `prove::some_root_moves_nothing` (cli-b prove goldens, existing) + `prove::wrong_root_set_is_entry_missing` (pass `Some(&[other])`) | — / pass the full root list and assert the error | the `Err("…entry missing…")` | — |
| `run::no_id_row_runs_one_build_and_keeps_the_text` (id-less `app.json` + unreadable `.al`; `build.program == 1` from the trace; `fresh` `Err` text equals the literal captured today) | restore the single-app rerun | the count; the text if it ever diverges | remove |
| `l4::warning_text_is_rendered_once` (unit: a hand-stated `SummarizeDiagnostic` → `render_warnings(..) == ["warning: summarize: Summary fixed-point did not converge for SCC [a, b]"]`). Stated limit: it pins the rendering, not the print; the one print site in `prepare` and the one at `capability_cone.rs:3215` are pinned by nothing executable (no fixture caps, §4.4.7) | change the prefix | the literal | restore |
| A2.3, A2.4 (measurements, §6.3) | — | — | — |

## 9. Carried findings: what this document resolves, what it hands on

| Finding | Resolved here | Handed to |
|---|---|---|
| G-1 / R6-1 (instrument half) | `PD_SOLVE_NANOS`, `PD_SOLVE_CALLS`, `build.pd_solve`; `solve_bytes` split into arena vs working set; the solve inside the §4.1.6 timings; `SGL_SCAN_OWN_NANOS` for the own-entry population | step 6: retain `resolved` in view identity (budget = `PD_SOLVE_BYTES_WORKING`) or re-solve (cost = `PD_SOLVE_NANOS`), and re-budget §5.6/§5.12 |
| G-2 (step-2 half) | §4.4.4: digest's and prove's snapshot is composed after `det` is released; the tx map is a small projection taken before the release (Decision 56); `snapshot.<field>` census buckets size the `ViewTree`-equivalent | step 6: which `ViewTree` parts a digest-only plan keeps, and the digest fallback rung |
| G-3 (instrument half) | `BFS_WALK_NANOS` / `BFS_FOLD_NANOS` / `BFS_PICKS`; the numbers file names the command each is measured on (analyze cross-app, default set) | step 7a: its bound reads the walk half and names the command |
| G-5 | §6.1: per-timer band = max − min of the parent's interleaved runs; §6.2 records whether each median is ≥ 2 × its band | steps 7a, 7b, 10, 11 apply the rule before writing a bound |
| G-6 | §4.3.4: `ProgramMeta` holds the workspace text (5.09 MB on CDO); the `inputs` census bucket | — |
| R6-2 | §4.4.6: all four fields of `DetectorOutput`; `D1CohortIndex` and `LoopSetRegistry` derive `PartialEq` | — |
| R6-3 | §4.4.5: the no-id row keeps one build (the cross-app one); `fresh` is its `Result`; the `Err` text is pinned by a test; the second build is gone and counted | — |
| R6-5 | §4.3.2: the sets are named for what they hold; the order is kept because the projection's tie rule is order-sensitive; the `dependencyBodies` histogram equality is pinned | — |
| G-4, R6-4 | not this document's | step 6 |

## 10. Open items for the next child spec (steps 3, 4)

1. After step 2 the cross-app base still builds its second graph for the injected
   dependency edges (`capability_cone.rs:3496-3515`); step 4's `ExecRelation` replaces
   it. The step 0 `base` census bucket sizes what that saves.
2. The snapshot's fold solve (`:3185-3247`) is the last `compute_summaries_v2` outside
   `l4`; step 3 deletes it and takes the stderr line with it (§4.4.7).
3. `R3a5CrossAppBase.graph` and `combined_scc` are `Arc`s from step 2 (the view and
   `l4` clone the pointers, §4.4.3 rule 1, §4.4.1); step 4 decides whether the base
   survives at all or `rel` absorbs its fields.
4. `build_detector_context[_with]` survive as test-only shims returning `OwnedContext`
   (§4.4.2); step 4's spec says when the parity test moves to
   `build_detector_artifacts` and they are deleted.

## Review log

Two reviews of this document (2026-10-18), every finding checked in source at
`U:/Git/al-sem-issue-160-a1` before the fix. A = the implementability review, B = the
memory/speed/scheduling review.

| # | Finding | Verified at | Outcome |
|---|---|---|---|
| A1 | Step 1 changed `--update-baseline`: the baseline is written (`run.rs:584-591`) and then read back (`:597`), so today every current finding is dropped and a malformed old file is overwritten before any read; reading the `InputSet` bytes would apply the old file | `run.rs:584-598`; `gate_suppress_baseline_differential.rs:345-362` checks only the written file | Fixed: §4.2.3 `load_baseline` row applies the set just written and never reads the slot under the flag; A1.6 + `baseline::update_baseline_applies_the_set_just_written` pin stdout and the exit code |
| A2 / B4 | No slot for a failed `.al` walk; a partial list could reach the build | `source_text.rs:63-65`, `:85`; `model_instance_id.rs:83`; `provider.rs:34-35`; `workspace_diagnostics.rs:93-95` | Fixed: `al_walk: Result<(), String>` (§4.2.1); the three readers fail closed with today's texts (§4.2.3); `ClosureEntries` defined; A1.7 + `inputs::walk_error_fails_the_build` |
| A3 | `app_json: Result<AppJson, String>` could not tell unreadable from malformed; readers differ | `snapshot.rs:193-197`; `snapshot_full.rs:819-827`; `workspace_diagnostics.rs:145-151`; the text reaches output at `run.rs:884-886`, `:494-497` | Fixed: `app_json: FileInput` + `app_json_value: Option<Result<Value, String>>`; each reader's message named (§4.2.1, §4.2.3) |
| A4 | Exit 2 on every command collides with digest's "no roots" and prove's "unresolved routine"; the epic says "two sites exit 1" | `digest_cli.rs:1652`; `prove.rs:950`; `alsem.rs` write-failure sites: `:731`, `:755`, `:872`, `:910`, `:992`, `:1132`, `:1200`, `:1327-1339`, `:1404`, `:1477`, `:1496` (eleven, all exit 1); epic §5.10 `:318-320`, Decision 49 `:490`, step-1 row `:514` | Fixed: one text, exit = the command's failure code (analyze 2, others 1, as today) (§4.2.5, §7, A1.4); the epic's Decision 49, §5.10 and step-1 row amended to that rule and the real count — an epic-level decision changed on a verified miscount |
| A5 | A2.9's grep gate needs `scripts/ci-steps`, which the issue flow may not edit | `CLAUDE.md:873` | Fixed: a `--lib` test over `include_str!("session.rs")` (A2.9, §8.4) |
| A6 | Nothing pinned the edge order; `graph_export_covers_dependencies` checks presence, not order, and the fixture has no `conf` tie | `cli_graph_export.rs:597-650`; `project.rs:277-294` | Fixed: `project::add_keeps_first_via_on_conf_tie` + `resolved::all_edges_is_edges_then_dependency_bodies` (§8.3, A1b.3, §5) |
| A7 | A1b.2 vacuous (`body.take()`); its break does not compile (`ProgramContext` has no `Clone`) | `full.rs:1489`; `program_calls.rs:669-676` | Fixed: the test is dropped; A1b.2 now points at the span and A1b.4's measurement |
| A8 | query lost the non-convergence line (plan has no run) | `effect_query_cli.rs:207-261` → `summary_runner.rs:1811-1814` | Fixed: the line prints iff the `l4` cell was filled (§4.4.7, §4.4.4 query row) |
| A9 | §4.2.4 named one `fresh` `Err` case; there are more, and most never parse | `snapshot.rs:193-197`; `provider.rs:34-39`; `run.rs:300-326`; `workspace_diagnostics.rs:49-95` | Fixed: the cases listed and the one that parses isolated; A1.3's fixtures named |
| A10 | §4.2.3 kept policy validation at `:230` (move at step 12), §4.4.4 moved it above the build at step 2 | `pipeline.rs:196-241` | Fixed: at step 2 validation runs after the build and before `prepare`; the error is reported in today's order, so no precedence changes; Decision 7's move stays at step 12 (§4.2.3, §4.4.4) |
| A11 | `write_stderr` used in §5 but never defined | spec §4.2.5 vs §5 | Fixed: defined in §4.2.5 (no message, the stream is gone; the command's failure code) |
| A12 | The tx map moved from `ALL` to the plan's union mask with nothing pinning `TRANSACTION_SPANS` in it | `digest_cli.rs:543-544`; `prove.rs:843`; `detectors/mod.rs:1294` (d9 in the default set); `registry.rs:53-56` (the bit forces summaries) | Fixed: `Plan.tx_map`; `prepare` asserts the bit; A2.10 + `session::tx_map_plan_requires_transaction_spans` |
| B1 | The parity test calls `(det.run)` directly at `ALL` (no ordering bit) and `every_detector_alone_declares_what_it_reads` relied on `run_each`'s fill; both break once `get_ordering_facts` `expect`s | `gap_detector_substrate_parity.rs:114-121`; `registry.rs:141`, `:613-621`, `:1002-1007`; `d47.rs:379`, `d49.rs:123`, `d51.rs:167` | Fixed: the shims fill ordering for any mask with the bit; the parity masks gain `ORDERING_FACTS` (§4.4.2, §4.4.6) |
| B2 | A thin function cannot return a `DetectorContext<'v>` that borrows artifacts it built | 47 call sites in 17 files; `d17.rs:504`, `d50.rs:619`, `:784` are struct literals | Fixed: the shims return `OwnedContext` with `ctx()` (§4.4.2) |
| B3 | analyze kept `det`/`l4` alive through projection and formatting; today the context dies in the runner | `registry.rs:413-416`, `:526-529`; `detector_context.rs:1499-1505`; `run.rs:522-700` | Fixed: `release(Det)`, `release(L4)`, `release(Ordering)` right after `run(selected)` (§4.4.4) |
| B5 | `IN_CELL_BUILD` contradicted itself; its test could pin only the helper; the types already forbid a fill from a detector | `registry.rs:318` (detector signature), `:540-566` | Fixed: the flag and the test are dropped; one `!in_detector_loop()` assertion at `prepare`'s entry (§4.4.3 rule 2, §5, §8.4) |
| B6 | Two step-0 tests were `--lib` tests over process-wide `OnceLock` flags and global atomics; the 64 MB bound was absolute | `perf_trace.rs:402-407`; `detector_context.rs:348-429` | Fixed: both are spawned-process tests; the peak assertion is `0 < post_attach <= process` (§8.1) |
| B7 | `COMPOSE_REST_NANOS` undefined over accumulating atomics and three compositions | spec §4.1.1, §4.1.3 | Fixed: charged per call from entry snapshots (§4.1.1) |
| B8 | `L4Analysis` kept the ~730 MiB harvest alive through the query phase; today only the bundle survives | `detector_context.rs:1440`; `effect_query_cli.rs:219`; `prove.rs:964` | Fixed: `L4Harvest` moves into `det` by value; `L4Analysis` keeps scc, field index, bundle, diagnostics, warnings; no `Arc` fields (§4.4.1, §4.4.2) |
| B9 | Single-app `prepare` always ran Tarjan; today it runs only under `CORE_SUMMARIES` | `detector_context.rs:1296` | Fixed: `scc` is filled with `l4` (§4.4.3 rule 1) |
| B10 | `inputs.read` called a span in A1.1 and §4.2.1, a counter in §4.1.2 | spec | Fixed: a counter everywhere |
| B11 | §6.3 said `ProgramMeta` is small; §4.3.4 says 5.09 MB | spec | Fixed: §6.3 matches §4.3.4 |
| B12 | Goal 2 overstated the freeze: the loader re-reads each `.app`; the closure test pins the hash rows only | spec §4.2.2, §8.2 | Fixed: Goal 2 names the exception; the test states its limit |
| B13 | The no-id row ran a `FULL` build (with `resolve_dependency_bodies`) for an empty output | `full.rs:2165-2180`; `run.rs:285-307` | Fixed: the no-id row builds under `Summary`, today's profile (§4.4.5) |
| B14 | A second `mark_phase_start` (diff, once per side) was unspecified | `diff/cli.rs:103` → `program_calls.rs:727` | Fixed: restart; the last phase's maximum is reported (§4.1.4) |

Third and fourth reviews (2026-10-18, after the rows above). C = the 12-finding review,
D = the steps 0–2 review (I1–I3, M1–M7). Every finding checked in source at the same
tree before the fix; none refuted. Where a C and a D row are one finding, the C row
holds the fix and the D row points at it.

| # | Finding | Verified at | Outcome |
|---|---|---|---|
| C1 | Two post-build disk reads were missing from §4.2.3: `select_program_files` reads the id and walks during assembly (a file added after the read is `ProgramFiles::Missing`, fail-closed); the single-app arm walks to classify the failure. Goal 2 did not hold | `workspace.rs:1636-1661` (`:1643`, `:1644`, `:1657`, called at `:1671`); `run.rs:312-313` | Fixed: two rows in §4.2.3 (`inputs.app_id()`, `inputs.app_scoped_files()`); `inputs::al_file_added_after_read` (A1.2, §8.2); §4.4.5's "unchanged" corrected; §7 states the behaviour change (an added file no longer fails the model) |
| C2 / D-I1 | The A1.3 "fixture that needs it" (a nested app with an unreadable nested file) cannot make the fallback parse run: a second `app.json` makes `is_multi_app` true and the function returns before any parse; only a nested app behind a directory symlink reaches it (walkdir follows links, `count_app_json_paths` does not) | `workspace_diagnostics.rs:44-49`, `:86`, `:93-95`; `source_text.rs:67-68`, `:153-156`; `run.rs:308-326`, `:491-498` (`ProgramBuildFailed` never reaches `empty_output_result`) | Fixed: the fallback is DELETED. §4.2.4 names the one corner (symlinked nested app + unreadable file inside it + no root `.al`) and the rows it loses; §7 step 1 states it; A1.3's evidence is the absence of `al_syntax::parse` plus `objectless_file_removed_after_read`; §5 and §8.2 updated |
| C3 | Policy's `det` carried `RAW_INHERITED_FACTS` (rule 1's `\| RAW if plan.raw_inherited`), against epic I5 ("no `det` ever carries one … policy included"), Decision 12 and Decision 50; the ~10.9 GB raw cone would have lived through the detector loop | `pipeline.rs:216` then `:261-266` (today: separate, after the diagnostics context is gone); `registry.rs:63-70`; epic I5, Decisions 12, 50 | Fixed: `Plan.raw_inherited` deleted; `det` is always the plain union; policy's RAW build is a command-local `build_detector_artifacts(.., SUMMARIES \| RAW, ..)` after `release(Det)`, sharing the view's graph (§4.4.4, with the argument why `SUMMARIES \| RAW` equals today's `ALL \| RAW` for `PolicyModel`); §4.4.6's RAW parity context dropped; §4.4.8 policy row `ctx` 2, `cone` 2, `pd` 2, `scc` 3; Goal 4 and A2.1 state the exception; the epic's step-2 row says so |
| C4 | The child contradicted the epic in three places: `IN_CELL_BUILD` (epic §5.9 rule 1, Decision 31, step-2 row) vs B5; the step-1b row's `body.is_none()` vs A7; epic §5.8's `L4Analysis` holding the core summaries vs B8 | epic `:289-294`, `:476`, `:519-520`, `:259-264` | Fixed in the EPIC (as for Decision 49): rule 1 and Decision 31 withdraw `IN_CELL_BUILD`; the step-1b row points at the `b3.ctx_drop` span and the peak; §5.8 names `L4Harvest`; the step-2 row names the `prepare` assertion and policy's local RAW build |
| C5 | §4.4.7 said the snapshot's fold solve "prints the diagnostics it gets back"; it discards them (`let (summaries, _) =`), so deleting the solver's `eprintln!` would silently drop the snapshot's line (digest 2 → 1, fingerprint 2 → 1). Two `aldump` callers discard them too | `capability_cone.rs:3215`; `summary_runner.rs:1811-1814`; `capability_cone.rs:3619` (`project_r3a5_cross_app`), `summary.rs:887` (`project_r3a2`); `db_effect_solver.rs:3328` (a test) | Fixed: §4.4.7 states the added print at `:3215` (`render_warnings` + `eprintln!`) and the two `aldump` paths that lose the line; §7 step 2 lists them (epic I2); §4.4.1 notes the deleted bundle builders discarded them as well |
| C6 | `census_rest_is_never_negative`'s break (charge `BFS_FOLD_NANOS` twice) cannot fail: `ws-d1-multi-caller` has no recursive SCC, so the BFS arm never runs and the counter stays 0 | `tests/r0-corpus/ws-d1-multi-caller/src/D1MultiCaller.al`; `capability_cone.rs:2552-2563`; `detector_context.rs:362` | Fixed: the break charges `SINGLETON_NANOS` twice, which every routine of the fixture runs through (§8.1) |
| C7 | §4.4.5 row 2's "today" was wrong: the `why` text is the single-app rerun's own error whenever the rerun fails; the fixed string appears only when the rerun succeeds, and then `fresh` is `Ok` | `run.rs:312-316`, `:387-394` | Fixed: the row describes both today's cases and the delta on each (the fixed-string case turns `fresh` from `Ok` into `Err(e)`); §7 wording matches |
| C8 | `warning_text_is_rendered_once` pins the renderer; eight per-command print sites were pinned by nothing | spec §4.4.7 | Fixed: ONE print site, `View::prepare` after the `l4` fill (plus the stated one at `:3215`); the stated limit (no fixture caps, so the site is unpinned) is written in §4.4.7 and §8.4 |
| C9 | Raw bytes + a JSON parse would turn a non-UTF-8 `app.json` / baseline from a READ error into a parse error with different text | `snapshot.rs:194` (`read_to_string`); `baseline.rs:38`, `:44`, `:47` | Fixed: `FileInput::text()` reproduces `read_to_string`'s `InvalidData` error (`stream did not contain valid UTF-8`); both readers go through it; `app_json_value` is `None` on invalid UTF-8 (§4.2.1, §4.2.3) |
| C10 | Replacing `emit_full_census` removed the `[C1_CONE_CENSUS]` report without §7 saying so; the flag also gates two residual-census sites | `detector_context.rs:1161`; `cone_census.rs:77-80`, `:201`; `capability_cone.rs:2675`, `:2851` | Fixed: `ALSEM_RETAINED_CENSUS` replaces `C1_CONE_CENSUS` outright (one flag; the two residual sites move to it; the per-fact accounting is kept), stated in §4.1.5 and §7 step 0 |
| C11 / D-M5 | Citation drift (d7's call is `:144`, `:94` is its `fn`; the effect_query helper is `:424`); `combined_graph.rs:1128` (`project_r3a1_combined_graph`, aldump) was neither counted nor excluded; the counter sites after step 2 were unstated | `grep tarjan_scc\(` over `src/`; `detector_context.rs:1303` | Fixed: §4.1.2's list corrected and extended; a paragraph states where `build.detector_context` (at `build_detector_artifacts`) and the `:1303` `build.graph_scc` (inside `build_l4`) sit after step 2 |
| C12 | `bundle_equals_the_old_builders` compares against builders step 2 deletes; nothing said whether it runs before the deletion or keeps copies | spec §8.4, §4.4.1 | Fixed: both oracle tests (`bundle_equals_the_old_builders`, `base_graph_equals_context_graph`) are stated TRANSIENT: land beside the old code, deleted in the deletion commit, no test-only copies (§5, §8.4) |
| D-I2 | `test_support::minimal_ctx` builds today's context as a struct literal from a routine SLICE and owned graph/summaries; 63 call sites; neither `detector_context(model, ..)` nor `OwnedContext<'m>` fits it | `test_support.rs:367-424`; `grep minimal_ctx\(` (63 in 8 files) | Fixed: `DetectorContext::new(routines, objects, tables, art, ordering)` over slices; `detector_context(model, ..)` calls it; `minimal_ctx` returns `MinimalCtx<'a> { routines, art }` with `.ctx()` (§4.4.2, §7) |
| D-I3 | `View::new(inputs, program, target)` could not be built: `AnalysisTarget::build` returns no `ResolvedProgram`, the cross-app program is consumed inside `build_cross_app_base`, the `run_detectors` callers hold a `&Model`, and nothing at step 2 reads `View.program` | `capability_cone.rs:3455-3585`; `registry.rs:384`, `:447`, `:1088`; callers `run.rs:508/512/1032/1057`, `switch_dump.rs:126`, `finding.rs:952/1049` | Fixed: `View::new(target)`; `inputs` and `program` leave the step-2 `View` (the epic's struct annotated); `run_detectors[_cross_app]` keep their signatures over the shim and share `finish_run` with the view; their callers listed (§4.4.3) |
| D-M1 | Rule 1 said the cross-app graph is "moved out of the base"; §10 said it is an `Arc` on the base; the r3a5 projections and `from_cross_app_base` read `base.graph` | `effect_query_cli.rs:166-178`, `:207-220`; `capability_cone.rs:3613` | Fixed: the base's `graph` becomes `Arc<CombinedGraph>` and stays; the view clones the pointer (§4.4.3 rule 1, §10 item 3) |
| D-M2 | The SCC had two or three owners (`View.scc`, `L4Analysis.scc`, `base.combined_scc`), up to three copies over 31,658 routines | `capability_cone.rs:3336`; `scc.rs:40-46` | Fixed: one owner, `L4Analysis.scc: Arc<SccResult>`; `base.combined_scc` is an `Arc` cloned by pointer; `View.scc` deleted; `build_l4` takes `Option<Arc<SccResult>>` and runs Tarjan for `None` (§4.4.1, §4.4.3, §10) |
| D-M3 | Events read `det.summaries` from the default union with nothing in the plan saying so (non-empty only because d9 demands `TRANSACTION_SPANS`); digest had A12's guard, events none | `events.rs:422-429`; `detector_context.rs:914`, `:939`, `:1245` | Fixed: `Plan.tx_map: bool` generalized to `Plan.after_run: u32`; `prepare` asserts the bits are in the union; events declares `SUMMARIES`, digest/prove `TRANSACTION_SPANS` (§4.4.3, §4.4.4, A2.10, §8.4) |
| D-M4 | A 10 ms sampler can take no sample on the tiny fixture; `post_attach_mb == 0` fails `phase_peak_is_reported` intermittently | spec §4.1.4, §8.1 | Fixed: one synchronous sample at `mark_phase_start` and one at stop (§4.1.4) |
| D-M6 | Rule 5's per-fill reset changed what the census numbers mean; `report()` prints cumulative atomics, so step 0 and step 2 lines would not compare | `detector_context.rs:465-558`, `:1150` | Fixed: no reset, counters stay process-cumulative, the numbers file quotes the process total (§4.1.7, rule 5); the EPIC's §5.9 rule 4 amended |
| D-M7 | Rule 3 said `run(key)` "executes", but `run(&self) -> &RunOutput` cannot store a result without interior mutability (rule 4 forbids it) | spec §4.4.3 | Fixed: runs execute inside `prepare`; `run` returns the stored output (rule 3) |
