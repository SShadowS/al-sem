//! Issue #87 (T3): every `ruleId` that `alsem analyze --format sarif` emits is
//! described in the same document's `tool.driver.rules`. Runs the shipped
//! binary on the issue's reproducer (`ws-d34`, single-app), whose findings
//! include `d19-unused-parameter` and `d34-commit-in-loop`, two detectors the
//! old hand-written catalog left out.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

#[test]
fn every_sarif_result_rule_id_is_in_the_rules_catalog() {
    let ws = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join("ws-d34");
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["analyze"])
        .arg(&ws)
        .args(["--format", "sarif", "--single-app"])
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not SARIF JSON ({e}); exit {:?}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let run = &doc["runs"][0];
    let rule_ids: BTreeSet<&str> = run["tool"]["driver"]["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .map(|r| r["id"].as_str().expect("rule id"))
        .collect();
    let result_ids: BTreeSet<&str> = run["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["ruleId"].as_str().expect("ruleId"))
        .collect();
    // The reproducer must actually exercise the two detectors the issue names,
    // or the subset check below proves nothing about them.
    for named in ["d19-unused-parameter", "d34-commit-in-loop"] {
        assert!(
            result_ids.contains(named),
            "fixture no longer yields a {named} result; results: {result_ids:?}"
        );
    }
    let missing: Vec<&str> = result_ids.difference(&rule_ids).copied().collect();
    assert!(
        missing.is_empty(),
        "ruleIds with no tool.driver.rules entry: {missing:?}"
    );
}
