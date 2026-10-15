//! #69 A5 — cone/L4 temp-class parity on a matched graph (design spec
//! `docs/superpowers/specs/2026-10-14-issue-69-digest-temp-state-coherence-design.md`,
//! "A5").
//!
//! The capability cone (`project_r3a3`) and the L4 db-effect store
//! (`build_db_effect_bundle`'s solver) answer the same question — "in r's own
//! frame, is this write temporary?" — through two independent propagations.
//! Both are TRANSITIVE populations: the producer solve no longer stops (D1.2),
//! and L4 unions terminal rows into every ancestor.
//!
//! **Matched graph.** L4 is rebuilt on the combined graph RESTRICTED to edges
//! that have a typed twin (same from, to, call site and event), because the cone
//! propagates over typed edges only. `implicit-trigger` and any other edge with
//! no typed counterpart is dropped and listed by kind. The restriction is
//! computed from the graphs, never from the cone result being tested.
//!
//! **Supported ops.** The capability write verbs after D5: Insert→insert;
//! Modify and ModifyAll→modify; Delete and DeleteAll→delete. Rename is excluded
//! on both sides.
//!
//! **Fixtures with param guards** compare with ⊆ (cone ⊆ L4): L4 has no guards.
//! The list is explicit: [`GUARDED`].
//!
//! Compare 2 (reduced) is here. Compare 1 (unreduced, per producer) reads the
//! solve's arena and lives in `r3_temp_class_parity_arena.rs`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use al_sem::engine::l4::capability_cone::{PCapabilityFact, project_r3a3};
use al_sem::engine::l4::combined_graph::{CombinedGraph, build_combined_graph};
use al_sem::engine::l4::effect_lattice::TempStateKind;
use al_sem::engine::l4::effect_store::SummaryBundle;
use al_sem::engine::l4::scc::{SccInputGraph, tarjan_scc};
use al_sem::engine::l4::summary_runner::{FieldIndex, compute_summaries_v2_bundle_with_leaves};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::program::model::workspace::Model;

use crate::temp_state_probe;

/// Probe routines with a usable Boolean guard on the path to a write: their
/// cone may drop a guarded-false state that L4 (no guards) keeps.
pub const GUARDED: &[&str] = &[
    "GuardFwd",
    "GuardFwdFalse",
    "GuardFwdTrue",
    "GCyc1",
    "GCyc2",
    "GCycFalse",
    "GCycTrue",
    "GT1",
    "GT2",
    "GTFalse",
    "GTTrue",
    "CondTwice",
    "HelperTwoUi",
    "IncWrap",
];

pub fn corpus_fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    out.sort();
    out
}

/// L4's write verb for a raw AL op; `None` for everything A5 does not compare.
pub fn l4_verb(op: &str) -> Option<&'static str> {
    match op {
        "Insert" => Some("insert"),
        "Modify" | "ModifyAll" => Some("modify"),
        "Delete" | "DeleteAll" => Some("delete"),
        _ => None,
    }
}

/// One spelling for a table id: the cone says `app:Table:50300`, L4 says
/// `app/table/50300`; an unresolved table is no id on the cone side and
/// `unknown` on L4's.
pub fn table_key(id: &str) -> String {
    if id.is_empty() {
        return "unknown".to_string();
    }
    id.to_lowercase().replace(':', "/")
}

pub fn class_of(t: &TempStateKind) -> &'static str {
    if *t == TempStateKind::Known(true) {
        "kt"
    } else {
        "nt"
    }
}

fn projected_class(f: &PCapabilityFact) -> &'static str {
    let kt = f
        .extra
        .as_ref()
        .and_then(|e| e.get("tempState"))
        .is_some_and(|ts| {
            ts.get("kind").and_then(|k| k.as_str()) == Some("known")
                && ts.get("value").and_then(|v| v.as_bool()) == Some(true)
        });
    if kt { "kt" } else { "nt" }
}

/// L4 built on the matched graph: the model's combined graph restricted to the
/// edges with a typed twin. Returns the bundle and the dropped edges by kind.
pub fn matched_l4(resolved: &Model) -> (SummaryBundle, BTreeMap<String, usize>) {
    let ws = &resolved.workspace;
    let full = build_combined_graph(ws, &resolved.calls, &resolved.events.graph);
    let twins: HashSet<(String, String, Option<String>, Option<String>)> = full
        .typed_edges
        .iter()
        .filter_map(|t| {
            Some((
                t.from.clone(),
                t.to.clone()?,
                t.callsite_id.clone(),
                t.event_id.clone(),
            ))
        })
        .collect();
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    let mut edges_by_from = HashMap::new();
    for (from, list) in &full.edges_by_from {
        let kept: Vec<_> = list
            .iter()
            .filter(|e| {
                let key = (
                    e.from.clone(),
                    e.to.clone(),
                    e.callsite_id.clone(),
                    e.event_id.clone(),
                );
                let keep = twins.contains(&key);
                if !keep {
                    *dropped.entry(e.kind.clone()).or_default() += 1;
                }
                keep
            })
            .cloned()
            .collect();
        edges_by_from.insert(from.clone(), kept);
    }
    let graph = CombinedGraph {
        edges_by_from,
        ..full
    };
    let adjacency: HashMap<String, Vec<String>> = graph
        .edges_by_from
        .iter()
        .map(|(f, l)| (f.clone(), l.iter().map(|e| e.to.clone()).collect()))
        .collect();
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &adjacency,
    });
    let mut fields: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            fields
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }
    let (bundle, _, _) = compute_summaries_v2_bundle_with_leaves(
        &ws.routines,
        &graph,
        &scc,
        &resolved.calls.upgraded_bindings,
        &fields,
        &HashMap::new(),
        &std::collections::HashSet::new(),
    );
    (bundle, dropped)
}

/// routine name for a stable routine id (the id `project_r3a3` reports).
pub fn display_by_id(resolved: &Model) -> HashMap<String, String> {
    resolved
        .workspace
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.clone(), r.name.clone()))
        .collect()
}

/// Compare 2 over one workspace: for every routine r and every
/// `(verb, table)`, the classes of r's direct AND inherited write facts equal
/// (⊆ for [`GUARDED`] routines) the classes of L4's rows for that key at r.
fn compare_reduced(label: &str, resolved: &Model, guarded: &[&str], f: &mut Vec<String>) {
    let (bundle, dropped) = matched_l4(resolved);
    eprintln!("[{label}] A5 matched graph: dropped edges by kind {dropped:?}");
    let names = display_by_id(resolved);
    type Key = (String, String); // (verb, table)
    let mut cone: BTreeMap<String, BTreeMap<Key, BTreeSet<&str>>> = BTreeMap::new();
    for s in project_r3a3(resolved).summaries {
        let per = cone.entry(s.routine_id.clone()).or_default();
        for fact in s
            .capability_facts_direct
            .iter()
            .chain(&s.capability_facts_inherited)
        {
            if fact.resource_kind != "table"
                || !matches!(fact.op.as_str(), "insert" | "modify" | "delete")
            {
                continue;
            }
            let table = table_key(fact.resource_id.as_deref().unwrap_or(""));
            per.entry((fact.op.clone(), table))
                .or_default()
                .insert(projected_class(fact));
        }
    }
    let mut l4: BTreeMap<String, BTreeMap<Key, BTreeSet<&str>>> = BTreeMap::new();
    for r in &resolved.workspace.routines {
        let Some(ix) = bundle.routine_ix(&r.id) else {
            continue;
        };
        for row in bundle.db_effects(ix) {
            let Some(verb) = l4_verb(row.op) else {
                continue;
            };
            l4.entry(r.stable_routine_id.clone())
                .or_default()
                .entry((verb.to_string(), table_key(row.table_id)))
                .or_default()
                .insert(class_of(row.temp_state));
        }
    }
    let routines: BTreeSet<&String> = cone.keys().chain(l4.keys()).collect();
    for rid in routines {
        let name = names.get(rid).map(String::as_str).unwrap_or(rid);
        let empty = BTreeMap::new();
        let c = cone.get(rid).unwrap_or(&empty);
        let l = l4.get(rid).unwrap_or(&empty);
        let keys: BTreeSet<&Key> = c.keys().chain(l.keys()).collect();
        for k in keys {
            let none = BTreeSet::new();
            let cs = c.get(k).unwrap_or(&none);
            let ls = l.get(k).unwrap_or(&none);
            let ok = if guarded.contains(&name) {
                cs.is_subset(ls)
            } else {
                cs == ls
            };
            if !ok {
                f.push(format!(
                    "[{label}] Compare 2: {name} {}|{} cone {cs:?} != L4 {ls:?}",
                    k.0, k.1
                ));
            }
        }
    }
}

fn resolve(dir: &Path) -> Option<Model> {
    assemble_and_resolve_workspace_with_program_calls(dir)
}

/// A5 Compare 2 (reduced), on every `tests/r0-corpus` fixture and the probe.
/// Mutation (spec): skip transitions across intra-SCC edges in the solve →
/// Compare 1's "RecUi2 ∅ != L4 {nt}" (Compare 1 is the arena half).
#[test]
fn a5_cone_l4_class_parity_reduced() {
    let mut f = Vec::new();
    let tmp = temp_state_probe::write_workspace();
    let probe = resolve(tmp.path()).expect("probe model");
    compare_reduced("probe", &probe, GUARDED, &mut f);
    for dir in corpus_fixtures() {
        let label = dir.file_name().unwrap().to_string_lossy().to_string();
        if let Some(m) = resolve(&dir) {
            compare_reduced(&label, &m, &[], &mut f);
        }
    }
    assert!(f.is_empty(), "{} mismatches:\n{}", f.len(), f.join("\n"));
}
