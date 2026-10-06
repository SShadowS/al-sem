//! D17 — declared MinVersion vs resolved dependency version drift. Port of al-sem
//! `src/detectors/d17-min-version-drift.ts`.
//!
//! For each dep declared in the primary app.json `dependencies[]`
//! (`ctx.declared_dependencies`: `{app_guid, name, min_version}`), compare the declared
//! `min_version` against the resolved `.app` version (`ctx.app_versions`). If
//! `cmpVersion(resolved, min) > 0` AND the primary app actually calls into that dep
//! (a cross-app edge whose callee's appGuid == dep guid), emit ONE info finding.
//!
//! `id = d17/{appGuid}`; severity info; confidence possible; one evidence step (no
//! callsiteId); affectedObjects = [callerRoutine.objectId].

use std::collections::{HashMap, HashSet};

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FixOption, SourceAnchor, id_list,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

use super::anchor_of;

const DETECTOR: &str = "d17-min-version-drift";

/// Parse a version segment like JS `parseInt(x, 10) || 0`.
///
/// al-sem `cmpVersion` uses `Number.parseInt(x, 10) || 0`:
///  - Parses the LEADING decimal digits of the string (e.g. `"1a"` → `1`).
///  - Non-numeric / empty strings produce `NaN`, and `NaN || 0` evaluates to `0`.
///  - `parseInt("0", 10) || 0` evaluates to `0` (0 is falsy → `0`), which is still 0.
///
/// In practice the only case that differs from `.parse::<u64>()` is the leading-digits
/// case (`"1a"` → `1` here vs `0` with `parse().unwrap_or(0)`). Golden-safe: BC version
/// strings are well-formed dotted-quads.
fn parse_version_segment(x: &str) -> u64 {
    // Count leading ASCII decimal digits.
    let digit_end = x.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digit_end == 0 {
        return 0; // no leading digits → NaN → 0
    }
    x[..digit_end].parse::<u64>().unwrap_or(0)
}

/// Compare two BC version strings ("X.Y.Z.W"). Returns -1, 0, 1. Missing components
/// are treated as 0 so "24" < "24.1" < "24.1.0.0". Port of al-sem `cmpVersion`.
///
/// Matches al-sem's EXACT semantics: `parseInt(seg, 10) || 0` — leading-digit parse
/// (e.g. `"1a"` → `1`); non-numeric/empty → `0`; missing segments → `0`.
fn cmp_version(a: &str, b: &str) -> i32 {
    let pa: Vec<u64> = a.split('.').map(parse_version_segment).collect();
    let pb: Vec<u64> = b.split('.').map(parse_version_segment).collect();
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let da = pa.get(i).copied().unwrap_or(0);
        let db = pb.get(i).copied().unwrap_or(0);
        if da != db {
            return if da < db { -1 } else { 1 };
        }
    }
    0
}

/// A captured sample callsite for one drifting dep.
struct Sample {
    caller_routine_id: String,
    callee_routine_name: String,
    anchor: SourceAnchor,
}

pub fn detect_d17(
    _resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();

    let declared = &ctx.declared_dependencies;
    if declared.is_empty() {
        return Ok(DetectorOutput {
            findings,
            stats: DetectorStats::new(DETECTOR, 0, 0),
            diagnostics: vec![],
            d1_cohort_index: None,
        });
    }

    // Walk cross-app edges (primary caller → dep callee) collecting calledDepGuids +
    // the first sample callsite per dep guid.
    //
    // Iteration order: al-sem iterates `graph.edgesByFrom` (JS Map) in MAP INSERTION
    // ORDER = the first-appearance of each `from` key in the `edges` array (call-graph
    // emission order). Rust HashMap is unordered, so we use `edges_from_order` which
    // records that same first-appearance order at assembly time. When ≥2 primary callers
    // reach the same dep the FIRST one in emission order wins — matching al-sem exactly.
    let mut called_dep_guids: HashSet<String> = HashSet::new();
    let mut sample_by_dep: HashMap<String, Sample> = HashMap::new();
    for from in &ctx.graph.edges_from_order {
        let Some(edges) = ctx.graph.edges_by_from.get(from.as_str()) else {
            continue;
        };
        for e in edges {
            let Some(caller) = ctx.routine_by_id.get(e.from.as_str()) else {
                continue;
            };
            let Some(callee) = ctx.routine_by_id.get(e.to.as_str()) else {
                continue;
            };
            if ctx.dep_routine_ids.contains(&e.from) {
                continue;
            }
            let Some(caller_obj) = ctx.objects_by_id.get(caller.object_id.as_str()) else {
                continue;
            };
            let Some(callee_obj) = ctx.objects_by_id.get(callee.object_id.as_str()) else {
                continue;
            };
            if caller_obj.app_guid == callee_obj.app_guid {
                continue;
            }
            called_dep_guids.insert(callee_obj.app_guid.clone());
            if !sample_by_dep.contains_key(&callee_obj.app_guid) {
                let anchor: SourceAnchor = match &e.callsite_id {
                    Some(cid) => match ctx.call_site_by_id.get(cid.as_str()) {
                        Some(cs) => anchor_of(&cs.source_anchor, caller),
                        None => anchor_of(&caller.source_anchor, caller),
                    },
                    None => anchor_of(&caller.source_anchor, caller),
                };
                sample_by_dep.insert(
                    callee_obj.app_guid.clone(),
                    Sample {
                        caller_routine_id: caller.id.clone(),
                        callee_routine_name: callee.name.clone(),
                        anchor,
                    },
                );
            }
        }
    }

    let mut candidates_considered = 0usize;
    let mut skipped_other = 0u64;
    for dep in declared {
        candidates_considered += 1;
        let Some(resolved_version) = ctx.app_versions.get(&dep.app_guid) else {
            skipped_other += 1;
            continue;
        };
        if cmp_version(resolved_version, &dep.min_version) <= 0 {
            skipped_other += 1;
            continue;
        }
        if !called_dep_guids.contains(&dep.app_guid) {
            skipped_other += 1;
            continue;
        }
        let Some(sample) = sample_by_dep.get(&dep.app_guid) else {
            skipped_other += 1;
            continue;
        };
        let Some(caller_routine) = ctx.routine_by_id.get(sample.caller_routine_id.as_str()) else {
            skipped_other += 1;
            continue;
        };

        let id = format!("d17/{}", dep.app_guid);
        let root_cause = format!(
            "{} calls into {} ({}). app.json declares MinVersion {} for this dependency, but the \
             resolved .app is at version {} — your code may use APIs that don't exist on older \
             tenants.",
            caller_routine.name, dep.name, dep.app_guid, dep.min_version, resolved_version
        );

        let evidence_path = vec![EvidenceStep {
            routine_id: sample.caller_routine_id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: sample.anchor.clone(),
            note: format!(
                "calls {} in {} {} (declared MinVersion {})",
                sample.callee_routine_name, dep.name, resolved_version, dep.min_version
            ),
        }];

        let affected_objects = vec![caller_routine.object_id.clone()];

        let mut finding = Finding {
            id: id.clone(),
            root_cause_key: id,
            detector: DETECTOR.to_string(),
            title: "Declared MinVersion is older than the resolved dependency version".into(),
            root_cause,
            severity: "info".to_string(),
            confidence: to_confidence(&[], "possible"),
            primary_location: sample.anchor.clone(),
            evidence_path,
            additional_paths: None,
            affected_objects: id_list(affected_objects),
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description: format!(
                    "Bump app.json dependencies[].version for {} to at least {}, or test that \
                     your code paths into this dep also work on {}.",
                    dep.name, resolved_version, dep.min_version
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

// ---------------------------------------------------------------------------
// Native oracles — #[cfg(test)]
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::{cmp_version, detect_d17};
    use crate::engine::l2::features::PAnchor;
    use crate::engine::l3::event_graph::EventGraph;
    use crate::engine::l3::l3_workspace::{
        L3Object, L3Resolved, L3Routine, L3Workspace, RoutineVariables,
    };
    use crate::engine::l4::combined_graph::{CombinedEdge, CombinedGraph};
    use crate::engine::l5::detector_context::{DeclaredDep, DetectorContext};
    use crate::engine::l5::event_flow::EventFlowIndexes;

    // -----------------------------------------------------------------------
    // Oracle 1 — cmpVersion semantics (Fix 3)
    // -----------------------------------------------------------------------

    #[test]
    fn cmp_version_equal_dotted_quad() {
        assert_eq!(cmp_version("24.1.0.0", "24.1.0.0"), 0, "equal dotted quads");
        assert_eq!(cmp_version("24.1", "24.1"), 0, "equal short");
    }

    #[test]
    fn cmp_version_equal_different_length_missing_segments_are_zero() {
        // "24.1" == "24.1.0.0": missing segments treated as 0
        assert_eq!(cmp_version("24.1", "24.1.0.0"), 0);
        assert_eq!(cmp_version("24.1.0.0", "24.1"), 0);
    }

    #[test]
    fn cmp_version_less() {
        assert_eq!(cmp_version("1.0.0.0", "2.0.0.0"), -1);
        assert_eq!(cmp_version("24.0", "24.1"), -1);
    }

    #[test]
    fn cmp_version_greater() {
        assert_eq!(cmp_version("2.0.0.0", "1.0.0.0"), 1);
        assert_eq!(cmp_version("24.1", "24.0"), 1);
    }

    #[test]
    fn cmp_version_different_length_less() {
        // "24" < "24.1" because 24.0 < 24.1
        assert_eq!(cmp_version("24", "24.1"), -1);
    }

    #[test]
    fn cmp_version_different_length_greater() {
        // "24.1" > "24" because 24.1 > 24.0
        assert_eq!(cmp_version("24.1", "24"), 1);
    }

    #[test]
    fn cmp_version_malformed_segment_leading_digit_parse() {
        // al-sem parseInt("24x", 10) = 24 (leading-digit parse), not 0
        // "24x.1" vs "24.1" should be equal (both parse 24 for the first segment)
        assert_eq!(
            cmp_version("24x.1", "24.1"),
            0,
            "leading-digit parse: '24x' → 24"
        );
        // "25x.0" > "24.0" because 25 > 24
        assert_eq!(
            cmp_version("25x.0", "24.0"),
            1,
            "leading-digit parse: '25x' → 25 > 24"
        );
    }

    #[test]
    fn cmp_version_non_numeric_segment_is_zero() {
        // al-sem parseInt("x", 10) = NaN, NaN || 0 = 0
        assert_eq!(cmp_version("x.1", "0.1"), 0, "non-numeric 'x' → 0");
        assert_eq!(cmp_version("", "0"), 0, "empty string → 0 vs 0");
    }

    #[test]
    fn cmp_version_empty_strings() {
        // Both segments are empty → both 0 → equal
        assert_eq!(cmp_version("", ""), 0);
        // "1" > ""  because 1 > 0
        assert_eq!(cmp_version("1", ""), 1);
    }

    // -----------------------------------------------------------------------
    // Oracle 2 — d17 sample-selection order (Fix 2)
    //
    // Two primary callers (caller_a, caller_b) both call into the same dep routine.
    // Edges emitted in order: caller_a first, then caller_b (edge_list order).
    // The detector must pick caller_a's sample (first emission-order appearance).
    // -----------------------------------------------------------------------

    fn dummy_anchor() -> PAnchor {
        PAnchor {
            source_unit_id: "ws:test.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: "procedure".to_string(),
        }
    }

    fn make_object(id: &str, app_guid: &str, object_number: i64) -> L3Object {
        L3Object {
            id: id.to_string(),
            app_guid: app_guid.to_string(),
            object_type: "Codeunit".to_string(),
            object_number,
            name: format!("Obj{object_number}"),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: Some(Vec::new()),
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
        }
    }

    fn make_routine(id: &str, name: &str, object_id: &str, app_guid: &str) -> L3Routine {
        L3Routine {
            id: id.to_string(),
            stable_routine_id: format!("stable::{id}"),
            object_id: object_id.to_string(),
            object_type: "Codeunit".to_string(),
            name: name.to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: Vec::new(),
            app_guid: app_guid.to_string(),
            object_number: 1,
            normalized_signature_hash: String::new(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            source_anchor: dummy_anchor(),
            identifier_references: Vec::new(),
            unreachable_statements: Vec::new(),
            has_branching: false,
            var_assignments: Vec::new(),
            condition_references: Vec::new(),
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    fn make_edge(from: &str, to: &str, callsite_id: &str) -> CombinedEdge {
        CombinedEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: "direct".to_string(),
            callsite_id: Some(callsite_id.to_string()),
            operation_id: None,
            event_id: None,
            subscriber_app_id: None,
            resolution: "resolved".to_string(),
        }
    }

    fn make_ctx<'a>(
        routines: &'a [L3Routine],
        objects: &'a [L3Object],
        edges: Vec<CombinedEdge>,
        dep_routine_ids: BTreeSet<String>,
        declared: Vec<DeclaredDep>,
        app_versions: HashMap<String, String>,
    ) -> DetectorContext<'a> {
        let routine_by_id: HashMap<&'a str, &'a L3Routine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let objects_by_id: HashMap<&'a str, &'a L3Object> =
            objects.iter().map(|o| (o.id.as_str(), o)).collect();

        // Build edges_by_from + edges_from_order in edge-slice order (mirrors
        // al-sem buildCombinedGraph insertion order).
        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut edges_from_order: Vec<String> = Vec::new();
        for e in &edges {
            if !edges_by_from.contains_key(&e.from) {
                edges_from_order.push(e.from.clone());
            }
            edges_by_from
                .entry(e.from.clone())
                .or_default()
                .push(e.clone());
        }

        let mut nodes: Vec<String> = routines.iter().map(|r| r.id.clone()).collect();
        nodes.sort();

        DetectorContext {
            graph: CombinedGraph {
                nodes,
                edges_by_from,
                edges_from_order,
                uncertainty_edges: Vec::new(),
                typed_edges: Vec::new(),
            },
            event_graph: EventGraph {
                events: Vec::new(),
                edges: Vec::new(),
            },
            routine_by_id,
            objects_by_id,
            table_by_id: HashMap::new(),
            reverse_call_graph: std::collections::BTreeMap::new(),
            entry_points: BTreeSet::new(),
            transaction_spans: Vec::new(),
            resolved_call_edge_by_callsite: HashMap::new(),
            uncertainty_edges_by_from: HashMap::new(),
            uncertainties_by_node: HashMap::new(),
            uncertainties: Default::default(),
            call_site_by_id: HashMap::new(),
            summaries: HashMap::new(),
            // ⟨C1⟩ No summaries ⇒ no derived cone rows.
            cone_derived: Default::default(),
            event_flow_indexes: EventFlowIndexes::default(),
            parameter_roles_by_routine: HashMap::new(),
            upgraded_bindings_by_callsite: HashMap::new(),
            reachable_roots: BTreeSet::new(),
            internal_reachable_externally: false,
            dep_routine_ids,
            declared_dependencies: declared,
            app_versions,
            root_classifications_by_routine: HashMap::new(),
            ordering_facts: std::sync::OnceLock::new(),
            ordering_source: None,
            closed_world_temp_params: Default::default(),
            summarize_diagnostics: Vec::new(),
            db_effect_bundle: None,
            reverse_effect_index: None,
            fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex::build(
                routines, objects,
            ),
            cross_extension_subscribers: std::collections::BTreeMap::new(),
        }
    }

    fn empty_resolved_for(routines: &[L3Routine], objects: &[L3Object]) -> L3Resolved {
        L3Resolved {
            workspace: L3Workspace {
                routines: routines.to_vec(),
                objects: objects.to_vec(),
                tables: Vec::new(),
            },
            root_classifications: Vec::new(),
            primary_app: None,
            infra_diagnostics: Vec::new(),
            precomputed_calls: None,
            precomputed_events: None,
        }
    }

    /// Oracle: two primary callers (caller_a emitted first, caller_b second) both call
    /// into the same dep. The first sample MUST be from caller_a (emission-order
    /// insertion order, matching al-sem JS Map semantics).
    #[test]
    fn d17_sample_order_first_emission_order_caller_wins() {
        let ws_guid = "aaaaaaaa-0000-0000-0000-000000000001";
        let dep_guid = "dddddddd-0000-0000-0000-000000000001";

        let obj_ws = make_object("ws/Codeunit/1", ws_guid, 1);
        let obj_dep = make_object("dep/Codeunit/99", dep_guid, 99);
        let objects = [obj_ws.clone(), obj_dep.clone()];

        // caller_a: emitted first → should be the sample caller
        let mut r_caller_a = make_routine("r_caller_a", "CallerA", "ws/Codeunit/1", ws_guid);
        r_caller_a.object_number = 1;
        // caller_b: emitted second → should NOT be the sample caller
        let mut r_caller_b = make_routine("r_caller_b", "CallerB", "ws/Codeunit/1", ws_guid);
        r_caller_b.object_number = 1;
        let r_dep = make_routine("r_dep", "DepProc", "dep/Codeunit/99", dep_guid);

        let routines = [r_caller_a.clone(), r_caller_b.clone(), r_dep.clone()];

        // edges_from_order mirrors al-sem: caller_a first, then caller_b
        let edges = vec![
            make_edge("r_caller_a", "r_dep", "cs_a"),
            make_edge("r_caller_b", "r_dep", "cs_b"),
        ];

        let dep_routine_ids: BTreeSet<String> = ["r_dep".to_string()].into_iter().collect();
        let declared = vec![DeclaredDep {
            app_guid: dep_guid.to_string(),
            name: "DepLib".to_string(),
            min_version: "1.0.0.0".to_string(),
        }];
        let mut app_versions = HashMap::new();
        app_versions.insert(dep_guid.to_string(), "2.0.0.0".to_string());

        let ctx = make_ctx(
            &routines,
            &objects,
            edges,
            dep_routine_ids,
            declared,
            app_versions,
        );
        let resolved = empty_resolved_for(&routines, &objects);

        let output = detect_d17(&resolved, &ctx).unwrap();
        assert_eq!(output.findings.len(), 1, "should emit 1 finding");

        let finding = &output.findings[0];
        // The rootCause should name CallerA (first emission-order caller), not CallerB.
        assert!(
            finding.root_cause.contains("CallerA"),
            "sample caller must be CallerA (first emission order): got {}",
            finding.root_cause
        );
        assert!(
            !finding.root_cause.contains("CallerB"),
            "CallerB must NOT be the sample (wrong emission order): got {}",
            finding.root_cause
        );
    }

    /// Oracle: reverse emission order — caller_b emitted first, caller_a second.
    /// The sample MUST be from caller_b.
    #[test]
    fn d17_sample_order_reversed_emission_caller_b_wins() {
        let ws_guid = "aaaaaaaa-0000-0000-0000-000000000002";
        let dep_guid = "dddddddd-0000-0000-0000-000000000002";

        let obj_ws = make_object("ws2/Codeunit/1", ws_guid, 1);
        let obj_dep = make_object("dep2/Codeunit/99", dep_guid, 99);
        let objects = [obj_ws.clone(), obj_dep.clone()];

        let mut r_caller_a = make_routine("r2_caller_a", "CallerA2", "ws2/Codeunit/1", ws_guid);
        r_caller_a.object_number = 1;
        let mut r_caller_b = make_routine("r2_caller_b", "CallerB2", "ws2/Codeunit/1", ws_guid);
        r_caller_b.object_number = 1;
        let r_dep = make_routine("r2_dep", "DepProc2", "dep2/Codeunit/99", dep_guid);

        let routines = [r_caller_a.clone(), r_caller_b.clone(), r_dep.clone()];

        // Reversed: caller_b emitted first
        let edges = vec![
            make_edge("r2_caller_b", "r2_dep", "cs_b2"),
            make_edge("r2_caller_a", "r2_dep", "cs_a2"),
        ];

        let dep_routine_ids: BTreeSet<String> = ["r2_dep".to_string()].into_iter().collect();
        let declared = vec![DeclaredDep {
            app_guid: dep_guid.to_string(),
            name: "DepLib2".to_string(),
            min_version: "1.0.0.0".to_string(),
        }];
        let mut app_versions = HashMap::new();
        app_versions.insert(dep_guid.to_string(), "2.0.0.0".to_string());

        let ctx = make_ctx(
            &routines,
            &objects,
            edges,
            dep_routine_ids,
            declared,
            app_versions,
        );
        let resolved = empty_resolved_for(&routines, &objects);

        let output = detect_d17(&resolved, &ctx).unwrap();
        assert_eq!(output.findings.len(), 1, "should emit 1 finding");

        let finding = &output.findings[0];
        assert!(
            finding.root_cause.contains("CallerB2"),
            "sample caller must be CallerB2 (first emission order): got {}",
            finding.root_cause
        );
        assert!(
            !finding.root_cause.contains("CallerA2"),
            "CallerA2 must NOT be the sample: got {}",
            finding.root_cause
        );
    }
}
