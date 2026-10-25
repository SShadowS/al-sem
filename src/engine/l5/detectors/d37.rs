//! D37 — `Validate(...)` on a record variable with no subsequent `Modify` /
//! `ModifyAll` / `Insert` to persist the change. Port of al-sem
//! `src/detectors/d37-validate-without-persist.ts`.
//!
//! Detection (intra-routine, source-ordered): for each `Validate` op V on record-var
//! R, walk subsequent ops on R; if a Modify/Insert appears before any
//! state-overwriting op (Init/Reset/Get/Find*/Next/Copy/TransferFields) it is
//! persisted — skip; otherwise the Validate is unpersisted — flag.
//!
//! Detector-audit d37 FN-1: `ModifyAll` is NOT a persist of R's CURRENT record.
//! `R.ModifyAll(Field, Value)` issues a set-based UPDATE across R's filtered set
//! with a literal value — it never writes R's in-memory `Validate`'d fields, so a
//! `Validate` followed only by `ModifyAll` silently discards the change. It is
//! therefore excluded from the persist set (only a genuine `Modify`/`Insert` on R
//! persists the Validate).
//!
//! Suppressions: temporary records, by-var parameter records, and calls forwarding R
//! to a helper whose summary MAY persist (persistsCurrentRecord = "yes") or is
//! opaque/unresolved (conservative).
//!
//! Reads the CORE `RoutineSummary.parameterRoles` via
//! `ctx.parameter_roles_by_routine`, and the post-upgrade per-callsite bindings via
//! `ctx.upgraded_bindings_by_callsite` joined positionally with
//! `call_site.argument_bindings`.

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{
    anchor_of, before_anchor, is_platform_loaded_trigger_rec, is_table_receiver_rec,
    receiver_reset_after, table_rec_persisted_by_owner,
};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FixOption, id_list};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PAnchor;
use crate::program::model::calls::UpgradedBinding;
use crate::program::model::workspace::{Model, ModelRecordOperation, ModelRoutine};

pub(crate) const DETECTOR: &str = "d37-validate-without-persist";

// ModifyAll is intentionally EXCLUDED — it does not persist R's current record
// (see the module doc, d37 FN-1). Only Modify / Insert write R's Validate'd state.
const PERSIST_OPS: &[&str] = &["Modify", "Insert"];
const RESET_LIKE_OPS: &[&str] = &[
    "Init",
    "Reset",
    "Get",
    "FindFirst",
    "FindLast",
    "FindSet",
    "Find",
    "Next",
    "Copy",
    "TransferFields",
];

pub fn detect_d37(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_persisted = 0u64;
    let mut skipped_helper_may_persist = 0u64;
    let mut skipped_helper_persists_unknown = 0u64;
    let mut skipped_temp_record = 0u64;
    let mut skipped_parameter = 0u64;
    let mut skipped_trigger_rec = 0u64;
    let mut skipped_caller_owned_rec = 0u64;
    // A collided id's call sites, each with its LAST OWNING row (#160 step 11b
    // §4.6): `site_pick` reads that row's callee and bindings.
    let site_owners = crate::program::model::projection::site_owners(
        &crate::program::model::projection::RowsOfId::build(&ws.routines),
        &ws.routines,
    );
    for (ri, routine) in ws.routines.iter().enumerate() {
        // roleOf(routine) !== "primary" → skip. Source-only ⇒ all primary.
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }

        let param_record_names: std::collections::HashSet<String> = routine
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .map(|rv| rv.name.to_lowercase())
            .collect();

        for op in &routine.record_operations {
            if op.op != "Validate" {
                continue;
            }
            candidates_considered += 1;
            let var_key = op.record_variable_name.to_lowercase();
            // op.tempState.kind === "known" && op.tempState.value === true
            if super::is_known_temp(op) {
                skipped_temp_record += 1;
                continue;
            }
            if param_record_names.contains(&var_key) {
                skipped_parameter += 1;
                continue;
            }
            // G-9: `Validate` on the implicit `Rec` inside a page/table field
            // trigger is normal field-chain validation — persisting is the
            // (platform) caller's job, not the trigger's.
            if is_platform_loaded_trigger_rec(routine, &op.record_variable_name) {
                skipped_trigger_rec += 1;
                continue;
            }

            // Walk subsequent ops in source order, persistence vs reset.
            if later_persisted(&routine.record_operations, &var_key, op) == "persisted" {
                skipped_persisted += 1;
                continue;
            }

            // Phase 3: precise verdict using callee.persistsCurrentRecord summary.
            let source_variable_name_lc = op.record_variable_name.to_lowercase();
            let source_record_variable_id = op.record_variable_id.as_deref();
            let helper_verdict = post_validate_helper_verdict(
                routine,
                &routine
                    .call_sites
                    .iter()
                    .map(|cs| {
                        let own = site_owners
                            .get(cs.id.as_str())
                            .is_none_or(|o| o.get() == ri);
                        (cs, own)
                    })
                    .collect::<Vec<_>>(),
                source_record_variable_id,
                &source_variable_name_lc,
                &op.source_anchor,
                ctx,
            );
            if helper_verdict == "suppress-may-persist" {
                skipped_helper_may_persist += 1;
                continue;
            }
            if helper_verdict == "suppress-unknown" {
                skipped_helper_persists_unknown += 1;
                continue;
            }
            // "do-not-suppress". #299: a `Validate` on the receiver `Rec` of a
            // Table / TableExtension routine is persisted by its owner (d39's Table
            // owner rule), unless this routine puts `Rec` in a new state after it.
            // Checked last, so the counter holds only would-be findings.
            if is_table_receiver_rec(routine, &op.record_variable_name)
                && table_rec_persisted_by_owner(routine)
                && !receiver_reset_after(routine, ctx, &op.source_anchor, &op.id)
            {
                skipped_caller_owned_rec += 1;
                continue;
            }

            emit(routine, op, &mut findings, fp_index);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("persisted", skipped_persisted);
    stats.add_skip("helperMayPersist", skipped_helper_may_persist);
    stats.add_skip("helperPersistsUnknown", skipped_helper_persists_unknown);
    stats.add_skip("tempRecord", skipped_temp_record);
    stats.add_skip("parameter", skipped_parameter);
    stats.add_skip("triggerRec", skipped_trigger_rec);
    stats.add_skip("callerOwnedRec", skipped_caller_owned_rec);
    Ok(DetectorOutput::no_diag(findings, stats))
}

/// Iterate ops in source order on the same record var, after the Validate; the first
/// persist OR reset wins. Default "unpersisted".
fn later_persisted(
    ops: &[ModelRecordOperation],
    var_key: &str,
    validate_op: &ModelRecordOperation,
) -> &'static str {
    let mut sorted: Vec<&ModelRecordOperation> = ops
        .iter()
        .filter(|o| o.record_variable_name.to_lowercase() == var_key)
        .filter(|o| before_anchor(&validate_op.source_anchor, &o.source_anchor))
        .collect();
    sorted.sort_by(|a, b| {
        if before_anchor(&a.source_anchor, &b.source_anchor) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    for o in sorted {
        if PERSIST_OPS.contains(&o.op.as_str()) {
            return "persisted";
        }
        if RESET_LIKE_OPS.contains(&o.op.as_str()) {
            return "unpersisted";
        }
    }
    "unpersisted"
}

/// After the Validate op, walk callsites in source order. If any post-Validate helper
/// might persist (yes or unknown) the Validate is suppressed; only when EVERY
/// forwarding helper provably doesn't persist do we fall through to emit.
/// `sites` are the row's OWN call sites: a collided id's other arm sits at
/// other source positions, so "after this `Validate`" means nothing across
/// arms (#160 step 11b, final panel FP-14). Each site's callee and bindings
/// come from `site_pick`, i.e. from the site's LAST OWNING row; the flag says
/// whether that is this row. A site another row owns last is read as
/// `suppress-unknown` when it takes our record (final panel NEW-1): its callee
/// and bindings are that row's, and R5 keeps no bindings of this row.
fn post_validate_helper_verdict(
    routine: &ModelRoutine,
    sites: &[(&crate::program::body::features::PCallSite, bool)],
    source_record_variable_id: Option<&str>,
    source_variable_name_lc: &str,
    validate_anchor: &PAnchor,
    ctx: &DetectorContext,
) -> &'static str {
    for &(cs, own) in sites {
        if !before_anchor(validate_anchor, &cs.source_anchor) {
            continue;
        }
        // The site's resolved callee and its upgraded bindings, both from one
        // row, through `site_pick` (#160 step 11b §4.6).
        let pick = crate::program::model::projection::site_pick(
            &ctx.graph,
            &routine.id,
            &cs.id,
            &ctx.upgraded_bindings_by_callsite,
        );
        let upgraded = pick.bindings;
        for (i, binding) in cs.argument_bindings.iter().enumerate() {
            let up: Option<&UpgradedBinding> = upgraded.and_then(|u| u.get(i));
            let binding_resolution = up.map(|u| u.binding_resolution.as_str());
            let callee_parameter_is_var = up.map(|u| u.callee_parameter_is_var).unwrap_or(false);

            // Match by stable id when both sides have one; fall back to name.
            let matches_by_id = binding.source_record_variable_id.is_some()
                && source_record_variable_id.is_some()
                && binding.source_record_variable_id.as_deref() == source_record_variable_id;
            let matches_by_name =
                binding.source_variable_name.as_deref() == Some(source_variable_name_lc);
            if !matches_by_id && !matches_by_name {
                continue;
            }
            // Our record goes to a site whose callee and bindings are another
            // row's: nothing of this row says what the callee does with it.
            if !own {
                return "suppress-unknown";
            }
            // Literals / non-record expressions can't forward a record — skip.
            if binding_resolution == Some("non-record-arg") {
                continue;
            }
            // Unresolved-callee / ambiguous: preserve the old conservatism for
            // unresolved bindings that DO target our record.
            if binding_resolution != Some("resolved") {
                return "suppress-unknown";
            }
            // By-value callees can't persist the caller's record.
            if !callee_parameter_is_var {
                continue;
            }
            let edge_to = match pick.callee {
                Some(t) => t,
                None => return "suppress-unknown",
            };
            let callee = ctx.routine_by_id.get(edge_to).copied();
            let callee_role = callee.and_then(|c| {
                ctx.parameter_roles_by_routine.get(&c.id).and_then(|roles| {
                    roles
                        .iter()
                        .find(|r| r.parameter_index == binding.parameter_index)
                })
            });
            let callee_role = match callee_role {
                Some(r) => r,
                None => return "suppress-unknown",
            };
            // callee?.bodyAvailable === false → suppress-unknown
            if callee.map(|c| !c.body_available).unwrap_or(true) {
                return "suppress-unknown";
            }
            match callee_role.persists_current_record {
                crate::engine::l4::effect_lattice::EffectPresence::Yes => {
                    return "suppress-may-persist";
                }
                crate::engine::l4::effect_lattice::EffectPresence::Unknown => {
                    return "suppress-unknown";
                }
                // "no" — helper provably doesn't persist; keep walking.
                crate::engine::l4::effect_lattice::EffectPresence::No => {}
            }
        }
    }
    "do-not-suppress"
}

fn emit(
    routine: &ModelRoutine,
    op: &ModelRecordOperation,
    findings: &mut Vec<Finding>,
    fp_index: &FingerprintIndex,
) {
    let path = vec![EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: Some(op.id.clone()),
        callsite_id: None,
        loop_id: None,
        source_anchor: anchor_of(&op.source_anchor, routine),
        note: format!(
            "Validate on {} with no later Modify/Insert before the record is reloaded or the routine exits",
            op.record_variable_name
        ),
    }];

    let id = format!("d37/{}/{}", routine.id, op.id);
    let affected_tables: Vec<String> = match &op.table_id {
        Some(t) => vec![t.clone()],
        None => Vec::new(),
    };

    let mut finding = Finding {
        id: id.clone(),
        root_cause_key: id,
        detector: DETECTOR.to_string(),
        title: "Validate changes are not persisted".into(),
        root_cause: format!(
            "{} calls Validate on {} but never persists the change with Modify / Insert before the record is reloaded or the routine returns — the field write is discarded.",
            routine.name, op.record_variable_name
        ),
        severity: "medium".to_string(),
        confidence: to_confidence(&[], "possible"),
        primary_location: anchor_of(&op.source_anchor, routine),
        evidence_path: path,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: id_list(affected_tables),
        fix_options: vec![FixOption {
            description: format!(
                "Add {0}.Modify() after the Validate (or {0}.Insert() if the record is new). If the Validate is intentional (only running validation logic, not persisting), document the intent.",
                op.record_variable_name
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
        loose_subject: None,
    };
    finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
    findings.push(finding);
}
