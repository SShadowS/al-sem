//! R4-F Stage-3b — DIGEST witness + effects + occurrence-build path.
//!
//! Byte-parity port of al-sem's digest query slice:
//!   - `src/digest/effect-taxonomy.ts`  → effect_type_of / effect_detail_of / resource_display_of
//!   - `src/query/indexes.ts`           → build_fingerprint_indexes
//!   - `src/query/witness.ts`           → reconstruct_witness_paths (direct + inherited BFS)
//!   - `src/query/hop-projection.ts`    → project_path / project_hop (QueryWitnessHop)
//!   - `src/digest/digest-query.ts`     → digest_query (per-root effect build + dedupe + merge)
//!   - `src/digest/ordering-engine.ts`  → the OCCURRENCE-BUILD slice (canonical_key + occurrence_id)
//!
//! The `occurrenceId` (= `factId`) is the parity crux:
//!
//! ```text
//! factId = sha256Hex( routineId + "|" + linkSignature + "|"
//!                    + (evidenceOperationId? "operation":"callsite") + "|"
//!                    + (evidenceOperationId ?? evidenceCallsiteId ?? "") + "|"
//!                    + effectType )[0..16]
//! ```
//!
//! where `linkSignature = viaPaths[0].map(hop ->
//!   "{fromRoutineId}>{toRoutineId??""}@{callsiteId??""}/{kind}/{edgeId??""}/{""}").join(",")`.
//! `QueryWitnessHop` has NO `edgeId` → always "" → each hop segment ends "//".
//!
//! ## Determinism (R4-F Rev2)
//!
//! - effectMap, the index buckets, seenCanonicalKeys = `IndexMap`-style ordered `Vec`s;
//!   NO `HashMap` iteration reaches output / path-choice / hash.
//! - BFS queue = `VecDeque` (FIFO). All sorts stable (`sort_by`, chained `.cmp`).
//! - The JSON-stringify tiebreak serializer (`hops_json` / `value_source_json`)
//!   reproduces V8 `JSON.stringify` byte-for-byte: struct-field declaration order =
//!   the TS object-literal field order per hop variant, `None`/`undefined` OMITTED,
//!   no whitespace, standard escaping. ASCII corpus → ordinal `str::cmp` everywhere.
//! - Conditionality / transactionContext / guarantees / scopedGuarantees are STAGE 4
//!   and EXCLUDED from this projection.

use std::collections::HashMap;

use serde::Serialize;

use crate::engine::ids::sha256_hex;
use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, SnapCapabilityExtra, SnapTempState, SnapValueSource,
    SnapshotCallsiteEvidence, SnapshotGraphEdge, compose_snapshot,
};

// ===========================================================================
// effect-taxonomy.ts — effectTypeOf / effectDetailOf / resourceDisplayOf
// ===========================================================================

/// `mapOp` (effect-taxonomy.ts). Returns `None` for execute/subscribe/read.
fn map_op(op: &str) -> Option<&'static str> {
    match op {
        "commit" => Some("COMMIT"),
        "insert" => Some("DB_INSERT"),
        "modify" => Some("DB_MODIFY"),
        "delete" => Some("DB_DELETE"),
        "publish" => Some("EVENT_PUBLISH"),
        "send" => Some("HTTP"),
        "store-read" | "store-write" | "store-delete" => Some("ISOLATED_STORAGE"),
        "open" | "write-blob" => Some("FILE"),
        "start" => Some("BACKGROUND_TASK"),
        "log" => Some("TELEMETRY"),
        "ui-message" => Some("UI_MESSAGE"),
        "ui-confirm" => Some("UI_CONFIRM"),
        "ui-error" => Some("UI_ERROR"),
        "ui-window-open" => Some("UI_WINDOW_OPEN"),
        "error-throw" => Some("ERROR_THROW"),
        _ => None,
    }
}

/// The capability fact shape digest reads — the snapshot's `SnapshotCapabilityFact`.
/// We read straight off the composed snapshot (no re-projection).
type Fact = crate::engine::l5::snapshot::SnapshotCapabilityFact;

fn effect_type_of(fact: &Fact) -> Option<&'static str> {
    map_op(&fact.op)
}

/// `resourceDisplayOf` (effect-taxonomy.ts).
fn resource_display_of(
    fact: &Fact,
    stable_id_to_display: &HashMap<String, String>,
) -> Option<String> {
    let rid = fact.resource_id.as_ref()?;
    if fact.resource_kind == "table" {
        let parts: Vec<&str> = rid.split('/').collect();
        if parts.len() == 3 && parts[1] == "table" {
            let stable = format!("{}:Table:{}", parts[0], parts[2]);
            if let Some(name) = stable_id_to_display.get(&stable)
                && !name.is_empty()
            {
                return Some(name.clone());
            }
        }
        return None;
    }
    if fact.resource_kind == "event" {
        let marker = "/event/";
        if let Some(idx) = rid.rfind(marker) {
            let name = &rid[idx + marker.len()..];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        return None;
    }
    None
}

/// `effectDetailOf` (effect-taxonomy.ts). Returns an ORDERED list of (key, value)
/// pairs in insertion order: resourceId, resourceDisplay, (eventClass|method|storageOp), fileOp.
fn effect_detail_of(
    fact: &Fact,
    stable_id_to_display: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut detail: Vec<(String, String)> = Vec::new();

    if let Some(rid) = &fact.resource_id {
        detail.push(("resourceId".to_string(), rid.clone()));
    }

    if let Some(display) = resource_display_of(fact, stable_id_to_display) {
        detail.push(("resourceDisplay".to_string(), display));
    }

    match &fact.extra {
        Some(SnapCapabilityExtra::Event { event_class, .. }) => {
            // `fact.extra.eventClass !== undefined` — always present on the event variant.
            detail.push(("eventClass".to_string(), event_class.clone()));
        }
        Some(SnapCapabilityExtra::Http { method, .. }) => {
            detail.push(("method".to_string(), method.clone()));
        }
        Some(SnapCapabilityExtra::Storage { .. }) => {
            detail.push(("storageOp".to_string(), fact.op.clone()));
        }
        _ => {}
    }

    if fact.resource_kind == "file" {
        detail.push(("fileOp".to_string(), fact.op.clone()));
    }

    detail
}

// ===========================================================================
// indexes.ts — buildFingerprintIndexes (ordered buckets, no HashMap-iteration-to-output)
// ===========================================================================

const ROUTINE_ID_SEPARATOR: char = '#';

fn is_routine_stable_id(id: &str) -> bool {
    id.contains(ROUTINE_ID_SEPARATOR)
}

struct FingerprintIndexes<'a> {
    stable_id_to_display: HashMap<String, String>,
    routine_display_by_id: HashMap<String, String>,
    /// Per-from ordered edge bucket (source array order PRESERVED).
    outgoing_edges: HashMap<String, Vec<&'a SnapshotGraphEdge>>,
    /// Reverse of `outgoing_edges`: incoming_edges[to] = list of `from` node ids that
    /// have at least one edge pointing to `to`.  Used by the reverse-BFS reachability
    /// precomputation in `reconstruct_witness_paths` (FIX 1 — built once per snapshot,
    /// shared across all witness BFS calls).  Only stores `from` strings because the
    /// BFS only needs to know which predecessor nodes to visit — the actual edges are
    /// not needed for the reachability walk.
    incoming_edges: HashMap<String, Vec<String>>,
    /// Per-subject ordered fact bucket (direct ∪ inherited, source order PRESERVED).
    facts_by_routine: HashMap<String, Vec<&'a Fact>>,
    direct_facts_by_routine: HashMap<String, Vec<&'a Fact>>,
    /// DIRECT facts indexed by `(op, resource_kind)` — the strict prefix of
    /// `fact_equivalent` (which REQUIRES `op` and `resource_kind` equal). The
    /// reverse-BFS carrier seeding in `reconstruct_witness_paths` looks up only the
    /// candidates sharing the target fact's `(op, kind)` instead of scanning EVERY
    /// routine's direct facts per call (the previous per-call O(all-direct-facts)
    /// hot-spot). Built once per snapshot.
    direct_facts_by_op_kind: HashMap<(String, String), Vec<&'a Fact>>,
    coverage_by_routine: HashMap<String, &'a crate::engine::l5::snapshot::SnapshotCoverageRecord>,
    callsite_by_id: HashMap<String, &'a crate::engine::l5::snapshot::SnapshotCallsiteEvidence>,
    operation_by_id: HashMap<String, &'a crate::engine::l5::snapshot::SnapshotOperationEvidence>,
    event_display_by_id: HashMap<String, String>,
}

fn build_fingerprint_indexes(snap: &CapabilitySnapshot) -> FingerprintIndexes<'_> {
    let mut stable_id_to_display: HashMap<String, String> = HashMap::new();
    let mut routine_display_by_id: HashMap<String, String> = HashMap::new();

    for i in 0..snap.identities.stable_ids.len() {
        let id = snap
            .identities
            .stable_ids
            .get(i)
            .cloned()
            .unwrap_or_default();
        let display = snap
            .identities
            .display_names
            .get(i)
            .cloned()
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        stable_id_to_display.insert(id.clone(), display.clone());
        if is_routine_stable_id(&id) {
            routine_display_by_id.insert(id.clone(), display.clone());
        }
    }

    // outgoingEdges[from] ← typedEdges, ORDER PRESERVED (source array order).
    // incomingEdges[to] ← set of `from` ids — the reverse graph, built in the same pass.
    let mut outgoing_edges: HashMap<String, Vec<&SnapshotGraphEdge>> = HashMap::new();
    let mut incoming_edges: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &snap.typed_edges {
        let from = edge_from(edge).to_string();
        outgoing_edges.entry(from.clone()).or_default().push(edge);
        if let Some(to) = edge_to(edge) {
            incoming_edges.entry(to.to_string()).or_default().push(from);
        }
    }
    // Pre-sort each node's out-edges by `edge_compare` ONCE here (shared across the
    // whole analyze) instead of cloning+sorting per BFS state in
    // `reconstruct_witness_paths`. Behavior-identical: the witness BFS expanded
    // `out.clone().sort(edge_compare)` per state — same total order, computed once.
    for v in outgoing_edges.values_mut() {
        v.sort_by(|a, b| edge_compare(a, b));
    }

    // factsByRoutine ← capabilityFacts; directFactsByRoutine ← provenance=="direct".
    let mut facts_by_routine: HashMap<String, Vec<&Fact>> = HashMap::new();
    let mut direct_facts_by_routine: HashMap<String, Vec<&Fact>> = HashMap::new();
    let mut direct_facts_by_op_kind: HashMap<(String, String), Vec<&Fact>> = HashMap::new();
    for fact in &snap.capability_facts {
        facts_by_routine
            .entry(fact.subject.clone())
            .or_default()
            .push(fact);
        if fact.provenance == "direct" {
            direct_facts_by_routine
                .entry(fact.subject.clone())
                .or_default()
                .push(fact);
            direct_facts_by_op_kind
                .entry((fact.op.clone(), fact.resource_kind.clone()))
                .or_default()
                .push(fact);
        }
    }

    let mut coverage_by_routine: HashMap<
        String,
        &crate::engine::l5::snapshot::SnapshotCoverageRecord,
    > = HashMap::new();
    for rec in &snap.coverage {
        coverage_by_routine.insert(rec.subject.clone(), rec);
    }

    let mut callsite_by_id: HashMap<
        String,
        &crate::engine::l5::snapshot::SnapshotCallsiteEvidence,
    > = HashMap::new();
    for cs in &snap.callsite_index {
        callsite_by_id.insert(cs.callsite_id.clone(), cs);
    }

    let mut operation_by_id: HashMap<
        String,
        &crate::engine::l5::snapshot::SnapshotOperationEvidence,
    > = HashMap::new();
    for op in &snap.operation_index {
        operation_by_id.insert(op.operation_id.clone(), op);
    }

    // eventDisplayById ← publisher event declarations; eventName = eventId.split("::")[1].
    let mut event_display_by_id: HashMap<String, String> = HashMap::new();
    for decl in &snap.event_declarations {
        if decl.kind != "publisher" {
            continue;
        }
        let parts: Vec<&str> = decl.event_id.split("::").collect();
        let event_name = parts
            .get(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| decl.event_id.clone());
        event_display_by_id.insert(decl.event_id.clone(), event_name);
    }

    FingerprintIndexes {
        stable_id_to_display,
        routine_display_by_id,
        direct_facts_by_op_kind,
        outgoing_edges,
        incoming_edges,
        facts_by_routine,
        direct_facts_by_routine,
        coverage_by_routine,
        callsite_by_id,
        operation_by_id,
        event_display_by_id,
    }
}

// ---------------------------------------------------------------------------
// SnapshotGraphEdge accessors digest needs (the snapshot's accessors are private).
// ---------------------------------------------------------------------------

fn edge_kind(e: &SnapshotGraphEdge) -> &str {
    match e {
        SnapshotGraphEdge::DirectCall { kind, .. }
        | SnapshotGraphEdge::VariableTypedCall { kind, .. }
        | SnapshotGraphEdge::InterfaceDispatch { kind, .. }
        | SnapshotGraphEdge::ObjectRunResolved { kind, .. }
        | SnapshotGraphEdge::ObjectRunUnresolved { kind, .. }
        | SnapshotGraphEdge::EventDispatch { kind, .. } => kind,
    }
}

fn edge_from(e: &SnapshotGraphEdge) -> &str {
    match e {
        SnapshotGraphEdge::DirectCall { from, .. }
        | SnapshotGraphEdge::VariableTypedCall { from, .. }
        | SnapshotGraphEdge::InterfaceDispatch { from, .. }
        | SnapshotGraphEdge::ObjectRunResolved { from, .. }
        | SnapshotGraphEdge::ObjectRunUnresolved { from, .. }
        | SnapshotGraphEdge::EventDispatch { from, .. } => from,
    }
}

/// `to` endpoint. None only for object-run-unresolved (no `to`).
fn edge_to(e: &SnapshotGraphEdge) -> Option<&str> {
    match e {
        SnapshotGraphEdge::DirectCall { to, .. }
        | SnapshotGraphEdge::VariableTypedCall { to, .. }
        | SnapshotGraphEdge::InterfaceDispatch { to, .. }
        | SnapshotGraphEdge::ObjectRunResolved { to, .. }
        | SnapshotGraphEdge::EventDispatch { to, .. } => Some(to),
        SnapshotGraphEdge::ObjectRunUnresolved { .. } => None,
    }
}

/// callsiteId for a call-family edge (event-dispatch has none → None).
fn edge_callsite_id(e: &SnapshotGraphEdge) -> Option<&str> {
    match e {
        SnapshotGraphEdge::DirectCall { callsite_id, .. }
        | SnapshotGraphEdge::VariableTypedCall { callsite_id, .. }
        | SnapshotGraphEdge::InterfaceDispatch { callsite_id, .. }
        | SnapshotGraphEdge::ObjectRunResolved { callsite_id, .. }
        | SnapshotGraphEdge::ObjectRunUnresolved { callsite_id, .. } => Some(callsite_id),
        SnapshotGraphEdge::EventDispatch { .. } => None,
    }
}

/// `edgeCompare` (witness.ts:542): kind, then String(callsiteId??""), then String(to??"").
/// Chained `.cmp` (stable). All ordinal.
///
/// **Stability note (witness.ts:542 / V8-stable-preserves-equal-key-order, matches Rust
/// stable sort_by):** the TS comparator uses `?-1:1` (never returns 0), but an empirical
/// 2000-array V8 stress test confirmed that V8's Array.sort preserves insertion order for
/// equal keys under this comparator — identical to Rust's `stable sort_by` returning
/// `Equal`. The current `.cmp` is therefore CORRECT; do NOT change it to `?-1:1`.
fn edge_compare(a: &SnapshotGraphEdge, b: &SnapshotGraphEdge) -> std::cmp::Ordering {
    let ka = edge_kind(a);
    let kb = edge_kind(b);
    if ka != kb {
        return ka.cmp(kb);
    }
    let csa = edge_callsite_id(a).unwrap_or("");
    let csb = edge_callsite_id(b).unwrap_or("");
    if csa != csb {
        return csa.cmp(csb);
    }
    let toa = edge_to(a).unwrap_or("");
    let tob = edge_to(b).unwrap_or("");
    toa.cmp(tob)
}

// ===========================================================================
// witness.ts — WitnessHop union + reconstructWitnessPaths
// ===========================================================================

const HARD_PATH_CAP: usize = 256;
const MAX_DEPTH: usize = 64;
const MAX_STATES: usize = 25_000;

/// Internal WitnessHop union (witness.ts). Field set per the TS literal.
#[derive(Debug, Clone)]
enum WitnessHop {
    Call {
        routine_id: String,
        routine_display: String,
        callee_display: String,
        callsite_id: String,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    ObjectRun {
        routine_id: String,
        routine_display: String,
        target_object_id: Option<String>,
        target_display: Option<String>,
        resolved: bool,
        callsite_id: Option<String>,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    EventDispatch {
        routine_id: String,
        routine_display: String,
        event_id: String,
        event_display: String,
    },
    VariableTypedCall {
        routine_id: String,
        routine_display: String,
        receiver_type: String,
        callee_display: Option<String>,
        callsite_id: String,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    InterfaceDispatch {
        routine_id: String,
        routine_display: String,
        interface_name: String,
        candidate_count: usize,
        callee_display: Option<String>,
        callsite_id: String,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    Terminal {
        evidence_kind: TerminalKind,
        operation_id: Option<String>,
        callsite_id: Option<String>,
        #[allow(dead_code)]
        display_text: String,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalKind {
    Operation,
    Callsite,
    Synthetic,
}

impl WitnessHop {
    /// `hop.routineId` — the edge destination for every non-terminal hop kind.
    fn routine_id(&self) -> Option<&str> {
        match self {
            WitnessHop::Call { routine_id, .. }
            | WitnessHop::ObjectRun { routine_id, .. }
            | WitnessHop::EventDispatch { routine_id, .. }
            | WitnessHop::VariableTypedCall { routine_id, .. }
            | WitnessHop::InterfaceDispatch { routine_id, .. } => Some(routine_id),
            WitnessHop::Terminal { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct WitnessPath {
    hops: Vec<WitnessHop>,
}

/// Outcome of `reconstruct_witness_paths`: raw (un-projected) paths plus the
/// `truncated`/`incomplete` flags and the `(kind, detail)` diagnostic pairs.
struct WitnessOutcomeExt {
    paths: Vec<WitnessPath>,
    truncated: bool,
    incomplete: bool,
    /// Internal diagnostics (kind, optional detail). Forwarded by
    /// `reconstruct_witness_paths_pub` as `WitnessDiagnosticPub`.
    diagnostics: Vec<(String, Option<String>)>,
}

/// `buildDirectTerminal` (witness.ts).
fn build_direct_terminal(
    evidence_kind: TerminalKind,
    witness_id: &str,
    op_ev: Option<&crate::engine::l5::snapshot::SnapshotOperationEvidence>,
    cs_ev: Option<&crate::engine::l5::snapshot::SnapshotCallsiteEvidence>,
) -> WitnessHop {
    match evidence_kind {
        TerminalKind::Operation => {
            let display_text = op_ev
                .map(|e| e.display_text.clone())
                .unwrap_or_else(|| witness_id.to_string());
            WitnessHop::Terminal {
                evidence_kind: TerminalKind::Operation,
                operation_id: Some(witness_id.to_string()),
                callsite_id: None,
                display_text,
                source_file: op_ev.map(|e| e.source_file.clone()),
                line: op_ev.map(|e| e.start_line),
                column: op_ev.map(|e| e.start_column),
            }
        }
        TerminalKind::Callsite => {
            let display_text = cs_ev
                .map(|e| e.callee_display.clone())
                .unwrap_or_else(|| witness_id.to_string());
            WitnessHop::Terminal {
                evidence_kind: TerminalKind::Callsite,
                operation_id: None,
                callsite_id: Some(witness_id.to_string()),
                display_text,
                source_file: cs_ev.map(|e| e.source_file.clone()),
                line: cs_ev.map(|e| e.start_line),
                column: cs_ev.map(|e| e.start_column),
            }
        }
        TerminalKind::Synthetic => unreachable!("build_direct_terminal not called for synthetic"),
    }
}

/// `terminalHopFromFact` (witness.ts) — terminal for the matched direct fact.
fn terminal_hop_from_fact(fact: &Fact, idx: &FingerprintIndexes) -> WitnessHop {
    if let Some(wo) = &fact.witness_operation_id {
        let ev = idx.operation_by_id.get(wo.as_str()).copied();
        return WitnessHop::Terminal {
            evidence_kind: TerminalKind::Operation,
            operation_id: Some(wo.clone()),
            callsite_id: None,
            display_text: ev
                .map(|e| e.display_text.clone())
                .unwrap_or_else(|| wo.clone()),
            source_file: ev.map(|e| e.source_file.clone()),
            line: ev.map(|e| e.start_line),
            column: ev.map(|e| e.start_column),
        };
    }
    if let Some(wc) = &fact.witness_callsite_id {
        let ev = idx.callsite_by_id.get(wc.as_str()).copied();
        return WitnessHop::Terminal {
            evidence_kind: TerminalKind::Callsite,
            operation_id: None,
            callsite_id: Some(wc.clone()),
            display_text: ev
                .map(|e| e.callee_display.clone())
                .unwrap_or_else(|| wc.clone()),
            source_file: ev.map(|e| e.source_file.clone()),
            line: ev.map(|e| e.start_line),
            column: ev.map(|e| e.start_column),
        };
    }
    WitnessHop::Terminal {
        evidence_kind: TerminalKind::Synthetic,
        operation_id: None,
        callsite_id: None,
        display_text: format!("{} {}", fact.op, fact.resource_kind),
        source_file: None,
        line: None,
        column: None,
    }
}

/// `edgeToHop` (witness.ts). object-run-unresolved → None (BFS cannot walk through).
fn edge_to_hop(edge: &SnapshotGraphEdge, idx: &FingerprintIndexes) -> Option<WitnessHop> {
    match edge {
        SnapshotGraphEdge::DirectCall {
            to, callsite_id, ..
        } => {
            let display = idx
                .routine_display_by_id
                .get(to)
                .cloned()
                .unwrap_or_else(|| to.clone());
            let cs = idx.callsite_by_id.get(callsite_id.as_str()).copied();
            Some(WitnessHop::Call {
                routine_id: to.clone(),
                routine_display: display,
                callee_display: cs.map(|c| c.callee_display.clone()).unwrap_or_default(),
                callsite_id: callsite_id.clone(),
                source_file: cs.map(|c| c.source_file.clone()),
                line: cs.map(|c| c.start_line),
                column: cs.map(|c| c.start_column),
            })
        }
        SnapshotGraphEdge::ObjectRunResolved {
            to,
            callsite_id,
            target_object,
            ..
        } => {
            let display = idx
                .routine_display_by_id
                .get(to)
                .cloned()
                .unwrap_or_else(|| to.clone());
            let cs = idx.callsite_by_id.get(callsite_id.as_str()).copied();
            Some(WitnessHop::ObjectRun {
                routine_id: to.clone(),
                routine_display: display,
                target_object_id: Some(target_object.clone()),
                target_display: idx.stable_id_to_display.get(target_object).cloned(),
                resolved: true,
                callsite_id: Some(callsite_id.clone()),
                source_file: cs.map(|c| c.source_file.clone()),
                line: cs.map(|c| c.start_line),
                column: cs.map(|c| c.start_column),
            })
        }
        SnapshotGraphEdge::ObjectRunUnresolved { .. } => None,
        SnapshotGraphEdge::EventDispatch { to, event_id, .. } => {
            let display = idx
                .routine_display_by_id
                .get(to)
                .cloned()
                .unwrap_or_else(|| to.clone());
            Some(WitnessHop::EventDispatch {
                routine_id: to.clone(),
                routine_display: display,
                event_id: event_id.clone(),
                event_display: idx
                    .event_display_by_id
                    .get(event_id)
                    .cloned()
                    .unwrap_or_else(|| event_id.clone()),
            })
        }
        SnapshotGraphEdge::VariableTypedCall {
            to,
            callsite_id,
            receiver_type,
            ..
        } => {
            let display = idx
                .routine_display_by_id
                .get(to)
                .cloned()
                .unwrap_or_else(|| to.clone());
            let cs = idx.callsite_by_id.get(callsite_id.as_str()).copied();
            Some(WitnessHop::VariableTypedCall {
                routine_id: to.clone(),
                routine_display: display,
                receiver_type: receiver_type.clone(),
                callee_display: cs.map(|c| c.callee_display.clone()),
                callsite_id: callsite_id.clone(),
                source_file: cs.map(|c| c.source_file.clone()),
                line: cs.map(|c| c.start_line),
                column: cs.map(|c| c.start_column),
            })
        }
        SnapshotGraphEdge::InterfaceDispatch {
            to,
            callsite_id,
            interface_name,
            candidate_count,
            ..
        } => {
            let display = idx
                .routine_display_by_id
                .get(to)
                .cloned()
                .unwrap_or_else(|| to.clone());
            let cs = idx.callsite_by_id.get(callsite_id.as_str()).copied();
            Some(WitnessHop::InterfaceDispatch {
                routine_id: to.clone(),
                routine_display: display,
                interface_name: interface_name.clone(),
                candidate_count: *candidate_count,
                callee_display: cs.map(|c| c.callee_display.clone()),
                callsite_id: callsite_id.clone(),
                source_file: cs.map(|c| c.source_file.clone()),
                line: cs.map(|c| c.start_line),
                column: cs.map(|c| c.start_column),
            })
        }
    }
}

/// `factEquivalent` (witness.ts). resourceId compared ONLY when BOTH Some (asymmetric).
/// resourceArgSource compared via canonical JSON when both Some. dispatch → objectType
/// compared (undefined-tolerant).
fn fact_equivalent(a: &Fact, b: &Fact) -> bool {
    if a.op != b.op {
        return false;
    }
    if a.resource_kind != b.resource_kind {
        return false;
    }
    if let (Some(ra), Some(rb)) = (&a.resource_id, &b.resource_id)
        && ra != rb
    {
        return false;
    }
    if let (Some(sa), Some(sb)) = (&a.resource_arg_source, &b.resource_arg_source)
        && value_source_json(sa) != value_source_json(sb)
    {
        return false;
    }
    let oa = dispatch_object_type(a);
    let ob = dispatch_object_type(b);
    let a_is_dispatch = matches!(&a.extra, Some(SnapCapabilityExtra::Dispatch { .. }));
    let b_is_dispatch = matches!(&b.extra, Some(SnapCapabilityExtra::Dispatch { .. }));
    if a_is_dispatch || b_is_dispatch {
        // TS reads extra?.objectType on both; non-dispatch → undefined. Compare both
        // (undefined-tolerant: only the dispatch variant carries objectType).
        if oa != ob {
            return false;
        }
    }
    true
}

fn dispatch_object_type(f: &Fact) -> Option<&str> {
    match &f.extra {
        Some(SnapCapabilityExtra::Dispatch { object_type, .. }) => Some(object_type.as_str()),
        _ => None,
    }
}

/// `reconstructWitnessPaths(req)` with `limit:"all"` (→ HARD_PATH_CAP).
/// `reconstructWitnessPaths` (witness.ts) — the SINGLE witness-BFS implementation.
///
/// `cap` is the effective path cap (`witnessLimit`): default callers pass
/// `HARD_PATH_CAP`; the fingerprint query passes the user's `--witness` value.
/// Always tracks `truncated`, `incomplete`, and emits all 9 TS diagnostic kinds
/// (8 explicit + the `terminal-not-found` default-fallthrough) so the JSON
/// projection and the digest path agree on witness shape. The `(kind, detail)`
/// pairs mirror `projectFingerprintQuery`'s diag mapping (contracts/fingerprint-query.ts).
fn reconstruct_witness_paths(
    root_id: &str,
    fact: &Fact,
    idx: &FingerprintIndexes,
    cap: usize,
) -> WitnessOutcomeExt {
    let mut diagnostics: Vec<(String, Option<String>)> = Vec::new();

    if fact.provenance == "direct" {
        // Case A: witnessOperationId → terminal "operation".
        if let Some(wo) = &fact.witness_operation_id {
            let ev = idx.operation_by_id.get(wo.as_str()).copied();
            let hop = build_direct_terminal(TerminalKind::Operation, wo, ev, None);
            let incomplete = ev.is_none();
            if ev.is_none() {
                diagnostics.push(("missing-operation-evidence".to_string(), Some(wo.clone())));
            }
            return WitnessOutcomeExt {
                paths: vec![WitnessPath { hops: vec![hop] }],
                truncated: false,
                incomplete,
                diagnostics,
            };
        }
        // Case B: only witnessCallsiteId → terminal "callsite".
        if let Some(wc) = &fact.witness_callsite_id {
            let ev = idx.callsite_by_id.get(wc.as_str()).copied();
            let hop = build_direct_terminal(TerminalKind::Callsite, wc, None, ev);
            let incomplete = ev.is_none();
            if ev.is_none() {
                diagnostics.push(("missing-callsite-evidence".to_string(), Some(wc.clone())));
            }
            return WitnessOutcomeExt {
                paths: vec![WitnessPath { hops: vec![hop] }],
                truncated: false,
                incomplete,
                diagnostics,
            };
        }
        // Direct with no witness anchor → synthetic + missing-witness-anchor (detail=subject).
        return WitnessOutcomeExt {
            paths: vec![WitnessPath {
                hops: vec![WitnessHop::Terminal {
                    evidence_kind: TerminalKind::Synthetic,
                    operation_id: None,
                    callsite_id: None,
                    display_text: format!("{} {}", fact.op, fact.resource_kind),
                    source_file: None,
                    line: None,
                    column: None,
                }],
            }],
            truncated: false,
            incomplete: true,
            diagnostics: vec![(
                "missing-witness-anchor".to_string(),
                Some(fact.subject.clone()),
            )],
        };
    }

    // --- Case C: inherited fact (BFS) ---
    let mut paths: Vec<WitnessPath> = Vec::new();

    let Some(witness_cs) = &fact.witness_callsite_id else {
        // first-hop-not-found (no witnessCallsiteId) → detail = via (callsiteId absent).
        return WitnessOutcomeExt {
            paths: Vec::new(),
            truncated: false,
            incomplete: true,
            diagnostics: vec![("first-hop-not-found".to_string(), Some(fact.via.clone()))],
        };
    };

    let empty: Vec<&SnapshotGraphEdge> = Vec::new();
    let out_from_root = idx.outgoing_edges.get(root_id).unwrap_or(&empty);
    let first_edges: Vec<&SnapshotGraphEdge> = out_from_root
        .iter()
        .filter(|e| edge_callsite_id(e) == Some(witness_cs.as_str()))
        .copied()
        .collect();
    if first_edges.is_empty() {
        // first-hop-not-found → detail = callsiteId (present here).
        return WitnessOutcomeExt {
            paths: Vec::new(),
            truncated: false,
            incomplete: true,
            diagnostics: vec![("first-hop-not-found".to_string(), Some(witness_cs.clone()))],
        };
    }

    // --- Case-C BFS: parent-pointer arena (eliminates per-state path clones) ---
    //
    // Each arena node records the single hop that led INTO `routine` from its
    // parent, plus a back-pointer index. Visited-set reconstruction and path
    // materialisation walk the parent chain — O(depth), no allocation per expansion.
    //
    // Arena index 0 is never a real node (sentinel); real nodes start at index 1
    // so `parent: 0` can be used as "seed / no parent" sentinel.
    struct Node {
        routine: String,
        hop: WitnessHop, // the edge-hop that led INTO `routine`
        parent: usize,   // arena index of parent; 0 = seed (no parent)
        depth: usize,    // number of hops from root to this node (seed = 1)
    }

    // Arena slot 0 is a sentinel placeholder (never popped or referenced as a real node).
    let sentinel_hop = WitnessHop::Terminal {
        evidence_kind: TerminalKind::Synthetic,
        operation_id: None,
        callsite_id: None,
        display_text: String::new(),
        source_file: None,
        line: None,
        column: None,
    };
    let mut arena: Vec<Node> = vec![Node {
        routine: String::new(),
        hop: sentinel_hop,
        parent: 0,
        depth: 0,
    }];

    // Collect seed (routine, hop) pairs so we can sort before pushing to arena.
    let mut seed_pairs: Vec<(String, WitnessHop)> = Vec::new();
    for edge in &first_edges {
        let Some(hop) = edge_to_hop(edge, idx) else {
            continue;
        };
        let Some(to) = edge_to(edge) else {
            continue;
        };
        seed_pairs.push((to.to_string(), hop));
    }

    // seed-sort by routine (`.cmp`, stable). witness.ts:276 uses `(a,b)=> a.routine<b.routine?-1:1`
    // — an empirical V8 2000-array stress test confirmed V8-stable-preserves-equal-key-order for
    // this `?-1:1` comparator, so Rust's stable `.cmp` (which returns Equal on a tie) is CORRECT
    // and preserves typedEdges-insertion order for equal-routine seeds. Do NOT change to `?-1:1`.
    seed_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for (routine, hop) in seed_pairs {
        let idx_node = arena.len();
        arena.push(Node {
            routine,
            hop,
            parent: 0, // seed: no parent
            depth: 1,
        });
        queue.push_back(idx_node);
    }

    // Helper: walk the parent chain from `ni` and reconstruct the hop Vec (root→ni order).
    // This is called only on terminal / boundary hits — rare — so allocation here is fine.
    let reconstruct_hops = |arena: &Vec<Node>, ni: usize| -> Vec<WitnessHop> {
        let mut rev = Vec::new();
        let mut cur = ni;
        while cur != 0 {
            rev.push(arena[cur].hop.clone());
            cur = arena[cur].parent;
        }
        rev.reverse();
        rev
    };

    // Helper: build the per-path visited set by walking the parent chain.
    // Uses &str references into arena strings — no String allocation.
    // Returns a HashSet<*const str> as thin ptr keys to avoid lifetime issues.
    // Actually we collect routine str slices via raw pointer identity.
    // Simpler: collect into a Vec<&str> and do linear scan (depth is ≤ MAX_DEPTH=64,
    // so a linear scan is faster than hashing at that size).
    // We use a small inline vec to avoid heap allocation for the visited check.
    // The visited set must include `root_id` (same as original seeding: visited = {root, to}).
    // At expansion time we check: is `to` == root_id || is `to` on the parent chain?

    let mut state_count = 0usize;
    let mut truncated = false;
    let mut incomplete = false;
    let mut depth_exceeded = false;

    // Reachability-directed pruning. A node can lie on a witness path to `fact`
    // ONLY if `fact` is reachable from it through the forward call graph.
    //
    // FIX 1 — precompute a `valid_nodes` set ONCE per call via REVERSE-GRAPH BFS:
    //
    // Correctness: `facts_by_routine[N]` (direct ∪ inherited) contains a fact
    // equivalent to `fact` IFF N can reach (forward) some node that carries `fact`
    // as a DIRECT fact.  Therefore the old per-node check
    //   `facts_by_routine[to].any(equiv fact)`
    // is EXACTLY:
    //   "to is an ancestor-or-equal of some carrier in the forward call graph"
    // which is the same as:
    //   "to is reachable in the REVERSE graph from the set of carriers".
    //
    // So we:
    //   1. Scan `direct_facts_by_routine` for carrier nodes (far fewer facts than
    //      the inherited cone — this is the only place `fact_equivalent` is called
    //      for the prune, over DIRECT facts only).
    //   2. Reverse-BFS from carriers over `idx.incoming_edges` (= reverse of the
    //      same `typed_edges` the forward BFS uses).
    //   3. Replace the per-edge `can_reach` lookup with O(1) `valid_nodes.contains`.
    //
    // This eliminates the `fact_equivalent` hot-spot (previously ~750 k calls/root
    // on the CDO app) while visiting the identical set of nodes.
    let valid_nodes: std::collections::HashSet<&str> = {
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut rev_queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        // Seed: nodes that carry `fact` as a DIRECT fact. `fact_equivalent` REQUIRES
        // `op` and `resource_kind` equal, so only the `(op, kind)`-bucket can hold a
        // carrier — look up that bucket instead of scanning every routine's direct
        // facts per call (the previous O(all-direct-facts)/call hot-spot).
        //
        // ⟨issue 32⟩ The temp-class check must match the TERMINAL check below.
        // The two mispairings are NOT symmetric, and it is worth being exact about
        // which one is dangerous, because the cheap-looking edit is the fatal one:
        //
        //   - Seed BROADER than the terminal (e.g. dropping this check alone):
        //     `valid_nodes` is a strict SUPERSET, the forward BFS explores more
        //     nodes, and every terminal it would have found it still finds. Costs
        //     states, never answers — measured, not argued: dropping this guard
        //     moved nothing (both r4 tests and all 76 `cli_b_*` goldens green).
        //     So this guard is a COST optimisation resting on a soundness
        //     precondition, not a correctness guard in its own right.
        //   - Seed NARROWER than the terminal (guarding here while relaxing the
        //     terminal below): a node whose only carrier is the other class is
        //     never marked valid, the forward BFS never reaches it or anything
        //     behind it, and a terminal the matcher WOULD have accepted is never
        //     found — the witness degrades to `terminal-not-found` silently (see
        //     the terminal guard's own note on how that failure is swallowed).
        //     This is the direction to protect: never tighten here without
        //     tightening there.
        //
        // The soundness precondition the pairing rests on: `retag`
        // (`capability_cone.rs`) copies `extra` through untouched, so every
        // INHERITED fact has a same-class DIRECT producer somewhere in its cone,
        // and the guarded seed therefore admits exactly the routines the guarded
        // terminal can stop at. That precondition is pinned executably by
        // `oracle_r3a3_inherited_keys_trace_to_a_direct_producer`
        // (`tests/r3/r3a3_oracles.rs`) — with the temp class in
        // `inherited_fact_key`, that oracle now asserts precisely "every inherited
        // fact traces to a SAME-CLASS direct producer". Stated limit: the oracle
        // runs over the source-only fixture corpus, so a cross-app scope mismatch
        // between the cone's node set and `snap.capability_facts` would still
        // degrade witnesses undetected.
        if let Some(cands) = idx
            .direct_facts_by_op_kind
            .get(&(fact.op.clone(), fact.resource_kind.clone()))
        {
            let fact_kt = is_known_temp_snap(fact);
            for d in cands {
                if is_known_temp_snap(d) == fact_kt
                    && fact_equivalent(d, fact)
                    && visited.insert(d.subject.as_str())
                {
                    rev_queue.push_back(d.subject.as_str());
                }
            }
        }
        // Reverse-BFS: walk backward through incoming_edges.
        while let Some(cur) = rev_queue.pop_front() {
            if let Some(preds) = idx.incoming_edges.get(cur) {
                for pred in preds {
                    if visited.insert(pred.as_str()) {
                        rev_queue.push_back(pred.as_str());
                    }
                }
            }
        }
        visited
    };

    while !queue.is_empty() && paths.len() < cap {
        let Some(ni) = queue.pop_front() else {
            break;
        };
        state_count += 1;
        if state_count > MAX_STATES {
            // state-limit-exceeded (detail=maxExpandedStates=N), incomplete:true.
            diagnostics.push((
                "state-limit-exceeded".to_string(),
                Some(format!("maxExpandedStates={MAX_STATES}")),
            ));
            incomplete = true;
            break;
        }
        // depth check: arena[ni].depth is the number of hops to this node.
        // Original check was `state.hops.len() > MAX_DEPTH`; hops.len() == depth here.
        if arena[ni].depth > MAX_DEPTH {
            depth_exceeded = true;
            continue;
        }

        // Snapshot the routine string we need for lookups (avoids borrow conflicts
        // when we later mutably push to arena).
        let routine = arena[ni].routine.clone();

        // Terminal check: FIRST matching direct fact in insertion order.
        //
        // ⟨issue 32⟩ ...of the SAME temp class. `fact_equivalent` compares op,
        // resourceKind, resourceId, resourceArgSource and (for dispatch) objectType
        // — never temp state — so without this a physical fact would happily
        // terminate on a routine's known-temp write of the same table. The
        // occurrence would then be GRADED physical (from the fact) and ANCHORED at
        // an in-memory operation (from the terminal), which is how d47 comes to
        // report `WRITE_PENDING_AT_EXTERNAL_IO` for a `Temp.Insert(); Http.Get();
        // PhysWriter()` body. The class is total (absent temp state = not
        // known-temp), so this narrows the match without ever leaving it undefined,
        // and the reverse-BFS prune seed above applies the identical test.
        //
        // The guard lives HERE, not inside `fact_equivalent`: that predicate is
        // deliberately undefined-tolerant on `resource_id`/`resourceArgSource`, and
        // it is also used for the prune seed, where widening it would change which
        // nodes are walked for every fact rather than only the ones that can
        // mis-terminate.
        //
        // SOUNDNESS PRECONDITION, and why narrowing the match cannot lose a
        // terminal: `retag` (`capability_cone.rs`) copies `extra` through
        // untouched, so every INHERITED fact has a same-class DIRECT producer
        // somewhere in its cone. Pinned executably by
        // `oracle_r3a3_inherited_keys_trace_to_a_direct_producer`
        // (`tests/r3/r3a3_oracles.rs`), which — now that the temp class is in
        // `inherited_fact_key` — asserts exactly that. Stated limit: that oracle
        // runs over the source-only fixture corpus only.
        //
        // WHEN IT DOES FAIL, IT FAILS SILENTLY. `reconstruct_witness_paths` sets
        // `incomplete = true` and pushes `terminal-not-found`, but the digest path
        // never stores `WitnessOutcomeExt.incomplete` on `AccumulatedEffect` (only
        // `reconstruct_witness_paths_pub` forwards it). The occurrence then carries
        // no `evidence_operation_id`/`evidence_callsite_id`, `compute_ordering`
        // leaves `ordered_op = None`, and the effect contributes NO ordering fact —
        // so a d45/d47-class finding vanishes rather than the run failing loudly.
        //
        // UNMEASURED REGRESSION VECTOR ⟨review N9⟩: this guard can only push a
        // terminal DEEPER (the nearest equivalent direct fact may now be the wrong
        // class). A witness that used to terminate shallow and now terminates deep
        // could in principle cross `MAX_DEPTH` / `MAX_STATES` / `HARD_PATH_CAP` and
        // degrade exactly as above. The fixture corpus shows none, and the paired
        // seed means the producer is always reachable, but the `incomplete` /
        // `terminal-not-found` counts on a real workspace were never compared
        // before and after, and no CDO-gated ratchet covers the witness layer.
        let directs: &[&Fact] = idx
            .direct_facts_by_routine
            .get(&routine)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let fact_kt = is_known_temp_snap(fact);
        if let Some(equivalent) = directs
            .iter()
            .find(|d| is_known_temp_snap(d) == fact_kt && fact_equivalent(d, fact))
        {
            let terminal = terminal_hop_from_fact(equivalent, idx);
            let mut hops = reconstruct_hops(&arena, ni);
            hops.push(terminal);
            paths.push(WitnessPath { hops });
            continue;
        }
        // Opaque-or-unresolved-boundary: no out, no directs, coverage.directStatus=="unknown".
        let routine_out_len = idx
            .outgoing_edges
            .get(&routine)
            .map(|v| v.len())
            .unwrap_or(0);
        let cov_unknown = idx
            .coverage_by_routine
            .get(&routine)
            .map(|c| c.direct_status == "unknown")
            .unwrap_or(false);
        if routine_out_len == 0 && directs.is_empty() && cov_unknown {
            // opaque-or-unresolved-boundary (detail = routineId).
            diagnostics.push((
                "opaque-or-unresolved-boundary".to_string(),
                Some(routine.clone()),
            ));
            // Original: if !state.hops.is_empty() — depth>=1 means always non-empty here.
            // (Seeds have depth=1, so hops reconstructed are always ≥1 element.)
            let hops = reconstruct_hops(&arena, ni);
            if !hops.is_empty() {
                paths.push(WitnessPath { hops });
            }
            continue;
        }
        // Expand: out-edges are PRE-SORTED by `edge_compare` at index build, skip
        // visited. (No per-state clone+sort — see `build_fingerprint_indexes`.)
        //
        // Build the per-path visited set by walking the parent chain of `ni`.
        // Depth ≤ MAX_DEPTH (64), so cloning `depth+1` Strings once per popped node
        // (shared across all out-edge checks of this node) is O(depth), not O(depth *
        // out_degree) as in the old code.  The critical saving is that we no longer
        // clone the visited set AND hops per edge expansion — only once per node pop.
        // Includes root_id (matching original: seeded visited = {root, to}).
        let visited_routines: Vec<&str> = {
            let mut v: Vec<&str> = Vec::with_capacity(arena[ni].depth + 1);
            v.push(root_id);
            let mut cur = ni;
            while cur != 0 {
                v.push(arena[cur].routine.as_str());
                cur = arena[cur].parent;
            }
            v
        };
        let cur_depth = arena[ni].depth;

        // Collect expansions: (to_string, hop) pairs.  We build the list while
        // `visited_routines` is still live (read-only arena access), then push to
        // arena after the borrow ends.
        let mut expansions: Vec<(String, WitnessHop)> = Vec::new();
        {
            let sorted = idx
                .outgoing_edges
                .get(&routine)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            for &edge in sorted {
                let Some(to) = edge_to(edge) else {
                    continue;
                };
                if visited_routines.contains(&to) {
                    continue;
                }
                // Reachability prune: skip targets not in the precomputed valid set.
                if !valid_nodes.contains(to) {
                    continue;
                }
                let Some(hop) = edge_to_hop(edge, idx) else {
                    continue;
                };
                expansions.push((to.to_string(), hop));
            }
        }
        // Now push children to arena (mutable); `visited_routines` is still live here
        // but we only read `cur_depth` from it (a Copy value captured above).
        for (to, hop) in expansions {
            let child_idx = arena.len();
            arena.push(Node {
                routine: to,
                hop,
                parent: ni,
                depth: cur_depth + 1,
            });
            queue.push_back(child_idx);
        }
    }

    if paths.len() >= cap && !queue.is_empty() {
        truncated = true;
        diagnostics.push(("path-limit-reached".to_string(), Some(format!("cap={cap}"))));
    }
    if depth_exceeded {
        // depth-exceeded (detail=maxDepth=N), incomplete:true.
        diagnostics.push((
            "depth-exceeded".to_string(),
            Some(format!("maxDepth={MAX_DEPTH}")),
        ));
        incomplete = true;
    }
    if paths.is_empty() && !incomplete {
        // terminal-not-found: TS flows through the projection `default` → kind only, NO detail.
        diagnostics.push(("terminal-not-found".to_string(), None));
        incomplete = true;
    }

    // FINAL path sort: shortest-first, then JSON.stringify(raw WitnessHop[]) ordinal tiebreak.
    paths.sort_by(|a, b| {
        if a.hops.len() != b.hops.len() {
            return a.hops.len().cmp(&b.hops.len());
        }
        witness_hops_json(&a.hops).cmp(&witness_hops_json(&b.hops))
    });

    WitnessOutcomeExt {
        paths,
        truncated,
        incomplete,
        diagnostics,
    }
}

// ===========================================================================
// hop-projection.ts — QueryWitnessHop + projectPath
// ===========================================================================

/// `QueryWitnessHop` — the projected hop. Field order = the TS literal order in
/// `projectHop` PER variant (see hop-projection.ts). Custom Serialize emits in
/// declaration order, None/undefined OMITTED.
#[derive(Debug, Clone)]
pub struct QueryWitnessHop {
    pub kind: &'static str,
    pub from_routine_id: String,
    pub from_display: String,
    pub to_routine_id: Option<String>,
    pub to_display: Option<String>,
    pub callee_display: Option<String>,
    pub callsite_id: Option<String>,
    pub event_id: Option<String>,
    pub target_app_guid: Option<String>,
    pub edge_kind: Option<String>,
    pub anchor: Option<HopAnchor>,
    pub receiver_type: Option<String>,
    pub interface_name: Option<String>,
    pub candidate_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct HopAnchor {
    pub file: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

fn hop_anchor(
    source_file: &Option<String>,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<HopAnchor> {
    let file = source_file.as_ref()?;
    // normalizeAnchorPath(file, workspaceRoot): `ws:`-prefixed paths never start with
    // the absolute workspace root → returned verbatim (with `\` → `/`, no-op for ASCII corpus).
    Some(HopAnchor {
        file: file.replace('\\', "/"),
        line,
        column,
    })
}

/// `projectHop` (hop-projection.ts). Terminal hops → None (evidence, not edges).
fn project_hop(
    hop: &WitnessHop,
    from_id: &str,
    from_display: &str,
    idx: &FingerprintIndexes,
) -> Option<QueryWitnessHop> {
    let resolve_to_display = |routine_id: &str, hop_routine_display: &str| -> String {
        idx.routine_display_by_id
            .get(routine_id)
            .cloned()
            .unwrap_or_else(|| hop_routine_display.to_string())
    };

    match hop {
        WitnessHop::Terminal { .. } => None,
        WitnessHop::Call {
            routine_id,
            routine_display,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => Some(QueryWitnessHop {
            kind: "call",
            from_routine_id: from_id.to_string(),
            from_display: from_display.to_string(),
            to_routine_id: Some(routine_id.clone()),
            to_display: Some(resolve_to_display(routine_id, routine_display)),
            callee_display: Some(callee_display.clone()),
            callsite_id: Some(callsite_id.clone()),
            event_id: None,
            target_app_guid: None,
            edge_kind: Some("direct-call".to_string()),
            anchor: hop_anchor(source_file, *line, *column),
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
        }),
        WitnessHop::ObjectRun {
            routine_id,
            routine_display,
            target_display,
            resolved,
            callsite_id,
            source_file,
            line,
            column,
            ..
        } => Some(QueryWitnessHop {
            kind: "object-run",
            from_routine_id: from_id.to_string(),
            from_display: from_display.to_string(),
            to_routine_id: if *resolved {
                Some(routine_id.clone())
            } else {
                None
            },
            to_display: if *resolved {
                Some(resolve_to_display(routine_id, routine_display))
            } else {
                None
            },
            callee_display: target_display.clone(),
            callsite_id: callsite_id.clone(),
            event_id: None,
            target_app_guid: None,
            edge_kind: Some(
                if *resolved {
                    "object-run-resolved"
                } else {
                    "object-run-unresolved"
                }
                .to_string(),
            ),
            anchor: hop_anchor(source_file, *line, *column),
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
        }),
        WitnessHop::EventDispatch {
            routine_id,
            routine_display,
            event_id,
            ..
        } => Some(QueryWitnessHop {
            kind: "event-dispatch",
            from_routine_id: from_id.to_string(),
            from_display: from_display.to_string(),
            to_routine_id: Some(routine_id.clone()),
            to_display: Some(resolve_to_display(routine_id, routine_display)),
            callee_display: None,
            callsite_id: None,
            event_id: Some(event_id.clone()),
            target_app_guid: None,
            edge_kind: Some("event-dispatch".to_string()),
            anchor: None,
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
        }),
        WitnessHop::VariableTypedCall {
            routine_id,
            routine_display,
            receiver_type,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => Some(QueryWitnessHop {
            kind: "variable-typed-call",
            from_routine_id: from_id.to_string(),
            from_display: from_display.to_string(),
            to_routine_id: Some(routine_id.clone()),
            to_display: Some(resolve_to_display(routine_id, routine_display)),
            callee_display: callee_display.clone(),
            callsite_id: Some(callsite_id.clone()),
            event_id: None,
            target_app_guid: None,
            edge_kind: Some("variable-typed-call".to_string()),
            anchor: hop_anchor(source_file, *line, *column),
            receiver_type: Some(receiver_type.clone()),
            interface_name: None,
            candidate_count: None,
        }),
        WitnessHop::InterfaceDispatch {
            routine_id,
            routine_display,
            interface_name,
            candidate_count,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => Some(QueryWitnessHop {
            kind: "interface-dispatch",
            from_routine_id: from_id.to_string(),
            from_display: from_display.to_string(),
            to_routine_id: Some(routine_id.clone()),
            to_display: Some(resolve_to_display(routine_id, routine_display)),
            callee_display: callee_display.clone(),
            callsite_id: Some(callsite_id.clone()),
            event_id: None,
            target_app_guid: None,
            edge_kind: Some("interface-dispatch".to_string()),
            anchor: hop_anchor(source_file, *line, *column),
            receiver_type: None,
            interface_name: Some(interface_name.clone()),
            candidate_count: Some(*candidate_count),
        }),
    }
}

/// `projectPath` (hop-projection.ts). Terminal hops dropped; from-chains head-to-tail.
fn project_path(
    path: &WitnessPath,
    root_id: &str,
    root_display: &str,
    idx: &FingerprintIndexes,
) -> Vec<QueryWitnessHop> {
    let mut hops: Vec<QueryWitnessHop> = Vec::new();
    let mut prev_destination: String = root_id.to_string();

    for hop in &path.hops {
        if matches!(hop, WitnessHop::Terminal { .. }) {
            continue;
        }
        let from_id = prev_destination.clone();
        let from_display = if from_id == root_id {
            root_display.to_string()
        } else {
            idx.routine_display_by_id
                .get(&from_id)
                .cloned()
                .unwrap_or_else(|| from_id.clone())
        };
        if let Some(projected) = project_hop(hop, &from_id, &from_display, idx) {
            hops.push(projected);
        }
        // Advance: hop.routineId is the edge destination for every non-terminal hop.
        if let Some(rid) = hop.routine_id() {
            prev_destination = rid.to_string();
        }
    }

    hops
}

// ===========================================================================
// JSON.stringify TIEBREAK SERIALIZERS — byte-identical to V8 JSON.stringify.
// No whitespace, declaration field order, None/undefined OMITTED.
// ===========================================================================

/// JSON-escape a string per V8 JSON.stringify (standard JSON escaping).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `JSON.stringify(ValueSource)` — for factEquivalent's resourceArgSource compare.
/// Field order = the snapshot SnapValueSource declaration order (= al-sem ValueSource).
fn value_source_json(vs: &SnapValueSource) -> String {
    match vs {
        SnapValueSource::Literal { value } => {
            format!("{{\"kind\":\"literal\",\"value\":{}}}", json_escape(value))
        }
        SnapValueSource::Enum { enum_name, member } => {
            let mut s = format!(
                "{{\"kind\":\"enum\",\"enumName\":{}",
                json_escape(enum_name)
            );
            if let Some(m) = member {
                s.push_str(&format!(",\"member\":{}", json_escape(m)));
            }
            s.push('}');
            s
        }
        SnapValueSource::ConstantVar {
            var_name,
            initializer,
        } => {
            format!(
                "{{\"kind\":\"constant-var\",\"varName\":{},\"initializer\":{}}}",
                json_escape(var_name),
                value_source_json(initializer)
            )
        }
        SnapValueSource::Parameter { index, var_name } => {
            format!(
                "{{\"kind\":\"parameter\",\"index\":{},\"varName\":{}}}",
                index,
                json_escape(var_name)
            )
        }
        SnapValueSource::TableField {
            table_id,
            field_name,
        } => {
            format!(
                "{{\"kind\":\"table-field\",\"tableId\":{},\"fieldName\":{}}}",
                json_escape(table_id),
                json_escape(field_name)
            )
        }
        SnapValueSource::Expression => "{\"kind\":\"expression\"}".to_string(),
        SnapValueSource::Unknown => "{\"kind\":\"unknown\"}".to_string(),
    }
}

/// `JSON.stringify(WitnessHop[])` — RAW witness hops, for the witness final-sort tiebreak.
/// Field order = the TS WitnessHop union literal order per variant (witness.ts). Optional
/// (undefined) fields OMITTED. Used ONLY for sort stability (never emitted to golden).
fn witness_hops_json(hops: &[WitnessHop]) -> String {
    let mut s = String::from("[");
    for (i, hop) in hops.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&witness_hop_json(hop));
    }
    s.push(']');
    s
}

fn opt_num(s: &mut String, written: &mut bool, key: &str, v: Option<u32>) {
    if let Some(n) = v {
        if *written {
            s.push(',');
        }
        s.push_str(&format!("{}:{}", json_escape(key), n));
        *written = true;
    }
}

fn opt_str(s: &mut String, written: &mut bool, key: &str, v: &Option<String>) {
    if let Some(val) = v {
        if *written {
            s.push(',');
        }
        s.push_str(&format!("{}:{}", json_escape(key), json_escape(val)));
        *written = true;
    }
}

fn req_str(s: &mut String, written: &mut bool, key: &str, v: &str) {
    if *written {
        s.push(',');
    }
    s.push_str(&format!("{}:{}", json_escape(key), json_escape(v)));
    *written = true;
}

fn witness_hop_json(hop: &WitnessHop) -> String {
    let mut s = String::from("{");
    let mut w = false;
    match hop {
        // WitnessHop union literal field order (witness.ts):
        WitnessHop::Call {
            routine_id,
            routine_display,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => {
            req_str(&mut s, &mut w, "kind", "call");
            req_str(&mut s, &mut w, "routineId", routine_id);
            req_str(&mut s, &mut w, "routineDisplay", routine_display);
            req_str(&mut s, &mut w, "calleeDisplay", callee_display);
            req_str(&mut s, &mut w, "callsiteId", callsite_id);
            opt_str(&mut s, &mut w, "sourceFile", source_file);
            opt_num(&mut s, &mut w, "line", *line);
            opt_num(&mut s, &mut w, "column", *column);
        }
        WitnessHop::ObjectRun {
            routine_id,
            routine_display,
            target_object_id,
            target_display,
            resolved,
            callsite_id,
            source_file,
            line,
            column,
        } => {
            req_str(&mut s, &mut w, "kind", "object-run");
            req_str(&mut s, &mut w, "routineId", routine_id);
            req_str(&mut s, &mut w, "routineDisplay", routine_display);
            opt_str(&mut s, &mut w, "targetObjectId", target_object_id);
            opt_str(&mut s, &mut w, "targetDisplay", target_display);
            // resolved: boolean (always present)
            if w {
                s.push(',');
            }
            s.push_str(&format!("{}:{}", json_escape("resolved"), resolved));
            w = true;
            opt_str(&mut s, &mut w, "callsiteId", callsite_id);
            opt_str(&mut s, &mut w, "sourceFile", source_file);
            opt_num(&mut s, &mut w, "line", *line);
            opt_num(&mut s, &mut w, "column", *column);
        }
        WitnessHop::EventDispatch {
            routine_id,
            routine_display,
            event_id,
            event_display,
        } => {
            req_str(&mut s, &mut w, "kind", "event-dispatch");
            req_str(&mut s, &mut w, "routineId", routine_id);
            req_str(&mut s, &mut w, "routineDisplay", routine_display);
            req_str(&mut s, &mut w, "eventId", event_id);
            req_str(&mut s, &mut w, "eventDisplay", event_display);
        }
        WitnessHop::VariableTypedCall {
            routine_id,
            routine_display,
            receiver_type,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => {
            req_str(&mut s, &mut w, "kind", "variable-typed-call");
            req_str(&mut s, &mut w, "routineId", routine_id);
            req_str(&mut s, &mut w, "routineDisplay", routine_display);
            req_str(&mut s, &mut w, "receiverType", receiver_type);
            opt_str(&mut s, &mut w, "calleeDisplay", callee_display);
            req_str(&mut s, &mut w, "callsiteId", callsite_id);
            opt_str(&mut s, &mut w, "sourceFile", source_file);
            opt_num(&mut s, &mut w, "line", *line);
            opt_num(&mut s, &mut w, "column", *column);
        }
        WitnessHop::InterfaceDispatch {
            routine_id,
            routine_display,
            interface_name,
            candidate_count,
            callee_display,
            callsite_id,
            source_file,
            line,
            column,
        } => {
            req_str(&mut s, &mut w, "kind", "interface-dispatch");
            req_str(&mut s, &mut w, "routineId", routine_id);
            req_str(&mut s, &mut w, "routineDisplay", routine_display);
            req_str(&mut s, &mut w, "interfaceName", interface_name);
            if w {
                s.push(',');
            }
            s.push_str(&format!(
                "{}:{}",
                json_escape("candidateCount"),
                candidate_count
            ));
            w = true;
            opt_str(&mut s, &mut w, "calleeDisplay", callee_display);
            req_str(&mut s, &mut w, "callsiteId", callsite_id);
            opt_str(&mut s, &mut w, "sourceFile", source_file);
            opt_num(&mut s, &mut w, "line", *line);
            opt_num(&mut s, &mut w, "column", *column);
        }
        WitnessHop::Terminal {
            evidence_kind,
            operation_id,
            callsite_id,
            display_text,
            source_file,
            line,
            column,
        } => {
            req_str(&mut s, &mut w, "kind", "terminal");
            let ek = match evidence_kind {
                TerminalKind::Operation => "operation",
                TerminalKind::Callsite => "callsite",
                TerminalKind::Synthetic => "synthetic",
            };
            req_str(&mut s, &mut w, "evidenceKind", ek);
            opt_str(&mut s, &mut w, "operationId", operation_id);
            opt_str(&mut s, &mut w, "callsiteId", callsite_id);
            req_str(&mut s, &mut w, "displayText", display_text);
            opt_str(&mut s, &mut w, "sourceFile", source_file);
            opt_num(&mut s, &mut w, "line", *line);
            opt_num(&mut s, &mut w, "column", *column);
        }
    }
    s.push('}');
    s
}

/// `JSON.stringify(QueryWitnessHop[])` — projected hops, for the digest merge tiebreak
/// + exact-dup dedupe. Field order = the QueryWitnessHop literal per projectHop variant.
///
/// Optional (undefined) fields OMITTED.
fn query_hops_json(hops: &[QueryWitnessHop]) -> String {
    let mut s = String::from("[");
    for (i, hop) in hops.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&query_hop_json(hop));
    }
    s.push(']');
    s
}

fn query_hop_json(hop: &QueryWitnessHop) -> String {
    // Field order = QueryWitnessHop literal in projectHop (per-kind, but the object
    // literal lists the same field sequence). projectHop builds each variant in the
    // order: kind, fromRoutineId, fromDisplay, toRoutineId, toDisplay, calleeDisplay,
    // callsiteId, eventId, targetAppGuid, edgeKind, receiverType, interfaceName,
    // candidateCount, anchor — BUT the exact V8 order is the literal order PER variant.
    // The TS literals place `anchor` LAST in call/object-run/var-typed/iface/dep-export
    // and `edgeKind` last in event-dispatch (no anchor). We reproduce per-kind below.
    let mut s = String::from("{");
    let mut w = false;
    match hop.kind {
        "call" => {
            req_str(&mut s, &mut w, "kind", "call");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "calleeDisplay", &hop.callee_display);
            opt_str(&mut s, &mut w, "callsiteId", &hop.callsite_id);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        "object-run" => {
            req_str(&mut s, &mut w, "kind", "object-run");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "calleeDisplay", &hop.callee_display);
            opt_str(&mut s, &mut w, "callsiteId", &hop.callsite_id);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        "event-dispatch" => {
            req_str(&mut s, &mut w, "kind", "event-dispatch");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "eventId", &hop.event_id);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
        }
        "implicit-trigger" => {
            req_str(&mut s, &mut w, "kind", "implicit-trigger");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        "dependency-export" => {
            req_str(&mut s, &mut w, "kind", "dependency-export");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "calleeDisplay", &hop.callee_display);
            opt_str(&mut s, &mut w, "callsiteId", &hop.callsite_id);
            opt_str(&mut s, &mut w, "targetAppGuid", &hop.target_app_guid);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        "variable-typed-call" => {
            req_str(&mut s, &mut w, "kind", "variable-typed-call");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "calleeDisplay", &hop.callee_display);
            opt_str(&mut s, &mut w, "callsiteId", &hop.callsite_id);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_str(&mut s, &mut w, "receiverType", &hop.receiver_type);
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        "interface-dispatch" => {
            req_str(&mut s, &mut w, "kind", "interface-dispatch");
            req_str(&mut s, &mut w, "fromRoutineId", &hop.from_routine_id);
            req_str(&mut s, &mut w, "fromDisplay", &hop.from_display);
            opt_str(&mut s, &mut w, "toRoutineId", &hop.to_routine_id);
            opt_str(&mut s, &mut w, "toDisplay", &hop.to_display);
            opt_str(&mut s, &mut w, "calleeDisplay", &hop.callee_display);
            opt_str(&mut s, &mut w, "callsiteId", &hop.callsite_id);
            opt_str(&mut s, &mut w, "edgeKind", &hop.edge_kind);
            opt_str(&mut s, &mut w, "interfaceName", &hop.interface_name);
            if let Some(cc) = hop.candidate_count {
                if w {
                    s.push(',');
                }
                s.push_str(&format!("{}:{}", json_escape("candidateCount"), cc));
                w = true;
            }
            opt_anchor(&mut s, &mut w, "anchor", &hop.anchor);
        }
        _ => {}
    }
    s.push('}');
    s
}

fn opt_anchor(s: &mut String, written: &mut bool, key: &str, anchor: &Option<HopAnchor>) {
    if let Some(a) = anchor {
        if *written {
            s.push(',');
        }
        // SourceAnchorContract literal order: sourceKind, file, line, column (line/column
        // optional). normalizeAnchorPath produces { sourceKind:"source", file, line, column }.
        let mut inner = String::from("{");
        let mut iw = false;
        req_str(&mut inner, &mut iw, "sourceKind", "source");
        req_str(&mut inner, &mut iw, "file", &a.file);
        opt_num(&mut inner, &mut iw, "line", a.line);
        opt_num(&mut inner, &mut iw, "column", a.column);
        inner.push('}');
        s.push_str(&format!("{}:{}", json_escape(key), inner));
        *written = true;
    }
}

// ===========================================================================
// digest-query.ts — digestQuery driver (per-root effect build, dedupe, merge)
// + ordering-engine.ts occurrence-build slice.
// ===========================================================================

/// The terminal hop of a path (LAST terminal in hops, scanning back).
fn find_terminal(hops: &[WitnessHop]) -> Option<&WitnessHop> {
    hops.iter()
        .rev()
        .find(|h| matches!(h, WitnessHop::Terminal { .. }))
}

/// SourceAnchorContract — the evidence form. `unavailable` when no citable anchor.
#[derive(Debug, Clone)]
struct SourceAnchorContract {
    source_kind: &'static str, // "source" | "unavailable"
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    excerpt: Option<String>,
}

/// `evidenceFromTerminalHop` (digest-query.ts).
fn evidence_from_terminal(terminal: Option<&WitnessHop>) -> SourceAnchorContract {
    match terminal {
        Some(WitnessHop::Terminal {
            source_file: Some(sf),
            line,
            column,
            display_text,
            ..
        }) => SourceAnchorContract {
            source_kind: "source",
            file: Some(sf.replace('\\', "/")),
            line: *line,
            column: *column,
            excerpt: Some(display_text.clone()),
        },
        _ => SourceAnchorContract {
            source_kind: "unavailable",
            file: None,
            line: None,
            column: None,
            excerpt: None,
        },
    }
}

/// `dedupeKey` (digest-query.ts).
fn dedupe_key(
    effect_type: &str,
    terminal: Option<&WitnessHop>,
    fact: &Fact,
    detail: &[(String, String)],
) -> String {
    let anchor_id = match terminal {
        Some(WitnessHop::Terminal {
            operation_id: Some(op),
            ..
        }) => format!("op:{op}"),
        Some(WitnessHop::Terminal {
            callsite_id: Some(cs),
            ..
        }) => format!("cs:{cs}"),
        Some(WitnessHop::Terminal {
            source_file: Some(sf),
            line,
            ..
        }) => {
            format!("file:{}:{}", sf, line.unwrap_or(0))
        }
        _ => format!(
            "synthetic:{}:{}:{}",
            fact.op,
            fact.resource_kind,
            fact.resource_id.clone().unwrap_or_default()
        ),
    };
    let resource_id = fact.resource_id.clone().unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}",
        effect_type,
        anchor_id,
        fact.resource_kind,
        resource_id,
        detail_json(detail)
    )
}

/// `JSON.stringify(Record<string,string>)` — insertion-ordered detail object.
fn detail_json(detail: &[(String, String)]) -> String {
    let mut s = String::from("{");
    for (i, (k, v)) in detail.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{}:{}", json_escape(k), json_escape(v)));
    }
    s.push('}');
    s
}

/// Identity of a fact's ENTIRE `digest_one_root` loop contribution. Two facts with
/// equal keys produce byte-identical witness outcomes, projections, dedupe keys, and
/// merge inputs — so every occurrence after the first is a provable no-op on
/// `effect_map` (the merge dedupes identical paths by hops-JSON and merges
/// temp_state/truncation idempotently). Skipping them changes nothing in the output
/// while removing the measured ~2.3× duplicate-fact cost (witness-investigation.md §3).
///
/// Fields, and why each is here (grepped every `fact.`/`fact_equivalent` use inside
/// `digest_one_root`'s call tree — see witness-investigation.md §3 and the brief):
///   - `provenance`, `witness_operation_id`, `witness_callsite_id`, `op`, `subject` —
///     `reconstruct_witness_paths`'s direct BFS-input fields (cases A/B/C dispatch).
///   - `resource_kind`, `resource_id` — read by `reconstruct_witness_paths` (Case-C
///     reachability seeding via `direct_facts_by_op_kind`) AND by `dedupe_key`.
///   - `resource_arg_source` (serialized) — read by `fact_equivalent(d, fact)`
///     (called on the CURRENT fact as `b`, both in the `valid_nodes` reverse-BFS
///     seed AND the terminal-match check inside the Case-C BFS) — an easy field to
///     miss because it lives OUTSIDE `extra`, but it changes `fact_equivalent`'s
///     result and therefore the BFS's reachable-node set.
///   - `extra` (serialized) — carries `dispatch`'s `objectType` (also read by
///     `fact_equivalent`) and `table`'s `temp_state` (read directly in the loop for
///     the physical-write filter), plus every other extra field the (currently
///     unread-elsewhere) variants carry — over-included defensively.
///   - `detail` (the computed `effect_detail_of` output) — folded into `dedupe_key`'s
///     grouping key, so two facts differing only in a detail field must NOT collapse.
///
/// `confidence`/`via` are NOT included: `via` is read only for a diagnostics detail
/// string, and `confidence` is not read anywhere in `digest_one_root`'s call tree;
/// neither affects `effect_map`, and duplicate outcomes' `diagnostics`/`incomplete`
/// are NOT consumed by `digest_one_root` (only `reconstruct_witness_paths_pub`
/// forwards them, a different caller) — omitting them only lowers the dedup factor
/// if they ever start mattering, never causes incorrect output.
fn effect_fact_loop_identity(fact: &Fact, detail: &[(String, String)]) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
        fact.provenance,
        fact.op,
        fact.resource_kind,
        fact.resource_id.as_deref().unwrap_or(""),
        fact.witness_callsite_id.as_deref().unwrap_or(""),
        fact.witness_operation_id.as_deref().unwrap_or(""),
        fact.subject,
        serde_json::to_string(&fact.resource_arg_source).unwrap_or_default(),
        serde_json::to_string(&fact.extra).unwrap_or_default(),
        detail_json(detail),
    )
}

/// `buildCanonicalOccurrenceKey` (ordering-engine.ts) — link-signature from viaPaths[0].
fn build_canonical_key(
    root_routine_id: &str,
    via_paths: &[Vec<QueryWitnessHop>],
    terminal_evidence_kind: &str,
    terminal_evidence_id: &str,
    effect_type: &str,
) -> (String, String) {
    let mut link_signature = String::new();
    if let Some(first_path) = via_paths.first()
        && !first_path.is_empty()
    {
        let segments: Vec<String> = first_path
            .iter()
            .map(|hop| {
                // QueryWitnessHop has NO edgeId → "". Trailing slot also "".
                format!(
                    "{}>{}@{}/{}//",
                    hop.from_routine_id,
                    hop.to_routine_id.clone().unwrap_or_default(),
                    hop.callsite_id.clone().unwrap_or_default(),
                    hop.kind
                )
            })
            .collect();
        link_signature = segments.join(",");
    }

    let canonical_key = [
        root_routine_id,
        link_signature.as_str(),
        terminal_evidence_kind,
        terminal_evidence_id,
        effect_type,
    ]
    .join("|");

    (canonical_key, link_signature)
}

/// `buildOccurrenceIdFromKey` (ordering-engine.ts) — ordinal 0 in practice.
fn occurrence_id_from_key(canonical_key: &str, ordinal: u32) -> String {
    let raw = if ordinal == 0 {
        canonical_key.to_string()
    } else {
        format!("{canonical_key}#{ordinal}")
    };
    sha256_hex(&raw)[..16].to_string()
}

// ---------------------------------------------------------------------------
// Result types (the R4-F golden shape).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DigestEffectResult {
    pub effect_type: String,
    pub detail: Vec<(String, String)>,
    pub provenance: &'static str,
    pub evidence: ProjectedEvidence,
    pub evidence_operation_id: Option<String>,
    pub evidence_callsite_id: Option<String>,
    pub via_paths: Vec<Vec<ProjectedHop>>,
    pub via_paths_truncated: bool,
    /// Effect conditionality, computed PER-PATH from each raw witness path's OWN
    /// terminal hop (digest-query.ts computePathConditionality), folded with
    /// effectConditionality across all pre-capping paths. (#8 — was previously
    /// recomputed downstream from the effect-level shortest-path terminal only.)
    pub conditionality: crate::engine::l5::conditionality::EffectConditionality,
    pub fact_id: String,
    /// The originating CapabilityFact's subject stable id — used for the
    /// evidence-unavailable diagnostic's `factSubject` field.
    pub fact_subject: String,
    pub canonical_key: String,
    pub link_signature: String,
    /// Sort key fields (evidence.file, evidence.line) — for the effects sort.
    sort_file: String,
    sort_line: u32,
    /// S4-internal (NOT serialized in the digest-effects golden): tempState fed to
    /// the ordering engine's physical-write filter.
    pub temp_state: Option<SnapTempState>,
    /// S4: per-effect scoped guarantees (attached by `compute_ordering`).
    pub scoped_guarantees: Vec<crate::engine::l5::ordering_engine::ScopedGuarantee>,
}

#[derive(Debug, Clone)]
pub struct ProjectedEvidence {
    pub source_kind: &'static str,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
}

/// A projected hop carried into the result (already V8-field-ordered on serialize).
#[derive(Debug, Clone)]
pub struct ProjectedHop {
    pub inner: QueryWitnessHop,
}

#[derive(Debug, Clone)]
pub struct DigestEntryResult {
    pub routine_id: String,
    pub effects: Vec<DigestEffectResult>,
}

/// The callsiteId of a NON-terminal witness hop (the `callsiteId?` field TS reads on
/// the hop). EventDispatch has none. Terminal hops are handled separately.
fn hop_nonterminal_callsite_id(hop: &WitnessHop) -> Option<&str> {
    match hop {
        WitnessHop::Call { callsite_id, .. }
        | WitnessHop::VariableTypedCall { callsite_id, .. }
        | WitnessHop::InterfaceDispatch { callsite_id, .. } => Some(callsite_id),
        WitnessHop::ObjectRun { callsite_id, .. } => callsite_id.as_deref(),
        WitnessHop::EventDispatch { .. } => None,
        WitnessHop::Terminal { .. } => None,
    }
}

/// `computePathConditionality` (digest-query.ts) — conditionality of ONE raw witness
/// path, derived from each hop's OWN context. The terminal context comes from THIS
/// path's terminal hop (its operationId, else callsiteId), NOT the effect-level
/// shortest-path terminal. This is the #8 fix: per-path terminal threading.
fn compute_path_conditionality(
    path: &WitnessPath,
    cs_ctx: &HashMap<&str, Option<&str>>,
    op_ctx: &HashMap<&str, Option<&str>>,
) -> crate::engine::l5::conditionality::EffectConditionality {
    use crate::engine::l5::conditionality::{
        UNKNOWN, context_to_conditionality, path_conditionality,
    };
    let mut hop_contexts: Vec<crate::engine::l5::conditionality::EffectConditionality> = Vec::new();
    let mut terminal_ctx = UNKNOWN;
    for hop in &path.hops {
        match hop {
            WitnessHop::Terminal {
                operation_id,
                callsite_id,
                ..
            } => {
                terminal_ctx = if let Some(op) = operation_id {
                    context_to_conditionality(op_ctx.get(op.as_str()).copied().flatten())
                } else if let Some(cs) = callsite_id {
                    context_to_conditionality(cs_ctx.get(cs.as_str()).copied().flatten())
                } else {
                    UNKNOWN
                };
            }
            _ => {
                if let Some(csid) = hop_nonterminal_callsite_id(hop) {
                    hop_contexts.push(context_to_conditionality(
                        cs_ctx.get(csid).copied().flatten(),
                    ));
                } else {
                    hop_contexts.push(UNKNOWN);
                }
            }
        }
    }
    path_conditionality(&hop_contexts, terminal_ctx)
}

/// `digestQuery` (digest-query.ts) — per-root effect build. Roots in input order;
/// entries sorted by routineId at the end. `return_summaries` + `isolated_event_ids`
/// drive the S4 ordering engine (compute_ordering) — None for the S3-only path.
///
/// `ordering_witness_only`: when `true` (the ordering-only path), skip
/// `reconstruct_witness_paths` for effect types that the ordering engine never
/// grades.  The ordering engine only looks at: DB_INSERT, DB_MODIFY, DB_DELETE,
/// COMMIT, HTTP, FILE, UI_CONFIRM, UI_MESSAGE, UI_WINDOW_OPEN, ERROR_THROW.
/// Effects outside that set are still emitted (digest shape unchanged) but with
/// empty `via_paths` — the ordering engine ignores them at line ~294 of
/// ordering_engine.rs (`if via_paths.is_empty() { if owner != routine_id { return
/// None; } return Some(... chain: empty ...); }`).  This eliminates ~80% of the
/// witness-reconstruction work from `compute_digest_effects_for_ordering`.
fn digest_query(
    snap: &CapabilitySnapshot,
    roots: &[String],
    return_summaries: Option<&HashMap<String, crate::engine::return_summary::RoutineReturnSummary>>,
    isolated_event_ids: Option<&std::collections::HashSet<String>>,
    ordering_witness_only: bool,
) -> Vec<DigestEntryResult> {
    let idx = build_fingerprint_indexes(snap);

    // callsiteById (&str-keyed) for the ordering engine's cross-hop substrate.
    let mut callsite_by_id_str: HashMap<&str, &SnapshotCallsiteEvidence> = HashMap::new();
    for cs in &snap.callsite_index {
        callsite_by_id_str.insert(cs.callsite_id.as_str(), cs);
    }

    // controlContext lookups for per-path conditionality (#8). Built once.
    let cs_ctx: HashMap<&str, Option<&str>> = snap
        .callsite_index
        .iter()
        .map(|cs| (cs.callsite_id.as_str(), cs.control_context.as_deref()))
        .collect();
    let op_ctx: HashMap<&str, Option<&str>> = snap
        .operation_index
        .iter()
        .map(|op| (op.operation_id.as_str(), op.control_context.as_deref()))
        .collect();

    // Per-root computation is embarrassingly parallel: every input below is an
    // immutable `&` reference (snap/idx/maps), each root's witness reconstruction +
    // ordering pass is independent and internally deterministic, and `roots` is
    // deduped so `routine_id` keys are unique — the final `sort_by(routine_id)`
    // below fully determines output order regardless of scheduling order. Runs on
    // the GLOBAL rayon pool (no AL-source lowering happens here — the big-stack pool
    // is only for the CST lowerer; witness BFS is heap-based with MAX_DEPTH = 64).
    use rayon::prelude::*;
    let mut entries: Vec<DigestEntryResult> = roots
        .par_iter()
        .filter_map(|rid| {
            digest_one_root(
                rid,
                snap,
                &idx,
                &callsite_by_id_str,
                &cs_ctx,
                &op_ctx,
                return_summaries,
                isolated_event_ids,
                ordering_witness_only,
            )
        })
        .collect();

    // Sort entries by routineId.
    entries.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    entries
}

/// tempState of a fact's originating table-write (the physical-write filter's
/// input) — a plain field projection off `fact.extra`, NOT part of the expensive
/// witness-BFS half of `digest_one_root`'s loop, so it stays cheap to recompute for
/// an identity-duplicate fact whose BFS reconstruction is skipped.
fn fact_temp_state_of(fact: &Fact) -> Option<SnapTempState> {
    match &fact.extra {
        Some(SnapCapabilityExtra::Table { temp_state, .. }) => temp_state.clone(),
        _ => None,
    }
}

/// The snapshot-layer twin of `l4::cone_derived::fact_is_known_temp`, over a
/// `SnapTempState` rather than an L4 `PTempState`: true only for the exact
/// `known/true` signal. Absent, `Unknown` and parameter-dependent temp state all
/// read as NOT known-temp, exactly as the L4 predicate treats them — so the
/// class is TOTAL, never undefined.
fn is_known_temp_state(ts: Option<&SnapTempState>) -> bool {
    matches!(ts, Some(SnapTempState::Known { value: true }))
}

/// `is_known_temp_state` over a whole snapshot fact.
///
/// ⟨issue 32 / issue 33⟩ The witness terminal matcher `fact_equivalent` does not
/// compare temp state at all, while the ordering engine grades an occurrence
/// physical-or-not from the FACT's temp state. Those are two different objects,
/// so a physical fact can terminate on a temp operation and produce an
/// occurrence that is graded physical while anchored at an in-memory write.
///
/// **This was already reachable before issue 33 — the guard fixes a LIVE bug and
/// issue 33 only widens its reach.** It is tempting to say the cone's key
/// collapse masked it (a physical representative that shares a temp one's key
/// was discarded, so the pairing never arose), but `fact_equivalent` is strictly
/// WEAKER than `inherited_fact_key`: it ignores `confidence` entirely and treats
/// `resource_id: None` as a WILDCARD, comparing rids only when both are `Some`.
/// So a direct fact can be a terminal candidate for a fact it never shared a
/// cone key with. That wildcard is sufficient on its own — no ordering argument
/// is needed, and an earlier draft of this comment gave one that was FALSE (it
/// claimed a `None`-rid fact sorts FIRST; `capability_fact_sort_key` joins the
/// parts with `|` = 0x7C, which is above every byte a real rid starts with, so
/// an empty rid field sorts LAST). Shape on master, with no mixed cone key
/// anywhere:
///
/// ```text
/// Y:  TempRec.Insert()   // table unresolvable -> rid None, confidence
///                        //   "unresolved", temp_state known/true
///     Z()
/// Z:  Rec.Insert()       // table T, confidence "static", known/false
/// R:  Y()
/// ```
///
/// `R` inherits the physical `insert` of `T`; the BFS reaches `Y` first; `Y`'s
/// ONLY direct fact is the `None`-rid known-temp one, so there is nothing for
/// the scan to prefer; `fact_equivalent` returns true because the rid is
/// unconstrained, and the occurrence is graded physical from the FACT while
/// anchored at the in-memory op. Consequence for review and triage: the
/// guard is NOT inert on existing output — it can move `evidence` /
/// `evidence_operation_id` → `dedupe_key` → `canonical_key` → `occurrence_id`
/// for facts that exist today, and the paired seed guard shrinks `valid_nodes`,
/// which can drop `via_paths` and so change `build_canonical_key`'s link
/// signature. A "no golden moved" result cannot rule that out, because the
/// golden corpus contains zero mixed keys by measurement.
fn is_known_temp_snap(fact: &Fact) -> bool {
    is_known_temp_state(fact_temp_state_of(fact).as_ref())
}

/// The MERGE branch's tempState combination rule, extracted so the identity-duplicate
/// normalization path (see `digest_one_root`) can re-apply it exactly. Conservative:
/// stays known-temp only if BOTH sides are known-temp; otherwise degrades to the new
/// side once the existing side is no longer trusted-known. NOT assumed idempotent —
/// an intervening, different-identity fact sharing the same `dedupe_key` may have
/// changed the accumulator's `temp_state` since this identity's own first
/// contribution, so callers must always pass the CURRENT stored value, never skip
/// the recomputation.
fn merge_temp_state(
    existing: &Option<SnapTempState>,
    new: &Option<SnapTempState>,
) -> Option<SnapTempState> {
    let is_known_temp = |t: &Option<SnapTempState>| is_known_temp_state(t.as_ref());
    if is_known_temp(existing) && is_known_temp(new) {
        existing.clone()
    } else if is_known_temp(existing) {
        new.clone()
    } else {
        existing.clone()
    }
}

/// The MERGE branch's via-paths normalization, extracted so it can be shared between
/// a live merge (a genuinely new fact's projected paths, `new_*` non-empty) and the
/// identity-duplicate normalization path (`new_*` empty — see `digest_one_root`'s
/// identity-duplicate handling). Combines `existing_*` with `new_*` into
/// (path, conditionality, json) triples, stable-sorts by `(projected_len, json)`,
/// dedupes by json (first occurrence wins), then caps `via_paths` at `max_paths` and
/// ORs in `had_truncation`. This is byte-for-byte the inline logic the MERGE branch
/// used to run — extracting it does not change any output.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn merge_normalize_via_paths(
    existing_paths: &[Vec<QueryWitnessHop>],
    existing_conds: &[crate::engine::l5::conditionality::EffectConditionality],
    existing_jsons: &[String],
    existing_had_truncation: bool,
    new_paths: &[Vec<QueryWitnessHop>],
    new_conds: &[crate::engine::l5::conditionality::EffectConditionality],
    new_jsons: &[String],
    new_truncated: bool,
    max_paths: usize,
) -> (
    Vec<Vec<QueryWitnessHop>>,
    bool,
    Vec<Vec<QueryWitnessHop>>,
    Vec<crate::engine::l5::conditionality::EffectConditionality>,
    Vec<String>,
) {
    let mut merged: Vec<(
        Vec<QueryWitnessHop>,
        crate::engine::l5::conditionality::EffectConditionality,
        String,
    )> = Vec::with_capacity(existing_paths.len() + new_paths.len());
    for (i, p) in existing_paths.iter().enumerate() {
        let c = existing_conds
            .get(i)
            .copied()
            .unwrap_or(crate::engine::l5::conditionality::UNKNOWN);
        let j = existing_jsons
            .get(i)
            .cloned()
            .unwrap_or_else(|| query_hops_json(p));
        merged.push((p.clone(), c, j));
    }
    for ((p, c), j) in new_paths
        .iter()
        .cloned()
        .zip(new_conds.iter().copied())
        .zip(new_jsons.iter().cloned())
    {
        merged.push((p, c, j));
    }
    merged.sort_by(|a, b| {
        if a.0.len() != b.0.len() {
            return a.0.len().cmp(&b.0.len());
        }
        a.2.cmp(&b.2)
    });
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unique_paths: Vec<Vec<QueryWitnessHop>> = Vec::new();
    let mut unique_conds: Vec<crate::engine::l5::conditionality::EffectConditionality> = Vec::new();
    let mut unique_jsons: Vec<String> = Vec::new();
    for (p, c, j) in merged {
        if seen.insert(j.clone()) {
            unique_paths.push(p);
            unique_conds.push(c);
            unique_jsons.push(j);
        }
    }
    let had_truncation = existing_had_truncation || new_truncated || unique_paths.len() > max_paths;
    let via: Vec<Vec<QueryWitnessHop>> = unique_paths.iter().take(max_paths).cloned().collect();
    (
        via,
        had_truncation,
        unique_paths,
        unique_conds,
        unique_jsons,
    )
}

/// Per-identity dedup state (see `effect_fact_loop_identity`'s doc and
/// `digest_one_root`'s identity-duplicate handling).
enum IdentitySeen {
    /// First occurrence's contribution landed at this `effect_map` position (via
    /// either a fresh insert or a live merge into a pre-existing entry from a
    /// DIFFERENT identity sharing the same `dedupe_key`).
    First(usize),
    /// A duplicate has already been merge-normalized into the entry above; the
    /// normalized list is a fixed point for any further identical duplicate (see
    /// the `IdentitySeen::First` match arm's comment in `digest_one_root`).
    Normalized,
}

/// Per-root body of `digest_query` — extracted so the root loop can be driven by
/// `rayon::par_iter`. Pure over its immutable inputs; returns `None` when the root
/// has no display entry (mirrors the original loop's early `continue`).
#[allow(clippy::too_many_arguments)]
fn digest_one_root(
    rid: &str,
    snap: &CapabilitySnapshot,
    idx: &FingerprintIndexes<'_>,
    callsite_by_id_str: &HashMap<&str, &SnapshotCallsiteEvidence>,
    cs_ctx: &HashMap<&str, Option<&str>>,
    op_ctx: &HashMap<&str, Option<&str>>,
    return_summaries: Option<&HashMap<String, crate::engine::return_summary::RoutineReturnSummary>>,
    isolated_event_ids: Option<&std::collections::HashSet<String>>,
    ordering_witness_only: bool,
) -> Option<DigestEntryResult> {
    const MAX_PATHS: usize = 3;
    {
        let display = idx.routine_display_by_id.get(rid).cloned()?;

        let empty_facts: Vec<&Fact> = Vec::new();
        let all_facts = idx.facts_by_routine.get(rid).unwrap_or(&empty_facts);

        // Effect-fact filter: drop op=="execute"&&dispatch; keep iff effectTypeOf!=None.
        let effect_facts: Vec<&Fact> = all_facts
            .iter()
            .copied()
            .filter(|f| {
                if f.op == "execute"
                    && matches!(&f.extra, Some(SnapCapabilityExtra::Dispatch { .. }))
                {
                    return false;
                }
                effect_type_of(f).is_some()
            })
            .collect();

        // effectMap (IndexMap, insertion order) — ordered Vec of (key, AccumulatedEffect).
        struct AccumulatedEffect {
            effect_type: &'static str,
            detail: Vec<(String, String)>,
            provenance: &'static str,
            evidence: SourceAnchorContract,
            evidence_operation_id: Option<String>,
            evidence_callsite_id: Option<String>,
            via_paths: Vec<Vec<QueryWitnessHop>>,
            had_truncation: bool,
            all_paths: Vec<Vec<QueryWitnessHop>>,
            /// Per-path conditionality, parallel to `all_paths` (PRE-capping). Computed
            /// from each raw path's own terminal hop (#8).
            all_path_conds: Vec<crate::engine::l5::conditionality::EffectConditionality>,
            /// `query_hops_json` of each `all_paths[i]`, parallel to `all_paths` —
            /// computed once at projection/merge and reused for every later merge's
            /// sort tiebreak + dedupe (removes the repeated re-serialization the
            /// giant tail roots paid; see witness-investigation.md §3).
            all_path_jsons: Vec<String>,
            /// S4-internal (NOT serialized in the digest-effects golden): the
            /// originating table-write fact's tempState (for the physical-write filter).
            temp_state: Option<SnapTempState>,
            /// The fact's subject (stable id) — used for the evidence-unavailable diagnostic.
            fact_subject: String,
        }
        let mut effect_map: Vec<(String, AccumulatedEffect)> = Vec::new();
        // O(1) key → position index over effect_map (same keys, same positions —
        // the Vec keeps insertion order for output; this only replaces the O(F)
        // `iter().position()` scan that made the loop O(F²) on 1500-fact roots).
        let mut effect_index: HashMap<String, usize> = HashMap::new();
        // Merge-identity dedup (see `effect_fact_loop_identity`'s doc): facts whose
        // ENTIRE consumed field set is equal produce a byte-identical loop
        // contribution, so the expensive witness BFS is skipped for every occurrence
        // after the first. The FIRST duplicate additionally re-normalizes the
        // target accumulator entry (see the `IdentitySeen::First` match arm below)
        // to reproduce the old code's MERGE-branch normalization that a duplicate
        // would otherwise have triggered — see review finding in
        // .superpowers/sdd/alsem-parallel/wit-task-2-fix-report.md.
        let mut identity_seen: HashMap<String, IdentitySeen> = HashMap::new();

        // Effect types the ordering engine grades (FIX 2). When `ordering_witness_only`
        // is set we skip witness reconstruction for effect types outside this set —
        // the ordering engine drops effects with empty via_paths whose owner != root
        // and treats same-root effects as direct (empty CallChain). This eliminates
        // ~80% of witness-reconstruction calls from the ordering-only digest path.
        const ORDERING_RELEVANT: &[&str] = &[
            "DB_INSERT",
            "DB_MODIFY",
            "DB_DELETE",
            "COMMIT",
            "HTTP",
            "FILE",
            "UI_CONFIRM",
            "UI_MESSAGE",
            "UI_WINDOW_OPEN",
            "ERROR_THROW",
        ];

        for fact in &effect_facts {
            let Some(effect_type) = effect_type_of(fact) else {
                continue;
            };
            let detail = effect_detail_of(fact, &idx.stable_id_to_display);
            let identity = effect_fact_loop_identity(fact, &detail);

            // Merge-identity dedup — see effect_fact_loop_identity's doc AND
            // IdentitySeen's doc. Must run BEFORE reconstruct_witness_paths (the
            // expensive half); keeps insertion order unchanged (the target
            // accumulator entry's POSITION never moves, only its stored fields).
            match identity_seen.get(&identity) {
                Some(IdentitySeen::Normalized) => {
                    // SECOND+ duplicate: the entry was already merge-normalized by
                    // the FIRST duplicate below. Merging an identical subset of
                    // paths into an already sorted+deduped list is a provable no-op
                    // (the sorted+deduped list is a fixed point for its own
                    // members), and `merge_temp_state` re-applied with the SAME
                    // tempState input twice in a row is also a no-op (see that
                    // function's doc) — so there is nothing left to do here.
                    continue;
                }
                Some(&IdentitySeen::First(pos)) => {
                    // FIRST duplicate of this identity: reproduce, WITHOUT
                    // re-running the expensive BFS, the state transition the OLD
                    // code's MERGE branch would have applied here (this is the
                    // review finding this block fixes). The duplicate's own path
                    // set is a proven subset of the entry's current `all_paths`
                    // (identical BFS inputs ⇒ identical outcome — see
                    // `effect_fact_loop_identity`'s doc), so re-sorting/deduping the
                    // EXISTING paths alone (no new paths appended) yields exactly
                    // what stable-sorting + json-deduping (existing ∪ duplicate)
                    // would have produced: the duplicate's entries are exact
                    // (len, json) ties with ones already in `existing`, appear
                    // strictly AFTER them in the concatenation the old code built,
                    // and a stable sort never reorders equal-key ties — so they are
                    // always absorbed by the dedupe pass and never survive it.
                    let acc = &mut effect_map[pos].1;
                    let (via, had_truncation, all_paths, all_path_conds, all_path_jsons) =
                        merge_normalize_via_paths(
                            &acc.all_paths,
                            &acc.all_path_conds,
                            &acc.all_path_jsons,
                            acc.had_truncation,
                            &[],
                            &[],
                            &[],
                            false,
                            MAX_PATHS,
                        );
                    acc.via_paths = via;
                    acc.had_truncation = had_truncation;
                    acc.all_paths = all_paths;
                    acc.all_path_conds = all_path_conds;
                    acc.all_path_jsons = all_path_jsons;
                    // temp_state is NOT assumed idempotent (unlike the paths above):
                    // a different identity sharing this entry's dedupe_key may have
                    // mutated it since this identity's first occurrence, so it is
                    // cheaply recomputed for real with the SAME merge formula and
                    // this duplicate's own (non-BFS, plain field) tempState.
                    let dup_temp_state = fact_temp_state_of(fact);
                    acc.temp_state = merge_temp_state(&acc.temp_state, &dup_temp_state);

                    identity_seen.insert(identity, IdentitySeen::Normalized);
                    continue;
                }
                None => {}
            }

            // FIX 2: skip expensive witness reconstruction for effect types the
            // ordering engine never grades when called from the ordering-only path.
            let outcome = if ordering_witness_only && !ORDERING_RELEVANT.contains(&effect_type) {
                WitnessOutcomeExt {
                    paths: Vec::new(),
                    truncated: false,
                    incomplete: false,
                    diagnostics: Vec::new(),
                }
            } else {
                reconstruct_witness_paths(rid, fact, idx, HARD_PATH_CAP)
            };

            let shortest = outcome.paths.first();
            let terminal = shortest.and_then(|p| find_terminal(&p.hops));

            let evidence = evidence_from_terminal(terminal);
            let evidence_operation_id = match terminal {
                Some(WitnessHop::Terminal {
                    operation_id: Some(op),
                    ..
                }) => Some(op.clone()),
                _ => None,
            };
            let evidence_callsite_id = match terminal {
                Some(WitnessHop::Terminal {
                    callsite_id: Some(cs),
                    ..
                }) => Some(cs.clone()),
                _ => None,
            };

            // Project all paths to QueryWitnessHop[][] AND compute each raw path's
            // conditionality from its OWN terminal hop (#8 — must be done before
            // projection, which strips terminals).
            let projected_paths: Vec<Vec<QueryWitnessHop>> = outcome
                .paths
                .iter()
                .map(|p| project_path(p, rid, &display, idx))
                .collect();
            let path_conds: Vec<crate::engine::l5::conditionality::EffectConditionality> = outcome
                .paths
                .iter()
                .map(|p| compute_path_conditionality(p, cs_ctx, op_ctx))
                .collect();
            let projected_jsons: Vec<String> =
                projected_paths.iter().map(|p| query_hops_json(p)).collect();

            let key = dedupe_key(effect_type, terminal, fact, &detail);

            // tempState of the originating table-write fact (physical-write filter).
            let fact_temp_state: Option<SnapTempState> = fact_temp_state_of(fact);

            // Find existing in ordered effect_map.
            let existing_pos = effect_index.get(&key).copied();

            let final_pos = if let Some(pos) = existing_pos {
                // Merge: combine (path, cond, json) triples, sort shortest-first + JSON
                // tiebreak, dedupe exact dups by hops-JSON. Conds/jsons travel WITH
                // their paths (#8). Delegates to `merge_normalize_via_paths`, the SAME
                // helper the identity-duplicate normalization path above uses, so the
                // two can never drift apart.
                let existing = &effect_map[pos].1;
                let (via, had_truncation, all_paths, all_path_conds, all_path_jsons) =
                    merge_normalize_via_paths(
                        &existing.all_paths,
                        &existing.all_path_conds,
                        &existing.all_path_jsons,
                        existing.had_truncation,
                        &projected_paths,
                        &path_conds,
                        &projected_jsons,
                        outcome.truncated,
                        MAX_PATHS,
                    );
                let merged_temp = merge_temp_state(&existing.temp_state, &fact_temp_state);
                let acc = &mut effect_map[pos].1;
                acc.via_paths = via;
                acc.had_truncation = had_truncation;
                acc.all_paths = all_paths;
                acc.all_path_conds = all_path_conds;
                acc.all_path_jsons = all_path_jsons;
                acc.temp_state = merged_temp;
                pos
            } else {
                let via: Vec<Vec<QueryWitnessHop>> =
                    projected_paths.iter().take(MAX_PATHS).cloned().collect();
                let had_truncation = outcome.truncated || projected_paths.len() > MAX_PATHS;
                let pos = effect_map.len();
                effect_index.insert(key.clone(), pos);
                effect_map.push((
                    key,
                    AccumulatedEffect {
                        effect_type,
                        detail,
                        provenance: if fact.provenance == "direct" {
                            "direct"
                        } else {
                            "transitive"
                        },
                        evidence,
                        evidence_operation_id,
                        evidence_callsite_id,
                        via_paths: via,
                        had_truncation,
                        all_paths: projected_paths,
                        all_path_conds: path_conds,
                        all_path_jsons: projected_jsons,
                        temp_state: fact_temp_state,
                        fact_subject: fact.subject.clone(),
                    },
                ));
                pos
            };

            // Record where this identity's own (first) contribution landed, so a
            // later duplicate can normalize the SAME entry without re-running BFS
            // — see `IdentitySeen`'s doc and the match above.
            identity_seen.insert(identity, IdentitySeen::First(final_pos));
        }

        // Materialize effects from effectMap.values() insertion order.
        let mut effects: Vec<DigestEffectResult> = Vec::new();
        for (_key, acc) in effect_map {
            // Occurrence-build (canonical key + factId).
            let terminal_evidence_kind = if acc.evidence_operation_id.is_some() {
                "operation"
            } else {
                "callsite"
            };
            let terminal_evidence_id = acc
                .evidence_operation_id
                .clone()
                .or_else(|| acc.evidence_callsite_id.clone())
                .unwrap_or_default();

            let (canonical_key, link_signature) = build_canonical_key(
                rid,
                &acc.via_paths,
                terminal_evidence_kind,
                &terminal_evidence_id,
                acc.effect_type,
            );

            let sort_file = acc.evidence.file.clone().unwrap_or_default();
            let sort_line = acc.evidence.line.unwrap_or(0);

            // effectConditionality across ALL pre-capping paths (#8).
            let conditionality = crate::engine::l5::conditionality::effect_conditionality(
                &acc.all_path_conds,
                acc.had_truncation,
            );

            effects.push(DigestEffectResult {
                effect_type: acc.effect_type.to_string(),
                detail: acc.detail,
                provenance: acc.provenance,
                evidence: ProjectedEvidence {
                    source_kind: acc.evidence.source_kind,
                    file: acc.evidence.file,
                    line: acc.evidence.line,
                    column: acc.evidence.column,
                    excerpt: acc.evidence.excerpt,
                },
                evidence_operation_id: acc.evidence_operation_id,
                evidence_callsite_id: acc.evidence_callsite_id,
                via_paths: acc
                    .via_paths
                    .into_iter()
                    .map(|p| p.into_iter().map(|h| ProjectedHop { inner: h }).collect())
                    .collect(),
                via_paths_truncated: acc.had_truncation,
                conditionality,
                fact_id: String::new(), // filled below (after sort, via seenCanonicalKeys)
                fact_subject: acc.fact_subject,
                canonical_key,
                link_signature,
                sort_file,
                sort_line,
                temp_state: acc.temp_state,
                scoped_guarantees: Vec::new(),
            });
        }

        // Sort by (type, evidence.file ?? "", evidence.line ?? 0).
        effects.sort_by(|a, b| {
            if a.effect_type != b.effect_type {
                return a.effect_type.cmp(&b.effect_type);
            }
            if a.sort_file != b.sort_file {
                return a.sort_file.cmp(&b.sort_file);
            }
            a.sort_line.cmp(&b.sort_line)
        });

        // Occurrence-build: seenCanonicalKeys (canonicalKey → first occurrenceId).
        let mut seen_canonical_keys: Vec<(String, String)> = Vec::new();
        for eff in &mut effects {
            let existing = seen_canonical_keys
                .iter()
                .find(|(k, _)| k == &eff.canonical_key);
            let occ_id = if let Some((_, id)) = existing {
                id.clone()
            } else {
                let id = occurrence_id_from_key(&eff.canonical_key, 0);
                seen_canonical_keys.push((eff.canonical_key.clone(), id.clone()));
                id
            };
            eff.fact_id = occ_id;
        }

        // S4: compute_ordering — ALWAYS runs the ordering engine (mirrors TS digestQuery which
        // always calls computeOrdering regardless of whether routineReturnSummaries is provided).
        // return_summaries is forwarded as-is to compute_ordering; when None, the engine
        // degrades gracefully (checkCalleeReturnability → "ok"; errorEscapesChain → false).
        {
            // The ordering engine consumes each effect's conditionality (for
            // COMMIT_ON_SUCCESS_PATH). It is the SAME per-path value already stored on
            // the effect (#17 — the duplicated effect-level closure is gone).
            let ordering_inputs: Vec<crate::engine::l5::ordering_engine::OrderingEffectInput> =
                effects
                    .iter()
                    .map(
                        |e| crate::engine::l5::ordering_engine::OrderingEffectInput {
                            effect_type: e.effect_type.clone(),
                            evidence_operation_id: e.evidence_operation_id.clone(),
                            evidence_callsite_id: e.evidence_callsite_id.clone(),
                            via_paths: e
                                .via_paths
                                .iter()
                                .map(|p| p.iter().map(|h| h.inner.clone()).collect())
                                .collect(),
                            via_paths_truncated: e.via_paths_truncated,
                            temp_state: e.temp_state.clone(),
                            occurrence_id: e.fact_id.clone(),
                            conditionality: e.conditionality,
                        },
                    )
                    .collect();
            let scoped = crate::engine::l5::ordering_engine::compute_ordering(
                rid,
                &ordering_inputs,
                snap,
                callsite_by_id_str,
                return_summaries,
                isolated_event_ids,
            );
            for (i, eff) in effects.iter_mut().enumerate() {
                if let Some(sg) = scoped.get(i) {
                    eff.scoped_guarantees = sg.clone();
                }
            }
        }

        Some(DigestEntryResult {
            routine_id: rid.to_string(),
            effects,
        })
    }
}

// ===========================================================================
// Reportable roots (ordering-facts.ts isReportableRoutine + roots build).
// ===========================================================================

/// Roots = stable ids of reportable workspace routines, deduped + sorted.
/// `isReportableRoutine` = primary && body_available && !parse_incomplete. In the
/// source-only corpus every workspace routine is "primary" (no dependency role).
fn reportable_roots(resolved: &L3Resolved) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for r in &resolved.workspace.routines {
        // isReportableRoutine = body_available && !parse_incomplete (primary is implicit
        // in the source-only corpus). De Morgan of `!(body_available && !parse_incomplete)`.
        if !r.body_available || r.parse_incomplete {
            continue;
        }
        if r.stable_routine_id.is_empty() {
            continue;
        }
        roots.push(r.stable_routine_id.clone());
    }
    // dedupe + sort.
    roots.sort();
    roots.dedup();
    roots
}

// ===========================================================================
// R4-F STABLE PROJECTION — project_r4f_digest_effects.
// Top-level + per-effect key order MIRRORS the al-sem golden EXACTLY.
// ===========================================================================

/// Ordered evidence (SourceAnchorContract) serialize — sourceKind, [file], [line],
/// [column], [excerpt]; "unavailable" emits ONLY sourceKind.
struct EvidenceSer<'a>(&'a ProjectedEvidence);
impl Serialize for EvidenceSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let e = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("sourceKind", e.source_kind)?;
        if let Some(f) = &e.file {
            map.serialize_entry("file", f)?;
        }
        if let Some(l) = e.line {
            map.serialize_entry("line", &l)?;
        }
        if let Some(c) = e.column {
            map.serialize_entry("column", &c)?;
        }
        if let Some(x) = &e.excerpt {
            map.serialize_entry("excerpt", x)?;
        }
        map.end()
    }
}

/// Ordered detail (Record<string,string>) serialize — insertion order.
struct DetailSer<'a>(&'a [(String, String)]);
impl Serialize for DetailSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// Ordered QueryWitnessHop serialize — per-variant V8 field order (= projectHop literal).
struct HopSer<'a>(&'a QueryWitnessHop);
impl Serialize for HopSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let h = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("kind", h.kind)?;
        map.serialize_entry("fromRoutineId", &h.from_routine_id)?;
        map.serialize_entry("fromDisplay", &h.from_display)?;
        if let Some(v) = &h.to_routine_id {
            map.serialize_entry("toRoutineId", v)?;
        }
        if let Some(v) = &h.to_display {
            map.serialize_entry("toDisplay", v)?;
        }
        match h.kind {
            "event-dispatch" => {
                if let Some(v) = &h.event_id {
                    map.serialize_entry("eventId", v)?;
                }
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
            }
            "implicit-trigger" => {
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
                anchor_entry(&mut map, &h.anchor)?;
            }
            "dependency-export" => {
                if let Some(v) = &h.callee_display {
                    map.serialize_entry("calleeDisplay", v)?;
                }
                if let Some(v) = &h.callsite_id {
                    map.serialize_entry("callsiteId", v)?;
                }
                if let Some(v) = &h.target_app_guid {
                    map.serialize_entry("targetAppGuid", v)?;
                }
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
                anchor_entry(&mut map, &h.anchor)?;
            }
            "variable-typed-call" => {
                if let Some(v) = &h.callee_display {
                    map.serialize_entry("calleeDisplay", v)?;
                }
                if let Some(v) = &h.callsite_id {
                    map.serialize_entry("callsiteId", v)?;
                }
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
                if let Some(v) = &h.receiver_type {
                    map.serialize_entry("receiverType", v)?;
                }
                anchor_entry(&mut map, &h.anchor)?;
            }
            "interface-dispatch" => {
                if let Some(v) = &h.callee_display {
                    map.serialize_entry("calleeDisplay", v)?;
                }
                if let Some(v) = &h.callsite_id {
                    map.serialize_entry("callsiteId", v)?;
                }
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
                if let Some(v) = &h.interface_name {
                    map.serialize_entry("interfaceName", v)?;
                }
                if let Some(v) = h.candidate_count {
                    map.serialize_entry("candidateCount", &v)?;
                }
                anchor_entry(&mut map, &h.anchor)?;
            }
            // call / object-run
            _ => {
                if let Some(v) = &h.callee_display {
                    map.serialize_entry("calleeDisplay", v)?;
                }
                if let Some(v) = &h.callsite_id {
                    map.serialize_entry("callsiteId", v)?;
                }
                if let Some(v) = &h.edge_kind {
                    map.serialize_entry("edgeKind", v)?;
                }
                anchor_entry(&mut map, &h.anchor)?;
            }
        }
        map.end()
    }
}

fn anchor_entry<M: serde::ser::SerializeMap>(
    map: &mut M,
    anchor: &Option<HopAnchor>,
) -> Result<(), M::Error> {
    if let Some(a) = anchor {
        map.serialize_entry("anchor", &AnchorSer(a))?;
    }
    Ok(())
}

struct AnchorSer<'a>(&'a HopAnchor);
impl Serialize for AnchorSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let a = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("sourceKind", "source")?;
        map.serialize_entry("file", &a.file)?;
        if let Some(l) = a.line {
            map.serialize_entry("line", &l)?;
        }
        if let Some(c) = a.column {
            map.serialize_entry("column", &c)?;
        }
        map.end()
    }
}

/// Public helper for digest_cli: convert a `QueryWitnessHop` to a `serde_json::Value`
/// using the same field ordering as `HopSer`. Used by `project_digest_document`.
pub fn hop_to_json_value(hop: &QueryWitnessHop) -> serde_json::Value {
    serde_json::to_value(HopSer(hop)).unwrap_or(serde_json::Value::Null)
}

/// Ordered effect serialize — FIXED key order: type, detail, provenance, evidence,
/// [evidenceOperationId], [evidenceCallsiteId], viaPaths, viaPathsTruncated, factId,
/// canonicalKey, linkSignature.
struct EffectSer<'a>(&'a DigestEffectResult);
impl Serialize for EffectSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let e = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &e.effect_type)?;
        map.serialize_entry("detail", &DetailSer(&e.detail))?;
        map.serialize_entry("provenance", e.provenance)?;
        map.serialize_entry("evidence", &EvidenceSer(&e.evidence))?;
        if let Some(op) = &e.evidence_operation_id {
            map.serialize_entry("evidenceOperationId", op)?;
        }
        if let Some(cs) = &e.evidence_callsite_id {
            map.serialize_entry("evidenceCallsiteId", cs)?;
        }
        // viaPaths: Vec<Vec<ProjectedHop>>.
        let via: Vec<Vec<HopSer>> = e
            .via_paths
            .iter()
            .map(|p| p.iter().map(|h| HopSer(&h.inner)).collect())
            .collect();
        map.serialize_entry("viaPaths", &via)?;
        map.serialize_entry("viaPathsTruncated", &e.via_paths_truncated)?;
        map.serialize_entry("factId", &e.fact_id)?;
        map.serialize_entry("canonicalKey", &e.canonical_key)?;
        map.serialize_entry("linkSignature", &e.link_signature)?;
        map.end()
    }
}

struct EntrySer<'a>(&'a DigestEntryResult);
impl Serialize for EntrySer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let e = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("routineId", &e.routine_id)?;
        let effs: Vec<EffectSer> = e.effects.iter().map(EffectSer).collect();
        map.serialize_entry("effects", &effs)?;
        map.end()
    }
}

struct ProjectionSer<'a> {
    fixture_name: &'a str,
    entries: &'a [DigestEntryResult],
}
impl Serialize for ProjectionSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("fixtureName", self.fixture_name)?;
        map.serialize_entry("entryCount", &self.entries.len())?;
        let entries: Vec<EntrySer> = self.entries.iter().map(EntrySer).collect();
        map.serialize_entry("entries", &entries)?;
        map.end()
    }
}

/// Compute the per-root digest effects for a resolved source-only workspace
/// (S3 path — NO ordering engine; scopedGuarantees stay empty).
pub fn compute_digest_effects(resolved: &L3Resolved) -> Vec<DigestEntryResult> {
    let snap = compose_snapshot(resolved);
    let roots = reportable_roots(resolved);
    digest_query(&snap, &roots, None, None, false)
}

/// Compute the per-root digest effects WITH S4 ordering (scopedGuarantees attached).
/// Mirrors `computeOrderingFacts`: composeSnapshot + computeReturnSummaries +
/// isolatedEventIds + digestQuery(order:false).
///
/// Used by R4-F tests and any caller that needs summaries. For the CLI-B digest pipeline
/// use `compute_digest_effects_cli` instead (matches TS `runDigestPipeline` which does
/// NOT pass routineReturnSummaries to digestQuery).
pub fn compute_digest_effects_with_ordering(resolved: &L3Resolved) -> Vec<DigestEntryResult> {
    let snap = compose_snapshot(resolved);
    let roots = reportable_roots(resolved);
    let summaries = crate::engine::return_summary::compute_return_summaries(
        &resolved.workspace.routines,
        Some(&resolved.workspace.objects),
    );
    let isolated = crate::engine::l3::event_graph::isolated_event_ids(&resolved.workspace.routines);
    let isolated_opt = if isolated.is_empty() {
        None
    } else {
        Some(&isolated)
    };
    digest_query(&snap, &roots, Some(&summaries), isolated_opt, false)
}

/// Like [`compute_digest_effects_with_ordering`], but restricted to the roots whose
/// capability cone carries an IO/UI effect (op → `HTTP` / `FILE` / `UI_CONFIRM` /
/// `UI_MESSAGE` / `UI_WINDOW_OPEN`). Every ordering label
/// [`is_relevant_label`](crate::engine::l5::ordering_facts) gates on an io-occurrence
/// of exactly one of those types, so a root with NO such effect in its cone produces
/// ZERO ordering facts. Skipping those roots BEFORE the (expensive) per-effect witness
/// reconstruction is behavior-IDENTICAL for `compute_ordering_facts` (which only
/// retains non-empty fact sets) while collapsing the witness-reconstruction work from
/// "every reportable root" down to "roots that actually do IO/UI". The general
/// `compute_digest_effects_with_ordering` (consumed by the R4-F digest/scoped-guarantee
/// projections) is left unfiltered so those projections are unaffected.
pub fn compute_digest_effects_for_ordering(resolved: &L3Resolved) -> Vec<DigestEntryResult> {
    let snap = compose_snapshot(resolved);
    let all_roots = reportable_roots(resolved);

    // Per the 5 ordering labels (`is_relevant_label`), a root can produce an ordering
    // fact ONLY if its cone carries the label's ingredients. Re-derived from the spec
    // §4.2 grading table: FOUR labels (WRITE_PENDING_AT_EXTERNAL_IO,
    // EXTERNAL_IO_BEFORE_COMMIT, IO_BEFORE_ESCAPING_ERROR, EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN)
    // require an EXTERNAL IO (HTTP/FILE); the FIFTH (WRITE_PENDING_AT_UI) requires a
    // window-opening UI sink (UI_*) AND a pending DB write (INSERT/MODIFY/DELETE). So a
    // root keeps iff its cone has `external-io` OR (`ui-sink` AND `db-write`). All other
    // roots produce EMPTY ordering facts, so skipping their (expensive) witness
    // reconstruction is behavior-identical for `compute_ordering_facts` (which only
    // retains non-empty fact sets). `capability_facts` includes INHERITED facts
    // (subject = the cone owner) with the original op preserved, so cone presence is a
    // flat lookup. A correct SUPERSET of the producing-root set (ingredient presence,
    // not order — the engine still decides the actual order).
    let mut ext_io: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut ui_sink: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut db_write: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for f in &snap.capability_facts {
        match map_op(&f.op) {
            Some("HTTP") | Some("FILE") => {
                ext_io.insert(f.subject.as_str());
            }
            Some("UI_CONFIRM") | Some("UI_MESSAGE") | Some("UI_WINDOW_OPEN") => {
                ui_sink.insert(f.subject.as_str());
            }
            Some("DB_INSERT") | Some("DB_MODIFY") | Some("DB_DELETE") => {
                db_write.insert(f.subject.as_str());
            }
            _ => {}
        }
    }
    let roots: Vec<String> = all_roots
        .into_iter()
        .filter(|r| {
            let r = r.as_str();
            ext_io.contains(r) || (ui_sink.contains(r) && db_write.contains(r))
        })
        .collect();

    let summaries = crate::engine::return_summary::compute_return_summaries(
        &resolved.workspace.routines,
        Some(&resolved.workspace.objects),
    );
    let isolated = crate::engine::l3::event_graph::isolated_event_ids(&resolved.workspace.routines);
    let isolated_opt = if isolated.is_empty() {
        None
    } else {
        Some(&isolated)
    };
    digest_query(&snap, &roots, Some(&summaries), isolated_opt, true)
}

/// Compute the per-root digest effects for the CLI-B digest pipeline.
///
/// Mirrors TS `runDigestPipeline → digestQuery({order:false})` exactly: S4 ordering
/// is computed but `routineReturnSummaries` is NOT passed (the TS CLI path doesn't
/// pass them, so `errorEscapesChain` always returns false and `IO_BEFORE_ESCAPING_ERROR`
/// / any error-escape-based labels never fire from this path).
pub fn compute_digest_effects_cli(
    snap: &CapabilitySnapshot,
    resolved: &L3Resolved,
) -> Vec<DigestEntryResult> {
    let roots = reportable_roots(resolved);
    let isolated = crate::engine::l3::event_graph::isolated_event_ids(&resolved.workspace.routines);
    let isolated_opt = if isolated.is_empty() {
        None
    } else {
        Some(&isolated)
    };
    // No routineReturnSummaries — matches TS runDigestPipeline behavior.
    digest_query(snap, &roots, None, isolated_opt, false)
}

/// Project the R4-F digest-effects differential document, PRETTY-serialized with a
/// trailing newline (the exact on-disk golden form).
pub fn project_r4f_digest_effects(resolved: &L3Resolved, fixture_name: &str) -> String {
    let entries = compute_digest_effects(resolved);
    let doc = ProjectionSer {
        fixture_name,
        entries: &entries,
    };
    let mut s =
        serde_json::to_string_pretty(&doc).expect("serialize R4-F digest-effects projection");
    s.push('\n');
    s
}

// ===========================================================================
// R4-F STABLE PROJECTION — project_r4f_scoped_guarantees (Stage-4).
// Per-ScopedGuarantee key order (FIXED, the al-sem golden shape): label, scope,
// [writeOccurrenceId], [commitOccurrenceId], [ioOccurrenceId], [returnOccurrenceId],
// supportingEdgeIds, [commitEffectiveness], interveningBoundary, validForRefutation.
// Effects with no relevant scopedGuarantees are DROPPED; entries with no remaining
// effects are DROPPED (negatives → entryCount 0).
// ===========================================================================

fn is_relevant_label(label: &str) -> bool {
    matches!(
        label,
        "WRITE_PENDING_AT_EXTERNAL_IO"
            | "EXTERNAL_IO_BEFORE_COMMIT"
            | "WRITE_PENDING_AT_UI"
            | "IO_BEFORE_ESCAPING_ERROR"
            | "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN"
    )
}

struct ScopedGuaranteeSer<'a>(&'a crate::engine::l5::ordering_engine::ScopedGuarantee);
impl Serialize for ScopedGuaranteeSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let g = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("label", g.label)?;
        map.serialize_entry("scope", g.scope)?;
        if let Some(v) = &g.write_occurrence_id {
            map.serialize_entry("writeOccurrenceId", v)?;
        }
        if let Some(v) = &g.commit_occurrence_id {
            map.serialize_entry("commitOccurrenceId", v)?;
        }
        if let Some(v) = &g.io_occurrence_id {
            map.serialize_entry("ioOccurrenceId", v)?;
        }
        if let Some(v) = &g.return_occurrence_id {
            map.serialize_entry("returnOccurrenceId", v)?;
        }
        map.serialize_entry("supportingEdgeIds", &g.supporting_edge_ids)?;
        if let Some(v) = g.commit_effectiveness {
            map.serialize_entry("commitEffectiveness", v)?;
        }
        map.serialize_entry("interveningBoundary", g.intervening_boundary)?;
        map.serialize_entry("validForRefutation", &g.valid_for_refutation)?;
        map.end()
    }
}

struct ScopedEffectSer<'a> {
    fact_id: &'a str,
    effect_type: &'a str,
    guarantees: Vec<&'a crate::engine::l5::ordering_engine::ScopedGuarantee>,
}
impl Serialize for ScopedEffectSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("factId", self.fact_id)?;
        map.serialize_entry("type", self.effect_type)?;
        let sgs: Vec<ScopedGuaranteeSer> = self
            .guarantees
            .iter()
            .map(|g| ScopedGuaranteeSer(g))
            .collect();
        map.serialize_entry("scopedGuarantees", &sgs)?;
        map.end()
    }
}

struct ScopedEntrySer<'a> {
    routine_id: &'a str,
    effects: Vec<ScopedEffectSer<'a>>,
}
impl Serialize for ScopedEntrySer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("routineId", self.routine_id)?;
        map.serialize_entry("effects", &self.effects)?;
        map.end()
    }
}

struct ScopedProjectionSer<'a> {
    fixture_name: &'a str,
    entries: Vec<ScopedEntrySer<'a>>,
}
impl Serialize for ScopedProjectionSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("fixtureName", self.fixture_name)?;
        map.serialize_entry("entryCount", &self.entries.len())?;
        map.serialize_entry("entries", &self.entries)?;
        map.end()
    }
}

/// Project the R4-F scoped-guarantees differential document, PRETTY-serialized with
/// a trailing newline (the exact on-disk golden form).
pub fn project_r4f_scoped_guarantees(resolved: &L3Resolved, fixture_name: &str) -> String {
    let entries = compute_digest_effects_with_ordering(resolved);

    // Drop effects with no relevant scopedGuarantees; drop entries with no effects.
    let mut out_entries: Vec<ScopedEntrySer> = Vec::new();
    for entry in &entries {
        let mut out_effects: Vec<ScopedEffectSer> = Vec::new();
        for eff in &entry.effects {
            let relevant: Vec<&crate::engine::l5::ordering_engine::ScopedGuarantee> = eff
                .scoped_guarantees
                .iter()
                .filter(|g| is_relevant_label(g.label))
                .collect();
            if relevant.is_empty() {
                continue;
            }
            out_effects.push(ScopedEffectSer {
                fact_id: &eff.fact_id,
                effect_type: &eff.effect_type,
                guarantees: relevant,
            });
        }
        if out_effects.is_empty() {
            continue;
        }
        out_entries.push(ScopedEntrySer {
            routine_id: &entry.routine_id,
            effects: out_effects,
        });
    }

    let doc = ScopedProjectionSer {
        fixture_name,
        entries: out_entries,
    };
    let mut s =
        serde_json::to_string_pretty(&doc).expect("serialize R4-F scoped-guarantees projection");
    s.push('\n');
    s
}

// ===========================================================================
// cli-b/b3 PUBLIC FINGERPRINT-QUERY API
//
// Owned public versions of the internal borrowed structs, plus wrapper
// functions that the `fingerprint_query` module consumes.  Internal structs
// remain private so the digest-own tests don't change.
// ===========================================================================

/// Terminal hop info for human rendering — extracted from the internal
/// `WitnessHop::Terminal` that `project_path` drops from `query_hops`.
#[derive(Debug, Clone)]
pub struct TerminalHopInfo {
    /// "operation" | "callsite" | "synthetic"
    pub evidence_kind: String,
    pub display_text: String,
    pub source_file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Human-renderable hop — carries the EXACT fields `formatHop`
/// (format-fingerprint.ts:270) reads off the RAW `WitnessHop` (which the JSON
/// projection drops, e.g. `event-dispatch` → the SHORT `eventDisplay`, not the
/// full `event_id`; `implicit-trigger` → `triggerKind`). The human renderer
/// must consume THIS, not the projected `QueryWitnessHop`.
#[derive(Debug, Clone)]
pub enum HumanHop {
    Call {
        routine_display: String,
        callee_display: String,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    ObjectRun {
        routine_display: String,
        target_display: Option<String>,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    EventDispatch {
        /// SHORT name (`eventDisplayById.get(eid) ?? eid`), NOT the full event_id.
        event_display: String,
    },
    VariableTypedCall {
        routine_display: String,
        receiver_type: String,
        callee_display: Option<String>,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    InterfaceDispatch {
        routine_display: String,
        interface_name: String,
        candidate_count: usize,
        source_file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
}

/// Map a raw non-terminal `WitnessHop` → `HumanHop` (terminals → None; they are
/// rendered separately via `TerminalHopInfo`). Carries the SHORT `event_display`
/// for event-dispatch hops, which the JSON projection discards.
fn raw_hop_to_human(hop: &WitnessHop) -> Option<HumanHop> {
    match hop {
        WitnessHop::Call {
            routine_display,
            callee_display,
            source_file,
            line,
            column,
            ..
        } => Some(HumanHop::Call {
            routine_display: routine_display.clone(),
            callee_display: callee_display.clone(),
            source_file: source_file.clone(),
            line: *line,
            column: *column,
        }),
        WitnessHop::ObjectRun {
            routine_display,
            target_display,
            source_file,
            line,
            column,
            ..
        } => Some(HumanHop::ObjectRun {
            routine_display: routine_display.clone(),
            target_display: target_display.clone(),
            source_file: source_file.clone(),
            line: *line,
            column: *column,
        }),
        WitnessHop::EventDispatch { event_display, .. } => Some(HumanHop::EventDispatch {
            event_display: event_display.clone(),
        }),
        WitnessHop::VariableTypedCall {
            routine_display,
            receiver_type,
            callee_display,
            source_file,
            line,
            column,
            ..
        } => Some(HumanHop::VariableTypedCall {
            routine_display: routine_display.clone(),
            receiver_type: receiver_type.clone(),
            callee_display: callee_display.clone(),
            source_file: source_file.clone(),
            line: *line,
            column: *column,
        }),
        WitnessHop::InterfaceDispatch {
            routine_display,
            interface_name,
            candidate_count,
            source_file,
            line,
            column,
            ..
        } => Some(HumanHop::InterfaceDispatch {
            routine_display: routine_display.clone(),
            interface_name: interface_name.clone(),
            candidate_count: *candidate_count,
            source_file: source_file.clone(),
            line: *line,
            column: *column,
        }),
        WitnessHop::Terminal { .. } => None,
    }
}

/// Public projected path: a sequence of `QueryWitnessHop` values (terminal
/// hops dropped by `project_path`) PLUS the raw-derived human hops and the
/// optional terminal for human display.
#[derive(Debug, Clone)]
pub struct ProjectedPath {
    pub query_hops: Vec<QueryWitnessHop>,
    /// Human-renderable hops (raw-derived; NOT used in JSON). Parallel to the
    /// non-terminal raw hops.
    pub human_hops: Vec<HumanHop>,
    /// Terminal hop (if any) for human rendering only. NOT included in JSON output.
    pub terminal_hop: Option<TerminalHopInfo>,
}

/// Public per-witness diagnostic (mirrors al-sem `WitnessDiagnostic`).
#[derive(Debug, Clone)]
pub struct WitnessDiagnosticPub {
    pub kind: String,
    pub detail: Option<String>,
}

/// Public outcome of witness reconstruction (al-sem `reconstructWitnessPaths`
/// return type).
pub struct WitnessOutcomePub {
    /// Projected paths (terminal hops already stripped by `project_path`).
    pub paths: Vec<ProjectedPath>,
    pub truncated: bool,
    /// True when the BFS exceeded MAX_STATES (graph too large for full traversal).
    pub incomplete: bool,
    pub diagnostics: Vec<WitnessDiagnosticPub>,
}

/// Owned public version of the internal `FingerprintIndexes`.
/// Borrowed fields are cloned so the caller doesn't hold a snapshot reference.
pub struct FingerprintIndexesPub {
    pub stable_id_to_display: HashMap<String, String>,
    pub routine_display_by_id: HashMap<String, String>,
    pub outgoing_edges: HashMap<String, Vec<SnapshotGraphEdge>>,
    /// Per-subject capability facts (direct ∪ inherited), source order preserved.
    pub facts_by_routine: HashMap<String, Vec<crate::engine::l5::snapshot::SnapshotCapabilityFact>>,
    pub direct_facts_by_routine:
        HashMap<String, Vec<crate::engine::l5::snapshot::SnapshotCapabilityFact>>,
    pub coverage_by_routine: HashMap<String, crate::engine::l5::snapshot::SnapshotCoverageRecord>,
    pub callsite_by_id: HashMap<String, crate::engine::l5::snapshot::SnapshotCallsiteEvidence>,
    pub operation_by_id: HashMap<String, crate::engine::l5::snapshot::SnapshotOperationEvidence>,
    pub event_display_by_id: HashMap<String, String>,
}

/// Build the public fingerprint indexes from a `CapabilitySnapshot`.
/// Mirrors `buildFingerprintIndexes` (indexes.ts) but returns owned data.
pub fn build_fingerprint_indexes_pub(snap: &CapabilitySnapshot) -> FingerprintIndexesPub {
    let private = build_fingerprint_indexes(snap);
    FingerprintIndexesPub {
        stable_id_to_display: private.stable_id_to_display,
        routine_display_by_id: private.routine_display_by_id,
        outgoing_edges: private
            .outgoing_edges
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().cloned().collect()))
            .collect(),
        facts_by_routine: private
            .facts_by_routine
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().cloned().collect()))
            .collect(),
        direct_facts_by_routine: private
            .direct_facts_by_routine
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().cloned().collect()))
            .collect(),
        coverage_by_routine: private
            .coverage_by_routine
            .into_iter()
            .map(|(k, v)| (k, v.clone()))
            .collect(),
        callsite_by_id: private
            .callsite_by_id
            .into_iter()
            .map(|(k, v)| (k, v.clone()))
            .collect(),
        operation_by_id: private
            .operation_by_id
            .into_iter()
            .map(|(k, v)| (k, v.clone()))
            .collect(),
        event_display_by_id: private.event_display_by_id,
    }
}

/// Adapter: convert a `FingerprintIndexesPub` to a borrowed `FingerprintIndexes`
/// usable by the internal witness BFS helpers.
fn pub_idx_as_private(idx: &FingerprintIndexesPub) -> FingerprintIndexes<'_> {
    // Build incoming_edges (reverse graph) from outgoing_edges — mirrors what
    // `build_fingerprint_indexes` does from `typed_edges`.  `FingerprintIndexesPub`
    // stores owned `SnapshotGraphEdge` values so we derive `to` from each edge.
    let mut incoming_edges: HashMap<String, Vec<String>> = HashMap::new();
    for (from, edges) in &idx.outgoing_edges {
        for edge in edges {
            if let Some(to) = edge_to(edge) {
                incoming_edges
                    .entry(to.to_string())
                    .or_default()
                    .push(from.clone());
            }
        }
    }

    FingerprintIndexes {
        stable_id_to_display: idx.stable_id_to_display.clone(),
        routine_display_by_id: idx.routine_display_by_id.clone(),
        outgoing_edges: idx
            .outgoing_edges
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().collect()))
            .collect(),
        incoming_edges,
        facts_by_routine: idx
            .facts_by_routine
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().collect()))
            .collect(),
        direct_facts_by_routine: idx
            .direct_facts_by_routine
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().collect()))
            .collect(),
        direct_facts_by_op_kind: {
            let mut m: HashMap<(String, String), Vec<&Fact>> = HashMap::new();
            for v in idx.direct_facts_by_routine.values() {
                for f in v {
                    m.entry((f.op.clone(), f.resource_kind.clone()))
                        .or_default()
                        .push(f);
                }
            }
            m
        },
        coverage_by_routine: idx
            .coverage_by_routine
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect(),
        callsite_by_id: idx
            .callsite_by_id
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect(),
        operation_by_id: idx
            .operation_by_id
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect(),
        event_display_by_id: idx.event_display_by_id.clone(),
    }
}

/// `reconstructWitnessPaths` with a caller-supplied cap (mirrors the `witnessLimit`
/// parameter).  Returns projected paths — terminal hops are stripped by `project_path`.
/// Also tracks `incomplete` (MAX_STATES exceeded) and emits a `path-limit-reached`
/// diagnostic when `truncated`.
pub fn reconstruct_witness_paths_pub(
    root_id: &str,
    fact: &crate::engine::l5::snapshot::SnapshotCapabilityFact,
    idx: &FingerprintIndexesPub,
    cap: usize,
) -> WitnessOutcomePub {
    let private_idx = pub_idx_as_private(idx);
    let root_display = idx
        .routine_display_by_id
        .get(root_id)
        .cloned()
        .unwrap_or_else(|| root_id.to_string());

    // Run the single witness BFS with the requested cap.
    let outcome = reconstruct_witness_paths(root_id, fact, &private_idx, cap);

    // Project paths: strip terminal hops into query_hops; preserve terminal
    // separately for human rendering.
    let projected: Vec<ProjectedPath> = outcome
        .paths
        .iter()
        .map(|p| {
            let query_hops = project_path(p, root_id, &root_display, &private_idx);
            // Human hops: the RAW non-terminal hops, carrying short eventDisplay etc.
            let human_hops: Vec<HumanHop> = p.hops.iter().filter_map(raw_hop_to_human).collect();
            // Extract the terminal hop (always the last hop in a WitnessPath, if any).
            let terminal_hop = p.hops.last().and_then(|last| {
                if let WitnessHop::Terminal {
                    evidence_kind,
                    display_text,
                    source_file,
                    line,
                    column,
                    ..
                } = last
                {
                    Some(TerminalHopInfo {
                        evidence_kind: match evidence_kind {
                            TerminalKind::Operation => "operation".to_string(),
                            TerminalKind::Callsite => "callsite".to_string(),
                            TerminalKind::Synthetic => "synthetic".to_string(),
                        },
                        display_text: display_text.clone(),
                        source_file: source_file.clone(),
                        line: *line,
                        column: *column,
                    })
                } else {
                    None
                }
            });
            ProjectedPath {
                query_hops,
                human_hops,
                terminal_hop,
            }
        })
        .collect();

    // Forward all diagnostics from the internal outcome (missing-witness-anchor,
    // path-limit-reached, state-limit-exceeded, etc. — already built by reconstruct_witness_paths).
    let diagnostics: Vec<WitnessDiagnosticPub> = outcome
        .diagnostics
        .into_iter()
        .map(|(kind, detail)| WitnessDiagnosticPub { kind, detail })
        .collect();

    WitnessOutcomePub {
        paths: projected,
        truncated: outcome.truncated,
        incomplete: outcome.incomplete,
        diagnostics,
    }
}

// ===========================================================================
// Native unit test — occurrence_id round-trips a hand-built canonical key.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::snapshot::{SnapshotIdentityTable, SnapshotRange, SnapshotSourceAnchor};

    #[test]
    fn occurrence_id_round_trips_hand_built_key() {
        // Hand-built canonical key (direct-fact form: empty linkSignature).
        let routine_id = "g:Codeunit:50000#abc";
        let (key, link) =
            build_canonical_key(routine_id, &[], "operation", "r0/h/op1", "DB_MODIFY");
        assert_eq!(link, "");
        assert_eq!(key, "g:Codeunit:50000#abc||operation|r0/h/op1|DB_MODIFY");
        let occ = occurrence_id_from_key(&key, 0);
        // Round-trip: occ == sha256Hex(key)[..16].
        assert_eq!(occ, sha256_hex(&key)[..16].to_string());
        assert_eq!(occ.len(), 16);
        assert!(occ.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn link_signature_event_dispatch_hop_has_no_callsite() {
        // An event-dispatch hop with no callsiteId → segment `@/event-dispatch//`.
        let hop = QueryWitnessHop {
            kind: "event-dispatch",
            from_routine_id: "A".to_string(),
            from_display: "a".to_string(),
            to_routine_id: Some("B".to_string()),
            to_display: Some("b".to_string()),
            callee_display: None,
            callsite_id: None,
            event_id: Some("evt".to_string()),
            target_app_guid: None,
            edge_kind: Some("event-dispatch".to_string()),
            anchor: None,
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
        };
        let (_key, link) = build_canonical_key("R", &[vec![hop]], "callsite", "cs", "HTTP");
        assert_eq!(link, "A>B@/event-dispatch//");
    }

    // -----------------------------------------------------------------------
    // Helper: build a minimal Fact (SnapshotCapabilityFact) for unit tests.
    // -----------------------------------------------------------------------
    fn make_fact(
        op: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        provenance: &str,
        witness_operation_id: Option<&str>,
        witness_callsite_id: Option<&str>,
    ) -> Fact {
        Fact {
            subject: "root#sig".to_string(),
            op: op.to_string(),
            resource_kind: resource_kind.to_string(),
            resource_id: resource_id.map(|s| s.to_string()),
            resource_arg_source: None,
            confidence: "high".to_string(),
            provenance: provenance.to_string(),
            via: "direct".to_string(),
            witness_operation_id: witness_operation_id.map(|s| s.to_string()),
            witness_callsite_id: witness_callsite_id.map(|s| s.to_string()),
            extra: None,
        }
    }

    // -----------------------------------------------------------------------
    // Helper: build an empty CapabilitySnapshot (all vecs empty, no frames).
    // -----------------------------------------------------------------------
    fn empty_snapshot() -> CapabilitySnapshot {
        CapabilitySnapshot {
            identities: SnapshotIdentityTable {
                stable_ids: vec![],
                display_names: vec![],
            },
            capability_facts: vec![],
            typed_edges: vec![],
            operation_index: vec![],
            callsite_index: vec![],
            callsite_resolutions: vec![],
            analysis_gaps: vec![],
            coverage: vec![],
            event_declarations: vec![],
            root_classifications: vec![],
            routine_order_frames: None,
        }
    }

    // -----------------------------------------------------------------------
    // Oracle 1: multi-path TIE — two equal-length witness paths sort by
    // lexicographically-smaller `witness_hops_json` first (witness.ts:352-355).
    //
    // Level: serializer + sort-comparator (no BFS driver needed).
    // -----------------------------------------------------------------------
    #[test]
    fn multi_path_tie_sort_by_lex_smaller_json_first() {
        // Build two WitnessHop::Call paths with the SAME hop count (1 + 1 terminal = 2
        // hops each) but different routineIds so their JSON differs.
        // Path A: call to "routineA" + terminal op.
        let path_a = WitnessPath {
            hops: vec![
                WitnessHop::Call {
                    routine_id: "g:Codeunit:1#aaa".to_string(),
                    routine_display: "RouterA".to_string(),
                    callee_display: "DoA".to_string(),
                    callsite_id: "cs1".to_string(),
                    source_file: None,
                    line: None,
                    column: None,
                },
                WitnessHop::Terminal {
                    evidence_kind: TerminalKind::Synthetic,
                    operation_id: None,
                    callsite_id: None,
                    display_text: "insert table".to_string(),
                    source_file: None,
                    line: None,
                    column: None,
                },
            ],
        };
        // Path B: call to "routineZ" + same terminal. "g:Codeunit:1#zzz" > "g:Codeunit:1#aaa"
        // so path_a JSON < path_b JSON.
        let path_b = WitnessPath {
            hops: vec![
                WitnessHop::Call {
                    routine_id: "g:Codeunit:1#zzz".to_string(),
                    routine_display: "RouterZ".to_string(),
                    callee_display: "DoZ".to_string(),
                    callsite_id: "cs1".to_string(),
                    source_file: None,
                    line: None,
                    column: None,
                },
                WitnessHop::Terminal {
                    evidence_kind: TerminalKind::Synthetic,
                    operation_id: None,
                    callsite_id: None,
                    display_text: "insert table".to_string(),
                    source_file: None,
                    line: None,
                    column: None,
                },
            ],
        };

        // Confirm JSON serialization order matches expectation.
        let json_a = witness_hops_json(&path_a.hops);
        let json_b = witness_hops_json(&path_b.hops);
        assert!(
            json_a < json_b,
            "path_a JSON should be lex-smaller than path_b JSON: a={json_a:?} b={json_b:?}"
        );

        // Simulate the final sort (same as witness.ts:352-355 and the Rust BFS exit).
        // Intentionally reversed to verify the sort corrects the order.
        let mut paths: Vec<WitnessPath> = [path_b, path_a].into();
        paths.sort_by(|a, b| {
            if a.hops.len() != b.hops.len() {
                return a.hops.len().cmp(&b.hops.len());
            }
            witness_hops_json(&a.hops).cmp(&witness_hops_json(&b.hops))
        });

        // The lex-smaller path (path_a, routineA) must be first.
        assert!(
            matches!(&paths[0].hops[0], WitnessHop::Call { routine_id, .. } if routine_id == "g:Codeunit:1#aaa"),
            "expected path_a (routineA) to sort first; got {:?}",
            paths[0].hops[0]
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 2: factEquivalent None-guard (witness.ts:380, asymmetric).
    //
    // Level: unit — fact_equivalent directly.
    // -----------------------------------------------------------------------
    #[test]
    fn fact_equivalent_none_guard_asymmetric() {
        // a.resource_id = Some("x"), b.resource_id = None → treated as equivalent
        // (the "either undefined → match" leniency).
        let a = make_fact(
            "insert",
            "table",
            Some("g/table/50000"),
            "direct",
            None,
            None,
        );
        let b = make_fact("insert", "table", None, "direct", None, None);
        assert!(
            fact_equivalent(&a, &b),
            "one-sided None resourceId should be treated as equivalent"
        );
        assert!(
            fact_equivalent(&b, &a),
            "symmetry: None ↔ Some should also be equivalent"
        );

        // Both Some and DIFFER → false.
        let c = make_fact(
            "insert",
            "table",
            Some("g/table/99999"),
            "direct",
            None,
            None,
        );
        assert!(
            !fact_equivalent(&a, &c),
            "both Some with different resourceIds should NOT be equivalent"
        );

        // Both Some and SAME → true.
        let d = make_fact(
            "insert",
            "table",
            Some("g/table/50000"),
            "direct",
            None,
            None,
        );
        assert!(
            fact_equivalent(&a, &d),
            "both Some with equal resourceIds should be equivalent"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 3: object-run-unresolved edge → edge_to_hop returns None
    // (witness.ts:461-464 "BFS cannot walk through").
    //
    // Level: unit — edge_to_hop directly.
    // -----------------------------------------------------------------------
    #[test]
    fn object_run_unresolved_edge_to_hop_is_none() {
        let edge = SnapshotGraphEdge::ObjectRunUnresolved {
            kind: "object-run-unresolved",
            callsite_id: "cs_unresolved".to_string(),
            from: "root#sig".to_string(),
            target_object: None,
            target_id_source: SnapValueSource::Unknown,
            object_type: "Codeunit".to_string(),
            source_anchor: SnapshotSourceAnchor {
                source_unit_id: "su1".to_string(),
                range: SnapshotRange {
                    start_line: 1,
                    start_column: 0,
                    end_line: 1,
                    end_column: 10,
                },
                enclosing_routine_id: "root#sig".to_string(),
                syntax_kind: "method_call".to_string(),
            },
            edge_id: "eid-unresolved".to_string(),
        };
        let snap = empty_snapshot();
        let idx = build_fingerprint_indexes(&snap);
        assert!(
            edge_to_hop(&edge, &idx).is_none(),
            "object-run-unresolved should produce None from edge_to_hop"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 4: occurrence dedup — two effects with the SAME canonicalKey get the
    // SAME factId (ordering-engine.ts seenCanonicalKeys); two with different keys
    // get different factIds.
    //
    // Level: unit — build_canonical_key + occurrence_id_from_key.
    // -----------------------------------------------------------------------
    #[test]
    fn occurrence_dedup_same_key_same_fact_id() {
        let routine = "g:Codeunit:50000#abc";

        // Two effects that produce the same canonical key (same routine, no via-paths,
        // same evidence kind/id, same effect type).
        let (key1, _) = build_canonical_key(routine, &[], "operation", "r0/op1", "DB_INSERT");
        let (key2, _) = build_canonical_key(routine, &[], "operation", "r0/op1", "DB_INSERT");
        assert_eq!(key1, key2);
        let id1 = occurrence_id_from_key(&key1, 0);
        let id2 = occurrence_id_from_key(&key2, 0);
        assert_eq!(id1, id2, "same canonical key must produce same factId");

        // Two effects with different effect types → different canonical keys → different factIds.
        let (key3, _) = build_canonical_key(routine, &[], "operation", "r0/op1", "DB_MODIFY");
        assert_ne!(key1, key3);
        let id3 = occurrence_id_from_key(&key3, 0);
        assert_ne!(
            id1, id3,
            "different canonical keys must produce different factIds"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 5: seed-tie order-preservation — stable sort_by(.cmp) is a no-op on
    // equal keys, meaning insertion order is preserved for equal-routine seeds.
    // This grounds finding B (witness.ts:276 V8-stable-preserves-equal-key-order).
    //
    // Level: unit — sort_by on a Vec of equal-key items.
    // -----------------------------------------------------------------------
    #[test]
    fn seed_sort_equal_routine_preserves_insertion_order() {
        // Simulate the seed-sort scenario: multiple "State"-like items with the SAME
        // `routine` value. Stable sort must leave them in their original order.
        #[derive(Debug, PartialEq, Eq)]
        struct SeedItem {
            routine: String,
            insertion_index: usize,
        }

        let items: Vec<SeedItem> = vec![
            SeedItem {
                routine: "same#abc".to_string(),
                insertion_index: 0,
            },
            SeedItem {
                routine: "same#abc".to_string(),
                insertion_index: 1,
            },
            SeedItem {
                routine: "same#abc".to_string(),
                insertion_index: 2,
            },
        ];

        let mut sorted = items;
        sorted.sort_by(|a, b| a.routine.cmp(&b.routine));

        // All routines are equal → sort must not reorder (stable sort invariant).
        assert_eq!(sorted[0].insertion_index, 0);
        assert_eq!(sorted[1].insertion_index, 1);
        assert_eq!(sorted[2].insertion_index, 2);
    }

    // -----------------------------------------------------------------------
    // Oracle 6: synthetic-terminal direct fact (no witnessOperationId, no
    // witnessCallsiteId) → terminal_hop_from_fact emits Synthetic terminal
    // with display_text = "{op} {resourceKind}" and no IDs. The dedupe-key for
    // this terminal uses the "synthetic:op:resourceKind:resourceId" branch.
    //
    // Level: unit — terminal_hop_from_fact + dedupe_key (synthetic branch).
    // -----------------------------------------------------------------------
    #[test]
    fn synthetic_terminal_direct_fact_no_witness_anchor() {
        let fact = make_fact(
            "commit",
            "table",
            Some("g/table/50000"),
            "direct",
            None,
            None,
        );

        let snap = empty_snapshot();
        let idx = build_fingerprint_indexes(&snap);

        let hop = terminal_hop_from_fact(&fact, &idx);
        match &hop {
            WitnessHop::Terminal {
                evidence_kind,
                operation_id,
                callsite_id,
                display_text,
                ..
            } => {
                assert_eq!(*evidence_kind, TerminalKind::Synthetic);
                assert!(
                    operation_id.is_none(),
                    "synthetic terminal must have no operationId"
                );
                assert!(
                    callsite_id.is_none(),
                    "synthetic terminal must have no callsiteId"
                );
                assert_eq!(
                    display_text, "commit table",
                    "display_text must be '{{op}} {{resourceKind}}'"
                );
            }
            other => panic!("expected Terminal hop, got {other:?}"),
        }

        // The dedupe_key for a synthetic terminal must use the
        // "synthetic:op:resourceKind:resourceId" branch (no op/cs anchor).
        let detail: Vec<(String, String)> = vec![];
        let key = dedupe_key("COMMIT", Some(&hop), &fact, &detail);
        assert!(
            key.starts_with("COMMIT|synthetic:commit:table:g/table/50000|"),
            "dedupe_key synthetic branch must embed op/resourceKind/resourceId: {key:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 7 (#8): PER-PATH terminal conditionality. An effect reached via TWO
    // paths terminating at DIFFERENT controlContexts must compute each path's
    // conditionality from its OWN terminal hop — NOT from a single effect-level
    // (shortest-path) terminal applied to all paths. We build two raw WitnessPaths:
    //   - Path A: a top-level callsite hop, terminating at a top-level operation.
    //   - Path B: a conditional callsite hop, terminating at a loop-body operation.
    // Per-path: A → unconditional-on-success, B → loop-body (most-restrictive).
    // Effect fold (least-restrictive across paths) → unconditional-on-success.
    // If the engine used ONE terminal for both paths, B would not be loop-body.
    // -----------------------------------------------------------------------
    #[test]
    fn per_path_terminal_conditionality_differs_per_path() {
        use crate::engine::l5::conditionality::{CONDITIONAL, LOOP_BODY, UNCONDITIONAL};

        // Terminal-context maps: op "opTop" is top-level; op "opLoop" is loop-body.
        let mut op_ctx: HashMap<&str, Option<&str>> = HashMap::new();
        op_ctx.insert("opTop", Some("top-level"));
        op_ctx.insert("opLoop", Some("loop-body"));
        // Callsite ctx: cs "csTop" top-level, cs "csCond" conditional.
        let mut cs_ctx: HashMap<&str, Option<&str>> = HashMap::new();
        cs_ctx.insert("csTop", Some("top-level"));
        cs_ctx.insert("csCond", Some("conditional"));

        let path_a = WitnessPath {
            hops: vec![
                WitnessHop::Call {
                    routine_id: "r#callee".into(),
                    routine_display: "Callee".into(),
                    callee_display: "Do".into(),
                    callsite_id: "csTop".into(),
                    source_file: None,
                    line: None,
                    column: None,
                },
                WitnessHop::Terminal {
                    evidence_kind: TerminalKind::Operation,
                    operation_id: Some("opTop".into()),
                    callsite_id: None,
                    display_text: String::new(),
                    source_file: None,
                    line: None,
                    column: None,
                },
            ],
        };
        let path_b = WitnessPath {
            hops: vec![
                WitnessHop::Call {
                    routine_id: "r#callee".into(),
                    routine_display: "Callee".into(),
                    callee_display: "Do".into(),
                    callsite_id: "csCond".into(),
                    source_file: None,
                    line: None,
                    column: None,
                },
                WitnessHop::Terminal {
                    evidence_kind: TerminalKind::Operation,
                    operation_id: Some("opLoop".into()),
                    callsite_id: None,
                    display_text: String::new(),
                    source_file: None,
                    line: None,
                    column: None,
                },
            ],
        };

        let ca = compute_path_conditionality(&path_a, &cs_ctx, &op_ctx);
        let cb = compute_path_conditionality(&path_b, &cs_ctx, &op_ctx);
        // Path A: hops [top-level] + terminal top-level → most-restrictive = unconditional.
        assert_eq!(ca, UNCONDITIONAL);
        // Path B: hops [conditional] + terminal loop-body → most-restrictive = loop-body.
        assert_eq!(cb, LOOP_BODY);
        assert_ne!(
            ca, cb,
            "the two paths MUST yield different conditionalities (#8 per-path terminal)"
        );

        // Effect fold: least-restrictive across the two paths → unconditional.
        let folded = crate::engine::l5::conditionality::effect_conditionality(&[ca, cb], false);
        assert_eq!(folded, UNCONDITIONAL);

        // Control: if (buggily) BOTH paths used path B's loop-body terminal, path A would
        // become loop-body and the fold would be loop-body — NOT unconditional. Assert the
        // buggy outcome differs, proving the per-path threading matters.
        let buggy_a =
            crate::engine::l5::conditionality::path_conditionality(&[CONDITIONAL], LOOP_BODY); // A's hop + B's terminal
        assert_eq!(buggy_a, LOOP_BODY);
        let buggy_fold =
            crate::engine::l5::conditionality::effect_conditionality(&[buggy_a, cb], false);
        assert_ne!(
            buggy_fold, folded,
            "shared-terminal (buggy) fold must differ from per-path fold"
        );
    }

    // -----------------------------------------------------------------------
    // Review-finding regression: an identity-duplicate's FIRST occurrence must
    // merge-normalize the accumulator (via `merge_normalize_via_paths`) instead of
    // leaving a fresh-insert entry's RAW BFS order un-normalized.
    //
    // `merge_normalize_via_paths` is the pure, extracted helper both the live MERGE
    // branch and the identity-duplicate path in `digest_one_root` call — testing it
    // directly exercises the EXACT code both paths run (a full `digest_one_root`
    // fixture would need a whole `CapabilitySnapshot`/`FingerprintIndexes` graph to
    // reach this code and would only re-verify plumbing already covered by the CDO
    // byte-compare; the divergence itself lives entirely in this helper).
    // -----------------------------------------------------------------------

    /// Minimal QueryWitnessHop for path-length/sort tests — content is irrelevant to
    /// `merge_normalize_via_paths`'s logic (only `path.len()` and the caller-supplied
    /// json string participate in its sort/dedupe), so every field is a placeholder.
    fn mk_hop(tag: &str) -> QueryWitnessHop {
        QueryWitnessHop {
            kind: "call",
            from_routine_id: format!("R{tag}"),
            from_display: tag.to_string(),
            to_routine_id: None,
            to_display: None,
            callee_display: None,
            callsite_id: None,
            event_id: None,
            target_app_guid: None,
            edge_kind: None,
            anchor: None,
            receiver_type: None,
            interface_name: None,
            candidate_count: None,
        }
    }

    #[test]
    fn first_duplicate_normalization_matches_reference_two_full_merges() {
        // Reproduce the review finding's divergence scheme: a fresh-insert entry
        // stores paths in RAW BFS final order — NOT sorted by (projected_len, json).
        // Here: a raw-len-2 ("terminal") path stored BEFORE a raw-len-1 ("boundary")
        // path, i.e. the OPPOSITE of merge-normalized order (which sorts shortest
        // first, then lexicographically by json).
        let terminal_path = vec![mk_hop("t1"), mk_hop("t2")]; // len 2
        let boundary_path = vec![mk_hop("b1")]; // len 1
        let raw_paths = vec![terminal_path.clone(), boundary_path.clone()];
        let raw_conds: Vec<crate::engine::l5::conditionality::EffectConditionality> =
            vec![crate::engine::l5::conditionality::UNCONDITIONAL; 2];
        let raw_jsons = vec!["Z_terminal_json".to_string(), "A_boundary_json".to_string()];

        // This IS the fresh-insert branch's stored state (`all_paths`/`had_truncation`
        // set directly from `outcome`/`projected_paths` — never sorted). Simulate it
        // directly rather than via a helper, since the fresh-insert branch never
        // normalizes.
        let existing_had_truncation = false;

        // --- Our fix: FIRST duplicate normalizes with an EMPTY new contribution. ---
        let (via_dup, trunc_dup, all_paths_dup, conds_dup, jsons_dup) = merge_normalize_via_paths(
            &raw_paths,
            &raw_conds,
            &raw_jsons,
            existing_had_truncation,
            &[],
            &[],
            &[],
            false,
            3,
        );

        // --- Reference: OLD code's real MERGE branch, fed the duplicate's OWN
        // contribution as a genuine "new" fact (byte-identical to the existing
        // paths, since identical BFS inputs ⇒ identical outcome — see
        // `effect_fact_loop_identity`'s doc). This is what actually running BFS a
        // second time for the duplicate and merging it for real would have produced.
        let (via_ref, trunc_ref, all_paths_ref, conds_ref, jsons_ref) = merge_normalize_via_paths(
            &raw_paths,
            &raw_conds,
            &raw_jsons,
            existing_had_truncation,
            &raw_paths,
            &raw_conds,
            &raw_jsons,
            false,
            3,
        );

        assert_eq!(
            jsons_dup, jsons_ref,
            "first-duplicate normalization (no new paths) must match a real second full merge"
        );
        assert_eq!(conds_dup, conds_ref);
        assert_eq!(trunc_dup, trunc_ref);
        assert_eq!(via_dup.len(), via_ref.len());
        for (a, b) in via_dup.iter().zip(via_ref.iter()) {
            assert_eq!(a.len(), b.len());
        }
        assert_eq!(all_paths_dup.len(), all_paths_ref.len());

        // And prove the fix actually CHANGES the un-normalized fresh-insert state:
        // sorted-first-by-len puts the boundary (len 1) path ahead of the terminal
        // (len 2) path — the reverse of the raw insertion order above.
        assert_eq!(
            jsons_dup,
            vec!["A_boundary_json".to_string(), "Z_terminal_json".to_string()],
            "normalization must re-sort the raw fresh-insert order by (len, json)"
        );
        assert_ne!(
            jsons_dup, raw_jsons,
            "the raw fresh-insert order must actually be non-normalized for this test \
             to be meaningful"
        );
    }

    #[test]
    fn second_and_later_duplicates_are_a_no_op_fixed_point() {
        // An already merge-normalized (sorted + deduped) list must be unchanged by
        // a further identity-duplicate normalization pass (SECOND+ duplicates are
        // skipped outright in `digest_one_root`, but this proves WHY that is sound:
        // normalizing an already-normalized list with no new paths is a fixed point).
        let sorted_paths = vec![vec![mk_hop("b1")], vec![mk_hop("t1"), mk_hop("t2")]];
        let sorted_conds: Vec<crate::engine::l5::conditionality::EffectConditionality> =
            vec![crate::engine::l5::conditionality::UNCONDITIONAL; 2];
        let sorted_jsons = vec!["A_boundary_json".to_string(), "Z_terminal_json".to_string()];

        let (via, had_truncation, all_paths, conds, jsons) = merge_normalize_via_paths(
            &sorted_paths,
            &sorted_conds,
            &sorted_jsons,
            false,
            &[],
            &[],
            &[],
            false,
            3,
        );

        assert_eq!(
            jsons, sorted_jsons,
            "already-normalized order must be unchanged"
        );
        assert_eq!(conds, sorted_conds);
        assert!(!had_truncation);
        assert_eq!(all_paths.len(), 2);
        assert_eq!(via.len(), 2);
    }

    /// `SnapTempState` has no `PartialEq` (production code has no need for it) — a
    /// small structural comparator local to this test avoids adding a derive to
    /// `snapshot.rs` for test-only purposes.
    fn temp_state_eq(a: &Option<SnapTempState>, b: &Option<SnapTempState>) -> bool {
        match (a, b) {
            (None, None) => true,
            (
                Some(SnapTempState::Known { value: v1 }),
                Some(SnapTempState::Known { value: v2 }),
            ) => v1 == v2,
            (
                Some(SnapTempState::ParameterDependent {
                    parameter_index: p1,
                }),
                Some(SnapTempState::ParameterDependent {
                    parameter_index: p2,
                }),
            ) => p1 == p2,
            (Some(SnapTempState::Unknown), Some(SnapTempState::Unknown)) => true,
            _ => false,
        }
    }

    #[test]
    fn merge_temp_state_recomputes_for_real_not_assumed_idempotent() {
        // existing known-temp=true, new NOT known-temp → downgrades to `new`
        // (mirrors the live MERGE branch's formula exactly).
        let known_true = Some(SnapTempState::Known { value: true });
        let known_false = Some(SnapTempState::Known { value: false });

        let downgraded = merge_temp_state(&known_true, &known_false);
        assert!(
            temp_state_eq(&downgraded, &known_false),
            "existing known-temp merged with a non-known-temp new value must downgrade"
        );

        // Re-applying the SAME `new` value again must be a no-op (idempotent for a
        // repeated identical duplicate) — this is the case
        // `digest_one_root`'s SECOND+ duplicate skip relies on.
        let reapplied = merge_temp_state(&downgraded, &known_false);
        assert!(
            temp_state_eq(&reapplied, &downgraded),
            "re-applying the identical new value must not change the result further"
        );

        // Both known-temp=true → stays existing (unchanged, no downgrade).
        let both_true = merge_temp_state(&known_true, &known_true);
        assert!(temp_state_eq(&both_true, &known_true));
    }
}
