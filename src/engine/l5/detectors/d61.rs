//! D61 — `IsHandled := true` bypasses a critical write (OPT-IN). BCQuality
//! `do-not-bypass-critical-operations-with-ishandled`. d43 flags the generic
//! IsHandled-skip shape; d61 refines: the publisher-side guard protects a
//! record WRITE, and a subscriber provably sets the flag — the write is
//! silently skippable by an extension.
//!
//! Join (every leg exact, every uncertainty skips):
//!  1. publisher: event-publisher routine with a var Boolean param named
//!     ishandled/handled;
//!  2. caller: routine with a RESOLVED call to that publisher, binding a local
//!     var to the IsHandled param; a post-call `if` guard on that var
//!     (condition_references) whose statement contains a record write op — not
//!     in the branch a simple guard runs when the var is TRUE (`if X then
//!     <write>` runs the write when the subscriber sets the flag);
//!  3. subscriber: an event-graph subscriber of the same event assigning
//!     literal `true` to its own ishandled/handled param.
//!
//! Finding per (caller callsite × subscriber). Severity: high. Confidence:
//! likely when the subscriber body has NO branching (unconditional claim),
//! else possible.

use std::collections::HashMap;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, before_anchor};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PAnchor;
use crate::program::model::workspace::{Model, ModelRoutine};

const DETECTOR: &str = "d61-ishandled-bypasses-critical-write";

const CRITICAL_WRITE_OPS: &[&str] = &[
    "Insert",
    "Modify",
    "Delete",
    "DeleteAll",
    "ModifyAll",
    "Rename",
];

fn is_ishandled_name(raw: &str) -> bool {
    let n = raw.trim_matches('"').to_lowercase();
    n == "ishandled" || n == "handled"
}

fn anchor_within(inner: &PAnchor, outer: &PAnchor) -> bool {
    let starts_ok = outer.start_line < inner.start_line
        || (outer.start_line == inner.start_line && outer.start_column <= inner.start_column);
    let ends_ok = inner.end_line < outer.end_line
        || (inner.end_line == outer.end_line && inner.end_column <= outer.end_column);
    starts_ok && ends_ok
}

/// Whether `write` can be skipped by the flag. `false` only when the guard is a
/// SIMPLE one (`if X`, `if not X`, `if X = false`: the `if` node's
/// `conditionGuard`) and the write is in the branch that runs when X is TRUE (the
/// THEN of `if X`, the ELSE of `if not X`) — then setting the flag runs the
/// write rather than bypassing it. A compound condition (`if A and not X`) has no
/// `conditionGuard`, and its structure is not recorded, so it stays a candidate.
fn write_skipped_when_flag_set(
    caller: &ModelRoutine,
    guard: &crate::program::body::features::PConditionReference,
    write: &PAnchor,
) -> bool {
    use crate::program::body::features::PCFNNode;
    fn find(n: &PCFNNode, at: (u32, u32)) -> Option<&PCFNNode> {
        if n.kind == "if" && n.source_range.map(|r| (r.0, r.1)) == Some(at) {
            return Some(n);
        }
        n.children
            .iter()
            .chain(n.else_children.iter())
            .flatten()
            .find_map(|c| find(c, at))
    }
    let within = |branch: &Option<Vec<PCFNNode>>| {
        branch.iter().flatten().any(|c| {
            c.source_range.is_some_and(|(sl, sc, el, ec)| {
                let outer = PAnchor {
                    start_line: sl,
                    start_column: sc,
                    end_line: el,
                    end_column: ec,
                    ..write.clone()
                };
                anchor_within(write, &outer)
            })
        })
    };
    let a = &guard.statement_anchor;
    let Some(node) = caller
        .statement_tree
        .as_ref()
        .and_then(|t| find(t, (a.start_line, a.start_column)))
    else {
        return true;
    };
    match node.condition_guard.as_ref() {
        Some(g) if g.identifier == guard.identifier => match g.polarity.as_str() {
            "positive" => !within(&node.children),
            "negative" => !within(&node.else_children),
            _ => true,
        },
        _ => true,
    }
}

pub fn detect_d61(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_no_critical_write = 0u64;
    let mut skipped_write_runs_when_handled = 0u64;
    let mut skipped_no_flipping_subscriber = 0u64;

    // Leg 1: IsHandled-pattern publishers → (param index, event id).
    let mut publisher_meta: HashMap<&str, (u32, &str)> = HashMap::new();
    let mut event_by_publisher: HashMap<&str, &str> = HashMap::new();
    for ev in &ctx.event_graph.events {
        if let Some(pr) = &ev.publisher_routine_id {
            event_by_publisher.insert(pr.as_str(), ev.id.as_str());
        }
    }
    for r in &ws.routines {
        if r.kind != "event-publisher" {
            continue;
        }
        let Some(p) = r.parameters.iter().find(|p| {
            p.is_var
                && p.type_text.trim().eq_ignore_ascii_case("boolean")
                && is_ishandled_name(&p.name)
        }) else {
            continue;
        };
        let Some(&event_id) = event_by_publisher.get(r.id.as_str()) else {
            continue;
        };
        publisher_meta.insert(r.id.as_str(), (p.index, event_id));
    }
    if publisher_meta.is_empty() {
        let stats = DetectorStats::new(DETECTOR, 0, 0);
        return Ok(DetectorOutput::no_diag(findings, stats));
    }

    // Leg 3: subscribers that assign literal true to their ishandled param,
    // keyed by event id.
    let routine_by_id = &ctx.routine_by_id;
    let mut flippers_by_event: HashMap<&str, Vec<(&ModelRoutine, &PAnchor)>> = HashMap::new();
    for edge in &ctx.event_graph.edges {
        let Some(sub) = routine_by_id.get(edge.subscriber_routine_id.as_str()) else {
            continue;
        };
        if !sub.body_available || sub.parse_incomplete {
            continue;
        }
        let Some(asg) = sub.var_assignments.iter().find(|a| {
            is_ishandled_name(&a.lhs_name)
                && a.rhs_literal_value
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        }) else {
            continue;
        };
        flippers_by_event
            .entry(edge.event_id.as_str())
            .or_default()
            .push((sub, &asg.source_anchor));
    }

    // Leg 2: callers with a post-call guard protecting a critical write.
    for caller in &ws.routines {
        if !caller.body_available || caller.parse_incomplete {
            continue;
        }
        for cs in &caller.call_sites {
            let Some(edge) = ctx.resolved_call_edge_by_callsite.get(&cs.id) else {
                continue;
            };
            let Some(to) = edge.to.as_deref() else {
                continue;
            };
            let Some(&(param_index, event_id)) = publisher_meta.get(to) else {
                continue;
            };
            let Some(publisher) = routine_by_id.get(to) else {
                continue;
            };
            // The caller-side variable bound to the IsHandled param. A plain
            // scalar Boolean local is NOT a record variable, so the L3
            // argument-binding classifier leaves it `sourceKind: "unknown"` /
            // `sourceVariableName: None` (that classifier only resolves
            // record-shaped bindings) — mirrors d43's `enumerate_dispatch_sites`
            // (d43.rs:134-161): prefer the binding's `source_variable_name` when
            // classified, else fall back to the raw (trimmed, lowercased)
            // argument text.
            let binding = cs
                .argument_bindings
                .iter()
                .find(|b| b.parameter_index == param_index);
            let name_from_binding = binding
                .filter(|b| b.source_kind != "unknown")
                .and_then(|b| b.source_variable_name.as_deref())
                .filter(|n| !n.is_empty())
                .map(|s| s.to_string());
            let name_from_text = if name_from_binding.is_none() {
                cs.argument_texts
                    .get(param_index as usize)
                    .map(|t| t.trim().to_lowercase())
            } else {
                None
            };
            let Some(guard_var) = name_from_binding
                .or(name_from_text)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            // Post-call guard statement referencing that var.
            let Some(guard) = caller.condition_references.iter().find(|cr| {
                cr.identifier == guard_var && before_anchor(&cs.source_anchor, &cr.statement_anchor)
            }) else {
                continue;
            };
            candidates_considered += 1;
            // A critical write inside the guarded statement.
            let Some(write) = caller.record_operations.iter().find(|op| {
                CRITICAL_WRITE_OPS.contains(&op.op.as_str())
                    && anchor_within(&op.source_anchor, &guard.statement_anchor)
            }) else {
                skipped_no_critical_write += 1;
                continue;
            };
            // The write must sit in the branch that runs when the flag is FALSE:
            // `if not IsHandled then <write>` or `if IsHandled then .. else <write>`.
            // A write that runs when the subscriber set the flag is not bypassed
            // (engine-switch S7.6 triage: `if IsReportLayoutSelected then ..
            // Modify()` after OnSelectReportLayout, 5 false positives on CDO/DO).
            if !write_skipped_when_flag_set(caller, guard, &write.source_anchor) {
                skipped_write_runs_when_handled += 1;
                continue;
            }
            let Some(flippers) = flippers_by_event.get(event_id) else {
                skipped_no_flipping_subscriber += 1;
                continue;
            };

            for (sub, asg_anchor) in flippers {
                let unconditional = !sub.has_branching;
                let confidence: FindingConfidence =
                    to_confidence(&[], if unconditional { "likely" } else { "possible" });
                let mut finding = Finding {
                    id: format!("d61/{}/{}/{}", caller.id, cs.id, sub.id),
                    root_cause_key: format!("d61/{}/{}", caller.id, cs.id),
                    detector: DETECTOR.to_string(),
                    title: "IsHandled bypasses critical write".into(),
                    root_cause: format!(
                        "{} guards a {} on {} behind `if not {}` after publishing {}; \
                         subscriber {} sets {} := true{} — the write is silently skipped.",
                        caller.name,
                        write.op,
                        write.record_variable_name,
                        guard_var,
                        publisher.name,
                        sub.name,
                        guard_var,
                        if unconditional {
                            " unconditionally"
                        } else {
                            ""
                        }
                    ),
                    severity: "high".to_string(),
                    confidence,
                    primary_location: anchor_of(&write.source_anchor, caller),
                    evidence_path: vec![
                        EvidenceStep {
                            routine_id: caller.id.clone(),
                            operation_id: None,
                            callsite_id: Some(cs.id.clone()),
                            loop_id: None,
                            source_anchor: anchor_of(&cs.source_anchor, caller),
                            note: format!("publishes {}", publisher.name),
                        },
                        EvidenceStep {
                            routine_id: caller.id.clone(),
                            operation_id: Some(write.id.clone()),
                            callsite_id: None,
                            loop_id: None,
                            source_anchor: anchor_of(&write.source_anchor, caller),
                            note: format!("guarded critical {}", write.op),
                        },
                        EvidenceStep {
                            routine_id: sub.id.clone(),
                            operation_id: None,
                            callsite_id: None,
                            loop_id: None,
                            source_anchor: anchor_of(asg_anchor, sub),
                            note: format!("subscriber sets {guard_var} := true"),
                        },
                    ],
                    additional_paths: None,
                    affected_objects: id_list([caller.object_id.as_str(), sub.object_id.as_str()]),
                    affected_tables: id_list(write.table_id.iter().cloned()),
                    fix_options: vec![FixOption {
                        description: "If the subscriber replaces the write, make it perform an \
                                      equivalent durable operation; otherwise restrict the \
                                      IsHandled contract to non-critical steps (split the event)."
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
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("noCriticalWrite", skipped_no_critical_write);
    stats.add_skip("writeRunsWhenHandled", skipped_write_runs_when_handled);
    stats.add_skip("noFlippingSubscriber", skipped_no_flipping_subscriber);
    Ok(DetectorOutput::no_diag(findings, stats))
}
