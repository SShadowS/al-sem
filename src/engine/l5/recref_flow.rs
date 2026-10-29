//! G34: where the table a RecordRef (or a record in a Variant) holds comes
//! from, so a guard on `R.Number` can be read against the root's own record.
//!
//! On Continia DO, `VendorLedgerPrint` puts a Vendor Ledger Entry in a
//! RecordRef, hands it to the e-mail handler (`SetRecRefFilter` keeps it in a
//! codeunit global), and prints through that handler. The print path copies the
//! global (`RecRef := RecRefFilter.Duplicate()`) and passes the copy down to
//! `CDO File.CalcSalesDisc`, which starts with `if RecRef.Number <>
//! Database::"Sales Header" then exit;`: its `Codeunit.Run(Sales-Calc.
//! Discount)` cannot run from there, but `prove … writes-table:Vendor` answered
//! `yes` through it.
//!
//! Each routine gets a flow-insensitive summary: for each of its RecordRef and
//! Variant variables, the [`Atom`]s it may hold anywhere in the routine (its
//! value on entry for a parameter, `Unset` for a local, a table bound by
//! `GetTable` of a record, another variable's atoms by `:=` or `Duplicate()`,
//! or `Top` for anything the summary cannot follow), and for the object's
//! globals the atoms the routine may assign. A variable passed to a resolved
//! call takes on what the callee may leave in that parameter (its own
//! summary, read across the call); an event publisher answers for its loaded
//! subscribers, and an interface call for its loaded implementers. An
//! unresolved or dynamic call, an `Open`, a `Copy` or an unknown value is
//! `Top`, so a summary over-approximates every value the variable can hold.
//!
//! The loaded worlds are the ones `prove` already answers in: a cone follows
//! only loaded subscribers (its `no` carries `loaded-subscribers`), and an
//! interface call in the cone is an `open-world-dispatch` blocker unless the
//! interface is provably closed (or `--closed-world` waives it). A summary
//! reads only calls of routines the cone reaches, so a value an unloaded
//! implementer could give never decides an answer the cone's own blockers
//! leave open.
//!
//! A codeunit global is read through the INSTANCE it lives on: a codeunit a
//! routine declares as a local (not `SingleInstance`, never passed on or
//! assigned) is fresh for each call of that routine, so its globals hold only
//! `Unset` or what the calls made on it assign
//! ([`RecRefFlow::instance_globals`]). A self-call carries the caller's
//! instance along.
//!
//! [`LiteralReach`](crate::engine::l5::literal_reach::LiteralReach) evaluates
//! the atoms under the parameter tables and globals one search state knows.

use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::body::features::{PCFNNode, PCallSite, PCallee, TableCase};
use crate::program::model::workspace::ModelParameter;
use crate::program::model::workspace::{Model, ModelRoutine};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// One value a RecordRef or Variant may hold, in one routine's terms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Atom {
    /// A table, by its folded logical name.
    Table(Arc<str>),
    /// No table (a RecordRef never opened, `Number` 0; an empty Variant).
    Unset,
    /// What parameter `i` held on entry.
    Param(u32),
    /// What the routine's own instance holds in this global (folded name).
    Global(Arc<str>),
    /// Anything.
    Top,
}

pub(crate) type Atoms = BTreeSet<Atom>;

/// A known set of tables, folded and sorted; `""` is [`Atom::Unset`].
pub(crate) type TSet = Arc<[Arc<str>]>;

/// Per routine (flow index), the parameters a search state carries.
pub(crate) type RelevantParams = HashMap<usize, BTreeSet<u32>>;
/// Per object, the globals a search state carries.
pub(crate) type RelevantGlobals<'a> = HashMap<&'a str, BTreeSet<Arc<str>>>;

/// Where a variable is rebound in place: (position, what it is bound to).
type InPlace = HashMap<String, Vec<((u32, u32), Part)>>;

/// The most rounds the summaries are iterated; past it nothing is tracked.
const MAX_ROUNDS: usize = 64;

fn lc(s: &str) -> String {
    s.trim().trim_matches('"').to_ascii_lowercase()
}

fn is_tracked_type(t: &str) -> bool {
    let t = t.trim();
    t.eq_ignore_ascii_case("recordref") || t.eq_ignore_ascii_case("variant")
}

fn is_publisher(r: &ModelRoutine) -> bool {
    use crate::program::attributes::has_attribute;
    ["IntegrationEvent", "BusinessEvent", "InternalEvent"]
        .iter()
        .any(|a| has_attribute(&r.attributes_parsed, a))
}

fn top() -> Atoms {
    Atoms::from([Atom::Top])
}

/// Adds atoms; `Top` does not absorb the others, so what a value depends on
/// (its parameters and globals) stays readable for [`RecRefFlow::relevance`].
fn add(into: &mut Atoms, from: impl IntoIterator<Item = Atom>) -> bool {
    let before = into.len();
    into.extend(from);
    into.len() != before
}

/// G35: a condition a fact runs under: the RecordRef variable, whether its
/// table must be IN the tables (else NOT in them), the folded tables.
type Cond = (String, bool, Arc<[String]>);

/// A source region `[from, to)` whose code runs only under a condition.
type CondRegion = ((u32, u32), (u32, u32), Cond);

/// A fact: the variable, what it may take on, and where it sits.
type FixedFact = (String, Part, Option<(u32, u32)>);

/// A value under known inputs: the tables, or `None` for anything.
pub(crate) type Val = Option<TSet>;

fn union_val(a: &Val, b: &Val) -> Val {
    match (a, b) {
        (Some(x), Some(y)) => {
            let s: BTreeSet<Arc<str>> = x.iter().chain(y.iter()).cloned().collect();
            Some(s.into_iter().collect())
        }
        _ => None,
    }
}

fn empty_val() -> Val {
    Some(Arc::from([]))
}

/// A tracked variable of one routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Param(u32),
    Local,
    Global,
}

/// One part of a binding's value.
#[derive(Debug, Clone)]
enum Part {
    /// What the named variable (or record) holds.
    Copy(String),
    Atoms(Atoms),
}

/// One routine's binding facts: a variable may take on the union of parts.
#[derive(Debug, Default)]
struct Facts {
    tracked: HashMap<String, VarKind>,
    /// Facts that do not depend on other routines, with where they sit.
    fixed: Vec<FixedFact>,
    /// (call site index, argument position, variable): the variable passed there.
    passed: Vec<(usize, usize, String)>,
    /// G35: the conditions each fact runs under (parallel to `fixed`/`passed`).
    fixed_conds: Vec<Vec<Cond>>,
    passed_conds: Vec<Vec<Cond>>,
    /// Variables an alias rebinding reaches (a by-value Variant parameter in
    /// it may hand a changed RecordRef back).
    alias_shared: HashSet<String>,
}

/// Per routine: the atoms of each tracked own variable, and of each global it
/// assigns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Summary {
    vars: HashMap<String, Atoms>,
    global_writes: HashMap<String, Atoms>,
}

pub(crate) struct RecRefFlow<'a> {
    routines: &'a [ModelRoutine],
    by_stable: HashMap<&'a str, usize>,
    /// Call site id -> the routines it resolves to.
    targets: HashMap<&'a str, Vec<usize>>,
    /// Call sites that may run code no summary describes (interface dispatch,
    /// a dynamic or unresolved target), and platform builtins.
    open_sites: HashSet<&'a str>,
    builtin_sites: HashSet<&'a str>,
    /// Publisher -> its loaded subscribers.
    subscribers: HashMap<usize, Vec<usize>>,
    table_names: HashMap<&'a str, Arc<str>>,
    /// Object id -> `SingleInstance`.
    single_instance: HashMap<&'a str, bool>,
    facts: Vec<Facts>,
    summaries: Vec<Summary>,
    /// Per object: the globals' writes over each routine's self-call closure,
    /// in that routine's terms (`closure_writes`).
    closure: HashMap<&'a str, HashMap<usize, HashMap<String, Atoms>>>,
    /// Objects whose instance escapes (`this`/`CurrCodeunit` passed on).
    self_escapes: HashSet<&'a str>,
    converged: bool,
    /// G35: per routine, whether it is evaluated under its inputs.
    sensitive: Vec<bool>,
    eval: RefCell<EvalState>,
}

/// One evaluation's key: the routine, its tracked parameters' input tables,
/// its instance's globals.
type EvalKey = (usize, Vec<(u32, Val)>, Vec<(Arc<str>, Val)>);

/// G35: memoized evaluations of sensitive routines under concrete inputs
/// (a least fixpoint, by chaotic iteration over recursive calls).
#[derive(Default)]
struct EvalState {
    memo: HashMap<EvalKey, HashMap<String, Val>>,
    /// Keys whose value is final (computed in an iteration that changed
    /// nothing).
    done: HashSet<EvalKey>,
    in_progress: HashSet<EvalKey>,
    /// An in-progress value was read, or a memo value grew, this iteration.
    read_partial: bool,
    grew: bool,
    /// Evaluations left before everything is `None` (anything).
    budget: usize,
}

/// Evaluations one `RecRefFlow` may run in all.
const EVAL_BUDGET: usize = 200_000;
/// The deepest chain of calls one evaluation follows.
const EVAL_DEPTH: usize = 48;

impl<'a> RecRefFlow<'a> {
    pub(crate) fn new(model: &'a Model, edges: &'a [SnapshotGraphEdge]) -> Self {
        let routines = model.workspace.routines.as_slice();
        let by_id: HashMap<&str, usize> = routines
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.as_str(), i))
            .collect();
        let mut by_stable = HashMap::new();
        for (i, r) in routines.iter().enumerate() {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(i);
        }
        // A call's targets: the model's resolved calls (workspace callers) and
        // the snapshot graph's call edges (dependency callers too).
        // A site with interface dispatch, or a dynamic or unresolved target, may
        // run code no summary describes; a platform builtin keeps its arguments.
        let mut targets: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut open_sites: HashSet<&str> = HashSet::new();
        let mut builtin_sites: HashSet<&str> = HashSet::new();
        let mut push = |cs: &'a str, t: usize| {
            let v = targets.entry(cs).or_default();
            if !v.contains(&t) {
                v.push(t);
            }
        };
        for e in &model.calls.edges {
            use crate::program::model::taxonomy::DispatchKind;
            match e.dispatch_kind {
                DispatchKind::Dynamic | DispatchKind::Unresolved => {
                    open_sites.insert(e.callsite_id.as_str());
                    continue;
                }
                DispatchKind::Builtin => {
                    builtin_sites.insert(e.callsite_id.as_str());
                    continue;
                }
                _ => {}
            }
            match e.to.as_deref().and_then(|to| by_id.get(to)) {
                Some(&t) => push(e.callsite_id.as_str(), t),
                None => {
                    open_sites.insert(e.callsite_id.as_str());
                }
            }
        }
        for e in edges {
            let (cs, to) = match e {
                SnapshotGraphEdge::DirectCall {
                    callsite_id, to, ..
                }
                | SnapshotGraphEdge::VariableTypedCall {
                    callsite_id, to, ..
                }
                | SnapshotGraphEdge::InterfaceDispatch {
                    callsite_id, to, ..
                }
                | SnapshotGraphEdge::ObjectRunResolved {
                    callsite_id, to, ..
                } => (callsite_id, to),
                SnapshotGraphEdge::ObjectRunUnresolved { callsite_id, .. } => {
                    open_sites.insert(callsite_id.as_str());
                    continue;
                }
                _ => continue,
            };
            match by_stable.get(to.as_str()) {
                Some(&t) => push(cs.as_str(), t),
                None => {
                    open_sites.insert(cs.as_str());
                }
            }
        }
        let mut subscribers: HashMap<usize, Vec<usize>> = HashMap::new();
        for e in edges {
            if let SnapshotGraphEdge::EventDispatch { from, to, .. } = e
                && let (Some(&p), Some(&s)) =
                    (by_stable.get(from.as_str()), by_stable.get(to.as_str()))
            {
                let v = subscribers.entry(p).or_default();
                if !v.contains(&s) {
                    v.push(s);
                }
            }
        }
        let table_names = model
            .workspace
            .tables
            .iter()
            .map(|t| (t.id.as_str(), Arc::from(t.name.to_lowercase())))
            .collect();
        let single_instance = model
            .workspace
            .objects
            .iter()
            .map(|o| (o.id.as_str(), o.single_instance == Some(true)))
            .collect();
        // Per object: the tracked globals some routine rebinds in place.
        let mut in_place: HashMap<&str, HashSet<String>> = HashMap::new();
        for r in routines {
            for cs in &r.call_sites {
                if let PCallee::Member { receiver, .. } = &cs.callee {
                    let x = lc(receiver);
                    if r.variables.iter().any(|v| {
                        lc(&v.name) == x
                            && v.scope.as_deref() == Some("global")
                            && is_tracked_type(&v.declared_type)
                    }) && in_place_part(r, cs, &x).is_some()
                    {
                        in_place.entry(r.object_id.as_str()).or_default().insert(x);
                    }
                }
            }
        }
        let none = HashSet::new();
        let mut self_escapes = HashSet::new();
        let facts: Vec<Facts> = routines
            .iter()
            .map(|r| {
                if r.call_sites.iter().any(|cs| {
                    cs.argument_texts
                        .iter()
                        .any(|a| matches!(lc(a).as_str(), "this" | "currcodeunit"))
                }) {
                    self_escapes.insert(r.object_id.as_str());
                }
                facts_of(r, in_place.get(r.object_id.as_str()).unwrap_or(&none))
            })
            .collect();
        let mut flow = RecRefFlow {
            routines,
            by_stable,
            targets,
            open_sites,
            builtin_sites,
            subscribers,
            table_names,
            single_instance,
            summaries: facts.iter().map(initial_summary).collect(),
            facts,
            closure: HashMap::new(),
            self_escapes,
            converged: false,
            sensitive: Vec::new(),
            eval: RefCell::new(EvalState {
                budget: EVAL_BUDGET,
                ..Default::default()
            }),
        };
        flow.solve_alias_reach();
        flow.solve();
        if flow.converged {
            flow.solve_closures();
            flow.solve_sensitive();
        }
        flow
    }

    /// G35: a variable reaches an alias rebinding when its routine rebinds an
    /// alias of it, or passes it on to a call that may (an open or unresolved
    /// call, or a parameter that reaches one). Only such a by-value Variant
    /// parameter can hand a changed RecordRef back ([`keeps_by_value`]).
    fn solve_alias_reach(&mut self) {
        loop {
            let mut grown: Vec<(usize, String)> = Vec::new();
            for (i, f) in self.facts.iter().enumerate() {
                let r = &self.routines[i];
                for (ci, k, v) in &f.passed {
                    if f.alias_shared.contains(v) {
                        continue;
                    }
                    let cs = &r.call_sites[*ci];
                    let reaches = if self.open_sites.contains(cs.id.as_str()) {
                        true
                    } else if let Some(ts) = self.targets.get(cs.id.as_str()) {
                        ts.iter().any(|&t| {
                            let callees: Vec<(usize, Option<&ModelParameter>)> =
                                if is_publisher(&self.routines[t]) {
                                    let name = self.routines[t]
                                        .parameters
                                        .iter()
                                        .find(|p| p.index == *k as u32)
                                        .map(|p| lc(&p.name));
                                    self.subscribers
                                        .get(&t)
                                        .into_iter()
                                        .flatten()
                                        .map(|&s| {
                                            (
                                                s,
                                                self.routines[s].parameters.iter().find(|p| {
                                                    name.as_deref() == Some(lc(&p.name).as_str())
                                                }),
                                            )
                                        })
                                        .collect()
                                } else {
                                    vec![(
                                        t,
                                        self.routines[t]
                                            .parameters
                                            .iter()
                                            .find(|p| p.index == *k as u32),
                                    )]
                                };
                            callees.into_iter().any(|(c, p)| match p {
                                Some(p) => self.facts[c].alias_shared.contains(&lc(&p.name)),
                                None => !is_publisher(&self.routines[t]),
                            })
                        })
                    } else {
                        writes_arguments(cs)
                            || !(self.builtin_sites.contains(cs.id.as_str())
                                || keeps_unresolved(r, cs))
                    };
                    if reaches {
                        grown.push((i, v.clone()));
                    }
                }
            }
            if grown.is_empty() {
                return;
            }
            for (i, v) in grown {
                self.facts[i].alias_shared.insert(v);
            }
        }
    }

    /// G35: a routine is SENSITIVE when one of its facts runs under a
    /// condition, or a call it passes a tracked variable to reaches one: its
    /// values then depend on the inputs beyond what a summary keeps, and
    /// [`Self::arg_tables`] evaluates it under them.
    fn solve_sensitive(&mut self) {
        let n = self.routines.len();
        let mut s: Vec<bool> = self
            .facts
            .iter()
            .map(|f| {
                f.fixed_conds.iter().any(|c| !c.is_empty())
                    || f.passed_conds.iter().any(|c| !c.is_empty())
            })
            .collect();
        loop {
            let mut changed = false;
            for i in 0..n {
                if s[i] {
                    continue;
                }
                let r = &self.routines[i];
                let hit = self.facts[i].passed.iter().any(|(ci, _, _)| {
                    self.targets
                        .get(r.call_sites[*ci].id.as_str())
                        .into_iter()
                        .flatten()
                        .any(|&t| {
                            s[t] || self
                                .subscribers
                                .get(&t)
                                .into_iter()
                                .flatten()
                                .any(|&x| s[x])
                        })
                });
                if hit {
                    s[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.sensitive = s;
    }

    /// The tables the argument at position `k` of call site `cs` of `i` may
    /// hold when `i` runs with these parameter tables and globals. A
    /// sensitive routine is evaluated under them (G35); any other reads its
    /// summary.
    pub(crate) fn arg_tables(
        &self,
        i: usize,
        cs: &PCallSite,
        k: u32,
        params: &[(u32, TSet)],
        globals: &[(Arc<str>, TSet)],
    ) -> Val {
        let summary = eval(&self.arg_atoms(i, cs, k), params, globals);
        // The evaluation adds what the summary cannot know (it may also give
        // up); both over-approximate, so either answer is sound.
        if summary.is_some() || !self.sensitive.get(i).copied().unwrap_or(false) {
            return summary;
        }
        self.evaluate_arg(i, cs, k, params, globals)
    }

    /// `arg_tables` by evaluating `i` under its inputs.
    fn evaluate_arg(
        &self,
        i: usize,
        cs: &PCallSite,
        k: u32,
        params: &[(u32, TSet)],
        globals: &[(Arc<str>, TSet)],
    ) -> Val {
        let arg = cs.argument_texts.get(k as usize)?;
        let mut inputs: Vec<(u32, Val)> = self.facts[i]
            .tracked
            .values()
            .filter_map(|v| match v {
                VarKind::Param(p) => Some((
                    *p,
                    params
                        .iter()
                        .find(|(j, _)| j == p)
                        .map(|(_, s)| Arc::clone(s)),
                )),
                _ => None,
            })
            .collect();
        inputs.sort();
        let globals: Vec<(Arc<str>, Val)> = globals
            .iter()
            .map(|(g, s)| (Arc::clone(g), Some(Arc::clone(s))))
            .collect();
        let vals = self.evaluate(i, inputs, globals.clone())?;
        self.val_of_text(i, &vals, &globals, arg)
    }

    /// Evaluates `i` under its inputs to a fixpoint over recursive calls:
    /// repeats while an iteration read a value still being computed and some
    /// value grew. `None` when it does not settle.
    fn evaluate(
        &self,
        i: usize,
        inputs: Vec<(u32, Val)>,
        globals: Vec<(Arc<str>, Val)>,
    ) -> Option<HashMap<String, Val>> {
        for _ in 0..12 {
            let mut touched = HashSet::new();
            {
                let mut st = self.eval.borrow_mut();
                st.read_partial = false;
                st.grew = false;
            }
            let vals = self.eval_routine(i, &inputs, &globals, 0, &mut touched);
            let mut st = self.eval.borrow_mut();
            if !(st.read_partial && st.grew) {
                st.done.extend(touched);
                return Some(vals);
            }
        }
        None
    }

    /// The starting values of `i`'s tracked own variables: a parameter its
    /// input, a local no table.
    fn initial_vals(&self, i: usize, inputs: &[(u32, Val)]) -> HashMap<String, Val> {
        self.facts[i]
            .tracked
            .iter()
            .filter_map(|(name, k)| match k {
                VarKind::Param(p) => Some((
                    name.clone(),
                    inputs
                        .iter()
                        .find(|(j, _)| j == p)
                        .and_then(|(_, v)| v.clone()),
                )),
                VarKind::Local => Some((name.clone(), Some(Arc::from([Arc::from("")])))),
                VarKind::Global => None,
            })
            .collect()
    }

    fn eval_routine(
        &self,
        i: usize,
        inputs: &[(u32, Val)],
        globals: &[(Arc<str>, Val)],
        depth: usize,
        touched: &mut HashSet<EvalKey>,
    ) -> HashMap<String, Val> {
        // A routine that is not sensitive reads its summary: no memo, no budget.
        if !self.sensitive[i] {
            let mut vals = self.initial_vals(i, inputs);
            for (v, atoms) in &self.summaries[i].vars {
                vals.insert(v.clone(), eval_val(atoms, inputs, globals));
            }
            return vals;
        }
        let key: EvalKey = (i, inputs.to_vec(), globals.to_vec());
        let all_top = || -> HashMap<String, Val> {
            self.facts[i]
                .tracked
                .keys()
                .map(|k| (k.clone(), None))
                .collect()
        };
        {
            let mut st = self.eval.borrow_mut();
            if st.done.contains(&key) {
                return st.memo[&key].clone();
            }
            if st.in_progress.contains(&key) {
                st.read_partial = true;
                return st
                    .memo
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| self.initial_vals(i, inputs));
            }
            if depth > EVAL_DEPTH || st.budget == 0 {
                return all_top();
            }
            st.budget -= 1;
            st.in_progress.insert(key.clone());
        }
        touched.insert(key.clone());
        let mut vals = self.initial_vals(i, inputs);
        if let Some(prev) = self.eval.borrow().memo.get(&key) {
            for (k, v) in prev {
                let cur = vals.entry(k.clone()).or_insert_with(empty_val);
                *cur = union_val(cur, v);
            }
        }
        let f = &self.facts[i];
        if !self.sensitive[i] {
            for (v, atoms) in &self.summaries[i].vars {
                vals.insert(v.clone(), eval_val(atoms, inputs, globals));
            }
        } else {
            loop {
                let mut changed = false;
                let mut merge = |vals: &mut HashMap<String, Val>, v: &str, add: Val| {
                    if matches!(f.tracked.get(v), None | Some(VarKind::Global)) {
                        return;
                    }
                    let cur = vals.entry(v.to_string()).or_insert_with(empty_val);
                    let new = union_val(cur, &add);
                    if new != *cur {
                        *cur = new;
                        changed = true;
                    }
                };
                for (n, (v, part, _)) in f.fixed.iter().enumerate() {
                    if !self.conds_hold(i, &f.fixed_conds[n], &vals, globals) {
                        continue;
                    }
                    let add = match part {
                        Part::Copy(w) => self.val_of_text(i, &vals, globals, w),
                        Part::Atoms(a) => eval_val(a, inputs, globals),
                    };
                    merge(&mut vals, v, add);
                }
                for (n, (ci, k, v)) in f.passed.iter().enumerate() {
                    if !self.conds_hold(i, &f.passed_conds[n], &vals, globals) {
                        continue;
                    }
                    if let Some(add) =
                        self.out_val(i, &vals, globals, *ci, *k as u32, depth, touched)
                    {
                        merge(&mut vals, v, add);
                    }
                }
                if !changed {
                    break;
                }
            }
        }
        let mut st = self.eval.borrow_mut();
        st.in_progress.remove(&key);
        let before = st
            .memo
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.initial_vals(i, inputs));
        if before != vals {
            st.grew = true;
        }
        st.memo.insert(key, vals.clone());
        vals
    }

    /// What `text` (an argument or a value) holds in `i` under `vals`.
    fn val_of_text(
        &self,
        i: usize,
        vals: &HashMap<String, Val>,
        globals: &[(Arc<str>, Val)],
        text: &str,
    ) -> Val {
        let name = lc(text);
        match self.facts[i].tracked.get(&name) {
            Some(VarKind::Global) => {
                return globals
                    .iter()
                    .find(|(g, _)| **g == *name)
                    .and_then(|(_, v)| v.clone());
            }
            Some(_) => return vals.get(&name).cloned().unwrap_or(None),
            None => {}
        }
        match self.atoms_of(i, &HashMap::new(), &name).into_iter().next() {
            Some(Atom::Table(t)) => Some(Arc::from([t])),
            _ => None,
        }
    }

    fn conds_hold(
        &self,
        i: usize,
        conds: &[Cond],
        vals: &HashMap<String, Val>,
        globals: &[(Arc<str>, Val)],
    ) -> bool {
        conds.iter().all(
            |(rv, must_in, tables)| match self.val_of_text(i, vals, globals, rv) {
                None => true,
                Some(held) => held
                    .iter()
                    .any(|t| tables.iter().any(|x| x.as_str() == &**t) == *must_in),
            },
        )
    }

    /// What the variable passed at position `k` of call site `ci` of `i` may
    /// hold after the call, under `vals`; `None` when the call keeps it.
    #[allow(clippy::too_many_arguments)]
    fn out_val(
        &self,
        i: usize,
        vals: &HashMap<String, Val>,
        globals: &[(Arc<str>, Val)],
        ci: usize,
        k: u32,
        depth: usize,
        touched: &mut HashSet<EvalKey>,
    ) -> Option<Val> {
        let r = &self.routines[i];
        let cs = &r.call_sites[ci];
        if self.open_sites.contains(cs.id.as_str()) {
            return Some(None);
        }
        let Some(ts) = self.targets.get(cs.id.as_str()) else {
            let keeps = !writes_arguments(cs)
                && (self.builtin_sites.contains(cs.id.as_str()) || keeps_unresolved(r, cs));
            return (!keeps).then_some(None);
        };
        let mut out = empty_val();
        for &t in ts {
            let callee = &self.routines[t];
            if is_publisher(callee) {
                let Some(pname) = callee
                    .parameters
                    .iter()
                    .find(|p| p.index == k)
                    .map(|p| lc(&p.name))
                else {
                    return Some(None);
                };
                for &s in self.subscribers.get(&t).into_iter().flatten() {
                    if let Some(sp) = self.routines[s]
                        .parameters
                        .iter()
                        .find(|p| lc(&p.name) == pname)
                        && let Some(o) = self.callee_out(
                            i,
                            vals,
                            globals,
                            cs,
                            s,
                            sp,
                            Some(callee),
                            depth,
                            touched,
                        )
                    {
                        out = union_val(&out, &o);
                    }
                }
                continue;
            }
            let Some(p) = callee.parameters.iter().find(|p| p.index == k) else {
                return Some(None);
            };
            if let Some(o) = self.callee_out(i, vals, globals, cs, t, p, None, depth, touched) {
                out = union_val(&out, &o);
            }
        }
        Some(out)
    }

    /// What callee `t` may leave in its parameter `p`, run with the arguments
    /// of `cs` under `vals` (for a subscriber, `publisher`'s positions);
    /// `None` when it keeps the caller's value.
    #[allow(clippy::too_many_arguments)]
    fn callee_out(
        &self,
        i: usize,
        vals: &HashMap<String, Val>,
        globals: &[(Arc<str>, Val)],
        cs: &PCallSite,
        t: usize,
        p: &ModelParameter,
        publisher: Option<&ModelRoutine>,
        depth: usize,
        touched: &mut HashSet<EvalKey>,
    ) -> Option<Val> {
        let callee = &self.routines[t];
        if !callee.body_available || callee.parse_incomplete {
            return Some(None);
        }
        let pname = lc(&p.name);
        if !self.facts[t].tracked.contains_key(&pname) {
            return (!p.is_record).then_some(None);
        }
        if keeps_by_value(callee, &self.facts[t], p) {
            return None;
        }
        let mut inputs: Vec<(u32, Val)> = Vec::new();
        for (name, kind) in &self.facts[t].tracked {
            let VarKind::Param(j) = kind else { continue };
            let pos = match publisher {
                None => Some(*j),
                Some(pb) => pb
                    .parameters
                    .iter()
                    .find(|y| lc(&y.name) == *name)
                    .map(|y| y.index),
            };
            let v = pos
                .and_then(|pos| cs.argument_texts.get(pos as usize))
                .and_then(|a| self.val_of_text(i, vals, globals, a));
            inputs.push((*j, v));
        }
        inputs.sort();
        let g: Vec<(Arc<str>, Val)> =
            if publisher.is_none() && on_self(cs) && callee.object_id == self.routines[i].object_id
            {
                globals.to_vec()
            } else {
                Vec::new()
            };
        let res = self.eval_routine(t, &inputs, &g, depth + 1, touched);
        Some(res.get(&pname).cloned().unwrap_or(None))
    }

    /// Whether the summaries reached a fixpoint (else nothing is tracked).
    pub(crate) fn usable(&self) -> bool {
        self.converged
    }

    pub(crate) fn index_of(&self, stable: &str) -> Option<usize> {
        self.by_stable.get(stable).copied()
    }

    pub(crate) fn routine(&self, i: usize) -> &'a ModelRoutine {
        &self.routines[i]
    }

    fn solve(&mut self) {
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for i in 0..self.routines.len() {
                if self.facts[i].tracked.is_empty() {
                    continue;
                }
                let s = self.summarize(i);
                if s != self.summaries[i] {
                    self.summaries[i] = s;
                    changed = true;
                }
            }
            if !changed {
                self.converged = true;
                return;
            }
        }
    }

    /// The atoms `text` (an argument or a value) stands for in routine `i`.
    fn atoms_of(&self, i: usize, vars: &HashMap<String, Atoms>, text: &str) -> Atoms {
        let r = &self.routines[i];
        let name = lc(text);
        match self.facts[i].tracked.get(&name) {
            Some(VarKind::Global) => return Atoms::from([Atom::Global(Arc::from(name))]),
            Some(_) => return vars.get(&name).cloned().unwrap_or_else(top),
            None => {}
        }
        r.record_variables
            .iter()
            .find(|v| lc(&v.name) == name)
            .and_then(|v| {
                v.table_id
                    .as_deref()
                    .and_then(|id| self.table_names.get(id).cloned())
                    .or_else(|| v.table_name.as_deref().map(|n| Arc::from(lc(n))))
            })
            .map_or_else(top, |t| Atoms::from([Atom::Table(t)]))
    }

    /// What callee `t` may leave in its parameter `k`, in the caller `i`'s
    /// terms at call site `cs`.
    fn out_of(
        &self,
        i: usize,
        vars: &HashMap<String, Atoms>,
        cs: &PCallSite,
        t: usize,
        k: u32,
    ) -> Atoms {
        let callee = &self.routines[t];
        if is_publisher(callee) {
            // The loaded subscribers, matched by parameter name.
            let Some(pname) = callee
                .parameters
                .iter()
                .find(|p| p.index == k)
                .map(|p| lc(&p.name))
            else {
                return top();
            };
            let mut out = Atoms::new();
            for &s in self.subscribers.get(&t).into_iter().flatten() {
                let sub = &self.routines[s];
                if let Some(sk) = sub.parameters.iter().find(|p| lc(&p.name) == pname) {
                    add(
                        &mut out,
                        self.out_mapped(i, vars, cs, s, sk.index, Some(callee)),
                    );
                }
            }
            return out;
        }
        self.out_mapped(i, vars, cs, t, k, None)
    }

    /// `out_of` for one routine `t` whose parameter `k` the argument at the
    /// same position of `cs` binds (for a subscriber, `publisher`'s).
    fn out_mapped(
        &self,
        i: usize,
        vars: &HashMap<String, Atoms>,
        cs: &PCallSite,
        t: usize,
        k: u32,
        publisher: Option<&ModelRoutine>,
    ) -> Atoms {
        let callee = &self.routines[t];
        if !callee.body_available || callee.parse_incomplete {
            return top();
        }
        let Some(p) = callee.parameters.iter().find(|p| p.index == k) else {
            return top();
        };
        let Some(atoms) = self.summaries[t].vars.get(&lc(&p.name)) else {
            // A record parameter has a fixed table: the record passed in
            // keeps it. Any other type may hand anything back.
            return if p.is_record { Atoms::new() } else { top() };
        };
        if keeps_by_value(callee, &self.facts[t], p) {
            return Atoms::new();
        }
        let self_call =
            publisher.is_none() && on_self(cs) && callee.object_id == self.routines[i].object_id;
        let mut out = Atoms::new();
        for a in atoms {
            match a {
                Atom::Param(j) if *j == k => {}
                Atom::Param(j) => {
                    // The caller's argument at the callee's position j (for a
                    // subscriber, at the publisher's parameter of that name).
                    let pos = match publisher {
                        None => Some(*j),
                        Some(pb) => callee
                            .parameters
                            .iter()
                            .find(|x| x.index == *j)
                            .and_then(|x| pb.parameters.iter().find(|y| lc(&y.name) == lc(&x.name)))
                            .map(|y| y.index),
                    };
                    match pos.and_then(|pos| cs.argument_texts.get(pos as usize)) {
                        Some(arg) => {
                            add(&mut out, self.atoms_of(i, vars, arg));
                        }
                        None => return top(),
                    }
                }
                Atom::Global(g) if self_call => {
                    out.insert(Atom::Global(Arc::clone(g)));
                }
                Atom::Global(_) | Atom::Top => return top(),
                Atom::Table(_) | Atom::Unset => {
                    out.insert(a.clone());
                }
            }
        }
        out
    }

    /// What a variable passed at position `k` of call site `cs` of `i` may
    /// hold after the call, beyond what it held; `None` when the call keeps it.
    fn passed_atoms(
        &self,
        i: usize,
        vars: &HashMap<String, Atoms>,
        cs: &PCallSite,
        k: u32,
    ) -> Option<Atoms> {
        if self.open_sites.contains(cs.id.as_str()) {
            return Some(top());
        }
        match self.targets.get(cs.id.as_str()) {
            Some(ts) => {
                let mut out = Atoms::new();
                for &t in ts {
                    add(&mut out, self.out_of(i, vars, cs, t, k));
                }
                Some(out)
            }
            None if !writes_arguments(cs)
                && (self.builtin_sites.contains(cs.id.as_str())
                    || keeps_unresolved(&self.routines[i], cs)) =>
            {
                None
            }
            None => Some(top()),
        }
    }

    fn summarize(&self, i: usize) -> Summary {
        let r = &self.routines[i];
        let f = &self.facts[i];
        let mut s = initial_summary(f);
        // Parts that may read other routines' summaries, resolved once per round.
        let mut parts: Vec<(String, Part)> = f
            .fixed
            .iter()
            .map(|(v, p, _)| (v.clone(), p.clone()))
            .collect();
        for (ci, k, v) in &f.passed {
            let cs = &r.call_sites[*ci];
            if let Some(atoms) = self.passed_atoms(i, &self.summaries[i].vars, cs, *k as u32) {
                parts.push((v.clone(), Part::Atoms(atoms)));
            }
        }
        // Resolve within the routine to a fixpoint.
        loop {
            let mut changed = false;
            for (v, part) in &parts {
                let atoms = match part {
                    Part::Atoms(a) => a.clone(),
                    Part::Copy(w) => self.atoms_of(i, &s.vars, w),
                };
                let into = match f.tracked.get(v) {
                    Some(VarKind::Global) => s.global_writes.entry(v.clone()).or_default(),
                    Some(_) => s.vars.entry(v.clone()).or_default(),
                    None => continue,
                };
                changed |= add(into, atoms);
            }
            if !changed {
                break;
            }
        }
        s
    }

    /// Per object, each routine's writes to each global over its self-call
    /// closure, in its own terms: its own writes, and a self-call's (read
    /// across the call, a global of the instance staying itself).
    fn solve_closures(&mut self) {
        let mut by_object: HashMap<&'a str, Vec<usize>> = HashMap::new();
        for (i, r) in self.routines.iter().enumerate() {
            if self.facts[i]
                .tracked
                .values()
                .any(|k| *k == VarKind::Global)
            {
                by_object.entry(r.object_id.as_str()).or_default();
            }
        }
        for (i, r) in self.routines.iter().enumerate() {
            if let Some(v) = by_object.get_mut(r.object_id.as_str()) {
                v.push(i);
            }
        }
        for (obj, members) in by_object {
            let mut w: HashMap<usize, HashMap<String, Atoms>> = members
                .iter()
                .map(|&m| (m, self.summaries[m].global_writes.clone()))
                .collect();
            let mut converged = false;
            for _ in 0..MAX_ROUNDS {
                let mut changed = false;
                for &m in &members {
                    let r = &self.routines[m];
                    let mut mine = w[&m].clone();
                    for cs in &r.call_sites {
                        if !on_self(cs) {
                            continue;
                        }
                        for &t in self.targets.get(cs.id.as_str()).into_iter().flatten() {
                            if self.routines[t].object_id != r.object_id {
                                continue;
                            }
                            let Some(tw) = w.get(&t) else { continue };
                            for (g, atoms) in tw {
                                let mapped = self.map_into_caller(m, cs, atoms);
                                changed |= add(mine.entry(g.clone()).or_default(), mapped);
                            }
                        }
                    }
                    w.insert(m, mine);
                }
                if !changed {
                    converged = true;
                    break;
                }
            }
            if converged {
                self.closure.insert(obj, w);
            }
        }
    }

    /// A callee's atoms across a self-call `cs` of `i`: its parameters become
    /// the arguments, a global stays itself.
    fn map_into_caller(&self, i: usize, cs: &PCallSite, atoms: &Atoms) -> Atoms {
        let mut out = Atoms::new();
        for a in atoms {
            match a {
                Atom::Param(j) => match cs.argument_texts.get(*j as usize) {
                    Some(arg) => {
                        add(&mut out, self.atoms_of(i, &self.summaries[i].vars, arg));
                    }
                    None => return top(),
                },
                _ => {
                    add(&mut out, [a.clone()]);
                }
            }
        }
        out
    }

    /// The atoms of the argument at position `k` of call site `cs` of `i`.
    pub(crate) fn arg_atoms(&self, i: usize, cs: &PCallSite, k: u32) -> Atoms {
        match cs.argument_texts.get(k as usize) {
            Some(a) => self.atoms_of(i, &self.summaries[i].vars, a),
            None => top(),
        }
    }

    /// Whether `name` is a tracked variable of `i`, and how.
    pub(crate) fn initial_atom(&self, i: usize, name: &str) -> Option<Atom> {
        Some(match self.facts[i].tracked.get(name)? {
            VarKind::Param(p) => Atom::Param(*p),
            VarKind::Local => Atom::Unset,
            VarKind::Global => Atom::Global(Arc::from(name)),
        })
    }

    /// Whether call site `cs` of `i` is a call on a codeunit instance `i`
    /// owns (a local codeunit variable, fresh per call, never passed on or
    /// assigned, of a codeunit that is not `SingleInstance` and does not pass
    /// itself on): the receiver's folded name.
    fn own_instance(&self, i: usize, cs: &PCallSite) -> Option<String> {
        let r = &self.routines[i];
        let PCallee::Member { receiver, .. } = &cs.callee else {
            return None;
        };
        let x = lc(receiver);
        let var = r.variables.iter().find(|v| lc(&v.name) == x)?;
        if var.is_parameter
            || var.scope.as_deref() != Some("local")
            || !var
                .declared_type
                .trim()
                .to_ascii_lowercase()
                .starts_with("codeunit")
        {
            return None;
        }
        let escapes = r
            .call_sites
            .iter()
            .any(|c| c.argument_texts.iter().any(|a| lc(a) == x))
            || r.var_assignments
                .iter()
                .any(|a| lc(&a.lhs_name) == x || a.rhs_identifier.as_deref() == Some(x.as_str()));
        (!escapes).then_some(x)
    }

    /// The atoms, in `i`'s terms, the globals of the instance call site `cs`
    /// runs on may hold over that instance's life, for the globals `wanted`
    /// of the callee's object: `Unset`, and whatever every call `i` makes on
    /// that instance may assign them. `None` when the instance is not one `i`
    /// owns, or a call on it is not followed.
    pub(crate) fn instance_globals(
        &self,
        i: usize,
        cs: &PCallSite,
        callee: usize,
        wanted: &[Arc<str>],
    ) -> Option<Vec<(Arc<str>, Atoms)>> {
        if wanted.is_empty() {
            return Some(Vec::new());
        }
        let obj = self.routines[callee].object_id.as_str();
        if self.single_instance.get(obj).copied().unwrap_or(true) || self.self_escapes.contains(obj)
        {
            return None;
        }
        let closure = self.closure.get(obj)?;
        let x = self.own_instance(i, cs)?;
        let r = &self.routines[i];
        let mut out: Vec<(Arc<str>, Atoms)> = wanted
            .iter()
            .map(|g| (Arc::clone(g), Atoms::from([Atom::Unset])))
            .collect();
        for c in &r.call_sites {
            let PCallee::Member { receiver, .. } = &c.callee else {
                continue;
            };
            if lc(receiver) != x {
                continue;
            }
            let ts = self.targets.get(c.id.as_str())?;
            for &t in ts {
                if self.routines[t].object_id != obj {
                    return None;
                }
                let Some(w) = closure.get(&t) else {
                    // A routine with no tracked global writes none.
                    continue;
                };
                for (g, atoms) in out.iter_mut() {
                    let Some(written) = w.get(&**g) else { continue };
                    for a in written {
                        match a {
                            Atom::Param(j) => match c.argument_texts.get(*j as usize) {
                                Some(arg) => {
                                    add(atoms, self.atoms_of(i, &self.summaries[i].vars, arg));
                                }
                                None => {
                                    add(atoms, top());
                                }
                            },
                            // The same global keeps a value it already holds.
                            Atom::Global(h) if h == g => {}
                            Atom::Global(_) | Atom::Top => {
                                add(atoms, top());
                            }
                            Atom::Table(_) | Atom::Unset => {
                                atoms.insert(a.clone());
                            }
                        }
                    }
                }
            }
        }
        Some(out)
    }

    /// Whether call site `cs` of `i` is a self-call (it keeps the instance).
    pub(crate) fn is_self_call(&self, i: usize, cs: &PCallSite, callee: usize) -> bool {
        on_self(cs) && self.routines[callee].object_id == self.routines[i].object_id
    }

    /// The parameters and globals whose values matter, given `seeds` (atoms
    /// a guard reads, per routine): a callee's relevant parameter makes the
    /// atoms of every argument passed to it relevant in its caller, and a
    /// relevant global makes relevant what assigns it.
    pub(crate) fn relevance(
        &self,
        seeds: impl IntoIterator<Item = (usize, Atom)>,
    ) -> (RelevantParams, RelevantGlobals<'a>) {
        let mut params: RelevantParams = HashMap::new();
        let mut globals: RelevantGlobals<'a> = HashMap::new();
        let mut q: VecDeque<(usize, Atom)> = seeds.into_iter().collect();
        // Callee -> (caller, call site index).
        let mut callers: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for (i, r) in self.routines.iter().enumerate() {
            for (ci, cs) in r.call_sites.iter().enumerate() {
                for &t in self.targets.get(cs.id.as_str()).into_iter().flatten() {
                    callers.entry(t).or_default().push((i, ci));
                }
            }
        }
        // Object -> routines.
        let mut members: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in self.routines.iter().enumerate() {
            members.entry(r.object_id.as_str()).or_default().push(i);
        }
        while let Some((i, a)) = q.pop_front() {
            match a {
                Atom::Param(p) => {
                    if !params.entry(i).or_default().insert(p) {
                        continue;
                    }
                    for &(f, ci) in callers.get(&i).into_iter().flatten() {
                        let cs = &self.routines[f].call_sites[ci];
                        for b in self.arg_atoms(f, cs, p) {
                            q.push_back((f, b));
                        }
                    }
                }
                Atom::Global(g) => {
                    let obj = self.routines[i].object_id.as_str();
                    if !globals.entry(obj).or_default().insert(Arc::clone(&g)) {
                        continue;
                    }
                    let Some(closure) = self.closure.get(obj) else {
                        continue;
                    };
                    for &m in members.get(obj).into_iter().flatten() {
                        // What assigns it, in each member's terms, and the
                        // arguments of the calls made on an instance of it.
                        for b in closure
                            .get(&m)
                            .and_then(|w| w.get(&*g))
                            .into_iter()
                            .flatten()
                        {
                            q.push_back((m, b.clone()));
                        }
                        for &(f, ci) in callers.get(&m).into_iter().flatten() {
                            let cs = &self.routines[f].call_sites[ci];
                            if self.own_instance(f, cs).is_none() {
                                continue;
                            }
                            for b in closure
                                .get(&m)
                                .and_then(|w| w.get(&*g))
                                .into_iter()
                                .flatten()
                            {
                                if let Atom::Param(j) = b {
                                    for c in self.arg_atoms(f, cs, *j) {
                                        q.push_back((f, c));
                                    }
                                }
                            }
                        }
                    }
                }
                Atom::Table(_) | Atom::Unset | Atom::Top => {}
            }
        }
        (params, globals)
    }
}

/// `atoms` under known parameter tables and globals: the tables, or `None`
/// when one atom is not known.
pub(crate) fn eval(
    atoms: &Atoms,
    params: &[(u32, TSet)],
    globals: &[(Arc<str>, TSet)],
) -> Option<TSet> {
    let mut out: BTreeSet<Arc<str>> = BTreeSet::new();
    for a in atoms {
        match a {
            Atom::Table(t) => {
                out.insert(Arc::clone(t));
            }
            Atom::Unset => {
                out.insert(Arc::from(""));
            }
            Atom::Param(i) => {
                let (_, s) = params.iter().find(|(j, _)| j == i)?;
                out.extend(s.iter().cloned());
            }
            Atom::Global(g) => {
                let (_, s) = globals.iter().find(|(h, _)| h == g)?;
                out.extend(s.iter().cloned());
            }
            Atom::Top => return None,
        }
    }
    Some(out.into_iter().collect())
}

/// [`eval`] with inputs that may themselves be anything.
fn eval_val(atoms: &Atoms, inputs: &[(u32, Val)], globals: &[(Arc<str>, Val)]) -> Val {
    let params: Vec<(u32, TSet)> = inputs
        .iter()
        .filter_map(|(j, v)| v.as_ref().map(|s| (*j, Arc::clone(s))))
        .collect();
    let globals: Vec<(Arc<str>, TSet)> = globals
        .iter()
        .filter_map(|(g, v)| v.as_ref().map(|s| (Arc::clone(g), Arc::clone(s))))
        .collect();
    eval(atoms, &params, &globals)
}

fn initial_summary(f: &Facts) -> Summary {
    let mut s = Summary::default();
    for (name, kind) in &f.tracked {
        let a = match kind {
            VarKind::Param(p) => Atom::Param(*p),
            VarKind::Local => Atom::Unset,
            VarKind::Global => continue,
        };
        s.vars.insert(name.clone(), Atoms::from([a]));
    }
    s
}

/// What member call `cs` of `r` on tracked `recv` binds it to, when it
/// rebinds it in place: `GetTable(X)` what `X` holds, `Close` no table, `Open`
/// and `Copy` anything, and `Get(Id)` anything unless it keeps the table.
fn in_place_part(r: &ModelRoutine, cs: &PCallSite, recv: &str) -> Option<Part> {
    let PCallee::Member { method, .. } = &cs.callee else {
        return None;
    };
    Some(match method.to_ascii_lowercase().as_str() {
        "gettable" => match cs.argument_texts.first() {
            Some(arg) => Part::Copy(lc(arg)),
            None => Part::Atoms(top()),
        },
        "open" | "copy" => Part::Atoms(top()),
        "close" => Part::Atoms(Atoms::from([Atom::Unset])),
        "get" if get_keeps_table(r, cs, recv) => return None,
        "get" => Part::Atoms(top()),
        _ => return None,
    })
}

/// Whether call `cs` runs on the caller's own instance: `Proc(…)`,
/// `this.Proc(…)` or `CurrCodeunit.Proc(…)` (the target must be in the same
/// object).
fn on_self(cs: &PCallSite) -> bool {
    match &cs.callee {
        PCallee::Bare { .. } => true,
        PCallee::Member { receiver, .. } => {
            matches!(lc(receiver).as_str(), "this" | "currcodeunit")
        }
        _ => false,
    }
}

/// A platform method or function that hands a value back through an
/// argument (`Dict.Get(Key, Value)`, `List.Get(I, Value)`, `Evaluate(X, …)`):
/// it may rebind a Variant passed to it.
fn writes_arguments(cs: &PCallSite) -> bool {
    match &cs.callee {
        PCallee::Member { method, .. } => {
            let m = method.to_ascii_lowercase();
            // `R.GetTable(X)` reads `X` (it binds `R`).
            (m.starts_with("get") && m != "gettable")
                || m.starts_with("tryget")
                || matches!(
                    m.as_str(),
                    "record" | "read" | "readfrom" | "selecttoken" | "value" | "remove"
                )
        }
        PCallee::Bare { name } => matches!(lc(name).as_str(), "evaluate" | "copyarray"),
        _ => true,
    }
}

/// An unresolved call that cannot rebind a RecordRef or Variant passed to it:
/// a method of a platform value (a record, a RecordRef, a FieldRef…) or an
/// object's static run/save (`Report.SaveAs(…, RecRef)`), or `Format`.
fn keeps_unresolved(r: &ModelRoutine, cs: &PCallSite) -> bool {
    match &cs.callee {
        PCallee::Member { receiver, method } => {
            let x = lc(receiver);
            if let Some(v) = r.variables.iter().find(|v| lc(&v.name) == x) {
                let t = v.declared_type.trim().to_ascii_lowercase();
                // An object variable's unresolved method could do anything.
                return ![
                    "codeunit",
                    "page",
                    "report",
                    "xmlport",
                    "query",
                    "interface",
                    "dotnet",
                ]
                .iter()
                .any(|k| t.starts_with(k));
            }
            if r.record_variables.iter().any(|v| lc(&v.name) == x) {
                return true;
            }
            // A RecordRef method on a named return value (`procedure
            // GetRecRef(…) RecRef: RecordRef` … `RecRef.GetTable(RecVariant)`):
            // it reads its argument.
            if r.return_type
                .as_deref()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("recordref"))
                && matches!(
                    method.to_ascii_lowercase().as_str(),
                    "gettable" | "settable" | "setview" | "setposition" | "copy"
                )
            {
                return true;
            }
            matches!(x.as_str(), "report" | "page" | "xmlport")
                && matches!(
                    method.to_ascii_lowercase().as_str(),
                    "run"
                        | "runmodal"
                        | "saveas"
                        | "print"
                        | "execute"
                        | "saveasxml"
                        | "saveaspdf"
                        | "saveasexcel"
                        | "saveasword"
                        | "saveashtml"
                )
        }
        // Text formatting and dialogs read their values.
        PCallee::Bare { name } => matches!(
            lc(name).as_str(),
            "format" | "strsubstno" | "error" | "message" | "confirm"
        ),
        _ => false,
    }
}

/// G34: each top-level `if R.Number <> Database::"T" then exit;` of `r`
/// whose `R` holds, at the guard, a value the caller can name: a variable
/// nothing binds before it (no assignment, no method call on it, not passed
/// on), or any variable `anytime` says holds one value at all times (an
/// object global's instance value). Returns `(R, [T], the operation and
/// call-site ids after the guard)`.
pub(crate) fn exit_guards(
    r: &ModelRoutine,
    anytime: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<String>, HashSet<String>)> {
    let mut out: Vec<(String, Vec<String>, HashSet<String>)> = Vec::new();
    let Some(tree) = r.statement_tree.as_ref() else {
        return out;
    };
    let top: &[PCFNNode] = match tree.kind.as_str() {
        "block" => tree.children.as_deref().unwrap_or_default(),
        _ => std::slice::from_ref(tree),
    };
    for (n, stmt) in top.iter().enumerate() {
        let (Some((rec, tables)), Some((line, col, ..))) =
            (&stmt.table_exit_guard, stmt.source_range)
        else {
            continue;
        };
        let before = |a: &crate::program::body::features::PAnchor| {
            (a.start_line, a.start_column) < (line, col)
        };
        let bound_before = !anytime(rec)
            && (r
                .var_assignments
                .iter()
                .any(|a| lc(&a.lhs_name) == *rec && before(&a.source_anchor))
                || r.call_sites.iter().any(|cs| {
                    before(&cs.source_anchor)
                        && (cs.argument_texts.iter().any(|a| lc(a) == *rec)
                            || matches!(&cs.callee, PCallee::Member { receiver, .. }
                                if lc(receiver) == *rec))
                }));
        if bound_before {
            continue;
        }
        let mut ids = HashSet::new();
        for later in &top[n + 1..] {
            subtree_ids(later, &mut ids);
        }
        out.push((rec.clone(), tables.clone(), ids));
    }
    out
}

/// The operation and call-site ids in `node`'s subtree.
fn subtree_ids(node: &PCFNNode, out: &mut HashSet<String>) {
    for id in [&node.operation_id, &node.callsite_id]
        .into_iter()
        .flatten()
    {
        out.insert(id.clone());
    }
    for c in node
        .condition_leaves
        .iter()
        .chain(&node.children)
        .chain(&node.else_children)
        .flatten()
    {
        subtree_ids(c, out);
    }
}

/// Whether positions `a` and `b` of one routine lie in different branches of
/// one `if` or `case` that no loop encloses, so neither runs after the other
/// in one call.
fn exclusive(n: &PCFNNode, a: (u32, u32), b: (u32, u32)) -> bool {
    let holds = |c: &PCFNNode, p: (u32, u32)| {
        c.source_range
            .is_some_and(|(sl, sc, el, ec)| (sl, sc) <= p && p < (el, ec))
    };
    if matches!(n.kind.as_str(), "while" | "repeat" | "for" | "foreach") {
        return false;
    }
    let branches: Vec<&PCFNNode> = match n.kind.as_str() {
        "if" => n
            .children
            .iter()
            .chain(&n.else_children)
            .flatten()
            .collect(),
        "case" => n.children.iter().flatten().collect(),
        _ => Vec::new(),
    };
    let (ba, bb) = (
        branches.iter().position(|c| holds(c, a)),
        branches.iter().position(|c| holds(c, b)),
    );
    if let (Some(x), Some(y)) = (ba, bb)
        && x != y
    {
        return true;
    }
    n.children
        .iter()
        .chain(&n.else_children)
        .flatten()
        .find(|c| holds(c, a) && holds(c, b))
        .is_some_and(|c| exclusive(c, a, b))
}

/// Whether `V.GetTable(X)` at `cs` runs only past an exit guard that `V`
/// held table `T`, with `X` a record of table `T`: `V` then already held `T`,
/// so the rebinding adds no table (CDO File's `CalcSalesDisc` puts its own
/// Sales Header back after `if RecRef.Number <> Database::"Sales Header"
/// then exit;`).
fn rebinds_to_guarded(
    r: &ModelRoutine,
    guards: &[(String, Vec<String>, HashSet<String>)],
    cs: &PCallSite,
    v: &str,
    part: &Part,
) -> bool {
    let Part::Copy(x) = part else {
        return false;
    };
    let Some(table) = r
        .record_variables
        .iter()
        .find(|rv| lc(&rv.name) == *x)
        .and_then(|rv| rv.table_name.as_deref())
        .map(lc)
    else {
        return false;
    };
    guards.iter().any(|(rec, tables, ids)| {
        rec == v && tables.len() == 1 && tables[0] == table && ids.contains(&cs.id)
    })
}

fn facts_of(r: &ModelRoutine, obj_in_place: &HashSet<String>) -> Facts {
    let mut f = Facts::default();
    for v in r.variables.iter() {
        if !is_tracked_type(&v.declared_type) {
            continue;
        }
        let kind = if v.is_parameter {
            match v.parameter_index {
                Some(p) => VarKind::Param(p),
                None => continue,
            }
        } else if v.scope.as_deref() == Some("global") {
            VarKind::Global
        } else {
            VarKind::Local
        };
        f.tracked.insert(lc(&v.name), kind);
    }
    if f.tracked.is_empty() {
        return f;
    }
    let guards = exit_guards(r, |n| f.tracked.get(n) == Some(&VarKind::Global));
    // Aliases: `A := B` between tracked variables shares one RecordRef (a
    // Variant holds the reference), unlike `A := B.Duplicate()`. Rebinding one
    // in place (`Open`, `GetTable`…) after the assignment rebinds every member
    // of its alias class, so each member may then hold anything. An object
    // global rebound in place in any routine of the object may be at any time.
    let mut in_place: InPlace = HashMap::new();
    for cs in &r.call_sites {
        if let PCallee::Member { receiver, .. } = &cs.callee
            && f.tracked.contains_key(&lc(receiver))
            && let Some(part) = in_place_part(r, cs, &lc(receiver))
        {
            in_place.entry(lc(receiver)).or_default().push((
                (cs.source_anchor.start_line, cs.source_anchor.start_column),
                part,
            ));
        }
    }
    // Union-find over the alias assignments: name -> parent.
    let mut class: HashMap<String, String> = HashMap::new();
    fn find(class: &HashMap<String, String>, x: &str) -> String {
        let mut cur = x.to_string();
        while let Some(p) = class.get(&cur) {
            if *p == cur {
                break;
            }
            cur = p.clone();
        }
        cur
    }
    let mut alias_at: Vec<(String, (u32, u32))> = Vec::new();
    for a in &r.var_assignments {
        let l = lc(&a.lhs_name);
        if !f.tracked.contains_key(&l) {
            continue;
        }
        let part = match (&a.rhs_identifier, &a.rhs_member) {
            (Some(w), _) => {
                let w = lc(w);
                if f.tracked.contains_key(&w) {
                    class.entry(l.clone()).or_insert_with(|| l.clone());
                    class.entry(w.clone()).or_insert_with(|| w.clone());
                    let (rl, rw) = (find(&class, &l), find(&class, &w));
                    class.insert(rl, rw);
                    alias_at.push((
                        l.clone(),
                        (a.source_anchor.start_line, a.source_anchor.start_column),
                    ));
                }
                Part::Copy(w)
            }
            (None, Some((w, m))) if m == "duplicate" => Part::Copy(lc(w)),
            _ => Part::Atoms(top()),
        };
        let at = (a.source_anchor.start_line, a.source_anchor.start_column);
        f.fixed.push((l, part, Some(at)));
    }
    if !alias_at.is_empty() {
        let names: Vec<String> = class.keys().cloned().collect();
        for root in names
            .iter()
            .map(|n| find(&class, n))
            .collect::<HashSet<_>>()
        {
            let members: Vec<&String> = names.iter().filter(|n| find(&class, n) == root).collect();
            let assigns: Vec<(u32, u32)> = alias_at
                .iter()
                .filter(|(l, _)| members.contains(&l))
                .map(|(_, p)| *p)
                .collect();
            // What a rebinding that may run after an alias assignment binds.
            let mut shared: Vec<(Part, Option<(u32, u32)>)> = Vec::new();
            for m in &members {
                if f.tracked.get(*m) == Some(&VarKind::Global) && obj_in_place.contains(*m) {
                    shared.push((Part::Atoms(top()), None));
                }
                for (s, part) in in_place.get(*m).into_iter().flatten() {
                    let may_follow = assigns.iter().any(|&p| {
                        !r.statement_tree
                            .as_ref()
                            .is_some_and(|t| exclusive(t, p, *s))
                    });
                    if may_follow {
                        shared.push((part.clone(), Some(*s)));
                    }
                }
            }
            for m in members {
                if !shared.is_empty() {
                    f.alias_shared.insert(m.clone());
                }
                for (part, at) in &shared {
                    f.fixed.push((m.clone(), part.clone(), *at));
                }
            }
        }
    }
    for (ci, cs) in r.call_sites.iter().enumerate() {
        let at = Some((cs.source_anchor.start_line, cs.source_anchor.start_column));
        if let PCallee::Member { receiver, .. } = &cs.callee {
            let v = lc(receiver);
            if f.tracked.contains_key(&v) {
                if let Some(part) = in_place_part(r, cs, &v)
                    && !rebinds_to_guarded(r, &guards, cs, &v, &part)
                {
                    f.fixed.push((v, part, at));
                }
                continue;
            }
        }
        if let PCallee::Bare { name } = &cs.callee {
            match lc(name).as_str() {
                "clear" => {
                    if let Some(v) = cs.argument_texts.first().map(|a| lc(a))
                        && f.tracked.contains_key(&v)
                    {
                        f.fixed
                            .push((v, Part::Atoms(Atoms::from([Atom::Unset])), at));
                    }
                    continue;
                }
                "clearall" => {
                    for (v, k) in &f.tracked {
                        if *k == VarKind::Global {
                            f.fixed
                                .push((v.clone(), Part::Atoms(Atoms::from([Atom::Unset])), at));
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        for (k, a) in cs.argument_texts.iter().enumerate() {
            let v = lc(a);
            if f.tracked.contains_key(&v) {
                f.passed.push((ci, k, v));
            }
        }
    }
    // G35: the conditions each fact runs under.
    let regions = cond_regions(r, &f.tracked);
    let conds_at = |at: Option<(u32, u32)>| -> Vec<Cond> {
        let Some(p) = at else {
            return Vec::new();
        };
        regions
            .iter()
            .filter(|(from, to, _)| *from <= p && p < *to)
            .map(|(_, _, c)| c.clone())
            .collect()
    };
    f.fixed_conds = f.fixed.iter().map(|(_, _, at)| conds_at(*at)).collect();
    f.passed_conds = f
        .passed
        .iter()
        .map(|(ci, _, _)| {
            let a = &r.call_sites[*ci].source_anchor;
            conds_at(Some((a.start_line, a.start_column)))
        })
        .collect();
    f
}

/// G35: the regions of `r` that run only under a condition on a tracked
/// RecordRef's table: each branch of a `case R.Number of Database::"T": …`
/// (an `else` for the tables no branch names, when every branch names
/// tables), and what follows a top-level `if R.Number <> Database::"T" then
/// exit;`. The condition is on the table `R` holds when the `case` or the
/// guard runs, which is one of the tables `R` may hold at all.
fn cond_regions(r: &ModelRoutine, tracked: &HashMap<String, VarKind>) -> Vec<CondRegion> {
    let mut out = Vec::new();
    let Some(tree) = r.statement_tree.as_ref() else {
        return out;
    };
    fn walk(n: &PCFNNode, tracked: &HashMap<String, VarKind>, out: &mut Vec<CondRegion>) {
        if n.kind == "case"
            && let Some(TableCase::On(rv)) = &n.table_case
            && tracked.contains_key(rv)
        {
            let branches: Vec<&PCFNNode> = n.children.iter().flatten().collect();
            let mut listed: Vec<String> = Vec::new();
            let mut all_named = true;
            for b in &branches {
                match &b.table_case {
                    Some(TableCase::Tables(t)) => listed.extend(t.iter().cloned()),
                    _ if b.is_case_else => {}
                    _ => all_named = false,
                }
            }
            let listed: Arc<[String]> = listed.into();
            for b in &branches {
                let Some((sl, sc, el, ec)) = b.source_range else {
                    continue;
                };
                let cond = match &b.table_case {
                    Some(TableCase::Tables(t)) => Some((rv.clone(), true, t.clone().into())),
                    _ if b.is_case_else && all_named => {
                        Some((rv.clone(), false, Arc::clone(&listed)))
                    }
                    _ => None,
                };
                if let Some(c) = cond {
                    out.push(((sl, sc), (el, ec), c));
                }
            }
        }
        for c in n.children.iter().chain(&n.else_children).flatten() {
            walk(c, tracked, out);
        }
    }
    walk(tree, tracked, &mut out);
    let top: &[PCFNNode] = match tree.kind.as_str() {
        "block" => tree.children.as_deref().unwrap_or_default(),
        _ => std::slice::from_ref(tree),
    };
    for stmt in top {
        if let (Some((rv, tables)), Some((_, _, el, ec))) =
            (&stmt.table_exit_guard, stmt.source_range)
            && tracked.contains_key(rv)
        {
            out.push((
                (el, ec),
                (u32::MAX, u32::MAX),
                (rv.clone(), true, tables.clone().into()),
            ));
        }
    }
    out
}

/// G35: a by-value Variant parameter is the callee's own copy: rebinding it
/// does not reach the caller, unless an alias rebinding of a RecordRef it
/// holds does (`R := Variant; R.Open(…)`).
fn keeps_by_value(_callee: &ModelRoutine, f: &Facts, p: &ModelParameter) -> bool {
    !p.is_var
        && p.type_text.trim().eq_ignore_ascii_case("variant")
        && !f.alias_shared.contains(&lc(&p.name))
}

/// `R.Get(Id)` keeps `R`'s table when `Id` is a local whose every assignment
/// is `Id := R.RecordId` and that nothing else is passed to.
fn get_keeps_table(r: &ModelRoutine, cs: &PCallSite, recref: &str) -> bool {
    let [id] = cs.argument_texts.as_slice() else {
        return false;
    };
    let id = lc(id);
    let local = r
        .variables
        .iter()
        .any(|v| lc(&v.name) == id && !v.is_parameter && v.scope.as_deref() == Some("local"));
    if !local {
        return false;
    }
    let mut assigned = false;
    for a in r.var_assignments.iter().filter(|a| lc(&a.lhs_name) == id) {
        match &a.rhs_member {
            Some((w, m)) if lc(w) == recref && m == "recordid" => assigned = true,
            _ => return false,
        }
    }
    assigned
        && r.call_sites
            .iter()
            .filter(|c| c.id != cs.id)
            .all(|c| c.argument_texts.iter().all(|a| lc(a) != id))
}
