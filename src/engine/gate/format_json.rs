//! `format_json` — port of al-sem `src/contracts/document.ts` (`serializeDocument`),
//! `src/contracts/analyze.ts` (`wrapAnalyzeReport`), and
//! `src/cli/format-compact-json.ts` (`buildCompactReport`).
//!
//! ## Canonical serializer
//!
//! `serialize_document(value)` mirrors `serializeDocument(doc)` exactly:
//!   - Recursively sorts all object keys alphabetically.
//!   - Drops JSON null values (mirrors JS `undefined` omission: al-sem's
//!     `sortedReplacer` drops keys whose value is `undefined`; in Rust we never
//!     insert `null` for `None` fields — they are simply absent from the
//!     `serde_json::Value` we build by hand).
//!   - 2-space indent.
//!   - Single trailing `\n`.
//!
//! The A1 `serialize_detector_stats` in `registry.rs` is now a thin wrapper
//! over this canonical serializer (the stats array is a plain `Value::Array`
//! whose object elements already have sorted keys from `to_json_value`; wrapping
//! them through `serialize_document_value` is a no-op sort-wise).
//!
//! ## Envelope + payload assembly
//!
//! `build_analyze_json` assembles the full `DocumentEnvelope<"analyze-report", _>`
//! from the gate pipeline outputs and returns the serialized string (no trailing
//! newline — the caller appends one, matching al-sem's `process.stdout.write`).

use crate::engine::gate::projection::FindingSummary;
use crate::engine::l5::finding::StableEvidenceStep;
use crate::engine::l5::registry::{DetectorStats, Diagnostic};
use crate::program::model::coverage::AnalysisCoverage;

/// Per-finding opt-in evidence augmentation (only built/consumed under
/// `--with-evidence`). Aligned BY INDEX with `JsonFormatInputs.findings`.
///
/// - `evidence_path` is the stable-projected call chain (routineIds in `:`-form).
/// - `enclosing_member` / `originating_object` are the POSITION-derived member-trigger
///   discriminator (RE-1/RE-2): the member-wrapper whose range CONTAINS the finding's
///   primaryLocation start, smallest containing range. `None` when no member-trigger
///   wrapper contains the finding (object-level / procedure findings) — omitted.
pub struct FindingEvidence {
    pub evidence_path: Vec<StableEvidenceStep>,
    pub enclosing_member: Option<String>,
    pub originating_object: Option<String>,
}

// ---------------------------------------------------------------------------
// Canonical serializer (mirrors `serializeDocument` / `sortedReplacer`)
// ---------------------------------------------------------------------------

/// Recursively sort all object keys in a `serde_json::Value` alphabetically,
/// dropping any `Value::Null` entries (mirrors JS `undefined`-drop). Arrays
/// keep their order; primitives are returned as-is.
pub fn sort_and_drop_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            // Collect, drop nulls, sort by key, recurse into values.
            //
            // NULL-vs-UNDEFINED ASSUMPTION (mirrors al-sem `sortedReplacer`, which
            // drops `undefined` but KEEPS `null`): this engine NEVER builds a genuine
            // JSON `null` into the analyze payload — every optional field is ABSENT
            // (the `if let Some(..)` insert pattern) when undefined, never inserted as
            // `Value::Null`. So dropping nulls here is equivalent to JS's undefined-drop.
            // If a future field is legitimately `null`-valued (not absent-when-missing),
            // this drop would silently squelch it — revisit then.
            let mut pairs: Vec<(String, serde_json::Value)> = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, sort_and_drop_nulls(v)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let sorted: serde_json::Map<String, serde_json::Value> = pairs.into_iter().collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sort_and_drop_nulls).collect())
        }
        other => other,
    }
}

/// Canonical document serializer: recursively sorted object keys, null-drop,
/// 2-space indent, trailing `\n`.
///
/// Mirrors `serializeDocument` in al-sem `src/contracts/document.ts`.
/// This is the SINGLE canonical serializer used by both the JSON envelope and
/// the A1 `serialize_detector_stats` wrapper.
pub fn serialize_document_value(value: serde_json::Value) -> String {
    let sorted = sort_and_drop_nulls(value);
    let mut out =
        serde_json::to_string_pretty(&sorted).expect("serde_json serialization of sorted Value");
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// FindingSummary → serde_json::Value (the payload `findings[]` shape)
// ---------------------------------------------------------------------------

/// Project a `FindingLocation` to a `serde_json::Value` object (sorted key
/// insertion order is irrelevant — `sort_and_drop_nulls` will sort them).
fn location_to_value(loc: &crate::engine::gate::projection::FindingLocation) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("column".to_string(), loc.column.into());
    obj.insert("file".to_string(), loc.file.clone().into());
    obj.insert("line".to_string(), loc.line.into());
    if let Some(ref oid) = loc.object_id {
        obj.insert("objectId".to_string(), oid.clone().into());
    }
    if let Some(ref oname) = loc.object_name {
        obj.insert("objectName".to_string(), oname.clone().into());
    }
    if let Some(ref rid) = loc.routine_id {
        obj.insert("routineId".to_string(), rid.clone().into());
    }
    if let Some(ref rname) = loc.routine_name {
        obj.insert("routineName".to_string(), rname.clone().into());
    }
    serde_json::Value::Object(obj)
}

/// Like `location_to_value`, but additionally attaches the POSITION-derived
/// `enclosingMember`/`originatingObject` discriminator (RE-1/RE-2) when present.
/// Used ONLY for a finding's `primaryLocation` under `--with-evidence`; both keys
/// are OMITTED when `None` (object-level/procedure findings) so absence is honest.
fn primary_location_to_value(
    loc: &crate::engine::gate::projection::FindingLocation,
    ev: &FindingEvidence,
) -> serde_json::Value {
    let mut v = location_to_value(loc);
    if let serde_json::Value::Object(ref mut obj) = v {
        if let Some(ref member) = ev.enclosing_member {
            obj.insert("enclosingMember".to_string(), member.clone().into());
        }
        if let Some(ref oo) = ev.originating_object {
            obj.insert("originatingObject".to_string(), oo.clone().into());
        }
    }
    v
}

/// Project a `FindingSummary` to a `serde_json::Value` (the `findings[]` element
/// shape from `src/projection/finding-summary.ts` as consumed by
/// `buildCompactReport`). Key order is irrelevant — `sort_and_drop_nulls` sorts.
fn finding_summary_to_value(s: &FindingSummary, ev: Option<&FindingEvidence>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    // affectedObjects (always present, even when empty)
    let ao: Vec<serde_json::Value> = s
        .affected_objects
        .iter()
        .map(|x| x.clone().into())
        .collect();
    obj.insert("affectedObjects".to_string(), serde_json::Value::Array(ao));

    // affectedTables (always present, even when empty)
    let at: Vec<serde_json::Value> = s.affected_tables.iter().map(|x| x.clone().into()).collect();
    obj.insert("affectedTables".to_string(), serde_json::Value::Array(at));

    // confidence: { level, [cappedBy] }
    let mut conf = serde_json::Map::new();
    conf.insert("level".to_string(), s.confidence_level.clone().into());
    if let Some(ref cb) = s.confidence_capped_by {
        let arr: Vec<serde_json::Value> = cb.iter().map(|x| x.clone().into()).collect();
        conf.insert("cappedBy".to_string(), serde_json::Value::Array(arr));
    }
    obj.insert("confidence".to_string(), serde_json::Value::Object(conf));

    obj.insert("detector".to_string(), s.detector.clone().into());

    // evidencePath (opt-in, --with-evidence only) — the stable-projected call chain.
    // Always emitted under the flag (even when empty []) so the key's presence is the
    // schema signal; OMITTED entirely on the default path (ev is None).
    if let Some(e) = ev {
        let steps = serde_json::to_value(&e.evidence_path)
            .expect("StableEvidenceStep[] serializes to JSON");
        obj.insert("evidencePath".to_string(), steps);
    }

    obj.insert("fingerprint".to_string(), s.fingerprint.clone().into());

    // fixHint (optional — omit when absent)
    if let Some((ref desc, ref safety)) = s.fix_hint {
        let mut fh = serde_json::Map::new();
        fh.insert("description".to_string(), desc.clone().into());
        fh.insert("safety".to_string(), safety.clone().into());
        obj.insert("fixHint".to_string(), serde_json::Value::Object(fh));
    }

    obj.insert("id".to_string(), s.id.clone().into());

    // pathCount — always emit (even 1); `pathCount` is defined as `number`
    // (not `number | undefined`) after projectFinding always assigns it.
    obj.insert(
        "pathCount".to_string(),
        serde_json::Value::Number(s.path_count.into()),
    );

    // primaryLocation (always present). Under --with-evidence it additionally carries the
    // position-derived enclosingMember/originatingObject discriminator (omitted when None).
    obj.insert(
        "primaryLocation".to_string(),
        match ev {
            Some(e) => primary_location_to_value(&s.primary_location, e),
            None => location_to_value(&s.primary_location),
        },
    );

    obj.insert("rootCause".to_string(), s.root_cause.clone().into());
    obj.insert("severity".to_string(), s.severity.clone().into());

    // terminalLocation (optional — omit when absent)
    if let Some(ref tl) = s.terminal_location {
        obj.insert("terminalLocation".to_string(), location_to_value(tl));
    }

    obj.insert("title".to_string(), s.title.clone().into());

    serde_json::Value::Object(obj)
}

// ---------------------------------------------------------------------------
// Diagnostic projection (mirrors `projectOneDiagnostic` + `projectDiagnostics`
// from `src/contracts/snapshot.ts`)
// ---------------------------------------------------------------------------

/// Project a single `Diagnostic` to its contract shape: `{ code, severity, message }`.
/// `code = "DIAG-${d.stage}"` — no anchor/subject (those are deferred in al-sem).
///
/// Mirrors `projectOneDiagnostic` from al-sem `src/contracts/snapshot.ts`.
fn project_one_diagnostic(d: &Diagnostic) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("code".to_string(), format!("DIAG-{}", d.stage).into());
    obj.insert("message".to_string(), d.message.clone().into());
    obj.insert("severity".to_string(), d.severity.clone().into());
    serde_json::Value::Object(obj)
}

/// Project `Vec<Diagnostic>` to the contract diagnostics array.
///
/// This engine does not implement al-sem's VERSION001 cache-version diagnostic
/// (the `versionDiagnostic()` prepend some callers may expect) — diagnostics are
/// projected as-is, with no version-mismatch entry ever injected.
pub fn project_diagnostics(diags: &[Diagnostic]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = diags.iter().map(project_one_diagnostic).collect();
    serde_json::Value::Array(arr)
}

// ---------------------------------------------------------------------------
// Envelope + payload assembly (mirrors `buildCompactReport` + `wrapAnalyzeReport`
// + `makeEnvelope`)
// ---------------------------------------------------------------------------

/// The inputs the JSON formatter needs from the resolved pipeline.
pub struct JsonFormatInputs<'a> {
    /// Post-filter, post-scope, post-limit findings (pre-sorted).
    pub findings: &'a [FindingSummary],
    /// Detector diagnostics from `RunOutput`.
    pub diagnostics: &'a [Diagnostic],
    /// Per-detector stats from `RunOutput`.
    pub detector_stats: &'a [DetectorStats],
    /// Coverage from `resolved.project_coverage_disk`.
    pub coverage: &'a AnalysisCoverage,
    /// `--deterministic` flag (pins `generatedAt`).
    pub deterministic: bool,
    /// Effective driver version (from `driver_version()`).
    pub driver_version: String,
    /// Opt-in `--with-evidence` augmentation. `None` ⇒ default path: NO `evidencePath`,
    /// NO `enclosingMember`/`originatingObject`, `schemaVersion "1.0.0"` (byte-identical
    /// to today). `Some(slice)` ⇒ per-finding evidence aligned BY INDEX with `findings`,
    /// emitted on each finding (+ its `primaryLocation`), and `schemaVersion "1.1.0"`.
    pub evidence: Option<&'a [FindingEvidence]>,
}

/// Build the `DocumentEnvelope<"analyze-report", _>` as a `serde_json::Value`,
/// then serialize it with `serialize_document_value`.
///
/// Returns the serialized string WITHOUT the trailing newline (the caller appends
/// one, matching al-sem's `process.stdout.write`).
pub fn build_analyze_json(inputs: &JsonFormatInputs<'_>) -> String {
    // --- payload.summary.bySeverity + byDetector ---
    let mut by_severity: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut by_detector: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for f in inputs.findings {
        let sev_count = by_severity
            .entry(f.severity.clone())
            .or_insert(serde_json::Value::Number(0u64.into()));
        if let Some(n) = sev_count.as_u64() {
            *sev_count = (n + 1).into();
        }
        let det_count = by_detector
            .entry(f.detector.clone())
            .or_insert(serde_json::Value::Number(0u64.into()));
        if let Some(n) = det_count.as_u64() {
            *det_count = (n + 1).into();
        }
    }

    // --- payload.summary.detectorStats ---
    let stats_arr: Vec<serde_json::Value> = inputs
        .detector_stats
        .iter()
        .map(|s| s.to_json_value())
        .collect();

    // --- payload.summary.opaqueApps ---
    let opaque_arr: Vec<serde_json::Value> = inputs
        .coverage
        .opaque_apps
        .iter()
        .map(|s| s.clone().into())
        .collect();

    // --- payload.summary ---
    let mut summary = serde_json::Map::new();
    summary.insert(
        "byDetector".to_string(),
        serde_json::Value::Object(by_detector),
    );
    summary.insert(
        "bySeverity".to_string(),
        serde_json::Value::Object(by_severity),
    );
    summary.insert(
        "detectorStats".to_string(),
        serde_json::Value::Array(stats_arr),
    );
    summary.insert(
        "opaqueApps".to_string(),
        serde_json::Value::Array(opaque_arr),
    );
    summary.insert(
        "routinesAnalyzed".to_string(),
        inputs.coverage.routines_total.into(),
    );
    summary.insert(
        "sourceUnitsParsed".to_string(),
        inputs.coverage.source_units_parsed.into(),
    );
    summary.insert("totalFindings".to_string(), inputs.findings.len().into());

    // --- payload.findings ---
    // Under --with-evidence (`inputs.evidence` Some), each finding is paired BY INDEX with
    // its FindingEvidence; on the default path (None) evidence is None for every finding so
    // no new keys are emitted (byte-identical to today).
    let findings_arr: Vec<serde_json::Value> = inputs
        .findings
        .iter()
        .enumerate()
        .map(|(i, f)| finding_summary_to_value(f, inputs.evidence.and_then(|e| e.get(i))))
        .collect();

    // --- payload ---
    let mut payload = serde_json::Map::new();
    payload.insert(
        "findings".to_string(),
        serde_json::Value::Array(findings_arr),
    );
    payload.insert("summary".to_string(), serde_json::Value::Object(summary));

    // --- envelope ---
    // Pinned `"1970-01-01T00:00:00Z"` (deterministic) or a live 24-byte
    // `toISOString()`-shaped timestamp. Shared with the cli-b snapshot envelope.
    let generated_at = pinned_or_now_iso8601(inputs.deterministic);

    let mut envelope = serde_json::Map::new();
    envelope.insert(
        "alsemVersion".to_string(),
        inputs.driver_version.clone().into(),
    );
    envelope.insert("deterministic".to_string(), inputs.deterministic.into());
    envelope.insert(
        "diagnostics".to_string(),
        project_diagnostics(inputs.diagnostics),
    );
    envelope.insert("generatedAt".to_string(), generated_at.into());
    envelope.insert("kind".to_string(), "analyze-report".into());
    envelope.insert("payload".to_string(), serde_json::Value::Object(payload));
    // RE-8: default (no --with-evidence) keeps "1.0.0" (the differential asserts it);
    // under the flag emit "1.1.0". The no-flag output stays byte-identical.
    let schema_version = if inputs.evidence.is_some() {
        "1.1.0"
    } else {
        "1.0.0"
    };
    envelope.insert("schemaVersion".to_string(), schema_version.into());

    // Serialize (sort all keys, drop nulls, 2-space indent, trailing newline).
    // Strip the trailing newline — the caller appends it.
    let serialized = serialize_document_value(serde_json::Value::Object(envelope));
    // serialize_document_value always appends '\n' — strip it for the caller.
    serialized
        .strip_suffix('\n')
        .unwrap_or(&serialized)
        .to_string()
}

/// Approximate Gregorian calendar (no leap second, but correct for Gregorian
/// leap years). Only used on the non-deterministic code path (tests pin to
/// UNIX epoch via `--deterministic`).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian 400-year cycle = 146097 days.
    let mut year = 1970u64;
    loop {
        let leap = is_leap(year);
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let months = [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &dm in &months {
        let dm2 = if month == 2 && is_leap(year) { 29 } else { dm };
        if days < dm2 {
            break;
        }
        days -= dm2;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// The envelope `generatedAt` value for a given determinism mode. `deterministic`
/// → the LITERAL pinned `"1970-01-01T00:00:00Z"` (20 bytes, NO milliseconds — the
/// form the golden corpus pins, NOT `toISOString()` of epoch 0). Non-deterministic
/// → a live 24-byte `YYYY-MM-DDTHH:mm:ss.sssZ` timestamp matching JS
/// `new Date().toISOString()`. Shared by the analyze JSON envelope and the cli-b
/// capability-snapshot envelope so both honor `--deterministic` identically.
pub fn pinned_or_now_iso8601(deterministic: bool) -> String {
    if deterministic {
        "1970-01-01T00:00:00Z".to_string()
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        let ms = now.subsec_millis();
        let s = secs % 60;
        let m = (secs / 60) % 60;
        let h = (secs / 3600) % 24;
        let days = secs / 86400;
        let (y, mo, d) = days_to_ymd(days);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_keys_basic() {
        let v = serde_json::json!({ "z": 1, "a": 2, "m": 3 });
        let out = serialize_document_value(v);
        // Keys must be in alphabetical order.
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "m", "z"]);
    }

    #[test]
    fn null_dropped() {
        let v = serde_json::json!({ "a": 1, "b": null, "c": "x" });
        let out = serialize_document_value(v);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let obj = parsed.as_object().unwrap();
        assert!(obj.contains_key("a"), "a must be present");
        assert!(!obj.contains_key("b"), "b (null) must be dropped");
        assert!(obj.contains_key("c"), "c must be present");
    }

    #[test]
    fn empty_object_serializes_as_braces() {
        let v = serde_json::json!({});
        let out = serialize_document_value(v);
        assert_eq!(out.trim(), "{}");
    }

    #[test]
    fn arrays_keep_order() {
        let v = serde_json::json!([3, 1, 2]);
        let out = serialize_document_value(v);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let arr: Vec<u64> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect();
        assert_eq!(arr, vec![3, 1, 2]);
    }

    #[test]
    fn trailing_newline() {
        let v = serde_json::json!({});
        let out = serialize_document_value(v);
        assert!(out.ends_with('\n'), "must end with \\n");
    }

    #[test]
    fn nested_object_keys_sorted() {
        let v = serde_json::json!({ "z": { "y": 1, "a": 2 }, "a": { "z": 3, "b": 4 } });
        let out = serialize_document_value(v);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let outer_keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(outer_keys, vec!["a", "z"]);
        let inner_a: Vec<&str> = parsed["a"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(inner_a, vec!["b", "z"]);
    }

    #[test]
    fn two_space_indent() {
        let v = serde_json::json!({ "a": 1 });
        let out = serialize_document_value(v);
        // serde_json pretty with 2-space indent produces `{\n  "a": 1\n}\n`.
        assert!(out.contains("  \"a\""), "must use 2-space indent");
    }
}
