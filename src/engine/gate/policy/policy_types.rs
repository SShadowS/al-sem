//! Policy types — port of al-sem `src/policy/policy-types.ts`.
//!
//! The compiled `Predicate` AST + `Rule` / `PolicyDoc` + the per-rule run
//! summary. The AST is serialized (insertion-order) by `policy explain`'s
//! "Normalized AST" block via [`predicate_to_json`].

/// Predicate operator. al-sem `PredicateOperator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOperator {
    Eq,
    In,
    Glob,
    GlobIn,
}

impl PredicateOperator {
    pub fn as_str(&self) -> &'static str {
        match self {
            PredicateOperator::Eq => "==",
            PredicateOperator::In => "in",
            PredicateOperator::Glob => "glob",
            PredicateOperator::GlobIn => "glob-in",
        }
    }
}

/// A predicate value: either a single string (`==`/`glob`) or a list (`in`/`glob-in`).
/// Mirrors al-sem's `value: unknown` which is always a `string` or `string[]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateValue {
    Str(String),
    List(Vec<String>),
}

/// The compiled predicate AST. al-sem `Predicate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Field {
        field: String,
        operator: PredicateOperator,
        value: PredicateValue,
    },
    All {
        children: Vec<Predicate>,
    },
    Any {
        children: Vec<Predicate>,
    },
    Not {
        child: Box<Predicate>,
    },
}

/// Policy severity (5-level). al-sem `PolicySeverity`.
pub const ALLOWED_SEVERITIES: &[&str] = &["critical", "high", "medium", "low", "info"];
/// al-sem `UnknownPolicy`.
pub const ALLOWED_UNKNOWN: &[&str] = &["fail-open", "fail-closed"];
/// al-sem `CoveragePolicy`.
pub const ALLOWED_COVERAGE: &[&str] = &["complete", "partial", "any"];
/// al-sem `FactOriginFilter`.
pub const ALLOWED_FACTS: &[&str] = &["direct", "inherited", "any"];

/// A compiled rule. al-sem `Rule`.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub message: Option<String>,
    pub severity: String,
    pub when: Predicate,
    pub except: Option<Predicate>,
    pub require_coverage: Option<String>,
    pub on_unknown: Option<String>,
    pub facts: Option<String>,
    /// A pin's `requires` (policy version 2; rules-to-policy spec §2.2): a
    /// predicate over one rule key a writer applies. `Some` makes the rule a
    /// pin: its `when` selects write facts, and a selected writer violates it
    /// when no key it applies satisfies `requires`.
    pub requires: Option<Predicate>,
}

/// The newest policy format version this engine reads. Version 2 added pins
/// (`requires` and the `write.*` / `rule.*` fields); version 1 is the format
/// without them and is still read.
pub const POLICY_FORMAT_VERSION: i64 = 2;

/// Policy defaults block. al-sem `PolicyDoc.defaults`.
#[derive(Debug, Clone, Default)]
pub struct PolicyDefaults {
    pub on_unknown: Option<String>,
    pub require_coverage: Option<String>,
}

/// A fully validated policy document. al-sem `PolicyDoc`.
#[derive(Debug, Clone)]
pub struct PolicyDoc {
    pub version: i64,
    pub description: Option<String>,
    pub defaults: Option<PolicyDefaults>,
    pub rules: Vec<Rule>,
}

impl PolicyDoc {
    /// Whether some rule is a pin. Only then does `policy check` build the
    /// routines' rules and the writer index (the loader keeps every write and
    /// rule field inside a pin, so this is the one test).
    #[must_use]
    pub fn has_pins(&self) -> bool {
        self.rules.iter().any(|r| r.requires.is_some())
    }
}

/// Per-rule run counters. al-sem `RuleRunSummary` (the subset the engine emits —
/// no `traces`; `errors` is skip-if-none).
#[derive(Debug, Clone)]
pub struct RuleRunSummary {
    pub rule_id: String,
    pub routines_evaluated: usize,
    pub routines_matched: usize,
    pub routines_skipped_coverage: usize,
    pub routines_skipped_unknown: usize,
    pub routines_passed: usize,
    pub findings_emitted: usize,
    pub errors: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Predicate AST → JSON (insertion-order) for `policy explain`.
//
// al-sem renders `JSON.stringify(rule.when, undefined, 2)`. The compiled AST node
// key order is the construction order:
//   field: { kind, field, operator, value }
//   all:   { kind, children }
//   any:   { kind, children }
//   not:   { kind, child }
// Built on the shared `gate::ordered_json` insertion-order serializer.
// ---------------------------------------------------------------------------

use crate::engine::gate::ordered_json::{Jv, serialize_jv};

fn predicate_to_jv(p: &Predicate) -> Jv {
    match p {
        Predicate::Field {
            field,
            operator,
            value,
        } => {
            let value_jv = match value {
                PredicateValue::Str(s) => Jv::s(s),
                PredicateValue::List(xs) => Jv::Arr(xs.iter().map(|x| Jv::s(x)).collect()),
            };
            Jv::Obj(vec![
                ("kind".to_string(), Jv::s("field")),
                ("field".to_string(), Jv::s(field)),
                ("operator".to_string(), Jv::s(operator.as_str())),
                ("value".to_string(), value_jv),
            ])
        }
        Predicate::All { children } => Jv::Obj(vec![
            ("kind".to_string(), Jv::s("all")),
            (
                "children".to_string(),
                Jv::Arr(children.iter().map(predicate_to_jv).collect()),
            ),
        ]),
        Predicate::Any { children } => Jv::Obj(vec![
            ("kind".to_string(), Jv::s("any")),
            (
                "children".to_string(),
                Jv::Arr(children.iter().map(predicate_to_jv).collect()),
            ),
        ]),
        Predicate::Not { child } => Jv::Obj(vec![
            ("kind".to_string(), Jv::s("not")),
            ("child".to_string(), predicate_to_jv(child)),
        ]),
    }
}

/// Serialize the predicate AST to `JSON.stringify(p, undefined, 2)` form (no
/// trailing newline). Used by `policy explain`'s "Normalized AST" block.
pub fn predicate_to_json(p: &Predicate) -> String {
    serialize_jv(&predicate_to_jv(p))
}
