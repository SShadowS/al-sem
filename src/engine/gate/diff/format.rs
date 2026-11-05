//! Diff formatters: human / json (envelope-wrapped) / sarif. Port of al-sem
//! `src/diff/format-diff.ts` + `src/cli/diff.ts`'s json envelope path
//! (`wrapDiffReport` + `serializeDocument`).

use indexmap::IndexMap;

use crate::engine::gate::cbor::CborValue;
use crate::engine::l5::snapshot_full::to_sorted_json;

use super::fingerprint::DiffCategory;
use super::{DiffDiagnostic, DiffEngineResult, DiffFinding, DiffSummary, Severity};

/// The diff-report contract version (al-sem `DIFF_CONTRACT_VERSION`).
const DIFF_CONTRACT_VERSION: &str = "1.0.0";

/// Render the diff result in the requested format. `driver_version` /
/// `deterministic` / `analyzer_diagnostics` are only consulted for the json
/// envelope.
pub fn format_diff(
    result: &DiffEngineResult,
    format: &str,
    driver_version: &str,
    deterministic: bool,
    analyzer_diagnostics: &[CborValue],
) -> String {
    match format {
        "json" => render_json_envelope(result, driver_version, deterministic, analyzer_diagnostics),
        "sarif" => render_sarif(result),
        _ => render_human(result),
    }
}

// ── human ───────────────────────────────────────────────────────────────────

fn render_human(result: &DiffEngineResult) -> String {
    let mut lines: Vec<String> = Vec::new();
    if result.findings.is_empty() && result.diagnostics.is_empty() {
        return "No diff findings.\n".to_string();
    }
    if !result.diagnostics.is_empty() {
        lines.push("diagnostics:".to_string());
        for d in &result.diagnostics {
            lines.push(format!("  {}: {}", d.kind, diagnostic_json_inline(d)));
        }
        lines.push(String::new());
    }
    if result.findings.is_empty() {
        return format!("{}\n", lines.join("\n"));
    }
    let s = &result.summary;
    lines.push(format!(
        "Diff: {} finding(s). critical={} high={} medium={} low={} info={}",
        result.findings.len(),
        s.findings_by_severity[0],
        s.findings_by_severity[1],
        s.findings_by_severity[2],
        s.findings_by_severity[3],
        s.findings_by_severity[4],
    ));
    lines.push(String::new());

    let contract: Vec<&DiffFinding> = result
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                DiffCategory::Abi | DiffCategory::Schema | DiffCategory::Events
            )
        })
        .collect();
    let effects: Vec<&DiffFinding> = result
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                DiffCategory::Capabilities | DiffCategory::Permissions
            )
        })
        .collect();

    if !contract.is_empty() {
        lines.push("Contract:".to_string());
        for f in &contract {
            lines.push(format_finding(f));
        }
        lines.push(String::new());
    }
    if !effects.is_empty() {
        lines.push("Effects:".to_string());
        for f in &effects {
            lines.push(format_finding(f));
        }
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n"))
}

fn format_finding(f: &DiffFinding) -> String {
    let rename_note = match (
        &f.subject.old_original_stable_id,
        &f.subject.normalized_stable_id,
    ) {
        (Some(orig), normalized) if orig != normalized => {
            format!(" (renamed from {orig})")
        }
        _ => String::new(),
    };
    let coverage_note = match &f.coverage_state {
        Some((old, new)) if old != "complete" || new != "complete" => {
            format!(" [cov old={old} new={new}]")
        }
        _ => String::new(),
    };
    format!(
        "  [{}] {}: {}{}{}",
        f.severity.as_str(),
        f.kind.as_str(),
        f.subject.display_name,
        rename_note,
        coverage_note
    )
}

/// `JSON.stringify(d)` of a diagnostic — insertion-order, compact (no spaces).
fn diagnostic_json_inline(d: &DiffDiagnostic) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in &d.fields {
        parts.push(format!("{}:{}", json_str(k), compact_json(v)));
    }
    format!("{{{}}}", parts.join(","))
}

fn compact_json(v: &CborValue) -> String {
    match v {
        CborValue::Null | CborValue::Undefined => "null".into(),
        CborValue::Bool(b) => if *b { "true" } else { "false" }.into(),
        CborValue::Int(n) => n.to_string(),
        CborValue::Float(f) => f.to_string(),
        CborValue::Text(s) => json_str(s),
        CborValue::Array(items) => {
            let inner: Vec<String> = items.iter().map(compact_json).collect();
            format!("[{}]", inner.join(","))
        }
        CborValue::Map(m) => {
            let inner: Vec<String> = m
                .iter()
                .map(|(k, val)| format!("{}:{}", json_str(k), compact_json(val)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

fn json_str(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── json (envelope-wrapped) ─────────────────────────────────────────────────

/// Build a finding as a CborValue map (the raw payload finding shape).
pub(crate) fn finding_to_cbor(f: &DiffFinding) -> CborValue {
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert("id".into(), CborValue::Text(f.id.clone()));
    m.insert(
        "category".into(),
        CborValue::Text(f.category.as_str().into()),
    );
    m.insert("kind".into(), CborValue::Text(f.kind.as_str().into()));
    m.insert(
        "severity".into(),
        CborValue::Text(f.severity.as_str().into()),
    );

    // subject — undefined optional keys are dropped by the sorted serializer, so
    // only insert present ones.
    let mut subj: IndexMap<String, CborValue> = IndexMap::new();
    subj.insert(
        "normalizedStableId".into(),
        CborValue::Text(f.subject.normalized_stable_id.clone()),
    );
    if let Some(o) = &f.subject.old_original_stable_id {
        subj.insert("oldOriginalStableId".into(), CborValue::Text(o.clone()));
    }
    if let Some(n) = &f.subject.new_stable_id {
        subj.insert("newStableId".into(), CborValue::Text(n.clone()));
    }
    subj.insert(
        "displayName".into(),
        CborValue::Text(f.subject.display_name.clone()),
    );
    m.insert("subject".into(), CborValue::Map(subj));

    m.insert(
        "comparisonCone".into(),
        CborValue::Array(
            f.comparison_cone
                .iter()
                .map(|s| CborValue::Text(s.clone()))
                .collect(),
        ),
    );

    let mut details: IndexMap<String, CborValue> = IndexMap::new();
    for (k, v) in &f.details {
        details.insert(k.clone(), v.clone());
    }
    m.insert("details".into(), CborValue::Map(details));

    if let Some((old, new)) = &f.coverage_state {
        let mut cs: IndexMap<String, CborValue> = IndexMap::new();
        cs.insert("old".into(), CborValue::Text(old.clone()));
        cs.insert("new".into(), CborValue::Text(new.clone()));
        m.insert("coverageState".into(), CborValue::Map(cs));
    }

    CborValue::Map(m)
}

pub(crate) fn diagnostic_to_cbor(d: &DiffDiagnostic) -> CborValue {
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    for (k, v) in &d.fields {
        m.insert(k.clone(), v.clone());
    }
    CborValue::Map(m)
}

fn summary_to_cbor(s: &DiffSummary) -> CborValue {
    let mut by_cat: IndexMap<String, CborValue> = IndexMap::new();
    for (cat, n) in &s.findings_by_category {
        by_cat.insert(cat.as_str().into(), CborValue::Int(*n as i64));
    }
    let mut by_sev: IndexMap<String, CborValue> = IndexMap::new();
    let sev_names = ["critical", "high", "medium", "low", "info"];
    for (i, name) in sev_names.iter().enumerate() {
        by_sev.insert(
            (*name).into(),
            CborValue::Int(s.findings_by_severity[i] as i64),
        );
    }
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert("findingsByCategory".into(), CborValue::Map(by_cat));
    m.insert("findingsBySeverity".into(), CborValue::Map(by_sev));
    m.insert(
        "coverageIncompleteCones".into(),
        CborValue::Int(s.coverage_incomplete_cones as i64),
    );
    m.insert(
        "renamesApplied".into(),
        CborValue::Int(s.renames_applied as i64),
    );
    CborValue::Map(m)
}

/// The raw diff payload `{findings, diagnostics, summary}` as a CborValue map.
fn payload_to_cbor(result: &DiffEngineResult) -> CborValue {
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert(
        "findings".into(),
        CborValue::Array(result.findings.iter().map(finding_to_cbor).collect()),
    );
    m.insert(
        "diagnostics".into(),
        CborValue::Array(result.diagnostics.iter().map(diagnostic_to_cbor).collect()),
    );
    m.insert("summary".into(), summary_to_cbor(&result.summary));
    CborValue::Map(m)
}

/// Build + serialize the diff-report ENVELOPE (`wrapDiffReport` + `makeEnvelope`
/// + `serializeDocument`). Sorted-key, undefined-dropped, trailing newline.
fn render_json_envelope(
    result: &DiffEngineResult,
    driver_version: &str,
    deterministic: bool,
    analyzer_diagnostics: &[CborValue],
) -> String {
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    let mut env: IndexMap<String, CborValue> = IndexMap::new();
    env.insert("kind".into(), CborValue::Text("diff-report".into()));
    env.insert(
        "schemaVersion".into(),
        CborValue::Text(DIFF_CONTRACT_VERSION.into()),
    );
    env.insert(
        "alsemVersion".into(),
        CborValue::Text(driver_version.to_string()),
    );
    env.insert("deterministic".into(), CborValue::Bool(deterministic));
    env.insert("generatedAt".into(), CborValue::Text(generated_at));
    env.insert(
        "diagnostics".into(),
        CborValue::Array(analyzer_diagnostics.to_vec()),
    );
    env.insert("payload".into(), payload_to_cbor(result));

    // serializeDocument: sorted-key, undefined-dropped, 2-space, trailing newline.
    to_sorted_json(&CborValue::Map(env))
}

// ── sarif ───────────────────────────────────────────────────────────────────

fn severity_to_sarif_level(s: Severity) -> &'static str {
    match s {
        Severity::Critical | Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Low | Severity::Info => "note",
    }
}

/// Render SARIF 2.1.0. The `rules` + `results` arrays follow FINDING order (NOT
/// re-sorted); object keys are sorted by the serializer. Mirrors `renderSarif`.
fn render_sarif(result: &DiffEngineResult) -> String {
    let mut rules: Vec<CborValue> = Vec::new();
    let mut rules_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut results: Vec<CborValue> = Vec::new();

    for f in &result.findings {
        let rule_id = format!("{}.{}", f.category.as_str(), f.kind.as_str());
        if rules_seen.insert(rule_id.clone()) {
            let mut rule: IndexMap<String, CborValue> = IndexMap::new();
            rule.insert("id".into(), CborValue::Text(rule_id.clone()));
            rule.insert("name".into(), CborValue::Text(f.kind.as_str().into()));
            let mut short: IndexMap<String, CborValue> = IndexMap::new();
            short.insert(
                "text".into(),
                CborValue::Text(format!("{} {}", f.category.as_str(), f.kind.as_str())),
            );
            rule.insert("shortDescription".into(), CborValue::Map(short));
            rules.push(CborValue::Map(rule));
        }
        let mut res: IndexMap<String, CborValue> = IndexMap::new();
        res.insert("ruleId".into(), CborValue::Text(rule_id));
        res.insert(
            "level".into(),
            CborValue::Text(severity_to_sarif_level(f.severity).into()),
        );
        let mut msg: IndexMap<String, CborValue> = IndexMap::new();
        msg.insert(
            "text".into(),
            CborValue::Text(format!("{}: {}", f.kind.as_str(), f.subject.display_name)),
        );
        res.insert("message".into(), CborValue::Map(msg));
        let mut fp: IndexMap<String, CborValue> = IndexMap::new();
        fp.insert("default".into(), CborValue::Text(f.id.clone()));
        res.insert("fingerprints".into(), CborValue::Map(fp));
        results.push(CborValue::Map(res));
    }

    let mut driver: IndexMap<String, CborValue> = IndexMap::new();
    driver.insert("name".into(), CborValue::Text("al-sem-diff".into()));
    driver.insert(
        "informationUri".into(),
        CborValue::Text("https://github.com/anthropics/al-sem".into()),
    );
    driver.insert("rules".into(), CborValue::Array(rules));
    let mut tool: IndexMap<String, CborValue> = IndexMap::new();
    tool.insert("driver".into(), CborValue::Map(driver));
    let mut run: IndexMap<String, CborValue> = IndexMap::new();
    run.insert("tool".into(), CborValue::Map(tool));
    run.insert("results".into(), CborValue::Array(results));

    let mut sarif: IndexMap<String, CborValue> = IndexMap::new();
    sarif.insert(
        "$schema".into(),
        CborValue::Text(
            "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0.json".into(),
        ),
    );
    sarif.insert("version".into(), CborValue::Text("2.1.0".into()));
    sarif.insert("runs".into(), CborValue::Array(vec![CborValue::Map(run)]));

    to_sorted_json(&CborValue::Map(sarif))
}
