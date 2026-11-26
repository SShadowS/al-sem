//! ENG-194: a table name in the production cross-app model binds as the
//! program's resolver binds it, seen from the referencing app (Fable r1 B1:
//! pin the USE through `resolve` with a real program graph, not the helper).
//!
//! The workspace depends on four apps, written so a LAST-wins index would
//! pick wrong: symbol-only `Found` (the live `No. Series` 308, a `Dup` 70),
//! symbol-only `ZOld` (a `Moved` `No. Series` 308), the source app
//! `Zz Unrelated` (a `Shared` 60) and the source app `ZBase` (a `Shared` 50, a
//! tableextension of it, and code naming `Shared`), which depends on none of
//! them. The model
//! holds only the workspace's closure, so "an app outside the closure" is
//! seen from a dependency's own code: `ZBase` does not see `Zz Unrelated`.
//!
//! DISCRIMINATION (2026-11-26, each break made with Edit, the test run red,
//! then restored and green) is recorded in the test's doc.

use al_sem::program::model::workspace::ModelEntities;

const WS: &str = "aaaaaaaa-0000-0000-0000-0000000000a1";
const FOUND: &str = "ffffffff-0000-0000-0000-0000000000f1";
const ZOLD: &str = "dddddddd-0000-0000-0000-0000000000d1";
const ZBASE: &str = "bbbbbbbb-0000-0000-0000-0000000000b1";
const UNRELATED: &str = "cccccccc-0000-0000-0000-0000000000c1";

fn symbols(guid: &str, name: &str, objects: &str) -> String {
    format!(
        r#"{{"AppId":"{guid}","Name":"{name}","Publisher":"probe","Version":"1.0.0.0",{objects}}}"#
    )
}

fn table(id: i64, name: &str, props: &str) -> String {
    format!(
        r#"{{"Id":{id},"Name":"{name}","Properties":[{props}],"Fields":[{{"Id":1,"Name":"Entry No.","TypeDefinition":{{"Name":"Integer"}}}}]}}"#
    )
}

const WS_MAIN: &str = r#"codeunit 50201 "Ws Main"
{
    procedure Go()
    var
        NS: Record "No. Series";
        Z: Codeunit "Z Main";
    begin
        NS.FindFirst();
        Z.Work();
    end;
}

table 50200 "Dup"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }

    procedure Touch()
    begin
        Rec.Modify();
    end;
}
"#;

const ZBASE_SRC: &str = r#"table 50 "Shared"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

tableextension 51 "Shared Ext" extends "Shared"
{
    fields
    {
        field(51; "Z Flag"; Boolean) { }
    }
}

codeunit 52 "Z Main"
{
    procedure Work()
    var
        S: Record "Shared";
    begin
        S.FindFirst();
    end;
}
"#;

fn model() -> ModelEntities {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    let dir = tempfile::tempdir().unwrap();
    let dep = |g: &str, n: &str| {
        format!(r#"{{"id":"{g}","name":"{n}","publisher":"probe","version":"1.0.0.0"}}"#)
    };
    std::fs::write(
        dir.path().join("app.json"),
        format!(
            r#"{{"id":"{WS}","name":"Ws","publisher":"P","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{},{},{},{}]}}"#,
            dep(FOUND, "Found"),
            dep(ZOLD, "ZOld"),
            dep(ZBASE, "ZBase"),
            dep(UNRELATED, "Zz Unrelated"),
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("Main.al"), WS_MAIN).unwrap();
    let pk = dir.path().join(".alpackages");
    let moved = r#"{"Name":"ObsoleteState","Value":"Moved"}"#;
    let symbol_apps = [
        (
            FOUND,
            "Found",
            [table(308, "No. Series", ""), table(70, "Dup", "")].join(","),
        ),
        (ZOLD, "ZOld", table(308, "No. Series", moved)),
    ];
    for (guid, name, tables) in &symbol_apps {
        crate::symbol_app::write_symbol_app(
            &pk.join(format!("probe_{name}_1.0.0.0.app")),
            guid,
            name,
            "1.0.0.0",
            &symbols(guid, name, &format!(r#""Tables":[{tables}]"#)),
        );
    }
    let zbase_symbols = format!(
        r#""Tables":[{}],"TableExtensions":[{{"Id":51,"Name":"Shared Ext","TargetObject":"Shared"}}],"Codeunits":[{{"Id":52,"Name":"Z Main","Methods":[{{"Name":"Work","Parameters":[]}}]}}]"#,
        table(50, "Shared", "")
    );
    crate::symbol_app::write_source_app(
        &pk.join("probe_ZBase_1.0.0.0.app"),
        ZBASE,
        "ZBase",
        "1.0.0.0",
        &symbols(ZBASE, "ZBase", &zbase_symbols),
        &[("src/ZBase.al", ZBASE_SRC)],
        "",
    );
    // A source app too, so its table sits beside ZBase's in the model's
    // order (a LAST-wins pick depends on it).
    crate::symbol_app::write_source_app(
        &pk.join("probe_Zz Unrelated_1.0.0.0.app"),
        UNRELATED,
        "Zz Unrelated",
        "1.0.0.0",
        &symbols(
            UNRELATED,
            "Zz Unrelated",
            &format!(r#""Tables":[{}]"#, table(60, "Shared", "")),
        ),
        &[(
            "src/Shared.al",
            "table 60 \"Shared\"\n{\n    fields\n    {\n        field(1; \"Entry No.\"; Integer) { }\n    }\n}\n",
        )],
        "",
    );
    assemble_and_resolve_cross_app_program(
        &al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &Default::default()),
        false,
        al_sem::program::model::workspace::AssemblyNeeds::BASE,
    )
    .expect("cross-app model")
    .resolved
    .workspace
}

fn var_table(ws: &ModelEntities, app: &str, routine: &str, var: &str) -> Option<String> {
    let r = ws
        .routines
        .iter()
        .find(|r| r.app_guid == app && r.name == routine)
        .unwrap_or_else(|| {
            panic!(
                "routine {routine} of {app}; have {:?}",
                ws.routines
                    .iter()
                    .map(|r| (&r.app_guid, &r.name))
                    .collect::<Vec<_>>()
            )
        });
    r.record_variables
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(var))
        .unwrap_or_else(|| panic!("{routine}: record variable {var}"))
        .table_id
        .as_deref()
        .map(str::to_owned)
}

/// DISCRIMINATION (2026-11-26, each break made with Edit, run red, restored,
/// green): `workspace::resolve` building with `SymbolTable::build` (no scope)
/// fails "moved gives way"; an empty `TableScope::gone` fails "moved gives
/// way"; `abi_ingest` setting `obsolete_gone: false` fails "moved gives way";
/// no own-table branch for a `Table`'s `Rec` fails "own Rec";
/// `merge_extension_fields` taking the last table of the name
/// (`table_by_name`) fails "extension merges into the seen base" (only with
/// `Zz Unrelated` a source app too: as a symbol-only app its table sat before
/// ZBase's, and the last-wins pick was right by luck); `sees` always true
/// fails "closure only".
#[test]
fn a_table_name_binds_as_the_resolver_binds_it_in_the_model() {
    let ws = model();
    let real_named = |n: &str| {
        ws.tables
            .iter()
            .filter(|t| t.name == n && !t.is_extension_stub)
            .count()
    };
    // Preconditions, stated: each name has more than one real table in the
    // model, so a LAST-wins pick could be wrong.
    assert_eq!(real_named("No. Series"), 2, "two No. Series");
    assert_eq!(real_named("Shared"), 2, "two Shared");
    assert_eq!(real_named("Dup"), 2, "two Dup");

    // A Moved symbol table gives way to the live one (scope + `gone` from the
    // symbols' `ObsoleteState`).
    assert_eq!(
        var_table(&ws, WS, "Go", "NS").as_deref(),
        Some(format!("{FOUND}/table/308").as_str()),
        "moved gives way"
    );
    // A dependency's code sees its own closure only.
    assert_eq!(
        var_table(&ws, ZBASE, "Work", "S").as_deref(),
        Some(format!("{ZBASE}/table/50").as_str()),
        "closure only"
    );
    // A table's own Rec is the table, though its name has two visible tables.
    assert_eq!(
        var_table(&ws, WS, "Touch", "Rec").as_deref(),
        Some(format!("{WS}/table/50200").as_str()),
        "own Rec"
    );
    // A tableextension's fields join the base its own app sees.
    let has_flag = |id: String| {
        ws.tables
            .iter()
            .filter(|t| t.id == id)
            .any(|t| t.fields.iter().any(|f| f.name == "Z Flag"))
    };
    assert!(
        has_flag(format!("{ZBASE}/table/50")),
        "extension merges into the seen base"
    );
    assert!(
        !has_flag(format!("{UNRELATED}/table/60")),
        "extension leaves the unseen table alone"
    );
}
