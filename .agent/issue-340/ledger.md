# #340 ledger

## Claim

```json
{"run_id":"20261025-223613-b641af","issue":340,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/340-agent-discovery-digest-human-fo-a1","worktree":"/work/al-sem-issue-340-a1","body_hash":"d665125cb7e79408734e7a2da78e25357ddcf58daf0da95b20448f7450c97472","reconciled":[]}
```

Base (make-worktree): `6f1e2718a62e8a55eb3eaf3bb14534d58487fc6d`. Reviewers: gpt-6.1-sol (astra slot) and a local Opus (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Bounded.** The change is one output formatter in `src/engine/l5/digest_cli.rs`. It adds no new data: every diagnostic is already computed and already in the JSON. Its consumers are the human goldens, which are ours.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1: human output drops changed-roots diagnostics | confirmed | `format_digest_human(entries)` (`digest_cli.rs:1739`) takes the entries only. `digest_view` (`:2090`) builds the human text from it at `:2180`. `changed_roots.diagnostics` reaches only `project_digest_document`, which builds the JSON. |
| P2: the variants a line must render | 8 | `ChangedRootsDiagnostic` (`digest_cli.rs:182-216`): FileUnmatched, SelectorUnmatched, SelectorAmbiguous, DiffFileUnmatched, DiffFileAmbiguous, HunkOutsideRoutines, DiffFileDeleted, DiffParseError. JSON keys: `:1528-1580`. HunkOutsideRoutines' numbers are written as `*start_line as u64` (`:1566`). |
| P3: which committed goldens move | 6 human goldens, one added line each | Each of these `tests/cli-b-goldens/digest/*.json` carries exactly one `changedRootsDiagnostics` entry, `{"file":"src/Rec.Table.al","kind":"file-unmatched"}`: `ws-txn-d47-neg-temp`, `ws-txn-d47-pos-file`, `ws-txn-d47-pos-http-commit-after`, `ws-txn-d47-pos-http-nocommit`, `ws-txn-d49-pos-modify-message`, `ws-txn-d49-pos-modify-runmodal`. Their `.human.txt` twins gain that one line. No other digest golden carries a diagnostic. The human goldens run through `run_digest_pipeline`, so they pin the production call site. |
| P4: the ambiguous fixture exists | yes | `tests/cli/cli_digest_changed_spans.rs` `ambiguous_ws` (#77). With the diff path `Cloud/src/posting.al` and the default git-top reading, `path_naming_two_files_is_ambiguous` (`:2733-2744`) gets candidates `["ws:Cloud/src/posting.al", "ws:src/posting.al"]`, no roots, exit 2. |
| P5: the CLI test helper forces JSON | yes | `digest_raw` (`cli_digest_changed_spans.rs:91-110`) always appends `--deterministic --format json` after the caller's args. |
| P6: a diagnostic's strings can hold a line break | yes | `diff_parser.rs:140-142` decodes C-quoted `\n`, `\t`, `\r` in diff file names. |
| P7: today's trailing bytes | read | With entries, every entry block ends with an empty line, and the `join("\n")` gives text that ends in exactly one `\n`, with no blank line after it. With no entries, the text is `digest: no changed roots resolved\n`. `digest_view` then appends the `--tests` line plus `\n` (`:2181-2184`). |

## Design

**Placement.** `format_digest_human` becomes `format_digest_human(entries: &[DigestEntryFull], changed: &[ChangedRootsDiagnostic]) -> String`. Its one production caller is `digest_view` (`:2180`), which passes `&changed_roots.diagnostics`; its one unit-test caller is at `:2545-2558`. The text is built in this order:
- the entry blocks as today, or `digest: no changed roots resolved\n` when there are none;
- then, with NO blank line between, one line per changed-roots diagnostic, each ending in `\n`, in the order of `changed_roots.diagnostics` (the JSON array's order).

So the `--tests` line `digest_view` appends stays a separate last line. With no diagnostics, the output is byte-identical to today's.

**Line form.** `changed-roots: <kind> <subject>[ <detail>]`, where `<kind>` is the JSON `kind` string:

| variant | line |
|---------|------|
| FileUnmatched | `changed-roots: file-unmatched <file>` |
| SelectorUnmatched | `changed-roots: selector-unmatched <selector>` |
| SelectorAmbiguous | `changed-roots: selector-ambiguous <selector> (candidates: <c1>, <c2>, …)` |
| DiffFileUnmatched | `changed-roots: diff-file-unmatched <file>` |
| DiffFileAmbiguous | `changed-roots: diff-file-ambiguous <file> (candidates: <c1>, <c2>); pass --diff-root <folder> to read diff paths relative to <folder>`. The diagnostic arises only under the default git-top reading: an explicit root gives at most one reading (`digest_cli.rs:311-317`), and ambiguity needs two (`:345`). The text names no folder because either reading may be the one meant, and `--diff-root <folder>` settles both. |
| HunkOutsideRoutines | `changed-roots: hunk-outside-routines <file>:<start>-<end>` (the same `as u64` values the JSON writes) |
| DiffFileDeleted | `changed-roots: diff-file-deleted <file>` |
| DiffParseError | `changed-roots: diff-parse-error <file>: <detail>`, or `changed-roots: diff-parse-error <detail>` with no file |

The candidates are listed in the order the JSON gives them.

**Escaping.** Every interpolated string (file, selector, candidate, detail) goes through one helper. It replaces each `char::is_control` character with its `char::escape_default` form (`\n`, `\t`, `\r`, `\u{1b}`, …) and leaves every other character as it is. So one diagnostic is always one line, and ordinary paths, including the six goldens' `src/Rec.Table.al`, print unchanged.

**One table.** The table lives in one private exhaustive `match`, `fn changed_roots_diagnostic_human(d: &ChangedRootsDiagnostic) -> String`, next to `changed_roots_diagnostic_to_value`. Being exhaustive, a new variant fails to compile until it has a line. The candidate list and the parse-error file/no-file split stay inside that one function. The kind strings move into one `fn kind(&self) -> &'static str` on the enum, which both projections call, so the JSON and the human text cannot drift (DRY). The JSON bytes stay the same: `serde_json::Map` sorts its keys whatever order they are inserted in.

**Not changed.** The JSON document, the exit codes, the `dependencies:` prefix line, the `--tests` line (still last), the batch path's JSON, and the query-level diagnostics (out of scope; see Discoveries D1).

**Goldens.** The six human goldens in P3 move by exactly one line each: `changed-roots: file-unmatched src/Rec.Table.al`, added after the last entry block. A `golden-diff-triager` confirms that is the whole diff before `scripts/check-goldens --regen` runs. Any other moved line is unexplained and stops the regen.

**CHANGELOG.** `[Unreleased]` `### Fixed`: human digest output now lists the changed-roots diagnostics. Before, a human run could exit 0 while a diff file or `--file` went unresolved, and print nothing about it.

**Test helper.** `digest_raw` in `cli_digest_changed_spans.rs` gets a format parameter (`digest_raw_as(ws, args, diff, format)`). The existing `digest_raw` keeps `json` and calls it, so no existing test changes.

## Acceptance matrix

| # | Acceptance item | Proof |
|---|-----------------|-------|
| A1 | A human run with an ambiguous file prints one diagnostic line naming the file, its candidates and `--diff-root` | New CLI test `human_output_names_the_ambiguous_file` (`cli_digest_changed_spans.rs`). It uses `ambiguous_ws` and P4's diff, through `digest_raw_as(..., "human")`. It asserts:<br>- exit 2;<br>- stdout equals exactly `digest: no changed roots resolved\nchanged-roots: diff-file-ambiguous Cloud/src/posting.al (candidates: ws:Cloud/src/posting.al, ws:src/posting.al); pass --diff-root <folder> to read diff paths relative to <folder>\n`.<br>Discrimination: the CALL-SITE mutation, `format_digest_human(&entries, &[])` in `digest_view`, fails A1, while A3 stays green under it. |
| A2 | The JSON is unchanged | A1's test also runs `digest_raw_as(..., "json")` on the identical workspace, diff and flags (a separate invocation), and asserts `changedRootsDiagnostics == [ambiguous(["ws:Cloud/src/posting.al", "ws:src/posting.al"])]`. Every JSON golden stays byte-identical: `check-goldens` passes, and the regen moves no `.json` file. |
| A3 | Every variant has its line, escaped, and the trailing bytes are right | Unit test `changed_roots_human_lines` in `digest_cli.rs`. It hand-states one diagnostic per variant (8, DiffParseError both with and without a file), plus one FileUnmatched whose file holds `\n` and `\t`. It asserts the whole output with `assert_eq!` through `format_digest_human`, the production entry point, in two cases:<br>- (a) with no entries: `digest: no changed roots resolved\n` followed by the lines in input order, the control characters shown as `\n` and `\t`, one line each;<br>- (b) with one hand-built entry: the entry block, then the lines with no blank line between, ending in one `\n`.<br>Discrimination: dropping the escaping fails (a); adding a blank line fails (b). |
| A4 | No diagnostics, no change | The unit test `human_output_unchanged_without_changed_roots_diagnostics` asserts that `format_digest_human(x, &[])` equals today's literal output, with `assert_eq!`, for an empty entry list and for one entry, trailing newline included. Every human golden without diagnostics stays byte-identical. |
| A5 | Assembly with `--tests` and resolved entries | A CLI case in `cli_digest_changed_spans.rs` gives a diff that resolves one routine and also names an unmatched file. It runs with `--format human --tests`, and asserts:<br>- exit 0;<br>- the entry block, then exactly one `changed-roots: diff-file-unmatched <file>` line, then the `--tests` line as a separate last line.<br>The `dependencies:` prefix is covered by the existing `cli_digest_query_dependencies.rs:192` test, which stays green. |

## Timeline

- 2026-10-25 22:36: claim ok (attempt 1). make-worktree ok, base 6f1e2718. Loop 2/15.
- Classified bounded; probes P1–P4; Design written.
- Spec panel r1 (spec_rounds 1/5): both reviewers approve with fixes (sol 6, opus 8 of which F8 is a discovery). Merged into S1–S10. Design and matrix revised; P5–P7 added.
- Spec panel r2 (2/5): both accepted S1–S6 and S8–S10, and both re-raised S7 (minor): its rationale was false, since ambiguity arises only under the git-top reading. Corrected.
- Spec panel r3 (3/5): both accepted S7; both approve. **Converged**: 10 entries accepted by both.
- Impl step 1–2: A4 written against today's 1-arg formatter: ok (pre-change). A3: red = E0061 (2 args to the 1-arg fn), as stated. A1, A5: red, FAILED (no changed-roots line).
- Impl step 3: `kind()`, `changed_roots_diagnostic_human`, `escape_controls`, 2-arg `format_digest_human`; A1, A3, A4, A5 and `human_output_shows_incoherence_reason_and_op` green.
- Impl step 4 triage: check-goldens 1307/1308, the only failure `human_matches_goldens` (+47 bytes = the expected line). All 20 fixtures re-run through the binary: the 14 untouched goldens match byte for byte; exactly the 6 P3 human goldens gain the one line at the end; all 20 JSON identical. Regen held for the guard receipt (conductor writes it after an independent triager).
- Proof 1 (call site) done. NOTE: it was live in the worktree while the conductor's triager ran `cargo test --test cli` (reverted 22:53:31 UTC); the triager must rebuild after that time.
- Proofs 2 (escaping) and 3 (blank line) done, both reverted; A3 green.
- Impl step 4: conductor's receipt after an independent triager; `check-goldens --regen` moved exactly the 6 human goldens (+1 line each, the same line); `check-goldens` green (1308/1308).
- Impl step 6: `ci-steps task-affected` exit 0 (whole suite, 4480/4480); commit 5f32ae75; check-diff on HEAD: `{"reasons": []}`.
- Final panel r1 fixes G1–G4: G1 red (raw U+202E/U+2028 in case (a)), then green; proof recorded; G2 doc scope; G3 `kind` private (only this module calls it); G4 CHANGELOG placement. Targeted unit tests and A1/A5 ok; task-affected exit 0 (4480/4480); commit a0f1157d; check-diff `{"reasons": []}`.
- Final panel r1 (final_rounds 1/5):
  - sol: approve, no findings.
  - opus: approve, 3 optional minors (M1 = G3, M2 = G4, M3 = G9).
  - code-review (high, built-in skill): 8 findings. Fixed G1/G2 (with G3/G4), deferred G5 (D1) and G6 (new D2), refuted G7–G10 against the ratified contract.
- Final panel r2 (2/5): both accepted G1–G10 and S10, and both verdicts were approve. Opus raised N1, a minor CHANGELOG rewrap.
- Full gates on H = a0f1157d: all supervised, exit 0, queued 0 s (peer alsem-second paused its heavy work for cdo-gate). origin/master unchanged at 6f1e2718, so no rebase was needed.
- N1 fixed after H in ac041329 (CHANGELOG only, gate-inert). Raiser-only acceptance: opus accepted; sol `accepted-by-rule`. **Converged**: 21 entries.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| A1 `human_output_names_the_ambiguous_file`, A5 `human_tests_run_lists_the_unmatched_file_before_the_tests_line`, A3 control `changed_roots_human_lines` | call site in `digest_view`: `format_digest_human(&entries, &[])` (applied=1, grep) | A1 FAILED: left `"digest: no changed roots resolved\n"` (no changed-roots line); A5 FAILED: left `"  diagnostic: evidence-unavailable (EVENT_PUBLISH)"` at the changed-roots slot; A3 ok (control, as designed) | reverted (grep: real call 1, mutation 0); A1, A5, A3 ok | 5f32ae75 |
| A3 (a) `changed_roots_human_lines` | `escape_controls`: `out.extend(c.escape_default())` -> `out.push(c)` (grep: escape_default() calls 0; a first Edit that did not apply was caught by this assert and redone) | FAILED at :2713 (case a): left ends `file-unmatched src/a\nb\t.al\n` (raw LF/TAB), right `src/a\\nb\\t.al` | reverted (grep 1); ok | 5f32ae75 |
| A3 (b) `changed_roots_human_lines` | `format_digest_human`: push `'\n'` before the diagnostics when there are entries (grep 1) | FAILED at :2721 (case b): left `(no effects)\n\nchanged-roots: …` (blank line), right `(no effects)\nchanged-roots: …` | reverted (grep 0); ok | 5f32ae75 |
| A3 (a) `changed_roots_human_lines`, G1 bidi/line-separator case | `escape_controls`: drop the new `\|\| matches!(c, U+200E/F, U+202A–E, U+2066–9, U+2028/9)` check (grep: range 0) | FAILED at :2721 (case a): left tail `src\\x\u{202e}y\u{2028}z.al` (raw U+202E, U+2028), right `src\\x\\u{202e}y\\u{2028}z.al` | restored (grep 1); ok | a0f1157d |

## Findings register summary

21 entries, all accepted by sol (astra slot) and opus (flash slot). N1's sol mark is `accepted-by-rule`. Attest with `--substitute astra=sol --substitute flash=opus`.
- Spec S1–S10: all fixed.
- Final G1–G10: G1–G4 fixed, G5 and G6 deferred as discoveries D1 and D2 (minor), G7–G10 refuted.
- Final N1: fixed.

## Base/Head SHAs

B = `6f1e2718a62e8a55eb3eaf3bb14534d58487fc6d`. H = `a0f1157d6fcf1a74a358d342a5c3deb3fce8004e`. After H: `ac041329` (CHANGELOG only), then the evidence commit.

## Gate results

| gate | exit | log |
|------|------|-----|
| ci-steps-all | 0 | `.agent/runs/20261025-223613-b641af/logs/ci-steps-all/20261025T231252-7c07c9cc.log` |
| check-goldens-coverage | 0 | `.agent/runs/20261025-223613-b641af/logs/check-goldens-coverage/20261025T231814-27bd73fb.log` |
| check-goldens | 0 | `.agent/runs/20261025-223613-b641af/logs/check-goldens/20261025T231814-46d80240.log` |
| cdo-gate | 0 | `.agent/runs/20261025-223613-b641af/logs/cdo-gate/20261025T231828-69552d8c.log` (ENFORCE_CDO_WS=1) |

- Toolchain: rustc 1.96.0.
- Grammar: tree-sitter-al v4.5.0 `91a5db62`.
- CDO_WS: the pinned baseline, enforced by the gate's closure stamp.
- Goldens: exactly the 6 human goldens moved, triaged; no JSON golden moved.

## Discoveries

```json
[
  {
    "subsystem": "digest human format (query-level diagnostics)",
    "locator": "src/engine/l5/digest_cli.rs format_digest_human / digest_view; project_query_diagnostic (DIAG-digest-root-not-in-snapshot, DIAG-digest-no-coverage-record)",
    "symptom": "--format human prints no query-level diagnostics: a root dropped as root-not-in-snapshot, or one with no coverage record, leaves no line in the human output while the JSON envelope carries the DIAG-digest-* entry.",
    "kind": "enhancement",
    "origin_issue": 340,
    "reproducer": "a digest run whose root is not in the snapshot: the JSON envelope diagnostics hold DIAG-digest-root-not-in-snapshot; --format human prints nothing about it.",
    "pre_existing": true,
    "capability": "human digest output lists the query-level diagnostics (code and message), as #340 does for the changed-roots diagnostics",
    "acceptance": "a human run with a root not in the snapshot prints one line naming the code and the routine; the JSON is unchanged"
  },
  {
    "subsystem": "digest changed-roots (diff input)",
    "locator": "src/engine/l5/digest_cli.rs diff_key (every unreadable diff path becomes DiffFileUnmatched); src/engine/l5/diff_parser.rs (no .al filter)",
    "symptom": "Every non-AL file in a diff (app.json, .xlf translations, permission-set .xml) yields a diff-file-unmatched changed-roots diagnostic, as if a source file failed to resolve. The JSON has always carried them; since #340 the human output prints one line each, so the expected non-source files bury a real unmatched .al file.",
    "kind": "enhancement",
    "origin_issue": 340,
    "reproducer": "a diff touching app.json and one .al routine: changedRootsDiagnostics holds {kind: diff-file-unmatched, file: app.json}; --format human prints `changed-roots: diff-file-unmatched app.json`.",
    "pre_existing": true,
    "capability": "a diff path that is not an AL source file is reported apart from (or not as) an unresolved source file, so diff-file-unmatched means an .al file the workspace does not have",
    "acceptance": "a diff touching app.json, a .xlf and one .al routine gives the routine's root and no diff-file-unmatched for the two non-AL files (or a distinct, separately counted kind); an unmatched .al path still reports diff-file-unmatched"
  }
]
```

## Not delivered
