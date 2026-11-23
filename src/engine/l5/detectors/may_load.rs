//! ENG-177: whether a record variable MAY be loaded where a record operation runs, path by path.
//!
//! d11 (Modify without Get) and d21 (read without load) report an operation on a record that was
//! never loaded before it. Their textual signals (a load operation, a loading call or an
//! assignment from a loaded record anywhere before the operation) say a load happened somewhere
//! earlier, not that it reaches the operation: `A.Get(1); A := N; A.Modify()` and
//! `if B then begin A.Get(1); exit; end; A.Modify()` loaded `A` on no path that reaches the
//! `Modify`. This walk follows the statement tree ([`PCFNNode`]) and answers whether SOME path
//! reaching the operation ends with the record loaded. The detectors stay silent only when both
//! say so, so the walk can only add findings, never drop one.
//!
//! The state per path is one flag per tracked variable: may it be loaded here? Joins are OR (a
//! "may" question); a path that ends (`exit`, a terminating `Error`, `break`, `continue`) joins
//! nothing after it. Variables tracked: the operation's record and every variable a
//! whole-variable assignment copies into a tracked one. At entry a LOCAL record is not loaded;
//! every other variable (a parameter, a global, the implicit `Rec`, an unknown scope) may be,
//! because its state is the caller's or another routine's ("unknown is not no").
//!
//! Effects: a recognised load operation or loading call textually before the operation loads its
//! record (the same position rule as the detectors' own signals: a load later in a loop body
//! does not count). A whole-variable assignment `A := X` gives `A` the state of `X`; from any
//! other value (a call result, an expression) `A` may be loaded. `Clear(A)` unloads `A`. A field
//! write or an element write changes nothing. Anything the walk cannot read (no statement tree,
//! the operation not in it, a `try`, a pathologically deep tree, a loop that does not settle)
//! answers "may be loaded": the detectors' textual answer stands.

use std::collections::HashMap;

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{RECORD_LOAD_OPS, before_anchor, call_site_loads};
use crate::program::body::features::{
    FlowFact, PAnchor, PCFNNode, PCallSite, PCallee, PVarAssignment,
};
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};

/// Deeper than any real routine; a deeper tree answers "may be loaded".
const MAX_DEPTH: usize = 512;
/// Rounds of a loop's fixed point. The state only grows (OR), so it settles within one round per
/// tracked variable plus one; a loop that has not settled by then answers "may be loaded".
const MAX_LOOP_ROUNDS: usize = 64;

/// One flag per tracked variable (may it be loaded?); `None` when no path reaches the point.
type State = Option<Vec<bool>>;

fn join(a: State, b: &State) -> State {
    match (a, b) {
        (None, b) => b.clone(),
        (a, None) => a,
        (Some(mut a), Some(b)) => {
            for (x, y) in a.iter_mut().zip(b) {
                *x |= *y;
            }
            Some(a)
        }
    }
}

/// May the record of `op` be loaded where `op` runs, on some path that reaches it? `true` when
/// the walk cannot tell (see the module doc).
pub(crate) fn record_may_be_loaded_at(
    routine: &ModelRoutine,
    ctx: &DetectorContext,
    op: &ModelRecordOperation,
) -> bool {
    let Some(tree) = routine.statement_tree.as_deref() else {
        return true;
    };
    let target = op.record_variable_name.fold_identifier();
    // The tracked variables: the target, then (closing over whole-variable copies) every variable
    // copied into a tracked one.
    let mut vars = vec![target];
    let mut i = 0;
    while i < vars.len() {
        for a in &routine.var_assignments {
            if let Some(r) = &a.rhs_identifier
                && a.lhs_name == vars[i]
                && !vars.contains(r)
            {
                vars.push(r.clone());
            }
        }
        i += 1;
    }
    let entry = vars
        .iter()
        .map(|v| {
            // The routine's own declared locals (record rows carry no scope; the variable list
            // does).
            !routine.variables.own().iter().any(|mv| {
                !mv.is_parameter
                    && mv.scope.as_deref() == Some("local")
                    && mv.name.fold_identifier() == *v
            })
        })
        .collect();
    let mut rows: HashMap<(u32, u32), Vec<&PVarAssignment>> = HashMap::new();
    for a in &routine.var_assignments {
        if vars.contains(&a.lhs_name) {
            let p = (a.source_anchor.start_line, a.source_anchor.start_column);
            rows.entry(p).or_default().push(a);
        }
    }
    let mut w = Walk {
        ctx,
        vars,
        target_id: &op.id,
        target_anchor: &op.source_anchor,
        ops: routine
            .record_operations
            .iter()
            .map(|o| (o.id.as_str(), o))
            .collect(),
        calls: routine
            .call_sites
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect(),
        rows,
        at_target: None,
        loops: Vec::new(),
        gave_up: false,
    };
    w.node(tree, Some(entry), 0);
    if w.gave_up {
        return true;
    }
    // Not reached: the operation is not in the tree, or only on a path that has ended.
    w.at_target.is_none_or(|s| s[0])
}

#[derive(Default)]
struct LoopFrame {
    breaks: State,
    continues: State,
}

struct Walk<'a> {
    ctx: &'a DetectorContext<'a>,
    vars: Vec<String>,
    target_id: &'a str,
    target_anchor: &'a PAnchor,
    ops: HashMap<&'a str, &'a ModelRecordOperation>,
    calls: HashMap<&'a str, &'a PCallSite>,
    rows: HashMap<(u32, u32), Vec<&'a PVarAssignment>>,
    at_target: State,
    loops: Vec<LoopFrame>,
    gave_up: bool,
}

impl Walk<'_> {
    fn index(&self, v: &str) -> Option<usize> {
        self.vars.iter().position(|x| x == v)
    }

    fn seq(&mut self, nodes: Option<&Vec<PCFNNode>>, mut st: State, depth: usize) -> State {
        for n in nodes.map(Vec::as_slice).unwrap_or_default() {
            st = self.node(n, st, depth + 1);
        }
        st
    }

    fn leaves(&mut self, n: &PCFNNode, st: State, depth: usize) -> State {
        self.seq(n.condition_leaves.as_ref(), st, depth)
    }

    fn node(&mut self, n: &PCFNNode, st: State, depth: usize) -> State {
        if st.is_none() || self.gave_up {
            return None;
        }
        if depth > MAX_DEPTH {
            self.gave_up = true;
            return None;
        }
        match n.kind.as_str() {
            "block" | "case-branch" => {
                let st = self.leaves(n, st, depth);
                self.seq(n.children.as_ref(), st, depth)
            }
            "if" => {
                let st = self.leaves(n, st, depth);
                let then = match n.children.as_ref().and_then(|c| c.first()) {
                    Some(b) => self.node(b, st.clone(), depth + 1),
                    None => st.clone(),
                };
                let other = match n.else_children.as_ref().and_then(|c| c.first()) {
                    Some(b) => self.node(b, st, depth + 1),
                    None => st,
                };
                join(then, &other)
            }
            "case" => {
                let st = self.leaves(n, st, depth);
                let mut out = None;
                let mut has_else = false;
                for b in n.children.as_deref().unwrap_or_default() {
                    has_else |= b.is_case_else();
                    let post = self.node(b, st.clone(), depth + 1);
                    out = join(out, &post);
                }
                if !has_else {
                    out = join(out, &st);
                }
                out
            }
            "while" | "for" | "foreach" => self.head_loop(n, st, depth),
            "repeat" => self.tail_loop(n, st, depth),
            "break" | "continue" => {
                let is_break = n.kind == "break";
                match self.loops.last_mut() {
                    Some(f) if is_break => f.breaks = join(f.breaks.take(), &st),
                    Some(f) => f.continues = join(f.continues.take(), &st),
                    // Outside any loop (malformed): the path goes on.
                    None => return st,
                }
                None
            }
            "exit" | "error" => {
                self.leaves(n, st, depth);
                None
            }
            "try" => st.map(|s| vec![true; s.len()]),
            "op" => {
                let mut st = self.leaves(n, st, depth);
                if let Some(op) = n.operation_id.as_deref().and_then(|id| self.ops.get(id)) {
                    if op.id == self.target_id {
                        self.at_target = join(self.at_target.take(), &st);
                    }
                    if RECORD_LOAD_OPS.contains(&op.op.as_str())
                        && before_anchor(&op.source_anchor, self.target_anchor)
                        && let Some(i) = self.index(&op.record_variable_name.fold_identifier())
                        && let Some(s) = st.as_mut()
                    {
                        s[i] = true;
                    }
                }
                self.seq(n.children.as_ref(), st, depth)
            }
            "call" => {
                let mut st = self.leaves(n, st, depth);
                if let Some(cs) = n.callsite_id.as_deref().and_then(|id| self.calls.get(id))
                    && let Some(s) = st.as_mut()
                {
                    let before = before_anchor(&cs.source_anchor, self.target_anchor);
                    for (i, v) in self.vars.iter().enumerate() {
                        if is_clear_of(cs, v) {
                            s[i] = false;
                        } else if before && call_site_loads(self.ctx, cs, v) {
                            s[i] = true;
                        }
                    }
                }
                self.seq(n.children.as_ref(), st, depth)
            }
            // `other` (an assignment, `with`, `asserterror`, an unknown statement) and any
            // unrecognised kind: its leaves, its own effect, then its children.
            _ => {
                let mut st = self.leaves(n, st, depth);
                if let Some(FlowFact::Assign { target, .. }) = n.flow.as_deref()
                    && let Some((l, c, _, _)) = n.source_range
                    && let Some(s) = st.as_mut()
                {
                    self.assign(s, target.as_deref(), (l, c));
                }
                self.seq(n.children.as_ref(), st, depth)
            }
        }
    }

    /// The assignment at `pos` (one row per value it may take: ENG-175's conditional leaves).
    fn assign(&self, s: &mut [bool], target: Option<&str>, pos: (u32, u32)) {
        let Some(rows) = self.rows.get(&pos) else {
            return;
        };
        let before = s.to_vec();
        for (i, v) in self.vars.iter().enumerate() {
            let mine: Vec<_> = rows.iter().filter(|a| a.lhs_name == *v).collect();
            // A whole-variable copy takes its value's state; any other value of a whole-variable
            // target is one the walk cannot see (may be loaded). A field or element write: none.
            let copies = mine.iter().filter_map(|a| a.rhs_identifier.as_deref());
            let mut copied = false;
            let mut loaded = false;
            for r in copies {
                copied = true;
                loaded |= self.index(r).is_none_or(|j| before[j]);
            }
            let opaque =
                target == Some(v.as_str()) && mine.iter().any(|a| a.rhs_identifier.is_none());
            if copied || opaque {
                s[i] = loaded || opaque;
            }
        }
    }

    /// `while`, `for`, `foreach`: the condition at the head, then the body, to a fixed point.
    fn head_loop(&mut self, n: &PCFNNode, pre: State, depth: usize) -> State {
        self.loops.push(LoopFrame::default());
        let mut head = pre.clone();
        let mut settled = false;
        for _ in 0..MAX_LOOP_ROUNDS {
            let cond = self.leaves(n, head.clone(), depth);
            let body = self.seq(n.children.as_ref(), cond, depth);
            let continues = self.loops.last_mut().and_then(|f| f.continues.take());
            let next = join(join(pre.clone(), &body), &continues);
            if next == head {
                settled = true;
                break;
            }
            head = next;
        }
        let frame = self.loops.pop().unwrap_or_default();
        if !settled {
            self.gave_up = true;
            return None;
        }
        let out = self.leaves(n, head, depth);
        join(out, &frame.breaks)
    }

    /// `repeat`: the body, then the condition, to a fixed point.
    fn tail_loop(&mut self, n: &PCFNNode, pre: State, depth: usize) -> State {
        self.loops.push(LoopFrame::default());
        let mut head = pre.clone();
        let mut after = None;
        let mut settled = false;
        for _ in 0..MAX_LOOP_ROUNDS {
            let body = self.seq(n.children.as_ref(), head.clone(), depth);
            let continues = self.loops.last_mut().and_then(|f| f.continues.take());
            after = self.leaves(n, join(body, &continues), depth);
            let next = join(pre.clone(), &after);
            if next == head {
                settled = true;
                break;
            }
            head = next;
        }
        let frame = self.loops.pop().unwrap_or_default();
        if !settled {
            self.gave_up = true;
            return None;
        }
        join(after, &frame.breaks)
    }
}

/// `Clear(V)`: the platform's `Clear` on the variable `v` (lowercased) alone.
fn is_clear_of(cs: &PCallSite, v: &str) -> bool {
    matches!(&cs.callee, PCallee::Bare { name } if name.eq_fold_identifier("clear"))
        && cs.argument_texts.len() == 1
        && crate::record_ops::receiver_key_text(&cs.argument_texts[0])
            .trim_matches('"')
            .fold_identifier()
            == v
}
