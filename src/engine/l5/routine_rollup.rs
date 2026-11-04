//! The per-routine rollup every effects renderer reads (MCP output wave 2,
//! W2-2; design `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md`
//! §3.1, §3.6, §3.8).
//!
//! One root's [`DigestEntryFull`] (after the L4 temp-state join and the
//! literal pass) is sorted ONCE into sections by where the operation runs and
//! by write class ([`crate::engine::l5::rollup::classify_detail`]):
//!
//! - writes in workspace code: own rows with a source anchor, physical class,
//!   one record per table;
//! - caller-dependent writes: own rows in class `callerDependent`;
//! - crossings: every boundary group row, by its crossing (§3.3), with the
//!   refined resource lists;
//! - writes with no path kept: own rows whose evidence is unavailable, with the
//!   owner the operation index recovers (§3.5);
//! - temporary records, literal-dead rows, commits, events, the other kind
//!   families and the gaps.
//!
//! [`RollupTotals`] is the one summary: explain's `cards[].summary`, digest's
//! `entries[].summary` and both MCP summaries serialize it, and the human
//! `summary:` line prints it. Every total is a union by identity, never a sum
//! of section counts (T-W2-1).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Value, json};

use crate::engine::l4::operation_index::{IndexScope, OperationIndex, OwnerLookup};
use crate::engine::l5::digest_cli::{DigestEffectFull, DigestEntryFull};
use crate::engine::l5::rollup::{CallerReason, PhysicalReason, WriteClass, classify_detail};
use crate::engine::line_base::shown;
use crate::program::model::declaring::DeclaringRows;
use crate::program::model::role::Role;
use crate::program::model::workspace::Model;

/// A folded list (design §3.8 rule 4): the items shown, how many are hidden,
/// and the unit the full count is stated in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded<T> {
    pub shown: Vec<T>,
    pub hidden: usize,
    pub unit: &'static str,
}

impl<T> Folded<T> {
    /// The full count.
    #[must_use]
    pub fn total(&self) -> usize {
        self.shown.len() + self.hidden
    }
}

impl Folded<String> {
    /// `a, b, … +N <unit>` (or just the items when nothing is hidden).
    #[must_use]
    pub fn joined(&self) -> String {
        let mut s = self.shown.join(", ");
        if self.hidden > 0 {
            if !s.is_empty() {
                s.push_str(", ");
            }
            s.push_str(&format!("… +{} {}", self.hidden, self.unit));
        }
        s
    }
}

/// The one truncation helper (design §3.8 rule 4): the first `cap` items,
/// or every item when there are at most a quarter more than `cap` (tables:
/// 12, all when 15 or fewer). `all` (`--detail`, `--all`) shows everything.
#[must_use]
pub fn fold<T>(items: Vec<T>, cap: usize, unit: &'static str, all: bool) -> Folded<T> {
    let n = fold_len(items.len(), cap, all);
    let hidden = items.len() - n;
    let mut shown = items;
    shown.truncate(n);
    Folded {
        shown,
        hidden,
        unit,
    }
}

/// How many of `len` items [`fold`] shows.
#[must_use]
pub fn fold_len(len: usize, cap: usize, all: bool) -> usize {
    if all || len <= cap + cap / 4 {
        len
    } else {
        cap
    }
}

/// The caps of §3.8 rule 4.
pub mod caps {
    pub const TABLES: usize = 12;
    pub const FIELDS: usize = 6;
    pub const PUBLISHERS: usize = 6;
    pub const CAPTIONS: usize = 8;
    pub const PAGE_OBJECTS: usize = 6;
    pub const TOP_CALLEES: usize = 5;
    pub const ENTRY_POINTS: usize = 12;
    pub const TEMP_NAMES: usize = 7;
    pub const WORKSPACE_GAPS: usize = 40;
}

/// The model joins the rollup reads: names of routines and objects, the
/// operation index (owner, anchor, original kind of an operation id), app
/// names and the primary app.
pub struct RollupCx<'m> {
    pub model: &'m Model,
    ops: OperationIndex<'m>,
    declaring: DeclaringRows,
    primary: Option<String>,
    objects: HashMap<&'m str, usize>,
    /// Stable routine id → its first model row.
    rows: HashMap<&'m str, usize>,
    app_names: HashMap<String, String>,
    /// Qualified display → the distinct stable ids that share it (T-W2-24:
    /// overloads, and objects of one name in two apps).
    shared: HashMap<String, Vec<&'m str>>,
}

/// Who owns an operation (design §3.5): the qualified owner routine, its
/// anchor, its app and whether that app is a dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpOwner {
    pub display: String,
    pub stable_id: String,
    pub anchor: Option<String>,
    pub app: String,
    pub dependency: bool,
    /// `found`, `ambiguous` or `missing`.
    pub outcome: &'static str,
}

impl<'m> RollupCx<'m> {
    /// Over `model`; `apps` are the loaded dependencies' rows
    /// (`LoadedDependencies::apps`: `appId`, `name`).
    #[must_use]
    pub fn new(model: &'m Model, apps: &[Value]) -> RollupCx<'m> {
        let mut app_names: HashMap<String, String> = apps
            .iter()
            .filter_map(|a| {
                Some((
                    a["appId"].as_str()?.to_ascii_lowercase(),
                    a["name"].as_str()?.to_string(),
                ))
            })
            .collect();
        if let Some(p) = &model.primary_app {
            app_names.insert(p.app_guid.to_ascii_lowercase(), p.name.clone());
        }
        let mut cx = RollupCx {
            model,
            // Every id kind: a no-path row's owner and an operation's original
            // kind are read by `lookup` / `claims` (design §3.5).
            ops: OperationIndex::build(model, IndexScope::All),
            declaring: DeclaringRows::new(&model.workspace.objects),
            primary: model
                .primary_app
                .as_ref()
                .map(|p| p.app_guid.to_ascii_lowercase()),
            objects: model
                .workspace
                .objects
                .iter()
                .enumerate()
                .map(|(i, o)| (o.id.as_str(), i))
                .collect(),
            rows: {
                let mut m = HashMap::new();
                for (i, r) in model.workspace.routines.iter().enumerate() {
                    m.entry(r.stable_routine_id.as_str()).or_insert(i);
                }
                m
            },
            app_names,
            shared: HashMap::new(),
        };
        let mut shared: HashMap<String, Vec<&'m str>> = HashMap::new();
        for (&stable, &i) in &cx.rows {
            shared.entry(cx.qualified_row(i)).or_default().push(stable);
        }
        shared.retain(|_, ids| {
            ids.sort_unstable();
            ids.len() > 1
        });
        cx.shared = shared;
        cx
    }

    /// What tells a routine apart from another of the same qualified
    /// display (design §3.8 rule 6, T-W2-24): its parameter list for an
    /// overload in the same app (`(Msg: Text; Level: Integer)`), ` [<app>]`
    /// when a colliding routine is in another app; empty when the display is
    /// its own.
    #[must_use]
    pub fn disambiguator(&self, stable_id: &str) -> String {
        let Some(&i) = self.rows.get(stable_id) else {
            return String::new();
        };
        let Some(ids) = self.shared.get(&self.qualified_row(i)) else {
            return String::new();
        };
        let r = &self.model.workspace.routines[i];
        let app_of = |s: &str| {
            self.rows
                .get(s)
                .map(|&j| self.model.workspace.routines[j].app_guid.as_str())
        };
        let mut out = String::new();
        if ids
            .iter()
            .any(|s| *s != stable_id && app_of(s) == Some(r.app_guid.as_str()))
        {
            out.push_str(&format!(
                "({})",
                r.parameters
                    .iter()
                    .map(|p| format!(
                        "{}{}: {}",
                        if p.is_var { "var " } else { "" },
                        p.name,
                        p.type_text
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if ids
            .iter()
            .any(|s| app_of(s).is_some_and(|a| a != r.app_guid))
        {
            out.push_str(&format!(" [{}]", self.app_label(&r.app_guid)));
        }
        out
    }

    /// The `(object, routine)` names of a stable routine id's first row.
    #[must_use]
    pub fn names_of(&self, stable_id: &str) -> Option<(String, String)> {
        let &i = self.rows.get(stable_id)?;
        let r = &self.model.workspace.routines[i];
        let object = self
            .declaring
            .of(&self.model.workspace.objects, r)
            .map_or("", |o| o.name.as_str());
        Some((object.to_string(), r.name.clone()))
    }

    /// The source unit of a stable routine id's first row.
    #[must_use]
    pub fn default_file(&self, stable_id: &str) -> Option<String> {
        let &i = self.rows.get(stable_id)?;
        let f = &self.model.workspace.routines[i]
            .source_anchor
            .source_unit_id;
        (!f.is_empty()).then(|| f.clone())
    }

    /// The qualified display of a stable routine id, else the id.
    #[must_use]
    pub fn qualified(&self, stable_id: &str) -> String {
        self.rows.get(stable_id).map_or_else(
            || stable_id.to_string(),
            |&i| format!("{}{}", self.qualified_row(i), self.disambiguator(stable_id)),
        )
    }

    /// A witness path as one line (design §3.8 rules 6, 7): the root, then
    /// each hop's `Object::Routine` (the object only when it changes), tagged
    /// `[run]`, `[event]`, `[trigger]` or `[interface]` by its edge.
    #[must_use]
    pub fn path_line(
        &self,
        root: &str,
        hops: &[crate::engine::l5::digest::ProjectedHop],
    ) -> String {
        let mut object = self.names_of(root).map(|(o, _)| o).unwrap_or_default();
        let mut parts = vec![
            self.names_of(root)
                .map_or_else(|| root.to_string(), |(_, n)| n),
        ];
        for h in hops {
            let h = &h.inner;
            let tag = edge_tag(h.edge_kind.as_deref().unwrap_or(h.kind));
            let fallback = h
                .to_display
                .clone()
                .or_else(|| h.callee_display.clone())
                .unwrap_or_default();
            let to = h.to_routine_id.as_deref().unwrap_or("");
            let name = match self.names_of(to) {
                Some((o, n)) if o != object => {
                    object = o.clone();
                    format!("{o}::{n}{}", self.disambiguator(to))
                }
                Some((_, n)) => format!("{n}{}", self.disambiguator(to)),
                None => fallback,
            };
            parts.push(format!("{tag}{name}"));
        }
        parts.join(" → ")
    }

    /// A group witness (W2-0b/W2-6) as one line (design §3.8 rules 6, 7):
    /// the root, then each step's routine (the object only when it
    /// changes), tagged by its edge, then `(<what> at <terminal anchor>)`.
    /// With `from`, the steps before it print as `…` (the crossing's prefix,
    /// printed once elsewhere).
    #[must_use]
    pub fn witness_line(
        &self,
        root: &str,
        w: &crate::engine::l5::group_path::GroupPath,
        from: Option<usize>,
        what: &str,
    ) -> String {
        let (mut object, root_name) = self
            .names_of(root)
            .unwrap_or_else(|| (String::new(), root.to_string()));
        let mut parts = vec![root_name];
        let start = from.unwrap_or(0).min(w.steps.len());
        if start > 0 {
            parts.push("…".to_string());
            object = w.steps[start - 1]
                .display
                .rsplit_once("::")
                .map(|(o, _)| o.split_once(' ').map_or(o, |(_, n)| n).to_string())
                .unwrap_or_default();
        }
        for s in &w.steps[start..] {
            let tag = edge_tag(&s.edge);
            let (o, n) = match self.names_of(&s.to) {
                Some(on) => on,
                None => match s.display.rsplit_once("::") {
                    Some((o, n)) => (
                        o.split_once(' ').map_or(o, |(_, n)| n).to_string(),
                        n.to_string(),
                    ),
                    None => (String::new(), s.display.clone()),
                },
            };
            let name = if o != object && !o.is_empty() {
                object = o.clone();
                format!("{o}::{n}{}", self.disambiguator(&s.to))
            } else {
                format!("{n}{}", self.disambiguator(&s.to))
            };
            parts.push(format!("{tag}{name}"));
        }
        let at = match (&w.terminal.file, w.terminal.line) {
            (Some(f), Some(l)) => format!(" at {}", self.location(f, l)),
            _ => String::new(),
        };
        let mut line = format!("{} ({what}{at})", parts.join(" → "));
        if !w.through_entry_point {
            line.push_str(&format!(
                "; this path does not pass the entry point: {}",
                no_path_reason(w.reason.unwrap_or("search-cap"))
            ));
        }
        line
    }

    /// A source location as the human outputs print it (design §3.8 rule 5):
    /// `<file>:<line>`, 1-based.
    #[must_use]
    pub fn location(&self, file: &str, line: u32) -> String {
        format!("{file}:{}", shown(line))
    }

    /// The kind a witness's terminal operation prints as: its original
    /// record-operation kind, else the effect type.
    #[must_use]
    pub fn terminal_kind<'s>(&self, op: &str, effect_type: &'s str) -> &'s str
    where
        'm: 's,
    {
        self.ops
            .claims(op)
            .first()
            .and_then(|c| self.ops.record_operation(c))
            .map_or(effect_type, |o| o.op.as_str())
    }

    /// The qualified `<Type> <Object>::<Routine>` of model row `i`.
    #[must_use]
    pub fn qualified_row(&self, i: usize) -> String {
        let r = &self.model.workspace.routines[i];
        let object = self
            .declaring
            .of(&self.model.workspace.objects, r)
            .map_or("", |o| o.name.as_str());
        crate::engine::l4::effect_query_cli::owner_qualified(
            &crate::engine::l4::effect_query_cli::object_display(&r.object_type, object),
            &r.name,
        )
    }

    /// Is a table id (`<app>/table/<n>`) owned by the primary app?
    #[must_use]
    pub fn table_is_own(&self, table_id: &str) -> bool {
        let app = table_id.split('/').next().unwrap_or("");
        self.primary
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(app))
    }

    /// An app's name, else its guid.
    #[must_use]
    pub fn app_label(&self, guid: &str) -> String {
        self.app_names
            .get(&guid.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| guid.to_string())
    }

    /// An object id's display (`Codeunit Foo`), else the id.
    #[must_use]
    pub fn object_display(&self, object_id: &str) -> String {
        self.objects.get(object_id).map_or_else(
            || object_id.to_string(),
            |&i| {
                let o = &self.model.workspace.objects[i];
                crate::engine::l4::effect_query_cli::object_display(&o.object_type, &o.name)
            },
        )
    }

    /// The original record-operation kind of `op` (`Insert`, `DeleteAll`,
    /// …), from the first claiming row (explain's pick).
    #[must_use]
    pub fn op_kind(&self, op: &str) -> Option<&'m str> {
        let c = self.ops.claims(op).first()?;
        self.ops.original_kind(c)
    }

    /// Who owns `op` (design §3.5).
    #[must_use]
    pub fn op_owner(&self, op: &str) -> Option<OpOwner> {
        let (claim, outcome) = match self.ops.lookup(op) {
            OwnerLookup::Found(c) => (c, "found"),
            OwnerLookup::Ambiguous(cs) => (*cs.first()?, "ambiguous"),
            OwnerLookup::Missing => return None,
        };
        let r = self.ops.routine(&claim);
        let anchor = self
            .ops
            .anchor(&claim)
            .map(|a| format!("{}:{}", a.source_unit_id, shown(a.start_line)));
        Some(OpOwner {
            display: self.qualified_row(claim.row as usize),
            stable_id: r.stable_routine_id.clone(),
            anchor,
            app: r.app_guid.clone(),
            dependency: self.model.role_of_app(&r.app_guid) == Role::Dependency,
            outcome,
        })
    }
}

/// A path tag by edge kind (design §3.8 rule 7).
#[must_use]
pub fn edge_tag(kind: &str) -> &'static str {
    if kind.starts_with("object-run") {
        "[run] "
    } else if kind == "event-dispatch" {
        "[event] "
    } else if kind == "implicit-trigger" {
        "[trigger] "
    } else if kind == "interface-dispatch" {
        "[interface] "
    } else {
        ""
    }
}

/// Why a witness is unavailable, in words (design §3.4.2: never invent a
/// path; say why there is none).
#[must_use]
pub fn no_path_reason(reason: &str) -> &'static str {
    match reason {
        "no-witness" => "the witness search found no live route through this entry point",
        "search-cap" => "search cap reached",
        "cancelled" => "the search was cancelled",
        "no-operation-id" => "no operation id on the fact",
        _ => "no witness",
    }
}

/// The digest's write types.
fn is_write(ty: &str) -> bool {
    matches!(ty, "DB_INSERT" | "DB_MODIFY" | "DB_DELETE")
}

fn detail<'e>(e: &'e DigestEffectFull, key: &str) -> Option<&'e str> {
    e.detail
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A group row's crossing key: its crossing's boundary identity, else (a
/// hand-built group with no record) its boundary display and site file.
#[must_use]
pub fn crossing_key(e: &DigestEffectFull) -> String {
    e.crossing.as_ref().map_or_else(
        || {
            format!(
                "{}@{}",
                detail(e, "boundary").unwrap_or(""),
                e.evidence.file.as_deref().unwrap_or("")
            )
        },
        |c| c.id.clone(),
    )
}

/// A row is a boundary group: it names the routine it enters.
#[must_use]
pub fn is_group(e: &DigestEffectFull) -> bool {
    e.crossing.is_some() || detail(e, "boundary").is_some()
}

/// The kind family of a non-DB effect type (design §4.1 rule 2e).
#[must_use]
pub fn family_of(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "UI_CONFIRM" | "UI_MESSAGE" | "UI_WINDOW_OPEN" | "UI_FILE_DIALOG" | "UI_HYPERLINK"
        | "UI_PROGRESS" => "ui",
        "HTTP" => "http",
        "FILE" => "file",
        "BACKGROUND_TASK" => "backgroundTasks",
        "ISOLATED_STORAGE" => "isolatedStorage",
        "TELEMETRY" => "telemetry",
        "ERROR_THROW" | "ERROR_IMPLICIT" => "errors",
        // Not a kind family: the DB types, COMMIT and events have their own
        // sections, and the routine's own `UI_ERROR` is a check (rules).
        t if t.starts_with("DB_") => return None,
        "COMMIT" | "EVENT_PUBLISH" | "UI_ERROR" => return None,
        // W2-5 (2e): any other effect type is never dropped silently; it is
        // its own explicit bucket, printed by its type name.
        _ => OTHER_KINDS,
    })
}

/// The explicit bucket of an effect type no family names (W2-5).
pub const OTHER_KINDS: &str = "otherKinds";

/// The kind families in summary order, with their human labels.
pub const FAMILIES: &[(&str, &str)] = &[
    ("ui", "UI"),
    ("http", "HTTP"),
    ("file", "file"),
    ("backgroundTasks", "background tasks"),
    ("isolatedStorage", "isolated storage"),
    ("telemetry", "telemetry"),
    ("errors", "errors"),
    (OTHER_KINDS, "other effect kinds"),
];

/// One table's writes in one section (one record per table: T-W2-2).
#[derive(Debug, Clone)]
pub struct TableWrites {
    pub table_id: String,
    pub table: String,
    /// The original operation kinds of its rows' operations (`Insert`,
    /// `DeleteAll`, …; the digest type when no op row is known).
    pub kinds: BTreeSet<String>,
    /// The digest write types of its rows.
    pub types: BTreeSet<String>,
    /// Each row's class, by its digest type.
    pub classes: Vec<(String, WriteClass)>,
    /// The rows (indices into the entry's effects).
    pub rows: Vec<usize>,
    /// The representative row: shortest witness, then earliest anchor.
    pub rep: usize,
}

/// One crossing's group rows (design §3.3): a presentation key, the boundary
/// display, its targets and sites, and what the refined groups list.
#[derive(Debug, Clone, Default)]
pub struct CrossingView {
    /// The crossing's boundary identity (`cs:…`, `op:…`, `ev:…`), else the
    /// boundary display and site.
    pub id: String,
    /// The boundary display (`Codeunit X::OnRun`; the multi-target form names
    /// every target).
    pub name: String,
    pub target_ids: Vec<String>,
    /// `file:line` of each site (1-based).
    pub sites: Vec<String>,
    /// May-persist tables the refined `nt` groups list, id → display.
    pub tables: BTreeMap<String, String>,
    /// Per write type, the tables of [`Self::tables`].
    pub by_type: BTreeMap<String, BTreeSet<String>>,
    /// Tables only `kt` groups list.
    pub temp_tables: BTreeMap<String, String>,
    /// Writes on a table not resolved (`unresolvedResources`).
    pub unresolved_writes: usize,
    pub events: BTreeSet<String>,
    /// Non-DB effect types the crossing carries.
    pub kinds: BTreeSet<String>,
    pub commit: bool,
    /// Literal-dead group rows.
    pub dead_rows: usize,
    /// The row whose shortest path is the crossing's prefix.
    pub prefix_row: Option<usize>,
    pub rows: Vec<usize>,
}

impl CrossingView {
    /// Carries a may-persist write.
    #[must_use]
    pub fn writes(&self) -> bool {
        !self.tables.is_empty() || self.unresolved_writes > 0
    }
}

/// An own write with no witness (design §4.1 section 13).
#[derive(Debug, Clone)]
pub struct NoPathWrite {
    pub row: usize,
    pub table_id: String,
    pub table: String,
    pub kind: String,
    pub owner: Option<OpOwner>,
    pub class: WriteClass,
}

/// The gaps by where the call site is (design §3.6).
#[derive(Debug, Clone, Default)]
pub struct GapCounts {
    /// Indices into the entry's `unresolved`, call site in workspace code.
    pub workspace: Vec<usize>,
    pub dependency: usize,
    pub dependency_polymorphic: usize,
    pub dependency_dynamic: usize,
    pub not_placed: Vec<usize>,
    /// Dependency gaps' callees, most frequent first.
    pub top_callees: Vec<(String, usize)>,
}

/// The one summary (design §3.1, F15).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RollupTotals {
    pub may_write_tables: usize,
    pub owned_by_app: usize,
    pub by_dependencies: usize,
    pub unresolved_table_writes: usize,
    pub caller_dependent_tables: usize,
    /// `yes`, `may`, `none` (complete coverage, no limit) or `no observed`.
    pub commits: &'static str,
    pub commit_sites: usize,
    pub commit_entry_points: usize,
    pub events: usize,
    pub events_in_workspace: usize,
    /// Per kind family ([`FAMILIES`]): places (own sites + crossings).
    pub kinds: BTreeMap<&'static str, usize>,
    pub gaps_workspace: usize,
    pub gaps_dependency: usize,
    pub gaps_not_placed: usize,
    pub temporary_only_tables: usize,
    pub literal_dead_tables: usize,
    pub no_path_operations: usize,
    pub no_path_tables: usize,
    pub entry_points: usize,
    pub crossing_sites: usize,
    /// Explain only: known routine callers, page actions, tests.
    pub callers: Option<usize>,
    pub page_actions: Option<usize>,
    pub tests: Option<usize>,
}

impl RollupTotals {
    /// The one serialization (T-W2-13).
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "mayWriteTables": {
                "count": self.may_write_tables,
                "ownedByApp": self.owned_by_app,
                "byDependencies": self.by_dependencies,
            },
            "unresolvedTableWrites": self.unresolved_table_writes,
            "callerDependentTables": self.caller_dependent_tables,
            "commits": {
                "answer": self.commits,
                "ownSites": self.commit_sites,
                "entryPoints": self.commit_entry_points,
            },
            "events": {"distinct": self.events, "inWorkspaceCode": self.events_in_workspace},
            "kinds": self.kinds,
            "gaps": {
                "workspace": self.gaps_workspace,
                "dependency": self.gaps_dependency,
                "notPlaced": self.gaps_not_placed,
            },
            "temporaryOnlyTables": self.temporary_only_tables,
            "literalDeadTables": self.literal_dead_tables,
            "noPathKept": {"operations": self.no_path_operations, "tables": self.no_path_tables},
            "entryPoints": {"count": self.entry_points, "sites": self.crossing_sites},
        });
        if let Some(c) = self.callers {
            v["callers"] = json!(c);
        }
        if let Some(a) = self.page_actions {
            v["pageActions"] = json!(a);
        }
        if let Some(t) = self.tests {
            v["tests"] = json!(t);
        }
        v
    }

    /// The human `summary:` line (design §3.8 rule 2).
    #[must_use]
    pub fn human(&self) -> String {
        let mut parts = Vec::new();
        let writes = if self.may_write_tables == 0 && self.unresolved_table_writes == 0 {
            "no observed write that may persist".to_string()
        } else {
            let mut w = format!(
                "may write {} ({} owned by this app, {} by dependencies)",
                plural(self.may_write_tables, "table", "tables"),
                self.owned_by_app,
                self.by_dependencies
            );
            if self.unresolved_table_writes > 0 {
                w.push_str(&format!(
                    " and {} on a table not resolved",
                    plural(self.unresolved_table_writes, "write", "writes")
                ));
            }
            w
        };
        parts.push(writes);
        parts.push(format!(
            "caller-dependent: {}",
            if self.caller_dependent_tables == 0 {
                "none".to_string()
            } else {
                plural(self.caller_dependent_tables, "table", "tables")
            }
        ));
        let commits = match self.commits {
            "no observed" => "commits: no observed COMMIT".to_string(),
            "none" => "commits: none".to_string(),
            answer => {
                let mut where_ = Vec::new();
                if self.commit_sites > 0 {
                    where_.push(plural(self.commit_sites, "site", "sites"));
                }
                if self.commit_entry_points > 0 {
                    where_.push(plural(
                        self.commit_entry_points,
                        "commit-bearing entry point",
                        "commit-bearing entry points",
                    ));
                }
                format!("commits: {answer} ({})", where_.join(", "))
            }
        };
        parts.push(commits);
        parts.push(format!(
            "events: {} ({} raised in workspace code)",
            thousands(self.events),
            thousands(self.events_in_workspace)
        ));
        for (key, label) in FAMILIES {
            if let Some(&n) = self.kinds.get(key).filter(|n| **n > 0) {
                parts.push(format!("{label} at {}", plural(n, "place", "places")));
            }
        }
        if let Some(c) = self.callers {
            let actions = self.page_actions.unwrap_or(0);
            parts.push(match (c, actions) {
                (0, 0) => "callers: none".to_string(),
                (0, a) => format!(
                    "callers: none, reached from {}",
                    plural(a, "page action", "page actions")
                ),
                (c, 0) => format!("callers: {c}"),
                (c, a) => format!(
                    "callers: {c}, reached from {}",
                    plural(a, "page action", "page actions")
                ),
            });
        }
        if let Some(t) = self.tests {
            parts.push(if t == 0 {
                "tests: none".to_string()
            } else {
                format!("tests: {t}")
            });
        }
        parts.push(format!(
            "gaps: {} in workspace code, {} in dependency code, {} not placed",
            self.gaps_workspace, self.gaps_dependency, self.gaps_not_placed
        ));
        format!("summary: {}", parts.join("; "))
    }
}

/// `n unit` with the plural unit when `n != 1`.
#[must_use]
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// `n` with a thousands comma from 1,000 (design §3.8 rule 2).
#[must_use]
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One root's rollup (module doc).
#[derive(Debug, Clone, Default)]
pub struct RoutineRollup {
    /// Every may-persist table (the union of workspace code, every crossing's
    /// non-temporary set and the no-path rows), id → display.
    pub may_write: BTreeMap<String, String>,
    pub workspace_code: Vec<TableWrites>,
    /// Own physical writes on a table not resolved (rows).
    pub unresolved_own: Vec<usize>,
    pub caller_dependent: Vec<TableWrites>,
    pub crossings: Vec<CrossingView>,
    pub no_path: Vec<NoPathWrite>,
    /// Tables with temporary own contributions, id → display.
    pub temporary_ws: BTreeMap<String, String>,
    /// Temporary own rows.
    pub temporary_rows: Vec<usize>,
    /// Tables only `kt` groups list beyond a crossing.
    pub temporary_beyond: BTreeMap<String, String>,
    /// Tables with temporary contributions and no physical or caller-dependent
    /// one (computed from the temporary rows, T-W2-19).
    pub temporary_only: BTreeMap<String, String>,
    /// Tables listed only by literal-dead rows or refined out, id → display.
    pub literal_dead: BTreeMap<String, String>,
    pub literal_dead_rows: Vec<usize>,
    /// Own COMMIT rows (live).
    pub commit_sites: Vec<usize>,
    pub commit_unconditional: bool,
    /// Own EVENT_PUBLISH rows' event ids (live), with the row.
    pub events_ws: BTreeMap<String, usize>,
    /// Event ids only groups raise.
    pub events_beyond: BTreeSet<String>,
    /// Per kind family: own rows.
    pub kind_sites: BTreeMap<&'static str, Vec<usize>>,
    /// Per kind family: crossing indices.
    pub kind_crossings: BTreeMap<&'static str, Vec<usize>>,
    pub gaps: GapCounts,
    pub totals: RollupTotals,
}

/// The length of a row's shortest listed path (`usize::MAX` without one).
fn shortest(e: &DigestEffectFull) -> usize {
    e.via_paths.iter().map(Vec::len).min().unwrap_or(usize::MAX)
}

/// An anchor's `(file, line)` sort key.
fn anchor_key(e: &DigestEffectFull) -> (String, u32) {
    (
        e.evidence.file.clone().unwrap_or_default(),
        e.evidence.line.unwrap_or(u32::MAX),
    )
}

impl RoutineRollup {
    /// The may-persist tables' names, owned by this app and by dependencies,
    /// each sorted.
    #[must_use]
    pub fn may_write_names(&self, cx: &RollupCx<'_>) -> (Vec<String>, Vec<String>) {
        let (mut own, mut dep) = (Vec::new(), Vec::new());
        for (id, d) in &self.may_write {
            if cx.table_is_own(id) {
                own.push(d.clone());
            } else {
                dep.push(d.clone());
            }
        }
        own.sort();
        dep.sort();
        (own, dep)
    }

    /// The rollup of `entry` over `cx` (module doc).
    #[must_use]
    pub fn build(entry: &DigestEntryFull, cx: &RollupCx<'_>) -> RoutineRollup {
        let mut r = RoutineRollup::default();
        let mut ws: BTreeMap<String, TableWrites> = BTreeMap::new();
        let mut caller: BTreeMap<String, TableWrites> = BTreeMap::new();
        let mut crossings: Vec<CrossingView> = Vec::new();
        let mut crossing_ix: HashMap<String, usize> = HashMap::new();
        let mut caller_own: BTreeSet<String> = BTreeSet::new();
        let mut live_tables: BTreeMap<String, String> = BTreeMap::new();
        let mut dead_tables: BTreeMap<String, String> = BTreeMap::new();

        let add = |map: &mut BTreeMap<String, TableWrites>,
                   i: usize,
                   e: &DigestEffectFull,
                   id: &str,
                   name: &str,
                   class: WriteClass| {
            let kinds: BTreeSet<String> = e
                .origin_operation_ids
                .iter()
                .filter_map(|op| cx.op_kind(op).map(str::to_string))
                .collect();
            let t = map.entry(id.to_string()).or_insert_with(|| TableWrites {
                table_id: id.to_string(),
                table: name.to_string(),
                kinds: BTreeSet::new(),
                types: BTreeSet::new(),
                classes: Vec::new(),
                rows: Vec::new(),
                rep: i,
            });
            if kinds.is_empty() {
                t.kinds.insert(type_kind(&e.effect_type).to_string());
            } else {
                t.kinds.extend(kinds);
            }
            t.types.insert(e.effect_type.clone());
            t.classes.push((e.effect_type.clone(), class));
            t.rows.push(i);
        };

        for (i, e) in entry.effects.iter().enumerate() {
            let ty = e.effect_type.as_str();
            if is_group(e) {
                let key = crossing_key(e);
                let ci = *crossing_ix.entry(key.clone()).or_insert_with(|| {
                    let (targets, sites) = match &e.crossing {
                        Some(c) => (
                            c.targets.clone(),
                            c.sites
                                .iter()
                                .filter_map(|s| {
                                    Some(format!("{}:{}", s.file.clone()?, shown(s.line?)))
                                })
                                .collect(),
                        ),
                        None => (
                            Vec::new(),
                            e.evidence
                                .file
                                .iter()
                                .map(|f| format!("{f}:{}", e.evidence.line.map_or(0, shown)))
                                .collect(),
                        ),
                    };
                    crossings.push(CrossingView {
                        id: key.clone(),
                        name: detail(e, "boundary").unwrap_or("").to_string(),
                        target_ids: targets,
                        sites,
                        ..CrossingView::default()
                    });
                    crossings.len() - 1
                });
                let c = &mut crossings[ci];
                c.rows.push(i);
                if c.prefix_row
                    .is_none_or(|p| shortest(e) < shortest(&entry.effects[p]))
                {
                    c.prefix_row = (!e.via_paths.is_empty()).then_some(i).or(c.prefix_row);
                }
                if e.literal_unreachable {
                    c.dead_rows += 1;
                    if is_write(ty) {
                        for (id, d) in &e.group_resources {
                            dead_tables.insert(id.clone(), d.clone());
                        }
                    }
                    r.literal_dead_rows.push(i);
                    continue;
                }
                match ty {
                    t if is_write(t) => {
                        let class = classify_detail(&e.detail);
                        if let Some(u) =
                            detail(e, "unresolvedResources").and_then(|v| v.parse::<usize>().ok())
                        {
                            c.unresolved_writes += u;
                        }
                        for (id, d) in &e.group_resources {
                            if class == WriteClass::Temporary {
                                c.temp_tables.insert(id.clone(), d.clone());
                            } else {
                                c.tables.insert(id.clone(), d.clone());
                                c.by_type
                                    .entry(t.to_string())
                                    .or_default()
                                    .insert(id.clone());
                                live_tables.insert(id.clone(), d.clone());
                            }
                        }
                    }
                    "COMMIT" => c.commit = true,
                    "EVENT_PUBLISH" => {
                        c.events
                            .extend(e.group_resources.iter().map(|(id, _)| id.clone()));
                    }
                    "DB_READ" => {}
                    other => {
                        c.kinds.insert(other.to_string());
                    }
                }
                continue;
            }
            // Own rows.
            if e.literal_unreachable {
                if is_write(ty)
                    && let Some(id) = detail(e, "resourceId")
                {
                    dead_tables.insert(
                        id.to_string(),
                        detail(e, "resourceDisplay").unwrap_or(id).to_string(),
                    );
                }
                r.literal_dead_rows.push(i);
                continue;
            }
            match ty {
                t if is_write(t) => {
                    let class = classify_detail(&e.detail);
                    let id = detail(e, "resourceId");
                    let name = detail(e, "resourceDisplay").or(id).unwrap_or("");
                    // 2b: a temporary contribution is in the temporary section
                    // only, with or without a witness.
                    if class == WriteClass::Temporary {
                        if let Some(id) = id {
                            r.temporary_ws.insert(id.to_string(), name.to_string());
                        }
                        r.temporary_rows.push(i);
                        continue;
                    }
                    if e.evidence.source_kind == "unavailable"
                        && matches!(class, WriteClass::Physical(_))
                    {
                        let kind = e
                            .origin_operation_ids
                            .iter()
                            .find_map(|op| cx.op_kind(op))
                            .unwrap_or(type_kind(t))
                            .to_string();
                        let owner = e.origin_operation_ids.iter().find_map(|op| cx.op_owner(op));
                        if let Some(id) = id {
                            live_tables.insert(id.to_string(), name.to_string());
                        }
                        r.no_path.push(NoPathWrite {
                            row: i,
                            table_id: id.unwrap_or("").to_string(),
                            table: name.to_string(),
                            kind,
                            owner,
                            class,
                        });
                        continue;
                    }
                    match (class, id) {
                        (WriteClass::Temporary, _) => unreachable!("handled above"),
                        (WriteClass::CallerDependent(_), Some(id)) => {
                            caller_own.insert(id.to_string());
                            add(&mut caller, i, e, id, name, class);
                        }
                        (_, None) => r.unresolved_own.push(i),
                        (WriteClass::Physical(_), Some(id)) => {
                            live_tables.insert(id.to_string(), name.to_string());
                            add(&mut ws, i, e, id, name, class);
                        }
                    }
                }
                "COMMIT" => {
                    r.commit_unconditional |=
                        e.conditionality == crate::engine::l5::conditionality::UNCONDITIONAL;
                    r.commit_sites.push(i);
                }
                "EVENT_PUBLISH" => {
                    if let Some(id) = detail(e, "resourceId") {
                        r.events_ws.entry(id.to_string()).or_insert(i);
                    }
                }
                "DB_READ" => {}
                other => {
                    if let Some(f) = family_of(other) {
                        r.kind_sites.entry(f).or_default().push(i);
                    }
                }
            }
        }

        // Representatives: shortest witness, then earliest anchor (§3.4.2).
        let pick = |t: &mut TableWrites| {
            t.rep = *t
                .rows
                .iter()
                .min_by_key(|&&i| (shortest(&entry.effects[i]), anchor_key(&entry.effects[i])))
                .expect("a table record has a row");
        };
        r.workspace_code = ws.into_values().collect();
        r.caller_dependent = caller.into_values().collect();
        r.workspace_code.iter_mut().for_each(pick);
        r.caller_dependent.iter_mut().for_each(pick);
        // Order: by table name.
        r.workspace_code
            .sort_by(|a, b| a.table.cmp(&b.table).then(a.table_id.cmp(&b.table_id)));
        r.caller_dependent
            .sort_by(|a, b| a.table.cmp(&b.table).then(a.table_id.cmp(&b.table_id)));

        // Beyond sets.
        for c in &crossings {
            for (id, d) in &c.temp_tables {
                r.temporary_beyond.insert(id.clone(), d.clone());
            }
            for ev in &c.events {
                if !r.events_ws.contains_key(ev) {
                    r.events_beyond.insert(ev.clone());
                }
            }
        }
        // Order: crossings with may-persist writes first, then by table count,
        // then by name.
        crossings.sort_by(|a, b| {
            b.writes()
                .cmp(&a.writes())
                .then(b.tables.len().cmp(&a.tables.len()))
                .then(a.name.cmp(&b.name))
                .then(a.id.cmp(&b.id))
        });
        // The kind indices point into the sorted list.
        r.kind_crossings.clear();
        for (ci, c) in crossings.iter().enumerate() {
            for k in &c.kinds {
                let f = family_of(k).or((k == "UI_ERROR").then_some("errors"));
                if let Some(f) = f {
                    let v = r.kind_crossings.entry(f).or_default();
                    if v.last() != Some(&ci) {
                        v.push(ci);
                    }
                }
            }
        }
        r.crossings = crossings;

        // Temporary-only: temporary contributions, no physical or
        // caller-dependent one anywhere (T-W2-19).
        for (id, d) in r.temporary_ws.iter().chain(r.temporary_beyond.iter()) {
            if !live_tables.contains_key(id) && !caller_own.contains(id) {
                r.temporary_only.insert(id.clone(), d.clone());
            }
        }
        // Literal-dead: tables only dead rows name.
        r.literal_dead = dead_tables
            .into_iter()
            .filter(|(id, _)| !live_tables.contains_key(id))
            .collect();

        r.gaps = gaps_of(entry);
        r.may_write = live_tables;
        let complete = entry.coverage_status == "complete"
            && r.no_path.is_empty()
            && !entry.unresolved_traversal.truncated;
        r.totals = r.totals_of(cx, &caller_own, complete);
        r
    }

    fn totals_of(
        &self,
        cx: &RollupCx<'_>,
        caller_own: &BTreeSet<String>,
        complete: bool,
    ) -> RollupTotals {
        let live = &self.may_write;
        let owned = live.keys().filter(|id| cx.table_is_own(id)).count();
        let commit_entry_points = self.crossings.iter().filter(|c| c.commit).count();
        // §3.7: "none" only when coverage is complete and nothing limits it.
        let commits = if self.commit_unconditional {
            "yes"
        } else if self.commit_sites.is_empty() && commit_entry_points == 0 {
            if complete { "none" } else { "no observed" }
        } else {
            "may"
        };
        let mut kinds = BTreeMap::new();
        for (key, _) in FAMILIES {
            let n = self.kind_sites.get(key).map_or(0, Vec::len)
                + self.kind_crossings.get(key).map_or(0, Vec::len);
            if n > 0 {
                kinds.insert(*key, n);
            }
        }
        let no_path_tables: BTreeSet<&str> = self
            .no_path
            .iter()
            .map(|n| n.table_id.as_str())
            .filter(|t| !t.is_empty())
            .collect();
        let writing: Vec<&CrossingView> = self.crossings.iter().filter(|c| c.writes()).collect();
        RollupTotals {
            may_write_tables: live.len(),
            owned_by_app: owned,
            by_dependencies: live.len() - owned,
            unresolved_table_writes: self.unresolved_own.len()
                + self
                    .no_path
                    .iter()
                    .filter(|n| n.table_id.is_empty())
                    .count()
                + self
                    .crossings
                    .iter()
                    .map(|c| c.unresolved_writes)
                    .sum::<usize>(),
            caller_dependent_tables: caller_own.len(),
            commits,
            commit_sites: self.commit_sites.len(),
            commit_entry_points,
            events: self.events_ws.len() + self.events_beyond.len(),
            events_in_workspace: self.events_ws.len(),
            kinds,
            gaps_workspace: self.gaps.workspace.len(),
            gaps_dependency: self.gaps.dependency,
            gaps_not_placed: self.gaps.not_placed.len(),
            temporary_only_tables: self.temporary_only.len(),
            literal_dead_tables: self.literal_dead.len(),
            no_path_operations: self.no_path.len(),
            no_path_tables: no_path_tables.len(),
            entry_points: writing.len(),
            crossing_sites: writing.iter().map(|c| c.sites.len().max(1)).sum(),
            callers: None,
            page_actions: None,
            tests: None,
        }
    }
}

/// The digest type's spelling when no op row is known (§3.5).
#[must_use]
pub fn type_kind(ty: &str) -> &'static str {
    match ty {
        "DB_INSERT" => "Insert",
        "DB_MODIFY" => "Modify",
        "DB_DELETE" => "Delete",
        _ => "Write",
    }
}

/// The class note of a table line (design §4.1 section 10).
#[must_use]
pub fn class_note(classes: &[(String, WriteClass)]) -> String {
    let reasons: BTreeSet<&'static str> = classes
        .iter()
        .map(|(_, c)| match c {
            WriteClass::Physical(PhysicalReason::KnownPersistent) => "known persistent",
            WriteClass::Physical(PhysicalReason::TempStateUnknown) => "temp state unknown",
            WriteClass::Physical(PhysicalReason::NotJoined) => "temp state not joined",
            WriteClass::CallerDependent(CallerReason::ImplicitRec) => "depends on the caller's Rec",
            WriteClass::CallerDependent(CallerReason::Parameter(_)) => {
                "depends on the caller's record argument"
            }
            WriteClass::Temporary => "temporary",
        })
        .collect();
    reasons.into_iter().collect::<Vec<_>>().join(", ")
}

/// The JSON class name and reason of a write class (design §4.2).
#[must_use]
pub fn class_json(c: WriteClass) -> (&'static str, &'static str) {
    match c {
        WriteClass::Physical(PhysicalReason::KnownPersistent) => ("physical", "known-persistent"),
        WriteClass::Physical(PhysicalReason::TempStateUnknown) => {
            ("physical", "temp-state-unknown")
        }
        WriteClass::Physical(PhysicalReason::NotJoined) => ("physical", "not-joined"),
        WriteClass::CallerDependent(CallerReason::ImplicitRec) => ("callerDependent", "rec"),
        WriteClass::CallerDependent(CallerReason::Parameter(_)) => ("callerDependent", "parameter"),
        WriteClass::Temporary => ("temporary", "known-temporary"),
    }
}

/// The gaps of `entry` by where the call site is.
fn gaps_of(entry: &DigestEntryFull) -> GapCounts {
    let mut g = GapCounts::default();
    let mut callees: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, u) in entry.unresolved.iter().enumerate() {
        match u.callsite_file.as_deref() {
            Some(f) if f.starts_with("ws:") => g.workspace.push(i),
            Some(_) => {
                g.dependency += 1;
                match u.status.as_str() {
                    "polymorphic" => g.dependency_polymorphic += 1,
                    "dynamic-target" => g.dependency_dynamic += 1,
                    _ => {}
                }
                *callees.entry(u.callee_display.as_str()).or_default() += 1;
            }
            None => g.not_placed.push(i),
        }
    }
    let mut top: Vec<(String, usize)> = callees
        .into_iter()
        .map(|(c, n)| (c.to_string(), n))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    g.top_callees = top;
    g
}

// ---------------------------------------------------------------------------
// The human sections explain and digest share (design §4.1, §4.3): one
// function per section, so the two outputs print the same lines.
// ---------------------------------------------------------------------------

/// The most paths a row keeps (the producer keeps up to 3 per effect).
pub const PATH_CAP_MAX: usize = 3;

/// How much a rendering shows.
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    /// `--detail`: no fold, every name.
    pub detail: bool,
    /// Paths per item (design §5: `--max-paths N`, else 3 with `--detail`,
    /// else 1; at most 3).
    pub path_cap: usize,
}

impl RenderOpts {
    /// The effective path cap of design §5.
    #[must_use]
    pub fn new(detail: bool, max_paths: Option<usize>) -> RenderOpts {
        RenderOpts {
            detail,
            path_cap: max_paths
                .unwrap_or(if detail { 3 } else { 1 })
                .min(PATH_CAP_MAX),
        }
    }
}

/// `file:line` of a row's evidence (1-based), or why there is none.
fn row_anchor(e: &DigestEffectFull) -> String {
    match (&e.evidence.file, e.evidence.line) {
        (Some(f), Some(l)) => format!("{f}:{}", shown(l)),
        (Some(f), None) => f.clone(),
        _ => "no position kept".to_string(),
    }
}

/// The `path:` lines of a row: its shortest listed paths, up to `cap`.
fn path_lines(cx: &RollupCx<'_>, root: &str, e: &DigestEffectFull, cap: usize) -> Vec<String> {
    let mut paths: Vec<&Vec<crate::engine::l5::digest::ProjectedHop>> =
        e.via_paths.iter().collect();
    paths.sort_by_key(|p| p.len());
    paths
        .into_iter()
        .filter(|p| !p.is_empty())
        .take(cap)
        .map(|p| format!("path: {}", cx.path_line(root, p)))
        .collect()
}

/// A crossing's display: its boundary name, else its id.
fn crossing_name(c: &CrossingView) -> &str {
    if c.name.is_empty() { &c.id } else { &c.name }
}

/// The coverage, assumptions, limits and scope lines (design §3.7, §3.8
/// rule 3). `extra_limits`: the caller's own (explain's gaps).
#[must_use]
pub fn header_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    extra_limits: &[String],
) -> Vec<String> {
    let mut out = vec![r.totals.human()];
    let (mut open, mut dynamic) = (0usize, 0usize);
    for u in &entry.unresolved {
        match u.status.as_str() {
            "polymorphic" => open += 1,
            "dynamic-target" => dynamic += 1,
            _ => {}
        }
    }
    let mut cov = format!("coverage: {}", entry.coverage_status);
    if !entry.coverage_reasons.is_empty() {
        cov.push_str(&format!(" ({})", entry.coverage_reasons.join(", ")));
    }
    let mut counts = Vec::new();
    if open > 0 {
        counts.push(format!(
            "{} open world",
            plural(open, "interface call", "interface calls")
        ));
    }
    if dynamic > 0 {
        counts.push(format!(
            "{} not resolved",
            plural(dynamic, "dynamic call", "dynamic calls")
        ));
    }
    if !counts.is_empty() {
        cov.push_str(&format!(" — {}", counts.join(", ")));
    }
    out.push(cov);
    out.push(
        "assumptions: loaded program; unloaded implementations/subscribers are not enumerated"
            .to_string(),
    );
    let mut limits: Vec<String> = Vec::new();
    if !r.no_path.is_empty() {
        limits.push(format!(
            "no-path-kept {} on {}",
            plural(r.totals.no_path_operations, "operation", "operations"),
            plural(r.totals.no_path_tables, "table", "tables")
        ));
    }
    if entry.unresolved_traversal.truncated {
        limits.push(format!(
            "cone-truncated at depth {}",
            entry.unresolved_traversal.max_depth
        ));
    }
    let capped = entry
        .effects
        .iter()
        .flat_map(|e| e.group_resource_paths.iter())
        .filter(|(_, p)| matches!(p, Err(n) if n.reason == "search-cap"))
        .count();
    if capped > 0 {
        limits.push(format!(
            "path-search-capped {}",
            plural(capped, "resource", "resources")
        ));
    }
    limits.extend(extra_limits.iter().cloned());
    out.push(if limits.is_empty() {
        "limits: none".to_string()
    } else {
        format!("limits: {}", limits.join("; "))
    });
    out.push(format!(
        "scope: dependency code followed through every resolved call, with literal arguments, \
         grouped per entry point ({} at {}); temporary records are not writes; no lock facts \
         (alsem query effects lists locks); counts are distinct tables unless a line says \
         operations; lines are 1-based; --detail lists everything",
        plural(r.totals.entry_points, "entry point", "entry points"),
        plural(r.totals.crossing_sites, "site", "sites")
    ));
    out
}

/// The typed limits of an entry, as JSON (§3.7; the `limits:` line's data).
#[must_use]
pub fn limit_values(r: &RoutineRollup, entry: &DigestEntryFull) -> Vec<Value> {
    let mut out = Vec::new();
    if !r.no_path.is_empty() {
        out.push(json!({
            "kind": "no-path-kept",
            "operations": r.totals.no_path_operations,
            "tables": r.totals.no_path_tables,
        }));
    }
    if entry.unresolved_traversal.truncated {
        out.push(
            json!({"kind": "cone-truncated", "maxDepth": entry.unresolved_traversal.max_depth}),
        );
    }
    let capped = entry
        .effects
        .iter()
        .flat_map(|e| e.group_resource_paths.iter())
        .filter(|(_, p)| matches!(p, Err(n) if n.reason == "search-cap"))
        .count();
    if capped > 0 {
        out.push(json!({"kind": "path-search-capped", "resources": capped}));
    }
    out
}

/// An absence is "none" only with complete coverage and no limit (§3.7).
fn complete(entry: &DigestEntryFull, r: &RoutineRollup) -> bool {
    entry.coverage_status == "complete"
        && r.no_path.is_empty()
        && !entry.unresolved_traversal.truncated
}

/// `commits:` and its sites (design §4.1 section 9).
#[must_use]
pub fn commit_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    cx: &RollupCx<'_>,
    o: RenderOpts,
) -> Vec<String> {
    let bearing: Vec<&CrossingView> = r.crossings.iter().filter(|c| c.commit).collect();
    if r.commit_sites.is_empty() && bearing.is_empty() {
        return vec![if complete(entry, r) {
            "commits: none".to_string()
        } else {
            "commits: no observed COMMIT".to_string()
        }];
    }
    let mut out = vec![format!(
        "commits: {} — {} in workspace code; {}:",
        r.totals.commits,
        if r.commit_sites.is_empty() {
            "none".to_string()
        } else {
            plural(r.commit_sites.len(), "site", "sites")
        },
        plural(
            bearing.len(),
            "commit-bearing entry point",
            "commit-bearing entry points"
        )
    )];
    for &i in &r.commit_sites {
        let e = &entry.effects[i];
        out.push(format!(
            "  COMMIT at {} ({})",
            row_anchor(e),
            e.conditionality
        ));
        for p in path_lines(cx, &entry.routine_id, e, o.path_cap) {
            out.push(format!("    {p}"));
        }
    }
    for c in bearing {
        out.push(format!(
            "  {}, entered at {}",
            crossing_name(c),
            c.sites.join("; ")
        ));
        // W2-6 (2f): the COMMIT's own terminal witness through the crossing.
        if o.path_cap > 0 {
            out.extend(
                terminal_witness_lines(cx, entry, c, "COMMIT")
                    .into_iter()
                    .map(|l| format!("    {l}")),
            );
        }
    }
    out
}

/// The `path:` line of a crossing's resource-less group of type `ty`
/// (W2-6): its terminal witness, or why it has none.
fn terminal_witness_lines(
    cx: &RollupCx<'_>,
    entry: &DigestEntryFull,
    c: &CrossingView,
    ty: &str,
) -> Vec<String> {
    let Some(e) = c
        .rows
        .iter()
        .map(|&i| &entry.effects[i])
        .find(|e| e.effect_type == ty && !e.literal_unreachable)
    else {
        return Vec::new();
    };
    match &e.group_terminal_path {
        Some(Ok(w)) => vec![format!(
            "path: {}",
            cx.witness_line(
                &entry.routine_id,
                w,
                None,
                cx.terminal_kind(&w.terminal.operation_id, ty)
            )
        )],
        Some(Err(n)) => vec![format!("no path kept: {}", no_path_reason(n.reason))],
        None => vec!["no path kept: no witness was searched".to_string()],
    }
}

/// The index of a witness's crossing step: the first step entering one of
/// the crossing's targets.
fn crossing_step(
    w: &crate::engine::l5::group_path::GroupPath,
    targets: &[String],
) -> Option<usize> {
    w.steps.iter().position(|s| targets.contains(&s.to))
}

/// `writes in workspace code` (design §4.1 section 10). `fields(t)`: the
/// fields line of a table, when the caller knows one (explain).
#[must_use]
pub fn workspace_write_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    cx: &RollupCx<'_>,
    o: RenderOpts,
    fields: &dyn Fn(&TableWrites) -> Option<String>,
) -> Vec<String> {
    if r.workspace_code.is_empty() && r.unresolved_own.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "writes in workspace code ({}{}; operation counts are the cone's retained \
         representative operations, at least that many):",
        plural(r.workspace_code.len(), "table", "tables"),
        if r.unresolved_own.is_empty() {
            ""
        } else {
            " and writes on a table not resolved"
        }
    )];
    let folded = fold(
        r.workspace_code.iter().collect(),
        caps::TABLES,
        "tables",
        o.detail,
    );
    for t in &folded.shown {
        let rep = &entry.effects[t.rep];
        out.push(format!(
            "  {}: {} — {}",
            t.table,
            representative_counts(t, entry, cx),
            class_note(&t.classes)
        ));
        // Path and anchor of one witness (F13): the representative row's.
        let rep_kind = rep
            .origin_operation_ids
            .iter()
            .find_map(|op| cx.op_kind(op))
            .unwrap_or(type_kind(&rep.effect_type));
        out.push(format!("    {rep_kind} at {}", row_anchor(rep)));
        for p in path_lines(cx, &entry.routine_id, rep, o.path_cap) {
            out.push(format!("    {p}"));
        }
        if let Some(f) = fields(t) {
            out.push(format!("    {f}"));
        }
    }
    if folded.hidden > 0 {
        let names: Vec<String> = r.workspace_code[folded.shown.len()..]
            .iter()
            .map(|t| t.table.clone())
            .collect();
        out.push(format!(
            "  … +{} in the same form ({}) — --detail lists them",
            plural(folded.hidden, "table", "tables"),
            names.join(", ")
        ));
    }
    if !r.unresolved_own.is_empty() {
        let kinds: BTreeSet<String> = r
            .unresolved_own
            .iter()
            .flat_map(|&i| {
                let e = &entry.effects[i];
                let k: Vec<String> = e
                    .origin_operation_ids
                    .iter()
                    .filter_map(|op| cx.op_kind(op).map(str::to_string))
                    .collect();
                if k.is_empty() {
                    vec![type_kind(&e.effect_type).to_string()]
                } else {
                    k
                }
            })
            .collect();
        let at: BTreeSet<String> = r
            .unresolved_own
            .iter()
            .map(|&i| row_anchor(&entry.effects[i]))
            .collect();
        out.push(format!(
            "  table not resolved (determined at run time or not resolved): {} — at {}",
            kinds.into_iter().collect::<Vec<_>>().join(", "),
            at.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out
}

/// A table record's operation kinds with counts (W2-2 gap (c), design §3.6):
/// the distinct operation ids the cone RETAINED on its rows, per original
/// kind (`Insert 2, ModifyAll 1`), labelled by the section header as at
/// least that many. Not the exact count of §3.6: the cone keeps
/// representatives per row (`origin_operation_ids`), and the exact count
/// needs the terminal-fact index's per-root reach in the root's class, which
/// the passed entry does not keep. A row with no operation id counts its
/// digest type once.
fn representative_counts(t: &TableWrites, entry: &DigestEntryFull, cx: &RollupCx<'_>) -> String {
    let mut ops: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    let mut bare: BTreeMap<String, usize> = BTreeMap::new();
    for &i in &t.rows {
        let e = &entry.effects[i];
        if e.origin_operation_ids.is_empty() {
            *bare
                .entry(type_kind(&e.effect_type).to_string())
                .or_default() += 1;
        }
        for op in &e.origin_operation_ids {
            let kind = cx.op_kind(op).unwrap_or(type_kind(&e.effect_type));
            ops.entry(kind.to_string()).or_default().insert(op.as_str());
        }
    }
    let mut counts: BTreeMap<String, usize> = ops.into_iter().map(|(k, s)| (k, s.len())).collect();
    for (k, n) in bare {
        *counts.entry(k).or_default() += n;
    }
    counts
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `caller-dependent writes` (design §4.1 section 11).
#[must_use]
pub fn caller_dependent_lines(r: &RoutineRollup, entry: &DigestEntryFull) -> Vec<String> {
    if r.caller_dependent.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "caller-dependent writes ({}): written only if the caller passes a record that is not \
         temporary; alsem prove names the caller",
        plural(r.caller_dependent.len(), "table", "tables")
    )];
    for t in &r.caller_dependent {
        out.push(format!(
            "  {}: {} — {} at {}",
            t.table,
            t.kinds.iter().cloned().collect::<Vec<_>>().join(", "),
            class_note(&t.classes),
            row_anchor(&entry.effects[t.rep])
        ));
    }
    out
}

/// One entry point's lines: crossings to one target are merged; a
/// multi-target crossing stands alone (design §3.3).
struct EntryPoint<'r> {
    name: &'r str,
    sites: Vec<&'r str>,
    tables: BTreeMap<&'r str, &'r str>,
    by_type: BTreeMap<&'r str, BTreeSet<&'r str>>,
    unresolved: usize,
    kinds: BTreeSet<&'r str>,
    events: BTreeSet<&'r str>,
    commit: bool,
    prefix: Option<usize>,
    temp_only: bool,
    /// Its crossings (indices into `RoutineRollup::crossings`).
    crossings: Vec<usize>,
}

/// One may-persist table's witness beyond an entry point (W2-6).
struct TableWitness<'e> {
    table: &'e str,
    /// The terminal's digest write type.
    ty: &'e str,
    crossing: usize,
    witness: Result<&'e crate::engine::l5::group_path::GroupPath, &'static str>,
}

/// Every may-persist table's witness beyond `p`, nearest first (shortest
/// witness, then table name); a table with no witness last, by name.
fn table_witnesses<'e>(
    r: &'e RoutineRollup,
    entry: &'e DigestEntryFull,
    p: &EntryPoint<'_>,
) -> Vec<TableWitness<'e>> {
    let mut out: Vec<TableWitness<'e>> = Vec::new();
    for &ci in &p.crossings {
        let c = &r.crossings[ci];
        for &i in &c.rows {
            let e = &entry.effects[i];
            if e.literal_unreachable
                || !is_write(&e.effect_type)
                || classify_detail(&e.detail) == WriteClass::Temporary
            {
                continue;
            }
            for (id, w) in &e.group_resource_paths {
                let Some(table) = c.tables.get(id) else {
                    continue;
                };
                let witness = w.as_ref().map_err(|n| n.reason);
                // One witness per table: the shortest over its rows.
                if let Some(prev) = out.iter_mut().find(|t| t.table == table.as_str()) {
                    let shorter = match (&witness, &prev.witness) {
                        (Ok(a), Ok(b)) => a.steps.len() < b.steps.len(),
                        (Ok(_), Err(_)) => true,
                        _ => false,
                    };
                    if shorter {
                        *prev = TableWitness {
                            table,
                            ty: &e.effect_type,
                            crossing: ci,
                            witness,
                        };
                    }
                    continue;
                }
                out.push(TableWitness {
                    table,
                    ty: &e.effect_type,
                    crossing: ci,
                    witness,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        let len = |t: &TableWitness<'_>| t.witness.as_ref().map_or(usize::MAX, |w| w.steps.len());
        len(a).cmp(&len(b)).then(a.table.cmp(b.table))
    });
    out
}

fn entry_points(r: &RoutineRollup) -> Vec<EntryPoint<'_>> {
    let mut by_key: BTreeMap<&str, usize> = BTreeMap::new();
    let mut out: Vec<EntryPoint<'_>> = Vec::new();
    for (ci, c) in r.crossings.iter().enumerate() {
        let key = match c.target_ids.as_slice() {
            [one] => one.as_str(),
            _ => c.id.as_str(),
        };
        let i = *by_key.entry(key).or_insert_with(|| {
            out.push(EntryPoint {
                name: crossing_name(c),
                sites: Vec::new(),
                tables: BTreeMap::new(),
                by_type: BTreeMap::new(),
                unresolved: 0,
                kinds: BTreeSet::new(),
                events: BTreeSet::new(),
                commit: false,
                prefix: None,
                temp_only: true,
                crossings: Vec::new(),
            });
            out.len() - 1
        });
        let p = &mut out[i];
        p.crossings.push(ci);
        p.sites.extend(c.sites.iter().map(String::as_str));
        p.tables
            .extend(c.tables.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for (t, ids) in &c.by_type {
            p.by_type
                .entry(t.as_str())
                .or_default()
                .extend(ids.iter().map(String::as_str));
        }
        p.unresolved += c.unresolved_writes;
        p.kinds.extend(c.kinds.iter().map(String::as_str));
        p.events.extend(c.events.iter().map(String::as_str));
        p.commit |= c.commit;
        p.prefix = p.prefix.or(c.prefix_row);
        p.temp_only &= !c.writes();
    }
    for p in &mut out {
        p.sites.sort_unstable();
        p.sites.dedup();
    }
    out.sort_by(|a, b| {
        a.temp_only
            .cmp(&b.temp_only)
            .then(b.tables.len().cmp(&a.tables.len()))
            .then(a.name.cmp(b.name))
    });
    out
}

/// `writes beyond dependency entry points` (design §4.1 section 12).
#[must_use]
pub fn beyond_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    cx: &RollupCx<'_>,
    o: RenderOpts,
) -> Vec<String> {
    let points = entry_points(r);
    if points.is_empty() {
        return Vec::new();
    }
    let writing: Vec<&EntryPoint<'_>> = points.iter().filter(|p| !p.temp_only).collect();
    let others: Vec<&EntryPoint<'_>> = points.iter().filter(|p| p.temp_only).collect();
    let tables: BTreeSet<&str> = writing
        .iter()
        .flat_map(|p| p.tables.keys().copied())
        .collect();
    let sites: usize = writing.iter().map(|p| p.sites.len().max(1)).sum();
    let mut out = vec![format!(
        "writes beyond dependency entry points ({} through {} at {}):",
        plural(tables.len(), "table", "tables"),
        plural(writing.len(), "entry point", "entry points"),
        plural(sites, "site", "sites")
    )];
    let line = |p: &EntryPoint<'_>| -> String {
        let kinds: Vec<String> = p
            .by_type
            .iter()
            .map(|(t, ids)| format!("{} on {}", type_kind(t), ids.len()))
            .collect();
        // W2-2 gap (d), design §4.1: nearest first — by the length of each
        // table's witness through the entry point (prefix + call distance
        // from the target to the nearest live writer), then by name; a table
        // with no witness last, by name.
        let mut names: Vec<String> = table_witnesses(r, entry, p)
            .iter()
            .map(|t| t.table.to_string())
            .collect();
        let mut rest: Vec<String> = p
            .tables
            .values()
            .filter(|s| !names.iter().any(|n| n == *s))
            .map(|s| (*s).to_string())
            .collect();
        rest.sort();
        names.extend(rest);
        let names = fold(names, caps::TABLES, "tables", o.detail);
        let mut s = format!(
            "  {}, entered at {} — {}",
            p.name,
            p.sites.join("; "),
            plural(p.tables.len(), "table", "tables")
        );
        if !kinds.is_empty() {
            s.push_str(&format!(" ({})", kinds.join(", ")));
        }
        if !names.shown.is_empty() {
            s.push_str(&format!(": {}", names.joined()));
        }
        if p.unresolved > 0 {
            s.push_str(&format!(
                "; {} on a table not resolved",
                plural(p.unresolved, "write", "writes")
            ));
        }
        let mut also: Vec<&str> = Vec::new();
        if p.commit {
            also.push("COMMIT");
        }
        also.extend(p.kinds.iter().copied());
        if !also.is_empty() {
            s.push_str(&format!("; also {}", also.join(", ")));
        }
        if !p.events.is_empty() {
            s.push_str(&format!("; {}", plural(p.events.len(), "event", "events")));
        }
        s
    };
    let shown = fold(
        writing.clone(),
        caps::ENTRY_POINTS,
        "entry points",
        o.detail,
    );
    for p in &shown.shown {
        out.push(line(p));
        if o.path_cap == 0 {
            continue;
        }
        if let Some(i) = p.prefix {
            for l in path_lines(cx, &entry.routine_id, &entry.effects[i], 1) {
                out.push(format!("    prefix {l}"));
            }
        }
        // W2-6 (2f, design §3.4.2, §9 q2): one representative terminal
        // witness, labelled as such, to the nearest table; every table's
        // witness with --detail. With several sites, the representative
        // names the site it passes: a site's tables are its own.
        let witnesses = table_witnesses(r, entry, p);
        let render = |t: &TableWitness<'_>| -> String {
            let c = &r.crossings[t.crossing];
            match t.witness {
                Ok(w) => {
                    let from = crossing_step(w, &c.target_ids);
                    let what = cx.terminal_kind(&w.terminal.operation_id, type_kind(t.ty));
                    let mut s = cx.witness_line(&entry.routine_id, w, from, what);
                    if p.sites.len() > 1
                        && let Some(step) = from.and_then(|i| w.steps.get(i))
                        && let (Some(f), Some(l)) = (&step.file, step.line)
                    {
                        s.push_str(&format!("; through the site at {}", cx.location(f, l)));
                    }
                    s
                }
                Err(reason) => format!("no path kept: {}", no_path_reason(reason)),
            }
        };
        if o.detail {
            for t in &witnesses {
                out.push(format!("    {}: {}", t.table, render(t)));
            }
        } else if let Some(t) = witnesses.first() {
            out.push(format!(
                "    representative path ({}{}): {}",
                t.table,
                if witnesses.len() > 1 {
                    format!(", the nearest of {} tables", witnesses.len())
                } else {
                    String::new()
                },
                render(t)
            ));
        }
    }
    if shown.hidden > 0 {
        out.push(format!(
            "  … +{} with {} — --detail lists them",
            plural(shown.hidden, "entry point", "entry points"),
            plural(
                writing[shown.shown.len()..]
                    .iter()
                    .flat_map(|p| p.tables.keys())
                    .collect::<BTreeSet<_>>()
                    .len(),
                "table",
                "tables"
            )
        ));
    }
    if !others.is_empty() {
        if o.detail {
            out.push(format!(
                "  {} write only temporary records or no table:",
                plural(others.len(), "entry point", "entry points")
            ));
            for p in others {
                out.push(format!("  {}", line(p).trim_start()));
            }
        } else {
            out.push(format!(
                "  {} at {} write only temporary records or no table (--detail lists them)",
                plural(others.len(), "entry point", "entry points"),
                plural(
                    others.iter().map(|p| p.sites.len().max(1)).sum(),
                    "site",
                    "sites"
                )
            ));
        }
    }
    out
}

/// `writes with no path kept` (design §4.1 section 13), by the owner's app.
#[must_use]
pub fn no_path_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    cx: &RollupCx<'_>,
    o: RenderOpts,
) -> Vec<String> {
    if r.no_path.is_empty() {
        return Vec::new();
    }
    let root = cx
        .names_of(&entry.routine_id)
        .map_or_else(|| entry.routine_display.clone(), |(_, n)| n);
    let mut out = vec![format!(
        "writes with no path kept ({}, {}; the witness walk found no path from {root}):",
        plural(r.totals.no_path_tables, "table", "tables"),
        plural(r.no_path.len(), "operation", "operations")
    )];
    let mut by_app: BTreeMap<String, BTreeMap<&str, Vec<&NoPathWrite>>> = BTreeMap::new();
    for n in &r.no_path {
        let app = n.owner.as_ref().map_or_else(
            || "an unknown owner's".to_string(),
            |w| cx.app_label(&w.app),
        );
        let table = if n.table.is_empty() {
            "table not resolved"
        } else {
            n.table.as_str()
        };
        by_app
            .entry(app)
            .or_default()
            .entry(table)
            .or_default()
            .push(n);
    }
    for (app, tables) in by_app {
        let items: Vec<String> = tables
            .iter()
            .map(|(t, ns)| {
                let n = ns[0];
                match &n.owner {
                    Some(w) => format!(
                        "{t} ({} in {}{})",
                        n.kind,
                        w.display,
                        w.anchor
                            .as_ref()
                            .map(|a| format!(" at {a}"))
                            .unwrap_or_default()
                    ),
                    None => format!("{t} ({})", n.kind),
                }
            })
            .collect();
        let n = items.len();
        let f = fold(items, caps::TABLES, "tables", o.detail);
        out.push(format!(
            "  in {app} code ({}): {}",
            plural(n, "table", "tables"),
            f.joined()
        ));
    }
    out
}

/// `literal arguments rule out` (design §4.1 section 14).
#[must_use]
pub fn literal_dead_lines(r: &RoutineRollup, o: RenderOpts) -> Vec<String> {
    if r.literal_dead.is_empty() && r.literal_dead_rows.is_empty() {
        return Vec::new();
    }
    let mut s = format!(
        "literal arguments rule out: {} ({}) the cone lists but the root cannot reach",
        plural(r.literal_dead.len(), "table", "tables"),
        plural(r.literal_dead_rows.len(), "effect row", "effect rows")
    );
    if o.detail && !r.literal_dead.is_empty() {
        let names: Vec<&str> = r.literal_dead.values().map(String::as_str).collect();
        s.push_str(&format!(": {}", names.join(", ")));
    }
    vec![s]
}

/// `temporary records (not writes)` (design §4.1 section 15).
#[must_use]
pub fn temporary_lines(r: &RoutineRollup, o: RenderOpts) -> Vec<String> {
    if r.temporary_ws.is_empty() && r.temporary_beyond.is_empty() && r.temporary_rows.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<String> = r.temporary_ws.values().cloned().collect();
    names.sort();
    let names = fold(names, caps::TEMP_NAMES, "tables", o.detail);
    let mut s = format!(
        "temporary records (not writes): {} in workspace code ({})",
        plural(r.temporary_ws.len(), "table", "tables"),
        plural(r.temporary_rows.len(), "effect row", "effect rows")
    );
    if !names.shown.is_empty() {
        s.push_str(&format!(": {}", names.joined()));
    }
    let beyond_only = r
        .temporary_beyond
        .keys()
        .filter(|k| !r.temporary_ws.contains_key(*k))
        .count();
    s.push_str(&format!(
        "; {} only beyond entry points; only temporary: {}",
        plural(beyond_only, "table", "tables"),
        plural(r.temporary_only.len(), "table", "tables")
    ));
    vec![s]
}

/// The publisher of an event id `<publisher>/event/<name>`.
fn publisher_of(event_id: &str) -> &str {
    event_id.split("/event/").next().unwrap_or(event_id)
}

/// `events:` (design §4.1 section 16).
#[must_use]
pub fn event_lines(r: &RoutineRollup, cx: &RollupCx<'_>, o: RenderOpts) -> Vec<String> {
    let mut by_publisher: BTreeMap<&str, usize> = BTreeMap::new();
    for id in r.events_ws.keys() {
        *by_publisher.entry(publisher_of(id)).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = by_publisher.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let items: Vec<String> = ranked
        .iter()
        .map(|(p, n)| format!("{} {n}", cx.object_display(p)))
        .collect();
    let f = fold(items, caps::PUBLISHERS, "publishers", o.detail);
    let mut s = format!(
        "events: {} raised in workspace code",
        thousands(r.events_ws.len())
    );
    if !f.shown.is_empty() {
        s.push_str(&format!(", by publisher: {}", f.joined()));
    }
    s.push_str(&format!(
        "; {} more only beyond entry points",
        thousands(r.events_beyond.len())
    ));
    vec![s]
}

/// One line per present kind family (design §4.1 section 18).
#[must_use]
pub fn kind_lines(r: &RoutineRollup, entry: &DigestEntryFull) -> Vec<String> {
    let mut out = Vec::new();
    for (key, label) in FAMILIES {
        let n = r.totals.kinds.get(key).copied().unwrap_or(0);
        if n == 0 {
            continue;
        }
        let mut places: Vec<String> = r
            .kind_sites
            .get(key)
            .into_iter()
            .flatten()
            .map(|&i| {
                let e = &entry.effects[i];
                format!(
                    "{} at {} ({})",
                    e.effect_type,
                    row_anchor(e),
                    e.conditionality
                )
            })
            .collect();
        for &ci in r.kind_crossings.get(key).into_iter().flatten() {
            let c = &r.crossings[ci];
            let types: Vec<&str> = c
                .kinds
                .iter()
                .map(String::as_str)
                .filter(|k| family_of(k) == Some(*key) || (*key == "errors" && *k == "UI_ERROR"))
                .collect();
            places.push(format!("{} beyond {}", types.join(", "), crossing_name(c)));
        }
        out.push(format!(
            "{label} at {}: {}",
            plural(n, "place", "places"),
            places.join("; ")
        ));
    }
    out
}

/// `gaps in dependency code` (design §4.1 section 21).
#[must_use]
pub fn dependency_gap_lines(r: &RoutineRollup, o: RenderOpts) -> Vec<String> {
    let g = &r.gaps;
    if g.dependency == 0 && g.not_placed.is_empty() {
        return Vec::new();
    }
    let items: Vec<String> = g
        .top_callees
        .iter()
        .map(|(c, n)| format!("{c} ×{n}"))
        .collect();
    let f = fold(items, caps::TOP_CALLEES, "callees", o.detail);
    let mut s = format!(
        "gaps in dependency code: {} ({} interface, {} dynamic); not placed: {}",
        thousands(g.dependency),
        g.dependency_polymorphic,
        g.dependency_dynamic,
        g.not_placed.len()
    );
    if !f.shown.is_empty() {
        s.push_str(&format!("; top callees: {}", f.joined()));
    }
    vec![s]
}

/// `gaps in workspace code` (digest's form: one unresolved call per line;
/// explain prints its own gap list).
#[must_use]
pub fn workspace_gap_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    o: RenderOpts,
) -> Vec<String> {
    let mut idx: Vec<usize> = r.gaps.workspace.clone();
    if o.detail {
        idx.extend(r.gaps.not_placed.iter().copied());
    }
    if idx.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "gaps in workspace code ({}):",
        plural(
            r.gaps.workspace.len(),
            "call the engine cannot follow",
            "calls the engine cannot follow"
        )
    )];
    let lines: Vec<String> = idx
        .iter()
        .map(|&i| {
            let u = &entry.unresolved[i];
            let at = match (&u.callsite_file, u.callsite_line) {
                (Some(f), Some(l)) => format!("{f}:{}", shown(l)),
                (Some(f), None) => f.clone(),
                (None, _) => "no position kept".to_string(),
            };
            format!(
                "  {} ({}) in {} at {at}",
                u.callee_display, u.status, u.owning_routine_display
            )
        })
        .collect();
    let f = fold(lines, caps::WORKSPACE_GAPS, "gaps", o.detail);
    out.extend(f.shown.iter().cloned());
    if f.hidden > 0 {
        out.push(format!("  … +{} gaps — --detail lists them", f.hidden));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §3.8 rule 4: the cap, or all when at most a quarter more; the full
    /// count and its unit survive the fold (T-W2-3).
    #[test]
    fn fold_shows_the_cap_or_all_and_keeps_the_count() {
        let items = |n: usize| (0..n).map(|i| i.to_string()).collect::<Vec<_>>();
        for (n, shown) in [(11, 11), (12, 12), (13, 13), (15, 15), (16, 12), (40, 12)] {
            let f = fold(items(n), caps::TABLES, "tables", false);
            assert_eq!(f.shown.len(), shown, "{n}");
            assert_eq!(f.total(), n, "{n}");
            if shown < n {
                assert!(f.joined().ends_with(&format!("… +{} tables", n - shown)));
            } else {
                assert!(!f.joined().contains('…'));
            }
        }
        assert_eq!(
            fold(items(40), caps::TABLES, "tables", true).shown.len(),
            40
        );
    }

    /// T-W2-24 (design §3.8 rule 6): routines that share a qualified display
    /// print apart: an overload in the same app by its parameters, a routine
    /// of another app by that app's name; a display of its own prints bare.
    /// The model is stated by assignment (no fixture app pair shares an
    /// object name).
    ///
    /// Discrimination (2026-11-04): `disambiguator` returning "" always (the
    /// old display) fails at "overloads differ"; restored, passes.
    #[test]
    fn shared_displays_print_apart() {
        use crate::engine::l5::test_support::{empty_resolved, routine};
        use crate::program::model::workspace::ModelParameter;
        let param = |i: u32, name: &str, ty: &str| ModelParameter {
            index: i,
            name: name.to_string(),
            type_text: ty.to_string(),
            is_var: false,
            is_record: false,
            table_name: None,
        };
        let mut m = empty_resolved();
        let mut a = routine("Note", "procedure");
        a.stable_routine_id = "ws:Note#1".into();
        a.parameters = vec![param(0, "Msg", "Text")];
        let mut b = routine("Note", "procedure");
        b.stable_routine_id = "ws:Note#2".into();
        b.parameters = vec![param(0, "Msg", "Text"), param(1, "Level", "Integer")];
        let mut c = routine("Note", "procedure");
        c.stable_routine_id = "dep:Note#1".into();
        c.app_guid = "dep".into();
        let mut d = routine("Alone", "procedure");
        d.stable_routine_id = "ws:Alone".into();
        m.workspace.routines = vec![a, b, c, d];
        let apps = vec![serde_json::json!({"appId": "dep", "name": "Base App"})];
        let cx = RollupCx::new(&m, &apps);
        let (qa, qb, qc) = (
            cx.qualified("ws:Note#1"),
            cx.qualified("ws:Note#2"),
            cx.qualified("dep:Note#1"),
        );
        assert_ne!(qa, qb, "overloads differ");
        assert!(qa.ends_with("Note(Msg: Text) [app]"), "{qa}");
        assert!(
            qb.ends_with("Note(Msg: Text; Level: Integer) [app]"),
            "{qb}"
        );
        assert!(qc.ends_with("Note [Base App]"), "{qc}");
        assert!(cx.qualified("ws:Alone").ends_with("::Alone"));
    }

    #[test]
    fn thousands_commas() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(7435), "7,435");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
