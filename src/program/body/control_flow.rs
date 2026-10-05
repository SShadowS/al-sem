//! SHARED L2 control-flow primitives — faithful port of the
//! branch-termination logic in al-sem `src/index/control-context.ts`
//! (`Termination`, `terminates`, `joinTermination`, `branchTermination`,
//! `statementTermination`, `elseTermination`, `hasExplicitElse`).
//!
//! These operate over the R1a CFN skeleton (`PCFNNode`) and are reused by R1c
//! (operation-order) — keep them a pure, side-effect-free port.
//!
//! SOUNDNESS INVARIANT (the false-proof direction): `branch_termination` must
//! NEVER return `Fallthrough` for a branch that PROVABLY always terminates.
//! Under-reporting termination would let a continuation be classified at a lower
//! (less restrictive) context than is sound. The safe direction is to report
//! `Exit`/`Error` whenever we can prove the branch always leaves; `Fallthrough`
//! is reserved for branches that genuinely MAY continue. Loops / `case` / call /
//! op stay `Fallthrough` (the conservative default).

use super::features::PCFNNode;

/// How a branch body terminates with respect to its enclosing routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// The branch always returns from the routine (`exit`).
    Exit,
    /// The branch always raises (`error`).
    Error,
    /// Control MAY run off the end of the branch into the continuation.
    Fallthrough,
}

/// True when a termination value means the branch always leaves the routine.
pub fn terminates(t: Termination) -> bool {
    matches!(t, Termination::Exit | Termination::Error)
}

/// Combine the terminations of two always-terminating arms into the kind of the
/// enclosing construct. `exit` dominates `error` (both exit → exit; any
/// exit+error mix → exit; both error → error). Only called when BOTH terminate.
fn join_termination(a: Termination, b: Termination) -> Termination {
    if a == Termination::Exit || b == Termination::Exit {
        Termination::Exit
    } else {
        Termination::Error
    }
}

fn children_of(node: &PCFNNode) -> &[PCFNNode] {
    node.children.as_deref().unwrap_or(&[])
}

fn else_children_of(node: &PCFNNode) -> &[PCFNNode] {
    node.else_children.as_deref().unwrap_or(&[])
}

/// Determine how a branch body (block node or bare statement) terminates.
///
/// RECURSIVE: walks the block's statements; the FIRST statement that always
/// terminates (recursively) makes the whole block terminate that way. If we run
/// off the end without hitting an always-terminating statement, the body falls
/// through.
pub fn branch_termination(node: &PCFNNode) -> Termination {
    if node.kind == "block" {
        for s in children_of(node) {
            let t = statement_termination(s);
            if terminates(t) {
                return t;
            }
        }
        Termination::Fallthrough
    } else {
        statement_termination(node)
    }
}

/// Termination of a single statement (recursive). Returns `Exit`/`Error` only
/// when the statement PROVABLY always leaves the routine; otherwise fallthrough.
fn statement_termination(node: &PCFNNode) -> Termination {
    match node.kind.as_str() {
        "exit" => Termination::Exit,
        "error" => Termination::Error,
        "block" => branch_termination(node),
        "if" => {
            // An `if` always terminates only when it has an explicit else AND both
            // arms always terminate (no path can fall through).
            if !has_explicit_else(node) {
                return Termination::Fallthrough;
            }
            let then_body = children_of(node).first();
            let else_body = else_children_of(node).first();
            let (Some(then_body), Some(else_body)) = (then_body, else_body) else {
                return Termination::Fallthrough;
            };
            let then_t = branch_termination(then_body);
            let else_t = branch_termination(else_body);
            if terminates(then_t) && terminates(else_t) {
                join_termination(then_t, else_t)
            } else {
                Termination::Fallthrough
            }
        }
        // case / loops / call / op / other — conservatively fall through.
        _ => Termination::Fallthrough,
    }
}

/// The else-arm termination of an `if` node, accounting for an IMPLICIT (absent)
/// else — which always falls through into the continuation.
pub fn else_termination(node: &PCFNNode) -> Termination {
    match else_children_of(node).first() {
        Some(else_body) => branch_termination(else_body),
        None => Termination::Fallthrough,
    }
}

/// True when the `if` has an explicit else-branch in the source
/// (`elseChildren.length > 0`).
pub fn has_explicit_else(node: &PCFNNode) -> bool {
    !else_children_of(node).is_empty()
}
