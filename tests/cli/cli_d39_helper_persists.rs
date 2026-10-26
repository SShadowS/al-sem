//! #343: d39's `callerPersists` also counts a later `var` helper, or a call on
//! the record to a table procedure, that persists the record on every path to
//! an exit (`helperPersists`). Path-sensitive: an `exit` reached with the
//! callee's dirt unsaved keeps the finding, unless that path runs only when the
//! callee returned the value on which it leaves the record clean (the CDO
//! `if not InitRecurringDailyJob(..) then exit;` shape).
//!
//! Drives the REAL analyze pipeline (`run_analyze_with_exit`) over the committed
//! fixture `tests/fixtures/issue-343/ws-helper`; every precondition is written in
//! the fixture source, one routine per shape, and the test asserts the EXACT
//! d39 rows.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issue-343/ws-helper")
}

/// `"<routine>\tL<line>"` for each d39 finding, sorted.
fn d39_rows() -> Vec<String> {
    let args = AnalyzeArgs {
        workspace: fixture().to_string_lossy().to_string(),
        min_severity: None,
        detector: Some("d39-record-left-dirty-across-chain".to_string()),
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
        single_app: false,
        with_evidence: false,
    };
    let out =
        match run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "343-test") {
            Ok((out, ..)) => out,
            Err(e) => panic!("run_analyze failed: {e}"),
        };
    let json: serde_json::Value = serde_json::from_str(&out).expect("analyze JSON must parse");
    let mut rows: Vec<String> = json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"] == "d39-record-left-dirty-across-chain")
        .map(|f| {
            let loc = &f["primaryLocation"];
            format!(
                "{}\tL{}",
                loc["routineName"].as_str().expect("routineName"),
                loc["line"].as_u64().expect("line")
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Only A1 (CDO shape), A4 (plain helper), A6 (receiver call on a table
/// procedure), A9 (helper persisting through its own helper), A20 (three
/// helper levels), A26 (a helper between that provably leaves the record be)
/// and A27 (a guard whose callee's helper provably makes no dirt) are skipped.
/// Every other shape reports; each is named in the fixture. On base
/// `506149da` all 27 report.
#[test]
fn d39_counts_a_helper_that_persists_on_every_path() {
    let mut expected = vec![
        "ExitBeforeHelper\tL90",
        "ExitWithCall\tL101",
        "HelperOnOnePath\tL117",
        "GuardOnDirtyValue\tL133",
        "GuardDirtyOnFalse\tL142",
        "PersistInOtherArm\tL160",
        // Revision 2: fail-closed shapes.
        "HelperReinits\tL179",
        "CallerReloads\tL187",
        "LoopGuard\tL208",
        "ForHeader\tL226",
        "AssertErrorShape\tL233",
        "TernaryShape\tL242",
        "IsHandledHelper\tL265",
        "ReceiverInClean\tL283",
        "PublisherInClean\tL309",
        "FourLevels\tL346",
        // Revision 3: no L4-role trust for an intervening call; publishers fail closed.
        "ReassignInLoop\tL358",
        "IndirectReplace\tL372",
        "WrappedReceiverInClean\tL396",
        "PublisherBeforePersist\tL413",
    ];
    expected.sort();
    assert_eq!(d39_rows(), expected);
}
