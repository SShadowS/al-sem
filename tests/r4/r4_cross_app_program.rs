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
/// procedure `Foo`. The dependency's `Work` calls its own internal `Helper` and
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
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        W: Codeunit \"Dep Worker\";\n        E: Codeunit \"Dep Events\";\n    begin\n        W.Work();\n        E.OnFoo();\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50100,"Name":"Dep Worker","Methods":[{{"Name":"Work","Parameters":[]}}]}},{{"Id":50101,"Name":"Shared Name","Methods":[{{"Name":"Foo","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
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
                "codeunit 50100 \"Dep Worker\"\n{\n    procedure Work()\n    var\n        S: Codeunit \"Shared Name\";\n    begin\n        Helper();\n        S.Foo();\n    end;\n\n    internal procedure Helper()\n    begin\n    end;\n}\n",
            ),
            (
                "src/Shared.al",
                "codeunit 50101 \"Shared Name\"\n{\n    procedure Foo()\n    begin\n    end;\n}\n",
            ),
            (
                "src/Events.al",
                "codeunit 50102 \"Dep Events\"
{
    [IntegrationEvent(false, false)]
    procedure OnFoo()
    begin
    end;
}

codeunit 50103 \"Dep Listener\"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Events\", 'OnFoo', '', false, false)]
    local procedure HandleFoo()
    begin
    end;
}
",
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
        if graph.apps.resolve(from.object.app).guid != DEP_GUID || from.name_lc != "work" {
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
/// `Work` is found and the test fails (`left: []`); restored, it passes.
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
    procedure Work()
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

/// The cross-app fixtures: each has dependency code in the model.
fn cross_app_fixtures() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    [
        "r3a5-fixtures/ws",
        "r3a4-fixtures/ws",
        "r2-5b-fixtures/cross-app-resolution",
        "r0-corpus/ws-d13-internal-call",
        "r0-corpus/ws-d13-member-call",
        "r0-corpus/ws-d16-obsolete",
        "r0-corpus/ws-d17-drift",
    ]
    .iter()
    .map(|f| root.join(f))
    .collect()
}

/// The cross-app model's row ORDER is a contract: the symbol table is last-wins and
/// the extension-field merge first-wins, so a dependency's parsed source must come
/// after the symbol-only (ABI) rows it may shadow. Every routine row is, in order:
/// the workspace's (`ws:` units), then every symbol-only dependency's ABI rows
/// (no source unit), then every source-bearing dependency's parsed rows (`dep:`
/// units). Until engine-switch S9.6 this was pinned by comparing with the legacy
/// merged model (`cross_app_l3`), deleted with L3; the order is now stated directly.
///
/// Discrimination (2026-10-08): appending the ABI rows AFTER the parsed dependency
/// rows in `append_dependency_rows` fails this test on `r3a5-fixtures/ws` (ranks
/// `[0, 0, 2, 2, 1]`); restored, it passes.
#[test]
fn the_cross_app_model_rows_are_workspace_then_abi_then_parsed_dependencies() {
    use al_sem::program::model::workspace::{
        MODEL_INSTANCE_ID_DEFAULT as MI, assemble_and_resolve_cross_app_from_program,
    };
    for ws in cross_app_fixtures() {
        let ctx = build_context(&ws).expect("context");
        let (model, _) = assemble_and_resolve_cross_app_from_program(
            &ws,
            MI,
            false,
            &ctx,
            None,
            &Default::default(),
        )
        .expect("program model");
        // Rank of each routine's block: 0 workspace, 1 ABI, 2 parsed dependency.
        let ranks: Vec<u8> = model
            .workspace
            .routines
            .iter()
            .map(|r| {
                let unit = r.source_anchor.source_unit_id.as_str();
                if unit.starts_with("ws:") {
                    0
                } else if unit.is_empty() {
                    1
                } else {
                    assert!(unit.starts_with("dep:"), "{}: unit {unit:?}", ws.display());
                    2
                }
            })
            .collect();
        assert!(
            ranks.contains(&0) && ranks.iter().any(|&k| k > 0),
            "{}: workspace and dependency rows must both be present",
            ws.display()
        );
        if ws.ends_with("r3a5-fixtures/ws") {
            assert!(
                ranks.contains(&1) && ranks.contains(&2),
                "r3a5-fixtures/ws has both ABI and parsed dependency rows"
            );
        }
        assert!(
            ranks.windows(2).all(|w| w[0] <= w[1]),
            "{}: rows must be workspace, then ABI, then parsed dependencies: {ranks:?}",
            ws.display()
        );
    }
}

/// `"<object number>.<routine name>"` of a model routine id.
fn routine_label(m: &al_sem::program::model::workspace::Model, id: &str) -> String {
    let r = m
        .workspace
        .routines
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no model routine {id}"));
    format!("{}.{}", r.object_id.rsplit('/').next().unwrap(), r.name)
}

/// S7.3: in the cross-app model every body's calls come from the program engine —
/// the workspace's call into the dependency now lands on the dependency's model
/// routine, and the dependency's own calls resolve from its own view — and the
/// event graph binds a subscriber that lives in the dependency. (Since S8.2 the
/// workspace raises the dependency event: a subscriber of an event no demanded
/// routine raises is not in the model.)
///
/// Discrimination (2026-10-06): limiting `Converter::model_apps` to the primary app
/// turns the three calls into the dependency into to-less `ExternalTarget` edges
/// (and the dependency's own call sites get no program edge); limiting the event
/// graph's `in_model` to the primary app leaves `HandleFoo` unmapped. Each fails
/// the test; restored, it passes.
#[test]
fn the_cross_app_model_resolves_dependency_bodies_and_events() {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let dir = tempfile::tempdir().unwrap();
    shared_name_workspace(dir.path());
    let m = assemble_and_resolve_cross_app_program(dir.path(), MI, false)
        .expect("model")
        .resolved;
    let calls = m.calls.clone();
    let mut resolved: Vec<String> = calls
        .edges
        .iter()
        .filter_map(|e| {
            let to = e.to.as_ref()?;
            Some(format!(
                "{} -> {} {:?}",
                routine_label(&m, &e.from),
                routine_label(&m, to),
                e.resolution
            ))
        })
        .collect();
    resolved.sort();
    assert_eq!(
        resolved,
        vec![
            "50100.Work -> 50100.Helper Resolved",
            "50100.Work -> 50101.Foo Resolved",
            "50201.Go -> 50100.Work Resolved",
            "50201.Go -> 50102.OnFoo Resolved",
        ]
    );

    let events = m.events.clone();
    let handle = events
        .graph
        .edges
        .iter()
        .find(|e| routine_label(&m, &e.subscriber_routine_id) == "50103.HandleFoo")
        .expect("the dependency subscriber is in the event graph");
    assert_eq!(handle.resolution, "resolved");
    let event = events
        .graph
        .events
        .iter()
        .find(|ev| ev.id == handle.event_id)
        .unwrap();
    assert_eq!(
        routine_label(&m, event.publisher_routine_id.as_ref().unwrap()),
        "50102.OnFoo"
    );
}

/// S7.3: a call into a SYMBOL-ONLY dependency lands on that routine's model row
/// (the bodyless ABI row), joined through the program id `AbiRowIds` maps, not
/// left as a to-less dependency target. On `r3a5-fixtures/ws`, whose
/// `55555555-…` dependency ships no source.
///
/// Discrimination (2026-10-06): passing `None` for the ABI rows in
/// `assemble_and_resolve_cross_app_program` leaves those calls to-less and the
/// test fails; restored, it passes.
#[test]
fn a_call_into_a_symbol_only_dependency_lands_on_its_model_row() {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r3a5-fixtures/ws");
    let m = assemble_and_resolve_cross_app_program(&ws, MI, false)
        .expect("model")
        .resolved;
    let calls = m.calls.clone();
    let bodyless: Vec<&str> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.app_guid.starts_with("55555555") && !r.body_available)
        .map(|r| r.id.as_str())
        .collect();
    assert!(!bodyless.is_empty(), "fixture has symbol-only rows");
    let landed = calls
        .edges
        .iter()
        .filter(|e| e.to.as_deref().is_some_and(|t| bodyless.contains(&t)))
        .count();
    assert!(landed > 0, "no call landed on a symbol-only model row");
}

/// A workspace calling a dependency's `internal` procedure; `friend` decides whether
/// the dependency's manifest lists the workspace in `<InternalsVisibleTo>`.
fn internal_call_workspace(dir: &Path, friend: bool) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &dir.join("src/Main.al"),
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        D: Codeunit \"Dep Secret\";\n    begin\n        D.Hidden();\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50110,"Name":"Dep Secret","Methods":[{{"Name":"Hidden","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let friends = if friend {
        format!(
            r#"<InternalsVisibleTo><Module Id="{WS_GUID}" Name="XWs" Publisher="probe" /></InternalsVisibleTo>"#
        )
    } else {
        String::new()
    };
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[(
            "src/Secret.al",
            "codeunit 50110 \"Dep Secret\"\n{\n    internal procedure Hidden()\n    begin\n    end;\n}\n",
        )],
        &friends,
    );
}

fn d13_count(dir: &Path) -> usize {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d13-cross-app-internal-call".to_string()];
    project_r4_findings_cross_app(dir, "r0", &registered_detectors(), "x", &names).finding_count
}

/// d13 and calls into a dependency's `internal` procedure, end to end through the
/// cross-app base. A FRIEND (named in the dependency's `<InternalsVisibleTo>`) was
/// let in on purpose: no finding (engine-switch S8.5; 20 of 20 such cross-app
/// findings on CDO/DO were friend calls). A stranger's call cannot compile: the
/// program resolver refuses it (`InternalNotVisible`), so there is no edge to flag.
/// d13's remaining positive is the `[InternalProc]` shape (`ws-d13-internal-call`'s
/// golden).
///
/// Discrimination (2026-10-06): removing the friend skip in `detect_d13` reports
/// the friend case (`left: 1`); making `resolver::internal_visible_across` always
/// true (the legacy resolver's blindness to visibility) reports the stranger case.
/// Each fails the test; restored, it passes.
#[test]
fn d13_does_not_flag_a_call_the_dependency_allows() {
    let friend = tempfile::tempdir().unwrap();
    internal_call_workspace(friend.path(), true);
    assert_eq!(d13_count(friend.path()), 0);
    let stranger = tempfile::tempdir().unwrap();
    internal_call_workspace(stranger.path(), false);
    assert_eq!(d13_count(stranger.path()), 0);
}

/// An API page with no write-surface property: d64's shape B.
fn api_page(number: u32, name: &str, table: &str) -> String {
    format!(
        "page {number} \"{name}\"\n{{\n    PageType = API;\n    APIPublisher = 'probe';\n    APIGroup = 'probe';\n    APIVersion = 'v1.0';\n    EntityName = 'thing{number}';\n    EntitySetName = 'things{number}';\n    SourceTable = \"{table}\";\n\n    layout\n    {{\n        area(Content)\n        {{\n            field(Code; Rec.Code) {{ }}\n        }}\n    }}\n}}\n"
    )
}

fn table(number: u32, name: &str) -> String {
    format!(
        "table {number} \"{name}\"\n{{\n    fields\n    {{\n        field(1; Code; Code[20]) {{ }}\n    }}\n}}\n"
    )
}

/// S7.4: the cross-app model holds dependency OBJECTS, and an object-anchored
/// finding (d64 names its page) is scoped by the object's app: a dependency's API
/// page is not reported, the workspace's is. On DO, three Microsoft "System
/// Application Test Library" mock API pages were reported before this.
///
/// Discrimination (2026-10-06): dropping the object entries from
/// `run_detectors_cross_app`'s role map reports the dependency page too
/// (`left: 2`); restored, it passes.
#[test]
fn an_object_finding_in_a_dependency_is_out_of_scope() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(&ws.join("src/Table.al"), &table(50210, "Ws Thing"));
    write(
        &ws.join("src/Api.al"),
        &api_page(50211, "Ws Api", "Ws Thing"),
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let (dep_table, dep_page) = (
        table(50120, "Dep Thing"),
        api_page(50121, "Dep Api", "Dep Thing"),
    );
    write_source_app(
        &ws.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[("src/Table.al", &dep_table), ("src/Api.al", &dep_page)],
        "",
    );
    let names = vec!["d64-api-page-write-surface".to_string()];
    let p = project_r4_findings_cross_app(ws, "r0", &registered_detectors(), "x", &names);
    assert_eq!(p.finding_count, 1, "{:#?}", p.findings);
    assert!(
        p.findings[0]
            .primary_location
            .source_unit_id
            .starts_with("ws:"),
        "{:#?}",
        p.findings[0]
    );
}

/// A dependency's `Send` skips its table write when a subscriber sets IsHandled; the
/// workspace subscriber sets it, after an early `exit` when `early_exit`.
fn ishandled_workspace(dir: &Path, early_exit: bool) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    let guard = if early_exit {
        "        if Skip() then\n            exit;\n"
    } else {
        ""
    };
    write(
        &dir.join("src/Subs.al"),
        &format!(
            "codeunit 50230 \"Ws Subs\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Sender\", 'OnBeforeSend', '', false, false)]\n    local procedure HandleSend(var IsHandled: Boolean)\n    begin\n{guard}        IsHandled := true;\n    end;\n\n    local procedure Skip(): Boolean\n    begin\n        exit(false);\n    end;\n}}\n"
        ),
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            ("src/Log.al", &table(50130, "Dep Log")),
            (
                "src/Sender.al",
                "codeunit 50131 \"Dep Sender\"\n{\n    procedure Send()\n    var\n        IsHandled: Boolean;\n        Log: Record \"Dep Log\";\n    begin\n        IsHandled := false;\n        OnBeforeSend(IsHandled);\n        if IsHandled then\n            exit;\n        Log.Insert();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnBeforeSend(var IsHandled: Boolean)\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
}

fn d43_confidence(dir: &Path) -> Vec<String> {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d43-event-ishandled-skip".to_string()];
    project_r4_findings_cross_app(dir, "r0", &registered_detectors(), "x", &names)
        .findings
        .iter()
        .map(|f| f.confidence.level.clone())
        .collect()
}

/// S7.4 triage fix: d43 reaches a dependency caller that skips its write on
/// IsHandled. A subscriber that sets IsHandled unconditionally is "confirmed"; one
/// that can `exit` before the setter sets it on some paths only, so "likely" —
/// `classify_subscriber` called every top-level setter "always sets" (CDO's eDocs
/// Sending Profile subscriber, 26 findings on CDO and DO).
///
/// Discrimination (2026-10-06): removing the `exit_before` condition in
/// `classify_subscriber` makes the early-exit case "confirmed"; restored, it passes.
#[test]
fn d43_does_not_call_a_setter_after_an_early_exit_certain() {
    let plain = tempfile::tempdir().unwrap();
    ishandled_workspace(plain.path(), false);
    assert_eq!(d43_confidence(plain.path()), vec!["confirmed"]);
    let early = tempfile::tempdir().unwrap();
    ishandled_workspace(early.path(), true);
    assert_eq!(d43_confidence(early.path()), vec!["likely"]);
}

/// S7.4: the cross-app world is the workspace and the dependencies it requires. An
/// app that depends ON the workspace — a test app — is loaded by the snapshot (DO's
/// ancestor `.alpackages` holds one) but is not one of them; its subscribers made
/// two d45 false positives and hid four dead-event d12 findings on DO.
///
/// Discrimination (2026-10-06): making `ProgramContext::is_required_dependency`
/// accept every app but the workspace puts the test app's routine in the model and
/// fails the test; restored, it passes.
#[test]
fn an_app_that_depends_on_the_workspace_is_not_in_the_cross_app_model() {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let dir = tempfile::tempdir().unwrap();
    friend_workspace(dir.path(), true);
    let x = assemble_and_resolve_cross_app_program(dir.path(), MI, false).expect("model");
    let apps: std::collections::BTreeSet<&str> = x
        .resolved
        .workspace
        .routines
        .iter()
        .map(|r| r.app_guid.as_str())
        .collect();
    assert_eq!(apps.into_iter().collect::<Vec<_>>(), vec![WS_GUID]);
    assert!(x.dependency_apps.is_empty(), "{:?}", x.dependency_apps);
}

/// Several dependency subscribers and one workspace subscriber of a dependency
/// event all write one table.
fn shared_subscribers_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    let sub = |n: u32, name: &str| {
        format!(
            "codeunit {n} \"{name}\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Hub\", 'OnRegister', '', false, false)]\n    local procedure Handle{n}()\n    var\n        Log: Record \"Dep Log\";\n    begin\n        Log.Insert();\n    end;\n}}\n"
        )
    };
    write(&dir.join("src/Sub.al"), &sub(50240, "Ws Sub"));
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let dep_subs: Vec<(String, String)> = (0..20)
        .map(|i| {
            (
                format!("src/Sub{i}.al"),
                sub(50140 + i, &format!("Dep Sub {i}")),
            )
        })
        .collect();
    let mut sources: Vec<(&str, &str)> = dep_subs
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let log = table(50130, "Dep Log");
    sources.push(("src/Log.al", &log));
    sources.push((
        "src/Hub.al",
        "codeunit 50139 \"Dep Hub\"\n{\n    [IntegrationEvent(false, false)]\n    procedure OnRegister()\n    begin\n    end;\n}\n",
    ));
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &sources,
        "",
    );
}

/// S7.4 triage fix: d44 anchors a finding on a WORKSPACE subscriber when the event
/// has one. It took the first subscriber in id order, which in cross-app mode can
/// be a dependency's; the scope filter then dropped the finding (14 lost on CDO and
/// DO for OnRegisterManualSetup).
///
/// Discrimination (2026-10-06): making `d44::anchor_subscriber` return the first
/// subscriber drops the finding (`left: 0`); restored, it passes.
#[test]
fn d44_anchors_on_the_workspace_subscriber() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let dir = tempfile::tempdir().unwrap();
    shared_subscribers_workspace(dir.path());
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    let p = project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
    assert_eq!(p.finding_count, 1, "{:#?}", p.findings);
    assert_eq!(
        p.findings[0].primary_location.source_unit_id,
        "ws:src/Sub.al"
    );
}

/// S7.6 contract: with no dependency, cross-app mode IS single-app mode. On every
/// r0 corpus fixture without a `.alpackages` folder, every registered detector
/// reports the same findings through `project_r4_findings_cross_app` as through the
/// single-app `project_r4_findings` over the program-backed model. Before S7.6 the
/// cross-app context had no call-site index, root classifications or ordering
/// facts, so the detectors reading them (d40/d41/d42/d47/d49/d50/d51/d53/d55/d61)
/// were blind there.
///
/// Discrimination (2026-10-06): restoring the EMPTY call-site index in
/// `build_detector_context_cross_app` makes fixtures differ; restored, it passes.
#[test]
fn without_dependencies_cross_app_mode_reports_what_single_app_mode_does() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::{project_r4_findings, project_r4_findings_cross_app};
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
    let detectors = registered_detectors();
    let names: Vec<String> = detectors.iter().map(|d| d.name.clone()).collect();
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && !p.join(".alpackages").exists())
        .collect();
    dirs.sort();
    let (mut compared, mut differ) = (0, Vec::new());
    for ws in &dirs {
        let Some(single) = assemble_and_resolve_workspace_with_program_calls(ws) else {
            continue;
        };
        let ids = |p: al_sem::engine::l5::finding::R4FindingsProjection| {
            p.findings.into_iter().map(|f| f.id).collect::<Vec<_>>()
        };
        let a = ids(project_r4_findings(&single, &detectors, "x", &names));
        let b = ids(project_r4_findings_cross_app(
            ws, "r0", &detectors, "x", &names,
        ));
        compared += 1;
        if a != b {
            differ.push(ws.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    assert!(compared > 150, "compared only {compared} fixtures");
    assert!(differ.is_empty(), "cross-app != single-app on {differ:?}");
}

/// S7.6 triage fix, through the cross-app base: the dependency's own
/// `Temp Blob.CreateOutStream`, which calls `TempBlobImpl.CreateOutStream` on a
/// `Codeunit "Temp Blob Impl."`, gets no FILE fact (an in-memory stream). The
/// substring match took `Temp Blob Impl.` for `Temp Blob`, and CDO's d47 then
/// reported the same stream twice, once inside the System Application.
///
/// Discrimination (2026-10-06): restoring `contains("temp blob")` in
/// `capability::io::is_temp_blob_type` gives `CreateOutStream` a FILE fact and
/// fails the test; restored, it passes.
#[test]
fn temp_blob_impl_is_not_file_io() {
    use al_sem::engine::l4::capability_cone::project_r3a5_cross_app;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &ws.join("src/Main.al"),
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        B: Codeunit \"Temp Blob\";\n        S: OutStream;\n    begin\n        B.CreateOutStream(S);\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &ws.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            (
                "src/TempBlob.al",
                "codeunit 50150 \"Temp Blob\"\n{\n    procedure CreateOutStream(var OutStream: OutStream)\n    var\n        TempBlobImpl: Codeunit \"Temp Blob Impl.\";\n    begin\n        TempBlobImpl.CreateOutStream(OutStream);\n    end;\n}\n",
            ),
            (
                "src/TempBlobImpl.al",
                "codeunit 50151 \"Temp Blob Impl.\"\n{\n    procedure CreateOutStream(var OutStream: OutStream)\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
    let p = project_r3a5_cross_app(ws, "r0", "x");
    let dep_file_facts: Vec<String> = p
        .summaries
        .iter()
        .filter(|s| s.is_dep_routine)
        .flat_map(|s| s.capability_facts_direct.iter())
        .filter(|f| f.resource_kind == "file")
        .map(|f| f.op.clone())
        .collect();
    assert!(dep_file_facts.is_empty(), "{dep_file_facts:?}");
    // Not hollow: the workspace's own `Temp Blob` call is still classified.
    let ws_file = p
        .summaries
        .iter()
        .filter(|s| !s.is_dep_routine)
        .flat_map(|s| s.capability_facts_direct.iter())
        .any(|f| f.resource_kind == "file");
    assert!(ws_file, "the workspace's Temp Blob call has its FILE fact");
}

/// A routine raises an IsHandled event and writes inside `guard`; a subscriber
/// sets the flag.
fn d61_workspace(dir: &Path, guard: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}]}}"#
        ),
    );
    write(&dir.join("src/Log.al"), &table(50250, "Ws Log"));
    write(
        &dir.join("src/Mgr.al"),
        &format!(
            "codeunit 50251 \"Ws Mgr\"\n{{\n    procedure Go(var Log: Record \"Ws Log\")\n    var\n        IsHandled: Boolean;\n    begin\n        IsHandled := false;\n        OnBeforeGo(Log, IsHandled);\n        {guard}\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnBeforeGo(var Log: Record \"Ws Log\"; var IsHandled: Boolean)\n    begin\n    end;\n}}\n\ncodeunit 50252 \"Ws Handler\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Ws Mgr\", 'OnBeforeGo', '', false, false)]\n    local procedure H(var Log: Record \"Ws Log\"; var IsHandled: Boolean)\n    begin\n        IsHandled := true;\n    end;\n}}\n"
        ),
    );
}

fn d61_count(dir: &Path) -> usize {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d61-ishandled-bypasses-critical-write".to_string()];
    project_r4_findings_cross_app(dir, "r0", &registered_detectors(), "x", &names).finding_count
}

/// S7.6 triage fix: d61 flags a write the flag can SKIP, never one that runs
/// because the subscriber set it. `if IsHandled then Log.Modify()` is the
/// "a result was produced" shape (CDO/DO `OnSelectReportLayout`, 5 false
/// positives once the Base Application subscriber was in view); `if not IsHandled
/// then` and `if IsHandled then exit else` are the bypass. A compound guard
/// (`ws-event-ishandled-nested-guard`) stays flagged.
///
/// Discrimination (2026-10-06): making `write_skipped_when_flag_set` always true
/// flags the first case (`left: 1`); restored, it passes.
#[test]
fn d61_does_not_flag_a_write_that_runs_when_handled() {
    for (guard, expected) in [
        ("if IsHandled then Log.Modify();", 0),
        ("if not IsHandled then Log.Modify();", 1),
        ("if IsHandled then exit else Log.Modify();", 1),
    ] {
        let dir = tempfile::tempdir().unwrap();
        d61_workspace(dir.path(), guard);
        assert_eq!(d61_count(dir.path()), expected, "{guard}");
    }
}

/// S8.2 demand: which dependency routines the cross-app model holds.
fn demand_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &dir.join("src/Main.al"),
        "codeunit 50270 \"Ws Main\"\n{\n    procedure Go()\n    var\n        A: Codeunit \"Dep A\";\n    begin\n        A.Reached();\n    end;\n\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Pub\", 'OnThing', '', false, false)]\n    local procedure OnThing()\n    begin\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            (
                "src/A.al",
                "codeunit 50170 \"Dep A\"\n{\n    procedure Reached()\n    begin\n        Transitive();\n    end;\n\n    procedure Transitive()\n    begin\n    end;\n\n    procedure Unreached()\n    begin\n    end;\n}\n",
            ),
            (
                "src/Pub.al",
                "codeunit 50171 \"Dep Pub\"\n{\n    procedure Raise()\n    begin\n        Prepare();\n        OnThing();\n    end;\n\n    procedure Prepare()\n    begin\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnThing()\n    begin\n    end;\n}\n",
            ),
        ],
        "",
    );
}

/// S8.2: forward from the workspace (`Reached` and what it calls, `Transitive`),
/// reverse from a dependency event the workspace subscribes to (the publisher
/// `OnThing`, its raiser `Raise`, and what the raiser calls, `Prepare`). A
/// dependency routine nothing reaches (`Unreached`) is not in the model; its object
/// is.
///
/// Discrimination (2026-10-06): dropping the reverse rule in `cross_app_demand`
/// loses `OnThing`, `Raise` and `Prepare`; seeding no forward walk from the
/// workspace loses `Reached` and `Transitive`. Each fails the test; restored, it
/// passes.
#[test]
fn the_cross_app_model_holds_the_demanded_dependency_routines() {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let dir = tempfile::tempdir().unwrap();
    demand_workspace(dir.path());
    let x = assemble_and_resolve_cross_app_program(dir.path(), MI, false).expect("model");
    let ws = &x.resolved.workspace;
    let mut dep: Vec<&str> = ws
        .routines
        .iter()
        .filter(|r| r.app_guid == DEP_GUID)
        .map(|r| r.name.as_str())
        .collect();
    dep.sort_unstable();
    assert_eq!(
        dep,
        vec!["OnThing", "Prepare", "Raise", "Reached", "Transitive"]
    );
    assert!(
        ws.objects.iter().any(|o| o.name == "Dep A"),
        "objects stay whole"
    );
}

/// Two workspace subscribers of a workspace event each call dependency `A.Run`,
/// which calls `B.Write`, which writes `Dep Log`: the write is two dependency hops
/// away, behind a dependency-internal edge.
fn transitive_dep_write_workspace(dir: &Path) {
    transitive_dep_write_workspace_with(
        dir,
        "codeunit 50181 \"Dep A\"\n{\n    procedure Run2()\n    var\n        B: Codeunit \"Dep B\";\n    begin\n        B.Write();\n    end;\n}\n",
        "codeunit 50182 \"Dep B\"\n{\n    procedure Write()\n    var\n        Log: Record \"Dep Log\";\n    begin\n        Log.Insert();\n    end;\n}\n",
    );
}

/// [`transitive_dep_write_workspace`] with the dependency's `Dep A` / `Dep B`
/// codeunit sources given.
fn transitive_dep_write_workspace_with(dir: &Path, dep_a: &str, dep_b: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    let sub = |n: u32| {
        format!(
            "codeunit {n} \"Ws Sub {n}\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Ws Hub\", 'OnGo', '', false, false)]\n    local procedure Handle()\n    var\n        A: Codeunit \"Dep A\";\n    begin\n        A.Run2();\n    end;\n}}\n"
        )
    };
    write(&dir.join("src/Sub1.al"), &sub(50281));
    write(&dir.join("src/Sub2.al"), &sub(50282));
    write(
        &dir.join("src/Hub.al"),
        "codeunit 50280 \"Ws Hub\"\n{\n    procedure Go()\n    begin\n        OnGo();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnGo()\n    begin\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            ("src/Log.al", &table(50180, "Dep Log")),
            ("src/A.al", dep_a),
            ("src/B.al", dep_b),
        ],
        "",
    );
}

/// S8.1: the one detector-context builder folds the dependency-internal edges (the
/// R3a-4 intra-app edges) into the cone in cross-app mode, so a write two
/// dependency hops away reaches the workspace subscribers and d44 pairs them.
///
/// Discrimination (2026-10-06): not extending `graph.typed_edges` with
/// `injected_typed_edges` in `build_detector_context_with` loses the finding
/// (`left: 0`); restored, it passes.
#[test]
fn a_dependency_internal_edge_reaches_the_cone() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let dir = tempfile::tempdir().unwrap();
    transitive_dep_write_workspace(dir.path());
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    let p = project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
    assert_eq!(p.finding_count, 1, "{:#?}", p.findings);
}

/// A workspace subscriber of a dependency event writes `Ws Log`.
fn dep_publisher_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(&dir.join("src/Log.al"), &table(50290, "Ws Log"));
    write(
        &dir.join("src/Sub.al"),
        "codeunit 50291 \"Ws Sub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Pub\", 'OnThing', '', false, false)]\n    local procedure OnThing()\n    var\n        Log: Record \"Ws Log\";\n    begin\n        Log.Insert();\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[(
            "src/Pub.al",
            "codeunit 50171 \"Dep Pub\"\n{\n    procedure Raise()\n    begin\n        OnThing();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnThing()\n    begin\n    end;\n}\n",
        )],
        "",
    );
}

/// S8.4 (owner decision): a DEPENDENCY publisher is a d45 root when a primary
/// routine is in its subscriber chain, and the finding anchors on that workspace
/// subscriber (the publisher's own location is dependency source).
///
/// Discrimination (2026-10-06): restoring the primary-publisher-only gate in
/// `detect_d45` loses the finding (`left: 0`); restored, it passes.
#[test]
fn d45_reports_a_dependency_publisher_the_workspace_subscribes_to() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let dir = tempfile::tempdir().unwrap();
    dep_publisher_workspace(dir.path());
    let names = vec!["d45-event-transitive-table-exposure".to_string()];
    let p = project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
    assert_eq!(p.finding_count, 1, "{:#?}", p.findings);
    assert_eq!(
        p.findings[0].primary_location.source_unit_id,
        "ws:src/Sub.al"
    );
}

/// One workspace: tables `Wide` (key + 3 fields), `Narrow` (key + 1 field) and the
/// platform `Field` (2000000041, declared so it RESOLVES, as the cross-app model
/// resolves it), and one codeunit whose `body` is the procedure under test.
fn d3_workspace(dir: &Path, body: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}]}}"#
        ),
    );
    write(
        &dir.join("src/Tables.al"),
        "table 50300 Wide\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n        field(2; A; Text[50]) { }\n        field(3; B; Text[50]) { }\n        field(4; C; Text[50]) { }\n    }\n    keys { key(PK; Code) { Clustered = true; } }\n}\n\ntable 50301 Narrow\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n        field(2; A; Text[50]) { }\n    }\n    keys { key(PK; Code) { Clustered = true; } }\n}\n\ntable 2000000041 Field\n{\n    fields\n    {\n        field(1; TableNo; Integer) { }\n        field(2; \"No.\"; Integer) { }\n        field(3; FieldName; Text[30]) { }\n        field(4; Type; Integer) { }\n    }\n    keys { key(PK; TableNo, \"No.\") { Clustered = true; } }\n}\n",
    );
    write(
        &dir.join("src/Main.al"),
        &format!("codeunit 50302 \"D3 Probe\"\n{{\n{body}\n}}\n"),
    );
}

fn d3_count(dir: &Path) -> usize {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d3-missing-setloadfields".to_string()];
    project_r4_findings_cross_app(dir, "r0", &registered_detectors(), "x", &names).finding_count
}

/// S8.5 triage fixes to d3 (61.5% false positives on the cross-app sample). Each
/// procedure is a shape that must NOT be reported; the last one must be:
/// - A: a virtual system table (`Field`) that RESOLVES — SetLoadFields saves
///   nothing on metadata; the exemption assumed it never resolves.
/// - B: every loadable field is read anyway — nothing to trim.
/// - C: `Rec.Count` without parentheses is a method, not a field.
/// - D: the record escapes to an event publisher, or into a RecordRef
///   (`GetTable`) — the consumer may read any field.
/// - real: `Wide.Get` then reading one of three fields.
///
/// Discrimination (2026-10-06), one break each, each fails the test: the gate
/// treating any resolved table as physical (A); removing the nothing-to-trim skip
/// (B); removing the table-field check (C); treating the publisher / platform
/// callee as an analysable callee (D). Restored, it passes.
#[test]
fn d3_skips_what_setloadfields_cannot_help() {
    let cases = [
        (
            "A",
            "    procedure P()\n    var\n        F: Record Field;\n    begin\n        F.Get(18, 1);\n        Message(F.FieldName);\n    end;",
            0,
        ),
        (
            "B",
            "    procedure P()\n    var\n        N: Record Narrow;\n    begin\n        N.Get('X');\n        Message(N.A);\n    end;",
            0,
        ),
        (
            "C",
            "    procedure P()\n    var\n        W: Record Wide;\n    begin\n        W.Get('X');\n        Message(Format(W.Count));\n    end;",
            0,
        ),
        (
            "D-publisher",
            "    procedure P()\n    var\n        W: Record Wide;\n    begin\n        W.Get('X');\n        Message(W.A);\n        OnAfterGet(W);\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnAfterGet(W: Record Wide)\n    begin\n    end;",
            0,
        ),
        (
            "D-recordref",
            "    procedure P()\n    var\n        W: Record Wide;\n        R: RecordRef;\n    begin\n        W.Get('X');\n        Message(W.A);\n        R.GetTable(W);\n    end;",
            0,
        ),
        (
            "real",
            "    procedure P()\n    var\n        W: Record Wide;\n    begin\n        W.Get('X');\n        Message(W.A);\n    end;",
            1,
        ),
    ];
    for (name, body, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        d3_workspace(dir.path(), body);
        assert_eq!(d3_count(dir.path()), expected, "case {name}");
    }
}

/// S8 engine gap 1 (triage D, d44 Group B): a dependency writes a record its
/// own caller made `temporary`. No physical row is written, so the two workspace
/// subscribers do not overlap. The Core shapes from the triage, each a case:
/// - `var-param`: the helper writes its `var` parameter;
/// - `forwarded`: the `var` parameter is forwarded once more before the write;
/// - `table-method`: a table procedure writes `Rec` (`Buf.ClearBuffer()`);
/// - `physical`: the control — a non-temporary local, so d44 reports;
/// - `event-raise`: the record goes through a PUBLIC event (no closed world) to
///   a subscriber that writes it; the event edge carries the subscriber's
///   parameter into the publisher's frame by name, and the raiser's temporary
///   argument decides (`event-raise-physical` is its control).
///
/// Discrimination for `event-raise` (2026-10-07): no `event-dispatch` arm in
/// `substitute_entry` fails it (`left: 1`); restored, it passes.
///
/// `var-param` and `forwarded` already held; `table-method` was the gap: a table
/// method's `Rec` was `Known(false)`. Discrimination (2026-10-06): seeding `Rec`
/// `Known(false)` again in `ir_record_variables`, or reading only the argument
/// bindings in `pd_temp_state_at_callsite` (ignoring the receiver), fails case
/// `table-method` (`left: 1`); restored, it passes.
#[test]
fn a_dependency_write_to_a_temporary_argument_is_not_physical() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let run2 = |decl: &str, call: &str| {
        format!(
            "codeunit 50181 \"Dep A\"\n{{\n    procedure Run2()\n    var\n        B: Codeunit \"Dep B\";\n        Buf: Record {decl};\n    begin\n        {call};\n    end;\n}}\n"
        )
    };
    let b = "codeunit 50182 \"Dep B\"\n{\n    procedure Write(var Log: Record \"Dep Log\")\n    begin\n        Log.Insert();\n    end;\n\n    procedure Mid(var Log: Record \"Dep Log\")\n    begin\n        Write(Log);\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnFill(var Log: Record \"Dep Log\")\n    begin\n    end;\n}\n\ncodeunit 50184 \"Dep Sub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep B\", 'OnFill', '', false, false)]\n    local procedure Fill(var Log: Record \"Dep Log\")\n    begin\n        Log.Insert();\n    end;\n}\n\ntable 50183 \"Dep Buf\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    keys { key(PK; Code) { Clustered = true; } }\n\n    procedure ClearBuffer()\n    begin\n        DeleteAll();\n    end;\n}\n";
    let cases = [
        (
            "var-param",
            run2("\"Dep Log\" temporary", "B.Write(Buf)"),
            0,
        ),
        ("forwarded", run2("\"Dep Log\" temporary", "B.Mid(Buf)"), 0),
        (
            "table-method",
            run2("\"Dep Buf\" temporary", "Buf.ClearBuffer()"),
            0,
        ),
        ("physical", run2("\"Dep Log\"", "B.Mid(Buf)"), 1),
        (
            "event-raise",
            run2("\"Dep Log\" temporary", "B.OnFill(Buf)"),
            0,
        ),
        (
            "event-raise-physical",
            run2("\"Dep Log\"", "B.OnFill(Buf)"),
            1,
        ),
    ];
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    for (name, a, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        transitive_dep_write_workspace_with(dir.path(), &a, b);
        let p =
            project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
        assert_eq!(p.finding_count, expected, "case {name}: {:#?}", p.findings);
    }
}

/// S8 engine gap 2 (triage D, d44 Group C): a dependency writes only under
/// `if UpdateCache then`, and the dependency caller the workspace reaches passes
/// a literal `false`. The write cannot run on that path, so the two workspace
/// subscribers do not overlap. Each case is a `Dep A.Run2` body over the same
/// `Dep B`:
/// - `literal-false`: `B.GetState(false)` -> `if UpdateCache then Refresh()`;
/// - `forwarded`: `B.Outer(false)` forwards its parameter to `GetState`;
/// - `early-exit`: `if not UpdateCache then exit;` before the write;
/// - `literal-true`, `assigned` and `cleared` (the callee overwrites the
///   parameter) and `variable` (a local, not a literal): the controls, so d44
///   reports.
///
/// Discrimination (2026-10-07), each break fails the named case (`left: 1`) and
/// passes restored: a contradicting literal not dropping the fact
/// (`literal-false`); a forwarded parameter dropping its requirement
/// (`forwarded`); no early-exit guard (`early-exit`); the member's guarded call
/// edge adding no requirement in `fact_cone_for_scc` (`literal-false`); `Clear`
/// not counted as writing its argument in `guard_frames` (`cleared`).
#[test]
fn a_dependency_write_behind_a_false_literal_is_not_reached() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let run2 = |call: &str| {
        format!(
            "codeunit 50181 \"Dep A\"\n{{\n    procedure Run2()\n    var\n        B: Codeunit \"Dep B\";\n        Flag: Boolean;\n    begin\n        Flag := false;\n        {call};\n    end;\n}}\n"
        )
    };
    let b = "codeunit 50182 \"Dep B\"\n{\n    procedure GetState(UpdateCache: Boolean)\n    begin\n        if UpdateCache then\n            Refresh();\n    end;\n\n    procedure Outer(Update: Boolean)\n    begin\n        GetState(Update);\n    end;\n\n    procedure Guarded(UpdateCache: Boolean)\n    var\n        Log: Record \"Dep Log\";\n    begin\n        if not UpdateCache then\n            exit;\n        Log.Insert();\n    end;\n\n    procedure Assigned(UpdateCache: Boolean)\n    begin\n        UpdateCache := true;\n        if UpdateCache then\n            Refresh();\n    end;\n\n    procedure IsAny(): Boolean\n    var\n        Buf: Record \"Dep Log\" temporary;\n    begin\n        GetAll(false, Buf);\n        exit(not Buf.IsEmpty());\n    end;\n\n    procedure GetAll(LoadLogos: Boolean; var Buf: Record \"Dep Log\" temporary)\n    var\n        Src: Record \"Dep Log\";\n        I: Integer;\n        L: List of [Integer];\n    begin\n        foreach I in L do begin\n            if Src.FindSet() then\n                repeat\n                    Buf := Src;\n                    if LoadLogos then begin\n                        Refresh();\n                    end;\n                    if not Buf.Insert() then;\n                until Src.Next() = 0;\n        end;\n    end;\n\n    procedure Cleared(UpdateCache: Boolean)\n    begin\n        Clear(UpdateCache);\n        if not UpdateCache then\n            Refresh();\n    end;\n\n    local procedure Refresh()\n    var\n        Log: Record \"Dep Log\";\n    begin\n        Log.Insert();\n    end;\n}\n";
    let cases = [
        ("literal-false", "B.GetState(false)", 0),
        ("forwarded", "B.Outer(false)", 0),
        ("early-exit", "B.Guarded(false)", 0),
        ("literal-true", "B.GetState(true)", 1),
        ("assigned", "B.Assigned(false)", 1),
        ("cleared", "B.Cleared(true)", 1),
        ("nested-loop-guard", "B.IsAny()", 0),
        ("variable", "B.GetState(Flag)", 1),
    ];
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    for (name, call, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        transitive_dep_write_workspace_with(dir.path(), &run2(call), b);
        let p =
            project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
        assert_eq!(p.finding_count, expected, "case {name}: {:#?}", p.findings);
    }
}

/// One workspace: three physical tables, a `D8 Probe` codeunit whose `Caller`
/// body is the case under test, `Committer` (commits; `own` decides whether it
/// first writes all three tables), `W3` (writes all three), and `W3 CU`, whose
/// `OnRun` writes all three.
fn d8_workspace(dir: &Path, caller: &str, own: bool) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50499}}]}}"#
        ),
    );
    let tables: String = (1..=3)
        .map(|i| table(50400 + i, &format!("T{i}")))
        .collect::<Vec<_>>()
        .join("\n");
    write(&dir.join("src/Tables.al"), &tables);
    let writes = "        A.Insert();\n        B.Insert();\n        C.Insert();\n";
    let vars = "    var\n        A: Record T1;\n        B: Record T2;\n        C: Record T3;\n";
    let own_writes = if own { writes } else { "" };
    write(
        &dir.join("src/Probe.al"),
        &format!(
            "codeunit 50410 \"D8 Probe\"\n{{\n    procedure Caller(X: Boolean)\n    var\n        CU: Codeunit \"W3 CU\";\n    begin\n{caller}\n    end;\n\n    procedure Committer()\n{vars}    begin\n{own_writes}        Commit();\n    end;\n\n    procedure CommitIf(DoIt: Boolean)\n    begin\n        if not DoIt then\n            exit;\n        Commit();\n    end;\n\n    procedure W3()\n{vars}    begin\n{writes}    end;\n}}\n\ncodeunit 50411 \"W3 CU\"\n{{\n    trigger OnRun()\n{vars}    begin\n{writes}    end;\n}}\n"
        ),
    );
}

/// S8 engine gap 3 (triage C, d8 100% false positives cross-app): a routine is a
/// transaction "manager" only by the physical tables it writes BEFORE its call
/// toward the Commit, not by its whole cone. Each case is `Caller`'s body:
/// - `commit-own-writes`: the Commit routine's own writes are not its caller's;
/// - `after-commit`: writes after the call that commits;
/// - `sibling-branch`: writes in the branch that does not commit;
/// - `checked-run`: a checked `Codeunit.Run` writes in its own transaction;
/// - `case-branch`: a `case` runs one branch, not the earlier ones too;
/// - `earlier-commit`: a `Commit()` before the call commits what came before;
/// - `exit-arm`: writes in an `if` arm that always exits never reach the call;
/// - `checked-var-run`: `if CU.Run()` on a codeunit variable is a checked run;
/// - `guarded-commit`: the Commit runs only when `DoIt`, and the caller passes
///   `false`, so the caller is not in its transaction at all;
/// - `before`, `loop` (an earlier iteration's writes) and `guarded-commit-true`:
///   d8 reports.
///
/// Discrimination (2026-10-07), each break fails the named cases and passes
/// restored, in `pending_writes.rs`: counting the call toward the Commit itself
/// (`commit-own-writes`); collecting the statements after the target
/// (`after-commit`, `sibling-branch`); collecting both branches of the `if`
/// (`sibling-branch`, and with it `commit-own-writes`, `after-commit`); no
/// checked-run skip (`checked-run`); no loop rule (`loop`). Then, also 2026-10-07:
/// no `case` arm in `walk_node` (`case-branch`); no `out.clear()` at an earlier
/// `Commit()` (`earlier-commit`); every `if` arm counted as reaching
/// (`exit-arm`); no codeunit-variable `Run` in `is_checked_run`
/// (`checked-var-run`); the span walk ignoring the caller's arguments
/// (`guarded-commit`).
#[test]
fn d8_counts_only_the_writes_pending_at_the_commit() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let cases = [
        ("commit-own-writes", "        Committer();", true, 0),
        (
            "after-commit",
            "        Committer();\n        W3();",
            false,
            0,
        ),
        (
            "sibling-branch",
            "        if X then\n            W3()\n        else\n            Committer();",
            false,
            0,
        ),
        (
            "checked-run",
            "        if Codeunit.Run(Codeunit::\"W3 CU\") then;\n        Committer();",
            false,
            0,
        ),
        (
            "case-branch",
            "        case X of\n            true:\n                W3();\n            false:\n                Committer();\n        end;",
            false,
            0,
        ),
        (
            "earlier-commit",
            "        W3();\n        Commit();\n        Committer();",
            false,
            0,
        ),
        (
            "exit-arm",
            "        if X then begin\n            W3();\n            exit;\n        end;\n        Committer();",
            false,
            0,
        ),
        (
            "checked-var-run",
            "        if CU.Run() then;\n        Committer();",
            false,
            0,
        ),
        (
            "guarded-commit",
            "        W3();\n        CommitIf(false);",
            false,
            0,
        ),
        (
            "guarded-commit-true",
            "        W3();\n        CommitIf(true);",
            false,
            1,
        ),
        ("before", "        W3();\n        Committer();", false, 1),
        (
            "loop",
            "        while X do begin\n            Committer();\n            W3();\n        end;",
            false,
            1,
        ),
    ];
    let names = vec!["d8-commit-in-transaction".to_string()];
    let mut wrong: Vec<String> = Vec::new();
    for (name, caller, own, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        d8_workspace(dir.path(), caller, own);
        let p =
            project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names);
        if p.finding_count != expected {
            wrong.push(format!("{name}: {} (want {expected})", p.finding_count));
        }
    }
    assert!(wrong.is_empty(), "cases {wrong:?}");
}

const DEP2_GUID: &str = "dddd4444-0000-0000-0000-0000000000a7";

/// S8.6: the cone follows EVERY resolved call dependency code makes, not only
/// direct calls inside one dependency app. Each case is a `Dep A.Run2` whose write
/// reaches the two workspace subscribers only through such a call, so d44 pairs
/// them:
/// - `codeunit-run`: `Codeunit.Run(Codeunit::"Dep B")`, whose `OnRun` writes;
/// - `cross-dependency`: a call from `XDep` into a second app `XDep2`, which
///   writes its own table.
///
/// Discrimination (2026-10-07): injecting only the S7 admitted own-app edges
/// (direct, resolved method, interface `Maybe`, same app) fails both cases
/// (`left: 0`); injecting none also fails both; restored, both pass.
#[test]
fn the_cone_follows_every_dependency_call() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    let count = |dir: &Path| {
        project_r4_findings_cross_app(dir, "r0", &registered_detectors(), "x", &names).finding_count
    };

    let dir = tempfile::tempdir().unwrap();
    transitive_dep_write_workspace_with(
        dir.path(),
        "codeunit 50181 \"Dep A\"\n{\n    procedure Run2()\n    begin\n        Codeunit.Run(Codeunit::\"Dep B\");\n    end;\n}\n",
        "codeunit 50182 \"Dep B\"\n{\n    trigger OnRun()\n    var\n        Log: Record \"Dep Log\";\n    begin\n        Log.Insert();\n    end;\n}\n",
    );
    let mut wrong: Vec<String> = Vec::new();
    let n = count(dir.path());
    if n != 1 {
        wrong.push(format!("codeunit-run: {n}"));
    }

    let dir = tempfile::tempdir().unwrap();
    transitive_dep_write_workspace_with(
        dir.path(),
        "codeunit 50181 \"Dep A\"\n{\n    procedure Run2()\n    var\n        W: Codeunit \"Dep2 W\";\n    begin\n        W.Write();\n    end;\n}\n",
        "codeunit 50182 \"Dep B\"\n{\n}\n",
    );
    // XDep now depends on XDep2, which writes its own table.
    let dep_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.path().join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &dep_symbols,
        &[
            ("src/Log.al", &table(50180, "Dep Log")),
            (
                "src/A.al",
                "codeunit 50181 \"Dep A\"\n{\n    procedure Run2()\n    var\n        W: Codeunit \"Dep2 W\";\n    begin\n        W.Write();\n    end;\n}\n",
            ),
        ],
        &format!(
            r#"<Dependencies><Dependency Id="{DEP2_GUID}" Name="XDep2" Publisher="probe" MinVersion="1.0.0.0" /></Dependencies>"#
        ),
    );
    let dep2_symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP2_GUID}","Name":"XDep2","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.path().join(".alpackages/probe_XDep2_1.0.0.0.app"),
        DEP2_GUID,
        "XDep2",
        "1.0.0.0",
        &dep2_symbols,
        &[
            ("src/Log2.al", &table(50190, "Dep2 Log")),
            (
                "src/W.al",
                "codeunit 50191 \"Dep2 W\"\n{\n    procedure Write()\n    var\n        Log: Record \"Dep2 Log\";\n    begin\n        Log.Insert();\n    end;\n}\n",
            ),
        ],
        "",
    );
    let n = count(dir.path());
    if n != 1 {
        wrong.push(format!("cross-dependency: {n}"));
    }
    assert!(wrong.is_empty(), "cases {wrong:?}");
}

/// One workspace whose two subscribers write the `var Buf` of the dependency
/// event `Dep Feat.OnRequest`, raised by `Dep Feat.Collect` with `buf` (a
/// `Dep Log` declaration, `temporary` or not); `access` is the publisher's.
/// `sub_body` is each subscriber's body.
fn event_temp_workspace(dir: &Path, buf: &str, access: &str, sub_body: &str) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    let sub = |n: u32| {
        format!(
            "codeunit {n} \"Ws Sub {n}\"\n{{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Feat\", 'OnRequest', '', false, false)]\n    local procedure Handle(var Buf: Record \"Dep Log\")\n    begin\n{sub_body}\n    end;\n\n    local procedure Helper(var B: Record \"Dep Log\")\n    begin\n        B.Insert();\n    end;\n}}\n"
        )
    };
    write(&dir.join("src/Sub1.al"), &sub(50281));
    write(&dir.join("src/Sub2.al"), &sub(50282));
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[
            ("src/Log.al", &table(50180, "Dep Log")),
            (
                "src/Feat.al",
                &format!(
                    "codeunit 50183 \"Dep Feat\"\n{{\n    procedure Collect()\n    var\n        Tmp: Record {buf};\n    begin\n        OnRequest(Tmp);\n    end;\n\n    [IntegrationEvent(false, false)]\n    {access}procedure OnRequest(var Buf: Record \"Dep Log\")\n    begin\n    end;\n}}\n"
                ),
            ),
        ],
        "",
    );
}

/// S8 (S8.6 triage cause 5): a `local` event raised only with a `temporary`
/// record makes each subscriber's same-named `var` parameter temporary
/// (`event_param_temp`), so two subscribers writing it do not overlap (d44):
/// - `local-temp`: the subscribers write `Buf` directly;
/// - `forwarded`: they pass it to a helper that writes it;
/// - `physical-raise` and `public-publisher` (anyone may raise it): d44 reports.
///
/// Discrimination (2026-10-07): not calling `prove_event_param_temps` fails
/// `local-temp` and `forwarded`; dropping the `local` access rule fails
/// `public-publisher`; restored, all pass.
#[test]
fn a_local_event_raised_with_temporary_records_has_temporary_subscribers() {
    use al_sem::engine::l5::detectors::registered_detectors;
    use al_sem::engine::l5::finding::project_r4_findings_cross_app;
    let names = vec!["d44-event-multi-subscriber-overlap".to_string()];
    let cases = [
        (
            "local-temp",
            "\"Dep Log\" temporary",
            "local ",
            "        Buf.Insert();",
            0,
        ),
        (
            "forwarded",
            "\"Dep Log\" temporary",
            "local ",
            "        Helper(Buf);",
            0,
        ),
        (
            "physical-raise",
            "\"Dep Log\"",
            "local ",
            "        Buf.Insert();",
            1,
        ),
        (
            "public-publisher",
            "\"Dep Log\" temporary",
            "",
            "        Buf.Insert();",
            1,
        ),
    ];
    let mut wrong: Vec<String> = Vec::new();
    for (name, buf, access, body, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        event_temp_workspace(dir.path(), buf, access, body);
        let n =
            project_r4_findings_cross_app(dir.path(), "r0", &registered_detectors(), "x", &names)
                .finding_count;
        if n != expected {
            wrong.push(format!("{name}: {n} (want {expected})"));
        }
    }
    assert!(wrong.is_empty(), "cases {wrong:?}");
}

/// S9.0d: a run of a DEPENDENCY page whose source declares no `OnOpenPage`
/// reaches no routine (the resolver no longer invents an Opaque trigger), and
/// the model still sees the dependency callee it saw before: an external
/// callee naming that page, `PageRun` for `Page.Run(..)` and a method call for
/// `PageVar.RunModal()`. Without the run target the adapter would turn both
/// into the workspace shape (`PageRun`, no external type).
#[test]
fn a_run_of_a_dependency_page_without_entry_trigger_names_the_page() {
    use al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &dir.path().join("src/Main.al"),
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        P: Page \"Dep Page\";\n    begin\n        Page.Run(Page::\"Dep Page\");\n        P.RunModal();\n    end;\n}\n",
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Pages":[{{"Id":50110,"Name":"Dep Page"}}],"AppId":"{DEP_GUID}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    write_source_app(
        &dir.path().join(".alpackages/probe_XDep_1.0.0.0.app"),
        DEP_GUID,
        "XDep",
        "1.0.0.0",
        &symbols,
        &[(
            "src/DepPage.al",
            "page 50110 \"Dep Page\"\n{\n    PageType = ConfirmationDialog;\n}\n",
        )],
        "",
    );
    let m = assemble_and_resolve_cross_app_program(dir.path(), MI, false)
        .expect("model")
        .resolved;
    let calls = m.calls.clone();
    let mut got: Vec<String> = calls
        .edges
        .iter()
        .filter(|e| routine_label(&m, &e.from) == "50201.Go")
        .map(|e| {
            format!(
                "{:?} {:?} to={} ext={:?}",
                e.dispatch_kind,
                e.resolution,
                e.to.is_some(),
                e.external_type_ref
                    .as_ref()
                    .map(|t| (t.kind.as_str(), t.name.as_str()))
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            r#"Method ExternalTarget to=false ext=Some(("Page", "Dep Page"))"#.to_string(),
            r#"PageRun Opaque to=false ext=Some(("Page", "Dep Page"))"#.to_string(),
        ]
    );
}
