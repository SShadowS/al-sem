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

pageextension 50203 "Ws PE" extends "Moved Page"
{
    procedure PeTouch()
    begin
        Rec.Modify();
    end;
}

#if ONE
page 50210 "Split Page"
{
    SourceTable = "No. Series";
}
#else
page 50210 "Split Page"
{
    SourceTable = "No. Series";
    SourceTableTemporary = true;
}
#endif

pageextension 50211 "Split PE" extends "Split Page"
{
    procedure SplitTouch()
    begin
        Rec.Modify();
    end;
}

#if ONE
page 50212 "Same Page"
{
    SourceTable = "No. Series";
}
#else
page 50212 "Same Page"
{
    SourceTable = "No. Series";
    SourceTableTemporary = false;
}
#endif

pageextension 50213 "Same PE" extends "Same Page"
{
    procedure SameTouch()
    begin
        Rec.Modify();
    end;
}

table 50220 "Ws Only"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

#if ONE
page 50214 "Two Table Page"
{
    SourceTable = "No. Series";
}
#else
page 50214 "Two Table Page"
{
    SourceTable = "Ws Only";
}
#endif

pageextension 50215 "Two Table PE" extends "Two Table Page"
{
    procedure TwoTouch()
    begin
        Rec.Modify();
    end;
}

tableextension 50240 "Moving Ext" extends "New Base"
{
    keys
    {
        key(WsMovingKey; "Entry No.") { }
    }
}

table 50230 "Keyed"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

#if ONE
tableextension 50231 "Keyed Ext" extends "Keyed"
{
    fields
    {
        field(50231; "KA"; Integer) { }
    }
    keys
    {
        key(KeyA; "KA") { }
    }
}
#else
tableextension 50231 "Keyed Ext B" extends "Keyed"
{
    fields
    {
        field(50231; "KA"; Integer) { }
        field(50232; "KB"; Integer) { }
    }
    keys
    {
        key(KeyA; "KA") { }
        key(KeyB; "KB") { }
        key(KeyC; "KB") { SumIndexFields = "KA"; }
    }
}
#endif
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

#if ONE
table 55 "Split"
{
    fields
    {
        field(1; "A"; Integer) { }
    }
}
#else
table 55 "Split"
{
    fields
    {
        field(1; "B"; Integer) { }
    }
}
#endif

tableextension 56 "Split Ext" extends "Split"
{
    fields
    {
        field(56; "Split Flag"; Boolean) { }
    }
}

table 57 "New Base"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

table 58 "Old Base"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

#if NEW
tableextension 59 "Moving Ext" extends "New Base"
{
    fields
    {
        field(59; "New Flag"; Boolean) { }
    }
    keys
    {
        key(MovingKey; "Entry No.") { }
    }
}
#else
tableextension 59 "Moving Ext" extends "Old Base"
{
    fields
    {
        field(60; "Old Flag"; Boolean) { }
    }
}
#endif

codeunit 52 "Z Main"
{
    procedure Work()
    var
        S: Record "Shared";
        ZP: Page "Z Page";
    begin
        S.FindFirst();
        ZP.PageTouch();
        ZP.ExtTouch();
    end;
}

page 53 "Z Page"
{
    SourceTable = 50;

    procedure PageTouch()
    begin
        Rec.Modify();
    end;
}

pageextension 54 "Z Page Ext" extends "Z Page"
{
    procedure ExtTouch()
    begin
        Rec.Modify();
    end;
}
"#;

/// `Zz Unrelated`: its own table 50 and a temporary page of ZBase's page's
/// name, so a LAST-wins number or page lookup from ZBase picks these.
const UNRELATED_SRC: &str = r#"table 60 "Shared"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

table 50 "Other"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

page 62 "Z Page"
{
    SourceTable = "Shared";
    SourceTableTemporary = true;
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
    let source_no_series = r#"{"Name":"SourceTable","Value":"No. Series"}"#;
    let symbol_apps = [
        (
            FOUND,
            "Found",
            format!(
                r#""Tables":[{}],"Pages":[{{"Id":64,"Name":"Moved Page","Properties":[{source_no_series}]}}]"#,
                [table(308, "No. Series", ""), table(70, "Dup", "")].join(",")
            ),
        ),
        (
            ZOLD,
            "ZOld",
            format!(
                r#""Tables":[{}],"Pages":[{{"Id":63,"Name":"Moved Page","Properties":[{moved},{source_no_series}]}}]"#,
                table(308, "No. Series", moved)
            ),
        ),
    ];
    for (guid, name, objects) in &symbol_apps {
        crate::symbol_app::write_symbol_app(
            &pk.join(format!("probe_{name}_1.0.0.0.app")),
            guid,
            name,
            "1.0.0.0",
            &symbols(guid, name, objects),
        );
    }
    let zbase_symbols = format!(
        r#""Tables":[{}],"TableExtensions":[{{"Id":51,"Name":"Shared Ext","TargetObject":"Shared"}}],"Codeunits":[{{"Id":52,"Name":"Z Main","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"Pages":[{{"Id":53,"Name":"Z Page","Methods":[{{"Name":"PageTouch","Parameters":[]}}]}}],"PageExtensions":[{{"Id":54,"Name":"Z Page Ext","TargetObject":"Z Page","Methods":[{{"Name":"ExtTouch","Parameters":[]}}]}}]"#,
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
            &format!(
                r#""Tables":[{},{}],"Pages":[{{"Id":62,"Name":"Z Page"}}]"#,
                table(60, "Shared", ""),
                table(50, "Other", "")
            ),
        ),
        &[("src/Unrelated.al", UNRELATED_SRC)],
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
/// fails "closure only"; `page_temp` reading the last page of the name fails
/// "... not the unrelated temporary page"; merging into the FIRST row of a
/// split base fails "split base: the kept row"; one target per extension id
/// fails "split extension: both targets"; a number through the last table
/// object of the number fails "number, as its app sees it" (own-first is
/// proven in `symbol_table`'s unit test: ZBase does not see the other 50);
/// pass 3 picking the base page unscoped fails "page extension's base page";
/// every row of a split tableextension taking the LAST stub fails "split
/// extension: both targets"; no gone object ids in `TableScope::of` fails "a
/// moved page gives way"; deduplicating base-page rows by id alone fails
/// "alternatives naming different tables decline"; deduplicating them by
/// table AND temporary flag fails "alternatives naming one table give it";
/// the temporary flag read from the last alternative fails "alternatives that
/// differ in SourceTableTemporary force no temporary"; every key of a split
/// tableextension keeping the first alternative's id fails "both
/// alternatives' keys"; skipping a key whose same-NAMED extension already
/// added its members fails "two apps' same-named extensions each keep their
/// key"; no repeat check for a later alternative fails "... once each"; the
/// repeat check against the keys this alternative is adding (not only
/// earlier ones) drops KeyC and fails "... once each".
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
    // sol + Fable r2: an `#if`-split base table's extension fields go to the
    // row the by-id readers keep (the last real row of the id).
    let last_row_has = |id: String, field: &str| {
        ws.tables
            .iter()
            .rfind(|t| t.id == id && !t.is_extension_stub)
            .is_some_and(|t| t.fields.iter().any(|f| f.name == field))
    };
    assert_eq!(
        ws.tables
            .iter()
            .filter(|t| t.id == format!("{ZBASE}/table/55") && !t.is_extension_stub)
            .count(),
        2,
        "precondition: the split table has two rows"
    );
    assert!(
        last_row_has(format!("{ZBASE}/table/55"), "Split Flag"),
        "split base: the kept row"
    );
    // Fable r2: each row of an `#if`-split tableextension merges into its own
    // target; sol r3: with its own alternative's fields.
    assert!(
        last_row_has(format!("{ZBASE}/table/57"), "New Flag")
            && last_row_has(format!("{ZBASE}/table/58"), "Old Flag"),
        "split extension: both targets"
    );
    assert!(
        !last_row_has(format!("{ZBASE}/table/57"), "Old Flag")
            && !last_row_has(format!("{ZBASE}/table/58"), "New Flag"),
        "split extension: each target its own alternative's fields"
    );
    // Fable r3: a Moved symbol page gives way to the live page of its name.
    let pages_named = |n: &str| {
        ws.objects
            .iter()
            .filter(|o| o.object_type == "Page" && o.name == n)
            .count()
    };
    assert_eq!(pages_named("Moved Page"), 2, "precondition: two Moved Page");
    assert_eq!(
        var_table(&ws, WS, "PeTouch", "Rec").as_deref(),
        Some(format!("{FOUND}/table/308").as_str()),
        "a moved page gives way"
    );
    // sol r3 / Fable r4: `#if` alternatives of a base page that name
    // different tables decline; alternatives that name one table give it,
    // and their temporary flag only when they agree on it.
    let rec = |routine: &str| {
        ws.routines
            .iter()
            .find(|r| r.app_guid == WS && r.name == routine)
            .and_then(|r| {
                r.record_variables
                    .iter()
                    .find(|v| v.name.eq_ignore_ascii_case("Rec"))
            })
            .unwrap_or_else(|| panic!("{routine}'s Rec"))
    };
    let forced_temp = |routine: &str| {
        let v = rec(routine);
        v.temp_state.kind == "known" && v.temp_state.value == Some(true)
    };
    assert_eq!(
        pages_named("Two Table Page"),
        2,
        "precondition: split page rows"
    );
    assert_eq!(
        var_table(&ws, WS, "TwoTouch", "Rec"),
        None,
        "alternatives naming different tables decline"
    );
    assert_eq!(
        var_table(&ws, WS, "SplitTouch", "Rec").as_deref(),
        Some(format!("{FOUND}/table/308").as_str()),
        "alternatives naming one table give it"
    );
    assert!(
        !forced_temp("SplitTouch"),
        "alternatives that differ in SourceTableTemporary force no temporary"
    );
    assert_eq!(
        var_table(&ws, WS, "SameTouch", "Rec").as_deref(),
        Some(format!("{FOUND}/table/308").as_str()),
        "agreeing alternatives are one page"
    );
    // sol r4: two alternatives of one tableextension on ONE base each add
    // their key; sol + Fable r5: the alternative that renames the extension
    // and repeats `KeyA` adds it once (keys counted by the extension's own id
    // prefix, not its name).
    let keyed = ws
        .tables
        .iter()
        .rfind(|t| t.id == format!("{WS}/table/50230") && !t.is_extension_stub)
        .expect("Keyed");
    let own = format!("{WS}/table/50230/key/ext/{WS}/50231/");
    let ext_keys: Vec<&Vec<String>> = keyed
        .keys
        .iter()
        .filter(|k| k.id.starts_with(&own))
        .map(|k| &k.members)
        .collect();
    // sol r6: and the later alternative's own two keys over `KB` (KeyB,
    // KeyC) are both kept: keys of one alternative never match each other.
    assert_eq!(
        ext_keys.len(),
        3,
        "both alternatives' keys, once each: {ext_keys:?}"
    );
    // sol + Fable r5: another app's same-named extension with an identical
    // key on the same base keeps its key.
    let new_base = ws
        .tables
        .iter()
        .rfind(|t| t.id == format!("{ZBASE}/table/57") && !t.is_extension_stub)
        .expect("New Base");
    assert_eq!(
        new_base
            .keys
            .iter()
            .filter(|k| k.declared_by.as_deref() == Some("Moving Ext"))
            .count(),
        2,
        "two apps' same-named extensions each keep their key: {:?}",
        new_base.keys.iter().map(|k| &k.id).collect::<Vec<_>>()
    );
    // sol r2: a page's `SourceTable = 50` binds by number from its own app,
    // though the unrelated app's table 50 comes later.
    assert_eq!(
        var_table(&ws, ZBASE, "PageTouch", "Rec").as_deref(),
        Some(format!("{ZBASE}/table/50").as_str()),
        "number, as its app sees it"
    );
    // sol r2: a page extension's base page is the one its app sees: its
    // table, and its (absent) `SourceTableTemporary`, not the unrelated
    // temporary page's.
    assert_eq!(
        var_table(&ws, ZBASE, "ExtTouch", "Rec").as_deref(),
        Some(format!("{ZBASE}/table/50").as_str()),
        "page extension's base page"
    );
    let ext_rec = ws
        .routines
        .iter()
        .find(|r| r.app_guid == ZBASE && r.name == "ExtTouch")
        .and_then(|r| {
            r.record_variables
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case("Rec"))
        })
        .expect("ExtTouch's Rec");
    assert_ne!(
        (
            ext_rec.temp_state.kind.to_string(),
            ext_rec.temp_state.value
        ),
        ("known".to_string(), Some(true)),
        "page extension's base page is not the unrelated temporary page"
    );
}
