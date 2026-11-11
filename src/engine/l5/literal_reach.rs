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
use crate::engine::l5::commit_scope::CbGraph;
use crate::engine::l5::digest::{Crossing, CrossingSite, DigestEffectResult};
use crate::engine::l5::recref_flow::{
    Atom, Atoms, RecRefFlow, RelevantGlobals, RelevantParams, TSet, eval,
};
use crate::engine::l5::snapshot::{CapabilitySnapshot, SnapshotGraphEdge};
use crate::engine::l5::terminal_facts::TerminalFacts;
use crate::engine::l5::traps::{TrapCallee, TrapKind, call_trap, is_try_function};
use crate::program::body::features::{PCFNNode, TableCase};
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[path = "literal_routes.rs"]
pub mod routes;

#[path = "literal_values.rs"]
pub(crate) mod values;

#[path = "literal_flow.rs"]
mod lflow;

#[cfg(test)]
#[path = "literal_flow_tests.rs"]
mod lflow_tests;

pub(crate) use lflow::{GuardKind, LoopKind, RouteGuard};
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
    /// r3 §7 R1: the universe whose subscribers were considered, in words
    /// ([`universe_words`]).
    pub universe: String,
}

/// r3 §7 R1: the universe a model's subscriber inventory covers, in words:
/// the primary app, with its loaded dependencies when the model holds
/// dependency code (a test universe's primary app is the test app).
pub(crate) fn universe_words(model: &Model) -> String {
    let app = model
        .primary_app
        .as_ref()
        .map_or("the workspace app", |a| a.name.as_str());
    let deps = model
        .workspace
        .routines
        .iter()
        .any(|r| r.source_anchor.source_unit_id.starts_with("dep:"));
    if deps {
        format!("{app} with its loaded dependencies")
    } else {
        format!("{app} alone")
    }
}

/// r3 §7 R1: one publisher assumption with the sites it rules out, the one
/// template prove, explain and digest print and the `assumptionRecords`
/// entry they carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssumptionRecord {
    pub keep: KeepInfo,
    /// The sites (`file:line`, 1-based) the assumption rules out, in file
    /// then line order, each once.
    pub rules_out: Vec<String>,
}

impl AssumptionRecord {
    /// The records of `(assumption, site)` pairs: one per assumption, in
    /// first-seen order, its sites sorted by file then line and distinct.
    pub(crate) fn collect<'k>(
        pairs: impl IntoIterator<Item = (&'k KeepInfo, Option<String>)>,
    ) -> Vec<AssumptionRecord> {
        let mut out: Vec<AssumptionRecord> = Vec::new();
        for (k, site) in pairs {
            let at = match out.iter().position(|r| &r.keep == k) {
                Some(at) => at,
                None => {
                    out.push(AssumptionRecord {
                        keep: k.clone(),
                        rules_out: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            if let Some(s) = site
                && !out[at].rules_out.contains(&s)
            {
                out[at].rules_out.push(s);
            }
        }
        let key = |s: &String| -> (String, u32) {
            match s.rsplit_once(':') {
                Some((f, l)) if l.parse::<u32>().is_ok() => (f.to_string(), l.parse().unwrap_or(0)),
                _ => (s.clone(), 0),
            }
        };
        for r in &mut out {
            r.rules_out.sort_by_key(key);
        }
        out
    }

    /// The sentence (r3 §7 R1): the closure assumption, then what it rules
    /// out. A site in other records too needs their assumptions as well:
    /// [`Self::sentences`] states such sites with all of them.
    #[cfg(test)]
    pub(crate) fn sentence(&self) -> String {
        Self::joint_sentence(&[&self.keep], &self.rules_out)
    }

    /// The sentences of `records` (r3 §7 R1): its sites grouped by the
    /// records that list them, one sentence per group naming every
    /// assumption the group's sites need together (a site ruled out only by
    /// two publishers' keeps is stated with both, never as if either alone
    /// ruled it out). Groups in the order their first site is met (records
    /// in order, then their sites); a record with no site stands alone.
    pub(crate) fn sentences(records: &[AssumptionRecord]) -> Vec<String> {
        let mut groups: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for (ri, r) in records.iter().enumerate() {
            if r.rules_out.is_empty() {
                groups.push((vec![ri], Vec::new()));
            }
            for site in &r.rules_out {
                if !seen.insert(site.as_str()) {
                    continue;
                }
                let with: Vec<usize> = records
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.rules_out.contains(site))
                    .map(|(j, _)| j)
                    .collect();
                match groups.iter_mut().find(|(w, s)| *w == with && !s.is_empty()) {
                    Some((_, sites)) => sites.push(site.clone()),
                    None => groups.push((with, vec![site.clone()])),
                }
            }
        }
        groups
            .iter()
            .map(|(with, sites)| {
                let keeps: Vec<&KeepInfo> = with.iter().map(|&j| &records[j].keep).collect();
                Self::joint_sentence(&keeps, sites)
            })
            .collect()
    }

    /// One sentence: the assumptions `keeps` (all needed together), then the
    /// sites they rule out.
    fn joint_sentence(keeps: &[&KeepInfo], sites: &[String]) -> String {
        let clause = |k: &KeepInfo| -> String {
            if k.subscribers.is_empty() {
                format!(
                    "assumes {} has no loaded subscriber, so {} keeps its value ({}; no \
                     additional subscribers assumed)",
                    k.publisher, k.parameter, k.universe
                )
            } else {
                format!(
                    "assumes no subscribers beyond those considered in {} ({}), none of which \
                     changes {}",
                    k.universe,
                    k.subscribers.join(", "),
                    k.parameter
                )
            }
        };
        let mut s = match keeps {
            [] => String::new(),
            [one] => clause(one),
            [first, rest @ ..]
                if first.subscribers.is_empty()
                    && rest.iter().all(|k| {
                        k.subscribers.is_empty()
                            && k.parameter == first.parameter
                            && k.universe == first.universe
                    }) =>
            {
                let names: Vec<&str> = keeps.iter().map(|k| k.publisher.as_str()).collect();
                let (last, head) = names.split_last().unwrap_or((&"", &[]));
                format!(
                    "assumes {} and {last} have no loaded subscriber, so {} keeps its value ({}; \
                     no additional subscribers assumed)",
                    head.join(", "),
                    first.parameter,
                    first.universe
                )
            }
            _ => keeps
                .iter()
                .map(|k| clause(k))
                .collect::<Vec<_>>()
                .join(", and "),
        };
        match sites {
            [] => {}
            [one] => s.push_str(&format!("; rules out {one}")),
            [first, ..] => s.push_str(&format!(
                "; rules out {} sites (first: {first})",
                sites.len()
            )),
        }
        s
    }

    /// The `assumptionRecords` entry (additive).
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "publisher": self.keep.publisher,
            "parameter": self.keep.parameter,
            "universe": self.keep.universe,
            "subscribersConsidered": self.keep.subscribers,
            "rulesOut": self.rules_out,
        })
    }
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

/// r3 §7 R5: the guards of a route that crosses into dependency code, and
/// how many routines on it had no statement tree (their guards not
/// analyzed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RouteGuardSet {
    pub guards: Vec<RouteGuard>,
    pub unanalyzed: usize,
}

/// r3 E2: what the literal reach adds to one effect row for a reader: the
/// loaded-subscriber assumptions its removal rests on, and its witness
/// route's guards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FlowNotes {
    /// The row is unreachable only because these publishers' loaded
    /// subscribers keep a value, all of them needed together (empty: not by
    /// an assumption).
    pub assumed: Vec<KeepInfo>,
    /// The guards on the row's listed witness route, outermost first
    /// (`None`: no route was replayed).
    pub guards: Option<Vec<RouteGuard>>,
    /// r3 §7 R5: a boundary group's route was replayed only up to the
    /// dependency entry (the routines beyond it, or one of them, had no
    /// statement tree, or no witness through the entry was kept): its
    /// guards beyond the entry were not analyzed, and `guards` counts the
    /// analyzed part only.
    pub beyond_unanalyzed: bool,
    /// B7/L23: an error row's traps ([`TrapVerdict`]); `None` off error
    /// rows, and when no trap is on its routes or no search decided it.
    pub trap: Option<TrapVerdict>,
    /// B7 review: the row is [`TrapVerdict::Caught`] and a checked run with
    /// no write pending within the root is in the root's reach, so the
    /// ruling may rest on it: it holds only if the root's own callers have
    /// no write pending when they call the root ([`TrapScope::clean_checked`]).
    pub trap_assumes_clean_entry: bool,
    /// B7 review round 2: as `trap_assumes_clean_entry`, and the clean run
    /// is clean only because a `Commit()` before it ends the write: the
    /// ruling also assumes the root's callers do not call it under
    /// `[CommitBehavior(Ignore)]` or `(Error)` ([`TrapScope::clean_by_commit`]).
    pub trap_assumes_normal_commit: bool,
    /// L30: the routines with `[ErrorBehavior(ErrorBehavior::Collect)]` on
    /// the row's listed route (the root first): a collectible error raised
    /// under one is collected and raised when it ends, unless the code
    /// clears it ([`crate::engine::l5::traps`]). Empty off error rows.
    pub collected_by: Vec<String>,
}

/// r4 final (c05): a route's guards counted by population: (enclosing
/// conditions, how many of them evaluated, earlier exits, how many of them
/// evaluated). The enclosing count is the route's full count, the
/// population `prove --site`'s "of M enclosing conditions" counts; every
/// reader of a route's guards counts through this one function.
pub(crate) fn route_guard_counts(g: &[RouteGuard]) -> (usize, usize, usize, usize) {
    g.iter().fold((0, 0, 0, 0), |(n, k, e, j), x| {
        let ev = usize::from(x.evaluated);
        if x.kind.encloses() {
            (n + 1, k + ev, e, j)
        } else {
            (n, k, e + 1, j + ev)
        }
    })
}

/// r3 §7 R5: the condition words of a row whose route was replayed
/// (`N guards on the route, k evaluated`, or `on the analyzed route` with
/// the boundary when the guards beyond the dependency entry were not
/// analyzed); `None` when no route was replayed, or a fully analyzed route
/// has no guard (the row's class label then says it). prove, explain and
/// digest print this form alike.
///
/// r4 final (c05): `N` counts every enclosing condition of the route's
/// sites (the population of `prove --site`'s "of M enclosing conditions",
/// loops included), never a displayed subset, and `k` counts over the same
/// guards; the earlier `if … then exit` checks the route passes are counted
/// apart, as prove's listing does not count them among its enclosing
/// conditions: `N guards, E earlier exits on the route; none evaluated`
/// (or `k guards and j exits evaluated`).
pub(crate) fn route_condition_words(notes: Option<&FlowNotes>) -> Option<String> {
    let n = notes?;
    let g = n.guards.as_ref()?;
    if g.is_empty() && !n.beyond_unanalyzed {
        return None;
    }
    let (enclosing, evaluated, exits, exits_evaluated) = route_guard_counts(g);
    let count = if enclosing == 1 {
        "1 guard".to_string()
    } else {
        format!("{enclosing} guards")
    };
    let route = if n.beyond_unanalyzed {
        "the analyzed route"
    } else {
        "the route"
    };
    // Engine limits wave 2 (compact card): with earlier exits, both counts
    // first and the evaluated ones after (`9 guards, 3 earlier exits on the
    // route; none evaluated`), so the common all-zero case is short.
    let words = if exits == 0 {
        format!("{count} on {route}, {evaluated} evaluated")
    } else {
        let exits_word = if exits == 1 {
            "1 earlier exit".to_string()
        } else {
            format!("{exits} earlier exits")
        };
        let guards_ev = |k: usize| {
            if k == 1 {
                "1 guard".to_string()
            } else {
                format!("{k} guards")
            }
        };
        let exits_ev = |k: usize| {
            if k == 1 {
                "1 exit".to_string()
            } else {
                format!("{k} exits")
            }
        };
        let ev = match (evaluated, exits_evaluated) {
            (0, 0) => "none evaluated".to_string(),
            (k, 0) => format!("{} evaluated", guards_ev(k)),
            (0, j) => format!("{} evaluated", exits_ev(j)),
            (k, j) => format!("{} and {} evaluated", guards_ev(k), exits_ev(j)),
        };
        format!("{count}, {exits_word} on {route}; {ev}")
    };
    Some(if n.beyond_unanalyzed {
        format!("{words}; guards beyond the dependency entry: not analyzed")
    } else {
        words
    })
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
///
/// TLS-22: one cache serves every worker of the literal pass. A tree is a
/// pure function of its entry state and `per_tree` (the search is first in,
/// first out over snapshot-ordered edges), so which worker builds it, and
/// whether it was evicted and rebuilt, cannot change an answer; only the
/// counters vary with timing. Searches run outside the lock.
pub(crate) struct SuffixCache<'a> {
    inner: std::sync::Mutex<SuffixInner<'a>>,
    per_tree: usize,
    budget: usize,
    /// Tests: runs after a tree lookup misses, the lock released, before the
    /// search (TLS-22 forces two workers to miss one entry state).
    #[cfg(test)]
    miss_hook: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct SuffixInner<'a> {
    trees: HashMap<State<'a>, Result<Arc<LiveTree<'a>>, Stop>>,
    order: VecDeque<State<'a>>,
    held: usize,
    /// W2-0c: per entry target, the routines reached with guards ignored (a
    /// set per distinct target routine, so few and small next to the trees).
    plain: HashMap<&'a str, Arc<HashSet<&'a str>>>,
    stats: SuffixStats,
}

/// A [`SuffixCache`]'s counters (diagnostics: no output reads them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SuffixStats {
    /// Searches run (a duplicate counts; a cancelled one too).
    pub(crate) built: usize,
    pub(crate) evicted: usize,
    pub(crate) peak_held: usize,
    /// The states the held trees hold together.
    pub(crate) held: usize,
    /// Trees a worker built while another worker's equal tree was inserted
    /// first (dropped; TLS-22).
    pub(crate) duplicate: usize,
}

impl Default for SuffixCache<'_> {
    fn default() -> Self {
        SuffixCache::with_limits(STATE_CAP, SUFFIX_CACHE_STATES)
    }
}

/// A lock that a panicked holder does not poison: what it guards is whole
/// after each insert.
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl<'a> SuffixCache<'a> {
    /// A cache whose trees hold at most `per_tree` states each and
    /// `budget` together.
    pub(crate) fn with_limits(per_tree: usize, budget: usize) -> Self {
        SuffixCache {
            inner: std::sync::Mutex::new(SuffixInner {
                trees: HashMap::new(),
                order: VecDeque::new(),
                held: 0,
                plain: HashMap::new(),
                stats: SuffixStats::default(),
            }),
            per_tree,
            budget: budget.max(per_tree),
            #[cfg(test)]
            miss_hook: std::sync::Mutex::new(None),
        }
    }

    /// The counters now. Tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stats(&self) -> SuffixStats {
        let c = lock(&self.inner);
        SuffixStats {
            held: c.held,
            ..c.stats
        }
    }

    /// Tests: set the hook [`Self::get`] runs after a miss.
    #[cfg(test)]
    pub(crate) fn set_miss_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.miss_hook) = Some(f);
    }

    /// The suffix tree from `start`, built on first use.
    fn get(
        &self,
        reach: &LiteralReach<'a>,
        start: State<'a>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<LiveTree<'a>>, Stop> {
        if let Some(t) = lock(&self.inner).trees.get(&start) {
            return t.clone();
        }
        #[cfg(test)]
        {
            // Cloned out first: the hook may block, and must not hold the lock.
            let hook = lock(&self.miss_hook).clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        let built = reach.tree_from(start.clone(), self.per_tree, cancel);
        let mut c = lock(&self.inner);
        c.stats.built += 1;
        // A cancelled search is no property of the entry: not cached.
        if matches!(built, Err(Stop::Cancelled)) {
            return Err(Stop::Cancelled);
        }
        // Another worker inserted it meanwhile: the same tree; keep theirs.
        if let Some(t) = c.trees.get(&start) {
            let t = t.clone();
            c.stats.duplicate += 1;
            return t;
        }
        let n = built.as_ref().map_or(0, |t| t.len());
        while c.held + n > self.budget {
            let Some(old) = c.order.pop_front() else {
                break;
            };
            if let Some(Ok(t)) = c.trees.remove(&old) {
                c.held -= t.len();
            }
            c.stats.evicted += 1;
        }
        let built = built.map(Arc::new);
        c.trees.insert(start.clone(), built.clone());
        c.order.push_back(start);
        c.held += n;
        c.stats.peak_held = c.stats.peak_held.max(c.held);
        built
    }

    /// The routines `from` reaches with guards ignored, built on first use
    /// (outside the lock: a whole-graph search).
    fn plain(&self, reach: &LiteralReach<'a>, from: &'a str) -> Arc<HashSet<&'a str>> {
        if let Some(p) = lock(&self.inner).plain.get(from) {
            return Arc::clone(p);
        }
        let p = Arc::new(reach.plain_reach(from));
        Arc::clone(lock(&self.inner).plain.entry(from).or_insert(p))
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

/// r3 E2: which publisher keep verdicts ([`lflow`]) a search follows: all of
/// them (the loaded-subscribers world), none (the strict world), or (r3 §7
/// R1) only the listed ones, sorted, every other publisher pass writing its
/// `var` arguments. The last one finds which keeps a ruling depends on
/// ([`LiteralReach::relied_keeps`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Keeps {
    All,
    None,
    Only(Arc<[u32]>),
}

impl Keeps {
    /// Whether keep verdict `k` is followed.
    fn follows(&self, k: u32) -> bool {
        match self {
            Keeps::All => true,
            Keeps::None => false,
            Keeps::Only(ks) => ks.binary_search(&k).is_ok(),
        }
    }
}

/// r3 §7 R1: a root's guarded search with every keep verdict followed, as a
/// graph ([`LiteralReach::keep_graph`]): its states in visit order (the
/// root's first), each one's index, each one's successors, and the keep
/// verdicts each one's flow evaluation asked about. [`LiteralReach::dead_under`]
/// replays it under fewer keeps.
struct KeepGraph<'a> {
    states: Vec<State<'a>>,
    index: HashMap<State<'a>, usize>,
    succ: Vec<Vec<usize>>,
    asked: Vec<Vec<u32>>,
}

/// [`Keeps::All`], to pass by reference.
const ALL: &Keeps = &Keeps::All;
/// [`Keeps::None`], to pass by reference.
const NONE: &Keeps = &Keeps::None;

/// r3 E2: the flow evaluation cache key: (the program, by address: the
/// programs never move once built; entry values; which publisher keeps are
/// followed). No borrowed id, so the reach stays covariant in its lifetime.
type FlowKey = (usize, Vec<(u32, VSet)>, Keeps);

/// [`LiteralReach::unreachable_effects_noted`]'s result: the dead effects,
/// per group its dead resources, and per effect dead only by an assumption
/// the keep verdicts it rests on.
/// Round-4 literal routes: and of the call sites asked about, those that
/// cannot run from the root (without relying on a publisher keep), and
/// (r4 final) the interface call sites among them whose implementation the
/// literal arguments fix on every route ([`LiteralReach::narrowed_sites`]).
pub(crate) type NotedUnreachable = (
    HashSet<usize>,
    HashMap<usize, Vec<String>>,
    HashMap<usize, Vec<KeepInfo>>,
    Vec<String>,
    Vec<String>,
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
    let universe = universe_words(model);
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
                        k.universe = universe.clone();
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
        universe: String::new(),
    })
}

/// B7/L23: the trap edge `e` is, by the one rule
/// ([`crate::engine::l5::traps`]): a synchronous call or object run whose
/// call site traps ([`call_trap`]: under asserterror, a checked run, a used
/// TryFunction), a record operation under asserterror (its triggers and
/// table events), or the dispatch of an isolated event. An interface
/// dispatch is never one (its target is not known to be a TryFunction for
/// every implementer). A call site the caller's row does not hold is no
/// trap: a trap is claimed only from facts.
fn edge_trap(
    e: &SnapshotGraphEdge,
    by_stable: &HashMap<&str, &ModelRoutine>,
    site_rows: &HashMap<&str, &ModelRoutine>,
    under_asserterror: &HashSet<&str>,
    isolated_events: &HashSet<&str>,
) -> Option<TrapKind> {
    match e {
        SnapshotGraphEdge::DirectCall {
            from,
            to,
            callsite_id,
            ..
        }
        | SnapshotGraphEdge::VariableTypedCall {
            from,
            to,
            callsite_id,
            ..
        }
        | SnapshotGraphEdge::ObjectRunResolved {
            from,
            to,
            callsite_id,
            ..
        } => {
            let callee = by_stable.get(to.as_str()).copied();
            // Only an edge that can trap is looked up (the call site scan
            // is per edge).
            let may = under_asserterror.contains(callsite_id.as_str())
                || matches!(e, SnapshotGraphEdge::ObjectRunResolved { .. })
                || callee.is_some_and(|c| {
                    is_try_function(c)
                        || (c.kind == "trigger" && c.name.eq_ignore_ascii_case("OnRun"))
                });
            if !may {
                return None;
            }
            // Engine limits B2: in the caller row (`#if` arm) that holds it.
            let cs = site_rows
                .get(callsite_id.as_str())
                .or_else(|| by_stable.get(from.as_str()))?
                .call_sites
                .iter()
                .find(|c| c.id == *callsite_id)?;
            call_trap(cs, TrapCallee::Routine(callee))
        }
        SnapshotGraphEdge::ImplicitTrigger { operation_id, .. }
        | SnapshotGraphEdge::TableEvent { operation_id, .. } => under_asserterror
            .contains(operation_id.as_str())
            .then_some(TrapKind::Asserterror),
        SnapshotGraphEdge::EventDispatch { event_id, .. } => isolated_events
            .contains(event_id.as_str())
            .then_some(TrapKind::IsolatedEvent),
        _ => None,
    }
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
    by_routine: std::sync::OnceLock<HashMap<&'a str, Vec<u32>>>,
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

/// TLS-22: the literal pass shares these between its workers, so they must
/// stay `Sync` (a `Cell`, `RefCell` or `Rc` added to them fails to compile here).
#[allow(dead_code)]
fn shared_by_the_pass_workers(r: &LiteralReach<'_>, c: &SuffixCache<'_>, t: &LiveTree<'_>) {
    fn sync<T: Sync + Send>(_: &T) {}
    sync(r);
    sync(c);
    sync(t);
}

/// The search context over one snapshot and model, shared by every root.
pub(crate) struct LiteralReach<'a> {
    out: HashMap<&'a str, Vec<OutEdge<'a>>>,
    by_stable: HashMap<&'a str, &'a ModelRoutine>,
    /// Engine limits B2 (per-arm ids): for a routine with several rows (two
    /// `#if` arms of one routine), each of its sites (call site, its
    /// operation id, operation, record operation) -> the row that holds it
    /// (the LAST holding row, class P). `by_stable` keeps the FIRST row for
    /// row-level reads; a site-level read goes through [`Self::row_at`].
    site_rows: HashMap<&'a str, &'a ModelRoutine>,
    /// Every row of a routine with several rows (stable id), in row order.
    arm_rows: HashMap<&'a str, Vec<&'a ModelRoutine>>,
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
    /// Round-4 literal routes: the implementations interface calls on a
    /// known enum value may reach ([`EnumImpls`]).
    enum_impls: EnumImpls<'a>,
    /// B7/L23: per routine (stable id), its out-edges that are traps (the
    /// edge's position in `out`, and the trap's kind), by the one rule
    /// [`crate::engine::l5::traps`] states. An error search
    /// ([`Self::untrapped_tree`]) does not cross them.
    traps: HashMap<&'a str, Vec<(u32, TrapKind)>>,
    /// L30: the routines (stable ids) that collect errors
    /// ([`crate::engine::l5::traps::collects_errors`]).
    collectors: HashSet<&'a str>,
    /// B7 review: the call sites the resolver classified as platform methods
    /// (no AL code runs behind them, so they leave no write pending).
    builtin_sites: HashSet<&'a str>,
    /// B7 review: the write facts a checked run's "is a write pending"
    /// question reads ([`PendingWrites`]); `None` when the snapshot has no
    /// checked run. (Built with the reach, not on first need: a cell would
    /// make `LiteralReach` invariant in `'a`.)
    pending: Option<PendingWrites<'a>>,
    /// B7 review round 2 (B4 x B7): the tree's `[CommitBehavior]` graph,
    /// which says whether a `Commit()` before a checked run surely ends the
    /// write ([`CbGraph::commit_surely_runs`]). `None` when no routine
    /// declares a behavior (every commit runs normally) or no checked run
    /// exists.
    commit_graph: Option<CbGraph<'a>>,
}

/// B7 review (a checked run inside an open write transaction): the write
/// facts of the whole snapshot, for "may a write be pending here".
struct PendingWrites<'a> {
    /// The operation ids of writes that are not known temporary (a
    /// parameter-dependent or unknown temp state counts as a write).
    write_sites: HashSet<&'a str>,
    /// The routines with such a write that has no site (it counts at entry).
    siteless_writers: HashSet<&'a str>,
    /// The routines that may leave a write pending when they return: a
    /// write of their own, a call no edge resolves (unknown code), a callee
    /// with no row, or a callee that may, except through a checked run (its
    /// writes are committed or rolled back by the run).
    may_write: HashSet<&'a str>,
    /// Per routine, its in-edges as (caller, position in the caller's `out`).
    rev: HashMap<&'a str, Vec<(&'a str, u32)>>,
}

/// [`LiteralReach::soft_checked_runs`]: the soft runs (caller, out-edge
/// position), whether a clean run is in reach, and whether one is clean only
/// because a `Commit()` ends the write.
type SoftRuns<'a> = (HashSet<(&'a str, u32)>, bool, bool);

/// B7 review round 2: how the "is a write pending at this checked run"
/// search reads a `Commit()` ([`LiteralReach::local_pending`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Commits {
    /// A commit ends the write before it when it runs normally on every
    /// route from the root (the B4 rule).
    End,
    /// No commit ends anything: the reading when the root's own callers
    /// may call it under `[CommitBehavior(Ignore)]` or `(Error)`.
    Never,
}

/// B7/L23: which traps an error search does not cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Untrap {
    /// Do not cross a definite trap: a used TryFunction, asserterror, and a
    /// checked run no write may be pending at ([`TrapScope::soft`]).
    Definite,
    /// Do not cross any trap: isolated events and checked runs a write may
    /// be pending at included.
    All,
}

/// B7/L23: the traps one root's error rulings read, built once per root
/// ([`LiteralReach::trap_scope`]).
pub(crate) struct TrapScope<'a> {
    /// The checked runs (caller, out-edge position) in the root's reach at
    /// which a write may be pending on some route from the root (B7
    /// review): such a run is itself a run-time error when the write is
    /// pending (Microsoft Learn, "Codeunit.Run(Integer [, var Record])
    /// Method", Transaction semantics), so it is no definite trap.
    soft: HashSet<(&'a str, u32)>,
    /// A checked run no write may be pending at (within the root) is in
    /// reach: a ruling "caught" may rest on it, which holds only if the
    /// root's own callers have no write pending when they call it.
    pub clean_checked: bool,
    /// B7 review round 2: some checked run counts as clean only because a
    /// `Commit()` before it ends the write. That commit runs normally on
    /// every route from the root as the root is entered normally
    /// ([`CbGraph::commit_surely_runs`]); a caller that calls the root under
    /// `[CommitBehavior(Ignore)]` or `(Error)` would make it commit nothing,
    /// so the ruling also assumes the callers set no such behavior.
    pub clean_by_commit: bool,
    /// The root's reach that crosses no definite trap.
    definite: OpenReach<'a>,
    /// The root's reach that crosses no trap; `None` when every trap in
    /// reach is definite.
    all: Option<OpenReach<'a>>,
}

impl TrapScope<'_> {
    /// Whether the search `untrap` does not cross the trap edge `(f, j)` of
    /// kind `k`.
    fn stops(&self, untrap: Untrap, f: &str, j: u32, k: TrapKind) -> bool {
        match untrap {
            Untrap::Definite => k.definite() && !self.soft.contains(&(f, j)),
            Untrap::All => true,
        }
    }
}

/// B7/L23: a root's routines reached with guards ignored and some traps not
/// crossed ([`LiteralReach::open_reach`]): each with the edge (caller, its
/// position among the caller's out-edges) it was first reached by.
pub(crate) struct OpenReach<'a> {
    parent: HashMap<&'a str, Option<(&'a str, u32)>>,
}

/// B7/L23: where a route is trapped: the trap's kind, the routine whose
/// edge it is (stable id), and the place of its call site (or, for an
/// isolated event, of the publisher), 0-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrapAt {
    pub kind: TrapKind,
    pub routine: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

impl TrapAt {
    /// `checked Codeunit.Run at file:line` (1-based).
    pub(crate) fn words(&self) -> String {
        match (&self.file, self.line) {
            (Some(f), Some(l)) => format!(
                "{} at {f}:{}",
                self.kind.words(),
                crate::engine::line_base::shown(l)
            ),
            (Some(f), None) => format!("{} in {f}", self.kind.words()),
            _ => self.kind.words().to_string(),
        }
    }

    /// [`Self::words`] printed next to a place in file `here`: a trap in
    /// that same file is named by its line (`… at line 234`).
    pub(crate) fn words_from(&self, here: Option<&str>) -> String {
        match (&self.file, self.line) {
            (Some(f), Some(l)) if here == Some(f.as_str()) => format!(
                "{} at line {}",
                self.kind.words(),
                crate::engine::line_base::shown(l)
            ),
            _ => self.words(),
        }
    }

    /// The condition of a [`TrapVerdict::UnlessWritePending`] ruling at this
    /// trap, in words.
    pub(crate) fn unless_pending_words(&self) -> String {
        match self.kind {
            TrapKind::CheckedRun => format!(
                "a write may be pending at the {}: a checked Codeunit.Run inside a write \
                 transaction is itself a run-time error, raised there; with no write pending it \
                 catches the error",
                self.words()
            ),
            _ => format!(
                "escapes only if a write is pending when the event is raised, or during an \
                 install or upgrade; otherwise caught by the {}",
                self.words()
            ),
        }
    }
}

/// B7/L23: how the traps on the routes from a root to an error site decide
/// it (only a completed search decides; anything else leaves the row as
/// it was).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrapVerdict {
    /// Every route crosses a definite trap: the error does not escape the
    /// root. `at` is the trap on the root's first witness route.
    Caught(TrapAt),
    /// Every route crosses a trap, and some cross only traps that fail with
    /// a write pending: an isolated event's (the error then escapes, and
    /// during an install or upgrade too; Microsoft Learn, "Isolated events
    /// in AL"), or a checked run a write may be pending at (the run is then
    /// itself a run-time error; "Codeunit.Run(Integer [, var Record])
    /// Method"). `at` is that trap on a route that crosses no definite trap.
    UnlessWritePending(TrapAt),
    /// Some route crosses no trap (the error escapes), and the root's first
    /// witness route crosses one: the listed route is not the escaping one.
    SomeRoutes(TrapAt),
}

/// Round-4 literal routes: which codeunit an enum value's implementation of
/// an interface is, read from the model's enums ([`ModelObject::enum_impls`]),
/// and the model's codeunits to match it against an interface call's target.
///
/// [`ModelObject::enum_impls`]: crate::program::model::workspace::ModelObject::enum_impls
#[derive(Default)]
struct EnumImpls<'a> {
    /// Enum name (folded) → the objects declaring its values: the enum and
    /// its extensions.
    declaring: HashMap<String, Vec<&'a crate::program::model::workspace::ModelObject>>,
    /// Codeunit object id → (number, name folded).
    codeunits: HashMap<&'a str, (i64, String)>,
    /// Model routine stable id → its object id.
    owner: HashMap<&'a str, &'a str>,
}

impl<'a> EnumImpls<'a> {
    fn new(model: &'a Model) -> Self {
        use crate::program::model::workspace::ModelObject;
        let mut declaring: HashMap<String, Vec<&'a ModelObject>> = HashMap::new();
        let mut codeunits = HashMap::new();
        for o in &model.workspace.objects {
            match o.object_type.as_str() {
                "Enum" => declaring.entry(values::fold(&o.name)).or_default().push(o),
                "EnumExtension" => {
                    if let Some(t) = &o.extends_target_name {
                        declaring.entry(values::fold(t)).or_default().push(o);
                    }
                }
                "Codeunit" => {
                    codeunits.insert(o.id.as_str(), (o.object_number, values::fold(&o.name)));
                }
                _ => {}
            }
        }
        let owner = model
            .workspace
            .routines
            .iter()
            .map(|r| (r.stable_routine_id.as_str(), r.object_id.as_str()))
            .collect();
        EnumImpls {
            declaring,
            codeunits,
            owner,
        }
    }

    /// The codeunit references implementing interface `iface` for the enum
    /// values `vals`, each resolved to at least one loaded codeunit; `None`
    /// when one value is not an enum value, is not found, or its
    /// implementation cannot be read or resolved (then every implementation
    /// stays possible).
    fn implementers(
        &self,
        iface: &str,
        vals: &[values::Val],
    ) -> Option<Vec<&'a crate::program::node_extract::ObjectRef>> {
        use crate::program::node_extract::ObjectRef;
        let mut out: Vec<&'a ObjectRef> = Vec::new();
        for v in vals {
            let values::Val::Enum(e, m) = v else {
                return None;
            };
            let objs = self.declaring.get(&**e)?;
            // The base enum's default covers an extension value that names
            // none of its own.
            let base_default = objs
                .iter()
                .find(|o| o.object_type == "Enum")
                .and_then(|o| o.enum_impls.as_ref())
                .map(|x| x.default.as_ref());
            let mut found = false;
            for o in objs {
                let Some(ei) = &o.enum_impls else {
                    // A declaring object whose values were not read (a
                    // symbol-only enum or extension) may hold the value.
                    return None;
                };
                let Some((_, pairs)) = ei.values.iter().find(|(n, _)| n.as_str() == &**m) else {
                    continue;
                };
                found = true;
                let pick = |ps: &'a Option<Vec<crate::program::model::workspace::EnumImplPair>>| {
                    ps.as_ref()
                        .map(|ps| ps.iter().find(|(i, _)| i.as_str() == iface).map(|(_, c)| c))
                };
                let c = match pick(pairs)? {
                    Some(c) => c,
                    None => match pick(&ei.default)? {
                        Some(c) => c,
                        None => match base_default {
                            Some(Some(d)) => d
                                .iter()
                                .find(|(i, _)| i.as_str() == iface)
                                .map(|(_, c)| c)?,
                            _ => return None,
                        },
                    },
                };
                let resolves = self.codeunits.values().any(|(n, name)| match c {
                    ObjectRef::Id(id) => id == n,
                    ObjectRef::Name { normalized_lc, .. } => normalized_lc == name,
                });
                if !resolves {
                    return None;
                }
                if !out.contains(&c) {
                    out.push(c);
                }
            }
            if !found {
                return None;
            }
        }
        Some(out)
    }

    /// Whether routine `to` (stable id) belongs to one of the codeunits
    /// `refs` names.
    fn is_one_of(&self, to: &str, refs: &[&crate::program::node_extract::ObjectRef]) -> bool {
        use crate::program::node_extract::ObjectRef;
        let Some((n, name)) = self.owner.get(to).and_then(|o| self.codeunits.get(o)) else {
            // A target that is no loaded codeunit routine: not ruled out.
            return true;
        };
        refs.iter().any(|c| match c {
            ObjectRef::Id(id) => id == n,
            ObjectRef::Name { normalized_lc, .. } => normalized_lc == name,
        })
    }
}

impl<'a> LiteralReach<'a> {
    pub(crate) fn new(tree: &'a CapabilitySnapshot, model: &'a Model) -> Self {
        let edges: &'a [SnapshotGraphEdge] = &tree.typed_edges;
        let facts: &'a [crate::engine::l5::snapshot::SnapshotCapabilityFact] =
            &tree.capability_facts;
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
        let mut rows_per_stable: HashMap<&str, usize> = HashMap::new();
        for r in routines {
            *rows_per_stable
                .entry(r.stable_routine_id.as_str())
                .or_default() += 1;
        }
        let mut site_rows: HashMap<&str, &ModelRoutine> = HashMap::new();
        let mut arm_rows: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
        for r in routines {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(r);
            if rows_per_stable[r.stable_routine_id.as_str()] > 1 {
                arm_rows
                    .entry(r.stable_routine_id.as_str())
                    .or_default()
                    .push(r);
                // Engine limits B2: each row (`#if` arm) owns its sites.
                for cs in &r.call_sites {
                    site_rows.insert(cs.id.as_str(), r);
                    site_rows.insert(cs.operation_id.as_str(), r);
                }
                for o in &r.operation_sites {
                    site_rows.insert(o.id.as_str(), r);
                }
                for o in &r.record_operations {
                    site_rows.insert(o.id.as_str(), r);
                }
            }
            let callees_at =
                |cs: &str| -> Vec<&str> { callees.get(cs).cloned().unwrap_or_default() };
            if let Some(f) = frame_guards_over(r, callees_at, &by_id) {
                // Engine limits B2: a routine's rows (its `#if` arms) each have
                // their own sites; its frame holds every row's sites (a shared
                // site: the LAST row's requirements) and the guard parameters
                // usable in every row.
                match frames.entry(r.stable_routine_id.as_str()) {
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(f);
                    }
                    std::collections::hash_map::Entry::Occupied(mut o) => {
                        let prev: &mut FrameGuards = o.get_mut();
                        prev.usable.retain(|i| f.usable.contains(i));
                        prev.by_site.extend(f.by_site);
                    }
                }
            }
            if let Some(t) = table_frame(r) {
                // As the frame: every row's sites, the parameters usable in
                // every row.
                match tables.entry(r.stable_routine_id.as_str()) {
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(t);
                    }
                    std::collections::hash_map::Entry::Occupied(mut o) => {
                        let prev: &mut TableFrame = o.get_mut();
                        prev.usable.retain(|i| t.usable.contains(i));
                        prev.by_site.extend(t.by_site);
                    }
                }
            }
            let passes_by_value = |cs_id: &str, arg: u32| {
                value_passes_by_value(r, cs_id, arg, &callees, &builtin_sites, &by_id)
            };
            let resolve = |l: &crate::program::body::features::ValueLabel| {
                values::resolve_label(r, &tables_by_id, l)
            };
            if let Some(v) = values::value_frame(r, passes_by_value, &resolve) {
                match value_frames.entry(r.stable_routine_id.as_str()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(v);
                    }
                    std::collections::hash_map::Entry::Occupied(mut o) => {
                        let prev: &mut values::ValueFrame = o.get_mut();
                        prev.usable.retain(|u| v.usable.contains(u));
                        prev.by_site.extend(v.by_site);
                    }
                }
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
        // B7/L23: the sites under asserterror, and the isolated events.
        let mut under_asserterror: HashSet<&str> = HashSet::new();
        for r in routines {
            for cs in &r.call_sites {
                if cs.under_asserterror == Some(true) {
                    under_asserterror.insert(cs.id.as_str());
                }
            }
            for o in &r.operation_sites {
                if o.under_asserterror == Some(true) {
                    under_asserterror.insert(o.id.as_str());
                }
            }
        }
        // Engine limits B2: an event isolated in some builds only is no trap: a
        // build that does not isolate it lets the error escape with no
        // condition, and the trap's words would say otherwise.
        let some_builds =
            crate::program::model::events::isolated_in_some_builds_only(&model.workspace.routines);
        let isolated_events: HashSet<&str> = model
            .events
            .graph
            .events
            .iter()
            .filter(|s| s.isolated == Some(true) && !some_builds.contains(&s.id))
            .map(|s| s.id.as_str())
            .collect();
        let mut out: HashMap<&str, Vec<OutEdge>> = HashMap::new();
        let mut traps: HashMap<&str, Vec<(u32, TrapKind)>> = HashMap::new();
        for e in edges {
            if let Some((from, to, site, binds)) = edge_parts(e) {
                let list = out.entry(from).or_default();
                if let Some(k) = edge_trap(
                    e,
                    &by_stable,
                    &site_rows,
                    &under_asserterror,
                    &isolated_events,
                ) {
                    let j = u32::try_from(list.len()).expect("an edge position fits u32");
                    traps.entry(from).or_default().push((j, k));
                }
                list.push((to, site, binds, e.edge_kind()));
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
        let mut reach = LiteralReach {
            out,
            by_stable,
            site_rows,
            arm_rows,
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
            enum_impls: EnumImpls::new(model),
            traps,
            collectors: routines
                .iter()
                .filter(|r| crate::engine::l5::traps::collects_errors(r))
                .map(|r| r.stable_routine_id.as_str())
                .collect(),
            builtin_sites,
            pending: None,
            commit_graph: None,
        };
        // B7 review: the write facts, only when some checked run needs them.
        if reach
            .traps
            .values()
            .flatten()
            .any(|&(_, k)| k == TrapKind::CheckedRun)
        {
            reach.pending = Some(reach.build_pending_writes());
            reach.commit_graph = CbGraph::of_tree(tree);
        }
        reach
    }

    /// Whether the out-edge of `f` to `to` (of edge kind `kind`) leaving at
    /// guarded site `site` cannot be taken under `b`: the site cannot run
    /// ([`Self::dead_at_m`]), or (round-4 literal routes) it is an interface
    /// dispatch whose receiver holds known enum values none of which selects
    /// `to`'s codeunit.
    fn edge_dead(
        &self,
        f: &str,
        site: &str,
        to: &str,
        kind: &str,
        b: &Bindings,
        keep: &Keeps,
    ) -> bool {
        self.dead_at_m(f, site, b, keep)
            || (kind == "interface-dispatch" && self.dispatch_excluded(f, site, to, b, keep))
    }

    /// Round-4 literal routes: whether interface call `site` of `f` cannot
    /// reach implementer routine `to` under `b`: the receiver is a local
    /// assigned from enum values the flow knows ([`lflow`]), each value's
    /// implementation of the interface resolves to a loaded codeunit, and
    /// `to` is in none of them. Anything unknown keeps the edge.
    fn dispatch_excluded(&self, f: &str, site: &str, to: &str, b: &Bindings, keep: &Keeps) -> bool {
        let Some(p) = self.flow_progs.get(f) else {
            return false;
        };
        let Some(ev) = self.flow_eval(f, b, keep) else {
            return false;
        };
        let Some((iface, vals)) = p.receiver_at(&ev, site) else {
            return false;
        };
        match self.enum_impls.implementers(iface, vals) {
            Some(refs) => !self.enum_impls.is_one_of(to, &refs),
            None => false,
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
        self.dead_at_m(routine, site, b, ALL)
    }

    /// [`Self::dead_at`]; `keep` says which publisher keeps the flow follows
    /// (r3 E2, [`lflow`]).
    fn dead_at_m(&self, routine: &str, site: &str, b: &Bindings, keep: &Keeps) -> bool {
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
    fn flow_eval(&self, routine: &str, b: &Bindings, keep: &Keeps) -> Option<Arc<lflow::FlowEval>> {
        let p = self.flow_progs.get(routine)?;
        let entry: Vec<(u32, VSet)> = p
            .key_params
            .iter()
            .filter_map(|&i| b.entry_value(i).map(|v| (i, v)))
            .collect();
        let key: FlowKey = (p as *const lflow::FlowProgram as usize, entry, keep.clone());
        if let Some(ev) = self.flow_cache.lock().expect("flow cache").get(&key) {
            return Some(Arc::clone(ev));
        }
        let r = self.by_stable.get(routine)?;
        let resolve = |l: &crate::program::body::features::ValueLabel| {
            values::resolve_label(r, &self.tables_by_id, l)
        };
        // The verdicts the evaluation asks about: an evaluation answers the
        // same under any keep choice that answers these the same way
        // ([`Self::dead_under`] reuses the full search on that).
        let asked = std::cell::RefCell::new(Vec::new());
        let mut ev = p.eval(
            &key.1,
            &resolve,
            &|k| {
                asked.borrow_mut().push(k);
                keep.follows(k)
            },
            None,
        );
        let mut asked = asked.into_inner();
        asked.sort_unstable();
        asked.dedup();
        ev.consulted = asked;
        let ev = Arc::new(ev);
        let mut cache = self.flow_cache.lock().expect("flow cache");
        if cache.len() >= FLOW_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&ev));
        Some(ev)
    }

    /// r3 E2: the keep verdicts the flow used at `routine` under `b`.
    fn keeps_at(&self, routine: &str, b: &Bindings) -> Vec<u32> {
        self.flow_eval(routine, b, ALL)
            .map_or_else(Vec::new, |ev| ev.keeps.clone())
    }

    /// The callee's bindings across a call at `site` of `caller` with `b`.
    fn bind(&self, caller: &str, site: &str, callee: &str, b: &Bindings) -> Bindings {
        self.bind_m(caller, site, callee, b, ALL)
    }

    /// [`Self::bind`]; `keep` as in [`Self::dead_at_m`].
    fn bind_m(
        &self,
        caller: &str,
        site: &str,
        callee: &str,
        b: &Bindings,
        keep: &Keeps,
    ) -> Bindings {
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
        let Some((r, cs)) = self
            .row_at(caller, site)
            .and_then(|r| r.call_sites.iter().find(|c| c.id == site).map(|cs| (r, cs)))
        else {
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
                .or_else(|| forwarded(*j).and_then(|i| b.entry_value(i)))
                // Round-4 literal routes: a Variant parameter holds the
                // static type of the caller's variable passed to it (the
                // implicit `Rec` included).
                .or_else(|| {
                    if *d != lflow::FDom::Variant {
                        return None;
                    }
                    let info = cs.argument_infos.get(*j as usize)?;
                    if !matches!(info.kind.as_str(), "identifier" | "quoted_identifier") {
                        return None;
                    }
                    let name = values::fold(&info.text);
                    let ty = match r.variables.iter().find(|v| values::fold(&v.name) == name) {
                        Some(v) => values::variant_type_of(&v.declared_type)?,
                        None => r
                            .record_variables
                            .iter()
                            .any(|v| v.is_implicit_rec && values::fold(&v.name) == name)
                            .then(|| values::Val::Type(Arc::from("record")))?,
                    };
                    Some(VSet::from([ty]))
                });
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
    fn search(&self, root: &'a str, keep: &Keeps) -> Option<Reached<'a>> {
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
            for (to, nb) in self.successors(f, &b, keep) {
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

    /// The states the guarded search ([`Self::search`]) steps to from `f`
    /// entered with `b`, following the keep verdicts `keep` follows: one per
    /// out-edge that can run, in edge order.
    fn successors(&self, f: &'a str, b: &Bindings, keep: &Keeps) -> Vec<State<'a>> {
        let mut out = Vec::new();
        for &(to, site, binds, kind) in self.out.get(f).into_iter().flatten() {
            let site = site.and_then(EdgeSite::guarded);
            if let Some(s) = site
                && self.edge_dead(f, s, to, kind, b, keep)
            {
                continue;
            }
            let nb = match (binds, site) {
                (true, Some(s)) => self.bind_m(f, s, to, b, keep),
                _ => Bindings::default(),
            };
            out.push((to, nb));
        }
        out
    }

    /// r3 §7 R1: the guarded search from `root` with every keep verdict
    /// followed ([`Self::search`]), kept as a graph for
    /// [`Self::dead_under`]. `None` when it hits the cap.
    fn keep_graph(&self, root: &'a str) -> Option<KeepGraph<'a>> {
        let start: State<'a> = (root, Bindings::default());
        let mut g = KeepGraph {
            states: vec![start.clone()],
            index: HashMap::from([(start, 0)]),
            succ: Vec::new(),
            asked: Vec::new(),
        };
        let mut i = 0;
        while i < g.states.len() {
            let (f, b) = g.states[i].clone();
            g.asked.push(
                self.flow_eval(f, &b, ALL)
                    .map_or_else(Vec::new, |ev| ev.consulted.clone()),
            );
            let mut next = Vec::new();
            for s in self.successors(f, &b, ALL) {
                let j = match g.index.get(&s) {
                    Some(&j) => j,
                    None => {
                        let j = g.states.len();
                        g.index.insert(s.clone(), j);
                        g.states.push(s);
                        if g.states.len() > STATE_CAP {
                            return None;
                        }
                        j
                    }
                };
                next.push(j);
            }
            g.succ.push(next);
            i += 1;
        }
        Some(g)
    }

    /// r3 §7 R1: of `rows` (rows the search with every keep followed rules
    /// out), those the search following only `keep`'s verdicts rules out
    /// too, exactly as [`Self::search`] and [`Self::judge`] under `keep`
    /// would say (`None`: that search hits the cap), without running it.
    /// `shell` holds the search's routines reached with guards ignored
    /// (they do not depend on the keeps); its states are filled for the
    /// judging and left empty.
    ///
    /// A state's flow evaluation depends on the keep choice only through
    /// the verdicts it asks about ([`lflow::FlowEval::consulted`]), and
    /// everything the search and the judge read of a state under a keep
    /// choice comes from that one evaluation ([`Self::flow_eval`]). So a
    /// state of `g` whose evaluation `keep` answers the same way as under
    /// every keep (every verdict it asked about followed) has the same
    /// successors and the same rulings: its successors are read from `g`.
    /// Only the other states, and the states not in `g` they lead to, are
    /// stepped again. Which states are reached does not depend on the order
    /// they are met, so the cap is hit exactly when the search hits it.
    /// A row of `rows` stays dead unless a reached state that is stepped
    /// again lets it run (every other reached state rules it out as in
    /// `g`), so only those states, and only `rows`, are judged
    /// ([`Self::judge_row`], the judge's own ruling of one row).
    fn dead_under(
        &self,
        g: &KeepGraph<'a>,
        shell: &mut Reached<'a>,
        rows: &[usize],
        keep: &Keeps,
        effects: &[DigestEffectResult],
        per_site: &[&str],
    ) -> Option<HashSet<usize>> {
        let same = |i: usize| g.asked[i].iter().all(|&k| keep.follows(k));
        let mut seen = vec![false; g.states.len()];
        seen[0] = true;
        let mut count = 1usize;
        let mut fresh: HashSet<State<'a>> = HashSet::new();
        let mut q_old: VecDeque<usize> = VecDeque::from([0]);
        let mut q_new: VecDeque<State<'a>> = VecDeque::new();
        let mut stepped: HashMap<&'a str, Vec<Bindings>> = HashMap::new();
        loop {
            let (f, b) = if let Some(i) = q_old.pop_front() {
                if same(i) {
                    for &j in &g.succ[i] {
                        if !seen[j] {
                            seen[j] = true;
                            count += 1;
                            if count > STATE_CAP {
                                return None;
                            }
                            q_old.push_back(j);
                        }
                    }
                    continue;
                }
                g.states[i].clone()
            } else if let Some(s) = q_new.pop_front() {
                s
            } else {
                break;
            };
            for s in self.successors(f, &b, keep) {
                match g.index.get(&s) {
                    Some(&j) => {
                        if !seen[j] {
                            seen[j] = true;
                            count += 1;
                            if count > STATE_CAP {
                                return None;
                            }
                            q_old.push_back(j);
                        }
                    }
                    None => {
                        if fresh.insert(s.clone()) {
                            count += 1;
                            if count > STATE_CAP {
                                return None;
                            }
                            q_new.push_back(s);
                        }
                    }
                }
            }
            stepped.entry(f).or_default().push(b);
        }
        shell.1 = stepped;
        let out = rows
            .iter()
            .copied()
            .filter(|&i| self.judge_row(shell, &effects[i], per_site, keep, None))
            .collect();
        shell.1.clear();
        Some(out)
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
        self.tree_recording_m(start, cap, cancel, rec, ALL, None)
    }

    /// [`Self::tree_recording`]; `keep` as in [`Self::dead_at_m`]; `untrap`:
    /// the trap edges it does not cross (B7/L23).
    fn tree_recording_m(
        &self,
        start: State<'a>,
        cap: usize,
        cancel: &dyn Fn() -> bool,
        mut rec: Option<&mut routes::Recorder<'a>>,
        keep: &Keeps,
        untrap: Option<&TrapScope<'a>>,
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
            let traps = match untrap {
                None => &[][..],
                Some(_) => self.traps.get(f).map_or(&[][..], Vec::as_slice),
            };
            for (j, &(to, edge_site, binds, kind)) in
                self.out.get(f).into_iter().flatten().enumerate()
            {
                if let Some(scope) = untrap
                    && traps
                        .iter()
                        .any(|&(t, k)| t as usize == j && scope.stops(Untrap::Definite, f, t, k))
                {
                    continue;
                }
                let site = edge_site.and_then(EdgeSite::guarded);
                if let Some(s) = site
                    && self.edge_dead(f, s, to, kind, &b, keep)
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
            by_routine: std::sync::OnceLock::new(),
        })
    }

    /// Over `tree`, the first state (in visit order) of a routine owning one
    /// of `ops` where that operation can run, with that operation.
    fn first_live_owner<'t>(
        &self,
        tree: &'t LiveTree<'a>,
        ops: &'t [String],
    ) -> Option<(&'t State<'a>, &'t str)> {
        self.first_live_owner_m(tree, ops, ALL)
    }

    /// [`Self::first_live_owner`]; `keep` as in [`Self::dead_at_m`].
    fn first_live_owner_m<'t>(
        &self,
        tree: &'t LiveTree<'a>,
        ops: &'t [String],
        keep: &Keeps,
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
                if !self.dead_at_m(f, o, b, keep) {
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

    /// B7/L23: the guarded search from `root` that crosses no definite trap
    /// of `scope`, following no publisher keep verdict: it finds a route
    /// that escapes every definite trap, for printing. `None` when it hits
    /// the cap, or for a root the model has no row for.
    pub(crate) fn untrapped_tree(&self, root: &str, scope: &TrapScope<'a>) -> Option<LiveTree<'a>> {
        let (&root, _) = self.by_stable.get_key_value(root)?;
        self.tree_recording_m(
            (root, Bindings::default()),
            STATE_CAP,
            &|| false,
            None,
            NONE,
            Some(scope),
        )
        .ok()
    }

    /// B7/L23: whether a trap of a definite kind, an isolated event's, and
    /// a checked run leave a routine `tree` reaches.
    fn traps_reached(&self, tree: &LiveTree<'a>) -> (bool, bool, bool) {
        let (mut definite, mut isolated, mut checked) = (false, false, false);
        for (f, list) in &self.traps {
            if tree.states_of(f).is_empty() {
                continue;
            }
            for &(_, k) in list {
                if k.definite() {
                    definite = true;
                } else {
                    isolated = true;
                }
                checked |= k == TrapKind::CheckedRun;
            }
        }
        (definite, isolated, checked)
    }

    /// B7/L23: the traps root's error rulings read ([`TrapScope`]), from its
    /// ordinary search `tree`. `None` when no trap is in its reach (no
    /// ruling to make), or for a root the model has no row for.
    pub(crate) fn trap_scope(&self, root: &str, tree: &LiveTree<'a>) -> Option<TrapScope<'a>> {
        let (definite, isolated, checked) = self.traps_reached(tree);
        if !definite && !isolated {
            return None;
        }
        let (soft, clean_checked, clean_by_commit) = if checked {
            self.soft_checked_runs(root)?
        } else {
            (HashSet::new(), false, false)
        };
        let mut scope = TrapScope {
            soft,
            clean_checked,
            clean_by_commit,
            definite: OpenReach {
                parent: HashMap::new(),
            },
            all: None,
        };
        let definite = self.open_reach(root, &|f, j, k| scope.stops(Untrap::Definite, f, j, k))?;
        scope.definite = definite;
        if isolated || !scope.soft.is_empty() {
            scope.all = Some(self.open_reach(root, &|_, _, _| true)?);
        }
        Some(scope)
    }

    /// Whether out-edge `j` of `f` is a checked run.
    fn is_checked_run_edge(&self, f: &str, j: u32) -> bool {
        self.traps
            .get(f)
            .is_some_and(|l| l.iter().any(|&(t, k)| t == j && k == TrapKind::CheckedRun))
    }

    /// B7 review: the snapshot's write facts ([`PendingWrites`]).
    fn build_pending_writes(&self) -> PendingWrites<'a> {
        use crate::engine::l5::terminal_facts::{TerminalSite, TerminalTemp};
        {
            let mut write_sites = HashSet::new();
            let mut siteless_writers = HashSet::new();
            let mut seeds: HashSet<&'a str> = HashSet::new();
            for rec in self.terminals.records() {
                if !matches!(rec.effect_type, "DB_INSERT" | "DB_MODIFY" | "DB_DELETE")
                    || matches!(rec.temp, TerminalTemp::Known(true))
                {
                    continue;
                }
                seeds.insert(rec.owner);
                match rec.terminal {
                    TerminalSite::None => {
                        siteless_writers.insert(rec.owner);
                    }
                    t => {
                        write_sites.insert(t.id());
                    }
                }
            }
            let mut rev: HashMap<&'a str, Vec<(&'a str, u32)>> = HashMap::new();
            for (&f, list) in &self.out {
                for (j, &(to, _, _, _)) in list.iter().enumerate() {
                    let j = u32::try_from(j).expect("an edge position fits u32");
                    rev.entry(to).or_default().push((f, j));
                    // A callee with no row: its code is not known.
                    if !self.by_stable.contains_key(to) {
                        seeds.insert(to);
                    }
                }
            }
            // A call no edge resolves (not a platform method): unknown code.
            for (&id, r) in &self.by_stable {
                let sited: HashSet<&str> = self
                    .out
                    .get(id)
                    .into_iter()
                    .flatten()
                    .filter_map(|&(_, s, _, _)| s.and_then(EdgeSite::guarded))
                    .collect();
                if r.call_sites.iter().any(|c| {
                    !sited.contains(c.id.as_str()) && !self.builtin_sites.contains(c.id.as_str())
                }) {
                    seeds.insert(id);
                }
            }
            let mut may_write = seeds.clone();
            let mut q: VecDeque<&'a str> = seeds.into_iter().collect();
            while let Some(x) = q.pop_front() {
                for &(p, j) in rev.get(x).map_or(&[][..], Vec::as_slice) {
                    // A checked run's writes end with it (committed, or
                    // rolled back on an error).
                    if !self.is_checked_run_edge(p, j) && may_write.insert(p) {
                        q.push_back(p);
                    }
                }
            }
            PendingWrites {
                write_sites,
                siteless_writers,
                may_write,
                rev,
            }
        }
    }

    /// B7 review: whether site `s` of `f` may leave a write pending when it
    /// has run: a write that is not known temporary, a call into code that
    /// may ([`PendingWrites::may_write`]), or a call no edge resolves.
    fn site_may_write(&self, f: &'a str, s: &str, w: &PendingWrites<'a>) -> bool {
        if w.write_sites.contains(s) {
            return true;
        }
        let mut has_edge = false;
        for (j, &(to, site, _, _)) in self.out.get(f).into_iter().flatten().enumerate() {
            if site.and_then(EdgeSite::guarded) != Some(s) {
                continue;
            }
            has_edge = true;
            let j = u32::try_from(j).expect("an edge position fits u32");
            if !self.is_checked_run_edge(f, j) && w.may_write.contains(to) {
                return true;
            }
        }
        !has_edge
            && !self.builtin_sites.contains(s)
            && self
                .row_at(f, s)
                .is_some_and(|r| r.call_sites.iter().any(|c| c.id == s))
    }

    /// B7 review: for out-edge `j` of `f`, whether a write `f` itself makes
    /// (or a callee of its earlier sites leaves) may be pending when the
    /// edge's site runs, and whether a `Commit()` on the way to the site
    /// ended the transaction `f` was entered in. The order is read off `f`'s
    /// control-flow tree by the pending-writes walk
    /// ([`crate::engine::l5::pending_writes::sites_before`]); an edge with no
    /// site (an event dispatch) counts every site of `f`.
    ///
    /// B7 review round 2 (B4 x B7): a `Commit()` of `f` ends the write only
    /// when `commits` is [`Commits::End`] and it runs normally on every
    /// route from `root` ([`CbGraph::commit_surely_runs`], the B4 rule).
    /// Microsoft Learn, "CommitBehavior attribute": under `Ignore` "this
    /// commit is ignored" (the write stays pending), under `Error` "this
    /// commit causes an error" (it commits nothing).
    fn local_pending(
        &self,
        root: &str,
        f: &'a str,
        j: u32,
        w: &PendingWrites<'a>,
        commits: Commits,
        memo: &mut HashMap<(&'a str, u32), (bool, bool)>,
    ) -> (bool, bool) {
        if let Some(&v) = memo.get(&(f, j)) {
            return v;
        }
        let site = self.out[f][j as usize].1.and_then(EdgeSite::guarded);
        // Engine limits B2: a site is read in the row (`#if` arm) that holds
        // it; an edge with no site reads every row's whole body.
        let rows: Vec<&ModelRoutine> = match site {
            Some(s) => self.row_at(f, s).into_iter().collect(),
            None => self
                .arm_rows
                .get(f)
                .cloned()
                .unwrap_or_else(|| self.by_stable.get(f).copied().into_iter().collect()),
        };
        let v = if rows.is_empty() {
            (true, false)
        } else {
            let mut dirty = w.siteless_writers.contains(f);
            let mut committed = true;
            for row in rows {
                // B7 round 2: a `Commit()` of this row ends the write only
                // when it runs normally on every route from `root`.
                let commits_end = commits == Commits::End
                    && (!row.operation_sites.iter().any(|o| o.kind == "commit")
                        || self
                            .commit_graph
                            .as_ref()
                            .is_none_or(|g| g.commit_surely_runs(root, f)));
                let before = match site {
                    Some(s) => crate::engine::l5::pending_writes::sites_before_where(
                        row,
                        &[s],
                        false,
                        commits_end,
                    ),
                    None => crate::engine::l5::pending_writes::sites_before(row, &[], true),
                };
                dirty |= before.sites.iter().any(|s| self.site_may_write(f, s, w));
                committed &= before.committed;
            }
            (dirty, committed)
        };
        memo.insert((f, j), v);
        v
    }

    /// B7 review (Microsoft Learn, "Codeunit.Run(Integer [, var Record])
    /// Method", Transaction semantics: "If you're already in a transaction
    /// you must commit first before calling Codeunit.Run"): the checked runs
    /// in `root`'s reach at which a write may be pending on some route from
    /// `root`, and whether a checked run at which none may is in reach.
    /// The root's own entry is taken as clean (the answer states that as an
    /// assumption). A routine is entered with a write pending when some
    /// caller in reach calls it, not through a checked run, with a write
    /// pending at that call: a write of its own before the call (or a callee
    /// of an earlier site that may leave one), or one it was entered with
    /// and no `Commit()` on the way ended (a least fixpoint over the reach,
    /// so recursion is followed). `None` for a root the model has no row
    /// for.
    ///
    /// Also returns whether some checked run counts as clean only because a
    /// `Commit()` before it ends the write ([`TrapScope::clean_by_commit`]):
    /// the search is run a second time with no commit ending anything.
    fn soft_checked_runs(&self, root: &str) -> Option<SoftRuns<'a>> {
        let plain = self.open_reach(root, &|_, _, _| false)?;
        let checked: Vec<(&'a str, u32)> = plain
            .parent
            .keys()
            .flat_map(|&f| {
                self.traps
                    .get(f)
                    .into_iter()
                    .flatten()
                    .filter(|&&(_, k)| k == TrapKind::CheckedRun)
                    .map(move |&(j, _)| (f, j))
            })
            .collect();
        if checked.is_empty() {
            return Some((HashSet::new(), false, false));
        }
        let w = self
            .pending
            .as_ref()
            .expect("the write facts are built whenever a checked run exists");
        let none: &[(&'a str, u32)] = &[];
        // The routines in reach that reach a checked run's caller.
        let mut anc: HashSet<&'a str> = checked.iter().map(|&(f, _)| f).collect();
        let mut q: VecDeque<&'a str> = anc.iter().copied().collect();
        while let Some(x) = q.pop_front() {
            for &(p, _) in w.rev.get(x).map_or(none, Vec::as_slice) {
                if plain.parent.contains_key(p) && anc.insert(p) {
                    q.push_back(p);
                }
            }
        }
        // The soft runs under one reading of `Commit()`, and whether some
        // commit ended a write on the way to a site.
        let soft_under = |commits: Commits| {
            let mut memo: HashMap<(&'a str, u32), (bool, bool)> = HashMap::new();
            let mut pending_at = |p: &'a str, j: u32, dirty: &HashSet<&'a str>| {
                let (local, committed) = self.local_pending(root, p, j, w, commits, &mut memo);
                local || (dirty.contains(p) && !committed)
            };
            // Entered with a write pending (least fixpoint).
            let mut dirty: HashSet<&'a str> = HashSet::new();
            loop {
                let mut changed = false;
                for &x in &anc {
                    if dirty.contains(x) {
                        continue;
                    }
                    let entered_dirty =
                        w.rev
                            .get(x)
                            .map_or(none, Vec::as_slice)
                            .iter()
                            .any(|&(p, j)| {
                                anc.contains(p)
                                    && !self.is_checked_run_edge(p, j)
                                    && pending_at(p, j, &dirty)
                            });
                    if entered_dirty {
                        dirty.insert(x);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let soft: HashSet<(&'a str, u32)> = checked
                .iter()
                .copied()
                .filter(|&(f, j)| pending_at(f, j, &dirty))
                .collect();
            let cleared = memo.values().any(|&(_, committed)| committed);
            (soft, cleared)
        };
        let (soft, cleared) = soft_under(Commits::End);
        let clean = soft.len() < checked.len();
        // A run clean only because a commit ended the write: with no
        // commit ending anything, more runs are soft (a superset).
        let by_commit = clean && cleared && soft_under(Commits::Never).0.len() > soft.len();
        Some((soft, clean, by_commit))
    }

    /// L30: whether a routine that collects errors is in `tree`'s reach.
    pub(crate) fn collectors_reached(&self, tree: &LiveTree<'a>) -> bool {
        self.collectors
            .iter()
            .any(|c| !tree.states_of(c).is_empty())
    }

    /// L30: the routines that collect errors on the route from `root`
    /// through `steps` (the root first), each once, in route order, as
    /// `Name (file:line)` (1-based).
    pub(crate) fn collectors_on(&self, root: &str, steps: &[Step<'a>]) -> Vec<String> {
        let mut seen: Vec<&str> = Vec::new();
        let on = std::iter::once(root).chain(steps.iter().map(|s| s.to));
        for r in on {
            if self.collectors.contains(r) && !seen.contains(&r) {
                seen.push(r);
            }
        }
        seen.into_iter()
            .map(|r| match self.by_stable.get(r) {
                Some(m) => format!(
                    "{} ({}:{})",
                    m.name,
                    m.source_anchor.source_unit_id,
                    crate::engine::line_base::shown(m.source_anchor.start_line)
                ),
                None => r.to_string(),
            })
            .collect()
    }

    /// B7/L23: the first edge of `steps` that is a trap `untrap` stops
    /// under `scope`.
    fn first_trap(
        &self,
        steps: &[Step<'a>],
        untrap: Untrap,
        scope: &TrapScope<'a>,
    ) -> Option<TrapAt> {
        for s in steps {
            let (Some(list), Some(out)) = (self.traps.get(s.from), self.out.get(s.from)) else {
                continue;
            };
            for &(j, k) in list {
                let (to, site, _, kind) = out[j as usize];
                if scope.stops(untrap, s.from, j, k)
                    && to == s.to
                    && site == s.site
                    && kind == s.kind
                {
                    return Some(self.trap_at(s, k));
                }
            }
        }
        None
    }

    /// The place of trap `k` on step `s`: its call site or operation in the
    /// caller, or (an event) the publisher.
    fn trap_at(&self, s: &Step<'a>, k: TrapKind) -> TrapAt {
        // Engine limits B2: the caller row (`#if` arm) that holds the site.
        let r = match s.site {
            Some(EdgeSite::Guarded(id)) => self.row_at(s.from, id),
            _ => self.by_stable.get(s.from).copied(),
        };
        let anchor = match s.site {
            Some(EdgeSite::Guarded(id)) => r.and_then(|r| {
                r.call_sites
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| &c.source_anchor)
                    .or_else(|| {
                        r.operation_sites
                            .iter()
                            .find(|o| o.id == id)
                            .map(|o| &o.source_anchor)
                    })
            }),
            _ => r.map(|r| &r.source_anchor),
        };
        TrapAt {
            kind: k,
            routine: s.from.to_string(),
            file: anchor.map(|a| a.source_unit_id.clone()),
            line: anchor.map(|a| a.start_line),
        }
    }

    /// B7/L23: the routines `root` reaches over the snapshot's edges with
    /// guards IGNORED and without crossing a trap edge `stop` names (caller,
    /// position, kind), each with the edge it was first reached by. Guards
    /// ignored, it reaches at least every routine any guarded search that
    /// crosses no such trap reaches, so a routine it does not reach is
    /// reached only through a trap: the ruling "caught" is sound under any
    /// literal arguments and any keep verdict. (A guarded search here cost
    /// prove about x1.25 CPU on CDO; this one is a plain walk over the
    /// edges.) `None` for a root the model has no row for.
    fn open_reach(
        &self,
        root: &str,
        stop: &dyn Fn(&str, u32, TrapKind) -> bool,
    ) -> Option<OpenReach<'a>> {
        let (&root, _) = self.by_stable.get_key_value(root)?;
        let mut parent: HashMap<&'a str, Option<(&'a str, u32)>> = HashMap::from([(root, None)]);
        let mut q = VecDeque::from([root]);
        while let Some(f) = q.pop_front() {
            let traps = self.traps.get(f).map_or(&[][..], Vec::as_slice);
            for (j, &(to, _, _, _)) in self.out.get(f).into_iter().flatten().enumerate() {
                if traps.iter().any(|&(t, k)| t as usize == j && stop(f, t, k)) {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(v) = parent.entry(to) {
                    v.insert(Some((
                        f,
                        u32::try_from(j).expect("an edge position fits u32"),
                    )));
                    q.push_back(to);
                }
            }
        }
        Some(OpenReach { parent })
    }

    /// The edges [`Self::open_reach`] first reached `to` by, from its root.
    fn open_steps(&self, r: &OpenReach<'a>, to: &'a str) -> Vec<Step<'a>> {
        let mut steps = Vec::new();
        let mut cur = to;
        while let Some(Some((from, j))) = r.parent.get(cur) {
            let (t, site, _, kind) = self.out[from][*j as usize];
            steps.push(Step {
                from,
                to: t,
                site,
                kind,
            });
            cur = from;
        }
        steps.reverse();
        steps
    }

    /// B7/L23: how the traps decide an error site `ops` (its operation and
    /// call-site ids) from the root of `full` (the ordinary search), under
    /// the root's `scope` ([`Self::trap_scope`]). The site is caught when no
    /// routine owning one of `ops` is in the walk that crosses no definite
    /// trap. Every op must have a known owner, or nothing is ruled (a row
    /// may sit where the walks cannot see). `None`: the site has no live
    /// route in `full` (nothing to rule), an owner is unknown, or no route
    /// crosses a trap the ruling can name. A site whose only trap-free route
    /// the literal arguments rule out stays escaping (the walks ignore
    /// guards).
    pub(crate) fn trap_verdict(
        &self,
        full: &LiveTree<'a>,
        scope: &TrapScope<'a>,
        ops: &[String],
    ) -> Option<TrapVerdict> {
        let (steps, _) = self.tree_witness(full, ops)?;
        let owners: Vec<&'a str> = ops
            .iter()
            .map(|o| self.op_owner.get(o.as_str()).copied())
            .collect::<Option<_>>()?;
        if owners.is_empty() {
            return None;
        }
        let definite = &scope.definite;
        let Some(&open) = owners.iter().find(|o| definite.parent.contains_key(*o)) else {
            return self
                .first_trap(&steps, Untrap::Definite, scope)
                .map(TrapVerdict::Caught);
        };
        if let Some(all) = &scope.all
            && owners.iter().all(|o| !all.parent.contains_key(o))
        {
            return self
                .first_trap(&self.open_steps(definite, open), Untrap::All, scope)
                .map(TrapVerdict::UnlessWritePending);
        }
        self.first_trap(&steps, Untrap::Definite, scope)
            .map(TrapVerdict::SomeRoutes)
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

    /// The row of routine `stable` that holds the site `site` (a call site,
    /// its operation id, an operation or a record operation): for a routine
    /// declared in two `#if` arms, the arm whose site it is (engine limits B2,
    /// per-arm ids); otherwise [`Self::routine`].
    pub(crate) fn row_at(&self, stable: &str, site: &str) -> Option<&'a ModelRoutine> {
        self.site_rows
            .get(site)
            .filter(|r| r.stable_routine_id == stable)
            .copied()
            .or_else(|| self.routine(stable))
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
        cache: &SuffixCache<'a>,
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
                    && self.edge_dead(f, s, to, kind, b, ALL)
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
    /// that type with no resource (or, ENG-09, a checked run's commit: its
    /// `RUN_COMMIT` marker), sorted. `None` when one names no
    /// operation (it cannot be witnessed).
    pub(crate) fn terminal_ops(&self, kind: (&str, Option<&str>)) -> Option<Vec<String>> {
        use crate::engine::l5::terminal_facts::TerminalSite;
        let mut out = Vec::new();
        for r in self.terminals.of_kind(kind) {
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
        cache: &SuffixCache<'a>,
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
            if let (Some("interface-dispatch"), Some(cs)) =
                (h.edge_kind.as_deref(), h.callsite_id.as_deref())
                && self.dispatch_excluded(from, cs, to, &b, ALL)
            {
                return false;
            }
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

    /// r3 §7 R5: [`Self::route_guards_steps`] of a group witness (its route
    /// through the dependency entry to its terminal operation `op`), with
    /// how many routines on the route had no statement tree to read guards
    /// from (their guards were not analyzed).
    pub(crate) fn route_guards_checked(
        &self,
        root: &str,
        steps: &[Step<'_>],
        op: &str,
    ) -> RouteGuardSet {
        let unread = |r: &str| {
            self.by_stable
                .get(r)
                .is_none_or(|m| m.statement_tree.is_none() && !self.flow_progs.contains_key(r))
        };
        let mut unanalyzed = 0usize;
        let mut cur = root;
        for s in steps {
            if unread(s.from) {
                unanalyzed += 1;
            }
            cur = s.to;
        }
        if unread(cur) {
            unanalyzed += 1;
        }
        RouteGuardSet {
            guards: self.route_guards_steps(root, steps, op),
            unanalyzed,
        }
    }

    /// The guards `site` sits under in `routine` (r3 E2), evaluated under `b`.
    fn guards_at(&self, routine: &str, site: &str, b: &Bindings) -> Vec<RouteGuard> {
        // Engine limits B2: the row (`#if` arm) that holds the site. The flow
        // program is built for the routine's first row only, so another arm's
        // site reads its own row's tree.
        let Some(r) = self.row_at(routine, site) else {
            return Vec::new();
        };
        let prog = self.flow_progs.get(routine).filter(|_| {
            self.by_stable
                .get(routine)
                .is_some_and(|f| std::ptr::eq(*f, r))
        });
        let tree: &PCFNNode = match (prog, &r.statement_tree) {
            (Some(p), _) => p.tree(),
            (None, Some(t)) => t,
            (None, None) => return Vec::new(),
        };
        // An operation the tree holds as its call (an `Error(…)` statement's
        // operation site, whose tree node carries the call site instead):
        // found by the call site at the same source position (r4 final, c05:
        // the `case` around `CDOXmlNode`'s `Error` at :18 was lost).
        let Some(guards) = lflow::guards_of(tree, site).or_else(|| {
            let op = r.operation_sites.iter().find(|o| o.id == site)?;
            let a = &op.source_anchor;
            let cs = r.call_sites.iter().find(|c| {
                let s = &c.source_anchor;
                (s.start_line, s.start_column) == (a.start_line, a.start_column)
            })?;
            lflow::guards_of(tree, &cs.id)
        }) else {
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
                    Some(
                        crate::program::body::features::FlowFact::Cond { text, .. }
                        | crate::program::body::features::FlowFact::Loop { text },
                    ) => text.to_string(),
                    _ => String::new(),
                },
                evaluated: prog.is_some_and(|p| p.guard_evaluated(node, &kind, &conds, &resolve)),
                kind,
            })
            .collect()
    }

    /// Under the root's search: whether operation `op` cannot run (module doc).
    fn op_dead(&self, reached: &Reached<'a>, op: &str, keep: &Keeps) -> bool {
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
        let (dead, resources, _, _, _) =
            self.unreachable_effects_noted(root, effects, per_site, &[]);
        (dead, resources)
    }

    /// Round-4 literal routes: the call sites of `sites` the search
    /// `reached` (made with `keep`) rules out: each one's routine is reached
    /// with guards ignored, and no reached state of it lets the site run.
    fn dead_sites(&self, reached: &Reached<'a>, sites: &[&str], keep: &Keeps) -> Vec<String> {
        sites
            .iter()
            .filter(|s| {
                self.op_owner
                    .get(**s)
                    .is_some_and(|o| reached.0.contains(o))
                    && self.op_dead(reached, s, keep)
            })
            .map(|s| s.to_string())
            .collect()
    }

    /// r4 final (m02, c15): the interface call sites of `sites` that run from
    /// the root, but only with a receiver whose implementation the literal
    /// arguments fix: in every reached state of the site's routine where the
    /// site can run, the receiver holds known enum values, and each value's
    /// implementation of the interface is a loaded codeunit
    /// ([`EnumImpls::implementers`]). An enum extension cannot change an
    /// existing value's implementation, so such a site is not open world:
    /// the search already follows its dispatch only to those codeunits
    /// ([`Self::dispatch_excluded`]). One state with the receiver not known
    /// (a route that passes a variable enum, a value read from a table, a
    /// `foreach` over the ordinals) keeps the site open world. A site no
    /// state can run is not here: [`Self::dead_sites`] has it.
    fn narrowed_sites(&self, reached: &Reached<'a>, sites: &[&str], keep: &Keeps) -> Vec<String> {
        let (plain, states) = reached;
        sites
            .iter()
            .filter(|s| {
                let Some(&owner) = self.op_owner.get(**s) else {
                    return false;
                };
                let Some(p) = self.flow_progs.get(owner) else {
                    return false;
                };
                if !plain.contains(owner) {
                    return false;
                }
                let Some(bs) = states.get(owner) else {
                    return false;
                };
                let mut live = false;
                for b in bs {
                    if self.dead_at_m(owner, s, b, keep) {
                        continue;
                    }
                    live = true;
                    let fixed = self.flow_eval(owner, b, keep).is_some_and(|ev| {
                        p.receiver_at(&ev, s).is_some_and(|(iface, vals)| {
                            !vals.is_empty()
                                && self
                                    .enum_impls
                                    .implementers(iface, vals)
                                    .is_some_and(|refs| !refs.is_empty())
                        })
                    });
                    if !fixed {
                        return false;
                    }
                }
                live
            })
            .map(|s| s.to_string())
            .collect()
    }

    /// [`Self::unreachable_effects`], and (r3 E2) per effect it drops ONLY
    /// because a publisher's loaded subscribers keep a value ([`lflow`]), the
    /// keep verdicts its ruling depends on ([`Self::relied_keeps`]: every
    /// route to it, not one): a reader states that assumption,
    /// and a world that does not make it counts the effect. A group resource
    /// is dropped only when it is dead without the assumption too.
    ///
    /// Round-4 literal routes: and the call sites of `sites` (a cone's
    /// unresolved sites) that cannot run from the root, without the
    /// assumption too: an unresolved site no route can reach blocks nothing.
    pub(crate) fn unreachable_effects_noted(
        &self,
        root: &'a str,
        effects: &[DigestEffectResult],
        per_site: &[&str],
        sites: &[&str],
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
        let Some(reached) = self.search(root, ALL) else {
            return Default::default();
        };
        let (dead, mut resources) = self.judge(&reached, effects, per_site, ALL);
        let mut keeps: Vec<u32> = reached
            .1
            .iter()
            .filter(|(r, _)| self.flow_progs.contains_key(*r))
            .flat_map(|(r, bs)| bs.iter().flat_map(|b| self.keeps_at(r, b)))
            .collect();
        keeps.sort_unstable();
        keeps.dedup();
        if keeps.is_empty() {
            let dead_sites = self.dead_sites(&reached, sites, ALL);
            let narrowed = self.narrowed_sites(&reached, sites, ALL);
            return (dead, resources, HashMap::new(), dead_sites, narrowed);
        }
        // Without the publisher keeps: what is dead only through them.
        let strict_reached = self.search(root, NONE);
        let dead_sites = strict_reached
            .as_ref()
            .map_or_else(Vec::new, |r| self.dead_sites(r, sites, NONE));
        let narrowed = strict_reached
            .as_ref()
            .map_or_else(Vec::new, |r| self.narrowed_sites(r, sites, NONE));
        let strict = strict_reached.map(|r| self.judge(&r, effects, per_site, NONE));
        let infos_of = |ks: &[u32]| -> Vec<KeepInfo> {
            ks.iter()
                .filter_map(|&k| self.keep_info.get(k as usize).cloned())
                .collect()
        };
        let infos = infos_of(&keeps);
        let mut assumed = HashMap::new();
        match &strict {
            Some((d2, r2)) => {
                // Per effect, the verdicts its ruling depends on.
                let mut only: Vec<usize> = dead.difference(d2).copied().collect();
                only.sort_unstable();
                for (i, ks) in self.relied_keeps(root, &reached, effects, per_site, &keeps, &only) {
                    assumed.insert(i, infos_of(&ks));
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
        (dead, resources, assumed, dead_sites, narrowed)
    }

    /// r3 §7 R1: per effect of `only` (each dead from `root` when every keep
    /// verdict of `keeps` is followed, live when none is), the verdicts its
    /// ruling depends on, judged over the WHOLE search, not one route: an
    /// effect reached by two routes, each cut by its own publisher, needs
    /// both publishers' keeps, and a subscriber to either one makes it live.
    ///
    /// The set is a smallest one that still rules the effect out when only
    /// it is followed, every member needed (dropping any one makes the
    /// effect live): first the verdicts that are needed alone (dropping one
    /// with all the others followed makes it live); when they do not rule it
    /// out together, the rest of `keeps` minus each one that is not needed,
    /// in order. Fewer followed keeps never make more dead, so a verdict
    /// needed alone stays needed in every smaller set. All of `keeps` when a
    /// search is capped or following exactly `keeps` does not rule it out.
    /// The searches are shared by every effect, and each one is the search
    /// with every keep followed (`reached`) replayed where the dropped
    /// verdicts change it, judging `only` ([`Self::dead_under`]).
    fn relied_keeps(
        &self,
        root: &'a str,
        reached: &Reached<'a>,
        effects: &[DigestEffectResult],
        per_site: &[&str],
        keeps: &[u32],
        only: &[usize],
    ) -> Vec<(usize, Vec<u32>)> {
        if only.is_empty() {
            return Vec::new();
        }
        let Some(g) = self.keep_graph(root) else {
            return only.iter().map(|&i| (i, keeps.to_vec())).collect();
        };
        let mut shell: Reached<'a> = (reached.0.clone(), HashMap::new());
        let mut memo: HashMap<Vec<u32>, Option<HashSet<usize>>> = HashMap::new();
        let mut dead_with = |set: &[u32], i: usize| -> bool {
            memo.entry(set.to_vec())
                .or_insert_with(|| {
                    let ks = Keeps::Only(set.into());
                    self.dead_under(&g, &mut shell, only, &ks, effects, per_site)
                })
                .as_ref()
                .is_some_and(|d| d.contains(&i))
        };
        let without =
            |set: &[u32], k: u32| -> Vec<u32> { set.iter().copied().filter(|&x| x != k).collect() };
        only.iter()
            .map(|&i| {
                if !dead_with(keeps, i) {
                    return (i, keeps.to_vec());
                }
                let needed: Vec<u32> = keeps
                    .iter()
                    .copied()
                    .filter(|&k| !dead_with(&without(keeps, k), i))
                    .collect();
                if !needed.is_empty() && dead_with(&needed, i) {
                    return (i, needed);
                }
                let mut set = keeps.to_vec();
                for &k in keeps {
                    if needed.contains(&k) {
                        continue;
                    }
                    let fewer = without(&set, k);
                    if dead_with(&fewer, i) {
                        set = fewer;
                    }
                }
                (i, set)
            })
            .collect()
    }

    /// The effects the search `reached` (made with `keep`) rules out, and per
    /// group its dead resources ([`Self::unreachable_effects`]).
    fn judge(
        &self,
        reached: &Reached<'a>,
        effects: &[DigestEffectResult],
        per_site: &[&str],
        keep: &Keeps,
    ) -> (HashSet<usize>, HashMap<usize, Vec<String>>) {
        let mut out = HashSet::new();
        let mut dead_resources: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, e) in effects.iter().enumerate() {
            let mut dead = Vec::new();
            if self.judge_row(reached, e, per_site, keep, Some(&mut dead)) {
                out.insert(i);
            }
            if !dead.is_empty() {
                dead_resources.insert(i, dead);
            }
        }
        (out, dead_resources)
    }

    /// [`Self::judge`]'s ruling of one row `e`: whether the search
    /// `reached` (made with `keep`) rules it out. A group whose crossing can
    /// run is not ruled out; its dead resources go to `resources` when
    /// given (left alone when not: they never decide the row).
    fn judge_row(
        &self,
        reached: &Reached<'a>,
        e: &DigestEffectResult,
        per_site: &[&str],
        keep: &Keeps,
        resources: Option<&mut Vec<String>>,
    ) -> bool {
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
                return true;
            }
        }
        // An effect's (or a group resource's) merged facts are the cone's
        // representatives, one per kind per root: a dead representative
        // says nothing of the kind's other origins. So a kind is judged by
        // EVERY direct fact of it in the root's reach ([`Self::kind_dead`]).
        let kind = e.effect_type.as_str();
        if e.boundary_group {
            if let Some(out) = resources {
                out.extend(
                    e.group_resources
                        .iter()
                        .filter(|(r, _)| self.kind_dead(reached, (kind, Some(r.as_str())), keep))
                        .map(|(r, _)| r.clone()),
                );
            }
            return false;
        }
        if per_site.contains(&kind)
            && let Some(op) = e.evidence_operation_id.as_deref()
        {
            return self.op_dead(reached, op, keep);
        }
        let resource = e
            .detail
            .iter()
            .find(|(k, _)| k == "resourceId")
            .map(|(_, v)| v.as_str());
        self.kind_dead(reached, (kind, resource), keep)
    }

    /// Whether no direct fact of `kind` (effect type, resource id) can run
    /// from the root: at least one sits in a routine the root reaches with
    /// guards ignored (else the graph does not explain the effect, and nothing
    /// is concluded), and none sits in a reached state of its routine where its
    /// site can run. A routine reached only through code that cannot run has
    /// no state, so its facts count as dead.
    fn kind_dead(&self, reached: &Reached<'a>, kind: (&str, Option<&str>), keep: &Keeps) -> bool {
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

#[cfg(test)]
mod r7_tests {
    use super::*;

    fn guard(evaluated: bool) -> RouteGuard {
        RouteGuard {
            routine: "R".into(),
            line: Some(1),
            text: "X".into(),
            kind: GuardKind::Must(true),
            evaluated,
        }
    }

    /// r3 §7 R5: a route whose guards beyond the dependency entry were not
    /// analyzed counts the analyzed part only and says so; a fully analyzed
    /// one counts the route; no replayed route (or a fully analyzed route
    /// without a guard) leaves the class label.
    ///
    /// DISCRIMINATION (2026-11-08): `route_condition_words` ignoring
    /// `beyond_unanalyzed` (always the `on the route` form) fails the first
    /// assert; restored, passes.
    #[test]
    fn the_condition_words_state_the_analyzed_boundary() {
        let notes = |guards: Option<Vec<RouteGuard>>, beyond: bool| FlowNotes {
            assumed: Vec::new(),
            guards,
            beyond_unanalyzed: beyond,
            ..FlowNotes::default()
        };
        assert_eq!(
            route_condition_words(Some(&notes(Some(vec![guard(true), guard(false)]), true)))
                .as_deref(),
            Some(
                "2 guards on the analyzed route, 1 evaluated; guards beyond the dependency \
                 entry: not analyzed"
            )
        );
        assert_eq!(
            route_condition_words(Some(&notes(Some(vec![guard(false)]), false))).as_deref(),
            Some("1 guard on the route, 0 evaluated")
        );
        assert_eq!(
            route_condition_words(Some(&notes(Some(Vec::new()), true))).as_deref(),
            Some(
                "0 guards on the analyzed route, 0 evaluated; guards beyond the dependency \
                 entry: not analyzed"
            )
        );
        assert_eq!(
            route_condition_words(Some(&notes(Some(Vec::new()), false))),
            None
        );
        assert_eq!(route_condition_words(Some(&notes(None, false))), None);
        assert_eq!(route_condition_words(None), None);
    }

    /// r3 §7 R1: the two forms of the sentence and the two `rules out`
    /// forms; the records group by assumption with their sites in file then
    /// line order (`:9` before `:10`).
    #[test]
    fn an_assumption_record_reads_as_one_sentence() {
        let keep = |subs: &[&str]| KeepInfo {
            publisher: "Codeunit \"E\"::OnX".into(),
            parameter: "P".into(),
            subscribers: subs.iter().map(|s| s.to_string()).collect(),
            universe: "App alone".into(),
        };
        let none = keep(&[]);
        let recs = AssumptionRecord::collect([
            (&none, Some("ws:a.al:10".to_string())),
            (&none, Some("ws:a.al:9".to_string())),
            (&none, Some("ws:a.al:10".to_string())),
        ]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].rules_out, vec!["ws:a.al:9", "ws:a.al:10"]);
        assert_eq!(
            recs[0].sentence(),
            "assumes Codeunit \"E\"::OnX has no loaded subscriber, so P keeps its value (App \
             alone; no additional subscribers assumed); rules out 2 sites (first: ws:a.al:9)"
        );
        let two = keep(&["Codeunit \"S\"::A", "Codeunit \"S\"::B"]);
        let recs = AssumptionRecord::collect([(&two, Some("ws:a.al:3".to_string()))]);
        assert_eq!(
            recs[0].sentence(),
            "assumes no subscribers beyond those considered in App alone (Codeunit \"S\"::A, \
             Codeunit \"S\"::B), none of which changes P; rules out ws:a.al:3"
        );
        assert!(!recs[0].sentence().contains("loaded-subscribers"));
    }

    /// r3 §7 R1, round-4 c25: a site two records list needs both
    /// assumptions, and its sentence names both; a site one record lists
    /// alone keeps that record's own sentence. Hand-stated records: `OnX`
    /// rules out `:5` and `:7`, `OnY` rules out `:5`; a third, `OnZ` with a
    /// subscriber, also rules out `:9` with `OnX` (the mixed form).
    ///
    /// DISCRIMINATION (2026-11-09): grouping each site by its first record
    /// only (`with = vec![ri]`) prints `OnX` alone for `:5` and fails the
    /// first assert; restored, passes.
    #[test]
    fn a_site_two_assumptions_rule_out_names_both() {
        let keep = |p: &str, subs: &[&str]| KeepInfo {
            publisher: format!("Codeunit \"E\"::{p}"),
            parameter: "P".into(),
            subscribers: subs.iter().map(|s| s.to_string()).collect(),
            universe: "App alone".into(),
        };
        let (x, y, z) = (keep("OnX", &[]), keep("OnY", &[]), keep("OnZ", &["S::A"]));
        let recs = AssumptionRecord::collect([
            (&x, Some("ws:a.al:5".to_string())),
            (&x, Some("ws:a.al:7".to_string())),
            (&y, Some("ws:a.al:5".to_string())),
            (&x, Some("ws:a.al:9".to_string())),
            (&z, Some("ws:a.al:9".to_string())),
        ]);
        assert_eq!(
            AssumptionRecord::sentences(&recs),
            vec![
                "assumes Codeunit \"E\"::OnX and Codeunit \"E\"::OnY have no loaded subscriber, \
                 so P keeps its value (App alone; no additional subscribers assumed); rules out \
                 ws:a.al:5"
                    .to_string(),
                "assumes Codeunit \"E\"::OnX has no loaded subscriber, so P keeps its value (App \
                 alone; no additional subscribers assumed); rules out ws:a.al:7"
                    .to_string(),
                "assumes Codeunit \"E\"::OnX has no loaded subscriber, so P keeps its value (App \
                 alone; no additional subscribers assumed), and assumes no subscribers beyond \
                 those considered in App alone (S::A), none of which changes P; rules out \
                 ws:a.al:9"
                    .to_string(),
            ]
        );
    }
}
