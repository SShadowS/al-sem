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
//! not); otherwise it propagates. **A TryFunction does not roll back the
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
            CallersQuestion::PendingWrites => {
                "no caller has a database write pending when it runs (whole cone seen)"
            }
            CallersQuestion::InterceptErrors => {
                "every path propagates an error to its entry (whole cone seen)"
            }
            CallersQuestion::NonInteractive => {
                "every entry that reaches it is interactive (whole cone seen)"
            }
        }
    }
}

/// One witness: the decisive site and how it reaches R.
#[derive(Clone, Debug)]
pub struct Witness {
    /// `pending-write`, `inside-write`, `catch-and-return-false`,
    /// `asserterror`, `collectible`, `non-interactive-entry`.
    pub kind: &'static str,
    /// What it is, in words (`Modify on "Customer"`, `checked Codeunit.Run`).
    pub detail: String,
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
}

impl<'a> Index<'a> {
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
        match self.objects.get(id) {
            Some(o) => format!("{} \"{}\"", o.object_type, o.name),
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

    fn display(&self, g: &SitedGraph, n: NodeIx) -> String {
        match self.rows_of(g, n).first() {
            Some(r) => routine_display(r, self.objects.get(r.object_id.as_str()).copied()),
            None => g.display(n),
        }
    }
}

fn routine_display(r: &ModelRoutine, o: Option<&ModelObject>) -> String {
    let quote = |s: &str| {
        if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            s.to_string()
        } else {
            format!("\"{s}\"")
        }
    };
    match o {
        Some(o) => format!("{} {}::{}", o.object_type, quote(&o.name), r.name),
        None => format!("{}::{}", r.object_id, r.name),
    }
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
    witness_count: usize,
    open_witness_count: usize,
    blockers: BTreeMap<&'static str, Vec<ProveBlocker>>,
    blocker_keys: HashSet<(&'static str, String)>,
    blocked_counts: BTreeMap<&'static str, usize>,
    entries: BTreeMap<&'static str, BTreeSet<NodeIx>>,
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
        }];
        path.extend(self.path_of(from));
        let w = Witness {
            kind,
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
                        .find(|cs| {
                            fwd_own.iter().any(|(s, callee)| {
                                s.as_deref() == Some(cs.id.as_str())
                                    && self
                                        .cx
                                        .cone_derived
                                        .physical_write_ids_of(callee)
                                        .next()
                                        .is_some()
                            })
                        });
                    match via {
                        Some(cs) => (
                            format!(
                                "{} may write {} before the call",
                                cs.callee_text,
                                self.ix.table(t)
                            ),
                            Some((
                                cs.source_anchor.source_unit_id.clone(),
                                cs.source_anchor.start_line + 1,
                            )),
                        ),
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
        if st.boundary == Boundary::EventDispatch {
            // A subscriber's error reaches the publisher's caller.
            return SiteEval::default();
        }
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
                format!("{d} reaches it only through an open site (a hub)"),
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

    fn entry(&mut self, _root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, _hops: u32) {
        let pending = s.ext & PENDING != 0;
        for k in kinds {
            match self.q {
                CallersQuestion::PendingWrites => {
                    if pending {
                        self.entries.entry(k.as_str()).or_default().insert(n);
                    }
                }
                CallersQuestion::InterceptErrors => {
                    self.entries.entry(k.as_str()).or_default().insert(n);
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
        let w = Witness {
            kind: "non-interactive-entry",
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
        witness_count: 0,
        open_witness_count: 0,
        blockers: BTreeMap::new(),
        blocker_keys: HashSet::new(),
        blocked_counts: BTreeMap::new(),
        entries: BTreeMap::new(),
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
    // A collectible-only trap after the ones that catch every error.
    witnesses.sort_by_key(|w| (w.tier, w.hops, w.kind == "collectible"));
    let mut open = v.open_witnesses;
    open.sort_by_key(|w| w.hops);
    witnesses.extend(open);
    CallersAnswer {
        question: q,
        answer,
        witnesses,
        witness_count,
        open_witness_count: v.open_witness_count,
        blocked_by,
        blocked_counts: v.blocked_counts,
        entries: v.entries.into_iter().map(|(k, s)| (k, s.len())).collect(),
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
    json!({
        "answer": a.answer.as_str(),
        "evidence": evidence,
        "evidenceCount": a.witness_count,
        "evidenceThroughOpenSites": a.open_witness_count,
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
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn counts_line(m: &BTreeMap<&'static str, usize>) -> String {
    m.iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
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
            s.push_str(&format!(
                " — {}",
                plural(a.witness_count, "witness", "witnesses")
            ));
            if !a.entries.is_empty() {
                s.push_str(&format!(" ({})", counts_line(&a.entries)));
            }
        }
        ProveAnswer::Unknown => {
            let kinds: Vec<String> = a
                .blocked_counts
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            s.push_str(&format!(" — blocked: {}", kinds.join(", ")));
        }
        ProveAnswer::No => {}
    }
    s
}

/// explain's "callers' context" section for one card (agent-tasks §6.6): the
/// three answers' one-line forms, and their JSON (`callersContext`). `None`
/// when the routine is not in the sited graph.
#[must_use]
pub fn section(cx: &CallersCx<'_>, root: &str) -> Option<(serde_json::Value, Vec<String>)> {
    let nodes = cx.g.nodes_with_stable_id(root);
    if nodes.is_empty() {
        return None;
    }
    let mut lines = vec![
        "callers' context (for adding effects; the sections above are existing effects):"
            .to_string(),
    ];
    let mut json = serde_json::Map::new();
    for q in [
        CallersQuestion::PendingWrites,
        CallersQuestion::InterceptErrors,
        CallersQuestion::NonInteractive,
    ] {
        let a = answer(q, &nodes, cx);
        lines.push(format!("  {}", summary_line(&a)));
        json.insert(
            q.id().into(),
            serde_json::json!({
                "answer": a.answer.as_str(),
                "evidenceCount": a.witness_count,
                "entries": a.entries,
                "blockedByCounts": a.blocked_counts,
            }),
        );
    }
    lines.push(
        "  ask: alsem_prove <routine> callers-have-pending-writes | callers-intercept-errors | \
         callers-may-run-noninteractive (witnesses and blockers)"
            .to_string(),
    );
    Some((serde_json::Value::Object(json), lines))
}

/// prove's human answer for a callers' context question.
#[must_use]
pub fn format_human(header: &str, a: &CallersAnswer, detail: bool) -> String {
    let q = a.question;
    let mut lines = vec![
        format!("--- {header} ---"),
        format!(
            "  question: {} — callers' context (not about the routine's own effects)",
            q.id()
        ),
    ];
    lines.push(match a.answer {
        ProveAnswer::Yes => format!("  answer:   yes — {}", q.yes_text()),
        ProveAnswer::No => format!("  answer:   no — {}", q.no_text()),
        ProveAnswer::Unknown if a.witness_count > 0 => {
            "  answer:   unknown — witnesses only through open sites (a hub stands for the call), \
             and the cone is not complete"
                .to_string()
        }
        ProveAnswer::Unknown => {
            "  answer:   unknown — no witness found, but the cone is not complete".to_string()
        }
    });
    if a.witness_count > 0 {
        let shown = if detail {
            a.witnesses.len()
        } else {
            a.witnesses.len().min(3)
        };
        lines.push(format!(
            "  evidence: {} (nearest first)",
            plural(a.witness_count, "witness", "witnesses")
        ));
        for w in &a.witnesses[..shown] {
            let place = match (&w.file, w.line) {
                (Some(f), Some(l)) => format!(" at {f}:{l}"),
                _ => String::new(),
            };
            let dep = if w.caller_dependent {
                " (only when the caller passes a physical record)"
            } else {
                ""
            };
            lines.push(format!(
                "    [{}] {}{place}{dep} ({}, {})",
                w.kind,
                w.detail,
                plural(w.hops as usize, "hop", "hops"),
                w.tier.as_str()
            ));
            let path: Vec<String> = w
                .path
                .iter()
                .map(|h| match (&h.file, h.line) {
                    (Some(f), Some(l)) => format!("{} ({f}:{l})", h.display),
                    _ => h.display.clone(),
                })
                .collect();
            lines.push(format!("      path: {}", path.join(" → ")));
        }
        if a.witness_count > shown {
            lines.push(format!("    … {} more", a.witness_count - shown));
        }
    }
    if !a.entries.is_empty() {
        let label = match q {
            CallersQuestion::PendingWrites => "entries reached with a write pending",
            CallersQuestion::InterceptErrors => "uncaught at (entries reached with no trap)",
            CallersQuestion::NonInteractive => "non-interactive entries",
        };
        lines.push(format!("  {label}: {}", counts_line(&a.entries)));
    }
    if !a.stopped.is_empty() {
        lines.push(format!(
            "  stopped: {}",
            a.stopped
                .iter()
                .map(|(k, n)| format!("{} at {k}", plural(*n, "path", "paths")))
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
        lines.push(format!(
            "  blocked by: {}",
            a.blocked_counts
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let per_kind = if detail { BLOCKER_SAMPLES } else { 2 };
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for b in &a.blocked_by {
            let n = by.entry(b.kind).or_default();
            *n += 1;
            if *n > per_kind {
                continue;
            }
            let place = b
                .anchor
                .as_ref()
                .and_then(|x| Some(format!(" at {}:{}", x.file.as_ref()?, x.line? + 1)))
                .unwrap_or_default();
            lines.push(format!("    [{}] {}{place}", b.kind, b.detail));
        }
    }
    lines.push(format!(
        "  coverage: the reverse cone, {} over {}",
        plural(a.states, "state", "states"),
        plural(a.nodes, "node", "nodes")
    ));
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
