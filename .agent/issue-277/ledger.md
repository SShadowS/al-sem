# Issue #277 ledger: attempt 1 on server1 (#160 step 12: `alsem batch`)

## Attempt 1: claim

```json
{"run_id":"20261024-100928-5fc9b8","issue":277,"attempt":1,"branch":"issue/277-160-step-12-alsem-batch-a1","worktree":"/work/al-sem-issue-277-a1","body_hash":"0e7ba9a0f5cba393918011fabd6fce23da0c5cdc436b0559d20175f1d13261b1"}
```

Issue body hash: `0e7ba9a0f5cba393918011fabd6fce23da0c5cdc436b0559d20175f1d13261b1`.
Base `1134bed1` (master; includes step 6 at `b028ec32`, #274, and the grammar bump `317d3b34`/`c89c2ecb`, #278).
Pre-draft `/work/bench/160-12/draft-step12-design.md` (refreshed 2026-10-24 against step 6's head `a25d0cad`),
re-cited here against `1134bed1`. Operator decisions taken before the claim:
`/work/bench/160-12/OPERATOR-DECISIONS-2026-10-24.md`. Step 9 seam decisions relayed by the conductor
(2026-10-24, from step 9's owner).

### Attempt 1: classification

**architectural**: a SPEED step (epic row 12 carries a wall bound, a peak bound and E16's memory and
throughput bounds) that adds a new command, a new driver every view command runs through (standalone is a
batch of one), changes the `WitnessRequest` and `Plan` types on `View`, adds a worker pool with a memory
budget, and touches every command pipeline's prologue.

### Attempt 1: decisions already taken (written into the spec as decided)

- B1a: AD batch < standalone a + d. B1b: ADF batch < standalone a + d + f. (Operator, 12A-4.)
- E16 budget 28 GiB (shared cgroup `memory.max` 32 GiB). A batch run with `policy check` uses at most 2 workers,
  enforced by `alsem batch`. (Operator, 12A-6.)
- Case A counter row: fingerprint, prove, diff build their own single-app cut snapshot until step 9. (Operator, 12A-11.)
- `WitnessRequest` set form `{ ids: BTreeSet<String>, primary: bool }` with `union`: agreed with step 9's owner;
  #277 lands it. Step 9 uses only `Roots(ids)`; whichever merges second rebases (`Roots(ids)` →
  `{ids, primary: false}`). (Superseded by step 9's round 1, below: no `TreeUse`, no `take_tree`.)
- Step 9 round-1 seam (conductor, 2026-10-24): no kept trees (a cross-app tree is ≈ 282 MiB, 201 MB pd arena);
  every tree is read and dropped inside `prepare` before `det`; step 9 adds `TreeRead { Effects,
  FingerprintQuery, Inventory }` + `Cell::TreeRead` (`Cell::ALL` unchanged), which becomes a field of #277's
  `TreeDemand`; each read over its own job's subjects; a case-B byte test (T22) activates on the rebase.
- Step 9 outcome S′: fingerprint B0 and `diff` stay single-app; cross-app forms are prove, `fingerprint --routine`,
  `--inventory-only` (≈ 3.1-3.3 GB each, estimate). Worker count derived from the 28 GiB budget and per-kind peaks.
- Step 9 review seam (conductor, 2026-10-24): an empty `WitnessRequest` never means "no composition"; the plan's
  tree demand carries it (chosen over a `None` state; spec D5.1a, T19). Only policy asks `primary`; fingerprint's
  cross-app forms ask `Roots(pool)` / `Roots(∅)`; B0 (the only other `primary` reader) stays single-app on the cut.

### Attempt 1: assumption probes

Binary for runtime probes: `cargo build --profile cdo --bin alsem` in this worktree at `1134bed1` (1 m 08 s;
correctness profile, so walls below are not quoted as numbers, only ratios within one binary).
Probe output: `/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/pn6/`.
CDO = `jq -r .cdo_ws /work/alsem/.agent/config.json` (read-only). Cgroup at probe time: 32 GiB max, ≈ 8.4 GiB
anon in use (other sessions), load 6-10. Probes ran one at a time; none above 2 GB.

| probe | result | evidence |
|---|---|---|
| P0 no batch yet | no `Batch` in `enum Commands`; no `JobOutput`, `JobId` anywhere in `src/` | `src/bin/alsem.rs:100-122`; `grep -rn 'JobOutput\|JobId' src` → 0 |
| P0 `View` | fields `target, rel, caps, raw, tree, effects, l4, det, released, ordering, runs`; ONE `tree: Option<CapabilitySnapshot>` and ONE `effects: Option<DigestEffects>` per view (not per job) | `session.rs:30-56` |
| P0 `Cell` | `Det, L4, Ordering, Rel, Caps, Raw, Tree, Effects`; `Cell::ALL` = 5 (Caps, Tree never outlive `prepare`) | `session.rs:127-145` |
| P0 `WitnessRequest` | `enum { None, Roots(BTreeSet<String>), Primary }`; `scope(model)` maps Roots by `stable_routine_id`, Primary by `Role::Primary` | `session.rs:89-124` |
| P0 `Plan` | `runs: Vec<Vec<Detector>>`, ONE `scope_primary`, `need_l4`, `after_run`, `witness`, `needs_tree: bool`, `tests: bool`; `RunKey` = (detectors, demanded, scope_primary) | `session.rs:60-86` |
| P0 composition assertions | `a View takes at most one witness request, in its one composing prepare` (`:405`); `a tree is composed over a Roots witness request` (`:408-410`); `a View composes once, in its first composing prepare` (`:427`); `the dependency calls left the base in an earlier prepare` (`:525`) | `session.rs:400-530` |
| P0 tree in `prepare` | the tree is composed inside `prepare`, from `raw.take()` (ALL of `raw`), then effects, then `tree = None`, before `det` | `session.rs:517-551` |
| **P0 NEW: pd arena taken by the tree** | `compose_tree` does `caps.pd.take().unwrap_or_default()`; a SECOND tree on the same `caps` would get an EMPTY arena and a different `pd_solution`, silently. `det` does not read `caps.pd` (no `.pd` read in `detector_context.rs` outside the fill) | `snapshot.rs:1397`, `:1424-1432`; `detector_context.rs:1392` |
| **P0 NEW: dependency calls taken once** | cross-app `prepare` takes `b.dependency_calls` once and drops it after the (one) tree; a second tree job would hit the `left the base` panic | `session.rs:463-472`, `:519-528`, `:543` |
| P0 `compose_tree` drains `raw` | subjects = `raw.keys()`, facts by `raw.remove(id)` | `snapshot.rs:1396`, `:1415` |
| P0 `JobOutput` | absent. The writer side exists: `write_error` ("al-sem: write error: <target>: <e>"), `write_stdout`, `write_file`, `write_stderr`, `write_out`, `write_stderr_lines` in `alsem.rs` | `alsem.rs:721-770` |
| P-S1 analyze default = digest run | analyze `analyze_plan(detectors, scope==Primary)`, default detectors from `presets.rs:119` `default_detectors()`; digest `digest_plan(default_detectors(), roots, tests)` with `scope_primary: true`. Same `RunKey` at `--scope primary` | `run.rs:516`, `digest_cli.rs:1675`, `:1776`, `session.rs:164-186` |
| P-S2 policy reads raw by id | `model.raw.get(routine.id.as_str())`, panics "has a summary but no raw inherited facts" | `policy_engine.rs:270-278`, `pipeline.rs:275` |
| P-S5 builders | 8 production `View::new` sites (events, diff, run, policy, effect_query, prove, digest, fingerprint). Single-app: `AnalysisTarget::build` → `assemble_and_resolve_workspace_program(inputs, id, false)` = `ResolvedProgram::build(Workspace)` + `assemble_and_resolve_workspace_from_program(.., false, ..)` + `attach_program_calls`; analyze's `build_analysis_model(.., true)` = the same calls plus `physical`, `object_facts`, `fresh`. Cross-app: both call `build_cross_app_base` | `run.rs:180-203`, `:283-330`, `:370-395`; `program_calls.rs:686-700` |
| P-N10 (static half) | the three single-app builders run the same three functions; only fingerprint passes a non-`false` `skip_roots_config` (`alsem.rs:1345`). Runtime equality on r0 is Task 1's first test | as above |
| P1 stderr sites | 96 `eprint`/`eprintln` lines in `src/` outside `bin/`, `lsp/`, `server`, `watcher` and test files (the pre-draft counted 99 with a different filter); Task 1 makes the exact inventory | `grep -rn` (ledger command) |
| P7 policy load after build | `match resolve_policy_check(opts.inputs)` at `pipeline.rs:223`, after `View::new` (`:205`) | `pipeline.rs` |
| P8 stdin | one reader `read_to_string(&mut buf)` `alsem.rs:1101`; one `is_terminal` `:1553` | `alsem.rs` |
| Counter rows | `DIGEST_ROW` = program 1, bcg 1, cone 1, v2 1, scc 2, pd 1, ctx 1, snap 1, witness 1; `FINGERPRINT_ROW` = 1,2,2,1,3,2,1,1,1; policy cross-app = 1,1,1,1,2,1,1,0,1; analyze cross-app default = 1,1,1,1,2,1,1,0,0; `inputs.read` = program | `tests/cli/cli_build_counters.rs:92-125`, `:196-211`, `:378-388`, `:491-501`, `:673-688` |
| P11 step 9's cross-app forms | answered by step 9's spec (S′): prove, `fingerprint --routine`, `--inventory-only` cross-app ≈ 3.14-3.3 GB (estimate); B0 and `diff` stay single-app (≈ 1.47 GB); cross-app B0 killed at ≥ 29.1 GB | `/work/al-sem-issue-160-a10/docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md` §4.6, §8.0 |
| **P-N6 peaks after a view drop** | `fingerprint $WS --format json` single-app: 1,466,292 / 1,470,176 KiB. `diff $WS $WS` (two single-app views in one process, A then B): 1,631,876 / 1,628,576 KiB; `--strict`: 1,630,500 / 1,635,376 KiB. Ratio diff / fingerprint = 1.11, not ≈ 2: the second view does NOT stack on the first. The +160 MB is consistent with side A's snapshot held for the comparison (not separately measured). cdo profile, 2 rounds each, sequential | `pn6/{f,d,ds}-{1,2}.err` |

Deferred to the measurement plan (need release-fast and/or an exclusive slot; not run now because step 9's
session shares the 32 GiB cgroup):

| probe | why deferred | where it runs |
|---|---|---|
| P-N5 phase traces of `a`, `d` | wall attribution must come from release-fast | Task 1 (projection before the code), spec §7.4 |
| P-N7 four concurrent `d` at `RAYON_NUM_THREADS=5` | ≈ 12.6 GB together and needs an exclusive slot | Task 1, spec §7.4; no exclusive slot → the driver task waits and the conductor schedules P-N7 (F19) |
| P-N8 default `a` wall and peak | release-fast | Part 2 (it is a bench row) |
| P-N9 raw bytes of `d`/`h` roots | needs a census line (bytes), not just `witness.members` | Task 1 adds the line; F13 is pre-stated |
| P-N10 runtime half | a test, not a probe | Task 1's first test (T16) |
| Peaks of events, query, graph, prove (case A) for the E16 table | release-fast | Part 4b, spec §8 |

### Attempt 1: spec panel round 1

Reviews: `/work/alsem/.agent/runs/20261024-100928-5fc9b8/spec-r1-sol.md` (S1-S14) and `spec-r1-opus.md` (O1-O15);
round-1 snapshot `spec-r1.md` / `ledger-spec-r1.md` in the run dir. Every finding re-checked against `1134bed1`
before acting; register `.agent/issue-277/findings.json` (one entry per finding, disposition, evidence, anchor).
Conductor decisions written into the spec as decided: Q-a..Q-d; F18 activated (no thread code); the stop rule
(no second view or pass, S8); memory (S6/S7/O9: write-as-soon-as-ordered, held-bytes bound, measured shapes
`U`/`O`, Limit L1); the rest per the conductor's list. Spec §0.1 maps each change to its findings.

Extra code checks made in round 1 (all at `1134bed1`):

| check | result | evidence |
|---|---|---|
| bin `eprintln!` count | 49 | `grep -c 'eprintln!' src/bin/alsem.rs` |
| `log` sites in the shared build | 12 `warn!`/`info!` in `dependencies.rs`, `snapshot/cache.rs`, `program/model/workspace.rs`; env_logger init `alsem.rs:688` | grep |
| analyze writes its warning before stdout; policy its diagnostics after its output | `alsem.rs:1664-1670`; `:921-925` | read |
| `--update-baseline` writes inside compute | `run.rs:594` `save_baseline(Path::new(path), &summaries)` | read |
| diff loads old then new, strict gate, then renames | `diff/cli.rs:224`, `:228`, `:240`, `:254` | read |
| policy `--strict` gate runs before the policy load | `pipeline.rs:211-219` before `:223` | read |
| `witness_pass` counts only non-empty raw | `capability_cone.rs:3221` `if !raw.is_empty() {` | read |
| digest's `pd_solution` reader | `digest.rs:186-189` (read only by `enumerate_proofs`) | read |
| `assert_row` ties `inputs.read` to `program` | `cli_build_counters.rs:120-121` | read |
| `diff ws ws` standalone rows | program 2 (`:541-559`), `--strict` ctx 2 (`:563-590`) | read |

### Attempt 1: spec panel round 2

Reviews: `spec-r2-sol.md` (re-raises A-D of S2/O15, O2, S11, S13; new S15-S18) and `spec-r2-opus.md` (all 29
accepted; new O16-O19); snapshot `spec-r2.md` in the run dir. Conductor decisions: O16/S15 fixed by charging a
shape's peak to its heaviest standalone member's row (chosen over rows keyed by shape); the other items per the
conductor's list. Step 9 round 2 (`eb754df0`): tree reads fully owned (drop each tree right after its read);
release mask `u16`. Spec §0.1 lists the round-2 changes. Register updated (`findings.json`: round-1 marks,
round-2 entries, all anchors re-hashed).

Code checks made in round 2 (all at `1134bed1`):

| check | result | evidence |
|---|---|---|
| analyze has no `--out` | `AnalyzeCli` flags at `alsem.rs:454-529`: no `out` | read |
| digest no-input vs no-root | no input → `Err` at `digest_cli.rs:1745-1749`; no root → `digest_plan(.., ∅, tests)`, `prepare`, run, tx map, `l4`, then exit 2 (`:1776-1831`; `DigestRunResult.exit_code` "2 = zero roots", `:1647`) | read |
| `extra_diagnostics` never print | appended after `render_warnings` (`analysis.rs:104-106`); doc `:40-44` | read |
| single-app flags on events, policy, query | `alsem.rs:189-192`, `:267-272`, `:364-368` | read |
| baseline write failure today | `run.rs:594-595` `Err("failed to write baseline …")`; bin prints `al-sem: …`, exit 3 (`alsem.rs:1675-1689`) | read |
| `derive_capability_facts` accepts a borrowing accessor | `snapshot.rs:1637-1642` (`R: Borrow<[CapabilityFact]>`); cut path borrows at `:1305` | read |
| `log::warn!` in `app_package_zip.rs` | 3 sites, `:142`, `:175`, `:253` | grep |
| diff sides only read the model | `diff/cli.rs:134-141` `compose_full_snapshot(view.model(), &opts)`; `same_workspace` canonical, `:179-184` | read |
| release mask | `session.rs:53` `released: u8,` | read |

### Attempt 1: spec panel round 3

Opus approves on condition O20 (`spec-r3-opus.md`); sol's round 3 runs later. O20 fixed in D5.3 (borrow mode:
subject set from the `TreeDemand`, every raw reader through one subject-filtered accessor; drain mode gets an
extracted own-subjects map) and T25. Register: O20 added (round "12 r3"); O16-O19 and S15-S18 marked accepted
by Opus; every anchor re-hashed.

| check | result | evidence |
|---|---|---|
| raw readers in `compose_tree` walk every routine | subject set `raw.keys()` `snapshot.rs:1396` (scopes the pd arena `:1424-1431`); operation index `for r in base.ws_routines` … `raw.get(&r.id)` `:2132`/`:2143`; callsite index `:2258`/`:2269`; `derive_capability_facts` `take_raw(&r.id)` over `inp.ws_routines` `:1658` | read |
| raw entries only for walked scoped routines (O21, `spec-r3b-opus.md`) | `capability_cone.rs:2828` skips members outside `routine_ids`; `:2950-2951` inserts `raw_out` only for walked members. D5.3 borrow subjects = scope ∩ map keys; T25 (b). O20 accepted by Opus | read |

## Spec panel (conductor)

- Round 1 (spec 681 lines): sol S1-S14 (S6 critical: standalone peaks do not bound a batch), opus O1-O15; both "do not approve". Conductor decisions: Q-a..Q-d as both reviewers agreed (run exit 0 with per-job exits; job write failure = that command's standalone write-failure code; multiple workspaces; one tree per digest; census/trace on worker stderr); S9/O10 F18 active (no RAYON_NUM_THREADS code); S8 a one-build/one-pass miss is a STOP; S6 no arbitrary-manifest memory claim, bounded retention, measured shapes U/O.
- Round 2 (906 lines): opus accepted all 29, raised O16 (peak table charged U's peak to every kind → W 3); sol re-raised S2/O15/O2/S11/S13 and raised S15 (= O16), S16-S18. Conductor: a shape's peak goes to its heaviest member only.
- Round 3 (998 → 1021 lines): opus accepted all and raised O20 (borrow-mode readers not subject-filtered) and O21 (subjects = scope ∩ map keys), both fixed; sol (after a usage-limit wait) accepted all but re-raised O16/S15 for a leftover Part 4b sentence.
- Round 4: conductor fixed Part 4b (L809-813); both accepted O16 and S15; both approve. Register: 39 entries, all accepted by both. spec_rounds remaining 1.
- #160 step 9 seam agreed with alsem-main: #277 lands `WitnessRequest { ids, primary }` + `union`; empty request ≠ no composition (tree demand forces it); no kept trees, every tree read inside `prepare` before `det`, outputs fully owned; step 9's `TreeRead` becomes a field of `TreeDemand`; release mask u16; whichever merges second rebases.

## Task 1

Commits (branch, not pushed): `1fb6894d` base-binary records for T20 and T23; `2d6b850d` `witness.raw_bytes`
census key + T16. Gates at `2d6b850d`: `ci-steps task-affected` exit 0 (4057 passed, 8 skipped);
`check-goldens` exit 0 (1110 passed, no golden moved); pre-commit hook passed; `check-diff` vs
`origin/master` (= `1134bed1`) `reasons: []`.

- **Records** (from `alsem` built at `1134bed1`, `cdo` profile, clean detached worktree
  `/work/bench/160-12/base-wt`, run from the repo root, `RUST_LOG=off`; asserted by eye on the bytes):
  `tests/gate-goldens/batch/merged-streams/`: `degraded-analyze` (`analyze tests/r0-corpus/ws-txn-d47-pos-file
  --deterministic`: `al-sem: warning: analysis coverage degraded …` first, then the report; exit 0);
  `policy-diagnostics-out` (`policy check tests/fixtures/cli-c-policy/ws-policy-commit-in-subscriber --format json
  --deterministic --out <tmp>`: 3 `warning:` diagnostic lines, stdout empty; exit 0); `policy-out-fails` (same,
  `--out tests/gate-goldens/batch/merged-streams/no-such-dir/p.json`: one `al-sem: write error: …: No such file or
  directory (os error 2)` line, diagnostics not printed; exit 1). `update-baseline/`: `analyze tests/r0-corpus/ws-d1
  --deterministic --baseline tests/gate-goldens/batch/update-baseline/no-such-dir/b.json --update-baseline`: stdout
  0 bytes, stderr `al-sem: failed to write baseline '…': No such file or directory (os error 2)`, exit 3. `RECORD`
  in both directories; `README.md` names the argvs. The record-writer guard is Task 2's.
- **Census**: `witness.raw_bytes=` added; on a copy of `ws_dep()` (`digest . --diff change.diff`, debug binary)
  the line reads `witness.members=1 witness.non_primary_members=0 witness.raw_bytes=81` (the one key; that root
  has no facts). Uses `cone_census::capability_fact_heap_bytes` (the existing helper) plus vector capacity.

## Discrimination proofs

| test | break | fail | pass | commit |
|---|---|---|---|---|
| T16 `single_app_builders_agree` | `AnalysisTarget::build` passes `skip_roots_config` `true` (run.rs:193) | `run_builders_tests.rs:89` "ws-d51-jobqueue (no_roots_config=false): AnalysisTarget::build differs" | reverted, 1 passed (20.6 s) | `2d6b850d` |
| T9 `decision7_precedence_policy_and_build` | `resolve_policy_check` moved after the `--strict` gate in `run_policy_check` (validate after the build) | `cli_decision7_precedence.rs:59` `build.program` left 1, right 0 | reverted, passed | `346f7fad` |
| T9 `decision7_precedence_shard_out_and_build` | the `--shard`-without-`--out` block moved back after `compose_full_snapshot` in `run_fingerprint_pipeline` | `cli_decision7_precedence.rs:87` `build.program` left 1, right 0 | reverted, passed | `346f7fad` |
| T9 `decision7_precedence_policy_and_strict` | the same policy break as the first row | `cli_decision7_precedence.rs:124` stderr = the multi-app `error:` diagnostic, not `policy load error: ` | reverted, passed | `346f7fad` |
| T23 `update_baseline_write_failure` | compute-side write restored in `run_analyze_with_exit` (`std::fs::write(..).map_err("failed to write baseline '…'")?`) | `cli_update_baseline_write.rs:77` stderr `al-sem: failed to write baseline '…': No such file or directory (os error 2)` vs `al-sem: write error: …` | reverted, passed | `346f7fad` |
| `record_dirs_refuse_regen` | the `RECORD` assert dropped from `write_golden` | `batch_records.rs:50` "write_golden must refuse a record" (no panic) | reverted, passed | `346f7fad` |
| `inputs::tests::needs_union` (lib) | `union`'s `build_only` `&&` → `\|\|` | `inputs.rs:657` left `build_only: true`, right `false` | reverted, passed | `346f7fad` |

All breaks made with Edit and asserted applied (`grep -c` of the break text = 1 per file) before the run; the four
cli breaks ran in one run (each test reads only its own break), 5 failed / 1 passed (the success-case T23 test,
which no break targets).

## Task 1 projection

Binaries: `cargo build --profile release-fast --bin alsem --bin aldump` in clean detached worktree
`/work/bench/160-12/task1-wt` at `2d6b850d`, copied to `/work/bench/160-12/task1/bin` with `bins.sha`. Script
`/work/bench/160-12/task1/bench.sh`, through `agentflow run --name bench`; load gate (1-min ≤ 4 and no alsem
process) before each part; 15 s ticks of load, own running alsem threads and foreign alsem processes. `maxrss.py`
(`VmHWM`, mimalloc as shipped, no `MIMALLOC_PURGE_DELAY`). Argvs as spec §7.1 (no `--deterministic`). Reading:
`/work/bench/160-12/task1/reading.py` → `reading.txt`.

Contamination: P-N5's first run (`pn5-void`) and its re-run (`pn5-r2`) overlapped another session's CDO alsem runs
(#264) and step 9's bench builds (load ticks 60-170); both void for walls. P-N5 was run a third time (`pn5-r3`,
every tick ≤ 4.6, foreign 0): used. P-N7's round 3 (`pn7`) and the first round's W1q/W1f of `pn7-r2` overlapped
foreign load; `pn7-clean` keeps only the rounds whose ticks showed no foreign process and load < 10 (W4 × 5, W1q ×
3, W1f × 5). P-N9's walls are not read; its peaks are.

| run | wall median [range] | peak median (`VmHWM`) |
|---|---|---|
| `a` (5 rounds) | 19.04 s [18.79-19.30], band 0.51 | 3,142,400 KiB |
| `d` (5) | 40.15 s [39.76-40.37], band 0.61 | 3,146,856 KiB |
| `f` (5) | 5.51 s [5.49-5.60], band 0.11 | 1,465,740 KiB (output 129,890,105 B) |
| `d` (P-N9, census) | 41.79 s | 3,169,600 KiB; members 3, non-primary 0, raw_bytes 12,615,284 |
| `h` (P-N9) | 117.62 s [112.62-123.36] | 3,222,396 KiB; members 88, non-primary 0, raw_bytes 237,676,240 |
| `p` (P-N9) | 31.83 s | 8,280,524 KiB; members 5,479, non-primary 0, raw_bytes 6,035,658,628 |
| four `d` at `RAYON_NUM_THREADS=5` (5 clean rounds) | 69.31 s [68.85-72.15], band 3.30 | Σ 11,525,956-11,568,040 KiB |
| one `d` at 5 (3) | 47.84 s [46.68-48.66], band 1.98 | 2,885,200 KiB |
| one `d` unset (5) | 41.62 s [40.47-42.19], band 1.72 | 3,150,772 KiB |

`a`'s trace (`ALSEM_TRACE`, `trace_summary.py` `agg`, inclusive): `crossapp.base_total` 11.662 s,
`l4_l5.run_detectors` 7.080 s, `gate.workspace_diagnostics` 0.000, `gate.project_filter_scope_baseline_suppress`
0.034, `gate.coverage` 0.047, `gate.format` 0.018, `gate.teardown` 0.169, `analyze.total` 19.051 s. Projection +
SARIF serialize = the four `gate.*` spans before teardown = 0.098 s. Trace files `pn5-r3/{a,d}.trace.json`.

**STOP reading:**

| bound | inequality with numbers | verdict |
|---|---|---|
| B1a | AD′ = 40.15 + 0.098 = 40.25; AD′ + bd = 40.86 < (a − ba) + (d − bd) = 58.07 | PASS |
| B1a void check (reported) | a + d − AD′ = 18.94 ≥ 2 × (ba + 2·bd) = 3.46 | met |
| B1b | ADF′ = 45.76; ADF′ + bd + bf = 46.48 < (a − ba) + (d − bd) + (f − bf) = 63.47 | PASS |
| B2u | subjects of `d`, `h` all primary (non-primary 0) → extra 0; U′ = 8,280,524 + (3,222,396 − 3,169,600) = 8,333,320 ≤ 9,576,561 | PASS |
| B2o | 3,222,396 + 126,953 (130 MB) = 3,349,349 ≤ 4,036,601 | PASS |
| B5 | four-`d` Σ max 11,568,040 ≤ 29,360,128 | PASS |
| **B6** | W4′ + b = 69.31 + 3.30 = 72.61 ≤ 1.25 × (47.84 − 1.98) = 57.33 | **STOP** |
| B7 | 72.61 ≤ 4 × (41.62 − 1.72) = 159.60 | PASS |

B6 misses on rounds whose 1-min load stayed < 22 (ticks 6-10 with the four runs' own threads, foreign 0), so F10's
contention re-run does not apply; it was missed in both P-N7 runs (`pn7` rounds 1-2: 68.85/68.93 s vs 47.84/46.68
s). Four concurrent `d` at 5 threads each take 1.45 × one `d` at 5 (CPU per worker 155-166 s vs 108-113 s alone).
**The claim STOPS at Task 1 on B6** (spec §7.4; plan Task 1 Step 7).

Noted: the walls differ from spec §7.3's (step 6 attempt-8 tree): `d` 40.2 s vs 23.77 s, `h` 117.6 s vs 41.39 s
(CPU 1,470-1,530 s). Not `--deterministic` (one `d --deterministic` run: 40.72 s). Not measured further.

## Inventories

Full tables (file:line + text at `2d6b850d`): `.agent/issue-277/inventories-task1.md` (made by a `sonnet` sub-agent,
spot-checked: bin count 49 with a comment at `alsem.rs:725`; library grep 99; `l4/analysis.rs:118` is the one L4
print site). Totals:
- (a) 11 commands read an `InputSet` (analyze, digest, prove, fingerprint, diff per workspace side, events fanout,
  events chains, policy check, query touches, query effects, graph); `policy explain` and `cache prune` read none.
  Union for `WorkspaceInputs`: `workspace, app_json, app_json_value, app_json_paths, roots_config, closure, al_walk,
  al_files, driver_version, model_instance_id`, plus `coverage_yaml` and `snapshot_inputs` when a digest, prove,
  fingerprint or diff job is present. Request-only: `baseline` (analyze), `policy_file`/`policy_request` (policy).
  F12: fingerprint `--no-roots-config` (effect: assembly skips `roots_config`, and `roots_config_loaded()` is read).
  **Spec D3 conflict:** a diff file side has no `InputSet`; `snapshot_inputs` is derived from workspace files only,
  so it belongs in `WorkspaceInputs`, not `RequestInputs`.
- (b1) 49 grep lines in `alsem.rs` = 48 calls + 1 comment: 35 validation (before `InputSet::read`, no stdout), 12
  error path (no stdout), 1 before stdout (`:1665`, the degraded warning).
- (b2) 99 grep matches − 3 comment lines = 96 (the spec's number), but 30 of those are in `#[cfg(test)]` modules:
  66 production sites = 2 job lines (`l4/analysis.rs:118` L4 non-convergence, replay iff `CORE_SUMMARIES`;
  `program/impact_graph/mod.rs:272` graph coverage warning), 57 process lines (census/trace, env-gated), 7 off the
  alsem path (LSP / aldump only). No library `eprintln!` is a build line (the build prints through `log::`). Rayon:
  `perf_trace.rs:492` (span opened in the detector `par_iter`, `registry.rs:607-625`) and `l5/d1_dataflow.rs:2778-2814`
  (5 lines, from `detect_d1` on a rayon worker): both process lines (F11 note).
- (b3) `log::` sites: `dependencies.rs` 14 (6 warn, 2 info, 6 debug), `snapshot/cache.rs` 3,
  `program/model/workspace.rs` 1, `engine/deps/app_package_zip.rs` 3 (`:142`, `:175`, `:253`). The spec's 12 = the
  warn+info sites of the first three files; with `debug!` it is 18. None inside a rayon closure.
- Hands-off files (`digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs`): 0 sites.

## Operator decision after Task 1's STOP (2026-10-24)

- Task 1 STOPped on B6 only (four concurrent digests 1.45 × one; bound 1.25 ×; clean rounds). The same reading found master's digest 1.7-2.8 × slower than step 6's bench; alsem-main bisected it to 444d0c8b (step 6 final-panel fix F1-S1, "proof cap counts distinct candidates"), filed #289.
- Operator (asked directly): "I just need something which works within reasonable time for now. We should file a investigation later, but at least now I can run it." Recorded as amendment 12A-18: B6 is reported, not gating; W stays as D12 derives it; Task 12 re-reads B6 (on #289's fix if landed); all other bounds stay gating. Work continues at Task 2.

## Task 2

Commit (branch, not pushed): `346f7fad` `#277 Task 2: WorkspaceInputs/RequestInputs; validation before the build;
the baseline write as an output` (one commit: the rename and the semantic moves share files, and hunk staging is not
available). Gates at `346f7fad`: `ci-steps task-affected` exit 0 (unmapped paths -> whole suite: 4064 passed,
8 skipped); `check-goldens` exit 0 (1116 passed, 4 skipped; no golden moved); pre-commit hook passed; `check-diff`
against `origin/master` gave only master's own `.agent/issue-264|265` paths (master moved to `f40087d5`), so it was
re-run against the merge-base `1134bed1`: `reasons: []`. No CDO run.

- **Split.** `InputSet` -> `WorkspaceInputs`, `InputNeeds` -> `WorkspaceNeeds { build_only, fingerprint_rows }` with
  `union` (build-only only if both are). New `RequestInputs { policy_file, policy_request, baseline }`,
  `RequestInputs::read(workspace, PolicyRequest, Option<PathBuf>)`, one per command; `AnalyzeArgs::read_request()`;
  `PolicyCheckOptions.request`. `snapshot_inputs` stays in `WorkspaceInputs` (correction below). The `inputs`
  census bucket no longer counts the policy and baseline bytes (they are not in the shared set any more).
- **Decision 7.** `run_policy_check` resolves the policy first (before `AnalysisTarget::build`); `run_fingerprint_pipeline`
  checks `--shard` without `--out` first (after `reject_inventory_only_combos`, before the assembly). `diff --renames`
  untouched.
- **Baseline.** `run_analyze_with_exit(inputs, request, args, version) -> Result<AnalyzeOutput, String>`,
  `AnalyzeOutput = (stdout, exit, warning, Option<Vec<u8>>)` (a named alias: clippy `type_complexity`). The bin writes
  the bytes with `write_file(path, bytes, exit 2)` first, before the degraded warning and stdout. `save_baseline` deleted
  (no caller). The fail-closed empty-output path returns `None` (no finding set; it wrote nothing before either).
- **T23 normalization choice:** normalize. The test compares the record's stderr and the head's stderr with the OS
  message replaced by `<OS message>` (the record's from its own text after the path; the head's checked first against the
  runtime OS error of the same write), so it runs on any platform; the record bytes stay byte-identical and are read
  only through `read_record`. The `update-baseline` record holds no version field, so only the OS text needed it; the
  `alsemVersion: "2.0.3"` normalization belongs to T20 (Task 5) and is not built here.
- **`record_dirs_refuse_regen`** runs on a tempdir copy with its own `RECORD`, and asserts both real record dirs hold
  `RECORD`: a broken guard cannot overwrite a committed record during the proof.
- **T9 fixture:** the failing build is `cli_build_counters::unreadable_ws` (now `pub(crate)`), the error diagnostic
  is `cli_diff_strict::error_ws`; the bad policy is `version: [not, a, number`.

Deviations and corrections:
- **Plan-level correction (D3, from Task 1):** a diff FILE side has no `InputSet`; `snapshot_inputs` comes only from
  workspace files, so it belongs in `WorkspaceInputs`, not `RequestInputs` (spec D3 lists it under `RequestInputs`).
- **Deviation (approved by the conductor):** `pub type InputSet = WorkspaceInputs;` stays in `src/program/inputs.rs`
  with a `shortcut:` comment: `src/engine/l5/digest.rs` (hands-off, step 9's lane) names `InputSet` at two test
  sites (:6222, :7992). Not edited; both compile and pass in the suite above. Remove the alias when digest.rs is free.
  Master (`f40087d5`) adds three more `InputSet` uses in tests; they compile through the alias on rebase.
- **Plan interface not built as written:** `WorkspaceInputs::read` stays infallible and takes `&WorkspaceNeeds` (the
  read never fails by design: every error is kept in its slot), so there is no `Result<_, InputError>`.
  The "validation phase returning `Result<(), (String, i32)>`" is the policy load and the shard/out check moved to the
  top of each pipeline: the policy validation's product is the loaded policy, so a `()` check would load it twice.
  Task 5 turns these early returns into `JobFailure`.
- **Spec note (D3 list):** moving fingerprint's shard/out check also puts it before fingerprint's `--strict` gate and its
  missing-model-id error (both exit 1; stderr text differs). Not listed among D3's three precedence changes; stated in
  CHANGELOG with the others.
- Rebase note: master's `f40087d5` also edits `tests/cli/cli_d35_unknown_commit.rs`, which this commit touches (one
  call site): expect a small conflict there.

### Task 2 review fixes

- **R2-1:** the CHANGELOG "Errors found before the build win" entry now states that fingerprint's `--shard`/`--out`
  check also runs before `fingerprint --strict`'s diagnostic gate and before `fingerprint: could not compute
  modelInstanceId` (exit 1 in every case; only the stderr text differs). Same here: with `--shard` and no `--out`, a
  strict-fatal diagnostic or a missing model id now gives `--shard requires --out <directory>`.
- **R2-2:** the duplicated 3-line comment in `fingerprint_cli.rs` (a leftover of the discrimination revert) is deleted.
- **R2-3:** in `run_analyze_with_exit` the comment rustfmt had glued onto the `baseline_bytes = Some(..)` line (also a
  revert leftover) is back above the assignment.
- **R2-4:** no `Err` return follows the baseline assignment. In `run_analyze_with_exit` from `let mut baseline_bytes`
  to the final `Ok(..)`, the only `?` is `load_baseline_bytes(..)?` in the non-`--update-baseline` branch, which never
  sets the bytes; there is no `return` and no other `?` after it. So every run that wrote the baseline before still
  returns its bytes, and no CHANGELOG line is needed.
- **T9 limit:** the test's module doc now says that cases 1 and 2 (`unreadable_ws`) skip and pass without asserting
  when a file cannot be made unreadable (root, mode-ignoring file systems); case 3 always runs.
- R2-5 and R2-6 are notes for later tasks (regen goes through `write_golden`; the `InputSet` alias goes at the
  capstone). No change now.

## Task 3 — WitnessRequest set form, TreeDemand, per-run scope

Commit: `55c67ee7` `#277 Task 3: WitnessRequest set form, TreeDemand, per-run scope` (on `0a8a7829`).
Files: `src/engine/session.rs`, `src/engine/l5/digest_cli.rs`, `src/engine/l5/detector_context_step5_tests.rs`,
`src/engine/gate/policy/policy_engine.rs` (doc only).

- `WitnessRequest { ids, primary }` (private fields): `roots`, `primary`, `empty`, `union`, `is_empty`, private
  `covers` (for the tree assertion) and `scope` (same signature and return type; always `Subjects`, and `prepare`
  passes `WitnessScope::None` when the plan keeps no raw facts, so a composition without a request is unchanged).
- `JobId(pub u32)`, `TreeDemand { subjects, tests, read }`, `TreeRead { Effects }` (step 9 adds `FingerprintQuery`,
  `Inventory`, spec §9).
- `Plan { runs: Vec<(Vec<Detector>, bool)>, need_l4, after_run, witness, trees: Vec<(JobId, TreeDemand)> }`.
  `needs_tree`, `tests`, `Plan.scope_primary` and the enum form are gone. `RunKey.scope_primary` and `finish_run`'s scope
  come from the run's own entry. `Plan::composes(det_filled)` = request || tree demand || (a run needs derived rows
  and det is not filled); `prepare`'s `need_caps` is `plan.composes(det.is_some())`, `need_raw` is request || tree.
- The assertion text is `"every tree demand's subjects are inside the view's request"`. A second assertion,
  `"a View composes at most one tree"`, marked `shortcut:` until Task 4 (D5.2 "one tree as today").
- `Plan::needs_tree()` is replaced by `Plan::has_tree(job: JobId)`; digest reads `has_tree(JobId(0))`.
- **Release mask:** `View.released` and `Cell::bit` are `u16` now (conductor's instruction; spec §9 says step 9
  widens it, so in case A step 9's rebase finds it done).
- Factory signatures unchanged; `digest_plan` gives its tree `JobId(0)`.

Gate (final tree): `bash scripts/ci-steps task-affected` exit 0 (fmt, clippy-dev, gen-syntax, whole suite: 4066 passed,
8 skipped; the first run failed clippy on a redundant closure in `Plan::keys`, fixed). `scripts/check-goldens` exit 0,
1116 passed, 4 skipped; no golden moved. `check-diff` `{"reasons": []}`.

| Proof | Break (Edit, applied count asserted) | Result | Reverted |
|---|---|---|---|
| T14 `witness_request_union` red | test before the types | compile error: no `roots`/`primary`/`empty`/`union` | n/a |
| T14 discrimination | `primary: self.primary && other.primary` in `union` (1 match) | FAILS `(roots, primary)`: left `{"d1"}`, right `{"d1","p1","p2"}` | green |
| D5.1a, plan's literal break | `composes` = `!self.witness.is_empty()` alone (1 match) | 232 of 3085 lib+cli fail (5 session lib tests, e.g. `a_second_prepare_never_refills_or_widens`, `run_diagnostics_follow_the_run_mask`; most CLI goldens): the break also drops the det term | n/a |
| D5.1a, tree term only | `composes` = request \|\| det term (no tree term) (1 match) | **whole suite GREEN** (3935 passed): no existing test or counter row sees it | — |
| D5.1a new test `a_tree_demand_composes_with_an_empty_request` | same tree-term break | FAILS: `prepare` panics `caps filled above` | green (session:: 18 passed) |

- **Plan correction (Task 3 Step 3):** the plan's fallback, "add the `digest --tests` no-root counter row to
  `cli_build_counters.rs`", cannot pin D5.1a. Standalone digest always has a derived run, so its det fill forces the one
  composition whatever the tree term says; the row would read 1 under the break. The tree term is visible only with
  no run that needs derived rows (step 9's `--inventory-only`, or a batch view whose det is already filled). So the pin is
  the lib test above, through `prepare`, with the precondition stated (empty request, no run). No counter row added.
- No hands-off file touched: `digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs` name none of `WitnessRequest`,
  `needs_tree` or `Plan`.
- Untested: the rewritten subset assertion has no test (no factory can break it; the old `Roots` assertion had none).
  (Task 4 adds it, R3-1.)

## Task 4 — several trees from one pass

Commit: `8b304a1c` `#277 Task 4: several trees from one witness pass (drain/borrow, lent arena and calls)` (on
`55c67ee7`). Files: `src/engine/l5/snapshot.rs`, `src/engine/session.rs`, `src/engine/l5/digest_cli.rs`
(`take_effects(JobId(0))`), `src/engine/l5/digest_rule_b_tests.rs` (a module of `digest_cli.rs`, not `digest.rs`; the
new `compose_tree` call and `effects(JobId(0))`).

- `SubjectRaw { map, subjects }` (`new(map, scope)` = scope ∩ map keys, O21; `get` is `None` outside `subjects`;
  `subjects()`), `TreeRaw { Drain(map), Borrow(SubjectRaw) }`. `compose_tree(model, typed, dependency_calls, caps: &CapabilityAnalysis, raw: TreeRaw, pd: Cow<ConeArena>, app_names)`:
  `caps` is shared now (`pd.take()` was its only mutation). All four raw readers go through the mode: the subject set
  (and so the pd arena's scope) is the drained map's keys or `SubjectRaw::subjects`; both evidence indexes take an
  accessor `impl Fn(&str) -> Option<&[CapabilityFact]>` (the cut path passes `base.raw.get`); `derive_capability_facts`
  takes `map.remove` (drain) or `acc.get` (borrow).
- `prepare` (5): per tree demand in job order. Later readers of an id = a later tree's scope, or the "rest" request
  (the request's ids no tree asks, plus its `primary` bit: policy's shape). Drain if no subject in the map has a later
  reader: the whole map when every key is in the scope (the one-tree path, byte- and peak-identical to before), else
  the scope's entries moved out (`remove_entry`). Else borrow, then remove the entries whose last reader it was. pd:
  `Borrowed` for every tree but the last, `Owned(take)` for the last. Dependency calls lent, set `None` right after
  the last tree's composition (before its read, where the old code dropped them). `Raw` released after the last tree
  unless the rest request is non-empty (then `raw` stays for (8)). Each tree is read and released before the next.
  `effects: BTreeMap<JobId, DigestEffects>`, `effects(job)`, `take_effects(job)`. The `shortcut:` "a View composes at
  most one tree" assertion is gone. R3-2's `debug_assert!(!demand.subjects.primary)` is in the `Effects` arm.

Gate (final tree): `bash scripts/ci-steps task-affected` exit 0 (4070 passed, 8 skipped). `scripts/check-goldens`
exit 0 (1116 passed, 4 skipped); no golden moved, so standalone digest is byte-identical. `check-diff --base 1134bed1`
`{"reasons": []}`. Pre-commit hook passed unmodified. No CDO run: B4 (standalone digest peak) and F15 are Task 12's.

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| T25 (a) `subject_raw_hides_other_subjects` | borrow arm: operation index read from `acc.map` directly | `snapshot.rs:3535` "nothing of Y's entry reaches the tree" | reverted, passed | `8b304a1c` |
| T25 (b), O21's Z case | `SubjectRaw::new`'s filter `\|\| true` (subjects = the scope alone) | `snapshot.rs:3539` "Z has no entry": left `{"X","Z"}`, right `{"X"}` | reverted, passed | `8b304a1c` |
| T25 (b), pd half | same break, and the subject-set assert commented out (test-side, Edit) | `snapshot.rs:3541` "(b) equals (a), pd included": `pd.states` holds Z's state vs `[]` | reverted both, passed | `8b304a1c` |
| `two_trees_one_pass` | `if last_tree \|\| true` (every tree takes `caps.pd`) | `session.rs:1357` "job 1" (FillAndPost's effects differ) | reverted, passed | `8b304a1c` |
| `two_trees_one_pass` (B5's shape) | `drain = true \|\| …` (drain even with a later reader) | `session.rs:1375` "the later reader still finds the trees' primary subjects" | reverted, passed | `8b304a1c` |
| R3-1 `a_tree_outside_the_request_is_refused` | `covers(&t.subjects) \|\| true` | "test did not panic as expected" | reverted, passed | `8b304a1c` |

- Preconditions asserted in the tests: T25 map holds X and Y, not Z; a drain tree over {X, Y} shows Y (so (a)'s
  "nothing of Y" is observable). `two_trees_one_pass`: FillAndPost's standalone effects carry the `XRec.Fill`
  known-temp insert (rule B fixture: a producer only a proof off the pd arena witnesses); FillAndPost's rows are
  primary and present in `raw` after `prepare`.
- R3-1 passing half: `a_primary_request_covers_an_empty_tree`.
- **Red:** the tests and the implementation were written in one pass and compiled together; the separate red run was
  not made. The breaks above are the evidence each test can fail.
- **Plan correction (Step 2):** "`build.witness_pass` 1" cannot be read from a lib test (the counter goes to the
  process tracer, off in `cargo test`). The single composition is structural (`prepare` has one
  `compose_capabilities` call, guarded by the one-witness and one-composition assertions); the counter is T1's
  (Task 9, a traced batch run).
- **Plan correction (rest readers):** the plan names policy as the later raw reader but `Plan` holds only the union.
  `prepare` takes the later readers as the request minus the trees' ids (plus `primary`). That is exact for every
  case-A requester (only policy asks raw without a tree, and asks `primary` alone). A future non-tree job asking ids a
  tree also asks would need `Plan` to carry its own request.
- Hands-off: no edit to `digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs`; `compose_tree` has no caller there.

### Task 4 review fixes (R4-1..R4-3)

Commit: `0a1252a9` (on `8b304a1c`). R4-1: `two_trees_one_pass` gains two no-policy cases, run before the policy
case so their breaks fail at their own assertion: (a) disjoint roots CallWrite + FillAndPost (the first tree drains
its own entries via `remove_entry`, the last drains the rest whole); (b) FillAndPost twice (borrow, then a last-tree
drain). Each: both jobs' effects equal standalone, `raw` is `None` and `Raw` released. The new cases pass on the
committed code (they pin it); the breaks are their red. R4-2: the `compose_tree` comment says "in drain mode". R4-3:
`shortcut:` note on `rest` (a tree asking `primary` keeps raw to the end; upgrade when `Plan` carries the non-tree
jobs' own request).

Gate: `ci-steps task-affected` exit 0 (4070 passed, 8 skipped); `check-diff --base 1134bed1` `{"reasons": []}`.

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| `two_trees_one_pass` (a) | the extracting drain takes the whole map (`let own = std::mem::take(&mut map);`) | "(a) disjoint: job 1" (FillAndPost's effects `[]`) | reverted, passed | `0a1252a9` |
| `two_trees_one_pass` (b) | borrow tree's `done` filter `!later(id) \|\| true` (removes every subject) | "(b) same root: job 1" (effects `[]`) | reverted, passed | `0a1252a9` |

## Task 5 — the driver, `JobOutput`, analyze/digest/policy/events

Commits: `40a6dc4e` T20 test alone (green on the Task 4 head: a pin); `7368efaf` the driver and the four commands.
Gates at `7368efaf`: `ci-steps task-affected` exit 0 (whole suite: 4072 passed, 8 skipped); `check-goldens` exit 0
(1117 passed, 4 skipped), no golden file moved (`git status` clean under `tests/`); pre-commit hook passed both
commits; `check-diff --base 1134bed1` `{"reasons": []}`. Standalone traces checked by hand: `analyze`, `events fanout`,
`digest` (exit 2) carry one `batch.driver` `{"jobs":1,"views":1,"workers":1}`; `digest --format bad` `views: 0`.

- `src/engine/batch/{mod,job,output}.rs`. `Job { demands, resolve, compute, view_failed, finalize (default: one side),
  serialize, write_failure_exit (default 1) }`; `Failed(JobOutput)` is the zero-view job of a failed validation.
  The driver: zero-view jobs first, then per view in first-demand order: build, `resolve` per demand (a `JobFailure`
  contributes nothing), `Plan::merge`, one `prepare`, `compute` per demand in job order, view dropped, then `finalize`
  for every job whose last demand it held. `run_standalone(&dyn Job) -> u8` emits `batch.driver` and replays.
  `run_doc` is the library wrappers' entry (same driver, no replay).
- `ViewCx`: `release`/`release_all_but` keep the cells a LATER demand on the view declared in `PlanPart.reads`;
  `take_run` keeps a run a later demand's keys name. One job: every release lands where it did (the pinned
  `views_release_their_cells_before_the_next_build` sequences are unchanged).
- Library wrappers: `run_analyze_with_exit`, `digest_pipeline_with`, `run_policy_check`, `events_{fanout,chains}_with`
  build their job and call `run_doc`. The bin builds the job and calls `run_standalone`; every handler validation
  error in the four commands is a `Failed` job; the policy load error too (`PolicyJob::new` → `Err(outcome)` →
  `Failed(outcome.into_output())`). The bin's remaining writers (`write_out`, `write_stdout`, `write_stderr_lines`,
  fingerprint shards as `Dir` + `File`s) go through `replay`, taking owned bytes (no copy of the output).
- `AnalysisTarget::build_target` → `Result<_, TargetFailure>`; `build` = `build_target` + `TargetFailure::message(cmd)`.

Plan corrections / deviations (stated):
- **`JobOutput.exit` and `JobFailure.exit` are `u8`, not `i32`**: every exit code in the crate is `u8` and
  `ExitCode::from` takes `u8`. `replay(out, write_failure_exit: u8) -> u8`.
- **`PlanPart` is `{ plan: Plan, reads: Vec<Cell>, carry: Option<Box<dyn Any + Send>> }`**, not the plan's flat
  fields: a factory's one-job `Plan` already holds runs/need_l4/after_run/witness/tree; `Plan::merge` (session.rs,
  named `merge` because `Plan::union` is the private run-mask fold) unions them and files each tree under the job's
  id. `carry` hands digest's `ChangedRootsResult` from `resolve` to `compute` (the identity table must not live past
  resolve, D4.2 (3)).
- **`resolve(&self, side, &ResolveCx)`** instead of `(side, &Model, &IdentityTable)`: `ResolveCx` derives the identity
  table on first ask, so a view where no demand resolves roots builds none (D4.2 (3)'s "only when needed").
- **`compute(&self, side, &mut ViewCx)`** instead of `&View`: today's pipelines release cells mid-compute (analyze
  after the run, digest before the query, policy before the rule engine); with `&View` those releases would move after
  the compute and standalone peaks would rise (B4). `ViewCx` lets each job release at today's point, the driver
  deciding whether a later reader keeps the cell (D6.1).
- **`view_failed`** is a trait method (the build failure, mapped by each job to its own text: D4.1).
- **Two builders until Task 6** (`shortcut:` in `batch/mod.rs`): `analyze_extras` → `build_analysis_model`, else
  `AnalysisTarget::build_target`; a view's demands must agree (assert). `ViewNeeds.graph` and `.workspace` exist
  but are unread.
- **Inputs are still read per command** (`ViewDemand.inputs: Arc<WorkspaceInputs>`, `shortcut:` comment): plan Task 6
  Step 3 deletes the per-command reads; the driver then reads one per workspace path.
- **Fingerprint's `--shard`/`--out` early return is NOT a `JobFailure` yet**: fingerprint is a Task 6 command. The
  policy load error is (a zero-view `Failed` job).
- **Guard proof without `REGEN_TEMP_GOLDENS`**: the session's golden-regen hook blocks any command line naming
  `REGEN_TEMP_GOLDENS` without a triage receipt in `.agent/` (protected). `write_golden` does not read the variable, so
  the break called it unconditionally; same guard, same panic.
- Stated behaviour change (CHANGELOG "Changed"): validation lines go through `write_stderr` (a closed stderr gives the
  command's write-failure exit, not an `eprintln!` panic, exit 101); `analyze.total` no longer spans the build and the
  run, and `l4_l5.run_detectors` is gone (the driver's `prepare` has no analyze span). Task 12's bench reads spans:
  re-check any reading that used them.

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| T20 `write_order_under_merged_streams` | `replay` writes the actions in a fixed order (stable sort: Stdout, Stderr, File/Dir) | `cli_batch_merged_streams.rs:74` "degraded-analyze: merged bytes": the warning line after the report instead of before | reverted, passed (gate at `7368efaf`) | `40a6dc4e`, `7368efaf` |
| T20 guard (`write_golden` on a record) | T20 calls `batch_records::write_golden` on each record path before its compare | `batch_records.rs:30` "refusing to rewrite a base-binary record: …/merged-streams/degraded-analyze.merged.golden"; `git status tests/gate-goldens` clean | reverted, passed | `40a6dc4e` |
| `plan_union_of_parts` (lib, new) | `Plan::merge`'s run dedup `if true \|\| !out.runs…` | `session.rs:1582` "one run per key": left 5, right 3 | reverted, passed | `7368efaf` |
| `run_analyze_with_exit_uses_the_shared_aggregation` (changed: scans `analyze_view`) | in `analyze_view`: `let aggregate = analyzer_diagnostics_of_run; aggregate(resolved, &run)` (a first try with a space before `(` was undone by the formatter hook: `grep -c` 0, not counted) | `run.rs:1414` "analyze_view must call analyzer_diagnostics_of_run" | reverted, passed | `7368efaf` |
| `analyze_builds_through_the_dumped_functions` (changed: scans `build_view` and `analyze_view`) | analyze's demand `analyze_extras: false` | `switch_dump.rs:384` "analyze's view is built by build_analysis_model" | reverted, passed | `7368efaf` |
| `views_release_their_cells_before_the_next_build` (existing; pins the driver's release path) | `ViewCx::release_all_but` releases nothing (`let _ = keep;`) | `cli_build_counters.rs:1070`: policy `["ctx","rel:Caps"]`, events, digest sequences short of their `rel:*` | reverted, passed | `7368efaf` |

Not covered here (later tasks): multi-job `ViewCx` later-reader paths (T3/T7, Task 7/9); `batch.driver` per command
(T13, Task 7); write-failure lines for jobs in a batch (T5, Task 8).

### Task 5 review fixes (R5-1..R5-5)

Commit `5ad109e1` (on `7368efaf`). Gates: `ci-steps task-affected` exit 0 (4075 passed, 8 skipped); `check-goldens`
exit 0 (1119 passed, 4 skipped), no golden moved; `check-diff --base 1134bed1` `{"reasons": []}`. R5-6: no change
(Task 6).

- R5-1: `Failed { output, write_failure_exit }`; the bin's `failed(line, exit, write_failure_exit)`; every analyze
  validation site passes `exit::ANALYSIS_FAILURE`, every other command 1. Test level chosen: **spawned cli**
  (stderr is a `std::io::pipe()` whose reader is dropped before the spawn, so every stderr write fails
  deterministically; no race).
- R5-2: `analyze.total` opened in `run_analyze_cmd` around `standalone(&job)` and in `run_analyze_with_exit` around
  `run_doc`; the inner spans in `analyze_view`/`view_failed` are gone. Limit: in the bin the span now also covers the
  replay (the stdout write), which before ended when `run_analyze_with_exit` returned.
- R5-3: `run_view` asserts `Arc::ptr_eq` of every demand's inputs with the first's.

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| R5-1 `validation_error_on_a_closed_stderr_exits_with_the_write_failure_code` (new, cli_write_failure) | red before the fix (TDD); after it, `failed()` builds `write_failure_exit: 1 + 0 * write_failure_exit` | `cli_write_failure.rs:149` "analyze": left `Some(1)`, right `Some(2)` (both runs) | reverted, passed | `5ad109e1` |
| R5-2 `analyze_total_spans_the_build` (new, cli_build_counters) | bin: `drop(span("analyze","analyze.total"))` (the span closes at once) | `cli_build_counters.rs:78` "build.program at [1318.0] outside analyze.total [1199, 1204]" | reverted, passed | `5ad109e1` |
| R5-3 `a_view_refuses_two_reads_of_its_workspace` (new, lib `batch::tests`) | the assert's predicate `true \|\| Arc::ptr_eq(..)` | `batch/mod.rs:265`: "built past the driver's checks" instead of "a view's demands share one WorkspaceInputs" | reverted, passed | `5ad109e1` |
| R5-4 `analyze_builds_through_the_dumped_functions` (changed: also scans `impl Job for AnalyzeJob`) | `AnalyzeJob::compute` gains `if self.version.is_empty() { drop(build_analysis_model(self.inputs, false)); }` | `switch_dump.rs:385` "one model build on the analyze path (build_view's)": left 2, right 1 | reverted, passed | `5ad109e1` |
| R5-5 `plan_union_of_parts`: `need_l4` | `out.need_l4 = part.need_l4;` | `session.rs:1583` `assertion failed: u.need_l4` | reverted, passed | `5ad109e1` |
| R5-5: `after_run` | `out.after_run = part.after_run;` | `session.rs:1584` left 1, right 5 | reverted, passed | `5ad109e1` |
| R5-5: witness union | `out.witness = part.witness.clone();` | `session.rs:1588` left `{ ids: {}, primary: true }`, right `{ ids: {"a","b"}, primary: true }` | reverted, passed | `5ad109e1` |
| R5-5: tree under the job id | `.map(\|(j, tree)\| (j, tree))` (keeps the factory's `JobId(0)`) | `session.rs:1568` `one.has_tree(JobId(3)) && !one.has_tree(JobId(0))` | reverted, passed | `5ad109e1` |

## Task 6 — query, graph, prove, fingerprint, diff through the driver

Commit `bc02c36e` (one commit: the one builder, the five jobs and the counter rows share files; no hunk staging).
Gates at `bc02c36e`: `ci-steps task-affected` exit 0 (whole suite: 4075 passed, 8 skipped); `check-goldens` exit 0
(1120 passed, 4 skipped), no golden file moved (`git status tests/` shows only the three edited test files);
pre-commit hook passed; `check-diff --base 1134bed1` `{"reasons": []}`. No CDO run.

- **One builder** `gate::run::build_view_model(inputs, &ViewKey, ViewNeeds, on_program) -> Result<Option<ViewModel>,
  ViewFailure>`. `build_analysis_model` (the dump's entry) and `AnalysisTarget::build` are wrappers; `build_target`,
  `build_cross_app_analysis_model`, `failed_model` and the driver's `build_view` are gone, and with them the
  "one builder per view" assert. Single-app: `build_single_app` (analyze's body; the extras only when asked; `skip_roots_config`
  = `key.no_roots_config`). A missing id: non-analyze views fail before any build (as `AnalysisTarget` did), analyze
  builds the single-app program first (as before). `ViewFailure { target: TargetFailure, analyze: Option<Box<(fresh,
  ModelFailure)>> }`: each job words its own failure; the analyze half is boxed (`clippy::result_large_err`).
- **`ViewNeeds { analyze_extras, graph, model }`** with `MODEL`/`ANALYZE`/`GRAPH` and `union`; no `Default` (every job
  states it). `ViewNeeds.workspace` deleted (nothing read it).
- **graph (D9):** `GraphJob` (in `impact_graph/mod.rs`, replacing `export_workspace`); `Job::program(side, Result<&ResolvedProgram,
  &str>)`. The builder builds the cross-app program, lends it, and then (if a model demand exists) builds the base from
  it: `build_cross_app_base_from` → `cross_app_program_from` → `cross_app_program_census(program, …)` →
  `cross_app_assemble_from`. The originals keep their signatures. A graph-only view builds the program alone (counter row
  unchanged); the program build failure goes to graph through the same callback.
- **query:** `QuerySubstrate<'a> { view: &View, dependencies }` (`of(&view)`), `QueryJob` (`touches`/`effects`, `.written(json,
  out)`), `query_view` test entry; `into_query_parts` and `cross_app_query_drops_the_base` deleted (the property it pinned,
  "the base drops before the query", is reversed by D9; a borrow cannot copy, so no replacement test).
- **prove:** `ProveJob` (single-app view; `digest_plan(.., ∅, false)`, reads `Det`, `L4`). `finish_default_run` is the one
  `ViewCx` form (`finish_default_run_in` and the `&mut View` form are gone: the R5 shortcut closed).
- **fingerprint:** `FingerprintJob::new(opts) -> Result<_, JobFailure>` (shard without out); the bin turns it into a
  `Failed` job; the library wrapper keeps its early `FingerprintRunResult`. `fingerprint_query.rs` untouched.
- **diff:** `DiffJob::new(opts) -> Result<_, DiffRunOutcome>` (missing input, `.app`, old snapshot file: before any build);
  one demand per workspace side, one read when both sides have one view key; new snapshot file loaded in `finalize`;
  `finalize` keeps the order (old error, new error, strict gate, renames, engine, render). Driver: a job whose earlier
  side failed is skipped on later views (its failure stands for the side) — standalone `diff bad good` builds nothing,
  as before. Driver ends the `post_attach` phase when it drops each view. Driver sides are indexed by demand position
  (a file/ws diff has one demand with side index 1: caught by the existing `t6_snapshot_old_error_ws_new_exits_1`).
- Bin: the five handlers' validation lines go through `failed()`; `write_out` deleted.

**Traced prototype (Step 1):** the new binary's traces of `diff ws ws` / `--strict` on `ws_dep()` (read by
`cli_build_counters::diff_ws_ws{,_strict}`, every cell asserted): 1,2,2,0,2,2,0,2,2,1 and 1,3,3,1,4,3,1,2,2,1 — equal to
spec D8's rows. The pre-rewire binary (the Task 5 debug build, saved before this task's first build) gave 2,…,2 and
2,4,4,2,6,4,2,2,2,2 (red run below).

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| `diff_ws_ws`, `diff_ws_ws_strict` (rows rewritten), `inputs_read_once`, `views_release…` (diff case), `diff_ends_each_sides_phase` (rewritten) | red: the new test binary run against the pre-rewire `alsem` | all five fail: `build.program: got 2, want 1`, `inputs.read: got 2, want 1`; strict also bcg 4/3, cone 4/3, v2 2/1, scc 6/4, pd 4/3, ctx 2/1; phase `[s,e,s,e]` vs `[s,e]`; release sequence twice | new binary: 49 passed | `bc02c36e` |
| same rows (plan Step 4) | each diff side keyed on its own view: `ViewKey { no_roots_config: i == 1, ..view_key(..) }` | `diff_ws_ws`, `diff_ws_ws_strict` (`build.program: got 2, want 1` …), `views_release…`, `diff_ends_each_sides_phase` fail; `inputs_read_once` passes (the read is still shared) | reverted, passed | `bc02c36e` |
| `diff_failed_old_side_builds_no_new_side` (new) | driver skip rule off: `failed.cloned().filter(\|_\| false)` | `cli_build_counters.rs` `build.program` left 1, right 0 | reverted, passed. Pre-rewire binary: the three main asserts pass (standalone pin), only its first precondition (then `diff new new` = 1 build, a D8 row) failed; rewritten to the swapped-sides precondition (1 build in both binaries) | `bc02c36e` |
| `diff_ends_each_sides_phase` | driver's `pt::end_phase("post_attach")` commented out | left `["phase.start","phase.start"]`, right `[s,e,s,e]` | reverted, passed | `bc02c36e` |
| `Row.inputs` column (every row) | bin `run_query_touches_cmd` reads the workspace a second time | `[query single-app] inputs.read: got 2, want 1`, `[query touches cross-app] …`, `inputs_read_once` left 2 right 1; `build.program` cells pass | reverted, passed | `bc02c36e` |
| `inputs_read_once` (diff now 1) | `DiffJob::new` reads the new side again (`if false && view_key(..) == ..`) | fails: the driver's R5-3 assert panics (`batch/mod.rs:111`), so `run` sees no program build | reverted, passed | `bc02c36e` |
| `analyze_builds_through_the_dumped_functions` (rewritten: scans `run_view`, `build_analysis_model`, `ViewNeeds::ANALYZE`) | `AnalyzeJob::compute` gains a second `build_view_model(…)` call | `switch_dump.rs:388` "one model build on the analyze path (run_view's): build_view_model(" left 2, right 1 | reverted, passed | `bc02c36e` |
| `analysis_model_uses_the_program_parse` (now scans `build_single_app`) | the call spelled `(assemble_and_resolve_workspace_from_program)(` | `switch_dump.rs:487` left 0, right 1 | reverted, passed | `bc02c36e` |
| `workspace_side_strict_lines_carry_the_dependency_warning` (harness rewritten: a `Sides` wrapper job over `DiffJob` through `run_doc`) | `DiffJob::compute` drops `warning: dependency` lines | `diff/cli.rs:562` "missing \"warning: dependency …\"; got []" (first run of this break shared a source state with the R5-3 break above and panicked there; re-run alone) | reverted, passed | `bc02c36e` |

Task 5 shortcuts:
- **Two builders → one: closed** (above).
- **`ViewNeeds.graph` unread → closed** (the builder reads it); **`ViewNeeds.workspace` → deleted**.
- **Fingerprint `--shard`/`--out` early return → closed** (`JobFailure`, zero-view `Failed`).
- **Duplicated take-then-release → closed** (`finish_default_run` is one `ViewCx` function).
- **Each command reads its own inputs: stays** (`shortcut:` on `ViewDemand.inputs`). Standalone has one read either way;
  `diff` now reads once per view key itself. The cross-job "one read per workspace path with the union of needs" needs
  the manifest (the reader must know every job's needs before the first read) — Task 7. Doing it now would move every
  command's `WorkspaceInputs` out of its options (≈ 40 test call sites pass pre-read inputs to the library wrappers).
- Case A stays (D6.4): fingerprint, prove and each diff side compose their own cut snapshot in compute.

Plan/spec corrections:
- **Plan interface:** `build_view_model` returns `Result<Option<ViewModel>, ViewFailure>`, not `Result<AnalysisModel,
  ModelFailure>`: `ModelFailure` cannot carry the other commands' texts (`CrossAppFailure`'s kinds), and `AnalysisModel.model`
  stays as it is (the switch dump prints its `Err` Debug). `Ok(None)` is the graph-only view. `on_program` takes
  `Result<&ResolvedProgram, &str>` so a failed program build reaches graph too.
- **Plan Step 3 "delete the per-command `InputSet::read` calls": not done** (above; Task 7).
- **Stated counter change beyond D8:** a `diff` whose OLD workspace side fails now also reads the new side
  (`inputs.read` 2, was 1): both sides are read when the job is made, before the first build. No new-side build, same
  bytes. In CHANGELOG.
- **Trace changes (CHANGELOG):** the post-attach phase ends at each view drop (every command); single-app view builds open
  `preflight.fresh_program`/`l3.assemble_resolve`, cross-app model builds `crossapp.base_total`, for every command (not
  only analyze); a graph-only build opens none.
- **Memory (risk, not measured):** cross-app `query` keeps the whole base (leaf summaries, field index, …) through its
  render, as every other command does; it dropped them before (#160 Task 27 review I1). D9's choice; Task 12's bench
  reads query's peak.
- D8's "a failed old side discards the new side's doc and lines" is met by NOT building the new side's view (standalone
  identical: no view lines, no counters), not by discarding it.

Untested here (later tasks): a graph job on a view that also builds a model (the lend before `build_cross_app_base_from`;
T12, Task 8); the shortcut double build for graph + analyze on a workspace with no id.

### Task 6 review fixes (R6-1..R6-6)

Commit `03fe529b` (on `bc02c36e`). Gates: `ci-steps task-affected` exit 0 (4076 passed, 8 skipped); `check-goldens` exit 0
(1120 passed, 4 skipped), no golden moved; pre-commit hook passed; `check-diff --base 1134bed1` `{"reasons": []}`.

- **R6-1 (I1 restored):** new `Cell::Base` (NOT in `Cell::ALL`, so step 9's agreement holds; released only by name).
  `View::release(Cell::Base)` → `R3a5CrossAppBase::release_past_prepare` (drops the relation handle, `dependency_calls`,
  `friends`, `dep_routine_ids`, `field_index`, `leaf_summaries`, `fixed_roles`, `direct_full`, `direct_coverage`,
  `declared_dependencies`, `dependency_apps`, `resolved_app_versions`; keeps `resolved`, `loaded_apps`, `coverage`).
  Audit: after `prepare` no job reads a base field except the model and `dependencies()` (`loaded_apps`);
  `cross_app_inputs` and `loaded_app_names` are prepare-only, and `prepare` now asserts the base was not released. The
  base's `rel` became an `Option` behind `rel()` (16 call sites). Query's compute (`released_substrate`) clones the
  graph `Arc`, then `release_all_but(&[Cell::L4])` (the view's relation) and `release(Cell::Base)` through `ViewCx`, so
  a later job keeps what it declared. The render needs neither (query goldens unchanged). `QuerySubstrate` is now
  `&Model` + its own `Arc<PrimaryGraph>` + `&L4Analysis`.
- **R6-2:** plan Task 7 Step 2b added: one `WorkspaceInputs` per path, with the union of needs, before the first view.
  Its test is `batch_reads_each_workspace_once` (two jobs on one view, and two jobs on two views; no R5-3 panic). Its
  proof: read per job. The `shortcut:` comment on `ViewDemand.inputs` points at it.
- **R6-3:** `diff_failed_old_side_builds_no_new_side` asserts `inputs.read == 2`.
- **R6-5:** `crossapp.base_total` is closed around the `on_program` callback and reopened for the base build. A mixed
  graph+model view shows two spans; standalone graph opens none, and every other standalone command opens one. No test:
  only a batch has a mixed view.
- **R6-4:** no change; the multi-job skip-rule test is T21 (Task 7, as the coordinator directs). **R6-6:** no change
  (`scripts/` is protected).

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| `cross_app_query_drops_the_base` (restored; runs through the driver: an `Owners` wrapper job over `QueryJob` reads `released_substrate(cx).shared_owners()`) | `cx.release(Cell::Base)` commented out | `capability_cone.rs:5441` "(graph, scc) owners" left `(2, 2)`, right `(1, 1)` | reverted, passed | `03fe529b` |
| same | `cx.release_all_but(&[Cell::L4])` commented out | same assertion, left `(2, 2)` | reverted, passed | `03fe529b` |
| `diff_failed_old_side_builds_no_new_side` (R6-3 assert) | `DiffJob::new` reuses the old side's read for the new side (`if true \|\| view_key(..) == ..`) | `cli_build_counters.rs:632` left 1, right 2 | reverted, passed | `03fe529b` |

Red for R6-1: the restored test and the release were written together; the two breaks above are its red runs.

## Task 7 — `alsem batch`: manifest, union, view order, traces

Commit `2e1319f9` (one commit; why one: end of "12 early"); pre-commit hook passed; `check-diff --base 1134bed1`
`{"reasons": []}`. Every "Task 7" in the proof table means `2e1319f9`. Gates before commit: `ci-steps task-affected`
exit 0 (whole suite: 4098 passed, 8 skipped); `check-goldens` exit 0 (1136 passed, 4 skipped), no existing golden moved
(`git status --porcelain tests/` shows only this task's test files and the new `one-job/` dir). No CDO run, no release build.

- **Bin:** each handler is now `plan_<cmd>(cli) -> Planned` (`Failed(Failed)` or `Ready { reads, make }`); `make` builds the
  job off a shared `WorkspaceReader` and hands it to a continuation, so jobs (which borrow their options) can all be alive
  for one driver run (`with_jobs` nests the continuations). Standalone = `run_planned` (a one-job reader; reads where they
  always did, since the reader reads on first ask). `alsem batch`: `manifest::load` (shape, stream paths), then every argv
  parsed with the bin's own clap `Cli` (`parse_job`: clap's `error:` line, refused `batch`/`policy explain`/`cache prune`,
  `no command`), then each batch in-process in manifest order (`run_one_batch`), then the job lines on stdout (Decision 49
  write rule, exit 1). Exit 3 lines go out through `replay` (no driver instant).
- **Step 2b (one read per workspace path): closed.** `WorkspaceReader::new(reads)` unions the needs per absolute path before
  the first read; `read` returns the one `Arc` (asserts a later ask's needs are held). The `shortcut:` comment on
  `ViewDemand.inputs` is deleted. The reader holds `Weak`s only (E12-C2).
- **Driver:** `drive(jobs, done)` hands each job's `Doc` to `done` at finalize; `run_batch` serializes it (inside the job
  span) and writes every finished job whose earlier jobs are written (D6 (11)) through `replay_on` onto the job's two files
  (`output.rs`: `replay` = `replay_on` onto the real streams). Traces: `view.<v>` span around each view, `view.drop` instant,
  `view.teardown` span, `job.<j>.<cmd>` span around resolve/compute(/program/view_failed)/finalize/serialize with a
  `job.step` instant, `view.run` instant per `finish_run` (T15). `Job::name()` (required) and `Job::standalone_span()`
  (analyze's `analyze.total` moved from the bin into `run_standalone`, same extent minus the workspace read, as before).
- `exit_code.rs`: the `alsem batch` table. CHANGELOG: Added (batch), Changed (E12 items).

Plan/spec corrections:
- **`manifest::load` does not parse argv**: the clap `Cli` lives in the binary, so the bin parses (`parse_job`); `load` owns
  the JSON shape and stream paths, and resolves default streams to absolute `DIR/<b>.<i>.stdout`.
- **No hidden `--worker` flag yet**: nothing reads it until Task 10 (dead code is deleted, §4.13).
- **`JobLine.exit` is `Option<u8>`**, stream paths absolute (the one-job golden relativizes them).
- **No-subcommand argv**: clap prints the help text (no `error:` line) for an empty argv, so the reason is `no command`.
- **T19 break (a)** as the spec words it ("`composes` from `!witness.is_empty()`") stays GREEN: digest's run needs derived
  rows, so the view composes anyway. The proof row below breaks `keeps_raw` (raw kept only for a non-empty request), which
  is what drops the tree; it fails.
- **T21** is pinned twice: the hand-stated driver test (brief) and the spec's cli shape `batch_diff_sides`
  (`[diff A B, diff B A]`, `[diff A B, analyze A]` folded into one batch, `diff A A`, an old side that fails).
- **T6 rule's limit**: cut readers (fingerprint, prove, diff) compose inside compute (D6.4), so their fills lie in a job
  span; T6's batch holds none of them (stated in the test's doc).
- **T3 "+ ordering"**: measured row is the digest row plus the ordering cut (1,2,2,1,3,2,1,2,2,1).
- **D8 skip rule** now reads "a side after a failed EARLIER side (by side order)": D8 itself says job 2's new side of
  `[diff A B, diff B A]` computes on A before its old side on B, so the brief's "a side must never run before its own old
  side" is refuted by D8's text ("Job 1's old side and job 2's new side compute on A"); `finalize` still discards the new
  side when the old one failed (the existing `DiffJob::finalize` returns on the old error first).

Golden regen (conductor-approved receipt at `<worktree>/.agent/golden-triage.md`, gitignored, never staged, deleted after):
`scripts/check-goldens --regen` exit 0; `git status --porcelain tests/` showed only the 18 seeds filled (+ this task's
test files): `tests/gate-goldens/batch/one-job/{analyze,digest,prove,fingerprint,diff,events,policy,query,graph}.{line.golden.json,stdout.golden}`.
Each `.stdout.golden` was compared by hand (`cmp`) with that command's standalone stdout from the repository root: all 9
equal (the test also asserts it); each `.line.golden.json` is `{batch 0, job 0, exit 0, files [], stdout 0.0.stdout,
stderr 0.0.stderr}`. Then `check-goldens` (no regen) exit 0.

| test | break (Edit, `grep -c` = 1 before the run) | fail | pass | commit |
|---|---|---|---|---|
| `a_failed_side_stands_for_the_later_side` (lib, T21 driver half) | skip rule off: `failed.cloned().filter(\|_\| false)` | `batch/mod.rs` "side SideIx(1) computed after its old side failed" | reverted, passed | Task 7 |
| `a_job_finalizes_after_its_last_view_in_visit_order` (lib, E12-C1) | `last_view` from the LAST demand (`.last()` for `.max()`) | panic "every side ran" (`batch/mod.rs:167`) | reverted, passed | Task 7 |
| `batch_diff_sides` (cli, T21 / E12-C1) | same `.last()` break | run exit 101, stderr "every side ran" | reverted, passed | Task 7 |
| `a_view_drops_its_read_before_the_next_view` (lib, E12-C2) | `*d = None;` commented out | "A's read outlived its view" | reverted, passed | Task 7 |
| `diff_job_hands_its_reads_to_the_driver` (lib, E12-C2) | `DiffJob::demands` clones instead of `take()` | "the job kept a workspace read" | reverted, passed | Task 7 |
| `a_later_reader_keeps_its_cells_and_runs` (lib, D6.1) | `ViewCx::release`: `if true \|\| !self.later_cells…` | `session.rs:893` "l4 cell not filled" | reverted, passed | Task 7 |
| same (D6.2) | `ViewCx::take_run`: `if false && self.later_runs…` | `session.rs:938` "run RunKey {…} not prepared or already taken" | reverted, passed | Task 7 |
| `batch_counters_one_per_view` (T3 break 1) | one `run_view` per demand (`flat_map(\|d\| run_view(jobs, from_ref(d)))`) | `[five jobs, one view] build.program: got 5, want 1`, bcg 5, cone 4 … (no panic) | reverted, passed | Task 7 |
| `batch_counters_cut_readers`, `batch_counters_ordering_cut` (T3) | same break | `build.program: got 2, want 1` each | reverted, passed | Task 7 |
| `graph_and_a_model_job_share_one_program`, `analyze_and_digest_share_one_run` | same break | program 2 / two `view.run` | reverted, passed | Task 7 |
| `batch_counters_one_per_view` (T3 break 2, the guard) | one `prepare` per part: `for p in plans { view.prepare(&Plan::merge([p])) }` | run exit 101: `session.rs:522` "a View takes at most one witness request, in its one composing prepare" | reverted, passed | Task 7 |
| `batch_counters_two_views_one_read` (T3 two views) | reader cache off: `read.get(&key).filter(\|_\| false)` | `inputs.read: got 2, want 1` | reverted, passed | Task 7 |
| `batch_reads_each_workspace_once` (Step 2b) | same reader break | run exit 101: R5-3 "a view's demands share one WorkspaceInputs"; the two-view manifest by hand under the break: `inputs.read` 2 | reverted, passed | Task 7 |
| `batch_views_are_sequential` (T4) | the view span kept open into the next view (`prev = Some(view_span)`) | "view 1 opens after view 0 drops: [view.0 1585–39746, view.1 21326–39898]" | reverted, passed | Task 7 |
| `batch_trace_split_per_job` (T6) | job 0's span opened around `prepare` | "fill at 13023 inside [job.0.analyze]" | reverted, passed | Task 7 |
| `batch_manifest_errors` (T11) | subcommand check off: `refused(..).filter(\|_\| false)` | `["batch","m.json"]`: exit 101 (unreachable "no job"), want 3 | reverted, passed | Task 7 |
| same (empty batch) | `if false && batch.jobs.is_empty()` | `{"jobs":[]}`: exit 0, want 3 | reverted, passed | Task 7 |
| `standalone_is_a_batch_of_one` (T13) | `batch.driver` instant skipped for analyze (the observable of a bypass) | "analyze: one batch.driver" left 0 right 1 | reverted, passed | Task 7 |
| same (one-job family) | job line `exit: Some(exit + 1)` | "analyze: line" golden exit 0 vs 1 | reverted, passed | Task 7 |
| `analyze_and_digest_share_one_run` (T15) | run dedup off in `Plan::merge` (`if true \|\| …`) AND `prepare` (`if false && runs.contains_key`) | two `view.run` for one key: left 2 right 1 | reverted, passed | Task 7 |
| `empty_request_composes_only_with_a_tree` (T19 a) | `keeps_raw` = `!self.witness.is_empty()` (spec's `composes` break stays green, see corrections) | run exit 101 `session.rs:655` "raw filled above" | reverted, passed | Task 7 |
| same (T19 b) | `composes` = `true \|\| …` | query: `build.cone` left 1 right 0 | reverted, passed | Task 7 |
| `batch_view_key_spellings` (Review Focus 2) | key canonicalized (`std::fs::canonicalize`) | view spans left 1 right 2 | reverted, passed | Task 7 |
| `graph_and_a_model_job_share_one_program` (D9 in a batch) | graph demand keyed `no_roots_config: true` (own view) | `build.program` left 2 right 1 | reverted, passed | Task 7 |

Shortcut left: a job holds its own read `Arc` (borrowed from its `make` frame) for the whole batch; only `diff` hands its
reads to the driver, which drops each after its last view. In a multi-workspace batch every other job's read lives to the
batch's end (Task 12's bench reads the peak; upgrade: jobs read inputs through `ViewCx::inputs()` only).

## 12 early (E12-C1…C8, `/work/bench/160-12/early-code-review.md`)

| id | outcome | evidence | proof |
|---|---|---|---|
| E12-C1 | **fixed** | `drive` finalizes a job after its last view in VISIT order (`last_view` = max key position); the skip rule now reads only sides EARLIER by side index. The sub-point "a side must never run before its own old side" is refuted by D8: "Job 1's old side and job 2's new side compute on A". | `a_job_finalizes_after_its_last_view_in_visit_order`, `batch_diff_sides` |
| E12-C2 | **fixed** | the driver holds demands as `Option` and drops every demand of a view (its `Arc`) with the view; `DiffJob::demands` moves its reads to the driver; `WorkspaceReader` keeps `Weak`s; `DiffJob::compute` reads `cx.inputs()`. Standalone `diff A B` holds only B's read during B's build (A's dropped with view A). Both sides are still read up front (D3; R6-3's stated counter change kept: D8 says "the new side's `SideDoc` and its build lines are discarded", it does not forbid the read). | `a_view_drops_its_read_before_the_next_view`, `diff_job_hands_its_reads_to_the_driver` |
| E12-C3 | **fixed** | `run_view` drops the view (and `fresh`) inside a `view.teardown` span; run.rs teardown comment updated. | `analyze_frees_its_view_in_a_named_span`: break (span removed) → "one view.teardown span" left (0,0) right (1,1); reverted, passed |
| E12-C4 | skipped (after the Task 11 rebase) | — | — |
| E12-C5 | **fixed** | `policy_plan(detectors, rules)`; `PolicyJob::resolve` passes `self.policy.is_some()`; `--no-policy` asks an empty request. | `policy_check_no_policy_keeps_no_raw_facts`: break `true \|\| self.policy.is_some()` → `build.witness_pass: got 1, want 0`; reverted, passed |
| E12-C6 | **fixed** | `RequestInputs::read` opens `inputs.request_read` (not `inputs.read`: that counter counts workspace reads, and every counter row reads it) and emits a `request_inputs` census bucket (policy and baseline paths and bytes) under `ALSEM_RETAINED_CENSUS=1`; the file/path tallies are shared module functions. | `request_census_charges_its_files`: break (baseline not tallied) → bytes left 29 vs 100+7+paths; reverted, passed |
| E12-C7 | **fixed** | `pub type JobFailure = JobOutput`; `line`/`message` moved to `JobOutput`; the `From` impl and the `.into()` calls are gone. No behaviour change (whole suite and goldens green). | none (type merge) |
| E12-C8 | **fixed** | `ViewDemand::new` / `ViewDemand::one(inputs, single_app, no_roots_config, needs)` used by all 8 one-side jobs (key from the read's own path); `JobOutput::text(out, Result<(text, exit), msg>)` shared by query, prove, digest. | none (refactor; whole suite and goldens green) |

Why one commit: the E12 fixes (C1, C2, C8, C7) sit in the same hunks as Task 7's driver (`batch/mod.rs`, `job.rs`,
`diff/cli.rs`, every job file), every commit must build and pass the pre-commit hook, and interactive hunk staging is not
available; a split by file would not compile.

## Task 11 — case B: rebase onto step 9 (6172492f)

Rebase `-c core.commentChar=';' rebase origin/master`: 16 subjects equal `1134bed1..2e1319f9`; every rebased commit
builds (`cargo check --tests --bins` per commit in a detached scratch worktree, all OK). Conflicts were far wider than
the expected d35 one (`cli_d35_unknown_commit.rs` merged cleanly): Task 2 (CHANGELOG, fingerprint_cli shard check),
Task 3 + 4 (session.rs Plan/TreeRead/prepare, snapshot.rs, digest_rule_b_tests), Task 5 (run.rs build), Task 6
(alsem.rs, run.rs, capability_cone, session, prove, fingerprint_cli, counters), Task 6 review (Cell::Base), Task 7
(alsem.rs, batch job/mod, prove, fingerprint_cli, CHANGELOG). The seam landed inside those resolutions (each commit had
to build): step 9's `TreeRead::{FingerprintQuery, Inventory}` on `TreeDemand.read`; `gaps_in_cone` per tree demand;
`tree_reads: BTreeMap<JobId, TreeReadOut>` + `take_tree_read(job)`; prove's and the query's root resolution in
`Job::resolve`; `build_cross_app_base_from(.., no_roots_config)`; `Cell::Base` after `Cell::TreeRead` (bit 9; u16).
Case-B commits: 25dfd1a5 (tests, CHANGELOG), fefdb885 (alias), 53b815b0 (TreeApps, replaces 25dfd1a5's forbidden
`#[allow]`). Gates on 53b815b0: task-affected 0 (4252 passed), check-goldens 0 (1197 passed), check-diff no reasons.
No golden moved.

| test | break (Edit, applied once) | broken | restored |
|---|---|---|---|
| `batch_tree_reads_use_own_subjects` (T22) | every tree's subjects = the view's union (`plan.witness.subjects`; spec's break) | FAIL job 0 (inventory) stdout | PASS |
| same, on plain `ws_dep()` | same break | **PASS** (limit: `DoWork` writes dep table 18 itself, so no output shows a shared raw set) → fixture `ws_split` | — |
| same (T22) | each read over `plan.witness.ids` instead of `demand.subjects.ids` | FAIL job 1 (query CallLib) stdout | PASS |
| `batch_counters_cut_readers` (T3 case B, b) | `ProveJob::demands` single-app always | FAIL program got 1 want 2, … | PASS |
| same (a) | `prove_plan` asks no tree (`take(0)`) | FAIL batch panics (effects not filled) | PASS |
| `empty_request_composes_only_with_a_tree` (T19 c) | inventory asks `primary()` | FAIL exit 101 (prepare's "no tree read asks primary" debug_assert; not the witness counter) | PASS |
| `fingerprint_no_roots_config_cross_app_routine`, `inventory_only_no_roots_config_cross_app` (E12-C4) | view builder passes `false && key.no_roots_config` | FAIL both | PASS |
| `tree_read_lifecycle` (f) (Cell::Base in step 9's mask check) | `bit` = `1 << min(self, TreeRead)` | FAIL "(f) every Cell bit is distinct" | PASS |
| `digest_plan_asks_a_tree_only_for_roots` (rewritten: no `PartialEq` on `TreeRead`) | digest_plan `gaps_in_cone: true` | FAIL (and `only_proves_plan_counts_the_gaps_in_its_cone`) | PASS |

T3 case-B rows (measured = derived): (a) B0 + `prove --single-app`, one single-app view 1,2,2,1,3,2,1,2,2,1;
(b) B0 + prove cross-app, two views = DIGEST_ROW + FINGERPRINT_ROW 2,3,3,2,5,3,2,2,2,1. Step 9's standalone rows
(prove, `--routine`, `--inventory-only` = DIGEST_ROW / INVENTORY_ROW) pass unchanged.

## 12 early C4

**Fixed by the rebase.** On 2e1319f9 `build_view_model`'s cross-app path called `build_cross_app_base_from(program,
inputs, mi)` (no flag). Step 9's `build_cross_app_base_with(.., no_roots_config)` conflicted with it in Task 6; the
resolution gives `build_cross_app_base_from` the flag and the builder passes `key.no_roots_config`
(`src/engine/gate/run.rs`), and `AnalysisTarget::build_with` goes through the same builder. Two keys differing only in
the flag now build different models (the overlay applied or not). Proof: row above (both step 9 cross-app
`--no-roots-config` tests fail with the flag dropped, pass restored; they run the standalone command, which is a
one-job batch through this builder).

## Task 11 review fixes (R11-1..R11-3)

Commits `474ba17c` (R11-1) and `ddc97897` (R11-2 spec, R11-3, with Task 8 and the Task 7 review: they share files).
Gates on `ddc97897`'s tree: `ci-steps task-affected` exit 0 (4267 passed, 8 skipped); `check-goldens` exit 0, no golden
moved (`git status tests/` only test files); pre-commit hook passed both; `check-diff --base 6172492f` `{"reasons": []}`.

- R11-1: the three `Roots(∅)` comments now say "an empty request" (`digest_rule_b_tests.rs` ×2, `cli_build_counters.rs`).
- R11-2: spec D10 gains the two case-B rows (1,2,2,1,3,2,1,2,2,1 and 2,3,3,2,5,3,2,2,2,1); T22 and §9 name `ws_split`
  with the reason (on `ws_dep()` the union break is not observable).
- R11-3: `session.rs` `debug_assert!(!demand.subjects.primary)` → `assert!(…, "no tree read asks primary")`.

| test | break | fail | pass |
|---|---|---|---|
| `empty_request_composes_only_with_a_tree` (T19 c) | `fingerprint_inventory_plan` asks `WitnessRequest::primary()` | run exit 101, `session.rs:904` "no tree read asks primary" (now an `assert!`; debug build only — no release build run, per brief) | reverted, passed (gate) |

## Task 7 review fixes (R7-1..R7-9)

| id | verdict | what |
|---|---|---|
| R7-1 | **fixed** | `WorkspaceRead` trait (`get`, `hand_off`): `Arc<WorkspaceInputs>` (library callers keep their own `Arc`) and `HandOff` (the binary's: `hand_off` takes it). Every job's options hold `&dyn WorkspaceRead`; `demands` hands the read to the driver (`ViewDemand::one` takes it); compute reads `ViewCx::inputs`, resolve reads the new `ResolveCx::inputs`, a failed view's `ViewFailure.inputs` (analyze's empty report). Fingerprint's query document carries a `QueryWorkspace` (path + fingerprint), not the read. The bin's `ready` wraps the read in a `HandOff`. `batch.held` gains `inputs=` (the reads of later views; D6.3 (c) amended). |
| R7-2 | **fixed** | `cli_batch_traces.rs` walks the trace in file order (one mutex writer = emission order) with stacks of open job/view spans; no timestamps. |
| R7-3 | **stated deviation** | spec §10: diff reads both sides when made (D3 reads everything before any write; lazy reading would open read-after-write). |
| R7-4 | **fixed** | spec T6/D10 amended; the trace test adds fingerprint B0 (a cut reader) and asserts its fills lie in `job.6.fingerprint` and a view, every other fill in no job span. |
| R7-5 | **fixed** | the bin's `ready(workspace, needs, make)` reads once with the needs stated once and hands `make` the read; `diff_reads` (diff/cli.rs, `detect_input_kind`, the one place diff states its reads) replaces `is_dir`. No observable change (Workspace ⇔ exists ∧ is_dir ⇔ is_dir), so no test. |
| R7-6 | **fixed** | `JobLine.exit: u8`; `worker` removed (Task 10 adds it). |
| R7-7 | **fixed** | CHANGELOG states the two exceptions (view `log` lines, write-error line), the job-line replay, write checks, stdin, census, and the spelling limit. |
| R7-8 | **fixed for graph's id error; rest stated** | `GraphJob.workspace` (its own spelling) → `export_as(program, workspace)`; other build error texts of a shared view keep the first spelling (spec §10 stated limit, CHANGELOG). |
| R7-9 | **fixed** | the `view.<v>` span, its census line and `view.drop` only when the view has a demand to run. |

## Task 8 — writes, failures, stderr classes, diff across views

- **Write sets** (`manifest::check_write_sets(&[(name, JobPaths)], cwd)`, bin `job_paths`): every target resolved
  against the cwd (lexical `.`/`..`), streams already absolute; file/file equal (across and within jobs), file inside a
  dir target, nested dirs, read-after-write (other jobs only: a job reading what it writes is standalone behaviour,
  e.g. `--baseline B --update-baseline`), a file another job writes inside a job's workspace that the workspace read
  takes (`*.al`, `*.app`, `app.json`, `roots.config.json`, `al-sem.coverage.yaml`, `al-sem.policy.y[a]ml`). Exit 3, one
  line, nothing built.
- **Write failures**: `write_job` opens stdout then stderr (a failed stdout skips stderr); job exit = its write-failure
  code; the error line on the run's stderr (already). Zero-view phase existed (Task 5); now pinned (T24).
- **D7**: `output::job_line` + `capture` (thread-local, restored on panic). `build_l4`'s print and graph's coverage
  warning go through `job_line`. The driver captures `prepare` (the solve's lines, given once per job and view to each
  demand whose `Plan::builds_l4()`), each `compute`, graph `program` and `view_failed`; `finalize` keeps lines up to the
  first failed side; `serialize` prepends them. `run_doc` (library) prints them. `Plan::builds_l4` is now prepare's own
  `need_l4`.
- **Census**: `[retained-census] batch.held outputs= sides= inputs=` at each run view's start; `SideDoc::held(doc, bytes)`
  (diff's side: `CborValue::heap_bytes` + its lines, computed only with the census on); `JobOutput::bytes`.
- **Stdin**: `stdin_text()` (a `OnceLock`) read on first `--diff -`.
- **Seam**: `ALSEM_TEST_SUMMARIZE_DIAGNOSTIC` (spec T1 B1; `cfg!(debug_assertions)`) added here, since T21 needs a build
  line; Task 9 reuses it.

Plan/spec corrections:
- `check_write_sets` takes `(name, JobPaths)` rows, not `&Manifest`: the clap `Cli` is in the binary (as Task 7's
  `parse_job`), so the bin extracts each job's paths and the lib checks them.
- D7 "the line becomes a value on the cell that produced it": Task 1 found only two library job lines, both on the
  driver's thread; they are captured at print time per driver step (spec D7 "As built"), not stored on cells. Same replay
  rule. No "det line" exists.
- `diff A A --strict` prints the solve line ONCE (one view, one solve, D8), standalone and batch; before Task 6 it
  printed twice (two views). Not a golden (seam only).
- T5 (a)'s order is observed through write-error lines (two jobs whose writes fail), since files carry no order.
- T12's failing view is a no-`id` workspace (program builds for graph, then no model): it always runs, unlike
  `unreadable_ws`.

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| `a_job_holds_no_read_past_its_view` (lib, R7-1) | `HandOff::hand_off` `.clone()` for `.take()` | "A's read outlived its view" | reverted, passed |
| `a_skipped_view_opens_no_span` (R7-9) | a span opened when `on_view` is empty | views left 2 right 1 | reverted, passed |
| `graph_error_names_its_own_spelling` (R7-8) | `export_as` names `inputs.workspace` | job 1 stderr names `/tmp/…` not `/tmp/…/.` | reverted, passed |
| `batch_stdin_read_once` | `stdin_text` reads stdin per call | job 2 stdout "" | reverted, passed |
| `batch_relative_paths_two_anchors` | manifest stream path resolved against the cwd (`absolute(&p)`) | "stdout under the manifest's dir" | reverted, passed |
| `batch_write_conflicts_are_usage_errors` | file/file check off (`false && x == y`) | "(b) two jobs, one --out": exit 0, want 3 | reverted, passed |
| same | file-in-dir check off (`starts_with(d.join(..))`) | "(b3) a --out inside another job's shard directory": exit 0 | reverted, passed |
| same | dir/dir check off | "(b3) two nested shard directories": exit 0 | reverted, passed |
| same | read-after-write check off (`false && *f == read`) | "(b5) fingerprint --out X, then diff X ws": exit 0 | reverted, passed |
| same | workspace check off | "(b5) a workspace file another job writes": exit 0 | reverted, passed |
| `batch_writes_in_job_order` (T5 a) | each job written as soon as it finalizes (write at compute) | write errors `1.out` before `0.out` (4 lines) | reverted, passed |
| `batch_job_write_failures` (T5 d) | run exit 1 when any job line's exit ≠ 0 | run exit 1, want 0 | reverted, passed |
| `batch_zero_view_file_diff`, `batch_every_job_fails_validation` (T24) | zero-view phase skipped (`last.is_none() && false`) | lines 1 vs 3; 0 vs 2 | reverted, passed |
| `batch_view_failure_per_job` (T12) | every demand of a failed view gets the FIRST job's `view_failed` | run exit 101 "a side document of its own job" | reverted, passed |
| same (D9) | builder skips the program for graph when the id is missing (`true &&`) | run exit 101 "the call-graph export reads a cross-app program" | reverted, passed |
| `batch_view_log_lines_once` (T12) | one `run_view` per demand (view lines per job) | log lines doubled on the run's stderr | reverted, passed |
| `batch_diff_failed_old_side_drops_new_side_lines` (T21) | `finalize` keeps every side's lines (`true \|\| !failed`) | "job 1 replays its discarded new side's line" | reverted, passed |
| same (D7 once per view) | `first_of_job = true \|\| …` | "one solve": 2 lines (the first run of this break compared with standalone and stayed GREEN, since standalone runs the same driver; the test was rewritten to state the count) | reverted, passed |
| same (D7 replay) | prepare's lines never replayed (`false && builds_l4`) | "one solve": 0 lines | reverted, passed |
| `batch_held_census` (D6.3) | `inputs` counts the current view's read too | view B inputs 2868, want 0 | reverted, passed |
| same | diff's side reports 0 bytes | view B sides 0 | reverted, passed |
| `batch_trace_split_per_job` (T6, R7-2, R7-4) | job 0's span opened around `prepare` | "fill 3 inside [job.0.analyze]" | reverted, passed |
| `batch_closed_run_stdout_exits_1` (T5 c) | — (a pin: the run's own write rule predates Task 8) | — | passed |

## Task 8 review fixes (R8-1..R8-7)

Commit `dc7a0257` (R8-1..R8-4, R8-6, R8-7); R8-5 is in the Task 9 commit (its tests sit with the contract tests).
Gates on `dc7a0257`'s tree: `ci-steps task-affected` exit 0 (4267 passed, 8 skipped); `check-goldens` exit 0 (1211
passed, 4 skipped), no golden moved; pre-commit hook passed.

- R8-1: `manifest::resolve` folds `.`/`..`, canonicalizes the deepest existing ancestor and appends the rest (lowercased
  on Windows). Spec D2.4 amended (symlinks, case, 8.3 no longer a limit); CHANGELOG states it.
- R8-2: `.al` via `source_text::has_al_extension`, `.app` and `WORKSPACE_FILES` with `eq_ignore_ascii_case`.
- R8-3: fixed, not stated: `read_by_workspace` adds `cache_scope_role(&package_cache_scope(ws), f) == AppFile` (the
  loader's own candidate folders, existing or not, so a NEW `.app` is caught too; `discover_app_files` lists only
  existing files).
- R8-4: rows added (a `--out` on job 0's default stream file; job 1's stderr file not created after its stdout failed).
- R8-6: `with_test_seam` is `#[cfg(debug_assertions)]` (shadowing `diagnostics`, so release has no `mut`). No proof:
  a release build is out of scope (brief); the debug seam tests (`cli_batch_diff`, T1) still pass.
- R8-7: CHANGELOG Changed line for R11-3's `assert!`.

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| `batch_write_conflicts_are_usage_errors` (R8-1 row, unix) | `resolve` returns the lexical path (`if cwd.is_absolute() { return lexical; }`) | "(R8-1) a write through a symlink…": exit 0, want 3 | reverted, passed |
| same (R8-2 row) | `.al` test `e == "al"` (case-sensitive) | "(R8-2) an upper-case .AL file…": exit 0, want 3 | reverted, passed |
| same (R8-3 row) | `under_ws \|\| false && cache_scope_role(..)` | "(R8-3) an .app in an ancestor .alpackages…": exit 0 (and a write error: the dir does not exist) | reverted, passed |
| same (R8-4 stream row; a pin, green before the fix) | default streams named `{ext}x` | "(R8-4) a --out on another job's default stream file": exit 0, want 3 | reverted, passed |
| `batch_job_write_failures` (R8-4 order row) | `write_job` creates the stderr file before the stdout file | "a failed stdout file stops the job before its stderr file" | reverted, passed |

R8-5's tests and proofs are in Task 9 (commit `de5b151e`): `batch_job_lines_by_count`, `job_line_outside_a_step_reaches_stderr`.

## Task 9 — contract tests and the `Arc` census

Commit `de5b151e` (with R8-5). Gates on its tree: `ci-steps task-affected` exit 0 (4272 passed, 8 skipped);
`check-goldens` exit 0 (1216 passed, 4 skipped), no golden moved (`git status tests/`: test files only); pre-commit hook
passed; `check-diff --base 6172492f` `{"reasons": []}`. No CDO run, no release build.

- **Census (D11):** `Plan::fills()` (Rel/Raw/L4/Det as `prepare_inner` derives them); after the view's one `prepare`
  the driver prints `[retained-census] batch.shared cell=<model|rel|raw|l4|det> job_refs=<n>`; after the computes
  `batch.shared cell=runs bytes=<Σ distinct> allocs=<n> job_refs=<n>`. `ViewCx::take_run` records each handed run
  (`Weak` + `RunOutput::census_tally` bytes, census on only). Analyze now takes its run through `cx.take_run` (was
  `cx.view().run`), so every run reader is counted; output unchanged (goldens green).
- **Seams:** `ALSEM_TEST_GRAPH_COVERAGE_WARNING` (debug only; no fixture breaks the coverage contract) for R8-5 (b).
- **Fixtures:** T1 adds `ws_split` (S) and #265 A6d's workspace (`cli_d35_unknown_commit::dependency_subscriber_ws`,
  extracted from `dependency_subscriber_keeps_todays_text`, D) to `ws_dep()` (W) and the r0 dir (R).

Plan/spec corrections:
- **B2 and B3 are not observable on `ws_dep()`** (measured: both breaks GREEN with W and R only). B2 needs `ws_split`
  (as T22 found); B3 needs a dependency finding (`--scope all` ≠ primary): W has none, and a source dependency carrying
  the r0 d1 code gave no finding either (detectors walk workspace callers), so D is A6d's (a d35 finding on a dependency
  subscriber). Both added to T1 with their preconditions asserted (B3: D's two scopes differ standalone).
- **Bytes "beside" job_refs:** model/rel/det/l4 bytes stay the cells' own `bucket=` lines (printed once at the fill);
  the `batch.shared` line carries `job_refs` only. `runs` has no fill bucket, so its line carries the bytes.
- **B6's precondition** ("the second digest's JSON carries a proof") is not a JSON key (proofs feed the witness
  paths). The break was observed through R's prove (a later tree on the same view as R's digest).
- **T2's break** fails through `prepare`'s "every tree demand's subjects are inside the view's request" assert (exit 101),
  not through a differing tree: the guard catches a short union first.
- **Task 4's correction (witness counter) is in T1:** the traced precondition run asserts `build.witness_pass` +1 and
  `build.snapshot` ≥ +2 inside W's cross-app view span.
- T22 already active (Task 11); not re-added.

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| T1 B1 `batch_equals_standalone` | driver replay off: `if false && builds_l4 && first_of_job` | "B1's precondition: digest's l4 line, once" left 0 right 1 (every equality stayed green: standalone shares the driver) | reverted, passed |
| R8-5 (a) `batch_job_lines_by_count` | same break | "(a) one solve" left [0,0,0] right [1,1,0] | reverted, passed |
| T1 B2 | tree scopes = the union (`.map(\|_\| plan.witness.subjects(model))`) | first run (W, R only): GREEN → S added; then job 46 (S `--inventory-only`) stdout differs | reverted, passed |
| T1 B3 | `RunKey.scope_primary: true \|\| *scope_primary` | first run: GREEN → D added; then job 52 (D `analyze --scope all`) stdout differs | reverted, passed |
| T1 B5 | `let drain = true \|\| …` | traced run exit 101: `policy_engine.rs:277` "has a summary but no raw inherited facts" | reverted, passed |
| T1 B6 | `let pd = if true \|\| last_tree` | job 37 (R `prove CallerA`) stdout differs | reverted, passed |
| T2 `batch_output_independent_of_neighbours` | `Plan::merge` keeps only the first non-empty request | run exit 101 `session.rs:716` "every tree demand's subjects are inside the view's request" | reverted, passed |
| T7 `batch_arc_census` | `ViewCx::take_run` hands a field-by-field copy to a later reader | "runs" bytes 10240 vs 2560, allocs 4 (all four jobs share one key) | reverted, passed |
| R8-5 (b) | graph's `program` call outside `capture` | "(b) per graph job" left [0,0,0] right [1,1,0] | reverted, passed |
| R8-5 (c) `job_line_outside_a_step_reaches_stderr` | `job_line` drops an unclaimed line (`unclaimed.filter(\|_\| false)`) | left 0 right 1 | reverted, passed |

Red: T1/T2/T7 were written after the census code and passed on first run; the breaks above are their red runs. Limit:
R8-5 (a)'s "graph gets none" pins graph's own path only (graph never reaches the l4 replay).

## Task 10 — workers, admission, the peak table, threads (Steps 2-5; Step 1 deferred)

Commit `a5006853`. Gates on its tree: `ci-steps task-affected` exit 0 (whole suite: 4281 passed, 8 skipped);
`check-goldens` exit 0 (1220 passed, 4 skipped; run before the amend that only replaced the `#[ignore]` lib test),
no golden moved (`git status tests/`: test files only); pre-commit hook passed; `check-diff --base 6172492f`
`{"reasons": []}`. No CDO run, no release build.

- `batch/admission.rs`: `E16_BUDGET_KIB`, `JobKind = (&'static str, ViewUniverse)`, `KIND_PEAK_KIB`, `peak_kib`,
  `worker_count`, `admit` (the `batch.workers` instant `w`/`p_kib`/`policy_cap`, and the over-budget stderr line).
  Known rows (rounded up to 256 MiB): analyze x 3 GiB (`a` 3,142,400), analyze s 0.75 GiB (step 6 ≈ 0.72 GB),
  digest x 3.5 GiB (max of `d`/`h` 3,222,396 and step 6's 3.35 GiB), policy check x 8 GiB (max of `p` 8,280,524,
  7.94 GiB, projected `U` 8,333,320), fingerprint s 1.5 GiB (`f` 1,465,740). **Placeholders (`shortcut:`, = the
  policy row, need Step 1):** digest s, policy check s, fingerprint x, prove x, prove s, diff (s), events fanout x,
  events fanout s, query touches x, query touches s, graph (x). No row (take the largest by rule): events chains x/s,
  query effects x/s.
- `batch/worker.rs`: `run_workers` (one child per batch, `current_exe() batch --worker <b> [--out-dir] <manifest>`,
  at most `W` alive, a supervisor thread per child reports on a channel, the main thread reaps before each start;
  stdin bytes on a writer thread, stderr forwarded by `read_until('\n')` with `[w<b>] `; lines kept up to the first
  incomplete one, the rest synthesized `exit: null`, `worker: <ExitStatus>`; `batch.worker.start`/`.exit` instants
  from the main thread only, so the alive count is exact); `report_peak` (`VmHWM` under the census); the debug seam
  `ALSEM_TEST_WORKER_DIES=<b>` (`kill -KILL` self, else exit 9) after the jobs, before the lines.
- `JobLine.exit: Option<u8>` + `worker: Option<String>` (skipped when none; the one-job goldens are unchanged).
- Bin: `job_kind(&Commands)` (fingerprint's universe via the new `fingerprint_cli::reads_single_app`, sharing
  `FingerprintForm::from_flags` with `demands`; `fingerprint_flags` extracted), `reads_stdin` (`digest --diff -`),
  `run_worker`, `print_lines`/`write_run_stdout`. One batch stays in-process (`batch.workers` w 1, no worker).
- `big_stack_pool` emits `pool.threads` from the built pool. `--help` text (Limit L1, CDO-calibrated). exit_code
  doc, CHANGELOG.

Plan/spec corrections:
- **`#[ignore]` is forbidden by `check-diff`** (`forbidden:ignore-attribute`). The `U−p` ×4 case is
  `worker_count_u_without_policy`: it expects 3 while the events fanout or query touches cross-app row is a
  placeholder, 4 once both are measured. Step 1 needs no test edit; the test then checks 4.
- **T8 uses `RAYON_NUM_THREADS=3`**, not 2: the break (`available_parallelism`) must differ on a 2-core runner.
  It reads the WORKERS' traces (`trace.json.w<b>`), so it also pins inheritance into workers.
- `job_kind` reads the CLI, not the job (the parent never reads a workspace). `has_policy` counts a policy job
  even when it would fail validation (conservative).
- A worker gets stdin bytes when a job says `--diff -` even if `--changed <file>` overrides it (harmless, own thread).
- The parent reads stdin as bytes (`read_to_end`); each worker reads it as text, so a non-UTF-8 stdin fails in the
  worker exactly as standalone.
- Red: tests were written with the code; their red runs are the breaks below (as Task 9).

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| T17 lib injected + committed, cli policy case | policy cap off: `w.min(2)` → `w.min(w)` | injected "fits 3, policy" left 3 right 2; committed "U" left 3 right 2; cli "3 batches, one policy check: W" left 3 right 2 | reverted, passed |
| T17 lib injected | `max(1)` → `max(0)` | "fits 0" left 0 right 1 | reverted, passed |
| T17 lib committed (O16) | U's peak charged to the digest row (`("digest", X), POLICY_X`) | "AD" left 3 right 4 | reverted, passed |
| T17 lib `U−p` (interim form) | `peak_kib` takes the lightest kind (`.min()`) | left 4 right 3 | reverted, passed |
| T17 cli (never more than W alive) | reap when `alive == run.w + 1` | "5 batches: alive" left 5 right 4 | reverted, passed |
| T17 cli (stderr forwarded with `[w<b>] `) | prefix `format!("")` | "3 batches: [w0] [batch] worker 0 peak_kib= in [retained-census] …" | reverted, passed |
| T17 cli + T8 (D10 per-worker trace) | `.w{b}` → `.x{b}` | "worker 0's trace file: No such file or directory" (both tests) | reverted, passed |
| T8 `rayon_threads_size_the_big_stack_pool` | `.num_threads(available_parallelism)` | "worker 0" left 22 right 3 | reverted, passed |
| T10 `batch_stdin_read_once_across_workers` | workers inherit stdin (`Stdio::inherit()`: each reads its own) | "batch 0" left "" right the digest document (the formatter reflowed the line; `grep -c 'Stdio::inherit()'` = 1 after) | reverted, passed |
| `batch_worker_killed` | a missing line as exit 0 (`exit: Some(0)`) | "a job of the dead worker: {…"exit":0,…,"worker":"signal: 9 (SIGKILL)"}" | reverted, passed |

Not covered: the over-budget stderr line (no committed row comes near 28 GiB; `admit` is the only caller of the
check, pinned only by `worker_count`'s `fits 0` case); `report_peak` on a system without `/proc` (omitted by design).

**Step 1 rows (follow-up commit `c1475ac5`, `#277 Task 10 Step 1: the provisional peak rows; U−p runs at 4`).** The
nine measured rows (section "Task 10 Step 1 (provisional rows)") replace their placeholders, each with its max and a
citation of `/work/bench/160-12/task10-step1-peaks.md`. Still placeholders (`shortcut:`, the largest row): **fingerprint
cross-app** (`--routine`/`--inventory-only`) and **prove single-app** (not in the measured list). `worker_count_u_without_policy`
now asserts 4 outright. Gates on `c1475ac5`'s tree: `task-affected` exit 0 (4281 passed, 8 skipped); `check-goldens` exit 0
(1220 passed, 4 skipped), no golden moved; `check-diff` `{"reasons": []}`.

| test | break | fail | pass |
|---|---|---|---|
| T17 lib `U−p` ×4 → 4 | U's peak charged to the events fanout x row (`POLICY_X`) | "U−p" left 3 right 4 | reverted, passed |

## Task 9 review fixes (R9-1..R9-6)

Commit `7f89af03`. Gates on its tree: `ci-steps task-affected` exit 0 (4281 passed, 8 skipped); `check-goldens` exit 0
(1220 passed, 4 skipped), no golden moved (`git status tests/`: the two test files only); `check-diff --base 6172492f`
`{"reasons": []}`.

- R9-1: `policy2.yaml` (one rule `second-policy-writes`) written by `Fixture::new`; W's sarif policy job reads it. T1
  asserts the sarif job's standalone output on the first policy differs from its own (`policy.sarif` bytes).
- R9-2: T2 takes W's jobs by value (`j.iter().any(|a| a == w)`), same exclusions; asserts an analyze, digest, prove,
  events, policy and query neighbour (graph writes `--out`, so stays excluded).
- R9-3: `manifest::fold` canonicalizes an existing prefix before each `..`; `resolve` = `fold` + deepest existing
  ancestor. Row `(R9-3)` in `batch_write_conflicts_are_usage_errors` (`link → sub/real`; write `link/../x.json`, read
  `sub/x.json`).
- R9-4: `read_by_workspace(cwd, ws, f)`: `.alpackages` candidates from `package_cache_scope(fold(cwd, ws))`, each
  `resolve`d, then `cache_scope_role`; `under_ws` against `resolve(cwd, ws)`. Row `(R9-4)` (`b/link → sub/real`;
  workspace `b/link`, write a `.app` under b's package folder). `symlinked_dir` now makes `sub/real`, `link` and `b/link`.
- R9-5: `prepare_inner`'s `need_rel/raw/l4/det` come from `plan.fills()`.
- R9-6: `struct Batch` gone: `run_batch` returns `cli_batch_plan::BatchRun`, `job_of` reads `lines()`/`streams()`;
  `line_counts` uses `streams`. `standalone_in`/`batch_in` not reused: neither pipes stdin nor clears the seams (a
  change to every caller). The traced precondition run still uses `run_manifest` (it needs stdin). View precondition:
  every view ≥ 2 jobs, and the keys read off the jobs' argvs (workspace, `--single-app`; fingerprint/diff skipped)
  cover W×{x,s} and R×{x,s}.

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| R9-1 `batch_equals_standalone` | `RequestInputs::read` serves the first `--policy` path to every later read (a process `OnceLock`) | "policy.sarif: bytes differ" | reverted, passed |
| R9-2 `batch_output_independent_of_neighbours` | the old filter `j.get(1) == Some(w)` | "R9-2: a events neighbour on W's view" | reverted, passed |
| R9-3 row | `fold` never canonicalizes (`.ok().filter(\|_\| false)`; a first try with `Result::filter` did not compile, not counted) | "(R9-3) a write through link/.., …": exit 0, want 3 | reverted, passed |
| R9-4 row | scope from the canonical workspace (`package_cache_scope(&resolve(cwd, ws))`) | "(R9-4) an .app beside a symlinked workspace's spelling": exit 0 (write error: dir missing), want 3 | reverted, passed |
| R9-5 (one rule) | `Plan::fills` drops `raw` from Rel (`det \|\| l4`) | 5 lib session tests panic "rel filled above" (`a_raw_only_composition_is_released_by_prepare`, `a_tree_demand_composes_with_an_empty_request`, `a_det_fill_after_a_raw_only_composition_names_the_rule`, `a_primary_request_covers_an_empty_tree`, `two_trees_one_pass`); before R9-5 this break moved only the census | reverted, passed |
| R9-6 view precondition | — (a precondition; the views it names exist, green) | — | passed |

(The coordinator rebased onto a6d94a91: Task 10 = `6f348633`, Step 1 = `6b789731`, Task 9 review = `b1570e0e`.)

## Task 10 review fixes (R10-1..R10-6)

Commit `dc6df5a1` on `b1570e0e`. Gates on its tree: `ci-steps task-affected` exit 0 (4283 passed, 8 skipped);
`check-goldens` exit 0 (1220 passed, 4 skipped), no golden moved; `check-diff --base a6d94a91` `{"reasons": []}`. Also, with
debug assertions OFF in the dev profile (`CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=false`, own target dir under the scratchpad;
not a release build): `cargo clippy -p al-sem --all-targets -- -D warnings` exit 0, and the cli_batch_workers/contract/diff
tests: 7 run, 7 passed (the 4 seam tests compiled out).

- R10-1: `test_seam_die` is `#[cfg(debug_assertions)]` (no `cfg!`); its call in `run_worker` is `#[cfg(debug_assertions)]`
  (full path, no import). Audit: `with_test_seam` (`l4/analysis.rs`, `ALSEM_TEST_SUMMARIZE_DIAGNOSTIC`) and
  `forced_coverage_warning` (`impact_graph/mod.rs`, `ALSEM_TEST_GRAPH_COVERAGE_WARNING`) were already `#[cfg]` (R8-6 / R8-5).
- R10-2: `#[cfg(debug_assertions)]` on `batch_worker_killed`, `batch_job_lines_by_count`,
  `job_line_outside_a_step_reaches_stderr`, `batch_diff_failed_old_side_drops_new_side_lines` and their seam-only helpers
  (`SEAM_LINE`, `line_counts`, cli_batch_diff's `SEAM` and two imports). T1 (`batch_equals_standalone`) stays in every
  profile: only its B1 precondition (the seam line, once) is debug-only, since its equalities hold without the seam.
- R10-3: `admission::has_policy_check(kinds)` (command `"policy check"`, from `job_kind`); `admit(kinds, batches)` derives
  it, the bin no longer matches `Commands::Policy(_)`. Lib row `only_policy_check_caps`; `committed()` uses it.
  (`policy explain` is refused by the manifest anyway, so the old match was unreachable in practice.)
- R10-4: `run_workers` doc: a worker that exits non-zero but printed all its lines counts as whole.
- R10-5: `read_by_workspace` doc: for a relative spelling the walk climbs the cwd's ancestors too, a superset of the
  loader's; it can refuse a write with no real clash, never miss one.
- R10-6: `KIND_PEAK_KIB` doc names the kinds with no row: events chains and query effects, both universes.

| test | break (Edit, applied once) | fail | pass |
|---|---|---|---|
| R10-3 `only_policy_check_caps` (+ committed, U−p, cli T17) | `*command == "policy check" \|\| true` (the formatter reflowed it; `grep -c` 1 after) | "!has_policy_check(events chains, query effects)"; committed "AD" 2 vs 4; U−p 2 vs 4; cli "3 batches: W" 2 vs 3 | reverted, passed |
| R10-2 `batch_worker_killed` (assertions off) | `#[cfg(debug_assertions)]` removed from the test | "a job of the dead worker: {…"exit":0…}" (no seam in that build) | restored, compiled out |
| R10-1 (assertions off) | the call site's `#[cfg(debug_assertions)]` removed | `cargo check`: E0425 cannot find function `test_seam_die` (the seam is absent from a no-debug-assertions build) | restored, check passed |

## Discoveries

- **Four-worker batch throughput (B6).** `alsem batch` with four concurrent digest workers at `RAYON_NUM_THREADS=5` measured 1.45 × one digest's wall on CDO (bound 1.25 ×; per-worker 155-166 CPU-s vs 108-113 alone), on master 1134bed1 which carries the #289 digest regression. Investigate after #289: re-measure; if still > 1.25 ×, find the shared resource (memory bandwidth, allocator, rayon pool sizing) and either fix it or set W from a measured throughput table. Acceptance: B6 measured ≤ 1.25 × on CDO, or a documented W cap that meets it.

- **B2u: the union-shape batch peak exceeds its bound.** On CDO, the step-12 final bench measured the `U` shape (digest h + digest d + policy check, one view) at 10,056,180 KiB. The bound is 9,514,231 KiB, so the peak is 1.216× the largest standalone against 1.15×. The peak lands where job 2 (digest `h`) serializes its 441 MB output. Policy's raw facts (6.04 GB) are still alive at that point, because policy is job 3. Composing the trees adds about 1 GiB over the raw facts, where the spec projected about 53 MB. F13 case (c) applied. The operator accepted the miss as reported, not gating (amendment 12A-19, 2026-10-25). The fix is still open. Options: order a view's jobs so raw facts are released before the largest serialize; or trim the tree-compose overhead; or write a job's output before the next job computes. Acceptance: B2u ≤ 1.15× on CDO, or a measured, stated bound. Evidence: `docs/measurements/2026-10-25-issue-277-160-step12.md` and `/work/bench/160-12/final/diag/U.report`.
- **Batch wall bounds cannot be measured on the shared box.** B1a/B1b were void twice under the F3 void check: round-to-round bands were 3–9 s and 7–18 s at load 5–19. The batch saving held steady at about 15.5 s in every round. B4's wall rule missed on 13 of 24 rows, with H 0–1.9 s slower on the median, while every peak passed. The operator accepted both as reported (12A-19). Acceptance: re-measure B1a, B1b and the B4 walls on an exclusive, quiet host (or with CPU pinning) and record the result; or restate those wall bounds as CPU-seconds, which the noise does not move.

## Task 10 Step 1 (provisional rows)

Commit de5b151e, `alsem` built `--profile release-fast` (detached worktree `/work/bench/160-12/t10-wt`), 2026-10-24. Peak = `maxrss.py` kb (VmHWM), 3 rounds, rotated, one process at a time. Full file: `/work/bench/160-12/task10-step1-peaks.md`; raw lines `/work/bench/160-12/t10/runs1.txt`, `runs.txt`. Foreign load was 2-245 during the run (other session's gates); peaks are per process and not load-sensitive.

| kind | argv | peak_kib (3 rounds) | max | max rounded up to 256 MiB (KiB) | load notes |
|---|---|---|---|---|---|
| events fanout x | `events fanout <CDO_WS> --format json` | 3,172,880 / 3,192,604 / 3,166,192 | 3,192,604 | 3,407,872 | spread 0.8% |
| events fanout s | `events fanout <CDO_WS> --single-app --format json` | 734,144 / 724,228 / 747,860 | 747,860 | 786,432 | spread 3.3% |
| query touches x | `query touches <CDO_WS> --table 18 --format json` | 3,153,424 / 3,148,860 / 3,143,336 | 3,153,424 | 3,407,872 | spread 0.3% |
| query touches s | `query touches <CDO_WS> --table 6175281 --single-app --format json` (table 18 is a dependency table: exit 2 single-app) | 757,260 / 750,916 / 735,736 | 757,260 | 786,432 | spread 2.9% |
| graph | `graph <CDO_WS> --out <scratch>/graph.jsonl` | 2,585,552 / 2,585,416 / 2,556,468 | 2,585,552 | 2,621,440 | spread 1.1% |
| prove | `prove <CDO_WS> IssueReminder may-commit` (cross-app default) | 3,164,616 / 3,186,124 / 3,187,260 | 3,187,260 | 3,407,872 | spread 0.7% |
| diff | `diff <CDO_WS> <CDO_WS>` | 1,472,632 / 1,468,392 / 1,462,764 | 1,472,632 | 1,572,864 | spread 0.7% |
| policy check s | `policy check <CDO_WS> --single-app --format json` | 751,848 / 720,392 / 721,736 | 751,848 | 786,432 | spread 4.4% |
| digest s | `digest <CDO_WS> --single-app --file src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al --format json` | 746,136 / 725,680 / 744,992 | 746,136 | 786,432 | spread 2.8% |

## Final round 1 fixes (FR-1..FR-14)

Commit `47a1282f` on `dc6df5a1` (one commit). Gates on its tree: `ci-steps task-affected` exit 0 (whole suite: 4296
passed, 8 skipped; clippy-dev, fmt, gen-syntax green); `check-goldens` exit 0 (1233 passed, 4 skipped), no golden moved;
`check-diff --base a6d94a91` `{"reasons": []}`. Every claim was reproduced red before the fix (13 new CLI tests all
failed on `dc6df5a1` for the stated reason; FR-4 as the reviewer said: `capability_cone.rs` "the base's relation was
released past its view's prepare", exit 101). No FR was refuted.

Structure: the parent now PLANS every manifest job once (`plan(command, true)`), before the write check. That one move
carries FR-2 (a `Failed` plan's write set is its streams, `stream_paths`), FR-7/FR-8 (stdin is read lazily by
`plan_digest` exactly where standalone reads it; the parent's `STDIN` read, text or failure, goes to the workers),
FR-14 (a kind counts only when the plan reads a workspace) and the parent's `--changed` decision (FR-3 b). Workers get
a `WorkerInput` (argv, streams, `changed_is_file` per job, the stdin read) on their stdin and never open the manifest.

- FR-1 fixed: `resolve_auto_format(format, in_batch)`, `src/bin/alsem.rs:1983-1997`; debug seam `stdout_is_terminal`
  `:2001-2010` (`ALSEM_TEST_STDOUT_IS_TERMINAL`, `#[cfg(debug_assertions)]`, as the R10 seams). Test forces the tty
  branch without a pty, one batch and two batches.
- FR-2 fixed: `stream_paths` `alsem.rs:925-936`, applied at `:1054-1057`; `job_paths` no longer invents `Dir(".")`
  (`.extend(f.out…)`, unobservable now: a shard job without `--out` is `Failed`, whose paths are discarded — stated).
- FR-3 fixed: `WorkerInput`/`WorkerJob`/`StdinRead` `src/engine/batch/worker.rs:22-48`, written by `supervise`
  `:150-163`; `run_worker` `alsem.rs:1122-1158` reads stdin, pins `--changed` from the input; parent decides via
  `pin_changed` `:1531-1541` / `detect_changed` `:1550-1556` (also used by `job_paths`); the manifest is a read of "the
  run" `:1065-1072`.
- FR-4 fixed (guard, numbers kept meaningful): `R3a5CrossAppBase::held_rel` `capability_cone.rs:3744-3751`;
  `census_output_line` `session.rs:1115-1119` reads the base's handle, else the view cell's, else 0.
- FR-5 fixed: `taken_files` `src/engine/batch/manifest.rs:165-196` (walkdir following links, the source walk's skipped
  folders, plus `discover_app_files`, each canonicalized), cached per workspace in `check_write_sets`; used by
  `read_by_workspace` `:143-153`. CHANGELOG's symlink claim now true and amended.
- FR-6 fixed: `build_view_model` no-id branch `src/engine/gate/run.rs:402-415` takes the coverage of graph's program
  (no `build_single_app`). Pinned with `build.program` 1 on `ws_dep` with its `id` removed (a source-bearing dependency),
  and both jobs equal standalone.
- FR-7 fixed: no `reads_stdin`; stdin is read inside `plan_digest` after validation and the `--changed` override.
- FR-8 fixed: `STDIN` static `alsem.rs:1515-1528`, its failure forwarded as `StdinRead::Failed` and seeded in the worker.
- FR-9 fixed: `PolicyJob::reads` `src/engine/gate/policy/pipeline.rs:305-314`, used by `resolve` and `check`'s keep list.
- FR-10 fixed: `report_peak(0)` after the in-process batch, `alsem.rs:1079-1082`.
- FR-11 fixed: `PolicyJob::new`'s error path gives the read up (`pipeline.rs:241`); `fingerprint --shard` without
  `--out` fails before `ready` (`alsem.rs:1811-1818`, `reject_shard_without_out` `fingerprint_cli.rs:550-560`);
  `DiffJob::new`'s early outcomes all precede its reads (doc).
- FR-12 fixed: `Slot`/`hold`/`make_jobs` `alsem.rs:795-826`, `run_one_batch` `:1227-1240`; `with_jobs` deleted. Jobs
  that borrowed their options now own them: `EventsJob`, `PolicyJob`, `DiffJob` (options `Copy`), `FingerprintJob`
  (options `Clone`, the lib entry clones once). No new test (structural; every batch test runs the loop); no stack
  limit measured.
- FR-13 fixed: `check_stream_dirs` `manifest.rs:275-299`, called right after `load` (`alsem.rs:1028`). D2 was silent:
  fail fast, exit 3, amended in D2.1. T5 (d) and `batch_writes_in_job_order` now make a stream unwritable by naming a
  directory (a missing directory is a usage error now).
- FR-14 fixed: `kinds.extend(kind.filter(|_| !p.reads().is_empty()))` `alsem.rs:1059`.

Spec amended: A12.3 (proof row), D2.1 (FR-13, FR-2), D2.4 (FR-3 manifest, FR-5), D2.5 (FR-3 worker input), D3 (FR-3,
7, 8, 11, 12, 14), D6.1 (FR-9, FR-6), D12 (FR-14, FR-10). CHANGELOG: the `alsem batch` entry and the `--no-policy`
entry.

| test | break (Edit, applied once, `grep -n` checked) | fail | pass |
|---|---|---|---|
| FR-1 `batch_auto_format_is_json_on_a_terminal` | `"auto" if stdout_is_terminal()` (no `!in_batch`) | "1 batches: batch 0": terminal text vs JSON | reverted, passed |
| FR-2 `batch_failed_job_writes_only_its_streams` | `Planned::Failed(_) => written` | exit 3 "batch 0 job 2 and batch 0 job 3: both write …/0.3.stdout" | reverted, passed |
| FR-3a `batch_refuses_writing_the_manifest` | the run's read `vec![]` | exit 0, want 3 | reverted, passed |
| FR-3b `batch_changed_is_decided_once` | worker `pin_changed(&mut c, None)` | job exit 1 vs standalone 2 | reverted, passed |
| FR-4 `output_census_after_a_released_base` | `CrossApp(b) => Some(b.rel())` | run exit 101, "the base's relation was released past its view's prepare" | reverted, passed |
| FR-5 `batch_refuses_writing_a_linked_workspace_input` | `taken.contains(f)` → `taken.is_empty() && false` | "A.al": exit 0, want 3 | reverted, passed |
| FR-6 `graph_and_analyze_without_an_id_build_once` | the old `drop(program)` + `build_single_app` branch | `build.program` 2, want 1 | reverted, passed |
| FR-7 `batch_unread_stdin_does_not_block` | an eager `let _ = stdin_text();` before planning | "the run waits for stdin no job reads" (120 s) | reverted, passed |
| FR-8 `batch_stdin_read_error_is_each_jobs` | worker seeds `Failed` as `Some(String::new())` | "batch 0": exit 1, want 3 | reverted, passed |
| FR-9 `no_policy_reads_no_raw` | `None => &[Cell::Det, Cell::Raw]` | raw released at 19553, after job 1's compute (19394), not in the policy's | reverted, passed |
| FR-10 `one_batch_reports_its_peak` | `report_peak(0)` removed | no `[batch] worker 0 peak_kib=` line | reverted, passed |
| FR-11 `a_job_holds_no_read_past_its_view` (lib, extended) | `drop(opts.inputs.hand_off())` removed | "the failed job kept A's read" | reverted, passed |
| FR-13 `batch_missing_stream_directory_is_a_usage_error` | `check_stream_dirs` call → `Ok::<(), String>(())` | exit 0 (write error), want 3 | reverted, passed |
| FR-14 `admission_skips_failed_jobs` | `kinds.extend(kind)` | W 3, want 4 | reverted, passed |
| T5 (d) `batch_job_write_failures`, `batch_writes_in_job_order` (fixture changed) | stream `create` errors not reported (`map_err(\|_\| ())`) | 1 write error vs 2; 0 vs 2 | reverted, passed |
| bin unit `auto_format_non_tty_resolves_to_json` (signature changed) | `"auto" => OutputFormat::Terminal` | Terminal vs Json | reverted, passed |

Notes: the FR-1 break failed on the one-batch case first, so the two-batch case did not run under it (same rule, same
call site). FR-9's first form (a digest before `--no-policy`) could not discriminate: prepare already drops `raw`
when no plan asks for a witness; the kept test orders a policy with rules first. The `--no-policy` keep-list change alone
is not observable (an earlier reader now frees `raw` itself); it is kept for one declaration (`reads()`).

Risks: (1) FR-6 rests on the coverage of a cross-app (FULL) program equalling the single-app one; pinned on `ws_dep`
only, not on CDO. (2) `taken_files` walks each workspace once per run (every config-named, `.al`, `.app` file,
following links): on CDO a few thousand `stat`s; a link loop is skipped (`flatten`), so its targets are not compared.
(3) Standalone `fingerprint --shard X` without `--out` no longer reads the workspace before failing: any `log` line a
read would print is gone for that doubly-broken invocation (text and exit unchanged otherwise). (4) Planning in the
multi-batch parent reads every digest diff file once more than before (the workers read it again).

## Final round 2 fixes (R2-1..R2-5)

Commit `d7ef7a79` on `47a1282f` (one commit, "#277 final r2: R2-1..R2-5"). Gates on its tree: `ci-steps task-affected`
exit 0 (4303 passed, 8 skipped); `check-goldens` exit 0 (1238 passed, 4 skipped), no golden moved; `check-diff --base
a6d94a91` `{"reasons": []}`. Every item reproduced red on `47a1282f` first (the 5 new CLI tests and the R2-5 bin test failed for
the stated reason; the R2-3 test passes on `47a1282f`, which is already a loop, and fails on a restored recursion). No
item refuted.

- R2-1 fixed. (a) A `Failed` plan keeps the reads its command line names and drops only its writes
  (`alsem.rs:1058-1064`). (b) Plan-time file reads go through one run-wide table, `PLAN_FILES` / `plan_read` /
  `plan_exists` (`alsem.rs:1549-1571`), the type `PlanFiles` (`worker.rs:36-44`, `WorkerInput::files` `:32`). The
  parent hands it to every worker (`alsem.rs:1097`, `:1111`); the worker seeds it before planning (`:1151`). Readers:
  digest's `--diff <file>` (`alsem.rs:1667`, via `FileInput::read_with(..).text()`, so the message and the UTF-8
  error text stay `read_to_string`'s) and `policy check`'s request (`RequestInputs::read_with`, `inputs.rs:186-241`,
  `read` now delegates; `FileInput::read_with` `inputs.rs:88-92`). `changed_is_file` is kept as it was (FR-3). Each such
  file is read once per run, so a FIFO works. Spec D2.5 :329-330 rewritten (child plans off the parent's reads and
  answers), D2.1 amended.
- R2-2 fixed. `taken_files` returns `Taken { files, linked_dirs }` (`manifest.rs:169-217`): every linked directory
  the walk enters, canonicalized; `read_by_workspace` (`:146-158`) takes an `is_input_name` file under any of them,
  existing or not. A linked `.alpackages` was already caught (its candidate is `resolve`d to its target, `:151`);
  pinned now. Spec D2.4 amended; CHANGELOG states linked directories.
- R2-3 fixed (test). Bin unit test `make_jobs_runs_in_constant_stack` (`alsem.rs:2282-2299`): 100 000 zero-view jobs
  through `make_jobs` on a 256 KiB thread. Discriminates in the dev profile (no TCO guarantee; a release build could
  flatten a tail call, so the proof is stated for the debug test build only).
- R2-4 fixed. `PolicyCheck::resolve` (`pipeline.rs:249-283`) is the policy resolution split out of `PolicyJob::new`;
  `plan_policy_check` reads the request and resolves it in planning (`alsem.rs:1372-1392`): a bad policy is
  `Planned::Failed` (no reads, so no admission row, no policy cap) and the job is made with `PolicyJob::resolved`.
  Standalone too: a bad policy no longer reads the workspace first (text and exit unchanged; CHANGELOG says so).
  `PolicyJob::new` (lib, used by `run_policy_check` and the FR-11 test) keeps its early `hand_off` drop.
- R2-5 fixed. `WorkspaceReader` holds a strong `Arc` of every read in `making` (`job.rs:204`, pushed `:246`) until
  `made()` (`:223-226`); `make_jobs` calls it after the last job (`alsem.rs:825`), the lib `run_diff`
  (`diff/cli.rs:333-336`) after `DiffJob::new`. In the bin no job gives its read up at construction any more (R2-4),
  so the reader fix is the structural guard; the bin unit test pins it directly.

| test | break (Edit, applied once, `grep -c` = 1) | fail | pass |
|---|---|---|---|
| R2-1 `batch_failed_job_keeps_its_reads` | `reads: written.reads` → `reads: vec![]` | exit 0, want 3 | reverted, passed |
| R2-1 `batch_diff_fifo_is_read_once` (unix) | worker seeding → `let _ = input.files;` | "the run reads the FIFO twice" (120 s timeout) | reverted, passed (0.13 s) |
| R2-2 `batch_refuses_writing_under_a_linked_directory` (shared/New.al) | `linked_dirs.iter().any(..)` → `(taken.linked_dirs.is_empty() && false)` | "shared/New.al": exit 0, want 3 | reverted, passed |
| R2-2 same test (pkgs/New.app) | candidates `resolve(cwd, c)` → `fold(cwd, c)` | "pkgs/New.app": exit 0, want 3 | reverted, passed |
| R2-3 `make_jobs_runs_in_constant_stack` | `make_jobs` as a recursive `go` (one frame per job) | SIGABRT "has overflowed its stack" | reverted, passed |
| R2-5 `make_jobs_reads_a_workspace_once` | `making.push(..)` → `making.truncate(0)` | "the workspace was read twice" | reverted, passed |
| R2-5 same test | `reader.made();` in `make_jobs` removed | "the reader kept the read past the jobs' making" | reverted, passed |
| R2-4 `admission_skips_bad_policy_jobs` | bad-policy `Err` arm → a `ready(..)` plan making a `Failed` job | W 2, want 4 | reverted, passed |
| R2-5 `failed_policy_job_reads_once` | R2-4 break AND `making.truncate(0)` together | `inputs.read` 2, want 1 | reverted, passed |

Notes: `failed_policy_job_reads_once` passes under either break alone (R2-4 alone: the plan reads nothing; the reader
fix alone: the read is still held); it is red on `47a1282f`, which lacks both. The FR-11 lib test
`a_job_holds_no_read_past_its_view` passes unchanged. The diff lib test `the job kept a workspace read` (diff/cli.rs:504)
now calls `reader.made()` after `DiffJob::new`, as `run_diff` does.

FR anchors touched: FR-2 (`alsem.rs:1058-1064`, the `Failed` arm), FR-3 (`WorkerInput` `worker.rs:22-45`; `run_batch_cmd`
`alsem.rs:1097`, `:1111`; `run_worker` `:1151`), FR-5 (`manifest.rs:135-216`, `read_by_workspace` and `taken_files`),
FR-11 (`pipeline.rs:223-285`: `PolicyJob::new` delegates to `PolicyCheck::resolve`, the drop kept), FR-12 (`make_jobs`
`alsem.rs:810-827`, `reader.made()` added). FR-9/FR-14 code unchanged.

Risks: (1) the parent holds every plan-read file's bytes for the run and sends all of them to every worker as a JSON
byte array (about 3.5x the size); a large diff file costs that per worker. (2) A read error crosses to the worker as
its text only (kind `Other`); every reader prints only the text, so no output moves. (3) `policy check` now reads its
request in the multi-batch parent too, so its `inputs.request_read` span and `request_inputs` census line appear in the
parent's trace as well as the worker's. (4) In a batch, `policy check`'s policy source label now comes from its own
workspace spelling, not the first job's spelling of that workspace (as standalone).

## Final round 3 fixes (F1, F2)

Commit `76ef00cb` on `d7ef7a79` ("#277 final r3: F1 (implicit policy read), F2 (non-UTF-8 plan-file paths)"). Gates:
`ci-steps task-affected` exit 0 (4305 passed, 8 skipped); `check-goldens` exit 0 (1240 passed, 4 skipped), no golden
moved; `check-diff --base a6d94a91` `{"reasons": []}`. Both reproduced red on `d7ef7a79` (sol's manifest: exit 0, want
3; the non-UTF-8 run: exit 101, `a worker input serializes: Error("path contains invalid UTF-8 characters")` at
`alsem.rs:1119`). Neither refuted.

- F1 fixed (sol re-raise of R2-1 / FR-3). `plan_read` and `plan_exists` log every path they are asked into
  `PLAN_TOUCHED` (`alsem.rs:1559-1590`); the parent clears it before each `plan` and appends it to that job's
  `JobPaths::reads` after (`alsem.rs:1057-1062`), so a failed plan keeps every file its planning read or probed (an
  auto-detected `WS/al-sem.policy.yaml`, the `.yml` probe), and a `Ready` plan states them too (a superset of the
  workspace entry that already covered them). `pin_changed`'s `--changed` probe stays outside (FR-3's rule: an absent
  value taken for a routine is not a read; `batch_changed_is_decided_once` still passes). Spec D2.1/D2.5 and CHANGELOG
  amended.
- F2 fixed. `PlanFiles`'s two maps serialize through `os_keys` (`worker.rs:44-94`): `[raw path, value]` pairs, the
  path as bytes on unix (`OsStrExt::as_bytes`/`OsStringExt::from_vec`) and UTF-16 units on Windows
  (`encode_wide`/`from_wide`), never a string, so `serde_json::to_vec(&input)` cannot fail on a path. Spec D2.5 and
  CHANGELOG amended.

| test | break (Edit, applied once, `grep -c` = 1) | fail | pass |
|---|---|---|---|
| F1 `batch_failed_plan_keeps_its_implicit_reads` (`cli_batch_writes.rs:920`) | `.append(&mut PLAN_TOUCHED…)` → `.append(&mut Vec::new())` | exit 0, want 3 | reverted, passed |
| F2 `batch_plan_file_under_a_non_utf8_path` (unix, `cli_batch_workers.rs:555`) | `#[serde(with = "os_keys")]` removed from `reads` | exit 101, "a worker input serializes: … invalid UTF-8" | reverted, passed |

Notes: the F2 test pins `reads` only; `exists` uses the same `os_keys` path but no test feeds it a non-UTF-8 key
(auto-detect probes join a workspace path, UTF-8 in the test). FR anchors touched: FR-2/FR-3 parent planning loop
(`alsem.rs:1048-1070`), FR-3 `PlanFiles`/`WorkerInput` (`worker.rs:35-94`).

Risks: (1) a non-UTF-8 manifest DIRECTORY still panics, pre-existing and out of this scope: `JobStreams` and the run's
`JobLine` serialize their paths as strings (`batch/mod.rs:89-109`), and `JobLine` is the run's JSON stdout, which
needs a string. (2) A `Ready` job's probed paths are now reads too: a manifest that writes, from another job, an
absent `WS/al-sem.policy.yml` was already refused through the workspace entry, so no accepted manifest changes.

## Final panel round 1 (2026-10-24)
- Reviewed dc6df5a1 (diff vs a6d94a91, 169 files): sol (pi gpt-6.1-sol, final-r1-sol.md), Opus subagent (final-r1-opus.md), code-review skill at high (built-in, forked). All three: fix-first. Both reviewers accepted E12-C1..C8.
- Merged into FR-1..FR-14 (`.agent/runs/20261024-100928-5fc9b8/final-r1-merged.md`). All verified red on dc6df5a1, none refuted, fixed in one commit 47a1282f (see "Final round 1 fixes"). Gates on 47a1282f: task-affected 4296 passed, check-goldens 1233 passed, no golden moved, check-diff [].
- Register: 61 entries. Spec anchors re-pointed by content (37 shifted, marks kept); 7 spec entries whose sections were amended (S3 S4 S17 O5 = D2.4/D2.5, S6 = D12, O6 = D10, O9 = admission) and E12-C4 (run.rs, touched by FR-6) reset to unreviewed. Round 2 marks: those 8 plus FR-1..FR-14.
- Draft PR #297. Paused for the operator's restart after this round's fixes.

## Task 12 (2026-10-25, implementer)

Rebase: `git -c core.commentChar=';' rebase origin/master` from 76ef00cb onto b08987fc: 30 commits, no textual
conflict; all 30 subjects identical before and after (diffed). One SEMANTIC conflict: master's #272 added
`tests/cli/cli_implicit_rec_bindings.rs` calling `run_analyze_with_exit(&inputs, &args, v)` and destructuring 3
fields; #277's signature takes `&RequestInputs` and returns 4. Fixed in 84aee761 (pass `args.read_request()`,
`Ok((out, ..))`), as every other caller does.

Cherry-pick 1205ba32 → a7a98d07 (pins `ALCH_DRIVER_VERSION_OVERRIDE=2.0.3` in `standalone_is_a_batch_of_one`).
Its `std::env::set_var` raced: at crate version 9.9.9, `cli_batch_contract::batch_equals_standalone` (no env
guard, same process under `cargo test`) failed — its standalone SARIF said `driver.version` 2.0.3, its batch
9.9.9. Fixed in a994c9d4: the pin goes to each child (`batch_in` env; the standalone via `spawn_trace_events`).

Version audit of `tests/gate-goldens/batch/**` (grep `2.0.3`, `*Fingerprint`, `*Version`): every version-dependent
field is in `one-job/` (pinned) or is `merged-streams/`'s `alsemVersion` (normalized; its analyze carries no
other); `update-baseline/` has none. Run at 9.9.9 with the pin: `cli_batch_*` 59/59,
`cli_update_baseline_write` 2/2, `cli_batch_merged_streams` 1/1 pass. Nothing else needed pinning.

| proof | break | result | revert |
|---|---|---|---|
| version pin (`standalone_is_a_batch_of_one`) | Cargo.toml `version` 2.0.3 → 9.9.9, pin kept | pass (and all 59 `cli_batch_*`) | — |
| same | pin env renamed `PROOF_NO_PIN` (pin removed), 9.9.9 | FAIL `analyze: stdout`. Fields differing at 9.9.9 vs goldens, per job: `alsemVersion` (analyze, digest, prove, fingerprint, diff, query), `al_sem_version` (events, policy), graph meta `engine`, `workspaceFingerprint` (digest, prove, fingerprint) | pin restored, passed; Cargo.toml and Cargo.lock reverted with Edit, `git diff Cargo.toml Cargo.lock` empty |
| `worker_count_measured_case_b_rows` (admission.rs) | `("fingerprint", X)` back to `POLICY_X` | FAIL `case B` left 2 right 4 | reverted, 7/7 passed |

KIND_PEAK_KIB (617e3ea8): rows from Part 4b / `/work/bench/160-12/final/table.txt` (steps match): analyze X 13,
digest X 15, policy X 39, fingerprint X 13, prove S 3, events chains X 13 / S 3, query effects X 13 / S 3; others
unchanged. "No row yet" line removed. W: 4 without policy, 2 with (T17 committed tests unchanged and green).

Docs (de27ff0b): spec 12A-19 + bounds rows (B1a, B1b, B2u, B4 walls reported); CHANGELOG table, limits, pin.
Gates: check-goldens 1251 passed (no golden moved); check-diff `{"reasons": []}`; task-affected exit 0, 4317 passed, 8 skipped. HEAD de27ff0b.

- 2026-10-25 07:55 UTC: #304 (22c2d672, operator-approved: Linux release builds on ubuntu-22.04) resolved the glibc 2.34 floor issue outside the flow. The issue was `pidfd_spawnp`@GLIBC_2.39, pulled in by the worker spawn. Its discovery entry is removed. Rebased onto 22c2d672 cleanly: every subject is intact, the tree equals de27ff0b outside `.github/`, and every register anchor still hashes. New head a2d9fef5, pushed with --force-with-lease. Final round 5 is running on de27ff0b, and the four gates on a2d9fef5.
- Final round 5: sol says ship, with no findings. Opus says ship, with one minor docs-only finding: R5-1, the B4 wall count was 14/24, not 13/24. It was fixed in 33a820bc (spec) and 57214164 (numbers file), and closed by raiser-only acceptance. The panel converged: all 69 register entries are accepted by both reviewers.
- Part 4 on a2d9fef5 (release-fast, N=22, Q=5): B5 W4 sum 11.6 GB and W4u 14.1 GB, both under 28 GiB, pass. B5p W2p ran w=2 with two-largest sum 16.5 GB, pass. B7 61.98 s against 124.92 s, pass. B6 is 1.52× W1q against 1.25×, a miss that is only reported (12A-18). F9, F10 and F17 did not fire.

## Gate results
B = 22c2d67211a6a542c9847fe74df11bf6f5742332 (origin/master, #304). H = a2d9fef55d14c2cfc6e10303c2b1c78ac1a4322b. The gates ran through `agentflow run` on H: check-goldens-coverage 0, ci-steps-all 0, check-goldens 0, cdo-gate 0. After H only docs changed (33a820bc, 57214164) plus the evidence commit.
