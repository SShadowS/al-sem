# Issue #111 ledger: attempt 1 (`digest --diff` reads plain `diff -u` headers)

## Claim

```json
{"run_id":"20261021-125904-059382","issue":111,"attempt":1,"session":"claude-code-session:c8325c25-f1bf-436d-a204-90152a41120c","branch":"issue/111-agent-discovery-l5-digest-diff-a1","worktree":"/work/al-sem-issue-111-a1","body_hash":"641a7734dc36bb38d2a6cdde5bbc4ea87cf092ba31653ac85a8c7887c4ced09e","reconciled":[]}
```

Issue body hash: `641a7734dc36bb38d2a6cdde5bbc4ea87cf092ba31653ac85a8c7887c4ced09e`.
Base `617b402f853664cf1cec8d049d85561d66ac8aa2` (make-worktree, origin/master). Grammar `tree-sitter-al` `4d75846c`.
Reviewers: the operator's standing stand-ins, gpt-6.1-sol (slot astra) and Opus (slot flash); marks keyed `sol` /
`opus`; attest with `--substitute astra=sol --substitute flash=opus`. Loop budget: this is claim 1 of 10 (operator,
2026-10-21).

## Classification

**Bounded.** The defect is local to one function, `extract_path` in `src/engine/l5/diff_parser.rs` (~:165-175): it
takes everything after `--- ` / `+++ ` as the path, so a plain `diff -u` header `<path><TAB><timestamp>` keeps the
timestamp glued to the path, and `/dev/null<TAB><timestamp>` is not seen as a deletion. One parser rule and a CLI test
cover it; no other subsystem changes.

## Assumption probes

All on base `617b402f`, a dev build of `alsem`, workspace `tests/r0-corpus/ws-d8-commit-in-tx`, diffs made with GNU
`diff` in `.agent/runs/20261021-125904-059382/probe/` (outputs `*.diff` / `*.out` there). "Timestamp-free form" means
the same `diff -u` run with `--label`, so its headers carry no TAB.

| probe | result | evidence |
|---|---|---|
| P1 A plain `diff -u` change header is `<path><TAB><timestamp>` | held | `--- ./old/src/posting.al^I2026-10-21 13:02:30… +0000` / `+++ ./src/posting.al^I…` (`cat -A`, `p-plain.diff`) |
| P2 The timestamp breaks file matching (the issue's reproducer) | held | `digest --diff p-plain.diff` → exit 2, `changedRootsDiagnostics: [{"file":"./src/posting.al\t2026-10-21 13:02:06… +0000","kind":"diff-file-unmatched"}]`, 0 entries; the git-style `p-git.diff` (`a/`/`b/`, no TAB) → exit 0, 1 entry |
| P3 A timestamped `/dev/null` is not seen as a deletion | held | `diff -u ./src/subscriber.al /dev/null` → `+++ /dev/null^I2026-10-21 10:30:23… +0000`; digest → `diff-file-unmatched` on `"/dev/null\t2026-…"`; the timestamp-free `d-dot.diff` → `diff-file-deleted` on `./src/subscriber.al` (and the git form `d-git.diff` on `src/subscriber.al`) |
| P4 A `./` prefix needs no new rule | held | timestamp-free `p-dot.diff` (`--- ./old/src/posting.al`, `+++ ./src/posting.al`) → exit 0, 1 entry, identical to the git form; so only the TAB part is missing |
| P5 `diff -ru` adds nothing else to handle | held | `diff -ru ../old/src ./src` (`ru-plain.diff`) adds a `diff -ru <old> <new>` line and an `Only in ../old/src: subscriber.al` line; digest's only diagnostic is the same timestamped `diff-file-unmatched`, so both extra lines are already ignored |
| P6 Where the path is cut today | held | `extract_path` (`src/engine/l5/diff_parser.rs` ~:165-175): `rest = line[4..].trim_start()`, `/dev/null` compared with `rest` / `rest.trim_end()`, then `unquote_path(rest)` and `strip_ab_prefix`: nothing stops at a TAB |
| P7b Git's real header bytes (spec panel r1: "asserted, not shown") | held | scratch repo, git 2.53.0, `git diff \| cat -A`, the same with `-c core.quotePath=false`: a name with a TAB is C-quoted with an ESCAPED `\t`, `--- "a/with\ttab.al"$` (no raw TAB byte, both settings); a name with a space gets a trailing raw TAB, `--- a/with space.al^I$`; a plain name has none, `--- a/plain.al$` |
| P7c GNU diff's real header bytes (spec panel r1, opus #1: "GNU does not quote" is false) | held | scratch dirs, GNU diffutils 3.12, `diff -ru o n \| cat -A`: a name with a space or a TAB is C-quoted and the TAB follows the closing quote: `--- "o/sp ace.al"^I<time>$`, `--- "o/t\tab.al"^I<time>$`; a deletion is only `Only in o: del.al$`; with `-ruN` a deletion is the real path with the epoch time, `+++ n/del.al^I1970-01-01 00:00:00.000000000 +0000$` and `@@ -1 +0,0 @@`, never `/dev/null` |
| P7 A raw TAB never belongs to a path in these headers | held (P7b, P7c) | In the header WIRE FORMAT of both producers a raw TAB byte only ever ends the path: GNU writes `<path or "quoted path"><TAB><time>`; git writes a raw TAB only after an unquoted name containing a space; both C-quote a name that contains a TAB, so the name's own TAB is the escaped two-character `\t`, decoded by `unquote_path` AFTER the cut. Supported: file names containing a TAB (via quoting), on current GNU diff and git. Residual: a producer that writes a TAB-containing name raw and unquoted (an old `diff` without quoting) would be cut at that TAB |
| P8 Stale comment | held | `tests/cli/cli_digest_changed_spans.rs:2354-2356` says plain `diff -u` "appends a tab and a timestamp, which the parser does not strip" |

## Design

`extract_path` ends the header at the first raw TAB, before the `/dev/null` test and before unquoting:
`let rest = rest.split_once('\t').map_or(rest, |(path, _)| path);`. In both producers' wire format a raw TAB only ever
ends the path (P7, shown on git 2.53.0 and GNU diffutils 3.12); a name's own TAB is quoted as the escaped two
characters `\t` and decoded by `unquote_path` after the cut. Everything after it is unchanged: the `/dev/null` test (now
`/dev/null<TAB><time>` is a deletion, and `--- /dev/null<TAB><time>` an addition), `unquote_path` (now also given GNU's
quoted form `"./src/a b.al"` without the trailing `<TAB><time>` it cannot unquote today), the `\` → `/` normalisation and
`strip_ab_prefix`. A one-line code comment states the supported wire format and the residual (P7). No other function
changes: the `diff --git` and `diff -ru` lines, hunk headers, `Only in` lines, CRLF (stripped before `extract_path`) and
spaces before the TAB (trimmed) are already handled or ignored (P5; panel D2).

Tests. CLI tests run through the `alsem digest` binary via the existing `digest()` helper
(`tests/cli/cli_digest_changed_spans.rs:83`), so they pin the USE; parser unit tests sit next to the existing ones in
`diff_parser.rs`. Every test asserts an EXACT expected result, never only "equal to the other form" (which passes when
both are empty):
- **C1** a plain `diff -u` change with timestamped headers (the `p-plain` shape on `ws-d8-commit-in-tx`): exit 0, the
  changed line roots exactly `PostSalesDoc`, entries non-empty, no `changedRootsDiagnostics`; and the same document as
  the timestamp-free form. Break: drop the TAB cut → `diff-file-unmatched`, exit 2.
- **C2** a timestamped `/dev/null` deletion (`d-plain`): exit 2, exactly one diagnostic `{kind: "diff-file-deleted",
  file: "./src/subscriber.al"}`, no `diff-file-unmatched`; equal to the timestamp-free form. Break: drop the TAB cut.
- **C3** a `diff -ru` diff with its `diff -ru` line (`ru-plain`): the same exact expectations as C1. Break: drop the cut.
- **C4** GNU's quoted form with a timestamp, `--- "./src/a b.al"<TAB><time>` (a fixture file with a space in its
  name): matched, exit 0. This is the shape where cutting BEFORE unquoting matters. Break: cut AFTER `unquote_path`
  instead of before → unmatched.
- **U1** parser unit tests: `--- /dev/null<TAB><time>` with a real `+++` path is an ADDED file; CRLF line endings and
  spaces before the TAB give the same path; path, kind and hunks asserted.
- **U2** git's own forms keep their path: `--- a/with space.al<TAB>` (git's trailing TAB) and `--- "a/with\ttab.al"`
  (escaped TAB, decoded to a TAB in the name). These pass with or without the cut, so their breaks target what each
  claims: for the space case, cut at the first SPACE instead of TAB (U2 space case fails); for the tab case, cut AFTER
  `unquote_path` (the same break as C4: the decoded TAB is then cut and the name becomes `a/with`, so U2's claim that
  the TAB rule leaves git's escaped `\t` alone is what fails). Dropping the C-unescape of `\t` may be run as a second
  break, but it only shows the test reads the decoded name.
- The stale comment at `tests/cli/cli_digest_changed_spans.rs:2354-2356` is updated (P8).
Fixtures: the exact header bytes captured from git 2.53.0 and GNU diffutils 3.12 (P7b, P7c) are written into the tests
as literals, so no test depends on the host's `diff`/`git` version.

Non-goals (none is needed for the acceptance): normalising `./` to `a/`/`b/` in the `DiffFileDeleted` name (the
timestamp-free form keeps `./`, P3/P4); treating `Only in …` lines as deletions, and recognising `diff -ruN`'s
epoch-dated real-path deletion as a deletion (after this change it reads as a modified file with every line deleted;
filed as a discovery); a git name with a trailing space losing it to `unquote_path`'s trim (pre-existing, unchanged;
filed as a discovery).

Expected moves: none in any golden (no golden diff carries a TAB header); CDO unaffected (the change is in `--diff`
parsing only).

## Base/Head SHAs

- make-worktree base `617b402f853664cf1cec8d049d85561d66ac8aa2`; step 11 B = `617b402f853664cf1cec8d049d85561d66ac8aa2`
  (origin/master, unmoved), H = `042764fe8c4c2f679f2c77d8d22b9f1cc503123e`.

## Timeline

- make-worktree: `{"worktree": "/work/al-sem-issue-111-a1", "branch": "issue/111-agent-discovery-l5-digest-diff-a1", "base": "617b402f853664cf1cec8d049d85561d66ac8aa2", "target": null}`
- classify: bounded
- probes P1-P7 (conductor, read-only + a dev build): the TAB-glued timestamp is the only defect; `./` and `diff -ru`'s
  extra lines are already handled. `## Design` written (it is the spec for a bounded issue).
- Operator, mid-tick (2026-10-21): "#160 is highest priority"; chose to finish #111 first, then #160 (step 7b) next.
- spec panel round 1 (`panel-review`): `charge spec_rounds` → 4; `charge pi_calls` → 19; `charge subagents` → 59.
  Briefing `.agent/runs/20261021-125904-059382/spec-panel-r1-briefing.md` → `pi-sol-spec-r1.md`, `opus-spec-r1.md`.
  Both confirm the one-line fix (opus D1-D4 CONFIRM; sol D2/D4 CONFIRM, D1/D3 REJECT for evidence and test strength).
  10 entries: S1-1 to S1-5 important (vacuous equality; one break for all tests; P7 unshown and GNU-quoting claim false;
  GNU quoted+timestamp case missing; diff -ruN deletion scope), S1-6 to S1-10 minor. Conductor verified the git and GNU
  header bytes (P7b, P7c) and the stale comment (P8), and revised the Design; S1-1..S1-9 fixed, S1-10 (pre-existing
  trim) deferred to a discovery. Design-section hash `268a53b306a6e3f1`.
- spec panel round 2: `charge spec_rounds` → 3; `charge pi_calls` → 18; `charge subagents` → 58. Both accepted all 10;
  opus raised N1 (minor: U2's tab-case break tests `unquote_path`, not the cut) → S2-1, fixed by the conductor (break:
  cut after `unquote_path`). Design hash `f2d7215c6520765b`.
- spec panel round 3: `charge spec_rounds` → 2; `charge pi_calls` → 17; `charge subagents` → 57. Both accepted all 11
  entries, no new finding. **Spec panel converged** (S1-1..S1-9 and S2-1 fixed, S1-10 deferred minor).
- step 6 acceptance tests: opus subagent dispatched (it writes the red tests, stops for the conductor, then
  implements as step 8's implementer, keeping its context); `charge subagents` → 56.
- step 7 plan: `docs/superpowers/plans/2026-10-21-issue-111-digest-diff-tab-headers.md`, one task. Written by the
  conductor directly, NOT through `superpowers:writing-plans` (a one-task bounded fix; recorded as a substitution).
  `charge plan_tasks` → 11.
- step 6 result (uncommitted; `tests/cli/cli_digest_changed_spans.rs` +112, `diff_parser.rs` +52 tests only; logs
  `a1-p1-lib.log`, `a1-p1-cli.log`, both exit 101 as expected): header bytes as literals from P1/P3/P5/P7b/P7c; C4 builds
  its workspace in a temp dir (d8 copied, `posting.al` renamed `src/a b.al`), so no corpus fixture and no golden moves.
  - C1 `plain_diff_u_timestamped_headers_match` (cli:2402) FAIL: diagnostics `[{"file":"./src/posting.al\t2026-10-21
    13:02:06… +0000","kind":"diff-file-unmatched"}]`.
  - C2 `plain_diff_u_timestamped_dev_null_is_a_deletion` (cli:2418) FAIL: left `[{"file":"/dev/null\t2026-10-21
    10:30:23… +0000","kind":"diff-file-unmatched"}]`, right `[{"file":"./src/subscriber.al","kind":"diff-file-deleted"}]`.
  - C3 `plain_diff_ru_timestamped_headers_match` (cli:2452) FAIL: the same unmatched-with-timestamp diagnostic; the
    `diff -ru` and `Only in` lines add none.
  - C4 `gnu_quoted_timestamped_header_matches` (cli:2469) FAIL: `"\"./src/a b.al\"\t2026-…"` unmatched.
  - U1 `plain_diff_u_timestamped_dev_null_is_an_addition` (diff_parser.rs:955, 4 cases: LF/CRLF × with/without spaces
    before the TAB; path, kind Added, old_path None, hunk, changes, no errors) FAIL: path keeps `\t2026-…`.
  - U2 `git_headers_keep_their_path_under_the_tab_cut` (diff_parser.rs:984) PASS by design (git's space+TAB form →
    `with space.al`; escaped `"a/with\ttab.al"` → `with<TAB>tab.al`).
- step 8 implement: `halt-check` → not halted; phase 2 sent to the same opus agent (context intact); `charge
  subagents` → 55; `charge task_attempts --sub task1` → 2.
- Task 1 committed `a921bd7ece3ac8be124d00ee34cf0641b0df78b9` (diff_parser.rs, cli_digest_changed_spans.rs,
  CHANGELOG.md, the plan): `extract_path` cuts at the first TAB before the `/dev/null` test and `unquote_path`, with a
  two-line wire-format/residual comment; the #76 T3 comment updated. Green: `diff_parser::tests` 34 passed,
  `cli_digest_changed_spans::` 47 passed. Gate `task-affected` exit 0, 3595/3595, 8 skipped (`a1-gate-task-affected.log`).
  `check-diff --base origin/master` → `{"reasons": []}`. Sonnet reviewer dispatched (`charge subagents` → 54); the
  supervised `ci-steps-task` gate run in parallel (the reviewer is read-only, no cargo).
- Task review (sonnet): no critical or important finding; every proof break confirmed from the logs; minor: the comment
  at diff_parser.rs:168 credits git with a timestamp (folded into the final panel). Supervised `ci-steps-task` → exit 0,
  46.0 s, 3595/3595 (`logs/ci-steps-task/20261021T132304-eb79e33d.log`); tree clean.
- step 9: `push-branch` → head `a921bd7e`; `pr-create --draft` → **PR #187**. No bench: the change touches no measured
  code path. CHANGELOG entry present; no golden moved.
- step 10 final panel round 1: built-in `code-review` at high (`code-review-high-r1.md`); `charge final_rounds` → 4;
  `charge pi_calls` → 16; `charge subagents` → 53. Sol (`pi-sol-final-r1.md`): I1/I2 CONFIRM, two minors. Opus
  (`opus-final-r1.md`): I1/I2/I3 CONFIRM, two minors. code-review: the fix is right, but now `diff -ru` succeeds while
  `Only in` lines (added/deleted files) are silently ignored, where before the same diff failed loudly (verified at
  diff_parser.rs:759), and C3 locks that in; plus the CHANGELOG over-claims for `diff -ru old new` from a parent folder.
  Register F1-1..F1-10 (F1-1, F1-2 important). F1-5 refuted (the ledger is committed at step 12), F1-10 fixed by the
  conductor (Discoveries filled). The rest sent to the implementer: report `Only in` lines as `diff-file-unmatched`, C3
  updated, comments, module doc, CHANGELOG; `charge subagents` → 52.
- Round-1 fixes committed `042764fe8c4c2f679f2c77d8d22b9f1cc503123e` (diff_parser.rs, digest_cli.rs,
  cli_digest_changed_spans.rs, CHANGELOG.md): `DiffParseResult.only_in`, the `Only in <dir>: <name>` arm, digest reports
  each as `DiffFileUnmatched`; C3 pins it; module doc and comments; CHANGELOG scope. Gate `task-affected` exit 0,
  3596/3596 (`a1-r1-gate-task-affected.log`). `check-diff` → `{"reasons": []}`. Supervised `ci-steps-task` → exit 0,
  58.6 s, 3596/3596 (`logs/ci-steps-task/20261021T133341-754ba3ce.log`); tree clean.
- Final panel round 2: `charge final_rounds` → 3; `charge pi_calls` → 15; `charge subagents` → 51. Sol and opus accepted
  F1-1..F1-10 and confirmed no S* regresses; opus N1 (stale discovery text) → F2-1, fixed by the conductor; opus N2 (`Only
  in` arm reads a format-patch commit-message line) → F2-2, deferred as a stated known limit (`## Not delivered`).
- Final panel round 3: `charge final_rounds` → 2; `charge pi_calls` → 14; `charge subagents` → 50. Both accepted F2-1 and
  F2-2, no new finding. **Final panel converged**: 23 entries, all accepted by both; 19 fixed (7 important), 1 refuted,
  3 minor deferred (S1-10, F1-3, F2-2), no blocking entry deferred.
- step 11: `origin/master` still `617b402f` (no rebase; range-diff empty). **B = `617b402f853664cf1cec8d049d85561d66ac8aa2`,
  H = `042764fe8c4c2f679f2c77d8d22b9f1cc503123e`.** Full gate set: all four exit 0, supervised (Gate results).
- step 12: protected-path check over B..H, evidence sanitized and committed after H, `freeze-check`, `attest` with
  `--substitute astra=sol --substitute flash=opus`.

## Acceptance matrix

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| C1 `plain_diff_u_timestamped_headers_match` | B1 drop the TAB cut (grep-checked) | `diff-file-unmatched` on `./src/posting.al\t2026-10-21 13:02:06… +0000` (`a1-proof-B1-*.log`) | ok (`a1-proof-B1-revert-*.log`) | a921bd7e |
| C2 `plain_diff_u_timestamped_dev_null_is_a_deletion` | B1 | `diff-file-unmatched` on `/dev/null\t2026-10-21 10:30:23… +0000` | ok | a921bd7e |
| C3 `plain_diff_ru_timestamped_headers_match` | B1 | `diff-file-unmatched` on `./src/posting.al\t2026-10-21 13:03:28… +0000` | ok | a921bd7e |
| C4 `gnu_quoted_timestamped_header_matches` | B2 cut AFTER `unquote_path` | `diff-file-unmatched` on `"./src/a b.al"` (quotes kept, not unquoted) (`a1-proof-B2-*.log`) | ok | a921bd7e |
| U2 tab case `git_headers_keep_their_path_under_the_tab_cut` | B2 | left `"with"`, right `"with\ttab.al"` | ok | a921bd7e |
| U2 space case | B3 cut at the first space instead of TAB | :997 left `"with"`, right `"with space.al"` (`a1-proof-B3-*.log`) | ok | a921bd7e |
| U1 `plain_diff_u_timestamped_dev_null_is_an_addition` | B4 move the cut after the `/dev/null` test (also fails under B1) | :972 left `Modified`, right `Added` (`a1-proof-B4-*.log`) | ok | a921bd7e |

## Findings register summary

## Gate results

Step 11 on H `042764fe8c4c2f679f2c77d8d22b9f1cc503123e` (B `617b402f`), each `supervised: true`, `queued_s` 0.0, grammar
`tree-sitter-al` `4d75846c`, toolchain `rustc 1.96.0`:

| gate | exit | seconds | log | notes |
|---|--:|--:|---|---|
| `ci-steps-all` | 0 | 292.8 | `logs/ci-steps-all/20261021T133940-b642fa37.log` | nextest 3596 run / 3596 passed / 8 skipped |
| `check-goldens-coverage` | 0 | 0.0 | `logs/check-goldens-coverage/20261021T134433-bee12827.log` | coverage OK |
| `check-goldens` | 0 | 0.1 | `logs/check-goldens/20261021T134433-74d1b173.log` | tree stamp from `ci-steps-all`'s own test step on H |
| `cdo-gate` | 0 | 278.9 | `logs/cdo-gate/20261021T134433-d6edd65a.log` | `ENFORCE_CDO_WS=1`; `--lib` 2181 passed, 0 failed, 4 ignored; `cdo-gate: PASS` |

Tree clean outside `.agent/`. No golden moved. No bench (no measured code path). CDO workspace identity: sha256(path)
prefix `4429175c44dc4ad6`, at `bc3ccb18`.

## Discoveries

Filed by `/orchestrate` step 10 after the merge (both are non-goals of #111, register S1-5 and S1-10).

```json
[
  {"subsystem": "l5 / digest (diff parser: directory-diff deletions)", "locator": "src/engine/l5/diff_parser.rs (header and hunk parsing; the `Only in` arm records `<dir>/<name>` in `only_in`, which digest reports as diff-file-unmatched)", "symptom": "A GNU directory diff never yields DiffFileDeleted or an added file: plain `diff -ru` reports an added or deleted file only as an `Only in <dir>: <name>` line, which (since #111) is reported as diff-file-unmatched rather than mapped to a deletion or an addition, and `diff -ruN` writes the real path with the epoch time (`+++ n/del.al<TAB>1970-01-01 00:00:00.000000000 +0000`, `@@ -1 +0,0 @@`), which reads as a modified file with every line deleted.", "kind": "enhancement", "origin_issue": 111, "reproducer": "GNU diffutils 3.12: diff -ruN old new where old has a file new lacks -> alsem digest <ws> --diff that.diff: the file is a modified file with all lines deleted, not diff-file-deleted; diff -ru (no -N): the file is reported as diff-file-unmatched on `<dir>/<name>`, not diff-file-deleted.", "pre_existing": true, "capability": "digest --diff treats a `diff -ruN` epoch-dated, all-lines-deleted file, and an `Only in` line for a workspace file, as a deletion", "acceptance": "both shapes yield diff-file-deleted for the file, like a /dev/null deletion"},
  {"subsystem": "l5 / digest (diff parser: unquote_path)", "locator": "src/engine/l5/diff_parser.rs unquote_path (~:104, `s.trim()`)", "symptom": "unquote_path trims its input, so an unquoted git path whose file name ends in a space (git writes `--- a/name <TAB>`) loses that trailing space and cannot match the file.", "kind": "bug", "origin_issue": 111, "reproducer": "a workspace file whose name ends in a space, `src/x.al ` (git allows it), changed in git -> header `--- a/src/x.al <TAB>` -> digest reads the path as `src/x.al` and reports it unmatched.", "pre_existing": true, "capability": "a git header path keeps its trailing spaces (the trailing TAB, now cut, already marks the end)", "acceptance": "a header for a name ending in a space matches that file"}
]
```

## Not delivered

- Non-goals of the converged design: `Only in` lines mapped to deletions or additions (they are reported as
  `diff-file-unmatched`), `diff -ruN` epoch-dated deletions, and a git name with a trailing space (both filed as
  discoveries); a `diff -ru old new` run from a parent folder (tree-root folder `n/src/…`) is still unmatched.
- Known limit (final panel r2, opus N2): the `Only in` arm reads any top-level line, so an `Only in X: Y` line in a
  `git format-patch` commit message adds a false `diff-file-unmatched`. It is loud (a diagnostic), never silent, and
  does not change roots.
