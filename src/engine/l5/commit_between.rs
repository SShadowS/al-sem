//! ENG-165: `alsem prove <routine> commit-between --after A --before B`
//! (design `docs/superpowers/specs/2026-11-25-eng165-commit-between-design.md`).
//!
//! Can a COMMIT run after site A has returned and before the next run of
//! site B, on a path of the root? A send (A) whose log entry (B) an error can
//! lose once a commit made the send durable is the question's origin.
//!
//! **Where it looks** (design §4.1). A route to A and a route to B, both from
//! the root's state graph ([`LiteralReach::routes`]), share a prefix; its last
//! routine is the MEET routine M. The between region is A's tail (in each
//! routine on A's route below M, what may run after the call toward A), M's
//! middle (what may run after the site toward A and before the site toward
//! B) and B's head (in each routine on B's route below M, what may run before
//! the call toward B). Every route pair is checked (up to [`ROUTE_CAP`] routes
//! per site); an enumeration that stops at its cap makes the answer unknown.
//!
//! **Order inside a routine** (§4.2) is read off a small control-flow graph
//! built from the routine's control-flow tree ([`Cfg`]): blocks, both arms of
//! an `if`, one branch of a `case`, loops with their back edge, `exit`,
//! `break` (out of the loop), `continue` (to the loop's next pass), an
//! `Error` statement (the path ends). Calls in a condition or an argument run
//! before the statement's own call (measured, T11). The path from A stops at
//! B only where B surely runs from there ([`Cut`]); elsewhere it goes on to a
//! later B (a commit reached over a back edge is `loop-carried`).
//!
//! **What a region statement contributes**: its own commit and error sites,
//! and the cones of the edges leaving it (calls, event raises, a record
//! operation's triggers and table events), walked over the snapshot's graph.
//!
//! **Classes** (§4.3): a `Commit()` that runs normally ends the caller's
//! transaction (class 1, decides `yes`). A checked run's own commit, and any
//! commit whose route crosses a checked-run edge, commit only the run's own
//! transaction (class 2, `ownTransactionCommits`). A `Commit()` inside an
//! isolated subscriber is class 1 (measured, T14/T16). When A's own route
//! crosses a checked run (or an isolated event) below M, that run's commit at
//! its successful return makes A durable: a class-1 `run-commit-of-A`.
//! `[CommitBehavior]` reads as in the `--site` slice: a commit that runs only
//! under `Ignore` is none, one some route runs under `Error` is an error site.
//!
//! **Errors and traps** (§5.2): an error in a region cone behind a definite
//! trap (checked run, used `[TryFunction]`, `asserterror`) is caught there and
//! control goes on toward B: left out. An isolated event may fail to trap with
//! a write pending: the row stays (`trap-may-fail-pending`). A trap on A's
//! route below M catches an error in A's tail below it (left out); a trap on
//! the shared prefix above M swallows an error after A is committed and B is
//! skipped: the row stays, `swallowed-above-meet`.
//!
//! shortcut: a commit's listed route is its region statement's route plus
//! the cone path, and the commit is kept when its operation is live from the
//! root under literal arguments; the route through that one statement is not
//! itself checked against the literals (a possible over-listing, never a
//! dropped commit). Upgrade with a state-graph route through the statement.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::group_path::GroupPaths;
use crate::engine::l5::literal_reach::routes::ROUTE_CAP;
use crate::engine::l5::literal_reach::{EdgeSite, LiteralReach, LiveTree, Step};
use crate::engine::l5::prove_slice::{
    ROUTE_EDGE_BUDGET, SliceRoute, anchor_in, route_sites, route_value, site_facts,
};
use crate::engine::l5::snapshot::CapabilitySnapshot;
use crate::engine::l5::terminal_facts::TerminalRecord;
use crate::engine::l5::traps::TrapKind;
use crate::program::body::features::{PAnchor, PCFNNode, PathHint};
use crate::program::model::workspace::{Model, ModelRoutine};

/// The most routine states one region statement's cone walk visits; past it
/// the answer is unknown (`cone-walk-capped`).
pub const CONE_STATE_CAP: usize = 200_000;

/// How many rows of each list the human answer prints (`--detail`: all).
const HUMAN_ROWS: usize = 5;

// ---------------------------------------------------------------------------
// The question
// ---------------------------------------------------------------------------

/// `--after` / `--before`: a source file (a path or its tail), a 1-based line
/// and an optional 1-based column (UTF-16 code units, as an editor shows it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePoint {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
}

impl SitePoint {
    /// Parse `file:line` or `file:line:col` (design §3): when the last two
    /// `:`-segments are both numbers it is `file:line:col`, when only the last
    /// is, `file:line`. A drive letter (`C:/…`) never ends in two numbers.
    pub fn parse(flag: &str, text: &str) -> Result<SitePoint, String> {
        let bad = || {
            format!(
                "prove: {flag} '{text}' is not <file>:<line>[:<col>] (a source file path or \
                 its tail, a 1-based line, an optional 1-based column)"
            )
        };
        let num = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n > 0);
        let (head, last) = text.rsplit_once(':').ok_or_else(bad)?;
        let last = num(last).ok_or_else(bad)?;
        let (file, line, column) = match head.rsplit_once(':') {
            Some((file, mid)) if num(mid).is_some() => (file, num(mid).unwrap(), Some(last)),
            _ => (head, last, None),
        };
        let file = file.trim().replace('\\', "/");
        if file.is_empty() {
            return Err(bad());
        }
        Ok(SitePoint { file, line, column })
    }

    fn label(&self) -> String {
        match self.column {
            Some(c) => format!("{}:{}:{c}", self.file, self.line),
            None => format!("{}:{}", self.file, self.line),
        }
    }

    /// Whether `file` (an anchor's unit) is this point's file: equal, or ends
    /// in it at a `/` or `:` boundary, ASCII case-insensitive (as `--site`).
    fn file_matches(&self, file: &str) -> bool {
        let f = file.replace('\\', "/").to_ascii_lowercase();
        let want = self.file.to_ascii_lowercase();
        f == want
            || f.strip_suffix(&want).is_some_and(|head| {
                head.ends_with('/') || head.ends_with(':') || want.starts_with('/')
            })
    }
}

/// The two sites of one `commit-between` question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BetweenSites {
    pub after: SitePoint,
    pub before: SitePoint,
    /// Test knobs (default: off); the CLI never sets them.
    pub(crate) knobs: Knobs,
}

/// Knobs a test sets to reach a guard whose real precondition needs a huge
/// graph (a capped walk or search, a truncated cone) or a routine without a
/// tree: each sets the guard's INPUT by assignment (review r3 item 4), so the
/// guard's production condition is what the test pins. Production leaves
/// every one off; nothing outside the crate can set them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Knobs {
    /// The cone walk's state cap (default [`CONE_STATE_CAP`]).
    pub(crate) cone_cap: Option<usize>,
    /// The route enumeration's edge budget per site (default
    /// `ROUTE_EDGE_BUDGET`).
    pub(crate) route_budget: Option<usize>,
    /// The root's guarded search stopped at its cap (drops the live tree).
    pub(crate) search_stopped: bool,
    /// The root's cone was truncated (sets prove's obligation).
    pub(crate) cone_truncated: bool,
    /// The root's cone has analysis gaps (sets prove's obligation).
    pub(crate) analysis_gaps: bool,
    /// A routine (by name) whose control-flow tree is missing.
    pub(crate) no_tree: Option<String>,
}

impl BetweenSites {
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "after {} before {}",
            self.after.label(),
            self.before.label()
        )
    }

    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        let one =
            |p: &SitePoint| serde_json::json!({"file": p.file, "line": p.line, "column": p.column});
        serde_json::json!({"after": one(&self.after), "before": one(&self.before)})
    }
}

// ---------------------------------------------------------------------------
// The answer
// ---------------------------------------------------------------------------

/// A resolved site (A or B).
#[derive(Debug, Clone)]
pub struct SiteDesc {
    pub routine: String,
    pub routine_display: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub op: Option<String>,
}

/// The nearest-first key of a row (review output guidance): cone hops from
/// its region statement, part order (A-tail deepest first, middle, B-head),
/// the statement's place, workspace before dependency, then the row's place.
type NearKey = (usize, usize, u32, u32, bool, Option<String>, Option<u32>);

/// One error row, listed once in [`BetweenOut::errors`].
#[derive(Debug, Clone)]
pub struct ErrorRow {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub op_id: String,
    pub op: Option<String>,
    pub labels: Vec<&'static str>,
    /// `explicit` (an `Error`), `implicit` (a record operation, a check),
    /// `commit-behavior` (a commit under `[CommitBehavior(Error)]`), `window`
    /// (a page or report run with a write that may be pending).
    pub kind: &'static str,
    /// The region statement whose cone holds it (`file:line`).
    pub statement: Option<String>,
    pub hops: usize,
    near: NearKey,
}

/// One commit row.
#[derive(Debug, Clone)]
pub struct CommitRow {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub op_id: String,
    /// 1: ends the caller's transaction; 2: the run's own transaction only.
    pub class: u8,
    /// `commit`, `run-commit`, `run-commit-of-A`, `isolated-commit-of-A`.
    pub kind: &'static str,
    /// `A-tail`, `middle`, `B-head`, `inside-A`, `inside-B`.
    pub part: &'static str,
    pub labels: Vec<&'static str>,
    /// `commitsWithoutB`: how the paths end (`exit`, `return`, `error`,
    /// `b-never-after-a`).
    pub ends: Vec<&'static str>,
    pub route: Option<SliceRoute>,
    /// Indices into [`BetweenOut::errors`]: errors that may run after this
    /// commit and before B, nearest first.
    pub errors_after: Vec<usize>,
    /// Errors in the commit's own cone, whose order against it is not read.
    pub errors_same_cone: Vec<usize>,
    /// The region statement that starts its cone (`file:line`).
    pub statement: Option<String>,
    pub hops: usize,
    near: NearKey,
}

/// The `commit-between` answer of one root.
#[derive(Debug, Clone, Default)]
pub struct BetweenOut {
    /// A site could not be resolved: the usage error (exit 1).
    pub refused: Option<String>,
    pub after: Option<SiteDesc>,
    pub before: Option<SiteDesc>,
    /// One route from the root to A and to B (the first pair's).
    pub route_a: Option<SliceRoute>,
    pub route_b: Option<SliceRoute>,
    pub between: Vec<CommitRow>,
    pub inside_a: Vec<CommitRow>,
    pub inside_b: Vec<CommitRow>,
    pub without_b: Vec<CommitRow>,
    pub own_transaction: Vec<CommitRow>,
    /// Every error row, once; the lists hold indices into it.
    pub errors: Vec<ErrorRow>,
    err_index: HashMap<String, usize>,
    /// (list, operation id, kind, class) → the row's index in its list,
    /// while the walk builds; the lists are sorted (and this dropped) last.
    commit_index: HashMap<(u8, String, &'static str, u8), usize>,
    /// Errors that may run after A returns and before B (indices).
    pub errors_between: Vec<usize>,
    /// `b-never-after-a`, `not-in-one-build`, `a-dead-under-literals`,
    /// `b-dead-under-literals` (for a `no`); `route-search-capped` when no
    /// route was found but the search stopped at its budget (an `unknown`).
    pub relation: Option<&'static str>,
    /// What could not be followed: (kind, detail).
    pub blockers: Vec<(&'static str, String)>,
    pub pairs_examined: usize,
    /// The region's statement ids and the routines its cones reach, read by
    /// prove against the root's unresolved cone (design §5.5).
    pub region_sites: BTreeSet<String>,
    pub region_routines: BTreeSet<String>,
    /// Test knobs that set prove's obligations (the inputs of the
    /// `cone-truncated` / `analysis-gaps` guards) before the verdict reads
    /// them; always false in production.
    pub(crate) knob_cone_truncated: bool,
    pub(crate) knob_analysis_gaps: bool,
}

// ---------------------------------------------------------------------------
// Control-flow graph of one routine (design §4.2)
// ---------------------------------------------------------------------------

/// A routine's control-flow graph: one node per statement leaf (with its
/// site ids), joins for branches and loops, and three sinks.
struct Cfg<'t> {
    ids: Vec<Vec<&'t str>>,
    succ: Vec<Vec<usize>>,
    back: HashSet<(usize, usize)>,
    entry: usize,
    ret: usize,
    exit: usize,
    fail: usize,
}

struct Builder<'t> {
    g: Cfg<'t>,
    /// Per enclosing loop: (break target, continue target).
    loops: Vec<(usize, usize)>,
    /// Where an `Error` statement goes (inside `asserterror`: on).
    fail: Vec<usize>,
}

fn kids(o: &Option<Vec<PCFNNode>>) -> &[PCFNNode] {
    o.as_deref().unwrap_or(&[])
}

impl<'t> Builder<'t> {
    fn node(&mut self) -> usize {
        self.g.ids.push(Vec::new());
        self.g.succ.push(Vec::new());
        self.g.ids.len() - 1
    }

    fn edge(&mut self, a: usize, b: usize) {
        if !self.g.succ[a].contains(&b) {
            self.g.succ[a].push(b);
        }
    }

    fn back_edge(&mut self, a: usize, b: usize) {
        self.edge(a, b);
        self.g.back.insert((a, b));
    }

    fn list(&mut self, nodes: &'t [PCFNNode], next: usize) -> usize {
        let mut cur = next;
        for n in nodes.iter().rev() {
            cur = self.stmt(n, cur);
        }
        cur
    }

    fn site(&mut self, n: &'t PCFNNode) -> usize {
        let s = self.node();
        if let Some(c) = n.callsite_id.as_deref() {
            self.g.ids[s].push(c);
        }
        if let Some(o) = n.operation_id.as_deref() {
            self.g.ids[s].push(o);
        }
        s
    }

    fn branch(&mut self, then: &'t [PCFNNode], els: &'t [PCFNNode], next: usize) -> usize {
        let j = self.node();
        let t = self.list(then, next);
        let e = if els.is_empty() {
            next
        } else {
            self.list(els, next)
        };
        self.edge(j, t);
        self.edge(j, e);
        j
    }

    fn stmt(&mut self, n: &'t PCFNNode, next: usize) -> usize {
        let leaves = kids(&n.condition_leaves);
        match n.kind.as_str() {
            // The condition runs before each pass and once more at the end.
            "while" => {
                let head = self.node();
                let again = self.node();
                self.loops.push((next, again));
                let body = self.list(kids(&n.children), again);
                self.loops.pop();
                let cond = self.list(leaves, head);
                self.back_edge(again, cond);
                self.edge(head, body);
                self.edge(head, next);
                return cond;
            }
            // The range is evaluated once, before the body.
            "for" | "foreach" => {
                let head = self.node();
                let again = self.node();
                self.loops.push((next, again));
                let body = self.list(kids(&n.children), again);
                self.loops.pop();
                self.back_edge(again, head);
                self.edge(head, body);
                self.edge(head, next);
                return self.list(leaves, head);
            }
            // The body runs once before the condition is tested.
            "repeat" => {
                let top = self.node();
                let after = self.node();
                let cond = self.list(leaves, after);
                self.loops.push((next, cond));
                let body = self.list(kids(&n.children), cond);
                self.loops.pop();
                self.edge(top, body);
                self.back_edge(after, top);
                self.edge(after, next);
                return top;
            }
            _ => {}
        }
        let body = match n.kind.as_str() {
            "op" | "call" => {
                let s = self.site(n);
                let rest = self.list(kids(&n.children), next);
                self.edge(s, rest);
                s
            }
            "error" => {
                let s = self.site(n);
                let to = *self.fail.last().expect("a fail target");
                self.edge(s, to);
                s
            }
            "exit" => self.g.exit,
            "break" => self.loops.last().map_or(next, |l| l.0),
            "continue" => self.loops.last().map_or(next, |l| l.1),
            "if" => self.branch(kids(&n.children), kids(&n.else_children), next),
            "case" => {
                let j = self.node();
                let mut has_else = false;
                for b in kids(&n.children) {
                    has_else |= b.is_case_else();
                    let e = self.list(kids(&b.children), next);
                    self.edge(j, e);
                }
                if !has_else {
                    self.edge(j, next);
                }
                j
            }
            "try" => next,
            _ if n.path_hint() == Some(PathHint::AssertError) => {
                // `asserterror`: an error inside ends the block, not the path.
                self.fail.push(next);
                let b = self.list(kids(&n.children), next);
                self.fail.pop();
                b
            }
            _ if !kids(&n.else_children).is_empty() => {
                self.branch(kids(&n.children), kids(&n.else_children), next)
            }
            _ if n.callsite_id.is_some() || n.operation_id.is_some() => {
                let s = self.site(n);
                let rest = self.list(kids(&n.children), next);
                self.edge(s, rest);
                s
            }
            _ => self.list(kids(&n.children), next),
        };
        let entry = self.list(leaves, body);
        // A leaf that may not run (a ternary arm, `PathHint::MayRun`): the
        // path may skip it.
        if n.path_hint() == Some(PathHint::MayRun) {
            let j = self.node();
            self.edge(j, entry);
            self.edge(j, next);
            return j;
        }
        entry
    }
}

impl<'t> Cfg<'t> {
    fn of(tree: &'t PCFNNode) -> Cfg<'t> {
        let mut b = Builder {
            g: Cfg {
                ids: Vec::new(),
                succ: Vec::new(),
                back: HashSet::new(),
                entry: 0,
                ret: 0,
                exit: 0,
                fail: 0,
            },
            loops: Vec::new(),
            fail: Vec::new(),
        };
        b.g.ret = b.node();
        b.g.exit = b.node();
        b.g.fail = b.node();
        b.fail.push(b.g.fail);
        let ret = b.g.ret;
        b.g.entry = b.stmt(tree, ret);
        b.g
    }

    /// The nodes holding one of `ids`.
    fn nodes_of(&self, ids: &HashSet<&str>) -> Vec<usize> {
        (0..self.ids.len())
            .filter(|&n| self.ids[n].iter().any(|i| ids.contains(i)))
            .collect()
    }

    fn succs(&self, n: usize) -> Vec<usize> {
        self.succ[n].clone()
    }

    /// Forward reach from `starts`, not expanding the nodes in `stop`, and
    /// not taking back edges when `no_back`.
    fn forward(&self, starts: &[usize], stop: &HashSet<usize>, no_back: bool) -> Vec<bool> {
        let mut seen = vec![false; self.ids.len()];
        let mut q: VecDeque<usize> = VecDeque::new();
        for &s in starts {
            if !seen[s] {
                seen[s] = true;
                q.push_back(s);
            }
        }
        while let Some(n) = q.pop_front() {
            if stop.contains(&n) {
                continue;
            }
            for &m in &self.succ[n] {
                if no_back && self.back.contains(&(n, m)) {
                    continue;
                }
                if !seen[m] {
                    seen[m] = true;
                    q.push_back(m);
                }
            }
        }
        seen
    }

    /// The nodes that reach one of `targets` (themselves included), not
    /// passing through the nodes in `stop` (other than as the last node).
    fn backward(&self, targets: &[usize], stop: &HashSet<usize>) -> Vec<bool> {
        self.backward_m(targets, stop, false)
    }

    /// [`Self::backward`], not taking back edges when `no_back`.
    fn backward_m(&self, targets: &[usize], stop: &HashSet<usize>, no_back: bool) -> Vec<bool> {
        let mut pred: Vec<Vec<usize>> = vec![Vec::new(); self.ids.len()];
        for (a, ss) in self.succ.iter().enumerate() {
            for &b in ss {
                if !(no_back && self.back.contains(&(a, b))) {
                    pred[b].push(a);
                }
            }
        }
        let mut seen = vec![false; self.ids.len()];
        let mut q: VecDeque<usize> = targets.iter().copied().collect();
        for &t in targets {
            seen[t] = true;
        }
        while let Some(n) = q.pop_front() {
            for &p in &pred[n] {
                if !seen[p] && !stop.contains(&p) {
                    seen[p] = true;
                    q.push_back(p);
                }
            }
        }
        seen
    }

    /// Whether every path from the entry to a normal return passes one of
    /// `leave` (an error ends the whole call, so it never skips B alone).
    fn surely_passes(&self, leave: &[usize]) -> bool {
        let stop: HashSet<usize> = leave.iter().copied().collect();
        let f = self.forward(&[self.entry], &stop, false);
        !(f[self.ret] || f[self.exit])
    }

    /// The sinks `n` reaches without passing `stop`: `return`, `exit`,
    /// `error`.
    fn ends(&self, n: usize, stop: &HashSet<usize>) -> Vec<&'static str> {
        let f = self.forward(&[n], stop, false);
        let mut out = Vec::new();
        if f[self.ret] {
            out.push("return");
        }
        if f[self.exit] {
            out.push("exit");
        }
        if f[self.fail] {
            out.push("error");
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/// A resolved site: the row holding it, its stable routine and every id its
/// statement carries (call site, its operation, record operations and
/// operation sites at the same place).
#[derive(Clone)]
struct Resolved<'a> {
    owner: &'a str,
    ids: Vec<&'a str>,
    anchor: &'a PAnchor,
    op: Option<String>,
}

fn same_place(a: &PAnchor, b: &PAnchor) -> bool {
    (a.start_line, a.start_column, a.end_line, a.end_column)
        == (b.start_line, b.start_column, b.end_line, b.end_column)
}

/// Every id of the statement at `anchor` in `r`.
fn ids_at<'a>(r: &'a ModelRoutine, anchor: &PAnchor) -> Vec<&'a str> {
    let mut ids: Vec<&'a str> = Vec::new();
    for c in &r.call_sites {
        if same_place(&c.source_anchor, anchor) {
            ids.push(c.id.as_str());
            ids.push(c.operation_id.as_str());
        }
    }
    for o in &r.record_operations {
        if same_place(&o.source_anchor, anchor) {
            ids.push(o.id.as_str());
        }
    }
    for o in &r.operation_sites {
        if same_place(&o.source_anchor, anchor) {
            ids.push(o.id.as_str());
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Resolve `p` to one statement (design §3), or the refusal.
fn resolve<'a>(model: &'a Model, p: &SitePoint, flag: &str) -> Result<Resolved<'a>, String> {
    let mut found: Vec<(&'a ModelRoutine, &'a PAnchor, Option<String>)> = Vec::new();
    let table_names: HashMap<&str, &str> = HashMap::new();
    for r in &model.workspace.routines {
        let mut anchors: Vec<&'a PAnchor> = Vec::new();
        anchors.extend(r.call_sites.iter().map(|c| &c.source_anchor));
        anchors.extend(r.record_operations.iter().map(|o| &o.source_anchor));
        anchors.extend(r.operation_sites.iter().map(|o| &o.source_anchor));
        for a in anchors {
            if a.start_line + 1 != p.line || !p.file_matches(&a.source_unit_id) {
                continue;
            }
            if p.column.is_some_and(|c| a.start_column + 1 != c) {
                continue;
            }
            if found
                .iter()
                .any(|(fr, fa, _)| std::ptr::eq(*fr, r) && same_place(fa, a))
            {
                continue;
            }
            let id = ids_at(r, a).first().copied().unwrap_or("");
            let op = site_facts(r, id, &table_names).op_name;
            found.push((r, a, op));
        }
    }
    let files: BTreeSet<&str> = found.iter().map(|(_, a, _)| &*a.source_unit_id).collect();
    if files.len() > 1 {
        return Err(format!(
            "prove: {flag} '{}' names more than one file: {}; give more of the path",
            p.label(),
            files.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    match found.len() {
        0 => Err(format!(
            "prove: {flag} '{}' names no call or record operation (a site is a call or a \
             record operation on that line)",
            p.label()
        )),
        1 => {
            let (r, a, op) = found.remove(0);
            Ok(Resolved {
                owner: r.stable_routine_id.as_str(),
                ids: ids_at(r, a),
                anchor: a,
                op,
            })
        }
        _ => {
            let mut list: Vec<String> = found
                .iter()
                .map(|(_, a, op)| {
                    format!(
                        "{}:{}:{} {}",
                        a.source_unit_id,
                        a.start_line + 1,
                        a.start_column + 1,
                        op.as_deref().unwrap_or("?")
                    )
                })
                .collect();
            list.sort();
            Err(format!(
                "prove: {flag} '{}' holds {} sites; add the column (1-based): {}",
                p.label(),
                found.len(),
                list.join("; ")
            ))
        }
    }
}

/// The records of one routine's statement, by (owner, site id).
type ByOwner<'a, 'r> = HashMap<&'a str, Vec<&'r TerminalRecord<'a>>>;

/// One part of a pair's region.
struct Part<'a> {
    routine: &'a str,
    row: &'a ModelRoutine,
    kind: &'static str,
    /// Region nodes, in the routine's cfg.
    region: Vec<usize>,
    /// Region nodes reached without a back edge.
    region_no_back: HashSet<usize>,
    /// Nodes after the start that never reach the target.
    without: Vec<usize>,
    /// Where the part's paths start (after the site toward A, or the entry).
    starts: Vec<usize>,
    /// The part's place for the nearest-first key, by kind and depth from M
    /// (review r2 item 7: comparable across route pairs): A-tail deepest
    /// first (`1000 - depth`), M's middle `1000`, B-head `1000 + depth`.
    rank: usize,
    target: HashSet<usize>,
    /// The route from the root to this part's routine.
    prefix: Vec<Step<'a>>,
    /// Errors here are caught by a trap on A's route below M (N2), or come
    /// before B behind a trap on B's route.
    caught_by_route_trap: bool,
    swallowed_before_b: bool,
    /// A-tail: a path from the start returns normally (or ends in an error a
    /// trap on A's route catches), so control goes back toward M.
    cont: bool,
    /// Review B3: a path from this part goes on to B (every shallower A-tail
    /// part returns, and M's middle reaches B). Commits in a part that does
    /// not are `commitsWithoutB`.
    reaches_b: bool,
    /// Review B4: a B-head part below a checked run on B's route: its
    /// commits are the run's own (class 2).
    b_run: bool,
}

struct Walk<'a, 'w> {
    reach: &'w LiteralReach<'a>,
    tree: &'a CapabilitySnapshot,
    root: &'a str,
    live: Option<LiveTree<'a>>,
    live_cache: HashMap<&'a str, bool>,
    commits: ByOwner<'a, 'w>,
    errors: ByOwner<'a, 'w>,
    by_fact: HashMap<usize, &'w TerminalRecord<'a>>,
    cfgs: HashMap<usize, Cfg<'a>>,
    /// Cone walks by (sorted statement ids, routine) (review COST).
    cones: HashMap<ConeKey<'a>, Rc<Cone<'a>>>,
    /// Region cones whose side effects (reached routines, blockers) are in.
    region_folded: HashSet<ConeKey<'a>>,
    /// Whether a routine can reach itself over the edge graph (review r3
    /// item 1), memoized.
    self_reach: HashMap<&'a str, bool>,
    /// Edges (from, to, site) the graph marks `TriggerFires::Maybe`: a
    /// trigger of a bulk operation or a non-literal RunTrigger, a run into a
    /// trigger that may not run or runs once per row (a report dataitem, an
    /// XmlPort element, a page's OnAfterGetRecord) (reviews r5).
    maybe_fire: HashSet<(&'a str, &'a str, &'a str)>,
    knobs: Knobs,
    commit_scopes: Option<crate::engine::l5::commit_scope::CbGraph<'a>>,
    group_paths: GroupPaths<'a>,
    out: BetweenOut,
    names: HashMap<&'a str, &'a str>,
}

/// A cone walk's memo key: (sorted statement ids, then the routine), the
/// behavior set it starts under, own records only.
type ConeKey<'a> = (Vec<&'a str>, u8, bool);

/// What one cone walk found.
#[derive(Default)]
struct Cone<'a> {
    /// The walk's tree: per node, its parent, the edge into it and the trap
    /// that edge is.
    nodes: Vec<(Option<usize>, Step<'a>, Option<TrapKind>)>,
    /// (record, class, the walk node it was met at; `None`: the statement's
    /// own).
    /// (record, class, the walk node, the `[CommitBehavior]` set it runs
    /// under on that path, raw bits).
    commits: Vec<(usize, u8, Option<usize>, u8)>,
    errors: Vec<(usize, Vec<&'static str>, Option<usize>)>,
    /// Every routine the walk visited.
    visited: Vec<&'a str>,
    capped: bool,
}

impl<'a> Cone<'a> {
    /// The steps from the region statement to walk node `n`.
    fn steps(&self, n: Option<usize>) -> Vec<Step<'a>> {
        let mut out = Vec::new();
        let mut cur = n;
        while let Some(i) = cur {
            out.push(self.nodes[i].1);
            cur = self.nodes[i].0;
        }
        out.reverse();
        out
    }

    /// The labels of the path to walk node `n`.
    fn labels(&self, n: Option<usize>) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        let mut cur = n;
        while let Some(i) = cur {
            let l = label_of(self.nodes[i].1.kind, self.nodes[i].2);
            if !out.contains(&l) {
                out.push(l);
            }
            cur = self.nodes[i].0;
        }
        out.sort_unstable();
        out
    }
}

impl<'a, 'w> Walk<'a, 'w> {
    fn display(&self, stable: &str) -> String {
        match self.reach.routine(stable) {
            Some(r) => match self.names.get(r.object_id.as_str()) {
                Some(o) => format!("{o}::{}", r.name),
                None => r.name.clone(),
            },
            None => stable.to_string(),
        }
    }

    /// Build `row`'s graph once; `false` when the row has no tree.
    fn ensure_cfg(&mut self, row: &'a ModelRoutine) -> bool {
        if self.knobs.no_tree.as_deref() == Some(row.name.as_str()) {
            return false;
        }
        let Some(tree) = row.statement_tree.as_deref() else {
            return false;
        };
        self.cfgs
            .entry(row_key(row))
            .or_insert_with(|| Cfg::of(tree));
        true
    }

    fn cfg(&mut self, row: &'a ModelRoutine) -> Option<&Cfg<'a>> {
        if !self.ensure_cfg(row) {
            return None;
        }
        self.cfgs.get(&row_key(row))
    }

    fn live(&mut self, op: &'a str) -> bool {
        if let Some(v) = self.live_cache.get(op) {
            return *v;
        }
        let v = self
            .live
            .as_ref()
            .is_some_and(|t| self.reach.tree_witness(t, &[op.to_string()]).is_some());
        self.live_cache.insert(op, v);
        v
    }

    /// The behaviors (`commit_scope` N, I, E bits) of a commit met with the
    /// raw set `bits` (review S1, r2 item 1: per route above the region
    /// statement, over every path below it).
    fn states_of(bits: u8) -> u8 {
        use crate::engine::l5::commit_scope::{N, norm};
        match norm(bits) {
            0 => N,
            s => s,
        }
    }

    fn is_run_commit(&self, rec: &TerminalRecord<'_>) -> bool {
        crate::engine::l5::commit_scope::is_run_commit(&self.tree.capability_facts[rec.fact])
    }

    fn is_run_error(&self, rec: &TerminalRecord<'_>) -> bool {
        rec.effect_type == "ERROR_IMPLICIT"
            && self.tree.capability_facts[rec.fact].resource_id()
                == Some(crate::engine::l4::capability_cone::RUN_COMMIT)
    }

    /// The cone of the edges leaving `routine` at one of `ids`, and the
    /// statement's own records. `run_side`: A is inside a run here.
    /// The cone of the statement with `ids` in `routine` (memoized). A
    /// region cone (`region`) also records the routines it reaches (read
    /// against the root's unresolved cone) and blocks on a reached routine
    /// whose body is not available (review B2, S3).
    ///
    /// `b0` is the `[CommitBehavior]` set the statement runs under on the
    /// route above it (`commit_scope` raw bits); the walk carries the set
    /// down every edge, so a commit reached both under `Ignore` and normally
    /// keeps both (review r2, item 1). `own_only`: the statement's own
    /// records, no walk (a statement on a path that ends before B, whose
    /// cone decides nothing; review r2, cost 5).
    fn cone(
        &mut self,
        routine: &'a str,
        ids: &[&'a str],
        region: bool,
        b0: u8,
        own_only: bool,
    ) -> Rc<Cone<'a>> {
        let mut key: Vec<&'a str> = ids.to_vec();
        key.sort_unstable();
        key.push(routine);
        let key = (key, b0, own_only);
        let cone = match self.cones.get(&key) {
            Some(c) => c.clone(),
            None => {
                let c = Rc::new(self.walk_cone(routine, ids, b0, own_only));
                self.cones.insert(key.clone(), c.clone());
                c
            }
        };
        // Review r2, cost 4: a cone's side effects are folded once.
        if region && self.region_folded.insert(key) {
            if cone.capped {
                self.out.blockers.push((
                    "cone-walk-capped",
                    format!(
                        "a region statement's cone passed {CONE_STATE_CAP} routine states; the \
                         walk stopped"
                    ),
                ));
            }
            for &r in &cone.visited {
                if self.out.region_routines.insert(r.to_string()) {
                    let opaque = self.reach.routine(r).is_none_or(|row| !row.body_available);
                    if opaque {
                        let d = self.display(r);
                        self.out.blockers.push((
                            "region-body-not-available",
                            format!(
                                "{d} may run between A and B and its body is not available \
                                 (symbols only); what it does is not known"
                            ),
                        ));
                    }
                }
            }
        }
        cone
    }

    /// The behavior set `to` runs under, entered by an edge at `site` from a
    /// caller running under `b`.
    fn enter(&self, b: u8, site: Option<EdgeSite<'a>>, to: &str) -> u8 {
        match &self.commit_scopes {
            Some(cb) if !cb.root_unaffected(self.root) => {
                let s = match site {
                    Some(EdgeSite::Guarded(x)) => Some(x),
                    _ => None,
                };
                cb.entered(b, cb.step_at(s), to)
            }
            _ => b,
        }
    }

    fn walk_cone(&mut self, routine: &'a str, ids: &[&'a str], b0: u8, own_only: bool) -> Cone<'a> {
        let mut cone = Cone::default();
        let idset: HashSet<&str> = ids.iter().copied().collect();
        // The statement's own records.
        for (i, rec) in self.records_of(routine) {
            if idset.contains(rec.terminal.id()) {
                cone_record(self, &mut cone, i, rec, false, None, None, b0);
            }
        }
        if own_only {
            return cone;
        }
        // (routine, crossed a checked run, the first trap crossed, behaviors)
        type St<'s> = (&'s str, bool, Option<TrapKind>, u8);
        let mut seen: HashSet<St<'a>> = HashSet::new();
        let mut q: VecDeque<(St<'a>, usize)> = VecDeque::new();
        for (to, site, kind, trap) in self.reach.out_edges(routine) {
            let at = match site {
                Some(EdgeSite::Guarded(s)) => s,
                _ => continue,
            };
            if !idset.contains(at) {
                continue;
            }
            let st = (
                to,
                trap == Some(TrapKind::CheckedRun),
                trap,
                self.enter(b0, site, to),
            );
            if seen.insert(st) {
                let step = Step {
                    from: routine,
                    to,
                    site,
                    kind,
                };
                cone.nodes.push((None, step, trap));
                q.push_back((st, cone.nodes.len() - 1));
            }
        }
        let mut visited: HashSet<&'a str> = HashSet::new();
        while let Some(((r, run, trap, b), n)) = q.pop_front() {
            if seen.len() > self.knobs.cone_cap.unwrap_or(CONE_STATE_CAP) {
                cone.capped = true;
                break;
            }
            if visited.insert(r) {
                cone.visited.push(r);
            }
            for (i, rec) in self.records_of(r) {
                cone_record(self, &mut cone, i, rec, run, trap, Some(n), b);
            }
            for (to, site, kind, t) in self.reach.out_edges(r) {
                let st = (
                    to,
                    run || t == Some(TrapKind::CheckedRun),
                    trap.or(t),
                    self.enter(b, site, to),
                );
                if seen.insert(st) {
                    let step = Step {
                        from: r,
                        to,
                        site,
                        kind,
                    };
                    cone.nodes.push((Some(n), step, t));
                    q.push_back((st, cone.nodes.len() - 1));
                }
            }
        }
        cone
    }

    /// The behavior set the routine at the end of `prefix` runs under on that
    /// route (raw bits; `N` when no `[CommitBehavior]` applies to the root).
    fn route_start(&self, prefix: &[Step<'a>]) -> u8 {
        use crate::engine::l5::commit_scope::N;
        let Some(cb) = self
            .commit_scopes
            .as_ref()
            .filter(|cb| !cb.root_unaffected(self.root))
        else {
            return N;
        };
        let hops = prefix.iter().map(|s| {
            let site = match s.site {
                Some(EdgeSite::Guarded(x)) => Some(x),
                _ => None,
            };
            (s.to, site)
        });
        cb.route_states(self.root, hops, None)
    }

    /// Every COMMIT and error record `routine` owns, with its index.
    fn records_of(&self, routine: &str) -> Vec<(usize, &'w TerminalRecord<'a>)> {
        let mut v: Vec<(usize, &'w TerminalRecord<'a>)> = Vec::new();
        for map in [&self.commits, &self.errors] {
            if let Some(rs) = map.get(routine) {
                for r in rs {
                    v.push((index_of(self.reach, r), *r));
                }
            }
        }
        v
    }

    fn error_row(
        &self,
        rec: &TerminalRecord<'a>,
        labels: Vec<&'static str>,
        kind: &'static str,
        at: &At,
    ) -> ErrorRow {
        let id = rec.terminal.id();
        let row = self.reach.row_at(rec.owner, id);
        let anchor = row.and_then(|r| anchor_in(r, id));
        let table_names: HashMap<&str, &str> = HashMap::new();
        let op = row.and_then(|r| site_facts(r, id, &table_names).op_name);
        let file = anchor.map(|a| a.source_unit_id.to_string());
        let line = anchor.map(|a| a.start_line);
        ErrorRow {
            near: at.near(&file, line),
            file,
            line,
            column: anchor.map(|a| a.start_column),
            op_id: id.to_string(),
            op,
            labels,
            kind,
            statement: at.statement.clone(),
            hops: at.hops,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_row(
        &self,
        rec: &TerminalRecord<'a>,
        class: u8,
        kind: &'static str,
        part: &'static str,
        labels: Vec<&'static str>,
        steps: Vec<Step<'a>>,
        at: &At,
    ) -> CommitRow {
        let id = rec.terminal.id();
        let row = self.reach.row_at(rec.owner, id);
        let anchor = row.and_then(|r| anchor_in(r, id));
        let path = self.group_paths.path_of(self.reach, &steps, id);
        let sites = route_sites(self.reach, self.root, &path, (row, anchor));
        let file = anchor.map(|a| a.source_unit_id.to_string());
        let line = anchor.map(|a| a.start_line);
        CommitRow {
            near: at.near(&file, line),
            file,
            line,
            column: anchor.map(|a| a.start_column),
            op_id: id.to_string(),
            class,
            kind,
            part,
            labels,
            ends: Vec::new(),
            route: Some(SliceRoute { path, sites }),
            errors_after: Vec::new(),
            errors_same_cone: Vec::new(),
            statement: at.statement.clone(),
            hops: at.hops,
        }
    }

    /// Where a region statement sits, for the nearest-first key.
    fn at(&self, row: &'a ModelRoutine, ids: &[&'a str], part: usize, hops: usize) -> At {
        let anchor = ids.iter().find_map(|id| anchor_in(row, id));
        At {
            statement: anchor.map(|a| format!("{}:{}", a.source_unit_id, a.start_line + 1)),
            hops,
            part,
            line: anchor.map_or(u32::MAX, |a| a.start_line),
            column: anchor.map_or(u32::MAX, |a| a.start_column),
        }
    }
}

/// A row's region statement and depth (review output guidance).
#[derive(Clone)]
struct At {
    statement: Option<String>,
    hops: usize,
    part: usize,
    line: u32,
    column: u32,
}

impl At {
    fn near(&self, file: &Option<String>, line: Option<u32>) -> NearKey {
        let dep = file.as_deref().is_some_and(|f| f.starts_with("dep:"));
        (
            self.hops,
            self.part,
            self.line,
            self.column,
            dep,
            file.clone(),
            line,
        )
    }
}

fn index_of(_reach: &LiteralReach<'_>, rec: &TerminalRecord<'_>) -> usize {
    rec.fact
}

fn row_key(row: &ModelRoutine) -> usize {
    row as *const ModelRoutine as usize
}

fn label_of(kind: &str, trap: Option<TrapKind>) -> &'static str {
    match (kind, trap) {
        (_, Some(TrapKind::IsolatedEvent)) => "isolated-subscriber",
        ("event-dispatch", _) => "through-event",
        ("object-run-resolved", _) => "through-object-run",
        ("implicit-trigger" | "table-event", _) => "through-trigger",
        _ => "through-callee",
    }
}

/// Fold one record met in a cone walk into `cone`.
#[allow(clippy::too_many_arguments)]
fn cone_record<'a>(
    w: &mut Walk<'a, '_>,
    cone: &mut Cone<'a>,
    i: usize,
    rec: &TerminalRecord<'a>,
    run: bool,
    trap: Option<TrapKind>,
    node: Option<usize>,
    behaviors: u8,
) {
    let id = rec.terminal.id();
    if id.is_empty() || !w.live(id) {
        return;
    }
    if rec.effect_type == "COMMIT" {
        // `behaviors`: the `[CommitBehavior]` set on this path (review S1,
        // r2 item 1); a commit met on several paths is recorded per path.
        let class = if run || w.is_run_commit(rec) { 2 } else { 1 };
        if !cone
            .commits
            .iter()
            .any(|c| c.0 == i && c.1 == class && c.3 == behaviors)
        {
            cone.commits.push((i, class, node, behaviors));
        }
        return;
    }
    // A page or report run with a write pending (design §4.3): a page's
    // `RunModal` raises an error (measured, T18); a report with a request
    // page was not measured. Listed as possible errors, never as commits.
    let mut l: Vec<&'static str> = Vec::new();
    if rec.effect_type == "UI_WINDOW_OPEN" {
        match window_kind(w, rec) {
            Some(k) => l.push(k),
            None => return,
        }
    }
    // An error: behind a definite trap it is caught on the way (left out).
    match trap {
        Some(t) if t.definite() => {}
        Some(_) => {
            l.push("trap-may-fail-pending");
            cone.errors.push((i, l, node));
        }
        None => {
            if w.is_run_error(rec) {
                l.push("run-error-rolls-back-a");
            }
            cone.errors.push((i, l, node));
        }
    }
}

/// The label of a window-opening call with a write that may be pending:
/// `None` for a `StrMenu` (no page or report).
fn window_kind(w: &Walk<'_, '_>, rec: &TerminalRecord<'_>) -> Option<&'static str> {
    use crate::program::body::features::PCallee;
    use al_syntax::IdentifierFoldExt;
    let id = rec.terminal.id();
    let cs = w
        .reach
        .row_at(rec.owner, id)?
        .call_sites
        .iter()
        .find(|c| c.id == id || c.operation_id == id)?;
    let (kind, method) = match &cs.callee {
        PCallee::ObjectRun { object_kind, .. } => (object_kind.fold_identifier(), None),
        PCallee::Member { receiver, method } => {
            let r = receiver.fold_identifier();
            let k = if r == "report" {
                "report".to_string()
            } else {
                "page".to_string()
            };
            (k, Some(method.fold_identifier()))
        }
        _ => return None,
    };
    Some(match (kind.as_str(), method.as_deref()) {
        ("page", Some("runmodal")) => "runmodal-pending-write",
        ("page", _) => "page-run-if-a-write-is-pending (measured for RunModal only)",
        _ => "unmeasured: report run with a pending write",
    })
}

/// Whether B surely runs once the path reaches the site toward B in the
/// routine at `route[from..]` (design §4.2): B itself, or a call that is no
/// trap whose callee surely passes the next site, recursively.
fn b_sure<'a>(w: &mut Walk<'a, '_>, route: &[Step<'a>], from: usize, b: &Resolved<'a>) -> bool {
    if from >= route.len() {
        return true;
    }
    let s = route[from];
    let at = match s.site {
        Some(EdgeSite::Guarded(x)) => x,
        _ => return false,
    };
    let trap = w
        .reach
        .out_edges(s.from)
        .into_iter()
        .find(|(to, site, _, _)| *to == s.to && *site == s.site)
        .and_then(|e| e.3);
    if trap.is_some() {
        return false;
    }
    // Review r5 (sol) item 2: B is sure only if the route's edge fires
    // whenever its site runs.
    if !fires_surely(w, &s) {
        return false;
    }
    let Some(row) = w.reach.row_at(s.to, leave_id(route, from + 1, b)) else {
        return false;
    };
    let ids: HashSet<&str> = leave_ids(route, from + 1, b).into_iter().collect();
    let _ = at;
    let Some(cfg) = w.cfg(row) else {
        return false;
    };
    let leave = cfg.nodes_of(&ids);
    if leave.is_empty() || !cfg.surely_passes(&leave) {
        return false;
    }
    b_sure(w, route, from + 1, b)
}

/// Reviews r5 (sol item 2, Fable issue 2): whether route step `s` is a plain
/// edge that must run its one target every time its site runs, the only
/// edge a sure-B cut may stand on. A direct call, or a variable-typed call,
/// implicit trigger or object run the graph does not mark `Maybe`, with no
/// other edge at the same site. Never a table event (a bulk operation may
/// match no row; a subscriber may be bound by hand), an interface call (one
/// implementer of several runs, open or closed world), or an event edge.
fn fires_surely<'a>(w: &Walk<'a, '_>, s: &Step<'a>) -> bool {
    let Some(EdgeSite::Guarded(x)) = s.site else {
        return false;
    };
    let edges = w.reach.out_edges(s.from);
    let Some(&(_, _, kind, _)) = edges
        .iter()
        .find(|(to, site, _, _)| *to == s.to && *site == s.site)
    else {
        return false;
    };
    let plain = match kind {
        "direct-call" => true,
        "variable-typed-call" | "implicit-trigger" | "object-run-resolved" => {
            !w.maybe_fire.contains(&(s.from, s.to, x))
        }
        _ => false,
    };
    plain
        && !edges
            .iter()
            .any(|(to, site, _, _)| *site == s.site && *to != s.to)
}

/// Reviews r4 item 2 and r5 (sol item 1, Fable issue 1): whether route step
/// `s` may run its target more than once per run of its site, and why: a
/// trigger or table event of a bulk record operation (once per row), or of
/// an operation the model does not hold; or a variable-typed call or object
/// run the graph marks `Maybe` (a report dataitem's or XmlPort element's
/// trigger, once per row; a page's `OnAfterGetRecord`).
fn may_repeat<'a>(w: &Walk<'a, '_>, s: &Step<'a>) -> Option<(&'static str, &'static str)> {
    let Some(EdgeSite::Guarded(x)) = s.site else {
        return None;
    };
    let kind = w
        .reach
        .out_edges(s.from)
        .into_iter()
        .find(|(to, site, _, _)| *to == s.to && *site == s.site)
        .map(|e| e.2)?;
    match kind {
        "implicit-trigger" | "table-event" => match op_of(w, s.from, x) {
            Some(o) if is_bulk(o) => Some((
                "bulk-operation-on-route",
                "by a bulk record operation whose trigger runs once per row",
            )),
            Some(_) => None,
            None => Some((
                "bulk-operation-on-route",
                "by a record operation the model does not hold; whether it is a bulk operation \
                 (its trigger run once per row) is not known",
            )),
        },
        "variable-typed-call" | "object-run-resolved"
            if w.maybe_fire.contains(&(s.from, s.to, x)) =>
        {
            Some((
                "repeated-trigger-on-route",
                "into an object trigger that may run once per row or not at all (a report \
                 dataitem, an XmlPort element, a page's record trigger)",
            ))
        }
        _ => None,
    }
}

/// The record operation (its name) with id `x` in `routine`'s row.
fn op_of<'a>(w: &Walk<'a, '_>, routine: &'a str, x: &str) -> Option<&'a str> {
    w.reach
        .row_at(routine, x)
        .and_then(|row| row.record_operations.iter().find(|o| o.id == x))
        .map(|o| o.op.as_str())
}

fn is_bulk(op: &str) -> bool {
    op.eq_fold_identifier("ModifyAll") || op.eq_fold_identifier("DeleteAll")
}

/// The ids the route leaves its routine at, at position `i` (the target's own
/// ids past the last step).
fn leave_ids<'a>(route: &[Step<'a>], i: usize, target: &Resolved<'a>) -> Vec<&'a str> {
    if i >= route.len() {
        return target.ids.clone();
    }
    match route[i].site {
        Some(EdgeSite::Guarded(s)) => vec![s],
        _ => Vec::new(),
    }
}

fn leave_id<'a>(route: &[Step<'a>], i: usize, target: &Resolved<'a>) -> &'a str {
    leave_ids(route, i, target).first().copied().unwrap_or("")
}

/// The routine at position `i` of `route` (0: the root).
fn routine_at<'a>(root: &'a str, route: &[Step<'a>], i: usize) -> &'a str {
    if i == 0 { root } else { route[i - 1].to }
}

fn trap_of<'a>(w: &Walk<'a, '_>, s: &Step<'a>) -> Option<TrapKind> {
    w.reach
        .out_edges(s.from)
        .into_iter()
        .find(|(to, site, _, _)| *to == s.to && *site == s.site)
        .and_then(|e| e.3)
}

/// The (from, to, site) of every edge marked `TriggerFires::Maybe`.
fn maybe_fire_edges(tree: &CapabilitySnapshot) -> HashSet<(&str, &str, &str)> {
    use crate::engine::l4::combined_graph::TriggerFires::Maybe;
    use crate::engine::l5::snapshot::SnapshotGraphEdge as E;
    tree.typed_edges
        .iter()
        .filter_map(|e| match e {
            E::ImplicitTrigger {
                from,
                to,
                operation_id: site,
                fires: Maybe,
                ..
            }
            | E::VariableTypedCall {
                from,
                to,
                callsite_id: site,
                fires: Some(Maybe),
                ..
            }
            | E::ObjectRunResolved {
                from,
                to,
                callsite_id: site,
                fires: Some(Maybe),
                ..
            } => Some((from.as_str(), to.as_str(), site.as_str())),
            _ => None,
        })
        .collect()
}

/// The answer of `commit-between` for `root` (module doc).
pub(crate) fn compute<'a>(
    tree: &'a CapabilitySnapshot,
    model: &'a Model,
    root: &'a str,
    sites: &BetweenSites,
    cancel: &dyn Fn() -> bool,
) -> BetweenOut {
    let _s = crate::engine::perf_trace::span("prove", "prove.commit_between");
    let mut out = BetweenOut::default();
    let (a, b) = match (
        resolve(model, &sites.after, "--after"),
        resolve(model, &sites.before, "--before"),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            out.refused = Some(e);
            return out;
        }
    };
    let reach = LiteralReach::new(tree, model);
    let names: HashMap<&str, &str> = model
        .workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();
    let mut commits: ByOwner = HashMap::new();
    let mut errors: ByOwner = HashMap::new();
    let mut by_fact = HashMap::new();
    for rec in reach.terminals().records() {
        match rec.effect_type {
            "COMMIT" => commits.entry(rec.owner).or_default().push(rec),
            "ERROR_THROW" | "ERROR_IMPLICIT" | "UI_WINDOW_OPEN" => {
                errors.entry(rec.owner).or_default().push(rec)
            }
            _ => continue,
        }
        by_fact.insert(rec.fact, rec);
    }
    let live = reach.live_tree(root);
    let mut w = Walk {
        reach: &reach,
        tree,
        root,
        live,
        live_cache: HashMap::new(),
        commits,
        errors,
        by_fact,
        cfgs: HashMap::new(),
        cones: HashMap::new(),
        region_folded: HashSet::new(),
        self_reach: HashMap::new(),
        maybe_fire: maybe_fire_edges(tree),
        knobs: sites.knobs.clone(),
        commit_scopes: crate::engine::l5::commit_scope::CbGraph::of_tree(tree),
        group_paths: GroupPaths::new(&tree.typed_edges, model),
        out,
        names,
    };
    let desc = |w: &Walk, r: &Resolved| SiteDesc {
        routine: r.owner.to_string(),
        routine_display: w.display(r.owner),
        file: r.anchor.source_unit_id.to_string(),
        line: r.anchor.start_line,
        column: r.anchor.start_column,
        op: r.op.clone(),
    };
    w.out.after = Some(desc(&w, &a));
    w.out.before = Some(desc(&w, &b));
    // Review r2 item 8: a search that stopped at its cap knows nothing about
    // reach: unknown with the reason, not a usage error with a wrong one.
    w.out.knob_cone_truncated = sites.knobs.cone_truncated;
    w.out.knob_analysis_gaps = sites.knobs.analysis_gaps;
    if sites.knobs.search_stopped {
        w.live = None;
    }
    if w.live.is_none() {
        w.out.blockers.push((
            "route-graph-stopped",
            "the root's guarded search stopped at its state cap; whether the root reaches A and B \
             is not known"
                .to_string(),
        ));
        return w.out;
    }
    for (s, flag) in [(&a, "--after"), (&b, "--before")] {
        if !w.live.as_ref().is_some_and(|t| reach.reaches(t, s.owner)) {
            w.out.refused = Some(format!(
                "prove: {flag} names a site in {}, which the root does not reach",
                w.display(s.owner)
            ));
            return w.out;
        }
    }
    let graph = match reach.state_graph(root, cancel) {
        Ok(g) => g,
        Err(_) => {
            w.out.blockers.push((
                "route-graph-stopped",
                "the root's state graph stopped at its cap or was cancelled; no route was \
                 judged"
                    .to_string(),
            ));
            return w.out;
        }
    };
    let routes_to = |s: &Resolved<'a>| {
        let mut all: Vec<Vec<Step<'a>>> = Vec::new();
        let mut capped = false;
        for id in &s.ids {
            if reach.op_owner(id).is_none() {
                continue;
            }
            let budget = sites.knobs.route_budget.unwrap_or(ROUTE_EDGE_BUDGET);
            let found = reach.routes(&graph, id, ROUTE_CAP, budget, cancel);
            capped |= found.limit.is_some();
            for r in found.routes {
                if !all.contains(&r) {
                    all.push(r);
                }
            }
            if !all.is_empty() {
                break;
            }
        }
        (all, capped)
    };
    let (ra_all, ca) = routes_to(&a);
    let (rb_all, cb) = routes_to(&b);
    if ca || cb {
        w.out.blockers.push((
            "route-cap",
            format!(
                "the routes to A or B were not all enumerated (at most {ROUTE_CAP} per site, \
                 {ROUTE_EDGE_BUDGET} candidate edges); more route pairs may exist"
            ),
        ));
    }
    // Review r3 item 3: no route found by a capped search is not "dead".
    if ra_all.is_empty() {
        w.out.relation = Some(if ca {
            "route-search-capped"
        } else {
            "a-dead-under-literals"
        });
        return w.out;
    }
    if rb_all.is_empty() {
        w.out.relation = Some(if cb {
            "route-search-capped"
        } else {
            "b-dead-under-literals"
        });
        return w.out;
    }
    // One route to A and to B (spec §5.1), the first pair's.
    for (route, s, slot) in [(&ra_all[0], &a, 0), (&rb_all[0], &b, 1)] {
        let op = s.ids[0];
        let row = reach.row_at(s.owner, op);
        let path = w.group_paths.path_of(&reach, route, op);
        let sites = route_sites(&reach, root, &path, (row, Some(s.anchor)));
        let r = Some(SliceRoute { path, sites });
        if slot == 0 {
            w.out.route_a = r;
        } else {
            w.out.route_b = r;
        }
    }
    let mut never = 0usize;
    let mut other_build = 0usize;
    for ra in &ra_all {
        for rb in &rb_all {
            w.out.pairs_examined += 1;
            match pair(&mut w, &a, &b, ra, rb) {
                PairEnd::Judged => {}
                PairEnd::Never => never += 1,
                PairEnd::OtherBuild => other_build += 1,
            }
        }
    }
    let pairs = w.out.pairs_examined;
    if w.out.between.is_empty() && w.out.blockers.is_empty() {
        if other_build == pairs {
            w.out.relation = Some("not-in-one-build");
        } else if never + other_build == pairs {
            w.out.relation = Some("b-never-after-a");
        }
    }
    finish(&mut w.out);
    w.out
}

enum PairEnd {
    Judged,
    Never,
    OtherBuild,
}

/// One route pair (design §4.1).
fn pair<'a>(
    w: &mut Walk<'a, '_>,
    a: &Resolved<'a>,
    b: &Resolved<'a>,
    ra: &[Step<'a>],
    rb: &[Step<'a>],
) -> PairEnd {
    let root = w.root;
    let k = ra.iter().zip(rb).take_while(|(x, y)| x == y).count();
    let m = routine_at(root, ra, k);
    let a_leave = leave_ids(ra, k, a);
    let b_leave = leave_ids(rb, k, b);
    if a_leave.is_empty() || b_leave.is_empty() {
        w.out.blockers.push((
            "event-subscriber-order",
            format!(
                "A and B are reached through one event's different subscribers in {}; their \
                 order is not known",
                w.display(m)
            ),
        ));
        return PairEnd::Judged;
    }
    // Engine limits B2: the rows (`#if` arms) holding the two sites in M.
    let (Some(row_a), Some(row_b)) = (w.reach.row_at(m, a_leave[0]), w.reach.row_at(m, b_leave[0]))
    else {
        w.out.blockers.push((
            "site-routine-no-row",
            format!("{} has no model row", w.display(m)),
        ));
        return PairEnd::Judged;
    };
    if !std::ptr::eq(row_a, row_b) {
        return PairEnd::OtherBuild;
    }
    let swallowed = ra[..k]
        .iter()
        .any(|s| trap_of(w, s).is_some_and(TrapKind::definite));
    // Each check adds its own blocker (review r4 nit: every reason shows).
    recursion_on_route(w, ra, rb, k);
    loop_above_meet(w, ra, k);
    // Review r3 item 2: other edges at the sites the routes leave by, from M
    // (index k) down.
    for s in ra[k..].iter().chain(&rb[k..]) {
        site_co_edges(w, s);
    }
    // Review r4 item 2, reviews r5: a step that may run its target more than
    // once (a bulk operation, a per-row object trigger) anywhere on A's
    // route, the shared prefix included, runs A's part or M itself again;
    // those runs are on no part. B's route below M is checked too, as the
    // safe side (the next B is the first run's, whose head is walked; a run
    // that may not reach B is no sure B, `fires_surely`).
    for s in ra.iter().chain(&rb[k..]) {
        repeat_blocker(w, s);
    }
    let mut parts: Vec<Part<'a>> = Vec::new();
    // A's tail, deepest first.
    let mut a_route_trap_at: Option<usize> = None;
    for (j, s) in ra.iter().enumerate().skip(k) {
        if trap_of(w, s).is_some_and(TrapKind::definite) && a_route_trap_at.is_none() {
            a_route_trap_at = Some(j);
        }
    }
    for i in (k + 1..=ra.len()).rev() {
        let r = ra[i - 1].to;
        let leave = leave_ids(ra, i, a);
        // Review S4: a publisher left by its event (A in a subscriber): the
        // publisher's body raises the event and returns; nothing runs after.
        // Review r2 item 2: the event's other subscribers run in no defined
        // order around A's, so they may commit after A: block.
        if leave.is_empty() {
            sibling_subscribers(w, r, &ra[i]);
            continue;
        }
        let Some(row) = w.reach.row_at(r, leave[0]) else {
            continue;
        };
        let caught = a_route_trap_at.is_some_and(|j| j < i);
        match tail_part(w, r, row, &leave, ra[..i].to_vec(), caught, 1000 - (i - k)) {
            Some(p) => parts.push(p),
            None => return PairEnd::Judged,
        }
    }
    // M's middle.
    let sure = b_sure(w, rb, k, b);
    {
        if !w.ensure_cfg(row_a) {
            let t = no_tree(w, m);
            w.out.blockers.push(t);
            return PairEnd::Judged;
        }
        check_complete(w, m, row_a);
        let cfg = &w.cfgs[&row_key(row_a)];
        let aset: HashSet<&str> = a_leave.iter().copied().collect();
        let bset: HashSet<&str> = b_leave.iter().copied().collect();
        let an = cfg.nodes_of(&aset);
        let bn = cfg.nodes_of(&bset);
        if an.is_empty() || bn.is_empty() {
            let d = w.display(m);
            w.out.blockers.push((
                "site-not-in-tree",
                format!("{d}'s control-flow tree does not hold the site"),
            ));
            return PairEnd::Judged;
        }
        let starts: Vec<usize> = an.iter().flat_map(|&n| cfg.succs(n)).collect();
        let target: HashSet<usize> = bn.iter().copied().collect();
        let stop: HashSet<usize> = if sure { target.clone() } else { HashSet::new() };
        let f = cfg.forward(&starts, &stop, false);
        if !bn.iter().any(|&n| f[n]) {
            // B never runs after A in M: what A reaches ends before B.
            let without: Vec<usize> = (0..f.len()).filter(|&n| f[n]).collect();
            let p = Part {
                routine: m,
                row: row_a,
                kind: "middle",
                region: Vec::new(),
                region_no_back: HashSet::new(),
                without,
                starts: starts.clone(),
                rank: 1000,
                target,
                prefix: ra[..k].to_vec(),
                caught_by_route_trap: false,
                swallowed_before_b: false,
                cont: true,
                reaches_b: false,
                b_run: false,
            };
            parts.push(p);
            // Review B3: B never runs after A, so no A-tail part reaches it.
            for p in parts.iter_mut() {
                p.reaches_b = false;
            }
            judge(w, a, b, &parts, swallowed, ra, rb, k);
            return PairEnd::Never;
        }
        let bk = cfg.backward(&bn, &stop);
        let fnb = cfg.forward(&starts, &stop, true);
        let bnb = cfg.backward_m(&bn, &stop, true);
        let region: Vec<usize> = (0..f.len())
            .filter(|&n| f[n] && bk[n] && (!target.contains(&n) || !sure))
            .collect();
        let region_no_back: HashSet<usize> = region
            .iter()
            .copied()
            .filter(|&n| fnb[n] && bnb[n])
            .collect();
        let without: Vec<usize> = (0..f.len()).filter(|&n| f[n] && !bk[n]).collect();
        parts.push(Part {
            routine: m,
            row: row_a,
            kind: "middle",
            region,
            region_no_back,
            without,
            starts,
            rank: 1000,
            target,
            prefix: ra[..k].to_vec(),
            caught_by_route_trap: false,
            swallowed_before_b: false,
            cont: true,
            reaches_b: true,
            b_run: false,
        });
    }
    // Review B3: an A-tail part reaches B only when it and every shallower
    // A-tail part return toward M (parts are deepest first; M's middle,
    // pushed last, reaches B here).
    let mut ok = true;
    for p in parts.iter_mut().rev() {
        if p.kind == "A-tail" {
            p.reaches_b = ok && p.cont;
            ok = p.reaches_b;
        }
    }
    // B's head, shallowest first.
    let mut b_trap = false;
    let mut b_run = false;
    for i in k + 1..=rb.len() {
        let t = trap_of(w, &rb[i - 1]);
        b_trap |= t.is_some_and(TrapKind::definite);
        b_run |= t == Some(TrapKind::CheckedRun);
        let r = rb[i - 1].to;
        let leave = leave_ids(rb, i, b);
        // Review S4: a publisher left by its event (B in a subscriber); its
        // other subscribers may run before B's (review r2 item 2).
        if leave.is_empty() {
            sibling_subscribers(w, r, &rb[i]);
            continue;
        }
        let Some(row) = w.reach.row_at(r, leave[0]) else {
            continue;
        };
        let sure_here = b_sure(w, rb, i, b);
        if !w.ensure_cfg(row) {
            let t = no_tree(w, r);
            w.out.blockers.push(t);
            return PairEnd::Judged;
        }
        check_complete(w, r, row);
        let cfg = &w.cfgs[&row_key(row)];
        let lset: HashSet<&str> = leave.iter().copied().collect();
        let ln = cfg.nodes_of(&lset);
        if ln.is_empty() {
            let d = w.display(r);
            w.out.blockers.push((
                "site-not-in-tree",
                format!("{d}'s control-flow tree does not hold the site"),
            ));
            return PairEnd::Judged;
        }
        let target: HashSet<usize> = ln.iter().copied().collect();
        let stop: HashSet<usize> = if sure_here {
            target.clone()
        } else {
            HashSet::new()
        };
        let f = cfg.forward(&[cfg.entry], &stop, false);
        let bk = cfg.backward(&ln, &stop);
        let fnb = cfg.forward(&[cfg.entry], &stop, true);
        let bnb = cfg.backward_m(&ln, &stop, true);
        let region: Vec<usize> = (0..f.len())
            .filter(|&n| f[n] && bk[n] && (!target.contains(&n) || !sure_here))
            .collect();
        let region_no_back = region
            .iter()
            .copied()
            .filter(|&n| fnb[n] && bnb[n])
            .collect();
        parts.push(Part {
            routine: r,
            row,
            kind: "B-head",
            region,
            region_no_back,
            without: Vec::new(),
            starts: vec![cfg.entry],
            rank: 1000 + (i - k),
            target,
            prefix: rb[..i].to_vec(),
            caught_by_route_trap: false,
            swallowed_before_b: b_trap,
            cont: true,
            reaches_b: true,
            b_run,
        });
    }
    judge(w, a, b, &parts, swallowed, ra, rb, k);
    PairEnd::Judged
}

/// Review r2 item 2: when a route leaves `publisher` by its event (step
/// `s`), the event's other subscribers run in no defined order around the
/// route's: they may commit between A and B. Not walked yet: a blocker.
fn sibling_subscribers<'a>(w: &mut Walk<'a, '_>, publisher: &'a str, s: &Step<'a>) {
    let Some(EdgeSite::Event(ev)) = s.site else {
        return;
    };
    let others = w
        .reach
        .out_edges(publisher)
        .into_iter()
        .filter(|(to, site, _, _)| *site == Some(EdgeSite::Event(ev)) && *to != s.to)
        .count();
    if others > 0 {
        let d = w.display(publisher);
        w.out.blockers.push((
            "sibling-subscribers",
            format!(
                "the event raised in {d} has {others} other subscriber(s) that run in no \
                 defined order around the route's; what they do between A and B is not followed"
            ),
        ));
    }
}

/// Review r3 item 1, r4 item 1: a routine on either route that can call
/// itself again may run again around A or B, and what its outer run does after
/// the inner one returns is on no part: no route shows it (the route search
/// never repeats a state, so a real stack `M → X → X → A` is listed as
/// `M → X`). Every routine of both routes is checked, the ones holding A and B
/// included: any unlisted real stack repeats a routine that stays on a listed
/// route. Not followed: a blocker, so a `no` is never given on such a route.
fn recursion_on_route<'a>(w: &mut Walk<'a, '_>, ra: &[Step<'a>], rb: &[Step<'a>], k: usize) {
    let root = w.root;
    let on_route: Vec<&'a str> = (0..=ra.len())
        .map(|j| routine_at(root, ra, j))
        .chain((k + 1..=rb.len()).map(|j| routine_at(root, rb, j)))
        .collect();
    for r in on_route {
        if reaches_itself(w, r) {
            let d = w.display(r);
            w.out.blockers.push((
                "recursion-on-route",
                format!(
                    "{d}, on the route to A or B, can call itself again (recursion); what its \
                     other runs do between A and B is not followed"
                ),
            ));
            return;
        }
    }
}

/// Review r2 item 3: a loop above the meet routine (a call site toward M on
/// the shared prefix that can reach itself) runs M again, so the next B after
/// A may be in a later run of M. Not followed: a blocker, so a `no` is never
/// given on such a route.
fn loop_above_meet<'a>(w: &mut Walk<'a, '_>, ra: &[Step<'a>], k: usize) {
    let root = w.root;
    for j in 0..k {
        let r = routine_at(root, ra, j);
        let Some(EdgeSite::Guarded(x)) = ra[j].site else {
            continue;
        };
        // Review r3 item 7: a routine with no row is not known not to loop.
        let Some(row) = w.reach.row_at(r, x) else {
            let d = w.display(r);
            w.out.blockers.push((
                "loop-above-meet",
                format!("{d}, above the meet routine, has no model row; a loop there is not read"),
            ));
            return;
        };
        let looped = if w.ensure_cfg(row) {
            let cfg = &w.cfgs[&row_key(row)];
            let set: HashSet<&str> = [x].into_iter().collect();
            let nodes = cfg.nodes_of(&set);
            let starts: Vec<usize> = nodes.iter().flat_map(|&n| cfg.succs(n)).collect();
            let f = cfg.forward(&starts, &HashSet::new(), false);
            // A node not placed is not known not to loop.
            nodes.is_empty() || nodes.iter().any(|&n| f[n])
        } else {
            true
        };
        if looped {
            let d = w.display(r);
            w.out.blockers.push((
                "loop-above-meet",
                format!(
                    "{d}, above the meet routine, may call toward it again (a loop); the next B \
                     after A may be in a later run, which is not followed"
                ),
            ));
            return;
        }
    }
}

/// Whether `r` can reach itself over the edge graph (every edge kind, guards
/// ignored: wider than the root's live routes, so only ever more `unknown`).
fn reaches_itself<'a>(w: &mut Walk<'a, '_>, r: &'a str) -> bool {
    if let Some(&v) = w.self_reach.get(r) {
        return v;
    }
    let mut seen: HashSet<&'a str> = HashSet::new();
    let mut stack = vec![r];
    let mut found = false;
    'dfs: while let Some(x) = stack.pop() {
        for (to, ..) in w.reach.out_edges(x) {
            if to == r {
                found = true;
                break 'dfs;
            }
            if seen.insert(to) {
                stack.push(to);
            }
        }
    }
    w.self_reach.insert(r, found);
    found
}

/// Review r3 item 2: route step `s` leaves its routine at a record operation
/// or object run that has other edges at the same site (an implicit trigger,
/// a table event's subscribers, an object's several triggers). They run in
/// the same execution of the statement, before or after the route's target,
/// and no part walks them: a blocker. Interface implementers and other
/// alternatives (one of them runs, instead of the route's) are not counted.
fn site_co_edges<'a>(w: &mut Walk<'a, '_>, s: &Step<'a>) {
    let Some(EdgeSite::Guarded(x)) = s.site else {
        return;
    };
    let others = w
        .reach
        .out_edges(s.from)
        .into_iter()
        .filter(|(to, site, kind, _)| {
            *site == Some(EdgeSite::Guarded(x))
                && *to != s.to
                && matches!(
                    *kind,
                    "implicit-trigger" | "table-event" | "object-run-resolved"
                )
        })
        .count();
    if others > 0 {
        let d = w.display(s.from);
        w.out.blockers.push((
            "site-other-edges",
            format!(
                "the statement where the route leaves {d} also runs {others} other trigger(s) or \
                 subscriber(s) around the route's target; what they do between A and B is not \
                 followed"
            ),
        ));
    }
}

/// A route step that may run its target more than once ([`may_repeat`]): a
/// loop the control-flow tree does not show. Not followed: a blocker.
fn repeat_blocker<'a>(w: &mut Walk<'a, '_>, s: &Step<'a>) {
    if let Some((kind, why)) = may_repeat(w, s) {
        let d = w.display(s.from);
        w.out.blockers.push((
            kind,
            format!("the route leaves {d} {why}; its other runs around A and B are not followed"),
        ));
    }
}

/// Review B1: a site of `row` that may matter (an edge leaves it, or a
/// commit or error record names it) but that no node of its control-flow
/// graph holds, e.g. a call in a `case` label, which the control-flow tree
/// does not harvest. Its order is not known: a blocker, never a silent `no`.
fn check_complete<'a>(w: &mut Walk<'a, '_>, routine: &'a str, row: &'a ModelRoutine) {
    let Some(cfg) = w.cfgs.get(&row_key(row)) else {
        return;
    };
    let held: HashSet<&str> = cfg.ids.iter().flatten().copied().collect();
    let edged: HashSet<&str> = w
        .reach
        .out_edges(routine)
        .into_iter()
        .filter_map(|(_, s, _, _)| match s {
            Some(EdgeSite::Guarded(x)) => Some(x),
            _ => None,
        })
        .collect();
    let recorded: HashSet<&str> = w
        .records_of(routine)
        .into_iter()
        .map(|(_, r)| r.terminal.id())
        .collect();
    let matters = |id: &str| edged.contains(id) || recorded.contains(id);
    let mut missing: Vec<&PAnchor> = Vec::new();
    // Review r2 item 4: an unplaced call with no edge and no record (an
    // unresolved call) is read against the root's unresolved cone like a
    // region statement: its order is not known either.
    let mut unplaced: Vec<String> = Vec::new();
    for c in &row.call_sites {
        let ids = [c.id.as_str(), c.operation_id.as_str()];
        if !ids.iter().any(|i| held.contains(i)) {
            unplaced.extend(ids.iter().map(|i| (*i).to_string()));
        }
        if !ids.iter().any(|i| held.contains(i)) && ids.iter().any(|i| matters(i)) {
            missing.push(&c.source_anchor);
        }
    }
    for o in &row.record_operations {
        if !held.contains(o.id.as_str()) && matters(&o.id) {
            missing.push(&o.source_anchor);
        }
    }
    for o in &row.operation_sites {
        if !held.contains(o.id.as_str()) && matters(&o.id) {
            missing.push(&o.source_anchor);
        }
    }
    // A site that shares its place with one the graph holds is held.
    let held_places: Vec<&PAnchor> = row
        .call_sites
        .iter()
        .filter(|c| held.contains(c.id.as_str()) || held.contains(c.operation_id.as_str()))
        .map(|c| &c.source_anchor)
        .chain(
            row.record_operations
                .iter()
                .filter(|o| held.contains(o.id.as_str()))
                .map(|o| &o.source_anchor),
        )
        .collect();
    missing.retain(|a| !held_places.iter().any(|h| same_place(h, a)));
    w.out.region_sites.extend(unplaced);
    if let Some(a) = missing.first() {
        let d = w.display(routine);
        w.out.blockers.push((
            "site-outside-control-flow",
            format!(
                "{d} has {} site(s) its control-flow tree does not place (first at {}:{}, e.g. a \
                 call in a case label); their order is not known",
                missing.len(),
                a.source_unit_id,
                a.start_line + 1
            ),
        ));
    }
}

fn no_tree<'a>(w: &Walk<'a, '_>, r: &str) -> (&'static str, String) {
    (
        "site-routine-no-tree",
        format!(
            "{} has no control-flow tree (loaded from symbols only, or its body is not \
             available); its order cannot be read",
            w.display(r)
        ),
    )
}

/// A's tail in routine `r`: after the site toward A, up to a normal return.
fn tail_part<'a>(
    w: &mut Walk<'a, '_>,
    r: &'a str,
    row: &'a ModelRoutine,
    leave: &[&'a str],
    prefix: Vec<Step<'a>>,
    caught: bool,
    rank: usize,
) -> Option<Part<'a>> {
    if !w.ensure_cfg(row) {
        let b = no_tree(w, r);
        w.out.blockers.push(b);
        return None;
    }
    check_complete(w, r, row);
    let cfg = &w.cfgs[&row_key(row)];
    let lset: HashSet<&str> = leave.iter().copied().collect();
    let ln = cfg.nodes_of(&lset);
    if ln.is_empty() {
        let d = w.display(r);
        w.out.blockers.push((
            "site-not-in-tree",
            format!("{d}'s control-flow tree does not hold the site"),
        ));
        return None;
    }
    let starts: Vec<usize> = ln.iter().flat_map(|&n| cfg.succs(n)).collect();
    // N2: below a trap on A's route an error ends this routine as a return
    // does: the trap catches it and control goes on toward B.
    let mut targets = vec![cfg.ret, cfg.exit];
    if caught {
        targets.push(cfg.fail);
    }
    let none = HashSet::new();
    let f = cfg.forward(&starts, &none, false);
    let bk = cfg.backward(&targets, &none);
    let fnb = cfg.forward(&starts, &none, true);
    let bnb = cfg.backward_m(&targets, &none, true);
    let region: Vec<usize> = (0..f.len()).filter(|&n| f[n] && bk[n]).collect();
    let region_no_back = region
        .iter()
        .copied()
        .filter(|&n| fnb[n] && bnb[n])
        .collect();
    let without = (0..f.len()).filter(|&n| f[n] && !bk[n]).collect();
    let cont = targets.iter().any(|&t| f[t]);
    Some(Part {
        routine: r,
        row,
        kind: "A-tail",
        region,
        region_no_back,
        without,
        starts,
        rank,
        target: targets.into_iter().collect(),
        prefix,
        caught_by_route_trap: caught,
        swallowed_before_b: false,
        cont,
        reaches_b: true,
        b_run: false,
    })
}

/// One region (or without-B) statement's cone, in its part.
struct NodeCone<'a> {
    part: usize,
    node: usize,
    ids: Vec<&'a str>,
    cone: Rc<Cone<'a>>,
    loop_carried: bool,
    /// On a path that ends before B.
    without: bool,
}

/// Fold one pair's parts into the answer.
#[allow(clippy::too_many_arguments)]
fn judge<'a>(
    w: &mut Walk<'a, '_>,
    a: &Resolved<'a>,
    b: &Resolved<'a>,
    parts: &[Part<'a>],
    swallowed: bool,
    ra: &[Step<'a>],
    rb: &[Step<'a>],
    k: usize,
) {
    use crate::engine::l5::commit_scope::{E, N};
    let mut cones: Vec<NodeCone<'a>> = Vec::new();
    // Review N8: per part, the nodes a path reaches from its start without
    // passing a direct class-1 `Commit()` (a write may still be pending there).
    let mut pending: Vec<Vec<bool>> = Vec::new();
    for (pi, p) in parts.iter().enumerate() {
        let node_ids: Vec<(usize, bool, Vec<&'a str>)> = {
            let Some(cfg) = w.cfg(p.row) else {
                pending.push(Vec::new());
                continue;
            };
            p.region
                .iter()
                .map(|&n| (n, false, cfg.ids[n].clone()))
                .chain(p.without.iter().map(|&n| (n, true, cfg.ids[n].clone())))
                .collect()
        };
        let mut commit_nodes: HashSet<usize> = HashSet::new();
        for (n, without, ids) in node_ids {
            if ids.is_empty() {
                continue;
            }
            let ids = expand_ids(p.row, &ids);
            if !without {
                for id in &ids {
                    w.out.region_sites.insert((*id).to_string());
                }
                w.out.region_routines.insert(p.routine.to_string());
            }
            let b0 = w.route_start(&p.prefix);
            let cone = w.cone(p.routine, &ids, !without, b0, without);
            if cone
                .commits
                .iter()
                .any(|(_, c, node, b)| *c == 1 && node.is_none() && Walk::states_of(*b) & N != 0)
            {
                commit_nodes.insert(n);
            }
            cones.push(NodeCone {
                part: pi,
                node: n,
                ids,
                cone,
                loop_carried: !without && !p.region_no_back.contains(&n),
                without,
            });
        }
        let f = w
            .cfgs
            .get(&row_key(p.row))
            .map(|cfg| cfg.forward(&p.starts, &commit_nodes, false))
            .unwrap_or_default();
        // A commit node itself is reached, but what follows it is not.
        let mut f = f;
        for &c in &commit_nodes {
            if let Some(x) = f.get_mut(c) {
                *x = false;
            }
        }
        pending.push(f);
    }
    // Errors, each once, per (part, node).
    let mut errs: Vec<(usize, usize, usize)> = Vec::new();
    for nc in &cones {
        let p = &parts[nc.part];
        if p.caught_by_route_trap {
            continue;
        }
        for (i, labels, node) in &nc.cone.errors {
            let Some(rec) = record(w, *i) else { continue };
            let kind = match rec.effect_type {
                "ERROR_THROW" => "explicit",
                "UI_WINDOW_OPEN" => "window",
                _ => "implicit",
            };
            // N8: a page or report run where no write can be pending (every
            // path to it passes a direct commit) raises no pending-write error.
            if kind == "window" && !pending[nc.part].get(nc.node).copied().unwrap_or(true) {
                continue;
            }
            let mut l = labels.clone();
            if swallowed {
                l.push("swallowed-above-meet");
            }
            if p.swallowed_before_b {
                l.push("swallowed-before-b");
            }
            let hops = nc.cone.steps(*node).len();
            let at = w.at(p.row, &nc.ids, p.rank, hops);
            let row = w.error_row(rec, l, kind, &at);
            let idx = push_error(&mut w.out, row);
            errs.push((nc.part, nc.node, idx));
        }
        // S1: a commit some path runs under `[CommitBehavior(Error)]` is an
        // error (per route above the statement, every path below).
        for (i, class, node, bits) in &nc.cone.commits {
            let Some(rec) = record(w, *i) else { continue };
            if *class != 1 {
                continue;
            }
            if Walk::states_of(*bits) & E != 0 {
                let hops = nc.cone.steps(*node).len();
                let at = w.at(p.row, &nc.ids, p.rank, hops);
                let row = w.error_row(rec, vec!["commit-behavior-error"], "commit-behavior", &at);
                let idx = push_error(&mut w.out, row);
                errs.push((nc.part, nc.node, idx));
            }
        }
    }
    let mut listed: HashSet<usize> = w.out.errors_between.iter().copied().collect();
    for &(_, _, idx) in &errs {
        if listed.insert(idx) {
            w.out.errors_between.push(idx);
        }
    }
    // The errors after node `n` of part `pi`: in later parts, and in the
    // nodes of its part it reaches; those of its own cone, apart.
    // Review r2 cost 1: memoized per region node.
    let mut after_memo: HashMap<(usize, usize), (Vec<usize>, Vec<usize>)> = HashMap::new();
    let after = |w: &Walk<'a, '_>, pi: usize, n: usize| -> (Vec<usize>, Vec<usize>) {
        let f = w
            .cfgs
            .get(&row_key(parts[pi].row))
            .map(|cfg| cfg.forward(&[n], &parts[pi].target, false));
        let (mut out, mut same) = (Vec::new(), Vec::new());
        let (mut seen_out, mut seen_same) = (HashSet::new(), HashSet::new());
        for &(p2, n2, idx) in &errs {
            if p2 == pi && n2 == n {
                if seen_same.insert(idx) {
                    same.push(idx);
                }
            } else if (p2 > pi
                || (p2 == pi
                    && f.as_ref()
                        .is_none_or(|f| f.get(n2).copied().unwrap_or(false))))
                && seen_out.insert(idx)
            {
                out.push(idx);
            }
        }
        out.sort_by(|x, y| w.out.errors[*x].near.cmp(&w.out.errors[*y].near));
        (out, same)
    };
    // Commits.
    for nc in &cones {
        let p = &parts[nc.part];
        for (i, class, node, bits) in &nc.cone.commits {
            let Some(rec) = record(w, *i) else { continue };
            let steps = nc.cone.steps(*node);
            let hops = steps.len();
            let mut full = p.prefix.clone();
            full.extend(steps.iter().copied());
            let mut class = *class;
            if class == 1 {
                if Walk::states_of(*bits) & N == 0 {
                    continue;
                }
                if p.b_run {
                    class = 2;
                }
            }
            let mut labels = nc.cone.labels(*node);
            if nc.loop_carried {
                labels.push("loop-carried");
            }
            let kind = if class == 2 && steps.is_empty() {
                "run-commit"
            } else {
                "commit"
            };
            let at = w.at(p.row, &nc.ids, p.rank, hops);
            let mut row = w.commit_row(rec, class, kind, p.kind, labels, full, &at);
            if nc.without {
                if class == 1 {
                    row.ends = w
                        .cfgs
                        .get(&row_key(p.row))
                        .map(|cfg| cfg.ends(nc.node, &p.target))
                        .unwrap_or_default();
                    push_commit(&mut w.out, WITHOUT_B, row);
                }
            } else if class == 2 {
                push_commit(&mut w.out, OWN, row);
            } else if !p.reaches_b {
                row.ends = vec!["b-never-after-a"];
                push_commit(&mut w.out, WITHOUT_B, row);
            } else {
                let (e, s) = after_memo
                    .entry((nc.part, nc.node))
                    .or_insert_with(|| after(w, nc.part, nc.node))
                    .clone();
                row.errors_after = e;
                row.errors_same_cone = s;
                push_commit(&mut w.out, BETWEEN, row);
            }
        }
    }
    // N1: a checked run (or an isolated event) on A's own route below M:
    // its commit at the successful return makes A durable.
    for (j, s) in ra.iter().enumerate().skip(k) {
        let Some(t) = trap_of(w, s) else { continue };
        let kind = match t {
            TrapKind::CheckedRun => "run-commit-of-A",
            TrapKind::IsolatedEvent => "isolated-commit-of-A",
            _ => continue,
        };
        let Some(part) = parts
            .iter()
            .position(|p| p.routine == routine_at(w.root, ra, j) && p.kind != "B-head")
        else {
            continue;
        };
        let at = match s.site {
            Some(EdgeSite::Guarded(x)) => x,
            _ => "",
        };
        let row_r = w.reach.row_at(s.from, at);
        let anchor = row_r.and_then(|r| anchor_in(r, at));
        let path = w.group_paths.path_of(w.reach, &ra[..j], at);
        let sites = route_sites(w.reach, w.root, &path, (row_r, anchor));
        let mut labels = vec![kind];
        if t == TrapKind::IsolatedEvent {
            labels.push("if-nothing-pending-at-the-raise");
        }
        let place = row_r.map(|r| w.at(r, &[at], parts[part].rank, 0));
        let file = anchor.map(|x| x.source_unit_id.to_string());
        let line = anchor.map(|x| x.start_line);
        let mut errors: Vec<usize> = Vec::new();
        let mut seen = HashSet::new();
        for &(pi, _, idx) in &errs {
            if pi >= part && seen.insert(idx) {
                errors.push(idx);
            }
        }
        errors.sort_by(|x, y| w.out.errors[*x].near.cmp(&w.out.errors[*y].near));
        let reaches = parts[part].reaches_b;
        let row = CommitRow {
            near: place
                .as_ref()
                .map_or((0, part, 0, 0, false, None, None), |p| p.near(&file, line)),
            file,
            line,
            column: anchor.map(|x| x.start_column),
            op_id: at.to_string(),
            class: 1,
            kind,
            part: "A-tail",
            labels,
            ends: if reaches {
                Vec::new()
            } else {
                vec!["b-never-after-a"]
            },
            route: Some(SliceRoute { path, sites }),
            errors_after: if reaches { errors } else { Vec::new() },
            errors_same_cone: Vec::new(),
            statement: place.and_then(|p| p.statement),
            hops: 0,
        };
        if reaches {
            push_commit(&mut w.out, BETWEEN, row);
        } else {
            push_commit(&mut w.out, WITHOUT_B, row);
        }
    }
    // Commits inside A and inside B (S3: not region cones).
    let all: Vec<usize> = {
        let mut v: Vec<usize> = Vec::new();
        let mut seen = HashSet::new();
        for &(_, _, idx) in &errs {
            if seen.insert(idx) {
                v.push(idx);
            }
        }
        v.sort_by(|x, y| w.out.errors[*x].near.cmp(&w.out.errors[*y].near));
        v
    };
    for (s, inside) in [(a, true), (b, false)] {
        let owner = s.owner;
        let ids = s.ids.clone();
        // Review r3 item 7: each site's cone starts under its own route.
        let b0 = w.route_start(if inside { ra } else { rb });
        let cone = w.cone(owner, &ids, false, b0, false);
        let Some(row_s) = w.reach.row_at(owner, ids.first().copied().unwrap_or("")) else {
            continue;
        };
        for (i, class, node, bits) in &cone.commits {
            let Some(rec) = record(w, *i) else { continue };
            if *class == 1 && Walk::states_of(*bits) & N == 0 {
                continue;
            }
            if node.is_none() && rec.owner == owner && ids.contains(&rec.terminal.id()) {
                // The site itself is a commit statement: it is not inside it.
                continue;
            }
            let steps = cone.steps(*node);
            let mut full = if inside { ra.to_vec() } else { Vec::new() };
            full.extend(steps.iter().copied());
            let part = if inside { "inside-A" } else { "inside-B" };
            let at = w.at(
                row_s,
                &ids,
                if inside { 0 } else { usize::MAX },
                steps.len(),
            );
            let mut row = w.commit_row(rec, *class, "commit", part, cone.labels(*node), full, &at);
            if inside && *class == 1 {
                // Review r2 item 7: an index list shared by every inside-A row.
                row.errors_after.clone_from(&all);
            }
            if inside {
                push_commit(&mut w.out, INSIDE_A, row);
            } else {
                push_commit(&mut w.out, INSIDE_B, row);
            }
        }
    }
}

/// A statement's ids plus the ids at the same place (its operation, a record
/// operation, an operation site).
fn expand_ids<'a>(row: &'a ModelRoutine, ids: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = ids.to_vec();
    for id in ids {
        if let Some(a) = anchor_in(row, id) {
            out.extend(ids_at(row, a));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn record<'a, 'w>(w: &Walk<'a, 'w>, fact: usize) -> Option<&'w TerminalRecord<'a>> {
    w.by_fact.get(&fact).copied()
}

/// Add `e` to the error table once (by operation id, review COST: an index,
/// not a scan); a repeat merges its labels and keeps the nearer key.
fn push_error(out: &mut BetweenOut, e: ErrorRow) -> usize {
    match out.err_index.get(&e.op_id) {
        Some(&i) => {
            let x = &mut out.errors[i];
            for l in e.labels {
                if !x.labels.contains(&l) {
                    x.labels.push(l);
                }
            }
            if e.near < x.near {
                x.near = e.near;
                x.statement = e.statement;
                x.hops = e.hops;
            }
            i
        }
        None => {
            out.err_index.insert(e.op_id.clone(), out.errors.len());
            out.errors.push(e);
            out.errors.len() - 1
        }
    }
}

const BETWEEN: u8 = 0;
const INSIDE_A: u8 = 1;
const INSIDE_B: u8 = 2;
const WITHOUT_B: u8 = 3;
const OWN: u8 = 4;

/// Add `c` to list `list` once per (operation, kind, class), found by an
/// index (review r2 cost 2), merging labels and errors and keeping the nearer
/// route.
fn push_commit(out: &mut BetweenOut, list: u8, c: CommitRow) {
    let key = (list, c.op_id.clone(), c.kind, c.class);
    let v = match list {
        BETWEEN => &mut out.between,
        INSIDE_A => &mut out.inside_a,
        INSIDE_B => &mut out.inside_b,
        WITHOUT_B => &mut out.without_b,
        _ => &mut out.own_transaction,
    };
    match out.commit_index.get(&key).copied() {
        Some(i) => {
            let x = &mut v[i];
            for l in c.labels {
                if !x.labels.contains(&l) {
                    x.labels.push(l);
                }
            }
            let seen: HashSet<usize> = x.errors_after.iter().copied().collect();
            x.errors_after
                .extend(c.errors_after.into_iter().filter(|e| !seen.contains(e)));
            if c.near < x.near {
                x.near = c.near;
                x.statement = c.statement;
                x.hops = c.hops;
                x.route = c.route;
            }
        }
        None => {
            v.push(c);
            let i = v.len() - 1;
            out.commit_index.insert(key, i);
        }
    }
}

/// Sort every list nearest first (review output guidance), deterministically.
fn finish(out: &mut BetweenOut) {
    out.commit_index.clear();
    // A commit that is between A and B on some route pair is not also listed
    // as ending before B (another pair's path): one fact, its strongest list.
    let between: HashSet<(String, &'static str)> = out
        .between
        .iter()
        .map(|c| (c.op_id.clone(), c.kind))
        .collect();
    out.without_b
        .retain(|c| !between.contains(&(c.op_id.clone(), c.kind)));
    let near: Vec<NearKey> = out.errors.iter().map(|e| e.near.clone()).collect();
    for v in [
        &mut out.between,
        &mut out.inside_a,
        &mut out.inside_b,
        &mut out.without_b,
        &mut out.own_transaction,
    ] {
        v.sort_by(|x, y| (&x.near, &x.op_id, x.kind).cmp(&(&y.near, &y.op_id, y.kind)));
        for c in v.iter_mut() {
            c.errors_after.sort_by(|a, b| near[*a].cmp(&near[*b]));
            c.labels.sort_unstable();
        }
    }
    for e in &mut out.errors {
        e.labels.sort_unstable();
    }
    out.errors_between.sort_by(|a, b| near[*a].cmp(&near[*b]));
    out.blockers.sort();
    out.blockers.dedup();
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn place(file: &Option<String>, line: Option<u32>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{}", crate::engine::line_base::shown(l)),
        (Some(f), None) => f.clone(),
        _ => "(no place)".to_string(),
    }
}

fn is_dep(file: &Option<String>) -> bool {
    file.as_deref().is_some_and(|f| f.starts_with("dep:"))
}

/// The labels that depend on runtime state (shown once, as a class).
const CONDITIONAL: &[&str] = &[
    "loop-carried",
    "trap-may-fail-pending",
    "if-nothing-pending-at-the-raise",
];

fn conditional(labels: &[&'static str]) -> Vec<&'static str> {
    labels
        .iter()
        .copied()
        .filter(|l| {
            CONDITIONAL.contains(l)
                || l.starts_with("page-run")
                || l.starts_with("runmodal")
                || l.starts_with("unmeasured")
        })
        .collect()
}

fn error_value(i: usize, e: &ErrorRow) -> serde_json::Value {
    serde_json::json!({
        "id": i,
        "file": e.file,
        "line": e.line.map(crate::engine::line_base::shown),
        "column": e.column,
        "op": e.op,
        "operationId": e.op_id,
        "kind": e.kind,
        "statement": e.statement,
        "hops": e.hops,
        "labels": e.labels,
        "conditional": conditional(&e.labels),
    })
}

/// Counts of `ids` (error table indices): total, workspace, dependency.
fn split(errors: &[ErrorRow], ids: &[usize]) -> serde_json::Value {
    let dep = ids.iter().filter(|i| is_dep(&errors[**i].file)).count();
    serde_json::json!({"count": ids.len(), "workspace": ids.len() - dep, "dependency": dep})
}

fn commit_value(errors: &[ErrorRow], c: &CommitRow) -> serde_json::Value {
    let mut after = split(errors, &c.errors_after);
    after["nearest"] = c.errors_after.first().copied().into();
    after["sameCone"] = c.errors_same_cone.clone().into();
    serde_json::json!({
        "file": c.file,
        "line": c.line.map(crate::engine::line_base::shown),
        "column": c.column,
        "operationId": c.op_id,
        "class": c.class,
        "kind": c.kind,
        "part": c.part,
        "statement": c.statement,
        "hops": c.hops,
        "labels": c.labels,
        "conditional": conditional(&c.labels),
        "ends": c.ends,
        "route": c.route.as_ref().map(route_value),
        "errorsAfterCommit": after,
    })
}

fn site_value(s: &Option<SiteDesc>, route: &Option<SliceRoute>) -> serde_json::Value {
    match s {
        Some(s) => serde_json::json!({
            "routine": s.routine,
            "routineDisplay": s.routine_display,
            "file": s.file,
            "line": crate::engine::line_base::shown(s.line),
            "column": s.column,
            "op": s.op,
            "route": route.as_ref().map(route_value),
        }),
        None => serde_json::Value::Null,
    }
}

/// Per class, workspace and dependency counts of a commit list.
fn class_counts<'r>(v: impl IntoIterator<Item = &'r CommitRow> + Clone) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for class in [1u8, 2] {
        let rows: Vec<&CommitRow> = v.clone().into_iter().filter(|c| c.class == class).collect();
        if rows.is_empty() {
            continue;
        }
        let dep = rows.iter().filter(|c| is_dep(&c.file)).count();
        m.insert(
            format!("class{class}"),
            serde_json::json!({"count": rows.len(), "workspace": rows.len() - dep, "dependency": dep}),
        );
    }
    serde_json::Value::Object(m)
}

/// The region statements the `between` rows start from, nearest first:
/// (statement, part, its first row's key, rows).
fn statements(v: &[CommitRow]) -> Vec<(String, &'static str, &CommitRow, Vec<&CommitRow>)> {
    let mut out: Vec<(String, &'static str, &CommitRow, Vec<&CommitRow>)> = Vec::new();
    for c in v {
        let s = c
            .statement
            .clone()
            .unwrap_or_else(|| "(no place)".to_string());
        match out.iter_mut().find(|x| x.0 == s) {
            Some(x) => x.3.push(c),
            None => out.push((s, c.part, c, vec![c])),
        }
    }
    out
}

impl BetweenOut {
    /// The `payload.slice` value of a `commit-between` answer.
    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        let e = &self.errors;
        let list = |v: &[CommitRow]| v.iter().map(|c| commit_value(e, c)).collect::<Vec<_>>();
        let stmts: Vec<serde_json::Value> = statements(&self.between)
            .into_iter()
            .enumerate()
            .map(|(order, (s, part, first, rows))| {
                serde_json::json!({
                    "site": s,
                    "part": part,
                    "order": order,
                    "commits": class_counts(rows.iter().copied()),
                    "nearestHops": first.hops,
                })
            })
            .collect();
        let nearest: serde_json::Map<String, serde_json::Value> = [
            ("between", self.between.first()),
            ("commitsInsideA", self.inside_a.first()),
            ("commitsInsideB", self.inside_b.first()),
            ("commitsWithoutB", self.without_b.first()),
            ("ownTransactionCommits", self.own_transaction.first()),
        ]
        .into_iter()
        .filter_map(|(k, c)| c.map(|c| (k.to_string(), commit_value(e, c))))
        .collect();
        serde_json::json!({
            "question": "commit-between",
            "after": site_value(&self.after, &self.route_a),
            "before": site_value(&self.before, &self.route_b),
            "verdictBasis": "class-1 commits (ending the caller's transaction) that may run after \
                             A returns and before the next B",
            "summary": {
                "between": class_counts(&self.between),
                "commitsInsideA": class_counts(&self.inside_a),
                "commitsInsideB": class_counts(&self.inside_b),
                "commitsWithoutB": class_counts(&self.without_b),
                "ownTransactionCommits": class_counts(&self.own_transaction),
                "errorsBetween": split(e, &self.errors_between),
            },
            "nearest": nearest,
            "statements": stmts,
            "between": list(&self.between),
            "commitsInsideA": list(&self.inside_a),
            "commitsInsideB": list(&self.inside_b),
            "commitsWithoutB": list(&self.without_b),
            "ownTransactionCommits": list(&self.own_transaction),
            "errors": e.iter().enumerate().map(|(i, x)| error_value(i, x)).collect::<Vec<_>>(),
            "errorsBetween": self.errors_between,
            "relation": self.relation,
            "blockers": self.blockers.iter().map(|(k, d)| serde_json::json!({"kind": k, "detail": d})).collect::<Vec<_>>(),
            "pairsExamined": self.pairs_examined,
            "staticLabel": crate::engine::l5::prove_slice::STATIC_LABEL,
        })
    }

    fn error_line(&self, i: usize) -> String {
        let e = &self.errors[i];
        format!(
            "{} {}{}",
            place(&e.file, e.line),
            e.op.as_deref().unwrap_or("error"),
            if e.labels.is_empty() {
                String::new()
            } else {
                format!(" ({})", e.labels.join(", "))
            }
        )
    }

    /// One witness row: the commit, its route and guards, and its nearest
    /// error with the counts of the rest.
    fn witness(&self, c: &CommitRow, lines: &mut Vec<String>) {
        lines.push(format!(
            "    nearest: [COMMIT class {}] {} ({}, {} hop{}{})",
            c.class,
            place(&c.file, c.line),
            c.part,
            c.hops,
            if c.hops == 1 { "" } else { "s" },
            if c.labels.is_empty() {
                String::new()
            } else {
                format!("; {}", c.labels.join(", "))
            }
        ));
        if let Some(r) = &c.route {
            lines.push(format!("      route: {}", r.path.hops.join(" → ")));
            let guards: Vec<String> = r
                .sites
                .iter()
                .flat_map(|s| s.shown.iter().map(|g| g.describe()))
                .collect();
            if !guards.is_empty() {
                lines.push(format!(
                    "      guards (selected, not all conditions): {}",
                    guards.join("; ")
                ));
            }
        }
        if let Some(&first) = c.errors_after.first() {
            let dep = c
                .errors_after
                .iter()
                .filter(|i| is_dep(&self.errors[**i].file))
                .count();
            lines.push(format!(
                "      then may fail at {} — {} workspace, {} dependency errors follow (--detail)",
                self.error_line(first),
                c.errors_after.len() - dep,
                dep
            ));
        }
    }

    fn rows(&self, title: &str, rows: &[CommitRow], detail: bool, lines: &mut Vec<String>) {
        if rows.is_empty() {
            return;
        }
        let c1 = rows.iter().filter(|c| c.class == 1).count();
        lines.push(format!(
            "  {title}: {} (class 1: {c1}, class 2: {})",
            rows.len(),
            rows.len() - c1
        ));
        let shown = if detail { rows.len() } else { 1 };
        for c in rows.iter().take(shown) {
            self.witness(c, lines);
            if !c.ends.is_empty() {
                lines.push(format!("      ends by {}", c.ends.join(" or ")));
            }
        }
    }

    /// The human lines of a `commit-between` answer (design §5.4's order).
    #[must_use]
    pub fn human_lines(&self, detail: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let site = |s: &Option<SiteDesc>| match s {
            Some(s) => format!(
                "{}:{} {} in {}",
                s.file,
                crate::engine::line_base::shown(s.line),
                s.op.as_deref().unwrap_or("site"),
                s.routine_display
            ),
            None => "(not resolved)".to_string(),
        };
        lines.push(format!("  A (after):  {}", site(&self.after)));
        lines.push(format!("  B (before): {}", site(&self.before)));
        lines.push(format!(
            "  route pairs examined: {}; {}",
            self.pairs_examined,
            crate::engine::l5::prove_slice::STATIC_LABEL
        ));
        if let Some(r) = self.relation {
            lines.push(format!("  relation: {r}"));
        }
        // Between: the nearest witness, then per statement between A and B.
        if !self.between.is_empty() {
            let direct = self.between.iter().filter(|c| c.hops == 0).count();
            let stmts = statements(&self.between);
            // Statements that start a cone (hops > 0), not the direct rows'.
            let through: BTreeSet<&str> = self
                .between
                .iter()
                .filter(|c| c.hops > 0)
                .filter_map(|c| c.statement.as_deref())
                .collect();
            lines.push(format!(
                "  between A and B: {} commits: {direct} direct, {} through {} statements",
                self.between.len(),
                self.between.len() - direct,
                through.len()
            ));
            self.witness(&self.between[0], &mut lines);
            lines.push("  by statement between A and B:".to_string());
            let shown = if detail { stmts.len() } else { HUMAN_ROWS };
            for (s, part, first, rows) in stmts.iter().take(shown) {
                let dep = rows.iter().filter(|c| is_dep(&c.file)).count();
                let mut kinds: Vec<&str> = rows.iter().flat_map(|c| c.labels.clone()).collect();
                kinds.sort_unstable();
                kinds.dedup();
                lines.push(format!(
                    "    {s} ({part}): {} commits ({} workspace, {dep} dependency; {}), nearest {} \
                     at {} hop{}",
                    rows.len(),
                    rows.len() - dep,
                    if kinds.is_empty() {
                        "direct".to_string()
                    } else {
                        kinds.join(", ")
                    },
                    place(&first.file, first.line),
                    first.hops,
                    if first.hops == 1 { "" } else { "s" }
                ));
                if detail {
                    for c in rows {
                        self.witness(c, &mut lines);
                    }
                }
            }
            if stmts.len() > shown {
                lines.push(format!(
                    "    … {} more statements (--detail lists them)",
                    stmts.len() - shown
                ));
            }
        }
        // Errors after A, before B.
        if !self.errors_between.is_empty() {
            let count = |kind: &str, dep: bool| {
                self.errors_between
                    .iter()
                    .filter(|i| {
                        let e = &self.errors[**i];
                        (e.kind == kind) && is_dep(&e.file) == dep
                    })
                    .count()
            };
            lines.push(format!(
                "  errors after A before B: {} — explicit {} workspace / {} dependency; implicit {} \
                 / {}; other {}",
                self.errors_between.len(),
                count("explicit", false),
                count("explicit", true),
                count("implicit", false),
                count("implicit", true),
                self.errors_between.len()
                    - count("explicit", false)
                    - count("explicit", true)
                    - count("implicit", false)
                    - count("implicit", true),
            ));
            let first: Vec<usize> = self
                .errors_between
                .iter()
                .copied()
                .filter(|i| {
                    let e = &self.errors[*i];
                    e.kind == "explicit" && !is_dep(&e.file)
                })
                .take(if detail { usize::MAX } else { HUMAN_ROWS })
                .collect();
            for i in first {
                lines.push(format!("    {}", self.error_line(i)));
            }
        }
        // Conditional facts, shown once.
        let mut cond: BTreeMap<&str, usize> = BTreeMap::new();
        for c in self.between.iter().chain(&self.inside_a) {
            for l in conditional(&c.labels) {
                *cond.entry(l).or_default() += 1;
            }
        }
        for i in &self.errors_between {
            for l in conditional(&self.errors[*i].labels) {
                *cond.entry(l).or_default() += 1;
            }
        }
        if !cond.is_empty() {
            lines.push(format!(
                "  conditional (depends on runtime state), shown once: {}",
                cond.iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // Inside A, inside B: during the call, not between.
        for (rows, which, flag) in [
            (&self.inside_a, "A", "--after"),
            (&self.inside_b, "B", "--before"),
        ] {
            if rows.is_empty() {
                continue;
            }
            let c1 = rows.iter().filter(|c| c.class == 1).count();
            let deeper = rows[0]
                .route
                .as_ref()
                .and_then(|r| r.sites.get(1).or(r.sites.first()))
                .map(|s| place(&s.file, s.line))
                .unwrap_or_else(|| "a site inside it".to_string());
            lines.push(format!(
                "  inside {which}: {} commits (class 1: {c1}, class 2: {}) — they run during {which}, \
                 not between; name {deeper} as {flag} to order them",
                rows.len(),
                rows.len() - c1
            ));
            if let Some(c) = rows
                .iter()
                .find(|c| c.class == 1 && !c.errors_after.is_empty())
            {
                lines.push(format!(
                    "    inside {which}: a commit, then an error before B (not between):"
                ));
                self.witness(c, &mut lines);
            }
        }
        self.rows(
            "commits after A on paths that end before B",
            &self.without_b,
            detail,
            &mut lines,
        );
        self.rows(
            "own-transaction commits (class 2: never make A's pending write durable)",
            &self.own_transaction,
            detail,
            &mut lines,
        );
        lines
    }
}

#[cfg(test)]
#[path = "commit_between_tests.rs"]
mod tests;
