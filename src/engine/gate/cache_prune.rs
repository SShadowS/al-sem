//! `cache prune` — dependency cache maintenance.
//!
//! Ports al-sem `src/deps/dependency-cache.ts`:
//!   - `classifyArtifactForPrune` → [`classify_artifact_for_prune`]
//!   - `pruneCache`              → [`prune_cache`]
//!   - `contentHashFromRawText`  → [`content_hash_from_raw_text`] (literal string replacement)
//!
//! The classification logic is BYTE-IDENTICAL to al-sem's:
//!   1. Filename must be `<64 lowercase hex chars>.json`.
//!   2. File must be readable + valid JSON + pass the `isDependencyArtifact` shape guard.
//!   3. `artifact.header.artifactKey` must equal the filename stem.
//!   4. `artifact.header.versions` must equal the current CACHE_VERSIONS + devFingerprint tuple.
//!   5. Content hash recompute (literal string replacement, NOT re-serialise) must match stored hash.
//!
//! The stdout report from `cache prune --dry-run` is also byte-identical to al-sem's:
//!
//! ```text
//! al-sem cache: <cacheDir>
//!   would remove N file(s) totalling X.X KB; kept M.
//!   - file.json  (X.X KB)  <status>
//! ```

use std::path::Path;

use crate::engine::ids::sha256_hex;

// ---------------------------------------------------------------------------
// CACHE_VERSIONS constants — the version tuple stamped into every dependency-
// cache artifact header. This engine owns the cache format now: bump the
// relevant const whenever a schema or classification-behavior change should
// invalidate every existing cache entry. The `kept` fixture pins this tuple.
// (Originated from al-sem's `src/deps/cache-versions.ts` / `src/providers/
// discover.ts` during the port; that TS source no longer exists to track.)
//
// The `analyzer` stamp is `CACHE_ANALYZER_VERSION` (below) — a cache-format
// version we own, independent of the engine's display/identity version.
// ---------------------------------------------------------------------------

/// Grammar version tag (mirrors `GRAMMAR_VERSION` in al-sem `discover.ts`).
pub const CACHE_VERSION_GRAMMAR: &str = "tree-sitter-al-v4.0.0-native";

/// Symbol-reader schema version (al-sem `cache-versions.ts` `symbolReader`).
/// Bumped 17→18 for the temp-state-tracking epoch (Task 16): the symbol-reader
/// surface now carries promoted object-global record vars + ABI per-param record
/// vars (with bound tableIds), so cached symbol-reader tuples from the prior epoch
/// must be invalidated.
pub const CACHE_VERSION_SYMBOL_READER: &str = "18";

/// Summary schema version (al-sem `cache-versions.ts` `summarySchema`).
pub const CACHE_VERSION_SUMMARY_SCHEMA: &str = "33";

/// Dep-cache serialization format version (al-sem `cache-versions.ts` `depCache`).
pub const CACHE_VERSION_DEP_CACHE: &str = "8";

/// Resource policy version (al-sem `cache-versions.ts` `resourcePolicy`).
pub const CACHE_VERSION_RESOURCE_POLICY: &str = "1";

/// The cache artifact format version stamped as the `analyzer` field in every
/// dependency-cache header. This versions OUR on-disk cache format — it is
/// deliberately decoupled from [`crate::engine::gate::version::driver_version`]
/// (the engine's display/identity version): bump this const only when a cache-shape
/// or classification-behavior change should invalidate every existing cache entry.
pub const CACHE_ANALYZER_VERSION: &str = "0.0.12";

/// The dev fingerprint used when not in a release build.
///
/// `ALCH_RELEASE=1` → `""` (release builds carry no dev fingerprint); otherwise
/// `ALCH_DEV_FINGERPRINT` if set, else `"dev"`.
pub fn dev_fingerprint() -> String {
    if std::env::var("ALCH_RELEASE").as_deref() == Ok("1") {
        return String::new();
    }
    std::env::var("ALCH_DEV_FINGERPRINT").unwrap_or_else(|_| "dev".to_string())
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Classification status assigned to each cache file by [`classify_artifact_for_prune`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneStatus {
    /// File passes all checks — do not delete it.
    Kept,
    /// Filename does not match `<64 hex chars>.json`.
    RemovedBadName,
    /// File is unreadable, not valid JSON, or fails the shape guard.
    RemovedUnreadable,
    /// Valid artifact but version stamp does not match the current build.
    RemovedVersionMismatch,
    /// Valid artifact + version stamp OK but content hash recompute does not match.
    RemovedContentHashMismatch,
}

impl PruneStatus {
    /// The canonical string label, matching al-sem's TS status values.
    pub fn as_str(&self) -> &'static str {
        match self {
            PruneStatus::Kept => "kept",
            PruneStatus::RemovedBadName => "removed-bad-name",
            PruneStatus::RemovedUnreadable => "removed-unreadable",
            PruneStatus::RemovedVersionMismatch => "removed-version-mismatch",
            PruneStatus::RemovedContentHashMismatch => "removed-content-hash-mismatch",
        }
    }
}

/// One entry classified by [`prune_cache`] — the filename, byte size, and status.
#[derive(Debug, Clone)]
pub struct PruneCacheEntry {
    pub file: String,
    pub bytes: u64,
    pub status: PruneStatus,
}

/// Result returned by [`prune_cache`].
#[derive(Debug)]
pub struct PruneCacheResult {
    pub cache_dir: String,
    pub entries: Vec<PruneCacheEntry>,
    pub bytes_freed: u64,
    pub files_removed: u64,
}

// ---------------------------------------------------------------------------
// Core classification logic
// ---------------------------------------------------------------------------

/// Recompute the content hash from the on-disk raw text using a LITERAL string
/// replacement — identical to al-sem's `contentHashFromRawText`:
///
/// ```text
/// rawText.replace(`"artifactContentHash":"${storedHash}"`,
///                 '"artifactContentHash":""')
/// → sha256(result)
/// ```
///
/// Parse-then-reserialize would change whitespace/key order → wrong hash.
pub fn content_hash_from_raw_text(raw_text: &str, stored_hash: &str) -> String {
    let needle = format!("\"artifactContentHash\":\"{}\"", stored_hash);
    let replacement = "\"artifactContentHash\":\"\"";
    let without_hash = raw_text.replacen(&needle, replacement, 1);
    sha256_hex(&without_hash)
}

/// Classify a single cache file (by its absolute path) for pruning.
///
/// Returns the classification status. Engine-never-throws: any I/O or parse
/// failure → `removed-unreadable`.
pub fn classify_artifact_for_prune(path: &Path) -> PruneStatus {
    // ── Step 1: filename must be `<64 lowercase hex chars>.json` ──────────
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let key = match extract_hex_key(file_name) {
        Some(k) => k,
        None => return PruneStatus::RemovedBadName,
    };

    // ── Step 2: read the file ──────────────────────────────────────────────
    let raw_text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return PruneStatus::RemovedUnreadable,
    };

    // ── Step 3: parse JSON + shape guard ──────────────────────────────────
    let parsed: serde_json::Value = match serde_json::from_str(&raw_text) {
        Ok(v) => v,
        Err(_) => return PruneStatus::RemovedUnreadable,
    };

    if !is_dependency_artifact(&parsed) {
        return PruneStatus::RemovedUnreadable;
    }

    // ── Step 4: artifact key must match the filename stem ─────────────────
    let artifact_key = parsed["header"]["artifactKey"].as_str().unwrap_or_default();
    if artifact_key != key {
        return PruneStatus::RemovedUnreadable;
    }

    // ── Step 5: version stamp must match the current build ────────────────
    // We iterate the EXPECTED keys only (mirrors al-sem's
    // `for (const [k,val] of Object.entries(expected))`): the artifact must
    // carry the matching value for each expected key, but is ALLOWED to have
    // extra/unknown version keys (a struct/map `==` would wrongly reject those).
    let v = &parsed["header"]["versions"];
    let fp = dev_fingerprint();
    let expected: [(&str, &str); 7] = [
        ("analyzer", CACHE_ANALYZER_VERSION),
        ("grammar", CACHE_VERSION_GRAMMAR),
        ("symbolReader", CACHE_VERSION_SYMBOL_READER),
        ("summarySchema", CACHE_VERSION_SUMMARY_SCHEMA),
        ("depCache", CACHE_VERSION_DEP_CACHE),
        ("resourcePolicy", CACHE_VERSION_RESOURCE_POLICY),
        // devFingerprint is a version field too (release-build aware).
        ("devFingerprint", fp.as_str()),
    ];
    for (k, expected_val) in &expected {
        match v[k].as_str() {
            Some(actual) if actual == *expected_val => {}
            _ => return PruneStatus::RemovedVersionMismatch,
        }
    }

    // ── Step 6: content hash recompute ────────────────────────────────────
    let stored_hash = parsed["header"]["artifactContentHash"]
        .as_str()
        .unwrap_or_default();
    let recomputed = content_hash_from_raw_text(&raw_text, stored_hash);
    if recomputed != stored_hash {
        return PruneStatus::RemovedContentHashMismatch;
    }

    PruneStatus::Kept
}

// ---------------------------------------------------------------------------
// prune_cache
// ---------------------------------------------------------------------------

/// Scan `cache_dir`, classify every non-tmp file, optionally delete the
/// `removed-*` ones (skip when `dry_run = true`), and return the summary.
///
/// The file list is sorted lexicographically before processing — `readdir`
/// order is not guaranteed, so we sort for determinism (mirrors al-sem's
/// `files.sort()`).
///
/// Engine-never-throws: I/O failures → skip the file silently (same as al-sem).
pub fn prune_cache(cache_dir_override: Option<&str>, dry_run: bool) -> PruneCacheResult {
    let cache_dir = resolve_cache_dir(cache_dir_override);
    let mut entries: Vec<PruneCacheEntry> = Vec::new();
    let mut bytes_freed: u64 = 0;
    let mut files_removed: u64 = 0;

    let dir_path = Path::new(&cache_dir);
    if !dir_path.is_dir() {
        return PruneCacheResult {
            cache_dir,
            entries,
            bytes_freed,
            files_removed,
        };
    }

    // Read + sort directory entries.
    let mut file_names: Vec<String> = match std::fs::read_dir(dir_path) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => {
            return PruneCacheResult {
                cache_dir,
                entries,
                bytes_freed,
                files_removed,
            };
        }
    };
    file_names.sort();

    for file_name in &file_names {
        // Skip temp files left by interrupted writes (mirrors al-sem's `.tmp.` guard).
        if file_name.contains(".tmp.") {
            continue;
        }

        let full_path = dir_path.join(file_name);

        // stat the file — skip non-files and unstat-able entries.
        let metadata = match std::fs::metadata(&full_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_file() {
            continue;
        }
        let bytes = metadata.len();

        let status = classify_artifact_for_prune(&full_path);

        entries.push(PruneCacheEntry {
            file: file_name.clone(),
            bytes,
            status: status.clone(),
        });

        if status == PruneStatus::Kept {
            continue;
        }

        // al-sem (dependency-cache.ts:251-259) attempts the unlink FIRST and on
        // failure `continue`s WITHOUT counting the file — leaving it on disk and
        // out of the freed totals (honest accounting). Dry-run never unlinks and
        // always accumulates. So only the real-delete path gates the increment on
        // a successful unlink.
        if !dry_run && std::fs::remove_file(&full_path).is_err() {
            continue;
        }

        bytes_freed += bytes;
        files_removed += 1;
    }

    PruneCacheResult {
        cache_dir,
        entries,
        bytes_freed,
        files_removed,
    }
}

// ---------------------------------------------------------------------------
// Stdout report builder
// ---------------------------------------------------------------------------

/// Render the cache prune result as the stdout text emitted by `cache prune`.
///
/// Matches al-sem's exact wording:
///
/// ```text
/// al-sem cache: <cacheDir>
///   would remove N file(s) totalling X.X KB; kept M.
///   - file.json  (X.X KB)  <status>
/// ```
///
/// For a real (non-dry-run) invocation the verb changes:
///
/// ```text
///   removed N file(s) totalling X.X KB; kept M.
/// ```
pub fn format_prune_report(result: &PruneCacheResult, dry_run: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("al-sem cache: {}\n", result.cache_dir));

    if result.entries.is_empty() {
        out.push_str("  (empty)\n");
        return out;
    }

    let removed: Vec<&PruneCacheEntry> = result
        .entries
        .iter()
        .filter(|e| e.status != PruneStatus::Kept)
        .collect();
    let kept = result.entries.len() - removed.len();

    let verb = if dry_run { "would remove" } else { "removed" };
    out.push_str(&format!(
        "  {} {} file(s) totalling {}; kept {}.\n",
        verb,
        result.files_removed,
        kb(result.bytes_freed),
        kept
    ));

    for e in &removed {
        out.push_str(&format!(
            "  - {}  ({})  {}\n",
            e.file,
            kb(e.bytes),
            e.status.as_str()
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Resolve the cache directory path.
/// Explicit override → use as-is; else `~/.al-sem/cache/`.
fn resolve_cache_dir(override_path: Option<&str>) -> String {
    if let Some(p) = override_path {
        return p.to_string();
    }
    // Use the `dirs` crate for cross-platform home directory resolution.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".al-sem")
        .join("cache")
        .to_string_lossy()
        .to_string()
}

/// Extract the 64 lowercase hex key from a filename of the form `<key>.json`.
/// Returns `None` if the filename does not match.
fn extract_hex_key(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".json")?;
    if stem.len() == 64 && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Some(stem)
    } else {
        None
    }
}

/// Format a byte count as `"X.X KB"` — mirrors al-sem's `(n / 1024).toFixed(1) + " KB"`.
///
/// JavaScript's `toFixed(1)` uses round-half-away-from-zero (for positive values).
/// We replicate this: multiply by 10, add 5, floor-divide by 10, then format.
/// This avoids the `.x5` half-way boundary ambiguity.
fn kb(bytes: u64) -> String {
    // bytes / 1024, rounded to 1 decimal place (round-half-away-from-zero).
    // scaled = floor((bytes * 10 + 512) / 1024)  →  tenths of KB, rounded.
    // Compute in u128 so `bytes * 10` cannot overflow (debug-panic) at absurd sizes.
    let scaled = (bytes as u128 * 10 + 512) / 1024;
    let whole = scaled / 10;
    let frac = scaled % 10;
    format!("{}.{} KB", whole, frac)
}

/// Minimal shape guard — mirrors al-sem's `isDependencyArtifact`.
/// Checks structural requirements but not semantic validity.
fn is_dependency_artifact(v: &serde_json::Value) -> bool {
    let obj = match v.as_object() {
        Some(o) => o,
        None => return false,
    };

    // header must be an object
    let header = match obj.get("header").and_then(|h| h.as_object()) {
        Some(h) => h,
        None => return false,
    };

    // schemaVersion must be 2 (DEPENDENCY_ARTIFACT_SCHEMA_VERSION).
    // TS checks `h.schemaVersion !== 2` (JS numeric ===). `as_u64() == Some(2)`
    // would reject a `2.0` JSON float where TS keeps it — but unreachable in
    // practice: every writer emits the integer literal 2 (canonicalStringify
    // renders the number 2 as "2", never "2.0").
    if header.get("schemaVersion").and_then(|s| s.as_u64()) != Some(2) {
        return false;
    }

    // artifactKey and artifactContentHash must be strings
    if header.get("artifactKey").and_then(|v| v.as_str()).is_none() {
        return false;
    }
    if header
        .get("artifactContentHash")
        .and_then(|v| v.as_str())
        .is_none()
    {
        return false;
    }

    // versions must be an object
    if header.get("versions").and_then(|v| v.as_object()).is_none() {
        return false;
    }

    // directDependencies must be an array
    if header
        .get("directDependencies")
        .and_then(|v| v.as_array())
        .is_none()
    {
        return false;
    }

    // summaryMode must be one of the 4 valid values
    let summary_mode = header
        .get("summaryMode")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !matches!(
        summary_mode,
        "full"
            | "structural-only-resource-guard"
            | "structural-only-parser-unavailable"
            | "structural-only-no-dep-summaries"
    ) {
        return false;
    }

    // abi must be an object with objects/tables/routines/eventPublishers arrays
    let abi = match obj.get("abi").and_then(|a| a.as_object()) {
        Some(a) => a,
        None => return false,
    };
    if abi.get("objects").and_then(|v| v.as_array()).is_none() {
        return false;
    }
    if abi.get("tables").and_then(|v| v.as_array()).is_none() {
        return false;
    }
    if abi.get("routines").and_then(|v| v.as_array()).is_none() {
        return false;
    }
    if abi
        .get("eventPublishers")
        .and_then(|v| v.as_array())
        .is_none()
    {
        return false;
    }

    // diagnostics must be an array
    if obj.get("diagnostics").and_then(|v| v.as_array()).is_none() {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_rounds_correctly() {
        // 1820 bytes = 1.777... KB → rounds to 1.8 KB (the dry-run.txt value)
        assert_eq!(kb(1820), "1.8 KB");
        // 867 bytes = 0.847... KB → rounds to 0.8 KB
        assert_eq!(kb(867), "0.8 KB");
        // 870 bytes = 0.849... KB → rounds to 0.8 KB
        assert_eq!(kb(870), "0.8 KB");
        // 859 bytes → 0.8 KB
        assert_eq!(kb(859), "0.8 KB");
        // 35 bytes → 0.0 KB
        assert_eq!(kb(35), "0.0 KB");
        // 48 bytes → 0.0 KB
        assert_eq!(kb(48), "0.0 KB");
        // 1024 bytes = exactly 1.0 KB
        assert_eq!(kb(1024), "1.0 KB");
        // 512 bytes = 0.5 KB
        assert_eq!(kb(512), "0.5 KB");
    }

    /// kb() must not overflow (debug-panic) at absurd byte counts — the u128
    /// widening (item 7) guards `bytes * 10`. u64::MAX bytes is ~16 exabytes.
    #[test]
    fn kb_does_not_overflow_at_u64_max() {
        // Just verify it computes a value without panicking.
        let s = kb(u64::MAX);
        assert!(s.ends_with(" KB"), "kb(u64::MAX) = {s}");
    }

    #[test]
    fn extract_hex_key_valid() {
        let key = "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe";
        assert_eq!(extract_hex_key(&format!("{}.json", key)), Some(key));
    }

    #[test]
    fn extract_hex_key_rejects_non_json() {
        assert!(extract_hex_key("cafecafe.txt").is_none());
    }

    #[test]
    fn extract_hex_key_rejects_wrong_length() {
        assert!(extract_hex_key("cafe.json").is_none());
    }

    #[test]
    fn extract_hex_key_rejects_uppercase() {
        let upper = "CAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFECAFE";
        assert!(extract_hex_key(&format!("{}.json", upper)).is_none());
    }

    #[test]
    fn extract_hex_key_rejects_nothex() {
        assert!(extract_hex_key("nothex.json").is_none());
    }

    /// `CACHE_VERSION_GRAMMAR` participates in the dependency-cache version tuple
    /// (`expected` in `classify_artifact_for_prune`), so it must track the grammar
    /// this engine actually links. It did not: it read
    /// `"tree-sitter-al-v2.5.2-native"` while the pinned grammar was v3.2.0 —
    /// never bumped across two grammar upgrades, and nothing failed.
    ///
    /// This is the executable form of that doc claim. A version literal that can be
    /// compared against its source should die as an assert, not be hunted as prose.
    ///
    /// Path resolution: `TREE_SITTER_AL_PATH` first (worktrees get no submodule
    /// checkout — see CLAUDE.md's Prerequisites section), falling back to
    /// `CARGO_MANIFEST_DIR`/tree-sitter-al for a normal checkout. `ci.yml` exports
    /// `TREE_SITTER_AL_PATH` for the test step, so both paths are covered.
    ///
    /// DISCRIMINATION PROOF (recorded 2026-08-02): set the constant back to
    /// `"tree-sitter-al-v2.5.2-native"` — this test FAILS with `CACHE_VERSION_GRAMMAR
    /// is "tree-sitter-al-v2.5.2-native" but the linked grammar is v3.2.0 — bump the
    /// constant (it keys dependency-cache invalidation, it is not a comment)`.
    /// Restore to the current constant — this test PASSES.
    #[test]
    fn cache_version_grammar_tracks_the_linked_grammar() {
        let root = std::env::var("TREE_SITTER_AL_PATH")
            .unwrap_or_else(|_| format!("{}/tree-sitter-al", env!("CARGO_MANIFEST_DIR")));
        let pkg = std::fs::read_to_string(format!("{root}/package.json")).expect(
            "tree-sitter-al/package.json must be readable (set TREE_SITTER_AL_PATH or check out the submodule)",
        );
        let v: serde_json::Value = serde_json::from_str(&pkg).expect("package.json parses");
        let grammar_version = v["version"].as_str().expect("package.json has a version");

        assert!(
            CACHE_VERSION_GRAMMAR.contains(grammar_version),
            "CACHE_VERSION_GRAMMAR is {CACHE_VERSION_GRAMMAR:?} but the linked grammar \
             is v{grammar_version} — bump the constant (it keys dependency-cache \
             invalidation, it is not a comment)"
        );
    }
}
