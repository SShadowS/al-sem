//! Effects a root cannot reach once literal Boolean arguments are followed
//! (G23).
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
//! This is a per-root reachability over the snapshot's own edges (the graph the
//! cone was built over), whose state is a routine plus the known literal values
//! of its usable guard parameters (the cone's own [`param_guard`] frames):
//! - an edge from a site whose requirements contradict the known values is not
//!   followed;
//! - a direct or variable-typed call binds the callee's usable parameters from
//!   a `true`/`false` argument, or from the caller's own known parameter it
//!   forwards; every other edge kind (events, triggers, object runs, interface
//!   dispatch) binds nothing.
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
use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet, VecDeque};

/// The most (routine, bindings) states one root's search visits.
const STATE_CAP: usize = 400_000;

/// Known parameter values: (parameter index, value), sorted.
type Bindings = Vec<(u32, bool)>;

fn contradicts(reqs: &[Req], b: &Bindings) -> bool {
    reqs.iter()
        .any(|&(i, v)| b.iter().any(|&(j, w)| j == i && w != v))
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
        SnapshotGraphEdge::InterfaceDispatch {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, Some(callsite_id.as_str()), false),
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

/// The search context over one snapshot and model, shared by every root.
pub(crate) struct LiteralReach<'a> {
    out: HashMap<&'a str, Vec<OutEdge<'a>>>,
    by_stable: HashMap<&'a str, &'a ModelRoutine>,
    frames: HashMap<&'a str, FrameGuards>,
    /// Operation id → the stable id of the routine it sits in.
    op_owner: HashMap<&'a str, &'a str>,
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
        let mut by_stable = HashMap::new();
        let mut op_owner = HashMap::new();
        for r in routines {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(r);
            let callees_at =
                |cs: &str| -> Vec<&str> { callees.get(cs).cloned().unwrap_or_default() };
            if let Some(f) = frame_guards_over(r, callees_at, &by_id) {
                frames.insert(r.stable_routine_id.as_str(), f);
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
        let mut out: HashMap<&str, Vec<OutEdge>> = HashMap::new();
        for e in edges {
            if let Some((from, to, site, binds)) = edge_parts(e) {
                out.entry(from).or_default().push((to, site, binds));
            }
        }
        LiteralReach {
            out,
            by_stable,
            frames,
            op_owner,
        }
    }

    fn reqs_at(&self, routine: &str, site: &str) -> &[Req] {
        self.frames
            .get(routine)
            .and_then(|f| f.by_site.get(site))
            .map_or(&[], Vec::as_slice)
    }

    /// The callee's bindings across a call at `site` of `caller` with `b`.
    fn bind(&self, caller: &str, site: &str, callee: &str, b: &Bindings) -> Bindings {
        let Some(f) = self.frames.get(callee) else {
            return Vec::new();
        };
        let Some(cs) = self
            .by_stable
            .get(caller)
            .and_then(|r| r.call_sites.iter().find(|c| c.id == site))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &j in &f.usable {
            let text = cs
                .argument_texts
                .get(j as usize)
                .map(|t| t.trim().to_ascii_lowercase());
            let v = match text.as_deref() {
                Some("true") => Some(true),
                Some("false") => Some(false),
                _ => cs
                    .argument_bindings
                    .iter()
                    .find(|x| x.parameter_index == j && x.source_kind == "parameter")
                    .and_then(|x| x.source_parameter_index)
                    .and_then(|i| b.iter().find(|&&(k, _)| k == i).map(|&(_, w)| w)),
            };
            if let Some(v) = v {
                out.push((j, v));
            }
        }
        out.sort_unstable();
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
        let mut seen: HashSet<(&str, Bindings)> = HashSet::from([(root, Vec::new())]);
        let mut states: HashMap<&str, Vec<Bindings>> = HashMap::from([(root, vec![Vec::new()])]);
        let mut q = VecDeque::from([(root, Vec::new())]);
        while let Some((f, b)) = q.pop_front() {
            for &(to, site, binds) in self.out.get(f).into_iter().flatten() {
                if let Some(s) = site
                    && contradicts(self.reqs_at(f, s), &b)
                {
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind(f, s, to, &b),
                    _ => Vec::new(),
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

    /// The indices of `effects` (one root's) unreachable from `root` under
    /// literal arguments (module doc).
    pub(crate) fn unreachable_effects(
        &self,
        root: &'a str,
        effects: &[DigestEffectResult],
    ) -> HashSet<usize> {
        let mut out = HashSet::new();
        // A root with no usable guard anywhere has nothing to prune.
        if self.frames.is_empty() {
            return out;
        }
        let Some((plain, states)) = self.search(root) else {
            return out;
        };
        for (i, e) in effects.iter().enumerate() {
            if e.boundary_group || e.origin_operation_ids.is_empty() {
                continue;
            }
            let all_dead = e.origin_operation_ids.iter().all(|op| {
                let Some(&owner) = self.op_owner.get(op.as_str()) else {
                    return false;
                };
                if !plain.contains(owner) {
                    return false;
                }
                let reqs = self.reqs_at(owner, op);
                !states
                    .get(owner)
                    .is_some_and(|bs| bs.iter().any(|b| !contradicts(reqs, b)))
            });
            if all_dead {
                out.insert(i);
            }
        }
        out
    }
}
