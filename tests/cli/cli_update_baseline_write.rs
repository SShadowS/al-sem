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
/// drops dependency findings itself at `--scope primary`; the update still
/// sees them (the scope tail's `scoped_out`), so every live entry is checked
/// and kept. A dependency entry whose finding is gone is kept by the primary
/// run (out of scope) and dropped only by a `--scope all` run. Through the
/// binary, on the file's bytes; the counts on stderr and in the JSON.
///
/// Mutation (2026-11-20): `.chain(run.scoped_out ...)` removed from
/// `analyze_view`'s `before_filters` -> the live dependency entries read
/// "out of scope" on stderr and in `keptUnchecked` (red); restored, green.
/// Mutation (2026-11-20): `prunes_dependency: true` in `analyze_view`'s
/// `UpdateRun` -> the default update drops the stale dependency entry (red);
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
    let fps = serde_json::from_str::<serde_json::Value>(&seed).unwrap()["fingerprints"]
        .as_array()
        .unwrap()
        .len();
    let out = update_dep_ws(&path, &["--format", "json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["payload"]["summary"]["baselineUpdate"],
        serde_json::json!({"written": fps, "dropped": 0, "added": 0, "keptUnchecked": {}})
    );

    // A gone dependency entry: kept by the primary run, dropped by `--scope all`.
    let stale = with_stale(&seed, "d14-dead-routine", "dependency");
    std::fs::write(&path, &stale).unwrap();
    let (code, counts) = update_json(DEP_WS, &path, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), stale);
    assert_eq!(
        counts["keptUnchecked"],
        serde_json::json!({"outOfScope": 1})
    );
    let (code, counts) = update_json(DEP_WS, &path, &["--scope", "all"]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(counts["dropped"], 1);
}

/// TLS-106 (review, sol P1): scope drift. An entry recorded `primary` whose
/// finding the primary-scope tail now drops is still present (presence is
/// tested before the scope tail), so a default update keeps it and gives it
/// the run's own scope.
///
/// Mutation (2026-11-20): `.chain(run.scoped_out ...)` removed from
/// `analyze_view`'s `before_filters` -> the drifted entry is pruned (red);
/// restored, green.
#[test]
fn a_fingerprint_only_the_scope_tail_dropped_keeps_its_entry() {
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seeded(&dir);
    let dep = "\"scope\": \"dependency\"";
    assert!(seed.contains(dep), "precondition: a dependency entry");
    let drifted = seed.replacen(dep, "\"scope\": \"primary\"", 1);
    std::fs::write(&path, &drifted).unwrap();
    let (code, counts) = update_json(DEP_WS, &path, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(counts["dropped"], 0);
}

const STALE: &str = "0000000000000000";

/// `analyze <ws> --deterministic --baseline <base> --update-baseline --format
/// json <extra>`: its exit and `payload.summary.baselineUpdate`.
fn update_json(ws: &str, base: &str, extra: &[&str]) -> (Option<i32>, serde_json::Value) {
    let mut args = vec![
        "analyze",
        ws,
        "--deterministic",
        "--baseline",
        base,
        "--update-baseline",
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    let out = alsem(&args);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "no JSON (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (
        out.status.code(),
        v["payload"]["summary"]["baselineUpdate"].clone(),
    )
}

/// A seeded baseline of `ws` (an update from no file), its path and bytes.
fn seed_of(dir: &tempfile::TempDir, ws: &str, extra: &[&str]) -> (String, String) {
    let path = dir.path().join("b.json").to_str().unwrap().to_string();
    let (code, _) = update_json(ws, &path, extra);
    assert_eq!(code, Some(0), "seed");
    let text = std::fs::read_to_string(&path).expect("seeded");
    (path, text)
}

/// `text` with a hand-stated entry `{STALE, detector, scope}` added (its
/// fingerprint sorts first, so both lists take it at their head).
fn with_stale(text: &str, detector: &str, scope: &str) -> String {
    let fps = "  \"fingerprints\": [\n";
    let entries = "  \"entries\": [\n";
    assert_eq!(text.matches(fps).count(), 1, "precondition: fingerprints");
    assert_eq!(text.matches(entries).count(), 1, "precondition: entries");
    text.replacen(fps, &format!("{fps}    \"{STALE}\",\n"), 1)
        .replacen(
            entries,
            &format!(
                "{entries}    {{\n      \"fingerprint\": \"{STALE}\",\n      \"detector\": \
                 \"{detector}\",\n      \"scope\": \"{scope}\"\n    }},\n"
            ),
            1,
        )
}

/// TLS-106 (both reviews): an update prunes. A stale entry of a detector that
/// ran completely, on whole inputs, is dropped; every live entry is kept;
/// `--limit 0` adds nothing. The JSON counts say so.
///
/// Mutation (2026-11-20): `complete:` given an empty set in `analyze_view`'s
/// `UpdateRun` -> the stale entry is kept as "detector incomplete" (red);
/// restored, green.
#[test]
fn a_stale_entry_is_dropped_and_live_ones_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seed_of(&dir, DEP_WS, &[]);
    let fps = serde_json::from_str::<serde_json::Value>(&seed).unwrap()["fingerprints"]
        .as_array()
        .unwrap()
        .len();
    std::fs::write(&path, with_stale(&seed, "d19-unused-parameter", "primary")).unwrap();
    let (code, counts) = update_json(DEP_WS, &path, &["--limit", "0"]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(
        counts,
        serde_json::json!({"written": fps, "dropped": 1, "added": 0, "keptUnchecked": {}})
    );
}

/// TLS-106: a detector the run did not select keeps every entry, live or
/// stale, counted as "detector not run".
///
/// Mutation (2026-11-20): `selected:` given every registered detector in
/// `analyze_view`'s `UpdateRun` -> they read "detector incomplete" (red);
/// restored, green.
#[test]
fn a_detector_not_run_keeps_its_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seed_of(&dir, DEP_WS, &[]);
    let stale = with_stale(&seed, "d19-unused-parameter", "primary");
    let d19 = stale.matches("\"d19-unused-parameter\"").count();
    std::fs::write(&path, &stale).unwrap();
    let (code, counts) = update_json(DEP_WS, &path, &["--detector", "d33-unfiltered-bulk-write"]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), stale);
    assert_eq!(
        counts["keptUnchecked"],
        serde_json::json!({"detectorNotRun": d19})
    );
}

/// TLS-106 (review, Fable 2 and sol P1): a detector whose walk stopped short
/// keeps its entries. A real cut: d45's relay walk stops at 256 subscribers,
/// and this workspace gives its publisher 258.
///
/// Mutation (2026-11-20): `complete:` given the selected set (no
/// `ran_completely`) in `analyze_view`'s `UpdateRun` -> the stale entry is
/// dropped (red); restored, green.
/// Mutation (2026-11-20): d45's `relay_caps += 1` made `+= 0` -> the stale
/// entry is dropped (red); restored, green.
#[test]
fn a_cut_detector_keeps_its_entries() {
    const D45: &str = "d45-event-transitive-table-exposure";
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    crate::copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-event-d45-deep"),
        &ws,
    );
    let ws = ws.to_str().unwrap().to_string();
    let (path, seed) = seed_of(&dir, &ws, &["--detector", D45]);
    std::fs::write(&path, with_stale(&seed, D45, "primary")).unwrap();
    let subscribers: String = (0..256)
        .map(|i| {
            format!(
                "    [EventSubscriber(ObjectType::Codeunit, Codeunit::SalesMgr, 'OnAfterSales', \
                 '', false, false)]\n    local procedure H{i}(var S: Record Sales)\n    begin\n    \
                 end;\n\n"
            )
        })
        .collect();
    std::fs::write(
        Path::new(&ws).join("src/Many.al"),
        format!("codeunit 50050 ManySubscribers\n{{\n{subscribers}}}\n"),
    )
    .unwrap();
    let (code, counts) = update_json(&ws, &path, &["--detector", D45]);
    assert_eq!(code, Some(0));
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains(STALE),
        "the stale entry is kept: {written}"
    );
    let kept = counts["keptUnchecked"].as_object().unwrap();
    assert!(
        kept.keys().all(|k| k == "detectorIncomplete")
            && kept["detectorIncomplete"].as_u64() >= Some(1),
        "{counts}"
    );
}

/// TLS-106: a run whose inputs are not whole (here a file parsed with
/// recovery) keeps every entry it did not see.
///
/// Mutation (2026-11-20): `inputs_whole: true` in `analyze_view`'s
/// `UpdateRun` -> the stale entry is dropped (red); restored, green.
#[test]
fn inputs_not_whole_keep_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    crate::copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join(DEP_WS), &ws);
    let ws = ws.to_str().unwrap().to_string();
    let (path, seed) = seed_of(&dir, &ws, &[]);
    std::fs::write(&path, with_stale(&seed, "d19-unused-parameter", "primary")).unwrap();
    std::fs::write(
        Path::new(&ws).join("src/Broken.al"),
        "codeunit 50999 Broken\n{\n    procedure P()\n    begin\n        if then\n    end;\n}\n",
    )
    .unwrap();
    let (code, counts) = update_json(&ws, &path, &[]);
    assert_eq!(code, Some(0));
    assert!(std::fs::read_to_string(&path).unwrap().contains(STALE));
    assert!(
        counts["keptUnchecked"]["inputsNotWhole"].as_u64() >= Some(1),
        "{counts}"
    );
}

/// TLS-106 (review, sol P1): an app opaque now that the file did not record
/// lost bodies, so the run prunes nothing. Opacity the file recorded stays
/// allowed: the same stale entry is dropped beside it.
///
/// Mutation (2026-11-20): the run's opaque apps passed empty
/// (`.take(0)`) in `analyze_view`'s `UpdateRun` -> the seed records none, the
/// precondition fails (red); restored, green. The comparison itself is pinned
/// by `baseline::tests::a_newly_opaque_app_stops_pruning`.
#[test]
fn a_newly_opaque_app_keeps_entries() {
    const WS: &str = "tests/r0-corpus/ws-cross-object-chain";
    let dir = tempfile::tempdir().unwrap();
    let (path, seed) = seed_of(&dir, WS, &[]);
    let recorded = "  \"opaqueApps\": [\n    \"CrossChainDep\",\n    \"CrossChainDep2\"\n  ],\n";
    assert!(
        seed.contains(recorded),
        "precondition: two opaque apps: {seed}"
    );
    // Recorded opacity: the stale entry is dropped.
    std::fs::write(&path, with_stale(&seed, "d19-unused-parameter", "primary")).unwrap();
    let (code, counts) = update_json(WS, &path, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    assert_eq!(counts["dropped"], 1);
    // The file recorded none: the run's opaque apps are new; it keeps.
    let unrecorded = with_stale(
        &seed.replace(recorded, "  \"opaqueApps\": [],\n"),
        "d19-unused-parameter",
        "primary",
    );
    std::fs::write(&path, &unrecorded).unwrap();
    let (code, counts) = update_json(WS, &path, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), unrecorded);
    assert_eq!(
        counts["keptUnchecked"],
        serde_json::json!({"inputsNotWhole": 1})
    );
}

/// TLS-106 (review, sol P2): the file an update merges into is checked before
/// any analysis, so a malformed one is refused (exit 2, nothing written) even
/// when the workspace yields no model.
///
/// Mutation (2026-11-20): the `check_update_file` call removed from
/// `AnalyzeJob::new` -> exit 0 (red); restored, green.
#[test]
fn a_malformed_baseline_is_refused_before_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("empty");
    std::fs::create_dir(&ws).unwrap();
    let path = dir.path().join("b.json");
    std::fs::write(&path, "not json").unwrap();
    let out = alsem(&[
        "analyze",
        ws.to_str().unwrap(),
        "--baseline",
        path.to_str().unwrap(),
        "--update-baseline",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("al-sem: analysis failure — baseline"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
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
