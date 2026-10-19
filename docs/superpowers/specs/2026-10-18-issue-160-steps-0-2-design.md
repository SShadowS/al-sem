# #160 child spec: steps 0, 1, 1b, 2 — measurement, frozen inputs, retained program, owned artifacts

Status: implementable design, pre-implementation. Child of
`2026-10-18-issue-160-analysis-session-design.md` (the epic). The epic holds the goal,
the architecture, every Decision, the invariants (I1–I11) and the delivery order. This
document holds the exact types, functions, call sites, tests and measurements for the
first four steps. Where the two disagree, this document is wrong.

Base: master `1e54624a`, worktree `U:/Git/al-sem-issue-160-a2` (head `5eca5bb5`: no
`src/`, `tests/` or `crates/` change since `1e54624a`; the first attempt's worktree,
`…-a1`, is gone). Every `file:line` is in that tree and was read before it was cited. Section numbers are the ones the epic cites
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
   (`src/engine/l5/digest_cli.rs:534-560` against `:1655-1660`), prove's
   (`src/engine/l5/prove.rs:838-859` against `:902`) and events' (`src/engine/gate/events.rs:422-426`,
   `:567-571` against `:406`) are deleted.

Steps 0, 1, 1b, 2 are byte-neutral (epic I2): no golden target moves, no `--lib` test
outcome moves except the tests §7 names (each deleted or rewritten together with the
code it pins), the CDO `aldump` histogram SHA is unchanged. The stderr and exit-code
changes are listed in §7, by step.

## 2. Non-goals

- No `ExecRelation` (step 4), no cone work (7a, 7b), no identity work (10, 11), no
  snapshot on `Transfer::Cone` (6), no batch (12). Step 2 keeps today's cross-app base
  (`R3a5CrossAppBase`, `src/engine/l4/capability_cone.rs:3329-3370`) as the thing a
  `View` holds; `rel` replaces it at step 4.
- No witness scope, no `ViewTree`: digest, prove and fingerprint keep composing today's
  snapshot (`src/engine/l5/snapshot.rs:1150-1156`), only later in the command (§4.4.4).
- No persistent cache; `InputSet` is a value, not a key (epic §2).
- No change to the LSP binary's build path: `SnapshotBuilder`'s struct and its
  existing `build*` methods keep their shape (§4.2.6).
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
| A0.1 | Every timer in §4.1.1 exists and is zero-cost off | `cargo test -p al-sem --lib cones_census`: with the census off, `start()` returns `None` (existing contract, `detector_context.rs:454-456`); every new counter goes through `add_gated()` or a local fold, and the `COMPOSE_REST` snapshot is gated on `enabled()` (§4.1.1); the use-level check: CDO `context.capability_cones` wall, census OFF, head median ≤ parent median + band (§6.2); CDO SARIF byte-identical census on vs off (§6.2) | 0 |
| A0.2 | Every build counter in §4.1.2 reads from the trace | `tests/cli/cli_build_counters.rs`: per command and universe, the counter table of §4.1.3 exactly | 0 |
| A0.3 | A duplicate build fails A0.2 | one discrimination per counter (§8.1) | 0 |
| A0.4 | Both peaks are reported (Windows only: `read_rss()` returns `None` elsewhere, `perf_trace.rs:173-176`, so both counters are 0 there) | the trace's last `peak.process_mb` and `peak.post_attach_mb` counters; the stderr mirror prints both; `phase_peak_is_reported` is `#[cfg(windows)]` (CI runs on `ubuntu-latest`, `.github/workflows/ci.yml:22`; every measurement in §6 runs on the Windows dev machine) | 0 |
| A0.5 | The byte census covers §4.1.5's buckets and changes no output | CDO SARIF byte-identical census on vs off; the census prints a per-bucket line tagged `exact`/`shallow`; one `census-probe retained` run on CDO records the live heap at each bucket mark, and the numbers file states per mark live, Σ buckets and the residual, with Σ ≤ live at every mark (§4.1.5) | 0 |
| A0.6 | The full-graph timings exist | `aldump --issue160-full-graph-timings <ws>` prints the four numbers of §4.1.6 | 0 |
| A0.7 | Medians and bands are recorded per timer | the numbers file §4.1.7, one row per timer, ≥ 3 interleaved runs | 0 |
| A1.1 | `InputSet` is read once per workspace | the `inputs.read` counter (§4.1.2) is 1 per command in the trace (A0.2's reader), and 2 for `diff ws ws` (one `InputSet` per side, §4.2.6) | 1 |
| A1.2 | No reader touches the disk after the build | one disk-edit-after-read test per input class (§4.2.3, §8.2), including a `.al` file ADDED after the read (`inputs::al_file_added_after_read`: today `select_program_files` walks again and fails closed on it, `workspace.rs:1644`, `:1657`) | 1 |
| A1.3 | The second parse is gone | `workspace_diagnostics.rs` no longer calls `al_syntax::parse` (the index rows come from the build's `files_without_object`, §4.2.4); `inputs::objectless_file_removed_after_read` (§8.2) is the executable pin: the row survives the file's deletion, so it came from the build, not from a walk. The one corner whose bytes move is stated in §7 | 1 |
| A1.4 | One writer, one failure rule | closed-pipe test: `analyze` exits 2 with `al-sem: write error: stdout: <OS message>`; `digest --out` into an unwritable path exits 1 with the `<path>` line (§4.2.5, §8.2) | 1 |
| A1.5 | Byte-neutral | 8 golden targets + `cargo test -p al-sem --lib`; CDO histogram SHA unchanged | 1 |
| A1.6 | `--update-baseline` keeps today's output | `baseline::update_baseline_applies_the_set_just_written`: stdout and exit code equal today's (every current finding dropped); a malformed old file is overwritten, never read (§4.2.3, §8.2) | 1 |
| A1.7 | A failed `.al` walk still fails closed, with today's whole error text | `inputs::walk_error_fails_the_build`, two cases (a link loop; an unreadable subfolder, by the §8 recipe: `chmod` on Unix, a deny ACE on Windows): the build `Err` text equals the FULL literal today's code prints on the same input (the OS text twice in the subfolder case; one `cfg`-gated literal per platform, since the OS text differs, §8.2), `model_instance_id == None`, and the diagnostics equal today's (§4.2.1, §8.2) | 1 |
| A1b.1 | `alsem graph` output unchanged | `tests/cli-query-goldens/graph.test-selection-graph.jsonl` byte golden (`tests/cli/cli_graph_export.rs:702-716`) | 1b |
| A1b.2 | The body is released at today's point | the `b3.ctx_drop` span is still where it is (`program_calls.rs:674-676`) and A1b.4's post-attach peak is flat; no unit test (`attach_program_calls` consumes the program, so a read after the release does not compile; nothing at run time is left to assert, see §8.3) | 1b |
| A1b.3 | The two edge sets reach the projection in today's order | `project::add_keeps_first_via_on_conf_tie` + `resolved::all_edges_is_edges_then_dependency_bodies` (the helpers) + `project::cross_set_tie_keeps_the_first_sets_via` (the use: a hand-stated tie across the two sets through `project`, which reads them only through `EdgeView::all_edges`, §8.3); dropping `dependency_body_edges`, or only the DEPENDENCY `site_facts` (the merge `full.rs:2177` does today), from the projection input → `graph_export_covers_dependencies` fails (`cli_graph_export.rs:597-650`; extended if needed, §8.3) | 1b |
| A1b.4 | No new peak | both peaks ≤ parent + band on CDO analyze and digest (§6.3) | 1b |
| A1b.5 | `InputSet` bytes are in the census | the `inputs` bucket prints a number (5.09 MB expected on CDO) | 1b |
| A2.1 | One solve per command; one context per command (policy: two, its RAW build is the stated second, §4.4.4) | A0.2's table at its §4.4.8 values | 2 |
| A2.2 | Byte-neutral | 8 golden targets + lib; CDO detector baseline identical (`cdo-measure`) | 2 |
| A2.3 | digest is faster | CDO digest wall saving (parent median − head median) ≥ parent median of `digest.tx_context_map` − band(`digest.tx_context_map`) − band(digest wall) (§6.3; the span is added at step 0, §4.1.1; relative per epic I6, never the P5 "21 s"). The left side is a difference of two whole-command walls, so the whole-command wall's own noise band is subtracted too; the head has no matching span to measure against | 2 |
| A2.4 | No new peak | both peaks ≤ parent + band on CDO analyze, digest (cross-app) and prove, and on `fingerprint` and `digest --single-app` (single-app: the view builds and owns the graph, so a missed `release(Graph)` shows only there; prove and digest: `l4` now lives through snapshot composition, §4.4.4) | 2 |
| A2.5 | Output never depends on fill state | `RunKey` carries the mask; the superset-mask parity test compares the whole `DetectorOutput` (§4.4.6) | 2 |
| A2.6 | prove's `Some(root)` moves nothing | cli-b prove goldens unchanged; they are the pin (§8.4) | 2 |
| A2.7 | The failure classification keeps one build | `build.program` count on the no-id and build-failed rows is 1; the no-id `Err` text and the build-failed row's `ProgramBuildFailed` line are pinned (§4.4.5, §8.4) | 2 |
| A2.8 | The cell census sizes the det and l4 cells (amended in attempt 5, plan A4) | `context.*` buckets print when the det cell is built, and `l4.*` buckets when the l4 cell is filled (query included). A structure shared with the cross-app base is counted once, by the base's bucket. `retained_census_changes_no_output` requires a `bucket=l4.` line | 2 |
| A2.9 | No lazy fill anywhere in `View` | `session::no_lazy_fill_in_this_file` (`--lib`): over `include_str!("session.rs")` with every line whose trimmed text starts with `//` removed (so a doc comment that names a type, such as rule 4's own wording, does not fail it), no line contains `OnceLock`, `OnceCell`, `LazyLock`, `Lazy<`, `Cell<`, `Mutex`, `RwLock`, `Atomic`, `thread_local` or `static mut` (the needles are spelled with `concat!`, e.g. `concat!("Once", "Lock")`, so the test's own text does not match); stated limit in §4.4.3 rule 4 (a type from another module); `every_detector_alone_declares_what_it_reads` stays green (`registry.rs:998-1016`). Not a `scripts/ci-steps` grep: the issue flow may not edit `scripts/` (CLAUDE.md:873) | 2 |
| A2.10 | What a post-run projection reads off `det` is in the plan | `session::after_run_bits_must_be_in_the_union`: `prepare` on a `Plan { after_run: TRANSACTION_SPANS }` whose union mask lacks the bit panics, naming the bit (§4.4.3, §8.4); AND the factories: `session::command_plans_declare_after_run` calls the production `digest_plan` and `events_plan` (§4.4.4) with a detector set that lacks d9 and d1, and `prepare` panics naming `TRANSACTION_SPANS` / `SUMMARIES`; AND the use, on the command path: the `--lib` tests `digest_cli::tx_context_without_d9`, `prove::tx_context_without_d9` and `events::{fanout,chains}_without_d1_d9` (one per command; unit tests, because the inner forms are `pub(crate)`) run the commands' inner forms (`digest_pipeline_with`, `prove_pipeline_with`, `events_fanout_with` / `events_chains_with`, §4.4.4) with a reduced default set whose union lacks the bit (asserted first) and expect `prepare`'s panic naming it; a command that builds a plan without `after_run` completes instead (§8.4); digest/prove declare `TRANSACTION_SPANS` (the tx map), events `SUMMARIES` (the fanout's coverage read) | 2 |

## 4. Design

### 4.1 Step 0 — the measurement harness

Everything here is diagnostic. The rule of `perf_trace` and `cones_census` holds: with
the flag off, a probe is one `OnceLock` read and nothing else (`src/engine/perf_trace.rs:9-12`,
`src/engine/l5/detector_context.rs:449-463`). A probe that costs production time gets
deleted for the wrong reason.

#### 4.1.1 Timers

All in `cones_census` (`detector_context.rs:344-559`), gated by `ALSEM_CONES_CENSUS=1`,
read through the existing `start()`/`add_since()`/`add_gated()` helpers. A renamed counter is
a new counter: the old name is deleted, never left reading zero (perf-probe law 2).

**The gating rule for every new counter.** `add()` (`detector_context.rs:431-433`) is
UNGATED: it always does a `fetch_add`. `add_gated()` (`:439-443`) exists because an
ungated per-entry `add` once inflated compose from about 11.0 s to 12.8 s. So:

- A count inside a per-entry or per-pick loop (`SGL_CONE_ENTRIES`, `SGL_OWN_ENTRIES`,
  `SGL_OPEN_ENTRIES`, `BFS_PICKS`) is folded into a local `usize` in the loop, the way
  today's `c_entries` is (`capability_cone.rs:1882-1885`), and charged ONCE per routine
  with `add_gated()`. No atomic is touched per entry.
- Every new per-routine or per-call charge (`BFS_PICKS`, `PD_SOLVE_CALLS`, the three
  `SGL_*_ENTRIES`) uses `add_gated()`, never `add()`. Timers use `start()`/`add_since()`,
  which are already gated (`:454-463`).
- `COMPOSE_REST_NANOS`' entry snapshot (eight atomic loads and an `Instant`) and its
  exit charge run only under `if cones_census::enabled()`. With the census off,
  `compose_inherited_cones` does one `OnceLock` read for it and nothing else.

A0.1 checks this at the USE, not only at the helper: the step-0 head's CDO
`context.capability_cones` wall with the census OFF is within the parent's band
(§6.2). A probe that slows production would skew every later bound that reads step 0's
medians (steps 7a, 7b, 10, 11).

| Timer / counter | Where it is taken | What it answers |
|---|---|---|
| `BFS_WALK_NANOS`, `BFS_FOLD_NANOS`, `BFS_PICKS` (replace `BFS_NANOS`) | `capability_cone.rs:2552-2561`: walk = `inherited_facts_by_bfs` (`:2553-2560`), fold = `emit_picks` (`:2561`); `BFS_PICKS` += `picks.len()` before the fold | G-3: how much of the 10.0–10.5 s is the walk 7a removes and how much is the per-pick fold only `fold_row` (7b) removes |
| `SGL_SCAN_CONE_NANOS`, `SGL_SCAN_OWN_NANOS`, `SGL_SUBST_NANOS` (replace `SGL_SCAN_NANOS`); `SGL_CONE_ENTRIES`, `SGL_OWN_ENTRIES`, `SGL_OPEN_ENTRIES` (replace `SGL_CONE_ENTRIES`) | `capability_cone.rs:1776-1798` (`subst`: the frame-bound entries carried per edge), `:1812-1852` (the cone scan; count closed entries and open entries from `edge_subst` apart), `:1856-1873` (the own-entry loop) | the singleton cost per entry class: closed cone entries (7b's memo target), open (frame-bound) entries (stay on `substitute_entry`), own entries (G-1's population) |
| `PD_SOLVE_NANOS`, `PD_SOLVE_CALLS` | around `super::pd_solve::solve(..)` at `capability_cone.rs:2447` | G-1: the solve's share of the untimed 4.0–4.8 s compose remainder; a second solve is a second call |
| `PD_SOLVE_BYTES_ARENA`, `PD_SOLVE_BYTES_WORKING` (replace `PD_SOLVE_BYTES`) | `pd_solve.rs:465-500`: `solve_bytes` splits into `arena_bytes(arena)` (producers, states, links, edges, `by_routine`: what step 6 may retain) and `working_bytes(resolved, names, reps)` (the resolved own entries, the interned names, the reps: what step 6 decides to retain or re-solve). Today `PD_SOLVE_BYTES` has a SECOND charge site: `derive_pd_arena` adds each snapshot `PdArena`'s bytes to it, next to `PD_SNAPSHOT_BYTES` (`src/engine/l5/snapshot.rs:1602-1606`; the counter's doc says so, `detector_context.rs:411-415`). After the split that site charges `PD_SNAPSHOT_BYTES` alone, never `_ARENA` (that would count the retained arena twice) or `_WORKING`. So today's `solve_bytes=` value is `arena + working + snapshot_bytes`; the report prints all three, and the numbers file states that sum next to the old figure, so the epic's 525 MiB figure (epic §3) keeps its meaning | G-1: the two budgets step 6 chooses between |
| `COMPOSE_REST_NANOS` (an `AtomicI64`, the one SIGNED census counter; the others stay unsigned, `detector_context.rs:348-424`) | `compose_inherited_cones`, charged PER CALL: at entry read the eight component atomics (`FACTCONE`, `COVERAGE`, `BFS_WALK`, `BFS_FOLD`, `SINGLETON`, `DERIVED`, `EMIT`, `PD_SOLVE`) into locals; at exit add `elapsed as i64 − Σ(component now − component at entry) as i64`, computed in `i64` (no saturating or wrapping `u64` arithmetic), and `report()` prints it signed. The atomics accumulate over the process (digest composes three times, §4.1.3), so a whole-process subtraction would be meaningless; the per-call delta is not | what is still unexplained after the split (the report prints it; a negative value is a timer overlap bug) |
| span `digest.tx_context_map` (and `prove.tx_context_map`) | around the `compute_tx_context_map` call (`digest_cli.rs:1616`; `prove.rs:958`): the whole duplicate context build step 2 deletes | A2.3's bound: the saving is measured against the parent's median of this span, not an absolute figure (epic I6). Step 2 deletes the call, and the span with it |
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
| `build.snapshot` | `compose_snapshot_from_base` entry (`src/engine/l5/snapshot.rs:1161`) |
| `build.ordering_facts` | `compute_ordering_facts` entry (`src/engine/l5/ordering_facts.rs:218`) |
| `inputs.read` (step 1) | `InputSet::read` entry (§4.2.2) |

After step 2 two sites move with the code they count: `build.detector_context` sits at
`build_detector_artifacts`' entry (§4.4.2; the shim calls it, so a test or `aldump`
counts it too, which is what A0.2's reader wants), and `detector_context.rs:1303`'s
`build.graph_scc` sits at the single-app Tarjan call inside `build_l4` (§4.4.1). `build_l4`
is called from inside `build_detector_artifacts` (§4.4.2), but that nesting counts no
counter twice: `build.detector_context` counts the det build and `build.graph_scc`
counts the one top-level Tarjan, never the solver's inner one, so Decision 21 holds. The deleted
sites (`effect_query_cli.rs:237`, the context's own graph) are deleted with their code.
A2.1's table is the check.

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
| analyze, cross-app, `--detector d47-…` | 1 | 4 | 1 | 1 | 3 | 1 | 1 | 1 | d47 requires `ORDERING_FACTS` alone (`detectors/mod.rs:1575`) and the runner folds exactly that (`registry.rs:454-461`), so the context builds its graph (`detector_context.rs:940`) but no cone (`need_summaries`, `:914`), no v2 solve and no Tarjan (`:1296`). The cone, v2, two of the three SCCs (`capability_cone.rs:2783`, `:3198`) and `pd_solve` are the ordering cell's own snapshot (`digest.rs:4016-4019`); the third SCC is the base's (`:3477`) |
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
- Windows only. `read_rss()` is `None` on every other platform (`perf_trace.rs:173-176`,
  `#[cfg(not(windows))]`), so there both counters are emitted as 0. No Linux reader is
  added: CI (`ubuntu-latest`, `.github/workflows/ci.yml:22`) asserts no peak, and every
  §6 measurement runs on the Windows dev machine. `phase_peak_is_reported` is
  `#[cfg(windows)]` (§8.1).
- A sample interval of 10 ms misses spikes shorter than that. The band absorbs what it
  misses; the instrument is the same on both sides of every comparison (§6.1).
- A second `mark_phase_start` in the same process (diff attaches once per side,
  `diff/cli.rs:103` → `program_calls.rs:727`) ends the running phase and starts a new
  one; `peak.post_attach_mb` is the maximum over every phase. diff calls
  `pt::end_phase` when a side's model is dropped, so the other side's parse and attach
  are in no phase. (Code-review CR2 replaced the first design, which reset the maximum
  and reported only the last phase.) Only an open `pt::run` guard reports the peak, so
  without one `mark_phase_start` starts no sampler (CR10); the start, end and final read
  run under one lock (CR5); an `ALSEM_TRACE_EXIT_AFTER` exit reports both peaks too (CR6).

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
`len`, shared allocations once, allocator overhead excluded. The exception is the
producer-solve buckets: `arena.link_graph` and `arena.working` (via
`pd_solve::arena_tally` and `working_tally`), and `snapshot.pd_solution` (via
`pd_arena_bytes`). They count every `Vec`, `String` and hash table at capacity, with one
control byte per slot. That keeps the accounting of the pre-#160 `solve_bytes` figure
(525 MiB on CDO, epic §3), so step 0's numbers stay comparable with it (final panel
r2-r3, F3-2, F4-1). One `census.rs` module
under `src/engine/` with one `fn bucket(name, bytes, allocs, kind)` printer; each site
walks its own structure.

**Buckets are lower bounds; the total comes from the counting allocator.** `alsem`
runs on mimalloc (`src/bin/alsem.rs:28`), and no in-process live-heap total exists, so
the binary cannot compute a residual. So:
- Each bucket line carries `kind=exact` (the walk reaches every heap allocation of the
  structure: the source texts, the per-fact cone accounting `cone_census.rs:86-190`, the
  `pd_solve` byte functions) or `kind=shallow` (top-level containers by `len ×
  size_of`, plus the `String`/`Vec` heap one level down, and nothing deeper). No new
  heap-size trait is written for the IR or graph types.
- The ground-truth totals come from the tool the repo already has, `tools/census-probe`
  (a counting `GlobalAlloc` over `System`, `tools/census-probe/src/main.rs:42`, `:232`,
  fed by `census_hook::mark`, `src/census_hook.rs:11-19`; one atomic load per mark in
  production). Step 0 adds one `census_hook::mark` at each bucket point of the table
  below, taken BEFORE that point's walk (so the walk's own allocations are never in
  the total), and adds a probe mode, `census-probe retained <ws>`, that drives the
  cross-app analyze path (`build_cross_app_base`, the context build, `compose_snapshot`)
  with `ALSEM_RETAINED_CENSUS=1`. It is its own mode, apart from `model` (`q6`, `:1066`),
  so the marks never skew `model`'s phase peaks (code-review CR1). One `retained` run
  prints both the bucket lines and the live heap at every mark. The probe counts requested bytes, so its total does not
  depend on the allocator.
- `program.parsed_ir` and `program.graph` have no walker: their sizes are the probe's
  live-heap deltas at the existing marks `2.parse`, `3.dep_layer` and `4.assemble_graph`
  (`full.rs:1810`, `:1831`, `:1850`, inside `build_context_from_snapshot_cached`, which
  the analyze build reaches through `fresh_program_from_snapshot_with`, `:2108`). The
  command path has no mark between the snapshot and the parse: `1.snapshot` is set only
  in `build_context_with` (`full.rs:1738`), not in `build_program_with_coverage_profiled`
  (`:2136-2152`). So step 0 adds `census_hook::mark("1.snapshot")` there, right after the
  `preflight.snapshot_build` block (`:2140-2148`) and before
  `fresh_program_from_snapshot_with` (`:2149`). `program.parsed_ir` is then the growth
  from `1.snapshot` to `2.parse`. The snapshot (its texts and the dependency ABI,
  `AppUnit.abi`, `snapshot.rs:51-52`) is all allocated before `1.snapshot`, so it is not
  in the delta and nothing is subtracted.
- **Who owns the workspace texts.** The snapshot's workspace `SourceFile.text` and
  `InputSet.al_files[].text` are the SAME `Arc<str>` from step 1 on (`embedded.rs:19`,
  §4.2.3 row 1). At step 0 there is no `InputSet`, and `program.snapshot_texts` walks
  every unit. From step 1 on, the `inputs` bucket owns the workspace texts and
  `program.snapshot_texts` walks the DEPENDENCY units only, so Σ never counts the
  ~5 MB twice. The numbers file says which rule each step's line used.
- The numbers file, not the binary, computes per mark: probe live heap, Σ of the buckets
  alive there, and `residual = live − Σ`. Σ ≤ live at every mark is the check that can
  fail (a double count, or a bucket that counts capacity it does not own).
- `tools/` is outside the protected paths and outside the repo workspace and CI
  (`tools/census-probe/Cargo.toml`, own `[workspace]`). Step 1 changes three signatures
  `q6` calls (`build_program_with_coverage`, `compute_gate_model_instance_id`,
  `assemble_and_resolve_workspace_program`, `:1071`, `:1076`, `:1082`); step 1 updates
  `q6` in the same commit.

| Bucket | Walked where | What it holds |
|---|---|---|
| `program.snapshot_texts` | just before `b3.report_drop` (`program_calls.rs:670`) | step 0: every `SourceFile.text` of every app in `ctx.snapshot()` (`src/snapshot/snapshot.rs:34-69`); from step 1: the dependency apps' texts only (the workspace texts are the `inputs` bucket's, see above) |
| `program.parsed_ir` | no walker: the probe's live-heap growth from the new mark `1.snapshot` to mark `2.parse` | the workspace unit's and the dependency units' `ParsedFile.file` (`src/snapshot/parse.rs:17-29`) |
| `program.graph` | no walker: the probe's live-heap growth from mark `3.dep_layer` to mark `4.assemble_graph` | `ctx.graph()` (`full.rs:1538-1540`) |
| `program.report` | same point | `report.edges`, `site_facts`, `parenless_calls`, the histograms |
| `inputs` (step 1) | same point | the `InputSet` (§4.2.1), including every `al_files[].text`: this bucket owns the workspace texts the snapshot shares by `Arc` |
| `model` | end of `attach_program_calls_with` (`:689`) | `Model` (`src/program/model/workspace.rs:1890-1914`): rows, `calls`, `events` |
| `base` | end of `build_cross_app_base` (`capability_cone.rs:3565-3585`) | per field of `R3a5CrossAppBase` (`:3329-3370`): `graph`, `direct_full`, `leaf_summaries`, `injected_typed_edges`, … |
| `context.<field>` | end of `build_detector_context_with` (`detector_context.rs:1564-1616`), replacing `emit_full_census` (`:1161`) | one line per field of `DetectorContext` |
| `snapshot.<field>` | end of `compose_snapshot_from_base` (`snapshot.rs:1161-1190`) | one line per field of `CapabilitySnapshot` (`:881-899`): this is the `ViewTree`-equivalent G-2 asked for (`capability_facts`, `typed_edges`, `operation_index`, `callsite_index`, `coverage`, …) and the derived `pd_solution` |
| `arena` | `pd_solve::census` (`pd_solve.rs:435-457`) | the two halves of §4.1.1 |
| `context.<field>`, `l4.<field>` (step 2; amended in attempt 5, plan A4) | `context.*` when the det cell is built; `l4.*` when `build_l4` fills an `L4Analysis` | the owned artifacts (`DetectorArtifacts`) and the solve (`l4.bundle`, `l4.scc`, `l4.summarize_diagnostics`, `l4.warnings`); an `Arc` shared with the cross-app base (its graph, its SCC) is counted by `base.*` only, with no alias line |

A walk over 15 M cone entries is not free; it runs only under the flag, and A0.5 proves
the flag changes no byte of output.

#### 4.1.6 Full-graph timings for step 6's stop rule

Step 6 moves the snapshot from the cut graph (`resolved.calls` as the base cuts it,
`capability_cone.rs:3457-3467`, `:3564`) to the full one. Its stop rule (Decision 52)
needs today's cost of the snapshot over the FULL graph, measured before any step 6 code
exists. `aldump --issue160-full-graph-timings <ws>`:

1. builds the cross-app base;
2. builds the snapshot's base with ONE thing changed, the graph. Step 0 splits
   `build_r3a3_source_only_base` (`capability_cone.rs:4015-4101`) into
   `build_r3a3_source_only_base_over(resolved: &Model, graph: CombinedGraph) ->
   R3a3SourceBase`, which is today's body with the graph taken as a parameter (calls and
   event graph still cloned from `resolved`, `:4019-4020`; the fold from `:4037` on),
   and the old function, which builds its graph (`:4021`) and calls it, so every
   existing caller and golden sees the same bytes. The aldump mode calls
   `build_r3a3_source_only_base_over(&base.resolved, base.graph.clone())`: at step 0
   `R3a5CrossAppBase.graph` is a plain `CombinedGraph` (`capability_cone.rs:3335`).
   Step 2 makes it `Arc<CombinedGraph>` (§4.4.3), and the same commit changes this one
   call to `(*base.graph).clone()`. So:
   - **graph**: `base.graph`, the cut calls plus the injected dependency edges
     (`:3470`, `:3513-3515`): the graph the context's cone walks, which is the relation
     step 6 moves the snapshot onto (`Transfer::Cone`);
   - **calls** (`R3a3SourceBase.calls`, the callsite ledger): `(*base.resolved.calls).clone()`,
     the CUT set (`:3456-3467`, `:3564`), which is exactly what today's cross-app snapshot
     reads, since it composes from `base.resolved`. The full set before the cut is not
     in the base, and it is not needed: the injected edges are how the dependency
     callers reach the graph;
   - **direct facts and coverage**: computed by the snapshot's own code over that graph
     and those calls: `direct_facts_for_routine` plus the uncertainty fold
     (`compute_uncertainty_coverage_reasons`, `:4036-4055`). NOT `base.direct_coverage`,
     which is unfolded (`:3540-3548`). So the snapshot's coverage rule is today's, only
     the graph moves;
   - **cone**: the `compose_cone_over_graph(.., ConeOutput::RawOnly)` call inside it
     (`:4082`), now over `base.graph`;
3. runs `compose_snapshot_from_base` on that base;
4. prints: the cone's wall (the `r3a3.compose_cone` span), `PD_SOLVE_NANOS` (G-1: the
   solve is inside the cone's wall, and is printed on its own), the snapshot
   derivation's wall, and the raw population (inherited facts summed, bytes by the
   census). The same four numbers over today's cut graph come from the same mode run
   through the unsplit `build_r3a3_source_only_base(&base.resolved)`, so the numbers file
   puts cut and full side by side from one binary.

This is a measurement-only path: it ships behind the aldump flag, is not called by any
command, and is deleted when step 6 lands.

#### 4.1.7 The numbers file

`docs/measurements/2026-10-<dd>-issue-160-step0.md`. Sections: machine and binary
(commit, profile `release-fast`, `RAYON_NUM_THREADS`); per command and universe the
§4.1.3 counts as read from the trace; per timer of §4.1.1 and per span the epic's
bounds read (`context.capability_cones`, `b3.adapter`, `b3.adapter.join`,
`b3.adapter.by_decl`, `analyze.total`, digest's wall, `crossapp.attach_program_calls`)
the median and the band over ≥ 3 interleaved runs (§6.1). A `cones_census` timer is a
process-cumulative atomic (`detector_context.rs:348-424`). Today `report()` (`:465-558`)
has ONE call site, at the end of the detector context's cone block
(`detector_context.rs:1150`). The snapshot's cone (`compose_cone_over_graph`,
`capability_cone.rs:2764-2906`; `build_r3a3_source_only_base`, `:4015-4101`) never
prints. So today digest's last line covers all three cones only because its snapshot is
composed first (`digest_cli.rs:1580`). After step 2 the snapshot is composed after `det`,
and its cone and `pd_solve` would reach no printed line; `fingerprint` already loses its
snapshot's cone today, and `diff` (ctx 0) prints nothing. **So step 0 moves the print to
process exit:** the call at `detector_context.rs:1150` is deleted, and `alsem`'s `main`
and `aldump`'s `main` each call it once, after the command returns (gated, so with the
census off it is one `OnceLock` read). The binaries are separate crates under `src/bin/`
and `cones_census` is `pub(crate)` (`detector_context.rs:344`), so step 0 adds one public
wrapper beside it, `pub fn report_cones_census()` in `detector_context` (which is `pub`,
`src/engine/l5/mod.rs:54`), and the module stays crate-private. The one early exit,
`perf_trace::maybe_exit_after` (`src/engine/perf_trace.rs:622-629`, `process::exit(0)`
under `ALSEM_TRACE_EXIT_AFTER`), skips `main`'s return, so it calls
`cones_census::report()` just before its `exit` (same crate). Neither binary has another
`process::exit` (`grep` over `src/`). The numbers file quotes that
one line: the process total. That is the number a later step is compared on (digest's
total today sums its three cone builds; after step 2 it sums two, and the difference is
the deleted context's cone). The counters are never
reset (epic §5.9 rule 4, as amended: a reset would make the step 0 and step 2 lines
measure different things). Both peaks per command;
the census buckets; the §4.1.6 numbers; the existing ordering counters
(`ordering.roots_all`, `ordering.roots_kept`, `digest.rs:4060-4062`) on a d47 run; and
the SCC distribution, read from the trace's existing `scc_stats` instant event
(`detector_context.rs:1314-1355`: largest SCC, recursive SCC count and members, the
largest SCC's intra-edge anatomy) on CDO analyze, cross-app. The epic's 7a row uses
"step 0's SCC distribution" as its stop rule (epic §9), so it is recorded here. After
step 2 the event moves into `build_l4` and fires on BOTH arms: on the SCC it computes
(single-app) and on the base's `Arc` it is passed (cross-app), so the step-2 numbers
file records the same event.
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
    /// `None` when the command takes none, under `--no-policy`, or when auto-detect
    /// finds no file. Read here, VALIDATED where it is today. The rules are
    /// `resolve_policy_check`'s (`pipeline.rs:47-80`), unchanged: `--policy p` reads
    /// `absolutize(p)` (`:56-57`); auto-detect takes the FIRST of
    /// `al-sem.policy.yaml`, `.yml` whose `workspace.join(name).exists()` and reads
    /// `absolutize` of it (`:72-75`). An unreadable `.yaml` is stored as `Err`; it
    /// does not fall through to `.yml`. `FileInput.path` is that ABSOLUTIZED path.
    pub policy_file: Option<PolicyInput>,
    /// The `PolicyRequest` the set was read with (code review r1 CR5). `policy_file`
    /// is `None` both under `--no-policy` and when auto-detect finds no file, so
    /// `policy check` reads `--no-policy` from here and `PolicyCheckOptions` has no
    /// `no_policy` of its own; a set read with `PolicyRequest::None` is refused.
    pub policy_request: PolicyRequest,
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
    /// `Err` on an unreadable folder or a link loop (`:63-65`, `:85`).
    /// Then `al_files` is EMPTY and every reader fails closed as today: the build
    /// `Err`s with `walking source <root>: <chain>` (`provider.rs:34-35`),
    /// `model_instance_id` is `None` (`model_instance_id.rs:83`, `.ok()?`), the
    /// diagnostics return the provider rows alone (`workspace_diagnostics.rs:93-95`).
    /// A partial list must never reach the build as if it were the whole app.
    ///
    /// The slot keeps the `io::Error` itself, not its text. `:85` turns the walkdir
    /// error into a custom `io::Error` (walkdir-2.5.0 `error.rs:253-261`), whose
    /// `source()` is the inner OS error for an I/O failure (`:212-217`; `None` for
    /// a loop, `:215`), and `full.rs:2147` prints the whole chain with `{e:#}`. So
    /// an unreadable subfolder prints today `… walking source R: IO error for
    /// operation on P: <os>: <os>` (the OS text twice). A `String` from
    /// `e.to_string()` would lose the last link. The `Arc` is there because the
    /// set is shared (`Arc<InputSet>`) and `anyhow` needs an owned error: the build
    /// does `Err(anyhow::Error::new(Arc::clone(e))).context(..)`, and std's
    /// `impl Error for Arc<T>` forwards `Display` and `source()`, so the chain is
    /// today's, link for link.
    pub al_walk: Result<(), Arc<std::io::Error>>,
    /// Every `.al` file of the `NestedApps::Walk` list, sorted by `rel_posix`.
    pub al_files: Vec<WsFile>,
    /// `driver_version()` read once (`src/engine/gate/version.rs:37-39`).
    pub driver_version: String,
    /// `compute_gate_model_instance_id`'s value (`src/engine/gate/model_instance_id.rs:75-96`),
    /// computed from the fields above with the same algorithm; `None` = fail-closed.
    pub model_instance_id: Option<String>,
    /// `InputNeeds::build_only` as asked (see there).
    pub build_only: bool,
}
pub struct FileInput { pub path: PathBuf, pub bytes: Result<Vec<u8>, std::io::Error> }
impl FileInput {
    /// The bytes as UTF-8. Invalid UTF-8 is an `io::Error` of kind `InvalidData`
    /// with the text `stream did not contain valid UTF-8` — the error
    /// `std::fs::read_to_string` returns, byte for byte (std's `INVALID_UTF8`
    /// const). Every reader that used `read_to_string` today (`app.json`,
    /// `snapshot.rs:194`; the baseline, `baseline.rs:38`; the policy file,
    /// `load_policy_from_file`, `src/engine/gate/policy/policy_loader.rs:323-331`)
    /// reads through this, so `read <path>: stream did not contain valid UTF-8`,
    /// `cannot read baseline '<path>': stream did not contain valid UTF-8` and
    /// `failed to read <absolutized path>: <e>` keep their text and stay READ
    /// errors, never parse errors. The hash rows (`derive_inputs`) read `bytes`.
    pub fn text(&self) -> Result<&str, std::io::Error>;
}
pub enum PolicyOrigin { Explicit, Auto }   // the `explicit:` / `auto:` source label (`pipeline.rs:61`, `:79`)
pub struct PolicyInput { pub origin: PolicyOrigin, pub file: FileInput }
pub struct ClosureEntries { pub files: Vec<DiscoveredAppFile>, pub unreadable: Vec<(PathBuf, std::io::Error)> }
pub struct WsFile {
    pub rel_posix: String, pub abs_path: PathBuf,
    /// Under a child directory that holds its own `app.json`: the file the
    /// `NestedApps::Skip` walk would not list (`source_text.rs:70-75`).
    pub nested_app: bool,
    /// `Err(os message)`: the slot stays so the diagnostics can warn from it.
    pub text: Result<Arc<str>, String>,
}
pub struct InputNeeds {
    /// Only what the snapshot build itself reads today: `app.json`, the `.al` walk and
    /// texts, the closure. Set by `SnapshotBuilder::build*` (the LSP, `aldump`, tests,
    /// §4.2.6) and by no command. `read` then skips the `app.json` walk, the per-folder
    /// `nested_app` checks (every `nested_app` is `false`), the model-instance id
    /// (`None`), `roots.config.json`, the coverage yaml, policy, baseline and hash rows;
    /// those slots hold their empty values. `InputSet.build_only` records it, and
    /// `app_scoped_files()`, `roots_config_loaded()`, `compute_workspace_diagnostics`
    /// and `select_program_files` assert `!build_only` (release builds too), so a
    /// command can never read an empty slot as an answer.
    pub build_only: bool,
    pub fingerprint_rows: bool, pub policy: PolicyRequest, pub baseline: Option<PathBuf>,
}
impl Default for InputNeeds { /* build_only: false, fingerprint_rows: false, PolicyRequest::None, no baseline */ }
impl InputNeeds { pub fn build_only() -> InputNeeds; } // `build_only: true`, the rest default
pub enum PolicyRequest { None, AutoDetect, Path(PathBuf), Disabled }
impl InputSet {
    pub fn read(workspace: &Path, needs: &InputNeeds) -> Arc<InputSet>; // the ONE disk reader; charges counter `inputs.read` (§4.1.2). Returns the `Arc` every holder shares (`SnapshotBuilder`'s build, `ProgramMeta`): commands hold the `Arc` and pass `&Arc<InputSet>`, never a bare `&InputSet` (an `Arc` cannot be recovered from one without cloning the set)
    pub fn file(&self, rel_posix: &str) -> Option<&WsFile>;          // index built in `read`
    pub fn app_scoped_files(&self) -> impl Iterator<Item = &WsFile>; // `!nested_app`: today's `NestedApps::Skip` list
    pub fn app_id(&self) -> Option<&str>;                            // `app_json_value` `Some(Ok)` with a non-empty string `id`, verbatim
    pub fn roots_config_loaded(&self) -> bool;                       // validates the bytes, as `roots_config_was_loaded`
}
```

`read` never fails: every read error is stored in its slot, the walk's in `al_walk`.
The build decides what a missing or unreadable input means, exactly as today.

#### 4.2.2 Reading

`InputSet::read` does, in this order (a `build_only` read skips the steps §4.2.1 names):
`app.json` (one read), the `app.json` walk, the
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
| `SnapshotBuilder::build_inner` reads and parses `app.json`; `WorkspaceProvider` walks and reads every `.al` | `src/snapshot/snapshot.rs:193-197`, `:236-239`; `src/snapshot/provider.rs:34-56` | `build_inner` takes `inputs: &Arc<InputSet>` (the struct is unchanged, §4.2.6); `app_json.text()` `Err` (unreadable, or not UTF-8) fails with `read <path>: <io>`, `app_json_value` `Some(Err)` with `parse <path>: <serde>` (today's two texts, with today's split: invalid UTF-8 is a read error, `FileInput::text`); `al_walk` `Err` fails with `workspace source provider: walking source <root>: <chain>`, the chain built from the stored `io::Error` as §4.2.1 says, so `{e:#}` prints today's text link for link; the workspace `SourceRoot.files` are built from `al_files` with the SAME `Arc<str>` (no copy), keeping `walk_al_source`'s rules exactly (`provider.rs:33-58`): every `al_files` entry in `rel_posix` order, nested ones included (the `NestedApps::Walk` list); NO `.al` file gives `Ok(None)`, so the unit's `source` is `None` and its tier `TrustTier::SymbolOnly` (`snapshot.rs:236-244`), never an empty `Some(SourceRoot)`; `content_hash` is blake3 over the texts in that order (`provider.rs:48-53`). The new function replaces `walk_al_source` for the workspace provider only; a `WsFile.text` `Err` fails the build with today's message (`workspace source provider: reading source <abs>: <os>`), so an unreadable file still fails the program build |
| `compute_gate_model_instance_id(ws)` walks and reads | `model_instance_id.rs:75-96`; every command's first call | `inputs.model_instance_id`: `None` when `app_id()` is `None` or `al_walk` is `Err` (`:76`, `:83`) |
| `finish_resolved` reads `roots.config.json` and `app.json` | `src/program/model/workspace.rs:1767` (→ `root_classification.rs:450-463`), `:1775` (→ `:1789-1800`) | `compute_root_classifications(&ws, inputs.roots_config.as_ref())` validates from the bytes; `primary_app` from `inputs.app_json` |
| `compute_workspace_diagnostics(ws)` reads `app.json`, counts `app.json` files, walks, reads and PARSES every `.al` | `workspace_diagnostics.rs:44-47`, `:93-106`, `:119-129` | `compute_workspace_diagnostics(inputs, files_without_object: Option<&[String]>)`: the provider rows from `app_json_paths` / `app_id()` (readable = `app_json_value` is `Some(Ok)`, `:145-151`) / each `WsFile.text` `Err` (same texts, same order); `al_walk` `Err` returns after the provider rows (`:93-95`); the index rows from the bit (§4.2.4) |
| `project_coverage_disk(ws)` walks with `NestedApps::Skip` | `src/program/model/coverage.rs:298-323`; `run.rs:432` | `project_coverage_inputs(inputs)`: `app_scoped_files()` in the same sorted order. The source-grep guard `analyze_builds_through_the_dumped_functions` (`src/engine/switch_dump.rs:355-377`) bans `project_coverage_disk` in `run_analyze_with_exit`; after the rename that needle matches nothing, so the banned list gains `project_coverage_inputs` in the same commit (the old needle is replaced, never left guarding a deleted name) |
| `select_program_files(ws, ctx)` reads the root `app.json` id and walks with `NestedApps::Skip` DURING model assembly, after the program build; a `.al` file added between the build's walk and this one is `ProgramFiles::Missing` and the model fails closed (`"program parse lacks"`) | `src/program/model/workspace.rs:1636-1661` (`:1643` `read_root_app_guid`, `:1644` `discover_al_files_app_scoped`, `:1657` `Missing`); called at `:1671` from `assemble_l3_workspace_from_program`, which `assemble_and_resolve_workspace_from_program` reaches | `select_program_files(inputs, ctx)`: `inputs.app_id()` and `inputs.app_scoped_files()`. The build parsed exactly that list (same `InputSet`), so `Missing` can no longer come from a disk change; the arm stays as the fail-closed guard against a parse that dropped a file. `inputs::al_file_added_after_read` (§8.2) pins it. The `&InputSet` reaches it by replacing the `workspace: &Path` parameter along the whole chain: `assemble_l3_workspace_from_program` (`workspace.rs:1663`), `finish_resolved` (it reads roots config and `app.json`, row above), `assemble_and_resolve_workspace_from_program` (`:1431`) and `assemble_and_resolve_cross_app_from_program` (`:1457`) take `inputs: &InputSet` (the path is `inputs.workspace`); the names stay, so `analysis_model_uses_the_program_parse`'s needle (`switch_dump.rs:437-451`) still matches. Their test callers (`workspace.rs:2840`, `:2853`, `:2900`, `:2920`; `program_calls.rs:1890`; `switch_dump.rs:424`, §4.2.6) gain one `InputSet::read` line each |
| `build_analysis_model`'s single-app arm walks with `NestedApps::Skip` to tell `ProgramBuildFailed` from `AssemblyFailed` | `run.rs:312-313` | `inputs.app_scoped_files().next().is_some()`: the same rule, no walk. (The cross-app arm's rerun of this walk goes with the rerun, §4.4.5) |
| `build_suppression_map(ws_path, unit_ids)` reads each `.al` again | `src/engine/gate/inline_suppression.rs:197-209` (→ `:37-38`); `run.rs:615` | parses suppressions from `inputs.file(rel).text`; a slot with `Err` yields no suppressions (today: a read error yields none) |
| `read_workspace_apps(ws)` reads `app.json` for pr-summary | `run.rs:122-157`; `:648` | from `inputs.app_json` with the same defaults |
| `derive_inputs(ws)` hashes `app.json`, every closure `.app`, `roots.config.json`, the coverage yaml | `snapshot_full.rs:815-887`; `:63`, `:74` | `inputs.snapshot_inputs.as_ref().expect("fingerprint rows were not requested")`: the command declares the need (`fingerprint`, `digest`, `prove`, `diff`). The `app-json` row hashes `app_json.bytes` whenever they are `Ok`, parse or no parse (`:819-827` hashes the file, never the JSON) |
| `roots_config_was_loaded(ws)` re-reads and validates | `root_classification.rs:443-445`; `fingerprint_cli.rs:359`, `snapshot_full.rs:858` | `inputs.roots_config_loaded()` |
| `resolve_policy_check(ws, path, no_policy)` reads the policy file | `pipeline.rs:47-80`; `:230` | takes `inputs.policy_file` (`PolicyInput`: the absolutized path, the origin for the `explicit:`/`auto:` label, the exists-first rule already applied by `read`, §4.2.1); check's two `load_policy_from_file` calls (`:58`, `:76`) become `load_policy_from_input(&FileInput)`, which reads `text()` and keeps the `failed to read <path>: <e>` text (`policy_loader.rs:323-331`); `load_policy_from_file` stays for `policy explain` (`pipeline.rs:341-369`), which runs no semantic build and is not rewired; validation stays at `:230`, after the `--strict` check (`:219-226`), at step 1 AND at step 2 (the RAW build that needs the policy's presence is built after the run, where `:261` builds it today, §4.4.4), so no precedence changes. The reporting-order move is Decision 7, step 12 |
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
  `FreshCoverage` from step 1b on. The new field breaks the `FreshCoverage` struct
  literals that name every field (`grep "FreshCoverage {"`): the builder
  (`full.rs:2315`, which sets it) and two test literals in
  `src/engine/gate/preflight.rs`, `clean()` (`:133`) and the one in
  `all_signals_retained_in_fixed_order` (`:210`). Both tests gain
  `files_without_object: vec![]`, assertions unchanged. The other six literals there
  end in `..clean()` and need no edit.
- `Model` (`workspace.rs:1890-1914`) gains `workspace_diagnostics: Vec<Diagnostic>`,
  built in `attach_program_calls_with` (`program_calls.rs:677-685`) from an
  `inputs: &InputSet` PARAMETER and `coverage.files_without_object`, next to
  `dependency_diagnostics` (the #91 precedent). The InputSet is a parameter, NOT a
  field of `AppSetSnapshot`: that struct (`src/snapshot/snapshot.rs:61-68`: `apps`,
  `workspace_app`, `world`) derives `Clone, Debug` and is the LSP's retained snapshot
  (`src/lsp/snapshot.rs`), which would then hold an InputSet that goes stale across
  incremental updates (§2, §5). So `attach_program_calls` (`program_calls.rs:641-648`)
  and `attach_program_calls_with` (`:652-658`) each gain `inputs: &InputSet`. Their
  callers are `run.rs:357` (single-app arm), `program_calls.rs:727`
  (`assemble_and_resolve_workspace_program`) and the cross-app call at `:898`; each
  already holds the command's `&Arc<InputSet>` from step 1 (§4.2.6). No `tests/` file
  calls either (`grep attach_program_calls\(` over `tests/`: none). From step 1b the
  value passed is `&program.meta.inputs`. `analyzer_diagnostics_of_run`
  (`run.rs:1085-1117`) reads it as source (1) and loses its `ws_path` parameter. The
  source-grep guard `run_analyze_with_exit_uses_the_shared_aggregation`
  (`run.rs:1175-1210`) bans `compute_workspace_diagnostics(` inside
  `run_analyze_with_exit`. After this change an inline copy of source (1) would read the
  field, and that needle would not match it. So the same commit adds the needle
  `.workspace_diagnostics` to its banned list (the replace-the-needle rule of the
  `project_coverage_disk` row, §4.2.3). The old needle stays: the function still exists.
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
  (the `Could not read` warning included). That is the whole `alsem` byte change: a
  symlinked nested app, an unreadable file inside it, and no root `.al` file, in
  either universe (cross-app reruns the single-app build today, `run.rs:387`). No
  fixture has the shape and no golden carries it.
- The third caller is `aldump --switch-dump` (`src/engine/switch_dump.rs:47`, the
  `diagnostics.workspace` rows), which calls the function with no build gating today.
  After step 1 `dump_lines` reads one `InputSet`, runs the program build it already
  runs (`:51`, `build_program_with_coverage`) FIRST, and passes that build's
  `files_without_object` (`None` when it fails). On a failed build (an unreadable
  `.al` is enough, no symlink needed) the dump loses its `No object declaration
  found in <rel>` rows for the readable objectless files. No golden or test reads the
  dump (`grep switch_dump\|switch-dump` over `tests/`: none); stated in §7 as an
  `aldump` output change.

The new `Model` field breaks every `Model { .. }` struct literal (`grep
"dependency_diagnostics:"` outside the builder at `program_calls.rs:682`, cross-checked
with `grep "Model {"`): 12 literals in 8 files, one of them production. (The review that
found this counted 14 in 10; the grep lists the 12 below.)
- Production: `empty_output_result`'s `empty_resolved` (`run.rs:863`) sets
  `workspace_diagnostics: vec![]`. Its rows already come from
  `compute_workspace_diagnostics` in the same function (§4.2.4 bullet above); that empty
  model feeds only `format_html` (`run.rs:867-872`), as its `dependency_diagnostics:
  vec![]` does today. No output moves.
- Tests, each `workspace_diagnostics: vec![]` (or `Vec::new()`), assertions unchanged:
  `format_html.rs:1218`, `:1275`; `d1.rs:3194`, `:4277`; `d17.rs:526`; `d35.rs:289`;
  `d50.rs:811`; `detector_context.rs:1711` (deleted at step 2 with its test, §4.4.2),
  `:1752`; `registry.rs:1098`.
- `analyzer_diagnostics_of_run_keeps_every_source_in_order` (`run.rs:1129`, literal at
  `:1149`) sets `workspace_diagnostics` to a hand-stated row (a hand-stated
  precondition, CLAUDE.md), since it is source (1) of that function.
§7 step 1 lists them.

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
passed down as `&Arc<InputSet>` (§4.2.1): `run_analyze_with_exit(inputs: &Arc<InputSet>, args)`; `run_digest_pipeline(inputs, …)`;
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
take `&Arc<InputSet>`.

**Which `InputNeeds` the other callers use.** `InputNeeds::build_only()` is for
`SnapshotBuilder::build*` alone (§4.2.1). Every other caller reads
`InputSet::read(path, &InputNeeds::default())` on the line before its call, because the
model path after the build asserts `!build_only` (`select_program_files`, §4.2.1). The
callers of `build_program_with_coverage[_profiled]` outside the commands are:
- `aldump --compiler-oracle` (`src/bin/aldump.rs:139`);
- the `--lib` tests at `src/program/model/site_links.rs:110`,
  `src/program/physical.rs:128`, `src/program/registry.rs:198` and `:261`,
  `src/program/resolve/applicability.rs:955`, `src/program/dep_summary.rs:546`,
  `src/program/resolve/full.rs:4436`, `src/program/model/workspace.rs:2838`, `:2885` and
  `src/engine/switch_dump.rs:51`, `:414`;
- `tests/cli/cli_attribute_trivia.rs:463` and `tools/census-probe` (`:1071`, §4.1.5).

Each gains the one `InputSet::read` line. The path wrappers
`assemble_and_resolve_workspace_with_program_calls`, `assemble_and_resolve_inline_program`
and `assemble_and_resolve_inline_program_default` (`program_calls.rs:698-790`) keep their
`&Path` / inline-files signatures and do that read inside, with
`InputNeeds::default()`. So the 69 `tests/` files that call them (33 in `tests/gap`,
12 `r3`, 10 `r4`, 8 `temp_state`, 3 `cli`, 3 top-level; `grep -rl` over `tests/`) do
not change.

`SnapshotBuilder { workspace_root, local_providers }` (`src/snapshot/snapshot.rs:130-135`)
keeps its fields. It is built as a struct literal at 28 sites (`grep "SnapshotBuilder {"`:
`full.rs:1711`, `:1748`, `:2142`, `:4071`, `:4516`; `dep_cache.rs:512`, `:591`, `:625`;
`program/build.rs:1359`, `:1401`; `resolve/semantic_golden.rs:1407`; `snapshot/parse.rs:178`;
ten in `snapshot.rs`'s own tests; `tests/lsp/program_graph.rs:16`,
`tests/lsp/snapshot_robustness.rs:16`; four in `tests/program_resolve_harness.rs`), and
none of them changes. One method is added: `build_with_inputs(&self, inputs:
&Arc<InputSet>, ..)`, which asserts `inputs.workspace == self.workspace_root` and is
what `build_program_with_coverage_profiled` (`full.rs:2142`) calls. The existing
`build`, `build_with_diagnostics`, `build_with_options` and
`build_deferring_dependency_text` (`snapshot.rs:142-180`) keep their signatures and
become `build_with_inputs(&InputSet::read(&self.workspace_root, &InputNeeds::build_only()), ..)`:
one fresh read per build, so the LSP (`src/lsp/snapshot.rs:599-604`), `aldump` and the
test sites keep working unedited and still read the disk exactly once per build.
`build_only` (§4.2.1) is what keeps that read at today's cost. A full read would add
work the LSP never did: a second whole-tree walk (`count_app_json_paths`,
`source_text.rs:145-169`), one `read_dir` per folder for `nested_app`, a SHA-256 over every
unit id, and the `roots.config.json` / coverage-yaml reads. `build_context_with` can
build twice per call (`full.rs:1718-1734`), so it would pay that twice, on every LSP
rung-2 rebuild. With `build_only` each build reads what `walk_al_source` and
`build_inner` read today, no more. The measurement: step 1's numbers file records
`cargo bench --bench lsp_pipeline` `build_full` (100 and 1000 files) parent vs head;
`perf_bounds`' 3× bound stays the gate. `InputNeeds::default()` is NOT the build-only
set: a command states its needs, and `build_only` is `false` unless asked.

`switch_dump.rs:437-451` greps `build_analysis_model`'s source body for exactly one
`assemble_and_resolve_workspace_from_program(` call: the function keeps its name and
that shape. Two more `switch_dump.rs` `--lib` tests move with step 1:
`analyze_builds_through_the_dumped_functions` (`:355-377`) gets `project_coverage_inputs`
in its banned list in place of the renamed `project_coverage_disk` (§4.2.3), and
`analyze_model_selects_app_scoped_files_from_the_program_parse` (`:384-428`) writes its
files, then calls `InputSet::read(&root, &InputNeeds::default())`, and passes that `Arc`
to `build_program_with_coverage(&inputs)` and `select_program_files(&inputs, &ctx)`. Its
assertions are unchanged: the nested file is in the parse and not in the selection
(`app_scoped_files()` drops `nested_app` files).

The in-process golden harnesses (`tests/cli/cli_b_*`, `cli_a_*`, `cli_c_*`, every file
the grep in §8.2 lists) call the pipeline functions; each gains one `InputSet::read`
line. No golden moves.

### 4.3 Step 1b — `ResolvedProgram`

#### 4.3.1 Types (`src/program/resolved.rs`)

**Amended by code review r1 of the implementation (2026-10-19, CR1/2/5/6/8/9/12):** the
body is NOT an `Option`. A reader of the whole build takes `&ResolvedProgram`; the model
builder consumes it (`attach_program_calls(rows, program) -> (Model, ProgramMeta)`), so a
read after the release does not compile instead of panicking at a `.expect`. Steps 2 and
12 share one program the same way: the `graph` job borrows it, the model consumes it,
and the holder keeps the returned `ProgramMeta` (epic §5.9 rule 3's order). The build
takes a `ProgramScope` (`Workspace` = Summary, no dependency edges; `CrossApp` = FULL +
`resolve_dependency_bodies`) instead of a `BuildProfile`, so retention and meaning
cannot be chosen apart; `meta.scope` (private, read through `ProgramMeta::scope()`,
final panel F2-3) records it and `export_from_program` refuses a `Workspace` program.
`ResolvedProgram`'s `meta` and `body` are private too (`meta()`, `body()`, and the
crate-only `into_parts()` for the consumers that split a build), so no caller can pair
one build's meta with another's body. `ProgramMeta` loses `primary_app_ref` (the body's
`ctx.primary_app_ref` is the one source; `edge_view()` takes no argument) and
`loaded_apps` (`ResolvedProgram::loaded_apps()` computes it while the body is alive, for
the two readers that need it). `ProgramBody` loses `builtin_dispatch_audit` and
`event_flow_dual_publisher_alias_skips` (no reader; dropped in `build`), and its two edge
sets are private (`all_edges()`, plus `dependency_body_edges()` for the graph's
`dependencyBodies` histogram, which reads that slice; `EdgeView`'s are private too). The
block below is the original design, kept for the record.

```rust
pub struct ResolvedProgram { pub meta: ProgramMeta, pub body: Option<ProgramBody> }
pub struct ProgramMeta {           // only the fields a step 0-2 reader reads (§4.3.3)
    pub inputs: Arc<InputSet>,
    pub primary_app_ref: AppRef,
    pub recovered_files: Vec<String>,
    /// Carries `files_without_object` (§4.2.4).
    pub coverage: FreshCoverage,
    pub histogram: Histogram, pub primary_histogram: Histogram,   // over `edges` only (`full.rs:1403-1418`)
    pub coverage_contract: Coverage,                               // `report.coverage` (`full.rs:262-263`)
    pub loaded_apps: Vec<serde_json::Value>,                       // `full.rs:2190`
}
// Not in `ProgramMeta` before a step reads them: `profile`, the app set (`apps`,
// `workspace_app`) and `abi_integrity`, which the epic's §5.2 lists. No step 0-2 reader
// needs them (`graph` reads the app set through `body.ctx.snapshot()` while the body is
// alive, §4.3.3), and an `AppMeta` type defined with no reader would be a guess. Each
// joins at the first step whose reader needs it after the body is gone, with its type
// defined there (epic §5.2, as annotated in round 10).
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
    pub fn edge_view(&self, primary_app_ref: AppRef) -> EdgeView<'_>;
}
/// What the adapter family reads of a resolved program: the primary app, both edge
/// sets and the merged site facts. Borrowed, `Copy`.
#[derive(Clone, Copy)]
pub struct EdgeView<'r> {
    pub primary_app_ref: AppRef,
    pub edges: &'r [ClassifiedEdge],
    pub dependency_body_edges: &'r [ClassifiedEdge],
    pub site_facts: &'r HashMap<ObligationId, SiteFacts>,
}
impl<'r> EdgeView<'r> {
    pub fn all_edges(self) -> impl Iterator<Item = &'r ClassifiedEdge> + Clone; // same order as `ProgramBody::all_edges`
}
impl ProgramReport {
    /// A report straight from the resolver (`fresh_program_from_snapshot*`, the tests'
    /// `build_models`): `edges` = `report.edges`, `dependency_body_edges` = `&[]`.
    pub fn edge_view(&self) -> EdgeView<'_>;
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

**Amended by code review r1 (CR14):** the projection does not keep the filter over
`all_edges`. The `dependencyBodies` histogram reads `body.dependency_body_edges()`
directly (`impact_graph/mod.rs`, no clone); the equivalence argued below is now a
`debug_assert!` in `ResolvedProgram::build` (every dependency body edge is a `CallSite`
from a dependency app). A1b.1's golden and the CDO `alsem graph` bytes (parent vs head,
equal) pin the output. The bullets below are the original design.

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

**Amended by code review r1 (CR5, CR7, CR8, CR9, CR13):** where the text below says
`ResolvedProgram::build(inputs, FULL)` / `Summary`, read `ProgramScope::CrossApp` /
`Workspace`. The names `meta.primary_app_ref` and `meta.loaded_apps` are gone: readers
use the body's `ctx.primary_app_ref` (through `edge_view()`, which takes no argument)
and `ResolvedProgram::loaded_apps()`. There is no `body.take().expect(..)`:
`attach_program_calls(rows, program)` consumes the program and returns `(Model,
ProgramMeta)`, reading inputs and coverage from the meta, and the single-app arm moves
`meta.coverage` into `AnalysisModel.fresh` instead of cloning it. `aldump
--compiler-oracle` also builds through `ResolvedProgram::build` (`Workspace`, or
`CrossApp` under `--all-apps`). The bullets below are the original design.

- `impact_graph::export_workspace(inputs)` → `export_from_program(&ResolvedProgram)`;
  it reads `body.ctx` (`graph()`, `all_units()`, `decl_surface()`, `snapshot()`),
  `body.all_edges()`, `body.site_facts`, `meta.coverage_contract.missing`,
  `meta.recovered_files`, `meta.primary_app_ref`, `meta.loaded_apps`, the histograms
  and `inputs.driver_version`. The id check (`mod.rs:199-204`) reads `inputs.app_id()`
  and stays AFTER the build, where it is today on purpose (`mod.rs:194-198`): a missing
  or malformed `app.json` keeps the build's `read <path>: <io>` / `parse <path>: <serde>`
  error, and the id message appears only when the build succeeded. With the build `Ok`
  the JSON parsed, so `app_id()` is `None` exactly when the snapshot's `guid` is empty
  today (`id` missing, not a string, or empty): the same rule, no stderr change.
  `alsem graph` builds `ResolvedProgram::build(inputs, FULL)` and exports; the
  private build at `mod.rs:192-193` is gone.
- **One form for steps 1b and 2: the model builders build the program INSIDE**, as
  today (`program_calls.rs:822-823`). `assemble_and_resolve_cross_app_program(inputs:
  &Arc<InputSet>, model_instance_id, skip_roots_config)` (the §4.2.6 signature) starts
  with `let mut program = ResolvedProgram::build(inputs, FULL)` and owns that local:
  demand (`:838-845`) reads `program.body.ctx.graph()` and `body.all_edges()`; assembly
  (`:846-856`) reads `body.ctx` and `body.parenless_calls`; `loaded_apps` (`:895`) is
  `meta.loaded_apps`; `attach_program_calls_with` takes
  `program.body.take().expect("built once")` and `&program.meta.coverage`. Step 1b keeps
  its return type (`Option<CrossAppProgram>`, the build `Err` mapped to `None` as `.ok()?`
  does today); step 2 changes ONLY the return type to `Result<_, CrossAppFailure>`, the
  build's `Err(e)` becoming `CrossAppFailure::Build(e)` at that same line (§4.4.5). No
  caller builds or holds a `ResolvedProgram` for these builders; `build_cross_app_base`
  (`capability_cone.rs:3433`) calls the builder with `inputs`, as it calls it with a path
  today.
- `assemble_and_resolve_workspace_program` (`:713-728`) the same way, with `Summary`:
  it takes `inputs`, builds inside, keeps its return type. `build_analysis_model`'s
  single-app arm (`run.rs:285-357`) builds inside itself as today (`:285-291`), now
  `ResolvedProgram::build(inputs, Summary)`, and owns that local.
- The adapter (`program_calls.rs:922-970`) reads `body.all_edges()` where it read
  `report.edges` (through `SiteLinks::build`, `:429`) and `body.site_facts`. The
  signatures that change, every one from `&ProgramReport` (or a slice) to one
  `EdgeView<'_>` (§4.3.1), in one commit:
  - `SiteLinks::build(view: EdgeView<'r>)` (`site_links.rs:64`; it iterates
    `view.all_edges()`); its tests at `:121`, `:133` pass `report.edge_view()`;
  - `join(view, ctx, ws)`, `site_census(view, ctx, ws)` and `adapter(view, ctx, ws, ..)`
    (`program_calls.rs:401`, `:386`, `:922`): they read `view.primary_app_ref`
    (`:407`, `:952`, `:957-958`) and `view.site_facts` (`:967`);
  - `resolved_calls_from_program(view, ctx, ws, ..)` (`:622`). Its callers that hold a
    plain resolver report (`build_models`, `:1873`, `:2083`, `:2408`, `:2468`, `:2482`;
    `program_calls_adapter_tests.rs:151`, `:1310`) pass `report.edge_view()`, so those
    tests keep `ProgramReport` and see the edges they see today (no dependency bodies);
    `build_models` itself (`:1876`) gains one `InputSet::read(workspace,
    &InputNeeds::default())` line for its `assemble_and_resolve_workspace_from_program`
    call (§4.2.3);
  - `cross_app_demand(primary, graph, edges: impl IntoIterator<Item = &'a ClassifiedEdge>)`
    (`demand.rs:29-33`; it walks the edges once, `:37`); the builder passes
    `body.all_edges()`.
  - the projection's `Inputs.edges` (`project.rs:32`) becomes an `EdgeView<'a>`, and
    `engine_edges` (`:322-324`) iterates `inp.edges.all_edges()`. The view already
    carries the primary app and the site facts, so `Inputs.primary` (`:29`) and
    `Inputs.site_facts` (`:33`) are DELETED, not kept beside it: `project` reads
    `inp.edges.primary_app_ref` (today `inp.primary`, `:1025`) and
    `inp.edges.site_facts` (today `inp.site_facts`, `:385`, `:402`), so the projection
    has one source for each. So no caller of
    `project` chooses the order: `export_from_program` passes the view, and the test
    builder (`:1351-1383`) builds one from its two lists.
  The production callers pass `program.body.edge_view(program.meta.primary_app_ref)`.
- `AnalysisModel.fresh` (`run.rs:251-261`) stays a `Result<FreshCoverage, String>`; it
  is `program.meta.coverage.clone()` on success.

#### 4.3.4 Release point and sizes (G-6)

`attach_program_calls_with` drops the body at `program_calls.rs:669-676` (`b3.report_drop`,
`b3.ctx_drop`) — unchanged, except that `ProgramBody` is what is dropped. **`ProgramMeta`
is a local of the builder that built the program, at steps 1b and 2.** The builders keep
their return shapes (§4.3.3): each reader's value is copied out before the builder
returns (`AnalysisModel.fresh` = `meta.coverage`, moved out of the meta
`attach_program_calls` hands back (amended by code review r1, CR7), `loaded_apps` into
`CrossAppProgram`), and `ProgramMeta` drops when its builder returns (`graph`: when
`export_from_program` returns). No command, `AnalysisTarget` or `View` holds it at these
steps; it joins a holder at the first step whose reader needs it after the build (epic
§5.2). What the process retains is the command's own `Arc<InputSet>` (§4.2.6), which
`meta.inputs` shares. Its size is NOT "a few KB": `al_files[].text` is the whole
workspace source, 5.09 MB on CDO (582 files), and grows with the workspace. On CDO this
is inside the 85 MB band; the `inputs` census bucket prints it (A1b.5) so a larger
workspace is a number, not a surprise. Everything else in `ProgramMeta` is small
(histograms, ledger rows, the loaded-app rows).

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
    // No `field_index`: it is a solver INPUT only (`detector_context.rs:1365-1393`; the
    // base's copy feeds its own summaries, `capability_cone.rs:3481-3488`, `:3545`), so it
    // stays a local of `build_l4` and drops when the solve returns. Kept here it would
    // live through digest's and prove's snapshot composition with no reader.
    pub bundle: SummaryBundle,                // the compact store (`effect_store.rs:709`)
    pub summarize_diagnostics: Vec<SummarizeDiagnostic>,
    /// The non-convergence line(s) the solver used to print (§4.4.7).
    pub warnings: Vec<String>,
}
/// The solver's per-routine core summaries (the `HashMap<String, RoutineSummary>` that
/// `compute_summaries_v2_bundle_with_leaves` returns, `summary_runner.rs:1079-1083`),
/// UNHARVESTED. The `det` builder that called `build_l4` harvests it; a plan with no
/// `det` fill drops it, as query's builders drop `_map` today (`effect_query_cli.rs:251`).
pub struct CoreSummaries(pub HashMap<String, RoutineSummary>);
/// What a caller asks `build_l4` for. `extra_diagnostics` is EMPTY in every production
/// caller; only the `#[cfg(test)]` `View::prepare_with_diagnostics` (§8.4) sets it.
/// `build_l4` appends it to `summarize_diagnostics` AFTER rendering `warnings`, so an
/// injected row is never printed.
pub struct L4Request { pub scc: Option<Arc<SccResult>>, pub extra_diagnostics: Vec<SummarizeDiagnostic> }
pub fn build_l4(model: &Model, graph: &CombinedGraph, req: L4Request,
                cross: Option<&CrossAppInputs<'_>>) -> (L4Analysis, CoreSummaries);
```

**The solve runs after the cone, as today.** Today the context builds its cone first
(`context.capability_cones`, `detector_context.rs:953-1155`: `compose_cone_over_graph`
at `:1005`, the `pd_solve` arena dropped at `:1017`) and the SCC and v2 solve after it
(`:1296-1396`); the harvest drains `core_summaries` last (`:1427-1495`). So the bundle
and the core map (up to ~730 MiB on 8020, `:1440`) never exist while the cone's working
set (the 525 MiB `pd_solve` arena and 15 M entries, epic §3) is alive. Step 2 keeps that
order: when a `det` fill happens, `build_detector_artifacts` calls `build_l4` itself, at
today's `:1296` position, after its cone (§4.4.2). Only a plan with no `det` fill (query)
calls `build_l4` from `prepare` directly; it builds no cone.

`build_l4` is today's inline block (`detector_context.rs:1296-1511`) lifted out: the
SCC (`:1297-1307`, run when `req.scc` is `None`; a cross-app caller passes
`Arc::clone(&base.combined_scc)`, which is the same Tarjan over the same
`edges_by_from`, `capability_cone.rs:3473-3480`; no third copy exists, the `View`
holds no `scc` of its own), the field index
(`:1365-1372`, a local, not returned) and `compute_summaries_v2_bundle_with_leaves` (`:1387-1396`) with the
leaves and fixed roles from `cross`. The harvest (`:1427-1495`) is NOT in `build_l4`:
it drains the core summaries AND reads `uncertainty_edges_by_from` (`:1473`), a
context field built at `:1257` from the model's calls, so it runs in
`build_detector_artifacts`, which builds that field, exactly where it runs today. Its
result (the uncertainty union, up to ~730 MiB on 8020, `:1440`) is never built on a
plan with no `det` fill: query pays the solve and drops the core map, as its two
builders do today (`:219` `.0`, `:251` `_map`), and no harvest. `L4Analysis` holds
neither; today's digest drops graph and scc at the end of the statement
(`build_db_effect_bundle(resolved).2`, `digest_cli.rs:1626`), and prove keeps both to
the end of its scope (`let (_graph, _scc, bundle)`, `prove.rs:964`: a `_name`
binding is not a drop). `CoreSummaries` is a LOCAL of the function that called
`build_l4`, never a `View` field: inside `build_detector_artifacts` the harvest drains
it when the mask has `CORE_SUMMARIES`, and it drops before that function returns in
every other case; on query's plan it drops inside `prepare`. So the up to ~730 MiB map
never outlives the call that built it. `build_l4` replaces
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
    pub uncertainties: UncertaintyIndex,                                   // harvested here from `CoreSummaries` (`:1427-1495`)
    pub uncertainties_by_node: HashMap<String, UncertaintySetId>,          // same harvest
    pub parameter_roles_by_routine: HashMap<String, Vec<RecordRoleSummary>>, // same harvest
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
/// `l4: Some(req)` = build the `l4` cell here, after the cone (§4.4.1), and return it;
/// `None` = no solve (a mask without `CORE_SUMMARIES` on a plan with no `need_l4`, and
/// policy's RAW build).
pub fn build_detector_artifacts(model: &Model, demanded: u32, graph: Arc<CombinedGraph>,
                                l4: Option<L4Request>, cross: Option<CrossAppInputs<'_>>)
                                -> (DetectorArtifacts, Option<L4Analysis>);
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
/// Owner for the shims below (tests and `aldump`; no `alsem` command): a `DetectorContext` borrows its artifacts, so
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
  owned fields keep their names on `DetectorArtifacts`, and their types too with ONE
  exception: `graph` is `Arc<CombinedGraph>` where today it is `CombinedGraph`
  (`detector_context.rs:610`), because the view's graph cell shares it (§4.4.3). The
  harvest still runs inside this builder, so no other field becomes an `Arc`. Every
  detector read of `graph` is a field read (`ctx.graph.edges_by_from`, `.nodes`, …) or
  a borrow (`let graph = &ctx.graph;`, `d7.rs:143`, then `graph.nodes`), which compile
  through auto-deref. A use that needs `&CombinedGraph` in a generic or by-value
  position writes `&*ctx.graph`; the implementation fixes each such compile error that
  way, and no reader takes the graph by value (it is borrowed today). The struct-literal
  tests (`d17.rs:504`, `d50.rs:619`, `:784`, `minimal_ctx`) write `Arc::new(graph)`. The
  borrowed maps keep their names and types on `DetectorContext` (epic open question 9,
  closed).
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
- Two more registry `--lib` tests read the deleted `OnceLock` or pin `run_each`'s
  pre-loop fill, which step 2 moves into `View::prepare`. Each is stated here (a `--lib`
  outcome change §7 lists):
  - `declared_ordering_facts_are_filled_before_the_parallel_loop` (`registry.rs:926-951`,
    with its probe `ordering_facts_probe`, `:892-903`, which reads
    `c.ordering_facts.get()`) is DELETED: the thing it pins, the fill at
    `registry.rs:613-621`, is deleted. Its invariant (the ordering facts exist before
    the parallel loop starts, so no fill runs inside it, the `r4_differential` deadlock)
    moves to `session::prepare_fills_ordering_before_the_runs` (§8.4): a `View` over
    `empty_resolved()`, a plan with one probe detector that `requires:
    ORDERING_FACTS` and reports `c.ordering.is_some()` from inside the loop as
    `"filled"`/`"empty"`; the test expects `["filled"]`. Hand-stated precondition: the
    probe only LOOKS at the slot. The placement it pins is the use (`prepare`'s order:
    ordering, then runs), not a helper.
  - `undeclared_ordering_facts_read_inside_the_loop_fails_loudly` (`:962-982`) is KEPT,
    rewritten to the shim (`let owned = build_detector_context(&resolved, 0); let ctx =
    owned.ctx();`): at mask 0 the shim fills no ordering, the reader calls
    `get_ordering_facts`, the `expect` panics with a text containing
    `substrate::ORDERING_FACTS`, and `run_each` turns that into the diagnostic the test
    looks for. Its `#[cfg(debug_assertions)]` and the reader's are dropped: the `expect`
    holds in release too. Its discrimination break changes from "delete the
    `debug_assert!`" to "replace the `expect` with a fall-back to an empty map".
- A third `--lib` test pins the deleted lazy fill: `ordering_facts_are_lazy_and_parity_with_direct_compute`
  (`detector_context.rs:1700-1725`). It asserts `ctx.ordering_facts.get().is_none()`, then
  calls `get_ordering_facts()` on an `ALL` context, which has no `ORDERING_FACTS` bit
  (`registry.rs:141`). After step 2 the field is gone (no compile) and the call hits the
  `expect`. It is DELETED: laziness is the thing step 2 removes, and its parity half
  (ordering equals a direct `compute_ordering_facts`) is the definition of the fill, which
  `session::prepare_fills_ordering_before_the_runs` and the parity test (§4.4.6, mask
  `ALL | ORDERING_FACTS`) exercise. §7 step 2 lists it.
- `build_detector_artifacts` is `build_detector_context_with` (`:899-1617`) minus the
  graph build (`:938-946`: it takes the view's graph), with the inline SCC, field index
  and solve (`:1296-1396`) replaced by ONE call `build_l4(model, &graph, req, cross)` at
  the same place, after the cone (`:953-1155`), when `l4` is `Some`. The harvest
  (`:1427-1495`) stays here and drains that call's `CoreSummaries`; minus the borrowed
  maps (`:1164-1172`, `:1529-1534`, `:1557-1558`: they move to `detector_context`). It
  returns the `L4Analysis` it built, which `prepare` stores as the `l4` cell.
  `summarize_diagnostics` is `l4.summarize_diagnostics.clone()` when
  `demanded & CORE_SUMMARIES != 0` and an `l4` was built, else empty — the decision (a)
  rule of `registry.rs:40-44`, now per run (§4.4.3).
- `build_detector_context` / `build_detector_context_with` stay as shims for the tests
  and `aldump` (no `alsem` command calls them; not `#[cfg(test)]`, see below) that
  return an `OwnedContext` (above): graph + `build_detector_artifacts` with
  `l4: Some(L4Request { scc: None, extra_diagnostics: vec![] })` when the mask has
  `CORE_SUMMARIES` (its `L4Analysis` is dropped on return), **plus the ordering fill whenever the
  mask has `ORDERING_FACTS`**, done FIRST and through the same `fill_ordering(model)` helper
  `View::prepare` calls (§4.4.3 rule 1), so it runs on `big_stack_pool().install` as
  today's `run_each` fill does (`registry.rs:604-621`: it recurses over statement trees,
  `compute_return_summaries`). `aldump`'s main is on no big stack (no `big_stack` use in
  `src/bin/aldump.rs`), and neither is a test thread, so a fill on the caller's thread
  could overflow on real Base Application source. The ordering
  clause is what keeps `every_detector_alone_declares_what_it_reads` and
  `undeclared_ordering_facts_read_inside_the_loop_fails_loudly` meaning what they mean
  today (the third ordering test is replaced, see the bullet above): the fill
  leaves `run_each` (`registry.rs:613-621`), so `every_detector_alone_declares_what_it_reads`
  (`:1002`, a context at `detector.requires`) gets its ordering from the shim, and a
  detector that reads without declaring finds `None`, panics in `get_ordering_facts`
  with the `expect` text above (it contains `substrate::ORDERING_FACTS`, the substring
  `:1007` greps), and `run_each` turns that into the diagnostic the test counts. The 47
  call sites (17 files, `grep build_detector_context`) each become two lines; the three
  tests that write the context as a struct literal (`d17.rs:504`, `d50.rs:619`, `:784`)
  write a `DetectorArtifacts` literal and call `detector_context`; the 63
  `minimal_ctx` sites get `MinimalCtx` (above) (§7). No `alsem` command calls the
  shims. The tests and `aldump` reach them through `run_detectors` /
  `run_detectors_cross_app` (§4.4.3), and `aldump` ALSO calls one directly:
  `switch_dump.rs:120` builds `build_detector_context(&resolved, 0)` for the event-graph
  rows. So the shims are NOT `#[cfg(test)]`: they stay `pub` library functions, used by
  tests and `aldump` only ("test-only" above means "no `alsem` command"). The switch
  dump's call becomes the two-line `owned`/`ctx()` form like every other site.

#### 4.4.3 `View`, `Plan`, `RunKey` (`src/engine/session.rs`)

```rust
pub struct View {
    pub target: AnalysisTarget,                 // `SingleApp(Model)` | `CrossApp(R3a5CrossAppBase)` (`run.rs:173-176`); `rel` at step 4
    graph: Option<Arc<CombinedGraph>>,          // one graph per view; cross-app: `Arc::clone(&base.graph)` (`:3470-3480`)
    l4: Option<L4Analysis>,                     // no `core` field: `CoreSummaries` is a local of the builder that called `build_l4` (§4.4.1, rule 1)
    det: Option<DetectorArtifacts>,
    released: u8,                               // one bit per `Cell` released; a released cell is never refilled (rule 1)
    ordering: Option<HashMap<String, OrderingFacts>>,
    runs: BTreeMap<RunKey, Arc<RunOutput>>,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RunKey { pub detectors: Vec<String>, pub demanded: u32, pub scope_primary: bool }
pub struct Plan { runs: Vec<Vec<Detector>>, scope_primary: bool, need_l4: bool, // fields private to `session.rs`: built only by the factories (§4.4.4)
                  /// The substrate bits a post-run projection reads off `det` (epic rule 3 item 8):
                  /// digest and prove `TRANSACTION_SPANS` (`tx_context_map`), events `SUMMARIES`
                  /// (`compute_fanout_report`'s coverage read, `events.rs:429`). `prepare` asserts
                  /// `union & after_run == after_run` before the `det` fill, naming the missing bit;
                  /// the artifact a projection reads is then in the plan, not an accident of the
                  /// detector set (A2.10).
                  after_run: u32 }
pub enum Cell { Det, L4, Ordering, Graph }
impl View {
    pub fn new(target: AnalysisTarget) -> View;       // nothing else: no step-2 reader of `InputSet` or `ResolvedProgram` goes through the view
    pub fn prepare(&mut self, plan: &Plan);           // fills every `None` cell the plan needs, in order: ordering, graph, det (its cone, then `l4`, then its harvest), runs; a plan with no run (query): graph, then `l4` alone
    #[cfg(test)]
    pub(crate) fn prepare_with_diagnostics(&mut self, plan: &Plan, diags: Vec<SummarizeDiagnostic>); // `prepare` with `L4Request.extra_diagnostics = diags` (§8.4); both call one private `prepare_inner`
    pub fn release(&mut self, cell: Cell);
    pub fn model(&self) -> &Model;                    // `target.resolved()`
    pub fn graph(&self) -> &Arc<CombinedGraph>;
    pub fn l4(&self) -> &L4Analysis;  pub fn det(&self) -> &DetectorArtifacts;
    pub fn detector_context(&self) -> DetectorContext<'_>;
    pub fn run(&self, key: &RunKey) -> Arc<RunOutput>; // a clone of the stored `Arc`; panics if `prepare` did not run it. An `Arc`, not `&RunOutput`: analyze calls `release(..)` (`&mut self`) while it still uses the output (§4.4.4), and a borrow of `self` cannot live across that
}
```

`View::new` takes the target alone. The commands hold `&Arc<InputSet>` (passed down from
`alsem.rs`, §4.2.6) and, through `AnalysisModel.fresh`, a copy of `meta.coverage`
(§4.3.3); `ProgramMeta` itself stays a local of the builder (§4.3.4). Nothing at step 2
reads either through the view,
and `AnalysisTarget::build` returns no `ResolvedProgram` (the cross-app path builds
and consumes it inside `build_cross_app_base`, `capability_cone.rs:3455-3585`). The
epic's `View.program` joins at the first step that reads it from the view (epic §5.9,
as annotated). `Universe` is the target's variant, not a field.

Rules (epic §5.9), as code:

1. **Serial fills from built inputs.** `prepare` fills every cell the plan needs that
   is still `None`. **Each cell is filled at most once per `View`.** A second `prepare`
   (no step-2 command makes one) obeys three asserts, release builds too, each naming
   the cell: (a) a cell the plan needs whose `released` bit is set panics ("<cell>
   released; a View fills each cell once"), since a refill after `release(Det)` would
   find no `CoreSummaries` (the first `det` fill took them) and silently build `det`
   without uncertainties or parameter roles; (b) a `det` that is already filled must
   cover the new plan: `det.demanded & union == union`, else panic naming the missing
   bits, so a wider plan never runs on a narrower `det` and output never depends on
   fill state (A2.5); (c) a `det` fill at a `CORE_SUMMARIES` mask must build `l4` in
   the SAME `build_detector_artifacts` call, since `CoreSummaries` is that call's local;
   a `det` fill that needs `l4` while `l4` is already filled panics. Fills, in order: `ordering` FIRST, when any run demands `ORDERING_FACTS`
   (`fill_ordering(model)`, the one helper the shim also calls, §4.4.2: it runs
   `compute_ordering_facts(model)` on `big_stack_pool().install`, since the fill recurses
   over statement trees, `registry.rs:604-609`). It reads the model alone, and it composes a
   whole snapshot of its own (`digest.rs:4016-4019`) that dies when it returns; filling it
   before `graph`, `l4` and `det` means only the small facts map is alive beside them,
   where today the full context and that snapshot are alive together. Byte-neutral (a
   function of the model); the deadlock rule holds (every fill is before the runs). Then
   `graph` (single-app: `build_combined_graph` over `model.calls`/`events`,
   today's `detector_context.rs:938-946`; cross-app: `Arc::clone(&base.graph)` — the
   base's `graph` becomes `Arc<CombinedGraph>` at step 2 and STAYS on the base, since
   the r3a5 projections (`capability_cone.rs:3613`) and `QuerySubstrate::from_cross_app_base`
   (`effect_query_cli.rs:171`, until §4.4.1 replaces it) read it there; §10 item 3),
   then `det` at `union(masks)` — never with `RAW_INHERITED_FACTS` (epic I5, Decision 50;
   policy's RAW build is not a cell, §4.4.4) — which also fills `l4`:
   `let (det, l4) = build_detector_artifacts(model, mask, Arc::clone(&graph), req, cross)`
   with `req = Some(L4Request { scc, extra_diagnostics })` when
   `plan.need_l4 || mask has CORE_SUMMARIES`, else `None`; `self.det = Some(det)`,
   `self.l4 = l4`. The builder runs its cone, THEN `build_l4`, THEN the harvest, which is
   today's order (§4.4.1): the solve never runs while the cone's working set is alive.
   Single-app `scc` is `None` and `build_l4` runs today's Tarjan `:1297-1307`, which today
   also runs only under `CORE_SUMMARIES` at `:1296`, so a `--single-app --detector
   <non-core>` run pays no extra Tarjan; cross-app `scc` is
   `Some(Arc::clone(&base.combined_scc))`; `build_l4` prints its `warnings` before it
   returns, §4.4.7. A plan with no run and `need_l4` (query) fills `l4` with
   `build_l4(model, &graph, req, cross)` directly and drops its `CoreSummaries` there.
   `prepare` asserts `mask & plan.after_run == plan.after_run` before the `det` fill,
   naming the missing bit — for `TRANSACTION_SPANS` that is the bit that forces the
   summaries the tx map is built from, `registry.rs:53-56`. Then each run, stored in
   `runs`. Every builder takes
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
4. **No interior mutability or lazy fill** in `session.rs`: no `OnceLock`, `OnceCell`,
   `LazyLock`, `Lazy`, `Cell`, `RefCell`, `UnsafeCell`, `Mutex`, `RwLock`, `Atomic*`
   (an `AtomicU8` is the obvious shortcut for `View.released`), `thread_local!` or
   `static mut` (A2.9's `--lib` test reads the file's own source, §8.4; its needle for the
   `Cell` family is `Cell<`, which also matches `RefCell<`, `OnceCell<`,
   `UnsafeCell<` and never the file's own `enum Cell` or a `Cell::Det`).
   Every fill is on `prepare(&mut self)`; every reader takes `&self`. Stated limit: a
   lazy fill hidden in a type defined in another module is not caught by A2.9; the
   `&mut self`-only fill is the rule, and a `View` field of a type with interior
   mutability is a review flag.
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
means the two are never alive together after step 2. Today that holds only in part.
Step 2 removes one overlap and ADDS a smaller one, so "never raises" is a measurement
(A2.4), not a structural claim:
- digest: `digest_cli.rs:1646-1650` drops the snapshot before the diagnostics run, but
  the tx map's whole context (its cone working set, its v2 solve and core map) is built
  while the finished snapshot is alive, `:1580`–`:1650`. After step 2 no context
  overlaps the snapshot.
- prove: it composes the snapshot (`prove.rs:897`), then builds the diagnostics context
  (`:902`, inside `build_envelope_diagnostics_json`) and later the tx map's `ALL`
  context (`:958`) while that snapshot is still alive. After step 2 neither context
  overlaps the snapshot.
- **The new overlap: `l4` lives through snapshot composition.** Today digest and prove
  build their bundle AFTER the snapshot is composed (`digest_cli.rs:1580` then
  `:1625-1628`; `prove.rs:897` then `:964`). After step 2 `l4` (`bundle` and `scc`; no
  `field_index`, §4.4.1) is built inside the `det` fill, before `compose_snapshot`, and
  lives until `release(L4)` after the query. It cannot be released before composition:
  `DbTempStates` borrows the bundle (`digest_cli.rs:744-761`) and is read by the query,
  which reads the snapshot. So `l4` is alive during composition's own working set: its
  cone and `pd_solve` (`capability_cone.rs:4082-4090`) and its fold solve, which
  materializes summaries (`:3215-3222`; `summary_runner.rs:1383-1389`). This holds in
  both universes, for digest and for prove. The `l4` census bucket (A2.8) sizes it, and
  A2.4 bounds both peaks on digest (both universes) AND prove, so a regression is seen.
  On a miss the epic's fallback ladder applies (§6.3).
- Inside the `det` fill, the cone's working set and the solve's output are never alive
  together, as today: `build_detector_artifacts` runs its cone, then `build_l4`, then
  the harvest (§4.4.1, rule 1). A fill order that ran `l4` before `det` would keep the
  bundle and the core map (up to ~730 MiB on 8020) alive through the cone on every plan
  with `CORE_SUMMARIES`. That would raise the post-attach peak, and A2.4 would see it.

| Command | Order on the `View` | Deleted |
|---|---|---|
| analyze | `prepare(Plan { runs: [selected], scope_primary: args.scope == Primary })`; `run(selected)`; `release(Det)`, `release(L4)`, `release(Ordering)`, `release(Graph)` — today the context dies inside the runner, before any projection (`registry.rs:413-416`, `:526-529`), and the `View` must not hold it longer; `analyzer_diagnostics_of_run` over that run; projection, filter, baseline, suppression, coverage, format (`run.rs:522-700`, unchanged) | nothing (analyze had no duplicate); the failure path's second build (§4.4.5) |
| digest | the pipeline's no-input check (`digest_cli.rs:1590-1608`: empty `changedFiles`, `changedRoutines` and a blank `diffText`, reachable through `--diff <file>` on an empty or whitespace file, which passes the CLI check at `alsem.rs:1049-1058`) runs FIRST, right after `AnalysisTarget::build` and before `prepare`, so a bad input never pays the detector run or the solve. It keeps its place after the target build, so an id or build error still wins over it, as today. Then `prepare(Plan { runs: [default], scope_primary: true, need_l4: true, after_run: TRANSACTION_SPANS })`; `run(default)` → diagnostics JSON; `tx_context_map(&det.transaction_spans)` (the loop of `digest_cli.rs:546-558` as a pure function; today's map is built at `ALL`, `:543-544`, and the default set's union has `TRANSACTION_SPANS` because d9 is a default detector, `detectors/mod.rs:1294` — `prepare`'s `after_run` assertion keeps that true, A2.10); `release(Det)`, `release(Graph)`; `compose_snapshot(model)`; `resolve_changed_roots`; `compute_digest_effects_cli(&snap, model, Some(&roots))`; `DbTempStates::new(model, &l4.bundle)`; `run_digest_query_full_from_entries`; drop snapshot, entries, temp states; `release(L4)`; project JSON (with the diagnostics computed first); serialize | the second context (`:534-560`), the separate bundle (`:1625-1628`) |
| prove | the same plan; `run(default)` → diagnostics; `tx_context_map(&det.transaction_spans)` (today at `ALL`, `prove.rs:843`); `release(Det)`, `release(Graph)`; `compose_snapshot`; roots; `compute_digest_effects_cli(&snap, model, Some(from_ref(root)))` (today `None`, `prove.rs:961`; each root's pass is independent, `digest.rs:4094-4112`); `DbTempStates::new(model, &l4.bundle)`; query; `unresolved_cone`; `release(L4)`; project | the second context (`prove.rs:838-859`), the separate bundle (`:964`) |
| fingerprint | `prepare(Plan { runs: [default] })`; `run(default)` → `analyzer_diags_for` (`fingerprint_cli.rs:547-555`); `--strict` gate; `release(Det)`, `release(L4)`, `release(Graph)` (today the context, graph included, dies inside `analyzer_diags_for` at `fingerprint_cli.rs:364`, before `:403`); `compose_full_snapshot` / `compose_snapshot` as today (`:403`, `:464`) | nothing |
| diff | per side: a `View`; `--strict` → `run(default)`; `release(Det)`, `release(L4)`, `release(Graph)`; `compose_full_snapshot` (`diff/cli.rs:117`); the `View` dropped before the next side loads | nothing |
| events fanout / chains | `prepare(Plan { runs: [default], after_run: SUMMARIES })`; `run(default)` → `analyzer_diagnostic_lines`; `--strict`; the report from `det.event_graph`, `det.event_flow_indexes`, `det.summaries` (`events.rs:427-429`, `:572`; today from an `ALL` context, `:422-426`; `event_graph` and `event_flow_indexes` are built under every mask, `detector_context.rs:939`, `:1245`, and `summaries` under `SUMMARIES \| TRANSACTION_SPANS`, `:914` — the plan says so instead of relying on d9); `release(Det)`, `release(L4)`, `release(Graph)` | the second context (`:422-426`, `:567-571`) |
| policy check | `prepare(Plan { runs: [default] })`; `run(default)` → diag lines; `--strict`; `resolve_policy_check` from `inputs.policy_file`, reported where `:230` reports it today (§4.2.3); `release(Det)`, then `release(L4)` (the RAW build below takes `l4: None`, so nothing after the run reads `l4`; holding it through the ~10.9 GB raw-cone phase would be waste); then, with a policy loaded, the RAW build where `:261-266` builds it today: `let raw = build_detector_artifacts(model, SUMMARIES \| RAW_INHERITED_FACTS, Arc::clone(view.graph()), None, target.cross_app_inputs())` — a command LOCAL, not a cell, so no `det` ever carries `RAW_INHERITED_FACTS` (epic I5, Decisions 12 and 50) and the ~10.9 GB raw cone (`registry.rs:63-70`) is alive only after the detector loop, as today (`pipeline.rs:216` then `:261`); `PolicyModel::new(…, &raw.event_graph.events, …, &raw.summaries)` and `run_policy_engine` (`:281-289`), i.e. the policy rule engine runs AFTER `det` is released, off this local, not as an epic item-8 projection off `det` (epic §5.9 rule 3 item 8 and Decision 47, amended for the steps before 6: round 9 of the epic's log); drop `raw`; `release(Graph)` (the RAW build is the graph's last reader). The mask is `SUMMARIES \| RAW` and not today's `ALL \| RAW`: `PolicyModel` reads `event_graph` (built under every mask, `detector_context.rs:939`) and `summaries` (the `need_summaries` block, `:914`, `:1141-1146`, which `CORE_SUMMARIES` and `CLOSED_WORLD_TEMP` do not touch; `select_facts`, `policy_engine.rs:110-125`, reads the raw half), so the two maps are equal; `:926-930`'s assertion (`RAW` needs `SUMMARIES`) holds. The RAW build shares the view's graph (its own `build_combined_graph` and the v2 solve it ran under `ALL` are gone), so §4.4.8 counts it as `ctx` 2, `cone` 2, `pd_solve` 2, `v2` 1. It goes with step 6 (Decision 16) | the RAW context's own graph, Tarjan and solve (`:261-266` at `ALL`) |
| query | `prepare(Plan { runs: [], need_l4: true })` (`l4.warnings` printed by `build_l4`, §4.4.7); `DbEffectQuery::build(&l4.bundle, &l4.scc, &graph)` | the two bundle builders (`effect_query_cli.rs:207-261`) |
| graph | no `View`: `export_from_program(&program)` | the private build (§4.3.3) |

**Each command's `Plan` is built by one named production function** in `session.rs`,
and the command calls it: `digest_plan(detectors: Vec<Detector>) -> Plan` (prove calls
it too: the same plan), `events_plan(detectors)`, and one per other row. The function
sets `after_run` (`TRANSACTION_SPANS` for digest and prove, `SUMMARIES` for events).
A test of the factory alone does not pin its USE: a command could build an equal literal
plan with `after_run: 0` and stay green, because the default set's union already holds
both bits (d9, d1). So the command path itself takes the detector set: each of
`run_digest_pipeline`, `run_prove_pipeline` and `run_events_fanout` /
`run_events_chains` becomes a thin public wrapper over a `pub(crate)` inner form
(`digest_pipeline_with(detectors, …)`, `prove_pipeline_with(detectors, …)`,
`events_fanout_with(detectors, …)`, `events_chains_with(detectors, …)`) that builds its
plan with the factory from the set it is given; the wrapper passes today's default set
and nothing else. A2.10's use test runs the INNER form with a set whose union lacks the bit
and expects `prepare`'s panic naming it (§8.4). Replacing the factory call with any plan
whose `after_run` is 0 (another command's factory, or a copy of this one) then fails that
test: nothing panics, and the projection silently reads an empty `det.transaction_spans`
/ `det.summaries` (a COMMIT's context falls back to `"unknown"`, `digest_cli.rs:906-914`). `Plan`'s fields are private to `session.rs` and its only constructors are the
factories, so a literal `Plan { .. }` outside `session.rs` does not compile. (The table
above writes each factory's result as a literal, for reading only.)

`release(Graph)` follows the graph's last reader in every row except query (whose
`DbEffectQuery` reads it to the end). Single-app, the view builds and owns the graph,
and today it dies with the context inside the runner (`registry.rs:413-416`) or inside
`analyzer_diags_for`; without the release it would live on through snapshot
composition (fingerprint, diff `--strict`, digest and prove `--single-app`, analyze
`--single-app` through projection). Cross-app the release drops only the view's
pointer; the base keeps its `Arc`, as today. `det` holds a second `Arc` to the same
graph, so `release(Det)` comes first. What stays: `l4.scc` lives with `l4.bundle`
through digest's and prove's snapshot composition (the new overlap above). It is new in
every case: today prove's `_scc` (`prove.rs:964`) and digest's SCC (end of
`digest_cli.rs:1626`) both come into being AFTER the snapshot was composed (`prove.rs:897`,
`digest_cli.rs:1580`). After step 2 the SCC alone (no graph) is alive during
composition. The `l4` census bucket sizes it, and A2.4 measures prove,
`digest --single-app` and `fingerprint` so a regression in either universe is seen.

Byte-neutrality of the reorders: every artifact is a pure function of the model, so
computing the diagnostics run before the snapshot changes no byte of stdout. What it
changes on stderr is in §4.4.7 and §7.

#### 4.4.5 analyze's failure classification (R6-3, Decision 29)

Today `build_cross_app_analysis_model` (`run.rs:370-402`) classifies a failed cross-app
build by RUNNING THE SINGLE-APP BUILD (`:387`), whose program build (`:285-291`) comes
before its id check (`:296-307`); `fresh` is that second build's result and
`empty_output_result` uses it as reason precedence 1 (`:884-890`). The second build is
deleted.

Classifying without a rerun needs to know WHY the cross-app path failed, and today's
API discards that: `build_cross_app_base` returns `Option<R3a5CrossAppBase>`
(`capability_cone.rs:3433-3436`), and it is `None` in three cases: the program build
failed (`build_cross_app_program(workspace).ok()?`, `program_calls.rs:822-823`, the
error dropped), the model assembly failed (`)?`, `:856`), or the model has no
`primary_app` (`.as_ref()?`, `capability_cone.rs:3442-3446`). Step 2 gives both
functions a `Result` that carries the case:

```rust
pub enum CrossAppFailure {
    /// The program build's own `Err` (row 2).
    Build(String),
    /// Build `Ok`, assembly `None`: the build's coverage (row 3).
    Assembly { fresh: FreshCoverage },
    /// Build and assembly `Ok`, `primary_app` `None` (row 4).
    NoPrimaryApp { fresh: FreshCoverage },
}
pub fn assemble_and_resolve_cross_app_program(inputs: &Arc<InputSet>, model_instance_id: &str,
    skip_roots_config: bool) -> Result<CrossAppProgram, CrossAppFailure>;   // `program_calls.rs:816-908`
pub(crate) fn build_cross_app_base(inputs: &Arc<InputSet>, model_instance_id: &str)
    -> Result<R3a5CrossAppBase, CrossAppFailure>;                          // `capability_cone.rs:3433`
```

Every other caller of the two (`grep` finds 19 call sites in 12 files: the r3a4/r3a5
projections, `QuerySubstrate`, `finding.rs`, `registry.rs`, and the `tests/r3`, `r4`,
`temp_state` suites) maps the `Err` back to today's `None` with `.ok()`, so none of
them changes behaviour. The rows, with `fresh` defined on each:

| Row | Today | After step 2 |
|---|---|---|
| `inputs.model_instance_id` is `None` (no readable `app.json` with a non-empty string `id`, or a failed walk) | no cross-app build; one single-app build; `NoModelInstanceId`; `fresh` = that build's `Result` | ONE build, under today's profile: `ResolvedProgram::build(inputs, Summary)` — the id is known before any build, so this row never pays `FULL`'s `resolve_dependency_bodies` for an empty output; `NoModelInstanceId`; `fresh` = its `Result`. The `Err` text of a snapshot failure is the same for both profiles (`full.rs:2140-2148`: the snapshot build fails before the profile matters); a test pins it on an id-less `app.json` plus an unreadable `.al` (A2.7) |
| id present; `Err(CrossAppFailure::Build(e))` | single-app rerun (`:387`), which fails the same way; its arm walks (`:312-313`) and classifies `ProgramBuildFailed(why)` with `why` = the RERUN's own error (`:315`) if the `NestedApps::Skip` list is non-empty, else `AssemblyFailed`; `:392` passes that `Err` through; `fresh` = the rerun's `Err`. (The fixed `"the cross-app model could not be built"`, `:389-391`, appears only when the cross-app build fails but the rerun SUCCEEDS; then `fresh` is `Ok` today) | `ProgramBuildFailed(e)` if `inputs.app_scoped_files()` is non-empty, else `AssemblyFailed`; `fresh = Err(e)`. The `why` text is the cross-app build's own error: the same text as today when the rerun fails the same way (the usual case: one program build fails twice for one reason), and the real error instead of the fixed string in the cross-app-only failure, where `fresh` turns from `Ok` into `Err(e)`. The string reached the user only through `analysis failure — program engine build failed: <why>` (`:494-498`), exit 2; no test pins it (grep: none) |
| build `Ok`, cross-app assembly returns `None` (empty model, `workspace.rs:1781-1784`): `Err(CrossAppFailure::Assembly { fresh })` | single-app rerun also assembles to nothing; `AssemblyFailed`; `fresh = Ok(single-app coverage)` | `AssemblyFailed`; `fresh = Ok(cross-app coverage)` (the variant's `fresh`). `empty_output_result` reads `fresh` only as `Err` text (`:884-890`), so the coverage value never reaches output |
| build and assembly `Ok`, `primary_app` `None`: `Err(CrossAppFailure::NoPrimaryApp { fresh })` | `primary_app` comes from a second disk read of `app.json` (`read_primary_app_from_disk`, `workspace.rs:1774`, `:1790-1800`), so only a file changed between that read and the id read could reach this; the single-app rerun then succeeds and the row is `ProgramBuildFailed("the cross-app model could not be built")` with `fresh` `Ok` | unreachable from step 1: `primary_app` and the model-instance id both come from `inputs.app_json` (§4.2.3), and both need the same non-empty string `id`. Kept as a fail-closed arm with today's outcome: `ProgramBuildFailed("the cross-app model could not be built")`, `fresh = Ok(fresh)`. No test (no input reaches it) |

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
- ONE print site: `build_l4` itself prints `warnings` with `eprintln!` just before it
  returns. Every solve goes through it: `View::prepare`'s `l4` fill (so on `alsem` the
  line is printed iff the cell was filled, `plan.need_l4 || some run's mask has
  CORE_SUMMARIES`) and the `OwnedContext` shims (§4.4.2), so the `aldump` modes that
  reach the shims through `run_detectors` / `run_detectors_cross_app`
  (`switch_dump.rs:126`, `finding.rs:952`, `:1049`) print exactly as today, when the
  context's own solve printed. It is the same place in the stream the solver printed
  from (during the build, before any stdout write), and the same kind of site (a
  library `eprintln!`, §5). One site, not one per command or per caller: with eight
  commands each printing for itself, a command that forgot would lose the line with no
  test failing (no fixture caps). "Filled", not "some run demands it": query's plan has no run
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
limit: the print site itself (`build_l4`, `:3215`) is pinned by no executable test, since
no fixture produces a `SummarizeDiagnostic`; one site is the mitigation.

#### 4.4.8 Counts after step 2 (what A2.1 pins)

| Command | program | bcg | cone | v2 | scc | pd_solve | ctx | snap |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| analyze, cross-app, default | 1 | 2 | 1 | 1 | 2 | 1 | 1 | 0 |
| analyze, cross-app, `--detector d47-…` (the context's graph is gone; no `l4` fill: `need_l4` is false and the mask has no `CORE_SUMMARIES`) | 1 | 3 | 1 | 1 | 3 | 1 | 1 | 1 |
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
  it reports (`detector_context.rs:435-443`). A count goes through a local fold plus
  `add_gated()`, never the ungated `add()` (§4.1.1). **A probe that costs time with
  the flag off** is caught at the use by A0.1's census-off cone wall against the parent
  (§6.2), not only by `start()`'s contract.
- **A counter that counts a test helper.** `build.graph_scc` sits at named call sites;
  `tarjan_scc` itself is untouched, so the solver's inner Tarjan and d7's own never
  count.
- **A read that still hits the disk.** The disk-edit-after-read tests (§8.2) cover one
  class each. A new reader added later is caught by none of them; the rule is that
  every function taking `&Path` under `src/engine/` and `src/program/model/` is a
  review flag from step 1 on.
- **A stale `InputSet`.** The LSP keeps its own file reading; `SnapshotBuilder::build`
  and its siblings read a fresh `build_only` `InputSet` per build (§4.2.6). Nothing caches an `InputSet` across builds.
  No struct the LSP retains holds one: `AppSetSnapshot` gains no field, and the model's
  `workspace_diagnostics` are built from an `&InputSet` parameter (§4.2.4). The holders
  are `ProgramMeta` (step 1b; a builder local, dropped when the builder returns, §4.3.4)
  and the command's own `Arc`, dropped at command end.
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
  test (it would have nothing to compare). The same
  rule covers `l4::bundle_equals_the_old_builders` (§8.4): `build_l4` lands beside
  `build_db_effect_bundle` / `cross_app_db_effect_bundle`, the test runs, then the
  deletion commit removes builders and test together. Neither keeps a test-only copy:
  a copy would pin the copy, not the code. **The commit history is NOT the proof**: the
  issue flow squash-merges after gating the branch HEAD (CLAUDE.md, autonomous-flow
  exception), where these tests are already gone, so no gate ever runs them and the
  intermediate commits do not reach `master`. So for each TRANSIENT test
  (`bundle_equals_the_old_builders`, `base_graph_equals_context_graph`,
  `bytes_split_sums_to_the_old_total`) the ledger records: the intermediate commit SHA it
  ran at; the exact `cargo test` command and its pass line at that SHA; the discrimination
  break, its failing assertion text and the green revert, at that same SHA. The final
  review checks the three rows exist and that each SHA is an ancestor of the branch
  HEAD, and the PR body quotes them, so the evidence survives the squash (no
  `scripts/` change: the check is a review item, not a gate).
- **A profile-dependent `Err` text on the no-id row.** A2.7's test pins the text on the
  one input that produces it.

## 6. Measurement plan

### 6.1 Protocol (every step)

CDO `U:/Git/DO-cdo-baseline/Cloud` at `bc3ccb18`, clean, pinned `.alpackages`;
`release-fast` binaries copied out of `target/` (cdo-measure skill §1); the parent commit
and the step's head built from the same `tree-sitter-al` commit; no concurrent cargo
build. Commands: `analyze --format sarif`, `digest --diff c.diff` (the P5 diff, 48 roots),
`fingerprint --format json --out f.json`, and for step 2 also `prove` on one P5 root and
`digest --single-app --diff c.diff`. The peaks come from `read_rss()`, which exists on
Windows only (§4.1.4, A0.4); the dev machine is Windows.
Runs interleave parent and head, ≥ 3 each, `ALSEM_TRACE=1 ALSEM_TRACE_STDERR=1`, the
process wall from the harness clock (`scratchpad/meas.py` pattern), both peaks from the
trace (§4.1.4). `RAYON_NUM_THREADS` unset on both sides (Decision 36).

Per timer: median and band = max − min over the parent's runs (G-5, Decision 54). That
observed band is for WALL times and timers only. **A peak's band is the epic's: 3 % of
the parent median (epic §5.12, Decision 22; about 85 MB on CDO).** So `band(peak)` below
always means 3 % of that peak's parent median; the observed max − min of the peaks is
recorded in the numbers file but bounds nothing. A bound is written as "head median ≤ /
≥ parent median ± band". The numbers file records every run, not only the medians.

### 6.2 Step 0

One bound, A0.1's use-level check: CDO `analyze` `context.capability_cones` wall with
the census OFF, head median ≤ parent median + band. A miss means a new probe costs
production time; the probe is fixed before any step-0 number is recorded. Records:
§4.1.7's table, including whether each timer's median is ≥ 2 × its
band (I6's rule). From the P5 numbers the expectation is: `context.capability_cones`
(20.9 s), `digest.tx_context_map` (≈ 21 s, A2.3's baseline; A2.3 subtracts both its band and
digest's wall band, so the check is its median ≥ 2 × the sum of the two) and digest's wall
(63–67 s) comfortably; `b3.adapter` (4.8 s) likely;
`b3.adapter.join` and `b3.adapter.by_decl` unknown until measured — if either's median
is < 2 × its band, step 10 / 11 is re-labelled non-speed before its spec is written
(epic I6). Census on/off byte check on SARIF (A0.5).

### 6.3 Steps 1, 1b, 2

| Step | Bound | Expected |
|---|---|---|
| 1 | none; both peaks and `analyze.total` reported against the parent; LSP `build_full` (Criterion, 100 and 1000 files) recorded parent vs head (§4.2.6) | peak +5 MB (the retained texts), inside the band; `build_full` flat (a `build_only` read does today's disk work) |
| 1b | process peak and post-attach peak ≤ parent median + band(peak) for analyze and digest | flat within the band: the retained `Arc<InputSet>` holds the workspace texts (5.09 MB on CDO, §4.3.4) and nothing else of size; the body drops where it did |
| 2 | digest wall: parent median − head median ≥ parent median(`digest.tx_context_map`) − band(`digest.tx_context_map`) − band(digest wall) (A2.3; band(digest wall) = max − min of the parent's digest wall runs, §6.1: the saving is read off two whole-command walls, whose noise the span's band does not cover, e.g. 63.70 vs 66.81 s on P5); both peaks ≤ parent median + band(peak) for analyze, digest (cross-app) and prove, and for `fingerprint` and `digest --single-app` (A2.4); cell census printed at the first run | ≈ −21 s on digest on the P5 machine (the deleted context; indicative only, the bound is relative); peaks flat or lower: inside the `det` fill the solve still follows the cone (§4.4.4); the one new overlap is `l4` (bundle, SCC) alive during snapshot composition in digest and prove (§4.4.4), sized by the `l4` bucket and expected well below the deleted context's working set; prove deletes the same two builds as digest (an `ALL` context, `prove.rs:843`, and a bundle, `:964`), events one `ALL` context (`events.rs:422-426`, cone plus v2 solve), so each saves the same order as digest's ~21 s on CDO, not fractions of a second. Not bounded: the §4.4.8 counters pin those deletions; the prove run of §6.1 records its wall; analyze unchanged |

A miss on a peak walks the epic's fallback ladder (§5.12). A miss on the digest saving
is a stop: the deleted context is the whole claim.

## 7. Golden families each step may move

Byte-neutral means none of the 8 targets of `scripts/check-goldens` moves and no
`--lib` outcome moves except the tests named in the Goldens column below. Per step,
what is allowed to change outside the goldens:

| Step | Goldens | Stated non-golden changes |
|---|---|---|
| 0 | none; `pd_solve::bytes_split_sums_to_the_old_total` is added and later deleted with `solve_bytes` (TRANSIENT, §8.1) | the `[cones-census]` stderr lines are renamed/split (a diagnostic, off by default); the `[C1_CONE_CENSUS]` report and its `C1_CONE_CENSUS=1` flag are replaced by the `context.<field>` buckets under `ALSEM_RETAINED_CENSUS=1` (§4.1.5; off by default); two new trace counter names. **Known stale references the issue flow may not fix** (`.claude/` is protected, CLAUDE.md): `.claude/skills/byte-census/SKILL.md:46` and `.claude/skills/perf-probe/SKILL.md:46` (main checkout) still name `C1_CONE_CENSUS`; the user updates them to `ALSEM_RETAINED_CENSUS` when step 0 merges. Historical docs and CHANGELOG entries that name the old flag describe past runs and stay |
| 1 | none; the in-process harnesses gain an `InputSet::read` line each (`tests/cli/cli_a_*`, `cli_b_*`, `cli_c_*`, `cli_d35_unknown_commit`, `cli_digest_*`, `cli_p1_inventory`, `cli_s6_consumers`, `cli_workspace_source`, `cli_analyze_*`, `cli_attribute_trivia`, `cli_event_dispatch_first_hop`, `cli_implicit_trigger_cone`); in `src/engine/switch_dump.rs` (§4.2.6) `analyze_builds_through_the_dumped_functions` bans `project_coverage_inputs` in place of the renamed `project_coverage_disk`, and `analyze_model_selects_app_scoped_files_from_the_program_parse` builds through an `InputSet`, same assertions; the `workspace.rs` / `program_calls.rs` tests that call the `*_from_program` entries gain an `InputSet::read` line (§4.2.3); the 11 test `Model` literals (`format_html.rs:1218`, `:1275`; `d1.rs:3194`, `:4277`; `d17.rs:526`; `d35.rs:289`; `d50.rs:811`; `detector_context.rs:1711`, `:1752`; `registry.rs:1098`; `run.rs:1149`) gain `workspace_diagnostics`, same assertions, and the production literal at `run.rs:863` gains `vec![]` (§4.2.4); the two full `FreshCoverage` test literals (`preflight.rs:133`, `:210`) gain `files_without_object: vec![]`, same assertions; `run_analyze_with_exit_uses_the_shared_aggregation` (`run.rs:1175-1210`) gains the needle `.workspace_diagnostics` (§4.2.4); the `--lib` build tests and `aldump --compiler-oracle` that call `build_program_with_coverage` gain an `InputSet::read` line (§4.2.6); `tools/census-probe`'s `q6` follows the step-1 signatures (§4.1.5; not a CI target) | a stdout/file write failure: one text, `al-sem: write error: <stream\|path>: <OS message>`, on every command; analyze exits 2 (today: abort), every other command keeps its exit 1 (today: exit 1 with four different texts); one stdout corner (§4.2.4): a nested app reached through a directory symlink, with an unreadable file inside it and no root `.al` file, loses its `No object declaration found in <rel>` info rows (the build parsed nothing; no fixture, no golden); a `.al` file added after the read no longer fails the model closed (§4.2.3, `select_program_files`); `aldump --switch-dump` on a workspace whose program build fails loses its `No object declaration found in <rel>` rows (§4.2.4; no golden reads the dump); the `inputs.read` counter |
| 1b | `tests/cli-query-goldens/graph.test-selection-graph.jsonl` must NOT move (A1b.1); the `--lib` tests that call `SiteLinks::build`, `site_census` or `resolved_calls_from_program` pass `report.edge_view()` (§4.3.3), same assertions | none |
| 2 | none; `tests/gap/gap_detector_substrate_parity.rs` widened; `--lib` tests that build a `DetectorContext` by hand (`d17.rs:504`, `d50.rs:619`, `:784`, the other `detector_context.rs` tests, among them `core_summaries_stay_lean_while_the_bundle_carries_the_db_effect_rows` and `reverse_effect_index_is_built_only_when_its_bit_is_demanded`, which read the bundle through `OwnedContext::l4()` (attempt 5, plan A3); the `set_uncertainties_for_test` callers mutate `DetectorArtifacts` before borrowing) and the 63 `minimal_ctx` sites (`test_support.rs:367`) are rewritten to the split, same assertions; three `--lib` outcome changes (§4.4.2): in `registry.rs`, `declared_ordering_facts_are_filled_before_the_parallel_loop` is DELETED (the `run_each` fill it pins is gone) and replaced by `session::prepare_fills_ordering_before_the_runs`, and `undeclared_ordering_facts_read_inside_the_loop_fails_loudly` is rewritten to the shim and now runs in release too (its `#[cfg(debug_assertions)]` dropped); in `detector_context.rs`, `ordering_facts_are_lazy_and_parity_with_direct_compute` (`:1700-1725`) is DELETED (it pins the lazy `OnceLock` fill step 2 removes) | on a capped SCC: fewer duplicate non-convergence lines on `alsem` (§4.4.7), and `aldump --r3a2` / `--r3a5-cross-app-summary` lose the line (their solves discard the diagnostics, `summary.rs:887`, `capability_cone.rs:3619`); the `aldump` modes that reach the shims (`--switch-dump`, `switch_dump.rs:126`; the r4 findings projections, `finding.rs:952`, `:1049`, cross-app included) keep theirs, because `build_l4` prints (§4.4.7); on a capped SCC under a plan with d47/d49/d51, the ordering snapshot's line now comes BEFORE the `l4` line (ordering fills first, §4.4.3 rule 1; today it came after the context's); analyze's build-failure message carries the real error where today's fixed string stood (§4.4.5, row 2: cross-app build fails, single-app rerun would have succeeded); digest with no usable input (an empty `--diff` file) still exits with the pipeline's error, but its check now runs before any snapshot (§4.4.4), so on a capped SCC the snapshot's non-convergence line no longer comes before the error |

The CDO `aldump --program-call-graph-stats` histogram SHA and the `cdo-measure` detector
baseline are checked at every step (epic I10, E14).

**Protected docs that go stale (user follow-ups, like the `.claude/skills` rows above).**
No step needs to edit a protected path. But `CLAUDE.md` (protected, its own rule) will
lag: its Key Modules list lacks `src/program/inputs.rs` (step 1),
`src/program/resolved.rs` (1b), `src/engine/session.rs`, `src/engine/l4/analysis.rs`
(2) and `src/engine/census.rs` (0); its Testing section's `tests/common/{cdo,regen}.rs`
list lacks `unreadable.rs` (step 1). The PR of each step names these lines for the user
to update when it merges. `scripts/check-goldens` needs no edit: no step adds a golden
directory.

## 8. Discrimination-proof plan

Every new test lists its break, the failing assertion, and the revert (epic I9). Breaks
are made with the Edit tool, run, then reverted with Edit; both outcomes go in the
ledger. A scripted break is never used.

**Recipe: an unreadable file or folder.** Bad bytes do not work: `decode_al_source` is
lossy (`source_text.rs:116-124`), so a read never fails on content. The walker skips
dangling links (`NotFound`, `:80-84`) and non-files (`:87-89`). The tests that need an unreadable
`.al` (`no_id_row_runs_one_build_and_keeps_the_text`, `build_failed_row_runs_one_build`,
the A1.3 / A1.7 cases) share ONE helper, `tests/common/unreadable.rs`, hoisted like
`cdo.rs`/`regen.rs` by `#[path = "../common/unreadable.rs"]` in each umbrella `main.rs`
and, for the `--lib` tests, by a `#[cfg(test)] #[path = …] mod unreadable;` line in the
module that uses it (one source, no copy). A `#[path]` is resolved from the declaring
file's directory (and, inside an inline `mod tests { }` of a non-`mod.rs` file, from
`<dir>/<file stem>/`; Rust Reference, "the `path` attribute"). So the line goes at the
file's TOP level, never inside `mod tests`, and climbs one `../` per directory below the
repo root: from `src/program/inputs.rs` it is
`#[path = "../../tests/common/unreadable.rs"]`; from `src/engine/gate/run.rs`,
`"../../../tests/common/unreadable.rs"`. `fn unreadable(path) -> Option<Guard>`:
- Unix: `chmod 000` on the file (or folder). Then it tries `std::fs::read` (a folder:
  `read_dir`); if that still succeeds (running as root, or a file system that ignores
  modes) it restores the mode and returns `None`. CI runs as a non-root user.
- Windows, a file: opens it with `OpenOptionsExt::share_mode(0)` and keeps the handle in
  the guard; every other open, in this process or a spawned `alsem`, fails with a sharing
  violation. It checks that `std::fs::read` now fails, else returns `None`.
- Windows, a folder: share mode does not stop a directory listing, so it adds a DENY
  ACE for list-directory: `icacls <dir> /deny *S-1-1-0:(RD)` (Everyone; `RD` = read
  data / list directory), run with `std::process::Command`. The owner keeps the right
  to change the ACL, so the guard restores it with `icacls <dir> /remove:d *S-1-1-0`.
  It checks that `std::fs::read_dir` now fails (the walk's `read_dir` then fails with
  an OS error, which is case (b) of A1.7), else restores and returns `None`. So case
  (b) and its break (2) run on the Windows dev machine, and on CI's Linux by `chmod`.
- The guard restores the mode / closes the handle on drop, so the tempdir can be deleted.
- `None` means skip: the test prints `skipped: cannot make <path> unreadable here` and
  returns. A skip is never a pass in the ledger; every proof run of §8 is recorded on a
  machine where the helper returned `Some`.

### 8.1 Step 0

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `cli_build_counters::<command>_<universe>` (one per §4.1.3 row) | per counter: add a second call of the counted builder at its site (e.g. a second `build_combined_graph` in `build_cross_app_base`); for `build.graph_scc`: add a `pt::counter_delta` to the solver's inner Tarjan | the table cell for that counter | remove the call / the counter |
| `cli_build_counters::census_rest_is_never_negative` (spawned `alsem analyze` on `ws-d1-multi-caller` with `ALSEM_CONES_CENSUS=1`; parse the `[cones-census]` `rest` line from stderr). Spawned, not `--lib`: `cones_census::enabled()` is a `OnceLock` read once from the environment (`detector_context.rs:426-429`) and its counters are process-global atomics (`:348-424`), so another lib test in the same process can fix the flag off or add to the counters first | charge `SINGLETON_NANOS` (`:362`) twice. Not `BFS_FOLD_NANOS`: the fixture has no recursive SCC (`tests/r0-corpus/ws-d1-multi-caller/src/D1MultiCaller.al`: three callers of one helper, no cycle), so the `recursive` arm (`capability_cone.rs:2552-2561`) never runs there and a double charge of a counter that stays 0 changes nothing — the break would pass. Every routine of the fixture is a singleton (`:2563`), so the break exercises a live counter. The double charge alone lowers `rest` by one singleton time, which need not exceed the untimed remainder on a microsecond fixture, so the break also adds the call's whole `elapsed` to `SINGLETON_NANOS` at `compose_inherited_cones`' exit, before `rest` is taken: then `rest = −Σ(component deltas) < 0` whenever any component ran, which the singleton arm does on every routine. `rest` is `i64` and printed signed (§4.1.1), so the test can see a negative value at all | `rest >= 0` | once |
| `pd_solve::bytes_split_sums_to_the_old_total` (unit, hand-stated small arena; TRANSIENT, like the two §5 oracle tests: it lands in the commit that adds `arena_bytes` and `working_bytes` BESIDE the unchanged `solve_bytes`, asserts `arena_bytes + working_bytes == solve_bytes` on that arena, and is deleted in the commit that deletes `solve_bytes`; the ledger records its run at a named intermediate SHA, §5. While both exist the oracle is the old code, not a copy; after the deletion nothing is left to compare, and a hand-stated byte literal would depend on `size_of` per platform) | drop `reps` from `working_bytes` | the sum equals `solve_bytes` | restore |
| `cli_build_counters::phase_peak_is_reported` (`#[cfg(windows)]`: `read_rss()` is `None` elsewhere, §4.1.4, so on CI's Linux both peaks are 0 and the test could neither pass nor discriminate; spawned `alsem analyze` on `ws-d1-multi-caller` with `ALSEM_TRACE=1`; read both `peak.*` counters). Spawned for the same reason: the tracer is a `OnceLock` from the environment (`perf_trace.rs:402-407`). An absolute "≥ 64 MB" bound is not used — a shared test process may already sit above it | stop the sampler before attach runs | `0 < peak.post_attach_mb <= peak.process_mb` (the sampler never ran → 0) | restore |
| A0.5 (measurement, not a test): SARIF census on == off on CDO | — | — | — |

### 8.2 Step 1

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `inputs::read_once` (`inputs.read` count 1 per command, 2 for `diff ws ws`, one per side; via the trace reader). A SPAWNED-process test in `tests/cli/cli_build_counters.rs` beside A0.2's rows, not a `--lib` test: the tracer is a `OnceLock` set once from the environment (`perf_trace.rs:402-407`) and the counters exist only in the trace file (B6's rule) | call `InputSet::read` a second time in `AnalysisTarget::build` | count == 1 | remove |
| `inputs::app_json_edit_after_read` (read, then rewrite `app.json` on disk to `{}`: no `id`, no `name`, `version` or `publisher`, and different bytes; run `analyze --format json`, `analyze --format pr-summary` and `fingerprint --format json`; each output equals its untouched run, whole bytes). One edit reaches every reader: each one restored to its own disk read sees a changed answer. A break that stays green gets that reader's output field added to the assertion until it fails; the ledger records one red run per reader | one break per reader, each separately: restore the disk read of (1) `read_workspace_apps` (`run.rs:122-157`); (2) `compute_gate_model_instance_id` (`model_instance_id.rs:75-96`); (3) `read_primary_app_from_disk` (`workspace.rs:1789-1800`); (4) `read_root_app_json_id` in the diagnostics (`workspace_diagnostics.rs:145-155`); (5) the `app-json` hash in `derive_inputs` (`snapshot_full.rs:819-827`) | (1) the pr-summary bytes; (2) the json bytes (no model id → fail-closed); (3) the json bytes; (4) the json diagnostics; (5) the fingerprint string | restore |
| `inputs::app_json_paths_edit_after_read` (analyze json: read, then add `sub/app.json` on disk, run; the diagnostics equal the untouched run's: no multi-app error) | restore `count_app_json_paths(workspace)` in `compute_workspace_diagnostics` (`workspace_diagnostics.rs:44`) | the diagnostics | restore |
| `inputs::coverage_yaml_edit_after_read` (fingerprint json on a workspace with an `al-sem.coverage.yaml`: read, rewrite the yaml's bytes, run; `workspaceFingerprint` and the `policy` row of `inputs[]` equal the untouched run's) | restore the yaml's disk read in `derive_inputs` (`snapshot_full.rs:868-878`) | the fingerprint string | restore |
| `inputs::roots_config_edit_after_read` (fingerprint json: read, delete `roots.config.json`, run; `inputs[]` rows and `rootClassifications` equal the untouched run) | restore the disk read in `load_roots_config` | the snapshot bytes | remove |
| `inputs::al_text_edit_after_read` (analyze json on `ws-d14-dead-routine`: run once untouched and take the finding's `detector` and line L; then read, append ` // al-sem-ignore <detector>` (that detector id, in full) to the END of line L on disk, so no line shifts, run; the finding is still reported). The id is required: the parser needs `\s+([\w-]+)` after the keyword (`inline_suppression.rs:38-50`) and the match needs `s.detector_id == finding.detector` and `line == directive_line \|\| line == directive_line + 1` (`:242-244`); a bare `// al-sem-ignore` suppresses nothing, and the break below would stay green. The ledger records that the break run shows `findings.len()` one lower (the directive is live) | restore `load_inline_suppressions`'s disk read | `findings.len()` | remove |
| `inputs::objectless_file_removed_after_read` (analyze json: a workspace with one objectless `.al`; read, delete that file, run; the `DIAG-index` line is still emitted) | restore the second parse / disk walk in `compute_workspace_diagnostics` | the diagnostic's presence | remove |
| `inputs::al_file_added_after_read` (analyze json on `ws-d1-multi-caller` copied to a tempdir: read, write a new `src/Added.al` with one codeunit, run; the output equals the untouched run's). Today's `select_program_files` would walk again, list the new file, miss it in the parse and return `ProgramFiles::Missing` (`workspace.rs:1644`, `:1657`): no model | restore the `discover_al_files_app_scoped` walk at `workspace.rs:1644` | the output equality (the model fails closed) | remove |
| `inputs::closure_app_replaced_after_read` (fingerprint json on the dependency workspace: read, overwrite the dependency `.app` bytes, run; `workspaceFingerprint` unchanged). Stated limit: it pins the HASH rows only; the loader still parses the `.app` after the hash (§4.2.2, Goal 2's exception), so a swap between the two reads gives a model from the new bytes under the old hash — today's race, unchanged | restore `hash_file` reads in `derive_inputs` | the fingerprint string | remove |
| `inputs::policy_edit_after_read` (policy check json: read, rewrite the yaml on disk, run; findings unchanged) | restore the read in `resolve_policy_check` | the policy output | remove |
| `inputs::baseline_edit_after_read` (analyze `--baseline`: read, rewrite the baseline file to suppress everything, run; findings unchanged) | restore `load_baseline`'s read | `findings.len()` | remove |
| `baseline::update_baseline_applies_the_set_just_written` (A1.6; in-process, `ws-d8-commit-in-tx` like `update_baseline_round_trips_committed_file`, `tests/cli/gate_suppress_baseline_differential.rs:345-362`, which checks only the written file): `--baseline <tmp> --update-baseline` with (a) no file, (b) a malformed file; stdout has zero findings and the exit is clean in both, as today (`run.rs:584-598`: written, then read back) | apply `inputs.baseline` instead of the written set | (a) `findings.len()`; (b) the exit code (2, from the malformed read) | restore |
| `inputs::walk_error_fails_the_build` (A1.7; two cases in a tempdir: (a) a directory symlink loop at the root, skipped where the platform cannot create one; (b) an unreadable subfolder holding one `.al`, made by the §8 recipe, on the Windows dev machine and on Linux). In each: `al_walk` is `Err`; the `build_program_with_coverage(&inputs)` `Err` text (the step-1 entry; step 1b's commit changes the call to `ResolvedProgram::build(&inputs, Summary)`, same assertion) equals a FULL literal captured from today's code on the same input (for (b) it ends `IO error for operation on <P>: <os>: <os>`; the tempdir path is substituted into the literal). The OS text differs by platform (`Access is denied. (os error 5)` on Windows, `Permission denied (os error 13)` on Linux), so each case holds one literal per platform behind `#[cfg(windows)]` / `#[cfg(unix)]`, each captured from today's code on that platform at the parent commit and recorded in the ledger. The Linux literal and the Linux discrimination run come from WSL2 Ubuntu on the dev machine (`wsl -d Ubuntu`, cargo 1.96, default user uid 1000, so `chmod 000` is enforced; root would bypass it): a clone of the parent commit and its `target/` on the WSL ext4 filesystem, not `/mnt/u` (drvfs does not enforce Unix modes), `TREE_SITTER_AL_PATH` pointing at a grammar clone at the same commit, the tempdir under `/tmp`; the capture is a scratch test outside the repo's protected paths that prints `{e:#}`, and is not committed; step 1's commit writes the captured text into the test. CI (`ubuntu-latest`) then runs the `#[cfg(unix)]` arm on every PR. A platform with no captured literal fails to compile, it does not skip; `inputs.model_instance_id` is `None`; `compute_workspace_diagnostics` returns the provider rows alone. A loop alone could not see a lost link: walkdir's loop error has no `source()` (`error.rs:215`) | (1) make `read` store `Ok(())` and the partial list; (2) store `e.to_string()` and rebuild the error from that text | (1) the three assertions; (2) case (b)'s literal (the last `: <os>` is gone) | restore |
| `cli_write_failure::analyze_closed_pipe_exits_2` (spawn with piped stdout, drop the read end before the child writes, wait) | restore `println!("{out}")` | exit code 2 and the stderr line | remove |
| `cli_write_failure::digest_out_unwritable_exits_1` (`--out` into a directory that is a file) | restore the `al-sem: digest: write error:` text | the `al-sem: write error: <path>:` line (exit stays 1) | remove |
| A1.3: no test of its own. There is no fallback to pin (§4.2.4); `inputs::objectless_file_removed_after_read` above is the pin that the index rows come from the build | — | — | — |

### 8.3 Step 1b

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `graph_export_golden` (existing, `cli_graph_export.rs:702-716`) | — (the fixture has no known `conf` tie, so an order swap passes it; the two rows below pin the order) | — | — |
| `project::add_keeps_first_via_on_conf_tie` (unit in `project.rs`'s tests: two hand-built `E`s with the same `(from, to, kind)`, equal `conf`, different `via`, added in order) | change `new.conf < old.conf` to `<=` (`project.rs:287`) | the kept `via` is the first one's | restore |
| `resolved::all_edges_is_edges_then_dependency_bodies` (unit: a `ProgramBody` with one hand-stated edge in each set; asserts the order of `body.all_edges()` AND of `body.edge_view(p).all_edges()`) | swap the two sets in `all_edges`; separately, swap the two slice fields inside `ProgramBody::edge_view` | the yielded order | restore |
| `project::cross_set_tie_keeps_the_first_sets_via` (unit on `project.rs`'s test builder, `:1351-1383`, which gains a `dependency_body_edges` list; it pins the USE: `project` itself iterates `inp.edges.all_edges()`, §4.3.3). Hand-stated tie across the two sets, from one dependency routine R: in `edges`, R's event-flow edge with an unresolved route (`project.rs:333-335` sets `unknown`; a `Publisher` has no bounds, `:406`, so `:444-452` adds `(R, Hub::Any, Direct, Open)` with via `"unresolved"`; `from` is `Key::Routine(e.from)` for both, `:325`); in `dependency_body_edges`, R's `DynamicOpen` call site with an `OpenBound::Any` site fact (via `"dynamic"`, `:408-409`, same key at `:432-441`). Assert the `Hub::Any` edge's via is `"unresolved"` | in `project`'s `engine_edges`, iterate `dependency_body_edges` before `edges` | the kept via (`"dynamic"`) | restore |
| `graph_export_covers_dependencies` (existing, `:597-650`; checks presence and `sourcePath`, not order) | feed `project` `edges` only (drop `dependency_body_edges`); separately, delete only the dependency merge of site facts inside `ResolvedProgram::build` (today `report.site_facts.extend(dependency.site_facts)`, `full.rs:2177`), keeping the workspace's. Dropping ALL site facts would also fail on the workspace facts alone and prove nothing about the merge. The projection reads site facts for `run` edges and open bounds (`project.rs:383-406`); if the dependency fixture has no dependency call site with a run target or an open bound, the second break stays green, and the test gains a dependency-side `Codeunit.Run` plus an assertion on its `run` edge until it fails | the dependency node's `sourcePath`/edges; the dependency-side `run` edge | restore |
| A1b.2: no unit test. `attach_program_calls_with` consumes the program (amended by code review r1; the original design took `program.body.take()`, whose `body.is_none()` afterwards was true by construction and pinned nothing), so a read after the release is a compile error; the break "keep a clone" does not compile (`ProgramContext` derives no `Clone`, `full.rs:1489`). The claim is that the body's memory is released at `b3.ctx_drop` (`program_calls.rs:674-676`); A1b.4's post-attach peak is the measurement of that | — | — | — |
| `resolved::dependency_bodies_histogram_is_the_dependency_call_sites` (unit on a variant of the dependency workspace whose counting rule is trivial: the test writes its own source-bearing dependency (`symbol_app::write_source_app`, as `cli_digest_query_dependencies.rs:89-110` does) with one codeunit holding `procedure A() begin B(); B(); end;` and an empty `procedure B()`, and nothing else that makes a call site (no record method, builtin, trigger, event or `Codeunit.Run`); the workspace calls `A` once. So the dependency source has exactly two `CallSite` obligations, both plain calls to the dependency's own `B`, and the hand-stated literal is the histogram with `total` 2 and `resolvedSource` 2, every other bucket 0. `export_from_program`'s `meta` `dependencyBodies` histogram equals that literal (the break adds the workspace's one call to `A`, so `total` becomes 3). The shared dependency fixture is not used, because its one dependency call site is a record `Insert` whose bucket a reader would have to copy from the output; the §4.3.2 claim is then pinned on the production output, not on two computations inside the test) | in the production filter (`impact_graph/mod.rs:222-230`, the `dependency_edges` filter over `all_edges`), drop the `from.app != primary` test | the histogram literal (workspace call sites are counted too) | restore |

### 8.4 Step 2

The `session::` tests build their `View` over `empty_resolved()`. Today that fn is
private to `registry.rs`'s test module (`registry.rs:1088`), and `test_support` is a
private `#[cfg(test)] mod` of `l5` (`src/engine/l5/mod.rs:158-159`), which
`src/engine/session.rs` cannot reach. Step 2 moves `empty_resolved` into
`test_support` as a `pub fn` (the `Model` literal of §4.2.4's `registry.rs:1098` row
moves with it) and makes the module `#[cfg(test)] pub(crate) mod test_support;`.
`registry.rs`'s tests call the moved fn.

| Test | Break | Fails at | Revert |
|---|---|---|---|
| `cli_build_counters` at the §4.4.8 values (existing test, table edited) | re-add `compute_tx_context_map`'s context build in digest | `ctx == 1` | remove |
| `l4::bundle_equals_the_old_builders` (TRANSIENT, §5: unit on `ws-d8-commit-in-tx` and the dependency workspace: `build_l4` vs `build_db_effect_bundle` / `cross_app_db_effect_bundle` compared through a test-only `SummaryBundle::rows_for_test()` rendering every routine's `db_effects`; lands with `build_l4` beside the old builders, deleted in the commit that deletes them; pass and break recorded at a named intermediate SHA, §5) | pass empty `leaf_summaries` to `build_l4` on the cross-app case | the rendered rows | restore |
| `session::base_graph_equals_context_graph` (TRANSIENT, §5: unit, cross-app: `base.graph == build_combined_graph(..) + injected`, `PartialEq`; deleted with the context's own graph build; pass and break recorded at a named intermediate SHA, §5) | drop the injected extend on one side | equality | restore |
| `session::after_run_bits_must_be_in_the_union` (A2.10, unit: `prepare` on `Plan { runs: [[a detector with requires: 0]], after_run: TRANSACTION_SPANS }` over `View::new(AnalysisTarget::SingleApp(Box::new(empty_resolved())))`) | remove the assertion | the expected panic | restore |
| `session::command_plans_declare_after_run` (A2.10, the factories: for each of `digest_plan` and `events_plan`, build the plan from one probe detector with `requires: 0`, over `empty_resolved()`; `prepare` panics naming the plan's bit). It pins the factories only; the next row pins their use | set `after_run: 0` in `digest_plan`; separately in `events_plan` | the expected panic of that case | restore |
| `digest_cli::tx_context_without_d9`, `prove::tx_context_without_d9`, `events::{fanout,chains}_without_d1_d9` (A2.10, the USE on the command path, `--lib` because the inner forms are `pub(crate)`, §4.4.4: run `digest_pipeline_with` / `prove_pipeline_with` on `tests/r0-corpus/ws-d8-commit-in-tx` with one changed routine (so the no-input check passes and `prepare` is reached) and the default set minus every detector whose `requires` holds `TRANSACTION_SPANS` (d9 today), and `events_fanout_with` / `events_chains_with` on `tests/r0-corpus/ws-event-fanout` with the default set minus every detector whose `requires` holds `SUMMARIES` or `TRANSACTION_SPANS` (d1 and d9 today); each test first asserts that the reduced set's union lacks the bit, so the precondition is stated, not assumed, then expects `prepare`'s panic naming `TRANSACTION_SPANS` / `SUMMARIES`). Without it, a command that stops calling its factory (an equal plan with `after_run: 0`, or another command's factory) stays green: the default set's union already holds `TRANSACTION_SPANS` (d9) and `SUMMARIES` (d1) (`detectors/mod.rs:1242-1294`, `registry.rs:141`), and a missing span map reads as `"unknown"`, not an error (`digest_cli.rs:906-914`) | add a temporary `digest_plan_no_after_run` to `session.rs` (`digest_plan`'s body with `after_run: 0`) and call it from `digest_pipeline_with` instead of `digest_plan`, leaving `digest_plan` unchanged; separately the same in `prove_pipeline_with`; separately an `events_plan_no_after_run` called from `events_fanout_with`, then from `events_chains_with` | the expected panic (none comes; the run completes) | restore |
| `session::no_lazy_fill_in_this_file` (A2.9, unit: `include_str!("session.rs")` minus `//` lines, needles built with `concat!`) | add an `OnceLock` field to `View`; separately, a `RefCell<Option<DetectorArtifacts>>` field (the second needle family); separately, make `released` an `AtomicU8` (the third) | the `contains` assertion | remove |
| `session::prepare_fills_ordering_before_the_runs` (unit, replaces the deleted `declared_ordering_facts_are_filled_before_the_parallel_loop` and `ordering_facts_are_lazy_and_parity_with_direct_compute`, §4.4.2: a `View` over `empty_resolved()`, one probe detector with `requires: ORDERING_FACTS` that reports `c.ordering.is_some()` from inside the loop) | move the ordering fill in `prepare` after the runs | `["filled"]` (the probe reports `"empty"`) | restore |
| `registry::undeclared_ordering_facts_read_inside_the_loop_fails_loudly` (existing, rewritten to the shim, §4.4.2) | replace `get_ordering_facts`' `expect` with a fall-back to an empty map | the `substrate::ORDERING_FACTS` diagnostic | restore |
| `session::a_second_prepare_never_refills_or_widens` (unit, rule 1's asserts (a) and (b); two cases on `empty_resolved()`: `prepare` at a mask, `release(Det)`, `prepare` again → panics naming `Det`; `prepare` at a narrow mask, then a plan whose union adds a bit → panics naming the bit) | delete assert (a); separately, delete assert (b) | the expected panic of that case | restore |
| (no `fill_inside_detector_loop_is_refused`: a detector cannot reach a fill, rule 2, so such a test could only call the fill helper directly, which pins the helper and not a use) | — | — | — |
| `session::run_diagnostics_follow_the_run_mask` (unit: one view over `empty_resolved()`, one plan with two runs, masks with and without `CORE_SUMMARIES`, filled by `View::prepare_with_diagnostics(&plan, vec![d])` with one hand-stated `SummarizeDiagnostic` `d`). The seam is needed: no fixture produces a `SummarizeDiagnostic` (§4.4.7), so a real solve gives an empty list and the test would pass vacuously; and a pre-filled `l4` would break rule 1(c). `prepare_with_diagnostics` is `#[cfg(test)]` and goes through the same private `prepare_inner` as `prepare`, which passes `d` as `L4Request.extra_diagnostics` into the one `build_l4` call inside `build_detector_artifacts` (§4.4.1), so the test drives the production fill and run path, not a helper. Assertions: the CORE run's `summarize_diagnostics == [d]` (the seam reached the run: the non-vacuity check), and the other run's is empty | take the diagnostics from `det.demanded` instead of the key | the empty assertion | restore |
| `gap_detector_substrate_parity` widened (§4.4.6) | make d1 return `d1_cohort_index: None` when `ctx.demanded & TRANSACTION_SPANS != 0`. The bit must differ between the two contexts: the full one is `ALL \| ORDERING_FACTS` and carries `TRANSACTION_SPANS` (`registry.rs:141`), d1's minimal one is its `requires`, `SUMMARIES \| CORE_SUMMARIES \| CLOSED_WORLD_TEMP` (`detectors/mod.rs:1248-1250`), which does not. (A `RAW_INHERITED_FACTS` break never fires: no context of this test carries that bit) | the `DetectorOutput` equality on `ws-d1` | restore |
| cli-b prove goldens (existing) are the pin for A2.6. No separate test: a test of how `compute_digest_effects_cli` filters roots (`digest.rs:4094-4112`) would pin the helper, not prove's call | pass `Some(from_ref(&other_root))` at prove's call (a root that is not the requested one) | the prove golden bytes (the requested root's entry is missing) | restore |
| `run::no_id_row_runs_one_build_and_keeps_the_text` (id-less `app.json` + unreadable `.al`; `build.program == 1` from the trace; `fresh` `Err` text equals the literal captured today). This test and `run::build_failed_row_runs_one_build` below are SPAWNED-process tests in `tests/cli/cli_build_counters.rs` (the `run::` prefix names the code they pin, not their crate), for the reason `inputs::read_once` is (§8.2). Spawned, `fresh`'s `Err` text is visible only on stderr: `empty_output_result` puts it in the preflight message (`run.rs:884-897`) and `alsem` prints it as `al-sem: warning: <msg>` (`alsem.rs:1612`), so the text assertion reads that line. Today this row already runs ONE build (no id, so no cross-app base; `run.rs:370-376`, `:285-307`), so this test pins the text and the profile, not a deleted build | call `ResolvedProgram::build` a second time on the no-id row | the count | remove |
| `run::build_failed_row_runs_one_build` (A2.7 row 2: cross-app analyze on an `app.json` WITH an id plus an unreadable `.al`, built as in the test above; `build.program == 1` from the trace (today 2: the cross-app build, then the single-app rerun at `run.rs:387`); exit 2; stderr is `analysis failure — program engine build failed: <why>` (`run.rs:494-498`) with `<why>` the build's own error, which contains `reading source <abs>` (`provider.rs:38-39`); the assertion is a `contains`, since the full text carries the OS message) | restore the single-app rerun | the count | remove |
| `l4::warning_text_is_rendered_once` (unit: a hand-stated `SummarizeDiagnostic` → `render_warnings(..) == ["warning: summarize: Summary fixed-point did not converge for SCC [a, b]"]`). Stated limit: it pins the rendering, not the print; the one print site in `build_l4` and the one at `capability_cone.rs:3215` are pinned by nothing executable (no fixture caps, §4.4.7) | change the prefix | the literal | restore |
| A2.3, A2.4 (measurements, §6.3) | — | — | — |

## 9. Carried findings: what this document resolves, what it hands on

| Finding | Resolved here | Handed to |
|---|---|---|
| G-1 / R6-1 (instrument half) | `PD_SOLVE_NANOS`, `PD_SOLVE_CALLS`, `build.pd_solve`; `solve_bytes` split into arena vs working set; the solve inside the §4.1.6 timings; `SGL_SCAN_OWN_NANOS` for the own-entry population | step 6: retain `resolved` in view identity (budget = `PD_SOLVE_BYTES_WORKING`) or re-solve (cost = `PD_SOLVE_NANOS`), and re-budget §5.6/§5.12 |
| G-2 (step-2 half) | §4.4.4: digest's and prove's snapshot is composed after `det` is released; the tx map is a small projection taken before the release (Decision 56); `snapshot.<field>` census buckets size the `ViewTree`-equivalent | step 6: which `ViewTree` parts a digest-only plan keeps, and the digest fallback rung |
| G-3 (instrument half) | `BFS_WALK_NANOS` / `BFS_FOLD_NANOS` / `BFS_PICKS`; the numbers file names the command each is measured on (analyze cross-app, default set) | step 7a: its bound reads the walk half and names the command |
| G-5 | §6.1: per-timer band = max − min of the parent's interleaved runs; §6.2 records whether each median is ≥ 2 × its band | steps 7a, 7b, 10, 11 apply the rule before writing a bound |
| G-6 | §4.3.4: the command's `Arc<InputSet>` (shared by the builder-local `ProgramMeta`) holds the workspace text (5.09 MB on CDO); the `inputs` census bucket | — |
| R6-2 | §4.4.6: all four fields of `DetectorOutput`; `D1CohortIndex` and `LoopSetRegistry` derive `PartialEq` | — |
| R6-3 | §4.4.5: the no-id row keeps one build, under today's single-app `Summary` profile (`ResolvedProgram::build(inputs, Summary)`, review B13; never `FULL`); `fresh` is its `Result`; the `Err` text is pinned by a test; the second build is gone and counted | — |
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
4. `build_detector_context[_with]` survive as shims returning `OwnedContext`, used by
   the tests and `aldump` (`switch_dump.rs:120`, `run_detectors*`) (§4.4.2); step 4's
   spec says when the parity test and `aldump` move to `build_detector_artifacts` and
   they are deleted.
5. (For step 12, recorded now.) `InputSet` mixes WORKSPACE inputs (`app.json`, the
   walk, the closure, roots config, coverage yaml) with REQUEST inputs (`policy_file`,
   `baseline`, `snapshot_inputs`, `build_only`, §4.2.1). Two batch jobs on one workspace
   with different `--policy` or `--baseline` cannot share one `InputSet`, yet the epic
   keys a view by (workspace, universe, `--no-roots-config`) and calls the frozen set
   the identity (epic §2, §5.1, step 12). Step 12's spec splits the request inputs out
   (or keys on them). Steps 0-2 run one request per process, so nothing here depends on
   it.

## Review log

Two reviews of this document (2026-10-18), every finding checked in source at
`U:/Git/al-sem-issue-160-a1` before the fix. A = the implementability review, B = the
memory/speed/scheduling review.

| # | Finding | Verified at | Outcome |
|---|---|---|---|
| A1 | Step 1 changed `--update-baseline`: the baseline is written (`run.rs:584-591`) and then read back (`:597`), so today every current finding is dropped and a malformed old file is overwritten before any read; reading the `InputSet` bytes would apply the old file | `run.rs:584-598`; `gate_suppress_baseline_differential.rs:345-362` checks only the written file | Fixed: §4.2.3 `load_baseline` row applies the set just written and never reads the slot under the flag; A1.6 + `baseline::update_baseline_applies_the_set_just_written` pin stdout and the exit code |
| A2 / B4 | No slot for a failed `.al` walk; a partial list could reach the build | `source_text.rs:63-65`, `:85`; `model_instance_id.rs:83`; `provider.rs:34-35`; `workspace_diagnostics.rs:93-95` | Fixed: `al_walk: Result<(), String>` (§4.2.1); the three readers fail closed with today's texts (§4.2.3); `ClosureEntries` defined; A1.7 + `inputs::walk_error_fails_the_build`. **Superseded** by a2-r3 F1: the slot keeps the `io::Error` (`Arc<std::io::Error>`), since a `String` lost the error chain |
| A3 | `app_json: Result<AppJson, String>` could not tell unreadable from malformed; readers differ | `snapshot.rs:193-197`; `snapshot_full.rs:819-827`; `workspace_diagnostics.rs:145-151`; the text reaches output at `run.rs:884-886`, `:494-497` | Fixed: `app_json: FileInput` + `app_json_value: Option<Result<Value, String>>`; each reader's message named (§4.2.1, §4.2.3) |
| A4 | Exit 2 on every command collides with digest's "no roots" and prove's "unresolved routine"; the epic says "two sites exit 1" | `digest_cli.rs:1652`; `prove.rs:950`; `alsem.rs` write-failure sites: `:731`, `:755`, `:872`, `:910`, `:992`, `:1132`, `:1200`, `:1327-1339`, `:1404`, `:1477`, `:1496` (eleven, all exit 1); epic §5.10 `:318-320`, Decision 49 `:490`, step-1 row `:514` | Fixed: one text, exit = the command's failure code (analyze 2, others 1, as today) (§4.2.5, §7, A1.4); the epic's Decision 49, §5.10 and step-1 row amended to that rule and the real count — an epic-level decision changed on a verified miscount |
| A5 | A2.9's grep gate needs `scripts/ci-steps`, which the issue flow may not edit | `CLAUDE.md:873` | Fixed: a `--lib` test over `include_str!("session.rs")` (A2.9, §8.4) |
| A6 | Nothing pinned the edge order; `graph_export_covers_dependencies` checks presence, not order, and the fixture has no `conf` tie | `cli_graph_export.rs:597-650`; `project.rs:277-294` | Fixed: `project::add_keeps_first_via_on_conf_tie` + `resolved::all_edges_is_edges_then_dependency_bodies` (§8.3, A1b.3, §5) |
| A7 | A1b.2 vacuous (`body.take()`); its break does not compile (`ProgramContext` has no `Clone`) | `full.rs:1489`; `program_calls.rs:669-676` | Fixed: the test is dropped; A1b.2 now points at the span and A1b.4's measurement |
| A8 | query lost the non-convergence line (plan has no run) | `effect_query_cli.rs:207-261` → `summary_runner.rs:1811-1814` | Fixed: the line prints iff the `l4` cell was filled (§4.4.7, §4.4.4 query row) |
| A9 | §4.2.4 named one `fresh` `Err` case; there are more, and most never parse | `snapshot.rs:193-197`; `provider.rs:34-39`; `run.rs:300-326`; `workspace_diagnostics.rs:49-95` | Fixed: the cases listed and the one that parses isolated; A1.3's fixtures named |
| A10 | §4.2.3 kept policy validation at `:230` (move at step 12), §4.4.4 moved it above the build at step 2 | `pipeline.rs:196-241` | Fixed: at step 2 validation runs after the build and before `prepare`; the error is reported in today's order, so no precedence changes; Decision 7's move stays at step 12 (§4.2.3, §4.4.4) |
| A11 | `write_stderr` used in §5 but never defined | spec §4.2.5 vs §5 | Fixed: defined in §4.2.5 (no message, the stream is gone; the command's failure code) |
| A12 | The tx map moved from `ALL` to the plan's union mask with nothing pinning `TRANSACTION_SPANS` in it | `digest_cli.rs:543-544`; `prove.rs:843`; `detectors/mod.rs:1294` (d9 in the default set); `registry.rs:53-56` (the bit forces summaries) | Fixed: `Plan.tx_map`; `prepare` asserts the bit; A2.10 + `session::tx_map_plan_requires_transaction_spans`. **Superseded** by D-M3: `Plan.after_run: u32` and the test `session::after_run_bits_must_be_in_the_union` |
| B1 | The parity test calls `(det.run)` directly at `ALL` (no ordering bit) and `every_detector_alone_declares_what_it_reads` relied on `run_each`'s fill; both break once `get_ordering_facts` `expect`s | `gap_detector_substrate_parity.rs:114-121`; `registry.rs:141`, `:613-621`, `:1002-1007`; `d47.rs:379`, `d49.rs:123`, `d51.rs:167` | Fixed: the shims fill ordering for any mask with the bit; the parity masks gain `ORDERING_FACTS` (§4.4.2, §4.4.6) |
| B2 | A thin function cannot return a `DetectorContext<'v>` that borrows artifacts it built | 47 call sites in 17 files; `d17.rs:504`, `d50.rs:619`, `:784` are struct literals | Fixed: the shims return `OwnedContext` with `ctx()` (§4.4.2) |
| B3 | analyze kept `det`/`l4` alive through projection and formatting; today the context dies in the runner | `registry.rs:413-416`, `:526-529`; `detector_context.rs:1499-1505`; `run.rs:522-700` | Fixed: `release(Det)`, `release(L4)`, `release(Ordering)` right after `run(selected)` (§4.4.4) |
| B5 | `IN_CELL_BUILD` contradicted itself; its test could pin only the helper; the types already forbid a fill from a detector | `registry.rs:318` (detector signature), `:540-566` | Fixed: the flag and the test are dropped; one `!in_detector_loop()` assertion at `prepare`'s entry (§4.4.3 rule 2, §5, §8.4) |
| B6 | Two step-0 tests were `--lib` tests over process-wide `OnceLock` flags and global atomics; the 64 MB bound was absolute | `perf_trace.rs:402-407`; `detector_context.rs:348-429` | Fixed: both are spawned-process tests; the peak assertion is `0 < post_attach <= process` (§8.1) |
| B7 | `COMPOSE_REST_NANOS` undefined over accumulating atomics and three compositions | spec §4.1.1, §4.1.3 | Fixed: charged per call from entry snapshots (§4.1.1) |
| B8 | `L4Analysis` kept the ~730 MiB harvest alive through the query phase; today only the bundle survives | `detector_context.rs:1440`; `effect_query_cli.rs:219`; `prove.rs:964` | Fixed: `L4Harvest` moves into `det` by value; `L4Analysis` keeps scc, field index, bundle, diagnostics, warnings; no `Arc` fields (§4.4.1, §4.4.2). **Superseded:** `L4Harvest` by the unharvested `CoreSummaries` (a2-r1 2-8; a `prepare` local since a2-r2 R1-8, a local of the builder that calls `build_l4` since a2-r4 1), and "no `Arc` fields" by `scc: Arc<SccResult>` (D-M2) |
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

Round a2-r1 (2026-10-18): two reviews in `critiques-child-last.md` (first review =
ids 1-1..1-5, second = 2-1..2-10). Every claim checked in source at
`U:/Git/al-sem-issue-160-a2` (head `5eca5bb5`, no `src/` or `tests/` change since
`1e54624a`). None refuted.

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| a2-r1 | 1-1 | important | fixed: §4.1.3 d47 row 1/4/1/1/3/1/1/1, §4.4.8 1/3/1/1/3/1/1/1 | `detectors/mod.rs:1575` (`requires: ORDERING_FACTS`); `registry.rs:454-461`; `detector_context.rs:914` (`need_summaries`), `:1296` (CORE gate on Tarjan and v2); `digest.rs:4016-4019` (the ordering snapshot) |
| a2-r1 | 1-2 | important | fixed: `release(Graph)` after the last graph reader in every §4.4.4 row but query; paragraph after the table; A2.4 and §6.1/§6.3 add `fingerprint` and `digest --single-app` peaks | `registry.rs:413-416` (context dies in the runner); `fingerprint_cli.rs:350`, `:364`, `:403` |
| a2-r1 | 1-3 | minor | fixed: §6.3 expectation (prove and events each save about one context build, same order as digest); Goal 4 names events' deleted context | `src/engine/gate/events.rs:422-426` (`ALL` context); `prove.rs:843`, `:964` |
| a2-r1 | 1-4 | minor | fixed: `InputSet::read` returns `Arc<InputSet>`; commands pass `&Arc<InputSet>` (§4.2.1, §4.2.6, §4.4.3) | spec §4.2.6 vs §4.3.1 (`ProgramMeta.inputs: Arc<InputSet>`) |
| a2-r1 | 1-5 | minor | fixed differently: the struct and its four `build*` methods keep their shape; one `build_with_inputs` method is added, so none of the 28 literal sites changes (§4.2.6, §4.2.3 row 1, §2, §5) | `grep "SnapshotBuilder {"`: 22 in `src/` (10 in `snapshot.rs` tests, `#[cfg(test)]` at `:395`), 6 in `tests/`; `snapshot.rs:130-135`, `:142-182` |
| a2-r1 | 2-1 | important | fixed: the parity break keys on `TRANSACTION_SPANS` (in `ALL`, not in d1's `requires`) | `registry.rs:141` (`ALL`); `detectors/mod.rs:1248-1250` (d1 requires); spec §4.4.6 (no RAW context) |
| a2-r1 | 2-2 | important | fixed: `phase_peak_is_reported` is `#[cfg(windows)]`; A0.4, §4.1.4, §6.1 say the peaks are Windows-only; no Linux reader added | `perf_trace.rs:173-176` (`#[cfg(not(windows))] … None`); `.github/workflows/ci.yml:22` (`ubuntu-latest`) |
| a2-r1 | 2-3 | important | fixed: `COMPOSE_REST_NANOS` is `AtomicI64`, computed in `i64`, printed signed; the break adds the call's whole `elapsed` to `SINGLETON_NANOS`, so `rest = −Σ < 0` | `detector_context.rs:348-424` (unsigned atomics) |
| a2-r1 | 2-4 | important | fixed: new `run::build_failed_row_runs_one_build` (id + unreadable `.al`, count 1, today 2); the no-id test's break is now a second build, and its note says it pins text and profile | `run.rs:370-376` (no id → no base), `:285-307`, `:387` (rerun), `:494-498` |
| a2-r1 | 2-5 | minor | fixed: §4.2.4 names `aldump --switch-dump` as the third caller (build first, pass its `files_without_object`); §7 step 1 states the dump change | `switch_dump.rs:47`, `:51`; `run.rs:788`, `:1092`; no `tests/` reader of the dump |
| a2-r1 | 2-6 | minor | fixed: A1.1 and `inputs::read_once` expect 2 for `diff ws ws` | spec §4.2.6 (one `InputSet` per side) |
| a2-r1 | 2-7 | minor | fixed: covered by 1-2's `release(Graph)`; the `l4.scc` co-residency in `digest --single-app` is stated and measured (A2.4); §4.4.1's prove claim corrected | `digest_cli.rs:1580`, `:1626` (`.2`); `prove.rs:964` (`let (_graph, _scc, bundle)` keeps both) |
| a2-r1 | 2-8 | minor | fixed: `build_l4` returns the unharvested `CoreSummaries`; the harvest stays in `build_detector_artifacts`; query never harvests. Epic §5.8 amended (Round 8) | `detector_context.rs:1427-1495` (harvest), `:1257`, `:1473` (`uncertainty_edges_by_from`, a context field); `effect_query_cli.rs:251` (`_map`); `summary_runner.rs:1079-1083` |
| a2-r1 | 2-9 | minor | fixed: the break drops only the dependency site-facts merge; the test gains a dependency `Codeunit.Run` assertion if that break stays green | `full.rs:2177`; `project.rs:383-406` (site facts feed `run` edges and open bounds) |
| a2-r1 | 2-10 | minor | fixed: `prove::wrong_root_set_is_entry_missing` dropped; the cli-b prove goldens are the pin, with a break at prove's own call | `digest.rs:4094-4112` (the helper's filter) |

Round a2-r2 (2026-10-18): two reviews (R1 = adversarial review, ids R1-1..R1-11; R2 =
second review, ids R2-1..R2-12). Every claim checked in source at
`U:/Git/al-sem-issue-160-a2` (head `5eca5bb5`). None refuted. R1-2 = R2-5 and R1-4 =
R2-1 (the R1 row holds the fix); R2's note on B8's stale wording is part of R1-11.

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| a2-r2 | R1-1 | important | fixed: §4.1.1 gating rule (per-entry counts folded locally, every new charge via `add_gated`, `COMPOSE_REST` snapshot under `enabled()`); A0.1 gains a use-level check (census-off CDO cone wall ≤ parent + band, §6.2); §5 | `detector_context.rs:431-433` (`add` ungated), `:439-443` (`add_gated`, the 11.0 → 12.8 s note), `:454-456`; `capability_cone.rs:1882-1885` (today's per-routine fold `c_entries`) |
| a2-r2 | R1-2 | minor | fixed: §4.4.2 deletes `declared_ordering_facts_are_filled_before_the_parallel_loop` (+ its probe) and replaces it with `session::prepare_fills_ordering_before_the_runs`; `undeclared_…_fails_loudly` kept, rewritten to the shim, runs in release; §7 step 2 and §8.4 list both; §1 and §7 allow these named `--lib` changes | `registry.rs:613-621` (the fill), `:892-903` (`c.ordering_facts.get()`), `:926-951`, `:962-982`; `detector_context.rs:831-846` |
| a2-r2 | R1-3 | minor | fixed: §4.2.3 / §4.2.6 / §7: the banned list gains `project_coverage_inputs`; the S2a selection test builds through an `InputSet` | `switch_dump.rs:355-377` (`project_coverage_disk` needle), `:384-428` (`build_program_with_coverage(&root)`, `select_program_files(&root, &ctx)`) |
| a2-r2 | R1-4 | important | fixed: option (b), an `inputs: &InputSet` PARAMETER on `attach_program_calls[_with]` (3 callers, none in `tests/`); `AppSetSnapshot` gains no field, so the LSP holds no InputSet (§4.2.4, §5); the `*_from_program` chain takes `&InputSet` in place of `&Path` (§4.2.3) | `snapshot.rs:61-68` (`apps`, `workspace_app`, `world`; `Clone, Debug`); `program_calls.rs:641-658`, callers `run.rs:357`, `program_calls.rs:727`, `:898`; `workspace.rs:1431`, `:1457`, `:1636-1671` |
| a2-r2 | R1-5 | minor | fixed: §4.4.2 states `graph` is the one field whose type changes (`Arc<CombinedGraph>`); detector reads are field reads or borrows (auto-deref); `&*ctx.graph` where needed; literals write `Arc::new` | `detector_context.rs:610` (`pub graph: CombinedGraph`); `d7.rs:143`; `d1_dataflow.rs:3812` |
| a2-r2 | R1-6 | minor | fixed: `PolicyInput { origin, file }`; `read` applies today's resolution (absolutized path, exists-first, no fall-through on an unreadable `.yaml`); `load_policy_from_input` reads `text()` and keeps `failed to read <path>: <e>`; `policy explain` keeps the disk reader (§4.2.1, §4.2.3) | `policy_loader.rs:323-331`; `pipeline.rs:55-61`, `:72-79`, `:106-114` (`absolutize`), `:341-369` (explain) |
| a2-r2 | R1-7 | minor | fixed: §4.3.3: graph's id check stays AFTER the build and reads `inputs.app_id()`; the same rule as today's empty `guid`; no stderr change | `impact_graph/mod.rs:191-204` (comment `:194-198`: after the build on purpose) |
| a2-r2 | R1-8 | minor | fixed: rule 1: each cell filled at most once per `View` (`released` bits; a refill panics); `CoreSummaries` is a `prepare` local, never a field, so it cannot outlive the call; a CORE `det` fill must share the `l4` fill's `prepare`; test `session::a_second_prepare_never_refills_or_widens` | spec §4.4.3 (`l4.zip(core.take())`), §4.4.1; `detector_context.rs:1440` (~730 MiB) |
| a2-r2 | R1-9 | minor | fixed in the EPIC: §5.9 rule 3 item (8) and Decision 47 run the policy engine at item 8 from step 6 only; before that after `release(Det)` off the RAW local (epic round 9) | epic `:313-314`, `:507`; spec §4.4.4 policy row; epic I5, Decision 50 |
| a2-r2 | R1-10 | minor | fixed: §4.2.3 row 1 keeps `walk_al_source`'s rules: no `.al` → `Ok(None)` → `SymbolOnly`; blake3 over the texts in order; nested files included | `provider.rs:33-58`; `snapshot.rs:236-244` |
| a2-r2 | R1-11 | minor | fixed: the step-2 paragraph moved below the §4.1.2 table (the three rows are back inside it); B8 and A12 point forward to their replacements | spec §4.1.2; rows B8, A12 vs a2-r1 2-8, D-M2, D-M3 |
| a2-r2 | R2-1 | important | = R1-4 | — |
| a2-r2 | R2-2 | important | fixed: §4.4.5 `CrossAppFailure { Build(String), Assembly { fresh }, NoPrimaryApp { fresh } }`; both builders return `Result<_, CrossAppFailure>`; the other 19 call sites use `.ok()`; a fourth row for no-primary-app (unreachable from step 1, today's fixed-string outcome kept fail-closed) | `capability_cone.rs:3433-3446` (`Option`, `.as_ref()?`); `program_calls.rs:822-823` (`.ok()?`), `:856` (`)?`); `run.rs:370-402`; `workspace.rs:1774`, `:1790-1800` |
| a2-r2 | R2-3 | important | fixed: `derive_pd_arena` charges `PD_SNAPSHOT_BYTES` only; old `solve_bytes=` = arena + working + snapshot, printed and stated in the numbers file; `bytes_split_sums_to_the_old_total` is TRANSIENT against the still-present `solve_bytes` (no hand-stated literal) | `snapshot.rs:1602-1606`; `detector_context.rs:411-422`, `:545-557`; `pd_solve.rs:453-456`, `:465-500` |
| a2-r2 | R2-4 | minor | fixed: `View::run` returns `Arc<RunOutput>` | spec §4.4.3 (`runs: BTreeMap<RunKey, Arc<RunOutput>>`), §4.4.4 analyze row (`release` after `run`) |
| a2-r2 | R2-5 | minor | = R1-2 | — |
| a2-r2 | R2-6 | minor | fixed: A2.9 strips `//` lines and checks `OnceLock`, `OnceCell`, `LazyLock`, `Lazy<`, `Cell<`, `Mutex`, `RwLock`, `static mut` (`Cell<` avoids the file's `enum Cell`); the cross-module limit is stated (rule 4) | spec A2.9, §4.4.3 rule 4, `pub enum Cell` |
| a2-r2 | R2-7 | minor | fixed: rule 1 assert (b): a filled `det` must cover the new union, else panic naming the bits; same test as R1-8 | spec §4.4.3 rule 1; A2.5 |
| a2-r2 | R2-8 | minor | fixed: policy row releases `L4` right after `Det`, before the RAW build (`l4: None`) | spec §4.4.4; `registry.rs:63-70` (raw cone) |
| a2-r2 | R2-9 | minor | fixed: §4.1.7 records the `scc_stats` event; after step 2 `build_l4` emits it on both arms | `detector_context.rs:1314-1355`; epic §9 7a row ("step 0's SCC distribution") |
| a2-r2 | R2-10 | minor | fixed: the shims are not `#[cfg(test)]`; `switch_dump.rs:120` is named as a direct `aldump` caller | `switch_dump.rs:120` (`build_detector_context(&resolved, 0)`) |
| a2-r2 | R2-11 | minor | fixed: §7 step 0 lists the two protected stale references for the user (`byte-census/SKILL.md:46`, and also `perf-probe/SKILL.md:46`, found by the same grep) | `grep C1_CONE_CENSUS` over `.claude/` in the main checkout |
| a2-r2 | R2-12 | minor | fixed: §4.4.4 preamble: today prove's snapshot overlaps both its contexts; step 2 lowers that, so the prove peak claim is conservative | `prove.rs:897`, `:902`, `:958` |

Round a2-r3 (2026-10-18): two reviews (F = first review, F1..F5; S = second review,
S1..S11). Every claim checked in source at `U:/Git/al-sem-issue-160-a2` (no `src/`,
`tests/` or `crates/` change since `1e54624a`). None refuted. F2 = S5 (the F row holds
the fix). One correction to a finding's count: S6 said 14 `Model` literals in 10 files;
the grep finds 12 in 8, which is what §4.2.4 lists.

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| a2-r3 | F1 | important | fixed: `al_walk: Result<(), Arc<std::io::Error>>`; the build wraps `anyhow::Error::new(Arc::clone(e))`, so `{e:#}` keeps today's chain (the OS text twice); A1.7 gains an unreadable-subfolder case (Unix) and compares the FULL literal; second break "store `e.to_string()`" fails it (§4.2.1, §4.2.3, A1.7, §8.2) | `source_text.rs:85` (`e.into()`); walkdir-2.5.0 `error.rs:212-217` (`source()` = inner OS error), `:215` (loop: `None`), `:253-261` (custom `io::Error`); `provider.rs:34-35`; `full.rs:2147` (`{e:#}`); `run.rs:884-886` (reason = `fresh` `Err` text) |
| a2-r3 | F2 | minor | fixed: `ordering_facts_are_lazy_and_parity_with_direct_compute` is DELETED, named in §4.4.2, §7 step 2 (three `--lib` changes) and §8.4 (covered by `prepare_fills_ordering_before_the_runs`) | `detector_context.rs:1700-1725` (`ordering_facts.get().is_none()`, `get_ordering_facts()` on `ALL`); `registry.rs:141` (`ALL` lacks the bit) |
| a2-r3 | F3 | minor | fixed: the one print site is `build_l4` itself, which the shims also call, so `--switch-dump` and the r4 findings projections keep their line; §4.4.7, rule 1, §7 step 2 | `summary_runner.rs:1811-1814`; `switch_dump.rs:126`; `finding.rs:952`, `:1049` (`run_detectors`, `run_detectors_cross_app`) |
| a2-r3 | F4 | minor | fixed: the edit appends ` // al-sem-ignore <detector>` (the finding's own id) to the END of line L, so no line shifts and the directive matches; the ledger records the break run lowering `findings.len()` | `inline_suppression.rs:38-50` (`\s+([\w-]+)` required), `:242-244` (id and line match) |
| a2-r3 | F5 | minor | fixed: §9 R6-3 says the no-id row's one build is the single-app `Summary` build (B13) | spec §4.4.5 row 1; `run.rs:370-395` → `:285-307` |
| a2-r3 | S1 | important | fixed: buckets are lower bounds tagged `exact`/`shallow`; no heap-size trait for the IR or graph (their sizes are probe deltas at the existing marks); step 0 adds a `census_hook::mark` per bucket point and extends `census-probe`'s `model` mode to the cross-app analyze path; the numbers file computes residual = live − Σ and checks Σ ≤ live; A0.5 rewritten (§4.1.5) | `src/bin/alsem.rs:28` (mimalloc); `tools/census-probe/src/main.rs:42`, `:232` (counting `GlobalAlloc`), `:1066` (`q6`); `src/census_hook.rs:11-19`; `full.rs:1810`, `:1831`, `:1850` (marks) |
| a2-r3 | S2 | important | fixed: one form for both steps: the model builders take `inputs` and build `ResolvedProgram` INSIDE (as today); step 1b keeps `Option`, step 2 changes only the return type to `Result<_, CrossAppFailure>` (§4.3.3) | `program_calls.rs:816-823` (`build_cross_app_program(workspace).ok()?`); `capability_cone.rs:3432-3441` (the base calls the builder) |
| a2-r3 | S3 | important | fixed: §4.1.6 splits `build_r3a3_source_only_base` into an `_over(resolved, graph)` form; the mode passes `base.graph`; calls = the cut `base.resolved.calls` (today's snapshot ledger input); coverage = the snapshot's own uncertainty fold over that graph, not `base.direct_coverage`; the cut-graph numbers come from the same mode | `capability_cone.rs:3456-3467`, `:3564` (cut), `:3540-3548` (unfolded), `:3993-4010` (`R3a3SourceBase`), `:4015-4101`, `:4021` (graph), `:4037` (fold), `:4082` (`RawOnly`) |
| a2-r3 | S4 | important | fixed: one `fill_ordering(model)` helper on `big_stack_pool().install`, called by `View::prepare` and the shim (§4.4.2, §4.4.3 rule 1) | `registry.rs:600-621` (pool + the recursion note); no `big_stack` in `src/bin/aldump.rs` |
| a2-r3 | S5 | minor | = F2 | — |
| a2-r3 | S6 | minor | fixed: §4.2.4 lists all 12 literals (11 test, 1 production); `run.rs:863` gets `vec![]` (its rows come from `compute_workspace_diagnostics` in the same function; the empty model feeds only `format_html`); §7 step 1 lists them | `grep "dependency_diagnostics:"` and `grep "Model {"` over `src/`, `tests/`; `run.rs:854-872` |
| a2-r3 | S7 | minor | fixed: `InputNeeds::build_only` (used by `SnapshotBuilder::build*` only) skips the `app.json` walk, `nested_app`, the id, roots/coverage/policy/baseline/hash rows; command readers assert `!build_only`; step 1 records the LSP `build_full` bench (§4.2.1, §4.2.6, §6.3) | `source_text.rs:145-169`; `full.rs:1718-1734` (two builds per call); `lsp/snapshot.rs:599-604` |
| a2-r3 | S8 | minor | fixed: §5 says the history is not the proof under a squash merge; the ledger records each TRANSIENT test's pass and break at a named intermediate SHA, quoted in the PR body; review checks it (no `scripts/` change) | CLAUDE.md autonomous-flow exception (squash-merge after gating HEAD) |
| a2-r3 | S9 | minor | fixed: `profile`, `apps`, `workspace_app`, `abi_integrity` dropped from the step-1b `ProgramMeta` until a reader exists; epic §5.2 annotated (epic round 10) | spec §4.3.3 (the only `meta.` readers); `AppMeta` undefined |
| a2-r3 | S10 | minor | fixed: §8 recipe: one helper `tests/common/unreadable.rs` (`#[path]`-hoisted for umbrellas and `--lib`); Unix `chmod 000` with a read-back check (root → skip); Windows `share_mode(0)` handle for files, folders skip; a skip is never a ledger pass | `source_text.rs:116-124` (lossy decode), `:80-84` (`NotFound` skipped), `:87-89` (non-files) |
| a2-r3 | S11 | minor | fixed: rule 1 fills `ordering` FIRST (model-only; its own snapshot dies before `graph`/`l4`/`det` exist); §7 step 2 states the stderr order change on a capped SCC | `digest.rs:4016-4019` (`compose_snapshot` inside the fill); `ordering_facts.rs:218-219` (model only) |

Round a2-r4 (2026-10-18): two reviews in `critiques-child-a2-r3.md` (first review = ids
1..6, item 3 split into 3a/3b; second = M1..M7). Every claim checked in source at
`U:/Git/al-sem-issue-160-a2` (head `5eca5bb5`). None refuted. M3 = 6 and M5 = 5 (the
first-review row holds the fix). Epic amended for 1 and M4 (epic Round 11).

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| a2-r4 | 1 | important | fixed: `build_detector_artifacts` takes `l4: Option<L4Request>` and calls `build_l4` itself after its cone, at today's `:1296` spot, then harvests; it returns the `L4Analysis` for the cell. Rule 1, rule 1(c), §4.4.1, §4.4.2, the shim text, §4.4.4's preamble and §6.3 updated; "never raises" now holds. Epic §5.8 and §5.9 rule 3 item (7) amended | `detector_context.rs:953` (`context.capability_cones`), `:1005` (`compose_cone_over_graph`), `:1017` (`drop(outcome.pd_solution)`), `:1296-1396` (SCC + v2 solve after the cone), `:1387` (`core_summaries`), `:1427-1495` (harvest), `:1440` (~730 MiB) |
| a2-r4 | 2 | important | fixed: `L4Request.extra_diagnostics` (empty in production) plus a `#[cfg(test)]` `View::prepare_with_diagnostics` over the same `prepare_inner`; the test also asserts the CORE run gets `[d]` (non-vacuous). `empty_resolved` moves to `test_support`, which becomes `pub(crate)` (§8.4) | `registry.rs:808-809` (`#[cfg(test)] mod tests`), `:1088` (`fn empty_resolved`, private); `src/engine/l5/mod.rs:158-159` (`#[cfg(test)] mod test_support;`, private); spec §4.4.7 (no fixture caps) |
| a2-r4 | 3a | minor | fixed: step 0 adds `census_hook::mark("1.snapshot")` after the snapshot block of `build_program_with_coverage_profiled`; `program.parsed_ir` = `1.snapshot` → `2.parse`, nothing subtracted (§4.1.5) | `full.rs:1738` (`1.snapshot` only in `build_context_with`), `:2140-2149` (no mark on the command path), `:1810` (`2.parse`) |
| a2-r4 | 3b | minor | fixed: from step 1 the `inputs` bucket owns the workspace texts and `program.snapshot_texts` walks dependency units only (§4.1.5 bullet and table) | `src/snapshot/embedded.rs:19` (`pub text: Arc<str>`); spec §4.2.3 row 1 (same `Arc`) |
| a2-r4 | 4 | minor | fixed: one borrowed `EdgeView<'r>` (primary app, both edge sets, site facts) with `ProgramBody::edge_view` and `ProgramReport::edge_view`; `SiteLinks::build`, `join`, `site_census`, `adapter`, `resolved_calls_from_program` take it; `cross_app_demand` takes an iterator; tests keep `ProgramReport` (§4.3.1, §4.3.3, §7 step 1b) | `site_links.rs:64` (`build(&ProgramReport)`), `:121`, `:133`; `program_calls.rs:386`, `:401`, `:407`, `:622`, `:922`, `:952-967`, `:1873`, `:2083`, `:2408`, `:2468`, `:2482`; `program_calls_adapter_tests.rs:151`, `:1310` (all from `build_models`); `demand.rs:29-37` (one pass) |
| a2-r4 | 5 | minor | fixed: §4.1.6 writes `base.graph.clone()` at step 0; step 2's commit changes it to `(*base.graph).clone()` | `capability_cone.rs:3335` (`pub graph: CombinedGraph`) |
| a2-r4 | 6 | minor | fixed: digest row: the pipeline's no-input check runs right after `AnalysisTarget::build`, before `prepare`; §7 step 2 states the lost snapshot warning line on that error path | `digest_cli.rs:1572` (target build), `:1580` (snapshot), `:1590-1608` (check); `alsem.rs:1049-1058` (CLI check tests the `--diff` argument, not the file's text) |
| a2-r4 | M1 | minor | fixed: §4.2.4 names the two full `FreshCoverage` literals; §4.2.6 lists the other `build_program_with_coverage[_profiled]` callers and says they use `InputNeeds::default()`; the three path wrappers read the `InputSet` inside, so the 69 `tests/` files that call them do not change; §7 step 1 | `preflight.rs:133`, `:210` (no `..`; the other six use `..clean()`); `aldump.rs:139`; `site_links.rs:110`, `physical.rs:128`, `registry.rs:198`, `:261`, `applicability.rs:955`, `dep_summary.rs:546`, `full.rs:4436`, `workspace.rs:2838`, `:2885`, `switch_dump.rs:51`, `:414`, `cli_attribute_trivia.rs:463`; `program_calls.rs:698-790`; `grep -rl` over `tests/` |
| a2-r4 | M2 | minor | fixed: `run_analyze_with_exit_uses_the_shared_aggregation` gains the needle `.workspace_diagnostics` in the commit that adds the field (§4.2.4, §7 step 1) | `run.rs:1175-1210` (banned list `compute_workspace_diagnostics(` …) |
| a2-r4 | M3 | minor | = 6 | — |
| a2-r4 | M4 | minor | fixed: epic I2 gains the named-`--lib`-test exception; both documents name worktree `…-a2` as the base | epic I2 (`:438-440` before the edit); child line 9 and epic line 14 (`…-a1`) |
| a2-r4 | M5 | minor | = 5 | — |
| a2-r4 | M6 | minor | fixed: A2.9 and rule 4 add the needles `Atomic` and `thread_local`; §8.4 adds a third break (`released` as `AtomicU8`) | spec A2.9, §4.4.3 rule 4 (`released: u8`) |
| a2-r4 | M7 | minor | fixed: `inputs::read_once`, `run::no_id_row_runs_one_build_and_keeps_the_text` and `run::build_failed_row_runs_one_build` are spawned-process tests in `tests/cli/cli_build_counters.rs`; the `fresh` text is read from the `al-sem: warning:` line (§8.2, §8.4) | `perf_trace.rs:402-407` (tracer `OnceLock`); `run.rs:884-897` (reason → preflight message); `alsem.rs:1612` (`eprintln!("al-sem: warning: {msg}")`) |

Round panel-r1 (2026-10-18): two-model panel, reviewer slots `sol` (S1..S5) and `opus`
(O1..O12), outputs under `.agent/runs/20261018-125302-fa1912/spec-r1-*.md`. Every claim
checked in source at `U:/Git/al-sem-issue-160-a2`. S1 = O3 and S2 ⊂ O5 (one fix each).
None refuted; O4's tie example was corrected (a bound-less `DynamicOpen` site gets via
`"unresolved"`, `project.rs:444-452`; the tie needs an `OpenBound::Any`), the finding
holds. Epic amended for O2, O3, O10, O11 (epic Round 12). Register:
`.agent/issue-160/findings.json` (P1-1 … P1-15).

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| panel-r1 | S1 = O3 | important | fixed: §4.4.4 states the new overlap (`l4` bundle + SCC alive during snapshot composition in digest and prove, both universes) instead of "never raises"; it cannot be released earlier (`DbTempStates` borrows the bundle); `field_index` leaves `L4Analysis` (a `build_l4` local); A2.4 and §6.3 add prove to the peak bound | `digest_cli.rs:1580` vs `:1625-1628`; `prove.rs:897` vs `:964`; `digest_cli.rs:744-761` (`DbTempStates<'a>` borrows `&'a SummaryBundle`); `detector_context.rs:1365-1393` (field index = solver input); `capability_cone.rs:3215-3222`, `:4082-4090` |
| panel-r1 | S2 + O5 | important | fixed: `inputs::app_json_edit_after_read` rewrites `app.json` to `{}` and compares analyze json, pr-summary and fingerprint json bytes, one break per reader (5); new `inputs::app_json_paths_edit_after_read` and `inputs::coverage_yaml_edit_after_read` (§8.2) | `run.rs:122-157`; `model_instance_id.rs:75-96`; `workspace.rs:1774`, `:1789-1800`; `workspace_diagnostics.rs:44`, `:145-155`; `snapshot_full.rs:819-827`, `:868-878` |
| panel-r1 | S3 | minor | fixed: §6.1: a peak's band is the epic's 3 % of the parent median; the observed max − min applies to walls and timers only | epic §5.12 (3 %), I6 ("Peak: both numbers of §5.12") |
| panel-r1 | S4 | minor | fixed: §4.3.4: `ProgramMeta` is a builder local at steps 1b-2; readers' values are copied out; the process retains the command's `Arc<InputSet>`; §4.4.3, §5, §6.3, §9 G-6 follow | §4.3.3 (builders keep return shapes; `fresh` = `meta.coverage.clone()`); `run.rs:251-261` |
| panel-r1 | S5 | minor | fixed: step 1's `inputs::walk_error_fails_the_build` calls `build_program_with_coverage(&inputs)`; step 1b's commit switches it to `ResolvedProgram::build` | `full.rs:2123-2153` (the step-1 entry) |
| panel-r1 | O1 | important | fixed: §4.1.7: step 0 deletes the per-cone `report()` call and prints once at process exit from `alsem`'s and `aldump`'s `main` | `detector_context.rs:1150` (the only `cones_census::report()` call); `capability_cone.rs:2764-2906`, `:4015-4101` (none) |
| panel-r1 | O2 | important | fixed: A2.3, §6.3 and the epic bound digest's saving on the parent median of a new step-0 span `digest.tx_context_map` minus its band (I6), not on the P5 "21 s" | `digest_cli.rs:1616`, `prove.rs:958` (the `compute_tx_context_map` calls); epic I6, Decisions 20, 44 |
| panel-r1 | O4 | important | fixed: `project`'s `Inputs.edges` becomes an `EdgeView` read only through `all_edges`; new `project::cross_set_tie_keeps_the_first_sets_via` pins the use with a hand-stated cross-set tie; `all_edges_is_edges_then_dependency_bodies` also covers `edge_view` | `project.rs:32`, `:322-325`, `:333-335`, `:406-452` (keep-first at `:287`) ; test builder `:1351-1383` |
| panel-r1 | O6 | important | fixed: §8 recipe gains a Windows folder recipe (deny ACE via `icacls … /deny *S-1-1-0:(RD)`, restored by `/remove:d`); A1.7 case (b) runs on Windows and Linux | spec §8 recipe (was "A folder: returns `None`"); §6.1 (Windows dev machine) |
| panel-r1 | O7 | minor | fixed: each command's `Plan` comes from a named production function (`digest_plan`, `events_plan`, …) taking the detector set; new `session::command_plans_declare_after_run` calls them with a set lacking d9 and d1 | `detectors/mod.rs:1242-1251` (d1), `:1290-1295` (d9); `registry.rs:141` |
| panel-r1 | O8 | minor | fixed: §8 recipe states the `#[path]` rule (top level only, one `../` per directory) with two worked paths | Rust Reference, `path` attribute |
| panel-r1 | O9 | minor | fixed: the test compares `export_from_program`'s `dependencyBodies` histogram to a hand-stated count; the break is in the production filter | `impact_graph/mod.rs:222-230` |
| panel-r1 | O10 | minor | fixed: §10 item 5 and epic open question 15 record the request-vs-workspace split for step 12 | spec §4.2.1 (`policy_file`, `baseline`, `snapshot_inputs`, `build_only`); epic §2, §5.1, step 12 |
| panel-r1 | O11 | minor | fixed: epic §4 names the two causes apart | `capability_cone.rs:4019-4021`, `:4036-4055`; `detector_context.rs:935-946` |
| panel-r1 | O12 | minor | fixed: §7 lists the `CLAUDE.md` lines that go stale (Key Modules, `tests/common` list) as user follow-ups; no protected path is edited | `CLAUDE.md` Key Modules and Testing sections |

Round panel-r2 (2026-10-18): the same panel re-marked P1-1 … P1-15 (outputs
`.agent/runs/20261018-125302-fa1912/spec-r2-*.md`). sol re-raised P1-10, opus re-raised
P1-7; both raised the same epic-propagation gap as N1 (one entry). Every claim checked
in source at `U:/Git/al-sem-issue-160-a2`. None refuted. Epic amended for P1-7 and P2-1
(epic Round 13).

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| panel-r2 | P1-7 (opus re-raise) | important | fixed: A2.3, §6.2, §6.3, the epic step-2 row, E15 and I6: the saving is a difference of two whole-command walls, so the bound also subtracts band(digest wall) (max − min of the parent's digest wall runs); I6 states the rule for a deleted sub-phase | P5 digest walls 63.70 vs 66.81 s; the head has no `digest.tx_context_map` span (step 2 deletes the call, `digest_cli.rs:1616`) |
| panel-r2 | P1-10 (sol re-raise) | minor | fixed: §4.4.4: `Plan`'s fields are private to `session.rs` (a literal plan outside it does not compile); each command's pipeline is a public wrapper over a `pub(crate)` inner form that takes the detector set; new `--lib` use tests `digest_cli::tx_context_without_d9`, `prove::tx_context_without_d9`, `events::{fanout,chains}_without_d1_d9` run the inner form with a reduced set (union asserted to lack the bit) and expect `prepare`'s panic; the break calls a temporary no-`after_run` factory from the command, leaving `digest_plan` unchanged | `digest_cli.rs:906-914` (a missing span map reads `"unknown"`, no error); `detectors/mod.rs:1242-1251`, `:1290-1295`; `digest_cli.rs:1558`, `prove.rs:880`, `gate/events.rs:369`, `:520` (the entries) |
| panel-r2 | P2-1 (sol N1 = opus N1) | important | fixed: epic §5.8 drops `field index` from `L4Analysis`; §5.9 rule 3, Decision 56 and the G-2 row state the `l4` overlap during composition instead of "no co-residency" / "phase peak flat" | child §4.4.1, §4.4.4; `digest_cli.rs:744-761` (`DbTempStates` borrows the bundle) |
| panel-r2 | P2-2 (opus N2) | minor | fixed: §4.1.7: a `pub fn report_cones_census()` wrapper in `detector_context` for the binaries; `perf_trace::maybe_exit_after` calls `cones_census::report()` before its `process::exit(0)` | `detector_context.rs:344` (`pub(crate) mod cones_census`); `src/engine/l5/mod.rs:54`; `src/engine/perf_trace.rs:622-629`; no other `process::exit` in `src/` outside `main.rs` |
| panel-r2 | P2-3 (opus N3) | minor | fixed: A1.7 and the §8.2 row: one `#[cfg(windows)]` / `#[cfg(unix)]` literal per platform, each captured at the parent commit on that platform | Windows `os error 5` vs Unix `os error 13` texts |
| panel-r2 | P2-4 (opus N4) | minor | fixed: §4.3.3: `Inputs.primary` and `Inputs.site_facts` are deleted; `project` reads both from the `EdgeView` | `project.rs:24-39` (fields), `:385`, `:402`, `:1025` (readers); `impact_graph/mod.rs:210-220` |
| panel-r2 | P2-5 (opus N5) | minor | fixed: the histogram test writes its own dependency (`A` calls `B` twice, nothing else makes a call site); literal `total` 2, `resolvedSource` 2; the break makes it 3 | `cli_digest_query_dependencies.rs:89-110` (the shared fixture's one dependency call site is a record `Insert`); `impact_graph/mod.rs:222-230` |

Round panel-r3 (2026-10-18): the same panel accepted all 20 entries P1-1 … P2-5 at
register hash `8b4d5415b7068bfe` (outputs `.agent/runs/20261018-125302-fa1912/spec-r3-*.md`)
and raised three new minor findings. Each checked in source; none refuted. Epic amended
for P3-2 (epic Round 14).

| Round | Id | Severity | Outcome | Evidence |
|---|---|---|---|---|
| panel-r3 | P3-1 (sol N1) | minor | fixed: §4.4.3's `Plan` block declares its four fields without `pub`, matching §4.4.4's rule (private to `session.rs`, built only by the factories); every field read in the spec (`plan.need_l4`, `plan.after_run`) is inside `session.rs` | child §4.4.3 `Plan` block (was `pub runs … pub after_run`); §4.4.4 privacy paragraph |
| panel-r3 | P3-2 (opus N3-1) | minor | fixed: epic I6 withdraws the "8 % of the command median" band for every bound; a whole-command wall's band is max − min of the parent's wall runs, as child A2.3 and §6.3 already use | epic I6 (kept 8 % "only for whole-command bounds" next to max − min); P5 digest walls 63.70 vs 66.81 s (3.1 s spread vs ≈ 5 s at 8 %) |
| panel-r3 | P3-3 (opus N3-2) | minor | fixed: the §8.2 `walk_error_fails_the_build` row names the Linux capture route: WSL2 Ubuntu on the dev machine (cargo 1.96, uid 1000), parent-commit clone and `target/` on ext4 not `/mnt/u`, tempdir under `/tmp`, a scratch uncommitted capture test printing `{e:#}`; CI `ubuntu-latest` runs the `#[cfg(unix)]` arm per PR. No protected path is edited | `wsl -l -v` (Ubuntu, WSL 2); `wsl -d Ubuntu -- cargo --version` = 1.96.0, `id -u` = 1000; `.github/workflows/ci.yml:22` (`runs-on: ubuntu-latest`); child §6.1 (Windows dev machine) |
