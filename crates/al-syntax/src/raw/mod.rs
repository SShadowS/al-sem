//! Raw grammar layer. Generated vocabulary (`RawKind`/`FieldName`) now; typed CST
//! wrappers + shape facts to follow (Phase 0). During the migration this is `pub`
//! so the dual-run harness can compare `Origin`s; it is NOT a tree-sitter type, and
//! it is tightened to crate-internal at the Phase 5 seal.

pub mod generated;
pub mod node;

pub use generated::{FieldName, GRAMMAR_NODE_TYPES_HASH, NAMED_KIND_COUNT, RawKind};
pub use node::RawNode;

#[cfg(test)]
mod tests {
    use super::{FieldName, GRAMMAR_NODE_TYPES_HASH, NAMED_KIND_COUNT, RawKind};

    #[test]
    fn raw_kind_round_trips() {
        assert_eq!(RawKind::from_raw("procedure"), RawKind::Procedure);
        assert_eq!(RawKind::from_raw("code_block"), RawKind::CodeBlock);
        assert_eq!(
            RawKind::from_raw("statement_block"),
            RawKind::StatementBlock
        );
        assert_eq!(
            RawKind::from_raw("declaration_body"),
            RawKind::DeclarationBody
        );
        assert_eq!(RawKind::Procedure.as_str(), "procedure");
        assert_eq!(RawKind::from_raw("ERROR"), RawKind::Error);
        // Update when the grammar adds/removes a NAMED node kind (the generated
        // `NAMED_KIND_COUNT` const is authoritative; this pins it as a sanity anchor).
        assert_eq!(NAMED_KIND_COUNT, 467);
        assert_eq!(GRAMMAR_NODE_TYPES_HASH.len(), 64);
    }

    #[test]
    #[should_panic(expected = "unknown node kind")]
    fn unknown_kind_panics() {
        let _ = RawKind::from_raw("definitely_not_a_real_kind");
    }

    /// `try_from_raw` is the TOTAL sibling: the exact inputs that make
    /// `from_raw` panic must yield `None` here, never a panic and never a
    /// wrong kind. A persistence layer calls this on strings it did not
    /// choose (`RawNode::kind_str` returns the raw kind of ANY node, named or
    /// anonymous), so "does not abort the process" is the contract.
    #[test]
    fn try_from_raw_is_none_where_from_raw_panics() {
        assert_eq!(RawKind::try_from_raw("definitely_not_a_real_kind"), None);
        // An ANONYMOUS token kind — a real grammar string, but not a named
        // kind, so it has no variant. This is the case that motivated the
        // function: it is reachable from live data, not just from a bug.
        assert_eq!(RawKind::try_from_raw(";"), None);
        assert_eq!(RawKind::try_from_raw(""), None);
        // And it agrees with `from_raw` on every string that IS a kind.
        assert_eq!(RawKind::try_from_raw("procedure"), Some(RawKind::Procedure));
        assert_eq!(RawKind::try_from_raw("ERROR"), Some(RawKind::Error));
    }

    /// `ALL` is exhaustive, positional and consistent with `as_str`/
    /// `try_from_raw`. The positional half (`ALL[k as usize] == k`) is what
    /// lets a caller encode a kind as a varint index and decode it by lookup;
    /// nothing else in the language guarantees the generated array order
    /// matches the generated variant order, so it is asserted here.
    #[test]
    fn all_is_positional_and_round_trips_every_kind() {
        // NOTE: `ALL.len() == NAMED_KIND_COUNT + 1` is deliberately NOT
        // asserted — `ALL`'s declared type IS `[RawKind; NAMED_KIND_COUNT + 1]`,
        // so a wrong count is a compile error and the assertion could never
        // fail. The two assertions below are the ones that can.
        for (i, k) in RawKind::ALL.iter().enumerate() {
            assert_eq!(*k as usize, i, "ALL[{i}] = {k:?} is not at its own index");
            assert_eq!(
                RawKind::try_from_raw(k.as_str()),
                Some(*k),
                "{k:?} does not round-trip through as_str/try_from_raw"
            );
        }
        // No duplicate variants, so the index mapping is a bijection.
        let distinct: std::collections::BTreeSet<&str> =
            RawKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(distinct.len(), RawKind::ALL.len());
    }

    #[test]
    fn field_round_trips() {
        assert_eq!(FieldName::Name.as_raw(), "name");
        assert_eq!(FieldName::Body.as_raw(), "body");
        assert_eq!(FieldName::Member.as_raw(), "member");
    }
}
