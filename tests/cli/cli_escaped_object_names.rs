//! #240 / #249: the model holds an object's and a table's LOGICAL name (`A "B" C`
//! for `"A ""B"" C"`), and every lookup that matches a WRITTEN reference keeps
//! binding the same object. A written reference is raw source text (`""` still
//! doubled) and is unescaped exactly once; a name that is already logical (a
//! model name, dependency symbol text) is used as is (`NameRef`).
//!
//! - T5: the R0 identity snapshot and the L2 projection name the object as the
//!   model does, and R0's signature hash is the logical one.
//! - T6: two tables whose logical names differ only by an escape (`A"B`,
//!   `A""B`) are each bound by their own written reference, and a logical name
//!   is never unescaped a second time.
//! - T7: every raw reference shape (`extends`, `SourceTable`, a page extension's
//!   base page, `TableNo`, `Codeunit.Run(Codeunit::"…")`) binds the escaped object.
//! - T8: a workspace reference written with `""` binds a dependency table whose
//!   symbol name is logical.

use std::path::Path;
use std::process::Command;

use al_sem::program::model::symbol_table::NameRef;
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

fn model(ws: &Path) -> al_sem::program::model::workspace::Model {
    let inputs = al_sem::program::inputs::InputSet::read(ws, &Default::default());
    al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(&inputs, MI, true)
        .expect("model")
}

/// The resolved table id of every record op on `var` in the routine `routine` of
/// the object `object_id`.
fn op_tables(
    model: &al_sem::program::model::workspace::Model,
    object_id: &str,
    routine: &str,
    var: &str,
) -> Vec<Option<String>> {
    let r = model
        .workspace
        .routines
        .iter()
        .find(|r| r.object_id == object_id && r.name == routine)
        .unwrap_or_else(|| panic!("routine {object_id}::{routine}"));
    let ops: Vec<Option<String>> = r
        .record_operations
        .iter()
        .filter(|o| o.record_variable_name.eq_ignore_ascii_case(var))
        .map(|o| o.table_id.clone())
        .collect();
    assert!(
        !ops.is_empty(),
        "precondition: {routine} has an op on {var}"
    );
    ops
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
/// Discrimination: (i) treat every name as logical (the `Raw` arm returns its
/// text as is) -> R1's `A""B` matches 50111's logical name; fails. (ii) unescape
/// `Logical` names too -> `table_by_name(Logical("A\"\"B"))` returns 50110 and
/// table 50111's own `Rec.Insert()` binds 50110; fails. Each restored, it passes.
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
        ("R1", op_tables(&m, &cu, "Go", "R1")[0].clone()),
        ("R2", op_tables(&m, &cu, "Go", "R2")[0].clone()),
        (
            "Logical lookup",
            m.table_by_name(NameRef::Logical("A\"\"B"))
                .map(|t| t.stable_table_id()),
        ),
        (
            "implicit Rec of table 50111",
            op_tables(&m, &format!("{GUID}/Table/50111"), "Touch", "Rec")[0].clone(),
        ),
    ];
    let want = vec![
        ("R1", table(50110)),
        ("R2", table(50111)),
        ("Logical lookup", Some(format!("{GUID}:Table:50111"))),
        ("implicit Rec of table 50111", table(50111)),
    ];
    assert_eq!(got, want);
}

/// T7. One escaped table, `T ""Q"" X`, reached through every raw reference shape
/// the model resolves itself: a TableExtension's `extends` (its field merges onto
/// the base table), a Page's `SourceTable`, a PageExtension of an escaped page
/// (`extends`, then the base page's `SourceTable`), a codeunit's `TableNo`, and a
/// `Codeunit.Run(Codeunit::"C ""Q"" U")` target object.
///
/// Pass 2b of `record_types` (a variable outside `record_variables`) reads the
/// same declared type text pass 1 reads, through the same `NameRef`; no source
/// shape reaches it once pass 1 resolves, so it has no assertion of its own.
///
/// Discrimination: break the `Raw` arm (return its text unescaped) -> each
/// assertion is checked under the break; see the ledger proof table for which
/// fail and which (if any) are resolved elsewhere.
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
    let m = model(ws);

    // Every shape is collected first and compared once, so a failure shows each
    // shape's value (F5: which shapes a break moves).
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
    let got = vec![
        (
            "TableExtension extends: field merged",
            Some(merged.to_string()),
        ),
        (
            "Page SourceTable",
            op_tables(&m, &format!("{GUID}/Page/50130"), "OnOpenPage", "Rec")[0].clone(),
        ),
        (
            "PageExtension base page",
            op_tables(
                &m,
                &format!("{GUID}/PageExtension/50140"),
                "OnOpenPage",
                "Rec",
            )[0]
            .clone(),
        ),
        (
            "Codeunit TableNo",
            op_tables(&m, &format!("{GUID}/Codeunit/50150"), "OnRun", "Rec")[0].clone(),
        ),
        (
            "Codeunit.Run target",
            runs.first().and_then(|(_, t)| t.clone()),
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
    ];
    assert_eq!(runs.len(), 1, "one object run: {runs:?}");
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
/// Discrimination: break the `Raw` arm (no unescape) -> the write has no table;
/// fails. Restored, it passes.
#[test]
fn escaped_reference_binds_a_dependency_table() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    app_json(
        ws,
        &format!(r#"{{"id":"{DEP_GUID}","name":"XBase","publisher":"probe","version":"1.0.0.0"}}"#),
    );
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
