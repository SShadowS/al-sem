//! Baseline file load / apply / update — first a port of al-sem `src/cli/baseline.ts`;
//! since TLS-106 a schema-2 file that an update merges into instead of replacing.
//!
//! Schema 2 (`--update-baseline` writes it):
//! `{ schemaVersion: "2", generatedAt, universe, primaryApp, opaqueApps: string[],
//! fingerprints: string[], entries: [{ fingerprint, detector, scope }] }`.
//!   - `fingerprints` is what suppression reads ([`load_baseline`]), so a schema-1
//!     reader still works. A fingerprint listed with no entry is a schema-1 entry (no
//!     detector known): a schema-1 file, a hand edit, an old binary's rewrite.
//!   - `entries` says which detector produced each fingerprint and whether it was in
//!     the primary scope or a dependency's, so an update can tell "fixed" from "not
//!     looked at" ([`update_baseline`], the rule of TLS-106 §3).
//!   - `universe` (`cross-app` / `single-app`) and `primaryApp` bind the file to the
//!     runs that may update it; an empty `primaryApp` is not stamped yet.
//!   - `opaqueApps` lists the symbol-only dependency apps the file's runs had. A run
//!     with an opaque app the file does not list lost bodies it may have had: it
//!     prunes nothing (its inputs are not whole).
//!
//! Versions: `"1"`, a missing `schemaVersion` or a numeric one are schema 1 (a file
//! of fingerprints only); `"2"` is schema 2; any other string is a schema this alsem
//! does not know, refused by a plain read AND an update (fail closed: a later schema
//! may give `fingerprints` another meaning).
//!
//! Plain reads ([`load_baseline`]): a missing file is an empty set; a file that is
//! not JSON, has no `fingerprints` array or an unknown version is an error (exit 2).
//!
//! Byte-stability: `generatedAt` is pinned to the Unix epoch, lists are sorted
//! (byte order) and deduped, and the text mirrors `JSON.stringify(file, null, 2) +
//! "\n"`: 2-space indent, `": "` after keys, a trailing newline.

use std::collections::{BTreeMap, BTreeSet};
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
    if let Some(version) = unknown_version(&v) {
        return Err(format!(
            "baseline '{}': schemaVersion '{version}' is not one this alsem knows (1 or 2)",
            path.display()
        ));
    }
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

/// A `schemaVersion` string other than `"1"` and `"2"`. A missing or numeric
/// version is schema 1 (TLS-106: such files were read before).
fn unknown_version(v: &serde_json::Value) -> Option<&str> {
    v.get("schemaVersion")
        .and_then(|x| x.as_str())
        .filter(|s| *s != "1" && *s != "2")
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

/// A finding's scope: in the primary app's scope, or a dependency's (by the
/// registry's own predicate, `RunOutput::finding_is_primary`). Ordered as the
/// words sort, so entries sort by their text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryScope {
    Dependency,
    Primary,
}

impl EntryScope {
    pub fn of(primary: bool) -> Self {
        if primary {
            EntryScope::Primary
        } else {
            EntryScope::Dependency
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EntryScope::Dependency => "dependency",
            EntryScope::Primary => "primary",
        }
    }
}

/// One schema-2 entry. Sorted by (fingerprint, detector, scope).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub fingerprint: String,
    pub detector: String,
    pub scope: EntryScope,
}

/// Why an update kept an entry it could not check (TLS-106 §3). Every kept
/// entry is counted under one reason: the first that applies, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeptUnchecked {
    /// A schema-1 fingerprint (no entry) that no run has seen yet.
    NoDetectorKnown,
    /// Its detector is no longer registered (removed or renamed).
    DetectorNotRegistered,
    /// Its detector was not selected (`--detector`, `--preset`, opt-in).
    DetectorNotRun,
    /// Its detector errored or reported an incomplete result.
    DetectorIncomplete,
    /// A dependency entry, and the run did not look at dependency findings.
    OutOfScope,
    /// The run's inputs were not whole (`preflight::inputs_whole`).
    InputsNotWhole,
}

impl KeptUnchecked {
    /// The `keptUnchecked` key in `payload.summary.baselineUpdate`.
    pub fn key(self) -> &'static str {
        match self {
            KeptUnchecked::NoDetectorKnown => "noDetectorKnown",
            KeptUnchecked::DetectorNotRegistered => "detectorNotRegistered",
            KeptUnchecked::DetectorNotRun => "detectorNotRun",
            KeptUnchecked::DetectorIncomplete => "detectorIncomplete",
            KeptUnchecked::OutOfScope => "outOfScope",
            KeptUnchecked::InputsNotWhole => "inputsNotWhole",
        }
    }

    /// The words on stderr.
    pub fn words(self) -> &'static str {
        match self {
            KeptUnchecked::NoDetectorKnown => "no detector known",
            KeptUnchecked::DetectorNotRegistered => "detector no longer registered",
            KeptUnchecked::DetectorNotRun => "detector not run",
            KeptUnchecked::DetectorIncomplete => "detector incomplete",
            KeptUnchecked::OutOfScope => "out of scope",
            KeptUnchecked::InputsNotWhole => "inputs not whole",
        }
    }
}

/// What an update knows about its run. Hand-statable, so the rule is tested on
/// stated inputs.
pub struct UpdateRun {
    /// `cross-app` or `single-app`.
    pub universe: &'static str,
    /// The primary app's id (empty: none known).
    pub primary_app: String,
    /// The run saw dependency findings: a cross-app `--scope all` run.
    pub prunes_dependency: bool,
    /// `preflight::inputs_whole`.
    pub inputs_whole: bool,
    /// The run's symbol-only dependency apps (`FreshCoverage::opaque_apps`).
    pub opaque_apps: BTreeSet<String>,
    /// Every finding the detectors emitted: before the registry's scope tail
    /// (`RunOutput::scoped_out` included) and before the gate's
    /// `--min-severity`, detector allow-list, scope filter and `--limit`.
    pub before_filters: Vec<Entry>,
    /// The findings the run shows (after its filters): what an update adds.
    pub shown: Vec<Entry>,
    /// The detectors this run selected.
    pub selected: BTreeSet<String>,
    /// The selected detectors that ran completely (`RunOutput::ran_completely`).
    pub complete: BTreeSet<String>,
    /// Every registered detector.
    pub registered: BTreeSet<String>,
}

/// Why an update refused, writing nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Unreadable, not JSON, an unknown `schemaVersion`, a wrong shape, or an
    /// entry whose fingerprint is not listed (exit 2).
    Malformed(String),
    /// The file binds another universe or primary app (exit 3).
    OtherRun(String),
}

/// The result of an update: the bytes to write and the counts it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineUpdate {
    pub bytes: Vec<u8>,
    /// Fingerprints in the written file.
    pub written: usize,
    /// Old fingerprints the file no longer lists.
    pub dropped: usize,
    /// Fingerprints the file did not list before.
    pub added: usize,
    /// Old entries kept unchecked, by reason (a schema-1 fingerprint counts as one).
    pub kept_unchecked: BTreeMap<KeptUnchecked, usize>,
}

impl BaselineUpdate {
    /// `al-sem: baseline: kept N entries unchecked: <n> <reason>, ...`, or `None`
    /// when every old entry was checked.
    pub fn stderr_line(&self) -> Option<String> {
        let total: usize = self.kept_unchecked.values().sum();
        if total == 0 {
            return None;
        }
        let parts: Vec<String> = self
            .kept_unchecked
            .iter()
            .map(|(r, n)| format!("{n} {}", r.words()))
            .collect();
        Some(format!(
            "al-sem: baseline: kept {total} entries unchecked: {}",
            parts.join(", ")
        ))
    }

    /// `payload.summary.baselineUpdate`.
    pub fn to_json(&self) -> serde_json::Value {
        let kept: serde_json::Map<String, serde_json::Value> = self
            .kept_unchecked
            .iter()
            .map(|(r, n)| (r.key().to_string(), (*n).into()))
            .collect();
        serde_json::json!({
            "written": self.written,
            "dropped": self.dropped,
            "added": self.added,
            "keptUnchecked": kept,
        })
    }
}

/// What a schema-2 file binds.
struct Stamp {
    universe: String,
    /// Empty: not stamped yet.
    primary_app: String,
    opaque_apps: BTreeSet<String>,
}

/// The old file, as an update reads it.
struct OldFile {
    /// `None` for schema 1 or no file.
    stamp: Option<Stamp>,
    fingerprints: BTreeSet<String>,
    /// Sorted and deduped.
    entries: Vec<Entry>,
}

/// Check the file an update will merge into, before any analysis (TLS-106):
/// `Err` (exit 2, nothing written) when it is unreadable, not JSON, of an
/// unknown schema, malformed or inconsistent. A missing file passes.
pub fn check_update_file(file: &FileInput) -> Result<(), String> {
    match read_old(file) {
        Ok(_) => Ok(()),
        Err(Refusal::Malformed(m) | Refusal::OtherRun(m)) => Err(m),
    }
}

fn read_old(file: &FileInput) -> Result<OldFile, Refusal> {
    let path = file.path.display();
    let bad =
        |why: String| Refusal::Malformed(format!("baseline '{path}': {why}; nothing written"));
    let text = match file.text() {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OldFile {
                stamp: None,
                fingerprints: BTreeSet::new(),
                entries: Vec::new(),
            });
        }
        Err(e) => return Err(bad(format!("cannot read it: {e}"))),
    };
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| bad(format!("not valid JSON: {e}")))?;
    let text_field = |o: &serde_json::Value, key: &str| -> Result<String, Refusal> {
        o.get(key)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .ok_or_else(|| bad(format!("'{key}' is missing or not a string")))
    };
    let strings = |key: &str| -> Result<BTreeSet<String>, Refusal> {
        v.get(key)
            .and_then(|a| a.as_array())
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
            .ok_or_else(|| bad(format!("'{key}' is missing or not an array of strings")))
    };
    let fingerprints = strings("fingerprints")?;
    let version = v.get("schemaVersion");
    match version.and_then(|x| x.as_str()) {
        // Missing or numeric: schema 1, as before (TLS-106 review).
        _ if version.is_none_or(serde_json::Value::is_number) => Ok(OldFile {
            stamp: None,
            fingerprints,
            entries: Vec::new(),
        }),
        Some("1") => Ok(OldFile {
            stamp: None,
            fingerprints,
            entries: Vec::new(),
        }),
        Some("2") => {
            let universe = text_field(&v, "universe")?;
            if universe != "cross-app" && universe != "single-app" {
                return Err(bad(format!("unknown universe '{universe}'")));
            }
            let primary_app = text_field(&v, "primaryApp")?;
            let opaque_apps = strings("opaqueApps")?;
            let rows = v
                .get("entries")
                .and_then(|e| e.as_array())
                .ok_or_else(|| bad("'entries' is missing or not an array".to_string()))?;
            let mut entries = Vec::with_capacity(rows.len());
            for row in rows {
                let fingerprint = text_field(row, "fingerprint")?;
                let scope = match text_field(row, "scope")?.as_str() {
                    "primary" => EntryScope::Primary,
                    "dependency" => EntryScope::Dependency,
                    other => return Err(bad(format!("unknown entry scope '{other}'"))),
                };
                if !fingerprints.contains(&fingerprint) {
                    return Err(bad(format!(
                        "inconsistent: the entry for {fingerprint} is not in 'fingerprints'"
                    )));
                }
                entries.push(Entry {
                    fingerprint,
                    detector: text_field(row, "detector")?,
                    scope,
                });
            }
            entries.sort();
            entries.dedup();
            Ok(OldFile {
                stamp: Some(Stamp {
                    universe,
                    primary_app,
                    opaque_apps,
                }),
                fingerprints,
                entries,
            })
        }
        Some(other) => Err(bad(format!(
            "schemaVersion '{other}' is not one this alsem knows (1 or 2)"
        ))),
        None => Err(bad(
            "'schemaVersion' is neither a string nor a number".to_string()
        )),
    }
}

/// Why `entry`, which this run did not report, must be kept unchecked, or
/// `None` when the run proves it gone. `inputs_whole` is the run's, also
/// false when the run has an opaque app the file did not record.
fn unchecked_reason(entry: &Entry, run: &UpdateRun, inputs_whole: bool) -> Option<KeptUnchecked> {
    if !run.registered.contains(&entry.detector) {
        Some(KeptUnchecked::DetectorNotRegistered)
    } else if !run.selected.contains(&entry.detector) {
        Some(KeptUnchecked::DetectorNotRun)
    } else if !run.complete.contains(&entry.detector) {
        Some(KeptUnchecked::DetectorIncomplete)
    } else if entry.scope == EntryScope::Dependency && !run.prunes_dependency {
        Some(KeptUnchecked::OutOfScope)
    } else if !inputs_whole {
        Some(KeptUnchecked::InputsNotWhole)
    } else {
        None
    }
}

/// `--update-baseline` (TLS-106 §3): merge this run into the old file.
///
/// - **Drop** an old entry only when its fingerprint is absent from the run's
///   findings before every filter, its detector ran completely, the entry is in
///   the run's scope and the run's inputs were whole.
/// - **Keep** every other old entry as it was; a schema-1 fingerprint the run
///   saw gets the entries of the findings that carry it.
/// - **Add** the findings the run shows.
///
/// Refuses (writing nothing) a malformed or inconsistent file, an unknown
/// schema, and a file bound to another universe or primary app. A missing
/// file is a new one.
pub fn update_baseline(old: &FileInput, run: &UpdateRun) -> Result<BaselineUpdate, Refusal> {
    let old = read_old(old)?;
    // An empty primary app on either side is "not stamped": never a refusal.
    if let Some(s) = &old.stamp
        && (s.universe != run.universe
            || (!s.primary_app.is_empty()
                && !run.primary_app.is_empty()
                && s.primary_app != run.primary_app))
    {
        return Err(Refusal::OtherRun(format!(
            "baseline was written by a {} run of app {}; this is a {} run of app {}; \
             nothing written",
            s.universe, s.primary_app, run.universe, run.primary_app
        )));
    }
    let primary_app = match &old.stamp {
        Some(s) if run.primary_app.is_empty() => s.primary_app.clone(),
        _ => run.primary_app.clone(),
    };
    // Opacity the file already accepted stays allowed; an app opaque now that
    // the file did not record lost its bodies: nothing absent is proved gone.
    // The file keeps only the opacity every one of its runs had.
    let (inputs_whole, opaque_apps) = match &old.stamp {
        Some(s) => (
            run.inputs_whole && run.opaque_apps.is_subset(&s.opaque_apps),
            s.opaque_apps
                .intersection(&run.opaque_apps)
                .cloned()
                .collect(),
        ),
        None => (run.inputs_whole, run.opaque_apps.clone()),
    };
    let mut seen: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in &run.before_filters {
        seen.entry(e.fingerprint.as_str()).or_default().push(e);
    }
    let mut old_by_fp: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in &old.entries {
        old_by_fp.entry(e.fingerprint.as_str()).or_default().push(e);
    }
    let mut kept_unchecked: BTreeMap<KeptUnchecked, usize> = BTreeMap::new();
    let mut fingerprints: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<Entry> = Vec::new();
    for fp in &old.fingerprints {
        match (seen.get(fp.as_str()), old_by_fp.get(fp.as_str())) {
            // Seen: the run's own entries (detector and scope) replace the old.
            (Some(found), _) => entries.extend(found.iter().map(|e| (*e).clone())),
            // A schema-1 fingerprint no run has seen.
            (None, None) => {
                fingerprints.insert(fp.clone());
                *kept_unchecked
                    .entry(KeptUnchecked::NoDetectorKnown)
                    .or_default() += 1;
            }
            (None, Some(rows)) => {
                for e in rows {
                    if let Some(reason) = unchecked_reason(e, run, inputs_whole) {
                        *kept_unchecked.entry(reason).or_default() += 1;
                        entries.push((*e).clone());
                    }
                }
            }
        }
    }
    entries.extend(run.shown.iter().cloned());
    entries.sort();
    entries.dedup();
    fingerprints.extend(entries.iter().map(|e| e.fingerprint.clone()));
    let written = fingerprints.len();
    let dropped = old.fingerprints.difference(&fingerprints).count();
    let added = fingerprints.difference(&old.fingerprints).count();
    let header = Header {
        universe: run.universe,
        primary_app: &primary_app,
        opaque_apps: &opaque_apps,
    };
    Ok(BaselineUpdate {
        bytes: serialize_v2(&header, &fingerprints, &entries).into_bytes(),
        written,
        dropped,
        added,
        kept_unchecked,
    })
}

/// The schema-2 header an update writes.
struct Header<'a> {
    universe: &'a str,
    primary_app: &'a str,
    opaque_apps: &'a BTreeSet<String>,
}

/// `  "<key>": [ ... ],` (2-space JSON), a sorted list of strings.
fn push_string_list<'a>(out: &mut String, key: &str, items: impl Iterator<Item = &'a String>) {
    let rows: Vec<String> = items.map(|s| format!("    {}", json_str(s))).collect();
    if rows.is_empty() {
        out.push_str(&format!("  \"{key}\": [],\n"));
    } else {
        out.push_str(&format!("  \"{key}\": [\n{}\n  ],\n", rows.join(",\n")));
    }
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("string encodes")
}

/// The schema-2 text: `JSON.stringify(file, null, 2) + "\n"`.
fn serialize_v2(header: &Header, fingerprints: &BTreeSet<String>, entries: &[Entry]) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"schemaVersion\": \"2\",\n");
    out.push_str(&format!("  \"generatedAt\": \"{EPOCH_GENERATED_AT}\",\n"));
    out.push_str(&format!("  \"universe\": {},\n", json_str(header.universe)));
    out.push_str(&format!(
        "  \"primaryApp\": {},\n",
        json_str(header.primary_app)
    ));
    push_string_list(&mut out, "opaqueApps", header.opaque_apps.iter());
    push_string_list(&mut out, "fingerprints", fingerprints.iter());
    if entries.is_empty() {
        out.push_str("  \"entries\": []\n");
    } else {
        let rows: Vec<String> = entries
            .iter()
            .map(|e| {
                format!(
                    "    {{\n      \"fingerprint\": {},\n      \"detector\": {},\n      \
                     \"scope\": \"{}\"\n    }}",
                    json_str(&e.fingerprint),
                    json_str(&e.detector),
                    e.scope.as_str()
                )
            })
            .collect();
        out.push_str(&format!("  \"entries\": [\n{}\n  ]\n", rows.join(",\n")));
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

    // -------------------------------------------------------------------------
    // TLS-106 — the update rule, on hand-stated runs and files
    // -------------------------------------------------------------------------

    const D1: &str = "d1-db-op-in-loop";
    const D3: &str = "d3-missing-setloadfields";
    const APP: &str = "11111111-1111-1111-1111-111111111111";

    fn e(fp: &str, detector: &str, scope: EntryScope) -> Entry {
        Entry {
            fingerprint: fp.to_string(),
            detector: detector.to_string(),
            scope,
        }
    }

    fn p(fp: &str, detector: &str) -> Entry {
        e(fp, detector, EntryScope::Primary)
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A clean, complete, primary-scope cross-app run of D1 and D3 that found nothing.
    fn run() -> UpdateRun {
        UpdateRun {
            universe: "cross-app",
            primary_app: APP.to_string(),
            prunes_dependency: false,
            inputs_whole: true,
            opaque_apps: set(&["Opaque Dep"]),
            before_filters: vec![],
            shown: vec![],
            selected: set(&[D1, D3]),
            complete: set(&[D1, D3]),
            registered: set(&[D1, D3]),
        }
    }

    fn file(text: Option<&str>) -> FileInput {
        FileInput {
            path: "b.json".into(),
            bytes: match text {
                Some(t) => Ok(t.as_bytes().to_vec()),
                None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            },
        }
    }

    /// The schema-2 bytes of `entries`, with `extra` fingerprints that have no entry.
    fn v2(entries: &[Entry], extra: &[&str]) -> String {
        let mut fps: BTreeSet<String> = entries.iter().map(|e| e.fingerprint.clone()).collect();
        fps.extend(extra.iter().map(|s| s.to_string()));
        let mut entries = entries.to_vec();
        entries.sort();
        let opaque = set(&["Opaque Dep"]);
        let header = Header {
            universe: "cross-app",
            primary_app: APP,
            opaque_apps: &opaque,
        };
        serialize_v2(&header, &fps, &entries)
    }

    fn update(old: &str, run: &UpdateRun) -> BaselineUpdate {
        update_baseline(&file(Some(old)), run).expect("update")
    }

    fn text(u: &BaselineUpdate) -> String {
        String::from_utf8(u.bytes.clone()).unwrap()
    }

    fn reasons(u: &BaselineUpdate) -> Vec<(&'static str, usize)> {
        u.kept_unchecked
            .iter()
            .map(|(r, n)| (r.key(), *n))
            .collect()
    }

    /// A fixed finding (its fingerprint gone, its detector ran completely,
    /// same universe, whole inputs, primary entry, primary run) is dropped.
    ///
    /// Mutation (2026-11-20): `unchecked_reason`'s final `None` made
    /// `Some(KeptUnchecked::InputsNotWhole)` -> the fixed entry is kept (red);
    /// restored, green.
    #[test]
    fn a_fixed_finding_is_dropped() {
        let old = v2(&[p("aa", D1), p("bb", D1)], &[]);
        let mut r = run();
        r.before_filters = vec![p("bb", D1)];
        r.shown = vec![p("bb", D1)];
        let u = update(&old, &r);
        assert_eq!(text(&u), v2(&[p("bb", D1)], &[]));
        assert_eq!((u.written, u.dropped, u.added), (1, 1, 0));
        assert!(u.kept_unchecked.is_empty() && u.stderr_line().is_none());
    }

    /// Every cause that keeps an unseen entry, one entry each, byte for byte,
    /// with its count. Each cause is stated on its own run.
    ///
    /// Mutation (2026-11-20): the `DetectorNotRun` branch deleted -> the
    /// `--detector`/`--preset` case drops its entry (red); restored, green.
    /// Mutation (2026-11-20): the `DetectorIncomplete` branch deleted -> the
    /// errored case drops (red); restored, green.
    /// Mutation (2026-11-20): the `OutOfScope` branch deleted -> the primary
    /// run drops the dependency entry (red); restored, green.
    /// Mutation (2026-11-20): the `InputsNotWhole` branch deleted -> the
    /// degraded run drops (red); restored, green.
    /// Mutation (2026-11-20): the `DetectorNotRegistered` branch deleted -> the
    /// renamed detector's entry reads `detectorNotRun` (red); restored, green.
    #[test]
    fn each_cause_keeps_its_unseen_entries_byte_for_byte() {
        type Case = (&'static str, Entry, fn(&mut UpdateRun), &'static str);
        let cases: [Case; 5] = [
            (
                "--detector / --preset: D3 not selected",
                p("aa", D3),
                |r| r.selected = set(&[D1]),
                "detectorNotRun",
            ),
            (
                "D1 errored or incomplete: not complete",
                p("aa", D1),
                |r| r.complete = set(&[D3]),
                "detectorIncomplete",
            ),
            (
                "primary-scope (or single-app) run, dependency entry",
                e("aa", D1, EntryScope::Dependency),
                |_| {},
                "outOfScope",
            ),
            (
                "degraded: a recovered file",
                p("aa", D1),
                |r| r.inputs_whole = false,
                "inputsNotWhole",
            ),
            (
                "the detector was renamed",
                p("aa", "d99-gone"),
                |_| {},
                "detectorNotRegistered",
            ),
        ];
        for (case, entry, tweak, reason) in cases {
            let old = v2(std::slice::from_ref(&entry), &[]);
            let mut r = run();
            tweak(&mut r);
            let u = update(&old, &r);
            assert_eq!(text(&u), old, "{case}");
            assert_eq!(reasons(&u), vec![(reason, 1)], "{case}");
            assert_eq!((u.written, u.dropped, u.added), (1, 0, 0), "{case}");
        }
    }

    /// `--scope all` (cross-app) is the one run that prunes a dependency entry.
    #[test]
    fn a_scope_all_run_prunes_a_gone_dependency_entry() {
        let old = v2(&[e("aa", D1, EntryScope::Dependency)], &[]);
        let mut r = run();
        r.prunes_dependency = true;
        let u = update(&old, &r);
        assert_eq!(text(&u), v2(&[], &[]));
        assert_eq!(u.dropped, 1);
    }

    /// Severity drift and `--limit`: a finding the run made but does not show
    /// (below `--min-severity`, past `--limit`) is present before the filters,
    /// so its entry stays; only what is shown is added.
    ///
    /// Mutation (2026-11-20): `seen` built from `run.shown` instead of
    /// `run.before_filters` -> "aa" is dropped (red); restored, green.
    #[test]
    fn a_finding_present_before_the_filters_keeps_its_entry() {
        let old = v2(&[p("aa", D1)], &[]);
        let mut r = run();
        r.before_filters = vec![p("aa", D1), p("bb", D1), p("cc", D3)];
        r.shown = vec![p("bb", D1)];
        let u = update(&old, &r);
        assert_eq!(text(&u), v2(&[p("aa", D1), p("bb", D1)], &[]));
        assert_eq!((u.written, u.dropped, u.added), (2, 0, 1));
        assert!(u.kept_unchecked.is_empty());
    }

    /// Several findings with one fingerprint (one detector, one scope) write
    /// one entry.
    ///
    /// Mutation (2026-11-20): `entries.dedup()` removed -> the entry is
    /// written twice (red); restored, green.
    #[test]
    fn one_entry_per_fingerprint_and_detector() {
        let mut r = run();
        r.before_filters = vec![p("aa", D1), p("aa", D1)];
        r.shown = r.before_filters.clone();
        let u = update_baseline(&file(None), &r).expect("update");
        assert_eq!(text(&u), v2(&[p("aa", D1)], &[]));
    }

    /// Schema 1: suppression reads it unchanged; an update keeps every unseen
    /// fingerprint (counted as no detector known), gives a seen one the
    /// detector that produced it, and writes schema 2 stamped with the run.
    /// Its unseen fingerprints survive a second update byte for byte.
    ///
    /// Mutation (2026-11-20): the schema-1 loop's `Some(found)` arm made a
    /// no-op -> "bb" is written with no entry (red); restored, green.
    #[test]
    fn schema_1_moves_to_schema_2() {
        let v1 = "{\n  \"schemaVersion\": \"1\",\n  \"generatedAt\": \
                  \"1970-01-01T00:00:00.000Z\",\n  \"fingerprints\": [\n    \"aa\",\n    \
                  \"bb\"\n  ]\n}\n";
        assert_eq!(
            load_baseline_bytes(&file(Some(v1))).unwrap(),
            set(&["aa", "bb"])
        );
        let mut r = run();
        r.before_filters = vec![p("bb", D3)];
        let u = update(v1, &r);
        let written = text(&u);
        assert_eq!(written, v2(&[p("bb", D3)], &["aa"]));
        assert_eq!(reasons(&u), vec![("noDetectorKnown", 1)]);
        assert_eq!(
            u.stderr_line().as_deref(),
            Some("al-sem: baseline: kept 1 entries unchecked: 1 no detector known")
        );
        // Read back: suppression sees both; a second update keeps "aa" as it was.
        assert_eq!(
            load_baseline_bytes(&file(Some(&written))).unwrap(),
            set(&["aa", "bb"])
        );
        assert_eq!(text(&update(&written, &r)), written);
    }

    /// Malformed, inconsistent and unknown-version files are refused (exit 2);
    /// another universe or primary app is refused (exit 3); a missing file is
    /// written. A refusal returns no bytes, so nothing is written.
    ///
    /// Mutation (2026-11-20): the "not in 'fingerprints'" check deleted -> the
    /// inconsistent file is accepted (red); restored, green.
    /// Mutation (2026-11-20): the stamp comparison deleted -> the single-app
    /// update of a cross-app file is accepted (red); restored, green.
    #[test]
    fn bad_and_foreign_files_are_refused() {
        let good = v2(&[p("aa", D1)], &[]);
        let malformed = [
            ("not json", "nope".to_string()),
            ("unknown version", good.replace("\"2\"", "\"3\"")),
            ("boolean version", good.replace("\"2\"", "true")),
            (
                "no opaqueApps",
                good.replace("  \"opaqueApps\": [\n    \"Opaque Dep\"\n  ],\n", ""),
            ),
            (
                "fingerprint not a string",
                good.replace(
                    "\"fingerprints\": [\n    \"aa\"",
                    "\"fingerprints\": [\n    7",
                ),
            ),
            (
                "bad scope",
                good.replace("\"primary\"\n", "\"elsewhere\"\n"),
            ),
            (
                "inconsistent",
                good.replace(
                    "\"fingerprints\": [\n    \"aa\"\n  ]",
                    "\"fingerprints\": []",
                ),
            ),
        ];
        for (case, bad) in malformed {
            assert_ne!(bad, good, "{case}: the break applied");
            assert!(
                matches!(
                    update_baseline(&file(Some(&bad)), &run()),
                    Err(Refusal::Malformed(_))
                ),
                "{case}"
            );
        }
        let mut single = run();
        single.universe = "single-app";
        let mut other_app = run();
        other_app.primary_app = "22222222-2222-2222-2222-222222222222".to_string();
        for (case, r) in [("--single-app", single), ("other app", other_app)] {
            assert!(
                matches!(
                    update_baseline(&file(Some(&good)), &r),
                    Err(Refusal::OtherRun(_))
                ),
                "{case}"
            );
        }
        let u = update_baseline(&file(None), &run()).expect("a missing file is new");
        assert_eq!(text(&u), v2(&[], &[]));
    }

    /// The counts on stderr and in `payload.summary.baselineUpdate`, by reason.
    #[test]
    fn kept_unchecked_counts_by_reason() {
        let old = v2(
            &[
                p("aa", D3),
                p("bb", D3),
                e("cc", D1, EntryScope::Dependency),
                p("dd", D1),
            ],
            &["ee"],
        );
        let mut r = run();
        r.selected = set(&[D1]);
        r.complete = set(&[D1]);
        r.shown = vec![p("ff", D1)];
        r.before_filters = r.shown.clone();
        let u = update(&old, &r);
        assert_eq!(
            u.stderr_line().as_deref(),
            Some(
                "al-sem: baseline: kept 4 entries unchecked: 1 no detector known, \
                 2 detector not run, 1 out of scope"
            )
        );
        assert_eq!(
            u.to_json(),
            serde_json::json!({
                "written": 5, "dropped": 1, "added": 1,
                "keptUnchecked": {"noDetectorKnown": 1, "detectorNotRun": 2, "outOfScope": 1}
            })
        );
    }

    /// A seen fingerprint's entries are replaced by the run's own: an old
    /// `dependency` row whose finding is now primary is written `primary`
    /// (review, Fable 4), so a later primary run can prune it.
    ///
    /// Mutation (2026-11-20): the seen arm pushes the old rows instead of
    /// `found` -> the `dependency` row is kept (red); restored, green.
    #[test]
    fn a_seen_fingerprint_takes_the_runs_entries() {
        let old = v2(&[e("aa", D1, EntryScope::Dependency)], &[]);
        let mut r = run();
        r.before_filters = vec![p("aa", D1)];
        let u = update(&old, &r);
        assert_eq!(text(&u), v2(&[p("aa", D1)], &[]));
    }

    /// An app opaque now that the file did not record lost its bodies: the
    /// run prunes nothing (inputs not whole), and the file keeps only the
    /// opacity it had. Opacity the file recorded stays allowed
    /// (`a_fixed_finding_is_dropped` prunes beside "Opaque Dep").
    ///
    /// Mutation (2026-11-20): the `is_subset` test removed from
    /// `update_baseline` -> the stale entry is dropped (red); restored, green.
    #[test]
    fn a_newly_opaque_app_stops_pruning() {
        let old = v2(&[p("aa", D1)], &[]);
        let mut r = run();
        r.opaque_apps = set(&["Opaque Dep", "Newly Opaque"]);
        let u = update(&old, &r);
        assert_eq!(text(&u), old, "kept, header unchanged");
        assert_eq!(reasons(&u), vec![("inputsNotWhole", 1)]);
    }

    /// Duplicate rows in the old file count once and are written once
    /// (review, sol P2: the count must equal the entries kept).
    ///
    /// Mutation (2026-11-20): `entries.dedup()` in `read_old` removed -> the
    /// count reads 2 (red); restored, green.
    #[test]
    fn duplicate_rows_count_once() {
        let old = v2(&[p("aa", D3)], &[]);
        let row = "    {\n      \"fingerprint\": \"aa\",\n      \"detector\": \
                   \"d3-missing-setloadfields\",\n      \"scope\": \"primary\"\n    }";
        assert_eq!(old.matches(row).count(), 1, "precondition: the row");
        let doubled = old.replace(row, &format!("{row},\n{row}"));
        let mut r = run();
        r.selected = set(&[D1]);
        let u = update(&doubled, &r);
        assert_eq!(text(&u), old);
        assert_eq!(reasons(&u), vec![("detectorNotRun", 1)]);
    }

    /// Versions (review, Fable 3): a missing or numeric `schemaVersion` is
    /// schema 1, for an update and a plain read; an unknown string version is
    /// refused by both.
    ///
    /// Mutation (2026-11-20): the missing-or-numeric arm of `read_old`
    /// deleted -> the update refuses them (red); restored, green.
    /// Mutation (2026-11-20): the `unknown_version` check in
    /// `load_baseline_bytes` deleted -> the plain read of "3" passes (red);
    /// restored, green.
    #[test]
    fn missing_and_numeric_versions_are_schema_1_unknown_strings_refused() {
        for (case, old) in [
            ("missing", "{\"fingerprints\": [\"aa\"]}"),
            (
                "numeric",
                "{\"schemaVersion\": 1, \"fingerprints\": [\"aa\"]}",
            ),
        ] {
            assert_eq!(load_baseline_bytes(&file(Some(old))).unwrap(), set(&["aa"]));
            let u = update(old, &run());
            assert_eq!(text(&u), v2(&[], &["aa"]), "{case}");
            assert_eq!(reasons(&u), vec![("noDetectorKnown", 1)], "{case}");
        }
        let unknown = "{\"schemaVersion\": \"3\", \"fingerprints\": [\"aa\"]}";
        assert!(load_baseline_bytes(&file(Some(unknown))).is_err());
        assert!(check_update_file(&file(Some(unknown))).is_err());
        assert!(check_update_file(&file(Some("nope"))).is_err());
        assert!(check_update_file(&file(None)).is_ok());
    }

    /// An empty `primaryApp` is not stamped: the first update with an id
    /// stamps it, never a refusal (review, Fable 5).
    ///
    /// Mutation (2026-11-20): the two `is_empty()` tests dropped from the
    /// stamp comparison -> the update is refused (red); restored, green.
    #[test]
    fn an_empty_primary_app_is_stamped_by_the_first_update_with_an_id() {
        let unstamped = v2(&[p("aa", D1)], &[]).replace(APP, "");
        assert!(unstamped.contains("\"primaryApp\": \"\""), "precondition");
        let mut r = run();
        r.before_filters = vec![p("aa", D1)];
        let u = update(&unstamped, &r);
        assert_eq!(text(&u), v2(&[p("aa", D1)], &[]));
    }
}
