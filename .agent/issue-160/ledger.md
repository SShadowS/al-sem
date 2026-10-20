# Issue #160 ledger: attempt 6 (epic step 3: fold from uncertainty edges)

Earlier attempts' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170), step 1b at `6c6bce17` (PR #172), step 2 at `d4d8b71f` (PR #174). This file replaces them for attempt 6.

Part of #160. This PR delivers epic step 3 only (spec
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §9, row **3**).
#160 stays open until its last step (owner decision, 2026-10-18): the PR says "Part of
#160" and carries no closing keyword.

## Claim

```json
{"run_id":"20261020-012541-985513","issue":160,"attempt":6,"session":"local:71f7a720-41fd-4208-b8ee-a38bb3975305","branch":"issue/160-alsem-batch-build-the-model-onc-a6","worktree":"U:\\Git\\al-sem-issue-160-a6","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Base `b961710616ddd27154f3bbff7d74d02254e67f7a` (make-worktree, origin/master). Build
output: `H:\al-sem-targets\al-sem-issue-160-a6` (`target_root`).

Reviewers: the operator's standing choice (2026-10-14, used on steps 1b and 2):
gpt-6.1-sol via pi (slot astra) and Opus 5.5 via the Agent tool (slot flash). Register
marks are keyed `sol` / `opus`; attest with
`--substitute astra=gpt-6.1-sol --substitute flash=opus-5.5`.

## Classification

**Bounded.** The epic fixes the contract (row 3). The change replaces one function,
`compute_uncertainty_coverage_reasons` (`src/engine/l4/capability_cone.rs:3245-3299`),
with a scan over the combined graph's uncertainty edges. Its two callers
(`project_r3a3`, `build_r3a3_source_only_base_over`) keep their fold loop unchanged. No
new data and no output-shape change. The ledger's `## Design` is the spec.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 A base summary never carries one of the four kinds | holds | `base_intraprocedural_summary` (`src/engine/l4/summary_runner.rs:507-589`) emits only `opaque-body` or `parse-incomplete`, otherwise no uncertainty |
| P2 The solve never moves one of the four kinds from a callee to a caller, nor between SCC members | holds | `solve_side_facts` (`src/engine/l4/db_effect_solver.rs:1004-1143`): every base, inherited and uncertainty-edge entry folds into `shared` only behind `!is_callsite_local_kind` (`:1019`, `:1044`, `:1097`); the one unguarded fold is the synthesized `opaque-callee` (`:1067-1078`), not one of the four. A member's own entries are its base, its own opaque-callee entries and `uncertainty_edges_by_from[m]` (`:1016`, `:1078`, `:1086-1102`); final = `dedupe(shared ++ own(m))` (`:1130-1141`). The multi-effective-SCC path (`:1588-1627`) feeds siblings through the same filter (it is reachable even with no leaves: a `to` with no workspace row splits a Tarjan SCC, `summary_runner.rs:1439-1440`, `scc.rs:93-105`). Existing solver tests already pin non-propagation of `member-not-found` (`:3015-3305`) |
| P3 `dedupe_uncertainties` never drops a kind | holds | its key is `kind\|at` (`db_effect_solver.rs:883-886`); the fold keeps a set of KINDS, so dedup cannot change it |
| P4 Every workspace routine is a solved member | holds | `graph.nodes` = every `ws.routines` id (`src/engine/l4/combined_graph.rs:402`); `compute_summaries_v2` passes no leaves, so `is_recomputed` holds for every node (`summary_runner.rs:1439-1440`); `run_one_scc_roles` returns every non-leaf member with a base summary (`:1973-2017`, `:2165+`), and every routine has one (`:1382-1391`); side facts and roles cover the same set, so `unwrap_or_default` (`:1581`) hides no member. Duplicate routine ids collapse in the solver's maps and in Tarjan, and both the old and the new side key by id, so both take the union of that id's edges |
| P5 The roles cap changes no coverage reason | holds | a cap appends `fixpoint-capped` (`summary_runner.rs:1587-1596`), not one of the four, and widens parameter roles (`:2181-2203`), which the fold discards; its only other effect is the `render_warnings` stderr line at `capability_cone.rs:3270-3272`. Made executable by A3.3 |
| P6 Readers of the fold | listed | `project_r3a3` (`capability_cone.rs:4004`: r3a3 goldens, `aldump --r3a3-cone-coverage`); `build_r3a3_source_only_base_over` (`:4206`) via `build_r3a3_source_only_base`: the snapshot (fingerprint, prove, diff, digest's snapshot, r4f), d47's ordering snapshot (`digest.rs:4017-4018`, from `analyze --detector d47-…`), the lib test `model_calls_reach_detector_context_and_ordering_base` (`detector_context.rs:1966-2010`); and directly `aldump --issue160-full-graph-timings` (`capability_cone.rs:4310-4312`), which passes the cross-app base's full graph: the equality still holds there (P2 does not depend on the graph; that graph's nodes and `from`s are still `ws.routines` ids, `:3512-3528`), and the mode prints timings and counts only; no oracle input exercises this graph, which is accepted because P2 is graph-independent (final panel opus 6) |
| P7 An uncertainty edge whose `from` is not a workspace routine | harmless | the builder does not filter `from` (`combined_graph.rs:275-373`); the old map has no entry for such an id because there is no workspace routine or base to assemble for it (Tarjan itself can visit it, `scc.rs:93-105`); the new map may carry it, but both readers look up `ws.routines` ids only (`capability_cone.rs:4013-4047`, `:4212-4242`). The oracle counts such keys (A3.4) |
| P8 Every (kind × SCC shape) cell occurs in the oracle's inputs | required, not assumed | A3.5: each of the 12 cells must be > 0 over the union of the oracle's inputs; an inline fixture supplies any cell the corpora lack |

Conclusion: for every workspace routine `r`, the old fold's reasons equal
`{ ue.kind | ue ∈ graph.uncertainty_edges, ue.from == r.id, is_callsite_local_kind(ue.kind) }`.
No other emitter of the four kinds exists in `src/` (only `combined_graph.rs:304,327,343,348,353`
plus test code), which answers epic §13 Q1.

## Design

**Change.**
1. `db_effect_solver::is_callsite_local_kind` becomes `pub(crate)`. Its doc says the list now
   also defines the coverage-reason kinds: until now there were two equal lists (the
   solver's filter and the fold's `match`); this step couples them on purpose (epic §6).
2. New `fn callsite_local_coverage_reasons(graph: &CombinedGraph) -> HashMap<String, BTreeSet<String>>`
   in `capability_cone.rs`: one pass over `graph.uncertainty_edges`, inserting
   `ue.uncertainty.kind` under `ue.from` when `is_callsite_local_kind` holds. The reason
   text equals the kind text (the old `match` maps each kind to itself).
3. Both callers call the scan instead of the fold. `compute_uncertainty_coverage_reasons`
   stays, body byte-identical, under `#[cfg(test)]` as the oracle's old side; only its
   attribute and doc change.
4. Gone per snapshot (and per `project_r3a3`): one `compute_summaries_v2`
   (`build.summaries_v2`), one `graph_scc` (`build.graph_scc`), one `field_index`, that
   solve's summaries-census contributions (`ALSEM_SUMMARIES_CENSUS=1` totals) and its
   `compute_summaries_v2_phase_split` trace instant. No test reads the census or the
   instant. `bcg`, `cone`, `pd`, `ctx`, `snap` do not move: the graph is the caller's, and
   the cone's own Tarjan stays.
5. The span `r3a3.direct_facts` no longer contains a solve, so per the "a changed span gets
   a new name" rule (`capability_cone.rs:4173-4174`) it becomes `r3a3.direct_facts_scan`;
   `r3a3_spans_open_once_per_base` (`tests/cli/cli_build_counters.rs:877-905`) follows.
6. **stderr move, stated exactly.** The snapshot's private solve printed a roles-cap
   non-convergence line for its own graph. After this step:
   - commands with no L4 solve lose the line entirely: `diff ws ws` without `--strict`,
     `analyze --detector d47-…` (no `l4` fill), `aldump --r3a3-cone-coverage` and the aldump snapshot modes;
   - commands with an L4 solve keep only L4's own line (`build_l4`, `analysis.rs:118` — now
     truly the one print site), which reports L4's graph; in cross-app commands that is the
     base's graph, not the snapshot's cut graph.
   Why that is right: the dropped line reported the convergence of roles nothing read; the
   fold read only the four selected uncertainty kinds; a cap appends `fixpoint-capped` and
   leaves that selected kind set unchanged (P5).
7. Comments brought up to date: `db_effect_solver.rs:1091-1094` (uncertainty edges also carry
   `dynamic-dispatch` and `unresolved-call`, `combined_graph.rs:335-339,357-361`);
   `capability_cone.rs:3232-3244` (the fold's doc: test-only oracle), `:3996-4003` and `:4183`
   (the callers: a scan, no solve); `analysis.rs:139-140` (`graph_scc` no longer behind the
   snapshot fold); `tests/cli/cli_build_counters.rs:178-180` and `:331-359` (A3.2, A3.9).
8. The child spec's §4.4.8 counter table gains an "after step 3" restatement (epic I1:
   every step that moves the table restates it).

**Not changed.** The fold policy (step 5), the cut graph (step 6), which routines the fold
applies to, the coverage readers, any serialization code.

## Acceptance matrix

| # | Claim (epic row 3) | Proof |
|---|---|---|
| A3.1 | r3a3, cli-b, r4f unchanged on stdout | `scripts/ci-steps task` green with no regenerated file. cli-b and r4f compare bytes (`tests/cli/cli_b_snapshot_differential.rs:126-140`, `tests/r4/r4f_snapshot.rs:77-106`); r3a3 compares `serde_json::Value` (`tests/r3/r3a3_differential.rs:427-435,494`) — paired with: the diff touches no projection or serialization code, and A3.4 proves the one changed input (the reason map) equal |
| A3.2 | −1 `build.summaries_v2`, −1 `build.graph_scc` per snapshot | `tests/cli/cli_build_counters.rs` rows (process totals): `analyze_cross_app_d47` v2 1→0, scc 3→2; `digest_cross_app`, `digest_single_app`, `prove`, `FINGERPRINT_ROW` (json, query, inventory-only) v2 2→1, scc 4→3; `diff_ws_ws` (two snapshots) v2 2→0, scc 4→2; `diff_ws_ws_strict` v2 4→2, scc 8→6. No other column or row moves. Child spec §4.4.8 restated; header comment updated |
| A3.3 | stderr: the non-convergence line moves as Design 6 states | (i) a capped-cycle lib fixture in `capability_cone.rs`'s test module (a copy of `summary_runner`'s private `CYC_SRC` shape, `summary_runner.rs:2397-2412`, whose cycle member `HandleCyc` also makes an `external-target` call — the `tests/r0-corpus/ws-r2b-external/src/caller.al` shape — and a `member-not-found` call; neither adds a resolved edge, so the SCC still caps at 1). Under `with_roles_iteration_cap(1, …)` (`summary_runner.rs:56-77`): the precondition is asserted by calling `compute_summaries_v2` on the same graph and SCC (its diagnostics are non-empty and a cycle member carries `fixpoint-capped`); then old == new with both reason sets non-empty; (ii) a child-process test on the pattern of `build_l4_prints_the_capped_line_once` (`summary_runner.rs:2577-2612`): `build_r3a3_source_only_base` on the copied, uncertainty-bearing fixture of (i) (`CAP_SRC`) under cap 1 prints zero `warning: summarize:` lines. Discrimination: restore the `render_warnings` loop on the scan path → one line. CHANGELOG states Design 6 |
| A3.4 | oracle old == new | lib tests (in `capability_cone.rs`'s test module, so they can call the `#[cfg(test)]` fold; `cdo-gate` runs `--lib`). Domain: for every distinct `ws.routines` id, `old.get(id).unwrap_or(∅) == new.get(id).unwrap_or(∅)`; separately count keys of `new` that are not `ws.routines` ids (recorded; P7). Inputs: every `tests/r0-corpus` dir with an `app.json` at any depth (217; 207 before final-panel F1-3); the 32 `app.json` dirs under `tests/fixtures`, `tests/r3a4-fixtures`, `tests/r3a5-fixtures`, `tests/r2-5b-fixtures`; the inline fixtures of A3.5 and A3.3; and CDO (`tests/common/cdo.rs`'s `cdo_ws_or_enforce`, included once at the crate root as `crate::cdo` since F1-6). Each input set asserts its exact workspace count (F1-3) and 0 extra keys (F1-4). Since F1-2 every input also runs on its CROSS-APP model (`assemble_and_resolve_cross_app_program`): CDO cross-app 31656 routines, 0 extra keys |
| A3.4-dev | **Stated deviation: "gap"** | `tests/gap` has no fixture directories; its scenarios are inline AL strings inside the integration crate, which a `--lib` test cannot reach, and an integration test cannot call the `#[cfg(test)]` fold. The oracle substitutes the 32 fixture dirs above plus the hand-stated inline fixtures (A3.5) for the gap half |
| A3.5 | the oracle covers all four kinds in every SCC shape | columns: non-recursive singleton, self-loop singleton (`scc.recursive` with one member, `scc.rs:132-145`), multi-member SCC. All 12 (kind × column) cells must be > 0 over the union of the oracle's inputs (asserted). A hand-stated inline lib fixture (built like `CYC_SRC`) carries a call of each of the four kinds inside a two-member cycle, inside a self-loop, and in a non-recursive routine, so no cell depends on corpus luck. CDO's own counts are printed and recorded in Gate results via one `--nocapture` run |
| A3.6 | discrimination 1: the OLD kind list | remove the `external-target` arm from the `#[cfg(test)]` fold's `match` → the oracle fails (the test asserts its `external-target` count is > 0 on the non-CDO inputs, so the break bites in the task gate). The shared `is_callsite_local_kind` is never the break site (it also drives the solver's propagation filter) |
| A3.7 | discrimination 2: the new scan against hand-stated records | unit test with a hand-built `CombinedGraph`: routine A carries `interface-open-world`, `dynamic-dispatch`, `ambiguous-overload`, `member-not-found` twice (two call sites), `external-target`, `unresolved-call`; routine B carries one `external-target`; an orphan `from` carries one `ambiguous-overload`. Expected map written literally (not through the classifier): A → the four, B → `{external-target}`, orphan → `{ambiguous-overload}`. Break: add `\|\| kind == "unresolved-call"` to the scan's own filter → fails |
| A3.8 | CDO ratchets and histogram unchanged | `scripts/cdo-gate` green |
| A3.9 | `run_digest_empty_diff_builds_no_context` keeps its guard | add `count(&m, "build.snapshot") == 0` and fix its doc (it named the fold solve as the snapshot's witness). Discrimination: compose the snapshot before the no-input check → fails |
| A3.10 | oracle cost recorded | the r0 + fixtures oracle's debug wall time measured once, recorded in Gate results; one model build per fixture serves both the comparison and the cell counts |

## Base/Head SHAs

| round | B | H |
|---|---|---|
| final gates (step 11) | `b961710616ddd27154f3bbff7d74d02254e67f7a` | `511c6e582698b6e5e3ffbf68e8cb2a91ca0e7c21` |

## Timeline

- 2026-10-20 01:25 preflight ok; claim attempt 6; make-worktree base `b9617106`.
- Classification bounded; probes P1-P7 hold by source.
- Spec round 1 (charged, 4 left): sol 8 findings, opus 14; merged into S1-1..S1-16 (8 important, 8 minor); all fixed in this revision.
- Spec round 2 (3 left): both accepted S1-1..16; new S2-1..S2-5 (1 important, 4 minor), all fixed; design hash `b26a93a8acc688be`.
- Spec round 3 (2 left): both accepted all 21 against `b26a93a8acc688be`, no new findings. **Converged.** Opus's non-blocking note (A3.3 (ii) must use the copied fixture from (i), not the private `CYC_SRC`) is carried into the plan.
- Plan `docs/superpowers/plans/2026-10-20-issue-160-step3-fold-from-uncertainty-edges.md`: 2 tasks (plan_tasks charged 2, 10 left). Step 6 (acceptance tests) is folded into Task 1's red phase: the same implementer writes every A3.* test first against a stub scan and records the red output here, then makes it green; nothing red is committed alone. halt-check: not halted.
- Task 1 deviations (implementer, approved by the conductor): (a) the A3.2 counter rows move into Task 1's commit — the 8 `cli_build_counters` rows fail with exactly the predicted v2/scc moves once the scan lands, so they cannot wait for Task 2; Task 2 keeps the header comment, A3.9, the spec table and the CHANGELOG. (b) A second `#[path]` include of `tests/common/cdo.rs` trips clippy ("file is loaded as a module multiple times"), so `src/program/dep_cache.rs`'s `#[cfg(test)] mod cdo` becomes `pub(crate)` and the oracle reuses it, instead of "included as `dep_cache.rs:300` does".
- Task 1 `3b85d7d3`, Task 2 `64c82f9f` (implementer opus; per-task gate green, 3563 passed). Independent review (sonnet, `task-review-r1.md`): 1 important (`project_r3a3` call site unpinned), 6 minor; fixes 1, 2, 4-7 sent back to the same implementer; #3 declined (optional; A3.2 and #1 pin the call sites).
- Step 9 task gate on `64c82f9f`: exit 0, supervised, 113.9 s, log `.agent/runs/20261020-012541-985513/logs/ci-steps-task/20261020T023112-c7f15eff.log`. Branch pushed; draft PR #178 opened so CI runs during the final panel.
- Review fixes `cb6bb380` (implementer hit a network error after committing; commit and proof rows verified by the conductor). Task gate on `cb6bb380`: exit 0, supervised, 109.7 s, log `logs/ci-steps-task/20261020T024604-5c6e4a14.log`; pushed.
- Final panel round 1 (final_rounds charged, 4 left): built-in `code-review` high over the branch, sol (pi) and opus (Agent) in parallel. 16 merged findings F1-1..F1-16 (2 important: the snapshot base's use of the scan unpinned; the oracle never ran the cross-app model). Fixes `511c6e58` (implementer); ledger fixes by the conductor; task gate green.
- Final panel round 2 (3 left): opus accepted all 37 and raised F2-1 (ledger stale); sol accepted 36 and re-raised F1-9 (CDO claim without a recorded gate run). `cdo-gate` run on `511c6e58` (PASS) and recorded; ledger updated.
- Final panel round 3 (2 left): both accepted F1-9 and F2-1, no new findings. **Converged** (38 entries, every one accepted by both).
- Step 11: `git fetch`; `origin/master` = base `b9617106`, so no rebase and no range-diff (the branch's commits are unchanged). B = `b961710616ddd27154f3bbff7d74d02254e67f7a`, H = `511c6e58` (full SHA in Base/Head below). No CDO A/B bench: epic row 3 sets no speed or memory bound, and the counters (A3.2) are the measurement.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| `callsite_local_scan_matches_the_fold_on_fixtures` (A3.6) | delete the `"external-target"` arm from the `#[cfg(test)]` fold's `match` (break checked: arm count 0) | FAILED, 35 mismatches, e.g. `ws-baseapp-closure r0/1a3a6448…: old {}, new {"external-target"}` | ok (5/5 module tests after revert) | 3b85d7d3 |
| `callsite_local_scan_hand_stated` (A3.7) | scan filter `is_callsite_local_kind(kind) \|\| kind == "unresolved-call"` (break checked: count 1) | FAILED, `left` A carries `"unresolved-call"`, `right` A has the four kinds only | ok | 3b85d7d3 |
| `snapshot_base_prints_no_capped_line` (A3.3 ii) | call the old fold (solve + `render_warnings` loop) on the scan path in `build_r3a3_source_only_base_over` (break checked: marker count 1) | FAILED, `left: ["warning: summarize: Summary fixed-point did not converge for SCC [...50102#2e36…, ...50102#d6aa…]"]`, `right: []` | ok | 3b85d7d3 |
| `callsite_local_scan_matches_the_fold_under_a_roles_cap` (A3.3 i, cap precondition) | precondition solve under `with_roles_iteration_cap(1000, …)` instead of 1 (break checked: count 1) | FAILED, `precondition: the cap fires` | ok | 3b85d7d3 |
| `callsite_local_scan_matches_the_fold_on_fixtures` (A3.5 cell assertion) | oracle runs `CAP_SRC` instead of `CELLS_SRC` (drops the inline cells fixture; break checked: count 1) | FAILED, `cell (ambiguous-overload, self-loop) is empty` | ok | 3b85d7d3 |
| `cli_build_counters::run_digest_empty_diff_builds_no_context` (A3.9) | `compose_snapshot(target.resolved())` inserted before digest's no-input check in `digest_cli.rs` (break checked: marker count 1) | FAILED, `the no-input check must run before compose_snapshot`; counters show `build.snapshot: 1` and no `build.summaries_v2`, so the old assertion alone would have passed | ok (41/41 `cli_build_counters`) | 64c82f9f |
| `cli_build_counters::r3a3_spans_open_once_per_base` (span rename) | source span back to `"r3a3.direct_facts"` (break checked: count 1) | FAILED, `no snapshot base built` | ok (41/41 `cli_build_counters`) | 3b85d7d3 |
| `cli_build_counters::aldump_r3a3_runs_no_solve` (review r1 #1: pins `project_r3a3`'s call site) | drop the oracle's `#[cfg(test)]` and call it in `project_r3a3` instead of the scan (break checked: marker count 1) | FAILED, `build.summaries_v2` left 1, right 0; counters `{"build.graph_scc": 2, "build.summaries_v2": 1, …}` | ok (42/42 `cli_build_counters`) | cb6bb380 |
| `callsite_local_scan_hand_stated` + `callsite_local_scan_matches_the_fold_on_fixtures` (review r1 #7: the shared list) | delete `"external-target"` from `db_effect_solver::is_callsite_local_kind` (break checked: count 1; a first attempt whose Edit did not apply ran green and was discarded) | both FAILED: A3.7 `left` lacks `external-target` and `B`; oracle 36 mismatches, e.g. `ws-baseapp-closure r0/1a3a6448…: old {"external-target"}, new {}` | ok (5/5 module tests) | cb6bb380 |
| `snapshot_base_folds_the_scan_into_direct_coverage` (F1-1: the base USES the scan) | in `build_r3a3_source_only_base_over`, call the scan but use an empty map (`let uncertainty_reasons: HashMap<&str, BTreeSet<&str>> = HashMap::new();`; helper unchanged; break checked: marker count 1) | FAILED, `Plain: direct status complete`, `Plain: no reason ambiguous-overload in []` (same for every kind, and for `Loop` and `Ping`) | ok (6/6 module tests) | 511c6e58 |
| r3a3 goldens, for `project_r3a3` (F1-1 check: does an existing test catch the same break there?) | the same empty-map break in `project_r3a3` (break checked: marker count 1) | `--test r3`: `r3a3_differential::differential_r3a3_cone_coverage_match_goldens` FAILED (`coverages_with_non_trivial_inherited_status` 11 vs golden 23). The goldens already pin this call site, so no new test was added | reverted; covered by the final gate | 511c6e58 |
| `callsite_local_scan_matches_the_fold_on_fixtures` (F1-3: per-root counts, full-depth r0 walk) | r0 walked at depth 1 again (break checked: marker count 1) | FAILED, `r0-corpus: app dirs`, left 207, right 217 | ok (6/6) | 511c6e58 |
| `callsite_local_scan_matches_the_fold_on_fixtures` (F1-4: no extra key) | the scan also inserts each kind under `"not-a-routine"` (break checked: marker count 1) | FAILED, 52 lines like `ws-bare-implicit-rec: new key not-a-routine is no workspace routine`, with 0 old/new mismatches, so only the new check catches it | ok (6/6) | 511c6e58 |
| `callsite_local_scan_matches_the_fold_on_fixtures` (F1-2: the cross-app half runs on every dir) | `cross_app_model` returns `None` (`.filter(\|_\| false)`; break checked: marker count 1) | FAILED, `r0-corpus: cross-app runs`, left 0, right 217. Limit: this proves the cross-app half is never skipped; its comparison is the shared `check_oracle`, which the A3.6 and #7 rows prove | ok (6/6) | 511c6e58 |

## Findings register summary

38 entries in `findings.json`: spec panel S1-1..S1-16 (8 important) and S2-1..S2-5 (1 important);
final panel F1-1..F1-16 (2 important: F1-1, F1-2) and F2-1. Every entry `fixed`, except F1-15
`refuted` (an accepted, stated limit: the full-graph timings caller has no oracle input; P2 is
graph-independent). Every entry accepted by both reviewers (sol = gpt-6.1-sol, slot astra;
opus = Opus 5.5, slot flash). The built-in `code-review` high and an independent sonnet task
review (`task-review-r1.md`, 1 important + 6 minor, fixed in `cb6bb380`) fed the register.

## Gate results

**Task 1 red (stub `callsite_local_coverage_reasons` returning an empty map, debug, 2026-10-20):**
`cargo test -p al-sem --lib callsite_local_scan_tests` → `1 passed; 4 failed` (the CDO test skipped
without `CDO_WS`):
- `callsite_local_scan_hand_stated`: `left: {}`, right the literal map.
- `..._under_a_roles_cap`: `old {"r0/4a91bf54…": {"external-target", "member-not-found"}}, new {}`.
- `snapshot_base_prints_no_capped_line`: `left: ["warning: summarize: Summary fixed-point did not converge for SCC [...]"]`, `right: []`.
- `..._on_fixtures`: 62 `old {…}, new {}` mismatches over r0-corpus, fixtures and `CELLS_SRC`.

**A3.10 oracle cost (debug, warm build):** r0 + fixtures + inline oracle: 240 workspaces (207 r0,
32 fixture dirs, 1 inline), 1082 routines, 0 extra new keys, wall time 1.3-1.7 s across runs.
Cells over the union (non-recursive / self-loop / multi-member): ambiguous-overload 8/1/1,
external-target 33/1/1, interface-open-world 14/1/1, member-not-found 7/1/1. Without `CELLS_SRC`
the corpora have 0 in every recursive cell for ambiguous-overload, and only the inline fixture
fills the recursive cells of the other kinds.

**CDO oracle (`--profile cdo`, `CDO_WS` = the pinned CDO baseline, identity `1e512deb11c28d51`, one run):** ok; 5479
routines, 0 extra new keys, 3.9 s (build 1 m 46 s).

| kind | non-recursive | self-loop | multi-member |
|---|--:|--:|--:|
| ambiguous-overload | 18 | 0 | 0 |
| external-target | 393 | 0 | 0 |
| interface-open-world | 15 | 0 | 0 |
| member-not-found | 0 | 0 | 0 |

**After the final-panel r1 fixes (`511c6e58`):** the fixture oracle covers 217 r0 dirs (full
depth), 29 + 1 + 1 + 1 fixture dirs and `CELLS_SRC`, in both modes: single-app 1090 routines,
cross-app 1134, 0 extra keys. CDO single-app unchanged (5479 routines, every recursive cell 0).
CDO cross-app: 31656 routines, 0 extra keys, 53.5 s:

| kind | non-recursive | self-loop | multi-member |
|---|--:|--:|--:|
| ambiguous-overload | 119 | 1 | 5 |
| external-target | 19 | 0 | 0 |
| interface-open-world | 149 | 0 | 19 |
| member-not-found | 0 | 0 | 0 |

Task gate on `511c6e58`: exit 0, supervised, 108.2 s, `logs/ci-steps-task/20261020T030724-f2ff02af.log`.
A3.10 re-measured on `511c6e58` (debug, warm, one run): the fixture oracle (both modes, 1 + 217 +
32 workspaces each) finishes in 3.23 s (`test result: ok. 1 passed`).

**`cdo-gate` on `511c6e58`** (supervised, `CDO_WS` = the pinned CDO baseline, identity `1e512deb11c28d51`): exit 0, 210.7 s,
`logs/cdo-gate/20261020T031113-8fcf7449.log`: `program_resolve_harness` 201 passed; `lsp
program_graph::/snapshot_robustness::` 2 passed; `l4_summary_differential` 20 passed; `--lib`
2161 passed; `cdo-gate: PASS`. (A3.8, F1-9; repeated on the attested head in step 11.)

**Final gate set on H `511c6e582698b6e5e3ffbf68e8cb2a91ca0e7c21` (step 11; every run supervised):**

| gate | exit | seconds | log (under `.agent/runs/20261020-012541-985513/logs/`) |
|---|--:|--:|---|
| ci-steps-all (1st) | 1 | 435.4 | `ci-steps-all/20261020T031751-6ffc0716.log` — suite 3565 passed; failed only at perf-bounds `compute_all_within_bound` scaling ratio 7.060 (one fast 500-file sample; see Discoveries) |
| ci-steps-all (re-run) | 0 | 55.0 | `ci-steps-all/20261020T032814-67c89f8e.log` — 3565 passed, 8 skipped; perf-bounds green |
| check-goldens-coverage | 0 | 0.3 | `check-goldens-coverage/20261020T032507-87f03c3d.log` |
| check-goldens | 0 | 0.4 | `check-goldens/20261020T032508-a4cd28d9.log` — skipped on the tree stamp: this exact tree passed `ci-steps test` (inside the first ci-steps-all), which runs every golden target |
| cdo-gate | 0 | 134.8 | `cdo-gate/20261020T032509-72451a1f.log` (`CDO_WS` = the pinned CDO baseline, identity `1e512deb11c28d51`) |

`git status --porcelain -- . ':!.agent'` empty after the gates. GitHub CI on PR #178 at H: `test`
pass (8m30s). Toolchain rustc 1.96.0; grammar tree-sitter-al `8682cb5f`; CDO baseline worktree
pinned at `bc3ccb18`; `CDO_WS` identity `1e512deb11c28d51` = sha256 of `<path>@bc3ccb18`, first 16 hex.

**Task 1 gate:** `scripts/ci-steps task-affected` green (3563 passed, 8 skipped); pre-commit
goldens OK (906 passed); `agentflow check-diff` → `reasons: []`. No golden file changed.

## Discoveries

Candidate (to file): `tests/perf_bounds.rs:899` `compute_all_within_bound`'s scaling check compares
the MIN of 9 samples at 500 and 2000 files; one fast 500-file outlier (3.12 ms against 5.3-7.5 ms for
the other eight) pushed the ratio to 7.060 ≥ 7 and failed `ci-steps all` on H, in code this diff
does not reach (`src/lsp/**`, `src/analysis.rs` untouched). A ratio of mins is not robust to one
outlier on the small side; a median (or min-of-small vs median-of-large) would be.

```json
{"kind":"bug","title":"perf_bounds compute_all scaling ratio flakes on one fast 500-file sample","body":"tests/perf_bounds.rs:899 compares min(t500) to min(t2000). On #160 step 3's final gate (2026-10-20, H 511c6e58) samples500=[3.1155,5.2816,5.3712,7.2963,7.4237,7.1745,6.3899,6.3035,7.4585] ms and samples2000 min 21.9955 ms gave ratio 7.060 >= 7 and failed ci-steps all, though the diff touches no src/lsp code. One outlier on the small side decides the ratio. Use medians (or another outlier-robust statistic) for the scaling check."}
```

Otherwise none. (Epic §13 Q1 is answered in-place: no emitter of the four kinds exists outside
`combined_graph::build_combined_graph`.)

## Not delivered

- The epic row 3 "gap" half of the oracle: substituted by the 32 fixture dirs plus the inline
  fixtures (A3.4-dev), since `tests/gap`'s inline scenarios cannot reach a test-only lib oracle.
  Stated in the CHANGELOG and in the epic's row 3 note.
- No break makes old and new diverge only on a cross-app model, so the cross-app oracle's
  comparison rests on the shared `check_oracle` (proof row F1-2, stated limit).
- #160 itself stays open: this PR is step 3 of the epic ("Part of #160", no closing keyword).
