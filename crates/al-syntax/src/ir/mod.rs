//! The owned AL syntax IR — the public surface `al-syntax` exposes to the engine.
//!
//! Produced once by the lowerer from the tree-sitter CST; the engine consumes only
//! this (never `tree_sitter::Node`, raw kinds, or queries). It models AL at the
//! syntactic level the analyzer reasons about — NOT a resolver HIR (that stays in
//! the engine). Every node carries an [`Origin`] back to source for byte-identical
//! anchors and traceable findings.
//!
//! Storage is a push-only arena ([`Ir`]) of `Expr`/`Stmt`/`Block`; nodes reference
//! each other by `Copy` newtype ids. No node is ever removed and ids are never
//! reused, so an id is stable for the life of one `AlFile`.

mod decl;
mod expr;
mod stmt;

pub use decl::{
    AttributeIr, FieldDecl, ObjectDecl, ObjectKind, ObjectProperty, PageControl, Param,
    RoutineDecl, RoutineKind, VarDecl,
};
pub use expr::{BinaryOp, Expr, ExprKind, Literal, TypeOp, UnaryOp};
pub use stmt::{Block, BlockItem, CaseBranch, PreprocGroup, Stmt, StmtKind};

use std::ops::Range;

/// A source position. Our own type — `tree_sitter::Point` never crosses the IR
/// boundary. `column` is a UTF-8 byte column within the line (matches the engine's
/// existing anchor column semantics).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

/// Provenance of an IR node: where it came from in source, for anchors + findings.
///
/// `PartialEq`/`Eq` are structural over every field, so a test that round-trips an
/// `Origin` through a wire format can assert equality TOTALLY rather than
/// field-by-field — a hand-listed comparison silently stops covering any field
/// added later (`src/program/pack`'s codec tests rely on this).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The raw grammar kind string, fed verbatim to anchor `syntax_kind` (parity).
    pub kind_text: &'static str,
    pub byte: Range<usize>,
    pub start: Point,
    pub end: Point,
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
        pub struct $name(u32);
        impl $name {
            #[inline]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}
id_type!(ExprId);
id_type!(StmtId);
id_type!(BlockId);

/// Push-only arena holding the `Expr`/`Stmt`/`Block` pools an `AlFile` references.
#[derive(Default)]
pub struct Ir {
    exprs: Vec<Expr>,
    stmts: Vec<Stmt>,
    blocks: Vec<Block>,
}

impl Ir {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_expr(&mut self, e: Expr) -> ExprId {
        let id = ExprId(u32::try_from(self.exprs.len()).expect("expr arena overflow"));
        self.exprs.push(e);
        id
    }
    pub fn add_stmt(&mut self, s: Stmt) -> StmtId {
        let id = StmtId(u32::try_from(self.stmts.len()).expect("stmt arena overflow"));
        self.stmts.push(s);
        id
    }
    pub fn add_block(&mut self, b: Block) -> BlockId {
        let id = BlockId(u32::try_from(self.blocks.len()).expect("block arena overflow"));
        self.blocks.push(b);
        id
    }

    #[inline]
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.index()]
    }
    #[inline]
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id.index()]
    }
    #[inline]
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.index()]
    }

    pub fn iter_exprs(&self) -> impl Iterator<Item = &Expr> {
        self.exprs.iter()
    }

    pub fn iter_stmts(&self) -> impl Iterator<Item = &Stmt> {
        self.stmts.iter()
    }

    pub fn expr_count(&self) -> usize {
        self.exprs.len()
    }
    pub fn stmt_count(&self) -> usize {
        self.stmts.len()
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

/// Whether the parse was clean or hit tree-sitter error recovery.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ParseStatus {
    /// No `ERROR`/`MISSING` nodes anywhere.
    Clean,
    /// Recovery nodes present — the IR is partial; do not cache as authoritative.
    Recovered,
}

/// A syntactic problem the lowerer recorded (recovery node, or an `Unknown` it
/// could not classify). Never silently dropped — surfaced as data.
#[derive(Clone, Debug)]
pub struct SyntaxIssue {
    pub message: String,
    pub origin: Origin,
}

/// The lowered representation of one AL source file.
pub struct AlFile {
    pub objects: Vec<ObjectDecl>,
    pub ir: Ir,
    pub issues: Vec<SyntaxIssue>,
    pub parse_status: ParseStatus,
}
