# Issue #77 — `digest --diff` rejects a diff made at a git root above the workspace

## Claim

```json
{"run_id":"20261015-231824-342aab","issue":77,"attempt":1,"session":"claude-code-session:cc1781f3-fe4e-414a-aea6-4919dc8d314c","branch":"issue/77-agent-discovery-l5-digest-chang-a1","worktree":"U:\\Git\\al-sem-issue-77-a1","body_hash":"c16a63e9dfe63ded8e8f988f5b3d00d40f51d80f4036dc1b6c2645101ffac297"}
```

Issue body sha256 `c16a63e9…ac297` (from the claim). Worktree from `master` @
`662f5137a54253496dedb2c81277f639492f2ffc`. Grammar `8682cb5f` via `TREE_SITTER_AL_PATH`.

Reviewer roster (operator instruction, 2026-10-15): `gpt-6.1-sol` fills the `astra` slot (`sol`)
and `claude-opus-5.5` fills the `flash` slot (`opus`); attested with `--substitute astra=sol
--substitute flash=opus`.

## Classification

**BOUNDED.** One lookup (`resolve_changed_roots`, `src/engine/l5/digest_cli.rs:376-480`): a changed
file path is normalised (`normalize_input`, `:107`) and looked up in the routine span index, whose
keys are workspace-relative (`ws:src/posting.al` → `src/posting.al`). A diff made at the enclosing
git repository's top level names `Cloud/src/posting.al`, which matches nothing, so it gives
`diff-file-unmatched` and no roots (exit 2, `:1652`: zero roots → 2).

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — the reproducer holds on `master` | **HOLDS**: `master`'s release-fast `alsem` (`662f5137`). With ws-d8 copied to `$SCRATCH/77/ws` (no `.git` above it) and a diff with headers `a/Cloud/src/posting.al` / `b/Cloud/src/posting.al` and a hunk on line 8, `alsem digest <ws> --diff root.diff --format json` → exit 2, `changedRootsDiagnostics = [{file: Cloud/src/posting.al, kind: diff-file-unmatched}]`, 0 entries (R2-04) | `$SCRATCH/77/p1.json`; `digest_cli.rs:463-472` |
| P2 — who calls the resolver with paths | only `digest` (`run_digest_pipeline`, `digest_cli.rs:1585`). `prove` passes a routine selector only (`prove.rs:905-909`, `files: None`, `diff_text: None`) | source |
| P3 — what makes digest exit 2 | zero roots only (`exit_code = if roots.is_empty() {2} else {0}`); an unmatched file is a diagnostic, not a failure | `digest_cli.rs:1652` |
| P4 — `strip_ab_prefix` | strips only `a/` / `b/` (`diff_parser.rs:94-100`); the repo prefix survives it | source |

## Design

(Revised in spec round 1: R1-01…R1-15.)

**The rule: workspace-first, then the workspace's repository prefixes.** A changed path (from
`--diff` or `--file`) is a workspace-relative path first. Only if it names no workspace
FILE is it read as relative to a directory that contains the workspace, and the workspace's path
below that directory is stripped. This is a workspace-first interpretation with a
repository-relative fallback, not a detection of each diff's coordinate system (R1-04).

**Where the prefixes come from:**

1. **Explicit:** `--diff-prefix <path>`, the workspace's path inside the repository the diff was
   made in (`--diff-prefix Cloud`). It is the ONLY prefix when given.
   - **Strict mode (final round 1, F1-05).** With the flag, every path input is
     REPOSITORY-relative. That covers `--diff` records (deleted ones included), `--file`, the
     `--changed` file lists, and the poisoned set.
   - A path resolves only by stripping the prefix. There is NO workspace-first step, so
     `src/x.al` is unmatched under `--diff-prefix Cloud` (T9).
   - A deleted record whose path does not start with the prefix is `diff-file-unmatched`, not
     `diff-file-deleted` (F1-08, T10).
   - Discovery mode is unchanged: workspace-first.
   - The value is normalised with `normalize_input`, and `/` is appended.
   - A value that is empty after normalising (`.`, `""`), rooted (`/x`, `//srv`, `C:/…`), or that
     contains a `..` segment is rejected: exit 1, with `digest: --diff-prefix must be a relative
     path inside the repository`.
   - This keeps #76's rule that a rooted path never matches (R2-03).

   The flag covers what discovery cannot see (R1-05):
   - a copied workspace;
   - a CI artifact;
   - a diff from another clone;
   - the issue's literal reproducer (`<tmp>/ws` with headers `Cloud/src/posting.al`).
2. **Discovered, by default:**
   - Canonicalise the workspace path (`std::fs::canonicalize`, which follows symlinks and junctions;
     `std::path::absolute` if that fails), so `.`, relative paths and aliases work (R1-02, R1-06).
   - Walk that one path's `ancestors()`, the workspace itself included. Every ancestor holding a
     `.git` entry (a directory, or the file of a worktree or submodule) gives one prefix: the
     workspace's path below it, from `strip_prefix` on the same canonical path, as `/`-joined
     components normalised like `normalize_input`, with a trailing `/`.
   - The prefixes are tried innermost first, so a submodule (`DO/Cloud/.git`) and the outer repo
     (`DO/.git`) both work (R1-09).
   - The workspace itself holding `.git` gives an empty prefix, which adds nothing.

**Resolution.** One helper, `resolve_changed_path(raw) -> ResolvedPath { key: Option<String> }`, is
used by every path input: the `--diff` files, the hunk-error `poisoned` set (#72), and
`--file`. The poisoned set and the lookup use the SAME resolved key, so an alias can never
revive a poisoned file (R1-03, R1-07).
1. `normalize_input(raw)`. If it names a **workspace file**, that is the key. A workspace file is
   either of these (R1-01, R2-05, R3-01):
   - **A discovered workspace source file:** any `ws:` `.al` file in the list the analysis target
     already holds, i.e. the program engine's snapshot. It is never walked a second time and never
     includes a dependency's files.
     - (Corrected at implementation.) That list is walked with `NestedApps::Walk`
       (`snapshot/provider.rs:34`). The model's rows, which give the spans, use the app-scoped
       `Skip` list. Skip ⊆ Walk, so every span file is in the discovered list.
     - A nested app's file therefore counts as a workspace file. It is never stripped and stays
       `*-unmatched`, as today, because it has no spans.
     - The unit test `every_span_file_is_a_discovered_workspace_file` pins both halves.
   - **A file with routine spans.**

   The second case keeps every path that resolves today on step 1 by construction. A unit test
   asserts that every span file is in the DISCOVERED list alone, before the span files are added
   (R4-01). That catches a `NestedApps` mode mismatch, which a test over the merged set could not. Together they cover routine-less and object-less
   files.
   - A file that matches here but has no spans keeps today's diagnostic: `diff-file-unmatched`
     (`--diff`) or `file-unmatched` (`--file`) (R2-05). It is never stripped.
2. Otherwise, for each prefix in order: if the normalised path starts with it, strip it. The first
   stripped path that names a workspace file is the key.
3. Otherwise there is no key, and the input is `diff-file-unmatched` / `file-unmatched` as today.

Diagnostics keep the parsed path, the way `diff_parser` produced it (R1-04).

**Plumbing (as shipped).**
- `ChangedInput<'a>` derives `Default` and gains two fields:
  - `prefixes: PathPrefixes`, either `Discovered(Vec<String>)` (the default) or `Explicit(String)`;
  - `workspace_files: &'a [String]`, borrowed from `Model::workspace_files`. That field is filled
    from the program snapshot's `ws` unit and is read only by `digest`.
- A private `ChangedPaths` holds the workspace-file set (the discovered files plus the span files)
  and the prefixes. `ChangedPaths::resolve_changed_path` serves every path input.
- `ChangedRootsResult.path_prefixes_used` (`BTreeSet`) feeds `ChangedInputContract`, which projects
  it as `changed.pathPrefixesUsed`. The field is omitted when empty (F1-07).

The constructors are:
- `run_digest_pipeline` (`digest_cli.rs:1585`), which fills it from `--diff-prefix` or discovery
  through `workspace_repo_prefixes(workspace: &Path) -> Vec<String>`;
- `prove` (`prove.rs:905`), which leaves it empty (routine selector only).

No other constructor exists in `src/` or `tests/` (R1-11).

`alsem digest` gains `--diff-prefix <path>`. It reaches `run_digest_pipeline` as a new parameter,
`diff_prefix: Option<&str>`. That function has 7 callers, and every one passes it (R2-04):
- `src/bin/alsem.rs:1097`;
- `tests/cli/cli_attribute_trivia.rs:33`;
- `cli_b_digest_differential.rs:154`, `:188`;
- `cli_digest_temp_state.rs:51`, `:1221`;
- `cli_s6_consumers.rs:78`.

The tests pass `None`. Stripping applies to EVERY path input: `--file`, the file lists of
`--changed`, and `--diff`. The workspace file list rides on `ChangedInput` too, as
`workspace_files`.

**Corpus effect (R1-12).** The corpus fixtures sit inside this repository, so a corpus digest run
now discovers prefixes such as `tests/r0-corpus/<fixture>/`. Exact-first keeps every path that
resolves today resolving the same way. The digest differential and exit-oracle suites run as gates.

**Limits, stated and not changed:**
- `--changed <value>` auto-detection reads an existing `.al` path relative to the CWD as a diff. That
  is pre-existing (`auto_detect_changed`, R1-13).
- `--no-prefix` and the `i/` / `w/` mnemonic diff prefixes are not stripped (`strip_ab_prefix`).
- A repository file outside the workspace whose path also names a workspace file resolves to the
  workspace file (workspace-first, R1-04).

**Tests.** They are in `tests/cli/` and drive `CARGO_BIN_EXE_alsem digest`. Each uses a tempdir copy
of `ws-d8-commit-in-tx` at `<tmp>/repo/Cloud` with `<tmp>/repo/.git/` created, unless it says
otherwise.

- **T1** `diff_from_the_repo_root_resolves_like_a_workspace_diff`.
  - A diff with headers `Cloud/src/posting.al` and a hunk on line 8 gives exit 0 and the SAME
    changed roots as the same diff naming `src/posting.al`.
  - No `diff-file-unmatched`.
  - Mutation: drop step 2 → exit 2, unmatched.
- **T1b** `diff_prefix_flag_resolves_a_copied_workspace`.
  - The literal reproducer: the workspace is at `<tmp>/x/ws`, with no `.git` above it (asserted).
    `--diff-prefix Cloud` and headers `Cloud/src/posting.al` give exit 0 and the same roots as T1.
  - Mutation: ignore the flag → exit 2.
- **T2** `file_inputs_from_the_repo_root_resolve`.
  - `--file Cloud/src/posting.al` resolves like `--file src/posting.al`.
  - So does the `--changed` comma list `Cloud/src/posting.al,x.al`, which is not auto-detected as a
    diff (R2-02).
  - Mutation: step 2 only for diffs.

Every exit-2 test first parses stdout as a digest document (the existing `digest()` helper in
`cli_digest_changed_spans.rs`). So a clap usage error, which also exits 2, can never pass one
(R2-02).
- **T3** `a_path_in_the_workspace_wins_over_the_stripped_one`.
  - The workspace also holds `Cloud/src/posting.al`, a second codeunit with its own number, name and
    procedure. Neither its object id nor its name collides with ws-d8. Its procedure spans line 8.
  - A diff naming `Cloud/src/posting.al` (line 8) gives that inner procedure as the root (asserted
    by display name), and `PostSalesDoc` is absent (R1-10).
  - Mutation: stripped first → `PostSalesDoc`.
- **T3b** `a_routine_less_workspace_file_is_not_stripped`.
  - Two variants of the workspace's inner `Cloud/src/posting.al`: one holding only a table, and one
    holding only comments (no object).
  - A diff naming it gives no root from `src/posting.al`: exit 2, no `PostSalesDoc`, and today's
    diagnostic kind (`diff-file-unmatched` for `--diff`, `file-unmatched` for `--file`). This holds
    through both `--diff` and `--file` (R1-01, R2-05).
  - Mutation: decide "workspace file" by spans or by `ModelObject` anchors → `PostSalesDoc`
    appears (the comments-only variant).
- **T4** `no_git_top_level_means_no_prefix`.
  - The workspace is at `<tmp>/x/Cloud`: the same name, but no `.git` at or above it. The test
    asserts that precondition and skips with a message if an ancestor has `.git`.
  - The `Cloud/` diff stays `diff-file-unmatched`, exit 2 (R1-08).
  - Mutation: a basename heuristic → exit 0.
- **T5** `a_repo_file_outside_the_workspace_stays_unmatched`.
  - A root diff touching `Cloud/src/posting.al` and `OnPrem/src/posting.al` gives roots for the
    first, `diff-file-unmatched` for `OnPrem/src/posting.al`, and exit 0.
  - Mutation: drop the prefix-match requirement in step 2 (strip any first segment). Then
    `OnPrem/src/posting.al` wrongly strips to the real `src/posting.al` and is no longer reported
    (R2-01).
- **T6** `a_poisoned_root_relative_file_gives_no_roots` (#72 through the prefix).
  - One diff names `Cloud/src/posting.al` with a malformed `@@` in one record and a valid hunk on
    line 8 in another. Run it in both record orders, plus the variant where the malformed record is
    `Cloud/src/posting.al` and the healthy one is `src/posting.al`.
  - Each gives no roots from that file, `diff-parse-error` naming the parsed path, and exit 2.
  - Mutation: build `poisoned` from the raw normalised path → roots appear (R1-03, R1-07).
  - Collision control, using T3's workspace with both `Cloud/src/posting.al` (the inner file) and
    `src/posting.al`:
    - a malformed header in `Cloud/src/posting.al` poisons only the inner file;
    - a healthy line-8 hunk in `src/posting.al` still gives `PostSalesDoc`.

    Mutation: poison every candidate key (raw and stripped) → `PostSalesDoc` is lost (R2-06).
- **T8** `diff_prefix_rejects_rooted_empty_and_dotdot`.
  - `--diff-prefix /x`, `.`, `""`, `a/../b` and `C:/x` each exit 1 with the message.
  - Unit test: `normalize_diff_prefix`. Mutation: accept them → a rooted `/src/x.al` resolves (R2-03).
- **T7** `nested_git_entries_give_innermost_first_prefixes`.
  - With `<tmp>/repo/.git` and `<tmp>/repo/Cloud/.git` (a submodule), both a `Cloud/`-prefixed diff
    and an unprefixed one resolve (R1-09).
- **T9** `explicit_diff_prefix_never_reads_a_path_as_workspace_relative`.
  - The workspace is at `<tmp>/x/ws` with no `.git` above it, run with `--diff-prefix Cloud`.
  - A diff naming `src/posting.al` gives `diff-file-unmatched`, no `PostSalesDoc`, and exit 2.
  - A diff naming `Cloud/src/posting.al` gives `PostSalesDoc` and exit 0.
  - Mutation: exact-first in explicit mode (F1-05).
- **T10** `deleted_records_outside_an_explicit_prefix_are_unmatched`.
  - With `--diff-prefix Cloud`:
    - `OnPrem/src/Old.al` deleted → `diff-file-unmatched`;
    - `Cloud/src/gone.al` deleted → `diff-file-deleted`;
    - `PostSalesDoc` is a root, and the exit is 0.
  - Mutation: no resolution for deleted records (F1-08).
- **T1** also asserts `changed.pathPrefixesUsed == ["cloud/"]`, and that the field is absent for a
  workspace-relative diff (F1-07).
- **Unit tests** for `workspace_repo_prefixes`:
  - top level → `[]` (empty prefix dropped);
  - nested `[Cloud/]`;
  - two levels `[Cloud/, DO/Cloud/]`, innermost first;
  - a `.git` file;
  - no `.git`;
  - a relative `.` path;
  - a trailing slash;
  - a symlinked workspace, skipped where symlinks are unsupported (R1-02, R1-06).

**CHANGELOG `### Fixed` text:**

(As shipped in `CHANGELOG.md`, `44a3a43a`.)

> `alsem digest --diff`, `--file` and the file lists of `--changed` now accept paths written
> relative to a directory that contains the workspace, such as a `git diff` run in `DO/` for the
> workspace `DO/Cloud` (#77). By default a path that names a workspace file as written wins, and
> any other path is retried with the workspace's path below each enclosing git work tree's top
> level stripped, innermost first. Discovery needs `.git` at or above the workspace. The new
> `--diff-prefix <path>` names the workspace's path inside the diff's repository instead (e.g.
> `--diff-prefix Cloud`, for a copied workspace or another clone). With it, every path is
> repository-relative and resolves only with that prefix stripped: there is no workspace-first
> step, so `src/x.al` is unmatched under `--diff-prefix Cloud`. A rooted, empty, drive (`C:`) or
> `..` value is rejected. A repository file outside the workspace stays `diff-file-unmatched`
> (`--diff`) or `file-unmatched` (`--file`), except that by default (no `--diff-prefix`) one whose
> path also names a workspace file resolves to that workspace file. A deleted file is still
> `diff-file-deleted`, except that under `--diff-prefix` a deleted file outside the prefix is
> `diff-file-unmatched`. The digest document lists the prefixes that resolved at least one path in
> `changed.pathPrefixesUsed` (omitted when none did).

## Timeline

- Claimed (attempt 1; tick 6 of `/orchestrate --max-issues 40`; first of three value-2/S issues,
  oldest). Preflight first refused on `disk-free:33GB`: `scripts/sweep-builds --days 2` (13.6 GB of
  stale main-checkout output) and `cargo clean` of the blocked #83 worktree's target freed 55 GB.
- Probes P1-P4; bounded design.
- Spec round 1:
  - sol: 3 important + 1 minor. Routine-span membership, symlinks, poisoned-key aliasing, CHANGELOG.
  - opus: 1 critical + 4 important + 6 minor. The literal reproducer needs an explicit flag; relative
    paths; poisoned keys; T4 environment; submodules; T3 detail; constructors; corpus effect;
    auto-detect; CHANGELOG; hint.
  - Design rewritten: `--diff-prefix`; canonicalise; every `.git` ancestor innermost first; one
    resolved key for the lookup and poisoning; T1b/T3b/T6/T7. Register R1-01…R1-15 (R1-13 and R1-15
    deferred, minor).
- Spec round 2: both not ready.
  - Re-raised R1-01 (object-less files), R1-03 (collision control), R1-04 (CHANGELOG).
  - New R2-01…R2-06: T5's mutation; the flag is `--file`, not `--changed-files`; `--diff-prefix`
    validation; the 7 `run_digest_pipeline` callers; P1 run on master; diagnostic kind.
  - All fixed; P1 confirmed (exit 2, `diff-file-unmatched`).
- Spec round 3: opus ready; sol re-raised R1-01 (a stale Limits bullet). Opus minors R3-01
  (`NestedApps` mode) and R3-02; all fixed.
- Spec round 4: both accepted all 23, ready. Opus minor R4-01 (the span-file test must check the
  discovered list alone); fixed.
- Spec round 5 (`spec_rounds` 0 left): both accepted all 24, no new findings, "ready". **SPEC
  CONVERGED.**
- Step 7: plan `docs/superpowers/plans/2026-10-15-issue-77-digest-repo-relative-paths.md`, 1 task.
  Steps 6 and 8 are dispatched as ONE opus implementer (red, then green, with proofs);
  `$SCRATCH/77/impl.md`.
- Implementation is commit **`a86f2ed7`**.
  - RED: T1, T1b, T2, T5, T6 (`bad-root-healthy-ws`), T7, T8. T3, T3b, T4 and two T6 variants pass
    on base by design.
  - GREEN: `cli` 360, `--lib` 2037, `differential` 12; clippy clean; `check-goldens` green, no
    golden moved; check-diff `[]`.
  - 16 discrimination proofs. The symlink unit test SKIPs here (os error 1314, no symlink
    privilege), so it has no proof.
  - Deviations:
    - The workspace-file list is the program snapshot's (`NestedApps::Walk`), a superset of the
      model's span files (`Skip`). So a nested app's file counts as a workspace file and is never
      stripped; it stays `*-unmatched` as today. The span ⊆ discovered test pins this.
    - `Model` gains `workspace_files`, copied before `ProgramContext` is dropped.
    - The unit tests were written after the helpers, so they have proofs but no executed RED.
- Task review (sonnet) and `code-review high` running in parallel.
- Task review (`$SCRATCH/77/review.md`): **approve**, with 4 minors (F1-01…F1-04).
- `code-review high`: 8 findings (F1-05…F1-12).
  - Dispositions: 6 fixed, CR2 refuted (outer prefixes are deliberate, R1-09), CR7 deferred (the
    Model field avoids a second walk).
  - Fixes, commit **`9256e60a`**:
    - an explicit `--diff-prefix` is STRICT: paths resolve only by stripping it, with no
      workspace-first step (T9);
    - deleted records outside an explicit prefix are `diff-file-unmatched` (T10);
    - `changed.pathPrefixesUsed` is new, omitted when empty (T1);
    - test skip guards; the workspace file list is borrowed, not cloned; help text, docs and the
      CHANGELOG are updated.
  - 3 proofs; `cli` 362, `--lib` 2037, `differential` 12; clippy clean; no golden moved;
    check-diff `[]`.
  - The design change from the converged spec (strict explicit mode) goes to the final panel.
- Gates started on `9256e60a`; final panel round 1 sent.
- Final panel round 1 (`final_rounds` 4 left): both accepted 35 of 36 and re-raised R1-04 (the
  shipped CHANGELOG dropped the discovery-mode qualifier).
  - sol minor F2-01: the symlink test lacked a skip guard.
  - opus minor F2-02: the ledger Design did not match what shipped.
  - Gates on `9256e60a` were stopped before any verdict (orphan `ci-steps build` killed).
  - Fixed in **`44a3a43a`**: the CHANGELOG qualifier and the symlink skip guard. Clippy clean, hook
    goldens OK, check-diff `[]`. The ledger Design was updated.
- Final panel round 2 (`final_rounds` 3 left): both accepted all 38, no new findings, "ready".
  **FINAL PANEL CONVERGED.** Gates running on `44a3a43a`.

## Acceptance matrix

| Acceptance (issue #77) | proof |
|---|---|
| diff paths are resolved relative to the workspace by stripping the workspace's path below the git top level | discovered prefixes (`workspace_repo_prefixes`), T1 and T7, and the unit tests |
| (or a `--diff-root` option) | `--diff-prefix <path>` (strict); T1b runs the issue's literal reproducer; T9, T10, T8 |
| a test with a `Cloud/`-prefixed diff resolves the same roots as the unprefixed one | T1 (the same roots, and `pathPrefixesUsed == ["cloud/"]`) |

## Discrimination proofs

Full table in `$SCRATCH/77/impl.md` and `fix.md`.

| test | break | fail | commit |
|---|---|---|---|
| T1 | drop step 2 | `cli_digest_repo_prefix.rs:155` | `a86f2ed7` |
| T1b | ignore the flag | `:181` | `a86f2ed7` |
| T2 | step 2 only for diffs | `:199` | `a86f2ed7` |
| T3 | stripped path first | `:224` (`PostSalesDoc` instead of `InnerPost`) | `a86f2ed7` |
| T3b | workspace file = span files only | `:248` | `a86f2ed7` |
| T4 | basename heuristic | `:285` | `a86f2ed7` |
| T5 | strip any first segment | `:309` | `a86f2ed7` |
| T6 / collision control | `poisoned` from the raw path; poison every candidate | `:367`; `:384` | `a86f2ed7` |
| T7 | stop at the first `.git` | `:405` | `a86f2ed7` |
| T8 + `normalize_diff_prefix` unit test | accept every value | `:426`; `digest_cli.rs:1852` | `a86f2ed7` |
| unit `per_layout` / `without_git` / `relative_path` / `every_span_file…` | outermost first; no `.git` test; no canonicalisation; empty `workspace_files` | `:1895`; `:1913`; `:1929`; `:2014` | `a86f2ed7` |
| T9 | exact-first in explicit mode | `:440` | `9256e60a` |
| T10 | no resolution for deleted records | `:482` | `9256e60a` |
| T1 `pathPrefixesUsed` | never record | `:168` | `9256e60a` |
| symlink unit test | — | not provable here: it SKIPs (os error 1314) | — |

## Findings register summary

38 entries:
- spec rounds: R1-01…R1-15, R2-01…R2-06, R3-01/02, R4-01;
- final round 1: F1-01…F1-12, F2-01/02.

Outcome: 34 fixed, 1 refuted (F1-06: outer `.git` prefixes, which are deliberate under R1-09), and
3 deferred, all minor:
- R1-13: `--changed` auto-detect (discovery D1);
- R1-15: an optional hint;
- F1-11: the file list lives on `Model`.

Every entry is accepted by sol and opus (final round 2).

## Gate results

On H = `44a3a43ab1f5d5eced71687872e52f273cedbfe2`, with B = `662f5137a54253496dedb2c81277f639492f2ffc`.
`origin/master` has not moved, so there was no rebase. Toolchain rustc 1.96.0; grammar `8682cb5f`;
CDO_WS = the pinned baseline (`bc3ccb18`).

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 669.7 | `logs/ci-steps-all/20261016T003403-3d94a5bd.log` |
| check-goldens-coverage | 0 | 0.6 | `logs/check-goldens-coverage/20261016T004513-a12a4a48.log` |
| check-goldens | 0 | 99.3 | `logs/check-goldens/20261016T004514-bcb27192.log` |
| cdo-gate | 0 | 442.6 | `logs/cdo-gate/20261016T004654-f6c247a7.log` |

Every gate ran supervised. After the gates, `git status --porcelain -- . ':!.agent'` was empty.

## Discoveries

```json
[
  {
    "subsystem": "l5 / digest (--changed auto-detection)",
    "locator": "src/engine/l5/digest_cli.rs auto_detect_changed / auto_detect_changed_with (~:134-151)",
    "symptom": "`--changed <value>` treats any value naming an existing path relative to the process CWD as a diff file. A user in DO/ typing `--changed Cloud/src/posting.al` (an .al source, not a patch) gets that AL file parsed as a unified diff, so it gives no roots or a diff-parse-error instead of selecting the file.",
    "kind": "bug",
    "origin_issue": 77,
    "reproducer": "cd <repo>; alsem digest Cloud --changed Cloud/src/posting.al (an existing .al file) -> parsed as --diff, not as a changed file.",
    "pre_existing": true,
    "capability": "--changed classifies an existing `.al` file as a changed source file, not a diff",
    "acceptance": "An existing path ending in `.al` is classified as Files, not Diff; a test pins it alongside the existing auto-detect tests."
  }
]
```

## Not delivered

- No hint when a path misses and no prefix exists (R1-15, deferred).
- `--no-prefix` and the `i/` / `w/` diff prefixes are not stripped (a stated limit).
