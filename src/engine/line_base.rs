//! The line base of the engine-query surface (MCP output plan
//! `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`, item 1a; owner
//! decision 1).
//!
//! Model anchors hold tree-sitter rows: 0-based. Until 2026-11-04 `explain`,
//! `prove`, `digest`, `query`, `fingerprint` and the snapshots printed and
//! serialized those rows as they were, while `alsem graph`, `alsem analyze`,
//! SARIF, the HTML report, every editor, `grep -n` and an agent's `Read` count
//! from 1. An agent that opened `file:18` landed one statement early. Now every
//! line those commands print or serialize is 1-based, and each JSON document
//! says so with `lineBase: 1`.
//!
//! The rule: the model stays 0-based, and the conversion happens once, where a
//! line leaves the engine (a JSON value, a CBOR value or a human `file:line`).
//! Nothing sorts or hashes on a converted line. Columns are unchanged (0-based).

/// The base of every line the engine-query surface prints or serializes.
pub const LINE_BASE: u32 = 1;

/// A model anchor's 0-based line as the engine-query surface shows it.
#[must_use]
pub const fn shown(zero_based: u32) -> u32 {
    zero_based + LINE_BASE
}

/// [`shown`] over an optional line.
#[must_use]
pub fn shown_opt(zero_based: Option<u32>) -> Option<u32> {
    zero_based.map(shown)
}

/// A serde `serialize_with` for a 0-based `u32` line field: writes [`shown`].
pub fn serialize_shown<S: serde::Serializer>(line: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u32(shown(*line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_shown_one_higher() {
        assert_eq!(shown(0), 1);
        assert_eq!(shown(17), 18);
        assert_eq!(shown_opt(Some(4)), Some(5));
        assert_eq!(shown_opt(None), None);
    }
}
