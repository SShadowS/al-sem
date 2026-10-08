//! D43 — event-IsHandled-skip. Port of al-sem
//! `src/detectors/d43-event-ishandled-skip.ts`.
//!
//! Flags a publisher whose `IsHandled` guard lets a subscriber skip the publisher's
//! default write: the caller guards table writes on an `IsHandled` actual, a
//! subscriber of that event sets `IsHandled := true`, but the subscriber does NOT
//! write the guarded table — so the default write is skipped and nothing replaces it.
//!
//! ## Substrate
//! - `enumerate_dispatch_sites` (ported below from al-sem `engine/dispatch-sites.ts`)
//!   walks `ctx.graph.typed_edges` for caller→publisher edges, finds the publisher's
//!   `var Boolean` IsHandled formal, binds the caller's actual, and collects post-call
//!   `conditionReferences` guards + the caller's guarded table writes.
//! - `classify_subscriber` reads the subscriber's `var_assignments` + `has_branching`
//!   + `statement_tree` (CFN nesting) to decide mustSetTrue / maySetTrue / noSetTrue.
//! - `build_cross_extension_subscribers` (event_flow.rs) populates the finding's
//!   `cross_extension_subscribers`; `event_kind_of` populates `event_kind`.
//!
//! ## Substrate guard
//! If NO routine has any conditionReference AND there is ≥1 event subscriber, al-sem
//! pushes one warning diagnostic and bails with zero findings. The Rust port mirrors
//! the BAIL (the diagnostic is informational only and does not reach the byte-parity
//! surface, so it is dropped — DetectorOutput carries no diagnostics).
//!
//! Within-detector sort by `compareStrings(a.id, b.id)` (byte order). Fingerprint
//! computed PER-FINDING before the sort (al-sem computes it inside the loop).

use std::collections::{HashMap, HashSet};

use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::event_flow::{event_kind_of, is_handled_re};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor,
};
use crate::program::body::features::{PAnchor, PCFNNode, PConditionReference};
use crate::program::model::workspace::{Model, ModelRoutine};

use super::anchor_of;

const DETECTOR: &str = "d43-event-ishandled-skip";

// ===========================================================================
// DispatchSite substrate (port of al-sem engine/dispatch-sites.ts)
// ===========================================================================

/// The caller's actual variable bound to the publisher's IsHandled formal.
struct DispatchSiteHandledActual {
    /// Lowercased identifier name (the conditionReference match key).
    variable_name: String,
}

/// One caller→publisher dispatch site. Mirrors al-sem `DispatchSite`.
struct DispatchSite {
    event_id: String,
    caller_routine: String,
    callsite_id: String,
    handled_actual: Option<DispatchSiteHandledActual>,
    /// Caller conditionReferences referencing the IsHandled actual AFTER the callsite.
    post_call_guards: Vec<PConditionReference>,
    /// Tables the caller writes (transitively) when a post-call guard exists.
    guarded_tables_written: Vec<String>,
}

/// `/^boolean$/i` — case-insensitive exact "boolean".
fn is_boolean_type(type_text: &str) -> bool {
    type_text.eq_ignore_ascii_case("boolean")
}

/// `enumerateDispatchSites` — bridge the event-flow substrate with d43's dispatch-site
/// guard analysis. Faithful port.
fn enumerate_dispatch_sites(
    ctx: &DetectorContext,
    routine_by_id: &HashMap<&str, &ModelRoutine>,
) -> Vec<DispatchSite> {
    let mut out: Vec<DispatchSite> = Vec::new();

    // publisher routine id → event id (from the event graph; authoritative EventId).
    let mut publisher_event_id: HashMap<&str, &str> = HashMap::new();
    for ev in &ctx.event_graph.events {
        if let Some(pr) = &ev.publisher_routine_id {
            publisher_event_id.insert(pr.as_str(), ev.id.as_str());
        }
    }

    for edge in &ctx.graph.typed_edges {
        if edge.kind != "direct-call"
            && edge.kind != "object-run-resolved"
            && edge.kind != "dependency-export"
        {
            continue;
        }
        let Some(to) = &edge.to else {
            continue;
        };
        let Some(event_id) = publisher_event_id.get(to.as_str()).copied() else {
            continue; // target is not a publisher
        };
        let Some(callsite_id) = &edge.callsite_id else {
            continue;
        };
        let Some(caller) = routine_by_id.get(edge.from.as_str()).copied() else {
            continue;
        };
        let Some(publisher) = routine_by_id.get(to.as_str()).copied() else {
            continue;
        };

        // Find the callsite in the caller's call sites matching the edge's callsiteId.
        let Some(call_site) = caller.call_sites.iter().find(|cs| &cs.id == callsite_id) else {
            continue;
        };

        // Publisher's IsHandled-shaped formal: var Boolean matching IS_HANDLED_RE.
        let mut handled_formal_index: Option<usize> = None;
        for (i, p) in publisher.parameters.iter().enumerate() {
            if !p.is_var {
                continue;
            }
            if !is_boolean_type(&p.type_text) {
                continue;
            }
            if !is_handled_re(&p.name) {
                continue;
            }
            handled_formal_index = Some(i);
            break;
        }

        // Identify the caller's actual bound to the IsHandled formal.
        let mut handled_actual: Option<DispatchSiteHandledActual> = None;
        if let Some(idx) = handled_formal_index {
            // Primary: argumentBindings[idx].sourceVariableName (when sourceKind != unknown).
            let name_from_binding = call_site
                .argument_bindings
                .get(idx)
                .filter(|b| b.source_kind != "unknown")
                .and_then(|b| b.source_variable_name.as_ref())
                .filter(|n| !n.is_empty())
                .cloned();

            // Fallback: argumentTexts[idx], trimmed + lowercased.
            let name_from_text = if name_from_binding.is_none() {
                call_site
                    .argument_texts
                    .get(idx)
                    .map(|t| t.trim().to_lowercase())
            } else {
                None
            };

            let var_name = name_from_binding.or(name_from_text);
            if let Some(vn) = var_name
                && !vn.is_empty()
            {
                handled_actual = Some(DispatchSiteHandledActual { variable_name: vn });
            }
        }

        // Post-call conditionReferences from the CALLER's features.
        let mut post_call_guards: Vec<PConditionReference> = Vec::new();
        if let Some(ha) = &handled_actual {
            let call_row = call_site.source_anchor.start_line;
            let call_col = call_site.source_anchor.start_column;
            let actual_lower = ha.variable_name.to_lowercase();
            for cref in &caller.condition_references {
                if cref.identifier != actual_lower {
                    continue;
                }
                let r = &cref.reference_anchor;
                if r.start_line < call_row {
                    continue;
                }
                if r.start_line == call_row && r.start_column <= call_col {
                    continue;
                }
                post_call_guards.push(cref.clone());
            }
        }

        // Guarded writes: full caller transitive writes when ≥1 post-call guard exists.
        let mut guarded_tables_written: Vec<String> = Vec::new();
        if !post_call_guards.is_empty()
            && let Some(summary) = ctx.summaries.get(&caller.id)
        {
            guarded_tables_written = ctx
                .cone_derived
                .writes_physical_tables_of(&summary.routine_id);
        }

        out.push(DispatchSite {
            event_id: event_id.to_string(),
            caller_routine: caller.id.clone(),
            callsite_id: callsite_id.clone(),
            handled_actual,
            post_call_guards,
            guarded_tables_written,
        });
    }

    // Sort: eventId, callerRoutine, callsiteId (compareStrings).
    out.sort_by(|a, b| {
        a.event_id
            .cmp(&b.event_id)
            .then_with(|| a.caller_routine.cmp(&b.caller_routine))
            .then_with(|| a.callsite_id.cmp(&b.callsite_id))
    });

    out
}

// ===========================================================================
// classify_subscriber
// ===========================================================================

// The `MustSetTrue` / `MaySetTrue` / `NoSetTrue` variants share the `SetTrue`
// postfix by design — they mirror al-sem's classification semantics and the
// names are intentional (consistent with the al-sem source).
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetterClassification {
    MustSetTrue,
    MaySetTrue,
    NoSetTrue,
}

/// `isAssignmentNestedInTree` — true when the assignment's start position lands
/// inside a conditional/loop branch of the routine's `statement_tree`. Faithful
/// port (DFS with conditional-context tracking; start-position equality match).
fn is_assignment_nested_in_tree(assignment_anchor: &PAnchor, tree: Option<&PCFNNode>) -> bool {
    let Some(tree) = tree else {
        return false; // no tree → conservative: NOT nested.
    };
    let target_line = assignment_anchor.start_line;
    let target_col = assignment_anchor.start_column;

    fn is_conditional_kind(kind: &str) -> bool {
        matches!(
            kind,
            "if" | "while" | "repeat" | "for" | "foreach" | "case" | "case-branch"
        )
    }

    fn visit(
        node: &PCFNNode,
        in_conditional: bool,
        target_line: u32,
        target_col: u32,
        result: &mut bool,
    ) {
        if *result {
            return;
        }
        if let Some((sl, sc, _, _)) = node.source_range
            && sl == target_line
            && sc == target_col
        {
            *result = in_conditional;
            return;
        }
        let child_conditional = in_conditional || is_conditional_kind(&node.kind);
        if let Some(children) = &node.children {
            for c in children {
                visit(c, child_conditional, target_line, target_col, result);
                if *result {
                    return;
                }
            }
        }
        if let Some(else_children) = &node.else_children {
            for c in else_children {
                visit(c, true, target_line, target_col, result); // else is always conditional
                if *result {
                    return;
                }
            }
        }
    }

    let mut result = false;
    visit(tree, false, target_line, target_col, &mut result);
    result
}

/// `classifySubscriber` — mustSetTrue / maySetTrue / noSetTrue. Faithful port.
fn classify_subscriber(
    subscriber: &str,
    routine_by_id: &HashMap<&str, &ModelRoutine>,
) -> SetterClassification {
    let Some(r) = routine_by_id.get(subscriber).copied() else {
        return SetterClassification::NoSetTrue;
    };
    let sets: Vec<&crate::program::body::features::PVarAssignment> = r
        .var_assignments
        .iter()
        .filter(|a| is_handled_re(&a.lhs_name) && a.rhs_literal_value.as_deref() == Some("true"))
        .collect();
    if sets.is_empty() {
        return SetterClassification::NoSetTrue;
    }
    // hasBranching == false trivially means top-level → mustSetTrue (fast-path).
    if !r.has_branching {
        return SetterClassification::MustSetTrue;
    }
    // A top-level setter guarantees true unless an `exit` before it can leave the
    // routine first: `if not Applies then exit; ... IsHandled := true;` sets it on
    // some paths only (engine-switch S7.4 triage: CDO's eDocs Sending Profile
    // subscriber, reported "confirmed"/"always sets" though it overrides one
    // channel only).
    for setter in &sets {
        let a = &setter.source_anchor;
        if !is_assignment_nested_in_tree(a, r.statement_tree.as_ref())
            && !exit_before(r.statement_tree.as_ref(), (a.start_line, a.start_column))
        {
            return SetterClassification::MustSetTrue;
        }
    }
    SetterClassification::MaySetTrue
}

/// Whether the tree holds an `exit` that starts before `pos` (line, column).
fn exit_before(tree: Option<&PCFNNode>, pos: (u32, u32)) -> bool {
    fn visit(node: &PCFNNode, pos: (u32, u32)) -> bool {
        if node.kind == "exit"
            && let Some((sl, sc, _, _)) = node.source_range
            && (sl, sc) < pos
        {
            return true;
        }
        node.children
            .iter()
            .chain(node.else_children.iter())
            .flatten()
            .any(|c| visit(c, pos))
    }
    tree.is_some_and(|t| visit(t, pos))
}

/// `classifyConfidence` — the confidence ladder. Faithful port.
fn classify_confidence(site: &DispatchSite, setter: SetterClassification) -> &'static str {
    if site.post_call_guards.is_empty() {
        return "possible";
    }
    if setter == SetterClassification::MustSetTrue && site.handled_actual.is_some() {
        return "confirmed";
    }
    if setter == SetterClassification::MaySetTrue {
        return "likely";
    }
    "possible"
}

// ===========================================================================
// detect_d43
// ===========================================================================

pub fn detect_d43(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<crate::engine::l5::registry::DetectorOutput, crate::engine::l5::registry::DetectorError>
{
    use crate::engine::l5::registry::{DetectorOutput, DetectorStats};

    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates = 0usize;
    let mut skipped_no_guard = 0u64;
    let mut skipped_no_setter = 0u64;

    // Substrate guard: no conditionReference anywhere AND ≥1 event subscriber → bail.
    let saw_any_condition_ref = ws
        .routines
        .iter()
        .any(|r| !r.condition_references.is_empty());
    let event_subscriber_count = ws
        .routines
        .iter()
        .filter(|r| r.kind == "event-subscriber")
        .count();
    if !saw_any_condition_ref && event_subscriber_count > 0 {
        // al-sem pushes a warning diagnostic + bails (mirrors index.ts ctx.diagnostics.push).
        // This diagnostic propagates to RunOutput.diagnostics and thence to the JSON envelope.
        return Ok(DetectorOutput {
            findings: Vec::new(),
            stats: DetectorStats::new(DETECTOR, 0, 0),
            diagnostics: vec![crate::engine::l5::registry::Diagnostic {
                severity: "warning".to_string(),
                stage: "detect".to_string(),
                message: format!(
                    "{}: conditionReferences substrate empty; dispatch-site detection limited",
                    DETECTOR
                ),
            }],
            d1_cohort_index: None,
        });
    }

    // eventKind per internal eventId.
    let mut event_kind_by_id: HashMap<&str, &'static str> = HashMap::new();
    for ev in &ctx.event_graph.events {
        event_kind_by_id.insert(ev.id.as_str(), event_kind_of(&ev.event_kind));
    }

    // Cross-extension subscriber lookup per event — shared, built once in ctx.
    let cross_ext_by_event = &ctx.cross_extension_subscribers;

    let sites = enumerate_dispatch_sites(ctx, &ctx.routine_by_id);
    for site in &sites {
        if site.post_call_guards.is_empty() {
            skipped_no_guard += 1;
            continue;
        }
        if site.guarded_tables_written.is_empty() {
            skipped_no_guard += 1;
            continue;
        }
        let subs = ctx
            .event_flow_indexes
            .subscribers_by_event
            .get(&site.event_id)
            .cloned()
            .unwrap_or_default();

        // Setters: subs classified as must/maySetTrue.
        let mut setters: Vec<(String, SetterClassification)> = Vec::new();
        for sub in &subs {
            let c = classify_subscriber(sub, &ctx.routine_by_id);
            if c != SetterClassification::NoSetTrue {
                setters.push((sub.clone(), c));
            }
        }
        if setters.is_empty() {
            skipped_no_setter += 1;
            continue;
        }

        let guarded_set: HashSet<&str> = site
            .guarded_tables_written
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Coverage candidates: subs writing ≥1 guarded table.
        let mut coverage_candidates_count = 0usize;
        for sub in &subs {
            let Some(r) = ctx.routine_by_id.get(sub.as_str()).copied() else {
                continue;
            };
            let Some(summary) = ctx.summaries.get(&r.id) else {
                continue;
            };
            if ctx
                .cone_derived
                .writes_physical_tables_of(&summary.routine_id)
                .iter()
                .any(|t| guarded_set.contains(t.as_str()))
            {
                coverage_candidates_count += 1;
            }
        }

        for (setter, classification) in &setters {
            candidates += 1;
            let Some(r) = ctx.routine_by_id.get(setter.as_str()).copied() else {
                continue;
            };
            let Some(summary) = ctx.summaries.get(&r.id) else {
                continue;
            };
            let setter_writes: HashSet<String> = ctx
                .cone_derived
                .writes_physical_tables_of(&summary.routine_id)
                .into_iter()
                .collect();
            let missing: Vec<&String> = site
                .guarded_tables_written
                .iter()
                .filter(|t| !setter_writes.contains(*t))
                .collect();
            if missing.is_empty() {
                continue;
            }
            let coverage_status = if coverage_candidates_count > 0 {
                "candidate-coverage"
            } else {
                "no-other-writers"
            };
            let severity_base = if coverage_status == "candidate-coverage" {
                "medium"
            } else {
                "high"
            };
            let confidence_level = classify_confidence(site, *classification);
            let severity = if confidence_level == "possible" {
                match severity_base {
                    "high" => "medium",
                    "medium" => "low",
                    other => other,
                }
            } else {
                severity_base
            };

            let caller = ctx.routine_by_id.get(site.caller_routine.as_str()).copied();

            for table in &missing {
                let root_cause_key = format!(
                    "d43/{}|{}|{}|{}|{}",
                    site.event_id, site.caller_routine, site.callsite_id, setter, table
                );

                // Evidence step 1: dispatch site (caller anchor, fallback to subscriber anchor).
                let guard_via = site
                    .handled_actual
                    .as_ref()
                    .map(|h| h.variable_name.clone())
                    .unwrap_or_else(|| "IsHandled".to_string());
                let step1_anchor: SourceAnchor = match caller {
                    Some(c) => anchor_of(&c.source_anchor, c),
                    None => anchor_of(&r.source_anchor, r),
                };
                let step1 = EvidenceStep {
                    routine_id: site.caller_routine.clone(),
                    operation_id: None,
                    callsite_id: Some(site.callsite_id.clone()),
                    loop_id: None,
                    source_anchor: step1_anchor,
                    note: format!(
                        "dispatch site for {}; guard via {}",
                        site.event_id, guard_via
                    ),
                };
                // Evidence step 2: subscriber.
                let setter_note = if *classification == SetterClassification::MustSetTrue {
                    "always"
                } else {
                    "may"
                };
                let step2 = EvidenceStep {
                    routine_id: setter.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&r.source_anchor, r),
                    note: format!("subscriber {setter_note} set IsHandled := true"),
                };

                let cross_ext = cross_ext_by_event
                    .get(&site.event_id)
                    .filter(|v| !v.is_empty())
                    .cloned();

                let mut finding = Finding {
                    id: root_cause_key.clone(),
                    root_cause_key: root_cause_key.clone(),
                    detector: DETECTOR.to_string(),
                    title: "Event subscriber sets IsHandled but does not perform the publisher's default write".into(),
                    root_cause: format!(
                        "Caller {} guards table writes on IsHandled; subscriber {} sets it true but doesn't write {}. coverage={}",
                        site.caller_routine, setter, table, coverage_status
                    ),
                    severity: severity.to_string(),
                    confidence: FindingConfidence {
                        level: confidence_level.to_string(),
                        capped_by: None,
                        evidence: Vec::new(),
                    },
                    primary_location: anchor_of(&r.source_anchor, r),
                    evidence_path: vec![step1, step2],
                    additional_paths: None,
                    affected_objects: Vec::new(),
                    affected_tables: vec![(*table).as_str().into()],
                    fix_options: vec![FixOption {
                        description: "Either perform the missing write in the subscriber, or stop setting IsHandled := true.".into(),
                        safety: "high".into(),
                    }],
                    provenance: vec![Evidence {
                        source: "tree-sitter",
                        note: None,
                    }],
                    actionable_anchor: None,
                    fingerprint: None,
                    event_kind: event_kind_by_id.get(site.event_id.as_str()).map(|s| s.to_string()),
                    cross_extension_subscribers: cross_ext,
                    cohort_contexts: None,
                };
                finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
                findings.push(finding);
            }
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates, emitted);
    stats.add_skip("other", skipped_no_guard + skipped_no_setter);
    Ok(DetectorOutput::no_diag(findings, stats))
}
