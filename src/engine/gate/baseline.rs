//! Baseline file load / save / apply — faithful port of al-sem `src/cli/baseline.ts`.
//!
//! `BaselineFile = { schemaVersion: "1", generatedAt, fingerprints: string[] }`.
//!   - `load_baseline(path)` → `Ok(set)` of fingerprints on success;
//!     `Ok(empty)` when the file does not exist (mirrors al-sem `existsSync` guard);
//!     `Err(msg)` when the file EXISTS but is malformed JSON OR has a non-array
//!     `fingerprints` field (mirrors al-sem's bare `JSON.parse` + `new Set(parsed.fingerprints)`
//!     which throws on malformed input — propagated to the CLI as ANALYSIS_FAILURE, exit 2).
//!   - `apply_baseline(findings, set)` → keep findings whose fingerprint is NOT in the set.
//!   - `serialize_baseline(findings)` → the bytes `--update-baseline` writes:
//!     `{ schemaVersion: "1", generatedAt: "1970-01-01T00:00:00.000Z",
//!     fingerprints: sorted+deduped }`. The bin writes them (#277 D2.4).
//!
//! Byte-stability: `generatedAt` is pinned to the Unix epoch (`new Date(0).toISOString()`),
//! fingerprints are sorted (ascending, lexicographic — matching JS `Array.prototype.sort`
//! over hex-string fingerprints) and deduped. The serialization mirrors
//! `JSON.stringify(file, null, 2) + "\n"` exactly: 2-space indent, `": "` after keys,
//! and a trailing newline.

use std::collections::BTreeSet;
use std::path::Path;

use crate::engine::gate::projection::FindingSummary;
use crate::program::inputs::FileInput;

/// The pinned `generatedAt` value — `new Date(0).toISOString()`.
pub const EPOCH_GENERATED_AT: &str = "1970-01-01T00:00:00.000Z";

/// Load a baseline file into a set of fingerprints.
///
/// - **Missing file** (`io::ErrorKind::NotFound`): returns `Ok(empty set)` — mirrors
///   al-sem's `if (!existsSync(path)) return new Set()` (no error, graceful empty).
/// - **Malformed JSON** or **non-array `fingerprints` field**: returns `Err(message)` —
///   mirrors al-sem's bare `JSON.parse(readFileSync(...))` + `new Set(parsed.fingerprints)`
///   which throw a `SyntaxError` / `TypeError` propagated to the CLI catch block as
///   `EXIT.ANALYSIS_FAILURE` (exit 2) with `al-sem: analysis failure — <message>` to stderr.
///   The Rust caller must map `Err` to the same exit-2 path and emit NO report.
/// - **Other I/O errors** (unreadable / permission denied): also returns `Err(message)`.
pub fn load_baseline(path: &Path) -> Result<BTreeSet<String>, String> {
    load_baseline_bytes(&FileInput {
        path: path.to_path_buf(),
        bytes: std::fs::read(path),
    })
}

/// [`load_baseline`] over a file already read (the command's `RequestInputs`, #160).
/// Reads [`FileInput::text`], so a non-UTF-8 file is still a READ error
/// (`cannot read baseline`), never `is not valid JSON`.
pub fn load_baseline_bytes(file: &FileInput) -> Result<BTreeSet<String>, String> {
    let path = &file.path;
    let text = match file.text() {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing file → graceful empty set, no error (mirrors al-sem existsSync guard).
            return Ok(BTreeSet::new());
        }
        Err(e) => return Err(format!("cannot read baseline '{}': {e}", path.display())),
    };
    let v = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|e| format!("baseline '{}' is not valid JSON: {e}", path.display()))?;
    let arr = v
        .get("fingerprints")
        .and_then(|f| f.as_array())
        .ok_or_else(|| {
            format!(
                "baseline '{}': 'fingerprints' is missing or not an array",
                path.display()
            )
        })?;
    Ok(arr
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect())
}

/// Return only findings whose fingerprint is NOT in the baseline. Order-preserving.
pub fn apply_baseline(
    findings: &[FindingSummary],
    baseline: &BTreeSet<String>,
) -> Vec<FindingSummary> {
    findings
        .iter()
        .filter(|f| !baseline.contains(&f.fingerprint))
        .cloned()
        .collect()
}

/// Serialize a `BaselineFile` from the given findings: sorted + deduped fingerprints,
/// epoch `generatedAt`. The exact bytes `--update-baseline` writes (`run_analyze_with_exit`
/// returns them; the bin writes them). Matches
/// `JSON.stringify(file, null, 2) + "\n"`.
pub fn serialize_baseline(findings: &[FindingSummary]) -> String {
    // BTreeSet gives sorted + deduped, matching `[...new Set(...)].sort()` over hex
    // fingerprint strings (ASCII lexicographic ascending — identical to JS default sort).
    let fingerprints: BTreeSet<&str> = findings.iter().map(|f| f.fingerprint.as_str()).collect();

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"schemaVersion\": \"1\",\n");
    out.push_str(&format!("  \"generatedAt\": \"{EPOCH_GENERATED_AT}\",\n"));
    if fingerprints.is_empty() {
        out.push_str("  \"fingerprints\": []\n");
    } else {
        out.push_str("  \"fingerprints\": [\n");
        let n = fingerprints.len();
        for (i, fp) in fingerprints.iter().enumerate() {
            let comma = if i + 1 < n { "," } else { "" };
            // Fingerprints are hex strings — no JSON escaping needed, but go through
            // serde_json::to_string to be safe against any unexpected character.
            let encoded = serde_json::to_string(fp).expect("string encodes");
            out.push_str(&format!("    {encoded}{comma}\n"));
        }
        out.push_str("  ]\n");
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gate::projection::FindingLocation;

    fn mk(fp: &str) -> FindingSummary {
        FindingSummary {
            id: fp.to_string(),
            fingerprint: fp.to_string(),
            loose_fingerprint: String::new(),
            detector: "d".to_string(),
            title: String::new(),
            root_cause: String::new(),
            severity: "high".to_string(),
            confidence_level: "high".to_string(),
            confidence_capped_by: None,
            primary_location: FindingLocation {
                file: "ws:src/A.al".to_string(),
                line: 1,
                column: 1,
                object_id: None,
                object_name: None,
                routine_id: None,
                routine_name: None,
            },
            terminal_location: None,
            affected_objects: vec![],
            affected_tables: vec![],
            path_count: 1,
            fix_hint: None,
        }
    }

    #[test]
    fn serialize_sorts_and_dedups() {
        // Two distinct fingerprints, out of order + a dup; expect sorted ascending, deduped.
        let findings = vec![
            mk("dc5877f64fbaacd7"),
            mk("1bc4584bf1de491e"),
            mk("1bc4584bf1de491e"),
        ];
        let s = serialize_baseline(&findings);
        let expected = "{\n  \"schemaVersion\": \"1\",\n  \"generatedAt\": \"1970-01-01T00:00:00.000Z\",\n  \"fingerprints\": [\n    \"1bc4584bf1de491e\",\n    \"dc5877f64fbaacd7\"\n  ]\n}\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn empty_findings_yield_empty_array() {
        let s = serialize_baseline(&[]);
        assert!(s.contains("\"fingerprints\": []"));
    }

    #[test]
    fn apply_baseline_keeps_unbaselined() {
        let findings = vec![mk("aaa"), mk("bbb")];
        let baseline: BTreeSet<String> = ["aaa".to_string()].into_iter().collect();
        let kept = apply_baseline(&findings, &baseline);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].fingerprint, "bbb");
    }

    // -------------------------------------------------------------------------
    // Fix #3 oracles — load_baseline error semantics
    // -------------------------------------------------------------------------

    /// Missing baseline file → `Ok(empty set)` — mirrors al-sem `existsSync` guard.
    /// al-sem: `if (!existsSync(path)) return new Set()` — no throw, no error.
    #[test]
    fn fix3_missing_file_is_ok_empty() {
        let tmp = std::env::temp_dir().join(format!(
            "alsem-baseline-missing-{}-{}.json",
            std::process::id(),
            "fix3a"
        ));
        // Ensure it does NOT exist.
        let _ = std::fs::remove_file(&tmp);
        let result = load_baseline(&tmp);
        assert!(
            result.is_ok(),
            "missing baseline file must be Ok(empty), got Err: {:?}",
            result.err()
        );
        assert!(
            result.unwrap().is_empty(),
            "missing baseline file must yield empty set"
        );
    }

    /// Malformed JSON → `Err(...)` — mirrors al-sem's bare `JSON.parse` throwing a
    /// `SyntaxError`, caught by the CLI analyze-action catch → EXIT.ANALYSIS_FAILURE (2).
    #[test]
    fn fix3_malformed_json_is_err() {
        let tmp = std::env::temp_dir().join(format!(
            "alsem-baseline-malformed-{}-{}.json",
            std::process::id(),
            "fix3b"
        ));
        std::fs::write(&tmp, b"this is not json {{{").unwrap();
        let result = load_baseline(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert!(
            result.is_err(),
            "malformed JSON baseline must return Err, got Ok"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not valid JSON"),
            "error message should mention 'not valid JSON', got: {msg}"
        );
    }

    /// `fingerprints` field missing → `Err(...)` — mirrors al-sem's `new Set(undefined)`
    /// which succeeds (empty set), but a completely absent field OR non-array type is an
    /// integrity error.  al-sem would silently succeed for undefined→empty-Set; however
    /// a non-array value (e.g. a number) makes `new Set(123)` throw TypeError.
    /// We test with a number value (non-array) → must be `Err`.
    #[test]
    fn fix3_non_array_fingerprints_is_err() {
        let tmp = std::env::temp_dir().join(format!(
            "alsem-baseline-nonarray-{}-{}.json",
            std::process::id(),
            "fix3c"
        ));
        std::fs::write(
            &tmp,
            br#"{"schemaVersion":"1","generatedAt":"1970-01-01T00:00:00.000Z","fingerprints":42}"#,
        )
        .unwrap();
        let result = load_baseline(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert!(
            result.is_err(),
            "non-array fingerprints must return Err, got Ok"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not an array"),
            "error message should mention 'not an array', got: {msg}"
        );
    }
}
