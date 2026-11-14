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

/// A project-collection marker (`.al-sem-projects`) bounds the search to the
/// project that holds the workspace, nearer than the git top level: with a
/// corpus of more than [`MAX_APP_JSON`] other apps beside it (which caps the
/// search from the git top level), the fixture's own test app is found, the
/// sibling fixture's dependent test app is not, and the search is complete.
/// A workspace that is the project itself searches nothing and says
/// "under the project folder".
///
/// DISCRIMINATION (2026-11-05): ignoring the marker in `search_root` (only
/// `.git` counts) fails at `complete` ("the cap of 64 app.json files was
/// reached before the search ended", searched from the git top level);
/// restored, passes.
#[test]
fn a_projects_marker_bounds_the_search_to_the_project() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    std::fs::create_dir(r.join(".git")).unwrap();
    let corpus = r.join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(corpus.join(PROJECTS_MARKER), "").unwrap();
    // Fixture A: a production app and its test app, side by side.
    app(&corpus.join("a/App"), PROD, "Prod", &[]);
    app(
        &corpus.join("a/Test"),
        "22222222-2222-2222-2222-222222222222",
        "Fixture A Tests",
        &[PROD],
    );
    al(&corpus.join("a/Test/src"), "T.Codeunit.al", TEST_CU);
    // Fixture B: another test app of the same production id.
    app(
        &corpus.join("b/Test"),
        "33333333-3333-3333-3333-333333333333",
        "Fixture B Tests",
        &[PROD],
    );
    al(&corpus.join("b/Test/src"), "T.Codeunit.al", TEST_CU);
    // Enough other fixtures to cap a search from the git top level.
    for i in 0..(MAX_APP_JSON + 3) {
        app(
            &corpus.join(format!("n{i:03}")),
            &format!("{i:08}-0000-0000-0000-000000000000"),
            "n",
            &[],
        );
    }
    let s = discover(&corpus.join("a/App"), &ScopeRequest::default());
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Fixture A Tests"]);
    assert_eq!(s.searched, 1, "only fixture a's Test");
    let root = s.search_root.clone().expect("a search root");
    assert_eq!(root, (corpus.join("a"), false));

    // The workspace is the project itself: nothing else to search.
    app(&corpus.join("solo"), PROD, "Solo", &[]);
    let s = discover(&corpus.join("solo"), &ScopeRequest::default());
    assert!(s.complete, "{:?}", s.incomplete);
    assert_eq!(s.searched, 0);
    assert_eq!(
        s.sibling_summary(),
        "no app under the project folder (its parent holds .al-sem-projects) depends on this one \
         with a test codeunit (searched 0 app.json files)"
    );
}

/// Engine limits wave 0 (B8): a workspace that sits in a repository beside a
/// project collection (a folder holding `.al-sem-projects`) and a nested
/// repository (a folder holding its own `.git`), each with more than
/// [`MAX_APP_JSON`] unrelated `app.json` files, is searched from the git top
/// level without entering either: the search completes, finds the sibling
/// test app, and finds a test app that is a repository of its own (its
/// `app.json` is read before the nested-repository check). The precondition
/// is stated: the workspace is under neither folder, so `search_root` is the
/// git top level and only the walk can keep out of them.
///
/// DISCRIMINATION (2026-11-09): `other_project` without its `.git` arm
/// fails at `complete` ("the cap of 64 app.json files was reached ...",
/// searched 64 from `vendor/clone`); without its marker arm fails the same
/// way from `corpus`; restored, passes.
#[test]
fn the_walk_does_not_enter_other_projects() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    std::fs::create_dir(r.join(".git")).unwrap();
    app(&r.join("work/App"), PROD, "Prod", &[]);
    app(
        &r.join("work/Test"),
        "22222222-2222-2222-2222-222222222222",
        "Prod Tests",
        &[PROD],
    );
    al(&r.join("work/Test/src"), "T.Codeunit.al", TEST_CU);
    // A test app that is a repository of its own (a submodule, a clone).
    app(
        &r.join("TestRepo"),
        "33333333-3333-3333-3333-333333333333",
        "Prod Tests Repo",
        &[PROD],
    );
    std::fs::write(r.join("TestRepo/.git"), "gitdir: elsewhere\n").unwrap();
    al(&r.join("TestRepo/src"), "T.Codeunit.al", TEST_CU);
    // A project collection and a nested repository, each over the cap, each
    // holding a dependent test app that must not be found.
    let corpus = r.join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(corpus.join(PROJECTS_MARKER), "").unwrap();
    let clone = r.join("vendor/clone");
    std::fs::create_dir_all(clone.join(".git")).unwrap();
    for base in [&corpus, &clone] {
        app(
            &base.join("dep"),
            "44444444-4444-4444-4444-444444444444",
            "Unrelated Tests",
            &[PROD],
        );
        al(&base.join("dep/src"), "T.Codeunit.al", TEST_CU);
        for i in 0..(MAX_APP_JSON + 3) {
            app(
                &base.join(format!("n{i:03}")),
                &format!("{i:08}-0000-0000-0000-000000000000"),
                "n",
                &[],
            );
        }
    }
    let s = discover(&r.join("work/App"), &ScopeRequest::default());
    assert_eq!(
        s.search_root.as_ref(),
        Some(&(r.to_path_buf(), true)),
        "precondition: searched from the git top level"
    );
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Prod Tests Repo", "Prod Tests"]);
    assert_eq!(s.searched, 2, "TestRepo and work/Test only");
}

/// A REAL repository (`git init`): `check-ignore` needs one.
fn git_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let ok = crate::git_env::git_command()
        .arg("-C")
        .arg(t.path())
        .args(["init", "-q"])
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "git init failed");
    t
}

/// A dependent test app at `dir`, named `name`.
fn test_app(dir: &Path, id: &str, name: &str) {
    app(dir, id, name, &[PROD]);
    al(&dir.join("src"), "T.Codeunit.al", TEST_CU);
}

/// Engine limits B8 (TLS-48): auto-discovery does not enter a folder the
/// repository's `.gitignore` ignores (a directory pattern, a bare name), and
/// a folder that holds the workspace is entered even when ignored, with its
/// workspace's siblings found.
///
/// DISCRIMINATION (2026-11-13): `walk` without the `retain` on `ignored`
/// fails at the first `names` ("Copy Tests" and "Out Tests" found); passing
/// `honor_ignore = true` to every child fails at the second (no "Scratch
/// Tests"); restored, passes.
#[test]
fn discovery_skips_folders_git_ignores() {
    let t = git_repo();
    let r = t.path();
    std::fs::write(r.join(".gitignore"), "build/\nOut\nscratch/\n").unwrap();
    app(&r.join("App"), PROD, "Prod", &[]);
    test_app(
        &r.join("Test"),
        "22222222-2222-2222-2222-222222222222",
        "Prod Tests",
    );
    test_app(
        &r.join("build/Copy"),
        "33333333-3333-3333-3333-333333333333",
        "Copy Tests",
    );
    test_app(
        &r.join("Out"),
        "44444444-4444-4444-4444-444444444444",
        "Out Tests",
    );
    let s = discover(&r.join("App"), &ScopeRequest::default());
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Prod Tests"]);
    assert_eq!(s.searched, 1, "Test only");

    // A workspace in an ignored folder still finds its sibling there.
    app(&r.join("scratch/App"), PROD, "Prod", &[]);
    test_app(
        &r.join("scratch/Tests"),
        "55555555-5555-5555-5555-555555555555",
        "Scratch Tests",
    );
    let s = discover(&r.join("scratch/App"), &ScopeRequest::default());
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Prod Tests", "Scratch Tests"]);
}

/// TLS-48 review r1: a search root inside an ignored folder (here a marked
/// project folder under an ignored `mark/`) still finds its siblings (git
/// reports all of the root ignored), and an ignored sibling that is a
/// repository of its own (a nested clone) is still read as an app.
///
/// DISCRIMINATION (2026-11-13): starting the walk with `honor_ignore = true`
/// fails at the first `names` (empty); dropping the `.git` arm of the
/// `retain` fails at the second (no "Clone Tests"); restored, passes.
#[test]
fn an_ignored_root_and_an_ignored_clone_are_still_searched() {
    let t = git_repo();
    let r = t.path();
    std::fs::write(r.join(".gitignore"), "mark/\nclone/\n").unwrap();
    std::fs::create_dir_all(r.join("mark")).unwrap();
    std::fs::write(r.join("mark").join(PROJECTS_MARKER), "").unwrap();
    app(&r.join("mark/p1/App"), PROD, "Prod", &[]);
    test_app(
        &r.join("mark/p1/T"),
        "22222222-2222-2222-2222-222222222222",
        "Marked Tests",
    );
    let s = discover(&r.join("mark/p1/App"), &ScopeRequest::default());
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Marked Tests"]);

    app(&r.join("App"), PROD, "Prod", &[]);
    test_app(
        &r.join("clone"),
        "33333333-3333-3333-3333-333333333333",
        "Clone Tests",
    );
    let ok = crate::git_env::git_command()
        .arg("-C")
        .arg(r.join("clone"))
        .args(["init", "-q"])
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "git init clone failed");
    let s = discover(&r.join("App"), &ScopeRequest::default());
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Clone Tests"]);
}

/// TLS-48 review sol r1: git's answer is matched by raw name bytes. An
/// ignored folder whose name is not UTF-8 (`tests-\xff`) must not be confused
/// with a sibling whose name holds U+FFFD (`tests-\u{FFFD}`, what a lossy
/// decode of the first gives), so the real test app there is found.
///
/// DISCRIMINATION (2026-11-13): decoding git's output with
/// `String::from_utf8_lossy` (the old code) finds "Ignored Tests" instead of
/// "Real Tests"; restored, passes. Linux only: macOS (APFS) refuses a
/// non-UTF-8 folder name.
#[cfg(target_os = "linux")]
#[test]
fn an_ignored_non_utf8_folder_is_not_its_lossy_sibling() {
    use std::os::unix::ffi::OsStrExt;
    let t = git_repo();
    let r = t.path();
    let ignored = r.join(std::ffi::OsStr::from_bytes(b"tests-\xff"));
    std::fs::write(r.join(".gitignore"), b"tests-\xff/\n").unwrap();
    app(&r.join("App"), PROD, "Prod", &[]);
    test_app(
        &ignored,
        "77777777-7777-7777-7777-777777777777",
        "Ignored Tests",
    );
    test_app(
        &r.join("tests-\u{FFFD}"),
        "88888888-8888-8888-8888-888888888888",
        "Real Tests",
    );
    let s = discover(&r.join("App"), &ScopeRequest::default());
    assert!(s.complete, "{:?}", s.incomplete);
    let names: Vec<&str> = s.apps.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Real Tests"]);
}
