//! cli-c/c3 — `cache prune` classification + dry-run byte-parity differential tests.
//!
//! # Coverage
//!
//! (a) NATIVE ORACLES: `classify_artifact_for_prune` byte-for-byte vs
//!     `classification.json` for all 5 fixture files.
//! (b) DRY-RUN DIFFERENTIAL: run `cache prune --dry-run --dep-cache-dir <fixture-cache>`
//!     and byte-compare the stdout (with the first line normalized from the abs path to
//!     `<CACHE_DIR>`) to `dry-run.txt`.
//! (c) INTEGRATION: copy the fixture cache to a temp dir, run a real (non-dry-run)
//!     prune, assert the 4 `removed-*` files are deleted and the `kept` file remains.
//!     NOT byte-differentialed — the mutation is filesystem observable only.
//!
//! # Golden source
//! All goldens + fixtures are vendored in-repo at `tests/cli-c-goldens/cache/`
//! (temp-state epoch, Task 16). al-sem is FROZEN — never modified — and is no
//! longer read at all: the symbolReader cache bump 17→18 invalidated al-sem's
//! original cache fixtures (a version bump the frozen archive can never
//! regenerate), so Task 3.3 retired the al-sem fallback and made this in-repo
//! corpus (the kept/content-hash-mismatch fixtures bumped to "18", with the
//! kept fixture's `artifactContentHash` recomputed) the sole, hard-required
//! source. There is no skip-gate: a missing fixture is a hard failure.

use std::path::PathBuf;

use al_sem::engine::gate::cache_prune::{
    PruneCacheEntry, PruneStatus, classify_artifact_for_prune, format_prune_report, prune_cache,
};

use crate::regen;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// This crate's manifest dir (the alch-engine worktree root).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// In-repo vendored cli-c cache golden corpus (temp-state epoch, Task 16; sole
/// source since Task 3.3 retired the al-sem fallback).
fn cache_goldens_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("cli-c-goldens")
        .join("cache")
}

fn fixture_cache_dir() -> PathBuf {
    cache_goldens_dir().join("fixture-cache")
}

/// Hard-require the vendored fixture-cache directory. Without this, a missing
/// `fixture-cache/` would make `classify_artifact_for_prune` degrade a
/// nonexistent path to `RemovedUnreadable`/`RemovedBadName` — coincidentally
/// the SAME status `classification_oracle_unreadable`/`classification_oracle_
/// bad_name` expect, so those two tests would spuriously PASS against an
/// absent corpus instead of failing loudly (Task 3.3 review fix).
fn require_fixture_cache() {
    let dir = fixture_cache_dir();
    assert!(
        dir.is_dir(),
        "vendored fixture-cache missing at {} (Task 3.3)",
        dir.display()
    );
}

fn load_golden(name: &str) -> String {
    let path = cache_goldens_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read golden {}: {e}", path.display()))
}

/// Render `entries` in the exact on-disk `classification.json` form (2-space
/// indent, `file`/`bytes`/`status` key order per object). Hand-built rather
/// than via `serde_json::Value` because this crate's `serde_json` does not
/// enable `preserve_order`, so a generic `Value::Object` would alphabetize
/// keys (`bytes`,`file`,`status`) instead of matching the committed golden's
/// `file`,`bytes`,`status` order.
fn classification_json_text(entries: &[PruneCacheEntry]) -> String {
    let mut out = String::from("[\n");
    for (i, e) in entries.iter().enumerate() {
        out.push_str("  {\n");
        out.push_str(&format!("    \"file\": {:?},\n", e.file));
        out.push_str(&format!("    \"bytes\": {},\n", e.bytes));
        out.push_str(&format!("    \"status\": {:?}\n", e.status.as_str()));
        out.push_str(if i + 1 == entries.len() {
            "  }\n"
        } else {
            "  },\n"
        });
    }
    out.push_str("]\n");
    out
}

// ---------------------------------------------------------------------------
// (a) Native oracles — classify_artifact_for_prune per fixture file
// ---------------------------------------------------------------------------

/// The 5 fixture files and their expected classification statuses.
const FIXTURE_CLASSIFICATIONS: &[(&str, PruneStatus)] = &[
    (
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef.json",
        PruneStatus::RemovedContentHashMismatch,
    ),
    (
        "babebabebabebabebabebabebabebabebabebabebabebabebabebabebabebabe.json",
        PruneStatus::RemovedVersionMismatch,
    ),
    (
        "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe.json",
        PruneStatus::Kept,
    ),
    (
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.json",
        PruneStatus::RemovedUnreadable,
    ),
    ("nothex.json", PruneStatus::RemovedBadName),
];

#[test]
fn classification_oracle_content_hash_mismatch() {
    require_fixture_cache();
    let file = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef.json";
    let path = fixture_cache_dir().join(file);
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedContentHashMismatch,
        "expected removed-content-hash-mismatch for {file}"
    );
}

#[test]
fn classification_oracle_version_mismatch() {
    require_fixture_cache();
    let file = "babebabebabebabebabebabebabebabebabebabebabebabebabebabebabebabe.json";
    let path = fixture_cache_dir().join(file);
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedVersionMismatch,
        "expected removed-version-mismatch for {file}"
    );
}

#[test]
fn classification_oracle_kept() {
    require_fixture_cache();
    let file = "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe.json";
    let path = fixture_cache_dir().join(file);
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::Kept,
        "expected kept for {file}"
    );
}

#[test]
fn classification_oracle_unreadable() {
    require_fixture_cache();
    let file = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.json";
    let path = fixture_cache_dir().join(file);
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedUnreadable,
        "expected removed-unreadable for {file}"
    );
}

#[test]
fn classification_oracle_bad_name() {
    require_fixture_cache();
    let file = "nothex.json";
    let path = fixture_cache_dir().join(file);
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedBadName,
        "expected removed-bad-name for {file}"
    );
}

/// Aggregate oracle: verify ALL 5 statuses against classification.json.
#[test]
fn classification_oracle_all_vs_golden_json() {
    require_fixture_cache();

    // REGEN path (Task T0.6 — this family previously had none; the retired-
    // al-sem-oracle note below is about refreshing from al-sem, which is a
    // separate, still-impossible concern — this regenerates from THIS engine's
    // OWN current classification, the Rust-owned-baseline doctrine). When
    // `REGEN_TEMP_GOLDENS=1`, reclassify every file `prune_cache` sees (dry-run,
    // so nothing is deleted) and write `classification.json` from that.
    if regen::regen_mode() {
        let cache_dir = fixture_cache_dir();
        let result = prune_cache(Some(&cache_dir.to_string_lossy()), true);
        let golden_path = cache_goldens_dir().join("classification.json");
        std::fs::write(&golden_path, classification_json_text(&result.entries))
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!(
            "REGEN cli-c cache classification golden: {}",
            golden_path.display()
        );
        return;
    }

    // Load and parse the classification golden.
    let golden_text = load_golden("classification.json");
    let golden: serde_json::Value =
        serde_json::from_str(&golden_text).expect("classification.json must be valid JSON");
    let golden_arr = golden
        .as_array()
        .expect("classification.json must be an array");

    // Build expected map: file → status string.
    let expected: std::collections::HashMap<String, String> = golden_arr
        .iter()
        .map(|entry| {
            let file = entry["file"].as_str().expect("file field").to_string();
            let status = entry["status"].as_str().expect("status field").to_string();
            (file, status)
        })
        .collect();

    // Classify each fixture and compare.
    for (file, _) in FIXTURE_CLASSIFICATIONS {
        let path = fixture_cache_dir().join(file);
        let actual = classify_artifact_for_prune(&path);
        let actual_str = actual.as_str();
        let expected_str = expected
            .get(*file)
            .map(|s| s.as_str())
            .unwrap_or_else(|| panic!("file {file} not found in classification.json"));
        assert_eq!(
            actual_str, expected_str,
            "classification mismatch for {file}: got {actual_str:?}, want {expected_str:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// (b) Dry-run differential — stdout byte-matches dry-run.txt (with normalization)
// ---------------------------------------------------------------------------

#[test]
fn dry_run_differential_vs_golden() {
    require_fixture_cache();
    let cache_dir = fixture_cache_dir();
    let cache_dir_str = cache_dir.to_string_lossy();

    // Run the dry-run prune.
    let result = prune_cache(Some(&cache_dir_str), true);
    let report = format_prune_report(&result, true);

    // Normalize: replace the abs cache dir path in the first line with `<CACHE_DIR>`.
    // al-sem golden: `al-sem cache: <CACHE_DIR>`
    let normalized = report.replacen(cache_dir_str.as_ref(), "<CACHE_DIR>", 1);

    // REGEN path (Task T0.6 — this family previously had none; see the note on
    // `classification_oracle_all_vs_golden_json` above). When
    // `REGEN_TEMP_GOLDENS=1`, write the already-normalized report straight to
    // the golden file instead of comparing.
    if regen::regen_mode() {
        let golden_path = cache_goldens_dir().join("dry-run.txt");
        std::fs::write(&golden_path, &normalized)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!(
            "REGEN cli-c cache dry-run golden: {}",
            golden_path.display()
        );
        return;
    }

    // Load the committed golden.
    let golden = load_golden("dry-run.txt");

    assert_eq!(
        normalized, golden,
        "dry-run output does not match dry-run.txt golden.\n\
         --- normalized ---\n{normalized}\n--- golden ---\n{golden}"
    );
}

// ---------------------------------------------------------------------------
// (c) Integration test — real (non-dry-run) prune deletes removed-* files
// ---------------------------------------------------------------------------

#[test]
fn integration_real_prune_deletes_removed_files() {
    require_fixture_cache();
    // Copy the fixture cache to a temp dir so we can mutate it.
    let tmp_dir = std::env::temp_dir().join(format!(
        "al-sem-cache-prune-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

    // Copy all fixture files into the temp dir.
    let src_dir = fixture_cache_dir();
    for entry in std::fs::read_dir(&src_dir).expect("read fixture-cache") {
        let entry = entry.expect("read entry");
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let dst = tmp_dir.join(entry.file_name());
        std::fs::copy(&src, &dst).expect("copy fixture file");
    }

    let tmp_str = tmp_dir.to_string_lossy();

    // Run a REAL prune (not dry-run).
    let result = prune_cache(Some(&tmp_str), false);

    // 4 files should be removed, 1 kept.
    assert_eq!(
        result.files_removed, 4,
        "expected 4 files removed; got {}",
        result.files_removed
    );

    // The `kept` file must still exist.
    let kept_file = "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe.json";
    assert!(
        tmp_dir.join(kept_file).exists(),
        "kept file {kept_file} should still exist after real prune"
    );

    // The 4 `removed-*` files must NOT exist.
    let removed_files = [
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef.json",
        "babebabebabebabebabebabebabebabebabebabebabebabebabebabebabebabe.json",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.json",
        "nothex.json",
    ];
    for f in &removed_files {
        assert!(
            !tmp_dir.join(f).exists(),
            "removed file {f} should be deleted after real prune"
        );
    }

    // Clean up.
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ---------------------------------------------------------------------------
// Corpus-invisible native oracles — classification edges the 5 fixtures don't
// cover. These build their own temp-dir fixtures (no al-sem checkout needed).
// ---------------------------------------------------------------------------

/// A canonical, valid (kept-equivalent) artifact body for the CURRENT engine
/// version tuple, with `<KEY>` / `<HASH>` placeholders to be filled in by tests.
/// The byte layout matches al-sem's `writeCachedArtifact` canonical output
/// (recursively key-sorted, compact). The content hash is computed over the
/// body with `artifactContentHash` cleared, so a test can produce a genuinely
/// `kept` file by computing the hash itself.
fn canonical_artifact_with_versions(key: &str, versions_json: &str) -> String {
    // `abi` sorts before `header`; within `header`, keys are alphabetical, and
    // `artifactContentHash` sits between `appIdentity` and `artifactKey`.
    // We leave artifactContentHash empty here; the test fills it in.
    format!(
        "{{\"abi\":{{\"eventPublishers\":[],\"objects\":[],\"routines\":[],\"tables\":[]}},\
\"diagnostics\":[],\
\"header\":{{\"appIdentity\":{{\"appGuid\":\"00000000-0000-0000-0000-000000000000\",\
\"name\":\"Synthetic\",\"publisher\":\"AlSemTest\",\"sourceKind\":\"symbol-only\",\
\"version\":\"1.0.0.0\"}},\
\"artifactContentHash\":\"<HASH>\",\
\"artifactKey\":\"{key}\",\
\"directDependencies\":[],\
\"packageHash\":\"{key}\",\
\"packageSemanticHash\":\"{key}\",\
\"schemaVersion\":2,\
\"summaryMode\":\"structural-only-no-dep-summaries\",\
\"versions\":{versions_json}}}}}"
    )
}

/// The current-build version JSON object (the tuple the `kept` classification
/// requires). Reproduced from al-sem's cache-versions.ts + dev fingerprint.
fn current_versions_json(extra_key: Option<(&str, &str)>) -> String {
    // Sorted keys (canonical): analyzer, depCache, devFingerprint, grammar,
    // resourcePolicy, summarySchema, symbolReader — plus any extra key in sort
    // position. We build a serde_json object then canonicalize manually.
    let mut map = serde_json::Map::new();
    map.insert("analyzer".into(), "0.0.12".into());
    map.insert("depCache".into(), "8".into());
    map.insert("devFingerprint".into(), "dev".into());
    map.insert("grammar".into(), "tree-sitter-al-v4.0.0-native".into());
    map.insert("resourcePolicy".into(), "1".into());
    map.insert("summarySchema".into(), "33".into());
    map.insert("symbolReader".into(), "18".into());
    if let Some((k, v)) = extra_key {
        map.insert(k.into(), v.into());
    }
    // serde_json::to_string sorts keys when the map preserves insertion order?
    // No — Map iterates insertion order. We want a STABLE, key-sorted render to
    // match canonical bytes. Collect, sort, render.
    let mut pairs: Vec<(String, String)> = map
        .into_iter()
        .map(|(k, v)| (k, v.as_str().unwrap_or_default().to_string()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let body = pairs
        .iter()
        .map(|(k, v)| format!("\"{k}\":\"{v}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

/// Make a unique temp dir for a synthetic-oracle test.
fn synth_temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "al-sem-cache-synth-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create synth temp dir");
    dir
}

/// (item 3) A non-`.json`, non-hex64 file (`junk.txt`) classifies as
/// `removed-bad-name` — the listing must NOT pre-filter by `.json` extension.
#[test]
fn oracle_junk_txt_is_bad_name() {
    let dir = synth_temp_dir("junk");
    let path = dir.join("junk.txt");
    std::fs::write(&path, "not even json").unwrap();
    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedBadName,
        "junk.txt must classify as removed-bad-name"
    );

    // And prune_cache must SWEEP it (it must appear in entries, not be ignored).
    let result = prune_cache(Some(&dir.to_string_lossy()), true);
    let entry = result
        .entries
        .iter()
        .find(|e| e.file == "junk.txt")
        .expect("junk.txt must appear in prune entries (not pre-filtered away)");
    assert_eq!(entry.status, PruneStatus::RemovedBadName);
    assert_eq!(
        result.files_removed, 1,
        "junk.txt should be counted as removed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// (item 4) A valid artifact whose `versions` carries an EXTRA/unknown key
/// still classifies as `kept` — the version check iterates EXPECTED keys only
/// (a struct/map `==` would wrongly reject it).
#[test]
fn oracle_extra_version_key_still_kept() {
    let dir = synth_temp_dir("extrakey");
    let key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // Build the body with an extra version key, then compute the real content hash.
    let versions = current_versions_json(Some(("futureField", "999")));
    let body_empty_hash = canonical_artifact_with_versions(key, &versions).replace("<HASH>", "");
    // sha256 of the empty-hash body (mirror writeCachedArtifact).
    let content_hash = sha256_hex_test(&body_empty_hash);
    let final_body =
        canonical_artifact_with_versions(key, &versions).replace("<HASH>", &content_hash);

    let path = dir.join(format!("{key}.json"));
    std::fs::write(&path, &final_body).unwrap();

    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::Kept,
        "artifact with an extra version key must still be kept (expected-keys-only compare)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// (item 5) A `<hex>.tmp.N.json` file is SKIPPED by prune_cache (substring
/// `.tmp.` match) — it never appears in entries and is not classified/listed.
#[test]
fn oracle_tmp_file_is_skipped() {
    let dir = synth_temp_dir("tmp");
    let key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let tmp_name = format!("{key}.tmp.5.json");
    std::fs::write(dir.join(&tmp_name), "anything at all").unwrap();

    let result = prune_cache(Some(&dir.to_string_lossy()), true);
    assert!(
        result.entries.iter().all(|e| e.file != tmp_name),
        "{tmp_name} must be skipped (not classified/listed)"
    );
    assert_eq!(result.files_removed, 0, "no files should be removed");
    assert!(result.entries.is_empty(), "tmp-only dir → no entries");
    let _ = std::fs::remove_dir_all(&dir);
}

/// (item 6) An artifact with an invalid `summaryMode` (not one of the 4 verbose
/// literals) fails `isDependencyArtifact` → `removed-unreadable`, NOT a panic.
#[test]
fn oracle_wrong_summary_mode_is_unreadable() {
    let dir = synth_temp_dir("summode");
    let key = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    // Build a body with the BASE "structural-only" (a wrong/short value), keeping
    // all else valid. summaryMode is not one of the 4 verbose literals → guard fails.
    let versions = current_versions_json(None);
    let body = canonical_artifact_with_versions(key, &versions)
        .replace(
            "<HASH>",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .replace(
            "\"summaryMode\":\"structural-only-no-dep-summaries\"",
            "\"summaryMode\":\"structural-only\"",
        );
    let path = dir.join(format!("{key}.json"));
    std::fs::write(&path, &body).unwrap();

    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedUnreadable,
        "wrong summaryMode must degrade to removed-unreadable (shape guard), not panic"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// (item 6 precedence) When BOTH the shape is invalid AND the versions differ,
/// the shape guard runs FIRST → `removed-unreadable` (NOT version-mismatch).
#[test]
fn oracle_shape_invalid_beats_version_mismatch() {
    let dir = synth_temp_dir("precedence");
    let key = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    // Stale version (symbolReader "1") AND an invalid summaryMode. Shape guard
    // (summaryMode) fires before the version check → removed-unreadable.
    let mut stale = current_versions_json(None);
    stale = stale.replace("\"symbolReader\":\"18\"", "\"symbolReader\":\"1\"");
    let body = canonical_artifact_with_versions(key, &stale)
        .replace(
            "<HASH>",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .replace(
            "\"summaryMode\":\"structural-only-no-dep-summaries\"",
            "\"summaryMode\":\"bogus-mode\"",
        );
    let path = dir.join(format!("{key}.json"));
    std::fs::write(&path, &body).unwrap();

    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedUnreadable,
        "shape-invalid artifact is removed-unreadable even when versions also differ"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// (item 6 negative control) A stale-version-ONLY artifact (valid shape) →
/// `removed-version-mismatch`, proving the precedence test above isn't trivially
/// passing for the wrong reason.
#[test]
fn oracle_stale_version_only_is_version_mismatch() {
    let dir = synth_temp_dir("staleonly");
    let key = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    let mut stale = current_versions_json(None);
    stale = stale.replace("\"symbolReader\":\"18\"", "\"symbolReader\":\"1\"");
    // Valid shape (correct summaryMode), but the content hash need not match —
    // the version check (step 5) runs BEFORE the content-hash recompute (step 6),
    // so a stale version short-circuits to version-mismatch regardless of hash.
    let body = canonical_artifact_with_versions(key, &stale).replace(
        "<HASH>",
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let path = dir.join(format!("{key}.json"));
    std::fs::write(&path, &body).unwrap();

    assert_eq!(
        classify_artifact_for_prune(&path),
        PruneStatus::RemovedVersionMismatch,
        "valid-shape stale-version artifact must be removed-version-mismatch"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Local SHA-256 helper for the synthetic oracles (mirrors the engine's
/// `sha256_hex` — UTF-8 bytes, lowercase hex). Kept test-local so the test does
/// not depend on the engine exposing `ids::sha256_hex` publicly.
fn sha256_hex_test(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let bytes = hasher.finalize();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Refresh-FROM-AL-SEM test RETIRED (Task 3.3): the retired al-sem oracle's
// symbolReader version is permanently stuck at 17 while the engine's is 18,
// so it can never again regenerate a `kept`-classified fixture — refreshing
// from IT would silently re-break the corpus. That is unrelated to
// rebaselining from THIS engine's own output: `classification_oracle_all_vs_
// golden_json` and `dry_run_differential_vs_golden` above both gained a
// `REGEN_TEMP_GOLDENS=1` path (Task T0.6) that reclassifies the vendored
// fixture-cache and writes the Rust-owned baseline — al-sem is never
// consulted. The in-repo `fixture-cache/` corpus itself (Task 16 rebaseline)
// remains the sole source of INPUT fixtures; there is no automated way to
// refresh THOSE from al-sem, only the derived goldens from this engine.
// ---------------------------------------------------------------------------
