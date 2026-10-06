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
        "codeunit 50201 \"Ws Main\"\n{\n    procedure Go()\n    var\n        W: Codeunit \"Dep Worker\";\n    begin\n        W.Work();\n    end;\n}\n",
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

/// Every cross-app fixture: the program-built cross-app model's rows equal the
/// legacy merged model's (`build_cross_app_l3_r4`, the source-parsing variant) —
/// the same rows in the same order (the symbol table is last-wins and the
/// extension-field merge first-wins, so order is part of the contract), each row's
/// whole content equal.
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

/// Each row's whole `Debug` text. `decode` percent-decodes every `dep:` source-unit
/// id: the legacy path keeps a zip entry name raw, the program engine decodes it
/// once (plan decision 7; ids do not depend on it).
fn model_rows(
    ws: &al_sem::engine::l3::l3_workspace::L3Workspace,
    decode: bool,
) -> [Vec<String>; 3] {
    let fix = |row: String| {
        if !decode {
            return row;
        }
        let mut out = String::with_capacity(row.len());
        let mut rest = row.as_str();
        while let Some(i) = rest.find("\"dep:") {
            let (head, tail) = rest.split_at(i + 1);
            out.push_str(head);
            let end = tail.find('"').unwrap_or(tail.len());
            out.push_str(&percent_encoding::percent_decode_str(&tail[..end]).decode_utf8_lossy());
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    };
    [
        ws.objects.iter().map(|o| fix(format!("{o:?}"))).collect(),
        ws.tables.iter().map(|t| fix(format!("{t:?}"))).collect(),
        ws.routines.iter().map(|r| fix(format!("{r:?}"))).collect(),
    ]
}

///
/// Discrimination (2026-10-06): appending the symbol-only (ABI) rows AFTER the
/// parsed dependency rows in `append_dependency_rows` fails the order assertion on
/// `r3a5-fixtures/ws`; restored, it passes. Measured once on CDO (all rows equal,
/// same order, after decoding) and DO (571 extra objects, all from apps in an
/// ancestor `.alpackages` that the legacy scan never read: plan decision 3).
#[test]
fn the_cross_app_model_rows_equal_the_legacy_merged_model() {
    use al_sem::engine::deps::cross_app_l3::build_cross_app_l3_r4;
    use al_sem::engine::l3::l3_workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    use al_sem::program::model::workspace::assemble_and_resolve_cross_app_from_program;
    for ws in cross_app_fixtures() {
        let legacy = build_cross_app_l3_r4(&ws, MI).expect("legacy model");
        let ctx = build_context(&ws).expect("context");
        let (new, _) = assemble_and_resolve_cross_app_from_program(&ws, MI, false, &ctx)
            .expect("program model");
        // Not degenerate: dependency rows are present, ABI and parsed alike on the
        // fixture that has both kinds.
        let primary = new.primary_app.clone().expect("primary app");
        let dep: Vec<_> = new
            .workspace
            .routines
            .iter()
            .filter(|r| !r.app_guid.eq_ignore_ascii_case(&primary.app_guid))
            .collect();
        assert!(!dep.is_empty(), "{}: no dependency routines", ws.display());
        if ws.ends_with("r3a5-fixtures/ws") {
            assert!(dep.iter().any(|r| r.body_available) && dep.iter().any(|r| !r.body_available));
        }
        let (old_rows, new_rows) = (
            model_rows(&legacy.resolved.workspace, true),
            model_rows(&new.workspace, false),
        );
        for (kind, (o, n)) in ["objects", "tables", "routines"]
            .iter()
            .zip(old_rows.iter().zip(new_rows.iter()))
        {
            let only_old: Vec<_> = o.iter().filter(|r| !n.contains(r)).collect();
            let only_new: Vec<_> = n.iter().filter(|r| !o.contains(r)).collect();
            assert!(
                only_old.is_empty() && only_new.is_empty(),
                "{}: {kind} differ\nonly legacy: {only_old:#?}\nonly program: {only_new:#?}",
                ws.display()
            );
            assert_eq!(o, n, "{}: {kind} order differs", ws.display());
        }
    }
}

/// `"<object number>.<routine name>"` of a model routine id.
fn routine_label(m: &al_sem::engine::l3::l3_workspace::L3Resolved, id: &str) -> String {
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
/// event graph binds a subscriber that lives in the dependency.
///
/// Discrimination (2026-10-06): limiting `Converter::model_apps` to the primary app
/// turns the three calls into the dependency into to-less `ExternalTarget` edges
/// (and the dependency's own call sites get no program edge); limiting the event
/// graph's `in_model` to the primary app leaves `HandleFoo` unmapped. Each fails
/// the test; restored, it passes.
#[test]
fn the_cross_app_model_resolves_dependency_bodies_and_events() {
    use al_sem::engine::l3::l3_workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    use al_sem::engine::l3::program_calls::assemble_and_resolve_cross_app_program;
    let dir = tempfile::tempdir().unwrap();
    shared_name_workspace(dir.path());
    let m = assemble_and_resolve_cross_app_program(dir.path(), MI, false)
        .expect("model")
        .resolved;
    let calls = m.precomputed_calls.clone().expect("calls attached");
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
        ]
    );

    let events = m.precomputed_events.clone().expect("events attached");
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
    use al_sem::engine::l3::l3_workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    use al_sem::engine::l3::program_calls::assemble_and_resolve_cross_app_program;
    let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r3a5-fixtures/ws");
    let m = assemble_and_resolve_cross_app_program(&ws, MI, false)
        .expect("model")
        .resolved;
    let calls = m.precomputed_calls.clone().expect("calls attached");
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

/// S7.4, end to end through the program-backed cross-app base: d13 flags a call
/// into a dependency's `internal` procedure that the workspace may make — as a
/// friend; that is the only way such code compiles. Without the friend entry the
/// call cannot compile, the program resolver refuses it (`InternalNotVisible`),
/// and there is no edge for d13 to flag. (The legacy L3 resolver ignored
/// visibility and flagged both; `ws-d13-member-call`'s dependency now names its
/// workspace as a friend for this reason.)
///
/// Discrimination (2026-10-06): making `resolver::internal_visible_across` always
/// true (the legacy resolver's blindness to visibility) fails the stranger case
/// (`left: 1`); restored, it passes.
#[test]
fn d13_flags_a_friend_call_into_an_internal_procedure() {
    let friend = tempfile::tempdir().unwrap();
    internal_call_workspace(friend.path(), true);
    assert_eq!(d13_count(friend.path()), 1);
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
    use al_sem::engine::l3::l3_workspace::MODEL_INSTANCE_ID_DEFAULT as MI;
    use al_sem::engine::l3::program_calls::assemble_and_resolve_cross_app_program;
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
