# #160 child spec: step 12 (#277) — `alsem batch`: one model, several commands

Status: spec, attempt 1, round 2, 2026-10-24. Base `1134bed1` (master; step 6 merged at `b028ec32`, #274;
grammar `c89c2ecb`, #278). Worktree `/work/al-sem-issue-277-a1`. Made from the pre-draft
`/work/bench/160-12/draft-step12-design.md` after re-reading every cited line at `1134bed1`. Probe results
and their evidence: `.agent/issue-277/ledger.md`, `## Assumption probes`. Round 1's findings (S1-S14, O1-O15)
and what was done with each: `.agent/issue-277/findings.json`; §0.1 lists what they changed.

Parent: `docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` (the epic): §9 row **12**;
§5.9 rules 1-5; §5.10; §5.11; §5.12; E1, E12, E13, E15, E16; I1-I10; Decisions 7, 12, 14, 26, 30, 36, 49,
50, 51. Step 6 spec: `2026-10-23-issue-160-step6-one-cone-graph-design.md`. Step 6 numbers:
`docs/measurements/2026-10-24-issue-160-step6.md`. Step 9 spec (in review, names may move):
`/work/al-sem-issue-160-a10/docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md`.
Where this document and the epic disagree, the epic wins, except the amendments of §10 (12A-*).

Citations are `file:line` plus the line's text at `1134bed1`, so they can be found after the code moves.

**Step 12 is a SPEED step.** It carries a wall bound (B1a, B1b), peak bounds (B2, B3, B2u, B2o), a
no-regression bound for every rewired standalone command (B4) and E16's memory and throughput bounds
(B5-B7).

Words (each defined once):
- **Job**: one command line inside a batch. **Standalone**: the same command line run as its own process.
- **Batch**: an ordered list of jobs that runs in ONE process. **Worker**: one process running one batch.
  **Run**: one `alsem batch` invocation; it runs one or more batches on up to `W` workers.
- **View key**: (workspace path made absolute, universe, `--no-roots-config`). One `View` per key per batch.
- **Witness pass**: the one composition over `rel.cone()` that fills `caps` and, under a non-empty witness
  request, `raw` (the subjects' raw inherited facts). `build.cone` counts every composition;
  `build.witness_pass` counts only compositions that kept raw facts (`capability_cone.rs:3221` `if
  !raw.is_empty() {`).
- **Witness request**: which routines get raw facts in that pass (`session.rs:89` `pub enum WitnessRequest {`).
- **Tree**: a `CapabilitySnapshot` composed from the pass (`compose_tree`). A **tree job** reads one: digest
  (case A); digest, prove, `fingerprint --routine`, `fingerprint --inventory-only` (case B).
- **Cut reader**: a job that composes its own single-app snapshot on the cut base (`compose_snapshot` /
  `compose_full_snapshot`): fingerprint, prove, diff in case A; fingerprint B0 and diff in case B.
- **Case A** = step 12 merges BEFORE step 9. **Case B** = step 12 merges AFTER step 9.
- **`JobOutput`**: `{ actions: Vec<WriteAction>, exit }` of one job: its writes, in the order its standalone
  run makes them, held as bytes until the job is written (D2.3). It generalises the epic's
  `{ stdout, stderr, stderr_first, files, exit }` (§5.10): `stderr_first` is "the list starts with a stderr
  action" (12A-15).
- **`W`**: the number of workers a run uses at once (D12).

## 0. What the probes changed in the pre-draft

1. **The pd arena is taken by the first tree** (`snapshot.rs:1397` `let cone_pd =
   caps.pd.take().unwrap_or_default();`). A second tree on the same view would get an EMPTY arena and a
   different `pd_solution`, with no panic. Digest reads that arena in `enumerate_proofs` (`digest.rs:186-189`
   "the producer solve … Read only by [`enumerate_proofs`]"), so the second digest's proofs would change.
   The arena is lent to every tree but the last (D5.3). T1 break B6 pins it.
2. **The dependency calls are taken once** (`session.rs:463-472`, `b.dependency_calls.take()`, and dropped
   after the one tree, `:543` `drop(dependency_calls);`). With two tree jobs they are lent to every tree and
   dropped after the last (D5.3).
3. **P-N6: a dropped view's pages do not stack under the next view.** `diff $WS $WS` (two single-app views
   in one process) peaks at 1.11 × single-app `fingerprint` (1,631,876 vs 1,466,292 KiB, cdo profile, two
   rounds each), not ≈ 2 ×. So views built one after another (D4) cost the largest view, plus what is held.
4. **Step 9's outcome S′** (conductor, 2026-10-24): fingerprint B0 and `diff` stay single-app in case B too;
   prove, `fingerprint --routine` and `fingerprint --inventory-only` go cross-app at ≈ 3.1-3.3 GB each
   (step 9 §8.0, **estimate**). The bench's `f` (fingerprint B0) is single-app in both cases, so `ADF` has
   the same shape in case A and case B.
5. **`WitnessRequest`'s set form is agreed with step 9; this step lands it** (D5.1, §9). Two seam points
   decided with step 9's review: an empty request never means "no composition" (the tree demand carries it,
   D5.1a, T19); and no job but policy asks `primary` (fingerprint's cross-app forms ask `Roots`, D5.1's table).

### 0.1 What spec panel round 1 changed

Every finding was checked against the code at `1134bed1` before it was acted on (`findings.json`, field
`evidence`). The conductor decided Q-a to Q-d and the threads, memory and stop questions.

| Change | Findings | Where |
|---|---|---|
| A job's output is an ORDERED list of write actions, including failure short-circuits; one-job batches write in today's order | S1, O4 | D2.3, T13, T20 |
| A job's own write failure gives that job its command's write-failure code; a run-transport failure is separate; `exit_code.rs` gets a `batch` row | S2, O15a | D2.2, T5 |
| `--update-baseline` moves out of compute into `JobOutput`; write-set conflicts cover every target (intra-job aliases, shard directories); read-after-write is a usage error | S3, S4, O5 | D2.4, T5 |
| Diff contributes one side demand per side; it finalizes after its last side's view; old-side-first error order kept; `diff A A` shares one view | S5 | D8, T21 |
| Memory: outputs are written as soon as job order allows, so a worker retains across a view only outputs that wait for an earlier job; the budget covers the MEASURED manifest shapes only (a stated Limit); union-heavy and output-heavy manifests are measured | S6, S7, O9 | D6.3, D12, §7 |
| T7 separates shared-allocation dedup from legitimate union growth | S7 | T7 |
| A one-build/one-pass miss is a STOP, never a second view or pass | S8 | §8 (F13, F14) |
| Threads: `alsem batch` sets no `RAYON_NUM_THREADS` (F18 activated); the bench sets Q | S9, O10 | D12, §7 |
| T3's break is one `View` per job (counters read 2, no panic); the panic is a separate guard break | S10, O1 | T3 |
| Planning is two-phase per view: build, per-job `resolve(&Model)`, union, one `prepare`; one identity table per view | O2 | D4.2 |
| stderr inventory covers bin `eprintln!` and `log::warn!`; `log` lines are view lines, printed once per view | O3 | D7 |
| Spawned-binary seams for T1's non-convergence break and an observable PD-sensitive second digest | S11 | T1 |
| A driver-entry trace instant, so a standalone bypass fails a test | S12 | D10, T13 |
| B4 covers every rewired command and universe, diff included | S13 | §7 |
| T19 reads `witness.members` from census stderr with the census on | S14 | T19 |
| `inputs.read` is its own counter cell (once per workspace path); A12.2 names `cone` vs `witness` | O6, O7 | D10, A12.2 |
| Policy `--strict` precedence change stated | O8 | D3, T9 |
| T8 spawns a process; the peak-table override is gone (lib test over an injected table) | O11, O12 | T8, T17 |
| A graph job on a view whose model fails still gets its program and its own text | O13 | D9, T12 |
| P-N7 has a fallback when no exclusive slot is free; the ledger's §8.4 reference is §7.4 | O14 | §7.4, ledger |
| Step 9's round-1 seam (conductor relay): no kept trees, every tree read and dropped inside `prepare`; `TreeRead { Effects, FingerprintQuery, Inventory }` is a field of `TreeDemand`; fingerprint never asks `primary`; a case-B byte test over inventory, two `--routine` queries and prove | step 9 §4.5, §4.7 | D5.1, D5.2, D5.4, D6.5, §9, T22 |

Spec panel round 2 (findings S15-S18, O16-O19 and sol's re-raises of S2, O15, O2, S11, S13):

| Change | Findings | Where |
|---|---|---|
| A measured shape's peak is charged only to its heaviest standalone member's row, so `ADF` stays at `W` = 4; T17's policy cases follow the formula (1, 1, 2, 2) | O16 = S15, S16 | D12, T17 |
| T5 (d) tests analyze's write failure through an unwritable manifest `stdout` (analyze has no `--out`) | S2, O15 (re-raise A) | T5 |
| Digest's no-input failure contributes nothing; its no-resolved-root exit 2 still contributes the default run, `l4` and, under `--tests`, an empty-subject tree | O2 (re-raise B) | D4.2 |
| The non-convergence seam injects BEFORE `render_warnings` | S11 (re-raise C) | T1 |
| B4 adds the single-app rows of events, policy and query | S13 (re-raise D) | §7 Part 3 |
| The failed `--update-baseline` write's text and exit change is stated and tested | S17 | §2, D2.4, T23 |
| Zero-view jobs (file/file diff, validation failures) run in a zero-view phase | S18 | D2.2, D6 (0), T24 |
| Two stated stderr exceptions in a batch (12A-17) | O17 | Goal 2, A12.5, §10 |
| D5.3 borrows raw entries (no clone) as the design; F13 becomes checks then STOP | O18 | D5.3, §7.3, F13 |
| T7 runs default-detector jobs only, asserting one mask | O19 | T7 |
| `app_package_zip.rs` in D7's log list; `diff A A` all columns restated; the view key does not canonicalise; `pool.threads` from the new pool | O3 nit, open points 2-3 | D7, D8, T8 |
| Step 9 round 2 (`eb754df0`): every tree read's output is fully owned, so each tree drops right after its read; the release mask widens to `u16` | step 9 §4.7 | D5.4, D6.3, D6.5, §7.3, §9 |
| Round 3: in borrow mode every raw reader of `compose_tree` goes through one accessor filtered to the tree's `TreeDemand` subjects; drain mode gets an extracted own-subjects map | O20 | D5.3, T25 |

## Feasibility first

| Item (row 12) | Verdict | Evidence at `1134bed1` |
|---|---|---|
| A `batch` command | does not exist | `alsem.rs:100` `enum Commands {` has no `Batch`; no `JobOutput`/`JobId` in `src/` |
| Several commands on one view | feasible with ONE union witness request (D5) | `session.rs:405` `"a View takes at most one witness request, in its one composing prepare"`; `:427` `"a View composes once, in its first composing prepare"`. A batch does ONE `prepare` per view, so neither can fire |
| One pass at the union is exact | feasible | policy reads raw by id (`policy_engine.rs:270` `let raw = match model.raw.get(routine.id.as_str()) {`); digest's effects are per root; one subject's raw facts do not depend on the other subjects (Decision 14; `digest_rule_b_tests.rs` `fn t3_digest_scoped_equals_unscoped`) |
| A tree per job from one pass | feasible; needs a raw split and a lent pd arena (§0 items 1-2) | `snapshot.rs:1396` `let subjects ... = raw.keys().cloned().collect();`, `:1415` `derive_capability_facts(&inp, \|id\| raw.remove(id));` |
| The plan needs the model | yes: two-phase planning (D4.2) | digest resolves its roots from `view.model()` before `digest_plan` (`digest_cli.rs:1761-1780`) |
| Analyze and digest share their run | yes | analyze `run.rs:516` `analyze_plan(detectors, args.scope == Scope::Primary)` with `presets.rs:119` `Ok(crate::engine::l5::detectors::default_detectors())`; digest `digest_cli.rs:1675` `default_detectors()`, `session.rs:175` `scope_primary: true`. Same `RunKey` |
| Policy in a batch | yes; Decision 50 lapses | `session.rs:240` policy plan `witness: WitnessRequest::Primary`; step 6 kept Task 3 |
| One view builder | three to merge | analyze `build_analysis_model` (`run.rs:283`), `AnalysisTarget::build` (`run.rs:180`), fingerprint/prove/diff `assemble_and_resolve_workspace_program` (`program_calls.rs:686`). Same three functions single-app; both cross-app ones call `build_cross_app_base` |
| Graph on the view's program | yes | `impact_graph/mod.rs:193` `ResolvedProgram::build(inputs, ProgramScope::CrossApp)`; the cross-app base builds the same (`program_calls.rs:867`) |
| Bounds | feasible by projection for the measured shapes; P-N5/P-N7 confirm at Task 1 | §7.3 |

## 1. Goal

1. **`alsem batch <manifest>`** runs several analysis commands in one process. Jobs on one view key share
   ONE program build, model, relation, witness pass (at the union of the jobs' witness requests), `det`,
   `l4`, and every run whose `RunKey` they share.
2. **Every job's output equals its standalone run** (E13): stdout bytes, stderr lines (same lines, same
   order), the order of its writes, exit code, side-effect files. A job's output never depends on the other
   jobs of its batch (I4). **Two stated stderr exceptions (12A-17)**: in a batch, (a) a view's `log` lines
   (D7) and (b) a job's own `al-sem: write error:` line (D2.2) go to the worker's stderr, not the job's stderr
   file. In a one-job batch (standalone) both still land on the process stderr exactly as today.
3. **Standalone is a batch of one**: one driver, so "batch equals standalone" is one code path.
4. **A run fits the machine for the measured manifest shapes**: `alsem batch` picks the worker count from
   the 28 GiB budget and the measured peaks, and enforces it (D12; E16).
5. **It is faster**: §7's bounds.

Why: CI and the bc-dev tooling run analyze, digest and fingerprint on the same revision. Each pays the
program build, attach, context and default run again. Default analyze and digest run the SAME detector run,
so on CDO a batch of the two should cost about one digest (23.8 s) instead of 17.0 + 23.8 s (**estimate**,
P-N5 confirms).

## 2. Non-goals

- **No change to any command's standalone output.** All 8 golden targets byte-identical; histogram SHA
  equal; CDO SARIF equal (both scopes, both universes). The only stated standalone changes are Decision 7's
  precedence moves (D3) and the failed `--update-baseline` write's text and exit (D2.4, S17). The only stated
  counter changes are `diff A A` sharing one view (D8).
- **No `--threads` flag, no `--workers` flag, no thread-count code** (Decision 36). `alsem batch` neither
  reads nor sets `RAYON_NUM_THREADS`; children inherit the environment. `W` is derived (D12).
- **No parallel jobs inside a worker.** Jobs compute one after another; views build one after another.
  Parallelism stays inside each fill (rayon). Concurrency is between workers (processes).
- **No cache, no daemon, no revision key** (epic §2).
- **No `--no-roots-config` overlay on a view**: the flag is part of the view key.
- **Not in a batch**: `policy explain`, `cache prune`, the light `--project` mode, `batch` itself.
- **Case A: fingerprint, prove and diff are not moved onto the view's pass.** That is step 9's work (and,
  for B0 and `diff`, step 9's follow-up). Step 12 writes no code step 9 deletes.
- **No streaming serializer** unless the fallback ladder reaches it (Decision 51; F6).
- **No sharing of one tree between two jobs**, even with overlapping roots (decided, Q-c).
- **No memory guarantee for a manifest outside the measured shapes** (D12, Limit L1).

## 3. Acceptance matrix

Every test drives the real `alsem` binary (`tests/cli/`, spawned) unless marked lib, and states its
precondition by assertion first. Every test has a break (§5). T1 and T2 run with `ALSEM_RETAINED_CENSUS`,
`ALSEM_TRACE` unset and `RUST_LOG=off`, so a job's stderr holds only job lines (D7); tests that read census
or trace lines (T3, T6, T7, T17, T19) read them from the worker's own stderr or trace file, never from a
job's stderr.

| # | Item (issue / epic row 12) | Proof | Case A | Case B |
|---|---|---|---|---|
| A12.1 | plan; one view per key; fixed job order | D4, D5; T3, T4, T21 | yes | yes |
| A12.2 | one pass at the union (E1) | D5; T3 (cross-app view: `cone` 1, `witness` 1); T14 (lib, union); T19 (an empty request with a tree composes once; no request, no tree, no run composes zero times) | yes; a single-app view with cut readers: `cone` = 1 + one per cut reader, `witness` = one per cut reader (decided, §9) | yes; single-app views with B0 or `diff`: the same stated row |
| A12.3 | `JobOutput` writes; §5.11 batch rules (`--format auto` = Json, one `--diff -` reader, writes in job order) | D2, D3, D6; T5, T10, T20; T1's analyze job with no `--format` | yes | yes |
| A12.4 | no `--threads` | Decision 36; T8 | yes | yes |
| A12.5 | per job: stdout, stderr lines, write order, exit, files equal standalone (E13) — **the contract test over every command**. Stated exceptions in a batch (12A-17): a view's `log` lines and a job's own write-error line go to the worker's stderr | T1 (every batchable command and form, both universes, one batch vs each job standalone); T2; T20 (write order under `2>&1`); T21 (diff sides); T22 (case B tree reads, on rebase); T24 (zero-view jobs); T12 and T5 (d) pin the two exceptions; §7 Part 1 on CDO | yes | yes |
| A12.6 | one build / `det` / cone / witness pass per view; **separate passes → the counter reads 2 and the test fails** | T3: D10's rows exactly; its break builds one `View` per job, so `build.program`, `build.detector_context` and `build.witness_pass` read 2 and T3 fails on `assert_row` | yes | yes |
| A12.7 | the `Arc` census | D11; T7; §7 Part 1 | yes | yes |
| A12.8 | wall: B1a (AD < a + d) and B1b (ADF < a + d + f), decided | §7 Part 2 | yes | yes |
| A12.9 | each peak ≤ 1.15 × the largest standalone (E12) | §7 B2, B3, B2u, B2o | yes | yes |
| A12.10 | four workers under E16 (summed peak ≤ 28 GiB, decided; throughput) and the policy cap of 2 | D12; T17; §7 Part 4 (B5, B5p, B6, B7) | yes | yes |
| A12.11 | standalone commands not regressed | §7 B4 (every rewired command and universe) | yes | yes |
| A12.12 | split traces per job | D10; T6 | yes | yes |
| A12.13 | new golden family only; no existing golden moves | `scripts/check-goldens` green with no regen; the new family `tests/batch-goldens/` (T13, T20) | yes | yes |
| A12.14 | "early batch after step 2" row | void (12A-1): no early batch was built; Decision 50 lapses | — | — |
| E14 / I10 | CDO ratchets | `scripts/cdo-gate` | yes | yes |
| I2 | byte-identical standalone | 8 golden targets, `--lib`, CDO SARIF (4), histogram SHA | yes | yes |

**A miss on A12.2 or A12.6 is a STOP** (§8): the claim halts and the conductor takes it to the operator.
No fallback adds a second view or a second pass for one key.

## 4. Design

### 4.1 D1: standalone is a batch of one

Each view command is cut into five parts, in a new `src/engine/batch/` module:

```rust
pub trait Job {
    fn demands(&self) -> Vec<ViewDemand>;          // one per view the job reads (diff: one per side);
                                                   //   empty for a job with no view (a failed validation)
    fn resolve(&self, side: SideIx, model: &Model, ids: &IdentityTable)
        -> Result<PlanPart, JobFailure>;           // per view, after its build (D4.2)
    fn compute(&self, side: SideIx, view: &View) -> Result<SideDoc, JobFailure>;   // borrows the view
    fn finalize(&self, sides: Vec<Result<SideDoc, JobFailure>>) -> Doc;            // after its last view
    fn serialize(&self, doc: Doc) -> JobOutput;    // format, generatedAt, --deterministic, write actions
}
```

A one-view job has one side and a trivial `finalize`. `PlanPart` carries the job's runs (with scope),
`need_l4`, `after_run`, witness request, tree demand and the cells it reads.

- `alsem <cmd> …` parses as today, builds one job and runs the batch driver with it. The driver emits one
  trace instant `batch.driver` (`jobs`, `views`, `workers`) at entry, so a command that bypasses the driver
  fails T13.
- Each pipeline loses its "build target + `View::new` + `prepare` + `release`" prologue: analyze
  (`run.rs:513`), digest (`digest_cli.rs:1752`), policy (`pipeline.rs:205`), events (`events.rs:374`),
  query (`effect_query_cli.rs:152`, which also builds `build_cross_app_base` itself, `:147`), prove
  (`prove.rs:880`), fingerprint (`fingerprint_cli.rs:387`), diff (`diff/cli.rs:122`), graph
  (`export_workspace`). Library entry points kept for tests become one-job wrappers.
- Rejected: a separate batch driver beside today's commands. Two drivers to keep equal.

### 4.2 D2: the manifest, the run's output, writes

**D2.1 The manifest.** `alsem batch <manifest.json> [--out-dir DIR]`:

```json
{ "batches": [
  { "jobs": [
    { "argv": ["analyze", "<ws>", "--format", "sarif"], "stdout": "a.sarif", "stderr": "a.err" },
    { "argv": ["digest", "<ws>", "--diff", "-", "--format", "json"] },
    { "argv": ["fingerprint", "<ws>", "--format", "json", "--out", "fp.json"] } ] },
  { "jobs": [ … ] } ] }
```

`{ "jobs": [ … ] }` alone is one batch. Several workspaces in one batch are allowed (decided, Q-b).

- `argv` is parsed by the SAME clap `Commands` the binary uses (`Cli::try_parse_from(["alsem", …])`), so
  every flag, default and message is the standalone one. A job naming `batch`, `policy explain` or
  `cache prune`, or one clap rejects → usage error before anything runs: exit 3 (`CONFIG_ERROR`), one line
  naming the batch and job index and clap's own text. A job's OWN validation failure after a good parse
  (e.g. `events fanout` `invalid --format`, `alsem.rs` exits 1) is that job's output and exit, not a usage
  error.
- `stdout` / `stderr` (optional): where the job's streams go; default `DIR/<batch>.<job>.stdout` /
  `.stderr` (`DIR` defaults to the manifest's directory). Relative paths resolve against the manifest's
  directory; `--out`/`--shard`/`--baseline` paths in `argv` resolve as standalone (against the process cwd).

**D2.2 The run's own output and exit (decided, Q-a).**
- **Run stdout**: one JSON line per job, in manifest order (batch, then job), `{"batch":b,"job":i,
  "exit":n,"stdout":path,"stderr":path,"files":[…]}`; nothing else.
- **Run exit**: 0 when every job ran to its own exit, whatever it is; 3 for a manifest/usage error (nothing
  runs); 1 for a run-transport failure: the run's own stdout line write fails (Decision 49's text), or a
  worker dies without its lines (its jobs get `"exit":null` and `"worker":"<signal or status>"`).
  `exit_code.rs` gains a `batch` row stating this.
- **A job's own write failure** (one of its write actions fails: its stdout file, stderr file, `--out`,
  shard, baseline) is NOT a run failure: that job's remaining actions are skipped (today's short circuit,
  e.g. `alsem.rs:923` `if let Err(code) = write_stderr_lines(…) { return code; }`), its line's `exit` is its
  command's write-failure code (analyze 2, every other command 1, `exit_code.rs:6-8`), and the
  `al-sem: write error: <target>: <OS message>` line goes to the WORKER's stderr (the job's stderr file may
  be the failed target; 12A-17 (b)). Other jobs continue. T5 (c) and (d) test both kinds.
- **Zero-view jobs (S18).** A job with no demand (a `diff` whose two sides are snapshot files; a job that
  failed its own validation) runs in a **zero-view phase before the first view builds**: a validation failure
  already holds its `JobOutput`; a file/file diff loads its sides from `RequestInputs`, finalizes and
  serializes there. They are written by item (11) in job order like every other job. A batch whose every
  job is zero-view builds nothing (`build.program` 0) and still writes every job. T24 tests both.

**D2.3 Write actions, in standalone order.** `JobOutput.actions` is the ordered list of
`Stdout(bytes) | Stderr(bytes) | File(path, bytes) | Dir(path)` writes the standalone command makes, in the
order it makes them today, including a failure that ends the list. Examples at `1134bed1`: degraded analyze
writes its `al-sem: warning:` line BEFORE stdout (`alsem.rs:1664-1666`, then `write_stdout` `:1670`); policy
writes its output, then its analyzer diagnostics AFTER (`alsem.rs:921-925` "Analyzer diagnostics go to stderr
AFTER the output"); a validation error is a stderr action alone. A one-job batch (standalone) replays the
actions on the real streams in that order, so `2>&1` output is unchanged (T20 pins it against goldens
minted from the base binary). In a batch, a job's stdout and stderr go to two different files (D2.4), so
their relative order is not observable there; the order between stream and file actions still is (a failed
`--out` stops the stderr lines that follow it).

**D2.4 Every write target is checked before any build** (exit 3 on a conflict, `build.program` 0):
- the write set of a job: its stdout and stderr targets, `--out`, the shard directory (`--shard`), and
  `--baseline` when `--update-baseline` is set. `--update-baseline`'s write moves out of compute
  (`run.rs:594` `save_baseline(Path::new(path), &summaries)` is called inside `run_analyze_with_exit`
  today) into the job's `File` action, as epic §5.10 requires; the bytes come from `baseline.rs:88`
  ("Returns the exact bytes `save_baseline` would write"). The baseline `File` action comes BEFORE the
  `Stdout` action, as today's write comes before the output. **Stated standalone change (S17)**: a failed
  baseline write today returns `Err("failed to write baseline '<path>': <e>")` from compute (`run.rs:594-595`),
  which the bin prints as `al-sem: failed to write baseline …` with exit 3 (`alsem.rs:1675-1689`, the non-
  "analysis failure" branch); under the one write-failure rule (Decision 49) it becomes `al-sem: write error:
  <path>: <OS message>` with analyze's write-failure exit 2, and stdout stays unwritten in both. T23 states
  both behaviours (the base text and exit from a golden minted at Task 0, the new ones asserted);
- **two targets that are the same file** (compared after resolution: `.`/`..` folded, then the deepest
  existing ancestor canonicalized and the rest appended, lowercased on Windows; amended at the Task 8 review,
  R8-1, so a symlink, a Windows case difference or an 8.3 name is no longer a limit), across ALL jobs of the run AND within
  one job (its `stdout` and `stderr` naming one file);
- **a file inside a directory target, or two nested directory targets** (a shard directory and another
  target under it; `alsem.rs:1416-1430` creates shard files inside the directory);
- **read-after-write**: a path a job READS that another job of the run WRITES: diff file sides, `--baseline`
  without `--update-baseline`, `--renames`, `--policy`, digest `--diff <file>`, and any file a workspace read
  takes (`*.al`, `*.app`, `app.json`, the roots config, the coverage file under a job's workspace path).
  Standalone, the reader would see the earlier job's write; a batch reads every input before any write, so
  the conflict is refused, not ordered.
- `--diff -`: with one batch, read once in-process (D3). With several, the parent reads stdin once and
  passes the bytes to every child that needs them on the child's stdin.

**D2.5 Workers.** With two or more batches, the parent spawns one child per batch (`current_exe()`, hidden
`alsem batch --worker <b> <manifest>`), at most `W` at once, in manifest order (D12). Each child writes its
own jobs' actions (D6) and prints its job lines to its stdout; the parent collects them and prints them in
manifest order after the last child exits. A child's stderr carries only process lines (D7) and write-error
lines; the parent forwards it whole line by whole line, prefixed `[w<b>] `, so two children's lines never
mix inside a line (decided, Q-d). **One batch** runs in-process (no child, no prefix).

### 4.3 D3: inputs split; validation before the build (Decision 7)

- `InputSet` splits (epic open question 15): `WorkspaceInputs` (path, `app.json`, `app_json_paths`, roots
  config, coverage yaml, the closure, `al_walk`/`al_files`, `driver_version`, `model_instance_id`, the hash
  rows when a job needs them): ONE `Arc` per workspace path, read once with the union of the jobs' needs;
  `RequestInputs` (policy file and request, baseline, `snapshot_inputs` of a diff file side): one per job.
  Reading a field a standalone job would not read changes no output: the job never reads it. Task 1 lists,
  per command, which fields it reads, so the union is stated, not assumed.
- **Decision 7's moves land here**: fingerprint `--shard`/`--out` validation and the policy load
  (`pipeline.rs:223` `match resolve_policy_check(opts.inputs)`, today after `View::new` at `:205`) run in
  validation, before any build. Stated precedence changes (stderr text and exit of a doubly-broken
  invocation):
  1. a bad policy AND a failing build: the policy error, not the build error;
  2. a bad `--shard`/`--out` AND a failing build: the shard/out error;
  3. **`policy check --strict`**: a bad policy AND an error-severity analyzer diagnostic: today the
     diagnostic lines and exit 1 (`pipeline.rs:211-219`, the strict gate runs before `resolve_policy_check`);
     after the move, the policy error and exit 1. Same exit code, different stderr text.
  `diff --renames` keeps its order (Decision 7). One T9 case per change.
- **One `--diff -` reader** (§5.11): the driver reads stdin once (`alsem.rs:1101` `read_to_string(&mut buf)`
  moves into the driver) if any job says `--diff -`; every such job gets the same text.

### 4.4 D4: views, their order, their key, their builder, the plan

**D4.1 Views.**
- `ViewKey = (workspace path as spelled, made absolute, Universe, no_roots_config)`. Two spellings that
  absolutise differently are two views (stated).
- Within a batch, views are visited one after another, **in order of their first demand** (a job's demands
  in side order: diff's old side before its new side). A view is dropped after its last demand is computed;
  then the next view builds. Two views are never alive together. P-N6 shows a dropped view's pages do not
  stack (§0 item 3); F16 is the fallback if a batch shows otherwise.
- **One builder per view** returns `AnalysisModel` (the analyze builder's superset: target plus `fresh`,
  `physical`, `object_facts`); the extras are computed only when an analyze job is on the view. Each job
  maps a failed build to its own text and exit: analyze its empty-output rows (`run.rs:500`
  `Err(ModelFailure::NoModelInstanceId | ModelFailure::AssemblyFailed)`) and its build-failure text; the
  rest `AnalysisTarget::cross_app_failure_message(command, …)` (`run.rs:207`) or their own single-app text.
  Other views still run (T12). A graph job on a view whose MODEL fails still gets the program (D9). T16
  shows the three single-app builders give the same model (P-N10's runtime half).
- Cut readers (fingerprint, prove and diff in case A; B0 and `diff` in case B) are single-app jobs, so they
  land on the single-app view of their workspace. Fingerprint's builder reads `--no-roots-config`
  (`alsem.rs:1345` `no_roots_config: f.no_roots_config,`), prove's and diff's pass `false`: two keys when
  they differ.

**D4.2 Two-phase planning per view.** The plan needs the model: digest resolves its changed roots from
`view.model()` through `derive_identity_table`, `OwnerIndex` and `resolve_changed_roots` before it builds
its plan (`digest_cli.rs:1761-1780`); case B's prove (`{root}`) and `fingerprint --routine` (its pool) do the
same. So, per view:
1. **build** the view;
2. **resolve**: for each demand on the view, in job order, `resolve(side, &Model, &IdentityTable)` →
   `PlanPart` or `JobFailure`. Two digest outcomes differ here (S2-B):
   - **no input** (exit 1, "at least one of changedFiles, changedRoutines, or diffText is required",
     `digest_cli.rs:1745-1749`; `run_digest_empty_diff_builds_no_context` pins that it runs after the build
     and before `prepare`, with `build.detector_context` 0) → `JobFailure`: the job contributes NOTHING to
     the plan. The view still serves the other jobs (T1 row: digest with an empty diff next to analyze);
   - **no resolved root** (exit 2) is NOT a failure at resolve: standalone still prepares
     `digest_plan(detectors, ∅, tests)` and runs its default run, its tx map and its `l4` before the exit
     (`digest_cli.rs:1776-1831`; `session.rs:171-186` gives `Roots(∅)` + a tree with `--tests`, else an empty
     request and no tree). So it returns a `PlanPart` with the default run, `need_l4`, `after_run =
     TRANSACTION_SPANS`, an empty request, and a tree demand only under `--tests`; its compute then gives
     exit 2 as today (`digest_with_no_resolved_root_builds_no_tree`, `cli_build_counters.rs:401`, keeps its
     row: ctx 1, snap 0, witness 0);
3. **the identity table is built once per view** (the epic's `identities` cell, §5.9 item 4: "roots resolve
   from it"), only when a demand on the view resolves roots, and released when step 2 ends. Each tree still
   derives its own identity table inside `compose_tree`, as today (`snapshot.rs:1410` `let identities =
   derive_identity_table(model);`); that copy is part of the tree's content;
4. **union** the `PlanPart`s into one `Plan` (D5);
5. **one `prepare`**.

### 4.5 D5: one plan per view — the union

**Decision: the jobs' witness requests merge into ONE request, the union. The rule "a View takes at most
one witness request, in its one composing prepare" stays as it is.**

Why the union, not a changed rule:
1. The rule guards the one composition (§5.9 rule 3; E1): the pass fills `caps`, `det` consumes `caps`, and
   the dependency calls leave the base in the first prepare (`session.rs:525`). A second request means a
   second composition, which is exactly the row's "separate passes → counter 2 fails".
2. The union is exact. No reader of `raw` looks past its own subjects (policy by routine id, a tree by its
   own subject set, D5.3), and one subject's raw facts do not depend on which other subjects are in scope
   (Decision 14, `t3_digest_scoped_equals_unscoped`).
3. "One request per job" needs either several compositions (the cost row 12 removes) or one composition
   holding a raw map per request (the union's bytes plus code). Neither buys anything.

**D5.1 The request type (agreed with step 9; this step lands it).**

```rust
pub struct WitnessRequest { ids: BTreeSet<String>, primary: bool }
// scope(model): every row whose stable id is in `ids`, plus every Role::Primary row if `primary`
// union(a, b) = { ids: a.ids ∪ b.ids, primary: a.primary || b.primary }
// is_empty() = no ids and !primary: "no raw facts", NEVER "no composition" (D5.1a)
```

Today's `Roots(ids)` = `{ ids, primary: false }`; `Primary` = `{ ∅, true }`. Step 9 adds no variant; it uses
only `Roots(ids)`, which becomes `{ ids, primary: false }` on rebase (§9). T14 pins the union.

Who asks what (decided with step 9):

| Job | Request | Tree demand |
|---|---|---|
| policy check | `{ ∅, primary: true }` | none |
| digest with resolved roots | `{ roots, false }` | yes (`Effects`) |
| digest `--tests`, no resolved root | `{ ∅, false }` (empty) | yes (`Effects`; the test-coverage graph) |
| prove (case B) | `{ {root}, false }` | yes (`Effects`) |
| `fingerprint --routine` (case B) | `{ pool, false }` (the selected routines) | yes (`FingerprintQuery`) |
| `fingerprint --inventory-only` (case B) | `{ ∅, false }` (empty) | yes (`Inventory`) |
| fingerprint B0, `diff`, prove and the fingerprint query in case A | none on the view (their own cut composition, D6.4) | none |
| analyze, events, query, graph | empty | none |

**No job asks `primary` except policy.** Fingerprint is never `{ ∅, true }`: it asks its selected routines
(`--routine`) or ∅ (`--inventory-only`) (step 9 §4.7 rule 5). Its full snapshot (B0) would be the only other
`primary` reader, and it stays single-app on the cut in both cases (S′), as does `diff`. So the union of a cross-app view is policy's
`primary` bit (if policy is on it) plus the listed ids.

**D5.1a Composition is signalled by the plan, never by an empty set (decided with step 9).** An empty
request means "keep no raw facts". Whether the view composes is the plan's own question:
`compose = !witness.is_empty() || !trees.is_empty() || (a run needs derived rows and det is not filled)`.
So `--inventory-only` (`Roots(∅)` + an `Inventory` tree read) and `digest --tests` with no root (empty + a
tree) compose exactly once, as today and as step 9 §4.7 rule 2 states ("a tree demand forces the composition
whatever its request's size") (`session.rs:172` `let needs_tree = tests || !roots.is_empty();` keeps `Roots(∅)` for
the second today); a plan with no request, no tree and no run (query, graph) composes zero times. Rejected:
a distinct `None` state inside the set form; it would carry the same bit twice (the tree demand already says
it). T19 pins both shapes.

**D5.2 The tree demand is per job.** `Plan` replaces `needs_tree: bool` and `tests: bool`
(`session.rs:81`, `:84`) with `trees: Vec<(JobId, TreeDemand)>`, where

```rust
pub struct TreeDemand { subjects: WitnessRequest, tests: bool, read: TreeRead }
pub enum TreeRead { Effects /* digest; prove in case B */,
                    FingerprintQuery(QueryRead), Inventory(InventoryRead) /* step 9 */ }
```

Case A ships `TreeRead::Effects` only; `FingerprintQuery` and `Inventory` are step 9's (§9; names from step 9
§4.5, which may still move). **No tree outlives `prepare`** (step 9 §4.5: a cross-app tree is ≈ 282 MiB,
201 MB of it the pd arena): each tree is composed, its read is computed on it, and it is dropped, all inside
`prepare` before `det` takes `caps`, as digest's is today. So the producer arena is never alive during a run
(E18) and a worker holds no tree across a view boundary. Each tree job gets its OWN tree over its OWN
subjects (the epic's `trees: BTreeMap<JobId, _>`, §5.9). So a job's tree is its standalone tree by
construction. The cost is one `compose_tree` per tree job (2.2 s for `d`, step 6 §14.4), never a second
pass. The assertion `"a tree is composed over a Roots witness request"` (`session.rs:408-410`) becomes
"every tree demand's subjects are inside the view's request" (same intent: a tree never sees a subject the
pass did not compose).

**D5.3 The raw split, the pd arena and the dependency calls.** All three are consumed by today's one tree.
With several readers:
- **raw**: readers are, in order, the tree demands at item (5) in job order, then policy's rule engine at
  item (8). Each reader reads its own subjects' entries out of the ONE `raw` map, with **no clone** (O18; the
  design, not a fallback). `derive_capability_facts` is already generic over a borrowing accessor
  (`snapshot.rs:1637-1642` `take_raw: impl FnMut(&str) -> Option<R>` with `R: Borrow<[CapabilityFact]>`): the
  cut path calls it with `|id| base.raw.get(id).map(Vec::as_slice)` (`:1305`), the tree with
  `|id| raw.remove(id)` (`:1415`), and the two evidence indexes read `&raw` (`:1413-1414`). So `compose_tree`
  gets two modes:
  - **drain** (today's code inside `compose_tree`): the tree is the last reader of every one of its subjects.
    The driver first MOVES exactly those subjects' entries out of the union map into a fresh map
    (`raw.remove(id)` per subject) and hands `compose_tree` that map by value, as today. In a one-tree view
    with no policy the fresh map is the whole `raw`, so standalone digest is unchanged (B4). The extraction
    matters even in drain mode: two digests with disjoint roots are each the last reader of their own
    subjects, and the union map still holds the other's;
  - **borrow**: some subject has a later reader. **Every raw reader inside `compose_tree` walks every
    workspace routine and looks it up in the map** (O20): the subject set (`snapshot.rs:1396` `raw.keys()`,
    which also scopes the pd arena, `:1424-1431`), both evidence indexes (`:2143` `raw.get(&r.id)` in the
    operation index, `:2269` in the callsite index, each in a `for r in base.ws_routines` loop), and
    `derive_capability_facts` (`:1658` `take_raw(&r.id)` over `inp.ws_routines`). None is limited to the
    tree's own subjects, so handing them the shared union map would leak other jobs' subjects into this tree.
    So in borrow mode `compose_tree`'s subject set is **the `TreeDemand` scope (resolved through
    `WitnessRequest::scope`, D5.1) ∩ the map's keys** (O21), never the map's keys alone and never the scope
    alone. The composition inserts a raw entry only for a scoped routine it actually walks
    (`capability_cone.rs:2828` `if !routine_ids.contains(m) { continue;`, `:2950-2951`
    `raw_out.insert(m.clone(), inherited);`), so a scoped id can have no entry; today's `raw.keys()`
    (`snapshot.rs:1396`) excludes it, and so does drain's per-subject `remove`. Taking the scope alone would
    widen the pd arena's scope (`:1424-1431`) and change `pd_solution`. The tree gets ONE accessor, `SubjectRaw { map:
    &HashMap<..>, subjects: &BTreeSet<String> }`, whose `get(id)` returns `None` for any id outside
    `subjects`. All four readers go through it: the subject set and the pd arena's scope from `subjects`;
    the two indexes and `derive_capability_facts` from `get` (`|id| acc.get(id).map(Vec::as_slice)`). After
    `compose_tree` returns, the driver removes the entries whose last reader it was.
  The borrow mode keeps those entries alive for one `compose_tree` longer than a drain would; they are the
  tree's own subjects, already counted in the union's `raw` (D11). **A reader that misses the filter** (or a
  drain-mode tree handed the union map) gives the tree another job's subjects: T1 break B2 and T2 catch it
  on output; T25 catches it at unit level. Policy fails loud on a
  missing entry (`policy_engine.rs:278` `"policy: routine {} has a summary but no raw inherited facts"`), so
  a wrong move cannot pass silently (T1 break B5).
- **pd arena**: `compose_tree` takes `Cow<'_, ConeArena>`: `Borrowed` for every tree but the last,
  `Owned` (taken from `caps.pd`) for the last. A one-tree view drops the arena exactly where it does today
  (`snapshot.rs:1432` `drop(cone_pd);`), so standalone digest's peak does not move (B4).
- **dependency calls**: lent (`&[CallEdge]`) to every tree; dropped after the last tree. A view with no tree
  drops them at the start of `prepare` as today.

**D5.4 Tree reads are per job.** `effects: BTreeMap<JobId, DigestEffects>` (`take_effects(job)`), and in
case B step 9's `Cell::TreeRead` becomes `tree_reads: BTreeMap<JobId, TreeReadOut>` (`take_tree_read(job)`;
`Cell::ALL` unchanged, as step 9 §4.5 keeps it). Each read comes off its job's own tree, and **every read's
output is fully owned** (step 9 §4.7, at `eb754df0`: `FingerprintQueryResult` carries each witness's terminal
evidence; the inventory map carries its five keys, `analysisUniverse` included; `DigestEffects` already
owns its rows). So the driver drops each job's tree RIGHT AFTER that job's read, at (5), before the next
tree is composed and before `det`; no tree byte is held past its read. A read over its own subjects is what keeps `filter_by_class` from seeing another
job's facts (step 9 §4.7 rule 4; `snapshot_full/key_classes.rs:806-869`): inventory's tree holds no
inherited fact even when another job's does. T22 pins it in case B.

**D5.5 Runs.** `runs: Vec<(Vec<Detector>, scope_primary)>` (today one `scope_primary` for all runs,
`session.rs:70`). Two jobs with one `RunKey` share one `Arc<RunOutput>`. The union of masks → one `det`
(Decision 12); `need_l4` and `after_run` are the unions.

**D5.6 One `prepare` per view.** So "a View never widens a cell" and both one-composition assertions can
never fire in a batch. They stay as guards; T3's second break shows they fire.

### 4.6 D6: cell order, retention and the write point

Per view, in order of first demand (step 6's `prepare` order, `session.rs:357-366`):

```
(0) once per batch, before the first view: zero-view jobs (validation failures; file/file diff) finalize
    and serialize (D2.2); then (11)
(1) program build → graph jobs compute (D9)
(2) model, ProgramBody released, rel
(2b) resolve: identity table (once, if needed), each demand's PlanPart, table released (D4.2)
(3) ordering (if any run asks ORDERING_FACTS)
(4) ONE witness pass at the union request → caps, raw
(5) per tree demand, in job order: its tree (D5.3), its read (effects / query / inventory); tree released
(6) det (consumes caps), l4, harvest; caps released
(7) runs, one per distinct RunKey
(8) small projections off det: tx maps, fanout, policy's rule engine (det + raw); raw, det released
(9) every demand's compute, in job order → SideDoc. Cut readers compose their own snapshot here
(10) every job whose LAST demand this view held: finalize → Doc → serialize → JobOutput; Doc dropped
view dropped
(11) write: every finished job whose earlier jobs (in job order) are all written, in job order
after the last view: (11) again, for every job left
```

- **D6.1 Releases move to the driver.** Today each pipeline releases its own cells (analyze
  `view.release_all_but(&[])` after its run, `run.rs:522`; policy `release_all_but(&[Cell::Det, Cell::Raw])`;
  fingerprint `take_run` then `release_all_but(&[])`; digest `release(Cell::L4)`, `release(Cell::Effects)`).
  In a batch each cell is released after the last demand that declared it reads it (`PlanPart` declares
  reads). A one-job batch releases where the standalone does today (B4).
- **D6.2 Runs.** `take_run` becomes "the driver drops its `Arc` after the last reader".
- **D6.3 What a worker retains across a view boundary** (S6). Item (11) writes a job as soon as it and every
  earlier job are finished, so a worker holds across a view boundary only:
  (a) the `SideDoc`s of a diff job whose other side is on a later view (one side's snapshot; P-N6 sizes it at
  ≈ 160 MB on CDO), and (b) the `JobOutput`s of finished jobs that wait for an EARLIER job still on a later
  view. A manifest whose jobs are listed grouped by view holds nothing under (b). (c) (amended at Task 8,
  R7-1) the `WorkspaceInputs` of views still to come: D3 reads every workspace before the first build, so a
  later view's read exists from the start; each read is freed after its last view (a job hands its read to
  the driver in `demands` and reads it only through `ViewCx::inputs`/`ResolveCx::inputs`). The bound:
  **retained bytes at a view boundary ≤ Σ of the held `JobOutput`s + Σ of the held `SideDoc`s + Σ of the
  later views' reads**, printed under `ALSEM_RETAINED_CENSUS=1` as `[retained-census] batch.held
  outputs=<bytes> sides=<bytes> inputs=<bytes>` at each view start (`sides` counts diff's snapshot sides,
  the only side that can wait; `inputs` is each read's `inputs` census bucket, once per read). **No tree term**: every tree read's output is fully owned and each tree is dropped right after its
  read (D5.4), so held tree reads (`DigestEffects`, case B `TreeReadOut`) are part of the jobs' `SideDoc`s /
  `JobOutput`s, never a tree. CDO output sizes (step 6 Part 1 files): analyze SARIF 9.8 MB (primary), 74 MB (`--scope all`);
  digest `d` 20 MB, `h` 443 MB; fingerprint B0 JSON 130 MB. Part 2's output-heavy manifest `O` measures the
  worst case we run (§7); F5 spills held outputs to temp files if `O` misses.
- **D6.4 Cut readers.** They keep composing their own cut snapshot inside `compute` at (9), after `det` is
  released, exactly as standalone. Each is one more composition on the single-app view
  (`cli_build_counters.rs:491` `FINGERPRINT_ROW`: `bcg: 2, cone: 2, … witness: 1`). Decided (§9): a stated
  counter row, not shared code.
- **D6.5 Trees live only inside (5).** Every tree is composed, read and dropped at (5), one at a time (the
  next tree is composed only after the previous one is dropped), before `det`, as digest's is today. Step 9 does the same for prove, the query and inventory (step 9 §4.5), so no tree is
  kept past `prepare` in either merge order.

### 4.7 D7: stderr belongs to the artifact (§5.11, Decision 30)

Three classes of stderr line exist today; Task 1 inventories each, line by line, before the driver is
written:

| Class | Count at `1134bed1` | Batch behaviour |
|---|---|---|
| bin `eprintln!` in `src/bin/alsem.rs` handlers (validation and error texts, e.g. digest's stdin and diff-file errors) | 49 | **job lines**: they move behind the driver as `Stderr` actions of that job's `JobOutput`, in today's order (D2.3) |
| library `eprintln!`/`eprint!` outside `bin/`, `lsp/`, `server`, `watcher` and tests | 96 (the pre-draft's filter counted 99) | per line: a **job line** when a shared fill prints it today (it becomes a value on the cell that produced it, and each job's `serialize` replays the lines its standalone run printed: build lines always; the L4 non-convergence line iff its own mask has `CORE_SUMMARIES`; det lines iff it has a run), or a **process line** (`[retained-census]`, trace): stays on the worker's stderr, printed once |
| `log::warn!` and friends through `env_logger` (`alsem.rs:688` `env_logger::Builder::from_env(…default_filter_or("warn")).init()`), printed during the shared view build (`dependencies.rs`, `snapshot/cache.rs`, `program/model/workspace.rs`, `engine/deps/app_package_zip.rs` `:142`, `:175`, `:253`) | 12 `warn!`/`info!` sites in the first three files, 3 `log::warn!` in `app_package_zip.rs`; Task 1's inventory is exact | **view lines**: printed once per view, when the view builds, to the worker's stderr, not copied into any job's stderr. Standalone (one job, one view) they land on the process stderr exactly as today. They carry env_logger's timestamp, so no test byte-compares them: T1 runs with `RUST_LOG=off`; T12 asserts a view's `log` line appears once on the worker's stderr when two jobs share the view |

A line printed from a rayon worker mid-fill: F11. Census and trace stay on the worker's stderr or trace file
(decided, Q-d).

As built (Task 8): Task 1 found two library job lines, both printed on the driver's thread: the L4
non-convergence line (`build_l4`, rendered from `L4Analysis.warnings`) and `graph`'s coverage warning. Both
print through `batch::output::job_line`, which hands the line to the driver step running on that thread
(`capture`) and prints it anywhere else. The lines `prepare` captures are the `l4` solve's: each job whose
own plan solves `l4` (`Plan::builds_l4`) replays them once per view; the lines a `compute` or a graph
`program` captures are that job's side's. `serialize` puts a job's lines before its own writes, where
standalone printed them; a side after a failed side contributes none (D8).

### 4.8 D8: diff in a batch (S5)

- A `diff` job has two demands, old side then new side. A workspace side is a single-app demand on its
  workspace's view (`diff/cli.rs:122` `View::new(AnalysisTarget::SingleApp(..))`), in case A and case B (S′).
  A file side is a `RequestInputs` row with no view.
- Each side's `compute` returns a `SideDoc`: today's `load_side` result, `Ok((snapshot, diag_lines))` or
  `Err(msg)` (`diff/cli.rs:153-164`), plus the side's build lines. Each side composes its cut snapshot
  (D6.4).
- `finalize` runs after the job's LAST demand, on whichever view that is, and keeps today's logical order:
  old side's load error first (`diff/cli.rs:224` "failed to load snapshot"), then the new side's (`:228`);
  then the `--strict` gate over both sides' lines (`:240` `let any_error = old_diag_lines`,
  `strict_gate_lines`, old then new, once when the same workspace); then `--renames` (`:254`); then the engine and the render. When the old side failed,
  the new side's `SideDoc` and its build lines are discarded, because standalone never loads it.
- **`[diff A B, diff B A]`** visits view A, then view B (first demand). Job 1's old side and job 2's new side
  compute on A; job 1's new side and job 2's old side on B; both jobs finalize after B, each in its own
  logical side order. Held across the A→B boundary: job 2's new-side `SideDoc` and job 1's old-side `SideDoc`
  (D6.3 (a)). T21 pins it, plus `[diff A B, analyze A]` (shared workspace pairs).
- **`diff A A`** has two demands on ONE view key, so both sides compute on one view, each with its own cut
  composition. Stated counter change for standalone `diff ws ws`, every column, as (program, bcg, cone, v2,
  scc, pd, ctx, snap, witness, inputs):
  - without `--strict` (`cli_build_counters.rs:541-559`): today 2, 2, 2, 0, 2, 2, 0, 2, 2, 2 → expected
    **1**, 2, 2, 0, 2, 2, 0, 2, 2, **1**. Only the build and the input read halve; each side's cut still
    builds its own graph, cone, SCC and pd solve;
  - with `--strict` (`:563-590`; "`--strict` adds ctx 1, bcg 1, cone 1, v2 1, scc 2, pd 1 per side"): today
    2, 4, 4, 2, 6, 4, 2, 2, 2, 2 → expected **1, 3, 3, 1, 4, 3, 1**, 2, 2, **1**: the one view fills one `det`,
    so the strict half is paid once.
  These are expected rows; Task 1 reads them off a traced prototype before the test is rewritten (I2's
  exception). Output is unchanged: each side runs `compose_full_snapshot` over `view.model()`, which only
  reads the model (`diff/cli.rs:134-141`), and `--strict` already prints one list when `same_workspace`
  (`:174-183`, canonical compare) holds; cli-b diff goldens and T1 pin it. **The view key absolutises but does
  not canonicalise**: a symlinked second spelling of one workspace is two views, with today's counters and the
  same output (stated).

### 4.9 D9: graph and query

- graph reads `ResolvedProgram` only. It computes at (1) on the cross-app view's program, before the body is
  released: the view builder lends `&ResolvedProgram` to the view's graph jobs (`program_calls.rs:854`
  `pub(crate) program: ResolvedProgram,` holds it at that point). `export_from_program` keeps its scope check
  (`impact_graph/mod.rs:203`) and its `app_id` check after the build (`:213`).
- **A graph job on a view whose model fails** (assembly failure, or a missing `modelInstanceId`, which makes
  `AnalysisTarget::build` fail before building anything, `run.rs:184-188`) still gets the program: the view
  builder builds the program first whenever a graph job is on the view, hands it to the graph jobs, and only
  then attempts the model. A failed PROGRAM build gives the graph job its own standalone error text. A view
  whose jobs are all graph builds only the program, as standalone. T12 includes a graph job on the failing
  view.
- query: `into_query_parts(self)` (`session.rs:729`) consumes the view; it becomes borrows
  (`view.model()`, `view.rel().primary()`, `view.l4()`), so query shares a view.

### 4.10 D10: counters and traces

- Counters (`cli_build_counters.rs:109` `assert_row`: `build.program`, `combined_graph`, `cone`,
  `summaries_v2`, `graph_scc`, `pd_solve`, `detector_context`, `snapshot`, `witness_pass`, `inputs.read`).
  `assert_row` today checks `inputs.read == build.program` (`:120-121`); since D3 reads one `WorkspaceInputs`
  per workspace path, `Row` gains its own `inputs` cell (= program for every standalone row, as today).
  Expected batch rows (T3), as (program, bcg, cone, v2, scc, pd, ctx, snap, witness, inputs):
  - one cross-app view with analyze + digest + policy + events + query: **1, 1, 1, 1, 2, 1, 1, 1, 1, 1**
    (`DIGEST_ROW`, `:378`; policy cross-app is the same with snap 0, `:673-688`);
  - the same plus a second digest job with other roots: snap **2**, witness still **1**;
  - analyze cross-app + analyze `--single-app` (two views, one workspace): program **2**, ctx **2**,
    witness 0, **inputs 1**;
  - a single-app view with fingerprint B0 + prove (case A): `cone` = 1 (the view's run composition, no raw)
    + 2 (one cut per cut reader), `witness` = **2** (the cuts keep raw facts; the view's composition does
    not); T3 derives the other cells from `FINGERPRINT_ROW` and the `prove` row (`:435-454`) before it is
    written;
  - a view whose runs ask `ORDERING_FACTS`: + the cut composition (step 6 §14.10).
  - case B (§9, measured at Task 11): fingerprint B0 + `prove --single-app` on one single-app view:
    **1, 2, 2, 1, 3, 2, 1, 2, 2, 1**; B0 + prove cross-app on two views (`DIGEST_ROW` + `FINGERPRINT_ROW`):
    **2, 3, 3, 2, 5, 3, 2, 2, 2, 1**.
- Traces: one file per process (a child's file is per pid when `ALSEM_TRACE_FILE` is unset; when it is set,
  the parent passes `<file>.w<b>` to child `b`). The job index and command go in the span NAME
  (`job.3.digest`, `view.1`), plus an `instant_lazy` with the args (`perf_trace.rs` `pub fn span(cat:
  &'static str, name: impl Into<TraceName>)` takes no args). Every fill span lies inside a `view` span; every
  resolve, compute, finalize and serialize span inside exactly one `job` span (T6). A fill lies inside no
  `job` span, except a cut reader's own composition (D6.4: it composes inside its `compute`), whose fills
  lie inside its own job span and inside a `view` span (amended at Task 8, R7-4). `batch.driver` (D1) and
  `batch.workers` (D12) are instants.

### 4.11 D11: the `Arc` census (12A-5)

Two rules, kept apart (S7):
- **Shared allocations are counted once.** Model, `rel`, `det`, `l4`, `caps`, and each `RunOutput` per
  distinct `RunKey` are one allocation however many jobs read them; `job_refs` (how many jobs read the
  cell) is printed beside the bytes under `ALSEM_RETAINED_CENSUS=1`. Every `RunOutput` and tree handed to a
  job is an `Arc` or a borrow, never a clone.
- **Union storage grows legitimately and is stated, not capped by one job.** `raw` holds the union of the
  subjects (no clones: D5.3 borrows); `runs` holds one `RunOutput` per DISTINCT `RunKey`;
  `effects` and tree reads hold one per tree job (no tree itself survives `prepare`). These are bounded by the sum of the jobs' own buckets,
  not by the largest one.

### 4.12 D12: workers and the E16 budget (decided)

- **Threads**: no flag, no code (Decision 36; F18 activated). `RAYON_NUM_THREADS` sizes every pool
  (`big_stack.rs:65-70` builds a default `ThreadPoolBuilder` with a stack size, so it honours the variable);
  children inherit the parent's environment. The bench sets `Q` itself (§7).
- **The budget is 28 GiB** (decided: the tentacle's shared cgroup `memory.max` is 32 GiB; MemTotal is not
  reachable inside the container). A constant `E16_BUDGET_KIB = 29,360,128`.
- **Admission is explicit**:
  `W = max(1, min(batches in the manifest, 4, ⌊E16_BUDGET_KIB / P⌋))`, and **`W ≤ 2` when the manifest
  contains `policy check`** (decided; the cap is applied after the formula's `min`, in the same function, so
  it never raises `W`: with policy, `W` = min(formula, 2)). `P` = the
  largest `KIND_PEAK_KIB` row of any job kind in the manifest. Batches start in manifest order; at most `W`
  run at once. The parent emits a trace instant `batch.workers` (`w`, `p_kib`, `policy_cap`). When
  `⌊E16_BUDGET_KIB / P⌋ = 0` (no row today comes near), `W` = 1 and the parent prints one stderr line
  `al-sem batch: the largest measured job peak exceeds the 28 GiB budget; running one worker`.
- **`KIND_PEAK_KIB`** (committed table, one row per (command, universe), rounded up to the next 256 MiB)
  holds, per kind, the max of (a) that kind's standalone CDO `VmHWM` median and (b) the median peak of every
  measured batch shape whose **heaviest standalone member** is that kind (O16/S15, decided). A shape's peak
  is charged to ONE row, never to every kind in it: `U` → policy; `U−p` (U without policy) and `O` → digest
  (`h` is their heaviest member); `AD`, `ADF` → digest; `AS` → analyze cross-app. A manifest's `P` is the max
  row over its kinds, so a manifest shaped like a measured shape is admitted at that shape's peak (its
  heaviest member is in it), while a light kind is not inflated by a heavy shape it happens to be part of.
  Rejected: rows keyed by shape (they need a shape matcher for arbitrary manifests). Rows known today (release-fast, step 6 bench): analyze
  cross-app ≈ 2.99 GiB, digest ≈ 3.01-3.35 GiB, policy cross-app 7.94 GiB, fingerprint B0 single-app
  1.40 GiB, analyze single-app ≈ 0.72 GB. Part 4b measures every other row (events, query, graph, prove,
  diff, and the case B rows). A kind with no row takes the largest row (fail-safe). The table cites its
  numbers file.
- **Limit L1 (stated, not a guarantee).** The budget is enforced for manifests whose views are no heavier
  than the measured shapes: per view, at most the union of `U`'s requests and at most `U`'s tree jobs; per
  worker, held bytes (D6.3) at most `O`'s. A manifest outside these shapes (many large-root digests on one
  view, or a long list of 443 MB outputs that wait for a late job) can exceed its row. `alsem batch --help`
  says so, and that the table is CDO-calibrated: a larger workspace needs a re-measure.
- What `W` gives with the projected rows (§7.3 estimates; Part 4b replaces them): the digest row ≈ max(`h`
  3.35 GiB, `O` ≈ 3.64 GB, `U−p` ≈ 3.6 GB) ≈ 3.6 GB rounds to 3.75 GiB → no policy: `⌊28 / 3.75⌋ = 7` →
  **4** (so `ADF`, `W4`, `W4u` all run at 4); the policy row ≈ max(7.94 GiB, `U` ≈ 8.7 GB) rounds to
  8.25 GiB → with policy: `⌊28 / 8.25⌋ = 3`, capped → **2**; case B rows (prove, `fingerprint --routine`,
  `--inventory-only`, ≈ 3.1-3.3 GB, **estimate**) → **4**.
- Each worker prints `[batch] worker <b> peak_kib=<VmHWM>` (read from `/proc/self/status` at the end of its
  run; omitted where the file does not exist) to its stderr under `ALSEM_RETAINED_CENSUS=1`, so Part 4 can
  sum per-worker peaks (`RUSAGE_CHILDREN` gives the max, not the sum).

### 4.13 What is deleted

The per-pipeline target-and-view prologues (D1); `View::into_query_parts`; `export_workspace`'s own build;
the per-command `InputSet::read` calls in `alsem.rs`; the per-command `release*` and `take_run` calls (D6.1);
`save_baseline`'s call inside compute (D2.4); `Plan.needs_tree`, `Plan.tests` and `WitnessRequest`'s enum
form; every library `eprintln!` of D7's job-line list. Tests that pin deleted code are rewritten in the same
commit (I2's exception): the `session.rs` plan tests (`Plan::for_test`, `with_witness`, the `needs_tree`
asserts at `session.rs:1062-1071`), the `into_query_parts` users, and `cli_build_counters` (`Row` gains
`inputs`; the `diff ws ws` rows change per D8; batch rows added).

## 5. Test plan (every new test: break, failing assertion, revert; I9)

Fixtures: r0 `ws-*` dirs, `ws_dep()` (`cli_build_counters.rs:143`, a dependency with source),
`tests/common/dep_dispatch_fixture.rs`. Each break is made with Edit and reverted with Edit. Test-only seams
in the spawned binary exist only under `cfg(debug_assertions)` (the test profile), never in a release build.

| # | Test | Precondition (asserted) | Assertion | Break → fails |
|---|---|---|---|---|
| T1 | `batch_equals_standalone` (cli; E13; **the contract test over every command**): every batchable command and form — analyze sarif/json/html, `--scope all`, `--baseline` and `--update-baseline`; digest json/human, `--diff -`, `--tests`, an empty diff (exit 1) next to analyze; prove; fingerprint json/cbor.gz/query/human/`--inventory-only`/`--shard all`; diff ws ws, ws file, `--strict`; events fanout/chains; policy check json/sarif (two policies on one view); query touches/effects; graph — on `ws_dep()` and an r0 dir, both universes: ONE batch with all jobs vs each job standalone. Includes two digest jobs with different roots on one view, and digest + policy on one view. Census, trace off; `RUST_LOG=off` | the batch has ≥ 2 jobs per view, both universes, ≥ 2 tree jobs on one view (from the run's lines and a separate traced run) | per job: stdout bytes (`--deterministic` where the command has it), stderr lines, exit, every written file — equal | **B1**: drop the L4 line replay. Seam: a debug-only env var `ALSEM_TEST_SUMMARIZE_DIAGNOSTIC=1` makes `build_l4` push one non-convergence `SummarizeDiagnostic` (ending in `NON_CONVERGENCE_TAIL`) into `diagnostics` BEFORE `render_warnings` (`analysis.rs:104` `let warnings = render_warnings(&diagnostics);`), so the line is rendered into `L4Analysis.warnings` and replayed like a real one. Not `extra_diagnostics`: those are appended after rendering and never print (`analysis.rs:40-44`, `:105-106`); precondition: the standalone digest's stderr carries the non-convergence line (asserted non-empty). **B2**: give each tree the whole union `raw` → digest's subject set differs. **B3**: drop scope from `RunKey` → analyze `--scope all` differs. **B5**: use the drain mode (not the borrow mode, D5.3) for a digest tree when policy shares the view → policy panics on the missing entry. **B6**: give every tree `caps.pd.take()` (today's code) → the second digest job's proofs differ; precondition: the second digest's standalone JSON carries at least one proof from `enumerate_proofs` (`digest.rs:186-189`; asserted non-empty), so the arena is observable |
| T2 | `batch_output_independent_of_neighbours` (I4): job J alone vs J with every other job | J's subjects are a strict subset of its view's union (asserted) | J's `JobOutput` byte-equal | build the union from the first job only → J's tree differs |
| T3 | `batch_counters_one_per_view` (I1; "separate passes → counter 2 fails") | — | D10's rows exactly | **Break 1: one `View` per job** (each job builds and prepares its own view) → `build.program`, `build.detector_context`, `build.witness_pass` read 2, `assert_row` fails, no panic. **Break 2 (the guard): one `prepare` per job on ONE view** → the second composing prepare panics `a View composes once` / `a View takes at most one witness request` (`session.rs:400-437`) |
| T4 | `batch_views_are_sequential`: cross-app + `--single-app` jobs on one workspace | two `view` spans | the second view's first fill starts after the first view's drop instant | build every view before any compute |
| T5 | `batch_write_conflicts_and_failures`: (a) a job that fails after an earlier one succeeded; (b) two jobs with one `--out`; (b2) one job's `stdout` and `stderr` naming one file; (b3) a `--out` inside another job's shard directory; (b4) two `--update-baseline` jobs on one baseline; (b5) read-after-write: `fingerprint --out X` then `diff X ws`; `--update-baseline B` then `--baseline B`; a `--policy`/`--renames` file another job writes; (c) the run's own stdout is a closed pipe; (d) a fingerprint job whose `--out` directory does not exist, and an analyze job whose manifest `stdout` target is in a directory that does not exist (analyze has no `--out`, `alsem.rs:454-529`) | — | (a) both written in job order, the failed one with its standalone stderr/exit; (b)-(b5) exit 3, `build.program` 0, one line naming both jobs; (c) Decision 49's text, run exit 1; (d) the job line's exit 1 (fingerprint) / 2 (analyze), the write-error line on the worker's stderr, the other jobs written, run exit 0 | write at compute; drop each path check in turn; map a job write failure to the run's exit |
| T6 | `batch_trace_split_per_job` | — | every resolve/compute/finalize/serialize span inside exactly one `job` span; every fill span inside a `view` span, none inside a `job` span, except a cut reader's own composition, whose fills lie inside its own job span and a `view` span (D10, amended at Task 8) | open the first job's span around `prepare` |
| T7 | `batch_arc_census` (D11): one cross-app view with default-detector jobs only (analyze default, digest, policy, events fanout) | `ALSEM_RETAINED_CENSUS=1`; census lines read from the worker's stderr; **every job's run mask equals the union mask** (asserted from the trace's `RunKey`s: one distinct `demanded`), since `det`/`l4` are built at the union mask (O19) | **dedup**: the `model`, `rel`, `det`, `l4` buckets equal the largest standalone job's (exact bytes; the same allocation), and `runs` = Σ over DISTINCT `RunKey`s of the standalone `runs` bytes (analyze + digest sharing a key: the analyze standalone `runs` bytes once); `job_refs` = the number of reading jobs. **Union**: `raw` ≥ max and ≤ Σ of the jobs' standalone `raw` buckets (no clones, D5.3) | clone the `RunOutput` per job → `runs` doubles, the dedup assertion fails |
| T8 | `rayon_threads_size_the_big_stack_pool` (cli, spawned with `RAYON_NUM_THREADS=2` in the CHILD's environment; no `set_var`) | — | the `pool.threads` trace instant (new, not debug-only: free when tracing is off) reads 2. It is emitted by `big_stack_pool()` from the NEWLY BUILT pool (`pool.current_num_threads()` on the returned `ThreadPool`), not from rayon's free function, which reports the calling thread's pool | `.num_threads(available_parallelism)` |
| T9 | `decision7_precedence_<move>`, one per change of D3 (policy + build, shard/out + build, policy + `--strict` diagnostics) | both failures asserted separately | the stated text and exit, `build.program` 0 for the first two | validate after the build |
| T10 | `batch_stdin_read_once`: two digest jobs `--diff -`, one batch; and the same split over two batches | — | both get the diff | read per job → the second sees an empty diff |
| T11 | `batch_manifest_errors`: nested `batch`, `policy explain`, an unknown flag | — | exit 3, one line naming batch/job index and clap's text | skip the subcommand check |
| T12 | `batch_view_failure_per_job`: a cross-app view that fails to assemble its model, holding digest + policy + graph, plus a single-app view that builds; and a view with a `log::warn!`-producing dependency shared by two jobs | — | each failing job prints its own command's text and exit; the graph job exports as standalone; the other view's jobs succeed; the `log` line appears once on the worker's stderr | one shared error text; skip the program for graph on a failed model; print view lines per job |
| T13 | `standalone_is_a_batch_of_one` + the new golden family `tests/batch-goldens/` | — | for each command, the standalone trace holds exactly one `batch.driver` instant with `jobs=1`; `alsem analyze` and a one-job batch: equal bytes and counters; the family matches | **bypass the driver in analyze → no `batch.driver` instant, the test fails**. (Byte equality alone cannot tell two correct drivers apart; the instant can) |
| T14 | `witness_request_union` (lib): `scope(union(a, b)) = scope(a) ∪ scope(b)` on a model with primary and dependency rows, stated by assignment | — | equal sets | `primary: a.primary && b.primary` |
| T15 | `analyze_and_digest_share_one_run` (cli): one view, analyze default + digest | — | the trace has one `finish_run` per `RunKey` | put the job index in `RunKey` → two runs |
| T16 | `single_app_builders_agree` (lib, P-N10): the three single-app builders give equal models on r0 dirs (`--no-roots-config` both ways) | — | equal model rows | flip `skip_roots_config` in one → differs |
| T17 | `batch_worker_count`: (lib) the admission function over an injected table: `⌊budget / P⌋` = 0, 1, 3, ≥ 4 (enough batches) → `W` = 1, 1, 3, 4 without policy and **1, 1, 2, 2** with policy (the formula's `min`, then the cap: S16); (lib, the COMMITTED table, the shape-charging rule O16) the bench manifests `AD`, `ADF`, `U−p`, `O` ×4 batches → `W` 4, and `U` ×4 → 2: a shape's peak raised only its heaviest member's row, so the analyze and digest rows stay under `budget / 4`; (cli, the real table) manifests of 1, 3, 5 batches without policy and 3 batches with one `policy check` job | — | lib: the stated `W`s; cli: the `batch.workers` instant reads 1, 3, 4, **2**, and the child start/exit instants never show more than `W` children alive | drop the policy cap → 3; drop `max(1, …)` → 0; charge `U`'s peak to every kind in it → `ADF` gets 3 |
| T19 | `empty_request_composes_only_with_a_tree` (cli, counters from the trace; `witness.members` from the worker's stderr with `ALSEM_RETAINED_CENSUS=1`, `detector_context.rs:1380-1387`): (a) `digest --tests` on a change that resolves no root, alone and in a batch with analyze; (b) `query` alone and a batch of query + graph (no run, no request, no tree); (c) case B adds `fingerprint --inventory-only` | (a) the census line reads `witness.members=0` | (a) `build.cone` 1, `build.snapshot` 1, `build.witness_pass` 0; (b) `build.cone` 0, `build.witness_pass` 0 | infer "no composition" from `witness.is_empty()` → (a) `build.snapshot` 0 and the `testCoverage` block is missing; make an empty request compose → (b) `build.cone` 1 |
| T20 | `write_order_under_merged_streams` (cli, standalone, `2>&1` into one pipe): degraded analyze (the `al-sem: warning:` line before stdout, `alsem.rs:1664-1670`); `policy check` with diagnostics and `--out` (output file, then the diagnostic lines, `:921-925`); a policy job whose `--out` write fails (the stderr lines that follow are not written) | the warning / diagnostics are non-empty (asserted) | the merged bytes equal goldens minted from the base binary at Task 0 | write a fixed stdout, stderr, files order → the degraded-analyze golden differs |
| T22 | **case B only; activates on the rebase onto step 9** (`#[ignore]` with reason "needs step 9" until then): `batch_tree_reads_use_own_subjects` (cli): ONE cross-app batch of `fingerprint --inventory-only`, `fingerprint --routine R1`, `fingerprint --routine R2` (R1 ≠ R2), `prove R1 may-commit`, on `ws_split` (not `ws_dep()`: there `DoWork` writes the dependency's table 18 itself, so no output shows a shared raw set and the union break is not observable; measured at Task 11) | R1's and R2's standalone query outputs differ, and R1's cone reaches a dependency routine R2's does not (asserted), so a shared raw set is observable | each job byte-equal to its standalone run (stdout, stderr lines, exit, files) | give every tree read the whole union `raw` → inventory gains inherited facts / dependency apps (`filter_by_class`) and the R2 query gains R1's dependency rows |
| T21 | `batch_diff_sides` (cli): `[diff A B, diff B A]`; `[diff A B, analyze A]`; `diff A A`; a diff whose old side fails to load, with a new side that would print a build line | — | each job equals standalone (bytes, stderr lines, exit); the old-side error wins and the new side's lines are absent; `diff A A` reads D8's expected rows | finalize on the first view → job 2 compares half-loaded sides; replay the new side's lines on an old-side failure |
| T23 | `update_baseline_write_failure` (cli, standalone; S17): `analyze --baseline <dir-that-does-not-exist>/b.json --update-baseline` | the base behaviour, from a golden minted at Task 0 from the base binary: `al-sem: failed to write baseline '…': …`, exit 3, empty stdout (asserted from the golden) | the head: `al-sem: write error: <path>: <OS message>`, exit 2, empty stdout; the golden is kept as the record of the stated change | keep the compute-side `save_baseline` → the head still prints the old text and exit 3 |
| T24 | `batch_zero_view_jobs` (cli; S18): (a) a batch with a file/file `diff` (two snapshot files written by an earlier, separate run) between two workspace jobs; (b) a batch whose every job fails its own validation (`events fanout --format bad`, `fingerprint --shard` with a bad value) | — | (a) the diff job equals standalone and is written in job order; (b) every job's line and stderr equal standalone, `build.program` 0, run exit 0 | skip the zero-view phase → (a) the diff job has no line; (b) the run writes nothing |
| T25 | `subject_raw_hides_other_subjects` (lib, `snapshot.rs`; O20, O21): a model with three primary routines X, Y, Z; hand-stated raw entries (by assignment) for X and Y in ONE map, none for Z; (a) a borrow-mode `compose_tree` whose `TreeDemand` scope is {X}; (b) one whose scope is {X, Z} | both entries present in the map and Z absent from it (asserted) | (a) `SubjectRaw.get(Y)` is `None`; the tree's subject set is {X}; its capability facts, operation index and callsite index carry nothing derived from Y's entry; its output equals a drain-mode tree over a map holding only X. (b) the subject set is {X} (Z is in the scope but has no entry), and the output, `pd_solution` included, equals (a)'s | (a) read one of the four readers from the map directly (e.g. the operation index from `&map`) → Y's witness operation appears in the operation index. (b) take the subject set from the scope alone → Z enters the set and the pd arena's scope, `pd_solution` differs |

## 6. False-positive and failure shapes

- **A pass per job** (epic §11): T3 break 1 (counters) and break 2 (guards).
- **Output depending on the batch** (I4): per-job trees (D5.2) make it structural; T2.
- **A second tree on a taken pd arena or taken dependency calls** (§0 items 1-2): silent wrong proofs or a
  panic. D5.3; T1 B6.
- **A raw entry moved away from a later reader**: policy's own panic; T1 B5.
- **A tree alive during `det`**: D6.5; B2; the live-cell checkpoint (`session.rs:590` `view.live cells=`)
  never lists `Tree` or `PdArena` in a batch.
- **A stderr line lost, doubled, reordered, or moved across stdout**: T1 compares lines; T20 the merged order.
- **A view line printed per job**: T12.
- **A write that clobbers another job's output or a later job's input**: D2.4; T5 (b)-(b5).
- **A job write failure hidden as success, or a run failure blamed on a job**: D2.2; T5 (c), (d).
- **Two views alive at once / peaks stacking**: T4; P-N6; B3.
- **Held outputs on top of the next view's peak**: D6.3; B2o; F5.
- **Workers over the budget**: D12; T17; B5, B5p; Limit L1 names what is not covered.
- **A worker killed (OOM) reported as success**: its jobs get `"exit":null` and the run exits 1 (D2.2).
- **A request input leaking across jobs**: T1 with two policies on one view.
- **A stdin read twice**: T10.
- **Diff sides crossed between views**: T21.
- **A bound that holds only in one merge order**: B1a/B1b hold in both (decided).

## 7. Measurement plan

### 7.1 How it runs

One CDO bench after the final panel, through `agentflow run --name bench`: `release-fast`, clean detached
worktrees, `WS` from `cdo_ws` in `.agent/config.json` (never written into a file), peaks and walls by
`maxrss.py` (`/work/bench/160-10-11a/maxrss.py`: kernel `VmHWM`, mimalloc as shipped). `P` = branch base,
`H` = final-panel head. A round is void when the external load exceeds 8 (load gate before each part, as
step 9's OD-4). Runs interleave (a, d, f, s, AD, ADF, AS, U, O, a, …), never back to back of one kind. Every
run stays in the shared 32 GiB cgroup; no other memory-heavy work runs in the slot (step 9's session shares
it). `F` = step 6's `d` file (`CDOIssueDocument.Codeunit.al`); `F88` = step 6's `h` file set (88 roots).

Jobs: `a` = `analyze $WS --format sarif` (default detectors); `d` = `digest $WS --file $F --format json`;
`h` = `digest $WS <F88> --format json`; `f` = `fingerprint $WS --format json --out $T/f.json` (B0,
single-app in both cases); `s` = `analyze $WS --single-app --format sarif`; `p` = `policy check $WS
--format json` (bundled policy); `aa` = `analyze $WS --scope all --format sarif`.

Batches:
- `AD` = [a, d] (one view); `ADF` = [a, d, f] (two views); `AS` = [a, s] (two views).
- **`U` (union-heavy)** = [a, d, h, p, events fanout, query touches] (one cross-app view: the union of `d`'s
  and `h`'s roots plus policy's `primary`, two trees, D5.3's borrow mode for both digests).
- **`O` (output-heavy)** = [s, h, f, aa] (views: single-app (s, f) first, then cross-app (h, aa)): `f`'s
  130 MB output waits for `h` across the single→cross boundary (D6.3 (b)), on top of `h`'s view.

- **Part 1 (identity, counters, census)**: histogram SHA P = H; SARIF P vs H (both scopes, both universes);
  per job of `AD`, `ADF`, `AS`, `U`, `O`: stdout, stderr lines, exit, files `cmp` against H standalone
  (`generatedAt` removed); counters from the batch trace vs D10; one `ALSEM_RETAINED_CENSUS=1` run of `ADF`,
  `U`, `O` and of each standalone (D11's two rules on CDO; `batch.held` lines).
- **Part 2 (bounded)**: 5 interleaved rounds of `a`, `d`, `h`, `f`, `s`, `p`, `aa`, `AD`, `ADF`, `AS`, `U`, `O`
  at H.
- **Part 3 (B4, bounded)**: 3 interleaved rounds P vs H of every rewired standalone command, in every
  universe it has: analyze (x, s), digest `d` (x, s), fingerprint B0, prove (case A single-app; case B both),
  `diff $WS $WS` (and `--strict`), events fanout (x, s), policy check (x, s), query touches (x, s), graph
  (cross-app only). Every command with a `--single-app` flag (`alsem.rs:189-192`, `:267-272`, `:364-368`)
  gets its single-app row. Peaks and walls.
- **Part 4 (E16)**: `N` = `nproc` (22), `Q` = ⌊N/4⌋ (5), set by the bench in the environment (D12 sets
  nothing). 3 interleaved rounds of: `W1f` = `alsem batch` with one `ADF` batch, `RAYON_NUM_THREADS` unset;
  `W1q` = the same with `RAYON_NUM_THREADS=Q`; `W4` = `RAYON_NUM_THREADS=Q alsem batch` with four `ADF`
  batches (`W` = 4 derived), wall = start to last exit, Σ = sum of the four `[batch] worker … peak_kib`
  lines. Then `W2p` = `alsem batch` with four [p] batches: `batch.workers` must read 2; Σ of the two largest
  concurrent worker peaks. Then `W4u` = four [U-without-p] batches (`U` minus policy, so `W` = 4): Σ.
- **Part 4b (the peak table)**: one `maxrss.py` run per (command, universe) row of D12's table not already
  measured in Parts 2-4, 3 rounds; each row takes the max median over its standalone and every shape above
  whose **heaviest standalone member** is that row's kind (D12's rule; a shape is charged to one row, never
  to every kind it contains, O16/S15); the table cites the numbers file. Case B adds prove, `fingerprint --routine`,
  `--inventory-only` cross-app.

### 7.2 Bounds

`X` = median, `bX` = band (max − min). All bounds read this bench's own runs.

| Bound | Rule | Void check |
|---|---|---|
| **B1a** (decided) | `AD + bAD < (a − ba) + (d − bd)` | expected saving ≥ 2 × (ba + bd + bAD), else the round set is void and reruns once |
| **B1b** (decided) | `ADF + bADF < (a − ba) + (d − bd) + (f − bf)` | same form |
| B2 | `AD_peak ≤ 1.15 × max(a_peak, d_peak)`; `ADF_peak ≤ 1.15 × max(a_peak, d_peak, f_peak)` | — |
| B3 | `AS_peak ≤ 1.15 × max(a_peak, s_peak)` | — |
| **B2u** | `U_peak ≤ 1.15 × max(peaks of U's jobs standalone)` (policy's, in practice) | — |
| **B2o** | `O_peak ≤ 1.15 × max(peaks of O's jobs standalone)` (`h`'s, in practice) | — |
| B4 | for every Part 3 command x: `H_x ≤ 1.03 × P_x` (peaks) and `H_x + bH_x ≤ P_x + bP_x` (walls). `diff ws ws` is read against D8's stated change: its peak may drop, never rise | — |
| B5 (decided budget) | `Σ(W4) ≤ 29,360,128 KiB` and `Σ(W4u) ≤ 29,360,128 KiB` | — |
| B5p (decided cap) | `W2p` runs at most 2 workers at once and the Σ of any two concurrent workers ≤ 29,360,128 KiB | — |
| B6 | `W4 + bW4 ≤ 1.25 × (W1q − bW1q)` (throughput ≥ 0.8 × 4 × one at CPUs/4) — **reported, not gating (12A-18, operator 2026-10-24)** | — |
| B7 | `W4 + bW4 ≤ 4 × (W1f − bW1f)` (throughput ≥ one at full threads) | — |

The "post-attach phase peak" of §5.12 is "not measured" on Linux (#204); every peak here is `VmHWM` (12A-7).
B1 as first worded (ADF < a + d) is **reported, not a bound** (decided: replaced by B1a + B1b).

### 7.3 Feasibility from measured numbers (CDO; step 6 final bench, release-fast, 3 rounds)

| Run | Wall, median [range] | Peak (`VmHWM`) |
|---|---|---|
| `o` (analyze d47/d49/d51; proxy for `a`) | 16.99 s [16.93-17.04] | 3,137,828 KiB |
| `d` digest 3 roots | 23.77 s [23.71-24.34] | 3,151,384 KiB |
| `h` digest 88 roots | 41.39 s [41.38-41.45] | 3,510,088 KiB |
| policy cross-app | 27.50 s [27.49-27.84] | 8,327,444 KiB |
| fingerprint B0 single-app (1 round) | 5.39 s | 1,471,224 KiB |
| analyze `--single-app` (older head) | 3.7 s | 714-755 MB |

Default `a` was not timed at step 6's head (10+11a head: 18.6 s); Part 2 measures it (P-N8).

**Estimates** (not measured):
- `AD` on one view = digest's whole run (its run IS analyze's) + analyze's projection and SARIF (≈ 1 s) ≈
  **24.8 s**. `ADF` = `AD` + the single-app `f` view (≈ 5.4 s) ≈ **30.2 s**. Same in case A and case B
  (S′ keeps `f` single-app).
- `U` ≤ policy's peak (raw of every primary routine held through `det`) + `h`'s tree at (5) while policy's
  raw is alive (`h` − `d` ≈ 0.36 GB is the tree's own increment at step 6). No clone term: D5.3 borrows. No
  tree bytes past the read: every tree read's output is fully owned (step 9 §4.7: `FingerprintQueryResult`
  carries each witness's terminal evidence; the inventory map its five keys), so each tree is dropped right
  after its read, at (5), before `det`. The tree increment and policy's own peak (at the `det` fill) need
  not coincide, so ≈ **8.7 GB** is an upper estimate, against a limit of 1.15 × 8,327,444 = 9,576,561 KiB.
  P-N9 checks the borrow mode keeps no extra bytes.
- `O` ≈ `h`'s view peak + `f`'s held 130 MB output (no held tree) ≈ **3.64 GB** against 1.15 × 3,510,088 =
  4,036,601 KiB.

| Bound | Limit | Projection | Verdict |
|---|---|---|---|
| B1a | < 40.02 s − bands | 24.8 s | pass (saving ≈ 15 s; void need ≈ 2 s) |
| B1b | < 45.4 s − bands | 30.2 s | pass |
| B1 as first worded (reported) | < 40.02 s | 30.2 s | would pass |
| B2 (ADF) | ≤ 3,624,092 KiB | ≈ 3.16-3.20 M KiB (view 1's peak; `a`, `d` are written before view 2 since they precede `f`) | pass, ≈ 0.4 GB room |
| B3 (AS) | ≤ 3,608,502 KiB | ≈ a's peak | pass |
| **B2u** | ≤ 9,576,561 KiB | ≤ ≈ 8.7 GB (**estimate**, provisional) | pass with ≈ 0.9 GB room; P-N9 at Task 1 decides; then F13's checks, then STOP |
| **B2o** | ≤ 4,036,601 KiB | ≈ 3.64 GB | pass |
| B4 | ≤ 1.03 × P | a one-job batch holds a plan and one output (D6.1 keeps today's release points) | pass (likely) |
| B5 (ADF, case A and B) | Σ ≤ 29,360,128 KiB | 4 × ≈ 3.2 M ≈ 12.8 M KiB | pass |
| B5 (W4u) | same | 4 × ≈ 3.6 GB ≈ 14.4 GB (**estimate**) | pass |
| B5, case B batch of prove / `fingerprint --routine` / `--inventory-only` (reported) | same | 4 × ≈ 3.3 GB ≈ 13.2 GB (**estimate**, step 9 §8.0) | pass (estimate; Part 4b measures the rows) |
| B5p (policy, W = 2) | Σ of two ≤ 29,360,128 KiB | 2 × 8,327,444 ≈ 16.7 M KiB | pass |
| B6 | W4 ≤ 1.25 × W1q | unknown: `d` uses ≈ 4.6 cores on average (110 CPU-s / 23.8 s), `o` ≈ 8.8; four at Q = 5 use 20 of 22 CPUs; the risk is memory bandwidth | **unknown → P-N7** |
| B7 | W4 ≤ 4 × W1f | W4 ≈ 1.2-1.6 × W1f (**estimate**) | pass (likely) |

### 7.4 Projection before the code (binding)

At Task 1, before the driver is written:
- **P-N5**: `ALSEM_TRACE` of standalone `a` and `d`, release-fast (build, attach, det, run, tree, effects,
  projection, serialize); replaces the `AD`/`ADF` estimates.
- **P-N9**: a census line with the raw bytes of `d`'s and `h`'s roots and of policy's `primary` set; sizes
  what D5.3's borrow mode keeps alive for one `compose_tree` longer, and `U`'s estimate. It is read with
  D5.3's borrow mode applied (it is the design); a projected B2u miss then goes through F13 before the stop.
- **P-N7**: four concurrent standalone `d` at `RAYON_NUM_THREADS=5` vs one at 5 vs one at full, Σ `VmHWM`;
  projects B5-B7. It needs an **exclusive memory slot** (≈ 12.6 GB in the shared cgroup, no other session's
  memory-heavy run). **If no exclusive slot is free at Task 1**: Task 1 does not project B6 from a contended
  run; the driver task waits (its earlier, non-driver tasks may proceed); the conductor decides when the slot
  is free (it schedules both sessions sharing the cgroup) and runs P-N7 then, before the driver task starts.

If a projection misses a bound, the claim stops at Task 1 with the numbers (the inputs split and Decision 7's
moves may land; no `batch` command).

### 7.5 Numbers file

`docs/measurements/2026-10-<dd>-issue-277-160-step12.md`: Parts 1-4b, the case (A or B), the argv of every
job, `W` and the table rows.

## 8. Fallbacks (every "if X", stated in advance; each re-runs the identity gates it could affect)

**Stop rule (S8).** A miss on one build / one `det` / one cone / one witness pass per view (A12.2, A12.6) is a
**STOP**, not a fallback: the claim halts and the conductor takes the numbers to the operator. No fallback
below adds a second view or a second pass for one key.

| # | If | Then |
|---|---|---|
| F1 | T1 or Part 1 shows a job differing from standalone | a defect: name the stream and the cell; fix the root; re-run. Not closed in one attempt → stop at Task 1. Never an allow-list |
| F2 | B1a or B1b missed, met-not-void | name the phase with `trace_summary.py`: a duplicate fill (counters ≠ D10) → STOP (stop rule); else ONE fix, re-run Part 2 once; still missed → that row FAILS, the claim stops |
| F3 | a bound void twice | that row FAILS; nothing merges |
| F4 | B2/B3/B2u/B2o missed and a census at serialization shows `det`/`caps`/`model`/`rel`/`l4`/tree bytes alive | release at the union's last reader (D6.1); re-run Part 2 once |
| F5 | still missed; held `JobOutput`/`SideDoc` bytes are the cause (`batch.held`) | spill each held `JobOutput` to a temp file, deleted after its write; re-run once |
| F6 | still missed; one serializer's spike | stream that serializer (Decision 51); still missed → FAILS |
| F8 | B4 missed (a standalone grew) | a one-job batch holds nothing extra: write without holding; re-run once; still missed → FAILS |
| F9 | B5, B5p or B2u-driven table rows missed | first check `KIND_PEAK_KIB` against the measured peaks: a row below its measurement is a defect, fixed by the measured value, which lowers `W`; re-run Part 4 once. Still missed at the derived `W` → stated FAIL of E16 memory; record Σ; no relabel |
| F10 | B6/B7 missed | contention (`/proc/loadavg` > N during Part 4) → re-run once in a fresh slot; a pool not honouring `RAYON_NUM_THREADS` → fix; still missed → FAILS |
| F11 | a library stderr line is printed from a rayon worker mid-fill | the worker returns its lines; the fill concatenates in index order |
| F12 | a command reads a workspace field only under a request flag, with an effect | read it per job from `RequestInputs` |
| F13 | B2u (projected by P-N9 or measured) misses with D5.3's borrow mode in place (it is the design since round 2, O18) | name the bytes with the census at the miss point: (a) entries the borrow mode keeps alive past their last reader → a defect, remove them right after that reader; (b) a tree alive past its read → a defect (D6.5); (c) neither → the miss is the union itself: B2u FAILS and the claim STOPS (stop rule): no second view, no second pass |
| F14 | case B: query / inventory / prove tree reads and policy share a view and B2u-shaped peaks miss | F13's checks; still missed → STOP (stop rule) |
| F15 | the `Cow` pd arena (D5.3) moves standalone digest's peak (B4) | give `compose_tree` the arena by value for a one-tree view (today's code path) and `Borrowed` only when a view has ≥ 2 trees; T1 B6 still pins the many-tree case |
| F16 | a batch shows a second view's peak stacking on the first (B3, against P-N6) | call `mi_collect(true)` after a view drop (mimalloc is already the allocator); re-run B3 once; still stacking → B3 FAILS |
| F17 | the derived `W` is lower than 4 for the bench's `ADF` (a table row higher than projected) | B5-B7 are read at the derived `W` and the numbers file states it; `W4` becomes `W<k>` with B6's factor `k/4 × 0.8`; the E16 "four workers" row is then a stated FAIL of "four", not a relabel |
| F18 | (activated by the conductor, round 1) | `alsem batch` sets no `RAYON_NUM_THREADS`; children inherit; the bench sets `Q` |
| F19 | P-N7 has no exclusive slot at Task 1 | §7.4: the driver task waits; the conductor schedules P-N7 |

## 9. Case A / case B (step 9)

Agreed with step 9's owner (conductor, 2026-10-24). Whichever of #277 and step 9 merges second rebases;
neither redesigns.

- **`WitnessRequest`**: this step lands the set form (D5.1). Step 9 uses only `Roots(ids)`: prove
  `Roots({root})`, `fingerprint --routine` `Roots(pool)`, `fingerprint --inventory-only` `Roots(∅)`. None of
  them asks `primary`; only policy does (D5.1's table). An empty request never means "no composition"
  (D5.1a; step 9 §4.7 rule 2): `Roots(∅)` with an `Inventory` read composes once, so `--inventory-only`
  keeps step 9's `DIGEST_ROW` with witness 0.
  - Case A (12 first): step 9 writes `{ ids, primary: false }` where its spec writes `Roots(ids)`, adds its
    `TreeRead` variants to `TreeDemand.read` (D5.2), not to `Plan`, makes `Cell::TreeRead` per job
    (`take_tree_read(job)`, D5.4), and resolves its roots in `resolve(&Model)` (D4.2).
  - Case B (9 first): this step rebases step 9's `Roots(ids)` to `{ ids, primary: false }`, moves step 9's
    `Plan.tree: TreeRead` into `TreeDemand.read` (its `None` becomes "no tree demand"), makes the
    `TreeRead` cell per job, moves prove's and the fingerprint query's root resolution into
    `resolve(&Model)`, and turns on T22, on the `ws_split` fixture (on `ws_dep()` the union break is not
    observable).
- **No kept trees**: both read and drop every tree inside `prepare`, before `det` (step 9 §4.5; D6.5). Step 9
  dropped `TreeUse` and `take_tree`. Every read's output is fully owned (step 9 §4.7 at `eb754df0`), which is
  what lets the driver drop each tree right after its read (D5.4).
- **Release mask**: step 9 widens `View.released` from `u8` to `u16` (today `session.rs:53` `released: u8,`).
  #277 adds no `Cell` (effects and tree reads become per-job maps inside the existing `Effects` and
  `TreeRead` cells; the resolve-phase identity table is not a cell). Should a cell be added during
  implementation, it takes the next free bit, and whichever of #277 and step 9 merges second renumbers.
- **Cut readers**: case A, fingerprint (all forms), prove and diff compose their own cut snapshot on the
  single-app view: `cone` 1 + one per cut reader, `witness` one per cut reader (decided). Case B, prove and
  the fingerprint query / inventory-only forms are tree jobs on the view's pass (cross-app by default,
  single-app with `--single-app`); fingerprint B0 and `diff` stay cut readers on the single-app view (S′),
  with the same stated row. Step 9's follow-up (B0 and `diff` cross-app) removes the row.
- **Counter rows**: the second to merge restates D10's rows for its commands (prove becomes `DIGEST_ROW` in a
  batch view, step 9 §4.6).
- **Bounds**: B1a, B1b, B2, B3 use B0 `f`, single-app in both cases, so their projections do not change. B5
  in case B adds the reported row for the cross-app prove / query forms (≈ 3.3 GB each, **estimate**;
  Part 4b measures and the table takes them).

## 10. Decided items and amendments this spec asks the epic to take

Decided by the operator before the claim (`/work/bench/160-12/OPERATOR-DECISIONS-2026-10-24.md`):
- **12A-4 (decided)**: the wall bound is B1a (AD < a + d) and B1b (ADF < a + d + f), both true in either merge
  order. B1 as first worded is reported.
- **12A-6 (decided)**: E16's "physical RAM − 4 GB" is a 28 GiB budget (the shared cgroup's 32 GiB). A run
  with `policy check` uses at most 2 workers, enforced by `alsem batch` (D12). Other runs may use 4 within
  28 GiB, for the measured shapes (Limit L1).
- **12A-11 (decided)**: case A cut readers (fingerprint, prove, diff) keep their own single-app snapshot,
  a stated counter row, until step 9.

Agreed with step 9 (conductor relay): **12A-13**, the `WitnessRequest` set form lands here (§9); composition
is signalled by the plan (request non-empty, a tree demand, or a run needing derived rows), never inferred
from an empty set (D5.1a); no tree outlives `prepare`; step 9's `TreeRead` is a field of `TreeDemand` (D5.2).

Decided by the conductor in round 1: Q-a (run exit 0 when every job ran; per-job exits in the lines; a
`batch` row in `exit_code.rs`; a job's write failure takes its command's write-failure code), Q-b (several
workspaces), Q-c (one tree per digest job), Q-d (process lines once, on the worker's stderr, forwarded by
whole lines); threads (F18: no code); the stop rule (§8).

Asked of the epic:
- **12A-1**: the "early batch after step 2" row is void; Decision 50 lapses (policy is batchable).
- **12A-2**: standalone is a batch of one (D1).
- **12A-3**: B1a-B3 compare head with head; B4 bounds the standalone rewire parent vs head.
- **12A-5**: the `Arc` census rule, in its two parts (D11).
- **12A-7**: "each peak" = `VmHWM`; the post-attach peak is "not measured" (#204).
- **12A-8**: views one after another in order of first demand; each job is written as soon as it and every
  earlier job are finished (D6).
- **12A-9**: `InputSet` splits into `WorkspaceInputs` and `RequestInputs` (epic open question 15).
- **12A-10**: refused jobs, write conflicts (across and within jobs, nested directories) and read-after-write
  are usage errors (D2.4).
- **12A-12**: the union rule: a view takes exactly one witness request, the union of its jobs'; each tree job
  gets its own tree from that one pass; raw entries are moved by their last reader and borrowed by earlier
  ones (no clone);
  the pd arena and the dependency calls are lent to every tree but the last (D5).
- **12A-14**: E16's "four workers" is enforced by `alsem batch` as a derived `W` (D12), not left to the
  caller, and holds for the measured manifest shapes (Limit L1); the throughput bounds are read at the
  derived `W`.
- **12A-15**: `JobOutput` is an ordered list of write actions (D2.3); the epic's `stderr_first` is its
  special case.
- **12A-16**: planning is two-phase per view (build, resolve, union, prepare); the identity table is built
  once per view and released after resolution (D4.2).
- **12A-17**: E13's "stderr lines equal standalone" has two stated exceptions in a batch: a view's `log`
  lines (D7) and a job's own write-error line (D2.2) go to the worker's stderr, not the job's stderr file. In
  a one-job batch both land on the process stderr as today.
- **12A-18 (operator decision, 2026-10-24, after Task 1's STOP)**: B6 (four-worker throughput ≥ 0.8 × 4 ×
  one) is **reported, not gating**. Task 1 measured four concurrent digests at 1.45 × one (bound 1.25 ×) on
  clean rounds, while every other bound passed. The same reading found master's digest regressed after step
  6's merge (`d` 40 s vs 23.8 s, `h` 118 s vs 41 s; cause 444d0c8b, filed as #289), which inflates the
  contention. The operator chose a working `alsem batch` now over the throughput target: `W` stays as D12
  derives it; Task 12 measures and reports B6 (on #289's fix if it has landed); a follow-up investigation of
  four-worker throughput is filed as a discovery. B1a, B1b, B2, B2u, B2o, B5 and B7 stay gating.
- **Stated deviation (Task 8, R7-3)**: `diff` reads both workspace sides when the job is made, before the
  first build (D3: every input is read before any write, and read-after-write is checked against them), not
  when the new side's view starts. Standalone `diff A B` therefore holds B's read during A's build (D6.3 (c)
  counts it), and a `diff` whose old side fails still reads its new side (`inputs.read` 2, R6-3's row).
  Reading the new side lazily would let a later read see another job's write, which D2.4 refuses only
  because every read comes first. B4 reads diff's peak.
- **Stated limit (Task 8, R7-8)**: jobs that spell one workspace path differently share its one read
  (D3), which carries the first job's spelling. graph's missing-`id` error names the job's own spelling; any
  other build error text of a shared view names the first spelling.

## 11. Open questions

None for the operator. Q-a to Q-d were decided by the conductor in round 1 (§10). Step 9's names may still
move (the conductor forwards the final ones; §9 is written against `Roots`, `TreeRead`, `Cell::TreeRead`,
`take_tree_read`).

## 12. Dependencies

- **6 (merged)**: `Cell::{Caps, Raw, Tree, Effects}`, `WitnessRequest`, the composing `prepare` order,
  policy on `Cell::Raw`, digest on the cone tree.
- **9 (either order)**: §9.
- **#204 (not a dependency)**: if it lands first, B2-B4 gain the post-attach peak.
- **13 (after)**: the incremental host may reuse the driver.
