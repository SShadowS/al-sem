# Issue #89 ledger

## Claim

```json
{"run_id":"20261016-141653-d9d838","issue":89,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/89-agent-discovery-gate-diff-cli-a-a1","worktree":"U:\\Git\\al-sem-issue-89-a1","body_hash":"726b4dbd3fbb0d6f1e30c308627c35ddef825ba50745a29005a24845ed8b096e"}
```

Issue body hash: `726b4dbd3fbb0d6f1e30c308627c35ddef825ba50745a29005a24845ed8b096e`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Their marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** One CLI orchestrator (`src/engine/gate/diff/cli.rs`) ignores a flag. The gate it should apply already exists, shared by `fingerprint`, `events` and `policy`. No new subsystem is needed.

## Design

**Defect.** `run_diff` (`src/engine/gate/diff/cli.rs:171-177`) discards `opts.strict` (`let _ = opts.strict;`) and always uses an empty analyzer-diagnostic list. `alsem diff --help` promises "Exit 1 on any error-severity analyzer diagnostic (workspace mode)" (`src/bin/alsem.rs:444`). So in workspace mode, `--strict` does nothing: `alsem diff wsA wsB --strict` exits 0 on a workspace where `alsem fingerprint wsB --strict` exits 1.

**Fix (revised after spec round 1).**
- **Lines.** `load_snapshot_from_workspace` already builds that side's `Model` to compose the snapshot. When `--strict` is set, it also returns that side's analyzer-diagnostic lines. It moves the model into `AnalysisTarget::SingleApp(Box::new(resolved))` (the variant holds a `Box<Model>`, `src/engine/gate/run.rs:173-176`), takes the lines from the shared `analyzer_diagnostic_lines(ws, &target)` (`run.rs:1122-1130`) and composes the snapshot from `target.resolved()`. That is the same `compute_analyzer_diagnostics` list `fingerprint --strict` gates on (`fingerprint_cli.rs:547-555`), in the same `<severity>: <message>` form `fingerprint`, `events` and `policy` print. `diff` stays single-app (#70), hence `SingleApp`.
  - Each side is resolved, its lines collected and its snapshot composed before the next side loads. So at most one model is alive at a time, as today, and the gate runs after both sides have loaded. Holding both models in order to gate before composing would double peak memory, for a failure path only.
- **Scope.** The gate covers EVERY workspace-mode input: old, new, or both. A mixed run (a snapshot file on one side) gates on its workspace side. A diff is only as trustworthy as the analysis of both its sides, and the help says "any error-severity analyzer diagnostic". A snapshot file contributes nothing: snapshot deserialization keeps no analyzer diagnostics (`src/engine/gate/snapshot_deserialize.rs:116-141`).
- **Position.** The gate runs right after both sides have loaded and before the `--renames` overlay load. It therefore pre-empts the engine run, the formatter, `--fail-on` and the strict-coverage gate, as `events` gates before its coverage handling (`src/engine/gate/events.rs:407-455`).
- **Failure output.** If any collected line has severity `error`, `run_diff` returns `output: None`, `error_message: None`, exit 1, and `stderr_lines` = every collected line. `output: None` matters: `src/bin/alsem.rs:977-1001` would truncate `--out` on `Some("")`. Nothing goes to stdout, nothing is written to `--out`, and the workspace-mode note is OMITTED, as `fingerprint`, `events` and `policy` print only the diagnostics. Order: the old side's lines in their own order, then the new side's IN FULL (`strict_gate_lines`). Revised in final-panel round 1 (code-review 2): the earlier rule dropped a new-side line identical to an old-side one, which hid that the NEW side has a path-free error the two revisions share. The only dedupe left is when both arguments are the SAME workspace (`same_workspace`, compared canonically): then its list prints once. Each side prints exactly what `fingerprint --strict` would for it.
- **No error.** Under `--strict` with no error-severity line, the run continues exactly as without `--strict`, and warning or info lines are NOT printed. That is deliberate: it keeps `--strict`'s success output identical to a non-strict run, so no ws-mode golden moves. Printing analyzer diagnostics on a successful ws-mode run is a separate change (see Not delivered).
- **Unchanged paths.** Without `--strict`, diagnostics are not computed at all (it costs a full default-detector run), the envelope `diagnostics` stays `[]`, and the stderr note is unchanged. Snapshot-input mode is unchanged: `--strict` stays a no-op there, and every diff golden is byte-identical.
- The stale comment at `cli.rs:171-176` and the module doc are corrected.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| `run_diff` discards `strict` and uses an empty list | confirmed | `src/engine/gate/diff/cli.rs:176-177` |
| help promises a workspace-mode gate | confirmed | `src/bin/alsem.rs:444` |
| a shared `<severity>: <message>` helper exists | confirmed | `analyzer_diagnostic_lines`, `src/engine/gate/run.rs:1122-1130` |
| fingerprint's gate: error -> all diags to stderr, exit 1, no output | confirmed | `src/engine/l5/fingerprint_cli.rs:361-378` |
| a fixture can produce an error-severity analyzer diagnostic AND still load | by code inspection (model id, file selection; `build_program_with_coverage` not traced), so T2 is the runtime proof. Expected for a nested second `app.json` (no `.al` beside it) next to a valid root app: `multi-app source workspace unsupported` (error, stage `discover`). `compute_gate_model_instance_id` reads only the root id and does not reject a second manifest, and model selection keeps the root app's files. A root `app.json` with no `id` is NOT usable: model-id computation fails before any gate (spec round 1). | `src/engine/gate/workspace_diagnostics.rs:44-65`; `src/engine/gate/model_instance_id.rs:75-95`; `src/program/model/workspace.rs:1636-1685` |
| no test sets `strict: true` for any command today | confirmed (`grep -rln "strict: true" tests src` is empty) | the acceptance test is the first |

## Base/Head SHAs

B = `521fc69edd11166653803f6ef58b24a4e8238bcd` (origin/master; unchanged since the claim, so no rebase was needed). H = `11e360117075ac19511f32b32d5f8e3ac819328c`.

## Timeline

- Worktree created at master `521fc69e`.
- Classified as bounded; design written.
- Spec panel (sol + opus, the operator roster), 3 rounds (spec_rounds 3; pi_calls 6):
  - r1: both CONFIRMed the core design (gate both sides, failure = stderr + exit 1). Fixes: `SingleApp` takes a `Box<Model>`; T4 compares full output; `output: None`; the id-less fixture is unusable; T5 checks that a clean pair stays quiet; gate position and the omitted note.
  - r2: sol re-raised S6, so T8 (order) and T9 (gate before the overlay) were added. Opus raised N1-N3 (`RUST_LOG=off`, a load-failure assert, T7 uses the same path).
  - r3: both accepted all 12; no new findings. CONVERGED.
- Plan: one task (bounded; the ledger's Design is the spec). Steps 6 and 8 were done by ONE opus subagent in TDD order: tests red first, then the implementation. This deviates from the skill's separate test dispatch; the RED output is recorded below. subagents 1.
- T1 (opus): `cli_diff_strict` 9/9; `cargo test --test cli` 359 passed; lib 2032 passed; clippy 0; no golden moved.
- T1 review (sonnet, subagents 2): 0 critical, 0 important, 8 minor.
  - Taken: a doc comment on `DiffCliOptions.strict`; the shared failure assert now requires every stderr line to be a diagnostic line.
  - Not taken:
    - a micro-optimisation of the dedupe;
    - T8 already checks positions (the filtered list keeps stderr order);
    - the CHANGELOG entry is under Unreleased → Fixed (checked);
    - staging is limited to the code paths;
    - the within-side repeat rule is the stated limit (proof v);
    - a warning-only `--strict` run is untested (Not delivered).
  - Re-run: `cli_diff_strict` 9/9.
- Commit `f8963bcf`; the hook's goldens OK; `check-diff` gave no reasons. Gates round 1 at `f8963bcf`: all 0.
- Final panel round 1 (final_rounds 1):
  - `code-review` high (the built-in skill) raised 8 findings, C1-C8.
    - Fixed: C2 (the dedupe hid a shared path-free new-side error; now `strict_gate_lines` prints both sides in full, one list only for the same workspace) and C7.
    - Deferred: C1, C5 and C6 (discoveries).
    - Refuted: C3, C4 and C8.
  - Commit `7e05bc34`; gates round 2 all 0.
  - sol and opus accepted all 20 entries. Sol raised F1 (T10 added) and F2 (the proof row ii wording).
  - Commit `11e36011` (T10).
- Final panel round 2: sol accepted all 22 with no new findings. Opus accepted all 22 and raised G1 and G2, both ledger wording (break c's description, stale line numbers); fixed.
- Final panel round 3: both accepted all 24; no new findings. CONVERGED. final_rounds 3; pi_calls 12.
- `origin/master` is still `521fc69e`, so no rebase is needed.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| A test with an error-severity diagnostic in workspace mode makes `diff --strict` exit 1 and print the diagnostic | New `tests/cli` member, run through the `alsem` binary with `RUST_LOG=off`, so an `env_logger` warning can never add a stderr line. Every failing case (T1, T3, T6-T10) also asserts that stderr has no `failed to load snapshot` line: the input loaded, and the gate is what failed. T2 is the runtime proof that the nested-manifest workspace loads.<br>**T1**: old = a clean workspace, new = the same plus a nested second `app.json` (the multi-app error). `alsem diff <old> <new> --strict --out <f>` exits 1. Its stderr contains a line starting `error: multi-app source workspace unsupported` and does NOT contain the workspace-mode note. stdout is empty. An existing sentinel at `<f>` is byte-unchanged, and a run with an absent `<f>` creates no file.<br>**T2** (the input loads; the gate fails, not the input): the same inputs without `--strict` exit 0 and print the diff.<br>**T3**: an error on the OLD side alone exits 1 under `--strict`.<br>**T6** (mixed): a clean snapshot file as old and the error workspace as new exits 1 under `--strict`.<br>**T7** (dedupe): the same error workspace, passed as the SAME path for old and new (the multi-app message embeds absolute paths, so two copies would give two different lines), prints its error line exactly once.<br>**T8** (order): two DIFFERENT error workspaces (their multi-app messages differ by path) as old and new. Under `--strict` stderr holds the old side's error line BEFORE the new side's.<br>**T10** (non-error lines kept; added in final-panel round 1): an old side whose only diagnostic is `info` (an `.al` file declaring no object) and a new side with the error. stderr carries both, the info line first.<br>**T9** (position): the error workspace with `--strict --renames <a path that does not exist>` exits 1 with the analyzer error line and WITHOUT the `failed to load rename overlay` error. That proves the gate runs before the overlay load. |
| snapshot-input mode is unchanged | **T4**: for the existing clean snapshot pair, `run_diff`'s outcome (stdout text, stderr lines, exit code) with `--strict` is byte-identical to without it, for `json`, `human` and `sarif`.<br>**T5** (clean ws pair, the gate stays quiet): `alsem diff tests/fixtures/ws-diff-removed-field/a b --strict` exits 0. Its stdout byte-equals `tests/cli-b-goldens/diff/ws-mode-schema-field-removed.stdout.txt`, and its stderr is exactly the note, as in `ws-mode-schema-field-removed.stderr.txt`.<br>Plus every diff golden family byte-identical (`check-goldens`). |

## Discrimination proofs

The implementer (opus) made each break with Edit and reverted it with Edit, then touched `cli.rs`. Before any fix, the RED run gave T1, T3, T6, T7, T8 and T9 FAIL (exit 0, or the overlay loaded first) and T2, T4 and T5 PASS (the controls).

| # | break | result | decisive fail line |
|---|---|---|---|
| i | gate never fires (`if any_error && false`) | T1, T3, T6, T7, T8, T9 fail | `cli_diff_strict.rs:96 left: Some(0) right: Some(1)`; T9 `:307` overlay error (line numbers as of `11e36011`; the T1-review assert moved later lines down by 6) |
| ii | gate on the NEW side only. The exact edit: `any_error` scans only `new_diag_lines`, AND the printed list starts EMPTY (the old side's lines are dropped from the output too). A predicate-only change would still print both lists, so T8 would pass and only T3 would catch it. | T3, T8 fail | T3 `:96`; T8 `:280 one line per side ... left: 1 right: 2` |
| iii | gate moved below the overlay load | T9 fails | `:307 the overlay must not be loaded; stderr=["failed to load rename overlay: ..."]` |
| iv | `output: Some(String::new())` | T1 fails | `:143 --out must not be touched` |
| v | drop ALL duplicates (also within a side) | no test fails | **stated limit**: no fixture yields a repeated line within one side, so only the cross-side dedupe is pinned (T7, T8) |

After every revert: `cli_diff_strict` 9/9 and `cargo test --test cli` 359 passed.

Final-panel round 1 fix (dedupe rule revised), proved by the conductor with Edit:

| # | break | result | decisive fail line |
|---|---|---|---|
| a | `strict_gate_lines` drops new-side lines equal to an old-side line | U1 `diff::cli::tests::strict_gate_lines_keep_a_shared_line_from_two_workspaces` fails | `left: ["error: cap hit", "warning: w"] right: ["error: cap hit", "warning: w", "error: cap hit"]` |
| b | `same_workspace` always false | T7 fails | `cli_diff_strict.rs:269 printed exactly once; got [<the multi-app line twice>]` |
| c | `strict_gate_lines` filters the OLD side's list down to its `error:` lines and leaves the new side unchanged, so the old side's non-error lines are dropped (final-panel round 1, sol: collection must keep non-error lines). The new side's error still prints, so `assert_gate_failed` passes and the failure lands at T10's own info-line check. | T10 fails | `cli_diff_strict.rs:334 the old side's info line; stderr=["error: multi-app ..."]` |

After the reverts: U1 1/1 and `cli_diff_strict` 9/9. Proof v (the old within-side limit) no longer applies: no dedupe runs within or across sides, apart from the same-workspace case.

## Findings register summary

24 entries, every one accepted by both sol and opus.
- 17 fixed, including every critical and important entry (S1, S2, S3, S4, C2).
- 4 refuted: S9, C3, C4 and C8.
- 3 deferred, all minor and filed as discoveries: C1, C5 and C6.

## Gate results

Final, at H = `11e360117075ac19511f32b32d5f8e3ac819328c`. Every gate ran with `supervised: true`; logs are under `.agent/runs/20261016-141653-d9d838/logs/`.

| gate | exit | log | tests |
|---|---|---|---|
| ci-steps-all | 0 | `ci-steps-all/20261016T153051-068e109d.log` | 3283 passed, 0 failed |
| check-goldens-coverage | 0 | `check-goldens-coverage/20261016T153655-9722eb19.log` | coverage OK |
| check-goldens | 0 | `check-goldens/20261016T153655-103b0a3d.log` | 794 passed, 0 failed; no golden moved |
| cdo-gate | 0 | `cdo-gate/20261016T153805-39432322.log` | 2251 passed, 0 failed; ratchets green |

- Earlier rounds: at `f8963bcf` (round 1) and `7e05bc34` (round 2), all four gates exited 0.
- After every round, `git status --porcelain -- . ':!.agent'` was empty.
- Toolchain: rustc 1.96.0. Grammar: tree-sitter-al `8682cb5f`. CDO_WS identity hash: `f1d20e6aac6f8b6e` (the pinned baseline at `bc3ccb18`).

## Discoveries

```json
[
  {"subsystem": "gate / diff CLI (snapshot inputs)", "locator": "src/engine/gate/snapshot_deserialize.rs:116-141 (the envelope unwrap keeps only payload); src/engine/gate/diff/cli.rs load_side", "symptom": "alsem diff old.cbor.gz new.cbor.gz --strict never gates: the analyzer diagnostics `fingerprint` writes into the snapshot envelope are dropped on deserialization, so the CI form the ws-mode note recommends (persist snapshots with fingerprint, diff those) cannot fail on an analyzer error recorded in a snapshot.", "kind": "enhancement", "origin_issue": 89, "reproducer": "alsem fingerprint <ws with a nested second app.json> --format cbor.gz --out new.cbor.gz (no --strict); alsem diff old.cbor.gz new.cbor.gz --strict -> exit 0, although the envelope's diagnostics hold the multi-app error.", "pre_existing": true, "capability": "diff --strict gates on the analyzer diagnostics carried by a snapshot file's envelope", "acceptance": "a snapshot whose envelope carries an error-severity diagnostic makes diff --strict exit 1 and print it; a snapshot with none is unchanged; snapshot goldens move only if a golden envelope carries an error."},
  {"subsystem": "gate / strict gates (fingerprint, diff, events, policy)", "locator": "src/engine/l5/fingerprint_cli.rs:366 (sev == \"error\"); src/engine/gate/events.rs:407, :552; src/engine/gate/policy/pipeline.rs:219; src/engine/gate/diff/cli.rs (starts_with(\"error:\"))", "symptom": "The --strict error test exists in four copies in two forms: a structured severity compare in fingerprint, a formatted-string prefix in diff/events/policy. A change to the line format silently disables three of them.", "kind": "enhancement", "origin_issue": 89, "reproducer": "grep -rn 'starts_with(\"error:\")\\|sev == \"error\"' src/engine", "pre_existing": true, "capability": "one shared strict-gate helper in gate::run decides on structured severity and yields the stderr lines", "acceptance": "all four commands call the helper; a unit test pins the decision on structured severity; no golden moves."},
  {"subsystem": "gate / diff + fingerprint (--strict cost)", "locator": "src/engine/gate/diff/cli.rs load_snapshot_from_workspace; src/engine/l5/fingerprint_cli.rs:364 then compose_full_snapshot", "symptom": "Under --strict each workspace side solves twice: analyzer_diagnostic_lines runs the default detector set (L4 summaries + detectors), then compose_full_snapshot builds its own cone/producer solve. On CDO-sized workspaces that roughly doubles the --strict run time.", "kind": "enhancement", "origin_issue": 89, "reproducer": "time alsem diff <ws> <ws> --strict vs without --strict on a large workspace (not measured).", "pre_existing": true, "capability": "the strict diagnostics and the snapshot share one solve", "acceptance": "a measured before/after on DO shows the --strict overhead drops, with byte-identical diagnostics and snapshots."},
  {"subsystem": "gate / diff CLI (ws-mode envelope diagnostics)", "locator": "src/engine/gate/diff/cli.rs (analyzer_diagnostics = Vec::new() for the envelope); src/engine/gate/diff/format.rs:296-314", "symptom": "A workspace-mode diff's JSON envelope always has diagnostics: [], while fingerprint fills the same channel from compute_analyzer_diagnostics; a consumer of diff --format json never sees analyzer diagnostics.", "kind": "enhancement", "origin_issue": 89, "reproducer": "alsem diff <ws with a nested second app.json> <same> --format json (no --strict) -> \"diagnostics\": [] although fingerprint reports the multi-app error for that workspace.", "pre_existing": true, "capability": "ws-mode diff --format json carries each side's analyzer diagnostics in the envelope", "acceptance": "ws-mode JSON envelopes carry the diagnostics (side-attributed); ws-mode goldens move only in the diagnostics field, each triaged."}
]
```

## Not delivered

- No test drives `--strict` over a workspace with a WARNING-only diagnostic (the design says such a run continues unchanged and prints nothing extra). T5 covers a clean pair, and no small fixture yields a warning-only analyzer diagnostic.
- Analyzer diagnostics are not printed on a SUCCESSFUL workspace-mode run, with or without `--strict`. That is unchanged behaviour, and it keeps ws-mode goldens byte-identical.
- The workspace-mode JSON envelope's `diagnostics` channel stays `[]`. `fingerprint` fills it, `diff` never has, and filling it would move ws-mode goldens. That is outside this issue's acceptance; it is recorded as a discovery.
