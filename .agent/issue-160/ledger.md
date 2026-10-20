# Issue #160 ledger: attempts 1-2 on server1 (epic step 7a: one inherited set per SCC)

**Attempt 2** (pre-ratified, #45). Claim:

```json
{"run_id":"20261020-113707-ad56ee","issue":160,"attempt":2,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/160-alsem-batch-build-the-model-onc-a2","worktree":"/work/al-sem-issue-160-a2","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","handoff":{"from_attempt":1,"from_run":"20261020-072815-106c18","sha":"e828eb182b3c7ce54974080214a4e1cac2fb215c","reviewers":{"astra":"sol","flash":"opus"},"register_blob":"e5e5e51d7bba81a556c1cdcb46effed81c978c02"}}
```

Base `68253a8e15b7cb1856f9f6536dabbe6b02702b23` (make-worktree, origin/master). Handoff reviewers
(`handoff.reviewers`): astra = sol, flash = opus; the operator's instruction still holds for this loop, so
attest passes `--substitute astra=sol --substitute flash=opus`. Attempt 1's record follows unchanged; attempt
2's own timeline lines are marked **[a2]**.

Earlier attempts' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170), step 1b at `6c6bce17` (PR #172), step 2 at `d4d8b71f` (PR #174), step 3 at `c554b704` (PR #178), step 4 at `c186f4d1` (PR #180). This file replaces them for this attempt. The attempt counter restarted at 1 because server1's `.agent/` is fresh (handoff note §1); this is the eighth #160 claim overall.

Part of #160. This PR delivers epic step 7a only (spec
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.6 and §9, row **7a**).
#160 stays open until its last step (owner decision, 2026-10-18): the PR says "Part of
#160" and carries no closing keyword.

## Claim

```json
{"run_id":"20261020-072815-106c18","issue":160,"attempt":1,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/160-alsem-batch-build-the-model-onc-a1","worktree":"/work/al-sem-issue-160-a1","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883","title":"alsem batch: build the model once and run several commands against it","reconciled":[]}
```

Base `d37e8634d72699b0d66e8b7636c16aecb3d45611` (make-worktree, origin/master). Build
output: the worktree's own `target/` (no `target_root` on server1).
Grammar: `TREE_SITTER_AL_PATH=/work/src/tree-sitter-al`. CDO workspace: `cdo_ws` in the main checkout's
`.agent/config.json` (its path is kept out of git, as `agentflow sanitize` requires).

Reviewers: the operator's stand-ins for this loop (2026-10-20): gpt-6.1-sol via pi (slot
astra) and Opus 5.5 via the Agent tool (slot flash). Register marks are keyed `sol` / `opus`;
attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Architectural.** Step 7a replaces the cone's per-routine inherited-membership walk
(`DerivedOnly`) with one set per Cone-SCC, keeps the old path test-only until CDO passes,
and carries a CDO wall bound (`bfs.walk` ≤ 1 s and a cone saving bound read against step 0's
medians and bands) and a retained-bytes bound (≤ 98 MB). It must also decide the step-4
amendment M3: whether the Cone adjacency and its SCC join `ExecRelation` at 7a or at step 6.
A child spec is written.

## Assumption probes

All read-only, by the spec author against `d37e8634` in `/work/al-sem-issue-160-a1` (2026-10-20). No build
or CDO run: every probe is a source read, a grep, or a committed numbers file. "Falsified" rows reshaped
the child spec as stated.

| probe | result | evidence |
|---|---|---|
| P1 Members of one recursive Cone-SCC fold the same inherited set | **held, with two per-member parts**: members differ only in their own producer-solve entries and the self half (their raw direct facts). Guards do not differ (a recursive SCC has no guard anchor; the walk drops guards). The only per-member filter, `routine_ids.contains(m)`, gates emission, not membership | capability_cone.rs:2484, 2069-2083 (own); 2641-2650 (self half); 2528-2538 (anchor only for a non-recursive singleton); 2054-2056; 2576 |
| P2 The per-member walk skips the subject's own direct facts, but the SCC's fact cone includes them | held, harmless: `visited` starts with the subject, and the self half folds its raw direct facts anyway, so F(direct(m)) ⊆ F(self half) | capability_cone.rs:1991-1992, 1557-1569, 2646-2650 |
| P3 The fold depends only on the key, and it is a union | held: `fold_fact` reads kind, op, resource id and known-temp, all four in `inherited_fact_key` (issue 33); freeze sorts and dedups | cone_derived.rs:51-66, 725-772, 831-860 |
| P4 A recursive SCC's fact cone has exactly the walk's keys plus m's own direct keys | held: with no anchor, members' direct facts sit at dist 0 with no guard and successor entries carry the base key with `guard: None`; the walk reaches every sibling and every successor SCC | capability_cone.rs:2140-2291 (2164-2207, 2270-2287), 2003-2066, 2117-2136 |
| P5 A root recursive SCC's cone exists | **falsified**: root cones are never built (the root skip). Reshaped: 7a builds the set for a root SCC locally and drops it after its members | capability_cone.rs:2525-2550, 2703-2708 |
| P6 Epic open question 2: a non-producer fact's temp class is constant along a path | held: producers never enter a cone; `substitute_entry` copies the rep unchanged | capability_cone.rs:1557-1569, 1586-1612; pd_solve.rs:1-19 |
| P7 `bfs.walk` on CDO, its band, and I6 | held: analyze median 8,627.5 ms, band 223.6 ms (38.6× the band). The band is head-only (the parent had no such timer at step 0); 7a re-reads both from its own parent runs | step-0 numbers §4.1.1 |
| P8 `bfs.fold` | 1,029.3 ms, band 31.8 ms; the walk is 89 % of the recursive branch | step-0 numbers §4.1.1 |
| P9 The cone span's band | parent 22,193.1 / 1,126.3 ms; head 20,780.6 / 2,041.8 ms: 5–9× `bfs.walk`'s band. Reshaped: amendment 7A-1 | step-0 numbers, analyze table |
| P10 Largest Cone-SCC is 479 | held: a Base App hub that #96's implicit-trigger edges created; Σ recursive members 714 (`BFS_CALLS`), picks 4,344,888 | step-0 §4.1.1 counts; .agent/issue-96/ledger.md:96 |
| P11 "Step 0's SCC distribution" is the Cone graph's | **falsified**: the recorded `scc_stats` is the PRIMARY SCC from `build_l4` (max 1); of the Cone graph only max 479 and Σ 714 are known. Reshaped: 7A-3, Task 1 adds a Cone-SCC census that the stop rule reads | analysis.rs:184-224; step-0 numbers "SCC distribution"; step-2 numbers §scc_stats |
| P12 Step 0's "disagreement" (max 1 vs 479; nodes 31,658 vs sccs 31,656) | explained: two numbers from two different graphs; the gap of 2 is two colliding routine ids (G-18) that `nodes` repeats | exec_relation.rs:83-88 vs capability_cone.rs:2844-2849; detector_context.rs:1190; CHANGELOG #160 step 3 |
| P13 7a can reuse `rel.scc()` | **falsified**: `rel.scc` is the Primary SCC, so the Cone SCC stays inside the cone. Feeds the M3 decision | exec_relation.rs:83-88, 110-122 |
| P14 "retained ≤ 98 MB" is a bound today's code meets | **falsified**: 98 = `context.cone_derived` 65,308,148 B + `context.summaries` 37,702,889 B = 98.24 MiB, over 98; the source scratchpad is not in the repo. Reshaped: 7A-2, head ≤ parent (equality expected) | step-0 census table; epic §3 lines 87-88 |
| P15 Who reads derived rows | only the det cone makes a production store (DerivedOnly/Both). Readers: capability_query, transaction_spans, pending_writes, d1/d2/d34/d35/d43/d44/d45/d48/d50, effect_query, digest. Snapshot/r3a3/r3a5 use RawOnly | detector_context.rs:1198-1224; capability_cone.rs:3846, 4106, 4285; grep |
| P16 Where the old path's outputs differ in order | only the interning order of resource ids; no output reads it (accessors sort by string, spans sort, pending_writes uses a BTreeSet, the store has no `PartialEq`) | cone_derived.rs:499-646; transaction_spans.rs:251-262; pending_writes.rs:57, 191-196 |
| P17 The BFS can be deleted at 7a | **falsified**: RawOnly (r3a3, r3a5, snapshot) and Both (policy RAW) need per-member witnesses, so `inherited_facts_by_bfs` stays production code; only its fold use becomes test-only | capability_cone.rs:3846, 4106, 4285; detector_context.rs:1202; pipeline.rs:258 |
| P18 `fold_row` exists | no: the builder has begin / fold_fact / finish_routine / cfg(test) fold_routine only. It is new 7b code and stays out of 7a | cone_derived.rs:701-827 |
| P19 Build counters move | no: 7a adds no graph, Tarjan or cone; `cli_build_counters` unchanged | capability_cone.rs:2833-2849 |
| P20 A test pins the census text (`bfs_walk`, `picks=`) | none | grep over tests/ and src/ |
| P21 bench_ab can carry `ALSEM_CONES_CENSUS` | **falsified**: it strips every `ALSEM_*` variable; the spec uses a per-side `exec` wrapper outside the repo | scripts/bench_ab.py:39 |
| P22 bench_ab runs on server1 as-is | **falsified (infra gap)**: `psutil` is missing and there is no pip or uv. Not worked around (operator, 2026-10-20); parts 2-3 of the bench need it in the image | bench_ab.py:37; `python3 -c "import psutil"` → ModuleNotFoundError; `python3 -m pip` → No module named pip |
| P23 Trace peaks and `rss_mb` on server1 | absent: `read_rss` is a `None` stub off Windows. 7a has no peak bound; peaks are reported as polled RSS only | src/engine/perf_trace.rs:174-177 |
| P24 cdo-measure on server1 / CDO pin | cdo-measure was absent at probe time (owner-PC skill; since installed, 2026-10-20); the CDO baseline is at `bc3ccb18` with an empty porcelain status | find /; git -C /work/src/cdo-baseline |
| P25 Gap scenarios reachable by a lib oracle | no (as at step 3); the fixture dirs and inline graphs stand in | CHANGELOG #160 step 3 "Gap deviation" |

## Base/Head SHAs

- Attempt 2: make-worktree base `68253a8e15b7cb1856f9f6536dabbe6b02702b23`; step 11 B = `68253a8e15b7cb1856f9f6536dabbe6b02702b23`
  (origin/master, unmoved), H = `91a66203a99e4d8901fc8b64477cbf4f564bb5c6`.

## Timeline

- make-worktree: `{"worktree": "/work/al-sem-issue-160-a1", "branch": "issue/160-alsem-batch-build-the-model-onc-a1", "base": "d37e8634d72699b0d66e8b7636c16aecb3d45611", "target": null}`
- classify: architectural
- Skills missing on server1: `panel-review`, `discrimination-proof`, `golden-diff-triager`,
  `superpowers:writing-plans` and `superpowers:subagent-driven-development` live only under the
  owner PC's `.claude/skills/` (server1's `.claude/` has `commands/` only). Substitution, named so
  it cannot read as the specified step: panels follow the rules `/issue` step 5 and step 10 state
  inline (briefing file with absolute paths and a confirm/reject checklist; round 1 asks for every
  finding; later rounds verify); proofs follow CLAUDE.md "Testing Philosophy" (break with Edit, run,
  revert with Edit, record both outcomes); the plan follows the format of
  `docs/superpowers/plans/2026-10-20-issue-160-step4-exec-relation.md`.
- spec author (opus subagent) dispatched for probes + child spec; `charge subagents` → remaining 59
- spec author returned: 25 probes (above) and the child spec
  `docs/superpowers/specs/2026-10-20-issue-160-step7a-scc-membership-design.md` (647 lines, uncommitted).
- **Infra gap (operator, 2026-10-20): `psutil` and pip are missing on server1**, so `scripts/bench_ab.py`
  (parts 2-3 of the CDO bench) cannot run here. Not worked around, by the operator's instruction; the
  image must provide it before the bench. **Resolved by the image since** (conductor re-check, 10:5x UTC):
  `python3 -c "import psutil"` → 7.1.0 for both `python3` and `python` (spec panel r1, opus F6 → S1-19).
- **Infra fault, 07:5x–10:40 UTC**: the project hooks were installed in `.claude/settings.local.json`
  with a `cargo fmt` guard that used bash's `<<<` under `/bin/sh` (dash) and blocked every Bash call.
  The session was interrupted and the heartbeat loop died, so the lease went stale (no `recover` ran).
  The operator fixed the hook at the source (kraken); `beat` under the same run id revived the lease
  (`stale: false`), so no `recover`/`unblock` was needed. #160 stayed `agent-working`.
- Skills now present on server1 (operator, 2026-10-20): `panel-review`, `discrimination-proof`,
  `cdo-measure`, `golden-diff-triager`, superpowers. The substitution note above applies only to the
  phases run before this point (classification and probes).
- spec panel round 1 (`panel-review` skill): `charge spec_rounds` → remaining 4; `charge pi_calls` → 19;
  `charge subagents` → 58. Briefing `.agent/runs/20261020-072815-106c18/spec-panel-r1-briefing.md`;
  reviewers gpt-6.1-sol via pi (slot astra) → `pi-sol-spec-r1.md`, Opus via the Agent tool (slot flash)
  → `opus-spec-r1.md`.
  Outcome: both CONFIRM the equality proof (C1), root sets (C2), M3 deferred (C3), fold_row out (C4), the
  test-only oracle (C7) and byte-identical goldens (C9); both REJECT the test plan (C8) and parts of the
  bounds and stop rule (C5, C6). Merged into 23 register entries S1-1..S1-23 (11 important, 12 minor),
  hash `9602ff7887951373` (the spec as reviewed). Sent to the spec author for fixes.
- round-1 fixes: the spec author verified all 23 against the source; 22 fixed, S1-18 fixed with one part
  refuted (detector_context.rs:1202 was already right). Conductor re-read three cited lines
  (capability_cone.rs:2272 `anchor.is_some() &&`; detector_context.rs:1202 `ConeOutput::Both`;
  bench_ab.py:98-114 returns 0 after `NON-ZERO EXIT`); all held. Spec sha256 prefix `8918ea6cbf20f65b`
  (frozen copy `.agent/runs/20261020-072815-106c18/spec-r2-8918ea6c.md`). Every new test and break is a hand
  trace that has not run yet; each test asserts its precondition first.
- spec panel round 2: `charge spec_rounds` → remaining 3; `charge pi_calls` → 18; `charge subagents` → 56.
  Briefing `spec-panel-r2-briefing.md`; sol continues its round-1 thread → `pi-sol-spec-r2.md`; the round-1
  Opus reviewer resumed with its context → `opus-spec-r2.md`.
  Outcome: 21 of 23 accepted by both. S1-1 re-raised by sol (the fixture's table T is not declared by the
  test `model()`, which has only `Cust`; conductor verified at capability_cone.rs:5765-5774), accepted by
  opus. S1-9 re-raised by both. New: S2-1 important (the bench checks still exit 0 on a failed sample;
  opus N1, sol under S1-9), S2-2 minor (T3b's set-size assertion is 0 == 0; opus N2). Sent to the author;
  `charge subagents` → 55.
- round-2 fixes: the author fixed S1-1 (fixtures use declared tables; new test helper `model_with_tables` for
  T3b), S1-9/S2-1 (every bench part `set -euo pipefail`, one sourced `check()` over `runs.jsonl`'s grid),
  S2-2 (each Mi commits, so the set has one entry). The conductor asked for Part 1's `check.sh` placeholder to
  be filled before the round (it was); fields verified at bench_ab.py:64-66, :73, :82. Spec sha256 prefix
  `4d724f656299b132` (frozen `spec-r3-4d724f65.md`; diff `spec-r2-to-r3.diff`, 174 lines). All 25 entries
  `fixed`, every mark reset to `unreviewed` against the new hash.
- spec panel round 3: `charge spec_rounds` → remaining 2; `charge pi_calls` → 17; `charge subagents` → 54.
  Briefing `spec-panel-r3-briefing.md` → `pi-sol-spec-r3.md`, `opus-spec-r3.md`.
  Outcome: both accepted all 25 entries. Sol: no new finding. Opus: one new minor, S3-1 (§6's stop-rule
  reading reuses §8's 10-trace / `c-head-*` checks, so a good 6-trace reading fails); one register cite
  corrected (`MAY_COMMIT` is at cone_derived.rs:109). Not converged. Sent to the author with a sweep of every
  bench script for side/round/trace/file-pattern mismatches and a `bash -n` of each; `charge subagents` → 53.
- round-3 fix: S3-1 fixed (§6 `stoprule.sh` self-contained: t1/t1b, 3 rounds, 6 traces, 6 census files) and
  seven more script mismatches fixed in the sweep (missing `cd`, loose globs, uncounted census files, silent
  exits under `pipefail` on empty globs and `grep -c`, a silent SARIF `cmp`, a non-idempotent parent
  worktree). Conductor re-ran `bash -n` on all six extracted blocks
  (`.agent/runs/20261020-072815-106c18/spec7a-scripts/`): all ok. Spec sha256 prefix `eaa3be681bc85e2a`
  (frozen `spec-r4-eaa3be68.md`; diff `spec-r3-to-r4.diff`, 124 lines, §6/§8 scripts only).
- spec panel round 4: `charge spec_rounds` → remaining 1; `charge pi_calls` → 16; `charge subagents` → 52.
  Briefing `spec-panel-r4-briefing.md` → `pi-sol-spec-r4.md`, `opus-spec-r4.md`.
  Outcome: both accepted all 26 entries. New, both from the sweep: S4-1 important (sol: a reused parent-wt is
  checked for its commit but not for a clean tree) and S4-2 minor (opus N4: stoprule.sh builds the worktree
  without checking HEAD is the Task-1 sha or the tree is clean). Not converged. Sent to the author with a sweep
  of every build site for the same check; `charge subagents` → 51. Round 5 is the last spec round.
- round-4 fixes: S4-1 (parent-wt reused only at d37e8634 AND with an empty porcelain status, else FAIL and a
  human removes it; never reset) and S4-2 (stoprule.sh takes the Task-1 sha, checks HEAD and a clean
  src/crates/Cargo.toml/Cargo.lock, records the sha); the sweep added the same check before Part 1's two
  head-tree builds (release-fast and the CDO oracle `cargo test --profile cdo`). Conductor re-ran `bash -n`:
  all ok. Spec sha256 prefix `7630443e2a3cbe23` (frozen `spec-r5-7630443e.md`; diff `spec-r4-to-r5.diff`, 47
  lines, scripts only). 28 entries, all `fixed`, marks reset.
- spec panel round 5 (final): `charge spec_rounds` → remaining 0; `charge pi_calls` → 15; `charge subagents`
  → 50. Briefing `spec-panel-r5-briefing.md` → `pi-sol-spec-r5.md`, `opus-spec-r5.md`.
  Outcome: opus accepted all 28, no new finding. Sol accepted 26 and re-raised S4-1 and S4-2 for one shared
  reason (its N1, important): the new clean-tree checks `test -z "$(git … status …)"` pass when `git status`
  itself fails with empty output. Conductor verified it at spec :628, :812, :818. **Spec panel not converged
  at the cap (5 of 5 rounds).**
- Handoff check (#45 rule, `cli.py:775-781`, judged against the stand-in roster sol/opus): no `open` entry;
  no blocking entry `deferred`; every mark is `accepted` or `re-raised` (none `unreviewed`); every entry is
  accepted by at least one reviewer (26 by both, S4-1 and S4-2 by opus). Handoff-ready. N1 is recorded under
  S4-1/S4-2, the entries sol re-raised for it, not as a new entry: a new entry would carry no marks.
- After the cap the conductor applied sol's own N1 fix at the three sites (`dirty=$(git … status …) || {
  FAIL; exit 1; }; test -z "$dirty" || …`; spec :628-629, :813-814, :820-821; no `test -z "$(` remains). The
  handed-off spec is therefore NOT the one the panel last read (`7630443e2a3cbe23`): the next attempt's
  final panel round 1 must re-mark S4-1 and S4-2 against it, and `attest` asks both reviewers to accept
  every carried entry.
- `agentflow sanitize` refused the CDO workspace path (spec :623, :725, :750, :782, :865, :917; ledger :18).
  The bench scripts now read it as `WS=$(jq -er .cdo_ws /work/alsem/.agent/config.json)`, the prose names
  `cdo_ws`, and `scripts/cdo-gate "$WS"`. No logic change. Sanitize then clean. Handed-off spec sha256 prefix
  `7236fa5125a950c2`.

- **[a2]** make-worktree: `{"worktree": "/work/al-sem-issue-160-a2", "branch": "issue/160-alsem-batch-build-the-model-onc-a2", "base": "68253a8e15b7cb1856f9f6536dabbe6b02702b23", "target": null}`
- **[a2]** Pre-ratified from attempt 1 at `e828eb182b3c7ce54974080214a4e1cac2fb215c`: restored the spec, ledger
  and register (`git restore --source`); committed the spec as `af76001e` ("docs: pre-ratified design for #160
  from attempt 1"). Steps 2-5 skipped; the carried spec is the spec and the carried register is the register.
  Carried S4-1 and S4-2 are `re-raised` by sol: the final panel's round 1 must re-mark them, with every other
  carried entry, against the current hash.
- **[a2]** step 6 acceptance tests: opus subagent dispatched; `charge subagents` → 59. Result (uncommitted, in
  a new `scc_set_tests` module at the end of `src/engine/l4/capability_cone.rs`; filter
  `cargo test -p al-sem --lib scc_set_tests`, log `.agent/runs/20261020-113707-ad56ee/t-scc-final.log`):
  - FAIL on today's code, each on a stats assertion naming the missing behaviour: T1 (`recursive_sccs` 0 ≠ 1),
    T2 (`recursive_roots` 0 ≠ 1), T3b (`own_entries_recursive` 0 ≠ 64; the 8-states precondition holds), T11
    (`recursive_members_all` 0 ≠ 2), T5 (`member_walks` 479 ≠ 0; rows and oracle equality hold), T6 (chain sets
    0 ≠ 20), T9 (`member_walks` 479 ≠ 0 through production `compose_cone_over_graph`; its raw half passes), T10
    (`recursive_sccs` 0 ≠ 2).
  - PASS, by design: the §5.1 comparator (test code; all 7 pairs fail on their own field); T3 and T4 (no stats
    assertion; today's path already produces their rows; their breaks act only on the new fold, so their
    proofs run after the switch); T7 (the oracle; 249 dirs × 2 modes, 2.7 s). T8 skipped (no `CDO_WS`; gated
    with `crate::cdo::cdo_ws_or_enforce()`; asserts `recursive_members_emitted == 714`).
  - Hand traces checked on real code: with every stats assertion temporarily flipped (and reverted with Edit),
    every literal row and non-stats precondition in T1, T2, T3b, T5, T6 and T11 held. No spec trace is wrong.
  - Stubs: `RecursiveMembership { PerScc, #[cfg(test)] PerMemberWalk }` (:2402); `ComposeStats` (:2416, 12
    fields, only `member_walks` counted, `pub` for now because nothing reads it yet and `pub(crate)` fails
    dead-code clippy); `compose_inherited_cones` gains an unused `_membership` parameter (`shortcut:` comment)
    and returns the stats; `compose_cone_over_graph_with` (:2886); `#[cfg(test)] row_ids()` (cone_derived.rs:452);
    a shared `#[cfg(test)] test_facts` module (cone_derived.rs:868); `DetConeInputs` + `det_cone_inputs`
    (detector_context.rs:1111/1119) extracted from `build_detector_artifacts` (`DIRECT_NANOS` now also times
    the small publisher-map build); `model_with_tables` (capability_cone.rs:5843).
  - Whole lib: 2172 passed, 8 failed (exactly the red tests), 4 ignored (`lib-all.log`). Dev clippy `-D warnings`
    exit 0 (`clippy.log`).
  - **Coverage limit (for the CHANGELOG):** T7's shape classes over all 249 fixture dirs, both modes: multi-member
    4, roots 4, every other class 0 (self-loops, non-roots, with-successor, guarded successors, members with own
    entries). Their breaks reach only the hand tests (T1, T3, T4) and T8 on CDO. T7 asserts the two non-zero
    classes as minimums with a LIMIT comment. Its class count uses a test-only copy of the cone loop
    (`sccs_with_frame_bound_cone`) that must follow production.
  - Spec deviation: the §5.1 comparator checks `writes_physical_tables_of` before `physical_table_write_ops_of`
    (needed so the physical-writes pair differs in one field only).
- **[a2]** step 7 plan: the spec author (attempt 1's opus subagent, context intact) wrote
  `docs/superpowers/plans/2026-10-20-issue-160-step7a-scc-membership.md` (`superpowers:writing-plans`);
  `charge subagents` → 58. Tasks: 1 instrument, no behaviour change (T1, T10, T11 green); CHECKPOINT (conductor
  runs §6 `stoprule.sh <Task-1 sha>` under `agentflow run --name bench`; S1-S3 can stop the step); 2 the switch
  (T2, T3b, T5, T6, T9 green, plus a new `scc_set_root_with_no_emitted_member_builds_nothing`; T3/T4 proofs after
  it); 3 old fold test-only (small); 4 T8 on CDO (small); 5 CHANGELOG and numbers-file skeleton (small).
  The plan first parked the five switch-only red tests with `#[ignore]`; refused by the conductor, since
  `scripts/agentflow/protect.py:16` flags an added `#[ignore` (`ignore-attribute`) and `check-diff` runs before
  every task commit. Revised: Task 1 moves them byte for byte to
  `.agent/runs/20261020-113707-ad56ee/parked-switch-tests.rs` (outside the repo) and Task 2 restores them.
  Bench parent stays `d37e8634`: `git diff --stat d37e8634 68253a8e -- src crates tests Cargo.toml Cargo.lock` is
  empty (conductor verified), stated in the numbers file.
  `charge plan_tasks` × 5 → remaining 7.
- **[a2]** step 8 implement: `halt-check` → not halted. Task 1 implementer (opus) dispatched; `charge subagents` →
  57.
- **[a2]** Task 1 committed `68a7d6222cd2a48f747eeafd0d6303999da92d4f` (capability_cone.rs, cone_derived.rs,
  detector_context.rs, the plan). Gate `scripts/ci-steps task-affected` → exit 0, 3577 run / 3577 passed / 8 skipped
  (`.agent/runs/20261020-113707-ad56ee/t1-gate2.log`; the first run `t1-gate.log` failed dev clippy on
  `field 'stats' is never read`, fixed by making `ConeOutcome.stats` `#[cfg(test)]`). `cli_build_counters::` 43/43
  (`census_rest_is_never_negative` included). TDD red `t1-red.log` (T10 0≠2, T1 0≠1, T11 0≠2), green `t1-green.log`
  (8 passed). `charge task_attempts --sub task1` → 2 left. Parked (outside the repo, held by the conductor):
  `parked-switch-tests.rs` sha256 `2d132eaf99f9cabf9a1c1108989f693cfb3b867cc98889db7a64b18c735160ca` (5 `#[test]`;
  helpers `t5_graph`, `t3b_src`, `raw_row`, then T2, T3b, T5, T6, T9) and `parked-switch-outside-module.rs` sha256
  `2516fce61821a81dbf888d375e83032f4667c19a40a04750c6f82e39bf4a4b6d` (`model_with_tables`). Task 2 must also re-add
  `model_with_tables` and `MAY_COMMIT` to two `use` lines. Deviations: `ConeOutcome.stats` is test-only and
  production reads the counts via `cones_census::add_compose_stats` inside `compose_inherited_cones` (so
  `project_r3a3`'s call counts too); extra `CONE_SCC_MEMBER_WALKS` atomic; doc comments and empty section headers
  moved with the parked tests. `check-diff --base master` reported `protected-path:scripts/kraken-setup.sh`: local
  `master` (`1e18860e`) is 2 commits behind `origin/master` (`cacf559e`, `68253a8e`, both kraken), so
  `master..HEAD` includes origin's own edits to that file. Against the branch's merge-base
  `check-diff --base origin/master` → `{"reasons": []}`; the branch's diff is its 5 files. Sonnet reviewer dispatched;
  `charge subagents` → 56.
- **[a2]** Task 1 review (sonnet): 1 important: the production use of the stats (`add_compose_stats`,
  capability_cone.rs:2851, and the `cone_scc:` line) is pinned by no test (conductor verified: `grep -rn cone_scc
  tests/` empty); the stop rule's S1 reads that line. Minor: T10 asserts 5 of the fields; old-vs-new comparisons are
  vacuous until the switch (expected; Task 2's proofs carry them; the stale `shortcut:` marker is Task 2's to delete);
  `direct=` now also times `det_cone_inputs`' map build. Clean: no behaviour change with the census off, the overlap
  rule, counter definitions (note: `recursive_roots` counts roots with no emitted member, the probe does not; S1/S2
  must not compare them), the three proofs, SOLID/DRY, protected paths. Findings 1, 2, 4 sent back to the same
  implementer (one new commit, plus the conductor's spec path fix a1 → a2 for the bench scripts);
  `charge task_attempts --sub task1` → 1 left.
- **[a2]** Task 1 review fix committed `99e0f6b5271734a9c9099ab6ff9e105f03263a31` (capability_cone.rs test asserts only,
  production byte-identical to 68a7d622; detector_context.rs; tests/cli/cli_build_counters.rs; the spec's a1 → a2
  path edit). Gate `task-affected` exit 0, 3578/3578, 8 skipped (`r1-gate.log`). New process test
  `census_cone_scc_line_counts_the_recursive_sccs` pins `add_compose_stats` and the `cone_scc:` line (S1's own
  check included); T10 asserts emitted 4, max 3, walks 4; T3 asserts `own_entries_recursive == 1`. `direct=`: the
  implementer moved the timer INTO `det_cone_inputs` around the per-routine loop (the conductor's literal "after the
  call" would have timed nothing; verified in the diff), so `direct=` covers what it did at step 0 and the plan's
  Task 5 CHANGELOG note about `DIRECT_NANOS` is stale. `check-diff --base origin/master` → `{"reasons": []}`.
- **[a2]** CHECKPOINT (spec §6): `halt-check` → not halted. `stoprule.sh` written verbatim from spec :619-675 to
  `/work/bench/160-7a-t1/stoprule.sh` (`bash -n` ok), run as `agentflow run --name bench --timeout 30 --cwd
  /work/al-sem-issue-160-a2 -- bash /work/bench/160-7a-t1/stoprule.sh 99e0f6b5271734a9c9099ab6ff9e105f03263a31`.
  Result: `{"exit_code": 0, "seconds": 536.2, "queued_s": 0.0, "supervised": true}`, log
  `.agent/runs/20261020-113707-ad56ee/logs/bench/20261020T121922-cd58691c.log` (`OK ... a: 6 samples`, `OK ... c: 6
  samples`, `OK stoprule reading at 99e0f6b5...`). Readings in `/work/bench/160-7a-t1/` (`c-census.txt`,
  `a-spans.txt`). Every run's `cone_scc:` line: `recursive=67 members_all=714 members_emitted=714 roots=0
  self_loops=30 max=479 sizes=1:30 2:9 3-9:22 10-99:5 100+:1 member_walks=714 own_entries=757002 root_set_probe=0.0ms
  root_set_entries=0`; `compose:` `x714 picks=4392766 max_scc_members=479`. `bfs_walk` over the 6 run-`c` files:
  7861.7, 7815.4, 7953.4, 8525.9, 7907.2, 8299.0 ms → `Pw` 7,930.3, `Bw` 710.5. `context.capability_cones` over the 6
  run-`a` traces: 18721.4, 18127.4, 18992.1, 18039.7, 18407.9, 17739.2 ms → median 18,267.6, `Bc` 1,252.9. `R`
  (median `root_set_probe`) 0.0.
  - **S1 pass**: `members_emitted` 714 = `compose:` x714; `members_all` 714 ≥ 714; `max` 479 = `max_scc_members` 479.
  - **S2 pass**: `R` 0.0 ≤ 500 ms. CDO has no recursive ROOT SCC (`roots=0`), so the root-set build never runs on CDO;
    it is covered only by T2 and the new no-emitted-member test. To state in the numbers file and CHANGELOG.
  - **S3 pass**: `Pw − 1000 − Bw` = 6,219.8 ≥ `2 × Bc` = 2,505.8. The speed bound is not void.
  - Recorded: `own_entries` 757,002 (the new path folds each once; reported under `bfs_fold`, not bounded).
  Task 2 may start.
- **[a2]** Task 2 (the switch): `halt-check` → not halted; parked files' sums re-checked (unchanged); sent to the
  same opus implementer (context intact: it parked the tests); `charge subagents` → 55.
- **[a2]** Task 2 committed `0b8082d3b2e3a649b6c8021a451392ce74778f84` (capability_cone.rs, detector_context.rs,
  tests/cli/cli_build_counters.rs). Gate `task-affected` exit 0, 3584/3584, 8 skipped (`t2-gate.log`); hook passed,
  no golden moved. Parked files restored byte for byte (both sums matched before; `cmp` of each inserted range
  silent). Red (`t2-red.log`): T2 0≠1 built, T3b 0≠8 folded, T5/T9 479≠0 walks, T6 0≠20 sets; green `scc_set_tests`
  14/14 (`t2-green2.log`), `cli_build_counters::` 44/44 (`t2-cli.log`). T10 now asserts `member_walks == 0`,
  `(built, borrowed) == (1, 1)`; the census test expects `member_walks=0 sets_borrowed=0 sets_built=1 set_entries=18`
  on ws-d7. `root_set_probe` and its census fields deleted; the `shortcut:` stub marker deleted. **Plan deviation:**
  the planned "root SCC with no emitted member" cannot occur (Tarjan starts only from `nodes`, so an SCC with no
  member in `nodes` is reached from one and is never a root); replaced by `scc_set_none_for_an_scc_with_no_emitted_member`
  (`nodes=[A,C]`; A→X, X→Y, Y→X, X→C). Null proofs, stated: T7 stays green under T1's break (no fixture has a
  recursive SCC with a successor; the T7 limit), and the census test stays green under a no-successor break (ws-d7's
  root has none). `check-diff --base origin/master` → `{"reasons": []}`. `charge task_attempts --sub task2` → 2 left.
  Sonnet reviewer (Task 1's, context intact) asked to verify the Tarjan claim and the §4.1 equality in code;
  `charge subagents` → 54.
- **[a2]** Task 2 review (sonnet): no critical or important finding. Verified in code: the §4.1 equality (anchorless
  merge drops guards as the walk did; own entries and the set's representative share keys, and the fold reads only
  key fields; `direct` populated only for `nodes` on both paths), borrow lifetimes (`fact_cones[&i]` freed only by
  predecessors after the member loop; `&fact_cones[&i]` taken only when `use_set && !root_scc`), the Tarjan claim
  (`tarjan_scc` starts only from `graph.nodes`, so an SCC with no member in `nodes` is never a root), no change for
  RawOnly/Both, the census overlap rule, and every proof break. T5 and T9 plus the census CLI test pin the production
  use. Minor: (m1) `emitted` now counted for every singleton SCC; (m2) T7 empty for successor/own-entry/guarded
  classes, so T8 on CDO is the only real-code oracle for them; (m3) `walk_total`/`BFS_WALK_NANOS` docs drift and a
  stale comment in `inherited_facts_by_bfs`. m1 and m3 folded into Task 3; m2 into Task 4 (T8) and the CHANGELOG.
- **[a2]** Tasks 3 and 4 (both small; one shared review): `halt-check` → not halted; sent to the same implementer;
  `charge subagents` → 53.
- **[a2]** Task 3 committed `5db93bd6f7be5c32d29c4335701950d970ad7295` (capability_cone.rs, detector_context.rs): the
  per-member fold only under `#[cfg(test)] PerMemberWalk` (`emit_picks(subject, picks, mode)` builds the raw Vec
  only; the oracle's fold is a loop at the call site), deletion point documented; plus review minors m1 (`emitted`
  only when recursive) and m3 (docs). Gate `task-affected` exit 0, 3584 passed, 8 skipped (`t3-gate.log`); release
  clippy `scripts/ci-steps clippy` exit 0 (`t3-clippy-release.log`). `check-diff --base origin/master` → `{"reasons": []}`.
  `charge task_attempts --sub task3` → 2 left.
- **[a2]** Task 4: no file changed, so no commit (plan Task 4 step 4). T8 `scc_set_matches_member_walk_on_cdo` run with
  `CDO_WS` on that one command line (`cargo test --profile cdo -p al-sem --lib … -- --nocapture`), exit 0, 1 passed
  (`t4-cdo.log`): cross-app `31658 routines, 67 recursive SCCs, 714 members, max 479, self-loops 30 multi-member 37
  roots 0 non-roots 67 with-successor 64 with-guarded-successor 11 members-with-own-entry 671, 61.3s`; single-app
  `5479 routines, 1 recursive SCCs, 1 members, … with-successor 1 … members-with-own-entry 1, 4.0s` (cdo-profile wall,
  not a performance figure). `recursive_members_emitted == 714` held. T8 covers successor, guarded-successor and
  own-entry SCCs on real code; it does NOT cover a recursive root (CDO has 0) or a root with successors (only T2's
  hand graph). `charge task_attempts --sub task4` → 2 left.
- **[a2]** Shared Task 3+4 review (sonnet, context intact) and Task 5 (CHANGELOG + numbers-file skeleton, corrected
  for the stale `DIRECT_NANOS` note) dispatched in parallel; `charge subagents` → 52, 51.
- **[a2]** Task 3+4 review: no critical or important finding. Clean: Task 3 changes no production output (production
  always takes the empty `PerScc` arm; nothing oracle-only in non-test builds); the moved oracle fold is the true old
  fold (same picks, order and condition; `use_set` false under `PerMemberWalk`); m1/m3 right. Minor: (1) T8 prints its
  class counts but asserts only `members_emitted == 714`, so a moved baseline could drop the classes silently; (2) the
  guard paths have no CDO-level proof (only T1/T4). Both queued for the implementer after Task 5, as one commit
  (minimum asserts in T8's cross-app arm; T8 run once under T4's break). `charge task_attempts --sub task4` → 1 left.
- **[a2]** Task 5 committed `443b37540292768f0a34201cd7029cadfc98461a` (CHANGELOG.md, new
  `docs/measurements/2026-10-20-issue-160-step7a.md`): CHANGELOG under `[Unreleased]` → `### Changed`, "part of #160",
  no closing keyword; states the root-set coverage limit (roots 0 on CDO), T8 as the only real-code oracle for the
  successor/guarded/own-entry classes, the impossible no-emitted-root case, and drops the stale `DIRECT_NANOS` note.
  The numbers file holds the §6 reading and T8, and marks every bench value "pending: §8 bench"; the workspace is named
  only as `cdo_ws`. Gate `task-affected` exit 0 (`t5-gate.log`).
- **[a2]** Task 3+4 review follow-up committed `683b024e450a614a0b64238b89b914b9996bf1ca` (capability_cone.rs, +12
  lines, test only): T8's cross-app arm asserts `multi_member`, `with_successor`, `with_guarded_successor` and
  `members_with_own_entry` ≥ 1. Gate `task-affected` exit 0 (`t4r-gate.log`). T8 under T4's guard break is RED on
  CDO, so the guard path is oracle-checked on real code. Limit: the other three new class asserts share one loop
  with the proven one but were not each broken.
- Implementation complete (Tasks 1-5). `charge task_attempts` remaining: task1 1, task2 2, task3 2, task4 1.
- **[a2]** step 9 task gate: `halt-check` → not halted; `check-diff --base origin/master` → `{"reasons": []}`;
  `agentflow run --name ci-steps-task` → `{"exit_code": 0, "seconds": 48.6, "supervised": true}`, 3584 run / 3584
  passed / 8 skipped (`.agent/runs/20261020-113707-ad56ee/logs/ci-steps-task/20261020T130626-778e090b.log`); tree
  clean outside `.agent/`. Head `683b024e450a614a0b64238b89b914b9996bf1ca`.
- **[a2]** Draft PR NOT opened: **infra fault, the GitHub token on server1 became invalid** after 11:37 UTC (fetch
  and claim worked then). `push-branch` → `{"error": "push of issue/160-… was rejected"}`; `pr-create --draft` →
  `GhError: HTTP 401: Bad credentials`; `gh auth status` → "The token in … hosts.yml is invalid". Read access to
  `origin` still works. Not a verdict on the branch: the lease is held, the operator is notified to run
  `gh auth refresh -h github.com`, and the final panel (which needs no GitHub) runs meanwhile; the push and the draft
  PR follow once the token is fixed.
- **[a2]** step 10 final panel: the built-in `code-review` skill run at `high` on the issue branch (forked, in the
  background). Round 1 started in parallel: `charge final_rounds` → remaining 4; `charge pi_calls` → 19; `charge
  subagents` → 50. Briefing `.agent/runs/20261020-113707-ad56ee/final-panel-r1-briefing.md` (branch diff
  `branch-r1.diff`, 3387 lines; spec sha256 prefix `6ce7e30647e9c551`; spec changes since the handoff
  `spec-since-handoff.diff`, the N1 fix and the a1 → a2 paths); it asks for every finding, F1-F7, and a re-mark of
  all 28 carried entries. Reviewers: sol (its spec-panel thread) → `pi-sol-final-r1.md`; opus (the spec-panel
  reviewer, context intact) → `opus-final-r1.md`.
- **[a2]** Final panel round 1 results. `code-review` high (`code-review-high-r1.md`): 9 findings, "no correctness bug
  in the production change". Opus: no critical or important finding, F1-F7 CONFIRM, all 28 carried entries accepted,
  5 minor doc points. Sol: 1 important (the §8 oracle grep misses T8's printed lines) and 6 minor, F1/F2/F3/F5
  CONFIRM, F4/F6/F7 REJECT on those minors, all 28 carried entries accepted. Carried entries: all 28 now accepted by
  both against spec `6ce7e30647e9c551` (S4-1/S4-2 included). New entries F1-1..F1-19, each claim checked at its line
  by the conductor before it was written: F1-1 important; the rest minor. Refuted with evidence: F1-4 (the remaining
  per-member fold is 7b's, D7a-2/A7a.7), F1-9 (the cfg(test) variant is spec §4.3's design, confirmed C7/F5), F1-10
  (`scripts/kraken-setup.sh` is not in the branch; code-review diffed against the stale local `master`; carried to
  Discoveries). Deferred (minor): F1-3 (Both folds the set and walks; correctness unaffected; only roots pay a build,
  0 on CDO; the bench's Part 3 `policy check` wall decides), F1-6 (bench placeholders, filled after the bench and
  before attest). F1-19 fixed by the conductor (the ledger's 'Not delivered'). Fixes dispatched: spec author F1-1,
  F1-14, F1-16, F1-17 (spec and plan, no commit); implementer F1-2, F1-5, F1-7, F1-8, F1-11, F1-12, F1-13, F1-15,
  F1-18 (one commit). `charge subagents` → 49, 48.
- **[a2]** Round-1 fixes: implementer `7f19daa111488040d0ac277c7cb78949a68b61b7` (CHANGELOG, numbers file,
  capability_cone.rs, cone_derived.rs, detector_context.rs, cli_build_counters.rs; gate `task-affected` exit 0,
  `p1-gate.log`), including F1-7's move of the class counts into production `ComposeStats` (the `cone_scc:` census
  line gains `own_entry_members= with_successor= with_guarded_successor=`; T7 and T8 green with unchanged classes,
  T8 red on CDO under the production class break). Spec author's spec and plan fixes committed by the conductor as
  `9445588be5971e98c4d8b834c1944631f47bd755`. `check-diff --base origin/master` → `{"reasons": []}`; supervised
  `ci-steps-task` → exit 0, 44.7 s, 3584/3584, 8 skipped (`logs/ci-steps-task/20261020T133615-d01e94e7.log`); tree
  clean. Register: 42 fixed, 3 refuted, 2 deferred, none open; every hash moved to spec `5e79028b36d38d86` and every
  mark reset to `unreviewed`.
- **[a2]** Final panel round 2: `charge final_rounds` → remaining 3; `charge pi_calls` → 18; `charge subagents` → 47.
  Briefing `final-panel-r2-briefing.md` (fix diff `final-r1-fixes.diff`, 837 lines) → `pi-sol-final-r2.md`,
  `opus-final-r2.md`.
  Outcome: opus accepted all 47 (refutations and deferrals included) and raised F2-1 (minor: the
  `with_guarded_successor` count scans successor cones on every production compose for a census-only number). Sol
  accepted 44 and re-raised 3 minor doc points: F1-15 (a comment still calls a root's cone build waste), F1-17 (the
  plan's census-line note predates F1-7's three fields), F1-18 (the numbers file's 'same two lines each' is false
  for the red class-break run). All four verified at their lines by the conductor. Not converged. Sent to the
  implementer as one commit; `charge subagents` → 46.
- **[a2]** Round-2 fixes committed `7bb9f90c4cfbf61ab2a0e72c03a048f8b07eb2ea` (capability_cone.rs, the plan, the
  numbers file): the guarded-successor scan gated under `cfg!(test) || cones_census::enabled()`; three doc fixes.
  `check-diff --base origin/master` → `{"reasons": []}`; supervised `ci-steps-task` → exit 0, 55.1 s, 3584/3584
  (`logs/ci-steps-task/20261020T134907-f666e4e2.log`); tree clean. Spec unchanged (`5e79028b36d38d86`).
- **[a2]** Final panel round 3: `charge final_rounds` → remaining 2; `charge pi_calls` → 17; `charge subagents` →
  45. Briefing `final-panel-r3-briefing.md` (fix diff `final-r2-fixes.diff`, 73 lines). Sol (`pi-sol-final-r3.md`)
  and opus (`opus-final-r3.md`): F1-15, F1-17, F1-18 and F2-1 accepted; the diff breaks none of the 44 accepted
  entries; no new finding. **Final panel converged**: 48 entries, every one accepted by both reviewers against spec
  `5e79028b36d38d86`; 43 fixed (14 important), 3 refuted, 2 minor deferred (F1-3 to the bench's `policy check`
  wall; F1-6 until the bench fills the placeholders, before attest); no blocking entry deferred.
- **[a2]** CDO bench (once, after the panel): `halt-check` → not halted. §8's three scripts written verbatim from
  the converged spec to `/work/bench/160-7a/part{1,2,3}.sh` (`bash -n` ok). Part 1 run as `agentflow run --name
  bench --timeout 45 --cwd /work/al-sem-issue-160-a2 -- bash /work/bench/160-7a/part1.sh
  7bb9f90c4cfbf61ab2a0e72c03a048f8b07eb2ea`.
  Part 1 → `{"exit_code": 0, "seconds": 472.1, "supervised": true}`, log
  `.agent/runs/20261020-113707-ad56ee/logs/bench/20261020T135353-78209f2d.log` (`OK part1`). Shas: parent
  `d37e8634d72699b0d66e8b7636c16aecb3d45611`, head `7bb9f90c4cfbf61ab2a0e72c03a048f8b07eb2ea`. **A7a.3**: CDO SARIF
  byte-identical at `--scope primary` and `--scope all` (both `cmp` silent); histogram SHA-256
  `c032c8db56f1fd070fc4b7e4f1971a73019a4c8569db81a6aaeb7e0c1a1594ef` both sides (= step 4's). **A7a.6**: retained
  `context.cone_derived` 65,273,122 B and `context.summaries` 36,400,882 B, parent = head. Oracle (T8, cdo profile):
  cross-app `31658 routines, 67 recursive SCCs, 714 members, max 479, … with-successor 64 with-guarded-successor 11
  members-with-own-entry 671, 60.3s`; single-app `5479 routines, 1 recursive SCCs, … 4.1s`; `1 passed`.
  Part 2 → `{"exit_code": 0, "seconds": 701.8, "supervised": true}`, log
  `logs/bench/20261020T140206-a8602bdc.log` (`OK … bench-a: 10 samples`, `OK … bench-c: 10 samples`, `OK part2`).
  Run `a` (census off, 5 rounds per side): `context.capability_cones` parent median `Pc` 18,782.9 ms, band `Bc` 904.3;
  head median `Hc` 10,104.4, band 662.8; head lower in 5/5 rounds (`/work/bench/160-7a/bench-a-spans.txt`). Run `c`
  (census on): parent `bfs_walk` 8347.4, 8517.1, 8304.2, 8295.9, 7958.6 ms → `Pw` 8,304.2, `Bw` 558.5; head
  `bfs_walk` 0.0 ms in all 5; `bfs_fold` parent 779.9, 731.6, 753.7, 747.1, 720.3 (median 747.1), head 394.0, 438.4,
  396.9, 383.7, 394.4 (median 394.4); every head `cone_scc:` line `member_walks=0 own_entries=757002 sets_borrowed=67
  sets_built=0` (`bench-c-bfs.txt`).
  - Void check: `Pw − 1000 − Bw` = 6,745.7 ≥ `2 × Bc` = 1,808.6 → the bound is not void.
  - **A7a.5 met**: `Pc − Hc` = 8,678.5 ≥ `Pw − 1000 − Bw − Bc` = 5,841.4 (7A-1); literal form 8,678.5 ≥ 6,745.7 also met.
  - **A7a.4 met**: head `bfs_walk` median 0.0 ms ≤ 1,000 ms.
  - stdout SHA equal across sides in both runs (`check`).
  Part 3 → `{"exit_code": 0, "seconds": 922.7, "supervised": true}`, log `logs/bench/20261020T141416-4bf0fa77.log`
  (`OK … bench-r: 20 samples`, `OK part3`; `/work/bench/160-7a/bench-r-summary.txt`). `as` (`analyze --single-app`):
  wall parent 3.8 s (band 0.5), head 3.8 s (band 0.5), +0.1 s, head higher 4/5 (inside the band); polled RSS 693.3 vs
  699.1 MB; stdout `c6c4d832599851f3` both. `px` (cross-app `policy check`): wall parent 85.0 s (band 16.7), head
  76.9 s (band 3.3), −8.1 s, head lower 5/5; polled RSS 13,806.9 vs 13,816.1 MB (+9.2, inside the band); stdout
  `a1b8f1ad82042442` both. Trace peaks: none on Linux (`read_rss` stub), as stated.
  Bench complete; every part green, so no bound is "not measured".
- **[a2]** Bench numbers committed `91a66203a99e4d8901fc8b64477cbf4f564bb5c6` (CHANGELOG.md, numbers file; gate
  `task-affected` exit 0, `bench-docs-gate.log`): no `<bench>` or 'pending: §8 bench' marker left (grep count 0 in both
  files); every figure cross-checked against `/work/bench/160-7a/`. Register F1-6 → fixed; F1-3 → refuted by the Part 3
  measurement. Register now 44 fixed, 4 refuted, none open or deferred; all 48 accepted by both reviewers.
- **[a2]** step 11: `git fetch origin` (read access works) → `origin/master` still `68253a8e15b7cb1856f9f6536dabbe6b02702b23`,
  the branch's merge-base, so no rebase, no `charge rebase_regate`, and the range-diff is empty (the branch's commits
  are unchanged): no extra final-panel round. **B = `68253a8e15b7cb1856f9f6536dabbe6b02702b23`, H =
  `91a66203a99e4d8901fc8b64477cbf4f564bb5c6`.** Full gate set started in order, each supervised: `ci-steps-all`,
  `check-goldens-coverage`, `check-goldens`, `cdo-gate` (the executor sets `CDO_WS` from `cdo_ws`).
  All four exit 0, supervised (see Gate results).
- **[a2]** step 12 freeze: `halt-check` → not halted; `check-diff --base 68253a8e… --head 91a66203…` → `{"reasons": []}`;
  `origin/master` re-fetched, still B. The numbers file's step-11 `cdo-gate` line filled (193.5 s; gate-inert prose
  after H). `sanitize` on the ledger, the register and the numbers file → `{"violations": {}}`. Evidence and that line
  committed after H, then `freeze-check` and `attest` (`--substitute astra=sol --substitute flash=opus`, the
  operator's instruction for this loop; the claim's `handoff.reviewers` records astra = sol, flash = opus).

## Acceptance matrix

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| T10 `compose_stats_counts_scc_shapes` | capability_cone.rs:2613 `if recursive {` → `if recursive \|\| scc_entry.members.len() == 1 {` | `recursive_sccs` left 3, right 2 (`t1-proof-t10-break.log`) | 1 passed (`t1-proof-t10-revert.log`) | 68a7d622 |
| T11 `scc_set_skips_members_outside_nodes` | :2653-2655 delete `if !routine_ids.contains(m) { continue; }` | `row_ids()` left `{"A","B","C"}`, right `{"A","C"}` (`t1-proof-t11-break.log`) | 1 passed (`t1-proof-t11-revert.log`) | 68a7d622 |
| T1 `scc_set_rows_hand_stated` (precondition) | :2620 delete `stats.recursive_sccs += 1;` (grep count 0) | `precondition: one recursive SCC, {A, B}` left 0, right 1 (`t1-proof-t1-break.log`) | `scc_set_tests` 8 passed (`t1-proof-t1-revert.log`) | 68a7d622 |
| `census_cone_scc_line_counts_the_recursive_sccs` | capability_cone.rs:2851 delete `...cones_census::add_compose_stats(&stats);` (grep count 0) | starts_with at tests/cli/cli_build_counters.rs:779; line reads `cone_scc: recursive=0 members_all=0 ...` (`r1-proof-cli-break.log`) | 1 passed (`r1-proof-cli-revert.log`) | 99e0f6b5 |
| T10 (new fields) | :2665 delete `stats.member_walks += 1;` | `member_walks` left 0, right 4 (`r1-proof-stats-break.log`) | `scc_set_tests` 8 passed (`r1-proof-stats-revert.log`) | 99e0f6b5 |
| T3 (own entries) | :2666 delete `stats.own_entries_recursive += own_entries(m).len() as u64;` | `own entries of the recursive members ...` left 0, right 1 (`r1-proof-stats-break.log`) | `scc_set_tests` 8 passed (`r1-proof-stats-revert.log`) | 99e0f6b5 |
| T1 §5.2 | `fact_cone_for_scc` successor loop: `if anchor.is_none() { break; }` | `A: writes` left `["t/A"]`, right `["t/A","t/C"]` | ok | 0b8082d3 |
| T7 under T1's break | same | **green (null)**: no fixture has a recursive SCC with a successor (the stated T7 limit) | — | 0b8082d3 |
| T2 §5.3 | root set `.then(\|\| fact_cone_for_scc(..))` → `.then(ConeFacts::new)` | `X: writes` left `[]`, right `["t/S"]` (fails on X, not Y as §5.3 says) | ok | 0b8082d3 |
| T3 break 1 | `own_entries(m)` → `own_entries(&scc_entry.members[0])` | `Q's row` left `[]`, right `[…/table/50100]` | ok | 0b8082d3 |
| T3 break 2 + T3b | delete the own-entry fold loop | T3 `Q's row` left `[]`; T3b `M1's row` left `[]` (its preconditions held) | ok | 0b8082d3 |
| T4 §5.5 | `if anchor.is_some() && frame_bound(entry)` → `if frame_bound(entry)` | `A: the T2 write` left `[]`, right `[…/table/50100]` | ok | 0b8082d3 |
| T5 §5.6 | walk condition gains `\|\| mode.wants_derived()` | `no per-member walk under DerivedOnly` left 479, right 0 | ok | 0b8082d3 |
| T6 §5.6 | build `fact_cone_for_scc` per member inside the loop | `chain: one set per SCC` left 60, right 20 | ok | 0b8082d3 |
| T9 (a) | `compose_cone_over_graph` passes `PerMemberWalk` | `... runs a per-member walk under DerivedOnly` left 479, right 0 | ok | 0b8082d3 |
| T9 (b) | `emit_picks` without `sort_inherited` | `T1 Both: A's raw inherited Vec` left `[e/B, t/C]`, right `[t/C, e/B]` | ok | 0b8082d3 |
| T11 §5.5b (post-switch) | delete `if !routine_ids.contains(m) { continue; }` | `row_ids()` left `{"A","B","C"}`, right `{"A","C"}` | ok | 0b8082d3 |
| `scc_set_none_for_an_scc_with_no_emitted_member` | drop `&& emitted > 0` from `use_set` | `no set for an SCC with no emitted member` left 1, right 0 | ok | 0b8082d3 |
| T10 (changed) | delete `stats.scc_sets_built += 1` | `one set built (the root), one borrowed (the self-loop)` left (0, 1), right (1, 1) | ok | 0b8082d3 |
| census test (changed), break 1 | root set built with no successors (`&BTreeSet::new()`) | **green (null)**: ws-d7's root SCC has no successors | — | 0b8082d3 |
| census test (changed), break 2 | the T2 empty-root-set break | starts_with fails; line reads `… sets_built=1 set_entries=0 …` | 44/44 ok | 0b8082d3 |
| T9 (a), re-run after Task 3 | `compose_cone_over_graph` passes `PerMemberWalk` | `… runs a per-member walk under DerivedOnly` left 479, right 0 (`t3-proof-T9a-break.log`) | ok | 5db93bd6 |
| the moved oracle fold (T1's old-vs-new) | delete the fold loop in the `PerMemberWalk` arm (grep 0) | T1: `A: writes_tables_of differs` | `scc_set_tests` 14/14 (`t3-proof-revert.log`) | 5db93bd6 |
| T8 on CDO (§5.8, T3 break 2) | delete the `for entry in own_entries(m)` fold after the set fold (grep 0) | `CDO (cross-app): r0/00f472ff…: writes_tables_of differs`, exit 101 (`t4-cdo-proof-break.log`) | 1 passed, same two lines (`t4-cdo-proof-revert.log`) | no commit (Task 4) |
| T8 on CDO, guard path (T4's break) | `if anchor.is_some() && frame_bound(entry)` → `if frame_bound(entry)` (grep 0) | `CDO (cross-app): r0/00f472ff…: writes_tables_of differs`, exit 101 (`t4r-cdo-T4break.log`) | 1 passed (`t4r-cdo-revert.log`) | 683b024e |
| T8 new class asserts | remove `c.with_guarded_successor += 1` from `count_classes` (grep 0) | `CDO cross-app: no with-guarded-successor class`, printed `with-guarded-successor 0`, exit 101 (`t4r-cdo-classbreak.log`) | 1 passed, cross-app `… with-guarded-successor 11 members-with-own-entry 671, 63.4s` (`t4r-cdo-revert.log`) | 683b024e |

## Findings register summary

28 spec-panel entries over 5 rounds (S1-1..S1-23 from round 1; S2-1, S2-2; S3-1; S4-1, S4-2), 14 important
and 14 minor, all `fixed`. Marks (sol / opus): 26 accepted / accepted; S4-1 and S4-2 re-raised / accepted
(sol N1, fixed after the cap, unreviewed). Reviews: `.agent/runs/20261020-072815-106c18/pi-sol-spec-r{1..5}.md`
and `opus-spec-r{1..5}.md`.

## Gate results

Attempt 2, step 11, on H `91a66203a99e4d8901fc8b64477cbf4f564bb5c6` (B `68253a8e`), each through `agentflow run`,
`supervised: true`, `queued_s` 0.0 (logs under `/work/alsem/.agent/runs/20261020-113707-ad56ee/logs/`):

| gate | exit | seconds | log | notes |
|---|--:|--:|---|---|
| `ci-steps-all` | 0 | 272.6 | `ci-steps-all/20261020T143409-eae64745.log` | nextest 3584 run / 3584 passed / 8 skipped; 0 FAILED; release clippy, release build, `perf_bounds` (CI's exact bar) |
| `check-goldens-coverage` | 0 | 0.0 | `check-goldens-coverage/20261020T143842-00875ced.log` | `coverage OK (36 golden dirs declared)` |
| `check-goldens` | 0 | 0.1 | `check-goldens/20261020T143842-5c1f5e4a.log` | `this exact tree passed 'scripts/ci-steps test' -- goldens already verified`: the tree stamp from `ci-steps-all`'s own test step on H, which runs every golden target (CLAUDE.md "Test tiers") |
| `cdo-gate` | 0 | 193.5 | `cdo-gate/20261020T143842-b887e35f.log` | `ENFORCE_CDO_WS=1`; `l4_summary_differential` 20 passed; `--lib` 2178 passed, 0 failed, 4 ignored (T8 included); `cdo-gate: PASS` |

Tree clean outside `.agent/` after the gates. Toolchain `rustc 1.96.0 (ac68faa20 2026-05-25)`; grammar
`tree-sitter-al` at `8682cb5f`; CDO workspace at `bc3ccb18`, identity sha256(path) prefix `4429175c44dc4ad6` (the path
itself kept out of git). Earlier task gates (attempt 2): `ci-steps-task` exit 0 on `683b024e`, `9445588b` and
`7bb9f90c` (3584/3584 each). CDO bench: see Timeline (all three parts exit 0; A7a.3-A7a.6 met). The head's whole
`cdo-gate` wall, for the numbers file's step-11 line: 193.5 s.

## Discoveries

Not filed by attempt 1 (a handoff files nothing); carried for the attempt that merges. Attempt 2 adds the third
entry (register F1-10).

```json
[
  {"subsystem": "perf_trace", "locator": "src/engine/perf_trace.rs:174-177", "symptom": "read_rss() returns None off Windows, so on Linux (server1) the trace has no peak.process_mb / peak.post_attach_mb and spans carry no rss_mb; #160's peak bounds (steps 6, 9, 10, 12; the post-attach peak) cannot be measured there", "kind": "instrument gap", "origin_issue": 160, "reproducer": "ALSEM_TRACE=1 ALSEM_TRACE_STDERR=1 alsem analyze <CDO workspace> --format sarif 2>&1 | grep peak.  (prints nothing on Linux)", "pre_existing": true, "capability": "a Linux /proc/self/status VmRSS/VmHWM reader", "acceptance": "on Linux the trace prints peak.process_mb and peak.post_attach_mb and spans carry rss_mb; a test pins it off Windows"},
  {"subsystem": "l4 scc_stats event", "locator": "src/engine/l4/analysis.rs:220-222", "symptom": "`nodes` counts graph.nodes, which repeats colliding routine ids, so nodes != the sum of SCC members (31,658 vs 31,656 on CDO cross-app); the step-0 numbers file recorded it as an unexplained inconsistency", "kind": "instrument (minor)", "origin_issue": 160, "reproducer": "ALSEM_TRACE=1 alsem analyze <CDO workspace>; read the scc_stats event", "pre_existing": true, "capability": "count distinct nodes, or add a duplicate_nodes field", "acceptance": "the event's node count equals the sum of members, or names the duplicates"},
  {"subsystem": "kraken setup (owner action; the flow may not edit scripts/)", "locator": "scripts/kraken-setup.sh:13", "symptom": "the tree-sitter-al exclude line is appended to $(git rev-parse --git-common-dir)/info/exclude under set -euo pipefail without creating info/ first, so a clone without .git/info (empty or custom template) aborts the rest of kraken setup, including the pi-mcp clone", "kind": "bug", "origin_issue": 160, "reproducer": "git clone --template=/nonexistent-empty-dir <repo> x && cd x && bash scripts/kraken-setup.sh  -> 'No such file or directory' on the exclude append, setup stops", "pre_existing": true, "capability": "kraken setup succeeds on a clone without .git/info", "acceptance": "the script runs mkdir -p on the exclude file's directory before appending; a clone without .git/info completes setup"}
]
```

## Not delivered

**Attempt 2 (current).** Code, tests, plan, CHANGELOG and the numbers-file skeleton are delivered (Tasks 1-5).
Still open at the time of writing:
- the one CDO bench (§8, three parts), run after the final panel converges; until then every bench value in the
  CHANGELOG and the numbers file is a placeholder (register F1-6), and A7a.3-A7a.6 are unmeasured;
- the step-11 full gate set (`ci-steps all`, `check-goldens --verify-coverage`, `check-goldens`, `cdo-gate`);
- the push, the draft PR and GitHub CI: blocked by the infra fault that made server1's GitHub token invalid
  (Timeline, step 9); not a verdict on the branch;
- coverage limits, stated in the CHANGELOG: CDO has no recursive root SCC (the root-set build is hand-tested only),
  T7's fixtures reach only multi-member SCCs and roots, and T8 is the only real-code oracle for successor,
  guarded-successor and own-entry SCCs.

**Attempt 1 (as recorded then).** It delivered the step-7a DESIGN only, handed off as pre-ratified at the
spec-panel cap (#45). No code, no tests, no plan, no gate run, no bench. The next attempt started at acceptance
tests (`/issue` step 6) from the handed-off spec and this register.

Open for the next attempt (attempt 1's list):
- Final panel round 1 must re-mark S4-1 and S4-2 against the fixed spec (see Timeline).
- Every new test and break in the spec is a hand trace that has never run; each asserts its precondition
  first, so a wrong trace fails loudly.
- Peaks: `read_rss` returns None off Windows, so server1 traces carry no peak (step 7a has no peak bound).
- Candidate discoveries from the probes (Linux `read_rss`; the `scc_stats` node count) are left for the
  attempt that merges; psutil was an image gap, since resolved.
