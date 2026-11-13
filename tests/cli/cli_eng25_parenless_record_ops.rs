//! ENG-25: a record operation written without parentheses in expression
//! position (`if R.FindFirst then`, `exit(R.IsEmpty)`) made no record
//! operation. Probe: docs/superpowers/notes/2026-11-12-eng25-parenless-record-ops-probe.md.

use std::process::Command;

const SOURCE: &str = r#"
table 52700 "E25 Row"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; Count2; Integer) { }
    }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
    procedure BareFind(): Boolean
    begin
        exit(FindFirst);
    end;
    procedure ParamCount(Count: Integer): Integer
    begin
        exit(Count);
    end;
}
table 52704 "E25 Quiet"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
}
table 52705 "E25 Ext Base"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
}
tableextension 52706 "E25 Ext A" extends "E25 Ext Base"
{
    fields
    {
        field(50; IsEmpty; Boolean) { }
    }
}
tableextension 52707 "E25 Ext B" extends "E25 Ext Base"
{
    fields
    {
        field(51; IsEmpty; Integer) { }
    }
}
table 52708 "E25 Prot Base"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
    protected var
        Count: Integer;
}
tableextension 52709 "E25 Prot Ext" extends "E25 Prot Base"
{
    procedure ProtCount(): Integer
    begin
        exit(Count);
    end;
}
table 52702 "E25 Fieldy"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; Count; Integer) { }
    }
    keys { key(PK; K) { } }
}
codeunit 52701 "E25 Ops"
{
    procedure IfFind()
    var
        R: Record "E25 Row";
    begin
        if R.FindFirst then;
    end;
    procedure NotEmpty(): Boolean
    var
        R: Record "E25 Row";
    begin
        exit(not R.IsEmpty);
    end;
    procedure WhileNext()
    var
        R: Record "E25 Row";
    begin
        while R.Next <> 0 do;
    end;
    procedure ModifyValue(): Boolean
    var
        R: Record "E25 Row";
    begin
        exit(R.Modify);
    end;
    procedure ArrFind()
    var
        Recs: array[2] of Record "E25 Row";
    begin
        if Recs[1].FindFirst then;
    end;
    procedure WithFind()
    var
        R: Record "E25 Row";
    begin
        with R do
            if FindFirst then;
    end;
    procedure StmtFind()
    var
        R: Record "E25 Row";
    begin
        R.FindFirst;
    end;
    procedure FieldCount(): Integer
    var
        F: Record "E25 Fieldy";
    begin
        exit(F.Count);
    end;
    procedure RefCount(): Integer
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"E25 Fieldy");
        exit(RR.Count);
    end;
    procedure QuietModify(): Boolean
    var
        Q: Record "E25 Quiet";
    begin
        exit(Q.Modify);
    end;
    procedure AmbigRead(): Boolean
    var
        X: Record "E25 Ext Base";
    begin
        exit(X.IsEmpty);
    end;
}
codeunit 52703 "E25 Listener"
{
    [EventSubscriber(ObjectType::Table, Database::"E25 Row", 'OnAfterModifyEvent', '', false, false)]
    local procedure OnAfterModifyRow(var Rec: Record "E25 Row"; var xRec: Record "E25 Row"; RunTrigger: Boolean)
    begin
        Commit();
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e25e25e0-0000-4000-8000-0000000000aa","name":"E25","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    t
}

fn answer(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_default()
        .to_string()
}

fn touches(ws: &str, table: &str, from: &str) -> String {
    answer(&[
        "query",
        "touches",
        "--table",
        table,
        "--from",
        from,
        "--direction",
        "down",
        ws,
    ])
}

/// A parens-less record read in expression position reads its table: a
/// member, the implicit `Rec` in a table, `not R.IsEmpty`, an array element,
/// a `with` frame. A `Count` that is a FIELD of its table stays a field read.
/// Breaks (2026-11-12): keep no parens-less record operation in `full.rs`'s
/// filter -> `IfFind` answers `no`; ignore `record_op_field_named` ->
/// `FieldCount` answers `yes`. Each fails; restored, it passes. (2026-11-13)
/// `WhileNext` has no other read, so the first break makes it `no` too.
#[test]
fn a_parenless_record_read_reads_its_table() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for from in [
        "\"E25 Row\".BareFind",
        "\"E25 Ops\".IfFind",
        "\"E25 Ops\".NotEmpty",
        "\"E25 Ops\".ArrFind",
        "\"E25 Ops\".WithFind",
        "\"E25 Ops\".StmtFind",
        "\"E25 Ops\".WhileNext",
    ] {
        let a = touches(ws, "E25 Row", from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
    let a = touches(ws, "E25 Fieldy", "\"E25 Ops\".FieldCount");
    assert!(a.starts_with("answer: down: no"), "FieldCount: {a}");
}

/// What is NOT a parens-less record operation, and one that is despite a
/// field of its name: a bare parameter `Count` in a table reads the parameter;
/// a field declared twice (two extensions, two types) is still a field; a
/// RecordRef's `Count` is the method even when its table has a field `Count`.
/// Breaks (2026-11-13): drop the implicit-`Rec` shadow skip in
/// `resolve_file_obligations` -> `ParamCount` answers `yes`; go back to the
/// unique-only `field_in_table` for the field veto -> `AmbigRead` answers
/// `yes`; drop the `record_receiver` gate on the veto -> `RefCount` answers
/// `no`; (round 2) leave the base object's `protected var` names out of the
/// shadow set -> `ProtCount` answers `yes`. Each fails; restored, it passes.
/// The field rule is the compiler's: alc 18.0.41 accepts `exit(F.Count)` as a
/// Boolean and `exit(F.IsEmpty)` as a Text where those are fields (2026-11-13).
#[test]
fn a_shadowing_name_or_field_is_no_parenless_record_operation() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let a = touches(ws, "E25 Row", "\"E25 Row\".ParamCount");
    assert!(a.starts_with("answer: down: no"), "ParamCount: {a}");
    let a = touches(ws, "E25 Ext Base", "\"E25 Ops\".AmbigRead");
    assert!(a.starts_with("answer: down: no"), "AmbigRead: {a}");
    let a = touches(ws, "E25 Fieldy", "\"E25 Ops\".RefCount");
    assert!(a.starts_with("answer: down: yes"), "RefCount: {a}");
    let a = touches(ws, "E25 Prot Base", "\"E25 Prot Ext\".ProtCount");
    assert!(a.starts_with("answer: down: no"), "ProtCount: {a}");
}

fn may_commit(ws: &str, routine: &str) -> String {
    answer(&["prove", ws, routine, "may-commit", "--format", "human"])
}

/// A parens-less `Modify` runs no trigger (RunTrigger false) but raises the
/// table events: "E25 Row"'s `OnAfterModifyEvent` subscriber commits. "E25
/// Quiet" has the same committing `OnModify` and no subscriber, so its
/// parens-less `Modify` cannot commit.
/// Breaks (2026-11-12): keep no parens-less record operation ->
/// `ModifyValue` answers `no`. (2026-11-13): make a parens-less `Modify`
/// default to RunTrigger true (`applicability.rs`) -> `QuietModify` answers
/// `yes`. Each fails; restored, it passes.
#[test]
fn a_parenless_modify_raises_its_table_events() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let a = may_commit(ws, "\"E25 Ops\"::ModifyValue");
    assert!(a.starts_with("answer: yes"), "ModifyValue: {a}");
    let a = may_commit(ws, "\"E25 Ops\"::QuietModify");
    assert!(a.starts_with("answer: no"), "QuietModify: {a}");
}
