//! **Static background starts** (callgraph gap F3; spec
//! `2026-10-13-callgraph-export-design.md` §9): the platform methods that run
//! a codeunit in a new session or task, and which of their arguments name it.
//!
//! | Method | Catalog id | Codeunit argument | Failure codeunit |
//! |---|---|---|---|
//! | `StartSession(var SessionId, CodeunitId, …)` | `startsession` | 1 | |
//! | `Session.StartSession(var SessionId, CodeunitId, …)` | `Session::startsession` | 1 | |
//! | `TaskScheduler.CreateTask(CodeunitId, FailureCodeunitId, …)` | `TaskScheduler::createtask` | 0 | 1 |
//! | `CurrPage.EnqueueBackgroundTask(var TaskId, CodeunitId, …)` | `PageInstance::enqueuebackgroundtask` | 1 | |
//!
//! A codeunit argument is **literal** when it is `Codeunit::X`,
//! `Codeunit::"X"` or an integer (`50100`; the transpiled bodies of symbol
//! packages spell every start that way). A literal start gets an
//! [`EdgeKind::BackgroundStart`](super::edge::EdgeKind::BackgroundStart)
//! edge to the codeunit's `OnRun` beside the site's catalog call. Any other
//! argument (a variable, an expression) names no target: the site keeps its
//! open `Run{codeunit}` hub (the export's `h:run:codeunit`), the honest "any
//! codeunit". A failure codeunit of `0` means none.
//!
//! The resolver classifies a site by the catalog id its call resolved to
//! ([`starter_of_builtin`]), so a workspace procedure that happens to be
//! named `StartSession` is never taken for one. The body facts have no
//! catalog id; their readers (the capability `start` facts, the callers'
//! context questions) classify by the callee's spelling
//! ([`starter_of_callee`]), from the same table.
//!
//! **Semantics.** The started codeunit runs in its own session or task: it
//! shares no transaction with the starter (its writes are not the starter's
//! pending writes and a `Commit` there commits nothing of the starter's),
//! it has no UI, and an error there does not propagate back to the starter.
//! Effects found beyond a background start are therefore never the
//! starter's own commits, UI or errors.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, ExprId, ExprKind, Literal};

use super::edge::BuiltinId;

/// One platform method that starts a codeunit in the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Starter {
    /// The catalog id a call of it resolves to.
    pub builtin: &'static str,
    /// The receiver as written, folded (`None`: a bare call).
    pub receiver: Option<&'static str>,
    /// The method, folded.
    pub method: &'static str,
    /// The index of the argument naming the started codeunit.
    pub codeunit_arg: usize,
    /// The index of `TaskScheduler.CreateTask`'s failure codeunit, which runs
    /// (in the task's session) only when the task fails.
    pub failure_arg: Option<usize>,
}

/// Every background start AL has (see the module doc's table).
pub const STARTERS: &[Starter] = &[
    Starter {
        builtin: "startsession",
        receiver: None,
        method: "startsession",
        codeunit_arg: 1,
        failure_arg: None,
    },
    Starter {
        builtin: "Session::startsession",
        receiver: Some("session"),
        method: "startsession",
        codeunit_arg: 1,
        failure_arg: None,
    },
    Starter {
        builtin: "TaskScheduler::createtask",
        receiver: Some("taskscheduler"),
        method: "createtask",
        codeunit_arg: 0,
        failure_arg: Some(1),
    },
    Starter {
        builtin: "PageInstance::enqueuebackgroundtask",
        receiver: Some("currpage"),
        method: "enqueuebackgroundtask",
        codeunit_arg: 1,
        failure_arg: None,
    },
];

/// The starter a call resolved to this catalog builtin is.
#[must_use]
pub fn starter_of_builtin(id: &BuiltinId) -> Option<&'static Starter> {
    STARTERS.iter().find(|s| s.builtin == id.0)
}

/// The starter a call spelled `receiver.method(...)` (or bare `method(...)`)
/// is, by name alone (for body facts, which carry no catalog id).
#[must_use]
pub fn starter_of_callee(receiver: Option<&str>, method: &str) -> Option<&'static Starter> {
    let m = method.fold_identifier();
    let r = receiver.map(|r| r.fold_identifier());
    STARTERS
        .iter()
        .find(|s| s.method == m && s.receiver == r.as_deref())
}

/// What one codeunit argument of a start names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartTarget {
    /// A literal codeunit: its reference text (a name, quotes stripped, or a
    /// number) and whether it is a name.
    Literal { target_ref: String, is_name: bool },
    /// No codeunit: an absent optional argument, or a failure codeunit of `0`.
    None,
    /// Not a literal: the target is known only at run time.
    Dynamic,
}

/// What argument `arg` of a start names. `failure`: the argument is the
/// failure codeunit, where `0` (and an absent argument) means none.
#[must_use]
pub fn start_target(file: &AlFile, args: &[ExprId], arg: usize, failure: bool) -> StartTarget {
    let Some(&e) = args.get(arg) else {
        return if failure {
            StartTarget::None
        } else {
            StartTarget::Dynamic
        };
    };
    match &file.ir.expr(e).kind {
        ExprKind::DatabaseReference(text) => {
            let Some((kind, name)) = text.split_once("::") else {
                return StartTarget::Dynamic;
            };
            if !kind.trim().eq_ignore_ascii_case("codeunit") {
                return StartTarget::Dynamic;
            }
            let name = name.trim();
            if let Some(q) = name.strip_prefix('"') {
                StartTarget::Literal {
                    target_ref: q.strip_suffix('"').unwrap_or(q).to_string(),
                    is_name: true,
                }
            } else {
                StartTarget::Literal {
                    target_ref: name.to_string(),
                    is_name: name.parse::<i64>().is_err(),
                }
            }
        }
        ExprKind::Literal(Literal::Int(n)) => match n.trim().parse::<i64>() {
            Ok(0) if failure => StartTarget::None,
            Ok(n) if n > 0 => StartTarget::Literal {
                target_ref: n.to_string(),
                is_name: false,
            },
            _ => StartTarget::Dynamic,
        },
        _ => StartTarget::Dynamic,
    }
}

/// [`start_target`] over a body fact's argument (`PCallSite::argument_infos`,
/// which carry no IR): the same three answers from the argument's kind and
/// text.
#[must_use]
pub fn start_target_of_info(
    info: Option<&crate::program::body::features::PExpressionInfo>,
    failure: bool,
) -> StartTarget {
    let Some(info) = info else {
        return if failure {
            StartTarget::None
        } else {
            StartTarget::Dynamic
        };
    };
    match info.kind.as_str() {
        "database_reference"
            if info
                .qualifier
                .as_deref()
                .is_some_and(|q| q.trim().eq_ignore_ascii_case("codeunit")) =>
        {
            let Some(name) = info.member.as_deref().map(str::trim) else {
                return StartTarget::Dynamic;
            };
            StartTarget::Literal {
                target_ref: name.to_string(),
                is_name: name.parse::<i64>().is_err(),
            }
        }
        "integer" => match info.text.trim().parse::<i64>() {
            Ok(0) if failure => StartTarget::None,
            Ok(n) if n > 0 => StartTarget::Literal {
                target_ref: n.to_string(),
                is_name: false,
            },
            _ => StartTarget::Dynamic,
        },
        _ => StartTarget::Dynamic,
    }
}

/// [`start_targets`] over a body fact's arguments ([`start_target_of_info`]).
#[must_use]
pub fn start_targets_of_infos(
    s: &Starter,
    infos: &[crate::program::body::features::PExpressionInfo],
) -> (Vec<(StartTarget, bool)>, bool) {
    let mut out = vec![(
        start_target_of_info(infos.get(s.codeunit_arg), false),
        false,
    )];
    if let Some(f) = s.failure_arg {
        out.push((start_target_of_info(infos.get(f), true), true));
    }
    let dynamic = out.iter().any(|(t, _)| *t == StartTarget::Dynamic);
    out.retain(|(t, _)| matches!(t, StartTarget::Literal { .. }));
    (out, dynamic)
}

/// The literal codeunits a start names (codeunit argument first, then the
/// failure codeunit), and whether any of its codeunit arguments is dynamic.
#[must_use]
pub fn start_targets(
    s: &Starter,
    file: &AlFile,
    args: &[ExprId],
) -> (Vec<(StartTarget, bool)>, bool) {
    let mut out = vec![(start_target(file, args, s.codeunit_arg, false), false)];
    if let Some(f) = s.failure_arg {
        out.push((start_target(file, args, f, true), true));
    }
    let dynamic = out.iter().any(|(t, _)| *t == StartTarget::Dynamic);
    out.retain(|(t, _)| matches!(t, StartTarget::Literal { .. }));
    (out, dynamic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_is_a_real_catalog_id() {
        use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin_id};
        use crate::program::resolve::receiver::FrameworkKind;
        for s in STARTERS {
            match s.builtin.split_once("::") {
                None => assert!(
                    crate::program::resolve::global_builtins::GLOBAL_BUILTIN_METHODS
                        .contains(s.builtin),
                    "{} is not a global builtin",
                    s.builtin
                ),
                Some((prefix, method)) => {
                    let kind = match prefix {
                        "Session" => FrameworkKind::Session,
                        "TaskScheduler" => FrameworkKind::TaskScheduler,
                        "PageInstance" => FrameworkKind::PageInstance,
                        other => panic!("unexpected prefix {other}"),
                    };
                    assert_eq!(
                        member_builtin_id(MemberCatalogKind::Framework(&kind), method).map(|b| b.0),
                        Some(s.builtin.to_string())
                    );
                }
            }
            assert_eq!(starter_of_builtin(&BuiltinId(s.builtin.into())), Some(s));
            assert_eq!(starter_of_callee(s.receiver, s.method), Some(s));
        }
        // Spelling is folded; an unknown receiver is no starter.
        assert!(starter_of_callee(Some("SESSION"), "StartSession").is_some());
        assert!(starter_of_callee(Some("MyCodeunit"), "StartSession").is_none());
        assert!(starter_of_callee(None, "CreateTask").is_none());
    }
}
