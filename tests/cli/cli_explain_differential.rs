//! `alsem explain` — the evidence-card CLI family (intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.4, step 4).
//!
//! Every case spawns the SHIPPED BINARY (`CARGO_BIN_EXE_alsem`), as
//! `cli_query_differential` does: a library-level test would leave the clap
//! arm, the flag validation, the `ViewNeeds::EXPLAIN` demand and the exit-code
//! plumbing deletable while it stayed green.
//!
//! Goldens: `tests/cli-explain-goldens/<slug>.{json,human.txt}`, Rust-owned.
//! Refresh with `REGEN_TEMP_GOLDENS=1 cargo test --test cli
//! cli_explain_differential::` (or `scripts/check-goldens --regen`), then
//! INSPECT the diff. `ws-explain.changed.diff` in the same folder is an input,
//! written by hand, not a golden.
//!
//! What the cases pin:
//! - `--routine` on a routine with rules (`ws-rules` `CheckOrder`: labels, a
//!   `TextConst`, earlier-check `not C`, a global label) and on one with
//!   rules, a write, an event with an `IsHandled` subscriber, a page action
//!   and a test (`ws-explain` `PostOrder`);
//! - `--routine` on a routine with NO rules (`Archive`): `"own": []` is
//!   present, never left out (spec Done 5);
//! - a name no routine has, and a name two routines share: exit 2 with
//!   prove's `routine-not-found` / `routine-ambiguous` wording;
//! - `--diff`: one card per changed routine, the hunk outside any routine
//!   reported as digest reports it;
//! - `--table/--field`: the field's writers (assignment and `Validate`) with
//!   one card each, and an unknown field (exit 2). `ReopenAll`'s `ModifyAll`
//!   of the same field is NOT listed: `field_writers`' stated limit (step 6);
//! - gaps: a test procedure's calls into a symbol-only `Assert`.
//! - rules across calls (step 5, fixture `ws-explain-calls`): inherited
//!   rules through one and two calls, a matching literal (atom dropped), a
//!   contradicting one (rule dropped), a forwarded parameter (re-anchored), a
//!   callee parameter assigned before its check (not substituted), a callee
//!   called only in a branch and one called after the first write (neither
//!   inherited), a `MayNotStop` callee rule (listed, never applied), a
//!   recursive pair (the walk stops), caller rules with one skipping caller,
//!   table-trigger rules run by `Validate` / `Modify(true)` and skipped by a
//!   plain assignment and `Modify()`, an event subscriber's rule (under its
//!   extension point only), an error collector, and a page action reaching
//!   the routine through `RunObject`.
//!
//! The semantic tests below pin what bytes cannot: the `--diff` routine set
//! equals `digest --diff`'s (the reuse, not a copy), every list is present on
//! every card, and a batch job equals its standalone run.

use std::path::PathBuf;
use std::process::Command;

use crate::regen;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("cli-explain-goldens")
}

fn diff_input() -> String {
    goldens_dir()
        .join("ws-explain.changed.diff")
        .to_string_lossy()
        .into_owned()
}

fn regen_golden(golden_path: &std::path::Path, got: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    if let Some(parent) = golden_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", parent.display()));
    }
    std::fs::write(golden_path, got)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    true
}

fn strip_cr(s: &str) -> String {
    s.replace('\r', "")
}

/// `alsem <args...>`: `(stdout, stderr, exit code)`.
fn run_alsem(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// One golden case: a slug, the fixture, the flags after the workspace, and
/// the exit code both formats must produce.
struct Case {
    slug: &'static str,
    workspace: &'static str,
    args: Vec<String>,
    exit_code: i32,
}

fn case(slug: &'static str, workspace: &'static str, args: &[&str], exit_code: i32) -> Case {
    Case {
        slug,
        workspace,
        args: args.iter().map(|s| s.to_string()).collect(),
        exit_code,
    }
}

fn explain_argv(c: &Case) -> Vec<String> {
    let ws = fixtures_dir().join(c.workspace);
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());
    let mut argv = vec!["explain".to_string(), ws.to_string_lossy().into_owned()];
    argv.extend(c.args.iter().cloned());
    argv
}

fn check_case(c: &Case) {
    let argv = explain_argv(c);
    for (format, ext) in [("json", "json"), ("human", "human.txt")] {
        let mut args: Vec<&str> = argv.iter().map(String::as_str).collect();
        args.extend(["--format", format]);
        let (got, stderr, code) = run_alsem(&args);
        assert_eq!(
            code, c.exit_code,
            "[{}/{format}] exit code — stdout:\n{got}\nstderr:\n{stderr}",
            c.slug
        );
        let golden = goldens_dir().join(format!("{}.{ext}", c.slug));
        if regen_golden(&golden, &got) {
            continue;
        }
        let expected = std::fs::read_to_string(&golden).unwrap_or_else(|e| {
            panic!(
                "[{}/{format}] read golden {} failed: {e} — run \
                 REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_explain_differential:: \
                 to (re)capture",
                c.slug,
                golden.display()
            )
        });
        assert_eq!(
            strip_cr(&got),
            strip_cr(&expected),
            "[{}/{format}] diverged from {}",
            c.slug,
            golden.display()
        );
    }
}

fn json_of(c: &Case) -> serde_json::Value {
    let argv = explain_argv(c);
    let mut args: Vec<&str> = argv.iter().map(String::as_str).collect();
    args.extend(["--format", "json"]);
    let (got, stderr, _) = run_alsem(&args);
    serde_json::from_str(&got).unwrap_or_else(|e| panic!("{}: not JSON ({e}): {stderr}", c.slug))
}

fn rules_check_order() -> Case {
    case(
        "rules.checkorder",
        "ws-rules",
        &["--routine", "CheckOrder"],
        0,
    )
}
fn explain_post_order() -> Case {
    case(
        "explain.postorder",
        "ws-explain",
        &["--routine", "PostOrder"],
        0,
    )
}
fn explain_archive() -> Case {
    case(
        "explain.archive",
        "ws-explain",
        &["--routine", "Archive"],
        0,
    )
}
fn explain_diff() -> Case {
    let d = diff_input();
    case("explain.diff", "ws-explain", &["--diff", &d], 0)
}
fn explain_field() -> Case {
    case(
        "explain.field-status",
        "ws-explain",
        &["--table", "Explain Order", "--field", "Status"],
        0,
    )
}

#[test]
fn explain_routine_with_rules() {
    check_case(&rules_check_order());
}

#[test]
fn explain_routine_with_rules_effects_action_event_and_test() {
    check_case(&explain_post_order());
}

#[test]
fn explain_routine_without_rules_lists_own_empty() {
    check_case(&explain_archive());
}

/// A test procedure: its calls into a symbol-only `Assert` are anchored
/// `symbol-only-dependency` gaps.
#[test]
fn explain_gaps_of_symbol_only_calls() {
    check_case(&case(
        "explain.test-routine",
        "ws-explain",
        &["--routine", "PostUnreleasedFails"],
        0,
    ));
}

#[test]
fn explain_unknown_routine_exits_2() {
    check_case(&case(
        "explain.not-found",
        "ws-explain",
        &["--routine", "NoSuchRoutine"],
        2,
    ));
}

#[test]
fn explain_ambiguous_routine_exits_2() {
    check_case(&case(
        "explain.ambiguous",
        "ws-explain",
        &["--routine", "CheckAmount"],
        2,
    ));
}

#[test]
fn explain_diff_one_card_per_changed_routine() {
    check_case(&explain_diff());
}

#[test]
fn explain_field_writers() {
    check_case(&explain_field());
}

/// The step-5 routines of `ws-explain-calls` (golden slug, routine), one
/// golden pair each.
const CALLS_ROUTINES: &[(&str, &str)] = &[
    ("calls.postorder", "PostOrder"),
    ("calls.quickpost", "QuickPost"),
    ("calls.forwardpost", "ForwardPost"),
    ("calls.previewpost", "PreviewPost"),
    ("calls.recursivepost", "RecursivePost"),
    ("calls.release", "Release"),
];

fn calls_case(routine: &str) -> Case {
    let (slug, routine) = CALLS_ROUTINES
        .iter()
        .find(|(_, r)| *r == routine)
        .copied()
        .expect("a step-5 routine");
    case(slug, "ws-explain-calls", &["--routine", routine], 0)
}

#[test]
fn explain_rules_across_calls() {
    for (_, r) in CALLS_ROUTINES {
        check_case(&calls_case(r));
    }
}

/// The one card of `--routine <routine>` on `ws-explain-calls`.
fn calls_card(routine: &str) -> serde_json::Value {
    let doc = json_of(&calls_case(routine));
    doc["cards"][0].clone()
}

/// The message texts (or, with none, the whenText) of a rule list.
fn texts(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|r| {
            r["message"]["text"].as_str().map_or_else(
                || r["whenText"].as_str().unwrap().to_string(),
                str::to_string,
            )
        })
        .collect()
}

/// The first rule of `list` whose message text is `text`.
fn rule_with<'a>(list: &'a serde_json::Value, text: &str) -> &'a serde_json::Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|r| r["message"]["text"] == text)
        .unwrap_or_else(|| panic!("no rule with message {text:?} in {list}"))
}

/// The inherited `TestField(Amount)` of `CheckOrder` (a platform message).
fn check_order_testfield(list: &serde_json::Value) -> Option<&serde_json::Value> {
    list.as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "TestField" && r["anchor"]["file"] == "ws:src/checks.al")
}

/// Spec §4.3: what is inherited, and what is not.
#[test]
fn inherited_rules_follow_order_conditionality_and_literals() {
    let post = calls_card("PostOrder");
    let inh = &post["rules"]["inherited"];
    let t = texts(inh);
    // One call, and two calls deep.
    assert_eq!(
        rule_with(inh, "The order has no number.")["via"],
        serde_json::json!(["CheckOrder"])
    );
    assert_eq!(
        rule_with(inh, "The customer is blocked.")["via"],
        serde_json::json!(["CheckOrder", "CheckCustomer"])
    );
    // A callee called only in a branch, and one called after the first write.
    assert!(
        !t.contains(&"The amount is too big.".to_string()),
        "conditional call: {t:?}"
    );
    assert!(
        !t.contains(&"The order is already posted.".to_string()),
        "call after the write: {t:?}"
    );
    // A matching literal drops the atom: of `not No. empty`, `Strict` and
    // `Amount empty`, the two field atoms are left.
    let tf =
        check_order_testfield(inh).expect("CheckOrder's TestField, inherited with Strict = true");
    assert_eq!(
        tf["whenText"], "not (Order.\"No.\" = '') and Order.Amount is empty",
        "the matching literal drops the Strict atom: {tf}"
    );
    assert!(
        tf["when"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["kind"] == "FieldEmpty"),
        "the matching literal drops the Strict atom: {tf}"
    );
    // A MayNotStop callee rule is listed, as may-not-stop.
    assert_eq!(
        rule_with(inh, "A negative amount is collected.")["stops"],
        "may-not-stop"
    );
    // Table triggers run by Validate and Modify(true), with the write named.
    let ops: Vec<&str> = inh
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["trigger"]["op"].as_str())
        .collect();
    assert!(
        ops.contains(&"Validate") && ops.contains(&"Modify"),
        "trigger rules: {ops:?}"
    );
    // A subscriber's rule is under its extension point, not inherited.
    assert!(
        !t.contains(&"A zero amount cannot post.".to_string()),
        "{t:?}"
    );
    let subs = &post["extensionPoints"][0]["subscriberRoutines"][0]["rules"];
    assert!(
        texts(subs).contains(&"A zero amount cannot post.".to_string()),
        "{subs}"
    );
    // A page action's RunObject reaches the routine through OnRun.
    assert!(
        post["reachedFrom"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "PostIt" && r["runObject"].is_string()),
        "{}",
        post["reachedFrom"]
    );

    // A contradicting literal drops the rule; the rest stay.
    let quick = calls_card("QuickPost");
    let qinh = &quick["rules"]["inherited"];
    assert!(
        check_order_testfield(qinh).is_none(),
        "Strict = false: {qinh}"
    );
    assert!(texts(qinh).contains(&"The order has no number.".to_string()));

    // A forwarded parameter re-anchors the atom to the caller's parameter.
    let fwd = calls_card("ForwardPost");
    let tf = check_order_testfield(&fwd["rules"]["inherited"]).expect("forwarded TestField");
    assert!(
        tf["when"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "ParamBool" && a["index"] == 1),
        "re-anchored to ForwardPost's StrictMode (index 1): {tf}"
    );
    assert!(
        tf["whenText"].as_str().unwrap().contains("StrictMode"),
        "{tf}"
    );

    // A callee parameter assigned before its check is not substituted: the
    // literal `false` does not drop the rule.
    let preview = calls_card("PreviewPost");
    let p = rule_with(&preview["rules"]["inherited"], "A preview cannot post.");
    assert!(
        p["when"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "Other"),
        "kept as written: {p}"
    );

    // A recursive pair: the walk stops, each rule once.
    let rec = calls_card("RecursivePost");
    let n = texts(&rec["rules"]["inherited"])
        .iter()
        .filter(|t| *t == "The amount is negative.")
        .count();
    assert_eq!(n, 1, "{}", rec["rules"]["inherited"]);
}

/// Spec §4.3 caller rules and §5's table-trigger and error-collector gaps.
#[test]
fn caller_rules_and_trigger_and_collector_gaps() {
    let release = calls_card("Release");
    let callers = &release["rules"]["callers"];
    let own = rule_with(callers, "A posted order cannot be released.");
    assert_eq!(
        own["appliedBy"],
        serde_json::json!(["Codeunit Calls Post::ReleaseChecked"])
    );
    assert_eq!(
        own["skippedBy"],
        serde_json::json!(["Codeunit Calls Post::ReleaseDirect"])
    );
    // A MayNotStop rule is present, never applied.
    let collected = rule_with(callers, "A negative amount is collected.");
    assert_eq!(
        collected["presentIn"],
        serde_json::json!(["Codeunit Calls Post::ReleaseChecked"])
    );
    assert_eq!(collected["appliedBy"], serde_json::json!([]));
    assert_eq!(release["rules"]["knownCallers"], 2);

    let quick = calls_card("QuickPost");
    let gaps = quick["gaps"].as_array().unwrap();
    let skipped: Vec<&serde_json::Value> = gaps
        .iter()
        .filter(|g| g["kind"] == "table-trigger-rules-skipped")
        .collect();
    assert!(
        skipped
            .iter()
            .any(|g| g["op"] == "assign" && g["field"] == "Amount"),
        "a plain assignment skips OnValidate: {skipped:?}"
    );
    assert!(
        skipped.iter().any(|g| g["op"] == "modify"),
        "Modify() skips OnModify: {skipped:?}"
    );
    assert!(
        gaps.iter().any(|g| g["kind"] == "error-collector"),
        "LogIfBlocked's call to an error collector: {gaps:?}"
    );
    // PostOrder runs both triggers: nothing skipped.
    let post = calls_card("PostOrder");
    assert!(
        !post["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["kind"] == "table-trigger-rules-skipped"),
        "{}",
        post["gaps"]
    );
}

#[test]
fn explain_unknown_field_exits_2() {
    check_case(&case(
        "explain.field-unknown",
        "ws-explain",
        &["--table", "Explain Order", "--field", "NoSuchField"],
        2,
    ));
}

// ---------------------------------------------------------------------------
// Semantic assertions: the goldens pin bytes, these pin meaning.
// ---------------------------------------------------------------------------

/// `--diff` resolves its routines with `digest --diff`'s code, so the two
/// commands name the same changed set for the same diff.
#[test]
fn explain_diff_names_the_routines_digest_diff_names() {
    let ws = fixtures_dir().join("ws-explain");
    let ws = ws.to_string_lossy();
    let d = diff_input();
    let (digest, _, code) = run_alsem(&["digest", &ws, "--diff", &d, "--format", "json"]);
    assert_eq!(code, 0, "digest --diff");
    let digest: serde_json::Value = serde_json::from_str(&digest).unwrap();
    let mut from_digest: Vec<String> = digest["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["routine"]["stableId"].as_str().unwrap().to_string())
        .collect();
    from_digest.sort();
    let explain = json_of(&explain_diff());
    let mut from_explain: Vec<String> = explain["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["routine"]["id"].as_str().unwrap().to_string())
        .collect();
    from_explain.sort();
    assert!(!from_digest.is_empty(), "the diff changes routines");
    assert_eq!(from_explain, from_digest);
    // And digest's changed-roots diagnostics come through unchanged.
    assert_eq!(
        explain["diagnostics"], digest["payload"]["changedRootsDiagnostics"],
        "explain --diff reports digest's changed-roots diagnostics"
    );
}

/// Spec Done 5 and §4.4: every list of every card is present, an empty one
/// included. Since step 5 the cross-call lists are computed, so no card
/// carries step 4's `cross-call-rules-not-computed` gap any more.
#[test]
fn every_card_lists_are_present_and_cross_call_rules_are_computed() {
    let mut all = vec![
        rules_check_order(),
        explain_post_order(),
        explain_archive(),
        explain_diff(),
        explain_field(),
    ];
    all.extend(CALLS_ROUTINES.iter().map(|(_, r)| calls_case(r)));
    for c in all {
        let doc = json_of(&c);
        assert_eq!(doc["schema"], "al-sem.explain/1", "{}", c.slug);
        assert!(doc["diagnostics"].is_array(), "{}: diagnostics", c.slug);
        let cards = doc["cards"].as_array().expect("cards");
        assert!(!cards.is_empty(), "{}: cards", c.slug);
        for card in cards {
            for path in [
                "/rules/own",
                "/rules/inherited",
                "/rules/callers",
                "/effects/writes",
                "/effects/commitSites",
                "/effects/events",
                "/effects/ui",
                "/reachedFrom",
                "/extensionPoints",
                "/tests",
                "/gaps",
            ] {
                assert!(
                    card.pointer(path).is_some_and(serde_json::Value::is_array),
                    "{}: {path} missing or not a list",
                    c.slug
                );
            }
            assert!(card["effects"]["commits"].is_string(), "{}", c.slug);
            assert!(card["rules"]["knownCallers"].is_u64(), "{}", c.slug);
            assert!(
                !card["gaps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|g| g["kind"] == "cross-call-rules-not-computed"),
                "{}: step 5 computes the cross-call lists",
                c.slug
            );
        }
    }
    // The routine with no checks says so with an empty list.
    let archive = json_of(&explain_archive());
    assert_eq!(archive["cards"][0]["rules"]["own"], serde_json::json!([]));
}

/// The cards of a `--diff` and of a `--table/--field` run come sorted by
/// their routine's anchor.
#[test]
fn cards_are_sorted_by_anchor() {
    for c in [explain_diff(), explain_field()] {
        let doc = json_of(&c);
        let keys: Vec<(String, u64, u64)> = doc["cards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|card| {
                let a = &card["routine"]["anchor"];
                (
                    a["file"].as_str().unwrap().to_string(),
                    a["line"].as_u64().unwrap(),
                    a["column"].as_u64().unwrap(),
                )
            })
            .collect();
        assert!(keys.len() >= 2, "{}: two cards to order", c.slug);
        if let Some(writers) = doc["field"]["writers"].as_array() {
            let ids = |v: &[serde_json::Value], key: &str| -> Vec<String> {
                v.iter()
                    .map(|x| x.pointer(key).unwrap().as_str().unwrap().to_string())
                    .collect()
            };
            assert_eq!(
                ids(writers, "/id"),
                ids(doc["cards"].as_array().unwrap(), "/routine/id"),
                "{}: the writers list in the cards' order",
                c.slug
            );
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{}: cards sorted by anchor", c.slug);
    }
}

/// Usage errors, before any build: no mode exits 1 (digest's no-input
/// code); two modes, `--table` without `--field` and `--diff-root` without
/// `--diff` exit 3; a bad `--format` exits 1.
#[test]
fn explain_usage_errors() {
    let ws = fixtures_dir().join("ws-explain");
    let ws = ws.to_string_lossy();
    let d = diff_input();
    for (args, exit) in [
        (vec!["explain", &ws], 1),
        (vec!["explain", &ws, "--routine", "X", "--diff", &d], 3),
        (vec!["explain", &ws, "--table", "Explain Order"], 3),
        (
            vec!["explain", &ws, "--routine", "X", "--diff-root", "."],
            3,
        ),
        (vec!["explain", &ws, "--routine", "X", "--format", "xml"], 1),
    ] {
        let (stdout, stderr, code) = run_alsem(&args);
        assert_eq!(code, exit, "{args:?}: stderr {stderr}");
        assert!(stdout.is_empty(), "{args:?}: no output");
        assert!(!stderr.is_empty(), "{args:?}: says why");
    }
}

/// `explain` runs as a batch job: next to a digest on the same workspace
/// (one view, whose union of needs builds the rules for both), each job's
/// output equals its standalone run.
#[test]
fn explain_runs_in_a_batch_like_standalone() {
    use crate::cli_batch_plan::{argv, assert_each_standalone, batch_in};
    let root = repo_root();
    let ws = "tests/r0-corpus/ws-explain";
    let d = "tests/cli-explain-goldens/ws-explain.changed.diff";
    let jobs = vec![
        argv(&["explain", ws, "--routine", "PostOrder", "--format", "json"]),
        argv(&["digest", ws, "--routine", "PostOrder", "--deterministic"]),
        argv(&["explain", ws, "--diff", d]),
    ];
    let run = batch_in(&jobs, Some(&root), &[]);
    assert_each_standalone(&run, &jobs, Some(&root));
}
