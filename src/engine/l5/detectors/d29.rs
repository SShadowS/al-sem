//! D29 — an `[EventSubscriber]` of a record-modify (or delete) event that calls
//! `Modify` / `ModifyAll` / `Delete` / `DeleteAll` on the inbound record parameter
//! re-fires the same publisher event, opening a recursive-trigger loop.
//! Port of al-sem `src/detectors/d29-subscriber-modify-on-event-record.ts`.
//!
//! Two conditions:
//!  1. Routine carries `[EventSubscriber(..., '<eventName>', ...)]` and
//!     `<eventName>` matches a Modify/Delete-shaped pattern.
//!  2. Body has a Modify/ModifyAll/Delete/DeleteAll on a record-typed parameter.
//!
//! event-subscriber only + bodyAvailable + !parseIncomplete; skip when there is no
//! record-typed parameter. Within-detector sort by `compareStrings(a.id, b.id)`.

use std::collections::HashSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::{anchor_of, is_known_temp};
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, id_list,
};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::{AttributeInfo, find_attribute, string_arg};
use crate::program::model::workspace::{Model, ModelRecordOperation, ModelRoutine};

const DETECTOR: &str = "d29-subscriber-modify-on-event-record";

const MUTATING_OPS: &[&str] = &["Modify", "ModifyAll", "Delete", "DeleteAll"];

/// Extract the publisher-event name from a routine's `[EventSubscriber(...)]`
/// attribute. Returns the third positional arg (the event name), lowercased, or
/// None when the routine isn't a subscriber / the event-name arg is missing.
/// Mirrors al-sem `eventName` (reuses `find_attribute` / `string_arg`).
fn event_name(attrs: &[AttributeInfo]) -> Option<String> {
    let attr = find_attribute(attrs, "EventSubscriber")?;
    string_arg(attr, 2).map(|s| s.to_lowercase())
}

/// Hand-rolled faithful reproduction of al-sem's `MODIFY_EVENT_PATTERNS`
/// (`String.test`, case-insensitive, UNANCHORED ⇒ substring search). The six
/// regexes are:
///   /onafter(?:validate)?modify(?:event)?/i
///   /onbefore(?:validate)?modify(?:event)?/i
///   /onaftermodifyevent/i        (⊂ regex 1)
///   /onbeforemodifyevent/i       (⊂ regex 2)
///   /onafterdelete(?:event)?/i
///   /onbeforedelete(?:event)?/i
///
/// Each `(?:…)?` group is OPTIONAL and each pattern is unanchored, so a name
/// matches iff it CONTAINS one of the concrete required cores below. The trailing
/// `(?:event)?` adds no new required substring (when the longer "…event" form is
/// present, the shorter prefix is already a substring), so the exact match set is:
///   onaftermodify, onaftervalidatemodify, onbeforemodify, onbeforevalidatemodify,
///   onafterdelete, onbeforedelete
/// The two redundant regexes (3, 4) are subsets of 1 / 2 and add nothing. This set
/// reproduces every regex's match set exactly on event-name strings.
const MODIFY_EVENT_CORES: &[&str] = &[
    "onaftermodify",
    "onaftervalidatemodify",
    "onbeforemodify",
    "onbeforevalidatemodify",
    "onafterdelete",
    "onbeforedelete",
];

fn is_modify_event(name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    // `name` is already lowercased by `event_name`; the cores are lowercase.
    MODIFY_EVENT_CORES.iter().any(|core| name.contains(core))
}

pub fn detect_d29(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_non_modify_event = 0u64;
    let mut skipped_no_record_param = 0u64;
    let mut skipped_run_trigger_false = 0u64;
    let mut skipped_temp_record = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) !== "primary" → skip. Source-only: every routine is
        // primary, so this never skips (mirrors al-sem semantics).
        if routine.kind != "event-subscriber" {
            continue;
        }
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        // al-sem's `void parseRoutineAttributes(routine)` cross-check is a
        // side-effect-free no-op that does not affect output — OMITTED.

        let evt = event_name(&routine.attributes_parsed);
        if !is_modify_event(evt.as_deref()) {
            skipped_non_modify_event += 1;
            continue;
        }

        let record_param_names: HashSet<String> = routine
            .parameters
            .iter()
            .filter(|p| p.is_record)
            .map(|p| p.name.to_lowercase())
            .collect();
        if record_param_names.is_empty() {
            skipped_no_record_param += 1;
            continue;
        }
        candidates_considered += 1;

        let evt_lc = evt.as_deref().unwrap_or("<unknown event>");

        for op in &routine.record_operations {
            if !MUTATING_OPS.contains(&op.op.as_str()) {
                continue;
            }
            let var_key = op.record_variable_name.to_lowercase();
            if !record_param_names.contains(&var_key) {
                continue;
            }
            // FP-1: `Modify(false)` / `Delete(false)` / `ModifyAll(..., false)`
            // (RunTrigger=false) is the canonical pattern to AVOID recursive
            // trigger re-fire — the platform skips the modify/delete triggers, so
            // the same event is NOT raised again. No recursion → no finding.
            // (Only an exact `false` literal suppresses; `true`/unknown keep firing.)
            if op.run_trigger == Some(false) {
                skipped_run_trigger_false += 1;
                continue;
            }
            // A provably-temporary record (in-memory) fires no database triggers,
            // so a Modify/Delete on it cannot re-raise the publisher event.
            if is_known_temp(op) {
                skipped_temp_record += 1;
                continue;
            }
            emit(routine, op, evt_lc, &mut findings, fp_index);
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("nonModifyEvent", skipped_non_modify_event);
    stats.add_skip("noRecordParam", skipped_no_record_param);
    stats.add_skip("runTriggerFalse", skipped_run_trigger_false);
    stats.add_skip("tempRecord", skipped_temp_record);
    Ok(DetectorOutput::no_diag(findings, stats))
}

fn emit(
    routine: &ModelRoutine,
    op: &ModelRecordOperation,
    event_name_lc: &str,
    findings: &mut Vec<Finding>,
    fp_index: &FingerprintIndex,
) {
    let path = vec![
        EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_of(&routine.source_anchor, routine),
            note: format!("[EventSubscriber] {} on '{}'", routine.name, event_name_lc),
        },
        EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: Some(op.id.clone()),
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_of(&op.source_anchor, routine),
            note: format!(
                "{} on {} — the event's inbound record parameter",
                op.op, op.record_variable_name
            ),
        },
    ];

    let id = format!("d29/{}/{}", routine.id, op.id);
    let root_cause_key = id.clone();

    let confidence: FindingConfidence = to_confidence(&[], "likely");

    let root_cause = format!(
        "{} subscribes to '{}' and calls {} on {}, the inbound record parameter — re-firing \
         the same event from a subscriber can recurse.",
        routine.name, event_name_lc, op.op, op.record_variable_name
    );

    let affected_tables: Vec<String> = match &op.table_id {
        Some(t) => vec![t.clone()],
        None => Vec::new(),
    };

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: "Event subscriber mutates the inbound record".into(),
        root_cause,
        severity: "medium".to_string(),
        confidence,
        primary_location: anchor_of(&op.source_anchor, routine),
        evidence_path: path,
        additional_paths: None,
        affected_objects: vec![routine.object_id.as_str().into()],
        affected_tables: id_list(affected_tables),
        fix_options: vec![FixOption {
            description:
                "Either use Modify(false) to suppress trigger re-firing, perform the mutation \
                 on a fresh record loaded by primary key, or move the work outside the \
                 subscriber path."
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
