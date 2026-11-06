//! The answer from hand-stated parts (agent-tasks C-10, C-11, C-12): each
//! precondition is constructed literally, never asked of the engine.

use serde_json::{Value, json};

use super::*;
use crate::engine::test_select::render::{RenderOpts, TierPick};

const OPTS: RenderOpts = RenderOpts {
    tier: TierPick::Local,
    max_tests: None,
    json: true,
    surface: crate::engine::follow_up::Surface::Cli,
    single_app: false,
};

fn roots(stamp: &str) -> Value {
    json!({
        "schema": "al-sem.tests-roots", "version": 1, "workspace": "/ws/App", "appId": "p",
        "productionSourceStamp": stamp, "workspaceTests": 0,
        "roots": [{"stableId": "p:Codeunit:1#aa", "display": "Codeunit A::R"}],
        "diagnostics": [],
        "scope": {"origin": "auto", "complete": true, "incomplete": [], "searched": 1,
                  "apps": [{"name": "T", "root": "/ws/Test", "appId": "t", "found": "auto",
                            "files": 1}],
                  "skipped": [], "summary": "1 test app discovered (T)"},
    })
}

fn select(stamp: &str, root: Value, tests: Vec<Value>) -> Value {
    json!({
        "schema": "al-sem.tests-select", "version": 1, "universe": "tests:x", "testAppId": "t",
        "productionSourceStamp": stamp, "productionTier": "localApproximate", "testsLoaded": 2,
        "roots": [root], "tests": tests, "universalHub": false, "hidden": [],
        "unresolvedTestCalls": 0, "missingDependencies": [], "limits": [],
    })
}

fn reached() -> Value {
    json!({"stableId": "p:Codeunit:1#aa", "status": "reached"})
}

fn test(tier: &str) -> Value {
    json!({"stableId": "t:Codeunit:9#bb", "display": "Codeunit T::X", "codeunitId": 9,
           "codeunit": "T", "procedure": "X", "file": "x.al", "tier": tier, "class": "direct",
           "hops": 2, "reaches": "p:Codeunit:1#aa", "path": ["Codeunit T::X", "Codeunit A::R"]})
}

fn run_with(answers: Vec<Value>) -> (Value, usize) {
    let req = TestsRequest {
        workspace: "/ws/App".into(),
        routines: vec!["R".into()],
        ..TestsRequest::default()
    };
    let mut answers = answers.into_iter();
    let mut calls = 0;
    let mut run = |_: Target<'_>, _: Vec<String>| -> Result<Value, String> {
        calls += 1;
        Ok(answers.next().expect("one answer per part"))
    };
    let r = orchestrate(&req, OPTS, &mut run).unwrap();
    (serde_json::from_str(&r.text).unwrap(), calls)
}

/// C-10: a test universe built from other production bytes is asked once
/// more with the production part; when the stamps still differ, that test
/// app is not analyzed, with the stamp reason; when they then agree, the
/// answer uses the second pair. Never an answer with mismatched stamps.
///
/// DISCRIMINATION (2026-11-05): skipping the stamp comparison (the `Ok(v) if
/// v[...] != ...` arm never taken) answers the first pair: `calls` is 2 and
/// the refusal is missing; both assertions fail.
#[test]
fn mismatched_production_stamps_are_retried_once_then_refused() {
    let (v, calls) = run_with(vec![
        roots("A"),
        select("B", reached(), vec![test("local")]),
        roots("A"),
        select("B", reached(), vec![test("local")]),
    ]);
    assert_eq!(calls, 4);
    assert_eq!(v["selectionComplete"], false);
    let f = &v["requiredFallback"][0];
    assert_eq!(f["reason"], "test-app-not-analyzed");
    assert!(
        f["detail"]
            .as_str()
            .unwrap()
            .contains("different production bytes")
    );
    assert!(v["runFirst"].as_array().unwrap().is_empty(), "{v:#}");
    let (v, calls) = run_with(vec![
        roots("A"),
        select("B", reached(), vec![test("local")]),
        roots("C"),
        select("C", reached(), vec![test("local")]),
    ]);
    assert_eq!(calls, 4);
    assert_eq!(v["selectionComplete"], true, "{v:#}");
    assert_eq!(v["productionSourceStamp"], "C");
}

/// C-11: a root the test universe does not load is `not-loaded` (never
/// `no-known-path`) and sets the fallback.
///
/// DISCRIMINATION (2026-11-05): dropping `not-loaded` from the fallback
/// statuses in `render` leaves `selectionComplete` true: fails.
#[test]
fn a_root_not_loaded_requires_the_fallback() {
    let (v, _) = run_with(vec![
        roots("A"),
        select(
            "A",
            json!({"stableId": "p:Codeunit:1#aa", "status": "not-loaded"}),
            vec![],
        ),
    ]);
    assert_eq!(v["changed"][0]["status"], "not-loaded");
    assert_eq!(v["selectionComplete"], false);
    assert_eq!(v["requiredFallback"][0]["reason"], "root-not-loaded");
}

/// C-12: through-dependency tests reached only via dependency nodes whose
/// cones cover most of the graph get the sameness line.
///
/// DISCRIMINATION (2026-11-05): ignoring `sameness` (the `same` flag forced
/// false) prints "(--tier dependency lists them)" instead: fails.
#[test]
fn through_dependency_sameness_is_said() {
    let mut root = reached();
    root["throughDependency"] =
        json!({"tests": 1, "sameness": true, "via": [], "viaMeasured": true});
    let parts = vec![
        roots("A"),
        select("A", root, vec![test("local"), test("throughDependency")]),
    ];
    let req = TestsRequest {
        workspace: "/ws/App".into(),
        routines: vec!["R".into()],
        ..TestsRequest::default()
    };
    let mut parts = parts.into_iter();
    let mut run =
        |_: Target<'_>, _: Vec<String>| -> Result<Value, String> { Ok(parts.next().unwrap()) };
    let human = orchestrate(
        &req,
        RenderOpts {
            json: false,
            ..OPTS
        },
        &mut run,
    )
    .unwrap()
    .text;
    assert!(
        human.contains(
            "[via-dependency] 1 test — effectively the suite's posting tests, not specific to this change"
        ),
        "{human}"
    );
}
