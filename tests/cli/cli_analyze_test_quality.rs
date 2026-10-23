//! #253: `al-call-hierarchy --project <ws> --analyze --format json` gains a
//! `test_quality` block: one row per `[Test]` procedure with its assert counts,
//! summed per test codeunit and for the workspace. Driven through the SHIPPED
//! binary on `tests/fixtures/analyze-test-quality/` (with a nested app under
//! `nested/`) and `tests/fixtures/analyze-test-quality-noapp/` (no app.json,
//! so no call graph).
//!
//! The nested app's `app.json` is committed as `nested/app.json.tmpl` and
//! written only into a tempdir copy (as #159's nested-app test does): a
//! committed nested app would break the #160 cross-app fixture sweep
//! (`adapter_loses_nothing_cross_app_on_the_fixtures`), because a nested
//! folder's sites are program-only by design
//! (`program_calls::tests::nested_app_json_folder_is_program_only`).
//!
//! Every expected number is hand-counted from the fixture text.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::cli_analyze_call_hierarchy_json::{repo_root, run_analyze, run_analyze_in, stdout_text};

const FIXTURE: &str = "tests/fixtures/analyze-test-quality";
const NOAPP_FIXTURE: &str = "tests/fixtures/analyze-test-quality-noapp";
const GUID: &str = "25300000-0000-4000-8000-000000000253";
const MAIN: &str = "src/Tests/Main.Codeunit.al";
const SPLIT: &str = "src/Tests/Split.Codeunit.al";
const NESTED: &str = "nested/src/NestedTests.Codeunit.al";

/// The `--format json` stdout of a tempdir copy of [`FIXTURE`] with its
/// nested app's `app.json` written from the template.
fn fixture_json_raw() -> String {
    let ws = tempfile::tempdir().expect("workspace");
    crate::copy_dir(&repo_root().join(FIXTURE), ws.path());
    std::fs::rename(
        ws.path().join("nested/app.json.tmpl"),
        ws.path().join("nested/app.json"),
    )
    .expect("materialize the nested app.json");
    stdout_text(&run_analyze_in(ws.path(), ".", "json"))
}

fn analyze(project: &str) -> Value {
    let raw = if project == FIXTURE {
        fixture_json_raw()
    } else {
        stdout_text(&run_analyze(project, "json"))
    };
    serde_json::from_str(&raw).expect("valid JSON")
}

fn tq(v: &Value) -> &Value {
    v.get("test_quality")
        .unwrap_or_else(|| panic!("no `test_quality` key in the analyze JSON"))
}

fn procedures(v: &Value) -> &Vec<Value> {
    tq(v)["procedures"]
        .as_array()
        .expect("test_quality.procedures[]")
}

fn codeunits(v: &Value) -> &Vec<Value> {
    tq(v)["codeunits"]
        .as_array()
        .expect("test_quality.codeunits[]")
}

/// A row's field, asserting the key is PRESENT (missing and `null` differ).
fn field<'a>(r: &'a Value, key: &str) -> &'a Value {
    r.get(key)
        .unwrap_or_else(|| panic!("row has no `{key}` field: {r}"))
}

/// The one procedure row at `(path, line)`.
fn proc_at<'a>(v: &'a Value, path: &str, line: u64) -> &'a Value {
    let hits: Vec<&Value> = procedures(v)
        .iter()
        .filter(|r| r["path"] == path && r["line"] == line)
        .collect();
    assert_eq!(hits.len(), 1, "one test row at {path}:{line}: {hits:#?}");
    hits[0]
}

/// `(assert_count, assert_count_reachable, zero_assert, has_asserterror)`.
fn counts(r: &Value) -> (Value, Value, Value, Value) {
    (
        field(r, "assert_count").clone(),
        field(r, "assert_count_reachable").clone(),
        field(r, "zero_assert").clone(),
        field(r, "has_asserterror").clone(),
    )
}

fn c(own: u64, reachable: u64, asserterror: bool) -> (Value, Value, Value, Value) {
    (
        own.into(),
        reachable.into(),
        (reachable == 0).into(),
        asserterror.into(),
    )
}

/// Hand list of every `[Test]` procedure: (path, line, name).
fn hand_tests() -> Vec<(&'static str, u64, &'static str)> {
    vec![
        (NESTED, 10, "NestedTest"),
        (MAIN, 13, "ZeroAssert"),
        (MAIN, 22, "OnlyAssertError"),
        (MAIN, 28, "AreEqualShapes"),
        (MAIN, 73, "HiddenAssert"),
        (MAIN, 81, "RenamedAssert"),
        (MAIN, 90, "ThisAssert"),
        (MAIN, 96, "ParamReceiver"),
        (MAIN, 107, "Traversal"),
        (MAIN, 144, "InBodyIf"),
        (MAIN, 154, "ReachesCycle"),
        (MAIN, 180, "CrossCodeunit"),
        (MAIN, 186, "CallsNested"),
        (MAIN, 192, "CallsTwin"),
        (MAIN, 198, "DirectOnly"),
        (SPLIT, 9, "U"),
        (SPLIT, 16, "T"),
        (SPLIT, 23, "T"),
    ]
}

// ── S11: every fixture file parses cleanly ───────────────────────────────

#[test]
fn fixtures_parse_without_error() {
    let mut files = 0;
    for dir in [FIXTURE, NOAPP_FIXTURE] {
        for entry in walkdir::WalkDir::new(repo_root().join(dir)) {
            let entry = entry.expect("walk");
            if entry.path().extension().is_none_or(|e| e != "al") {
                continue;
            }
            let source = std::fs::read_to_string(entry.path()).expect("read");
            let file = al_syntax::parse(&source);
            assert!(
                !file.objects.is_empty(),
                "{}: no object",
                entry.path().display()
            );
            for obj in &file.objects {
                for r in &obj.routines {
                    assert!(
                        !r.parse_incomplete,
                        "{}: routine {} did not parse cleanly",
                        entry.path().display(),
                        r.name
                    );
                }
            }
            files += 1;
        }
    }
    assert_eq!(files, 10, "fixture precondition: .al file count");
}

// ── A1: the row set, and the join with `metrics` ─────────────────────────

#[test]
fn a1_rows_are_the_tests_and_join_their_metrics_rows() {
    let v = analyze(FIXTURE);
    let got: BTreeSet<(String, u64, String)> = procedures(&v)
        .iter()
        .map(|r| {
            (
                r["path"].as_str().expect("path").to_string(),
                r["line"].as_u64().expect("line"),
                r["procedure_name"].as_str().expect("name").to_string(),
            )
        })
        .collect();
    let want: BTreeSet<(String, u64, String)> = hand_tests()
        .into_iter()
        .map(|(p, l, n)| (p.to_string(), l, n.to_string()))
        .collect();
    assert_eq!(got, want, "the [Test] procedure set");
    assert_eq!(procedures(&v).len(), want.len(), "no duplicate rows");

    let metrics = v["metrics"].as_array().expect("metrics");
    for r in procedures(&v) {
        let joined: Vec<&Value> = metrics
            .iter()
            .filter(|m| m["path"] == r["path"] && m["line"] == r["line"])
            .collect();
        assert_eq!(joined.len(), 1, "exactly one metrics row for {r}");
        for k in [
            "object_type",
            "object_name",
            "object_id",
            "procedure_name",
            "routine_id",
        ] {
            assert_eq!(field(r, k), field(joined[0], k), "{k} of {r}");
        }
        let stable = field(r, "object_stable_id");
        if r["path"] == NESTED {
            assert_eq!(field(r, "routine_id"), &Value::Null, "nested: {r}");
            assert_eq!(stable, &Value::Null, "nested object_stable_id: {r}");
        } else {
            assert!(field(r, "routine_id").is_string(), "root routine_id: {r}");
            let want = format!("{GUID}/Codeunit/{}", r["object_id"]);
            assert_eq!(stable, want.as_str(), "object_stable_id of {r}");
        }
    }
}

// ── A1b: no app.json -> no graph -> reachable fields null ────────────────

#[test]
fn a1b_without_a_graph_reachable_and_zero_assert_are_null() {
    let v = analyze(NOAPP_FIXTURE);
    let rows = procedures(&v);
    assert_eq!(rows.len(), 2, "precondition: two [Test] rows");
    let own: Vec<u64> = rows
        .iter()
        .map(|r| {
            field(r, "assert_count")
                .as_u64()
                .expect("numeric own count")
        })
        .collect();
    assert_eq!(own, [1, 0]);
    for r in rows {
        for k in [
            "routine_id",
            "object_stable_id",
            "assert_count_reachable",
            "zero_assert",
        ] {
            assert_eq!(field(r, k), &Value::Null, "{k} without a graph: {r}");
        }
    }
    let cu = &codeunits(&v)[0];
    let ws = &tq(&v)["workspace"];
    for sum in [cu, ws] {
        assert_eq!(field(sum, "assert_count"), 1, "{sum}");
        assert_eq!(field(sum, "assert_count_reachable"), &Value::Null, "{sum}");
        assert_eq!(field(sum, "zero_assert_procedures"), &Value::Null, "{sum}");
        assert_eq!(field(sum, "test_procedures"), 2, "{sum}");
    }
}

// ── A2 / A2b: zero asserts, and asserterror alone ────────────────────────

#[test]
fn a2_non_assert_calls_do_not_count_and_asserterror_does() {
    let v = analyze(FIXTURE);
    assert_eq!(counts(proc_at(&v, MAIN, 13)), c(0, 0, false), "ZeroAssert");
    assert_eq!(
        counts(proc_at(&v, MAIN, 22)),
        c(1, 1, true),
        "OnlyAssertError"
    );
}

// ── A3: the AreEqual expected-value split ─────────────────────────────────

#[test]
fn a3_are_equal_literal_and_expression_split() {
    let v = analyze(FIXTURE);
    let r = proc_at(&v, MAIN, 28);
    assert_eq!(counts(r), c(9, 9, false), "AreEqualShapes");
    assert_eq!(
        field(r, "are_equal"),
        &serde_json::json!({"total": 9, "literal_expected": 6, "expression_expected": 3}),
    );
}

// ── A6: handlers, permissions and helpers are not tests ──────────────────

#[test]
fn a6_non_test_attributes_give_no_row() {
    let v = analyze(FIXTURE);
    let names: BTreeSet<&str> = procedures(&v)
        .iter()
        .map(|r| r["procedure_name"].as_str().expect("name"))
        .collect();
    for not_a_test in [
        "PermissionsOnly",
        "ConfirmYes",
        "MessageSink",
        "VerifyX",
        "Check",
    ] {
        assert!(!names.contains(not_a_test), "{not_a_test} is not a test");
    }
}

// ── A7: a test and a helper split across object-level #if arms ───────────

#[test]
fn a7_split_declarations() {
    let v = analyze(FIXTURE);
    let (arm1, arm2) = (proc_at(&v, SPLIT, 16), proc_at(&v, SPLIT, 23));
    assert!(field(arm1, "routine_id").is_string());
    assert_eq!(
        field(arm1, "routine_id"),
        field(arm2, "routine_id"),
        "precondition: both arms share one id"
    );
    assert_eq!(
        counts(arm1),
        c(1, 4, false),
        "T arm 1: own 1 + HelperA 1 + HelperB 2"
    );
    assert_eq!(
        counts(arm2),
        c(2, 5, false),
        "T arm 2: own 2 + HelperA 1 + HelperB 2"
    );
    assert_eq!(
        counts(proc_at(&v, SPLIT, 9)),
        c(0, 3, false),
        "U: S's two arms summed"
    );
}

// ── A8 / A9: the receiver is matched by its declared type ────────────────

#[test]
fn a8_a9_receiver_by_type_scope_and_this() {
    let v = analyze(FIXTURE);
    assert_eq!(
        counts(proc_at(&v, MAIN, 73)),
        c(0, 0, false),
        "HiddenAssert"
    );
    assert_eq!(
        counts(proc_at(&v, MAIN, 81)),
        c(2, 2, false),
        "RenamedAssert"
    );
    assert_eq!(counts(proc_at(&v, MAIN, 90)), c(1, 1, false), "ThisAssert");
    assert_eq!(
        counts(proc_at(&v, MAIN, 96)),
        c(0, 1, false),
        "ParamReceiver reaches Check's parameter assert"
    );
}

// ── A10 / A11: every statement slot, and in-body #if ─────────────────────

#[test]
fn a10_a11_walker_visits_every_slot() {
    let v = analyze(FIXTURE);
    assert_eq!(counts(proc_at(&v, MAIN, 107)), c(12, 12, true), "Traversal");
    assert_eq!(
        counts(proc_at(&v, MAIN, 144)),
        c(2, 2, false),
        "InBodyIf: both arms count"
    );
}

// ── A12 / A14 / A14b: the reachable count ────────────────────────────────

#[test]
fn a12_a14_reachable_count() {
    let v = analyze(FIXTURE);
    assert_eq!(
        counts(proc_at(&v, MAIN, 154)),
        c(0, 3, false),
        "ReachesCycle: H 1 + H2 2, once each; the event subscriber is not followed"
    );
    assert_eq!(
        counts(proc_at(&v, MAIN, 180)),
        c(0, 1, false),
        "CrossCodeunit"
    );
    assert_eq!(counts(proc_at(&v, MAIN, 198)), c(1, 1, false), "DirectOnly");
    assert_eq!(
        counts(proc_at(&v, NESTED, 10)),
        c(1, 2, false),
        "NestedTest"
    );

    // A14: a root test does not count a nested app's helper. Precondition:
    // the call resolves (the helper has callers), so the app filter is what
    // excludes it.
    let metrics = v["metrics"].as_array().expect("metrics");
    let fan_in = |path: &str, name: &str| {
        metrics
            .iter()
            .find(|m| m["path"] == path && m["procedure_name"] == name)
            .and_then(|m| m["fan_in"].as_u64())
            .unwrap_or_else(|| panic!("no fan_in for {path} {name}"))
    };
    assert!(fan_in("nested/src/NestedHelper.Codeunit.al", "NestedHelper") >= 1);
    assert_eq!(
        counts(proc_at(&v, MAIN, 186)),
        c(0, 0, false),
        "CallsNested"
    );

    // A14b: root `TQ Twin` and nested `TQ Twin Nested` are both codeunit
    // 50260, so their `Twin` routines share one RoutineNodeId. Only the nested
    // one calls the root assert helper. Precondition: that call resolves.
    assert!(fan_in("src/Helpers/RootAssertHelper.Codeunit.al", "Check") >= 1);
    assert!(fan_in("src/Helpers/Twin.Codeunit.al", "Twin") >= 1);
    assert_eq!(counts(proc_at(&v, MAIN, 192)), c(0, 0, false), "CallsTwin");
}

// ── A4: codeunit and workspace sums ──────────────────────────────────────

#[test]
fn a4_sums_equal_their_procedures() {
    let v = analyze(FIXTURE);
    let keys = [
        "assert_count",
        "assert_count_reachable",
        "zero_assert_procedures",
        "asserterror_procedures",
        "test_procedures",
    ];
    let sum = |rows: &[&Value]| -> Vec<u64> {
        let n = |r: &Value, k: &str| r[k].as_u64().unwrap_or_else(|| panic!("{k}: {r}"));
        vec![
            rows.iter().map(|r| n(r, "assert_count")).sum(),
            rows.iter().map(|r| n(r, "assert_count_reachable")).sum(),
            rows.iter().filter(|r| r["zero_assert"] == true).count() as u64,
            rows.iter().filter(|r| r["has_asserterror"] == true).count() as u64,
            rows.len() as u64,
        ]
    };
    let are_equal = |rows: &[&Value]| -> Vec<u64> {
        ["total", "literal_expected", "expression_expected"]
            .iter()
            .map(|k| {
                rows.iter()
                    .map(|r| r["are_equal"][k].as_u64().expect("are_equal"))
                    .sum()
            })
            .collect()
    };
    let read = |s: &Value| -> (Vec<u64>, Vec<u64>) {
        (
            keys.iter()
                .map(|k| field(s, k).as_u64().unwrap_or_else(|| panic!("{k}: {s}")))
                .collect(),
            ["total", "literal_expected", "expression_expected"]
                .iter()
                .map(|k| s["are_equal"][k].as_u64().expect("are_equal"))
                .collect(),
        )
    };

    // Hand totals: (assert, reachable, zero, asserterror, tests), are_equal.
    let hand = [
        (NESTED, vec![1, 2, 0, 0, 1], vec![0, 0, 0]),
        (MAIN, vec![28, 33, 4, 2, 14], vec![9, 6, 3]),
        (SPLIT, vec![3, 12, 0, 0, 3], vec![0, 0, 0]),
    ];
    assert_eq!(codeunits(&v).len(), hand.len(), "one row per test codeunit");
    let all: Vec<&Value> = procedures(&v).iter().collect();
    for (cu, (path, want, want_eq)) in codeunits(&v).iter().zip(&hand) {
        assert_eq!(cu["path"], *path, "codeunit order");
        let mine: Vec<&Value> = all.iter().copied().filter(|r| r["path"] == *path).collect();
        // Precondition (S9): the last test of each codeunit asserts, so
        // dropping it from the sums moves them.
        let last = mine.last().expect("a test");
        assert!(last["assert_count"].as_u64() > Some(0), "{last}");
        assert!(last["assert_count_reachable"].as_u64() > Some(0), "{last}");
        assert_eq!(
            read(cu),
            (sum(&mine), are_equal(&mine)),
            "computed sums of {cu}"
        );
        assert_eq!(
            read(cu),
            (want.clone(), want_eq.clone()),
            "hand sums of {cu}"
        );
    }
    let ws = &tq(&v)["workspace"];
    assert_eq!(read(ws), (sum(&all), are_equal(&all)), "workspace computed");
    assert_eq!(
        read(ws),
        (vec![32, 47, 4, 2, 18], vec![9, 6, 3]),
        "workspace hand totals"
    );
}

// ── A5: deterministic output and order ───────────────────────────────────

#[test]
fn a5_output_is_deterministic_and_in_path_line_order() {
    let first = fixture_json_raw();
    let second = fixture_json_raw();
    assert_eq!(first, second, "two runs differ");
    let v: Value = serde_json::from_str(&first).expect("json");
    for rows in [procedures(&v), codeunits(&v)] {
        let order: Vec<(String, u64)> = rows
            .iter()
            .map(|r| {
                (
                    r["path"].as_str().expect("path").to_string(),
                    r["line"].as_u64().expect("line"),
                )
            })
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "rows must be in (path, line) order");
    }
}
