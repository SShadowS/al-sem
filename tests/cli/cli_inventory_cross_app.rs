//! #160 step 9 D4: `alsem fingerprint --inventory-only` reads the five
//! inventory keys off a tree composed with no raw facts, cross-app by default
//! (`--single-app`: the workspace alone). Spec
//! `docs/superpowers/specs/2026-10-24-issue-160-step9-cross-app-prove-fingerprint-design.md`
//! §5, inventory arms: T7, T8, T13, T22 (T11 is a lib test,
//! `inventory_reads_a_tree_with_no_raw_facts`; its counter half is
//! `cli_build_counters::fingerprint_inventory_only`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const VERSION_OVERRIDE: &str = "cli-inv-x-v1";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `alsem fingerprint <ws> --inventory-only --format json --deterministic [extra]`
/// -> (stdout, stderr, exit code).
fn inventory(ws: &Path, extra: &[&str]) -> (String, String, i32) {
    let ws = ws.to_string_lossy();
    let mut a = vec![
        "fingerprint",
        &ws,
        "--inventory-only",
        "--format",
        "json",
        "--deterministic",
    ];
    a.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&a)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).replace('\r', ""),
        out.status.code().unwrap_or(-1),
    )
}

/// T7 (inventory arm, E9), spec §5: over every r0 dir with an `app.json` and
/// no dependency, the cross-app inventory equals the `--single-app` one byte
/// for byte (stdout, stderr diagnostics, exit code), apart from the
/// `analysisUniverse` value. In-process through `run_fingerprint_pipeline`.
/// DISCRIMINATION: B1, a cross-app-only analyzer diagnostic, fails it; B2, a
/// class filter that drops one primary `rootClassifications` row
/// (`filter_by_class`), fails it.
#[test]
fn no_dependency_parity_inventory_only() {
    use al_sem::engine::l5::fingerprint_cli::{
        FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
    };
    let dirs = crate::cli_prove_cross_app::no_dependency_dirs();
    assert!(dirs.len() >= 180, "precondition: dirs ({})", dirs.len());
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for dir in &dirs {
        let inputs =
            al_sem::program::inputs::WorkspaceInputs::read(dir, &crate::fingerprint_needs());
        let run = |single_app: bool| {
            let opts = FingerprintOptions {
                inputs: &inputs,
                driver_version: "t",
                format: FingerprintFormat::Json,
                out: None,
                shard: None,
                witness_limit: None,
                roots: None,
                routine_selectors: Vec::new(),
                include_inherited: true,
                is_query_requested: false,
                deterministic: true,
                strict: false,
                verbosity: "compact",
                inventory_only: true,
                no_roots_config: false,
                single_app,
            };
            run_fingerprint_pipeline(&opts).map(|r| {
                let text = match r.output {
                    FingerprintOutput::Text(t) => t,
                    _ => String::from("<binary>"),
                };
                (text, r.exit_code, r.stderr_diagnostics)
            })
        };
        let (cross, single) = (run(false), run(true));
        let cross = cross.map(|(t, c, d)| {
            (
                t.replacen(
                    "\"analysisUniverse\": \"cross-app\"",
                    "\"analysisUniverse\": \"single-app\"",
                    1,
                ),
                c,
                d,
            )
        });
        if cross != single {
            failures.push(dir.display().to_string());
        }
        compared += 1;
    }
    assert!(compared >= 180, "precondition: dirs compared ({compared})");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T8 (inventory arm), spec §5: on the dependency fixture the universes
/// differ (precondition), and `--single-app` equals the base's default output
/// byte for byte (a golden minted from the base binary `1134bed1`; the
/// inventory allows no difference). DISCRIMINATION: ignoring the flag fails
/// the precondition; corrupting the single-app output fails the golden.
#[test]
fn single_app_flag_reaches_inventory_only() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own(tmp.path(), "");
    let (cross, _, c1) = inventory(tmp.path(), &[]);
    let (single, err, c2) = inventory(tmp.path(), &["--single-app"]);
    assert_eq!((c1, c2), (0, 0), "{err}");
    assert_ne!(cross, single, "precondition: the universes differ");
    // Minted from the base binary, so never regenerated.
    let path = repo().join("tests/cli-b-goldens/single-app/inventory.dd.json");
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    assert_eq!(single, want, "--single-app != the base's default");
}

/// T13 (inventory arm), spec §5: `ws-d51-jobqueue`'s `roots.config.json` gives
/// a classification a `configEntryId` (precondition, without the flag).
/// Assertion: cross-app with `--no-roots-config`, no classification has one
/// (the inventory carries no `inputsMetadata`). DISCRIMINATION: `build_with`
/// dropping the parameter fails it.
#[test]
fn inventory_only_no_roots_config_cross_app() {
    let ws = repo().join("tests/r0-corpus/ws-d51-jobqueue");
    let config_ids = |doc: &Value| -> usize {
        doc["payload"]["rootClassifications"]
            .as_array()
            .map_or(0, |a| {
                a.iter()
                    .filter(|c| c.get("configEntryId").is_some())
                    .count()
            })
    };
    let (with, _, c) = inventory(&ws, &[]);
    assert_eq!(c, 0);
    let with: Value = serde_json::from_str(&with).unwrap();
    assert!(config_ids(&with) > 0, "precondition: {with}");
    let (without, _, c) = inventory(&ws, &["--no-roots-config"]);
    assert_eq!(c, 0);
    let without: Value = serde_json::from_str(&without).unwrap();
    assert_eq!(without["payload"]["analysisUniverse"], "cross-app");
    assert_eq!(config_ids(&without), 0, "{without}");
}

/// T22 (inventory arm), spec §5: the r0 dirs with no `app.json` keep their
/// error, `fingerprint: could not compute modelInstanceId`, exit 1, in both
/// universes. DISCRIMINATION: a different failure text on the new path fails it.
#[test]
fn no_app_json_keeps_its_error_inventory_only() {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(repo().join("tests/r0-corpus"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.join("app.json").exists())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 9, "precondition: the 9 dirs: {dirs:?}");
    for dir in &dirs {
        for extra in [&[][..], &["--single-app"][..]] {
            let (_, err, code) = inventory(dir, extra);
            assert_eq!(code, 1, "{dir:?} {extra:?}");
            assert_eq!(
                err.trim_end(),
                "fingerprint: could not compute modelInstanceId",
                "{dir:?} {extra:?}"
            );
        }
    }
}

/// #160 step 9 (Task 4 review item 2): a content oracle for the cross-app
/// inventory with a dependency. Fixture: the workspace's `Ws Runner.OnRun`
/// runs the dependency's `Dep Runner`, whose own `OnRun` is a classified root
/// in the cross-app model (precondition, read off the model). The golden was
/// minted from the head and triaged line by hand (ledger): every
/// `rootClassifications`, `coverage` and `routineInventory` row is primary; the
/// dependency identities are the ones primary rows reference; `apps` gains
/// the `XBase` dependency row. DISCRIMINATION: `filter_by_class` keeping
/// every classification (a dependency one leaks) fails it.
#[test]
fn cross_app_inventory_matches_its_golden() {
    let tmp = tempfile::tempdir().unwrap();
    crate::cli_fingerprint_routine_cross_app::write_two_on_runs(tmp.path());
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(tmp.path(), &Default::default());
    let t = al_sem::engine::gate::run::AnalysisTarget::build(&inputs, false, "t").expect("cross");
    let m = t.resolved();
    assert!(
        m.root_classifications.iter().any(|rc| m
            .workspace
            .routines
            .iter()
            .any(|r| r.id == rc.routine_id
                && m.routine_role(r) == al_sem::program::model::role::Role::Dependency)),
        "precondition: the model classifies a dependency routine"
    );
    let (got, err, code) = inventory(tmp.path(), &[]);
    assert_eq!(code, 0, "{err}");
    // Minted from the head and triaged by hand (ledger), so never regenerated.
    let path = repo().join("tests/cli-b-goldens/cross-app/inventory.two-onruns.json");
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    assert_eq!(got, want);
}
