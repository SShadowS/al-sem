//! IR statements, blocks, and the preproc grouping.
//!
//! A [`Block`] is an ordered list of [`BlockItem`]s. Most are `Stmt`; a
//! statement-position `#if`/`#elif`/`#else` chain is a [`PreprocGroup`] holding
//! every branch the routine's build context leaves live (the IR is not a
//! single-configuration projection). The branches are ALTERNATIVES: one build
//! compiles at most one of them (engine limits B2). Collecting walkers descend a
//! group's branches in document order (preserving visit order); a control-flow
//! walk must treat them as alternatives, like a `case`'s arms, with an implicit
//! empty arm when [`PreprocGroup::falls_through`]; block-sibling analyses treat the
//! group as a boundary.

use super::{BlockId, ExprId, Origin, StmtId};

pub struct Block {
    pub items: Vec<BlockItem>,
    pub origin: Origin,
}

pub enum BlockItem {
    Stmt(StmtId),
    Preproc(PreprocGroup),
}

/// A `#if cond ... #elif ... #else ... #endif` chain in statement position. Its
/// branches are alternatives (a build compiles at most one). Only the routine's
/// own build context decides a directive (a branch it decides false is left out);
/// no app's `preprocessorSymbols` are read.
///
/// The lowerer emits a group only when it is a real choice: two or more live
/// branches, or one with [`Self::falls_through`]. A chain whose single live branch
/// every build takes is spliced into the enclosing block as plain statements.
pub struct PreprocGroup {
    /// One `Block` per live branch (`#if`, each `#elif`, `#else`), in source order.
    /// Each block's origin runs from its directive to the next directive.
    pub branches: Vec<BlockId>,
    /// Some build takes none of the branches (no `#else`, and no branch the build
    /// context decides true): the implicit empty branch.
    pub falls_through: bool,
    /// From the `#if` directive to the end of its `#endif`.
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
