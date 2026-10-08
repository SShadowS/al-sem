//! D32 — a `local procedure` parameter of type `Boolean` is passed the same
//! literal (`true` or `false`) at every resolved primary-app call site. The
//! parameter is dead: either the value is never actually variable, or the
//! branches it gates can be flattened out of the procedure.
//!
//! Port of al-sem `src/detectors/d32-constant-boolean-parameter.ts`.
//!
//! Tight scope (mirrors al-sem):
//!  - `access_modifier == Some("local")` only — public/internal/protected
//!    procedures may have callers outside the workspace; non-local stay out.
//!  - `kind == "procedure"` only — triggers and event subscribers have
//!    publisher-dictated signatures; flattening is not an option.
//!  - `body_available` — bodyless routines have no callsite evidence.
//!  - parameter `type_text` contains "boolean" (case-insensitive word-boundary
//!    check — hand-rolled without regex).
//!  - at least 2 resolved primary-app call sites (direct kind only).
//!  - every reaching edge must be `direct`; mixed call shapes leave too much
//!    uncertainty — bail rather than risk a false positive.
//!  - each direct-edge callsite `argument_texts[param.index]` must exist and
//!    be a boolean literal (`true`/`false`, trimmed + lowercased, exact match).
//!  - all resolved callers must agree on the SAME literal; any divergence skips
//!    that parameter.
//!
//! Within-detector sort: `a.id.cmp(&b.id)`.

use std::collections::BTreeSet;

use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::fingerprint::FingerprintIndex;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::{Model, ModelParameter, ModelRoutine};

const DETECTOR: &str = "d32-constant-boolean-parameter";

/// `isBooleanLiteral` — matches exactly "true" or "false" (case-insensitive,
/// after trimming). Returns the canonical lowercase form, or `None`.
fn is_boolean_literal(arg: &str) -> Option<&'static str> {
    match arg.trim().to_lowercase().as_str() {
        "true" => Some("true"),
        "false" => Some("false"),
        _ => None,
    }
}

/// Hand-rolled word-boundary check for "boolean" (case-insensitive) in
/// `type_text`. Mirrors al-sem `/\bBoolean\b/i`.
///
/// A "word boundary" here means: the character before the match (if any) and
/// the character after the match (if any) must each be a non-alphanumeric,
/// non-underscore character (i.e. NOT `[A-Za-z0-9_]`).
fn has_boolean_word(type_text: &str) -> bool {
    let needle = "boolean";
    let lower = type_text.to_lowercase();
    let bytes = lower.as_bytes();
    let nlen = needle.len();
    let tlen = bytes.len();
    if tlen < nlen {
        return false;
    }
    for i in 0..=(tlen - nlen) {
        if &bytes[i..i + nlen] != needle.as_bytes() {
            continue;
        }
        // Word-boundary check before the match.
        if i > 0 {
            let b = bytes[i - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                continue;
            }
        }
        // Word-boundary check after the match.
        let after = i + nlen;
        if after < tlen {
            let b = bytes[after];
            if b.is_ascii_alphanumeric() || b == b'_' {
                continue;
            }
        }
        return true;
    }
    false
}

pub fn detect_d32(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_non_local = 0u64;
    let mut skipped_no_boolean_params = 0u64;
    let mut skipped_too_few_callers = 0u64;
    let mut skipped_unresolved_or_mixed_edges = 0u64;
    let mut skipped_varies = 0u64;

    for callee in &ws.routines {
        // roleOf(callee) !== "primary" → skip. Source-only: every routine is
        // primary (mirrors al-sem semantics).
        if !callee.body_available {
            continue;
        }
        // kind == "procedure" only.
        if callee.kind != "procedure" {
            continue;
        }
        // access_modifier == Some("local") only.
        if callee.access_modifier.as_deref() != Some("local") {
            skipped_non_local += 1;
            continue;
        }

        // Boolean parameters.
        let bool_params: Vec<&ModelParameter> = callee
            .parameters
            .iter()
            .filter(|p| has_boolean_word(&p.type_text))
            .collect();
        if bool_params.is_empty() {
            skipped_no_boolean_params += 1;
            continue;
        }

        // Incoming edges from the reverse call graph.
        let incoming: &[crate::engine::l4::combined_graph::CombinedEdge] = ctx
            .reverse_call_graph
            .get(&callee.id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let direct_edges: Vec<_> = incoming.iter().filter(|e| e.kind == "direct").collect();

        if direct_edges.len() < 2 {
            skipped_too_few_callers += 1;
            continue;
        }

        // Any non-direct edge → mixed → bail.
        if incoming.iter().any(|e| e.kind != "direct") {
            skipped_unresolved_or_mixed_edges += 1;
            continue;
        }

        candidates_considered += 1;

        for param in bool_params {
            let mut values: BTreeSet<&'static str> = BTreeSet::new();
            let mut bailed = false;

            for e in &direct_edges {
                // Resolve caller.
                let caller = match ctx.routine_by_id.get(e.from.as_str()) {
                    Some(r) => r,
                    None => {
                        bailed = true;
                        break;
                    }
                };
                // roleOf(caller) !== "primary" → bail. Source-only: always primary.
                // (Mirrors al-sem's `roleOf(caller) !== "primary"` check.)
                // For completeness: body_available is not required of the CALLER
                // (only that we can find the call site).

                // Find the call site by callsite_id.
                let callsite_id = match e.callsite_id.as_deref() {
                    Some(id) => id,
                    None => {
                        bailed = true;
                        break;
                    }
                };
                let cs = caller.call_sites.iter().find(|cs| cs.id == callsite_id);
                let cs = match cs {
                    Some(c) => c,
                    None => {
                        bailed = true;
                        break;
                    }
                };

                // argument_texts[param.index]
                let arg_text = cs.argument_texts.get(param.index as usize);
                let arg_text = match arg_text {
                    Some(t) => t,
                    None => {
                        bailed = true;
                        break;
                    }
                };

                let lit = match is_boolean_literal(arg_text) {
                    Some(l) => l,
                    None => {
                        bailed = true;
                        break;
                    }
                };

                values.insert(lit);
                if values.len() > 1 {
                    bailed = true;
                    break;
                }
            }

            if bailed {
                skipped_varies += 1;
                continue;
            }

            let constant = match values.into_iter().next() {
                Some(v) => v,
                None => continue,
            };

            emit(
                callee,
                param,
                constant,
                direct_edges.len(),
                &mut findings,
                fp_index,
            );
        }
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("nonLocal", skipped_non_local);
    stats.add_skip("noBooleanParams", skipped_no_boolean_params);
    stats.add_skip("tooFewCallers", skipped_too_few_callers);
    stats.add_skip("unresolvedOrMixedEdges", skipped_unresolved_or_mixed_edges);
    stats.add_skip("varies", skipped_varies);
    Ok(DetectorOutput::no_diag(findings, stats))
}

fn emit(
    callee: &ModelRoutine,
    param: &ModelParameter,
    constant_value: &str,
    caller_count: usize,
    findings: &mut Vec<Finding>,
    fp_index: &FingerprintIndex,
) {
    let path = vec![EvidenceStep {
        routine_id: callee.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: None,
        source_anchor: anchor_of(&callee.source_anchor, callee),
        note: format!(
            "local procedure {}({}: {}) — parameter at position {}",
            callee.name, param.name, param.type_text, param.index
        ),
    }];

    let id = format!("d32/{}/p{}", callee.id, param.index);
    let root_cause_key = id.clone();

    let confidence: FindingConfidence = to_confidence(&[], "likely");

    let root_cause = format!(
        "{} declares Boolean parameter '{}' at position {}, but all {} resolved primary-app \
         callers pass `{}` — the parameter is dead.",
        callee.name, param.name, param.index, caller_count, constant_value
    );

    let fix_description = format!(
        "Flatten the procedure: assume '{} = {}' at every site, simplify the body, and remove \
         the parameter. If the parameter is intended for future use, leave a TODO documenting it.",
        param.name, constant_value
    );

    let mut finding = Finding {
        id,
        root_cause_key,
        detector: DETECTOR.to_string(),
        title: "Boolean parameter is always passed the same literal".into(),
        root_cause,
        severity: "info".to_string(),
        confidence,
        primary_location: anchor_of(&callee.source_anchor, callee),
        evidence_path: path,
        additional_paths: None,
        affected_objects: vec![callee.object_id.as_str().into()],
        affected_tables: Vec::new(),
        fix_options: vec![FixOption {
            description: fix_description.into(),
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
