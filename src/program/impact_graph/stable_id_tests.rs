//! ENG-90: no two nodes of a sited graph share a StableRoutineId
//! ([`SitedGraph::shared_stable_ids`]) on real code: every corpus and fixture
//! workspace, plus a probe of the ASCII shapes closest to a collision. The
//! CDO harness checks the same on CDO (`cdo_sited_graph_stable_ids_are_unique`).
//! Names differing only in `İ` or a word-final `Σ` do share an id (see
//! `shared_stable_ids`); none of these workspaces holds one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::program::inputs::WorkspaceInputs;
use crate::program::resolved::{ProgramScope, ResolvedProgram};

fn sited(ws: &Path) -> SitedGraph {
    let inputs = Arc::new(WorkspaceInputs::read(ws, &Default::default()));
    let program = ResolvedProgram::build(&inputs, ProgramScope::CrossApp)
        .unwrap_or_else(|e| panic!("{}: {e}", ws.display()));
    sited_from_program(&program, &|| false).expect("not cancelled")
}

/// The shapes closest to a collision, each giving one id per node:
/// - one XMLport element name at two nesting levels, once quoted: the graph
///   keeps ONE node for both triggers;
/// - a subscriber whose `#if` arms subscribe to different events: split into
///   two nodes, each with its own `~` suffix (`split_decls`).
const PROBE: &str = r#"table 50900 "P90 Table"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { }
    }
}

codeunit 50900 "P90 Root"
{
    procedure Root()
    var
        T: Record "P90 Table";
    begin
        T.ModifyAll(Amount, 0);
    end;
}

codeunit 50901 "P90 Subs"
{
#if A
    [EventSubscriber(ObjectType::Table, Database::"P90 Table", OnAfterInsertEvent, '', false, false)]
    local procedure S(var Rec: Record "P90 Table"; RunTrigger: Boolean)
    var
        C: Codeunit "P90 Root";
    begin
        C.Root();
    end;
#else
    [EventSubscriber(ObjectType::Table, Database::"P90 Table", OnAfterDeleteEvent, '', false, false)]
    local procedure S(var Rec: Record "P90 Table"; RunTrigger: Boolean)
    var
        C: Codeunit "P90 Root";
    begin
        C.Root();
    end;
#endif
}

xmlport 50900 "P90 Port"
{
    schema
    {
        textelement(Top)
        {
            textelement(Name)
            {
                trigger OnBeforePassVariable()
                var
                    C: Codeunit "P90 Root";
                begin
                    C.Root();
                end;
            }
            textelement(Inner)
            {
                textelement("Name")
                {
                    trigger OnBeforePassVariable()
                    var
                        C: Codeunit "P90 Root";
                    begin
                        C.Root();
                    end;
                }
            }
        }
    }
}
"#;

/// Mutation (2026-11-18): `stable_routine_id` returning the unsuffixed id for
/// a split declaration -> `shared_stable_ids` lists the probe's two `S` nodes
/// on one id (red); restored, green.
#[test]
fn probe_nodes_never_share_a_stable_id() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join(".git")).unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"90909090-0000-0000-0000-000000000090","name":"P90","publisher":"RC","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Probe.al"), PROBE).unwrap();
    let g = sited(&ws);
    // The precondition: the split really happened, so the suffix path is
    // what keeps the two `S` nodes apart.
    let split: Vec<String> = (0..g.nodes.len() as u32)
        .filter_map(|n| g.stable_id(n))
        .filter(|id| id.contains(":Codeunit:50901#"))
        .collect();
    assert_eq!(
        split.len(),
        2,
        "the #if subscriber splits in two: {split:?}"
    );
    // And the XMLport's two triggers are one node.
    let port = (0..g.nodes.len() as u32)
        .filter_map(|n| g.stable_id(n))
        .filter(|id| id.contains(":XMLport:50900#"))
        .count();
    assert_eq!(port, 1, "the nested `Name` triggers are one node");
    assert_eq!(g.shared_stable_ids(), Vec::new());
}

/// Every workspace under `tests/r0-corpus` and `tests/fixtures` (a folder
/// with an `app.json`).
#[test]
fn corpus_nodes_never_share_a_stable_id() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut workspaces: Vec<PathBuf> = ["tests/r0-corpus", "tests/fixtures"]
        .iter()
        .flat_map(|d| std::fs::read_dir(root.join(d)).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    workspaces.sort();
    assert!(workspaces.len() > 200, "{} workspaces", workspaces.len());
    let shared: Vec<_> = workspaces
        .into_iter()
        .map(|ws| {
            let s = sited(&ws).shared_stable_ids();
            (ws, s)
        })
        .filter(|(_, s)| !s.is_empty())
        .collect();
    assert!(shared.is_empty(), "nodes sharing a stable id: {shared:#?}");
}
