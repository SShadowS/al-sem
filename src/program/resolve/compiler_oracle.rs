//! The AL compiler's call graph as an independent oracle (engine-switch S9.0).
//!
//! The AL extension ships `altool graph` (Microsoft.BusinessCentral.CallGraph):
//! `altool graph extract-whole --corpus <ws> --corpus <dep sources> --graph g.jsonl`
//! compiles every app together and writes the compiler's own call graph, one JSON
//! row per line: `node` rows (methods, triggers, objects, built-in methods) and
//! `edge` rows (`caller`, `callee`, `kind` = Direct/Trigger/Event/Interface,
//! `confidence` = Resolved/OverApprox, `sourcePath`, 1-based `line`). This module
//! reads that file and lines it up against the program resolver's edges, call
//! site by call site, so the two engines can be compared. It never feeds the
//! product: it is a test reference only.
//!
//! Granularity: a SITE is (caller routine, line). The compiler graph has no
//! column or callee text, and the semantic goldens already ignore columns. A
//! TARGET is (app guid, object key, routine name), all lower-case; the object key
//! is the object number when it has one, else its name (as
//! [`differential::project_fresh`] spells it). Built-in methods are not compared:
//! the two engines name them differently, and the compiler's built-in rows carry
//! no app.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufRead;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::program::resolve::differential::{CanonicalEdge, CanonicalKey};
use crate::program::resolve::edge::EdgeKind;

/// One compiler graph row (only the fields this module reads).
#[derive(Deserialize)]
struct Row {
    t: String,
    v: serde_json::Value,
}

/// A compiler node, reduced to the identity this module compares on.
#[derive(Clone, Debug)]
struct CNode {
    kind: String,
    app: String,
    object_type: String,
    object_key: String,
    member: Option<String>,
    line: u32,
}

/// The edge classes both engines share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EdgeClass {
    /// Direct calls, method calls, interface dispatch, object runs.
    Call,
    /// Object runs (`Page.Run`, `Codeunit.Run`, ...). The compiler graph has no
    /// object-run edges (`Page.RunModal` is a call to the built-in `Page.RunModal`
    /// with no target object), so these are reported, never compared.
    Run,
    /// Implicit table triggers (`Rec.Insert(true)` -> `OnInsert`).
    Trigger,
    /// Event publisher -> subscriber.
    Event,
}

/// A routine as both engines can name it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Routine {
    pub app: String,
    pub object_kind: String,
    pub object_key: String,
    pub routine: String,
}

/// A call site: the calling routine and the 1-based source line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Site {
    pub caller: Routine,
    pub line: u32,
    pub class: EdgeClass,
}

/// A routine target, without the object kind (dependency ABI targets do not
/// carry it on the program side).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Target {
    pub app: String,
    pub object_key: String,
    pub routine: String,
}

/// The compiler graph, as site -> targets.
#[derive(Default)]
pub struct CompilerGraph {
    pub sites: BTreeMap<Site, BTreeSet<Target>>,
    /// Edges the reader could not map (unknown node ids, module pseudo-objects).
    pub unmapped_edges: usize,
    /// Edges per compiler (kind, confidence), for the report.
    pub edge_kinds: BTreeMap<String, usize>,
}

fn s(v: &serde_json::Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_string)
}

/// Parse a node id `al . <guid> / <Type> <id> <Name> # <member>#<hash>`.
fn parse_node(v: &serde_json::Value) -> Option<(String, CNode)> {
    let id = s(v, "id")?;
    let rest = id.strip_prefix("al . ")?;
    let (app, rest) = rest.split_once(" / ")?;
    let object_id = v.get("objectId").and_then(|x| x.as_i64()).unwrap_or(0);
    let object_type = rest.split(' ').next()?.to_ascii_lowercase();
    let object_name = s(v, "objectName").unwrap_or_default();
    let object_key = if object_id != 0 {
        object_id.to_string()
    } else {
        object_name.to_ascii_lowercase()
    };
    Some((
        id.clone(),
        CNode {
            kind: s(v, "kind").unwrap_or_default(),
            app: app.to_ascii_lowercase(),
            object_type,
            object_key,
            member: s(v, "memberName").map(|m| m.to_ascii_lowercase()),
            line: v.get("line").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        },
    ))
}

fn routine_of(n: &CNode) -> Option<Routine> {
    if !matches!(n.kind.as_str(), "Method" | "Trigger") {
        return None;
    }
    if n.app == "00000000-0000-0000-0000-000000000000" {
        return None; // built-in methods and the corpus pseudo-module
    }
    Some(Routine {
        app: n.app.clone(),
        object_kind: n.object_type.clone(),
        object_key: n.object_key.clone(),
        routine: n.member.clone()?,
    })
}

impl CompilerGraph {
    /// Read a stitched `altool graph` file.
    ///
    /// # Errors
    /// The file cannot be opened, or a line is not valid JSON.
    pub fn read(path: &Path) -> Result<Self, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut nodes: HashMap<String, CNode> = HashMap::new();
        let mut edges: Vec<serde_json::Value> = Vec::new();
        for line in std::io::BufReader::new(f).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            let row: Row = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            match row.t.as_str() {
                "node" => {
                    if let Some((id, n)) = parse_node(&row.v) {
                        nodes.insert(id, n);
                    }
                }
                "edge" => edges.push(row.v),
                _ => {}
            }
        }
        let mut g = CompilerGraph::default();
        for e in edges {
            let kind = s(&e, "kind").unwrap_or_default();
            let conf = s(&e, "confidence").unwrap_or_default();
            *g.edge_kinds.entry(format!("{kind}/{conf}")).or_default() += 1;
            let (Some(caller), Some(callee)) = (
                s(&e, "caller").and_then(|c| nodes.get(&c)),
                s(&e, "callee").and_then(|c| nodes.get(&c)),
            ) else {
                g.unmapped_edges += 1;
                continue;
            };
            let line = e.get("line").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            // The compiler emits a self-edge at each routine's declaration line.
            if std::ptr::eq(caller, callee) && line == caller.line {
                continue;
            }
            let class = match kind.as_str() {
                "Trigger" => EdgeClass::Trigger,
                "Event" => EdgeClass::Event,
                _ => EdgeClass::Call,
            };
            // An interface DECLARATION's member is no caller: its `Interface`
            // edges to the implementations restate the call sites' edges.
            if caller.object_type == "interface" {
                continue;
            }
            let Some(caller_r) = routine_of(caller) else {
                g.unmapped_edges += 1;
                continue;
            };
            let entry = g
                .sites
                .entry(Site {
                    caller: caller_r,
                    line,
                    class,
                })
                .or_default();
            // A call to an interface member: the compiler also emits one
            // `Interface` edge per implementation, which is what the program
            // resolver reports; the interface declaration itself is no target.
            if callee.object_type == "interface" {
                continue;
            }
            if let Some(t) = routine_of(callee) {
                entry.insert(Target {
                    app: t.app,
                    object_key: t.object_key,
                    routine: t.routine,
                });
            }
        }
        // Sites whose only callees were built-ins have no target to compare.
        g.sites.retain(|_, t| !t.is_empty());
        Ok(g)
    }
}

/// Instance runs (`CU.Run()`, `MyPage.RunModal()`) resolve as `Call` edges to the
/// target object's entry trigger; a trigger is never directly callable in AL, so
/// a `Call` whose every routine target is a trigger is an object run. Reclassify
/// them as `Run` so they line up with the static `Page.Run(...)` form.
pub fn reclassify_entry_runs(
    edges: &mut [crate::program::resolve::edge::Edge],
    graph: &crate::program::graph::ProgramGraph,
) {
    use crate::program::resolve::edge::RouteTarget;
    let triggers: std::collections::HashSet<&crate::program::node::RoutineNodeId> = graph
        .routines
        .iter()
        .filter(|r| r.is_trigger)
        .map(|r| &r.id)
        .collect();
    for e in edges.iter_mut() {
        if e.kind != EdgeKind::Call {
            continue;
        }
        let is_run = {
            let mut routines = e
                .all_routes()
                .filter_map(|r| match &r.target {
                    RouteTarget::Routine(id) => Some(id),
                    _ => None,
                })
                .peekable();
            routines.peek().is_some() && routines.all(|id| triggers.contains(id))
        };
        if is_run {
            e.kind = EdgeKind::Run;
        }
    }
}

/// A program routine as the compiler graph names a caller.
#[must_use]
pub fn routine_of_node(
    id: &crate::program::node::RoutineNodeId,
    apps: &crate::program::node::AppRegistry,
) -> Routine {
    key_routine(&crate::program::resolve::differential::routine_to_key(
        id, apps,
    ))
}

/// A program routine as the compiler graph names a callee.
#[must_use]
pub fn target_of_node(
    id: &crate::program::node::RoutineNodeId,
    apps: &crate::program::node::AppRegistry,
) -> Target {
    let r = routine_of_node(id, apps);
    Target {
        app: r.app,
        object_key: r.object_key,
        routine: r.routine,
    }
}

/// Every (caller, class, callee) pair with the first line it occurs on, the
/// granularity of the compiler graph (see [`pairs`]); self-recursion is left
/// out, as there.
#[must_use]
pub fn pair_lines(
    sites: &BTreeMap<Site, BTreeSet<Target>>,
) -> BTreeMap<(Routine, EdgeClass, Target), u32> {
    let mut out: BTreeMap<(Routine, EdgeClass, Target), u32> = BTreeMap::new();
    for (site, targets) in sites {
        let c = &site.caller;
        for t in targets {
            if t.app == c.app && t.object_key == c.object_key && t.routine == c.routine {
                continue;
            }
            let line = out
                .entry((c.clone(), site.class, t.clone()))
                .or_insert(site.line);
            *line = (*line).min(site.line);
        }
    }
    out
}

fn key_routine(k: &CanonicalKey) -> Routine {
    Routine {
        app: k.app_guid.to_ascii_lowercase(),
        object_kind: k.object_kind.clone(),
        object_key: k.object_lc.clone(),
        routine: k.routine_lc.clone(),
    }
}

/// The program resolver's edges as site -> targets. Program lines are 0-based;
/// the compiler's are 1-based, so each line is shifted by one.
#[must_use]
pub fn program_sites(edges: &[CanonicalEdge]) -> BTreeMap<Site, BTreeSet<Target>> {
    let mut out: BTreeMap<Site, BTreeSet<Target>> = BTreeMap::new();
    for e in edges {
        let class = match e.kind {
            EdgeKind::Call => EdgeClass::Call,
            EdgeKind::Run => EdgeClass::Run,
            EdgeKind::ImplicitTrigger => EdgeClass::Trigger,
            EdgeKind::EventFlow => EdgeClass::Event,
        };
        let targets: BTreeSet<Target> = e
            .targets
            .iter()
            .filter(|t| t.kind != 255)
            .filter_map(|t| {
                Some(Target {
                    app: t.app.clone()?.to_ascii_lowercase(),
                    object_key: t.object_lc.clone(),
                    routine: t.routine_lc.clone()?,
                })
            })
            .collect();
        if targets.is_empty() {
            continue;
        }
        out.entry(Site {
            caller: key_routine(&e.site.caller),
            line: e.site.span.start.line + 1,
            class,
        })
        .or_default()
        .extend(targets);
    }
    out
}

/// Collapse sites to caller -> targets per edge class. The compiler graph keeps
/// ONE edge per (caller, callee) pair, at the first line it occurs, so a
/// line-level comparison would report every later call to the same callee as
/// missing; pairs are the granularity both engines share.
#[must_use]
pub fn pairs(
    sites: &BTreeMap<Site, BTreeSet<Target>>,
) -> BTreeMap<(Routine, EdgeClass), BTreeSet<Target>> {
    let mut out: BTreeMap<(Routine, EdgeClass), BTreeSet<Target>> = BTreeMap::new();
    for (site, targets) in sites {
        let c = &site.caller;
        // Self-recursion: the compiler folds a routine's call to itself into the
        // declaration-line self-edge it emits for every routine, so it is not
        // comparable.
        let targets = targets.iter().filter(|t| {
            !(t.app == c.app && t.object_key == c.object_key && t.routine == c.routine)
        });
        out.entry((c.clone(), site.class))
            .or_default()
            .extend(targets.cloned());
    }
    out.retain(|_, t| !t.is_empty());
    out
}

/// One caller whose callee set differs between the engines, for triage.
#[derive(Clone, Debug, Serialize)]
pub struct Disagreement {
    pub caller: Routine,
    pub class: EdgeClass,
    /// Callees only the compiler reports.
    pub compiler_only: BTreeSet<Target>,
    /// Callees only the program resolver reports.
    pub program_only: BTreeSet<Target>,
}

/// The pair-level comparison of both engines.
#[derive(Debug, Default, Serialize)]
pub struct OracleReport {
    pub callers: usize,
    pub pairs_agree: usize,
    pub pairs_compiler_only: usize,
    pub pairs_program_only: usize,
    pub disagreements: Vec<Disagreement>,
}

/// Compare caller -> callee pairs, keeping only callers whose app is in `apps`
/// (lower-case guids; empty = every app).
#[must_use]
pub fn compare(
    compiler: &BTreeMap<Site, BTreeSet<Target>>,
    program: &BTreeMap<Site, BTreeSet<Target>>,
    apps: &BTreeSet<String>,
) -> OracleReport {
    let c = pairs(compiler);
    let p = pairs(program);
    let in_scope = |k: &(Routine, EdgeClass)| apps.is_empty() || apps.contains(&k.0.app);
    let mut r = OracleReport::default();
    let empty = BTreeSet::new();
    let keys: BTreeSet<&(Routine, EdgeClass)> =
        c.keys().chain(p.keys()).filter(|k| in_scope(k)).collect();
    for k in keys {
        r.callers += 1;
        let ct = c.get(k).unwrap_or(&empty);
        let pt = p.get(k).unwrap_or(&empty);
        let both = ct.intersection(pt).count();
        r.pairs_agree += both;
        let co: BTreeSet<Target> = ct.difference(pt).cloned().collect();
        let po: BTreeSet<Target> = pt.difference(ct).cloned().collect();
        r.pairs_compiler_only += co.len();
        r.pairs_program_only += po.len();
        if !co.is_empty() || !po.is_empty() {
            r.disagreements.push(Disagreement {
                caller: k.0.clone(),
                class: k.1,
                compiler_only: co,
                program_only: po,
            });
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(lines: &[&str]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f
    }

    const APP: &str = "11111111-0000-0000-0000-000000000001";

    fn node(obj: &str, ty: &str, id: i64, member: &str, line: u32) -> String {
        format!(
            r#"{{"t":"node","v":{{"id":"al . {APP} / {ty} {id} {obj} # {member}#1","kind":"Method","objectName":"{obj}","objectId":{id},"memberName":"{member}","line":{line}}}}}"#
        )
    }

    fn edge(from: &str, to: &str, line: u32, kind: &str) -> String {
        format!(
            r#"{{"t":"edge","v":{{"caller":"al . {APP} / {from}#1","callee":"al . {APP} / {to}#1","kind":"{kind}","confidence":"Resolved","line":{line}}}}}"#
        )
    }

    /// The reader keeps a real call, and drops the compiler's declaration-line
    /// self-edge, calls to an interface declaration, edges whose caller is an
    /// interface declaration, and built-in callees.
    ///
    /// Discrimination (2026-10-07): removing the interface-caller skip, the
    /// interface-callee skip, or the declaration self-edge skip each fails this
    /// test; restored, it passes.
    #[test]
    fn the_reader_keeps_calls_and_drops_what_is_not_comparable() {
        let f = write(&[
            &node("A", "Codeunit", 50100, "Run2", 3),
            &node("A", "Codeunit", 50100, "Helper", 9),
            &node("I", "Interface", 0, "M", 2),
            r#"{"t":"node","v":{"id":"al . 00000000-0000-0000-0000-000000000000 / BuiltIn 0 Table # Get","kind":"BuiltInMethod","objectName":"Table","objectId":0,"memberName":"Get"}}"#,
            &edge(
                "Codeunit 50100 A # Run2",
                "Codeunit 50100 A # Run2",
                3,
                "Direct",
            ),
            &edge(
                "Codeunit 50100 A # Run2",
                "Codeunit 50100 A # Helper",
                5,
                "Direct",
            ),
            &edge("Codeunit 50100 A # Run2", "Interface 0 I # M", 6, "Direct"),
            &edge(
                "Interface 0 I # M",
                "Codeunit 50100 A # Helper",
                2,
                "Interface",
            ),
        ]);
        let g = CompilerGraph::read(f.path()).unwrap();
        let sites: Vec<(u32, Vec<String>)> = g
            .sites
            .iter()
            .map(|(s, t)| (s.line, t.iter().map(|t| t.routine.clone()).collect()))
            .collect();
        assert_eq!(sites, vec![(5, vec!["helper".to_string()])]);
    }

    fn r(name: &str) -> Routine {
        Routine {
            app: APP.into(),
            object_kind: "codeunit".into(),
            object_key: "50100".into(),
            routine: name.into(),
        }
    }

    fn t(name: &str) -> Target {
        Target {
            app: APP.into(),
            object_key: "50100".into(),
            routine: name.into(),
        }
    }

    /// Pairs, not lines: the compiler keeps one edge per (caller, callee) pair,
    /// so a second call to the same callee on another line is no disagreement;
    /// a callee only one side has is; self-recursion is ignored.
    ///
    /// Discrimination (2026-10-07): keeping self pairs in `pairs` fails this test;
    /// restored, it passes.
    #[test]
    fn compare_is_per_caller_callee_pair() {
        let site = |line, class| Site {
            caller: r("run2"),
            line,
            class,
        };
        let compiler: BTreeMap<Site, BTreeSet<Target>> =
            [(site(5, EdgeClass::Call), [t("helper"), t("other")].into())].into();
        let program: BTreeMap<Site, BTreeSet<Target>> = [
            (site(5, EdgeClass::Call), [t("helper")].into()),
            (
                site(8, EdgeClass::Call),
                [t("helper"), t("run2"), t("extra")].into(),
            ),
        ]
        .into();
        let rep = compare(&compiler, &program, &BTreeSet::new());
        assert_eq!(rep.pairs_agree, 1);
        assert_eq!(rep.pairs_compiler_only, 1);
        assert_eq!(rep.pairs_program_only, 1);
        let d = &rep.disagreements[0];
        assert_eq!(d.compiler_only, [t("other")].into());
        assert_eq!(d.program_only, [t("extra")].into());
    }
}
