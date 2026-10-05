//! IR statements, blocks, and the preproc grouping.
//!
//! A [`Block`] is an ordered list of [`BlockItem`]s. Most are `Stmt`; a
//! `#if`/`#else` region is a [`PreprocGroup`] holding BOTH branches (legacy indexes
//! both for BC version-compat — the IR is not a single-configuration projection).
//! Recursive feature walkers descend a `PreprocGroup`'s branches in document order
//! (preserving visit order, spec INV-1); block-sibling analyses may treat the group
//! as a boundary (matching legacy `block_statements`, which splices only
//! `statement_block`). The flat-vs-structured choice is validated by Phase 1 dual-run.

use super::{BlockId, ExprId, Origin, StmtId};

pub struct Block {
    pub items: Vec<BlockItem>,
    pub origin: Origin,
}

pub enum BlockItem {
    Stmt(StmtId),
    Preproc(PreprocGroup),
}

/// A `#if cond ... #else ... #endif` region. Both branches are lowered and kept;
/// directives are not evaluated.
pub struct PreprocGroup {
    /// One `Block` per branch (`#if`, each `#elif`, `#else`), in source order.
    pub branches: Vec<BlockId>,
    pub origin: Origin,
}

pub struct Stmt {
    pub kind: StmtKind,
    pub origin: Origin,
}

pub enum StmtKind {
    Assignment {
        target: ExprId,
        value: ExprId,
        /// `+=`, `-=`, `*=` or `/=`: the target is READ as well as written (#14).
        /// `false` for a plain `:=`.
        compound: bool,
    },
    /// A call in statement position (`Foo();` / `Rec.SetRange(...);`).
    Call(ExprId),
    If {
        cond: ExprId,
        then_block: BlockId,
        else_block: Option<BlockId>,
    },
    Case {
        scrutinee: ExprId,
        branches: Vec<CaseBranch>,
        else_block: Option<BlockId>,
    },
    While {
        cond: ExprId,
        body: BlockId,
    },
    Repeat {
        body: BlockId,
        until: ExprId,
    },
    For {
        var: ExprId,
        from: ExprId,
        to: ExprId,
        down: bool,
        body: BlockId,
    },
    Foreach {
        var: ExprId,
        iterable: ExprId,
        body: BlockId,
    },
    With {
        receiver: ExprId,
        body: BlockId,
    },
    /// `if guard then ...` recovery aside; normal `try` maps here. Sets has_branching.
    Try {
        body: BlockId,
        catch_block: Option<BlockId>,
    },
    /// `asserterror <stmt/block>` — establishes the under-asserterror context that
    /// flows to descendant call sites (legacy `under_asserterror`).
    AssertError(BlockId),
    Exit(Option<ExprId>),
    Break,
    Continue,
    /// A bare `begin..end` nested block.
    Block(BlockId),
    /// A syntactically present statement the lowerer does not yet model.
    Unknown,
}

pub struct CaseBranch {
    /// The match value(s) for this branch (`1, 2:` → two patterns).
    pub patterns: Vec<ExprId>,
    pub body: BlockId,
    pub origin: Origin,
}
