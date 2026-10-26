# #339 ledger

## Claim

```json
{"run_id":"20261026-004237-f107b1","issue":339,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/339-agent-discovery-digest-changed--a1","worktree":"/work/al-sem-issue-339-a1","body_hash":"b40da60b8b81033e3a4facfbe9212a1cda75f6f6ed8a5af1c96f7c0ba3fb0d83","reconciled":[]}
```

- Base (make-worktree): `258da61060fda62d4257f0dd1468e6c40c3366d6`.
- Reviewers: gpt-6.1-sol (astra slot) and a local Opus (flash slot).
- Attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Bounded.** The file-input step of `resolve_changed_roots` gets the path readings that #77 gave diff paths. The CLI then decides the path root for file inputs as well as for diffs. One changed-roots kind is added.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1: `--file` matches the workspace-relative key only | confirmed | `resolve_changed_roots` step 1 (`digest_cli.rs:611-617`) looks up `spans.of_file(&normalize_input(file))` and gives `FileUnmatched` when that is empty. No git-top reading exists for it. |
| P2: the path root is decided only when a diff is read | confirmed | `decide_diff_root(d, has_diff)` returns `DiffRoot::Workspace` without probing when `!has_diff` (`alsem.rs:1718-1721`). The batch parent decides it the same way, through `pin_diff_root` → `DigestInputs::of(d).has_diff()` (`:1668`). |
| P3: `--changed a.al,b.al` becomes file inputs | confirmed | `DigestInputs::of` extends `files` from `ChangedAutoDetect::Files` (`alsem.rs:1689-1696`). So one fix in the file step covers `--file`, `--changed-files` and `--changed a.al,…`. |
| P4: #77's readings and match are reusable | confirmed | `diff_readings` (`digest_cli.rs:333-348`, via `path_readings`) gives the as-written reading and, under GitTop, the below-the-top reading, filtered to workspace file keys. `diff_key` (`:403-425`) maps one reading with routines to a key, two readings to ambiguous, and anything else to unmatched. |
| P5: committed goldens do not move | expected | The cli-b and gate digest fixtures live under `tests/r0-corpus/<ws>` inside this repository, whose top level has `.git`, so GitTop applies. A `--file src/x.al` gives the as-written reading `src/x.al`. Its below-top reading strips the prefix `tests/r0-corpus/<ws>`, which `src/x.al` does not start with, so it gives none. That leaves one reading, the same key as today. |
| P6: schema 1.8.0 is unreleased | confirmed | #353 (258da610) set 1.8.0 on master. The last release, v2.1.1, ships 1.7.0. Nothing outside this repository reads a 1.8.0 document yet; #358 tracks CentralGauge adding it. |

## Design

**Scope.** The file inputs are `--file` (repeatable) and the `--changed a.al,b.al` comma list (P3). There is no `--changed-files` flag: that name is legacy wording from the TypeScript port, left in `--changed`'s help (`alsem.rs:689`) and in the no-input error (`:1820`). That wording is filed as discovery D1 and not changed here.

**Shared matcher.** `diff_key`'s match becomes a neutral helper, `fn match_path(spans, files, root, path) -> PathMatch`, with `enum PathMatch { One(String), Ambiguous(Vec<String>), None }`.
- The candidates are built as `diff_key` builds them today: each reading's `ws:` units, sorted and deduplicated within that reading, and concatenated in reading order (as written, then below the top). They are not sorted globally.
- Workspace FILES count, routines or not, so a table-only reading still makes a path ambiguous (#77).
- `diff_key` keeps its signature and maps `Ambiguous`/`None` to `DiffFileAmbiguous`/`DiffFileUnmatched`. Its behaviour does not change.

**File step.** For each file input:
- **scheme-prefixed** (`ws:…`, `app:<guid>:…`): this is a snapshot id, not a filesystem path. It keeps exactly one reading under every root, `normalize_input(file)`, which is today's key. So a `ws:` candidate copied out of a `file-ambiguous` diagnostic picks that one file;
- otherwise `match_path`:
  - `One(key)` → the file's routine spans, as today;
  - `Ambiguous(c)` → `FileAmbiguous { file, candidates: c }`, with no roots from THAT input (other inputs still contribute);
  - `None` → `FileUnmatched { file }`, as today.

`workspace_file_keys(input.workspace_files)` is built once per `resolve_changed_roots` call, above both steps. File roots and diff roots still union by declaration, without duplicates. A file input still contributes whole-file roots when the same file's diff section has a rejected hunk header: the #72 poisoning covers diff-derived roots only (`digest_cli.rs:670-680`).

**New kind.** `FileAmbiguous`:
- JSON: `{kind: "file-ambiguous", file, candidates}`.
- Human: `changed-roots: file-ambiguous <file> (candidates: <c1>, <c2>); pass one candidate as --file, or --diff-root <folder> to read paths relative to <folder>`.

Every interpolated string is escaped as in #340. The kind joins the three exhaustive projections (`kind()`, JSON, human).

**Root decision.**
- `DigestInputs::reads_paths()` is `has_diff() || !files.is_empty()`. `pin_diff_root` (batch) and `run_digest` (standalone) call `decide_diff_root(d, inputs.reads_paths())`. So a file-only run probes for `.git` as a diff run does, and records the probe as a planning read, as diffs do (#77). `has_diff` stays for its other callers.
- **The lone-`--diff-root` check** (`alsem.rs:1811-1816`) is gated on `!reads_paths()`. Its message becomes `al-sem: digest: --diff-root needs a diff or file inputs (--diff, --file or --changed)`, still exit 3. A routine-only or input-free `--diff-root` still exits 3. `lone_diff_root_is_exit_3` updates its message assertion and gains a `--routine` case.
- An explicit `--diff-root` that is a regular file, given with file inputs only, reaches #77's validation and fails with `is not a folder`, exit 3.

**Explicit `--diff-root` with files.** A plain-path file input is read relative to the given folder only, as #77 reads diff paths (`DiffRoot::Explicit` gives one reading). So `--diff d --diff-root <repo> --file src/x.al`, with the workspace at `<repo>/Cloud`, used to resolve and is now `file-unmatched`; the user writes `Cloud/src/x.al` or `ws:src/x.al`. That keeps one rule for every changed path, and it is a behaviour change, listed under CHANGELOG `### Changed`. File-only runs with `--diff-root` used to fail (exit 3) and now work.

**Schema.** 1.8.0 is unreleased (P6). Both reviewers chose to add `file-ambiguous` to it, so the version does not move. The 1.8.0 contract line lists #353 and #339. It says the file side is not purely additive either: a path that resolved (the as-written reading) can now be `file-ambiguous` and give no roots. The pre-stated 1.9.0 fallback is not taken.

**Stated limits** (CHANGELOG, plus discoveries D2 and D3):
- `--changed X` is classified as a diff when `X` exists relative to the CURRENT directory (`digest_cli.rs:146-149`), which predates #339. Run from the git top, `--changed Cloud/src/posting.al` names an existing file, so it is read as diff text. A comma list such as `a.al,b.al` is not affected unless that exact string is a path (D2).
- Absolute paths stay unmatched, as today (D3).
- Prefix folders compare exactly, case included (#77 `same_folder`): `cloud/src/posting.al` under GitTop `Cloud` stays unmatched. The workspace key itself stays case-normalized.

**Not changed:** the diff path handling (#77, #111, #353), routine selectors, exit codes (ambiguity alone gives no roots, so exit 2), `prove`'s changed-roots call (it passes no file inputs and `workspace_files: &[]`, `prove.rs:944`; a one-line doc note says file inputs there would need the workspace files).

**Docs:**
- the `--file` help ("Workspace-relative" becomes "workspace- or git-top-relative, or a `ws:` id");
- the `--changed` and `--diff-root` help, the latter naming both ambiguity kinds;
- the docs of `decide_diff_root`, `ChangedInput.workspace_files`, `workspace_file_keys`, `DiffRoot` and `DigestRequest.diff_root`, which change from "diff paths" to "changed paths".

**CHANGELOG** (`[Unreleased]`):
- `### Fixed`: `--file` and `--changed a.al,…` accept paths relative to the git top level (automatic mode) or to `--diff-root` (explicit mode), like `--diff`. A path naming a workspace file both ways is `file-ambiguous`. A `ws:` id is read as written.
- `### Changed`: the 1.8.0 entry gains the kind. With `--diff-root`, plain file paths are read relative to it, which can turn a resolving `--file src/x.al` into `file-unmatched`. `--diff-root` with only file inputs is now accepted.

## Acceptance matrix

| # | Acceptance item (from #339) | Proof |
|---|-----------------------------|-------|
| A1 | A `Cloud/`-prefixed `--file` resolves the same roots as the unprefixed one | CLI test `git_top_relative_file_equals_workspace_relative`, on `repo_ws` (the d8 workspace at `<t>/repo/Cloud`, with `<t>/repo/.git`). It asserts:<br>- `--file Cloud/src/posting.al`, `--file ./Cloud/src/posting.al`, `--file Cloud/SRC/POSTING.AL` (the workspace key is case-normalized; only the prefix folder compares exactly) and `--file src/posting.al` give equal `entries`;<br>- roots exactly the posting file's routines (the literal list from `src/posting.al`'s declarations, not just one name), with no diagnostics and exit 0;<br>- `--changed Cloud/src/posting.al,Cloud/src/posting.al` (a comma list that is no path) gives the same, with the child's `current_dir` set to `t` (so no such path exists there).<br>Discriminations:<br>- `reads_paths()` returns `has_diff()` only → `file-unmatched`;<br>- the file step uses `normalize_input` only → `file-unmatched`. |
| A2 | A path naming a file both ways is reported ambiguous | CLI test `file_naming_two_files_is_ambiguous`, on `ambiguous_ws`. It asserts:<br>- `--file Cloud/src/posting.al` gives exactly `[{kind:"file-ambiguous", file:"Cloud/src/posting.al", candidates:["ws:Cloud/src/posting.al","ws:src/posting.al"]}]` (exact order), with no roots and exit 2;<br>- the `--format human` stdout is exactly that line plus the `no changed roots resolved` line;<br>- with a second, resolving input, the owner-qualified `--routine D8PostingChain::PostSalesDoc` (a bare `PostSalesDoc` is itself `selector-ambiguous` here, because the copy declares it too), it exits 0 with exactly that routine's root and exactly one diagnostic, the `file-ambiguous`;<br>- with two workspace files whose names differ only in case inside one reading (`src/posting.al` and `src/Posting.al`, on a case-sensitive file system; skipped with a printed note where the file system folds case), the candidates list every unit with its original spelling, in sorted order within the reading;<br>- `--file ws:Cloud/src/posting.al` and `--file ws:src/posting.al` each resolve one file, with no diagnostic.<br>A table-only counterpart of #77's `table_only_reading_still_ambiguous`, with `--file` instead of `--diff`, gives `file-ambiguous`.<br>Discrimination: map `Ambiguous` to the first reading → roots appear and there is no diagnostic. |
| A3 | Explicit `--diff-root`, mixed inputs, no git | CLI tests:<br>- (a) `--diff-root <t>/repo --file Cloud/src/posting.al` resolves. `--diff-root <t>/repo --file src/posting.al` is `file-unmatched` (the stated rule). `--diff-root <t>/repo --file ws:src/posting.al` resolves. The mixed regression (V4): `--diff <Cloud/-prefixed diff of subscriber.al> --diff-root <t>/repo --file src/posting.al` now gives the diff's roots plus `file-unmatched` on `src/posting.al`, and with `--file Cloud/src/posting.al` gives both sets of roots;<br>- (b) a workspace with no `.git` above it gives `file-unmatched` for `--file Cloud/src/posting.al`, as today, and resolves `--file src/posting.al`;<br>- (c) `--diff-root <a regular file>` with only `--file` inputs gives exit 3, and stderr contains `is not a folder`;<br>- (d) `lone_diff_root_is_exit_3` has the new message, plus a `--routine`-only case that also exits 3;<br>- (e) `--file src/posting.al` plus a diff whose posting section has a rejected hunk header: the file's roots are present, plus the diff's `diff-parse-error`. A healthy `--file src/posting.al` plus the line-9 posting diff: roots are the file's whole set, each once, and `entries` equal the file-only run's;<br>- (f) the boundaries under GitTop: an absolute path to an existing workspace file and `cloud/src/posting.al` (wrong case) are both `file-unmatched`. |
| A4 | Batch decides the root for file-only jobs | Batch tests:<br>- a one-job manifest (`--file Cloud/src/posting.al` on `repo_ws`) resolves as standalone does;<br>- a two-batch manifest with the same file-only job in the worker batch, and a second worker job carrying an explicit `--diff-root`, both resolve as standalone does.<br>Discrimination: the batch path keeps `has_diff()` → unmatched. |
| A5 | Nothing else moves | The `#77`/`#111`/`#353` diff tests stay green; the one exception is `lone_diff_root_is_exit_3`'s message (A3 d). `check-goldens` passes with no regen (P5). The #340 unit test `changed_roots_human_lines` gains the variant, with control and bidi characters in its `file` and in one candidate, and asserts the exact escaped line. |

## Timeline

- 2026-10-26 00:43: claim ok (attempt 1). make-worktree ok, base 258da610. Loop 6/15 (shared).
- Classified bounded; probes P1–P6; Design written.
- Spec r1 (spec_rounds 1/5): both approve with fixes. Opus raised one critical: the lone-`--diff-root` check (also Sol's #1). Merged into V1–V16; Design, matrix and D1–D3 revised.
- Spec r2 (2/5): opus accepted all and raised N1 (V17). Sol re-raised V4, V7, V8, V9. A1–A3 revised.
- Spec r3 (3/5): both accepted V4, V7, V8, V9, V11, V17; both approve. **Converged**: 17 entries. Opus's notes go to the implementer brief.
- Impl step 1: tests A1–A5 written (11 CLI tests incl. the updated `lone_diff_root_is_exit_3` and refactored A21; one lib case). A21's manifest runner extracted as `batches_as_standalone` for A4.
- Impl step 2 (red): 7 CLI FAIL (A1, A2, A2 table-only, A3a, A3c, A3d, A4); 4 PASS as unchanged behaviour (A3b, A3e, A3f, A21). Lib test did not compile (no `FileAmbiguous`), as planned.
- Impl step 3 (green): `match_path`/`PathMatch`, `file_key`, `FileAmbiguous`, `reads_paths`; 102/102 `cli_digest_changed_spans`, 35/35 `digest_cli` lib tests pass.
- Impl step 4: 7 discrimination proofs, all broken=FAIL restored=PASS; the `ws:`-via-`match_path` break leaves A2 green (stated limit, see table).
- Impl step 5: `ci-steps task-affected` exit 0 (fmt, clippy-dev, gen-syntax, whole suite 4505/4505; no golden moved). `check-diff` reasons `[]`. Commit b4b78690.
- Final panel r1 fixes W1–W8:
  - W1: a `ws:` id picks one unit exactly. W2: with a `match_path` fallback. W3: only `ws:` counts as an id.
  - W4: `workspace_file_keys` is built lazily (`LazyCell`). W5: a shared `ws_rest` under `strip_scheme`.
  - W6: ledger row 1 reworded. W7: help text. W8: doc rewraps.
  - 103/103 `cli_digest_changed_spans` pass. 3 proofs, each broken=FAIL and restored=PASS (W1, W2, the P4 rerun).
  - `ci-steps task-affected` exit 0 (whole suite 4506/4506; no golden moved). `check-diff` reasons `[]`. Commit 0903cd05.
- Final panel r2 fix W11: a `ws:` id falls back to `match_path` only when no workspace FILE has its exact spelling; a table-only exact file is `file-unmatched`. CHANGELOG line rewrapped. 103/103 pass; 1 proof broken=FAIL restored=PASS. `task-affected` exit 0 (4506/4506; no golden moved). `check-diff` reasons `[]`. Commit e32970af.
- Conductor: task gate on b4b78690, 4505/4505. Draft PR #359.
  - Final r1 (final_rounds 1/5): sol approve-with-fixes (2 minor); opus approve-with-fixes (6 minor; the deviation is confirmed sound); code-review high (6). Merged into W1–W10; W9 refuted (ratified shape), W10 deferred (D1).
  - Final r2 (2/5): opus accepted all. Sol re-raised W1 (important), fixed as W11. The gate run on 0903cd05 was stopped as void.
  - Final r3 (3/5): both accepted W1 and W11. **Converged**: 28 entries.
  - Full gates on H = e32970af: all supervised, exit 0, queued 0 s. Peer alsem-second paused its probes for cdo-gate. origin/master unchanged (258da610).

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| A1 `git_top_relative_file_equals_workspace_relative` | `reads_paths` returns `self.has_diff()` (alsem.rs; the break applied, checked by grep: the original `self.has_diff() \|\| !self.files.is_empty()` was gone; the test then failed) | FAIL at :3919, `[{"file":"Cloud/src/posting.al","kind":"file-unmatched"}]` | PASS | b4b78690 |
| A1 `git_top_relative_file_equals_workspace_relative` | `file_key`'s scheme test `if has_scheme(path)` → `if true` (every input takes `normalize_input` only; applied: count 1) | FAIL at :3919, `file-unmatched` on `Cloud/src/posting.al` | PASS | b4b78690 |
| A2 `file_naming_two_files_is_ambiguous` | `Ambiguous` arm → `Ok(diff_readings(..).remove(0))` (the first reading; applied: count 1) | FAIL at :3983, diagnostics `[]` (expected the `file-ambiguous`) | PASS | b4b78690 |
| A3(a) `diff_root_reads_file_inputs`; A2 `file_naming_two_files_is_ambiguous` | `if has_scheme(path)` → `if false` (a `ws:` input goes through `match_path`; applied: count 1) | A3(a) FAIL at :3919, `file-unmatched` on `ws:src/posting.al`. **A2 PASSES**: under GitTop a `ws:` id's below-top reading never matches (its first folder is `ws:Cloud`, not `Cloud`) and the as-written reading strips the scheme, so `match_path` gives the same one reading. Stated LIMIT: only A3(a) (Explicit root) pins the scheme rule. | PASS (both) | b4b78690 |
| A3(a) `diff_root_reads_file_inputs`; A3(c) `diff_root_file_with_file_inputs_is_exit_3` | lone check `!reads_paths` → `!has_diff` (applied: count 1) | both FAIL: stderr `--diff-root needs a diff or file inputs …` (A3(a) no JSON at :86; A3(c) no `is not a folder` at :4138) | PASS (both) | b4b78690 |
| A4 `batch_decides_the_root_for_file_only_jobs` | `pin_diff_root` decides with `DigestInputs::of(d).has_diff()` (applied: count 1) | FAIL in `assert_each_standalone` (cli_batch_plan.rs:128): batch exit `Some(2)` vs standalone `Some(0)` | PASS | b4b78690 |
| A2 `file_naming_two_files_is_ambiguous` (r1 W1 case block) | `file_hits`' exact filter → `!unit.is_empty() \|\| source_unit_id == unit` (keeps every unit of the key; applied: count 1) | FAIL at :3920, `ws:src/Posting.al` roots `["OnAfterPostSalesDoc","OnAfterPostSalesDoc","PostSalesDoc","PostSalesDoc"]` (both files) | PASS | 0903cd05 |
| `ws_named_folder_resolves_under_diff_root` (r1 W2, unix) | `if !exact.is_empty()` → `if !exact.is_empty() \|\| true` (no `match_path` fallback for an id; applied: count 1) | FAIL at :3920, roots `[]` | PASS | 0903cd05 |
| A3(a) `diff_root_reads_file_inputs`; A2 `file_naming_two_files_is_ambiguous` (r1 rerun on `file_hits`) | `if let Some(rest) = ws_rest(path)` → `ws_rest(path).filter(\|_\| false)` (a `ws:` id goes through `match_path` only; applied: count 1) | both FAIL: A3(a) at :3919 (`file-unmatched` on `ws:src/posting.al`); A2 at :3920 (the W1 block roots both files). This closes row 4's A2 limit. | PASS (both) | 0903cd05 |
| A2 `file_naming_two_files_is_ambiguous` (r2 W11 case: table-only `src/Posting.al`) | the exact-file branch's `if ixs.is_empty() { return Err(FileUnmatched) } return Ok(ixs)` → `if !ixs.is_empty() { return Ok(ixs) }` (the r1 routine-span-based fallback; applied: count 1) | FAIL at :4058, diagnostics `[]` (fell back and rooted `src/posting.al`; expected `file-unmatched` on `ws:src/Posting.al`) | PASS | e32970af |
| A5 `changed_roots_human_lines` (lib) | `FileAmbiguous` human arm prints `file` and `candidates.join(", ")` raw (applied: count 1) | FAIL at digest_cli.rs:2921: raw tab / newline / U+202E in the line instead of `\t`, `\n`, `\u{202e}` | PASS | b4b78690 |

## Findings register summary

There are 28 entries, all accepted by sol (astra slot) and opus (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.
- Spec V1–V17: all fixed.
- Final W1–W8 and W11: fixed.
- Final W9: refuted. W10: deferred (minor, discovery D1).

## Base/Head SHAs

B = `258da61060fda62d4257f0dd1468e6c40c3366d6`. H = `e32970af9cf52ecb1c01f473eeb628f4a94c8048`.

## Gate results

| gate | exit | log |
|------|------|-----|
| ci-steps-all | 0 | `.agent/runs/20261026-004237-f107b1/logs/ci-steps-all/20261026T013435-c5b03732.log` |
| check-goldens-coverage | 0 | `.agent/runs/20261026-004237-f107b1/logs/check-goldens-coverage/20261026T014114-5d51c240.log` |
| check-goldens | 0 | `.agent/runs/20261026-004237-f107b1/logs/check-goldens/20261026T014114-896d6161.log` |
| cdo-gate | 0 | `.agent/runs/20261026-004237-f107b1/logs/cdo-gate/20261026T014114-ddeab248.log` (ENFORCE_CDO_WS=1) |

- Toolchain: rustc 1.96.0.
- Grammar: tree-sitter-al v4.5.0 `91a5db62`.
- CDO_WS: the pinned baseline, enforced by the gate's closure stamp.
- No golden moved.

## Discoveries

```json
[
  {
    "subsystem": "digest CLI wording (legacy --changed-files / --changed-routines names)",
    "locator": "src/bin/alsem.rs --changed help (`.al` entries → --changed-files; else → --changed-routines) and the no-input error 'at least one of --changed-files, --changed-routines, --diff, or --changed is required'",
    "symptom": "The digest CLI names flags it does not have: there is no --changed-files or --changed-routines (the real flags are --file and --routine). The words come from the TypeScript port.",
    "kind": "bug",
    "origin_issue": 339,
    "reproducer": "alsem digest <ws> --changed-files a.al -> clap: unexpected argument; alsem digest <ws> (no inputs) -> the error names --changed-files and --changed-routines.",
    "pre_existing": true,
    "capability": "digest help and errors name only the flags that exist (--file, --routine, --diff, --changed)",
    "acceptance": "the no-input error and the --changed help name --file and --routine; a test pins the error text"
  },
  {
    "subsystem": "digest --changed auto-detection (cwd-relative existence)",
    "locator": "src/engine/l5/digest_cli.rs auto_detect_changed_with (exists(value) → Diff); src/bin/alsem.rs detect_changed / changed_is_file",
    "symptom": "--changed X is read as a DIFF FILE whenever X exists relative to the current directory. Run from the git top, `--changed Cloud/src/posting.al` names the source file itself, so the AL source is parsed as diff text instead of being taken as a changed file.",
    "kind": "bug",
    "origin_issue": 339,
    "reproducer": "cd <repo>; alsem digest Cloud --changed Cloud/src/posting.al -> diff-parse / no roots, while --file Cloud/src/posting.al resolves (#339).",
    "pre_existing": true,
    "capability": "--changed tells a diff file from a changed .al source path (e.g. an existing .al path is a changed file, not a diff), independently of the cwd",
    "acceptance": "from the git top, --changed Cloud/src/posting.al resolves like --file Cloud/src/posting.al; a real .diff/.patch path is still read as a diff"
  },
  {
    "subsystem": "digest changed-roots (absolute file paths)",
    "locator": "src/engine/l5/digest_cli.rs normalize_input / path_readings (an absolute path keeps its root segment and never matches a workspace key)",
    "symptom": "--file /abs/path/to/ws/src/x.al (an absolute path to a workspace file) is file-unmatched; only workspace-, git-top- or --diff-root-relative paths and ws: ids resolve.",
    "kind": "enhancement",
    "origin_issue": 339,
    "reproducer": "alsem digest <ws> --file <ws>/src/posting.al -> file-unmatched.",
    "pre_existing": true,
    "capability": "an absolute path below the workspace resolves to its workspace file",
    "acceptance": "--file <abs ws>/src/posting.al resolves like --file src/posting.al; an absolute path outside the workspace stays file-unmatched"
  }
]
```

## Not delivered
