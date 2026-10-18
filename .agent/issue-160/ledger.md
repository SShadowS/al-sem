# Issue #160 ledger

## Claim

```json
{"run_id":"20261018-001557-11efb6","issue":160,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/160-alsem-batch-build-the-model-onc-a1","worktree":"U:\\Git\\al-sem-issue-160-a1","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`.
Worktree created from master `1e54624a`.

**Attempt 2 (current):**

```json
{"run_id":"20261018-125302-fa1912","issue":160,"attempt":2,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/160-alsem-batch-build-the-model-onc-a2","worktree":"U:\\Git\\al-sem-issue-160-a2","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Base `60522971afd71f6853b7120bfef5f9a3d7460991` (`make-worktree`, from `origin/master`).
Attempt 1 was recovered as crashed by a parallel session, because its lease went stale: design workflows ran about 2.7 h with no `beat`. Its specs and ledger were carried over unchanged from the preserved worktree (`al-sem-issue-160-a1.crashed-20261018-095737`).

## Classification

**Architectural.** A new CLI entry point that runs several existing subcommands
against one shared model build. It touches every participating subcommand's entry
path in `src/bin/alsem.rs` (each must be split into "build model" and "project
from model"), and it carries a byte-identity contract against the separate
commands. That is more than a few-paragraph bounded change.

## Assumption probes

Full probe: `.agent/runs-probe-160.md` (untracked). CDO = `U:/Git/DO-cdo-baseline/Cloud`, release-fast, 2 runs per command.

| probe | result | evidence |
|---|---|---|
| One shared build serves analyze + digest + fingerprint | **FALSIFIED.** fingerprint/prove/diff always build single-app with the Summary profile; analyze/digest default cross-app FULL. The issue's example batch needs 2 builds | probe P1 |
| Model build dominates run time | **FALSIFIED.** analyze 36.1 s: snapshot+parse ~2.0, resolve ~3.0, model assembly ~8.7 (cross-app build 13.8 s, 38 %), detector context 21.6 s (60 %; `context.capability_cones` 20.9 s), detectors ~0.2, output ~0.4 | probe P5 trace |
| Per-command times | analyze 43.6/37.5 s, 2.84 GB; digest 66.8/63.7 s, 2.84 GB; fingerprint 7.5/7.0 s, 1.38 GB; noise ~±8 % | probe P5 |
| digest builds its detector context once | **FALSIFIED.** `compute_tx_context_map` (`digest_cli.rs:543`) builds a second ~21 s context beside the diagnostics detector run (`:1655`) | probe P2 |
| Separate runs are byte-stable (Acceptance 1) | **FALSIFIED** without `--deterministic`: digest and fingerprint differ run-to-run on `generatedAt`; SARIF is stable | probe P6 |
| Saving from sharing the model only | ~14 s of ~106 s (≈13 %, inside noise). Sharing the default-set detector context too: ~57 s (≈54 %) | probe P6 |

## Base/Head SHAs

- Branch base (make-worktree): `60522971afd71f6853b7120bfef5f9a3d7460991`.
- Before the rebase, the final head was `a5d2424c8028206afa0206b891fab8df663e8fbe`. The final panel converged there: 47 entries at hash `1c327b36bf780baa`, rounds 1-5.
- The rebase onto `origin/master` (`rebase_regate` charged once) hit one conflict, in CHANGELOG.md under `[Unreleased]`. The resolution keeps #160's Added and Changed entries before #104's Fixed entry. `git range-diff` shows the other 18 commits unchanged (`=`); only the CHANGELOG context of `docs: #160 step 0 numbers` moved. No code or test changed, so no further final-panel round was needed (and `final_rounds` is at its cap).
- **B = `1e9492d84d8b0c3ff0017c1bc5514d33f1f2a229`** (origin/master), **H = `1071a16d390c95d398e8b87463cda84d8d4383a3`**.

## Timeline

- Claim + worktree at master `1e54624a`; preflight ok; heartbeat 1792275490.
- Probes dispatched (read-only subagent; code map + CDO phase timing).
- Owner decision (2026-10-18): best solution; #160 re-scoped to the analysis-session epic; fingerprint/prove/diff become cross-app. Three external-review claims investigated: modelInstanceId (true, intended), is_dependency_role stub (true, harmless today, fixed at step 8/9), digest source-only base (CONFIRMED bug, filed #161).
- Design workflow (4 readers + cone timing, 2 designs, judge, 2 critics), then 5 revision rounds (blocking 11, 11, 4, 5, 6: not converging). Split into the epic (contract level, 715 lines) and a detailed steps 0-2 spec; 3 critic rounds on the latter (blocking 12, 9, 6).
- Attempt 1 recovered as crashed (stale lease). Attempt 2 claimed at base `60522971`; specs carried (`5eca5bb5`). A background heartbeat loop now runs every 8 min.
- Child spec: 3 critic rounds (blocking 4, 5, 2), then a fix pass for all 13 findings (`48e95c74`).
- Spec panel (gpt-6.1-sol in the astra slot, claude-opus-5.5 in the flash slot; `spec_rounds` 4 of 5):
  - r1: 17 findings, merged into 15 entries, all fixed (`1b371d69`).
  - r2: 2 re-raised plus 5 new (`6fe03727`).
  - r3: all 20 accepted, 3 new minor (`55377697`).
  - r4: all 23 accepted by both at hash `885d97a30ca54f85`, no new findings. **CONVERGED.**
- Owner decision (2026-10-18): #160 stays open as the epic. This run builds steps 0, 1, 1b and 2. Its acceptance matrix is the steps 0-2 spec's, and its PR says "Part of #160", not "Closes #160". This is a stated exception to issue.md step 13's rule, made by the owner.
- Plan `e4ca0516`: 28 tasks across steps 0, 1, 1b and 2. `plan_tasks` was exhausted at the 13th charge (cap 12). The plan is NOT cut down to fit; the steps are independently mergeable. **This run delivers step 0 only (plan tasks 1-8).** Steps 1 (tasks 9-15), 1b (16-18) and 2 (19-28) follow as later runs of #160, each under the cap.

## Acceptance matrix

## Discrimination proofs

Every break was made with the Edit tool and applied exactly once. Each failing line below names its own break. Full detail, including the code-review and final-panel fixes: `.agent/runs/20261018-125302-fa1912/final-r1-dispositions.md` and `code-review-r1-dispositions.md`.

| plan task | test | break | failing line | revert |
|---|---|---|---|---|
| 1 (A0.3) build.program | the 20 `assert_row` row tests in `cli_build_counters` (the other 8 tests in that module do not read `build.*` and cannot fail on these breaks) | a real second `build_program_with_coverage_profiled(.., FULL)` in `build_program_with_coverage` (full.rs), at 7ce88bab | `[query single-app] build.program: got 2, want 1` (10 rows failed; diff: got 4, want 2) | green 20/20 |
| 1 build.combined_graph | same 20 row tests | a real second `build_combined_graph` in `build_cross_app_base` | `[analyze cross-app] build.combined_graph: got 4, want 3` (8 cross-app rows) | green |
| 1 build.cone | same | a real second `compose_cone_over_graph` in `build_detector_context_with` | `[analyze single-app] build.cone: got 2, want 1` | green |
| 1 build.summaries_v2 | same | a real second `compute_summaries_v2_bundle_with_leaves` in the context | `[analyze cross-app] build.summaries_v2: got 2, want 1` | green |
| 1 build.graph_scc | same | the counter placed inside the solver's inner Tarjan (`db_effect_solver.rs:99`); this is the one counter mutation the spec allows, because no second builder call exists for it | `[query single-app] build.graph_scc: got 5, want 1` | green |
| 1 build.pd_solve | same | a real second `pd_solve::solve(producers.clone(), ..)` | `[analyze cross-app] build.pd_solve: got 2, want 1` | green |
| 1 build.detector_context | same | a real second `build_detector_context_with` in the cross-app runner | `[analyze cross-app] build.detector_context: got 2, want 1` | green |
| 1 build.snapshot | same | a real second `compose_snapshot_from_base` (`snapshot_full.rs:72`) | `[fingerprint json] build.snapshot: got 2, want 1` (diff: got 4, want 2) | green |
| 1 build.ordering_facts | the d47 row's own `assert_eq` | a real second `compute_ordering_facts` in the ordering fill | `[analyze cross-app d47] build.ordering_facts left: 2 right: 1` | green |
| 2 | `census_rest_is_never_negative` | charge `SINGLETON_NANOS` twice, plus the whole compose time at exit | `negative compose remainder ... rest=-0.384ms` | green |
| 3 (transient) | `pd_solve::tests::bytes_split_sums_to_the_old_total` at 6632fa74 (temporary worktree) | drop `reps` from `working_bytes` | `pd_solve.rs:1139 left: 1305 right: 1834` | green |
| 5 | `phase_peak_is_reported`, `diff_ends_each_sides_phase` | `mark_phase_start` returns at once | `post_attach=0 process=22`; `cli_build_counters.rs:681 left: []` | green |
| 6 | `retained_census_changes_no_output` | (1) the bucket prints to stdout; (2) the `model` bucket is removed | `census changed stdout`; `missing bucket=model` | green |
| 7 | `--test r3` | the wrapper passes an emptied graph to `_over` | 3 failed (`oracle_r3a3_inherited_keys_trace_to_a_direct_producer`, `pd_arena_links_are_snapshot_edges`, `a5_cone_l4_class_parity_per_producer`) | green 63/63 |
| CR1-CR10, F2-1..F2-6 | see the two dispositions files | | | |

Note (F2-4/F3-1): the round 1 re-run in `final-r1-dispositions.md` changed counter deltas from 1 to 2 and reported "26 of 29". Those runs check counter values only. They are not the second-builder-call proofs the spec asks for. The Task 1 rows above are the original second-call proofs. Each was made with Edit and tagged `// BREAK160`, once per proof. They were run at 7ce88bab, the commit that added the counters, and the counters have not changed since.

## Findings register summary

## Gate results

Round 1, at HEAD `b68a7845` (step 0 complete). All gates ran supervised under run `20261018-125302-fa1912`. Logs are under `.agent/runs/20261018-125302-fa1912/logs/`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 373.9 | ci-steps-all/20261018T184055-1a156ec0.log |
| check-goldens-coverage | 0 | 0.6 | check-goldens-coverage/20261018T184709-7277d08d.log |
| check-goldens | 0 | 64.9 | check-goldens/20261018T184710-9850c8f5.log |
| cdo-gate | 0 | 102.0 | cdo-gate/20261018T184815-ba00068a.log |

`git status --porcelain -- . ':!.agent'` is empty after the gates. No golden moved during step 0.

The gates also passed after the code-review fixes (at a32d0519; stopped and re-run after the round 1 fixes) and at 0d2ea47c (ci 690.1 s, goldens 64.4 s, cdo 198.1 s, all exit 0).

**Final, at H `1071a16d` (after the rebase onto B `1e9492d8`):**

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 725.3 | ci-steps-all/20261018T202854-bab51ec6.log |
| check-goldens-coverage | 0 | 0.3 | check-goldens-coverage/20261018T204100-97515c57.log |
| check-goldens | 0 | 64.5 | check-goldens/20261018T204100-b41b2ba9.log |
| cdo-gate | 0 | 201.8 | cdo-gate/20261018T204205-2543c10f.log |

All four ran supervised, and the tree was clean afterwards. Toolchain: Rust 1.96. Grammar: tree-sitter-al `main` at the committed `node-types.sha256`. CDO_WS: the pinned `U:/Git/DO-cdo-baseline/Cloud` (enforced by cdo-gate's stamp checks).

## Discoveries

```json
[{"subsystem": "l5/detector_context scc_stats", "locator": "src/engine/l5/detector_context.rs:1390", "symptom": "On CDO analyze cross-app, the trace's scc_stats event reports nodes 31658 in sccs 31656 with max_scc 1. These cannot all hold. The cone walk's census reports max_scc_members 479 on the same command.", "kind": "bug", "origin_issue": 160, "reproducer": "ALSEM_TRACE=1 ALSEM_TRACE_FILE=t.json alsem analyze U:/Git/DO-cdo-baseline/Cloud --format sarif, then read the scc_stats instant event (nodes, sccs, max_scc). With ALSEM_CONES_CENSUS=1 on the same command, the census prints max_scc_members=479. Unverified hypotheses: nodes is graph.nodes.len() (:1445), which may hold duplicate ids that tarjan_scc folds; and the context's tarjan runs over edges_by_from only (:1392-1400), while the cone walks a graph that also carries the injected dependency typed edges.", "pre_existing": true, "capability": "Epic step 7a's stop rule reads step 0's SCC distribution. It needs one SCC population that is self-consistent and names the graph it describes.", "acceptance": "scc_stats' nodes equals the sum of its SCC sizes, and the numbers file states which graph each of the two SCC figures is over, with the difference explained by source."}]
```

Filed separately during probing: #161 (cross-app digest misses dependency-internal effects).

## Not delivered
