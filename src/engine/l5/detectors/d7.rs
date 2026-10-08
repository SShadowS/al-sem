//! D7 — recursive event expansion. Port of al-sem
//! `src/detectors/d7-recursive-event-expansion.ts`.
//!
//! Tarjan SCC over the COMBINED graph (`ctx.graph.edges_by_from`, following `e.to`).
//! Keep an SCC of size >= 2 IFF it has an `event-dispatch` edge WITHIN the SCC AND a
//! primary member. Reports an event-subscriber cycle that can recurse unboundedly.
//!
//! ## Tarjan: inline port, NOT scc.rs reuse
//! d7 ports its OWN recursive Tarjan over `edgesByFrom` (the FULL combined-edge set,
//! every kind). The al-sem inline runs over the same edge set and `tarjanSCC` returns
//! every SCC (size 1 included); d7 then filters `length >= 2`. Reusing l4 `scc.rs`
//! would run over a DIFFERENT adjacency (it pre-derives `to`-only lists, identical
//! here) — but the inline port removes the divergence risk entirely, and the SCC
//! grouping is invariant under recursive-vs-iterative since node + edge iteration
//! order is the same. Members are sorted by the INTERNAL RoutineId string
//! (`[...scc].sort()`), which is what `id = d7/{members.join(",")}` keys on; the
//! projection then rewrites each internal id to stable form.
//!
//! `candidatesConsidered = (sccs of size>=2).length`. confidence
//! `to_confidence(&[], "likely")`. severity high.

use std::collections::{HashMap, HashSet};

use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PAnchor;
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d7-recursive-event-expansion";

/// State for the recursive Tarjan SCC (faithful port of d7's inline `tarjanSCC`).
struct Tarjan<'a> {
    edges_by_from: &'a HashMap<String, Vec<CombinedEdge>>,
    index: usize,
    indices: HashMap<String, usize>,
    lowlink: HashMap<String, usize>,
    on_stack: HashSet<String>,
    stack: Vec<String>,
    sccs: Vec<Vec<String>>,
}

impl<'a> Tarjan<'a> {
    fn strongconnect(&mut self, v: &str) {
        self.indices.insert(v.to_string(), self.index);
        self.lowlink.insert(v.to_string(), self.index);
        self.index += 1;
        self.stack.push(v.to_string());
        self.on_stack.insert(v.to_string());

        let empty: Vec<CombinedEdge> = Vec::new();
        // Clone the `to` targets up-front so we don't hold a borrow of `edges_by_from`
        // across the recursive call (the original iterates `graph.edgesByFrom.get(v)`).
        let tos: Vec<String> = self
            .edges_by_from
            .get(v)
            .unwrap_or(&empty)
            .iter()
            .map(|e| e.to.clone())
            .collect();
        for w in &tos {
            if !self.indices.contains_key(w) {
                self.strongconnect(w);
                let lvl = *self.lowlink.get(v).unwrap_or(&0);
                let lwl = *self.lowlink.get(w).unwrap_or(&0);
                self.lowlink.insert(v.to_string(), lvl.min(lwl));
            } else if self.on_stack.contains(w) {
                let lvl = *self.lowlink.get(v).unwrap_or(&0);
                let iwl = *self.indices.get(w).unwrap_or(&0);
                self.lowlink.insert(v.to_string(), lvl.min(iwl));
            }
        }

        if self.lowlink.get(v) == self.indices.get(v) {
            let mut scc: Vec<String> = Vec::new();
            while let Some(w) = self.stack.pop() {
                self.on_stack.remove(&w);
                let is_root = w == v;
                scc.push(w);
                if is_root {
                    break;
                }
            }
            self.sccs.push(scc);
        }
    }
}

/// Run d7's inline Tarjan over the combined graph's `nodes` + `edges_by_from`.
fn tarjan_scc(
    nodes: &[String],
    edges_by_from: &HashMap<String, Vec<CombinedEdge>>,
) -> Vec<Vec<String>> {
    let mut t = Tarjan {
        edges_by_from,
        index: 0,
        indices: HashMap::new(),
        lowlink: HashMap::new(),
        on_stack: HashSet::new(),
        stack: Vec::new(),
        sccs: Vec::new(),
    };
    for v in nodes {
        if !t.indices.contains_key(v) {
            t.strongconnect(v);
        }
    }
    t.sccs
}

/// The fallback anchor for a routine missing from `routine_by_id` — al-sem's
/// `{ sourceUnitId: "", range: 0s, enclosingRoutineId: id, syntaxKind: "routine" }`.
fn fallback_anchor(id: &str) -> SourceAnchor {
    SourceAnchor {
        source_unit_id: String::new(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        enclosing_routine_id: id.to_string(),
        syntax_kind: "routine".to_string(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    }
}

/// Build a routine's own `SourceAnchor` (`routine.sourceAnchor`) in internal form.
fn routine_anchor(a: &PAnchor, routine: &ModelRoutine) -> SourceAnchor {
    super::anchor_of(a, routine)
}

pub fn detect_d7(
    _resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;

    let graph = &ctx.graph;
    let sccs: Vec<Vec<String>> = tarjan_scc(&graph.nodes, &graph.edges_by_from)
        .into_iter()
        .filter(|scc| scc.len() >= 2)
        .collect();

    let candidates_considered = sccs.len();
    let mut findings: Vec<Finding> = Vec::new();

    let empty: Vec<CombinedEdge> = Vec::new();

    for scc in &sccs {
        let in_set: HashSet<&str> = scc.iter().map(|s| s.as_str()).collect();
        let has_event_edge = scc.iter().any(|from| {
            graph
                .edges_by_from
                .get(from)
                .unwrap_or(&empty)
                .iter()
                .any(|e| e.kind == "event-dispatch" && in_set.contains(e.to.as_str()))
        });
        if !has_event_edge {
            continue;
        }
        // roleOf(routine) === "primary": source-only ⇒ any indexed routine is
        // primary. The SCC contains only combined-graph nodes (workspace routines),
        // so a member that resolves in routine_by_id is primary.
        let has_primary = scc
            .iter()
            .any(|id| ctx.routine_by_id.contains_key(id.as_str()));
        if !has_primary {
            continue;
        }

        // `[...scc].sort()` — sort by the INTERNAL RoutineId string.
        let mut sorted_scc: Vec<String> = scc.clone();
        sorted_scc.sort();

        let path: Vec<EvidenceStep> = sorted_scc
            .iter()
            .map(|id| {
                let routine = ctx.routine_by_id.get(id.as_str()).copied();
                let (anchor, note) = match routine {
                    Some(r) => (
                        routine_anchor(&r.source_anchor, r),
                        format!("participant: {}", r.name),
                    ),
                    None => (fallback_anchor(id), format!("participant: {id}")),
                };
                EvidenceStep {
                    routine_id: id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor,
                    note,
                }
            })
            .collect();

        let Some(first_id) = sorted_scc.first() else {
            continue;
        };
        let Some(anchor_routine) = ctx.routine_by_id.get(first_id.as_str()).copied() else {
            continue;
        };

        // rootCause: "Routines A → B → ... → first.name form an event-dispatch cycle ..."
        let chain: Vec<String> = sorted_scc
            .iter()
            .map(|id| {
                ctx.routine_by_id
                    .get(id.as_str())
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .collect();
        let root_cause = format!(
            "Routines {} → {} form an event-dispatch cycle — invoking any of them at runtime can \
             trigger unbounded recursion.",
            chain.join(" → "),
            anchor_routine.name
        );

        // affectedObjects: sorted-deduped member objectIds (over the SORTED scc).
        let mut affected_objects: Vec<String> = Vec::new();
        {
            let mut seen: HashSet<String> = HashSet::new();
            for id in &sorted_scc {
                if let Some(r) = ctx.routine_by_id.get(id.as_str())
                    && seen.insert(r.object_id.clone())
                {
                    affected_objects.push(r.object_id.clone());
                }
            }
            affected_objects.sort();
        }

        let confidence: FindingConfidence = to_confidence(&[], "likely");

        let id = format!("d7/{}", sorted_scc.join(","));
        let root_cause_key = id.clone();

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: "Event subscriber chain forms a cycle".into(),
            root_cause,
            severity: "high".to_string(),
            confidence,
            primary_location: routine_anchor(&anchor_routine.source_anchor, anchor_routine),
            evidence_path: path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description:
                    "Break the cycle: either remove one of the event publishes from a subscriber, \
                     or gate the publish on a 'currently-processing' flag."
                        .into(),
                safety: "low".into(),
            }],
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        };
        finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
        findings.push(finding);
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    Ok(DetectorOutput {
        findings,
        stats: DetectorStats::new(DETECTOR, candidates_considered, emitted),
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
