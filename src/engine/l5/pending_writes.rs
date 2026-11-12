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
//! reaches is that span member's to count. A checked `Codeunit.Run` in the
//! callee's cone leaves nothing pending (it commits its own writes or rolls
//! them back, ENG-22): a workspace callee that reaches one is walked
//! ([`RunCleanCones`]), and only the writes outside those runs count.
//!
//! A `Commit()` statement on the way to the target (an earlier statement of a
//! block that holds it) ends the earlier transaction: what ran before it is
//! not pending, and the routine's callers are not in the span either
//! ([`Before::committed`], which `transaction_spans` reads as the span's start).
//!
//! A target inside a loop counts the whole body (an earlier iteration), but a
//! `Commit()` that is an earlier statement of a block holding the target in
//! the same iteration ends that too.
//!
//! Known over-approximations (a write that cannot be pending is counted): an
//! earlier statement's branch that always exits, a Commit inside a branch or a
//! loop that does not hold the target (it may not run, so it ends nothing),
//! and a callee that commits (its
//! whole cone counts, also what its own Commit already ended). Known
//! under-approximation: the other subscribers of an event a span member raises
//! (they run in an unknown order around the one that reaches the Commit) are
//! not counted.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::engine::l4::cone_derived::{ConeDerivedStore, fact_is_known_temp, write_op_bit};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::reverse_call_graph::ReverseCallGraph;
use crate::program::body::control_flow::{branch_termination, terminates};
use crate::program::body::features::PCFNNode;
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

/// The run-wide lookups [`pending_of`] reads.
#[derive(Clone, Copy)]
pub(crate) struct PendingCtx<'a> {
    pub fwd: &'a ForwardSites<'a>,
    pub routines: &'a HashMap<&'a str, &'a ModelRoutine>,
    pub summaries: &'a HashMap<String, FullRoutineSummary>,
    pub cone_derived: &'a ConeDerivedStore,
    /// The ids with two or more rows (#160 step 11b §4.6).
    pub collided: &'a HashSet<&'a str>,
    /// The callees' cones without their checked runs; `None` counts each
    /// callee's whole cone (a caller that holds one routine's sites only).
    pub clean: Option<&'a RunCleanCones<'a>>,
}

/// ENG-22: the writes and events a called routine leaves pending, with every
/// checked `Codeunit.Run` in its cone left out (BC commits the run's own writes
/// when it succeeds and rolls them back when it fails: none stay pending,
/// measured on BC 28 and confirmed on Windows, engine limits L28).
///
/// Only the routines whose forward sites are known (workspace callers: the
/// primary graph) and that reach a checked run over them are walked; every
/// other routine reached counts its whole cone, so a run inside dependency
/// code still counts its writes (an over-count, never a missed write).
///
/// Not walked either: a routine with no summary, a collided id, and a routine
/// whose cone follows an edge (a typed edge with a target) that its forward
/// sites lack. So a walked routine's cone is its own writes plus its forward
/// successors' cones, and leaving a run's target out drops only what is
/// behind the run. Besides, a walked routine adds its group's residue
/// ([`Self::residue`]): the writes its cone holds that neither its group's own
/// writes nor the cone of a routine called from outside the group (a checked
/// run's target included) explain. That catches a cone fact with no edge
/// behind it; it subtracts by table, so such a fact on a table a run also
/// writes is still dropped (no such fact is known; on CDO the residue was empty
/// for every routine, 4,694 checked).
pub(crate) struct RunCleanCones<'a> {
    /// The routines walked: they reach a checked run over `fwd`, have a
    /// summary and one row (a collided id's edges are read through one row
    /// only, so its whole cone counts).
    walk: HashSet<&'a str>,
    /// A walked routine → its group (strongly connected over `fwd`).
    group_of: HashMap<&'a str, usize>,
    groups: Vec<Vec<&'a str>>,
    residue: std::sync::Mutex<HashMap<usize, std::sync::Arc<Effects<'a>>>>,
    memo: std::sync::Mutex<HashMap<&'a str, std::sync::Arc<Pending>>>,
}

/// Physical tables written and events published, borrowed.
#[derive(Default)]
struct Effects<'a> {
    tables: HashSet<&'a str>,
    events: HashSet<&'a str>,
}

impl<'a> Effects<'a> {
    /// A routine's whole cone (read by its own id when it has no summary).
    fn add_cone(&mut self, x: &'a str, cx: &PendingCtx<'a>) {
        let id = cx.summaries.get(x).map_or(x, |s| s.routine_id.as_str());
        let cd = cx.cone_derived;
        self.tables
            .extend(cd.physical_write_ids_of(id).map(|i| cd.resolve_res(i)));
        self.events
            .extend(cd.event_ids_of(id).iter().map(|i| cd.resolve_res(*i)));
    }

    /// A routine's own physical writes and publishes.
    fn add_direct(&mut self, x: &'a str, cx: &PendingCtx<'a>) {
        let Some(s) = cx.summaries.get(x) else {
            return;
        };
        for f in &s.capability_facts_direct {
            let Some(rid) = f.resource_id.as_deref() else {
                continue;
            };
            match f.resource_kind {
                "table" if write_op_bit(f.op).is_some() && !fact_is_known_temp(f) => {
                    self.tables.insert(rid);
                }
                "event" if f.op == "publish" => {
                    self.events.insert(rid);
                }
                _ => {}
            }
        }
    }
}

/// The site `from → to` is a checked run.
fn is_run_edge(
    from: &str,
    site: Option<&str>,
    to: &str,
    routines: &HashMap<&str, &ModelRoutine>,
) -> bool {
    let Some(site) = site else { return false };
    routines
        .get(from)
        .and_then(|r| r.call_sites.iter().find(|c| c.id == site))
        .is_some_and(|cs| is_checked_run(cs, routines.get(to).copied()))
}

impl<'a> RunCleanCones<'a> {
    /// The routines to walk (those that reach a checked run over `fwd`, with a
    /// summary and one row) and their groups.
    pub(crate) fn build(
        fwd: &'a ForwardSites<'a>,
        routines: &HashMap<&'a str, &'a ModelRoutine>,
        summaries: &HashMap<String, FullRoutineSummary>,
        collided: &HashSet<&'a str>,
        typed: &[crate::engine::l4::combined_graph::TypedEdge],
    ) -> Self {
        let mut callers: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
        let mut reaches_run = HashSet::new();
        let mut queue = Vec::new();
        for (&from, outs) in fwd {
            for &(site, to) in outs {
                callers.entry(to).or_default().push(from);
                if is_run_edge(from, site, to, routines) && reaches_run.insert(from) {
                    queue.push(from);
                }
            }
        }
        while let Some(x) = queue.pop() {
            for &c in callers.get(x).map_or(&[][..], Vec::as_slice) {
                if reaches_run.insert(c) {
                    queue.push(c);
                }
            }
        }
        // Review round 4: a routine whose cone follows an edge the forward
        // sites lack is not walked. Its residue cannot tell that edge's write
        // apart from the same table behind a checked run (the residue
        // subtracts by table), so its whole cone counts.
        let mut unseen_edge: HashSet<&str> = HashSet::new();
        for e in typed {
            let Some(to) = e.to.as_deref() else {
                continue;
            };
            if reaches_run.contains(e.from.as_str())
                && !fwd
                    .get(e.from.as_str())
                    .is_some_and(|outs| outs.iter().any(|&(_, t)| t == to))
            {
                unseen_edge.insert(e.from.as_str());
            }
        }
        let walk: HashSet<&'a str> = reaches_run
            .into_iter()
            .filter(|x| {
                summaries.contains_key(*x) && !collided.contains(x) && !unseen_edge.contains(x)
            })
            .collect();
        // Groups over every forward site between walked routines (a checked
        // run's edge included: what loops back through it is one group).
        let mut nodes: Vec<String> = walk.iter().map(|x| x.to_string()).collect();
        nodes.sort();
        let edges_by_from: HashMap<String, Vec<String>> = walk
            .iter()
            .map(|&x| {
                let mut to: Vec<String> = fwd
                    .get(x)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .filter(|(_, t)| walk.contains(t))
                    .map(|(_, t)| t.to_string())
                    .collect();
                to.sort();
                to.dedup();
                (x.to_string(), to)
            })
            .collect();
        let sccs = crate::engine::l4::scc::tarjan_scc(&crate::engine::l4::scc::SccInputGraph {
            nodes: &nodes,
            edges_by_from: &edges_by_from,
        });
        let by_name: HashMap<&str, &'a str> = walk.iter().map(|&x| (x, x)).collect();
        let groups: Vec<Vec<&'a str>> = sccs
            .sccs
            .iter()
            .map(|s| s.members.iter().map(|m| by_name[m.as_str()]).collect())
            .collect();
        let group_of = groups
            .iter()
            .enumerate()
            .flat_map(|(i, g)| g.iter().map(move |&x| (x, i)))
            .collect();
        RunCleanCones {
            walk,
            group_of,
            groups,
            residue: std::sync::Mutex::new(HashMap::new()),
            memo: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Group `g`'s residue: what its members' cones hold that neither the
    /// members' own writes nor the cone of a routine they call outside the
    /// group (over any forward site, a checked run's included) explain: the
    /// cone's facts over edges the forward sites lack.
    fn residue(&self, g: usize, cx: &PendingCtx<'a>) -> std::sync::Arc<Effects<'a>> {
        let lock = || self.residue.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(r) = lock().get(&g) {
            return r.clone();
        }
        let members = &self.groups[g];
        let mut cone = Effects::default();
        let mut known = Effects::default();
        for &x in members {
            cone.add_cone(x, cx);
            known.add_direct(x, cx);
            for &(_, to) in cx.fwd.get(x).map_or(&[][..], Vec::as_slice) {
                if self.group_of.get(to) != Some(&g) {
                    known.add_cone(to, cx);
                }
            }
        }
        cone.tables.retain(|t| !known.tables.contains(t));
        cone.events.retain(|e| !known.events.contains(e));
        let r = std::sync::Arc::new(cone);
        lock().insert(g, r.clone());
        r
    }

    /// `callee`'s pending effects, or `None` when it is not walked (its whole
    /// cone counts).
    fn of(&self, callee: &'a str, cx: &PendingCtx<'a>) -> Option<std::sync::Arc<Pending>> {
        if !self.walk.contains(callee) {
            return None;
        }
        let lock = || self.memo.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = lock().get(callee) {
            return Some(p.clone());
        }
        let mut out = Effects::default();
        let mut groups_added: HashSet<usize> = HashSet::new();
        let mut seen: HashSet<&'a str> = HashSet::from([callee]);
        let mut stack = vec![callee];
        while let Some(x) = stack.pop() {
            if !self.walk.contains(x) {
                out.add_cone(x, cx);
                continue;
            }
            out.add_direct(x, cx);
            let g = self.group_of[x];
            if groups_added.insert(g) {
                let r = self.residue(g, cx);
                out.tables.extend(r.tables.iter().copied());
                out.events.extend(r.events.iter().copied());
            }
            for &(site, to) in cx.fwd.get(x).map_or(&[][..], Vec::as_slice) {
                if !is_run_edge(x, site, to, cx.routines) && seen.insert(to) {
                    stack.push(to);
                }
            }
        }
        // The walk follows forward sites the cone does not (on CDO 140
        // routines' sites reach writes their cone leaves out): keep only the
        // cone's, so leaving the runs out can remove a write, never add one.
        let mut cone = Effects::default();
        cone.add_cone(callee, cx);
        let out = std::sync::Arc::new(Pending {
            tables: out
                .tables
                .into_iter()
                .filter(|t| cone.tables.contains(t))
                .map(str::to_string)
                .collect(),
            events: out
                .events
                .into_iter()
                .filter(|e| cone.events.contains(e))
                .map(str::to_string)
                .collect(),
        });
        lock().insert(callee, out.clone());
        Some(out)
    }
}

/// What `r` has pending when control reaches any of `targets`, and how it got
/// there (agent-tasks design §6.3, the callers' context questions): the sites
/// it counted, whether it fell back to the whole body, and whether a
/// `Commit()` statement on the way cleared what came before. One reading of
/// the order with the transaction spans: [`sites_before`], then
/// [`pending_of`].
#[derive(Debug, Default, Clone)]
pub(crate) struct PendingReport {
    pub pending: Pending,
    /// The sites of `r` that may run before a target (a target itself
    /// excluded).
    pub sites: BTreeSet<String>,
    /// No tree, or a target the tree does not hold: every site counted.
    pub whole_body: bool,
    /// A `Commit()` statement runs on the way to every target
    /// ([`Before::committed`]): what ran before it is committed, so `r`'s
    /// own callers have no write pending at the target through `r`.
    pub committed: bool,
}

/// [`PendingReport`] for `r` at `targets`.
pub(crate) fn pending_before_report(
    r: &ModelRoutine,
    targets: &[&str],
    whole_body: bool,
    cx: &PendingCtx<'_>,
) -> PendingReport {
    let before = sites_before(r, targets, whole_body);
    let pending = pending_of(r, &before, targets, cx);
    PendingReport {
        pending,
        sites: before.sites,
        whole_body: before.all,
        committed: before.committed,
    }
}

/// The sites of a routine that may run, in the open transaction, before any of
/// its targets ([`sites_before`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Before {
    /// The sites, without the targets themselves.
    pub sites: BTreeSet<String>,
    /// The order is unknown (no tree, a target the tree does not hold, or
    /// `whole_body`): `sites` is every site of the routine.
    pub all: bool,
    /// The order is known and a `Commit()` statement runs on the way to EVERY
    /// target (an earlier statement of a block that holds it, not in a branch
    /// or a loop of its own): the transaction open when the routine was
    /// entered has ended before any target is reached.
    pub committed: bool,
    /// ENG-22 review: a target may have run before itself (it sits in a loop
    /// that holds it, so an earlier pass ran it). The targets are left out of
    /// `sites` all the same; a reader that asks what is pending AT a target
    /// counts the target's own leftovers from that earlier pass.
    pub repeats: bool,
}

/// The sites of `r` that may run in the open transaction before control
/// reaches any of `targets`. With no tree, a target the tree does not hold, or
/// `whole_body`, every site of `r` counts.
pub(crate) fn sites_before(r: &ModelRoutine, targets: &[&str], whole_body: bool) -> Before {
    sites_before_where(r, targets, whole_body, true)
}

/// [`sites_before`], where `commits_end` says whether `r`'s `Commit()`
/// statements end the open transaction at all. B7: a caller that knows a
/// `[CommitBehavior]` may ignore them (or turn them into errors) passes
/// `false`, and no commit then clears what ran before it
/// ([`crate::engine::l5::commit_scope::CbGraph::commit_surely_runs`]).
pub(crate) fn sites_before_where(
    r: &ModelRoutine,
    targets: &[&str],
    whole_body: bool,
    commits_end: bool,
) -> Before {
    let mut sites: BTreeSet<String> = BTreeSet::new();
    let mut all = whole_body || r.statement_tree.is_none();
    let mut committed = !targets.is_empty();
    // ENG-22: the platform's `IsInWriteTransaction()` calls join the commit
    // ids (call-site ids never collide with operation ids), so
    // `if IsInWriteTransaction() then Commit();` reads as a `Commit()`
    // ([`is_guarded_commit`]); under a `[CommitBehavior]` that stops commits
    // (`commits_end` false) it ends nothing either.
    let commits: BTreeSet<&str> = r
        .operation_sites
        .iter()
        .filter(|o| commits_end && o.kind == "commit")
        .map(|o| o.id.as_str())
        .chain(
            r.call_sites
                .iter()
                .filter(|c| commits_end && c.write_txn_query)
                .map(|c| c.id.as_str()),
        )
        .collect();
    if let Some(tree) = &r.statement_tree
        && !all
    {
        for t in targets {
            let mut before = BTreeSet::new();
            let mut cleared = false;
            if !walk_list(
                std::slice::from_ref(tree),
                t,
                &commits,
                &mut before,
                &mut cleared,
            ) {
                all = true;
                break;
            }
            committed &= cleared;
            sites.extend(before);
        }
    }
    if all {
        committed = false;
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
    let mut repeats = false;
    for t in targets {
        repeats |= sites.remove(*t);
    }
    Before {
        sites,
        all,
        committed,
        repeats,
    }
}

/// [`sites_before`] over every row of a routine id that reaches one of
/// `targets`: `(row, the targets it holds, its sites before them)`.
///
/// Engine limits B2 (per-arm ids): a routine declared in two `#if` arms is
/// two rows, and each arm holds only its own call sites, so a target is in one
/// arm's tree. Reading the id's last row alone missed arm A's targets, fell
/// back to arm B's whole body and lost arm A's writes before its call toward
/// the Commit. Each row that holds a target (or every row, when an edge has no
/// site) is read with its own targets; a row that holds none does not reach
/// the Commit through this member in its build. With one row, or when no row
/// holds a target, `rows`' last row is read as before.
pub(crate) fn rows_before<'r, 't>(
    rows: &[&'r ModelRoutine],
    targets: &[&'t str],
    unsited: bool,
) -> Vec<(&'r ModelRoutine, Vec<&'t str>, Before)> {
    let Some(&last) = rows.last() else {
        return Vec::new();
    };
    if rows.len() > 1 {
        let parts: Vec<_> = rows
            .iter()
            .filter_map(|&r| {
                let own: Vec<&'t str> = targets
                    .iter()
                    .copied()
                    .filter(|t| crate::program::model::projection::row_holds_site(r, t))
                    .collect();
                (unsited || !own.is_empty()).then(|| {
                    let before = sites_before(r, &own, unsited);
                    (r, own, before)
                })
            })
            .collect();
        if !parts.is_empty() {
            return parts;
        }
    }
    vec![(last, targets.to_vec(), sites_before(last, targets, unsited))]
}

/// What `r` has pending when control reaches any of `targets`: the effects of
/// `before`, which [`sites_before`] computed over the same `targets`.
pub(crate) fn pending_of(
    r: &ModelRoutine,
    before: &Before,
    targets: &[&str],
    cx: &PendingCtx<'_>,
) -> Pending {
    let targets: BTreeSet<&str> = targets.iter().copied().collect();
    effects(r, &before.sites, &targets, cx)
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
    cx: &PendingCtx<'_>,
) -> Pending {
    let PendingCtx {
        fwd,
        routines,
        summaries,
        cone_derived,
        collided,
        clean,
    } = *cx;
    let mut out = Pending::default();
    if let Some(s) = summaries.get(&r.id) {
        // A collided id's direct facts are the union of its rows' facts (rows
        // that share site ids, or `#if` arms each with their own, engine
        // limits B2): a sited fact is this row's only if the row itself makes
        // it (#160 step 11b §4.6, every piece of a key from one row).
        // (Only a collided id's rows compute it, sorted for the lookup.)
        let own = collided.contains(r.id.as_str()).then(|| {
            let mut own = crate::engine::l4::capability_cone::direct_facts_for_routine(r, &[]).0;
            own.sort();
            own
        });
        for f in &s.capability_facts_direct {
            let at = f
                .witness_operation_id
                .as_ref()
                .or(f.witness_callsite_id.as_ref());
            if at.is_some()
                && own
                    .as_ref()
                    .is_some_and(|own| own.binary_search(f).is_err())
            {
                continue;
            }
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
        if let Some(p) = clean.and_then(|c| c.of(to, cx)) {
            out.tables.extend(p.tables.iter().cloned());
            out.events.extend(p.events.iter().cloned());
            continue;
        }
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
/// result used), which runs the codeunit in its OWN transaction. The one
/// rule lives in [`crate::engine::l5::traps`] (B7: the forward error answers
/// read it too).
pub(crate) use crate::engine::l5::traps::is_checked_run;

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
        || is_guarded_commit(n, commits)
}

/// ENG-22: `if IsInWriteTransaction() then Commit();` ends the open write
/// transaction on both paths: when the platform method is false nothing is
/// pending, otherwise the `Commit()` ends it (measured on BC 28:
/// `IsInWriteTransaction()` is true exactly when a checked run fails). Only
/// that shape: the whole condition is the call, not negated (`path_hint`),
/// the call is the platform's (its id is in `commits`, put there by
/// [`sites_before_where`] from `PCallSite::write_txn_query`), there is no
/// `else`, and the `then` branch is one `Commit()` statement. Any other
/// branch commit (`if X then Commit()`) ends nothing, as before.
fn is_guarded_commit(n: &PCFNNode, commits: &BTreeSet<&str>) -> bool {
    use crate::program::body::features::PathHint;
    if n.kind != "if"
        || n.path_hint != Some(PathHint::CondCall { negated: false })
        || !kids(&n.else_children).is_empty()
    {
        return false;
    }
    let [leaf] = kids(&n.condition_leaves) else {
        return false;
    };
    if !leaf
        .callsite_id
        .as_deref()
        .is_some_and(|c| commits.contains(c))
    {
        return false;
    }
    let mut then = kids(&n.children);
    while let [only] = then {
        if only.kind == "block" {
            then = kids(&only.children);
        } else {
            break;
        }
    }
    matches!(then, [stmt] if stmt.operation_id.as_deref().is_some_and(|id| commits.contains(id)))
}

/// Add to `out` the sites that may run before `t` in the statement list
/// `nodes`; `false` when `t` is not in it. A `Commit()` statement on the way
/// ends the earlier transaction: everything collected so far (including what
/// the enclosing levels added) is committed, so `out` restarts after it, and
/// `cleared` is set.
fn walk_list(
    nodes: &[PCFNNode],
    t: &str,
    commits: &BTreeSet<&str>,
    out: &mut BTreeSet<String>,
    cleared: &mut bool,
) -> bool {
    let Some(k) = nodes.iter().position(|n| contains(n, t)) else {
        return false;
    };
    for n in &nodes[..k] {
        if is_commit(n, commits) {
            out.clear();
            *cleared = true;
            continue;
        }
        collect_reaching(n, out);
    }
    walk_node(&nodes[k], t, commits, out, cleared);
    true
}

/// `n` holds `t`: add what runs before `t` inside `n`.
fn walk_node(
    n: &PCFNNode,
    t: &str,
    commits: &BTreeSet<&str>,
    out: &mut BTreeSet<String>,
    cleared: &mut bool,
) {
    let leaves = kids(&n.condition_leaves);
    if walk_list(leaves, t, commits, out, cleared) {
        // `t` is in the condition (or the arguments). A `while` condition
        // runs again after each pass of the body, and a `repeat` condition
        // after the body of the same pass: the body may run before `t`
        // (ENG-22 review). A `for` range is evaluated once, before the body.
        // An earlier pass also ran the whole condition (round 2): `t` itself
        // and the operands after it (`while Worker() do;`).
        if matches!(n.kind.as_str(), "while" | "repeat") {
            for c in kids(&n.children)
                .iter()
                .chain(kids(&n.else_children))
                .chain(leaves)
            {
                collect_reaching(c, out);
            }
        }
        return;
    }
    // A condition (or a call's arguments) runs before the body (or the call).
    for l in leaves {
        collect_reaching(l, out);
    }
    if is_site(n, t) {
        return;
    }
    match n.kind.as_str() {
        // An earlier iteration ran the whole body. Then this iteration runs
        // the body's statements before the one that holds `t`: a `Commit()`
        // among them ends everything before it, earlier iterations included
        // (B7 review: CDO's `SendQueue` commits before its checked
        // `Codeunit.Run` inside a `repeat` loop).
        "for" | "foreach" | "while" | "repeat" => {
            for c in kids(&n.children).iter().chain(kids(&n.else_children)) {
                collect_reaching(c, out);
            }
            if !walk_list(kids(&n.children), t, commits, out, cleared) {
                walk_list(kids(&n.else_children), t, commits, out, cleared);
            }
        }
        // A `case` runs ONE branch: only the one that holds `t`.
        "case" => {
            if let Some(b) = kids(&n.children).iter().find(|b| contains(b, t)) {
                walk_node(b, t, commits, out, cleared);
            }
        }
        // Only the branch that holds `t` runs before it.
        _ => {
            if !walk_list(kids(&n.children), t, commits, out, cleared) {
                walk_list(kids(&n.else_children), t, commits, out, cleared);
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
            path_hint: None,
            table_case: None,
            field_exit_guard: None,
            table_exit_guard: None,
            junction_guards: None,
            value_fact: None,
            flow: None,
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
            &mut out,
            &mut false
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

    /// `(sites before T, cleared)` with `C` the routine's one Commit.
    fn before_committed(tree: &PCFNNode, t: &str) -> (Vec<String>, bool) {
        let mut out = BTreeSet::new();
        let mut cleared = false;
        let commits: BTreeSet<&str> = ["C"].into_iter().collect();
        assert!(walk_list(
            std::slice::from_ref(tree),
            t,
            &commits,
            &mut out,
            &mut cleared
        ));
        (out.into_iter().collect(), cleared)
    }

    /// A Commit statement on the way to T ends the open transaction (`cleared`)
    /// also when T is nested deeper; a Commit in a branch, in a loop, or after
    /// T does not.
    #[test]
    fn a_commit_on_the_way_clears_and_a_branch_or_loop_commit_does_not() {
        let b = |v: Vec<PCFNNode>| with("block", v, None);
        let on_the_way = b(vec![
            op("a"),
            op("C"),
            op("x"),
            with("if", vec![b(vec![op("y"), op("T")])], None),
        ]);
        assert_eq!(
            before_committed(&on_the_way, "T"),
            (vec!["x".into(), "y".into()], true)
        );
        let in_branch = b(vec![
            op("a"),
            with("if", vec![b(vec![op("C")])], None),
            op("T"),
        ]);
        assert!(!before_committed(&in_branch, "T").1);
        // A Commit in a loop on the way to T in the same iteration ends what
        // ran before, earlier iterations included (B7 review: CDO's
        // `SendQueue` commits before its checked run inside `repeat`).
        let in_loop = b(vec![
            op("a"),
            with("while", vec![op("w"), op("C"), op("T"), op("z")], None),
        ]);
        assert_eq!(before_committed(&in_loop, "T"), (vec![], true));
        // One after T in the loop body does not (the first iteration
        // reaches T before it).
        let loop_after = b(vec![
            op("a"),
            with("while", vec![op("w"), op("T"), op("C")], None),
        ]);
        let (sites, cleared) = before_committed(&loop_after, "T");
        assert!(!cleared);
        assert!(sites.contains(&"a".to_string()) && sites.contains(&"w".to_string()));
        // One in a loop that does not hold T ends nothing.
        let other_loop = b(vec![op("a"), with("while", vec![op("C")], None), op("T")]);
        assert!(!before_committed(&other_loop, "T").1);
        let after = b(vec![op("a"), op("T"), op("C")]);
        assert_eq!(before_committed(&after, "T"), (vec!["a".into()], false));
    }
}
