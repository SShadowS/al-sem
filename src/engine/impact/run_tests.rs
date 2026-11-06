//! The impact answer from hand-stated parts (agent-tasks B-1, B-10 and the
//! nearest tested caller): every precondition is constructed literally,
//! never asked of the engine.

use serde_json::{Value, json};

use super::*;
use crate::engine::impact::render::RenderOpts;

const ROOT: &str = "p:Codeunit:1#aa";
const CALLER: &str = "p:Codeunit:2#bb";

fn main_doc() -> Value {
    json!({
        "schema": "al-sem.impact-main", "version": 1, "workspace": "/ws/App", "appId": "p",
        "productionSourceStamp": "S", "workspaceTests": 0,
        "roots": [{"stableId": ROOT, "display": "Codeunit A::R",
                   "anchor": {"file": "ws:src/a.al", "line": 3}, "status": "changed",
                   "effects": {"mayWriteTables": {"count": 2, "ownedByApp": 1, "byDependencies": 1},
                               "commits": {"answer": "may", "ownSites": 1}, "kinds": {"errors": 3},
                               "events": {"distinct": 0}}}],
        "diagnostics": ["hunk outside routines: src/t.al:4-6"],
        "diagnosticsDetail": [{"kind": "hunk-outside-routines", "file": "src/t.al",
                               "startLine": 4, "endLine": 6, "text": "hunk outside routines: src/t.al:4-6"},
                              {"kind": "diff-file-unmatched", "file": "other/x.al",
                               "text": "diff file unmatched: other/x.al"}],
        "accounting": {"changedLines": 5, "totals": {"total": 5, "routineBody": 2, "routineSignature": 0,
            "objectDeclaration": {"total": 2, "byKind": {"field": 2}}, "outsideObject": 0,
            "removedRoutineText": 0, "routineNotInModel": 0,
            "unmapped": {"total": 1, "byReason": {"not-in-workspace": 1}}},
            "files": [], "onlyIn": [], "errors": []},
        "scope": {"origin": "auto", "complete": true, "incomplete": [], "searched": 1,
                  "apps": [{"name": "T", "root": "/ws/Test", "appId": "t", "found": "auto", "files": 1}],
                  "skipped": [], "summary": "1 test app discovered (T)"},
    })
}

fn up_doc() -> Value {
    json!({
        "schema": "al-sem.impact-up", "version": 1, "graph": {"nodes": 9, "sites": 9},
        "roots": [{"stableId": ROOT, "status": "walked", "display": "Codeunit A::R",
                   "ancestors": {"total": 1, "local": 1, "throughDependency": 0, "onlyOpen": 0},
                   "entries": [], "userOrJobEntries": 0, "external": [],
                   "eventPublishers": {"count": 0, "first": []},
                   "callers": {"count": 1, "local": 1, "first": [{"display": "Codeunit B::C", "file": "ws:src/b.al", "line": 7}]},
                   "nearestAncestors": [{"stableId": CALLER, "display": "Codeunit B::C", "hops": 1}],
                   "hidden": {"universalHub": false, "byChannel": {}}, "limited": null}],
        "union": {"ancestors": {"total": 1}, "entries": []},
        "cost": {"perRootStates": 2, "unionStates": 2},
    })
}

fn select(root_status: &str, probes: Vec<Value>) -> Value {
    json!({
        "schema": "al-sem.tests-select", "version": 1, "universe": "tests:x", "testAppId": "t",
        "productionSourceStamp": "S", "productionTier": "localApproximate", "testsLoaded": 2,
        "roots": [{"stableId": ROOT, "status": root_status}], "tests": [], "universalHub": false,
        "hidden": [], "unresolvedTestCalls": 0, "missingDependencies": [], "limits": [],
        "probes": probes,
    })
}

fn req() -> ImpactRequest {
    ImpactRequest {
        workspace: "/ws/App".into(),
        routines: vec!["R".into()],
        tests: true,
        ..ImpactRequest::default()
    }
}

/// Runs the parts in order: main, up, then the test answers given.
fn answer(tests: Vec<Result<Value, String>>, json_out: bool) -> (String, usize) {
    let mut parts = vec![Ok(main_doc()), Ok(up_doc())];
    parts.extend(tests);
    let mut parts = parts.into_iter();
    let mut calls = 0;
    let mut run = |_: Target<'_>, _: Vec<String>| -> Result<Value, String> {
        calls += 1;
        parts.next().expect("one answer per part")
    };
    let r = orchestrate(
        &req(),
        super::super::compare::Comparison {
            source: "routines",
            ..Default::default()
        },
        RenderOpts {
            json: json_out,
            ..RenderOpts::default()
        },
        &mut run,
        None,
    )
    .unwrap();
    (r.text, calls)
}

/// B-1: the accounting comes first (the line after the comparison), before
/// any routine; every diagnostic of the main part is in the JSON, and the
/// human text counts the ones it does not spell.
///
/// DISCRIMINATION (2026-11-05): moving the `input:` push after the per-root
/// block in `render` fails at "accounting first"; restored, passes.
#[test]
fn accounting_comes_first_and_every_diagnostic_appears() {
    let (human, _) = answer(vec![Ok(select("reached", vec![]))], false);
    let lines: Vec<&str> = human.lines().collect();
    // Output validation §5.2: the four header lines lead, then the
    // comparison line, then the accounting.
    assert!(lines[0].starts_with("impact "), "the header: {human}");
    assert!(
        lines[5].starts_with("input: 1 changed routine resolved"),
        "accounting first: {human}"
    );
    // The first routine block (the header's answer names it too).
    let first_root = lines
        .iter()
        .skip(4)
        .position(|l| l.contains("Codeunit A::R"))
        .unwrap()
        + 4;
    let input = lines.iter().position(|l| l.starts_with("input:")).unwrap();
    assert!(input < first_root, "accounting first");
    assert!(
        human.contains("hunks outside routines") || human.contains("root diagnostics: 1"),
        "{human}"
    );
    assert!(human.contains("diff file unmatched: other/x.al"), "{human}");
    // §7.6: "no base given" prints once, in the header's assumptions.
    assert!(human.contains("no base was built"), "{human}");
    let (j, _) = answer(vec![Ok(select("reached", vec![]))], true);
    let v: Value = serde_json::from_str(&j).unwrap();
    assert_eq!(v["schema"], "al-sem.impact");
    assert_eq!(v["input"]["diagnostics"].as_array().unwrap().len(), 2);
    assert_eq!(v["roots"][0]["delta"], Value::Null);
    assert_eq!(v["base"], Value::Null);
}

/// B-10: a test part that cannot be admitted leaves `tests: not computed`
/// with its reason, and the rest of the answer present.
///
/// DISCRIMINATION (2026-11-05): printing only the fallback's reason name
/// for `test-app-not-analyzed` (not its detail) in `render::tests_line`
/// fails at "the reason is stated"; restored, passes. Propagating the
/// select part's error with `?` in `select_parts` did NOT fail: the answer
/// still stands because `orchestrate` reports any tests failure as `tests:
/// not computed` (the property holds at two layers; recorded as a limit of
/// this test).
#[test]
fn a_test_part_not_admitted_is_reported_and_the_rest_stands() {
    let (human, calls) = answer(
        vec![Err("the engine refused it: over the memory budget".into())],
        false,
    );
    assert_eq!(calls, 3);
    assert!(human.contains("Codeunit A::R"), "{human}");
    assert!(human.contains("effects: may write 2 tables"), "{human}");
    assert!(
        human.contains("over the memory budget"),
        "the reason is stated: {human}"
    );
}

/// A root no test universe has gets its nearest tested caller: the nearest
/// production ancestor the test universe has and a test reaches (the probe
/// part), never "not reached".
///
/// DISCRIMINATION (2026-11-05): skipping the probe pass (`missing` left
/// empty) leaves `nearestTestedCaller` null: fails at the assertion on its
/// stable id; restored, passes.
#[test]
fn a_root_not_in_projection_gets_its_nearest_tested_caller() {
    let probe = json!({"stableId": CALLER, "status": "reached",
                       "nearest": "Codeunit T::X", "nearestTier": "local", "nearestHops": 2});
    let (j, calls) = answer(
        vec![
            Ok(select("not-in-projection", vec![])),
            Ok(select("not-in-projection", vec![probe.clone()])),
        ],
        true,
    );
    assert_eq!(calls, 4, "main, up, select, probe");
    let v: Value = serde_json::from_str(&j).unwrap();
    let n = &v["roots"][0]["tests"]["nearestTestedCaller"];
    assert_eq!(n["stableId"], CALLER, "{v:#}");
    assert_eq!(n["nearestTest"], "Codeunit T::X");
    assert_eq!(
        v["tests"]["changed"][0]["nearestTestedCaller"]["stableId"],
        CALLER
    );
    let (human, _) = answer(
        vec![
            Ok(select("not-in-projection", vec![])),
            Ok(select("not-in-projection", vec![probe])),
        ],
        false,
    );
    assert!(
        human.contains("nearest tested caller: Codeunit B::C (1 hop up), reached by Codeunit T::X"),
        "{human}"
    );
    assert!(!human.contains("not reached"), "{human}");
}

/// No base: nothing is called added or deleted, and the answer says the
/// effects are current with no before/after.
#[test]
fn without_a_base_no_root_is_added_or_deleted() {
    let (j, _) = answer(vec![Ok(select("reached", vec![]))], true);
    let v: Value = serde_json::from_str(&j).unwrap();
    for r in v["roots"].as_array().unwrap() {
        let st = r["status"].as_str().unwrap();
        assert!(!["added", "deleted"].contains(&st), "{st}");
    }
    assert!(
        v["assumptions"][0]
            .as_str()
            .unwrap()
            .contains("no before/after")
    );
}

/// B-11: the base side not admitted (its runner refuses with the reason):
/// the answer keeps the current side (accounting, roots, callers, tests)
/// and says why there is no before/after.
///
/// Break: propagate the base side's error out of `orchestrate_with` → no
/// answer at all.
#[test]
fn a_base_not_admitted_keeps_the_current_side_and_says_why() {
    let mut parts = vec![Ok(main_doc()), Ok(up_doc()), Ok(select("reached", vec![]))].into_iter();
    let mut run = |_: Target<'_>, _: Vec<String>| -> Result<Value, String> {
        parts.next().expect("one answer per part")
    };
    let mut bcx = super::super::before::BaseCx {
        cache: None,
        scratch: std::env::temp_dir(),
        diff_text: String::new(),
        run: Box::new(|_, _| Err("base not built: every place is held".into())),
    };
    let r = orchestrate(
        &ImpactRequest {
            delta: true,
            ..req()
        },
        super::super::compare::Comparison {
            source: "base",
            comparison_base: Some("0123456789abcdef".into()),
            ..Default::default()
        },
        RenderOpts::default(),
        &mut run,
        Some(&mut bcx),
    )
    .unwrap();
    assert!(
        r.text.contains("before/after: not computed — "),
        "{}",
        r.text
    );
    assert!(r.text.contains("Codeunit A::R"), "{}", r.text);
    assert!(r.text.contains("callers: 1 routine"), "{}", r.text);
}
