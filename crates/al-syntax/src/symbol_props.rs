//! Semantic CST-backed lookups that intentionally do NOT live in the IR.
//!
//! Currently: per-`field` / per-`action` property extraction for two niche LSP
//! requests (`field_properties` / `action_properties`). The IR models a table
//! field's number/name/type/class and its caption/tooltip text, and a page
//! action's caption/tooltip/`RunObject`/triggers (`ObjectDecl::actions`), but not
//! every property of either — so rather than bloat the always-parsed IR for a
//! rarely-used request, these lookups walk the raw CST HERE and return OWNED data.
//! Which nodes count as actions is one shared test
//! (`crate::lower::is_action_declaration`), so both paths agree.
//!
//! This preserves the Phase-4 invariant: `al-syntax` is the only crate that links
//! tree-sitter. No `tree_sitter::*` type (or raw grammar kind) ever crosses this
//! boundary — callers see only [`SymbolProperties`].

use crate::raw::{FieldName, RawKind, RawNode};

/// Which declaration kind to look up.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SymbolDeclKind {
    /// A table `field(<id>; <Name>; <Type>) { ... }`.
    Field,
    /// A page `action(<Name>) { ... }`.
    Action,
}

/// A declaration's properties (plus the field id, for table fields).
#[derive(Clone, Debug, Default)]
pub struct SymbolProperties {
    /// The numeric field id (table fields only); `None` for actions or on parse failure.
    pub field_id: Option<u32>,
    /// `property` name=value pairs in declaration order.
    pub properties: Vec<SymbolProperty>,
}

/// A single `PropertyName = value` entry.
#[derive(Clone, Debug)]
pub struct SymbolProperty {
    pub name: String,
    pub value: String,
}

/// Find a `field`/`action` declaration by name (outer-quote-stripped,
/// case-insensitive) and return its properties. `None` when no such declaration
/// exists in `source`.
pub fn lookup_symbol_properties(
    source: &str,
    kind: SymbolDeclKind,
    target_name: &str,
) -> Option<SymbolProperties> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&crate::language::language())
        .expect("load AL grammar");
    let tree = parser.parse(source, None)?;
    // An action is what the lowerer's `ObjectDecl::actions` holds (one shared
    // test), so this lookup and the IR agree on which declarations are actions.
    let want: fn(RawKind) -> bool = match kind {
        SymbolDeclKind::Field => |k| k == RawKind::FieldDeclaration,
        SymbolDeclKind::Action => crate::lower::is_action_declaration,
    };
    let target = clean_name(target_name);
    let root = RawNode::new(tree.root_node());
    let decl = find_named_decl(root, want, &target, source)?;
    Some(extract_properties(
        decl,
        source,
        kind == SymbolDeclKind::Field,
    ))
}

/// Outer-quote-strip + lowercase, matching the legacy handler's name comparison.
fn clean_name(s: &str) -> String {
    s.trim().trim_matches('"').to_lowercase()
}

/// Recursively locate the first declaration of a `want` kind whose `name` field
/// (cleaned) equals `target`.
fn find_named_decl<'t>(
    node: RawNode<'t>,
    want: fn(RawKind) -> bool,
    target: &str,
    source: &str,
) -> Option<RawNode<'t>> {
    if want(node.kind())
        && let Some(name_node) = node.field(FieldName::Name)
        && clean_name(name_node.text(source)) == target
    {
        return Some(node);
    }
    for child in node.named_children() {
        if let Some(found) = find_named_decl(child, want, target, source) {
            return Some(found);
        }
    }
    None
}

/// Pull `field_id` (when requested) + every `property` name=value out of a
/// declaration. tree-sitter-al v3 wraps a declaration's properties in a `body`
/// (`declaration_body`); descend it when present, else iterate the declaration.
fn extract_properties(decl: RawNode, source: &str, extract_field_id: bool) -> SymbolProperties {
    let mut result = SymbolProperties::default();

    if extract_field_id
        && let Some(id_node) = decl.field(FieldName::Id)
        && let Ok(id) = id_node.text(source).trim().parse::<u32>()
    {
        result.field_id = Some(id);
    }

    let container = decl.field(FieldName::Body).unwrap_or(decl);
    for child in container.named_children() {
        if child.kind() != RawKind::Property {
            continue;
        }
        let Some(name_node) = child.field(FieldName::Name) else {
            continue;
        };
        let name = name_node.text(source).trim().to_string();
        let value = match child.field(FieldName::Value) {
            Some(v) => v.text(source).trim().to_string(),
            None => value_after_eq(child.text(source)),
        };
        result.properties.push(SymbolProperty { name, value });
    }

    result
}

/// Fallback value extraction (when the `property` node has no `value` field):
/// everything after `=`, one trailing `;` removed — byte-identical to the legacy
/// `extract_property_value` (whole text when there is no `=`).
fn value_after_eq(text: &str) -> String {
    let text = text.trim();
    if let Some(eq) = text.find('=') {
        let value = text[eq + 1..].trim();
        value.strip_suffix(';').unwrap_or(value).trim().to_string()
    } else {
        text.to_string()
    }
}
