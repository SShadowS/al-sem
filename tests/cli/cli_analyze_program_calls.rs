//! B3 Phase A, Task 8: `alsem analyze` runs the detectors on the program
//! engine's call resolution (there is no flag any more). Pinned against the
//! detectors run directly over the program-built model
//! (`assemble_and_resolve_workspace_program`): analyze's findings equal them.
//! Until engine-switch S9.6 the reference was the B3 harness
//! (`b3_diff::detector_diff_for_workspace`), which also proved the fixtures'
//! findings differ under L3's own calls; L3 is deleted, so that half is gone.
//!
//! The comparison is structural (detector, file, 1-based line and column,
//! severity, title): analyze builds under the gate model-instance id and the
//! reference under the default one, so routine ids and root-cause keys differ
//! in their id parts by construction.

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;

type Key = (String, String, u64, u64, String, String);

fn harness_keys(fs: &[Finding]) -> Vec<Key> {
    let mut v: Vec<Key> = fs
        .iter()
        .map(|f| {
            let a = &f.primary_location;
            (
                f.detector.clone(),
                a.source_unit_id.clone(),
                u64::from(a.start_line) + 1,
                u64::from(a.start_column) + 1,
                f.severity.clone(),
                f.title.to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

fn args(ws: &Path) -> AnalyzeArgs {
    let all: Vec<String> = registered_detectors().into_iter().map(|d| d.name).collect();
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(all.join(",")),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: true,
        group_by: None,
        deterministic: true,
        with_evidence: false,
        single_app: false,
    }
}

fn analyze_keys(ws: &Path) -> Vec<Key> {
    let (out, _, _) = run_analyze_with_exit(&args(ws), "test").expect("analyze runs");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json output");
    let s =
        |f: &serde_json::Value, p: &str| f.pointer(p).and_then(|x| x.as_str()).unwrap().to_string();
    let n = |f: &serde_json::Value, p: &str| f.pointer(p).and_then(|x| x.as_u64()).unwrap();
    let mut keys: Vec<Key> = v["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|f| {
            (
                s(f, "/detector"),
                s(f, "/primaryLocation/file"),
                n(f, "/primaryLocation/line"),
                n(f, "/primaryLocation/column"),
                s(f, "/severity"),
                s(f, "/title"),
            )
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn analyze_matches_the_detectors_over_the_program_model_on_r0_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    for name in ["ws-member-call-resolution", "ws-overload-callresult-guards"] {
        let ws = root.join(name);
        let model = assemble_and_resolve_workspace_program(&ws, MODEL_INSTANCE_ID_DEFAULT, false)
            .expect("program model");
        let want = harness_keys(&run_detectors(&model, &registered_detectors()).findings);
        assert!(
            !want.is_empty(),
            "{name}: precondition: the fixture has findings"
        );
        assert_eq!(
            analyze_keys(&ws),
            want,
            "{name}: analyze != detectors over the model"
        );
    }
}

/// An unreadable workspace still gives the could-not-verify output (Ok), not
/// an error: the program build fails with the L3 assembly, and the error is
/// only raised when L3 assembles but the program build does not.
#[test]
fn analyze_of_a_missing_workspace_is_still_could_not_verify() {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/__t7_no_such_ws__");
    assert!(!ws.exists());
    assert!(
        run_analyze_with_exit(&args(&ws), "test").is_ok(),
        "a missing workspace is empty output, not an error"
    );
}

/// Engine-switch S8.3: `alsem analyze` is cross-app by default — the workspace with
/// the dependency code it demands — so a call into a dependency's `[InternalProc]`
/// is a d13 finding. `--single-app` analyses the workspace alone, where the
/// dependency routine is not in the model and d13 has nothing to report.
///
/// Discrimination (2026-10-06): making `build_analysis_model` ignore
/// `single_app == false` (always the single-app build) loses the default finding;
/// restored, it passes.
#[test]
fn analyze_is_cross_app_by_default_and_single_app_on_request() {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d13-internal-call");
    let detectors = |a: &AnalyzeArgs| {
        let (out, _, _) = run_analyze_with_exit(a, "test").expect("analyze runs");
        let v: serde_json::Value = serde_json::from_str(&out).expect("json output");
        v["payload"]["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .map(|f| f["detector"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let default = args(&ws);
    assert_eq!(detectors(&default), vec!["d13-cross-app-internal-call"]);
    let single = AnalyzeArgs {
        single_app: true,
        ..args(&ws)
    };
    assert!(detectors(&single).is_empty());
}
