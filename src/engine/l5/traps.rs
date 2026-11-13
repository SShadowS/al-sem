//! **Traps** (engine limits B7/L23, L30): the one rule for where an error
//! raised below a call stops travelling up. Both directions read it: the
//! callers' question (`callers-intercept-errors`,
//! [`crate::engine::l5::callers_context`]) walks UP from a routine and asks
//! whether some caller traps its error; the forward error answers (`prove …
//! throws-error`, explain's and digest's error lines, through the literal
//! pass in [`crate::engine::l5::literal_reach`]) walk DOWN from a root and
//! drop an error every route to which crosses a trap.
//!
//! A trap, per call edge `caller --site--> callee` (Microsoft Learn):
//! - **checked run** ([`TrapKind::CheckedRun`]): `if Codeunit.Run(…)`,
//!   `Ok := Codeunit.Run(…)`, or `MyCodeunit.Run()` on a codeunit variable
//!   with its result used. The error becomes `false` and the run's own
//!   transaction is rolled back ("Codeunit.Run(Integer [, var Record])
//!   Method": "If you omit this optional return value and the operation does
//!   not execute successfully, a runtime error will occur"; "Transaction
//!   semantics": changes are committed at the end of the codeunit unless an
//!   error occurs). For a codeunit variable's `.Run`, the target must be the
//!   `OnRun` trigger (a user procedure named `Run` is an ordinary call).
//!   **A checked run needs a clean transaction**: the same page says "If
//!   you're already in a transaction you must commit first before calling
//!   Codeunit.Run"; a checked run inside an open write transaction is itself
//!   a run-time error, raised at the run. So the forward answers count a
//!   checked run as a definite trap only where no write may be pending at it
//!   on any route from the root ([`crate::engine::l5::literal_reach`],
//!   `LiteralReach::soft_checked_runs`); where one may, the error escapes
//!   (`escapes-if-write-pending`), and a caught ruling states that it
//!   assumes the root's own callers have no write pending
//!   ([`entry_assumption`], in prove, explain and digest alike).
//!   A `Commit()` before the run ends the write only when it runs normally
//!   on every route from the root (B4's `[CommitBehavior]` rule,
//!   [`crate::engine::l5::commit_scope::CbGraph::commit_surely_runs`]).
//!   Learn ("CommitBehavior attribute"): under `Ignore` "this commit is
//!   ignored", so the write stays pending; under `Error` "this commit causes
//!   an error", so it commits nothing either. When a run counts as clean
//!   only because such a commit ends the write, the ruling also assumes the
//!   root's callers do not call it under either behavior.
//! - **TryFunction with its result used** ([`TrapKind::TryFunction`]): a call
//!   to a `[TryFunction]` routine whose value is tested
//!   ([`PCallSite::result_tested`]: the whole right-hand side of `Ok := …`,
//!   or the whole condition of an `if`, `while` or `until`, through
//!   parentheses and `not`). Learn ("Handling errors using try methods"):
//!   "If a try method call doesn't use the return value, the try method
//!   operates like an ordinary method, and errors are exposed as usual";
//!   "If a try method call uses the return value in an `OK:=` statement or
//!   a conditional statement such as `if-then`, errors are caught". Learn
//!   names `OK :=` and `if-then`; reading a `while` or `until` condition as
//!   "a conditional statement such as `if-then`", and `B := not TryIt()` as
//!   an assignment of the value, goes slightly beyond the named shapes and
//!   was not checked on BC. Learn names no other shape, and its one argument example
//!   (`DoWork(DoInsert(Rec)); // Allowed as the DoWork return value isn't
//!   used so it's not a try function`) does not establish that a try call
//!   used as an argument traps. So an argument, `exit(TryIt())` or a part of
//!   `a and TryIt()` is counted as an ordinary call: an error there is
//!   reported as escaping (it may be caught at run time; never a false
//!   "no"). **A TryFunction does not roll back the database changes made
//!   before the error** (same page); nothing here says it does. Two points
//!   are not modeled. (1) On premises with `DisableWriteInsideTryFunctions`
//!   (the default there) a write inside a try method is itself a run-time
//!   error. The answers do not add it, and Learn does not say whether the
//!   try call catches it; if it does not, `throws-error` under-reports it,
//!   so this is NOT known to be safe (engine limits L23, left). (2) Learn
//!   says "Arguments for a try function are evaluated inside the try
//!   function", so an error in such an argument is caught too, while the
//!   answers report it as escaping (safe: an over-report).
//! - **asserterror** ([`TrapKind::Asserterror`]): a call or record operation
//!   under `asserterror` (a test expects the error).
//! - **isolated event** ([`TrapKind::IsolatedEvent`]): the dispatch from the
//!   publisher of an isolated event (`[IntegrationEvent(_, _, true)]`,
//!   `[BusinessEvent(_, true)]`, `[InternalEvent(_, true)]`) to a
//!   subscriber. Learn ("Isolated events in AL"): each subscriber runs in a
//!   transaction of its own, an error rolls it back and the publisher goes
//!   on, BUT "write transactions should explicitly be committed before
//!   invoking an isolated event. Otherwise, the isolated event will be
//!   invoked like an normal event, that is, errors inside an event
//!   subscriber will cause the entire operation to fail", and during an
//!   extension's install, uninstall or upgrade the events run normally. So
//!   this trap is CONDITIONAL ([`TrapKind::definite`] is false): it never
//!   removes an error from a forward answer, it only states the condition
//!   under which the error escapes.
//!
//! One more checked-run test stays apart: the transaction spans' commit
//! boundary (`transaction_spans::is_checked_codeunit_run`) counts only
//! `Codeunit.Run(…)` with its result used, not a codeunit variable's `.Run`.
//! It answers a different question (where a transaction ends), and moving
//! it onto this rule would change the commit answers; that is left to a
//! change of its own (engine limits B7, what is left).
//!
//! Not a trap: **`[ErrorBehavior(ErrorBehavior::Collect)]`** (L30). Learn
//! ("Collecting errors", "Collectible errors API"): only errors raised as a
//! collectible `ErrorInfo` are collected, the code goes on after them, and
//! "if any errors are present in the collected list when a procedure ends,
//! the user receives an error dialog" unless the code clears them with
//! `ClearCollectedErrors`. So a collected error still escapes, later; a
//! forward answer states it ([`collects_errors`]) and keeps the error.

use crate::program::body::features::{PCallSite, PCallee};
use crate::program::model::workspace::ModelRoutine;
use al_syntax::IdentifierFoldExt;

/// The kind of a trap (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TrapKind {
    CheckedRun,
    TryFunction,
    Asserterror,
    IsolatedEvent,
    /// ENG-22: not a trap of the call edges: the clean-transaction check of
    /// a checked run, which raises the run's own error when a write is
    /// pending at it. Only a ruling of that error names it (never an entry
    /// of the trap table).
    RunCheck,
    /// ENG-10: not a trap of the call edges: the server's refusal of a
    /// database write inside a try call (on premises,
    /// `DisableWriteInsideTryFunctions`), which raises the try call's own
    /// error. Only a ruling of that error names it.
    TryRefusal,
}

impl TrapKind {
    /// Whether the trap catches every error that reaches it. An isolated
    /// event's does not when a write is pending (module doc).
    #[must_use]
    pub(crate) fn definite(self) -> bool {
        !matches!(self, TrapKind::IsolatedEvent)
    }

    /// The trap in words, as the answers print it.
    #[must_use]
    pub(crate) fn words(self) -> &'static str {
        match self {
            TrapKind::CheckedRun => "checked Codeunit.Run",
            TrapKind::TryFunction => "TryFunction with its result used",
            TrapKind::Asserterror => "asserterror",
            TrapKind::IsolatedEvent => "isolated event",
            TrapKind::RunCheck => "clean-transaction check of the checked Codeunit.Run",
            TrapKind::TryRefusal => "database-write refusal of the try call",
        }
    }

    /// The JSON word.
    #[must_use]
    pub(crate) fn id(self) -> &'static str {
        match self {
            TrapKind::CheckedRun => "checked-run",
            TrapKind::TryFunction => "try-function",
            TrapKind::Asserterror => "asserterror",
            TrapKind::IsolatedEvent => "isolated-event",
            TrapKind::RunCheck => "checked-run-pending-write",
            TrapKind::TryRefusal => "try-write-refused",
        }
    }
}

/// What a call edge enters: a routine row (when known), or (the callers'
/// sited graph) an object node, with whether it is a codeunit.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TrapCallee<'a> {
    Routine(Option<&'a ModelRoutine>),
    Object { codeunit: bool },
}

/// A checked run (module doc): the call runs a codeunit in its OWN
/// transaction and turns its error into `false`.
#[must_use]
pub(crate) fn is_checked_run(cs: &PCallSite, callee: Option<&ModelRoutine>) -> bool {
    // ENG-24 (sol round 2): the resolver's decision (`PCallSite::checked_run`),
    // which also covers a codeunit variable's `Ok := C.Run()` whose `OnRun` has
    // no row (none declared, or a symbol-only dependency's): with no callee
    // the member form below cannot tell.
    cs.checked_run || checked_run(cs, TrapCallee::Routine(callee))
}

/// [`is_checked_run`] over either callee shape.
#[must_use]
pub(crate) fn checked_run(cs: &PCallSite, callee: TrapCallee<'_>) -> bool {
    match &cs.callee {
        PCallee::ObjectRun { object_kind, .. } => {
            let codeunit = match callee {
                TrapCallee::Object { codeunit } => codeunit,
                TrapCallee::Routine(_) => object_kind == "Codeunit",
            };
            codeunit && cs.object_run_return_used == Some(true)
        }
        PCallee::Member { method, .. } => {
            method.eq_fold_identifier("run")
                && !cs.in_statement_position
                && match callee {
                    TrapCallee::Object { codeunit } => codeunit,
                    TrapCallee::Routine(c) => {
                        c.is_some_and(|c| c.kind == "trigger" && c.name.eq_fold_identifier("OnRun"))
                    }
                }
        }
        _ => false,
    }
}

/// Whether every build of `r` declares it `[TryFunction]`. A `[TryFunction]`
/// only some `#if` arms give it (engine limits B2) traps nothing for certain:
/// a build without it lets the error escape, so the call is an ordinary one.
#[must_use]
pub(crate) fn is_try_function(r: &ModelRoutine) -> bool {
    crate::program::attributes::has_definite_attribute(&r.attributes_parsed, "TryFunction")
}

/// The trap a synchronous call `cs` into `callee` sets (module doc), the
/// asserterror one first; `None` for an ordinary call. A TryFunction call
/// traps only in a shape Learn names ([`PCallSite::result_tested`]); called
/// as a statement, or with its value used any other way (an argument,
/// `exit(…)`), it is counted as an ordinary call.
#[must_use]
pub(crate) fn call_trap(cs: &PCallSite, callee: TrapCallee<'_>) -> Option<TrapKind> {
    if cs.under_asserterror == Some(true) {
        return Some(TrapKind::Asserterror);
    }
    if checked_run(cs, callee) {
        return Some(TrapKind::CheckedRun);
    }
    let into_try = matches!(callee, TrapCallee::Routine(Some(c)) if is_try_function(c));
    (into_try && cs.result_tested).then_some(TrapKind::TryFunction)
}

/// The assumption a caught ruling rests on when a checked run may be its
/// trap (module doc), as every forward error answer states it: prove's
/// assumptions, explain's and digest's header and caught line. `None` when
/// no caught ruling needs it. `normal_commit`: the run is clean only because
/// a `Commit()` before it ends the write.
#[must_use]
pub(crate) fn entry_assumption(clean_entry: bool, normal_commit: bool) -> Option<String> {
    if !clean_entry {
        return None;
    }
    Some(if normal_commit {
        "assumes the routine's callers have no write pending when they call it and do not call \
         it under [CommitBehavior(Ignore)] or [CommitBehavior(Error)]: an error left out as \
         caught may rest on a checked Codeunit.Run, which is itself a run-time error inside a \
         write transaction, and on a Commit() before it that ends the write"
            .to_string()
    } else {
        "assumes the routine's callers have no write pending when they call it: an error left \
         out as caught may rest on a checked Codeunit.Run, and a checked run inside a write \
         transaction is itself a run-time error"
            .to_string()
    })
}

/// ENG-10: the assumption every forward error answer of a root that is
/// itself a `[TryFunction]` and may run a refused write rests on (prove's
/// assumptions, explain's and digest's header). `None` otherwise.
#[must_use]
pub(crate) fn try_root_assumption(try_root_refusal: bool) -> Option<String> {
    try_root_refusal.then(|| {
        "assumes the routine's callers call it as a statement: it is a [TryFunction], and a \
         caller that uses its return value gets an error no try call catches when the server \
         refuses a database write, LockTable or Commit it runs (on premises by default)"
            .to_string()
    })
}

/// L30: whether `r` collects errors (`[ErrorBehavior(ErrorBehavior::Collect)]`):
/// a collectible error raised under it is collected and raised when the
/// collecting procedure ends, unless the code clears it (module doc).
#[must_use]
pub(crate) fn collects_errors(r: &ModelRoutine) -> bool {
    let Some(a) = crate::program::attributes::find_attribute(&r.attributes_parsed, "ErrorBehavior")
    else {
        return false;
    };
    a.args.first().is_some_and(|arg| {
        arg.member
            .as_deref()
            .or(arg.value.as_deref())
            .unwrap_or(arg.text.as_str())
            .rsplit([':', '.'])
            .next()
            .is_some_and(|m| m.trim().eq_fold_identifier("collect"))
    })
}
