//! D59 — `var Boolean` security-guard parameter on an `[IntegrationEvent]`.
//! BCQuality `integrationevent-var-parameter-bypasses-security-guards`: any
//! subscriber (including third-party) can flip a writable Boolean that gates a
//! security decision. Name-heuristic (documented FP surface — precision-first
//! deny-list for the sanctioned IsHandled handshake).
//!
//! Severity: medium. Confidence: possible.

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::attributes::has_attribute;

const DETECTOR: &str = "d59-integrationevent-var-boolean-guard";

/// Guard-name heuristic. Deny-list first (IsHandled is the sanctioned
/// handshake), then permission/skip-shaped prefixes and substrings.
fn is_guard_name(raw: &str) -> bool {
    let n = raw.trim_matches('"').to_lowercase();
    if n == "ishandled" || n == "handled" {
        return false;
    }
    n.starts_with("skip")
        || n.starts_with("bypass")
        || n.starts_with("allow")
        || n.contains("hasaccess")
        || n.contains("permission")
        || n.contains("authoriz")
        || n.contains("authoris")
        || n == "isallowed"
        || n == "isvalid"
        || n == "cancontinue"
}

pub fn detect_d59(
    resolved: &L3Resolved,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_non_guard_name = 0u64;

    for routine in &ws.routines {
        if routine.kind != "event-publisher" {
            continue;
        }
        if !has_attribute(&routine.attributes_parsed, "IntegrationEvent") {
            continue;
        }
        for p in &routine.parameters {
            if !p.is_var || !p.type_text.trim().eq_ignore_ascii_case("boolean") {
                continue;
            }
            candidates_considered += 1;
            if !is_guard_name(&p.name) {
                skipped_non_guard_name += 1;
                continue;
            }

            let confidence: FindingConfidence = to_confidence(&[], "possible");
            let id = format!("d59/{}/{}", routine.id, p.index);
            let mut finding = Finding {
                id: id.clone(),
                root_cause_key: id,
                detector: DETECTOR.to_string(),
                title: "Writable security-guard parameter on integration event".into(),
                root_cause: format!(
                    "Integration event {} exposes `var {}: Boolean` — any subscriber \
                     (including third-party extensions) can flip this guard and bypass the \
                     security decision it feeds.",
                    routine.name, p.name
                ),
                severity: "medium".to_string(),
                confidence,
                primary_location: anchor_of(&routine.source_anchor, routine),
                evidence_path: vec![EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&routine.source_anchor, routine),
                    note: format!(
                        "[IntegrationEvent] {} (var {}: Boolean)",
                        routine.name, p.name
                    ),
                }],
                additional_paths: None,
                affected_objects: vec![routine.object_id.as_str().into()],
                affected_tables: Vec::new(),
                fix_options: vec![FixOption {
                    description: format!(
                        "Make {} non-var (informational), or replace the writable guard with \
                         an explicit, audited decision API the publisher controls.",
                        p.name
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
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("nonGuardName", skipped_non_guard_name);
    Ok(DetectorOutput::no_diag(findings, stats))
}

#[cfg(test)]
mod tests {
    use super::is_guard_name;

    #[test]
    fn guard_names_flagged() {
        for n in [
            "HasAccess",
            "SkipValidation",
            "BypassCheck",
            "AllowPosting",
            "IsAllowed",
            "\"Has Permission\"",
            "Authorized",
        ] {
            assert!(is_guard_name(n), "{n} should be a guard name");
        }
    }

    #[test]
    fn sanctioned_and_plain_names_not_flagged() {
        for n in ["IsHandled", "Handled", "Found", "Result", "Done"] {
            assert!(!is_guard_name(n), "{n} should NOT be a guard name");
        }
    }
}
