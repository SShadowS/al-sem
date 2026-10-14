//! #71: `alsem digest --file` / `--diff` resolve changed roots by routine
//! DECLARATION SPAN (the attribute lines, the header and the whole body).
//!
//! This file grows with the #71 plan: Task 3 adds the model's declaration start,
//! Task 4 the declaration-only routines; the digest acceptance tests join it in
//! Task 5.
//! Ledger: `.agent/issue-71/ledger.md` (Design, Acceptance matrix).

use std::path::Path;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::ids::{
    normalized_signature_hash, to_stable_object_id, to_stable_routine_id_from_parts,
};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn corpus(fixture: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture)
}

/// The d8 fixture's `src/posting.al`, LF, as committed. `PostSalesDoc` is lines
/// 3-16, `[IntegrationEvent]` is line 18, `OnAfterPostSalesDoc` lines 19-21.
fn d8_posting() -> String {
    std::fs::read_to_string(corpus("ws-d8-commit-in-tx").join("src/posting.al"))
        .unwrap()
        .replace('\r', "")
}

/// Copy `ws-d8-commit-in-tx` into `dir`, with `posting.al` replaced by `posting`
/// (the post-change file text).
fn d8_workspace(dir: &Path, posting: &str) {
    let src = corpus("ws-d8-commit-in-tx");
    write(
        &dir.join("app.json"),
        &std::fs::read_to_string(src.join("app.json")).unwrap(),
    );
    write(
        &dir.join("src/subscriber.al"),
        &std::fs::read_to_string(src.join("src/subscriber.al"))
            .unwrap()
            .replace('\r', ""),
    );
    write(&dir.join("src/posting.al"), posting);
}

/// Plan Task 3: a routine's declaration starts at its first attached attribute;
/// `source_anchor` still starts at the keyword.
#[test]
fn model_routine_declaration_start_includes_attributes() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    // 0-based: `[IntegrationEvent]` row 17, `procedure OnAfterPostSalesDoc` row 18.
    let p = r("OnAfterPostSalesDoc");
    assert_eq!(
        (p.declaration_start_line, p.source_anchor.start_line),
        (17, 18)
    );
    let q = r("PostSalesDoc");
    assert_eq!(
        (q.declaration_start_line, q.source_anchor.start_line),
        (2, 2)
    );
}

/// #71 attribute-start limits: (a) an attribute separated from its routine by a
/// COMMENT is not claimed by `declaration_start_line` today (issue #80's lowerer
/// bug drops it; a #80 fix moves this assertion on purpose); (b) a multi-line
/// attribute's FIRST line is the declaration start.
#[test]
fn declaration_start_line_attribute_limits() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    write(
        &dir.path().join("src/Attrs.Codeunit.al"),
        "codeunit 50120 AttrsCu\n{\n    [IntegrationEvent(false, false)]\n    // note\n    procedure Commented()\n    begin\n    end;\n\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::AttrsCu,\n        'Commented', '', false, false)]\n    local procedure MultiLine()\n    begin\n    end;\n}\n",
    );
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name}: no routine"))
    };
    // 0-based: `[IntegrationEvent]` row 2, `// note` row 3, `procedure` row 4.
    let c = r("Commented");
    assert_eq!(
        (c.declaration_start_line, c.source_anchor.start_line),
        (4, 4),
        "#80: the comment-separated attribute is not claimed"
    );
    // 0-based: `[EventSubscriber(` row 8, its last line row 9, `procedure` row 10.
    let m = r("MultiLine");
    assert_eq!(
        (m.declaration_start_line, m.source_anchor.start_line),
        (8, 10)
    );
}

const DECL_GUID: &str = "7a1e0000-0000-0000-0000-000000000074";

fn decl_only_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{DECL_GUID}","name":"Issue71Decl","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{{"from":50500,"to":50599}}],"runtime":"14.0"}}"#
        ),
    );
    write(
        &dir.join("src/IFoo.Interface.al"),
        "interface IFoo\n{\n    procedure Bar();\n    procedure Baz(X: Integer);\n}\n",
    );
    write(
        &dir.join("src/IQux.Interface.al"),
        "interface IQux\n{\n    procedure Bar();\n}\n",
    );
    write(
        &dir.join("src/Add.ControlAddIn.al"),
        "controladdin MyAddIn\n{\n    procedure Init();\n}\n",
    );
    write(
        &dir.join("src/IEmpty.Interface.al"),
        "interface IEmpty\n{\n}\n",
    );
    write(
        &dir.join("src/Cu.Codeunit.al"),
        "codeunit 50500 DeclCu\n{\n    procedure Run()\n    begin\n        Message('x');\n    end;\n}\n",
    );
}

/// The id the design gives a declaration-only procedure without parameters or
/// return value: the object segment `<guid>:<Kind>:0` and the signature hash
/// folded with the declaring object's name.
fn decl_only_id(kind: &str, object: &str, procedure: &str) -> String {
    to_stable_routine_id_from_parts(
        &to_stable_object_id(&format!("{DECL_GUID}/{kind}/0")),
        &normalized_signature_hash(procedure, &[], None),
        Some(object),
    )
}

/// Plan Task 4: Interface/ControlAddIn procedures ride on their object row with a
/// name-folded stable id; they are still not `ModelRoutine`s.
#[test]
fn interface_object_rows_carry_declaration_only_routines() {
    let dir = tempfile::tempdir().unwrap();
    decl_only_workspace(dir.path());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let obj = |name: &str| {
        model
            .workspace
            .objects
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("{name}: no object row"))
    };
    let rows = |name: &str| -> Vec<(String, String, u32, u32)> {
        obj(name)
            .declaration_only_routines
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    d.stable_routine_id.clone(),
                    d.source_anchor.start_line,
                    d.declaration_start_line,
                )
            })
            .collect()
    };
    let ifoo = rows("IFoo");
    assert_eq!(ifoo.len(), 2, "{ifoo:?}");
    assert_eq!(
        ifoo[0],
        (
            "Bar".to_string(),
            decl_only_id("Interface", "IFoo", "Bar"),
            2,
            2
        )
    );
    assert_eq!((ifoo[1].0.as_str(), ifoo[1].2), ("Baz", 3));
    assert_ne!(ifoo[1].1, ifoo[0].1);
    assert_eq!(
        rows("IQux"),
        vec![(
            "Bar".to_string(),
            decl_only_id("Interface", "IQux", "Bar"),
            2,
            2
        )]
    );
    assert_eq!(
        rows("MyAddIn")[0].1,
        decl_only_id("ControlAddIn", "MyAddIn", "Init")
    );
    assert!(rows("IEmpty").is_empty());
    assert!(rows("DeclCu").is_empty());
    assert!(
        !model.workspace.routines.iter().any(|r| r.name == "Bar"),
        "the Interface skip stays"
    );
}
