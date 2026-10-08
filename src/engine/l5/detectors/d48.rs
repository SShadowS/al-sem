//! D48 — external IO (HTTP or FILE) executed inside a loop, directly or
//! transitively through in-loop call chains. Port of al-sem
//! `src/detectors/d48-io-in-loop.ts`.
//!
//! Severity: HTTP → `high`, FILE → `medium`.
//!
//! Pattern mirrors d1: the walk starts from in-loop callsites; `terminals_at`
//! returns only LOCAL/DIRECT IO effects (from `capability_facts_direct`,
//! `provenance == "direct"`), NOT inherited facts — the walker provides
//! transitivity and loop-depth propagation. Emit only `Complete` paths with
//! `effective_loop_depth > 0`.
//!
//! ## Reading the http/file facts
//! IO terminals come from each routine's `FullRoutineSummary.capability_facts_direct`
//! filtered to `provenance == "direct"` AND `resource_kind ∈ {http, file}` AND a
//! present `witness_callsite_id`. The `witness_callsite_id`'s `loop_stack.len()` is
//! the LOCAL loop depth. The display method (HTTP "Send"/"Post"/…) comes from
//! `CapabilityExtra::Http { method }`; file facts carry no extra (→ no method).
//!
//! The pruning predicate `routine_touches_external_io` checks the REACHABLE
//! (direct ∪ inherited) facts for any http/file resource kind.
//!
//! ## Source-only role path
//! `roleOf(routine) === "primary"` holds for every routine source-only, so the role
//! gate is a no-op pass.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::engine::l4::capability_cone::CapabilityExtra;
use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l4::cone_derived::{ConeDerivedStore, io_kind_bit};
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::path_walker::{
    PathCtx, Terminal, WalkBounds, WalkOpts, WalkPolicy, WalkStop, walk_evidence,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d48-io-in-loop";

const BOUNDS: WalkBounds = WalkBounds {
    max_depth: 20,
    max_nodes: 500,
};

/// IO resource kinds D48 targets. ⟨C1 Task 2 fix I1⟩ Routed through
/// `cone_derived::io_kind_bit` — the ONE definition of "is this IO" — so this
/// (the direct-terminal producer) and `routine_touches_external_io` (the
/// walk-pruning gate, below) cannot desync on a future third IO kind.
fn is_io_resource_kind(kind: &str) -> bool {
    io_kind_bit(kind).is_some()
}

fn severity_for_io_kind(kind: &str) -> &'static str {
    if kind == "http" { "high" } else { "medium" }
}

/// One LOCAL/DIRECT IO terminal — the Rust analogue of al-sem's
/// `IoTerminal extends Terminal`. The PW-0 `Terminal` carries `op_id`; D48 instead
/// keys the witness by callsite, so the witness callsite id is stashed in `op_id`
/// (it is what `build_terminal_step` and the call-chain recovery read back). The
/// rich fields (ioKind / methodName / anchor) live in this side table, looked up by
/// `(routine_id, witness_callsite_id)`.
#[derive(Debug, Clone)]
struct IoTerminal {
    witness_callsite_id: String,
    source_anchor: SourceAnchor,
    io_kind: String,
    method_name: Option<String>,
    /// al-sem `IoTerminal.localLoopDepth` = the witness callsite's `loopStack.length`.
    /// The walker computes `effectiveLoopDepth = inheritedLoopDepth + localLoopDepth`.
    local_loop_depth: i64,
}

/// `directIoTerminalsFor` — LOCAL/DIRECT IO effects of a routine. Uses
/// `capability_facts_direct` only (`provenance == "direct"`), filtered to http/file
/// with a witness callsite. Anchored via the witness callsite (loop_stack →
/// localLoopDepth).
fn direct_io_terminals_for(
    routine: &ModelRoutine,
    summary: &FullRoutineSummary,
) -> Vec<IoTerminal> {
    if summary.capability_facts_direct.is_empty() {
        return Vec::new();
    }
    let cs_by_id: HashMap<&str, &crate::program::body::features::PCallSite> = routine
        .call_sites
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();

    let mut out: Vec<IoTerminal> = Vec::new();
    for fact in &summary.capability_facts_direct {
        if fact.provenance != "direct" {
            continue;
        }
        if !is_io_resource_kind(fact.resource_kind) {
            continue;
        }
        let Some(witness) = &fact.witness_callsite_id else {
            continue;
        };
        let cs = cs_by_id.get(witness.as_str()).copied();
        let source_anchor = match cs {
            Some(cs) => anchor_of(&cs.source_anchor, routine),
            None => anchor_of(&routine.source_anchor, routine),
        };
        let method_name = match &fact.extra {
            Some(CapabilityExtra::Http { method, .. }) => Some(method.clone()),
            _ => None,
        };
        out.push(IoTerminal {
            witness_callsite_id: witness.clone(),
            source_anchor,
            io_kind: fact.resource_kind.to_string(),
            method_name,
            // al-sem witnessCallsite.loopStack.length (0 when the witness callsite isn't
            // found — defensive; the witness is the fact's own callsite, normally present).
            local_loop_depth: cs.map(|c| c.loop_stack.len() as i64).unwrap_or(0),
        });
    }
    out
}

/// `routineTouchesExternalIo` — REACHABLE (direct ∪ inherited) facts contain any
/// http/file kind. Prunes the walk.
///
/// ⟨C1 Task 2⟩ Reads the folded cone presence flag instead of scanning the raw
/// `direct ∪ inherited` chain. [`ConeDerivedStore::touches_io`] is `{http, file}`
/// — the COMPLETE IO vocabulary [`is_io_resource_kind`] tests — and an unfolded
/// routine reads as the empty row, reproducing the old `None ⇒ false` arm for a
/// callee with no summary at all.
fn routine_touches_external_io(store: &ConeDerivedStore, routine_id: &str) -> bool {
    store.touches_io(routine_id)
}

/// The terminal-step note: `"<KIND> <method>"` or just `"<KIND>"`. al-sem
/// uppercases the io kind.
fn io_method_note(io_kind: &str, method_name: Option<&str>) -> String {
    let up = io_kind.to_uppercase();
    match method_name {
        Some(m) => format!("{up} {m}"),
        None => up,
    }
}

/// The D48 WalkPolicy. `io_terminals_by_routine` is the LOCAL/DIRECT terminal table
/// (NOT inherited). `terminals_at` returns one PW-0 `Terminal` per IO fact, with the
/// witness callsite id stashed in `op_id`.
struct D48Policy<'a> {
    routine_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
    /// ⟨C1 Task 2⟩ The derived cone substrate `expand`'s IO pruning gate reads.
    cone_derived: &'a ConeDerivedStore,
    edges_by_from: &'a HashMap<String, Vec<CombinedEdge>>,
    io_terminals_by_routine: &'a HashMap<String, Vec<IoTerminal>>,
    call_site_by_id: &'a HashMap<&'a str, &'a crate::program::body::features::PCallSite>,
}

impl<'a> WalkPolicy for D48Policy<'a> {
    fn terminals_at(&self, node: &str, _ctx: &PathCtx) -> Vec<Terminal> {
        let Some(terms) = self.io_terminals_by_routine.get(node) else {
            return Vec::new();
        };
        terms
            .iter()
            .map(|t| Terminal {
                routine_id: node.to_string(),
                local_loop_depth: t.local_loop_depth,
                op_id: Some(t.witness_callsite_id.clone()),
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
                routine_touches_external_io(self.cone_derived, &e.to)
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
            note: format!("calls {to_name}"),
        }
    }

    fn build_terminal_step(&self, t: &Terminal, _ctx: &PathCtx) -> EvidenceStep {
        let witness = t.op_id.clone().unwrap_or_default();
        let term = self
            .io_terminals_by_routine
            .get(&t.routine_id)
            .and_then(|terms| terms.iter().find(|x| x.witness_callsite_id == witness));
        let (anchor, note) = match term {
            Some(term) => (
                term.source_anchor.clone(),
                io_method_note(&term.io_kind, term.method_name.as_deref()),
            ),
            None => (
                SourceAnchor {
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
                String::new(),
            ),
        };
        EvidenceStep {
            routine_id: t.routine_id.clone(),
            operation_id: None,
            callsite_id: Some(witness),
            loop_id: None,
            source_anchor: anchor,
            note,
        }
    }

    fn loop_depth_of_edge(&self, edge: &CombinedEdge) -> i64 {
        // al-sem D48 supplies no custom loopDepthOfEdge, so walkEvidence falls back to
        // the SHARED default: ctx.callSiteById.get(edge.callsiteId).loopStack.length.
        edge.callsite_id
            .as_ref()
            .and_then(|cid| self.call_site_by_id.get(cid.as_str()))
            .map(|cs| cs.loop_stack.len() as i64)
            .unwrap_or(0)
    }
}

pub fn detect_d48(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    // Pre-build the LOCAL/DIRECT IoTerminal table (NOT inherited).
    let mut io_terminals_by_routine: HashMap<String, Vec<IoTerminal>> = HashMap::new();
    for r in &ws.routines {
        if let Some(summary) = ctx.summaries.get(&r.id) {
            let terms = direct_io_terminals_for(r, summary);
            if !terms.is_empty() {
                io_terminals_by_routine.insert(r.id.clone(), terms);
            }
        }
    }

    let policy = D48Policy {
        routine_by_id: &ctx.routine_by_id,
        cone_derived: &ctx.cone_derived,
        edges_by_from: &ctx.graph.edges_by_from,
        io_terminals_by_routine: &io_terminals_by_routine,
        call_site_by_id: &ctx.call_site_by_id,
    };

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;
    let mut skipped_opaque_callee = 0u64;
    let mut skipped_dynamic_dispatch = 0u64;

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

        let loop_by_id: HashMap<&str, &crate::program::body::features::PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();

        // (a) Direct in-loop IO ops within this routine.
        if let Some(direct_terminals) = io_terminals_by_routine.get(&routine.id) {
            for terminal in direct_terminals {
                // Re-fetch the witness callsite for its loop stack.
                let Some(cs) = routine
                    .call_sites
                    .iter()
                    .find(|c| c.id == terminal.witness_callsite_id)
                else {
                    continue;
                };
                if cs.loop_stack.is_empty() {
                    continue;
                }
                let Some(rep) = cs.loop_stack.last() else {
                    continue;
                };
                let Some(loop_info) = loop_by_id.get(rep.as_str()).copied() else {
                    continue;
                };

                let loop_step = EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: Some(loop_info.id.clone()),
                    source_anchor: anchor_of(&loop_info.source_anchor, routine),
                    note: format!("{} loop", loop_info.loop_type),
                };
                let note = io_method_note(&terminal.io_kind, terminal.method_name.as_deref());
                let io_step = EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: Some(terminal.witness_callsite_id.clone()),
                    loop_id: None,
                    source_anchor: terminal.source_anchor.clone(),
                    note,
                };

                let severity = severity_for_io_kind(&terminal.io_kind);
                let method_for_cause = terminal
                    .method_name
                    .clone()
                    .unwrap_or_else(|| "IO".to_string());
                let mut finding = Finding {
                    id: format!("d48/{}/{}", routine.id, terminal.witness_callsite_id),
                    root_cause_key: format!("d48/{}", routine.id),
                    detector: DETECTOR.to_string(),
                    title: "External IO inside a loop".into(),
                    root_cause: format!(
                        "A {} loop in {} directly calls {} {} on every iteration.",
                        loop_info.loop_type,
                        routine.name,
                        terminal.io_kind.to_uppercase(),
                        method_for_cause
                    ),
                    severity: severity.to_string(),
                    confidence: to_confidence(&[], "likely"),
                    primary_location: terminal.source_anchor.clone(),
                    evidence_path: vec![loop_step, io_step],
                    additional_paths: None,
                    affected_objects: vec![routine.object_id.as_str().into()],
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Move the {} call outside the loop or batch requests to avoid N \
                             external calls.",
                            terminal.io_kind.to_uppercase()
                        )
                        .into(),
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
                finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
                findings.push(finding);
            }
        }

        // (b) In-loop calls to IO-touching callees — walk the call chain.
        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue;
            }
            let Some(rep) = cs.loop_stack.last() else {
                continue;
            };
            let Some(loop_info) = loop_by_id.get(rep.as_str()).copied() else {
                continue;
            };

            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges
                    .iter()
                    .find(|e| e.callsite_id.as_deref() == Some(cs.id.as_str()))
            });
            let Some(edge) = edge else {
                skipped_opaque_callee += 1;
                continue;
            };
            if edge.kind == "interface" || edge.kind == "dynamic" {
                skipped_dynamic_dispatch += 1;
                continue;
            }
            if !routine_touches_external_io(&ctx.cone_derived, &edge.to) {
                continue;
            }

            let loop_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: Some(loop_info.id.clone()),
                source_anchor: anchor_of(&loop_info.source_anchor, routine),
                note: format!("{} loop", loop_info.loop_type),
            };
            let to_name = ctx
                .routine_by_id
                .get(edge.to.as_str())
                .map(|r| r.name.clone())
                .unwrap_or_else(|| edge.to.clone());
            let call_step = EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: Some(cs.id.clone()),
                loop_id: None,
                source_anchor: anchor_of(&cs.source_anchor, routine),
                note: format!("calls {to_name}"),
            };

            let results = walk_evidence(
                &edge.to,
                &policy,
                BOUNDS,
                WalkOpts {
                    initial_loop_depth: cs.loop_stack.len() as i64,
                    initial_steps: vec![loop_step, call_step],
                },
                ctx.uncertainty_view(),
                // Hot-tier walk stats are a d1-only concern for now.
                None,
            );

            for result in &results {
                if result.stop != WalkStop::Complete {
                    continue;
                }
                if result.effective_loop_depth == 0 {
                    continue;
                }
                let Some(last_step) = result.path.last() else {
                    continue;
                };
                let terminal_routine_id = last_step.routine_id.clone();
                let Some(terminal_callsite_id) = last_step.callsite_id.clone() else {
                    continue;
                };
                let matched = io_terminals_by_routine
                    .get(&terminal_routine_id)
                    .and_then(|terms| {
                        terms
                            .iter()
                            .find(|t| t.witness_callsite_id == terminal_callsite_id)
                    });
                let Some(matched) = matched else {
                    continue;
                };

                let severity = severity_for_io_kind(&matched.io_kind);
                let io_method_note_s =
                    io_method_note(&matched.io_kind, matched.method_name.as_deref());

                let mut affected_objects: BTreeSet<String> = BTreeSet::new();
                affected_objects.insert(routine.object_id.clone());
                if let Some(tr) = ctx.routine_by_id.get(terminal_routine_id.as_str()) {
                    affected_objects.insert(tr.object_id.clone());
                }

                let terminal_routine_name = ctx
                    .routine_by_id
                    .get(terminal_routine_id.as_str())
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| terminal_routine_id.clone());

                let mut finding = Finding {
                    id: format!(
                        "d48/{}/{}/{}",
                        routine.id, terminal_routine_id, terminal_callsite_id
                    ),
                    root_cause_key: format!("d48/{}", routine.id),
                    detector: DETECTOR.to_string(),
                    title: "External IO inside a loop".into(),
                    root_cause: format!(
                        "A {} loop in {} reaches {} in {} on every iteration.",
                        loop_info.loop_type, routine.name, io_method_note_s, terminal_routine_name
                    ),
                    severity: severity.to_string(),
                    confidence: to_confidence(&uncertainty_lites(&result.uncertainties), "likely"),
                    primary_location: matched.source_anchor.clone(),
                    evidence_path: result.path.clone(),
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Move the {} call outside the loop or batch requests to avoid N \
                             external calls per iteration.",
                            matched.io_kind.to_uppercase()
                        )
                        .into(),
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
                finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
                findings.push(finding);
            }
        }
    }

    // Sort by id (compareStrings) then dedup by id (first-wins).
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: HashSet<String> = HashSet::new();
    let mut emitted: Vec<Finding> = Vec::new();
    for f in findings {
        if seen.insert(f.id.clone()) {
            emitted.push(f);
        }
    }

    let count = emitted.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, count);
    stats.add_skip("parseIncomplete", skipped_parse_incomplete);
    stats.add_skip("opaqueCallee", skipped_opaque_callee);
    stats.add_skip("dynamicDispatch", skipped_dynamic_dispatch);
    Ok(DetectorOutput {
        findings: emitted,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

/// Convert accumulated `Uncertainty` to `UncertaintyLite` (`UncertaintyLite::of`
/// applies the shared callsiteId → operationId → routineId precedence).
fn uncertainty_lites(
    uncertainties: &[crate::engine::l4::summary::Uncertainty],
) -> Vec<UncertaintyLite> {
    uncertainties.iter().map(UncertaintyLite::of).collect()
}
