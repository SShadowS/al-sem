//! B3 Phase A, Task 8: `alsem analyze` runs the detectors on the B3 adapter's
//! call resolution (there is no flag any more). Pinned against the B3
//! harness (`b3_diff::detector_diff_for_workspace`), which is the reference:
//! analyze's findings equal the harness's "new" side and differ from its
//! "old" side (L3's own calls).
//!
//! The comparison is structural (detector, file, 1-based line and column,
//! severity, title): analyze assembles L3 under the gate model-instance id
//! and the harness under the default one, so routine ids and root-cause keys
//! differ in their id parts by construction.
//!
//! Each fixture is one the committed r0 triage table
//! (`docs/b3-triage/r0-corpus.md`) lists with a finding difference, and the
//! test asserts old != new first, so an analyze that ignored the adapter
//! would fail it.

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use al_sem::engine::l3::b3_diff::detector_diff_for_workspace;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;

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
fn analyze_matches_the_harness_new_side_on_r0_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    for name in ["ws-member-call-resolution", "ws-overload-callresult-guards"] {
        let ws = root.join(name);
        let d = detector_diff_for_workspace(&ws, false).expect("harness runs");
        let (old, new) = (harness_keys(&d.old), harness_keys(&d.new));
        assert_ne!(
            old, new,
            "{name}: precondition: the harness sees a difference"
        );
        let got = analyze_keys(&ws);
        assert_eq!(got, new, "{name}: analyze != harness new");
        assert_ne!(got, old, "{name}: analyze equals harness old");
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
