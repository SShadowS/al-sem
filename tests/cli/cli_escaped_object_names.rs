//! #240 / #249: the model holds an object's and a table's LOGICAL name (`A "B" C`
//! for `"A ""B"" C"`), and so does every reference it stores (`extends`,
//! `SourceTable`, a record variable's table, `TableNo`, a dataitem table, a
//! `Codeunit.Run` target). Each is unescaped exactly once where it is produced
//! (source text keeps a `""`; dependency symbol text is already logical unless it
//! is source-shaped), so the lookups compare logical with logical.
//!
//! - T5: the R0 identity snapshot and the L2 projection name the object as the
//!   model does, and R0's signature hash is the logical one.
//! - T6: two tables whose logical names differ only by an escape (`A"B`,
//!   `A""B`) are each bound by their own written reference, and a logical name
//!   is never unescaped a second time.
//! - T7: every source reference shape binds the escaped object, including a name
//!   that ends in an escaped quote (`"Q ""X"""`).
//! - T8: a workspace reference written with `""` binds a dependency table whose
//!   symbol name is logical.
//! - T8b: a dependency parameter typed in the already-quoted shape
//!   (`Record "A""B"`, source text) binds `A"B`, not `A""B`.

use std::path::Path;
use std::process::Command;

use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
use serde_json::Value;

const GUID: &str = "24024024-0000-0000-0000-0000000002a0";
const DEP_GUID: &str = "24024024-0000-0000-0000-0000000002de";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn app_json(dir: &Path, dependencies: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Ws240","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{dependencies}]}}"#
        ),
    );
}

fn dep_dependency() -> String {
    format!(r#"{{"id":"{DEP_GUID}","name":"XBase","publisher":"probe","version":"1.0.0.0"}}"#)
}

fn model(ws: &Path) -> al_sem::program::model::workspace::Model {
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
    al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(&inputs, MI, true)
        .expect("model")
}

/// The resolved table id of the first record op on `var` in the routine
/// `routine` of the object `object_id`.
fn op_table(
    model: &al_sem::program::model::workspace::Model,
    object_id: &str,
    routine: &str,
    var: &str,
) -> Option<String> {
    let r = model
        .workspace
        .routines
        .iter()
        .find(|r| r.object_id == object_id && r.name == routine)
        .unwrap_or_else(|| panic!("routine {object_id}::{routine}"));
    r.record_operations
        .iter()
        .find(|o| o.record_variable_name.eq_ignore_ascii_case(var))
        .unwrap_or_else(|| panic!("precondition: {routine} has an op on {var}"))
        .table_id
        .clone()
}

fn table(n: i64) -> Option<String> {
    Some(format!("{GUID}/table/{n}"))
}

/// T5. R0 (`engine::snapshot`) and L2 (`l2_workspace`) name the escaped interface
/// `A "B" C`, as the model does, and R0's signature hash is the literal
/// `sha256("Interface|0|A \"B\" C")` (the model contract's, T4).
///
/// Discrimination: revert the R0 producer alone (`name = obj.name.clone()`) ->
/// R0's name and hash fail; revert the L2 producer alone (`name = o.name.clone()`)
/// -> L2's name fails. Each restored, it passes.
#[test]
fn r0_and_l2_name_the_object_as_the_model_does() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/Iface.Interface.al"),
        "interface \"A \"\"B\"\" C\"\n{\n    procedure P();\n}\n",
    );

    let r0 = al_sem::engine::snapshot::snapshot_workspace(ws).unwrap();
    let r0: Vec<(&str, &str)> = r0
        .objects
        .iter()
        .filter(|o| o.kind == "Interface")
        .map(|o| (o.name.as_str(), o.signature_fingerprint.as_str()))
        .collect();
    assert_eq!(
        r0,
        vec![(
            "A \"B\" C",
            "e9c79d08c44fa746cf5789db704e01146d67291e7228b0d33f050e449979c616"
        )],
        "R0"
    );

    let l2 = al_sem::program::body::l2_workspace::project_workspace(ws).unwrap();
    let l2: Vec<&str> = l2
        .objects
        .iter()
        .filter(|o| o.object_type == "Interface")
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(l2, vec!["A \"B\" C"], "L2");

    let m = model(ws);
    let names: Vec<&str> = m
        .workspace
        .objects
        .iter()
        .filter(|o| o.object_type == "Interface")
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(names, vec!["A \"B\" C"], "model");
}

/// T6. `"A""B"` is the table `A"B` (50110); `"A""""B"` is the table `A""B`
/// (50111). Each written reference binds its own table, and the logical name
/// `A""B` (a model name) binds 50111, never 50110.
///
/// Discrimination: (i) drop the unescape of a record variable's table name
/// (`parse_record_table_name`) -> R1's `A""B` matches 50111; fails. (ii) a lookup
/// that unescapes its (already logical) input -> the logical lookup and table
/// 50111's own `Rec.Insert()` bind 50110; fails. Each restored, it passes.
#[test]
fn colliding_written_names_bind_their_own_table() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/A.Table.al"),
        "table 50110 \"A\"\"B\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
    );
    write(
        &ws.join("src/B.Table.al"),
        "table 50111 \"A\"\"\"\"B\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n\n    procedure Touch()\n    begin\n        Rec.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/W.Codeunit.al"),
        "codeunit 50112 \"W\"\n{\n    procedure Go()\n    var\n        R1: Record \"A\"\"B\";\n        R2: Record \"A\"\"\"\"B\";\n    begin\n        R1.Insert();\n        R2.Insert();\n    end;\n}\n",
    );
    let m = model(ws);
    let cu = format!("{GUID}/Codeunit/50112");
    // Collected first and compared once, so a failure shows every value.
    let got = vec![
        ("R1", op_table(&m, &cu, "Go", "R1")),
        ("R2", op_table(&m, &cu, "Go", "R2")),
        (
            "logical lookup",
            m.table_by_name("A\"\"B").map(|t| t.stable_table_id()),
        ),
        (
            "implicit Rec of table 50111",
            op_table(&m, &format!("{GUID}/Table/50111"), "Touch", "Rec"),
        ),
    ];
    let want = vec![
        ("R1", table(50110)),
        ("R2", table(50111)),
        ("logical lookup", Some(format!("{GUID}:Table:50111"))),
        ("implicit Rec of table 50111", table(50111)),
    ];
    assert_eq!(got, want);
}

/// T7. Two escaped tables, `T ""Q"" X` and `Q ""X"""` (logical `Q "X"`, whose
/// name ends in an escaped quote), reached through every source reference shape
/// the model resolves itself: a TableExtension's `extends` (its field merges onto
/// the base table), a Page's `SourceTable`, a PageExtension of an escaped page
/// (`extends`, then the base page's `SourceTable`), a codeunit's `TableNo`, a
/// record variable, a report dataitem's table (its trigger's `Rec`, and the
/// dataitem's own name used as a record variable), and a
/// `Codeunit.Run(Codeunit::"C ""Q"" U")` target object.
///
/// Pass 2b of `record_types` (a variable outside `record_variables`) reads the
/// declared type through `record_table_name_of`; no source shape reaches it once
/// pass 1 resolves, so its unescape is pinned by `record_types`' unit test.
///
/// Discrimination: drop the unescape at each producer; see the ledger proof
/// table. Every shape is collected and compared once, so a failure shows which
/// shapes a break moves.
#[test]
fn every_raw_reference_shape_binds_the_escaped_object() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, "");
    write(
        &ws.join("src/T.Table.al"),
        "table 50100 \"T \"\"Q\"\" X\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
    );
    write(
        &ws.join("src/TExt.TableExt.al"),
        "tableextension 50120 \"T Ext\" extends \"T \"\"Q\"\" X\"\n{\n    fields\n    {\n        field(50120; \"Ext F\"; Integer)\n        {\n        }\n    }\n}\n",
    );
    write(
        &ws.join("src/P.Page.al"),
        "page 50130 \"T \"\"P\"\" Page\"\n{\n    SourceTable = \"T \"\"Q\"\" X\";\n\n    trigger OnOpenPage()\n    begin\n        Rec.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/PExt.PageExt.al"),
        "pageextension 50140 \"T Page Ext\" extends \"T \"\"P\"\" Page\"\n{\n    trigger OnOpenPage()\n    begin\n        Rec.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/Runner.Codeunit.al"),
        "codeunit 50150 \"T Runner\"\n{\n    TableNo = \"T \"\"Q\"\" X\";\n\n    trigger OnRun()\n    begin\n        Rec.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/Target.Codeunit.al"),
        "codeunit 50151 \"C \"\"Q\"\" U\"\n{\n    procedure Noop()\n    begin\n    end;\n}\n",
    );
    write(
        &ws.join("src/Caller.Codeunit.al"),
        "codeunit 50152 \"Caller\"\n{\n    procedure Go()\n    begin\n        Codeunit.Run(Codeunit::\"C \"\"Q\"\" U\");\n    end;\n}\n",
    );
    // A name that ENDS in an escaped quote: `trim_matches('"')` would eat it.
    write(
        &ws.join("src/Q.Table.al"),
        "table 50160 \"Q \"\"X\"\"\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
    );
    write(
        &ws.join("src/QRunner.Codeunit.al"),
        "codeunit 50161 \"Q Runner\"\n{\n    TableNo = \"Q \"\"X\"\"\";\n\n    trigger OnRun()\n    begin\n        Rec.Insert();\n    end;\n\n    procedure UseVar()\n    var\n        R: Record \"Q \"\"X\"\"\";\n    begin\n        R.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/QPage.Page.al"),
        "page 50162 \"Q Page\"\n{\n    SourceTable = \"Q \"\"X\"\"\";\n\n    trigger OnOpenPage()\n    begin\n        Rec.Insert();\n    end;\n}\n",
    );
    write(
        &ws.join("src/R.Report.al"),
        "report 50170 \"T Report\"\n{\n    dataset\n    {\n        dataitem(D; \"T \"\"Q\"\" X\")\n        {\n            trigger OnAfterGetRecord()\n            begin\n                Rec.Insert();\n            end;\n        }\n    }\n\n    trigger OnPreReport()\n    begin\n        D.Insert();\n    end;\n}\n",
    );
    let m = model(ws);
    // A named dataitem is a record variable in every routine of the report. The
    // body walk does not record an op on a dataitem receiver (`D.Insert()` here),
    // so the variable's own table name and resolved table are asserted.
    let dataitem_var = m
        .workspace
        .routines
        .iter()
        .find(|r| r.object_id == format!("{GUID}/Report/50170") && r.name == "OnPreReport")
        .expect("routine OnPreReport")
        .record_variables
        .iter()
        .find(|v| v.name == "D")
        .map(|v| (v.table_name.clone(), v.table_id.clone()));

    let base = m
        .workspace
        .tables
        .iter()
        .find(|t| t.id == table(50100).unwrap())
        .expect("table 50100");
    let merged = base.fields.iter().any(|f| f.field_number == 50120);
    let r3a1 = m.project_r3a1_combined_graph();
    let runs: Vec<(String, Option<String>)> = r3a1
        .typed_edges
        .iter()
        .filter(|e| e.kind.starts_with("object-run"))
        .map(|e| (e.kind.clone(), e.target_object.clone()))
        .collect();
    assert_eq!(runs.len(), 1, "one object run: {runs:?}");
    let got = vec![
        (
            "TableExtension extends: field merged",
            Some(merged.to_string()),
        ),
        (
            "Page SourceTable",
            op_table(&m, &format!("{GUID}/Page/50130"), "OnOpenPage", "Rec"),
        ),
        (
            "PageExtension base page",
            op_table(
                &m,
                &format!("{GUID}/PageExtension/50140"),
                "OnOpenPage",
                "Rec",
            ),
        ),
        (
            "Codeunit TableNo",
            op_table(&m, &format!("{GUID}/Codeunit/50150"), "OnRun", "Rec"),
        ),
        (
            "Codeunit.Run target",
            runs.first().and_then(|(_, t)| t.clone()),
        ),
        (
            "trailing escape: TableNo",
            op_table(&m, &format!("{GUID}/Codeunit/50161"), "OnRun", "Rec"),
        ),
        (
            "trailing escape: record variable",
            op_table(&m, &format!("{GUID}/Codeunit/50161"), "UseVar", "R"),
        ),
        (
            "trailing escape: Page SourceTable",
            op_table(&m, &format!("{GUID}/Page/50162"), "OnOpenPage", "Rec"),
        ),
        (
            "Report dataitem table",
            op_table(
                &m,
                &format!("{GUID}/Report/50170"),
                "OnAfterGetRecord",
                "Rec",
            ),
        ),
    ];
    let want = vec![
        (
            "TableExtension extends: field merged",
            Some("true".to_string()),
        ),
        ("Page SourceTable", table(50100)),
        ("PageExtension base page", table(50100)),
        ("Codeunit TableNo", table(50100)),
        (
            "Codeunit.Run target",
            Some(format!("{GUID}:Codeunit:50151")),
        ),
        ("trailing escape: TableNo", table(50160)),
        ("trailing escape: record variable", table(50160)),
        ("trailing escape: Page SourceTable", table(50160)),
        ("Report dataitem table", table(50100)),
    ];
    assert_eq!(
        dataitem_var,
        Some((Some("T \"Q\" X".to_string()), table(50100))),
        "the named dataitem's record variable: logical table name and its table"
    );
    assert_eq!(got, want);
}

/// `alsem <args> --deterministic --format json` -> (stdout as JSON, exit code).
fn alsem_json(args: &[&str]) -> (Value, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .args(["--deterministic", "--format", "json"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", "cli-escaped-names-v1")
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    (
        serde_json::from_str(&stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: {stdout} stderr={}",
                String::from_utf8_lossy(&out.stderr)
            )
        }),
        out.status.code().unwrap_or(-1),
    )
}

/// T8. A symbol-only dependency declares table 18 `T "Q" X` (its SymbolReference
/// name is logical); the workspace writes `Record "T ""Q"" X"`. The digest binds
/// the write to the dependency's table and names it logically. On base it missed
/// (raw `T ""Q"" X` against logical `T "Q" X`).
///
/// Discrimination: drop the unescape in `parse_record_table_name` -> the write
/// has no table; fails. Restored, it passes.
#[test]
fn escaped_reference_binds_a_dependency_table() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, &dep_dependency());
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_XBase_1.0.0.0.app"),
        DEP_GUID,
        "XBase",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"T \"Q\" X","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"AppId":"{DEP_GUID}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    write(
        &ws.join("src/Work.al"),
        "codeunit 50100 \"Ws Work\"\n{\n    procedure DoWork()\n    var\n        R: Record \"T \"\"Q\"\" X\";\n    begin\n        R.Insert();\n    end;\n}\n",
    );
    let (doc, code) = alsem_json(&["digest", ws.to_str().unwrap(), "--file", "src/Work.al"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["payload"]["dependencies"]["mode"], "crossApp", "{doc}");
    let mut res = Vec::new();
    for e in doc["payload"]["entries"].as_array().unwrap() {
        for f in e["effects"].as_array().unwrap() {
            res.push((
                f["type"].as_str().unwrap_or("").to_string(),
                f["detail"]["resourceId"].as_str().unwrap_or("").to_string(),
                f["detail"]["resourceDisplay"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            ));
        }
    }
    assert_eq!(
        res,
        vec![(
            "DB_INSERT".to_string(),
            format!("{DEP_GUID}/table/18"),
            "T \"Q\" X".to_string()
        )],
        "{doc}"
    );
}

/// T8b (final panel F1). A symbol-only dependency declares tables 50110 `A"B` and
/// 50111 `A""B` (logical SymbolReference names) and a routine whose parameter
/// type is the already-quoted, source-shaped `Record "A""B"`, i.e. the table
/// `A"B`. In the cross-app model that parameter's record variable binds 50110.
///
/// Discrimination: drop the unescape in `record_types::record_table_name_of` ->
/// the parameter binds 50111 (`A""B` taken as already logical); fails.
#[test]
fn already_quoted_dependency_parameter_binds_its_logical_table() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(ws, &dep_dependency());
    let field = r#"[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]"#;
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_XBase_1.0.0.0.app"),
        DEP_GUID,
        "XBase",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":50110,"Name":"A\"B","Fields":{field}}},{{"Id":50111,"Name":"A\"\"B","Fields":{field}}}],"Codeunits":[{{"Id":10,"Name":"Dep Lib","Methods":[{{"Name":"Take","Parameters":[{{"Name":"R","IsVar":true,"TypeDefinition":{{"Name":"Record \"A\"\"B\""}}}}]}}]}}],"AppId":"{DEP_GUID}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    write(
        &ws.join("src/Work.al"),
        "codeunit 50100 \"Ws Work\"\n{\n    procedure DoWork()\n    var\n        Lib: Codeunit \"Dep Lib\";\n        X: Record \"A\"\"B\";\n    begin\n        Lib.Take(X);\n    end;\n}\n",
    );
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
    let cross = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs, MI, true,
    )
    .expect("cross-app model");
    let take = cross
        .resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Take")
        .expect("precondition: the dependency routine is in the cross-app model");
    let rv: Vec<(Option<&str>, Option<&str>)> = take
        .record_variables
        .iter()
        .filter(|v| v.name == "R")
        .map(|v| (v.table_name.as_deref(), v.table_id.as_deref()))
        .collect();
    let want_id = format!("{DEP_GUID}/table/50110");
    assert_eq!(rv, vec![(Some("A\"B"), Some(want_id.as_str()))]);
}
