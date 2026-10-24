# Issue #160 ledger: attempt 10 on server1 (epic step 9: prove and fingerprint cross-app)

## Attempt 10: claim

```json
{"attempt":10,"body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","branch":"issue/160-alsem-batch-build-the-model-onc-a10","issue":160,"run_id":"20261024-093848-0d842b","session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","title":"alsem batch: build the model once and run several commands against it","worktree":"/work/al-sem-issue-160-a10"}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`.
Base `b028ec325c674c6746384a552e5781b7ef2f3047` (master with step 6 merged, #274).
Pre-draft `/work/bench/160-9/draft-step9-design.md` (refreshed against step 6 head `581f3397`, re-cited here
against `b028ec32`). Operator decisions taken before the claim: `/work/bench/160-9/OPERATOR-DECISIONS-2026-10-24.md`.
`findings.json` is carried unchanged (88 entries, all accepted by both, from step 6); this step's entries take
round names `9 r1`, `9 r2`, and so on.

### Attempt 10: classification

**architectural**: a SPEED step with memory risk (epic row 9, §5.12). It changes the `View`'s tree lifecycle
(a kept tree), moves prove and two fingerprint forms onto the cross-app view, and adds a flag to two commands.

### Attempt 10: operator decisions (already decided; written into the spec as decided)

- OD-1 outcome **S′** (probes 2026-10-24, `/work/bench/160-9/probe-out/OD1-RESULT.md`): fold ratio 97.65 %, B0
  cross-app OOM-killed at ≥ 29.1 GB in the shared 32 GiB cgroup. Step 9 ships prove, the fingerprint query
  (`--routine`) and `--inventory-only` cross-app. Fingerprint B0 and `diff` cross-app are a follow-up (Discoveries).
- OD-2 yes (prove falls back to digest's B2′ bound). OD-3 accepted (rule B in prove and the query).
  OD-4: a bench round is void only when the external load exceeds 8. Memory ceiling: the shared 32 GiB cgroup.

### Attempt 10: assumption probes

Base binary: `cargo build --profile cdo --bin alsem` in the worktree at `b028ec32` (correctness profile; no wall
time from it is quoted). Probe files: `/work/bench/160-9/a10-probe/`. CDO pinned workspace, read-only.

| probe | result | evidence |
|---|---|---|
| P0 drift | 22 files moved between the pre-draft's `581f3397` and `b028ec32`; every spec cite re-read at `b028ec32`. Then rebased onto `317d3b34` (#278, grammar, no cited file) | `git diff --stat 581f3397 b028ec32 -- src tests`; `git diff --name-only b028ec32 317d3b34` |
| P1 diagnostics | CDO digest `d`, `h`: 0 envelope diagnostics in both universes (move 1 latent on CDO) | `p14-{d,h}-{x,s}.sum` |
| P4 caps | cross-app max visited `d` 19,284, `h` 19,201 of 25,000; 0 truncated; single-app max 354 / 366 | `p14-*.sum` |
| P4 peaks | cross-app digest `d` 3,137,152 KiB, `h` 3,216,736 KiB; single-app 743,192 / 736,700 KiB | `p14-*.err` |
| P15 prove == digest single-app | 18 of 18 non-dummy cli-b prove cases: the golden's evidence equals digest `--single-app --routine R`'s effects with the same factIds; 0 unresolved both (limit: cited effects only; no gap case) | `p15.txt` |
| P5 dependency root rows | static: no role filter in `derive_root_classifications`; CDO not run (T6 hand-states, F10) | `snapshot.rs:2866` |
| P7 no-dep r0 dirs | 201 of 216 (12 `.alpackages`, 13 app.json dependencies, 9 no app.json) | shell count |
| P8 roots config | only `ws-d51-jobqueue` | `ls` |
| P9 counters | prove and `FINGERPRINT_ROW` bcg 2/cone 2/scc 3/pd 2; `DIGEST_ROW` bcg 1/cone 1/scc 2/pd 1 | `cli_build_counters.rs:378-501` |
| P13 fail-closed shapes | holds at the base | `snapshot.rs:2599-2600,2636-2637`; `unresolved_cone.rs:152-154` |
| P16 routine_to_stable | the cut base uses `routine_to_stable_of(ws)` | `capability_cone.rs:4555,4647` |
| P17 permissions | `derive_permissions` reads `base.raw` (every primary routine's raw facts): inventory's tree path passes none | `snapshot_full.rs:912-951` |
| P18 cli-b prove fixtures | all have an `app.json`, so the cross-app default builds | case list |
| P5 (re-run, static, after 9 r1) | the cross-app model DOES classify dependency routines: `classify_roots` runs over the whole cross-app `ModelEntities` with no role filter | `root_classification.rs:290`; `workspace.rs:1920-1921` |
| P19 tree size (after 9 r1, `sonnet` agent) | CDO digest `d` tree census buckets: 296,115,964 B (282.4 MiB) cross-app (`pd_solution` 201,311,977 B exact), 14,488,230 B single-app; `--tests` same tree. Peaks 3,137,052 / 732,472 / 3,169,488 KiB | `/work/bench/160-9/a10-probe/census-{d-x,d-s,dt-x}.err` |

### Attempt 10: timeline

- 2026-10-24 09:40 UTC: claim read, worktree at base `b028ec32`, ledger section started.
- 09:44: `cargo build --profile cdo --bin alsem` at the base, rc 0. 09:46-09:53: P15 (cli-b), P1/P4 (CDO digest
  `d`, `h`, both universes; peak ≤ 3.3 GB; cgroup anon ≈ 8.5 GB before).
- ~10:20: spec `cbbc7a79`, plan `f9c33f8e`; rebased onto `317d3b34` (origin/master moved, #278); spec note
  `8fe927a6`. `check-diff --base origin/master --head HEAD`: `reasons: []`. Not pushed. Next: spec panel round
  `9 r1` (issue step 5).
- ~10:55: spec panel `9 r1` back (sol: 1 critical, 12 important, 2 minor; Opus: 11 important, 10 minor), both
  fix-first. 30 register entries (6 merged across sources), all `fixed`, reviews `unreviewed`. P5 re-run
  (static) and P19 (tree census, `sonnet`). Design change: every tree is read inside `prepare` before `det`
  (no kept tree). Rebased onto `1134bed1` (#237); fix commit `50eed0c7`; check-diff `reasons: []`. Next: round
  `9 r2` (diff-only briefing).
- round `9 r2` back, both fix-first. Marks: Opus accepted all 30; sol accepted 28, re-raised S9-2 (terminal
  evidence) and O9-4 (no app.json fails earlier, at the model-instance id: no change). 6 new entries (S9-16+O9-22,
  S9-17..S9-20, O9-23). Fix commit `eb754df0` (self-contained query result, five inventory keys, u16 release
  mask, F8/T24, F2 globals, D9's source-less set per caller); marks reset on the entries whose fix text moved:
  S9-2, O9-4, O9-8, S9-10+O9-15, S9-12+O9-11, O9-3. check-diff `reasons: []` (master still `1134bed1`).
  Register 124 entries. Next: round `9 r3`.
- round `9 r3` back: Opus accepted all 12; sol accepted 11, re-raised S9-2 (trigger and event boundaries). New
  O9-24 (boundary per path, empty paths). One fix `34e2f3a1`: per-path `PathBoundary` (cs:/op:/ev:), the fact's
  evidence = first path's boundary else today's operation evidence; T25 five arms, breaks B1-B5. Marks reset:
  S9-2, S9-16+O9-22 (its anchor text changed). check-diff `reasons: []` (master `1134bed1`). Register 125.
  Next: round `9 r4` (the last pre-approved spec round).
- round `9 r4`: Opus accepted S9-2, S9-16+O9-22, O9-24 (VERDICT converge) and raised O9-25 (minor: query contract
  version). sol rate-limited: its marks on those 3 stay `unreviewed` for the coordinator. Fix `888e69f2` (docs
  only): `FINGERPRINT_QUERY_CONTRACT_VERSION` 1.2.0 → 1.3.0, stated golden move, T25 pin, CHANGELOG step.
  check-diff `reasons: []` (master `1134bed1`). Register 126.
- ~12:05: impl Tasks 0-1. Task 0: base `1134bed1` release-fast `alsem` at `/work/bench/160-9/base-bin/alsem`
  (sha256 `00856d74…085fc5`, `bins.sha`); sweep `/work/bench/160-9/sweep/sweep.sh`, base outputs
  `/work/bench/160-9/sweep/base` (217 dirs, 798 roots, 19 cli-b cases, 8,235 cases; rc 0 except the cli-b dummy-doc
  exit 2). Task 1 `ac447e3c`: red first (compile) for 1.2 and T20; task-affected 4058/4058, check-goldens rc 0 (no
  move), 1.6 green unchanged, `ci-steps task` rc 0 (45.8 s), check-diff `reasons: []` (master `1134bed1`).
- ~13:05: Task 2 `a95b95d0` (prove on the cross-app view). cli-b prove differential green with no regen;
  task-affected 4071/4071; check-goldens rc 0 (no existing golden moved; 2 new T8 goldens under
  `tests/cli-b-goldens/single-app/`, minted by the base binary, path-independent); `ci-steps task` rc 0 (43.5 s);
  check-diff `reasons: []` (master `1134bed1`). T23 prove half: base default == head `--single-app`, 19,266 files
  (6,422 cases: 798 roots × 4 questions × json/human + 19 cli-b cases), 0 differ. Head binary for the sweep: the dev
  build (`/work/bench/160-9/head-dev/alsem`) because the coordinator killed the release-fast build (timing hold);
  byte identity does not depend on the profile. Task 1 review items folded in (TreeRead in `view.live`; T20 pins an
  unfilled TreeRead is not released). T2 is a lib test with a hand-stated body-less source row (no AL source
  produces one: a symbols-only member of a source app has no model row, `external`; an interface or
  declaration-only member is never a resolved target).
- ~14:20: Task 3 `6f7a24ac` (fingerprint `--routine` on the cross-app view), plan 3.1-3.7 and the 3.9 sweep; 3.8
  (CDO early read) NOT run: coordinator hold (no release builds, no CDO). Golden triage: 26 cli-b fingerprint
  goldens moved, 25 by the `schemaVersion` 1.2.0 -> 1.3.0 line only, the ws-txn-d49-pos-modify-runmodal json by
  that line + `mayCommit.presence` no -> unknown (ModifyThenRunModal inheritedStatus partial, object-run-unresolved,
  inherited facts shown) and its human file by one `commit:     unknown` line; golden-diff-triager (sonnet): all
  allowed; receipt `.agent/golden-triage.md`; regen == the triaged dump (26 files). No B0/snapshot golden moved.
  T8 query goldens minted from the base binary (only the stated `schemaVersion` line differs from head
  `--single-app`). T23 `--routine` half (dev head binary, `/work/bench/160-9/head-dev/alsem-t3`): 4,788 fq files,
  3,975 identical, 783 differ by the version line only, 30 by `mayCommit` (15 json, each `no` -> `unknown` on a block
  whose inheritedStatus is not complete, + their 15 human twins), 0 other; prove half still 19,266/0; inventory
  651/0. Gates: task-affected 4087/4087, check-goldens rc 0, `ci-steps task` rc 0 (44.6 s).
- ~14:35: origin/master moved to `70c3b743` (#264). Rebased (clean, autostash); the #264 `run_prove_pipeline` call
  site needed `single_app`: fix-forward `ca542dde`. Rebased SHAs: Task 1 `bd007ed3`, Task 2 `96d9843a`, Task 3
  `fedc1d2a`. check-goldens rc 0 (1151/1151), `ci-steps task` rc 0 (133.7 s), check-diff `reasons: []`. Base check:
  a dev binary of `70c3b743` swept the corpus: 8,235/8,235 outputs equal `1134bed1`'s, so the sweep base and the T8
  goldens stand. The rebased head (`ca542dde`, dev) re-swept `--single-app`: 24,705/24,705 files equal the
  pre-rebase head's, so the T23 results above hold at `ca542dde`. Stopped before plan 3.8 (CDO early read) per the
  coordinator's hold.
- ~15:30: Task 2 review items (ship): (4) prove's zero-root branch now releases `L4`/`Effects` before exit 2, as
  digest does; fix `dd8cb6e5` with `prove_with_no_root_releases_as_digest_does` (red before the fix). The Task 2
  commit's "release sequence equals digest's" claim held only for a resolved root before `dd8cb6e5`; it holds for
  both now. (5) the `scope` comment (same commit). (1) sweep coverage: `/work/bench/160-9/sweep/sweep-q.sh` adds, per
  root, `writes-table:<t>` / `publishes-event:<e>` for every table the root writes and every event it publishes in
  the base `--routine` output (name = the display, or the id's last segment when the display is the id): 281 cases
  (219 tables, 62 events), answers 261 yes / 20 no; base default == head `--single-app` (dev `ca542dde`) on all
  1,686 files. (2) T2-B1 reaching the claim and (3) prove T8 corrupt break: rows below.
- ~16:10: Task 4 `90b92578` (inventory-only on a tree with no raw facts), dev profile only (hold). T18: cli-b B0 /
  snapshot / diff differentials and `cli_snapshot_schema4` green with no regen after the split. Task 3 review items
  folded in: `PathBoundary.kind` is an enum; the dependency-only `--routine` test; the `--single-app` help text naming
  `--inventory-only` is now true. T8 inventory golden minted from the base binary (path-independent). T23: the dev
  head (`/work/bench/160-9/head-dev/alsem-t4`) `--single-app` sweep equals the Task 3 head on all 24,705 files and the
  base on all 651 inventory files. Gates: task-affected 4184/4184, check-goldens rc 0 (1158/1158, no golden moved),
  `ci-steps task` rc 0 (106.5 s), check-diff `reasons: []` (master `70c3b743`). Not done: 3.8 (CDO), any release
  build.
- ~17:00: plan 3.8, the CDO early memory read (hold lifted for memory only; a `sonnet` runner, shared slot, one
  process at a time, release-fast head `90b92578` built under its own CARGO_TARGET_DIR, sha256 `c91e893c…d0e0a`,
  `/work/bench/160-9/head-bin/`; R = `IssueReminder`, the `d` root with the most visited routines (19,284), stable id
  in `/work/bench/160-9/prove-routine.txt`; every wall time void: contended). Bound 1.08 × P_a = 3,411,638 KiB
  (P_a 3,158,924 KiB, step 6 numbers file):

  | run | peak KiB | output | note |
  |---|---|---|---|
  | head `--routine R` json, cross-app | 3,718,888 | 307,264,898 B (1 block, totalClassifications 3908) | **MISS** (+9.0 %) |
  | same, `ALSEM_RETAINED_CENSUS=1` | 3,712,156 | same | `view.output cells= rel_strong=1 tree=0`; `witness.non_primary_members=0` |
  | head `--single-app` json | 751,468 | 2,201,845 B | == base except the `schemaVersion` line |
  | base `1134bed1` default (single-app) json | 746,048 | 2,201,845 B | |
  | head cross-app, `--format human` | 3,161,736 | 40,401,340 B | under the bound |
  | head cross-app, json `--witness false` | 3,170,732 | 6,865,332 B | under the bound |

  Reading: F1 is clean (nothing alive at output). The tree read and the query result fit (human and witness-free
  runs are under the bound); the ~550 MB over the bound is the json projection of a 307 MB document
  (`project_fingerprint_query_full` builds one `serde_json::Value` of the whole document beside the result, then
  the string). F2 trims the tree-read side (`fingerprint_query`'s indexes), so it cannot close this miss; F3 would
  ship `--routine` single-app. NOT applied: reported to the coordinator for a decision (a projection-side fix is
  not a pre-stated fallback).
- ~17:20: Task 5 `1726e678` (D9) + Task 4 review items; master moved to `f40087d5` (#265): rebased clean. Rebased
  SHAs: T1 `e0d82222`, T2 `b3c4d5ca`, T3 `88b1e7de`, #264 fix `c7c1fd1d`, T2 review `a2d2adc7`, T4 `b2e551c6`, T5
  `402745a8`. D9 deviation: `cli_snapshot_schema4::opaque_gap_of_an_app_with_source_fires` hand-states a source app
  to force F21's assertion; D9 removes its precondition by construction, so it is rewritten as
  `..._is_per_routine` (spec said that file stays unchanged; this one test cannot). Task 4 review: T18 now exists
  (goldens minted by the BASE code, `70c3b743`, via a scratch test in the base worktree, removed after; DD cross-app
  and single-app, R5B cross-app; path-independent); a cross-app inventory content golden on the two-`OnRun`
  fixture, triaged by hand: rootClassifications / coverage / routineInventory = the 5 primary routines only (the
  classified dependency `Dep Runner.OnRun` absent); dependency identities = exactly those primary rows reference
  (`Dep Lib`, `OnDepEvent`, `Outer` via RaiseDep / HandleInWs / CallChain; `Dep Runner` + its `OnRun` via
  `Ws Runner.OnRun`'s `Codeunit.Run`; table 18 via TouchCust; `Inner`, `Dep Sub`, `HandleDep` absent); apps = primary
  + XBase; the stale doc fixed. Gates on `402745a8`: check-goldens rc 0 (1163/1163, no golden moved), `ci-steps
  task` rc 0 (85.2 s), check-diff `reasons: []` (master `f40087d5`).
- ~18:10: **the B1q fix** `726d355b` (coordinator's decision: fix the root cause, not F3): the `--routine` json
  projection streams (`Doc`: a normalized small value, a sorted null-dropping map, or a sequence built element by
  element, serialized with serde_json's pretty formatter); no `serde_json::Value` of the whole document. Not a design
  change (no spec round). Bytes: cli-b fingerprint goldens green with no regen; T23 sweep (dev
  `/work/bench/160-9/head-dev/alsem-stream`, `--single-app`) == the Task 4 head on all 24,705 files; CDO
  cross-app `fingerprint --routine R --format json`: 307,264,898 B, `cmp`-identical to `90b92578`'s. **B1q before
  / after: 3,718,888 KiB (MISS) -> 3,158,988 KiB (pass; bound 3,411,638, 7.4 % under)** (release-fast `726d355b`,
  sha256 `d6a60232…f0d33add`, one process, wall void: contended). Change: 322 insertions / 109 deletions in
  `fingerprint_query.rs`, ~100 of them tests. Stated limit: the projection's USE of `Doc` (facts as a streamed
  sequence) is pinned by the CDO peak, not by a unit test; the unit tests pin `Doc`'s bytes and its streaming.
- ~18:30: Task 6 `16a48166` (CHANGELOG `Changed` and `Fixed`, `compose_snapshot`'s readers, 9A-6's label; Task 3
  review item 4: the `--witness 0` / no-path evidence is the fact's own operation, stated as intended). `ci-steps
  task` rc 0 (70.3 s), check-diff `reasons: []` (master `f40087d5`). Stopped before Task 7 (its bench waits for
  #289).
- ~19:20: **final panel round 1** `7ca8f996` (sol: 2 important; Opus: ship, 7 minor; code review: 8). Register
  round "9 final r1", F9-S1..S2, F9-O1..O7, F9-C1..C8: 16 fixed, F9-C4 refuted (`--strict` needs the run's
  diagnostics, and the run reads the one composition the tree reads; stated in `run_routine_query`'s doc).
  Duplicates merged: O2 = S1, O1 = C1, O5 with C3. Proofs: 7 rows under "Discrimination proofs". Gates on
  `7ca8f996`: `ci-steps task` rc 0 (124.3 s), check-goldens rc 0 (no golden moved), lib + cli 3218/3218,
  check-diff `reasons: []` (master `f40087d5`). Byte identity against `726d355b` (dev `/work/bench/160-9/r1/
  alsem-r1-dev` vs release-fast `alsem-726d`): T23 sweep default 0 / 24,705 differ, `--single-app` 0 / 24,705;
  question sweep 0 / 1,687 each. CDO cross-app, release-fast `7ca8f996` (sha256 `e528347e…a2265f`) vs `726d355b`,
  one process each: `fingerprint --routine R --format json` 307,264,898 B `cmp`-identical to `fq-x2.json`;
  prove json and human, `--inventory-only` (32,147,319 B) and a failing selector (exit 2, 792 B) all
  identical. **B1q peak re-read: 3,162,648 KiB** (bound 3,411,638; was 3,158,988 at `726d355b`; within noise,
  so O3's earlier view drop does not move the peak, which is not at projection time). The failing-selector run
  peaks at 3,153,632 KiB with no tree against 3,153,376 with one: the peak is the model and det build, not the
  tree, so O4 saves time, not peak. Wall times void (contended).
- ~19:45: final panel round 2 converged (all 17 accepted by both, ship). Master moved to `b2c73d4f` (#271) after
  base `f40087d5`; not rebased (coordinator rebases).
- ~21:10: **Task 7** on `7ca8f996` vs parent `f40087d5` (release-fast, clean detached worktrees
  `/work/bench/160-9/final/{parent,head}-wt`; alsem sha256 `53f0dadb…` / `59dfccbb…`), one process at a time:
  - Part 2 bounds (median of 3, KiB): D parent `a` 3,150,128 / head 3,162,204 (+0.38 %, pass); B2 `pv` 3,160,284,
    B1q `fq` 3,164,904, B1i `fi` 3,165,108 (limit 3,244,631, pass); B1s `fp` 1,466,052 / 1,472,272 (pass); B3s
    `df` 1,628,228 / 1,478,352 (pass, −9.2 %, not attributed); ceiling max 3,189,692. F1-F13: none fired (F6 not
    needed). Walls void under OD-4 in all 3 rounds (external load 11.94 / 120.60 / 103.36) and affected by #289.
  - Part 1: histogram identical; analyze SARIF 4/4 identical; single-app `pv` (json, human), `fi` identical, `fq`
    only `schemaVersion`; `fp` json, cbor.gz, `df` identical. Census: `witness.non_primary_members=0` and
    `view.output cells= rel_strong=1 tree=0` for `pv`, `fq`, `fi`.
  - Triage (spec §3): `pv` yes → yes (evidence a `Base Application.OnRun` COMMIT group); `fq` 0 of 80 primary
    fact keys lost (2 inherited facts' `via` call → object-run); `fi` 0 rows lost, apps 1 → 9, +742 dependency
    identities, coverage moves explained (392 direct partial → complete were `external-target` only; 2
    inherited complete → partial enter dependency `interface-open-world`). No defect.
  - 7.5 T23 sweep (parent default vs head `--single-app`): 813 / 24,705 differ, all `fq` (798 `schemaVersion`,
    15 `mayCommit` no → unknown with inherited `partial`); question sweep 0 / 1,687.
  - 7.6 `cdo-gate` (executor, `bash scripts/cdo-gate`): rc 0, 2,721 passed, 469.5 s.
  - Numbers file `docs/measurements/2026-10-24-issue-160-step9.md`: `4074f171` (its gate line was a placeholder by
    a failed Edit) and `ded71251` (fixes that line; no amend). check-diff `reasons: []` against base `f40087d5`;
    against `origin/master` it lists only #271's `.agent/issue-271/*` files, which master gained after the base.

# Issue #160 ledger: attempt 7 on server1 (epic step 6: one cone graph, one scoped witness pass)

Attempt 9 (carried over, 2026-10-24 07:27 UTC): run `20261024-072718-f20fa1`, branch
`issue/160-alsem-batch-build-the-model-onc-a9`, worktree `/work/al-sem-issue-160-a9`, base `9bd2996f` (make-worktree,
origin/master after #250/#252). Operator ran `unblock 160` after attempt 8 hit the 10 h cap. The a8 commits
`678d6540..32d01fae` were cherry-picked (CHANGELOG conflicts taken from master, then the #160 entries re-added in
`2af87561`; the code delta a8→a9 equals master's 678d6540→9bd2996f delta outside CHANGELOG/.agent). check-diff
`reasons: []`.
Attempt 9 final panel: r1 (sol, Opus, code-review skill at high) 18 findings → 16 register entries (F1-*), all fixed or
refuted with evidence; rebased onto `83ba322a` (#239: one conflict in `selector_index.rs` and the `fingerprint_cli.rs`
call site, resolved) and `830a150c` (#241, clean); r2: Opus accepted all 16, sol accepted 15 and re-raised F1-O5
(provenance named a stale base), fixed in `056295ad` (docs only); r3: both accepted F1-O5, NEW none, both VERDICT
ship. Register: 88 entries, every one accepted by both (R3-01/R3-02 rule marks re-pointed to the rebased
`cad084a7`). Executor gates on H = `056295ad`: ci-steps-all rc 0 (333 s), check-goldens-coverage rc 0,
check-goldens rc 0, cdo-gate rc 0 (274 s). Operator decisions this attempt: bench void rule = external load > 8
only (step 6's final-bench rounds count); 24 h wall-clock cap (`83ba322a`).

## Claim

```json
{"run_id":"20261023-000639-b5e936","issue":160,"attempt":7,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/160-alsem-batch-build-the-model-onc-a7","worktree":"/work/al-sem-issue-160-a7","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","title":"alsem batch: build the model once and run several commands against it","reconciled":[]}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`.
Base `459294f18703b06a371d351bd95871e6f2314e99` (make-worktree, origin/master; step 5's merge, #223).
Operator: goal "Finish #160 as fast as possible with as few interactions as possible; merge is allowed" (2026-10-22).
Speed decisions (2026-10-22): step 8 runs as #220 in a second session and **must merge before this step's
implementation** (both edit `src/engine/l5/snapshot.rs`); step 13b deferred to #222; identity/comparison runs only at
stage boundaries and the final head. Reviewers: gpt-6.1-sol (slot astra) and Opus (slot flash); attest with
`--substitute astra=sol --substitute flash=opus`. Pre-draft: `/work/bench/160-6/draft-step6-design.md`, probes
`/work/bench/160-6/probes-todo.md` (static half pre-run on master 455ac2d1). Inputs from step 5 and 11b discoveries:
#215-#217 (per-row cone/solve, guard frames, twin-node fan-out), #227 (dependency L4 solve, D4), #228 (one cross-app
producer, D5). The previous step's ledger (5) stays in git history at `459294f1`.

## Classification

**architectural**: the step adds a `View` cell (`caps`), changes the cell order (one composition before `det`,
effects before the runs), deletes the cut graph, `ConeOutput`, `RAW_INHERITED_FACTS` and policy's RAW build, and is a
SPEED step with memory risk (epic row 6, §5.12).

## Assumption probes

Base `459294f1`, release-fast binaries `/work/bench/160-6/parent/` (`bins.sha`), CDO `bc3ccb1` clean. Scripts
`/work/bench/160-6/probe-base.sh`, `probe-base2.sh`; output `/work/bench/160-6/probe/`. Run through
`agentflow run --name bench`. Load gate passed at 00:18, but a spike to 1-min load 78 at 00:22 hit round 3 of P7.

| probe | result | evidence |
|---|---|---|
| A6.4 feasibility (pre-code) | **FALSIFIED: `o` cannot stay ≤ parent + band.** `o` builds no det cone today (counters `cone 1` = the cut snapshot's); the head must compose over the full cone graph. One full-graph composition costs 6,136 ms (`context.capability_cones`, default analyze; pd_solve 2,788 ms, fact_cone 1,543 ms, singleton 879 ms) against the cut's `r3a3.compose_cone` 677 ms (pd_solve 74 ms). Net ≥ +5 s on a 17.4 s median before any raw walk or extra ordering roots | `probe/p34-an.err`, `p34-an-spans.txt`, `p7-o-spans.txt`, `p34-o.err` |
| P7 void check (B ≤ 5 % P) | **FALSIFIED at the base under today's load**: `d` median 21.6 s, band 2.1 s (9.8 %; base2 band 20.1 s with the spike); `o` 17.4 s, band 12.3 s (spike round), 1.4 s without it (8 %) | `probe/p7-summary.txt`, `p7/runs.jsonl` |
| P0 drift | falsified as expected: `capability_cone.rs` (+188 −390), `detector_context.rs` (+134 −155), `snapshot.rs` (+164 −20), `exec_relation.rs`, `cone_derived.rs`, `digest_cli.rs`, `session.rs`, `policy_engine.rs` changed since `5c42b8f3`; every citation re-read at `459294f1` | `git diff --numstat 5c42b8f3 459294f1` |
| Q7 key | the cone is still string-keyed after 11b (`compose_inherited_cones(.., routine_ids: &BTreeSet<String>, ..)`, `capability_cone.rs:2585`): `WitnessScope` keys by string id | source |
| Policy today | **policy check composes raw for EVERY routine on the cone graph, dependency rows included** (`det_cone_inputs` nodes = every model routine, `detector_context.rs:1266`; `Both` at `:1375`): I5 is violated today. Peak 14.33 GB (custom policy), 13.20 GB (bundled); wall 65.7 s / 52.2 s; custom 8,429 findings, bundled 19 | `probe/p10.err`, `p10b.err`, `p10-policy*.json` (SHA `351d6a57…`, `938fa67a…`) |
| P5 roots read identities only | holds: `resolve_changed_roots` (`digest_cli.rs:372`) passes `snap` only to `build_selector_indexes` (`:386`), which reads `snap.identities.stable_ids` alone (`selector_index.rs:84`) | source grep |
| P9 counters | hold (unchanged by 11b and 5): analyze default `1/1/1/1/2/1/1/0` (`program/bcg/cone/v2/scc/pd/ctx/snap`), d47 `1/2/1/0/2/1/1/1`, digest and prove `1/2/2/1/3/2/1/1`, policy `1/1/2/1/3/2/2/0`, fingerprint `1/2/2/1/3/2/1/1`, query `1/1/0/1/1/0/0/0` | `tests/cli/cli_build_counters.rs:192-715` |
| P13 single-app identity (static) | holds: `ExecRelation::single_app` = `new(&ws, &calls.edges, &[], &events.graph)`, cut = `build_combined_graph(&ws, &calls, &events.graph)`; one assembler (`exec_relation.rs:50-93`, `capability_cone.rs:4510`) | source |
| P12 fixture (static) | holds: dep `Outer` → `Inner`, Customer (18) write + `Commit`; ws `CallChain` → `Lib.Outer()` (`tests/common/dep_dispatch_fixture.rs:5-8,21-30,66-70`). Dynamic r3a5 check not run (T1 is the evidence) | source |
| P3 base half | cut snapshot on `d`: `snapshot.r3a3_base` 952 ms (graph 71, clones 22, direct scan 201, compose 650) + `snapshot.derive` 988 ms = 1.94 s of 21.6 s; det cone 6,015 ms | `probe/p7-d-spans.txt` |
| P4 base half | `o`: `context.ordering_facts` 4,970 ms = `ordering.compose_snapshot` 2,100 + `ordering.digest_query` 2,825; roots_all 5,479, roots_kept 1,350, ordering capability facts 150,209; findings d47 2 + d49 6 (cross-app all/primary and single-app equal) | `probe/p7-o-spans.txt`, `p34-o.trace.json`, `p11-ord-*.sarif` |
| P7 peaks | kernel VmHWM: `d` 3,151 / 3,156 / 3,153 MB; `o` 3,145 / 3,166 / 3,160 MB | `probe/p7-maxrss.txt` |
| P14 census (o, cut) | `snapshot.*` ≈ 128 MB shallow (facts 68.5 MB, pd 21.3 MB, order frames 10.7 MB); `arena.link_graph` 13.2 MB; `rel.graph` 26.3 MB; cut pd edge_unmapped 0 | `probe/p14-o.err`, `p34-o.err` |
| Histogram | SHA `c032c8db…` (step 5's) | `probe/hist.json` |
| P1, P2, P3 head, P4 head, P6 dynamic, P8 | not run: each needs the scope type (Task 1). They are the stop rule's inputs (spec §8.4) | — |

## Design

Spec `docs/superpowers/specs/2026-10-23-issue-160-step6-one-cone-graph-design.md` (round 1 text). Amendments 6A-1 to
6A-10; 6A-10 (cost bound for analyze-d47) answers the A6.4 falsification. #215/#216/#217/#227 deferred, #228 kept as
fallback F11. Re-cite every **[#220]** line after #220 merges, before the plan.

## Base/Head SHAs

## Timeline

- Claimed (attempt 7), worktree from `459294f1`.
- Spec round 1 text `dff79c6e`; base probes run (`/work/bench/160-6/probe/`).
- Spec panel round 1 (sol: request changes; opus: 0 critical, 11 important, 15 minor): 33 findings R1-01..R1-33, all
  fixed in `6ae8946a`; register `.agent/issue-160/findings.json`. Q3 decided statically (R1-20).
- Spec panel round 2 (opus 32/33, sol 29/33 accepted): re-raised R1-02, R1-17, R1-24, R1-28 and new R2-01, R2-02
  fixed in `8e121683` (K includes input assembly; T9 on a routine selector; policy peak bound B5 + F13; implicit-trigger
  exemption; parent effects credited in S_P(d); F2 fold-first). Standalone path's `det_cone_inputs` split noted.
- Spec panel round 3 (sol 7/7, opus 6/6 accepted; opus raised 2 minor): R3-01 (F2 adds the map_op-keyed bits first;
  second walk inside the same member loop) and R3-02 (F13's inputs; B5 at 1.03 x) fixed in `c62dab36` (spec only).
- Spec converged in panel round 4 (37 entries accepted). Plan `docs/superpowers/plans/2026-10-23-issue-160-step6-one-cone-graph.md`
  committed `924a23db` (4 tasks; Task 1 ends at the stop-rule STOP; rebase onto post-#220 master first).
- Task 1: branch rebased onto the post-#220 master `7afa91a3` (docs commits replayed; plan now `abba53dc`). Code
  commit `72abd499` (`#160 step 6 Task 1: WitnessScope, the tree builder, the stop-rule probe`), byte-neutral:
  `ci-steps task-affected` 3714/3714 pass, `check-goldens` 951/951 (no move), release clippy clean, check-diff
  `reasons: []`. Spec re-cited on `7afa91a3` in `2a4a867f` (docs only; no design claim changed), check-diff clean.
- Task 1 measurements (parent `7afa91a3`, head `2a4a867f`, 05:27-06:30): S1 and S2 FAIL on both d and o; root cause
  per-fact witness reconstruction over dependency-inherited facts; no fallback applies. **STOP at Task 1** (see
  `## Task 1 STOP`); discovery filed in `## Discoveries`; Task 2 not started.
- Witness redesign (operator: redesign within step 6). Diagnostic (`/work/bench/160-6/w6/diag1.sh`, scratch diag-wt):
  the cost is the per-fact simple-path BFS hitting `MAX_STATES` (4,627/15,983 calls), not path count (cap 3: 306 s CPU
  vs 430 s) nor proofs (3.5 s). Prototype 1 (`w6/proto1.sh`, `ALSEM_W6_DAG`): one node-BFS predecessor DAG per (root,
  first hop), each fact's witnesses = all shortest paths to its nearest carriers (CLRS BFS predecessor subgraph; the
  shared-suffix DAG). d: E′ 148 s → 5.9 s (cap 3: 4.6-4.9 s), peak 4.0 → 6.0 GB; o: E′ 2,096 s → 285 s (proofs
  1,664 CPU-s, ordering engine 3,790 CPU-s), peak 25.2 GB, K 14.6 s + T 25.9 s unchanged. Cut output: digest differs
  (printed via paths 370 → 336, truncated 88 → 84, 2 conditionality `unknown` → `conditional`; types/detail/evidence
  equal); ordering SARIF identical. **No design fits S1/S2 as written**: d S1 ≈ 4.5-5.9 s vs Bd 0.2 s, S2 6.0 vs
  3.22 GB; o S1 ≈ 320 s vs 8.6 s (K − S_P alone 8.6 s = the whole cap), S2 25 GB. Spec and plan NOT amended;
  operator decision needed.
- Attempt 8 (run 20261023-120326-46d656, base `6a2b7c8b`): `6a94aeac` fixes #78's compile break in
  `step6_probe.rs` (`OwnerIndex::build(model)`; `ci-steps task-affected` 3812/3812 pass). Amendment 6A-12 added as
  spec §14 in `4eca8a3b` (the four operator decisions recorded as decided; citations re-checked by line text against
  `6a2b7c8b`, which fixed four approximate digest_cli cites and filled the capability_cone shift table). Plan split in
  `752d032a` (Task 1′, 2a, 2b, 3 conditional, 4). check-diff `reasons: []` on all three. Next: spec panel on §14;
  Task 1′ not started (still owes `check-goldens` + release clippy on the rebased tree).
- Spec panel on §14 (opus 0C/7I/9m, A5 rejected; sol 8I/2m). Memory attribution prototypes in scratch diag-wt
  (`/work/bench/160-6/w6/proto4-7.sh`, outputs `w6/p4`-`p7`): (1) the §14.4/14.6 peaks were taken on `aldump`, which
  runs glibc malloc; production `alsem` runs mimalloc (purge delay 0, `alsem.rs:28`). Under mimalloc: K+T 3.12 GB
  (= parent 3.13), `d` DAG 4.78-4.79 GB (+1.67 GB, not +2.88), single-thread 4.00 GB, cap 3 4.74 GB; E′ 5.5-6.2 s.
  (2) The owner is not the DAGs (4 builds, 76,897 nodes, 95,061 preds for `d`) nor the path count (cap 3 ≈ same).
  (3) Multi-root probe: `CDOUnhandledSalesPagesMgt.Codeunit.al` (88 roots, 334,078 raw facts, 301,356 effect facts):
  E′ **77.0 s**, peak **22.5 GB** (15.5 M paths; proofs 121 CPU-s, ordering 134 CPU-s); `CDOEMail` (65 light roots):
  E′ 1.7 s, 3.11 GB. The digest cost and peak scale with the volume of dependency effects × witnesses per root, i.e.
  with the OUTPUT; no witness-search change fixes it. Spec NOT amended: operator decision on digest's output shape
  needed before the round-1 fixes are written.
- Operator chose option A (boundary witnesses). Prototype 8 (`w6/proto8.sh`, `w6/p8/`, mimalloc, scratch diag-wt:
  DAG paths cut at the first dependency routine, terminal "enters <routine>" at the boundary call site): **worse**.
  `d`: effects 15,983 → 15,981 (effects stay keyed by resource; the boundary only shortens paths: printed hops
  1,230,675 → 447,002), E′ 7.1 s → 28.3-48.5 s, peak 4.77 GB → 28.3-29.0 GB; digest's per-root ordering pass
  (`compute_ordering`, scoped guarantees) went 2.5 → 47.6-84.8 CPU-s. The 88-root file was OOM-killed at 32 GB in all
  3 rounds. Cause: thousands of effects now share one boundary call-site terminal, and the ordering pass grows
  super-linearly in effects per terminal. Spec NOT amended; reported for a decision.
- Operator chose B (grouped boundary effects) with automatic fallback D. Prototype 9 (`w6/proto9.sh`, `w6/p9/`,
  mimalloc): rule B + the ordering engine's happens-before test on demand (a fixable quadratic: all-pairs HB edges
  scanned per query, group ops cloned per occurrence). `d` (3 rounds): 1,020 effects (56 groups), E′ 3.06-3.16 s,
  probe peak 3.10-3.12 GB (= parent), S1′ 1.54 s; `h` (88 roots, 3 rounds): 22,232 effects (862 groups), E′
  21.2-22.0 s, peak 3.10-3.12 GB. d47/d49/d51 SARIF byte-identical (`60708358…`, all/primary/single-app); cut digest
  JSON identical to DAG-only. All operator bounds pass → B branch. Spec + plan `606945e3`, ids made contiguous
  `3f335f1d` (A1-01..A1-24 in the register, 61 entries). check-diff against `--base origin/master` reports
  `protected-path:.agent/issue-236/*` only because master moved to `3a2d5923` (#243 added those files; the branch sits
  on `6a2b7c8b`); against the merge base `6a2b7c8b`: `reasons: []`.
- Spec panel round 2 on the amendment (branch rebased onto `3a2d5923`, HEAD `56fb9e76`): opus accepted A1-01..A1-24;
  sol accepted all but A1-01 and A1-11 (re-raised). Fixed with 9 new findings A2-01..A2-09 in `f6f1c648` (spec +
  plan, docs only): one key_cmp stream with distinct paths (A1-01, A2-06, W4c); O1's discriminating breaks on a
  literal fixture (A1-11); rule B's boundary identity and per-boundary partition (A2-01, B5), the cut wins over a
  primary carrier behind dependency code (A2-02, B6), boundary occurrences never carry must-labels and read
  `unknown` (A2-03, B7), proofs cut at the boundary with their class (A2-04, B8), Task 2b split into 2b-1/2b-2 so D
  keeps Task 3 (A2-05), T8 reads `h` (A2-07), 12 GiB is a single-job ceiling (A2-08), per-fact marking cost cached
  and re-measured (A2-09); P-4 counts `unconditional` → `conditional` flips as their own class (A1-03). No
  measurement needed (margins of §14.4). Register 70 entries. check-diff `--base origin/master` reports
  `protected-path:.claude/commands/issue.md` only because master moved to `37c9daed` (#248); against the merge base
  `3a2d5923`: `reasons: []`.
- 2026-10-23 §14 panel round 3 fixes, `ef45f063` (docs only, spec + plan). Marks written (A1-01 opus re-raised,
  A2-01 sol re-raised, A2-06 both, A2-09 sol re-raised; the rest accepted). One enumeration rule in §14.3.1 item 4:
  a trie walk over projected prefixes (frontier sets, children grouped by hop JSON), one length class at a time,
  key (projected length, projected JSON, end key) with the boundary identity last (A1-01, A2-06, A3-01); partition
  by boundary after the enumeration, proofs on the same key, viaPaths print the primary prefix only (enters hop in
  evidence/detail); cost bounded by distinct candidates (A2-09); B5 is one fact behind `R → Helper` (A2-01); W4b
  fixtures (1)-(4); O1 `d` order id 2 in both docs. Register 71 entries (+A3-01). check-diff `--base origin/master`
  reports `.agent/issue-83/*` only because master moved to `40c84c13` (#83); against the merge base `37c9daed`:
  `reasons: []`.
- 2026-10-23 §14 panel round 4: opus and sol accepted all their ids; sol NEW A4-01 (opaque seeds). Fix `9273e39a`
  (rebased `cbaf387d`). Round 5 (spec_rounds remaining 0): both accepted A4-01, NEW none. **Spec converged**:
  72 entries, every one accepted by both (R3-01/R3-02 accepted-by-rule; their `fix_commit` re-pointed to the rebased
  `5c91c800`). Branch on origin/master `5b8d198f`, check-diff `reasons: []`. Next: Task 1′.
- Task 1′ Steps 1/3 (sonnet): `check-goldens` 996/996 rc 0, `ci-steps clippy` rc 0 on `cbaf387d`; parent bins at
  `5b8d198f` in `/work/bench/160-6/parent5b8/` (alsem `adc09612…`, aldump `565ffb53…`). Step 4 (opus): probe
  `/work/bench/160-6/probe1p/` (alsem `e360e9b5…`, aldump `462d5425…`, patch `split/proto1p.patch`); deviations from
  the spec, all non-cost-bearing: group name `enters <Routine display>`, event-dispatch boundary unanchored, primary =
  guid prefix, final sort/dedupe on raw-hop JSON, no streaming top-k merge, no cut debug_assert, DagCache inside
  HopCache, no fingerprint per-root cache. Marking cache misses 92 % on `h` (278,147 builds / 23,204 hits).
  d47/d49/d51 SARIF byte-identical to the parent at all scopes and `--single-app`.
- **Operator decision (2026-10-23 ~17:10 UTC):** CentralGauge's v2.0.2 bench holds the CPU (load ticks > 100; RAM
  free). Task 1′'s time bounds S1′/S3′ are read on process CPU time (user+sys, `maxrss.py cpu_s`), design minus the
  parent-shape arm, 3 rounds, no load gate (`proto1p.sh cpu`, `w6/p1p-cpu/`). Peaks are read as measured. The
  wall-time bounds B1′/B6 still decide at Task 4's final bench on a quiet machine.
- Task 1′ Step 5 (sonnet runner; `w6/p1p/`, `w6/p1p-cpu/`; slot rc 0, goldens rc 0, cpu rc 0):
  - P-1 design medians, `d`: K 6,673 / T 2,239 / E′ 2,750 ms, peak 3,128,272 KiB (rounds' max load tick ≤ 6.3,
    valid). `h`: E′ 19,005 ms (16,730–19,146), peak 3,221,580 KiB (ticks ≤ 11.2, void as timing).
    Parent arm `d`: S_P 2,208, D_P 7,313 ms (void). Parent whole-command `alsem digest` peaks: Pm(d) 3,146,596,
    Pm(h) 3,146,584 KiB.
  - CPU (user+sys, medians of 3): `d` design 120.28 / parent 114.89 s (Δ 5.39 s); `h` design 377.57 / parent
    115.59 s (Δ 262 s).
  - `pd_edge_unmapped` 0, `non_primary_members` 0 on `d` and `h`.
  - P-3: SARIF parent == old == dag at primary, all, o1. P-4: no unpaired effects; `unknown → conditional` 2,
    `trunc true → false` 4, **`unconditional-on-success → conditional` 0** (decision 7's veto question is moot).
  - P-5: default 996/996; under `ALSEM_W6_DAG=1` one test moves: `cli_b_fingerprint_oracles::
    item7_path_limit_reached_detail_cap` (`assertion failed: out.truncated`, `tests/cli/cli_b_fingerprint_oracles.rs:540`).
    Not an F-W3 test; Task 2a must triage it (expected shape: A2-06's distinct-path count no longer reaches the
    cap on that fixture) before any rebaseline.
- **Task 1′ STOP read: PASS.**
  - S1′ (`d`): Δcpu 5.39 s ≤ 8 s; wall terms (valid design rounds, void parent) give 6,673 − 7,313 + 2,239 + 2,750
    − 2,208 = 2,141 ms ≤ 8,000.
  - S2′: 3,128,272 ≤ Pm(d) 3,146,596 + 3,145,728 KiB.
  - S3′ (`h`): E′ 19.0 s ≤ 60 s, peak 3,221,580 ≤ 12,582,912 KiB. The `h` wall rounds ran at load ≤ 11.2 on 22
    cores; load only lengthens wall, so a contended pass is an upper bound. The CPU delta (262 CPU-s) does NOT
    read against a wall bound for a parallel phase: E′'s proofs/ordering run per root across 22 threads
    (prototype: proofs 91, ordering 112–125 CPU-s at E′ 21 s). Recorded as the reading the operator's CPU rule
    could not settle; B6 (wall, quiet machine) decides at Task 4.
  - Terms frozen above. Fallback D not fired. Next: Task 2a.
- Task 2a (opus, 2026-10-23 ~18:30 UTC) committed `b90a01fe`. Red (old search, `/tmp/a8-red1.log`): W1, W4c, W5,
  W6b, W7, W9 FAIL (W1 `["A>W1","A>B>W2"]`; W5 `[]` + `state-limit-exceeded`; W6b `depth-exceeded` on a found
  witness; W7 2 via paths; W4c/W9 `Z(opaque)` kept at cap 1); W2, W4, W6 pass; W3, W8, W4b, W10, O1 compile-red
  (`DagCache`, `CarrierCache`, `DAG_BUILDS`, `ViaWindow`, `HbIndex`, `O1_*`, `W10_*` missing, `/tmp/a8-red2.log`).
  Green: task gate 3847/3847 (executor log `ci-steps-task/20261023T182341-67cc3c76.log` and the rerun after the
  `MergeOut` alias). Goldens: `check-goldens` 996/996, NO golden moved (no regen). item7 triage: real cause is the
  fixture, not A2-06 — its direct facts sat under keys A/B with `subject = "ROOT"`; the old BFS read the
  routine-keyed map at its terminal, the DAG reads carriers from subjects (as the old `valid_nodes` seed did), so
  `terminal-not-found`; fixed the subjects, assertion unchanged, two distinct candidates > cap 1 → truncated. Step 4
  (sonnet runner, `/work/bench/160-6/task2a/step4/`): CDO `o` SARIF parent `adc09612` vs head `4f46f92a` byte-identical
  at `--scope primary`, `--scope all`, `--single-app` (sha `badbf4ff…`, 8 results: 2 d47, 6 d49, 0 d51), all rc 0.
  check-diff: `reasons: []` at base `5b8d198f`; against moved origin/master `7c35891a` it lists #247's committed
  `.agent/issue-247/*` (base drift; rebase owed).
- Task 2a review fixes (opus) committed `e7943f95` on the rebased `d4d78ba0`: equal-JSON parallel edges fold into
  the candidate's conditionality (`WitnessPath.alternatives`, least restrictive per step; new test red
  `conditional` ≠ `unconditional-on-success`, then green); single-routine trie frontier + spec/plan wording; depth
  check cached per (DAG, carrier set); no panic on a carrier without its fact; fingerprint cap once;
  `hb.has_edge` direct. Executor task gate 3857/3857 rc 0; `check-goldens` skipped by the tree stamp of that green
  `ci-steps test` (goldens inside it); check-diff `--base origin/master` `reasons: []`. Witness diagnostic order is
  read only by the fingerprint query (JSON array in emitted order; the human renderer finds `path-limit-reached` by
  kind); digest, prove, SARIF and the HTML report do not read witness diagnostics.
- Task 2b-1 (opus) committed `575021ff`: `Cell::{Caps, Raw}`, `WitnessRequest`, one composition in `prepare`,
  `build_detector_artifacts` over `caps` (composes nothing), `compose_detector_artifacts` for the shim and policy's
  RAW build. Red: lib tests compile-red (`WitnessRequest`, `with_witness`, `View::raw`, `Cell::Caps`, fields;
  `/tmp/a8-2b1-red-lib.log`); CLI `views_release_their_cells_before_the_next_build` red (no `rel:Caps`/`rel:Raw`);
  the T5 rows incl. the new d47+d35 row and the `witness` column were green before (the refactor keeps counts; their
  proof is the extra-cone break). Green: task-affected 3861/3861; executor task gate 3861/3861 rc 0
  (`ci-steps-task/20261023T185752-1c6961a9.log`), no golden moved. check-diff `--base origin/master` (`7c35891a`)
  `reasons: []`. CDO `policy check --deterministic` (sonnet runner, `/work/bench/160-6/task2b1/`), parent `adc09612`
  vs head `fa58994e` (release-fast at `575021ff`): json/sarif/human stdout and stderr byte-identical, all rc 0
  (json `533464cd…`, sarif `fccf90f1…`, human `7bdceffb…`).
- Task 2b-2 (opus) committed `81fecff4`: digest on the view's composition over the cone graph, rule B. Red:
  CLI tests on `575021ff` (`/work/bench/160-6/task2b2/red-cli.log`: T1 `left: []` (#161), digest rows 2/2/3/2,
  release sequence, checkpoint (no `view.live`), RF2 snapshot 1, a6/temp-state rewrites); lib tests compile-red
  (`red-lib.log`: `DigestEffects`, `digest_effects`, `View::effects`, `boundary_group`, `compose_tree` arity).
  Green: task-affected 3881/3881; executor task gate 3881/3881 rc 0 (`ci-steps-task/20261023T203309-183c5410.log`);
  `check-goldens` 1004/1004, no golden file moved. check-diff `--base origin/master` lists only
  `.agent/issue-240/*` (master moved to `60434aef`, #240); against the merge base `7c35891a`: `reasons: []`.
  Step 8 (D outcome): scratch revert of `81fecff4` = the `575021ff` tree (empty diff); `ci-steps task` 3861/3861
  rc 0, check-goldens rc 0; CDO policy check json/sarif parent == head == d byte-identical. CDO (sonnet runner,
  `/work/bench/160-6/task2b2/cdo/`, head alsem `448ed75a…`): `d` 138 → 998 effects (88 groups), `h` 2397 → 22156
  (1293 groups); `witness.non_primary_members=0` on `d` and `h`; `edge_unmapped=0`; `view.live` = Rel,Effects,L4,Det;
  d47/d49/d51 SARIF byte-identical at primary/all/single-app (`badbf4ff…`); CPU `h` 136 → 376 s, peak +326 MB.
- Rebase onto `adde813c` (#254 after #240): 19 commits, conflicts in `CHANGELOG.md` (twice) and `digest_cli.rs`
  (the #254 coverage block read the deleted snapshot). New tip `9115d81c` (2b-2), `24d00b9d` (2b-1). #254 on the
  cone graph, folded into the replayed 2b-2 commit: `digest_plan(.., tests)`, `Plan.tests`; `digest_effects` builds
  `CoverageGraph::from_snapshot(tree, model)` while the tree is alive and `DigestEffects.coverage_graph` keeps it;
  digest_cli computes the block from it (default graph when no root). `test_coverage.rs` join logic unchanged (one
  comment: the tree now has dependency-internal edges). `cli_digest_test_coverage` 13/13 and the 47 `adapter_*`
  lib tests pass with their expectations unchanged; break (graph dropped) fails 10 of the 13. Executor task gate
  3944/3944 rc 0 (`20261023T210457-afff90b6.log`); forced check-goldens 1027/1027; check-diff `--base origin/master`
  `reasons: []`. CDO `--tests` (sonnet runner, `/work/bench/160-6/task2b2/tests/`, base `adde813c` vs head
  `9115d81c`): `testCoverage` identical on `d` (3 uncovered `no-path`) and `h` (88 uncovered `no-path`); CDO has
  no `[Test]` procedure (`testProceduresConsidered` 0), so this run cannot show the cone-graph traversal change.
  Coordinator's Discoveries entry (rule-B group conditionality) merged into the list; it parses (2 entries).
- Task 2b-2 review fixes committed `64ac03a0` (last attempt): `--tests` with no resolved root builds the tree
  (empty `Roots`), so the block is #254's again (11 tests, partial; new CLI test pinned to `adde813c`'s output);
  B7 MUST-set test on `WriteCallCommit` / `DepWriteThenCommit` (both filters proven); CHANGELOG states the group
  class/tempState shape and the `--tests` cone-graph semantics; the rule-B Discoveries symptom gains the MAY
  pending-write note (JSON parses). Executor task gate 3946/3946 rc 0 (`20261023T213143-f5765478.log`); forced
  check-goldens 1028/1028. check-diff `--base origin/master` lists `.agent/issue-253/*` and
  `cargo-version-changed` only because master moved to `0b0b95c9` (v2.0.3, #253); against the branch base
  `adde813c`: `reasons: []`. Not rebased.
- 2026-10-23 ~22:00 UTC: rebased onto origin/master `0b0b95c9` (v2.0.3), no conflicts, check-diff `reasons: []`.
- 2026-10-23 ~22:20 UTC: Task 3 committed `72a30b67` (on `d95d648d`). `bash scripts/ci-steps task` rc 0 (3965/3965,
  run directly: the executor refused with the 10 h cap), `CHECK_GOLDENS_FORCE=1 scripts/check-goldens` 1042/1042,
  nothing moved. CDO `policy check --deterministic` json/sarif/human x bundled/custom policy: parent `adc09612` ==
  head on stdout and stderr (`/work/bench/160-6/t3-identity/`; default json 19 findings both).
  **Attempt 8 ends `blocked wall-clock`** (claimed 12:03, 10 h cap at 22:03). Remaining: Task 4 (CDO identity,
  triage, quiet-slot bench B1′/B2′/B6/B7/B3-B5, cdo-gate, numbers file), the full gates, final panel, CI, merge.
  Next attempt carries this branch over (as attempt 8 did from a7).
- 2026-10-24 ~00:00-00:45 UTC (between attempts, no executor run): determinism fix `5406d96c` (unresolved-cone
  order total; master v2.0.3 unaffected, 3 identical runs); rebased onto origin/master `678d6540` (#256), CHANGELOG
  #160 + #256 entries moved to [Unreleased] (`bb81b4db`). Direct gate runs on `bb81b4db`: `ci-steps task` 3971/3971,
  `ci-steps all` rc 0, `check-goldens --verify-coverage` rc 0, `cdo-gate` rc 0 (2386 passed). Task 4 bench and Part 1
  identity (direct, `/work/bench/160-6/final/`): B1′ d 20.78 → 23.77 s, B2′ 3.14 → 3.15 GB, B6 h 21.96 → 41.39 s,
  B7 3.51 GB, B3/B4 o 17.27 → 16.99 s / 3.16 → 3.14 GB, B5 policy 51.4 → 27.5 s / 13.2 → 8.3 GB; all bounds pass;
  rounds' max 1-min load 8.3-9.2 with an otherwise idle machine (own threads). Part 1: default analyze, o (both
  scopes, both modes), policy (bundled + custom), histogram, fingerprint (bar generatedAt) identical to v2.0.3.
  Attempt 9 must re-run the gates through the executor and repeat the bench under its claim if required.
- 2026-10-24 `5406d96c`: unresolved-cone items get a total order (column, candidates, callsite id); `digest d`
  cross-app swapped two anchorless `WordTemplates.Merge` rows between runs (hash order). Task gate 3966/3966,
  forced check-goldens 1042/1042, nothing moved; master v2.0.3 d 3 runs identical (its cone has no such rows).

## Acceptance matrix

## Discrimination proofs

| Test | Mutation | Fail | Pass | Commit |
|---|---|---|---|---|
| T2 `t2_witness_scope_gates_raw_per_member` | break 1: gate no-op (`ConeScope::member` raw = `true \|\| contains`) | FAIL at "no key M in raw" (`capability_cone.rs:7541`) | pass after revert | `72abd499` |
| T2 | break 2: coverage insert moved under `if mode.wants_raw()` | FAIL at "coverage has M" (`left: None`) | pass after revert | `72abd499` |
| T2 | break 2b: the `(false, false)` skip branch emits no coverage | FAIL at "derived=false: coverage has M" | pass after revert | `72abd499` |
| `compose_tree_equals_the_cut_snapshot_single_app` | compose_tree's pd scope = `Some(∅)` | FAIL: `pd.states/links/by_routine differs` (57 lines, from `ws-d1-multi-caller`) | pass after revert | `72abd499` |
| `compose_tree_equals_the_cut_snapshot_single_app` | compose_tree drains no raw (`\|_\| None`) | FAIL: `capability_facts differs` (`ws-callsite-resolutions`, …) | pass after revert | `72abd499` |
| W1 `w1_dag_witness_is_the_shortest_path_only` | read every reached carrier, not those at `dmin` (`at_min` = all reached, bit = own δ, lengths = their δs) | FAIL `["A>W1","A>B>W2"]` | pass after revert | `b90a01fe` |
| W7 `w7_conditionality_is_read_over_the_shortest_witnesses` | same as W1 (the longer unguarded path to W2 joins) | FAIL `unconditional-on-success` ≠ `conditional` (fixture gained W2; same-carrier longer path is not in the DAG, so no mutation reaches it) | pass after revert | `b90a01fe` |
| W2 `w2_dag_witness_keeps_every_shortest_path` | keep only the first predecessor (`preds.push` dropped for later equal-δ preds) | FAIL `["X>A>W"]`; W4, W4b also fail | pass after revert | `b90a01fe` |
| W3 `w3_one_dag_per_root_and_first_hop` | skip the cache lookup (`self.0.remove(key)` before `entry`) | FAIL 3 ≠ 2 (W8 also 2 ≠ 1) | pass after revert | `b90a01fe` |
| W8 `w8_one_dag_per_root_across_its_facts` | fresh `DagCache` per call in `build_block` | FAIL 2 ≠ 1 | pass after revert | `b90a01fe` |
| W4 `w4_dag_truncates_at_the_callers_cap` | walk limit `cap` instead of `cap + 1` | FAIL `(256,false,false)` ≠ `(256,true,true)`; W4b, W4c, W9, corpus prefix oracle, item7 also fail | pass after revert | `b90a01fe` |
| W4 | hard-code 256 in the truncation (`merged.len() > HARD_PATH_CAP`, `truncate(HARD_PATH_CAP)`) | FAIL cap-3 arm `(4,false,false)` ≠ `(3,true,true)` | pass after revert | `b90a01fe` |
| W4b `w4b_trie_walk_equals_the_brute_force_key_order` | walk edges, not the trie (`chunk_by(\|_,_\| false)`) | first green: post-walk dedupe hid the duplicates; fixture (2) gained `S → V` (object run, sorts after the triggers); then FAIL "(2) equal JSON cap 3" truncation `false` ≠ `true` | pass after revert | `b90a01fe` |
| W4b | length classes descending (stands in for one walk over all lengths) | FAIL "(3) lengths cap 1": length-3 carrier before the length-1 opaque | pass after revert | `b90a01fe` |
| W4b / W9 (A4-01) | mark carriers only (opaque admission `false &&`) | FAIL W4b "(3) lengths cap 1"; W9 "Z counts toward cap + 1"; W4c; kept-opaque-diags test | pass after revert | `b90a01fe` |
| W4c `w4c_carrier_and_opaque_tie_is_ordered_by_projected_json` | opaque children walked first (`sort_by_key(!opaque)`) | first green: the final key sort hid the order with one opaque seed; fixture gained seed Y; then FAIL `["Y(opaque)"]` ≠ `["A"]` | pass after revert | `b90a01fe` |
| W5 `w5_dag_witness_has_no_state_budget` | stop the DAG BFS after `MAX_STATES` pops | FAIL `[]` ≠ `["A>Z>W"]` | pass after revert | `b90a01fe` |
| W6 `w6_carrier_past_max_depth_is_depth_exceeded` | never set the depth flag (`&& false`) | FAIL `[terminal-not-found]` (W6b too) | pass after revert | `b90a01fe` |
| W6b `w6b_depth_exceeded_is_per_fact` | diagnostic whenever the DAG has a depth frontier | FAIL fact A carries `depth-exceeded` | pass after revert | `b90a01fe` |
| W9 `w9_opaque_seeds_are_candidates_and_other_opaque_nodes_are_not` | report every opaque node the DAG reaches (opaque set over all DAG nodes, admitted/emitted at any depth) | first green: W9's fixture had `dmin = 1`, so M was never walked; arm added (O beside W below M, `dmin = 2`); then FAIL `["M>O(opaque)","M>W"]` | pass after revert | `b90a01fe` |
| W10 (hand + corpus) | window `MAX_VIA_PATHS` instead of `+ 1` (`at < MAX`, `truncate(MAX)`) | FAIL both (hand: truncated `false` ≠ `true`; corpus: shadow assertion in a rayon worker) | pass after revert | `b90a01fe` |
| O1 (b) `o1_on_demand_happens_before_on_hand_built_ops` + O1 (a) corpus | drop `may_co_execute` | FAIL "the two arms never co-execute"; O1 (a) and W10 corpus (shared `o1_check`) fail | pass after revert | `b90a01fe` |
| O1 (b) + (a) | drop the same-group check (compare across groups) | FAIL "no edge between groups"; O1 (a) fails | pass after revert | `b90a01fe` |
| `capped_ordinary_witness_reports_opaque_boundaries_of_kept_paths_only` (rewritten, cap 2) | opaque diagnostics before the cap | FAIL `[Y, Z]` ≠ `[Y]` | pass after revert | `b90a01fe` |
| `event_first_hop_subscriber_that_cannot_reach_the_write_gives_no_path` (rewritten) | admit and accept every seed | FAIL (panic "a marked carrier carries the fact") | pass after revert | `b90a01fe` |
| cli_b `item7_path_limit_reached_detail_cap` (fixture fixed) | fixture back to `subject = "ROOT"`; and walk limit `cap` | FAIL `[terminal-not-found]`; FAIL `[]` (no truncation) | pass after revert | `b90a01fe` |
| note: W7 | W7's break is W1's (the overlap is intended: the same-carrier longer path is never in the DAG, so only another carrier's path can carry the unguarded conditionality) | — | — | `b90a01fe` |
| `equal_json_parallel_edges_fold_into_the_candidates_conditionality` | read only a group's first edge (`s[1..]` → `s[s.len()..]` in `candidate`) | FAIL `conditional` ≠ `unconditional-on-success` | pass after revert (task gate 3857/3857) | `e7943f95` |
| T5 `cli_build_counters::analyze_*` (6 rows incl. new `analyze_cross_app_d47_with_a_summaries_detector`) | extra det cone: `compose_capabilities(.., {derived, None})` at the top of `build_detector_artifacts` | FAIL every analyze row: `build.cone` 2 ≠ 1 (d47+d35: 3 ≠ 2), `graph_scc` +1, `pd_solve` +1 | pass after revert | `575021ff` |
| T8 break 1 `step6_t8_a_primary_witness_request_materializes_no_dependency_member` | `WitnessRequest::Primary` resolved as `WitnessScope::All` | FAIL `left: 4, right: 0` (4 dependency members materialized) | pass after revert | `575021ff` |
| `caps_die_with_the_det_fill_and_raw_needs_a_witness_request` + `views_release_their_cells_before_the_next_build` | skip `note_release(Caps)` after the det fill | FAIL "Caps released after the det fill"; CLI sequences lack `rel:Caps` | pass after revert | `575021ff` |
| `caps_die_with_the_det_fill_and_raw_needs_a_witness_request` | fill `Raw` whatever the request (`if true`) | FAIL "no witness request, no raw" | pass after revert | `575021ff` |
| `a_view_takes_one_witness_request` | assertion disabled (`true \|\| ..`) | FAIL: panic is "Caps released", not the expected text | pass after revert | `575021ff` |
| T1 `t1_call_chain_reports_the_dependency_modify_and_commit_as_groups` (CLI) | tree over the primary callers' typed edges only (`r.cone()` filtered to non-dependency `from`) | FAIL: both effects lose their witness (`[("COMMIT","",..,[],0),("DB_MODIFY","",..)]`) | pass after revert | 2b-2 |
| B1 `b1_effects_behind_one_call_are_one_group_per_type` | group key = `dedupe_key` | FAIL `left: 2, right: 1` (one effect per resource) | pass after revert | 2b-2 |
| B2 `b2_a_primary_witness_keeps_its_per_resource_effect` | `is_dependency` true for every routine (group every transitive effect) | FAIL plain effects `left: 0, right: 1` | pass after revert | 2b-2 |
| B3 `b3_a_group_lists_its_first_ten_resources` | `take(11)` | FAIL 11 resources listed | pass after revert | 2b-2 |
| B4 `b4_rule_b_is_off_on_the_cut` | `compose_snapshot_from_base` sets `boundary: Some(boundary_rule(..))` | FAIL the cut groups (`DB_MODIFY <guid>.Write Customer`) | pass after revert | 2b-2 |
| B5 `b5_one_fact_behind_four_boundaries_is_four_groups` + W4b rule-B (4) | key candidates by projected path only (`Candidate::key_cmp` without the end key) | FAIL one group (`XDep.DepA` only); W4b "(4) shared prefix cap 1" truncation | pass after revert | 2b-2 |
| B5 | select only the fact's first boundary (`end_of(i).and(first_b)`) | FAIL one group | pass after revert | 2b-2 |
| B6 `b6_a_primary_carrier_behind_dependency_code_is_in_the_group` | let the primary carrier win over the cut (marking propagates over dependency routines, no boundary edges) | FAIL extra `DB_INSERT - Ws Log` per-resource effect | pass after revert | 2b-2 |
| B7 `b7_a_boundary_commit_carries_no_must_label` | boundary occurrence in the MUST commit set (`is_boundary = false`, `must = true`) | FAIL `COMMIT_DOMINATES_RETURN`, `WRITE_COMMITTED_BEFORE_RETURN` on the group | pass after revert | 2b-2 |
| B7 | root-scope `COMMIT_DOMINATES_RETURN` guard dropped | FAIL root `COMMIT_DOMINATES_RETURN` on the group | pass after revert | 2b-2 |
| B8 `b8_proofs_are_cut_at_the_boundary_and_keep_their_class` | proofs pass through the boundary (`cut` always `None`) | FAIL the kt insert is a per-resource effect with a dependency-internal terminal (`dep:..src/Lib.al` 11:8), 1 hop | pass after revert | 2b-2 |
| W4b rule-B (3) `w4b_rule_b_cut_keys_equal_the_brute_force_order` | length classes descending | FAIL "(3) boundary lengths cap 1" (length 3 before 1) | pass after revert | 2b-2 |
| T3 `t3_digest_scoped_equals_unscoped` | break 1: under `Subjects`, non-subject SCCs get an empty fact cone | FAIL at the oracle (ws-callsite-resolutions, ws-d12, ws-d2, …); a first variant (no cone at all) panicked in the fact-cone guard instead | pass after revert | 2b-2 |
| T3 | break 2: under `Subjects`, recursive members never raw | FAIL at the oracle on `ws-d7-event-cycle` (recursive) | pass after revert | 2b-2 |
| T3 | break 3 (evidence only): not run; the call-site maps are built inside `compute_ordering` from the tree, which carries no subject marker, so a scoped-arm-only break needs a test hook | — | — | 2b-2 |
| T4 (standalone parity) | the cut base's composition `raw: Subjects(∅)` | FAIL 9 `cli_b_fingerprint_differential` goldens | pass after revert | 2b-2 |
| T5 digest rows `digest_cross_app` / `digest_single_app` | extra det cone at the top of `build_detector_artifacts` | FAIL `cone 2≠1, scc 3≠2, pd 2≠1` both rows | pass after revert | 2b-2 |
| T6 `t6_an_anchorless_cone_edge_has_no_row_and_its_link_is_dropped` | keep the row with a default anchor | FAIL "no typedEdges row" | pass after revert | 2b-2 |
| T7 (a) `t7a_a_dependency_callers_rows_are_the_ledgers` | `dependency_calls` left out of the tree's ledger | FAIL leftover row `("Codeunit.Run(..)","direct","resolved",None)`, the unresolved row missing | pass after revert | 2b-2 |
| T7 (b) `t7b_a_call_into_a_body_less_dependency_row_is_unfetched` | `bodyless_dependency: None` (the `resolved` arm kept) | FAIL `["resolved"]` | pass after revert | 2b-2 |
| T8 `t8_the_trees_coverage_is_the_det_cells` | tree coverage from the cut's fold | FAIL `("complete", [])` ≠ `("partial", ["ambiguous-overload"])` | pass after revert | 2b-2 |
| T9 (a) `t9a_a_routine_selector_reads_the_table_digest_builds` | the pipeline block drops `CallWrite`'s row from the table | FAIL 0 entries | pass after revert | 2b-2 |
| traversal block (A1-16), `cli_b_digest_differential` | the query serializes a default `UnresolvedTraversal` | FAIL `visitedRoutines` 0 ≠ 3 (json and diff goldens) | pass after revert | 2b-2 |
| checkpoint `digest_holds_no_tree_at_its_first_run` (CLI) | skip `*tree = None` | FAIL `Tree alive: [Rel, PdArena, Tree, Effects, L4, Det]` | pass after revert | 2b-2 |
| Review Focus 2 `digest_with_no_resolved_root_builds_no_tree` (CLI) | `needs_tree` always true | FAIL `build.snapshot` 1 ≠ 0 | pass after revert | 2b-2 |
| minor 1 `a_witness_request_after_a_composition_names_the_rule` | `Cell::Caps` dropped from the assertion's bits | FAIL "Caps released; a View fills each cell once" | pass after revert | 2b-2 |
| minor 2 `a_raw_only_composition_is_released_by_prepare` | `*caps = None` deleted | FAIL "Caps dropped" | pass after revert | 2b-2 |
| `digest_plan_asks_a_tree_only_for_roots` | `Roots` scoped by internal id | FAIL `Subjects({})` ≠ `{r1, r2}` | pass after revert | 2b-2 |
| `cross_app_digest_reads_the_cross_app_store`, `a6_dependency_trigger_reaches_the_workspace_caller` (rewritten for rule B) | rule B off in `compose_tree` | FAIL both (per-resource inserts, no group) | pass after revert | 2b-2 |
| note (2b-2) | every revert confirmed by one run after the last: `ci-steps task-affected` 3881/3881 and the executor task gate 3881/3881 | — | — | 2b-2 |
| #254 on the tree, `cli_digest_test_coverage` (13 tests) | `coverage_graph` always `None` (`.filter(\|_\| false)`) | FAIL 10 of 13 | pass after revert | `9115d81c` |
| `tests_block_with_no_resolved_root_is_the_workspaces` (CLI) | the `CoverageGraph::default()` fallback restored (`needs_tree` for roots only) | FAIL `status: complete`, no reasons, 0 tests ≠ #254's `partial`, 5 reasons, 11 tests | pass after revert | `64ac03a0` |
| B7 MUST sets `b7_boundary_groups_stay_out_of_a_primary_commits_must_sets` | `must = \|_\| true` (`is_boundary` kept) | FAIL at WriteCallCommit: no `WRITE_COMMITTED_BEFORE_RETURN` (the boundary commit is the nearest) | pass after revert | `64ac03a0` |
| B7 MUST sets | the write-set `filter(must)` only (`filter(\|_\| true)`) | FAIL at DepWriteThenCommit: `WRITE_COMMITTED_BEFORE_RETURN` from the boundary write | pass after revert | `64ac03a0` |
| note (review fixes) | every revert confirmed: executor task gate 3946/3946, forced check-goldens 1028/1028 | — | — | `64ac03a0` |
| T8 break 1 (policy) `policy_view_materializes_no_dependency_member` | `WitnessRequest::Primary` → `WitnessScope::All` | FAIL `left: 4, right: 0` (4 dependency members materialized) | pass after revert | Task 3 |
| T3 policy arm `policy_selects_the_parents_facts_per_primary_routine` | `select_facts` `"inherited" => Vec::new()` (raw dropped) | FAIL 162 cases differ (e.g. `dep/dep_ambiguous cross` inherited 1 → 0) | pass after revert | Task 3 |
| `command_plans_declare_after_run` (policy case added) | `policy_plan` `after_run: 0` | FAIL "policy: expected a panic, none came" | pass after revert | Task 3 |
| `inherited_raw_panics_when_never_materialized` (message changed) | `None => &[]` | FAIL "test did not panic as expected" | pass after revert | Task 3 |
| `views_release_their_cells_before_the_next_build` (policy sequence restated) | delete `release_all_but(&[Cell::Det, Cell::Raw])` | FAIL policy got `[ctx, Caps, Det, L4, Ordering, Rel, Raw]` | pass after revert | Task 3 |
| `policy_check_matches_an_inherited_dependency_fact` (mutation rewritten on `Cell::Raw`) | rule engine gets an empty map for `view.raw()` | FAIL findings `[]` ≠ `[policy-inherited-insert, Work.al, 12]` | pass after revert | Task 3 |
| T5 policy rows `policy_check_cross_app` / `_single_app` | a second `compose_detector_artifacts(SUMMARIES)` in `run_policy_check` | FAIL both rows: cone 2≠1, scc 3≠2, pd 2≠1, ctx 2≠1 | pass after revert | Task 3 |
| note (Task 3) | oracle `tests/fixtures/policy-facts-oracle/selected-facts.json` recorded on `86d24beb` (scratch worktree, recorder at `/work/bench/160-6/t3-oracle-recorder.rs`), deterministic over two runs (sha256 `1f51f01f…`); reverts confirmed by `ci-steps task` 3965/3965 and forced check-goldens 1042/1042 | — | — | Task 3 |
| determinism fix `ties_on_the_old_key_sort_by_column_candidates_then_callsite_id` | `item_order` without the candidates + callsite-id tie-breaks; then without the column tie-break | FAIL `[csZ,csN,csM,…]` ≠ `[csM,csN,csZ,…]`; FAIL `[…,csP,csQ]` ≠ `[…,csQ,csP]` | pass after each revert | `5406d96c` |
| F1-S1 `proof_cap_counts_distinct_candidates_after_the_boundary_cut` | red first (raw counting) | FAIL cap 2 `[cs-a]` ≠ `[cs-a, cs-c]` | pass with fix | `0f21dbb6` |
| F1-S1 same | crossed paths into the FIFO queue (no level priority) | FAIL cap 2 `[cs-a, cs-b]` ≠ `[cs-a, cs-c]` | pass after revert | `0f21dbb6` |
| F1-S1 same | raw counting + no crossed-key prune | FAIL cap 3 (cs-b lost, truncated) | pass after revert | `0f21dbb6` |
| F1-S1 same | raw counting alone | PASS: the prune alone covers it (stated; the twin row below isolates it) | — | `0f21dbb6` |
| F1-S1 `twin_proofs_count_once_toward_the_cap` (two-producer row) | raw counting alone | FAIL `[([0,1],0),([0,1],1)]` (longer proofs lost) | pass after revert | `0f21dbb6` |
| F1-S2 `an_event_boundary_group_is_in_the_may_relations` | red first; then drop the placement (`evidence_callsite_id` stays None) | FAIL `["ORDERING_UNKNOWN"]` | pass after revert | `502b1c8a` |
| F1-C1 `a_routine_missing_from_raw_fails_loudly` | red first (old `map_or(&[])`) | FAIL "test did not panic as expected" | pass with fix | `d22b0129` |
| F1-C1 `a_direct_rule_never_reads_raw` | drop the `facts: direct` exemption | FAIL (the panic) | pass after revert | `d22b0129` |
| F1-C2 `the_dependency_calls_leave_the_base_in_the_first_prepare` | the view passes `&c[..0]` to compose_tree | FAIL items `[]` (no R.NoSuchMethod) | pass after revert | `8ba75e0d` |
| F1-C2 same | `clone()` instead of `take()` | FAIL "digest: released" `Some(2)` | pass after revert | `8ba75e0d` |
| F1-O3 `the_callee_sorts_before_the_column` | red first (column before callee) | FAIL `[csS, csR]` ≠ `[csR, csS]` | pass with fix | `581f3397` |
| F1-O4 `a_multi_target_boundary_names_every_entered_routine` | red first (no per-identity display map) | FAIL `XDep.DepB` ≠ `XDep.DepA, XDep.DepB` | pass with fix | `66f4cd5a` |
| F1-C3 `a_det_fill_after_a_raw_only_composition_names_the_rule` | red first (generic refill text) | FAIL "Caps released; a View fills each cell once" | pass with fix | `a25d0cad` |
| note (final r1) | stated limit: F1-S2's prefix trim (pop of the raising call hop) is not pinned (intra labels read the owner group); the 502b1c8a body's "empty either way" is wrong, without the pop the chain holds one link | — | — | `502b1c8a` |
| step 9 1.2 `build_with_no_roots_config_reaches_both_arms` | red first: `build_with` absent (E0599, compile) | FAIL (compile) | pass with 1.1 | `ac447e3c` |
| step 9 1.2 same | literal `false` in `build_cross_app_base_with`'s `assemble_and_resolve_cross_app_program` call | FAIL at "cross-app: --no-roots-config leaves no config_entry_id" (left 1, right 0); single-app half passed first | pass after revert (re-read line 3825) | `ac447e3c` |
| step 9 T20 `tree_read_lifecycle` | red first: `TreeRead`, `Cell::TreeRead`, `take_tree_read`, `census_output_line` absent (compile) | FAIL (compile) | pass with 1.3-1.4 | `ac447e3c` |
| step 9 T20 same | B1: `*tree = None` skipped for `TreeRead::FingerprintQuery` | FAIL at (c) "the FingerprintQuery tree died inside prepare" (as named) | pass after revert | `ac447e3c` |
| step 9 T20 same | B2: `Cell::bit` gives `TreeRead` the bit of `Det` | FAIL at (f) "every Cell bit is distinct" (left 8, right 9) (as named) | pass after revert | `ac447e3c` |
| step 9 T20 (review item 2) | `release_all_but` releases `TreeRead` without the `is_some()` guard | FAIL at "(b) an unfilled TreeRead is not released, so no rel:TreeRead trace" (left 256) | pass after revert | `a95b95d0` |
| step 9 Task 2 red | prove forced single-app (`single_app \|\| true`, the base's universe) | FAIL T1 (precondition: Outer → Work row), T8 (precondition: universes differ), T19 (precondition: coverage), T24 (answer `unknown`); T7, T12, T14, T15, T22, T16 pass (not universe tests) | pass after revert | `a95b95d0` |
| step 9 T1 `prove_two_hop_not_proven` | B1: `&c[..0]` as the tree's dependency calls in `prepare` | FAIL at the PRECONDITION "the Outer → Work row" (digest reads the same ledger), not the named callsite assertion; direct run of the broken binary: blockers `analysis-gap`, `coverage-incomplete` only (callsite gone) | pass after revert | `a95b95d0` |
| step 9 T1 same | B2: `gaps_in_cone: 0 *` in `digest_effects` | FAIL at "analysis-gap" (as named) | pass after revert | `a95b95d0` |
| step 9 T2 `prove_one_hop_ledger_and_gap` (lib) | B1: `if false && into_unfetched` (`snapshot.rs:2599`) | FAIL at the PRECONDITION "CallWrite → Write is unfetched-dependency" (left `resolved`), not the named claim | pass after revert | `a95b95d0` |
| step 9 T2 same | B2: `gaps_in_cone: 0 *` | FAIL at the blocker set (analysis-gap gone) (as named) | pass after revert | `a95b95d0` |
| step 9 T12 `abi_only_target_blocks_prove` | skip `external` rows in `unresolved_cone`'s harvest | FAIL at the PRECONDITION "the row is external" (digest reads the same harvest); direct run: blockers `coverage-incomplete` only, answer `unknown` (as the spec states) | pass after revert | `a95b95d0` |
| step 9 T19 `prove_commits_on_success_path_through_boundary_is_unknown` | boundary groups get their computed path conditionality (`digest.rs:3935`) | FAIL: answer `yes` (`unconditional-on-success`) (as named) | pass after revert | `a95b95d0` |
| step 9 T24 `prove_matches_a_group_resource_beyond_ten` | `effect_resource_matches` on `detail` only (`\|\| false &&`) | FAIL: answer `no` (a false `no`, the S9-1 shape) | pass after revert | `a95b95d0` |
| step 9 T15 `prove_cone_truncation_blocks` | `cone_truncated: false` (`prove.rs:175`) | FAIL at `coneTruncated` (answer `no`) (as named) | pass after revert | `a95b95d0` |
| step 9 T14 `diagnostics_channel_prove` | the run's diagnostics replaced by `[]` in prove | FAIL at the version-warning assertion (as named) | pass after revert | `a95b95d0` |
| step 9 T7 `no_dependency_parity_prove` | B1: a cross-app-only diagnostic appended to the envelope | FAIL at the failures assertion (every root) (as named) | pass after revert | `a95b95d0` |
| step 9 T8 `single_app_flag_reaches_prove` | the flag ignored (`single_app && false`) | FAIL at the PRECONDITION "the universes differ" (both runs cross-app) | pass after revert | `a95b95d0` |
| step 9 T22 `no_app_json_keeps_its_error_prove` | the build error remapped to `prove: workspace did not resolve` (the spec's break, a dummy model id, changes the same line; this form needs no fake id) | FAIL at the error-line assertion | pass after revert | `a95b95d0` |
| step 9 T16 `prove`, `prove_single_app`, `prove_no_root` | prove also composes the cut (`compose_snapshot` after the run: "left on the cut") | FAIL all three (bcg 2, cone 2, scc 3, pd 2, snap 2, witness 2) | pass after revert | `a95b95d0` |
| step 9 T16 `prove_no_root` | `digest_plan(.., tests: true)` (a tree with no root) | FAIL at `build.snapshot` 0 | pass after revert | `a95b95d0` |
| step 9 T17 `prove_scoped_equals_unscoped` (lib) | `.take(0)` in `WitnessRequest::Roots`' scope (no subject) | FAIL at the failures assertion (every root with transitive effects, both universes) | pass after revert | `a95b95d0` |
| step 9 Task 3 red | `--routine` path forced single-app (`opts.single_app \|\| true`) | FAIL T3 (claim: inherited modify), T25 call / two-boundary / witness-0 (precondition: no inherited insert), T26 (precondition: direct coverage partial), T8 (precondition: universes differ); T5, T6, T7, T13, T14, T22 pass (not universe tests) | pass after revert | `6f7a24ac` |
| step 9 T3 `fingerprint_routine_inherits_through_dependency` | the pool never filled (`m.len() == 99`) | FAIL at "an inherited modify" (production pool pinned) | pass after revert | `6f7a24ac` |
| step 9 T5 `fingerprint_selector_primary_only` | selector index with `None` (`primary_guid.and(None)`) | FAIL: exit 2, `selector-ambiguous` (as named; T6 also fails there) | pass after revert | `6f7a24ac` |
| step 9 T6 `fingerprint_total_counts_primary_roots` | count every classification (`\|\| true`) | FAIL: total 7 vs 5 (TWO higher: the dependency OnRun and one more dependency classification; spec said one) | pass after revert | `6f7a24ac` |
| step 9 T21 `fingerprint_wide_query_stays_single_app` | every query form to the new path | FAIL at `build.combined_graph` 1 vs 2 (human, no flag) (as named) | pass after revert | `6f7a24ac` |
| step 9 T16 `fingerprint_query`, `fingerprint_query_single_app` | `--routine` left on the cut (`if false && ..`) | FAIL both: `combined_graph` 2 vs 1 | pass after revert | `6f7a24ac` |
| step 9 T25 call arm | B1: the adapter's `boundary: idx.boundary.and(None)` | FAIL at "the witness never reaches Inner" (hops reach `Inner`) (as named); the two-boundary arm fails too | pass after revert | `6f7a24ac` |
| step 9 T25 two-boundary arm | B2: the first path's boundary for every path (`ends.get(i * 0)`, `paths[0]`'s terminal) | FAIL at "each path its own call boundary" (as named) | pass after revert | `6f7a24ac` |
| step 9 T25 trigger/event (lib `fingerprint_witness_boundary_trigger_and_event`) | B3: `path_boundary` for `cs:` only | FAIL at the trigger arm (no boundary); second run with only `ev:` dropped: FAIL at the event arm | pass after revert | `6f7a24ac` |
| step 9 T25 two-boundary arm | B4: the evidence picked from a `HashMap` of boundaries, 5 runs | FAIL 5/5: 4 at "the evidence is the first path's boundary", 1 at "two runs are byte-equal" | pass after revert | `6f7a24ac` |
| step 9 T25 `--witness 0` arm | B5: no evidence with no path (`.filter(\|_\| false)`) | FAIL (as named) | pass after revert | `6f7a24ac` |
| step 9 T26 `fingerprint_may_commit_uses_inherited_coverage` | `direct_status` always (`include_inherited && false`) | FAIL: `no` vs `unknown` (as named) | pass after revert | `6f7a24ac` |
| step 9 T17 `fingerprint_query_scoped_equals_unscoped` (lib) | `.take(0)` in `Roots`' scope | first FAIL at the precondition (inherited counted on the scoped side); counting moved to the unscoped side, then FAIL at the failures assertion | pass after revert | `6f7a24ac` |
| step 9 T7 `no_dependency_parity_fingerprint_routine` | B1: a cross-app-only analyzer diagnostic on the new path | FAIL at the failures assertion | pass after revert | `6f7a24ac` |
| step 9 T8 `single_app_flag_reaches_fingerprint_routine` | the flag ignored (`single_app && false`) | FAIL at the precondition "the universes differ" | pass after revert | `6f7a24ac` |
| step 9 T8 same (Task 2 review item 3) | the single-app run's diagnostics cleared (universes still differ) | FAIL at "json: --single-app != the base's default" (golden equality) | pass after revert | `6f7a24ac` |
| step 9 T13 `fingerprint_no_roots_config_cross_app_routine` | `build_with(.., false, ..)` on the new path | FAIL at "the AST kinds only" (as named) | pass after revert | `6f7a24ac` |
| step 9 T14 `diagnostics_channel_fingerprint_routine` | the run's diagnostics dropped (`take(0)`) on the new path | FAIL (as named) | pass after revert | `6f7a24ac` |
| step 9 T22 `no_app_json_keeps_its_error_fingerprint_routine` | the new path's error remapped | FAIL at the error line | pass after revert | `6f7a24ac` |
| step 9 T2 `prove_one_hop_ledger_and_gap` (review item 2) | skip `unfetched-dependency` rows in `unresolved_cone`'s harvest (the tree's ledger, the precondition, unchanged) | FAIL at the CLAIM: blockers lack `unresolved-callsite: Lib.Write in CallWrite` | pass after revert | `dd8cb6e5` |
| step 9 T8 `single_app_flag_reaches_prove` (review item 3) | the single-app run's diagnostics replaced by `[]` (universes still differ) | FAIL at "json: --single-app != the base's default" (golden equality) | pass after revert | `dd8cb6e5` |
| step 9 `prove_with_no_root_releases_as_digest_does` | red first = the pre-fix code (no `release(L4)`/`release(Effects)` in the dummy branch) | FAIL: sequence lacks `rel:L4`, `rel:Effects` | pass with the fix | `dd8cb6e5` |
| step 9 Task 4 red | inventory left on the cut (`if false && opts.inventory_only`; the old B0 inventory branch is deleted, so it falls to the full snapshot) | FAIL T11 (lib), T8, T13, both T16 inventory rows, 3 `cli_p1_inventory` tests; T7, T22 pass (not universe tests) | pass after revert | `90b92578` |
| step 9 T11 `inventory_reads_a_tree_with_no_raw_facts` (lib) | `Roots(∅)`'s scope = every primary routine (in `WitnessRequest::scope`; the test's expected tree is hand-stated `Subjects(∅)`) | FAIL at the envelope equality; the counter row fails at `witness_pass` 1 | pass after revert | `90b92578` |
| step 9 T7 `no_dependency_parity_inventory_only` | B1: a cross-app-only analyzer diagnostic | FAIL | pass after revert | `90b92578` |
| step 9 T7 same | B2: `filter_by_class` drops the first primary `rootClassifications` row | FAIL | pass after revert | `90b92578` |
| step 9 T8 `single_app_flag_reaches_inventory_only` | the flag ignored | FAIL at the precondition "the universes differ" | pass after revert | `90b92578` |
| step 9 T8 same | the single-app inventory drops `coverage` | FAIL at "--single-app != the base's default" | pass after revert | `90b92578` |
| step 9 T13 `inventory_only_no_roots_config_cross_app` | `build_with(.., false, ..)` on the inventory path | FAIL at the `configEntryId` count | pass after revert | `90b92578` |
| step 9 T22 `no_app_json_keeps_its_error_inventory_only` | the error remapped | FAIL | pass after revert | `90b92578` |
| step 9 T16 `fingerprint_inventory_only`, `_single_app` | inventory left on the cut | FAIL both: `combined_graph` 2 vs 1 | pass after revert | `90b92578` |
| step 9 `fingerprint_selector_on_a_dependency_only_routine_is_not_found` | selector index with `None` | FAIL: exit 0 vs 2 | pass after revert | `90b92578` |
| step 9 T27 `source_app_bodyless_row_gets_body_unavailable_gap` (lib) | red first = D9's filter removed (`true \|\| source_less(guid)`, the pre-D9 rule) | FAIL at "body-unavailable for Write" (an app-level symbol-only-boundary instead); T2 fails too; `cli_snapshot_schema4::opaque_gap_of_an_app_with_source_is_per_routine` fails with F21's debug assertion | pass with D9 | `1726e678` |
| step 9 T18 `full_snapshot_split_is_byte_identical` | an empty permission array from `compose_full_snapshot` | first FAIL at the precondition (it read the head's own output); precondition moved onto the base golden, then FAIL at "full-snapshot.dd.cross-app: json" | pass after revert | `1726e678` |
| step 9 `cross_app_inventory_matches_its_golden` | `filter_by_class` keeps every root classification (`\|\| true`) | FAIL at the golden equality (the dependency `OnRun` leaks) | pass after revert | `1726e678` |
| step 9 B1q `doc_serializes_as_the_normalized_value` (lib) | `Doc::Map` without its key sort | FAIL | pass after revert | `726d355b` |
| step 9 B1q same | `Doc::Map` without its null drop | FAIL | pass after revert | `726d355b` |
| step 9 B1q `doc_builds_seq_items_while_writing` (lib) | `Doc::Seq` collects every element before writing (the old whole-array shape) | FAIL; the cli-b fingerprint byte tests PASS under the same break (they pin bytes, not memory), as expected | pass after revert | `726d355b` |
| step 9 final r1 F9-S1 `query_projection_streams_its_facts` (lib, through `project_fingerprint_query_full`) | `families_doc` collects a resource's fact JSON (`Vec<Value>`) before its `Doc::Seq` | FAIL: `fact 1 is built after fact 0 is written: [641, 641]` | pass after revert (lib+cli 3218/3218; `ci-steps task` rc 0) | `7ca8f996` |
| step 9 final r1 F9-S2 `routine_query_releases_its_tree_before_det` (cli, shipped binary's trace, both universes) | `prepare`'s `*tree = None; note_release(Tree)` moved after the det fill | FAIL: `["snap","rel:Raw","ctx","rel:Tree",…]` ("the tree outlives det") | pass after revert | `7ca8f996` |
| step 9 final r1 F9-C1 `only_proves_plan_counts_the_gaps_in_its_cone` (lib, the view's effects) | `true` in place of `plan.gaps_in_cone` at `prepare`'s `digest_effects` call | FAIL: digest arm `Some(0)` vs `None` | pass after revert | `7ca8f996` |
| step 9 final r1 F9-O4 `failing_routine_selector_composes_no_tree` (cli) | a failing selector sent down the tree path (`resolution.or_else(\|_\| Ok(empty pool))`) | FAIL: `(Some(0), 1, 1)` vs `(Some(2), 0, 1)` | pass after revert | `7ca8f996` |
| step 9 final r1 F9-O4 `fingerprint_query_scoped_equals_unscoped`'s count arm (lib, every r0 dir + 4 dependency fixtures, both universes) | `selector_failure_result` without its primary filter (`\|\| true`) | FAIL: 7 cross-app dirs (e.g. `ws-cross-object-chain` 35 off the model vs 25 off the tree) | pass after revert | `7ca8f996` |
| step 9 final r1 F9-C5 `selector_ambiguous_candidates_are_deterministic` (lib, now on `resolve_routine_selectors`) | candidates `.rev()` | FAIL | pass after revert | `7ca8f996` |
| step 9 final r1 F9-C8 `tree_read_lifecycle` (b), `release_all_but_keeps_only_the_named_cells`, `views_release_their_cells_before_the_next_build` | `Cell::TreeRead` dropped from `Cell::ALL` (no special case) | FAIL all three: lifecycle at "(b) the untaken result is dropped" (line 1502; the line-865 panic is (a)'s caught one), the released mask, the policy/fingerprint/digest/prove/events sequences | pass after revert | `7ca8f996` |

## Findings register summary

## Gate results

| Gate | Head | Result |
|---|---|---|
| `scripts/ci-steps task-affected` | `72abd499` tree | 3714/3714 pass, 8 skipped |
| `scripts/check-goldens` (no regen) | `72abd499` tree | 951/951, no golden moved |
| `scripts/ci-steps clippy` (release) | `72abd499` tree | clean |
| check-diff | `72abd499`, `2a4a867f` | `reasons: []` |
| final r1: executor `ci-steps task` | `81e5ec6c` | rc 0, 4031/4031 pass, 8 skipped (`logs/ci-steps-task/20261024T084930-ba01a07b.log`) |
| final r1: `CHECK_GOLDENS_FORCE=1 scripts/check-goldens` | `81e5ec6c` | rc 0, 1089/1089, no golden moved |
| final r1: check-diff `--base 9bd2996f` | `81e5ec6c` | `reasons: []` |
| final r1: check-diff `--base origin/master` | `81e5ec6c` | origin/master moved to `83ba322a` (#239): reasons list only #239's own `.agent/issue-239/*` and `scripts/agentflow/budget.py`; needs a rebase before merge |
| final r1: CR-2 CDO cross-app `analyze` peak (VmHWM KiB, 3 rounds, min/median/max) | parent `0b0b95c9` / old head `72a30b67` / new head `8ba75e0d` | 3,111,080/3,133,272/3,157,644 · 3,130,276/3,136,376/3,142,660 · 3,136,392/3,146,380/3,165,060: inside the parent's own 46.6 MB spread, no measurable change (`/work/bench/160-6/final-r1/out/`) |

## Task 1 STOP: the stop rule's reading (spec §8.4)

Bench `/work/bench/160-6/task1-measure.sh` (slot job, load gate passed 05:27), output `/work/bench/160-6/task1/m/`;
P = `7afa91a3` release-fast (`parent220/bins.sha`), probe = `2a4a867f` aldump (`task1/bins/bins.sha`). CDO clean.
Deviation: `o` ran once (smoke run 03:18 + bench run), capped at 30 min by the conductor. Load ticks > 8 in part 2
come from the probes themselves (18+ cores busy) and from another session after 06:10 (load 81); no bound below is
within 100x of its threshold, so no void check can flip it.

| Term (ms, median of 3 / 5) | d | o |
|---|--:|--:|
| K (inputs + compose) | 6,768.6 (262 + 6,507) | 15,000.8 (264 + 14,737; smoke, 1 run) |
| T (compose_tree) | 2,849.7 | 26,605.1 (smoke) |
| E′ (effects) | **147,893.7** (146,435-154,306) | **2,096,212** (smoke); bench run killed at 1,804 s, unfinished |
| S_P (parent shape, 5 runs) | 2,901.4 (2,832-3,342) | 5,996.7 (5,895-6,630) |
| D_P (parent det cone, 5 runs) | 6,894.9 (6,219-15,516; run 5 under load) | — |
| parent band | d wall 21.4 s, band 0.2 s (0.9 %) | `analyze.total` 17,159.6 ms, band 602.6 (3.5 %) |
| probe peak (KiB) | 4,030,680 | ≥ 22,884,328 (killed) |
| parent whole-command peak (KiB) | 3,128,520 | 3,133,624 |

- Round rule: both parent bands ≤ 5 % → Part 2 would run 5 rounds, band = max − min.
- **S1 d**: K − D_P + T + E′ − S_P = 6,768.6 − 6,894.9 + 2,849.7 + 147,893.7 − 2,901.4 = **147,715.7 ms** against
  Bd = 200 ms. **FAILS** (~740x).
- **S1 o**: K + T + E′ − S_P ≈ 15,000.8 + 26,605.1 + 2,096,212 − 5,996.7 = **2,131,821 ms** against 0.5 × Po =
  8,579.8 ms. **FAILS** (~250x).
- **S2**: d 4,030,680 / 3,128,520 = **1.29x** > 1.03x, FAILS; o ≥ 22.9 GB / 3.13 GB = **≥ 7.3x**, FAILS. Policy
  composition peak 9,433,852 KiB < parent whole `policy check` 14,324,904 KiB: F13 not pre-decided.
- `PD_EDGE_UNMAPPED` 0 on d and o (smoke o completed; anchorless_with_links 0). `non_primary_members` 0 on d, o,
  policy. `dependency_calls` 69,013, ≥ 40.8 MB (lower bound) < 50 MB: F12 not pre-decided. Single-app body-less rows:
  0 (D6 cannot move single-app output).
- **Root cause** (diagnostic counters in an uncommitted scratch build, `/work/bench/160-6/diag-wt`, log
  `.agent/runs/20261023-000639-b5e936/logs/bench/20261023T062430-5a135bd7.log`): E′ is the per-fact witness
  reconstruction in `digest_one_root` (`reconstruct_witness_paths`), not the ordering engine. For d's 3 roots the cut
  tree holds 236 facts (138 effect facts → 138 witness BFS, ~0 s); the cone-graph tree holds 17,721 inherited facts
  (15,983 effect facts → 15,983 witness BFS, 349 s CPU, 656,898 paths, 4,627 truncated or limit-hit; ordering 0.9 s).
  So ~116x more effect facts per root (every dependency write/commit the root reaches through base-app code) times
  ~20 ms per BFS over a pd closure of 804,891 states (cut: 76,101 total). `o` is the same function at scale: 8.48 M
  raw facts over 5,479 subjects, 1,568 kept roots (cut: 1,350 roots, 60,075 effect facts, 19,880 BFS, 8.9 s).
- **Fallbacks**: F2 gates the raw walk by ingredient but keeps the kept-root set and their facts (T3), so E′ is
  unchanged by construction; F11 removes only the `direct=` part of K (~0.26 s); F12, F13 are not triggered; F5, F6
  do not apply. **None addresses the witness-reconstruction cost. Per §8.4 the claim stops at Task 1** (scope type,
  split output and builder merged behind the old wiring, nothing switched); discovery below; Task 2 not started.

## Discoveries

```json
[{"subsystem": "l5 / digest (rule-B boundary groups)", "locator": "src/engine/l5/digest.rs boundary group conditionality (spec §14.3.5, A2-03)", "symptom": "Every rule-B boundary group is conditionality `unknown`, even when the boundary call itself sits behind a primary-side guard. On CDO `d` the parent graded 18 such effects (guarded-by-IsHandled 14, error-path 4; Base Application tables); on the cone graph all 998 effects are `unknown`. Operator accepted this for #160 (2026-10-23) and asked for this follow-up. Boundary occurrences also stay in the MAY pending-write check: a boundary write dominating an IO/UI op yields WRITE_PENDING_AT_*, a dominance test on a write the callee only may perform.", "kind": "enhancement", "origin_issue": 160, "pre_existing": false, "reproducer": "alsem digest <CDO> --file src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al --format json: parent (5b8d198f) vs #160 step 6 head; jq conditionality histogram.", "capability": "a group inherits the conditionality its primary prefix proves (guarded-by-IsHandled, error-path, conditional) and stays unknown otherwise; never a must-guarantee from inside the dependency", "acceptance": "the 18 CDO `d` effects regain their parent grade as groups; a fixture with an IsHandled-guarded boundary call pins it (break: force unknown); no group ever reports unconditional-on-success"}, {"subsystem": "l5 / fingerprint B0 and gate/diff (cross-app)", "locator": "src/engine/l5/fingerprint_cli.rs B0 path (compose_full_snapshot with SnapshotUniverse::SingleApp) and src/engine/gate/diff/cli.rs load_snapshot_from_workspace (#160 step 9 spec, OD-1 = S')", "symptom": "Epic #160 row 9 is delivered for prove, fingerprint --routine and fingerprint --inventory-only only. Fingerprint's full snapshot (json with no query flag, cbor, cbor.gz, --shard), the query forms with no --routine (human with no flag, --roots alone) and diff stay single-app. Measured 2026-10-24 (/work/bench/160-9/probe-out/OD1-RESULT.md): cross-app B0 on CDO holds 8,483,157 capability facts and was OOM-killed by the shared 32 GiB cgroup at >= 29,113,888 KiB before it serialized (single-app: 101,776 facts, 1.47 GB peak, 130 MB JSON). The pre-draft's D9 fold does NOT help: folding dependency-carrier facts by (subject, op, resourceKind, resourceId, dispatch) keeps 97.65 % of the facts (>= 97.48 % under any carrier attribution), because the key holds the subject and the facts are already nearly distinct per subject. diff ws ws cross-app (P3) was not run: it would compose two such trees.", "kind": "enhancement", "origin_issue": 160, "pre_existing": false, "reproducer": "On the #160 step 9 head: alsem fingerprint <CDO> --format json reports analysisUniverse single-app while alsem fingerprint <CDO> --routine R reads the cross-app view. The probe patch /work/bench/160-9/probe-out/p2.patch reproduces the OOM (run it alone; it needs more than 29 GB).", "capability": "fingerprint B0 (every format and --shard), the wide query forms and diff build the cross-app view by default, with --single-app restoring today's bytes, within a stated peak bound that fits beside other work in a 32 GiB cgroup; cli-b B0 headers read cross-app; the dependency-shard output move and fixture (c) (dependency-mediated capability-gained in diff) are delivered", "acceptance": "a design that does not depend on a subject-keyed fold (for example a snapshot streamed from the composition, or a fact form that shares dependency-side facts across subjects), with a measured CDO peak and output size before the claim; fixture (c) passes with a discrimination break; no-dependency parity; --single-app byte-identical"}]
```

## Not delivered

- PARKED (2026-10-23 ~08:00 UTC, operator): Task 1 stop rule failed (d E′ 148 s, o > 35 min / ≥ 22.9 GB; root cause per-fact witness search). Witness redesign prototyped (BFS predecessor DAG, CLRS §22.2, uncommitted in /work/bench/160-6/diag-wt): d E′ 148 → 6 s, still over bounds; o cannot fit. Operator chose the SPLIT (digest-only step 6, ordering stays on the cut graph, epic amendment 6A-12) — not yet written. Then parked for the CentralGauge demo (2026-10-27): this session switches to #156/#157 for a 1.7.x release. Branch pushed at 2a4a867f; ledger/register uncommitted in this worktree (backup /work/bench/160-6/parked-ledger/).

- RESUMED (2026-10-23 ~12:05 UTC, attempt 8, run 20261023-120326-46d656): operator ran `unblock 160` after v2.0.1. Worktree from master 6a2b7c8b; the parked branch a7 (10266644..2a4a867f) cherry-picked (one conflict in capability_cone.rs: kept Task 1's ConeOutcome doc, dropped #221-era ConeResultPub); Task 1 commit now edca9f0c (its subject line was lost in the conflict continue; body intact). Known follow-up: step6_probe.rs needs #78's new OwnerIndex argument to resolve_changed_roots. Next: the digest-only split amendment (6A-12, pre-drafted in /work/bench/160-6/split/), one spec-panel round (1 left), then Tasks 2-4.
