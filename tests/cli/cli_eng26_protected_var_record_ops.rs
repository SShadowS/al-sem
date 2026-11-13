//! ENG-26: a base object's `protected var` record used in an extension
//! (`Other.Modify(true)` in a tableextension whose base declares `protected
//! var Other: Record T`) made no record operation, so `query touches` and
//! `prove … may-commit` answered a definite `no`.

use std::process::Command;

const SOURCE: &str = r#"
table 52900 "E26 Target"
{
    fields
    {
        field(1; K; Code[20]) { }
    }
    keys { key(PK; K) { } }
    trigger OnModify()
    begin
        Commit();
    end;
}
table 52905 "E26 Quiet"
{
    fields
    {
        field(1; K; Code[20]) { }
    }
    keys { key(PK; K) { } }
}
table 52901 "E26 Base"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
    protected var
        Other: Record "E26 Target";
        TempOther: Record "E26 Target" temporary;
        RefOther: RecordRef;
}
tableextension 52902 "E26 Ext" extends "E26 Base"
{
    procedure ReadOther(): Boolean
    begin
        exit(Other.Get('x'));
    end;
    procedure WriteOther()
    begin
        Other.Modify(true);
    end;
    procedure BareRead(): Boolean
    begin
        exit(Other.IsEmpty);
    end;
    procedure WriteTemp()
    begin
        TempOther.Modify(true);
    end;
    procedure BareRefRead(): Boolean
    begin
        exit(RefOther.IsEmpty);
    end;
    // A local of the same name shadows the protected var.
    procedure ShadowedWrite()
    var
        Other: Record "E26 Quiet";
    begin
        Other.Modify(true);
    end;
    // So does a NON-record local: this `Other.Modify(true)` is the codeunit's.
    procedure ShadowedByCodeunit()
    var
        Other: Codeunit "E26 Helper";
    begin
        Other.Modify(true);
    end;
}
codeunit 52906 "E26 Helper"
{
    procedure Modify(Flag: Boolean)
    begin
    end;
}
page 52903 "E26 Base Page"
{
    SourceTable = "E26 Base";
    layout { area(Content) { repeater(G) { field(K; Rec.K) { } } } }
    protected var
        PageOther: Record "E26 Target";
}
pageextension 52904 "E26 Page Ext" extends "E26 Base Page"
{
    procedure PageWrite()
    begin
        PageOther.Modify(true);
    end;
}
"#;

fn workspace(source: &str, deps: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        format!(
            r#"{{"id":"e26e26e0-0000-4000-8000-0000000000aa","name":"E26","publisher":"t","version":"1.0.0.0","dependencies":[{deps}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), source).unwrap();
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

/// A base table's and a base page's protected record, read and written from
/// their extensions, touches the variable's own table (the page's source table
/// is another one), and the write runs its trigger. A local of the same name
/// shadows the protected var.
/// Breaks (2026-11-13): leave `inherited_protected_var` out of the call-result
/// path in `full.rs` -> `ReadOther` answers `no`; drop the parens-less arm for
/// a call-result record -> `BareRead` answers `no`. Each fails; restored, it
/// passes. (An explicit shadowing check in `inherited_protected_var` made no
/// difference when broken: a same-named record local is a record operation
/// before this path, so the check was removed; `ShadowedWrite` pins the rule.)
/// A NON-record local of the same name hides it too, through receiver typing's
/// order (Step 2, declared names, before Step 2c, the base's protected vars).
/// Break (2026-11-13): run Step 2c first in `infer_receiver_type` ->
/// `ShadowedByCodeunit` touches "E26 Target". Fails; restored, it passes.
/// Review round 1 (Fable): the declaration's `temporary` (or its absence) is the
/// op's temp state, and a parens-less read of a protected RecordRef answers
/// `unknown`. Breaks: ignore `CallResultRecord::temporary` in
/// `call_result_operation` -> `WriteOther` and `WriteTemp` read "possibly
/// persistent"; drop the `inherited_protected` arm of the parens-less filter
/// -> `BareRefRead` answers `no`. Each fails; restored, it passes.
#[test]
fn a_base_objects_protected_record_is_a_record_operation() {
    let t = workspace(SOURCE, "");
    let ws = t.path().to_str().unwrap();
    for from in [
        "\"E26 Ext\".ReadOther",
        "\"E26 Ext\".WriteOther",
        "\"E26 Ext\".BareRead",
        "\"E26 Page Ext\".PageWrite",
    ] {
        let a = touches(ws, "E26 Target", from);
        assert!(a.starts_with("answer: down: yes"), "{from}: {a}");
    }
    // The page's source table is not what `PageOther` writes.
    let a = touches(ws, "E26 Base", "\"E26 Page Ext\".PageWrite");
    assert!(
        a.starts_with("answer: down: no"),
        "PageWrite on E26 Base: {a}"
    );
    let a = touches(ws, "E26 Target", "\"E26 Ext\".ShadowedWrite");
    assert!(a.starts_with("answer: down: no"), "ShadowedWrite: {a}");
    let a = touches(ws, "E26 Quiet", "\"E26 Ext\".ShadowedWrite");
    assert!(a.starts_with("answer: down: yes"), "ShadowedWrite: {a}");
    let a = touches(ws, "E26 Target", "\"E26 Ext\".ShadowedByCodeunit");
    assert!(a.starts_with("answer: down: no"), "ShadowedByCodeunit: {a}");
    // The declaration says whether the record is temporary.
    let a = touches(ws, "E26 Target", "\"E26 Ext\".WriteOther");
    assert!(a.contains("1 on a persistent record"), "WriteOther: {a}");
    let a = touches(ws, "E26 Target", "\"E26 Ext\".WriteTemp");
    assert!(a.contains("1 on temporary records"), "WriteTemp: {a}");
    // A protected RecordRef, read without parentheses: open, so `unknown`.
    let a = touches(ws, "E26 Target", "\"E26 Ext\".BareRefRead");
    assert!(a.starts_with("answer: down: unknown"), "BareRefRead: {a}");
    let a = answer(&[
        "prove",
        ws,
        "TableExtension \"E26 Ext\"::WriteOther",
        "may-commit",
        "--format",
        "human",
    ]);
    assert!(a.starts_with("answer: yes"), "WriteOther may-commit: {a}");
}

const DEP_GUID: &str = "e26e26e0-0000-4000-8000-0000000000bb";

/// A dependency known only by its symbols: "Dep Base" declares `protected var
/// Other: Record "Dep Target"` (the symbol file's `"Protected": true`).
const DEP_SYMBOLS: &str = r#"{"AppId":"e26e26e0-0000-4000-8000-0000000000bb","Name":"Dep","Publisher":"t","Version":"1.0.0.0",
"Tables":[
 {"Id":60000,"Name":"Dep Target","Fields":[{"Id":1,"Name":"K","TypeDefinition":{"Name":"Integer"}}]},
 {"Id":60001,"Name":"Dep Base","Fields":[{"Id":1,"Name":"K","TypeDefinition":{"Name":"Integer"}}],
  "Variables":[{"Name":"Other","TypeDefinition":{"Name":"Record","Subtype":{"Name":"Dep Target","Id":60000}},"Protected":true},
               {"Name":"Hidden","TypeDefinition":{"Name":"Record","Subtype":{"Name":"Dep Target","Id":60000}}}]}
]}"#;

const EXT_SOURCE: &str = r#"
tableextension 52910 "E26 Dep Ext" extends "Dep Base"
{
    procedure WriteOther()
    begin
        Other.Modify(true);
    end;
}
"#;

/// A base object known only by its dependency's symbols still gives its
/// protected record to the extension: the symbol file's protected variables
/// are read (`symbol_reference::protected_vars_of`).
/// Break (2026-11-13): leave `protected_vars` empty in `abi_ingest` ->
/// `WriteOther` answers `unknown` (the dependency is not followed, so never a
/// wrong `no`, but not the `yes` the symbols allow). Fails; restored, it passes.
#[test]
fn a_symbol_only_base_gives_its_protected_record_too() {
    let t = workspace(
        EXT_SOURCE,
        &format!(r#"{{"id":"{DEP_GUID}","name":"Dep","publisher":"t","version":"1.0.0.0"}}"#),
    );
    crate::symbol_app::write_symbol_app(
        &t.path().join(".alpackages/t_Dep_1.0.0.0.app"),
        DEP_GUID,
        "Dep",
        "1.0.0.0",
        DEP_SYMBOLS,
    );
    let ws = t.path().to_str().unwrap();
    let a = touches(ws, "Dep Target", "\"E26 Dep Ext\".WriteOther");
    assert!(a.starts_with("answer: down: yes"), "{a}");
}
