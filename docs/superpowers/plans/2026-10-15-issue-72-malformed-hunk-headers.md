# Issue #72: malformed hunk headers are a parse error of their file — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest --diff` reports a malformed (or out-of-range) hunk header as one
`diff-parse-error` that names its file, and takes NO diff-derived roots from that file,
instead of silently treating the file as hunkless and rooting every declaration in it.

**Architecture:** The parser (`src/engine/l5/diff_parser.rs`) keeps `parse_hunk_header` as
the pure grammar oracle. The caller turns a rejected `@@` line into a header error, holds it
as pending for the current file section, and resolves it to the section's path (the record
it emitted, else the destination named by `diff --git` / `rename to` / `---`) in a new
`DiffParseResult.file_errors`. In a `diff --git` section, every line after a header error
up to the next `diff --git` is skipped. `unquote_path` decodes octal escapes as UTF-8 bytes,
so every path spelling agrees. The consumer (`src/engine/l5/digest_cli.rs`) projects the file
on the diagnostic and vetoes every diff record whose NORMALIZED path is poisoned.

**Tech Stack:** Rust (edition 2024), `cargo test`, `serde_json`.

**Spec:** `.agent/issue-72/ledger.md`, section `## Design` (converged after spec panel round
4), incl. its acceptance matrix and round-4 implementation note. The red tests are recorded
in that ledger's `## Acceptance tests (red)`.

## Global Constraints

- Worktree `U:\Git\al-sem-issue-72-a1`, branch `issue/72-agent-discovery-digest-diff-par-a1`, base `5c67fcf9`.
- `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` before every cargo command (the worktree has no grammar clone).
- Never set `CDO_WS`.
- Format with `rustfmt <file>` only, never `cargo fmt`.
- Edit with the Edit/Write tools only, never scripts (discrimination breaks included).
- No `#[ignore` and no `#[allow(` in added lines.
- Do not touch `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, `Cargo.toml` version fields, `tree-sitter-al`.
- No `git stash`; stage only the named paths, never `git add -A`.
- Do not pipe a gate through `| tail`; redirect to a log and grep it.
- Wire string: the diagnostic kind stays `diff-parse-error`; the new key is `file`, omitted when there is no file.

## Review Focus

1. A header error in a git section whose `diff --git` names cannot be split (unequal halves with spaces) and that has no `rename to`/`---`/`+++` path: the error must still be reported (global, no `file`), and nothing is vetoed. Pinned by `git_header_dest_names` (ambiguous -> `None`) plus the resolution fallback in Step 3; git never writes such a line.
2. A `format-patch --stdout` series whose second mail's commit message has a line starting with `@@`: it is read inside the previous file's section and vetoes that file (stated limit in the Design, not fixed). Reviewers should not "fix" it in this task.
3. A truncated hunk (valid header, short body) must keep its #71 behaviour: a global error with no `file`, roots from the lines read. Pinned by the existing `hunk_body_is_count_bounded` (c)/(c1), `bare_header_pair_inside_short_hunk_is_body`, `minus_line_past_old_count_ends_the_hunk`.
4. `--file` / `--routine` inputs must still give their roots when the diff poisons the same file (the veto covers diff-derived roots only). The code keeps them by construction: they are collected before the diff block. No new test; the reviewer checks the `continue` sits inside the diff loop only.
5. A literal-UTF-8 name that is not valid UTF-8 after octal decoding (`"\303.al"`) must not panic: it decodes to U+FFFD. Pinned by `unquote_path_octal_escapes_are_utf8_bytes`.

---

### Task 1: Header errors belong to their file; the digest vetoes that file

One task: the acceptance tests are already written (uncommitted) and the parser change alone
would turn the CLI tests from "no error" to "error without veto", so tests, parser, consumer
and CHANGELOG land in one commit.

**Files:**
- Modify: `src/engine/l5/diff_parser.rs` (types `:69-73`, `unquote_path` `:87-131`, new `git_header_dest`, `parse_unified_diff` `:360-589`; tests already edited)
- Modify: `src/engine/l5/digest_cli.rs` (`ChangedRootsDiagnostic::DiffParseError` `:179-181`, diff block `:419-466`, projection `:1257-1260`)
- Modify: `CHANGELOG.md` (`## [Unreleased]` -> `### Fixed`, line 193; the #71 entry at `:274-280`)
- Test (already written, uncommitted): `tests/cli/cli_digest_changed_spans.rs`, `src/engine/l5/diff_parser.rs` `mod tests`

**Interfaces:**
- Produces (diff_parser, `pub`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct FileError { pub path: String, pub detail: String }
  pub struct DiffParseResult { pub files: Vec<DiffFile>, pub errors: Vec<String>, pub file_errors: Vec<FileError> }
  ```
- Produces (diff_parser, private): `fn git_header_dest(rest: &str) -> Option<String>` where `rest` is the text after `diff --git `; `fn quoted_b_start(rest: &str) -> Option<usize>`.
- Produces (digest_cli): `ChangedRootsDiagnostic::DiffParseError { file: Option<String>, detail: String }`, JSON `{"kind":"diff-parse-error","file":…,"detail":…}`, `file` omitted when `None`.

- [ ] **Step 1: Confirm the red state**

```bash
cd /u/Git/al-sem-issue-72-a1 && export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al
cargo test --test cli cli_digest_changed_spans:: > target/t72-red-cli.log 2>&1; echo exit=$?
grep -E 'test result|FAILED' target/t72-red-cli.log
cargo test -p al-sem --lib diff_parser > target/t72-red-lib.log 2>&1; echo exit=$?
grep -E '^error' target/t72-red-lib.log | sort | uniq -c
```
Expected: CLI `28 passed; 11 failed` (the 11 listed in the ledger); lib does not compile (10 x E0609 `file_errors`, 1 x E0425 `git_header_dest`).

- [ ] **Step 2: Parser types and the byte-level decoder**

In `diff_parser.rs`, replace `DiffParseResult` (`:69-73`) with:

```rust
/// A hunk-header error of one file section (#72): the header failed the grammar
/// or its range does not fit. `path` is the path the section is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct DiffParseResult {
    pub files: Vec<DiffFile>,
    /// Errors with no file: truncated hunks, and header errors whose section
    /// names no path.
    pub errors: Vec<String>,
    /// Header errors, each attached to its file section's path (#72).
    pub file_errors: Vec<FileError>,
}
```

Replace the body of `unquote_path` after the `inner` slice (`:96-130`) so it collects bytes:

```rust
    let inner = &trimmed[1..trimmed.len() - 1];
    // git quotes a name as C escapes of its BYTES: `\303\251` is the two UTF-8
    // bytes of `é`, so escapes push bytes and the buffer is decoded once (#72).
    let mut bytes: Vec<u8> = Vec::with_capacity(inner.len());
    let mut buf = [0u8; 4];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('"') => bytes.push(b'"'),
            Some('\\') => bytes.push(b'\\'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('r') => bytes.push(b'\r'),
            Some('a') => bytes.push(0x07),
            Some('b') => bytes.push(0x08),
            Some('f') => bytes.push(0x0c),
            Some('v') => bytes.push(0x0b),
            Some(o1) if o1.is_ascii_digit() => {
                // Octal escape \NNN: one byte.
                let mut oct = String::new();
                oct.push(o1);
                for _ in 0..2 {
                    if chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        oct.push(chars.next().unwrap());
                    }
                }
                if let Ok(n) = u8::from_str_radix(&oct, 8) {
                    bytes.push(n);
                }
            }
            Some(other) => {
                bytes.push(b'\\');
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
```

Update the doc of `unquote_path` (add one line above `fn unquote_path`): `/// Unquote a git C-quoted path; an invalid UTF-8 result degrades to U+FFFD.`

- [ ] **Step 3: `git_header_dest`**

Add after `extract_path`:

```rust
/// Byte index of the opening quote of the LAST quoted token of `rest`. Scans
/// forward and skips escaped characters inside a token, so an escaped `\"`
/// is never taken for a token boundary (a backward search for `"` would land
/// inside the token).
fn quoted_b_start(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut last = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            last = Some(i);
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        i += 1;
    }
    last
}

/// The destination (b-side) path of a `diff --git <a> <b>` line; `rest` is the
/// text after `diff --git ` (#72). A quoted b-token is unquoted; else one space
/// splits the two names; else the line splits at its middle space when both
/// halves name the same path (git writes unquoted names with spaces only when
/// both sides are equal). Anything else is ambiguous: `None`.
fn git_header_dest(rest: &str) -> Option<String> {
    let rest = rest.trim_end();
    let b = if rest.ends_with('"') {
        &rest[quoted_b_start(rest)?..]
    } else if rest.matches(' ').count() == 1 {
        rest.split_once(' ')?.1
    } else {
        // A space byte is always a char boundary, so the slices are safe.
        let mid = rest.len() / 2;
        if rest.as_bytes().get(mid) != Some(&b' ') {
            return None;
        }
        let (a, b) = (&rest[..mid], &rest[mid + 1..]);
        if strip_ab_prefix(a) != strip_ab_prefix(b) {
            return None;
        }
        b
    };
    let p = unquote_path(b).replace('\\', "/");
    Some(strip_ab_prefix(&p).to_string())
}
```

- [ ] **Step 4: Header errors in `parse_unified_diff`**

4a. The empty-input return (`:362-365`) gains `file_errors: Vec::new(),`.

4b. After `let mut pending_old_path …` (`:384`) add the section state:

```rust
    // #72: header errors are pending per file section and resolved to the
    // section's path when it closes. `section_dest`: the path the section is
    // about (its record's path once it emits one). `in_section`: a `diff --git`,
    // `---` or `rename` line has been seen (a `@@` line before that is a
    // preamble line, e.g. a format-patch message, and is ignored).
    let mut file_errors: Vec<FileError> = Vec::new();
    let mut pending_header_errors: Vec<String> = Vec::new();
    let mut section_dest: Option<String> = None;
    let mut in_section = false;
    let mut git_section = false;
    // After a header error in a `diff --git` section, the rejected hunk's body
    // has no trustworthy length: skip every line up to the next `diff --git`.
    let mut skip_to_git = false;
    let resolve = |pending: &mut Vec<String>,
                   dest: &Option<String>,
                   file_errors: &mut Vec<FileError>,
                   errors: &mut Vec<String>| {
        for detail in pending.drain(..) {
            match dest {
                Some(path) => file_errors.push(FileError {
                    path: path.clone(),
                    detail,
                }),
                None => errors.push(detail),
            }
        }
    };
```

4c. At the top of the loop body, right after `let line = raw_line.trim_end_matches('\r');`:

```rust
        if skip_to_git && !line.starts_with("diff --git ") {
            continue;
        }
```

(`open` is always `None` while skipping: a header error opens no hunk.)

4d. `---` arm (`:433-443`) becomes:

```rust
        if line.starts_with("--- ") {
            // A `---` closes the section when a record is open or errors are
            // pending; right after `diff --git` it closes nothing, so the git
            // destination survives.
            if current_file.is_some() || !pending_header_errors.is_empty() {
                resolve(
                    &mut pending_header_errors,
                    &section_dest,
                    &mut file_errors,
                    &mut errors,
                );
                section_dest = None;
                git_section = false;
            }
            finalize(
                &mut current_file,
                &mut files,
                &mut pending_rename_from,
                &mut pending_rename_to,
                &mut pending_old_path,
            );
            let old = extract_path(line);
            if section_dest.is_none() {
                section_dest = old.clone();
            }
            pending_old_path = Some(old);
            in_section = true;
            continue;
        }
```

4e. `+++` arm: after the `current_file = match new_path { … };` assignment, before `pending_rename_from = None;`, add:

```rust
            section_dest = current_file.as_ref().map(|f| f.path.clone());
```

4f. Rename arms (`:510-517`) become:

```rust
        if let Some(from) = line.strip_prefix("rename from ") {
            pending_rename_from = Some(unquote_path(from.trim()).replace('\\', "/"));
            in_section = true;
            continue;
        }
        if let Some(to) = line.strip_prefix("rename to ") {
            let to = unquote_path(to.trim()).replace('\\', "/");
            section_dest = Some(to.clone());
            pending_rename_to = Some(to);
            in_section = true;
            continue;
        }
```

(`section_dest` takes the same string as the rename record's `path`, so the veto and the
record agree. The Design's "strip_ab_prefix for all paths" applies to `---`/`+++`/`diff --git`
spellings, which carry `a/`/`b/`; `rename` lines carry none.)

4g. `diff --git` arm (`:520-545`): in the hunkless-rename branch, take the path first and
make it the section destination; then resolve; then open the new section:

```rust
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // 100% rename with no hunks
            if pending_rename_from.is_some()
                && pending_rename_to.is_some()
                && current_file.is_none()
            {
                let path = pending_rename_to.take().unwrap();
                section_dest = Some(path.clone());
                files.push(DiffFile {
                    path,
                    kind: DiffFileKind::Renamed,
                    old_path: pending_rename_from.take(),
                    hunks: Vec::new(),
                });
            } else {
                finalize(
                    &mut current_file,
                    &mut files,
                    &mut pending_rename_from,
                    &mut pending_rename_to,
                    &mut pending_old_path,
                );
            }
            resolve(
                &mut pending_header_errors,
                &section_dest,
                &mut file_errors,
                &mut errors,
            );
            skip_to_git = false;
            section_dest = git_header_dest(rest);
            git_section = true;
            in_section = true;
            pending_rename_from = None;
            pending_rename_to = None;
            pending_old_path = None;
            continue;
        }
```

4h. Hunk-header arm (`:547-562`) becomes:

```rust
        // Hunk header — pass the full line so the strict `^@@…@@` regex anchors
        // correctly. Inside a file section, a header the grammar rejects (#72)
        // or whose range does not fit (#71) is an error of that section.
        if line.starts_with("@@") {
            let header_error = match parse_hunk_header(line) {
                Some(Ok(hunk)) => {
                    if let Some(f) = current_file.as_mut() {
                        let state = OpenHunk::new(&hunk);
                        f.hunks.push(hunk);
                        if !state.is_complete() {
                            open = Some(state);
                        }
                    }
                    None
                }
                Some(Err(e)) => Some(e),
                None => Some(format!("malformed hunk header `{line}`")),
            };
            if let Some(e) = header_error
                && in_section
            {
                pending_header_errors.push(e);
                if git_section {
                    skip_to_git = true;
                }
            }
        }
        // All other lines ignored
```

4i. End of input (`:570-588`): in the rename branch take the path first like 4g, then resolve
and return the new field:

```rust
    if pending_rename_from.is_some() && pending_rename_to.is_some() && current_file.is_none() {
        let path = pending_rename_to.take().unwrap();
        section_dest = Some(path.clone());
        files.push(DiffFile {
            path,
            kind: DiffFileKind::Renamed,
            old_path: pending_rename_from.take(),
            hunks: Vec::new(),
        });
    } else {
        finalize(
            &mut current_file,
            &mut files,
            &mut pending_rename_from,
            &mut pending_rename_to,
            &mut pending_old_path,
        );
    }
    resolve(
        &mut pending_header_errors,
        &section_dest,
        &mut file_errors,
        &mut errors,
    );

    DiffParseResult {
        files,
        errors,
        file_errors,
    }
```

Update the module doc (`:11-13`): replace "and always returns {files, errors}." with
"and always returns {files, errors, file_errors}. A `@@` line inside a file section that is
not a valid hunk header is an error of that file (#72); in a `diff --git` section the lines
after it, up to the next `diff --git`, are skipped."

```bash
rustfmt --edition 2024 src/engine/l5/diff_parser.rs
cargo test -p al-sem --lib diff_parser > target/t72-lib.log 2>&1; echo exit=$?
grep -E 'test result|FAILED' target/t72-lib.log
```
Expected: all `diff_parser` tests pass (incl. `malformed_hunk_header_is_an_error_of_its_file`, `git_header_dest_names`, `unquote_path_octal_escapes_are_utf8_bytes`, updated `out_of_range_hunk_header_is_an_error_not_a_panic`, and every untouched truncation test).

- [ ] **Step 5: Consumer — file on the diagnostic, normalized-path veto**

In `digest_cli.rs`, the variant (`:179-181`):

```rust
    DiffParseError {
        /// The file the error belongs to (a hunk-header error, #72); `None`
        /// for a truncated hunk or a header error whose section names no path.
        file: Option<String>,
        detail: String,
    },
```

Diff block: replace the errors loop (`:424-428`) with:

```rust
        for err in &parsed.errors {
            diagnostics.push(ChangedRootsDiagnostic::DiffParseError {
                file: None,
                detail: err.clone(),
            });
        }
        for fe in &parsed.file_errors {
            diagnostics.push(ChangedRootsDiagnostic::DiffParseError {
                file: Some(fe.path.clone()),
                detail: fe.detail.clone(),
            });
        }
        // A file with a hunk-header error gives no diff-derived roots, in ANY of
        // its records: the dropped hunk's changes are unknown, so whole-file roots
        // would hide the error (#72). Keyed by the normalized path, the key
        // `spans.of_file` matches with.
        let poisoned: std::collections::HashSet<String> = parsed
            .file_errors
            .iter()
            .map(|fe| normalize_input(&fe.path))
            .collect();
```

and in the per-record loop, right after the `DiffFileUnmatched` check's closing `}`
(before `if diff_file.hunks.is_empty()`):

```rust
            if poisoned.contains(&normalize_input(&diff_file.path)) {
                continue;
            }
```

Projection (`:1257-1260`):

```rust
        ChangedRootsDiagnostic::DiffParseError { file, detail } => {
            m.insert("kind".into(), "diff-parse-error".into());
            if let Some(file) = file {
                m.insert("file".into(), file.clone().into());
            }
            m.insert("detail".into(), detail.clone().into());
        }
```

Update the hunkless comment (`:445`) to: `// Rename (or mode change) with no content — every declaration. A file whose hunk header was rejected never gets here (poisoned above).`

```bash
rustfmt --edition 2024 src/engine/l5/digest_cli.rs
cargo build --bin alsem > target/t72-build.log 2>&1; echo exit=$?
grep -rn 'DiffParseError' src tests | grep -v 'file' 
```
Expected: build exit 0; the grep lists no construction or match of `DiffParseError` that lacks `file` (if one shows, add `file: None` / `file, ..`).

- [ ] **Step 6: Run the acceptance tests green**

```bash
cargo test --test cli cli_digest_changed_spans:: > target/t72-cli.log 2>&1; echo exit=$?
grep -E 'test result|FAILED' target/t72-cli.log
cargo test -p al-sem --lib diff_parser > target/t72-lib.log 2>&1; echo exit=$?
grep -E 'test result|FAILED' target/t72-lib.log
```
Expected: CLI `39 passed; 0 failed`; lib all pass. Record both counts in the ledger's
`## Acceptance matrix` section (test name -> green).

- [ ] **Step 7: Discrimination proofs**

Each break is made with Edit, the named test is run (`cargo test --test cli cli_digest_changed_spans::<name>` or `cargo test -p al-sem --lib diff_parser::tests::<name>`, output redirected to `target/t72-dp-<n>.log`), the red line is recorded, the break is reverted with Edit, and the test is re-run green. Confirm each break actually applied (the Edit succeeded and `git diff` shows exactly that hunk) before trusting a red. Record every row in the ledger's `## Discrimination proofs` as `| # | break | test | red (quoted) | reverted green |`.

| # | break (in production code) | test that must go red | predicted red |
|---|----------------------------|-----------------------|---------------|
| 1 | delete the `if poisoned.contains(…) { continue; }` block | `malformed_hunk_header_gives_parse_error_and_no_roots` | roots `["OnAfterPostSalesDoc", "PostSalesDoc"]` (alone) not empty |
| 2 | in 4h, make the `None` arm `None => None` (ignore a non-matching `@@` line) | `malformed_hunk_header_gives_parse_error_and_no_roots` | `exactly one diff-parse-error` left: 0 |
| 3 | in `parse_hunk_header`, return `None` when text follows the closing `@@` (reject the heading form) | `malformed_hunk_header_gives_parse_error_and_no_roots` | precondition `@@ -8,3 +8,3 @@ procedure PostSalesDoc()`: roots empty / diag present |
| 4 | in Step 5, push `file: None` for `file_errors` too | `malformed_hunk_poisons_only_its_own_file` | `e["file"]` left: Null |
| 5 | in Step 5, poison every record when `file_errors` is non-empty (`if !parsed.file_errors.is_empty() { continue; }`) | `malformed_hunk_poisons_only_its_own_file` | roots `[]` vs `["HandlePosted"]` |
| 6 | poison per record: replace the `poisoned` key with the record index (veto only `files` whose path appears in `file_errors` AND that have no hunks) — simplest form: `if poisoned.contains(…) && diff_file.hunks.is_empty()` | `malformed_record_poisons_every_record_of_its_path` | roots `["PostSalesDoc"]` not empty |
| 7 | key the veto by the raw path (`fe.path.clone()` / `diff_file.path.clone()`, no `normalize_input`) | `malformed_record_poisons_every_record_of_its_path` | `src/Posting.al` case: roots `["PostSalesDoc"]` |
| 8 | drop `section_dest` from resolution (pass `&None` to every `resolve` call) | `git_section_error_before_plus_header_vetoes_its_path` | `e["file"]` left: Null |
| 9 | in 4g, `section_dest = None;` instead of `git_header_dest(rest)` | `git_header_names_the_file_of_an_immediate_header_error` | `e["file"]` left: Null |
| 10 | in `git_header_dest`, take the a side (`rest.split_once(' ')?.0` in the one-space branch) | `git_header_names_the_file_of_an_immediate_header_error` (old/new case) and `git_header_dest_names` | file `"src/old.al"`; unit `Some("src/old.al")` vs `Some("src/new.al")` |
| 11 | in `unquote_path`, push an octal escape as `char::from(n)` UTF-8 bytes (each escape its own char) | `escaped_and_literal_spellings_of_one_file_share_the_veto`, `unquote_path_octal_escapes_are_utf8_bytes` | precondition: `diff-file-unmatched` `src/Ã©.al`; unit `"src/Ã©.al"` |
| 12 | in `quoted_b_start`, drop the escape skip (`if bytes[i] == b'\\' { i += 1; }`) | `git_header_dest_names` | escaped-quote case not `Some("x\"y.al")` |
| 13 | in 4h, never set `skip_to_git` | `rejected_hunk_body_never_opens_a_file` | roots contain `HandlePosted` |
| 14 | in 4g, delete `skip_to_git = false;` | `rejected_body_skip_resumes_at_next_git_section` | roots `[]`, exit 2 |
| 15 | in 4d, close only when `current_file.is_some()` (drop `|| !pending_header_errors.is_empty()`) | `plain_section_error_stays_with_its_file` | `e["file"]` = `"src/subscriber.al"`, roots `[]` |
| 16 | in 4f/4g/4i, do not set `section_dest` from `rename to` nor from the hunkless-rename record | `malformed_header_in_a_rename_section_gives_no_roots` | `e["file"]` left: Null, both roots |
| 17 | in 4h, drop the `&& in_section` condition | `preamble_hunk_header_is_not_an_error` | one `diff-parse-error` |
| 18 | in 4h, make `Some(Err(e))` push to `errors` (global) instead of pending | `out_of_range_hunk_header_is_a_parse_error`, `out_of_range_hunk_header_is_an_error_not_a_panic` | `e["file"]` Null, both roots; unit `r.errors.len()` 1 |
| 19 | in `parse_hunk_header`, drop the `fits` range check (return `Some(Ok(…))` always) | `out_of_range_hunk_header_is_a_parse_error` | process exit 101 (`attempt to add with overflow`), JSON parse panic |
| 20 | apply the veto to clean renames (`if diff_file.hunks.is_empty() && diff_file.kind == DiffFileKind::Renamed { continue; }` before the hunkless branch) | `rename_with_and_without_hunks` | pure rename roots `[]` |

A proof that comes back GREEN is evidence about the test: stop, find out why (a redundant
path, a break that did not apply), and record it rather than moving on.

- [ ] **Step 8: CHANGELOG**

Under `## [Unreleased]` -> `### Fixed` (line 193), add as the first entries:

```markdown
- `alsem digest --diff`: a hunk header that is not a valid `@@ -a,b +c,d @@` line (e.g.
  `@@ garbage @@`, `@@ -8,3 @@`, a combined-diff `@@@` header) was silently ignored, so the
  file looked hunkless and EVERY declaration in it became a changed root, with no
  diagnostic and exit 0. It is now a `diff-parse-error` with a new `file` key naming the
  file, and that file gives no diff-derived roots in any of its records (matched by the
  normalized path; `--file`/`--routine` roots are kept). In a `diff --git` section the lines
  after the bad header, up to the next `diff --git`, are skipped: the rejected hunk's body has
  no trustworthy length. A digest with such an error is incomplete for that file; a diff
  whose only file is broken gives no roots and exits 2. An `@@` line before the first file
  section (a patch preamble) is still ignored. Pinned by
  `malformed_hunk_header_gives_parse_error_and_no_roots` and nine sibling tests in
  `tests/cli/cli_digest_changed_spans.rs`, and `malformed_hunk_header_is_an_error_of_its_file`
  / `git_header_dest_names` in `diff_parser` (#72).
- `alsem digest --diff`: a git-quoted non-ASCII path (git's default, e.g.
  `"b/src/\303\251.al"` for `src/é.al`) decoded each octal escape as its own character
  (`src/Ã©.al`), so a healthy diff of the file gave `diff-file-unmatched`, no roots and exit
  2. Escapes are now decoded as UTF-8 bytes (`\a \b \f \v` added). Pinned by
  `unquote_path_octal_escapes_are_utf8_bytes` and
  `escaped_and_literal_spellings_of_one_file_share_the_veto` (#72).
```

In the #71 entry (`:274-280`), replace "and opens no hunk; a file
  left with no hunk then takes every declaration, as a hunkless rename does." with
"and opens no hunk. (#72 then made it an error of its file: that file gives no roots.)"

- [ ] **Step 9: Goldens and the task gate**

Expected golden movement: NONE (no golden carries a `diff-parse-error`; the one `--diff`
golden, `tests/cli-b-goldens/digest/ws-d8-commit-in-tx.changed.diff`, is valid with ASCII
paths). Do not regen.

```bash
scripts/check-goldens > target/t72-goldens.log 2>&1; echo exit=$?
grep -E 'FAIL|ok|passed|failed' target/t72-goldens.log | tail -20
scripts/ci-steps task > target/t72-task.log 2>&1; echo exit=$?
grep -E 'FAIL|error|test result' target/t72-task.log | tail -30
git status --short
```
Expected: both exit 0; `git status` shows only `CHANGELOG.md`,
`src/engine/l5/diff_parser.rs`, `src/engine/l5/digest_cli.rs`,
`tests/cli/cli_digest_changed_spans.rs` (plus the untracked `.agent/` and this plan). If a
golden moves, stop: that contradicts the Design; triage it, do not regen.

- [ ] **Step 10: Commit**

```bash
git add CHANGELOG.md src/engine/l5/diff_parser.rs src/engine/l5/digest_cli.rs tests/cli/cli_digest_changed_spans.rs docs/superpowers/plans/2026-10-15-issue-72-malformed-hunk-headers.md
git commit -m "digest --diff: a malformed hunk header is a parse error of its file, never whole-file roots (#72)"
```
