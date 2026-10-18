//! #160 step 1 (spec `2026-10-18-issue-160-steps-0-2-design.md` §4.2.3, §8.2):
//! a command reads its workspace inputs ONCE, into an `InputSet`, and every
//! reader after that reads the set, never the disk. Each test reads the set,
//! then edits the disk, then runs the pipeline with the earlier set: the output
//! equals an untouched run's. In-process, because the edit must land BETWEEN the
//! read and the build.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
};
use al_sem::program::inputs::InputSet;

fn r0(ws: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws)
}

fn read(ws: &Path) -> Arc<InputSet> {
    InputSet::read(ws, &Default::default())
}

/// `analyze --format json --deterministic` over `inputs` (the harness call of
/// `cli_a_json_differential.rs`).
fn analyze_json(inputs: &Arc<InputSet>) -> String {
    let args = AnalyzeArgs {
        workspace: inputs.workspace.to_string_lossy().into_owned(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        with_evidence: false,
        single_app: false,
    };
    match run_analyze_with_exit(inputs, &args, "engine-default") {
        Ok((out, _, _)) => out,
        Err(e) => panic!("analyze failed: {e}"),
    }
}

/// `fingerprint --format json --deterministic` over `inputs` (the harness call
/// of `cli_b_fingerprint_differential.rs`), parsed.
fn fingerprint_json(inputs: &Arc<InputSet>) -> serde_json::Value {
    let opts = FingerprintOptions {
        inputs,
        driver_version: "engine-default",
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
        inventory_only: false,
        no_roots_config: false,
    };
    let r = run_fingerprint_pipeline(&opts).expect("fingerprint pipeline");
    match r.output {
        FingerprintOutput::Text(t) => serde_json::from_str(&t).expect("fingerprint json"),
        _ => panic!("expected text output"),
    }
}

/// Spec §8.2 `inputs::al_file_added_after_read`: before step 1,
/// `select_program_files` walked the disk again during model assembly, listed
/// the new file, missed it in the program's parse and failed the model closed
/// (`program parse lacks`).
#[test]
fn al_file_added_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d1-multi-caller"), d.path());
    let untouched = analyze_json(&read(d.path()));
    let v: serde_json::Value = serde_json::from_str(&untouched).expect("json");
    assert!(
        v["payload"]["findings"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "precondition: the untouched run has findings:\n{untouched}"
    );
    let inputs = read(d.path());
    std::fs::write(
        d.path().join("src/Added.al"),
        "codeunit 50199 Added { procedure P() begin end; }",
    )
    .unwrap();
    assert_eq!(analyze_json(&inputs), untouched);
}

/// Spec §8.2 `inputs::roots_config_edit_after_read`: the root classifications
/// come from the `roots.config.json` bytes read with the set, not a later
/// disk read (`finish_resolved` -> `compute_root_classifications`). The
/// `inputs[]` rows are `derive_inputs`', rewired in plan Task 12.
#[test]
fn roots_config_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d51-jobqueue"), d.path());
    let untouched = fingerprint_json(&read(d.path()));
    let roots = &untouched["payload"]["rootClassifications"];
    assert!(
        roots
            .as_array()
            .is_some_and(|a| a.iter().any(|r| r["source"] != "ast")),
        "precondition: the config moves a classification: {roots}"
    );
    let inputs = read(d.path());
    std::fs::remove_file(d.path().join("roots.config.json")).unwrap();
    assert_eq!(
        fingerprint_json(&inputs)["payload"]["rootClassifications"],
        *roots
    );
}
