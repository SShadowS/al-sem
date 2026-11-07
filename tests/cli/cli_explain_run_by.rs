//! Who calls a routine and what runs a trigger (round-2 output validation,
//! `docs/superpowers/specs/2026-11-07-output-validation-r2.md` §2.3 G3 and
//! §2.4 G4's W1 rows), driven through the shipped `alsem` binary on
//! `tests/fixtures/explain-run-by` (its README states every case). The
//! expectations are hand-stated from the fixture text, never computed by
//! production code.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/explain-run-by/ws")
}

fn explain(routine: &str, json: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.arg("explain")
        .arg(format!("--routine={routine}"))
        .arg("--single-app");
    if json {
        cmd.arg("--format=json");
    }
    let out = cmd.arg("--").arg(fixture()).output().expect("spawn alsem");
    assert!(
        out.status.success(),
        "explain {routine} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// G3: "callers" is one set on every line of the card, the direct callers
/// (the caller rules' set): `Target` is called by two procedures (`Middle`,
/// `ProcB`) and by the `Direct` action's trigger; two page actions reach it
/// in all (`Direct` directly, `Indirect` through `Middle`).
///
/// DISCRIMINATION (2026-11-07): counting only the routine callers as the
/// direct set again (`DirectCallers::total` from the rows that are not
/// action triggers) fails the summary assert (`2 direct`).
#[test]
fn callers_count_direct_page_actions() {
    let human = explain(r#"Codeunit "RB Mgt"::Target"#, false);
    // Round-2 G25: the summary's `callers and tests:` line is not printed
    // beside the `callers:` line below (one copy).
    assert!(!human.contains("callers and tests:"), "{human}");
    assert!(
        human.contains(
            r#"callers: 3 direct — 2 routines: Codeunit "RB Mgt"::Middle, Codeunit "RB Mgt"::ProcB; 1 page action in 1 object (--detail lists them); 2 page actions reach it in all (directly or through callers)"#
        ),
        "{human}"
    );
    let v: Value = serde_json::from_str(&explain(r#"Codeunit "RB Mgt"::Target"#, true)).unwrap();
    let card = &v["cards"][0];
    assert_eq!(card["callers"]["direct"], 3, "{card:#}");
    assert_eq!(card["callers"]["directPageActions"], 1, "{card:#}");
    assert_eq!(card["reachedFromSummary"]["actions"], 2, "{card:#}");
}

/// G4 (W1 rows): a trigger says what runs it, never `callers: none`: a
/// field's `OnValidate` by its `Validate` calls (one, in `Validator`) and the
/// pages on its table as potential UI surfaces (one); `OnLookup` by no code
/// call; a page action is an entry point; a subscriber by its publisher;
/// any other trigger with no direct caller says what runs it is not listed
/// yet.
///
/// DISCRIMINATION (2026-11-07): `RunBy::of` returning `None` for every
/// trigger (so the card prints `callers: …` as before) fails the first
/// assert.
#[test]
fn a_trigger_says_what_runs_it() {
    let validate = explain(r#"Table "RB Ledger"."No."::OnValidate"#, false);
    assert!(
        validate.contains(
            r#"run by: Validate calls on the field: 1 site (Codeunit "RB Mgt"::Validator at ws:src/RunBy.al:92); potential UI surfaces: 1 page on table "RB Ledger" (Page "RB Ledger Card"; whether a page shows or edits this field is not established); an assignment to the field does not run it"#
        ),
        "{validate}"
    );
    // Round-2 G25: one `run by:` line (no summary copy).
    assert_eq!(validate.matches("run by:").count(), 1, "{validate}");
    let lookup = explain(r#"Table "RB Ledger".Amount::OnLookup"#, false);
    assert!(
        lookup.contains("run by: no code call (AL code cannot run a field's OnLookup; a user's lookup does); potential UI surfaces: 1 page on table \"RB Ledger\""),
        "{lookup}"
    );
    let action = explain(r#"Page "RB Actions".action(Direct)::OnAction"#, false);
    assert!(
        action.contains("  run by: entry point: this action (no code caller)\n"),
        "{action}"
    );
    assert!(
        !action.contains("reached from page action"),
        "the action is not listed as reached from itself: {action}"
    );
    let subscriber = explain(r#"Codeunit "RB Subscriber"::OnAfterInsertLedger"#, false);
    assert!(
        subscriber.contains(
            r#"run by: the publisher: Table "RB Ledger"::OnAfterInsertEvent; direct code callers: none found"#
        ),
        "{subscriber}"
    );
    let insert = explain(r#"Table "RB Ledger"::OnInsert"#, false);
    assert!(
        insert.contains("run by: record operations that run it: none observed"),
        "{insert}"
    );
    for text in [&validate, &lookup, &action, &subscriber, &insert] {
        assert!(!text.contains("is not listed for this kind yet"), "{text}");
        assert!(
            !text
                .lines()
                .any(|l| l.trim_start().starts_with("callers: none")
                    || l.contains("callers and tests: callers: none")),
            "{text}"
        );
    }
}

/// G4 (W3 rows): a table's record trigger is run by the record operations
/// whose implicit-trigger edge reaches it — `Inserter`'s `Insert(true)`, not
/// `InsertsQuietly`'s `Insert()` (no RunTrigger) — with the pages on its
/// table as potential UI surfaces; a page trigger a run runs (`OnOpenPage`)
/// by the code runs of the page; one a run does not run (`OnInsertRecord`)
/// by the page's lifecycle.
#[test]
fn table_and_page_triggers_say_what_runs_them() {
    let insert = explain(r#"Table "RB Journal"::OnInsert"#, false);
    assert!(
        insert.contains(
            r#"run by: record operations that run it: 1 site (Insert at ws:src/RunBy.al:143 in Codeunit "RB Runner"::Inserter); potential UI surfaces: 1 page on table "RB Journal" (Page "RB Journal Card")"#
        ),
        "{insert}"
    );
    assert!(!insert.contains("InsertsQuietly"), "{insert}");
    let v: Value = serde_json::from_str(&explain(r#"Table "RB Journal"::OnInsert"#, true)).unwrap();
    let rb = &v["cards"][0]["runBy"];
    assert_eq!(rb["kind"], "table-trigger", "{rb:#}");
    assert_eq!(
        rb["recordOperations"].as_array().map(Vec::len),
        Some(1),
        "{rb:#}"
    );
    let open = explain(r#"Page "RB Journal Card"::OnOpenPage"#, false);
    assert!(
        open.contains(
            r#"run by: code runs of the page: 1 routine (Codeunit "RB Runner"::OpensCard); page actions that open it: none observed"#
        ),
        "{open}"
    );
    let new_record = explain(r#"Page "RB Journal Card"::OnInsertRecord"#, false);
    assert!(
        new_record.contains(
            r#"run by: the page's lifecycle while a user works in Page "RB Journal Card""#
        ),
        "{new_record}"
    );
}
