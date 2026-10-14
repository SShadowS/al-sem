//! Port of al-sem `src/digest/diff-parser.ts`.
//!
//! Minimal unified diff parser: produces a list of DiffFile (with hunks) from
//! `git diff` / `diff -u` output. Each hunk body is read line by line, bounded
//! by the header's old/new counts, and its additions and deletion gaps are
//! recorded as [`HunkChange`]s. Inside a body, header tests (`--- `, `+++ `,
//! `diff --git`, `@@`) do not apply; a line that does not fit the remaining
//! counts ends the hunk as truncated (an error) and is re-read as a top-level
//! line.
//!
//! Handles the 9 spec edge cases: a/b/ prefix stripping, quoted paths, /dev/null,
//! renames, omitted hunk count (defaults to 1), zero-length ranges, CRLF,
//! "\ No newline" marker, and always returns {files, errors}.

#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// 1-based first line of the new file covered by this hunk.
    pub new_start: i32,
    /// Number of new-file lines (may be 0 for pure deletions).
    pub new_count: i32,
    /// Number of old-file lines (may be 0 for pure additions; omitted → 1).
    pub old_count: i32,
    /// The changes the hunk body records, in body order.
    pub changes: Vec<HunkChange>,
}

/// One change of a hunk body. 1-based new-file numbers; `DeletedAfter(L)` = a
/// gap between new lines L and L+1 (one per run of consecutive `-` lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkChange {
    Added(i32),
    DeletedAfter(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffFileKind {
    Modified,
    Added,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone)]
pub struct DiffFile {
    /// The NEW path (b/ stripped, unquoted, forward-slash normalized).
    pub path: String,
    pub kind: DiffFileKind,
    pub old_path: Option<String>,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone)]
pub struct DiffParseResult {
    pub files: Vec<DiffFile>,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_ab_prefix(p: &str) -> &str {
    if p.starts_with("a/") || p.starts_with("b/") {
        &p[2..]
    } else {
        p
    }
}

fn unquote_path(s: &str) -> String {
    let trimmed = s.trim();
    // A single `"` passes BOTH `starts_with('"')` and `ends_with('"')` (it's the
    // same character) — `trimmed.len() - 1` then underflows the "inner" slice's
    // end below 1 (its start), which panics. Anything shorter than a valid empty
    // quoted string (`""`, len 2) can't be unquoted; degrade to the raw token.
    if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
        return trimmed.to_string();
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => result.push('"'),
            Some('\\') => result.push('\\'),
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some('r') => result.push('\r'),
            Some(o1) if o1.is_ascii_digit() => {
                // Octal escape \NNN
                let mut oct = String::new();
                oct.push(o1);
                for _ in 0..2 {
                    if chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        oct.push(chars.next().unwrap());
                    }
                }
                if let Ok(n) = u8::from_str_radix(&oct, 8) {
                    result.push(n as char);
                }
            }
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}

fn extract_path(line: &str) -> Option<String> {
    // Remove leading --- / +++ and one space (line[4..])
    let rest = line[4..].trim_start();
    // /dev/null detection tolerates trailing whitespace (TS matches `rest.trim()`) (#16).
    if rest == "/dev/null" || rest.trim_end() == "/dev/null" {
        return None;
    }
    let unquoted = unquote_path(rest);
    let normalized = unquoted.replace('\\', "/");
    Some(strip_ab_prefix(&normalized).to_string())
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    // Strict port of the TS regex (#15):
    //   /^@@\s+-(\d+)(?:,(\d+))?\s+\+(\d+)(?:,(\d+))?\s+@@/
    // Anchored at the start; trailing text after the closing `@@` is allowed (no `$`).
    // `\s+` requires at least one whitespace at each separator. Malformed `@@` lines
    // that the TS regex rejects (e.g. missing `-`/`+`, no whitespace, non-digit counts)
    // return None here too.
    let bytes = line.as_bytes();
    let mut i = 0usize;

    fn is_ws(b: u8) -> bool {
        // JS \s: space, tab, CR, LF, form-feed, vertical-tab (ASCII subset suffices for diffs).
        matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x0b)
    }
    fn eat_lit(bytes: &[u8], i: &mut usize, lit: &[u8]) -> bool {
        if bytes[*i..].starts_with(lit) {
            *i += lit.len();
            true
        } else {
            false
        }
    }
    fn eat_ws1(bytes: &[u8], i: &mut usize) -> bool {
        let start = *i;
        while *i < bytes.len() && is_ws(bytes[*i]) {
            *i += 1;
        }
        *i > start
    }
    fn eat_digits(bytes: &[u8], i: &mut usize) -> Option<i32> {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == start {
            return None;
        }
        std::str::from_utf8(&bytes[start..*i])
            .ok()?
            .parse::<i32>()
            .ok()
    }

    // ^@@
    if !eat_lit(bytes, &mut i, b"@@") {
        return None;
    }
    // \s+
    if !eat_ws1(bytes, &mut i) {
        return None;
    }
    // -(\d+)
    if i >= bytes.len() || bytes[i] != b'-' {
        return None;
    }
    i += 1;
    eat_digits(bytes, &mut i)?;
    // (?:,(\d+))?  — omitted count → 1
    let mut old_count = 1;
    if i < bytes.len() && bytes[i] == b',' {
        i += 1;
        old_count = eat_digits(bytes, &mut i)?;
    }
    // \s+
    if !eat_ws1(bytes, &mut i) {
        return None;
    }
    // \+(\d+)
    if i >= bytes.len() || bytes[i] != b'+' {
        return None;
    }
    i += 1;
    let new_start = eat_digits(bytes, &mut i)?;
    // (?:,(\d+))?  — omitted count → 1
    let mut new_count = 1;
    if i < bytes.len() && bytes[i] == b',' {
        i += 1;
        new_count = eat_digits(bytes, &mut i)?;
    }
    // \s+
    if !eat_ws1(bytes, &mut i) {
        return None;
    }
    // @@
    if !eat_lit(bytes, &mut i, b"@@") {
        return None;
    }

    Some(DiffHunk {
        new_start,
        new_count,
        old_count,
        changes: Vec::new(),
    })
}

/// The hunk whose body is being read: lines left on each side, and the next
/// new-file line (`cursor`). Git writes `new_start` as the line BEFORE the gap
/// when `new_count == 0`, so the cursor then starts one later.
struct OpenHunk {
    new_start: i32,
    old_left: i32,
    new_left: i32,
    cursor: i32,
    in_deletion_run: bool,
}

impl OpenHunk {
    fn new(h: &DiffHunk) -> Self {
        OpenHunk {
            new_start: h.new_start,
            old_left: h.old_count,
            new_left: h.new_count,
            cursor: if h.new_count == 0 {
                h.new_start + 1
            } else {
                h.new_start
            },
            in_deletion_run: false,
        }
    }

    fn is_complete(&self) -> bool {
        self.old_left <= 0 && self.new_left <= 0
    }

    /// Consume one line as body. `None`: the line is not body (its side has no
    /// count left, or it is not a body line), so the hunk ends INCOMPLETE.
    /// `Some(change)`: accepted; `change` is the change it records, if any.
    fn accept(&mut self, line: &str) -> Option<Option<HunkChange>> {
        match line.as_bytes().first() {
            Some(b'\\') => Some(None),
            None | Some(b' ') if self.old_left > 0 && self.new_left > 0 => {
                self.old_left -= 1;
                self.new_left -= 1;
                self.cursor += 1;
                self.in_deletion_run = false;
                Some(None)
            }
            Some(b'-') if self.old_left > 0 => {
                self.old_left -= 1;
                let starts_run = !self.in_deletion_run;
                self.in_deletion_run = true;
                Some(starts_run.then_some(HunkChange::DeletedAfter(self.cursor - 1)))
            }
            Some(b'+') if self.new_left > 0 => {
                self.new_left -= 1;
                self.in_deletion_run = false;
                let c = HunkChange::Added(self.cursor);
                self.cursor += 1;
                Some(Some(c))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse_unified_diff(text: &str) -> DiffParseResult {
    if text.trim().is_empty() {
        return DiffParseResult {
            files: Vec::new(),
            errors: Vec::new(),
        };
    }

    // Normalize CRLF → LF
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    // A final '\n' is the last line's terminator, not an empty context line.
    if normalized.ends_with('\n') {
        lines.pop();
    }
    let mut open: Option<OpenHunk> = None;

    let mut files: Vec<DiffFile> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // State
    let mut current_file: Option<DiffFile> = None;
    let mut pending_rename_from: Option<String> = None;
    let mut pending_rename_to: Option<String> = None;
    let mut pending_old_path: Option<Option<String>> = None; // None means "not seen"; Some(None) means /dev/null
    let mut in_file = false;

    // Mirrors TS `finalizeFile`: clears the pending rename trackers ONLY when a
    // file was actually finalized (currentFile !== null). pendingOldPath is always
    // reset. This guard is load-bearing for rename-with-hunks classification (#2):
    // a `diff --git → rename from/to → --- → +++` sequence must keep the pending
    // renames alive across the `---` line so the `+++` arm sees them.
    let finalize = |current: &mut Option<DiffFile>,
                    files: &mut Vec<DiffFile>,
                    rename_from: &mut Option<String>,
                    rename_to: &mut Option<String>,
                    old_path: &mut Option<Option<String>>| {
        if let Some(f) = current.take() {
            files.push(f);
            *rename_from = None;
            *rename_to = None;
        }
        *old_path = None;
    };

    for raw_line in &lines {
        // Strip trailing CR
        let line = raw_line.trim_end_matches('\r');

        // Inside a hunk body, header tests do not apply.
        if let Some(h) = open.as_mut() {
            if let Some(change) = h.accept(line) {
                if let Some(c) = change {
                    current_file
                        .as_mut()
                        .and_then(|f| f.hunks.last_mut())
                        .expect("an open hunk belongs to the current file")
                        .changes
                        .push(c);
                }
                if h.is_complete() {
                    open = None;
                }
                continue;
            }
            errors.push(format!("hunk at +{} truncated", h.new_start));
            open = None;
            // The line that ended the hunk is a top-level line: fall through.
        }

        // Edge case 8: "\ No newline at end of file" — skip
        if line.starts_with("\\ ") {
            continue;
        }

        // --- line (old-side path header)
        if line.starts_with("--- ") {
            finalize(
                &mut current_file,
                &mut files,
                &mut pending_rename_from,
                &mut pending_rename_to,
                &mut pending_old_path,
            );
            pending_old_path = Some(extract_path(line));
            in_file = true;
            continue;
        }

        // +++ line (new-side path header)
        if line.starts_with("+++ ") && in_file {
            let new_path = extract_path(line);
            // `old_p`: the old-side path, coalescing both "not seen" (None) and
            // /dev/null (Some(None)) to None — mirrors TS `pendingOldPath ?? null`.
            let old_p: Option<String> = pending_old_path.take().flatten();
            current_file = match new_path {
                None => {
                    // +++ /dev/null → deleted. TS: path = oldP ?? "unknown",
                    // oldPath = oldP ?? undefined (omitted when oldP is null). Bug #1:
                    // the previous code re-read the already-taken pending_old_path → None
                    // → path always "unknown". Now we use the captured old_p.
                    let path = old_p.clone().unwrap_or_else(|| "unknown".to_string());
                    Some(DiffFile {
                        path,
                        kind: DiffFileKind::Deleted,
                        old_path: old_p,
                        hunks: Vec::new(),
                    })
                }
                Some(new_p) => {
                    // pending_old_path was Some(None) ⇒ old_p is None ⇒ `--- /dev/null` → added.
                    // (The `in_file` guard guarantees a `---` was seen, so the only way
                    // old_p is None here is the /dev/null case — matching TS `pendingOldPath === null`.)
                    if let Some(old_real) = old_p {
                        let is_rename =
                            pending_rename_from.is_some() || pending_rename_to.is_some();
                        let kind = if is_rename {
                            DiffFileKind::Renamed
                        } else {
                            DiffFileKind::Modified
                        };
                        let old_path = if is_rename {
                            pending_rename_from.clone()
                        } else if old_real != new_p {
                            Some(old_real)
                        } else {
                            None
                        };
                        Some(DiffFile {
                            path: new_p,
                            kind,
                            old_path,
                            hunks: Vec::new(),
                        })
                    } else {
                        // old_p None ⇒ `--- /dev/null` → added.
                        Some(DiffFile {
                            path: new_p,
                            kind: DiffFileKind::Added,
                            old_path: None,
                            hunks: Vec::new(),
                        })
                    }
                }
            };
            pending_rename_from = None;
            pending_rename_to = None;
            continue;
        }

        // Rename headers
        if let Some(from) = line.strip_prefix("rename from ") {
            pending_rename_from = Some(unquote_path(from.trim()).replace('\\', "/"));
            continue;
        }
        if let Some(to) = line.strip_prefix("rename to ") {
            pending_rename_to = Some(unquote_path(to.trim()).replace('\\', "/"));
            continue;
        }

        // diff --git header → signals end of previous file
        if line.starts_with("diff --git ") {
            // 100% rename with no hunks
            if pending_rename_from.is_some()
                && pending_rename_to.is_some()
                && current_file.is_none()
            {
                files.push(DiffFile {
                    path: pending_rename_to.take().unwrap(),
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
            pending_rename_from = None;
            pending_rename_to = None;
            pending_old_path = None;
            in_file = false;
            continue;
        }

        // Hunk header — pass the full line so the strict `^@@…@@` regex anchors correctly.
        if line.starts_with("@@")
            && let Some(hunk) = parse_hunk_header(line)
            && let Some(ref mut f) = current_file
        {
            let state = OpenHunk::new(&hunk);
            f.hunks.push(hunk);
            if !state.is_complete() {
                open = Some(state);
            }
        }
        // All other lines ignored
    }

    if let Some(h) = open {
        errors.push(format!("hunk at +{} truncated", h.new_start));
    }

    // End-of-input
    if pending_rename_from.is_some() && pending_rename_to.is_some() && current_file.is_none() {
        files.push(DiffFile {
            path: pending_rename_to.take().unwrap(),
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

    DiffParseResult { files, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diff_returns_empty() {
        let r = parse_unified_diff("  ");
        assert!(r.files.is_empty());
        assert!(r.errors.is_empty());
    }

    #[test]
    fn simple_modified_file() {
        let diff = "diff --git a/src/Foo.al b/src/Foo.al\n\
            --- a/src/Foo.al\n\
            +++ b/src/Foo.al\n\
            @@ -10,3 +10,4 @@ SomeContext\n\
            \x20ctx1\n\
            -old\n\
            +new\n\
            +new2\n\
            \x20ctx2\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files.len(), 1);
        let f = &r.files[0];
        assert_eq!(f.path, "src/Foo.al");
        assert_eq!(f.kind, DiffFileKind::Modified);
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.hunks[0].new_start, 10);
        assert_eq!(f.hunks[0].new_count, 4);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn omitted_hunk_count_defaults_to_1() {
        let diff = "diff --git a/F.al b/F.al\n--- a/F.al\n+++ b/F.al\n@@ -5 +5 @@\n-a\n+b\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files[0].hunks[0].new_count, 1);
    }

    #[test]
    fn deleted_file_carries_old_path() {
        // ORACLE (#1): assert the PATH, not just the kind. The previous
        // `deleted_file_is_kind_deleted` test masked a bug where the deleted arm
        // re-read the already-taken pending_old_path → path always "unknown".
        let diff =
            "diff --git a/Old.al b/Old.al\n--- a/Old.al\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files.len(), 1);
        let f = &r.files[0];
        assert_eq!(f.kind, DiffFileKind::Deleted);
        assert_eq!(
            f.path, "Old.al",
            "deleted path must be the captured old path"
        );
        assert_eq!(f.old_path.as_deref(), Some("Old.al"));
    }

    #[test]
    fn added_file_is_kind_added() {
        let diff =
            "diff --git a/New.al b/New.al\n--- /dev/null\n+++ b/New.al\n@@ -0,0 +1,2 @@\n+x\n+y\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files[0].kind, DiffFileKind::Added);
        assert_eq!(r.files[0].path, "New.al");
        assert_eq!(r.files[0].old_path, None);
    }

    #[test]
    fn rename_with_hunks_is_renamed() {
        // ORACLE (#2): `diff --git → rename from/to → --- → +++ → @@` must classify
        // as "renamed" (currentFile is null at `---`, so the pending renames survive).
        // The previous unconditional rename-reset at `---` produced "modified".
        let diff = "diff --git a/Old.al b/New.al\n\
            similarity index 95%\n\
            rename from Old.al\n\
            rename to New.al\n\
            --- a/Old.al\n\
            +++ b/New.al\n\
            @@ -1,3 +1,4 @@\n\
            \x20ctx1\n\
            -old\n\
            +new\n\
            +new2\n\
            \x20ctx2\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files.len(), 1);
        let f = &r.files[0];
        assert_eq!(
            f.kind,
            DiffFileKind::Renamed,
            "must be renamed, not modified"
        );
        assert_eq!(f.path, "New.al");
        assert_eq!(f.old_path.as_deref(), Some("Old.al"));
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.hunks[0].new_start, 1);
        assert_eq!(f.hunks[0].new_count, 4);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn pure_rename_no_hunks_is_renamed() {
        // 100% rename with no content diff — emitted at the next `diff --git` / EOF.
        let diff = "diff --git a/Old.al b/New.al\n\
            similarity index 100%\n\
            rename from Old.al\n\
            rename to New.al\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].kind, DiffFileKind::Renamed);
        assert_eq!(r.files[0].path, "New.al");
        assert_eq!(r.files[0].old_path.as_deref(), Some("Old.al"));
    }

    #[test]
    fn dev_null_added_then_deleted_two_files() {
        // /dev/null on both sides across two file sections.
        let diff = "diff --git a/New.al b/New.al\n--- /dev/null\n+++ b/New.al\n@@ -0,0 +1,1 @@\n+x\n\
            diff --git a/Gone.al b/Gone.al\n--- a/Gone.al\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-y\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files.len(), 2);
        assert_eq!(r.files[0].kind, DiffFileKind::Added);
        assert_eq!(r.files[0].path, "New.al");
        assert_eq!(r.files[1].kind, DiffFileKind::Deleted);
        assert_eq!(r.files[1].path, "Gone.al");
        assert_eq!(r.files[1].old_path.as_deref(), Some("Gone.al"));
    }

    #[test]
    fn zero_count_hunk_parses() {
        // ORACLE (#14): a pure-deletion hunk `@@ -1,2 +5,0 @@` → newCount 0, newStart 5.
        let diff = "diff --git a/F.al b/F.al\n--- a/F.al\n+++ b/F.al\n@@ -1,2 +5,0 @@\n-a\n-b\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files[0].hunks.len(), 1);
        assert_eq!(r.files[0].hunks[0].new_start, 5);
        assert_eq!(r.files[0].hunks[0].new_count, 0);
    }

    #[test]
    fn malformed_hunk_headers_rejected() {
        // ORACLE (#15): lines the strict TS regex rejects must NOT produce a hunk.
        assert!(
            super::parse_hunk_header("@@ +5 @@").is_none(),
            "missing -old"
        );
        assert!(
            super::parse_hunk_header("@@-5 +5@@").is_none(),
            "no whitespace"
        );
        assert!(
            super::parse_hunk_header("@@ -a +5 @@").is_none(),
            "non-digit old"
        );
        assert!(
            super::parse_hunk_header("@@ -5 +5").is_none(),
            "missing closing @@"
        );
        // Valid forms still parse.
        assert!(super::parse_hunk_header("@@ -5,3 +10,4 @@").is_some());
        assert!(super::parse_hunk_header("@@ -5 +10 @@ ctx").is_some());
    }

    #[test]
    fn dev_null_with_trailing_whitespace() {
        // ORACLE (#16): `+++ /dev/null ` (trailing space) is still /dev/null → deleted.
        let diff =
            "diff --git a/Old.al b/Old.al\n--- a/Old.al\n+++ /dev/null \n@@ -1,1 +0,0 @@\n-x\n";
        let r = parse_unified_diff(diff);
        assert_eq!(r.files[0].kind, DiffFileKind::Deleted);
        assert_eq!(r.files[0].path, "Old.al");
    }

    // -----------------------------------------------------------------------
    // T2.4 (1): `unquote_path` lone-quote panic
    // -----------------------------------------------------------------------

    #[test]
    fn unquote_path_lone_quote_does_not_panic() {
        // A single `"` (e.g. a diff truncated to `--- "`) passes BOTH
        // `starts_with('"')` and `ends_with('"')` (same char) — the old code then
        // computed `trimmed[1..0]`, which panics ("slice index starts at 1 but
        // ends at 0"). Must degrade to returning the raw token, never panic.
        assert_eq!(unquote_path("\""), "\"");
    }

    #[test]
    fn unquote_path_empty_quoted_string() {
        // Regression: `""` (len 2, empty inner) must still unquote to empty.
        assert_eq!(unquote_path("\"\""), "");
    }

    #[test]
    fn unquote_path_normal_escaped_regression() {
        // Regression: a normal quoted path with an escape still round-trips.
        assert_eq!(unquote_path("\"a\\\"b.al\""), "a\"b.al");
    }

    #[test]
    fn diff_header_lone_quote_path_does_not_panic() {
        // Reachable from user input: `--- "` (diff truncated mid-header). Must
        // parse to SOME path (or be dropped), never panic the whole digest.
        let diff = "diff --git a/X b/X\n--- \"\n+++ b/X\n@@ -1 +1 @@\n-a\n+b\n";
        let r = parse_unified_diff(diff);
        // No panic reaching here is the assertion; also sanity-check we got a
        // result back (either a parsed file or a recorded parse issue).
        assert!(r.files.len() <= 1);
    }

    #[test]
    fn rename_from_lone_quote_does_not_panic() {
        // Reachable via `rename from "` / `rename to "` (~:329-334), same bug.
        let diff = "diff --git a/Old.al b/New.al\n\
            similarity index 100%\n\
            rename from \"\n\
            rename to \"\n";
        let _ = parse_unified_diff(diff);
    }

    // -----------------------------------------------------------------------
    // #71: count-bounded hunk bodies
    // -----------------------------------------------------------------------

    use super::HunkChange::{Added, DeletedAfter};

    const HEAD: &str = "--- a/x.al\n+++ b/x.al\n";

    fn changes(diff: &str) -> (Vec<Vec<HunkChange>>, Vec<String>) {
        let r = parse_unified_diff(diff);
        (
            r.files
                .iter()
                .flat_map(|f| f.hunks.iter().map(|h| h.changes.clone()))
                .collect(),
            r.errors,
        )
    }

    #[test]
    fn hunk_changes_follow_the_body() {
        // old: ctx, -, -, blank = 4; new: ctx, +, blank, + = 4.
        let (c, e) = changes(&format!(
            "{HEAD}@@ -3,4 +3,4 @@\n ctx\n-old1\n-old2\n+new\n\n+tail\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(3), Added(4), Added(6)]]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn pure_deletion_cursor_starts_after_new_start() {
        let (c, _) = changes(&format!("{HEAD}@@ -9,1 +8,0 @@\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(8)]]);
        let (c, _) = changes(&format!("{HEAD}@@ -1,1 +0,0 @@\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(0)]]);
    }

    #[test]
    fn truncated_hunk_is_an_error_with_or_without_final_newline() {
        for d in [
            format!("{HEAD}@@ -3,2 +3,1 @@\n-old\n"),
            format!("{HEAD}@@ -3,2 +3,1 @@\n-old"),
        ] {
            let (c, e) = changes(&d);
            assert_eq!(e, vec!["hunk at +3 truncated".to_string()], "{d:?}");
            assert_eq!(c, vec![vec![DeletedAfter(2)]], "{d:?}");
        }
    }

    #[test]
    fn header_looking_lines_inside_a_body_are_body() {
        let r = parse_unified_diff(&format!("{HEAD}@@ -9,1 +9,1 @@\n--- x\n+++ y\n"));
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].hunks[0].changes, vec![DeletedAfter(8), Added(9)]);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn the_line_ending_a_truncated_hunk_is_redispatched() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,2 +3,1 @@\n-old\ndiff --git a/y.al b/y.al\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al", "y.al"]);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.files[1].hunks[0].changes, vec![DeletedAfter(8), Added(9)]);
    }

    #[test]
    fn minus_line_past_old_count_ends_the_hunk() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,1 +3,2 @@\n-old\n+a\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al", "y.al"]);
        assert_eq!(r.errors, vec!["hunk at +3 truncated".to_string()]);
    }

    #[test]
    fn exact_fill_completes_the_hunk() {
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,2 +3,1 @@\n-old\n--- a/y.al\n+++ b/y.al\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        assert_eq!(r.files.len(), 1);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let c: Vec<_> = r.files[0].hunks.iter().map(|h| h.changes.clone()).collect();
        assert_eq!(
            c,
            vec![
                vec![DeletedAfter(2), Added(3)],
                vec![DeletedAfter(8), Added(9)]
            ]
        );
    }

    #[test]
    fn no_newline_marker_is_not_counted() {
        let (c, e) = changes(&format!(
            "{HEAD}@@ -9,1 +9,1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(8), Added(9)]]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn omitted_counts_bound_the_body() {
        let r = parse_unified_diff(&format!("{HEAD}@@ -5 +5 @@\n-a\n+b\n ctx-after\n"));
        let h = &r.files[0].hunks[0];
        assert_eq!((h.old_count, h.new_count), (1, 1));
        assert_eq!(h.changes, vec![DeletedAfter(4), Added(5)]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
