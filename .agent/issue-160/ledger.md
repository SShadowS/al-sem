# Issue #160 ledger: attempt 3 on server1 (epic step 7b: closed-class sets, fold memo)

## Claim

```json
{"run_id":"20261021-185406-413c0c","issue":160,"attempt":3,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/160-alsem-batch-build-the-model-onc-a3","worktree":"/work/al-sem-issue-160-a3","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","reconciled":[]}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`.
Base `afd5d88f4ceb9c4457b992d58b6b1237c4461c02` (make-worktree, origin/master; includes the flow speed-ups #194).
Operator (2026-10-21): #160 is highest priority; take epic step 7b next. Reviewers: the operator's standing
stand-ins, gpt-6.1-sol (slot astra) and Opus (slot flash); attest with `--substitute astra=sol --substitute flash=opus`.
Loop budget: claim 3 of 10.

Earlier steps' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170), step 1b
at `6c6bce17` (PR #172), step 2 at `d4d8b71f` (PR #174), step 3 at `c554b704` (PR #178), step 4 at `c186f4d1` (PR #180),
step 7a at `617b402f` (PR #181). This file and `findings.json` replace them for this step.

Part of #160. This PR delivers epic step 7b only (spec
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.6 and §9, row **7b**). #160 stays open
until its last step (owner decision, 2026-10-18): the PR says "Part of #160" and carries no closing keyword.

## Classification

**Architectural.** Epic §9 row 7b: a `FactKeyInterner`, a hash-consed `SetArena`, the closed/open split, a
per-`SetId` fold memo and `ConeDerivedBuilder::fold_row`, replacing the per-fact fold for closed entries in
`compose_inherited_cones`. New data structures across the cone engine, a byte-neutral obligation (7a's oracle, every
golden unchanged) and a pre-registered drop rule (X ≥ 2 × band, X derived before code). It gets a child spec, as
steps 3, 4 and 7a did.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 master moved since 7a's bench: is 7a's band still the right one? | NO, re-measured. `afd5d88f` differs from 7a's head `7bb9f90c` by a grammar bump (8682cb5f lowerer arms, `crates/al-syntax`, `cache_prune.rs`); no cone code moved. CDO shape unchanged: routines 31,658, singleton calls 30,942, fact_cone x28,191, 714 recursive members, as at 7a | `git diff --stat 7bb9f90c afd5d88f -- src crates`; `/work/bench/160-7b/bench-c/c-base-*.err` |
| P2 cone-span band at base (census OFF, 5 rounds, run `a`) | median 10,168.7 ms, band **1,502.2** (one spiked run, 11,647.3; the other four 10,145.1-10,188.3). Probe binary census-off is inert: 9,992.9-10,751.5 | `/work/bench/160-7b/bench-a/`, `bench-a-spans.txt`; load 2.86 → 6.08 (the bench's own threads; 22 cores) |
| P3 timer band at base, Q = `singleton` + `bfs_fold` (census ON) | reading 1 (5 runs): median 5,268.5, band 4,241.6, from ONE spiked run (run 5: every timer ×1.7-2.2, `pd_solve` 5,857 against 2,626-2,846, which 7b does not touch); without it 735.9. Reading 2 (6 runs, same binary twice): median 5,377.3, band **472.1**. 7a's head (`7bb9f90c`): band 573.2 | `/work/bench/160-7b/bench-c/c-base-*.err`, `bench-c2/c-*.err`; load 6.08 → 5.57 (r1), 3.54 → 4.47 (r2) |
| P4 the old work: scan + fold at base, median of reading 1 | `scan_cone + scan_own + fold` 4,269.3 ms (band 2,819.6 with the spike; 394.1 in reading 2, median 4,438.8); `bfs_fold` 389.8; plus 386-423 ms of singleton time outside the sub-timers (`own_keys`, one `format!` per own entry, 1,681,935) | same files |
| P5 X, derived (throwaway census patch, `/work/bench/160-7b/probe/probe.patch`, built release-fast, run census ON interleaved with base, 5 rounds; reverted, `git diff -- src crates` empty) | per-callee-SCC memo, singleton head work `memo + row + open + own` median **789.2** ms (788.5/888.9/789.2/852.7/786.5); recursive set fold once per SCC + per member `fold_row` + own: median **41.0** ms. **X = (4,269.3 − 789.2) + (389.8 − 41.0) = 3,480.1 + 348.8 = 3,828.9 ms.** Conservative: the probe re-ran `substitute_entry` inside `open` and ran cold (before the real path); the 386 ms `own_keys` overhead is not counted | `bench-c/c-probe-*.err` (`probe7b:` lines) |
| P6 feasibility: X ≥ 2 × band | YES against the cone-span band (3,828.9 ≥ 3,004.4, spike included) and the timer band of reading 2 (≥ 944.2). NO only against reading 1's timer band with its spiked run (8,483.2) | P2, P3, P5 |
| P7 does a memo key determine the folded row (closed entries)? | YES. `fold_fact` reads only `resource_kind`, `op`, `resource_id`, known-temp (`cone_derived.rs:736-783`), all in `inherited_fact_key`; a closed entry has no guard (`frame_bound` = `guard.is_some()`, `capability_cone.rs:1537`); `fact_cones[j]` is inserted once (`:2597`) and never mutated until freed. Probe: memo rows + open + own + self = the real row for **31,656 of 31,656** rows, mismatch 0, every run | `probe7b pop: … checked=31656 mismatch=0` |
| P8 is the epic's interner + hash-consed SetArena needed? | NO. 25,498 memo builds (3,879,321 closed entries) against 10,584,764 per-best folds today; distinct closed sets among them 14,438 (hash-consing saves ≤ 11,060 builds, ≤ ~220 ms, below every band, and needs a key interner); distinct per-singleton union sets 7,825 of 15,838, but `fold_row` costs 5.8 ms in total. Memo key = the callee Cone-SCC index | `probe7b pop:` |
| P9 closed vs open split | closed 12,610,531 scanned today vs open (frame-bound) 152,230 (1.2 %); 109,826 frame-bound entries skipped by memo builds; open stays on `substitute_entry` | `singleton pop:`, `probe7b pop:` |
| P10 recursive sets | Σ SCC set size over 67 SCCs = 67,491, against 4,292,741 set entries folded per member today (98.4 % fewer folds); own entries 757,002 stay per member | `probe7b pop: rec_set_entries`, `cone_scc: set_entries` |
| P11 memo memory (build-local, Decision 32) | live max 1,568 rows / 836,418 ids (≈ 4 MB), freed with the cone it mirrors | `memo_live_max`, `memo_live_ids_max` |
| P12 duplicate edges | 51,095 cone-bearing edges, 42,912 distinct (subject, callee SCC): 8,183 duplicate cone reads today | `cone_edges`, `distinct_j` |
| P13 ordering bits (Decision 38) in the row? | NO. `ConeDerivedRow.flags` holds 5 presence bits (`cone_derived.rs:111-123`); the ordering bits live in step 2's ordering cell (`detector_context.rs:1044`). 7b does not touch them | grep |
| P14 census text pinned by tests? | only the `cone_scc:` line (`census_cone_scc_line_counts_the_recursive_sccs`); the `singleton:` line is unpinned | `grep -rn 'scan_cone\|singleton:' tests` empty |
| P15 X as a projection of the COMPLETE new region (panel r1: sol #3, opus M1/M2). Probe 2 (`/work/bench/160-7b/probe2/probe2.patch`, reverted, `git diff -- src crates` empty); `bench3.sh` under `agentflow run --name bench`, base vs probe2, census ON, 5 rounds, 338.5 s, exit 0, load 1.82 → 3.90 | base Q = `singleton + bfs_fold`: 5,279.5 / 5,372.5 / 5,238.4 / 5,219.4 / 5,376.8, median **5,279.5**. Projected head Q = `sgl_full` (J collect + dedup, memo builds, `fold_row`, a fresh subst + open fold, own fold) + `drop` (memo destruction) + `rec`: 892.4 / 1,063.5 / 873.4 / 897.4 / 952.4, median **897.4**. **X = 5,279.5 − 897.4 = 4,382.1 ms** (supersedes P5's 3,828.9 sub-timer estimate). Probe finalisation (self half + `finish_routine`, outside Q both sides) 116.8-122.5 ms | `/work/bench/160-7b/bench-c3/c-*.err` (`probe7b full:` lines) |
| P16 complete rows equal (sol #5) | 7a's comparator fields (row-id sets, flags, four lists, `physical_table_write_ops_of` masks): **31,656 / 31,656 rows equal, row-id sets equal, all 5 runs** | `checked=31656 mismatch=0`, no `ROW IDS differ` line |
| P17 hash-consing, entry-weighted (sol #4, opus I7) | duplicate memo builds 11,060 fold 2,047,522 of 3,879,321 entries (52.8 %); their time 304.1-403.3 ms (median 314.4); hashing every new closed set (`DefaultHasher` over key strings) 182.1-198.6 ms (median 183.4). Net gain ≈ 131 ms. The P8 "≤ ~220 ms" is withdrawn | `dup_memo`, `dup_entries`, `hash` |
| P18 `own_keys` alone (sol #11) | 301.5-380.7 ms per run (median 310.5); the rest of the old singleton remainder (382.2-742.3 ms per base run in reading 1) is dropping `best` and loop overhead | `own_keys=` |
| P19 singleton out-degree (sol #16) | max out-degree 292, max distinct callee SCCs per subject 156 | `max_deg`, `max_j` |
| P20 `resource_id == Some("")` (sol #17) | **0** among closed memo entries, substituted open entries and own entries | `empty_rid=0` |
| P21 Q band, reading 3 (5 base runs, P15's base side) | 157.4 (5,219.4-5,376.8); memo timer at probe 573.2-701.8 ms | P15 files |
| P22 does step 0 record the closed/open split? (sol #14 vs opus E5) | YES for populations and scan time: `SGL_CLOSED/OPEN/OWN_ENTRIES` 12,498,190 / 151,140 / 2,420,676 and `scan_cone` 2,669.3 ms (`docs/measurements/2026-10-18-issue-160-step0.md:118`, `:137`). It records NO new-path cost (distinct callee sets, memo build cost), and ran on Windows at a pre-7a base. Sol is right on the fact; 7B-4's reason is restated | grep of the step-0 doc |
| P23 can a test fixture produce `resource_id == Some("")` (the new `fold_set` `debug_assert!`; opus r2 "not checked") | NOT FOUND by reading the code. In production, a fact's `resource_id` comes from `op.table_id` (`capability_cone.rs:609`, model table ids built as `{app_guid}/table/{n}`, `workspace.rs:592`, assigned in `record_types.rs:138-248`) or `evt.id` (`:1092`); every other site passes `None`. No hand-built test fact in `src/` or `tests/` passes `Some("")` as a resource id: the `Some("")` hits are routine-id parts (`ids.rs`), record-variable names (`table_display.rs:205`), dependency fields (`dependencies.rs:1005`) and a regen flag (`tests/common/regen.rs:68`). The first `cargo test` run with the assert is the real check | `grep -rn 'Some("")' src tests`, `grep -n 'resource_id' capability_cone.rs` |
| P24 r2 S1-7: a T7 fixture with no singleton memo build | `tests/r0-corpus/ws-d7-event-cycle` has one file, `event-a.al`, with 6 procedures. Sol says all 6 are recursive event participants, so `closed_memo_builds` can legitimately be 0 per composition; the spec asserts `> 0` only summed over T7's corpus (and on CDO cross-app). I did not check the fixture's SCC structure | `ls`, `grep -c procedure` |

## Design

(child spec: `docs/superpowers/specs/2026-10-21-issue-160-step7b-closed-sets-fold-memo-design.md`)
Feasibility (r1, superseded): X = 3,828.9 ms ≥ 2 × band (cone span 1,502.2; timer 472.1 on the quiet reading).
Feasibility (after panel r1): X = 4,382.1 ms (P15, a projection of the complete new region) ≥ 2 × every spec-time band
except reading 1's disturbed Q band; 7b is NOT dropped. Conductor decisions written into the spec: §8 bounds take bands
only from the final bench's parent runs; a void bound is re-run once (Part 2) and then FAILS → drop; a missed bound →
drop; an unnamed T8 mismatch class → drop; final oracle = `DerivedOnly` vs streamed F(`RawOnly` ∪ `direct_raw`).
Amendments asked: 7B-1 (no interner/SetArena: memo keyed by callee Cone-SCC), 7B-2 (bounded timer Q = `singleton`
+ `bfs_fold`), 7B-3 (cone bound subtracts both bands), 7B-4 (X derived at this step's base, not step 0).

## Base/Head SHAs

- Claim base `afd5d88f4ceb9c4457b992d58b6b1237c4461c02` (make-worktree). Rebased onto `fd77939d` (#197-#201) after Task 4,
  then onto `4f5d4c1b` (#202) after the bench; both rebases clean, `git range-diff` all `=` (the first moved only Task 2's
  CHANGELOG context), so no extra panel round.
- **B** = `4f5d4c1b5495b1d05a624aecf7aeb744720c14d8` (origin/master at step 11, unmoved at step 12).
- **H** = `af823c6c98543cc3c6685482c6f2c422bde4156b` (the gated head; `9ac7c4a6`'s rebased image).

## Timeline

- make-worktree: `{"worktree": "/work/al-sem-issue-160-a3", "branch": "issue/160-alsem-batch-build-the-model-onc-a3", "base": "afd5d88f4ceb9c4457b992d58b6b1237c4461c02", "target": null}`
- classify: architectural
- post-7a baseline read from `/work/bench/160-7a/bench-c/c-head-1.err` (7a's head `7bb9f90c`, CDO cross-app
  `analyze`, release-fast): compose 10,061.7 ms = singleton 4,914.4 (scan_cone 2,794.6, fold 1,374.6, scan_own 193.0,
  subst 129.0) + fact_cone 1,564.8 (x28,191) + bfs_fold 394.0 + derived_fold 208.5 + untimed pd_solve 2,705.2 + rest.
  7a's retained run dir (`~/.al-sem/agentflow/runs/20261020-113707-ad56ee`) is gone after the tentacle rebuild (the
  home directory is not kept); the bench files under `/work/bench/160-7a/` survived.
- probes (2026-10-21 19:00-19:21): built base `afd5d88f` release-fast (`/work/bench/160-7b/base/alsem`); applied a
  throwaway census patch (per-callee-SCC memo + per-SCC set fold, timed beside the real path, rows compared), built
  `/work/bench/160-7b/probe/alsem`, saved the diff to `probe/probe.patch`, reverted (`git diff -- src crates` empty).
  `agentflow run --name bench` (exclusive slot) × 2: `bench.sh` (run `a` census off + run `c` census on, base vs
  probe, 5 rounds each, 679.7 s, exit 0) and `bench2.sh` (base twice, census on, 3 rounds, 193.7 s, exit 0). Load
  avg 2.9-6.1 during the runs. Spec written; nothing committed.
- spec panel r1 (sol 10 important + 8 minor; opus 1 critical + 7 important + 8 minor). Probe 2 (19:30-19:44): re-applied
  the probe patch, widened it (whole-region timer, memo-drop timer, `own_keys` timer, entry-weighted duplicate builds
  and hash cost, max out-degree, empty-rid count, 7a's full comparator incl. masks and row ids), built
  `/work/bench/160-7b/probe2/alsem`, saved `probe2.patch`, reverted (`git diff -- src crates` empty); `bench3.sh` under
  `agentflow run --name bench` (338.5 s, exit 0, load 1.82 → 3.90). X re-derived as 4,382.1 ms (P15). Spec revised for
  every holding finding. Nothing committed.
- spec panel round 1 counters (spec_rounds 1/5, pi_calls 1, subagents 3). Conductor decisions for the fallbacks:
  void twice = speed acceptance fails = drop; missed = drop; unnameable CDO mismatch = drop; final oracle DerivedOnly
  vs F(RawOnly ∪ direct_raw), streamed per subject (16 GiB container); a recorded T8 CDO run before PerMemberWalk's
  deletion. Opus E5 refuted (step 0 records the split). Register S1-1..S1-23, all fixed, anchors per spec section.
- spec panel round 2 (spec_rounds 2/5, pi_calls 2, subagents 3): continued threads, new spec + register ids. opus
  accepted all 23 + N1 (S2-1, pre-warm omission); sol accepted 18, re-raised S1-7/8/10/16/19. All fixed (P23: no
  fixture yields `resource_id == Some("")`, by reading the code; P24). Spec 692 lines.
- spec panel round 3 (spec_rounds 3/5, pi_calls 3, subagents 3), ids S1-7/8/10/16/19 + S2-1 only: both accepted all
  six, no new findings. **Converged**: 24 entries, all fixed, all accepted by both.
- step 6 acceptance tests: opus subagent dispatched (subagents → 55), red and uncommitted.
- step 7 plan `docs/superpowers/plans/2026-10-21-issue-160-step7b-closed-sets-fold-memo.md` (drafted by the spec
  author, subagents → 54): 4 tasks — T1 the switch + acceptance tests (one commit), T2 CHANGELOG + numbers skeleton
  (docs; the "commit before the deletion"), T3 the recorded `cdo-gate` oracle run (no commit; licenses T4), T4 delete
  `RecursiveMembership` (the enum, since `PerScc` would be its only value) and install T7'/T8'. `charge plan_tasks` × 4
  → remaining 6. Accepted: the bench-numbers docs commit follows T4 (gate-inert, after H); the CHANGELOG entry is
  reverted with the code if 7b is dropped.
- pre-work (operator's request, 2026-10-21): two read-only opus agents draft step 10 and step 11 child specs under
  `/work/bench/160-10/`, `/work/bench/160-11/` (no CDO runs, no builds, while 7b benches). Both drafts done
  (`draft-step10-design.md`, `draft-step11-design.md`, each with `probes-todo.md`). The step-10 drafter found
  `read_rss` is a stub on Linux (`perf_trace.rs:175`), so the trace's memory peaks read 0 on server1 — recorded as a
  discovery here.
- Task 1 committed `1101e9d9` (switch + step-6 tests + spec + plan); 11 discrimination proofs (T1-T7, T9), all red on
  the break and green on the revert; T8/T8' green on CDO. `check-diff` reports the stub's `#[allow(` → stopped.
- Task 1 follow-up `e50026ce` (conductor's order): `ComposeOptions { mode, membership, raw_sink }` replaces the
  `#[allow(clippy::too_many_arguments)]` on `compose_cone_over_graph_with`; production passes
  `ComposeOptions::production(mode)` (`PerScc`, no sink). Signature refactor only; the test helpers `compose`,
  `compose_model`, `check_model` changed only their call (no assertion changed), so no proof re-run.
  `ci-steps task-affected` exit 0 (3598 passed); `ci-steps clippy` (release, `-D warnings`) exit 0; `check-diff`
  `{"reasons": []}`.
- Census note for Task 2's CHANGELOG: the `singleton calls:` and `singleton pop:` lines are now counted only where a
  singleton scans `best` (the raw half, `RawOnly`/`Both`, or the test oracle), so under `analyze` (`DerivedOnly`) they
  read 0. The memo path's populations are on the new `closed:` line.
- Task 1 review minors `93ccd999` (tests only): T2 exact counts (builds 1, reads 2, entries 0, as predicted), T3
  asserts its two-edge precondition on `build_typed_edge_graph`, T5 drops its memo-vs-memo rows comparison. Proofs
  re-run for T2 (3 breaks) and T3 (2 breaks). `task-affected` exit 0 (3598 passed); `check-diff` `{"reasons": []}`.
- Task 2 `a91ffae6` (docs only): CHANGELOG entry (incl. the 0-reading `singleton calls:`/`singleton pop:` lines, the
  `closed:` line, `PerMemberWalk` deleted at the step's end) and `docs/measurements/2026-10-21-issue-160-step7b.md`
  skeleton (`<bench>` placeholders). `task-affected` exit 0; `check-diff` `{"reasons": []}`. This is the commit
  Task 3's recorded `cdo-gate` runs on; `git status --porcelain -- src tests crates` empty.
- Task 3 (no commit): conductor's `cdo-gate` at `a91ffae6` PASS; T8/T8' alone on CDO with RSS, both CDO breaks red
  (no LIMIT needed for T2's class). Recorded under Gate results; licenses Task 4.
- Task 4 `9af9c15d`: `RecursiveMembership`/`PerMemberWalk` deleted (enum, arms, parameters, `ComposeOptions.membership`);
  T7/T8 removed, T7'/T8' the only oracle; spec §4.7 table migrated (`derived_and_raw_fold[_model]`, T10, 7a T5 reads
  `member_walks == 479` off RawOnly). 4 proofs (own-entry fold, T2 class, DerivedOnly scan, 7a T5 walk). T8' RSS
  3,836.2 MiB. `check-diff` vs origin/master now flags master's own new commits (base moved; rebase needed).
- rebase onto origin/master `fd77939d` (#197-#201 merged by the operator; `--autostash` for the tracked evidence
  files): one CHANGELOG conflict (both sides added under Unreleased → both kept). `git range-diff afd5d88f..9af9c15d
  origin/master..HEAD`: Task 1 `1101e9d9`→`078639a7`, `e50026ce`→`0f53300a`, `93ccd999`→`7889e07f` and Task 4
  `9af9c15d`→`12c441e6` all `=`; Task 2 `a91ffae6`→`1123f330` differs only in CHANGELOG context lines. So Task 3's
  recorded `cdo-gate` run on `a91ffae6` is the same patch as `1123f330`. `check-diff --base origin/master` `[]`.
- task gate `ci-steps-task` on `12c441e6`: exit 0, supervised, 59.3 s
  (`logs/ci-steps-task/20261021T213429-280610f9.log`). Sonnet review of Tasks 2+4 dispatched (subagents → 51).
- push-branch head `12c441e6`; draft PR #203.
- Sonnet review of Tasks 2+4: no critical/important; minors (stale T7/T8 comment names, two doc wraps, CHANGELOG
  sentence stale after the deletion, the numbers file's Parent) fixed in `e8fd2788` (comments/docs only;
  task-affected exit 0; check-diff `[]`). The rebase licence was judged sound (`git diff a91ffae6 1123f330 -- src crates
  tests Cargo.* scripts/ci-steps scripts/cdo-gate` empty).
- final panel round 1 (final_rounds 1/5, pi_calls 4, subagents 50 left): code-review skill at high (forked) + sol +
  opus over `origin/master...e8fd2788`, full briefing. Code-review: no correctness bug, 10 findings (efficiency, docs,
  tests). sol: 1 important (B1 timers) + 4 minor. opus: 0 critical, 2 important (Part 3 rounds vs Decision 58; RSS
  methodology) + 7 minor. Fixed in `0c08452b` (code), `10bc2a14` (docs), `f3ccb89c` (spec/plan sync); register F1-1..F1-14
  (F1-11 deferred minor = discovery; F1-12, F1-14 refuted). Task gate on `f3ccb89c`: exit 0, 27.2 s
  (`logs/ci-steps-task/20261021T220052-bed7c8e5.log`). Pushed `f3ccb89c`.
- final panel round 2 (final_rounds 2/5, pi_calls 5, subagents 49 left), diff `e8fd2788..f3ccb89c` + ids F1-1..F1-14 in
  each reviewer's round-1 thread: both accepted all 14, no new findings. **Converged**: 38 entries, all accepted by both,
  no blocking entry deferred.
- bench (spec §8): dispatched to the spec author under `agentflow run --name bench` (one supervised run, exit 0, 1,601.1 s,
  log `logs/bench/20261021T221142-1ee49d0a.log`, raw files `/work/bench/160-7b-final/`). Void checks hold; **B1 met**
  (Pq − Hq = 5,473.8 − 1,056.7 = 4,417.1 ≥ 3,641.5), **B2 met** (Pc − Hc = 11,601.5 − 6,365.4 = 5,236.1 ≥ 2,930.0), the
  literal epic form met (≥ 3,670.6); head lower in 5/5 on both. Part 1 identity holds; Part 3 `policy check` 107.1 →
  93.8 s (the `Both` fallback does not fire). Numbers committed `9ac7c4a6` (CHANGELOG + numbers file only).
- step 11: rebase onto `4f5d4c1b` (#202), clean, `git range-diff` all 10 commits `=` → no extra panel round. Full set on
  `af823c6c` (see Gate results), all exit 0, supervised; tree clean outside `.agent`.

## Acceptance matrix

| Item | Delivered by | Status |
|---|---|---|
| A7b.1 `FactKeyInterner`, hash-consed `SetArena` | amendment 7B-1 (memo keyed by callee Cone-SCC; net gain of hash-consing ≈ 131 ms, P17) | not delivered as named — by the accepted amendment |
| A7b.2 closed/open split | memo rows for closed entries, `substitute_entry` + `fold_fact` for open; T2 | delivered |
| A7b.3 per-SCC memo | T1 (2 builds, 3 with the memo off), T3, T6 | delivered |
| A7b.4 `fold_row` | T1, T4 (recursive set folded once per SCC) | delivered |
| A7b.5 goldens unchanged | `check-goldens` 910 passed, no golden moved; CDO SARIF byte-identical both scopes; histogram SHA `c032c8db…` equal (bench Part 1) | delivered |
| A7b.6 7a's oracle | T7/T8 widened, green in the recorded `cdo-gate` at `a91ffae6` (= `1123f330` → rebased); then T7'/T8' (DerivedOnly vs streamed RawOnly fold) | delivered |
| A7b.7 X before code, ≥ 2 × band | X = 4,382.1 ms (P15); bench void checks hold | delivered |
| A7b.8 B1 | 4,417.1 ≥ 3,641.5 | delivered |
| A7b.9 B2 | 5,236.1 ≥ 2,930.0 (literal epic form 5,236.1 ≥ 3,670.6) | delivered |
| A7b.10 `PerMemberWalk` deleted | Task 4 (`12c441e6`), after the recorded `cdo-gate` run | delivered |
| A7b.11 E14 CDO ratchets | `cdo-gate` exit 0 on `af823c6c` | delivered |
| A7b.12 retained bytes | `context.cone_derived` 65,273,122 and `context.summaries` 36,400,882 equal both sides (bench Part 1) | delivered |

### Acceptance tests (step 6)

Written against base `afd5d88f` + the stubs below; not committed (red by design). Each fails on the assertion
named, for the reason the spec states; the row assertions in front of it pass. All in
`src/engine/l4/capability_cone.rs` mod `scc_set_tests` unless stated.

Stubs added (all behaviour-neutral for production output):
- `ComposeStats` gains `closed_memo_builds`, `closed_memo_reads`, `closed_memo_entries` (stay 0: no memo yet),
  `scc_set_folds` (counted where today's code folds the set: once PER MEMBER; 7b moves it to once per SCC) and
  `singleton_scans` (`+= 1` at every `inherited_facts_for_singleton` call: every call builds `best` today).
- `compose_inherited_cones` and `compose_cone_over_graph_with` gain `raw_sink: Option<RawSink<'_>>` (new alias
  `RawSink`); at each member's emission a `Some` sink gets the raw Vec and `out` keeps `Vec::new()`. Production
  callers pass `None` (both `compose_inherited_cones` call sites, `compose_cone_over_graph`). New `cfg(test)`
  `compose_raw_streaming` (§4.7) = `RawOnly` + `PerScc` + `Some(sink)`. This is the real streaming entry, not a
  placeholder: a non-streaming stub would hold every raw Vec on CDO (the 10 GB shape).
- Test helpers: `check_model` widened; new `check_model_raw_fold`, `Check` fn type, `check_cross_app_with`
  (replaces `check_cross_app`), `oracle_on_fixtures` / `oracle_on_cdo` (T7/T8 bodies, shared by T7'/T8'),
  `Classes.closed_memo_builds`, `production()`, `T2_7B_SRC`.

| test | file:fn | proves | failing output (short) |
|---|---|---|---|
| T1 | `closed_memo_folds_a_shared_callee_once` | A7b.3/A7b.4: one memo row per callee cone; builds 2, reads 3, entries 3; rows A,B,C = t/C,t/D | `one memo row per callee cone read …` left 0 right 2 |
| T2 | `closed_memo_excludes_frame_bound_entries` | A7b.2: N (G(false)) lacks Cust, M has it; the memo is used (builds ≥ 1, reads ≥ 2) | `N and M read G's cone through the memo (builds 0, reads 0)` |
| T3 | `duplicate_edges_fold_one_row` | A7b.3: J(m) dedup; builds 1, reads 1 | `C's cone, built once` left 0 right 1 |
| T4 | `scc_set_folds_once_per_scc` | A7b.4: set folded once per SCC; Q writes Cust, P nothing (preconditions on `pd_solution` as 7a T3) | `the SCC's set is folded once, not once per member` left 2 right 1 |
| T5 | `derived_only_builds_no_best` | §7 "production stays on the scan": `Both` raw Vecs of A, C, D stated literally, `Both` scans 3; `DerivedOnly` scans 0 (production entry) | `DerivedOnly built \`best\` for a singleton` left 3 right 0 |
| T6 | none (the extended leak `debug_assert!`, §4.4) | A7b.3 memo freed | no test code: the `&& memo.is_empty()` clause lands with the memo |
| T7 widened | `scc_set_matches_member_walk_on_fixtures` → `oracle_on_fixtures(check_model, …)` | A7b.6: PerScc `singleton_scans == 0` per composition; PerMemberWalk memo builds/reads 0; Σ `closed_memo_builds > 0` | panics on `failures`: 480 × `the PerScc side scanned \`best\` for N singletons`; no row diff |
| T7' | `derived_only_matches_raw_fold_on_fixtures` | A7b.6 final oracle: DerivedOnly (production) vs fold of streamed RawOnly ∪ `direct_raw`; `member_walks == 0`, `singleton_scans == 0` per composition; Σ builds > 0 | 480 × `DerivedOnly ran an old path (member_walks 0, singleton_scans N)`; **no row diff on any of 498 compositions** |
| T8 widened | `scc_set_matches_member_walk_on_cdo` → `oracle_on_cdo(check_model, …)` | A7b.6 on CDO; cross-app builds > 0 (asserted after `failures`); oracle lines now print `closed_memo_builds` and `singleton_scans` | cdo profile, 62.0 s cross-app: `scanned \`best\` for 30942 singletons` (cross-app), 5478 (single-app); no row diff |
| T8' | `derived_only_matches_raw_fold_on_cdo` | as T7', on CDO | 74.7 s cross-app: `DerivedOnly ran an old path (member_walks 0, singleton_scans 30942)`, single-app 5478; **no row diff** (31,658 + 5,479 rows); max RSS not recorded at step 6 (no `/usr/bin/time`); recorded later with `getrusage` (see Gate results: Task 1 3,870.8 MiB, Task 3 3,846.3 MiB, Task 4 3,836.2 MiB, final-panel r1 warm binary 3,857.8 MiB) |
| T9 | `tests/cli/cli_build_counters.rs::census_closed_line_counts_the_memo` (`ws-compose`, base `singleton pop: closed_entries=1`) | §4.8 census line exists once, `memo_builds > 0` | `one closed line:` left 0 right 1 |

Untouched and green with the stubs: the other 47 `capability_cone`/`cone_derived` lib tests (incl. 7a T1-T6, T10,
T11, `singleton_walk_*`); dev clippy `-p al-sem --lib --tests` clean. Discrimination proofs (§5 breaks) are for the
implementation step, after the tests go green.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| T1 `closed_memo_folds_a_shared_callee_once` | singleton memo: `if !memo.contains_key(&j)` → `if true \|\| …` (rebuild every read); re-run on the final `Entry::Vacant` shape as `memo.remove(&j)` after `fold_row` | both: `one memo row per callee cone read (C's once for A and B, D's for C)` left 3 right 2 | 1 passed | 1101e9d9 |
| T2 `closed_memo_excludes_frame_bound_entries` | memo build: drop `.filter(\|e\| !frame_bound(e))` (fold_set over every cone entry) | `N: G(false) contradicts the guard` left [`…160003/table/50100`] right [] | 1 passed | 1101e9d9 |
| T3 `duplicate_edges_fold_one_row` | J(m): drop `&& !callee_sccs.contains(&yj)` | `A reads C's row once, not once per edge (the J(m) dedup)` left 2 right 1 | 1 passed | 1101e9d9 |
| T4 `scc_set_folds_once_per_scc` (break 1) | recursive branch: `fold_set` + `scc_set_folds += 1` per member, not per SCC | `the SCC's set is folded once, not once per member` left 2 right 1 | 1 passed | 1101e9d9 |
| T4 (break 2) | set row = `fold_set(set ∪ every member's own entries)` | `P's row` left [`…160003/table/50100`] right [] | 1 passed | 1101e9d9 |
| T5 `derived_only_builds_no_best` | singleton: `if !mode.wants_raw() { return }` → `if false && …` (DerivedOnly falls through to the scan) | `DerivedOnly built \`best\` for a singleton` left 3 right 0 | 1 passed | 1101e9d9 |
| T6 (leak `debug_assert!`) | refcount-free loop: delete `memo.remove(y)`; run T1 | T1 panics at the leak `debug_assert!`: `cone leak: 0 fact cone(s) / 0 coverage cone(s) / 2 closed-set memo row(s) survived the SCC walk` | 1 passed | 1101e9d9 |
| T7 `scc_set_matches_member_walk_on_fixtures` (break 1) | singleton memo path: own-entry fold `for e in own { fold_fact }` → `for _e in own {}` | row diffs, first `ws-d1-multi-caller: r0/1f7597…: writes_tables_of differs` (also ws-d37, ws-d40, ws-e2e, single- and cross-app); T7' and `singleton_walk_folds_own_entries` fail too | 3 passed (T7, T7', singleton_walk_folds_own_entries) | 1101e9d9 |
| T7 (break 2) | `use_memo = wants_derived() && (true \|\| membership == PerScc)` (PerMemberWalk also memoizes) | 261 × `the PerMemberWalk oracle used the memo`, first `ws-argument-bindings: … (builds 4, reads 6)`; 0 row diffs | 1 passed | 1101e9d9 |
| T9 `census_closed_line_counts_the_memo` | `add_compose_stats`: delete `add(&CLOSED_MEMO_BUILDS, …)` | `no memo row built: [cones-census]   closed: memo_builds=0 memo_reads=1 memo_entries=1 scc_set_folds=0 singleton_scans=0 memo_live_max=1` | 45 passed (`cli_build_counters::`) | 1101e9d9 |
| T2 (exact counts, review minor) break 1 | memo build: drop `.filter(\|e\| !frame_bound(e))` | `N: G(false) contradicts the guard` left [`…160003/table/50100`] right [] | 1 passed | 93ccd999 |
| T2 break 2 | memo build: `.inspect(entries += 1)` moved before the `frame_bound` filter (counts the guarded entry, folds the same row) | `G's only entry is guarded, so its memo row folds no entry` left 1 right 0 | 1 passed | 93ccd999 |
| T2 break 3 | `memo.remove(&j)` after `fold_row` (rebuild every read) | `G's cone: one memo row, shared by N and M` left 2 right 1 | 1 passed | 93ccd999 |
| T3 (with precondition) break 1 | J(m): drop `&& !callee_sccs.contains(&yj)` | `A reads C's row once, not once per edge (the J(m) dedup)` left 2 right 1 (the new precondition passed first) | 1 passed | 93ccd999 |
| T3 precondition | `build_typed_edge_graph`: `list.dedup_by(\|a, b\| a.to == b.to)` after the sort | `precondition: the cone's graph keeps both A -> C edges` left 1 right 2 | 1 passed | 93ccd999 |
| T8 + T8' on CDO (Task 3, at a91ffae6) break (a) | singleton memo path: own-entry fold `for e in own { fold_fact }` → `for _e in own {}` | both red at the `failures` assert: 2 row diffs each, `CDO (cross-app): r0/0000102f…: writes_tables_of differs`, `CDO: r0/000025ae…: flags_of differs` | both `1 passed` (re-run after revert) | a91ffae6 (no commit) |
| T8 + T8' on CDO (Task 3) break (b), T2's class | memo build: drop `.filter(\|e\| !frame_bound(e))` | both RED (no LIMIT needed): `CDO (cross-app): r0/008c9f2c…: physical_table_reads_of differs`, `CDO: r0/008c9f2c…: physical_table_reads_of differs` | both `1 passed` (re-run after revert) | a91ffae6 (no commit) |
| T7' + T8' (Task 4) break 1 | singleton own-entry fold → `for _e in own {}` | T7' red: 18 row diffs, first `ws-d1-multi-caller: r0/1f7597…: writes_tables_of differs`; T8' red: `CDO (cross-app): r0/0000102f…: writes_tables_of differs`, `CDO: r0/000025ae…: flags_of differs` | T7' in `task-affected` (3596 passed); T8' `1 passed` | 9af9c15d |
| T7' + T8' (Task 4) break 2, T2's class | memo build: drop `.filter(\|e\| !frame_bound(e))` | T8' red: `CDO (cross-app)` and `CDO: r0/008c9f2c…: physical_table_reads_of differs`; T7' GREEN (no fixture has a guarded successor entry reaching a memo row: the fixtures' `with-guarded-successor` is 0), the class is carried by T2 and T8' | as above | 9af9c15d |
| T7' (Task 4) break 3 | memo path: `if !mode.wants_raw() { return }` → `if false && …` (DerivedOnly takes the scan) | T7' red: 480 × `DerivedOnly ran an old path`, first `ws-api-page: … (member_walks 0, singleton_scans 2)`, 0 row diffs; T5 `derived_only_builds_no_best` red too | pass (`task-affected`) | 9af9c15d |
| 7a T5 `scc_set_479_cycle_scales` (migrated) | recursive branch: `let picks = if mode.wants_raw()` → `if true \|\| …` (DerivedOnly walks) | `no per-member walk under DerivedOnly` left 479 right 0 | pass (`task-affected`) | 9af9c15d |
| `fold_fact_refuses_an_empty_resource_id` (new, final panel r1) | `Scratch::add`: delete the `resource_id != Some("")` `debug_assert!` | `note: test did not panic as expected` | 1 passed | 0c08452b |
| T7' via `raw_fold_hook` (final panel r1) | hook: `raw.iter()` → `raw.iter().take(0)` (the raw Vec is dropped unfolded) | 144 row diffs, first `ws-callsite-resolutions: r0/4cf8c46b…: flags_of differs` | pass (`task-affected`) | 0c08452b |
| T8' missing composition (final panel r1) | `check_cross_app`: `return None` for `where_ == "CDO"` | `CDO cross-app: no composition to check (no model built)` | T8' on CDO `1 passed` (cross-app `… singleton_scans 0, 84.2s`) | 0c08452b |

(A first break-(a) attempt did not apply — the Edit was refused, `grep -c` read 0 — and its two green runs are discarded; the break was re-applied and checked before the runs above.)

## Findings register summary

38 entries: spec panel S1-1..S1-23, S2-1 (1 critical, 14 important, 9 minor; all fixed); final panel F1-1..F1-14 (3
important, 11 minor; 11 fixed, F1-11 deferred minor (a discovery), F1-12 and F1-14 refuted). Every entry accepted by both
reviewers (sol, opus); no blocking entry deferred. Spec panel converged in 3 rounds, final panel in 2.

## Gate results

**Step 11 full set on H = `af823c6c`** (all `"supervised": true`, queued 0 s; logs under the run dir `logs/`):

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 388.6 | `ci-steps-all/20261021T224046-f8140ac8.log` |
| check-goldens-coverage | 0 | 0.0 | `check-goldens-coverage/20261021T224715-189be23e.log` |
| check-goldens | 0 | 0.1 | `check-goldens/20261021T224715-0fa5ca32.log` (tree stamp: this tree passed `ci-steps test` inside `ci-steps all`) |
| cdo-gate | 0 | 269.3 | `cdo-gate/20261021T224715-d060ee5b.log` |

`git status --porcelain -- . ':!.agent'` empty.


- **Task 3 recorded run (licenses Task 4).** HEAD `a91ffae6`; `agentflow run --name cdo-gate` (run by the conductor):
  exit 0, supervised, 597.6 s, `cdo-gate: PASS` (lib 2185 passed, T8/T8' included, `ENFORCE_CDO_WS=1`); log
  `/work/alsem/.agent/runs/20261021-185406-413c0c/logs/cdo-gate/20261021T205008-d45dfb87.log`. The oracle lines
  (captured stdout there) come from T8/T8' run alone at the same HEAD, `cdo` profile, `--nocapture`, one at a time:
  - T8 `scc_set_matches_member_walk_on_cdo`, pass:
    `scc-set oracle on CDO (cross-app): 31658 routines, 67 recursive SCCs, 714 members, max 479, … closed-memo-builds 25498, closed_memo_builds 25498 singleton_scans 0, 69.9s`;
    `scc-set oracle on CDO (single-app): 5479 routines, 1 recursive SCCs, 1 members, … closed_memo_builds 2751 singleton_scans 0, 5.2s`.
    The `PerMemberWalk` side's `closed_memo_builds == 0 && closed_memo_reads == 0` is asserted inside `check_model` (no
    failure). Max RSS **3,917,996 kB (3,826.2 MiB)**.
  - T8' `derived_only_matches_raw_fold_on_cdo`, pass:
    `raw-fold oracle on CDO (cross-app): … closed_memo_builds 25498 singleton_scans 0, 79.6s`;
    `raw-fold oracle on CDO (single-app): … closed_memo_builds 2751 singleton_scans 0, 4.4s`.
    Max RSS **3,938,636 kB (3,846.3 MiB)**. T8' − T8 = **+20.2 MiB** (rule: ≤ 256 MB).
  - Both breaks red (proof table). Afterwards `git status --porcelain -- src tests crates` empty, HEAD `a91ffae6`,
    both CDO oracles green again.
- **Task 4 `9af9c15d`** (the deletion; last code commit). `cargo test -p al-sem --lib` 2186 passed; `check-goldens`
  910 passed, no golden moved; release `ci-steps clippy` exit 0; `ci-steps task-affected` exit 0 (3596 passed, 8
  skipped). T8' alone on CDO (`cdo` profile): `raw-fold oracle on CDO (cross-app): 31658 routines, 67 recursive SCCs,
  714 members, … closed_memo_builds 25498 singleton_scans 0, 74.6s`; single-app `… closed_memo_builds 2751
  singleton_scans 0, 4.4s`; pass. Max RSS **3,928,228 kB (3,836.2 MiB)** = T8 (Task 3) + 10.0 MiB (rule ≤ T8 + 256 MB).
- `check-diff --base origin/master` after Task 4: reasons `protected-path:.github/workflows/grammar-drift.yml`,
  `protected-path:scripts/kraken-setup.sh`. Cause: origin/master moved to `fd77939d` (#197-#201) after the branch
  base `afd5d88f`; the branch touches neither path (`git diff --stat afd5d88f HEAD -- .github scripts` empty).
  `check-diff --base afd5d88f`: `{"reasons": []}`. Needs the rebase onto the new master (conductor), not a code fix.
- **Final panel r1 fixes.** Code `0c08452b` (timers, `raw_fold_hook`, classifier `debug_assert!`, census lines, T8'
  message, docs): `ci-steps task-affected` exit 0 (3597 passed, 8 skipped), release `ci-steps clippy` exit 0,
  `check-diff` `{"reasons": []}`; `cargo test -p al-sem --lib` 2187 passed. Docs `10bc2a14` (Decision 58 rounds,
  parent `fd77939d`, grammar `4d75846c`, `Both` fallback): `task-affected` exit 0 (3597 passed), `check-diff` `[]`.
- **RSS, warm binary (opus #2).** Every `cargo test` RSS above was taken after a separate
  `cargo test --profile cdo -p al-sem --lib --no-run`: the measured logs show no `Compiling` line and `Finished … in
  0.11-0.13s` (Task 3's T8 and T8', Task 4's T8'), so cargo compiled nothing and the max is the test binary's (cargo
  itself is small). Re-measured at `0c08452b` on the already-built binary run directly
  (`target/cdo/deps/al_sem-91c9905995cf54cf engine::l4::capability_cone::scc_set_tests::derived_only_matches_raw_fold_on_cdo --exact --nocapture --test-threads=1`,
  `CDO_WS` from config, `ENFORCE_CDO_WS=1`): pass, cross-app `… closed-memo-builds 25498, singleton_scans 0, 71.8s`,
  single-app `… 2751, singleton_scans 0, 4.5s`; max RSS **3,950,344 kB (3,857.8 MiB)** = Task 3's T8
  (3,826.2 MiB, warm) + 31.6 MiB (rule ≤ + 256 MB).
- **The CDO bench (spec §8), run once after the final panel.** P `fd77939d`, H `f3ccb89c`, both built release-fast
  from clean detached worktrees `/work/bench/160-7b-final/{parent,head}-wt` (`TREE_SITTER_AL_PATH` = the main
  checkout's grammar, `RAYON_NUM_THREADS` unset; `shas.txt`); the head's `cdo` test binary prebuilt (`--no-run`).
  - Command (one supervised run, started once the 1-min load was ≤ 4, at 3.17):
    `PYTHONUTF8=1 python3 scripts/agentflow --root /work/alsem --run-id 20261021-185406-413c0c run --name bench --timeout 45 --cwd /work/al-sem-issue-160-a3 -- bash /work/bench/160-7b-final/bench.sh`.
    Result: exit 0, `"supervised": true`, 1,601.1 s, log
    `/work/alsem/.agent/runs/20261021-185406-413c0c/logs/bench/20261021T221142-1ee49d0a.log`
    (`OK part1`, `OK part2`, `OK part3`; 7a's `check.sh` verbatim).
  - Load (1/5/15 min, `/work/bench/160-7b-final/load.txt`, plus a 60 s tick):
    - Part 1: 3.17/8.76/13.77 → middle 7.07 → end 6.14;
    - Part 2: 6.14 → 5.95 → 5.57;
    - Part 3: 5.57 → end 9.48;
    - ticks 3.17-11.37.
  - **Part 1.**
    - SARIF `cmp` silent at `--scope primary` (9,698,287 B) and `--scope all` (73,864,653 B).
    - Histogram SHA-256 equal on both sides: `c032c8db…94ef`.
    - Retained `context.cone_derived` 65,273,122 = 65,273,122; `context.summaries` 36,400,882 = 36,400,882.
    - T8' (`derived_only_matches_raw_fold_on_cdo`) passed: cross-app `… closed-memo-builds 25498, singleton_scans 0,
      75.5s`; single-app `… 2751, singleton_scans 0, 4.4s`.
  - **Part 2** (5 interleaved rounds per side, samples in round order).
    - Q = `singleton + bfs_fold`, parent: 5,473.8 / 6,095.5 / 5,355.2 / 5,354.9 / 5,522.2. Pq 5,473.8, Bq 740.6.
    - Q, head: 982.2 / 1,209.5 / 1,056.7 / 902.7 / 1,289.8. Hq 1,056.7, band 387.1.
    - Span `context.capability_cones`, parent: 11,850.0 / 11,601.5 / 11,864.1 / 11,152.5 / 11,281.8. Pc 11,601.5,
      Bc 711.5.
    - Span, head: 6,384.2 / 6,365.4 / 6,214.7 / 6,289.3 / 6,442.6. Hc 6,365.4, band 227.9.
    - Void checks: B1 4,382.1 ≥ 1,481.2, holds. B2 3,641.5 ≥ 1,423.0, holds. No re-run.
    - **B1 met:** 4,417.1 ≥ 3,641.5. **B2 met:** 5,236.1 ≥ 2,930.0. Literal form 5,236.1 ≥ 3,670.6, met.
    - Head sub-timer medians (ms): subst 132.5, memo 683.0, row 5.8, open 19.7, own 139.6, `bfs_fold` 39.7.
    - `closed:` line as expected in all 5 head rounds (25498 / 42912 / 3879321 / 67 / 0 / 1568).
    - stdout SHA `245055170098d892` on both sides.
  - **Part 3** (3 rounds per side, Decision 58).
    - `as` (`analyze --single-app`): 5.7 (0.8) vs 5.7 (1.9) s; RSS 700.6 / 707.6 MB; stdout `c6c4d832599851f3`.
    - `px` (cross-app `policy check`): 107.1 (6.3) vs 93.8 (6.4) s, −13.3 s; RSS 13,877.9 / 13,847.9 MB; stdout
      `a1b8f1ad82042442`.
    - The `Both` fallback does not fire.
  - **Verdict: every bound met, none void.** Numbers committed as `9ac7c4a6` (`CHANGELOG.md`,
    `docs/measurements/2026-10-21-issue-160-step7b.md` only; pre-commit hook passed).

- Task 1 (`1101e9d9`): `scripts/ci-steps task-affected` exit 0 (3598 passed, 8 skipped; a first run failed dev
  clippy `map_entry` on `contains_key` + `insert`, fixed with the `Entry` API before the commit);
  `cargo test -p al-sem --lib` 2188 passed; `scripts/check-goldens` 910 passed, no golden moved; pre-commit hook passed.
- Task 1 CDO, `cdo` profile, run alone, `ENFORCE_CDO_WS=1` (before the `Entry` refactor, same behaviour):
  - T8 widened: `scc-set oracle on CDO (cross-app): 31658 routines, 67 recursive SCCs, 714 members, … closed_memo_builds
    25498 singleton_scans 0`; `(single-app): 5479 routines, … closed_memo_builds 2751 singleton_scans 0`; pass.
    Max RSS 3,942,696 kB (3,850.3 MiB) (`getrusage(RUSAGE_CHILDREN).ru_maxrss` of `cargo test`; no `/usr/bin/time`).
  - T8': `raw-fold oracle on CDO (cross-app): … closed_memo_builds 25498 singleton_scans 0`; single-app 2751 / 0; pass.
    Max RSS 3,963,696 kB (3,870.8 MiB) = T8 + 20.5 MiB (rule: ≤ T8 + 256 MB).
- Task 1 `check-diff` (after the commit): **reason** `forbidden:allow-attribute:src/engine/l4/capability_cone.rs:
  #[allow(clippy::too_many_arguments)]` — the step-6 stub's attribute on `compose_cone_over_graph_with` (its 8th
  parameter `raw_sink`). Stopped and reported. Fixed in `e50026ce` (`ComposeOptions`): `check-diff` `{"reasons": []}`,
  `ci-steps task-affected` exit 0, release `ci-steps clippy` exit 0.

## Discoveries

```json
[
  {"subsystem": "engine / perf_trace (memory peaks on Linux)", "locator": "src/engine/perf_trace.rs:175 read_rss (the non-Windows arm returns None)", "symptom": "On Linux (server1, the kraken tentacle) read_rss is a stub, so the trace's peak.process_mb and peak.post_attach_mb read 0; every #160 memory bound measured there falls back to psutil's 20 ms RSS sampling, which is not a true high-water mark, and the post-attach peak cannot be read at all.", "kind": "enhancement", "origin_issue": 160, "reproducer": "On server1, `alsem analyze <ws> --trace <file>` (any workspace): the trace's peak.process_mb / peak.post_attach_mb events are 0; on Windows the same command reports K32GetProcessMemoryInfo's peak working set.", "pre_existing": true, "capability": "read_rss on Linux from /proc/self/status (VmRSS, VmHWM), with the post-attach peak taken by resetting VmHWM via /proc/self/clear_refs (\"5\") at attach", "acceptance": "on Linux the trace's peak.process_mb equals VmHWM at exit and peak.post_attach_mb is the high-water mark after the attach point; a unit test covers the parsing"},
  {"subsystem": "engine / l4 capability cone (step 7b memo path)", "locator": "src/engine/l4/capability_cone.rs inherited_facts_for_singleton (memo build, subst) and the recursive set_row fold in compose_inherited_cones", "symptom": "The 7b memo path still does avoidable work: a memo row is built and kept for a callee cone with a single reader (no reuse possible); a non-root recursive SCC's cone is folded once for set_row and again for each singleton's memo row; under DerivedOnly the subst Vec (one Vec per out-edge, Arc clones per guarded entry) is built only to fold e.rep.", "kind": "enhancement", "origin_issue": 160, "reproducer": "ALSEM_CONES_CENSUS=1 alsem analyze <cdo_ws> on 7b's head: closed: memo_builds 25498 (the single-reader share is unmeasured); singleton: subst 132.5 ms, memo 683.0 ms (bench medians, docs/measurements/2026-10-21-issue-160-step7b.md).", "pre_existing": false, "capability": "fold a single-reader callee cone straight into the routine row; share one fold between set_row and the memo for a guard-free SCC cone; fold substitute_entry's rep directly under DerivedOnly", "acceptance": "each change keeps every golden and the T7'/T8' oracle green and shows a saving beyond the noise band on the singleton timer in a CDO A/B bench"}
]
```

## Not delivered

- A7b.1 as named (`FactKeyInterner`, hash-consed `SetArena`): replaced by amendment 7B-1 (a memo keyed by the callee
  Cone-SCC), accepted by both panels; measured net gain of hash-consing ≈ 131 ms (P17).
- #160 itself stays open: this PR is epic step 7b only ("Part of #160").
