//! ENG-193: the resolver does not prefer the workspace's own object by name. The workspace
//! declares `W.X` (namespace W) and its dependency `A.X`; a workspace file `namespace C; using A;`
//! calls `X.F()`. alc 18.0.41 binds the dependency's `A.X` (the own `W.X` is invisible from
//! namespace C; Fable's probe q1 on the ENG-158 design). Without namespaces in the model the two
//! are ambiguous, so the call must not become a `Source` edge to the own `W.X.F` (the own-app
//! shortcut made exactly that edge). Control: a call to a name only the workspace declares (`Y`)
//! does make its edge.

use std::path::Path;
use std::process::Command;

const DEP_GUID: &str = "19319319-0000-4000-8000-0000000000a1";
const WS_GUID: &str = "19319319-0000-4000-8000-000000000001";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"15.0","Namespaces":[{"Name":"A","Codeunits":[{"Id":51001,"Name":"X","Methods":[{"Name":"F","Parameters":[]}]}]}]}"#;

const DEP_SOURCE: &str =
    "namespace A;\n\ncodeunit 51001 X\n{\n    procedure F()\n    begin\n    end;\n}\n";

const WS_OWN: &str =
    "namespace W;\n\ncodeunit 50030 X\n{\n    procedure F()\n    begin\n    end;\n}\n";

/// No namespace: visible from every file (tier 3).
const WS_GLOBAL: &str = "codeunit 50031 Y\n{\n    procedure F()\n    begin\n    end;\n}\n";

const WS_USE: &str = "namespace C;\n\nusing A;\n\ncodeunit 50010 U\n{\n    procedure G()\n    var\n        Cu: Codeunit X;\n    begin\n        Cu.F();\n    end;\n\n    procedure H()\n    var\n        Cy: Codeunit Y;\n    begin\n        Cy.F();\n    end;\n}\n";

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{WS_GUID}","name":"Ws193","publisher":"t","version":"1.0.0.0","runtime":"15.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"Dep193","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Own.al"), WS_OWN).unwrap();
    std::fs::write(ws.join("src/Global.al"), WS_GLOBAL).unwrap();
    std::fs::write(ws.join("src/Use.al"), WS_USE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep193_1.0.0.0.app"),
        DEP_GUID,
        "Dep193",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/A.al", DEP_SOURCE)],
        "",
    );
    ws
}

/// The `alsem graph` records of `ws`.
fn graph(ws: &Path) -> Vec<serde_json::Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["graph", &ws.to_string_lossy()])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "alsem graph: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Discrimination (2026-11-24): the own-app shortcut restored in `ResolveIndex::resolve_object_ref`
/// and `ProgramGraph::resolve_object` makes the `G -> W.X.F` edge (red); removed, green.
#[test]
fn the_workspace_object_does_not_shadow_a_dependency_object_of_its_name() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let recs = graph(&ws);
    let method = |object: &str, member: &str, app: &str| -> String {
        recs.iter()
            .find(|r| {
                r["t"] == "node"
                    && r["v"]["kind"] == "Method"
                    && r["v"]["objectName"] == object
                    && r["v"]["memberName"] == member
                    && r["v"]["appId"] == app
            })
            .unwrap_or_else(|| panic!("precondition: {object}::{member} of {app} is a graph node"))
            ["v"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let edge = |from: &str, to: &str| {
        recs.iter()
            .any(|r| r["t"] == "edge" && r["v"]["caller"] == from && r["v"]["callee"] == to)
    };
    let g = method("U", "G", WS_GUID);
    let h = method("U", "H", WS_GUID);
    let own_x_f = method("X", "F", WS_GUID);
    let y_f = method("Y", "F", WS_GUID);
    assert!(
        edge(&h, &y_f),
        "control: a name only the workspace declares binds"
    );
    assert!(
        !edge(&g, &own_x_f),
        "X is declared by the workspace (W) and the dependency (A): no Source edge to the own W.X"
    );
    // Nor a guessed edge to the dependency's: the call stays open, on the name hub.
    let dep_x_f = recs.iter().find(|r| {
        r["t"] == "node"
            && r["v"]["objectName"] == "X"
            && r["v"]["memberName"] == "F"
            && r["v"]["appId"] == DEP_GUID
    });
    if let Some(d) = dep_x_f {
        assert!(!edge(&g, d["v"]["id"].as_str().unwrap()), "no edge to A.X");
    }
    assert!(edge(&g, "h:name:f/0"), "the call is open, on the name hub");
}
