//! Shared text helpers for the L2/L3 projection (tree-sitter-free).
//!
//! Quote stripping mirrors `src/parser/ast.ts`; [`Utf16Cols`] is the single choke
//! point for column emission (an identity pass-through over the byte column the
//! grammar/anchors already use — see the note on the type).

/// Strip surrounding double quotes — matches al-sem `stripQuotes`: only strips
/// when text is >= 2 chars AND starts with `"` AND ends with `"`.
pub fn strip_quotes(text: &str) -> &str {
    let mut chars = text.chars();
    let first = chars.next();
    let last = chars.next_back();
    if first == Some('"') && last == Some('"') {
        &text[1..text.len() - 1]
    } else {
        text
    }
}

/// Column converter, keyed per source.
///
/// EMPIRICAL FINDING (R1a Task 2): al-sem's anchors use UTF-8 byte columns within
/// a line (web-tree-sitter reports byte columns), so `col` is an identity
/// pass-through over the byte column. The type is retained as the single choke
/// point for column emission in case a future grammar/binding diverges.
pub struct Utf16Cols<'a> {
    _source: &'a str,
}

impl<'a> Utf16Cols<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { _source: source }
    }

    /// Return the byte column verbatim (matches al-sem's anchors).
    pub fn col(&self, _row: usize, byte_col: usize) -> u32 {
        byte_col as u32
    }
}
