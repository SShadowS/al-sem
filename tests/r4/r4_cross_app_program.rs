//! Engine-switch S7: the cross-app world built from the program engine.
//!
//! S7.1 — owning-app body resolution: a dependency's call sites are resolved from
//! the dependency's own view (its closure, its own-app shadowing), never the
//! workspace's.

use std::path::Path;

use crate::symbol_app::write_source_app;

use al_sem::program::node::ObjKey;
use al_sem::program::resolve::edge::{Evidence, RouteTarget};
use al_sem::program::resolve::full::build_context;

const WS_GUID: &str = "aaaa1111-0000-0000-0000-0000000000a7";
const DEP_GUID: &str = "bbbb2222-0000-0000-0000-0000000000a7";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Workspace and dependency both declare a codeunit named "Shared Name" with a
/// procedure `Foo`. The dependency's `Run` calls its own internal `Helper` and
/// `Shared Name`.Foo through a variable.
fn shared_name_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &dir.join("src/Shared.al"),
        "codeunit 50200 \"Shared Name\"\n{\n    procedure Foo()\n    begin\n    end;\n}\n",
    );
    write(
        &dir.join("src/Main.al"),
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        W: Codeunit \"Dep Worker\";\n    begin\n        W.Run();\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50100,"Name":"Dep Worker","Methods":[{{"Name":"Run","Parameters":[]}}]}},{{"Id":50101,"Name":"Shared Name","Methods":[{{"Name":"Foo","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            (
                "src/Worker.al",
                "codeunit 50100 \"Dep Worker\"\n{\n    procedure Run()\n    var\n        S: Codeunit \"Shared Name\";\n    begin\n        Helper();\n        S.Foo();\n    end;\n\n    internal procedure Helper()\n    begin\n    end;\n}\n",
            ),
            (
                "src/Shared.al",
                "codeunit 50101 \"Shared Name\"\n{\n    procedure Foo()\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
}

/// The target object number of every resolved route from the dependency's `run`.
fn targets_of_dep_run(dir: &Path) -> Vec<(String, i64)> {
    let ctx = build_context(dir).expect("context");
    let resolution = ctx.resolve_dependency_bodies();
    let graph = ctx.graph();
    let mut out = Vec::new();
    for ce in &resolution.edges {
        let from = &ce.edge.from;
        if graph.apps.resolve(from.object.app).guid != DEP_GUID || from.name_lc != "run" {
            continue;
        }
        for r in &ce.edge.routes {
            assert!(
                !matches!(r.evidence, Evidence::Unknown(_)),
                "unresolved route from the dependency: {:?}",
                r.evidence
            );
            if let RouteTarget::Routine(id) = &r.target {
                let app = graph.apps.resolve(id.object.app).guid.clone();
                let ObjKey::Id(n) = id.object.key else {
                    panic!("numberless target {id:?}")
                };
                out.push((app, n));
            }
        }
    }
    out.sort();
    out
}

/// Both calls resolve INSIDE the dependency: `Helper` to its own codeunit, and
/// `"Shared Name".Foo` to the dependency's own "Shared Name", not the workspace's
/// (the workspace is not in the dependency's closure).
///
/// Discrimination (2026-10-06): passing the primary app as the caller app in
/// `resolve_dependency_bodies` makes every dependency caller id carry the
/// workspace's app (and miss the object map), so no edge from the dependency's
/// `Run` is found and the test fails (`left: []`); restored, it passes.
#[test]
fn a_dependency_body_resolves_from_its_own_app() {
    let dir = tempfile::tempdir().unwrap();
    shared_name_workspace(dir.path());
    assert_eq!(
        targets_of_dep_run(dir.path()),
        vec![(DEP_GUID.to_string(), 50100), (DEP_GUID.to_string(), 50101)]
    );
}

const TEST_GUID: &str = "cccc3333-0000-0000-0000-0000000000a7";

/// A test app that depends on the workspace (as a test app does) calls the
/// workspace's `internal` procedure. `friend` decides whether the workspace's
/// app.json lists the test app in `internalsVisibleTo` (with its GUID upper-cased:
/// GUIDs compare case-insensitively; and a stale name, so only the GUID can match).
fn friend_workspace(dir: &Path, friend: bool) {
    let friends = if friend {
        format!(
            r#","internalsVisibleTo":[{{"id":"{}","name":"XTest Old Name","publisher":"probe"}}]"#,
            TEST_GUID.to_uppercase()
        )
    } else {
        String::new()
    };
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}]{friends}}}"#
        ),
    );
    write(
        &dir.join("src/Secret.al"),
        "codeunit 50202 \"Ws Secret\"
{
    internal procedure Hidden()
    begin
    end;
}
",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50300,"Name":"Test Caller","Methods":[{{"Name":"Run","Parameters":[]}}]}}],"AppId":"{TEST_GUID}","Name":"XTest","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XTest_1.0.0.0.app"),
        TEST_GUID,
        "XTest",
        "1.0.0.0",
        &symbols,
        &[(
            "src/Caller.al",
            "codeunit 50300 \"Test Caller\"
{
    procedure Run()
    var
        S: Codeunit \"Ws Secret\";
    begin
        S.Hidden();
    end;
}
",
        )],
        &format!(
            r#"<Dependencies><Dependency Id="{WS_GUID}" Name="XWs" Publisher="probe" MinVersion="1.0.0.0" /></Dependencies>"#
        ),
    );
}

/// The test app's call to the workspace's `Hidden`: the evidence of its one route.
fn hidden_call_evidence(dir: &Path) -> String {
    let ctx = build_context(dir).expect("context");
    let resolution = ctx.resolve_dependency_bodies();
    let graph = ctx.graph();
    let edges: Vec<_> = resolution
        .edges
        .iter()
        .filter(|ce| graph.apps.resolve(ce.edge.from.object.app).guid == TEST_GUID)
        .collect();
    assert_eq!(edges.len(), 1, "one call site in the test app");
    assert_eq!(edges[0].edge.routes.len(), 1);
    let r = &edges[0].edge.routes[0];
    match &r.target {
        RouteTarget::Routine(id) => {
            format!("{}:{}", graph.apps.resolve(id.object.app).guid, id.name_lc)
        }
        _ => format!("{:?}", r.evidence),
    }
}

/// The workspace's own app.json `internalsVisibleTo` is read: its friend resolves
/// the call, a non-friend is refused. Before S7.1 the workspace unit's friend list
/// was always empty, so DO's test app had 517 `InternalNotVisible` call sites.
///
/// Discrimination (2026-10-06): restoring `internals_visible_to: Vec::new()` for
/// the workspace unit in `SnapshotBuilder` makes the friend case come back
/// `Unknown(InternalNotVisible)`; so does comparing the friend GUID with `==` in
/// `wire_friend_authorizations`. Restored, it passes.
#[test]
fn a_friend_dependency_sees_the_workspaces_internal_members() {
    let friend = tempfile::tempdir().unwrap();
    friend_workspace(friend.path(), true);
    assert_eq!(
        hidden_call_evidence(friend.path()),
        format!("{WS_GUID}:hidden")
    );

    let stranger = tempfile::tempdir().unwrap();
    friend_workspace(stranger.path(), false);
    assert_eq!(
        hidden_call_evidence(stranger.path()),
        "Unknown(InternalNotVisible)"
    );
}
