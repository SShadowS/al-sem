//! D46 — `Commit` reachable from an Install/Upgrade codeunit lifecycle trigger.
//! Port of al-sem `src/detectors/d46-commit-in-lifecycle.ts`. OPT-IN (kept out of
//! the default al-sem registry; the R4 differential filters by detector name, so
//! registering it here only surfaces it when a fixture explicitly requests it).
//!
//! Start condition: `routine.kind === "trigger"` (primary, body_available,
//! !parse_incomplete) on a Codeunit whose `object_subtype` is "install" or
//! "upgrade" (case-insensitive). Walk: transitive via `walk_evidence`, terminating
//! at `operation_sites` where `kind == "commit"`; event-dispatch edges are NOT
//! followed (D2 owns those). Emits only `WalkStop::Complete` paths.
//!
//! Severity: `high` in all cases. Confidence: `likely` (capped to `possible` if the
//! path accumulated any uncertainty). Within-detector sort by `compareStrings(id)`,
//! then dedup by id (first-wins); fingerprint pre-projection over internal ids.

use std::collections::{HashMap, HashSet};

use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l4::summary::Uncertainty;
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, SourceAnchor};
use crate::engine::l5::path_walker::{
    PathCtx, Terminal, WalkBounds, WalkOpts, WalkPolicy, WalkStop, walk_evidence,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::{PAnchor, PCallSite, POperationSite};
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d46-commit-in-lifecycle";

/// The standard interprocedural budget shared by D1/D2's walk_evidence callers.
const BOUNDS: WalkBounds = WalkBounds {
    max_depth: 20,
    max_nodes: 500,
};

/// Build a `SourceAnchor` from a `PAnchor` with an explicit enclosing routine id.
fn anchor_from(a: &PAnchor, routine_id: &str) -> SourceAnchor {
    SourceAnchor {
        source_unit_id: a.source_unit_id.clone(),
        start_line: a.start_line,
        start_column: a.start_column,
        end_line: a.end_line,
        end_column: a.end_column,
        enclosing_routine_id: routine_id.to_string(),
        syntax_kind: a.syntax_kind.clone(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    }
}

/// Convert a walk's accumulated `Uncertainty` set to the `UncertaintyLite` shape
/// `to_confidence` consumes (`UncertaintyLite::of` applies the shared
/// callsiteId → operationId → routineId precedence).
fn uncertainty_lites(uncertainties: &[Uncertainty]) -> Vec<UncertaintyLite> {
    uncertainties.iter().map(UncertaintyLite::of).collect()
}

/// The D46 WalkPolicy — follows non-event-dispatch call edges, terminates at any
/// `operation_sites` commit in the visited routine. The Terminal's `op_id` carries
/// the commit operation id; `build_terminal_step` recovers its anchor.
struct D46Policy<'a> {
    routine_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
    /// RoutineId → its commit operation sites (precomputed for O(1) lookup).
    commit_sites_by_routine: &'a HashMap<String, Vec<&'a POperationSite>>,
    edges_by_from: &'a HashMap<String, Vec<CombinedEdge>>,
}

impl<'a> WalkPolicy for D46Policy<'a> {
    fn terminals_at(&self, node: &str, _ctx: &PathCtx) -> Vec<Terminal> {
        self.commit_sites_by_routine
            .get(node)
            .map(|sites| {
                sites
                    .iter()
                    .map(|s| Terminal {
                        routine_id: node.to_string(),
                        local_loop_depth: s.loop_stack.len() as i64,
                        op_id: Some(s.id.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn expand(&self, node: &str, _ctx: &PathCtx) -> Vec<CombinedEdge> {
        self.edges_by_from
            .get(node)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| e.kind != "event-dispatch")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn build_hop_step(&self, edge: &CombinedEdge, _ctx: &PathCtx) -> EvidenceStep {
        // Look up the callsite anchor (CombinedEdge has no sourceAnchor itself).
        let from_routine = self.routine_by_id.get(edge.from.as_str()).copied();
        let cs: Option<&PCallSite> = edge.callsite_id.as_ref().and_then(|cid| {
            from_routine.and_then(|fr| fr.call_sites.iter().find(|c| &c.id == cid))
        });
        let to_name = self
            .routine_by_id
            .get(edge.to.as_str())
            .map(|r| r.name.clone())
            .unwrap_or_else(|| edge.to.clone());
        let source_anchor = if let Some(cs) = cs {
            anchor_from(&cs.source_anchor, &edge.from)
        } else if let Some(fr) = from_routine {
            anchor_from(&fr.source_anchor, &edge.from)
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
        // Recover the commit op site for its anchor (op_id was emitted by
        // terminals_at over the SAME routine's commit sites).
        let op = t.op_id.as_ref().and_then(|oid| {
            self.commit_sites_by_routine
                .get(&t.routine_id)
                .and_then(|sites| sites.iter().find(|s| &s.id == oid).copied())
        });
        let source_anchor = match op {
            Some(s) => anchor_from(&s.source_anchor, &t.routine_id),
            None => SourceAnchor {
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
        };
        EvidenceStep {
            routine_id: t.routine_id.clone(),
            operation_id: t.op_id.clone(),
            callsite_id: None,
            loop_id: None,
            source_anchor,
            note: "Commit".to_string(),
        }
    }

    fn loop_depth_of_edge(&self, _edge: &CombinedEdge) -> i64 {
        // d46 does not track loop depth (it never reads effective_loop_depth); the
        // walk's depth contribution is irrelevant. al-sem's d46 also supplies no
        // loopDepthOfEdge effect beyond the default callsite lookup; since the
        // finding ignores it, returning 0 is byte-faithful.
        0
    }
}

pub fn detect_d46(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    // Pre-build RoutineId → commit operation sites for O(1) terminal lookup.
    let mut commit_sites_by_routine: HashMap<String, Vec<&POperationSite>> = HashMap::new();
    for r in &ws.routines {
        let commits: Vec<&POperationSite> = r
            .operation_sites
            .iter()
            .filter(|s| s.kind == "commit")
            .collect();
        if !commits.is_empty() {
            commit_sites_by_routine.insert(r.id.clone(), commits);
        }
    }

    let policy = D46Policy {
        routine_by_id: &ctx.routine_by_id,
        commit_sites_by_routine: &commit_sites_by_routine,
        edges_by_from: &ctx.graph.edges_by_from,
    };

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_parse_incomplete = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) === "primary": source-only ⇒ always true.
        // Only lifecycle triggers — kind must be "trigger".
        if routine.kind != "trigger" {
            continue;
        }
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            skipped_parse_incomplete += 1;
            continue;
        }

        // Only Install or Upgrade codeunits.
        let Some(obj) = ctx.objects_by_id.get(routine.object_id.as_str()).copied() else {
            continue;
        };
        if obj.object_type != "Codeunit" {
            continue;
        }
        let subtype_lc = obj.object_subtype.as_deref().map(str::to_lowercase);
        let is_lifecycle = matches!(subtype_lc.as_deref(), Some("install") | Some("upgrade"));
        if !is_lifecycle {
            continue;
        }
        // The raw (original-case) subtype for the rootCause / note text.
        let subtype_raw = obj.object_subtype.as_deref().unwrap_or("");

        candidates_considered += 1;

        // Initial evidence step: the lifecycle trigger as the root.
        let initial_steps = vec![EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_from(&routine.source_anchor, &routine.id),
            note: format!("{} ({} trigger)", routine.name, subtype_raw),
        }];

        let results = walk_evidence(
            &routine.id,
            &policy,
            BOUNDS,
            WalkOpts {
                initial_loop_depth: 0,
                initial_steps,
            },
            ctx.uncertainty_view(),
            // Hot-tier walk stats are a d1-only concern for now.
            None,
        );

        for result in &results {
            if result.stop != WalkStop::Complete {
                continue;
            }
            let Some(terminal) = result.path.last() else {
                continue;
            };
            let Some(terminal_op_id) = terminal.operation_id.as_ref() else {
                continue;
            };

            let id = format!("d46/{}/{}", routine.id, terminal_op_id);
            let root_cause_key = format!("d46/{}", routine.id);

            let mut finding = Finding {
                id,
                root_cause_key,
                detector: DETECTOR.to_string(),
                title: "Commit reachable from Install/Upgrade lifecycle trigger".into(),
                root_cause: format!(
                    "{} is an {} codeunit trigger that reaches Commit \u{2014} the platform's \
                     deploy transaction becomes non-atomic and cannot be rolled back if an \
                     error occurs after the Commit.",
                    routine.name, subtype_raw
                ),
                severity: "high".to_string(),
                confidence: to_confidence(&uncertainty_lites(&result.uncertainties), "likely"),
                primary_location: anchor_from(&routine.source_anchor, &routine.id),
                evidence_path: result.path.clone(),
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Remove the Commit from the install/upgrade path. The platform \
                                  wraps the Install/Upgrade trigger in its own transaction \u{2014} \
                                  an explicit Commit breaks that guarantee."
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
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    // Sort by id, then dedup by id (same commit site via multiple paths → first).
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
    Ok(DetectorOutput {
        findings: emitted,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
