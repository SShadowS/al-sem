//! r3 E2 (spec `2026-11-08-output-validation-r3.md` §1 E2): values inside a
//! routine, and `var` parameters passed to an event publisher (n01).
//!
//! Continia DO's `CreateAndSendMail` takes `UseSMTP` by value, may set it
//! (`if UseHandleDoc then UseSMTP := true`), passes it `var` to the publisher
//! `OnBeforeCreateAndSendMailV2` and derives `IsEmailPreview := not UseSMTP`;
//! the COMMIT in `OpenDOMail` runs only under `if IsEmailPreview`. A caller
//! passing `UseSMTP = true` cannot reach it, unless a subscriber of the
//! publisher changes `UseSMTP`. [`literal_values`](super::values) binds only
//! parameters that never change, so it could not see this.
//!
//! **Tracked variables** ([`FlowProgram`]): the routine's locals and by-value
//! parameters of type Boolean, Integer, BigInteger, Option or Enum, and its
//! `var` parameters of those types from their first assignment in the
//! routine. Two `var` formals can name the same actual (`P(X, X)`), and a
//! `var` formal can name a global the routine writes, so a tracked `var`
//! formal becomes unknown at any write to another `var` formal, to a global
//! of a compatible type (or a name that may be one, or a member), and at any
//! call or database operation (a callee or trigger may write the aliased
//! actual). Globals are never tracked. A local starts at its type's default
//! value (`false`, `0`, the first Option member; an Enum local starts
//! unknown); a by-value parameter at its bound entry value, else unknown.
//!
//! **Order** ([`Exec`]): each statement is evaluated with its incoming state,
//! then its writes and call side effects apply. A condition or an assigned
//! value is read AFTER the calls inside it ran (a call that writes a variable
//! the expression reads makes it unknown: sound whatever the evaluation
//! order). **Values**: literals, labels (read as [`super::values`] reads
//! them), other tracked variables, `not`/`and`/`or`, `=`/`<>`/`in`. Anything
//! else is unknown. Passing a variable to a `var` parameter, to `Evaluate`,
//! `Clear` (and the few other platform methods that write an argument), or
//! to a call the resolver did not resolve writes it (unknown), except to a
//! publisher whose subscribers are proven not to change it (below).
//! **Joins**: every feasible incoming branch joins; a set over [`MAX_SET`]
//! values is unknown. **Loops**: a variable written anywhere in a loop
//! (its counter, writes through calls included) is unknown at every read in
//! the loop, before the write too, and after the loop.
//!
//! **Publisher `var` parameters** ([`keep_verdict`]): passing a tracked
//! variable `var` to an event publisher keeps its value only when every loaded
//! subscriber of the event (this model is the universe: a test universe's
//! subscribers are in its model) is proven unable to change the matching
//! parameter: a subscriber that assigns it, passes it on `var` (to a helper or
//! another publisher), to a call not resolved, or to `Evaluate`/`Clear`
//! changes it; a subscriber whose body is not available, an event with a
//! subscription not bound to a publisher, or any model subscriber the event
//! graph could not read, prevents keeping it. The kept value rests on the
//! loaded-subscribers assumption: [`FlowEval::keeps`] names the verdicts a
//! state used, so a reader can state it and check what it decides.
//!
//! A site is DEAD under a routine's entry values when no feasible path of the
//! routine reaches it ([`FlowEval`]).

use super::values::{self, Domain, VSet, Val};
use crate::program::body::features::{
    FlowExpr, FlowFact, PCFNNode, PathHint, ValueFact, ValueLabel,
};
use crate::program::model::workspace::ModelRoutine;
use al_syntax::IdentifierFoldExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The most values a variable's set holds before it counts as unknown.
const MAX_SET: usize = 16;

/// The platform methods that write a variable passed to them (besides a
/// `var` parameter of a resolved routine): `Evaluate(var X, ..)`, `Clear(X)`,
/// `Dictionary.Get(K, var V)`, `InStream.Read(var X)`, `StartSession(var
/// Id, ..)`, `JsonToken.SelectToken(.., var T)` and their kin. A variable
/// passed to any other platform method keeps its value.
pub(super) const WRITING_BUILTINS: &[&str] = &[
    "evaluate",
    "clear",
    "get",
    "read",
    "readfrom",
    "startsession",
    "selecttoken",
    "selectsinglenode",
    "trygetvalue",
];

/// The domain a tracked variable's values are compared in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FDom {
    Bool,
    Val(Domain),
    /// Round-4 literal routes: an `Interface "I"` local (the interface's
    /// name, folded, last segment). Its values are the enum values it was
    /// assigned from: each selects that value's implementation of `I`.
    Iface(Arc<str>),
    /// Round-4 literal routes: a `Variant`. Its values are the static types
    /// of what it holds ([`Val::Type`]), which decide its type tests
    /// (`IsRecord`, `IsCodeunit`, …).
    Variant,
}

/// The domain of a variable of type `type_text`; `None` for a type the flow
/// does not track.
pub(super) fn flow_domain(type_text: &str) -> Option<FDom> {
    if type_text.trim().eq_fold_identifier("boolean") {
        return Some(FDom::Bool);
    }
    if type_text.trim().eq_fold_identifier("variant") {
        return Some(FDom::Variant);
    }
    if let Some(i) = values::interface_name(type_text) {
        return Some(FDom::Iface(i));
    }
    values::domain_of(type_text).map(FDom::Val)
}

/// `v` in domain `d` after AL's conversion, when it is modelled. An
/// interface takes an enum value (its implementation is chosen then); a
/// Variant takes a static type (a value assigned to one keeps its type).
pub(super) fn conv(v: &Val, d: &FDom) -> Option<Val> {
    match (v, d) {
        (Val::Enum(..), FDom::Iface(_)) | (Val::Type(_), FDom::Variant) => Some(v.clone()),
        (_, FDom::Iface(_) | FDom::Variant) | (Val::Type(_), _) => None,
        (Val::Bool(b), FDom::Bool) => Some(Val::Bool(*b)),
        (Val::Bool(_), FDom::Val(_)) | (_, FDom::Bool) => None,
        (_, FDom::Val(d)) => values::in_domain(v, d),
    }
}

/// `s` in domain `d`, sorted and distinct; `None` when one value does not
/// convert.
pub(super) fn conv_set(s: &[Val], d: &FDom) -> Option<VSet> {
    let mut out = s.iter().map(|v| conv(v, d)).collect::<Option<Vec<Val>>>()?;
    out.sort();
    out.dedup();
    Some(out.into())
}

/// Whether a `var` formal of domain `a` can name a global of domain `b`.
fn compatible(a: &FDom, b: &FDom) -> bool {
    matches!(
        (a, b),
        (FDom::Bool, FDom::Bool)
            | (FDom::Val(Domain::Int), FDom::Val(Domain::Int))
            | (FDom::Val(Domain::Enum(_)), FDom::Val(Domain::Enum(_)))
            | (FDom::Iface(_), FDom::Iface(_))
            | (FDom::Variant, FDom::Variant)
    )
}

fn one(v: Val) -> VSet {
    Arc::from([v])
}

fn bools(s: &[Val]) -> Option<Vec<bool>> {
    s.iter()
        .map(|v| match v {
            Val::Bool(b) => Some(*b),
            _ => None,
        })
        .collect()
}

fn of_bools(mut b: Vec<bool>) -> VSet {
    b.sort_unstable();
    b.dedup();
    b.into_iter().map(Val::Bool).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Local,
    Param(u32),
    VarParam,
}

#[derive(Debug)]
struct TVar {
    dom: FDom,
    kind: Kind,
}

/// What a call does to one tracked variable passed to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArgWrite {
    /// It may change it.
    Havoc,
    /// A publisher whose loaded subscribers cannot change it: the keep
    /// verdict's id ([`keep_verdict`]).
    Keep(u32),
}

/// One routine's flow program (module doc). Built only for a routine whose
/// conditions read a tracked variable or whose calls pass a value the flow
/// may know; a routine with a `with` statement has none (a bare name inside
/// it may be a field).
pub(super) struct FlowProgram {
    tree: Arc<PCFNNode>,
    vars: Vec<TVar>,
    by_name: HashMap<String, u16>,
    /// The object's globals the routine can see, folded, with their domain
    /// (`None`: a type the flow does not track).
    globals: HashMap<String, Option<FDom>>,
    /// Per call site: the tracked variables it may write.
    writes: HashMap<String, Vec<(u16, ArgWrite)>>,
    /// Site id (operation or call site) → its index.
    sites: HashMap<String, u32>,
    /// An `error-call` operation id → the call site at its position (as
    /// `param_guard::alias_error_ops` pairs them).
    alias: HashMap<String, String>,
    /// By-value parameters (index, slot).
    params: Vec<(u32, u16)>,
    /// Round-4 literal routes: per call site on a tracked interface local
    /// (an interface dispatch), the local's slot.
    recv: HashMap<String, u16>,
    /// Relevance (module [`relevance`]): the parameters whose entry value
    /// can decide a condition, bit per parameter index (< 64).
    pub(super) guard_mask: u64,
    /// Per call site and argument index: the parameters its value reads.
    pub(super) arg_masks: Vec<(String, u32, u64)>,
    /// Whether a condition reads a tracked variable.
    pub(super) has_guard: bool,
    /// The (call site, argument index) pairs whose value the flow may know.
    pub(super) known_args: Vec<(String, u32)>,
    /// The by-value parameters whose entry value the evaluation reads (the
    /// fixpoint of [`relevance`]), sorted: the evaluation's cache key.
    pub(super) key_params: Vec<u32>,
}

/// What one relevance walk collects ([`FlowProgram::masks`]).
struct MaskWalk {
    guard: u64,
    args: Vec<(String, u32, u64)>,
    has_guard: bool,
    known: Vec<(String, u32)>,
}

/// The result of one evaluation of a program under one set of entry values.
#[derive(Debug, Default)]
pub(super) struct FlowEval {
    dead: HashSet<u32>,
    /// Per call-site index: the argument values the flow knows there.
    args: HashMap<u32, Vec<(u32, VSet)>>,
    /// The keep verdicts a reachable call used ([`ArgWrite::Keep`]), sorted.
    pub(super) keeps: Vec<u32>,
    /// Every keep verdict the evaluation asked about (followed or not, at a
    /// reachable call or in a loop's write set), sorted. Filled by the
    /// caller that supplies `keep` (the literal reach): an evaluation under
    /// another keep choice that answers these the same way is this one.
    pub(super) consulted: Vec<u32>,
    /// Round-4 literal routes: per reachable interface call site (its index)
    /// the enum values its receiver may hold there (`None`: unknown).
    recv: HashMap<u32, Option<VSet>>,
}

/// Every node of `n`'s subtree, `n` first.
fn visit<'n>(n: &'n PCFNNode, f: &mut dyn FnMut(&'n PCFNNode)) {
    f(n);
    for c in n
        .condition_leaves
        .iter()
        .chain(&n.children)
        .chain(&n.else_children)
        .flatten()
    {
        visit(c, f);
    }
}

/// The tracked names an expression reads.
fn reads<'e>(e: &'e FlowExpr, out: &mut Vec<&'e str>) {
    match e {
        FlowExpr::Var(n) => out.push(n),
        FlowExpr::Not(x) => reads(x, out),
        FlowExpr::And(a, b) | FlowExpr::Or(a, b) => {
            reads(a, out);
            reads(b, out);
        }
        FlowExpr::Cmp { lhs, rhs, .. } => {
            reads(lhs, out);
            reads(rhs, out);
        }
        FlowExpr::In(x, items) => {
            reads(x, out);
            for i in items {
                reads(i, out);
            }
        }
        FlowExpr::TypeTest { var, .. } => out.push(var),
        FlowExpr::Bool(_) | FlowExpr::Label(_) | FlowExpr::Unknown => {}
    }
}

impl FlowProgram {
    /// `r`'s program. `writes_of(call site id, argument index)` says what the
    /// call does to a variable passed there (`None`: nothing).
    pub(super) fn build(
        r: &ModelRoutine,
        writes_of: &mut dyn FnMut(&str, u32) -> Option<ArgWrite>,
    ) -> Option<FlowProgram> {
        let tree = Arc::clone(r.statement_tree.as_ref()?);
        let mut has_with = false;
        visit(&tree, &mut |n| {
            if matches!(n.flow.as_deref(), Some(FlowFact::With)) {
                has_with = true;
            }
        });
        if has_with {
            return None;
        }
        let mut vars = Vec::new();
        let mut by_name = HashMap::new();
        let mut globals = HashMap::new();
        let mut params = Vec::new();
        for v in r.variables.iter() {
            let name = values::fold(&v.name);
            match v.scope.as_deref() {
                Some("global") => {
                    globals
                        .entry(name)
                        .or_insert_with(|| flow_domain(&v.declared_type));
                    continue;
                }
                None => continue,
                _ => {}
            }
            let Some(dom) = flow_domain(&v.declared_type) else {
                continue;
            };
            let kind = if v.is_parameter {
                let Some(p) = v
                    .parameter_index
                    .and_then(|i| r.parameters.iter().find(|p| p.index == i))
                else {
                    continue;
                };
                // Only an interface LOCAL is tracked: a parameter holds an
                // implementation its caller chose, not an enum value.
                if matches!(dom, FDom::Iface(_)) {
                    continue;
                }
                if p.is_var {
                    Kind::VarParam
                } else {
                    Kind::Param(p.index)
                }
            } else {
                Kind::Local
            };
            if by_name.contains_key(&name) || vars.len() >= u16::MAX as usize {
                continue;
            }
            let slot = vars.len() as u16;
            if let Kind::Param(i) = kind {
                params.push((i, slot));
            }
            by_name.insert(name, slot);
            vars.push(TVar { dom, kind });
        }
        if vars.is_empty() {
            return None;
        }
        // What each call does to the tracked variables passed to it.
        let mut writes: HashMap<String, Vec<(u16, ArgWrite)>> = HashMap::new();
        for cs in &r.call_sites {
            for (j, info) in cs.argument_infos.iter().enumerate() {
                let name = match info.kind.as_str() {
                    "identifier" | "quoted_identifier" => values::fold(&info.text),
                    _ => continue,
                };
                let Some(&slot) = by_name.get(&name) else {
                    continue;
                };
                if let Some(w) = writes_of(&cs.id, j as u32) {
                    writes.entry(cs.id.clone()).or_default().push((slot, w));
                }
            }
        }
        let mut recv = HashMap::new();
        for cs in &r.call_sites {
            let crate::program::body::features::PCallee::Member { receiver, .. } = &cs.callee
            else {
                continue;
            };
            if let Some(&slot) = by_name.get(&values::fold(receiver))
                && matches!(vars[slot as usize].dom, FDom::Iface(_))
            {
                recv.insert(cs.id.clone(), slot);
            }
        }
        let mut sites = HashMap::new();
        visit(&tree, &mut |n| {
            for id in [&n.operation_id, &n.callsite_id].into_iter().flatten() {
                let next = sites.len() as u32;
                sites.entry(id.clone()).or_insert(next);
            }
        });
        let mut alias = HashMap::new();
        for op in r.operation_sites.iter().filter(|o| o.kind == "error-call") {
            if sites.contains_key(&op.id) {
                continue;
            }
            let a = &op.source_anchor;
            if let Some(cs) = r.call_sites.iter().find(|cs| {
                cs.source_anchor.start_line == a.start_line
                    && cs.source_anchor.start_column == a.start_column
            }) {
                alias.insert(op.id.clone(), cs.id.clone());
            }
        }
        let mut p = FlowProgram {
            tree,
            vars,
            by_name,
            globals,
            writes,
            sites,
            alias,
            params,
            recv,
            guard_mask: 0,
            arg_masks: Vec::new(),
            has_guard: false,
            known_args: Vec::new(),
            key_params: Vec::new(),
        };
        p.masks();
        (p.has_guard || !p.known_args.is_empty()).then_some(p)
    }

    /// The flow domain of by-value parameter `i`, if tracked.
    pub(super) fn param_dom(&self, i: u32) -> Option<FDom> {
        self.params
            .iter()
            .find(|&&(p, _)| p == i)
            .map(|&(_, s)| self.vars[s as usize].dom.clone())
    }

    /// The relevance masks (flow-insensitive: a variable's mask is every
    /// parameter it may be derived from, through assignments and the
    /// conditions they sit under); whether a condition reads a tracked
    /// variable, and the call arguments whose value the flow may know.
    fn masks(&mut self) {
        let n = self.vars.len();
        let mut mask = vec![0u64; n];
        for &(i, slot) in &self.params {
            if i < 64 {
                mask[slot as usize] = 1 << i;
            }
        }
        let tree = Arc::clone(&self.tree);
        for _ in 0..8 {
            let before = mask.clone();
            let mut w = MaskWalk {
                guard: 0,
                args: Vec::new(),
                has_guard: false,
                known: Vec::new(),
            };
            self.mask_walk(&tree, 0, &mut mask, &mut w);
            self.guard_mask = w.guard;
            self.arg_masks = w.args;
            self.has_guard = w.has_guard;
            self.known_args = w.known;
            if mask == before {
                break;
            }
        }
    }

    fn expr_mask(&self, e: &FlowExpr, mask: &[u64]) -> (u64, bool) {
        let mut names = Vec::new();
        reads(e, &mut names);
        let mut m = 0;
        let mut tracked = false;
        for n in names {
            if let Some(&s) = self.by_name.get(n) {
                m |= mask[s as usize];
                tracked = true;
            }
        }
        (m, tracked)
    }

    fn mask_walk(&self, n: &PCFNNode, ctrl: u64, mask: &mut [u64], w: &mut MaskWalk) {
        let mut inner = ctrl;
        match n.flow.as_deref() {
            Some(FlowFact::Cond { expr, .. }) => {
                let (m, tracked) = self.expr_mask(expr, mask);
                w.guard |= m;
                w.has_guard |= tracked;
                inner |= m;
            }
            Some(FlowFact::Assign {
                target: Some(t),
                value,
                ..
            }) => {
                if let Some(&s) = self.by_name.get(t) {
                    let (m, _) = self.expr_mask(value, mask);
                    mask[s as usize] |= m | ctrl;
                }
            }
            Some(FlowFact::Args(a)) => {
                if let Some(cs) = &n.callsite_id {
                    for (j, e) in a.iter().enumerate() {
                        if *e == FlowExpr::Unknown {
                            continue;
                        }
                        w.known.push((cs.clone(), j as u32));
                        let (m, _) = self.expr_mask(e, mask);
                        if m != 0 {
                            w.args.push((cs.clone(), j as u32, m));
                        }
                    }
                }
            }
            _ => {}
        }
        // Round-4 literal routes: an interface call's receiver decides which
        // implementation runs, as a condition decides a branch.
        if let Some(&s) = n.callsite_id.as_ref().and_then(|cs| self.recv.get(cs)) {
            w.guard |= mask[s as usize];
            w.has_guard = true;
        }
        for c in n.condition_leaves.iter().flatten() {
            self.mask_walk(c, ctrl, mask, w);
        }
        for c in n.children.iter().chain(&n.else_children).flatten() {
            self.mask_walk(c, inner, mask, w);
        }
    }

    /// The index of site `id` (an `error-call` operation through the call
    /// site at its position).
    fn site(&self, id: &str) -> Option<u32> {
        self.sites
            .get(id)
            .or_else(|| self.alias.get(id).and_then(|cs| self.sites.get(cs)))
            .copied()
    }

    /// Whether site `id` cannot run under `ev`.
    pub(super) fn dead(&self, ev: &FlowEval, id: &str) -> bool {
        self.site(id).is_some_and(|i| ev.dead.contains(&i))
    }

    /// The argument values the flow knows at call site `cs` under `ev`.
    pub(super) fn args_at<'e>(&self, ev: &'e FlowEval, cs: &str) -> &'e [(u32, VSet)] {
        self.sites
            .get(cs)
            .and_then(|i| ev.args.get(i))
            .map_or(&[], Vec::as_slice)
    }

    /// The program's slot of by-value parameter `i`, if tracked.
    pub(super) fn tracks_param(&self, i: u32) -> bool {
        self.params.iter().any(|&(p, _)| p == i)
    }

    /// One evaluation under `entry` (values of [`Self::key_params`], each in
    /// the parameter's own domain): `keep(k)` says whether keep verdict `k`
    /// is followed ([`ArgWrite::Keep`]; else that publisher pass writes);
    /// `conds`, when given, receives each condition's value by node address.
    pub(super) fn eval(
        &self,
        entry: &[(u32, VSet)],
        resolve: &dyn Fn(&ValueLabel) -> Option<Val>,
        keep: &dyn Fn(u32) -> bool,
        conds: Option<&mut HashMap<usize, Option<VSet>>>,
    ) -> FlowEval {
        let mut slots: Vec<Option<VSet>> = self
            .vars
            .iter()
            .map(|v| match (&v.kind, &v.dom) {
                (Kind::Local, FDom::Bool) => Some(one(Val::Bool(false))),
                (Kind::Local, FDom::Val(Domain::Int)) => Some(one(Val::Int(0))),
                _ => None,
            })
            .collect();
        for (i, set) in entry {
            if let Some(&(_, slot)) = self.params.iter().find(|(p, _)| p == i) {
                slots[slot as usize] = conv_set(set, &self.vars[slot as usize].dom);
            }
        }
        let mut x = Exec {
            p: self,
            resolve,
            keep,
            seen: HashSet::new(),
            live: HashSet::new(),
            args: HashMap::new(),
            keeps: Vec::new(),
            recv: HashMap::new(),
            loops: Vec::new(),
            conds,
        };
        let mut st = Some(slots);
        x.exec(&self.tree, &mut st);
        let mut keeps = x.keeps;
        keeps.sort_unstable();
        keeps.dedup();
        FlowEval {
            dead: x.seen.difference(&x.live).copied().collect(),
            args: x.args,
            keeps,
            consulted: Vec::new(),
            recv: x.recv,
        }
    }

    /// Round-4 literal routes: at interface call site `cs`, the interface it
    /// dispatches on and the enum values its receiver may hold under `ev`;
    /// `None` when the site is not one on a tracked interface local, was not
    /// reached, or the receiver's value is unknown.
    pub(super) fn receiver_at<'e>(&self, ev: &'e FlowEval, cs: &str) -> Option<(&str, &'e VSet)> {
        let slot = *self.recv.get(cs)?;
        let FDom::Iface(iface) = &self.vars[slot as usize].dom else {
            return None;
        };
        let i = self.sites.get(cs)?;
        let v = ev.recv.get(i)?.as_ref()?;
        Some((iface, v))
    }
}

type Slots = Vec<Option<VSet>>;
/// A state: the tracked variables' values, or `None` where no path runs.
type St = Option<Slots>;

fn join(a: St, b: St) -> St {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => Some(
            a.into_iter()
                .zip(b)
                .map(|(x, y)| match (x, y) {
                    (Some(x), Some(y)) if x == y => Some(x),
                    (Some(x), Some(y)) => {
                        let mut u: Vec<Val> = x.iter().chain(y.iter()).cloned().collect();
                        u.sort();
                        u.dedup();
                        (u.len() <= MAX_SET).then(|| u.into())
                    }
                    _ => None,
                })
                .collect(),
        ),
    }
}

/// Whether a condition of value `v` may be `b` (a value that is not a
/// Boolean set decides nothing).
fn allows(v: &Option<VSet>, b: bool) -> bool {
    match v.as_deref().and_then(bools) {
        Some(bs) if !bs.is_empty() => bs.contains(&b),
        _ => true,
    }
}

/// Whether an `if … then` branch always leaves the routine (its last
/// statement, through blocks, is an `exit` or an `Error`).
pub(super) fn always_leaves(children: Option<&Vec<PCFNNode>>) -> bool {
    match children.and_then(|c| c.last()) {
        Some(n) if n.kind == "exit" || n.kind == "error" => true,
        Some(n) if n.kind == "block" => always_leaves(n.children.as_ref()),
        _ => false,
    }
}

struct Exec<'p> {
    p: &'p FlowProgram,
    resolve: &'p dyn Fn(&ValueLabel) -> Option<Val>,
    keep: &'p dyn Fn(u32) -> bool,
    seen: HashSet<u32>,
    live: HashSet<u32>,
    args: HashMap<u32, Vec<(u32, VSet)>>,
    keeps: Vec<u32>,
    recv: HashMap<u32, Option<VSet>>,
    /// Per enclosing loop: the states at its `break`s.
    loops: Vec<Vec<St>>,
    conds: Option<&'p mut HashMap<usize, Option<VSet>>>,
}

impl Exec<'_> {
    fn dom_of(&self, e: &FlowExpr) -> Option<&FDom> {
        match e {
            FlowExpr::Var(n) => self.p.by_name.get(n).map(|&s| &self.p.vars[s as usize].dom),
            _ => None,
        }
    }

    /// `e`'s possible values (raw: a label in its own domain); `None` unknown.
    fn eval(&self, e: &FlowExpr, s: &Slots) -> Option<VSet> {
        match e {
            FlowExpr::Bool(b) => Some(one(Val::Bool(*b))),
            FlowExpr::Label(l) => (self.resolve)(l).map(one),
            FlowExpr::Var(n) => self.p.by_name.get(n).and_then(|&i| s[i as usize].clone()),
            FlowExpr::Not(x) => {
                let b = bools(&self.eval(x, s)?)?;
                Some(of_bools(b.into_iter().map(|v| !v).collect()))
            }
            FlowExpr::And(a, b) | FlowExpr::Or(a, b) => {
                let is_and = matches!(e, FlowExpr::And(..));
                let x = self.eval(a, s).and_then(|v| bools(&v));
                let y = self.eval(b, s).and_then(|v| bools(&v));
                // `false and ?` is false, `true or ?` is true.
                let dominant = !is_and;
                if [&x, &y]
                    .iter()
                    .any(|v| v.as_deref() == Some(&[dominant][..]))
                {
                    return Some(one(Val::Bool(dominant)));
                }
                let (x, y) = (x?, y?);
                let mut out = Vec::new();
                for &p in &x {
                    for &q in &y {
                        out.push(if is_and { p && q } else { p || q });
                    }
                }
                Some(of_bools(out))
            }
            FlowExpr::Cmp { equal, lhs, rhs } => {
                let d = self.dom_of(lhs).or_else(|| self.dom_of(rhs)).cloned();
                let (mut a, mut b) = (self.eval(lhs, s)?, self.eval(rhs, s)?);
                if let Some(d) = &d {
                    a = conv_set(&a, d)?;
                    b = conv_set(&b, d)?;
                }
                let mut out = Vec::new();
                for x in a.iter() {
                    for y in b.iter() {
                        if std::mem::discriminant(x) != std::mem::discriminant(y) {
                            return None;
                        }
                        out.push((x == y) == *equal);
                    }
                }
                Some(of_bools(out))
            }
            FlowExpr::In(x, items) => {
                let d = self.dom_of(x).cloned();
                let xs = self.eval(x, s)?;
                let xs = match &d {
                    Some(d) => conv_set(&xs, d)?,
                    None => xs,
                };
                let mut known: Vec<Val> = Vec::new();
                let mut all_known = true;
                for i in items {
                    match self
                        .eval(i, s)
                        .and_then(|v| d.as_ref().map_or(Some(v.clone()), |d| conv_set(&v, d)))
                    {
                        Some(v) => known.extend(v.iter().cloned()),
                        None => all_known = false,
                    }
                }
                let mut out = Vec::new();
                for v in xs.iter() {
                    if known.contains(v) {
                        out.push(true);
                    } else if all_known {
                        out.push(false);
                    } else {
                        return None;
                    }
                }
                Some(of_bools(out))
            }
            FlowExpr::TypeTest { var, method } => {
                let &slot = self.p.by_name.get(var)?;
                if self.p.vars[slot as usize].dom != FDom::Variant {
                    return None;
                }
                let held = s[slot as usize].as_ref()?;
                let mut out = Vec::new();
                for v in held.iter() {
                    let Val::Type(t) = v else {
                        return None;
                    };
                    out.push(values::variant_test(method, t)?);
                }
                Some(of_bools(out))
            }
            FlowExpr::Unknown => None,
        }
    }

    /// Round-4 literal routes: record what an interface call site's receiver
    /// holds in state `s` (joined over every visit).
    fn record_recv(&mut self, n: &PCFNNode, s: &Slots) {
        let Some(cs) = &n.callsite_id else {
            return;
        };
        let (Some(&slot), Some(&i)) = (self.p.recv.get(cs), self.p.sites.get(cs)) else {
            return;
        };
        let v = s[slot as usize].clone();
        let joined = match self.recv.remove(&i) {
            None => v,
            Some(was) => join(Some(vec![was]), Some(vec![v])).and_then(|mut j| j.pop().flatten()),
        };
        self.recv.insert(i, joined);
    }

    fn mark(&mut self, n: &PCFNNode, st: &St) {
        for id in [&n.operation_id, &n.callsite_id].into_iter().flatten() {
            if let Some(&i) = self.p.sites.get(id) {
                self.seen.insert(i);
                if st.is_some() {
                    self.live.insert(i);
                }
            }
        }
    }

    /// Every tracked `var` formal becomes unknown (the alias rule).
    fn havoc_var_formals(&self, st: &mut St, except: Option<u16>) {
        if let Some(s) = st {
            for (i, v) in self.p.vars.iter().enumerate() {
                if v.kind == Kind::VarParam && except != Some(i as u16) {
                    s[i] = None;
                }
            }
        }
    }

    fn havoc(&self, st: &mut St, w: &[bool]) {
        if let Some(s) = st {
            for (i, written) in w.iter().enumerate() {
                if *written {
                    s[i] = None;
                }
            }
        }
    }

    /// The tracked variables `n`'s subtree may write.
    fn written(&self, n: &PCFNNode, w: &mut [bool]) {
        let formals = |w: &mut [bool]| {
            for (i, v) in self.p.vars.iter().enumerate() {
                if v.kind == Kind::VarParam {
                    w[i] = true;
                }
            }
        };
        match n.flow.as_deref() {
            Some(FlowFact::Assign { target, .. }) => {
                match target.as_ref().and_then(|t| self.p.by_name.get(t)) {
                    Some(&s) => {
                        w[s as usize] = true;
                        if self.p.vars[s as usize].kind == Kind::VarParam {
                            formals(w);
                        }
                    }
                    None => formals(w),
                }
            }
            Some(FlowFact::Opaque | FlowFact::With) => w.iter_mut().for_each(|x| *x = true),
            _ => {}
        }
        if n.kind == "try" {
            w.iter_mut().for_each(|x| *x = true);
        }
        if n.callsite_id.is_some() || n.operation_id.is_some() {
            formals(w);
        }
        if let Some(ws) = n.callsite_id.as_ref().and_then(|cs| self.p.writes.get(cs)) {
            for &(s, a) in ws {
                match a {
                    ArgWrite::Keep(k) if (self.keep)(k) => {}
                    _ => w[s as usize] = true,
                }
            }
        }
        if let Some(ValueFact::LoopVar(v)) = &n.value_fact
            && let Some(&s) = self.p.by_name.get(v)
        {
            w[s as usize] = true;
        }
        for c in n
            .condition_leaves
            .iter()
            .chain(&n.children)
            .chain(&n.else_children)
            .flatten()
        {
            self.written(c, w);
        }
    }

    fn written_of(&self, n: &PCFNNode) -> Vec<bool> {
        let mut w = vec![false; self.p.vars.len()];
        self.written(n, &mut w);
        w
    }

    fn exec_leaves(&mut self, n: &PCFNNode, st: &mut St) {
        for l in n.condition_leaves.iter().flatten() {
            if l.path_hint == Some(PathHint::MayRun) {
                let mut alt = st.clone();
                self.exec(l, &mut alt);
                *st = join(st.take(), alt);
            } else {
                self.exec(l, st);
            }
        }
    }

    fn exec_list(&mut self, nodes: &[PCFNNode], st: &mut St) {
        for n in nodes {
            self.exec(n, st);
        }
    }

    fn record_cond(&mut self, n: &PCFNNode, v: &Option<VSet>) {
        if let Some(c) = self.conds.as_deref_mut() {
            c.insert(n as *const PCFNNode as usize, v.clone());
        }
    }

    fn cond_value(&self, n: &PCFNNode, st: &St) -> Option<VSet> {
        match (n.flow.as_deref(), st) {
            (Some(FlowFact::Cond { expr, .. }), Some(s)) => self.eval(expr, s),
            _ => None,
        }
    }

    fn exec(&mut self, n: &PCFNNode, st: &mut St) {
        match n.kind.as_str() {
            "block" => {
                self.exec_leaves(n, st);
                self.exec_list(n.children.as_deref().unwrap_or_default(), st);
            }
            "call" | "op" | "error" => {
                self.exec_leaves(n, st);
                self.mark(n, st);
                if let Some(s) = st.as_ref() {
                    self.record_recv(n, s);
                }
                if st.is_some() {
                    if let (Some(cs), Some(FlowFact::Args(a)), Some(s)) =
                        (&n.callsite_id, n.flow.as_deref(), st.as_ref())
                    {
                        let known: Vec<(u32, VSet)> = a
                            .iter()
                            .enumerate()
                            .filter_map(|(j, e)| self.eval(e, s).map(|v| (j as u32, v)))
                            .collect();
                        if !known.is_empty()
                            && let Some(&i) = self.p.sites.get(cs)
                        {
                            self.args.insert(i, known);
                        }
                    }
                    self.havoc_var_formals(st, None);
                    if let Some(ws) = n.callsite_id.as_ref().and_then(|cs| self.p.writes.get(cs)) {
                        for &(slot, w) in ws {
                            match w {
                                ArgWrite::Keep(k) if (self.keep)(k) => self.keeps.push(k),
                                _ => {
                                    if let Some(s) = st.as_mut() {
                                        s[slot as usize] = None;
                                    }
                                }
                            }
                        }
                    }
                }
                if n.kind == "error" {
                    *st = None;
                }
            }
            "if" => {
                self.exec_leaves(n, st);
                let v = self.cond_value(n, st);
                self.record_cond(n, &v);
                let mut then = if allows(&v, true) { st.clone() } else { None };
                let mut otherwise = if allows(&v, false) { st.clone() } else { None };
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut then);
                self.exec_list(
                    n.else_children.as_deref().unwrap_or_default(),
                    &mut otherwise,
                );
                *st = join(then, otherwise);
            }
            "case" => self.exec_case(n, st),
            "while" => {
                let w = self.written_of(n);
                self.havoc(st, &w);
                self.exec_leaves(n, st);
                let v = self.cond_value(n, st);
                self.record_cond(n, &v);
                let mut body = if allows(&v, true) { st.clone() } else { None };
                self.loops.push(Vec::new());
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut body);
                let breaks = self.loops.pop().unwrap_or_default();
                let mut after = if allows(&v, false) { st.clone() } else { None };
                for b in breaks {
                    after = join(after, b);
                }
                *st = after;
            }
            "repeat" => {
                let w = self.written_of(n);
                self.havoc(st, &w);
                self.loops.push(Vec::new());
                let mut body = st.clone();
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut body);
                // A `continue` goes to the `until` with the head's state
                // (every value the loop writes is unknown there already).
                let mut end = join(body, st.clone());
                self.exec_leaves(n, &mut end);
                let breaks = self.loops.pop().unwrap_or_default();
                for b in breaks {
                    end = join(end, b);
                }
                *st = end;
            }
            "for" | "foreach" => {
                self.exec_leaves(n, st);
                let w = self.written_of(n);
                self.havoc(st, &w);
                self.loops.push(Vec::new());
                let mut body = st.clone();
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut body);
                let breaks = self.loops.pop().unwrap_or_default();
                for b in breaks {
                    *st = join(st.take(), b);
                }
            }
            "exit" => {
                self.exec_leaves(n, st);
                *st = None;
            }
            "break" => {
                if let Some(l) = self.loops.last_mut() {
                    l.push(st.take());
                }
                *st = None;
            }
            "continue" => {
                // Back to the loop head, whose state already holds every
                // value the loop may write.
                *st = None;
            }
            "try" => {
                if let Some(s) = st {
                    s.iter_mut().for_each(|v| *v = None);
                }
            }
            "other" => self.exec_other(n, st),
            _ => {
                // A kind this pass does not know: run what it holds, then
                // forget every value.
                self.exec_leaves(n, st);
                let mut inner = st.clone();
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut inner);
                let mut alt = st.clone();
                self.exec_list(n.else_children.as_deref().unwrap_or_default(), &mut alt);
                *st = join(inner, alt);
                if let Some(s) = st {
                    s.iter_mut().for_each(|v| *v = None);
                }
            }
        }
    }

    fn exec_other(&mut self, n: &PCFNNode, st: &mut St) {
        self.exec_leaves(n, st);
        match n.flow.as_deref() {
            Some(FlowFact::Assign {
                target,
                value,
                compound,
            }) => {
                let Some(s) = st.as_mut() else {
                    return;
                };
                match target.as_ref().and_then(|t| self.p.by_name.get(t)) {
                    Some(&i) => {
                        let d = &self.p.vars[i as usize].dom;
                        let v = if *compound {
                            None
                        } else {
                            self.eval(value, s).and_then(|v| conv_set(&v, d))
                        };
                        s[i as usize] = v;
                        if self.p.vars[i as usize].kind == Kind::VarParam {
                            self.havoc_var_formals(st, Some(i));
                        }
                    }
                    None => {
                        // A global (only one of a compatible type can be a
                        // `var` formal's actual), a name that may be one, a
                        // member or an element: the alias rule.
                        let g = target.as_ref().and_then(|t| self.p.globals.get(t));
                        match g {
                            Some(None) => {}
                            Some(Some(gd)) => {
                                let gd = gd.clone();
                                for (k, v) in self.p.vars.iter().enumerate() {
                                    if v.kind == Kind::VarParam && compatible(&v.dom, &gd) {
                                        s[k] = None;
                                    }
                                }
                            }
                            None => self.havoc_var_formals(st, None),
                        }
                    }
                }
            }
            Some(FlowFact::Opaque | FlowFact::With) => {
                let mut inner = st.clone();
                self.exec_list(n.children.as_deref().unwrap_or_default(), &mut inner);
                if let Some(s) = st {
                    s.iter_mut().for_each(|v| *v = None);
                }
            }
            _ => {
                // An `asserterror` block, or a statement holding a block: an
                // error inside may be caught, so what follows runs with every
                // value the block may write unknown.
                if let Some(children) = &n.children {
                    let w = self.written_of(n);
                    let mut inner = st.clone();
                    self.exec_list(children, &mut inner);
                    self.havoc(st, &w);
                    self.havoc_var_formals(st, None);
                }
            }
        }
    }

    fn exec_case(&mut self, n: &PCFNNode, st: &mut St) {
        self.exec_leaves(n, st);
        let scrutinee = match n.flow.as_deref() {
            Some(FlowFact::Cond { expr, .. }) => Some(expr),
            _ => None,
        };
        let d = scrutinee.and_then(|e| self.dom_of(e)).cloned();
        let v: Option<VSet> = match (scrutinee, st.as_ref()) {
            (Some(e), Some(s)) => self.eval(e, s).and_then(|v| match &d {
                Some(d) => conv_set(&v, d),
                None => Some(v),
            }),
            _ => None,
        };
        self.record_cond(n, &v);
        let branches: Vec<&PCFNNode> = n.children.iter().flatten().collect();
        let arm_set = |b: &PCFNNode| -> Option<Vec<Val>> {
            let Some(ValueFact::Arm(labels)) = &b.value_fact else {
                return None;
            };
            labels
                .iter()
                .map(|l| {
                    let raw = l.as_ref().and_then(|l| (self.resolve)(l))?;
                    match &d {
                        Some(d) => conv(&raw, d),
                        None => Some(raw),
                    }
                })
                .collect()
        };
        let arms: Vec<Option<Vec<Val>>> = branches
            .iter()
            .filter(|b| !b.is_case_else)
            .map(|b| arm_set(b))
            .collect();
        // Every value the scrutinee may hold matches a resolved label.
        let covered = match (&v, arms.iter().cloned().collect::<Option<Vec<_>>>()) {
            (Some(v), Some(sets)) => {
                let all: Vec<Val> = sets.into_iter().flatten().collect();
                v.iter().all(|x| all.contains(x))
            }
            _ => false,
        };
        let mut out: St = None;
        let mut arm_ix = 0;
        let mut has_else = false;
        for b in branches {
            let alive = if b.is_case_else {
                has_else = true;
                !covered
            } else {
                let set = arms.get(arm_ix).cloned().flatten();
                arm_ix += 1;
                match (&v, set) {
                    (Some(v), Some(set)) => v.iter().any(|x| set.contains(x)),
                    _ => true,
                }
            };
            let mut s = if alive { st.clone() } else { None };
            self.exec_list(b.children.as_deref().unwrap_or_default(), &mut s);
            out = join(out, s);
        }
        if !has_else && !covered {
            out = join(out, st.clone());
        }
        *st = out;
    }
}

/// One loaded subscriber's verdict for a publisher parameter: whether it may
/// change the parameter (module doc).
pub(super) fn subscriber_changes(
    s: &ModelRoutine,
    param: &str,
    passes_by_value: &dyn Fn(&str, u32) -> bool,
) -> bool {
    let Some(sp) = s.parameters.iter().find(|p| values::fold(&p.name) == param) else {
        return false;
    };
    if !sp.is_var {
        return false;
    }
    if s.var_assignments
        .iter()
        .any(|a| values::fold(&a.lhs_name) == param)
    {
        return true;
    }
    if let Some(tree) = &s.statement_tree {
        let mut looped = false;
        visit(tree, &mut |n| {
            if let Some(ValueFact::LoopVar(v)) = &n.value_fact
                && v == param
            {
                looped = true;
            }
        });
        if looped {
            return true;
        }
    }
    s.call_sites.iter().any(|cs| {
        cs.argument_bindings.iter().any(|b| {
            b.source_kind == "parameter"
                && b.source_parameter_index == Some(sp.index)
                && !passes_by_value(&cs.id, b.parameter_index)
        })
    })
}

/// What a guard on a route is (a witness's condition, module doc).
///
/// Two populations (r4 final, c05): the **enclosing conditions** of a
/// route's sites, every `if`/`case`/loop whose body holds the site, the
/// population `prove --site`'s route listing counts from the IR
/// (`crate::program::body::site_guards`, "of M enclosing conditions"); and
/// the **earlier exits** ([`GuardKind::Exit`]), which enclose nothing.
/// [`GuardKind::encloses`] tells them apart, so every reader counts the
/// enclosing ones as prove does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardKind {
    /// An `if` (or `while`) whose condition must be this value: `true` in
    /// its then-branch (a `while`'s body), `false` in its else-branch.
    Must(bool),
    /// One arm of a `case` (its position among the non-`else` arms).
    Arm(usize),
    /// The `else` of a `case`.
    CaseElse,
    /// The body of a `for`, a `foreach` or a `repeat` (its construct): the
    /// flow evaluates none of them.
    Loop(LoopKind),
    /// An earlier `if … then exit;` the site comes after: its condition
    /// must be false. Not an enclosing condition.
    Exit,
}

impl GuardKind {
    /// Whether the guard encloses the site (every kind but
    /// [`GuardKind::Exit`]): the population prove's "of M enclosing
    /// conditions" counts.
    pub(crate) fn encloses(&self) -> bool {
        !matches!(self, GuardKind::Exit)
    }
}

/// The loop a [`GuardKind::Loop`] guard is the body of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopKind {
    /// `for V := A to B do`: a loop-entry condition.
    For,
    /// `foreach V in X do`: a loop-entry condition.
    Foreach,
    /// `repeat … until C`: a continuation condition (the body runs once
    /// before `C` is tested).
    Repeat,
}

impl LoopKind {
    /// The construct's keyword.
    pub(crate) fn construct(self) -> &'static str {
        match self {
            LoopKind::For => "for",
            LoopKind::Foreach => "foreach",
            LoopKind::Repeat => "repeat",
        }
    }
}

/// One guard on a witness's route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteGuard {
    /// The routine it sits in (its name).
    pub routine: String,
    /// 1-based line of the guard statement.
    pub line: Option<u32>,
    /// The condition's (or the `case` scrutinee's, or the loop's) source
    /// text.
    pub text: String,
    pub kind: GuardKind,
    /// Whether the flow evaluated it on this route (its value is known and
    /// lets the route run).
    pub evaluated: bool,
}

/// The guards `site` sits under in `tree`, outermost first: (guard node,
/// kind). `None` when the site is not in the tree.
pub(super) fn guards_of<'t>(
    tree: &'t PCFNNode,
    site: &str,
) -> Option<Vec<(&'t PCFNNode, GuardKind)>> {
    fn has_site(n: &PCFNNode, site: &str) -> bool {
        n.operation_id.as_deref() == Some(site) || n.callsite_id.as_deref() == Some(site)
    }
    fn walk<'t>(n: &'t PCFNNode, site: &str, acc: &mut Vec<(&'t PCFNNode, GuardKind)>) -> bool {
        if has_site(n, site) {
            return true;
        }
        for l in n.condition_leaves.iter().flatten() {
            if walk(l, site, acc) {
                return true;
            }
        }
        match n.kind.as_str() {
            "if" | "while" => {
                let guarded = matches!(n.flow.as_deref(), Some(FlowFact::Cond { .. }));
                if let Some(c) = &n.children {
                    let len = acc.len();
                    if guarded {
                        acc.push((n, GuardKind::Must(true)));
                    }
                    if walk_list(c, site, acc) {
                        return true;
                    }
                    acc.truncate(len);
                }
                if let Some(c) = &n.else_children {
                    let len = acc.len();
                    if guarded {
                        acc.push((n, GuardKind::Must(false)));
                    }
                    if walk_list(c, site, acc) {
                        return true;
                    }
                    acc.truncate(len);
                }
                false
            }
            "case" => {
                let guarded = matches!(n.flow.as_deref(), Some(FlowFact::Cond { .. }));
                let mut arm = 0;
                for b in n.children.iter().flatten() {
                    let len = acc.len();
                    let kind = if b.is_case_else {
                        GuardKind::CaseElse
                    } else {
                        arm += 1;
                        GuardKind::Arm(arm - 1)
                    };
                    if guarded {
                        acc.push((n, kind));
                    }
                    if walk_list(b.children.as_deref().unwrap_or_default(), site, acc) {
                        return true;
                    }
                    acc.truncate(len);
                }
                false
            }
            // A loop's body: an enclosing condition of every site in it, as
            // `site_guards` counts it (r4 final, c05).
            "for" | "foreach" | "repeat" => {
                let kind = match n.kind.as_str() {
                    "for" => LoopKind::For,
                    "foreach" => LoopKind::Foreach,
                    _ => LoopKind::Repeat,
                };
                let len = acc.len();
                acc.push((n, GuardKind::Loop(kind)));
                for c in n.children.iter().chain(&n.else_children) {
                    if walk_list(c, site, acc) {
                        return true;
                    }
                }
                acc.truncate(len);
                false
            }
            _ => {
                for c in n.children.iter().chain(&n.else_children) {
                    if walk_list(c, site, acc) {
                        return true;
                    }
                }
                false
            }
        }
    }
    fn walk_list<'t>(
        nodes: &'t [PCFNNode],
        site: &str,
        acc: &mut Vec<(&'t PCFNNode, GuardKind)>,
    ) -> bool {
        let len = acc.len();
        for n in nodes {
            if walk(n, site, acc) {
                return true;
            }
            // What follows `if C then exit;` runs only when C is false.
            if n.kind == "if"
                && n.else_children.is_none()
                && matches!(n.flow.as_deref(), Some(FlowFact::Cond { .. }))
                && always_leaves(n.children.as_ref())
            {
                acc.push((n, GuardKind::Exit));
            }
        }
        acc.truncate(len);
        false
    }
    let mut acc = Vec::new();
    walk(tree, site, &mut acc).then_some(acc)
}

impl FlowProgram {
    /// The tree the program evaluates.
    pub(super) fn tree(&self) -> &PCFNNode {
        &self.tree
    }

    /// Whether the guard `node` of kind `kind` is decided in this route's
    /// favour by the values `conds` recorded (and `resolve` reads labels).
    pub(super) fn guard_evaluated(
        &self,
        node: &PCFNNode,
        kind: &GuardKind,
        conds: &HashMap<usize, Option<VSet>>,
        resolve: &dyn Fn(&ValueLabel) -> Option<Val>,
    ) -> bool {
        let Some(Some(v)) = conds.get(&(node as *const PCFNNode as usize)) else {
            return false;
        };
        match kind {
            GuardKind::Must(b) => bools(v).is_some_and(|bs| bs == [*b]),
            GuardKind::Exit => bools(v).is_some_and(|bs| bs == [false]),
            // The flow records no value for a loop.
            GuardKind::Loop(_) => false,
            GuardKind::Arm(_) | GuardKind::CaseElse => {
                let d = match node.flow.as_deref() {
                    Some(FlowFact::Cond {
                        expr: FlowExpr::Var(n),
                        ..
                    }) => self
                        .by_name
                        .get(n)
                        .map(|&s| self.vars[s as usize].dom.clone()),
                    _ => None,
                };
                let sets: Option<Vec<Vec<Val>>> = node
                    .children
                    .iter()
                    .flatten()
                    .filter(|b| !b.is_case_else)
                    .map(|b| match &b.value_fact {
                        Some(ValueFact::Arm(ls)) => ls
                            .iter()
                            .map(|l| {
                                let raw = l.as_ref().and_then(resolve)?;
                                match &d {
                                    Some(d) => conv(&raw, d),
                                    None => Some(raw),
                                }
                            })
                            .collect(),
                        _ => None,
                    })
                    .collect();
                let Some(sets) = sets else {
                    return false;
                };
                let all: Vec<&Val> = sets.iter().flatten().collect();
                match kind {
                    GuardKind::CaseElse => v.iter().all(|x| !all.contains(&x)),
                    // An arm is evaluated when every value the scrutinee may
                    // hold is one of its labels.
                    GuardKind::Arm(i) => sets
                        .get(*i)
                        .is_some_and(|s| v.iter().all(|x| s.contains(x))),
                    GuardKind::Must(_) | GuardKind::Exit | GuardKind::Loop(_) => false,
                }
            }
        }
    }
}
