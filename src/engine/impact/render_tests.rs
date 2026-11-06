//! The human answer's folds (agent-tasks §8.7 B-8, for a before/after answer
//! too): hand-stated parts, every precondition constructed literally.

use serde_json::{Value, json};

use super::*;
use crate::engine::impact::run::DeltaOutcome;

/// `n` changed roots, each with a before/after of `clauses` long changes.
fn parts(n: usize, clauses: usize) -> (Value, Value, DeltaOutcome) {
    let id = |i: usize| format!("p:Codeunit:{i}#aa");
    let roots: Vec<Value> = (0..n)
        .map(|i| {
            json!({"stableId": id(i), "display": format!("Codeunit A::R{i}"),
                   "anchor": {"file": "ws:src/a.al", "line": i + 1}, "status": "changed",
                   "effects": {"mayWriteTables": {"count": 0}, "commits": {"answer": "no", "ownSites": 0},
                               "kinds": {}, "events": {"distinct": 0}}})
        })
        .collect();
    let main = json!({"roots": roots, "accounting": null, "diagnosticsDetail": []});
    let up_roots: Vec<Value> = (0..n)
        .map(|i| {
            json!({"stableId": id(i), "status": "walked",
                   "ancestors": {"total": 1}, "entries": [], "callers": {"count": 0, "first": []},
                   "eventPublishers": {"count": 0}, "external": []})
        })
        .collect();
    let up = json!({"roots": up_roots, "union": {"ancestors": {"total": 1}, "entries": []}});
    let long = "x".repeat(300);
    let deltas: serde_json::Map<String, Value> = (0..n)
        .map(|i| {
            let own: Vec<Value> = (0..clauses)
                .map(
                    |c| json!({"change": "added", "label": format!("calls Codeunit B::{long}{c}")}),
                )
                .collect();
            (id(i), json!({"pairing": "id", "own": own, "cone": []}))
        })
        .collect();
    let delta = DeltaOutcome {
        block: json!({}),
        delta: json!({"deltas": deltas, "paired": n, "behaviorChanged": n, "deleted": [],
                      "schemaChanges": {"findings": [], "notComputed": "events"}}),
    };
    (main, up, delta)
}

fn human(main: &Value, up: &Value, delta: DeltaOutcome, opts: RenderOpts) -> String {
    render(
        &Comparison {
            source: "base",
            ..Default::default()
        },
        main,
        &Ok(up.clone()),
        &Err("not asked".into()),
        &Ok(delta),
        opts,
    )
    .text
}

/// Twenty routines of six 300-byte changes each pass the budget: the
/// default answer shows fewer, stays under it, and its fold names the full
/// count and why. Asked for (`max-roots`), every routine asked is shown.
///
/// Break: drop the budget loop in `render` → 20 routines, ~40 KB.
#[test]
fn a_large_answer_is_folded_under_its_budget_with_the_full_count() {
    let (main, up, delta) = parts(30, 6);
    let text = human(&main, &up, delta, RenderOpts::default());
    assert!(text.len() <= HUMAN_BUDGET, "{} bytes", text.len());
    let shown = text
        .lines()
        .filter(|l| l.starts_with("  Codeunit A::R"))
        .count();
    assert!((1..20).contains(&shown), "{shown} shown");
    assert!(text.contains(&format!("first {shown}:")), "{text}");
    assert!(
        text.contains(&format!(
            "  … +{} routines (the answer is kept under 38 KiB: --max-roots N",
            30 - shown
        )),
        "{text}"
    );

    let (main, up, delta) = parts(30, 6);
    let text = human(&main, &up, delta, RenderOpts::new(false, Some(25), false));
    let shown = text
        .lines()
        .filter(|l| l.starts_with("  Codeunit A::R"))
        .count();
    assert_eq!(shown, 25);
    assert!(text.contains("  … +5 routines (--max-roots N"), "{text}");
}

/// A count change lists its first two sites and counts the rest; `detail`
/// lists every one.
///
/// Break: list every site by default (`SITES_SHOWN` = `usize::MAX`).
#[test]
fn a_count_change_lists_its_first_sites_and_counts_the_rest() {
    let (main, up, mut delta) = parts(1, 0);
    let sites: Vec<String> = (1..=10).map(|l| format!("ws:src/a.al:{l}")).collect();
    delta.delta["deltas"]["p:Codeunit:0#aa"]["own"] = json!([{"change": "changed",
        "label": "calls Codeunit B::F", "base": 1, "new": 11, "addedSites": sites}]);
    let text = human(&main, &up, delta, RenderOpts::default());
    assert!(
        text.contains(
            "calls Codeunit B::F: 1 → 11 sites (added at ws:src/a.al:1, ws:src/a.al:2, … +8)"
        ),
        "{text}"
    );
    let (main, up, mut delta) = parts(1, 0);
    delta.delta["deltas"]["p:Codeunit:0#aa"]["own"] = json!([{"change": "changed",
        "label": "calls Codeunit B::F", "base": 1, "new": 11, "addedSites": sites}]);
    let text = human(&main, &up, delta, RenderOpts::new(false, None, true));
    assert!(text.contains("ws:src/a.al:9, ws:src/a.al:10)"), "{text}");
}

/// V3 §2.1: a root's line prints its display (which carries an overloaded
/// name's parameter types); the short handle stays in JSON only.
#[test]
fn a_root_line_prints_its_display_not_its_handle() {
    let (mut main, up, delta) = parts(1, 0);
    main["roots"][0]["handle"] = json!("/2");
    let text = human(&main, &up, delta, RenderOpts::default());
    assert!(text.contains("  Codeunit A::R0 (ws:src/a.al:1)"), "{text}");
    assert!(!text.contains("R0/2"), "{text}");
}

/// A witness only an open site (a hub) reaches says so, and so does the
/// count of such entries (round-1 c20: a `run:report` hub's witness was
/// printed beside the real path as a plain "reached from" entry). A path
/// through dependency code is named too; a local one carries no label.
///
/// Output validation V2 (§3.2, §8.1): every witness starts with its tier in
/// the word table's words, and the entry count says how many are via-hub.
///
/// Break: drop the tier from `node_short` → the `[via-hub]` witness assert
/// fails (2026-11-06, recorded in the V2 commit).
#[test]
fn an_open_site_witness_is_labelled_with_its_tier() {
    let (main, mut up, delta) = parts(1, 0);
    up["roots"][0]["entries"] = json!([{"kind": "report-trigger", "count": 3, "onlyOpen": 1,
        "userOrJob": true, "first": [
            {"display": "Report 357::company::onpostdataitem", "hops": 2, "tier": "local"},
            {"display": "Report 202::header::onaftergetrecord", "hops": 3, "tier": "throughDependency"},
            {"display": "Report 50100::onprereport", "hops": 5, "tier": "onlyOpen"}]}]);
    let text = human(&main, &up, delta, RenderOpts::new(false, None, true));
    assert!(text.contains("report triggers 3 (via-hub 1)"), "{text}");
    assert!(
        text.contains("[local] Report 357::company::onpostdataitem (2 hops); "),
        "{text}"
    );
    assert!(
        text.contains("[via-dependency] Report 202::header::onaftergetrecord (3 hops)"),
        "{text}"
    );
    assert!(
        text.contains("[via-hub] Report 50100::onprereport (5 hops)"),
        "{text}"
    );
}
