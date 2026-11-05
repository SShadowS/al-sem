//! The freshness check's rules (agent-tasks design §2.3-2.5): the class
//! table (F-4), the deadline (F-9) and the check against a descriptor.

use super::*;

fn class(cats: &[&str]) -> (String, bool) {
    let set: BTreeSet<&str> = cats.iter().copied().collect();
    let v = refresh_class(&set).expect("a class");
    (
        v["class"].as_str().unwrap().to_string(),
        v["certain"].as_bool().unwrap(),
    )
}

/// F-4: every row of §2.3's table, each with the session's own class.
/// `roots-config`, `coverage-config` and `app-json-paths` alone are
/// `BodyOnly` (the workspace session's tests pin that a change of each
/// alone refreshes as B: `package_change_rebuilds_and_roots_config_is_b`,
/// `coverage_config_and_nested_app_json_are_b`).
///
/// DISCRIMINATION (2026-11-05): mapping `app-json` to `BodyOnly` (dropping
/// it from the package list) fails at "app-json"; restored, it passes.
#[test]
fn categories_give_the_sessions_refresh_classes() {
    for cat in [
        "package",
        "package-list",
        "unreadable-package-folder",
        "app-json",
        "source-dependency",
        "meta",
    ] {
        assert_eq!(class(&[cat]), ("Package".into(), true), "{cat}");
        assert_eq!(
            class(&[cat, "al-file", "al-walk"]),
            ("Package".into(), true),
            "{cat} wins"
        );
    }
    assert_eq!(class(&["al-walk"]), ("Declaration".into(), true));
    assert_eq!(
        class(&["al-walk", "al-file", "roots-config"]),
        ("Declaration".into(), true)
    );
    assert_eq!(class(&["roots-config"]), ("BodyOnly".into(), true));
    assert_eq!(class(&["coverage-config"]), ("BodyOnly".into(), true));
    assert_eq!(class(&["app-json-paths"]), ("BodyOnly".into(), true));
    assert_eq!(class(&["al-file"]), ("BodyOnly|Declaration".into(), false));
    assert_eq!(
        class(&["al-file", "roots-config"]),
        ("BodyOnly|Declaration".into(), false)
    );
    assert!(refresh_class(&BTreeSet::new()).is_none());
}

/// A descriptor of `ws` as a revision would publish it: an observation's
/// manifest with its stats.
fn descriptor_of(ws: &std::path::Path) -> InputDescriptor {
    let m = Manifest::observe(ws, false, crate::engine::resident::manifest::Hashing::All);
    InputDescriptor {
        generation: Some(3),
        inputs_stamp: m.stamp(),
        workspace: ws.to_path_buf(),
        universe: "crossApp".into(),
        manifest: m.descriptor_entries(),
    }
}

/// The check against a published descriptor: fresh on an unchanged disk
/// (stat-cached: nothing read), stale naming the edited file, its category
/// and the uncertain class after an edit; `verify` reads every file.
#[test]
fn a_check_compares_the_disk_with_the_descriptor() {
    let dir =
        crate::engine::resident::manifest::tests::copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let d = descriptor_of(ws);
    let c = check(&d, CheckOptions::new(false));
    assert_eq!(c["complete"], true, "{c}");
    assert_eq!(c["fresh"], true, "{c}");
    assert_eq!(c["filesHashed"], 0, "stat-cached: {c}");
    assert_eq!(c["checkedRevision"]["generation"], 3, "{c}");
    let v = check(&d, CheckOptions::new(true));
    assert_eq!(v["fresh"], true, "{v}");
    assert!(v["filesHashed"].as_u64().unwrap() > 0, "verify reads: {v}");

    let p = ws.join("Caller.al");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("Compute(5)", "Compute(6)")).unwrap();
    let c = check(&d, CheckOptions::new(false));
    assert_eq!(c["complete"], true, "{c}");
    assert_eq!(c["fresh"], false, "{c}");
    assert_eq!(c["filesHashed"], 1, "{c}");
    assert_eq!(
        c["changes"],
        json!([{"category": "al-file", "path": p.to_string_lossy()}]),
        "{c}"
    );
    assert_eq!(c["refreshClass"]["class"], "BodyOnly|Declaration", "{c}");
    assert_eq!(c["refreshClass"]["certain"], false, "{c}");
    let line = headline(&EngineState {
        workspace: "/ws",
        universe: "cross-app",
        check: Ok(&c),
        activity: None,
        refresh_state: "idle",
        failed: None,
    });
    assert_eq!(
        line,
        "cross-app engine for /ws: STALE. 1 .al file changed on disk since generation 3. Refresh class: BodyOnly or Declaration (known after parsing them). Next: any command with freshness fresh (the default) refreshes first; or call alsem_refresh."
    );
}

/// F-9: a check past its deadline answers `complete: false` with the
/// reason, never `fresh`, and its headline says freshness is unknown.
///
/// DISCRIMINATION (2026-11-05): answering a timed-out check as `fresh:
/// true` (the `incomplete` closure's `"fresh": false` flipped) fails at
/// "never fresh"; restored, it passes.
#[test]
fn a_check_past_its_deadline_is_incomplete_never_fresh() {
    let dir =
        crate::engine::resident::manifest::tests::copy_fixture("tests/fixtures/lsp-diff-deps");
    let d = descriptor_of(dir.path());
    let c = check(
        &d,
        CheckOptions {
            verify: false,
            deadline: Duration::from_millis(50),
            delay: Duration::from_millis(1000),
        },
    );
    assert_eq!(c["complete"], false, "{c}");
    assert_eq!(c["fresh"], false, "never fresh: {c}");
    assert!(
        c["incompleteReasons"][0]
            .as_str()
            .is_some_and(|r| r.contains("did not finish within 50 ms")),
        "{c}"
    );
    let line = headline(&EngineState {
        workspace: "/ws",
        universe: "cross-app",
        check: Ok(&c),
        activity: None,
        refresh_state: "idle",
        failed: None,
    });
    assert_eq!(
        line,
        "cross-app engine: the check did not finish (the check did not finish within 50 ms); freshness unknown."
    );
}
