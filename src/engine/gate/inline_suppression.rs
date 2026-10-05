//! Inline `// al-sem-ignore <detectorId>[: reason]` suppression — faithful port of
//! al-sem `src/cli/inline-suppression.ts`.
//!
//! The parse tree is not retained after indexing, so directives are scanned by
//! RE-READING the workspace `.al` source from disk. A workspace source unit's id is
//! `ws:<relpath>`; given the workspace root the absolute path is `<root>/<relpath>`.
//!
//! DIRECTIVE SYNTAX: `// al-sem-ignore <detector-id>[: <reason>]`.
//!
//! SUPPRESSION WINDOW: a directive suppresses a finding whose `primary_location.line`
//! (1-based) equals the directive's line OR the directive's line + 1 (same line OR the
//! line immediately following — the directive sits above the offending statement). The
//! `+1` is exact: a blank line between directive and finding is NOT covered. Keyed by
//! `(file, detectorId)` — robust to exact column position.
//!
//! DEFAULT-ON: applied automatically like a compiler pragma. The count of suppressed
//! findings is surfaced to the caller (the bin emits it to stderr).

use std::collections::HashMap;
use std::path::Path;

use crate::engine::gate::projection::FindingSummary;

/// A parsed `// al-sem-ignore` directive found in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSuppression {
    /// The detector id named by the directive (e.g. "d47-io-unsafe-txn").
    pub detector_id: String,
    /// 1-based line number of the directive comment itself.
    pub directive_line: u32,
    /// The reason text after the `: ` separator, if present.
    pub reason: Option<String>,
}

/// Map of sourceUnitId → InlineSuppression[].
pub type SuppressionMap = HashMap<String, Vec<InlineSuppression>>;

/// Parse `// al-sem-ignore` directives from raw source text. Line numbers are 1-based.
///
/// Mirrors the al-sem regex `/\/\/\s*al-sem-ignore\s+([\w-]+)(?:\s*:\s*(.+))?/`:
///   - optional whitespace after `//`,
///   - the literal `al-sem-ignore`,
///   - ≥1 whitespace, then the detector id `[\w-]+` (word chars + hyphen),
///   - optionally `:` (with surrounding whitespace) then a non-empty reason `(.+)`.
///
/// The regex is UNANCHORED (matches anywhere in the line) and takes the FIRST match,
/// exactly like `DIRECTIVE_RE.exec(line)`.
pub fn parse_inline_suppressions_from_source(source: &str) -> Vec<InlineSuppression> {
    let mut result = Vec::new();
    for (i, line) in source.split('\n').enumerate() {
        if let Some(s) = match_directive(line, (i + 1) as u32) {
            result.push(s);
        }
    }
    result
}

/// Match the al-sem `DIRECTIVE_RE` against one line. Returns the parsed directive or
/// `None`. Implemented by hand (no regex crate) to keep the byte-for-byte semantics of
/// the JS regex explicit and dependency-free.
fn match_directive(line: &str, line_no: u32) -> Option<InlineSuppression> {
    // Find `//` then `al-sem-ignore` allowing optional whitespace between them. The JS
    // regex is unanchored, so scan every `//` occurrence and take the first that matches.
    let mut search_from = 0usize;
    while let Some(rel) = line[search_from..].find("//") {
        let slash = search_from + rel;
        let mut p = slash + 2; // past "//"
        // \s* — optional whitespace (Unicode-aware: JS \s covers NBSP etc.).
        p = skip_js_ws(line, p);
        // literal "al-sem-ignore"
        const LIT: &str = "al-sem-ignore";
        if line[p..].starts_with(LIT) {
            let mut q = p + LIT.len();
            // \s+ — at least one whitespace before the detector id.
            let ws_start = q;
            q = skip_js_ws(line, q);
            if q > ws_start {
                // [\w-]+ — detector id (word chars: [A-Za-z0-9_] plus hyphen).
                // Detector ids are ASCII-only so byte-based is correct here.
                let id_start = q;
                let bytes = line.as_bytes();
                while q < bytes.len() && is_word_or_hyphen(bytes[q]) {
                    q += 1;
                }
                if q > id_start {
                    let detector_id = line[id_start..q].to_string();
                    // (?:\s*:\s*(.+))? — optional reason.
                    let reason = parse_reason(&line[q..]);
                    return Some(InlineSuppression {
                        detector_id,
                        directive_line: line_no,
                        reason,
                    });
                }
            }
        }
        // Advance by ONE so a valid `//` at offset+1 (e.g. inside `///`) is not skipped.
        // JS regex is unanchored and scans character-by-character; `+2` would step over
        // the second `/` of `///`, causing `/// al-sem-ignore <id>` to be missed.
        search_from = slash + 1;
    }
    None
}

/// Parse the optional `(?:\s*:\s*(.+))?` reason tail. `(.+)` requires ≥1 char and `.`
/// does not match a newline (we operate on a single line). The captured reason is then
/// `.trim()`-ed and an empty trim collapses to `None` (al-sem `m[2]?.trim() || undefined`).
fn parse_reason(tail: &str) -> Option<String> {
    let mut p = skip_js_ws(tail, 0);
    if p >= tail.len() || tail.as_bytes()[p] != b':' {
        return None;
    }
    p += 1; // past ':'
    p = skip_js_ws(tail, p);
    // (.+) — needs at least one remaining char.
    if p >= tail.len() {
        return None;
    }
    let raw = &tail[p..];
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Advance byte offset `pos` in `s` past any JS `\s` characters. Returns the new
/// byte offset. Operates char-by-char so multi-byte Unicode whitespace is handled
/// correctly (JS `\s` covers U+00A0 NBSP, U+1680, U+2000–U+200A, U+2028, U+2029,
/// U+202F, U+205F, U+3000, U+FEFF in addition to the ASCII whitespace set).
/// Newlines (U+000A) never reach here — input is split on `\n` first.
fn skip_js_ws(s: &str, mut pos: usize) -> usize {
    while pos < s.len() {
        // Decode the next char at this byte offset.
        let ch = match s[pos..].chars().next() {
            Some(c) => c,
            None => break,
        };
        if is_js_ws_char(ch) {
            pos += ch.len_utf8();
        } else {
            break;
        }
    }
    pos
}

/// JS `\s` — the full whitespace set as defined in the ECMAScript spec.
/// Matches the union of WhiteSpace and LineTerminator characters, minus U+000A
/// (newline — never present here because input is split on `\n`).
fn is_js_ws_char(c: char) -> bool {
    matches!(
        c,
        // ASCII whitespace: TAB, VT, FF, CR, SPACE
        '\t' | '\x0B' | '\x0C' | '\r' | ' '
        // Unicode whitespace added by JS \s (ECMAScript "WhiteSpace" + "LineTerminator"):
        | '\u{00A0}' // NO-BREAK SPACE
        | '\u{1680}' // OGHAM SPACE MARK
        | '\u{2000}' // EN QUAD
        | '\u{2001}' // EM QUAD
        | '\u{2002}' // EN SPACE
        | '\u{2003}' // EM SPACE
        | '\u{2004}' // THREE-PER-EM SPACE
        | '\u{2005}' // FOUR-PER-EM SPACE
        | '\u{2006}' // SIX-PER-EM SPACE
        | '\u{2007}' // FIGURE SPACE
        | '\u{2008}' // PUNCTUATION SPACE
        | '\u{2009}' // THIN SPACE
        | '\u{200A}' // HAIR SPACE
        | '\u{2028}' // LINE SEPARATOR
        | '\u{2029}' // PARAGRAPH SEPARATOR
        | '\u{202F}' // NARROW NO-BREAK SPACE
        | '\u{205F}' // MEDIUM MATHEMATICAL SPACE
        | '\u{3000}' // IDEOGRAPHIC SPACE
        | '\u{FEFF}' // ZERO WIDTH NO-BREAK SPACE (BOM)
    )
}

/// JS `[\w-]` = `[A-Za-z0-9_-]`. Detector ids are always ASCII.
fn is_word_or_hyphen(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Scan a source file on disk for directives. Missing/unreadable → empty (no error).
pub fn load_inline_suppressions(absolute_path: &Path) -> Vec<InlineSuppression> {
    match crate::source_text::read_al_source(absolute_path) {
        Ok(content) => parse_inline_suppressions_from_source(&content),
        Err(_) => Vec::new(),
    }
}

/// Build a `SuppressionMap` by scanning the given workspace source units. Only `ws:`
/// prefixed units are scanned (dependency `.app` sources are not workspace-authored).
/// `<root>/<relpath>` reconstructs the absolute path (relpath uses `/`; `Path::join`
/// handles it on every platform).
pub fn build_suppression_map<'a, I>(workspace_root: &Path, source_unit_ids: I) -> SuppressionMap
where
    I: IntoIterator<Item = &'a str>,
{
    let mut map: SuppressionMap = HashMap::new();
    for unit_id in source_unit_ids {
        let Some(rel) = unit_id.strip_prefix("ws:") else {
            continue;
        };
        let abs = workspace_root.join(rel);
        let suppressions = load_inline_suppressions(&abs);
        if !suppressions.is_empty() {
            map.insert(unit_id.to_string(), suppressions);
        }
    }
    map
}

/// Result of applying inline suppressions: the kept and the suppressed indices into the
/// caller's finding list.
pub struct SuppressionOutcome {
    /// Indices (into the input slice) of findings that were KEPT.
    pub kept: Vec<usize>,
    /// Indices (into the input slice) of findings that were SUPPRESSED.
    pub suppressed: Vec<usize>,
}

/// Apply inline suppressions to projected findings. A finding is suppressed when:
///   1. its `primary_location.file` (sourceUnitId) appears in the map,
///   2. the map has a directive with the finding's exact `detector` id,
///   3. its `primary_location.line` (1-based) equals the directive's `directive_line`
///      OR `directive_line + 1`.
///
/// Returns index lists so the caller can keep its `(summary, raw)` pairing intact.
pub fn apply_inline_suppressions(
    findings: &[FindingSummary],
    suppressions: &SuppressionMap,
) -> SuppressionOutcome {
    let mut kept = Vec::new();
    let mut suppressed = Vec::new();
    for (i, finding) in findings.iter().enumerate() {
        let file = &finding.primary_location.file;
        let line = finding.primary_location.line;
        let matched = match suppressions.get(file) {
            None => false,
            Some(file_suppressions) => file_suppressions.iter().any(|s| {
                s.detector_id == finding.detector
                    && (line == s.directive_line || line == s.directive_line + 1)
            }),
        };
        if matched {
            suppressed.push(i);
        } else {
            kept.push(i);
        }
    }
    SuppressionOutcome { kept, suppressed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directive_with_reason() {
        let s = parse_inline_suppressions_from_source(
            "        // al-sem-ignore d47-io-unsafe-txn: reviewed, idempotent endpoint\n",
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detector_id, "d47-io-unsafe-txn");
        assert_eq!(s[0].directive_line, 1);
        assert_eq!(
            s[0].reason.as_deref(),
            Some("reviewed, idempotent endpoint")
        );
    }

    #[test]
    fn parses_bare_directive_no_reason() {
        let s = parse_inline_suppressions_from_source("// al-sem-ignore d1-db-op-in-loop");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detector_id, "d1-db-op-in-loop");
        assert_eq!(s[0].reason, None);
    }

    #[test]
    fn empty_reason_collapses_to_none() {
        let s = parse_inline_suppressions_from_source("// al-sem-ignore d1-db-op-in-loop:   ");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].reason, None);
    }

    #[test]
    fn non_directive_lines_ignored() {
        let s =
            parse_inline_suppressions_from_source("// just a comment\nClient.Get('x', Resp);\n");
        assert!(s.is_empty());
    }

    #[test]
    fn directive_plus_one_window() {
        let map: SuppressionMap = HashMap::from([(
            "ws:src/A.al".to_string(),
            vec![InlineSuppression {
                detector_id: "d47-io-unsafe-txn".to_string(),
                directive_line: 14,
                reason: None,
            }],
        )]);
        let mk = |line: u32| FindingSummary {
            id: format!("f{line}"),
            fingerprint: format!("fp{line}"),
            detector: "d47-io-unsafe-txn".to_string(),
            title: String::new(),
            root_cause: String::new(),
            severity: "critical".to_string(),
            confidence_level: "high".to_string(),
            confidence_capped_by: None,
            primary_location: crate::engine::gate::projection::FindingLocation {
                file: "ws:src/A.al".to_string(),
                line,
                column: 1,
                object_id: None,
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            path_count: 1,
            fix_hint: None,
        };
        // line 14 (same), 15 (+1) suppressed; 13 and 16 kept.
        let findings = vec![mk(13), mk(14), mk(15), mk(16)];
        let out = apply_inline_suppressions(&findings, &map);
        assert_eq!(out.suppressed, vec![1, 2]);
        assert_eq!(out.kept, vec![0, 3]);
    }

    #[test]
    fn wrong_detector_does_not_suppress() {
        let map: SuppressionMap = HashMap::from([(
            "ws:src/A.al".to_string(),
            vec![InlineSuppression {
                detector_id: "d1-db-op-in-loop".to_string(),
                directive_line: 14,
                reason: None,
            }],
        )]);
        let finding = FindingSummary {
            id: "f".to_string(),
            fingerprint: "fp".to_string(),
            detector: "d47-io-unsafe-txn".to_string(),
            title: String::new(),
            root_cause: String::new(),
            severity: "critical".to_string(),
            confidence_level: "high".to_string(),
            confidence_capped_by: None,
            primary_location: crate::engine::gate::projection::FindingLocation {
                file: "ws:src/A.al".to_string(),
                line: 15,
                column: 1,
                object_id: None,
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            path_count: 1,
            fix_hint: None,
        };
        let out = apply_inline_suppressions(&[finding], &map);
        assert_eq!(out.kept, vec![0]);
        assert!(out.suppressed.is_empty());
    }

    // -------------------------------------------------------------------------
    // Fix #1 oracles — triple-slash and odd-offset `//` forms (FIX: slash+1 advance)
    // -------------------------------------------------------------------------

    /// `/// al-sem-ignore d47` — triple-slash: the `//` at offset 0 matches, then the
    /// third `/` becomes part of `\s*` scan... wait: `\s*` skips whitespace only, and `/`
    /// is not whitespace, so the `//` at offset 0 tries to match but sees `/al-sem-ignore`
    /// which fails (no whitespace + no literal). With `slash+1` we retry `//` at offset 1
    /// (the second and third `/`) and that `//` is followed by ` al-sem-ignore d47`. JS
    /// returns `d47`; Rust must now also return `Some("d47")`.
    #[test]
    fn fix1_triple_slash_matches_js() {
        let s = parse_inline_suppressions_from_source("/// al-sem-ignore d47");
        assert_eq!(s.len(), 1, "triple-slash should match like JS");
        assert_eq!(s[0].detector_id, "d47");
    }

    /// `//// al-sem-ignore d48` — four slashes: `//` at offset 0 fails (`//` at 2 fails
    /// too but `//` at 2 is followed by ` al-sem-ignore d48` which matches). With
    /// `slash+1` the scan finds `//` at offset 1 (fails), then `//` at offset 2
    /// (` al-sem-ignore d48` — matches). JS returns `d48`.
    #[test]
    fn fix1_quad_slash_matches_js() {
        let s = parse_inline_suppressions_from_source("//// al-sem-ignore d48");
        assert_eq!(s.len(), 1, "four-slash should match like JS");
        assert_eq!(s[0].detector_id, "d48");
    }

    /// Plain `// al-sem-ignore d49` still works after the +1 fix.
    #[test]
    fn fix1_plain_double_slash_still_works() {
        let s = parse_inline_suppressions_from_source("// al-sem-ignore d49");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detector_id, "d49");
    }

    /// Mid-line pragma `x := 1; // al-sem-ignore d47` — JS unanchored, matches anywhere.
    /// Verify before AND after the fix (was always correct, but confirm it still is).
    #[test]
    fn mid_line_pragma_matches_unanchored() {
        let s = parse_inline_suppressions_from_source("x := 1; // al-sem-ignore d47");
        assert_eq!(s.len(), 1, "mid-line pragma must match (JS unanchored)");
        assert_eq!(s[0].detector_id, "d47");
    }

    // -------------------------------------------------------------------------
    // Fix #2 oracles — Unicode whitespace (JS \s superset, Fix: is_js_ws_char)
    // -------------------------------------------------------------------------

    /// NBSP (U+00A0) after `//` — `//\u{00A0}al-sem-ignore d47` — JS `\s` matches NBSP
    /// as whitespace; Rust must now also match.
    #[test]
    fn fix2_nbsp_after_double_slash_matches_js() {
        // Build the line programmatically so the NBSP is unambiguous.
        let line = "//\u{00A0}al-sem-ignore d47".to_string();
        let s = parse_inline_suppressions_from_source(&line);
        assert_eq!(
            s.len(),
            1,
            "NBSP after // should be treated as JS \\s whitespace"
        );
        assert_eq!(s[0].detector_id, "d47");
    }

    /// NBSP between `al-sem-ignore` and detector id — `// al-sem-ignore\u{00A0}d47` —
    /// JS `\s+` before the id matches NBSP; Rust must also match.
    #[test]
    fn fix2_nbsp_before_detector_id_matches_js() {
        let line = "// al-sem-ignore\u{00A0}d47".to_string();
        let s = parse_inline_suppressions_from_source(&line);
        assert_eq!(
            s.len(),
            1,
            "NBSP before detector id should be treated as JS \\s"
        );
        assert_eq!(s[0].detector_id, "d47");
    }
}
