//! Predicate evaluator — port of al-sem `src/policy/predicate-evaluator.ts`.
//!
//! Tristate (Kleene) evaluation of a compiled predicate over a [`FieldEvalContext`].
//! 4 operators (`==` / `in` / `glob` / `glob-in`) + the 3 Kleene truth tables
//! (AND/OR/NOT). Two modes:
//!   - `full`: every field evaluated normally (fact-scope reads `ctx.fact`).
//!   - `applicability`: fact-scope fields short-circuit to `unknown` (no fact
//!     required) — used to skip routines a rule structurally cannot apply to.

use crate::engine::gate::policy::policy_types::{Predicate, PredicateOperator, PredicateValue};
use crate::engine::gate::policy::predicate_fields::{
    FieldEvalContext, FieldScope, FieldValue, evaluate_field, get_field_def,
};

/// Kleene tristate. al-sem `Tristate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tristate {
    True,
    False,
    Unknown,
}

/// AND (all): false dominates > unknown > true.
pub fn kleene_and(a: Tristate, b: Tristate) -> Tristate {
    if a == Tristate::False || b == Tristate::False {
        Tristate::False
    } else if a == Tristate::Unknown || b == Tristate::Unknown {
        Tristate::Unknown
    } else {
        Tristate::True
    }
}

/// OR (any): true dominates > unknown > false.
pub fn kleene_or(a: Tristate, b: Tristate) -> Tristate {
    if a == Tristate::True || b == Tristate::True {
        Tristate::True
    } else if a == Tristate::Unknown || b == Tristate::Unknown {
        Tristate::Unknown
    } else {
        Tristate::False
    }
}

/// NOT: swaps true/false, unknown→unknown.
pub fn kleene_not(a: Tristate) -> Tristate {
    match a {
        Tristate::True => Tristate::False,
        Tristate::False => Tristate::True,
        Tristate::Unknown => Tristate::Unknown,
    }
}

/// Evaluation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalMode {
    Full,
    Applicability,
}

/// Match an operator against the actual field value. al-sem `matchOperator`.
fn match_operator(op: PredicateOperator, actual: &FieldValue, expected: &PredicateValue) -> bool {
    match op {
        PredicateOperator::Eq => match (actual, expected) {
            // strict eq: actual === expected. actual is a single string here.
            (FieldValue::KnownStr(a), PredicateValue::Str(e)) => a == e,
            _ => false,
        },
        PredicateOperator::In => {
            // membership; if actual is an array, ANY-overlap.
            let PredicateValue::List(list) = expected else {
                return false;
            };
            match actual {
                FieldValue::KnownStr(a) => list.iter().any(|e| e == a),
                FieldValue::KnownList(arr) => arr.iter().any(|a| list.iter().any(|e| e == a)),
                FieldValue::Unknown(_) => false,
            }
        }
        PredicateOperator::Glob => {
            let PredicateValue::Str(pat) = expected else {
                return false;
            };
            glob_match(pat, &actual_to_string(actual))
        }
        PredicateOperator::GlobIn => {
            let PredicateValue::List(pats) = expected else {
                return false;
            };
            match actual {
                FieldValue::KnownList(arr) => {
                    pats.iter().any(|g| arr.iter().any(|a| glob_match(g, a)))
                }
                _ => {
                    let s = actual_to_string(actual);
                    pats.iter().any(|g| glob_match(g, &s))
                }
            }
        }
    }
}

/// Test-only re-export of [`match_operator`] so the differential's glob-vs-glob-in
/// array-asymmetry oracle can drive the operator directly.
#[doc(hidden)]
pub fn match_operator_for_test(
    op: PredicateOperator,
    actual: &FieldValue,
    expected: &PredicateValue,
) -> bool {
    match_operator(op, actual, expected)
}

/// al-sem `actualToString`: strings pass through, arrays join with `,`.
fn actual_to_string(v: &FieldValue) -> String {
    match v {
        FieldValue::KnownStr(s) => s.clone(),
        FieldValue::KnownList(arr) => arr.join(","),
        // String(undefined) etc. — never reached for a KNOWN value; defensive.
        FieldValue::Unknown(_) => String::new(),
    }
}

/// Case-insensitive anchored glob, byte-parity with al-sem's `globToRegExp`
/// (`predicate-evaluator.ts` `compileGlob`): `new RegExp("^" + escaped + "$", "i")`
/// where `escaped` escapes the regex metachars `. + ^ $ { } ( ) | [ ] \`, then
/// `*` → `.*` and `?` → `.`.
///
/// CRITICAL no-ReDoS / no-overflow guarantee: we compile to the `regex` crate
/// (a non-backtracking finite automaton — LINEAR time, no recursion), so a
/// malicious user policy glob (e.g. `*a*a*…*a` against a long value, or a 10k-char
/// pattern) can never hang or stack-overflow the engine. The previous hand-rolled
/// backtracking matcher was catastrophically exponential AND recursed per char.
///
/// Line-terminator parity: a JS `RegExp` `.` WITHOUT the `s` flag excludes the four
/// ECMAScript line terminators (`\n` U+000A, `\r` U+000D, U+2028 LS, U+2029 PS).
/// Rust regex `.` (default `(?-s)`) excludes only `\n`, so we translate `*`/`?` to
/// the EXPLICIT negated class `[^\n\r\u{2028}\u{2029}]` to match al-sem exactly
/// (this also fixes a latent `\r`/LS/PS divergence in the old matcher).
///
/// Case-insensitivity: al-sem's `i`-flag (no `u`) is UTF-16 default case folding.
/// We use ASCII-case-insensitive matching (`ascii_case_insensitive(true)`): for the
/// ASCII/BMP table/event/routine/object names the policy corpus compares, this is
/// identical to JS; full-Unicode `case_insensitive` would ADD folds JS-without-`u`
/// does NOT do (Kelvin sign K↔k, long-s ſ↔s), so ASCII-CI is the closer match. The
/// non-ASCII UTF-16-vs-unicode-case edge is the tracked cross-cutting comparator
/// item (shared with `compareStrings`), not a policy-specific divergence.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    // ASCII-case-insensitivity is implemented by folding ASCII letters on BOTH sides
    // (literal pattern chars + the value) rather than the regex `i` flag, because the
    // regex needs `unicode(true)` for the `\x{2028}`/`\x{2029}` line-terminator class
    // but we want ASCII-only folding (full-Unicode `case_insensitive` would add
    // Kelvin/long-s folds JS-without-`u` does not). `to_ascii_lowercase` touches only
    // A-Z, leaving every other codepoint (incl. the 4 line terminators) intact.
    let folded_value = value.to_ascii_lowercase();
    GLOB_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(re) = cache.get(pattern) {
            return re.is_match(&folded_value);
        }
        let re = compile_glob(pattern);
        let m = re.is_match(&folded_value);
        cache.insert(pattern.to_string(), re);
        m
    })
}

thread_local! {
    /// Per-thread compiled-regex cache (mirrors al-sem's module-level `globCache`).
    static GLOB_CACHE: std::cell::RefCell<std::collections::HashMap<String, regex::Regex>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Translate a glob pattern to an anchored `regex::Regex` (case folding is done by
/// the caller via ASCII-lowercasing both sides, so literal chars are lowercased here
/// and the regex itself is case-SENSITIVE).
fn compile_glob(pattern: &str) -> regex::Regex {
    // `[^\n\r\u{2028}\u{2029}]` — JS `.`-without-`s` (excludes the 4 line terminators).
    const NON_LT: &str = "[^\\n\\r\\x{2028}\\x{2029}]";
    let mut re = String::with_capacity(pattern.len() + 4);
    re.push('^');
    for c in pattern.chars() {
        match c {
            '*' => {
                re.push_str(NON_LT);
                re.push('*');
            }
            '?' => re.push_str(NON_LT),
            // Escape the regex metachars al-sem escapes: . + ^ $ { } ( ) | [ ] \
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(c.to_ascii_lowercase());
            }
            // Literal: ASCII-lowercase (the value is folded the same way) — this is
            // the ASCII-CI implementation. Non-ASCII literals pass through verbatim.
            other => re.push(other.to_ascii_lowercase()),
        }
    }
    re.push('$');
    // On the off-chance a pathological pattern exceeds the default regex size limit,
    // fall back to a never-match regex rather than aborting (engine-never-throws).
    regex::Regex::new(&re).unwrap_or_else(|_| {
        regex::Regex::new("[^\\x00-\\x{10FFFF}]").expect("never-match regex is valid")
    })
}

/// Evaluate a single field predicate to a tristate. `mode` controls fact-scope
/// short-circuiting.
fn field_tristate(
    field: &str,
    operator: PredicateOperator,
    value: &PredicateValue,
    ctx: &FieldEvalContext,
    mode: EvalMode,
) -> Tristate {
    let Some(def) = get_field_def(field) else {
        return Tristate::Unknown;
    };
    // Applicability: a fact-scoped field cannot be decided without a fact, nor
    // a pin's write or rule field without its write fact or applied key.
    if mode == EvalMode::Applicability
        && matches!(
            def.scope,
            FieldScope::Fact | FieldScope::Write | FieldScope::Rule
        )
    {
        return Tristate::Unknown;
    }
    let fv = evaluate_field(def, ctx);
    match fv {
        FieldValue::Unknown(_) => Tristate::Unknown,
        known => {
            if match_operator(operator, &known, value) {
                Tristate::True
            } else {
                Tristate::False
            }
        }
    }
}

/// Tristate-only evaluation (no trace). al-sem `evalTristate`.
pub fn eval_tristate(p: &Predicate, ctx: &FieldEvalContext, mode: EvalMode) -> Tristate {
    match p {
        Predicate::Field {
            field,
            operator,
            value,
        } => field_tristate(field, *operator, value, ctx, mode),
        Predicate::All { children } => {
            if children.is_empty() {
                return Tristate::True; // empty conjunction → true
            }
            let mut acc = Tristate::True;
            for c in children {
                acc = kleene_and(acc, eval_tristate(c, ctx, mode));
                if acc == Tristate::False {
                    return Tristate::False; // short-circuit
                }
            }
            acc
        }
        Predicate::Any { children } => {
            if children.is_empty() {
                return Tristate::False; // empty disjunction → false
            }
            let mut acc = Tristate::False;
            for c in children {
                acc = kleene_or(acc, eval_tristate(c, ctx, mode));
                if acc == Tristate::True {
                    return Tristate::True;
                }
            }
            acc
        }
        Predicate::Not { child } => kleene_not(eval_tristate(child, ctx, mode)),
    }
}

/// Full evaluation (fact present). al-sem `evaluateResult`.
pub fn evaluate_result(p: &Predicate, ctx: &FieldEvalContext) -> Tristate {
    eval_tristate(p, ctx, EvalMode::Full)
}

/// Applicability evaluation (fact fields ⇒ unknown). al-sem `evaluateApplicability`.
/// `false` ⇒ no fact can satisfy P for this routine ⇒ rule not applicable.
pub fn evaluate_applicability(p: &Predicate, ctx: &FieldEvalContext) -> Tristate {
    eval_tristate(p, ctx, EvalMode::Applicability)
}
