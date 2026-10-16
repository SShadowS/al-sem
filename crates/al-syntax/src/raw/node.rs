//! `RawNode` — a zero-copy typed lens over a `tree_sitter::Node`.
//!
//! This is the single place tree-sitter's node API is touched. Payload accessors
//! (kind/ranges/positions/id/text/error flags) are public; structural navigation
//! (`field`, `named_children`) is crate-internal so only the raw typed wrappers and
//! the lowerer descend the tree — consumers above `al-syntax` see only the IR.

use crate::raw::generated::{FieldName, RawKind};
use tree_sitter::Node;

#[derive(Copy, Clone)]
pub struct RawNode<'t> {
    node: Node<'t>,
}

impl<'t> RawNode<'t> {
    pub(crate) fn new(node: Node<'t>) -> Self {
        Self { node }
    }

    // ---- payload (public) ----

    /// Semantic kind. Panics on a kind absent from the pinned grammar (loud — a
    /// grammar/binary mismatch), since only named children are ever classified.
    pub fn kind(self) -> RawKind {
        RawKind::from_raw(self.node.kind())
    }

    /// The raw grammar kind string, for anchor `syntax_kind` byte-parity.
    pub fn kind_str(self) -> &'static str {
        crate::language::static_language()
            .node_kind_for_id(self.node.kind_id())
            .expect("a parsed node's kind id is a symbol of its own grammar")
    }

    /// tree-sitter node id. EPHEMERAL: only valid within one parse; tree-sitter
    /// recycles ids across re-parses. Used to key the L2 op/callsite maps during a
    /// single lowering pass — never persist or compare across parses.
    pub fn id(self) -> usize {
        self.node.id()
    }

    pub fn byte_range(self) -> std::ops::Range<usize> {
        self.node.byte_range()
    }
    pub fn start_position(self) -> tree_sitter::Point {
        self.node.start_position()
    }
    pub fn end_position(self) -> tree_sitter::Point {
        self.node.end_position()
    }
    pub fn is_error(self) -> bool {
        self.node.is_error()
    }
    pub fn is_missing(self) -> bool {
        self.node.is_missing()
    }
    pub fn has_error(self) -> bool {
        self.node.has_error()
    }

    /// Source text of this node.
    pub fn text(self, src: &str) -> &str {
        &src[self.node.byte_range()]
    }

    // ---- structural navigation (crate-internal) ----

    /// The single child held in `field`, if present.
    pub(crate) fn field(self, f: FieldName) -> Option<RawNode<'t>> {
        self.node.child_by_field_name(f.as_raw()).map(RawNode::new)
    }

    /// Whether this is a *named* node (vs an anonymous token like `,`/`;`/a keyword).
    /// Only named nodes have a [`RawKind`] in the generated vocabulary, so callers
    /// that iterate field children which may include anonymous separators must filter
    /// on this before reading [`RawNode::kind`] (which panics on an unknown kind).
    pub(crate) fn is_named(self) -> bool {
        self.node.is_named()
    }

    /// All children held in `field` (for `multiple: true` fields), document order.
    pub(crate) fn children_by_field(self, f: FieldName) -> Vec<RawNode<'t>> {
        let mut cursor = self.node.walk();
        self.node
            .children_by_field_name(f.as_raw(), &mut cursor)
            .map(RawNode::new)
            .collect()
    }

    /// Named children in tree-sitter **document order**. NEVER byte-sorted: nested
    /// `call_expression`s share a start position, so only traversal order is
    /// well-defined, and L2 op/callsite numbering depends on it (spec INV-1).
    // Consumed by the lowerer (0d).
    #[allow(dead_code)]
    pub(crate) fn named_children(self) -> Vec<RawNode<'t>> {
        let mut cursor = self.node.walk();
        self.node
            .named_children(&mut cursor)
            .map(RawNode::new)
            .collect()
    }
}
