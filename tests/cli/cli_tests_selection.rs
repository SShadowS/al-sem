//! `alsem tests` (agent-tasks design §4, milestone C), through the SHIPPED
//! binary: test scope, per-root status, run-first ranking, the fallback and
//! `selectionComplete`.
//!
//! Fixtures: `tests/fixtures/test-selection-graph` (the workspace's own tests)
//! and `tests/fixtures/test-universe` (a production app with a sibling test
//! app), copied into a temporary folder holding a `.git` marker so
//! auto-discovery searches only the fixture's apps. Expected tests, hops and
//! statuses are hand-stated from the fixture text.
//!
//! Byte goldens: `tests/cli-output-goldens/tests/`. Refresh:
//! `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_tests_selection::` — then
//! INSPECT the diff.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::regen;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// The `test-universe` fixture in a temporary repository (a `.git` marker).
fn universe_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&repo_root().join("tests/fixtures/test-universe"), t.path());
    std::fs::create_dir(t.path().join(".git")).unwrap();
    // A sibling that depends on the production app and holds no source (the
    // fixture cannot carry it: the fixture oracles build every app folder).
    std::fs::create_dir(t.path().join("TestFR")).unwrap();
    std::fs::write(
        t.path().join("TestFR/app.json"),
        r#"{"id": "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a03", "name": "TU Production Tests FR",
            "publisher": "al-sem tests", "version": "1.0.0.0",
            "dependencies": [{"id": "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a01", "name": "TU Production",
                              "publisher": "al-sem tests", "version": "1.0.0.0"}]}"#,
    )
    .unwrap();
    t
}

/// `alsem tests <ws> <args>`: (stdout, exit).
fn tests(ws: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("tests")
        .arg(ws)
        .args(args)
        .output()
        .expect("spawn alsem");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.trim().is_empty() || out.status.code() != Some(0),
        "stderr: {stderr}"
    );
    (
        String::from_utf8(out.stdout).expect("utf8"),
        out.status.code().unwrap_or(-1),
    )
}

fn json(ws: &Path, args: &[&str]) -> Value {
    let mut a = args.to_vec();
    a.extend(["--format", "json"]);
    let (out, code) = tests(ws, &a);
    assert_eq!(code, 0, "{out}");
    serde_json::from_str(&out).expect("al-sem.tests JSON")
}

/// The golden check: `text` with the temporary root spelled `<root>`.
fn golden(name: &str, text: &str, root: Option<&Path>) {
    let mut text = match root {
        Some(r) => text.replace(&r.to_string_lossy().to_string(), "<root>"),
        None => text.to_string(),
    };
    // A universe key hashes the (temporary) paths: `tests:<key>`.
    let mut from = 0;
    while let Some(i) = text[from..].find("\"tests:").map(|i| from + i + 7) {
        let hex = text[i..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        if hex == 16 {
            text.replace_range(i..i + 16, "<key>");
        }
        from = i;
    }
    let path = repo_root()
        .join("tests/cli-output-goldens/tests")
        .join(name);
    if regen::regen_mode() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        regen::write_golden(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden {}: {e} (REGEN_TEMP_GOLDENS=1 mints it)",
            path.display()
        )
    });
    assert_eq!(text, want, "golden {name} moved");
}

/// C-1: the workspace's own tests. `ApplyDecay` is reached by
/// `NightlyRecalcDecays` on a local path: the test calls
/// `Acc Loyalty Rules::RecalcNightly`, which runs the codeunit `Acc Nightly
/// Recalc` (its object node), whose `OnRun` calls `ApplyDecay`: 4 hops.
///
/// DISCRIMINATION (2026-11-05): ranking by codeunit id only (dropping the
/// tier and hops from `tests.sort_by` in `render`) does not move this
/// one-test answer, so the order is pinned by `ranks_by_tier_then_hops`
/// below; walking forward edges instead of reversed ones finds no test.
#[test]
fn workspace_tests_select_the_local_test_with_its_hops() {
    let ws = repo_root().join("tests/fixtures/test-selection-graph");
    let v = json(&ws, &["--routine", "ApplyDecay"]);
    assert_eq!(v["schema"], "al-sem.tests");
    assert_eq!(v["changed"][0]["status"], "reached");
    let first = &v["runFirst"][0];
    assert_eq!(first["procedure"], "NightlyRecalcDecays");
    assert_eq!(first["tier"], "local");
    assert_eq!(first["hops"], 4);
    assert_eq!(first["class"], "direct");
    let (human, code) = tests(&ws, &["--routine", "ApplyDecay"]);
    assert_eq!(code, 0);
    golden("selection-graph.applydecay.human.txt", &human, None);
}

/// C-1 order: tier first, then hops, then codeunit id. `Post` is reached at
/// 1 hop and `Calc` at 1 (its own test) and 2 (through `Post`): asking for
/// both ranks the two 1-hop tests before nothing deeper.
#[test]
fn ranks_by_tier_then_hops() {
    let t = universe_repo();
    let v = json(
        &t.path().join("App"),
        &["--routine", "TU Core::Leaf", "--routine", "TU Core::Calc"],
    );
    let hops: Vec<u64> = v["runFirst"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["hops"].as_u64().unwrap())
        .collect();
    let mut sorted = hops.clone();
    sorted.sort_unstable();
    assert_eq!(hops, sorted, "{v:#}");
}

/// C-2 and C-3 through the binary: the sibling test app is discovered (its
/// `TestFR` sibling skipped `no-source`), the production app is loaded into
/// the test universe as local source, and the changed root's StableRoutineId
/// (resolved in the production universe) is found there: `Post` is reached
/// by `PostUpdatesQty` at 1 hop, and the test's `reaches` names that id.
#[test]
fn a_sibling_test_app_is_discovered_and_reaches_the_production_root() {
    let t = universe_repo();
    let ws = t.path().join("App");
    let v = json(&ws, &["--routine", "TU Core::Post"]);
    let scope = &v["scope"];
    assert_eq!(scope["discovery"], "auto");
    assert_eq!(scope["discoveryComplete"], true);
    assert_eq!(scope["apps"][0]["name"], "TU Production Tests");
    assert_eq!(scope["apps"][0]["testsLoaded"], 2);
    assert_eq!(scope["apps"][0]["productionTier"], "localApproximate");
    assert_eq!(scope["skipped"][0]["reason"], "no-source");
    let id = v["changed"][0]["stableId"].as_str().unwrap().to_string();
    assert_eq!(v["changed"][0]["status"], "reached");
    assert_eq!(v["runFirst"][0]["procedure"], "PostUpdatesQty");
    assert_eq!(v["runFirst"][0]["hops"], 1);
    assert_eq!(v["runFirst"][0]["reaches"][0], id);
    assert_eq!(v["selectionComplete"], true, "{v:#}");
    // The test app declares Library Assert (Microsoft), which is not loaded.
    assert!(v["next"].as_str().unwrap().contains("Library Assert"));
    let (human, _) = tests(&ws, &["--routine", "TU Core::Post"]);
    golden("universe.post.human.txt", &human, Some(t.path()));
    let (json_text, _) = tests(&ws, &["--routine", "TU Core::Post", "--format", "json"]);
    golden("universe.post.json", &json_text, Some(t.path()));
}

/// C-4: a production leaf a test reaches only through another production
/// routine (the kind the pruned export drops) is `reached`, never missing.
#[test]
fn a_leaf_reached_through_production_code_is_reached() {
    let t = universe_repo();
    let v = json(&t.path().join("App"), &["--routine", "TU Core::Leaf"]);
    assert_eq!(v["changed"][0]["status"], "reached");
    assert_eq!(v["runFirst"][0]["procedure"], "PostUpdatesQty");
    assert_eq!(v["runFirst"][0]["hops"], 2);
}

/// C-6: a routine run only by a subscriber to a platform table event
/// (`OnAfterModifyEvent`) has a selection-relevant hidden channel in its
/// cone: no test reaches it, its status is `analysis-incomplete` (never
/// `no-known-path`), the fallback says why, and `selectionComplete` is false.
/// A routine with no caller at all and no hidden channel is `no-known-path`
/// with a complete selection.
///
/// DISCRIMINATION (2026-11-05): treating a platform table event as covered
/// (`hidden_class` returning `false` for it) makes `Audited`'s status
/// `no-known-path` and `selectionComplete` true: this test fails on both.
#[test]
fn a_platform_table_event_in_the_cone_keeps_the_selection_incomplete() {
    let t = universe_repo();
    let ws = t.path().join("App");
    let v = json(&ws, &["--routine", "TU Core::Audited"]);
    assert_eq!(v["changed"][0]["status"], "analysis-incomplete");
    assert_eq!(v["selectionComplete"], false);
    let reasons: Vec<&str> = v["requiredFallback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"hidden-callers"), "{reasons:?}");
    assert_eq!(v["hidden"][0]["class"], "platform-table-event");
    let (human, _) = tests(&ws, &["--routine", "TU Core::Audited"]);
    assert!(human.find("selection complete: no").unwrap() < human.find("this is static").unwrap());
    golden("universe.audited.human.txt", &human, Some(t.path()));
    let v = json(&ws, &["--routine", "TU Core::Untested"]);
    assert_eq!(v["changed"][0]["status"], "no-known-path");
    assert_eq!(v["selectionComplete"], true, "{v:#}");
}

/// C-5: no scope at all (no `[Test]` in the workspace, `testApps: []`): the
/// "none — no test was considered" text, exit 0, and never "none reach it".
///
/// DISCRIMINATION (2026-11-05): dropping the `!any_scope` line in `render`
/// fails the `test scope: none` assertion.
#[test]
fn no_test_scope_says_no_test_was_considered() {
    let t = universe_repo();
    let ws = t.path().join("App");
    std::fs::write(ws.join(".al-sem.json"), r#"{"testApps": []}"#).unwrap();
    let (human, code) = tests(&ws, &["--routine", "TU Core::Post"]);
    assert_eq!(code, 0);
    assert!(human.contains("test scope: none — "), "{human}");
    assert!(human.contains("This does not mean no tests are needed."));
    assert!(!human.contains("none reach"));
    assert!(human.contains("selection complete: no"));
    golden("universe.noscope.human.txt", &human, Some(t.path()));
}

/// `--test-app` names the app (no discovery); a routine no selector finds
/// exits 2 and selects nothing.
#[test]
fn named_test_apps_and_unresolved_routines() {
    let t = universe_repo();
    let ws = t.path().join("App");
    let test = t.path().join("Test");
    let v = json(
        &ws,
        &[
            "--routine",
            "TU Core::Calc",
            "--test-app",
            test.to_str().unwrap(),
        ],
    );
    assert_eq!(v["scope"]["discovery"], "call");
    assert_eq!(v["runFirst"][0]["procedure"], "CalcAddsOne");
    let (human, code) = tests(&ws, &["--routine", "TU Core::Nope"]);
    assert_eq!(code, 2, "{human}");
    assert!(human.contains("changed: no routine resolved"));
}
