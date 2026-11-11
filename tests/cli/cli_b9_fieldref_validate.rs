//! Engine limits B9 (`docs/engine-limits.md` §1) and the RecordRef case ENG-06
//! found with it.
//!
//! B9: `FR := RR.Field(2); FR.Validate(1);` raises the table's validate events
//! and runs the field's `OnValidate`, as `R.Validate(Amt, 1)` does. The engine
//! read `FieldRef.Validate` as a plain built-in call, so a subscriber of the
//! field's `OnAfterValidateEvent` that commits was not in the cone and `prove`
//! answered a definite `may-commit: no`.
//!
//! The RecordRef case: a record operation that can fire a table trigger, on a
//! record whose table is not known here (`RR.Modify(true)` with `RR` a
//! `RecordRef` parameter), may run any table's triggers and table events. The
//! engine made no edge and no gap for it, so `prove` answered `no (every
//! candidate decided)`. Both now answer `unknown` when the table or field is
//! not known, and follow the field when it is.
//!
//! Each case is pinned through the `prove` answer the CLI prints.

use std::process::Command;

use serde_json::Value;

const SOURCE: &str = r#"table 50900 "B9 T"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amt; Decimal) { }
        field(3; Other; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

// A table whose field 2 commits in its own OnValidate trigger, and whose
// OnModify commits: the one modify trigger loaded.
table 50903 "B9 Trig"
{
    trigger OnModify()
    begin
        Commit();
    end;

    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amt; Decimal)
        {
            trigger OnValidate()
            begin
                Commit();
            end;
        }
        field(3; Other; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

codeunit 50901 "B9 Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"B9 T", 'OnAfterValidateEvent', 'Amt', false, false)]
    local procedure H(var Rec: Record "B9 T"; var xRec: Record "B9 T"; CurrFieldNo: Integer)
    begin
        Commit();
    end;
}

codeunit 50902 "B9 Probe"
{
    // The control: a record Validate raises the field's validate events.
    procedure RecValidate()
    var
        R: Record "B9 T";
    begin
        R.Validate(Amt, 1);
    end;

    procedure ByNumber()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(2);
        FR.Validate(1);
    end;

    procedure ByName()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field('Amt');
        FR.Validate(1);
    end;

    procedure ByFieldNo()
    var
        RR: RecordRef;
        R: Record "B9 T";
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(R.FieldNo(Amt));
        FR.Validate(1);
    end;

    procedure InPlace()
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"B9 T");
        RR.Field(2).Validate(1);
    end;

    // Field 3 has no subscriber: a decided FieldRef reaches only its field's.
    procedure OtherField()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(3);
        FR.Validate(1);
    end;

    // The field's own OnValidate trigger runs.
    procedure TriggerField()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 Trig");
        FR := RR.Field(2);
        FR.Validate(1);
    end;

    // Another field's OnValidate trigger does not.
    procedure TriggerOtherField()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 Trig");
        FR := RR.Field(3);
        FR.Validate(1);
    end;

    // The RecordRef comes from the caller: its table is not known here.
    procedure UnknownTable(var RR: RecordRef)
    var
        FR: FieldRef;
    begin
        FR := RR.Field(2);
        FR.Validate(1);
    end;

    // Passed to a call, the FieldRef may be rebound there.
    procedure PassedOn()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(3);
        Touch(FR);
        FR.Validate(1);
    end;

    local procedure Touch(var F: FieldRef)
    begin
    end;

    // Two bindings, two fields: either may be validated.
    procedure TwoFields(b: Boolean)
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        if b then
            FR := RR.Field(2)
        else
            FR := RR.Field(3);
        FR.Validate(1);
    end;

    // ENG-06's RecordRef case: an operation that may fire any table's trigger.
    procedure ModifyUnknownTable(var RR: RecordRef)
    begin
        RR.Modify(true);
    end;

    procedure CallsModifyUnknownTable(var RR: RecordRef)
    begin
        ModifyUnknownTable(RR);
    end;

    // No loaded table has an insert trigger or insert-event subscriber.
    procedure InsertUnknownTable(var RR: RecordRef)
    begin
        RR.Insert(true);
    end;

    // A record of a table the program does not hold: its own OnInsert is code
    // the engine cannot see.
    procedure InsertNotLoaded()
    var
        M: Record "B9 Missing";
    begin
        M.Insert(true);
    end;

    // No RunTrigger: no trigger of it runs, and nothing subscribes to insert.
    procedure InsertNotLoadedNoTrigger()
    var
        M: Record "B9 Missing";
    begin
        M.Insert();
    end;

    // No RunTrigger: the table events only, and nothing subscribes to modify.
    procedure ModifyUnknownTableNoTrigger(var RR: RecordRef)
    begin
        RR.Modify();
    end;

    // The control: the same operation on a RecordRef of a known table.
    procedure ModifyKnownTable()
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"B9 T");
        RR.Modify(true);
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b9b9b9b9-0000-4000-8000-0000000000b9","name":"B9","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/b9.al"), SOURCE).unwrap();
    t
}

fn prove(ws: &str, routine: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            ws,
            &format!("B9 Probe::{routine}"),
            "may-commit",
            "--single-app",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    v["payload"]["result"].clone()
}

/// The 1-based line of the first line holding `text` after the line holding
/// `after`.
fn line_of(text: &str, after: &str) -> u64 {
    let start = SOURCE.lines().position(|l| l.contains(after)).expect(after);
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.contains(text))
        .expect(text);
    (start + at + 1) as u64
}

/// The `coverage-incomplete` blocker names the open record operation.
fn assert_open_record_gap(r: &Value) {
    assert_eq!(r["answer"], "unknown", "{r:#}");
    let details: Vec<&str> = r["blockedBy"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["detail"].as_str())
        .collect();
    assert!(
        details.iter().any(
            |d| d.contains("a record operation or FieldRef.Validate on a table not known here")
        ),
        "{r:#}"
    );
}

/// B9: a FieldRef bound to the field, by number, by name, by `FieldNo` or
/// written in place, reaches the subscriber of that field's validate event, as
/// the record Validate does. Before the fix every FieldRef form was `no`.
#[test]
fn a_fieldref_validate_raises_its_fields_validate_event() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let commit = line_of("Commit();", "local procedure H(");
    for routine in ["RecValidate", "ByNumber", "ByName", "ByFieldNo", "InPlace"] {
        let r = prove(&ws, routine);
        assert_eq!(r["answer"], "yes", "{routine}: {r:#}");
        assert_eq!(
            r["evidence"][0]["evidence"]["line"], commit,
            "{routine}: {r:#}"
        );
    }
}

/// A decided FieldRef reaches only its own field: field 3 has no subscriber
/// and no trigger, so the answer stays a definite `no`.
#[test]
fn a_fieldref_validate_of_another_field_stays_no() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "OtherField");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// The field's own `OnValidate` trigger runs; another field's does not (the
/// resolver keeps only the validated field's trigger, as `TriggerSiteRule`
/// does for `Rec.Validate(F, …)`).
#[test]
fn a_fieldref_validate_runs_only_its_fields_trigger() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "TriggerField");
    assert_eq!(r["answer"], "yes", "{r:#}");
    assert_eq!(
        r["evidence"][0]["evidence"]["line"],
        line_of("Commit();", "trigger OnValidate()"),
        "{r:#}"
    );
    let r = prove(&ws, "TriggerOtherField");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// A FieldRef whose table is not known here, one passed to a call (which may
/// rebind it), and one whose bindings name two fields may validate any of
/// several fields: `unknown`, with the gap named, never `no`. Before the fix
/// the first was `no (every candidate decided)`.
#[test]
fn an_undecided_fieldref_validate_is_unknown() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for routine in ["UnknownTable", "PassedOn", "TwoFields"] {
        let r = prove(&ws, routine);
        assert_open_record_gap(&r);
    }
}

/// ENG-06's RecordRef case (B10): `RR.Modify(true)` on a RecordRef parameter
/// may run any table's `OnModify`, and a loaded table's (`B9 Trig`) commits, so
/// it is `unknown`, and so is a caller's answer. Before the fix both were
/// `no (every candidate decided)`. The same operation on a RecordRef of a known
/// table with no trigger (`B9 T`) stays `no`.
#[test]
fn a_trigger_op_on_a_table_not_known_is_unknown() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for routine in ["ModifyUnknownTable", "CallsModifyUnknownTable"] {
        let r = prove(&ws, routine);
        assert_open_record_gap(&r);
    }
    let r = prove(&ws, "ModifyKnownTable");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// A record of a named table the program does not hold: its own `OnInsert`
/// may commit, whatever the loaded tables have (none has an insert trigger
/// here), so `Insert(true)` is `unknown`. Without RunTrigger no trigger runs
/// and nothing subscribes to insert events: `no`.
#[test]
fn a_trigger_op_on_a_table_not_loaded_is_unknown() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove(&ws, "InsertNotLoaded"));
    let r = prove(&ws, "InsertNotLoadedNoTrigger");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// An operation on a table not known runs nothing when no loaded table has a
/// trigger of its family, or a subscriber to its table events (`RR.Insert(true)`:
/// none loaded), or when it fires no trigger and nothing subscribes to its
/// events (`RR.Modify()`): those stay `no` within the loaded apps the answer
/// states, as before ENG-06.
#[test]
fn an_op_on_a_table_not_known_that_nothing_loaded_answers_stays_no() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for routine in ["InsertUnknownTable", "ModifyUnknownTableNoTrigger"] {
        let r = prove(&ws, routine);
        assert_eq!(r["answer"], "no", "{routine}: {r:#}");
    }
}
