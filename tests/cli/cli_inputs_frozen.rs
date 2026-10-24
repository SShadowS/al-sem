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
use al_sem::program::inputs::{InputNeeds, InputSet, PolicyRequest};

fn r0(ws: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws)
}

fn read(ws: &Path) -> Arc<InputSet> {
    InputSet::read(ws, &Default::default())
}

/// What `fingerprint` reads: the hash rows too.
fn read_fingerprint(ws: &Path) -> Arc<InputSet> {
    InputSet::read(ws, &crate::fingerprint_needs())
}

/// `analyze --deterministic` arguments (the harness shape of
/// `cli_a_json_differential.rs`).
fn analyze_args(ws: &Path, format: OutputFormat) -> AnalyzeArgs {
    AnalyzeArgs {
        workspace: ws.to_string_lossy().into_owned(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format,
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
    }
}

/// Run `analyze` over `inputs`: stdout and exit code.
fn analyze_run(inputs: &Arc<InputSet>, args: &AnalyzeArgs) -> (String, u8) {
    match run_analyze_with_exit(inputs, args, "engine-default") {
        Ok((out, exit, _)) => (out, exit),
        Err(e) => panic!("analyze failed: {e}"),
    }
}

/// `analyze --format <format>` over `inputs`.
fn analyze(inputs: &Arc<InputSet>, format: OutputFormat) -> String {
    analyze_run(inputs, &analyze_args(&inputs.workspace, format)).0
}

/// `analyze --format json --deterministic` over `inputs`.
fn analyze_json(inputs: &Arc<InputSet>) -> String {
    analyze(inputs, OutputFormat::Json)
}

fn findings(json: &str) -> Vec<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(json).expect("analyze json");
    v["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .clone()
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
        single_app: true,
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
/// disk read (`finish_resolved` -> `compute_root_classifications`), and so do
/// the `inputs[]` rows (`derive_inputs`' `roots-config` row).
#[test]
fn roots_config_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d51-jobqueue"), d.path());
    let untouched = fingerprint_json(&read_fingerprint(d.path()));
    let roots = &untouched["payload"]["rootClassifications"];
    assert!(
        roots
            .as_array()
            .is_some_and(|a| a.iter().any(|r| r["source"] != "ast")),
        "precondition: the config moves a classification: {roots}"
    );
    let rows = &untouched["payload"]["inputs"];
    assert!(
        rows.as_array()
            .is_some_and(|a| a.iter().any(|r| r["kind"] == "roots-config")),
        "precondition: a roots-config row: {rows}"
    );
    let inputs = read_fingerprint(d.path());
    std::fs::remove_file(d.path().join("roots.config.json")).unwrap();
    let after = fingerprint_json(&inputs);
    assert_eq!(after["payload"]["rootClassifications"], *roots);
    assert_eq!(after["payload"]["inputs"], *rows);
}

/// Spec §8.2 `inputs::app_json_edit_after_read`: one edit of `app.json`
/// reaches every reader of it: the model id, the primary app, the workspace
/// diagnostics, pr-summary's app registry (`read_workspace_apps`) and the
/// fingerprint's `app-json` row. Whole outputs are compared, the envelope's
/// `diagnostics` array included.
#[test]
fn app_json_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d1-multi-caller"), d.path());
    let json0 = analyze_json(&read_fingerprint(d.path()));
    let pr0 = analyze(&read_fingerprint(d.path()), OutputFormat::PrSummary);
    let fp0 = fingerprint_json(&read_fingerprint(d.path()));
    assert!(!findings(&json0).is_empty(), "precondition: findings");
    let inputs = read_fingerprint(d.path());
    std::fs::write(d.path().join("app.json"), "{}").unwrap();
    assert_eq!(
        analyze_json(&inputs),
        json0,
        "json (model id / primary app / diagnostics)"
    );
    assert_eq!(
        analyze(&inputs, OutputFormat::PrSummary),
        pr0,
        "pr-summary (read_workspace_apps)"
    );
    assert_eq!(
        fingerprint_json(&inputs),
        fp0,
        "fingerprint (derive_inputs app-json row / diagnostics)"
    );
}

/// The envelope's `diagnostics` array of an analyze json output.
fn diagnostics(json: &str) -> Vec<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(json).expect("analyze json");
    v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .clone()
}

/// Spec §8.2 `inputs::objectless_file_removed_after_read`: the index row
/// `No object declaration found in <rel>` comes from the program build's own
/// parse (`FreshCoverage::files_without_object`), not a second parse of the
/// disk, so deleting the file after the read leaves the row in place.
#[test]
fn objectless_file_removed_after_read() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("app.json"),
        r#"{"id":"11111111-0000-0000-0000-0000000d1301","name":"Objectless","publisher":"PT","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(
        d.path().join("src/A.al"),
        "codeunit 50100 A { procedure P() begin end; }",
    )
    .unwrap();
    std::fs::write(d.path().join("src/Empty.al"), "// only a comment\n").unwrap();
    let want = "No object declaration found in src/Empty.al";
    let untouched = analyze_json(&read(d.path()));
    assert!(
        diagnostics(&untouched)
            .iter()
            .any(|x| x["message"] == want && x["code"] == "DIAG-index"),
        "precondition: the untouched run has the index row:\n{untouched}"
    );
    let inputs = read(d.path());
    std::fs::remove_file(d.path().join("src/Empty.al")).unwrap();
    assert_eq!(analyze_json(&inputs), untouched);
}

/// Spec §8.2 `inputs::app_json_paths_edit_after_read`: the multi-app check
/// reads the `app.json` list of the set, so a nested `app.json` written after
/// the read adds no multi-app error.
#[test]
fn app_json_paths_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d1-multi-caller"), d.path());
    let untouched = analyze_json(&read(d.path()));
    let inputs = read(d.path());
    std::fs::create_dir_all(d.path().join("sub")).unwrap();
    std::fs::write(d.path().join("sub/app.json"), "{}").unwrap();
    assert!(
        diagnostics(&analyze_json(&read(d.path())))
            .iter()
            .any(|x| x["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("multi-app source workspace unsupported"))),
        "precondition: a fresh read sees the second app.json"
    );
    assert_eq!(analyze_json(&inputs), untouched);
}

/// Spec §8.2 `inputs::coverage_yaml_edit_after_read`: the `policy` input row
/// hashes the `al-sem.coverage.yaml` bytes read with the set.
#[test]
fn coverage_yaml_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d1-multi-caller"), d.path());
    let yaml = d.path().join("al-sem.coverage.yaml");
    std::fs::write(&yaml, "version: 1\n").unwrap();
    let untouched = fingerprint_json(&read_fingerprint(d.path()));
    let policy_row = |v: &serde_json::Value| {
        v["payload"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "policy")
            .cloned()
    };
    assert!(
        policy_row(&untouched).is_some(),
        "precondition: a policy row"
    );
    let inputs = read_fingerprint(d.path());
    std::fs::write(&yaml, "version: 2\n").unwrap();
    let after = fingerprint_json(&inputs);
    assert_eq!(
        after["payload"]["workspaceFingerprint"],
        untouched["payload"]["workspaceFingerprint"]
    );
    assert_eq!(policy_row(&after), policy_row(&untouched));
}

/// Spec §8.2 `inputs::al_text_edit_after_read`: inline suppressions are parsed
/// from the `.al` text read with the set. The directive names the finding's
/// detector and sits at the END of the finding's line, so no line shifts and
/// a disk read would suppress the finding.
#[test]
fn al_text_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(&r0("ws-d14-dead-routine"), d.path());
    let first = findings(&analyze_json(&read(d.path())));
    let f = &first[0];
    let det = f["detector"].as_str().unwrap().to_string();
    let file = f["primaryLocation"]["file"].as_str().unwrap();
    let rel = file.strip_prefix("ws:").expect("a workspace finding");
    let line = f["primaryLocation"]["line"].as_u64().unwrap() as usize;
    let inputs = read(d.path());
    let p = d.path().join(rel);
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    lines[line - 1].push_str(&format!(" // al-sem-ignore {det}"));
    std::fs::write(&p, lines.join("\n")).unwrap();
    assert_eq!(findings(&analyze_json(&inputs)).len(), first.len());
}

/// Spec §8.2 `inputs::closure_app_replaced_after_read`: the `dep-package` row
/// hashes the `.app` bytes read with the set. Stated limit: it pins the HASH
/// rows only. The loader still opens the `.app` itself after the hash (spec
/// §4.2.2), so a swap between the two reads gives a model from the new bytes
/// under the old hash: today's race, unchanged.
#[test]
fn closure_app_replaced_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::cli_digest_query_dependencies::write_dependency_workspace(d.path());
    let untouched = fingerprint_json(&read_fingerprint(d.path()));
    let rows = &untouched["payload"]["inputs"];
    assert!(
        rows.as_array()
            .is_some_and(|a| a.iter().any(|r| r["kind"] == "dep-package")),
        "precondition: a dep-package row: {rows}"
    );
    let inputs = read_fingerprint(d.path());
    // The same app, symbol-only: different bytes, still a loadable package.
    crate::cli_digest_query_dependencies::symbol_only_dependency(d.path());
    let after = fingerprint_json(&inputs);
    assert_eq!(
        after["payload"]["workspaceFingerprint"],
        untouched["payload"]["workspaceFingerprint"]
    );
    assert_eq!(after["payload"]["inputs"], *rows);
}

/// `policy check --format json` over `inputs`.
fn policy_check_json(inputs: &Arc<InputSet>) -> Option<String> {
    use al_sem::engine::gate::policy::pipeline::{PolicyCheckOptions, run_policy_check};
    run_policy_check(&PolicyCheckOptions {
        inputs,
        format: "json",
        out: None,
        deterministic: true,
        strict: false,
        driver_version: "t",
        single_app: false,
    })
    .text
}

/// Code review CR5: the policy mode comes from the set alone. A set read with
/// no policy request would hold no policy file although the workspace has one,
/// and the check would silently run the bundled default: it is refused.
#[test]
#[should_panic(expected = "without a policy request")]
fn policy_check_refuses_a_set_read_without_a_policy_request() {
    let ws =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli-c-policy/ws-policy-custom");
    assert!(ws.join("al-sem.policy.yaml").exists(), "precondition");
    policy_check_json(&InputSet::read(&ws, &InputNeeds::default()));
}

/// Spec §8.2 `inputs::policy_edit_after_read`: the policy is loaded from the
/// bytes read with the set (auto-detected `al-sem.policy.yaml`).
#[test]
fn policy_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    crate::copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli-c-policy/ws-policy-custom"),
        d.path(),
    );
    let read_auto = || {
        InputSet::read(
            d.path(),
            &InputNeeds {
                policy: PolicyRequest::AutoDetect,
                ..InputNeeds::default()
            },
        )
    };
    let untouched = policy_check_json(&read_auto()).expect("policy output");
    assert!(
        untouched.contains("cust-all-rule"),
        "precondition: the workspace policy was loaded"
    );
    let inputs = read_auto();
    std::fs::write(
        d.path().join("al-sem.policy.yaml"),
        "version: [not a policy\n",
    )
    .unwrap();
    assert_eq!(
        policy_check_json(&inputs).as_deref(),
        Some(untouched.as_str())
    );
}

/// `ws-d8-commit-in-tx` under the transaction preset, with `--baseline`.
fn d8_args(dir: &Path, baseline: &Path, update: bool) -> AnalyzeArgs {
    AnalyzeArgs {
        preset: Some("transaction-integrity".to_string()),
        baseline: Some(baseline.to_string_lossy().into_owned()),
        update_baseline: update,
        ..analyze_args(dir, OutputFormat::Json)
    }
}

/// Spec §8.2 `inputs::baseline_edit_after_read`: the baseline applied is the
/// file as read with the set (here: absent, so empty), not a later disk read.
#[test]
fn baseline_edit_after_read() {
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("baseline.json");
    let ws = r0("ws-d8-commit-in-tx");
    let args = d8_args(&ws, &base, false);
    let first = findings(&analyze_run(&args.read_inputs(), &args).0);
    assert!(!first.is_empty(), "precondition: findings");
    let inputs = args.read_inputs();
    let all: Vec<&str> = first
        .iter()
        .map(|f| f["fingerprint"].as_str().unwrap())
        .collect();
    std::fs::write(
        &base,
        serde_json::json!({"schemaVersion": "1", "generatedAt": "x", "fingerprints": all})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        findings(&analyze_run(&args.read_inputs(), &args).0).len(),
        0,
        "precondition: the rewritten baseline suppresses everything"
    );
    assert_eq!(findings(&analyze_run(&inputs, &args).0).len(), first.len());
}

/// A1.6 `baseline::update_baseline_applies_the_set_just_written`: with
/// `--update-baseline` the set applied is the set just written, so every
/// current finding is dropped, whatever the old file held: (a) no file,
/// (b) a malformed file (never read, so never an error).
#[test]
fn update_baseline_applies_the_set_just_written() {
    let ws = r0("ws-d8-commit-in-tx");
    let d = tempfile::tempdir().unwrap();
    let probe = d8_args(&ws, &d.path().join("none.json"), false);
    assert!(
        !findings(&analyze_run(&probe.read_inputs(), &probe).0).is_empty(),
        "precondition: findings without a baseline"
    );
    for (case, old) in [("no file", None), ("malformed", Some("not json"))] {
        let base = d.path().join(format!("{}.json", case.replace(' ', "-")));
        if let Some(text) = old {
            std::fs::write(&base, text).unwrap();
        }
        let args = d8_args(&ws, &base, true);
        let (out, exit) = analyze_run(&args.read_inputs(), &args);
        assert_eq!(exit, 0, "{case}");
        assert_eq!(findings(&out).len(), 0, "{case}");
    }
}

/// Final panel r1 (F2-1): analyze's JSON `alsemVersion` is the version the set
/// captured (`inputs.driver_version`, §4.2.1), never a second environment read.
/// The precondition is stated by assignment: the set's version differs from any
/// version the environment could give. Both JSON paths: findings (a model) and
/// the empty output (no `app.json`, so no model instance id).
#[test]
fn analyze_json_reports_the_captured_driver_version() {
    let empty = tempfile::tempdir().unwrap();
    for (case, ws) in [
        ("findings", r0("ws-d1-multi-caller")),
        ("empty", empty.path().to_path_buf()),
    ] {
        let mut inputs = read(&ws);
        Arc::get_mut(&mut inputs).expect("one owner").driver_version = "captured-160".to_string();
        let v: serde_json::Value =
            serde_json::from_str(&analyze_json(&inputs)).expect("analyze json");
        assert_eq!(v["alsemVersion"], "captured-160", "{case}");
    }
}
