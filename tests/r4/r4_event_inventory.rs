//! Engine-switch S4 (spec G7, issue #57): the detectors' event graph comes from
//! the program engine's subscription inventory on the `alsem analyze` path.
//!
//! Before S4, L3 built it from the workspace symbol table alone, so a subscriber
//! to a DEPENDENCY publisher was an `unknown` edge and `build_event_flow_indexes`
//! dropped it: d44 could never see two workspace subscribers of a Base App event.
//! These tests state their preconditions as source text (a symbol-only dependency
//! that publishes the event; a workspace publisher whose overloads make one
//! subscription ambiguous) and run the production path
//! (`assemble_and_resolve_workspace_with_program_calls` -> the detector context).

use std::collections::HashMap;
use std::path::Path;

use crate::symbol_app::write_symbol_app;

use al_sem::engine::l5::detector_context::build_detector_context;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::event_flow::compute_fanout;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::events::PublisherRef;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

const WS_GUID: &str = "aaaa1111-0000-0000-0000-000000000057";
const DEP_GUID: &str = "bbbb2222-0000-0000-0000-000000000057";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn app_json(dependencies: &str) -> String {
    format!(
        r#"{{"id":"{WS_GUID}","name":"EvWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50570,"to":50599}}],"dependencies":[{dependencies}]}}"#
    )
}

/// Workspace: two codeunits subscribe to the dependency event `OnAfterPost` and
/// both write table "Ev Log"; `HandleBoth` also subscribes to `OnBeforePost`.
fn dependency_publisher_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &app_json(&format!(
            r#"{{"id":"{DEP_GUID}","name":"EvDep","publisher":"probe","version":"1.0.0.0"}}"#
        )),
    );
    let event = |name: &str| {
        format!(
            r#"{{"Name":"{name}","Parameters":[],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"false"}},{{"Value":"false"}}]}}]}}"#
        )
    };
    write_symbol_app(
        &dir.join(".alpackages/probe_EvDep_1.0.0.0.app"),
        DEP_GUID,
        "EvDep",
        "1.0.0.0",
        &format!(
            r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":60570,"Name":"Ev Dep Pub","Methods":[{},{}]}}],"AppId":"{DEP_GUID}","Name":"EvDep","Publisher":"probe","Version":"1.0.0.0"}}"#,
            event("OnAfterPost"),
            event("OnBeforePost")
        ),
    );
    write(
        &dir.join("src/Subs.al"),
        r#"table 50570 "Ev Log"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50571 "Ev Sub A"
{
    // The event named as an identifier, not a text literal (S4.3a).
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Ev Dep Pub", OnAfterPost, '', false, false)]
    local procedure HandleAfterPost()
    var
        Log: Record "Ev Log";
    begin
        Log.Insert();
    end;
}

codeunit 50572 "Ev Sub B"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Ev Dep Pub", 'OnAfterPost', '', false, false)]
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Ev Dep Pub", 'OnBeforePost', '', false, false)]
    local procedure HandleBoth()
    var
        Log: Record "Ev Log";
    begin
        Log.Modify();
    end;
}

codeunit 50573 "Ev Sub C"
{
    // The publisher named by object number, not by name (S4.3b).
    [EventSubscriber(ObjectType::Codeunit, 60570, 'OnBeforePost', '', false, false)]
    local procedure HandleBeforePost()
    var
        Log: Record "Ev Log";
    begin
        Log.Delete();
    end;
}
"#,
    );
}

/// d44 sees two workspace subscribers of a DEPENDENCY event writing one table.
#[test]
fn d44_sees_workspace_subscribers_of_a_dependency_event() {
    let dir = tempfile::tempdir().unwrap();
    dependency_publisher_workspace(dir.path());
    let resolved =
        assemble_and_resolve_workspace_with_program_calls(dir.path()).expect("workspace resolves");

    let event_id = format!("{DEP_GUID}/Codeunit/60570/event/onafterpost");
    let d44: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d44-event-multi-subscriber-overlap")
        .collect();
    assert_eq!(d44.len(), 1, "d44 is registered");
    let run = run_detectors(&resolved, &d44);
    let ids: Vec<&str> = run.findings.iter().map(|f| f.id.as_str()).collect();
    assert!(
        ids.iter()
            .any(|id| id.starts_with(&format!("d44/{event_id}|"))),
        "two subscribers of the dependency event write \"Ev Log\": d44 must report \
         the overlap; findings: {ids:?}"
    );
    // `OnBeforePost`'s second subscriber names the publisher by number.
    let before_prefix = format!("d44/{DEP_GUID}/Codeunit/60570/event/onbeforepost|");
    assert!(
        ids.iter().any(|id| id.starts_with(&before_prefix)),
        "HandleBoth and the by-number HandleBeforePost both write \"Ev Log\" on \
         OnBeforePost; findings: {ids:?}"
    );

    // The dependency publisher is a symbol with no model routine, and the
    // routine with two attributes has one edge per subscription.
    let ctx = build_detector_context(&resolved, 0);
    let symbol = ctx
        .event_graph
        .events
        .iter()
        .find(|e| e.id == event_id)
        .expect("dependency event symbol");
    assert_eq!(symbol.publisher_routine_id, None);
    assert!(
        matches!(&symbol.publisher_ref, Some(PublisherRef::Dependency { target, .. })
            if target == &format!("{DEP_GUID}/Codeunit/60570::onafterpost/0")),
        "{:?}",
        symbol.publisher_ref
    );
    let both = resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "HandleBoth")
        .expect("HandleBoth");
    let mut both_edges: Vec<(&str, &str)> = ctx
        .event_graph
        .edges
        .iter()
        .filter(|e| e.subscriber_routine_id == both.id)
        .map(|e| (e.event_id.as_str(), e.resolution.as_str()))
        .collect();
    both_edges.sort();
    let before = format!("{DEP_GUID}/Codeunit/60570/event/onbeforepost");
    assert_eq!(
        both_edges,
        vec![
            (event_id.as_str(), "resolved"),
            (before.as_str(), "resolved")
        ],
        "each [EventSubscriber] of a routine is its own subscription"
    );
}

/// G7: a subscription that does not bind stays in the event graph and makes the
/// event's fan-out coverage partial. `HandleX(A: Integer)` matches both
/// `OnX` overloads, so it is ambiguous, not a proven subscriber.
#[test]
fn an_unbound_subscription_keeps_fanout_coverage_partial() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("app.json"), &app_json(""));
    write(
        &dir.path().join("src/Pub.al"),
        r#"codeunit 50580 "Ev Ws Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnX(A: Integer)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnX(A: Text)
    begin
    end;
}

codeunit 50581 "Ev Ws Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Ev Ws Pub", 'OnX', '', false, false)]
    local procedure HandleX(A: Integer)
    begin
    end;
}
"#,
    );
    let resolved =
        assemble_and_resolve_workspace_with_program_calls(dir.path()).expect("workspace resolves");
    let ctx = build_detector_context(&resolved, 0);
    let edges: Vec<&str> = ctx
        .event_graph
        .edges
        .iter()
        .map(|e| e.resolution.as_str())
        .collect();
    assert_eq!(
        edges,
        vec!["ambiguous"],
        "the subscription is kept, unbound"
    );

    let fanout = compute_fanout(&ctx.event_graph, &ctx.event_flow_indexes, &HashMap::new());
    let onx: Vec<_> = fanout.iter().filter(|f| f.event_name == "OnX").collect();
    assert!(!onx.is_empty(), "OnX is a published event: {fanout:?}");
    for f in onx {
        assert_eq!(f.direct_subscriber_count, 0, "ambiguous is not proven");
        assert_eq!(
            f.coverage.dispatch_edges, "partial",
            "an unbound subscription makes the dispatch partial"
        );
    }
}

/// A platform field event fires per field. `OnAfterValidateEvent` for "Alpha" and
/// for "Beta" are different events, so their subscribers are not co-subscribers:
/// d44 pairs the two "Alpha" subscribers only. Found on CDO, where two
/// subscribers of Sales Header's `OnAfterValidateEvent` filter different fields.
#[test]
fn d44_pairs_platform_field_event_subscribers_per_field() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("app.json"), &app_json(""));
    write(
        &dir.path().join("src/Doc.al"),
        r#"table 50590 "Ev Doc"
{
    fields
    {
        field(1; Alpha; Integer) { }
        field(2; Beta; Integer) { }
    }
    keys { key(PK; Alpha) { } }
}

table 50591 "Ev Log2"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50592 "Ev Field Subs"
{
    [EventSubscriber(ObjectType::Table, Database::"Ev Doc", 'OnAfterValidateEvent', 'Alpha', false, false)]
    local procedure AlphaOne(var Rec: Record "Ev Doc"; var xRec: Record "Ev Doc"; CurrFieldNo: Integer)
    var
        Log: Record "Ev Log2";
    begin
        Log.Insert();
    end;

    [EventSubscriber(ObjectType::Table, Database::"Ev Doc", 'OnAfterValidateEvent', 'Alpha', false, false)]
    local procedure AlphaTwo(var Rec: Record "Ev Doc"; var xRec: Record "Ev Doc"; CurrFieldNo: Integer)
    var
        Log: Record "Ev Log2";
    begin
        Log.Modify();
    end;

    [EventSubscriber(ObjectType::Table, Database::"Ev Doc", 'OnAfterValidateEvent', 'Beta', false, false)]
    local procedure BetaOne(var Rec: Record "Ev Doc"; var xRec: Record "Ev Doc"; CurrFieldNo: Integer)
    var
        Log: Record "Ev Log2";
    begin
        Log.Delete();
    end;
}
"#,
    );
    let resolved =
        assemble_and_resolve_workspace_with_program_calls(dir.path()).expect("workspace resolves");
    let d44: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d44-event-multi-subscriber-overlap")
        .collect();
    let run = run_detectors(&resolved, &d44);
    let ids: Vec<&str> = run.findings.iter().map(|f| f.id.as_str()).collect();
    let alpha = format!("d44/{WS_GUID}/Table/50590/event/onaftervalidateevent/alpha|");
    assert_eq!(ids.len(), 1, "one overlap, on Alpha only: {ids:?}");
    assert!(ids[0].starts_with(&alpha), "{ids:?}");
}
