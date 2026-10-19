# Issue #160 ledger: attempt 5 (epic step 2: DetectorArtifacts, L4Analysis, View)

Earlier attempts' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170), step 1b at `6c6bce17` (PR #172). This file replaces them for attempt 5.

## Claim

```json
{"run_id":"20261019-162640-c4cf84","issue":160,"attempt":5,"session":"claude-code-session:7284feb0-02fe-4cde-ba4d-420af3df6e9b","branch":"issue/160-alsem-batch-build-the-model-onc-a5","worktree":"U:\\Git\\al-sem-issue-160-a5","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Base `1524b1fd20abc3cac3180ea9fc6d906b70368491` (make-worktree, origin/master; v2.0.0). Build output: `H:\al-sem-targets\al-sem-issue-160-a5` (`target_root`).

## Classification

**Architectural.** This is step 2 of the #160 epic. The owner re-scoped #160 on 2026-10-18: each step is its own PR, the PR says "Part of #160", it uses no closing keyword, and #160 stays open.

## Assumption probes

Steps 2-5 are carried. The specs were panel-converged in attempt 2 and are on master: `docs/superpowers/specs/2026-10-18-issue-160-*.md`; step 2 is §4.4, acceptance A2.*. This run delivers **plan tasks 19-28** of `docs/superpowers/plans/2026-10-18-issue-160-steps-0-2.md`. The plan predates steps 1 and 1b, so its file/line references are checked against the base before task 19 (plan drift audit, below). The bounds' baseline is the step 1b numbers file (`6c6bce17`).

## Base/Head SHAs

B = `1524b1fd20abc3cac3180ea9fc6d906b70368491` (claim). H = (pending).

## Timeline

- Before the claim: the flow was halted by a false post-merge red on #107 (`89c29a11`): its `cdo-gate` ran with no `CDO_WS` (usage error, exit 2), on the merge and on the revert. This session ran `cdo-gate` on master `1524b1fd` with `CDO_WS=U:/Git/DO-cdo-baseline/Cloud`: PASS. The operator cleared the HALT and resolved the incident; #107's `agent-regressed` / `agent-revert-blocked` labels removed.
- Removed `agent-done` from #160; preflight ok (with `CDO_WS` set); claim attempt 5; make-worktree at `1524b1fd`. Heartbeat loop every 8 min. `plan_tasks` charged 10 (tasks 19-28).
- Plan drift audit (opus subagent, read-only): 122 references in Tasks 19-28 checked against `1524b1fd`: 33 OK, 66 moved, 17 changed, 4 gone, 2 counts wrong; 13 premise problems. The four largest (P1 test types, P2 digest red on the parent, P3 `db_effect_bundle` readers, P6 warning text) were source-checked by the conductor. Written into the plan as "Step 2 amendments" A1-A11. Audit: `.agent/runs/20261019-162640-c4cf84/plan-drift.md`.
- Spec panel on the amendments, 5 rounds (`spec_rounds` 5 of 5; `.agent/runs/20261019-162640-c4cf84/spec-r{1..5}-{briefing,sol,opus}.md`; threads continued by id). r1: 14 distinct findings S1-1..S1-14 (5 important); r2: both accepted all 14, 5 new (S2-1 important: shared-graph double count); r3: both accepted S2-*, opus 2 minors (S3-*); r4: both accepted S3-*, opus 1 minor (S4-1); r5: both accepted S4-1, no new finding. Converged: 22 entries, all `fixed`, all accepted by both. Doc commits `5602de9e`, `fdab0240`, `c757e08f`, `a62bb97f`, `75fe69f4` (r5 wording note, not a finding). Spec amended in place with attempt-5 notes: A2.8, the §4.1.5 census row, §4.4.4's SCC overlap sentence, the §6.3 and §7 step-2 rows. `check-diff` empty after each commit.

## Acceptance matrix

This run's acceptance rows are the spec's step 2 rows (A2.1-A2.10). Each maps to a test or measurement in plan tasks 19-28 (the plan's self-review coverage line).

## Discrimination proofs

Every break was made with the Edit tool, its application checked by grep count, and reverted to green. Reports: `.agent/runs/20261019-162640-c4cf84/task<N>-report.md`.

**Task 19, `32ddde7e` (superset-mask parity compares the whole `DetectorOutput`)**. Red: compile failure (`E0369` / `E0277` on `DetectorOutput`). Green: `ci-steps task` exit 0, 3526 passed, 8 skipped. The widened equality was green on the unbroken code (no real full-vs-minimal difference on any parity fixture). Reviewed by the conductor: a 6-line diff of three derives and the wider assert.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| gap_detector_substrate_parity::every_detector_parity_between_full_and_minimal_ctx | d1 returns `d1_cohort_index: None` when `!ctx.transaction_spans.is_empty()` (grep count 1). Precondition checked with a temporary probe: on `ws-d8-commit-in-tx` the full context has 1 span, the minimal 0 | `detector d1-db-op-in-loop under-declares its substrate requirements (ws-d8-commit-in-tx)`. The old findings-only assert stays green under this break | green |

**Task 20, `14b01319522f41c4ac402899c640814147e8f08e` (`L4Analysis` + `build_l4` beside the old builders, TRANSIENT oracle)**. Red: compile failure (`L4Request`, `build_l4`, `render_warnings` missing). Green: `ci-steps task` exit 0, 3529 passed, 8 skipped; `--lib engine::l4::analysis` 2 passed; `forced_roles_cap` 4 passed; `--test r3` 63 passed. The old context's CORE_SUMMARIES block now calls the shared helpers (`graph_scc`, `solver_field_index`, `emit_scc_stats`); no golden moved.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| analysis::bundle_equals_the_old_builders (TRANSIENT; `cargo test -p al-sem --lib engine::l4::analysis`, pass line `bundle_equals_the_old_builders ... ok`) | (1) empty `leaf_summaries` in `build_l4` (grep 1) | stayed GREEN, as A2 predicted (the only leaf `HandleRec` has no out-edge): evidence about the fixture, recorded | reverted |
| same | (1b) drop `edges_by_from` for every `fixed_roles` member inside `build_l4` (grep 1) | `assertion left == right failed: cross-app` (`OnDepRec`'s and `Fwd`'s `Modify` rows gone) | green |
| analysis::warning_text_is_rendered_once + summary_runner::forced_roles_cap_renders_the_printed_warning | prefix `"{}: {}: {text}"` to `"{} {}: {text}"` (grep 1) | `left: ["warning summarize: Summary fixed-point did not converge for SCC [a, b]"]` (both tests) | green |
| analysis::warning_text_is_rendered_once | drop the cap filter (render every diagnostic) (grep 1) | `right: []` got the line | green |
| summary_runner::forced_roles_cap_renders_the_printed_warning | producer tail replaced by `; BREAK4` (grep 1) | `left: []` against the solver's printed line | green |

Task 20 review (opus `code-reviewer`, `task20-review.md`): 0 critical, 0 important, 6 minor. m1, m3-m6 fixed in `63bd6535` (`task20-fix-report.md`; `ci-steps task` exit 0, 3529 passed); m2 (no test pins "`l4.scc` iff computed") carried to Task 21/24.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| analysis::bundle_equals_the_old_builders (m1, the cross-app role assertion) | the solve inside `build_l4` with no leaves and no fixed roles, as if `cross` were `None` (grep 2) | `OnDepRec's role must stay Unknown (cross's fixed roles)`: left `(Yes, Known([]), Yes, No, Full)`. The rows assertion stayed green under this break | green |

**Task 21, `344369f0` (owned `DetectorArtifacts`, borrowed `DetectorContext<'v>`, ordering as a filled slot)**. 31 files. Red: the rewritten `undeclared_ordering_facts_read_inside_the_loop_fails_loudly` did not compile (no `ctx()`). Green: `ci-steps task` exit 0, 3528 passed, 8 skipped; `--lib engine::l5` 432; `--test gap` 265; `cli_build_counters::` 33 (rows unchanged). Deleted, as the plan names: `ordering_facts_are_lazy_and_parity_with_direct_compute`, `declared_ordering_facts_are_filled_before_the_parallel_loop` + its probe. The parity test's full mask is now `ALL | ORDERING_FACTS` (closes Task 19's open item). Both Task 20 `cfg_attr` bridges removed.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| registry::undeclared_ordering_facts_read_inside_the_loop_fails_loudly | `get_ordering_facts`' `expect` replaced by an empty-map fallback (grep 1) | fails (no `substrate::ORDERING_FACTS` diagnostic); `every_detector_alone_declares_what_it_reads` green in the same run | green |
| registry::every_detector_alone_declares_what_it_reads (debug) | the shim skips `fill_ordering` (grep 1) | fails naming d47, d49, d51 | green |
| cli_build_counters::retained_census_counts_the_single_app_scc_once (NEW, Task 20 review m2, single-app half) | `computed_scc` forced false in `build_l4` (grep 1) | `left: 0, right: 1` | green |
| cli_build_counters::retained_census_changes_no_output | `census_l4` skipped (grep 1) | `missing bucket=l4.` | green |

Task 21 review (opus `code-reviewer`, `task21-review.md`): 0 critical, 2 important (I-1 BLOCKING: the five borrowed maps `DetectorContext::new` builds were dropped from the census though alive for the whole run; I-2: `CORE_SUMMARIES` without an `l4` request gives a silently empty harvest), 4 minor. Fixed in `3b5411ae` (`task21-fix-report.md`; `ci-steps task` exit 0, 3529 passed): I-1, the five buckets print again under their step 0 names from `DetectorContext::new` (`census_borrowed_maps`, `FingerprintIndex::census_tally` restored; spec §4.1.5 row amended); I-2, `build_detector_artifacts` asserts `CORE_SUMMARIES` ⇒ `l4.is_some()` (release builds too); M-3/M-4 comments and names.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::retained_census_changes_no_output (I-1) | `census_borrowed_maps(&ctx);` deleted (grep 0) | `missing bucket=context.fingerprint_index` | green |
| detector_context::tests::core_summaries_without_an_l4_request_panics (I-2, NEW) | the `assert!` deleted (grep 0) | `CORE_SUMMARIES with l4: None must panic: ()` | green |

**Task 22, `d19722a11c14e242540c39c98892aeb55608ba8d` (`session.rs`: `View`, `Plan`, `RunKey`, `Cell`; TRANSIENT graph oracle)**. Green: `ci-steps task` exit 0, 3536 passed, 8 skipped; `cli_build_counters::` unchanged; no golden moved. `registry::finish_run` extracted; both runners call it. The #70 fixture writer split into `write_fixture_with(ws, dep_al)` (`write_fixture` writes the same bytes) so the transient test has a dependency-to-dependency call, i.e. injected edges, which it asserts non-empty.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| session::after_run_bits_must_be_in_the_union | the `after_run` assertion removed | `test did not panic as expected` | green |
| session::command_plans_declare_after_run | `digest_plan` `after_run: 0` | `digest: expected a panic, none came` | green |
| same | `events_plan` `after_run: 0` | `events: expected a panic, none came` | green |
| session::no_lazy_fill_in_this_file | a `OnceLock` field on `View` | `session.rs contains OnceLock` | green |
| same | a `RefCell<Option<DetectorArtifacts>>` field | `session.rs contains Cell<` | green |
| same | `released: AtomicU8` | `session.rs contains Atomic` | green |
| session::prepare_fills_ordering_before_the_runs | ordering fill moved after the runs | `left: ["empty"] right: ["filled"]` | green |
| session::a_second_prepare_never_refills_or_widens | assert (a) removed | `(a) refill after release: expected a panic, none came` | green |
| same | assert (b) removed | `(b) a wider plan: expected a panic, none came` | green |
| session::run_diagnostics_follow_the_run_mask | diagnostics chosen by `art.demanded` instead of the key | `v.run(&k[1]).summarize_diagnostics.is_empty()` failed | green |
| session::base_graph_equals_context_graph (TRANSIENT; `cargo test -p al-sem --lib engine::session`, recorded at `d19722a1`) | `cross_app_context_graph` skips the injected extend | `assertion failed: *base.graph == shim_graph` | green |

Task 22 review (opus `code-reviewer`, `task22-review.md`): 0 critical, 0 important, 9 minor, BLOCKING 0. Fixed before Task 23 (same agent, its own commit): #1 rule 1(c) matched to the spec (`req` when `core || plan.need_l4`, assert `l4` empty) plus a (c) test; #3 the weak `"Det"` match; #4 tests for the RAW assert and for every substrate bit having a name; #9 a `cell::` needle. Carried as notes: #2 `context.ctx_drop` now follows the role-scope sort (a trace-order change for Task 28's A/B); #6 `View::detector_context()` builds the borrowed maps on every call, so Tasks 24-27 read `det()` fields directly where they can; #7 the cross-app branch of `prepare` has no test until Task 24 (a precondition for Task 24: its goldens and §4.4.8 rows cover it); #5 `View.target` is `pub` as the spec says (a `&mut View` holder could swap it), a spec-level question for the final panel; #8 three identical factory bodies are intended (one named factory per command).

**Task 22 review fixes, `77577a4b`** (`task22-fix-report.md`; `ci-steps task` exit 0). Rule 1(c) now matches spec §4.4.3 (`req` when `core || plan.need_l4`; a det fill needing `l4` while it is filled panics).

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| session::a_det_fill_needing_l4_never_finds_l4_filled (NEW) | the (c) assert deleted (grep 0) | `CORE: expected a panic, none came` | green |
| session::a_det_cell_never_carries_raw_inherited_facts (NEW) | the RAW assert deleted | `test did not panic as expected` | green |
| session::every_substrate_bit_has_a_name (NEW) | the `ORDERING_FACTS` arm of `substrate_bit_name` deleted | `bit 6: an unnamed substrate bit` | green |
| session::a_second_prepare_never_refills_or_widens (a) | (a) message changed to one the old `contains("Det")` accepted | the exact `Det released` assert fails | green |
| session::no_lazy_fill_in_this_file | `use std::cell::RefCell as R;` added | `session.rs contains cell::` | green |

**Task 23, `88ea1f55` (classify a failed cross-app build from its own `Result`; no single-app rerun)**. Red: `run_build_failed_row_runs_one_build` `build.program: 2` (as the plan predicted). Green: `ci-steps task` exit 0, 3541 passed, 8 skipped; `cli_build_counters::` 35 (success rows unchanged). `CrossAppFailure`'s `fresh` is `Box<FreshCoverage>` (clippy `result_large_err` under `-D warnings`; a cold path). Captured no-id literal (Windows, from `77577a4b`): `al-sem: warning: coverage could not be verified: snapshot build failed: workspace source provider: reading source <tempdir>\A.al: The process cannot access the file because it is being used by another process. (os error 32)`; pinned exactly under `cfg(windows)`, by its prefix elsewhere. The Linux text was not captured; the WSL Linux check before the first push runs the test on Linux.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::run_build_failed_row_runs_one_build | the single-app rerun restored on `Build(_)` | `build.program left: 2 right: 1` | green |
| cli_build_counters::run_no_id_row_runs_one_build_and_keeps_the_text | a second `ResolvedProgram::build` on the no-id row | `build.program left: 2 right: 1` | green |
| same (profile) | `ProgramScope::CrossApp` on the no-id row | first run STAYED GREEN (same text, same count under both scopes); the test was strengthened to assert no `crossapp.program_build_full` span; then `the no-id row builds under the single-app profile` | green |

The `NoPrimaryApp` arm has no test (no input reaches it, spec §4.4.5).

Task 23 review (opus `code-reviewer`, read-only, `task23-review.md`): 0 critical, 0 important, 4 minor, BLOCKING 0. To apply after Task 24 (it edits `run.rs`): #1 the no-id block duplicates `build_analysis_model(inputs, true)`, so call it; #2 the `Assembly` row is untested, and spec §4.4.5 row 3's "`empty_output_result` reads `fresh` only as Err text" is wrong (it reads `files_without_object` from `Ok(f)`; output is unaffected): correct the spec sentence, and add a one-build test if an input reaches assembly `None` with the build `Ok`. Stated limit (#3): when `unreadable` cannot make a file unreadable (root, or a file system that ignores modes), both tests skip green; the GitHub runner is non-root. #4: Task 28's CHANGELOG `Changed` entry names the row-2 message change (the real error instead of "the cross-app model could not be built").

**Task 24, `ace8a199` (analyze, fingerprint and diff run through a `View`)**. Red: `analyze_cross_app_default` `build.combined_graph: got 3, want 2` (and `graph_scc` 3/2), `analyze_cross_app_d47` 4/3, the new census test `l4.scc lines: left: 1 right: 0`. Green: `ci-steps task` exit 0, 3542 passed, 8 skipped; `cli_build_counters::` 36; cli_a_/cli_b_fingerprint/cli_b_diff/cli_diff_strict 91; `--test r4` 58; no golden moved; stdout and stderr unchanged. This is the first coverage of `prepare`'s cross-app branch (Task 22 review #7). `compute_analyzer_diagnostics*` stay for digest/prove/events/policy (Tasks 25-26), the snapshot golden source and census-probe.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::analyze_cross_app_default / _d47 (bcg) | a second graph built beside `Arc::clone(&b.graph)` in the View's cross-app arm | `build.combined_graph: got 3, want 2` / `got 4, want 3` | green |
| cli_build_counters::retained_census_counts_the_cross_app_graph_and_scc_on_the_base (NEW; Task 20 review m2, cross-app half) | the cross-app arm builds and censuses its own graph | `context.graph lines: left: 1 right: 0` | green |
| same | cross-app `scc: None` | `l4.scc lines: left: 1 right: 0` | green |
| fingerprint rows | `release(Cell::Graph)` removed in fingerprint | NO failure, as the plan predicts: a peak claim with no counter; A2.4's fingerprint peak (Task 28) is its measurement | reverted |

Task 24 review (opus `code-reviewer`, read-only, `task24-review.md`): 0 critical, 1 important, 8 minor, BLOCKING 0. I1 (important): `View.runs` keeps the `Arc<RunOutput>` alive through snapshot composition in fingerprint and `diff --strict`; `release` cannot drop it, and spec §4.4.4/§6.3 list `l4` (digest/prove) as the only new overlap. Decision: option (a), `View::take_run(key)` removes the run once its diagnostics are read. Sent to the Task 25 agent mid-task: it adds `take_run` + a test and uses it in digest/prove. A follow-up commit applies it to fingerprint/diff and fixes M1 (re-open a `context.build_total` span around the det fill in `prepare_inner`, so Task 28's trace comparison matches), M2 (a source-scan test that fingerprint/diff use `analyzer_diagnostics_of_view`), M3 (assert the `base.combined_scc` line), M4 (two stale docs), M6 (diff releases only under `--strict`). Accepted: M5 (fingerprint releases before the `--strict` gate: frees memory sooner; nothing reads the cells in between; an intended deviation from §4.4.4's order), M7 (pre-existing format duplication, left as is). M8: after Task 26, deleting `compute_analyzer_diagnostics*` also needs a decision on `tests/cli/cli_b_snapshot_differential.rs:37` and `tools/census-probe`. Branch-gate precondition: cross-app analyze byte-identity on real code is checked by CDO SARIF parent vs head (Task 28 / `cdo-gate`).

**Task 23 review fixes, `bcc0a885`** (`task23-fix-report.md`; `ci-steps task` exit 0, 3543 passed). The no-id row calls `build_analysis_model(inputs, true)`; spec §4.4.5 row 3 corrected; the `Assembly` arm IS reachable with the build `Ok` (an `app.json` with an id and `A.al` = `// no object`), now pinned.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::run_assembly_row_runs_one_build_and_reads_its_coverage (NEW) | a rerun restored on the `Assembly` arm | `build.program left: 2 right: 1` | green |
| same | the `Assembly` arm passes `Err("x")` as the coverage | warning `...verified: x` instead of `...No object declaration found in A.al` | green |

**Task 25, `860fe7cc` (digest and prove reuse one View; duplicate context and bundle deleted)**. Red (inner forms added, still ignoring `detectors`): both `tx_context_without_d9` complete without panic; `digest_cross_app` `combined_graph 5/3`, `ctx 2/1` (and cone/v2/scc/pd); `digest_single_app` and `prove` `combined_graph 4/2`, `ctx 2/1`; `run_digest_empty_diff_builds_no_context` red on `build.summaries_v2` `1/0` (A8). Green: `ci-steps task` exit 0, 3547 passed, 8 skipped; no golden moved (the View's `l4.bundle` gives byte-identical `tempState` output in both universes). `View::take_run` added (Task 24 review I1) and used by the shared `envelope_and_tx_map`, so neither command keeps the run alive through composition.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::digest_cross_app / digest_single_app | the `ALL` context build re-added after `envelope_and_tx_map` | `[digest cross-app] build.detector_context: got 2, want 1` | green |
| digest_cli::tx_context_without_d9 (NEW) | a temporary `digest_plan_no_after_run` used by `digest_pipeline_with` | `prepare must refuse a plan whose union lacks TRANSACTION_SPANS: ()`; prove's test stayed green | green |
| prove::tx_context_without_d9 (NEW) | the same factory used by `prove_pipeline_with` | same message; digest's test stayed green | green |
| cli_b_prove_differential (A2.6) | prove effects with `Some(from_ref("no-such-root"))` | `ws-d34/DirectCommitInLoop/may-commit: JSON golden mismatch at byte 216` | green |
| cli_build_counters::run_digest_empty_diff_builds_no_context (NEW) | the no-input check moved after `prepare` | `the no-input check must run before prepare` `left: 1 right: 0` | green |
| same | `compose_snapshot` run before the check | `build.summaries_v2` `left: 1 right: 0` | green |
| session::a_taken_run_is_gone (NEW) | `take_run` does not remove | `a second take: expected a panic, none came` | green |

`check-diff` after `860fe7cc` REFUSED: `forbidden:allow-attribute:src/engine/l5/digest_cli.rs:#[allow(clippy::too_many_arguments)]` on the new `digest_pipeline_with` (9 parameters; the base's `run_digest_pipeline` carried the same allow already). Fix: the CLI inputs travel as one argument struct, so no new `allow` is added.

**`e4eb1ddd` (digest_pipeline_with takes one `DigestRequest`; no new allow)** and **`bff11976` (Task 24 review fixes)** (`task25-24-fix-report.md`; `ci-steps task` exit 0, 3549 passed; no golden moved). `git diff 1524b1fd -- src tests` adds no `#[allow(` / `#[expect(`; `check-diff` empty again. `max_paths_override` was already unused at the parent (no request field for it). I1: fingerprint and `diff --strict` `take_run` right after the diagnostics, before composition. M1: `prepare_inner` opens `context.build_total` (single-app) / `crossapp.context_build` (cross-app) around the det fill. M2/M3/M4/M6 as the review asked.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| digest_cli::tx_context_without_d9 (after the request refactor) | `digest_plan` fed the default set instead of `detectors` | `prepare must refuse a plan whose union lacks TRANSACTION_SPANS: ()` | green |
| run::fingerprint_and_diff_read_diagnostics_off_the_view (NEW, M2) | `take_run` deleted in `diff/cli.rs` | `diff/cli.rs must call take_run(` | green |
| cli_build_counters::retained_census_counts_the_cross_app_graph_and_scc_on_the_base (M3) | the `base.combined_scc` bucket renamed | `base.combined_scc lines: left: 0 right: 1` | green |
| cli_build_counters::analyze_det_fill_keeps_the_context_build_span (NEW, M1) | the span condition made false | ``crossapp.context_build` spans left: 0 right: 1`` | green |

Task 25 review (opus `code-reviewer`, read-only, over `860fe7cc`, `e4eb1ddd`, `bff11976`; `task25-review.md`): 0 critical, 0 important, 7 minor, BLOCKING 0. To apply after Task 26: #1 `run_digest_empty_diff_builds_no_context` also pins exit 1 and the no-input message; #2 the dated discrimination note in `cli_digest_query_dependencies.rs` names the deleted `compute_tx_context_map` (name the View seam, re-prove); #3 `envelope_and_tx_map` also releases `Ordering` and gets a name or doc that says it releases cells (the cross-app sentence qualified); #4 prove releases `l4` right after the query instead of after `unresolved_cone`; #5 the source-scan test documents its two limits (comment cut, no order check); #6 `tx_context_map` private. #7: Task 28's CHANGELOG names the deleted `digest.tx_context_map` / `prove.tx_context_map` spans and the `context.build_total` / `crossapp.context_build` spans now on digest, prove, fingerprint and `diff --strict`.

**Task 26, `c1a5101c` (events and policy check reuse one View; policy's RAW build stays command-local)**. Red (inner forms ignoring `detectors`): both `*_without_d1_d9` complete; `events_fanout_cross_app` / `events_chains_cross_app` `combined_graph 4/2`, `ctx 2/1` (and cone/v2/scc/pd); `events_single_app` `ctx 2/1`; `policy_check_cross_app` `combined_graph 4/2`, `v2 2/1`, `scc 5/3`; single-app `combined_graph 2/1`. Green: `ci-steps task` exit 0, 3551 passed, 8 skipped; cli-c-events and cli-c-policy byte-identical; no added `allow`. Remaining callers (Must 3): `compute_analyzer_diagnostics` by `tests/cli/cli_b_snapshot_differential.rs`; `compute_analyzer_diagnostics_cross_app` by `tools/census-probe`; `analyzer_diagnostic_lines` by nothing (dead).

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| events::fanout_without_d1_d9 (NEW) | a temporary `events_plan_no_after_run` from `events_fanout_with` | `prepare must refuse a plan whose union lacks SUMMARIES; the run completed`; chains stayed green | green |
| events::chains_without_d1_d9 (NEW) | the same factory from `events_chains_with` | same; fanout stayed green | green |
| cli_build_counters::policy_check_{cross,single}_app | the RAW build takes its own graph instead of `Arc::clone(view.graph())` | `build.combined_graph: got 3, want 2` / `got 2, want 1` | green |
| cli_build_counters::events_{fanout_cross_app,single_app} | one `ALL` context re-added in `events_fanout_with` | `build.detector_context: got 2, want 1` | green |

Task 26 review (opus `code-reviewer`, read-only, `task26-review.md`): 0 critical, 0 important, 8 minor, BLOCKING 0. To apply after Task 27: #1 the A2.10 message checks match `reads <bit> off det`, not the bare bit (`"SUMMARIES"` also matches `CORE_SUMMARIES`; also `session::command_plans_declare_after_run`); #2 events releases `l4` right after `take_run` (as policy does; a lower peak, a refinement of §4.4.4's order, noted in the spec row); #3 events also releases `Ordering`, and the `release_cells` doc is reworded; #4 the stale caller docs in `run.rs`; #5 a duplicated #70 comment; #7 one `--strict` case each for events and policy (a pre-existing gap: the strict early return moved, byte-identical only by construction); #8 `use` imports. #6 (`analyzer_diagnostic_lines` dead) is in Task 27.

**Task 25 review fixes, `832f95bc`** (`task25-fix-report.md`; `ci-steps task` exit 0). #1 exit 1 and the no-input text pinned; #3 `envelope_and_tx_map` renamed `finish_default_run`, also releases `Ordering`; #4 prove releases `l4` right after the query; #5 the scan test's limits documented; #6 `tx_context_map` private. #2: the planned break (`None` instead of `target.cross_app_inputs()` in `View::prepare`) stayed GREEN: on the View path the dependency-internal edges reach the commit span through `base.graph`, not through `cross`. The live seam, the cross-app graph fill (`Arc::clone(&b.graph)` replaced by a graph built without the injected edges), fails `cross_app_commit_span_sees_dependency_writes`; the dated note in `cli_digest_query_dependencies.rs` records both.

**Task 27, `c23846f3` (query reuses the l4 cell; old bundle builders and transient oracles deleted; non-convergence line printed by `build_l4`)**. Deleted: `build_db_effect_bundle`, `cross_app_db_effect_bundle`, `bundle_equals_the_old_builders` (TRANSIENT), `base_graph_equals_context_graph` (TRANSIENT), the solver's `eprintln!`, the dead `analyzer_diagnostic_lines`, `QuerySubstrate::from_cross_app_base`/`from_resolved` (no callers), `CrossAppInputs.injected_typed_edges` (no reader). `cross_app_context_graph` + `census_context_graph` became one `context_graph(model, base)` (the cross-app arm returns the base's `Arc`; the single-app arm builds and censuses), used by the View and by the shim (`build_detector_context_with(.., Option<&R3a5CrossAppBase>)`). The snapshot fold prints `render_warnings`. Every `cli_build_counters` row equals spec §4.4.8; no golden moved; no added `allow`.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::query_{touches,effects}_cross_app, query_single_app | `build_l4` called a second time in `QuerySubstrate::build` | `build.summaries_v2: got 2, want 1` (all 3 rows) | green |
| analysis::build_l4_prints_the_capped_line_once (NEW; spec §4.4.7's print site now pinned) | the print loop prints nothing | `left: []` | green |
| same | each line printed twice | `left: [<line>, <line>]` | green |
| capability_cone::dependency_publisher_roles_stay_unknown (A9 port, part (i)) | the port passes `None` for the cross inputs | STAYED GREEN: a stated limit, in the test doc (fixed roles change roles only; the one leaf has no out-edge) | n/a |
| same | `edges_by_from` removed for `fixed_roles` members in `build_l4` | `(i) OnDepRec has no subscriber Modify row in the store: []` | green |
| r4_cross_app_program::a_dependency_internal_edge_reaches_the_cone (re-proved: the extend now lives only in the base) | `build_cross_app_base` skips the injected-edge extend | `left: 0 right: 1` | green |

Open after Task 27: with `bundle_equals_the_old_builders` deleted, its m1 role assertion (the only proved pin that `build_l4` uses `cross`'s fixed roles and leaves) is gone. Next commit: break `build_l4` to ignore `cross` and see which test fails; if none, add the role assertion back on a production path. The snapshot fold's print site (`compute_uncertainty_coverage_reasons`) stays unpinned (no fixture caps; spec §4.4.7 stated limit).

**Task 26 review fixes, `3234214c`** (`task26-fix-report.md`; `ci-steps task` exit 0). Part A (is `build_l4`'s use of `cross` still pinned after the oracle's deletion?): YES. The Task 20 break (solve with no leaves and no fixed roles) fails 2 `--lib` tests: `dependency_publisher_roles_stay_unknown` (`(ii) OnDepRec: want (Unknown, ×5), got (Yes, Known([]), Yes, No, Full)`) and `issue107_dependency_publisher_presence_composes_subscribers` (`(ctx) OnDepRec.requiresLoadedAtEntry: want unknown, got yes`); r3/r4/cli stay green. Fixes: #1 exact `reads <bit> off det` match; #2 events releases `l4` right after `take_run` (spec §4.4.4 events row noted); #3 events releases `Ordering`; #4 caller docs; #5 duplicate comment; #7 `--strict` pinned; #8 imports.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| session::command_plans_declare_after_run, events::{fanout,chains}_without_d1_d9, digest_cli/prove::tx_context_without_d9 | `prepare`'s message names `CORE_<bit>` (a text the old `contains(bit)` accepted) | `the plan reads CORE_SUMMARIES off det after its run, but no run demands it` fails the exact match | green |
| cli_strict_events_policy::events_strict_exits_1_on_an_error_diagnostic (NEW) | the strict early return disabled in `events_view` | `fanout: exit 1 under --strict` left 0 right 1 | green |
| cli_strict_events_policy::policy_check_strict_exits_1_on_an_error_diagnostic (NEW) | the strict early return disabled in `run_policy_check` | `exit 1 under --strict` left 0 right 1 | green |

Task 27 review (opus `code-reviewer`, read-only, `task27-review.md`): 0 critical, 1 important (BLOCKING), 5 minor. I1: cross-app `alsem query` now keeps the whole `R3a5CrossAppBase` alive through the query and render (the View holds `AnalysisTarget::CrossApp(base)`); before, `from_cross_app_base` kept only `resolved`, graph, SCC and bundle; ~84 MB on CDO (step 0 census), unstated and unmeasured. Decision: fix, not just state: query takes the View apart into the parts it reads (model, graph, `l4`) so the base's cone-only fields are freed as at the parent. Key question answered: a test still FAILS if `build_l4` ignores `fixed_roles` (`dependency_publisher_roles_stay_unknown` part (ii) through the shim, and `issue107_dependency_publisher_presence_composes_subscribers`); NO test fails if it ignores `leaf_summaries` (already a stated limit: on every fixture a leaf has no out-edge, so the solver recomputes its direct rows). Also to fix: #2 the cross-app shim recomputes the SCC `base.combined_scc` holds (pass it); #3 `base.injected_typed_edges` is now a dead copy of edges already in `base.graph` (~16 MB on CDO; drop it, census from a local length); #4 stale comments (list in the review); #5 three `analysis.rs` helpers private; #6 one `build_l4` doc line stating the leaf limit.

**Task 27 review fixes, `19c421ec`** (`task27-fix-report.md`; `ci-steps task` exit 0, 3554 passed; no golden moved; no added `allow`). I1: `View::into_query_parts(self) -> (Model, Arc<CombinedGraph>, L4Analysis)`; `QuerySubstrate` holds only those, so the rest of the cross-app base is freed before the query and render, as at the parent. #2: the cross-app shim passes the base's SCC. #3: `R3a5CrossAppBase.injected_typed_edges` dropped (the edges move into `graph.typed_edges`, no clone); the census line `bucket=base.injected_typed_edges` (16,159,056 bytes on CDO at step 0) DISAPPEARS at head: Task 28 reads its absence as the saving of the parent's second copy (≥ 16 MB, its string heap was never in the shallow bucket). #4-#6 comments, visibility, the leaf limit in `build_l4`'s doc.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| capability_cone::cross_app_query_drops_the_base (NEW; red before the fix `left: (2, 2) right: (1, 1)`) | `into_query_parts` keeps the base alive (leaks it) | `(graph, scc) owners` `left: (2, 2)` | green |
| cross_app_shim_reuses_the_base_scc (NEW) | shim `scc: None` | `the shim computed its own SCC` | green |

All ten plan tasks (19-28's code tasks 19-27) are committed and every per-task review is closed with no open blocking finding. Next: Task 28 (CDO measurement, numbers file, CHANGELOG).

**Task 28, `9eb84928` (`docs: #160 step 2 numbers`)**: `docs/measurements/2026-10-19-issue-160-step2.md` and the CHANGELOG `[Unreleased]` entry. CDO, parent `1524b1fd` vs head `19c421ec`, `bench_ab.py` 5 rounds × 6 commands, all 60 runs exit 0, every stdout byte-identical. A2.3 met (digest saving 18.83 s ≥ bound 17.35 s); A2.4 met for all five bounded commands (`digest --single-app` post-attach +11 MB, the predicted `l4` overlap, inside its 561 MB bound); A2.2 met (cdo-measure 2277 / 30 712 findings, 0 fingerprints added or removed; histogram `c032c8db…`, fingerprint file `6c49731f…`, SARIF identical both sides); A2.1 met (CDO counter rows = §4.4.8). Prove's −1.66 GB peak is traced to the one-root effects pass (`digest.roots`), not the deleted context.

WSL Linux check at `9eb84928` (clone from the main repo; a worktree's `.git` points at a Windows path WSL cannot follow): `cargo clippy --release --all-targets --all-features -- -D warnings` exit 0; `cargo test --workspace --no-run` exit 0; `cli_build_counters::` + `cli_strict_events_policy::` 40 passed as uid 1000; the three unreadable-file tests RAN on Linux (no `skipped:` line with `--nocapture`). Log `H:/al-sem-160-step2/wsl-linux.log`.

Final code review (the built-in `code-review` skill at high, over `1524b1fd..9eb84928`): 10 findings F1-CR1..CR10. CR1 (important) refuted: spec §4.4.3 rule 1(c) by design, unreachable in step 2 (one `prepare` per command), batch is step 12's. CR4 deferred to step 12 (every step 2 plan has one run). CR2, CR3, CR5-CR10 fixed in `04b644e0` (`code-review-fix-report.md`; `ci-steps task` exit 0, 3556 passed; no golden moved).

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::digest_build_failed_row_carries_the_build_error (NEW, CR3) | `AnalysisTarget::build` back to `.map_err(\|_\| ..)` | `digest: the cross-app model could not be built (--single-app analyses the workspace alone)` (no build error) | green |
| session::release_all_but_keeps_only_the_named_cells (NEW, CR5) | `Cell::ALL` without `Cell::Ordering` | `ordering released` | green |

Measurement audit (`measurement-auditor`, opus, `.agent/runs/20261019-162640-c4cf84/measurement-audit.md`): 0 critical, 4 important, 8 minor, BLOCKING 0; every verdict holds when recomputed. All addressed in `8ab72066`:

- I1: analyze −99 MiB: the census explains 40.5 MiB, the rest is stated as unattributed.
- I2: query −38 MiB is inside the 3 % band, and the Task 27 fix contributes 0 against the parent.
- I3: a single-app `digest` census was RUN; the retained `l4` cell is 2.2 MB of the +11 MiB, the rest unattributed. The CHANGELOG wording "the solve overlaps" was corrected.
- I4: the process peak is unchanged for analyze and digest.
- M1: A2.3 on the harness clock: saving 18.84 s, bound 17.33 s, met.
- M2-M8: wording, units (MiB), the prove split 1.69 + 0.33 + ~0.67 s, and the protocol checks.

Re-measurement on the post-fix head `04b644e0` against the parent (`bench_ab.py` 3 rounds, `H:/al-sem-160-step2/bench2/`):

- A2.4 met again for all five commands.
- Every stdout, the histogram and the census SARIF are identical.
- The census differs from `19c421ec`'s only by `base.upgraded_bindings` (15 519 744 B) gone.
- The walls were noisy this run (another load). The fixes DO touch the path A2.3 measures (CR2 moves the field index into `build_l4`'s cross-app solve; CR5 changes `finish_default_run`), so A2.3 was re-measured on the final code (`42349a8d`, below; final panel P1-3).

Intermediate-commit notes (no code change): M-1, from `344369f0` until the View path (Task 24) the analyze shim keeps the `L4Analysis` (bundle AND SCC) alive through the detector loop, so a peak measured on an intermediate commit is not a step 2 result; M-2, the `context.ordering_facts` span now nests inside `context.build_total` / `crossapp.context_build` (it ran after the build before), so a Task 28 trace comparison of `context.build_total` moves by the ordering time on plans with d47/d49/d51.

Open from Task 20: `build_l4` and `census_l4` carry `#[cfg_attr(not(test), allow(dead_code))]` as a one-commit bridge; Task 21 wires them in and removes both.

Open from Task 19: the full mask stays `ALL` (spec §4.4.6 says `ALL | ORDERING_FACTS`). It matters only once `get_ordering_facts` `expect`s, so Task 21 makes that change.

## Findings register summary

- Spec panel (plan amendments, 5 rounds, `gpt-6.1-sol` + `claude-opus-5.5` via `pi_ask`): S1-1..S1-14, S2-1..S2-5, S3-1, S3-2, S4-1. 6 important, 16 minor; all fixed in the plan/spec, all accepted by both.
- Reviewers: the operator's standing instruction (also used for steps 1 and 1b) replaces the `/issue` roster: `gpt-6.1-sol` and `claude-opus-5.5`, marks under `reviews.sol` / `reviews.opus`, attested with `--substitute astra=sol --substitute flash=opus`.

## Gate results

On `8e8d2c9fdd7836ad32b400c5dbfce69d658d2283` (the code-review fixes, numbers and ledger committed; clean tree), all through `agentflow run`, `supervised: true`, queued 0 s. Logs under `.agent/runs/20261019-162640-c4cf84/logs/`; toolchain `rustc` 1.96, grammar `8682cb5f`; `cdo-gate` with `CDO_WS=U:/Git/DO-cdo-baseline/Cloud` from the environment (no argument).

| gate | exit | seconds | log |
|---|--:|--:|---|
| `ci-steps all` | 0 | 654.9 | `logs/ci-steps-all/20261019T213714-b7cf8091.log`: 3556 passed, 8 skipped |
| `check-goldens --verify-coverage` | 0 | 0.3 | `logs/check-goldens-coverage/20261019T214809-106f813e.log` |
| `check-goldens` | 0 | 24.8 | `logs/check-goldens/20261019T214810-70393da6.log` |
| `cdo-gate` | 0 | 247.0 | `logs/cdo-gate/20261019T214836-1353f76f.log`: `cdo-gate: PASS`, 2154 lib tests |

WSL Linux check on `8e8d2c9f`: clippy release `-D warnings` exit 0, `--no-run` exit 0, 41 CLI tests passed (`H:/al-sem-160-step2/wsl-linux2.log`).

Final panel r1 (`gpt-6.1-sol` + `claude-opus-5.5`, fresh threads; `final-r1-{briefing,sol,opus}.md`): no critical or important finding from either. sol accepted F1-CR1..CR10; opus accepted 9 and re-raised CR6 (the CHANGELOG headline misses `diff`'s per-side fold solve and the d47/d49/d51 ordering snapshot). New entries P1-1..P1-11 (all minor). The gates above predate the r1 fixes and run again on the new head.

Final panel r1 fixes: `42349a8d` (`final-r1-fix-report.md`; `ci-steps task` exit 0; no golden moved). P1-1, P1-2, P1-4, P1-5, P1-6, P1-8, P1-9 and the re-raised F1-CR6 fixed; P1-10, P1-11 deferred to step 12.

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| capability_cone::dependency_publisher_roles_stay_unknown_through_the_view (NEW, P1-1) | the View's det fill passes `None` for the cross inputs | `OnDepRec through the View; left: (Yes, Known([]), Yes, No, Full)`; the shim-path tests stay green under the same break | green |
| cli_build_counters::views_release_their_cells_before_the_next_build (NEW, P1-5) | policy's `release_all_but` before the RAW build deleted | `policy check: got [ctx, ctx, rel:Graph], want [ctx, rel:Det, rel:L4, rel:Ordering, ctx, rel:Graph]` | green |
| same | fingerprint's release and events' early `l4` release deleted | `fingerprint: got [ctx, snap]`; `events fanout: got [ctx, rel:Det, …]` | green |

Gates on `96c7967d` (the r1 fixes, numbers, ledger): `ci-steps all` exit 0 (609.5 s, `logs/ci-steps-all/20261019T221651-a0b165f6.log`), `check-goldens --verify-coverage` 0 (0.5 s), `check-goldens` 0 (31.1 s), `cdo-gate` 0 (154.8 s, `logs/cdo-gate/20261019T222735-f7d2ff27.log`), all supervised. WSL Linux check on `96c7967d`: clippy release `-D warnings` 0, `--no-run` 0, 42 CLI tests passed; `run_no_id_row_runs_one_build_and_keeps_the_text` run alone with `--nocapture` on Linux: ok, no `skipped:` line, so the new `cfg(unix)` literal executed (`H:/al-sem-160-step2/wsl-linux3.log`).

Final panel r2 (same threads; `final-r2-{briefing,sol,opus}.md`): no critical, important or code-defect finding. Both accepted P1-1, P1-3, P1-4, P1-5, P1-6, P1-8, P1-9, P1-10, P1-11; P1-2 accepted with the condition of a recorded Linux run (met above). sol re-raised P1-7 (record `largest_scc`'s anatomy) and both re-raised F1-CR6 (the headline's "until step 3" is wrong for policy's RAW build, which step 6 removes). opus new minors: R2-1 `view.release` is not listed as a trace change; R2-2 the release-sequence test omits digest and prove; R2-3 two stale records (this ledger's "do not touch" sentence, P1-2's evidence). All fixed in the r2 commit (docs + `final-r2-fix-report.md` for the test).

P1-3: A2.3 re-measured on `42349a8d` (bench3, 5 rounds, d + ds): saving 23.35 s ≥ bound 10.68 s, met. The machine was loaded (parent wall band 8.2 s), so the first bench's 17.33 s bound stays the tighter evidence. P1-7: `scc_stats` recorded (head once per digest, parent twice, identical payload). Both are in the numbers file.

## Discoveries

(none yet)

## Not delivered

`alsem batch` itself is not in step 2 (the epic puts it at step 12; an early batch is possible after step 2). Steps 3+ are later runs of #160.
