# Issue #76 — a `--file` path written with `./` (or another spelling) is never matched

## Claim

```json
{"run_id":"20261015-164440-ab031b","issue":76,"attempt":1,"session":"claude-code-session:cc1781f3-fe4e-414a-aea6-4919dc8d314c","branch":"issue/76-agent-discovery-l5-digest-chang-a1","worktree":"U:\\Git\\al-sem-issue-76-a1","body_hash":"4fcdf19073b8bda18f88c1f1aab7585b88bfe3b9dd7a40c396344112f9f71194"}
```

Issue body sha256 `4fcdf190…1194` (from the claim). Worktree from `master` @
`8f6d09203bfe55c5d597823758ed3a14ecfe9f9d`. Grammar `8682cb5f` via `TREE_SITTER_AL_PATH`.

## Classification

**BOUNDED, one function.** Every path comparison in digest's changed-root resolution goes through
`normalize_input` (`src/engine/l5/digest_cli.rs:100`): the span index keys (`:340`), `--file`
inputs (`:382`), diff file paths (`:454`) and the poisoned-file set (`:445`, `:461`). It strips the
`ws:`/`app:` scheme, turns `\` into `/` and lowercases, but it never collapses `.` segments, empty
segments or `..`. One fix in that one function covers every caller.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — the reproducer fails on `master` | **HOLDS** | `target/release-fast/alsem.exe` in the main checkout (built from `b77025e4`; `git diff b77025e4 8f6d0920 -- digest_cli.rs` does not touch `strip_scheme`/`normalize_input`). `alsem digest tests/r0-corpus/ws-d8-commit-in-tx --file <f> --deterministic`: `src/posting.al` → exit 0, 2 entries; `./src/posting.al` → **exit 2, 0 entries, `file-unmatched`** |
| P2 — each Acceptance spelling on `master` | `src/./posting.al` → exit 2 `file-unmatched`; **`src\posting.al` → exit 0, 2 entries (ALREADY works: `\` is mapped to `/`, `digest_cli.rs:89-96`)**; `SRC/Posting.al` → exit 0 (lowercased) | same command, each form |
| P3 — other spellings of the same file | `src//posting.al` → exit 2; `src/../src/posting.al` → exit 2 | same command |
| P4 — the span index keys are already clean | yes: they are made from `source_unit_id` (`ws:src/posting.al`), so the clean-up leaves every key unchanged for files whose names contain no literal `\` (the exception is stated in Design) | `digest_cli.rs:338-343`; the keys in P1's matched entries |
| P5 — diagnostics echo the user's spelling | yes: `FileUnmatched { file: file.clone() }` keeps the original input; only the lookup key is normalised | `digest_cli.rs:382-386` |

## Design

`normalize_input(p)` becomes, in this order (R1-04): `strip_scheme(p)` (unchanged: scheme off,
`\` → `/`), then a narrow LEXICAL clean-up of the `/`-separated segments, then lowercase. The
clean-up does exactly two things (R1-01, R1-02):
- it drops every `.` segment;
- it drops every EMPTY segment except the run of empty segments at RAW index 0 of the split (counted
  before any `.` is dropped), of which it keeps at most 2 (R4-01). So `src//x.al` → `src/x.al`,
  `/src/x.al` stays rooted, `//srv/x.al` stays UNC, `///src/x.al` → `//src/x.al`, and
  `.//src/x.al` → `src/x.al` (its raw index 0 is `.`, not empty);
- a trailing `/` is an empty segment like any other, so it is dropped: `src/posting.al/` matches
  `src/posting.al` (deliberate leniency; R4-02).

Nothing else changes: `..` is NOT cancelled (with symlinks, `link/../x.al` need not be workspace
`x.al`), and a drive prefix such as `C:` stays as written. Then the kept
segments are joined with `/`. The function stays pure: no `canonicalize`, no file system access.
The span index keys are already in this form for every file whose name contains no literal `\`
(P4: they contain no `.` or empty segments), so none of those keys changes. **Widened alias
(R1-02, stated):** on POSIX, a file whose NAME literally contains `\` (e.g. `src\.\x.al`) was
already folded by the pre-existing `\` → `/` mapping. It now also loses the `.` and empty segments,
so it keys like `src/x.al`, and a selection of either returns both files' routines. Windows forbids
`\` in a file name, but POSIX discovery does not reject such names, so file names with a literal `\`
are OUTSIDE the guarantee this change gives. No collision handling is added; the limit is listed in
Not delivered. Every caller gets the fix at once. Diagnostics keep the user's original spelling (P5).

Not changed, and stated as limits: `..` segments (`src/../src/x.al` still does not match);
absolute and rooted paths (still no match); the PRE-EXISTING `\` → `/` mapping of `strip_scheme`,
which on POSIX can make a file whose name contains a literal `\` collide with a nested path (R1-02;
present before this change, and slightly widened by it, as stated above); `--routine` selectors.

Tests:
- **T1, the Acceptance test** (`tests/cli/cli_digest_changed_spans.rs`, new
  `file_spellings_resolve_to_the_same_routines`): it drives the `alsem` binary
  (`CARGO_BIN_EXE_alsem`) on `tests/r0-corpus/ws-d8-commit-in-tx` with `src/posting.al`,
  `./src/posting.al`, `src/./posting.al`, `src\posting.al`, `.\src\posting.al` (PowerShell's
tab-completion form; R4-05) and `src//posting.al`. Each must give
  exit 0, the SAME entry list (the routine ids, hand-checked to be the 2 routines of `posting.al`)
  as `src/posting.al`, and an empty `changedRootsDiagnostics` (R1-05). Mutation: drop the
  `.`-segment skipping → `./` and `src/./` fail.
- **T1n, negative controls** (same file, `rooted_and_climbing_spellings_still_do_not_match`):
  `/src/posting.al` and `src/../src/posting.al` each give exit 2, zero entries and one
  `file-unmatched` whose `file` is the original spelling. Mutation: drop the leading empty segment
  too → `/src/posting.al` matches → fails (R1-01).
- **T2** (unit test in `digest_cli.rs`, `normalize_input_drops_dot_and_empty_segments`):
  hand-stated input → output pairs: `./src/X.al` → `src/x.al`, `src/./x.al` → `src/x.al`,
  `src//x.al` → `src/x.al`, `/src/x.al` → `/src/x.al`, `//srv/x.al` → `//srv/x.al`,
  `../x.al` → `../x.al`, `src/../x.al` → `src/../x.al`, `ws:src\x.al` → `src/x.al`,
`.//src/x.al` → `src/x.al`, `///src/x.al` → `//src/x.al`, `src/x.al/` → `src/x.al`,
`.\SRC\x.al` → `src/x.al`. Mutation:
  cancel `..` → `src/../x.al` becomes `x.al` → fails (R1-03).
- **T3** (diff path, R4-04): two `--diff` inputs, one with headers `--- ./src/posting.al` /
  `+++ ./src/posting.al` (as `diff -u --label` writes them; plain `diff -u` adds a tab and a
  timestamp, which the parser keeps in the path, pre-existing, Discovery 1) and one with `--- a/./src/posting.al` /
  `+++ b/./src/posting.al`, each with a one-line hunk at line 9 of `posting.al`. Each must give
  exit 0, `roots == ["PostSalesDoc"]` (hunk matching, not whole-file) and an empty
  `changedRootsDiagnostics`. Mutation: bypass `normalize_input` for the diff lookup → fails.
- **T4** (the poisoned-file set, R1-06): extend the existing veto test so a malformed hunk header
  in a section for `./src/posting.al` plus a healthy section for `src/posting.al`, and the
  reverse spellings, in both record orders, give no `posting` roots, keep the error's original
  spelling, and give EXACTLY `diag_kinds == ["diff-parse-error"]` (R4-03). Precondition asserted
  first: a healthy `./src/posting.al` record alone gives `roots == ["PostSalesDoc"]`. Mutation:
  normalise the poison set with the old function → the case with the malformed record spelled
  `./src/posting.al` and the healthy one `src/posting.al` fails (only that direction; in the reverse
  direction the old and new keys coincide).

## Base/Head SHAs

- Base: `8f6d09203bfe55c5d597823758ed3a14ecfe9f9d` (`master`; = `origin/master` at step 11, no rebase)
- Head (approved diff): `daab14adc33ab6489dcdc95f0281117ffb207e20`. Commits: `1a1fc6b4`, `04c00852`, `daab14ad`.

## Timeline

- Claimed (attempt 1), worktree created; probes P1-P5 run; bounded design written.
- Spec round 1 (astra + flash): 3 important + 3 minor; design narrowed (no `..`, keep root), tests
  T1n/T4 added, T2 fixed.
- Spec round 2: flash ready; astra re-raised R1-02 (the widened literal-backslash alias).
- Spec round 3: flash ready; astra re-raised R1-02 again, documentation only (stale "not widened"
  claims, "impossible" overstated). Fixed in the ledger and register.
- **Operator instruction (mid-run): the external reviewer is gpt-6.1-sol, with Opus as the second
  reviewer.** The executor's two slots are filled by named stand-ins: `astra` slot = `gpt-6.1-sol`
  (`sol`), `flash` slot = `claude-opus-5.5` (`opus`), passed to `attest` as `--substitute
  astra=sol --substitute flash=opus`, so the attestation names who actually signed. From spec
  round 4 on, `sol` and `opus` mark every entry; the earlier astra/flash marks stay on record.
- Spec round 4 (sol + opus): sol accepted all 6, "ready"; opus accepted all 6, "ready", plus 5
  minors (R4-01…R4-05: the exact empty-segment rule, the trailing `/`, the T3/T4 details and the
  `.\src\posting.al` spelling). All fixed in the ledger.
- Spec round 5 (`spec_rounds` 0): sol and opus accepted all 11, "ready". **SPEC CONVERGED.**
- Step 7: plan `docs/superpowers/plans/2026-10-15-issue-76-digest-file-path-spellings.md`, 1 task.
  Steps 6+8 ran as ONE opus dispatch (red, then green with proofs), because the whole change is
  one function plus its tests. Commit **`1a1fc6b4`** (`$SCRATCH/76/impl.md`): `--lib digest_cli` 10,
  `--test cli` 347 passed, clippy clean, no golden moved, `check-diff` `{"reasons": []}`.
- Task review (sonnet): approve, 3 minors (`$SCRATCH/76/review.md`), the same as code-review's.
- Final panel round 1 (sol + opus) plus the built-in `code-review high` (forked): both panelists
  flagged the CHANGELOG/T3 over-claim. Plain `diff -u` timestamped headers are still unparsed;
  that limit predates #76 and is Discovery 1. Fixed in **`04c00852`**. code-review also found two
  small code points, fixed in **`daab14ad`** (normalize once; exact slash-only doc), plus the
  `--changed` auto-detect gap (Discovery 2) and a test-helper duplication (deferred). Gate runs
  on `1a1fc6b4` and `04c00852` were STOPPED before any verdict, because the tree was about to
  change; the gates were rerun on `daab14ad`.
- Final panel round 2: sol and opus accepted all 18 entries, "ready". **FINAL PANEL CONVERGED.**

## Acceptance matrix

| Acceptance | proof |
|---|---|
| `./src/x.al`, `src/./x.al` and `src\x.al` resolve to the same routines as `src/x.al` | T1 (plus `//`), T2, T3, T4; T1n pins what still does not match |
| a test drives the alsem binary with each form | T1 spawns `CARGO_BIN_EXE_alsem` once per form |

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| T1 `file_spellings_resolve_to_the_same_routines` | no `.` skipping (`!s.is_empty()` only) | `cli_digest_changed_spans.rs:2323: left: 2 right: 0` (exit for `./src/posting.al`) | ok | `1a1fc6b4` |
| T1n `rooted_and_climbing_spellings_still_do_not_match` | keep 0 leading empty segments (`.min(0)`) | `:2339: left: 0 right: 2` (`/src/posting.al` exited 0) | ok | `1a1fc6b4` |
| T2 `normalize_input_drops_dot_and_empty_segments` | cancel `..` | `digest_cli.rs:1723: left: "x.al" right: "src/../x.al"` | ok | `1a1fc6b4` |
| T3 (diff path) | diff lookup uses `path.to_lowercase()` instead of `normalize_input` | `:2361: left: 2 right: 0` | ok | `1a1fc6b4` |
| T4 (poison set) | poison set built with the old normalisation | `:1355` roots not empty for the `a/./src/posting.al` diff | ok | `1a1fc6b4` |

RED on base (`$SCRATCH/76/impl.md`): T1, T2, T3, T4 fail for the stated reasons; T1n passes on base by
design (rooted and `..` never matched).

## Findings register summary

18 entries: R1-01…R1-06 (spec round 1, astra + flash), R4-01…R4-05 (spec round 4, opus),
F1-01…F1-07 (final round 1: sol, opus, code-review). Severity: 4 important, 14 minor. Disposition: 15
fixed, 1 refuted (F1-04, with reason), 2 deferred (F1-06, F1-07; both minor). The reviewer roster was
changed mid-run by the operator: the `astra` slot is filled by `gpt-6.1-sol` (`sol`) and the `flash`
slot by `claude-opus-5.5` (`opus`); both accepted all 18 entries against the code at `daab14ad`, and
the attestation is made with `--substitute astra=sol --substitute flash=opus`.

**Acceptance status:** met. T1 drives the `alsem` binary with `./src/posting.al`, `src/./posting.al`,
`src\posting.al` (plus `.\src\posting.al` and `src//posting.al`), and each returns the same routines as
`src/posting.al`.

## Gate results

At H = `daab14ad`, all through the supervisor (`"supervised": true`):

| gate | exit | seconds | log |
|---|---|---|---|
| `ci-steps-all` | 0 | 714.9 | `.agent/runs/20261015-164440-ab031b/logs/ci-steps-all/20261015T172750-af78cdfa.log` |
| `check-goldens-coverage` | 0 | 0.7 | `…/logs/check-goldens-coverage/20261015T173946-386973db.log` |
| `check-goldens` | 0 | 63.4 | `…/logs/check-goldens/20261015T173947-7e01f035.log` |
| `cdo-gate` | 0 | 320.7 | `…/logs/cdo-gate/20261015T174051-2c22ed8c.log` |

`git status --porcelain -- . ':!.agent'` is empty after the gates. No golden moved. Toolchain rustc
1.96.0; grammar `8682cb5f`; CDO_WS identity hash `1e512deb11c28d513ade51d45a7e4cbd2f8a3fb6f145ba94387ec8e5b288bce7`
(sha256 of `<CDO_WS path>@bc3ccb18`). The change is local to digest's path comparison, so no CDO
number can move; `cdo-gate`'s ratchets are green.

## Discoveries

```json
[
  {
    "subsystem": "l5 / digest (diff parser)",
    "locator": "src/engine/l5/diff_parser.rs extract_path / unquote_path (~:165-175)",
    "symptom": "A unified diff written by plain `diff -u` has headers like `--- ./src/posting.al<TAB>2026-10-15 16:44:40.000000000 +0200`; the parser keeps everything after the TAB as part of the path, so the file is reported diff-file-unmatched (exit 2), and `+++ /dev/null<TAB><timestamp>` is not recognised as a deletion.",
    "kind": "bug",
    "origin_issue": 76,
    "reproducer": "diff -u ./old/src/posting.al ./src/posting.al > p.diff (headers carry a tab + timestamp); alsem digest tests/r0-corpus/ws-d8-commit-in-tx --diff p.diff --deterministic -> diff-file-unmatched, exit 2.",
    "pre_existing": true,
    "capability": "digest --diff parses plain `diff -u` / `diff -ru` headers (path ends at the first TAB; /dev/null with a timestamp is a deletion)",
    "acceptance": "a cli test feeds a diff with timestamped headers (and a /dev/null deletion with a timestamp) and gets the same roots / DiffFileDeleted as the timestamp-free form."
  },
  {
    "subsystem": "l5 / digest (--changed auto-detection)",
    "locator": "src/engine/l5/digest_cli.rs auto_detect_changed_with (~:133)",
    "symptom": "`alsem digest . --changed ./src/posting.al`, run from the workspace root, treats the existing .al file as a DIFF (auto-detect checks existence first), finds no diff records, and exits 2 with diffProvided true, no entries and no diagnostic; the same input through --file returns the file's routines.",
    "kind": "bug",
    "origin_issue": 76,
    "reproducer": "cd tests/r0-corpus/ws-d8-commit-in-tx && alsem digest . --changed ./src/posting.al --deterministic -> exit 2, entries [], changedRootsDiagnostics [].",
    "pre_existing": true,
    "capability": "--changed tells an existing .al source file from a diff file (or reports why it found nothing)",
    "acceptance": "the reproducer returns posting.al's routines (as --file does), or exits with a diagnostic naming the misdetection; a diff file passed to --changed is still read as a diff."
  }
]
```

## Not delivered

- Absolute/rooted paths and `..` segments are still not matched (out of scope; same as today; pinned by T1n).
- The pre-existing `\` → `/` mapping can collide a POSIX file with a literal `\` in its name with a nested path. This change WIDENS it slightly: such a name also loses `.` and empty segments, e.g. `src\.\x.al` now keys like `src/x.al`. Windows forbids `\` in file names; on POSIX such names are outside this change's guarantee.
