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

/// One routine's classified reach (round-2 r2 §2.1, W2), hand-stated: `code`
/// and `nav` ancestors (local) and entries, of `program` entries in all.
fn classified(code: usize, nav: usize, entries: (usize, usize), program: usize) -> Value {
    let share = |n: usize| {
        json!({"reached": n, "program": program,
               "percent": (n * 100).checked_div(program)})
    };
    json!({
        "relationRecorded": true,
        "byRelation": {"ancestors": {
            "code": {"local": code, "viaDependency": 0},
            "uiNavigation": {"local": nav, "viaDependency": 0},
            "candidates": {"overConfiguration": {"code": 0, "uiNavigation": 0},
                           "overRule": 0, "overHub": 2},
            "total": code + nav + 2}},
        "entriesByKind": {"code": {"page action": entries.0},
                          "uiNavigation": {"page action": entries.1}},
        "programShare": {"code": share(entries.0), "uiNavigation": share(entries.1),
                         "all": share(entries.0 + entries.1)},
        "nearest": {"code": {"display": "Page \"Card\"", "hops": 3, "tier": "local",
                             "relation": "code", "entry": "page action", "established": true},
                    "uiNavigation": {"display": "Page \"Sales Invoice\"", "hops": 2,
                                     "tier": "via-dependency", "relation": "UI navigation",
                                     "action": "Open Card", "entry": "page action",
                                     "established": true}},
        "externalCode": [{"why": "public", "count": 4}],
    })
}

/// Round-2 r2 §2.1 Rules: a population whose canonical entries are at least
/// 90% of the program's is `program-wide` and folds its counts by kind; a
/// narrow population prints its share and its kinds; a limited walk prints
/// neither a share nor an absence verdict.
///
/// DISCRIMINATION (2026-11-07): computing the share against the reached
/// set instead of the program's entries (`population_share` dividing by
/// `program.min(reached)`) fails the first assert: the navigation line
/// prints 100%, not 95%, and the narrow population turns program-wide.
#[test]
fn program_wide_needs_the_programs_entries_and_an_unlimited_walk() {
    let (main, mut up, delta) = parts(1, 0);
    let base = up["roots"][0].clone();
    let with = |c: Value| {
        let mut r = base.clone();
        r.as_object_mut()
            .unwrap()
            .extend(c.as_object().unwrap().clone());
        r
    };
    up["roots"][0] = with(classified(3, 40, (2, 95), 100));
    let text = human(&main, &up, delta, RenderOpts::default());
    assert!(
        text.contains(
            "reached by UI navigation: 40 ancestors — local 40, via-dependency 0; program-wide: \
             95% of the program's entries (95 of 100); counts by kind folded (--detail prints \
             them)"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "reached by code: 3 ancestors — local 3, via-dependency 0; 2% of the \
                       program's entries (2 of 100)"
        ),
        "the narrow population is not program-wide: {text}"
    );
    assert!(
        text.contains("      entries by kind: page actions 2\n"),
        "{text}"
    );
    assert!(!text.contains("page actions 95"), "folded: {text}");
    assert!(
        text.contains(
            "nearest entry: [via-dependency, UI navigation] Page \"Sales Invoice\" action \
             \"Open Card\" (page action, 2 hops)"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "external: apps outside the program may call routines of its code population"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "candidates (relation not established): over a rule edge 0, over a hub 2 ancestors"
        ),
        "{text}"
    );
    // A limited walk: no share, no absence verdict.
    let (main, mut up, delta) = parts(1, 0);
    let mut r = with(classified(0, 40, (0, 95), 100));
    r["limited"] = json!("state budget 2000000");
    up["roots"][0] = r;
    let text = human(&main, &up, delta, RenderOpts::default());
    assert!(!text.contains("program-wide"), "{text}");
    assert!(!text.contains("every candidate decided"), "{text}");
    assert!(
        text.contains(
            "reached by code: none — no established code-path witness in the analyzed graph; \
                       analysis partial (limit: state budget 2000000)"
        ),
        "{text}"
    );
}

/// Round-2 c20: the run-first codeunits cover the tests the block picked
/// (its tier, local by default), not every test with a modeled path. The
/// line states what the codeunits cover and counts the others apart, by
/// tier. Hand-stated: one local test in codeunit 69000, 446 tests through
/// dependency code and 231 over a hub.
///
/// DISCRIMINATION (2026-11-06): printing every test with a modeled path as
/// the ones the codeunits cover (`picked + rest` for `picked` in
/// `tests_line`) fails at "a superset of the 1 test picked on a local
/// path"; restored, passes.
#[test]
fn the_run_first_line_states_what_its_codeunits_cover() {
    let (main, up, delta) = parts(1, 0);
    let doc = json!({
        "scope": {"apps": [{"name": "T"}], "workspaceTests": 0},
        "runFirst": [{"codeunitId": 69000, "procedure": "A"}],
        "runFirstTotal": 1,
        "tier": "local",
        "byTier": {"local": 1, "throughDependency": 446, "onlyOpen": 231},
        "selectionComplete": false,
        "requiredFallback": [{"reason": "unresolved-test-calls"}],
        "changed": [],
    });
    let tests = Ok(TestsOutcome {
        doc,
        parts: Vec::new(),
        nearest: Default::default(),
    });
    let text = render(
        &Comparison {
            source: "base",
            ..Default::default()
        },
        &main,
        &Ok(up),
        &tests,
        &Ok(delta),
        RenderOpts::default(),
    )
    .text;
    assert!(
        text.contains(
            "tests: run first (app \"T\"): codeunits 69000 — whole codeunits, so every test in \
             them runs: a superset of the 1 test picked on a local path; 677 tests have a \
             modeled path only via dependency code (446) or via a hub (231) and are not in this \
             filter; then the whole suite"
        ),
        "{text}"
    );
    assert!(!text.contains("for the 678 tests"), "{text}");
}
