//! The writes PENDING at a Commit (engine-switch S8, triage C gap 3).
//!
//! A transaction span unions each member's whole forward cone, which counts
//! writes that are not in the transaction the Commit ends: the Commit routine's
//! own writes as its caller's, writes after the Commit, writes in a sibling
//! branch that never reaches it, and a checked `Codeunit.Run`'s writes, which
//! run in their own transaction. d8 called a routine a transaction "manager"
//! from that count, and every one of its cross-app findings on CDO/DO was a
//! false positive.
//!
//! Here a span member's pending writes are the effects of the sites that MAY
//! RUN BEFORE its call toward the Commit (for the Commit routine: before the
//! Commit itself), read off the routine's control-flow tree ([`walk_list`]):
//! every earlier statement in the same block, the conditions on the way down,
//! only the branch that holds the target, and the whole body of a loop around
//! it (an earlier iteration). A site's effects are its own physical table
//! writes and published events, plus, for a call, the callee's whole cone. The
//! call toward the Commit itself never counts, even inside a loop: what it
//! reaches is that span member's to count.
//!
//! Known over-approximations (a write that cannot be pending is counted): an
//! earlier statement's branch that always exits, and a span member that
//! commits earlier in its own body. Known under-approximation: the other
//! subscribers of an event a span member raises (they run in an unknown order
//! around the one that reaches the Commit) are not counted.

use std::collections::{BTreeSet, HashMap};

use crate::engine::l4::cone_derived::{ConeDerivedStore, fact_is_known_temp, write_op_bit};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::reverse_call_graph::ReverseCallGraph;
use crate::program::body::control_flow::{branch_termination, terminates};
use crate::program::body::features::{PCFNNode, PCallee};
use crate::program::model::workspace::ModelRoutine;

/// Every caller's out-edges as `(site, callee)`: the site is the edge's call site,
/// or its operation for an implicit trigger; `None` for an edge with neither (an
/// event dispatch from a publisher).
pub(crate) type ForwardSites<'a> = HashMap<&'a str, Vec<(Option<&'a str>, &'a str)>>;

pub(crate) fn forward_sites(reverse: &ReverseCallGraph) -> ForwardSites<'_> {
    let mut out: ForwardSites<'_> = HashMap::new();
    for (to, edges) in reverse {
        for e in edges {
            let site = e.callsite_id.as_deref().or(e.operation_id.as_deref());
            out.entry(e.from.as_str())
                .or_default()
                .push((site, to.as_str()));
        }
    }
    out
}

/// The physical tables and events a routine has pending.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Pending {
    pub tables: BTreeSet<String>,
    pub events: BTreeSet<String>,
}

/// The sites of `r` that lead to a span member (`in_span`), and whether one of
/// those edges has no site (then the whole body counts).
pub(crate) fn toward_sites<'a>(
    r: &str,
    in_span: impl Fn(&str) -> bool,
    fwd: &ForwardSites<'a>,
) -> (Vec<&'a str>, bool) {
    let mut sites = Vec::new();
    let mut unsited = false;
    for (site, to) in fwd.get(r).map_or(&[][..], Vec::as_slice) {
        if !in_span(to) {
            continue;
        }
        match site {
            Some(s) => sites.push(*s),
            None => unsited = true,
        }
    }
    (sites, unsited)
}

/// What `r` has pending when control reaches any of `targets`. With no tree, a
/// target the tree does not hold, or `whole_body`, every site of `r` counts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pending_before(
    r: &ModelRoutine,
    targets: &[&str],
    whole_body: bool,
    fwd: &ForwardSites<'_>,
    routines: &HashMap<&str, &ModelRoutine>,
    summaries: &HashMap<String, FullRoutineSummary>,
    cone_derived: &ConeDerivedStore,
) -> Pending {
    let mut sites: BTreeSet<String> = BTreeSet::new();
    let mut all = whole_body || r.statement_tree.is_none();
    let commits: BTreeSet<&str> = r
        .operation_sites
        .iter()
        .filter(|o| o.kind == "commit")
        .map(|o| o.id.as_str())
        .collect();
    if let Some(tree) = &r.statement_tree
        && !all
    {
        for t in targets {
            let mut before = BTreeSet::new();
            if !walk_list(std::slice::from_ref(tree), t, &commits, &mut before) {
                all = true;
                break;
            }
            sites.extend(before);
        }
    }
    if all {
        sites.clear();
        if let Some(tree) = &r.statement_tree {
            collect(tree, &mut sites);
        }
        sites.extend(r.call_sites.iter().map(|c| c.id.clone()));
        sites.extend(r.operation_sites.iter().map(|o| o.id.clone()));
    }
    // A call toward the Commit is never this routine's own pending write, even
    // when a loop runs it earlier: what it reaches is the span member's to
    // count (its writes before ITS call toward the Commit), and its whole cone
    // also holds the Commit and everything after it.
    for t in targets {
        sites.remove(*t);
    }
    let targets: BTreeSet<&str> = targets.iter().copied().collect();
    effects(r, &sites, &targets, fwd, routines, summaries, cone_derived)
}

/// The effects of `sites` in `r`: its own physical writes and published events
/// there, plus each called routine's whole cone, except behind a checked
/// `Codeunit.Run` (its own transaction). A `target` (a call toward the Commit)
/// adds only the event it raises itself: the raise happens before the
/// subscriber that commits runs, while the callee's own writes are that span
/// member's to count.
fn effects(
    r: &ModelRoutine,
    sites: &BTreeSet<String>,
    targets: &BTreeSet<&str>,
    fwd: &ForwardSites<'_>,
    routines: &HashMap<&str, &ModelRoutine>,
    summaries: &HashMap<String, FullRoutineSummary>,
    cone_derived: &ConeDerivedStore,
) -> Pending {
    let mut out = Pending::default();
    if let Some(s) = summaries.get(&r.id) {
        for f in &s.capability_facts_direct {
            let at = f
                .witness_operation_id
                .as_ref()
                .or(f.witness_callsite_id.as_ref());
            // A fact with no site is the routine's own, at its entry (a
            // publisher's `publish`): before any target.
            // A target that is not ALSO before itself (a loop) adds its event only.
            let at_target = at.is_some_and(|w| !sites.contains(w) && targets.contains(w.as_str()));
            if at.is_some_and(|w| !sites.contains(w)) && !at_target {
                continue;
            }
            let Some(rid) = f.resource_id.as_deref() else {
                continue;
            };
            match f.resource_kind {
                "table" if !at_target && write_op_bit(f.op).is_some() && !fact_is_known_temp(f) => {
                    out.tables.insert(rid.to_string());
                }
                "event" if f.op == "publish" => {
                    out.events.insert(rid.to_string());
                }
                _ => {}
            }
        }
    }
    for (site, to) in fwd.get(r.id.as_str()).map_or(&[][..], Vec::as_slice) {
        let Some(site) = site else { continue };
        if !sites.contains(*site) {
            continue;
        }
        if r.call_sites
            .iter()
            .find(|c| c.id == *site)
            .is_some_and(|cs| is_checked_run(cs, routines.get(*to).copied()))
        {
            continue;
        }
        let Some(callee) = summaries.get(*to) else {
            continue;
        };
        for id in cone_derived.physical_write_ids_of(&callee.routine_id) {
            out.tables.insert(cone_derived.resolve_res(id).to_string());
        }
        for id in cone_derived.event_ids_of(&callee.routine_id) {
            out.events.insert(cone_derived.resolve_res(*id).to_string());
        }
    }
    out
}

/// A checked run: `if Codeunit.Run(...)` or `if MyCodeunit.Run(...)` (the
/// result used), which runs the codeunit in its OWN transaction. For a
/// codeunit variable's `.Run`, the target must be the `OnRun` trigger (a user
/// procedure named `Run` is an ordinary call).
pub(crate) fn is_checked_run(
    cs: &crate::program::body::features::PCallSite,
    callee: Option<&ModelRoutine>,
) -> bool {
    match &cs.callee {
        PCallee::ObjectRun { object_kind, .. } => {
            object_kind == "Codeunit" && cs.object_run_return_used == Some(true)
        }
        PCallee::Member { method, .. } => {
            method.eq_ignore_ascii_case("run")
                && !cs.in_statement_position
                && callee
                    .is_some_and(|c| c.kind == "trigger" && c.name.eq_ignore_ascii_case("OnRun"))
        }
        _ => false,
    }
}

fn is_site(n: &PCFNNode, t: &str) -> bool {
    n.operation_id.as_deref() == Some(t) || n.callsite_id.as_deref() == Some(t)
}

fn kids(v: &Option<Vec<PCFNNode>>) -> &[PCFNNode] {
    v.as_deref().unwrap_or(&[])
}

fn contains(n: &PCFNNode, t: &str) -> bool {
    is_site(n, t)
        || kids(&n.condition_leaves).iter().any(|c| contains(c, t))
        || kids(&n.children).iter().any(|c| contains(c, t))
        || kids(&n.else_children).iter().any(|c| contains(c, t))
}

/// Every site in `n`'s subtree.
fn collect(n: &PCFNNode, out: &mut BTreeSet<String>) {
    for id in [&n.operation_id, &n.callsite_id].into_iter().flatten() {
        out.insert(id.clone());
    }
    for c in kids(&n.condition_leaves)
        .iter()
        .chain(kids(&n.children))
        .chain(kids(&n.else_children))
    {
        collect(c, out);
    }
}

/// Every site in `n`'s subtree that can run and then fall through: an `if`
/// arm (or a `case` branch) that always exits or raises never reaches what
/// follows, so its sites are not before it.
fn collect_reaching(n: &PCFNNode, out: &mut BTreeSet<String>) {
    for id in [&n.operation_id, &n.callsite_id].into_iter().flatten() {
        out.insert(id.clone());
    }
    for c in kids(&n.condition_leaves) {
        collect_reaching(c, out);
    }
    let arm_reaches = |b: &PCFNNode| !terminates(branch_termination(b));
    for c in kids(&n.children).iter().chain(kids(&n.else_children)) {
        let arm = match n.kind.as_str() {
            "if" | "case-branch" => arm_reaches(c),
            _ => true,
        };
        if arm {
            collect_reaching(c, out);
        }
    }
}

/// Whether `n` is a statement that always commits: a `Commit()` leaf.
fn is_commit(n: &PCFNNode, commits: &BTreeSet<&str>) -> bool {
    n.operation_id
        .as_deref()
        .is_some_and(|id| commits.contains(id))
}

/// Add to `out` the sites that may run before `t` in the statement list
/// `nodes`; `false` when `t` is not in it. A `Commit()` statement on the way
/// ends the earlier transaction: everything collected so far (including what
/// the enclosing levels added) is committed, so `out` restarts after it.
fn walk_list(
    nodes: &[PCFNNode],
    t: &str,
    commits: &BTreeSet<&str>,
    out: &mut BTreeSet<String>,
) -> bool {
    let Some(k) = nodes.iter().position(|n| contains(n, t)) else {
        return false;
    };
    for n in &nodes[..k] {
        if is_commit(n, commits) {
            out.clear();
            continue;
        }
        collect_reaching(n, out);
    }
    walk_node(&nodes[k], t, commits, out);
    true
}

/// `n` holds `t`: add what runs before `t` inside `n`.
fn walk_node(n: &PCFNNode, t: &str, commits: &BTreeSet<&str>, out: &mut BTreeSet<String>) {
    let leaves = kids(&n.condition_leaves);
    if walk_list(leaves, t, commits, out) {
        return; // `t` is in the condition (or the arguments)
    }
    // A condition (or a call's arguments) runs before the body (or the call).
    for l in leaves {
        collect_reaching(l, out);
    }
    if is_site(n, t) {
        return;
    }
    match n.kind.as_str() {
        // An earlier iteration ran the whole body.
        "for" | "foreach" | "while" | "repeat" => {
            for c in kids(&n.children).iter().chain(kids(&n.else_children)) {
                collect_reaching(c, out);
            }
        }
        // A `case` runs ONE branch: only the one that holds `t`.
        "case" => {
            if let Some(b) = kids(&n.children).iter().find(|b| contains(b, t)) {
                walk_node(b, t, commits, out);
            }
        }
        // Only the branch that holds `t` runs before it.
        _ => {
            if !walk_list(kids(&n.children), t, commits, out) {
                walk_list(kids(&n.else_children), t, commits, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str) -> PCFNNode {
        PCFNNode {
            kind: kind.to_string(),
            operation_id: None,
            callsite_id: None,
            condition_guard: None,
            condition_leaves: None,
            children: None,
            else_children: None,
            is_case_else: false,
            source_range: None,
        }
    }

    fn op(id: &str) -> PCFNNode {
        let mut n = node("op");
        n.operation_id = Some(id.to_string());
        n
    }

    fn with(kind: &str, children: Vec<PCFNNode>, else_children: Option<Vec<PCFNNode>>) -> PCFNNode {
        let mut n = node(kind);
        n.children = Some(children);
        n.else_children = else_children;
        n
    }

    fn before(tree: &PCFNNode, t: &str) -> Vec<String> {
        let mut out = BTreeSet::new();
        assert!(walk_list(
            std::slice::from_ref(tree),
            t,
            &BTreeSet::new(),
            &mut out
        ));
        out.into_iter().collect()
    }

    /// `a; if c then (b; T) else e; z` -> before T: a, b (not e, not z, not T).
    /// `while: (w; T)` -> before T: w and T itself (an earlier iteration).
    #[test]
    fn may_before_follows_order_branches_and_loops() {
        let tree = with(
            "block",
            vec![
                op("a"),
                with(
                    "if",
                    vec![with("block", vec![op("b"), op("T")], None)],
                    Some(vec![op("e")]),
                ),
                op("z"),
            ],
            None,
        );
        assert_eq!(before(&tree, "T"), vec!["a", "b"]);
        let looped = with(
            "block",
            vec![with("while", vec![op("w"), op("T")], None), op("z")],
            None,
        );
        assert_eq!(before(&looped, "T"), vec!["T", "w"]);
    }
}
