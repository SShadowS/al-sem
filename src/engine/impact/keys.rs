//! **Semantic effect keys** (agent-tasks design §10.3, astra a11): what one
//! routine does, keyed by WHAT, never by where. A display anchor
//! (`file:line`) is not an identity: a comment inserted above an unchanged
//! `Commit()` moves every line below it, and a delta keyed by position would
//! print a removal and an addition. So each side of a before/after answer
//! keeps, per routine:
//!
//! - **own sites**, a MULTISET by key: every site in the routine's own body,
//!   from the engine's direct capability facts (`direct_facts_for_routine`:
//!   the sites the cone starts from, unreachable sites already dropped) and
//!   its resolved calls. Keys:
//!
//!   | key | from |
//!   |---|---|
//!   | `write|<table>|<Op>|<class>` | an `Insert`/`Modify`/`ModifyAll`/`Delete`/`DeleteAll`/`Rename`; class `physical`, `temporary`, `caller-dependent` or `unknown` |
//!   | `commit` | a `Commit()` |
//!   | `error|<op>|<message>` | an `Error(...)` (message: `label:<name>`, `text:<hash>`, `expr:<hash>`), an implicit error |
//!   | `ui|<op>` | `Message`, `Confirm`, a page or report run, a dialog, `Hyperlink` |
//!   | `http|<method>`, `file|<op>`, `background|<target>` | the I/O families |
//!   | `run|<kind>|<target>` | `Codeunit.Run`, `Page.RunModal`, … |
//!   | `other|<resource kind>|<op>` | telemetry, isolated storage |
//!   | `callee|<stable id>` | a resolved call to a routine |
//!   | `event|<stable id>` | a call that raises an event (its publisher) |
//!
//!   Each site keeps its anchor (display only) and its normalized operation
//!   identity (operation, receiver, argument shape), which pairs sites of one
//!   key across the two sides in source order (§10.3, last bullet).
//! - **the cone's keys**, a SET: every effect the routine's digest entry
//!   reaches, own or beyond calls and dependency entry points
//!   (`<TYPE>|<resource>`), literal-dead rows left out.
//! - **its coverage**: the digest entry's status and reasons, the gaps on its
//!   cone, the depth-bounded site searches, the literal-dead rows, and the own
//!   write keys whose class is uncertain. A key only the base side has is
//!   "no longer observed", not "removed", when the new side is incomplete
//!   (§10.4, [`super::delta`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::engine::l4::capability_cone::{CapabilityExtra, direct_facts_for_routine};
use crate::engine::l5::digest_cli::{DigestEntryFull, SITES_PARTIAL};
use crate::engine::l5::routine_rollup::{RollupCx, RollupTotals};
use crate::program::body::features::PCallee;
use crate::program::model::calls::CallEdge;
use crate::program::model::taxonomy::DispatchKind;
use crate::program::model::workspace::{Model, ModelRoutine};

/// The summaries' schema version (a cached base whose summaries carry
/// another is not read). 2: a routine's labels and callers carry its short
/// handle (`Name/n`), and the summary its `handle`. 3 (engine limits B2,
/// per-arm ids): a routine declared in two `#if` arms of one file gives each
/// arm its own site ids, so each arm's site carries its own line; a base
/// cached before could cite the other arm's line for it, and a delta against
/// it would report a moved site that did not move. 4 (engine limits B2, wave 2
/// final review): one id declared in two FILES numbers its rows apart too
/// (the second file's site ids move), and its rows are read as alternatives
/// (an attribute or a property one declaration lacks is a "may"). 5 (the
/// follow-up): each file's site numbers of such a routine sit in a block of
/// its own (every file's ids move once), a declaration's own attribute is
/// definite for its own body, and a dispatch reaches only the declarations
/// that subscribe. 6 (follow-up 2): a subscriber whose declarations subscribe
/// differently is one routine per declaration (new routine, stable and site
/// ids). 7 (follow-up 3): the program graph splits it too, and the model's
/// ids of a split declaration derive from the graph's (they all move once).
/// 8: an `[EventSubscriber]` the engine cannot read is a coverage gap.
/// 9: also one whose validate-event field cannot be read.
/// 10 (ENG-08): a statement that starts an XmlPort import is an implicit
/// error. The bump also covers ENG-06's open record operations and ENG-07's
/// request-page triggers, which moved summaries at 9 (a base cached by a
/// binary of the same version would show their facts as added).
/// 11 (ENG-09): a checked `Codeunit.Run` commits.
/// 12 (ENG-22): a checked `Codeunit.Run` raises its own error when a write is
/// pending at it.
pub const SUMMARY_SCHEMA: u32 = 12;

/// One own site of a key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    /// `ws:<path>` of the site (display).
    pub file: String,
    /// 1-based (display).
    pub line: u32,
    /// 0-based (source order only).
    pub column: u32,
    /// The normalized operation identity: operation, receiver, argument
    /// shape. Never a position.
    pub ident: String,
}

/// What the routine's analysis could and could not see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// The digest entry's coverage status (`complete`, `partial`, …).
    pub status: String,
    pub reasons: Vec<String>,
    /// Unresolved calls on the cone (every one; by where the site is).
    pub unresolved: usize,
    pub gaps_workspace: usize,
    pub gaps_dependency: usize,
    /// The effect types whose site search stopped at its depth bound.
    pub sites_partial: Vec<String>,
    /// The unresolved-call traversal stopped at its depth bound.
    pub traversal_truncated: bool,
    /// Literal analysis: rows unreachable once literal Boolean arguments are
    /// followed (left out of the keys), and the tables only they write.
    pub literal_dead_rows: usize,
    pub literal_dead_tables: usize,
    /// Own write keys whose class is not decided (`unknown`,
    /// `caller-dependent`).
    pub write_class_uncertain: Vec<String>,
    /// No body (an interface or control add-in declaration), or the digest
    /// did not compute this routine.
    pub no_body: bool,
}

impl Coverage {
    /// Whether a key this side lacks may still be there unseen.
    #[must_use]
    pub fn incomplete(&self) -> bool {
        self.status != "complete"
            || self.unresolved > 0
            || !self.sites_partial.is_empty()
            || self.traversal_truncated
            || self.no_body
    }

    /// Why [`Self::incomplete`], in words (empty when complete).
    #[must_use]
    pub fn why(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.no_body {
            w.push("no body was analyzed".to_string());
        }
        if self.unresolved > 0 {
            w.push(crate::engine::l5::routine_rollup::plural(
                self.unresolved,
                "unresolved call in its cone",
                "unresolved calls in its cone",
            ));
        }
        if !self.sites_partial.is_empty() {
            w.push(format!(
                "the site search stopped at its depth bound ({})",
                self.sites_partial.join(", ")
            ));
        }
        if self.traversal_truncated {
            w.push("the cone walk stopped at its depth bound".to_string());
        }
        if self.status != "complete" && !self.status.is_empty() && w.is_empty() {
            w.push(if self.reasons.is_empty() {
                format!("coverage {}", self.status)
            } else {
                format!("coverage {} ({})", self.status, self.reasons.join(", "))
            });
        }
        w
    }
}

/// One routine's summary on one side (see the module doc).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootSummary {
    pub stable_id: String,
    /// `Codeunit X::Name`.
    pub display: String,
    /// Its short handle on this side ([`Names::handle`]; empty when the
    /// display is its own).
    #[serde(default)]
    pub handle: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Own sites by key.
    pub own: BTreeMap<String, Vec<Site>>,
    /// The cone's keys.
    pub cone: BTreeSet<String>,
    /// A display text per key (own and cone).
    pub labels: BTreeMap<String, String>,
    pub coverage: Coverage,
    /// Who calls it on this side (direct resolved calls), for a deleted
    /// routine's "still called in the base".
    pub callers: Callers,
}

/// Direct callers on one side.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Callers {
    pub count: usize,
    /// Callers that are page action triggers.
    pub page_actions: usize,
    /// The first ten: `{display, file, line}`.
    pub first: Vec<CallerSite>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerSite {
    pub display: String,
    pub file: String,
    pub line: u32,
}

/// The names a summary prints.
pub struct Names<'m> {
    table_name: HashMap<&'m str, &'m str>,
    /// Internal routine id → its row.
    by_id: HashMap<&'m str, &'m ModelRoutine>,
    /// Stable id → its first row.
    by_stable: HashMap<&'m str, &'m ModelRoutine>,
    /// Stable id → every row of it, in row order (engine limits B2: a
    /// routine declared in two `#if` arms is two rows, each with its own
    /// sites).
    rows_by_stable: HashMap<&'m str, Vec<&'m ModelRoutine>>,
    /// Internal routine id → the edges it emits.
    edges_from: HashMap<&'m str, Vec<&'m CallEdge>>,
    /// Internal routine id → the edges that reach it (callers).
    edges_to: HashMap<&'m str, Vec<&'m CallEdge>>,
    /// The short handles the rest of the output names an overload by
    /// ([`RollupCx::handle`]: `Name/n`, `Name/n[position: type]`, ` [<app>]`).
    rollup: &'m RollupCx<'m>,
}

impl<'m> Names<'m> {
    #[must_use]
    pub fn new(model: &'m Model, rollup: &'m RollupCx<'m>) -> Names<'m> {
        let ws = &model.workspace;
        let mut by_stable = HashMap::new();
        let mut rows_by_stable: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
        for r in &ws.routines {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(r);
            rows_by_stable
                .entry(r.stable_routine_id.as_str())
                .or_default()
                .push(r);
        }
        let mut edges_from: HashMap<&str, Vec<&CallEdge>> = HashMap::new();
        let mut edges_to: HashMap<&str, Vec<&CallEdge>> = HashMap::new();
        for e in &model.calls.edges {
            edges_from.entry(e.from.as_str()).or_default().push(e);
            if let Some(t) = &e.to {
                edges_to.entry(t.as_str()).or_default().push(e);
            }
        }
        Names {
            table_name: ws
                .tables
                .iter()
                .map(|t| (t.id.as_str(), t.name.as_str()))
                .collect(),
            by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            by_stable,
            rows_by_stable,
            edges_from,
            edges_to,
            rollup,
        }
    }

    /// Every row of a stable id, in row order (empty when none).
    #[must_use]
    pub fn rows(&self, stable_id: &str) -> &[&'m ModelRoutine] {
        self.rows_by_stable
            .get(stable_id)
            .map_or(&[], Vec::as_slice)
    }

    /// The one routine display of a routine row (V3 §2.1, no member
    /// caption).
    #[must_use]
    pub fn display(&self, r: &ModelRoutine) -> String {
        crate::engine::l5::routine_rollup::format_routine(&self.rollup.names.parts_of(r), false)
    }

    /// A routine row's short handle: empty when its display is its own, else
    /// what tells it apart (`/1`, `/2[1: Integer]`, ` [<app>]`).
    #[must_use]
    pub fn handle(&self, r: &ModelRoutine) -> String {
        self.rollup.handle(&r.stable_routine_id)
    }

    /// How a label or a caller names a routine: [`Self::display`], which
    /// prints an overloaded name's parameter types (V3 §2.1).
    #[must_use]
    pub fn named(&self, r: &ModelRoutine) -> String {
        self.display(r)
    }

    /// The row of a stable id.
    #[must_use]
    pub fn routine(&self, stable_id: &str) -> Option<&'m ModelRoutine> {
        self.by_stable.get(stable_id).copied()
    }

    fn table(&self, id: &str) -> String {
        self.table_name
            .get(id)
            .map_or_else(|| id.to_string(), |n| (*n).to_string())
    }

    /// `ws:<path>` of a source unit id.
    fn file(&self, unit: &str) -> String {
        unit.to_string()
    }
}

/// A write's class from its operation's temp state.
fn write_class(t: Option<&crate::program::body::features::PTempState>) -> &'static str {
    use crate::program::body::features::known_temp_suppresses;
    let known = t.filter(|t| t.kind == "known").and_then(|t| t.value);
    match t {
        Some(_) if known_temp_suppresses(known) => "temporary",
        Some(_) if known.is_some() => "physical",
        Some(t) if t.kind == "parameter-dependent" => "caller-dependent",
        _ => "unknown",
    }
}

/// A value source in a key (`Codeunit::"X"`, a literal, else its kind).
fn source_text(v: Option<&BodyValueSourceRef<'_>>) -> String {
    match v {
        None => "unknown".into(),
        Some(s) => s.text(),
    }
}

/// A fact's value source (the L4 type).
enum BodyValueSourceRef<'a> {
    L4(&'a crate::engine::l4::capability_cone::ValueSource),
}

impl BodyValueSourceRef<'_> {
    fn text(&self) -> String {
        use crate::engine::l4::capability_cone::ValueSource as V;
        let BodyValueSourceRef::L4(v) = self;
        match v {
            V::Literal { value } => value.to_lowercase(),
            V::Enum { enum_name, member } => format!(
                "{}::{}",
                enum_name.to_lowercase(),
                member.as_deref().unwrap_or("").to_lowercase()
            ),
            V::ConstantVar { initializer, .. } => BodyValueSourceRef::L4(initializer).text(),
            V::Parameter { index, .. } => format!("parameter {index}"),
            V::TableField { field_name, .. } => format!("field {}", field_name.to_lowercase()),
            V::Expression => "expression".into(),
            V::Unknown => "unknown".into(),
        }
    }
}

/// A short hash of `text` (message keys).
fn short_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..16].to_string()
}

/// An `Error(...)`'s message key: the label's name, the static text's hash,
/// else the expression's normalized hash.
fn message_key(
    r: &ModelRoutine,
    args: &[crate::program::body::features::PExpressionInfo],
) -> String {
    let Some(first) = args.first() else {
        return "none".into();
    };
    let text = first.text.trim();
    if text.starts_with('\'') {
        return format!("text:{}", short_hash(text));
    }
    let name = text.trim_matches('"').to_lowercase();
    if r.variables.iter().any(|v| {
        v.name.eq_ignore_ascii_case(&name) && v.declared_type.to_lowercase().starts_with("label")
    }) {
        return format!("label:{name}");
    }
    let norm: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    format!("expr:{}", short_hash(&norm))
}

/// The own sites of routine `rows` (its rows: one, or each `#if` arm of a
/// routine declared in two, engine limits B2) by key, with their labels. Each
/// row gives its own direct facts, and each site is read in the row that
/// holds it (the LAST holding row, class P).
fn own_keys(
    names: &Names<'_>,
    rows: &[&ModelRoutine],
    labels: &mut BTreeMap<String, String>,
) -> BTreeMap<String, Vec<Site>> {
    let mut own: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    let Some(&r) = rows.last() else {
        return own;
    };
    let call_site = |id: &str| {
        rows.iter()
            .rev()
            .find_map(|r| r.call_sites.iter().find(|c| c.id == id))
    };
    let op_row = |id: &str| -> Option<&ModelRoutine> {
        rows.iter().rev().copied().find(|r| {
            r.record_operations.iter().any(|o| o.id == id)
                || r.operation_sites.iter().any(|o| o.id == id)
        })
    };
    let op_site = |id: &str| {
        let r = op_row(id)?;
        r.record_operations
            .iter()
            .find(|o| o.id == id)
            .map(|o| {
                (
                    &o.source_anchor,
                    format!(
                        "{}/{}/{}",
                        o.op.to_lowercase(),
                        o.record_variable_name.to_lowercase(),
                        o.field_arguments.as_ref().map_or(0, Vec::len)
                    ),
                )
            })
            .or_else(|| {
                r.operation_sites
                    .iter()
                    .find(|o| o.id == id)
                    .map(|o| (&o.source_anchor, o.kind.clone()))
            })
    };
    let site_of = |anchor: &crate::program::body::features::PAnchor, ident: String| Site {
        file: names.file(&anchor.source_unit_id),
        line: anchor.start_line + 1,
        column: anchor.start_column,
        ident,
    };
    let call_ident = |c: &crate::program::body::features::PCallSite| {
        format!(
            "{}/{}",
            c.callee_text.to_lowercase().replace(' ', ""),
            c.argument_texts.len()
        )
    };
    // Every row's own facts; a fact two rows share (one site in both) once.
    let mut facts = Vec::new();
    for row in rows {
        for f in direct_facts_for_routine(row, &[]).0 {
            if !facts.contains(&f) {
                facts.push(f);
            }
        }
    }
    for f in &facts {
        let anchor = if let Some(op) = f.witness_operation_id.as_deref() {
            op_site(op)
        } else if let Some(cs) = f.witness_callsite_id.as_deref() {
            call_site(cs).map(|c| (&c.source_anchor, call_ident(c)))
        } else {
            None
        };
        let source = f.resource_arg_source.as_ref().map(BodyValueSourceRef::L4);
        let (key, label) = match (f.resource_kind, f.op) {
            ("table", "insert" | "modify" | "delete" | "rename") => {
                let (sub, temp) = match &f.extra {
                    Some(CapabilityExtra::Table {
                        op_subtype,
                        temp_state,
                        ..
                    }) => (op_subtype.clone(), temp_state.as_ref()),
                    _ => (None, None),
                };
                let op = sub.unwrap_or_else(|| f.op.to_string());
                let class = write_class(temp);
                let table = f.resource_id.clone().unwrap_or_else(|| "?".into());
                let shown = match &f.resource_id {
                    Some(t) => names.table(t),
                    None => "a table not resolved".into(),
                };
                (
                    format!("write|{table}|{op}|{class}"),
                    format!("write {shown} {op} ({class})"),
                )
            }
            ("table", _) | ("event", "subscribe") => continue,
            // ENG-09: a checked run's commit keys as its cone row does
            // (`COMMIT|checked-run`, `delta::cone_twin`).
            ("transaction", "commit") => match f.resource_id.as_deref() {
                Some(r) => (format!("commit|{r}"), "commit (checked run)".into()),
                None => ("commit".into(), "commit".into()),
            },
            ("error", op) => {
                let msg = if op == "error-throw" {
                    let r = f
                        .witness_operation_id
                        .as_deref()
                        .and_then(op_row)
                        .unwrap_or(r);
                    f.witness_operation_id
                        .as_deref()
                        .and_then(|id| r.operation_sites.iter().find(|o| o.id == id))
                        .and_then(|o| {
                            r.call_sites.iter().find(|c| {
                                matches!(&c.callee, PCallee::Bare { name } if name.eq_ignore_ascii_case("error"))
                                    && c.source_anchor.start_line == o.source_anchor.start_line
                                    && c.source_anchor.start_column == o.source_anchor.start_column
                            })
                        })
                        .map_or_else(|| "none".into(), |c| message_key(r, &c.argument_infos))
                } else if f.resource_id.as_deref()
                    == Some(crate::engine::l4::capability_cone::RUN_COMMIT)
                {
                    // ENG-22: a checked run's own error keys as its cone row
                    // does (`ERROR_IMPLICIT|checked-run`, `delta::cone_twin`).
                    crate::engine::l4::capability_cone::RUN_COMMIT.to_string()
                } else {
                    String::new()
                };
                let shown = match msg.split_once(':') {
                    None if msg == crate::engine::l4::capability_cone::RUN_COMMIT => {
                        " (checked run with a write pending)".to_string()
                    }
                    Some(("label", n)) => format!(" (label {n})"),
                    Some(("text", h)) => format!(" (text #{})", &h[..8.min(h.len())]),
                    Some(("expr", h)) => format!(" (expression #{})", &h[..8.min(h.len())]),
                    _ => String::new(),
                };
                (
                    format!("error|{op}|{msg}"),
                    format!("error {}{shown}", op.trim_start_matches("error-")),
                )
            }
            ("ui", "ui-error") => continue,
            ("ui", op) => (
                format!("ui|{op}"),
                format!("UI {}", op.trim_start_matches("ui-")),
            ),
            ("http", _) => {
                let method = match &f.extra {
                    Some(CapabilityExtra::Http { method, .. }) => method.clone(),
                    _ => f.op.to_string(),
                };
                (format!("http|{method}"), format!("HTTP {method}"))
            }
            ("file", op) => (format!("file|{op}"), format!("file {op}")),
            ("background", _) => {
                let t = source_text(source.as_ref());
                (format!("background|{t}"), format!("background task {t}"))
            }
            (kind, "execute") => {
                let t = source_text(source.as_ref());
                (format!("run|{kind}|{t}"), format!("runs {kind} {t}"))
            }
            (kind, op) => (format!("other|{kind}|{op}"), format!("{kind} {op}")),
        };
        labels.entry(key.clone()).or_insert(label);
        let site = anchor.map_or_else(
            || Site {
                ident: f.op.to_string(),
                ..Site::default()
            },
            |(a, ident)| site_of(a, ident),
        );
        own.entry(key).or_default().push(site);
    }
    // Resolved calls to routines (one per call site and target).
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for e in names.edges_from.get(r.id.as_str()).into_iter().flatten() {
        let Some(to) = e.to.as_deref() else {
            continue;
        };
        if matches!(
            e.dispatch_kind,
            DispatchKind::Builtin | DispatchKind::ImplicitTrigger | DispatchKind::TableEvent
        ) || !seen.insert((e.callsite_id.as_str(), to))
        {
            continue;
        }
        let Some(callee) = names.by_id.get(to) else {
            continue;
        };
        let (key, label) = if callee.kind == "event-publisher" {
            (
                format!("event|{}", callee.stable_routine_id),
                format!("raises {}", names.named(callee)),
            )
        } else {
            (
                format!("callee|{}", callee.stable_routine_id),
                format!("calls {}", names.named(callee)),
            )
        };
        labels.entry(key.clone()).or_insert(label);
        let site = call_site(&e.callsite_id).map_or_else(
            || Site {
                ident: "call".into(),
                ..Site::default()
            },
            |c| site_of(&c.source_anchor, call_ident(c)),
        );
        own.entry(key).or_default().push(site);
    }
    for sites in own.values_mut() {
        sites.sort_by_key(|a| (a.line, a.column));
    }
    own
}

/// A digest row's resource id (`detail.resourceId`).
fn resource(e: &crate::engine::l5::digest_cli::DigestEffectFull) -> Option<&str> {
    e.detail
        .iter()
        .find(|(k, _)| k == "resourceId")
        .map(|(_, v)| v.as_str())
}

fn resource_display(e: &crate::engine::l5::digest_cli::DigestEffectFull) -> Option<&str> {
    e.detail
        .iter()
        .find(|(k, _)| k == "resourceDisplay")
        .map(|(_, v)| v.as_str())
}

/// The cone's keys of a digest entry, with their labels; and how many rows
/// literal analysis left out.
fn cone_keys(
    entry: &DigestEntryFull,
    labels: &mut BTreeMap<String, String>,
) -> (BTreeSet<String>, usize) {
    let mut cone = BTreeSet::new();
    let mut dead = 0;
    let mut add = |ty: &str, rid: Option<&str>, shown: Option<&str>| {
        let key = format!("{ty}|{}", rid.unwrap_or(""));
        labels.entry(key.clone()).or_insert_with(|| match shown {
            Some(s) if !s.is_empty() => format!("{ty} {s}"),
            _ => ty.to_string(),
        });
        cone.insert(key);
    };
    for e in &entry.effects {
        if e.literal_unreachable {
            dead += 1;
            continue;
        }
        if e.group_resources.is_empty() {
            add(&e.effect_type, resource(e), resource_display(e));
        } else {
            for (rid, shown) in &e.group_resources {
                add(&e.effect_type, Some(rid), Some(shown));
            }
        }
    }
    (cone, dead)
}

/// Direct callers of `r` on this side.
fn callers(names: &Names<'_>, r: &ModelRoutine) -> Callers {
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut c = Callers::default();
    for e in names.edges_to.get(r.id.as_str()).into_iter().flatten() {
        if !seen.insert((e.from.as_str(), e.callsite_id.as_str())) {
            continue;
        }
        let Some(from) = names.by_id.get(e.from.as_str()) else {
            continue;
        };
        c.count += 1;
        if from.object_type == "Page" && from.name.eq_ignore_ascii_case("OnAction") {
            c.page_actions += 1;
        }
        if c.first.len() < 10 {
            // Engine limits B2: in the caller's row (`#if` arm) that holds it.
            let site = names
                .rows(&from.stable_routine_id)
                .iter()
                .rev()
                .find_map(|r| r.call_sites.iter().find(|s| s.id == e.callsite_id));
            c.first.push(CallerSite {
                display: names.named(from),
                file: site.map_or_else(
                    || from.source_anchor.source_unit_id.clone(),
                    |s| s.source_anchor.source_unit_id.clone(),
                ),
                line: site.map_or(from.source_anchor.start_line, |s| {
                    s.source_anchor.start_line
                }) + 1,
            });
        }
    }
    c
}

/// The summary of `stable_id`: its own sites and callers from the model, its
/// cone and coverage from its digest entry and rollup totals (`None` when
/// the digest did not compute it). `None` when the model has no such
/// routine.
#[must_use]
pub fn summarize(
    names: &Names<'_>,
    stable_id: &str,
    entry: Option<&DigestEntryFull>,
    totals: Option<&RollupTotals>,
) -> Option<RootSummary> {
    let r = names.routine(stable_id)?;
    let mut labels = BTreeMap::new();
    let own = own_keys(names, names.rows(stable_id), &mut labels);
    let (cone, literal_dead_rows) =
        entry.map_or_else(|| (BTreeSet::new(), 0), |e| cone_keys(e, &mut labels));
    let mut sites_partial: Vec<String> = entry
        .map(|e| {
            e.entry_diagnostics
                .iter()
                .filter(|d| d.kind == SITES_PARTIAL)
                .filter_map(|d| d.effect_type.clone())
                .collect()
        })
        .unwrap_or_default();
    sites_partial.sort();
    sites_partial.dedup();
    let write_class_uncertain = own
        .keys()
        .filter(|k| {
            k.starts_with("write|") && (k.ends_with("|unknown") || k.ends_with("|caller-dependent"))
        })
        .cloned()
        .collect();
    let coverage = Coverage {
        status: entry.map_or_else(|| "not-computed".into(), |e| e.coverage_status.clone()),
        reasons: entry
            .map(|e| e.coverage_reasons.clone())
            .unwrap_or_default(),
        unresolved: entry.map_or(0, |e| e.unresolved.len()),
        gaps_workspace: totals.map_or(0, |t| t.gaps_workspace),
        gaps_dependency: totals.map_or(0, |t| t.gaps_dependency),
        sites_partial,
        traversal_truncated: entry.is_some_and(|e| e.unresolved_traversal.truncated),
        literal_dead_rows,
        literal_dead_tables: totals.map_or(0, |t| t.literal_dead_tables),
        write_class_uncertain,
        no_body: entry.is_none_or(|e| e.declaration_only) || !r.body_available,
    };
    Some(RootSummary {
        stable_id: stable_id.to_string(),
        display: names.display(r),
        handle: names.handle(r),
        file: Some(r.source_anchor.source_unit_id.clone()),
        line: Some(r.source_anchor.start_line + 1),
        own,
        cone,
        labels,
        coverage,
        callers: callers(names, r),
    })
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
