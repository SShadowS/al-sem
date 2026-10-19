//! #70 (spec `docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-design.md`,
//! Acceptance rows 1a-1f): `alsem events fanout`, `alsem events chains` and
//! `alsem policy check` load the workspace's dependencies by default (the
//! cross-app base `alsem analyze` builds), `--single-app` analyses the workspace
//! alone, and every JSON document states which apps were loaded and how
//! (a top-level `dependencies` object), as `digest` and `query` do since #66.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem`, so the clap arm and its default are
//! pinned, not only the library pipelines (row 1d).
//!
//! The fixture is `tests/common/dep_dispatch_fixture.rs`: a dependency `XBase`
//! (embedded source) declaring the global integration event `OnDepEvent`, and a
//! workspace whose `Ws Sub.HandleInWs` subscribes to it.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture::{DEP_GUID, WS_AL, write_own};

const VERSION_OVERRIDE: &str = "cli-deps-v1";

/// `alsem <args> --deterministic` -> (stdout, stderr, exit code).
fn alsem(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--deterministic")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).replace('\r', ""),
        out.status.code().unwrap_or(-1),
    )
}

fn json(args: &[&str]) -> (Value, i32) {
    let mut a = args.to_vec();
    a.extend(["--format", "json"]);
    let (out, err, code) = alsem(&a);
    (
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: exit {code}: stdout {out:?} stderr {err}")),
        code,
    )
}

/// `dependencies.apps` as `(appId, source)` pairs.
fn app_sources(doc: &Value) -> Vec<(String, String)> {
    doc["dependencies"]["apps"]
        .as_array()
        .unwrap_or_else(|| panic!("no top-level dependencies.apps: {doc}"))
        .iter()
        .map(|a| {
            (
                a["appId"].as_str().unwrap_or("").to_string(),
                a["source"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn dep_event_id() -> String {
    format!("{DEP_GUID}/Codeunit/10/event/ondepevent")
}

fn embedded() -> Vec<(String, String)> {
    vec![(DEP_GUID.to_string(), "embedded".to_string())]
}

/// Every `event-dispatch` node of a chains document, anywhere in the tree.
fn dispatch_nodes(doc: &Value) -> Vec<&Value> {
    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        if v["kind"] == "event-dispatch" {
            out.push(v);
        }
        for c in v["children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    for c in doc["chains"].as_array().into_iter().flatten() {
        walk(c, &mut out);
    }
    out
}

/// Rows 1a, 1d, 1e. Default (`--scope primary`): an entry for the dependency's
/// `OnDepEvent` with at least one direct subscriber, and the statement
/// `{mode: crossApp, apps: [XBase embedded]}`; the human output starts with the
/// `dependencies:` line. `--single-app`: no such entry, `{mode: singleApp,
/// apps: []}`, no human line.
///
/// Mutation: build the single-app model in the default branch of
/// `run_events_fanout` -> no `OnDepEvent` entry (today's output, `totalEvents: 0`).
#[test]
fn events_fanout_reports_a_dependency_event_with_a_workspace_subscriber() {
    let dir = tempfile::tempdir().unwrap();
    write_own(dir.path(), "");
    let ws = dir.path().to_string_lossy().to_string();

    let (doc, code) = json(&["events", "fanout", &ws]);
    assert_eq!(code, 0, "{doc}");
    let entry = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["eventId"] == dep_event_id());
    let entry =
        entry.unwrap_or_else(|| panic!("no fanout entry for the dependency's OnDepEvent: {doc}"));
    assert!(
        entry["directSubscriberCount"].as_u64().unwrap_or(0) >= 1,
        "{entry}"
    );
    assert_eq!(doc["dependencies"]["mode"], "crossApp", "{doc}");
    assert_eq!(app_sources(&doc), embedded(), "{doc}");
    let (human, _, code) = alsem(&["events", "fanout", &ws, "--format", "human"]);
    assert_eq!(code, 0);
    assert!(
        human.starts_with("dependencies: XBase (embedded)\n"),
        "{human}"
    );

    let (single, code) = json(&["events", "fanout", &ws, "--single-app"]);
    assert_eq!(code, 0, "{single}");
    assert!(
        !single["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["eventId"] == dep_event_id()),
        "{single}"
    );
    assert_eq!(single["dependencies"]["mode"], "singleApp", "{single}");
    assert!(app_sources(&single).is_empty(), "{single}");
    let (human, _, _) = alsem(&["events", "fanout", &ws, "--format", "human", "--single-app"]);
    assert!(!human.starts_with("dependencies:"), "{human}");
}

/// Rows 1b, 1d, 1e. Default: a chain walks into the dependency's `OnDepEvent`
/// (an `event-dispatch` node with at least one subscriber); `--single-app`: no
/// such node.
///
/// Mutation: build the single-app model in the default branch of
/// `run_events_chains` -> no dispatch node (today: `totalRoots: 0`).
#[test]
fn events_chains_walks_a_dependency_event_with_a_workspace_subscriber() {
    let dir = tempfile::tempdir().unwrap();
    write_own(dir.path(), "");
    let ws = dir.path().to_string_lossy().to_string();

    let (doc, code) = json(&["events", "chains", &ws]);
    assert_eq!(code, 0, "{doc}");
    let nodes = dispatch_nodes(&doc);
    let node = nodes
        .iter()
        .find(|n| n["eventId"] == dep_event_id())
        .unwrap_or_else(|| panic!("no chain dispatches the dependency's OnDepEvent: {doc}"));
    assert!(
        node["children"]
            .as_array()
            .is_some_and(|c| c.iter().any(|s| s["kind"] == "subscriber")),
        "{node}"
    );
    assert_eq!(doc["dependencies"]["mode"], "crossApp", "{doc}");
    assert_eq!(app_sources(&doc), embedded(), "{doc}");
    let (human, _, code) = alsem(&["events", "chains", &ws, "--format", "human"]);
    assert_eq!(code, 0);
    assert!(
        human.starts_with("dependencies: XBase (embedded)\n"),
        "{human}"
    );

    let (single, code) = json(&["events", "chains", &ws, "--single-app"]);
    assert_eq!(code, 0, "{single}");
    assert!(
        !dispatch_nodes(&single)
            .iter()
            .any(|n| n["eventId"] == dep_event_id()),
        "{single}"
    );
    assert_eq!(single["dependencies"]["mode"], "singleApp", "{single}");
    assert!(app_sources(&single).is_empty(), "{single}");
    let (human, _, _) = alsem(&["events", "chains", &ws, "--format", "human", "--single-app"]);
    assert!(!human.starts_with("dependencies:"), "{human}");
}

// ---------------------------------------------------------------------------
// policy check (row 1c)
// ---------------------------------------------------------------------------

/// The policy tests' extra workspace subscriber: it reaches the dependency's
/// `Inner` (insert + `Commit`) only through `Lib.Outer()`.
const COMMIT_SUB_AL: &str = r#"codeunit 50172 "Ws Commit Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Lib", 'OnDepEvent', '', false, false)]
    local procedure HandleCommit()
    var
        Lib: Codeunit "Dep Lib";
    begin
        Lib.Outer();
    end;
}
"#;

/// The policy fixture's WORKSPACE routines, hand-counted: `CallChain`,
/// `RaiseDep`, `TouchCust` (`Ws Work`), `HandleInWs` (`Ws Sub`), `HandleCommit`
/// (`Ws Commit Sub`). The dependency's `Outer`, `Inner`, `OnDepEvent`,
/// `HandleDep` are not evaluated in either mode.
const WS_ROUTINES: u64 = 5;

/// One rule: a DIRECT insert into a table named `Customer`. Only `TouchCust`
/// has one; the table is the dependency's, so it has a name cross-app only.
const DIRECT_POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: dep-customer-insert
    title: "Direct insert into the dependency's Customer"
    severity: high
    when:
      capability.op: insert
      capability.resource.table.name: "Customer"
    facts: direct
"#;

/// One rule: an INHERITED insert, on `CallChain` only (which reaches the
/// dependency's insert through `Outer` -> `Inner` and nothing else). The
/// `routine.name` restriction keeps `RaiseDep` (which may inherit its
/// subscribers' inserts) out of the count.
const INHERITED_POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: inherited-insert
    title: "Inherited insert"
    severity: high
    when:
      routine.name: "CallChain"
      capability.op: insert
    facts: inherited
"#;

/// The policy fixture + one policy file; returns (workspace, policy path).
fn policy_fixture(dir: &Path, policy: &str) -> (String, String) {
    let ws = dir.join("ws");
    write_own(&ws, COMMIT_SUB_AL);
    let p = dir.join("policy.yaml");
    std::fs::write(&p, policy).unwrap();
    (
        ws.to_string_lossy().to_string(),
        p.to_string_lossy().to_string(),
    )
}

/// The 0-based line of `procedure <name>()` in `src` (a finding's
/// `primaryLocation.range.startLine` convention).
fn decl_line(src: &str, name: &str) -> u64 {
    let needle = format!("procedure {name}()");
    let hits: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(&needle))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "{name}: {hits:?}");
    hits[0] as u64
}

/// A rule summary's counters, in the JSON's order: evaluated, matched,
/// skippedCoverage, skippedUnknown, passed, findingsEmitted.
fn counts(doc: &Value, rule: &str) -> [u64; 6] {
    let s = doc["ruleSummaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["ruleId"] == rule)
        .unwrap_or_else(|| panic!("no summary for {rule}: {doc}"));
    [
        "routinesEvaluated",
        "routinesMatched",
        "routinesSkippedCoverage",
        "routinesSkippedUnknown",
        "routinesPassed",
        "findingsEmitted",
    ]
    .map(|k| s[k].as_u64().unwrap_or(u64::MAX))
}

/// `(detector, sourceUnitId, startLine)` of every finding.
fn findings(doc: &Value) -> Vec<(String, String, u64)> {
    doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["detector"].as_str().unwrap_or("").to_string(),
                f["primaryLocation"]["sourceUnitId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                f["primaryLocation"]["range"]["startLine"]
                    .as_u64()
                    .unwrap_or(u64::MAX),
            )
        })
        .collect()
}

fn no_dep_anchor(doc: &Value) {
    for (det, unit, _) in findings(doc) {
        assert!(!unit.starts_with("dep:"), "{det} anchored in {unit}: {doc}");
    }
}

/// Rows 1c (first and fourth parts), 1d, 1e. The direct rule: default (cross-app)
/// -> 1 finding on `TouchCust`; `--single-app` -> 0 (the table has no name
/// there, so the fact is unknown and the rule fails open). `routinesEvaluated`
/// is the hand-counted number of WORKSPACE routines in both modes; the other
/// counters are hand-stated per mode:
///   cross-app:  evaluated 5, matched 1, skipped-coverage 0, skipped-unknown 0,
///               passed 4 (CallChain, RaiseDep, HandleInWs, HandleCommit), findings 1
///   single-app: evaluated 5, matched 0, skipped-coverage 0, skipped-unknown 1
///               (TouchCust), passed 4, findings 0
///
/// Mutations: single-app model in the default branch -> 0 findings; evaluating
/// dependency routines (filtering only findings) -> `routinesEvaluated` 9.
#[test]
fn policy_check_resolves_a_dependency_table_name() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, policy) = policy_fixture(dir.path(), DIRECT_POLICY);

    let (doc, code) = json(&["policy", "check", &ws, "--policy", &policy]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        findings(&doc),
        vec![(
            "policy-dep-customer-insert".to_string(),
            "ws:src/Work.al".to_string(),
            decl_line(WS_AL, "TouchCust"),
        )],
        "{doc}"
    );
    assert_eq!(
        counts(&doc, "dep-customer-insert"),
        [WS_ROUTINES, 1, 0, 0, 4, 1],
        "{doc}"
    );
    no_dep_anchor(&doc);
    assert_eq!(doc["dependencies"]["mode"], "crossApp", "{doc}");
    assert_eq!(app_sources(&doc), embedded(), "{doc}");
    let (human, _, code) = alsem(&[
        "policy", "check", &ws, "--policy", &policy, "--format", "human",
    ]);
    assert_eq!(code, 0);
    assert!(
        human.starts_with("dependencies: XBase (embedded)\n"),
        "{human}"
    );

    let (single, code) = json(&["policy", "check", &ws, "--policy", &policy, "--single-app"]);
    assert_eq!(code, 0, "{single}");
    assert!(findings(&single).is_empty(), "{single}");
    assert_eq!(
        counts(&single, "dep-customer-insert"),
        [WS_ROUTINES, 0, 0, 1, 4, 0],
        "{single}"
    );
    assert_eq!(single["dependencies"]["mode"], "singleApp", "{single}");
    assert!(app_sources(&single).is_empty(), "{single}");
    let (human, _, _) = alsem(&[
        "policy",
        "check",
        &ws,
        "--policy",
        &policy,
        "--format",
        "human",
        "--single-app",
    ]);
    assert!(!human.starts_with("dependencies:"), "{human}");
}

/// Row 1c, second mutation: the bundled default policy's
/// `no-commit-in-event-subscribers` finds `HandleCommit`, whose `Commit` is in
/// the dependency's `Inner`, reached only through `Outer`; the cone sees it only
/// when the cross-app detector context takes the base's graph, which carries
/// the dependency-internal typed edges.
///   cross-app:  evaluated 5, matched 1 (HandleCommit), passed 1 (HandleInWs), findings 1
///   single-app: evaluated 5, matched 0, passed 2, findings 0 (precondition)
///
/// Mutation: the cross-app model with the single-app detector context
/// (`build_detector_context` without `CrossAppInputs`) -> no commit fact -> 0.
#[test]
fn policy_check_follows_dependency_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, _) = policy_fixture(dir.path(), DIRECT_POLICY);
    let rule = "no-commit-in-event-subscribers";

    let (doc, code) = json(&["policy", "check", &ws]);
    assert_eq!(code, 0, "{doc}");
    let commit: Vec<_> = findings(&doc)
        .into_iter()
        .filter(|(d, _, _)| d == &format!("policy-{rule}"))
        .collect();
    assert_eq!(
        commit,
        vec![(
            format!("policy-{rule}"),
            "ws:src/Extra.al".to_string(),
            decl_line(COMMIT_SUB_AL, "HandleCommit"),
        )],
        "{doc}"
    );
    assert_eq!(counts(&doc, rule), [WS_ROUTINES, 1, 0, 0, 1, 1], "{doc}");
    no_dep_anchor(&doc);

    let (single, code) = json(&["policy", "check", &ws, "--single-app"]);
    assert_eq!(code, 0, "{single}");
    assert_eq!(
        counts(&single, rule),
        [WS_ROUTINES, 0, 0, 0, 2, 0],
        "{single}"
    );
}

/// Row 1c, third part: a `facts: inherited` rule matches the dependency insert
/// `CallChain` reaches only through `Outer` -> `Inner`: exit 0 and exactly one
/// finding, on `CallChain`, cross-app; none single-app.
///   cross-app:  evaluated 5, matched 1, passed 0, findings 1 (the other four
///               routines fail the `routine.name` applicability and are not counted)
///   single-app: evaluated 5, matched 0, passed 1, findings 0
///
/// Mutation: cross-app demand without `RAW_INHERITED_FACTS` -> the run panics
/// (`full_summary.rs:107-116`) -> non-zero exit.
#[test]
fn policy_check_matches_an_inherited_dependency_fact() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, policy) = policy_fixture(dir.path(), INHERITED_POLICY);

    let (doc, code) = json(&["policy", "check", &ws, "--policy", &policy]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        findings(&doc),
        vec![(
            "policy-inherited-insert".to_string(),
            "ws:src/Work.al".to_string(),
            decl_line(WS_AL, "CallChain"),
        )],
        "{doc}"
    );
    assert_eq!(
        counts(&doc, "inherited-insert"),
        [WS_ROUTINES, 1, 0, 0, 0, 1],
        "{doc}"
    );

    let (single, code) = json(&["policy", "check", &ws, "--policy", &policy, "--single-app"]);
    assert_eq!(code, 0, "{single}");
    assert!(findings(&single).is_empty(), "{single}");
    assert_eq!(
        counts(&single, "inherited-insert"),
        [WS_ROUTINES, 0, 0, 0, 1, 0],
        "{single}"
    );
}

// ---------------------------------------------------------------------------
// The no-dependency contract (row 1f)
// ---------------------------------------------------------------------------

/// Remove the top-level `"dependencies": {...}` member (its whole line range,
/// including the trailing comma) from a pretty-printed JSON document.
fn strip_dependencies(text: &str) -> String {
    let Some(key) = text.find("\"dependencies\":") else {
        return text.to_string();
    };
    let line_start = text[..key].rfind('\n').map_or(0, |i| i + 1);
    let open = key + text[key..].find('{').expect("dependencies object");
    let mut depth = 0usize;
    let mut close = open;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    close = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let line_end = text[close..]
        .find('\n')
        .map_or(text.len(), |i| close + i + 1);
    format!("{}{}", &text[..line_start], &text[line_end..])
}

/// Does this fixture's `app.json` declare `platform` (an implicit System +
/// System Application dependency)?
fn declares_platform(ws: &Path) -> bool {
    let text = std::fs::read_to_string(ws.join("app.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    v.get("platform").is_some_and(|p| !p.is_null())
}

/// The cross-app statement's expected `apps`, as `(name, source)`: the two
/// implicit platform dependencies, both missing (no fixture has a package),
/// in `loaded_apps`' name order; or none.
fn expected_cross_apps(ws: &Path) -> Vec<(String, String)> {
    if declares_platform(ws) {
        vec![
            ("System".to_string(), "missing".to_string()),
            ("System Application".to_string(), "missing".to_string()),
        ]
    } else {
        Vec::new()
    }
}

fn app_names(doc: &Value) -> Vec<(String, String)> {
    doc["dependencies"]["apps"]
        .as_array()
        .unwrap_or_else(|| panic!("no top-level dependencies.apps: {doc}"))
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap_or("").to_string(),
                a["source"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// Every argv of `cli_c_events_differential` and `cli_c_policy_differential`,
/// as CLI arguments (without `--format`), each with its workspace.
fn contract_runs() -> Vec<(Vec<String>, std::path::PathBuf, &'static [&'static str])> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let corpus = root.join("r0-corpus");
    let policy = root.join("fixtures/cli-c-policy");
    const EVENTS: &[&str] = &["human", "json"];
    const POLICY: &[&str] = &["human", "json", "sarif"];
    let mut runs = Vec::new();
    let mut push = |sub: &[&str], ws: std::path::PathBuf, extra: &[&str], formats| {
        let mut argv: Vec<String> = sub.iter().map(|s| s.to_string()).collect();
        argv.push(ws.to_string_lossy().to_string());
        argv.extend(extra.iter().map(|s| s.to_string()));
        runs.push((argv, ws, formats));
    };
    for f in [
        "ws-events",
        "ws-event-fanout",
        "ws-event-chains",
        "ws-event-pub-cycle",
        "ws-event-cycle",
        "ws-event-ishandled",
        "ws-event-multi-sub-overlap",
        "ws-event-read-after-write",
        "ws-event-d45-deep",
        "ws-d8-commit-in-tx",
        "ws-txn-d47-event-pos",
        "ws-d12-dead-event",
        "ws-event-partial-coverage",
    ] {
        push(&["events", "fanout"], corpus.join(f), &[], EVENTS);
        push(&["events", "chains"], corpus.join(f), &[], EVENTS);
    }
    push(
        &["events", "fanout"],
        corpus.join("ws-event-fanout"),
        &["--scope", "all"],
        EVENTS,
    );
    push(
        &["events", "chains"],
        corpus.join("ws-event-fanout"),
        &["--scope", "all"],
        EVENTS,
    );
    push(
        &["events", "chains"],
        corpus.join("ws-event-d45-deep"),
        &["--max-depth", "1"],
        EVENTS,
    );
    for p in ["warn", "strict", "ignore"] {
        push(
            &["events", "fanout"],
            corpus.join("ws-event-partial-coverage"),
            &["--coverage-policy", p],
            EVENTS,
        );
    }
    for f in [
        "ws-policy-commit-in-subscriber",
        "ws-policy-commit-in-trigger",
        "ws-policy-api-ui",
        "ws-policy-api-dynamic-dispatch",
        "ws-policy-trigger-http",
        "ws-policy-install-business-write",
        "ws-policy-api-isolated-storage",
        "ws-policy-api-ledger-write",
        "ws-policy-commit-inherited-trigger",
        "ws-policy-custom",
    ] {
        push(&["policy", "check"], policy.join(f), &[], POLICY);
    }
    push(
        &["policy", "check"],
        policy.join("ws-policy-clean"),
        &["--no-policy"],
        &["json"],
    );
    runs
}

/// Row 1f. On every events and policy differential argv (none of those
/// workspaces has a package), cross-app vs `--single-app`: exit codes equal,
/// stderr (analyzer diagnostics) equal, the dependencies statement right PER
/// MODE (single-app `{singleApp, []}` and no human line; cross-app `crossApp`
/// with exactly the missing System / System Application rows when `app.json`
/// declares `platform`, else none, and the human first line present iff `apps`
/// is non-empty), and everything else byte-equal (JSON minus `dependencies`,
/// human minus that first line, SARIF whole).
///
/// Mutations: omit the missing-row append loop (`full.rs:2160-2167`) -> the
/// cross-app `apps` loses its platform rows -> fails. Stated limit: the
/// single-app detector runner for stderr in cross-app mode is NOT caught here
/// (no dependency).
#[test]
fn without_dependencies_events_and_policy_cross_app_equal_single_app() {
    let mut compared = 0;
    for (argv, ws, formats) in contract_runs() {
        let want_apps = expected_cross_apps(&ws);
        for format in formats {
            let mut a: Vec<&str> = argv.iter().map(String::as_str).collect();
            a.extend(["--format", format]);
            let (cross, cross_err, cross_code) = alsem(&a);
            a.push("--single-app");
            let (single, single_err, single_code) = alsem(&a);
            let what = format!("{argv:?} --format {format}");
            assert_eq!(
                cross_code, single_code,
                "{what}: exit codes; single-app stderr: {single_err}"
            );
            assert_eq!(cross_err, single_err, "{what}: stderr");
            match *format {
                "json" => {
                    let c: Value = serde_json::from_str(&cross).expect("cross json");
                    let s: Value = serde_json::from_str(&single).expect("single json");
                    assert_eq!(s["dependencies"]["mode"], "singleApp", "{what}: {single}");
                    assert!(app_names(&s).is_empty(), "{what}: {single}");
                    assert_eq!(c["dependencies"]["mode"], "crossApp", "{what}: {cross}");
                    assert_eq!(app_names(&c), want_apps, "{what}: {cross}");
                    assert_eq!(
                        strip_dependencies(&cross),
                        strip_dependencies(&single),
                        "{what}"
                    );
                }
                "human" => {
                    assert!(!single.starts_with("dependencies:"), "{what}: {single}");
                    let rest = if want_apps.is_empty() {
                        assert!(!cross.starts_with("dependencies:"), "{what}: {cross}");
                        cross.as_str()
                    } else {
                        let line = "dependencies: System (missing), System Application (missing)\n";
                        assert!(cross.starts_with(line), "{what}: {cross}");
                        &cross[line.len()..]
                    };
                    assert_eq!(rest, single, "{what}");
                }
                _ => assert_eq!(cross, single, "{what}"),
            }
            compared += 1;
        }
    }
    // events: 13 fixtures x 2 subcommands x 2 formats + scope-all 2 x 2 +
    // max-depth 1 x 2 + partial-coverage 3 x 2 = 64; policy: 10 x 3 + 1 = 31.
    assert!(compared >= 95, "compared only {compared} outputs");
}

/// Row 1f: a workspace that declares no dependency at all (no `platform`, no
/// `dependencies`) has `apps: []` and no human `dependencies:` line in either
/// mode, for all three commands.
///
/// Mutation: print the human `dependencies:` line with zero apps -> fails (the
/// #66 discrimination).
#[test]
fn without_any_declared_dependency_no_statement_line() {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d12-dead-event");
    assert!(
        !declares_platform(&ws),
        "precondition: no platform declared"
    );
    let ws = ws.to_string_lossy().to_string();
    for sub in [
        &["events", "fanout"][..],
        &["events", "chains"][..],
        &["policy", "check"][..],
    ] {
        for (mode, extra) in [("crossApp", &[][..]), ("singleApp", &["--single-app"][..])] {
            let mut a: Vec<&str> = sub.to_vec();
            a.push(&ws);
            a.extend_from_slice(extra);
            let (doc, code) = json(&a);
            assert_eq!(code, 0, "{a:?}: {doc}");
            assert_eq!(doc["dependencies"]["mode"], mode, "{a:?}: {doc}");
            assert!(app_names(&doc).is_empty(), "{a:?}: {doc}");
            a.extend(["--format", "human"]);
            let (human, _, code) = alsem(&a);
            assert_eq!(code, 0, "{a:?}");
            assert!(!human.starts_with("dependencies:"), "{a:?}: {human}");
        }
    }
}
