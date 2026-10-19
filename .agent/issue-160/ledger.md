# Issue #160 ledger: attempt 4 (epic step 1b: retained ResolvedProgram)

Earlier attempts' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170). This file replaces them for attempt 4.

## Claim

```json
{"run_id":"20261019-085626-fcb498","issue":160,"attempt":4,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/160-alsem-batch-build-the-model-onc-a4","worktree":"U:\\Git\\al-sem-issue-160-a4","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Base `576dd29685e426ad2780e5c3700f4bd66ec5212b` (make-worktree, origin/master). Build output: `H:\al-sem-targets\al-sem-issue-160-a4` (`target_root`).

## Classification

**Architectural.** This is step 1b of the #160 epic. The owner re-scoped #160 on 2026-10-18: each step is its own PR, the PR says "Part of #160", it uses no closing keyword, and #160 stays open.

## Assumption probes

Steps 2-5 are carried. The specs were panel-converged in attempt 2 and are on master: `docs/superpowers/specs/2026-10-18-issue-160-*.md`; step 1b is §4.3, acceptance A1b.*. This run delivers **plan tasks 16-18** of `docs/superpowers/plans/2026-10-18-issue-160-steps-0-2.md`. The bounds' baselines are the step 0 and step 1 numbers files.

## Base/Head SHAs

B = `576dd29685e426ad2780e5c3700f4bd66ec5212b` (`origin/master`, unchanged since the claim). H = `778972cef6ef192499d4af39485d6055dd9f1d15` (gated; later commits change only the ledger and the register).

## Timeline

- Claim attempt 4 + make-worktree at `576dd296`; preflight ok (one other live lease: #107). Heartbeat loop every 8 min. `plan_tasks` charged 3 (tasks 16-18).
- Discovery #171 filed (executor gitops decodes git output as cp1252 on Windows; seen in attempt 3).
- Code review (high, the `code-review` built-in skill, two forked reviews merged): 14 findings CR1-CR14 in `.agent/runs/20261019-085626-fcb498/code-review-r1.md`.
- An opus fix agent committed `b8b59b5a` (CR1-CR9, CR11-CR14). The session hit the account spend limit at 10:59 before the agent wrote its dispositions file; CR10 was mid-measurement (bench r1, 8 pairs).
- Resumed 2026-10-19 by session `7284feb0` (same run id; the lease was kept live by the old session's heartbeat loops, still running as orphans). Every fix in `b8b59b5a` source-checked against its finding; the four discrimination proofs re-run by this session (below); register entries `F1-CR*` written with `hash` = the fix commit's first 16 hex digits (`b8b59b5a086b94ae`).
- `cargo build --profile release-fast` of `b8b59b5a`: exit 0, no warnings. CDO bench r2 (6 rounds, analyze + digest, `576dd296` vs `b8b59b5a`) for CR10 and A1b.4: every run exit 0; A1b.4 met on all four peaks (−4.5 to +2.5 MB); digest wall +0.1 s (3 of 6), analyze +0.8 s (2 of 6); SARIF, digest bytes (less `generatedAt`) and the CDO histogram SHA unchanged. CR10 refuted with r1 + r2 (14 more pairs); the step 1b numbers file records both.
- No `cfg(unix)` code in `576dd296..b8b59b5a`'s fixes, so the WSL Linux check (passed after tasks 16-18) was not re-run. `origin/master` still `576dd296`: no rebase.

## Acceptance matrix

This run's acceptance rows are the spec's step 1b rows (A1b.*). Each one maps to a test in plan tasks 16-18.

## Discrimination proofs

Every break was made with the Edit tool and applied exactly once, then reverted to green. Each of the three tasks was reviewed by opus with no blocking defects.

**Task 16, `821cb7a4` (ResolvedProgram, two edge sets kept apart)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| program::resolved::tests::all_edges_is_edges_then_dependency_bodies | `all_edges` chains dependency bodies first | `resolved.rs:229` left [..202.., ..101..] right [..101.., ..202..] | green |
| same | `edge_view` swaps the two slice fields | `resolved.rs:233` (the view assert) | green |
| cli_build_counters::retained_census_changes_no_output (A1b.5 inputs bucket) | `InputSet::census` drops the `.al` text bytes | first run stayed GREEN (the fixture's `.al` bytes were below the other inputs). The test was strengthened by padding one `.al` file with a 64 KiB comment. Then: `inputs 2446 < .al bytes 66005` | green 32/32 |

**Task 17, `1d4b6bf7` (`alsem graph` reads the ResolvedProgram; one EdgeView)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| project::add_keeps_first_via_on_conf_tie | `<` to `<=` in `Builder::add` | left "second" right "first" | green 14/14 |
| project::cross_set_tie_keeps_the_first_sets_via | the engine edges iterate dependency bodies first | left "dynamic" right "unresolved" | green |
| cli_graph_export::dependency_bodies_histogram_is_the_dependency_call_sites | drop the `from.object.app != primary_app_ref` filter | resolvedSource 3/total 3 against 2/2 | green 8/8 |
| cli_graph_export::graph_export_covers_dependencies | the projection is fed `dependency_body_edges: &[]` | `exactly one node Dep Worker.Work: []` | green |
| same | delete only `report.site_facts.extend(dependency.site_facts)` | first run GREEN (the fixture had no dependency run site; the plan predicted this limit). A dependency `Page.Run` and a run-edge assertion were added; then `exactly one node Dep Worker.ShowPage: []` | green 8/8 |

**Task 18, `275a97a6`:** a docs-only measurement task, with no new test.

**Review fixes, `b8b59b5a`** (each break made with Edit, its absence checked by grep count 0, run with `cargo nextest`, then reverted; logs under `H:/al-sem-160-bench/`)

| finding | test | break | fail output (1 line) | revert |
|---|---|---|---|---|
| CR3 | program::resolved::tests::report_tally_counts_the_meta_rows | the `coverage_contract.missing.len()` buffer deleted from `report_tally` | left 161 right 281 | green |
| CR11 | program::resolved::tests::snapshot_texts_exclude_the_workspace + cli_build_counters::retained_census_changes_no_output | `.filter(\|u\| u.id != snap.workspace_app)` removed from `snapshot_texts_tally` | unit: left (12624, 8) right (0, 0); census: `program.snapshot_texts 66344 counts the workspace texts` | green (both) |
| CR2 | cli_graph_export::export_refuses_a_workspace_scoped_program | the scope guard made `&& false` | panic `reading dependency trees needs DependencyBodies::Keep` | green |
| CR14 | cli_graph_export::dependency_bodies_histogram_is_the_dependency_call_sites | histogram fed `body.all_edges()` | total 3 against 2 | green (reverted against the formatter hook's reflow) |

After all reverts: `git diff -- src/ tests/` empty; the five tests above plus `all_edges_is_edges_then_dependency_bodies`: 6 passed.

**Final panel r1 fixes**

| finding | test | break | fail output (1 line) | revert |
|---|---|---|---|---|
| F2-3 (r1) | doctest `program::resolved::ProgramMeta::scope` (`compile_fail,E0616`: setting `p.meta.scope` from outside the crate) | `scope` made `pub` again (grep count 1) | `Test compiled successfully, but it's marked compile_fail` | green |
| F2-3 (r2: meta/body sealed) | doctest `program::resolved::ResolvedProgram` (`compile_fail,E0616`: `std::mem::swap(&mut a.meta, &mut b.meta)`) | `meta` made `pub` (grep count 1) | that doctest FAILED `Test compiled successfully…`; the scope doctest stayed ok | green |
| F2-3 (r2) | doctest `ProgramMeta::scope`, retargeted to an owned `&mut ProgramMeta` (so it tests `scope`, not the now-private `meta`) | `scope` made `pub` (grep count 1; `pub meta` count 0) | that doctest FAILED `Test compiled successfully…`; the swap doctest stayed ok | green (both: 2 passed) |

## Findings register summary

- Code review r1 (`code-review` built-in skill, high): F1-CR1..CR14. 13 fixed (`b8b59b5a`), CR10 refuted by re-measurement (r1 + r2, 14 pairs).
- Final panel r1 (`gpt-6.1-sol`, `claude-opus-5.5` via `pi_ask`; files `.agent/runs/20261019-085626-fcb498/final-r1-{briefing,sol,opus}.md`): sol accepted 13, re-raised CR4; opus accepted 13, re-raised CR13. New: F2-1..F2-6. CR4 fixed at the definition (census.rs), CR13's evidence replaced by a direct CDO oracle comparison.
- Final panel r2 (`final-r2-{briefing,sol,opus}.md`, same threads): opus accepted all 8; sol accepted 7 and re-raised F2-3 (the public `meta`/`body` still let one build's meta be swapped onto another's body; opus raised the same as a nit). Fixed by sealing both fields (`meta()`, `body()`, crate-only `into_parts()`), pinned by a second doctest. Opus's two other minors: F2-7 (the numbers file's branch gate), F2-8 (two more API changes for the CHANGELOG).
- Final panel r3 (`final-r3-*.md`, `778972ce`): both accepted F2-3 and F2-8, no new findings; opus accepted F2-7, sol re-raised it (the frozen-head gates not yet recorded). The four gates then ran on H = `778972ce`, all exit 0, recorded above. r4 asks both for F2-7 only.
- Final panel r4 (`final-r4-*.md`): both accepted F2-7, no new findings. Converged: all 22 entries accepted by both. `final_rounds` used 4 of 5.

## Gate results

On `933e4f41` (the review-fix head plus register), all through `agentflow run`, logs under `.agent/runs/20261019-085626-fcb498/logs/`:

| gate | exit | seconds |
|---|--:|--:|
| `ci-steps all` | 0 | 612.4 (queued 180 behind #107); 3481 passed, 8 skipped |
| `check-goldens --verify-coverage` | 0 | 0.3 (36 golden dirs declared) |
| `check-goldens` | 0 | 27.2 |
| `cdo-gate` | 0 | 225.7 (`cdo-gate: PASS`, 2122 lib tests) |

On the frozen head `778972cef6ef192499d4af39485d6055dd9f1d15` (after the final-panel r1 and r2 fixes; `origin/master` still `576dd296`, no rebase):

| gate | exit | seconds | log |
|---|--:|--:|---|
| `ci-steps all` | 0 | 605.0 | `logs/ci-steps-all/20261019T130611-216c2047.log`: 3481 passed, 8 skipped; both `resolved.rs` `compile_fail` doctests ok |
| `check-goldens --verify-coverage` | 0 | 0.4 | `logs/check-goldens-coverage/20261019T131617-c7e65ae5.log` (36 dirs) |
| `check-goldens` | 0 | 31.1 | `logs/check-goldens/20261019T131618-94ddef56.log` |
| `cdo-gate` | 0 | 204.7 | `logs/cdo-gate/20261019T131650-4dd43e04.log` (`cdo-gate: PASS`) |

## Discoveries

- #171 (filed): executor gitops decodes git output as cp1252.

## Not delivered

Step 2 (plan tasks 19-28) is not in this run. It is its own later run of #160.
