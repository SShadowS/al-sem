//! D2 — an event raised INSIDE a loop whose subscribers touch the database. Port
//! of al-sem `src/detectors/d2-event-fanout-in-loop.ts`. The complement of d1: d1
//! SKIPS event-dispatch edges; d2 is the detector that HANDLES them.
//!
//! Detection (NOT a single walk from the loop): for each primary routine, for each
//! in-loop callsite that resolves to an event-PUBLISHER routine, enumerate the
//! publisher's event-dispatch SUBSCRIBER edges. Each subscriber whose summary
//! `touchesDb != "no"` is a witness; a supplementary `walk_evidence` from the
//! subscriber locates the exact DB op site (the same PW-0 substrate d1 uses, with a
//! D1-shaped policy: terminals = the routine's db-touching record ops, expand over
//! non-event-dispatch db-reaching edges).
//!
//! Two keys (mirrors d1):
//!   - `id` = `d2/{loopId}/{eventId}` — drops within-walker duplicates (same loop
//!     publishes same event from two callsites).
//!   - `rootCauseKey` = `d2/{eventId}` — `merge_by_terminal` folds M different loops
//!     publishing the SAME event into ONE finding with the others in
//!     `additionalPaths`.
//!
//! Two-stage collapse: dedup by `id` (first-wins) → `merge_by_terminal`. Fingerprint
//! computed AFTER merge (affectedObjects/affectedTables are unioned across paths),
//! then sort by `id`.
//!
//! d2 does NOT set `eventKind` / `crossExtensionSubscribers` (al-sem leaves them
//! unset) — they stay `None`.
//!
//! ## Source-only role path
//! `roleOf(routine) === "primary"` holds for every routine in the SOURCE-ONLY Rust
//! pipeline, so the role gate is a no-op pass (mirrors d1).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l4::cone_derived::ConeDerivedStore;
use crate::engine::l4::summary::{Uncertainty, dedupe_uncertainties};
use crate::engine::l5::capability_query::{EffectPresence, touches_db_derived};
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, is_known_temp, is_terminator_next, op_targets_virtual_system_table,
};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::op_classification::{classify_op, is_db_touching_class};
use crate::engine::l5::path_merge::merge_by_terminal;
use crate::engine::l5::path_walker::{
    PathCtx, Terminal, WalkBounds, WalkOpts, WalkPolicy, WalkStop, walk_evidence,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::table_display::{DescribeOp, describe_table};
use crate::program::model::workspace::{Model, ModelRoutine, ModelTable};

const DETECTOR: &str = "d2-event-fanout-in-loop";

const BOUNDS: WalkBounds = WalkBounds {
    max_depth: 20,
    max_nodes: 500,
};

/// `${op} on ${describeTable(op, routine, tableById)}` — the terminal step note.
fn table_note(
    op: &crate::program::model::workspace::ModelRecordOperation,
    routine: Option<&ModelRoutine>,
    table_by_id: &HashMap<&str, &ModelTable>,
) -> String {
    let describe = DescribeOp {
        table_id: op.table_id.as_deref(),
        record_variable_name: &op.record_variable_name,
    };
    format!(
        "{} on {}",
        op.op,
        describe_table(&describe, routine, table_by_id)
    )
}

/// Convert accumulated `Uncertainty` to `UncertaintyLite` for `to_confidence`
/// (`UncertaintyLite::of` applies the shared callsiteId → operationId →
/// routineId precedence).
fn uncertainty_lites(uncertainties: &[Uncertainty]) -> Vec<UncertaintyLite> {
    uncertainties.iter().map(UncertaintyLite::of).collect()
}

/// The D2 WalkPolicy — identical in shape to D1's (terminals = db-touching record
/// ops; expand over non-event-dispatch db-reaching edges). It is the SUPPLEMENTARY
/// walk that locates the exact DB op inside a witness subscriber.
struct D2Policy<'a> {
    routine_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
    table_by_id: &'a HashMap<&'a str, &'a ModelTable>,
    summaries: &'a HashMap<String, crate::engine::l5::full_summary::FullRoutineSummary>,
    edges_by_from: &'a HashMap<String, Vec<CombinedEdge>>,
    call_site_by_id: &'a HashMap<&'a str, &'a crate::program::body::features::PCallSite>,
    /// ⟨C1 Task 2⟩ The derived cone substrate `expand`'s `touchesDb` gate reads.
    cone_derived: &'a ConeDerivedStore,
}

impl<'a> WalkPolicy for D2Policy<'a> {
    fn terminals_at(&self, node: &str, _ctx: &PathCtx) -> Vec<Terminal> {
        let Some(r) = self.routine_by_id.get(node).copied() else {
            return Vec::new();
        };
        r.record_operations
            .iter()
            .filter(|op| is_db_touching_class(classify_op(&op.op)))
            // Class C (G-1): a `until <var>.Next() …` terminator is the subscriber
            // loop's own cursor advancement, never an actionable db op — mirrors
            // d1's interprocedural terminal filter (d1.rs).
            .filter(|op| !is_terminator_next(op))
            // Class C (G-6): an op on a BC virtual/system table reads the platform's
            // in-memory metadata store (no SQL round-trip), so it is never a d2
            // terminal — mirrors d1.
            .filter(|op| !op_targets_virtual_system_table(op, r, self.table_by_id))
            // Class A: an op provably on a Known(true) temporary record does no
            // physical-db work — mirrors d33's temp gate.
            .filter(|op| !is_known_temp(op))
            .map(|op| Terminal {
                routine_id: node.to_string(),
                local_loop_depth: op.loop_stack.len() as i64,
                op_id: Some(op.id.clone()),
            })
            .collect()
    }

    fn expand(&self, node: &str, _ctx: &PathCtx) -> Vec<CombinedEdge> {
        let Some(edges) = self.edges_by_from.get(node) else {
            return Vec::new();
        };
        edges
            .iter()
            .filter(|e| {
                if e.kind == "event-dispatch" {
                    return false;
                }
                match self.summaries.get(&e.to) {
                    Some(s) => touches_db_derived(self.cone_derived, s) != EffectPresence::No,
                    None => false,
                }
            })
            .cloned()
            .collect()
    }

    fn build_hop_step(&self, edge: &CombinedEdge, _ctx: &PathCtx) -> EvidenceStep {
        let from_routine = self.routine_by_id.get(edge.from.as_str()).copied();
        let cs = edge.callsite_id.as_ref().and_then(|cid| {
            from_routine.and_then(|fr| fr.call_sites.iter().find(|c| &c.id == cid))
        });
        let to_name = self
            .routine_by_id
            .get(edge.to.as_str())
            .map(|r| r.name.clone())
            .unwrap_or_else(|| edge.to.clone());
        let trigger_note = if edge.kind == "implicit-trigger" {
            format!(" (via implicit {to_name} trigger)")
        } else {
            String::new()
        };
        let source_anchor = if let Some(cs) = cs {
            anchor_of(&cs.source_anchor, from_routine.unwrap())
        } else if let Some(fr) = from_routine {
            anchor_of(&fr.source_anchor, fr)
        } else {
            SourceAnchor {
                source_unit_id: String::new(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                enclosing_routine_id: edge.from.clone(),
                syntax_kind: "call".to_string(),
                normalized_text_hash: None,
                leading_context_hash: None,
                trailing_context_hash: None,
            }
        };
        EvidenceStep {
            routine_id: edge.from.clone(),
            operation_id: None,
            callsite_id: edge.callsite_id.clone(),
            loop_id: None,
            source_anchor,
            note: format!("calls {to_name}{trigger_note}"),
        }
    }

    fn build_terminal_step(&self, t: &Terminal, _ctx: &PathCtx) -> EvidenceStep {
        let routine = self.routine_by_id.get(t.routine_id.as_str()).copied();
        let op = t.op_id.as_ref().and_then(|oid| {
            routine.and_then(|r| r.record_operations.iter().find(|o| &o.id == oid))
        });
        match op {
            Some(op) => EvidenceStep {
                routine_id: t.routine_id.clone(),
                operation_id: Some(op.id.clone()),
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_of(&op.source_anchor, routine.unwrap()),
                note: table_note(op, routine, self.table_by_id),
            },
            None => EvidenceStep {
                routine_id: t.routine_id.clone(),
                operation_id: t.op_id.clone(),
                callsite_id: None,
                loop_id: None,
                source_anchor: SourceAnchor {
                    source_unit_id: String::new(),
                    start_line: 0,
                    start_column: 0,
                    end_line: 0,
                    end_column: 0,
                    enclosing_routine_id: t.routine_id.clone(),
                    syntax_kind: String::new(),
                    normalized_text_hash: None,
                    leading_context_hash: None,
                    trailing_context_hash: None,
                },
                note: String::new(),
            },
        }
    }

    fn loop_depth_of_edge(&self, edge: &CombinedEdge) -> i64 {
        edge.callsite_id
            .as_ref()
            .and_then(|cid| self.call_site_by_id.get(cid.as_str()))
            .map(|cs| cs.loop_stack.len() as i64)
            .unwrap_or(0)
    }
}

pub fn detect_d2(resolved: &Model, ctx: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    // model.routines.filter(r.kind === "event-publisher").map(r.id).
    let publisher_routine_ids: HashSet<&str> = ws
        .routines
        .iter()
        .filter(|r| r.kind == "event-publisher")
        .map(|r| r.id.as_str())
        .collect();

    // resolvePublishedEvent(operationId): resolvedCallEdgeByOperation.get(op).to →
    // eventByPublisher.get(to).id. Build both indexes (first-writer-wins).
    // resolvedCallEdgeByOperation: first resolved combined edge per operationId.
    // Iterate the SORTED node list (not edges_by_from.values(), whose HashMap order is
    // nondeterministic) so first-writer-wins is stable — mirroring al-sem's ordered
    // model.callGraph scan. (For a single-target publish callsite the operationId has one
    // resolved edge, so the pick is unambiguous; the sort only matters defensively.)
    let mut edge_by_operation: HashMap<&str, &CombinedEdge> = HashMap::new();
    for node in &ctx.graph.nodes {
        if let Some(edges) = ctx.graph.edges_by_from.get(node) {
            for e in edges {
                if let Some(op) = &e.operation_id {
                    edge_by_operation.entry(op.as_str()).or_insert(e);
                }
            }
        }
    }
    // eventByPublisher: first EventSymbol per publisherRoutineId → its event id.
    let mut event_by_publisher: HashMap<&str, &str> = HashMap::new();
    for sym in &ctx.event_graph.events {
        if let Some(pr) = &sym.publisher_routine_id {
            event_by_publisher
                .entry(pr.as_str())
                .or_insert(sym.id.as_str());
        }
    }
    // eventName lookup: event id → eventName.
    let mut event_name_by_id: HashMap<&str, &str> = HashMap::new();
    for sym in &ctx.event_graph.events {
        event_name_by_id
            .entry(sym.id.as_str())
            .or_insert(sym.event_name.as_str());
    }
    let resolve_published_event = |operation_id: &str| -> Option<String> {
        let edge = edge_by_operation.get(operation_id)?;
        event_by_publisher
            .get(edge.to.as_str())
            .map(|s| s.to_string())
    };

    let policy = D2Policy {
        routine_by_id: &ctx.routine_by_id,
        table_by_id: &ctx.table_by_id,
        summaries: &ctx.summaries,
        edges_by_from: &ctx.graph.edges_by_from,
        call_site_by_id: &ctx.call_site_by_id,
        cone_derived: &ctx.cone_derived,
    };

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_opaque_callee = 0u64;
    let mut skipped_dynamic_dispatch = 0u64;
    let mut unresolved_subscriber = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) === "primary": source-only ⇒ always true.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }
        candidates_considered += 1;

        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue; // publish must be inside a loop
            }
            // Resolved edge for this callsite.
            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges
                    .iter()
                    .find(|e| e.callsite_id.as_deref() == Some(cs.id.as_str()))
            });
            let Some(edge) = edge else {
                skipped_opaque_callee += 1;
                continue; // opaque callee
            };
            if edge.kind == "interface" || edge.kind == "dynamic" {
                skipped_dynamic_dispatch += 1;
                continue;
            }
            if !publisher_routine_ids.contains(edge.to.as_str()) {
                continue; // not an event publish
            }
            let Some(event_id) = resolve_published_event(&cs.operation_id) else {
                continue;
            };

            // Subscriber edges: event-dispatch edges from the publisher for THIS event.
            let sub_edges: Vec<&CombinedEdge> = ctx
                .graph
                .edges_by_from
                .get(&edge.to)
                .map(|edges| {
                    edges
                        .iter()
                        .filter(|e| {
                            e.kind == "event-dispatch"
                                && e.event_id.as_deref() == Some(event_id.as_str())
                        })
                        .collect()
                })
                .unwrap_or_default();

            let event_name = event_name_by_id
                .get(event_id.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| event_id.clone());

            let loop_id = cs.loop_stack.last().expect("non-empty loop_stack").clone();
            let loop_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: Some(cs.id.clone()),
                loop_id: Some(loop_id.clone()),
                source_anchor: anchor_of(&cs.source_anchor, routine),
                note: format!("loop raises event {event_name}"),
            };

            let mut subscriber_steps: Vec<EvidenceStep> = Vec::new();
            let mut affected_objects: BTreeSet<String> = BTreeSet::new();
            affected_objects.insert(routine.object_id.clone());
            let mut affected_tables: BTreeSet<String> = BTreeSet::new();
            let mut uncertainties: Vec<Uncertainty> = Vec::new();
            let mut any_db_subscriber = false;
            let mut all_resolved = true;

            for sub_edge in &sub_edges {
                if sub_edge.resolution != "resolved" {
                    all_resolved = false;
                }
                let Some(sub_routine) = ctx.routine_by_id.get(sub_edge.to.as_str()).copied() else {
                    unresolved_subscriber += 1;
                    continue; // unresolvedSubscriber
                };
                if !sub_routine.body_available {
                    all_resolved = false;
                    continue;
                }
                let Some(sub_summary) = ctx.summaries.get(&sub_routine.id) else {
                    continue;
                };
                if touches_db_derived(&ctx.cone_derived, sub_summary) == EffectPresence::No {
                    continue;
                }

                // Class A/C: the coarse `touchesDb` summary above is unaware of the
                // structural suppression signals (terminator-Next, virtual/system
                // table, Known(true) temp) that the D2Policy `terminals_at` filters
                // out (mirroring d1). Run the supplementary walk FIRST and only count
                // this subscriber as a real db subscriber when it yields a Complete
                // path to a SURVIVING (non-suppressed) db op — so a subscriber whose
                // ops are ALL terminator/virtual/temp does not make d2 fire.
                let results = walk_evidence(
                    &sub_routine.id,
                    &policy,
                    BOUNDS,
                    WalkOpts {
                        initial_loop_depth: 0,
                        initial_steps: Vec::new(),
                    },
                    ctx.uncertainty_view(),
                    // Hot-tier walk stats are a d1-only concern for now.
                    None,
                );
                let Some(complete) = results.iter().find(|r| r.stop == WalkStop::Complete) else {
                    continue;
                };

                any_db_subscriber = true;
                affected_objects.insert(sub_routine.object_id.clone());
                for u in sub_summary_uncertainties(ctx, &sub_routine.id) {
                    uncertainties.push(u.clone());
                }
                for t in ctx.cone_derived.writes_tables_of(&sub_summary.routine_id) {
                    affected_tables.insert(t);
                }
                let sub_object_name = ctx
                    .objects_by_id
                    .get(sub_routine.object_id.as_str())
                    .map(|o| o.name.clone())
                    .unwrap_or_else(|| sub_routine.object_id.clone());
                let sub_app_id = sub_edge.subscriber_app_id.clone().unwrap_or_default();
                subscriber_steps.push(EvidenceStep {
                    routine_id: sub_routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&sub_routine.source_anchor, sub_routine),
                    note: format!(
                        "subscriber {} in {} (app {}) touches the database",
                        sub_routine.name, sub_object_name, sub_app_id
                    ),
                });

                subscriber_steps.extend(complete.path.iter().cloned());
                for u in &complete.uncertainties {
                    uncertainties.push(u.clone());
                }
                if let Some(term) = complete.path.last()
                    && let Some(op_id) = &term.operation_id
                    && let Some(term_routine) =
                        ctx.routine_by_id.get(term.routine_id.as_str()).copied()
                    && let Some(term_op) = term_routine
                        .record_operations
                        .iter()
                        .find(|o| &o.id == op_id)
                    && let Some(tid) = &term_op.table_id
                {
                    affected_tables.insert(tid.clone());
                }
            }

            if !any_db_subscriber {
                continue;
            }

            let base_level = if all_resolved { "likely" } else { "possible" };
            let confidence: FindingConfidence = to_confidence(
                &uncertainty_lites(&dedupe_uncertainties(uncertainties)),
                base_level,
            );

            let mut evidence_path = Vec::with_capacity(1 + subscriber_steps.len());
            evidence_path.push(loop_step);
            evidence_path.extend(subscriber_steps);

            let finding = Finding {
                id: format!("d2/{loop_id}/{event_id}"),
                root_cause_key: format!("d2/{event_id}"),
                detector: DETECTOR.to_string(),
                title: "Event raised inside a loop fans out to database work".into(),
                root_cause: format!(
                    "{} raises {event_name} inside a loop; subscribers touch the database \
                     every iteration.",
                    routine.name
                ),
                severity: "high".to_string(),
                confidence,
                primary_location: anchor_of(&cs.source_anchor, routine),
                evidence_path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: id_list(affected_tables),
                fix_options: vec![FixOption {
                    description:
                        "Raise the event once outside the loop, or batch the work the subscribers do."
                            .to_string().into(),
                    safety: "medium".into(),
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
            // Fingerprint deferred until AFTER merge_by_terminal.
            findings.push(finding);
        }
    }

    // Two-stage collapse: dedup by id (first-wins) → merge_by_terminal.
    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<Finding> = Vec::new();
    for f in findings {
        if seen.contains(&f.id) {
            continue;
        }
        seen.insert(f.id.clone());
        deduped.push(f);
    }
    let mut merged = merge_by_terminal(deduped);
    for f in &mut merged {
        f.fingerprint = Some(fp_index.fingerprint_of(f));
    }
    merged.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = merged.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("opaqueCallee", skipped_opaque_callee);
    stats.add_skip("dynamicDispatch", skipped_dynamic_dispatch);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    stats.add_skip("unresolvedSubscriber", unresolved_subscriber);
    Ok(DetectorOutput {
        findings: merged,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

/// al-sem reads `subRoutine.summary.uncertainties` (the CORE RoutineSummary
/// uncertainties), which the Rust `FullRoutineSummary` drops; the DetectorContext
/// instead exposes `uncertainties_by_node = core.uncertainties ∪ uncertaintyEdgesByFrom`.
/// This is PROVABLY EQUAL to al-sem's core set, not an over-approximation: the core
/// `RoutineSummary.uncertainties` is itself built by folding in the routine's own
/// to-less callsite edges (`uncertaintyEdgesByFrom[R]`) — al-sem `summary-runner.ts`
/// composeRoutineCtx, mirrored in our `summary_runner.rs` (`graph.uncertainty_edges`
/// where `ue.from == R`). So `core[R] ⊇ uncertaintyEdgesByFrom[R]` ⟹ `core[R] ∪
/// edges[R] = core[R]`. Reading the per-node union therefore yields exactly al-sem's
/// `subSummary.uncertainties`. (The supplementary walk then adds its own accumulated
/// uncertainties, which al-sem's `walkEvidence` derives from the SAME union; deduped
/// before to_confidence.)
///
/// Returns an ITERATOR of borrows, resolved from the node's interned set on
/// demand. It used to `.cloned()` the whole `Vec` for its one caller to
/// immediately re-clone element-by-element — a deep copy of a set that is read
/// only, discarded immediately, and shared with every other node in the same SCC.
fn sub_summary_uncertainties<'a>(
    ctx: &'a DetectorContext,
    routine_id: &str,
) -> impl Iterator<Item = &'a Uncertainty> + 'a {
    ctx.uncertainty_view().values_of(routine_id)
}
