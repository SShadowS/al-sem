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
//! r3 E1 adds Integer, Option and Enum values ([`values`]): a usable value
//! parameter is bound from an integer literal, an option or enum label, or
//! the caller's own bound parameter, and prunes the `case` arms (and `else`)
//! and `if =` / `<>` / `in` branches it cannot take.
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
use crate::engine::l5::digest::{Crossing, CrossingSite, DigestEffectResult};
use crate::engine::l5::recref_flow::{
    Atom, Atoms, RecRefFlow, RelevantGlobals, RelevantParams, TSet, eval,
};
use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::engine::l5::terminal_facts::TerminalFacts;
use crate::program::body::features::{PCFNNode, TableCase};
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

#[path = "literal_routes.rs"]
pub mod routes;

#[path = "literal_values.rs"]
mod values;

#[path = "literal_flow.rs"]
mod lflow;

#[cfg(test)]
#[path = "literal_flow_tests.rs"]
mod lflow_tests;

pub(crate) use lflow::{GuardKind, RouteGuard};
use values::{VSet, ValueFrame, ValueReq};

/// r3 E2: the most cached flow evaluations kept at once; past it the cache
/// starts over (an evaluation depends only on its key, so a rebuilt one is
/// the same).
const FLOW_CACHE_MAX: usize = 200_000;

/// r3 E2: a publisher parameter whose loaded subscribers are proven unable to
/// change it ([`lflow`]'s keep verdict): what an answer that relies on it
/// states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeepInfo {
    /// The publisher, `Codeunit "X"::OnSomething`.
    pub publisher: String,
    /// The parameter's name as declared.
    pub parameter: String,
    /// The loaded subscribers considered (`Codeunit "Y"::Handler`), sorted;
    /// empty when none is loaded.
    pub subscribers: Vec<String>,
}

impl KeepInfo {
    /// The compact form a digest row carries: `<publisher> keeps <param>
    /// (no loaded subscriber)` or `(subscribers: …)`.
    pub(crate) fn short(&self) -> String {
        if self.subscribers.is_empty() {
            format!(
                "{} keeps {} (no loaded subscriber)",
                self.publisher, self.parameter
            )
        } else {
            format!(
                "{} keeps {} (subscribers: {})",
                self.publisher,
                self.parameter,
                self.subscribers.join(", ")
            )
        }
    }

    /// The plain-English statement of the assumption.
    pub(crate) fn statement(&self) -> String {
        if self.subscribers.is_empty() {
            format!(
                "{} has no loaded subscriber, so {} keeps its value",
                self.publisher, self.parameter
            )
        } else {
            format!(
                "the loaded subscribers of {} ({}) do not change {}",
                self.publisher,
                self.subscribers.join(", "),
                self.parameter
            )
        }
    }
}

/// r3 E2: what the literal reach adds to one effect row for a reader: the
/// loaded-subscriber assumptions its removal rests on, and its witness
/// route's guards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FlowNotes {
    /// The row is unreachable only because these publishers' loaded
    /// subscribers keep a value (empty: not by an assumption).
    pub assumed: Vec<KeepInfo>,
    /// The guards on the row's listed witness route, outermost first
    /// (`None`: no route was replayed).
    pub guards: Option<Vec<RouteGuard>>,
}

/// The most (routine, bindings) states one root's search visits.
pub(crate) const STATE_CAP: usize = 400_000;

/// A search checks its cancellation closure once per this many states.
const CANCEL_EVERY: usize = 4096;

/// W2-0b: the most states all cached suffix trees of one read hold together
/// (design §3.4.3 item 3); one tree holds at most [`STATE_CAP`].
const SUFFIX_CACHE_STATES: usize = 4 * STATE_CAP;

/// The routines a path of `steps` from `start` visits, `start` first.
fn routines_of<'a>(start: &'a str, steps: &[Step<'a>]) -> Vec<&'a str> {
    match steps.first() {
        None => vec![start],
        Some(first) => std::iter::once(first.from)
            .chain(steps.iter().map(|s| s.to))
            .collect(),
    }
}

/// One crossing's entries into dependency code from one root
/// ([`LiteralReach::crossing_entries`]): (caller state, the crossing edge,
/// the callee's bindings).
pub(crate) struct CrossingEntries<'a>(Vec<(State<'a>, Step<'a>, Bindings)>);

impl CrossingEntries<'_> {
    /// No entry (a crossing the root's search never takes). Tests only.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        CrossingEntries(Vec::new())
    }

    /// How many distinct entry states the crossing has. Tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

/// What [`LiteralReach::group_witness`] found (design §3.4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupWitness<'a> {
    /// A path that traverses the crossing: `steps[entry]` is the crossing's
    /// edge; `terminal` is the live operation the last step's routine owns.
    Through {
        steps: Vec<Step<'a>>,
        entry: usize,
        terminal: String,
    },
    /// A suffix search stopped early and the root tree's own path does not
    /// traverse the crossing: real, but never evidence of "beyond X".
    Outside {
        steps: Vec<Step<'a>>,
        terminal: String,
        reason: Stop,
    },
    /// No witness: the searches completed and found none (`None`), or one
    /// stopped early and the root tree has no path either.
    Unavailable(Option<Stop>),
}

impl<'a> GroupWitness<'a> {
    /// The routines the witness visits, root first; empty when unavailable.
    pub(crate) fn routines(&self) -> Vec<&'a str> {
        match self {
            GroupWitness::Through { steps, .. } | GroupWitness::Outside { steps, .. } => steps
                .first()
                .map_or(Vec::new(), |s| routines_of(s.from, steps)),
            GroupWitness::Unavailable(_) => Vec::new(),
        }
    }
}

/// W2-0b: the suffix trees of one read, by entry state `(target,
/// bindings)` (design §3.4.3 item 2). A suffix depends on nothing but its
/// entry state, so one tree serves every root, crossing and resource that
/// enters there. Bounded twice: [`STATE_CAP`] per tree (a larger search is
/// `Stop::Capped`, and that outcome is cached too), and an aggregate state
/// budget over all trees held, kept by evicting the OLDEST insertion first,
/// so what is evicted depends only on the order of requests, never on
/// timing. An evicted tree is rebuilt on its next request, with the same
/// result.
pub(crate) struct SuffixCache<'a> {
    trees: HashMap<State<'a>, Result<Rc<LiveTree<'a>>, Stop>>,
    order: VecDeque<State<'a>>,
    held: usize,
    per_tree: usize,
    budget: usize,
    /// W2-0c: per entry target, the routines reached with guards ignored (a
    /// set per distinct target routine, so few and small next to the trees).
    plain: HashMap<&'a str, Rc<HashSet<&'a str>>>,
    /// Trees built and evicted (the cost the commit message measures).
    pub(crate) built: usize,
    pub(crate) evicted: usize,
    pub(crate) peak_held: usize,
}

impl Default for SuffixCache<'_> {
    fn default() -> Self {
        SuffixCache::with_limits(STATE_CAP, SUFFIX_CACHE_STATES)
    }
}

impl<'a> SuffixCache<'a> {
    /// A cache whose trees hold at most `per_tree` states each and
    /// `budget` together.
    pub(crate) fn with_limits(per_tree: usize, budget: usize) -> Self {
        SuffixCache {
            trees: HashMap::new(),
            order: VecDeque::new(),
            held: 0,
            per_tree,
            budget: budget.max(per_tree),
            plain: HashMap::new(),
            built: 0,
            evicted: 0,
            peak_held: 0,
        }
    }

    /// The states the held trees hold together. Tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn held(&self) -> usize {
        self.held
    }

    /// The suffix tree from `start`, built on first use.
    fn get(
        &mut self,
        reach: &LiteralReach<'a>,
        start: State<'a>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Rc<LiveTree<'a>>, Stop> {
        if let Some(t) = self.trees.get(&start) {
            return t.clone();
        }
        let built = reach.tree_from(start.clone(), self.per_tree, cancel);
        self.built += 1;
        // A cancelled search is no property of the entry: not cached.
        if matches!(built, Err(Stop::Cancelled)) {
            return Err(Stop::Cancelled);
        }
        let n = built.as_ref().map_or(0, |t| t.len());
        while self.held + n > self.budget {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(Ok(t)) = self.trees.remove(&old) {
                self.held -= t.len();
            }
            self.evicted += 1;
        }
        let built = built.map(Rc::new);
        self.trees.insert(start.clone(), built.clone());
        self.order.push_back(start);
        self.held += n;
        self.peak_held = self.peak_held.max(self.held);
        built
    }

    /// The routines `from` reaches with guards ignored, built on first use.
    fn plain(&mut self, reach: &LiteralReach<'a>, from: &'a str) -> Rc<HashSet<&'a str>> {
        Rc::clone(
            self.plain
                .entry(from)
                .or_insert_with(|| Rc::new(reach.plain_reach(from))),
        )
    }
}

/// W2-0c (design §3.3.1): whether a crossing reaches one of its group's
/// resources, judged from the crossing's own entry states, not the root's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStatus {
    /// A live state of a candidate operation's owner is reached beyond the
    /// crossing.
    Reachable,
    /// Every suffix search completed, a candidate's owner is reached with
    /// guards ignored, and no state of it lets the operation run: the
    /// resource is not written beyond this crossing.
    Disproved,
    /// Nothing concluded, for the reason given. Never deletes the resource.
    NotEstablished(Undecided),
}

/// Why a [`ResourceStatus`] is not established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Undecided {
    /// The root's search never takes the crossing's edges.
    NoEntry,
    /// No candidate operation, or one that names none.
    NoOperation,
    /// A suffix search stopped early (cap or cancellation).
    SearchStopped,
    /// No candidate's owner is reached even with guards ignored: the cone
    /// followed a graph this search does not, so nothing is concluded.
    OwnerNotReached,
    /// Not a table write: W2-0c judges table writes only (an event's or a
    /// read's resources are never dropped).
    KindNotJudged,
}

/// A table requirement in one routine's frame: (parameter index, whether its
/// table must be IN the set (else NOT in it), the folded table names).
type TableReq = (u32, bool, Arc<[String]>);

/// G34: a requirement that what `atoms` stand for (in the routine's terms)
/// holds one of the tables.
type AtomReq = (Atoms, Arc<[String]>);

/// Known parameter values, each list sorted by parameter index: Booleans, and
/// the folded names of the tables a record argument may hold (`""`: none).
/// G34: and the tables the routine's own instance's globals may hold.
/// r3 E1: and the Integer, Option and Enum values a parameter may hold
/// ([`values`]); a parameter absent here holds an unknown value.
/// r3 E2: and the entry values of the parameters the routine's flow program
/// reads that neither list above binds ([`lflow`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
struct Bindings {
    bools: Vec<(u32, bool)>,
    tables: Vec<(u32, TSet)>,
    globals: Vec<(Arc<str>, TSet)>,
    values: Vec<(u32, VSet)>,
    flow: Vec<(u32, VSet)>,
}

impl Bindings {
    /// r3 E2: parameter `i`'s known entry value, from whichever list binds it.
    fn entry_value(&self, i: u32) -> Option<VSet> {
        if let Some((_, v)) = self.flow.iter().find(|(k, _)| *k == i) {
            return Some(Arc::clone(v));
        }
        if let Some(&(_, b)) = self.bools.iter().find(|(k, _)| *k == i) {
            return Some(Arc::from([values::Val::Bool(b)]));
        }
        self.values
            .iter()
            .find(|(k, _)| *k == i)
            .map(|(_, v)| Arc::clone(v))
    }
}

/// r3 E2: the flow evaluation cache key: (the program, by address: the
/// programs never move once built; entry values; whether publisher keeps are
/// followed). No borrowed id, so the reach stays covariant in its lifetime.
type FlowKey = (usize, Vec<(u32, VSet)>, bool);

/// [`LiteralReach::unreachable_effects_noted`]'s result: the dead effects,
/// per group its dead resources, and per effect dead only by an assumption
/// the keep verdicts it rests on.
pub(crate) type NotedUnreachable = (
    HashSet<usize>,
    HashMap<usize, Vec<String>>,
    HashMap<usize, Vec<KeepInfo>>,
);

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
    crate::engine::l4::param_guard::alias_error_ops(r, &mut by_site);
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

/// r3 E1: whether call site `cs_id` of `r` passes argument `arg` by value: every
/// routine it reaches takes that parameter by value, or, reaching none, the
/// resolver classified it as a platform method other than `Evaluate` and
/// `Clear` (which write their argument). A call the resolver did not resolve
/// may write it.
fn value_passes_by_value(
    r: &ModelRoutine,
    cs_id: &str,
    arg: u32,
    callees: &HashMap<&str, Vec<&str>>,
    builtin_sites: &HashSet<&str>,
    by_id: &HashMap<&str, &ModelRoutine>,
) -> bool {
    use crate::program::body::features::PCallee;
    match callees.get(cs_id) {
        Some(targets) if !targets.is_empty() => targets.iter().all(|to| {
            by_id
                .get(to)
                .and_then(|callee| callee.parameters.iter().find(|p| p.index == arg))
                .is_some_and(|p| !p.is_var)
        }),
        _ => {
            if !builtin_sites.contains(cs_id) {
                return false;
            }
            let name = r
                .call_sites
                .iter()
                .find(|c| c.id == cs_id)
                .map(|c| match &c.callee {
                    PCallee::Bare { name } => name.to_ascii_lowercase(),
                    PCallee::Member { method, .. } => method.to_ascii_lowercase(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            !matches!(name.as_str(), "evaluate" | "clear")
        }
    }
}

/// r3 E2: the flow programs ([`lflow`]) of the model's routines, the
/// parameters each binds through [`Bindings::flow`] (with their domains), and
/// the publisher keep verdicts the programs use.
#[allow(clippy::type_complexity)]
fn build_flow<'a>(
    model: &'a Model,
    by_id: &HashMap<&str, &'a ModelRoutine>,
    by_stable: &HashMap<&'a str, &'a ModelRoutine>,
    callees: &HashMap<&str, Vec<&str>>,
    builtin_sites: &HashSet<&str>,
    frames: &HashMap<&'a str, FrameGuards>,
    value_frames: &HashMap<&'a str, ValueFrame>,
) -> (
    HashMap<&'a str, lflow::FlowProgram>,
    HashMap<&'a str, Vec<(u32, lflow::FDom)>>,
    Vec<KeepInfo>,
) {
    use crate::program::body::features::PCallee;
    use lflow::ArgWrite;
    let objects: HashMap<&str, &crate::program::model::workspace::ModelObject> = model
        .workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o))
        .collect();
    let display = |r: &ModelRoutine| -> String {
        match objects.get(r.object_id.as_str()) {
            Some(o) if o.name.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                format!("{} {}::{}", o.object_type, o.name, r.name)
            }
            Some(o) => format!("{} \"{}\"::{}", o.object_type, o.name, r.name),
            None => r.name.clone(),
        }
    };
    // The universe's subscriptions, by event id; the names of events some
    // subscription could not bind to a publisher object; and whether any model
    // subscriber's subscriptions are missing from the graph.
    let census = &model.events.census;
    let census_uncertain =
        census.unmapped_subscribers > 0 || census.subscribers_without_subscription > 0;
    let mut subs: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut unbound: HashSet<String> = HashSet::new();
    for e in &model.events.graph.edges {
        if e.event_id.starts_with("unknown/")
            && let Some((_, n)) = e.event_id.rsplit_once("/event/")
        {
            unbound.insert(n.to_string());
        }
        subs.entry(e.event_id.as_str())
            .or_default()
            .push(e.subscriber_routine_id.as_str());
    }
    let mut keep_info: Vec<KeepInfo> = Vec::new();
    let mut verdicts: HashMap<(&str, u32), Option<u32>> = HashMap::new();
    let mut progs: HashMap<&'a str, lflow::FlowProgram> = HashMap::new();
    for r in &model.workspace.routines {
        let stable = r.stable_routine_id.as_str();
        if !by_stable.get(stable).is_some_and(|x| std::ptr::eq(*x, r)) {
            continue;
        }
        let mut writes_of = |cs: &str, j: u32| -> Option<ArgWrite> {
            let targets = callees.get(cs).map_or(&[][..], Vec::as_slice);
            if targets.is_empty() {
                if !builtin_sites.contains(cs) {
                    return Some(ArgWrite::Havoc);
                }
                let name = r
                    .call_sites
                    .iter()
                    .find(|c| c.id == cs)
                    .map(|c| match &c.callee {
                        PCallee::Bare { name } => name.to_ascii_lowercase(),
                        PCallee::Member { method, .. } => method.to_ascii_lowercase(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                return lflow::WRITING_BUILTINS
                    .contains(&name.as_str())
                    .then_some(ArgWrite::Havoc);
            }
            let mut keep: Option<u32> = None;
            let mut all_keep = true;
            let mut any_var = false;
            for to in targets {
                let Some(c) = by_id.get(to).copied() else {
                    return Some(ArgWrite::Havoc);
                };
                let Some(p) = c.parameters.iter().find(|p| p.index == j) else {
                    return Some(ArgWrite::Havoc);
                };
                if !p.is_var {
                    all_keep = false;
                    continue;
                }
                any_var = true;
                let v = *verdicts.entry((c.id.as_str(), j)).or_insert_with(|| {
                    keep_verdict(
                        c,
                        j,
                        census_uncertain,
                        &subs,
                        &unbound,
                        by_id,
                        callees,
                        builtin_sites,
                    )
                    .map(|mut k| {
                        k.publisher = display(c);
                        k.subscribers = subs
                            .get(
                                crate::program::model::events::encode_event_id(
                                    &c.object_id,
                                    &c.name,
                                )
                                .as_str(),
                            )
                            .into_iter()
                            .flatten()
                            .filter_map(|s| by_id.get(s).map(|s| display(s)))
                            .collect();
                        k.subscribers.sort();
                        k.subscribers.dedup();
                        keep_info.push(k);
                        (keep_info.len() - 1) as u32
                    })
                });
                match (v, keep) {
                    (Some(k), None) => keep = Some(k),
                    (Some(k), Some(was)) if k == was => {}
                    _ => all_keep = false,
                }
            }
            if !any_var {
                return None;
            }
            match (all_keep, keep) {
                (true, Some(k)) => Some(ArgWrite::Keep(k)),
                _ => Some(ArgWrite::Havoc),
            }
        };
        if let Some(p) = lflow::FlowProgram::build(r, &mut writes_of) {
            progs.insert(stable, p);
        }
    }
    // Relevance: the parameters whose entry value can decide a condition of
    // the routine, or is passed to a parameter a callee needs.
    let mask_of = |usable: &mut dyn Iterator<Item = u32>| {
        usable.filter(|&i| i < 64).fold(0u64, |m, i| m | (1 << i))
    };
    let fixed = |c: &str| -> u64 {
        mask_of(
            &mut frames
                .get(c)
                .into_iter()
                .flat_map(|f| f.usable.iter().copied()),
        ) | mask_of(
            &mut value_frames
                .get(c)
                .into_iter()
                .flat_map(|f| f.usable.iter().map(|(i, _)| *i)),
        )
    };
    let stable_of = |to: &str| by_id.get(to).map(|c| c.stable_routine_id.as_str());
    let mut key: HashMap<&str, u64> = progs.iter().map(|(k, p)| (*k, p.guard_mask)).collect();
    loop {
        let mut changed = false;
        for (k, p) in &progs {
            let mut m = key[k];
            for (cs, j, am) in &p.arg_masks {
                if *j >= 64 || m & am == *am {
                    continue;
                }
                let needed = callees
                    .get(cs.as_str())
                    .into_iter()
                    .flatten()
                    .filter_map(|to| stable_of(to))
                    .any(|c| (key.get(c).copied().unwrap_or(0) | fixed(c)) & (1 << j) != 0);
                if needed {
                    m |= am;
                }
            }
            if m != key[k] {
                key.insert(k, m);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let needs = |c: &str| key.get(c).copied().unwrap_or(0) | fixed(c);
    // A program is kept when a condition reads a tracked variable, or a call
    // passes a value the flow may know to a parameter its callee needs.
    let useful: HashSet<&str> = progs
        .iter()
        .filter(|(_, p)| {
            p.has_guard
                || p.known_args.iter().any(|(cs, j)| {
                    *j < 64
                        && callees
                            .get(cs.as_str())
                            .into_iter()
                            .flatten()
                            .filter_map(|to| stable_of(to))
                            .any(|c| needs(c) & (1 << j) != 0)
                })
        })
        .map(|(k, _)| *k)
        .collect();
    progs.retain(|k, _| useful.contains(k));
    let mut bind = HashMap::new();
    for (k, p) in progs.iter_mut() {
        let m = key.get(k).copied().unwrap_or(0);
        p.key_params = (0..64u32)
            .filter(|&i| m & (1 << i) != 0 && p.tracks_param(i))
            .collect();
        let f = fixed(k);
        let list: Vec<(u32, lflow::FDom)> = p
            .key_params
            .iter()
            .filter(|&&i| f & (1 << i) == 0)
            .filter_map(|&i| p.param_dom(i).map(|d| (i, d)))
            .collect();
        if !list.is_empty() {
            bind.insert(*k, list);
        }
    }
    (progs, bind, keep_info)
}

/// r3 E2: whether every loaded subscriber of publisher `c` is proven unable to
/// change its parameter `j` ([`lflow`] module doc); the verdict's statement
/// (publisher and subscribers are filled in by the caller).
#[allow(clippy::too_many_arguments)]
fn keep_verdict(
    c: &ModelRoutine,
    j: u32,
    census_uncertain: bool,
    subs: &HashMap<&str, Vec<&str>>,
    unbound: &HashSet<String>,
    by_id: &HashMap<&str, &ModelRoutine>,
    callees: &HashMap<&str, Vec<&str>>,
    builtin_sites: &HashSet<&str>,
) -> Option<KeepInfo> {
    if c.kind != "event-publisher" || census_uncertain {
        return None;
    }
    let p = c.parameters.iter().find(|p| p.index == j)?;
    let name = values::fold(&p.name);
    if unbound.contains(&c.name.to_lowercase()) {
        return None;
    }
    let id = crate::program::model::events::encode_event_id(&c.object_id, &c.name);
    for sid in subs.get(id.as_str()).into_iter().flatten() {
        let s = by_id.get(sid)?;
        if !s.body_available || s.statement_tree.is_none() {
            return None;
        }
        let passes =
            |cs: &str, a: u32| value_passes_by_value(s, cs, a, callees, builtin_sites, by_id);
        if lflow::subscriber_changes(s, &name, &passes) {
            return None;
        }
    }
    Some(KeepInfo {
        publisher: String::new(),
        parameter: p.name.trim().trim_matches('"').to_string(),
        subscribers: Vec::new(),
    })
}

/// The edge's (from, to, site, binds-arguments) view.
fn edge_parts(e: &SnapshotGraphEdge) -> Option<(&str, &str, Option<EdgeSite<'_>>, bool)> {
    fn cs(s: &str) -> Option<EdgeSite<'_>> {
        Some(EdgeSite::Guarded(s))
    }
    Some(match e {
        SnapshotGraphEdge::DirectCall {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, cs(callsite_id), true),
        SnapshotGraphEdge::VariableTypedCall {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, cs(callsite_id), true),
        // G34: an implementer takes the call's arguments by position, as a
        // direct callee does.
        SnapshotGraphEdge::InterfaceDispatch {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, cs(callsite_id), true),
        SnapshotGraphEdge::ObjectRunResolved {
            from,
            to,
            callsite_id,
            ..
        } => (from, to, cs(callsite_id), false),
        // MCP output wave 2 (W2-0a, design §3.4.3 item 1): an event edge
        // carries its event id, so a crossing's event edge can be matched by
        // identity. It is never a guarded site (an event edge has no guard).
        SnapshotGraphEdge::EventDispatch {
            from, to, event_id, ..
        } => (from, to, Some(EdgeSite::Event(event_id.as_str())), false),
        // F1: a table event leaves from its raising operation, whose guards
        // apply as an implicit trigger's do.
        SnapshotGraphEdge::ImplicitTrigger {
            from,
            to,
            operation_id,
            ..
        }
        | SnapshotGraphEdge::TableEvent {
            from,
            to,
            operation_id,
            ..
        } => (
            from,
            to,
            Some(EdgeSite::Guarded(operation_id.as_str())),
            false,
        ),
        SnapshotGraphEdge::ObjectRunUnresolved { .. } => return None,
    })
    .map(|(f, t, s, b)| (f.as_str(), t.as_str(), s, b))
}

/// The identity an out-edge leaves from: a call site or firing operation (a
/// site whose guards the search applies), or an event (no guard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeSite<'a> {
    Guarded(&'a str),
    Event(&'a str),
}

impl<'a> EdgeSite<'a> {
    /// The site whose guards apply; `None` for an event edge.
    fn guarded(self) -> Option<&'a str> {
        match self {
            EdgeSite::Guarded(s) => Some(s),
            EdgeSite::Event(_) => None,
        }
    }
}

/// An out-edge: (callee, the site it leaves from, whether it binds arguments,
/// the snapshot edge kind).
type OutEdge<'a> = (&'a str, Option<EdgeSite<'a>>, bool, &'a str);

/// A search state: a routine and the known values of its parameters.
type State<'a> = (&'a str, Bindings);

/// The edge a tree state was first reached by: its site and edge kind.
type Via<'a> = (Option<EdgeSite<'a>>, &'a str);

/// One edge of a witness: from, to, the site it leaves from, its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Step<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub site: Option<EdgeSite<'a>>,
    pub kind: &'a str,
}

/// Why a guarded search stopped before it was complete (MCP output wave 2
/// design §3.4.3 item 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// It visited more than its state cap.
    Capped,
    /// The caller's cancellation check said stop.
    Cancelled,
}

/// A search's result: the routines reached with guards ignored, and the
/// binding states reached per routine with them.
type Reached<'a> = (HashSet<&'a str>, HashMap<&'a str, Vec<Bindings>>);

/// G35: a guarded search's states with their parents, in visit order
/// ([`LiteralReach::live_tree`]).
pub(crate) struct LiveTree<'a> {
    /// Each state's parent and the edge from it (`None` at the start).
    parent: HashMap<State<'a>, Option<(State<'a>, Via<'a>)>>,
    order: Vec<State<'a>>,
    /// W2-0b: routine → its states' positions in `order`, ascending; built
    /// on first use, so a witness for one resource is a lookup, not a scan
    /// of the whole tree.
    by_routine: std::cell::OnceCell<HashMap<&'a str, Vec<u32>>>,
}

impl<'a> LiveTree<'a> {
    /// The positions in visit order of `routine`'s states.
    fn states_of(&self, routine: &str) -> &[u32] {
        self.by_routine
            .get_or_init(|| {
                let mut m: HashMap<&'a str, Vec<u32>> = HashMap::new();
                for (i, (f, _)) in self.order.iter().enumerate() {
                    m.entry(*f)
                        .or_default()
                        .push(u32::try_from(i).expect("a tree position fits u32"));
                }
                m
            })
            .get(routine)
            .map_or(&[], Vec::as_slice)
    }

    /// The states visited, in visit order.
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    /// The edges from the tree's start to `state`, in order.
    fn steps_to(&self, state: &State<'a>) -> Vec<Step<'a>> {
        let mut steps = Vec::new();
        let mut cur = state.clone();
        while let Some(Some((p, (site, kind)))) = self.parent.get(&cur) {
            steps.push(Step {
                from: p.0,
                to: cur.0,
                site: *site,
                kind,
            });
            cur = p.clone();
        }
        steps.reverse();
        steps
    }
}

/// The search context over one snapshot and model, shared by every root.
pub(crate) struct LiteralReach<'a> {
    out: HashMap<&'a str, Vec<OutEdge<'a>>>,
    by_stable: HashMap<&'a str, &'a ModelRoutine>,
    frames: HashMap<&'a str, FrameGuards>,
    tables: HashMap<&'a str, TableFrame>,
    /// r3 E1: per routine (stable id) its value frame ([`values`]).
    value_frames: HashMap<&'a str, ValueFrame>,
    /// r3 E2: per routine (stable id) its flow program ([`lflow`]), the
    /// parameters a call binds through [`Bindings::flow`], the keep verdicts
    /// the programs name, and the evaluations made so far.
    flow_progs: HashMap<&'a str, lflow::FlowProgram>,
    flow_bind: HashMap<&'a str, Vec<(u32, lflow::FDom)>>,
    keep_info: Vec<KeepInfo>,
    flow_cache: std::sync::Mutex<HashMap<FlowKey, Arc<lflow::FlowEval>>>,
    /// Model table id -> the table (a real one over an extension stub), for
    /// the labels of a record field ([`values::resolve_label`]).
    tables_by_id: HashMap<&'a str, &'a crate::program::model::workspace::ModelTable>,
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
    /// The cone's DIRECT facts, one lossless record each (MCP output wave 2,
    /// W2-0a: [`TerminalFacts`]); [`Self::kind_dead`] reads them by kind,
    /// (effect type, resource id).
    terminals: TerminalFacts<'a>,
}

impl<'a> LiteralReach<'a> {
    pub(crate) fn new(
        edges: &'a [SnapshotGraphEdge],
        facts: &'a [crate::engine::l5::snapshot::SnapshotCapabilityFact],
        model: &'a Model,
    ) -> Self {
        // The strings are borrowed from the rows' tables, which the rows of
        // `facts` keep alive for `'a`.
        let terminals = TerminalFacts::new(facts);
        let routines = &model.workspace.routines;
        let by_id: HashMap<&str, &ModelRoutine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut callees: HashMap<&str, Vec<&str>> = HashMap::new();
        // r3 E1: the call sites the resolver classified as platform methods.
        let mut builtin_sites: HashSet<&str> = HashSet::new();
        for e in &model.calls.edges {
            if let Some(to) = e.to.as_deref() {
                callees.entry(e.callsite_id.as_str()).or_default().push(to);
            }
            if e.resolution == crate::program::model::taxonomy::Resolution::Builtin {
                builtin_sites.insert(e.callsite_id.as_str());
            }
        }
        let tables_by_id =
            crate::program::model::workspace::table_by_id_preferring_real(&model.workspace.tables);
        let mut frames = HashMap::new();
        let mut tables = HashMap::new();
        let mut value_frames = HashMap::new();
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
            let passes_by_value = |cs_id: &str, arg: u32| {
                value_passes_by_value(r, cs_id, arg, &callees, &builtin_sites, &by_id)
            };
            let resolve = |l: &crate::program::body::features::ValueLabel| {
                values::resolve_label(r, &tables_by_id, l)
            };
            if let Some(v) = values::value_frame(r, passes_by_value, &resolve) {
                value_frames.insert(r.stable_routine_id.as_str(), v);
            }
            for op in &r.record_operations {
                op_owner.insert(op.id.as_str(), r.stable_routine_id.as_str());
            }
            for op in &r.operation_sites {
                op_owner.insert(op.id.as_str(), r.stable_routine_id.as_str());
            }
            for cs in &r.call_sites {
                op_owner.insert(cs.operation_id.as_str(), r.stable_routine_id.as_str());
                // G46: a boundary group's crossing is named by call site id.
                op_owner.insert(cs.id.as_str(), r.stable_routine_id.as_str());
            }
        }
        let (flow_progs, flow_bind, keep_info) = build_flow(
            model,
            &by_id,
            &by_stable,
            &callees,
            &builtin_sites,
            &frames,
            &value_frames,
        );
        let table_names = model
            .workspace
            .tables
            .iter()
            .map(|t| (t.id.as_str(), Arc::from(t.name.to_lowercase())))
            .collect();
        let mut out: HashMap<&str, Vec<OutEdge>> = HashMap::new();
        for e in edges {
            if let Some((from, to, site, binds)) = edge_parts(e) {
                out.entry(from)
                    .or_default()
                    .push((to, site, binds, e.edge_kind()));
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
            value_frames,
            flow_progs,
            flow_bind,
            keep_info,
            flow_cache: std::sync::Mutex::new(HashMap::new()),
            tables_by_id,
            table_names,
            op_owner,
            flow,
            guards,
            rel_params,
            rel_globals,
            terminals,
        }
    }

    /// The tree's terminal records (design §3.4.1). Read by W2-0b's
    /// crossing-constrained witnesses; until then by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn terminals(&self) -> &TerminalFacts<'a> {
        &self.terminals
    }

    /// The out-edges of `from` that are event dispatches, as (subscriber,
    /// event id): an event edge's identity on the search's own adjacency.
    /// Read by W2-0b's prefix phase; until then by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn event_edges_from(&self, from: &str) -> Vec<(&'a str, &'a str)> {
        self.out
            .get(from)
            .into_iter()
            .flatten()
            .filter_map(|&(to, site, _, _)| match site {
                Some(EdgeSite::Event(ev)) => Some((to, ev)),
                _ => None,
            })
            .collect()
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

    fn value_reqs_at(&self, routine: &str, site: &str) -> &[ValueReq] {
        self.value_frames
            .get(routine)
            .and_then(|f| f.by_site.get(site))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `site` of `routine` cannot run under `b`.
    fn dead_at(&self, routine: &str, site: &str, b: &Bindings) -> bool {
        self.dead_at_m(routine, site, b, true)
    }

    /// [`Self::dead_at`]; `keep` says whether the flow follows publisher keeps
    /// (r3 E2, [`lflow`]).
    fn dead_at_m(&self, routine: &str, site: &str, b: &Bindings, keep: bool) -> bool {
        contradicts(self.reqs_at(routine, site), b)
            || contradicts_tables(self.table_reqs_at(routine, site), b)
            || values::contradicts(self.value_reqs_at(routine, site), b)
            || self
                .guards
                .get(routine)
                .and_then(|g| g.get(site))
                .is_some_and(|reqs| contradicts_atoms(reqs, b))
            || self.flow_progs.get(routine).is_some_and(|p| {
                self.flow_eval(routine, b, keep)
                    .is_some_and(|ev| p.dead(&ev, site))
            })
    }

    /// r3 E2: the evaluation of `routine`'s flow program under `b`'s entry
    /// values, cached by its key. `None` when the routine has no program.
    fn flow_eval(&self, routine: &str, b: &Bindings, keep: bool) -> Option<Arc<lflow::FlowEval>> {
        let p = self.flow_progs.get(routine)?;
        let entry: Vec<(u32, VSet)> = p
            .key_params
            .iter()
            .filter_map(|&i| b.entry_value(i).map(|v| (i, v)))
            .collect();
        let key: FlowKey = (p as *const lflow::FlowProgram as usize, entry, keep);
        if let Some(ev) = self.flow_cache.lock().expect("flow cache").get(&key) {
            return Some(Arc::clone(ev));
        }
        let r = self.by_stable.get(routine)?;
        let resolve = |l: &crate::program::body::features::ValueLabel| {
            values::resolve_label(r, &self.tables_by_id, l)
        };
        let ev = Arc::new(p.eval(&key.1, &resolve, &|_| keep, None));
        let mut cache = self.flow_cache.lock().expect("flow cache");
        if cache.len() >= FLOW_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&ev));
        Some(ev)
    }

    /// r3 E2: the keep verdicts the flow used at `routine` under `b`.
    fn keeps_at(&self, routine: &str, b: &Bindings) -> Vec<u32> {
        self.flow_eval(routine, b, true)
            .map_or_else(Vec::new, |ev| ev.keeps.clone())
    }

    /// The callee's bindings across a call at `site` of `caller` with `b`.
    fn bind(&self, caller: &str, site: &str, callee: &str, b: &Bindings) -> Bindings {
        self.bind_m(caller, site, callee, b, true)
    }

    /// [`Self::bind`]; `keep` as in [`Self::dead_at_m`].
    fn bind_m(&self, caller: &str, site: &str, callee: &str, b: &Bindings, keep: bool) -> Bindings {
        let (f, t) = (self.frames.get(callee), self.tables.get(callee));
        let vf = self.value_frames.get(callee);
        let fb = self.flow_bind.get(callee);
        let ci = self.flow.index_of(callee);
        let rel_params = ci.and_then(|c| self.rel_params.get(&c));
        let rel_globals = ci.and_then(|c| {
            self.rel_globals
                .get(self.flow.routine(c).object_id.as_str())
        });
        if f.is_none()
            && t.is_none()
            && vf.is_none()
            && fb.is_none()
            && rel_params.is_none()
            && rel_globals.is_none()
        {
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
        // r3 E2: what the caller's flow knows of each argument at this call
        // (its locals, a parameter it changed, a derived value).
        let caller_flow = self
            .flow_progs
            .get(caller)
            .zip(self.flow_eval(caller, b, keep));
        let flow_arg = |j: u32| -> Option<VSet> {
            let (p, ev) = caller_flow.as_ref()?;
            p.args_at(ev, site)
                .iter()
                .find(|(k, _)| *k == j)
                .map(|(_, v)| Arc::clone(v))
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
                    .and_then(|i| b.bools.iter().find(|&&(k, _)| k == i).map(|&(_, w)| w))
                    .or_else(|| match flow_arg(j).as_deref() {
                        Some([values::Val::Bool(v)]) => Some(*v),
                        _ => None,
                    }),
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
        // r3 E1: an integer literal or a label (read in the CALLER's scope),
        // else the caller's own known value parameter it forwards; each in
        // the callee parameter's domain, else unknown.
        for (j, d) in vf.map_or(&[][..], |v| v.usable.as_slice()) {
            let literal = cs
                .argument_infos
                .get(*j as usize)
                .and_then(crate::program::body::features::ValueLabel::of_info)
                .and_then(|l| values::resolve_label(r, &self.tables_by_id, &l))
                .and_then(|v| values::in_domain(&v, d))
                .map(|v| VSet::from([v]));
            let in_d = |held: &[values::Val]| -> Option<VSet> {
                let mut vs = held
                    .iter()
                    .map(|v| values::in_domain(v, d))
                    .collect::<Option<Vec<_>>>()?;
                vs.sort();
                vs.dedup();
                Some(VSet::from(vs))
            };
            let set = literal
                .or_else(|| {
                    let i = forwarded(*j)?;
                    let (_, held) = b.values.iter().find(|(k, _)| *k == i)?;
                    in_d(held)
                })
                // r3 E1's last rule, wired by E2: a caller's tracked local.
                .or_else(|| flow_arg(*j).and_then(|v| in_d(&v)));
            if let Some(set) = set {
                out.values.push((*j, set));
            }
        }
        // r3 E2: the parameters the callee's flow reads that the lists above
        // do not bind: what the caller's flow knows of the argument, else a
        // literal, else the caller's own known parameter it forwards.
        for (j, d) in fb.map_or(&[][..], Vec::as_slice) {
            let raw = flow_arg(*j)
                .or_else(|| {
                    let text = cs
                        .argument_texts
                        .get(*j as usize)?
                        .trim()
                        .to_ascii_lowercase();
                    match text.as_str() {
                        "true" => Some(Arc::from([values::Val::Bool(true)])),
                        "false" => Some(Arc::from([values::Val::Bool(false)])),
                        _ => cs
                            .argument_infos
                            .get(*j as usize)
                            .and_then(crate::program::body::features::ValueLabel::of_info)
                            .and_then(|l| values::resolve_label(r, &self.tables_by_id, &l))
                            .map(|v| Arc::from([v])),
                    }
                })
                .or_else(|| forwarded(*j).and_then(|i| b.entry_value(i)));
            if let Some(set) = raw.and_then(|v| lflow::conv_set(&v, d)) {
                out.flow.push((*j, set));
            }
        }
        // G34: what the value flow knows of the callee's relevant parameters
        // (G33's included, where it knows more), and of its instance's globals.
        if let (Some(c), Some(fi)) = (ci, self.flow.index_of(caller)) {
            for &j in rel_params.into_iter().flatten() {
                if let Some(set) = self.flow.arg_tables(fi, cs, j, &b.tables, &b.globals) {
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
        out.values.sort();
        out.flow.sort();
        out
    }

    /// Every routine reached from `root` with guards ignored, and the binding
    /// states reached with them; `None` when the guarded search hit the cap.
    fn search(&self, root: &'a str, keep: bool) -> Option<Reached<'a>> {
        let mut plain: HashSet<&str> = HashSet::from([root]);
        let mut q = VecDeque::from([root]);
        while let Some(f) = q.pop_front() {
            for &(to, _, _, _) in self.out.get(f).into_iter().flatten() {
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
            for &(to, site, binds, _) in self.out.get(f).into_iter().flatten() {
                let site = site.and_then(EdgeSite::guarded);
                if let Some(s) = site
                    && self.dead_at_m(f, s, &b, keep)
                {
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind_m(f, s, to, &b, keep),
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
    /// its order of visit, so [`Self::group_witness`] can read a path that can
    /// run off it. `None` when the search hit the cap. (A read names its
    /// roots by [`Self::live_tree_of`].)
    pub(crate) fn live_tree(&self, root: &'a str) -> Option<LiveTree<'a>> {
        self.tree_from((root, Bindings::default()), STATE_CAP, &|| false)
            .ok()
    }

    /// [`Self::live_tree`] of a root named by a string of any lifetime (a
    /// read's own entry): the search runs from the model's own copy of the
    /// id, so its states may outlive the entry (a streamed read's suffix
    /// trees serve every batch). `None` too for an id the model has no row
    /// for (every entry's root is a model routine), as for a capped search:
    /// nothing is pruned on it.
    pub(crate) fn live_tree_of(&self, root: &str) -> Option<LiveTree<'a>> {
        let (&root, _) = self.by_stable.get_key_value(root)?;
        self.tree_from((root, Bindings::default()), STATE_CAP, &|| false)
            .ok()
    }

    /// The guarded search from `start` (a routine entered with known
    /// bindings), keeping each state's parent and the edge from it, in visit
    /// order. Stops at `cap` states, or when `cancel` says so (checked every
    /// [`CANCEL_EVERY`] states).
    fn tree_from(
        &self,
        start: State<'a>,
        cap: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<LiveTree<'a>, Stop> {
        self.tree_recording(start, cap, cancel, None)
    }

    /// [`Self::tree_from`], and when `rec` is given every transition the
    /// search examines, live or not (agent-tasks E2: the state-transition
    /// graph [`routes::StateGraph`] is built from them). The search itself
    /// is the same with or without `rec`: the same states, in the same
    /// order, with the same first parents.
    fn tree_recording(
        &self,
        start: State<'a>,
        cap: usize,
        cancel: &dyn Fn() -> bool,
        rec: Option<&mut routes::Recorder<'a>>,
    ) -> Result<LiveTree<'a>, Stop> {
        self.tree_recording_m(start, cap, cancel, rec, true)
    }

    /// [`Self::tree_recording`]; `keep` as in [`Self::dead_at_m`].
    fn tree_recording_m(
        &self,
        start: State<'a>,
        cap: usize,
        cancel: &dyn Fn() -> bool,
        mut rec: Option<&mut routes::Recorder<'a>>,
        keep: bool,
    ) -> Result<LiveTree<'a>, Stop> {
        let mut parent: HashMap<State<'a>, Option<(State<'a>, Via<'a>)>> =
            HashMap::from([(start.clone(), None)]);
        let mut order: Vec<State<'a>> = Vec::new();
        if let Some(r) = rec.as_deref_mut() {
            r.start(start.clone());
        }
        let mut q = VecDeque::from([start]);
        while let Some((f, b)) = q.pop_front() {
            if order.len().is_multiple_of(CANCEL_EVERY) && cancel() {
                return Err(Stop::Cancelled);
            }
            order.push((f, b.clone()));
            if let Some(r) = rec.as_deref_mut() {
                r.open_state();
            }
            for (j, &(to, edge_site, binds, kind)) in
                self.out.get(f).into_iter().flatten().enumerate()
            {
                let site = edge_site.and_then(EdgeSite::guarded);
                if let Some(s) = site
                    && self.dead_at_m(f, s, &b, keep)
                {
                    if let Some(r) = rec.as_deref_mut() {
                        r.dead(j);
                    }
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind_m(f, s, to, &b, keep),
                    _ => Bindings::default(),
                };
                let key = (to, nb);
                if !parent.contains_key(&key) {
                    if parent.len() > cap {
                        return Err(Stop::Capped);
                    }
                    if let Some(r) = rec.as_deref_mut() {
                        // A new state's index is its discovery position,
                        // which is its position in `order` (the queue is
                        // first in, first out).
                        r.live_new(j, key.clone(), parent.len());
                    }
                    parent.insert(key.clone(), Some(((f, b.clone()), (edge_site, kind))));
                    q.push_back(key);
                } else if let Some(r) = rec.as_deref_mut() {
                    r.live_seen(j, &key);
                }
            }
        }
        Ok(LiveTree {
            parent,
            order,
            by_routine: std::cell::OnceCell::new(),
        })
    }

    /// Over `tree`, the first state (in visit order) of a routine owning one
    /// of `ops` where that operation can run, with that operation.
    fn first_live_owner<'t>(
        &self,
        tree: &'t LiveTree<'a>,
        ops: &'t [String],
    ) -> Option<(&'t State<'a>, &'t str)> {
        // The earliest position over every op's owner states where the op can
        // run; at one position the first op in `ops` order (as a scan of the
        // tree in visit order would find).
        let mut best: Option<(u32, &'t str)> = None;
        for o in ops {
            let Some(owner) = self.op_owner.get(o.as_str()) else {
                continue;
            };
            for &i in tree.states_of(owner) {
                if best.is_some_and(|(b, _)| b <= i) {
                    break;
                }
                let (f, b) = &tree.order[i as usize];
                if !self.dead_at(f, o, b) {
                    best = Some((i, o.as_str()));
                    break;
                }
            }
        }
        best.map(|(i, o)| (&tree.order[i as usize], o))
    }

    /// A witness over `tree` to the first state (in visit order) of a
    /// routine owning one of `ops` where that operation can run: the edges
    /// from the tree's start and the operation (final CDO output review,
    /// item 2: an own row whose listed paths all cannot run).
    pub(crate) fn tree_witness<'t>(
        &self,
        tree: &'t LiveTree<'a>,
        ops: &'t [String],
    ) -> Option<(Vec<Step<'a>>, &'t str)> {
        let (state, op) = self.first_live_owner(tree, ops)?;
        Some((tree.steps_to(state), op))
    }

    /// Whether `tree` has a state of `routine` (stable id): the guarded
    /// search reaches it under some bindings (agent-tasks E1: a fact with no
    /// site of its own is in the root's reach exactly then).
    pub(crate) fn reaches(&self, tree: &LiveTree<'a>, routine: &str) -> bool {
        !tree.states_of(routine).is_empty()
    }

    /// Whether `root` itself owns operation `op` and it can run at the
    /// root's start state (no known values): a zero-hop witness.
    pub(crate) fn live_in_root(&self, root: &str, op: &str) -> bool {
        self.op_owner.get(op).is_some_and(|o| *o == root)
            && !self.dead_at(root, op, &Bindings::default())
    }

    /// The ids of the routine owning operation `op` (stable id), if known.
    pub(crate) fn op_owner(&self, op: &str) -> Option<&'a str> {
        self.op_owner.get(op).copied()
    }

    /// The model routine of stable id `stable`.
    pub(crate) fn routine(&self, stable: &str) -> Option<&'a ModelRoutine> {
        self.by_stable.get(stable).copied()
    }

    /// Whether the edge `from → to` leaving at `site` is one of `crossing`'s
    /// typed edges (design §3.3): matched by source, target AND site
    /// identity, never by the site id alone; an event edge by its event id.
    fn on_crossing(c: &Crossing, from: &str, to: &str, site: Option<EdgeSite<'_>>) -> bool {
        c.edges.iter().any(|e| {
            e.from == from
                && e.to == to
                && match (site, &e.site) {
                    (
                        Some(EdgeSite::Guarded(s)),
                        CrossingSite::Callsite(x) | CrossingSite::Operation(x),
                    ) => s == x,
                    (Some(EdgeSite::Event(s)), CrossingSite::Event(x)) => s == x,
                    _ => false,
                }
        })
    }

    /// MCP output wave 2, W2-0b (design §3.4.3; G1): one witness from the
    /// root to a live operation of `ops` that TRAVERSES `crossing`.
    ///
    /// Two phases. The prefix is `root_tree` (the root's guarded search):
    /// every `(caller state, callee bindings)` at which one of the crossing's
    /// typed edges is taken is an entry, so the bindings the prefix
    /// established survive. The suffix is one guarded search per distinct
    /// entry `(target, bindings)`, from [`SuffixCache`]: it depends on
    /// nothing else, so prefix + suffix is a witness, and the shortest total
    /// wins (ties: the first entry in the root tree's visit order). When no
    /// entry reaches an operation and some suffix search stopped early, the
    /// root tree's own path is the fallback, CHECKED for crossing membership
    /// (an ordinary witness when it does traverse the crossing).
    pub(crate) fn group_witness(
        &self,
        root_tree: &LiveTree<'a>,
        crossing: &Crossing,
        entries: &CrossingEntries<'a>,
        ops: &[String],
        cache: &mut SuffixCache<'a>,
        cancel: &dyn Fn() -> bool,
    ) -> GroupWitness<'a> {
        // Suffix phase.
        let mut best: Option<(usize, Vec<Step<'a>>, usize, String)> = None;
        let mut stopped: Option<Stop> = None;
        for (caller, step, nb) in &entries.0 {
            let prefix = root_tree.steps_to(caller);
            if best.as_ref().is_some_and(|b| prefix.len() + 1 >= b.0) {
                continue;
            }
            let tree = match cache.get(self, (step.to, nb.clone()), cancel) {
                Ok(t) => t,
                Err(s) => {
                    stopped = stopped.or(Some(s));
                    continue;
                }
            };
            if let Some((state, op)) = self.first_live_owner(&tree, ops) {
                let suffix = tree.steps_to(state);
                let total = prefix.len() + 1 + suffix.len();
                if best.as_ref().is_none_or(|b| total < b.0) {
                    let entry = prefix.len();
                    let mut steps = prefix;
                    steps.push(*step);
                    steps.extend(suffix);
                    best = Some((total, steps, entry, op.to_string()));
                }
            }
        }
        if let Some((_, steps, entry, terminal)) = best {
            return GroupWitness::Through {
                steps,
                entry,
                terminal,
            };
        }
        let Some(stop) = stopped else {
            return GroupWitness::Unavailable(None);
        };
        // A suffix stopped early: the root tree's own path, checked.
        let Some((state, op)) = self.first_live_owner(root_tree, ops) else {
            return GroupWitness::Unavailable(Some(stop));
        };
        let steps = root_tree.steps_to(state);
        let terminal = op.to_string();
        match steps
            .iter()
            .position(|s| Self::on_crossing(crossing, s.from, s.to, s.site))
        {
            Some(entry) => GroupWitness::Through {
                steps,
                entry,
                terminal,
            },
            None => GroupWitness::Outside {
                steps,
                terminal,
                reason: stop,
            },
        }
    }

    /// The prefix phase (design §3.4.3 item 1): every `(caller state, edge,
    /// callee bindings)` at which one of `crossing`'s typed edges is taken in
    /// `root_tree`, distinct by callee state, in root-tree visit order (so
    /// the first entry of a callee state has its shortest prefix). ALL
    /// entries are kept: the shortest prefix need not witness every resource.
    pub(crate) fn crossing_entries(
        &self,
        root_tree: &LiveTree<'a>,
        crossing: &Crossing,
    ) -> CrossingEntries<'a> {
        let mut entries: Vec<(State<'a>, Step<'a>, Bindings)> = Vec::new();
        let mut seen: HashSet<State<'a>> = HashSet::new();
        // Only the states of the crossing edges' source routines, in visit
        // order.
        let mut at: Vec<u32> = crossing
            .edges
            .iter()
            .map(|e| e.from.as_str())
            .collect::<HashSet<&str>>()
            .into_iter()
            .flat_map(|f| root_tree.states_of(f).iter().copied())
            .collect();
        at.sort_unstable();
        for (f, b) in at.iter().map(|&i| &root_tree.order[i as usize]) {
            for &(to, edge_site, binds, kind) in self.out.get(f).into_iter().flatten() {
                if !Self::on_crossing(crossing, f, to, edge_site) {
                    continue;
                }
                let site = edge_site.and_then(EdgeSite::guarded);
                if let Some(s) = site
                    && self.dead_at(f, s, b)
                {
                    continue;
                }
                let nb = match (binds, site) {
                    (true, Some(s)) => self.bind(f, s, to, b),
                    _ => Bindings::default(),
                };
                if seen.insert((to, nb.clone())) {
                    let step = Step {
                        from: f,
                        to,
                        site: edge_site,
                        kind,
                    };
                    entries.push(((f, b.clone()), step, nb));
                }
            }
        }
        CrossingEntries(entries)
    }

    /// The routines `from` reaches over this search's edges with guards
    /// ignored.
    fn plain_reach(&self, from: &'a str) -> HashSet<&'a str> {
        let mut seen: HashSet<&'a str> = HashSet::from([from]);
        let mut q = VecDeque::from([from]);
        while let Some(f) = q.pop_front() {
            for &(to, _, _, _) in self.out.get(f).into_iter().flatten() {
                if seen.insert(to) {
                    q.push_back(to);
                }
            }
        }
        seen
    }

    /// W2-0c: the operations that could write `resource` as an effect of
    /// type `effect_type` in a group of class `kt` (known temporary or not):
    /// EVERY direct fact of that kind (the terminal records), not only the
    /// group's representatives, whose class can be the group's — a producer
    /// (parameter-dependent) record could be either at a caller, so it is
    /// always a candidate. `None` when a candidate names no operation (it
    /// cannot be judged).
    pub(crate) fn candidate_ops(
        &self,
        effect_type: &str,
        resource: &str,
        kt: bool,
        representatives: &[String],
    ) -> Option<Vec<String>> {
        use crate::engine::l5::terminal_facts::{TerminalSite, TerminalTemp};
        let mut out: Vec<String> = representatives.to_vec();
        for r in self.terminals.of_kind((effect_type, Some(resource))) {
            let class_fits = match r.temp {
                TerminalTemp::ParameterDependent(_) => true,
                TerminalTemp::Known(true) => kt,
                _ => !kt,
            };
            if !class_fits {
                continue;
            }
            match r.terminal {
                TerminalSite::None => return None,
                t => out.push(t.id().to_string()),
            }
        }
        out.sort();
        out.dedup();
        Some(out)
    }

    /// W2-6 (2f, design §3.4.2): the operations a resource-less group of
    /// `effect_type` (COMMIT, HTTP, UI, …) could end at: every direct fact of
    /// that type with no resource, sorted. `None` when one names no
    /// operation (it cannot be witnessed).
    pub(crate) fn terminal_ops(&self, effect_type: &str) -> Option<Vec<String>> {
        use crate::engine::l5::terminal_facts::TerminalSite;
        let mut out = Vec::new();
        for r in self.terminals.of_kind((effect_type, None)) {
            match r.terminal {
                TerminalSite::None => return None,
                t => out.push(t.id().to_string()),
            }
        }
        out.sort();
        out.dedup();
        Some(out)
    }

    /// W2-0c (design §3.3.1): `ops`' status beyond a crossing, from its
    /// entry states (`entries`, the incoming binding states).
    pub(crate) fn resource_status(
        &self,
        entries: &CrossingEntries<'a>,
        ops: &[String],
        cache: &mut SuffixCache<'a>,
        cancel: &dyn Fn() -> bool,
    ) -> ResourceStatus {
        if entries.0.is_empty() {
            return ResourceStatus::NotEstablished(Undecided::NoEntry);
        }
        if ops.is_empty() {
            return ResourceStatus::NotEstablished(Undecided::NoOperation);
        }
        let mut stopped = false;
        let mut owner_reached = false;
        for (_, step, nb) in &entries.0 {
            match cache.get(self, (step.to, nb.clone()), cancel) {
                Ok(tree) => {
                    if self.first_live_owner(&tree, ops).is_some() {
                        return ResourceStatus::Reachable;
                    }
                }
                Err(_) => stopped = true,
            }
            if !owner_reached {
                let plain = cache.plain(self, step.to);
                owner_reached = ops.iter().any(|o| {
                    self.op_owner
                        .get(o.as_str())
                        .is_some_and(|w| plain.contains(w))
                });
            }
        }
        if stopped {
            ResourceStatus::NotEstablished(Undecided::SearchStopped)
        } else if !owner_reached {
            ResourceStatus::NotEstablished(Undecided::OwnerNotReached)
        } else {
            ResourceStatus::Disproved
        }
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

    /// r3 E2: the guards on a listed witness route, outermost first: per hop
    /// those its call site sits under in its routine, then those of the
    /// first of `terminal_ops` the last routine owns. Each is `evaluated`
    /// when the routine's flow, under the route's bindings (replayed as
    /// [`Self::path_live`] does), decides it in the route's favour.
    pub(crate) fn route_guards(
        &self,
        root: &str,
        path: &[crate::engine::l5::digest::ProjectedHop],
        terminal_ops: &[String],
    ) -> Vec<RouteGuard> {
        let mut out = Vec::new();
        let mut b = Bindings::default();
        let mut cur: String = root.to_string();
        for hop in path {
            let h = &hop.inner;
            let from = h.from_routine_id.as_str();
            if let Some(s) = h.callsite_id.as_deref().or(h.operation_id.as_deref()) {
                out.extend(self.guards_at(from, s, &b));
            }
            let Some(to) = h.to_routine_id.as_deref() else {
                return out;
            };
            b = match (h.edge_kind.as_deref(), h.callsite_id.as_deref()) {
                (Some("direct-call" | "variable-typed-call" | "interface-dispatch"), Some(cs)) => {
                    self.bind(from, cs, to, &b)
                }
                _ => Bindings::default(),
            };
            cur = to.to_string();
        }
        if let Some(op) = terminal_ops
            .iter()
            .find(|o| self.op_owner.get(o.as_str()).is_some_and(|w| *w == cur))
        {
            out.extend(self.guards_at(&cur, op, &b));
        }
        out
    }

    /// r3 E2: [`Self::route_guards`] of a searched witness (`steps` from the
    /// root, ending at operation `op`; no step: `op` is the root's own).
    pub(crate) fn route_guards_steps(
        &self,
        root: &str,
        steps: &[Step<'_>],
        op: &str,
    ) -> Vec<RouteGuard> {
        let mut out = Vec::new();
        let mut b = Bindings::default();
        let mut cur: &str = root;
        for s in steps {
            let site = s.site.and_then(EdgeSite::guarded);
            if let Some(site) = site {
                out.extend(self.guards_at(s.from, site, &b));
            }
            b = match (s.kind, site) {
                ("direct-call" | "variable-typed-call" | "interface-dispatch", Some(cs)) => {
                    self.bind(s.from, cs, s.to, &b)
                }
                _ => Bindings::default(),
            };
            cur = s.to;
        }
        out.extend(self.guards_at(cur, op, &b));
        out
    }

    /// The guards `site` sits under in `routine` (r3 E2), evaluated under `b`.
    fn guards_at(&self, routine: &str, site: &str, b: &Bindings) -> Vec<RouteGuard> {
        let Some(r) = self.by_stable.get(routine) else {
            return Vec::new();
        };
        let prog = self.flow_progs.get(routine);
        let tree: &PCFNNode = match (prog, &r.statement_tree) {
            (Some(p), _) => p.tree(),
            (None, Some(t)) => t,
            (None, None) => return Vec::new(),
        };
        let Some(guards) = lflow::guards_of(tree, site) else {
            return Vec::new();
        };
        let resolve = |l: &crate::program::body::features::ValueLabel| {
            values::resolve_label(r, &self.tables_by_id, l)
        };
        let mut conds = HashMap::new();
        if let Some(p) = prog {
            let entry: Vec<(u32, VSet)> = p
                .key_params
                .iter()
                .filter_map(|&i| b.entry_value(i).map(|v| (i, v)))
                .collect();
            p.eval(&entry, &resolve, &|_| true, Some(&mut conds));
        }
        guards
            .into_iter()
            .map(|(node, kind)| RouteGuard {
                routine: r.name.clone(),
                line: node.source_range.map(|s| s.0 + 1),
                text: match node.flow.as_deref() {
                    Some(crate::program::body::features::FlowFact::Cond { text, .. }) => {
                        text.to_string()
                    }
                    _ => String::new(),
                },
                evaluated: prog.is_some_and(|p| p.guard_evaluated(node, &kind, &conds, &resolve)),
                kind,
            })
            .collect()
    }

    /// Under the root's search: whether operation `op` cannot run (module doc).
    fn op_dead(&self, reached: &Reached<'a>, op: &str, keep: bool) -> bool {
        let (plain, states) = reached;
        let Some(&owner) = self.op_owner.get(op) else {
            return false;
        };
        if !plain.contains(owner) {
            return false;
        }
        !states
            .get(owner)
            .is_some_and(|bs| bs.iter().any(|b| !self.dead_at_m(owner, op, b, keep)))
    }

    /// The indices of `effects` (one root's) unreachable from `root` under
    /// literal arguments (module doc), and per boundary group (G33) the
    /// resources whose every originating operation is: `prove` matches none of
    /// them. A resource with an operation not known is never dropped.
    ///
    /// `per_site`: the effect types whose rows are each ONE site and are all
    /// of the root's sites (`digest::SITE_ENUMERATED`, unless the site search
    /// stopped at its depth bound): such a row is judged by its own operation
    /// ([`Self::op_dead`]), not by its kind, because no other site hides
    /// behind it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn unreachable_effects(
        &self,
        root: &'a str,
        effects: &[DigestEffectResult],
        per_site: &[&str],
    ) -> (HashSet<usize>, HashMap<usize, Vec<String>>) {
        let (dead, resources, _) = self.unreachable_effects_noted(root, effects, per_site);
        (dead, resources)
    }

    /// [`Self::unreachable_effects`], and (r3 E2) per effect it drops ONLY
    /// because a publisher's loaded subscribers keep a value ([`lflow`]), the
    /// keep verdicts the root's search used: a reader states that assumption,
    /// and a world that does not make it counts the effect. A group resource
    /// is dropped only when it is dead without the assumption too.
    pub(crate) fn unreachable_effects_noted(
        &self,
        root: &'a str,
        effects: &[DigestEffectResult],
        per_site: &[&str],
    ) -> NotedUnreachable {
        // A root with no usable guard anywhere has nothing to prune.
        if self.frames.is_empty()
            && self.tables.is_empty()
            && self.value_frames.is_empty()
            && self.guards.is_empty()
            && self.flow_progs.is_empty()
        {
            return Default::default();
        }
        let Some(reached) = self.search(root, true) else {
            return Default::default();
        };
        let (dead, mut resources) = self.judge(&reached, effects, per_site, true);
        let mut keeps: Vec<u32> = reached
            .1
            .iter()
            .filter(|(r, _)| self.flow_progs.contains_key(*r))
            .flat_map(|(r, bs)| bs.iter().flat_map(|b| self.keeps_at(r, b)))
            .collect();
        keeps.sort_unstable();
        keeps.dedup();
        if keeps.is_empty() {
            return (dead, resources, HashMap::new());
        }
        // Without the publisher keeps: what is dead only through them.
        let strict = self
            .search(root, false)
            .map(|r| self.judge(&r, effects, per_site, false));
        let infos_of = |ks: &[u32]| -> Vec<KeepInfo> {
            ks.iter()
                .filter_map(|&k| self.keep_info.get(k as usize).cloned())
                .collect()
        };
        let infos = infos_of(&keeps);
        let mut assumed = HashMap::new();
        match &strict {
            Some((d2, r2)) => {
                // Per effect, the verdicts that cut its strict route.
                let tree = self
                    .tree_recording_m(
                        (root, Bindings::default()),
                        STATE_CAP,
                        &|| false,
                        None,
                        false,
                    )
                    .ok();
                for &i in dead.difference(d2) {
                    let e = &effects[i];
                    let mut ops: Vec<String> = e.evidence_operation_id.iter().cloned().collect();
                    ops.extend(e.origin_operation_ids.iter().cloned());
                    if ops.is_empty() {
                        let resource = e
                            .detail
                            .iter()
                            .find(|(k, _)| k == "resourceId")
                            .map(|(_, v)| v.as_str());
                        ops = self
                            .terminals
                            .of_kind((e.effect_type.as_str(), resource))
                            .map(|r| r.terminal.id().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    let deciding = tree
                        .as_ref()
                        .map(|t| self.deciding_keeps(t, &ops))
                        .filter(|k| !k.is_empty());
                    assumed.insert(i, deciding.map_or_else(|| infos.clone(), |k| infos_of(&k)));
                }
                for (i, list) in resources.iter_mut() {
                    list.retain(|res| r2.get(i).is_some_and(|l| l.contains(res)));
                }
            }
            None => {
                for &i in &dead {
                    assumed.insert(i, infos.clone());
                }
                resources.clear();
            }
        }
        resources.retain(|_, l| !l.is_empty());
        (dead, resources, assumed)
    }

    /// r3 E2: the keep verdicts that cut the route to one of `ops`: along the
    /// strict search's (`strict`, made without keeps) path to the first live
    /// owner, the first site the kept values make dead; of the verdicts its
    /// routine's flow used there, those without which the site is live again
    /// (all it used when no single one decides). Empty when no site on the
    /// path is cut that way (the cut came through a binding upstream).
    fn deciding_keeps(&self, strict: &LiveTree<'a>, ops: &[String]) -> Vec<u32> {
        let mut found: Option<(&State<'a>, &str)> = None;
        'outer: for o in ops {
            let Some(owner) = self.op_owner.get(o.as_str()) else {
                continue;
            };
            for &i in strict.states_of(owner) {
                let st = &strict.order[i as usize];
                if !self.dead_at_m(st.0, o, &st.1, false) {
                    found = Some((st, o.as_str()));
                    break 'outer;
                }
            }
        }
        let Some((state, op)) = found else {
            return Vec::new();
        };
        // The states from the root to `state`, each with the site it leaves by.
        let mut chain: Vec<(State<'a>, &str)> = vec![(state.clone(), op)];
        let mut cur = state.clone();
        while let Some(Some((p, (site, _)))) = strict.parent.get(&cur) {
            if let Some(s) = site.and_then(EdgeSite::guarded) {
                chain.push((p.clone(), s));
            }
            cur = p.clone();
        }
        chain.reverse();
        let mut used_all: Vec<u32> = Vec::new();
        for ((f, b), s) in &chain {
            let Some(p) = self.flow_progs.get(f) else {
                continue;
            };
            let Some(r) = self.by_stable.get(f) else {
                continue;
            };
            let resolve = |l: &crate::program::body::features::ValueLabel| {
                values::resolve_label(r, &self.tables_by_id, l)
            };
            let entry: Vec<(u32, VSet)> = p
                .key_params
                .iter()
                .filter_map(|&i| b.entry_value(i).map(|v| (i, v)))
                .collect();
            let full = p.eval(&entry, &resolve, &|_| true, None);
            used_all.extend(full.keeps.iter().copied());
            if !p.dead(&full, s) {
                continue;
            }
            let deciding: Vec<u32> = full
                .keeps
                .iter()
                .copied()
                .filter(|&k| {
                    let ev = p.eval(&entry, &resolve, &|x| x != k, None);
                    !p.dead(&ev, s)
                })
                .collect();
            return if deciding.is_empty() {
                full.keeps.clone()
            } else {
                deciding
            };
        }
        used_all.sort_unstable();
        used_all.dedup();
        used_all
    }

    /// The effects the search `reached` (made with `keep`) rules out, and per
    /// group its dead resources ([`Self::unreachable_effects`]).
    fn judge(
        &self,
        reached: &Reached<'a>,
        effects: &[DigestEffectResult],
        per_site: &[&str],
        keep: bool,
    ) -> (HashSet<usize>, HashMap<usize, Vec<String>>) {
        let mut out = HashSet::new();
        let mut dead_resources: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, e) in effects.iter().enumerate() {
            // G48: a group is one crossing into a dependency (its boundary
            // identity is that call site or operation). It cannot run when the
            // crossing itself cannot, whatever its facts are: UI, events,
            // commits as well as table writes. Its merged facts do NOT decide
            // it: the cone keeps one representative fact per kind per root, so
            // a dead representative says nothing of the kind's other origins.
            if e.boundary_group {
                // W2-0a: the crossing's own typed site (an event crossing has
                // no guard, so it is never dead here); a hand-built group with
                // no record falls back to its evidence ids.
                let site = match &e.crossing {
                    Some(c) => c.site.guarded_id(),
                    None => e
                        .evidence_callsite_id
                        .as_deref()
                        .or(e.evidence_operation_id.as_deref()),
                };
                let crossing_dead = site.is_some_and(|site| self.op_dead(reached, site, keep));
                if crossing_dead {
                    out.insert(i);
                    continue;
                }
            }
            // An effect's (or a group resource's) merged facts are the cone's
            // representatives, one per kind per root: a dead representative
            // says nothing of the kind's other origins. So a kind is judged by
            // EVERY direct fact of it in the root's reach ([`Self::kind_dead`]).
            let kind = e.effect_type.as_str();
            if e.boundary_group {
                let dead: Vec<String> = e
                    .group_resources
                    .iter()
                    .filter(|(r, _)| self.kind_dead(reached, (kind, Some(r.as_str())), keep))
                    .map(|(r, _)| r.clone())
                    .collect();
                if !dead.is_empty() {
                    dead_resources.insert(i, dead);
                }
                continue;
            }
            if per_site.contains(&kind)
                && let Some(op) = e.evidence_operation_id.as_deref()
            {
                if self.op_dead(reached, op, keep) {
                    out.insert(i);
                }
                continue;
            }
            let resource = e
                .detail
                .iter()
                .find(|(k, _)| k == "resourceId")
                .map(|(_, v)| v.as_str());
            if self.kind_dead(reached, (kind, resource), keep) {
                out.insert(i);
            }
        }
        (out, dead_resources)
    }

    /// Whether no direct fact of `kind` (effect type, resource id) can run
    /// from the root: at least one sits in a routine the root reaches with
    /// guards ignored (else the graph does not explain the effect, and nothing
    /// is concluded), and none sits in a reached state of its routine where its
    /// site can run. A routine reached only through code that cannot run has
    /// no state, so its facts count as dead.
    fn kind_dead(&self, reached: &Reached<'a>, kind: (&str, Option<&str>), keep: bool) -> bool {
        let mut any_reached = false;
        for r in self.terminals.of_kind(kind) {
            let (subject, site) = (r.owner, r.terminal.id());
            if !reached.0.contains(subject) {
                continue;
            }
            any_reached = true;
            // A fact with no site cannot be shown dead.
            if site.is_empty() {
                return false;
            }
            let live = reached
                .1
                .get(subject)
                .is_some_and(|bs| bs.iter().any(|b| !self.dead_at_m(subject, site, b, keep)));
            if live {
                return false;
            }
        }
        any_reached
    }
}
