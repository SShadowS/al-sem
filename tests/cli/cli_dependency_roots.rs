//! G31: `prove` and `digest` start inside a dependency when the routine is
//! named by its StableRoutineId (a display name still selects only the
//! workspace's). Rule B (spec 2026-10-23 §14.3.5) ends a workspace root's
//! witness at the first dependency routine and groups the effects beyond it;
//! a dependency root starts past that boundary, so it is read with rule B off:
//! its witness paths follow the dependency code to each effect. Before, it got
//! no effect at all (the reportable roots leave dependency routines out).
//!
//! Fixture: `Ws Work.CallChain` calls `Dep Lib.Outer`, whose `Inner` inserts a
//! Customer (`dep_dispatch_fixture`).

use std::path::Path;
use std::process::Command;

use crate::dep_dispatch_fixture as dd;

fn prove(ws: &Path, routine: &str) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove"])
        .arg(ws)
        .args([routine, "writes-table:Customer", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))["payload"]
        ["result"]
        .clone()
}

/// The hops (`from>to`) of each via path of each evidence item.
fn hops(result: &serde_json::Value) -> Vec<Vec<String>> {
    result["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|e| e["viaPaths"].as_array().into_iter().flatten())
        .map(|p| {
            p.as_array()
                .unwrap()
                .iter()
                .map(|h| format!("{}>{}", s(&h["fromDisplay"]), s(&h["toDisplay"])))
                .collect()
        })
        .collect()
}

fn s(v: &serde_json::Value) -> &str {
    v.as_str().unwrap_or("")
}

#[test]
fn a_dependency_root_follows_the_dependency_code() {
    let t = tempfile::tempdir().unwrap();
    dd::write_own(t.path(), "");
    // The dependency routine's id, off the cross-app model.
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(t.path(), &Default::default());
    let target = al_sem::engine::gate::run::AnalysisTarget::build(&inputs, false, "t")
        .expect("cross-app target");
    let m = target.resolved();
    let outer = m
        .workspace
        .routines
        .iter()
        .find(|r| {
            r.name == "Outer" && m.routine_role(r) == al_sem::program::model::role::Role::Dependency
        })
        .expect("precondition: Dep Lib.Outer is a dependency routine")
        .stable_routine_id
        .clone();

    // Precondition: from the workspace, the write is a group at the boundary.
    let ws = prove(t.path(), "CallChain");
    assert_eq!(ws["answer"], "yes", "precondition: {ws}");
    assert!(
        ws["evidence"][0]["detail"]["boundary"].is_string(),
        "precondition: a boundary group: {ws}"
    );

    // From the dependency routine: its own path to the insert.
    let dep = prove(t.path(), &outer);
    assert_eq!(dep["answer"], "yes", "{dep}");
    assert!(
        dep["evidence"][0]["detail"]["boundary"].is_null(),
        "no boundary group: {dep}"
    );
    assert!(
        hops(&dep).iter().any(|p| p == &["Outer>Inner".to_string()]),
        "the path into Inner: {dep}"
    );
}

/// A run of a dependency codeunit that declares no `OnRun` runs no AL code:
/// no blocker, whether the dependency's code is read from the workspace or
/// from inside the dependency.
#[test]
fn a_run_of_a_dependency_codeunit_without_onrun_blocks_nothing() {
    let lib = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    var
        Reader: Codeunit "Dep Reader";
    begin
        Reader.Run();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}

codeunit 13 "Dep Reader"
{
    procedure GetName(): Text
    begin
        exit('x');
    end;
}
"#;
    let t = tempfile::tempdir().unwrap();
    dd::write_own_lib(t.path(), "", lib);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("prove")
        .arg(t.path())
        .args(["CallChain", "may-commit", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let r: serde_json::Value =
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))["payload"]
            ["result"]
            .clone();
    assert_eq!(r["answer"], "no", "{r}");
}

/// A dependency root's L4 summary holds no rows, so its writes' `tempState`
/// is read off the cone fact's own class: a write to a temporary record is no
/// table write (it read `writes-table: yes` with no temp state at all).
#[test]
fn a_dependency_roots_temporary_write_is_no_table_write() {
    let lib = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    begin
        Inner();
    end;

    procedure Inner()
    var
        TempCust: Record Customer temporary;
    begin
        TempCust.Insert();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}
"#;
    let t = tempfile::tempdir().unwrap();
    dd::write_own_lib(t.path(), "", lib);
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(t.path(), &Default::default());
    let target = al_sem::engine::gate::run::AnalysisTarget::build(&inputs, false, "t")
        .expect("cross-app target");
    let m = target.resolved();
    let outer = m
        .workspace
        .routines
        .iter()
        .find(|r| {
            r.name == "Outer" && m.routine_role(r) == al_sem::program::model::role::Role::Dependency
        })
        .expect("precondition: Dep Lib.Outer is a dependency routine")
        .stable_routine_id
        .clone();
    let r = prove(t.path(), &outer);
    assert_eq!(r["answer"], "no", "{r}");
}
