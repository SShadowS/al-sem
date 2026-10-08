//! D57 — Growing globals in SingleInstance subscribers. A SingleInstance
//! codeunit's globals live for the SESSION; an event subscriber that appends to
//! a global collection (`List`/`Dictionary` `.Add`/`.Insert`/`.AddRange`) or
//! inserts into a global TEMP record, with NO clearing path anywhere in the
//! object (`Clear`/`Remove*` member or bare `Clear(<g>)`, `Delete`/`DeleteAll`
//! for records), grows unboundedly — a session-lifetime memory leak.
//!
//! Every uncertainty (non-global receiver, unknown scope, cleared-somewhere)
//! SKIPS — advisory precision-first. Severity: medium. Confidence: possible.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, is_known_temp};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor,
};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::body::features::PCallee;
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d57-singleinstance-growing-state";

const GROW_METHODS: &[&str] = &["Add", "Insert", "AddRange"];
const CLEAR_METHODS: &[&str] = &[
    "Clear",
    "Remove",
    "RemoveAt",
    "RemoveRange",
    "DeleteAll",
    "Delete",
];

pub fn detect_d57(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_cleared_in_object = 0u64;
    let mut skipped_not_collection = 0u64;

    // SingleInstance codeunit object ids.
    let si_objects: HashSet<&str> = ws
        .objects
        .iter()
        .filter(|o| o.object_type == "Codeunit" && o.single_instance == Some(true))
        .map(|o| o.id.as_str())
        .collect();
    if si_objects.is_empty() {
        let stats = DetectorStats::new(DETECTOR, 0, 0);
        return Ok(DetectorOutput::no_diag(findings, stats));
    }

    // Per SingleInstance object: receiver names (lowercased) with ANY clearing
    // signal anywhere in the object.
    let mut cleared_by_object: HashMap<&str, HashSet<String>> = HashMap::new();
    for r in &ws.routines {
        if !si_objects.contains(r.object_id.as_str()) {
            continue;
        }
        let set = cleared_by_object.entry(r.object_id.as_str()).or_default();
        for cs in &r.call_sites {
            match &cs.callee {
                PCallee::Member { receiver, method }
                    if CLEAR_METHODS.iter().any(|m| m.eq_fold_identifier(method)) =>
                {
                    set.insert(receiver.to_lowercase());
                }
                // `argument_bindings.source_variable_name` is populated only for
                // record-shaped identifiers (parameter/implicit-rec/local record —
                // see the L2 argument-binding post-pass); a `List`/`Dictionary`
                // global is never a record variable, so it always binds
                // "unknown" there. Read the raw single-argument text instead —
                // the same structural pattern d3's `is_clear_call_on` uses for
                // `Clear(<var>)` on a record variable.
                PCallee::Bare { name }
                    if name.eq_fold_identifier("Clear") && cs.argument_texts.len() == 1 =>
                {
                    set.insert(cs.argument_texts[0].trim().to_lowercase());
                }
                _ => {}
            }
        }
        for op in &r.record_operations {
            if op.op == "DeleteAll" || op.op == "Delete" {
                set.insert(op.record_variable_name.to_lowercase());
            }
        }
    }

    for routine in &ws.routines {
        if routine.kind != "event-subscriber" {
            continue;
        }
        if !si_objects.contains(routine.object_id.as_str()) {
            continue;
        }
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        let cleared = cleared_by_object.get(routine.object_id.as_str());
        let is_cleared = |name_lc: &str| cleared.is_some_and(|s| s.contains(name_lc));

        // (a) Global List/Dictionary growth.
        for cs in &routine.call_sites {
            let PCallee::Member { receiver, method } = &cs.callee else {
                continue;
            };
            if !GROW_METHODS.iter().any(|m| m.eq_fold_identifier(method)) {
                continue;
            }
            let recv_lc = receiver.to_lowercase();
            let Some(v) = routine
                .variables
                .iter()
                .find(|v| v.name.to_lowercase() == recv_lc && v.scope.as_deref() == Some("global"))
            else {
                continue;
            };
            let ty = v.declared_type.to_lowercase();
            if !(ty.starts_with("list of") || ty.starts_with("dictionary of")) {
                skipped_not_collection += 1;
                continue;
            }
            candidates_considered += 1;
            if is_cleared(&recv_lc) {
                skipped_cleared_in_object += 1;
                continue;
            }
            findings.push(build_finding(
                fp_index,
                routine,
                &cs.id,
                anchor_of(&cs.source_anchor, routine),
                &format!(
                    "{} is an event subscriber in a SingleInstance codeunit appending to \
                     the global {} {} — no clearing path exists in the object, so the \
                     collection grows for the whole session.",
                    routine.name, v.declared_type, receiver
                ),
                receiver,
            ));
        }

        // (b) Global temp-record growth.
        for op in &routine.record_operations {
            if op.op != "Insert" {
                continue;
            }
            let var_lc = op.record_variable_name.to_lowercase();
            let Some(rv) = routine
                .record_variables
                .iter()
                .find(|rv| rv.name.to_lowercase() == var_lc)
            else {
                continue;
            };
            if rv.scope.as_deref() != Some("global") {
                continue;
            }
            // Physical global inserts are transaction detectors' territory —
            // d57 only tracks in-memory growth.
            if !is_known_temp(op) {
                continue;
            }
            candidates_considered += 1;
            if is_cleared(&var_lc) {
                skipped_cleared_in_object += 1;
                continue;
            }
            findings.push(build_finding(
                fp_index,
                routine,
                &op.id,
                anchor_of(&op.source_anchor, routine),
                &format!(
                    "{} is an event subscriber in a SingleInstance codeunit inserting into \
                     the global temporary record {} — no Delete/DeleteAll exists in the \
                     object, so the buffer grows for the whole session.",
                    routine.name, op.record_variable_name
                ),
                &op.record_variable_name,
            ));
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("clearedInObject", skipped_cleared_in_object);
    stats.add_skip("notCollection", skipped_not_collection);
    Ok(DetectorOutput::no_diag(findings, stats))
}

fn build_finding(
    fp_index: &FingerprintIndex,
    routine: &crate::program::model::workspace::ModelRoutine,
    site_id: &str,
    anchor: SourceAnchor,
    root_cause: &str,
    var_name: &str,
) -> Finding {
    let confidence: FindingConfidence = to_confidence(&[], "possible");
    let id = format!("d57/{}/{}", routine.id, site_id);
    let mut finding = Finding {
        id: id.clone(),
        root_cause_key: format!("d57/{}/{}", routine.id, var_name.to_lowercase()),
        detector: DETECTOR.to_string(),
        title: "Growing global state in SingleInstance subscriber".into(),
        root_cause: root_cause.to_string(),
        severity: "medium".to_string(),
        confidence,
        primary_location: anchor.clone(),
        evidence_path: vec![EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor,
            note: format!("unbounded append to global {var_name}"),
        }],
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: vec![FixOption {
            description: format!(
                "Bound the growth: clear/drain {var_name} (Clear/Remove/DeleteAll) on a \
                 defined lifecycle point, or replace the session-lifetime cache with a \
                 keyed lookup that overwrites instead of appending."
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
    finding
}
