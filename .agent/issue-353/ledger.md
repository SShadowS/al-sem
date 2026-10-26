# #353 ledger

## Claim

```json
{"run_id":"20261025-233405-5ebca9","issue":353,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/353-agent-discovery-digest-changed--a1","worktree":"/work/al-sem-issue-353-a1","body_hash":"420771e6a279e146923572dcf1f91623e3db6b193e2d3fbb7c03163857f7f603","reconciled":[]}
```

- Base (make-worktree): `26d1428c6568285b1ab105087d7c2eeaefbc09d4`.
- Reviewers: gpt-6.1-sol (astra slot), local Opus (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.
- Queue: alsem-main put this ahead of #339.
- Merge hold: do not merge before #302 (alsem-second) lands.

## Classification

**Bounded.** The change is one classification step in `resolve_changed_roots`'s diff branch (`src/engine/l5/digest_cli.rs`). It adds one changed-roots diagnostic kind, with its JSON and human projections, and bumps the schema minor version. Roots, entries and effects are untouched.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1: every non-AL diff path becomes `diff-file-unmatched` today | confirmed | Three paths lead there:<br>- `resolve_changed_roots`'s diff loop (`digest_cli.rs:626-639`) sends every non-deleted record to `diff_key`, which returns `DiffFileUnmatched` unless a reading names a workspace `.al` file with routines (`:345-368`).<br>- `Only in` lines go through `diff_key` too (`:668-676`).<br>- A deleted record becomes `DiffFileDeleted` whatever its extension (`:627-631`).<br>`parse_unified_diff` filters on no extension. |
| P2: the workspace file keys hold only `.al` files | confirmed | `workspace_file_keys(input.workspace_files)` is built from `WorkspaceInputs::al_files` (`ChangedInput::workspace_files` doc, `:231-233`). So a non-AL path never matches, and `diff-file-ambiguous` can never fire on one. |
| P3: CentralGauge impact (from alsem-main, who read CentralGauge's source) | low | `analyze-campaign.ts:1331` counts `kind == "diff-file-unmatched"` as `unmatched_files` raw data, not a reported column. The patch it digests is workspace-root relative (`:1337`). `kind` is checked only to be a string (`:424`), so a new kind is safe. Its `ALSEM_SCHEMAS` pins schema versions (CHANGELOG #77 entry, line 121). |
| P4: what a schema bump moves | 22 goldens + 3 tests | 21 `tests/cli-b-goldens/digest/*` goldens and `tests/gate-goldens/batch/one-job` carry `"schemaVersion": "1.7.0"`. Literal asserts are at `cli_digest_query_dependencies.rs:175`, `cli_digest_temp_state.rs:277-278` and `cli_digest_changed_spans.rs:1863`. The #77 bump moved the same set. |
| P5: no committed golden has a non-AL diff path | confirmed | The changed-roots diagnostics in the digest goldens are all `file-unmatched src/Rec.Table.al` (#340 P3). The diff-path tests in `cli_digest_changed_spans.rs` use `.al` paths, apart from the `/dev/null<TAB>…` mutation notes (`:2487`, `:2523`), whose tests assert no such diagnostic. |

## Design

**Rule.** A diff path is AL-looking when `crate::source_text::has_al_extension(Path::new(path))` holds (`src/source_text.rs:34-38`). That is the same test workspace discovery uses, so no second helper is written. A record that is NOT AL-looking makes no roots and gives one diagnostic, `DiffFileNotAl { file }`:
- JSON `{kind: "diff-file-not-al", file}`;
- human line `changed-roots: diff-file-not-al <file>`, escaped as in #340.

That is one diagnostic from the classification. A non-AL file whose hunk header was rejected also keeps its `diff-parse-error` from `file_errors`, as today.

**Where the check runs.** Two guards share the helper.
1. At the top of the `parsed.files` loop (`digest_cli.rs:626`), before both the deleted branch and `diff_key`. It decides on the record's path: for a deletion that is the `---` path, as today's `DiffFileDeleted.file`; otherwise the NEW path.
   - **Rename AL → non-AL** (`old_path` AL-looking, new path not, e.g. `Foo.al` → `Foo.al.bak`): AL source left the workspace, so it gives `DiffFileDeleted { file: <old_path> }` instead of `diff-file-not-al`.
   - **Rename non-AL → AL:** the new path is AL-looking, so it is handled exactly as today.
2. Before `diff_key` in the `Only in` loop (`:668`). An `Only in` path carries no file-or-folder information: `diff -r` without `-N` writes a new folder as `Only in ./src: NewFeature` (`diff_parser.rs:773-779`). So an `Only in` path is `diff-file-not-al` only when it is not AL-looking AND no workspace file key lies below it under any reading (prefix `<reading>/`). The readings are built the way `diff_readings` builds them (`:329-334`), but taken BEFORE its exact-file filter (`:335-340`, the `filter` at `:338`), because a folder never passes that filter. A folder holding workspace AL files keeps today's handling (`diff-file-unmatched`, its roots lost as before). That is stated as a limit, not fixed here.

`diff_key` itself is unchanged.

**Meanings after the change.**
- `diff-file-unmatched`: an AL-looking path that resolves no routine-bearing workspace file. That covers a path the workspace does not have, and a workspace file with no routine (`:343-368`). It also covers an `Only in` folder of workspace files.
- `diff-file-deleted`: a deleted AL-looking file, or the old side of an AL → non-AL rename.
- `diff-file-ambiguous`: unchanged (P2).

**Stated limits.**
- The rule sees paths only, so it classifies AL-LOOKING paths, not proven files. An `Only in` folder named `x.al` reads as AL (`diff-file-unmatched`), and the old tree is not probed, because its paths may not exist on disk. So a folder only the OLD tree has (`Only in ../old/src: Feature`), with nothing in the workspace below it, reads as `diff-file-not-al`, even if the AL files it held were deleted.
- Binary and mode-only `diff --git` sections produce no record today (`diff_parser.rs:712-737`), so they get no diagnostic either way. The rationale is "no parsed record is dropped", not "every touched file is reported".

**Not affected.**
- `--file`/`--changed-files` inputs (`FileUnmatched`) are user-named and stay as they are.
- `diff_key`, roots, entries and effects for AL records are unchanged.
- Exit codes are unchanged: a diff of only non-AL files has no roots and exits 2.

**Why a kind, not a drop.** A drop would hide a parsed record. The code says `Only in` paths are "never dropped silently" (`:666-667`), and the #77 contract makes `kind` an open set (`:1787-1790`). With a distinct kind, a consumer can count unmatched AL files (`diff-file-unmatched`) and still see what else the diff touched. Both reviewers chose this.

**Schema: 1.8.0.** It adds a kind, and it narrows two existing kinds, so the same input under the same version string would change CentralGauge's `unmatched_files` count. Both reviewers chose the bump over the fallback.
- The contract comment at `:1786-1790` gets a 1.8.0 line, including the AL-only meanings.
- The 22 goldens and 3 literal asserts in P4 move by that one field, and the `(current: 1.7.0, #77)` comment at `cli_digest_changed_spans.rs:1862` moves with it.

**Rollout.** Merging to master releases nothing. The 2026-10-27 demo runs on the released v2.1.1, which still ships digest 1.7.0. A release that ships 1.8.0 waits until CentralGauge's `ALSEM_SCHEMAS` lists 1.8.0. Discovery D1 tracks that. The operator controls release tags, and the CHANGELOG entry says this. The fallback (keep 1.7.0) is not taken.

**DRY.** `kind()`, `changed_roots_diagnostic_to_value` and `changed_roots_diagnostic_human` each get the new arm. Their matches are exhaustive, so the compiler forces all three, and `kind()` stays the one spelling.

**CHANGELOG** (`[Unreleased]`):
- `### Changed`: digest `schemaVersion` 1.8.0. Consumers that pin 1.7.0 (CentralGauge `ALSEM_SCHEMAS`) must add it before a release ships it.
- `### Fixed`: non-AL diff files are `diff-file-not-al`, no longer `diff-file-unmatched`. This narrows the #111 entry's "reported as `diff-file-unmatched`" to AL-looking paths.

## Acceptance matrix

| # | Acceptance item (from #353) | Proof |
|---|-----------------------------|-------|
| A1 | A diff touching `app.json`, a `.xlf` and one `.al` routine gives the routine's root and a distinct kind, not `diff-file-unmatched`, for the non-AL files | CLI test `non_al_diff_files_are_their_own_kind` (`cli_digest_changed_spans.rs`), on `ws-d8-commit-in-tx`. The `diff --git` diff holds, in order: the `D8_POSTING_HUNK` change, then hunks on `app.json`, `Translations/x.g.xlf`, `permissions.xml` and `LICENSE` (no extension). Asserts:<br>- roots `["PostSalesDoc"]`, and `entries` equal to the run with the AL hunk alone;<br>- `changedRootsDiagnostics` exactly four `diff-file-not-al`, in diff order, on those four paths;<br>- exit 0;<br>- the same diff with `--format human`: stdout carries exactly those four `changed-roots: diff-file-not-al <file>` lines, in order.<br>Discrimination: make the guard always true (AL) → four `diff-file-unmatched`. |
| A2 | An unmatched `.al` path still reports `diff-file-unmatched` | A separate test, `al_looking_paths_stay_unmatched`, uses hunks on `src/Missing.al` and `src/Other.Al` (mixed case, no workspace file). It asserts exactly two `diff-file-unmatched`, in order, and exit 2. Discrimination: make `has_al_extension` case-sensitive, so `Other.Al` becomes `diff-file-not-al`.<br>Existing tests `only_in_path_outside_the_workspace_stays_unmatched` and every `assert_unmatched` user stay green unchanged. |
| A3 | Deletions, renames and `Only in` follow the rule | Each case below is its own CLI test with an exact diagnostics list, and each has a mutation that drops only that branch's check:<br>- (a) `app.json` deleted (`+++ /dev/null`) → `diff-file-not-al` on `app.json`. The existing `plain_diff_u_timestamped_dev_null_is_a_deletion` (`:2490`) keeps a deleted `.al` as `diff-file-deleted`.<br>- (b) `Only in ./: app.json` → `diff-file-not-al` on `./app.json`.<br>- (c) `Only in .: src`, a folder holding workspace files → `diff-file-unmatched` on `./src`, as today.<br>- (d) rename `src/posting.al` → `src/posting.json` with a hunk → `diff-file-deleted` on `src/posting.al`, no roots.<br>- (e) hunkless rename `src/old.json` → `src/posting.al` (an existing workspace file) → whole-file roots of `src/posting.al`, no diagnostic. |
| A4 | Invariants kept | Each is a CLI case:<br>- (a) a diff of only `app.json` → no entries, exit 2, one `diff-file-not-al`;<br>- (b) `app.json` with a rejected hunk header → its `diff-parse-error` AND `diff-file-not-al`;<br>- (c) `--file app.json` → `file-unmatched` as today.<br>The #340 unit test `changed_roots_human_lines` gains the variant, so it now asserts 12 lines. That proves the rendering; A1's human assertion proves the use. |
| A5 | The schema bump is complete | The 22 goldens move by exactly the `schemaVersion` line, confirmed by a golden-diff-triager. The 3 literal asserts and the `:1862` comment are updated. `check-goldens` is green. |

## Timeline

- 2026-10-25 23:35: claim ok (attempt 1). make-worktree ok, base 26d1428c. Loop 4/15.
- Classified bounded; probes P1–P5; Design written.
- Spec r1 (spec_rounds 1/5): both reviewers chose the new kind and the 1.8.0 bump; both approve with fixes. Merged into T1–T12. Design and matrix revised: has_al_extension reused; Only-in folder rule; AL→non-AL rename gives diff-file-deleted; rollout gated on CentralGauge.
- Spec r2 (2/5): both accepted T1–T12. Opus added two minors (T13 wording, T14 plan note), folded in.
- Spec r3 (3/5): both accepted T3, T9, T13 and T14. Opus N3 (T15): line citations from the wrong base, corrected.
- Spec r4 (4/5): both accepted T3, T14 and T15; both approve. **Converged**: 15 entries.
- Impl Step 1: wrote 8 CLI tests (A1-A4) and the `changed_roots_human_lines` variant (12 lines).
- Impl Step 2 red: A1, A3a, A3b, A3d and A4 fail on the old kinds; A2, A3c and A3e pass already (they pin unchanged behaviour); the unit test fails to compile (no `DiffFileNotAl`).
- Impl Step 3 green: variant, three projections, the two guards, schema 1.8.0, the 3 literal asserts. Unit test 1/1 passes; cli digest tests 111/111 pass.
- Impl Step 4: `check-goldens` (no regen) exit 100, 3 failing tests, all on `schemaVersion` 1.7.0 -> 1.8.0. Golden list sent to the conductor; waiting for the triage receipt.
- Impl Step 5: proofs P1-P6, each broken=FAIL and restored=PASS; every mutation is reverted (grep finds none).
- Impl Step 4 (after the receipt): `check-goldens --regen` changed exactly the 22 files, 1 line each (schemaVersion); `check-goldens` green, 1316/1316.
- Impl Step 6: `task-affected` exit 0 (fmt, clippy-dev, gen-syntax, whole suite 4488/4488). check-diff exit 1: it flags only `.agent/issue-302/{findings.json,ledger.md}`, which come from #302 on origin/master 506149da (this branch is not rebased; the three-dot diff holds only this change's 28 files). Committed dd4eb1e2.
- The conductor rebased the branch onto 506149da (#302); HEAD is 81df6255.
- Final panel r1 fixes H1-H8: the copy needs kind Renamed; the diff-root prefix/parent folder counts; the `unknown` placeholder stays deleted; the A3e proof; a once-built folder set; the contract comment; CHANGELOG.
- New tests H1, H2 and H3 were written with the fixes. Each proof's broken run is the old behaviour, which serves as the red.
- Proofs H1, H2, H3 and A3e: each broken=FAIL and restored=PASS.
- changed_spans 93/93. task-affected exit 0 (whole suite 4496/4496). No golden moved. check-diff exit 0.
- Committed 844c7100.
- Conductor:
  - Rebased dd4eb1e2 onto 506149da (#302) before the panel: clean, now 81df6255. Task gate 4493/4493. Draft PR #357.
  - Golden triage: an independent golden-diff-triager confirmed 22 moves, each by the schemaVersion line only. Receipt written, then the implementer regenerated the goldens.
- Final panel r1 (final_rounds 1/5):
  - sol: approve with fixes, 2 important and 1 minor;
  - opus: approve with fixes, 6 minor (F1 and F2 are sol's two important findings);
  - code-review (high, built-in skill): 7 findings.
  - Merged into U1–U9. U8 and U9 are refuted as the ratified T13 and T12; the rest are fixed in 844c7100.
- Final panel r2 (2/5): both accepted U1–U9 and T12. T12 was re-marked because its anchor hash went stale after spec r4 edited line 40. Both approve. **Converged**: 24 entries.
- Full gates on H = 844c7100: all supervised, exit 0, queued 0 s. Peer alsem-second paused its probes for cdo-gate. origin/master unchanged (506149da).

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| `non_al_diff_files_are_their_own_kind` (A1) | guard 1 `if !al_looking(&diff_file.path)` -> `if !(true \|\| al_looking(..))` (count 1) | FAIL: four `diff-file-unmatched` (app.json, Translations/x.g.xlf, permissions.xml, LICENSE) | PASS after revert | 81df6255 (rebased dd4eb1e2) |
| `al_looking_paths_stay_unmatched` (A2) | `has_al_extension`: `e.eq_ignore_ascii_case("al")` -> `e == "al"` (count 1) | FAIL: `src/Other.Al` is `diff-file-not-al` | PASS after revert; `git diff src/source_text.rs` empty | 81df6255 (rebased dd4eb1e2) |
| `deleted_non_al_file_is_not_al` (A3a) | guard 1 gets `diff_file.kind != DiffFileKind::Deleted &&` (count 1) | FAIL: `app.json` is `diff-file-deleted` | PASS after revert | 81df6255 (rebased dd4eb1e2) |
| `only_in_non_al_file_is_not_al` (A3b) | Only-in guard gets `false &&` (count 1) | FAIL: `./app.json` is `diff-file-unmatched` | PASS after revert | 81df6255 (rebased dd4eb1e2) |
| `only_in_workspace_folder_stays_unmatched` (A3c) | Only-in guard drops `&& !holds_workspace_files(..)` (count 1) | FAIL: `./src` is `diff-file-not-al` | PASS after revert | 81df6255 (rebased dd4eb1e2) |
| `rename_from_al_to_non_al_is_a_deletion` (A3d) | rename arm `Some(old) if al_looking(old)` -> `if false && al_looking(old)` (count 1) | FAIL: `src/posting.json` is `diff-file-not-al` | PASS after revert | 81df6255 (rebased dd4eb1e2) |
| `copy_from_al_to_non_al_is_not_al` (H1) | rename arm drops `diff_file.kind == DiffFileKind::Renamed &&` (the kind check's count went 1 -> 0) | FAIL: `src/posting.al` is `diff-file-deleted` (A3d still passes) | PASS after revert | 844c7100 |
| `only_in_diff_root_prefix_folder_stays_unmatched` (H2) | `\|\| (any_file && at_or_above_prefix(..))` -> `\|\| (false && any_file && ..)` (count 1) | FAIL: `./Cloud` and `./repo-sibling-folder` both `diff-file-not-al` (A3c still passes). The test stops at its first assert, so the `./apps` parent case goes through the same branch but has no proof of its own | PASS after revert | 844c7100 |
| `dev_null_both_sides_stays_deleted` (H3) | `if !placeholder &&` -> `if !(placeholder && false) &&` (count 1) | FAIL: `unknown` is `diff-file-not-al` | PASS after revert | 844c7100 |
| `rename_from_non_al_to_al_takes_the_new_file` (A3e, H4) | guard 1 classifies `diff_file.old_path.as_deref().unwrap_or(&diff_file.path)` | FAIL: `diff-file-not-al`, no roots. The diagnostic names `src/posting.al`, not `src/old.json` as predicted, because the arm reports `diff_file.path` | PASS after revert | 844c7100 |

## Findings register summary

There are 24 entries, all accepted by sol (astra slot) and opus (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.
- Spec T1–T15: all fixed.
- Final U1–U7: fixed.
- Final U8 and U9: refuted.

## Base/Head SHAs

B = `506149dafc20ae5ab21619b1ccb5804faab2d6ee`. H = `844c710013c8abbf2699e04e306744a7b115d710`.

## Gate results

| gate | exit | log |
|------|------|-----|
| ci-steps-all | 0 | `.agent/runs/20261025-233405-5ebca9/logs/ci-steps-all/20261026T002212-e5b7e761.log` |
| check-goldens-coverage | 0 | `.agent/runs/20261025-233405-5ebca9/logs/check-goldens-coverage/20261026T002729-ffa59237.log` |
| check-goldens | 0 | `.agent/runs/20261025-233405-5ebca9/logs/check-goldens/20261026T002729-dad62575.log` |
| cdo-gate | 0 | `.agent/runs/20261025-233405-5ebca9/logs/cdo-gate/20261026T002740-bfbcddb8.log` (ENFORCE_CDO_WS=1) |

- Toolchain: rustc 1.96.0.
- Grammar: tree-sitter-al v4.5.0 `91a5db62`.
- CDO_WS: the pinned baseline, enforced by the gate's closure stamp.
- Goldens: the only moves are the 22 schemaVersion lines, triaged and regenerated.

## Discoveries

```json
[
  {
    "subsystem": "CentralGauge digest consumer (schema pin)",
    "locator": "CentralGauge ALSEM_SCHEMAS; al-sem src/engine/l5/digest_cli.rs schemaVersion 1.8.0",
    "symptom": "al-sem digest moves to schemaVersion 1.8.0 (#353: the new diff-file-not-al changed-roots kind; diff-file-unmatched and diff-file-deleted now cover AL paths only). A consumer pinning 1.7.0 rejects 1.8.0 documents.",
    "kind": "enhancement",
    "origin_issue": 353,
    "reproducer": "alsem digest <ws> --diff <patch> on master after #353: schemaVersion is 1.8.0.",
    "pre_existing": false,
    "capability": "CentralGauge accepts digest 1.8.0 and keeps counting unmatched_files from diff-file-unmatched (now AL-only)",
    "acceptance": "CentralGauge's ALSEM_SCHEMAS lists 1.8.0; a 1.8.0 digest of a patch touching app.json reports unmatched_files 0 for it"
  }
]
```

## Not delivered
