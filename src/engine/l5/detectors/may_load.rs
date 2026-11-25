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
//! Effects, wherever they stand on the path (a load later in a loop body reaches the next pass):
//! - a recognised load operation, or `TransferFields` (a well-defined state, as `Copy` and
//!   `Init`), loads its record;
//! - a call may load a record it is handed (`Clear` leaves a well-defined state, as `Init`; a
//!   by-var out parameter leaves the callee's, unknown) unless every binding of it is a resolved
//!   by-value parameter, a record a procedure is called ON (a table procedure runs on the caller's
//!   record; a platform method, `SetRecFilter`, resolves to no routine and loads nothing),
//!   and one a loading call loads;
//! - a whole-variable assignment `A := X` gives `A` the state of `X`; from any other value (a call
//!   result, an expression) `A` may be loaded. A field write or an element write changes nothing.
//!
//! So the one thing that unloads a record is a copy of a record no path has loaded. Anything the
//! walk cannot read answers "may be loaded", and the detectors' textual answer stands: no
//! statement tree, the operation not in it, a load or call on a tracked record the tree does not
//! hold (a `case` pattern: `case true of A.Get(1): ...`), an `asserterror` or `try` statement (an
//! error inside is caught), a pathologically deep tree, a loop that does not settle.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{RECORD_LOAD_OPS, call_site_loads};
use crate::program::body::features::{
    FlowFact, PCFNNode, PCallSite, PCallee, PVarAssignment, PathHint,
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
        seen: HashSet::new(),
        gave_up: false,
        no_load: None,
    };
    w.node(tree, Some(entry), 0);
    if w.gave_up {
        return true;
    }
    // A load or call on a tracked record the tree does not hold: the walk saw only part of the
    // routine (Fable r1: a `case` pattern is no leaf).
    let unseen_op = routine
        .record_operations
        .iter()
        .any(|o| !w.seen.contains(o.id.as_str()) && w.op_effect(o).is_some());
    let unseen_call = routine
        .call_sites
        .iter()
        .any(|c| !w.seen.contains(c.id.as_str()) && w.vars.iter().any(|v| w.call_may_load(c, v)));
    if unseen_op || unseen_call {
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
    ops: HashMap<&'a str, &'a ModelRecordOperation>,
    calls: HashMap<&'a str, &'a PCallSite>,
    rows: HashMap<(u32, u32), Vec<&'a PVarAssignment>>,
    at_target: State,
    loops: Vec<LoopFrame>,
    /// The operation and call-site ids the walk reached.
    seen: HashSet<&'a str>,
    gave_up: bool,
    /// The tested operation whose load the "not found" branch of its `if` leaves out.
    no_load: Option<String>,
}

impl<'a> Walk<'a> {
    fn index(&self, v: &str) -> Option<usize> {
        self.vars.iter().position(|x| x == v)
    }

    fn seq(&mut self, nodes: Option<&Vec<PCFNNode>>, st: State, depth: usize) -> State {
        self.seq_slice(nodes.map(Vec::as_slice).unwrap_or_default(), st, depth)
    }

    /// The nodes in order; a run of one ternary's leaves (`B ? X : Y`) walks each
    /// alternative from the run's entry state and joins the two, since only one runs (sol r1:
    /// sequencing them let `X`'s load reach `Y`). A ternary whose leaves are not one run (a
    /// nested ternary splits it) gives up: "may be loaded".
    fn seq_slice(&mut self, nodes: &[PCFNNode], mut st: State, depth: usize) -> State {
        let mut i = 0;
        while i < nodes.len() {
            let Some((group, _)) = nodes[i].ternary_arm() else {
                st = self.node(&nodes[i], st, depth + 1);
                i += 1;
                continue;
            };
            let in_group = |n: &PCFNNode| n.ternary_arm().is_some_and(|(g, _)| g == group);
            let end = i + nodes[i..].iter().take_while(|n| in_group(n)).count();
            if nodes[end..].iter().any(in_group) {
                self.gave_up = true;
                return None;
            }
            let mut out = None;
            for side in [true, false] {
                let mut s = st.clone();
                for n in nodes[i..end]
                    .iter()
                    .filter(|n| n.ternary_arm().is_some_and(|(_, then)| then == side))
                {
                    s = self.node(n, s, depth + 1);
                }
                out = join(out, &s);
            }
            st = out;
            i = end;
        }
        st
    }

    fn leaves(&mut self, n: &PCFNNode, st: State, depth: usize) -> State {
        self.seq(n.condition_leaves.as_ref(), st, depth)
    }

    /// Code no path reaches loads nothing: its operations and calls count as seen.
    fn mark_seen(&mut self, n: &PCFNNode) {
        for id in [&n.operation_id, &n.callsite_id].into_iter().flatten() {
            if let Some(o) = self.ops.get(id.as_str()) {
                self.seen.insert(o.id.as_str());
            }
            if let Some(c) = self.calls.get(id.as_str()) {
                self.seen.insert(c.id.as_str());
            }
        }
        for group in [&n.condition_leaves, &n.children, &n.else_children]
            .into_iter()
            .flatten()
        {
            for c in group {
                self.mark_seen(c);
            }
        }
    }

    fn node(&mut self, n: &PCFNNode, st: State, depth: usize) -> State {
        if st.is_none() {
            self.mark_seen(n);
            return None;
        }
        if self.gave_up {
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
                // sol r1: a condition that is one tested record operation (`if A.Get(1)`,
                // `if not A.FindFirst()`) loads its record only on the branch where it found
                // one; on the other the record is as it was before the operation.
                let leaves = n.condition_leaves.as_deref().unwrap_or_default();
                let tested = match (n.path_hint(), leaves.split_last()) {
                    (Some(PathHint::CondOp { negated }), Some((last, before))) => last
                        .operation_id
                        .clone()
                        .map(|op| (op, negated, last, before)),
                    _ => None,
                };
                let (then_in, else_in) = match tested {
                    Some((op, negated, last, before)) => {
                        let pre = self.seq_slice(before, st, depth);
                        let found = self.node(last, pre.clone(), depth + 1);
                        self.no_load = Some(op);
                        let missed = self.node(last, pre, depth + 1);
                        self.no_load = None;
                        if negated {
                            (missed, found)
                        } else {
                            (found, missed)
                        }
                    }
                    None => {
                        let st = self.leaves(n, st, depth);
                        (st.clone(), st)
                    }
                };
                let then = match n.children.as_ref().and_then(|c| c.first()) {
                    Some(b) => self.node(b, then_in, depth + 1),
                    None => then_in,
                };
                let other = match n.else_children.as_ref().and_then(|c| c.first()) {
                    Some(b) => self.node(b, else_in, depth + 1),
                    None => else_in,
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
                // A terminating `Error(..)` is its own call site: reached, so not "unseen"
                // (Fable r2).
                if let Some(cs) = n.callsite_id.as_deref().and_then(|id| self.calls.get(id)) {
                    self.seen.insert(cs.id.as_str());
                }
                self.leaves(n, st, depth);
                None
            }
            "try" => st.map(|s| vec![true; s.len()]),
            // An error inside `asserterror` is caught and the routine goes on (Fable r1).
            _ if n.path_hint() == Some(PathHint::AssertError) => st.map(|s| vec![true; s.len()]),
            "op" => {
                let mut st = self.leaves(n, st, depth);
                if let Some(op) = n
                    .operation_id
                    .as_deref()
                    .and_then(|id| self.ops.get(id).copied())
                {
                    self.seen.insert(op.id.as_str());
                    if op.id == self.target_id {
                        self.at_target = join(self.at_target.take(), &st);
                    }
                    if let Some(i) = self.op_effect(op)
                        && let Some(s) = st.as_mut()
                    {
                        s[i] = true;
                    }
                }
                self.seq(n.children.as_ref(), st, depth)
            }
            "call" => {
                let mut st = self.leaves(n, st, depth);
                if let Some(cs) = n
                    .callsite_id
                    .as_deref()
                    .and_then(|id| self.calls.get(id).copied())
                {
                    self.seen.insert(cs.id.as_str());
                    if let Some(s) = st.as_mut() {
                        for (i, v) in self.vars.iter().enumerate() {
                            if self.call_may_load(cs, v) {
                                s[i] = true;
                            }
                        }
                    }
                }
                self.seq(n.children.as_ref(), st, depth)
            }
            // `other` (an assignment, `with`, an unknown statement) and any unrecognised kind:
            // its leaves, its own effect, then its children.
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

    /// The tracked variable a record operation loads: a recognised load, or `TransferFields` (a
    /// well-defined state, as `Copy` and `Init`).
    fn op_effect(&self, op: &ModelRecordOperation) -> Option<usize> {
        if self.no_load.as_deref() == Some(op.id.as_str()) {
            return None;
        }
        (RECORD_LOAD_OPS.contains(&op.op.as_str()) || op.op == "TransferFields")
            .then(|| self.index(&op.record_variable_name.fold_identifier()))
            .flatten()
    }

    /// Whether the call may leave the record `v` (lowercased) loaded: it loads it
    /// (`GetBySystemId` on it, a by-var loader), runs on it (a procedure, a resolved routine,
    /// called ON the record: a table procedure works on the caller's record, Fable r1; never a
    /// platform method, Fable r2), or is handed it other than by a
    /// resolved by-value parameter (`Clear` leaves a well-defined state, as `Init`; a by-var out
    /// parameter, `GetCriterion(Edited)`, the callee's, unknown).
    fn call_may_load(&self, cs: &PCallSite, v: &str) -> bool {
        let named = |t: &str| {
            crate::record_ops::receiver_key_text(t)
                .trim_matches('"')
                .fold_identifier()
                == v
        };
        // A procedure (a resolved routine) called ON the record; a platform method such as
        // `SetRecFilter` resolves to no routine and loads nothing (Fable r2).
        if let PCallee::Member { receiver, .. } = &cs.callee
            && named(receiver)
            && self.ctx.resolved_call_edge_by_callsite.contains_key(&cs.id)
        {
            return true;
        }
        if cs.argument_texts.iter().any(|a| named(a)) {
            let upgraded = self.ctx.upgraded_bindings_by_callsite.get(&cs.id);
            let mut bound = false;
            for (i, b) in cs.argument_bindings.iter().enumerate() {
                if b.source_variable_name.as_deref() != Some(v) {
                    continue;
                }
                bound = true;
                match upgraded.and_then(|u| u.get(i)) {
                    Some(up)
                        if up.binding_resolution == "resolved" && !up.callee_parameter_is_var => {}
                    _ => return true,
                }
            }
            if !bound {
                return true;
            }
        }
        call_site_loads(self.ctx, cs, v)
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
