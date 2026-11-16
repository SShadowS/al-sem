//! ENG-99: an object's global record variables are ONE row per object declaration, shared by
//! the routines that do not shadow them, with the object-level id `{object_id}/grv/{name}`;
//! a per-routine change to one copies that row for that routine only.

use std::sync::Arc;

const SOURCE: &str = r#"table 50990 "SG T" { fields { field(1; K; Integer) { } } }
codeunit 50991 "SG Cu"
{
    var
        G: Record "SG T";

    procedure Guarded()
    begin
        if not G.IsTemporary() then
            Error('temporary only');
        G.Insert();
    end;

    procedure Plain()
    begin
        G.Insert();
    end;

    procedure AlsoPlain()
    begin
        G.Modify();
    end;

    procedure Shadows()
    var
        G: Record "SG T";
    begin
        G.Insert();
    end;
}
"#;

fn model() -> al_sem::program::model::workspace::Model {
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"99999999-0000-4000-8000-000000000099","name":"SG","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &crate::fingerprint_needs());
    assemble_and_resolve_workspace_program(
        &inputs,
        false,
        al_sem::program::model::workspace::AssemblyNeeds::BASE,
    )
    .expect("model")
}

/// Sharing: after resolution, `Plain` and `AlsoPlain` hold the very same `G` row (one
/// allocation), with the object-level id, its table resolved. Isolation: `Guarded`'s entry
/// guard (`if not G.IsTemporary() then Error`) makes ITS `G` temporary, on a copy; the shared
/// row stays as declared, so `Plain` does not see the guard. `Shadows` has its own `G`.
#[test]
fn object_globals_are_shared_and_a_per_routine_write_stays_in_its_routine() {
    let m = model();
    let g_of = |routine: &str| {
        let r = m
            .workspace
            .routines
            .iter()
            .find(|r| r.name == routine)
            .unwrap_or_else(|| panic!("no routine {routine}"));
        Arc::clone(
            r.record_variables
                .iter()
                .find(|v| v.name == "G")
                .unwrap_or_else(|| panic!("{routine} has no G")),
        )
    };
    let (guarded, plain, also, shadows) = (
        g_of("Guarded"),
        g_of("Plain"),
        g_of("AlsoPlain"),
        g_of("Shadows"),
    );
    // Isolation first, for every unguarded sibling, so a leak is named as one whatever the
    // routines' order.
    let known_true = |v: &al_sem::program::model::workspace::ModelRecordVariable| {
        v.temp_state.kind == "known" && v.temp_state.value == Some(true)
    };
    assert!(
        known_true(&guarded),
        "the entry guard makes Guarded's G temporary"
    );
    for (name, v) in [("Plain", &plain), ("AlsoPlain", &also)] {
        assert!(
            !known_true(v),
            "the guard must not leak into a sibling routine ({name}): {:?}",
            v.temp_state
        );
    }
    assert!(
        Arc::ptr_eq(&plain, &also),
        "the declaration's G is one shared row after resolution"
    );
    assert!(
        plain.id.ends_with("/grv/g") && plain.scope.as_deref() == Some("global"),
        "object-level id: {}",
        plain.id
    );
    assert!(plain.table_id.is_some(), "the shared row is resolved");
    assert!(
        !Arc::ptr_eq(&guarded, &plain),
        "the guarded routine holds its own copy"
    );
    assert_eq!(guarded.id, plain.id, "the copy keeps the object-level id");
    assert!(
        shadows.scope.as_deref() != Some("global") && !shadows.id.contains("/grv/"),
        "a local G shadows the global: {}",
        shadows.id
    );
}
