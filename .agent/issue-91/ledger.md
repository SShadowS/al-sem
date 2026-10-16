# Issue #91 ledger

## Claim

```json
{"run_id":"20261016-162024-49d4b6","issue":91,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/91-agent-discovery-gate-analyzer-d-a1","worktree":"U:\\Git\\al-sem-issue-91-a1","body_hash":"dd571f6d9dd112016a514fdc2f961033e29689f1459867fdc572d10679039565"}
```

Issue body hash: `dd571f6d9dd112016a514fdc2f961033e29689f1459867fdc572d10679039565`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** One diagnostic source is missing from one shared aggregation function. The data already exists in the program build; it is just not carried to the model.

## Design

**Defect.** `alsem analyze` concatenates SIX diagnostic sources inline (`run_analyze_with_exit`, `src/engine/gate/run.rs:~555-590`). Source (2) is `dependency_diagnostics(fc)`: the program build's dependency-ledger warnings (missing, older than declared, unreadable, unidentified packages, `run.rs:421-458`).
- The shared aggregation every other command uses, `analyzer_diagnostics_of_run` (`run.rs:1149-1170`, reached through `compute_analyzer_diagnostics(_cross_app)` and `analyzer_diagnostic_lines`), has no (2).
- So `fingerprint`, `digest`, `events fanout`/`chains`, `policy check` and `diff` report a different, smaller set than `analyze` does for the same workspace (callers: `fingerprint_cli.rs:551`, `digest_cli.rs:54,65`, `events.rs:405,550`, `policy/pipeline.rs:216`, `diff/cli.rs:107`). `prove` reports through digest's.
- The general defect is the shared aggregation's omission. It reaches the commands through two paths (revised after spec round 1):
  - **single-app**: `assemble_and_resolve_workspace_program` (`program_calls.rs:699-714`) calls `build_program_with_coverage` and DISCARDS the `FreshCoverage` (`:704-705`);
  - **cross-app**: the coverage IS kept (`assemble_and_resolve_cross_app_program` → `build_cross_app_base`, `program_calls.rs:807-808,885-903`; `capability_cone.rs:3470-3477`), but the aggregation never reads it.
- `docs/OUTSTANDING.md:151-156` records the gap and the fix ("keep it on the model and emit the same warnings"). This issue closes that entry.

**Fix.**
1. `Model` gains `dependency_diagnostics: Vec<Diagnostic>`: the program build's dependency-ledger warnings, in ledger order.
2. `attach_program_calls` and its cross-app twin `attach_program_calls_with` take the build's `&FreshCoverage` as a required parameter and fill the field. They are the constructors of every PROGRAM-BACKED model. A required parameter means no program-backed model can be built without stating its coverage, so the omission cannot recur silently for them.
   - Their three call sites pass the coverage of the same build:
     - `build_analysis_model` (single-app, `run.rs:352`). Its `fc` was already moved into `fresh` (`run.rs:~287`), so it passes `fresh.as_ref()`, which is `Ok` in that branch, or borrows before the move;
     - `assemble_and_resolve_workspace_program`: the coverage it discarded;
     - `assemble_and_resolve_cross_app_program`: `coverage` from `build_cross_app_program`, the same object `build_cross_app_base` keeps as `base.coverage`, so analyze's cross-app (2) and the model's agree.
   - `Model` also has direct struct literals: the empty-output HTML model (`run.rs:~934-945`) and HTML formatter tests (`format_html.rs:1209-1220,1265-1276`). They get `dependency_diagnostics: Vec::new()`. They carry no program build, and the empty-output paths keep computing only workspace diagnostics (`run.rs:850-862,963-969`), unchanged.
3. `dependency_diagnostics` moves from `engine::gate::run` to `FreshCoverage::dependency_diagnostics()` (`program::resolve::full`, next to the ledger it reads), so the program layer does not call up into the gate. It has no caller outside `run.rs` (grep). The body moves VERBATIM: one entry can emit both an ingest-error and an older-version warning, and unidentified-package warnings follow all ledger warnings. Text and order are unchanged.
4. `analyzer_diagnostics_of_run` adds source (2), `resolved.dependency_diagnostics`, right after (1), in analyze's order.
5. `run_analyze_with_exit` drops its inline block and calls `analyzer_diagnostics_of_run(ws_path, resolved, &run)` with ITS OWN detector run, not `compute_analyzer_diagnostics`, which would re-run the default set. So the selected detectors' diagnostics are kept. This is ONE aggregation for every command. The doc comment listing the six al-sem sources moves onto `analyzer_diagnostics_of_run`. The stale "(2) not produced here" doc (`run.rs:1089-1090,1095-1104`) is corrected, and the `docs/OUTSTANDING.md` entry is closed.
6. **analyze is unchanged.** Same sources, same order, same text. The model's diagnostics come from the same `FreshCoverage` analyze already used. Analyze's `(2)` was gated on `fresh` being `Ok`, and a successfully built PROGRAM-BACKED `Model` only exists when it is. The direct literals of step 2 are built only on the empty-output path, which never reaches this aggregation. So every analyze golden must stay byte-identical.
7. **Other commands GAIN the dependency warnings**, which is the intended change. Their goldens move only where a fixture's ledger has a problem. Candidates are the r0-corpus fixtures that declare dependencies (`ws-preflight-missingdep`, `ws-d17-drift`, `ws-d16-obsolete`, `ws-d13-*`, `ws-d1-dep-terminal`, `ws-r2b-opaque`, …); none is presumed to move. **Triage rule** (revised, spec round 1):
   - In JSON goldens (fingerprint/digest/prove envelopes), a moved golden may only ADD `{code: "DIAG-dependencies", message, severity: "warning"}` records, plus the bracket/whitespace change of a formerly empty array.
   - In stderr goldens (events, policy, and any `--strict` run), it may only ADD `warning: dependency …` / `warning: dependency package could not be read: …` lines. They come right after the workspace-diagnostic lines and before the summarize/overlay/detect lines.
   - Every existing diagnostic, its relative order, every payload byte and every exit code stay unchanged. Anything else is unexplained and blocks the regen.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| analyze includes dependency diagnostics, the shared aggregation does not | confirmed | `run.rs:584` vs `run.rs:1149-1170` |
| the single-app model path discards the coverage; the cross-app path keeps it but the aggregation never reads it | confirmed | `program_calls.rs:704-705`; cross-app `program_calls.rs:807-808,892-903`, `capability_cone.rs:3470-3477` |
| `attach_program_calls(_with)` are the only constructors of PROGRAM-BACKED models (3 call sites); 3 direct `Model` literals exist (the empty-output HTML model, 2 HTML tests) and get an empty Vec | confirmed | `run.rs:352`, `program_calls.rs:713`, `attach_program_calls_with`; literals `run.rs:~934-945`, `format_html.rs:1209-1220,1265-1276` |
| cross-app keeps its coverage | confirmed | `build_cross_app_program` returns `FreshCoverage` (`full.rs:2120-2122`); `base.coverage` is used at `run.rs:376` |
| fixtures reproduce it | (a) the issue's case: `tests/r0-corpus/ws-codeunit-rec` with `CodeunitRecLibA` declared at `2.0.0.0` against the packaged `1.0.0.0` (the loader keeps it; the ledger matches by guid; `below_declared_version` compares numerically) gives `is present only at version 1.0.0.0`. (b) `tests/r0-corpus/ws-preflight-missingdep` declares a dependency with no package and gives `is missing`. | `ws-codeunit-rec/app.json` and its packaged `probe_CodeunitRecLibA_1.0.0.0.app`, `full.rs:~1945-1955`, `~2340-2360`, `dependencies.rs:65-80`; `ws-preflight-missingdep/app.json` |

## Base/Head SHAs

B = `0ead34f718478394b5198485319f6c2757eaab93` (origin/master; unchanged since the claim, so no rebase was needed). H = `5c7de0b360bc05aa96a88763393740b3cbdc8f08`.

## Timeline

- Worktree created at master `0ead34f7`.
- Classified as bounded; design written.
- Spec panel (sol + opus, the operator roster), 5 rounds (spec_rounds 5/5; pi_calls 10):
  - r1: both confirmed the approach. Fixes: two propagation paths, direct `Model` literals, prove added, diff via a loader unit test, a stricter triage rule, the older-version fixture, same-mode comparison, a source check for the shared call, `fc` moved, docs.
  - r2: S1/S2 re-raised (wording); S6 re-raised by both, so T2 now asserts the exact message before equality; new N1 (digest input), N2 (fingerprint B0), N3 (pin 2.0.0.0).
  - r3: all accepted; P1 (probe row).
  - r4: all accepted; P2 (digest path).
  - r5: both accepted all 15; no new findings. CONVERGED.
- T1 (opus, TDD): T1-T4 red, then green; proofs i-iv; `cargo test -p al-sem --lib` and clippy clean; analyze goldens byte-identical. 3 events stderr goldens moved (triaged in `.agent/golden-triage.md`: +2 `dependency "System" / "System Application" ... is missing` lines each for `ws-event-partial-coverage`, whose platform is 26.0.0.0 with no `.alpackages`). `check-goldens` (no regen) over every target: only those 3. Then `check-goldens-regen` exit 0, moving exactly those 3 files.
- T1 review (sonnet): 0/0/8 minor. Taken: drop the `FreshCoverage` clone (`fresh.as_ref()`); T3 cut at the next item plus a late anchor; T3 bans `summarize_diagnostics`/`infra_diagnostics`; CHANGELOG wording. Commit `32f27f6c`; gates round 1 all 0.
- Final panel round 1:
  - `code-review` high raised 10 (C1-C10).
    - Fixed: C1 (CHANGELOG qualified for analyze's empty-output path), C4 (`model_text` dumps the field; the inline-vs-disk test checks the disk-only `is missing` warnings explicitly), C5, C6, C7, C8 and C10.
    - Refuted: C2 and C9.
    - Deferred: C3 (absolute paths in diagnostics; pre-existing, filed as a discovery).
  - Commit `7d53e6ed`; gates round 2 all 0.
  - sol accepted 24 and re-raised C4: an empty list passed the `.all()` guard. Now the test requires warnings for every fixture that declares dependencies without packages, and that at least one compared fixture does; proved by break c4.
  - opus accepted all 25 and raised F1: T3's anchor was not late. It now anchors on the function's last statement.
  - Commit `5c7de0b3` for both.
- Final panel round 2: both accepted all 26. Opus raised G1, a ledger-only fix (the c4 row was outside the proofs table, and a commit hash was missing); fixed.
- Final panel round 3: both accepted all 27; no new findings. CONVERGED. final_rounds 3; pi_calls 16.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| run_analyze_with_exit calls the shared aggregation | **T1** (unit, `run.rs` tests): `analyzer_diagnostics_of_run` over a hand-built `RunOutput` and a `Model` with one entry in each source returns them in the order (1) workspace, (2) dependency, (3) summarize, (5) overlay, (6) detect.<br>**T3** (source check, `run.rs` tests): `include_str!("run.rs")`, cut out the body of `fn run_analyze_with_exit`, and assert it calls `analyzer_diagnostics_of_run(` and contains no `compute_workspace_diagnostics(` nor `dependency_diagnostics(`. The behaviour cannot tell the inline block from the shared call (the output is identical), so the call itself is pinned. Both analyze golden families stay byte-identical. |
| dependency diagnostics appear in all six commands' diagnostics for the reproducer | **T2** (cli, binary). Two fixtures in tempdirs:<br>(a) **the issue's reproducer**: `tests/r0-corpus/ws-codeunit-rec` copied, its `app.json` dependency on `CodeunitRecLibA` (`eeee5555-…`) set to exactly `2.0.0.0`, which is numeric so `below_declared_version` compares it (`full.rs:~1945-1987`). Its `.alpackages` holds `1.0.0.0`, giving `… is present only at version 1.0.0.0`;<br>(b) `tests/r0-corpus/ws-preflight-missingdep`, giving `… is missing`.<br>EVERY row (analyze included) asserts the EXACT expected dependency message for its fixture is present, and then that the command's dependency-diagnostic messages equal analyze's in the SAME mode. The exact-message check is what fails when a shared break removes the warning from analyze too; equality alone cannot. Rows:<br>- single-app: `fingerprint <ws> --format json` (the issue's command; the B0 envelope path, no query flags) and `prove <ws> <id> may-commit` (JSON by default, `<id>` a stable id from the fixture), against `analyze --single-app --format json`;<br>- cross-app defaults: `digest <ws> --file <a workspace-RELATIVE .al path: src/Item.Table.al for (a), src/caller.al for (b)> --format json` (digest needs changed-root input, `alsem.rs:~1045-1059`; a rooted path matches nothing; assert the JSON envelope was written, not a particular exit code), `events fanout`, `events chains` and `policy check`, against `analyze --format json`. events and policy are read on stderr.<br>**T4** (diff, unit in `diff/cli.rs` tests): `load_snapshot_from_workspace(<fixture a>, …, strict = true)` returns the `warning: dependency … is present only at version 1.0.0.0` line. diff prints analyzer lines only on a failing gate, so the loader is the observable point. |
| a test pins it with a discrimination proof | Breaks, each recorded separately: (i) drop (2) from `analyzer_diagnostics_of_run`, so T1, every T2 row (on its exact-message check) and T4 fail; (ii) restore analyze's inline block, so T3 fails; (iii) give the model's field an empty Vec in `assemble_and_resolve_workspace_program`, so T2's single-app rows and T4 fail; (iv) the same in the cross-app attach, so T2's cross-app rows fail on their exact-message check (default analyze uses that attach too, so equality alone would still pass). |

## Discrimination proofs

The implementer (opus) made each break with Edit and reverted it with Edit, then touched the files. Before the fix, the RED run gave T2's non-analyze rows failing on their exact-message check (and the analyze rows passing), T3 and T4 failing, and T1 failing to compile until `Model` gained the field (it was written last).

| # | break | decisive failures | after revert |
|---|---|---|---|
| i | `analyzer_diagnostics_of_run`: the (2) `extend` commented out | T1 `run.rs:1158 left: ["discover","summarize","overlay","detect"] right: [...,"dependencies",...]`; T4 `diff/cli.rs:370 got []`; T2 all 7 rows, including analyze `:164 got []` and the rest on `:144` | pass |
| ii | `run_analyze_with_exit` gets its inline block back (the call removed) | T3 `run.rs:1193 run_analyze_with_exit must call analyzer_diagnostics_of_run`; T1, T2 and T4 green (the output is identical, as predicted) | pass |
| iii | `assemble_and_resolve_workspace_program`: field forced to `Vec::new()` | T4; T2 fingerprint and prove `:144 missing`; the cross-app rows and analyze green | pass |
| iv | cross-app attach: field forced to `Vec::new()` | T2 digest, events fanout, events chains, policy check `:144`, and default analyze `:164 got []`; fingerprint and prove green | pass |
| c4 | (final-panel round 1, sol C4) the field emptied in `assemble_and_resolve_workspace_program` | `the_inline_program_model_is_the_disk_model` fails: `program_calls.rs:1979 …ws-d1-dep-terminal: declares dependencies with no packages, so the disk model must warn` | pass |

Final: lib 3/3 new tests, `cli_dependency_diagnostics` 7/7.

## Findings register summary

27 entries, every one accepted by both sol and opus.
- 24 fixed, including every critical and important entry (S2, S3, S4, S5, S6, N1).
- 2 refuted: C2 (the `expect` documents a local invariant; the alternative re-clones) and C9 (the source check is required, since behaviour cannot tell the two forms apart).
- 1 deferred: C3, minor, pre-existing, filed as a discovery.

## Gate results

Final, at H = `5c7de0b360bc05aa96a88763393740b3cbdc8f08`. Every gate ran with `supervised: true`; logs are under `.agent/runs/20261016-162024-49d4b6/logs/`.

| gate | exit | log | tests |
|---|---|---|---|
| ci-steps-all | 0 | `ci-steps-all/20261016T174936-fd17caff.log` | 3293 passed, 0 failed |
| check-goldens-coverage | 0 | `check-goldens-coverage/20261016T180043-520aefca.log` | coverage OK |
| check-goldens | 0 | `check-goldens/20261016T180043-82660496.log` | 801 passed, 0 failed |
| cdo-gate | 0 | `cdo-gate/20261016T180146-f070718e.log` | 2254 passed, 0 failed; ratchets green |

- Earlier rounds: at `32f27f6c` (round 1) and `7d53e6ed` (round 2), all four gates exited 0.
- After every round, `git status --porcelain -- . ':!.agent'` was empty.
- Golden regen: `check-goldens-regen` exit 0 (`check-goldens-regen/20261016T170021-01e7d37a.log`). It moved exactly the 3 triaged events stderr goldens.
- Toolchain: rustc 1.96.0. Grammar: tree-sitter-al `8682cb5f`. CDO_WS identity hash: `f1d20e6aac6f8b6e`.

## Discoveries

```json
[
  {"subsystem": "gate / analyzer diagnostics (paths in messages)", "locator": "src/program/resolve/full.rs (unidentified_packages, u.path.display()) via FreshCoverage::dependency_diagnostics; src/engine/gate/workspace_diagnostics.rs:52-64 (multi-app message)", "symptom": "Analyzer diagnostics embed ABSOLUTE host paths (an unreadable dependency package, the multi-app app.json list). Since #91 the dependency ones also reach fingerprint/digest/prove JSON envelopes and events/policy stderr, so --deterministic output differs across machines and checkout directories whenever such a diagnostic fires.", "kind": "bug", "origin_issue": 91, "reproducer": "Put a corrupt .app in <ws>/.alpackages and run alsem fingerprint <ws> --format json --deterministic from two different checkout paths -> the envelopes' diagnostics differ only by the absolute path.", "pre_existing": true, "capability": "analyzer diagnostic messages name workspace-relative paths, so deterministic output is host-independent", "acceptance": "the unreadable-package and multi-app messages print paths relative to the workspace root; a test runs one fixture from two temp roots and gets byte-identical deterministic envelopes."}
]
```

## Not delivered
