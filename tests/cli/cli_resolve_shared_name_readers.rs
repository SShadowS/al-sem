//! ENG-193 (Fable r1): readers of an object name that two visible apps share (the workspace's `W`
//! objects beside a dependency's `KA` objects of the same names, each binding the own object in
//! alc 18.0.41 since every workspace file is `namespace W;`) must not lose reach once the name no
//! longer binds the own object by default:
//! - B1: the workspace's `pageextension PE extends P` still runs with page `P`, and `P`'s
//!   `SourceTable = T` still reaches `T`'s triggers (`alsem graph`);
//! - B2: an integration event whose subscriber names its publisher by a shared name (`Codeunit::X`,
//!   and `Database::T` for a tableextension's event) is not "dead" (d12);
//! - B3 (Fable r2): nor is an extension's event whose subscriber names the base by number
//!   (`ObjectType::Table, 50030` for `TE`'s, `ObjectType::Page, 50030` for `PE`'s); the unsubscribed
//!   `X::OnDead` and `TE::OnExtDead` are the control and stay dead;
//! - B4 (Fable r3): a subscriber naming the DEPENDENCY's `T`/`P` by number (61001) binds the
//!   dependency's event, never the workspace extension's event of the same name, so `TE`'s
//!   `OnShared`/`OnSharedArg` and `PE`'s `OnPShared` stay dead and the subscribers stay bound.

use std::path::Path;
use std::process::Command;

const DEP_GUID: &str = "19319319-0000-4000-8000-0000000000b1";
const WS_GUID: &str = "19319319-0000-4000-8000-000000000002";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"15.0","Namespaces":[{"Name":"KA","Codeunits":[{"Id":61001,"Name":"X","Methods":[{"Name":"F","Parameters":[]}]}],"Tables":[{"Id":61001,"Name":"T","Fields":[{"Id":1,"Name":"Code","TypeDefinition":{"Name":"Code"}}]}],"Pages":[{"Id":61001,"Name":"P"}]}]}"#;

const DEP_SOURCE: &str = "namespace KA;\n\ncodeunit 61001 X\n{\n    procedure F() begin end;\n}\ntable 61001 T\n{\n    fields { field(1; Code; Code[20]) { } }\n    keys { key(PK; Code) { } }\n    [IntegrationEvent(false, false)]\n    procedure OnShared() begin end;\n    [IntegrationEvent(false, false)]\n    procedure OnSharedArg(A: Integer) begin end;\n    procedure RaiseT() begin OnShared(); OnSharedArg(1); end;\n}\npage 61001 P\n{\n    SourceTable = T;\n    layout { area(Content) { field(Code; Rec.Code) { } } }\n    [IntegrationEvent(false, false)]\n    procedure OnPShared() begin end;\n    procedure RaiseP() begin OnPShared(); end;\n}\n";

const WS_SOURCE: &str = r#"namespace W;

table 50030 T
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
    trigger OnInsert() begin end;
}
tableextension 50040 TE extends T
{
    trigger OnBeforeInsert() begin end;
    [IntegrationEvent(false, false)]
    procedure OnExt() begin end;
    [IntegrationEvent(false, false)]
    procedure OnExtByNumber() begin end;
    [IntegrationEvent(false, false)]
    procedure OnExtDead() begin end;
    [IntegrationEvent(false, false)]
    procedure OnShared() begin end;
    [IntegrationEvent(false, false)]
    procedure OnSharedArg() begin end;
    procedure RaiseExt() begin OnExt(); OnExtByNumber(); OnExtDead(); OnShared(); OnSharedArg(); end;
}
page 50030 P
{
    SourceTable = T;
    layout { area(Content) { field(Code; Rec.Code) { } } }
    trigger OnOpenPage() begin end;
}
pageextension 50041 PE extends P
{
    trigger OnOpenPage() begin end;
    [IntegrationEvent(false, false)]
    procedure OnPExtByNumber() begin end;
    [IntegrationEvent(false, false)]
    procedure OnPShared() begin end;
    procedure RaisePE() begin OnPExtByNumber(); OnPShared(); end;
}
codeunit 50032 X
{
    [IntegrationEvent(false, false)]
    procedure OnE() begin end;
    [IntegrationEvent(false, false)]
    procedure OnDead() begin end;
    procedure Raise() begin OnE(); OnDead(); end;
}
codeunit 50031 Sub
{
    [EventSubscriber(ObjectType::Table, Database::T, 'OnExt', '', false, false)]
    local procedure H1() begin end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::X, 'OnE', '', false, false)]
    local procedure H2() begin end;
    [EventSubscriber(ObjectType::Table, 50030, 'OnExtByNumber', '', false, false)]
    local procedure H3() begin end;
    [EventSubscriber(ObjectType::Page, 50030, 'OnPExtByNumber', '', false, false)]
    local procedure H4() begin end;
    [EventSubscriber(ObjectType::Table, 61001, 'OnShared', '', false, false)]
    local procedure H5() begin end;
    [EventSubscriber(ObjectType::Table, 61001, 'OnSharedArg', '', false, false)]
    local procedure H6() begin end;
    [EventSubscriber(ObjectType::Page, 61001, 'OnPShared', '', false, false)]
    local procedure H7() begin end;
}
codeunit 50010 U
{
    procedure G() begin Page.Run(Page::P); end;
}
"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{WS_GUID}","name":"Ws193b","publisher":"t","version":"1.0.0.0","runtime":"15.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"Dep193b","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/W.al"), WS_SOURCE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep193b_1.0.0.0.app"),
        DEP_GUID,
        "Dep193b",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/KA.al", DEP_SOURCE)],
        "",
    );
    ws
}

fn alsem(ws: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg(ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Discrimination (2026-11-24, each break then restored, green): the extension indices built
/// from `resolve_object` (one base or none) instead of `visible_objects` fail the `PE` edge; the
/// page's ambiguous source table dropped (no `AnyOf` branch) fails the `T::OnInsert` edge.
#[test]
fn an_extension_and_a_source_table_of_a_shared_name_keep_their_edges() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let recs: Vec<serde_json::Value> = alsem(&ws, &["graph"])
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let node = |kind: &str, object_type: &str, object: &str, member: Option<&str>| -> String {
        recs.iter()
            .find(|r| {
                r["t"] == "node"
                    && r["v"]["kind"] == kind
                    && r["v"]["objectType"] == object_type
                    && r["v"]["objectName"] == object
                    && r["v"]["appId"] == WS_GUID
                    && member.is_none_or(|m| r["v"]["memberName"] == m)
            })
            .unwrap_or_else(|| panic!("precondition: {object_type} {object} {member:?} is a node"))
            ["v"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let edge = |from: &str, to: &str| {
        recs.iter()
            .any(|r| r["t"] == "edge" && r["v"]["caller"] == from && r["v"]["callee"] == to)
    };
    let page = node("Object", "Page", "P", None);
    let pe_open = node("Trigger", "PageExtension", "PE", Some("OnOpenPage"));
    let t_insert = node("Trigger", "Table", "T", Some("OnInsert"));
    assert!(
        edge(&page, &pe_open),
        "the page runs its extension's OnOpenPage"
    );
    assert!(
        edge(&page, &t_insert),
        "the page's source table's OnInsert is reachable"
    );
    // B4: a subscriber naming the DEPENDENCY's `T`/`P` by number binds its event; the
    // workspace's `TE`/`PE` (whose base name the dependency shares) declare events of the same
    // name and must not make the binding ambiguous.
    for sub in ["H5", "H7"] {
        let hidden = &recs
            .iter()
            .find(|r| {
                r["t"] == "node"
                    && r["v"]["objectName"] == "Sub"
                    && r["v"]["memberName"] == sub
                    && r["v"]["appId"] == WS_GUID
            })
            .unwrap_or_else(|| panic!("precondition: Sub.{sub} is a node"))["v"]["hiddenCallers"];
        assert!(
            !hidden.to_string().contains("unboundSubscriber"),
            "Sub.{sub} is bound: {hidden}"
        );
    }
}

/// Discrimination (2026-11-24): d12 without the `maybe_subscribed` check reports both `OnE` and
/// `OnExt` dead; with it but without the base-object reference, `OnExt` (a tableextension's event,
/// subscribed by `Database::T`). `PublisherExtensions::build` from `resolve_object` (an extension
/// listed under a unique base only) adds `OnExtByNumber` and `OnPExtByNumber`. B4 (Fable r3): with
/// the shared extensions joined to the first candidate set instead of a fallback, `H6` binds
/// `TE::OnSharedArg` (exact arity beats the dependency's `OnSharedArg(A)`) and drops it from
/// this list; the other test's `H5` then reads `unboundSubscriber` (ambiguous).
#[test]
fn an_event_subscribed_through_a_shared_name_is_not_dead() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let out = alsem(&ws, &["analyze", "--scope", "all", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    let d12: Vec<String> = v["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == "d12-dead-integration-event")
        .map(|f| f["primaryLocation"]["routineName"].to_string())
        .collect();
    let mut d12 = d12;
    d12.sort();
    assert_eq!(
        d12,
        [
            r#""OnDead""#,
            r#""OnExtDead""#,
            r#""OnPShared""#,
            r#""OnShared""#,
            r#""OnSharedArg""#
        ],
        "only the unsubscribed events are dead"
    );
}
