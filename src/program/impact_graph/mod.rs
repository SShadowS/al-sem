//! `alsem graph` (#63): the program's call graph for diff-scoped test selection.
//!
//! Design and rationale: `docs/superpowers/specs/2026-10-13-callgraph-export-design.md`.
//! This doc is the wire contract (spec §4.2-§4.6 and §5); a consumer may rely on
//! exactly what it states.
//!
//! # Wire format (v1)
//!
//! JSONL, one `{"t": <type>, "v": {...}}` object per line. Line types: `meta`
//! (exactly one, first), then `node` lines sorted by `id`, then `edge` lines
//! sorted by `(caller, callee, kind)`. The output is deterministic for the same
//! input and engine version.
//!
//! `meta.schema` is `"al-sem.callgraph"`, `meta.version` is `1` (a major
//! version). New optional fields and new values of `via`, `hiddenCallers`,
//! `hub`, `assumptions` and node `kind` are additive; a consumer must ignore
//! unknown fields and read unknown values fail-safe (an unknown edge `kind` as
//! indirect, an unknown `confidence` as `Open`, an unknown `hiddenCallers` value
//! as hidden callers). A changed meaning or a removed field bumps `version`.
//!
//! # Semantics: may-execute
//!
//! An edge `caller -> callee` means: when `caller` executes, `callee` may
//! execute. Walking edges backwards from a changed routine finds every test that
//! may execute it, with three extra rules for the consumer:
//!
//! - **the universal hub** (`hub: "any"`, `universal: true`, id `h:any`) has no
//!   outgoing edges but stands for "may call anything": treat it as a caller of
//!   every node;
//! - **`hiddenCallers`** on a node: a way it can run that no edge shows. If a
//!   changed routine's reverse cone contains such a node, static reachability is
//!   incomplete for that change;
//! - **`Open` edges** (hub edges): the path is a possibility, not a resolved
//!   call. A test found only through `Open` edges is a weaker selection.
//!
//! Nodes (`kind`): `Method` and `Trigger` (routines), `Object` (a runnable object:
//! a run reaches all of its triggers), `Hub` (a bounded stand-in for the targets of
//! open call sites: `name` = any procedure with that name and parameter count,
//! `run` = any object of a kind, `record` = any trigger of a record operation,
//! `callbacks` = everything a symbol-only app can call back into, `any`).
//!
//! Edges (`kind`, `via`):
//! - `Direct` `call` — a call; `Interface` `interface` — interface dispatch;
//!   `Event` `eventFlow` — publisher -> subscriber; `Trigger` `implicitTrigger` —
//!   record operation -> table trigger; `Run` `run` — a run of an object, to the
//!   object node (never straight to a trigger);
//! - `Trigger` `lifecycle` — object node -> its triggers, its extensions'
//!   triggers, a page's parts, source-table triggers and platform events, the
//!   objects its actions open, an XmlPort's table triggers;
//! - `Run` `declaredRunnable` — a routine declaring a `Page`/`Report`/`XmlPort`/
//!   `Query`/`TestPage`/`TestRequestPage` variable may run that object;
//! - `Event` `testHandler` — a test -> the handlers it lists; `Run` `testRunner`
//!   — a test -> its codeunit (its `OnRun` runs first);
//! - `Open` edges: `unresolved`, `dynamic`, `dispatchingBuiltin` (into a hub),
//!   `parseRecovered`, `coverageMiss` (into `h:any`), `opaqueBody` (symbol-only
//!   code -> its app's callbacks hub), `hubExpansion` (hub -> what it stands for).
//!
//! `confidence`: `Resolved` (runs whenever the caller runs the site),
//! `OverApprox` (may run: a candidate set, a condition, a rule above), `Open`.
//!
//! Identity: node `id`s are opaque and stable for the same input and engine
//! version (`r:`/`o:` + a 96-bit hash, `h:` readable). Join changed code by
//! location: a workspace node's `sourcePath` (relative to its app's source root,
//! `/`-separated) and `line`..`endLine` (1-based; `line` is the `procedure`/
//! `trigger` keyword or access modifier row, attributes excluded).
//!
//! Pruning: the output keeps every workspace routine and every node on a path
//! from a workspace routine to a workspace routine (the universal hub counts as
//! a caller of every node), plus every node with `hiddenCallers` that reaches a
//! workspace routine.

mod facts;
mod project;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

pub use facts::{ObjectFacts, RoutineFacts, RunTarget, SourceFacts, extract_source_facts};
use project::{Hub, Key};
pub use project::{Inputs, Projection, project};

use crate::program::graph::ProgramGraph;
use crate::program::graphify_export::object_kind_str;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId};
use crate::program::resolve::edge::{Edge as EngineEdge, Histogram};
use crate::program::resolve::full::ObligationId;
use crate::snapshot::TrustTier;

/// `meta.schema`.
pub const SCHEMA: &str = "al-sem.callgraph";
/// `meta.version`.
pub const VERSION: u32 = 1;

/// One `node` line.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_only: Option<bool>,
    /// `true` on a platform event publisher the engine synthesizes (no
    /// declaration, so no location).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_callers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub universal: Option<bool>,
}

/// One `edge` line.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub caller: String,
    pub callee: String,
    pub kind: &'static str,
    pub confidence: &'static str,
    pub via: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<&'static str>,
}

/// The export: `meta`, nodes and edges, ready to write.
pub struct ImpactGraph {
    pub meta: Value,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl ImpactGraph {
    /// The JSONL document (each line ends in `\n`).
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        fn line<T: Serialize>(out: &mut String, t: &str, v: &T) {
            out.push_str("{\"t\":\"");
            out.push_str(t);
            out.push_str("\",\"v\":");
            out.push_str(&serde_json::to_string(v).expect("export rows serialize"));
            out.push_str("}\n");
        }
        let mut out = String::new();
        line(&mut out, "meta", &self.meta);
        for n in &self.nodes {
            line(&mut out, "node", n);
        }
        for e in &self.edges {
            line(&mut out, "edge", e);
        }
        out
    }
}

/// Build the export for a workspace: the cross-app program
/// ([`crate::program::resolve::full::build_cross_app_program`]), its source
/// facts, the projection and `meta`.
pub fn export_workspace(workspace_root: &Path) -> Result<ImpactGraph, String> {
    let inputs = crate::program::inputs::InputSet::read(workspace_root, &Default::default());
    let (ctx, report, coverage) = crate::program::resolve::full::build_cross_app_program(&inputs)?;
    // The gate's identity rule (#73): `app.json` needs a non-empty string `id`.
    // Checked on the snapshot's own reading of it (`guid` is "" exactly when the
    // `id` is missing, not a string, or empty, `snapshot.rs:199-208`), so the id
    // checked is the id exported, with no second read. After the build, so a
    // missing or unparseable `app.json` keeps the build's `read`/`parse` error.
    if ctx.snapshot().workspace_app.guid.is_empty() {
        return Err(format!(
            "could not compute modelInstanceId for workspace '{}': app.json has no non-empty string id",
            workspace_root.display()
        ));
    }
    let graph = ctx.graph();
    let index = crate::program::resolve::index::ResolveIndex::build(graph);
    let facts = extract_source_facts(graph, &index, &ctx.all_units());
    let surface = ctx.decl_surface();
    let recovered: HashSet<String> = report.recovered_files.iter().cloned().collect();
    let projection = project(&Inputs {
        graph,
        index: &index,
        surface: &surface,
        primary: report.primary_app_ref,
        edges: &report.edges,
        site_facts: &report.site_facts,
        facts: &facts,
        missing_obligations: &report.coverage.missing,
        recovered_files: &recovered,
    });

    let dependency_edges: Vec<EngineEdge> = report
        .edges
        .iter()
        .filter(|ce| {
            ce.edge.from.object.app != report.primary_app_ref
                && matches!(ce.obligation_id, ObligationId::CallSite { .. })
        })
        .map(|ce| ce.edge.clone())
        .collect();
    let snap = ctx.snapshot();
    let ws = snap.apps.first();
    let apps = crate::program::resolve::full::loaded_apps(&ctx, &coverage);
    let meta = json!({
        "schema": SCHEMA,
        "version": VERSION,
        "engine": crate::engine::gate::version::driver_version(),
        "semantics": "may-execute",
        "workspace": {
            "appId": snap.workspace_app.guid.to_ascii_lowercase(),
            "name": snap.workspace_app.name,
            "publisher": snap.workspace_app.publisher,
            "version": snap.workspace_app.version,
            "runtime": ws.and_then(|u| u.compilation.runtime.clone()),
        },
        "apps": apps,
        "assumptions": ["opaqueCodeRunsNoObjectsById", "platformEntryPointsNotModelled"],
        "recoveredFiles": report.recovered_files,
        "coverageHolds": crate::program::resolve::full::coverage_holds(&report.coverage),
        "counts": {
            "nodes": projection.nodes.len(),
            "edges": projection.edges.len(),
            "hubs": projection.hubs,
            "universalCallers": projection.universal_callers,
        },
        "histogram": {
            "primaryScoped": histogram_json(&report.primary_histogram),
            "wholeProgram": histogram_json(&report.histogram),
            "dependencyBodies": histogram_json(&Histogram::of_edges(&dependency_edges)),
        },
    });
    if !crate::program::resolve::full::coverage_holds(&report.coverage) {
        eprintln!(
            "al-sem graph: warning: the coverage contract does not hold ({} obligations without an edge); \
             their routines are linked to the universal hub",
            report.coverage.missing.len()
        );
    }
    Ok(ImpactGraph {
        meta,
        nodes: projection.nodes,
        edges: projection.edges,
    })
}

fn histogram_json(h: &Histogram) -> Value {
    json!({
        "total": h.total,
        "resolvedSource": h.resolved_source,
        "resolvedCatalog": h.resolved_catalog,
        "resolvedAbiExternal": h.resolved_abi_external,
        "conditionalResolved": h.conditional_resolved,
        "honestDynamic": h.honest_dynamic,
        "honestEmpty": h.honest_empty,
        "unknown": h.unknown,
        "ambiguousResolved": h.ambiguous_resolved,
    })
}

/// The app part of an identity hash: guid and name (the guid can be empty).
fn app_identity(graph: &ProgramGraph, app: AppRef) -> (String, String) {
    let id = graph.apps.resolve(app);
    (id.guid.to_ascii_lowercase(), id.name.clone())
}

fn hash_id(prefix: &str, parts: &[&str]) -> String {
    let mut buf = String::new();
    for p in parts {
        crate::program::sig_fp::write_len_prefixed(&mut buf, p);
    }
    let h = blake3::hash(buf.as_bytes());
    let hex: String = h.as_bytes()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{prefix}{hex}")
}

fn object_parts(graph: &ProgramGraph, o: &ObjectNodeId) -> [String; 4] {
    let (guid, name) = app_identity(graph, o.app);
    let key = match &o.key {
        ObjKey::Id(n) => format!("#{n}"),
        ObjKey::Name(s) => format!("${s}"),
    };
    [guid, name, object_kind_str(o.kind).to_string(), key]
}

fn kind_lc(k: Option<al_syntax::ir::ObjectKind>) -> String {
    k.map_or_else(
        || "any".to_string(),
        |k| object_kind_str(k).to_ascii_lowercase(),
    )
}

/// A node's wire id (see the module doc).
pub(crate) fn key_id(key: &Key, graph: &ProgramGraph) -> String {
    match key {
        Key::Object(o) => {
            let p = object_parts(graph, o);
            hash_id("o:", &[&p[0], &p[1], &p[2], &p[3]])
        }
        Key::Routine(r) => {
            let p = object_parts(graph, &r.object);
            let member = r.enclosing_member_lc.as_deref().unwrap_or("");
            let params = r.params_count.to_string();
            let sig = r.sig_fp.to_string();
            hash_id(
                "r:",
                &[
                    &p[0], &p[1], &p[2], &p[3], member, &r.name_lc, &params, &sig,
                ],
            )
        }
        Key::Hub(h) => match h {
            Hub::Any => "h:any".to_string(),
            Hub::Name { name_lc, arity } => format!("h:name:{name_lc}/{arity}"),
            Hub::Run(k) => format!("h:run:{}", kind_lc(*k)),
            Hub::Record(op) => format!("h:record:{op}"),
            Hub::Callbacks(app) => {
                let (guid, name) = app_identity(graph, *app);
                format!("h:callbacks:{}", if guid.is_empty() { name } else { guid })
            }
        },
    }
}

/// A node row.
pub(crate) fn render_node(
    key: &Key,
    id: &str,
    inp: &Inputs<'_>,
    hidden: &HashMap<crate::program::node::RoutineNodeId, Vec<&'static str>>,
) -> Node {
    let graph = inp.graph;
    let blank = Node {
        id: id.to_string(),
        kind: "Hub",
        app_id: None,
        object_type: None,
        object_id: None,
        object_name: None,
        member_name: None,
        member: None,
        arity: None,
        source_path: None,
        line: None,
        end_line: None,
        dependency: None,
        symbol_only: None,
        synthetic: None,
        test: None,
        hidden_callers: Vec::new(),
        hub: None,
        hub_arg: None,
        universal: None,
    };
    let object_fields = |o: &ObjectNodeId, n: &mut Node| {
        let obj = graph
            .objects
            .binary_search_by(|x| x.id.cmp(o))
            .ok()
            .map(|i| &graph.objects[i]);
        n.app_id = Some(app_identity(graph, o.app).0);
        n.object_type = Some(object_kind_str(o.kind));
        n.object_id = obj.and_then(|x| x.declared_id).or(match o.key {
            ObjKey::Id(k) => Some(k),
            ObjKey::Name(_) => None,
        });
        n.object_name = Some(obj.map_or_else(
            || match &o.key {
                ObjKey::Id(k) => k.to_string(),
                ObjKey::Name(s) => s.to_string(),
            },
            |x| x.name.to_string(),
        ));
        n.dependency = Some(o.app != inp.primary);
        n.symbol_only = Some(obj.is_some_and(|x| x.tier == TrustTier::SymbolOnly));
    };
    match key {
        Key::Object(o) => {
            let mut n = Node {
                kind: "Object",
                ..blank
            };
            object_fields(o, &mut n);
            n
        }
        Key::Routine(r) => {
            let node = graph
                .routines
                .binary_search_by(|x| x.id.cmp(r))
                .ok()
                .map(|i| &graph.routines[i]);
            let mut n = Node {
                kind: if node.is_some_and(|x| x.is_trigger) {
                    "Trigger"
                } else {
                    "Method"
                },
                ..blank
            };
            object_fields(&r.object, &mut n);
            n.symbol_only = Some(node.is_some_and(|x| x.tier == TrustTier::SymbolOnly));
            n.arity = Some(r.params_count);
            n.member_name =
                Some(node.map_or_else(|| r.name_lc.to_string(), |x| x.name.to_string()));
            let meta = inp.surface.get_with_path(r);
            n.member = meta
                .and_then(|(m, _)| m.enclosing_member.as_ref().map(|s| s.to_string()))
                .or_else(|| r.enclosing_member_lc.as_ref().map(|s| s.to_string()));
            if let Some((m, path)) = meta
                && n.symbol_only != Some(true)
            {
                n.source_path = Some(path.to_string());
                n.line = Some(m.origin.start.row + 1);
                n.end_line = Some(m.origin.end.row + 1);
            }
            if node.is_some_and(|x| {
                x.publisher_kind == Some(crate::program::resolve::event::PublisherKind::Platform)
            }) {
                n.synthetic = Some(true);
            }
            if inp.facts.routines.get(r).is_some_and(|f| f.test) {
                n.test = Some(true);
            }
            n.hidden_callers = hidden.get(r).cloned().unwrap_or_default();
            n
        }
        Key::Hub(h) => {
            let (class, arg) = match h {
                Hub::Any => ("any", None),
                Hub::Name { name_lc, arity } => ("name", Some(format!("{name_lc}/{arity}"))),
                Hub::Run(k) => ("run", Some(kind_lc(*k))),
                Hub::Record(op) => ("record", Some((*op).to_string())),
                Hub::Callbacks(app) => ("callbacks", Some(app_identity(graph, *app).0)),
            };
            Node {
                hub: Some(class),
                hub_arg: arg,
                universal: matches!(h, Hub::Any).then_some(true),
                ..blank
            }
        }
    }
}
