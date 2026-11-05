//! Site guards: the conditions that enclose each site of a routine (agent-tasks
//! design `docs/superpowers/specs/2026-11-05-agent-tasks-design.md` §9.4, E1).
//!
//! A **guard arm** is one block a condition leads into: the `then` or `else`
//! block of an `if`, one arm (or the `else`) of a `case`, or the body of a
//! loop. A site (a call, a record operation, an `Error`) inside that block
//! runs only when the arm is taken, so the arm's condition is context for
//! "why did this site run". [`SiteGuards::enclosing`] lists, for a site, every
//! arm whose block contains it, innermost first.
//!
//! These are the **enclosing syntactic conditions only**. They are not all the
//! conditions a site needs: an earlier `exit`, a check that stops, a callee's
//! result and the loop's own state all matter too. A reader shows them as
//! selected context and says how many it left out.
//!
//! Two loop conditions mean different things, and the arm says which:
//!
//! - a `while C do` body runs only when `C` holds on entry: a **loop-entry**
//!   condition (as are `for` and `foreach`, which may run no time);
//! - a `repeat … until C` body runs once before `C` is tested, and again only
//!   while `C` is false: a **continuation** condition, not a precondition of
//!   the first run.
//!
//! Built at model assembly, from the routine's IR (only the IR has the
//! condition text), and only when the view declares `ViewNeeds::site_guards`
//! (`crate::program::model::workspace::AssemblyNeeds`): `alsem prove` with a
//! filter (`--match`, `--site`, `--op`, `--table`) declares it, and no other
//! command does.

use al_syntax::ir::{AlFile, BlockId, BlockItem, ExprId, Origin, RoutineDecl, StmtKind};

use super::features::PAnchor;

/// The most characters a guard's condition text keeps (design §9.4); a longer
/// one ends in `…`.
pub const GUARD_TEXT_MAX: usize = 120;

/// Which block a condition leads into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// `if C then <here>`.
    Then,
    /// `if C then … else <here>`: `C` was false.
    Else,
    /// `case X of P1, P2: <here>`.
    CaseArm,
    /// `case X of … else <here>`: `X` matched none of the arms.
    CaseElse,
    /// `while C do <here>`: a loop-entry condition.
    While,
    /// `repeat <here> until C`: a continuation condition (the body runs once
    /// before `C` is tested).
    Repeat,
    /// `for V := A to B do <here>`: a loop-entry condition.
    For,
    /// `foreach V in X do <here>`: a loop-entry condition.
    Foreach,
}

impl GuardKind {
    /// The JSON `kind`: what the condition is to the block.
    #[must_use]
    pub fn role(self) -> &'static str {
        match self {
            GuardKind::Then => "then",
            GuardKind::Else => "else",
            GuardKind::CaseArm => "case-arm",
            GuardKind::CaseElse => "case-else",
            GuardKind::While | GuardKind::For | GuardKind::Foreach => "loop-entry",
            GuardKind::Repeat => "continuation",
        }
    }

    /// The JSON `construct`: the statement the condition belongs to.
    #[must_use]
    pub fn construct(self) -> &'static str {
        match self {
            GuardKind::Then | GuardKind::Else => "if",
            GuardKind::CaseArm | GuardKind::CaseElse => "case",
            GuardKind::While => "while",
            GuardKind::Repeat => "repeat",
            GuardKind::For => "for",
            GuardKind::Foreach => "foreach",
        }
    }
}

/// One guard arm (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardArm {
    pub kind: GuardKind,
    /// The condition as written, whitespace folded, at most
    /// [`GUARD_TEXT_MAX`] characters: an `if`'s or `while`'s condition, a
    /// `repeat`'s `until` condition, a `case`'s scrutinee, a `for`'s
    /// `V := A to B`, a `foreach`'s `V in X`.
    pub text: Box<str>,
    /// A `case` arm's values as written (`P1, P2`); `None` otherwise.
    pub arm: Option<Box<str>>,
    /// The line of the statement the condition belongs to (0-based, as the
    /// model's anchors are).
    pub line: u32,
    /// The block the condition leads into, from its start to its end (line,
    /// column, in the model's anchor coordinates).
    pub from: (u32, u32),
    pub to: (u32, u32),
}

impl GuardArm {
    fn contains(&self, site: &PAnchor) -> bool {
        self.from <= (site.start_line, site.start_column)
            && (site.end_line, site.end_column) <= self.to
    }
}

/// One routine's guard arms, in walk (pre-) order: an arm comes before every
/// arm nested in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteGuards {
    pub arms: Vec<GuardArm>,
}

impl SiteGuards {
    /// The arms whose block contains `site` (an anchor in this routine),
    /// innermost first. The arms containing one site nest, and the walk
    /// lists an arm before the arms inside it, so the innermost is the last.
    #[must_use]
    pub fn enclosing(&self, site: &PAnchor) -> Vec<&GuardArm> {
        let mut v: Vec<&GuardArm> = self.arms.iter().filter(|a| a.contains(site)).collect();
        v.reverse();
        v
    }

    /// The bytes the arms hold on the heap and inline (a measurement aid).
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.arms.capacity() * std::mem::size_of::<GuardArm>()
            + self
                .arms
                .iter()
                .map(|a| a.text.len() + a.arm.as_ref().map_or(0, |s| s.len()))
                .sum::<usize>()
    }
}

/// `text` with every run of whitespace folded to one space, at most
/// [`GUARD_TEXT_MAX`] characters (a longer one keeps the first
/// `GUARD_TEXT_MAX - 1` and ends in `…`).
#[must_use]
pub fn fold_text(text: &str) -> Box<str> {
    let folded: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= GUARD_TEXT_MAX {
        return folded.into_boxed_str();
    }
    let mut s: String = folded.chars().take(GUARD_TEXT_MAX - 1).collect();
    s.push('…');
    s.into_boxed_str()
}

struct Walker<'a> {
    file: &'a AlFile,
    source: &'a str,
    anchor: &'a dyn Fn(&Origin) -> PAnchor,
    arms: Vec<GuardArm>,
}

impl Walker<'_> {
    fn src(&self, e: ExprId) -> &str {
        let o = &self.file.ir.expr(e).origin;
        self.source.get(o.byte.clone()).unwrap_or("")
    }

    fn push(
        &mut self,
        kind: GuardKind,
        text: Box<str>,
        arm: Option<Box<str>>,
        line: u32,
        b: BlockId,
    ) {
        let a = (self.anchor)(&self.file.ir.block(b).origin);
        self.arms.push(GuardArm {
            kind,
            text,
            arm,
            line,
            from: (a.start_line, a.start_column),
            to: (a.end_line, a.end_column),
        });
    }

    fn block(&mut self, b: BlockId) {
        let items: Vec<(Option<al_syntax::ir::StmtId>, Vec<BlockId>)> = self
            .file
            .ir
            .block(b)
            .items
            .iter()
            .map(|it| match it {
                BlockItem::Stmt(s) => (Some(*s), Vec::new()),
                BlockItem::Preproc(g) => (None, g.branches.clone()),
            })
            .collect();
        for (stmt, branches) in items {
            match stmt {
                Some(s) => self.stmt(s),
                // An `#if` arm is no runtime condition: walked through.
                None => {
                    for br in branches {
                        self.block(br);
                    }
                }
            }
        }
    }

    fn stmt(&mut self, s: al_syntax::ir::StmtId) {
        let stmt = self.file.ir.stmt(s);
        let line = stmt.origin.start.row;
        match &stmt.kind {
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let (cond, then_block, else_block) = (*cond, *then_block, *else_block);
                let text = fold_text(self.src(cond));
                self.push(GuardKind::Then, text.clone(), None, line, then_block);
                self.block(then_block);
                if let Some(e) = else_block {
                    self.push(GuardKind::Else, text, None, line, e);
                    self.block(e);
                }
            }
            StmtKind::Case {
                scrutinee,
                branches,
                else_block,
            } => {
                let text = fold_text(self.src(*scrutinee));
                let arms: Vec<(Box<str>, BlockId)> = branches
                    .iter()
                    .map(|b| {
                        let pats: Vec<&str> =
                            b.patterns.iter().map(|p| self.src(*p).trim()).collect();
                        (fold_text(&pats.join(", ")), b.body)
                    })
                    .collect();
                let else_block = *else_block;
                for (pats, body) in arms {
                    self.push(GuardKind::CaseArm, text.clone(), Some(pats), line, body);
                    self.block(body);
                }
                if let Some(e) = else_block {
                    self.push(GuardKind::CaseElse, text, None, line, e);
                    self.block(e);
                }
            }
            StmtKind::While { cond, body } => {
                let (cond, body) = (*cond, *body);
                let text = fold_text(self.src(cond));
                self.push(GuardKind::While, text, None, line, body);
                self.block(body);
            }
            StmtKind::Repeat { body, until } => {
                let (body, until) = (*body, *until);
                let text = fold_text(self.src(until));
                self.push(GuardKind::Repeat, text, None, line, body);
                self.block(body);
            }
            StmtKind::For {
                var,
                from,
                to,
                down,
                body,
            } => {
                let (body, dir) = (*body, if *down { "downto" } else { "to" });
                let text = fold_text(&format!(
                    "{} := {} {dir} {}",
                    self.src(*var).trim(),
                    self.src(*from).trim(),
                    self.src(*to).trim()
                ));
                self.push(GuardKind::For, text, None, line, body);
                self.block(body);
            }
            StmtKind::Foreach {
                var,
                iterable,
                body,
            } => {
                let body = *body;
                let text = fold_text(&format!(
                    "{} in {}",
                    self.src(*var).trim(),
                    self.src(*iterable).trim()
                ));
                self.push(GuardKind::Foreach, text, None, line, body);
                self.block(body);
            }
            StmtKind::With { body, .. } | StmtKind::AssertError(body) | StmtKind::Block(body) => {
                let body = *body;
                self.block(body);
            }
            StmtKind::Try { body, catch_block } => {
                let (body, catch_block) = (*body, *catch_block);
                self.block(body);
                if let Some(c) = catch_block {
                    self.block(c);
                }
            }
            StmtKind::Assignment { .. }
            | StmtKind::Call(_)
            | StmtKind::Exit(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Unknown => {}
        }
    }
}

/// The guard arms of `routine` (module doc). `anchor` gives an IR origin's
/// anchor as the model spells anchors, so the arms compare with the sites'.
pub fn build_site_guards(
    file: &AlFile,
    routine: &RoutineDecl,
    source: &str,
    anchor: &dyn Fn(&Origin) -> PAnchor,
) -> SiteGuards {
    let Some(body) = routine.body else {
        return SiteGuards::default();
    };
    let mut w = Walker {
        file,
        source,
        anchor,
        arms: Vec::new(),
    };
    w.block(body);
    w.arms.shrink_to_fit();
    SiteGuards { arms: w.arms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_text_folds_whitespace_and_caps_the_length() {
        assert_eq!(&*fold_text("a  and\n    b"), "a and b");
        let long = "x".repeat(200);
        let t = fold_text(&long);
        assert_eq!(t.chars().count(), GUARD_TEXT_MAX);
        assert!(t.ends_with('…'));
        assert_eq!(
            &*fold_text(&"y".repeat(GUARD_TEXT_MAX)),
            "y".repeat(GUARD_TEXT_MAX)
        );
    }
}
