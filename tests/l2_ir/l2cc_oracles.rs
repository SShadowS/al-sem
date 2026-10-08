//! R1b EXIT GATE — native L2-DIRECT control-context invariant oracle.
//!
//! These are ground-truth-free, lattice-invariant oracles run NATIVELY against
//! the Rust L2 control-context walker (`src/engine/l2/control_context.rs` over the
//! validated R1a CFN skeleton). They assert the `ControlContext` lattice
//! invariants DIRECTLY on each `OperationSite`/`CallSite.control_context` value —
//! NOT a golden diff against expected strings (that is `l2cc_vectors.rs`), and NOT
//! the downstream L4 effect-reachability the TS `reachability-crosscheck` oracle
//! adds (see "Covered vs deferred" below).
//!
//! ## Why an L2-DIRECT oracle (not the TS reachability-crosscheck port)
//!
//! The TS `reachability-crosscheck*.test.ts` oracle reasons over DOWNSTREAM L4
//! effect summaries (a control-context that narrows to `unreachable` must drop the
//! effect from the routine summary; an `error-path` op must not seed a finding on
//! a path that always raises). The Rust L2 output has NO effects/summaries — it
//! stops at the index boundary — so porting that oracle here would assert nothing
//! about L2. Instead we assert, at the L2 boundary, the lattice invariants the TS
//! oracle ULTIMATELY RESTS ON: if the L2 control-context classification is correct,
//! the downstream reachability behavior the TS oracle guards follows by
//! construction. A control-context bug (wrong lattice `max`, missed termination,
//! mis-scoped IsHandled elevation, broken error-path propagation) breaks one of
//! the invariants below.
//!
//! Each invariant is a focused `#[test]` over a small inline AL fixture, driven
//! through the real walker via [`analyze_named_routine`] (the same entry point the
//! emitter + `l2cc_vectors.rs` use). The fixtures are independent of the finite
//! `ws-*` corpus, so they catch control-context bugs the corpus misses.
//!
//! ## Covered (the L2 control-context lattice core, R1b's guard)
//!   - condition leaves evaluate at AMBIENT context (an op inside an if/while/case
//!     condition is at the enclosing context, NOT `conditional`/`loop-body`);
//!   - branch bodies (then/else, case arms) are >= `conditional`;
//!   - loop bodies are `loop-body`;
//!   - an `if` inside a loop stays `loop-body` (lattice `max` accumulation, never
//!     drops back to `conditional`);
//!   - single-arm `if Bad then Error()` -> the error branch is `error-path`, the
//!     continuation is UNCHANGED (not narrowed, not unreachable);
//!   - bare `Error()` / `exit` -> following same-block sites are `unreachable`;
//!   - both-arms-terminating `if` -> the continuation is `unreachable`;
//!   - `case` with any terminating arm -> the continuation narrows to
//!     `conditional`, NEVER `unreachable` (a case is not exhaustive at L2);
//!   - `[TryFunction]` routine -> control-context ABSENT (None) on ALL sites;
//!   - IsHandled positive (`if X then exit`) + negative (`if not X then`) polarity
//!     upgrade ONLY the TS-recognized region for ELIGIBLE vars, and an INELIGIBLE
//!     var (a by-value bool param) does NOT upgrade.
//!
//! ## Deferred (NOT L2 control-context; later gates / not L2-observable)
//!   - The TS oracle's downstream effect-reachability claims (an `unreachable`
//!     op is dropped from the L4 summary; an `error-path` op does not seed a
//!     finding on an always-raising path; `may-commit` / `commits-on-success-path`
//!     PROVE answers). Those depend on L3 resolve + L4 summaries + digest/prove and
//!     are R1c+/R2 surfaces. At L2 we assert the upstream lattice invariant they
//!     all rest on (this file).
//!   - Operation-ORDER / scope-frame context (`order`/`onSuccessPath`/
//!     `dominatesSuccessReturn`) is R1c, structurally absent from the L2 projection.
//!
//! If any case below revealed a Rust/invariant divergence, the fix would live in
//! `src/engine/l2/{control_context,control_flow}.rs`. As of this gate every case
//! passes with no `src/engine/l2/**` change required.

use al_sem::program::body::control_context::{
    ControlContext, RoutineControlContexts, analyze_named_routine,
};
use al_sem::program::body::features::PCallee;

const APP_GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const MODEL_INSTANCE_ID: &str = "r0";
const SOURCE_UNIT_ID: &str = "ws:src/vec.al";

/// Run the full Rust L2 control-context analysis over an inline single-file
/// workspace and return the result for `routine` (panics if the routine isn't
/// found — a missing routine is itself an oracle failure).
fn analyze(source: &str, routine: &str) -> RoutineControlContexts {
    analyze_named_routine(source, routine, APP_GUID, MODEL_INSTANCE_ID, SOURCE_UNIT_ID)
        .unwrap_or_else(|| panic!("routine `{routine}` not found by the Rust L2 walker"))
}

/// Context of the FIRST operation site whose `kind` matches `kind_filter`
/// (`None` => any kind) at the given source line. Lines are passed 1-based (as
/// they read in the fixture string); source anchors are 0-based, so we convert.
/// Matching by line is unambiguous for these single-statement-per-line fixtures
/// (columns vary with leading whitespace).
fn op_ctx_at_line(a: &RoutineControlContexts, line_1based: u32, kind_filter: Option<&str>) -> Ctx {
    let line = line_1based - 1;
    let mut hits = a.operation_sites.iter().filter(|op| {
        op.source_anchor.start_line == line && kind_filter.map(|k| op.kind == k).unwrap_or(true)
    });
    match hits.next() {
        Some(op) => Ctx(a.by_operation.get(&op.id).copied()),
        None => panic!(
            "no operation site (kind={:?}) at line {line}; sites present: {:?}",
            kind_filter,
            a.operation_sites
                .iter()
                .map(|o| (o.source_anchor.start_line, o.kind.as_str()))
                .collect::<Vec<_>>()
        ),
    }
}

/// Context of the FIRST call site at the given source line (1-based, as in the
/// fixture string; anchors are 0-based, so we convert).
fn callsite_ctx_at_line(a: &RoutineControlContexts, line_1based: u32) -> Ctx {
    let line = line_1based - 1;
    let mut hits = a
        .call_sites
        .iter()
        .filter(|cs| cs.source_anchor.start_line == line);
    match hits.next() {
        Some(cs) => Ctx(a.by_callsite.get(&cs.id).copied()),
        None => panic!(
            "no call site at line {line}; sites present: {:?}",
            a.call_sites
                .iter()
                .map(|c| (c.source_anchor.start_line, callee_text(&c.callee)))
                .collect::<Vec<_>>()
        ),
    }
}

fn callee_text(c: &PCallee) -> String {
    match c {
        PCallee::Bare { name } => name.clone(),
        PCallee::Member { method, .. } => method.clone(),
        _ => "<other>".to_string(),
    }
}

/// A control-context value, comparable by lattice rank and printable.
/// `None` == ABSENT == al-sem left `controlContext` undefined.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Ctx(Option<ControlContext>);

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(c) => write!(f, "{}", c.as_str()),
            None => write!(f, "<absent>"),
        }
    }
}

impl Ctx {
    fn is(self, c: ControlContext) -> bool {
        self.0 == Some(c)
    }
    /// Lattice rank, treating absent as below `top-level` for >= comparisons.
    fn rank(self) -> i16 {
        self.0.map(|c| c.rank() as i16).unwrap_or(-1)
    }
}

// ===========================================================================
// condition leaves evaluate at AMBIENT context
// ===========================================================================

/// An op in an if-condition is at the ENCLOSING context (top-level here), not
/// `conditional`. The branch body it guards is `conditional`. This is the
/// "condition leaves at ambient" invariant.
#[test]
fn condition_leaf_in_if_is_ambient_not_conditional() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        if Cust.IsEmpty() then\n            Cust.Init();\n    end;\n}";
    let a = analyze(src, "P");
    // `Cust.IsEmpty()` (line 5) is a condition leaf → ambient top-level.
    assert!(
        op_ctx_at_line(&a, 5, None).is(ControlContext::TopLevel),
        "if-condition leaf must be at AMBIENT (top-level), got {:?}",
        op_ctx_at_line(&a, 5, None)
    );
    // `Cust.Init()` (line 6) is the then-body → conditional.
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::Conditional),
        "then-body must be conditional, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
}

/// A condition leaf inside a WHILE header evaluates at ambient (top-level), not
/// `loop-body` — only the body is loop-body.
#[test]
fn condition_leaf_in_while_header_is_ambient_not_loop_body() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        while Cust.IsEmpty() do\n            Cust.Init();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 5, None).is(ControlContext::TopLevel),
        "while-condition leaf must be at AMBIENT (top-level), got {:?}",
        op_ctx_at_line(&a, 5, None)
    );
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::LoopBody),
        "while-body must be loop-body, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
}

// ===========================================================================
// branch bodies >= conditional; loop bodies = loop-body
// ===========================================================================

/// Both arms of an if (then + explicit else) are >= conditional.
#[test]
fn both_if_arms_are_conditional() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        if Cust.IsEmpty() then\n            Cust.Init()\n        else\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::Conditional),
        "then-arm must be conditional, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
    assert!(
        op_ctx_at_line(&a, 8, None).is(ControlContext::Conditional),
        "else-arm must be conditional, got {:?}",
        op_ctx_at_line(&a, 8, None)
    );
}

/// A case arm body is >= conditional.
#[test]
fn case_arm_body_is_conditional() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; i: Integer)\n    begin\n        case i of\n            1:\n                Cust.Insert();\n            2:\n                Cust.Modify();\n        end;\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 7, None).is(ControlContext::Conditional),
        "case arm 1 body must be conditional, got {:?}",
        op_ctx_at_line(&a, 7, None)
    );
    assert!(
        op_ctx_at_line(&a, 9, None).is(ControlContext::Conditional),
        "case arm 2 body must be conditional, got {:?}",
        op_ctx_at_line(&a, 9, None)
    );
}

/// A loop body op is `loop-body`; a top-level op before the loop is `top-level`.
#[test]
fn loop_body_is_loop_body() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        while Cust.Next() <> 0 do\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 5, None).is(ControlContext::TopLevel),
        "pre-loop op must be top-level, got {:?}",
        op_ctx_at_line(&a, 5, None)
    );
    assert!(
        op_ctx_at_line(&a, 7, None).is(ControlContext::LoopBody),
        "loop-body op must be loop-body, got {:?}",
        op_ctx_at_line(&a, 7, None)
    );
}

// ===========================================================================
// lattice max accumulation: if inside a loop stays loop-body
// ===========================================================================

/// An `if` nested INSIDE a loop must keep its body at `loop-body` (lattice
/// `max(loop-body, conditional) = loop-body`), NOT drop back to `conditional`.
/// This is the core "max accumulation" invariant — a regression that resets the
/// context per-construct instead of taking the lattice max would show
/// `conditional` here.
#[test]
fn if_inside_loop_stays_loop_body() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; flag: Boolean)\n    begin\n        if flag then begin\n            Cust.FindSet();\n            while Cust.Next() <> 0 do begin\n                if Cust.IsEmpty() then\n                    Cust.Modify();\n            end;\n        end;\n    end;\n}";
    let a = analyze(src, "P");
    // Inside the if(flag) but before the loop → conditional.
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::Conditional),
        "op inside outer if (pre-loop) must be conditional, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
    // `Cust.IsEmpty()` (line 8) is the inner if's condition leaf, evaluated at
    // the ambient loop-body context → loop-body (NOT conditional, NOT top-level).
    assert!(
        op_ctx_at_line(&a, 8, None).is(ControlContext::LoopBody),
        "inner if-condition leaf inside loop must be loop-body, got {:?}",
        op_ctx_at_line(&a, 8, None)
    );
    // `Cust.Modify()` (line 9) is the inner if's then-body inside the loop. The
    // lattice max of loop-body and conditional is loop-body — must NOT be conditional.
    assert!(
        op_ctx_at_line(&a, 9, None).is(ControlContext::LoopBody),
        "if-body inside loop must stay loop-body (lattice max), got {:?}",
        op_ctx_at_line(&a, 9, None)
    );
}

// ===========================================================================
// error-path: single-arm `if Bad then Error()`
// ===========================================================================

/// A single-arm `if Bad then Error()`: the error branch is `error-path`, and the
/// continuation after the if is UNCHANGED (top-level here, NOT narrowed to
/// conditional and NOT unreachable — the implicit else falls through).
#[test]
fn single_arm_error_branch_is_error_path_continuation_unchanged() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; bad: Boolean)\n    begin\n        if bad then\n            Error('boom');\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    // The Error() call site (line 6) is on the error path.
    assert!(
        callsite_ctx_at_line(&a, 6).is(ControlContext::ErrorPath),
        "Error() call in single-arm guard must be error-path, got {:?}",
        callsite_ctx_at_line(&a, 6)
    );
    // The paired error-call op (line 6) inherits error-path via the post-pass.
    assert!(
        op_ctx_at_line(&a, 6, Some("error-call")).is(ControlContext::ErrorPath),
        "error-call op must inherit error-path, got {:?}",
        op_ctx_at_line(&a, 6, Some("error-call"))
    );
    // The continuation (line 7) is UNCHANGED — top-level, NOT conditional/unreachable.
    assert!(
        op_ctx_at_line(&a, 7, None).is(ControlContext::TopLevel),
        "continuation after single-arm error guard must be UNCHANGED (top-level), got {:?}",
        op_ctx_at_line(&a, 7, None)
    );
}

// ===========================================================================
// unreachable: bare Error() / exit
// ===========================================================================

/// A bare unconditional `Error()` makes all following same-block sites
/// `unreachable`. The Error() itself is at ambient (top-level).
#[test]
fn bare_error_makes_following_unreachable() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        Error('always');\n        Cust.Modify();\n        Commit();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        callsite_ctx_at_line(&a, 5).is(ControlContext::TopLevel),
        "bare Error() call must be at ambient top-level, got {:?}",
        callsite_ctx_at_line(&a, 5)
    );
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::Unreachable),
        "op after bare Error() must be unreachable, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
    assert!(
        op_ctx_at_line(&a, 7, Some("commit")).is(ControlContext::Unreachable),
        "Commit after bare Error() must be unreachable, got {:?}",
        op_ctx_at_line(&a, 7, Some("commit"))
    );
}

/// A bare unconditional `exit` makes following same-block sites `unreachable`.
#[test]
fn bare_exit_makes_following_unreachable() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer)\n    begin\n        exit;\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::Unreachable),
        "op after bare exit must be unreachable, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
}

// ===========================================================================
// unreachable: both-arms-terminating if
// ===========================================================================

/// An `if` whose then AND explicit else both terminate (both `exit`) → the
/// continuation after it is `unreachable`.
#[test]
fn both_arms_terminate_continuation_unreachable() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; flag: Boolean)\n    begin\n        if flag then\n            exit\n        else\n            exit;\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 9, None).is(ControlContext::Unreachable),
        "continuation after both-arms-exit if must be unreachable, got {:?}",
        op_ctx_at_line(&a, 9, None)
    );
}

// ===========================================================================
// case with a terminating arm narrows to conditional, never unreachable
// ===========================================================================

/// A `case` with one terminating arm (`exit`) and one fall-through arm narrows
/// the continuation to `conditional`, NEVER `unreachable` — a case is not proven
/// exhaustive at L2, so some arm may fall through to the continuation.
#[test]
fn case_with_terminating_arm_continuation_conditional_not_unreachable() {
    let src = "codeunit 50100 A\n{\n    procedure P(var Cust: Record Customer; i: Integer)\n    begin\n        case i of\n            1:\n                exit;\n            2:\n                Cust.Insert();\n        end;\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    // Continuation after the case (line 11).
    let cont = op_ctx_at_line(&a, 11, None);
    assert!(
        cont.is(ControlContext::Conditional),
        "continuation after case-with-terminating-arm must narrow to conditional, got {:?}",
        cont
    );
    assert!(
        !cont.is(ControlContext::Unreachable),
        "continuation after case-with-terminating-arm must NEVER be unreachable, got {:?}",
        cont
    );
}

// ===========================================================================
// TryFunction → control-context ABSENT on all sites
// ===========================================================================

/// A `[TryFunction]` routine yields NO control-context on any site (the walker
/// returns empty maps → the field is absent). Every op/callsite must map to None.
#[test]
fn tryfunction_all_contexts_absent() {
    let src = "codeunit 50100 A\n{\n    [TryFunction]\n    procedure TryIt(var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        if Cust.IsEmpty() then\n            Cust.Modify();\n        Commit();\n    end;\n}";
    let a = analyze(src, "TryIt");
    assert!(
        !a.operation_sites.is_empty(),
        "fixture sanity: TryFunction routine should still have operation sites"
    );
    for op in &a.operation_sites {
        let ctx = a.by_operation.get(&op.id).copied();
        assert!(
            ctx.is_none(),
            "TryFunction op `{}` @line {} must have ABSENT context, got {:?}",
            op.kind,
            op.source_anchor.start_line,
            Ctx(ctx)
        );
    }
    for cs in &a.call_sites {
        let ctx = a.by_callsite.get(&cs.id).copied();
        assert!(
            ctx.is_none(),
            "TryFunction call site @line {} must have ABSENT context, got {:?}",
            cs.source_anchor.start_line,
            Ctx(ctx)
        );
    }
}

// ===========================================================================
// IsHandled polarity: upgrades only the TS-recognized region for eligible vars
// ===========================================================================

/// Positive polarity (`if IsHandled then exit;`) on an ELIGIBLE by-var Boolean
/// param upgrades the CONTINUATION (after the guard) to `is-handled-guarded`, and
/// nothing before it.
#[test]
fn ishandled_positive_upgrades_continuation_only() {
    let src = "codeunit 50100 A\n{\n    procedure P(var IsHandled: Boolean; var Cust: Record Customer)\n    begin\n        Cust.FindSet();\n        if IsHandled then\n            exit;\n        Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    // eligibility sanity.
    assert!(
        a.eligibility
            .by_var_bool_params
            .iter()
            .any(|n| n == "ishandled"),
        "IsHandled must be an eligible by-var bool param, got {:?}",
        a.eligibility.by_var_bool_params
    );
    // Before the guard (line 5) → unchanged top-level.
    assert!(
        op_ctx_at_line(&a, 5, None).is(ControlContext::TopLevel),
        "op BEFORE positive guard must stay top-level, got {:?}",
        op_ctx_at_line(&a, 5, None)
    );
    // After the guard (line 8) → upgraded to is-handled-guarded.
    assert!(
        op_ctx_at_line(&a, 8, None).is(ControlContext::IsHandledGuarded),
        "continuation after positive IsHandled guard must be is-handled-guarded, got {:?}",
        op_ctx_at_line(&a, 8, None)
    );
}

/// Negative polarity (`if not IsHandled then <body>`) on an ELIGIBLE var upgrades
/// the BODY of the then-arm to `is-handled-guarded` (not the continuation).
#[test]
fn ishandled_negative_upgrades_body_only() {
    let src = "codeunit 50100 A\n{\n    procedure P(var IsHandled: Boolean; var Cust: Record Customer)\n    begin\n        if not IsHandled then\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        op_ctx_at_line(&a, 6, None).is(ControlContext::IsHandledGuarded),
        "negative IsHandled guard then-body must be is-handled-guarded, got {:?}",
        op_ctx_at_line(&a, 6, None)
    );
}

/// An INELIGIBLE var (a by-VALUE bool param, not by-var) does NOT upgrade: the
/// negative-polarity body stays `conditional`, never `is-handled-guarded`. This is
/// the eligibility-discrimination invariant — only `var` bool params (and
/// published bool vars) qualify.
#[test]
fn ishandled_ineligible_by_value_param_does_not_upgrade() {
    let src = "codeunit 50100 A\n{\n    procedure P(flag: Boolean; var Cust: Record Customer)\n    begin\n        if not flag then\n            Cust.Modify();\n    end;\n}";
    let a = analyze(src, "P");
    assert!(
        a.eligibility.by_var_bool_params.is_empty() && a.eligibility.published_bool_vars.is_empty(),
        "by-value bool param must NOT be eligible, got byVar={:?} pub={:?}",
        a.eligibility.by_var_bool_params,
        a.eligibility.published_bool_vars
    );
    let body = op_ctx_at_line(&a, 6, None);
    assert!(
        body.is(ControlContext::Conditional),
        "ineligible (by-value) negative guard body must stay conditional, got {:?}",
        body
    );
    assert!(
        !body.is(ControlContext::IsHandledGuarded),
        "ineligible var must NEVER upgrade to is-handled-guarded, got {:?}",
        body
    );
    // Lattice sanity: the ineligible body (conditional) strictly outranks both an
    // absent context and top-level — i.e. it landed at conditional, not below.
    assert!(
        body.rank() > Ctx(None).rank(),
        "conditional must outrank absent"
    );
    assert!(
        body.rank() > Ctx(Some(ControlContext::TopLevel)).rank(),
        "conditional must outrank top-level"
    );
}
