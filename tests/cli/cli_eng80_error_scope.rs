//! ENG-80 review F2 (Fable r1): a bare `Error` is the builtin only when every scope that could
//! declare a `procedure Error` before the catalog was searched (`resolve_bare_with_args`'s
//! `scope_complete`). Each place that clears it, with a fixture: a `with` body, an extension
//! whose base is not loaded, an implicit `Rec` table loaded from symbols, a source table with a
//! symbol-only extension, and a report dataitem on a symbol-only table. In each the call may
//! raise: `Next` stays live and the may-raise node is kept. Controls: a codeunit, and a page on a
//! workspace table, read the builtin (`Next` dead).

use al_sem::program::body::features::PCFNNode;
use al_sem::program::model::workspace::ModelRoutine;

const SRC_GUID: &str = "22222222-0000-0000-0000-0000000000f0";
const EXT_GUID: &str = "22222222-0000-0000-0000-0000000000f1";
const SYM_GUID: &str = "22222222-0000-0000-0000-0000000000f2";

const SOURCE: &str = r#"table 50380 "E80 Ws T"
{
    fields { field(1; K; Integer) { } }
}

page 50381 "E80 Ws Page"
{
    SourceTable = "E80 Ws T";

    trigger OnOpenPage() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

page 50382 "E80 Src Page"
{
    SourceTable = "E80 Src T";

    trigger OnOpenPage() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

page 50383 "E80 Sym Page"
{
    SourceTable = "E80 Sym T";

    trigger OnOpenPage() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

pageextension 50384 "E80 Missing Ext" extends "E80 Missing Page"
{
    trigger OnOpenPage() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

report 50385 "E80 Sym Report"
{
    dataset
    {
        dataitem(D; "E80 Sym T")
        {
            trigger OnAfterGetRecord() begin Error('x'); Next(); end;
        }
    }

    local procedure Next() begin end;
}

page 50387 "E80 Int Page"
{
    SourceTable = "E80 Int T";

    trigger OnOpenPage() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

tableextension 50388 "E80 Int Ext" extends "E80 Int T"
{
    procedure ExtGo() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

codeunit 50386 "E80 Scope"
{
    procedure Plain() begin Error('x'); Next(); end;

    procedure WithRec()
    var
        R: Record "E80 Ws T";
    begin
        with R do
            Error('x');
        Next();
    end;

    local procedure Next() begin end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        format!(
            r#"{{"id":"e80e80e0-0000-4000-8000-0000000000f9","name":"E80 Host","publisher":"t","version":"1.0.0.0","dependencies":[{{"id":"{SRC_GUID}","name":"E80 Src","publisher":"t","version":"1.0.0.0"}},{{"id":"{EXT_GUID}","name":"E80 SymExt","publisher":"t","version":"1.0.0.0"}},{{"id":"{SYM_GUID}","name":"E80 Sym","publisher":"t","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    let pk = t.path().join(".alpackages");
    crate::symbol_app::write_source_app(
        &pk.join("t_E80 Src_1.0.0.0.app"),
        SRC_GUID,
        "E80 Src",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","AppId":"{SRC_GUID}","Name":"E80 Src","Publisher":"t","Version":"1.0.0.0","Tables":[{{"Id":70980,"Name":"E80 Src T","Fields":[{{"Id":1,"Name":"K","TypeDefinition":{{"Name":"Integer"}}}}]}},{{"Id":70983,"Name":"E80 Int T","Fields":[{{"Id":1,"Name":"K","TypeDefinition":{{"Name":"Integer"}}}}]}}]}}"#
        ),
        &[
            (
                "src/T.al",
                "table 70980 \"E80 Src T\"\n{\n    fields { field(1; K; Integer) { } }\n}\n",
            ),
            (
                "src/Int.al",
                "table 70983 \"E80 Int T\"\n{\n    fields { field(1; K; Integer) { } }\n\n    internal procedure Error(Msg: Text)\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
    crate::symbol_app::write_symbol_app(
        &pk.join("t_E80 SymExt_1.0.0.0.app"),
        EXT_GUID,
        "E80 SymExt",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","AppId":"{EXT_GUID}","Name":"E80 SymExt","Publisher":"t","Version":"1.0.0.0","TableExtensions":[{{"Id":70981,"Name":"E80 Src T Ext","TargetObject":"E80 Src T"}}]}}"#
        ),
    );
    crate::symbol_app::write_symbol_app(
        &pk.join("t_E80 Sym_1.0.0.0.app"),
        SYM_GUID,
        "E80 Sym",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","AppId":"{SYM_GUID}","Name":"E80 Sym","Publisher":"t","Version":"1.0.0.0","Tables":[{{"Id":70982,"Name":"E80 Sym T","Fields":[{{"Id":1,"Name":"K","TypeDefinition":{{"Name":"Integer"}}}}]}}]}}"#
        ),
    );
    t
}

/// The one workspace row of `object` named `name`.
fn row<'a>(rows: &'a [ModelRoutine], object: &str, name: &str) -> &'a ModelRoutine {
    let found: Vec<_> = rows
        .iter()
        .filter(|r| r.name == name && r.object_id.ends_with(object))
        .collect();
    assert_eq!(found.len(), 1, "{object}::{name}: one row");
    found[0]
}

fn next_dead(r: &ModelRoutine) -> bool {
    let next: Vec<_> = r
        .call_sites
        .iter()
        .filter(|c| c.callee_text == "Next")
        .collect();
    assert_eq!(next.len(), 1, "{}: one Next call", r.name);
    next[0].control_context.as_deref() == Some("unreachable")
}

fn has_may_raise(r: &ModelRoutine) -> bool {
    fn walk(n: &PCFNNode) -> bool {
        (n.kind == "error" && n.callsite_id.is_none())
            || [&n.children, &n.else_children, &n.condition_leaves]
                .into_iter()
                .flatten()
                .flatten()
                .any(walk)
    }
    r.statement_tree.as_deref().is_some_and(walk)
}

#[test]
fn an_unsearched_scope_leaves_a_bare_error_may_raise() {
    let t = workspace();
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(t.path(), &Default::default());
    let cross = al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs,
        true,
        al_sem::program::model::workspace::AssemblyNeeds::BASE,
    )
    .expect("cross-app model");
    let rows = &cross.resolved.workspace.routines;
    for (object, name) in [("50386", "Plain"), ("50381", "OnOpenPage")] {
        let r = row(rows, object, name);
        assert!(next_dead(r), "control {object}::{name}: the builtin");
    }
    let mut failures = Vec::new();
    for (object, name, what) in [
        ("50386", "WithRec", "a with body"),
        (
            "50382",
            "OnOpenPage",
            "a source table with a symbol-only extension",
        ),
        ("50383", "OnOpenPage", "a symbol-only implicit Rec table"),
        (
            "50384",
            "OnOpenPage",
            "an extension whose base is not loaded",
        ),
        (
            "50385",
            "OnAfterGetRecord",
            "a report dataitem on a symbol-only table",
        ),
        (
            "50387",
            "OnOpenPage",
            "an implicit Rec table's access-excluded Error",
        ),
        (
            "50388",
            "ExtGo",
            "an extension base's access-excluded Error",
        ),
    ] {
        let r = row(rows, object, name);
        if next_dead(r) || !has_may_raise(r) {
            failures.push(format!(
                "{object}::{name} ({what}): dead={} may_raise={}",
                next_dead(r),
                has_may_raise(r)
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
