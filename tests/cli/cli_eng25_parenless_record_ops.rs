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
        if R.FindSet() then
            repeat
            until R.Next = 0;
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
/// `FieldCount` answers `yes`. Each fails; restored, it passes.
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
    ] {
        let a = touches(ws, "E25 Row", from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
    let a = touches(ws, "E25 Fieldy", "\"E25 Ops\".FieldCount");
    assert!(a.starts_with("answer: down: no"), "FieldCount: {a}");
}

/// A parens-less `Modify` runs no trigger (RunTrigger false) but raises the
/// table events: its `OnAfterModifyEvent` subscriber commits.
/// Break (2026-11-12): keep no parens-less record operation -> `no`.
#[test]
fn a_parenless_modify_raises_its_table_events() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let a = answer(&[
        "prove",
        ws,
        "\"E25 Ops\"::ModifyValue",
        "may-commit",
        "--format",
        "human",
    ]);
    assert!(a.starts_with("answer: yes"), "{a}");
}
