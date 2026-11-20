//! ENG-168: `alsem analyze --focus` end to end (spec
//! `docs/superpowers/specs/2026-11-20-eng168-analyze-focus-design.md` §4).
//!
//! No new fixture: `ws-d34` yields d19 (maintainability) x2 then d34 (behaviour)
//! x3, sorted by detector, which is exactly the shape that tells focus-before-limit
//! from limit-before-focus. `ws-d14-dead-routine` yields only d14, so
//! `--focus performance` hides every finding. The partition itself is pinned on
//! hand-stated rows in `engine::gate::focus`'s unit tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::focus::parse_focus;
use al_sem::engine::gate::run::{AnalyzeArgs, AnalyzeOutput, OutputFormat, run_analyze_with_exit};
use serde_json::Value;

fn corpus(fixture: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture)
}

fn args(ws: &Path, format: OutputFormat, focus: Option<&str>) -> AnalyzeArgs {
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format,
        sarif_version_override: Some("gate-sarif-v1".to_string()),
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        with_evidence: false,
        single_app: false,
        focus: focus.map(|f| parse_focus(f).expect("valid focus")),
    }
}

fn run(a: &AnalyzeArgs) -> Result<AnalyzeOutput, String> {
    run_analyze_with_exit(&a.read_inputs(), &a.read_request(), a, "test")
}

fn json(a: &AnalyzeArgs) -> Value {
    let (out, ..) = run(a).expect("analyze runs");
    serde_json::from_str(&out).expect("json")
}

fn detectors(v: &Value) -> Vec<String> {
    v["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["detector"].as_str().unwrap().to_string())
        .collect()
}

/// The precondition the order test rests on: unfocused, ws-d34 lists d19 first.
fn d34_ws() -> PathBuf {
    let ws = corpus("ws-d34");
    let all = detectors(&json(&args(&ws, OutputFormat::Json, None)));
    assert_eq!(
        all.iter()
            .map(|d| d.split('-').next().unwrap())
            .collect::<Vec<_>>(),
        ["d19", "d19", "d34", "d34", "d34"],
        "ws-d34's findings moved; this file's tests rest on them"
    );
    ws
}

/// `--focus behaviour` hides d19 (maintainability), keeps every d34 finding
/// byte-equal to the unfocused run, and counts the hidden ones.
#[test]
fn behaviour_hides_maintainability_and_keeps_shown_findings_byte_equal() {
    let ws = d34_ws();
    let plain = json(&args(&ws, OutputFormat::Json, None));
    let focused = json(&args(&ws, OutputFormat::Json, Some("behaviour")));
    let plain_d34: Vec<&Value> = plain["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == "d34-commit-in-loop")
        .collect();
    let shown: Vec<&Value> = focused["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(shown, plain_d34);
    assert_eq!(
        focused["payload"]["summary"]["hidden"],
        serde_json::json!({
            "focus": ["behaviour"],
            "before": "limit, baseline, suppressions",
            "total": 2,
            "byDetector": [{"detector": "d19-unused-parameter", "concern": "maintainability", "count": 2}],
        })
    );
    // No --focus: no key (the run is byte-identical to before; the goldens hold).
    assert!(plain["payload"]["summary"].get("hidden").is_none());

    let both = json(&args(
        &ws,
        OutputFormat::Json,
        Some("behaviour,maintainability"),
    ));
    assert_eq!(detectors(&both).len(), 5);
    assert_eq!(both["payload"]["summary"]["hidden"]["total"], 0);
}

/// `--limit 1` after focus: the slot goes to a shown d34, never to a hidden d19.
///
/// Mutation (2026-11-20): moved the focus partition after `--limit` in
/// `analyze_view` -> the limit kept a d19 that focus then hid: the run showed
/// `[]`, not `["d34-commit-in-loop"]` (red); restored, green.
#[test]
fn focus_applies_before_limit() {
    let ws = d34_ws();
    let mut a = args(&ws, OutputFormat::Json, Some("behaviour"));
    a.limit = Some(1);
    let v = json(&a);
    assert_eq!(detectors(&v), ["d34-commit-in-loop"]);
    assert_eq!(v["payload"]["summary"]["hidden"]["total"], 2);
}

/// The hidden counts are taken before the baseline: the two d19 findings, which
/// the baseline lists, are still counted (the stated definition), while the d34
/// findings it does not list are shown with their evidence, under schema 2.0.0.
///
/// Mutation (2026-11-20): moved the focus partition below the baseline block in
/// `analyze_view` -> the baseline dropped both d19 first and `hidden.total` read
/// 0, not 2 (red); restored, green.
#[test]
fn hidden_counts_are_taken_before_the_baseline() {
    let ws = d34_ws();
    let plain = json(&args(&ws, OutputFormat::Json, None));
    let d19: Vec<&str> = plain["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == "d19-unused-parameter")
        .map(|f| f["fingerprint"].as_str().unwrap())
        .collect();
    assert_eq!(d19.len(), 2, "precondition: two d19 fingerprints");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("baseline.json");
    std::fs::write(
        &base,
        serde_json::json!({"schemaVersion": "1", "generatedAt": "1970-01-01T00:00:00Z",
            "fingerprints": d19})
        .to_string(),
    )
    .unwrap();

    let mut a = args(&ws, OutputFormat::Json, Some("behaviour"));
    a.baseline = Some(base.to_string_lossy().to_string());
    a.with_evidence = true;
    let v = json(&a);
    assert_eq!(v["schemaVersion"], "2.0.0");
    let shown = v["payload"]["findings"].as_array().unwrap();
    assert_eq!(shown.len(), 3, "{shown:#?}");
    for f in shown {
        assert_eq!(f["detector"], "d34-commit-in-loop");
        assert!(
            f["evidencePath"].as_array().is_some_and(|p| !p.is_empty()),
            "d34 carries its evidence: {f:#}"
        );
    }
    assert_eq!(v["payload"]["summary"]["hidden"]["total"], 2);
}

/// The hidden counts are taken after `--min-severity`: at `critical` the two
/// `info` d19 findings are gone before focus sees them, so nothing is counted.
///
/// Mutation (2026-11-20): moved the focus partition above `filter_findings` in
/// `analyze_view` -> `hidden.total` read 2, not 0 (red); restored, green.
#[test]
fn hidden_counts_are_taken_after_min_severity() {
    let ws = d34_ws();
    let mut a = args(&ws, OutputFormat::Json, Some("behaviour"));
    a.min_severity = Some("critical".to_string());
    let v = json(&a);
    assert_eq!(detectors(&v), ["d34-commit-in-loop"]);
    assert_eq!(v["payload"]["summary"]["hidden"]["total"], 0);
    assert_eq!(
        v["payload"]["summary"]["hidden"]["byDetector"],
        serde_json::json!([])
    );
}

/// The hidden d35 count of #265 A6d's workspace (a subscriber in a SOURCE
/// dependency) at `scope`, under `--focus performance`.
fn hidden_d35(scope: Scope) -> u64 {
    let (_tmp, ws) = crate::cli_d35_unknown_commit::dependency_subscriber_ws();
    let mut a = args(&ws, OutputFormat::Json, Some("performance"));
    a.scope = scope;
    let v = json(&a);
    v["payload"]["summary"]["hidden"]["byDetector"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["detector"] == "d35-commit-in-event-subscriber")
        .map_or(0, |r| r["count"].as_u64().unwrap())
}

/// The hidden counts follow `--scope`: the dependency's d35 finding is counted
/// under `--scope all` and not under `--scope primary`.
///
/// Mutation (2026-11-20): moved the focus partition above the scope block in
/// `analyze_view` -> still green. The cross-app run already drops
/// dependency-anchored findings for `--scope primary` itself, before
/// `analyze_view`'s scope filter, so no existing workspace has a finding that
/// only that filter removes. This test pins the outcome (what `--scope primary`
/// counts), not the partition's place relative to the scope block.
#[test]
fn hidden_counts_follow_scope() {
    let all = hidden_d35(Scope::All);
    let primary = hidden_d35(Scope::Primary);
    assert_eq!(all, primary + 1, "all {all}, primary {primary}");
}

/// The refused combinations on the direct entry (`AnalyzeJob::new`): an error,
/// no baseline bytes, before any analysis.
///
/// Mutation (2026-11-20): ignored the `check_focus_conflicts` result in
/// `AnalyzeJob::new` (`let _ = ...;`) -> `run` returned `Ok` with a JSON report,
/// "must refuse --update-baseline --focus" (red); restored, green.
#[test]
fn library_entry_refuses_update_baseline_and_fail_on() {
    let ws = corpus("ws-d34");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("baseline.json");
    let mut a = args(&ws, OutputFormat::Json, Some("behaviour"));
    a.baseline = Some(base.to_string_lossy().to_string());
    a.update_baseline = true;
    let e = run(&a).expect_err("must refuse --update-baseline --focus");
    assert!(
        e.contains("--update-baseline cannot be combined with --focus"),
        "{e}"
    );
    assert!(
        !e.starts_with("analysis failure"),
        "a config error (exit 3): {e}"
    );

    let mut a = args(&ws, OutputFormat::Json, Some("behaviour"));
    a.fail_on = Some("high".to_string());
    let e = run(&a).expect_err("must refuse --fail-on --focus");
    assert!(
        e.contains("--fail-on cannot be combined with --focus"),
        "{e}"
    );
}

fn alsem(argv: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(argv)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// The CLI path (`plan_analyze`): exit 3 and nothing written; an unknown value
/// names the known ones; `behavior` is accepted.
///
/// Mutation (2026-11-20): `plan_analyze`'s `check_focus_conflicts` refusal
/// disabled (`... && false`) -> the `--update-baseline` run exited 101 (the job's
/// `expect` in `plan_analyze`'s job closure panicked on the focus error), not 3
/// (red); restored, green.
#[test]
fn cli_refuses_and_validates_focus() {
    let ws = corpus("ws-d34");
    let ws = ws.to_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("baseline.json");
    let base_s = base.to_str().unwrap();
    // A baseline that exists before the run must come out byte-identical.
    let before = b"{\"schemaVersion\":\"1\",\"fingerprints\":[\"pre-existing\"]}";
    std::fs::write(&base, before).unwrap();

    let (out, err, code) = alsem(&[
        "analyze",
        ws,
        "--focus",
        "behaviour",
        "--baseline",
        base_s,
        "--update-baseline",
    ]);
    assert_eq!(code, 3, "{err}");
    assert!(
        err.contains("--update-baseline cannot be combined with --focus"),
        "{err}"
    );
    assert_eq!(out, "", "no report");
    assert_eq!(std::fs::read(&base).unwrap(), before, "baseline untouched");

    let (out, err, code) = alsem(&["analyze", ws, "--focus", "behaviour", "--fail-on", "high"]);
    assert_eq!(code, 3, "{err}");
    assert!(
        err.contains("--fail-on cannot be combined with --focus"),
        "{err}"
    );
    assert_eq!(out, "", "no report");

    let (out, err, code) = alsem(&["analyze", ws, "--focus", "speed"]);
    assert_eq!(code, 3, "{err}");
    assert_eq!(out, "", "no report");
    assert!(
        err.contains("'speed'")
            && err.contains("performance, behaviour (or behavior), maintainability"),
        "{err}"
    );

    let (out, err, code) = alsem(&[
        "analyze",
        ws,
        "--focus",
        "behavior",
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).expect("json");
    assert_eq!(
        v["payload"]["summary"]["hidden"]["focus"],
        serde_json::json!(["behaviour"])
    );
}

/// Every text format states the line, also when focus hides every finding
/// (ws-d14-dead-routine has only d14).
#[test]
fn every_format_states_what_was_hidden_when_nothing_is_shown() {
    let ws = corpus("ws-d14-dead-routine");
    let line = "hidden by --focus performance (before limit, baseline and suppressions): \
                1 finding (maintainability: d14 1)";

    let v = json(&args(&ws, OutputFormat::Json, Some("performance")));
    assert!(detectors(&v).is_empty());
    assert_eq!(v["payload"]["summary"]["hidden"]["total"], 1);

    // Terminal: the line directly under "No findings in this view.", never the
    // bare "No findings." sentence.
    let (out, ..) = run(&args(&ws, OutputFormat::Terminal, Some("performance"))).unwrap();
    assert!(
        out.contains(&format!("\n\nNo findings in this view.\n{line}")),
        "{out}"
    );
    assert!(!out.contains("No findings. (Absence"), "{out}");

    let mut a = args(&ws, OutputFormat::Terminal, Some("performance"));
    a.group_by = Some("detector".to_string());
    let (out, ..) = run(&a).unwrap();
    assert!(
        out.contains(&format!(
            "Grouped by detector (top 0):\nNo findings in this view.\n{line}"
        )),
        "{out}"
    );

    let (out, ..) = run(&args(&ws, OutputFormat::PrSummary, Some("performance"))).unwrap();
    assert!(
        out.starts_with("### Transaction integrity — No findings in this view."),
        "{out}"
    );
    assert!(out.contains(&format!("_{line}_")), "{out}");

    let (out, ..) = run(&args(&ws, OutputFormat::Html, Some("performance"))).unwrap();
    assert!(
        out.contains(&format!("<div class=\"coverage\">{line}</div>")),
        "{out}"
    );
    assert!(
        out.contains(r#"<p class="empty">No findings in this view.</p>"#),
        "{out}"
    );

    // The shown case: pr-summary carries it under its heading as well.
    let (out, ..) = run(&args(&ws, OutputFormat::PrSummary, Some("maintainability"))).unwrap();
    assert!(
        out.contains("hidden by --focus maintainability (before limit, baseline and suppressions): 0 findings"),
        "{out}"
    );
}

/// SARIF: the same object in `runs[0].properties.hidden` and the partial-report
/// warning; on a degraded workspace both warnings, preflight first.
#[test]
fn sarif_states_hidden_and_warns_beside_the_degraded_warning() {
    let ws = d34_ws();
    let (out, _, warnings, _) = run(&args(&ws, OutputFormat::Sarif, Some("behaviour"))).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    let json_hidden = json(&args(&ws, OutputFormat::Json, Some("behaviour")))["payload"]["summary"]
        ["hidden"]
        .clone();
    assert_eq!(v["runs"][0]["properties"]["hidden"], json_hidden);
    let rules: Vec<&str> = v["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ruleId"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["d34-commit-in-loop"; 3]);
    assert_eq!(
        warnings.last().map(String::as_str),
        Some("--focus: a partial report (2 findings hidden); do not upload it as the full scan")
    );
    let (out, _, warnings, _) = run(&args(&ws, OutputFormat::Sarif, None)).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["runs"][0].get("properties").is_none());
    assert!(warnings.is_empty(), "{warnings:?}");

    let degraded = corpus("ws-baseapp-closure");
    let (_, _, warnings, _) =
        run(&args(&degraded, OutputFormat::Sarif, Some("behaviour"))).unwrap();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("symbol-only dependency app"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].starts_with("--focus: a partial report"),
        "{warnings:?}"
    );
}

/// The bin prints EVERY warning, in order: on a degraded workspace a focused
/// SARIF run's stderr carries the preflight warning, then the partial-report one.
///
/// Mutation (2026-11-20): `AnalyzeJob::serialize` printed only the first warning
/// (`for msg in stderr_warning.into_iter().take(1)`) -> one `al-sem: warning:`
/// line, not 2 (red); restored, green.
#[test]
fn the_bin_prints_both_warnings_in_order() {
    let ws = corpus("ws-baseapp-closure");
    let (out, err, code) = alsem(&[
        "analyze",
        ws.to_str().unwrap(),
        "--format",
        "sarif",
        "--focus",
        "behaviour",
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\"hidden\""), "{out}");
    let warnings: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("al-sem: warning: "))
        .collect();
    assert_eq!(warnings.len(), 2, "{err}");
    assert!(warnings[0].contains("symbol-only dependency app"), "{err}");
    assert!(
        warnings[1].starts_with("al-sem: warning: --focus: a partial report ("),
        "{err}"
    );
}

/// A refused batch job must not empty, through its own stdout, the baseline its
/// command names: the run refuses the manifest (exit 3) and B is untouched.
///
/// Mutation (2026-11-20): the refused job's `kept` list emptied in the bin's
/// batch planning (`kept: vec![]`) -> the run was not refused: it exited 0, not
/// 3 (red); restored, green. Mutation (2026-11-20): `check_write_sets`' own-stream
/// test flipped (`writes[w].0 != r`) -> this test exited 0, not 3, and
/// `cli_batch_writes::batch_failed_job_writes_only_its_streams` exited 3, not 0
/// (red); restored, green.
#[test]
fn a_refused_batch_job_keeps_the_baseline_it_names() {
    use crate::cli_batch_writes::{job, run_manifest};
    let ws = corpus("ws-d34");
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.json");
    let before = b"{\"schemaVersion\":\"1\",\"fingerprints\":[\"pre-existing\"]}";
    std::fs::write(&base, before).unwrap();
    let b = base.to_str().unwrap();
    let (out, _) = run_manifest(
        dir.path(),
        &[job(
            &[
                "analyze",
                ws.to_str().unwrap(),
                "--focus",
                "behaviour",
                "--baseline",
                b,
                "--update-baseline",
            ],
            Some(&base),
            None,
        )],
        None,
        &[],
        None,
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("is refused"), "{err}");
    assert_eq!(std::fs::read(&base).unwrap(), before, "B untouched");
}

/// A preset composes with focus: `transaction-integrity --focus behaviour`
/// hides its member d48 (performance) and counts it.
#[test]
fn preset_member_of_another_concern_is_hidden_and_counted() {
    let ws = corpus("ws-txn-d48-pos");
    let mut a = args(&ws, OutputFormat::Json, None);
    a.preset = Some("transaction-integrity".to_string());
    let plain = json(&a);
    let d48 = detectors(&plain)
        .iter()
        .filter(|d| *d == "d48-io-in-loop")
        .count();
    assert!(d48 > 0, "precondition: the fixture yields d48");

    a.focus = Some(parse_focus("behaviour").unwrap());
    let v = json(&a);
    assert!(!detectors(&v).iter().any(|d| d == "d48-io-in-loop"));
    let rows = v["payload"]["summary"]["hidden"]["byDetector"]
        .as_array()
        .unwrap();
    assert!(
        rows.iter().any(|r| r["detector"] == "d48-io-in-loop"
            && r["concern"] == "performance"
            && r["count"] == d48),
        "{rows:?}"
    );
}

/// No model (fail-closed workspace): every format says "focus requested,
/// analysis unavailable", never a count.
///
/// Mutation (2026-11-20): `empty_output_result`'s focus statement forced to `None`
/// (`.filter(|_| false)`) -> the JSON's `summary.hidden` was `Null` (red);
/// restored, green.
#[test]
fn no_model_says_analysis_unavailable_in_every_format() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"name":"Fail Closed","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("Foo.al"), "codeunit 50100 Foo { }").unwrap();
    let ws = dir.path();
    let line = "--focus behaviour: focus requested, analysis unavailable";

    let v = json(&args(ws, OutputFormat::Json, Some("behaviour")));
    assert_eq!(
        v["payload"]["summary"]["hidden"],
        serde_json::json!({"focus": ["behaviour"], "status": "focus requested, analysis unavailable"})
    );
    let (out, _, warnings, _) = run(&args(ws, OutputFormat::Sarif, Some("behaviour"))).unwrap();
    let s: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        s["runs"][0]["properties"]["hidden"]["status"],
        "focus requested, analysis unavailable"
    );
    assert!(
        s["runs"][0]["properties"]["hidden"].get("total").is_none(),
        "nothing was counted: {s:#}"
    );
    assert_eq!(
        warnings.len(),
        2,
        "could-not-verify, then the focus warning"
    );
    assert_eq!(
        warnings[1],
        "--focus: a partial report (focus requested, analysis unavailable); do not upload \
         it as the full scan"
    );
    for format in [
        OutputFormat::Terminal,
        OutputFormat::PrSummary,
        OutputFormat::Html,
    ] {
        let (out, ..) = run(&args(ws, format, Some("behaviour"))).unwrap();
        assert!(out.contains(line), "{format:?}: {out}");
    }
}
