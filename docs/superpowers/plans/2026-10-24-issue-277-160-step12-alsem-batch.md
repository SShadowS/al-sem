# #277 (#160 step 12): `alsem batch` — one model, several commands — plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem batch <manifest>` runs several analysis commands in one process. Jobs on one view key
share one program build, model, relation, witness pass (at the union of their requests), `det`, `l4` and
every shared run. Every job's output equals its standalone run, and standalone is a batch of one.

**Architecture** (spec §4):
- A new `src/engine/batch/` module holds the `Job` trait, `JobOutput` (an ordered list of write actions),
  the manifest, the driver and the worker pool. Every view command becomes a `Job`; `alsem <cmd>` builds
  one job and runs the driver (D1).
- `View` takes ONE plan per view: the union `WitnessRequest { ids, primary }`, one `TreeDemand` per tree
  job, runs keyed with their own scope. `compose_tree` gets a drain mode and a borrow mode (`SubjectRaw`),
  a `Cow` pd arena and lent dependency calls, so several trees come off one pass (D5).
- Views build one after another in order of first demand; each job is written as soon as it and every
  earlier job are finished (D6). Two or more batches run on up to `W` child processes, `W` derived from a
  28 GiB budget and a committed peak table (D12).

**Tech stack:** Rust (clap, rayon, serde_json). Plus `scripts/ci-steps`, `scripts/check-goldens`,
`scripts/cdo-gate`, `scripts/agentflow`, `scripts/trace_summary.py`, `/work/bench/160-10-11a/maxrss.py`;
the `discrimination-proof`, `cdo-measure` skills; the `golden-diff-triager` and `measurement-auditor`
agents.

**Spec:** `docs/superpowers/specs/2026-10-24-issue-277-160-step12-alsem-batch-design.md` (converged at
round 4, committed `c86087fc`; "spec §n" / "Dn" / "Tn" / "Fn" below). Epic:
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9 row 12 (the epic wins except
the spec's §10 amendments 12A-*). Probes: `.agent/issue-277/ledger.md`. Step 9 (the other lane):
`/work/al-sem-issue-160-a10/docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md`.

**Plan-level clarifications of the spec** (decided by the conductor, 2026-10-24):
- **No T18.** The spec's §5 numbers its tests T1-T17, T19-T25; there is no T18 to implement.
- **D8's `diff A A` rows are read at Task 6 Step 1, not at Task 1.** D8 says "Task 1 reads them off a
  traced prototype", but the prototype needs the driver; the rows are read before the counter test is
  rewritten, which keeps D8's intent.
- **An empty manifest** (`{"jobs": []}`, `{"batches": []}`, a batch with no jobs) is a manifest error:
  exit 3, one line, nothing runs (pinned in T11).
- **The peak table:** Task 10 measures provisional rows for kinds with no known row; the bench (Task 12)
  measures Part 4b first, commits the table, rebuilds `H`, then runs Part 4.
- **The new golden family lives at `tests/gate-goldens/batch/`**, not `tests/batch-goldens/`: a
  subdirectory of an already-declared directory, so `scripts/` is not edited (see the golden plan).

The tasks (sizes S / M / L; **X** = needs the exclusive memory slot). 12 tasks; Task 11 is conditional.
- **Task 1** (M, **X**): base-binary records (T20, T23); the binding projection (spec §7.4) P-N5, P-N9,
  P-N7; the inventories; T16. **Can STOP the claim.**
- **Task 2** (M): `InputSet` split, Decision 7's moves, `--update-baseline`'s write leaves compute (T9, T23).
- **Task 3** (M): `WitnessRequest` set form, `TreeDemand`, per-run scope (T14).
- **Task 4** (L): several trees from one pass: drain/borrow, `Cow` arena, lent calls (T25).
- **Task 5** (L): the driver and `JobOutput`; analyze, digest, policy, events through it (T20).
- **Task 6** (L): query, graph, prove, fingerprint, diff through the driver; `diff A A` rows (D8).
- **Task 7** (L): `alsem batch`: manifest, two-phase planning, union, view order, traces
  (T3, T4, T6, T11, T13, T15, T19).
- **Task 8** (L): write sets, write failures, the zero-view phase, stderr classes, view failures, diff
  across views (T5, T12, T21, T24).
- **Task 9** (L): the contract tests and the `Arc` census (T1, T2, T7).
- **Task 10** (M): workers, admission, the peak table, threads (T8, T10, T17).
- **Task 11** (M, **conditional: case B only**): rebase onto step 9; T22 activates.
- **Task 12** (L, **X**): the CDO bench (Parts 1-4b, every bound), the table's measured rows,
  `cdo-gate`, numbers file.

Order rule: Tasks 2-4 do not build the driver and may run while Task 1 waits for its exclusive slot
(spec §7.4, F19). **Task 5 does not start until Task 1's STOP reading is written and passes.**

## Global Constraints

- **Editing.** Edit/Write tools only. No `sed -i`, `perl -pi`, heredoc or script rewrites, discrimination
  breaks included (break with Edit, revert with Edit, re-read the line to confirm the break applied).
- **Formatting.** `rustfmt --edition 2024 <file>` per touched `.rs` file; never `cargo fmt`.
- **No silencing.** No `#[allow(`; the only `#[ignore` is T22's, with reason `"needs step 9"`, and only
  in case A. Dead code is deleted (spec §4.13).
- **Test-only seams** exist only under `cfg(debug_assertions)`, never in a release build (spec §5).
- **New `tests/cli/*.rs` files** are registered as `mod` items in `tests/cli/main.rs`, or they never compile
  or run.
- **Protected paths, never edited by the flow:** `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`,
  `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`. In `.agent/`, only ledger rows a task names.
- **Hands-off (step 9's lane):** `src/engine/l5/digest.rs`, `src/engine/l5/ordering_engine.rs`,
  `src/engine/l5/fingerprint_query.rs`. The spec assigns no edit to them; no task below edits them. A task
  that finds it must: stop that step and ask the conductor (it coordinates with step 9).
- **Environment.** `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every shell. CDO only as
  `CDO_WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json) <command>`; the path is never written into a
  file, log name or ledger row. No timing or peak from the `cdo` profile.
- **Per-task gate, before the commit.**
  1. `bash scripts/ci-steps task-affected > <log> 2>&1`, then `grep -nE 'FAILED|^error|test result' <log>`
     and the exit status. Never `| tail`.
  2. `bash scripts/check-goldens > <log> 2>&1`: **no existing golden may move** (A12.13), except the
     records a task names. Any move: stop, triage with the `golden-diff-triager` agent, explain.
  3. The pre-commit hook passes unmodified (never `--no-verify`).
- **After each commit,** from `/work/alsem`:
  `python3 scripts/agentflow --root /work/alsem --run-id 20261024-100928-5fc9b8 check-diff --base origin/master --head HEAD --issue 277 --cwd /work/al-sem-issue-277-a1`.
  Required `reasons: []`; else fix forward in a new commit, never amend.
- **Staging.** Stage by name; never `git add -A`; never stage `.agent/`.
- **TDD.** Each task writes its tests first and runs them red (a compile failure counts). A test that pins
  TODAY's behaviour (T16, T20, the base half of T23) is green from the start; its red is its break.
- **Discrimination proof** for every test a task turns green, with the break the spec §5 names: break,
  run the one test, record that it fails AT the named assertion, revert, record the pass. One ledger row
  per proof under `## Discrimination proofs` (`test | break | fail | pass | commit`). A break that stays
  green: stop and report.
- **Measurement** goes through the shared slot, output under `/work/bench/160-12/`:
  `python3 /work/alsem/scripts/agentflow --root /work/alsem --run-id 20261024-100928-5fc9b8 run --name bench --timeout 60 --cwd <dir> -- <command>`.
  Load gate before each part (1-min load ≤ 4, wait ≤ 15 min, then fail closed); a load tick > 8 voids the
  part (one re-run). Binaries: `cargo build --profile release-fast --bin alsem --bin aldump` in a clean
  detached worktree under `/work/bench/160-12/`, copied out with a `bins.sha`. **Exclusive** (X) means
  the conductor confirms no other session (step 9 shares the 32 GiB cgroup) runs memory-heavy work for the
  slot; two X tasks never run at once.
- **CHANGELOG.md** gets a line in each task that changes user-visible behaviour (Tasks 2, 7, 10).

## Review Focus

Each line is a test in its owning task.

1. **Relative paths in one job, two anchors.** Manifest `stdout`/`stderr` resolve against the manifest's
   directory, `--out`/`--shard`/`--baseline` against the process cwd (D2.1). Run from a cwd that is not
   the manifest's directory, a mix-up writes the wrong file silently. Task 8:
   `batch_relative_paths_two_anchors`.
2. **One workspace, two spellings** (`ws`, `./ws`, `ws/`). D4.1 keys on the absolute spelling, not the
   canonical one. Task 7: `batch_view_key_spellings` asserts one `view` span per distinct
   `std::path::absolute` result, with output equal to standalone either way.
3. **Commands the batch refuses but T11 does not list:** `cache prune` and an argv with no subcommand (the
   light `--project` mode, spec §2). Task 7 adds both to T11: exit 3, one line.
4. **A worker killed by a signal** must give its jobs `"exit":null`, `"worker":"<signal>"` and run exit 1
   (D2.2). Task 10: `batch_worker_killed`, with a debug-only seam `ALSEM_TEST_WORKER_ABORT=<b>` (that
   child calls `std::process::abort()` before its first view).
5. **An empty manifest** (decided: exit 3, nothing runs). Task 7 adds it to T11.

---

## Task 1 — base records and the binding projection (M, X) — can STOP the claim

Spec §7.4 (binding), §7.3, D2.3 and D2.4 (the base records), D3 (field list), D7 (stderr inventory),
P-N10; T16.

**Files:**
- Create `tests/gate-goldens/batch/merged-streams/{degraded-analyze,policy-diagnostics-out,policy-out-fails}.merged.golden`
  with a `.exit` beside each, and `tests/gate-goldens/batch/merged-streams/RECORD`.
- Create `tests/gate-goldens/batch/update-baseline/{base.stdout.golden,base.stderr.golden,base.exit}` and
  `tests/gate-goldens/batch/update-baseline/RECORD`.
- Create `tests/gate-goldens/batch/README.md` (one paragraph: what each subdirectory pins; a directory with a
  `RECORD` file holds base-binary records that are never regenerated).
- Modify `src/engine/l5/detector_context.rs:1380-1387`: the census line gains `witness.raw_bytes=<n>`.
- Test: T16 in `src/engine/gate/run.rs`'s test module (or a new `src/engine/gate/run_builders_tests.rs`
  if the module is crowded).
- Ledger: `## Task 1 projection`, `## Inventories`. No driver code in this task.

**Interfaces (produced):** the base records Tasks 2 and 5 read; the census key `witness.raw_bytes` (bytes
of the pass's `raw` map: key heap bytes + `capacity × size_of::<CapabilityFact>()` + each fact's owned heap,
with the census's existing fact-vector sizing helper if one exists); the two inventories Tasks 2 and 8
implement.

- [ ] **Step 1: base records.** Build `alsem` (`cdo` profile is fine: bytes, not timings) from a clean
  detached worktree at `1134bed1` under `/work/bench/160-12/base-wt`. Pick the fixtures (asserted, named in
  the README): a degraded-analyze workspace whose run prints `al-sem: warning:` before stdout
  (`alsem.rs:1664-1670`); a `policy check` workspace with analyzer diagnostics and `--out`; the same with
  `--out` in a missing directory. Fixtures from `tests/r0-corpus/`, `tests/fixtures/` or the
  `cli_write_failure` / `cli_c_policy_differential` members; a new fixture only if none fits (then
  `tests/fixtures/batch/`, which moves no golden). Run each with `2>&1` into one file (`--deterministic`
  where the command has it); record bytes and exit. Run
  `analyze <ws> --baseline <missing-dir>/b.json --update-baseline`; record stdout, stderr, exit (expected
  `al-sem: failed to write baseline '…': …`, exit 3, empty stdout; assert it).
- [ ] **Step 2: T16 `single_app_builders_agree`** (lib): on every r0 `ws-*` dir, `--no-roots-config` true
  and false, `AnalysisTarget::build` (single-app), `build_analysis_model(.., true)`'s model and
  `assemble_and_resolve_workspace_program(inputs, id, skip)` give equal model rows (serialised rows, or field
  by field). Green on the base; break: flip `skip_roots_config` in one builder → differs on a dir with a
  roots config (precondition: at least one r0 dir has one, asserted). Proof row.
- [ ] **Step 3: the census line.** Add `witness.raw_bytes=`; one run on `ws_dep()` with
  `ALSEM_RETAINED_CENSUS=1` shows the key printed and > 0.
- [ ] **Step 4: inventories** (ledger, exact, `file:line` + text at the branch head):
  (a) per command, every `InputSet` field it reads (D3; the union `WorkspaceInputs` reads);
  (b) D7's three classes: the 49 bin `eprintln!`s (each with its command and its position relative to the
  stdout write); the 96 library `eprint`/`eprintln` sites (outside `bin/`, `lsp/`, `server`, `watcher`,
  tests), each classed **job line** (which fill prints it, and the replay rule: build lines always; L4
  non-convergence iff `CORE_SUMMARIES` in the job's mask; det lines iff the job has a run) or **process
  line**; the `log::` sites in `dependencies.rs`, `snapshot/cache.rs`, `program/model/workspace.rs`,
  `engine/deps/app_package_zip.rs` (12 + 3 expected). A site in a hands-off file: flag to the conductor.
  A site printed from a rayon worker mid-fill: note it for F11.
- [ ] **Step 5: gate and commit** (`#277 Task 1: base-binary records, witness.raw_bytes census key, T16`).
  Byte-neutral: no existing golden moves. `check-diff`.
- [ ] **Step 6: measure** (release-fast binaries of this commit, `/work/bench/160-12/task1/`, interleaved
  rounds, `maxrss.py`):
  - **P-N5** (quiet slot): 5 rounds each of `a`, `d`, `f` (spec §7.1's argv) for walls and bands; one
    `ALSEM_TRACE` run each of `a` and `d` (`trace_summary.py`: build, attach, det, run, tree, effects,
    projection, serialize).
  - **P-N9** (quiet slot): 3 rounds each of `d`, `h`, `p` with `ALSEM_RETAINED_CENSUS=1`: peaks,
    `witness.members`, `witness.raw_bytes`. Assert from the run ids that `d`'s and `h`'s subjects are
    primary routines (so the union of `U` is policy's set).
  - **P-N7** (**exclusive slot**, ≈ 12.6 GB): 3 rounds of four concurrent `d` at `RAYON_NUM_THREADS=5`
    (Σ `VmHWM`, wall = start to last exit), one `d` at 5, one `d` unset. **No exclusive slot free:** record
    P-N7 as pending; Tasks 2-4 may proceed; Task 5 waits until the conductor schedules the slot (F19).
- [ ] **Step 7: STOP reading** (ledger, with the numbers). Each line passes, or the claim **STOPS at
  Task 1** (only Task 2's input split and Decision 7 moves may still land; no `batch` command):
  - **B1a projected**: `AD′ = d + (a's projection + SARIF serialize + write, from a's trace)`;
    pass iff `AD′ + bd < (a − ba) + (d − bd)`. Also report whether `(a + d − AD′) ≥ 2 × (ba + 2·bd)`
    (else B1a is likely void at Task 12; not a STOP).
  - **B1b projected**: `ADF′ = AD′ + f`; pass iff `ADF′ + bd + bf < (a − ba) + (d − bd) + (f − bf)`.
  - **B2u projected**: `U′ = p_peak + (h_peak − d_peak) + extra`, `extra` = 0 when `d`'s and `h`'s
    subjects ⊆ policy's set, else `h`'s `witness.raw_bytes`; pass iff `U′ ≤ 9,576,561 KiB`. A miss goes
    through F13 first: (a) entries kept past their last reader or (b) a tree past its read are design
    defects to remove before re-projecting; (c) neither → **STOP**.
  - **B2o projected**: `h_peak + 130 MB ≤ 4,036,601 KiB`.
  - **B5 projected**: four-`d` Σ ≤ 29,360,128 KiB. **B6 projected**: `W4′ + b ≤ 1.25 × (one-at-5 − b)`.
    **B7 projected**: `W4′ + b ≤ 4 × (one-unset − b)`. A B6/B7 miss with 1-min load > 22 during the
    round: one re-run in a fresh slot (F10), then STOP on a second miss.
  - Pass: write the projections as the expectation Task 12 is read against; go on.

## Task 2 — inputs split, Decision 7's moves, the baseline write (M)

Spec D3, D2.4 (the baseline bullet, S17), §2 (the stated standalone changes), 12A-9; T9, T23.

**Files:**
- Modify `src/program/inputs.rs`: `InputSet` → `WorkspaceInputs` (read once, `Arc`, with a
  `WorkspaceNeeds` union of Task 1's field list) + `RequestInputs` (policy file and request, baseline,
  diff file side `snapshot_inputs`).
- Modify `src/bin/alsem.rs`, `src/engine/gate/policy/pipeline.rs:205-223`,
  `src/engine/l5/fingerprint_cli.rs` (shard/out validation), every `InputSet::read` caller.
- Modify `src/engine/gate/run.rs:594-595` (no `save_baseline` in `run_analyze_with_exit`; return the bytes
  from `baseline.rs:88`'s function) and `src/bin/alsem.rs:1664-1689` (write them with the bin's
  `write_file` before stdout; a failure prints `al-sem: write error: <path>: <OS message>`, exit 2, stdout
  unwritten).
- Tests: `tests/cli/cli_decision7_precedence.rs` (T9), `tests/cli/cli_update_baseline_write.rs` (T23);
  create `tests/cli/batch_records.rs` (the guarded record reader and the family's golden writer; see the
  golden plan), which T23 is the first to use.
- `CHANGELOG.md` (Changed: the three precedence moves; the baseline write-failure text and exit).

**Interfaces (produced):**
- `pub struct WorkspaceInputs { … }` with `pub fn read(path: &Path, needs: WorkspaceNeeds) -> Result<Arc<WorkspaceInputs>, InputError>`;
  `pub struct WorkspaceNeeds` (one bool per optional field) with `fn union(self, other) -> Self`.
- `pub struct RequestInputs { … }`, one per job.
- A validation phase per command, run before any build, returning `Result<(), (String, i32)>` (the error
  type becomes `JobFailure` in Task 5).
- `run_analyze_with_exit` returns the baseline bytes (`Option<Vec<u8>>`) instead of writing them.

- [ ] **Step 1: T9, red** — `decision7_precedence_policy_and_build`, `decision7_precedence_shard_out_and_build`,
  `decision7_precedence_policy_and_strict` (spawned, counters from the trace). Each first asserts both
  failures separately (each alone gives its own text). Then: (1) bad policy + failing build → the policy
  text and exit, `build.program` 0; (2) bad `--shard` (or `--out`) + failing build → the shard/out text,
  `build.program` 0; (3) `policy check --strict`, bad policy + an error-severity diagnostic → the policy
  error, exit 1.
- [ ] **Step 2: T23 `update_baseline_write_failure`, red**: reads Task 1's `update-baseline/` record as
  the precondition (`failed to write baseline`, exit 3, empty stdout), then the head's run:
  `al-sem: write error: <path>: …`, exit 2, empty stdout. Plus a success case: the baseline file is written
  and stdout equals today's.
- [ ] **Step 3:** split the type; move the policy load and fingerprint's shard/out checks into validation;
  `diff --renames` keeps its order; move the baseline write out of compute. T9 and T23 green; every golden
  unchanged.
- [ ] **Step 4: proofs**: validate after the build → T9 (1) and (2) fail at `build.program == 0`, (3) at the
  stderr text; keep the compute-side `save_baseline` → T23 sees the old text and exit 3.
- [ ] **Step 5:** gate, commit (`#277 Task 2: WorkspaceInputs/RequestInputs; validation before the build; the baseline write as an output`), `check-diff`.

## Task 3 — the request set form, tree demands, per-run scope (M)

Spec D5.1, D5.1a, D5.2 (types only, one tree as today), D5.5, §9 (the set form lands here), 12A-13; T14.

**Files:** `src/engine/session.rs` (`WitnessRequest`, `Plan`, the assertion at `:408-410`, the plan tests
at `:1062-1071`), `src/engine/gate/policy/policy_engine.rs`, `src/engine/l5/digest_cli.rs` (digest's plan),
`src/engine/gate/run.rs` (analyze's plan), `src/engine/l5/detector_context_step5_tests.rs`.

**Interfaces (produced):**
- `pub struct WitnessRequest { ids: BTreeSet<String>, primary: bool }`; `pub fn roots(ids: BTreeSet<String>) -> Self`,
  `pub fn primary() -> Self`, `pub fn empty() -> Self`, `pub fn union(&self, other: &Self) -> Self`,
  `pub fn is_empty(&self) -> bool`, `scope(&self, model: &Model)` (today's signature and return type kept).
- `pub struct JobId(pub u32)` (the job's index in its batch) in `session.rs`.
- `pub struct TreeDemand { pub subjects: WitnessRequest, pub tests: bool, pub read: TreeRead }`,
  `pub enum TreeRead { Effects }` (case A ships only this variant).
- `Plan { runs: Vec<(Vec<Detector>, bool /* scope_primary */)>, need_l4, after_run, witness: WitnessRequest, trees: Vec<(JobId, TreeDemand)> }`;
  `needs_tree`, `tests` and the enum form deleted. `RunKey` keeps its shape; its `scope_primary` now comes
  from the run's own entry.
- `Plan::composes(&self, det_filled: bool) -> bool` = `!witness.is_empty() || !trees.is_empty() || (a run needs derived rows && !det_filled)` (D5.1a).
- The assertion text becomes `"every tree demand's subjects are inside the view's request"`.

- [ ] **Step 1: T14 `witness_request_union`, red** (lib, `session.rs` tests): a model with primary and
  dependency rows stated by assignment; `scope(union(a, b)) == scope(a) ∪ scope(b)` for
  (roots, roots), (roots, primary), (empty, primary), (empty, empty); and `union(empty, empty).is_empty()`.
- [ ] **Step 2:** implement; rewrite the plan tests on the new types (I2's exception, same commit). The
  standalone counter rows (`digest_with_no_resolved_root_builds_no_tree`, `cli_build_counters.rs:401`,
  `DIGEST_ROW`, the policy and query rows) stay as they are and are the standalone pin of D5.1a.
- [ ] **Step 3: proofs**: `primary: a.primary && b.primary` → the (roots, primary) case fails. For D5.1a:
  make `composes` read `!witness.is_empty()` alone → record which existing counter test fails; if none
  does, add the `digest --tests` no-root counter row to `cli_build_counters.rs` here.
- [ ] **Step 4:** gate (no golden moves), commit (`#277 Task 3: WitnessRequest set form, TreeDemand, per-run scope`), `check-diff`.

## Task 4 — several trees from one pass (L)

Spec D5.3, D5.4, D5.6, D6.5, F15; T25.

**Files:** `src/engine/l5/snapshot.rs` (`compose_tree` at `:1374-1432`, the operation index `:2132-2143`,
the callsite index `:2258-2269`, `derive_capability_facts` `:1637-1658`, the test at `:3224`),
`src/engine/session.rs:463-551` (the tree loop in `prepare`, per-job effects, `take_effects(job)`),
`src/engine/l5/digest_cli.rs` (`take_effects(JobId(0))`), `src/engine/l5/digest_rule_b_tests.rs:408`.

**Interfaces (produced):**
- `pub(crate) struct SubjectRaw<'a> { map: &'a HashMap<String, Vec<CapabilityFact>>, subjects: BTreeSet<String> }`,
  `fn new(map, scope: &BTreeSet<String>) -> Self` (subjects = scope ∩ map keys, O21),
  `fn get(&self, id: &str) -> Option<&'a [CapabilityFact]>` (`None` outside `subjects`),
  `fn subjects(&self) -> &BTreeSet<String>`.
- `pub(crate) enum TreeRaw<'a> { Drain(HashMap<String, Vec<CapabilityFact>>), Borrow(SubjectRaw<'a>) }`.
- `compose_tree(model, typed, dependency_calls: &[CallEdge], caps: &CapabilityAnalysis, raw: TreeRaw<'_>, pd: Cow<'_, ConeArena>, app_names) -> CapabilitySnapshot`
  (`caps` stays `&mut` only if `compose_tree` mutates a field other than `pd`). All four raw readers
  (subject set + pd arena scope, both indexes, `derive_capability_facts`) go through `TreeRaw`.
- `View`: `effects: BTreeMap<JobId, DigestEffects>`, `pub fn take_effects(&mut self, job: JobId) -> DigestEffects`.
- In `prepare` (5): per tree demand in job order: drain if this tree is the last raw reader of every one of
  its subjects (move exactly those entries out of the union map), else borrow; `Cow::Borrowed(caps.pd)`
  for every tree but the last, `Owned` for the last; dependency calls lent, dropped after the last tree
  (or at the start of `prepare` when the plan has no tree); the tree dropped right after its read; then
  remove the entries whose last reader it was. Policy (8) is a later raw reader.

- [ ] **Step 1: T25 `subject_raw_hides_other_subjects`, red** (lib, `snapshot.rs` tests), exactly as spec
  §5: routines X, Y, Z; one map with X and Y entries by assignment (precondition asserted); (a) borrow with
  scope {X}: `get(Y)` is `None`, subject set {X}, nothing from Y in the facts and both indexes, output
  equal to a drain tree over a map with X only; (b) scope {X, Z}: subject set {X}, output (`pd_solution`
  included) equal to (a)'s.
- [ ] **Step 2: `two_trees_one_pass`, red** (lib, session tests, an r0 dir or `ws_dep()` model): a plan
  with two `TreeDemand`s (disjoint roots) and a policy-shaped later raw reader; each job's `DigestEffects`
  equals a one-tree plan's for the same roots; `caps.pd` is consumed only by the last tree;
  `build.witness_pass` 1.
- [ ] **Step 3:** implement. One-tree views take today's path (drain over the whole map, owned arena), so
  standalone digest is byte- and peak-neutral (F15 is the pre-stated fallback if Task 12's B4 says
  otherwise).
- [ ] **Step 4: proofs**: T25 (a) read the operation index from `&map` → Y's operation appears; T25 (b)
  subjects from the scope alone → `pd_solution` differs; `two_trees_one_pass`: `caps.pd.take()` per tree →
  the second tree's effects differ (precondition: its standalone effects carry ≥ 1 proof, asserted).
- [ ] **Step 5:** gate, commit (`#277 Task 4: several trees from one witness pass (drain/borrow, lent arena and calls)`), `check-diff`.

## Task 5 — the driver, `JobOutput`, the first four commands (L)

**Starts only after Task 1's STOP reading passes (P-N7 included).** Spec D1, D2.3, D6 (one view, one job),
D6.1, D6.2, D10 (the `batch.driver` instant); T20.

**Files:**
- Create `src/engine/batch/mod.rs` (driver), `src/engine/batch/job.rs`, `src/engine/batch/output.rs`;
  register in `src/engine/mod.rs`.
- Modify `src/engine/gate/run.rs` (analyze job), `src/engine/l5/digest_cli.rs` (digest job),
  `src/engine/gate/policy/pipeline.rs` (policy job), `src/engine/gate/events.rs` (events job),
  `src/bin/alsem.rs` (handlers: parse → job → `batch::run_standalone`), `src/engine/session.rs` (release
  points move to the driver).
- Test: `tests/cli/cli_batch_merged_streams.rs` (reads through Task 2's `tests/cli/batch_records.rs`).

**Interfaces (produced):**
- `job.rs`: the `Job` trait of spec D1 verbatim; `pub struct SideIx(pub u8)`;
  `pub struct ViewKey { pub workspace: PathBuf /* std::path::absolute, not canonical */, pub universe: ViewUniverse, pub no_roots_config: bool }`;
  `pub enum ViewUniverse { CrossApp, SingleApp }`; `pub struct ViewDemand { pub key: ViewKey, pub side: SideIx, pub needs: ViewNeeds }`
  (`ViewNeeds`: `analyze_extras`, `graph`, `WorkspaceNeeds`); `pub struct PlanPart { runs, need_l4, after_run, witness: WitnessRequest, tree: Option<TreeDemand>, reads: Vec<Cell> }`;
  `pub struct JobFailure { pub actions: Vec<WriteAction>, pub exit: i32 }`; `SideDoc` and `Doc` are
  `Box<dyn Any + Send>` newtypes the job downcasts (no shared shape across commands).
- `output.rs`: `pub enum WriteAction { Stdout(Vec<u8>), Stderr(Vec<u8>), File(PathBuf, Vec<u8>), Dir(PathBuf) }`;
  `pub struct JobOutput { pub actions: Vec<WriteAction>, pub exit: i32 }`;
  `pub fn replay(out: &JobOutput, write_failure_exit: i32) -> i32` (standalone: real streams, in order,
  stops at the first failed action with `al-sem: write error: <target>: <e>` and returns
  `write_failure_exit`; reuses `alsem.rs`'s `write_error`/`write_*` helpers, moved here).
- `mod.rs`: `pub fn run_standalone(job: Box<dyn Job>) -> i32`, which emits the trace instant
  `batch.driver` (`jobs`, `views`, `workers`) and runs the general driver over a one-job batch:
  validate → per view (in first-demand order) build → `resolve` → union → one `prepare` → computes →
  finalize/serialize → write (D6's order). Releases: each cell after the last demand that declared it in
  `PlanPart.reads`; runs dropped after their last reader (D6.1, D6.2).

- [ ] **Step 1: T20 `write_order_under_merged_streams`** (spawned, standalone, `2>&1` into one pipe): the
  three `merged-streams/` records, bytes and exit equal; precondition: each record's warning/diagnostic part
  is non-empty (asserted). Green on the base (a pin). It reads the records through the guarded reader; it
  compares even under `REGEN_TEMP_GOLDENS` and never writes.
- [ ] **Step 2:** build the driver and rewire analyze, digest, policy, events. A degraded analyze emits
  `Stderr` before `Stdout`; policy emits output then its diagnostics; a failed `--out` ends the list. Every
  golden, `--lib` outcome and counter row unchanged.
- [ ] **Step 3: proofs**: serialize every job as fixed `[Stdout, Stderr…, File…]` → the degraded-analyze
  record differs. Guard proof: make T20 call the family's golden writer under `REGEN_TEMP_GOLDENS=1` → the
  guard panics (`refusing to rewrite a base-binary record`); revert.
- [ ] **Step 4:** gate, commit (`#277 Task 5: the batch driver and JobOutput; analyze, digest, policy, events as one-job batches`), `check-diff`.

## Task 6 — query, graph, prove, fingerprint, diff through the driver (L)

Spec D1 (the rest), D4.1 (one builder per view), D6.4, D8 (standalone half), D9, §4.13; the D8 counter rows.

**Files:** `src/engine/l4/effect_query_cli.rs:147-152` (no own cross-app base; borrows `view.model()`,
`view.rel().primary()`, `view.l4()`), `src/engine/session.rs:729` (`into_query_parts` deleted),
`src/program/impact_graph/mod.rs` (`export_workspace`'s own build deleted; `export_from_program` keeps its
checks at `:203`, `:213`), `src/program/model/program_calls.rs:854` (the builder lends `&ResolvedProgram`
before the body is released), `src/engine/l5/prove.rs:880`, `src/engine/l5/fingerprint_cli.rs:387`,
`src/engine/gate/diff/cli.rs:122-254`, `src/engine/gate/run.rs` (the one view builder returning
`AnalysisModel`), `src/bin/alsem.rs`, `tests/cli/cli_build_counters.rs` (`Row` gains `inputs`; the
`diff ws ws` rows `:541-590`).

**Interfaces (produced):**
- `pub fn build_view_model(key: &ViewKey, inputs: &WorkspaceInputs, needs: &ViewNeeds, on_program: impl FnOnce(&ResolvedProgram)) -> Result<AnalysisModel, ModelFailure>`
  (the program is built first and handed to `on_program` whenever `needs.graph`; extras only when
  `needs.analyze_extras`; a graph-only view builds the program alone).
- The diff job: two `ViewDemand`s (old side `SideIx(0)`, new `SideIx(1)`); a file side has no demand;
  `SideDoc` = `Result<(snapshot, diag_lines), String>` plus the side's build lines; `finalize` keeps
  `diff/cli.rs`'s order (old load error, new load error, `--strict` gate old then new, `--renames`, engine,
  render); a failed old side discards the new side's doc and lines.
- `Row { …, inputs: u64 }`; `assert_row` checks `inputs.read == r.inputs`.

- [ ] **Step 1: the traced prototype** (the plan-level clarification of D8): with the rewire done but before
  rewriting the rows, run `diff ws ws` and `diff ws ws --strict` on the `cli_build_counters` fixture with
  `ALSEM_TRACE`; record the rows in the ledger. They must equal spec D8's expected rows (without
  `--strict`: 1, 2, 2, 0, 2, 2, 0, 2, 2, 1; with: 1, 3, 3, 1, 4, 3, 1, 2, 2, 1). A differing cell: stop and
  explain before writing the test.
- [ ] **Step 2:** rewrite the two `diff ws ws` rows to those values and add `inputs` to every row
  (= `program` in every other standalone row). Red against the pre-rewire binary.
- [ ] **Step 3:** rewire the five commands; delete `into_query_parts`, `export_workspace`'s build and the
  per-command `InputSet::read` calls; library entry points kept for tests become one-job wrappers. Every
  golden and the cli-b diff goldens unchanged; only the two `diff ws ws` rows move.
- [ ] **Step 4: proof**: key each diff side on its own view (side index in the key) → the rewritten rows
  fail at `build.program` (2 ≠ 1).
- [ ] **Step 5:** gate, commit (`#277 Task 6: query, graph, prove, fingerprint, diff as one-job batches; diff A A shares one view`), `check-diff`.

## Task 7 — `alsem batch`: manifest, union, view order, traces (L)

Spec D2.1, D2.2 (run lines and exit, the `batch` row), D4.1, D4.2, D5 (union across jobs), D5.6, D10,
12A-8, 12A-16; T3, T4, T6, T11, T13, T15, T19; Review Focus 2, 3, 5.

**Files:** create `src/engine/batch/manifest.rs`; modify `src/engine/batch/mod.rs`, `src/bin/alsem.rs`
(`Commands::Batch { manifest, out_dir, worker: Option<u32> /* hidden */ }`),
`src/engine/gate/exit_code.rs` (the `batch` row in its doc table), `src/engine/perf_trace.rs` (if a
`TraceName` form is needed for `job.<i>.<cmd>` / `view.<i>`); tests `tests/cli/cli_batch_plan.rs` (T3, T4,
T15, T19, `batch_view_key_spellings`), `tests/cli/cli_batch_manifest.rs` (T11),
`tests/cli/cli_batch_traces.rs` (T6), `tests/cli/cli_batch_standalone.rs` (T13),
`tests/gate-goldens/batch/one-job/`; `CHANGELOG.md` (Added).

**Interfaces (produced):**
- `manifest.rs`: `pub struct Manifest { pub batches: Vec<BatchSpec> }`, `pub struct BatchSpec { pub jobs: Vec<JobSpec> }`,
  `pub struct JobSpec { pub argv: Vec<String>, pub stdout: Option<PathBuf>, pub stderr: Option<PathBuf> }`;
  `pub fn load(path: &Path, out_dir: Option<&Path>) -> Result<Manifest, String>` (accepts `{ "jobs": [...] }`
  as one batch; parses each argv with `Cli::try_parse_from(["alsem", …])`; refuses `batch`,
  `policy explain`, `cache prune`, a no-subcommand argv and an empty manifest or batch: exit 3, one line
  `al-sem batch: batch <b> job <i>: <clap's text or reason>`).
- `mod.rs`: `pub fn run_batch(batch: &BatchSpec, b: u32) -> Vec<JobLine>`;
  `pub struct JobLine { batch, job, exit: Option<i32>, stdout, stderr, files, worker: Option<String> }`
  serialised as one JSON line in spec D2.2's shape. Run exit: 0 / 3 / 1 per D2.2.
- Per view (D4.2): build → identity table once if any demand resolves roots → `resolve` per demand in job
  order (a `JobFailure` contributes nothing) → table released → union of `PlanPart`s → one `prepare`.
- Traces: span `view.<i>` around every fill; span `job.<i>.<cmd>` around each resolve, compute, finalize,
  serialize (plus an `instant_lazy` with the args); no fill inside a `job` span.

- [ ] **Step 1: tests, red** (each spawned; counters from the trace):
  - **T3** `batch_counters_one_per_view`: spec D10's case-A rows exactly (cross-app analyze + digest +
    policy + events + query → 1,1,1,1,2,1,1,1,1,1; + a second digest with other roots → snap 2, witness 1;
    analyze cross-app + `--single-app` → program 2, ctx 2, witness 0, inputs 1; single-app fingerprint B0 +
    prove → cone 3, witness 2, the other cells derived from `FINGERPRINT_ROW` and the prove row and written
    into the test before it is run; a run asking `ORDERING_FACTS` → + the cut composition).
  - **T4** `batch_views_are_sequential`: two `view` spans; the second's first fill starts after the
    first's drop instant.
  - **T6** `batch_trace_split_per_job`: every resolve/compute/finalize/serialize span inside exactly one
    `job` span; every fill inside a `view` span and in no `job` span.
  - **T11** `batch_manifest_errors`: nested `batch`, `policy explain`, `cache prune`, a no-subcommand argv,
    an unknown flag, `{"jobs": []}`, `{"batches": []}`, a batch with no jobs → exit 3, one line naming
    batch/job and the reason, `build.program` 0.
  - **T13** `standalone_is_a_batch_of_one` + `tests/gate-goldens/batch/one-job/`: every command's
    standalone trace has exactly one `batch.driver` with `jobs=1`; `alsem analyze …` vs a one-job manifest:
    equal bytes and counters; the one-job family matches.
  - **T15** `analyze_and_digest_share_one_run`: one `finish_run` per `RunKey`.
  - **T19** `empty_request_composes_only_with_a_tree`: (a) `digest --tests`, no resolved root, alone and
    with analyze: census `witness.members=0` (precondition), `build.cone` 1, `build.snapshot` 1,
    `build.witness_pass` 0; (b) `query` alone and query + graph: `build.cone` 0, `build.witness_pass` 0.
    (c) is case B (Task 11).
  - `batch_view_key_spellings` (Review Focus 2).
- [ ] **Step 2:** seed files for `one-job/` (golden plan), then implement the subcommand (one batch
  in-process; two or more batches run one after another in-process until Task 10), two-phase planning, the
  union, view order and drop, the write point (11), the run lines and exit, the `exit_code.rs` row, the
  traces. `scripts/check-goldens --regen`, inspect the `one-job/` diff (only the seeds fill), then
  `scripts/check-goldens`.
- [ ] **Step 2b: one read per workspace path (closes the `ViewDemand.inputs` shortcut, Task 6 R6-2).** Before the
  first view, the manifest loader collects every job's (workspace path, `WorkspaceNeeds`) from its parsed argv, reads
  ONE `WorkspaceInputs` per path (absolutised as `ViewKey` does) with the `union` of those needs, and hands the same
  `Arc` to every job on that path. The bin's per-command `WorkspaceInputs::read` calls become this one reader (a
  standalone command is a one-job batch, so its read is unchanged). Test `batch_reads_each_workspace_once` (spawned,
  counters from the trace): two jobs on one workspace and one view (analyze cross-app + `digest`, which needs
  `fingerprint_rows`) and two jobs on one workspace and two views (analyze cross-app + `--single-app`) each read once
  (`inputs.read` 1), every job equals its standalone run, and the driver's `Arc::ptr_eq` assert (R5-3) does not fire.
  Proof: read per job → `inputs.read` 2, and the R5-3 panic on the shared view. Delete the `shortcut:` comment on
  `ViewDemand.inputs`.
- [ ] **Step 3: proofs** (spec §5): T3 break 1 one `View` per job → `assert_row` fails at program/ctx/
  witness = 2, no panic; T3 break 2 one `prepare` per job on one view → the `a View composes once` /
  `a View takes at most one witness request` panic; T4 build every view before any compute; T6 open job
  0's span around `prepare`; T11 skip the subcommand check; T13 bypass the driver in analyze; T15 job index
  in `RunKey`; T19 (a) `composes` from `!witness.is_empty()` → snapshot 0, `testCoverage` missing, (b) an
  empty request composes → cone 1; view-key spellings: canonicalise the key → the span count changes.
- [ ] **Step 4:** gate, commit (`#277 Task 7: alsem batch — manifest, one plan per view, views in order`), `check-diff`.

## Task 8 — writes, failures, stderr classes, diff across views (L)

Spec D2.2 (job write failure, run-transport failure, zero-view jobs), D2.4, D6 (0), D6.3 (`batch.held`),
D7 (from Task 1's inventory), F11, D4.1 (per-job build failure), D8 (batch half), D9 (graph on a failed
model), 12A-10, 12A-17; T5, T12, T21, T24; Review Focus 1.

**Files:**
- `src/engine/batch/manifest.rs`: `fn check_write_sets(&Manifest, cwd: &Path) -> Result<(), String>` (same
  file across and within jobs, a file inside a directory target, nested directories, read-after-write over
  spec D2.4's read list, workspace reads under a job's workspace path included).
- `src/engine/batch/mod.rs`: the zero-view phase before the first view; job write failures; the census line
  `[retained-census] batch.held outputs=<bytes> sides=<bytes>` at each view start; a failed view: each job
  maps it to its own text and exit, other views run. `src/engine/batch/output.rs`: `write_to_files`.
- Every site in Task 1's job-line list (expected: `src/engine/l4/summary_runner.rs`,
  `src/engine/l5/detector_context.rs`, `src/program/resolve/full.rs`, `src/engine/l4/capability_cone.rs`,
  `src/dependencies.rs`, `src/program/inputs.rs`, `src/app_package.rs`, `src/engine/l4/analysis.rs`, …;
  none in a hands-off file): the line becomes a value on the cell that produced it (as
  `L4Analysis.warnings` already is); each job's `serialize` replays the lines its standalone run printed
  (build lines always; L4 non-convergence iff `CORE_SUMMARIES` in its mask; det lines iff it has a run).
  The `log::` sites stay as they are (view lines, printed once when the view builds).
- Tests: `tests/cli/cli_batch_writes.rs` (T5, T24, `batch_relative_paths_two_anchors`),
  `tests/cli/cli_batch_failures.rs` (T12), `tests/cli/cli_batch_diff.rs` (T21).

- [ ] **Step 1: tests, red**:
  - **T5** (a)-(d) exactly as spec §5 (b-cases: exit 3, `build.program` 0, one line naming both jobs;
    (c) run stdout a closed pipe → Decision 49's text, run exit 1; (d) fingerprint `--out` in a missing dir →
    job exit 1, analyze manifest `stdout` in a missing dir → job exit 2, the write-error line on the
    worker's stderr, other jobs written, run exit 0).
  - **T24** (a) a file/file `diff` between two workspace jobs equals standalone and is written in job order;
    (b) every job fails validation → lines and stderr equal standalone, `build.program` 0, run exit 0.
  - **`batch_relative_paths_two_anchors`** (Review Focus 1): cwd ≠ manifest dir; the manifest `stdout`
    lands under the manifest dir, `--out` under the cwd.
  - **T12** exactly as spec §5 (a cross-app view that fails to assemble, holding digest + policy + graph,
    plus a single-app view that builds; a `log::warn!`-producing dependency shared by two jobs: the line
    once on the worker's stderr).
  - **T21** `[diff A B, diff B A]`, `[diff A B, analyze A]`, `diff A A` (D8's rows), an old side that fails
    with a new side that would print a build line.
- [ ] **Step 2:** implement the write-set checks, write failures and the zero-view phase; run T5, T24 and
  the two-anchor test green.
- [ ] **Step 3:** implement the stderr classes, per-job view failures and diff sides across views; T12 and
  T21 green; every golden unchanged (standalone replays every line in today's order).
- [ ] **Step 4: proofs**: write at compute → T5 (a) order; drop each path check in turn → each b-case; map
  a job write failure to the run's exit → T5 (d); skip the zero-view phase → T24 (a) no line, (b) nothing
  written; resolve `--out` against the manifest dir → the two-anchor test; one shared error text, no program
  for graph on a failed model, view lines per job → T12; finalize on the first view, replay the new side's
  lines on an old-side failure → T21.
- [ ] **Step 5:** gate, commit (`#277 Task 8: write sets, write failures, zero-view jobs; job/view/process stderr lines; diff sides across views`), `check-diff`.

## Task 9 — the contract tests and the `Arc` census (L)

Spec E13, I4, D11, 12A-5; T1, T2, T7 (and T22 written, ignored).

**Files:** `src/engine/l4/analysis.rs:104` (debug-only seam `ALSEM_TEST_SUMMARIZE_DIAGNOSTIC=1`: `build_l4`
pushes one non-convergence `SummarizeDiagnostic` ending in `NON_CONVERGENCE_TAIL` into `diagnostics`
BEFORE `render_warnings`; not `extra_diagnostics`), `src/engine/batch/mod.rs` (`job_refs` beside each
shared bucket under `ALSEM_RETAINED_CENSUS=1`); tests `tests/cli/cli_batch_contract.rs` (T1, T2),
`tests/cli/cli_batch_census.rs` (T7), `tests/cli/cli_batch_case_b.rs` (T22, `#[ignore = "needs step 9"]`).

- [ ] **Step 1: T1 `batch_equals_standalone`**: the full command/form list of spec §5 on `ws_dep()` and an
  r0 dir, both universes, ONE batch vs each job standalone; `RUST_LOG=off`, census and trace off; per job
  stdout bytes (`--deterministic` where available), stderr lines, exit, every file. Preconditions asserted
  (≥ 2 jobs per view, both universes, ≥ 2 tree jobs on one view from a separate traced run; two digests
  with different roots on one view; digest + policy on one view; two policies on one view). **T2**
  `batch_output_independent_of_neighbours` (J's subjects a strict subset of its view's union, asserted).
  **T7** `batch_arc_census` (default-detector jobs only; one distinct `demanded` mask asserted; dedup of
  model/rel/det/l4 = the largest standalone's exact bytes, `runs` = Σ over distinct `RunKey`s, `job_refs`;
  `raw` between max and Σ of the jobs' standalone `raw`). Every failure is a defect fixed at its root (F1),
  never an allow-list. Write T22 as spec §5 states it, ignored until Task 11.
- [ ] **Step 2: proofs** (T1, spec §5): **B1** drop the L4 line replay (with the seam on; precondition:
  standalone digest's stderr has the line); **B2** give each tree the whole union `raw`; **B3** drop scope
  from `RunKey` (analyze `--scope all` differs); **B5** drain mode for a digest tree while policy shares the
  view → policy's `has a summary but no raw inherited facts` panic; **B6** `caps.pd.take()` per tree → the
  second digest's proofs differ (precondition: its standalone JSON has ≥ 1 proof). T2: union from the first
  job only. T7: clone the `RunOutput` per job → `runs` doubles.
- [ ] **Step 3:** gate, commit (`#277 Task 9: batch-equals-standalone contract, neighbour independence, Arc census`), `check-diff`.

## Task 10 — workers, admission, the peak table, threads (M, quiet slot for the provisional rows)

Spec D2.5, D3 (stdin bytes to children), D10 (trace file per child), D12, F18, Limit L1, 12A-6, 12A-14;
T8, T10, T17; Review Focus 4.

**Files:** create `src/engine/batch/admission.rs`, `src/engine/batch/worker.rs`; modify
`src/engine/batch/mod.rs`, `src/big_stack.rs:66-71` (the `pool.threads` instant from the built pool's
`current_num_threads()`), `src/bin/alsem.rs` (`batch --help` states Limit L1 and that the table is
CDO-calibrated); tests `tests/cli/cli_batch_workers.rs` (T8, T10, T17 cli, `batch_worker_killed`), T17 lib
in `admission.rs`; `CHANGELOG.md`.

**Interfaces (produced):**
- `pub const E16_BUDGET_KIB: u64 = 29_360_128;`
- `pub static KIND_PEAK_KIB: &[(JobKind, u64)]`, `JobKind = (command, ViewUniverse)`, each row rounded up
  to the next 256 MiB, with a comment citing its numbers file. Rows now: spec D12's known values
  (analyze cross-app, digest, policy cross-app, fingerprint B0 single-app, analyze single-app) and a
  provisional row for every other kind from Step 1. A kind with no row takes the largest row.
- `pub fn worker_count(kinds: &[JobKind], batches: usize, has_policy: bool, table: &[(JobKind, u64)]) -> usize`
  = `max(1, min(batches, 4, E16_BUDGET_KIB / P))`, then `min(·, 2)` if `has_policy`; `P` = the max row
  over `kinds`. `⌊E16_BUDGET_KIB / P⌋ = 0` prints `al-sem batch: the largest measured job peak exceeds the 28 GiB budget; running one worker`.
- Workers: one child per batch (`current_exe()`, `batch --worker <b> <manifest>`), at most `W` alive,
  started in manifest order; child stderr forwarded whole line by whole line with `[w<b>] `; job lines
  collected and printed in manifest order after the last child; stdin read once by the parent and passed
  on a child's stdin when it has a `--diff -` job; `ALSEM_TRACE_FILE` → `<file>.w<b>`; the
  `batch.workers` instant (`w`, `p_kib`, `policy_cap`) and child start/exit instants; under
  `ALSEM_RETAINED_CENSUS=1` each worker prints `[batch] worker <b> peak_kib=<VmHWM>` (omitted when
  `/proc/self/status` is absent). No `RAYON_NUM_THREADS` read or set.

- [ ] **Step 1: provisional rows** (quiet slot, one run at a time, release-fast, 3 rounds each through
  `maxrss.py`): events fanout (x, s), query touches (x, s), graph, prove (case A), `diff $WS $WS`, policy
  check single-app, digest single-app. Ledger rows; Task 12's Part 4b replaces them. (Needed so T17's
  committed-table case, `U−p` ×4 → `W` 4, does not fall back to the largest row for events and query.)
- [ ] **Step 2: tests, red**: **T17** lib over an injected table (`⌊budget/P⌋` = 0, 1, 3, ≥ 4 → 1, 1, 3, 4
  without policy, 1, 1, 2, 2 with); lib over the committed table (`AD`, `ADF`, `U−p`, `O` ×4 → 4; `U` ×4 →
  2); cli with 1, 3, 5 batches and 3 batches with one `policy check` → `batch.workers` 1, 3, 4, 2, never
  more than `W` children alive. **T8** spawned with `RAYON_NUM_THREADS=2` in the child's environment →
  `pool.threads` 2. **T10** two `--diff -` digests in one batch and split over two batches → both get the
  diff. **`batch_worker_killed`** (Review Focus 4).
- [ ] **Step 3:** implement.
- [ ] **Step 4: proofs**: drop the policy cap → 3; drop `max(1, …)` → 0; charge `U`'s peak to every kind
  → `ADF` gets 3 (T17); `.num_threads(available_parallelism)` (T8); read stdin per job → the second sees an
  empty diff (T10); treat a missing line as exit 0 (`batch_worker_killed`).
- [ ] **Step 5:** gate, commit (`#277 Task 10: workers under the E16 budget; the peak table; pool.threads`), `check-diff`.

## Task 11 — case B: rebase onto step 9 (M, conditional)

**Runs only if step 9 merges before #277** (spec §9, case B). Spec §9's case-B bullets, D5.2's `TreeRead`
variants, D5.4 (`tree_reads: BTreeMap<JobId, TreeReadOut>`, `take_tree_read(job)`, `Cell::ALL` unchanged),
D10's restated rows; T22 activates; T19 (c).

**Files:** whatever step 9 touched in `session.rs`, `prove.rs`, `fingerprint_cli.rs`, the counter rows;
`tests/cli/cli_batch_case_b.rs`, `tests/cli/cli_batch_plan.rs`.
**Hands-off warning:** step 9 owns `src/engine/l5/fingerprint_query.rs`. If moving the fingerprint query's
root resolution into `resolve(&Model)` needs an edit there, stop and ask the conductor first.

- [ ] **Step 1:** rebase; turn every `Roots(ids)` into `WitnessRequest::roots(ids)`; move step 9's
  `Plan.tree: TreeRead` into `TreeDemand.read` (`None` → no tree demand); make the `TreeRead` cell per job;
  move prove's and the fingerprint query's root resolution into `resolve(&Model)`; if step 9 widened
  `View.released` to `u16`, keep it; renumber a cell bit only if #277 added one (it adds none).
- [ ] **Step 2:** remove T22's `#[ignore]` (`batch_tree_reads_use_own_subjects`) and add T19 (c)
  (`fingerprint --inventory-only`: composes once, witness 0). Restate D10's rows for prove (`DIGEST_ROW` in
  a batch view) and T3's case-B rows.
- [ ] **Step 3: T22 proof**: give every tree read the whole union `raw` → inventory gains inherited facts /
  dependency apps and the R2 query gains R1's dependency rows (preconditions as spec §5).
- [ ] **Step 4:** gate, commit (`#277 Task 11: case B — step 9's tree reads as per-job TreeDemands`), `check-diff`.

## Task 12 — the CDO bench, the table, `cdo-gate` (L, X)

Spec §7.1-§7.5, §8 (F2-F17), A12.8-A12.11, E14, I2; every bound.

**Files:** `docs/measurements/2026-10-<dd>-issue-277-160-step12.md`; `src/engine/batch/admission.rs` (the
measured rows, Part 4b); `CHANGELOG.md`. Code otherwise only if a fallback fires (its own commit, tests,
proofs, gate, `check-diff`).

- [ ] **Step 1:** binaries `P` (branch base) and `H` (final-panel head); bench script
  `/work/bench/160-12/final/bench.sh` from `/work/bench/160-12/bench-draft.sh` with spec §7.1's jobs and
  batches (`AD`, `ADF`, `AS`, `U`, `O`), interleaved, load-gated. Every run in the shared cgroup with the
  exclusive slot held.
- [ ] **Step 2: Part 1**: histogram SHA P = H; SARIF P vs H (both scopes, both universes); per job of
  `AD`, `ADF`, `AS`, `U`, `O` byte-equal to H standalone (`generatedAt` removed); counters vs D10; census of
  `ADF`, `U`, `O` and each standalone (D11's two rules; `batch.held`). A miss → F1; a counter miss → STOP.
- [ ] **Step 3: Part 2** (5 rounds): **B1a**, **B1b** (void checks; F2, F3), **B2**, **B3**, **B2u**,
  **B2o** (F4, F5, F6, F13, F16).
- [ ] **Step 4: Part 3** (3 rounds, P vs H): **B4** for every rewired command and universe of spec §7.1
  (`diff $WS $WS` read against D8's change: its peak may drop, never rise). F8, F15.
- [ ] **Step 5: Part 4b first** (the table): one `maxrss.py` row per (command, universe) not measured in
  Parts 2-3, 3 rounds; each row = max(standalone median, the median of every shape whose heaviest
  standalone member is that kind); commit the table (`#277 Task 12: KIND_PEAK_KIB from the step-12 bench`),
  gate, `check-diff`; rebuild `H`.
- [ ] **Step 6: Part 4** on the rebuilt `H` (3 rounds; `N` = `nproc`, `Q` = ⌊N/4⌋ set by the bench):
  `W1f`, `W1q`, `W4` (four `ADF`), `W2p` (four `[p]`: `batch.workers` 2), `W4u` (four `U−p`): **B5**,
  **B5p**, **B6**, **B7** (F9, F10, F17).
- [ ] **Step 7:** `cdo-gate` on the final committed head, as `bash scripts/cdo-gate` through
  `agentflow run --name cdo-gate`; exit 0.
- [ ] **Step 8:** numbers file (Parts 1-4b, case A or B, every job's argv, `W`, the table rows, Task 1's
  projections against the measured values, fallbacks fired); CHANGELOG; `check-diff`; ledger
  `## Gate results`. Have the `measurement-auditor` agent check the numbers file before the final panel.

---

## Coverage map

| Spec test | Task | | Bound | Task |
|---|---|---|---|---|
| T1 | 9 | | B1a | 12 (projected in 1) |
| T2 | 9 | | B1b | 12 (projected in 1) |
| T3 | 7 | | B2 | 12 |
| T4 | 7 | | B2u | 12 (projected in 1) |
| T5 | 8 | | B2o | 12 (projected in 1) |
| T6 | 7 | | B3 | 12 |
| T7 | 9 | | B4 | 12 |
| T8 | 10 | | B5 | 12 (projected in 1) |
| T9 | 2 | | B5p | 12 |
| T10 | 10 | | B6 | 12 (projected in 1) |
| T11 | 7 | | B7 | 12 (projected in 1) |
| T12 | 8 | | | |
| T13 | 7 | | | |
| T14 | 3 | | | |
| T15 | 7 | | | |
| T16 | 1 | | | |
| T17 | 10 | | | |
| T18 | — (the spec has no T18) | | | |
| T19 | 7 ((c) in 11) | | | |
| T20 | 5 | | | |
| T21 | 8 | | | |
| T22 | 11 (written ignored in 9) | | | |
| T23 | 2 | | | |
| T24 | 8 | | | |
| T25 | 4 | | | |

Acceptance rows: A12.1 → 7, 8; A12.2 → 3, 7 (+11); A12.3 → 5, 7, 8; A12.4 → 10; A12.5 → 5, 8, 9 (+11);
A12.6 → 7; A12.7 → 9; A12.8-A12.11 → 12; A12.12 → 7; A12.13 → every task's gate + the golden plan;
A12.14 void; E14 / I10, I2 → 12.

## Golden plan

**One new family at `tests/gate-goldens/batch/`. No existing golden moves** (every task's `check-goldens`
runs with no regen; Task 7's one regen fills only its own seeds).

**Why `--test cli` covers it, and the coverage guard does not need a `scripts/` edit:**
- `scripts/check-goldens --verify-coverage` lists only top-level directories (`ls -d tests/*goldens*/ …`);
  `tests/gate-goldens` is already in `declared_dirs` (line 99), so a subfolder adds no undeclared directory.
- The family's tests are members of the `cli` umbrella (`tests/cli/*.rs`, each registered in
  `tests/cli/main.rs`), and `--test cli` is in `check-goldens`' `targets`. Each test opens its golden files by
  explicit path under `tests/gate-goldens/batch/`, so `check-goldens` runs them.
- The existing `gate-goldens` tests (`gate_sarif_differential`, `gate_prsummary_differential`,
  `gate_suppress_baseline_differential`) read named files and do not walk the directory (no `read_dir`),
  so a new subfolder does not disturb them. Task 1 confirms this by running `check-goldens` after adding the
  records.

| Subdirectory | Contents | Minted by | Regenerable |
|---|---|---|---|
| `merged-streams/` | `degraded-analyze`, `policy-diagnostics-out`, `policy-out-fails`: `.merged.golden` (the `2>&1` bytes) + `.exit`; a `RECORD` marker | Task 1, from the base binary `1134bed1` | **Never.** Records of today's behaviour (T20) |
| `update-baseline/` | `base.stdout.golden` (empty), `base.stderr.golden`, `base.exit` (3); a `RECORD` marker | Task 1, from the base binary | **Never.** The record of the stated change (T23, S17) |
| `one-job/` | per command, the one-job manifest's run line (paths relative to the manifest dir) and the job's stdout file, `--deterministic` where available | Task 7, through `REGEN_TEMP_GOLDENS=1` | Yes |

**The no-regen guard (T20, T23).** The base records are never rewritten under `REGEN_TEMP_GOLDENS`:
`tests/cli/batch_records.rs` (Task 2) holds `read_record(path) -> Vec<u8>` and the family's
`write_golden(path, bytes)`, which panics with `refusing to rewrite a base-binary record: <path>` when the
path's directory holds a `RECORD` file. T20 and T23 read through `read_record`, compare under regen as well
(they never skip and never write), and have no write path. Task 2 adds the guard's own test,
`record_dirs_refuse_regen` (calls `write_golden` on a `RECORD` path and expects the panic; break: drop the
`RECORD` check → no panic), and Task 5's guard proof pins it again for T20.

**Seed files** (the regen rewrites goldens but cannot mint one): Task 7 commits, before its regen, one seed
per `one-job/` golden (`<cmd>.line.golden.json` holding `{}` and `<cmd>.stdout.golden` empty), so the
test's existence check passes and the regen fills them; the diff is inspected before commit. `README.md` in
the family root says which subdirectories are records.
