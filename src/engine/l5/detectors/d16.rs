//! D16 — primary-app code calls a routine carrying `[Obsolete(...)]`. Port of al-sem
//! `src/detectors/d16-obsolete-routine-call.ts`.
//!
//! Walk every resolved combined-graph edge; flag edges where (a) the caller's role is
//! `"primary"` (dep_routine_ids miss), and (b) the callee carries an `[Obsolete(...)]`
//! attribute. severity Removed→high / Pending→info. NO appGuid gate — fires on any edge
//! to an obsolete callee.
//!
//! `id = d16/{from}/{callsiteId}/{to}` — `from` + `to` are INTERNAL RoutineIds (the
//! projection rewrites them to stable). Within-detector sort by `compareStrings(id)`.

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::{ObsoleteState, parse_routine_attributes};

use super::anchor_of;

const DETECTOR: &str = "d16-obsolete-routine-call";

pub fn detect_d16(
    _resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    // Unified build: maps EVERY routine's internal id (source AND dep) → stable id,
    // so any routine id embedded in the rootCauseKey is replaced with its stable id
    // before hashing (mirrors al-sem's stabilizing substitution). The prior dep-only
    // build is subsumed — dep routines are already in the all-routines map.
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;

    let mut froms: Vec<&String> = ctx.graph.edges_by_from.keys().collect();
    froms.sort();
    for from in froms {
        let edges = &ctx.graph.edges_by_from[from];
        for e in edges {
            let Some(caller) = ctx.routine_by_id.get(e.from.as_str()) else {
                continue;
            };
            let Some(callee) = ctx.routine_by_id.get(e.to.as_str()) else {
                continue;
            };
            // roleOf(caller) === "primary": dep routines are "dependency".
            if ctx.dep_routine_ids.contains(&e.from) {
                continue;
            }
            candidates_considered += 1;

            let attrs = parse_routine_attributes(&callee.attributes_parsed);
            let Some(obsolete_state) = attrs.obsolete_state else {
                skipped_other += 1;
                continue;
            };

            let anchor: SourceAnchor = match &e.callsite_id {
                Some(cid) => match ctx.call_site_by_id.get(cid.as_str()) {
                    Some(cs) => anchor_of(&cs.source_anchor, caller),
                    None => anchor_of(&caller.source_anchor, caller),
                },
                None => anchor_of(&caller.source_anchor, caller),
            };

            let (state_label, severity): (&str, &str) = match obsolete_state {
                ObsoleteState::Removed => ("Removed", "high"),
                ObsoleteState::Pending => ("Pending", "info"),
            };

            let callsite_token = e.callsite_id.as_deref().unwrap_or("x");
            let id = format!("d16/{}/{}/{}", e.from, callsite_token, e.to);

            let root_cause = match &attrs.obsolete_reason {
                Some(reason) => format!("{} calls {} — {}.", caller.name, callee.name, reason),
                None => format!("{} calls {}.", caller.name, callee.name),
            };

            let evidence_path = vec![EvidenceStep {
                routine_id: caller.id.clone(),
                operation_id: None,
                callsite_id: e.callsite_id.clone(),
                loop_id: None,
                source_anchor: anchor.clone(),
                note: format!("calls {} {}", state_label, callee.name),
            }];

            let mut affected_objects = vec![caller.object_id.clone(), callee.object_id.clone()];
            affected_objects.sort();

            let fix_description = attrs.obsolete_reason.clone().unwrap_or_else(|| {
                "Replace the call with the documented successor API.".to_string()
            });

            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: format!("Call to obsolete routine ({})", state_label).into(),
                root_cause,
                severity: severity.to_string(),
                confidence: to_confidence(&[], "confirmed"),
                primary_location: anchor,
                evidence_path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: fix_description.into(),
                    safety: "high".into(),
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
