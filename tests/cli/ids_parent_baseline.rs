//! Parent-baseline equality for the routine-id migration (context-server design
//! r3 §6.7, T-ids).
//!
//! The internal routine id is `"{prefix}/{64 lowercase hex}"`. The migration
//! changes only the prefix: it used to be the workspace's `modelInstanceId`
//! (a hash over the sorted `.al` file list), and becomes a per-app scope (16 hex
//! of sha256 over the app guid). Nothing a user keeps may move with it:
//!
//! - **finding fingerprints** — what a `--baseline` file stores and what SARIF's
//!   `fingerprints` (`al-sem/v1`) carries. `l5::fingerprint` hashes the `rootCauseKey`
//!   with every internal id swapped for its stable image, by scanning for
//!   `{prefix}/{64 hex}`;
//! - **stable ids** — `{stableObjectId}#{64 hex}` and the stable sub-ids
//!   (`…#{hex}/csN`, `…/opN`) that `stable_sub_id` builds by splitting an
//!   internal sub-id at its SECOND `/`. They reach analyze's `--with-evidence`
//!   JSON, digest and prove output.
//!
//! `tests/ids-parent-baseline/record.json` was MINTED AT THE PARENT COMMIT of the
//! migration (before the encoding changed), and every run compares today's
//! engine against it:
//!
//! 1. `parent_baseline_fingerprints_and_stable_ids_are_unchanged`: per fixture
//!    app, the sorted SARIF `ruleId|fingerprint` list (every registered detector,
//!    `--scope all`) and every stable routine id and stable sub-id occurring in
//!    analyze `--format json --with-evidence` and the all-files digest are equal
//!    to the record.
//! 2. `parent_minted_baselines_still_suppress_every_finding`: the `--baseline`
//!    file the parent minted (`--update-baseline`) for each fixture with findings
//!    still suppresses every finding today (zero SARIF results), and the
//!    unbaselined run still finds the same number.
//!
//! The record is NOT a golden: `REGEN_TEMP_GOLDENS` / `check-goldens --regen`
//! never rewrite it, on purpose — a regen that moved a fingerprint would
//! otherwise re-bless the very regression this test exists to catch. It is
//! re-minted only by hand, with `IDS_PARENT_BASELINE_MINT=<commit>`, and only at
//! a commit whose fingerprints are known good.
//!
//! A recorded string is a stable id and nothing else (`stable_id_re`), so the
//! record cannot move for the reason the migration legitimately moves other
//! output: internal ids and the hashes derived from them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const VERSION: &str = "ids-parent-baseline-v1";
const MINT_ENV: &str = "IDS_PARENT_BASELINE_MINT";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureRecord {
    /// Sorted `ruleId|<fingerprints as key=value,…>` per SARIF result.
    findings: Vec<String>,
    /// Sorted, deduplicated stable ids and stable sub-ids occurring in analyze's
    /// `--with-evidence` JSON and the all-files digest JSON.
    stable_ids: Vec<String>,
    /// The `--update-baseline` bytes the minting commit wrote, when the fixture
    /// has findings (`None` otherwise).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    baseline: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The commit the record was minted at (the migration's parent).
    minted_at: String,
    fixtures: BTreeMap<String, FixtureRecord>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn record_path() -> PathBuf {
    repo_root()
        .join("tests")
        .join("ids-parent-baseline")
        .join("record.json")
}

/// Every fixture app: each `tests/r0-corpus/*` and `tests/fixtures/*` directory
/// with a root `app.json` (the analyzer's precondition). Several carry
/// `.alpackages` dependencies, so cross-app models with more than one app —
/// hence more than one id prefix after the migration — are in the set.
fn fixture_apps() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for family in ["r0-corpus", "fixtures"] {
        let dir = repo_root().join("tests").join(family);
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.is_dir() && p.join("app.json").is_file())
            .collect();
        entries.sort();
        for p in entries {
            let name = format!(
                "{family}/{}",
                p.file_name().expect("dir name").to_string_lossy()
            );
            out.push((name, p));
        }
    }
    assert!(
        out.len() > 200,
        "fixture discovery found only {} apps — the corpus moved?",
        out.len()
    );
    out
}

fn all_detectors() -> String {
    al_sem::engine::l5::detectors::registered_detectors()
        .iter()
        .map(|d| d.name.clone())
        .collect::<Vec<_>>()
        .join(",")
}

fn analyze_args(ws: &Path, format: OutputFormat, baseline: Option<String>) -> AnalyzeArgs {
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(all_detectors()),
        preset: None,
        scope: Scope::All,
        limit: None,
        format,
        sarif_version_override: Some(VERSION.to_string()),
        fail_on: None,
        require_dependencies: false,
        baseline,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        with_evidence: matches!(format, OutputFormat::Json),
        single_app: false,
    }
}

fn analyze(args: &AnalyzeArgs) -> al_sem::engine::gate::run::AnalyzeOutput {
    run_analyze_with_exit(&args.read_inputs(), &args.read_request(), args, VERSION)
        .unwrap_or_else(|e| panic!("{}: analyze failed: {e}", args.workspace))
}

/// A stable routine id or stable sub-id: `{guid}:{Type}:{key}#{64 lowercase hex}`
/// (`ids::to_stable_routine_id_from_parts`), optionally followed by the site
/// suffixes `stable_sub_id` re-attaches (`/cs0`, `/op3`, `/loop2`, …). Only the
/// id itself is recorded, never the text around it: a finding id such as
/// `d47/{stable id}/WRITE_PENDING_AT_EXTERNAL_IO|{occurrence}|{occurrence}|` also
/// carries digest occurrence ids, which are sha256 over a canonical key holding
/// INTERNAL routine and operation ids (`l5::digest::occurrence_id_from_key`) —
/// per-run handles that moved with every file add before the migration and move
/// with it once.
fn stable_id_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"[0-9A-Za-z]{8}-[0-9A-Za-z]{4}-[0-9A-Za-z]{4}-[0-9A-Za-z]{4}-[0-9A-Za-z]{12}:[^#|]*?#[0-9a-f]{64}(?:/[a-z]+[0-9]+)*",
        )
        .expect("regex")
    })
}

fn collect_stable_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => {
            out.extend(stable_id_re().find_iter(s).map(|m| m.as_str().to_string()));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_stable_strings(x, out)),
        serde_json::Value::Object(m) => m.values().for_each(|x| collect_stable_strings(x, out)),
        _ => {}
    }
}

fn sarif_findings(sarif: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(sarif).expect("SARIF is JSON");
    let mut out: Vec<String> = v["runs"][0]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let fps = r["fingerprints"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("?")))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            // Every result carries a fingerprint; an empty one would make the
            // record blind to the very thing it pins.
            assert!(
                !fps.is_empty(),
                "a SARIF result without `fingerprints`: {r}"
            );
            format!("{}|{fps}", r["ruleId"].as_str().unwrap_or("?"))
        })
        .collect();
    out.sort();
    out
}

fn al_files(ws: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == ".alpackages") {
                    continue;
                }
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("al")) {
                let rel = p.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(ws, ws, &mut out);
    out.sort();
    out
}

/// Today's record for one fixture. `with_baseline_bytes` also runs
/// `--update-baseline` (minting only).
fn record_fixture(ws: &Path, with_baseline_bytes: bool) -> FixtureRecord {
    let sarif = analyze(&analyze_args(ws, OutputFormat::Sarif, None)).0;
    let findings = sarif_findings(&sarif);

    let mut stable = Vec::new();
    let json = analyze(&analyze_args(ws, OutputFormat::Json, None)).0;
    collect_stable_strings(
        &serde_json::from_str(&json).expect("analyze json"),
        &mut stable,
    );
    let files = al_files(ws);
    if !files.is_empty() {
        let inputs =
            al_sem::program::inputs::WorkspaceInputs::read(ws, &crate::fingerprint_needs());
        if let Ok(d) = run_digest_pipeline(
            &inputs,
            Some(files),
            None,
            None,
            VERSION,
            true,
            None,
            false,
            false,
        ) && let Ok(v) = serde_json::from_str::<serde_json::Value>(&d.json_text)
        {
            collect_stable_strings(&v, &mut stable);
        }
    }
    stable.sort();
    stable.dedup();

    let baseline = (with_baseline_bytes && !findings.is_empty()).then(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("baseline.json");
        let mut args = analyze_args(ws, OutputFormat::Sarif, Some(path.display().to_string()));
        args.update_baseline = true;
        let bytes = analyze(&args).3.expect("--update-baseline returns bytes");
        String::from_utf8(bytes).expect("baseline is UTF-8")
    });

    FixtureRecord {
        findings,
        stable_ids: stable,
        baseline,
    }
}

fn read_record() -> Record {
    let path = record_path();
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{} is not a record: {e}", path.display()))
}

/// Mint (only with `IDS_PARENT_BASELINE_MINT=<commit>`) or compare.
/// Both tests analyze every fixture in parallel. Their `par_iter` runs on the
/// big-stack pool, as every engine call site that lowers AL source does
/// (`al_sem::big_stack`): on rayon's default worker stack the lowerer
/// overflowed on CI's runners (SIGABRT, "has overflowed its stack").
fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    al_sem::big_stack::big_stack_pool().install(f)
}

#[test]
fn parent_baseline_fingerprints_and_stable_ids_are_unchanged() {
    on_big_stack(fingerprints_and_stable_ids_are_unchanged);
}

fn fingerprints_and_stable_ids_are_unchanged() {
    let fixtures = fixture_apps();
    if let Ok(commit) = std::env::var(MINT_ENV) {
        let fixtures: BTreeMap<String, FixtureRecord> = fixtures
            .par_iter()
            .map(|(name, ws)| (name.clone(), record_fixture(ws, true)))
            .collect();
        let n_findings: usize = fixtures.values().map(|f| f.findings.len()).sum();
        let n_stable: usize = fixtures.values().map(|f| f.stable_ids.len()).sum();
        assert!(
            n_findings > 100 && n_stable > 1000,
            "a degenerate record ({n_findings} findings, {n_stable} stable ids) would \
             prove nothing"
        );
        let record = Record {
            minted_at: commit,
            fixtures,
        };
        let path = record_path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut text = serde_json::to_string_pretty(&record).expect("serialize");
        text.push('\n');
        std::fs::write(&path, text).expect("write record");
        eprintln!(
            "MINTED {} ({n_findings} findings, {n_stable} stable ids)",
            path.display()
        );
        return;
    }

    let record = read_record();
    let mut names: Vec<&String> = fixtures.iter().map(|(n, _)| n).collect();
    names.sort();
    let recorded: Vec<&String> = record.fixtures.keys().collect();
    assert_eq!(
        names, recorded,
        "the fixture set differs from the record minted at {}: re-mint at a commit \
         whose fingerprints are known good",
        record.minted_at
    );

    let failures: Vec<String> = fixtures
        .par_iter()
        .filter_map(|(name, ws)| {
            let want = &record.fixtures[name];
            let got = record_fixture(ws, false);
            let mut why = Vec::new();
            if got.findings != want.findings {
                why.push(format!(
                    "findings: recorded {:?}\n      today {:?}",
                    want.findings, got.findings
                ));
            }
            if got.stable_ids != want.stable_ids {
                let lost: Vec<_> = want
                    .stable_ids
                    .iter()
                    .filter(|s| !got.stable_ids.contains(s))
                    .take(5)
                    .collect();
                let new: Vec<_> = got
                    .stable_ids
                    .iter()
                    .filter(|s| !want.stable_ids.contains(s))
                    .take(5)
                    .collect();
                why.push(format!("stable ids: lost {lost:?}\n      new {new:?}"));
            }
            (!why.is_empty()).then(|| format!("{name}:\n    {}", why.join("\n    ")))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} fixture(s) moved against the record minted at {}:\n{}",
        failures.len(),
        record.minted_at,
        failures.join("\n")
    );
}

/// A baseline the parent minted still suppresses every finding.
#[test]
fn parent_minted_baselines_still_suppress_every_finding() {
    on_big_stack(minted_baselines_still_suppress_every_finding);
}

fn minted_baselines_still_suppress_every_finding() {
    if std::env::var(MINT_ENV).is_ok() {
        return; // the record is being minted by the test above
    }
    let record = read_record();
    let with_baseline: Vec<(&String, &FixtureRecord)> = record
        .fixtures
        .iter()
        .filter(|(_, f)| f.baseline.is_some())
        .collect();
    assert!(
        with_baseline.len() >= 20,
        "only {} fixtures carry a parent-minted baseline — the record is degenerate",
        with_baseline.len()
    );
    let failures: Vec<String> = with_baseline
        .par_iter()
        .filter_map(|(name, f)| {
            let ws = repo_root()
                .join("tests")
                .join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("baseline.json");
            std::fs::write(&path, f.baseline.as_deref().expect("filtered")).expect("write");
            let unbaselined =
                sarif_findings(&analyze(&analyze_args(ws.as_path(), OutputFormat::Sarif, None)).0);
            let kept = sarif_findings(
                &analyze(&analyze_args(
                    ws.as_path(),
                    OutputFormat::Sarif,
                    Some(path.display().to_string()),
                ))
                .0,
            );
            let mut why = Vec::new();
            if unbaselined.len() != f.findings.len() {
                why.push(format!(
                    "{} findings without the baseline, {} at the parent",
                    unbaselined.len(),
                    f.findings.len()
                ));
            }
            if !kept.is_empty() {
                why.push(format!("the parent's baseline let through {kept:?}"));
            }
            (!why.is_empty()).then(|| format!("{name}: {}", why.join("; ")))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "parent-minted baselines no longer suppress (minted at {}):\n{}",
        record.minted_at,
        failures.join("\n")
    );
}
