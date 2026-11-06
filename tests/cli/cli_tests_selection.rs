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

use serde_json::{Value, json};

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

/// C-6: a routine run only by a subscriber to a platform table event no code
/// raises (`TU Log`'s `OnAfterInsertEvent`; nothing writes `TU Log`) has a
/// selection-relevant hidden channel in its cone: no test reaches it, its
/// status is `analysis-incomplete` (never `no-known-path`), the fallback says
/// why, and `selectionComplete` is false. A routine with no caller at all and
/// no hidden channel is `no-known-path` with a complete selection. Since F1
/// (spec `2026-11-06-db-event-subscribers.md`) a table event a record
/// operation raises is an edge: `Audited`, run by the subscriber of
/// `TU Item`'s `OnAfterModifyEvent`, is reached from `PostUpdatesQty` through
/// `Post`'s `Item.Modify()`.
///
/// DISCRIMINATION (2026-11-05): treating a platform table event as covered
/// (`hidden_class` returning `false` for it) makes the unraised routine's
/// status `no-known-path` and `selectionComplete` true: this test fails on
/// both. (2026-11-06) Returning no route from `resolve_table_events` makes
/// `Audited` `analysis-incomplete` again: fails `audited`.
#[test]
fn a_platform_table_event_in_the_cone_keeps_the_selection_incomplete() {
    let t = universe_repo();
    let ws = t.path().join("App");
    let v = json(&ws, &["--routine", "TU Core::Audited"]);
    assert_eq!(v["changed"][0]["status"], "reached", "audited: {v:#}");
    let (human, _) = tests(&ws, &["--routine", "TU Core::Audited"]);
    golden("universe.audited.human.txt", &human, Some(t.path()));
    let v = json(&ws, &["--routine", "TU Core::Logged"]);
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
    let (human, _) = tests(&ws, &["--routine", "TU Core::Logged"]);
    assert!(human.find("selection complete: no").unwrap() < human.find("this is static").unwrap());
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

/// C-7: explain's `tests:` line reads the same scope and never analyzes it:
/// "1 test app discovered (…); not analyzed in this call" when a sibling
/// qualifies, the switched-off scope when `testApps: []`, and JSON
/// `testScope.analyzed: false`. (That explain starts no test engine is pinned
/// at the MCP level, `mcp_tests`.)
///
/// DISCRIMINATION (2026-11-05): dropping the `siblings` arm of explain's
/// tests line (reverting to "no test code loaded") fails the first
/// assertion.
///
/// The summary's `tests:` clause agrees with that line (milestone C item 1):
/// "none in the workspace; 1 test app discovered (not analyzed here; alsem
/// tests)", JSON `summary.testApps`; with `testApps: []` it is "none (no
/// test code loaded)" again.
/// DISCRIMINATION (2026-11-05): passing `None` for the summary's scope
/// (`totals.sibling_tests = None` in `Card::build`) fails the summary
/// assertion with "tests: none (no test code loaded)"; restored, passes.
#[test]
fn explain_names_the_discovered_test_apps_without_analyzing_them() {
    let t = universe_repo();
    let ws = t.path().join("App");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("explain")
            .arg(&ws)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let human = run(&["--routine", "TU Core::Post"]);
    assert!(
        human.contains(
            "  tests: no [Test] procedure in the workspace app; 1 test app discovered (TU Production Tests); not analyzed in this call — alsem tests (MCP: alsem_tests) selects them"
        ),
        "{human}"
    );
    let summary_tests = |human: &str| -> String {
        let l = human
            .lines()
            .find(|l| l.trim_start().starts_with("callers and tests:"))
            .unwrap_or_else(|| panic!("no callers and tests line: {human}"));
        l[l.find("; tests: ").expect("a tests clause") + 2..].to_string()
    };
    assert_eq!(
        summary_tests(&human),
        "tests: none in the workspace; 1 test app discovered (not analyzed here; alsem tests)",
        "{human}"
    );
    let v: Value =
        serde_json::from_str(&run(&["--routine", "TU Core::Post", "--format", "json"])).unwrap();
    assert_eq!(v["cards"][0]["testScope"]["analyzed"], false);
    assert_eq!(v["cards"][0]["testScope"]["apps"][0], "TU Production Tests");
    assert_eq!(
        v["cards"][0]["summary"]["testApps"],
        json!({"discovered": 1, "discoveryComplete": true})
    );
    std::fs::write(ws.join(".al-sem.json"), r#"{"testApps": []}"#).unwrap();
    let human = run(&["--routine", "TU Core::Post"]);
    assert!(
        human.contains("sibling test apps turned off (testApps: [] in .al-sem.json)"),
        "{human}"
    );
    assert_eq!(
        summary_tests(&human),
        "tests: none (no test code loaded)",
        "{human}"
    );
}

/// R0-3: explain's `callers:` count (the model's call adjacency) equals the
/// reverse cone's direct callers (`select::direct_callers` over the sited
/// graph) for every workspace routine of the fixture. explain keeps reading
/// the model: building the sited graph for every explain would cost each
/// edit 0.93-1.19 s and 45 MiB on CDO (M0's measured lazy-graph decision);
/// this test is what keeps the two readers from drifting apart. The second
/// fixture, `caller-relations`, adds a page extension: a run of the base page
/// runs the extension's `OnOpenPage` (the resolver's `run_triggers`), and
/// its action is a page action (F16j), not a routine caller.
#[test]
fn explain_callers_equal_the_cones_direct_callers() {
    explain_callers_equal_direct_callers_in("tests/fixtures/test-selection-graph", 20);
    explain_callers_equal_direct_callers_in("tests/fixtures/caller-relations/ws", 18);
}

fn explain_callers_equal_direct_callers_in(fixture: &str, min_checked: usize) {
    use al_sem::engine::test_select::select::direct_callers;
    let ws = repo_root().join(fixture);
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let p = al_sem::program::resolved::ResolvedProgram::build(
        &inputs,
        al_sem::program::resolved::ProgramScope::CrossApp,
    )
    .unwrap();
    let g = al_sem::program::impact_graph::sited_from_program(&p, &|| false).unwrap();
    let mut checked = 0;
    let mut differ = Vec::new();
    for n in 0..g.node_count() as u32 {
        let (Some(id), Some(_)) = (g.stable_id(n), g.local_name(n)) else {
            continue;
        };
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(["explain"])
            .arg(&ws)
            .args(["--routine", &id, "--format", "json"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let model = v["cards"][0]["summary"]["callers"].as_u64().unwrap_or(0) as usize;
        let callers = direct_callers(&g, n);
        let cone = callers.len();
        if model != cone {
            let names: Vec<String> = callers.iter().map(|c| g.display(*c)).collect();
            differ.push(format!(
                "{}: explain {model}, cone {cone} ({})",
                g.display(n),
                names.join(", ")
            ));
        }
        checked += 1;
    }
    assert!(checked >= min_checked, "{fixture}: {checked}");
    assert!(differ.is_empty(), "{fixture}:\n{}", differ.join("\n"));
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

/// C-9 (CDO ratchet, `scripts/cdo-gate`): `alsem tests Cloud --routine
/// "CDO Telemetry::UpdateLastUpload"` lists exactly the 3 `CDO Telemetry
/// Tests` tests, tier local, 1 hop (the pruned export has no such routine;
/// the sited graph does). The sibling test app is auto-discovered beside the
/// pinned baseline (`DO-cdo-baseline/Test`). Also RunReportID's 68 local
/// tests in 7 codeunits and IssueReminder's 0 local with a nearest test
/// through dependency code; `selectionComplete` false with its reasons.
/// And (milestone C item 4, C-12 on CDO) RunReportID's human answer prints
/// the dependency-tier sameness line exactly: its 597 through-dependency
/// tests reach it via dependency nodes that may execute most of the program
/// (301 until F1, 2026-11-06: the record operations of the posting tests'
/// dependency code now raise the table events whose subscribers reach it).
/// DISCRIMINATION (2026-11-05, cdo profile): `render`'s `same` never true
/// fails it ("through dependency code: 301 tests (--tier dependency lists
/// them)"); restored, passes.
/// Run by `output_cdo::cdo_test_selection_ratchets` (the module `cdo-gate`
/// runs).
pub(crate) fn cdo_selection_ratchets() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let (human, code) = tests(&ws, &["--routine", "CDO E-Mail Handler::RunReportID"]);
    assert_eq!(code, 0, "{human}");
    assert!(
        human.lines().any(|l| l
            == "through dependency code: 597 tests — effectively the suite's posting tests, not specific to this change"),
        "{human}"
    );
    let v = json(&ws, &["--routine", "CDO Telemetry::UpdateLastUpload"]);
    let names: Vec<&str> = v["runFirst"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["procedure"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for t in v["runFirst"].as_array().unwrap() {
        assert_eq!(t["codeunit"], "CDO Telemetry Tests");
        assert_eq!(t["tier"], "local");
        assert_eq!(t["hops"], 1);
    }
    for n in [
        "UpdateLastUpload_ShouldSetLastTelemetryUploadToToday",
        "UpdateLastUpload_WhenNeverUploaded",
        "UpdateLastUpload_WhenCalledMultipleTimes",
    ] {
        assert!(names.iter().any(|x| x.starts_with(n)), "{n}: {names:?}");
    }
    let v = json(
        &ws,
        &[
            "--routine",
            "CDO E-Mail Handler::RunReportID",
            "--routine",
            "CDO Issue Document::IssueReminder",
        ],
    );
    let local: Vec<&Value> = v["runFirst"].as_array().unwrap().iter().collect();
    assert_eq!(local.len(), 68);
    let codeunits: std::collections::BTreeSet<&str> = local
        .iter()
        .map(|t| t["codeunit"].as_str().unwrap())
        .collect();
    assert_eq!(codeunits.len(), 7, "{codeunits:?}");
    let ir = v["changed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["display"] == "Codeunit CDO Issue Document::IssueReminder")
        .expect("IssueReminder resolved");
    assert_eq!(ir["nearest"]["tier"], "throughDependency", "{ir}");
    assert_eq!(v["selectionComplete"], false);
    let reasons: Vec<&str> = v["requiredFallback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"hidden-callers"), "{reasons:?}");
    assert!(reasons.contains(&"unresolved-test-calls"), "{reasons:?}");
}

/// C-8 (agent-tasks §4.8): `alsem graph` on a test app naming the production
/// app as a source dependency keeps every production routine through pruning
/// by default (`Untested`, which nothing calls, included) and names the
/// folder in `meta.apps[].sourceRoot`; `--keep-local-sources=false` prunes as
/// before. (With no local-source app loaded the export is byte-identical:
/// `cli_graph_export`'s golden.)
///
/// DISCRIMINATION (2026-11-05): defaulting `keep_local` to false drops
/// `Untested` from the default export: fails.
#[test]
fn graph_keeps_local_source_routines() {
    let t = universe_repo();
    let test = t.path().join("Test");
    std::fs::write(
        test.join("al-sem.dependencies.json"),
        r#"{ "sources": ["../App"] }"#,
    )
    .unwrap();
    let graph = |args: &[&str]| -> Vec<Value> {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("graph")
            .arg(&test)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let has_untested = |rows: &[Value]| {
        rows.iter()
            .any(|r| r["t"] == "node" && r["v"]["memberName"].as_str() == Some("Untested"))
    };
    let rows = graph(&[]);
    let app = rows[0]["v"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "TU Production")
        .unwrap()
        .clone();
    assert_eq!(app["source"], "localApproximate");
    assert!(
        app["sourceRoot"].as_str().unwrap().ends_with("App"),
        "{app}"
    );
    assert!(has_untested(&rows), "kept by default");
    assert!(!has_untested(&graph(&["--keep-local-sources=false"])));
}
