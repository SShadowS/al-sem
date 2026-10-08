//! D42 — cross-call wrong SetLoadFields. Port of al-sem
//! `src/detectors/d42-cross-call-wrong-setloadfields.ts`.
//!
//! At each resolved call edge forwarding a record to a callee, when the caller-side
//! narrowed load LF (the source-ordered cumulative SetLoadFields/AddLoadFields on the
//! forwarded variable at the callsite, or the caller-parameter
//! `currentLoadedFieldsAtExit` fallback) is a concrete list, the callee's
//! `requiredLoadedFieldsAtEntry` RF is a non-empty concrete list, and RF \ LF is
//! non-empty, the runtime issues an extra round-trip — silently defeating the
//! partial-load optimisation.
//!
//! Severity low. Confidence likely. Anchor: caller's argumentAnchor.
//!
//! Reads `ctx.parameter_roles_by_routine` (both the callee role and, for the
//! parameter-source fallback, the caller's own role) + `ctx.resolved_call_edge_by_callsite`.
//! The post-upgrade `bindingResolution` lives on `ctx.upgraded_bindings_by_callsite`,
//! joined POSITIONALLY with `cs.argument_bindings` by index.

use std::collections::BTreeSet;

use crate::engine::l4::summary::FieldList;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, before_anchor, flow_field_names_lc, normalize_load_field_arg,
    primary_key_field_names_lc,
};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PAnchor;
use crate::program::model::workspace::{Model, ModelRecordOperation};

const DETECTOR: &str = "d42-cross-call-wrong-setloadfields";

/// Caller-side narrowed-load shape AT a callsite.
enum Narrow {
    /// No prior SetLoadFields/AddLoadFields op — carries the full record.
    Full,
    /// The cumulative narrow (sorted, deduped field-name strings).
    Known(Vec<String>),
    /// A `Reset` between the last narrow and the anchor wiped the pending narrow.
    Unknown,
}

/// `computeNarrowAtCallsite` — source-ordered, intra-routine cumulative narrow for a
/// record variable at a callsite anchor. Port of the al-sem helper.
fn compute_narrow_at_callsite(
    ops: &[ModelRecordOperation],
    var_name_lc: &str,
    callsite_anchor: &PAnchor,
) -> Narrow {
    // pending: "none" | "unknown" | Vec<field>
    enum Pending {
        None,
        Unknown,
        Known(Vec<String>),
    }
    let mut pending = Pending::None;
    for op in ops {
        if op.record_variable_name.to_lowercase() != var_name_lc {
            continue;
        }
        if !before_anchor(&op.source_anchor, callsite_anchor) {
            continue;
        }
        match op.op.as_str() {
            "SetLoadFields" => {
                let set: BTreeSet<String> = op
                    .field_arguments
                    .as_ref()
                    .map(|fa| fa.iter().cloned().collect())
                    .unwrap_or_default();
                pending = Pending::Known(set.into_iter().collect());
            }
            "AddLoadFields" => {
                let additions = op.field_arguments.clone().unwrap_or_default();
                pending = match pending {
                    Pending::None => {
                        let set: BTreeSet<String> = additions.into_iter().collect();
                        Pending::Known(set.into_iter().collect())
                    }
                    Pending::Unknown => Pending::Unknown,
                    Pending::Known(existing) => {
                        let mut set: BTreeSet<String> = existing.into_iter().collect();
                        set.extend(additions);
                        Pending::Known(set.into_iter().collect())
                    }
                };
            }
            "Reset" => {
                pending = Pending::Unknown;
            }
            _ => {}
        }
    }
    match pending {
        Pending::None => Narrow::Full,
        Pending::Unknown => Narrow::Unknown,
        Pending::Known(fields) => Narrow::Known(fields),
    }
}

pub fn detect_d42(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_caller_full = 0u64;
    let mut skipped_callee_requires_none = 0u64;
    let mut skipped_callee_unknown = 0u64;
    let mut skipped_pk_only = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only ⇒ all primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        let empty_roles = Vec::new();
        let own_role = ctx
            .parameter_roles_by_routine
            .get(&routine.id)
            .unwrap_or(&empty_roles);
        let ops = &routine.record_operations;

        for cs in &routine.call_sites {
            let edge = match ctx.resolved_call_edge_by_callsite.get(&cs.id) {
                Some(e) => e,
                None => continue,
            };
            let to = match &edge.to {
                Some(t) => t,
                None => continue,
            };
            let callee = match ctx.routine_by_id.get(to.as_str()) {
                Some(c) => *c,
                None => continue,
            };

            let upgraded = ctx.upgraded_bindings_by_callsite.get(&cs.id);
            for (i, binding) in cs.argument_bindings.iter().enumerate() {
                let binding_resolution = upgraded
                    .and_then(|u| u.get(i))
                    .map(|u| u.binding_resolution.as_str());
                if binding_resolution != Some("resolved") {
                    continue;
                }
                let callee_role =
                    match ctx
                        .parameter_roles_by_routine
                        .get(&callee.id)
                        .and_then(|roles| {
                            roles
                                .iter()
                                .find(|r| r.parameter_index == binding.parameter_index)
                        }) {
                        Some(r) => r,
                        None => {
                            skipped_callee_unknown += 1;
                            continue;
                        }
                    };
                // RF = callee.requiredLoadedFieldsAtEntry — non-empty concrete list.
                // al-sem: `RF === "unknown" || RF.length === 0` → skip. Full never
                // occurs for this field; treat conservatively as the skip path.
                let rf: Vec<String> = match &callee_role.required_loaded_fields_at_entry {
                    FieldList::Known(names) if !names.is_empty() => names.clone(),
                    _ => {
                        skipped_callee_requires_none += 1;
                        continue;
                    }
                };
                // G-15(c): PRIMARY-KEY fields are always loaded regardless of
                // SetLoadFields — same exclusion G-12 applied to d3. Drop the
                // PK (first key) fields of the callee parameter's table from
                // the required set; when nothing else is required, the
                // forwarded narrow causes no extra round-trip. An unresolved
                // table / missing key excludes nothing (keep firing).
                // Drop PK fields (always loaded, G-15c) AND FlowField/FlowFilter
                // fields (class E Gap-Y: not physical columns — SetLoadFields
                // ignores them, so a FlowField the callee reads never forces a
                // wider narrow / extra round-trip). An unresolved table excludes
                // nothing (keep firing).
                let excluded_fields = ctx
                    .table_by_id
                    .get(callee_role.table_id.as_str())
                    .map(|t| {
                        let mut s = primary_key_field_names_lc(t);
                        s.extend(flow_field_names_lc(t));
                        s
                    })
                    .unwrap_or_default();
                let rf: Vec<String> = rf
                    .into_iter()
                    .filter(|f| !excluded_fields.contains(&normalize_load_field_arg(f)))
                    .collect();
                if rf.is_empty() {
                    skipped_pk_only += 1;
                    continue;
                }

                // Caller-side LF — two-tier resolution.
                let mut lf: Option<Narrow> = None;
                if let Some(source_name_lc) = &binding.source_variable_name {
                    lf = Some(compute_narrow_at_callsite(
                        ops,
                        source_name_lc,
                        &cs.source_anchor,
                    ));
                }
                // Fall back to caller-parameter currentLoadedFieldsAtExit when the
                // local scan yielded "unknown" and the source IS the routine's param.
                let is_unknown = matches!(lf, None | Some(Narrow::Unknown));
                if is_unknown && let Some(sp_idx) = binding.source_parameter_index {
                    let caller_role = own_role.iter().find(|r| r.parameter_index == sp_idx);
                    lf = Some(
                        match caller_role.map(|r| &r.current_loaded_fields_at_exit) {
                            Some(FieldList::Known(names)) => Narrow::Known(names.clone()),
                            Some(FieldList::Full) => Narrow::Full,
                            _ => Narrow::Unknown,
                        },
                    );
                }

                let loaded = match lf {
                    Some(Narrow::Unknown) | None => continue,
                    Some(Narrow::Full) => {
                        skipped_caller_full += 1;
                        continue;
                    }
                    Some(Narrow::Known(fields)) => fields,
                };
                let missing: Vec<String> = rf
                    .iter()
                    .filter(|f| !loaded.contains(*f))
                    .cloned()
                    .collect();
                if missing.is_empty() {
                    continue;
                }
                candidates_considered += 1;

                let src_or_record = binding.source_variable_name.as_deref();
                let path = vec![
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: None,
                        callsite_id: Some(cs.id.clone()),
                        loop_id: None,
                        source_anchor: anchor_of(&binding.argument_anchor, routine),
                        note: format!(
                            "forwards {} (narrowed to {}) to {}",
                            src_or_record.unwrap_or("record"),
                            loaded.join(", "),
                            callee.name
                        ),
                    },
                    EvidenceStep {
                        routine_id: callee.id.clone(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&callee.source_anchor, callee),
                        note: format!(
                            "{} requires {} loaded; the runtime will issue an extra SQL round-trip",
                            callee.name,
                            missing.join(", ")
                        ),
                    },
                ];

                let id = format!("d42/{}/{}/{}", routine.id, cs.id, binding.parameter_index);
                let mut affected_objects =
                    vec![routine.object_id.clone(), callee.object_id.clone()];
                affected_objects.sort();

                let mut finding = Finding {
                    id: id.clone(),
                    root_cause_key: id,
                    detector: DETECTOR.to_string(),
                    title: "Forwarded record's narrowed load misses a field the callee reads"
                        .to_string().into(),
                    root_cause: format!(
                        "{} narrowed {}'s load to {} but forwards it to {}, which reads {} — defeats the partial-load optimisation.",
                        routine.name,
                        src_or_record.unwrap_or("the record"),
                        loaded.join(", "),
                        callee.name,
                        missing.join(", ")
                    ),
                    severity: "low".to_string(),
                    confidence: to_confidence(&[], "likely"),
                    primary_location: anchor_of(&binding.argument_anchor, routine),
                    evidence_path: path,
                    additional_paths: None,
                    affected_objects: id_list(affected_objects),
                    affected_tables: Vec::new(),
                    fix_options: vec![FixOption {
                        description: format!(
                            "Add {} to the SetLoadFields/AddLoadFields call on {} before forwarding to {}.",
                            missing.join(", "),
                            src_or_record.unwrap_or("the record"),
                            callee.name
                        ).into(),
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
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("callerFull", skipped_caller_full);
    stats.add_skip("calleeRequiresNone", skipped_callee_requires_none);
    stats.add_skip("calleeUnknown", skipped_callee_unknown);
    stats.add_skip("pkOnly", skipped_pk_only);
    Ok(DetectorOutput::no_diag(findings, stats))
}
