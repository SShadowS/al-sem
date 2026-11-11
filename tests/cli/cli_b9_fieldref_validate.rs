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

// Another table whose field numbers differ: its Amt is field 3.
table 50904 "B9 Alt"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; X; Decimal) { }
        field(3; Amt; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

// F is field 2 in one build and field 3 in the other.
table 50907 "B9 FN"
{
    fields
    {
        field(1; "No."; Code[20]) { }
#if CLEAN99
        field(2; F; Decimal) { }
#else
        field(3; F; Decimal) { }
#endif
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

// Field 4 is declared under two names, one per build; the subscriber is Q's.
table 50905 "B9 Arms"
{
    fields
    {
        field(1; "No."; Code[20]) { }
#if CLEAN99
        field(4; P; Decimal) { }
#else
        field(4; Q; Decimal) { }
#endif
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

codeunit 50906 "B9 Arms Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"B9 Arms", 'OnAfterValidateEvent', 'Q', false, false)]
    local procedure HQ(var Rec: Record "B9 Arms"; var xRec: Record "B9 Arms"; CurrFieldNo: Integer)
    begin
        Commit();
    end;
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

    // `A.FieldNo(Amt)` is 3, Amt's number in B9 Alt: on B9 T that is Other,
    // which nothing subscribes to.
    procedure FieldNoOfAnotherTable()
    var
        RR: RecordRef;
        A: Record "B9 Alt";
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(A.FieldNo(Amt));
        FR.Validate(1);
    end;

    // `A.FieldNo(X)` is 2: on B9 T that is Amt, whose subscriber commits.
    procedure FieldNoOfAnotherTableHit()
    var
        RR: RecordRef;
        A: Record "B9 Alt";
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(A.FieldNo(X));
        FR.Validate(1);
    end;

    // `FN.FieldNo(F)` is 2 in one build (B9 T's Amt: commits) and 3 in the
    // other (Other: nothing).
    procedure FieldNoTwoNumbers()
    var
        RR: RecordRef;
        FN: Record "B9 FN";
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 T");
        FR := RR.Field(FN.FieldNo(F));
        FR.Validate(1);
    end;

    // Field 4 is P in one build and Q in the other: either may be validated.
    procedure ArmsField()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 Arms");
        FR := RR.Field(4);
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

    // A RecordRef opened on a table the program does not hold: the same.
    procedure RecRefInsertNotLoaded()
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"B9 Missing");
        RR.Insert(true);
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

/// A workspace whose only validate code is a table extension's field
/// `OnAfterValidate` trigger: no `OnValidate`, no validate-event subscriber.
const EXT_SOURCE: &str = r#"table 50910 "B9 Base"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amt; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

tableextension 50911 "B9 Base Ext" extends "B9 Base"
{
    fields
    {
        modify(Amt)
        {
            trigger OnAfterValidate()
            begin
                Commit();
            end;
        }
    }
}

codeunit 50912 "B9 Probe"
{
    // The RecordRef comes from the caller: any table's field may be validated,
    // and B9 Base's extension commits after validating Amt.
    procedure UnknownTable(var RR: RecordRef)
    var
        FR: FieldRef;
    begin
        FR := RR.Field(2);
        FR.Validate(1);
    end;
}
"#;

/// A workspace with no validate code at all: no `OnValidate`, no extension,
/// no subscriber.
const BARE_SOURCE: &str = r#"codeunit 50920 "B9 Probe"
{
    // A RecordRef opened on a table the program does not hold: that table's
    // fields' validate triggers are code the engine cannot see.
    procedure FieldRefNotLoaded()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"B9 Missing");
        FR := RR.Field(2);
        FR.Validate(1);
    end;

    // Two sources: a caller's RecordRef (any table) and one opened on a table
    // not loaded. The second's unseen triggers keep it open, whatever the
    // first says.
    procedure FieldRefMixedSources(var A: RecordRef; b: Boolean)
    var
        Z: RecordRef;
        FR: FieldRef;
    begin
        Z.Open(Database::"B9 Missing");
        if b then
            FR := A.Field(2)
        else
            FR := Z.Field(2);
        FR.Validate(1);
    end;

    // A RecordRef from the caller: any table, and no loaded table has
    // validate code, so nothing loaded runs.
    procedure FieldRefAnyTable(var RR: RecordRef)
    var
        FR: FieldRef;
    begin
        FR := RR.Field(2);
        FR.Validate(1);
    end;
}
"#;

fn bare_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b9b9b9b9-0000-4000-8000-0000000000bb","name":"B9 Bare","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/b9bare.al"), BARE_SOURCE).unwrap();
    t
}

fn ext_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b9b9b9b9-0000-4000-8000-0000000000ba","name":"B9 Ext","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/b9ext.al"), EXT_SOURCE).unwrap();
    t
}

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
    // Review finding: a RecordRef opened on a literal table not loaded is the
    // same case, not "any table" (no insert trigger is loaded anywhere).
    assert_open_record_gap(&prove(&ws, "RecRefInsertNotLoaded"));
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

/// Review finding (ENG-06 panel): `RR.Field(A.FieldNo(F))` is field `F`'s
/// NUMBER in `A`'s table, not the name `F` on the RecordRef's table. Amt is 3
/// in B9 Alt, which on B9 T is Other (no subscriber): `no`; X is 2 in B9 Alt,
/// which on B9 T is Amt (its subscriber commits): `yes`. Read as the name, the
/// first was a wrong `yes`.
#[test]
fn a_fieldno_of_another_table_names_that_number() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "FieldNoOfAnotherTable");
    assert_eq!(r["answer"], "no", "{r:#}");
    let r = prove(&ws, "FieldNoOfAnotherTableHit");
    assert_eq!(r["answer"], "yes", "{r:#}");
}

/// Review finding: a field number two `#if` builds declare under two names is
/// either field. The lookup by number declines it, so the FieldRef is
/// undecided and `unknown` (the Q build's subscriber commits); taking the
/// first declaration (P, no subscriber) was a wrong `no`.
#[test]
fn a_field_number_declared_twice_is_undecided() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove(&ws, "ArmsField"));
}

/// Review finding: a FieldRef on a RecordRef bound only to a table not loaded
/// is that table's unseen triggers, not "any table": no validate code is
/// loaded here, so the any-table rule would keep `no`, as it does (and should)
/// for a RecordRef from the caller.
#[test]
fn a_fieldref_on_a_table_not_loaded_is_unknown() {
    let t = bare_workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove(&ws, "FieldRefNotLoaded"));
    // Round-2 review: every source counts, not the first that fails.
    assert_open_record_gap(&prove(&ws, "FieldRefMixedSources"));
    let r = prove(&ws, "FieldRefAnyTable");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// Review finding: a table extension's `OnBeforeValidate`/`OnAfterValidate`
/// is loaded validate code too. With it the only one, an undecided FieldRef
/// is `unknown`, not a `no`.
#[test]
fn an_extension_validate_trigger_keeps_an_undecided_fieldref_open() {
    let t = ext_workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove(&ws, "UnknownTable"));
}

/// The scope label a `no` carries when it rests on the loaded tables only.
const LOADED_TABLES_ONLY: &str =
    "the triggers of a record operation on a table not known, of the loaded tables only";

/// The answer's assumption sentences (`scope.assumptions`).
fn assumptions(r: &Value) -> Vec<String> {
    r["scope"]["assumptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str().map(str::to_string))
        .collect()
}

/// Owner answer q-20261111T044238-cead8f76, condition 1: a `no` kept because no
/// loaded table can run anything for an operation on a table not known says so
/// in its assumptions ("no within the loaded apps"); a `no` with no such
/// operation in its cone does not.
#[test]
fn a_no_resting_on_the_loaded_tables_says_so() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "InsertUnknownTable");
    assert_eq!(r["answer"], "no", "{r:#}");
    assert!(
        assumptions(&r)
            .iter()
            .any(|a| a.contains(LOADED_TABLES_ONLY)),
        "{r:#}"
    );
    let r = prove(&ws, "OtherField");
    assert_eq!(r["answer"], "no", "{r:#}");
    assert!(
        !assumptions(&r)
            .iter()
            .any(|a| a.contains(LOADED_TABLES_ONLY)),
        "a decided operation states no such limit: {r:#}"
    );
    // Round-2 review: an op that fires no trigger (`Modify()`) rests on the
    // table events only; it states no trigger limit.
    let r = prove(&ws, "ModifyUnknownTableNoTrigger");
    assert_eq!(r["answer"], "no", "{r:#}");
    assert!(
        !assumptions(&r)
            .iter()
            .any(|a| a.contains(LOADED_TABLES_ONLY)),
        "no trigger fires there: {r:#}"
    );
}

/// Round-2 review: `B.FieldNo(F)` where `B` declares `F` under two numbers, one
/// per build, may name either field: undecided, `unknown` (field 2's
/// subscriber commits). Taking one build's number was a definite answer.
#[test]
fn a_fieldno_with_two_numbers_is_undecided() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove(&ws, "FieldNoTwoNumbers"));
}

/// A workspace with an any-table insert and nothing loaded to run for it,
/// optionally with a symbol-only dependency that declares a table.
fn sym_workspace(with_dependency: bool) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    let deps = if with_dependency {
        r#"[{"id":"22222222-0000-0000-0000-0000000000b9","name":"B9 Sym","publisher":"t","version":"1.0.0.0"}]"#
    } else {
        "[]"
    };
    std::fs::write(
        t.path().join("app.json"),
        format!(
            r#"{{"id":"b9b9b9b9-0000-4000-8000-0000000000bc","name":"B9 Host","publisher":"t","version":"1.0.0.0","dependencies":{deps}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/b9host.al"),
        "codeunit 50930 \"B9 Probe\"\n{\n    procedure InsertAnyTable(var RR: RecordRef)\n    begin\n        RR.Insert(true);\n    end;\n}\n",
    )
    .unwrap();
    if with_dependency {
        let symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000b9","Name":"B9 Sym","Publisher":"t","Version":"1.0.0.0","Tables":[{"Id":70930,"Name":"B9 Sym T","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}]}"#;
        crate::symbol_app::write_symbol_app(
            &t.path().join(".alpackages/t_B9 Sym_1.0.0.0.app"),
            "22222222-0000-0000-0000-0000000000b9",
            "B9 Sym",
            "1.0.0.0",
            symbols,
        );
    }
    t
}

/// `alsem prove` in the default (cross-app) mode.
fn prove_cross_app(ws: &str, routine: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove", ws, &format!("B9 Probe::{routine}"), "may-commit"])
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

/// Owner answer q-20261111T044238-cead8f76, condition 2: a loaded table whose
/// code is not loaded (a symbol-only dependency's) may have any trigger or
/// subscriber, so an operation that may hold any table is `unknown`, not `no`
/// (missing data is never absence). Without that dependency nothing loaded can
/// run, and the answer is `no` within the loaded apps. Cross-app mode, the
/// default.
#[test]
fn a_symbol_only_table_keeps_an_any_table_op_open() {
    let t = sym_workspace(true);
    let ws = t.path().to_string_lossy().into_owned();
    assert_open_record_gap(&prove_cross_app(&ws, "InsertAnyTable"));
    let t = sym_workspace(false);
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove_cross_app(&ws, "InsertAnyTable");
    assert_eq!(r["answer"], "no", "{r:#}");
    assert!(
        assumptions(&r)
            .iter()
            .any(|a| a.contains(LOADED_TABLES_ONLY)),
        "{r:#}"
    );
}
