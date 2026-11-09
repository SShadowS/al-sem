//! `[CommitBehavior]` scope (engine limits B4/L16): what an explicit
//! `Commit()` does once the routines on its route are taken into account.
//!
//! **The rule** (Microsoft Learn, "CommitBehavior attribute",
//! <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/attributes/devenv-commitbehavior-attribute>):
//! - `[CommitBehavior(CommitBehavior::Ignore)]` on a method makes every
//!   explicit commit ignored while the method runs; `Error` makes it raise an
//!   error. The behavior lasts for the method's whole scope (every method it
//!   calls, the subscribers of an event it publishes, a codeunit it runs) and
//!   returns to the previous one when the method ends.
//! - Only a MORE restrictive behavior can be set: under `Error`, an inner
//!   `Ignore` keeps `Error`. So the behavior a commit runs under is the
//!   strictest one declared on its route (`Normal < Ignore < Error`).
//! - It applies to explicit commits only, not to the implicit commit of
//!   `Codeunit.Run`. The cone's commit facts are explicit `Commit()` calls
//!   only, so every one of them is in scope.
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
//! (bits [`N`], [`I`], [`E`]): a declaration is one value, or two for the
//! object property. [`lift`] composes two sets by the stricter-of rule.

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

/// The stricter-of composition of two behavior sets: every pair's stricter
/// member. The bits are ordered `N < I < E`, so the stricter is the larger.
#[must_use]
pub fn lift(a: u8, b: u8) -> u8 {
    let mut out = 0;
    for x in [N, I, E] {
        if a & x == 0 {
            continue;
        }
        for y in [N, I, E] {
            if b & y != 0 {
                out |= x.max(y);
            }
        }
    }
    out
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
}

impl CommitScopes {
    /// The declarations of `model`'s routines.
    #[must_use]
    pub fn of_model(model: &Model) -> CommitScopes {
        let ws = &model.workspace;
        let objects: HashMap<&str, &ModelObject> = ws
            .objects
            .iter()
            .filter(|o| o.inherent_commit_behavior.is_some())
            .map(|o| (o.id.as_str(), o))
            .collect();
        let by_routine = ws
            .routines
            .iter()
            .filter_map(|r| {
                declared(r, objects.get(r.object_id.as_str()).copied())
                    .map(|d| (r.stable_routine_id.clone(), d))
            })
            .collect();
        CommitScopes { by_routine }
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

/// The snapshot's call graph with the declarations on its nodes, for one
/// digest read (built once per read, shared by every root). Edge set: every
/// typed edge with a target (calls, object runs, event dispatch, triggers,
/// table events), a superset of what the cone follows: an extra route can
/// only keep a commit or add a possible error, never drop one.
pub(crate) struct CbGraph<'a> {
    ix: HashMap<&'a str, u32>,
    names: Vec<&'a str>,
    succ: Vec<Vec<(u32, &'a SnapshotGraphEdge)>>,
    pred: Vec<Vec<u32>>,
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
        let mut succ: Vec<Vec<(u32, &'a SnapshotGraphEdge)>> = vec![Vec::new(); n];
        let mut pred: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (f, t, e) in raw {
            succ[f as usize].push((t, e));
            pred[t as usize].push(f);
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
            for &(y, _) in &succ[x as usize] {
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
                for &p in &pred[x as usize] {
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
    /// at one of them can run under when the node is entered normally.
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
            for &p in &self.pred[x as usize] {
                let new = t[p as usize] | lift(self.decl[p as usize], tx);
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
        (t != 0).then_some(t)
    }

    /// The behaviors of a listed route: the root's and each hop's
    /// declarations, composed.
    pub(crate) fn route_states<'s>(&self, root: &str, hops: impl Iterator<Item = &'s str>) -> u8 {
        hops.fold(self.decl_of(root), |s, h| lift(s, self.decl_of(h)))
    }

    /// The behaviors a commit beyond a crossing can run under: the listed
    /// prefix (`prefix` states, ending on the entered routine `entry`) and
    /// any commit the entered routine reaches. When the root can reach a
    /// declaring routine in primary code, a prefix that is not listed may
    /// hold one: every behavior such a routine declares is then possible too.
    pub(crate) fn beyond(&self, root: &str, prefix: u8, entry: &str) -> u8 {
        let Some(e) = self.node(entry) else {
            return prefix;
        };
        let reach = self.any_commit[e as usize];
        if reach == 0 {
            return prefix;
        }
        let mut pfx = prefix;
        if let Some(r) = self.node(root) {
            let extra = self.anc_primary[r as usize];
            if extra != 0 {
                pfx |= lift(self.decl[r as usize], extra | N);
            }
        }
        lift(pfx, reach)
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
    /// ends at a goal node reached under exactly `target`.
    fn guided_from(
        &self,
        s: u32,
        start: u8,
        guide: &[u8],
        target: u8,
        goal: impl Fn(u32) -> bool,
    ) -> Option<Route<'a>> {
        let admissible = |x: u32, cur: u8| lift(cur, guide[x as usize]) & target != 0;
        // A search state: a node and the behavior it runs under.
        type State = (u32, u8);
        let mut parent: HashMap<State, Option<(State, &'a SnapshotGraphEdge)>> = HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        for a in [N, I, E] {
            if lift(start, self.decl[s as usize]) & a != 0 && admissible(s, a) {
                parent.insert((s, a), None);
                queue.push_back((s, a));
            }
        }
        while let Some((x, cur)) = queue.pop_front() {
            if cur == target && goal(x) {
                let mut route = Vec::new();
                let mut at = (x, cur);
                while let Some(Some((prev, e))) = parent.get(&at) {
                    route.push(*e);
                    at = *prev;
                }
                route.reverse();
                return Some(route);
            }
            for &(y, e) in &self.succ[x as usize] {
                for a in [N, I, E] {
                    if self.decl[y as usize] & a == 0 {
                        continue;
                    }
                    let next = cur.max(a);
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
    }
}
