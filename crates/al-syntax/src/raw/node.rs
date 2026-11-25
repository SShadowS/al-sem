//! `RawNode` — a zero-copy typed lens over a `tree_sitter::Node`.
//!
//! This is the single place tree-sitter's node API is touched. Payload accessors
//! (kind/ranges/positions/id/text/error flags) are public; structural navigation
//! (`field`, `named_children`) is crate-internal so only the raw typed wrappers and
//! the lowerer descend the tree — consumers above `al-syntax` see only the IR.

use crate::raw::generated::{FieldName, RawKind};
use std::sync::LazyLock;
// `tree_sitter::FieldId` (a private alias of this type).
use std::num::NonZeroU16 as FieldId;
use tree_sitter::{Node, TreeCursor};

/// Per grammar symbol id, its name and its [`RawKind`] (ENG-83). Keyed by
/// `Node::kind_id`, the ALIAS-aware id: `Node::kind` returns that id's name,
/// so `kinds[id]` is exactly what `RawKind::try_from_raw(node.kind())` gave,
/// without the per-call C-string conversion and string match. Built once from
/// the loaded grammar (it is not pinned; no id is baked into generated code).
/// An id past the table (tree-sitter's built-in error symbols) takes the old
/// path.
struct KindTable {
    names: Vec<Option<&'static str>>,
    kinds: Vec<Option<RawKind>>,
}

static KIND_TABLE: LazyLock<KindTable> = LazyLock::new(|| {
    let lang = crate::language::static_language();
    let names: Vec<Option<&'static str>> = (0..lang.node_kind_count())
        .map(|id| {
            u16::try_from(id)
                .ok()
                .and_then(|id| lang.node_kind_for_id(id))
        })
        .collect();
    let kinds = names
        .iter()
        .map(|n| n.and_then(RawKind::try_from_raw))
        .collect();
    KindTable { names, kinds }
});

/// Per [`FieldName`] (indexed by `f as usize`), its field id in the loaded
/// grammar, or `None` when the grammar lacks it: `child_by_field_name` then
/// finds nothing, and so does this (ENG-83).
static FIELD_IDS: LazyLock<Vec<Option<FieldId>>> = LazyLock::new(|| {
    let lang = crate::language::static_language();
    FieldName::ALL
        .iter()
        .map(|f| lang.field_id_for_name(f.as_raw()))
        .collect()
});

fn field_id(f: FieldName) -> Option<FieldId> {
    FIELD_IDS[f as usize]
}

/// The named children of a node, in document order, read with one cursor:
/// no `Vec` and no separate `named_child_count` walk (the cursor still
/// allocates its stack, as before) ([`RawNode::named_children`]).
pub(crate) struct NamedChildren<'t> {
    cursor: TreeCursor<'t>,
    /// The cursor is on a child not yet looked at.
    pending: bool,
}

impl<'t> Iterator for NamedChildren<'t> {
    type Item = RawNode<'t>;

    fn next(&mut self) -> Option<RawNode<'t>> {
        while self.pending {
            let n = self.cursor.node();
            self.pending = self.cursor.goto_next_sibling();
            if n.is_named() {
                return Some(RawNode::new(n));
            }
        }
        None
    }
}

/// The leaf that covers each byte offset of a subtree, for offsets asked in
/// nondecreasing order, with one cursor that only moves forward
/// ([`RawNode::leaf_locator`]). Every node is passed at most once, so a whole file's
/// lookups cost about one walk; a fresh lookup from the root per offset walks the
/// root's children from the start each time (quadratic in a wide file, ENG-119 review).
pub(crate) struct LeafLocator<'t> {
    cursor: TreeCursor<'t>,
    /// The end of the node the cursor is on when the last offset fell after its last
    /// child; offsets below it fall there too.
    gap_end: usize,
    /// Descents started (a test's work count).
    #[cfg(test)]
    pub(crate) descents: usize,
}

impl<'t> LeafLocator<'t> {
    /// The childless node with `start <= p < end`, or `None` when no leaf covers `p`
    /// (white space between a node's children, or past the subtree's end). `p` must not
    /// be below an offset asked before.
    pub(crate) fn leaf_at(&mut self, p: usize) -> Option<RawNode<'t>> {
        let c = &mut self.cursor;
        // Forward to the first node, here or after, that ends past `p`.
        while c.node().end_byte() <= p {
            if !c.goto_next_sibling() && !c.goto_parent() {
                return None;
            }
        }
        if c.node().start_byte() > p {
            return None; // `p` lies before this node: in a gap
        }
        if p < self.gap_end {
            return None; // still after the last child of the node the cursor is on
        }
        // Down to the leaf. The cursor stays on a child that starts after `p`, so a
        // later offset never walks this node's children from the start again.
        #[cfg(test)]
        {
            self.descents += 1;
        }
        loop {
            let here = c.node();
            if c.goto_first_child_for_byte(p).is_none() {
                if here.child_count() == 0 {
                    return Some(RawNode::new(here));
                }
                // `p` is after `here`'s last child, and so is every later offset below
                // its end (review r2, sol: each one walked those children again).
                self.gap_end = here.end_byte();
                return None;
            }
            if c.node().start_byte() > p {
                return None;
            }
        }
    }
}

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
        match KIND_TABLE.kinds.get(usize::from(self.node.kind_id())) {
            Some(Some(k)) => *k,
            // Not a named kind of the table (the ERROR symbol past it, or an
            // anonymous token): the string path, as before (`RawKind::Error`
            // for ERROR; a panic for a kind that is not named).
            _ => RawKind::from_raw(self.node.kind()),
        }
    }

    /// The raw grammar kind string, for anchor `syntax_kind` byte-parity.
    pub fn kind_str(self) -> &'static str {
        let id = self.node.kind_id();
        match KIND_TABLE.names.get(usize::from(id)) {
            Some(Some(name)) => name,
            _ => crate::language::static_language()
                .node_kind_for_id(id)
                .expect("a parsed node's kind id is a symbol of its own grammar"),
        }
    }

    /// The grammar symbol id behind [`RawNode::kind_str`] (what an `Origin` stores).
    pub fn kind_id(self) -> u16 {
        self.node.kind_id()
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
        let id = field_id(f)?;
        self.node.child_by_field_id(id.get()).map(RawNode::new)
    }

    /// Whether this is a *named* node (vs an anonymous token like `,`/`;`/a keyword).
    /// Only named nodes have a [`RawKind`] in the generated vocabulary, so callers
    /// that iterate field children which may include anonymous separators must filter
    /// on this before reading [`RawNode::kind`] (which panics on an unknown kind).
    pub(crate) fn is_named(self) -> bool {
        self.node.is_named()
    }

    /// A [`LeafLocator`] over this node's subtree.
    pub(crate) fn leaf_locator(self) -> LeafLocator<'t> {
        LeafLocator {
            cursor: self.node.walk(),
            gap_end: 0,
            #[cfg(test)]
            descents: 0,
        }
    }

    /// All children held in `field` (for `multiple: true` fields), document order.
    pub(crate) fn children_by_field(self, f: FieldName) -> Vec<RawNode<'t>> {
        let Some(id) = field_id(f) else {
            return Vec::new();
        };
        let mut cursor = self.node.walk();
        self.node
            .children_by_field_id(id, &mut cursor)
            .map(RawNode::new)
            .collect()
    }

    /// Named children in tree-sitter **document order**. NEVER byte-sorted: nested
    /// `call_expression`s share a start position, so only traversal order is
    /// well-defined, and L2 op/callsite numbering depends on it (spec INV-1).
    // Consumed by the lowerer (0d).
    /// Every named descendant (this node included) whose raw grammar kind is in
    /// `kinds`, in document order, found with one cursor walk of the subtree. A
    /// match is not descended into.
    pub(crate) fn descendants_of_kinds(self, kinds: &[&str]) -> Vec<RawNode<'t>> {
        let mut out = Vec::new();
        let mut cursor = self.node.walk();
        loop {
            let n = cursor.node();
            let matched = n.is_named() && kinds.contains(&RawNode::new(n).kind_str());
            if matched {
                out.push(RawNode::new(n));
            }
            if !matched && cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.node() == self.node {
                    return out;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return out;
                }
            }
        }
    }

    /// The byte ranges `(start, end)` of every named node of the subtree (this one
    /// included) that `pred` accepts, in document order (starts nondecreasing), found
    /// with one cursor walk and no allocation per node. A match IS descended into.
    pub(crate) fn named_ranges_where(
        self,
        mut pred: impl FnMut(RawNode<'t>) -> bool,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut cursor = self.node.walk();
        loop {
            let n = cursor.node();
            if n.is_named() && pred(RawNode::new(n)) {
                out.push((n.start_byte(), n.end_byte()));
            }
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.node() == self.node {
                    return out;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return out;
                }
            }
        }
    }

    pub(crate) fn named_children(self) -> NamedChildren<'t> {
        let mut cursor = self.node.walk();
        let pending = cursor.goto_first_child();
        NamedChildren { cursor, pending }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `.al` file under `dir`, sorted.
    fn al_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("corpus dir")
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                al_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "al") {
                out.push(p);
            }
        }
    }

    fn ids<'t>(nodes: impl Iterator<Item = Node<'t>>) -> Vec<usize> {
        nodes.map(|n| n.id()).collect()
    }

    /// ENG-83: the id tables answer exactly what tree-sitter's string API
    /// answered, on every node of the r0 corpus (584 files) and a broken
    /// source (ERROR and MISSING nodes): `kind` and `kind_str` against
    /// `RawKind::try_from_raw(node.kind())` and `node.kind()`, aliased nodes
    /// included (keyed by the alias-aware `kind_id`); `named_children` against
    /// tree-sitter's; `field` and `children_by_field` against the lookups by
    /// name, for every field a child holds, and on every 8th file for every
    /// `FieldName` (absent ones included).
    ///
    /// DISCRIMINATIONS (2026-11-16), each red then green restored: `kind`
    /// keyed by `grammar_id` (aliases ignored) -> an aliased node reads
    /// `ReportKeyword` where `ObjectTypeKeyword` is due; `named_children`
    /// yielding anonymous children too -> fails on file 0's
    /// `page_declaration`; `field` reading the next field's id -> `BaseObject`
    /// of the same node differs.
    #[test]
    fn id_tables_answer_what_the_string_api_did() {
        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/r0-corpus");
        let mut files = Vec::new();
        al_files(&corpus, &mut files);
        assert!(
            files.len() > 500,
            "precondition: the corpus ({})",
            files.len()
        );
        let mut sources: Vec<String> = files
            .iter()
            .map(|p| std::fs::read_to_string(p).expect("read corpus file"))
            .collect();
        sources.push(
            "codeunit 1 X\n{\n    procedure P(\n    begin\n        if then x := ;\n    end\n"
                .into(),
        );
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&crate::language::language())
            .expect("load AL grammar");
        let (mut nodes, mut aliased, mut errors, mut fields_seen) =
            (0usize, 0usize, 0usize, 0usize);
        for (i, src) in sources.iter().enumerate() {
            let tree = parser.parse(src, None).expect("parse");
            let all_fields = i % 8 == 0 || i + 1 == sources.len();
            let mut cursor = tree.walk();
            let mut stack = vec![tree.root_node()];
            while let Some(n) = stack.pop() {
                nodes += 1;
                let r = RawNode::new(n);
                aliased += usize::from(n.grammar_id() != n.kind_id());
                // A true ERROR node: its id (65535) is past the table, so
                // `kind_str` takes the fallback.
                errors += usize::from(n.is_error());
                assert_eq!(r.kind_str(), n.kind(), "kind_str, file {i}");
                if let Some(want) = RawKind::try_from_raw(n.kind()) {
                    assert_eq!(r.kind(), want, "kind of {:?}, file {i}", n.kind());
                }
                assert_eq!(
                    ids(r.named_children().map(|c| c.node)),
                    ids(n.named_children(&mut cursor)),
                    "named_children of {:?}, file {i}",
                    n.kind()
                );
                let mut check = |f: FieldName| {
                    assert_eq!(
                        r.field(f).map(|c| c.id()),
                        n.child_by_field_name(f.as_raw()).map(|c| c.id()),
                        "field {f:?} of {:?}, file {i}",
                        n.kind()
                    );
                    assert_eq!(
                        ids(r.children_by_field(f).into_iter().map(|c| c.node)),
                        ids(n.children_by_field_name(f.as_raw(), &mut cursor)),
                        "children_by_field {f:?} of {:?}, file {i}",
                        n.kind()
                    );
                };
                if all_fields {
                    FieldName::ALL.iter().for_each(|&f| check(f));
                } else {
                    for k in 0..n.child_count() {
                        let Some(name) = n.field_name_for_child(k as u32) else {
                            continue;
                        };
                        let f = FieldName::ALL
                            .iter()
                            .copied()
                            .find(|f| f.as_raw() == name)
                            .expect("every grammar field has a FieldName");
                        fields_seen += 1;
                        check(f);
                    }
                }
                stack.extend(n.children(&mut cursor));
            }
        }
        eprintln!("nodes {nodes}, aliased {aliased}, error {errors}, fields {fields_seen}");
        assert!(nodes > 50_000, "precondition: nodes walked ({nodes})");
        assert!(aliased > 0, "precondition: aliased nodes ({aliased})");
        assert!(errors > 0, "precondition: ERROR nodes ({errors})");
        assert!(
            fields_seen > 10_000,
            "precondition: fields held ({fields_seen})"
        );
    }

    /// `FieldName::ALL[f as usize] == f`: the field-id table is indexed by it.
    #[test]
    fn field_name_all_is_in_declaration_order() {
        for (i, f) in FieldName::ALL.iter().enumerate() {
            assert_eq!(*f as usize, i, "{f:?}");
        }
    }
}
