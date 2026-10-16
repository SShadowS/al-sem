//! #78: every digest entry, fingerprint block and prove document names its
//! owning object as `"<ObjectType> <ObjectName>"`, the same value
//! `alsem query effects` prints (`RoutineJoin::of`), taken from the Model and
//! never from the snapshot's identity table.
//!
//! The fixture holds a Table and a TableExtension with the SAME number (50100),
//! the extension's file sorting after the table's, so the identity table keeps
//! the extension's name at `<guid>:Table:50100`. A test that took the owner from
//! the identity table would print `OD Item Ext` for the real table's routines.
//! Two Interfaces share the object id `<guid>/Interface/0`, so their owners
//! must come from the declaring object.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

const VERSION_OVERRIDE: &str = "cli-object-display-v1";
const GUID: &str = "7a1e0000-0000-0000-0000-000000000078";

const TABLE_OWNER: &str = "Table OD Item";
const TABLE_EXT_OWNER: &str = "TableExtension OD Item Ext";
const CODEUNIT_OWNER: &str = "Codeunit OD Cu";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// (file, expected digest roots by display, the literal owner of each).
fn files() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "src/a_Table.al",
            vec![
                ("OnInsert", TABLE_OWNER),
                ("OnValidate", TABLE_OWNER),
                ("TableProc", TABLE_OWNER),
            ],
        ),
        ("src/b_TableExt.al", vec![("ExtProc", TABLE_EXT_OWNER)]),
        ("src/c_Codeunit.al", vec![("CuProc", CODEUNIT_OWNER)]),
        ("src/d_IFoo.al", vec![("FooMethod", "Interface IFoo")]),
        ("src/e_IBar.al", vec![("BarMethod", "Interface IBar")]),
    ]
}

fn owner_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{GUID}","name":"Issue78Owner","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50100,"to":50199}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/a_Table.al"),
        "table 50100 \"OD Item\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n            trigger OnValidate()\n            begin\n                Message('v');\n            end;\n        }\n    }\n\n    trigger OnInsert()\n    begin\n        Message('i');\n    end;\n\n    procedure TableProc()\n    begin\n        Message('t');\n    end;\n}\n",
    );
    write(
        &dir.join("src/b_TableExt.al"),
        "tableextension 50100 \"OD Item Ext\" extends \"OD Item\"\n{\n    procedure ExtProc()\n    begin\n        Message('e');\n    end;\n}\n",
    );
    write(
        &dir.join("src/c_Codeunit.al"),
        "codeunit 50101 \"OD Cu\"\n{\n    procedure CuProc()\n    begin\n        Message('c');\n    end;\n}\n",
    );
    write(
        &dir.join("src/d_IFoo.al"),
        "interface IFoo\n{\n    procedure FooMethod();\n}\n",
    );
    write(
        &dir.join("src/e_IBar.al"),
        "interface IBar\n{\n    procedure BarMethod();\n}\n",
    );
}

/// Spawn `alsem <args>` -> (stdout parsed as JSON, exit code).
fn alsem(args: &[&str]) -> (Value, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: args={args:?} stdout={stdout} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (doc, out.status.code().unwrap_or(-1))
}

/// `alsem digest <ws> --file <file> --deterministic --format json`.
fn digest_file(ws: &Path, file: &str) -> (Value, i32) {
    alsem(&[
        "digest",
        ws.to_str().unwrap(),
        "--file",
        file,
        "--deterministic",
        "--format",
        "json",
    ])
}

fn entries(doc: &Value) -> Vec<Value> {
    doc["payload"]["entries"].as_array().unwrap().clone()
}

/// The stable id of the one digest entry of `file` displayed as `display`.
fn stable_id_of(ws: &Path, file: &str, display: &str) -> String {
    let (doc, code) = digest_file(ws, file);
    assert_eq!(code, 0, "{doc}");
    let hits: Vec<Value> = entries(&doc)
        .into_iter()
        .filter(|e| e["routine"]["display"] == display)
        .collect();
    assert_eq!(hits.len(), 1, "one `{display}` entry expected: {doc}");
    hits[0]["routine"]["stableId"].as_str().unwrap().to_string()
}

/// PRECONDITION shared by A1/A2/A4: the snapshot's identity table holds the
/// EXTENSION's name at the real table's id, so an identity-table owner would be
/// caught.
fn assert_identity_table_holds_extension_name(ws: &Path) {
    let model_id = compute_gate_model_instance_id(ws).unwrap();
    let model = assemble_and_resolve_workspace_program(ws, &model_id, false).unwrap();
    let snap = compose_snapshot(&model);
    let ids = &snap.identities;
    let key = format!("{GUID}:Table:50100");
    let at = ids
        .stable_ids
        .iter()
        .position(|s| *s == key)
        .unwrap_or_else(|| panic!("precondition: `{key}` in identities: {:?}", ids.stable_ids));
    assert_eq!(
        ids.display_names[at], "OD Item Ext",
        "precondition: the extension stub overwrote the table's identity name"
    );
}

/// A1. Every digest root's `routine.objectDisplay` is the literal owner, and for
/// every non-declaration-only root it equals `query effects`' `objectDisplay`.
///
/// Mutations: (i) keep the old `split_object_routine` -> every value is `""`;
/// (ii) take the owner from the identity table -> the Table's three routines
/// read `OD Item Ext`, and IFoo/IBar share one name.
#[test]
fn digest_entries_name_their_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_extension_name(ws);

    let mut got: Vec<(String, String, String)> = Vec::new();
    let mut want: Vec<(String, String, String)> = Vec::new();
    let mut docs = String::new();
    for (file, expected) in files() {
        let (doc, code) = digest_file(ws, file);
        assert_eq!(code, 0, "{file}: {doc}");
        let es = entries(&doc);
        let mut displays: Vec<&str> = es
            .iter()
            .map(|e| e["routine"]["display"].as_str().unwrap())
            .collect();
        displays.sort();
        let mut exp_displays: Vec<&str> = expected.iter().map(|(d, _)| *d).collect();
        exp_displays.sort();
        assert_eq!(
            displays, exp_displays,
            "precondition: roots of {file}: {doc}"
        );

        for e in &es {
            let r = &e["routine"];
            let display = r["display"].as_str().unwrap();
            let owner = expected.iter().find(|(d, _)| *d == display).unwrap().1;
            let decl_only = r["declarationOnly"] == true;
            assert_eq!(
                decl_only,
                owner.starts_with("Interface "),
                "precondition: declarationOnly of {display}: {doc}"
            );
            if !decl_only {
                // PRECONDITION: the literal is what `query effects` prints.
                let id = r["stableId"].as_str().unwrap();
                let (q, qcode) = alsem(&[
                    "query",
                    "effects",
                    ws.to_str().unwrap(),
                    "--routine",
                    id,
                    "--format",
                    "json",
                ]);
                assert_eq!(qcode, 0, "{q}");
                assert_eq!(
                    q["payload"]["routine"]["routine"]["objectDisplay"], owner,
                    "precondition: query effects owner of {display}: {q}"
                );
            }
            got.push((
                file.to_string(),
                display.to_string(),
                r["objectDisplay"]
                    .as_str()
                    .unwrap_or("<missing>")
                    .to_string(),
            ));
            want.push((file.to_string(), display.to_string(), owner.to_string()));
        }
        docs.push_str(&format!("\n{file}: {doc}"));
    }
    got.sort();
    want.sort();
    assert_eq!(got, want, "digest objectDisplay per root:{docs}");
    for (file, display, od) in &got {
        assert!(
            !od.is_empty(),
            "{file} {display}: empty objectDisplay:{docs}"
        );
    }
}

/// A2. The fingerprint query block of the REAL table's `OnInsert` trigger names
/// `Table OD Item` (the identity table would say `OD Item Ext`).
///
/// Mutations: as A1 (i) -> `""`; (ii) -> `OD Item Ext`.
#[test]
fn fingerprint_block_names_its_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_extension_name(ws);
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    assert!(id.starts_with(&format!("{GUID}:Table:50100#")), "{id}");

    // `--routine` makes it a query, so `--format json` yields blocks (not the
    // full-snapshot envelope).
    let (doc, code) = alsem(&[
        "fingerprint",
        ws.to_str().unwrap(),
        "--format",
        "json",
        "--routine",
        &id,
        "--deterministic",
    ]);
    assert_eq!(code, 0, "{doc}");
    let blocks: Vec<&Value> = doc["payload"]["blocks"]
        .as_array()
        .unwrap_or_else(|| panic!("blocks: {doc}"))
        .iter()
        .filter(|b| b["routine"]["stableId"] == id.as_str())
        .collect();
    assert_eq!(blocks.len(), 1, "precondition: one OnInsert block: {doc}");
    let b = blocks[0];
    assert_eq!(b["routine"]["display"], "OnInsert", "{doc}");
    assert_eq!(
        b["rootKinds"],
        serde_json::json!(["trigger-table"]),
        "precondition: OnInsert is root-classified: {doc}"
    );
    assert_eq!(b["routine"]["objectDisplay"], TABLE_OWNER, "{doc}");
}

/// A4. `alsem prove` on the real table's `OnInsert`, selected by its stable id,
/// names `Table OD Item`.
///
/// Mutations: as A1 (i) -> `""`; (ii) -> `OD Item Ext`.
#[test]
fn prove_names_its_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    assert_identity_table_holds_extension_name(ws);
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    assert!(id.starts_with(&format!("{GUID}:Table:50100#")), "{id}");

    let (doc, code) = alsem(&[
        "prove",
        ws.to_str().unwrap(),
        &id,
        "may-commit",
        "--deterministic",
    ]);
    // PRECONDITION: answered (exit 2 prints `objectDisplay: ""` for another reason).
    assert_eq!(code, 0, "{doc}");
    let r = &doc["payload"]["routine"];
    assert_eq!(r["stableId"], id.as_str(), "{doc}");
    assert_eq!(r["display"], "OnInsert", "{doc}");
    assert_eq!(r["objectDisplay"], TABLE_OWNER, "{doc}");
}

/// Spawn `alsem <args>` -> (stdout text, exit code).
fn alsem_text(args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    (stdout, out.status.code().unwrap_or(-1))
}

/// A5. The human headers of digest, fingerprint and prove print
/// `<owner>::<routine>` for the real table's `OnInsert` (digest and prove as
/// `--- … ---`, fingerprint as the block's first line).
///
/// Mutations: a renderer that prints only the routine, or the owner from the
/// identity table (`OD Item Ext`), fails its assert.
#[test]
fn human_headers_name_the_owning_object() {
    let dir = tempfile::tempdir().unwrap();
    owner_workspace(dir.path());
    let ws = dir.path();
    let w = ws.to_str().unwrap();
    let id = stable_id_of(ws, "src/a_Table.al", "OnInsert");
    let header = format!("{TABLE_OWNER}::OnInsert");

    let (digest, code) = alsem_text(&[
        "digest",
        w,
        "--file",
        "src/a_Table.al",
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{digest}");
    assert!(
        digest.lines().any(|l| l == format!("--- {header} ---")),
        "{digest}"
    );

    let (fp, code) = alsem_text(&[
        "fingerprint",
        w,
        "--routine",
        &id,
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{fp}");
    assert!(
        fp.lines().any(|l| l.starts_with(&format!("{header}  ["))),
        "{fp}"
    );

    let (prove, code) = alsem_text(&[
        "prove",
        w,
        &id,
        "may-commit",
        "--deterministic",
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "{prove}");
    assert!(prove.contains(&header), "{prove}");
}
