//! Internal routine ids do not depend on the workspace's file list (context-server
//! design r3 §6.7, T-ids).
//!
//! The id is `"{app scope}/{64 lowercase hex}"`, where the app scope is the first
//! 16 lowercase hex of sha256 over the routine's own app guid (lowercase). It used
//! to be the workspace `modelInstanceId`, a hash over the sorted `.al` file list,
//! so adding, removing or renaming ANY file moved every id of every app in the
//! model.
//!
//! Both tests build through the production entry (`AnalysisTarget::build`, the
//! model `analyze` and every single-model command use) and state their
//! preconditions by hand: the file-list change is shown to move
//! `modelInstanceId`, so a stable id is evidence about the id, not about a no-op
//! edit.

use std::path::Path;

use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::program::inputs::WorkspaceInputs;
use sha2::{Digest, Sha256};

const WS_GUID: &str = "AAAA0717-0000-0000-0000-000000000717";
const DEP_GUID: &str = "bbbb0717-0000-0000-0000-000000000717";

/// The expected app scope, computed here and not by the engine: the first 16
/// lowercase hex of sha256 over the lowercase guid.
fn app_scope(guid: &str) -> String {
    let digest = Sha256::digest(guid.to_lowercase().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn app_json(guid: &str, dependencies: &str) -> String {
    format!(
        r#"{{"id":"{guid}","name":"Ids Probe","publisher":"probe","version":"1.0.0.0","runtime":"13.0","dependencies":[{dependencies}]}}"#
    )
}

const ALPHA_AL: &str = "codeunit 50100 Alpha\n{\n    procedure DoWork()\n    var\n        B: Codeunit Beta;\n    begin\n        B.Help();\n    end;\n}\n";
const BETA_AL: &str = "codeunit 50101 Beta\n{\n    procedure Help()\n    begin\n    end;\n}\n";
const EXTRA_AL: &str = "codeunit 50102 Extra\n{\n    procedure Spare()\n    begin\n    end;\n}\n";
const GAMMA_AL: &str = "codeunit 50103 Gamma\n{\n    procedure Other()\n    begin\n    end;\n}\n";

/// `(object name, routine name) -> internal id` for the routines named in
/// `wanted`, plus the workspace's `modelInstanceId`.
fn ids_of(ws: &Path, wanted: &[(&str, &str)]) -> (Vec<String>, Option<String>) {
    let inputs = WorkspaceInputs::read(ws, &Default::default());
    let instance = inputs.model_instance_id.clone();
    let target = AnalysisTarget::build(&inputs, false, "ids").expect("cross-app model");
    let m = target.resolved();
    let ids = wanted
        .iter()
        .map(|(object, routine)| {
            let hits: Vec<&str> = m
                .workspace
                .routines
                .iter()
                .filter(|r| {
                    r.name == *routine
                        && m.workspace
                            .objects
                            .iter()
                            .any(|o| o.id == r.object_id && o.name == *object)
                })
                .map(|r| r.id.as_str())
                .collect();
            assert_eq!(
                hits.len(),
                1,
                "precondition: one {object}.{routine}: {hits:?}"
            );
            hits[0].to_string()
        })
        .collect();
    (ids, instance)
}

/// Adding, removing and renaming a `.al` file the routines do not live in leaves
/// every routine id unchanged — while `modelInstanceId` (the old prefix) moves
/// each time.
#[test]
fn routine_ids_survive_unrelated_file_add_remove_rename() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    write(&ws.join("app.json"), &app_json(WS_GUID, ""));
    write(&ws.join("src/Alpha.al"), ALPHA_AL);
    write(&ws.join("src/Beta.al"), BETA_AL);
    write(&ws.join("src/Extra.al"), EXTRA_AL);
    let wanted = [("Alpha", "DoWork"), ("Beta", "Help")];

    let (before, instance0) = ids_of(ws, &wanted);
    let scope = app_scope(WS_GUID);
    for id in &before {
        let (prefix, hex) = id.split_once('/').expect("two-part id");
        assert_eq!(prefix, scope, "{id}: the prefix is the app scope");
        assert!(
            hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{id}: 64 lowercase hex after the one `/`"
        );
    }

    let mut instances = vec![instance0];
    let mut step = |what: &str, edit: &dyn Fn()| {
        edit();
        let (after, instance) = ids_of(ws, &wanted);
        assert!(
            !instances.contains(&instance),
            "precondition ({what}): the file list changed, so modelInstanceId must move \
             ({instance:?} seen before)"
        );
        instances.push(instance);
        assert_eq!(after, before, "{what} moved a routine id");
    };
    step("adding src/Gamma.al", &|| {
        write(&ws.join("src/Gamma.al"), GAMMA_AL)
    });
    step("removing src/Extra.al", &|| {
        std::fs::remove_file(ws.join("src/Extra.al")).expect("rm")
    });
    step("renaming src/Beta.al", &|| {
        std::fs::create_dir_all(ws.join("src/sub")).expect("mkdir");
        std::fs::rename(ws.join("src/Beta.al"), ws.join("src/sub/BetaRenamed.al")).expect("rename")
    });
}

/// Two apps in one cross-app model declare the SAME object (`codeunit 50100
/// Helper`) with the SAME routine (`procedure Run()`). Their ids differ, and
/// each carries its own app's scope as prefix.
#[test]
fn routine_ids_are_unique_and_app_scoped_across_two_apps() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let dep_ref =
        format!(r#"{{"id":"{DEP_GUID}","name":"XIds","publisher":"probe","version":"1.0.0.0"}}"#);
    write(&ws.join("app.json"), &app_json(WS_GUID, &dep_ref));
    // The workspace's Helper, and a subscriber to the dependency's event.
    write(
        &ws.join("src/Helper.al"),
        "codeunit 50100 Helper\n{\n    procedure Run()\n    begin\n    end;\n}\n",
    );
    write(
        &ws.join("src/Sub.al"),
        "codeunit 50110 Sub\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Entry\", 'OnStart', '', false, false)]\n    local procedure OnStartHandler()\n    var\n        H: Codeunit Helper;\n    begin\n        H.Run();\n    end;\n}\n",
    );
    // The dependency's own Helper (same type, number, name and routine) raises the
    // event the workspace subscribes to, so the cross-app model demands it as the
    // raiser (`program::resolve::demand`'s reverse rule).
    let dep_helper = "codeunit 50100 Helper\n{\n    procedure Run()\n    var\n        E: Codeunit \"Dep Entry\";\n    begin\n        E.OnStart();\n    end;\n}\n";
    let dep_entry = "codeunit 10 \"Dep Entry\"\n{\n    [IntegrationEvent(false, false)]\n    procedure OnStart()\n    begin\n    end;\n}\n";
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":10,"Name":"Dep Entry","Methods":[{{"Name":"OnStart","Parameters":[],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"False"}},{{"Value":"False"}}]}}]}}]}},{{"Id":50100,"Name":"Helper","Methods":[{{"Name":"Run","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XIds","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages").join("probe_XIds_1.0.0.0.app"),
        DEP_GUID,
        "XIds",
        "1.0.0.0",
        &symbols,
        &[("src/Helper.al", dep_helper), ("src/Entry.al", dep_entry)],
        "",
    );

    let inputs = WorkspaceInputs::read(ws, &Default::default());
    let target = AnalysisTarget::build(&inputs, false, "ids").expect("cross-app model");
    let m = target.resolved();
    let app_of = |r: &al_sem::program::model::workspace::ModelRoutine| {
        al_sem::engine::ids::object_id_app_guid(&r.object_id)
            .expect("object id carries its app")
            .to_lowercase()
    };
    let runs: Vec<(String, String)> = m
        .workspace
        .routines
        .iter()
        .filter(|r| {
            r.name == "Run"
                && m.workspace
                    .objects
                    .iter()
                    .any(|o| o.id == r.object_id && o.name == "Helper")
        })
        .map(|r| (app_of(r), r.id.clone()))
        .collect();
    let mut apps: Vec<&str> = runs.iter().map(|(a, _)| a.as_str()).collect();
    apps.sort();
    let mut want_apps = vec![WS_GUID.to_lowercase(), DEP_GUID.to_lowercase()];
    want_apps.sort();
    assert_eq!(
        apps, want_apps,
        "precondition: one Helper.Run per app in the model: {runs:?}"
    );

    assert_ne!(runs[0].1, runs[1].1, "the two apps' Helper.Run share an id");
    for (app, id) in &runs {
        let (prefix, _) = id.split_once('/').expect("two-part id");
        assert_eq!(
            prefix,
            app_scope(app),
            "{id}: the prefix is its own app's scope"
        );
    }
    assert_ne!(
        app_scope(WS_GUID),
        app_scope(DEP_GUID),
        "precondition: the two apps' scopes differ"
    );

    // Every routine id in the model, both apps together, is unique (no row of
    // this fixture is duplicated by `#if` arms).
    let mut all: Vec<&str> = m.workspace.routines.iter().map(|r| r.id.as_str()).collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "routine ids collide across the two apps");
}
