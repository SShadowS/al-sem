//! Policy output formatters — port of al-sem `src/cli/format-policy.ts`.
//!
//! Three formats over a [`PolicyRunResult`]:
//!   - `json`: the `policy.check` envelope (insertion-order) with a POLICY-SPECIFIC
//!     Finding serializer (insertion-order, `fingerprint` LAST, optionals
//!     skip-if-none). NOT the gate stable projection. A `dependencies` statement
//!     follows `kind` (#70).
//!   - `sarif`: reuses the gate SARIF shape (driver + rules + results), fingerprints
//!     skip-if-none. No dependencies statement (no consumer reads one).
//!   - `human`: literal TAB-indented summary + findings list (the caller prepends
//!     the dependencies line).

use crate::engine::gate::ordered_json::{Jv, serialize_jv};
use crate::engine::gate::policy::policy_engine::PolicyRunResult;
use crate::engine::gate::policy::policy_types::RuleRunSummary;
use crate::engine::l5::finding::{EvidenceStep, Finding, SourceAnchor};

// ---------------------------------------------------------------------------
// Finding serializer (policy-specific — insertion order, fingerprint LAST).
// ---------------------------------------------------------------------------

/// Serialize the INTERNAL `SourceAnchor` to `{ sourceUnitId, range{…},
/// enclosingRoutineId, syntaxKind }`. Optional hash tails are skip-if-none (always
/// none for policy findings). Mirrors al-sem `SourceAnchor` JSON.stringify order.
fn anchor_to_jv(a: &SourceAnchor) -> Jv {
    let mut pairs: Vec<(String, Jv)> = vec![
        ("sourceUnitId".to_string(), Jv::s(&a.source_unit_id)),
        (
            "range".to_string(),
            Jv::Obj(vec![
                ("startLine".to_string(), Jv::Num(a.start_line as i64)),
                ("startColumn".to_string(), Jv::Num(a.start_column as i64)),
                ("endLine".to_string(), Jv::Num(a.end_line as i64)),
                ("endColumn".to_string(), Jv::Num(a.end_column as i64)),
            ]),
        ),
        (
            "enclosingRoutineId".to_string(),
            Jv::s(&a.enclosing_routine_id),
        ),
        ("syntaxKind".to_string(), Jv::s(&a.syntax_kind)),
    ];
    if let Some(h) = &a.normalized_text_hash {
        pairs.push(("normalizedTextHash".to_string(), Jv::s(h)));
    }
    if let Some(h) = &a.leading_context_hash {
        pairs.push(("leadingContextHash".to_string(), Jv::s(h)));
    }
    if let Some(h) = &a.trailing_context_hash {
        pairs.push(("trailingContextHash".to_string(), Jv::s(h)));
    }
    Jv::Obj(pairs)
}

/// Serialize an `EvidenceStep`: `{ routineId, sourceAnchor, note, [operationId],
/// [callsiteId], [loopId] }`. Optionals skip-if-none (al-sem order: operationId
/// then callsiteId).
fn evidence_step_to_jv(e: &EvidenceStep) -> Jv {
    let mut pairs: Vec<(String, Jv)> = vec![
        ("routineId".to_string(), Jv::s(&e.routine_id)),
        ("sourceAnchor".to_string(), anchor_to_jv(&e.source_anchor)),
        ("note".to_string(), Jv::s(&e.note)),
    ];
    if let Some(op) = &e.operation_id {
        pairs.push(("operationId".to_string(), Jv::s(op)));
    }
    if let Some(cs) = &e.callsite_id {
        pairs.push(("callsiteId".to_string(), Jv::s(cs)));
    }
    if let Some(lp) = &e.loop_id {
        pairs.push(("loopId".to_string(), Jv::s(lp)));
    }
    Jv::Obj(pairs)
}

/// Serialize a policy `Finding` in al-sem construction order, fingerprint LAST.
fn finding_to_jv(f: &Finding) -> Jv {
    let confidence = Jv::Obj(vec![
        ("level".to_string(), Jv::s(&f.confidence.level)),
        (
            "evidence".to_string(),
            Jv::Arr(
                f.confidence
                    .evidence
                    .iter()
                    .map(|e| {
                        // `source` is not stored on a `ConfidenceEvidence` — it is
                        // the one engine-wide constant, re-materialised here
                        // exactly as the R4 projection does.
                        let mut p = vec![(
                            "source".to_string(),
                            Jv::s(crate::engine::l5::finding::EVIDENCE_SOURCE),
                        )];
                        if let Some(n) = &e.note {
                            p.push(("note".to_string(), Jv::s(n)));
                        }
                        Jv::Obj(p)
                    })
                    .collect(),
            ),
        ),
    ]);

    let provenance = Jv::Arr(
        f.provenance
            .iter()
            .map(|e| {
                let mut p = vec![("source".to_string(), Jv::s(e.source))];
                if let Some(n) = &e.note {
                    p.push(("note".to_string(), Jv::s(n)));
                }
                Jv::Obj(p)
            })
            .collect(),
    );

    let mut pairs: Vec<(String, Jv)> = vec![
        ("id".to_string(), Jv::s(&f.id)),
        ("rootCauseKey".to_string(), Jv::s(&f.root_cause_key)),
        ("detector".to_string(), Jv::s(&f.detector)),
        ("title".to_string(), Jv::s(&f.title)),
        ("rootCause".to_string(), Jv::s(&f.root_cause)),
        ("severity".to_string(), Jv::s(&f.severity)),
        ("confidence".to_string(), confidence),
        (
            "primaryLocation".to_string(),
            anchor_to_jv(&f.primary_location),
        ),
        (
            "evidencePath".to_string(),
            Jv::Arr(
                crate::engine::l5::finding::evidence_path_of(f)
                    .iter()
                    .map(evidence_step_to_jv)
                    .collect(),
            ),
        ),
        (
            "affectedObjects".to_string(),
            Jv::Arr(f.affected_objects.iter().map(|s| Jv::s(s)).collect()),
        ),
        (
            "affectedTables".to_string(),
            Jv::Arr(f.affected_tables.iter().map(|s| Jv::s(s)).collect()),
        ),
        (
            "fixOptions".to_string(),
            Jv::Arr(
                f.fix_options
                    .iter()
                    .map(|fo| {
                        Jv::Obj(vec![
                            ("description".to_string(), Jv::s(&fo.description)),
                            ("safety".to_string(), Jv::s(&fo.safety)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("provenance".to_string(), provenance),
    ];
    // fingerprint LAST (skip-if-none — always present for policy findings).
    if let Some(fp) = &f.fingerprint {
        pairs.push(("fingerprint".to_string(), Jv::s(fp)));
    }
    Jv::Obj(pairs)
}

fn rule_summary_to_jv(s: &RuleRunSummary) -> Jv {
    let mut pairs: Vec<(String, Jv)> = vec![
        ("ruleId".to_string(), Jv::s(&s.rule_id)),
        (
            "routinesEvaluated".to_string(),
            Jv::Num(s.routines_evaluated as i64),
        ),
        (
            "routinesMatched".to_string(),
            Jv::Num(s.routines_matched as i64),
        ),
        (
            "routinesSkippedCoverage".to_string(),
            Jv::Num(s.routines_skipped_coverage as i64),
        ),
        (
            "routinesSkippedUnknown".to_string(),
            Jv::Num(s.routines_skipped_unknown as i64),
        ),
        (
            "routinesPassed".to_string(),
            Jv::Num(s.routines_passed as i64),
        ),
        (
            "findingsEmitted".to_string(),
            Jv::Num(s.findings_emitted as i64),
        ),
    ];
    if let Some(errs) = &s.errors {
        pairs.push((
            "errors".to_string(),
            Jv::Arr(errs.iter().map(|e| Jv::s(e)).collect()),
        ));
    }
    Jv::Obj(pairs)
}

// ---------------------------------------------------------------------------
// Public formatters.
// ---------------------------------------------------------------------------

/// `formatPolicy(result, { format: "json", deterministic, alsemVersion })`.
pub fn format_policy_json(
    result: &PolicyRunResult,
    dependencies: &crate::program::resolve::full::LoadedDependencies,
    driver_version: &str,
    deterministic: bool,
) -> String {
    // al-sem: deterministic → "0"; else `new Date().toISOString()` (ms `.sssZ`).
    let generated_at = if deterministic {
        "0".to_string()
    } else {
        crate::engine::gate::format_json::pinned_or_now_iso8601(false)
    };
    let envelope = Jv::Obj(vec![
        ("al_sem_version".to_string(), Jv::s(driver_version)),
        ("generated_at".to_string(), Jv::s(&generated_at)),
        ("kind".to_string(), Jv::s("policy.check")),
        (
            "dependencies".to_string(),
            Jv::from_json(&dependencies.to_json()),
        ),
        ("policySource".to_string(), Jv::s(&result.policy_source)),
        ("policyVersion".to_string(), Jv::Num(result.policy_version)),
        (
            "ruleSummaries".to_string(),
            Jv::Arr(
                result
                    .rule_summaries
                    .iter()
                    .map(rule_summary_to_jv)
                    .collect(),
            ),
        ),
        (
            "findings".to_string(),
            Jv::Arr(result.findings.iter().map(finding_to_jv).collect()),
        ),
        (
            "diagnostics".to_string(),
            Jv::Arr(
                result
                    .diagnostics
                    .iter()
                    .map(|d| {
                        Jv::Obj(vec![
                            ("severity".to_string(), Jv::s(&d.severity)),
                            ("stage".to_string(), Jv::s(&d.stage)),
                            ("message".to_string(), Jv::s(&d.message)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]);
    serialize_jv(&envelope)
}

/// SARIF severity → level (al-sem ternary): info/low → note, medium → warning, else error.
fn sarif_level(severity: &str) -> &'static str {
    match severity {
        "info" | "low" => "note",
        "medium" => "warning",
        _ => "error",
    }
}

/// `formatPolicy(result, { format: "sarif", … })`. Reuses the gate SARIF shape.
pub fn format_policy_sarif(result: &PolicyRunResult, driver_version: &str) -> String {
    let rules = Jv::Arr(
        result
            .rule_summaries
            .iter()
            .map(|s| {
                Jv::Obj(vec![
                    ("id".to_string(), Jv::s(&format!("policy-{}", s.rule_id))),
                    ("name".to_string(), Jv::s(&s.rule_id)),
                ])
            })
            .collect(),
    );

    let results = Jv::Arr(
        result
            .findings
            .iter()
            .map(|f| {
                let mut pairs: Vec<(String, Jv)> = vec![
                    ("ruleId".to_string(), Jv::s(&f.detector)),
                    (
                        "message".to_string(),
                        Jv::Obj(vec![("text".to_string(), Jv::s(&f.root_cause))]),
                    ),
                    ("level".to_string(), Jv::s(sarif_level(&f.severity))),
                    (
                        "locations".to_string(),
                        Jv::Arr(vec![Jv::Obj(vec![(
                            "physicalLocation".to_string(),
                            Jv::Obj(vec![
                                (
                                    "artifactLocation".to_string(),
                                    Jv::Obj(vec![(
                                        "uri".to_string(),
                                        Jv::s(&f.primary_location.source_unit_id),
                                    )]),
                                ),
                                (
                                    "region".to_string(),
                                    Jv::Obj(vec![
                                        (
                                            "startLine".to_string(),
                                            Jv::Num(f.primary_location.start_line as i64),
                                        ),
                                        (
                                            "startColumn".to_string(),
                                            Jv::Num(f.primary_location.start_column as i64),
                                        ),
                                    ]),
                                ),
                            ]),
                        )])]),
                    ),
                ];
                // fingerprints: skip-if-none.
                if let Some(fp) = &f.fingerprint {
                    pairs.push((
                        "fingerprints".to_string(),
                        Jv::Obj(vec![("al-sem/v1".to_string(), Jv::s(fp))]),
                    ));
                }
                Jv::Obj(pairs)
            })
            .collect(),
    );

    let envelope = Jv::Obj(vec![
        (
            "$schema".to_string(),
            Jv::s("https://json.schemastore.org/sarif-2.1.0.json"),
        ),
        ("version".to_string(), Jv::s("2.1.0")),
        (
            "runs".to_string(),
            Jv::Arr(vec![Jv::Obj(vec![
                (
                    "tool".to_string(),
                    Jv::Obj(vec![(
                        "driver".to_string(),
                        Jv::Obj(vec![
                            ("name".to_string(), Jv::s("al-sem")),
                            ("version".to_string(), Jv::s(driver_version)),
                            ("rules".to_string(), rules),
                        ]),
                    )]),
                ),
                ("results".to_string(), results),
            ])]),
        ),
    ]);
    serialize_jv(&envelope)
}

/// `formatPolicy(result, { format: "human", … })`. Literal TAB-indented.
pub fn format_policy_human(result: &PolicyRunResult) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Policy check — {}", result.policy_source));
    lines.push(String::new());
    lines.push("Rule summaries:".to_string());
    for s in &result.rule_summaries {
        lines.push(format!(
            "\t{}: evaluated={} matched={} passed={} skipped(coverage)={} skipped(unknown)={} findings={}",
            s.rule_id,
            s.routines_evaluated,
            s.routines_matched,
            s.routines_passed,
            s.routines_skipped_coverage,
            s.routines_skipped_unknown,
            s.findings_emitted
        ));
    }
    lines.push(String::new());
    if result.findings.is_empty() {
        lines.push("No policy findings.".to_string());
    } else {
        lines.push(format!("Findings ({}):", result.findings.len()));
        for f in &result.findings {
            let label: &str = if !f.title.is_empty() {
                &f.title
            } else {
                &f.root_cause
            };
            lines.push(format!("\t[{}] {} — {}", f.severity, f.detector, label));
        }
    }
    format!("{}\n", lines.join("\n"))
}
