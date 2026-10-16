//! Tree-sitter AL grammar FFI binding. The grammar C is compiled into this crate
//! by `build.rs`; this is the single link point for `tree_sitter_al`.

use tree_sitter::Language;

unsafe extern "C" {
    fn tree_sitter_al() -> Language;
}

/// Get the tree-sitter AL language.
///
/// # Safety
/// Calls into the compiled C code from tree-sitter-al.
pub fn language() -> Language {
    unsafe { tree_sitter_al() }
}

/// One process-wide handle on the grammar. Its kind names live in the grammar's
/// static tables; borrowing them through a `'static` `Language` is how a node's
/// kind keeps a `'static` lifetime now that tree-sitter 0.27's `Node::kind` ties
/// it to the tree.
pub(crate) fn static_language() -> &'static Language {
    static LANGUAGE: std::sync::LazyLock<Language> = std::sync::LazyLock::new(language);
    &LANGUAGE
}
