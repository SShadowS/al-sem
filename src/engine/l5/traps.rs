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
//! - **TryFunction with its result used** ([`TrapKind::TryFunction`]): a call
//!   to a `[TryFunction]` routine that is not a statement of its own. Learn
//!   ("Handling errors using try methods"): "If a try method call doesn't
//!   use the return value, the try method operates like an ordinary method,
//!   and errors are exposed as usual"; "If the return variable for a call to
//!   a function, which is attributed with [TryFunction] isn't used, then the
//!   call isn't considered a try function call". Learn names `OK :=` and
//!   `if … then` as uses; any other use of the value (an argument, `exit(…)`)
//!   is a use too. **A TryFunction does not roll back the database changes
//!   made before the error** (same page); nothing here says it does.
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
//! Not a trap: **`[ErrorBehavior(ErrorBehavior::Collect)]`** (L30). Learn
//! ("Collecting errors", "Collectible errors API"): only errors raised as a
//! collectible `ErrorInfo` are collected, the code goes on after them, and
//! "if any errors are present in the collected list when a procedure ends,
//! the user receives an error dialog" unless the code clears them with
//! `ClearCollectedErrors`. So a collected error still escapes, later; a
//! forward answer states it ([`collects_errors`]) and keeps the error.

use crate::program::body::features::{PCallSite, PCallee};
use crate::program::model::workspace::ModelRoutine;

/// The kind of a trap (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TrapKind {
    CheckedRun,
    TryFunction,
    Asserterror,
    IsolatedEvent,
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
    checked_run(cs, TrapCallee::Routine(callee))
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
            method.eq_ignore_ascii_case("run")
                && !cs.in_statement_position
                && match callee {
                    TrapCallee::Object { codeunit } => codeunit,
                    TrapCallee::Routine(c) => c.is_some_and(|c| {
                        c.kind == "trigger" && c.name.eq_ignore_ascii_case("OnRun")
                    }),
                }
        }
        _ => false,
    }
}

/// Whether `r` is declared `[TryFunction]`.
#[must_use]
pub(crate) fn is_try_function(r: &ModelRoutine) -> bool {
    crate::program::attributes::has_attribute(&r.attributes_parsed, "TryFunction")
}

/// The trap a synchronous call `cs` into `callee` sets (module doc), the
/// asserterror one first; `None` for an ordinary call. A TryFunction called
/// as a statement is an ordinary call.
#[must_use]
pub(crate) fn call_trap(cs: &PCallSite, callee: TrapCallee<'_>) -> Option<TrapKind> {
    if cs.under_asserterror == Some(true) {
        return Some(TrapKind::Asserterror);
    }
    if checked_run(cs, callee) {
        return Some(TrapKind::CheckedRun);
    }
    let into_try = matches!(callee, TrapCallee::Routine(Some(c)) if is_try_function(c));
    (into_try && !cs.in_statement_position).then_some(TrapKind::TryFunction)
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
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("collect"))
    })
}
