# Issue #160 ledger: attempt 3 (epic step 1: frozen InputSet)

Attempt 2's ledger and register (step 0) are on master at `5f096af9` (PR #165); `git show 5f096af9:.agent/issue-160/ledger.md`. This file replaces them for attempt 3.

## Claim

```json
{"run_id":"20261018-222527-440f4b","issue":160,"attempt":3,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/160-alsem-batch-build-the-model-onc-a3","worktree":"U:\\Git\\al-sem-issue-160-a3","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Issue body hash: `2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883`. The base comes from `make-worktree` (origin/master): `5f096af915a068d3673eeb0e45a83e7b9f041627`.

## Classification

**Architectural.** This is step 1 of the #160 epic. The owner re-scoped #160 on 2026-10-18: each step is its own PR, the PR says "Part of #160", it uses no closing keyword, and #160 stays open.

## Assumption probes

Steps 2-5 are carried. The epic spec and the steps 0-2 spec were designed and panel-converged in attempt 2: spec panel 4 rounds, 23 entries accepted by both reviewers. They have been on master since PR #165 (`docs/superpowers/specs/2026-10-18-issue-160-*.md`) and have not changed since. The plan is `docs/superpowers/plans/2026-10-18-issue-160-steps-0-2.md`; this run delivers **tasks 9-15 (step 1)**. Step 0's numbers file (`docs/measurements/2026-10-18-issue-160-step0.md`) is the baseline for this step's bounds.

## Base/Head SHAs

- make-worktree base: `5f096af9`. Rebase 1, onto `214a7879` (nextest + gate speed-ups): one CHANGELOG conflict, both sides kept.
- Rebase 2, onto `8405f96d` (#100 / #167): clean textually, but #167 added `tests/gap/gap_issue100_testfield_capability.rs` against the pre-step-1 `FullSnapshotOptions`/`PolicyCheckOptions` fields, and `ci-steps all` failed to compile (log `ci-steps-all/20261019T071250-ab9e7011.log`). Fixed in `ce7db1ed`: the test reads its inputs through `InputSet`, 14/14 pass. No assertion changed. `rebase_regate` is exhausted after this rebase.
- **B = `8405f96dd6ef849a37ee3cce327185174abec537`** (origin/master), **H = `ce7db1ed353669fb468dbba8088b808a430b9ede`**.

## Timeline

- Claim attempt 3 + make-worktree at `5f096af9`. Preflight showed `master-differs-from-origin`: another session's 2 unpushed docs commits on local master. With the owner's OK they were pushed (master `37f6de49`), so the preflight condition is cleared.
- A background heartbeat loop runs every 8 min. Claude Code stopped it once on low memory (the #100 session built in parallel); with the owner's OK it was restarted.
- `plan_tasks` charged 7 (tasks 9-15), 5 remaining.
- Implementation workflow, tasks 9-15: every task reviewed by opus. Task 10's review found a file I left out of the commit, fixed in `93c59e3b`. Task 9's `#![allow(dead_code)]` was refused by check-diff and removed.
- `code-review` high: 10 candidates. 7 fixed, 3 refuted with spec citations (`8cd6d98d`).
- Final panel r1: both reviewers accepted all 10 entries; 12 new findings merged into F2-1..F2-9 (`c670d966`). r2: opus accepted everything; sol re-raised F2-9 only (the empty Base/Head and Gate sections, now filled).

## Acceptance matrix

This run's acceptance rows are the steps 0-2 spec's step 1 rows (A1.*). Each one maps to a test in plan tasks 9-15.

## Discrimination proofs

Every break was made with the Edit tool and confirmed to apply exactly once before the test ran, then reverted to green. The SHAs below are the pre-rebase ones the proofs ran at. After the rebase onto 214a7879 they map (same trees apart from CHANGELOG): a47d9094→007c0205, 2041748c→b7b243db, 93c59e3b→649b989a, 09095df2→05bae2d3, 2796f6da→4522d226, 3126962e→626ee103, 648723d3→a8d281aa, 426e4c3d→376ef31b. The code-review fixes (8cd6d98d) carry their own proofs in `.agent/runs/20261018-222527-440f4b/code-review-r1-dispositions.md`.

**Task 9, `a47d9094` (InputSet types, read once)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| nested_app_files_are_listed_and_flagged | `nested_app: false && is_nested_app(..)` in `read` | `left: [("src/A.al", false), ("sub/src/B.al", false)] right: [.., ("sub/src/B.al", true)]` | green 8/8 |
| invalid_utf8_is_the_read_to_string_error | `text()` error text changed to "app.json is not UTF-8" | `left: "app.json is not UTF-8" right: "stream did not contain valid UTF-8"` | green |
| auto_detect_keeps_an_unreadable_yaml | auto-detect falls through on a read failure | `left: "...al-sem.policy.yml" right: "...al-sem.policy.yaml"` | green |
| snapshot_inputs_equal_derive_inputs | roots guard `if false && self.roots_config_loaded()` | ws-d51-jobqueue: left lacks the `roots-config` row | green |
| unreadable_al_file_keeps_its_slot | read error mapped to `Ok(Arc::from(""))` | `assertion failed: i.file("A.al").unwrap().text.is_err()` | green |
| model_instance_id_equals_the_gate_function | the version always 0.0.0.0 | left Some("d25646e60dbbbb2f") right Some("225874ed4a5f49e9") | green |
| build_only_* (2 tests) | not broken separately (they pin the build_only skips and the should_panic text) | - | green on Windows and WSL2 |
| Unix arm | all 8 under WSL2 Ubuntu (ext4, uid 1000), chmod arm of `unreadable.rs` | - | 8 passed, 0 skipped |

**Task 10, `2041748c` + `93c59e3b` (the build reads the InputSet)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| walk_error_fails_the_build (Win+WSL), walk_loop_fails_the_build (WSL) | `read`: `Err(_e) => (Ok(()), Vec::new())` | `assertion failed: inputs.al_walk.is_err()` | green, both platforms |
| walk_error_fails_the_build | provider: `anyhow!("{}", e.to_string())` instead of `Error::new(Arc::clone(e))` | the last `: <os>` part of the chain is gone (Win os error 5, WSL os error 13) | green |
| workspace_root_equals_walk_al_source | the provider reads raw bytes lossily instead of the InputSet text | left keeps the BOM, right does not | green |
| census-probe compile | the old call `build_program_with_coverage(&ws)` put back | `error[E0308]: expected &Arc<InputSet>, found &PathBuf` | builds |
| stated limit | the walk-error tests panic at their first assertion, so later assertions are not proven separately; a loop error has no `source()`, so the to_string break cannot fail the loop test | | |

**Task 11, `09095df2` (one InputSet per command)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| cli_build_counters::inputs_read_once | a second `InputSet::read` in `AnalysisTarget::build` | `"inputs.read": 2` | green |
| cli_inputs_frozen::al_file_added_after_read | `select_program_files` walks the disk again | left `findings: []`, `sourceUnitsParsed: 0`; right has the d1 finding | green |
| cli_inputs_frozen::roots_config_edit_after_read | `finish_resolved` reads `roots.config.json` from disk | `cli_inputs_frozen.rs:126` rootClassifications differ | green |
| switch_dump::analyze_builds_through_the_dumped_functions | a direct `project_coverage_inputs` call in `run_analyze_with_exit` (new needle) | `run_analyze_with_exit calls project_coverage_inputs directly` | the same call under the OLD needle stays green, so the needle swap matters; both reverted |
| cli_build_counters::inputs_relative_workspace_path_runs | `workspace_root = current_dir().join(..)` | `SnapshotBuilder: the InputSet was read for tests/r0-corpus/…, not U:\…` | green |

**Task 12, `2796f6da` (post-build readers; each break is a disk re-read instead of the InputSet)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| app_json_edit_after_read | (1) `read_workspace_apps`; (2) cross-app model id; (3) `finish_resolved` primary app; (5) `snapshot_rows` | (1) `pr-summary (read_workspace_apps)`; (2)/(3) `the cross-app model could not be built`; (5) `fingerprint (derive_inputs app-json row)` | green |
| coverage_yaml_edit_after_read, closure_app_replaced_after_read, roots_config_edit_after_read | `snapshot_rows` disk read | `workspaceFingerprint` differs; `inputs` rows differ | green |
| al_text_edit_after_read | `build_suppression_map` reads the `.al` | `left: 0 right: 1` | green |
| policy_edit_after_read | `load_policy_from_file(path)` | `left: None right: Some(..)` | green |
| baseline_edit_after_read | `load_baseline(&path)` | `left: 0 right: 2` | green |
| update_baseline_applies_the_set_just_written | apply `inputs.baseline` instead of the set just written | (a) `left: 2 right: 0`; (b) `baseline '..malformed.json' is not valid JSON` (exit 2) | green |

**Task 13, `3126962e` (objectless files, second parse deleted)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| objectless_file_removed_after_read | the diagnostics re-walk and re-parse (the old second parse) | `cli_inputs_frozen.rs:234` missing `No object declaration found in src/Empty.al` | green 11/11 |
| app_json_paths_edit_after_read | `count_app_json_paths(workspace)` | `:256` the multi-app DIAG-discover error appears | green |
| app_json_edit_after_read (reader 4) | the app id read from disk | `:179` json differs | green |
| walk_error_fails_the_build (diagnostics) | `if inputs.al_walk.is_err() && false` | `compute_workspace_diagnostics(&inputs, Some(&listed)).is_empty()` (stated limit: only the hand-stated `Some` list pins it) | green |
| run_analyze_with_exit_uses_the_shared_aggregation | an inline `.workspace_diagnostics` read in `run_analyze_with_exit` | `run_analyze_with_exit calls .workspace_diagnostics itself` | green |
| analyzer_diagnostics_of_run_keeps_every_source_in_order | drop source (1) | `left: [dependencies,…] right: [discover,dependencies,…]` | green |

**Task 14, `648723d3` (one writer)**

| test | break | fail output (1 line) | revert |
|---|---|---|---|
| analyze_closed_pipe_exits_2 | `println!("{out}")` brought back in analyze | `left: Some(101) right: Some(2)` | green 3/3 |
| digest_out_unwritable_exits_1, digest_closed_pipe_exits_1 | the old digest write block with its own `eprintln!` | `left: 0 right: 1` (no `al-sem: write error: …` line) | green 3/3 |

**Task 15, `426e4c3d`:** a docs-only measurement task, with no new test.

**Final panel r1 fixes, `c670d966`:**

| test | break | outcome | revert |
|---|---|---|---|
| walk_error_fails_the_build (A1.7: the Windows literal is the parent's text) | `snapshot.rs:274` provider put back to the parent's disk walk `walk_al_source(&inputs.workspace, …)` | PASSED with no `skipped` line: the committed literal equals the old code's error text (A1.7 provenance; captured at 007c0205, whose build path equals the parent's) | restored |
| analyze_json_reports_the_captured_driver_version | `run.rs:667`, then `:819`, back to `driver_version()` | FAILED: left "1.7.2" right "captured-160" | ok |
| fingerprint_inventory_only_usage_errors_read_no_inputs | `false && f.inventory_only` in the pre-read check | FAILED: `inputs.read` left 1 right 0 | ok |
| assert_row `inputs.read` (digest rows) | a second `InputSet::read` in `run_digest_cmd` | FAILED: `[digest cross-app] inputs.read: got 2, want 1` | ok |
| census_model_rows_counts_the_workspace_diagnostics | the `workspace_diagnostics` census line removed | FAILED: left 0 right 216 | ok |

Stated limit: the Linux A1.7 literal was not re-run in this round. Task 10's WSL rows are its evidence.

## Findings register summary

## Gate results

Every gate ran supervised under run `20261018-222527-440f4b`. Logs are under `.agent/runs/20261018-222527-440f4b/logs/<gate>/`. `ci-steps test` and `check-goldens` run on cargo-nextest; `cdo-gate` stays on `cargo test`.

| head | ci-steps-all | coverage | check-goldens | cdo-gate | note |
|---|---|---|---|---|---|
| 376ef31b (after rebase 1) | 0 (376 s) | 0 | 0 (25.5 s) | 0 (130 s) | |
| 8cd6d98d (code-review fixes) | 0 (641 s) | 0 | 0 (27 s) | 1 | the cdo-gate build saw the fix agent's uncommitted edits (`unresolved import reject_inventory_only_combos`); not a verdict on 8cd6d98d |
| c670d966 (final r1 fixes) | 0 (382 s) | 0 | 0 (27 s) | 0 (149 s) | an earlier run was stopped on low memory and re-run |
| beb4d531 (after rebase 2) | **1** | 0 | 0 | 0 | the #167 test does not compile against step 1's API (see Base/Head) |
| **H = ce7db1ed** | **0** (266.6 s, `20261019T072639-01b90a6c.log`) | **0** (`20261019T073106-802efe36.log`) | **0** (33.2 s, `20261019T073107-a3a0d5f9.log`) | **0** (82.9 s, `20261019T073141-7d70e58d.log`) | final; the tree was clean after |

No golden moved in step 1. Toolchain: Rust 1.96.0. Grammar: tree-sitter-al `main` at the committed `node-types.sha256`. CDO_WS: the pinned baseline, enforced by cdo-gate's stamp checks.

## Discoveries

## Not delivered

Steps 1b (plan tasks 16-18) and 2 (tasks 19-28) are not in this run. Each is its own later run of #160.
