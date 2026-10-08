//! D13 — primary-app code calls a routine in a DIFFERENT app declared `[InternalProc]`
//! or with `internal` visibility. Port of al-sem
//! `src/detectors/d13-cross-app-internal-call.ts`.
//!
//! Walk every resolved combined-graph edge; flag edges where (a) the caller's role is
//! `"primary"` (dep_routine_ids miss), (b) caller and callee live in DIFFERENT apps
//! (different appGuid — the CROSS-APP gate), and (c) the callee carries `[InternalProc]`
//! or its `access_modifier == "internal"`.
//!
//! Within-detector dedup by `id` (`d13/{from}/{callsiteId}`) then sort by
//! `compareStrings(id)`. Fingerprint computed pre-projection over internal ids.

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::parse_routine_attributes;
use crate::program::model::workspace::Model;

use super::anchor_of;

const DETECTOR: &str = "d13-cross-app-internal-call";

pub fn detect_d13(
    _resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
    let mut skipped_friend = 0u64;

    // Deterministic walk: collect edges into a sorted order so candidate counting +
    // dedup are stable; final sort by id makes output order independent regardless.
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

            let Some(caller_obj) = ctx.objects_by_id.get(caller.object_id.as_str()) else {
                continue;
            };
            let Some(callee_obj) = ctx.objects_by_id.get(callee.object_id.as_str()) else {
                continue;
            };
            // Only flag cross-app boundaries.
            if caller_obj.app_guid == callee_obj.app_guid {
                skipped_other += 1;
                continue;
            }

            let attrs = parse_routine_attributes(&callee.attributes_parsed);
            if !attrs.internal_proc && callee.access_modifier.as_deref() != Some("internal") {
                skipped_other += 1;
                continue;
            }
            // A caller the callee's app names in `<InternalsVisibleTo>` is a friend:
            // the dependency's owner opened its internals to it on purpose, and a
            // non-friend's call into an `internal` procedure does not compile at
            // all (engine-switch S8.5 triage: 20 of 20 cross-app findings on
            // CDO/DO were friend calls into Continia Delivery Network).
            if ctx
                .friends
                .get(&callee_obj.app_guid.to_ascii_lowercase())
                .is_some_and(|f| f.contains(&caller_obj.app_guid.to_ascii_lowercase()))
            {
                skipped_friend += 1;
                continue;
            }

            let callsite_token = e.callsite_id.as_deref().unwrap_or("x");
            let id = format!("d13/{}/{}", e.from, callsite_token);
            if seen.contains(&id) {
                continue;
            }
            seen.insert(id.clone());

            // anchor = cs?.sourceAnchor ?? caller.sourceAnchor (enclosing = caller).
            let anchor: SourceAnchor = match &e.callsite_id {
                Some(cid) => match ctx.call_site_by_id.get(cid.as_str()) {
                    Some(cs) => anchor_of(&cs.source_anchor, caller),
                    None => anchor_of(&caller.source_anchor, caller),
                },
                None => anchor_of(&caller.source_anchor, caller),
            };

            let mut affected_objects = vec![caller_obj.id.clone(), callee_obj.id.clone()];
            affected_objects.sort();

            let evidence_path = vec![EvidenceStep {
                routine_id: caller.id.clone(),
                operation_id: None,
                callsite_id: e.callsite_id.clone(),
                loop_id: None,
                source_anchor: anchor.clone(),
                note: format!(
                    "calls Internal {} in app {}",
                    callee.name, callee_obj.app_guid
                ),
            }];

            let root_cause = format!(
                "{} calls {} (app {}), which is declared Internal. Crossing this boundary breaks \
                 encapsulation and can stop compiling on any minor version bump of the dependency.",
                caller.name, callee.name, callee_obj.app_guid
            );

            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: "Cross-extension call into an internal procedure".into(),
                root_cause,
                severity: "high".to_string(),
                confidence: to_confidence(&[], "confirmed"),
                primary_location: anchor,
                evidence_path,
                additional_paths: None,
                affected_objects: id_list(affected_objects),
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: "Use the dependency's public API instead, or request the routine \
                                  be promoted to Public upstream."
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

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("other", skipped_other);
    stats.add_skip("friend", skipped_friend);
    Ok(DetectorOutput {
        findings,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
