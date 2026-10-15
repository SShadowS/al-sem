# Issue #72 ledger

## Claim

```json
{"run_id":"20261015-063959-96ddda","issue":72,"attempt":1,"session":"claude-code-local:bb3fcebb-4fcc-42e9-82de-a5b63c6b6cfa","branch":"issue/72-agent-discovery-digest-diff-par-a1","worktree":"U:\\Git\\al-sem-issue-72-a1","body_hash":"83a364cc92a4e42a96664ea5fa29f5414858d8a56f52e6f36860df574c15982d","title":"[agent-discovery][digest (diff parser)] parse_unified_diff never reports an error, so ChangedRootsDiagnostic::DiffParseE"}
```

Body hash: `83a364cc92a4e42a96664ea5fa29f5414858d8a56f52e6f36860df574c15982d`.
Operator reviewer directive: the astra slot is gpt-6.1-sol (pi) and the flash slot is Opus 5.5 (Agent tool). Caps: spec_rounds 5, final_rounds 5.

## Classification

**bounded.** #71 did NOT fix the acceptance. At `5c67fcf9` the issue's exact reproducer
(`@@ garbage @@` on `src/posting.al` of a ws-d8 copy) still gives exit 0, NO diagnostic, and
BOTH routines of the file as changed roots. #71 only added two error sources (truncated hunk
body; out-of-range header numbers), and even its out-of-range case still roots every routine
of the file (the dropped hunk leaves `hunks` empty, which `digest_cli` reads as a hunkless
file) — and a #71 test (`out_of_range_hunk_header_is_a_parse_error`) pins that
over-approximation. The fix is local to `diff_parser.rs` and `digest_cli.rs` plus tests. The
only output-shape change is an optional `file` key on `diff-parse-error`; no golden carries a
`diff-parse-error`, so no golden moves. Not a spike: no existing test pins the acceptance
(the one nearby test pins the opposite).

## Design

(Revised after spec panel rounds 1 and 2; findings register: `.agent/issue-72/findings.json`.)

**Root cause.** Three things together turn a malformed header into whole-file roots:
1. `src/engine/l5/diff_parser.rs:147` `parse_hunk_header` returns `None` for every line that
   starts with `@@` but fails the strict grammar, and the caller's let-chain at
   `:548-562` (`line.starts_with("@@") && let Some(parsed) = parse_hunk_header(line) &&
   let Some(ref mut f) = current_file`) then falls to "All other lines ignored". No error,
   no hunk. Only the out-of-range case returns `Some(Err)`, and even that error is dropped
   when no `current_file` is open (it sits inside the same let-chain).
2. The parser reports errors only in the global `DiffParseResult.errors: Vec<String>`, with
   no file attached. So the consumer cannot know WHICH file a broken header belonged to.
3. `src/engine/l5/digest_cli.rs:444-448`: `if diff_file.hunks.is_empty()` takes EVERY
   declaration of the file. That branch is right for a pure rename and, by #71's decision
   (Review Focus 2), for a header pair with no hunk; it is wrong for a file whose hunks the
   parser dropped. Point 2 is why it cannot tell them apart.

The root-cause fix is therefore to attach header errors to the file they belong to (fixes 2,
which makes 3 decidable) and to stop dropping malformed `@@` lines (fixes 1).

**Fix — parser (`diff_parser.rs`).**
- `parse_hunk_header` stays the pure grammar oracle, unchanged. In the caller, a line that
  starts with `@@` and gets `None` is a header error with detail
  ``malformed hunk header `<line>` ``; `Some(Err(e))` (out of range) is a header error with
  the existing detail `hunk at +N has an invalid line range`.
- "Inside a file section" = a `diff --git` line, a `---` line, or a `rename from/to` line has
  been seen since the last record was finalized, or a record (`current_file`) is open. A
  header error outside any section (a preamble, e.g. a `format-patch` commit message before
  the first diff) stays ignored as today: there is no file to over-approximate, and an error
  there would be a false alarm.
- **Section destination, kept independently of record creation (round 2, astra R2-A01).**
  The parser tracks `section_dest: Option<String>`, the path the current section is about:
  - set by a `diff --git` line from its b-side path (new helper `git_header_dest`): a quoted
    b-token is unquoted; else, if the rest holds exactly one space, the second token; else,
    if the rest splits at its middle space into two equal names after `a/`/`b/` stripping
    (git's own rule for unquoted names with spaces, which git writes only when both sides
    are equal), that name; else `None`;
  - overwritten by `rename to`;
  - set by a `---` line only when still `None` (plain `diff -u`; `/dev/null` gives `None`);
  - superseded by the record's path when the section emits a record (`+++` record or
    hunkless-rename record).
  All paths go through the same `unquote_path` + `\` -> `/` + `strip_ab_prefix` handling the
  `---`/`+++` arms already use.
- **`unquote_path` decodes bytes, not chars (round 3, astra R3-A01).** Today
  (`diff_parser.rs:110-120`) each octal escape becomes its own char (`n as char`), so git's
  default quoting of a non-ASCII name, `"b/src/\303\251.al"`, decodes to `src/Ã©.al`, not
  `src/é.al`. That is a pre-existing defect: probe P17 shows a plain, healthy git diff of
  `src/é.al` (git quotes non-ASCII names by default, `core.quotePath=true`) gives
  `diff-file-unmatched`, exit 2, no roots. With #72 it would also let a literal-UTF-8
  section of the same file escape the veto and name the wrong file. Fix at the root cause:
  `unquote_path` collects a `Vec<u8>` (plain chars push their UTF-8 bytes, an octal escape
  pushes ONE byte, the named escapes push their byte; add git's `\a \b \f \v` beside the
  existing `\n \t \r \" \\`), then decodes the whole buffer with
  `String::from_utf8_lossy`. It is the one decoder for `---`/`+++` paths (`extract_path`),
  `rename from`/`rename to` and the new `git_header_dest`, so all three spellings agree.
  An invalid sequence becomes U+FFFD in every spelling alike (no panic). Existing
  `unquote_path_*` unit tests (`diff_parser.rs:769-788`) stay green (ASCII only); add
  `unquote_path_octal_escapes_are_utf8_bytes` (`"\303\251"` -> `é`) beside them.
- **Header errors are pending per section, then resolved to a path.** A header error inside
  a section goes to `pending_header_errors`. They are resolved, and `section_dest` cleared,
  when the section closes: at `diff --git`, at end of input, and at a `---` line when a
  record is open OR pending errors exist (a `---` right after `diff --git` closes nothing,
  so the git dest survives). Resolution: path = the record the section emitted (the
  finalized `current_file`, or the hunkless-rename record pushed at `:526-531`/`:572-577`),
  else `section_dest`; each error becomes a `FileError { path, detail }` in a new
  `DiffParseResult.file_errors`. Only when both are `None` (a header error in a `diff --git`
  section whose names cannot be split and that has no `rename to`/`---`/`+++` path — git
  never writes that) does it fall to the global `errors` (no file; nothing can be vetoed).
  `DiffFile` itself does not change. This closes the malformed-rename bypass (astra R1-A03,
  P12), astra's round-2 counterexample (P16: the `+++` is skipped, the error still names
  `src/posting.al` from the `diff --git` line), and flash R2-F01 (plain `--- a/x.al`,
  `@@ garbage @@`, `--- a/y.al`, `+++ b/y.al`: the error resolves to `x.al` at the second
  `---`, never moving into y's record).
- **Rejected-body recovery (astra R1-A02, P14):** after a header error in a section that
  began with `diff --git`, the parser skips EVERY line until the next `diff --git` line (or
  end of input), which then closes the section and clears the skip state. The rejected
  hunk's body has no trustworthy length, so its `--- x`/`+++ y` lines must not open a
  record, and the file's later hunks must not attach to one. A body line always starts with
  ` `, `-`, `+` or `\`, so it is never a `diff --git ` line: exact for git output. It also
  means one error per broken git section (later `@@` lines are not re-reported).
  Consequence (flash R2-F03): in a git section a header error never meets a later `+++`;
  resolution to "the `+++` record" happens only when the error follows the `+++` (record
  open) or in plain `diff -u` sections. No test for "git error before `+++`, then a `+++`
  record": that path cannot occur.
- Truncation errors ("hunk at +N truncated") stay global `errors` strings, unchanged, and do
  NOT poison the file: their header ranges were valid, the changes read sit at real line
  numbers, and #71 pins their roots (`hunk_body_is_count_bounded` (c)/(c1),
  `bare_header_pair_inside_short_hunk_is_body`, `minus_line_past_old_count_ends_the_hunk`).
  The acceptance names only a malformed hunk HEADER.
- NO rule for a `---`/`+++` pair with zero hunks. #71 decided it deliberately (plan Review
  Focus 2, CHANGELOG, `hunkless_modified_file_takes_every_declaration` at
  `tests/cli/cli_digest_changed_spans.rs:1635-1660`): a header pair with no hunk takes every
  declaration, like a pure rename. That input is not a malformed header, so it is outside
  #72's acceptance; the earlier draft's no-hunk rule is DROPPED and that test stays green
  unchanged. A file whose only hunk header was rejected is now distinguished from it by
  `file_errors`, so the #71 decision no longer leaks into the malformed case.

**Fix — consumer (`digest_cli.rs`).**
- `ChangedRootsDiagnostic::DiffParseError { file: Option<String>, detail: String }`
  (flash F3, settled at the root cause: the parser now knows the file, so the diagnostic
  carries it as a structured field like `DiffFileUnmatched`/`HunkOutsideRoutines`/
  `DiffFileDeleted`, not a text prefix). Projected as `{"kind":"diff-parse-error",
  "file":…,"detail":…}`, with `file` omitted when `None` (truncation, sectionless errors).
  `file` is the record's path as written in the diff (same as the other per-file kinds).
- In the diff block (`:420-470`): first push one `DiffParseError { file: None }` per global
  error (as today), then one `DiffParseError { file: Some(path) }` per `file_errors` entry
  (parse order), then build `poisoned: HashSet<String>` = `normalize_input(path)` of every
  `file_errors` entry. In the per-record loop, after the deleted/unmatched checks,
  `if poisoned.contains(&normalize_input(&diff_file.path)) { continue; }`. The key is the
  NORMALIZED path (`digest_cli.rs:101-103`: scheme stripped, `/`, lowercased — the key
  `spans.of_file` matches with at `:435`), never the raw spelling. So a second record for the
  same file cannot bring roots back whatever its case (astra R1-A02/R2-A02, flash R2-F02;
  P13, P15), and a section that emitted no record still vetoes its path (P16).
- The veto covers DIFF-derived roots only. `--file` and `--routine` hits are collected
  before the diff block (`:373-416`) and are unioned in (`:467-474`); a user who names the
  routine explicitly still gets it.
- Exit code follows the existing rule (`:1631`: zero roots -> 2).

**Semantics per shape** (all on `src/posting.al`, body changes line 9 unless stated):

| shape | after fix |
|-------|-----------|
| `@@ garbage @@` (issue reproducer) | 1 `diff-parse-error` with `file: "src/posting.al"`, no roots, exit 2 |
| `@@ -x,y +a,b @@` | same |
| `@@ -8,3 +8,3x @@` (garbage inside the range) | same |
| `@@ -8,3 @@` (no `+` side) | same |
| `@@@ -8,3 -8,3 +8,3 @@@` (combined diff) | same (combined diffs are unsupported, now said so) |
| `@@ -1,1 +2147483647,0 @@` (#71 out of range) | same, detail `hunk at +2147483647 has an invalid line range` (was: both roots, exit 0) |
| `@@ -8,3 +8,3 @@ procedure PostSalesDoc()` (git section heading) | VALID, unchanged: root `PostSalesDoc`, no diag |
| malformed hunk + good hunk in one record, either order | 1 error, NO roots from the file, exit 2 |
| malformed record + later good record for the SAME path, either order (plain `diff -u`) | 1 error, no roots from the file |
| same, paths differing in case (`src/Posting.al` vs `src/posting.al`), either order | 1 error, no roots from the file (normalized key) |
| git section `diff --git`, `---`, `@@ garbage @@`, `+++` (skipped), plus a healthy `diff --git` section for the same path, either order (astra R2 counterexample) | 1 error with `file: "src/posting.al"` (from the `diff --git` line), no roots |
| `diff --git a/src/posting.al b/src/posting.al` then `@@ garbage @@` at once (no `---`/`+++`/rename), plus a healthy section for the same path, either order | 1 error with `file: "src/posting.al"` (only the `diff --git` line names it), no roots |
| `diff --git a/src/old.al b/src/posting.al` then `@@ garbage @@` at once, plus a healthy `src/posting.al` section, either order | 1 error with `file: "src/posting.al"` (b side), no roots |
| workspace file `src/é.al`: escaped section `diff --git "a/src/\303\251.al" "b/src/\303\251.al"` + `@@ garbage @@`, plus a literal-UTF-8 healthy section, and the reverse spellings, either order | 1 error with `file: "src/é.al"`, no roots |
| healthy git diff of `src/é.al` with git's default quoted names | roots from the hunk (was: `diff-file-unmatched`, exit 2) |
| P14, then a real `diff --git` section for `src/subscriber.al` with a valid line-9 hunk | 1 error naming posting.al, roots exactly [HandlePosted] (skip state cleared at `diff --git`) |
| plain `--- a/src/posting.al`, `@@ garbage @@`, then `--- a/src/subscriber.al`/`+++ b/src/subscriber.al` + valid hunk (flash R2-F01) | 1 error naming posting.al, roots [HandlePosted] |
| malformed hunk in posting.al + good hunk in subscriber.al | 1 error naming posting.al, roots [HandlePosted], exit 0 |
| git section: malformed header, then body lines `--- a/src/subscriber.al`/`+++ b/src/subscriber.al`, then a valid `@@` | 1 error naming posting.al, no roots at all (was: whole posting.al + HandlePosted) |
| `rename from/to` + `@@ garbage @@`, no pair | 1 error naming the rename target, no roots (was: whole file) |
| `@@ garbage @@` before any file section (preamble) | ignored as today, no error |
| pure rename, no pair | unchanged: every declaration, no diag |
| `---`/`+++` pair with no hunk (incl. mode change) | unchanged (#71 Review Focus 2): every declaration, no diag |
| truncated hunk | unchanged (#71): error without `file`, roots from the lines read |

**Existing tests the change touches** (sweep: every `parse_unified_diff`/`DiffParseError`/
`.errors` use, every diff literal in `tests/` and `diff_parser.rs` tests, and every `@@`
literal that fails the grammar; the only malformed `@@` literals in tests are the
`parse_hunk_header` oracle inputs at `diff_parser.rs:734-747`):

| test | change |
|------|--------|
| `tests/cli/cli_digest_changed_spans.rs::out_of_range_hunk_header_is_a_parse_error` (:1095) | UPDATED: roots empty, exit 2 (was both roots, exit 0); assert exactly one `diff-parse-error` with `file == "src/posting.al"` and detail containing `invalid line range`; doc comment updated, overflow-panic mutation kept; add a case with a valid hunk in the same record |
| `src/engine/l5/diff_parser.rs::tests::out_of_range_hunk_header_is_an_error_not_a_panic` (:919) | UPDATED: the error moves from `r.errors` to `r.file_errors` (still `len == 1`, `contains("invalid line range")`, path `x.al`), `r.errors` is empty |
| `diff_parser.rs::tests::malformed_hunk_headers_rejected` (:731) | UNCHANGED (grammar oracle; the function stays pure) |
| every other `diff_parser.rs` test asserting `r.errors` (truncation: :848, :868, :884, :946 …) | UNCHANGED (truncation stays global) |
| `cli_digest_changed_spans.rs` :984-985, :1019, :1051 (`parsed.errors` on truncation inputs) | UNCHANGED |
| `hunkless_modified_file_takes_every_declaration` (:1637), `rename_with_and_without_hunks` (:668) | UNCHANGED, must stay green (regression guards) |
| `hunk_body_is_count_bounded`, `bare_header_pair_*`, `minus_line_past_old_count_ends_the_hunk` | UNCHANGED (truncation); they assert `diag_kinds` only, so the projection change does not touch them |
| `cli_b_digest_differential` `--diff` golden (`tests/cli-b-goldens/digest/ws-d8-commit-in-tx.changed.diff` / `.diff.json`) | UNCHANGED: valid diff, `changedRootsDiagnostics: []` |

**Acceptance matrix.** CLI tests go through the `alsem` binary via `digest()` in
`tests/cli/cli_digest_changed_spans.rs` (`--test cli`). Every new test asserts EXACTLY ONE
`diff-parse-error` and its `file`, and the detail text of the specific error class, so a
redundant path cannot mask a mutation (astra A4).

| acceptance item | test | hand-stated precondition | mutation that must turn it red |
|-----------------|------|--------------------------|--------------------------------|
| malformed header -> 1 `diff-parse-error` naming the file + no roots from it | `malformed_hunk_header_gives_parse_error_and_no_roots` (loops the 5 malformed headers + out-of-range; each ALSO run with a valid `@@ -19,2 +19,3 @@` hunk in the same record, before and after) | the same body under `@@ -8,3 +8,3 @@` gives `["PostSalesDoc"]` and no diag; the valid hunk alone gives `["OnAfterPostSalesDoc"]`; the heading form stays valid | (a) delete the `poisoned` `continue` -> `OnAfterPostSalesDoc` (mixed) / both routines (alone), red; (b) restore "ignore a non-matching `@@` line" -> zero errors, red; (c) treat the heading form as malformed -> precondition red |
| the error names its file | same test + `malformed_hunk_poisons_only_its_own_file` (posting.al malformed, subscriber.al valid line 9) | subscriber hunk alone gives `["HandlePosted"]` | attach the error with `file: None` (or to the next record) -> `file` assert red; poison at diff level (any error -> no roots) -> `HandlePosted` missing, red |
| same path in two records | `malformed_record_poisons_every_record_of_its_path` (plain `diff -u`; same spelling in both orders, AND malformed `src/Posting.al` + healthy `src/posting.al` in both orders) | the good record alone gives `["PostSalesDoc"]`; `src/Posting.al` with a valid hunk alone gives `["PostSalesDoc"]` (P15) | veto per record -> `PostSalesDoc`, red; veto by raw path -> `PostSalesDoc` in the case-differing cases, red |
| git section with no record still vetoes its path (astra R2-A01) | `git_section_error_before_plus_header_vetoes_its_path` (P16 input, both section orders) | the healthy section alone gives `["PostSalesDoc"]` | resolve pending errors only from records (drop `section_dest` entirely) -> error has no `file` and `PostSalesDoc` is rooted, red. CORRECTED (astra R3-A02): deleting only the `diff --git` call of `git_header_dest` leaves P16 GREEN, because its `---` line sets `section_dest`; P16 is a regression for "a skipped `+++` still resolves", not the proof of the helper's use (next two rows are) |
| the `diff --git` destination is used in production (astra R3-A02) | `git_header_names_the_file_of_an_immediate_header_error` (`diff --git a/src/posting.al b/src/posting.al` then `@@ garbage @@` immediately, no `---`/`+++`/rename; plus a healthy section for the same path; both orders) | the healthy section alone gives `["PostSalesDoc"]` | delete the `git_header_dest` call at `diff --git` -> error has no `file` and `PostSalesDoc` is rooted, red |
| the b side drives the veto (astra R3-A02) | same test, case `diff --git a/src/old.al b/src/posting.al` + `@@ garbage @@` immediately, plus a healthy `src/posting.al` section, both orders | as above | take the a-side name -> error names `src/old.al` and `PostSalesDoc` is rooted, red |
| one decoder for every path spelling (astra R3-A01) | `escaped_and_literal_spellings_of_one_file_share_the_veto` (workspace file `src/é.al` holding the d8 posting codeunit; malformed escaped + healthy literal, and malformed literal + healthy escaped, both orders; plus the healthy escaped section alone) | the healthy escaped section alone gives `["PostSalesDoc"]` (red at base: P17, `diff-file-unmatched`); the healthy literal section alone gives `["PostSalesDoc"]` (P17b) | decode each octal escape as its own char again -> the healthy escaped section alone is unmatched, the error names `src/Ã©.al` and the literal healthy section is rooted, red |
| rejected body never opens a file (git format) | `rejected_hunk_body_never_opens_a_file` (P14 input) | the same lines after a VALID header with matching counts read `--- …`/`+++ …` as body (existing #71 behaviour) | drop the skip-until-`diff --git` state -> `HandlePosted` root, red |
| skip state resumes at the next `diff --git` (astra R2-A02) | `rejected_body_skip_resumes_at_next_git_section` (P14 + a real `diff --git a/src/subscriber.al b/src/subscriber.al` section, valid line-9 hunk): exactly `["HandlePosted"]`, exactly one error naming posting.al | the subscriber section alone gives `["HandlePosted"]` | never clear the skip state -> no roots, exit 2, red |
| plain pending error never moves to the next file (flash R2-F01; plain `diff -u` only) | `plain_section_error_stays_with_its_file` (`--- a/src/posting.al`, `@@ garbage @@`, `--- a/src/subscriber.al`, `+++ b/src/subscriber.al`, valid line-9 hunk) | the subscriber pair alone gives `["HandlePosted"]` | do not close the section at a `---` with pending errors -> error names subscriber.al and `HandlePosted` is vetoed, red |
| malformed rename section | `malformed_header_in_a_rename_section_gives_no_roots` (P12 input) | the clean rename (no `@@`) gives both roots (`rename_with_and_without_hunks`) | resolve pending errors without the rename record/`rename to` dest -> no `file`, both roots, red |
| preamble `@@` stays ignored | `preamble_hunk_header_is_not_an_error` (`@@ garbage @@` before the first `---`, then a valid file) | — | report sectionless `@@` lines -> one error, red |
| #71 out of range now meets acceptance | `out_of_range_hunk_header_is_a_parse_error` UPDATED (above) | — | drop the header-error mark on the range error -> both roots, red; drop the range check -> overflow panic (exit 101), red |
| parser level | new `diff_parser::tests::malformed_hunk_header_is_an_error_of_its_file` (error in `r.file_errors` with the right path, `r.errors` empty, good file unaffected) and `git_header_dest_names` (one space, equal halves with spaces, quoted b-token, rename header takes the b side, ambiguous -> `None`) | literal diffs | put the error in global `errors` -> red; attach to the wrong path -> red; take the a-side name -> red on the rename header |
| regressions held | `hunkless_modified_file_takes_every_declaration`, `rename_with_and_without_hunks` (add: no diag), truncation tests above | — | applying the poison to hunkless pairs, clean renames or truncation turns these red |

Each mutation is a prediction; the actual red/revert-green runs are recorded in
"Discrimination proofs" during implementation.

**Goldens expected to move: none.** `grep -rl diff-parse-error tests docs` finds only
`tests/cli/cli_digest_changed_spans.rs` and the #71 plan doc; the one `--diff` golden is a
valid diff with ASCII paths (the decoder change cannot move it). CHANGELOG: one `Fixed`
entry for #72, plus a `Fixed` line for git-quoted non-ASCII paths (P17, was
`diff-file-unmatched`); also correct the #71 entry's sentence "a file left
with no hunk then takes every declaration" (`CHANGELOG.md:276-277`) for the out-of-range case.

**Limits and failure shapes (stated, not fixed).**
- **Under-approximation with exit 0 (flash F6).** Malformed in file A + good in file B gives
  B's roots and exit 0. A's changed routines are NOT in the roots. Before this change the
  same input over-approximated (safe for test selection); now it under-approximates, which
  is what the acceptance asks for. Consumers must treat a `diff-parse-error` with a `file`
  as "that file's changes are missing; the digest is incomplete". No exit-code change.
- **Plain `diff -u` body recovery (astra A2 residual).** With no `diff --git` line there is
  no reliable file boundary: a rejected hunk's removed line `-- a/x.al` and added line
  `++ b/x.al` read exactly like a real next-file pair, and a real next file looks the same.
  The parser reads them as a new file (probe P14b); if that path is a real workspace file it
  can get roots from the rejected hunk's later lines. Same path is covered by the
  normalized-path veto; a different path is irreducibly ambiguous here (same class as #71's
  stated `diff -u` limit). Git output is covered exactly by the skip state.
- **format-patch series (flash F5).** In a `git format-patch --stdout` series, after the
  first patch the previous patch's last file section is still open while the next mail's
  headers and unindented commit message are read (nothing finalizes on `From …`/`Subject:`
  or the bare `---` separator). A commit-message line starting with `@@` there would be a
  header error and veto that previous file's roots. `git log -p` indents messages, so it is
  safe. Recorded as a limit; a mail-boundary rule is a separate enhancement.
- A `diff-parse-error` with no `file` (truncation; a header error in a `diff --git` section
  with unsplittable names and no `rename to`/`---`/`+++` path, which git never writes)
  suppresses nothing.
- Plain `diff -u` pending errors (flash R2-F01) are FIXED, not a limit: a `---` with pending
  errors closes the section and resolves them to the previous `---` path.

## Assumption probes

Binary: `target/debug/alsem.exe` built at `5c67fcf9` in this worktree. Workspace: copy of
`tests/r0-corpus/ws-d8-commit-in-tx`. Command: `alsem digest ws --diff <p> --deterministic
--single-app`. Every patch is `--- a/src/posting.al` / `+++ b/src/posting.al` + header +
a body changing line 9 (inside `PostSalesDoc`, lines 3-16) unless stated. "diags" means
`payload.changedRootsDiagnostics` (the envelope also carries a detector warning). Files under
`U:/Git/al-call-hierarchy/.agent/runs/20261015-063959-96ddda/probe/`.

| probe | result | evidence |
|-------|--------|----------|
| P0 valid `@@ -8,3 +8,3 @@` | roots [PostSalesDoc], diags [], exit 0 | `p0_good.json` |
| P1 issue reproducer `@@ garbage @@` | roots [PostSalesDoc, OnAfterPostSalesDoc] (whole file), diags [], exit 0 — **NOT fixed by #71** | `p1_garbage.json` |
| P2 `@@ -x,y +a,b @@` | whole file, diags [], exit 0 | `p2_letters.json` |
| P3a `@@ -8,3 +8,3 @@ procedure PostSalesDoc()` (legit section heading) | [PostSalesDoc], diags [], exit 0 — correct, must stay | `p3a_trailing_heading.json` |
| P3b `@@ -8,3 +8,3x @@` | whole file, diags [], exit 0 | `p3b_trailing_garbage_in_range.json` |
| P4 `@@ -8,3 @@` (no `+` side) | whole file, diags [], exit 0 | `p4_missing_plus.json` |
| P5 `@@@ -8,3 -8,3 +8,3 @@@` (combined) | whole file, diags [], exit 0 | `p5_combined.json` |
| P6 pair, no hunk | whole file, diags [], exit 0 — #71's deliberate decision, kept | `p6_nohunk.json`; `hunkless_modified_file_takes_every_declaration` |
| P7 `@@ garbage @@` + good `@@ -19,2 +19,3 @@` same file | [OnAfterPostSalesDoc], diags [], exit 0 (malformed hunk silently lost, good hunk kept) | `p7_mixed.json` |
| P8 `@@ -99999999999,1 +99999999999,1 @@` (#71 out of range) | 1 `diff-parse-error` "hunk at +99999999999 has an invalid line range" BUT whole file still rooted, exit 0 | `p8_overflow.json`; pinned as-is by `out_of_range_hunk_header_is_a_parse_error` (tests/cli/cli_digest_changed_spans.rs:1095) |
| P9 `diff --git` + `index` only (mode/binary-like) | no files, roots [], diags [], exit 2 | `p9_githeader_only.json` (out of scope: no content change) |
| P10 truncated hunk | 1 `diff-parse-error` "hunk at +8 truncated", roots [PostSalesDoc], exit 0 (#71 behaviour) | `p10_truncated.json` |
| P11 `@@ garbage @@` in posting.al + valid hunk in subscriber.al | [PostSalesDoc, OnAfterPostSalesDoc, HandlePosted], diags [], exit 0 | `p11.json` |
| P12 `diff --git` + `rename from/to` + `@@ garbage @@`, no pair | whole file, diags [], exit 0 (astra A3 confirmed) | `p12_rename_garbage.json` |
| P13 `@@ garbage @@` record, then a second `---`/`+++` record for the same path with a valid hunk | whole file, diags [], exit 0 (astra A2 confirmed; a per-record flag would still root `PostSalesDoc`) | `p13_same_path_reopen.json` |
| P14 git section: `@@ garbage @@`, then body lines `--- a/src/subscriber.al`/`+++ b/src/subscriber.al`, then `@@ -9,1 +9,1 @@` | [PostSalesDoc, OnAfterPostSalesDoc, HandlePosted], diags [], exit 0 — the rejected body opened subscriber.al and posting.al's hunk attached to it | `p14_body_opens_other_file.json` |
| P14b same as P14 without `diff --git` (plain) | same result; indistinguishable from a real second file | `p14b_plain.json` |
| P15 `src/Posting.al` (case differs from the workspace file) with a valid line-9 hunk | [PostSalesDoc], exit 0 — matching is case-insensitive, so a raw-path veto would miss it | `p15_case.json` |
| P17 healthy git diff of workspace file `src/é.al` with git's default quoted names (`"b/src/\303\251.al"`) | `diff-file-unmatched` with `file: "src/Ã©.al"`, roots [], exit 2 — pre-existing decoder defect (astra R3-A01 confirmed) | `p17_quoted_utf8.json` (workspace `wsu`: `ws` with `posting.al` renamed to `é.al`) |
| P17b the same diff with literal UTF-8 names | [PostSalesDoc], exit 0 | `p17b_literal_utf8.json` |
| P16 astra R2 counterexample (git section `---`, `@@ garbage @@`, `+++`; then a healthy section for the same path) | whole file, diags [], exit 0 at base | `p16_astra_r2.json` |
| existing test pinning acceptance? | none. `malformed_hunk_headers_rejected` (diff_parser.rs:731) pins `parse_hunk_header(..).is_none()` (the grammar oracle); `out_of_range_hunk_header_is_a_parse_error` pins whole-file roots | grep `diff-parse-error` / `DiffParseError` over src+tests |
| other parser consumers | only `digest_cli.rs:423` (+ tests) | grep `parse_unified_diff` |
| golden carrying a diff | only `tests/cli-b-goldens/digest/ws-d8-commit-in-tx.changed.diff`: valid, `changedRootsDiagnostics: []` | read |

## Base/Head SHAs

- B0 = `5c67fcf9887855d3bd5bd15e9c9d9c734bda4e3e` (master at claim; includes #71's diff-parser hardening)
- B = `5c67fcf9887855d3bd5bd15e9c9d9c734bda4e3e`, H = `751e3463e095b8b6ba0bd640d77b1cfcb6066d71` (final; no rebase needed)

## Timeline

- Claimed (attempt 1). Worktree created from master `5c67fcf9`.
- Classified bounded; design written.
- Spec panel round 1: both NOT READY (astra 4 important; flash 1 critical, 1 important, 5 minor). Design revised: no-hunk rule dropped (#71 Review Focus 2 kept), header errors attached per file with a structured `file`, normalized-path veto, rename-section transfer, git-section body skip, test sweep, limits stated.
- Spec panel round 2: flash READY, astra NOT READY; all 11 R1 entries accepted by both. Design revised: section destination from the `diff --git` line kept apart from record creation, errors resolved to a path at section close (`file_errors`), plain `---` closes a section with pending errors, normalized-path veto pinned with case-differing paths, skip-resume test added.
- Spec panel round 3: flash READY, astra NOT READY; all 16 accepted by both. Design rev 4: `unquote_path` decodes octal escapes as bytes then UTF-8 (one decoder for git destination, header pairs and renames; also fixes pre-existing P17); production CLI cases for an immediate header error after `diff --git` and for distinct a/b names; P16's mutation prediction corrected.
- Spec panel round 4 (spec_rounds 4/5, pi 4): astra READY and flash READY; both accepted all 18 entries, and neither raised anything new. Spec CONVERGED. flash's implementation note: when the b-side path is quoted, find where it starts with a scan that skips escaped quotes, not `rfind`; pin it with a unit case.
- Step 6: acceptance tests written (uncommitted); CLI 28 pass / 11 fail for the stated reasons, lib tests do not compile on the new API (see `## Acceptance tests (red)`).
- Step 7: plan `docs/superpowers/plans/2026-10-15-issue-72-malformed-hunk-headers.md`, 1 task, 20 discrimination proofs, no golden expected to move.
- Task 1 implemented, commit `4cf9ffd6`: CLI 39/39 and diff_parser 31/31 green, 20 proofs recorded (proof 16 first came back green as a redundant path; `malformed_header_in_a_rename_section_gives_no_roots` gained an unequal-names-with-spaces rename case that discriminates), task gate and check-goldens green, 0 goldens moved, check-diff `reasons: []`. Deviation decisions: (1) `section_dest` from `rename to` is NOT `a/`/`b/`-stripped: CONFIRMED. The Design's strip rule exists so every spelling equals the record's `path`; the rename record's `path` is the unstripped `rename to` string (pre-existing), git writes rename paths without prefixes, and stripping would rename a real top-level `a/`/`b/` directory, so the error would name a path no record has and the veto would miss it. (2) `in_section` is never cleared: CONFIRMED. The Design's "seen since the last record was finalized" is always true once set, because a record is finalized only at `---`, `diff --git` (each opens a section in the same step) or end of input; never clearing is the exact rule and matches the Design's stated format-patch limit (F5). Also corrected `git_header_dest`'s doc: git DOES write unequal unquoted names with spaces, for a rename, where `rename to` names the file (the Design's "only when both sides are equal" holds for non-renames).
- Final panel round 1 fix pass, commit `751e3463` (task gate green, check-diff `reasons: []`): fixed CR-01, CR-02, CR-05, CR-06, CR-07, FR1-A01, FR1-F01, FR1-F02; refuted CR-03; deferred CR-04 (minor). Changes: section state gathered in `Section { pending, dest, git, skip }` with one `close()` (CR-07); `git_header_dest` scans a leading quoted a-token escape-aware via `quoted_token_len` (FR1-A01); `diff --cc`/`diff --combined` are section headers that share the `diff --git` arm, counted as git sections so a `@@@` skip ends at the next header (FR1-F01; flash suggested `git = false`, but the combined body has no trustworthy length either); octal escapes accept only 0-7 and keep an above-`\377` value as text (FR1-F02). Tests: new CLI `added_file_header_error_names_its_plus_side`, new unit `combined_diff_headers_are_section_boundaries`, extended `git_header_names_the_file_of_an_immediate_header_error`, `git_header_dest_names`, `malformed_hunk_header_is_an_error_of_its_file`, `unquote_path_octal_escapes_are_utf8_bytes`; proofs 21-26. Deliberately unpinned (CR-02): the `git` reset on a `---` close (now inside `Section::close`) and `in_section = true` on `rename from`; both matter only for input that is not git output. CR-03: in git output only the `rename to` destination assignment carries behaviour; the two hunkless-rename-record assignments are kept because in non-git input they decide the error's file (Design rule "superseded by the record's path"). Stated limit (CR-04): a hunkless git section followed by a plain `---` with no section header (hand-concatenated input only) keeps the git section's state; a mode-only git section followed by such a `---` is line-for-line a git section with its own pair, so no bookkeeping rule can tell them apart.
- Final panel (final_rounds 2/5). Round 1 (pi 5): astra NOT READY (FR1-A01, important); flash READY on condition that CR-01 is fixed, agreeing with all 7 code-review findings and adding FR1-F01/F02. A gate run started on `4cf9ffd6` was stopped, because the head was about to change. Round 2 (pi 6): both READY; all 28 entries accepted; no new findings. CONVERGED.
- Rebase: `origin/master` is unchanged since the claim (`5c67fcf9`), so no rebase was needed.

## Acceptance tests (red)

Written at B0 (`5c67fcf9`), uncommitted, in `tests/cli/cli_digest_changed_spans.rs` (CLI,
through the `alsem` binary via `digest()`) and `src/engine/l5/diff_parser.rs` (`tests`
module). Every CLI test checks its hand-stated precondition FIRST; each red below fails
after its precondition passed (the failing line is the acceptance assertion), except
the escaped-path test, whose precondition is itself red at base (P17).

Commands (`TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al`):
- `cargo test --test cli cli_digest_changed_spans::` -> exit 101; **28 passed, 11 failed**
  (39 run). The 11 failures are exactly the 10 new tests below that pin new behaviour plus
  the updated #71 test; every other test of the module, including the regression guards,
  passes.
- `cargo test -p al-sem --lib diff_parser` -> exit 101; does NOT compile: 10 x `E0609 no
  field file_errors on type DiffParseResult`, 1 x `E0425 cannot find function
  git_header_dest`. Expected: those are the design's new API (`DiffParseResult.file_errors`,
  `git_header_dest`).

| matrix row | test | red output | reason |
|------------|------|------------|--------|
| malformed header -> 1 error naming the file + no roots | `malformed_hunk_header_gives_parse_error_and_no_roots` | `exactly one diff-parse-error` left: 0 (first malformed header, alone) | a malformed `@@` line is silently ignored today (preconditions: `@@ -8,3 +8,3 @@` and the heading form give `[PostSalesDoc]`, `VALID_19` gives `[OnAfterPostSalesDoc]`, all green) |
| the error names its file | `malformed_hunk_poisons_only_its_own_file` | `exactly one diff-parse-error` left: 0 | same; precondition (subscriber alone -> `[HandlePosted]`) green |
| same path in two records | `malformed_record_poisons_every_record_of_its_path` | `exactly one diff-parse-error` left: 0 | same; preconditions (`src/posting.al` and `src/Posting.al` alone -> `[PostSalesDoc]`) green |
| git section with no record vetoes its path | `git_section_error_before_plus_header_vetoes_its_path` | `exactly one diff-parse-error` left: 0 | same; precondition green |
| `diff --git` destination used / b side drives the veto | `git_header_names_the_file_of_an_immediate_header_error` | `exactly one diff-parse-error` left: 0 | same; precondition green |
| one decoder for every path spelling | `escaped_and_literal_spellings_of_one_file_share_the_veto` | precondition `diags(&doc).is_empty()` fails: `[{"file":"src/Ã©.al","kind":"diff-file-unmatched"}]` | `unquote_path` decodes each octal escape as its own char (P17) |
| rejected body never opens a file | `rejected_hunk_body_never_opens_a_file` | `exactly one diff-parse-error` left: 0 | no header error, no skip state; precondition (valid header reads `---`/`+++` as body) green |
| skip resumes at next `diff --git` | `rejected_body_skip_resumes_at_next_git_section` | `exactly one diff-parse-error` left: 0 | same; precondition green |
| plain pending error stays with its file | `plain_section_error_stays_with_its_file` | `exactly one diff-parse-error` left: 0 | same; precondition green |
| malformed rename section | `malformed_header_in_a_rename_section_gives_no_roots` | `exactly one diff-parse-error` left: 0 | same; precondition (clean rename -> both roots) green |
| preamble `@@` stays ignored | `preamble_hunk_header_is_not_an_error` | GREEN at base (regression guard, as designed: today's behaviour kept) | — |
| #71 out of range meets acceptance | `out_of_range_hunk_header_is_a_parse_error` (UPDATED: 3 cases, alone / + `VALID_19` before / after) | `e["file"]` left: `Null`, right: `"src/posting.al"`; diags `[{"detail":"hunk at +2147483647 has an invalid line range","kind":"diff-parse-error"}]` | the range error is global, carries no file, and the file still takes both roots |
| parser level | `diff_parser::tests::malformed_hunk_header_is_an_error_of_its_file`, `git_header_dest_names` | does not compile (E0609 / E0425) | new API `file_errors`, `git_header_dest` |
| parser level, #71 test updated | `diff_parser::tests::out_of_range_hunk_header_is_an_error_not_a_panic` | does not compile (E0609) | asserts the error moved to `file_errors` (path `x.al`), `errors` empty |
| one decoder (unit) | `diff_parser::tests::unquote_path_octal_escapes_are_utf8_bytes` (`\303\251` -> `é`; `\a\b\f\v`; lone `\303` -> U+FFFD) | cannot run: the lib test target does not compile | uses only existing API; its runtime red at base is shown by the CLI escaped-path red above (`src/Ã©.al`) |
| regressions held | `rename_with_and_without_hunks` (added: no diag on the pure rename), `hunkless_modified_file_takes_every_declaration`, truncation tests | GREEN at base | regression guards |

Logs: scratchpad `red_cli.log`, `red_lib.log` (session bb3fcebb).

## Acceptance matrix

Green at `4cf9ffd6` (logs `target/t72-cli.log`, `target/t72-lib.log` in the worktree):
`cargo test --test cli cli_digest_changed_spans::` -> 39 passed, 0 failed;
`cargo test -p al-sem --lib diff_parser` -> 31 passed, 0 failed. Every test of
`## Acceptance tests (red)` is green: `malformed_hunk_header_gives_parse_error_and_no_roots`,
`malformed_hunk_poisons_only_its_own_file`, `malformed_record_poisons_every_record_of_its_path`,
`git_section_error_before_plus_header_vetoes_its_path`,
`git_header_names_the_file_of_an_immediate_header_error`,
`escaped_and_literal_spellings_of_one_file_share_the_veto`, `rejected_hunk_body_never_opens_a_file`,
`rejected_body_skip_resumes_at_next_git_section`, `plain_section_error_stays_with_its_file`,
`malformed_header_in_a_rename_section_gives_no_roots` (strengthened, see proof 16),
`preamble_hunk_header_is_not_an_error`, `out_of_range_hunk_header_is_a_parse_error`,
`diff_parser::tests::{malformed_hunk_header_is_an_error_of_its_file, git_header_dest_names,
unquote_path_octal_escapes_are_utf8_bytes, out_of_range_hunk_header_is_an_error_not_a_panic}`.
Task gate `scripts/ci-steps task` exit 0 (3198 passed, 0 failed); `scripts/check-goldens` exit 0
(760 passed); goldens moved: 0. `check-diff` after commit: `reasons: []`.

## Discrimination proofs

Each break made with Edit (the Edit's exact-match is the "break applied" assertion), run, reverted
with Edit, re-run green. Logs `target/t72-dp-<n>.log` (red) / `t72-dp-<n>g.log` (green). Commit
`4cf9ffd6` for all rows.

| # | break | test | red (quoted) | reverted green | commit |
|---|-------|------|--------------|----------------|--------|
| 1 | delete `if poisoned.contains(..) { continue; }` | `malformed_hunk_header_gives_parse_error_and_no_roots` | panicked at :1238 `no root from the file` (alone case: `rootsResolved: 2`, PostSalesDoc + OnAfterPostSalesDoc) | ok, 1 passed | 4cf9ffd6 |
| 2 | 4h `None => None` | same | `exactly one diff-parse-error` left: 0 right: 1 | ok | 4cf9ffd6 |
| 3 | `parse_hunk_header` returns `None` when text follows the closing `@@` | same | `precondition @@ -8,3 +8,3 @@ procedure PostSalesDoc()` exit code left: 2 right: 0 | ok | 4cf9ffd6 |
| 4 | push `file: None` for `file_errors` | `malformed_hunk_poisons_only_its_own_file` | left: Null right: "src/posting.al" | ok | 4cf9ffd6 |
| 5 | `if !parsed.file_errors.is_empty() { continue; }` | same | left: [] right: ["HandlePosted"] | ok | 4cf9ffd6 |
| 6 | `poisoned.contains(..) && diff_file.hunks.is_empty()` | `malformed_record_poisons_every_record_of_its_path` | panicked at :1302 roots not empty (`PostSalesDoc` rooted) | ok | 4cf9ffd6 |
| 7 | raw-path key (`fe.path.clone()` / `&diff_file.path`) | same | panicked at :1302 on the `src/Posting.al` case, `PostSalesDoc` rooted | ok | 4cf9ffd6 |
| 8 | `resolve` ignores `dest` (always the global branch) | `git_section_error_before_plus_header_vetoes_its_path` | left: Null right: "src/posting.al" | ok | 4cf9ffd6 |
| 9 | 4g `section_dest = None` instead of `git_header_dest(rest)` | `git_header_names_the_file_of_an_immediate_header_error` | left: Null right: "src/posting.al" | ok | 4cf9ffd6 |
| 10 | one-space branch takes `.0` (a side) | same + `git_header_dest_names` | CLI: left: String("src/old.al") right: "src/posting.al"; unit: left: Some("src/old.al") right: Some("src/new.al") | both ok | 4cf9ffd6 |
| 11 | octal escape pushed as `char::from(n)` UTF-8 bytes | `escaped_and_literal_spellings_of_one_file_share_the_veto` + `unquote_path_octal_escapes_are_utf8_bytes` | CLI: precondition at :1413, diags `[{"file":"src/Ã©.al","kind":"diff-file-unmatched"}]`; unit: left: "src/Ã©.al" right: "src/é.al" | both ok | 4cf9ffd6 |
| 12 | `quoted_b_start` without the escape skip | `git_header_dest_names` | left: Some("y.al") right: Some("x\"y.al") | ok | 4cf9ffd6 |
| 13 | 4h sets `skip_to_git = false` (never true) | `rejected_hunk_body_never_opens_a_file` | panicked at :1468, `HandlePosted` rooted | ok | 4cf9ffd6 |
| 14 | delete `skip_to_git = false;` in 4g | `rejected_body_skip_resumes_at_next_git_section` | left: [] right: ["HandlePosted"] | ok | 4cf9ffd6 |
| 15 | 4d closes only on `current_file.is_some()` | `plain_section_error_stays_with_its_file` | left: String("src/subscriber.al") right: "src/posting.al" | ok | 4cf9ffd6 |
| 16 | remove all three `section_dest = Some(..)` (rename to, hunkless-rename record at `diff --git` and at end) | `malformed_header_in_a_rename_section_gives_no_roots` | FIRST RUN GREEN (`t72-dp-16a.log`): redundant path, the test's `diff --git a/src/old.al b/src/posting.al` line already names the target via `git_header_dest`. Test strengthened: second case `diff --git a/src/old name.al b/src/posting.al` (unequal unquoted names with spaces, which git does write for a rename; `git_header_dest` -> `None`). Re-run with the break: red on that case, left: Null right: "src/posting.al" (`t72-dp-16b.log`) | ok (`t72-dp-16g.log`) | 4cf9ffd6 |
| 17 | drop `&& in_section` | `preamble_hunk_header_is_not_an_error` | panicked at :1576, one `diff-parse-error` "malformed hunk header `@@ garbage @@`" | ok | 4cf9ffd6 |
| 18 | `Some(Err(e))` pushes to global `errors` | `out_of_range_hunk_header_is_a_parse_error` + `out_of_range_hunk_header_is_an_error_not_a_panic` | CLI: left: Null right: "src/posting.al" (`rootsResolved: 2`); unit: `r.errors` = ["hunk at +2147483647 has an invalid line range"] | both ok | 4cf9ffd6 |
| 19 | drop the `fits` check (`if false && ..`) | `out_of_range_hunk_header_is_a_parse_error` | child `panicked at src\engine\l5\diff_parser.rs:348:17: attempt to add with overflow`; test panicked at :99 | ok | 4cf9ffd6 |
| 20 | `if diff_file.hunks.is_empty() && kind == Renamed { continue; }` | `rename_with_and_without_hunks` | pure rename exit code left: 2 right: 0 | ok | 4cf9ffd6 |
| 21 (CR-01) | delete `section.dest = current_file…` in the `+++` arm (grep count 0 asserted) | `added_file_header_error_names_its_plus_side` | panicked at :1178 (`assert_malformed_error_of`), `file` Null for `--- /dev/null`/`+++ b/src/posting.al`/`@@ garbage @@` (`target/r1-p1-red.log`) | 40/40 ok (`target/r1-green-cli.log`) | 751e3463 |
| 22 (FR1-A01) | quoted a-token branch made unreachable (`starts_with("\u{0}")`, count 1 asserted) | `git_header_names_the_file_of_an_immediate_header_error` (quoted case, both orders) + `git_header_dest_names` | CLI panicked at :1178 on `diff --git "a/src/old\303\251 name.al" b/src/posting.al` (`target/r1-p2-red-cli.log`); unit left: None right: Some("src/posting.al") (`target/r1-p2-red-unit.log`) | both ok (`r1-green-*.log`) | 751e3463 |
| 23 (CR-02) | `Section::close` drops a dest-less error (`None => {}`, count 1 asserted) | `malformed_hunk_header_is_an_error_of_its_file` (unsplittable-header case) | left: 0 right: 1, `r.errors` = [] (`target/r1-p3-red.log`) | ok | 751e3463 |
| 24 (FR1-F01) | `section_header_dest` no longer matches `diff --cc `/`diff --combined ` (`--XX`/`--YY`, count asserted) | `combined_diff_headers_are_section_boundaries` | left: ["x.al"] right: ["x.al", "y.al"]: y.al swallowed by the skip, flash's trace (`target/r1-p4-red.log`) | ok | 751e3463 |
| 25 (FR1-F02) | `is_octal` back to `is_ascii_digit` (count asserted) | `unquote_path_octal_escapes_are_utf8_bytes` | left: "a\\18b" right: "a\u{1}8b" (`target/r1-p5-red.log`) | ok | 751e3463 |
| 26 (FR1-F02) | out-of-range octal dropped again (`Err(_) => {}`, count asserted) | `unquote_path_octal_escapes_are_utf8_bytes` | left: "ab" right: "a\\777b" (`target/r1-p6-red.log`) | ok | 751e3463 |

## Findings register summary

There are 28 entries: 1 critical, 9 important and 18 minor. 25 are fixed, 1 is refuted (CR-03), and 2 are deferred; both deferred entries are minor stated limits (R1-F05 and CR-04). Every entry is accepted by both reviewers. See `findings.json`.

Spec panel:
- Round 1: 11 findings (astra 4, flash 7).
- Round 2: both reviewers accepted all 11. 5 new findings, all fixed.
- Round 3: both accepted all 16. 2 new findings, R3-A01 and R3-A02, both fixed.
- Round 4: both READY, all 18 accepted.

Final panel:
- Round 1: the code review (the `code-reviewer` agent at high effort, standing in for the built-in code-review skill) gave CR-01..07. flash said READY, conditional on CR-01, and added FR1-F01 and FR1-F02. astra said NOT READY and raised FR1-A01.
- Round 2: both READY, all 28 accepted, nothing new.

## Gate results

H = `751e3463e095b8b6ba0bd640d77b1cfcb6066d71`, B = `5c67fcf9887855d3bd5bd15e9c9d9c734bda4e3e`. All four gates ran supervised and exited 0. The worktree was clean outside `.agent/` afterwards.

| gate | exit | log |
|---|---|---|
| ci-steps-all | 0 | `.agent/runs/20261015-063959-96ddda/logs/ci-steps-all/20261015T080510-e8278c62.log` |
| check-goldens-coverage | 0 | `.agent/runs/20261015-063959-96ddda/logs/check-goldens-coverage/20261015T081535-ed5aa0f4.log` |
| check-goldens | 0 | `.agent/runs/20261015-063959-96ddda/logs/check-goldens/20261015T081536-2841c5ef.log` |
| cdo-gate | 0 | `.agent/runs/20261015-063959-96ddda/logs/cdo-gate/20261015T081630-538c0ad5.log` |

Toolchain: 1.96.0-x86_64-pc-windows-msvc. Grammar: the tree-sitter-al clone on `main`. CDO: the pinned baseline at `bc3ccb18`. No goldens moved.

## Discoveries

```json
[]
```

## Not delivered
