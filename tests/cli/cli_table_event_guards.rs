//! Round-2 fact check of the 2026-11-06 output validation (calls c02, c12,
//! c20, n09, n10): table-event dispatch that cannot run in its raising
//! context, and a RecordRef opened on a literal table
//! (`program::table_event_guard`, `program::recordref_table`).
//!
//! - A subscriber whose first statement is `if Rec.IsTemporary() then exit;`
//!   runs nothing when the raising record is declared temporary; one whose
//!   first statement is `if not RunTrigger then exit;` runs nothing when the
//!   operation passes RunTrigger false (a literal, or left out). Neither the
//!   effects (the model's table-event edges, read by `prove`) nor the paths
//!   (the sited graph's reverse walk, read by `impact`) may use it there.
//! - Where the site does not decide (a record whose temp state the caller
//!   decides, a computed RunTrigger), the dispatch stays.
//! - A guard that still runs code (`if Rec.IsTemporary or Disabled() then
//!   exit;`: AL evaluates every operand) keeps its edge.
//! - A local RecordRef bound only by `Open(Database::"T")` is table T: its
//!   `Modify(true)` fires T's `OnModify`, its `Modify()` none, and both raise
//!   T's table events; before, the operation was the open record hub.

use std::process::Command;

const SOURCE: &str = r#"table 50700 "TG Rec"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
    trigger OnModify()
    var
        W: Record "TG W Trig";
    begin
        W.Insert();
    end;
}
table 50710 "TG W Temp" { fields { field(1; Id; Integer) { } } }
table 50711 "TG W RT" { fields { field(1; Id; Integer) { } } }
table 50712 "TG W Call" { fields { field(1; Id; Integer) { } } }
table 50713 "TG W Trig" { fields { field(1; Id; Integer) { } } }
table 50714 "TG W Ref" { fields { field(1; Id; Integer) { } } }
table 50715 "TG W Arm" { fields { field(1; Id; Integer) { } } }
table 50716 "TG W Arm2" { fields { field(1; Id; Integer) { } } }
codeunit 50720 "TG Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertTemp(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Temp";
    begin
        if Rec.IsTemporary() then
            exit;
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterDeleteEvent', '', false, false)]
    local procedure AfterDeleteRT(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W RT";
    begin
        if not RunTrigger then
            exit;
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsertCall(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Call";
    begin
        if Rec.IsTemporary or Disabled() then
            exit;
        W.Insert();
    end;

    local procedure Disabled(): Boolean
    begin
        exit(false);
    end;

#if CLEAN
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertArm(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Arm";
    begin
        if Rec.IsTemporary() then
            exit;
        W.Insert();
    end;
#else
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertArm(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Arm";
    begin
        W.Insert();
    end;
#endif

#if CLEAN
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertArmElse(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Arm2";
    begin
        W.Insert();
    end;
#else
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertArmElse(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Arm2";
    begin
        if Rec.IsTemporary() then
            exit;
        W.Insert();
    end;
#endif

    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterModifyEvent', '', false, false)]
    local procedure AfterModifyRef(var Rec: Record "TG Rec"; var xRec: Record "TG Rec"; RunTrigger: Boolean)
    var
        W: Record "TG W Ref";
    begin
        W.Insert();
    end;
}
codeunit 50730 "TG Callers"
{
    procedure InsertTemp()
    var
        R: Record "TG Rec" temporary;
    begin
        R.Insert();
    end;

    procedure InsertPersistent()
    var
        R: Record "TG Rec";
    begin
        R.Insert();
    end;

    procedure InsertTempParam(var R: Record "TG Rec" temporary)
    begin
        R.Insert();
    end;

    procedure InsertCallersRecord(var R: Record "TG Rec")
    begin
        R.Insert();
    end;

    procedure DeleteAllDefault()
    var
        R: Record "TG Rec";
    begin
        R.DeleteAll();
    end;

    procedure DeleteAllTrue()
    var
        R: Record "TG Rec";
    begin
        R.DeleteAll(true);
    end;

    procedure DeleteComputed(Run: Boolean)
    var
        R: Record "TG Rec";
    begin
        R.Delete(Run);
    end;

    procedure RefModifyTrue()
    var
        R: RecordRef;
    begin
        R.Open(Database::"TG Rec");
        R.Modify(true);
    end;

    procedure RefModifyDefault()
    var
        R: RecordRef;
    begin
        R.Open(Database::"TG Rec");
        R.Modify();
        R.Close();
    end;

    procedure RefTwoTables()
    var
        R: RecordRef;
    begin
        R.Open(Database::"TG Rec");
        R.Close();
        R.Open(Database::"TG W Ref");
        R.Modify(true);
    end;

    procedure RefComputed(No: Integer)
    var
        R: RecordRef;
    begin
        R.Open(No);
        R.Modify(true);
    end;
}
page 50750 "TG Temp Page"
{
    SourceTable = "TG Rec";
    SourceTableTemporary = true;
    actions
    {
        area(Processing)
        {
            action(InsertIt)
            {
                trigger OnAction()
                begin
                    Rec.Insert();
                end;
            }
        }
    }
}
page 50751 "TG Page"
{
    SourceTable = "TG Rec";
    actions
    {
        area(Processing)
        {
            action(InsertIt)
            {
                trigger OnAction()
                begin
                    Rec.Insert();
                end;
            }
        }
    }
}
codeunit 50742 "TG Open Temp Page"
{
    trigger OnRun()
    begin
        Page.Run(Page::"TG Temp Page");
    end;
}
codeunit 50743 "TG Open Page"
{
    trigger OnRun()
    begin
        Page.Run(Page::"TG Page");
    end;
}
codeunit 50740 "TG Run Temp"
{
    trigger OnRun()
    var
        R: Record "TG Rec" temporary;
    begin
        R.Insert();
    end;
}
codeunit 50741 "TG Run Persistent"
{
    trigger OnRun()
    var
        R: Record "TG Rec";
    begin
        R.Insert();
    end;
}
"#;

fn workspace(root: &std::path::Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"37373737-0000-4000-8000-000000000037","name":"TG","publisher":"t","version":"1.0.0.0"}"#,
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

fn prove_answer(ws: &str, routine: &str, question: &str) -> String {
    let v = alsem_json(&["prove", ws, routine, question, "--format", "json"]);
    v["payload"]["result"]["answer"]
        .as_str()
        .unwrap_or_else(|| panic!("{routine} {question}: no answer: {v}"))
        .to_string()
}

/// Effects: what each caller may write through the subscribers.
///
/// DISCRIMINATION (2026-11-07, each break made with Edit, run, reverted, run
/// green): dropping the model's kill arm (`program_calls::table_events`)
/// fails temp-local, temp-param, deleteall-default; reading the RunTrigger
/// site fact as never false fails deleteall-default; accepting a non-atom
/// operand in a guard condition fails or-call; the union instead of the
/// intersection over a node's `#if` rows fails if-arms and if-arms-else;
/// `recordref_constant_table` returning `None` fails ref-trigger, ref-event,
/// ref-default-event; letting two disagreeing bindings pass fails
/// ref-two-trigger (resolver) and ref-two-own (model `record_types`);
/// skipping the model's RecordRef pass fails ref-no-trigger and
/// ref-own-write.
#[test]
fn guarded_subscribers_do_not_run_where_the_site_proves_their_guard() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    // `want` "no" must be "no"; "not-no" accepts anything but "no" (a kept,
    // conditional dispatch, or a hub); "not-yes" anything but "yes".
    let cases: &[(&str, &str, &str, &str)] = &[
        ("temp-local", "TG Callers::InsertTemp", "TG W Temp", "no"),
        (
            "persistent",
            "TG Callers::InsertPersistent",
            "TG W Temp",
            "yes",
        ),
        (
            "temp-param",
            "TG Callers::InsertTempParam",
            "TG W Temp",
            "no",
        ),
        // The caller decides the temp state: kept.
        (
            "callers-record",
            "TG Callers::InsertCallersRecord",
            "TG W Temp",
            "not-no",
        ),
        // A guard beside a call still runs: kept, temporary or not.
        ("or-call", "TG Callers::InsertTemp", "TG W Call", "yes"),
        // RunTrigger left out (false): the `not RunTrigger` guard exits.
        (
            "deleteall-default",
            "TG Callers::DeleteAllDefault",
            "TG W RT",
            "no",
        ),
        (
            "deleteall-true",
            "TG Callers::DeleteAllTrue",
            "TG W RT",
            "yes",
        ),
        (
            "delete-computed",
            "TG Callers::DeleteComputed",
            "TG W RT",
            "not-no",
        ),
        // A RecordRef opened on a literal table is that table.
        (
            "ref-trigger",
            "TG Callers::RefModifyTrue",
            "TG W Trig",
            "yes",
        ),
        ("ref-event", "TG Callers::RefModifyTrue", "TG W Ref", "yes"),
        (
            "ref-no-trigger",
            "TG Callers::RefModifyDefault",
            "TG W Trig",
            "no",
        ),
        (
            "ref-default-event",
            "TG Callers::RefModifyDefault",
            "TG W Ref",
            "yes",
        ),
        // A computed table stays open.
        (
            "ref-computed",
            "TG Callers::RefComputed",
            "TG W Trig",
            "not-no",
        ),
        // The RecordRef's own write is to the table it was opened on.
        (
            "ref-own-write",
            "TG Callers::RefModifyDefault",
            "TG Rec",
            "yes",
        ),
        // Opened on two tables: neither is the operation's table.
        (
            "ref-two-own",
            "TG Callers::RefTwoTables",
            "TG Rec",
            "not-yes",
        ),
        (
            "ref-two-trigger",
            "TG Callers::RefTwoTables",
            "TG W Trig",
            "not-yes",
        ),
        // One `#if` arm exits on a temporary record, the other does not: the
        // dispatch stays.
        ("if-arms", "TG Callers::InsertTemp", "TG W Arm", "not-no"),
        (
            "if-arms-else",
            "TG Callers::InsertTemp",
            "TG W Arm2",
            "not-no",
        ),
    ];
    let mut wrong = Vec::new();
    for (tag, routine, table, want) in cases {
        let got = prove_answer(w, routine, &format!("writes-table:{table}"));
        let ok = match *want {
            "not-no" => got != "no",
            "not-yes" => got != "yes",
            _ => got == *want,
        };
        if !ok {
            wrong.push(format!("{tag}: {got} (want {want})"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Paths: the reverse walk from the guarded subscriber reaches the codeunit
/// whose `OnRun` inserts a persistent record, never the one whose `OnRun`
/// inserts a temporary record (both raise the same publisher).
///
/// DISCRIMINATION (2026-11-07): the walk crossing a raising site whatever
/// its guard (`reverse::walk_multi`) fails at "TG Run Temp"; the site fact's
/// `record_temporary` read as false (`full.rs`) fails there too; the page's
/// `SourceTableTemporary` read as false at the site fails at "TG Temp Page";
/// the lifecycle site's guard bits left 0 (`project.rs`) fails at "TG Open
/// Temp Page"; the union over `#if` arms (`facts.rs`) fails at the arm loop.
#[test]
fn impact_entries_skip_a_raising_site_the_subscriber_cannot_run_from() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let v = alsem_json(&[
        "impact",
        "--routine=TG Subscribers::AfterInsertTemp",
        "--no-tests",
        "--format=json",
        "--",
        w,
    ]);
    let text = v.to_string();
    assert!(
        text.contains("TG Run Persistent"),
        "the persistent raiser is an entry: {text}"
    );
    assert!(
        !text.contains("TG Run Temp"),
        "the temporary raiser is no entry of the guarded subscriber: {text}"
    );
    // A page's implicit `Rec` is temporary under `SourceTableTemporary`: its
    // action's `Rec.Insert()`, and a user's insert in the page (the page
    // object's lifecycle into its source table's events, reached by a run of
    // the page), raise the event on a temporary record.
    assert!(text.contains("TG Page\\\""), "{text}");
    assert!(text.contains("TG Open Page"), "{text}");
    assert!(!text.contains("TG Temp Page"), "{text}");
    assert!(!text.contains("TG Open Temp Page"), "{text}");
    // A subscriber whose guard also calls, and one whose `#if` arms disagree,
    // are reached from both.
    for sub in ["BeforeInsertCall", "AfterInsertArm", "AfterInsertArmElse"] {
        let v = alsem_json(&[
            "impact",
            &format!("--routine=TG Subscribers::{sub}"),
            "--no-tests",
            "--format=json",
            "--",
            w,
        ]);
        let text = v.to_string();
        assert!(
            text.contains("TG Run Persistent") && text.contains("TG Run Temp"),
            "{sub}: {text}"
        );
    }
}
