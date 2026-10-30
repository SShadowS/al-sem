//! Rules: the checks a routine applies, with the condition under which each
//! fires and its message (intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.2).
//!
//! A **check** is a statement that stops the success path when a condition
//! holds: `Error(...)`, `Rec.TestField(...)`, `Rec.FieldError(...)`, an `Error`
//! reached when a `Confirm` returned `false` (`ConfirmError`), and an `exit`
//! reached that way (`ConfirmExit`, a silent return). A **rule** is a check
//! together with its condition and message.
//!
//! Rules are built at model assembly, from the routine's IR (the full `if`
//! conditions exist only there), and only when the view declares
//! `ViewNeeds::rules` (`crate::program::model::workspace::AssemblyNeeds`). The
//! build has two halves:
//!
//! 1. [`build_rule_drafts`] (assembly) walks the IR and gives one
//!    [`RuleDraft`] per check. What is known from the syntax alone is decided
//!    here: the condition parts, the message, and `stops` when the message is an
//!    `ErrorInfo` (or not proven Text) or the check sits under `asserterror`.
//! 2. [`settle_rules`] (after call resolution) decides the one input syntax
//!    cannot: whether a bare `Error(...)` binds to the builtin or to a procedure
//!    named `Error` (#288). It then drops the `not C` parts an earlier check
//!    contributed when that earlier check may not stop, and gives the final
//!    [`PRule`]s.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{
    AlFile, BinaryOp, BlockId, BlockItem, ExprId, ExprKind, Literal, ObjectDecl, ObjectKind,
    Origin, RoutineDecl, StmtId, StmtKind, UnaryOp, VarDecl,
};

use super::features::{PAnchor, PCallSite};
use super::scope::{ScopeTier, scope_tier};

/// What kind of check a rule is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Error,
    TestField,
    FieldError,
    /// An `Error` reached when a `Confirm` returned `false`.
    ConfirmError,
    /// An `exit` reached when a `Confirm` returned `false`: the routine ends
    /// silently, without an error.
    ConfirmExit,
}

/// Whether the check stops the routine when it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stops {
    Always,
    MayNotStop { reason: String },
}

/// The message is an `ErrorInfo`: with a collectible error, execution goes on.
pub const REASON_ERROR_INFO: &str =
    "the message is an ErrorInfo: when the error is collectible, execution goes on";
/// One argument that #288 cannot prove is Text: it may be an `ErrorInfo`.
pub const REASON_UNPROVEN_TEXT: &str =
    "the argument is not proven Text, so it may be an ErrorInfo that does not stop";
/// The bare `Error` does not bind only to the builtin (#288): a procedure named
/// `Error` returns.
pub const REASON_OWN_ERROR: &str =
    "the bare Error may bind to a procedure named Error, which returns";
/// Under `asserterror` the error is caught.
pub const REASON_ASSERTERROR: &str = "inside asserterror, which catches the error";

/// A comparison operator in a [`ConditionAtom::FieldCompare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn of(op: BinaryOp) -> Option<CmpOp> {
        Some(match op {
            BinaryOp::Eq => CmpOp::Eq,
            BinaryOp::Ne => CmpOp::Ne,
            BinaryOp::Lt => CmpOp::Lt,
            BinaryOp::Le => CmpOp::Le,
            BinaryOp::Gt => CmpOp::Gt,
            BinaryOp::Ge => CmpOp::Ge,
            _ => return None,
        })
    }

    /// The operator that holds exactly when `self` does not.
    fn inverse(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Ne,
            CmpOp::Ne => CmpOp::Eq,
            CmpOp::Lt => CmpOp::Ge,
            CmpOp::Ge => CmpOp::Lt,
            CmpOp::Gt => CmpOp::Le,
            CmpOp::Le => CmpOp::Gt,
        }
    }
}

/// One part of a rule's condition (the condition is the AND of its atoms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionAtom {
    /// `R.F <op> value`. `table` is the record variable's table, when known.
    FieldCompare {
        record_var: String,
        table: Option<String>,
        field: String,
        op: CmpOp,
        value: String,
    },
    /// `R.F` is empty (`negated`: is not empty). `R.F = ''` and `R.F = 0` give
    /// this atom, as does `TestField(F)`.
    FieldEmpty {
        record_var: String,
        table: Option<String>,
        field: String,
        negated: bool,
    },
    /// A by-value `Boolean` parameter holds `value`.
    ParamBool { index: u32, value: bool },
    /// A call's result is `true` (`negated`: `false`), e.g. `if not CheckX() then`.
    CallResult { callee_text: String, negated: bool },
    /// A record lookup (`Get`, `Find*`, `IsEmpty`) returned `true`
    /// (`negated`: `false`). `op` is the method as written.
    RecordFound {
        record_var: String,
        op: String,
        negated: bool,
    },
    /// The `else` arm of a `case`: the scrutinee is none of the listed values.
    NoneOf {
        scrutinee: String,
        values: Vec<String>,
    },
    /// A condition the builder does not understand, kept as written so the
    /// condition stays complete.
    Other { text: String, anchor: PAnchor },
}

/// Where a rule's message comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSource {
    /// A string literal at the check.
    Literal,
    /// A `Label` variable, as declared.
    Label { var: String },
    /// A `TextConst` variable, as declared.
    TextConst { var: String },
    /// An `ErrorInfo` value.
    ErrorInfo,
    /// The platform's own message (`TestField`, `FieldError` without text).
    Platform,
    /// Anything whose text the engine cannot prove.
    Dynamic,
}

/// A rule's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMessage {
    pub source: MessageSource,
    /// The resolved text, `%1` placeholders kept. `None` when unknown.
    pub text: Option<String>,
    /// The argument expressions as written, in order (`Error(Lbl, a, b)` and
    /// `Error(StrSubstNo(Lbl, a, b))` both give `[a, b]`).
    pub args: Vec<String>,
    /// The label's `Comment = '...'` (it often says what `%1` is).
    pub label_comment: Option<String>,
    /// For a `Dynamic` message whose name has more than one declaration the
    /// routine can see (a global, or both arms of an `#if`): every candidate
    /// text, in scope order. Empty otherwise.
    pub candidates: Vec<String>,
}

/// One rule of a routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PRule {
    /// `{routine id}/rule{n}`, `n` in walk (source) order.
    pub id: String,
    pub kind: RuleKind,
    pub stops: Stops,
    /// The check statement (for `ConfirmExit`, the `exit`).
    pub anchor: PAnchor,
    /// AND of atoms; empty means "always, on this path".
    pub when: Vec<ConditionAtom>,
    /// The atoms' texts joined with `" and "`: each as written, a negated one as
    /// `not X` / `not (X)`. A built-in check's own condition reads
    /// `R.F is empty` / `R.F <> V`.
    pub when_text: String,
    pub message: Option<RuleMessage>,
    /// The trailing `//` comment on the check's line, else the run of own-line
    /// `//` comments directly above it (or above its `if`).
    pub comment: Option<String>,
}

/// One condition atom with its text, and, when an earlier check contributed it
/// (`not C`), that check's index in the routine's drafts.
#[derive(Debug, Clone)]
pub struct WhenPart {
    text: String,
    atom: ConditionAtom,
    from_rule: Option<usize>,
    /// From an enclosing `if` / `case` (not from an earlier statement).
    enclosing: bool,
    /// The `Confirm(...)` call this atom tests, if any.
    confirm: Option<ExprId>,
}

/// The assembly half of a rule (see the module doc). Build-local: only
/// [`settle_rules`] reads it.
#[derive(Debug, Clone)]
pub struct RuleDraft {
    id: String,
    kind: RuleKind,
    anchor: PAnchor,
    comment: Option<String>,
    message: Option<RuleMessage>,
    /// Decided at assembly; `None` = `Always` unless a `builtin_sites` entry
    /// binds elsewhere than the builtin.
    stops: Option<Stops>,
    /// Call sites that must bind to the builtin alone for the check to stop,
    /// each with the reason it may not stop otherwise: the bare `Error(...)`
    /// itself (#288), and a one-argument `Error(StrSubstNo(..))`'s
    /// `StrSubstNo` (Text only when it is the builtin). Settled after
    /// resolution.
    builtin_sites: Vec<(String, &'static str)>,
    parts: Vec<WhenPart>,
}

impl RuleDraft {
    /// The call-site ids [`settle_rules`] asks about.
    pub fn builtin_sites(&self) -> impl Iterator<Item = &str> {
        self.builtin_sites
            .iter()
            .filter(|_| self.stops.is_none())
            .map(|(cs, _)| cs.as_str())
    }
}

/// Turn one routine's drafts into rules. `binds_builtin(callsite_id)` answers
/// whether a call site binds only to the builtin (#288's resolution half).
/// Drafts are settled in order, so an earlier check's `stops` is final before
/// a later check reads it.
pub fn settle_rules(drafts: &[RuleDraft], binds_builtin: impl Fn(&str) -> bool) -> Vec<PRule> {
    let mut out: Vec<PRule> = Vec::with_capacity(drafts.len());
    for d in drafts {
        let stops = match &d.stops {
            Some(s) => s.clone(),
            None => d
                .builtin_sites
                .iter()
                .find(|(cs, _)| !binds_builtin(cs))
                .map_or(Stops::Always, |(_, reason)| Stops::MayNotStop {
                    reason: (*reason).to_string(),
                }),
        };
        // A `not C` from an earlier check holds only when that check stops.
        let kept: Vec<&WhenPart> = d
            .parts
            .iter()
            .filter(|p| p.from_rule.is_none_or(|i| out[i].stops == Stops::Always))
            .collect();
        out.push(PRule {
            id: d.id.clone(),
            kind: d.kind,
            stops,
            anchor: d.anchor.clone(),
            when: kept.iter().map(|p| p.atom.clone()).collect(),
            when_text: kept
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            message: d.message.clone(),
            comment: d.comment.clone(),
        });
    }
    out
}

/// What [`build_rule_drafts`] reads besides the routine's IR.
pub struct RuleInputs<'a> {
    pub file: &'a AlFile,
    pub object: &'a ObjectDecl,
    pub routine: &'a RoutineDecl,
    pub routine_id: &'a str,
    pub source: &'a str,
    /// The routine's call sites and, parallel to them, each one's expression
    /// index (`SiteExprs::calls`).
    pub call_sites: &'a [PCallSite],
    pub call_exprs: &'a [u32],
    /// Parallel to `call_sites`: for a bare `Error(...)` site, whether #288
    /// proves it takes the Text overload (`error_argument_proves_text`);
    /// ignored for any other site.
    pub error_text_proven: &'a [bool],
    /// The routine's record variables (after global promotion): name as
    /// written, table name.
    pub record_variables: &'a [(String, Option<String>)],
    /// Whether the whole object is free of preprocessor directives (#288's
    /// rule for what a global proves).
    pub object_clean: bool,
    /// The anchor of an IR origin, as the model spells anchors.
    pub anchor: &'a dyn Fn(&Origin) -> PAnchor,
}

/// The rule drafts of one routine, in source order (see the module doc).
pub fn build_rule_drafts(inputs: &RuleInputs<'_>) -> Vec<RuleDraft> {
    let Some(body) = inputs.routine.body else {
        return Vec::new();
    };
    let mut w = Walker {
        cx: inputs,
        drafts: Vec::new(),
    };
    w.block(body, &Ctx::default(), None);
    w.drafts
}

#[derive(Clone, Default)]
struct Ctx {
    parts: Vec<WhenPart>,
    asserterror: bool,
}

impl Ctx {
    fn with(&self, extra: Vec<WhenPart>) -> Ctx {
        let mut c = self.clone();
        c.parts.extend(extra);
        c
    }

    /// The `Confirm(...)` call whose `false` result leads here, if any.
    fn confirm_false(&self) -> Option<ExprId> {
        self.parts.iter().find_map(|p| match &p.atom {
            ConditionAtom::CallResult { negated: true, .. } if p.enclosing => p.confirm,
            _ => None,
        })
    }
}

/// What a name in the routine body refers to, in AL scope order.
enum NameRef<'a> {
    Parameter(Option<&'a str>),
    ReturnValue(Option<&'a str>),
    /// One declaration, which proves its text.
    Proven(&'a VarDecl),
    /// More than one declaration the routine can see, in scope order.
    Candidates(Vec<&'a VarDecl>),
    NotFound,
}

struct Walker<'a, 'c> {
    cx: &'c RuleInputs<'a>,
    drafts: Vec<RuleDraft>,
}

fn unquote(t: &str) -> &str {
    let t = t.trim();
    t.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
}

/// The leading word of a type text (`Text[100]` → `Text`).
fn type_head(ty: &str) -> &str {
    ty.trim().split(['[', ' ']).next().unwrap_or("")
}

fn is_type(ty: Option<&str>, name: &str) -> bool {
    ty.is_some_and(|t| type_head(t).eq_fold_identifier(name))
}

/// A record variable as written, and its table when known.
type RecordRef = (String, Option<String>);

const FOUND_OPS: [&str; 6] = ["Get", "Find", "FindFirst", "FindLast", "FindSet", "IsEmpty"];

impl Walker<'_, '_> {
    fn ir(&self) -> &al_syntax::ir::Ir {
        &self.cx.file.ir
    }

    fn src(&self, e: ExprId) -> String {
        let o = &self.ir().expr(e).origin;
        self.cx
            .source
            .get(o.byte.clone())
            .unwrap_or("")
            .trim()
            .to_string()
    }

    fn unparen(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Parenthesized(inner) = &self.ir().expr(e).kind {
            e = *inner;
        }
        e
    }

    /// The bare name of an identifier expression, unquoted.
    fn name_of(&self, e: ExprId) -> Option<String> {
        match &self.ir().expr(self.unparen(e)).kind {
            ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
                Some(super::scope::unescape_al_identifier(unquote(n)))
            }
            _ => None,
        }
    }

    /// `Some(table)` when `name` is a record variable. The implicit `Rec` of a
    /// table (or table extension) trigger has no declared table; it is the
    /// object's own table (or the one it extends).
    fn record_table(&self, name: &str) -> Option<Option<String>> {
        let table = self
            .cx
            .record_variables
            .iter()
            .find(|(n, _)| unquote(n).eq_fold_identifier(name))
            .map(|(_, t)| t.clone())?;
        if table.is_none() && name.eq_fold_identifier("Rec") {
            let o = self.cx.object;
            let own = match o.kind {
                ObjectKind::Table => Some(o.name.clone()),
                ObjectKind::TableExtension => o.extends_target.clone(),
                _ => None,
            };
            return Some(own.map(|t| super::scope::unescape_al_identifier(unquote(&t))));
        }
        Some(table)
    }

    fn is_variable_or_param(&self, name: &str) -> bool {
        !matches!(self.lookup(name), NameRef::NotFound)
    }

    /// `R.F` with `R` a record variable, or a bare `F` that names a field of
    /// the table this trigger runs on (its implicit `Rec`):
    /// `(record variable, table, field)`.
    fn field_ref(&self, e: ExprId) -> Option<(String, Option<String>, String)> {
        let e = self.unparen(e);
        match &self.ir().expr(e).kind {
            ExprKind::Member { object, member, .. } => {
                let rv = self.name_of(*object)?;
                let table = self.record_table(&rv)?;
                Some((
                    rv,
                    table,
                    super::scope::unescape_al_identifier(unquote(member)),
                ))
            }
            ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) => {
                let f = self.name_of(e)?;
                if !matches!(self.cx.object.kind, ObjectKind::Table)
                    || self.is_variable_or_param(&f)
                    || !self
                        .cx
                        .object
                        .fields
                        .iter()
                        .any(|d| d.name.eq_fold_identifier(&f))
                {
                    return None;
                }
                let table = self.record_table("Rec")?;
                Some(("Rec".to_string(), table, f))
            }
            _ => None,
        }
    }

    /// The index of a by-value `Boolean` parameter named by `e`.
    fn bool_param(&self, e: ExprId) -> Option<u32> {
        let n = self.name_of(e)?;
        let (i, p) = self
            .cx
            .routine
            .params
            .iter()
            .enumerate()
            .find(|(_, p)| unquote(&p.name).eq_fold_identifier(&n))?;
        (!p.by_ref && is_type(p.ty.as_deref(), "Boolean")).then_some(i as u32)
    }

    fn is_empty_literal(&self, e: ExprId) -> bool {
        match &self.ir().expr(self.unparen(e)).kind {
            ExprKind::Literal(Literal::Text(raw)) => raw == "''",
            ExprKind::Literal(Literal::Int(v)) => v == "0",
            _ => false,
        }
    }

    fn bool_literal(&self, e: ExprId) -> Option<bool> {
        match &self.ir().expr(self.unparen(e)).kind {
            ExprKind::Literal(Literal::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// `Confirm(...)` / `Dialog.Confirm(...)`.
    fn is_confirm_call(&self, e: ExprId) -> bool {
        let ExprKind::Call { function, .. } = &self.ir().expr(e).kind else {
            return false;
        };
        match &self.ir().expr(*function).kind {
            ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
                unquote(n).eq_fold_identifier("Confirm")
            }
            ExprKind::Member { object, member, .. } => {
                unquote(member).eq_fold_identifier("Confirm")
                    && self
                        .name_of(*object)
                        .is_some_and(|o| o.eq_fold_identifier("Dialog"))
            }
            _ => false,
        }
    }

    /// `R.Get(...)`, `R.FindFirst`, ... with `R` a record variable.
    fn record_found(&self, e: ExprId) -> Option<(String, String)> {
        let member_e = match &self.ir().expr(e).kind {
            ExprKind::Call { function, .. } => *function,
            ExprKind::Member { .. } => e,
            _ => return None,
        };
        let ExprKind::Member { object, member, .. } = &self.ir().expr(member_e).kind else {
            return None;
        };
        let op = unquote(member);
        if !FOUND_OPS.iter().any(|f| op.eq_fold_identifier(f)) {
            return None;
        }
        let rv = self.name_of(*object)?;
        self.record_table(&rv)?;
        Some((rv, op.to_string()))
    }

    fn not_text(&self, e: ExprId, text: &str) -> String {
        match &self.ir().expr(e).kind {
            ExprKind::Identifier(_)
            | ExprKind::QuotedIdentifier(_)
            | ExprKind::Member { .. }
            | ExprKind::Call { .. }
            | ExprKind::Parenthesized(_) => format!("not {text}"),
            _ => format!("not ({text})"),
        }
    }

    fn part(&self, text: String, atom: ConditionAtom) -> WhenPart {
        WhenPart {
            text,
            atom,
            from_rule: None,
            enclosing: false,
            confirm: None,
        }
    }

    /// The parts of condition `e` (`negate`: of `not e`), split on top-level
    /// `and` (and, negated, on `or`).
    fn cond_parts(&self, e: ExprId, negate: bool) -> Vec<WhenPart> {
        let e = self.unparen(e);
        match (&self.ir().expr(e).kind, negate) {
            (
                ExprKind::Binary {
                    op: BinaryOp::And,
                    lhs,
                    rhs,
                },
                false,
            )
            | (
                ExprKind::Binary {
                    op: BinaryOp::Or,
                    lhs,
                    rhs,
                },
                true,
            ) => {
                let mut v = self.cond_parts(*lhs, negate);
                v.extend(self.cond_parts(*rhs, negate));
                v
            }
            (
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    operand,
                },
                n,
            ) => self.cond_parts(*operand, !n),
            _ => vec![self.atom(e, negate)],
        }
    }

    /// One atom: `e` (`neg`: `not e`).
    fn atom(&self, e: ExprId, neg: bool) -> WhenPart {
        let written = self.src(e);
        let is_or = matches!(
            self.ir().expr(e).kind,
            ExprKind::Binary {
                op: BinaryOp::Or | BinaryOp::Xor,
                ..
            }
        );
        let text = if neg {
            self.not_text(e, &written)
        } else if is_or {
            // Joined with " and ", an `or` keeps its own grouping.
            format!("({written})")
        } else {
            written.clone()
        };
        let mut confirm = None;
        let atom = match &self.ir().expr(e).kind {
            ExprKind::Binary { op, lhs, rhs } if CmpOp::of(*op).is_some() => {
                let op = CmpOp::of(*op).expect("a comparison");
                if let Some((record_var, table, field)) = self.field_ref(*lhs) {
                    if self.is_empty_literal(*rhs) && matches!(op, CmpOp::Eq | CmpOp::Ne) {
                        Some(ConditionAtom::FieldEmpty {
                            record_var,
                            table,
                            field,
                            negated: (op == CmpOp::Ne) != neg,
                        })
                    } else {
                        Some(ConditionAtom::FieldCompare {
                            record_var,
                            table,
                            field,
                            op: if neg { op.inverse() } else { op },
                            value: self.src(*rhs),
                        })
                    }
                } else if let (Some(index), Some(b), CmpOp::Eq | CmpOp::Ne) =
                    (self.bool_param(*lhs), self.bool_literal(*rhs), op)
                {
                    Some(ConditionAtom::ParamBool {
                        index,
                        value: (b != (op == CmpOp::Ne)) != neg,
                    })
                } else {
                    None
                }
            }
            ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) => self
                .bool_param(e)
                .map(|index| ConditionAtom::ParamBool { index, value: !neg }),
            ExprKind::Call { .. } | ExprKind::Member { .. } => {
                if let Some((record_var, op)) = self.record_found(e) {
                    Some(ConditionAtom::RecordFound {
                        record_var,
                        op,
                        negated: neg,
                    })
                } else if matches!(self.ir().expr(e).kind, ExprKind::Call { .. }) {
                    if self.is_confirm_call(e) {
                        confirm = Some(e);
                    }
                    Some(ConditionAtom::CallResult {
                        callee_text: written,
                        negated: neg,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        let atom = atom.unwrap_or_else(|| ConditionAtom::Other {
            text: text.clone(),
            anchor: (self.cx.anchor)(&self.ir().expr(e).origin),
        });
        WhenPart {
            confirm,
            ..self.part(text, atom)
        }
    }

    /// The parts of one `case` arm.
    fn arm_parts(&self, scrutinee: ExprId, patterns: &[ExprId]) -> Vec<WhenPart> {
        let s = self.unparen(scrutinee);
        if let (Some(b), [p]) = (self.bool_literal(s), patterns) {
            // `case true of Cond: ...`: the pattern is the condition.
            return self.cond_parts(*p, !b);
        }
        let s_text = self.src(s);
        let values: Vec<String> = patterns.iter().map(|p| self.src(*p)).collect();
        if let [p] = patterns
            && !matches!(self.ir().expr(*p).kind, ExprKind::RangeExpr { .. })
            && let Some((record_var, table, field)) = self.field_ref(s)
        {
            let text = format!("{s_text} = {}", values[0]);
            let atom = if self.is_empty_literal(*p) {
                ConditionAtom::FieldEmpty {
                    record_var,
                    table,
                    field,
                    negated: false,
                }
            } else {
                ConditionAtom::FieldCompare {
                    record_var,
                    table,
                    field,
                    op: CmpOp::Eq,
                    value: values[0].clone(),
                }
            };
            return vec![self.part(text, atom)];
        }
        let text = match values.as_slice() {
            [v] if !matches!(self.ir().expr(patterns[0]).kind, ExprKind::RangeExpr { .. }) => {
                format!("{s_text} = {v}")
            }
            _ => format!("{s_text} in [{}]", values.join(", ")),
        };
        let anchor = (self.cx.anchor)(&self.ir().expr(s).origin);
        vec![self.part(text.clone(), ConditionAtom::Other { text, anchor })]
    }

    /// The parts of a `case`'s `else` arm.
    fn case_else_parts(&self, scrutinee: ExprId, arms: &[Vec<ExprId>]) -> Vec<WhenPart> {
        let s = self.unparen(scrutinee);
        if let Some(b) = self.bool_literal(s) {
            // `case true of A: ..; B: ..; else`: none of the conditions holds.
            return arms
                .iter()
                .flatten()
                .flat_map(|p| self.cond_parts(*p, b))
                .collect();
        }
        let scrutinee = self.src(s);
        let values: Vec<String> = arms.iter().flatten().map(|p| self.src(*p)).collect();
        let text = format!("{scrutinee} not in [{}]", values.join(", "));
        vec![self.part(text, ConditionAtom::NoneOf { scrutinee, values })]
    }

    /// The one statement of `b`, through `begin .. end` wrappers.
    fn single_stmt(&self, b: BlockId) -> Option<StmtId> {
        match self.ir().block(b).items.as_slice() {
            [BlockItem::Stmt(s)] => match &self.ir().stmt(*s).kind {
                StmtKind::Block(inner) => self.single_stmt(*inner),
                _ => Some(*s),
            },
            _ => None,
        }
    }

    fn block(&mut self, b: BlockId, ctx: &Ctx, owner_row: Option<u32>) {
        let items: Vec<(Option<StmtId>, Vec<BlockId>)> = self
            .ir()
            .block(b)
            .items
            .iter()
            .map(|it| match it {
                BlockItem::Stmt(s) => (Some(*s), Vec::new()),
                BlockItem::Preproc(g) => (None, g.branches.clone()),
            })
            .collect();
        let single = items.len() == 1;
        // The parts earlier statements of this block add (`not C`).
        let mut cur = ctx.clone();
        for (stmt, branches) in items {
            let Some(s) = stmt else {
                // Each `#if` arm is walked on its own; nothing flows out of it.
                for br in branches {
                    self.block(br, &cur, None);
                }
                continue;
            };
            let first = self.drafts.len();
            self.stmt(s, &cur, if single { owner_row } else { None });
            let added = self.drafts.len() - first;
            cur.parts.extend(self.after(s, first, added));
        }
    }

    /// What statement `s` adds for the statements after it in its block:
    /// `if C then exit;` adds `not C` (the early-exit rule `param_guard`
    /// applies to effects); `if C then <check>;` adds `not C`, kept only when
    /// that check stops (settled in [`settle_rules`]).
    fn after(&self, s: StmtId, first: usize, added: usize) -> Vec<WhenPart> {
        let StmtKind::If {
            cond,
            then_block,
            else_block: None,
        } = &self.ir().stmt(s).kind
        else {
            return Vec::new();
        };
        let Some(only) = self.single_stmt(*then_block) else {
            return Vec::new();
        };
        let from_rule = match &self.ir().stmt(only).kind {
            StmtKind::Exit(_) => None,
            StmtKind::Call(e) if added == 1 && self.check_kind(*e).is_some() => Some(first),
            _ => return Vec::new(),
        };
        self.cond_parts(*cond, true)
            .into_iter()
            .map(|p| WhenPart { from_rule, ..p })
            .collect()
    }

    fn stmt(&mut self, s: StmtId, ctx: &Ctx, owner_row: Option<u32>) {
        let stmt = self.ir().stmt(s);
        let row = stmt.origin.start.row;
        match &stmt.kind {
            StmtKind::Call(e) => self.check(s, *e, ctx, owner_row),
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let (cond, then_block, else_block) = (*cond, *then_block, *else_block);
                let enclosing = |v: Vec<WhenPart>| -> Vec<WhenPart> {
                    v.into_iter()
                        .map(|p| WhenPart {
                            enclosing: true,
                            ..p
                        })
                        .collect()
                };
                let then_ctx = ctx.with(enclosing(self.cond_parts(cond, false)));
                self.block(then_block, &then_ctx, Some(row));
                if let Some(e) = else_block {
                    let else_ctx = ctx.with(enclosing(self.cond_parts(cond, true)));
                    self.block(e, &else_ctx, Some(row));
                }
            }
            StmtKind::Case {
                scrutinee,
                branches,
                else_block,
            } => {
                let scrutinee = *scrutinee;
                let arms: Vec<(Vec<ExprId>, BlockId)> = branches
                    .iter()
                    .map(|b| (b.patterns.clone(), b.body))
                    .collect();
                let else_block = *else_block;
                for (patterns, body) in &arms {
                    let parts = self.arm_parts(scrutinee, patterns);
                    self.block(*body, &ctx.with(parts), None);
                }
                if let Some(e) = else_block {
                    let pats: Vec<Vec<ExprId>> = arms.iter().map(|(p, _)| p.clone()).collect();
                    let parts = self.case_else_parts(scrutinee, &pats);
                    self.block(e, &ctx.with(parts), None);
                }
            }
            StmtKind::While { cond, body } => {
                let (cond, body) = (*cond, *body);
                let parts = self.cond_parts(cond, false);
                self.block(body, &ctx.with(parts), None);
            }
            StmtKind::Repeat { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Foreach { body, .. }
            | StmtKind::With { body, .. } => {
                let body = *body;
                self.block(body, ctx, None);
            }
            StmtKind::Try { body, catch_block } => {
                let (body, catch_block) = (*body, *catch_block);
                self.block(body, ctx, None);
                if let Some(c) = catch_block {
                    self.block(c, ctx, None);
                }
            }
            StmtKind::AssertError(b) => {
                let b = *b;
                let mut c = ctx.clone();
                c.asserterror = true;
                self.block(b, &c, None);
            }
            StmtKind::Block(b) => {
                let b = *b;
                self.block(b, ctx, owner_row);
            }
            StmtKind::Exit(_) => {
                if let Some(confirm) = ctx.confirm_false() {
                    let message = self.confirm_message(confirm);
                    self.push(
                        RuleKind::ConfirmExit,
                        s,
                        ctx,
                        owner_row,
                        Vec::new(),
                        message,
                        None,
                        Vec::new(),
                    );
                }
            }
            StmtKind::Assignment { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Unknown => {}
        }
    }

    fn confirm_message(&self, confirm: ExprId) -> Option<RuleMessage> {
        let ExprKind::Call { args, .. } = &self.ir().expr(confirm).kind else {
            return None;
        };
        let (first, rest) = args.split_first()?;
        // `Confirm(Question, Default, Value1, ...)`: the default is not a
        // placeholder value.
        Some(self.text_message(*first, rest.get(1..).unwrap_or(&[])))
    }

    /// The kind of check a call expression is, with its record variable (for
    /// `TestField` / `FieldError`).
    fn check_kind(&self, e: ExprId) -> Option<(RuleKind, Option<RecordRef>)> {
        let ExprKind::Call { function, .. } = &self.ir().expr(e).kind else {
            return None;
        };
        let builtin = |m: &str| {
            if m.eq_fold_identifier("TestField") {
                Some(RuleKind::TestField)
            } else if m.eq_fold_identifier("FieldError") {
                Some(RuleKind::FieldError)
            } else {
                None
            }
        };
        match &self.ir().expr(*function).kind {
            ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
                let n = unquote(n);
                if n.eq_fold_identifier("Error") {
                    return Some((RuleKind::Error, None));
                }
                // A bare `TestField(F)` in a table runs on its implicit `Rec`.
                let kind = builtin(n)?;
                if !matches!(
                    self.cx.object.kind,
                    ObjectKind::Table | ObjectKind::TableExtension
                ) {
                    return None;
                }
                let table = self.record_table("Rec")?;
                Some((kind, Some(("Rec".to_string(), table))))
            }
            ExprKind::Member { object, member, .. } => {
                let kind = builtin(unquote(member))?;
                let rv = self.name_of(*object)?;
                let table = self.record_table(&rv)?;
                Some((kind, Some((rv, table))))
            }
            _ => None,
        }
    }

    /// The call site of call expression `e`.
    fn call_site(&self, e: ExprId) -> Option<usize> {
        let ix = u32::try_from(e.index()).ok()?;
        self.cx.call_exprs.iter().position(|x| *x == ix)
    }

    fn is_error_info(&self, e: ExprId) -> bool {
        let Some(n) = self.name_of(e) else {
            return false;
        };
        match self.lookup(&n) {
            NameRef::Parameter(ty) | NameRef::ReturnValue(ty) => is_type(ty, "ErrorInfo"),
            NameRef::Proven(v) => is_type(v.ty.as_deref(), "ErrorInfo"),
            NameRef::Candidates(vs) => vs.iter().all(|v| is_type(v.ty.as_deref(), "ErrorInfo")),
            NameRef::NotFound => false,
        }
    }

    /// Statement `s` is the call `e`: a draft when it is a check.
    fn check(&mut self, s: StmtId, e: ExprId, ctx: &Ctx, owner_row: Option<u32>) {
        let Some((kind, record)) = self.check_kind(e) else {
            return;
        };
        let ExprKind::Call { args, .. } = &self.ir().expr(e).kind else {
            return;
        };
        let args = args.clone();
        let mut stops = ctx.asserterror.then(|| Stops::MayNotStop {
            reason: REASON_ASSERTERROR.to_string(),
        });
        let mut builtin_sites = Vec::new();
        let mut own = Vec::new();
        let (kind, message) = match kind {
            RuleKind::Error => {
                let message = args
                    .split_first()
                    .map(|(m, rest)| self.text_message(*m, rest));
                let site = self.call_site(e);
                if args.len() == 1 {
                    let proven = site
                        .and_then(|i| self.cx.error_text_proven.get(i).copied())
                        .unwrap_or(false);
                    // `Error(StrSubstNo(..))`: Text when that call is the builtin,
                    // which only resolution can tell.
                    let strsubstno = self.strsubstno_site(args[0]);
                    if let (false, Some(cs)) = (proven, &strsubstno) {
                        builtin_sites.push((cs.clone(), REASON_UNPROVEN_TEXT));
                    } else if !proven && stops.is_none() {
                        let reason = if message
                            .as_ref()
                            .is_some_and(|m| m.source == MessageSource::ErrorInfo)
                        {
                            REASON_ERROR_INFO
                        } else {
                            REASON_UNPROVEN_TEXT
                        };
                        stops = Some(Stops::MayNotStop {
                            reason: reason.to_string(),
                        });
                    }
                } else if args.is_empty() && stops.is_none() {
                    stops = Some(Stops::MayNotStop {
                        reason: REASON_UNPROVEN_TEXT.to_string(),
                    });
                }
                if let Some(i) = site {
                    builtin_sites.insert(0, (self.cx.call_sites[i].id.clone(), REASON_OWN_ERROR));
                }
                let kind = if ctx.confirm_false().is_some() {
                    RuleKind::ConfirmError
                } else {
                    RuleKind::Error
                };
                (kind, message)
            }
            RuleKind::TestField | RuleKind::FieldError => {
                let (record_var, table) = record.expect("a record check");
                let Some((field_e, rest)) = args.split_first() else {
                    return;
                };
                let field = self.name_of(*field_e).unwrap_or_else(|| self.src(*field_e));
                // As written: `R.F` for `R.TestField(F)`, `F` for a bare
                // `TestField(F)` on the implicit `Rec`.
                let bare = matches!(
                    &self.ir().expr(e).kind,
                    ExprKind::Call { function, .. } if !matches!(
                        self.ir().expr(*function).kind,
                        ExprKind::Member { .. }
                    )
                );
                let r_text = if bare {
                    self.src(*field_e)
                } else {
                    format!("{record_var}.{}", self.src(*field_e))
                };
                // A trailing ErrorInfo (`TestField(F, ErrInfo)`, `TestField(F, V,
                // ErrInfo)`, `FieldError(F, ErrInfo)`) may not stop.
                let error_info = rest.last().is_some_and(|x| self.is_error_info(*x));
                let rest: &[ExprId] = if error_info {
                    &rest[..rest.len() - 1]
                } else {
                    rest
                };
                let message = if error_info {
                    if stops.is_none() {
                        stops = Some(Stops::MayNotStop {
                            reason: REASON_ERROR_INFO.to_string(),
                        });
                    }
                    RuleMessage {
                        source: MessageSource::ErrorInfo,
                        text: None,
                        args: Vec::new(),
                        label_comment: None,
                        candidates: Vec::new(),
                    }
                } else if kind == RuleKind::FieldError
                    && let Some((m, more)) = rest.split_first()
                {
                    self.text_message(*m, more)
                } else {
                    RuleMessage {
                        source: MessageSource::Platform,
                        text: None,
                        args: Vec::new(),
                        label_comment: None,
                        candidates: Vec::new(),
                    }
                };
                if kind == RuleKind::TestField {
                    own.push(match rest.first() {
                        None => self.part(
                            format!("{r_text} is empty"),
                            ConditionAtom::FieldEmpty {
                                record_var,
                                table,
                                field,
                                negated: false,
                            },
                        ),
                        Some(v) => self.part(
                            format!("{r_text} <> {}", self.src(*v)),
                            ConditionAtom::FieldCompare {
                                record_var,
                                table,
                                field,
                                op: CmpOp::Ne,
                                value: self.src(*v),
                            },
                        ),
                    });
                }
                (kind, Some(message))
            }
            RuleKind::ConfirmError | RuleKind::ConfirmExit => unreachable!("not a call kind"),
        };
        self.push(kind, s, ctx, owner_row, own, message, stops, builtin_sites);
    }

    /// The call-site id of `e` when it is a bare `StrSubstNo(..)` call.
    fn strsubstno_site(&self, e: ExprId) -> Option<String> {
        let e = self.unparen(e);
        let ExprKind::Call { function, .. } = &self.ir().expr(e).kind else {
            return None;
        };
        let is_bare = matches!(
            self.ir().expr(*function).kind,
            ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_)
        );
        if !is_bare
            || !self
                .name_of(*function)
                .is_some_and(|f| f.eq_fold_identifier("StrSubstNo"))
        {
            return None;
        }
        self.call_site(e).map(|i| self.cx.call_sites[i].id.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: RuleKind,
        s: StmtId,
        ctx: &Ctx,
        owner_row: Option<u32>,
        own: Vec<WhenPart>,
        message: Option<RuleMessage>,
        stops: Option<Stops>,
        builtin_sites: Vec<(String, &'static str)>,
    ) {
        let origin = &self.ir().stmt(s).origin;
        let mut parts = ctx.parts.clone();
        parts.extend(own);
        let draft = RuleDraft {
            id: format!("{}/rule{}", self.cx.routine_id, self.drafts.len()),
            kind,
            anchor: (self.cx.anchor)(origin),
            comment: self.comment(origin.start.row, origin.end.row, owner_row),
            message,
            stops,
            builtin_sites,
            parts,
        };
        self.drafts.push(draft);
    }

    /// The check's comment: a trailing `//` on its first or last line (or on
    /// its `if`'s line), else the own-line `//` run directly above it (or above
    /// its `if`).
    fn comment(&self, start: u32, end: u32, owner_row: Option<u32>) -> Option<String> {
        let comments = &self.cx.file.comments;
        let strip = |t: &str| -> String {
            let t = t
                .strip_prefix("///")
                .or_else(|| t.strip_prefix("//"))
                .unwrap_or(t);
            t.strip_prefix(' ').unwrap_or(t).trim_end().to_string()
        };
        let rows = [Some(end), Some(start), owner_row];
        if let Some(c) = comments
            .iter()
            .find(|c| !c.block && !c.own_line && rows.contains(&Some(c.origin.start.row)))
        {
            return Some(strip(&c.text));
        }
        let above = |row: u32| -> Option<String> {
            let mut lines = Vec::new();
            let mut r = row;
            while r > 0 {
                r -= 1;
                let Some(c) = comments
                    .iter()
                    .find(|c| !c.block && c.own_line && c.origin.start.row == r)
                else {
                    break;
                };
                lines.push(strip(&c.text));
            }
            lines.reverse();
            (!lines.is_empty()).then(|| lines.join("\n"))
        };
        above(start).or_else(|| owner_row.filter(|r| *r != start).and_then(above))
    }

    /// The declaration a bare name means, in AL scope order (params, the named
    /// return value, locals, globals: [`scope_tier`], shared with #288). The
    /// G52 rule decides whether it proves its text: a local only when it is
    /// declared once and no global has the name; a global only when it is
    /// declared once and the object is free of directives.
    fn lookup(&self, name: &str) -> NameRef<'_> {
        let r = self.cx.routine;
        let param = r
            .params
            .iter()
            .find(|p| unquote(&p.name).eq_fold_identifier(name));
        let is_return = r
            .return_name
            .as_deref()
            .is_some_and(|n| unquote(n).eq_fold_identifier(name));
        let locals: Vec<&VarDecl> = r
            .locals
            .iter()
            .filter(|v| unquote(&v.name).eq_fold_identifier(name))
            .collect();
        let globals: Vec<&VarDecl> = self
            .cx
            .object
            .globals
            .iter()
            .filter(|v| unquote(&v.name).eq_fold_identifier(name))
            .collect();
        match scope_tier(
            param.is_some(),
            is_return,
            !locals.is_empty(),
            !globals.is_empty(),
        ) {
            Some(ScopeTier::Parameter) => NameRef::Parameter(param.and_then(|p| p.ty.as_deref())),
            Some(ScopeTier::ReturnValue) => NameRef::ReturnValue(r.return_type.as_deref()),
            Some(ScopeTier::Local) => {
                if locals.len() == 1 && globals.is_empty() {
                    NameRef::Proven(locals[0])
                } else {
                    NameRef::Candidates(locals.into_iter().chain(globals).collect())
                }
            }
            Some(ScopeTier::Global) => {
                if globals.len() == 1 && self.cx.object_clean {
                    NameRef::Proven(globals[0])
                } else {
                    NameRef::Candidates(globals)
                }
            }
            None => NameRef::NotFound,
        }
    }

    /// The message whose text expression is `first`, with further arguments
    /// `rest`.
    fn text_message(&self, first: ExprId, rest: &[ExprId]) -> RuleMessage {
        let args: Vec<String> = rest.iter().map(|a| self.src(*a)).collect();
        let mut m = RuleMessage {
            source: MessageSource::Dynamic,
            text: None,
            args,
            label_comment: None,
            candidates: Vec::new(),
        };
        let first = self.unparen(first);
        match &self.ir().expr(first).kind {
            ExprKind::Literal(Literal::Text(raw)) => {
                m.source = MessageSource::Literal;
                m.text = al_syntax::ir::al_string_literal_text(raw);
            }
            ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) => {
                let name = self.name_of(first).expect("an identifier");
                match self.lookup(&name) {
                    NameRef::Parameter(ty) | NameRef::ReturnValue(ty) => {
                        if is_type(ty, "ErrorInfo") {
                            m.source = MessageSource::ErrorInfo;
                        }
                    }
                    NameRef::Proven(v) => self.declared_message(v, &mut m),
                    NameRef::Candidates(vs) => {
                        m.candidates = vs
                            .iter()
                            .filter_map(|v| v.label.as_ref().and_then(|l| l.text.clone()))
                            .collect();
                    }
                    NameRef::NotFound => {}
                }
            }
            ExprKind::Call { function, args } => {
                let is_strsubstno = self
                    .name_of(*function)
                    .is_some_and(|f| f.eq_fold_identifier("StrSubstNo"));
                if is_strsubstno && let Some((inner, inner_rest)) = args.split_first() {
                    let mut inner = self.text_message(*inner, inner_rest);
                    inner.args.extend(m.args);
                    return inner;
                }
            }
            _ => {}
        }
        m
    }

    /// The message of a variable with one declaration: a label's or a
    /// `TextConst`'s text, or an `ErrorInfo` (with its text when the routine
    /// sets it exactly once).
    fn declared_message(&self, v: &VarDecl, m: &mut RuleMessage) {
        let name = unquote(&v.name).to_string();
        if let Some(l) = &v.label {
            m.source = if is_type(v.ty.as_deref(), "TextConst") {
                MessageSource::TextConst { var: name }
            } else {
                MessageSource::Label { var: name }
            };
            m.text = l.text.clone();
            m.label_comment = l.comment.clone();
        } else if is_type(v.ty.as_deref(), "ErrorInfo") {
            m.source = MessageSource::ErrorInfo;
            if let Some(set) = self.error_info_message(&name) {
                m.text = set.text;
                m.args = set.args;
                m.label_comment = set.label_comment;
            }
        }
    }

    /// The message an `ErrorInfo` variable is given in this routine, when it
    /// is given exactly one: `EI.Message(X)` or `EI := ErrorInfo.Create(X, ..)`.
    fn error_info_message(&self, var: &str) -> Option<RuleMessage> {
        let body = self.cx.routine.body?;
        let mut found: Vec<ExprId> = Vec::new();
        self.each_stmt(body, &mut |w, s| match &w.ir().stmt(s).kind {
            StmtKind::Call(e) => {
                if let ExprKind::Call { function, args } = &w.ir().expr(*e).kind
                    && let ExprKind::Member { object, member, .. } = &w.ir().expr(*function).kind
                    && unquote(member).eq_fold_identifier("Message")
                    && w.name_of(*object)
                        .is_some_and(|o| o.eq_fold_identifier(var))
                    && let [a] = args.as_slice()
                {
                    found.push(*a);
                }
            }
            StmtKind::Assignment { target, value, .. } => {
                if w.name_of(*target)
                    .is_some_and(|t| t.eq_fold_identifier(var))
                    && let ExprKind::Call { function, args } = &w.ir().expr(*value).kind
                    && let ExprKind::Member { object, member, .. } = &w.ir().expr(*function).kind
                    && unquote(member).eq_fold_identifier("Create")
                    && w.name_of(*object)
                        .is_some_and(|o| o.eq_fold_identifier("ErrorInfo"))
                    && let Some(a) = args.first()
                {
                    found.push(*a);
                }
            }
            _ => {}
        });
        match found.as_slice() {
            [a] => Some(self.text_message(*a, &[])),
            _ => None,
        }
    }

    /// Every statement under block `b`, in source order.
    fn each_stmt(&self, b: BlockId, f: &mut dyn FnMut(&Self, StmtId)) {
        for it in &self.ir().block(b).items {
            match it {
                BlockItem::Stmt(s) => {
                    f(self, *s);
                    for inner in self.child_blocks(*s) {
                        self.each_stmt(inner, f);
                    }
                }
                BlockItem::Preproc(g) => {
                    for br in &g.branches {
                        self.each_stmt(*br, f);
                    }
                }
            }
        }
    }

    fn child_blocks(&self, s: StmtId) -> Vec<BlockId> {
        match &self.ir().stmt(s).kind {
            StmtKind::If {
                then_block,
                else_block,
                ..
            } => std::iter::once(*then_block).chain(*else_block).collect(),
            StmtKind::Case {
                branches,
                else_block,
                ..
            } => branches.iter().map(|b| b.body).chain(*else_block).collect(),
            StmtKind::While { body, .. }
            | StmtKind::Repeat { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Foreach { body, .. }
            | StmtKind::With { body, .. }
            | StmtKind::AssertError(body)
            | StmtKind::Block(body) => vec![*body],
            StmtKind::Try { body, catch_block } => {
                std::iter::once(*body).chain(*catch_block).collect()
            }
            _ => Vec::new(),
        }
    }
}
