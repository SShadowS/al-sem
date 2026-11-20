//! T23 (#277 spec D2.4, S17): `--update-baseline`'s write left compute and is
//! now the bin's first write, under the one write-failure rule.
//!
//! The precondition is Task 1's base-binary record (`update-baseline/`, read
//! through the guarded reader, never regenerated): at `1134bed1` a failed write
//! printed `al-sem: failed to write baseline '<path>': <OS message>` with exit 3
//! and an empty stdout. The head prints `al-sem: write error: <path>: <OS
//! message>` with exit 2, stdout still empty.
//!
//! Normalization: the record was minted on Linux, so its OS message is Linux's.
//! The compare replaces the OS message with `<OS message>` on both sides (the
//! record's from its own text, the head's from the OS's error for the same
//! write), so the test runs on any platform. The record holds no version field.

use std::path::Path;
use std::process::{Command, Output};

use crate::batch_records::{batch_goldens, read_record};

const BASELINE: &str = "tests/gate-goldens/batch/update-baseline/no-such-dir/b.json";

fn analyze(baseline: &str, update: bool) -> Output {
    let mut args = vec![
        "analyze",
        "tests/r0-corpus/ws-d1",
        "--deterministic",
        "--baseline",
        baseline,
    ];
    if update {
        args.push("--update-baseline");
    }
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem")
}

/// `text` with the OS message after `head` replaced by `<OS message>`.
fn normalize(text: &str, head: &str) -> String {
    let rest = text
        .strip_prefix(head)
        .unwrap_or_else(|| panic!("{text:?} does not start with {head:?}"));
    assert!(
        rest.ends_with('\n') && rest.len() > 1,
        "no OS message: {text:?}"
    );
    format!("{head}<OS message>\n")
}

#[test]
fn update_baseline_write_failure() {
    let dir = batch_goldens().join("update-baseline");
    // Precondition: the base behaviour, from the record.
    let base_err = String::from_utf8(read_record(&dir.join("base.stderr.golden"))).unwrap();
    assert_eq!(
        normalize(
            &base_err,
            &format!("al-sem: failed to write baseline '{BASELINE}': ")
        ),
        format!("al-sem: failed to write baseline '{BASELINE}': <OS message>\n")
    );
    assert!(read_record(&dir.join("base.stdout.golden")).is_empty());
    assert_eq!(read_record(&dir.join("base.exit")), b"3\n");

    // The head.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !root.join(BASELINE).parent().unwrap().exists(),
        "precondition: the baseline's folder does not exist"
    );
    let os = std::fs::write(root.join(BASELINE), b"").unwrap_err();
    let out = analyze(BASELINE, true);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err, format!("al-sem: write error: {BASELINE}: {os}\n"));
    assert_eq!(
        normalize(&err, &format!("al-sem: write error: {BASELINE}: ")),
        format!("al-sem: write error: {BASELINE}: <OS message>\n")
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout unwritten");
}

/// TLS-106's cross-app fixture: at `--scope all` it has dependency findings
/// (d14, d33 in the dependency's source), at the default scope only primary ones.
const DEP_WS: &str = "tests/fixtures/mcp-output/ws";

fn alsem(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem")
}

/// `analyze DEP_WS --baseline <base> --update-baseline <extra>`.
fn update_dep_ws(base: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "analyze",
        DEP_WS,
        "--deterministic",
        "--baseline",
        base,
        "--update-baseline",
    ];
    args.extend_from_slice(extra);
    alsem(&args)
}

/// A baseline seeded by a `--scope all` update of DEP_WS, and its bytes.
fn seeded(dir: &tempfile::TempDir) -> (String, String) {
    let path = dir.path().join("b.json").to_str().unwrap().to_string();
    let seed = update_dep_ws(&path, &["--scope", "all"]);
    assert_eq!(
        seed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&seed.stderr)
    );
    let text = std::fs::read_to_string(&path).expect("seeded");
    (path, text)
}

/// The entries of `scope` in a schema-2 file.
fn entries_of(text: &str, scope: &str) -> usize {
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["scope"] == scope)
        .count()
}

/// TLS-106, the case that matters most: a `--scope all` update, then a
/// default (primary-scope) update, keeps every dependency entry. The registry
/// drops dependency findings itself at `--scope primary`, so the second run
/// never sees them; the entry's scope keeps them. Through the binary, on the
/// file's bytes; the counts on stderr and in the JSON.
///
/// Mutation (2026-11-20): `prunes_dependency: true` in `analyze_view`'s
/// `UpdateRun` -> the default update drops the dependency entries (red);
/// restored, green.
#[test]
fn scope_all_then_default_update_keeps_dependency_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seeded(&dir);
    let deps = entries_of(&seed, "dependency");
    let primaries = entries_of(&seed, "primary");
    assert!(
        deps > 0 && primaries > 0,
        "precondition: both scopes: {seed}"
    );
    let out = update_dep_ws(&path, &["--format", "json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!("al-sem: baseline: kept {deps} entries unchecked: {deps} out of scope\n")
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let fps = serde_json::from_str::<serde_json::Value>(&seed).unwrap()["fingerprints"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        v["payload"]["summary"]["baselineUpdate"],
        serde_json::json!({
            "written": fps, "dropped": 0, "added": 0,
            "keptUnchecked": {"outOfScope": deps}
        })
    );
}

/// TLS-106: an update narrowed by `--min-severity`, `--detector`, `--preset`
/// or `--limit` keeps the file byte for byte (presence is tested before the
/// gate's filters; detectors outside the view keep their entries), and a
/// `--single-app` update of a cross-app file is refused (exit 3), the file
/// unchanged.
///
/// Mutation (2026-11-20): `before_filters` passed as `vec![]` in
/// `analyze_view`'s `UpdateRun` -> the filtered updates drop the entries they
/// do not show (red); restored, green.
#[test]
fn filtered_updates_keep_the_file_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seeded(&dir);
    for filter in [
        &["--scope", "all", "--min-severity", "critical"][..],
        &["--scope", "all", "--detector", "d19-unused-parameter"],
        &["--scope", "all", "--preset", "transaction-integrity"],
        &["--scope", "all", "--limit", "1"],
        &["--min-severity", "high"],
    ] {
        let out = update_dep_ws(&path, filter);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{filter:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), seed, "{filter:?}");
    }
    let out = update_dep_ws(&path, &["--single-app"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("al-sem: baseline was written by a "),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
}

/// The success case: the baseline file is written, and stdout equals a plain
/// `--baseline` run over the file just written (the set applied is the set
/// written, as before the move), but for the update's own counts
/// (`payload.summary.baselineUpdate`, TLS-106), which only the update reports.
#[test]
fn update_baseline_write_success() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("b.json");
    let path = path.to_str().unwrap();
    let updated = analyze(path, true);
    assert_eq!(
        updated.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    let written = std::fs::read_to_string(path).expect("baseline written");
    let v: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert!(
        !v["fingerprints"].as_array().unwrap().is_empty(),
        "precondition: ws-d1 has findings"
    );
    let plain = analyze(path, false);
    assert_eq!(plain.status.code(), Some(0));
    let mut updated: serde_json::Value = serde_json::from_slice(&updated.stdout).unwrap();
    let counts = updated["payload"]["summary"]
        .as_object_mut()
        .unwrap()
        .remove("baselineUpdate")
        .expect("the update's counts");
    assert_eq!(
        counts["written"],
        v["fingerprints"].as_array().unwrap().len()
    );
    let plain: serde_json::Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(updated, plain);
    assert_eq!(std::fs::read_to_string(path).unwrap(), written);
}
