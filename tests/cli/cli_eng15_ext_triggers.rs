//! ENG-15 (with ENG-16 merged into it, TLS-44): a record operation's trigger
//! fan-out (`resolver::resolve_implicit_trigger`) looked up only the table's
//! `On<Op>`, so a table extension's `OnBefore<Op>`/`OnAfter<Op>` (a field's
//! `OnAfterValidate` in a tableextension, `OnAfterInsert`, …) never got an edge,
//! and a known table whose code is loaded from symbols only read as having no
//! triggers. Both gave a definite `no` where the answer is `yes` or `unknown`.

use std::process::Command;

use serde_json::Value;

const SOURCE: &str = r#"
table 53200 "E15 T"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; F; Integer) { }
    }
    keys { key(PK; K) { } }
}
tableextension 53201 "E15 TExt" extends "E15 T"
{
    fields
    {
        modify(F)
        {
            trigger OnAfterValidate()
            begin
                Commit();
            end;
        }
    }

    trigger OnAfterInsert()
    begin
        Commit();
    end;

    trigger OnBeforeModify()
    begin
        Commit();
    end;
}
codeunit 53202 "E15 Ops"
{
    procedure DoInsert()
    var
        R: Record "E15 T";
    begin
        R.K := 1;
        if R.Insert(true) then;
    end;

    procedure DoInsertNoTrigger()
    var
        R: Record "E15 T";
    begin
        R.K := 1;
        if R.Insert(false) then;
    end;

    procedure DoModify()
    var
        R: Record "E15 T";
    begin
        if R.Get(1) then
            if R.Modify(true) then;
    end;

    procedure DoValidate()
    var
        R: Record "E15 T";
    begin
        R.Validate(F, 1);
    end;

    procedure DoValidateOther()
    var
        R: Record "E15 T";
    begin
        R.Validate(K, 1);
    end;
}
"#;

const APP: &str = r#"{"id":"e15e15e0-0000-4000-8000-0000000000cc","name":"E15","publisher":"t","version":"1.0.0.0","dependencies":[]}"#;

fn may_commit(ws: &std::path::Path, routine: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            ws.to_str().unwrap(),
            routine,
            "may-commit",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    v["payload"]["result"].clone()
}

/// A table extension's `OnAfterInsert`, `OnBeforeModify` and a field's
/// `OnAfterValidate` run for `Insert(true)`, `Modify(true)` and
/// `Validate(F, …)`; `Insert(false)` runs no trigger and validating another
/// field runs not F's.
/// Break (2026-11-13): `resolve_implicit_trigger` looks up `On<Op>` only in
/// the extensions (`ext_names` = `[trigger_name]`) -> `DoInsert`, `DoModify`
/// and `DoValidate` answer `no` (master 3dbf23ed answered `no` for all three).
/// Fails; restored, it passes.
#[test]
fn table_extension_before_and_after_triggers_run() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(t.path().join("app.json"), APP).unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    for (routine, want) in [
        ("DoInsert", "yes"),
        ("DoModify", "yes"),
        ("DoValidate", "yes"),
        ("DoInsertNoTrigger", "no"),
        ("DoValidateOther", "no"),
    ] {
        let r = may_commit(t.path(), &format!("Codeunit \"E15 Ops\"::{routine}"));
        assert_eq!(r["answer"], want, "{routine}: {r:#}");
    }
}

/// A workspace whose table "E16 Sym T" comes from a dependency loaded from
/// symbols only.
fn sym_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e16e16e0-0000-4000-8000-0000000000cd","name":"E16 Host","publisher":"t","version":"1.0.0.0","dependencies":[{"id":"22222222-0000-0000-0000-0000000000e1","name":"E16 Sym","publisher":"t","version":"1.0.0.0"}]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/a.al"),
        r#"codeunit 53210 "E16 Probe"
{
    procedure ModifySym()
    var
        R: Record "E16 Sym T";
    begin
        if R.Get('A') then
            R.Modify(true);
    end;

    procedure ModifySymNoTrigger()
    var
        R: Record "E16 Sym T";
    begin
        if R.Get('A') then
            R.Modify(false);
    end;

    local procedure GetSym(): Record "E16 Sym T"
    var
        R: Record "E16 Sym T";
    begin
        exit(R);
    end;

    procedure ModifyCallResult()
    begin
        GetSym().Modify(true);
    end;

    procedure ValidateFieldRef()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"E16 Sym T");
        FR := RR.Field(1);
        FR.Validate('A');
    end;

    procedure ImportSym(InS: InStream)
    begin
        if Xmlport.Import(Xmlport::"E16 Imp", InS) then;
    end;
}
xmlport 53213 "E16 Imp"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(T; "E16 Sym T")
            {
                fieldelement(No; T."No.") { }
            }
        }
    }
}
table 53211 "E16 Local T"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 53212 "E16 Local Subs"
{
    // Another table's modify-event subscriber: a modify op on ANY table may
    // reach one of its family, which must not open `ModifySymNoTrigger`.
    [EventSubscriber(ObjectType::Table, Database::"E16 Local T", 'OnAfterModifyEvent', '', false, false)]
    local procedure OnLocalModify(var Rec: Record "E16 Local T"; var xRec: Record "E16 Local T"; RunTrigger: Boolean)
    begin
        Commit();
    end;
}
"#,
    )
    .unwrap();
    let symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000e1","Name":"E16 Sym","Publisher":"t","Version":"1.0.0.0","Tables":[{"Id":70960,"Name":"E16 Sym T","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}]}"#;
    crate::symbol_app::write_symbol_app(
        &t.path().join(".alpackages/t_E16 Sym_1.0.0.0.app"),
        "22222222-0000-0000-0000-0000000000e1",
        "E16 Sym",
        "1.0.0.0",
        symbols,
    );
    t
}

/// ENG-16: a trigger-firing operation on a known table loaded from symbols only
/// may run triggers the engine cannot see: `unknown`, with the gap named, not
/// `no`. `Modify(false)` runs no trigger: still `no`.
/// The workspace also subscribes to ANOTHER table's `OnAfterModifyEvent`, so a
/// family-wide open bound would wrongly open `ModifySymNoTrigger`.
/// Breaks (2026-11-13): drop the `table_code_not_loaded` check in
/// `full.rs`'s record-operation path -> `ModifySym` answers `no` (as master
/// 3dbf23ed did); drop its `rule.run_trigger != Some(false)` condition ->
/// `ModifySymNoTrigger` answers `unknown` (the `RecordEvents` bound opens on
/// the other table's subscriber). Each fails; restored, it passes. (Fable
/// round 2: without that subscriber the second break stayed green, a test
/// defect, now fixed.)
#[test]
fn a_trigger_op_on_a_symbol_only_table_is_unknown() {
    let t = sym_workspace();
    let r = may_commit(t.path(), "Codeunit \"E16 Probe\"::ModifySym");
    assert_eq!(r["answer"], "unknown", "{r:#}");
    let details: Vec<&str> = r["blockedBy"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["detail"].as_str())
        .collect();
    assert!(
        details
            .iter()
            .any(|d| d.contains("whose code is loaded from symbols only")),
        "{r:#}"
    );
    let r = may_commit(t.path(), "Codeunit \"E16 Probe\"::ModifySymNoTrigger");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// ENG-69, folded into ENG-15 (sol round 1): every other path a record
/// operation takes asks the same opacity check
/// (`full::trigger_op_opens_unseen_code`): an operation on a function's
/// returned record (the path an inherited protected var takes too, ENG-26), a
/// decided `FieldRef.Validate`, and an XmlPort's own insert. Each was `no` on a
/// symbol-only table.
/// Breaks (2026-11-13): drop the flag on the call-result path
/// (`CallResultRecord::code_not_loaded` false) -> `ModifyCallResult` `no`;
/// on the FieldRef path (`FieldRefValidate::Decided::code_not_loaded` false)
/// -> `ValidateFieldRef` `no`; on the dataset path
/// (`DatasetEffect::code_not_loaded` false) -> `ImportSym` `no`. Each fails;
/// restored, it passes.
#[test]
fn every_record_operation_path_opens_on_a_symbol_only_table() {
    let t = sym_workspace();
    for routine in ["ModifyCallResult", "ValidateFieldRef", "ImportSym"] {
        let r = may_commit(t.path(), &format!("Codeunit \"E16 Probe\"::{routine}"));
        assert_eq!(r["answer"], "unknown", "{routine}: {r:#}");
    }
}

/// A source table whose EXTENSION is loaded from symbols only (sol round 1):
/// the extension's triggers are unseen too. The table is a dependency's, with
/// source; the extension is another dependency's, symbols only.
fn ext_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e16e16e0-0000-4000-8000-0000000000ce","name":"E16 Host2","publisher":"t","version":"1.0.0.0","dependencies":[{"id":"22222222-0000-0000-0000-0000000000e4","name":"E16 Src","publisher":"t","version":"1.0.0.0"},{"id":"22222222-0000-0000-0000-0000000000e5","name":"E16 SymExt","publisher":"t","version":"1.0.0.0"}]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("src/a.al"),
        r#"codeunit 53220 "E16 Probe2"
{
    procedure ModifySrc()
    var
        R: Record "E16 Src T";
    begin
        if R.Get(1) then
            R.Modify(true);
    end;

    procedure ModifySrcNoTrigger()
    var
        R: Record "E16 Src T";
    begin
        if R.Get(1) then
            R.Modify();
    end;
}
"#,
    )
    .unwrap();
    let src_symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000e4","Name":"E16 Src","Publisher":"t","Version":"1.0.0.0","Tables":[{"Id":70965,"Name":"E16 Src T","Fields":[{"Id":1,"Name":"K","TypeDefinition":{"Name":"Integer"}}]}]}"#;
    crate::symbol_app::write_source_app(
        &t.path().join(".alpackages/t_E16 Src_1.0.0.0.app"),
        "22222222-0000-0000-0000-0000000000e4",
        "E16 Src",
        "1.0.0.0",
        src_symbols,
        &[(
            "src/T.al",
            "table 70965 \"E16 Src T\"\n{\n    fields { field(1; K; Integer) { } }\n    keys { key(PK; K) { } }\n}\n",
        )],
        "",
    );
    let ext_symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000e5","Name":"E16 SymExt","Publisher":"t","Version":"1.0.0.0","TableExtensions":[{"Id":70966,"Name":"E16 Src T Ext","TargetObject":"E16 Src T"}]}"#;
    crate::symbol_app::write_symbol_app(
        &t.path().join(".alpackages/t_E16 SymExt_1.0.0.0.app"),
        "22222222-0000-0000-0000-0000000000e5",
        "E16 SymExt",
        "1.0.0.0",
        ext_symbols,
    );
    t
}

/// A trigger-firing operation on a source table with a symbol-only extension
/// is `unknown`; a `Modify()` (RunTrigger left out) stays `no`.
/// Break (2026-11-13): drop the extension scan in `table_code_not_loaded` ->
/// `ModifySrc` answers `no`. Fails; restored, it passes.
#[test]
fn a_symbol_only_extension_opens_its_table() {
    let t = ext_workspace();
    let r = may_commit(t.path(), "Codeunit \"E16 Probe2\"::ModifySrc");
    assert_eq!(r["answer"], "unknown", "{r:#}");
    let r = may_commit(t.path(), "Codeunit \"E16 Probe2\"::ModifySrcNoTrigger");
    assert_eq!(r["answer"], "no", "{r:#}");
}
