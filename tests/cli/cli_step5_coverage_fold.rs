//! #160 step 5 (spec `docs/superpowers/specs/2026-10-22-issue-160-step5-coverage-policy-design.md`
//! §5): the CLI readers of the detector context's coverage, each pinned through
//! its production command: T1b (digest and prove `transactionContext`), T2
//! (`policy check`, both `onUnknown` outcomes), T6 (`events fanout`, all three
//! `--coverage-policy` modes), T7 (d35) and T8 (d45, opt-in). The lib-level
//! tests (T1, T3-T5, T9) are in `src/engine/l5/detector_context_step5_tests.rs`.
//!
//! One inline workspace (fixture A plus an event), no r0-corpus fixture, so no
//! golden family. Every case spawns `CARGO_BIN_EXE_alsem`, single-app (the
//! workspace has no dependency).
//!
//! Fixture A: `Helper` makes the ambiguous `O(L)` call and one `Modify` on a
//! physical table, no Commit; `Root` calls `Helper`, then `Commit()`; `Entry`
//! calls `Root`. The event part: `S5 Pub` publishes `OnS5` (raised by `Raise`);
//! its subscriber `HandleS5` calls `Helper` and commits nothing. NOT compilable
//! AL (a Codeunit is neither a `Text` nor an `S5 ILog`): our overload resolver
//! is lenient, and the lib test T1 asserts the ambiguous edge exists.
//!
//! The one break for every test: delete the fold call in `det_cone_inputs` ->
//! `Helper` is `complete` in the context and every reader below answers as if
//! the ambiguous call were resolved.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const APP_JSON: &str = r#"{"id":"55555555-0000-0000-0000-000000160005","name":"Step5","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}],"dependencies":[]}"#;

const FIXTURE: &str = r#"table 50100 "S5 Order"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

interface "S5 ILog"
{
    procedure Touch();
}

codeunit 50100 "S5 Target"
{
    procedure O(X: Interface "S5 ILog"): Integer
    begin
        exit(0);
    end;

    procedure O(X: Text): Integer
    begin
        exit(1);
    end;
}

codeunit 50101 "S5 Work"
{
    procedure Helper()
    var
        L: Codeunit "S5 Target";
        T: Codeunit "S5 Target";
        R: Record "S5 Order";
    begin
        T.O(L);
        R.Modify();
    end;

    procedure Root()
    begin
        Helper();
        Commit();
    end;

    procedure Entry()
    begin
        Root();
    end;
}

codeunit 50102 "S5 Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnS5()
    begin
    end;

    procedure Raise()
    begin
        OnS5();
    end;
}

codeunit 50103 "S5 Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"S5 Pub", 'OnS5', '', false, false)]
    local procedure HandleS5()
    var
        W: Codeunit "S5 Work";
    begin
        W.Helper();
    end;
}
"#;

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).expect("mkdir src");
    std::fs::write(ws.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(ws.join("src/S5.al"), FIXTURE).expect("write S5.al");
    (dir, ws)
}

/// `alsem <args> --format json --deterministic` -> (parsed stdout, stderr, exit).
fn alsem_json(args: &[&str], ws: &Path) -> (Value, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args.iter().map(|a| {
            if *a == "<ws>" {
                ws.as_os_str()
            } else {
                std::ffi::OsStr::new(a)
            }
        }))
        .args(["--format", "json", "--deterministic"])
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
    let doc = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{e}: {args:?} exit {code}: stdout {stdout:?} stderr {stderr}"));
    (doc, stderr, code)
}

/// The `transactionContext` of every COMMIT effect in `effects`.
fn commit_contexts(effects: &Value) -> Vec<String> {
    effects
        .as_array()
        .unwrap_or_else(|| panic!("effects array: {effects}"))
        .iter()
        .filter(|e| e["type"] == "COMMIT")
        .map(|e| {
            e["transactionContext"]
                .as_str()
                .unwrap_or("<none>")
                .to_string()
        })
        .collect()
}

/// T1b (spec §5): digest and prove read the context's transaction map. `Root`'s
/// Commit closes a span that reaches `Helper`'s ambiguous call, so the COMMIT
/// effect's `transactionContext` is `unknown` in digest's output and in prove's
/// `evidence`. digest's `coverage` key for `Root` is the snapshot's, which
/// already folds: `partial` before and after the break. Break -> both
/// `transactionContext`s are `span-has-known-writes`; the `coverage` key stays.
#[test]
fn step5_t1b_digest_and_prove_transaction_context() {
    let (_dir, ws) = workspace();
    let (digest, err, code) = alsem_json(
        &["digest", "<ws>", "--routine", "Root", "--single-app"],
        &ws,
    );
    assert_eq!(code, 0, "digest exit: {err}");
    let entry = &digest["payload"]["entries"][0];
    assert_eq!(
        entry["routine"]["display"], "Root",
        "the digest entry is Root: {digest}"
    );
    assert_eq!(
        entry["coverage"]["status"], "partial",
        "digest's coverage key is the snapshot's (already folded): {}",
        entry["coverage"]
    );
    assert_eq!(
        commit_contexts(&entry["effects"]),
        vec!["unknown"],
        "digest: the COMMIT effect's transactionContext"
    );
    let (prove, err, code) = alsem_json(&["prove", "<ws>", "Root", "may-commit"], &ws);
    assert_eq!(code, 0, "prove exit: {err}");
    assert_eq!(
        prove["payload"]["result"]["answer"], "yes",
        "prove answer: {prove}"
    );
    assert_eq!(
        commit_contexts(&prove["payload"]["result"]["evidence"]),
        vec!["unknown"],
        "prove: the COMMIT evidence's transactionContext"
    );
}

/// T2's policy: two rules scoped to `Helper` by `routine.name`, both
/// `requireCoverage: complete`; A fail-closed, B fail-open.
const POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: helper-modify-closed
    title: "Modify in Helper (fail-closed)"
    severity: high
    when:
      routine.name: "Helper"
      capability.op: modify
    requireCoverage: complete
    onUnknown: fail-closed
  - id: helper-modify-open
    title: "Modify in Helper (fail-open)"
    severity: high
    when:
      routine.name: "Helper"
      capability.op: modify
    requireCoverage: complete
    onUnknown: fail-open
"#;

/// `(routinesMatched, routinesSkippedCoverage)` of `rule`.
fn counters(doc: &Value, rule: &str) -> (Option<u64>, Option<u64>) {
    let s = doc["ruleSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ruleSummaries: {doc}"))
        .iter()
        .find(|s| s["ruleId"] == rule)
        .unwrap_or_else(|| panic!("no summary for {rule}: {doc}"));
    (
        s["routinesMatched"].as_u64(),
        s["routinesSkippedCoverage"].as_u64(),
    )
}

/// T2 (spec §5): `policy check` (the production pipeline, its RAW build
/// included) reads the context's coverage. `Helper` is `partial`, so both
/// rules' `complete` gate fails it: A (fail-closed) emits exactly one coverage
/// finding on `Helper`, B (fail-open) emits nothing; each counts `Helper` as
/// skipped by coverage and matches nothing (every other routine is skipped by
/// `routine.name` before the gate). Break -> each rule emits its own finding
/// on `Helper`, skipped 0, matched 1.
#[test]
fn step5_t2_policy_coverage_gate_both_on_unknown_modes() {
    let (dir, ws) = workspace();
    let policy = dir.path().join("policy.yaml");
    std::fs::write(&policy, POLICY).expect("write policy");
    let policy = policy.to_string_lossy().to_string();
    let (doc, err, code) = alsem_json(
        &[
            "policy",
            "check",
            "<ws>",
            "--policy",
            &policy,
            "--single-app",
        ],
        &ws,
    );
    assert_eq!(code, 0, "policy check exit: {err}");
    assert_eq!(
        counters(&doc, "helper-modify-closed"),
        (Some(0), Some(1)),
        "A (fail-closed): (matched, skippedCoverage): {doc}"
    );
    assert_eq!(
        counters(&doc, "helper-modify-open"),
        (Some(0), Some(1)),
        "B (fail-open): (matched, skippedCoverage): {doc}"
    );
    let findings: Vec<(String, String, String)> = doc["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("findings: {doc}"))
        .iter()
        .map(|f| {
            (
                f["detector"].as_str().unwrap_or_default().to_string(),
                f["rootCause"].as_str().unwrap_or_default().to_string(),
                f["evidencePath"][0]["note"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        findings,
        vec![(
            "policy-helper-modify-closed".to_string(),
            "Coverage gate (requireCoverage=complete) failed: routine coverage is partial."
                .to_string(),
            "coverage=partial".to_string(),
        )],
        "only A's coverage finding, none from B"
    );
    let at = doc["findings"][0]["primaryLocation"]["range"]["startLine"].as_u64();
    let helper_line = FIXTURE
        .lines()
        .position(|l| l.contains("procedure Helper()"))
        .map(|i| i as u64);
    assert_eq!(at, helper_line, "A's finding is on Helper");
}

/// The fanout entry of `OnS5` (its `coverage` object), if present.
fn ons5_coverage(doc: &Value) -> Option<Value> {
    doc["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries: {doc}"))
        .iter()
        .find(|e| {
            e["eventId"]
                .as_str()
                .is_some_and(|id| id.ends_with("/event/ons5"))
        })
        .map(|e| e["coverage"].clone())
}

/// T6 (spec §5, E4): `events fanout` reads the context's inherited status.
/// `HandleS5` reaches `Helper`, so `OnS5`'s `capabilityComposition` is
/// `partial` under the default `warn` (exit 0); `--coverage-policy strict`
/// drops the entry, names it on stderr and exits 1; `ignore` keeps it as
/// `complete`. Break -> `complete` under `warn`, and `strict` keeps the entry
/// with exit 0.
#[test]
fn step5_t6_events_fanout_coverage_policy_modes() {
    let (_dir, ws) = workspace();
    let (warn, err, code) = alsem_json(&["events", "fanout", "<ws>", "--single-app"], &ws);
    assert_eq!(code, 0, "warn exit: {err}");
    assert_eq!(
        ons5_coverage(&warn).map(|c| c["capabilityComposition"].clone()),
        Some(Value::from("partial")),
        "warn: OnS5's capabilityComposition: {warn}"
    );
    let (strict, err, code) = alsem_json(
        &[
            "events",
            "fanout",
            "<ws>",
            "--single-app",
            "--coverage-policy",
            "strict",
        ],
        &ws,
    );
    assert_eq!(
        ons5_coverage(&strict),
        None,
        "strict: the partial entry is dropped: {strict}"
    );
    assert!(
        err.lines()
            .any(|l| l.starts_with("coverage-incomplete:") && l.contains("/event/ons5")),
        "strict: stderr names the dropped event: {err}"
    );
    assert_eq!(code, 1, "strict exit");
    let (ignore, err, code) = alsem_json(
        &[
            "events",
            "fanout",
            "<ws>",
            "--single-app",
            "--coverage-policy",
            "ignore",
        ],
        &ws,
    );
    assert_eq!(code, 0, "ignore exit: {err}");
    assert_eq!(
        ons5_coverage(&ignore).map(|c| c["capabilityComposition"].clone()),
        Some(Value::from("complete")),
        "ignore: the entry is kept, rewritten to complete: {ignore}"
    );
}

/// The findings of `detector` in an analyze report.
fn findings_of(report: &Value, detector: &str) -> Vec<Value> {
    report["payload"]["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("payload.findings: {report}"))
        .iter()
        .filter(|f| f["detector"] == detector)
        .cloned()
        .collect()
}

/// T7 (spec §5, E4): d35 reads the fold through `may_commit_derived`.
/// `HandleS5` commits nothing but reaches `Helper`'s ambiguous call, so its
/// `mayCommit` is UNKNOWN: one d35 finding, in the UNKNOWN wording, whose
/// reasons name `ambiguous-overload`. Break -> `mayCommit` is NO: no finding.
#[test]
fn step5_t7_d35_unknown_commit_from_the_fold() {
    let (_dir, ws) = workspace();
    let (report, err, code) = alsem_json(&["analyze", "<ws>", "--single-app"], &ws);
    assert_eq!(code, 0, "analyze exit: {err}");
    let d35 = findings_of(&report, "d35-commit-in-event-subscriber");
    assert_eq!(d35.len(), 1, "one d35 finding: {d35:#?}");
    assert_eq!(
        d35[0]["title"], "Commit may be reachable from event subscriber (incomplete call coverage)",
        "the UNKNOWN wording"
    );
    let root_cause = d35[0]["rootCause"].as_str().unwrap_or_default();
    assert!(
        root_cause.starts_with("HandleS5 is an event subscriber whose call coverage is incomplete")
            && root_cause.contains("ambiguous-overload"),
        "the reasons name ambiguous-overload: {root_cause}"
    );
}

/// T8 (spec §5, E4): d45 (opt-in, so `--detector`) reads the subscribers'
/// reachable coverage. `HandleS5` writes `S5 Order` through `Helper` and is
/// `partial`, so the finding's root cause carries `subscriberCoverage=partial`.
/// Break -> `subscriberCoverage=complete`.
#[test]
fn step5_t8_d45_subscriber_coverage_from_the_fold() {
    let (_dir, ws) = workspace();
    let (report, err, code) = alsem_json(
        &[
            "analyze",
            "<ws>",
            "--single-app",
            "--detector",
            "d45-event-transitive-table-exposure",
        ],
        &ws,
    );
    assert_eq!(code, 0, "analyze exit: {err}");
    let d45 = findings_of(&report, "d45-event-transitive-table-exposure");
    assert_eq!(d45.len(), 1, "one d45 finding: {d45:#?}");
    let root_cause = d45[0]["rootCause"].as_str().unwrap_or_default();
    assert!(
        root_cause.contains("dispatches to 1 subscriber(s) that write table")
            && root_cause.ends_with("subscriberCoverage=partial"),
        "the subscriber coverage is partial: {root_cause}"
    );
}
