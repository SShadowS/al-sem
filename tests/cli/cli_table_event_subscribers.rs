//! F1 (spec `docs/superpowers/specs/2026-11-06-db-event-subscribers.md`): a
//! record operation raises its table's platform events
//! (`OnBefore/OnAfter{Insert,Modify,Delete,Rename}Event`, and
//! `OnBefore/OnAfterValidateEvent` for the validated field) whatever its
//! RunTrigger, and on a temporary record too, so the subscribers' effects are
//! the operation's effects. Each subscriber below writes a table of its own;
//! `alsem prove <caller> writes-table:<that table>` must say `yes` exactly when
//! the caller's operation raises the subscriber's event.
//!
//! Before the fix every answer here was `no` with complete coverage: the cone
//! never followed a table event (c11 of the 2026-11-06 output validation).
//!
//! DISCRIMINATION (2026-11-06, each break made, run, reverted, run green):
//! making `resolve_table_events` return no route fails every `yes` case and
//! the witness test; clearing the table-event routes when
//! `rule.run_trigger == Some(false)` (the RunTrigger site rule) fails
//! `modify-false`, `modify-temp`, `insert-default` (a missing RunTrigger reads
//! as `false`), `delete-false` and `modifyall-false`; dropping the field filter
//! (`Converter::subscribes_to_element`) fails `validate-a-not-b`; making
//! `summary_runner::table_event_param_temp_state` answer `Unknown` fails
//! `twin-temp-rec`.

use std::process::Command;

const SOURCE: &str = r#"table 50600 "TE Rec"
{
    fields
    {
        field(1; Code; Code[20]) { }
        field(2; A; Text[30]) { trigger OnValidate() begin end; }
        field(3; B; Text[30]) { }
    }
    keys { key(PK; Code) { } }
    trigger OnModify()
    begin
    end;
}
table 50601 "TE Quiet"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50602 "TE Twin"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50610 "W Modify" { fields { field(1; Id; Integer) { } } }
table 50611 "W Insert" { fields { field(1; Id; Integer) { } } }
table 50612 "W Delete" { fields { field(1; Id; Integer) { } } }
table 50613 "W Rename" { fields { field(1; Id; Integer) { } } }
table 50614 "W ValA" { fields { field(1; Id; Integer) { } } }
table 50615 "W ValB" { fields { field(1; Id; Integer) { } } }
codeunit 50620 "TE Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnAfterModifyEvent', '', false, false)]
    local procedure AfterModify(var Rec: Record "TE Rec"; var xRec: Record "TE Rec"; RunTrigger: Boolean)
    var
        W: Record "W Modify";
    begin
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BeforeInsert(var Rec: Record "TE Rec"; RunTrigger: Boolean)
    var
        W: Record "W Insert";
    begin
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnAfterDeleteEvent', '', false, false)]
    local procedure AfterDelete(var Rec: Record "TE Rec"; RunTrigger: Boolean)
    var
        W: Record "W Delete";
    begin
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnAfterRenameEvent', '', false, false)]
    local procedure AfterRename(var Rec: Record "TE Rec"; var xRec: Record "TE Rec"; RunTrigger: Boolean)
    var
        W: Record "W Rename";
    begin
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnAfterValidateEvent', 'A', false, false)]
    local procedure AfterValidateA(var Rec: Record "TE Rec"; var xRec: Record "TE Rec"; CurrFieldNo: Integer)
    var
        W: Record "W ValA";
    begin
        W.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TE Rec", 'OnAfterValidateEvent', 'B', false, false)]
    local procedure AfterValidateB(var Rec: Record "TE Rec"; var xRec: Record "TE Rec"; CurrFieldNo: Integer)
    var
        W: Record "W ValB";
    begin
        W.Insert();
    end;

    // Writes the event's own record: as temporary as the raising operation's.
    [EventSubscriber(ObjectType::Table, Database::"TE Twin", 'OnBeforeDeleteEvent', '', false, false)]
    local procedure BeforeDeleteTwin(var Rec: Record "TE Twin"; RunTrigger: Boolean)
    begin
        Rec.Modify();
    end;
}
codeunit 50630 "TE Callers"
{
    procedure ModifyFalse()
    var
        R: Record "TE Rec";
    begin
        R.Get('X');
        R.Modify(false);
    end;

    procedure ModifyTemp()
    var
        R: Record "TE Rec" temporary;
    begin
        R.Modify(false);
    end;

    procedure InsertDefault()
    var
        R: Record "TE Rec";
    begin
        R.Insert();
    end;

    procedure DeleteFalse()
    var
        R: Record "TE Rec";
    begin
        R.Delete(false);
    end;

    procedure RenameIt()
    var
        R: Record "TE Rec";
    begin
        R.Rename('Y');
    end;

    procedure ValidateA()
    var
        R: Record "TE Rec";
    begin
        R.Validate(A, 'x');
    end;

    procedure ModifyAllFalse()
    var
        R: Record "TE Rec";
    begin
        R.ModifyAll(B, 'x', false);
    end;

    procedure QuietModify()
    var
        Q: Record "TE Quiet";
    begin
        Q.Modify(false);
    end;

    procedure ReadOnly()
    var
        R: Record "TE Rec";
    begin
        R.Get('X');
    end;

    procedure ModifyTrue()
    var
        R: Record "TE Rec";
    begin
        R.Get('X');
        R.Modify(true);
    end;

    procedure TwinDeleteTemp()
    var
        T: Record "TE Twin" temporary;
    begin
        T.Delete(false);
    end;
}
"#;

fn workspace(root: &std::path::Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"36363636-0000-4000-8000-000000000036","name":"TE","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    ws
}

/// `alsem prove`'s answer (`yes` / `no` / …) for `routine` and `question`.
fn prove_answer(ws: &str, routine: &str, question: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove", ws, routine, question, "--format", "json"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{routine} {question}: not JSON ({e}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    v["payload"]["result"]["answer"]
        .as_str()
        .unwrap_or_else(|| panic!("{routine} {question}: no answer: {v}"))
        .to_string()
}

#[test]
fn record_operations_reach_their_table_event_subscribers() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let cases: &[(&str, &str, &str, &str)] = &[
        ("modify-false", "TE Callers::ModifyFalse", "W Modify", "yes"),
        // A temporary record raises the events too; the subscriber's own write
        // is to a physical table.
        ("modify-temp", "TE Callers::ModifyTemp", "W Modify", "yes"),
        (
            "insert-default",
            "TE Callers::InsertDefault",
            "W Insert",
            "yes",
        ),
        ("delete-false", "TE Callers::DeleteFalse", "W Delete", "yes"),
        ("rename", "TE Callers::RenameIt", "W Rename", "yes"),
        ("validate-a", "TE Callers::ValidateA", "W ValA", "yes"),
        ("validate-a-not-b", "TE Callers::ValidateA", "W ValB", "no"),
        (
            "modifyall-false",
            "TE Callers::ModifyAllFalse",
            "W Modify",
            "yes",
        ),
        // Another event of the same table is not raised by Modify.
        (
            "modify-not-delete",
            "TE Callers::ModifyFalse",
            "W Delete",
            "no",
        ),
        // A table nothing subscribes to raises nothing anyone handles.
        ("quiet-table", "TE Callers::QuietModify", "W Modify", "no"),
        ("read-only", "TE Callers::ReadOnly", "W Modify", "no"),
        // The subscriber's `Rec.Modify()` works on the raising operation's
        // record, here a temporary one: no physical write.
        (
            "twin-temp-rec",
            "TE Callers::TwinDeleteTemp",
            "TE Twin",
            "no",
        ),
        // Beside its table trigger, Modify(true) still reaches the subscriber.
        ("modify-true", "TE Callers::ModifyTrue", "W Modify", "yes"),
    ];
    let mut wrong = Vec::new();
    for (tag, routine, table, want) in cases {
        let got = prove_answer(w, routine, &format!("writes-table:{table}"));
        if got != *want {
            wrong.push(format!("{tag}: {got} (want {want})"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The witness of a subscriber's write runs through the table event, beside
/// the table trigger the same operation fires (`Modify(true)` has both at one
/// operation): an `[event]` hop to the subscriber.
///
/// LIMIT (2026-11-06): making `digest::fact_first_hop` drop the event (an
/// `Op` first hop, which the op's trigger edge matches too) leaves this test
/// green, also with the trigger writing the same table: the witness search
/// ends at the fact's own operation, so the trigger seed never yields a path.
/// `FirstHop::OpEvent` only narrows the seeds; no test pins it.
#[test]
fn a_table_event_witness_goes_through_the_subscriber() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            w,
            "TE Callers::ModifyTrue",
            "writes-table:W Modify",
            "--format",
            "human",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let paths: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("path:"))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.contains("[event] TE Subscribers::AfterModify")),
        "{stdout}"
    );
}
