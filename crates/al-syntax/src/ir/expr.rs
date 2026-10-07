//! IR expressions. Names/literal text are stored inline so the IR is
//! self-contained (no source slice needed to read it); the engine interns as it
//! consumes. `Unknown` carries no payload — its `Origin` + a `SyntaxIssue` record
//! the failure (never a silent drop).

use super::{ExprId, Origin};

pub struct Expr {
    pub kind: ExprKind,
    pub origin: Origin,
}

pub enum ExprKind {
    Identifier(String),
    QuotedIdentifier(String),
    /// `object.member`. `member_origin` is the member identifier node's provenance
    /// (its name is `member`, raw with quotes) — needed for reference anchors.
    Member {
        object: ExprId,
        member: String,
        member_origin: Origin,
    },
    /// `function(args...)`
    Call {
        function: ExprId,
        args: Vec<ExprId>,
    },
    /// `base[index]` (subscript)
    Index {
        base: ExprId,
        index: ExprId,
    },
    Literal(Literal),
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Binary {
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Parenthesized(ExprId),
    /// `Enum::Value` — `enum_type` is lowered (it can be a `member_expression`
    /// like `Rec.Status::Open`), `value` is the member text.
    QualifiedEnum {
        enum_type: ExprId,
        value: String,
    },
    /// `Database::"Customer"` and similar object references.
    DatabaseReference(String),
    /// `a..b`
    RangeExpr {
        start: ExprId,
        end: ExprId,
    },
    /// `cond ? then_value : else_value`
    Ternary {
        cond: ExprId,
        then_value: ExprId,
        else_value: ExprId,
    },
    /// `value is Type` / `value as Type`; `ty` is the type text as written.
    TypeOp {
        op: TypeOp,
        value: ExprId,
        ty: String,
    },
    /// `[a, b, ...]`, the right side of `in`.
    List(Vec<ExprId>),
    /// A syntactically present expression the lowerer does not yet model. The kind
    /// is preserved via `Origin.kind_text`; a `SyntaxIssue` is recorded.
    Unknown,
}

pub enum Literal {
    Int(String),
    Decimal(String),
    Bool(bool),
    /// String / verbatim string content (raw text, quotes included).
    Text(String),
    Date(String),
    DateTime(String),
    Time(String),
    /// Any other literal kind, raw text preserved.
    Other(String),
}

impl ExprKind {
    /// The direct sub-expressions, in source order.
    #[must_use]
    pub fn children(&self) -> Vec<ExprId> {
        match self {
            ExprKind::Member { object, .. } => vec![*object],
            ExprKind::Call { function, args } => std::iter::once(*function)
                .chain(args.iter().copied())
                .collect(),
            ExprKind::Index { base, index } => vec![*base, *index],
            ExprKind::Unary { operand, .. } => vec![*operand],
            ExprKind::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
            ExprKind::Parenthesized(x) => vec![*x],
            ExprKind::QualifiedEnum { enum_type, .. } => vec![*enum_type],
            ExprKind::RangeExpr { start, end } => vec![*start, *end],
            ExprKind::Ternary {
                cond,
                then_value,
                else_value,
            } => vec![*cond, *then_value, *else_value],
            ExprKind::TypeOp { value, .. } => vec![*value],
            ExprKind::List(items) => items.clone(),
            ExprKind::Identifier(_)
            | ExprKind::QuotedIdentifier(_)
            | ExprKind::Literal(_)
            | ExprKind::DatabaseReference(_)
            | ExprKind::Unknown => Vec::new(),
        }
    }
}

/// `is` / `as`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TypeOp {
    Is,
    As,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum UnaryOp {
    Not,
    Neg,
    Plus,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    /// integer division `DIV`
    IntDiv,
    /// modulo `MOD`
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Xor,
    /// `IN` membership
    In,
    /// anything not in the set above, operator text preserved on the node origin.
    Other,
}
