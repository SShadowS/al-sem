//! The debugging slice of `alsem prove` (agent-tasks design
//! `docs/superpowers/specs/2026-11-05-agent-tasks-design.md` §9, milestone
//! E1): `--match <text>`, `--site <file:line>`, `--op <kind>` and `--table
//! <table>` filter an existing-effects question down to the SITES that may
//! explain one error or one write, and print for each matching site its
//! message text, one representative route from the root and the conditions
//! that enclose the route's sites.
//!
//! **What a site is.** One direct fact of the question's kinds (an `Error`,
//! an implicit error such as a `TestField` or a `Get` whose result is not
//! read, a record write, a `Commit()`, a UI call), read from the tree's
//! terminal records ([`crate::engine::l5::terminal_facts`]), whose routine
//! the root's guarded search ([`LiteralReach::live_tree`], literal arguments
//! followed) reaches in a state where the site can run. A site the literal
//! arguments rule out is counted, not listed.
//!
//! **Honesty rules** (each pinned by a test in `prove_slice_tests.rs`):
//!
//! 1. A filtered `no` needs every reachable site to be DECIDED against the
//!    filter. A site whose message text is not static (a computed message, a
//!    `GetLastErrorText` rethrow, an `ErrorInfo`, a platform message) cannot
//!    show that it does not match `--match`, so it blocks the `no`
//!    (`message-unknown`). So does a text with `%1` placeholders that did not
//!    match (`message-placeholders`: the values may hold the text) and a
//!    `FieldError` text (`message-platform-wrapped`: the platform adds the
//!    field and table to it). An implicit error matches `--match` only by
//!    its kind word (`TestField`, `Get`, …), and the answer says so.
//! 2. The guards shown are SELECTED context: at most
//!    [`GUARDS_PER_SITE`] innermost enclosing conditions per site on the
//!    route, with the count of the rest (`guards shown: k of n`). They are
//!    never all the conditions a site needs (an earlier `exit`, a callee's
//!    result, a check that stops are not guards here).
//! 3. A `while`/`for`/`foreach` condition is a loop-entry condition; a
//!    `repeat … until` condition is a continuation condition (the body ran
//!    once before it was tested). Each is labelled so.
//! 4. Everything is a static candidate, not an observed trace: the route is
//!    one example of how the root may reach the site.
//!
//! The slice is computed while the tree is alive (a level-3 read: the prove
//! plan's tree demand carries the [`SliceRequest`]); it reads the model's
//! rules (`ViewNeeds::rules`, the message texts) and guard arms
//! (`ViewNeeds::site_guards`), both declared by `alsem prove` only when a
//! filter is given.

use crate::engine::l5::group_path::{GroupPath, GroupPaths};
pub use crate::engine::l5::literal_reach::routes::{ROUTE_CAP, ROUTE_EDGE_BUDGET};
use crate::engine::l5::literal_reach::routes::{RouteCounts, RouteLimit, StateGraph};
use crate::engine::l5::literal_reach::{LiteralReach, LiveTree, TrapAt, TrapVerdict};
use crate::engine::l5::prove::{ProveQuestion, UI_TYPES, name_or_id_matches};
use crate::engine::l5::snapshot::CapabilitySnapshot;
use crate::engine::l5::terminal_facts::{TerminalSite, TerminalTemp};
use crate::program::body::features::{PAnchor, PCallee};
use crate::program::body::rules::{MessageSource, RuleKind};
use crate::program::body::site_guards::{GuardArm, GuardKind};
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet};

/// The most enclosing conditions shown per site on a route (design §9.4).
pub const GUARDS_PER_SITE: usize = 2;

/// The most matching sites a slice computes a route for; the rest are
/// counted.
pub const MAX_ROUTES: usize = 20;

/// The static-candidate label every slice answer carries.
pub const STATIC_LABEL: &str = "static candidates, not an observed trace";

/// Which sites a question's slice looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceQuestion {
    /// `throws-error`: explicit and implicit errors.
    Errors,
    /// `writes-table:<table>`: record writes on that table, or on a table not
    /// known.
    Writes { table: String },
    /// `may-commit`.
    Commits,
    /// `reaches-ui`.
    Ui,
}

impl SliceQuestion {
    /// The question's sites, as an answer names them (`error sites`).
    #[must_use]
    pub fn site_noun(&self) -> &'static str {
        match self {
            SliceQuestion::Errors => "error sites",
            SliceQuestion::Writes { .. } => "write sites",
            SliceQuestion::Commits => "COMMIT sites",
            SliceQuestion::Ui => "UI sites",
        }
    }
}

/// `--site <file:line>`: a source file (a path suffix, matched at a `/`
/// boundary, ASCII case-insensitive) and a 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteFilter {
    pub file: String,
    pub line: u32,
}

impl SiteFilter {
    /// Parse `file:line`.
    pub fn parse(text: &str) -> Result<SiteFilter, String> {
        let bad = || {
            format!(
                "prove: --site '{text}' is not <file>:<line> (a source file path or its tail, \
                 and a 1-based line)"
            )
        };
        let (file, line) = text.rsplit_once(':').ok_or_else(bad)?;
        let line: u32 = line.trim().parse().map_err(|_| bad())?;
        let file = file.trim().replace('\\', "/");
        if file.is_empty() || line == 0 {
            return Err(bad());
        }
        Ok(SiteFilter { file, line })
    }

    /// Whether a site at `file` (a source unit id, `ws:` or `dep:<app>:`
    /// prefixed) and 0-based `line` is this one.
    fn matches(&self, file: &str, line: u32) -> bool {
        if line + 1 != self.line {
            return false;
        }
        let f = file.replace('\\', "/").to_ascii_lowercase();
        let want = self.file.to_ascii_lowercase();
        f == want
            || f.strip_suffix(&want).is_some_and(|head| {
                head.ends_with('/') || head.ends_with(':') || want.starts_with('/')
            })
    }
}

/// The filters of one prove question (design §9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceRequest {
    pub question: SliceQuestion,
    pub match_text: Option<String>,
    pub site: Option<SiteFilter>,
    pub op: Option<String>,
    pub table: Option<String>,
    /// `--routes N` (agent-tasks E2, design §11): up to N distinct routes
    /// per matching site, 1..=[`ROUTE_CAP`]. 1 (the default) is E1's one
    /// route off the search tree, byte for byte; above 1 the routes are
    /// enumerated over the state-transition graph
    /// ([`crate::engine::l5::literal_reach::routes`]).
    pub routes: usize,
    /// The candidate edges one site's enumeration examines at most
    /// ([`ROUTE_EDGE_BUDGET`]; a fixture sets it lower to reach the cap).
    pub route_budget: usize,
}

impl SliceRequest {
    /// The request for `question` with these filters, `None` when no filter
    /// is given (the unfiltered answer, byte-identical), or the usage error.
    /// Which filter goes with which question (design §12.1, E1's scope):
    ///
    /// | filter | questions |
    /// |---|---|
    /// | `--match` | `throws-error` |
    /// | `--table` | `throws-error` |
    /// | `--op` | `throws-error`, `writes-table:` |
    /// | `--site` | `throws-error`, `writes-table:`, `may-commit`, `reaches-ui` |
    /// | `--routes` | with one of the filters above |
    pub fn of(
        question_text: &str,
        match_text: Option<&str>,
        site: Option<&str>,
        op: Option<&str>,
        table: Option<&str>,
        routes: Option<usize>,
    ) -> Result<Option<SliceRequest>, String> {
        if let Some(n) = routes
            && !(1..=ROUTE_CAP).contains(&n)
        {
            return Err(format!(
                "prove: --routes takes 1 to {ROUTE_CAP} (routes per matching site), not {n}"
            ));
        }
        if match_text.is_none() && site.is_none() && op.is_none() && table.is_none() {
            if routes.is_some() {
                return Err(
                    "prove: --routes goes with --match, --site, --op or --table (the \
                            routes of the sites they keep)"
                        .to_string(),
                );
            }
            return Ok(None);
        }
        let parsed = crate::engine::l5::prove::parse_question(question_text);
        let question = match &parsed {
            Some(ProveQuestion::ThrowsError) => SliceQuestion::Errors,
            Some(ProveQuestion::WritesTable { table }) => SliceQuestion::Writes {
                table: table.clone(),
            },
            Some(ProveQuestion::MayCommit) => SliceQuestion::Commits,
            Some(ProveQuestion::ReachesUi) => SliceQuestion::Ui,
            _ => {
                return Err(format!(
                    "prove: --match, --site, --op and --table go with throws-error, \
                     writes-table:<table>, may-commit or reaches-ui, not '{question_text}'"
                ));
            }
        };
        let errors = question == SliceQuestion::Errors;
        let writes = matches!(question, SliceQuestion::Writes { .. });
        if match_text.is_some() && !errors {
            return Err(format!(
                "prove: --match goes with throws-error (it matches error message text), not \
                 '{question_text}'"
            ));
        }
        if table.is_some() && !errors {
            return Err(format!(
                "prove: --table goes with throws-error (errors a record operation on that table \
                 raises); writes-table:<table> names its table itself, not '{question_text}'"
            ));
        }
        if op.is_some() && !(errors || writes) {
            return Err(format!(
                "prove: --op goes with throws-error or writes-table:<table>, not '{question_text}'"
            ));
        }
        let nonempty = |flag: &str, v: Option<&str>| -> Result<Option<String>, String> {
            match v.map(str::trim) {
                Some("") => Err(format!("prove: {flag} needs a non-empty value")),
                v => Ok(v.map(str::to_string)),
            }
        };
        Ok(Some(SliceRequest {
            question,
            match_text: nonempty("--match", match_text)?,
            site: site.map(SiteFilter::parse).transpose()?,
            op: nonempty("--op", op)?,
            table: nonempty("--table", table)?,
            routes: routes.unwrap_or(1),
            route_budget: ROUTE_EDGE_BUDGET,
        }))
    }

    /// The filter as the answer line names it: `matching "x" at f:3 op
    /// Insert on table T`.
    #[must_use]
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = &self.match_text {
            parts.push(format!("matching \"{m}\""));
        }
        if let Some(s) = &self.site {
            parts.push(format!("at {}:{}", s.file, s.line));
        }
        if let Some(o) = &self.op {
            parts.push(format!("op {o}"));
        }
        if let Some(t) = &self.table {
            parts.push(format!("on table {t}"));
        }
        // E2: only above E1's one route, so `--routes 1` is byte-identical.
        if self.routes > 1 {
            parts.push(format!("(up to {} routes per site)", self.routes));
        }
        parts.join(" ")
    }

    /// The filter as JSON (`payload.filter`).
    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        if let Some(v) = &self.match_text {
            m.insert("match".into(), v.clone().into());
        }
        if let Some(s) = &self.site {
            m.insert(
                "site".into(),
                serde_json::json!({"file": s.file, "line": s.line}),
            );
        }
        if let Some(v) = &self.op {
            m.insert("op".into(), v.clone().into());
        }
        if let Some(v) = &self.table {
            m.insert("table".into(), v.clone().into());
        }
        if self.routes > 1 {
            m.insert("routes".into(), self.routes.into());
        }
        serde_json::Value::Object(m)
    }
}

/// An error site's message (design §9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageText {
    /// Static text: a literal, a `Label` or a `TextConst`. `partial` says
    /// why the text the user sees may hold more than it: `placeholders`
    /// (`%1` values are not known) or `platform-wrapped` (a `FieldError`'s
    /// text, to which the platform adds the field and table).
    Known {
        source: &'static str,
        text: String,
        var: Option<String>,
        partial: Option<&'static str>,
    },
    /// No static text: `computed` (a variable or expression), `error-info`,
    /// `platform` (the platform's own message: `TestField`, `Get`, …) or
    /// `not-recorded` (no rule holds the check, e.g. an `Error` inside an
    /// expression).
    Unknown { reason: &'static str },
}

impl MessageText {
    /// Whether the text is static and whole.
    #[must_use]
    pub fn known(&self) -> bool {
        matches!(self, MessageText::Known { .. })
    }
}

/// How one site stands against the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It matches; `by_kind_word`: an implicit error matched `--match` by its
    /// kind word only (its message is the platform's).
    Match {
        by_kind_word: bool,
    },
    NoMatch,
    /// It cannot be decided: the reason is a blocker kind.
    Undecided(&'static str),
}

/// One enclosing condition shown on a route (design §9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardOut {
    pub kind: GuardKind,
    pub text: String,
    pub arm: Option<String>,
    pub file: String,
    /// 0-based, as the model's anchors are.
    pub line: u32,
}

impl GuardOut {
    fn of(a: &GuardArm, file: &str) -> GuardOut {
        GuardOut {
            kind: a.kind,
            text: a.text.to_string(),
            arm: a.arm.as_deref().map(str::to_string),
            file: file.to_string(),
            line: a.line,
        }
    }

    /// The condition as the human answer prints it.
    #[must_use]
    pub fn describe(&self) -> String {
        let t = &self.text;
        match self.kind {
            GuardKind::Then => format!("then-branch of `if {t} then`"),
            GuardKind::Else => format!("else-branch of `if {t} then`"),
            GuardKind::CaseArm => format!(
                "arm `{}:` of `case {t} of`",
                self.arm.as_deref().unwrap_or("")
            ),
            GuardKind::CaseElse => format!("else arm of `case {t} of`"),
            GuardKind::While => format!("loop-entry condition of `while {t} do`"),
            GuardKind::For => format!("loop-entry condition of `for {t} do`"),
            GuardKind::Foreach => format!("loop-entry condition of `foreach {t} do`"),
            GuardKind::Repeat => format!(
                "continuation condition of `repeat … until {t}` (the body runs once before it \
                 is tested)"
            ),
        }
    }

    fn to_value(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), self.kind.role().into());
        m.insert("construct".into(), self.kind.construct().into());
        m.insert("condition".into(), self.text.clone().into());
        if let Some(a) = &self.arm {
            m.insert("arm".into(), a.clone().into());
        }
        m.insert("file".into(), self.file.clone().into());
        m.insert(
            "line".into(),
            crate::engine::line_base::shown(self.line).into(),
        );
        serde_json::Value::Object(m)
    }
}

/// One site of a route and the conditions shown for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSite {
    pub file: Option<String>,
    /// 0-based.
    pub line: Option<u32>,
    pub shown: Vec<GuardOut>,
    /// Every enclosing condition of the site (`shown` is the innermost
    /// [`GUARDS_PER_SITE`] of them).
    pub total: usize,
}

/// One matching site's route: the witness and its sites' guards.
#[derive(Debug, Clone)]
pub struct SliceRoute {
    pub path: GroupPath,
    /// The call site of each step in its caller, then the terminal site.
    pub sites: Vec<RouteSite>,
}

impl SliceRoute {
    /// (shown, total) over the route's sites.
    #[must_use]
    pub fn guard_counts(&self) -> (usize, usize) {
        self.sites
            .iter()
            .fold((0, 0), |(s, t), r| (s + r.shown.len(), t + r.total))
    }
}

/// One reachable site of the question's kinds.
#[derive(Debug, Clone)]
pub struct SliceSite {
    pub effect_type: &'static str,
    /// The operation or call-site id; `""` for a fact with no site.
    pub op_id: String,
    /// The stable id of the routine it sits in.
    pub owner: String,
    pub file: Option<String>,
    /// 0-based.
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The operation as written: `Error`, `TestField`, `Get`, `Insert`, ….
    pub op_name: Option<String>,
    /// The record operation's table (its name), when it is one and known.
    pub table: Option<String>,
    /// An error site's message; `None` off the errors question.
    pub message: Option<MessageText>,
    pub outcome: Outcome,
    /// For a matching site among the first [`MAX_ROUTES`]: its route.
    pub route: Option<Result<SliceRoute, &'static str>>,
    /// E2 (`--routes N`, N > 1): for a matching site among the first
    /// [`MAX_ROUTES`], its enumerated routes. `route` stays E1's.
    pub routes: Option<RouteSet>,
    /// B7/L23: an error site that escapes only with a write pending at an
    /// isolated event, or escapes on a route with no trap while others are
    /// trapped; `None` otherwise (a site caught on every route is in
    /// [`SliceOut::caught`], not here).
    pub(crate) trap: Option<TrapVerdict>,
    /// B7 review: `route` was found by the search that crosses no definite
    /// trap (a [`TrapVerdict::SomeRoutes`] site's escaping route); `false`
    /// when that search could not give one and `route` is a trapped one.
    pub(crate) route_trap_free: bool,
}

/// B7/L23: an error site a definite trap catches on every route from the
/// root: its place (0-based line) and the trap on its first route.
#[derive(Debug, Clone)]
pub struct CaughtSite {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub(crate) at: TrapAt,
}

/// One site's enumerated routes (agent-tasks E2, design §11): shortest
/// first, each with its guards, and what the enumeration examined
/// ([`crate::engine::l5::literal_reach::routes`] has the definitions).
#[derive(Debug, Clone)]
pub struct RouteSet {
    pub routes: Vec<SliceRoute>,
    pub requested: usize,
    pub budget: usize,
    pub counts: RouteCounts,
    /// `None`: the enumeration completed, so `routes` is every route.
    pub limit: Option<RouteLimit>,
}

/// The slice of one root (module doc).
#[derive(Debug, Clone)]
pub struct SliceOut {
    pub request: SliceRequest,
    /// The root's guarded search completed (it stops at
    /// `literal_reach::STATE_CAP` states); when not, `sites` is empty and no
    /// site was judged.
    pub complete: bool,
    /// Every reachable site, matching ones first (shortest route first),
    /// then by place.
    pub sites: Vec<SliceSite>,
    /// Sites whose routine the root reaches, but in no state where the site
    /// can run under literal arguments: not listed.
    pub literal_dead: usize,
    /// `writes-table`: writes the root reaches on a temporary record (not
    /// table writes).
    pub temporary: usize,
    /// B7/L23: error sites the root reaches only through a definite trap
    /// (they never leave the root): not listed, never a match.
    pub caught: Vec<CaughtSite>,
    /// B7 review: a caught ruling may rest on a checked run with no write
    /// pending within the root: it holds only if the root's own callers have
    /// no write pending when they call it (the answer states this).
    pub caught_assumes_clean_entry: bool,
    /// B7 review round 2: ... and the clean run may be clean only because a
    /// `Commit()` before it ends the write: it holds only if the callers do
    /// not call the root under `[CommitBehavior(Ignore)]` or `(Error)`.
    pub caught_assumes_normal_commit: bool,
}

impl SliceOut {
    /// The matching sites.
    pub fn matching(&self) -> impl Iterator<Item = &SliceSite> {
        self.sites
            .iter()
            .filter(|s| matches!(s.outcome, Outcome::Match { .. }))
    }

    /// Per undecided reason, the sites, in order.
    #[must_use]
    pub fn undecided(&self) -> Vec<(&'static str, Vec<&SliceSite>)> {
        let mut out: Vec<(&'static str, Vec<&SliceSite>)> = Vec::new();
        for s in &self.sites {
            if let Outcome::Undecided(r) = s.outcome {
                match out.iter_mut().find(|(k, _)| *k == r) {
                    Some((_, v)) => v.push(s),
                    None => out.push((r, vec![s])),
                }
            }
        }
        out.sort_by_key(|(k, _)| *k);
        out
    }
}

/// Whether `text` holds a `%1`-style placeholder (or `#1`, the older one).
fn has_placeholders(text: &str) -> bool {
    let b = text.as_bytes();
    b.windows(2)
        .any(|w| (w[0] == b'%' || w[0] == b'#') && w[1].is_ascii_digit())
}

fn contains(outer: &PAnchor, inner: &PAnchor) -> bool {
    (outer.start_line, outer.start_column) <= (inner.start_line, inner.start_column)
        && (inner.end_line, inner.end_column) <= (outer.end_line, outer.end_column)
}

/// The anchor of site `id` in routine `r`: a call site, a record operation
/// or an operation site.
fn anchor_in<'m>(r: &'m ModelRoutine, id: &str) -> Option<&'m PAnchor> {
    r.call_sites
        .iter()
        .find(|c| c.id == id)
        .map(|c| &c.source_anchor)
        .or_else(|| {
            r.record_operations
                .iter()
                .find(|o| o.id == id)
                .map(|o| &o.source_anchor)
        })
        .or_else(|| {
            r.operation_sites
                .iter()
                .find(|o| o.id == id)
                .map(|o| &o.source_anchor)
        })
}

/// What a site is, read off its routine: its anchor, its operation as
/// written and (a record operation's) table.
struct SiteFacts<'m> {
    anchor: Option<&'m PAnchor>,
    op_name: Option<String>,
    table: Option<String>,
}

fn site_facts<'m>(
    r: &'m ModelRoutine,
    id: &str,
    table_names: &HashMap<&str, &str>,
) -> SiteFacts<'m> {
    let table_of = |tid: Option<&String>| {
        tid.and_then(|t| table_names.get(t.as_str()))
            .map(|n| (*n).to_string())
    };
    if let Some(o) = r.record_operations.iter().find(|o| o.id == id) {
        return SiteFacts {
            anchor: Some(&o.source_anchor),
            op_name: Some(o.op.clone()),
            table: table_of(o.table_id.as_ref()),
        };
    }
    if let Some(o) = r.operation_sites.iter().find(|o| o.id == id) {
        let name = match o.kind.as_str() {
            "error-call" => Some("Error".to_string()),
            "commit" => Some("Commit".to_string()),
            _ => None,
        };
        return SiteFacts {
            anchor: Some(&o.source_anchor),
            op_name: name,
            table: None,
        };
    }
    if let Some(c) = r.call_sites.iter().find(|c| c.id == id) {
        let (name, receiver) = match &c.callee {
            PCallee::Bare { name } => (Some(name.clone()), None),
            PCallee::Member { receiver, method } => (Some(method.clone()), Some(receiver)),
            _ => (None, None),
        };
        // A `Rec.FieldError(...)`: the record variable's table.
        let table = receiver.and_then(|rv| {
            r.record_variables
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(rv))
                .and_then(|v| v.table_id.as_ref())
                .and_then(|t| table_names.get(t.as_str()))
                .map(|n| (*n).to_string())
        });
        return SiteFacts {
            anchor: Some(&c.source_anchor),
            op_name: name,
            table,
        };
    }
    SiteFacts {
        anchor: None,
        op_name: None,
        table: None,
    }
}

/// An error site's message, from its routine's rules (the check whose
/// statement holds the site).
fn message_of(r: &ModelRoutine, op_name: Option<&str>, anchor: Option<&PAnchor>) -> MessageText {
    let kinds: &[RuleKind] = match op_name {
        Some(n) if n.eq_ignore_ascii_case("Error") => &[RuleKind::Error, RuleKind::ConfirmError],
        Some(n) if n.eq_ignore_ascii_case("TestField") => &[RuleKind::TestField],
        Some(n) if n.eq_ignore_ascii_case("FieldError") => &[RuleKind::FieldError],
        // `Get`, `FindFirst`, `Evaluate`, a platform native: the platform's
        // own message.
        _ => return MessageText::Unknown { reason: "platform" },
    };
    let rule = anchor.and_then(|a| {
        r.rules()
            .iter()
            .find(|rule| kinds.contains(&rule.kind) && contains(&rule.anchor, a))
    });
    let Some(m) = rule.and_then(|rule| rule.message.as_ref()) else {
        return MessageText::Unknown {
            reason: match rule {
                // A `TestField` with no text of its own: the platform's.
                Some(_) => "platform",
                None => "not-recorded",
            },
        };
    };
    let (source, var) = match &m.source {
        MessageSource::Literal => ("literal", None),
        MessageSource::Label { var } => ("label", Some(var.clone())),
        MessageSource::TextConst { var } => ("textconst", Some(var.clone())),
        MessageSource::ErrorInfo => {
            return MessageText::Unknown {
                reason: "error-info",
            };
        }
        MessageSource::Platform => return MessageText::Unknown { reason: "platform" },
        MessageSource::Dynamic => return MessageText::Unknown { reason: "computed" },
    };
    let Some(text) = m.text.clone() else {
        return MessageText::Unknown { reason: "computed" };
    };
    let partial = if op_name.is_some_and(|n| n.eq_ignore_ascii_case("FieldError")) {
        Some("platform-wrapped")
    } else if has_placeholders(&text) {
        Some("placeholders")
    } else {
        None
    };
    MessageText::Known {
        source,
        text,
        var,
        partial,
    }
}

/// One filter's verdict on a site.
enum One {
    Yes { by_kind_word: bool },
    No,
    Undecided(&'static str),
}

fn match_text_on(needle: &str, site: &SliceSite) -> One {
    let n = needle.to_lowercase();
    match &site.message {
        Some(MessageText::Known {
            text, var, partial, ..
        }) => {
            if text.to_lowercase().contains(&n)
                || var.as_ref().is_some_and(|v| v.to_lowercase().contains(&n))
            {
                One::Yes {
                    by_kind_word: false,
                }
            } else {
                match partial {
                    Some("placeholders") => One::Undecided("message-placeholders"),
                    Some(_) => One::Undecided("message-platform-wrapped"),
                    None => One::No,
                }
            }
        }
        _ => {
            // An implicit error's message is the platform's: it matches only
            // by its kind word (design §9.3).
            let word = site.op_name.as_deref().map(str::to_lowercase);
            if site.effect_type == "ERROR_IMPLICIT"
                && word
                    .as_deref()
                    .is_some_and(|w| !w.is_empty() && (n.contains(w) || w.contains(&n)))
            {
                One::Yes { by_kind_word: true }
            } else {
                One::Undecided("message-unknown")
            }
        }
    }
}

/// The site's outcome against `req` (module doc, rule 1). A filter it fails
/// decides `NoMatch` whatever the others say.
fn outcome(req: &SliceRequest, site: &SliceSite, caller_dependent: bool) -> Outcome {
    let mut verdicts: Vec<One> = Vec::new();
    if let Some(f) = &req.site {
        verdicts.push(match (&site.file, site.line) {
            (Some(file), Some(line)) => {
                if f.matches(file, line) {
                    One::Yes {
                        by_kind_word: false,
                    }
                } else {
                    One::No
                }
            }
            _ => One::Undecided("position-unknown"),
        });
    }
    if let Some(op) = &req.op {
        verdicts.push(match &site.op_name {
            Some(n) if n.eq_ignore_ascii_case(op) => One::Yes {
                by_kind_word: false,
            },
            Some(_) => One::No,
            None => One::Undecided("operation-unknown"),
        });
    }
    if let Some(t) = &req.table {
        let record_op = site
            .op_name
            .as_deref()
            .is_some_and(|n| !n.eq_ignore_ascii_case("Error"));
        verdicts.push(match (&site.table, record_op) {
            (Some(name), _) if name_or_id_matches(Some(name), None, t) => One::Yes {
                by_kind_word: false,
            },
            (Some(_), _) => One::No,
            // An `Error(...)` is raised by no record operation.
            (None, false) if site.op_name.is_some() => One::No,
            (None, _) => One::Undecided("table-unknown"),
        });
    }
    if let Some(m) = &req.match_text {
        verdicts.push(match_text_on(m, site));
    }
    let mut undecided = None;
    let mut by_kind_word = false;
    for v in verdicts {
        match v {
            One::No => return Outcome::NoMatch,
            One::Undecided(r) => {
                undecided.get_or_insert(r);
            }
            One::Yes { by_kind_word: k } => by_kind_word |= k,
        }
    }
    if let Some(r) = undecided {
        return Outcome::Undecided(r);
    }
    // A matching write is a table write only when its record is not
    // temporary; it is decided per caller (G17b), and a write of a table not
    // known may be any table.
    if let SliceQuestion::Writes { .. } = req.question {
        if caller_dependent {
            return Outcome::Undecided("temp-state-depends-on-caller");
        }
        if site.table.is_none() {
            return Outcome::Undecided("unknown-table-write");
        }
    }
    Outcome::Match { by_kind_word }
}

/// The guards of each site on `path` (module doc, rules 2 and 3): each
/// step's call site in its caller, then the terminal site `(owner, anchor)`.
fn route_sites<'a>(
    reach: &LiteralReach<'a>,
    root: &str,
    path: &GroupPath,
    terminal: (&str, Option<&PAnchor>),
) -> Vec<RouteSite> {
    let mut out = Vec::new();
    let mut caller = root.to_string();
    let mut at = |routine: &str, anchor: Option<&PAnchor>| {
        let r = reach.routine(routine);
        let (shown, total) = match (r, anchor) {
            (Some(r), Some(a)) => {
                let arms = r.site_guards().enclosing(a);
                (
                    arms.iter()
                        .take(GUARDS_PER_SITE)
                        .map(|g| GuardOut::of(g, &a.source_unit_id))
                        .collect(),
                    arms.len(),
                )
            }
            _ => (Vec::new(), 0),
        };
        out.push(RouteSite {
            file: anchor.map(|a| a.source_unit_id.clone()),
            line: anchor.map(|a| a.start_line),
            shown,
            total,
        });
    };
    for s in &path.steps {
        let anchor = s
            .edge_id
            .as_deref()
            .and_then(|id| reach.routine(&caller).and_then(|r| anchor_in(r, id)));
        at(&caller, anchor);
        caller = s.to.clone();
    }
    at(terminal.0, terminal.1);
    out
}

/// The slice of `root` over `tree` for `req` (module doc). `cancel` stops a
/// route enumeration (`--routes` above 1) and the state graph it reads; E1's
/// one route is read off the search tree, which is not cancelled.
pub(crate) fn compute_slice(
    tree: &CapabilitySnapshot,
    model: &Model,
    root: &str,
    req: &SliceRequest,
    cancel: &dyn Fn() -> bool,
) -> SliceOut {
    let _s = crate::engine::perf_trace::span("prove", "prove.slice");
    // The guard arms' cost (agent-tasks E1's measurement): every row's arms,
    // their text and the box, counted under `ALSEM_TRACE`.
    if crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Stages) {
        let (mut arms, mut bytes) = (0u64, 0u64);
        for r in &model.workspace.routines {
            if let Some(g) = &r.site_guards {
                arms += g.arms.len() as u64;
                bytes += (g.heap_bytes()
                    + std::mem::size_of::<crate::program::body::site_guards::SiteGuards>())
                    as u64;
            }
        }
        crate::engine::perf_trace::counter_delta("prove.site_guard_arms", arms);
        crate::engine::perf_trace::counter_delta("prove.site_guard_bytes", bytes);
    }
    let reach = LiteralReach::new(tree, model);
    let mut out = SliceOut {
        request: req.clone(),
        complete: false,
        sites: Vec::new(),
        literal_dead: 0,
        temporary: 0,
        caught: Vec::new(),
        caught_assumes_clean_entry: false,
        caught_assumes_normal_commit: false,
    };
    // E2: above one route, the state-transition graph, whose tree is the
    // search tree E1 reads (the same search, every transition kept).
    let graph: Option<StateGraph> = if req.routes > 1 {
        let _g = crate::engine::perf_trace::span("prove", "prove.route_graph");
        let Ok(g) = reach.state_graph(root, cancel) else {
            return out;
        };
        crate::engine::perf_trace::counter_delta("prove.route_graph_states", g.states() as u64);
        crate::engine::perf_trace::counter_delta(
            "prove.route_graph_transitions",
            g.transitions() as u64,
        );
        crate::engine::perf_trace::counter_delta(
            "prove.route_graph_extra_bytes",
            g.extra_bytes() as u64,
        );
        Some(g)
    } else {
        None
    };
    let tree_only: Option<LiveTree> = match &graph {
        Some(_) => None,
        None => match reach.live_tree(root) {
            Some(t) => Some(t),
            None => return out,
        },
    };
    let live: &LiveTree = match (&graph, &tree_only) {
        (Some(g), _) => &g.tree,
        (None, Some(t)) => t,
        (None, None) => unreachable!("one of the two was built"),
    };
    out.complete = true;
    let table_names: HashMap<&str, &str> = model
        .workspace
        .tables
        .iter()
        .map(|t| (t.id.as_str(), t.name.as_str()))
        .collect();
    let fits = |effect_type: &str, resource: Option<&str>| match &req.question {
        SliceQuestion::Errors => matches!(effect_type, "ERROR_THROW" | "ERROR_IMPLICIT"),
        SliceQuestion::Commits => effect_type == "COMMIT",
        SliceQuestion::Ui => UI_TYPES.contains(&effect_type),
        SliceQuestion::Writes { table } => {
            matches!(effect_type, "DB_INSERT" | "DB_MODIFY" | "DB_DELETE")
                && resource
                    .is_none_or(|r| name_or_id_matches(table_names.get(r).copied(), Some(r), table))
        }
    };
    let errors = req.question == SliceQuestion::Errors;
    // B4/L16: a commit no route from the root runs normally is no commit
    // site, and one some route runs under `CommitBehavior::Error` is an
    // error site (the digest's rule, `commit_scope`).
    let commit_scopes = crate::engine::l5::commit_scope::CbGraph::of_tree(tree);
    let commit_states = |owner: &str| -> u8 {
        commit_scopes
            .as_ref()
            .filter(|cb| !cb.root_unaffected(root))
            .and_then(|cb| cb.states_at(root, owner))
            .unwrap_or(crate::engine::l5::commit_scope::N)
    };
    // B7/L23: an error site a definite trap catches on every route is not
    // reachable as an error of the root: the walks that cross no trap
    // ([`LiteralReach::open_reach`]), only when a trap is in reach. (The
    // errors question's sites, and the error dialogs, `UI_ERROR`, of the UI
    // question.)
    let trapped_question = matches!(req.question, SliceQuestion::Errors | SliceQuestion::Ui);
    let scope = if trapped_question {
        reach.trap_scope(root, live)
    } else {
        None
    };
    // The guarded search that crosses no definite trap, built only for a
    // matching site that escapes on some routes (its printed route).
    let mut open_tree: Option<Option<LiveTree>> = None;
    let mut seen: HashSet<(&str, &str, &str)> = HashSet::new();
    // Per site: the site, and whether its write is caller-dependent.
    let mut sites: Vec<(SliceSite, Option<Vec<String>>)> = Vec::new();
    for rec in reach.terminals().records() {
        // A commit that raises an error under `CommitBehavior::Error`.
        let commit_error = rec.effect_type == "COMMIT"
            && commit_states(rec.owner) & crate::engine::l5::commit_scope::E != 0;
        let fits_here = match (&req.question, rec.effect_type) {
            (SliceQuestion::Commits, "COMMIT") => {
                commit_states(rec.owner) & crate::engine::l5::commit_scope::N != 0
            }
            (SliceQuestion::Errors, "COMMIT") => commit_error,
            _ => fits(rec.effect_type, rec.resource),
        };
        if !fits_here || !reach.reaches(live, rec.owner) {
            continue;
        }
        let effect_type = if errors && rec.effect_type == "COMMIT" {
            "ERROR_IMPLICIT"
        } else {
            rec.effect_type
        };
        if !seen.insert((rec.effect_type, rec.owner, rec.terminal.id())) {
            continue;
        }
        let r = reach.routine(rec.owner);
        if rec.terminal == TerminalSite::None {
            // A fact with no site (a platform native that may error): in
            // reach, but with no place, text or route.
            let site = SliceSite {
                effect_type,
                op_id: String::new(),
                owner: rec.owner.to_string(),
                file: None,
                line: None,
                column: None,
                op_name: r.map(|r| r.name.clone()),
                table: None,
                message: errors.then_some(MessageText::Unknown { reason: "platform" }),
                outcome: Outcome::Undecided("no-site"),
                route: None,
                routes: None,
                trap: None,
                route_trap_free: false,
            };
            sites.push((site, None));
            continue;
        }
        let ops = vec![rec.terminal.id().to_string()];
        if reach.tree_witness(live, &ops).is_none() {
            out.literal_dead += 1;
            continue;
        }
        if matches!(rec.temp, TerminalTemp::Known(true)) {
            out.temporary += 1;
            continue;
        }
        let trap = scope
            .as_ref()
            .filter(|_| crate::engine::l5::digest_cli::is_error_row_type(rec.effect_type))
            .and_then(|s| reach.trap_verdict(live, s, &ops));
        if let Some(TrapVerdict::Caught(at)) = &trap {
            let anchor = r.and_then(|r| anchor_in(r, rec.terminal.id()));
            // B7 review: the ruling may rest on a checked run, which holds
            // only if the root's callers have no write pending.
            out.caught_assumes_clean_entry |= scope.as_ref().is_some_and(|s| s.clean_checked);
            out.caught_assumes_normal_commit |= scope.as_ref().is_some_and(|s| s.clean_by_commit);
            out.caught.push(CaughtSite {
                file: anchor.map(|a| a.source_unit_id.clone()),
                line: anchor.map(|a| a.start_line),
                at: at.clone(),
            });
            continue;
        }
        let caller_dependent = matches!(rec.temp, TerminalTemp::ParameterDependent(_));
        let facts = r.map(|r| site_facts(r, rec.terminal.id(), &table_names));
        let anchor = facts.as_ref().and_then(|f| f.anchor);
        let op_name = match (&req.question, rec.op_kind) {
            // A write's ORIGINAL kind (`ModifyAll`, `Rename`, …).
            (SliceQuestion::Writes { .. }, Some(k)) => Some(k.to_string()),
            _ => facts.as_ref().and_then(|f| f.op_name.clone()),
        };
        let table = match (&req.question, rec.resource) {
            (SliceQuestion::Writes { .. }, Some(res)) => table_names
                .get(res)
                .map(|n| (*n).to_string())
                .or_else(|| Some(res.to_string())),
            (SliceQuestion::Writes { .. }, None) => None,
            _ => facts.as_ref().and_then(|f| f.table.clone()),
        };
        let message = match (errors, r) {
            (true, Some(r)) => Some(message_of(r, op_name.as_deref(), anchor)),
            (true, None) => Some(MessageText::Unknown {
                reason: "not-recorded",
            }),
            _ => None,
        };
        let mut site = SliceSite {
            effect_type,
            op_id: ops[0].clone(),
            owner: rec.owner.to_string(),
            file: anchor.map(|a| a.source_unit_id.clone()),
            line: anchor.map(|a| a.start_line),
            column: anchor.map(|a| a.start_column),
            op_name,
            table,
            message,
            outcome: Outcome::NoMatch,
            route: None,
            routes: None,
            trap,
            route_trap_free: false,
        };
        site.outcome = outcome(req, &site, caller_dependent);
        sites.push((site, Some(ops)));
    }
    // Routes for the first matching sites, shortest first.
    let group_paths = GroupPaths::new(&tree.typed_edges, model);
    let mut matching: Vec<usize> = (0..sites.len())
        .filter(|&i| matches!(sites[i].0.outcome, Outcome::Match { .. }))
        .collect();
    let place = |s: &SliceSite| (s.file.clone(), s.line, s.column, s.op_id.clone());
    let mut lengths: HashMap<usize, usize> = HashMap::new();
    for &i in &matching {
        if let Some(ops) = &sites[i].1
            && let Some((steps, _)) = reach.tree_witness(live, ops)
        {
            lengths.insert(i, steps.len());
        }
    }
    matching.sort_by(|&a, &b| {
        lengths
            .get(&a)
            .unwrap_or(&usize::MAX)
            .cmp(lengths.get(&b).unwrap_or(&usize::MAX))
            .then_with(|| place(&sites[a].0).cmp(&place(&sites[b].0)))
    });
    for &i in matching.iter().take(MAX_ROUTES) {
        let Some(ops) = sites[i].1.clone() else {
            continue;
        };
        let owner = sites[i].0.owner.clone();
        let anchor = reach.routine(&owner).and_then(|r| anchor_in(r, &ops[0]));
        // B7/L23: a site that escapes on some routes only prints a route
        // that crosses no trap (the first route found is trapped).
        let open = match (&sites[i].0.trap, &scope) {
            (Some(TrapVerdict::SomeRoutes(_)), Some(sc)) => open_tree
                .get_or_insert_with(|| reach.untrapped_tree(root, sc))
                .as_ref()
                .and_then(|u| group_paths.own_path(&reach, Some(u), &ops).ok()),
            _ => None,
        };
        sites[i].0.route_trap_free = open.is_some();
        let route = match open.map_or_else(|| group_paths.own_path(&reach, Some(live), &ops), Ok) {
            Ok(path) => {
                let rs = route_sites(&reach, root, &path, (&owner, anchor));
                Ok(SliceRoute { path, sites: rs })
            }
            Err(n) => Err(n.reason),
        };
        sites[i].0.route = Some(route);
        // E2: the site's distinct routes over the state graph.
        if let Some(g) = &graph {
            let _r = crate::engine::perf_trace::span("prove", "prove.routes");
            let found = reach.routes(g, &ops[0], req.routes, req.route_budget, cancel);
            crate::engine::perf_trace::counter_delta(
                "prove.route_edges_examined",
                found.counts.examined as u64,
            );
            let routes = found
                .routes
                .iter()
                .map(|steps| {
                    let path = group_paths.path_of(&reach, steps, &ops[0]);
                    let rs = route_sites(&reach, root, &path, (&owner, anchor));
                    SliceRoute { path, sites: rs }
                })
                .collect();
            sites[i].0.routes = Some(RouteSet {
                routes,
                requested: req.routes,
                budget: req.route_budget,
                counts: found.counts,
                limit: found.limit,
            });
        }
    }
    let rank: HashMap<usize, usize> = matching.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut order: Vec<usize> = (0..sites.len()).collect();
    order.sort_by(|&a, &b| {
        let ra = rank.get(&a).copied().unwrap_or(usize::MAX);
        let rb = rank.get(&b).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb)
            .then_with(|| place(&sites[a].0).cmp(&place(&sites[b].0)))
    });
    let mut slots: Vec<Option<SliceSite>> = sites.into_iter().map(|(s, _)| Some(s)).collect();
    out.sites = order
        .into_iter()
        .map(|i| slots[i].take().expect("each site once"))
        .collect();
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// How many matching sites the human answer prints in full (the JSON lists
/// every one with a route).
pub const HUMAN_SITES: usize = 5;

/// How many other reachable sites (outside the filter) the human answer
/// names; the rest are counted (§4.3).
pub const OTHER_SITES: usize = 5;

/// A site's operation as the other-sites line names it: the op as written,
/// `(implicit error)` after an implicit one, else the effect type.
fn site_op(s: &SliceSite) -> String {
    match (&s.op_name, s.effect_type) {
        (Some(o), "ERROR_IMPLICIT") => format!("{o} (implicit error)"),
        (Some(o), _) => o.clone(),
        (None, t) => t.to_string(),
    }
}

/// The other reachable sites (outside the filter) as JSON
/// (`payload.result.otherSites[]`, §4.3).
#[must_use]
pub fn other_sites_value(slice: &SliceOut) -> serde_json::Value {
    serde_json::Value::Array(
        slice
            .sites
            .iter()
            .filter(|s| !matches!(s.outcome, Outcome::Match { .. }))
            .map(|s| {
                let mut m = serde_json::Map::new();
                m.insert("type".into(), s.effect_type.into());
                if let Some(o) = &s.op_name {
                    m.insert("op".into(), o.clone().into());
                }
                if let Some(t) = &s.table {
                    m.insert("table".into(), t.clone().into());
                }
                m.insert("file".into(), s.file.clone().into());
                m.insert(
                    "line".into(),
                    s.line.map(crate::engine::line_base::shown).into(),
                );
                m.insert("routine".into(), s.owner.clone().into());
                if let Outcome::Undecided(r) = s.outcome {
                    m.insert("undecided".into(), r.into());
                }
                serde_json::Value::Object(m)
            })
            .collect(),
    )
}

fn location(file: Option<&str>, line: Option<u32>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{}", crate::engine::line_base::shown(l)),
        (Some(f), None) => f.to_string(),
        _ => "no position kept".to_string(),
    }
}

/// A site's message as the human answer prints it.
fn message_human(m: &MessageText) -> String {
    match m {
        MessageText::Known {
            source,
            text,
            var,
            partial,
        } => {
            let from = match (source, var) {
                (&"label", Some(v)) => format!(" (Label {v})"),
                (&"textconst", Some(v)) => format!(" (TextConst {v})"),
                _ => " (literal)".to_string(),
            };
            let note = match partial {
                Some("placeholders") => "; its %-placeholder values are not known",
                Some(_) => "; the platform adds the field and table to it",
                None => "",
            };
            format!("\"{text}\"{from}{note}")
        }
        MessageText::Unknown { reason } => match *reason {
            "computed" => "message not known (a variable or expression)".to_string(),
            "error-info" => "message not known (an ErrorInfo)".to_string(),
            "platform" => "message not known (the platform's own message)".to_string(),
            _ => "message not known (no static text recorded)".to_string(),
        },
    }
}

/// What one undecided reason says, for `count` sites.
#[must_use]
pub fn undecided_detail(reason: &str, count: usize) -> String {
    let sites =
        crate::engine::l5::routine_rollup::plural(count, "reachable site", "reachable sites");
    match reason {
        "message-unknown" => format!(
            "{sites} with no static message text (computed, ErrorInfo, platform or not recorded): an unknown message never shows that it does not match"
        ),
        "message-placeholders" => format!(
            "{sites} whose text did not match but has %-placeholders: their values may hold the text"
        ),
        "message-platform-wrapped" => format!(
            "{sites} (FieldError) whose text did not match: the platform adds the field and table to it"
        ),
        "position-unknown" => format!("{sites} with no source position kept"),
        "operation-unknown" => format!("{sites} whose operation kind is not known"),
        "table-unknown" => format!("{sites} whose record's table is not known"),
        "temp-state-depends-on-caller" => {
            format!("{sites} whose record is temporary or not depending on the caller")
        }
        "unknown-table-write" => format!("{sites} writing a table that is not known"),
        "no-site" => {
            format!("{sites} with no site of their own (a platform routine that may raise one)")
        }
        _ => sites,
    }
}

/// One route's human lines: `{indent}{title} (N hops): <path>`, then its
/// guards (module doc, rules 2 and 3). E1's one route is `title` "route".
fn route_lines(
    r: &SliceRoute,
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    root: &str,
    title: &str,
    indent: &str,
) -> Vec<String> {
    use crate::engine::l5::routine_rollup::plural;
    let mut lines = vec![format!(
        "{indent}{title} ({}): {}",
        plural(r.path.steps.len(), "hop", "hops"),
        cx.own_path_line(root, &r.path)
    )];
    let (k, n) = r.guard_counts();
    if n == 0 {
        lines.push(format!(
            "{indent}guards: none enclose the route's sites (other conditions may still apply: \
             earlier exits, callee results, checks)"
        ));
        return lines;
    }
    lines.push(format!(
        "{indent}guards shown ({k} of {n} enclosing conditions; selected context, not all \
         necessary conditions; innermost first, per site):"
    ));
    // One line per route site that has a guard: the site, then its
    // conditions with the line each statement starts on.
    for rs in r.sites.iter().filter(|rs| !rs.shown.is_empty()) {
        let gs: Vec<String> = rs
            .shown
            .iter()
            .map(|g| {
                format!(
                    "{} (line {})",
                    g.describe(),
                    crate::engine::line_base::shown(g.line)
                )
            })
            .collect();
        let more = if rs.total > rs.shown.len() {
            format!("; +{} outer", rs.total - rs.shown.len())
        } else {
            String::new()
        };
        lines.push(format!(
            "{indent}  at {}: {}{more}",
            location(rs.file.as_deref(), rs.line),
            gs.join("; ")
        ));
    }
    lines
}

/// What a route enumeration says before its routes (E2, design §11):
/// how many, the definition, whether it completed or which cap fired, and
/// the candidate edges it examined and rejected (never a count of routes it
/// did not list).
fn route_set_lines(set: &RouteSet) -> Vec<String> {
    use crate::engine::l5::routine_rollup::plural;
    let c = &set.counts;
    let mut lines = vec![format!(
        "    routes: {} listed (up to {} asked), fewest hops first; a route is a simple path \
         over (routine, known argument values) states, told apart by its sequence of sites",
        set.routes.len(),
        set.requested
    )];
    let left = plural(c.unexplored, "partial route", "partial routes");
    lines.push(match set.limit {
        None => "    enumeration complete: these are every route to this site under that \
                 definition"
            .to_string(),
        Some(RouteLimit::RouteCap) => format!(
            "    limit: the {} asked were listed with {left} left; more routes may exist",
            plural(set.requested, "route", "routes")
        ),
        Some(RouteLimit::EdgeBudget) => format!(
            "    limit: the edge budget ({}) was spent with {left} left; routes may be missing",
            set.budget
        ),
        Some(RouteLimit::Cancelled) => {
            format!("    limit: cancelled with {left} left; routes may be missing")
        }
    });
    let mut search = format!(
        "    search: {} examined (budget {}); rejected: {} by literal arguments ({} at the \
         edge's own site, {} further on: every way on from it is ruled out), {} back to a \
         state already on the route (recursion), {} whose callee cannot reach the site at all",
        plural(c.examined, "candidate edge", "candidate edges"),
        set.budget,
        c.literal_dead + c.literal_blocked,
        c.literal_dead,
        c.literal_blocked,
        c.repeats,
        c.no_way
    );
    if c.duplicates > 0 {
        search.push_str(&format!(
            "; {} with the sites of a listed route",
            plural(c.duplicates, "path", "paths")
        ));
    }
    search.push_str(" (counts of edges, not of routes)");
    lines.push(search);
    lines
}

/// The human lines of a slice, after the answer line. `cx` renders routes
/// and locations.
#[must_use]
pub fn human_lines(
    slice: &SliceOut,
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    root: &str,
    detail: bool,
    surface: crate::engine::follow_up::Surface,
) -> Vec<String> {
    use crate::engine::l5::routine_rollup::plural;
    let mut lines = Vec::new();
    if !slice.complete {
        lines.push(
            "  slice:    not computed — the root's search over literal arguments stopped at its \
             state cap"
                .to_string(),
        );
        return lines;
    }
    let matching: Vec<&SliceSite> = slice.matching().collect();
    let shown = if detail {
        matching.len()
    } else {
        matching.len().min(HUMAN_SITES)
    };
    for s in &matching[..shown] {
        let what = s.op_name.as_deref().unwrap_or(s.effect_type);
        let table = s
            .table
            .as_deref()
            .map(|t| format!(" on {t}"))
            .unwrap_or_default();
        let mut row = format!(
            "  site: {}  [{}] {what}{table}",
            location(s.file.as_deref(), s.line),
            s.effect_type
        );
        if let Some(m) = &s.message {
            row.push_str(&format!(" — {}", message_human(m)));
        }
        if matches!(s.outcome, Outcome::Match { by_kind_word: true }) {
            row.push_str(" (matched by its kind word only)");
        }
        lines.push(row);
        // B7/L23: the traps on its routes.
        match &s.trap {
            Some(TrapVerdict::UnlessWritePending(at)) => {
                lines.push(format!("    {}", at.unless_pending_words()));
            }
            Some(TrapVerdict::SomeRoutes(at)) if s.route_trap_free => lines.push(format!(
                "    escapes on the route below (no trap on it); other routes are caught by the {}",
                at.words()
            )),
            // No trap-free route could be printed (its search stopped at the
            // cap, or the literal arguments rule it out): the route below is
            // a trapped one.
            Some(TrapVerdict::SomeRoutes(at)) => lines.push(format!(
                "    may escape on a route with no trap, which could not be printed; the route \
                 below is caught by the {}",
                at.words()
            )),
            _ => {}
        }
        match (&s.routes, &s.route) {
            // E2: the enumerated routes (an enumeration that listed none
            // falls back to E1's route below its summary).
            (Some(set), route) => {
                lines.extend(route_set_lines(set));
                for (k, r) in set.routes.iter().enumerate() {
                    lines.extend(route_lines(
                        r,
                        cx,
                        root,
                        &format!("route {}", k + 1),
                        "    ",
                    ));
                }
                if set.routes.is_empty()
                    && let Some(Ok(r)) = route
                {
                    lines.extend(route_lines(
                        r,
                        cx,
                        root,
                        "route off the search tree (the enumeration listed none)",
                        "    ",
                    ));
                }
            }
            (None, Some(Ok(r))) => lines.extend(route_lines(r, cx, root, "route", "    ")),
            (None, Some(Err(reason))) => lines.push(format!("    no route kept: {reason}")),
            (None, None) => lines.push(format!(
                "    route not computed (only the first {MAX_ROUTES} matching sites get one)"
            )),
        }
    }
    if matching.len() > shown {
        lines.push(format!(
            "  … +{} matching sites ({} lists every one; the JSON lists each with its route)",
            matching.len() - shown,
            crate::engine::follow_up::input(surface, crate::engine::follow_up::Input::Detail)
        ));
    }
    // Output validation V2 (§4.3, F18): the other reachable sites, outside
    // the filter, named (up to `OTHER_SITES` with op kind and place, the rest
    // counted), and the input that shows their routes.
    let others: Vec<&SliceSite> = slice
        .sites
        .iter()
        .filter(|s| !matches!(s.outcome, Outcome::Match { .. }))
        .collect();
    let undecided = slice.undecided();
    let blocked: usize = undecided.iter().map(|(_, v)| v.len()).sum();
    if !others.is_empty() {
        let named: Vec<String> = others
            .iter()
            .take(OTHER_SITES)
            .map(|s| format!("{} at {}", site_op(s), location(s.file.as_deref(), s.line)))
            .collect();
        let mut s = format!(
            "  other reachable {} (outside the filter): {} — {}",
            slice.request.question.site_noun(),
            others.len(),
            named.join("; ")
        );
        if others.len() > OTHER_SITES {
            s.push_str(&format!("; … +{}", others.len() - OTHER_SITES));
        }
        if blocked > 0 {
            s.push_str(&format!(
                " ({blocked} of them undecided: they block a \"no\")"
            ));
        }
        lines.push(s);
        lines.push(format!(
            "  to see their routes: {}",
            match (
                &slice.request.op,
                others.iter().find_map(|s| s.op_name.as_deref()),
                surface
            ) {
                (Some(_), Some(op), crate::engine::follow_up::Surface::Mcp) => {
                    format!("op: \"{op}\", or no op")
                }
                (Some(_), Some(op), crate::engine::follow_up::Surface::Cli) => {
                    format!("--op {op}, or no --op")
                }
                (Some(_), None, crate::engine::follow_up::Surface::Mcp) => "no op".to_string(),
                (Some(_), None, crate::engine::follow_up::Surface::Cli) => "no --op".to_string(),
                _ => "the same question without the filter".to_string(),
            }
        ));
    }
    if slice.literal_dead > 0 {
        lines.push(format!(
            "  ruled out: {} the literal arguments never run",
            plural(slice.literal_dead, "site", "sites")
        ));
    }
    if slice.temporary > 0 {
        lines.push(format!(
            "  temporary: {} on temporary records (not table writes)",
            plural(slice.temporary, "write", "writes")
        ));
    }
    if let Some(first) = slice.caught.first() {
        lines.push(format!(
            "  caught on every route by a trap: {} (first: {}, by the {}); not reachable as an \
             error of this routine",
            plural(slice.caught.len(), "error site", "error sites"),
            location(first.file.as_deref(), first.line),
            first.at.words()
        ));
    }
    if let Some(first) = matching.first()
        && let Some(Ok(r)) = &first.route
    {
        // The innermost guard of the terminal site, else of the last site
        // that has one: where to look first.
        if let Some(g) = r.sites.iter().rev().find_map(|rs| rs.shown.first()) {
            lines.push(format!(
                "  to confirm: set a breakpoint at {} and read `{}`; al-sem reads no runtime traces",
                location(Some(&g.file), Some(g.line)),
                g.text
            ));
        } else {
            lines.push(format!(
                "  to confirm: set a breakpoint at {}; al-sem reads no runtime traces",
                location(first.file.as_deref(), first.line)
            ));
        }
    }
    lines
}

fn message_value(m: &MessageText) -> serde_json::Value {
    match m {
        MessageText::Known {
            source,
            text,
            var,
            partial,
        } => {
            let mut v = serde_json::json!({"known": true, "source": source, "text": text});
            if let Some(var) = var {
                v["var"] = var.clone().into();
            }
            if let Some(p) = partial {
                v["partial"] = (*p).into();
            }
            v
        }
        MessageText::Unknown { reason } => serde_json::json!({"known": false, "reason": reason}),
    }
}

fn route_value(r: &SliceRoute) -> serde_json::Value {
    let (k, n) = r.guard_counts();
    let sites: Vec<serde_json::Value> = r
        .sites
        .iter()
        .map(|rs| {
            serde_json::json!({
                "file": rs.file,
                "line": rs.line.map(crate::engine::line_base::shown),
                "guards": rs.shown.iter().map(GuardOut::to_value).collect::<Vec<_>>(),
                "enclosingTotal": rs.total,
            })
        })
        .collect();
    serde_json::json!({
        "hops": r.path.hops,
        "witness": crate::engine::l5::prove::group_witness_value(&r.path),
        "sites": sites,
        "guardsShown": k,
        "guardsTotal": n,
        "guardsAre": "selected context, not all necessary conditions",
    })
}

/// A route enumeration's JSON (`routeSearch`, E2): `complete` true only
/// when no partial route was left, else `limit` names the cap; the counts
/// are of candidate edges, never of routes.
fn route_search_value(set: &RouteSet) -> serde_json::Value {
    let c = &set.counts;
    serde_json::json!({
        "requested": set.requested,
        "listed": set.routes.len(),
        "complete": set.limit.is_none(),
        "limit": set.limit.map(RouteLimit::as_str),
        "edgeBudget": set.budget,
        "edgesExamined": c.examined,
        "edgesRejected": {
            "literalArgumentsAtSite": c.literal_dead,
            "literalArgumentsFurtherOn": c.literal_blocked,
            "repeatsState": c.repeats,
            "cannotReachSite": c.no_way,
        },
        "duplicatePaths": c.duplicates,
        "partialRoutesLeft": c.unexplored,
        "route": "a simple path over (routine, known argument values) states from the routine \
                  to the first state where the site can run",
        "distinctBy": "the sequence of (site, callee)",
        "order": "fewest hops, then the sequence of site ids",
    })
}

/// One site as JSON: the `evidence` row shape `verify.py` reads
/// (`evidence.{file, line}`), with the slice's fields beside it.
#[must_use]
pub fn site_value(s: &SliceSite) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), s.effect_type.into());
    m.insert(
        "evidence".into(),
        serde_json::json!({
            "sourceKind": if s.file.is_some() { "source" } else { "unavailable" },
            "file": s.file,
            "line": s.line.map(crate::engine::line_base::shown),
            "column": s.column,
        }),
    );
    m.insert("routine".into(), s.owner.clone().into());
    if !s.op_id.is_empty() {
        m.insert("operationId".into(), s.op_id.clone().into());
    }
    if let Some(o) = &s.op_name {
        m.insert("op".into(), o.clone().into());
    }
    if let Some(t) = &s.table {
        m.insert("table".into(), t.clone().into());
    }
    if let Some(msg) = &s.message {
        m.insert("message".into(), message_value(msg));
    }
    match s.outcome {
        Outcome::Match { by_kind_word } => {
            if by_kind_word {
                m.insert("matchedBy".into(), "kind-word".into());
            }
        }
        Outcome::NoMatch => {}
        Outcome::Undecided(r) => {
            m.insert("undecided".into(), r.into());
        }
    }
    // B7/L23 (additive): the traps on its routes.
    if let Some(t) = &s.trap {
        m.insert("trap".into(), trap_verdict_value(t));
    }
    match &s.route {
        Some(Ok(r)) => {
            m.insert("route".into(), route_value(r));
        }
        Some(Err(reason)) => {
            m.insert("routeUnavailable".into(), (*reason).into());
        }
        None => {}
    }
    // E2: the enumerated routes and what the enumeration examined.
    if let Some(set) = &s.routes {
        m.insert(
            "routes".into(),
            set.routes
                .iter()
                .map(route_value)
                .collect::<Vec<_>>()
                .into(),
        );
        m.insert("routeSearch".into(), route_search_value(set));
    }
    m.insert("confidence".into(), "static".into());
    serde_json::Value::Object(m)
}

/// How many undecided sites of one kind the JSON lists (their count is
/// whole): on Continia DO, `IssueReminder`'s cone has about 4,900.
pub const UNDECIDED_LISTED: usize = 10;

/// The slice's own JSON block (`payload.slice`): counts, the first
/// [`UNDECIDED_LISTED`] undecided sites of each kind, the label.
#[must_use]
pub fn slice_value(slice: &SliceOut) -> serde_json::Value {
    let undecided: Vec<serde_json::Value> = slice
        .undecided()
        .into_iter()
        .map(|(k, v)| {
            serde_json::json!({
                "kind": k,
                "count": v.len(),
                "sites": v.iter().take(UNDECIDED_LISTED).map(|s| site_value(s)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let caught: Vec<serde_json::Value> = slice
        .caught
        .iter()
        .map(|c| {
            serde_json::json!({
                "file": c.file,
                "line": c.line.map(crate::engine::line_base::shown),
                "trap": trap_at_value(&c.at),
            })
        })
        .collect();
    let mut v = serde_json::json!({
        "complete": slice.complete,
        "reachableSites": slice.sites.len(),
        "matchingSites": slice.matching().count(),
        "literalDead": slice.literal_dead,
        "temporaryWrites": slice.temporary,
        "undecided": undecided,
        "label": STATIC_LABEL,
    });
    // Additive: present only when a site is caught.
    if !caught.is_empty() {
        v["caught"] = caught.into();
    }
    v
}

/// B7/L23: a trap's place as JSON (`line` 1-based).
fn trap_at_value(at: &TrapAt) -> serde_json::Value {
    serde_json::json!({
        "kind": at.kind.id(),
        "routine": at.routine,
        "file": at.file,
        "line": at.line.map(crate::engine::line_base::shown),
    })
}

/// B7/L23: a site's trap verdict as JSON.
fn trap_verdict_value(t: &TrapVerdict) -> serde_json::Value {
    let (verdict, at) = match t {
        TrapVerdict::Caught(at) => ("caught", at),
        TrapVerdict::UnlessWritePending(at) => ("escapes-if-write-pending", at),
        TrapVerdict::SomeRoutes(at) => ("trapped-on-some-routes", at),
    };
    let mut v = trap_at_value(at);
    v["verdict"] = verdict.into();
    v
}

#[cfg(test)]
#[path = "prove_slice_tests.rs"]
mod tests;
