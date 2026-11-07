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
use crate::program::model::workspace::{Model, ModelRoutine};

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

    /// ` (N <unit>)` when the fold hides something, else empty: the full
    /// count with the fold's unit, for a line that states it nowhere else
    /// (T-W2-3: every `… +N <unit>` line also carries its full count).
    #[must_use]
    pub fn count_note(&self) -> String {
        if self.hidden > 0 {
            format!(" ({} {})", thousands(self.total()), self.unit)
        } else {
            String::new()
        }
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

/// One overload as the short-handle rule reads it: its parameter types, in
/// order, and its declaration's place (unit, line, column) for source order.
pub struct OverloadShape<'a> {
    pub types: Vec<&'a str>,
    pub order: (&'a str, u32, u32),
}

/// The short handle of overload `me` among its same-app, same-name `others`
/// ([`RollupCx::handle`]'s rule, shared with the test-selection paths): `/n`
/// (its arity) when no other has its arity; `/n[position: type]` naming the
/// parameter types it differs in from the same-arity ones; `/n#k` (k in source
/// order) when it differs in none. The flag says the handle is numbered
/// (`#k`), which only a legend tells apart.
#[must_use]
pub fn overload_short(me: &OverloadShape<'_>, others: &[OverloadShape<'_>]) -> (String, bool) {
    let n = me.types.len();
    let siblings: Vec<&OverloadShape<'_>> = others.iter().filter(|o| o.types.len() == n).collect();
    // A same-arity overload names the parameter types it differs in
    // (`/7[1: Record "X"]`); one that differs in none (two field triggers) is
    // numbered in source order (`/0#2`).
    let differing: Vec<String> = (0..n)
        .filter(|&p| siblings.iter().any(|o| o.types[p] != me.types[p]))
        .map(|p| format!("{}: {}", p + 1, me.types[p]))
        .collect();
    let numbered = !siblings.is_empty() && differing.is_empty();
    let short = if siblings.is_empty() {
        format!("/{n}")
    } else if !differing.is_empty() {
        format!("/{n}[{}]", differing.join("; "))
    } else {
        let before = siblings.iter().filter(|o| o.order < me.order).count();
        format!("/{n}#{}", before + 1)
    };
    (short, numbered)
}

/// An object, member or routine name as AL writes it (output validation V3,
/// §2.1): bare when it is a plain identifier (`[A-Za-z_][A-Za-z0-9_]*`),
/// else in double quotes with each `"` doubled.
#[must_use]
pub fn al_name(name: &str) -> std::borrow::Cow<'_, str> {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(format!("\"{}\"", name.replace('"', "\"\"")))
    }
}

/// The kind of member a member trigger's member is, when the source does
/// not say (a symbol-only row, a graph node): `action` for `OnAction`,
/// `dataitem` for a report/query data item trigger, `element` for an
/// XmlPort member, `field` for a table member or a field trigger of a page
/// or request page, else `control` (a page user-control event).
#[must_use]
pub fn infer_member_kind(object_type: &str, trigger: &str) -> &'static str {
    let t = trigger.to_ascii_lowercase();
    let ty = object_type.to_ascii_lowercase();
    if t == "onaction" {
        return "action";
    }
    if matches!(
        t.as_str(),
        "onpredataitem" | "onaftergetrecord" | "onpostdataitem"
    ) || ty == "query"
    {
        return "dataitem";
    }
    if ty == "xmlport" {
        return "element";
    }
    if ty == "table"
        || ty == "tableextension"
        || matches!(
            t.as_str(),
            "onvalidate"
                | "onlookup"
                | "onafterlookup"
                | "ondrilldown"
                | "onassistedit"
                | "oncontroladdin"
                | "onbeforevalidate"
                | "onaftervalidate"
        )
    {
        return "field";
    }
    "control"
}

/// A routine's member as the display prints it (§2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberPart<'a> {
    /// `field`, `action`, `dataitem`, `control` or `element`.
    pub kind: &'a str,
    pub name: &'a str,
    pub caption: Option<&'a str>,
}

/// The parts of the one routine display (output validation V3, §2.1, §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineParts<'a> {
    pub object_type: &'a str,
    pub object_name: &'a str,
    /// A dependency object's number, printed in brackets after its name
    /// (`Codeunit "Job Queue Dispatcher" (448)`, §2.2).
    pub object_number: Option<i64>,
    pub member: Option<MemberPart<'a>>,
    pub routine: &'a str,
    /// The parameter types (`(Integer, var Text)`) when the name is
    /// overloaded in its object, on every occurrence (§2.1).
    pub signature: Option<String>,
    /// The app label of two loaded objects with one display (` [<app>]`).
    pub app: Option<String>,
}

/// THE routine display (output validation V3, §2.1): every header, list
/// row, path hop and JSON `display` prints a routine through this one
/// function. `Table "CDO E-Mail Job"."Table No."::OnValidate`,
/// `Page X.action(Export "Export Template")::OnAction`,
/// `Codeunit "Job Queue Dispatcher" (448)::HandleRequest`,
/// `Codeunit Log::Note(Text, Integer)`. Names are quoted as AL needs
/// ([`al_name`]); a member's caption prints only when `caption` (headers and
/// legends, not path hops).
#[must_use]
pub fn format_routine(p: &RoutineParts<'_>, caption: bool) -> String {
    let mut out = format_object(p.object_type, p.object_name, p.object_number);
    if let Some(m) = &p.member {
        out.push_str(&format_member(m, caption));
    }
    if !out.is_empty() {
        out.push_str("::");
    }
    out.push_str(&al_name(p.routine));
    if let Some(sig) = &p.signature {
        out.push_str(sig);
    }
    if let Some(app) = &p.app {
        out.push_str(&format!(" [{app}]"));
    }
    out
}

/// The one display (no caption) split into its object and the rest, for a
/// line grouped under its object or a path hop whose object is the previous
/// hop's: (`Table "CDO E-Mail Job"`, `"Table No."::OnValidate`),
/// (`Codeunit Log`, `Note(Text)`), (`Page X`, `action(Y)::OnAction`).
#[must_use]
pub fn split_display(p: &RoutineParts<'_>) -> (String, String) {
    let object = format_object(p.object_type, p.object_name, p.object_number);
    let full = format_routine(p, false);
    let rest = match full.strip_prefix(&object) {
        Some(rest) if !object.is_empty() => rest
            .strip_prefix('.')
            .or_else(|| rest.strip_prefix("::"))
            .unwrap_or(rest)
            .to_string(),
        _ => full.clone(),
    };
    (object, rest)
}

/// An object as the routine display prints it: `Codeunit "Log Mgt"`, with a
/// dependency object's number in brackets; empty for no object.
#[must_use]
pub fn format_object(object_type: &str, object_name: &str, number: Option<i64>) -> String {
    let mut out = match (object_type.is_empty(), object_name.is_empty()) {
        (_, true) => object_type.to_string(),
        (true, false) => al_name(object_name).into_owned(),
        (false, false) => format!("{object_type} {}", al_name(object_name)),
    };
    if let Some(n) = number.filter(|n| *n > 0 && !out.is_empty()) {
        out.push_str(&format!(" ({n})"));
    }
    out
}

/// A member as the display prints it after its object: `."Table No."` for
/// a field, `.action(Name "Caption")` for the other kinds.
#[must_use]
pub fn format_member(m: &MemberPart<'_>, caption: bool) -> String {
    if m.kind == "field" {
        return format!(".{}", al_name(m.name));
    }
    let cap = match m.caption.filter(|c| caption && !c.is_empty()) {
        Some(c) => format!(" \"{}\"", c.replace('"', "\"\"")),
        None => String::new(),
    };
    format!(".{}({}{cap})", m.kind, al_name(m.name))
}

/// The routine display's JSON parts (§2.1): `signature` (the parameter
/// types, every routine), `member` (`{kind, name, caption}`) for a member
/// trigger.
#[must_use]
pub fn member_json(m: &MemberPart<'_>) -> Value {
    json!({"kind": m.kind, "name": m.name, "caption": m.caption})
}

/// The one display of a sited-graph node (output validation V3, §2.2), for a
/// node the model has no row for (a dependency routine or object reached on
/// a reverse walk): `Codeunit "Job Queue Dispatcher" (448)::HandleRequest`,
/// `Table "Sales Invoice Header" (110)."Applies-to Doc. No."::OnLookup`,
/// `Codeunit "X" (6225383)` for an object node. The declared names come from
/// the graph node, never from its key; a table field member's declared case
/// from `model`'s tables when it holds the table (the graph keeps the member
/// lowercased). A workspace object prints no number. `None` for a hub or a
/// node with no declared name.
#[must_use]
pub fn graph_display(
    g: &crate::program::impact_graph::SitedGraph,
    n: u32,
    model: Option<&Model>,
) -> Option<String> {
    graph_display_as(g, n, model, false)
}

/// [`graph_display`] where every app loaded from local source counts as
/// the subject's own (no object number): a test-selection answer, whose
/// graph's primary app is the test app and whose subject is the production
/// app beside it.
#[must_use]
pub fn graph_display_local(g: &crate::program::impact_graph::SitedGraph, n: u32) -> Option<String> {
    graph_display_as(g, n, None, true)
}

/// The §8.1 hub kind of a hub node (`Codeunit.Run of a variable`, …), or
/// `None` for a routine or object node.
#[must_use]
pub fn hub_kind_of(g: &crate::program::impact_graph::SitedGraph, n: u32) -> Option<String> {
    if g.flags(n) & crate::program::impact_graph::sited::flag::HUB == 0 {
        return None;
    }
    crate::engine::vocab::hub_kind(&g.node_id(n))
}

fn graph_display_as(
    g: &crate::program::impact_graph::SitedGraph,
    n: u32,
    model: Option<&Model>,
    local_is_own: bool,
) -> Option<String> {
    let local = local_is_own && g.flags(n) & crate::program::impact_graph::sited::flag::LOCAL != 0;
    // A hub in the §8.1 form (output validation V2): its kind from the
    // closed list, never its wire id (`h:run:codeunit`).
    if let Some(kind) = hub_kind_of(g, n) {
        return Some(crate::engine::vocab::hub(&kind, None, None));
    }
    if let Some((kind, name, number, primary)) = g.object_node_name(n) {
        return Some(format_object(
            kind,
            name,
            number.filter(|_| !primary && !local),
        ));
    }
    let mut nn = g.node_name(n)?;
    nn.primary |= local;
    let member_name: Option<String> = nn.member.map(|m| {
        if nn.member_declared {
            return m.to_string();
        }
        let declared = model.and_then(|model| {
            let app = g.node_app(n).map(|a| g.app_identity(a).0)?;
            let number = nn.object_number?;
            model
                .workspace
                .tables
                .iter()
                .filter(|t| t.table_number == number && t.app_guid.eq_ignore_ascii_case(&app))
                .flat_map(|t| &t.fields)
                .find(|f| f.name.eq_ignore_ascii_case(m))
                .map(|f| f.name.clone())
        });
        declared.unwrap_or_else(|| m.to_string())
    });
    let parts = RoutineParts {
        object_type: nn.object_kind,
        object_name: nn.object_name,
        object_number: nn.object_number.filter(|_| !nn.primary),
        member: member_name.as_deref().map(|name| MemberPart {
            kind: infer_member_kind(nn.object_kind, nn.routine),
            name,
            caption: None,
        }),
        routine: nn.routine,
        signature: (!nn.signature.is_empty()).then(|| nn.signature.to_string()),
        app: None,
    };
    Some(format_routine(&parts, false))
}

/// The model's names of every routine, for [`format_routine`] (§2.1): the
/// declaring object, the member, whether a name is overloaded in its object,
/// whether two apps' routines share a display, and each workspace routine's
/// `selector`. Built once per answer over the model's rows.
pub struct RoutineNames<'m> {
    pub model: &'m Model,
    declaring: DeclaringRows,
    /// Stable routine id → its first model row.
    rows: HashMap<&'m str, usize>,
    /// The stable ids whose name is overloaded in their object (same app,
    /// object, member and name; distinct stable ids).
    overloaded: std::collections::HashSet<&'m str>,
    /// The stable ids whose display (without app label) another app's
    /// routine shares.
    app_shared: std::collections::HashSet<&'m str>,
    /// The workspace display keys (no caption) that name more than one
    /// stable id: those routines' selector is their stable id.
    selector_shared: std::collections::HashSet<String>,
    app_names: HashMap<String, String>,
}

impl<'m> RoutineNames<'m> {
    /// Over `model`; `apps` are the loaded dependencies' rows
    /// (`LoadedDependencies::apps`: `appId`, `name`), for the app label.
    #[must_use]
    pub fn new(model: &'m Model, apps: &[Value]) -> RoutineNames<'m> {
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
        let mut rows: HashMap<&'m str, usize> = HashMap::new();
        for (i, r) in model.workspace.routines.iter().enumerate() {
            rows.entry(r.stable_routine_id.as_str()).or_insert(i);
        }
        let mut names = RoutineNames {
            model,
            declaring: DeclaringRows::new(&model.workspace.objects),
            rows,
            overloaded: Default::default(),
            app_shared: Default::default(),
            selector_shared: Default::default(),
            app_names,
        };
        // Overloads: one app, object, member and name; distinct stable ids.
        type Key = (String, String, Option<String>, String);
        let mut groups: HashMap<Key, Vec<&'m str>> = HashMap::new();
        // App collisions: one display (type, object name, member, name) in
        // two apps.
        let mut displays: HashMap<String, Vec<(&'m str, &'m str)>> = HashMap::new();
        for (&id, &i) in &names.rows {
            let r = &model.workspace.routines[i];
            let object = names.object_name_of(r).to_ascii_lowercase();
            groups
                .entry((
                    r.app_guid.to_ascii_lowercase(),
                    format!("{}/{object}", r.object_id),
                    r.enclosing_member.as_ref().map(|m| m.to_ascii_lowercase()),
                    r.name.to_ascii_lowercase(),
                ))
                .or_default()
                .push(id);
            displays
                .entry(format!(
                    "{}|{object}|{}|{}",
                    r.object_type.to_ascii_lowercase(),
                    r.enclosing_member.as_deref().unwrap_or("").to_lowercase(),
                    r.name.to_ascii_lowercase()
                ))
                .or_default()
                .push((id, r.app_guid.as_str()));
        }
        for ids in groups.values().filter(|ids| ids.len() > 1) {
            names.overloaded.extend(ids.iter().copied());
        }
        for ids in displays.values() {
            let first = ids[0].1;
            if ids.iter().any(|(_, a)| !a.eq_ignore_ascii_case(first)) {
                names.app_shared.extend(ids.iter().map(|(id, _)| *id));
            }
        }
        // Selectors: a workspace routine's display key (no caption) must
        // name it alone.
        let mut seen: HashMap<String, &'m str> = HashMap::new();
        let mut shared = std::collections::HashSet::new();
        for &id in names.rows.keys() {
            if !names.is_primary(id) {
                continue;
            }
            let key = crate::engine::l5::selector_index::normalize_display_key(
                &names.base_selector(id).unwrap_or_default(),
            );
            if let Some(other) = seen.insert(key.clone(), id)
                && other != id
            {
                shared.insert(key);
            }
        }
        names.selector_shared = shared;
        names
    }

    fn object_name_of(&self, r: &ModelRoutine) -> &'m str {
        self.declaring
            .of(&self.model.workspace.objects, r)
            .map_or("", |o| o.name.as_str())
    }

    /// The first model row of a stable routine id.
    #[must_use]
    pub fn row(&self, stable_id: &str) -> Option<&'m ModelRoutine> {
        self.rows
            .get(stable_id)
            .map(|&i| &self.model.workspace.routines[i])
    }

    /// Whether a stable id is a workspace (primary app) routine.
    #[must_use]
    pub fn is_primary(&self, stable_id: &str) -> bool {
        self.row(stable_id)
            .is_some_and(|r| self.model.role_of_app(&r.app_guid) != Role::Dependency)
    }

    /// An app's name, else its guid.
    #[must_use]
    pub fn app_label(&self, guid: &str) -> String {
        self.app_names
            .get(&guid.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| guid.to_string())
    }

    /// The display parts of model row `r` (§2.1, §2.2).
    #[must_use]
    pub fn parts_of<'r>(&'r self, r: &'r ModelRoutine) -> RoutineParts<'r> {
        let dependency = self.model.role_of_app(&r.app_guid) == Role::Dependency;
        let member = r.enclosing_member.as_deref().map(|name| MemberPart {
            kind: r
                .member_detail
                .as_ref()
                .map_or_else(|| infer_member_kind(&r.object_type, &r.name), |d| d.kind),
            name,
            caption: r.member_detail.as_ref().and_then(|d| d.caption.as_deref()),
        });
        let id = r.stable_routine_id.as_str();
        RoutineParts {
            object_type: &r.object_type,
            object_name: self.object_name_of(r),
            object_number: dependency.then_some(r.object_number),
            member,
            routine: &r.name,
            signature: self
                .overloaded
                .contains(id)
                .then(|| crate::engine::l5::selector_index::signature_text(&r.parameters)),
            app: self
                .app_shared
                .contains(id)
                .then(|| self.app_label(&r.app_guid)),
        }
    }

    /// The display of a stable routine id (§2.1), its caption with
    /// `caption`; `None` when the model has no row for it.
    #[must_use]
    pub fn display_with(&self, stable_id: &str, caption: bool) -> Option<String> {
        self.row(stable_id)
            .map(|r| format_routine(&self.parts_of(r), caption))
    }

    /// The header display of a stable routine id (with the member's
    /// caption), else the id.
    #[must_use]
    pub fn display(&self, stable_id: &str) -> String {
        self.display_with(stable_id, true)
            .unwrap_or_else(|| stable_id.to_string())
    }

    /// The path-hop display of a stable routine id (no caption), else the
    /// id.
    #[must_use]
    pub fn hop(&self, stable_id: &str) -> String {
        self.display_with(stable_id, false)
            .unwrap_or_else(|| stable_id.to_string())
    }

    /// The display a selector reads (§2.1): no caption, no dependency
    /// number, no app label.
    fn base_selector(&self, stable_id: &str) -> Option<String> {
        let r = self.row(stable_id)?;
        let mut p = self.parts_of(r);
        p.object_number = None;
        p.app = None;
        Some(format_routine(&p, false))
    }

    /// `selector` (§2.1): the shortest form that resolves to this one
    /// stable id. A workspace routine's display without caption (it holds
    /// the member, and the parameter list exactly when the name is
    /// overloaded); the stable id for a dependency routine (selectors are
    /// scoped to the workspace app) or a display another routine shares.
    #[must_use]
    pub fn selector(&self, stable_id: &str) -> String {
        if !self.is_primary(stable_id) {
            return stable_id.to_string();
        }
        match self.base_selector(stable_id) {
            Some(s)
                if !self.selector_shared.contains(
                    &crate::engine::l5::selector_index::normalize_display_key(&s),
                ) =>
            {
                s
            }
            _ => stable_id.to_string(),
        }
    }

    /// The identity fields every JSON routine row carries (§2.1):
    /// `display`, `signature` (the parameter types), `member` for a member
    /// trigger, `selector`. `stableId` is the caller's.
    #[must_use]
    pub fn identity_json(&self, stable_id: &str) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        let Some(r) = self.row(stable_id) else {
            return m;
        };
        let p = self.parts_of(r);
        m.insert("display".into(), json!(format_routine(&p, true)));
        m.insert(
            "signature".into(),
            json!(crate::engine::l5::selector_index::signature_text(
                &r.parameters
            )),
        );
        if let Some(mp) = &p.member {
            m.insert("member".into(), member_json(mp));
        }
        m.insert("selector".into(), json!(self.selector(stable_id)));
        m
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
    /// Commit sites and commit-bearing entry points printed with a path
    /// (MCP output step 2).
    pub const PATHED_ROWS: usize = 3;
    /// Places listed per kind family line.
    pub const KIND_PLACES: usize = 6;
}

/// The model joins the rollup reads: names of routines and objects, the
/// operation index (owner, anchor, original kind of an operation id), app
/// names and the primary app.
pub struct RollupCx<'m> {
    pub model: &'m Model,
    ops: OperationIndex<'m>,
    primary: Option<String>,
    objects: HashMap<&'m str, usize>,
    /// The one routine display (§2.1).
    pub names: RoutineNames<'m>,
    /// Qualified display → the distinct stable ids that share it (T-W2-24:
    /// overloads, and objects of one name in two apps), for the JSON
    /// `handle`.
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
        let mut cx = RollupCx {
            model,
            // Every id kind: a no-path row's owner and an operation's original
            // kind are read by `lookup` / `claims` (design §3.5).
            ops: OperationIndex::build(model, IndexScope::All),
            names: RoutineNames::new(model, apps),
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
            shared: HashMap::new(),
        };
        let mut shared: HashMap<String, Vec<&'m str>> = HashMap::new();
        for (i, r) in model.workspace.routines.iter().enumerate() {
            let ids = shared.entry(cx.qualified_row(i)).or_default();
            if !ids.contains(&r.stable_routine_id.as_str()) {
                ids.push(r.stable_routine_id.as_str());
            }
        }
        shared.retain(|_, ids| {
            ids.sort_unstable();
            ids.len() > 1
        });
        cx.shared = shared;
        cx
    }

    /// A routine's short handle, JSON `handle` only (V3 §2.1: human text
    /// prints the parameter list instead): `/n` (its arity), `/n[position:
    /// type]` (the types it differs in) when a same-app routine of its
    /// `<Type> <Object>::<Routine>` has its arity, `/n#k` (source order) when
    /// it differs in nothing but its place; then ` [<app>]` when another
    /// app's routine shares it. Empty when the name is its own.
    #[must_use]
    pub fn handle(&self, stable_id: &str) -> String {
        let Some(r) = self.names.row(stable_id) else {
            return String::new();
        };
        let Some(ids) = self
            .names
            .rows
            .get(stable_id)
            .and_then(|&i| self.shared.get(&self.qualified_row(i)))
        else {
            return String::new();
        };
        let row_of = |s: &str| self.names.row(s);
        let others: Vec<&ModelRoutine> = ids
            .iter()
            .filter(|s| **s != stable_id)
            .filter_map(|s| row_of(s))
            .filter(|o| o.app_guid == r.app_guid)
            .collect();
        let mut out = String::new();
        if !others.is_empty() {
            fn shape(o: &ModelRoutine) -> OverloadShape<'_> {
                OverloadShape {
                    types: o.parameters.iter().map(|p| p.type_text.as_str()).collect(),
                    order: (
                        o.source_anchor.source_unit_id.as_str(),
                        o.source_anchor.start_line,
                        o.source_anchor.start_column,
                    ),
                }
            }
            let other_shapes: Vec<OverloadShape<'_>> = others.iter().map(|o| shape(o)).collect();
            out.push_str(&overload_short(&shape(r), &other_shapes).0);
        }
        if ids
            .iter()
            .any(|s| row_of(s).is_some_and(|o| o.app_guid != r.app_guid))
        {
            out.push_str(&format!(" [{}]", self.app_label(&r.app_guid)));
        }
        out
    }

    /// A path hop's object display and the rest of its name (V3 §2.1): the
    /// object (`Codeunit "Log Mgt"`, a dependency's with its number) and
    /// `member::Routine(<types>)` without the member's caption, so a hop
    /// whose object is the previous hop's prints the rest alone.
    #[must_use]
    pub fn hop_parts(&self, stable_id: &str) -> Option<(String, String)> {
        let r = self.names.row(stable_id)?;
        Some(split_display(&self.names.parts_of(r)))
    }

    /// Whether a stable routine id's first row is in a loaded dependency
    /// (an app other than the primary one); `false` when unknown.
    #[must_use]
    pub fn in_dependency(&self, stable_id: &str) -> bool {
        self.names.row(stable_id).is_some_and(|r| {
            let app = r.app_guid.to_ascii_lowercase();
            self.primary.as_deref().is_some_and(|p| p != app)
        })
    }

    /// The source unit of a stable routine id's first row.
    #[must_use]
    pub fn default_file(&self, stable_id: &str) -> Option<String> {
        let f = &self.names.row(stable_id)?.source_anchor.source_unit_id;
        (!f.is_empty()).then(|| f.clone())
    }

    /// The display of a stable routine id (V3 §2.1, [`format_routine`],
    /// with the member's caption), else the id.
    #[must_use]
    pub fn qualified(&self, stable_id: &str) -> String {
        self.names.display(stable_id)
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
        let (mut object, root_name) = self
            .hop_parts(root)
            .unwrap_or_else(|| (String::new(), root.to_string()));
        let mut parts = vec![root_name];
        for h in hops {
            let h = &h.inner;
            let tag = edge_tag(h.edge_kind.as_deref().unwrap_or(h.kind));
            let fallback = h
                .to_display
                .clone()
                .or_else(|| h.callee_display.clone())
                .unwrap_or_default();
            let to = h.to_routine_id.as_deref().unwrap_or("");
            let name = match self.hop_parts(to) {
                Some((o, n)) if o != object && !o.is_empty() => {
                    object = o.clone();
                    format!("{o}{}{n}", hop_joint(&n))
                }
                Some((_, n)) => n,
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
        let at = match (&w.terminal.file, w.terminal.line) {
            (Some(f), Some(l)) => format!(" at {}", self.location(f, l)),
            _ => String::new(),
        };
        let mut line = format!("{} ({what}{at})", self.steps_line(root, &w.steps, from));
        if !w.through_entry_point {
            line.push_str(&format!(
                "; this path does not pass the entry point: {}",
                no_path_reason(w.reason.unwrap_or("search-cap"))
            ));
        }
        line
    }

    /// A witness's routines as one `a → b → …` line (design §3.8 rules 6,
    /// 7): the root, then each step's routine (the object only when it
    /// changes), tagged by its edge; with `from`, the steps before it print
    /// as `…`.
    #[must_use]
    pub fn steps_line(
        &self,
        root: &str,
        steps: &[crate::engine::l5::group_path::WitnessStep],
        from: Option<usize>,
    ) -> String {
        let (mut object, root_name) = self
            .hop_parts(root)
            .unwrap_or_else(|| (String::new(), root.to_string()));
        let mut parts = vec![root_name];
        let start = from.unwrap_or(0).min(steps.len());
        let step_parts =
            |s: &crate::engine::l5::group_path::WitnessStep| match self.hop_parts(&s.to) {
                Some(on) => on,
                None => match s.display.rsplit_once("::") {
                    Some((o, n)) => (o.to_string(), n.to_string()),
                    None => (String::new(), s.display.clone()),
                },
            };
        if start > 0 {
            parts.push("…".to_string());
            object = step_parts(&steps[start - 1]).0;
        }
        for s in &steps[start..] {
            let tag = edge_tag(&s.edge);
            let (o, n) = step_parts(s);
            let name = if o != object && !o.is_empty() {
                object = o.clone();
                format!("{o}{}{n}", hop_joint(&n))
            } else {
                n
            };
            parts.push(format!("{tag}{name}"));
        }
        parts.join(" → ")
    }

    /// An own row's searched witness ([`DigestEffectFull::own_path`]) as a
    /// `path:` line's text; a zero-hop one says the operation is the
    /// routine's own.
    #[must_use]
    pub fn own_path_line(
        &self,
        root: &str,
        w: &crate::engine::l5::group_path::GroupPath,
    ) -> String {
        if w.steps.is_empty() {
            format!(
                "{} (in the routine itself)",
                self.steps_line(root, &[], None)
            )
        } else {
            self.steps_line(root, &w.steps, None)
        }
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

    /// The `<Type> <Object>::<Routine>` key of model row `i` the JSON
    /// `handle` groups by (no member, no parameters).
    fn qualified_row(&self, i: usize) -> String {
        let r = &self.model.workspace.routines[i];
        let object = self.names.object_name_of(r);
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
        self.names.app_label(guid)
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
            display: format_routine(&self.names.parts_of(r), true),
            stable_id: r.stable_routine_id.clone(),
            anchor,
            app: r.app_guid.clone(),
            dependency: self.model.role_of_app(&r.app_guid) == Role::Dependency,
            outcome,
        })
    }
}

/// What joins a hop's object to the rest of its name ([`RollupCx::hop_parts`]):
/// `.` before a member (`"Table No."::OnValidate`, `action(X)::OnAction`),
/// `::` before the routine.
fn hop_joint(rest: &str) -> &'static str {
    let member = rest.starts_with('"') && {
        // A quoted first segment followed by `::` is a field member, not a
        // quoted routine name (which ends the text).
        let mut quoted = false;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            if c == '"' {
                quoted = !quoted;
            } else if !quoted {
                end = Some(i);
                break;
            }
        }
        end.is_some_and(|e| rest[e..].starts_with("::"))
    };
    let kinded = ["action(", "dataitem(", "control(", "element("]
        .iter()
        .any(|k| rest.starts_with(k));
    let plain_member = rest
        .split_once("::")
        .is_some_and(|(m, _)| !m.contains('(') && !m.is_empty());
    if member || kinded || plain_member {
        "."
    } else {
        "::"
    }
}

/// A path tag by edge kind (design §3.8 rule 7).
#[must_use]
pub fn edge_tag(kind: &str) -> &'static str {
    if kind.starts_with("object-run") {
        "[run] "
    } else if kind == "event-dispatch" || kind == "table-event" {
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
        "own-no-witness" => {
            "the listed paths cannot run under literal arguments, and none of this row's \
             operations can run in any state the search from the routine reaches (another \
             operation of this kind keeps the write live)"
        }
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

/// The entry point behind most of a routine's may-persist tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DominantEntryPoint {
    pub name: String,
    /// Where it is entered (`file:line`, `; `-joined).
    pub sites: String,
    /// The may-persist tables beyond it.
    pub tables: usize,
}

/// The one summary (design §3.1, F15).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RollupTotals {
    /// MCP output step 2: the entry point behind more than half of the
    /// may-persist tables (of 10 or more), named by the summary.
    pub dominant_entry_point: Option<DominantEntryPoint>,
    pub may_write_tables: usize,
    pub owned_by_app: usize,
    pub by_dependencies: usize,
    pub unresolved_table_writes: usize,
    pub caller_dependent_tables: usize,
    /// `yes`, `may`, `none` (complete coverage, no limit) or `no observed`.
    pub commits: &'static str,
    pub commit_sites: usize,
    pub commit_entry_points: usize,
    /// COMMIT rows under literal arguments that cannot run (output
    /// validation §8.2, F53): own sites and dependency routines entered.
    pub commit_dead_sites: usize,
    pub commit_dead_entry_points: usize,
    /// The COMMIT site search stopped at its depth bound
    /// ([`crate::engine::l5::digest_cli::SITES_PARTIAL`]): the two counts
    /// above are lower bounds, printed "at least".
    pub commit_sites_partial: bool,
    /// The kind families ([`FAMILIES`]) whose site search stopped at its
    /// depth bound: their `kinds` counts are lower bounds ("at least").
    pub kinds_partial: BTreeSet<&'static str>,
    /// A crossing the root can take toward an effect has no row
    /// ([`crate::engine::l5::digest::ENTRY_POINTS`]): the entry-point counts
    /// are lower bounds ("at least").
    pub entry_points_partial: bool,
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
    /// Every entry point (the presentation of design §3.3: crossings
    /// grouped by target; a multi-target crossing is its own), and the
    /// distinct sites they are entered at (by identity: `file:line`, or the
    /// crossing id for a crossing with no anchor). The scope line's counts.
    pub entry_points: usize,
    pub crossing_sites: usize,
    /// The distinct dependency routines those entry points enter (a
    /// multi-target entry point enters several): what a line that says
    /// "dependency routines entered" counts (round-2 c11).
    pub routines_entered: usize,
    /// Those of [`Self::entry_points`] with a may-persist write beyond them,
    /// and their distinct sites: the `writes beyond dependency entry points`
    /// header's counts. The rest write only temporary records or no table.
    pub writing_entry_points: usize,
    pub writing_sites: usize,
    /// The distinct routines [`Self::writing_entry_points`] enter.
    pub writing_routines: usize,
    /// Explain only: known routine callers, page actions, tests.
    pub callers: Option<usize>,
    /// Explain only (round-2 r2 §2.3-2.4): the summary's callers clause as
    /// the card states it (the direct callers as one set, or what runs a
    /// trigger); when set it replaces the clause built from `callers` and
    /// `page_actions`.
    pub callers_clause: Option<String>,
    pub page_actions: Option<usize>,
    pub tests: Option<usize>,
    /// Explain only (step 1b): the `[Test]` procedures the search started
    /// from; `None` when no test search ran.
    pub tests_loaded: Option<usize>,
    /// Explain only (agent-tasks C, item 1): the sibling test apps the
    /// scope discovery found (`apps`) and whether it finished (`complete`),
    /// so the summary agrees with the `tests:` detail line. `None` when no
    /// discovery ran (digest).
    pub sibling_tests: Option<SiblingTests>,
}

/// What explain's test scope discovery found, for the summary's `tests:`
/// clause: the number of sibling test apps (not analyzed by explain) and
/// whether the search finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiblingTests {
    pub apps: usize,
    pub complete: bool,
}

impl SiblingTests {
    /// The clause appended to the summary's `tests:` part, or `None` when
    /// discovery finished and found no test app (then "no test code loaded"
    /// is the whole truth).
    fn clause(self) -> Option<String> {
        match (self.apps, self.complete) {
            (0, true) => None,
            (0, false) => Some("test app discovery incomplete".to_string()),
            (n, complete) => Some(format!(
                "{} discovered (not analyzed here; the tests tool selects them){}",
                plural(n, "test app", "test apps"),
                if complete {
                    ""
                } else {
                    "; discovery incomplete"
                }
            )),
        }
    }
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
            "entryPoints": {
                "count": self.entry_points,
                "sites": self.crossing_sites,
                "withWrites": self.writing_entry_points,
                "withWritesSites": self.writing_sites,
                // Round-2 c11 (additive): the distinct routines entered.
                "routines": self.routines_entered,
                "withWritesRoutines": self.writing_routines,
            },
        });
        if self.commit_sites_partial {
            v["commits"]["atLeast"] = json!(true);
        }
        // §8.2 (F53, additive): the COMMIT rows literal arguments rule out,
        // counted apart, so `effectsByType.COMMIT` reconciles.
        if self.commit_dead_sites + self.commit_dead_entry_points > 0 {
            v["commits"]["literalDead"] = json!({
                "sites": self.commit_dead_sites,
                "entryPoints": self.commit_dead_entry_points,
            });
        }
        if let Some(d) = &self.dominant_entry_point {
            v["mayWriteTables"]["mostBeyond"] = json!({
                "entryPoint": d.name,
                "enteredAt": d.sites,
                "tables": d.tables,
            });
        }
        if !self.kinds_partial.is_empty() {
            v["kindsAtLeast"] = json!(self.kinds_partial);
        }
        if self.entry_points_partial {
            v["entryPoints"]["atLeast"] = json!(true);
        }
        if let Some(c) = self.callers {
            v["callers"] = json!(c);
        }
        if let Some(a) = self.page_actions {
            v["pageActions"] = json!(a);
        }
        if let Some(t) = self.tests {
            v["tests"] = json!(t);
        }
        if let Some(n) = self.tests_loaded {
            v["testsLoaded"] = json!(n);
        }
        if let Some(s) = self.sibling_tests {
            v["testApps"] = json!({"discovered": s.apps, "discoveryComplete": s.complete});
        }
        v
    }

    /// The human `summary:` line (design §3.8 rule 2).
    #[must_use]
    /// The summary as a short risk block (MCP output step 2): one line per
    /// question an editor asks — writes, transaction, UI and errors, the
    /// other effects, callers and tests, uncertainty — each line the clauses
    /// the one-line summary had. One string; its lines after the first are
    /// indented two more than the `summary:` line (`\n    `).
    /// `writes`: the `writes:` line's text after the label (output validation
    /// V2, §4.1: [`writes_summary`] builds it, leading with a conditional
    /// write when there is no may-persist one).
    pub fn human(&self, writes: &str) -> String {
        let mut block = vec![format!("writes: {writes}")];
        let mut parts: Vec<String> = Vec::new();
        let commits = match self.commits {
            "no observed" => format!(
                "commits: {}",
                crate::engine::vocab::none_observed(
                    "the cone through every resolved call",
                    "calls not resolved"
                )
            ),
            "none" => format!(
                "commits: {}",
                crate::engine::vocab::NO_EVERY_CANDIDATE_DECIDED
            ),
            answer => {
                let at_least = if self.commit_sites_partial {
                    "at least "
                } else {
                    ""
                };
                let mut where_ = Vec::new();
                if self.commit_sites > 0 {
                    where_.push(format!(
                        "{at_least}{}",
                        plural(self.commit_sites, "site", "sites")
                    ));
                }
                if self.commit_entry_points > 0 {
                    where_.push(format!(
                        "{at_least}{}",
                        plural(
                            self.commit_entry_points,
                            "dependency routine entered that commits",
                            "dependency routines entered that commit",
                        )
                    ));
                }
                // §8.2 (F53, c15): the rows literal arguments rule out.
                let mut dead = Vec::new();
                if self.commit_dead_sites > 0 {
                    dead.push(plural(self.commit_dead_sites, "site", "sites"));
                }
                if self.commit_dead_entry_points > 0 {
                    dead.push(plural(
                        self.commit_dead_entry_points,
                        "dependency routine entered",
                        "dependency routines entered",
                    ));
                }
                if !dead.is_empty() {
                    where_.push(format!(
                        "{} more under literal arguments that cannot run",
                        dead.join(" and ")
                    ));
                }
                format!("commits: {answer} ({})", where_.join(", "))
            }
        };
        let family = |key: &str, label: &str| -> Option<String> {
            let &n = self.kinds.get(key).filter(|n| **n > 0)?;
            Some(format!(
                "{label} at {}{}",
                at_least(self.kinds_partial.contains(key)),
                plural(n, "place", "places")
            ))
        };
        let risky: Vec<String> = FAMILIES
            .iter()
            .filter(|(k, _)| matches!(*k, "ui" | "errors"))
            .filter_map(|(k, l)| family(k, l))
            .collect();
        block.push(format!(
            "transaction, UI and errors: {commits}; {}",
            if risky.is_empty() {
                "UI and errors: none observed".to_string()
            } else {
                risky.join("; ")
            }
        ));
        let mut other = vec![format!(
            "events: {} ({} raised in workspace code)",
            thousands(self.events),
            thousands(self.events_in_workspace)
        )];
        other.extend(
            FAMILIES
                .iter()
                .filter(|(k, _)| !matches!(*k, "ui" | "errors"))
                .filter_map(|(k, l)| family(k, l)),
        );
        block.push(format!("other effects: {}", other.join("; ")));
        if let Some(c) = &self.callers_clause {
            parts.push(c.clone());
        } else if let Some(c) = self.callers {
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
            // The sibling test apps explain discovered (item 1): with one
            // found, "no test code loaded" alone read as "there are none".
            let siblings = self.sibling_tests.and_then(SiblingTests::clause);
            parts.push(match (t, self.tests_loaded, siblings) {
                (0, Some(0), None) => "tests: none (no test code loaded)".to_string(),
                (0, Some(0), Some(s)) => format!("tests: none in the workspace; {s}"),
                (0, Some(n), s) => format!(
                    "tests: none of {} loaded{}",
                    thousands(n),
                    s.map(|s| format!("; also {s}")).unwrap_or_default()
                ),
                (0, None, _) => "tests: not searched".to_string(),
                (t, _, s) => format!(
                    "tests: {t}{}",
                    s.map(|s| format!("; also {s}")).unwrap_or_default()
                ),
            });
        }
        if !parts.is_empty() {
            block.push(format!("callers and tests: {}", parts.join("; ")));
        }
        block.push(format!(
            "uncertainty: gaps: {} in workspace code, {} in dependency code, {} not placed",
            self.gaps_workspace, self.gaps_dependency, self.gaps_not_placed
        ));
        format!("summary — {}", block.join("\n    "))
    }

    /// The may-persist part of the `writes:` line (the tables, the
    /// dependency routine behind most of them), or `None` when there is no
    /// may-persist write.
    #[must_use]
    pub fn may_persist_words(&self) -> Option<String> {
        if self.may_write_tables == 0 && self.unresolved_table_writes == 0 {
            return None;
        }
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
        if let Some(d) = &self.dominant_entry_point {
            w.push_str(&format!(
                "; {} of them beyond {} (entered at {})",
                thousands(d.tables),
                d.name,
                d.sites
            ));
        }
        Some(w)
    }
}

/// The text of the summary's `writes:` line (output validation V2, §4.1,
/// F14). A caller-dependent write is a write risk, never a "no": with no
/// may-persist write the line leads with it and states its condition ("may
/// modify persistent T records (Modify at :24) when Rec is not temporary;
/// otherwise it changes a temporary record"), then the absence of any other
/// write in the §8.1 negative-certainty words. With both classes the
/// may-persist writes come first and the conditional ones follow on the
/// same line. With neither, the absence verdict alone.
#[must_use]
pub fn writes_summary(r: &RoutineRollup, entry: &DigestEntryFull) -> String {
    // The routine's own file, as its rows name it (`ws:` prefixed or not).
    let own_file = r
        .root_file
        .clone()
        .or_else(|| entry.routine_anchor.as_ref().and_then(|a| a.file.clone()))
        .map(|f| f.trim_start_matches("ws:").to_string());
    let conditional: Vec<String> = r
        .caller_dependent
        .iter()
        .map(|t| {
            let e = &entry.effects[t.rep];
            let at = match (&e.evidence.file, e.evidence.line) {
                (Some(f), Some(l)) if own_file.as_deref() == Some(f.trim_start_matches("ws:")) => {
                    format!(":{}", shown(l))
                }
                _ => row_anchor(e),
            };
            let kinds = t.kinds.iter().cloned().collect::<Vec<_>>().join(", ");
            let rec = t
                .classes
                .iter()
                .any(|(_, c)| matches!(c, WriteClass::CallerDependent(CallerReason::ImplicitRec)));
            let condition = if rec {
                "when Rec is not temporary (Rec is temporary only when its caller's record is)"
                    .to_string()
            } else {
                let p = t.classes.iter().find_map(|(_, c)| match c {
                    WriteClass::CallerDependent(CallerReason::Parameter(i)) => Some(*i),
                    _ => None,
                });
                match p {
                    Some(i) => format!(
                        "when the record its caller passes (parameter {}) is not temporary",
                        i + 1
                    ),
                    None => "when the record its caller passes is not temporary".to_string(),
                }
            };
            format!(
                "may modify persistent {} records ({kinds} at {at}) {condition}",
                t.table
            )
        })
        .collect();
    let complete = complete(entry, r);
    let absence = |what: &str| {
        if complete {
            format!("{what} (every candidate decided)")
        } else {
            format!(
                "{what}: {}",
                crate::engine::vocab::none_observed(
                    "the cone through every resolved call",
                    &not_searched(entry)
                )
            )
        }
    };
    // §9.1 (F56): the tables named when there are 3 or fewer, each with its
    // operation kinds when a workspace row has them.
    let named = (!r.may_write.is_empty() && r.may_write.len() <= 3).then(|| {
        r.may_write
            .iter()
            .map(
                |(id, name)| match r.workspace_code.iter().find(|t| &t.table_id == id) {
                    Some(t) => {
                        let e = &entry.effects[t.rep];
                        let at = match (&e.evidence.file, e.evidence.line) {
                            (Some(f), Some(l))
                                if own_file.as_deref() == Some(f.trim_start_matches("ws:")) =>
                            {
                                format!(" at :{}", shown(l))
                            }
                            _ => String::new(),
                        };
                        format!(
                            "{name} ({}{at})",
                            t.kinds.iter().cloned().collect::<Vec<_>>().join(", ")
                        )
                    }
                    None => name.clone(),
                },
            )
            .collect::<Vec<_>>()
            .join(", ")
    });
    let may = r.totals.may_persist_words().map(|w| match &named {
        Some(n) => format!("{w}: {n}"),
        None => w,
    });
    match (may, conditional.is_empty()) {
        (Some(w), true) => w,
        (Some(w), false) => format!("{w}; {}", conditional.join("; ")),
        (None, false) => format!(
            "{}; otherwise it changes a temporary record; {}",
            conditional.join("; "),
            absence("no other write")
        ),
        (None, true) => {
            if complete {
                crate::engine::vocab::NO_EVERY_CANDIDATE_DECIDED.to_string()
            } else {
                crate::engine::vocab::none_observed(
                    "the cone through every resolved call",
                    &not_searched(entry),
                )
            }
        }
    }
}

/// What an absence over `entry`'s cone did not search: its coverage
/// reasons, else the calls not resolved.
fn not_searched(entry: &DigestEntryFull) -> String {
    if entry.coverage_reasons.is_empty() {
        "calls not resolved".to_string()
    } else {
        entry.coverage_reasons.join(", ")
    }
}

/// `n unit` with the plural unit when `n != 1`.
#[must_use]
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// `"at least "` for a count that is a lower bound, else nothing.
#[must_use]
pub fn at_least(partial: bool) -> &'static str {
    if partial { "at least " } else { "" }
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
    /// COMMIT rows the root's literal arguments rule out (output validation
    /// §8.2, F53): own ones (sites) and crossing ones (dependency routines
    /// entered), counted apart from the live ones.
    pub commit_dead_sites: usize,
    pub commit_dead_crossings: usize,
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
    /// The root's own source unit (V2 §4.1: a write in it prints `at :24`).
    pub root_file: Option<String>,
}

/// The length of a row's shortest listed path, else of its searched own
/// witness (`usize::MAX` without either).
fn shortest(e: &DigestEffectFull) -> usize {
    e.via_paths
        .iter()
        .map(Vec::len)
        .min()
        .unwrap_or(match &e.own_path {
            Some(Ok(w)) => w.steps.len(),
            _ => usize::MAX,
        })
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
        let mut r = RoutineRollup {
            root_file: cx.default_file(&entry.routine_id),
            ..RoutineRollup::default()
        };
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
                    // T-W2-18 with T-W2-24: the entry point prints as its
                    // targets' qualified displays, an overload with its
                    // parameters (two crossings into two overloads of one
                    // routine printed one name on CDO); the row's raw
                    // `boundary` only when no target is known.
                    let name = if targets.is_empty() {
                        detail(e, "boundary").unwrap_or("").to_string()
                    } else {
                        targets
                            .iter()
                            .map(|t| cx.qualified(t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    crossings.push(CrossingView {
                        id: key.clone(),
                        name,
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
                    if ty == "COMMIT" {
                        r.commit_dead_crossings += 1;
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
                if ty == "COMMIT" {
                    r.commit_dead_sites += 1;
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
        r.totals = r.totals_of(
            cx,
            &caller_own,
            complete,
            crate::engine::l5::digest_cli::sites_partial(entry, "COMMIT"),
        );
        r.totals.kinds_partial = FAMILIES
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| {
                crate::engine::l5::digest::SITE_ENUMERATED.iter().any(|ty| {
                    (family_of(ty) == Some(*key) || (*key == "errors" && *ty == "UI_ERROR"))
                        && crate::engine::l5::digest_cli::sites_partial(entry, ty)
                })
            })
            .collect();
        r.totals.entry_points_partial = crate::engine::l5::digest_cli::sites_partial(
            entry,
            crate::engine::l5::digest::ENTRY_POINTS,
        );
        r
    }

    fn totals_of(
        &self,
        cx: &RollupCx<'_>,
        caller_own: &BTreeSet<String>,
        complete: bool,
        commit_sites_partial: bool,
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
        let points = EntryPointCounts::of(self);
        // MCP output step 2: the entry point behind most of the tables, when
        // one is (more than half of 10 or more).
        let dominant = entry_points(self)
            .iter()
            .map(|p| {
                let n = p.tables.keys().filter(|t| live.contains_key(**t)).count();
                (n, p.name.to_string(), p.sites.join("; "))
            })
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
            .filter(|(n, _, _)| live.len() >= 10 && *n * 2 > live.len())
            .map(|(n, name, sites)| DominantEntryPoint {
                name,
                sites,
                tables: n,
            });
        RollupTotals {
            dominant_entry_point: dominant,
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
            commit_dead_sites: self.commit_dead_sites,
            commit_dead_entry_points: self.commit_dead_crossings,
            commit_entry_points,
            commit_sites_partial,
            kinds_partial: BTreeSet::new(),
            entry_points_partial: false,
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
            entry_points: points.all.points,
            crossing_sites: points.all.sites,
            routines_entered: points.all.routines,
            writing_entry_points: points.writing.points,
            writing_sites: points.writing.sites,
            writing_routines: points.writing.routines,
            callers: None,
            callers_clause: None,
            page_actions: None,
            tests: None,
            tests_loaded: None,
            sibling_tests: None,
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
    /// The front end the answer is for (`--surface`, §6.1): the words of
    /// every fold note and follow-up.
    pub surface: crate::engine::follow_up::Surface,
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
            surface: crate::engine::follow_up::Surface::Cli,
        }
    }

    /// The same, for `surface`.
    #[must_use]
    pub fn with_surface(mut self, surface: crate::engine::follow_up::Surface) -> RenderOpts {
        self.surface = surface;
        self
    }

    /// `detail` in the surface's words (`--detail` | `detail: true`).
    #[must_use]
    pub fn detail_input(&self) -> &'static str {
        crate::engine::follow_up::input(self.surface, crate::engine::follow_up::Input::Detail)
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

/// An own write row's anchor line and its `path:` lines (design §3.4.2:
/// path and anchor of one witness; never invent a path, say why there is
/// none). The listed paths when it keeps one; else its searched witness
/// ([`DigestEffectFull::own_path`]), whose terminal is the anchor; else
/// `no path kept: <reason>`. `path_cap` 0 (`--max-paths 0`) prints the
/// anchor only. Final CDO output review, item 2: before, a row whose listed
/// paths the literal pass had all dropped printed its anchor and nothing
/// else.
pub fn own_witness_lines(
    cx: &RollupCx<'_>,
    root: &str,
    e: &DigestEffectFull,
    kind: &str,
    path_cap: usize,
) -> Vec<String> {
    let listed = path_lines(cx, root, e, path_cap);
    if !listed.is_empty() {
        let mut out = vec![format!("{kind} at {}", row_anchor(e))];
        out.extend(listed);
        return out;
    }
    match &e.own_path {
        Some(Ok(w)) => {
            let at = match (&w.terminal.file, w.terminal.line) {
                (Some(f), Some(l)) => cx.location(f, l),
                _ => row_anchor(e),
            };
            let mut out = vec![format!(
                "{} at {at}",
                cx.terminal_kind(&w.terminal.operation_id, kind)
            )];
            if path_cap > 0 {
                // The search may find the write only after the path enters
                // dependency code and comes back (a re-entry): say so, and
                // why the listed workspace routes were not used.
                let reenters = w.steps.iter().find(|s| cx.in_dependency(&s.to));
                out.push(match reenters {
                    Some(s) => format!(
                        "path (re-enters workspace code beyond {}; no listed workspace route \
                         runs with these literal arguments): {}",
                        cx.qualified(&s.to),
                        cx.own_path_line(root, w)
                    ),
                    None => format!("path: {}", cx.own_path_line(root, w)),
                });
            }
            out
        }
        other => {
            let mut out = vec![format!("{kind} at {}", row_anchor(e))];
            if path_cap > 0 {
                out.push(format!(
                    "no path kept: {}",
                    match other {
                        Some(Err(n)) => no_path_reason(n.reason),
                        _ => "no witness was searched",
                    }
                ));
            }
            out
        }
    }
}

/// A crossing's display: its boundary name, else its id.
fn crossing_name(c: &CrossingView) -> &str {
    if c.name.is_empty() { &c.id } else { &c.name }
}

/// One card's header facts (output validation §5.2): its summary block, and
/// the analysis, limits and scope that the answer's header line prints
/// (design §3.7, §3.8 rule 3 moved there).
#[derive(Debug, Clone)]
pub struct HeaderFacts {
    /// The `summary — …` block (several lines, `\n    ` between them).
    pub summary: String,
    pub analysis: crate::engine::result_header::Analysis,
    /// Each cap with its count.
    pub limits: Vec<String>,
    /// The dependency depth that limits this card: `dependency code: none
    /// reached`, or what was followed with its counts.
    pub depth: String,
}

impl HeaderFacts {
    /// The summary's first line, after `summary — ` (the answer line).
    #[must_use]
    pub fn answer(&self) -> &str {
        let first = self.summary.lines().next().unwrap_or("");
        first.strip_prefix("summary — ").unwrap_or(first)
    }

    /// The summary's lines after the first, each without its indent.
    #[must_use]
    pub fn rest(&self) -> Vec<&str> {
        self.summary.lines().skip(1).map(str::trim_start).collect()
    }
}

/// The completeness of one entry's effects: its coverage status and
/// reasons, and the open-world interface and dynamic calls its cone holds,
/// counted. Explain's and digest's `analysis` line, and each impact
/// root's (round-2 n09: impact said "analysis complete" where explain on
/// the same routine said "partial (… 8 interface calls open world)").
#[must_use]
pub fn entry_analysis(entry: &DigestEntryFull) -> crate::engine::result_header::Analysis {
    let mut reasons = entry.coverage_reasons.clone();
    let counts = open_dynamic_counts(entry);
    if !counts.is_empty() {
        reasons.push(counts.join(", "));
    }
    crate::engine::result_header::Analysis::of(entry.coverage_status == "complete", reasons)
}

/// The open-world interface calls and dynamic calls of an entry's cone, as
/// the analysis line counts them (`8 interface calls open world`).
fn open_dynamic_counts(entry: &DigestEntryFull) -> Vec<String> {
    let (mut open, mut dynamic) = (0usize, 0usize);
    for u in &entry.unresolved {
        match u.status.as_str() {
            "polymorphic" => open += 1,
            "dynamic-target" => dynamic += 1,
            _ => {}
        }
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
    counts
}

/// The header facts of one card (`extra_limits`: the caller's own, explain's
/// gaps).
#[must_use]
pub fn header_facts(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    extra_limits: &[String],
) -> HeaderFacts {
    let lines = header_lines(r, entry, extra_limits);
    let analysis = entry_analysis(entry);
    let limits_line = lines[3].strip_prefix("limits: ").unwrap_or("none");
    let limits = if limits_line == "none" {
        Vec::new()
    } else {
        limits_line.split("; ").map(str::to_string).collect()
    };
    // Round-2 n07: the rollup's entry points are the crossings that carry an
    // effect row; a cone that enters dependency code with no effect beyond
    // it (CDO File::SetFilename → File Management::GetFileName) has none.
    // Whether dependency code is reached at all is the cone walk's fact.
    let reached = entry.unresolved_traversal.dependency_routines;
    let depth = if r.totals.entry_points == 0 && r.totals.crossing_sites == 0 {
        if reached == 0 {
            "dependency code: none reached".to_string()
        } else {
            format!(
                "dependency code followed through every resolved call ({} reached; no effect \
                 beyond them)",
                plural(reached, "dependency routine", "dependency routines")
            )
        }
    } else {
        format!(
            "dependency code followed through every resolved call with literal arguments \
             ({}{} entered at {}{})",
            at_least(r.totals.entry_points_partial),
            plural(
                r.totals.routines_entered,
                "dependency routine",
                "dependency routines"
            ),
            at_least(r.totals.entry_points_partial),
            plural(
                r.totals.crossing_sites,
                "dependency call",
                "dependency calls"
            )
        )
    };
    HeaderFacts {
        summary: lines[0].clone(),
        analysis,
        limits,
        depth,
    }
}

/// The coverage, assumptions, limits and scope lines (design §3.7, §3.8
/// rule 3). `extra_limits`: the caller's own (explain's gaps).
#[must_use]
fn header_lines(
    r: &RoutineRollup,
    entry: &DigestEntryFull,
    extra_limits: &[String],
) -> Vec<String> {
    let mut out = vec![r.totals.human(&writes_summary(r, entry))];
    // §8.1 completeness: always with its subject (`analysis complete`,
    // `analysis partial (<reasons>)`).
    let mut cov = match entry.coverage_status.as_str() {
        "complete" => "analysis complete".to_string(),
        "partial" => "analysis partial".to_string(),
        other => format!("analysis {other}"),
    };
    if !entry.coverage_reasons.is_empty() {
        cov.push_str(&format!(" ({})", entry.coverage_reasons.join(", ")));
    }
    let counts = open_dynamic_counts(entry);
    if !counts.is_empty() {
        cov.push_str(&format!(" — {}", counts.join(", ")));
    }
    out.push(cov);
    out.push(format!(
        "assumptions: {}",
        crate::engine::result_header::LOADED_PROGRAM
    ));
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
         grouped per dependency routine entered ({}{}, at {}{}); writes to temporary records do \
         not directly persist those records; no lock facts (alsem query effects lists locks); \
         counts are distinct tables unless a line says operations; lines are 1-based; --detail \
         lists everything",
        at_least(r.totals.entry_points_partial),
        thousands(r.totals.routines_entered),
        at_least(r.totals.entry_points_partial),
        plural(
            r.totals.crossing_sites,
            "dependency call",
            "dependency calls"
        )
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
        // §5.3 (F31): `commits: no (every candidate decided)` is the
        // summary's own clause; it is not repeated.
        if complete(entry, r) && r.totals.commits == "none" {
            return Vec::new();
        }
        return vec![if complete(entry, r) {
            format!(
                "commits: {}",
                crate::engine::vocab::NO_EVERY_CANDIDATE_DECIDED
            )
        } else {
            format!(
                "commits: {}",
                crate::engine::vocab::none_observed(
                    "the cone through every resolved call",
                    &not_searched(entry)
                )
            )
        }];
    }
    // A site search stopped at its depth bound lists a lower bound.
    let at_least = if r.totals.commit_sites_partial {
        "at least "
    } else {
        ""
    };
    let mut out = vec![format!(
        "commits: {} — {} in workspace code; {at_least}{}:",
        r.totals.commits,
        if r.commit_sites.is_empty() {
            if at_least.is_empty() {
                "none"
            } else {
                "none found"
            }
            .to_string()
        } else {
            format!(
                "{at_least}{}",
                plural(r.commit_sites.len(), "site", "sites")
            )
        },
        plural(
            bearing.len(),
            "dependency routine entered that commits",
            "dependency routines entered that commit"
        )
    )];
    // MCP output step 2: the first `caps::PATHED_ROWS` sites and crossings
    // print their paths; the others their place only (`--detail`: all).
    // The others share one line, each with its place.
    let pathed = |k: usize| o.detail || k < caps::PATHED_ROWS;
    let mut rest: Vec<String> = Vec::new();
    for (k, &i) in r.commit_sites.iter().enumerate() {
        let e = &entry.effects[i];
        let row = format!(
            "COMMIT at {} ({})",
            row_anchor(e),
            crate::engine::vocab::condition(e.conditionality)
        );
        if !pathed(k) {
            rest.push(row);
            continue;
        }
        out.push(format!("  {row}"));
        for p in path_lines(cx, &entry.routine_id, e, o.path_cap) {
            out.push(format!("    {p}"));
        }
    }
    for (k, c) in bearing.into_iter().enumerate() {
        let row = format!("{}, entered at {}", crossing_name(c), c.sites.join("; "));
        if !pathed(k) {
            rest.push(row);
            continue;
        }
        out.push(format!("  {row}"));
        // W2-6 (2f): the COMMIT's own terminal witness through the crossing.
        if o.path_cap > 0 {
            out.extend(
                terminal_witness_lines(cx, entry, c, "COMMIT")
                    .into_iter()
                    .map(|l| format!("    {l}")),
            );
        }
    }
    if !rest.is_empty() {
        out.push(format!(
            "  and {} more ({} prints their paths): {}",
            rest.len(),
            o.detail_input(),
            rest.join("; ")
        ));
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
        out.extend(
            own_witness_lines(cx, &entry.routine_id, rep, rep_kind, o.path_cap)
                .into_iter()
                .map(|l| format!("    {l}")),
        );
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
            "  … +{} in the same form ({}) — {} lists them",
            plural(folded.hidden, "table", "tables"),
            names.join(", "),
            o.detail_input()
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
         temporary",
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
    /// Its sites by identity: each `file:line`, or the crossing id of a
    /// crossing with no anchor (so it still counts once).
    site_keys: BTreeSet<&'r str>,
    /// The dependency routines it enters, by stable id (a crossing with no
    /// known target by its crossing id, so it still counts once). A
    /// multi-target crossing (an event with three subscribers) is ONE entry
    /// point and several routines entered.
    routine_keys: BTreeSet<&'r str>,
}

/// One population's counts: its entry points, the distinct dependency
/// routines they enter and the distinct sites they are entered at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryCount {
    pub points: usize,
    pub routines: usize,
    pub sites: usize,
}

/// The entry-point counts every human line states (final CDO output
/// review, item 3; design §3.6 "a total is a union by identity, never a
/// sum"): entry points, distinct routines entered and distinct sites over
/// every entry point, over those with a may-persist write beyond them, and
/// over the rest. The entry points partition (each is one or the other), so
/// `all.points == writing.points + other.points`; the routines and sites are
/// unions, so a routine or site in both kinds counts once in `all`.
/// Before, the scope line counted writing CROSSINGS and summed their sites
/// (IssueReminder on CDO: "14 entry points at 14 sites") while the sections
/// counted entry points ("12 … at 14 sites", "15 … at 17 sites"). A line
/// that says "dependency routines entered" states `routines` (round-2 c11:
/// one event entry point into three subscribers printed "1 dependency
/// routine entered" beside the three routines' names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryPointCounts {
    pub all: EntryCount,
    pub writing: EntryCount,
    pub other: EntryCount,
}

impl EntryPointCounts {
    #[must_use]
    pub fn of(r: &RoutineRollup) -> EntryPointCounts {
        Self::of_points(&entry_points(r))
    }

    fn of_points(points: &[EntryPoint<'_>]) -> EntryPointCounts {
        let count = |pred: &dyn Fn(&EntryPoint<'_>) -> bool| {
            let chosen: Vec<&EntryPoint<'_>> = points.iter().filter(|p| pred(p)).collect();
            let routines: BTreeSet<&str> = chosen
                .iter()
                .flat_map(|p| p.routine_keys.iter().copied())
                .collect();
            let sites: BTreeSet<&str> = chosen
                .iter()
                .flat_map(|p| p.site_keys.iter().copied())
                .collect();
            EntryCount {
                points: chosen.len(),
                routines: routines.len(),
                sites: sites.len(),
            }
        };
        EntryPointCounts {
            all: count(&|_| true),
            writing: count(&|p| !p.temp_only),
            other: count(&|p| p.temp_only),
        }
    }
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
                site_keys: BTreeSet::new(),
                routine_keys: BTreeSet::new(),
            });
            out.len() - 1
        });
        let p = &mut out[i];
        p.crossings.push(ci);
        p.sites.extend(c.sites.iter().map(String::as_str));
        if c.sites.is_empty() {
            p.site_keys.insert(c.id.as_str());
        } else {
            p.site_keys.extend(c.sites.iter().map(String::as_str));
        }
        if c.target_ids.is_empty() {
            p.routine_keys.insert(c.id.as_str());
        } else {
            p.routine_keys
                .extend(c.target_ids.iter().map(String::as_str));
        }
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

/// One entry point's may-persist writes beyond it (MCP output wave 3a):
/// what `alsem query`'s `beyond` fields state, from the same entry points
/// [`beyond_lines`] renders (refined groups, after the literal pass).
#[derive(Debug, Clone)]
pub struct BeyondWrites<'r> {
    /// The boundary display (`Codeunit X::OnRun`).
    pub name: &'r str,
    /// The entered dependency routines (stable ids), sorted.
    pub targets: Vec<&'r str>,
    /// `file:line` of each site (1-based), sorted.
    pub sites: Vec<&'r str>,
    /// May-persist tables, id → display.
    pub tables: BTreeMap<&'r str, &'r str>,
    /// Per table id, the digest write types beyond (`DB_INSERT`, …).
    pub types_of: BTreeMap<&'r str, BTreeSet<&'r str>>,
    /// Writes on a table not resolved.
    pub unresolved: usize,
}

/// Every entry point that carries a may-persist write, in [`beyond_lines`]'
/// order (most tables first, then by name).
#[must_use]
pub fn beyond_writes(r: &RoutineRollup) -> Vec<BeyondWrites<'_>> {
    entry_points(r)
        .into_iter()
        .filter(|p| !p.temp_only)
        .map(|p| {
            let mut targets: Vec<&str> = p
                .crossings
                .iter()
                .flat_map(|&ci| r.crossings[ci].target_ids.iter().map(String::as_str))
                .collect();
            targets.sort_unstable();
            targets.dedup();
            let mut types_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for (ty, ids) in &p.by_type {
                for id in ids {
                    types_of.entry(id).or_default().insert(ty);
                }
            }
            BeyondWrites {
                name: p.name,
                targets,
                sites: p.sites,
                tables: p.tables,
                types_of,
                unresolved: p.unresolved,
            }
        })
        .collect()
}

/// The distinct dependency routines `ws` enter (an entry point with no known
/// target counts once, by its display): what a line that says "dependency
/// routines entered" states; `ws.len()` is the entry points (round-2 c11).
#[must_use]
pub fn routines_entered(ws: &[BeyondWrites<'_>]) -> usize {
    ws.iter()
        .flat_map(|w| {
            if w.targets.is_empty() {
                vec![w.name]
            } else {
                w.targets.clone()
            }
        })
        .collect::<BTreeSet<&str>>()
        .len()
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
    // The counts of the one union (EntryPointCounts), as the scope line's.
    let counts = EntryPointCounts::of(r);
    // Output validation §8.2 (F53): no dependency call writes a record that
    // may persist — an absence verdict, in the §8.1 words, with the calls
    // that write only temporary records (or no table) counted in its unit.
    if writing.is_empty() && !o.detail {
        let verdict = if r.totals.gaps_dependency == 0 && !r.totals.entry_points_partial {
            "no (every candidate decided".to_string()
        } else {
            format!(
                "none observed (searched: the whole cone beyond dependency calls with literal \
                 arguments; not searched: {} in dependency code",
                plural(
                    r.totals.gaps_dependency,
                    "call not resolved",
                    "calls not resolved"
                )
            )
        };
        return vec![format!(
            "writes beyond dependency calls: {verdict}; {} at {} write only temporary records \
             or no table, which does not directly persist them)",
            plural(
                counts.other.routines,
                "dependency routine entered",
                "dependency routines entered"
            ),
            plural(counts.other.sites, "dependency call", "dependency calls"),
        )];
    }
    let mut out = vec![format!(
        "writes beyond dependency calls ({} through {}{} at {}{}):",
        plural(tables.len(), "table", "tables"),
        at_least(r.totals.entry_points_partial),
        plural(
            counts.writing.routines,
            "dependency routine entered",
            "dependency routines entered"
        ),
        at_least(r.totals.entry_points_partial),
        plural(counts.writing.sites, "dependency call", "dependency calls")
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
        "dependency routines entered",
        o.detail,
    );
    let root_name = cx
        .hop_parts(&entry.routine_id)
        .map_or_else(|| entry.routine_id.clone(), |(_, n)| n);
    for p in &shown.shown {
        out.push(line(p));
        if o.path_cap == 0 {
            continue;
        }
        // The prefix: the path to the entry point. By default it is the
        // representative path's start, so the two print as one line.
        let prefix: Option<String> = p.prefix.and_then(|i| {
            path_lines(cx, &entry.routine_id, &entry.effects[i], 1)
                .into_iter()
                .next()
                .and_then(|l| l.strip_prefix("path: ").map(str::to_string))
        });
        if o.detail
            && let Some(pre) = &prefix
        {
            out.push(format!("    prefix path: {pre}"));
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
            // `root → … → rest` with the prefix in place of `…`.
            let mut text = render(t);
            let elided = format!("{root_name} → … → ");
            if let Some(pre) = &prefix
                && let Some(rest) = text.strip_prefix(&elided)
            {
                text = format!("{pre} → {rest}");
            } else if let Some(pre) = &prefix {
                out.push(format!("    prefix path: {pre}"));
            }
            out.push(format!(
                "    representative path ({}{}): {text}",
                t.table,
                if witnesses.len() > 1 {
                    format!(", the nearest of {} tables", witnesses.len())
                } else {
                    String::new()
                },
            ));
        } else if let Some(pre) = &prefix {
            out.push(format!("    prefix path: {pre}"));
        }
    }
    if shown.hidden > 0 {
        out.push(format!(
            "  … +{} with {} — {} lists them",
            plural(
                writing[shown.shown.len()..]
                    .iter()
                    .flat_map(|p| p.routine_keys.iter())
                    .collect::<BTreeSet<_>>()
                    .len(),
                "dependency routine entered",
                "dependency routines entered"
            ),
            plural(
                writing[shown.shown.len()..]
                    .iter()
                    .flat_map(|p| p.tables.keys())
                    .collect::<BTreeSet<_>>()
                    .len(),
                "table",
                "tables"
            ),
            o.detail_input()
        ));
    }
    if !others.is_empty() {
        if o.detail {
            out.push(format!(
                "  {} at {} write only temporary records or no table:",
                plural(
                    counts.other.routines,
                    "dependency routine entered",
                    "dependency routines entered"
                ),
                plural(counts.other.sites, "dependency call", "dependency calls")
            ));
            for p in others {
                out.push(format!("  {}", line(p).trim_start()));
            }
        } else {
            out.push(format!(
                "  {} at {} write only temporary records or no table ({} lists them)",
                plural(
                    counts.other.routines,
                    "dependency routine entered",
                    "dependency routines entered"
                ),
                plural(counts.other.sites, "dependency call", "dependency calls"),
                o.detail_input()
            ));
        }
    }
    // Shared runs of hops print once over the whole answer
    // (`path_segments`, output validation §7.5): each output segments the
    // region that holds this section.
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
        .hop_parts(&entry.routine_id)
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
        "temporary records (writes to them do not directly persist): {} in workspace code ({})",
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
        "; {} only beyond dependency calls; only temporary: {}",
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
    // §5.3 (F31): an empty section the summary already states (`events: 0`)
    // is not printed again.
    if r.events_ws.is_empty() && r.events_beyond.is_empty() {
        return Vec::new();
    }
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
    // Output validation §9.1 (F72): what the cone raises, told apart from
    // what the routine publishes itself (explain adds `publishes:`).
    let mut s = format!(
        "events raised ({} in workspace code, downstream)",
        thousands(r.events_ws.len())
    );
    // F80: the distinct names, per publisher, when they come from at most
    // [`EVENT_NAME_PUBLISHERS`] publishers (or with detail); else the
    // counts per publisher and how to get the names.
    if !ranked.is_empty() && (ranked.len() <= EVENT_NAME_PUBLISHERS || o.detail) {
        let name_of = |id: &str| -> String {
            cx.model
                .events
                .graph
                .events
                .iter()
                .find(|e| e.id == id)
                .map_or_else(
                    || id.rsplit("/event/").next().unwrap_or(id).to_string(),
                    |e| e.event_name.clone(),
                )
        };
        let groups: Vec<String> = ranked
            .iter()
            .map(|(p, _)| {
                let mut names: Vec<String> = r
                    .events_ws
                    .keys()
                    .filter(|id| publisher_of(id) == *p)
                    .map(|id| name_of(id))
                    .collect();
                names.sort();
                names.dedup();
                format!("{}: {}", cx.object_display(p), names.join(", "))
            })
            .collect();
        s.push_str(&format!(": {}", groups.join("; ")));
    } else {
        let f = fold(items, caps::PUBLISHERS, "publishers", o.detail);
        if !f.shown.is_empty() {
            s.push_str(&format!(
                ", by publisher{}: {}; names: {} lists them",
                f.count_note(),
                f.joined(),
                o.detail_input()
            ));
        }
    }
    s.push_str(&format!(
        "; {} more only beyond dependency calls",
        thousands(r.events_beyond.len())
    ));
    vec![s]
}

/// How many publishers the `events raised` line names each event of
/// (output validation §9.1, F80).
const EVENT_NAME_PUBLISHERS: usize = 5;

/// One line per present kind family (design §4.1 section 18). Telemetry
/// (output validation §5.3, F81) prints as its count in the summary's
/// `other effects:` only: a section about code nobody asked about is a count,
/// not a line; `detail` lists its places.
#[must_use]
pub fn kind_lines(r: &RoutineRollup, entry: &DigestEntryFull, o: RenderOpts) -> Vec<String> {
    let detail = o.detail;
    let mut out = Vec::new();
    for (key, label) in FAMILIES {
        let n = r.totals.kinds.get(key).copied().unwrap_or(0);
        if n == 0 || (*key == "telemetry" && !detail) {
            continue;
        }
        let mut places: Vec<String> = r
            .kind_sites
            .get(key)
            .into_iter()
            .flatten()
            .map(|&i| {
                let e = &entry.effects[i];
                // F3: a background start names the codeunit it starts.
                let starts = e
                    .detail
                    .iter()
                    .find(|(k, _)| k == "starts")
                    .map_or(String::new(), |(_, s)| format!(", starts {s}"));
                format!(
                    "{} at {} ({}){starts}",
                    e.effect_type,
                    row_anchor(e),
                    crate::engine::vocab::condition(e.conditionality)
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
        // MCP output step 2: the first places, the rest counted.
        let places = fold(places, caps::KIND_PLACES, "places", detail);
        out.push(format!(
            "{label} at {}{}: {}{}",
            at_least(r.totals.kinds_partial.contains(key)),
            plural(n, "place", "places"),
            places.shown.join("; "),
            if places.hidden > 0 {
                format!(
                    "; … +{} places ({} lists them)",
                    places.hidden,
                    o.detail_input()
                )
            } else {
                String::new()
            }
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
        s.push_str(&format!("; top callees{}: {}", f.count_note(), f.joined()));
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
        out.push(format!(
            "  … +{} gaps — {} lists them",
            f.hidden,
            o.detail_input()
        ));
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
                assert_eq!(f.count_note(), format!(" ({n} tables)"), "{n}");
            } else {
                assert!(!f.joined().contains('…'));
                assert_eq!(f.count_note(), "", "{n}");
            }
        }
        assert_eq!(
            fold(items(40), caps::TABLES, "tables", true).shown.len(),
            40
        );
    }

    /// T-W2-24, V3 §2.1: routines that share a qualified display print
    /// apart: an overload in the same app by its parameter types (on every
    /// occurrence), a routine of another app by that app's name; a name of
    /// its own prints bare. The model is stated by assignment (no fixture app
    /// pair shares an object name).
    ///
    /// Discrimination (2026-11-04, kept for V3): `RoutineNames::new` marking
    /// no stable id overloaded fails at "overloads differ"; restored, passes.
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
        // Same parameter names, other types: the typed form.
        let mut e = routine("Pick", "procedure");
        e.stable_routine_id = "ws:Pick#1".into();
        e.parameters = vec![param(0, "X", "Integer")];
        let mut f = routine("Pick", "procedure");
        f.stable_routine_id = "ws:Pick#2".into();
        f.parameters = vec![param(0, "X", "Text")];
        m.workspace.routines = vec![a, b, c, d, e, f];
        let apps = vec![serde_json::json!({"appId": "dep", "name": "Base App"})];
        let cx = RollupCx::new(&m, &apps);
        let (qa, qb, qc) = (
            cx.qualified("ws:Note#1"),
            cx.qualified("ws:Note#2"),
            cx.qualified("dep:Note#1"),
        );
        assert_ne!(qa, qb, "overloads differ");
        assert!(qa.ends_with("Note(Text) [app]"), "{qa}");
        assert!(qb.ends_with("Note(Text, Integer) [app]"), "{qb}");
        assert!(qc.ends_with("Note [Base App]"), "{qc}");
        assert!(cx.qualified("ws:Alone").ends_with("::Alone"));
        assert!(cx.qualified("ws:Pick#1").ends_with("Pick(Integer)"));
        assert!(cx.qualified("ws:Pick#2").ends_with("Pick(Text)"));
    }

    /// MCP output step 2: a path's short handle tells every overload apart.
    /// Hand-stated overloads (by assignment): `Note/1` and `Note/2` differ in
    /// arity; `Pick/1[1: Integer]` and `Pick/1[1: Text]` in a type; two
    /// `OnValidate` field triggers differ in nothing but their place, so they
    /// are numbered in source order and their declarations are the legend.
    ///
    /// Discrimination (2026-11-05): numbering every same-arity overload `#1`
    /// (`before + 1` → `1`) fails at "triggers differ"; restored, passes.
    #[test]
    fn short_handles_tell_every_overload_apart() {
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
        let mut e = routine("Pick", "procedure");
        e.stable_routine_id = "ws:Pick#1".into();
        e.parameters = vec![param(0, "X", "Integer")];
        let mut f = routine("Pick", "procedure");
        f.stable_routine_id = "ws:Pick#2".into();
        f.parameters = vec![param(0, "X", "Text")];
        let mut g = routine("OnValidate", "trigger");
        g.stable_routine_id = "ws:OnValidate#1".into();
        g.source_anchor.source_unit_id = "ws:src/T.al".into();
        g.source_anchor.start_line = 9;
        let mut h = routine("OnValidate", "trigger");
        h.stable_routine_id = "ws:OnValidate#2".into();
        h.source_anchor.source_unit_id = "ws:src/T.al".into();
        h.source_anchor.start_line = 19;
        m.workspace.routines = vec![a, b, e, f, h, g];
        let cx = RollupCx::new(&m, &[]);
        let handles: Vec<String> = [
            "ws:Note#1",
            "ws:Note#2",
            "ws:Pick#1",
            "ws:Pick#2",
            "ws:OnValidate#1",
            "ws:OnValidate#2",
        ]
        .iter()
        .map(|id| cx.handle(id))
        .collect();
        assert_eq!(handles[0], "/1");
        assert_eq!(handles[1], "/2");
        assert_eq!(handles[2], "/1[1: Integer]");
        assert_eq!(handles[3], "/1[1: Text]");
        assert_ne!(handles[4], handles[5], "triggers differ");
        assert_eq!(handles[4], "/0#1", "source order");
    }

    /// V3 §2.1/§2.2: the one display's member, caption, quoting and
    /// dependency-number rules, as `format_routine` prints them.
    #[test]
    fn the_one_display_prints_members_quotes_and_numbers() {
        let base = RoutineParts {
            object_type: "Table",
            object_name: "CDO E-Mail Job",
            object_number: None,
            member: Some(MemberPart {
                kind: "field",
                name: "Table No.",
                caption: Some("Table No."),
            }),
            routine: "OnValidate",
            signature: None,
            app: None,
        };
        assert_eq!(
            format_routine(&base, true),
            "Table \"CDO E-Mail Job\".\"Table No.\"::OnValidate"
        );
        let action = RoutineParts {
            object_type: "Page",
            object_name: "CDO E-Mail Templates",
            member: Some(MemberPart {
                kind: "action",
                name: "ExportTemplateAction",
                caption: Some("Export Template"),
            }),
            routine: "OnAction",
            ..base.clone()
        };
        assert_eq!(
            format_routine(&action, true),
            "Page \"CDO E-Mail Templates\".action(ExportTemplateAction \"Export Template\")::OnAction"
        );
        assert_eq!(
            format_routine(&action, false),
            "Page \"CDO E-Mail Templates\".action(ExportTemplateAction)::OnAction",
            "no caption on a path hop"
        );
        let dep = RoutineParts {
            object_type: "Codeunit",
            object_name: "Job Queue Dispatcher",
            object_number: Some(448),
            member: None,
            routine: "HandleRequest",
            ..base.clone()
        };
        assert_eq!(
            format_routine(&dep, true),
            "Codeunit \"Job Queue Dispatcher\" (448)::HandleRequest"
        );
        let plain = RoutineParts {
            object_type: "PageExtension",
            object_name: "CDOSalesInvoiceExt",
            object_number: None,
            member: None,
            routine: "Send",
            signature: Some("(Integer, var Text)".into()),
            ..base.clone()
        };
        assert_eq!(
            format_routine(&plain, true),
            "PageExtension CDOSalesInvoiceExt::Send(Integer, var Text)"
        );
        assert_eq!(al_name("A\"B"), "\"A\"\"B\"");
        assert_eq!(al_name("_x1"), "_x1");
        assert_eq!(al_name("1x"), "\"1x\"");
        // A hop's joint: a member follows its object with `.`.
        assert_eq!(hop_joint("\"Table No.\"::OnValidate"), ".");
        assert_eq!(hop_joint("Amount::OnValidate"), ".");
        assert_eq!(hop_joint("action(X)::OnAction"), ".");
        assert_eq!(hop_joint("Run"), "::");
        assert_eq!(hop_joint("Send(Integer)"), "::");
        assert_eq!(hop_joint("\"Do It\""), "::");
    }

    /// Final CDO output review, item 3 (design §3.3, §3.6): the scope line,
    /// the `writes beyond dependency entry points` header and the "write
    /// only temporary records" line count ENTRY POINTS (crossings grouped
    /// by target; a multi-target crossing is its own) and DISTINCT sites,
    /// and the scope's counts are the union of the two sections' by
    /// identity. Hand-stated crossings: two crossings into one writing
    /// target (two sites: one entry point), one temp-only crossing that
    /// SHARES one of those sites (a site entering both kinds counts once in
    /// the union), a multi-target crossing with no anchor (counted once by
    /// its id), and a second temp-only crossing at its own site.
    ///
    /// Discrimination (2026-11-05): counting the scope's sites as the sum
    /// of the per-entry-point counts (`.map(|p| p.site_keys.len()).sum()` in
    /// `EntryPointCounts::of`) fails at "the union: a shared site counts
    /// once" (5 sites, not 4); restored, passes. Before the fix the scope
    /// line printed the writing CROSSINGS (here 2) as entry points.
    #[test]
    fn entry_point_counts_are_unions_by_identity() {
        let crossing = |id: &str, targets: &[&str], sites: &[&str], writes: bool| CrossingView {
            id: id.to_string(),
            name: targets.join(", "),
            target_ids: targets.iter().map(|t| t.to_string()).collect(),
            sites: sites.iter().map(|s| s.to_string()).collect(),
            tables: if writes {
                BTreeMap::from([("dep/table/1".to_string(), "T".to_string())])
            } else {
                BTreeMap::new()
            },
            ..CrossingView::default()
        };
        let r = RoutineRollup {
            crossings: vec![
                crossing("cs:1", &["dep:Post"], &["ws:a.al:1"], true),
                crossing("cs:2", &["dep:Post"], &["ws:b.al:2"], true),
                crossing("cs:3", &["dep:Temp"], &["ws:b.al:2"], false),
                crossing("op:4", &["dep:Page::A", "dep:Page::B"], &[], false),
                crossing("cs:5", &["dep:Other"], &["ws:c.al:3"], false),
            ],
            ..RoutineRollup::default()
        };
        let c = EntryPointCounts::of(&r);
        let n = |points, routines, sites| EntryCount {
            points,
            routines,
            sites,
        };
        assert_eq!(c.writing, n(1, 1, 2), "one entry point at two sites");
        // Round-2 c11: the multi-target entry point enters two routines.
        assert_eq!(c.other, n(3, 4, 3), "three temp-only entry points: {c:?}");
        assert_eq!(
            c.all.points,
            c.writing.points + c.other.points,
            "entry points partition: {c:?}"
        );
        assert_eq!(c.all, n(4, 5, 4), "the union: a shared site counts once");
    }

    #[test]
    fn thousands_commas() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(7435), "7,435");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
