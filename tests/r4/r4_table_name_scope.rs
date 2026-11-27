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
        U: Codeunit "U Main";
    begin
        NS.FindFirst();
        NS.CalcFields("Ws Only Count");
        Z.Work();
        U.Add();
    end;
}

table 50200 "Dup"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Series Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("No. Series");
        }
    }

    procedure Touch()
    begin
        Rec.Modify();
    end;

    procedure CountSeries()
    begin
        Rec.CalcFields("Series Count");
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
        field(2; "A Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("Keyed");
        }
    }

    procedure WsFind()
    begin
        Rec.FindFirst();
    end;
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

table 50250 "Tri"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

#if A
tableextension 50251 "Tri Ext" extends "Tri"
{
    fields
    {
        field(50251; "F"; Integer) { }
    }
    keys
    {
        key(K1; "F") { }
    }
}
#elif B
tableextension 50251 "Tri Ext" extends "Tri"
{
    fields
    {
        field(50252; "F"; Integer) { }
    }
    keys
    {
        key(K1; "F") { }
        key(K2; "F") { SumIndexFields = "Entry No."; }
    }
}
#else
tableextension 50251 "Tri Ext" extends "Tri"
{
    fields
    {
        field(50251; "F"; Integer) { }
    }
    keys
    {
        key(K1; "F") { }
        key(K2; "F") { SumIndexFields = "Entry No."; }
    }
}
#endif

tableextension 50260 "Ws NS Ext" extends "No. Series"
{
    fields
    {
        field(50260; "Ws Note"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
        field(50261; "Ws Only Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("Ws Only");
        }
    }
}

#if TWO
tableextension 50290 "Ws Two Base Ext" extends "Ws Only"
{
    fields
    {
        field(50290; "Ws Two"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }
}
#else
tableextension 50290 "Ws Two Base Ext" extends "Keyed"
{
    fields
    {
        field(50290; "Ws Two"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }
}
#endif

#if NEW
tableextension 50280 "Ws Moving Ext" extends "Ws Only"
{
    trigger OnBeforeInsert()
    begin
        Rec.SetAutoCalcFields("A Count");
    end;
}
#else
tableextension 50280 "Ws Moving Ext" extends "Keyed"
{
    trigger OnBeforeInsert()
    begin
    end;
}
#endif

#if OPTIONAL
tableextension 50270 "Ws Split Ext" extends "Not Loaded"
{
    fields
    {
        field(50270; "Ws Opt"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }
}
#else
tableextension 50270 "Ws Split Ext" extends "No. Series"
{
    fields
    {
        field(50270; "Ws Opt"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }
}
#endif

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
        field(2; "K Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("Ws Only");
        }
    }

    procedure KeyedFind()
    begin
        Rec.FindFirst();
    end;
}

#if ONE
tableextension 50231 "Keyed Ext" extends "Keyed"
{
    fields
    {
        field(50231; "KA"; Integer) { }
        field(50233; "KX"; Integer) { }
    }
    keys
    {
        key(KeyA; "KA") { }
        key(KeyX; "KX") { }
    }
}
#else
tableextension 50231 "Keyed Ext B" extends "Keyed"
{
    fields
    {
        field(50231; "KA"; Integer) { }
        field(50232; "KB"; Integer) { }
        field(50234; "KX"; Integer) { }
    }
    keys
    {
        key(KeyA; "KA") { }
        key(KeyB; "KB") { }
        key(KeyC; "KB") { SumIndexFields = "KA"; }
        key(KeyD; "KA") { SumIndexFields = "KB"; }
        key(KeyX; "KX") { }
    }
}
#endif
"#;

const ZBASE_SRC: &str = r#"table 50 "Shared"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Shared Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("Shared");
        }
    }

    procedure P()
    begin
        Rec.FindFirst();
    end;
}

tableextension 51 "Shared Ext" extends "Shared"
{
    fields
    {
        field(51; "Z Flag"; Boolean)
        {
            trigger OnValidate()
            begin
            end;
        }
    }

    trigger OnBeforeInsert()
    begin
        Rec."Z Flag" := true;
    end;
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
        S.Insert(true);
        S.Validate("Z Flag", true);
        S.CalcFields("Shared Count");
        ZP.PageTouch();
        ZP.ExtTouch();
        RefCount();
        RefByName();
        S.P();
    end;

    procedure RefCount()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(50);
        FR := RR.Field(2);
        FR.CalcField();
    end;

    procedure RefByName()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"Shared");
        FR := RR.Field(2);
        FR.CalcField();
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

tableextension 63 "U Ext" extends "Shared"
{
    fields
    {
        field(63; "U Flow"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("Other");
        }
    }

    trigger OnBeforeInsert()
    begin
        Rec.SetAutoCalcFields("U Flow");
    end;
}

codeunit 64 "U Main"
{
    procedure Add()
    var
        S: Record "Shared";
    begin
        S.Insert(true);
    end;
}
"#;

fn model() -> ModelEntities {
    full_model().workspace
}

fn full_model() -> al_sem::program::model::workspace::Model {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    let dir = fixture();
    assemble_and_resolve_cross_app_program(
        &al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &Default::default()),
        false,
        al_sem::program::model::workspace::AssemblyNeeds::BASE,
    )
    .expect("cross-app model")
    .resolved
}

/// The workspace and its four dependencies, written to a fresh folder.
fn fixture() -> tempfile::TempDir {
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
                r#""Tables":[{},{}],"Pages":[{{"Id":62,"Name":"Z Page"}}],"TableExtensions":[{{"Id":63,"Name":"U Ext","TargetObject":"Shared"}}],"Codeunits":[{{"Id":64,"Name":"U Main","Methods":[{{"Name":"Add","Parameters":[]}}]}}]"#,
                table(60, "Shared", ""),
                table(50, "Other", "")
            ),
        ),
        &[("src/Unrelated.al", UNRELATED_SRC)],
        "",
    );
    dir
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
/// earlier ones) drops KeyC and fails "... once each"; a repeat check that
/// does not count (one earlier `KA` key skipping both later ones) drops KeyD
/// and fails "... once each"; a repeat matched on members alone (not the
/// fields they resolve to) drops the 50234 `KeyX` and fails "... once each";
/// resolving a key's members against the base's merged fields only (not the
/// alternative's own first) fails "each alternative's keys over its own
/// field".
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
    // Each key as (members, resolved field numbers), sorted.
    let mut ext_keys: Vec<(String, String)> = keyed
        .keys
        .iter()
        .filter(|k| k.id.starts_with(&own))
        .map(|k| {
            let numbers: Vec<&str> = k
                .fields
                .iter()
                .map(|f| f.rsplit('/').next().unwrap_or(""))
                .collect();
            (k.members.join(","), numbers.join(","))
        })
        .collect();
    ext_keys.sort();
    // sol r6: the later alternative's own two keys over `KB` (KeyB, KeyC) are
    // both kept: keys of one alternative never match each other; sol r7: its
    // two keys over `KA` (KeyA, KeyD) against the first alternative's one:
    // one repeat, one kept; sol r8: `KX` is field 50233 in one alternative
    // and 50234 in the other, so the two `KeyX` are different keys.
    let pair = |m: &str, n: &str| (m.to_string(), n.to_string());
    assert_eq!(
        ext_keys,
        vec![
            pair("ka", "50231"),
            pair("ka", "50231"),
            pair("kb", "50232"),
            pair("kb", "50232"),
            pair("kx", "50233"),
            pair("kx", "50234"),
        ],
        "both alternatives' keys, once each"
    );
    // sol r9: the ids themselves: the first alternative's keep `/{index}`, a
    // later one's take `/{index}/alt{n}`; KeyA of the second (index 0) is a
    // repeat and has none.
    let mut ids: Vec<String> = keyed
        .keys
        .iter()
        .filter_map(|k| k.id.strip_prefix(&own).map(str::to_owned))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        ["0", "1", "1/alt1", "2/alt1", "3/alt1", "4/alt1"],
        "key ids"
    );
    // sol r9: three alternatives, F = 50251, 50252, then 50251 again: the
    // third's keys resolve through ITS OWN field (50251), not the second's.
    let tri = ws
        .tables
        .iter()
        .rfind(|t| t.id == format!("{WS}/table/50250") && !t.is_extension_stub)
        .expect("Tri");
    let tri_own = format!("{WS}/table/50250/key/ext/{WS}/50251/");
    let mut tri_keys: Vec<String> = tri
        .keys
        .iter()
        .filter(|k| k.id.starts_with(&tri_own))
        .map(|k| {
            k.fields
                .iter()
                .map(|f| f.rsplit('/').next().unwrap_or("").to_owned())
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    tri_keys.sort();
    assert_eq!(
        tri_keys,
        ["50251", "50251", "50252", "50252"],
        "each alternative's keys over its own field"
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

/// ENG-218: a FlowField's `CalcFormula` table and a RecordRef's `Open` table
/// bind as ENG-194 binds a record's, seen from the app the formula or the
/// code is written in (`flow_fields::Tables`).
/// DISCRIMINATION (2026-11-27, each break made with Edit, run red, restored,
/// green): `Tables::by_name` ignoring the scope (an unscoped table) fails
/// "a moved formula table gives way"; the RecordRef number through the
/// model-wide number index fails "RecordRef number, own app"; the formula
/// bound from the host table's app instead of the field's declaring app
/// fails "an extension field's formula binds from its declaring app"; a
/// table and its extensions sharing `Rec` by NAME again (`object_groups`)
/// fails "another app's extension of a same-named table shares no Rec";
/// sharing through the union of every row of an object id (not the routine's
/// declaring row) fails "a split extension's alternative shares Rec only
/// with its own base"; a table extension that never shares fails "the
/// alternative shares Rec with its own base".
#[test]
fn flowfield_and_recordref_tables_bind_as_the_resolver_binds_them() {
    use al_sem::program::model::flow_fields::FlowFieldSource;
    let ws = model();
    let routine = |app: &str, name: &str| {
        ws.routines
            .iter()
            .find(|r| r.app_guid == app && r.name == name)
            .unwrap_or_else(|| panic!("routine {name} of {app}"))
    };
    let calc_sources = |app: &str, name: &str| -> Vec<FlowFieldSource> {
        routine(app, name)
            .record_operations
            .iter()
            .filter(|op| op.op == "CalcFields")
            .flat_map(|op| op.flow_field_reads.iter().map(|r| r.source.clone()))
            .collect()
    };
    let table = |id: String| FlowFieldSource::Table {
        table_id: id,
        temporary: false,
    };
    // The workspace's `count("No. Series")`: the Moved symbol table gives way.
    assert_eq!(
        calc_sources(WS, "CountSeries"),
        vec![table(format!("{FOUND}/table/308"))],
        "a moved formula table gives way"
    );
    // Fable r1: the workspace's extension field on Found's `No. Series`
    // counts the workspace's own "Ws Only": the formula is the workspace's,
    // seen from it (Found, the host table's app, does not see it).
    assert_eq!(
        calc_sources(WS, "Go"),
        vec![table(format!("{WS}/table/50220"))],
        "an extension field's formula binds from its declaring app"
    );
    // ZBase's `count("Shared")`: the unrelated app's `Shared` is not seen.
    assert_eq!(
        calc_sources(ZBASE, "Work"),
        vec![table(format!("{ZBASE}/table/50"))],
        "formula table within the closure"
    );
    // ZBase's `RR.Open(50)`: its own table 50, not the unrelated app's.
    let fr = &routine(ZBASE, "RefCount").field_ref_operations;
    assert_eq!(
        fr.iter()
            .map(|op| op.table_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some(format!("{ZBASE}/table/50").as_str())],
        "RecordRef number, own app"
    );
    // Fable r1: `RR.Open(Database::"Shared")` by name, from ZBase.
    let fr = &routine(ZBASE, "RefByName").field_ref_operations;
    assert_eq!(
        fr.iter()
            .map(|op| op.table_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some(format!("{ZBASE}/table/50").as_str())],
        "RecordRef name, within the closure"
    );
    // Fable r1: Unrelated's "U Ext" (of ITS `Shared`) sets
    // `SetAutoCalcFields("U Flow")` on its `Rec`; ZBase's own `Shared`
    // retrieves its `Rec`: the two do not share (a table and its extensions
    // share by table id), so ZBase's retrieval reads no "U Flow".
    let p_reads: Vec<String> = routine(ZBASE, "P")
        .record_operations
        .iter()
        .filter(|op| op.op == "FindFirst")
        .flat_map(|op| op.flow_field_reads.iter().map(|r| r.field.clone()))
        .collect();
    assert!(
        !p_reads.iter().any(|f| f == "U Flow"),
        "another app's extension of a same-named table shares no Rec: {p_reads:?}"
    );
    let auto = routine(UNRELATED, "OnBeforeInsert")
        .call_sites
        .iter()
        .any(|c| c.argument_texts.iter().any(|a| a.contains("U Flow")));
    assert!(
        auto,
        "precondition: Unrelated's SetAutoCalcFields is in the model"
    );
    // sol r2: the workspace's split "Ws Moving Ext" sets
    // `SetAutoCalcFields("A Count")` in its "Ws Only" alternative only; the
    // "Keyed" table's own retrieval shares nothing with it (each routine
    // shares through its own declaring row, not every row of the id).
    let keyed_reads: Vec<String> = routine(WS, "KeyedFind")
        .record_operations
        .iter()
        .filter(|op| op.op == "FindFirst")
        .flat_map(|op| op.flow_field_reads.iter().map(|r| r.field.clone()))
        .collect();
    assert!(
        !keyed_reads.iter().any(|f| f == "A Count"),
        "a split extension's alternative shares Rec only with its own base: {keyed_reads:?}"
    );
    // sol r3: and it does share with its own base: "Ws Only"'s own retrieval
    // gets "A Count".
    let own_reads: Vec<String> = routine(WS, "WsFind")
        .record_operations
        .iter()
        .filter(|op| op.op == "FindFirst")
        .flat_map(|op| op.flow_field_reads.iter().map(|r| r.field.clone()))
        .collect();
    assert!(
        own_reads.iter().any(|f| f == "A Count"),
        "the alternative shares Rec with its own base: {own_reads:?}"
    );
}

/// ENG-218: "the triggers of table T" (cross_rules' `table_triggers`) are
/// its own and those of the extensions whose stub says they extend T
/// (`ModelTable::extends_table_id`), not every tableextension of T's NAME:
/// ZBase's and Unrelated's `Shared` each have an extension with
/// `OnBeforeInsert`.
/// DISCRIMINATION (2026-11-27, each break made with Edit, run red, restored,
/// green): matching extensions by the extended name again fails "ZBase's
/// Shared: its own extension's trigger only"; `merge_extension_fields` not
/// setting `extends_table_id` fails it too (no extension trigger at all);
/// keeping every routine of a matching object id (not only the declaring
/// row's) fails "each base its own alternative's trigger"; pairing an id's
/// rows with its stubs in reverse fails "each table's trigger is its own
/// alternative's".
#[test]
fn table_triggers_are_those_of_the_extensions_of_this_table() {
    use al_sem::engine::l5::cross_rules::{CrossRules, CrossRulesIndex};
    let model = full_model();
    let ws = &model.workspace;
    let trigger_owner = |object: &str| {
        ws.routines
            .iter()
            .any(|r| r.object_id == object && r.name == "OnBeforeInsert")
    };
    let zbase_ext = format!("{ZBASE}/TableExtension/51");
    let unrelated_ext = format!("{UNRELATED}/TableExtension/63");
    // Precondition: both extensions' triggers are in the model.
    assert!(trigger_owner(&zbase_ext), "precondition: ZBase's trigger");
    assert!(
        trigger_owner(&unrelated_ext),
        "precondition: Unrelated's trigger"
    );
    let ix = CrossRulesIndex::new(&model);
    let rules = CrossRules::over(&model, &ix);
    let owners = |table: String| {
        let mut o: Vec<String> = rules
            .table_triggers(&table, None, "Insert")
            .into_iter()
            .map(|i| ws.routines[i].object_id.to_string())
            .collect();
        o.sort();
        o.dedup();
        o
    };
    assert_eq!(
        owners(format!("{ZBASE}/table/50")),
        vec![zbase_ext.clone()],
        "ZBase's Shared: its own extension's trigger only"
    );
    assert_eq!(
        owners(format!("{UNRELATED}/table/60")),
        vec![unrelated_ext.clone()],
        "Unrelated's Shared: its own extension's trigger only"
    );
    // sol r1: the workspace's split "Ws Moving Ext" has an `OnBeforeInsert`
    // in each `#if` alternative, one on "Ws Only" and one on "Keyed" (one
    // object id): each table gets its own alternative's trigger row only.
    let moving = format!("{WS}/TableExtension/50280");
    assert_eq!(
        ws.routines
            .iter()
            .filter(|r| r.object_id == moving && r.name == "OnBeforeInsert")
            .count(),
        2,
        "precondition: one trigger row per alternative"
    );
    let new_rows = rules.table_triggers(&format!("{WS}/table/50220"), None, "Insert");
    let old_rows = rules.table_triggers(&format!("{WS}/table/50230"), None, "Insert");
    assert_eq!(
        (new_rows.len(), old_rows.len()),
        (1, 1),
        "each base its own alternative's trigger: {new_rows:?} {old_rows:?}"
    );
    // Fable r2: and each row is its own base's alternative (the declaring row
    // extends that table), not merely a different row.
    let declaring = al_sem::program::model::declaring::DeclaringObjects::new(&ws.objects);
    let target = |rows: &[usize]| {
        declaring
            .of(&ws.routines[rows[0]])
            .and_then(|d| d.extends_target_name.clone())
    };
    assert_eq!(
        (target(&new_rows), target(&old_rows)),
        (Some("Ws Only".to_string()), Some("Keyed".to_string())),
        "each table's trigger is its own alternative's"
    );
}

/// ENG-218: `explain` of a tableextension field's `OnValidate` finds the
/// field on the table the extension's stub says it extends
/// (`explain::trigger_table`), not by a model-wide match of the extended
/// name: the workspace's "Ws NS Ext" extends `No. Series`, of which the model
/// holds two (Found's live one and ZOld's `Moved` one). `explain` takes only
/// workspace routines, and the workspace sees every app of the model, so a
/// moved table is where the stub and the name differ.
/// DISCRIMINATION (2026-11-27, break made with Edit, run red, restored,
/// green): `trigger_table` resolving the extended name model-wide again
/// fails "the field card names Found's table".
#[test]
fn explain_finds_an_extension_fields_table_through_its_stub() {
    let dir = fixture();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "explain",
            &dir.path().to_string_lossy(),
            "--routine",
            r#"TableExtension "Ws NS Ext"."Ws Note"::OnValidate"#,
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "json ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let run_by = &v["cards"][0]["runBy"];
    assert_eq!(
        (run_by["kind"].as_str(), run_by["table"].as_str()),
        (Some("field-validate"), Some("No. Series")),
        "the field card names Found's table: {v:#}"
    );
}

/// Run `alsem explain --format json` on the fixture for `routine`.
fn explain_json(routine: &str) -> serde_json::Value {
    let dir = fixture();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "explain",
            &dir.path().to_string_lossy(),
            "--routine",
            routine,
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "json ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// ENG-218 (sol r1, Fable r1): a field trigger of an `#if`-split
/// tableextension takes ITS OWN alternative's base: "Ws Split Ext" extends
/// "Not Loaded" in its first alternative and `No. Series` in the other.
/// explain's one card for the routine is its first declaring row's, whose
/// base is not known, so the card names no table (the other alternative's
/// known base is no answer for it).
/// DISCRIMINATION (2026-11-27, break made with Edit, run red, restored,
/// green): taking any alternative's known base (the stubs' first `Some`, as
/// the first ENG-218 head did) gives "field-validate" on `No. Series` and
/// fails "the unknown alternative names no table".
#[test]
fn explain_takes_each_alternatives_own_base() {
    let v = explain_json(r#"TableExtension "Ws Split Ext"."Ws Opt"::OnValidate"#);
    assert_eq!(
        v["cards"][0]["runBy"]["kind"].as_str(),
        Some("not-listed"),
        "the unknown alternative names no table: {v:#}"
    );
}

/// ENG-218 (sol r2): "Ws Two Base Ext" extends "Ws Only" in one `#if`
/// alternative and "Keyed" in the other, both known. explain's one card
/// covers both rows, so it names neither table rather than listing the other
/// alternative's sites under the first one's.
/// DISCRIMINATION (2026-11-27, break made with Edit, run red, restored,
/// green): reading only the card's first row gives "field-validate" on
/// "Ws Only" and fails "alternatives with different bases name no table".
#[test]
fn explain_declines_a_card_whose_alternatives_extend_different_tables() {
    let v = explain_json(r#"TableExtension "Ws Two Base Ext"."Ws Two"::OnValidate"#);
    assert_eq!(
        v["cards"][0]["runBy"]["kind"].as_str(),
        Some("not-listed"),
        "alternatives with different bases name no table: {v:#}"
    );
}
