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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

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

/// One error row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ErrorRow {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub op_id: String,
    pub op: Option<String>,
    pub labels: Vec<&'static str>,
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
    /// `commitsWithoutB`: how the paths end (`exit`, `return`, `error`).
    pub ends: Vec<&'static str>,
    pub route: Option<SliceRoute>,
    pub errors_after: Vec<ErrorRow>,
}

/// The `commit-between` answer of one root.
#[derive(Debug, Clone, Default)]
pub struct BetweenOut {
    /// A site could not be resolved: the usage error (exit 1).
    pub refused: Option<String>,
    pub after: Option<SiteDesc>,
    pub before: Option<SiteDesc>,
    pub between: Vec<CommitRow>,
    pub inside_a: Vec<CommitRow>,
    pub inside_b: Vec<CommitRow>,
    pub without_b: Vec<CommitRow>,
    pub own_transaction: Vec<CommitRow>,
    pub errors_between: Vec<ErrorRow>,
    /// `b-never-after-a`, `not-in-one-build`, `a-dead-under-literals`,
    /// `b-dead-under-literals` (for a `no`).
    pub relation: Option<&'static str>,
    /// What could not be followed: (kind, detail).
    pub blockers: Vec<(&'static str, String)>,
    pub pairs_examined: usize,
    /// The region's statement ids and the routines its cones reach, read by
    /// prove against the root's unresolved cone (design §5.5).
    pub region_sites: BTreeSet<String>,
    pub region_routines: BTreeSet<String>,
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
        self.list(leaves, body)
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
    target: HashSet<usize>,
    /// The route from the root to this part's routine.
    prefix: Vec<Step<'a>>,
    /// Errors here are caught by a trap on A's route below M (N2), or come
    /// before B behind a trap on B's route.
    caught_by_route_trap: bool,
    swallowed_before_b: bool,
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
    commit_scopes: Option<crate::engine::l5::commit_scope::CbGraph<'a>>,
    group_paths: GroupPaths<'a>,
    out: BetweenOut,
    names: HashMap<&'a str, &'a str>,
}

/// What one cone walk found.
#[derive(Default)]
struct Cone<'a> {
    /// (record, class, labels, steps from the region statement).
    commits: Vec<(usize, u8, Vec<&'static str>, Vec<Step<'a>>)>,
    errors: Vec<(usize, Vec<&'static str>)>,
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

    /// The commit states of `owner` from the root (`commit_scope`): N, I, E bits.
    fn commit_states(&self, owner: &str) -> u8 {
        self.commit_scopes
            .as_ref()
            .filter(|cb| !cb.root_unaffected(self.root))
            .and_then(|cb| cb.states_at(self.root, owner))
            .unwrap_or(crate::engine::l5::commit_scope::N)
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
    fn cone(&mut self, routine: &'a str, ids: &[&'a str]) -> Cone<'a> {
        let mut cone = Cone::default();
        let idset: HashSet<&str> = ids.iter().copied().collect();
        // The statement's own records.
        for (i, rec) in self.records_of(routine) {
            if !idset.contains(rec.terminal.id()) {
                continue;
            }
            cone_record(self, &mut cone, i, rec, false, None, Vec::new(), Vec::new());
        }
        // (routine, crossed a checked run, the trap crossed)
        type St<'s> = (&'s str, bool, Option<TrapKind>);
        let mut seen: HashSet<St<'a>> = HashSet::new();
        let mut q: VecDeque<(St<'a>, Vec<Step<'a>>, Vec<&'static str>)> = VecDeque::new();
        for (to, site, kind, trap) in self.reach.out_edges(routine) {
            let at = match site {
                Some(EdgeSite::Guarded(s)) => s,
                _ => continue,
            };
            if !idset.contains(at) {
                continue;
            }
            let st = (to, trap == Some(TrapKind::CheckedRun), trap);
            let step = Step {
                from: routine,
                to,
                site,
                kind,
            };
            if seen.insert(st) {
                q.push_back((st, vec![step], vec![label_of(kind, trap)]));
            }
        }
        while let Some(((r, run, trap), steps, labels)) = q.pop_front() {
            if seen.len() > CONE_STATE_CAP {
                self.out.blockers.push((
                    "cone-walk-capped",
                    format!(
                        "a region statement's cone passed {CONE_STATE_CAP} routine states; the \
                         walk stopped"
                    ),
                ));
                break;
            }
            self.out.region_routines.insert(r.to_string());
            for (i, rec) in self.records_of(r) {
                cone_record(
                    self,
                    &mut cone,
                    i,
                    rec,
                    run,
                    trap,
                    steps.clone(),
                    labels.clone(),
                );
            }
            for (to, site, kind, t) in self.reach.out_edges(r) {
                let run2 = run || t == Some(TrapKind::CheckedRun);
                let trap2 = trap.or(t);
                let st = (to, run2, trap2);
                if seen.insert(st) {
                    let mut s2 = steps.clone();
                    s2.push(Step {
                        from: r,
                        to,
                        site,
                        kind,
                    });
                    let mut l2 = labels.clone();
                    let l = label_of(kind, t);
                    if !l2.contains(&l) {
                        l2.push(l);
                    }
                    q.push_back((st, s2, l2));
                }
            }
        }
        cone
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

    fn error_row(&self, rec: &TerminalRecord<'a>, labels: Vec<&'static str>) -> ErrorRow {
        let id = rec.terminal.id();
        let row = self.reach.row_at(rec.owner, id);
        let anchor = row.and_then(|r| anchor_in(r, id));
        let table_names: HashMap<&str, &str> = HashMap::new();
        let op = row.and_then(|r| site_facts(r, id, &table_names).op_name);
        ErrorRow {
            file: anchor.map(|a| a.source_unit_id.to_string()),
            line: anchor.map(|a| a.start_line),
            column: anchor.map(|a| a.start_column),
            op_id: id.to_string(),
            op,
            labels,
        }
    }

    fn commit_row(
        &self,
        rec: &TerminalRecord<'a>,
        class: u8,
        kind: &'static str,
        part: &'static str,
        labels: Vec<&'static str>,
        steps: Vec<Step<'a>>,
    ) -> CommitRow {
        let id = rec.terminal.id();
        let row = self.reach.row_at(rec.owner, id);
        let anchor = row.and_then(|r| anchor_in(r, id));
        let path = self.group_paths.path_of(self.reach, &steps, id);
        let sites = route_sites(self.reach, self.root, &path, (row, anchor));
        CommitRow {
            file: anchor.map(|a| a.source_unit_id.to_string()),
            line: anchor.map(|a| a.start_line),
            column: anchor.map(|a| a.start_column),
            op_id: id.to_string(),
            class,
            kind,
            part,
            labels,
            ends: Vec::new(),
            route: Some(SliceRoute { path, sites }),
            errors_after: Vec::new(),
        }
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
    steps: Vec<Step<'a>>,
    labels: Vec<&'static str>,
) {
    let id = rec.terminal.id();
    if id.is_empty() || !w.live(id) {
        return;
    }
    if rec.effect_type == "COMMIT" {
        if w.is_run_commit(rec) {
            cone.commits.push((i, 2, labels, steps));
            return;
        }
        let states = w.commit_states(rec.owner);
        if states & crate::engine::l5::commit_scope::E != 0 {
            cone.errors.push((i, vec!["commit-behavior-error"]));
        }
        if states & crate::engine::l5::commit_scope::N == 0 {
            return;
        }
        cone.commits
            .push((i, if run { 2 } else { 1 }, labels, steps));
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
            cone.errors.push((i, l));
        }
        None => {
            if w.is_run_error(rec) {
                l.push("run-error-rolls-back-a");
            }
            cone.errors.push((i, l));
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
            let found = reach.routes(&graph, id, ROUTE_CAP, ROUTE_EDGE_BUDGET, cancel);
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
    if ra_all.is_empty() {
        w.out.relation = Some("a-dead-under-literals");
        return w.out;
    }
    if rb_all.is_empty() {
        w.out.relation = Some("b-dead-under-literals");
        return w.out;
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
        let Some(row) = w.reach.row_at(r, leave.first().copied().unwrap_or("")) else {
            continue;
        };
        let caught = a_route_trap_at.is_some_and(|j| j < i);
        match tail_part(w, r, row, &leave, ra[..i].to_vec(), caught) {
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
                target,
                prefix: ra[..k].to_vec(),
                caught_by_route_trap: false,
                swallowed_before_b: false,
            };
            parts.push(p);
            judge(w, a, b, &parts, swallowed, ra, k);
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
            target,
            prefix: ra[..k].to_vec(),
            caught_by_route_trap: false,
            swallowed_before_b: false,
        });
    }
    // B's head, shallowest first.
    let mut b_trap = false;
    for i in k + 1..=rb.len() {
        b_trap |= trap_of(w, &rb[i - 1]).is_some_and(TrapKind::definite);
        let r = rb[i - 1].to;
        let leave = leave_ids(rb, i, b);
        let Some(row) = w.reach.row_at(r, leave.first().copied().unwrap_or("")) else {
            continue;
        };
        let sure_here = b_sure(w, rb, i, b);
        if !w.ensure_cfg(row) {
            let t = no_tree(w, r);
            w.out.blockers.push(t);
            return PairEnd::Judged;
        }
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
            target,
            prefix: rb[..i].to_vec(),
            caught_by_route_trap: false,
            swallowed_before_b: b_trap,
        });
    }
    judge(w, a, b, &parts, swallowed, ra, k);
    PairEnd::Judged
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
) -> Option<Part<'a>> {
    if !w.ensure_cfg(row) {
        let b = no_tree(w, r);
        w.out.blockers.push(b);
        return None;
    }
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
    Some(Part {
        routine: r,
        row,
        kind: "A-tail",
        region,
        region_no_back,
        without,
        target: targets.into_iter().collect(),
        prefix,
        caught_by_route_trap: caught,
        swallowed_before_b: false,
    })
}

/// Fold one pair's parts into the answer.
fn judge<'a>(
    w: &mut Walk<'a, '_>,
    a: &Resolved<'a>,
    b: &Resolved<'a>,
    parts: &[Part<'a>],
    swallowed: bool,
    ra: &[Step<'a>],
    k: usize,
) {
    // Per part, per region node: its cone.
    struct NodeCone<'a> {
        part: usize,
        node: usize,
        cone: Cone<'a>,
        loop_carried: bool,
    }
    let mut cones: Vec<NodeCone<'a>> = Vec::new();
    let mut withouts: Vec<(usize, usize, Cone<'a>)> = Vec::new();
    for (pi, p) in parts.iter().enumerate() {
        let node_ids: Vec<(usize, Vec<&'a str>)> = {
            let Some(cfg) = w.cfg(p.row) else { continue };
            p.region
                .iter()
                .map(|&n| (n, cfg.ids[n].clone()))
                .chain(
                    p.without
                        .iter()
                        .map(|&n| (usize::MAX - n, cfg.ids[n].clone())),
                )
                .collect()
        };
        for (n, ids) in node_ids {
            if ids.is_empty() {
                continue;
            }
            let ids = expand_ids(p.row, &ids);
            if n > usize::MAX / 2 {
                let cone = w.cone(p.routine, &ids);
                withouts.push((pi, usize::MAX - n, cone));
                continue;
            }
            for id in &ids {
                w.out.region_sites.insert((*id).to_string());
            }
            w.out.region_routines.insert(p.routine.to_string());
            let cone = w.cone(p.routine, &ids);
            cones.push(NodeCone {
                part: pi,
                node: n,
                cone,
                loop_carried: !p.region_no_back.contains(&n),
            });
        }
    }
    // Errors per (part, node), with labels.
    let mut errs: Vec<(usize, usize, ErrorRow)> = Vec::new();
    // The region's errors, and those on paths that end before B (an error
    // there is what stops the code before B: the dangerous case).
    let sources: Vec<(usize, usize, &Cone<'a>)> = cones
        .iter()
        .map(|nc| (nc.part, nc.node, &nc.cone))
        .chain(withouts.iter().map(|(pi, n, c)| (*pi, *n, c)))
        .collect();
    for (part, node, cone) in sources {
        let p = &parts[part];
        if p.caught_by_route_trap {
            continue;
        }
        let nc = (part, node);
        for (i, labels) in &cone.errors {
            let Some(rec) = record(w, *i) else { continue };
            let mut l = labels.clone();
            if swallowed {
                l.push("swallowed-above-meet");
            }
            if p.swallowed_before_b {
                l.push("swallowed-before-b");
            }
            let row = w.error_row(rec, l);
            errs.push((nc.0, nc.1, row));
        }
    }
    for (_, _, e) in &errs {
        push_error(&mut w.out.errors_between, e.clone());
    }
    // Errors after a node: in its part, from the nodes it reaches; and every
    // later part.
    fn errors_after(
        cfgs: &HashMap<usize, Cfg<'_>>,
        parts: &[Part<'_>],
        errs: &[(usize, usize, ErrorRow)],
        part: usize,
        node: usize,
    ) -> Vec<ErrorRow> {
        let mut out: Vec<ErrorRow> = Vec::new();
        let reach_in = cfgs
            .get(&row_key(parts[part].row))
            .map(|cfg| cfg.forward(&[node], &parts[part].target, false));
        for (pi, n, e) in errs {
            let after = *pi > part
                || (*pi == part
                    && reach_in
                        .as_ref()
                        .is_none_or(|f| f.get(*n).copied().unwrap_or(false)));
            if after {
                push_error(&mut out, e.clone());
            }
        }
        out
    }
    // Commits in the region.
    for nc in &cones {
        let p = &parts[nc.part];
        for (i, class, labels, steps) in &nc.cone.commits {
            let Some(rec) = record(w, *i) else { continue };
            let mut l = labels.clone();
            if nc.loop_carried {
                l.push("loop-carried");
            }
            let mut full = p.prefix.clone();
            full.extend(steps.iter().copied());
            let kind = if *class == 2 && steps.is_empty() {
                "run-commit"
            } else {
                "commit"
            };
            let mut row = w.commit_row(rec, *class, kind, p.kind, l, full);
            if *class == 1 {
                row.errors_after = errors_after(&w.cfgs, parts, &errs, nc.part, nc.node);
                push_commit(&mut w.out.between, row);
            } else {
                push_commit(&mut w.out.own_transaction, row);
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
        // The A-tail part of the routine the run is called from (j > k), or
        // M's middle (j == k), and every later part.
        let part = parts
            .iter()
            .position(|p| p.routine == routine_at(w.root, ra, j) && p.kind != "B-head")
            .unwrap_or(0);
        let mut errors: Vec<ErrorRow> = Vec::new();
        for (pi, _, e) in &errs {
            if *pi >= part {
                push_error(&mut errors, e.clone());
            }
        }
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
        push_commit(
            &mut w.out.between,
            CommitRow {
                file: anchor.map(|x| x.source_unit_id.to_string()),
                line: anchor.map(|x| x.start_line),
                column: anchor.map(|x| x.start_column),
                op_id: at.to_string(),
                class: 1,
                kind,
                part: "A-tail",
                labels,
                ends: Vec::new(),
                route: Some(SliceRoute { path, sites }),
                errors_after: errors,
            },
        );
    }
    // Commits without B.
    for (pi, n, cone) in withouts {
        let p = &parts[pi];
        let ends = match w.cfg(p.row) {
            Some(cfg) => cfg.ends(n, &p.target),
            None => Vec::new(),
        };
        for (i, class, labels, steps) in &cone.commits {
            if *class != 1 {
                continue;
            }
            let Some(rec) = record(w, *i) else { continue };
            let mut full = p.prefix.clone();
            full.extend(steps.iter().copied());
            let mut row = w.commit_row(rec, 1, "commit", p.kind, labels.clone(), full);
            row.ends = ends.clone();
            push_commit(&mut w.out.without_b, row);
        }
    }
    // Commits inside A and inside B.
    for (s, route, inside) in [(a, ra, true), (b, &[][..], false)] {
        let route = if inside { route.to_vec() } else { Vec::new() };
        let owner = s.owner;
        let ids = s.ids.clone();
        let cone = w.cone(owner, &ids);
        let all_errors: Vec<ErrorRow> = errs.iter().map(|(_, _, e)| e.clone()).collect();
        for (i, class, labels, steps) in &cone.commits {
            let Some(rec) = record(w, *i) else { continue };
            if steps.is_empty() && rec.owner == owner && ids.contains(&rec.terminal.id()) {
                // The site itself is a commit statement: it is not inside it.
                continue;
            }
            let mut full = route.clone();
            full.extend(steps.iter().copied());
            let part = if inside { "inside-A" } else { "inside-B" };
            let mut row = w.commit_row(rec, *class, "commit", part, labels.clone(), full);
            if inside && *class == 1 {
                row.errors_after = all_errors.clone();
            }
            if inside {
                push_commit(&mut w.out.inside_a, row);
            } else {
                push_commit(&mut w.out.inside_b, row);
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

fn push_error(v: &mut Vec<ErrorRow>, e: ErrorRow) {
    match v.iter_mut().find(|x| x.op_id == e.op_id) {
        Some(x) => {
            for l in e.labels {
                if !x.labels.contains(&l) {
                    x.labels.push(l);
                }
            }
        }
        None => v.push(e),
    }
}

fn push_commit(v: &mut Vec<CommitRow>, c: CommitRow) {
    match v
        .iter_mut()
        .find(|x| x.op_id == c.op_id && x.kind == c.kind && x.class == c.class)
    {
        Some(x) => {
            for l in c.labels {
                if !x.labels.contains(&l) {
                    x.labels.push(l);
                }
            }
            for e in c.errors_after {
                push_error(&mut x.errors_after, e);
            }
        }
        None => v.push(c),
    }
}

/// Sort every list by place, deterministically.
fn finish(out: &mut BetweenOut) {
    let key = |c: &CommitRow| (c.file.clone(), c.line, c.column, c.op_id.clone(), c.kind);
    for v in [
        &mut out.between,
        &mut out.inside_a,
        &mut out.inside_b,
        &mut out.without_b,
        &mut out.own_transaction,
    ] {
        v.sort_by(|x, y| key(x).cmp(&key(y)));
        for c in v.iter_mut() {
            c.errors_after.sort();
            c.labels.sort_unstable();
        }
    }
    for e in &mut out.errors_between {
        e.labels.sort_unstable();
    }
    out.errors_between.sort();
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

fn error_value(e: &ErrorRow) -> serde_json::Value {
    serde_json::json!({
        "file": e.file,
        "line": e.line.map(crate::engine::line_base::shown),
        "column": e.column,
        "op": e.op,
        "operationId": e.op_id,
        "labels": e.labels,
    })
}

fn commit_value(c: &CommitRow) -> serde_json::Value {
    serde_json::json!({
        "file": c.file,
        "line": c.line.map(crate::engine::line_base::shown),
        "column": c.column,
        "operationId": c.op_id,
        "class": c.class,
        "kind": c.kind,
        "part": c.part,
        "labels": c.labels,
        "ends": c.ends,
        "route": c.route.as_ref().map(route_value),
        "errorsAfterCommit": c.errors_after.iter().map(error_value).collect::<Vec<_>>(),
    })
}

fn site_value(s: &Option<SiteDesc>) -> serde_json::Value {
    match s {
        Some(s) => serde_json::json!({
            "routine": s.routine,
            "routineDisplay": s.routine_display,
            "file": s.file,
            "line": crate::engine::line_base::shown(s.line),
            "column": s.column,
            "op": s.op,
        }),
        None => serde_json::Value::Null,
    }
}

impl BetweenOut {
    /// The `payload.slice` value of a `commit-between` answer.
    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        let list = |v: &[CommitRow]| v.iter().map(commit_value).collect::<Vec<_>>();
        serde_json::json!({
            "question": "commit-between",
            "after": site_value(&self.after),
            "before": site_value(&self.before),
            "verdictBasis": "class-1 commits (ending the caller's transaction) that may run after \
                             A returns and before the next B",
            "between": list(&self.between),
            "commitsInsideA": list(&self.inside_a),
            "commitsInsideB": list(&self.inside_b),
            "commitsWithoutB": list(&self.without_b),
            "ownTransactionCommits": list(&self.own_transaction),
            "errorsBetween": self.errors_between.iter().map(error_value).collect::<Vec<_>>(),
            "relation": self.relation,
            "blockers": self.blockers.iter().map(|(k, d)| serde_json::json!({"kind": k, "detail": d})).collect::<Vec<_>>(),
            "pairsExamined": self.pairs_examined,
            "staticLabel": crate::engine::l5::prove_slice::STATIC_LABEL,
        })
    }

    fn commit_lines(
        &self,
        title: &str,
        rows: &[&CommitRow],
        detail: bool,
        lines: &mut Vec<String>,
    ) {
        if rows.is_empty() {
            return;
        }
        lines.push(format!("  {title} ({}):", rows.len()));
        let shown = if detail { rows.len() } else { HUMAN_ROWS };
        for c in rows.iter().take(shown) {
            let mut l = format!(
                "    [COMMIT class {}] {} ({}{})",
                c.class,
                place(&c.file, c.line),
                c.part,
                if c.labels.is_empty() {
                    String::new()
                } else {
                    format!("; {}", c.labels.join(", "))
                }
            );
            if !c.ends.is_empty() {
                l.push_str(&format!("; ends by {}", c.ends.join(" or ")));
            }
            lines.push(l);
            if let Some(r) = &c.route {
                lines.push(format!("      route: {}", r.path.hops.join(" → ")));
            }
            for e in c.errors_after.iter().take(shown) {
                lines.push(format!(
                    "      then may fail at {} {}{}",
                    place(&e.file, e.line),
                    e.op.as_deref().unwrap_or("error"),
                    if e.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", e.labels.join(", "))
                    }
                ));
            }
            if c.errors_after.len() > shown {
                lines.push(format!(
                    "      … {} more errors after this commit (--detail lists them)",
                    c.errors_after.len() - shown
                ));
            }
        }
        if rows.len() > shown {
            lines.push(format!(
                "    … {} more (--detail lists them)",
                rows.len() - shown
            ));
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
        let mut dangerous: Vec<&CommitRow> = self
            .between
            .iter()
            .chain(&self.inside_a)
            .filter(|c| c.class == 1 && !c.errors_after.is_empty())
            .collect();
        dangerous.sort_by_key(|c| c.part == "inside-A");
        self.commit_lines(
            "DANGEROUS — a class-1 commit, then an error before B",
            &dangerous,
            detail,
            &mut lines,
        );
        let rest: Vec<&CommitRow> = self
            .between
            .iter()
            .filter(|c| c.errors_after.is_empty())
            .collect();
        self.commit_lines("commits between A and B", &rest, detail, &mut lines);
        let inside_a: Vec<&CommitRow> = self
            .inside_a
            .iter()
            .filter(|c| c.class != 1 || c.errors_after.is_empty())
            .collect();
        self.commit_lines(
            "commits inside A (they run during A; give a deeper --after to order them)",
            &inside_a,
            detail,
            &mut lines,
        );
        let inside_b: Vec<&CommitRow> = self.inside_b.iter().collect();
        self.commit_lines(
            "commits inside B (they run during B; give a deeper --before to order them)",
            &inside_b,
            detail,
            &mut lines,
        );
        let without: Vec<&CommitRow> = self.without_b.iter().collect();
        self.commit_lines(
            "commits after A on paths that end before B",
            &without,
            detail,
            &mut lines,
        );
        if !self.errors_between.is_empty() {
            let shown = if detail {
                self.errors_between.len()
            } else {
                HUMAN_ROWS
            };
            lines.push(format!(
                "  errors that may run after A and before B ({}):",
                self.errors_between.len()
            ));
            for e in self.errors_between.iter().take(shown) {
                lines.push(format!(
                    "    {} {}{}",
                    place(&e.file, e.line),
                    e.op.as_deref().unwrap_or("error"),
                    if e.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", e.labels.join(", "))
                    }
                ));
            }
            if self.errors_between.len() > shown {
                lines.push(format!(
                    "    … {} more (--detail lists them)",
                    self.errors_between.len() - shown
                ));
            }
        }
        let own: Vec<&CommitRow> = self.own_transaction.iter().collect();
        self.commit_lines(
            "own-transaction commits (class 2: they never make A's pending write durable)",
            &own,
            detail,
            &mut lines,
        );
        lines
    }
}

#[cfg(test)]
#[path = "commit_between_tests.rs"]
mod tests;
