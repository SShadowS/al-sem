//! Round-3 fact check of the 2026-11-07 output validation (n09, m03): a
//! page's lifecycle into its source table's record triggers and table events
//! (a user inserting, modifying, renaming, deleting or editing a field in the
//! page) honours what the page lets a user do (`program::page_ui`).
//!
//! - `InsertAllowed = false` blocks insert, `ModifyAllowed = false` modify and
//!   rename, `DeleteAllowed = false` delete; a field edit (`OnValidate`) needs
//!   insert or modify. These cannot be set at runtime: no route.
//! - `Editable = false` blocks insert, modify, rename and field edits, not
//!   delete (Microsoft Learn: "Do not use `CurrPage.Editable` to prevent users
//!   from deleting entries"). `Page.Editable(true)` can undo it at runtime, so
//!   a page some code may make editable keeps the route as a candidate
//!   (`overRule`, relation not established), not an established one.
//! - `SourceTableTemporary = true` leaves the table's own triggers (they run
//!   on temporary records) and carries the temporary record to its table
//!   events: a subscriber that exits on a temporary record is not reached.
//!
//! Every consumer reads the same answer: impact's populations and entries,
//! the tests a `TestPage` on the page selects, and `explain`'s potential UI
//! surfaces of a table trigger.

use std::collections::BTreeSet;
use std::process::Command;

const SOURCE: &str = r#"table 50800 "PU Rec"
{
    fields
    {
        field(1; Code; Code[20]) { }
        field(2; Name; Text[50])
        {
            trigger OnValidate()
            begin
            end;
        }
    }
    keys { key(PK; Code) { } }
    trigger OnInsert()
    begin
    end;
    trigger OnModify()
    begin
    end;
    trigger OnDelete()
    begin
    end;
    trigger OnRename()
    begin
    end;
}
codeunit 50820 "PU Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"PU Rec", 'OnAfterDeleteEvent', '', false, false)]
    local procedure AfterDelete(var Rec: Record "PU Rec"; RunTrigger: Boolean)
    begin
    end;

    [EventSubscriber(ObjectType::Table, Database::"PU Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertTemp(var Rec: Record "PU Rec"; RunTrigger: Boolean)
    begin
        if Rec.IsTemporary() then
            exit;
    end;
}
page 50850 "PU Open Page" { SourceTable = "PU Rec"; }
page 50851 "PU No Insert Page" { SourceTable = "PU Rec"; InsertAllowed = false; }
page 50852 "PU No Modify Page" { SourceTable = "PU Rec"; ModifyAllowed = false; }
page 50853 "PU No Delete Page" { SourceTable = "PU Rec"; DeleteAllowed = false; }
page 50854 "PU Read Only Page" { SourceTable = "PU Rec"; Editable = false; }
page 50855 "PU Read Only Set Page" { SourceTable = "PU Rec"; Editable = false; }
page 50856 "PU Temp Page" { SourceTable = "PU Rec"; SourceTableTemporary = true; }
page 50857 "PU No Edit Page" { SourceTable = "PU Rec"; InsertAllowed = false; ModifyAllowed = false; }
page 50858 "PU Read Only Ext Page" { SourceTable = "PU Rec"; Editable = false; }
pageextension 50859 "PU Read Only Ext" extends "PU Read Only Ext Page"
{
    trigger OnOpenPage()
    begin
        CurrPage.Editable(true);
    end;
}
codeunit 50860 "PU Run Open" { trigger OnRun() begin Page.Run(Page::"PU Open Page"); end; }
codeunit 50861 "PU Run No Insert" { trigger OnRun() begin Page.Run(Page::"PU No Insert Page"); end; }
codeunit 50862 "PU Run No Modify" { trigger OnRun() begin Page.Run(Page::"PU No Modify Page"); end; }
codeunit 50863 "PU Run No Delete" { trigger OnRun() begin Page.Run(Page::"PU No Delete Page"); end; }
codeunit 50864 "PU Run Read Only" { trigger OnRun() begin Page.Run(Page::"PU Read Only Page"); end; }
codeunit 50865 "PU Run Read Only Set"
{
    trigger OnRun()
    var
        P: Page "PU Read Only Set Page";
    begin
        P.Editable(true);
        P.Run();
    end;
}
codeunit 50866 "PU Run Temp" { trigger OnRun() begin Page.Run(Page::"PU Temp Page"); end; }
codeunit 50867 "PU Run No Edit" { trigger OnRun() begin Page.Run(Page::"PU No Edit Page"); end; }
codeunit 50868 "PU Run Read Only Ext" { trigger OnRun() begin Page.Run(Page::"PU Read Only Ext Page"); end; }
codeunit 50890 "PU Tests"
{
    Subtype = Test;

    [Test]
    procedure NewOnOpenPage()
    var
        P: TestPage "PU Open Page";
    begin
        P.OpenNew();
    end;

    [Test]
    procedure NewOnNoInsertPage()
    var
        P: TestPage "PU No Insert Page";
    begin
        P.OpenNew();
    end;

    [Test]
    procedure NewOnBothPages()
    var
        P: TestPage "PU Read Only Set Page";
        Q: TestPage "PU Open Page";
    begin
        P.OpenNew();
        Q.OpenNew();
    end;
}
codeunit 50891 "PU Candidate Tests"
{
    Subtype = Test;

    [Test]
    procedure NewOnReadOnlySetPage()
    var
        P: TestPage "PU Read Only Set Page";
    begin
        P.OpenNew();
    end;
}
"#;

fn workspace(root: &std::path::Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"38383838-0000-4000-8000-000000000038","name":"PU","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    ws
}

fn alsem_json(args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: not JSON ({e}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// The `PU Run …` codeunits (by the part after `PU Run `) that reach
/// `routine`, and how many of those codeunit entries are established UI
/// navigation and how many candidates over a rule edge (`overRule`).
fn entries(ws: &str, routine: &str) -> (BTreeSet<String>, u64, u64) {
    let v = alsem_json(&[
        "impact",
        &format!("--routine={routine}"),
        "--no-tests",
        "--detail",
        "--format=json",
        "--",
        ws,
    ]);
    let up = &v["roots"][0]["up"];
    let short = |d: &str| {
        d.strip_prefix("Codeunit \"PU Run ")
            .and_then(|s| s.split('"').next())
            .map(str::to_string)
    };
    let all: BTreeSet<String> = up["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{routine}: no entries: {v}"))
        .iter()
        .flat_map(|e| e["first"].as_array().cloned().unwrap_or_default())
        .filter_map(|f| f["display"].as_str().and_then(short))
        .collect();
    let count = |population: &str| {
        up["entriesByKind"][population]["codeunit OnRun"]
            .as_u64()
            .unwrap_or(0)
    };
    (all, count("uiNavigation"), count("overRule"))
}

fn set(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|s| (*s).to_string()).collect()
}

/// DISCRIMINATION (2026-11-07, each break made with Edit, the test run red,
/// reverted, run green): see the commit message for the break per guard.
#[test]
fn page_permissions_decide_which_record_triggers_a_user_runs_through_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let cases: &[(&str, &[&str], &[&str])] = &[
        // (routine, established entries, candidate entries)
        (
            "\"PU Rec\"::OnInsert",
            &["Open", "No Modify", "No Delete", "Temp"],
            &["Read Only Set", "Read Only Ext"],
        ),
        (
            "\"PU Rec\"::OnModify",
            &["Open", "No Insert", "No Delete", "Temp"],
            &["Read Only Set", "Read Only Ext"],
        ),
        (
            "\"PU Rec\"::OnRename",
            &["Open", "No Insert", "No Delete", "Temp"],
            &["Read Only Set", "Read Only Ext"],
        ),
        (
            "\"PU Rec\"::OnDelete",
            &[
                "Open",
                "No Insert",
                "No Modify",
                "No Edit",
                "Read Only",
                "Read Only Set",
                "Read Only Ext",
                "Temp",
            ],
            &[],
        ),
        (
            "\"PU Rec\".Name::OnValidate",
            &["Open", "No Insert", "No Modify", "No Delete", "Temp"],
            &["Read Only Set", "Read Only Ext"],
        ),
        (
            "\"PU Subscribers\"::AfterDelete",
            &[
                "Open",
                "No Insert",
                "No Modify",
                "No Edit",
                "Read Only",
                "Read Only Set",
                "Read Only Ext",
                "Temp",
            ],
            &[],
        ),
        // The temporary-source page's user insert raises the event on a
        // temporary record, which this subscriber exits on.
        (
            "\"PU Subscribers\"::AfterInsertTemp",
            &["Open", "No Modify", "No Delete"],
            &["Read Only Set", "Read Only Ext"],
        ),
    ];
    let mut failures = String::new();
    for (routine, established, candidates) in cases {
        let (all, nav, rule) = entries(w, routine);
        let mut want_all = set(established);
        want_all.extend(set(candidates));
        let (want_nav, want_rule) = (established.len() as u64, candidates.len() as u64);
        if all != want_all || nav != want_nav || rule != want_rule {
            failures.push_str(&format!(
                "{routine}: entries {all:?} (want {want_all:?}); established {nav} (want \
                 {want_nav}), candidates {rule} (want {want_rule})\n"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures}");
}

/// A `TestPage` on a page that does not allow inserts selects no test for
/// the table's `OnInsert`; one on a page that does still selects it.
#[test]
fn tests_through_a_page_honour_its_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let v = alsem_json(&[
        "tests",
        w,
        "--routine",
        "\"PU Rec\"::OnInsert",
        "--format",
        "json",
    ]);
    let text = v.to_string();
    assert!(text.contains("NewOnOpenPage"), "{text}");
    assert!(!text.contains("NewOnNoInsertPage"), "{text}");
}

/// `explain`'s potential UI surfaces of a table trigger leave out the pages
/// that block its operation for certain, and keep a candidate.
#[test]
fn explain_surfaces_leave_out_pages_that_block_the_operation() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let surfaces = |routine: &str| -> BTreeSet<String> {
        let v = alsem_json(&["explain", w, "--routine", routine, "--format", "json"]);
        v["cards"][0]["runBy"]["potentialUiSurfaces"]
            .as_array()
            .unwrap_or_else(|| panic!("{routine}: no surfaces: {v}"))
            .iter()
            .filter_map(|p| p.as_str())
            .filter_map(|p| p.strip_prefix("Page \"PU "))
            .filter_map(|p| p.strip_suffix(" Page\""))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(
        surfaces("\"PU Rec\"::OnInsert"),
        set(&[
            "Open",
            "No Modify",
            "No Delete",
            "Read Only Set",
            "Read Only Ext",
            "Temp"
        ])
    );
    assert_eq!(
        surfaces("\"PU Rec\"::OnDelete"),
        set(&[
            "Open",
            "No Insert",
            "No Modify",
            "No Edit",
            "Read Only",
            "Read Only Set",
            "Read Only Ext",
            "Temp"
        ])
    );
    assert_eq!(
        surfaces("\"PU Rec\".Name::OnValidate"),
        set(&[
            "Open",
            "No Insert",
            "No Modify",
            "No Delete",
            "Read Only Set",
            "Read Only Ext",
            "Temp"
        ])
    );
}

fn alsem_text(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Output validation r3 L3: a test whose only path to `OnInsert` is a
/// candidate page step (`PU Read Only Set Page` is `Editable = false`, and
/// code may make it editable) keeps relation `UI navigation` and carries
/// `candidate: "runtime-editable"`; a test that also opens an established
/// page (`NewOnBothPages`) does not: the established route wins. The human
/// text says `UI navigation (candidate)`; schema 2 drops the field.
///
/// DISCRIMINATION (2026-11-08): the walk not marking a candidate site
/// (`ext::CANDIDATE` left out of `Sel::step`) fails "the candidate test
/// carries it"; restored, it passes.
#[test]
fn a_test_through_a_candidate_page_is_marked_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let args = [
        "tests",
        w,
        "--routine",
        "\"PU Rec\"::OnInsert",
        "--format",
        "json",
    ];
    let v = alsem_json(&args);
    let row = |p: &str| {
        v["runFirst"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["procedure"] == p)
            .cloned()
            .unwrap_or_else(|| panic!("{p}: {v:#}"))
    };
    let cand = row("NewOnReadOnlySetPage");
    assert_eq!(cand["reaches"][0]["relation"], "UI navigation", "{cand:#}");
    assert_eq!(
        cand["reaches"][0]["candidate"], "runtime-editable",
        "the candidate test carries it: {cand:#}"
    );
    let both = row("NewOnBothPages");
    assert_eq!(both["reaches"][0]["relation"], "UI navigation", "{both:#}");
    assert!(both["reaches"][0].get("candidate").is_none(), "{both:#}");
    // The nearest test is an established one.
    assert!(
        v["changed"][0]["nearest"].get("candidate").is_none(),
        "{:#}",
        v["changed"][0]
    );
    let mut old = args.to_vec();
    old.extend(["--schema", "2"]);
    assert!(!alsem_json(&old).to_string().contains("runtime-editable"));
    let human = alsem_text(&["tests", w, "--routine", "\"PU Rec\"::OnInsert"]);
    assert!(
        human.contains("PU Candidate Tests: 1 test; first [local, UI navigation (candidate)]"),
        "{human}"
    );
}

/// Output validation r3 L3: `explain` marks a page that blocks the step only
/// while no runtime `Editable(…)` setter makes it editable, and lists those
/// pages in JSON (`mayBeReadOnlyAtRuntime`); a page without the property is
/// not marked.
///
/// DISCRIMINATION (2026-11-08): `pages_on_table` never marking a page
/// (`page_may_be_read_only` replaced by `false`) fails the first assert.
#[test]
fn explain_marks_pages_that_may_be_read_only_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let human = alsem_text(&["explain", w, "--routine", "\"PU Rec\"::OnInsert"]);
    assert!(
        human.contains("Page \"PU Read Only Set Page\" (may be read-only at runtime)"),
        "{human}"
    );
    assert!(
        !human.contains("Page \"PU Open Page\" (may be read-only at runtime)"),
        "{human}"
    );
    let v = alsem_json(&[
        "explain",
        w,
        "--routine",
        "\"PU Rec\"::OnInsert",
        "--format",
        "json",
    ]);
    assert_eq!(
        v["cards"][0]["runBy"]["mayBeReadOnlyAtRuntime"],
        serde_json::json!([
            "Page \"PU Read Only Ext Page\"",
            "Page \"PU Read Only Set Page\""
        ]),
        "{:#}",
        v["cards"][0]["runBy"]
    );
}
