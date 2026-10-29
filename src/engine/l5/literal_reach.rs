//! Effects a root cannot reach once literal Boolean arguments are followed
//! (G23), and once the table a record argument holds is followed (G33).
//!
//! The capability cone already drops a fact behind `if P then` when every call
//! path binds `P` to the opposite literal ([`crate::engine::l4::param_guard`]),
//! but only through non-recursive singleton SCCs: a recursive SCC shares one
//! cone across its members, so a requirement on one member's parameter has
//! nowhere to live. On Continia DO, `DocSave` calls `RunReport(false, …)`, which
//! passes `UseQueue` on to `RunReportID`, where `if UseQueue then QueueMail`
//! inserts a `CDO Queue Entry`; `RunReport` and `RunReportID` sit in a 489-routine
//! recursive SCC, so the write reached `DocSave`'s cone and `prove … writes-table`
//! answered `yes` for a path that cannot run.
//!
//! G33: a routine that takes a record as a `Variant`, reads its table with
//! `RecRef.GetTable(Variant)` and branches on `case RecRef.Number of
//! Database::"T": …` runs a branch only for the table its caller passes. Base
//! Application's `ApprovalsMgmt.PreventDeletingRecordWithOpenApprovalEntry` is
//! called from a Requisition Line's `OnDelete` with its own `Rec`, so its `Gen.
//! Journal Batch` branch (into the workflow engine) cannot run from there
//! ([`TableFrame`]).
//!
//! This is a per-root reachability over the snapshot's own edges (the graph the
//! cone was built over), whose state is a routine plus the known values of its
//! usable guard parameters (the cone's own [`param_guard`] frames) and the known
//! tables of its usable table parameters:
//! - an edge from a site whose requirements contradict the known values is not
//!   followed;
//! - a direct, variable-typed or interface call (G34: an implementer takes the
//!   arguments by position) binds the callee's usable parameters from a
//!   `true`/`false` argument, a record variable of known table, or the
//!   caller's own known parameter it forwards; every other edge kind (events,
//!   triggers, object runs) binds nothing.
//!
//! G34 adds the RecordRef/Variant value flow ([`RecRefFlow`]): a callee's
//! parameter (or its instance's global) that a guard reads, directly or
//! through what it is passed on to, is bound to the tables the argument's
//! atoms evaluate to in the caller's state, and an exit guard `if R.Number <>
//! Database::"T" then exit;` makes every later site of its routine require
//! `R` to have held `T`.
//!
//! An operation is UNREACHABLE when its routine is reached with guards ignored
//! but no reached state of that routine satisfies the operation's own
//! requirements. An effect is unreachable only when every one of its origin
//! operations is: a graph or id mismatch (a routine not reached even without
//! guards) never drops an effect, and a search over [`STATE_CAP`] states gives
//! up and drops nothing.
//!
//! [`param_guard`]: crate::engine::l4::param_guard

use crate::engine::l4::param_guard::{FrameGuards, Req, frame_guards_over};
use crate::engine::l5::digest::DigestEffectResult;
use crate::engine::l5::recref_flow::{
    Atom, Atoms, RecRefFlow, RelevantGlobals, RelevantParams, TSet, eval,
};
use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::body::features::{PCFNNode, TableCase};
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// The most (routine, bindings) states one root's search visits.
const STATE_CAP: usize = 400_000;

/// A table requirement in one routine's frame: (parameter index, whether its
/// table must be IN the set (else NOT in it), the folded table names).
type TableReq = (u32, bool, Arc<[String]>);

/// G34: a requirement that what `atoms` stand for (in the routine's terms)
/// holds one of the tables.
type AtomReq = (Atoms, Arc<[String]>);

/// Known parameter values, each list sorted by parameter index: Booleans, and
/// the folded names of the tables a record argument may hold (`""`: none).
/// G34: and the tables the routine's own instance's globals may hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
struct Bindings {
    bools: Vec<(u32, bool)>,
    tables: Vec<(u32, TSet)>,
    globals: Vec<(Arc<str>, TSet)>,
}

fn contradicts(reqs: &[Req], b: &Bindings) -> bool {
    reqs.iter()
        .any(|&(i, v)| b.bools.iter().any(|&(j, w)| j == i && w != v))
}

/// Whether no table of `held` meets a requirement that it be (or not be) in `set`.
fn violates(held: &TSet, must_be_in: bool, set: &[String]) -> bool {
    let in_set = |t: &Arc<str>| set.iter().any(|s| s.as_str() == &**t);
    if must_be_in {
        !held.iter().any(in_set)
    } else {
        held.iter().all(in_set)
    }
}

fn contradicts_tables(reqs: &[TableReq], b: &Bindings) -> bool {
    reqs.iter().any(|(i, must_be_in, set)| {
        b.tables
            .iter()
            .any(|(j, t)| j == i && violates(t, *must_be_in, set))
    })
}

fn contradicts_atoms(reqs: &[AtomReq], b: &Bindings) -> bool {
    reqs.iter().any(|(atoms, set)| {
        eval(atoms, &b.tables, &b.globals).is_some_and(|held| violates(&held, true, set))
    })
}

/// G34: one routine's exit guards: past a top-level `if R.Number <>
/// Database::"T" then exit;`, every site requires `R` to have held `T` there.
/// `R` is a parameter or local bound by nothing before the guard (so it holds
/// its entry value, or no table), or an object global (its instance's value
/// at any time). A rebinding after it does not matter: the guard already ran.
fn exit_guards(flow: &RecRefFlow, i: usize) -> HashMap<String, Vec<AtomReq>> {
    let mut out: HashMap<String, Vec<AtomReq>> = HashMap::new();
    let anytime = |n: &str| matches!(flow.initial_atom(i, n), Some(Atom::Global(_)));
    for (rec, tables, ids) in crate::engine::l5::recref_flow::exit_guards(flow.routine(i), anytime)
    {
        let Some(init) = flow.initial_atom(i, &rec) else {
            continue;
        };
        let req: AtomReq = (Atoms::from([init]), tables.into());
        for id in ids {
            out.entry(id).or_default().push(req.clone());
        }
    }
    out
}

/// G33: one routine's table frame. A parameter is USABLE when it is a
/// by-value `Variant` that is never assigned, read by `R.GetTable(P)` for a
/// LOCAL RecordRef `R` that nothing else binds (no other `GetTable`, `Open` or
/// `Copy`, no assignment). Then `R.Number` is `P`'s table wherever `R` is bound
/// (before that a branch on it takes none of the `Database::"T"` patterns, so
/// requiring the table can only keep a branch that cannot run, never drop one
/// that can). A call that passes `R` or `P` on may change them, but only for
/// what runs after it: a `case R.Number` guards its branches only when every
/// such call sits inside that `case` and the `case` is in no loop (Base
/// Application's approval code raises an `ElseCase` event with both from its
/// `else` branch).
#[derive(Debug, Default)]
struct TableFrame {
    usable: Vec<u32>,
    /// Operation id / call-site id -> the table requirements for it to run.
    by_site: HashMap<String, Vec<TableReq>>,
}

fn table_frame(r: &ModelRoutine) -> Option<TableFrame> {
    use crate::program::body::features::PCallee;
    let tree = r.statement_tree.as_ref()?;
    let lc = |s: &str| s.trim().trim_matches('"').to_ascii_lowercase();
    // R (folded) -> the parameter index its one GetTable reads.
    let mut bound: HashMap<String, u32> = HashMap::new();
    // R (folded) -> the call sites that pass R or its P on.
    let mut escapes: HashMap<String, HashSet<String>> = HashMap::new();
    for v in r.variables.iter().filter(|v| {
        !v.is_parameter
            && v.scope.as_deref() == Some("local")
            && v.declared_type.trim().eq_ignore_ascii_case("recordref")
    }) {
        let r_lc = lc(&v.name);
        if r.var_assignments.iter().any(|a| lc(&a.lhs_name) == r_lc) {
            continue;
        }
        let mut source: Option<u32> = None;
        let mut ok = true;
        let mut passed: HashSet<String> = HashSet::new();
        for cs in &r.call_sites {
            if cs.argument_texts.iter().any(|a| lc(a) == r_lc) {
                passed.insert(cs.id.clone());
            }
            let PCallee::Member { receiver, method } = &cs.callee else {
                continue;
            };
            if lc(receiver) != r_lc {
                continue;
            }
            match method.to_ascii_lowercase().as_str() {
                "gettable" => {
                    let p = cs.argument_texts.first().map(|a| lc(a)).and_then(|a| {
                        r.parameters
                            .iter()
                            .find(|p| lc(&p.name) == a)
                            .map(|p| p.index)
                    });
                    match (source, p) {
                        (None, Some(p)) => source = Some(p),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                "open" | "copy" => {
                    ok = false;
                    break;
                }
                _ => {}
            }
        }
        if let (true, Some(p)) = (ok, source) {
            bound.insert(r_lc.clone(), p);
            escapes.insert(r_lc, passed);
        }
    }
    // A usable parameter: a by-value Variant, never assigned. A call passing it
    // on (but its GetTable) is an escape of its RecordRef.
    bound.retain(|r_lc, p| {
        let Some(param) = r.parameters.iter().find(|x| x.index == *p) else {
            return false;
        };
        let name = lc(&param.name);
        if param.is_var
            || !param.type_text.trim().eq_ignore_ascii_case("variant")
            || r.var_assignments.iter().any(|a| lc(&a.lhs_name) == name)
        {
            return false;
        }
        let e = escapes.entry(r_lc.clone()).or_default();
        for cs in &r.call_sites {
            let passes = cs.argument_texts.iter().any(|a| lc(a) == name);
            let is_gettable = matches!(&cs.callee, PCallee::Member { method, .. }
                if method.eq_ignore_ascii_case("gettable"));
            if passes && !is_gettable {
                e.insert(cs.id.clone());
            }
        }
        true
    });
    if bound.is_empty() {
        return None;
    }
    let mut by_site: HashMap<String, Vec<TableReq>> = HashMap::new();
    let cx = TableWalk {
        bound: &bound,
        escapes: &escapes,
    };
    walk_tables(tree, &[], false, &cx, &mut by_site);
    let mut usable: Vec<u32> = bound.values().copied().collect();
    usable.sort_unstable();
    usable.dedup();
    Some(TableFrame { usable, by_site })
}

/// What [`walk_tables`] reads: the bound RecordRefs and their escapes.
struct TableWalk<'a> {
    bound: &'a HashMap<String, u32>,
    escapes: &'a HashMap<String, HashSet<String>>,
}

/// The call-site ids in `node`'s subtree.
fn subtree_sites<'n>(node: &'n PCFNNode, out: &mut HashSet<&'n str>) {
    if let Some(cs) = &node.callsite_id {
        out.insert(cs);
    }
    for c in node
        .condition_leaves
        .iter()
        .chain(&node.children)
        .chain(&node.else_children)
        .flatten()
    {
        subtree_sites(c, out);
    }
}

fn walk_tables(
    node: &PCFNNode,
    reqs: &[TableReq],
    in_loop: bool,
    cx: &TableWalk,
    by_site: &mut HashMap<String, Vec<TableReq>>,
) {
    if !reqs.is_empty() {
        for id in [&node.operation_id, &node.callsite_id]
            .into_iter()
            .flatten()
        {
            by_site.insert(id.clone(), reqs.to_vec());
        }
    }
    let in_loop = in_loop || matches!(node.kind.as_str(), "while" | "repeat" | "for" | "foreach");
    for l in node.condition_leaves.iter().flatten() {
        walk_tables(l, reqs, in_loop, cx, by_site);
    }
    let on = match &node.table_case {
        Some(TableCase::On(r)) if !in_loop && node.kind == "case" => {
            cx.bound.get(r).copied().filter(|_| {
                // Every call passing R or P on sits inside this case.
                let mut inside = HashSet::new();
                subtree_sites(node, &mut inside);
                cx.escapes
                    .get(r)
                    .is_none_or(|e| e.iter().all(|s| inside.contains(s.as_str())))
            })
        }
        _ => None,
    };
    let branches = node.children.iter().flatten();
    match on {
        Some(p) => {
            // The tables every recognised branch names: an `else` runs only
            // for a table none of them is.
            let listed: Arc<[String]> = branches
                .clone()
                .filter_map(|b| match &b.table_case {
                    Some(TableCase::Tables(t)) => Some(t.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            for b in branches {
                let extra: Option<TableReq> = match &b.table_case {
                    Some(TableCase::Tables(t)) => Some((p, true, t.clone().into())),
                    _ if b.is_case_else => Some((p, false, Arc::clone(&listed))),
                    _ => None,
                };
                let mut r = reqs.to_vec();
                r.extend(extra);
                walk_tables(b, &r, in_loop, cx, by_site);
            }
        }
        None => {
            for c in branches {
                walk_tables(c, reqs, in_loop, cx, by_site);
            }
        }
    }
    for c in node.else_children.iter().flatten() {
        walk_tables(c, reqs, in_loop, cx, by_site);
    }
}

/// The edge's (from, to, site, binds-arguments) view.
fn edge_parts(e: &SnapshotGraphEdge) -> Option<(&str, &str, Option<&str>, bool)> {
    Some(match e {
        SnapshotGraphEdge::DirectCall {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, Some(callsite_id.as_str()), true),
        SnapshotGraphEdge::VariableTypedCall {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, Some(callsite_id.as_str()), true),
        // G34: an implementer takes the call's arguments by position, as a
        // direct callee does.
        SnapshotGraphEdge::InterfaceDispatch {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, Some(callsite_id.as_str()), true),
        SnapshotGraphEdge::ObjectRunResolved {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, Some(callsite_id.as_str()), false),
        SnapshotGraphEdge::EventDispatch { from, to, .. } => (from, to, None, false),
        SnapshotGraphEdge::ImplicitTrigger {
            from,
            to,
            operation_id,
            ..
        } => (from, to, Some(operation_id.as_str()), false),
        SnapshotGraphEdge::ObjectRunUnresolved { .. } => return None,
    })
    .map(|(f, t, s, b)| (f.as_str(), t.as_str(), s, b))
}

/// An out-edge: (callee, the site it leaves from, whether it binds arguments).
type OutEdge<'a> = (&'a str, Option<&'a str>, bool);

/// A search's result: the routines reached with guards ignored, and the
/// binding states reached per routine with them.
type Reached<'a> = (HashSet<&'a str>, HashMap<&'a str, Vec<Bindings>>);

/// G35: a guarded search's states with their parents, in visit order
/// ([`LiteralReach::live_tree`]).
pub(crate) struct LiveTree<'a> {
    parent: HashMap<(&'a str, Bindings), Option<(&'a str, Bindings)>>,
    order: Vec<(&'a str, Bindings)>,
}

/// The search context over one snapshot and model, shared by every root.
pub(crate) struct LiteralReach<'a> {
    out: HashMap<&'a str, Vec<OutEdge<'a>>>,
    by_stable: HashMap<&'a str, &'a ModelRoutine>,
    frames: HashMap<&'a str, FrameGuards>,
    tables: HashMap<&'a str, TableFrame>,
    /// Model table id -> its logical name, folded.
    table_names: HashMap<&'a str, Arc<str>>,
    /// Operation id → the stable id of the routine it sits in.
    op_owner: HashMap<&'a str, &'a str>,
    /// G34: the RecordRef/Variant value flow, and per routine (stable id) the
    /// exit-guard requirements by site.
    flow: RecRefFlow<'a>,
    guards: HashMap<&'a str, HashMap<String, Vec<AtomReq>>>,
    /// G34: per routine (flow index) the parameters, and per object the
    /// globals, whose tables a search state carries.
    rel_params: RelevantParams,
    rel_globals: RelevantGlobals<'a>,
}

impl<'a> LiteralReach<'a> {
    pub(crate) fn new(edges: &'a [SnapshotGraphEdge], model: &'a Model) -> Self {
        let routines = &model.workspace.routines;
        let by_id: HashMap<&str, &ModelRoutine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut callees: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &model.calls.edges {
            if let Some(to) = e.to.as_deref() {
                callees.entry(e.callsite_id.as_str()).or_default().push(to);
            }
        }
        let mut frames = HashMap::new();
        let mut tables = HashMap::new();
        let mut by_stable = HashMap::new();
        let mut op_owner = HashMap::new();
        for r in routines {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(r);
            let callees_at =
                |cs: &str| -> Vec<&str> { callees.get(cs).cloned().unwrap_or_default() };
            if let Some(f) = frame_guards_over(r, callees_at, &by_id) {
                frames.insert(r.stable_routine_id.as_str(), f);
            }
            if let Some(t) = table_frame(r) {
                tables.insert(r.stable_routine_id.as_str(), t);
            }
            for op in &r.record_operations {
                op_owner.insert(op.id.as_str(), r.stable_routine_id.as_str());
            }
            for op in &r.operation_sites {
                op_owner.insert(op.id.as_str(), r.stable_routine_id.as_str());
            }
            for cs in &r.call_sites {
                op_owner.insert(cs.operation_id.as_str(), r.stable_routine_id.as_str());
            }
        }
        let table_names = model
            .workspace
            .tables
            .iter()
            .map(|t| (t.id.as_str(), Arc::from(t.name.to_lowercase())))
            .collect();
        let mut out: HashMap<&str, Vec<OutEdge>> = HashMap::new();
        for e in edges {
            if let Some((from, to, site, binds)) = edge_parts(e) {
                out.entry(from).or_default().push((to, site, binds));
            }
        }
        // G34: the exit guards, and what a search must carry to read them
        // (and G33's table parameters).
        let flow = RecRefFlow::new(model, edges);
        let mut guards = HashMap::new();
        let mut seeds: Vec<(usize, Atom)> = Vec::new();
        if flow.usable() {
            for (i, r) in routines.iter().enumerate() {
                if flow.index_of(&r.stable_routine_id) != Some(i) {
                    continue;
                }
                let g = exit_guards(&flow, i);
                if g.is_empty() {
                    continue;
                }
                for (atoms, _) in g.values().flatten() {
                    seeds.extend(atoms.iter().map(|a| (i, a.clone())));
                }
                guards.insert(r.stable_routine_id.as_str(), g);
            }
            for (stable, t) in &tables {
                if let Some(i) = flow.index_of(stable) {
                    seeds.extend(t.usable.iter().map(|&p| (i, Atom::Param(p))));
                }
            }
        }
        let (rel_params, rel_globals) = if seeds.is_empty() {
            Default::default()
        } else {
            flow.relevance(seeds)
        };
        LiteralReach {
            out,
            by_stable,
            frames,
            tables,
            table_names,
            op_owner,
            flow,
            guards,
            rel_params,
            rel_globals,
        }
    }

    fn reqs_at(&self, routine: &str, site: &str) -> &[Req] {
        self.frames
            .get(routine)
            .and_then(|f| f.by_site.get(site))
            .map_or(&[], Vec::as_slice)
    }

    fn table_reqs_at(&self, routine: &str, site: &str) -> &[TableReq] {
        self.tables
            .get(routine)
            .and_then(|f| f.by_site.get(site))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `site` of `routine` cannot run under `b`.
    fn dead_at(&self, routine: &str, site: &str, b: &Bindings) -> bool {
        contradicts(self.reqs_at(routine, site), b)
            || contradicts_tables(self.table_reqs_at(routine, site), b)
            || self
                .guards
                .get(routine)
                .and_then(|g| g.get(site))
                .is_some_and(|reqs| contradicts_atoms(reqs, b))
    }

    /// The callee's bindings across a call at `site` of `caller` with `b`.
    fn bind(&self, caller: &str, site: &str, callee: &str, b: &Bindings) -> Bindings {
        let (f, t) = (self.frames.get(callee), self.tables.get(callee));
        let ci = self.flow.index_of(callee);
        let rel_params = ci.and_then(|c| self.rel_params.get(&c));
        let rel_globals = ci.and_then(|c| {
            self.rel_globals
                .get(self.flow.routine(c).object_id.as_str())
        });
        if f.is_none() && t.is_none() && rel_params.is_none() && rel_globals.is_none() {
            return Bindings::default();
        }
        let Some((r, cs)) = self.by_stable.get(caller).and_then(|r| {
            r.call_sites
                .iter()
                .find(|c| c.id == site)
                .map(|cs| (*r, cs))
        }) else {
            return Bindings::default();
        };
        let forwarded = |j: u32| {
            cs.argument_bindings
                .iter()
                .find(|x| x.parameter_index == j && x.source_kind == "parameter")
                .and_then(|x| x.source_parameter_index)
        };
        let mut out = Bindings::default();
        for &j in f.map_or(&[][..], |f| f.usable.as_slice()) {
            let text = cs
                .argument_texts
                .get(j as usize)
                .map(|t| t.trim().to_ascii_lowercase());
            let v = match text.as_deref() {
                Some("true") => Some(true),
                Some("false") => Some(false),
                _ => forwarded(j)
                    .and_then(|i| b.bools.iter().find(|&&(k, _)| k == i).map(|&(_, w)| w)),
            };
            if let Some(v) = v {
                out.bools.push((j, v));
            }
        }
        for &j in t.map_or(&[][..], |t| t.usable.as_slice()) {
            let arg = cs
                .argument_texts
                .get(j as usize)
                .map(|a| a.trim().trim_matches('"').to_ascii_lowercase());
            // A record variable of the caller with a known table (the implicit
            // `Rec` included), else the caller's own known table parameter.
            let own = arg.as_deref().and_then(|a| {
                r.record_variables
                    .iter()
                    .find(|v| v.name.to_ascii_lowercase() == a)
                    .and_then(|v| v.table_id.as_deref())
                    .and_then(|id| self.table_names.get(id))
                    .map(|t| TSet::from([Arc::clone(t)]))
            });
            let table = own.or_else(|| {
                forwarded(j).and_then(|i| {
                    b.tables
                        .iter()
                        .find(|(k, _)| *k == i)
                        .map(|(_, t)| Arc::clone(t))
                })
            });
            if let Some(table) = table {
                out.tables.push((j, table));
            }
        }
        // G34: what the value flow knows of the callee's relevant parameters
        // (G33's included, where it knows more), and of its instance's globals.
        if let (Some(c), Some(fi)) = (ci, self.flow.index_of(caller)) {
            for &j in rel_params.into_iter().flatten() {
                let atoms = self.flow.arg_atoms(fi, cs, j);
                if let Some(set) = eval(&atoms, &b.tables, &b.globals) {
                    out.tables.retain(|(k, _)| *k != j);
                    out.tables.push((j, set));
                }
            }
            if let Some(wanted) = rel_globals.filter(|g| !g.is_empty()) {
                if self.flow.is_self_call(fi, cs, c) {
                    out.globals = b
                        .globals
                        .iter()
                        .filter(|(g, _)| wanted.contains(g))
                        .cloned()
                        .collect();
                } else {
                    let wanted: Vec<Arc<str>> = wanted.iter().cloned().collect();
                    for (g, atoms) in self
                        .flow
                        .instance_globals(fi, cs, c, &wanted)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(set) = eval(&atoms, &b.tables, &b.globals) {
                            out.globals.push((g, set));
                        }
                    }
                }
            }
        }
        out.bools.sort_unstable();
        out.tables.sort();
        out.globals.sort();
        out
    }

    /// Every routine reached from `root` with guards ignored, and the binding
    /// states reached with them; `None` when the guarded search hit the cap.
    fn search(&self, root: &'a str) -> Option<Reached<'a>> {
        let mut plain: HashSet<&str> = HashSet::from([root]);
        let mut q = VecDeque::from([root]);
        while let Some(f) = q.pop_front() {
            for &(to, _, _) in self.out.get(f).into_iter().flatten() {
                if plain.insert(to) {
                    q.push_back(to);
                }
            }
        }
        let mut seen: HashSet<(&str, Bindings)> = HashSet::from([(root, Bindings::default())]);
        let mut states: HashMap<&str, Vec<Bindings>> =
            HashMap::from([(root, vec![Bindings::default()])]);
        let mut q = VecDeque::from([(root, Bindings::default())]);
        while let Some((f, b)) = q.pop_front() {
            for &(to, site, binds) in self.out.get(f).into_iter().flatten() {
                if let Some(s) = site
                    && self.dead_at(f, s, &b)
                {
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind(f, s, to, &b),
                    _ => Bindings::default(),
                };
                if seen.insert((to, nb.clone())) {
                    if seen.len() > STATE_CAP {
                        return None;
                    }
                    states.entry(to).or_default().push(nb.clone());
                    q.push_back((to, nb));
                }
            }
        }
        Some((plain, states))
    }

    /// G35: the guarded search from `root`, keeping each state's parent and
    /// its order of visit, so [`Self::live_path`] can read a path that can
    /// run off it. `None` when the search hit the cap.
    pub(crate) fn live_tree(&self, root: &'a str) -> Option<LiveTree<'a>> {
        let start = (root, Bindings::default());
        let mut parent: HashMap<(&'a str, Bindings), Option<(&'a str, Bindings)>> =
            HashMap::from([(start.clone(), None)]);
        let mut order: Vec<(&'a str, Bindings)> = Vec::new();
        let mut q = VecDeque::from([start]);
        while let Some((f, b)) = q.pop_front() {
            order.push((f, b.clone()));
            for &(to, site, binds) in self.out.get(f).into_iter().flatten() {
                if let Some(s) = site
                    && self.dead_at(f, s, &b)
                {
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind(f, s, to, &b),
                    _ => Bindings::default(),
                };
                let key = (to, nb);
                if !parent.contains_key(&key) {
                    if parent.len() > STATE_CAP {
                        return None;
                    }
                    parent.insert(key.clone(), Some((f, b.clone())));
                    q.push_back(key);
                }
            }
        }
        Some(LiveTree { parent, order })
    }

    /// G35: over `tree`, the routines from its root to the first state (in
    /// visit order) of a routine owning one of `ops` where that operation can
    /// run; `None` when no such state was reached.
    pub(crate) fn live_path(&self, tree: &LiveTree<'a>, ops: &[String]) -> Option<Vec<&'a str>> {
        let (f, b) = tree.order.iter().find(|(f, b)| {
            ops.iter()
                .any(|o| self.op_owner.get(o.as_str()) == Some(f) && !self.dead_at(f, o, b))
        })?;
        let mut path = vec![*f];
        let mut cur = (*f, b.clone());
        while let Some(Some(p)) = tree.parent.get(&cur) {
            path.push(p.0);
            cur = p.clone();
        }
        path.reverse();
        Some(path)
    }

    /// Whether a listed witness path can run: replayed from the root with no
    /// known values, each hop's call site must not contradict what is known,
    /// and a direct, variable-typed or interface call binds the callee as in
    /// the search.
    pub(crate) fn path_live(&self, path: &[crate::engine::l5::digest::ProjectedHop]) -> bool {
        let mut b = Bindings::default();
        for hop in path {
            let h = &hop.inner;
            let from = h.from_routine_id.as_str();
            let site = h.callsite_id.as_deref().or(h.operation_id.as_deref());
            if let Some(s) = site
                && self.dead_at(from, s, &b)
            {
                return false;
            }
            let Some(to) = h.to_routine_id.as_deref() else {
                break;
            };
            b = match (h.edge_kind.as_deref(), h.callsite_id.as_deref()) {
                (Some("direct-call" | "variable-typed-call" | "interface-dispatch"), Some(cs)) => {
                    self.bind(from, cs, to, &b)
                }
                _ => Bindings::default(),
            };
        }
        true
    }

    /// Under the root's search: whether operation `op` cannot run (module doc).
    fn op_dead(&self, reached: &Reached<'a>, op: &str) -> bool {
        let (plain, states) = reached;
        let Some(&owner) = self.op_owner.get(op) else {
            return false;
        };
        if !plain.contains(owner) {
            return false;
        }
        !states
            .get(owner)
            .is_some_and(|bs| bs.iter().any(|b| !self.dead_at(owner, op, b)))
    }

    /// The indices of `effects` (one root's) unreachable from `root` under
    /// literal arguments (module doc), and per boundary group (G33) the
    /// resources whose every originating operation is: `prove` matches none of
    /// them. A resource with an operation not known is never dropped.
    pub(crate) fn unreachable_effects(
        &self,
        root: &'a str,
        effects: &[DigestEffectResult],
    ) -> (HashSet<usize>, HashMap<usize, Vec<String>>) {
        let mut out = HashSet::new();
        let mut dead_resources: HashMap<usize, Vec<String>> = HashMap::new();
        // A root with no usable guard anywhere has nothing to prune.
        if self.frames.is_empty() && self.tables.is_empty() && self.guards.is_empty() {
            return (out, dead_resources);
        }
        let Some(reached) = self.search(root) else {
            return (out, dead_resources);
        };
        for (i, e) in effects.iter().enumerate() {
            if e.boundary_group {
                let dead: Vec<String> = e
                    .group_resource_ops
                    .iter()
                    .filter(|(_, ops)| {
                        ops.as_ref().is_some_and(|ops| {
                            !ops.is_empty() && ops.iter().all(|op| self.op_dead(&reached, op))
                        })
                    })
                    .map(|(r, _)| r.clone())
                    .collect();
                if !dead.is_empty() {
                    dead_resources.insert(i, dead);
                }
                continue;
            }
            if e.origin_operation_ids.is_empty() {
                continue;
            }
            if e.origin_operation_ids
                .iter()
                .all(|op| self.op_dead(&reached, op))
            {
                out.insert(i);
            }
        }
        (out, dead_resources)
    }
}
