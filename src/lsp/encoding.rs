//! Byte<->UTF-16 column conversion (H-12) + LSP `positionEncoding` negotiation.
//!
//! The engine is UTF-8-byte-column-native throughout (`al-syntax`'s IR, the
//! call graph, everything downstream). LSP's mandatory fallback encoding is
//! UTF-16 code units (every client MUST support it per the 3.17 spec; UTF-8
//! is opt-in via `general.positionEncodings`). This module bridges the two
//! without depending on `lsp_types`, so it stays trivially unit-testable.

use std::ops::Range;
use std::sync::Arc;

/// Which column encoding an LSP session negotiated with the client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

/// Pick a [`PositionEncoding`] from the client's advertised
/// `general.positionEncodings` capability values (LSP 3.17, `general`
/// client capabilities).
///
/// Per spec, UTF-16 is the mandatory fallback: a client that omits the
/// capability entirely, or omits `"utf-8"` from the offered list, gets
/// UTF-16. UTF-8 is negotiated only when the client explicitly lists it.
pub fn negotiate(client_encodings: Option<&[String]>) -> PositionEncoding {
    let offers_utf8 = client_encodings.is_some_and(|encs| encs.iter().any(|e| e == "utf-8"));
    if offers_utf8 {
        PositionEncoding::Utf8
    } else {
        PositionEncoding::Utf16
    }
}

/// Lazy per-line byte<->UTF-16 conversion table for one file's text.
///
/// Owns its source text (`Arc<str>`, a cheap refcount-bump clone whenever
/// the caller already holds one — `ParsedFileEntry::text` does) rather than
/// borrowing it, so a `LineTable` is
/// lifetime-free and can be memoized behind a `OnceLock` on the very struct
/// that owns the text it was built from (`ParsedFileEntry::line_table`,
/// `src/lsp/snapshot.rs`) — a borrowed `LineTable<'t>` could never be
/// stored that way (self-referential). Line boundaries (byte ranges,
/// `\r`-stripped — see `new`'s doc) are computed once at construction;
/// the actual byte<->UTF-16 unit walk still happens on demand inside
/// `col_out`/`col_in`. AL source lines are short, so a per-call
/// `char_indices()` walk over one line is plenty fast — memoize nothing
/// fancier unless profiling ever says otherwise.
pub struct LineTable {
    text: Arc<str>,
    lines: Vec<Range<usize>>,
}

impl LineTable {
    /// Accepts anything cheaply convertible to `Arc<str>` — an `Arc<str>`
    /// the caller already holds (`Arc::clone`, no allocation, the common
    /// production path) or a borrowed `&str`/`String` (`Arc<str>: From<&str>`
    /// allocates a fresh copy — only ever hit by this module's own literal-
    /// constant unit tests below).
    pub fn new(text: impl Into<Arc<str>>) -> Self {
        let text: Arc<str> = text.into();
        let mut lines = Vec::new();
        let mut start = 0usize;
        for part in text.split('\n') {
            let raw_end = start + part.len();
            // Mirrors the old `part.strip_suffix('\r').unwrap_or(part)`
            // exactly: `\r` is always exactly 1 UTF-8 byte, so trimming it
            // off the END of this line's byte range is byte-for-byte
            // equivalent to the old borrowed-slice version.
            let end = if part.ends_with('\r') {
                raw_end - 1
            } else {
                raw_end
            };
            lines.push(start..end);
            start = raw_end + 1; // +1 skips the consumed '\n' delimiter.
        }
        LineTable { text, lines }
    }

    /// Out-of-range `line` (or a file with no trailing newline) resolves to
    /// an empty line rather than panicking — fail-closed clamp.
    fn line_text(&self, line: u32) -> &str {
        self.lines
            .get(line as usize)
            .map(|r| &self.text[r.clone()])
            .unwrap_or("")
    }

    /// UTF-8 byte column (engine-native) -> column in `enc` for LSP output.
    /// Out-of-range `byte_col` clamps to the line's end (fail-closed, never panics).
    pub fn col_out(&self, line: u32, byte_col: u32, enc: PositionEncoding) -> u32 {
        let text = self.line_text(line);
        if enc == PositionEncoding::Utf8 {
            return byte_col.min(text.len() as u32);
        }
        let byte_col = byte_col as usize;
        let mut consumed_bytes = 0usize;
        let mut units = 0u32;
        for c in text.chars() {
            if consumed_bytes >= byte_col {
                break;
            }
            consumed_bytes += c.len_utf8();
            units += c.len_utf16() as u32;
        }
        units
    }

    /// Inbound LSP column in `enc` -> UTF-8 byte column for engine lookups.
    /// Out-of-range `enc_col` clamps to the line's end (fail-closed, never panics).
    pub fn col_in(&self, line: u32, enc_col: u32, enc: PositionEncoding) -> u32 {
        let text = self.line_text(line);
        if enc == PositionEncoding::Utf8 {
            let byte_col = (enc_col as usize).min(text.len());
            // A malformed utf-8-negotiating client can send a byte offset
            // that lands mid-character (not a valid `str` char boundary);
            // every downstream consumer eventually slices the text at this
            // index, which panics on a non-boundary index. Round up to the
            // next valid boundary — i.e. never split a character, consume
            // it whole — mirroring the UTF-16 arm below, which (by
            // construction, since it only ever adds a whole `char`'s worth
            // of units at a time) rounds a mid-character target up to the
            // position right after that character.
            let mut b = byte_col;
            while b < text.len() && !text.is_char_boundary(b) {
                b += 1;
            }
            return b as u32;
        }
        let mut units_seen = 0u32;
        let mut bytes = 0u32;
        for c in text.chars() {
            if units_seen >= enc_col {
                break;
            }
            units_seen += c.len_utf16() as u32;
            bytes += c.len_utf8() as u32;
        }
        bytes
    }
}

/// Outbound column conversion, the one operation every position-bearing LSP
/// answer needs: a workspace file's [`LineTable`] or a dependency file's
/// [`LineIndex`].
pub trait ColOut {
    /// UTF-8 byte column -> column in `enc` (see [`LineTable::col_out`]).
    fn col_out(&self, line: u32, byte_col: u32, enc: PositionEncoding) -> u32;
}

impl ColOut for LineTable {
    fn col_out(&self, line: u32, byte_col: u32, enc: PositionEncoding) -> u32 {
        LineTable::col_out(self, line, byte_col, enc)
    }
}

impl ColOut for LineIndex {
    fn col_out(&self, line: u32, byte_col: u32, enc: PositionEncoding) -> u32 {
        LineIndex::col_out(self, line, byte_col, enc)
    }
}

/// A [`LineTable`] without the text: answers [`LineTable::col_out`] exactly,
/// in both encodings and with the same clamps, from each line's byte length
/// (`\r` stripped) plus the position and widths of every non-ASCII character.
/// The LSP keeps one per DEPENDENCY file instead of the file's text (engine-
/// switch S10.1): dependency positions are only ever converted outward (a
/// declaration's `origin` / `name_origin`), never inward, so `col_in` is not
/// offered. AL source is nearly all ASCII, so this is mostly 4 bytes per line
/// (11.69 MiB for a 7-root CG corpus's 9,919 dependency files, against 109.45
/// MiB of source; `tools/census-probe/runs-s10-1/`).
#[derive(Debug, PartialEq, Eq)]
pub struct LineIndex {
    line_lens: Vec<u32>,
    /// Every non-ASCII character, in (line, start) order.
    wide: Vec<WideChar>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WideChar {
    line: u32,
    /// Byte offset within the line.
    start: u32,
    len8: u8,
    len16: u8,
}

impl LineIndex {
    /// Line boundaries as [`LineTable::new`] draws them.
    pub fn new(text: &str) -> Self {
        let mut line_lens = Vec::new();
        let mut wide = Vec::new();
        for (line, part) in text.split('\n').enumerate() {
            let part = part.strip_suffix('\r').unwrap_or(part);
            line_lens.push(part.len() as u32);
            if !part.is_ascii() {
                wide.extend(part.char_indices().filter(|(_, c)| !c.is_ascii()).map(
                    |(start, c)| WideChar {
                        line: line as u32,
                        start: start as u32,
                        len8: c.len_utf8() as u8,
                        len16: c.len_utf16() as u8,
                    },
                ));
            }
        }
        line_lens.shrink_to_fit();
        wide.shrink_to_fit();
        LineIndex { line_lens, wide }
    }

    /// [`LineTable::col_out`]: UTF-8 byte column -> column in `enc`. Clamps to
    /// the line end; an out-of-range line is an empty line; a `byte_col` inside
    /// a multi-byte character counts that character whole.
    pub fn col_out(&self, line: u32, byte_col: u32, enc: PositionEncoding) -> u32 {
        let len = self.line_lens.get(line as usize).copied().unwrap_or(0);
        let b = byte_col.min(len);
        if enc == PositionEncoding::Utf8 {
            return b;
        }
        let first = self.wide.partition_point(|w| w.line < line);
        self.wide[first..]
            .iter()
            .take_while(|w| w.line == line && w.start < b)
            .fold(b, |units, w| {
                units - u32::from(w.len8).min(b - w.start) + u32::from(w.len16)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `LineIndex::col_out` equals `LineTable::col_out` at every byte column
    /// (in range, mid-character, past the end) of every line (and past the
    /// last), both encodings: CRLF and LF lines, an empty line, a last line
    /// with no newline, 2-, 3- and 4-byte characters (a surrogate pair), a line
    /// ending in a wide character, and pure ASCII.
    #[test]
    fn line_index_answers_col_out_exactly_as_line_table() {
        let text = "æøå x\r\n🚀 y\n\nplain\r\nx🚀\n€a€b€\r\nAL \"Kunde Nr.\" æ\nno newline 😀";
        let (table, index) = (LineTable::new(text), LineIndex::new(text));
        let lines = text.split('\n').count() as u32;
        for line in 0..=lines + 1 {
            for byte_col in 0..=40 {
                for enc in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                    assert_eq!(
                        index.col_out(line, byte_col, enc),
                        table.col_out(line, byte_col, enc),
                        "line {line} byte_col {byte_col} {enc:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn utf16_conversion_danish_and_emoji() {
        let t = LineTable::new("æøå x\n🚀 y\nplain\n");
        assert_eq!(t.col_out(0, 6, PositionEncoding::Utf16), 3); // after æøå = 6 bytes = 3 UTF-16 units
        assert_eq!(t.col_in(0, 3, PositionEncoding::Utf16), 6);
        assert_eq!(t.col_out(1, 4, PositionEncoding::Utf16), 2); // after 🚀 = 4 bytes = 2 units (surrogate pair)
        assert_eq!(t.col_out(2, 3, PositionEncoding::Utf8), 3); // utf-8 mode: identity
        assert_eq!(t.col_out(0, 999, PositionEncoding::Utf16), 5); // clamp to line end (5 chars -> 5 units)
    }

    #[test]
    fn ascii_passthrough_is_identity_in_both_encodings() {
        let t = LineTable::new("hello world\n");
        assert_eq!(t.col_out(0, 5, PositionEncoding::Utf16), 5);
        assert_eq!(t.col_in(0, 5, PositionEncoding::Utf16), 5);
        assert_eq!(t.col_out(0, 5, PositionEncoding::Utf8), 5);
        assert_eq!(t.col_in(0, 5, PositionEncoding::Utf8), 5);
    }

    #[test]
    fn out_of_range_column_clamps_to_line_end_never_panics() {
        let t = LineTable::new("abc\n");
        assert_eq!(t.col_out(0, 9999, PositionEncoding::Utf8), 3);
        assert_eq!(t.col_in(0, 9999, PositionEncoding::Utf16), 3);
        assert_eq!(t.col_in(0, 9999, PositionEncoding::Utf8), 3);
    }

    #[test]
    fn out_of_range_line_index_clamps_to_empty_line_never_panics() {
        let t = LineTable::new("abc\n");
        assert_eq!(t.col_out(50, 0, PositionEncoding::Utf16), 0);
        assert_eq!(t.col_in(50, 0, PositionEncoding::Utf16), 0);
    }

    #[test]
    fn utf8_col_in_rounds_up_mid_multibyte_char_to_next_boundary() {
        // "æøå x": æ occupies bytes 0..2, ø occupies bytes 2..4 — offsets 1
        // and 3 both land mid-character and must round up to the next
        // valid char boundary rather than passing an unsliceable index
        // through to the caller.
        let t = LineTable::new("æøå x\n");
        assert_eq!(t.col_in(0, 1, PositionEncoding::Utf8), 2);
        assert_eq!(t.col_in(0, 3, PositionEncoding::Utf8), 4);
    }

    #[test]
    fn utf8_col_in_rounds_up_mid_emoji_to_next_boundary() {
        // 🚀 is a 4-byte UTF-8 sequence occupying bytes 0..4 — every offset
        // strictly inside it (1, 2, 3) must round up past the whole
        // character (never split it); an offset already on a boundary (4)
        // is untouched (identity).
        let t = LineTable::new("🚀 y\n");
        assert_eq!(t.col_in(0, 1, PositionEncoding::Utf8), 4);
        assert_eq!(t.col_in(0, 2, PositionEncoding::Utf8), 4);
        assert_eq!(t.col_in(0, 3, PositionEncoding::Utf8), 4);
        assert_eq!(t.col_in(0, 4, PositionEncoding::Utf8), 4);
    }

    #[test]
    fn utf8_col_in_line_end_clamp_and_boundary_rounding_agree() {
        // A line ENDING in a multi-byte char: 'x' (1 byte) + 🚀 (4 bytes) =
        // 5 bytes total. An out-of-range offset clamps to text.len() (5,
        // already a valid boundary); an in-range offset landing
        // mid-character near the end rounds up to exactly that same line
        // length, never past it.
        let t = LineTable::new("x🚀\n");
        assert_eq!(t.col_in(0, 9999, PositionEncoding::Utf8), 5);
        assert_eq!(t.col_in(0, 3, PositionEncoding::Utf8), 5);
    }

    #[test]
    fn crlf_line_endings_match_lf_and_strip_the_carriage_return() {
        let crlf = LineTable::new("æøå x\r\n🚀 y\r\nplain\r\n");
        let lf = LineTable::new("æøå x\n🚀 y\nplain\n");
        // Same character math as the LF-only fixture for the shared cases.
        assert_eq!(
            crlf.col_out(0, 6, PositionEncoding::Utf16),
            lf.col_out(0, 6, PositionEncoding::Utf16)
        );
        assert_eq!(
            crlf.col_in(0, 3, PositionEncoding::Utf16),
            lf.col_in(0, 3, PositionEncoding::Utf16)
        );
        assert_eq!(
            crlf.col_out(1, 4, PositionEncoding::Utf16),
            lf.col_out(1, 4, PositionEncoding::Utf16)
        );
        assert_eq!(
            crlf.col_out(0, 999, PositionEncoding::Utf16),
            lf.col_out(0, 999, PositionEncoding::Utf16)
        );
        // "æøå x" is 8 UTF-8 bytes; the stripped `\r` (byte 8 in the raw,
        // unstripped line) must not count toward the line's length — a
        // byte_col at or past it clamps to 8, not 9.
        assert_eq!(crlf.col_out(0, 8, PositionEncoding::Utf8), 8);
        assert_eq!(crlf.col_out(0, 9, PositionEncoding::Utf8), 8);
    }

    #[test]
    fn negotiate_returns_utf8_only_when_client_offers_it() {
        assert_eq!(negotiate(None), PositionEncoding::Utf16);
        assert_eq!(negotiate(Some(&[])), PositionEncoding::Utf16);
        assert_eq!(
            negotiate(Some(&["utf-16".to_string()])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            negotiate(Some(&["utf-32".to_string()])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            negotiate(Some(&["utf-8".to_string()])),
            PositionEncoding::Utf8
        );
        assert_eq!(
            negotiate(Some(&["utf-16".to_string(), "utf-8".to_string()])),
            PositionEncoding::Utf8
        );
    }

    // ── snapshot-scoped cache enabler: LineTable owns its text ─────────────

    #[test]
    fn line_table_owns_its_text_and_outlives_the_original_string() {
        // The OLD `LineTable<'t> { lines: Vec<&'t str> }` borrowed its
        // source — a `LineTable` could never outlive the `&str` it was
        // built from. The refactor that enables the snapshot-scoped cache
        // (`ParsedFileEntry::line_table`, `src/lsp/snapshot.rs`) requires
        // `LineTable` to be lifetime-free (own an `Arc<str>` clone instead)
        // so it can be memoized behind a `OnceLock` on the struct that also
        // owns the text. This test locks that property in: the table is
        // built from a short-lived owned `String`, which is then DROPPED,
        // and the table is still used successfully afterward.
        let table = {
            let owned = String::from("hello\nworld\n");
            LineTable::new(owned.as_str())
            // `owned` drops here.
        };
        assert_eq!(table.col_out(0, 5, PositionEncoding::Utf8), 5);
        assert_eq!(table.col_out(1, 5, PositionEncoding::Utf8), 5);
    }

    #[test]
    fn line_table_new_accepts_an_arc_str_directly_without_reallocating() {
        // The production path (`ParsedFileEntry::line_table`) passes an
        // `Arc::clone` of already-held text — verify that path compiles and
        // behaves identically to the `&str`-literal construction other
        // tests in this module use.
        let arc_text: Arc<str> = Arc::from("abc\ndef\n");
        let table = LineTable::new(Arc::clone(&arc_text));
        assert_eq!(table.col_out(1, 3, PositionEncoding::Utf8), 3);
    }
}
