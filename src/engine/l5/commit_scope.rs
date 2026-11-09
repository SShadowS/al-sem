//! `[CommitBehavior]` scope (engine limits B4/L16): what an explicit
//! `Commit()` does once the routines on its route are taken into account.
//!
//! **The rule** (Microsoft Learn, "CommitBehavior attribute",
//! <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/attributes/devenv-commitbehavior-attribute>):
//! - `[CommitBehavior(CommitBehavior::Ignore)]` on a method makes every
//!   explicit commit ignored while the method runs; `Error` makes it raise an
//!   error. The behavior lasts for the method's whole scope (every method it
//!   calls, the subscribers of an event it publishes; object runs below) and
//!   returns to the previous one when the method ends.
//! - Only a MORE restrictive behavior can be set: under `Error`, an inner
//!   `Ignore` keeps `Error`. So the behavior a commit runs under is the
//!   strictest one declared on its route (`Normal < Ignore < Error`).
//! - It applies to explicit commits only, not to the implicit commit of
//!   `Codeunit.Run`. A checked run (`if Codeunit.Run(…) then`, the result
//!   used) commits its changes at its end unless an error occurs, and "the
//!   implicit commit by `Codeunit.Run` isn't affected by `CommitBehavior`"
//!   (Learn, "Codeunit.Run(Integer [, var Record]) Method",
//!   <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/codeunit/codeunit-run-method>).
//!   The cone has no fact for that implicit commit (an older gap, engine
//!   limits L16), so a commit inside a checked run stands in for it: once a
//!   route crosses a checked run ([`Step::CheckedRun`]) it runs normally
//!   ([`P`], pinned), whatever is declared before or after the crossing. An
//!   error the run's body raises is trapped by the run, so the pinned route
//!   adds no error either.
//! - Learn does not say whether the scope follows into an object started
//!   with `Codeunit.Run` (or `Report.Run`, `Page.Run`) whose result is not
//!   used. The engine reads it as MAY apply ([`Step::Run`]): the run's body
//!   runs under the caller's behavior or under its own declarations alone,
//!   so a commit in it stays possible and no definite "no" rests on the
//!   unverified reading.
//!
//! An object's `InherentCommitBehavior` property (parsed for source and
//! symbol objects) is not on Microsoft Learn. Its reading is the one that
//! never turns a possible effect into a definite "no": a routine without its
//! own attribute on such an object MAY run under the property, so it adds the
//! property's behavior as a possibility and keeps `Normal` too.
//!
//! **One rule, every reader.** [`declared`] is the routine's own declaration,
//! read by the callers' question (`callers_context`, which lists the
//! declarations above an added commit) and by the forward answers through
//! [`CommitScopes`] and [`CbGraph`], which the digest applies to each COMMIT
//! site it reports (`digest::digest_one_root`): a commit no route runs
//! normally is no commit; a commit some route runs under `Error` is an error
//! site (`ERROR_IMPLICIT` with detail `commitBehavior`).
//!
//! **Sets, not one value.** A route's behavior is a set of possible values
//! (bits [`N`], [`I`], [`E`], and [`P`] for a route pinned by a checked
//! run): a declaration is one value, or two for the object property.
//! [`lift`] composes two sets by the stricter-of rule; [`norm`] reads a
//! pinned route as a normal one.
//!
//! **A known over-approximation.** [`CbGraph`] walks every typed edge, a
//! superset of the edges the cone's witness search keeps. An extra edge can
//! add a route the cone cannot take: it may keep a commit as a commit or
//! add a possible error (a spurious `throws-error yes`), never drop one, so
//! no definite "no" comes from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::model::workspace::{Model, ModelObject, ModelRoutine};

/// The commit runs normally.
pub const N: u8 = 1;
/// The commit is ignored.
pub const I: u8 = 2;
/// The commit raises an error.
pub const E: u8 = 4;
/// The route crossed a checked `Codeunit.Run`: the commit stands for the
/// run's implicit commit, which no `[CommitBehavior]` changes. Read as
/// [`N`] by every answer ([`norm`]); kept apart so that no declaration
/// before or after the crossing can change it.
pub const P: u8 = 8;

/// The stricter-of composition of two behavior sets: every pair's stricter
/// member. The bits are ordered `N < I < E < P`, so the stricter is the
/// larger, and a pinned side stays pinned.
#[must_use]
pub fn lift(a: u8, b: u8) -> u8 {
    let mut out = 0;
    for x in [N, I, E, P] {
        if a & x == 0 {
            continue;
        }
        for y in [N, I, E, P] {
            if b & y != 0 {
                out |= x.max(y);
            }
        }
    }
    out
}

/// A behavior set as the answers read it: pinned ([`P`]) is normal.
#[must_use]
pub fn norm(s: u8) -> u8 {
    if s & P != 0 { (s & !P) | N } else { s }
}

/// How an edge passes the caller's behavior on to its callee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// A call, an event dispatch, a trigger: the callee runs under the
    /// caller's behavior (and its own declaration, by the stricter-of rule).
    Plain,
    /// An object run whose result is not used: the scope MAY follow into it
    /// (Learn is silent), so the callee runs under the caller's behavior or
    /// under its own declaration alone.
    Run,
    /// A checked `Codeunit.Run`: whatever commits inside is the run's
    /// implicit commit, which `[CommitBehavior]` does not change ([`P`]).
    CheckedRun,
}

impl Step {
    /// The behaviors the callee runs under, entered from a caller in `cur`
    /// with its own declaration `decl`.
    #[must_use]
    pub fn enter(self, cur: u8, decl: u8) -> u8 {
        match self {
            Step::Plain => lift(cur, decl),
            Step::Run => lift(cur, decl) | decl,
            Step::CheckedRun => {
                if cur == 0 {
                    0
                } else {
                    P
                }
            }
        }
    }

    /// The behaviors a commit in the callee can run under seen from the
    /// caller (declaration `decl`), when the callee entered normally gives
    /// `t` (0: it reaches none).
    #[must_use]
    fn back(self, decl: u8, t: u8) -> u8 {
        if t == 0 {
            return 0;
        }
        match self {
            Step::Plain => lift(decl, t),
            Step::Run => lift(decl, t) | t,
            Step::CheckedRun => P,
        }
    }
}

/// Where a routine's declaration comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclSource {
    /// The routine's own `[CommitBehavior(…)]`.
    Attribute,
    /// Its object's `InherentCommitBehavior` (the routine has no attribute).
    ObjectProperty,
}

/// A routine's commit-behavior declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    /// The behaviors the routine's scope may set ([`N`]/[`I`]/[`E`] bits).
    pub states: u8,
    pub source: DeclSource,
}

impl Declared {
    /// The declared value's word: `Ignore` or `Error`.
    #[must_use]
    pub fn word(self) -> &'static str {
        if self.states & E != 0 {
            "Error"
        } else {
            "Ignore"
        }
    }

    /// `CommitBehavior Error on <routine>` (or the object property's form).
    #[must_use]
    pub fn reason(self, routine_display: &str) -> String {
        match self.source {
            DeclSource::Attribute => format!("CommitBehavior {} on {routine_display}", self.word()),
            DeclSource::ObjectProperty => format!(
                "InherentCommitBehavior {} on the object of {routine_display} (may apply)",
                self.word()
            ),
        }
    }
}

/// THE rule for one routine: its own `[CommitBehavior(Ignore|Error)]`, else
/// its object's `InherentCommitBehavior` as a possibility; `None` when it
/// declares nothing (it runs its callees under its caller's behavior).
#[must_use]
pub fn declared(r: &ModelRoutine, object: Option<&ModelObject>) -> Option<Declared> {
    match crate::engine::return_summary::parse_commit_behavior(&r.attributes_parsed) {
        "ignore" => Some(Declared {
            states: I,
            source: DeclSource::Attribute,
        }),
        "error" => Some(Declared {
            states: E,
            source: DeclSource::Attribute,
        }),
        _ => match object.and_then(|o| o.inherent_commit_behavior.as_deref()) {
            Some("ignore") => Some(Declared {
                states: N | I,
                source: DeclSource::ObjectProperty,
            }),
            Some("error") => Some(Declared {
                states: N | E,
                source: DeclSource::ObjectProperty,
            }),
            _ => None,
        },
    }
}

/// Every declaring routine of a model, by stable routine id (the snapshot's
/// routine identity). Empty for nearly every workspace: then nothing below
/// runs.
#[derive(Debug, Clone, Default)]
pub struct CommitScopes {
    by_routine: HashMap<String, Declared>,
    /// The object-run call sites (by call-site id): [`Step::CheckedRun`]
    /// for a checked `Codeunit.Run`, else [`Step::Run`]. Filled only when
    /// some routine declares a behavior.
    runs: HashMap<String, Step>,
}

impl CommitScopes {
    /// The declarations of `model`'s routines, and (when there is one) its
    /// object-run call sites.
    #[must_use]
    pub fn of_model(model: &Model) -> CommitScopes {
        let ws = &model.workspace;
        let objects: HashMap<&str, &ModelObject> = ws
            .objects
            .iter()
            .filter(|o| o.inherent_commit_behavior.is_some())
            .map(|o| (o.id.as_str(), o))
            .collect();
        let by_routine: HashMap<String, Declared> = ws
            .routines
            .iter()
            .filter_map(|r| {
                declared(r, objects.get(r.object_id.as_str()).copied())
                    .map(|d| (r.stable_routine_id.clone(), d))
            })
            .collect();
        if by_routine.is_empty() {
            return CommitScopes::default();
        }
        CommitScopes {
            by_routine,
            runs: object_runs(model),
        }
    }

    /// How the edge at call site `callsite` passes the behavior on.
    #[must_use]
    pub fn step(&self, callsite: Option<&str>) -> Step {
        callsite
            .and_then(|c| self.runs.get(c).copied())
            .unwrap_or(Step::Plain)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_routine.is_empty()
    }

    #[must_use]
    pub fn get(&self, routine: &str) -> Option<Declared> {
        self.by_routine.get(routine).copied()
    }
}

/// Every object-run call site of `model`: `Codeunit.Run(…)`, `Report.Run`,
/// … ([`PCallee::ObjectRun`]) and a codeunit variable's `.Run` that reaches
/// an `OnRun` trigger, checked by the one rule the transaction answers use
/// ([`crate::engine::l5::pending_writes::is_checked_run`]).
fn object_runs(model: &Model) -> HashMap<String, Step> {
    use crate::engine::l5::pending_writes::is_checked_run;
    use crate::program::body::features::PCallee;
    let ws = &model.workspace;
    let mut runs: HashMap<String, Step> = HashMap::new();
    // Member `.Run` sites wait for their resolved target.
    let mut member: HashMap<&str, &crate::program::body::features::PCallSite> = HashMap::new();
    for r in &ws.routines {
        for cs in &r.call_sites {
            match &cs.callee {
                PCallee::ObjectRun { .. } => {
                    let step = if is_checked_run(cs, None) {
                        Step::CheckedRun
                    } else {
                        Step::Run
                    };
                    runs.insert(cs.id.clone(), step);
                }
                PCallee::Member { method, .. } if method.eq_ignore_ascii_case("run") => {
                    member.insert(cs.id.as_str(), cs);
                }
                _ => {}
            }
        }
    }
    if member.is_empty() {
        return runs;
    }
    let by_id: HashMap<&str, &ModelRoutine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
    for e in &model.calls.edges {
        let Some(cs) = member.get(e.callsite_id.as_str()) else {
            continue;
        };
        let Some(callee) = e.to.as_deref().and_then(|t| by_id.get(t)) else {
            continue;
        };
        if callee.kind != "trigger" || !callee.name.eq_ignore_ascii_case("OnRun") {
            continue;
        }
        let step = if is_checked_run(cs, Some(callee)) {
            Step::CheckedRun
        } else {
            Step::Run
        };
        runs.insert(e.callsite_id.clone(), step);
    }
    runs
}

/// The snapshot's call graph with the declarations on its nodes, for one
/// digest read (built once per read, shared by every root). Edge set: every
/// typed edge with a target (calls, object runs, event dispatch, triggers,
/// table events), a superset of what the cone follows: an extra route can
/// only keep a commit or add a possible error, never drop one.
pub(crate) struct CbGraph<'a> {
    ix: HashMap<&'a str, u32>,
    names: Vec<&'a str>,
    succ: Vec<Vec<(u32, &'a SnapshotGraphEdge, Step)>>,
    pred: Vec<Vec<(u32, Step)>>,
    /// Each node's declaration bits ([`N`] when it declares nothing).
    decl: Vec<u8>,
    /// Reachable from a declaring node (the node itself included): a commit
    /// in a node outside it runs normally from every root.
    zone: Vec<bool>,
    /// The declaration bits of the declaring nodes a node can reach (itself
    /// included); 0 = none.
    anc: Vec<u8>,
    /// As `anc`, for declaring nodes in primary code only.
    anc_primary: Vec<u8>,
    /// Whether a node holds a commit of its own.
    carrier: Vec<bool>,
    /// The behaviors some commit can run under when the node is entered
    /// normally (its own declaration included).
    any_commit: Vec<u8>,
    /// Per commit origin, the behaviors its commit can run under when a
    /// node is entered normally (0 = the node cannot reach it).
    toward: Mutex<HashMap<u32, Arc<Vec<u8>>>>,
    scopes: &'a CommitScopes,
}

/// A route the graph found: its edges, root side first.
pub(crate) type Route<'a> = Vec<&'a SnapshotGraphEdge>;

impl<'a> CbGraph<'a> {
    /// The graph of a tree: its typed edges, its rule-B dependency routines
    /// and the routines with a direct commit fact. `None` when no routine
    /// declares a behavior.
    pub(crate) fn of_tree(
        tree: &'a crate::engine::l5::snapshot::CapabilitySnapshot,
    ) -> Option<CbGraph<'a>> {
        let dependency = tree.boundary.as_ref().map(|b| &b.dependency_app);
        CbGraph::build(
            &tree.commit_scopes,
            tree.typed_edges.iter(),
            |r| dependency.is_some_and(|d| d.contains_key(r)),
            tree.capability_facts
                .iter()
                .filter(|f| f.op() == "commit" && f.provenance() == "direct")
                .map(|f| f.subject()),
        )
    }

    /// `None` when no routine declares a behavior (the common case: nothing
    /// to do). `edges` is the snapshot's typed edges (each routine's in
    /// source order), `is_dependency` rule B's test, `carriers` the routines
    /// with a commit of their own.
    pub(crate) fn build(
        scopes: &'a CommitScopes,
        edges: impl IntoIterator<Item = &'a SnapshotGraphEdge>,
        is_dependency: impl Fn(&str) -> bool,
        carriers: impl IntoIterator<Item = &'a str>,
    ) -> Option<CbGraph<'a>> {
        if scopes.is_empty() {
            return None;
        }
        let mut ix: HashMap<&'a str, u32> = HashMap::new();
        let mut names: Vec<&'a str> = Vec::new();
        let mut intern = |s: &'a str, ix: &mut HashMap<&'a str, u32>| -> u32 {
            *ix.entry(s).or_insert_with(|| {
                names.push(s);
                (names.len() - 1) as u32
            })
        };
        // By source routine in id order (a deterministic search order),
        // each routine's edges in their own order.
        let mut outgoing: std::collections::BTreeMap<
            &'a str,
            Vec<(&'a str, &'a SnapshotGraphEdge)>,
        > = std::collections::BTreeMap::new();
        for e in edges {
            if let Some(to) = e.edge_to() {
                outgoing.entry(e.edge_from()).or_default().push((to, e));
            }
        }
        let mut raw: Vec<(u32, u32, &'a SnapshotGraphEdge)> = Vec::new();
        for (from, out) in outgoing {
            let f = intern(from, &mut ix);
            for (to, e) in out {
                let t = intern(to, &mut ix);
                raw.push((f, t, e));
            }
        }
        let carriers: Vec<u32> = carriers.into_iter().map(|c| intern(c, &mut ix)).collect();
        let n = names.len();
        let mut succ: Vec<Vec<(u32, &'a SnapshotGraphEdge, Step)>> = vec![Vec::new(); n];
        let mut pred: Vec<Vec<(u32, Step)>> = vec![Vec::new(); n];
        for (f, t, e) in raw {
            let step = scopes.step(e.edge_callsite_id());
            succ[f as usize].push((t, e, step));
            pred[t as usize].push((f, step));
        }
        for p in &mut pred {
            p.sort_unstable();
            p.dedup();
        }
        let decl: Vec<u8> = names
            .iter()
            .map(|s| scopes.get(s).map_or(N, |d| d.states))
            .collect();
        let declaring: Vec<u32> = (0..n as u32).filter(|&i| decl[i as usize] != N).collect();
        // Forward: the zone.
        let mut zone = vec![false; n];
        let mut stack: Vec<u32> = declaring.clone();
        for &d in &declaring {
            zone[d as usize] = true;
        }
        while let Some(x) = stack.pop() {
            for &(y, _, _) in &succ[x as usize] {
                if !zone[y as usize] {
                    zone[y as usize] = true;
                    stack.push(y);
                }
            }
        }
        // Backward: which declarations each node can reach.
        let backward_bits = |seeds: &[u32]| -> Vec<u8> {
            let mut bits = vec![0u8; n];
            let mut stack: Vec<u32> = Vec::new();
            for &d in seeds {
                bits[d as usize] |= decl[d as usize] & (I | E);
                stack.push(d);
            }
            while let Some(x) = stack.pop() {
                let b = bits[x as usize];
                for &(p, _) in &pred[x as usize] {
                    if bits[p as usize] | b != bits[p as usize] {
                        bits[p as usize] |= b;
                        stack.push(p);
                    }
                }
            }
            bits
        };
        let anc = backward_bits(&declaring);
        let primary: Vec<u32> = declaring
            .iter()
            .copied()
            .filter(|&d| !is_dependency(names[d as usize]))
            .collect();
        let anc_primary = backward_bits(&primary);
        let mut carrier = vec![false; n];
        for &c in &carriers {
            carrier[c as usize] = true;
        }
        let mut g = CbGraph {
            ix,
            names,
            succ,
            pred,
            decl,
            zone,
            anc,
            anc_primary,
            carrier,
            any_commit: Vec::new(),
            toward: Mutex::new(HashMap::new()),
            scopes,
        };
        g.any_commit = g.backward_states(&carriers);
        Some(g)
    }

    /// The backward fixpoint from `origins`: per node, the behaviors a commit
    /// at one of them can run under when the node is entered normally
    /// ([`P`] kept: read it through [`norm`]).
    fn backward_states(&self, origins: &[u32]) -> Vec<u8> {
        let n = self.names.len();
        let mut t = vec![0u8; n];
        let mut stack: Vec<u32> = Vec::new();
        for &o in origins {
            t[o as usize] |= self.decl[o as usize];
            stack.push(o);
        }
        while let Some(x) = stack.pop() {
            let tx = t[x as usize];
            for &(p, step) in &self.pred[x as usize] {
                let new = t[p as usize] | step.back(self.decl[p as usize], tx);
                if new != t[p as usize] {
                    t[p as usize] = new;
                    stack.push(p);
                }
            }
        }
        t
    }

    fn node(&self, id: &str) -> Option<u32> {
        self.ix.get(id).copied()
    }

    /// A routine's declaration bits ([`N`] for an unknown one).
    pub(crate) fn decl_of(&self, id: &str) -> u8 {
        self.node(id).map_or(N, |x| self.decl[x as usize])
    }

    /// Whether nothing on any route from `root` declares a behavior: the
    /// root's commits all run normally.
    pub(crate) fn root_unaffected(&self, root: &str) -> bool {
        self.node(root).is_none_or(|r| self.anc[r as usize] == 0)
    }

    /// Whether a commit in `origin` can run under a declared behavior from
    /// any root.
    pub(crate) fn in_zone(&self, origin: &str) -> bool {
        self.node(origin).is_some_and(|o| self.zone[o as usize])
    }

    fn toward(&self, o: u32) -> Arc<Vec<u8>> {
        if let Some(t) = self.toward.lock().expect("toward cache").get(&o) {
            return Arc::clone(t);
        }
        let t = Arc::new(self.backward_states(&[o]));
        self.toward
            .lock()
            .expect("toward cache")
            .entry(o)
            .or_insert(t)
            .clone()
    }

    /// The behaviors the commit at `origin` can run under on any route from
    /// `root`; `None` when the graph has no route (a graph mismatch: the
    /// caller then keeps the commit as it is).
    pub(crate) fn states_at(&self, root: &str, origin: &str) -> Option<u8> {
        let (r, o) = (self.node(root)?, self.node(origin)?);
        let t = self.toward(o)[r as usize];
        (t != 0).then(|| norm(t))
    }

    /// The behaviors of a listed route, [`P`] kept (read it through
    /// [`norm`], or pass it to [`Self::beyond`]): the root's declaration,
    /// then each hop `(routine, call site)` entered by its edge's [`Step`].
    pub(crate) fn route_states<'s>(
        &self,
        root: &str,
        hops: impl Iterator<Item = (&'s str, Option<&'s str>)>,
    ) -> u8 {
        hops.fold(self.decl_of(root), |s, (h, cs)| {
            self.scopes.step(cs).enter(s, self.decl_of(h))
        })
    }

    /// The behaviors a commit beyond a crossing can run under: the listed
    /// prefix (`prefix` states from [`Self::route_states`], ending on the
    /// entered routine `entry`) and any commit the entered routine reaches.
    /// When the root can reach a declaring routine in primary code, a prefix
    /// that is not listed may hold one: every behavior such a routine
    /// declares is then possible too.
    pub(crate) fn beyond(&self, root: &str, prefix: u8, entry: &str) -> u8 {
        let Some(e) = self.node(entry) else {
            return norm(prefix);
        };
        let reach = self.any_commit[e as usize];
        if reach == 0 {
            return norm(prefix);
        }
        let mut pfx = prefix;
        if let Some(r) = self.node(root) {
            let extra = self.anc_primary[r as usize];
            if extra != 0 {
                pfx |= lift(self.decl[r as usize], extra | N);
            }
        }
        norm(lift(pfx, reach))
    }

    /// One route from `root` to the commit at `origin` that runs it under
    /// `target` (a single bit), shortest first; `None` when there is none.
    pub(crate) fn route_to(&self, root: &str, origin: &str, target: u8) -> Option<Route<'a>> {
        let (r, o) = (self.node(root)?, self.node(origin)?);
        let guide = self.toward(o);
        self.guided(r, &guide, target, |x| x == o)
    }

    /// One route from `entry` to some commit it reaches that runs under
    /// `target` once `entry` is entered in `start` (a single bit).
    pub(crate) fn route_beyond(&self, entry: &str, start: u8, target: u8) -> Option<Route<'a>> {
        let e = self.node(entry)?;
        let guide = &self.any_commit;
        let carrier = &self.carrier;
        self.guided_from(e, start, guide, target, |x| carrier[x as usize])
    }

    fn guided(
        &self,
        r: u32,
        guide: &[u8],
        target: u8,
        goal: impl Fn(u32) -> bool,
    ) -> Option<Route<'a>> {
        self.guided_from(r, N, guide, target, goal)
    }

    /// Breadth-first over (node, behavior) from `start` entering `s`: a step
    /// is taken only where `target` stays possible (`guide`), and the walk
    /// ends at a goal node reached under exactly `target` (a pinned route,
    /// [`P`], counts as [`N`]).
    fn guided_from(
        &self,
        s: u32,
        start: u8,
        guide: &[u8],
        target: u8,
        goal: impl Fn(u32) -> bool,
    ) -> Option<Route<'a>> {
        let admissible = |x: u32, cur: u8| norm(lift(cur, guide[x as usize])) & target != 0;
        // A search state: a node and the behavior it runs under.
        type State = (u32, u8);
        let mut parent: HashMap<State, Option<(State, &'a SnapshotGraphEdge)>> = HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        for a in [N, I, E, P] {
            if lift(start, self.decl[s as usize]) & a != 0 && admissible(s, a) {
                parent.insert((s, a), None);
                queue.push_back((s, a));
            }
        }
        while let Some((x, cur)) = queue.pop_front() {
            if norm(cur) == target && goal(x) {
                let mut route = Vec::new();
                let mut at = (x, cur);
                while let Some(Some((prev, e))) = parent.get(&at) {
                    route.push(*e);
                    at = *prev;
                }
                route.reverse();
                return Some(route);
            }
            for &(y, e, step) in &self.succ[x as usize] {
                let entered = step.enter(cur, self.decl[y as usize]);
                for next in [N, I, E, P] {
                    if entered & next == 0 {
                        continue;
                    }
                    if admissible(y, next) && !parent.contains_key(&(y, next)) {
                        parent.insert((y, next), Some(((x, cur), e)));
                        queue.push_back((y, next));
                    }
                }
            }
        }
        None
    }

    /// The routine that sets `target` on a route (`nodes`, root side
    /// first): the first whose declaration holds it, with its declaration.
    pub(crate) fn cause<'s>(
        &self,
        nodes: impl Iterator<Item = &'s str>,
        target: u8,
    ) -> Option<(&'s str, Declared)> {
        nodes
            .filter_map(|id| self.scopes.get(id).map(|d| (id, d)))
            .find(|(_, d)| d.states & target != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lift_takes_the_stricter_of_each_pair() {
        assert_eq!(lift(N, N), N);
        assert_eq!(lift(N, I), I);
        assert_eq!(lift(I, E), E);
        // Learn: under Error an inner Ignore keeps Error.
        assert_eq!(lift(E, I), E);
        assert_eq!(lift(N | I, N), N | I);
        assert_eq!(lift(N | E, I), I | E);
        // A pinned route stays pinned under any later declaration.
        assert_eq!(lift(P, E), P);
        assert_eq!(norm(P | I), N | I);
    }

    #[test]
    fn a_run_step_may_reset_and_a_checked_run_pins() {
        // An object run: the caller's Ignore may or may not follow.
        assert_eq!(Step::Run.enter(I, N), I | N);
        assert_eq!(Step::Run.back(I, N), I | N);
        // A checked run: its implicit commit ignores every declaration.
        assert_eq!(Step::CheckedRun.enter(E, I), P);
        assert_eq!(Step::CheckedRun.back(E, I), P);
        assert_eq!(Step::CheckedRun.back(E, 0), 0);
        assert_eq!(Step::Plain.enter(I, N), I);
    }
}
