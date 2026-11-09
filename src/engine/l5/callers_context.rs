//! **Callers' context** (agent-tasks design §6, milestone A): the three prove
//! questions that look UP at a routine's callers, for an agent about to ADD a
//! `Commit()`, an `Error(...)` or UI to routine R. The six older prove
//! questions ask about R's **existing effects** (what R and its callees
//! already do); these ask about the context R runs in.
//!
//! - [`CallersQuestion::PendingWrites`] (`callers-have-pending-writes`): may
//!   some caller have a database write pending (written, not yet committed,
//!   in the same transaction) when R runs?
//! - [`CallersQuestion::InterceptErrors`] (`callers-intercept-errors`): may an
//!   error raised in R be intercepted by a caller before it ends the run?
//! - [`CallersQuestion::NonInteractive`] (`callers-may-run-noninteractive`):
//!   may R run in a session where no user can answer UI?
//!
//! Each walks R's reverse cone over the sited graph ([`reverse::walk_multi`],
//! R0) with its own [`ConeVisitor`]: the visitor's `step` evaluates every
//! reverse edge `caller C --site s--> R'` against C's body facts in the model
//! (the site found by its anchor), and decides whether the walk goes on above
//! C. Answers are `yes` (a path exists in the static graph: a may-answer),
//! `no` (the whole cone was seen and nothing blocks it), or `unknown` with the
//! blocking reasons. There is no "safe" verdict.
//!
//! # Pending writes (§6.3)
//!
//! At each edge the caller's [`pending_before_report`] (the same tree walk
//! the transaction spans use) gives the sites of C that may run before s:
//! earlier statements, the conditions on the way down, only the branch that
//! holds s, a loop's whole body (an earlier iteration). A physical write
//! among them (its own, or a callee's cone write) is a `pending-write`
//! witness. A `Commit()` statement before s ends the walk above C on that path
//! (what ran before it is committed); a checked `Codeunit.Run` (its own
//! transaction) and a background start (a new session) end it too. A trigger
//! edge from a write on a physical record is an `inside-write` witness; on a
//! temporary record it proves nothing. Above a publisher, the other
//! subscribers of the same event run in an unknown order: their cone writes
//! count (an over-approximation). A commit inside a callee before s does not
//! clear (keeps `no` sound). A TryFunction caller is not modelled (writes in
//! a try function follow a server setting): it blocks `no`, on its own row.
//! A caller the tree walk cannot place s in (no tree, s not found, no body
//! facts) blocks `no` (`order-unknown`).
//!
//! # Intercepted errors (§6.4)
//!
//! An error crosses an edge `C --s--> R'` as: `catch-and-return-false` when
//! R' is a TryFunction and s's result is consumed (a TryFunction called as a
//! statement is an ordinary call; an error while evaluating the call's
//! arguments happens in the caller, a different edge), or when s is a checked
//! `Codeunit.Run` (its own transaction is rolled back); `asserterror` when s
//! is under `asserterror`; `collectible` when C carries
//! `[ErrorBehavior(ErrorBehavior::Collect)]` (only errors raised as
//! collectible are collected; the walk goes on, since an ordinary error is
//! not); `isolated-event` when the edge is an event dispatch from the
//! publisher of an isolated event (`[IntegrationEvent(_, _, true)]`,
//! `[BusinessEvent(_, true)]`, `[InternalEvent(_, true)]`, source or symbols):
//! the platform runs each subscriber in its own transaction and rolls it back
//! on an error, and the publisher goes on (Microsoft Learn, "Isolated events
//! in AL"). That trap does not hold when the raising transaction has a write
//! pending or during an install or upgrade, so the walk goes on above the
//! publisher; otherwise it propagates. **A TryFunction does not roll back the
//! database changes made before the error** (Microsoft, "Handling errors
//! using Try methods"); nothing here says it does. A trap does not cross a
//! session or task boundary: a background start ends the path.
//!
//! # Non-interactive (§6.5)
//!
//! Independent of a UI kind. A path is a witness when it reaches an entry no
//! user answers: an API page, a web-service or job-queue root (from
//! `roots.config.json`), an install or upgrade codeunit, or a codeunit a
//! static `StartSession`/`TaskScheduler.CreateTask`/`EnqueueBackgroundTask`
//! starts (the target of a [`Boundary::Background`] site, F3: R0 reports it
//! as a `background-start` entry, placed at its first start site). A start
//! whose codeunit is not a literal has no such site; it blocks `no`
//! (`unresolved-background-target`). A background start resets the session
//! context: the run below it is non-interactive whatever started it, and the
//! walk does not go above the starter (no question here does: the starter's
//! pending writes, traps and UI are another session's). A call inside the
//! then-branch of `if GuiAllowed then` is not followed (no witness through
//! it); a condition that mentions `GuiAllowed` in another shape keeps the
//! witness and is listed. Guards are read from the revision's pinned bytes
//! ([`crate::engine::resident::guards::site_guards`], M0-3), only for callers
//! that mention `GuiAllowed`, so no per-site fact is added to the model.
//! Tests are not witnesses (a test session handles UI with handlers); they
//! are counted apart.
//!
//! # Common blockers of `no`
//!
//! Hidden callers in the cone, `OnlyOpen` ancestors (a hub), external
//! ancestors (apps that depend on the workspace may call them; off with
//! `assumeNoDependents`, which is its own flag, not `closedWorld`), a walk
//! limit, and a caller whose body facts are not in the model.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::engine::l4::cone_derived::ConeDerivedStore;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::pending_writes::{
    ForwardSites, PendingCtx, PendingReport, is_checked_run, pending_before_report,
};
use crate::engine::l5::prove::{BlockerAnchor, ProveAnswer, ProveBlocker};
use crate::program::body::features::{PCallSite, PCallee};
use crate::program::impact_graph::SitedGraph;
use crate::program::impact_graph::reverse::{
    ConeVisitor, EntryKind, ExternalWhy, HiddenChannel, Limit, NodeIx, SiteIx, Tier, Via,
    WalkOptions, WalkScratch, WalkState, config_entries, walk_multi,
};
use crate::program::impact_graph::sited::{Anchor, Boundary, RECORD_OPS, flag};
use crate::program::model::workspace::{Model, ModelObject, ModelRoutine};
use crate::program::resolve::background::{Starter, starter_of_callee};

/// The three questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallersQuestion {
    PendingWrites,
    InterceptErrors,
    NonInteractive,
}

impl CallersQuestion {
    /// The question's id, as `alsem prove` takes it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            CallersQuestion::PendingWrites => "callers-have-pending-writes",
            CallersQuestion::InterceptErrors => "callers-intercept-errors",
            CallersQuestion::NonInteractive => "callers-may-run-noninteractive",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<CallersQuestion> {
        [
            CallersQuestion::PendingWrites,
            CallersQuestion::InterceptErrors,
            CallersQuestion::NonInteractive,
        ]
        .into_iter()
        .find(|q| q.id() == text)
    }

    /// The `yes` headline's tail.
    fn yes_text(self) -> &'static str {
        match self {
            CallersQuestion::PendingWrites => {
                "a caller may have a database write pending when it runs"
            }
            CallersQuestion::InterceptErrors => "a caller may intercept an error raised in it",
            CallersQuestion::NonInteractive => "it may run where no user can answer UI",
        }
    }

    fn no_text(self) -> &'static str {
        match self {
            CallersQuestion::PendingWrites => "no caller has a database write pending when it runs",
            CallersQuestion::InterceptErrors => "every path propagates an error to its entry",
            CallersQuestion::NonInteractive => "every entry that reaches it is interactive",
        }
    }
}

/// One witness: the decisive site and how it reaches R.
#[derive(Clone, Debug)]
pub struct Witness {
    /// `pending-write`, `inside-write`, `catch-and-return-false`,
    /// `asserterror`, `collectible`, `isolated-event`,
    /// `non-interactive-entry`.
    pub kind: &'static str,
    /// What it is, in words (`Modify on "Customer"`, `checked Codeunit.Run`).
    pub detail: String,
    /// The human line's text when it differs from `detail` (a
    /// non-interactive entry: the entry and where it is started, without
    /// the legacy kind words `detail` keeps for JSON).
    pub words: Option<String>,
    /// The decisive site (the pending write, the trap, the entry), 1-based.
    pub file: Option<String>,
    pub line: Option<u32>,
    /// The entry kind, for a non-interactive entry.
    pub entry: Option<&'static str>,
    pub hops: u32,
    pub tier: Tier,
    /// From the witness's caller down to R: each routine with the site it
    /// leaves at.
    pub path: Vec<PathHop>,
    /// The witness holds only when a caller passes a physical record.
    pub caller_dependent: bool,
}

/// One hop of a witness path.
#[derive(Clone, Debug)]
pub struct PathHop {
    pub display: String,
    pub stable_id: Option<String>,
    /// Where it leaves toward the next hop (1-based); `None` for R itself
    /// and for an edge no call site makes.
    pub file: Option<String>,
    pub line: Option<u32>,
    /// The edge's boundary into the next hop (`sync`, `run`, `event`,
    /// `trigger`, `lifecycle`, `test`, `open`); `None` for R.
    pub boundary: Option<&'static str>,
    /// The hub kind (§8.1 words) when this hop is a hub: an unresolved
    /// dispatch the graph stands for with one node.
    pub hub: Option<String>,
}

/// One question's answer.
#[derive(Clone, Debug)]
pub struct CallersAnswer {
    pub question: CallersQuestion,
    pub answer: ProveAnswer,
    /// The first [`MAX_WITNESSES`] witnesses, nearest first.
    pub witnesses: Vec<Witness>,
    pub witness_count: usize,
    /// Of [`Self::witness_count`], those whose path uses an open site (a
    /// hub): listed after the others, never a `yes` alone.
    pub open_witness_count: usize,
    /// At most [`BLOCKER_SAMPLES`] per kind; [`Self::blocked_counts`] has
    /// every kind's full count.
    pub blocked_by: Vec<ProveBlocker>,
    pub blocked_counts: BTreeMap<&'static str, usize>,
    /// Entries by kind: those reached with a write pending (pending writes),
    /// reached with no trap (intercepted errors), or non-interactive ones
    /// (non-interactive). Distinct entry nodes per kind.
    pub entries: BTreeMap<&'static str, usize>,
    /// The same entries in the §8.1 words (output validation V2): by entry
    /// kind for pending writes and intercepted errors, by execution context
    /// for non-interactive; split by whether the entry was reached only over
    /// a hub (`true`: a candidate) or not (`false`). Distinct entry nodes per
    /// word, so two legacy kinds that name one word count a node once.
    pub entry_words: BTreeMap<(bool, &'static str), usize>,
    /// Intercepted errors: the nearest route from an entry down to R with no
    /// trap on it (printed with `detail`, F79): its tier and hops.
    pub uncaught_route: Option<(Tier, u32, Vec<PathHop>)>,
    /// Paths the walk stopped on, by reason (`commit`, `checked-run`,
    /// `background-start`, `try-function`, `checked-run-trap`, `asserterror`,
    /// `gui-allowed`).
    pub stopped: BTreeMap<&'static str, usize>,
    /// `CommitBehavior`/`InherentCommitBehavior` rows on the paths.
    pub commit_behavior: Vec<String>,
    /// TryFunction callers met (pending writes).
    pub try_function_callers: Vec<String>,
    /// TryFunction calls whose result is discarded: ordinary calls.
    pub not_intercepted: Vec<String>,
    /// Test entries reached (non-interactive: UI needs a handler in each).
    pub test_entries: usize,
    /// Guards that mention GuiAllowed in a shape not recognized.
    pub unrecognized_guards: Vec<String>,
    /// `assumeNoDependents` was given.
    pub assume_no_dependents: bool,
    pub states: usize,
    pub nodes: usize,
    /// Round-2 G24: the distinct direct callers the walk analyzed (the
    /// nodes it reached one hop up from the roots).
    pub direct_callers: usize,
}

/// The fold note of a witness list (round-3 c02): `detail` (in `detail`'s
/// words) lists the `kept_hidden` kept ones with their paths; the
/// `counted` others are counted, not kept, so nothing lists them
/// (`--detail lists 2 more; 12 only counted`).
fn more_words(kept_hidden: usize, counted: usize, detail: &str) -> String {
    // Short: c02's answer has a size ratchet.
    match (kept_hidden, counted) {
        (0, c) => format!("{c} only counted"),
        (k, 0) => format!("{detail} lists {k} more"),
        (k, c) => format!("{detail} lists {k} more; {c} only counted"),
    }
}

/// Witnesses kept per answer.
pub const MAX_WITNESSES: usize = 20;
/// Blockers kept per kind.
pub const BLOCKER_SAMPLES: usize = 5;
/// Witnesses through an open site kept per answer.
pub const OPEN_WITNESSES: usize = 3;

/// What a walk reads.
pub struct CallersCx<'a> {
    pub model: &'a Model,
    pub g: &'a SitedGraph,
    pub summaries: &'a HashMap<String, FullRoutineSummary>,
    pub cone_derived: &'a ConeDerivedStore,
    /// The pinned read: guard texts and the roots.config presence. `None`
    /// reads no guard (every GuiAllowed shape counts as unrecognized).
    pub inputs: Option<&'a crate::program::inputs::WorkspaceInputs>,
    pub assume_no_dependents: bool,
    pub cancel: Option<&'a dyn Fn() -> bool>,
}

/// The ext bit: a write is pending on this path (pending writes).
const PENDING: u8 = 1;
/// The ext bit: the path passed an unrecognized GuiAllowed guard.
const UNRECOGNIZED: u8 = 1;

/// The model's lookups, built once per answer.
struct Index<'a> {
    by_stable: HashMap<&'a str, Vec<&'a ModelRoutine>>,
    by_id: HashMap<&'a str, &'a ModelRoutine>,
    objects: HashMap<&'a str, &'a ModelObject>,
    collided: HashSet<&'a str>,
    /// Per node, its model rows (cached).
    rows: std::cell::RefCell<HashMap<NodeIx, Vec<&'a ModelRoutine>>>,
    /// Forward sites per node: `fwd_sites[fwd_off[n]..fwd_off[n + 1]]`.
    fwd_off: Vec<u32>,
    fwd_sites: Vec<u32>,
    /// The one routine display (output validation V3, §2.1, §2.2).
    names: crate::engine::l5::routine_rollup::RoutineNames<'a>,
    model: &'a Model,
    /// The model's resolved calls by caller row id, built on first use
    /// (r3 H4: a pending write's own site deeper in a callee's cone).
    calls_from: std::cell::OnceCell<HashMap<&'a str, Vec<&'a str>>>,
}

/// How many routines [`Index::deep_write_site`] visits at most.
const DEEP_SITE_VISITS: usize = 20_000;

/// `r`'s own first physical `op` on `table` (not known temporary): its file
/// and 1-based line.
fn own_write_site(r: &ModelRoutine, table: &str, op: &str) -> Option<(String, u32)> {
    r.record_operations
        .iter()
        .filter(|o| o.table_id.as_deref() == Some(table))
        .filter(|o| o.op.eq_ignore_ascii_case(op))
        .filter(|o| !crate::program::body::features::temp_state_suppresses(o.temp_state.as_ref()))
        .min_by_key(|o| (o.source_anchor.start_line, o.source_anchor.start_column))
        .map(|o| {
            (
                o.source_anchor.source_unit_id.clone(),
                o.source_anchor.start_line + 1,
            )
        })
}

impl<'a> Index<'a> {
    /// r3 H4 (c06): where a callee's cone performs `op` on `table`: the
    /// nearest routine (breadth first over the resolved calls, `callee`
    /// itself first) whose own physical record operation it is, and that
    /// operation's first site (`file`, 1-based line). r3 §7 R3 (p09): when
    /// the resolved calls reach none (the write sits behind an event, a
    /// trigger or an object run, as the cone follows them), the same search
    /// over the sited graph's may-execute edges from the callee's nodes,
    /// into routines whose cone performs the write only (an unguided search
    /// on CDO passed 20,000 nodes before reaching p09's write).
    /// `None` when neither finds one within [`DEEP_SITE_VISITS`] routines.
    fn deep_write_site(
        &self,
        cx: &CallersCx<'_>,
        callee: &str,
        table: &str,
        op: &str,
    ) -> Option<(String, u32)> {
        let calls = self.calls_from.get_or_init(|| {
            let mut m: HashMap<&str, Vec<&str>> = HashMap::new();
            for e in &self.model.calls.edges {
                if let Some(to) = e.to.as_deref() {
                    m.entry(e.from.as_str()).or_default().push(to);
                }
            }
            m
        });
        let start = self.by_id.get(callee)?;
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        seen.insert(start.id.as_str());
        queue.push_back(start.id.as_str());
        while let Some(id) = queue.pop_front() {
            if let Some(site) = self
                .by_id
                .get(id)
                .and_then(|r| own_write_site(r, table, op))
            {
                return Some(site);
            }
            for &to in calls.get(id).into_iter().flatten() {
                if seen.len() >= DEEP_SITE_VISITS {
                    queue.clear();
                    break;
                }
                if seen.insert(to) {
                    queue.push_back(to);
                }
            }
        }
        // The sited graph: every edge kind the cone follows, entering only
        // a routine whose own cone still performs the write (a node with no
        // model row, an object or a hub, is passed through).
        let g = cx.g;
        let cone_writes = |r: &ModelRoutine| {
            cx.cone_derived
                .physical_table_write_ops_of(&r.id)
                .get(table)
                .is_some_and(|ops| ops.iter().any(|o| o.eq_ignore_ascii_case(op)))
        };
        let mut seen: HashSet<NodeIx> = HashSet::new();
        let mut queue: std::collections::VecDeque<NodeIx> = std::collections::VecDeque::new();
        for n in g.nodes_with_stable_id(&start.stable_routine_id) {
            if seen.insert(n) {
                queue.push_back(n);
            }
        }
        while let Some(n) = queue.pop_front() {
            if let Some(site) = self
                .rows_of(g, n)
                .into_iter()
                .find_map(|r| own_write_site(r, table, op))
            {
                return Some(site);
            }
            for &s in self.sites_from(n) {
                let to = g.sites()[s as usize].to;
                if seen.contains(&to) {
                    continue;
                }
                let rows = self.rows_of(g, to);
                if !rows.is_empty() && !rows.iter().any(|r| cone_writes(r)) {
                    continue;
                }
                if seen.len() >= DEEP_SITE_VISITS {
                    return None;
                }
                seen.insert(to);
                queue.push_back(to);
            }
        }
        None
    }
    fn new(model: &'a Model, g: &SitedGraph) -> Index<'a> {
        let mut by_stable: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
        let mut by_id: HashMap<&str, &ModelRoutine> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut collided: HashSet<&str> = HashSet::new();
        for r in &model.workspace.routines {
            if !r.stable_routine_id.is_empty() {
                by_stable.entry(&r.stable_routine_id).or_default().push(r);
            }
            if !seen.insert(&r.id) {
                collided.insert(&r.id);
            }
            // Class P: the LAST row of an id.
            by_id.insert(&r.id, r);
        }
        let objects = model
            .workspace
            .objects
            .iter()
            .map(|o| (o.id.as_str(), o))
            .collect();
        let n = g.node_count();
        let mut count = vec![0u32; n + 1];
        for s in g.sites() {
            count[s.from as usize + 1] += 1;
        }
        for i in 0..n {
            count[i + 1] += count[i];
        }
        let mut fill = count.clone();
        let mut fwd_sites = vec![0u32; g.site_count()];
        for (i, s) in g.sites().iter().enumerate() {
            let at = &mut fill[s.from as usize];
            fwd_sites[*at as usize] = i as u32;
            *at += 1;
        }
        Index {
            by_stable,
            by_id,
            objects,
            collided,
            rows: Default::default(),
            fwd_off: count,
            fwd_sites,
            names: crate::engine::l5::routine_rollup::RoutineNames::new(model, &[]),
            model,
            calls_from: std::cell::OnceCell::new(),
        }
    }

    fn sites_from(&self, n: NodeIx) -> &[u32] {
        let (a, b) = (
            self.fwd_off[n as usize] as usize,
            self.fwd_off[n as usize + 1] as usize,
        );
        &self.fwd_sites[a..b]
    }

    /// The model rows of node `n` (several for a collided or `#if` id).
    fn rows_of(&self, g: &SitedGraph, n: NodeIx) -> Vec<&'a ModelRoutine> {
        if let Some(r) = self.rows.borrow().get(&n) {
            return r.clone();
        }
        let rows = g
            .stable_id(n)
            .and_then(|id| self.by_stable.get(id.as_str()).cloned())
            .unwrap_or_default();
        self.rows.borrow_mut().insert(n, rows.clone());
        rows
    }

    /// The row of `n` that holds `unit` (the site's file), else its first.
    fn row_for(&self, g: &SitedGraph, n: NodeIx, unit: Option<&str>) -> Option<&'a ModelRoutine> {
        let rows = self.rows_of(g, n);
        unit.and_then(|u| {
            rows.iter()
                .copied()
                .find(|r| same_unit(&r.source_anchor.source_unit_id, u))
        })
        .or_else(|| rows.first().copied())
    }

    /// A table's name for the text (`Table "Sales Header"`), from its model
    /// object id; the id itself when the model has no such object.
    fn table(&self, id: &str) -> String {
        use crate::engine::l5::routine_rollup::format_object;
        // Round-2 G41: a table the model holds by its table id
        // (`<app>/table/<number>`, the write's own key) is named, never
        // printed by number and app guid.
        // A dependency table prints its number (§2.2).
        if !self.objects.contains_key(id)
            && let Some(t) = self
                .model
                .workspace
                .tables
                .iter()
                .find(|t| t.id == id && !t.is_extension_stub)
        {
            let own = self
                .model
                .primary_app
                .as_ref()
                .is_some_and(|p| p.app_guid.eq_ignore_ascii_case(&t.app_guid));
            return format_object("Table", &t.name, (!own).then_some(t.table_number));
        }
        match self.objects.get(id) {
            Some(o) => format_object(&o.object_type, &o.name, None),
            // `<app guid>/<kind>/<number>`: a dependency object the model
            // holds no row for.
            None => match id.split('/').collect::<Vec<_>>()[..] {
                [app, kind, number] => {
                    let mut k = kind.to_string();
                    if let Some(c) = k.get_mut(0..1) {
                        c.make_ascii_uppercase();
                    }
                    format!("{k} {number} (app {app})")
                }
                _ => id.to_string(),
            },
        }
    }

    /// A node as every line names it: the one routine display (V3 §2.1,
    /// no caption) from its model row, else the graph node's declared names
    /// (§2.2), else its key.
    fn display(&self, g: &SitedGraph, n: NodeIx) -> String {
        use crate::engine::l5::routine_rollup::{format_routine, graph_display};
        match self.rows_of(g, n).first() {
            Some(r) => format_routine(&self.names.parts_of(r), false),
            None => graph_display(g, n, Some(self.model)).unwrap_or_else(|| g.display(n)),
        }
    }
}

/// Round-2 G24: a pending write behind a call, by its operation and table:
/// `Modify on "T" at <file:line> in <callee>, before the call` (the JSON
/// detail) — the operation from the callee's cone (its physical write ops on
/// the table), and its site, in the callee or deeper (r3 H4). The second
/// string is the human words: the write and its site, the call it is inside
/// and that call's location, then the call to the routine (`root_call`).
fn pending_call_words(
    ix: &Index<'_>,
    cx: &CallersCx<'_>,
    cs: &PCallSite,
    callee: &str,
    table: &str,
    root_call: Option<&(String, u32)>,
) -> (String, String) {
    let display = ix.table(table);
    let name = display.strip_prefix("Table ").unwrap_or(&display);
    let ops = cx.cone_derived.physical_table_write_ops_of(callee);
    let op = ops
        .get(table)
        .and_then(|v| v.first())
        .map(|o| {
            let mut s = o.to_string();
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            s
        })
        .unwrap_or_else(|| "a write".to_string());
    // r3 H4 (c06): the write's own site, in the callee or deeper in its cone.
    let site = ix.deep_write_site(cx, callee, table, &op);
    let at = site
        .as_ref()
        .map(|(f, l)| format!(" at {f}:{l}"))
        .unwrap_or_default();
    let detail = format!("{op} on {name}{at} in {}, before the call", cs.callee_text);
    // The human words: the write and its site, the call it is inside with
    // that call's location, and the call this answer is about with its own.
    let inner = format!(
        "{}:{}",
        cs.source_anchor.source_unit_id,
        cs.source_anchor.start_line + 1
    );
    let words = format!(
        "{op} on {name}{at}, inside the call {} at {inner}{}",
        cs.callee_text,
        root_call
            .map(|(f, l)| format!(", before the call at {f}:{l}"))
            .unwrap_or_else(|| ", before the call".to_string())
    );
    (detail, words)
}

/// A site of a caller's body, found by its anchor.
enum BodySite<'r> {
    Call(&'r PCallSite),
    Record(&'r crate::program::model::workspace::ModelRecordOperation),
}

impl BodySite<'_> {
    fn id(&self) -> &str {
        match self {
            BodySite::Call(c) => &c.id,
            BodySite::Record(o) => &o.id,
        }
    }
}

/// The site of `r` at anchor `a` in `unit`: the one starting there, else the
/// smallest that holds its start. Calls first, then record operations.
fn body_site<'r>(r: &'r ModelRoutine, unit: &str, a: &Anchor) -> Option<BodySite<'r>> {
    let at = |p: &crate::program::body::features::PAnchor| {
        same_unit(&p.source_unit_id, unit)
            && p.start_line == a.start_line
            && p.start_column == a.start_col
    };
    let holds = |p: &crate::program::body::features::PAnchor| {
        same_unit(&p.source_unit_id, unit) && p.contains(a.start_line, a.start_col)
    };
    let size = |p: &crate::program::body::features::PAnchor| {
        (
            p.end_line.saturating_sub(p.start_line),
            p.end_column.saturating_sub(p.start_column),
        )
    };
    if let Some(c) = r.call_sites.iter().find(|c| at(&c.source_anchor)) {
        return Some(BodySite::Call(c));
    }
    if let Some(o) = r.record_operations.iter().find(|o| at(&o.source_anchor)) {
        return Some(BodySite::Record(o));
    }
    let call = r
        .call_sites
        .iter()
        .filter(|c| holds(&c.source_anchor))
        .min_by_key(|c| size(&c.source_anchor));
    let op = r
        .record_operations
        .iter()
        .filter(|o| holds(&o.source_anchor))
        .min_by_key(|o| size(&o.source_anchor));
    match (call, op) {
        (Some(c), Some(o)) if size(&o.source_anchor) < size(&c.source_anchor) => {
            Some(BodySite::Record(o))
        }
        (Some(c), _) => Some(BodySite::Call(c)),
        (None, Some(o)) => Some(BodySite::Record(o)),
        (None, None) => None,
    }
}

fn has_attr(r: &ModelRoutine, name: &str) -> bool {
    crate::program::attributes::has_attribute(&r.attributes_parsed, name)
}

/// The member of an attribute's first argument (`CommitBehavior::Ignore` →
/// `Ignore`).
fn attr_member(r: &ModelRoutine, name: &str) -> Option<String> {
    let a = crate::program::attributes::find_attribute(&r.attributes_parsed, name)?;
    let arg = a.args.first()?;
    arg.member
        .clone()
        .or_else(|| arg.value.clone())
        .or_else(|| Some(arg.text.clone()))
}

/// A call that starts a new session or task
/// ([`crate::program::resolve::background::STARTERS`]): `StartSession`,
/// `Session.StartSession`, `TaskScheduler.CreateTask`,
/// `CurrPage.EnqueueBackgroundTask`.
fn background_start(cs: &PCallSite) -> Option<&'static Starter> {
    match &cs.callee {
        PCallee::Member { receiver, method } => starter_of_callee(Some(receiver), method),
        PCallee::Bare { name } => starter_of_callee(None, name),
        _ => None,
    }
}

fn boundary_str(b: Boundary) -> &'static str {
    match b {
        Boundary::Sync => "sync",
        Boundary::Run => "run",
        Boundary::Background => "background",
        Boundary::EventDispatch => "event",
        Boundary::TriggerOf { .. } => "trigger",
        Boundary::Lifecycle => "lifecycle",
        Boundary::TestHandler | Boundary::TestRunner => "test",
        Boundary::Open => "open",
    }
}

/// A site's place: its unit as the model spells it and its 1-based line. The
/// sited graph names a unit by its path alone; the caller `node`'s app says
/// which: `ws:<path>` in the workspace app, `dep:<app guid>:<path>` in a
/// dependency.
fn anchor_place(g: &SitedGraph, node: NodeIx, a: &Anchor) -> (String, u32) {
    let unit = g.unit(a.unit);
    let file = match g.node_app(node) {
        _ if unit.starts_with("ws:") || unit.starts_with("dep:") => unit.to_string(),
        Some(app) if app == g.primary_app() => format!("ws:{unit}"),
        Some(app) => format!("dep:{}:{unit}", g.app_identity(app).0),
        None => unit.to_string(),
    };
    (file, a.start_line + 1)
}

/// The model's unit id (`ws:<path>`, `dep:<guid>:<path>`) names the sited
/// graph's unit (the program's: the path alone).
fn same_unit(model: &str, sited: &str) -> bool {
    model == sited
        || model.strip_prefix("ws:") == Some(sited)
        || model
            .strip_suffix(sited)
            .is_some_and(|head| head.ends_with(':'))
}

/// What one reverse edge does to a walk (cached per site: it depends on the
/// caller's facts, never on the state).
#[derive(Clone, Default)]
struct SiteEval {
    /// Not followed above the caller.
    stop: Option<&'static str>,
    /// Bits the state above gets.
    bits: u8,
}

/// Whether a codeunit argument's text is a literal: `Codeunit::"X"`,
/// `Codeunit::X` or a number (as `background::start_target` reads the IR).
/// `failure`: `TaskScheduler.CreateTask`'s failure codeunit, where `0`
/// names none (and so is no dynamic target either).
fn literal_or_none(text: Option<&String>, failure: bool) -> bool {
    let Some(t) = text.map(|t| t.trim()) else {
        return failure;
    };
    if let Ok(n) = t.parse::<i64>() {
        return n > 0 || (failure && n == 0);
    }
    t.to_ascii_lowercase().starts_with("codeunit::")
}

/// The background starts whose codeunit is not a literal (`<callee> at
/// <file>:<line>`, sorted): the graph has no edge to their target (they reach
/// the open `run:codeunit` hub). A literal start's target is a
/// [`Boundary::Background`] site's (F3), so R0 reports it as a
/// `background-start` entry itself.
fn unresolved_background_starts(model: &Model) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for r in &model.workspace.routines {
        for cs in &r.call_sites {
            let Some(s) = background_start(cs) else {
                continue;
            };
            if !seen.insert((r.id.as_str(), cs.id.as_str())) {
                continue;
            }
            let literal = literal_or_none(cs.argument_texts.get(s.codeunit_arg), false)
                && s.failure_arg
                    .is_none_or(|f| literal_or_none(cs.argument_texts.get(f), true));
            if !literal {
                out.push(format!(
                    "{} at {}:{}",
                    cs.callee_text,
                    cs.source_anchor.source_unit_id,
                    cs.source_anchor.start_line + 1
                ));
            }
        }
    }
    out.sort();
    out
}

/// How a GuiAllowed guard places a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuiGuard {
    /// Only where a user is present (`if GuiAllowed then …`).
    Inside,
    /// Only where no user is present (the else branch, `if not GuiAllowed`).
    Outside,
    /// A condition that mentions GuiAllowed in another shape.
    Unrecognized,
    None,
}

/// A condition text's shape: `GuiAllowed`, `GuiAllowed()`, a conjunction
/// with it, or its negation.
fn gui_shape(text: &str) -> Option<bool> {
    let norm: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let strip = |s: &str| -> String {
        let mut s = s.to_string();
        while s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
            s = s[1..s.len() - 1].to_string();
        }
        s
    };
    let is_gui = |s: &str| {
        let s = strip(s);
        s == "guiallowed" || s == "guiallowed()"
    };
    let n = strip(&norm);
    if is_gui(&n) {
        return Some(true);
    }
    if let Some(rest) = n.strip_prefix("not")
        && is_gui(rest)
    {
        return Some(false);
    }
    // A conjunction holding GuiAllowed: every conjunct runs. Split on the
    // spaced keyword of the original text.
    let lower = text.to_ascii_lowercase();
    if !lower.contains(" or ") && lower.contains(" and ") {
        let parts: Vec<&str> = lower.split(" and ").collect();
        if parts.iter().any(|p| is_gui(&p.replace(' ', ""))) {
            return Some(true);
        }
    }
    None
}

/// The GuiAllowed guard of a site from its enclosing conditions, outermost
/// first.
pub(crate) fn classify_gui(guards: &[crate::engine::resident::guards::Guard]) -> GuiGuard {
    let mut out = GuiGuard::None;
    for gd in guards {
        if !gd.text.to_ascii_lowercase().contains("guiallowed") {
            continue;
        }
        let shape = gui_shape(&gd.text);
        let here = match (gd.kind, shape) {
            ("if", Some(true)) | ("else", Some(false)) => GuiGuard::Inside,
            ("if", Some(false)) | ("else", Some(true)) => GuiGuard::Outside,
            _ => GuiGuard::Unrecognized,
        };
        match here {
            GuiGuard::Inside => return GuiGuard::Inside,
            GuiGuard::Unrecognized => out = GuiGuard::Unrecognized,
            GuiGuard::Outside if out == GuiGuard::None => out = GuiGuard::Outside,
            _ => {}
        }
    }
    out
}

/// The 0-based line of the first guard [`classify_gui`] finds unrecognized:
/// the controlling statement of a condition that mentions GuiAllowed in a
/// shape it does not read.
fn unrecognized_gui_line(guards: &[crate::engine::resident::guards::Guard]) -> Option<u32> {
    guards
        .iter()
        .find(|gd| classify_gui(std::slice::from_ref(gd)) == GuiGuard::Unrecognized)
        .map(|gd| gd.line)
}

/// One caller row's forward facts ([`Visitor::caller_facts`]).
#[derive(Default)]
struct CallerFacts {
    fwd: Vec<(Option<String>, String)>,
    /// `(site, body site id, place)` of each unresolved site.
    open: Vec<(SiteIx, String, (String, u32))>,
}

/// The walk's visitor: one per question.
struct Visitor<'a, 'c> {
    q: CallersQuestion,
    cx: &'c CallersCx<'a>,
    ix: &'c Index<'a>,
    /// Each reached state's hops and parent.
    hops: HashMap<WalkState, u32>,
    parent: HashMap<WalkState, (WalkState, SiteIx)>,
    evals: HashMap<SiteIx, SiteEval>,
    witnesses: Vec<Witness>,
    /// Witnesses whose path uses an open site (a hub): listed, but they make
    /// no `yes` alone.
    open_witnesses: Vec<Witness>,
    witness_keys: HashSet<(NodeIx, SiteIx, &'static str)>,
    /// The human words of the next witness (r3 H4), taken by `witness`.
    next_words: Option<String>,
    witness_count: usize,
    open_witness_count: usize,
    blockers: BTreeMap<&'static str, Vec<ProveBlocker>>,
    blocker_keys: HashSet<(&'static str, String)>,
    blocked_counts: BTreeMap<&'static str, usize>,
    entries: BTreeMap<&'static str, BTreeSet<NodeIx>>,
    entry_words: BTreeMap<(bool, &'static str), BTreeSet<NodeIx>>,
    uncaught: Option<(Tier, u32, Vec<PathHop>)>,
    stopped: BTreeMap<&'static str, usize>,
    commit_behavior: BTreeSet<String>,
    try_function_callers: BTreeSet<String>,
    not_intercepted: BTreeSet<String>,
    tests: BTreeSet<NodeIx>,
    unrecognized: BTreeSet<String>,
    nodes: HashSet<NodeIx>,
    behavior_done: HashSet<NodeIx>,
    facts: HashMap<(NodeIx, usize), std::rc::Rc<CallerFacts>>,
    guards: crate::engine::resident::guards::GuardCache,
    page_stops: HashSet<SiteIx>,
}

impl<'a, 'c> Visitor<'a, 'c> {
    fn block(
        &mut self,
        kind: &'static str,
        key: String,
        detail: String,
        at: Option<(String, u32)>,
    ) {
        if !self.blocker_keys.insert((kind, key)) {
            return;
        }
        *self.blocked_counts.entry(kind).or_default() += 1;
        let rows = self.blockers.entry(kind).or_default();
        if rows.len() < BLOCKER_SAMPLES {
            rows.push(ProveBlocker {
                kind,
                detail,
                // A blocker anchor's line is the model's 0-based row (prove
                // writes it 1-based); `at` is already 1-based.
                anchor: at.map(|(file, line)| BlockerAnchor {
                    source_kind: "source".into(),
                    file: Some(file),
                    line: Some(line.saturating_sub(1)),
                    column: None,
                    excerpt: None,
                }),
            });
        }
    }

    /// The path from state `s` down to its root, starting with `s`'s node.
    fn path_of(&self, s: WalkState) -> Vec<PathHop> {
        let g = self.cx.g;
        let mut out = Vec::new();
        let mut cur = s;
        loop {
            let (file, line, boundary, next) = match self.parent.get(&cur) {
                Some((child, site)) => {
                    let st = &g.sites()[*site as usize];
                    let (file, line) = st
                        .anchor
                        .map(|a| anchor_place(g, st.from, &a))
                        .map_or((None, None), |(f, l)| (Some(f), Some(l)));
                    (file, line, Some(boundary_str(st.boundary)), Some(*child))
                }
                None => (None, None, None, None),
            };
            out.push(PathHop {
                display: self.ix.display(g, cur.node),
                stable_id: g.stable_id(cur.node),
                file,
                line,
                boundary,
                hub: crate::engine::l5::routine_rollup::hub_kind_of(g, cur.node),
            });
            match next {
                Some(n) if out.len() < 64 => cur = n,
                _ => break,
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn witness(
        &mut self,
        kind: &'static str,
        caller: NodeIx,
        site: SiteIx,
        from: WalkState,
        detail: String,
        at: Option<(String, u32)>,
        caller_dependent: bool,
    ) {
        // The human words set for this witness (r3 H4), if any.
        let words = self.next_words.take();
        if !self.witness_keys.insert((caller, site, kind)) {
            return;
        }
        let g = self.cx.g;
        let st = &g.sites()[site as usize];
        // The witness's path is open when the state it came from was, or the
        // site itself is (a hub's stand-in edge).
        let tier = if st.boundary == Boundary::Open {
            Tier::OnlyOpen
        } else if g.flags(caller) & flag::LOCAL == 0 {
            from.tier.max(Tier::ThroughDependency)
        } else {
            from.tier
        };
        self.witness_count += 1;
        if tier == Tier::OnlyOpen {
            self.open_witness_count += 1;
            if self.open_witnesses.len() >= OPEN_WITNESSES {
                return;
            }
        } else if self.witnesses.len() >= MAX_WITNESSES {
            return;
        }
        let hops = self.hops.get(&from).copied().unwrap_or(0) + 1;
        let mut path = vec![PathHop {
            display: self.ix.display(g, caller),
            stable_id: g.stable_id(caller),
            file: st.anchor.map(|a| anchor_place(g, st.from, &a).0),
            line: st.anchor.map(|a| a.start_line + 1),
            boundary: Some(boundary_str(st.boundary)),
            hub: crate::engine::l5::routine_rollup::hub_kind_of(g, caller),
        }];
        path.extend(self.path_of(from));
        let w = Witness {
            kind,
            words,
            detail,
            file: at.as_ref().map(|a| a.0.clone()),
            line: at.map(|a| a.1),
            entry: None,
            hops,
            tier,
            path,
            caller_dependent,
        };
        if tier == Tier::OnlyOpen {
            self.open_witnesses.push(w);
        } else {
            self.witnesses.push(w);
        }
    }

    /// Evaluate edge `site` (caller `C` = its `from`) for the question.
    fn eval(&mut self, from: WalkState, site: SiteIx) -> SiteEval {
        let g = self.cx.g;
        let st = g.sites()[site as usize];
        let c = st.from;
        let cf = g.flags(c);
        // A hub or an object node has no body: nothing to evaluate (a hub
        // makes the path `OnlyOpen`, a blocker of its own).
        if cf & flag::HUB != 0 || g.object_node(c).is_some() {
            return SiteEval::default();
        }
        // An error crossing an event dispatch is decided by the publisher's
        // declaration alone, so its body facts are not needed (a symbol-only
        // publisher, such as System's `OnCompanyOpenCompleted`, has none).
        if self.q == CallersQuestion::InterceptErrors && st.boundary == Boundary::EventDispatch {
            return self.eval_intercept_dispatch(from, site, c);
        }
        let unit = st.anchor.map(|a| g.unit(a.unit).to_string());
        let Some(r) = self.ix.row_for(g, c, unit.as_deref()) else {
            let d = self.ix.display(g, c);
            self.block(
                "order-unknown",
                d.clone(),
                format!("{d}: no body facts in the model (code the analysis did not load)"),
                None,
            );
            return SiteEval::default();
        };
        let at = st.anchor.map(|a| anchor_place(g, c, &a));
        let body = match (&st.anchor, &unit) {
            (Some(a), Some(u)) => body_site(r, u, a),
            _ => None,
        };
        // Async: a background start begins a new session (and transaction).
        if let Some(BodySite::Call(cs)) = &body
            && background_start(cs).is_some()
        {
            return SiteEval {
                stop: Some("background-start"),
                bits: 0,
            };
        }
        match self.q {
            CallersQuestion::PendingWrites => self.eval_pending(from, site, c, r, body, at),
            CallersQuestion::InterceptErrors => self.eval_intercept(from, site, c, r, body, at),
            CallersQuestion::NonInteractive => self.eval_gui(site, c, r, at),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_pending(
        &mut self,
        from: WalkState,
        site: SiteIx,
        c: NodeIx,
        r: &'a ModelRoutine,
        body: Option<BodySite<'a>>,
        at: Option<(String, u32)>,
    ) -> SiteEval {
        let g = self.cx.g;
        let st = g.sites()[site as usize];
        let display = self.ix.display(g, c);
        // Above a publisher: the other subscribers of the event.
        if st.boundary == Boundary::EventDispatch {
            let mut bits = 0;
            for &s2 in self.ix.sites_from(c) {
                let o = g.sites()[s2 as usize];
                if o.boundary != Boundary::EventDispatch || o.to == st.to {
                    continue;
                }
                let Some(sub) = self.ix.row_for(g, o.to, None) else {
                    continue;
                };
                let tables: BTreeSet<String> = self
                    .cx
                    .cone_derived
                    .physical_write_ids_of(&sub.id)
                    .map(|id| self.cx.cone_derived.resolve_res(id).to_string())
                    .collect();
                if let Some(t) = tables.iter().next() {
                    let sd = self.ix.display(g, o.to);
                    let place = Some((
                        sub.source_anchor.source_unit_id.clone(),
                        sub.source_anchor.start_line + 1,
                    ));
                    self.witness(
                        "pending-write",
                        o.to,
                        site,
                        from,
                        format!(
                            "sibling subscriber {sd} of the same event may write {} \
                             (subscribers run in an unknown order)",
                            self.ix.table(t)
                        ),
                        place,
                        false,
                    );
                    bits = PENDING;
                }
            }
            return SiteEval { stop: None, bits };
        }
        if has_attr(r, "TryFunction") {
            self.try_function_callers.insert(display.clone());
            self.block(
                "tryfunction-caller",
                display.clone(),
                format!(
                    "{display} is a TryFunction: writes inside a try function follow a server \
                     setting the analysis does not model"
                ),
                at,
            );
            return SiteEval::default();
        }
        let Some(body) = body else {
            self.block(
                "order-unknown",
                format!("{display}#{site}"),
                format!("{display}: the call site is not in its body facts"),
                at,
            );
            return SiteEval::default();
        };
        if let BodySite::Call(cs) = &body {
            let to_row = self.ix.row_for(g, st.to, None);
            let to_object = g.object_node(st.to);
            let checked = match &to_object {
                Some(o) => {
                    o.kind == al_syntax::ir::ObjectKind::Codeunit
                        && match &cs.callee {
                            PCallee::ObjectRun { .. } => cs.object_run_return_used == Some(true),
                            PCallee::Member { method, .. } => {
                                method.eq_ignore_ascii_case("run") && !cs.in_statement_position
                            }
                            _ => false,
                        }
                }
                None => is_checked_run(cs, to_row),
            };
            if checked {
                return SiteEval {
                    stop: Some("checked-run"),
                    bits: 0,
                };
            }
        }
        let mut bits = 0;
        // R runs inside a write's trigger.
        if let (Boundary::TriggerOf { op }, BodySite::Record(ro)) = (st.boundary, &body) {
            let op_name = op
                .map(|i| RECORD_OPS[i as usize])
                .unwrap_or(ro.op.as_str())
                .to_ascii_lowercase();
            if ["insert", "modify", "delete", "rename"].contains(&op_name.as_str()) {
                let ts = ro.temp_state.as_ref();
                let temp = crate::program::body::features::temp_state_suppresses(ts);
                let caller_dep = ts.is_some_and(|t| t.kind == "parameter-dependent");
                if !temp {
                    let table = ro
                        .table_id
                        .as_deref()
                        .map_or_else(|| ro.record_variable_name.clone(), |t| self.ix.table(t));
                    self.witness(
                        "inside-write",
                        c,
                        site,
                        from,
                        format!("runs inside {} on {table}", ro.op),
                        at.clone(),
                        caller_dep,
                    );
                    bits = PENDING;
                }
            }
        }
        // The caller's forward sites (for its callees' cone writes) and its
        // unresolved sites, once per caller row.
        let facts = self.caller_facts(c, r);
        let fwd_own = &facts.fwd;
        let mut fwd: ForwardSites<'_> = HashMap::new();
        fwd.insert(
            r.id.as_str(),
            fwd_own
                .iter()
                .map(|(s, t)| (s.as_deref(), t.as_str()))
                .collect(),
        );
        let pcx = PendingCtx {
            fwd: &fwd,
            routines: &self.ix.by_id,
            summaries: self.cx.summaries,
            cone_derived: self.cx.cone_derived,
            collided: &self.ix.collided,
        };
        let target = body.id().to_string();
        let rep: PendingReport = pending_before_report(r, &[target.as_str()], false, &pcx);
        if rep.whole_body {
            self.block(
                "order-unknown",
                format!("{display}#{site}"),
                format!(
                    "{display}: the statement order before the call is not known (no statement \
                     tree, or the call is not in it)"
                ),
                at.clone(),
            );
            return SiteEval { stop: None, bits };
        }
        // An unresolved call before the site: its writes are not known.
        for (s2, id, place) in &facts.open {
            if rep.sites.contains(id) {
                self.block(
                    "effects-incomplete",
                    format!("{display}#{s2}"),
                    format!("{display}: a call before the site is not resolved"),
                    Some(place.clone()),
                );
            }
        }
        if let Some(t) = rep.pending.tables.iter().next() {
            // The decisive write: the caller's own first write there, else
            // the first call whose cone writes.
            let own = r
                .record_operations
                .iter()
                .filter(|o| rep.sites.contains(&o.id))
                .filter(|o| {
                    [
                        "insert",
                        "modify",
                        "delete",
                        "rename",
                        "modifyall",
                        "deleteall",
                    ]
                    .contains(&o.op.to_ascii_lowercase().as_str())
                        && !crate::program::body::features::temp_state_suppresses(
                            o.temp_state.as_ref(),
                        )
                })
                .min_by_key(|o| (o.source_anchor.start_line, o.source_anchor.start_column));
            let (detail, place) = match own {
                Some(o) => (
                    format!(
                        "{} on {} before the call",
                        o.op,
                        o.table_id
                            .as_deref()
                            .map_or_else(|| o.record_variable_name.clone(), |t| self.ix.table(t))
                    ),
                    Some((
                        o.source_anchor.source_unit_id.clone(),
                        o.source_anchor.start_line + 1,
                    )),
                ),
                None => {
                    let via = r
                        .call_sites
                        .iter()
                        .filter(|cs| rep.sites.contains(&cs.id))
                        .find_map(|cs| {
                            fwd_own.iter().find_map(|(s, callee)| {
                                (s.as_deref() == Some(cs.id.as_str())
                                    && self
                                        .cx
                                        .cone_derived
                                        .physical_write_ids_of(callee)
                                        .next()
                                        .is_some())
                                .then_some((cs, callee))
                            })
                        });
                    match via {
                        Some((cs, callee)) => {
                            let (detail, words) =
                                pending_call_words(self.ix, self.cx, cs, callee, t, at.as_ref());
                            self.next_words = Some(words);
                            (
                                detail,
                                Some((
                                    cs.source_anchor.source_unit_id.clone(),
                                    cs.source_anchor.start_line + 1,
                                )),
                            )
                        }
                        None => (
                            format!("a write on {} before the call", self.ix.table(t)),
                            at.clone(),
                        ),
                    }
                }
            };
            self.witness("pending-write", c, site, from, detail, place, false);
            bits = PENDING;
        }
        // A Commit before the site: callers above have nothing pending at R
        // through this path (a write after the Commit, before the site, is
        // the witness above and keeps the path).
        let stop = (rep.committed && bits == 0).then_some("commit");
        SiteEval { stop, bits }
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_intercept(
        &mut self,
        from: WalkState,
        site: SiteIx,
        c: NodeIx,
        r: &'a ModelRoutine,
        body: Option<BodySite<'a>>,
        at: Option<(String, u32)>,
    ) -> SiteEval {
        let g = self.cx.g;
        let st = g.sites()[site as usize];
        let display = self.ix.display(g, c);
        let collect =
            attr_member(r, "ErrorBehavior").is_some_and(|m| m.eq_ignore_ascii_case("collect"));
        if collect {
            self.witness(
                "collectible",
                c,
                site,
                from,
                format!(
                    "{display} has [ErrorBehavior(ErrorBehavior::Collect)]: collectible errors \
                     only (an ordinary Error is not collected)"
                ),
                at.clone(),
                false,
            );
        }
        let Some(body) = body else {
            self.block(
                "order-unknown",
                format!("{display}#{site}"),
                format!("{display}: the call site is not in its body facts"),
                at,
            );
            return SiteEval::default();
        };
        let under_asserterror = match &body {
            BodySite::Call(cs) => cs.under_asserterror == Some(true),
            BodySite::Record(_) => r
                .operation_sites
                .iter()
                .any(|o| o.id == body.id() && o.under_asserterror == Some(true)),
        };
        if under_asserterror {
            self.witness(
                "asserterror",
                c,
                site,
                from,
                "the call is under asserterror (a test expects the error)".into(),
                at,
                false,
            );
            return SiteEval {
                stop: Some("asserterror"),
                bits: 0,
            };
        }
        let BodySite::Call(cs) = body else {
            return SiteEval::default();
        };
        let to_object = g.object_node(st.to);
        let to_row = self.ix.row_for(g, st.to, None);
        let checked = match &to_object {
            Some(o) => {
                o.kind == al_syntax::ir::ObjectKind::Codeunit
                    && match &cs.callee {
                        PCallee::ObjectRun { .. } => cs.object_run_return_used == Some(true),
                        PCallee::Member { method, .. } => {
                            method.eq_ignore_ascii_case("run") && !cs.in_statement_position
                        }
                        _ => false,
                    }
            }
            None => is_checked_run(cs, to_row),
        };
        if checked {
            self.witness(
                "catch-and-return-false",
                c,
                site,
                from,
                format!(
                    "checked {}: the error becomes false; the run's own transaction is rolled \
                     back",
                    cs.callee_text
                ),
                at,
                false,
            );
            return SiteEval {
                stop: Some("checked-run-trap"),
                bits: 0,
            };
        }
        // The edge enters a TryFunction.
        let into_try = to_row.is_some_and(|t| has_attr(t, "TryFunction"));
        if into_try && st.boundary == Boundary::Sync {
            if cs.in_statement_position {
                self.not_intercepted.insert(format!(
                    "{} called as a statement at {}: its result is discarded, so it is an \
                     ordinary call",
                    cs.callee_text,
                    at.as_ref()
                        .map_or(String::new(), |(f, l)| format!("{f}:{l}"))
                ));
                return SiteEval::default();
            }
            self.witness(
                "catch-and-return-false",
                c,
                site,
                from,
                format!(
                    "TryFunction {} with its result used: the error becomes false; a \
                     TryFunction does not roll back database changes",
                    cs.callee_text
                ),
                at,
                false,
            );
            return SiteEval {
                stop: Some("try-function"),
                bits: 0,
            };
        }
        SiteEval::default()
    }

    /// Intercepted errors at an event dispatch `publisher --event-->
    /// subscriber`. A subscriber's error reaches the publisher's caller,
    /// except on an isolated event: the platform runs each subscriber in a
    /// transaction of its own, rolls that transaction back on an error, and
    /// goes on with the next subscriber and then the publisher (Microsoft
    /// Learn, "Isolated events in AL"). That is a trap, and a witness. It
    /// holds only when the raising transaction has no write pending (with
    /// one, the event runs as a normal event) and not during an extension's
    /// install, uninstall or upgrade, so the walk goes on above the publisher:
    /// a path above may still let the error through.
    fn eval_intercept_dispatch(&mut self, from: WalkState, site: SiteIx, c: NodeIx) -> SiteEval {
        let g = self.cx.g;
        let st = g.sites()[site as usize];
        if st.via() == "eventFlow" && g.is_isolated_publisher(c) {
            let display = self.ix.display(g, c);
            self.witness(
                "isolated-event",
                c,
                site,
                from,
                format!(
                    "isolated event {display}: the subscriber runs in a transaction of its own; \
                     an error rolls back its database changes and the publisher goes on (unless \
                     the raising transaction has a write pending, or an extension is being \
                     installed or upgraded: then it runs as a normal event and the error \
                     propagates)"
                ),
                None,
                false,
            );
        }
        SiteEval::default()
    }

    fn eval_gui(
        &mut self,
        site: SiteIx,
        c: NodeIx,
        r: &'a ModelRoutine,
        at: Option<(String, u32)>,
    ) -> SiteEval {
        let mentions = r
            .identifier_references
            .iter()
            .any(|i| i.eq_ignore_ascii_case("guiallowed"))
            || r.call_sites.iter().any(
                |cs| matches!(&cs.callee, PCallee::Bare { name } if name.eq_ignore_ascii_case("guiallowed")),
            );
        if !mentions {
            return SiteEval::default();
        }
        let (guard, condition_line) = match self.cx.inputs {
            Some(inputs) => {
                match crate::engine::resident::guards::site_guards_cached(
                    inputs,
                    self.cx.g,
                    site,
                    &mut self.guards,
                ) {
                    Ok(gs) => (classify_gui(&gs), unrecognized_gui_line(&gs)),
                    Err(_) => (GuiGuard::None, None),
                }
            }
            None => (GuiGuard::None, None),
        };
        match guard {
            GuiGuard::Inside => SiteEval {
                stop: Some("gui-allowed"),
                bits: 0,
            },
            GuiGuard::Unrecognized => {
                // The place is the condition's own line (the controlling
                // statement's), not the guarded call's: the line names
                // where the condition is.
                let at = match (at, condition_line) {
                    (Some((f, _)), Some(l)) => Some((f, l + 1)),
                    (at, _) => at,
                };
                let d = self.ix.display(self.cx.g, c);
                let place = at
                    .as_ref()
                    .map_or(String::new(), |(f, l)| format!(" at {f}:{l}"));
                self.unrecognized.insert(format!("{d}{place}"));
                self.block(
                    "unrecognized-guard",
                    format!("{d}#{site}"),
                    format!("{d}: a condition mentions GuiAllowed in a shape not recognized"),
                    at,
                );
                SiteEval {
                    stop: None,
                    bits: UNRECOGNIZED,
                }
            }
            _ => SiteEval::default(),
        }
    }

    /// The caller's forward sites as [`ForwardSites`] wants them (its body
    /// site id, the callee row's id; a run of an object stands for its
    /// lifecycle routines; a publisher's event dispatch has no site) and its
    /// unresolved (`Open`) sites, computed once per caller row.
    fn caller_facts(&mut self, c: NodeIx, r: &'a ModelRoutine) -> std::rc::Rc<CallerFacts> {
        let key = (c, r as *const ModelRoutine as usize);
        if let Some(f) = self.facts.get(&key) {
            return f.clone();
        }
        let g = self.cx.g;
        let mut out = CallerFacts::default();
        for &s2 in self.ix.sites_from(c) {
            let o = g.sites()[s2 as usize];
            let (Some(a), Some(u)) = (o.anchor, o.anchor.map(|a| g.unit(a.unit))) else {
                if o.boundary == Boundary::EventDispatch
                    && let Some(sub) = self.ix.row_for(g, o.to, None)
                {
                    out.fwd.push((None, sub.id.clone()));
                }
                continue;
            };
            let Some(bs) = body_site(r, u, &a) else {
                continue;
            };
            if o.boundary == Boundary::Open {
                out.open
                    .push((s2, bs.id().to_string(), anchor_place(g, c, &a)));
            }
            // A run of an object: its lifecycle targets (OnRun, triggers).
            let targets: Vec<NodeIx> = if g.object_node(o.to).is_some() {
                self.ix
                    .sites_from(o.to)
                    .iter()
                    .map(|&x| g.sites()[x as usize])
                    .filter(|x| x.boundary == Boundary::Lifecycle)
                    .map(|x| x.to)
                    .collect()
            } else {
                vec![o.to]
            };
            for t in targets {
                if let Some(callee) = self.ix.row_for(g, t, None) {
                    out.fwd.push((Some(bs.id().to_string()), callee.id.clone()));
                }
            }
        }
        let f = std::rc::Rc::new(out);
        self.facts.insert(key, f.clone());
        f
    }

    fn commit_rows(&mut self, n: NodeIx) {
        if self.q != CallersQuestion::PendingWrites || !self.behavior_done.insert(n) {
            return;
        }
        let g = self.cx.g;
        for r in self.ix.rows_of(g, n) {
            if let Some(m) = attr_member(r, "CommitBehavior") {
                self.commit_behavior.insert(format!(
                    "{m} on {} (an added COMMIT below it is {})",
                    self.ix.display(g, n),
                    if m.eq_ignore_ascii_case("ignore") {
                        "ignored"
                    } else {
                        "an error"
                    }
                ));
            }
            if let Some(o) = self.ix.objects.get(r.object_id.as_str())
                && let Some(b) = &o.inherent_commit_behavior
            {
                self.commit_behavior.insert(format!(
                    "InherentCommitBehavior {b} on {} {}",
                    o.object_type, o.name
                ));
            }
        }
    }
}

impl ConeVisitor for Visitor<'_, '_> {
    fn reach(&mut self, _root: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        self.hops.insert(s, hops);
        self.nodes.insert(s.node);
        if let (Via::Site(site), Some(f)) = (via, from) {
            self.parent.insert(s, (f, site));
        }
        let pending = self.q == CallersQuestion::PendingWrites && s.ext & PENDING != 0;
        if s.tier == Tier::OnlyOpen && !pending {
            let d = self.ix.display(self.cx.g, s.node);
            self.block(
                "only-open",
                d.clone(),
                format!("{d} reaches it only over a hub"),
                None,
            );
        }
        if !pending {
            self.commit_rows(s.node);
        }
    }

    fn hidden(&mut self, _root: u32, n: NodeIx, ch: HiddenChannel) {
        let d = self.ix.display(self.cx.g, n);
        let what = match ch {
            HiddenChannel::Universal => "the universal hub (a caller of every node)".to_string(),
            HiddenChannel::Flag(f) => f.to_string(),
        };
        self.block(
            "hidden-callers",
            format!("{d}|{what}"),
            format!("{d}: {what}"),
            None,
        );
    }

    fn entry(&mut self, _root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        use crate::engine::vocab;
        let pending = s.ext & PENDING != 0;
        let open = s.tier == Tier::OnlyOpen;
        for k in kinds {
            match self.q {
                CallersQuestion::PendingWrites => {
                    if pending {
                        self.entries.entry(k.as_str()).or_default().insert(n);
                        self.entry_words
                            .entry((open, vocab::entry_kind(*k)))
                            .or_default()
                            .insert(n);
                    }
                }
                CallersQuestion::InterceptErrors => {
                    self.entries.entry(k.as_str()).or_default().insert(n);
                    self.entry_words
                        .entry((open, vocab::entry_kind(*k)))
                        .or_default()
                        .insert(n);
                    // F79: the nearest route an error takes to an entry
                    // with no trap, established ones before candidates.
                    if self
                        .uncaught
                        .as_ref()
                        .is_none_or(|(t, h, _)| (s.tier, hops) < (*t, *h))
                    {
                        self.uncaught = Some((s.tier, hops, self.path_of(s)));
                    }
                }
                CallersQuestion::NonInteractive => match k {
                    EntryKind::TestProcedure => {
                        self.tests.insert(n);
                    }
                    EntryKind::ApiPage
                    | EntryKind::WebServiceExposed
                    | EntryKind::JobQueueEntrypoint
                    | EntryKind::InstallCodeunit
                    | EntryKind::UpgradeCodeunit
                    | EntryKind::BackgroundStart => {
                        self.entries.entry(k.as_str()).or_default().insert(n);
                        if let Some(c) = vocab::entry_context(*k) {
                            self.entry_words.entry((open, c)).or_default().insert(n);
                        }
                        let d = self.ix.display(self.cx.g, n);
                        self.entry_witness(s, k.as_str(), d);
                    }
                    _ => {}
                },
            }
        }
    }

    fn external(&mut self, _root: u32, n: NodeIx, why: ExternalWhy, s: WalkState, _hops: u32) {
        if self.q == CallersQuestion::PendingWrites && s.ext & PENDING != 0 {
            return;
        }
        let d = self.ix.display(self.cx.g, n);
        let reason = match why {
            ExternalWhy::Public => "public: apps that depend on this app may call it",
            ExternalWhy::Protected => "protected: an extension in another app may call it",
            ExternalWhy::Friend => "internal, and the app names internalsVisibleTo friends",
        };
        self.block(
            "external-callers",
            d.clone(),
            format!("{d} is {reason}"),
            None,
        );
    }

    fn limit(&mut self, l: Limit) {
        let what = match l {
            Limit::StateBudget(n) => format!("the walk reached its state budget ({n})"),
            Limit::Cancelled => "the walk was cancelled".to_string(),
        };
        self.block("limit", what.clone(), what, None);
    }

    fn step(&mut self, _g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        if self.page_event(from, site) {
            return None;
        }
        if self.q == CallersQuestion::PendingWrites && from.ext & PENDING != 0 {
            // A write is pending on this path already: follow it only to
            // count the entries it reaches.
            return Some(from.ext);
        }
        let e = match self.evals.get(&site) {
            Some(e) => e.clone(),
            None => {
                let e = self.eval(from, site);
                self.evals.insert(site, e.clone());
                if let Some(why) = e.stop {
                    *self.stopped.entry(why).or_default() += 1;
                }
                e
            }
        };
        if e.stop.is_some() {
            return None;
        }
        Some(from.ext | e.bits)
    }
}

impl Visitor<'_, '_> {
    /// Whether `site` is a page's lifecycle edge, where the walk stops. An
    /// open page runs its triggers and actions on user events, after the code
    /// that opened it has finished: the opener's transaction and its error
    /// handling are not around them, and a page needs a user. The page's
    /// triggers are entries of their own (`page-action`, `trigger-page`); a
    /// page action's `RunObject` is counted here, since the walker reports it
    /// only over a followed site.
    fn page_event(&mut self, from: WalkState, site: SiteIx) -> bool {
        let g = self.cx.g;
        let st = g.sites()[site as usize];
        let page = st.boundary == Boundary::Lifecycle
            && g.object_node(st.from).is_some_and(|o| {
                matches!(
                    o.kind,
                    al_syntax::ir::ObjectKind::Page | al_syntax::ir::ObjectKind::PageExtension
                )
            });
        if !page {
            return false;
        }
        if g.is_action_run(site) {
            let counts = match self.q {
                CallersQuestion::PendingWrites => from.ext & PENDING != 0,
                CallersQuestion::InterceptErrors => true,
                CallersQuestion::NonInteractive => false,
            };
            if counts {
                self.entries
                    .entry("page-action")
                    .or_default()
                    .insert(st.from);
                self.entry_words
                    .entry((from.tier == Tier::OnlyOpen, "page action"))
                    .or_default()
                    .insert(st.from);
            }
        }
        if self.page_stops.insert(site) {
            *self.stopped.entry("page-event").or_default() += 1;
        }
        true
    }

    fn entry_witness(&mut self, s: WalkState, kind: &'static str, display: String) {
        let key = (s.node, u32::MAX, kind);
        if !self.witness_keys.insert(key) {
            return;
        }
        self.witness_count += 1;
        if s.tier == Tier::OnlyOpen {
            self.open_witness_count += 1;
            if self.open_witnesses.len() >= OPEN_WITNESSES {
                return;
            }
        } else if self.witnesses.len() >= MAX_WITNESSES {
            return;
        }
        let g = self.cx.g;
        // A background-start entry is the started codeunit's object node: its
        // place is the first start site into it (the call, in the starter).
        let start = (kind == "background-start")
            .then(|| {
                g.sites_into(s.node).iter().find_map(|&x| {
                    let st = g.sites()[x as usize];
                    (st.boundary == Boundary::Background)
                        .then(|| st.anchor.map(|a| anchor_place(g, st.from, &a)))
                        .flatten()
                })
            })
            .flatten();
        let place = start.clone().or_else(|| {
            self.ix.row_for(g, s.node, None).map(|r| {
                (
                    r.source_anchor.source_unit_id.clone(),
                    r.source_anchor.start_line + 1,
                )
            })
        });
        let extra = if s.ext & UNRECOGNIZED != 0 {
            " (past a GuiAllowed condition not recognized)"
        } else {
            ""
        };
        let detail = match (&start, kind) {
            (Some((file, line)), _) => format!(
                "{display} is started in a background session or task (at {file}:{line}){extra}"
            ),
            (None, "background-start") => {
                format!("{display} is started in a background session or task{extra}")
            }
            _ => format!("{display} is a {kind} entry{extra}"),
        };
        // The human line's own words (output validation V2): the entry,
        // where it is started, the guard note; `detail` keeps its JSON text.
        let words = format!(
            "{display}{}{extra}",
            start
                .as_ref()
                .map(|(f, l)| format!(", started at {f}:{l}"))
                .unwrap_or_default()
        );
        let w = Witness {
            kind: "non-interactive-entry",
            words: Some(words),
            detail,
            file: place.as_ref().map(|p| p.0.clone()),
            line: place.map(|p| p.1),
            entry: Some(kind),
            hops: self.hops.get(&s).copied().unwrap_or(0),
            tier: s.tier,
            path: self.path_of(s),
            caller_dependent: false,
        };
        if s.tier == Tier::OnlyOpen {
            self.open_witnesses.push(w);
        } else {
            self.witnesses.push(w);
        }
    }
}

/// Answer `q` for the routine nodes `roots` (one routine; several nodes for
/// `#if` arms).
pub fn answer(q: CallersQuestion, roots: &[NodeIx], cx: &CallersCx<'_>) -> CallersAnswer {
    let g = cx.g;
    let ix = Index::new(cx.model, g);
    let unresolved_bg = unresolved_background_starts(cx.model);
    let config = config_entries(g, cx.model);
    let opts = WalkOptions {
        assume_no_dependents: cx.assume_no_dependents,
        cancel: cx.cancel,
        extra_entries: Some(&config),
        ..WalkOptions::default()
    };
    let mut v = Visitor {
        q,
        cx,
        ix: &ix,
        hops: HashMap::new(),
        parent: HashMap::new(),
        evals: HashMap::new(),
        witnesses: Vec::new(),
        open_witnesses: Vec::new(),
        witness_keys: HashSet::new(),
        next_words: None,
        witness_count: 0,
        open_witness_count: 0,
        blockers: BTreeMap::new(),
        blocker_keys: HashSet::new(),
        blocked_counts: BTreeMap::new(),
        entries: BTreeMap::new(),
        entry_words: BTreeMap::new(),
        uncaught: None,
        stopped: BTreeMap::new(),
        commit_behavior: BTreeSet::new(),
        try_function_callers: BTreeSet::new(),
        not_intercepted: BTreeSet::new(),
        tests: BTreeSet::new(),
        unrecognized: BTreeSet::new(),
        nodes: HashSet::new(),
        behavior_done: HashSet::new(),
        facts: HashMap::new(),
        guards: Default::default(),
        page_stops: HashSet::new(),
    };
    // The question's own blockers, before the walk.
    if q == CallersQuestion::NonInteractive {
        let has_config = cx
            .inputs
            .is_none_or(|i| matches!(i.roots_config, Some(Ok(_))));
        if !has_config {
            v.block(
                "publication-unknown",
                "roots.config.json".into(),
                "no roots.config.json in the workspace: job-queue entries and published web \
                 services live in data, not in source"
                    .into(),
                None,
            );
        }
        if let Some(i) = cx.inputs {
            let wsp = i.workspace.join("WebServicePublish.xml");
            if wsp.is_file() {
                v.block(
                    "unread-publication-file",
                    "WebServicePublish.xml".into(),
                    "WebServicePublish.xml is in the workspace and is not read: the web services \
                     it publishes are not entries here"
                        .into(),
                    None,
                );
            }
        }
        for u in &unresolved_bg {
            v.block(
                "unresolved-background-target",
                u.clone(),
                format!("{u}: a background start whose codeunit is not a literal"),
                None,
            );
        }
    }
    let mut scratch = WalkScratch::default();
    let stats = walk_multi(g, roots, &opts, &mut scratch, &mut v);
    let witness_count = v.witness_count;
    let mut blocked_by: Vec<ProveBlocker> = v.blockers.into_values().flatten().collect();
    blocked_by.sort_by(|a, b| a.kind.cmp(b.kind).then(a.detail.cmp(&b.detail)));
    // A witness only through an open site (a hub's stand-in for "any page
    // run", "any call of this name") makes no `yes`: the hub over-approximates.
    // It is listed, and the `only-open` blocker it came with keeps a `no` out.
    let firm = witness_count - v.open_witness_count;
    let answer = if firm > 0 {
        ProveAnswer::Yes
    } else if v.blocked_counts.is_empty() && witness_count == 0 {
        ProveAnswer::No
    } else {
        ProveAnswer::Unknown
    };
    let mut witnesses = v.witnesses;
    // A collectible-only trap, and an isolated event's (it holds only with no
    // write pending), after the ones that catch every error.
    witnesses.sort_by_key(|w| {
        (
            w.tier,
            w.hops,
            matches!(w.kind, "collectible" | "isolated-event"),
        )
    });
    let mut open = v.open_witnesses;
    open.sort_by_key(|w| w.hops);
    witnesses.extend(open);
    let direct_callers = v
        .hops
        .iter()
        .filter(|(_, h)| **h == 1)
        .map(|(s, _)| s.node)
        .collect::<HashSet<_>>()
        .len();
    CallersAnswer {
        direct_callers,
        question: q,
        answer,
        witnesses,
        witness_count,
        open_witness_count: v.open_witness_count,
        blocked_by,
        blocked_counts: v.blocked_counts,
        entries: v.entries.into_iter().map(|(k, s)| (k, s.len())).collect(),
        entry_words: entry_word_counts(q, v.entry_words),
        uncaught_route: v.uncaught,
        stopped: v.stopped,
        commit_behavior: v.commit_behavior.into_iter().collect(),
        try_function_callers: v.try_function_callers.into_iter().collect(),
        not_intercepted: v.not_intercepted.into_iter().collect(),
        test_entries: v.tests.len(),
        unrecognized_guards: v.unrecognized.into_iter().collect(),
        assume_no_dependents: cx.assume_no_dependents,
        states: stats.states,
        nodes: v.nodes.len(),
    }
}

/// The answer as prove's `result` object (prove 2.4.0): `answer`,
/// `evidence[]`, `blockedBy[]`, `assumptions[]`, and the callers' context
/// fields beside them.
#[must_use]
pub fn result_value(a: &CallersAnswer) -> serde_json::Value {
    use serde_json::json;
    let evidence: Vec<serde_json::Value> = a
        .witnesses
        .iter()
        .map(|w| {
            let path: Vec<serde_json::Value> = w
                .path
                .iter()
                .map(|h| {
                    json!({
                        "routine": h.stable_id,
                        "display": h.display,
                        "site": h.file.as_ref().map(|f| json!({"file": f, "line": h.line})),
                        "boundary": h.boundary,
                    })
                })
                .collect();
            json!({
                "kind": w.kind,
                "detail": w.detail,
                "evidence": {"file": w.file, "line": w.line, "sourceKind": "source"},
                "entry": w.entry.map(|k| json!({"kind": k})),
                "hops": w.hops,
                "tier": w.tier.as_str(),
                // §8.3: the §8.1 word in a new field beside the legacy one.
                "pathTier": crate::engine::vocab::tier(w.tier),
                "callerDependent": w.caller_dependent.then_some(true),
                "path": path,
            })
        })
        .collect();
    let blocked: Vec<serde_json::Value> = a
        .blocked_by
        .iter()
        .map(crate::engine::l5::prove::blocker_to_value)
        .collect();
    let mut assumptions = Vec::new();
    if a.assume_no_dependents {
        assumptions.push("assume-no-dependents");
    }
    let candidates = a.open_witness_count;
    json!({
        "answer": a.answer.as_str(),
        "evidence": evidence,
        "evidenceCount": a.witness_count,
        "evidenceThroughOpenSites": a.open_witness_count,
        // Output validation V2 (§3.1, §8.1 result tier): established by the
        // static model vs candidates over a hub, counted apart.
        "establishedWitnesses": a.witness_count - candidates,
        "candidateWitnesses": {
            "count": candidates,
            "byTier": if candidates > 0 { json!({"via-hub": candidates}) } else { json!({}) },
        },
        "blockedBy": blocked,
        "blockedByCounts": a.blocked_counts,
        "assumptions": assumptions,
        "entries": a.entries,
        "stopped": a.stopped,
        "commitBehavior": a.commit_behavior,
        "tryFunctionCallers": a.try_function_callers,
        "notIntercepted": a.not_intercepted,
        "testEntries": a.test_entries,
        "unrecognizedGuards": a.unrecognized_guards,
        "cone": {"states": a.states, "nodes": a.nodes},
        // Round-2 G24 (additive): the population explain's answers name.
        "directCallersAnalyzed": a.direct_callers,
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The entry words of one class (`false`: established, `true`: over a hub),
/// largest first: `page actions 3, page triggers 2`.
/// The precedence of an entry kind word when one entry has several (output
/// validation §8.2, F53): the most specific kind is its own (a page
/// action's `OnAction` is a page action, not also a page trigger).
fn entry_kind_rank(word: &str) -> usize {
    [
        "test procedure",
        "install trigger",
        "upgrade trigger",
        "page action",
        "page trigger",
        "report trigger",
        "table trigger",
        "event subscriber",
        "codeunit OnRun",
        "public procedure",
    ]
    .iter()
    .position(|w| *w == word)
    .unwrap_or(usize::MAX)
}

/// Distinct entries counted per word (output validation §8.2, F53). By
/// entry KIND (pending writes, intercepted errors) the kinds are disjoint:
/// each entry counts once, in its own kind ([`entry_kind_rank`]), and under
/// "established" when any of its occurrences is (a candidate over a hub
/// only otherwise), so the counts add up to the distinct entries. By
/// execution CONTEXT (non-interactive) contexts may overlap and each word
/// counts its own entries (the line says so).
fn entry_word_counts<N: Ord + Copy>(
    q: CallersQuestion,
    words: BTreeMap<(bool, &'static str), BTreeSet<N>>,
) -> BTreeMap<(bool, &'static str), usize> {
    if q == CallersQuestion::NonInteractive {
        return words.into_iter().map(|(k, s)| (k, s.len())).collect();
    }
    let mut own: BTreeMap<N, (bool, &'static str)> = BTreeMap::new();
    for ((open, w), nodes) in &words {
        for n in nodes {
            let e = own.entry(*n).or_insert((*open, *w));
            if (*open, entry_kind_rank(w)) < (e.0, entry_kind_rank(e.1)) {
                *e = (*open, *w);
            }
        }
    }
    let mut out: BTreeMap<(bool, &'static str), usize> = BTreeMap::new();
    for k in own.into_values() {
        *out.entry(k).or_default() += 1;
    }
    out
}

fn entry_words_line(a: &CallersAnswer, over_hub: bool) -> String {
    let mut v: Vec<(&str, usize)> = a
        .entry_words
        .iter()
        .filter(|((h, _), _)| *h == over_hub)
        .map(|((_, w), n)| (*w, *n))
        .collect();
    v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(y.0)));
    let by_context = a.question == CallersQuestion::NonInteractive;
    v.iter()
        .map(|(w, n)| {
            if by_context {
                format!("{w} {n}")
            } else {
                format!("{} {n}", crate::engine::vocab::entry_kind_plural(w))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every blocker kind with its count in the §8.1 words, largest first.
fn blocker_counts(a: &CallersAnswer) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = a
        .blocked_counts
        .iter()
        .map(|(k, n)| (crate::engine::vocab::blocker(k), *n))
        .collect();
    v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    v
}

/// What the human text needs to name follow-ups (output validation §6.1):
/// the surface's words, the call's scope inputs, and the routine's selector.
#[derive(Clone, Copy, Debug)]
pub struct HumanCx<'s> {
    pub fcx: crate::engine::follow_up::Cx,
    pub selector: &'s str,
}

impl HumanCx<'_> {
    /// `detail` in the surface's words.
    fn detail(&self) -> &'static str {
        self.fcx.input(crate::engine::follow_up::Input::Detail)
    }
}

/// The blocker kinds of an `unknown`, at most `cap` named (largest first),
/// the rest counted: `external callers 2, over a hub 9`.
fn blockers_named(
    kinds: &[(String, usize)],
    cap: usize,
    detail: bool,
    hcx: &HumanCx<'_>,
) -> String {
    let shown = if detail {
        kinds.len()
    } else {
        cap.min(kinds.len())
    };
    let mut s = kinds[..shown]
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    if kinds.len() > shown {
        let more = kinds.len() - shown;
        s.push_str(&format!(
            ", and {more} more {} ({} lists them)",
            if more == 1 { "kind" } else { "kinds" },
            hcx.detail()
        ));
    }
    s
}

/// The one-line form explain's "callers' context" section prints.
#[must_use]
pub fn summary_line(a: &CallersAnswer) -> String {
    let label = match a.question {
        CallersQuestion::PendingWrites => "pending writes",
        CallersQuestion::InterceptErrors => "intercepted errors",
        CallersQuestion::NonInteractive => "non-interactive",
    };
    let mut s = format!("{label}: {}", a.answer.as_str());
    match a.answer {
        ProveAnswer::Yes => {
            let of = match a.question {
                CallersQuestion::PendingWrites => "of an uncommitted write when this runs",
                CallersQuestion::InterceptErrors => "of a caller that may intercept its error",
                CallersQuestion::NonInteractive => "of an entry no user answers",
            };
            s.push_str(&format!(
                " — {} {of}",
                plural(a.witness_count, "witness", "witnesses")
            ));
            let established = entry_words_line(a, false);
            if !established.is_empty() {
                s.push_str(&format!(" ({established})"));
            }
        }
        ProveAnswer::Unknown => {
            let kinds: Vec<String> = blocker_counts(a)
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            s.push_str(&format!(" (blockers: {})", kinds.join(", ")));
        }
        ProveAnswer::No => s = format!("{label}: {}", NO_WORDS),
    }
    s
}

/// The §8.1 negative-certainty words of a callers' context `no`: the whole
/// cone was seen and nothing blocks it.
const NO_WORDS: &str = crate::engine::vocab::NO_EVERY_CANDIDATE_DECIDED;

/// The population a question's answer is about, for an `unknown`'s answer
/// line (§3.1: the question's own population, not all callers).
fn population(q: CallersQuestion) -> &'static str {
    match q {
        CallersQuestion::PendingWrites => "pending write",
        CallersQuestion::InterceptErrors => "intercepting caller",
        CallersQuestion::NonInteractive => "non-interactive route",
    }
}

/// What an `unknown` does not establish (§4.4, F20, F21, F79), in the
/// question's terms.
fn not_established(q: CallersQuestion) -> &'static str {
    match q {
        CallersQuestion::PendingWrites => {
            "that no caller has a write pending when it runs; do not assume a COMMIT added here \
             commits only this routine's work"
        }
        CallersQuestion::InterceptErrors => {
            "that no caller turns this error into false; and nothing here says which writes roll \
             back"
        }
        CallersQuestion::NonInteractive => {
            "that a user is present; do not assume an interactive session in code added here"
        }
    }
}

/// The hubs on the candidate paths, in the §8.1 form with the site that
/// enters each (the hop before it leaves there), distinct, at most `cap`.
fn candidate_hubs(a: &CallersAnswer, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in a.witnesses.iter().filter(|w| w.tier == Tier::OnlyOpen) {
        for (i, h) in w.path.iter().enumerate() {
            let Some(kind) = &h.hub else { continue };
            let at = i
                .checked_sub(1)
                .and_then(|j| w.path.get(j))
                .and_then(|p| Some(format!("{}:{}", p.file.as_ref()?, p.line?)));
            let s = crate::engine::vocab::hub(kind, None, at.as_deref());
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.truncate(cap);
    out
}

/// A witness path's hops, a hub in the §8.1 form with its entering site. A
/// location prints once (§7.5, F45): the hop that enters a hub does not
/// print the site the hub's `at` names.
pub(crate) fn path_text(path: &[PathHop]) -> String {
    let mut hops: Vec<String> = Vec::new();
    for (i, h) in path.iter().enumerate() {
        if let Some(kind) = &h.hub {
            let at = i
                .checked_sub(1)
                .and_then(|j| path.get(j))
                .and_then(|p| Some(format!("{}:{}", p.file.as_ref()?, p.line?)));
            hops.push(crate::engine::vocab::hub(kind, None, at.as_deref()));
            continue;
        }
        let enters_hub = path.get(i + 1).is_some_and(|n| n.hub.is_some());
        // Round-2 G45: a hop's location is where it leaves toward the next
        // hop, never its definition, and says so.
        let role = match h.boundary {
            Some("event") => "raised at",
            Some("trigger") => "operation at",
            _ => "called at",
        };
        hops.push(match (&h.file, h.line) {
            (Some(f), Some(l)) if !enters_hub => format!("{} ({role} {f}:{l})", h.display),
            _ => h.display.clone(),
        });
    }
    hops.join(" → ")
}

/// The blocker kinds that carry an action (§7.5, F45): a guard to rewrite
/// in a shape the analysis recognizes, a `roots.config.json` to add, a
/// publication file the analysis does not read.
const ACTION_KINDS: &[&str] = &[
    "unrecognized-guard",
    "publication-unknown",
    "unread-publication-file",
];

/// ` at <file>:<line>` of a blocker's anchor (1-based), or nothing.
fn blocker_place(b: &ProveBlocker) -> String {
    b.anchor
        .as_ref()
        .and_then(|x| Some(format!(" at {}:{}", x.file.as_ref()?, x.line? + 1)))
        .unwrap_or_default()
}

/// `to narrow it:` (round 2 G20, r1 §7.5): what the user can change for
/// each blocker kind that carries an action, before the other blockers — a
/// guard the analysis does not recognize (where), a `roots.config.json` to
/// add, a publication file it does not read. `None` with no such blocker.
fn narrow_line(a: &CallersAnswer) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for key in ACTION_KINDS {
        let Some(&n) = a.blocked_counts.get(key) else {
            continue;
        };
        let first = a.blocked_by.iter().find(|b| b.kind == *key);
        let place = first.map(blocker_place).unwrap_or_default();
        parts.push(match *key {
            "unrecognized-guard" if n == 1 => format!("the guard{place} is not recognized"),
            "unrecognized-guard" => {
                format!("{n} guards are not recognized (the first{place})")
            }
            "publication-unknown" => {
                "a roots.config.json would name the background targets (job-queue entries and \
                 published web services live in data, not in source)"
                    .to_string()
            }
            _ => first.map_or_else(
                || crate::engine::vocab::blocker(key).to_string(),
                |b| b.detail.clone(),
            ),
        });
    }
    (!parts.is_empty()).then(|| format!("  to narrow it: {}", parts.join("; ")))
}

/// `blocked by:` on one line (§7.5, F45): the kinds that carry an action
/// first, each with its first example (what to change and where), then
/// every other kind's count, largest first. `detail` lists examples of each
/// kind on the lines below.
fn blocked_by_line(
    a: &CallersAnswer,
    kinds: &[(String, usize)],
    detail: bool,
    hcx: &HumanCx<'_>,
) -> String {
    let action_words: Vec<String> = ACTION_KINDS
        .iter()
        .map(|k| crate::engine::vocab::blocker(k))
        .collect();
    let mut parts: Vec<String> = Vec::new();
    for key in ACTION_KINDS {
        let Some(n) = a.blocked_counts.get(key) else {
            continue;
        };
        // Round-2 G25: an `unknown`'s `to narrow it:` line already names
        // each action kind's example; it is not printed twice.
        let narrowed = a.answer == ProveAnswer::Unknown;
        let example = a
            .blocked_by
            .iter()
            .find(|b| b.kind == *key)
            .filter(|_| !narrowed)
            .map(|b| format!(" ({}{})", b.detail, blocker_place(b)))
            .unwrap_or_default();
        parts.push(format!(
            "{} {n}{example}",
            crate::engine::vocab::blocker(key)
        ));
    }
    parts.extend(
        kinds
            .iter()
            .filter(|(k, _)| !action_words.contains(k))
            .map(|(k, n)| format!("{k} {n}")),
    );
    let more = if detail {
        String::new()
    } else {
        format!("; {} lists examples of each kind", hcx.detail())
    };
    format!("  blocked by: {}{more}", parts.join(", "))
}

/// A stop reason in words (the JSON keeps its key).
fn stop_words(key: &str) -> &str {
    match key {
        "commit" => "a COMMIT",
        "checked-run" => "a checked Codeunit.Run",
        "checked-run-trap" => "a checked Codeunit.Run (its own transaction)",
        "background-start" => "a background start (a new session)",
        "try-function" => "a TryFunction",
        "asserterror" => "asserterror",
        "gui-allowed" => "if GuiAllowed",
        "page-event" => "a page's own trigger or action (a later user event)",
        k => k,
    }
}

/// The `re-analysis under an assumption:` line of an `unknown` (§4.4): every
/// input that removes a blocker kind, with what it removes and what stays.
/// For the callers' context questions the one input is
/// `assumeNoDependents` (it removes `external callers`). `None` when no
/// input removes a kind of this answer.
fn reanalysis_line(a: &CallersAnswer, detail: bool, hcx: &HumanCx<'_>) -> Option<String> {
    let call = reanalysis_follow_up(a, hcx)?;
    let external = a.blocked_counts.get("external-callers").copied()?;
    let who = match external {
        1 => "this routine".to_string(),
        2..=5 => format!("these {external} routines"),
        _ => format!("the {external} routines other apps may call"),
    };
    let stay: Vec<(String, usize)> = blocker_counts(a)
        .into_iter()
        .filter(|(k, _)| k != "external callers")
        .collect();
    let removes = if stay.is_empty() {
        format!("removes every blocker of this answer (external callers {external})")
    } else {
        format!(
            "removes external callers {external}; stay: {}",
            blockers_named(&stay, 3, detail, hcx)
        )
    };
    // §6.1: at `--surface=mcp` the complete call; on the CLI the flag.
    let how = match hcx.fcx.surface {
        crate::engine::follow_up::Surface::Mcp => call.text(hcx.fcx.surface),
        crate::engine::follow_up::Surface::Cli => {
            crate::engine::follow_up::Input::AssumeNoDependents
                .cli()
                .to_string()
        }
    };
    Some(format!(
        "  re-analysis under an assumption: {how} — only if no other app calls {who}; {removes}"
    ))
}

/// The `re-analysis under an assumption:` call of an `unknown` (§6.1): the
/// same question with `assumeNoDependents: true`, a conditional follow-up.
/// `None` when no input removes a kind of this answer.
#[must_use]
pub fn reanalysis_follow_up(
    a: &CallersAnswer,
    hcx: &HumanCx<'_>,
) -> Option<crate::engine::follow_up::FollowUp> {
    let external = a.blocked_counts.get("external-callers").copied()?;
    if a.assume_no_dependents || a.answer != ProveAnswer::Unknown {
        return None;
    }
    Some(
        crate::engine::follow_up::FollowUp::prove(
            hcx.fcx,
            hcx.selector,
            a.question.id(),
            &format!(
                "removes the external callers blockers ({external}), only if no other app calls \
                 them"
            ),
        )
        .arg("assumeNoDependents", true)
        .conditional(),
    )
}

/// The `plain reading:` line of an `unknown` (§4.4): what the analyzed
/// callers show, and why the answer is still unknown.
fn plain_reading(a: &CallersAnswer) -> String {
    let found = match (a.question, a.witness_count > 0) {
        (CallersQuestion::NonInteractive, true) => {
            "every non-interactive route found passes a hub, which may lead anywhere"
        }
        (CallersQuestion::PendingWrites, true) => {
            "every pending write found reaches it over a hub, which may lead anywhere"
        }
        (CallersQuestion::InterceptErrors, true) => {
            "every interception found reaches it over a hub, which may lead anywhere"
        }
        (CallersQuestion::NonInteractive, false) => {
            "no non-interactive entry was found in the analyzed callers"
        }
        (CallersQuestion::PendingWrites, false) => {
            "no pending write was found in the analyzed callers"
        }
        (CallersQuestion::InterceptErrors, false) => {
            "no interception was found in the analyzed callers"
        }
    };
    let why: Vec<String> = blocker_counts(a)
        .into_iter()
        .take(3)
        .map(|(k, n)| match k.as_str() {
            "external callers" => {
                format!("{} other apps may call", plural(n, "routine", "routines"))
            }
            // Round-3 c02: a noun phrase after "because of".
            "over a hub" => format!(
                "{} that {} it only over a hub",
                plural(n, "caller", "callers"),
                if n == 1 { "reaches" } else { "reach" }
            ),
            _ => format!("{k} {n}"),
        })
        .collect();
    format!(
        "  plain reading: {found}; unknown because of {}",
        why.join(", ")
    )
}

/// explain's "callers' context" section for one card (agent-tasks §6.6): the
/// three answers' one-line forms, and their JSON (`callersContext`). `None`
/// when the routine is not in the sited graph.
#[must_use]
pub fn section(cx: &CallersCx<'_>, root: &str, hcx: &HumanCx<'_>) -> Option<CallersSection> {
    let nodes = cx.g.nodes_with_stable_id(root);
    if nodes.is_empty() {
        return None;
    }
    // Output validation V2 (§5.1, F29): the section leads the card, and each
    // answer names its evidence: a `yes` its first witness (tier, what and
    // where, its path), an `unknown` every blocker kind and the input that
    // removes one. Round-2 G24: an answer that found nothing says among
    // which population (the direct callers the walk analyzed), and the
    // three answers' short forms are the card's `answer:` line.
    // r3 H7: a lock edit's answer leads with its limit: no lock facts are
    // kept for the callers (round-2 G24, G48), so the block's first line
    // says their held locks are not analyzed.
    let mut lines = vec![
        "for adding a COMMIT, an error or UI here (callers' context):".to_string(),
        "  locks held by callers: not analyzed (no lock facts)".to_string(),
    ];
    let mut short: Vec<String> = Vec::new();
    let mut json = serde_json::Map::new();
    for q in [
        CallersQuestion::PendingWrites,
        CallersQuestion::InterceptErrors,
        CallersQuestion::NonInteractive,
    ] {
        let a = answer(q, &nodes, cx);
        short.push(short_answer(&a));
        let first = a.witnesses.iter().find(|w| w.tier != Tier::OnlyOpen);
        if a.answer == ProveAnswer::Yes
            && let Some(w) = first
        {
            let what = match (&w.words, &w.file, w.line) {
                (Some(words), _, _) => words.clone(),
                (None, Some(f), Some(l)) => format!("{} at {f}:{l}", w.detail),
                _ => w.detail.clone(),
            };
            lines.push(format!(
                "  {}; first [{}]: {what}",
                summary_line(&a),
                crate::engine::vocab::tier(w.tier)
            ));
            lines.extend(crate::engine::path_segments::elide(vec![format!(
                "    path: {}",
                path_text(&w.path)
            )]));
        } else if a.answer == ProveAnswer::Unknown {
            lines.push(format!("  {}", found_among_line(&a)));
        } else {
            lines.push(format!("  {}", summary_line(&a)));
        }
        if a.answer == ProveAnswer::Unknown
            && let Some(l) = reanalysis_line(&a, false, hcx)
        {
            lines.push(format!("    {}", l.trim_start()));
        }
        let mut follow_ups: Vec<serde_json::Value> = Vec::new();
        if let Some(f) = reanalysis_follow_up(&a, hcx) {
            follow_ups.push(f.json());
        }
        json.insert(
            q.id().into(),
            serde_json::json!({
                "answer": a.answer.as_str(),
                "evidenceCount": a.witness_count,
                "entries": a.entries,
                "blockedByCounts": a.blocked_counts,
                "followUps": follow_ups,
                "directCallersAnalyzed": a.direct_callers,
            }),
        );
    }
    // Round-2 G24 (G48's output limitation): no lock facts are kept for the
    // callers, so their held locks are not analyzed (the first line above;
    // the `answer:` summary says so too).
    short.push("caller-held locks and their order: not analyzed".to_string());
    json.insert(
        "locksHeldByCallers".into(),
        serde_json::json!({"analyzed": false, "reason": "no lock facts"}),
    );
    // §6.1: one complete call; the other two questions take the same form.
    let ask = crate::engine::follow_up::FollowUp::prove(
        hcx.fcx,
        hcx.selector,
        CallersQuestion::PendingWrites.id(),
        "witnesses and blockers",
    );
    lines.push(format!(
        "  ask: {} (or question {} | {}) for witnesses and blockers",
        ask.text(hcx.fcx.surface),
        CallersQuestion::InterceptErrors.id(),
        CallersQuestion::NonInteractive.id()
    ));
    Some(CallersSection {
        json: serde_json::Value::Object(json),
        lines,
        answer: short.join("; "),
    })
}

/// explain's callers' context section for one card (round-2 G24).
pub struct CallersSection {
    /// JSON `callersContext`.
    pub json: serde_json::Value,
    /// The section's human lines.
    pub lines: Vec<String>,
    /// The three answers' short forms, the card's `answer:` line
    /// (`pending writes: yes (8 witnesses); intercepted errors: unknown; …`).
    pub answer: String,
}

/// One answer's short form for the `answer:` line (round-2 G24).
fn short_answer(a: &CallersAnswer) -> String {
    let label = match a.question {
        CallersQuestion::PendingWrites => "pending writes",
        CallersQuestion::InterceptErrors => "intercepted errors",
        CallersQuestion::NonInteractive => "non-interactive",
    };
    match a.answer {
        ProveAnswer::Yes => format!(
            "{label}: yes ({})",
            plural(a.witness_count, "witness", "witnesses")
        ),
        ProveAnswer::No => format!("{label}: no"),
        ProveAnswer::Unknown => format!("{label}: unknown"),
    }
}

/// An `unknown`'s line (round-2 G24): what was found among the computed
/// population (the direct callers the walk analyzed), then why the answer
/// is unknown in plain words.
fn found_among_line(a: &CallersAnswer) -> String {
    let label = match a.question {
        CallersQuestion::PendingWrites => "pending writes",
        CallersQuestion::InterceptErrors => "intercepted errors",
        CallersQuestion::NonInteractive => "non-interactive",
    };
    let among = plural(
        a.direct_callers,
        "analyzed direct caller",
        "analyzed direct callers",
    );
    let found = if a.open_witness_count > 0 {
        format!(
            "none established among {among} ({} over a hub)",
            plural(a.open_witness_count, "candidate", "candidates")
        )
    } else {
        format!("none found among {among}")
    };
    let why: Vec<String> = blocker_counts(a)
        .into_iter()
        .map(|(k, n)| match k.as_str() {
            "external callers" => {
                format!("{} other apps may call", plural(n, "routine", "routines"))
            }
            // Round-3 c02: a noun phrase after "because of".
            "over a hub" => format!(
                "{} that {} it only over a hub",
                plural(n, "caller", "callers"),
                if n == 1 { "reaches" } else { "reach" }
            ),
            _ => format!("{k} {n}"),
        })
        .collect();
    format!(
        "{label}: unknown — {found}; unknown because of {}",
        why.join(", ")
    )
}

/// prove's human answer for a callers' context question.
#[must_use]
pub fn format_human(header: &str, a: &CallersAnswer, detail: bool, hcx: &HumanCx<'_>) -> String {
    let q = a.question;
    let mut lines = vec![
        format!("--- {header} ---"),
        format!(
            "  question: {} — callers' context (not about the routine's own effects)",
            q.id()
        ),
    ];
    let candidates = a.open_witness_count;
    let established = a.witness_count - candidates;
    let kinds = blocker_counts(a);
    lines.push(match a.answer {
        ProveAnswer::Yes => format!("  answer:   yes — {}", q.yes_text()),
        ProveAnswer::No => format!("  answer:   {NO_WORDS} — {}", q.no_text()),
        // §3.1 (F10): only candidates over a hub were found; the answer
        // names the hub kinds on their paths, never presents them as
        // evidence.
        ProveAnswer::Unknown if candidates > 0 => {
            let hubs = candidate_hubs(a, 3);
            format!(
                "  answer:   unknown — no {} without a hub was established in the analyzed graph; \
                 {} a hub{}",
                population(q),
                plural(candidates, "candidate passes", "candidates pass"),
                if hubs.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", hubs.join(", "))
                }
            )
        }
        ProveAnswer::Unknown => format!(
            "  answer:   unknown (blocked by: {}) — no {} was found in the analyzed callers",
            blockers_named(&kinds, 3, false, hcx),
            population(q)
        ),
    });
    // §4.4 (F19-F21, F79): on `unknown`, the plain reading, the conclusion
    // the answer does not support, and the input that removes a blocker.
    if a.answer == ProveAnswer::Unknown {
        lines.push(plain_reading(a));
        // Round 2 G20: the blockers that carry an action, first.
        if let Some(l) = narrow_line(a) {
            lines.push(l);
        }
        lines.push(format!("  not established: {}", not_established(q)));
        if let Some(l) = reanalysis_line(a, detail, hcx) {
            lines.push(l);
        }
    }
    let shown_max = if detail { usize::MAX } else { 3 };
    let witness_line = |w: &Witness| -> String {
        let tier = crate::engine::vocab::tier(w.tier);
        let hops = plural(w.hops as usize, "hop", "hops");
        match (w.entry, &w.words) {
            (Some(k), Some(words)) => {
                let (kind, ctx) = crate::engine::vocab::entry_key(k);
                let ctx = ctx.map(|c| format!(" [context: {c}]")).unwrap_or_default();
                format!("    [{tier}] {kind}{ctx}: {words} ({hops})")
            }
            // r3 H4: a pending write inside a call names its own words (the
            // write's site, the call it is inside, both call locations).
            (None, Some(words)) => format!("    [{tier}] {}: {words} ({hops})", w.kind),
            _ => {
                let place = match (&w.file, w.line) {
                    (Some(f), Some(l)) => format!(" at {f}:{l}"),
                    _ => String::new(),
                };
                let dep = if w.caller_dependent {
                    " (only when the caller passes a physical record)"
                } else {
                    ""
                };
                format!("    [{tier}] {}: {}{place}{dep} ({hops})", w.kind, w.detail)
            }
        }
    };
    // §3.1: established witnesses first (or `none`), then the candidates,
    // each labelled; every witness line starts with its tier.
    let witnesses_start = lines.len();
    if a.witness_count > 0 {
        let firm: Vec<&Witness> = a
            .witnesses
            .iter()
            .filter(|w| w.tier != Tier::OnlyOpen)
            .collect();
        let open: Vec<&Witness> = a
            .witnesses
            .iter()
            .filter(|w| w.tier == Tier::OnlyOpen)
            .collect();
        if established == 0 {
            lines.push("  established witnesses: none".into());
        } else {
            let by = entry_words_line(a, false);
            lines.push(format!(
                "  established witnesses: {} (nearest first){}",
                established,
                if by.is_empty() || q != CallersQuestion::NonInteractive {
                    String::new()
                } else {
                    format!("; by context (contexts may overlap): {by}")
                }
            ));
            let shown = firm.len().min(shown_max);
            for w in &firm[..shown] {
                lines.push(witness_line(w));
                lines.push(format!("      path: {}", path_text(&w.path)));
            }
            if established > shown {
                lines.push(format!(
                    "    … {} more established ({})",
                    established - shown,
                    more_words(firm.len() - shown, established - firm.len(), hcx.detail())
                ));
            }
        }
        if candidates > 0 {
            let by = entry_words_line(a, true);
            lines.push(format!(
                "  candidates over a hub: {candidates}{}",
                if by.is_empty() || q != CallersQuestion::NonInteractive {
                    String::new()
                } else {
                    format!("; by context (contexts may overlap): {by}")
                }
            ));
            // Round-2 G33: one candidate by default (the counts by context
            // are on the line above); `detail` lists every kept one.
            let shown = open.len().min(if detail { usize::MAX } else { 1 });
            for w in &open[..shown] {
                lines.push(witness_line(w));
                lines.push(format!("      path: {}", path_text(&w.path)));
            }
            if candidates > shown {
                // Round-3 c02: say what `detail` adds, the kept candidates
                // (at most OPEN_WITNESSES), and what no option lists.
                lines.push(format!(
                    "    … {} more candidates ({})",
                    candidates - shown,
                    more_words(open.len() - shown, candidates - open.len(), hcx.detail())
                ));
            }
        }
    }
    // §7.5 (F45): a run of hops several witnesses share prints once, under
    // a stable segment id, listed after the witnesses.
    // Round-2 G33: then the repeated object and file elided on consecutive
    // hops.
    let witnesses = lines.split_off(witnesses_start);
    lines.extend(crate::engine::path_segments::elide(
        crate::engine::path_segments::segment(witnesses),
    ));
    // The entries reached: for non-interactive they are the witnesses'
    // entries (printed on the witness lines above); for the other two they
    // are not the witnesses (traps, pending writes) and print here.
    if (q != CallersQuestion::NonInteractive || a.witness_count == 0) && !a.entry_words.is_empty() {
        let label = match q {
            CallersQuestion::PendingWrites => "entries reached with a write pending",
            CallersQuestion::InterceptErrors => "uncaught at (entries reached with no trap)",
            CallersQuestion::NonInteractive => "non-interactive entries",
        };
        // §8.2 (F53): the unit, and how the kinds relate.
        let unit = if q == CallersQuestion::NonInteractive {
            " (distinct entries per context; contexts may overlap, not summed)"
        } else {
            " (distinct entries; an entry is counted once, in its own kind)"
        };
        for (over_hub, tag) in [(false, ""), (true, " [via-hub]")] {
            let by = entry_words_line(a, over_hub);
            if !by.is_empty() {
                lines.push(format!("  {label}{tag}: {by}{unit}"));
            }
        }
    }
    if detail
        && q == CallersQuestion::InterceptErrors
        && let Some((tier, hops, path)) = &a.uncaught_route
    {
        lines.push(format!(
            "  one uncaught route [{}] ({}): {}",
            crate::engine::vocab::tier(*tier),
            plural(*hops as usize, "hop", "hops"),
            path_text(path)
        ));
    }
    if !a.stopped.is_empty() {
        lines.push(format!(
            "  stopped: {}",
            a.stopped
                .iter()
                .map(|(k, n)| format!("{} at {}", plural(*n, "path", "paths"), stop_words(k)))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    match q {
        CallersQuestion::PendingWrites => {
            if a.commit_behavior.is_empty() {
                lines.push("  commit behavior: none on the paths".into());
            } else {
                for c in &a.commit_behavior {
                    lines.push(format!("  commit behavior: {c}"));
                }
            }
            if !a.try_function_callers.is_empty() {
                lines.push(format!(
                    "  TryFunction callers: {} ({}) — a COMMIT under a try function is not \
                     modelled",
                    a.try_function_callers.len(),
                    a.try_function_callers.join(", ")
                ));
            }
        }
        CallersQuestion::InterceptErrors => {
            for n in &a.not_intercepted {
                lines.push(format!("  not intercepted: {n}"));
            }
        }
        CallersQuestion::NonInteractive => {
            if a.test_entries > 0 {
                lines.push(format!(
                    "  test entries: {} (UI needs a handler in each; not counted as witnesses)",
                    a.test_entries
                ));
            }
        }
    }
    if !a.blocked_by.is_empty() {
        lines.push(blocked_by_line(a, &kinds, detail, hcx));
        if detail {
            let mut by: BTreeMap<&str, usize> = BTreeMap::new();
            for b in &a.blocked_by {
                let n = by.entry(b.kind).or_default();
                *n += 1;
                if *n > BLOCKER_SAMPLES {
                    continue;
                }
                lines.push(format!(
                    "    [{}] {}{}",
                    crate::engine::vocab::blocker(b.kind),
                    b.detail,
                    blocker_place(b)
                ));
            }
        }
    }
    // §7.5 (F83): the traversal statistic (states over nodes) is in the
    // JSON (`states`, `nodes`), not in the human text.
    lines.push(format!(
        "  assumptions: {}",
        if a.assume_no_dependents {
            "assumeNoDependents (no app outside the program calls it)"
        } else {
            "none (apps that depend on this app are not loaded; their calls block a no)"
        }
    ));
    lines.join("\n") + "\n"
}

#[cfg(test)]
#[path = "callers_context_tests.rs"]
mod tests;
