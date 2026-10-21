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
//! "\ No newline" marker, and always returns {files, errors, file_errors}. A `@@`
//! line inside a file section that is not a valid hunk header is an error of
//! that file (#72); in a `diff --git` section the lines after it, up to the next
//! `diff --git`, are skipped.

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
    /// A `DeletedAfter` whose deleted lines are one or more complete attribute
    /// groups (each starts with a trimmed `[` and runs, over one or more lines,
    /// until its brackets balance): such a gap right above a routine belongs to
    /// it. Brackets are counted on raw text, so a bracket inside a string
    /// argument (`'[x'`) can misclassify the run.
    DeletedAttributesAfter(i32),
}

impl HunkChange {
    /// The new-file line number the change carries.
    pub fn line(self) -> i32 {
        match self {
            HunkChange::Added(n)
            | HunkChange::DeletedAfter(n)
            | HunkChange::DeletedAttributesAfter(n) => n,
        }
    }
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

/// Unquote a git C-quoted path; an invalid UTF-8 result degrades to U+FFFD.
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
            Some(o1) if is_octal(o1) => {
                // Octal escape \NNN: one byte. git never writes one above \377;
                // such an escape is kept as text, like an unknown escape.
                let mut oct = String::new();
                oct.push(o1);
                for _ in 0..2 {
                    if chars.peek().copied().is_some_and(is_octal) {
                        oct.push(chars.next().unwrap());
                    }
                }
                match u8::from_str_radix(&oct, 8) {
                    Ok(n) => bytes.push(n),
                    Err(_) => {
                        bytes.push(b'\\');
                        bytes.extend_from_slice(oct.as_bytes());
                    }
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

fn is_octal(c: char) -> bool {
    matches!(c, '0'..='7')
}

fn extract_path(line: &str) -> Option<String> {
    // Remove leading --- / +++ and one space (line[4..])
    let rest = line[4..].trim_start();
    // `diff -u` / git write `<path or "quoted path">[<TAB><time>]`; both C-quote a name's own TAB (#111).
    // Residual: a producer writing a TAB-containing name raw and unquoted is cut at that TAB.
    let rest = rest.split_once('\t').map_or(rest, |(path, _)| path);
    // /dev/null detection tolerates trailing whitespace (TS matches `rest.trim()`) (#16).
    if rest == "/dev/null" || rest.trim_end() == "/dev/null" {
        return None;
    }
    let unquoted = unquote_path(rest);
    let normalized = unquoted.replace('\\', "/");
    Some(strip_ab_prefix(&normalized).to_string())
}

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
            // A quote byte is a char boundary, so the slice is safe.
            match quoted_token_len(&rest[i..]) {
                Some(n) => i += n,
                None => break,
            }
        } else {
            i += 1;
        }
    }
    last
}

/// Byte length of the quoted token that `s` starts with, both quotes included.
/// Escaped characters are skipped, so an escaped `\"` does not end the token.
/// `None`: the token is not terminated.
fn quoted_token_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// The destination (b-side) path of a `diff --git <a> <b>` line; `rest` is the
/// text after `diff --git ` (#72). A quoted a-token ends at its closing quote
/// and the b-token follows one space; else a quoted b-token is unquoted; else
/// one space splits the two names; else the line splits at its middle space
/// when both halves name the same path. Anything else is ambiguous: `None`
/// (for a non-rename, git writes unquoted names with spaces only when both
/// sides are equal; a rename's `rename to` line then names the file).
fn git_header_dest(rest: &str) -> Option<String> {
    let rest = rest.trim_end();
    let b = if rest.starts_with('"') {
        rest[quoted_token_len(rest)?..].strip_prefix(' ')?
    } else if rest.ends_with('"') {
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

/// The destination of a section header line: `Some(dest)` for `diff --git`
/// (its b-side, see [`git_header_dest`]) and for a merge's `diff --cc` /
/// `diff --combined` (its one path); `None` for any other line.
fn section_header_dest(line: &str) -> Option<Option<String>> {
    if let Some(rest) = line.strip_prefix("diff --git ") {
        return Some(git_header_dest(rest));
    }
    let path = line
        .strip_prefix("diff --cc ")
        .or_else(|| line.strip_prefix("diff --combined "))?;
    Some(Some(unquote_path(path).replace('\\', "/")))
}

/// `None`: not a hunk header. `Some(Err(_))`: a hunk header whose line range does
/// not fit in i32 (#71) — reported, never opened, so no cursor step can overflow.
fn parse_hunk_header(line: &str) -> Option<Result<DiffHunk, String>> {
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
    /// Digits saturate at i64::MAX; the range check below rejects them.
    fn eat_digits(bytes: &[u8], i: &mut usize) -> Option<i64> {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == start {
            return None;
        }
        let digits = std::str::from_utf8(&bytes[start..*i]).ok()?;
        Some(digits.parse::<i64>().unwrap_or(i64::MAX))
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
    let old_start = eat_digits(bytes, &mut i)?;
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

    // The body cursor runs up to `new_start + max(new_count, 1)` (`OpenHunk`), so
    // that sum must fit in i32; then no cursor step can overflow.
    let fits = |n: i64| i32::try_from(n).is_ok();
    let end = new_start.checked_add(new_count.max(1));
    if !(fits(old_start) && fits(old_count) && end.is_some_and(fits)) {
        return Some(Err(format!(
            "hunk at +{new_start} has an invalid line range"
        )));
    }
    Some(Ok(DiffHunk {
        new_start: new_start as i32,
        new_count: new_count as i32,
        old_count: old_count as i32,
        changes: Vec::new(),
    }))
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
    /// The current deletion run is still only complete attribute groups...
    attribute_run: bool,
    /// ...and this is the bracket depth of its open group (0 = between groups).
    attribute_depth: i32,
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
            attribute_run: false,
            attribute_depth: 0,
        }
    }

    fn is_complete(&self) -> bool {
        self.old_left <= 0 && self.new_left <= 0
    }

    /// Consume one line as body, recording its change in `changes` (the open
    /// hunk's). `false`: the line is not body (its side has no count left, or it
    /// is not a body line), so the hunk ends INCOMPLETE.
    fn accept(&mut self, line: &str, changes: &mut Vec<HunkChange>) -> bool {
        match line.as_bytes().first() {
            Some(b'\\') => true,
            None | Some(b' ') if self.old_left > 0 && self.new_left > 0 => {
                self.old_left -= 1;
                self.new_left -= 1;
                self.cursor += 1;
                self.in_deletion_run = false;
                true
            }
            Some(b'-') if self.old_left > 0 => {
                self.old_left -= 1;
                let text = line[1..].trim();
                if !self.in_deletion_run {
                    self.attribute_run = true;
                    self.attribute_depth = 0;
                    changes.push(HunkChange::DeletedAfter(self.cursor - 1));
                }
                // The run is attribute-only while it is a sequence of complete
                // attribute groups: each starts with `[` (at depth 0) and runs
                // until its brackets balance. Any other text between groups
                // (after a closing `]` only whitespace, another `[` group or a
                // `//` comment may follow on its line), or a stray `]`, makes the
                // whole run a plain deletion.
                if self.attribute_depth == 0 && !text.starts_with('[') {
                    self.attribute_run = false;
                }
                if self.attribute_run {
                    for (i, c) in text.char_indices() {
                        match c {
                            '[' => self.attribute_depth += 1,
                            ']' if self.attribute_depth > 0 => self.attribute_depth -= 1,
                            _ if self.attribute_depth > 0 || c.is_whitespace() => {}
                            _ if text[i..].starts_with("//") => break,
                            _ => {
                                self.attribute_run = false;
                                break;
                            }
                        }
                    }
                }
                // Classified after every line, so a group still open when the run
                // ends leaves a plain deletion.
                if let Some(last) = changes.last_mut() {
                    let l = last.line();
                    *last = if self.attribute_run && self.attribute_depth == 0 {
                        HunkChange::DeletedAttributesAfter(l)
                    } else {
                        HunkChange::DeletedAfter(l)
                    };
                }
                self.in_deletion_run = true;
                true
            }
            Some(b'+') if self.new_left > 0 => {
                self.new_left -= 1;
                self.in_deletion_run = false;
                changes.push(HunkChange::Added(self.cursor));
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }
}

/// #72: the header-error state of the current file section.
#[derive(Default)]
struct Section {
    /// Header errors not yet resolved to a file.
    pending: Vec<String>,
    /// The path the section is about (its record's path once it emits one).
    dest: Option<String>,
    /// The section began with a `diff --git`/`--cc`/`--combined` line.
    git: bool,
    /// After a header error in a git section, the rejected hunk's body has no
    /// trustworthy length: skip every line up to the next section header.
    skip: bool,
}

impl Section {
    /// Closes the section: its pending errors go to `dest` (global without
    /// one), and every field resets.
    fn close(&mut self, file_errors: &mut Vec<FileError>, errors: &mut Vec<String>) {
        for detail in self.pending.drain(..) {
            match &self.dest {
                Some(path) => file_errors.push(FileError {
                    path: path.clone(),
                    detail,
                }),
                None => errors.push(detail),
            }
        }
        *self = Section::default();
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
            file_errors: Vec::new(),
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

    // #72: header errors are pending per file section and resolved to the
    // section's path when it closes. `in_section`: a section header, `---` or
    // `rename` line has been seen (a `@@` line before that is a preamble line,
    // e.g. a format-patch message, and is ignored).
    let mut file_errors: Vec<FileError> = Vec::new();
    let mut section = Section::default();
    let mut in_section = false;

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
        let header_dest = section_header_dest(line);
        if section.skip && header_dest.is_none() {
            continue;
        }

        // Inside a hunk body, header tests do not apply.
        if let Some(h) = open.as_mut() {
            // `open` is set only right after its hunk is pushed onto
            // `current_file`, and no header arm runs while it is open, so this
            // always finds the hunk. Should that ever break, drop the open hunk
            // instead of panicking.
            if let Some(hunk) = current_file.as_mut().and_then(|f| f.hunks.last_mut()) {
                if h.accept(line, &mut hunk.changes) {
                    if h.is_complete() {
                        open = None;
                    }
                    continue;
                }
                errors.push(format!("hunk at +{} truncated", h.new_start));
            }
            open = None;
            // The line that ended the hunk is a top-level line: fall through.
        }

        // Edge case 8: "\ No newline at end of file" — skip
        if line.starts_with("\\ ") {
            continue;
        }

        // --- line (old-side path header)
        if line.starts_with("--- ") {
            // A `---` closes the section when a record is open or errors are
            // pending; right after `diff --git` it closes nothing, so the git
            // destination survives.
            if current_file.is_some() || !section.pending.is_empty() {
                section.close(&mut file_errors, &mut errors);
            }
            finalize(
                &mut current_file,
                &mut files,
                &mut pending_rename_from,
                &mut pending_rename_to,
                &mut pending_old_path,
            );
            let old = extract_path(line);
            if section.dest.is_none() {
                section.dest = old.clone();
            }
            pending_old_path = Some(old);
            in_section = true;
            continue;
        }

        // +++ line (new-side path header). It opens a file only right after a
        // `---` header: an orphan `+++` (e.g. a hunk's overrunning `+` line,
        // re-read at top level) must not replace the current file and lose its
        // hunks (#71).
        if line.starts_with("+++ ") && pending_old_path.is_some() {
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
                    // (The `pending_old_path` guard guarantees a `---` was seen, so the only way
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
            section.dest = current_file.as_ref().map(|f| f.path.clone());
            pending_rename_from = None;
            pending_rename_to = None;
            continue;
        }

        // Rename headers
        if let Some(from) = line.strip_prefix("rename from ") {
            pending_rename_from = Some(unquote_path(from.trim()).replace('\\', "/"));
            in_section = true;
            continue;
        }
        if let Some(to) = line.strip_prefix("rename to ") {
            // No `a/`/`b/` strip: git writes rename paths bare, and the rename
            // record's `path` is this same string, so the veto and the record agree.
            let to = unquote_path(to.trim()).replace('\\', "/");
            section.dest = Some(to.clone());
            pending_rename_to = Some(to);
            in_section = true;
            continue;
        }

        // Section header (`diff --git`, or a merge's `diff --cc`/`--combined`)
        // → signals end of previous file
        if let Some(dest) = header_dest {
            // 100% rename with no hunks
            if pending_rename_from.is_some()
                && pending_rename_to.is_some()
                && current_file.is_none()
            {
                let path = pending_rename_to.take().unwrap();
                section.dest = Some(path.clone());
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
            section.close(&mut file_errors, &mut errors);
            section.dest = dest;
            section.git = true;
            in_section = true;
            pending_rename_from = None;
            pending_rename_to = None;
            pending_old_path = None;
            continue;
        }

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
                section.pending.push(e);
                section.skip = section.git;
            }
        }
        // All other lines ignored
    }

    if let Some(h) = open {
        errors.push(format!("hunk at +{} truncated", h.new_start));
    }

    // End-of-input
    if pending_rename_from.is_some() && pending_rename_to.is_some() && current_file.is_none() {
        let path = pending_rename_to.take().unwrap();
        section.dest = Some(path.clone());
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
    section.close(&mut file_errors, &mut errors);

    DiffParseResult {
        files,
        errors,
        file_errors,
    }
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
    fn plain_diff_u_timestamped_dev_null_is_an_addition() {
        // #111 U1: GNU `diff -u` headers are `<path><TAB><timestamp>` (probe
        // P7c). `--- /dev/null<TAB><time>` makes the file ADDED; CRLF endings
        // and spaces before the TAB give the same path.
        for (eol, pad) in [("\n", ""), ("\r\n", ""), ("\n", "  "), ("\r\n", " ")] {
            let diff = format!(
                "--- /dev/null\t1970-01-01 00:00:00.000000000 +0000{eol}\
                 +++ ./src/new.al{pad}\t2026-10-21 13:02:06.656738697 +0000{eol}\
                 @@ -0,0 +1,2 @@{eol}+x{eol}+y{eol}"
            );
            let r = parse_unified_diff(&diff);
            assert_eq!(r.files.len(), 1, "{diff:?}");
            let f = &r.files[0];
            assert_eq!(f.path, "./src/new.al", "{diff:?}");
            assert_eq!(f.kind, DiffFileKind::Added, "{diff:?}");
            assert_eq!(f.old_path, None, "{diff:?}");
            assert_eq!(f.hunks.len(), 1, "{diff:?}");
            assert_eq!(f.hunks[0].new_start, 1, "{diff:?}");
            assert_eq!(f.hunks[0].new_count, 2, "{diff:?}");
            assert_eq!(
                f.hunks[0].changes,
                vec![HunkChange::Added(1), HunkChange::Added(2)],
                "{diff:?}"
            );
            assert!(r.errors.is_empty() && r.file_errors.is_empty(), "{diff:?}");
        }
    }

    #[test]
    fn git_headers_keep_their_path_under_the_tab_cut() {
        // #111 U2, git 2.53.0 bytes (probe P7b): an unquoted name with a space
        // gets a trailing raw TAB; a name with a TAB is C-quoted, its TAB the
        // escaped `\t`, decoded after the cut.
        let space = "diff --git a/with space.al b/with space.al\n\
            --- a/with space.al\t\n\
            +++ b/with space.al\t\n\
            @@ -1 +1 @@\n-x\n+y\n";
        let r = parse_unified_diff(space);
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].path, "with space.al");
        assert_eq!(r.files[0].kind, DiffFileKind::Modified);
        let tab = "diff --git \"a/with\\ttab.al\" \"b/with\\ttab.al\"\n\
            --- \"a/with\\ttab.al\"\n\
            +++ \"b/with\\ttab.al\"\n\
            @@ -1 +1 @@\n-x\n+y\n";
        let r = parse_unified_diff(tab);
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].path, "with\ttab.al");
        assert_eq!(r.files[0].kind, DiffFileKind::Modified);
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
    fn unquote_path_octal_escapes_are_utf8_bytes() {
        // #72 (astra R3-A01): git quotes a non-ASCII name as octal escapes of its
        // UTF-8 bytes; `\303\251` is ONE char, `é`, not `Ã©`.
        assert_eq!(unquote_path("\"src/\\303\\251.al\""), "src/\u{e9}.al");
        // git's other C escapes.
        assert_eq!(unquote_path("\"\\a\\b\\f\\v\""), "\u{7}\u{8}\u{c}\u{b}");
        // An invalid sequence degrades to U+FFFD, never a panic.
        assert_eq!(unquote_path("\"\\303.al\""), "\u{fffd}.al");
        // flash FR1-F02: git never writes `8`/`9` in an octal escape or one
        // above \377; such text is kept, never silently dropped.
        assert_eq!(unquote_path("\"a\\8b\""), "a\\8b");
        assert_eq!(unquote_path("\"a\\18b\""), "a\u{1}8b");
        assert_eq!(unquote_path("\"a\\777b\""), "a\\777b");
    }

    #[test]
    fn git_header_dest_names() {
        // #72: the b-side path of a `diff --git` line (the text after `diff --git `).
        let d = |rest: &str| super::git_header_dest(rest);
        assert_eq!(d("a/src/x.al b/src/x.al").as_deref(), Some("src/x.al"));
        // Unquoted names with spaces: for a non-rename, git writes them only
        // when both sides are equal, so the line splits at its middle space.
        assert_eq!(
            d("a/src/my file.al b/src/my file.al").as_deref(),
            Some("src/my file.al")
        );
        // A rename's unequal unquoted names with spaces cannot be split (its
        // `rename to` line names the file).
        assert_eq!(d("a/src/old name.al b/src/new.al"), None);
        // astra FR1-A01: a quoted a-token ends at its closing quote, so its
        // spaces are not separators and the unquoted b-token follows.
        assert_eq!(
            d("\"a/src/old\\303\\251 name.al\" b/src/posting.al").as_deref(),
            Some("src/posting.al")
        );
        assert_eq!(
            d("\"a/x\\\" y.al\" b/my file.al").as_deref(),
            Some("my file.al")
        );
        // Quoted b-token, decoded by the one decoder.
        assert_eq!(
            d("\"a/src/\\303\\251.al\" \"b/src/\\303\\251.al\"").as_deref(),
            Some("src/\u{e9}.al")
        );
        assert_eq!(
            d("a/old.al \"b/\\303\\251.al\"").as_deref(),
            Some("\u{e9}.al")
        );
        // flash R4: the quoted b-token's start is found by a scan that skips
        // escaped quotes (a backward search for `"` lands inside the token).
        assert_eq!(
            d("\"a/x\\\"y.al\" \"b/x\\\"y.al\"").as_deref(),
            Some("x\"y.al")
        );
        // A rename header takes the b side.
        assert_eq!(
            d("a/src/old.al b/src/new.al").as_deref(),
            Some("src/new.al")
        );
        // Ambiguous: several spaces and unequal halves.
        assert_eq!(d("a/src/a b.al b/src/c d.al"), None);
    }

    #[test]
    fn malformed_hunk_header_is_an_error_of_its_file() {
        // #72: a `@@` line that fails the header grammar inside a file section is
        // an error of that file (it was silently ignored); the next file is not
        // affected.
        let r = parse_unified_diff(
            "--- a/x.al\n+++ b/x.al\n@@ garbage @@\n-a\n+b\n--- a/y.al\n+++ b/y.al\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.file_errors.len(), 1, "{:?}", r.file_errors);
        assert_eq!(r.file_errors[0].path, "x.al");
        assert!(
            r.file_errors[0]
                .detail
                .contains("malformed hunk header `@@ garbage @@`"),
            "{:?}",
            r.file_errors
        );
        let y = r.files.iter().find(|f| f.path == "y.al").expect("y.al");
        assert_eq!(y.hunks.len(), 1);
        assert_eq!(y.hunks[0].changes, vec![DeletedAfter(0), Added(1)]);

        // CR-02: a section with no path (an unsplittable `diff --git` line and
        // nothing after it) keeps its error as a global one.
        let r = parse_unified_diff("diff --git a/x y.al b/z w.al\n@@ garbage @@\n");
        assert!(r.file_errors.is_empty(), "{:?}", r.file_errors);
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(
            r.errors[0].contains("malformed hunk header"),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn combined_diff_headers_are_section_boundaries() {
        // flash FR1-F01: a merge's `diff --cc`/`diff --combined` line starts a
        // section, so a rejected `@@@` header's skip ends there and every
        // later file keeps its own error. The first section (binary, no
        // record) must not leak into the next one.
        let r = parse_unified_diff(
            "diff --git a/b.dat b/b.dat\nBinary files a/b.dat and b/b.dat differ\n\
             diff --cc x.al\n--- a/x.al\n+++ b/x.al\n@@@ -1 -1 +1 @@@\n- a\n +b\n\
             diff --combined y.al\n--- a/y.al\n+++ b/y.al\n@@@ -1 -1 +1 @@@\n- a\n +b\n\
             diff --git a/z.al b/z.al\n--- a/z.al\n+++ b/z.al\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let paths: Vec<&str> = r.file_errors.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al", "y.al"]);
        let z = r.files.iter().find(|f| f.path == "z.al").expect("z.al");
        assert_eq!(z.hunks[0].changes, vec![DeletedAfter(0), Added(1)]);
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
            "{HEAD}@@ -3,2 +3,1 @@\n-old\n@@ -9,1 +9,1 @@\n-x\n+y\n"
        ));
        // The `@@` line that ends the truncated hunk must open the second hunk;
        // swallowing it loses that hunk (`-x`/`+y` are then top-level noise).
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.errors, vec!["hunk at +3 truncated".to_string()]);
        let c: Vec<_> = r.files[0].hunks.iter().map(|h| h.changes.clone()).collect();
        assert_eq!(
            c,
            vec![vec![DeletedAfter(2)], vec![DeletedAfter(8), Added(9)]]
        );
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
    fn out_of_range_hunk_header_is_an_error_not_a_panic() {
        // The cursor runs up to `new_start + max(new_count, 1)`; a header whose
        // range does not fit in i32 is reported and opens no hunk.
        for (header, body) in [
            ("@@ -1,1 +2147483647,0 @@", "-x\n"),
            ("@@ -1 +2147483647,2 @@", "+a\n+b\n"),
            ("@@ -1 +2147483648 @@", "-x\n+y\n"),
            ("@@ -99999999999999999999999 +1 @@", "-x\n+y\n"),
            ("@@ -2147483648 +1 @@", "-x\n+y\n"),
        ] {
            let r = parse_unified_diff(&format!("{HEAD}{header}\n{body}"));
            assert_eq!(r.files.len(), 1, "{header}");
            assert!(r.files[0].hunks.is_empty(), "{header}");
            // #72: a header error belongs to its file, not to the global list.
            assert!(r.errors.is_empty(), "{header}: {:?}", r.errors);
            assert_eq!(r.file_errors.len(), 1, "{header}: {:?}", r.file_errors);
            assert_eq!(r.file_errors[0].path, "x.al", "{header}");
            assert!(
                r.file_errors[0].detail.contains("invalid line range"),
                "{:?}",
                r.file_errors
            );
        }
        // The largest ranges that fit.
        let (c, e) = changes(&format!("{HEAD}@@ -1,1 +2147483646,1 @@\n-x\n+y\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2147483645), Added(2147483646)]]);
        assert!(e.is_empty(), "{e:?}");
        let (c, e) = changes(&format!("{HEAD}@@ -1,1 +2147483646,0 @@\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2147483646)]]);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn plus_header_without_a_pending_minus_header_keeps_the_current_file() {
        // `+new` fills the new count, so `+++ b/y.al` overruns the hunk and is
        // re-read at top level. With no `---` before it, it must not replace
        // (and lose) `x.al`, whose hunk already holds `Added(3)`.
        let r = parse_unified_diff(&format!(
            "{HEAD}@@ -3,2 +3,1 @@\n-old\n+new\n+++ b/y.al\n@@ -9 +9 @@\n-x\n+y\n"
        ));
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["x.al"]);
        let c: Vec<_> = r.files[0].hunks.iter().map(|h| h.changes.clone()).collect();
        assert_eq!(
            c,
            vec![
                vec![DeletedAfter(2), Added(3)],
                vec![DeletedAfter(8), Added(9)]
            ]
        );
        assert_eq!(r.errors, vec!["hunk at +3 truncated".to_string()]);
    }

    #[test]
    fn attribute_only_deletion_runs() {
        use super::HunkChange::DeletedAttributesAfter;
        // Every deleted line of the run is an attribute line (trimmed `[`).
        let (c, _) = changes(&format!("{HEAD}@@ -3,2 +2,0 @@\n-  [A]\n-[B(1)]\n"));
        assert_eq!(c, vec![vec![DeletedAttributesAfter(2)]]);
        // One other line, after or before, makes it a plain deletion.
        let (c, _) = changes(&format!("{HEAD}@@ -3,2 +2,0 @@\n-[A]\n-x\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2)]]);
        let (c, _) = changes(&format!("{HEAD}@@ -3,2 +2,0 @@\n-x\n-[A]\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2)]]);
        // A multi-line attribute group is one group once its brackets balance.
        let (c, _) = changes(&format!(
            "{HEAD}@@ -3,3 +2,0 @@\n-    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"X\",\n-        'OnAfterPost', '', false, false)]\n-    [A]\n"
        ));
        assert_eq!(c, vec![vec![DeletedAttributesAfter(2)]]);
        // A complete group followed by other text, or a group left open, is not.
        let (c, _) = changes(&format!(
            "{HEAD}@@ -3,3 +2,0 @@\n-    [B(1,\n-        2)]\n-    // note\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(2)]]);
        // Code after a group's closing `]` on its line is not attribute text; a
        // trailing `//` comment is.
        let (c, _) = changes(&format!(
            "{HEAD}@@ -3,2 +2,0 @@\n-    [IntegrationEvent(\n-        false, false)] local procedure Gone() begin end;\n"
        ));
        assert_eq!(c, vec![vec![DeletedAfter(2)]]);
        let (c, _) = changes(&format!("{HEAD}@@ -3,1 +2,0 @@\n-    [A] // note\n"));
        assert_eq!(c, vec![vec![DeletedAttributesAfter(2)]]);
        let (c, _) = changes(&format!("{HEAD}@@ -3,1 +2,0 @@\n-    [B(1,\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2)]]);
        // Separate runs are classified separately.
        let (c, _) = changes(&format!("{HEAD}@@ -3,3 +3,1 @@\n-x\n ctx\n-[A]\n"));
        assert_eq!(c, vec![vec![DeletedAfter(2), DeletedAttributesAfter(3)]]);
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
