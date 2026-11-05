//! Scope discovery (agent-tasks C-3).

use super::*;

/// An `app.json` with `id`, `name` and the dependency ids given.
fn app(dir: &Path, id: &str, name: &str, deps: &[&str]) {
    std::fs::create_dir_all(dir).unwrap();
    let deps: Vec<serde_json::Value> = deps
        .iter()
        .map(|d| serde_json::json!({"id": d, "name": "x", "publisher": "p", "version": "1.0.0.0"}))
        .collect();
    std::fs::write(
        dir.join("app.json"),
        serde_json::json!({"id": id, "name": name, "publisher": "p", "version": "1.0.0.0",
                           "dependencies": deps})
        .to_string(),
    )
    .unwrap();
}

fn al(dir: &Path, file: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

const PROD: &str = "11111111-1111-1111-1111-111111111111";
const TEST_CU: &str = "codeunit 50100 \"My Tests\"\n{\n    Subtype = Test;\n\n    [Test]\n    procedure T()\n    begin\n    end;\n}\n";

/// A repository: the production app, a test app, a dependent with no source,
/// a dependent with no test codeunit, an unrelated app with a test codeunit.
fn repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    std::fs::create_dir(r.join(".git")).unwrap();
    app(&r.join("App"), PROD, "Prod", &[]);
    al(
        &r.join("App/src"),
        "A.Codeunit.al",
        "codeunit 50000 A { }\n",
    );
    app(
        &r.join("Test"),
        "22222222-2222-2222-2222-222222222222",
        "Prod Tests",
        &[PROD],
    );
    al(&r.join("Test/src"), "T.Codeunit.al", TEST_CU);
    app(
        &r.join("TestFR"),
        "33333333-3333-3333-3333-333333333333",
        "Prod Tests FR",
        &[PROD],
    );
    app(
        &r.join("FR"),
        "44444444-4444-4444-4444-444444444444",
        "Prod FR",
        &[PROD],
    );
    al(
        &r.join("FR/src"),
        "R.Codeunit.al",
        "codeunit 50200 R\n{\n    Subtype = TestRunner;\n}\n",
    );
    app(
        &r.join("Other"),
        "55555555-5555-5555-5555-555555555555",
        "Other",
        &[],
    );
    al(&r.join("Other/src"), "T.Codeunit.al", TEST_CU);
    t
}

/// C-3: the sibling with a test codeunit is found; one with no `.al` is
/// `no-source`; a dependent whose only codeunit is a TestRunner is
/// `no-test-codeunit`; an app without the dependency is not listed.
///
/// DISCRIMINATION (2026-11-05): dropping the dependency check (`depends_on`
/// returning `true`) lists `Other` as a test app; reading `Subtype =
/// TestRunner` as a test codeunit lists `FR`.
#[test]
fn discovery_finds_dependents_with_test_codeunits_and_lists_the_skips() {
    let t = repo();
    let s = discover(&t.path().join("App"), &ScopeRequest::default());
    assert_eq!(s.origin, Origin::Auto);
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Prod Tests"]);
    assert_eq!(s.apps[0].test_codeunits, 1);
    assert_eq!(s.apps[0].test_attribute_lines, 1);
    let skipped: Vec<(&str, &str)> = s
        .skipped
        .iter()
        .map(|k| (k.name.as_str(), k.reason.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [
            ("Prod FR", "no-test-codeunit"),
            ("Prod Tests FR", "no-source")
        ]
    );
    assert_eq!(
        s.searched, 4,
        "Test, TestFR, FR, Other (the workspace's own excluded)"
    );
    assert_eq!(s.search_root.as_ref().map(|r| r.1), Some(true));
}

/// C-3: `testApps: []` in `.al-sem.json` turns discovery off; a configured
/// list is used as given; the call's list wins over the configuration.
#[test]
fn configured_and_called_test_apps_replace_discovery() {
    let t = repo();
    let ws = t.path().join("App");
    std::fs::write(ws.join(".al-sem.json"), r#"{"testApps": []}"#).unwrap();
    let s = discover(&ws, &ScopeRequest::default());
    assert_eq!(s.origin, Origin::Off);
    assert!(s.apps.is_empty());
    assert!(s.sibling_summary().contains("turned off"));
    std::fs::write(
        ws.join(".al-sem.json"),
        r#"{"testApps": ["../Test", "../Other"]}"#,
    )
    .unwrap();
    let s = discover(&ws, &ScopeRequest::default());
    assert_eq!(s.origin, Origin::Configured);
    assert_eq!(s.apps.len(), 1);
    assert_eq!(s.skipped[0].reason, SkipReason::NotADependent);
    let s = discover(
        &ws,
        &ScopeRequest {
            test_apps: Some(vec![t.path().join("TestFR")]),
        },
    );
    assert_eq!(s.origin, Origin::Call);
    assert_eq!(s.skipped[0].reason, SkipReason::NoSource);
}

/// C-3: a cap that fires is "discovery incomplete", never "no app depends on
/// this one".
///
/// DISCRIMINATION (2026-11-05): leaving `complete` true when the cap fires
/// (an empty `cap`) fails the `complete` assertion and the summary prints
/// "no app under the git top level depends on this one".
#[test]
fn the_app_json_cap_makes_discovery_incomplete() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    std::fs::create_dir(r.join(".git")).unwrap();
    app(&r.join("App"), PROD, "Prod", &[]);
    for i in 0..(MAX_APP_JSON + 3) {
        app(
            &r.join(format!("n{i:03}")),
            &format!("{i:08}-0000-0000-0000-000000000000"),
            "n",
            &[],
        );
    }
    let s = discover(&r.join("App"), &ScopeRequest::default());
    assert!(!s.complete);
    assert_eq!(s.searched, MAX_APP_JSON);
    assert!(
        s.sibling_summary().contains("incomplete"),
        "{}",
        s.sibling_summary()
    );
    assert!(!s.sibling_summary().contains("depends on this one"));
}
