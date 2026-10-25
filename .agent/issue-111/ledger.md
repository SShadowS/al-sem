# Issue #111: digest --diff reads plain `diff -u` headers (attempt 2)

Claim: run `20261025-152316-6a15b1`, attempt 2, branch `issue/111-agent-discovery-l5-digest-diff-a2`, worktree `/work/al-sem-issue-111-a2`, base `b229d97051835ac3e8ad606961b6749cd5b0579e`, body_hash `641a7734dc36bb38d2a6cdde5bbc4ea87cf092ba31653ac85a8c7887c4ced09e`.

Reviewers: gpt-6.1-sol (slot astra) and Opus (slot flash). Attest with `--substitute astra=sol --substitute flash=opus`. heavy_gates=2. Merges pre-approved. The operator unblocked this issue for a fresh attempt on current master, ahead of the v2.1.1 window on the evening of 2026-10-26. The old branch `issue/111-agent-discovery-l5-digest-diff-a1` is reference only.

## Timeline
- 2026-10-25 15:24 UTC: claimed. make-worktree reported "seeded": true. The first `cargo build --bin alsem` took 32 s.
- Attempt 1 (branch a1, base 617b402f) took the whole flow to green gates. It converged on the spec panel and on the final panel (r1 fixes in 042764fe). It was blocked only by an unrelated `perf_bounds` CI flake: `compute_summaries_v2_within_bound`, with samples from 147 to 410 ms.
- This attempt carries attempt 1's two code commits forward by cherry-pick onto b229d970:
  - a921bd7e becomes a4567174;
  - 042764fe becomes 970de26a.
  The only conflicts were in CHANGELOG.md. The #111 entry moved to `Unreleased` → `Fixed`, using attempt 1's final wording, and the old release section was left untouched.
- The task gate (`ci-steps task`) exited 0 on 970de26a, and check-diff was clean.
- Spec panel and final panel for this attempt: see below.
- Spec r1 + final r1 (combined): sol APPROVE/ship; Opus APPROVE/ship with minor OP1; code-review high raised cr1-cr9 → H-1..H-9 (`.agent/runs/20261025-152316-6a15b1/r1-merged.md`). Fixes in 37dc7896.
- Final r2 (diff-only): both re-raised H-4 (CHANGELOG still said "never silent"); Opus F1 (stale ledger :81/:84). Fixed in 7b3d5654 + ledger. Final r3 (diff-only): both ship; register 11 entries, all accepted by both. Opus nit (CHANGELOG :22 long line) left as is, to keep the gated head.
- Gates on H = 7b3d5654: check-goldens-coverage 0, ci-steps-all 0, check-goldens 0, cdo-gate 0 (records in the run's gate-runs.jsonl). B = b229d970.

## Classification
Bounded. The defect is local to `extract_path` in `src/engine/l5/diff_parser.rs`. It takes everything after `--- `/`+++ ` as the path, so a plain `diff -u` header `<path><TAB><timestamp>` keeps the timestamp glued to the path. One parser rule and CLI tests cover it.

## Assumption probes (carried from attempt 1, copied verbatim for OP1; still valid on b229d970 because `diff_parser.rs` is unchanged since 617b402f, as the attempt 2 panel confirmed)

All probes ran on base `617b402f` with a dev build of `alsem`, on workspace `tests/r0-corpus/ws-d8-commit-in-tx`. The diffs were made with GNU `diff` in `.agent/runs/20261021-125904-059382/probe/`; the `*.diff` and `*.out` outputs are there. "Timestamp-free form" means the same `diff -u` run with `--label`, so its headers carry no TAB.

| probe | result | evidence |
|---|---|---|
| P1 A plain `diff -u` change header is `<path><TAB><timestamp>` | held | `--- ./old/src/posting.al^I2026-10-21 13:02:30… +0000` / `+++ ./src/posting.al^I…` (`cat -A`, `p-plain.diff`) |
| P2 The timestamp breaks file matching (the issue's reproducer) | held | `digest --diff p-plain.diff` → exit 2, `changedRootsDiagnostics: [{"file":"./src/posting.al\t2026-10-21 13:02:06… +0000","kind":"diff-file-unmatched"}]`, 0 entries. The git-style `p-git.diff` (`a/`/`b/`, no TAB) → exit 0, 1 entry |
| P3 A timestamped `/dev/null` is not seen as a deletion | held | `diff -u ./src/subscriber.al /dev/null` → `+++ /dev/null^I2026-10-21 10:30:23… +0000`. Digest → `diff-file-unmatched` on `"/dev/null\t2026-…"`. The timestamp-free `d-dot.diff` → `diff-file-deleted` on `./src/subscriber.al`, and the git form `d-git.diff` on `src/subscriber.al` |
| P4 A `./` prefix needs no new rule | held | Timestamp-free `p-dot.diff` (`--- ./old/src/posting.al`, `+++ ./src/posting.al`) → exit 0, 1 entry, identical to the git form. So only the TAB part is missing |
| P5 `diff -ru` adds nothing else to handle | held | `diff -ru ../old/src ./src` (`ru-plain.diff`) adds a `diff -ru <old> <new>` line and an `Only in ../old/src: subscriber.al` line. Digest's only diagnostic is the same timestamped `diff-file-unmatched`, so both extra lines are already ignored |
| P6 Where the path is cut today | held | `extract_path` (`src/engine/l5/diff_parser.rs` ~:165-175): `rest = line[4..].trim_start()`, `/dev/null` is compared with `rest` / `rest.trim_end()`, then `unquote_path(rest)` and `strip_ab_prefix`. Nothing stops at a TAB |
| P7b Git's real header bytes (spec panel r1: "asserted, not shown") | held | Scratch repo, git 2.53.0, `git diff \| cat -A`, also with `-c core.quotePath=false`. A name with a TAB is C-quoted with an ESCAPED `\t`, `--- "a/with\ttab.al"$`, with no raw TAB byte under either setting. A name with a space gets a trailing raw TAB, `--- a/with space.al^I$`. A plain name has none, `--- a/plain.al$` |
| P7c GNU diff's real header bytes (spec panel r1, opus #1: "GNU does not quote" is false) | held | Scratch dirs, GNU diffutils 3.12, `diff -ru o n \| cat -A`. A name with a space or a TAB is C-quoted, and the TAB follows the closing quote: `--- "o/sp ace.al"^I<time>$`, `--- "o/t\tab.al"^I<time>$`. A deletion appears only as `Only in o: del.al$`. With `-ruN` a deletion is the real path with the epoch time, `+++ n/del.al^I1970-01-01 00:00:00.000000000 +0000$` and `@@ -1 +0,0 @@`, never `/dev/null` |
| P7 A raw TAB never belongs to a path in these headers | held (P7b, P7c) | In the header wire format of both producers, a raw TAB byte only ever ends the path. GNU writes `<path or "quoted path"><TAB><time>`. Git writes a raw TAB only after an unquoted name containing a space. Both C-quote a name that contains a TAB, so the name's own TAB is the escaped two-character `\t`, which `unquote_path` decodes AFTER the cut. Supported: file names containing a TAB (via quoting), on current GNU diff and git. Residual: a producer that writes a TAB-containing name raw and unquoted, such as an old `diff` without quoting, would be cut at that TAB |
| P8 Stale comment | held | `tests/cli/cli_digest_changed_spans.rs:2354-2356` says plain `diff -u` "appends a tab and a timestamp, which the parser does not strip" |

## Design (carried from attempt 1, copied verbatim for OP1; re-reviewed and approved by both reviewers in attempt 2's spec round 1)

`extract_path` ends the header at the first raw TAB, before the `/dev/null` test and before unquoting:
`let rest = rest.split_once('\t').map_or(rest, |(path, _)| path);`.

In both producers' wire format a raw TAB only ever ends the path (P7, shown on git 2.53.0 and GNU diffutils 3.12). A name's own TAB is quoted as the two escaped characters `\t`, and `unquote_path` decodes it after the cut.

Everything after the cut is unchanged:
- the `/dev/null` test: `/dev/null<TAB><time>` is now a deletion, and `--- /dev/null<TAB><time>` an addition;
- `unquote_path`: it now also receives GNU's quoted form `"./src/a b.al"` without the trailing `<TAB><time>`, which it could not unquote before;
- the `\` → `/` normalisation;
- `strip_ab_prefix`.

A one-line code comment states the supported wire format and the residual (P7).

No other function changes. These are already handled or ignored (P5; panel D2):
- the `diff --git` and `diff -ru` lines;
- hunk headers;
- `Only in` lines;
- CRLF, which is stripped before `extract_path`;
- spaces before the TAB, which are trimmed.

**Tests.** CLI tests run through the `alsem digest` binary via the existing `digest()` helper (`tests/cli/cli_digest_changed_spans.rs:83`), so they pin the use. Parser unit tests sit next to the existing ones in `diff_parser.rs`. Every test asserts an EXACT expected result, never only "equal to the other form", which would pass if both were empty.
- **C1:** a plain `diff -u` change with timestamped headers (the `p-plain` shape on `ws-d8-commit-in-tx`). Expect exit 0, changed line roots exactly `PostSalesDoc`, entries non-empty, no `changedRootsDiagnostics`, and the same document as the timestamp-free form. Break: drop the TAB cut → `diff-file-unmatched`, exit 2.
- **C2:** a timestamped `/dev/null` deletion (`d-plain`). Expect exit 2, exactly one diagnostic `{kind: "diff-file-deleted", file: "./src/subscriber.al"}`, no `diff-file-unmatched`, and equality with the timestamp-free form. Break: drop the TAB cut.
- **C3:** a `diff -ru` diff with its `diff -ru` line (`ru-plain`), with the same exact expectations as C1. Break: drop the cut.
- **C4:** GNU's quoted form with a timestamp, `--- "./src/a b.al"<TAB><time>`, on a fixture file with a space in its name. Expect it to match, exit 0. This is the shape where cutting BEFORE unquoting matters. Break: cut AFTER `unquote_path` instead of before → unmatched.
- **U1:** parser unit tests. `--- /dev/null<TAB><time>` with a real `+++` path is an ADDED file. CRLF line endings and spaces before the TAB give the same path. Path, kind and hunks are asserted.
- **U2:** git's own forms keep their path: `--- a/with space.al<TAB>` (git's trailing TAB) and `--- "a/with\ttab.al"` (an escaped TAB, decoded to a TAB in the name). These pass with or without the cut, so each break targets what that case claims:
  - space case: cut at the first SPACE instead of the TAB, and the space case fails;
  - tab case: cut AFTER `unquote_path`, the same break as C4. The decoded TAB is then cut and the name becomes `a/with`, so U2's claim that the TAB rule leaves git's escaped `\t` alone fails.
  - Dropping the C-unescape of `\t` may be run as a second break, but it only shows that the test reads the decoded name.
- The stale comment at `tests/cli/cli_digest_changed_spans.rs:2354-2356` is updated (P8).

**Fixtures:** the exact header bytes captured from git 2.53.0 and GNU diffutils 3.12 (P7b, P7c) are written into the tests as literals, so no test depends on the host's `diff` or `git` version.

**Non-goals** (the acceptance needs none of them):
- normalising `./` to `a/`/`b/` in the `DiffFileDeleted` name; the timestamp-free form keeps `./` (P3/P4);
- treating `Only in …` lines as deletions;
- recognising `diff -ruN`'s epoch-dated real-path deletion as a deletion. After this change it reads as a modified file with every line deleted. Filed as a discovery;
- a git name with a trailing space losing that space to `unquote_path`'s trim. Pre-existing and unchanged. Filed as a discovery.

**Expected moves:** none in any golden, because no golden diff carries a TAB header. CDO is unaffected, because the change is in `--diff` parsing only.

Attempt 1's final panel added one change: `diff -r`'s `Only in <dir>: <name>` lines are reported as `diff-file-unmatched` on `<dir>/<name>`, through a new `DiffParseResult::only_in`; since attempt 2's H-1, a `<dir>/<name>` that is a workspace file is instead a whole-file hit. This supersedes the "already ignored" wording in P5 and in the Design above. Both reviewers confirmed this in attempt 2's spec round 1 (SP2).

## Discoveries
- A directory diff gives no `DiffFileDeleted` or added file. An `Only in` file that only the old tree has, or an `Only in` directory, is reported as unmatched (a new-tree workspace file is a whole-file hit), and a `diff -ruN` deletion reads as a modified file with every line deleted. Pre-existing; a non-goal.
- `unquote_path` trims a git name's trailing space. Pre-existing; a non-goal.
- The `perf_bounds` `compute_summaries_v2_within_bound` check is noisy on the CI runner. Pre-existing. It is what blocked attempt 1.

## Round 1 fixes (attempt 2)

Findings: `.agent/runs/20261025-152316-6a15b1/r1-merged.md`. Head before: 970de26a.

| id | disposition | where |
|---|---|---|
| H-1 | fixed. Each `only_in` path goes through `normalize_input` + `spans.of_file`, like a header path. A hit is a whole-file hit (as the hunkless-rename branch), skipped when the path is poisoned; a miss stays `diff-file-unmatched` | `src/engine/l5/digest_cli.rs` `resolve_changed_roots` (~:515-526); tests `only_in_workspace_file_is_a_whole_file_hit`, `only_in_path_outside_the_workspace_stays_unmatched` in `tests/cli/cli_digest_changed_spans.rs` (~:2528-2572) |
| H-2 | fixed. `<dir>` loses its trailing `/` before the join, so `Only in new/: x.al` is `new/x.al` | `src/engine/l5/diff_parser.rs` ~:779; unit test `only_in_dir_with_trailing_slash_has_one_separator` (~:1054) |
| H-3 | fixed. `[Unreleased]` `### Changed` holds the `Only in` reading and the H-1 lookup, with H-4, H-5, H-8, H-9 as "Not covered". `### Fixed` keeps the TAB-header fix, with H-6 and H-7 in its "Not covered" | `CHANGELOG.md` :10-43 |
| H-4 | recorded. `Only in` on any top-level line (commit message) gives a loud stray diagnostic, never changes roots unless the path is a workspace file | CHANGELOG Changed, Not covered |
| H-5 | recorded. A directory `Only in` is one unmatched entry | CHANGELOG Changed, Not covered |
| H-6 | recorded. `diff -ruN` epoch-dated deletions are not deletions | CHANGELOG Fixed, Not covered |
| H-7 | recorded. `strip_ab_prefix` on plain GNU paths drops a top folder named `a` or `b` | CHANGELOG Fixed, Not covered |
| H-8 | recorded. A dir containing `: ` splits at the wrong place | CHANGELOG Changed, Not covered |
| H-9 | recorded. `Only in` misses reuse `diff-file-unmatched` and come after the per-file diagnostics (accepted design); after H-1 only real misses are listed | CHANGELOG Changed, Not covered |

Proof rows (each break made with Edit, its text grep-counted once, then reverted with Edit):

| test | red before fix | break | under break | after revert |
|---|---|---|---|---|
| `only_in_workspace_file_is_a_whole_file_hit` | FAILED (roots lacked `HandlePosted`, one `diff-file-unmatched`) | `if ixs.is_empty()` → `if ixs.is_empty() \|\| !ixs.is_empty()` (always unmatched) | FAILED | ok |
| `only_in_path_outside_the_workspace_stays_unmatched` | ok (pins existing miss behaviour) | replace the miss-branch `diagnostics.push(...)` with `let _ = file;` | FAILED | ok |
| `only_in_dir_with_trailing_slash_has_one_separator` | FAILED (`["new//x.al"]`) | `dir.trim_end_matches('/')` → `dir` | FAILED (`["new//x.al"]`) | ok |

Gates on the working tree: `bash scripts/ci-steps task-affected` exit 0 (4425 run, 4425 passed, 8 skipped); `bash scripts/check-goldens` exit 0 (1263 run, 1263 passed, 4 skipped); no golden file changed.
