//! #80 end to end: an `[EventSubscriber]` separated from its handler by a comment,
//! a pragma or an `#if` line still binds, so `alsem digest` sees the handler's
//! `Commit()` through the event dispatch from `FireN`.
//!
//! Each workspace is stated as source text in a tempdir (like
//! `cli_s6_consumers.rs`) and run through `run_digest_pipeline` with
//! `deterministic: true` (like `cli_b_digest_differential.rs`). No golden file.

use std::collections::BTreeMap;
use std::path::Path;

use al_sem::engine::l5::digest_cli::run_digest_pipeline;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Write `app.json` + `src/Trivia.al` into `ws`.
fn workspace(ws: &Path, al: &str) {
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaa8080-0000-0000-0000-000000000080","name":"Trivia80","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
    );
    write(&ws.join("src/Trivia.al"), al);
}

/// Digest `fires` in a workspace holding `al`; returns, per routine display name,
/// whether its effects include `COMMIT`.
fn commit_by_routine(al: &str, fires: &[&str]) -> BTreeMap<String, bool> {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let r = run_digest_pipeline(
        &al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs()),
        None,
        Some(fires.iter().map(|s| s.to_string()).collect()),
        None,
        "issue-80",
        true,  // deterministic
        None,  // max_paths
        false, // cross-app, the CLI default (#66)
        false, // no --tests
    )
    .expect("digest runs");
    let v: serde_json::Value = serde_json::from_str(&r.json_text).expect("json");
    let entries = v
        .pointer("/payload/entries")
        .and_then(|e| e.as_array())
        .unwrap_or_else(|| panic!("no payload.entries: {}", r.json_text));
    let mut out = BTreeMap::new();
    for e in entries {
        let name = e
            .pointer("/routine/display")
            .and_then(|d| d.as_str())
            .expect("routine.display")
            .to_string();
        let commit = e
            .get("effects")
            .and_then(|x| x.as_array())
            .map(|fx| {
                fx.iter()
                    .any(|f| f.get("type").and_then(|t| t.as_str()) == Some("COMMIT"))
            })
            .unwrap_or(false);
        out.insert(name, commit);
    }
    out
}

/// One `OnEN` + `FireN` per separator kind (1 line comment, 2 block comment,
/// 3 `#pragma`, 4 same-line comment, 5 `#if` around the attribute only, 6 the
/// attribute before an `#if` that wraps the handler), a no-trivia control (0),
/// and the target variant: `#if not X [EventSubscriber(…, #if X 'OnA' #else
/// 'OnB' #endif …)] #endif` binds to `OnB` only.
const MAIN: &str = r##"codeunit 50180 "Trivia Pub"
{
    procedure Fire1()
    begin
        OnE1();
    end;

    procedure Fire2()
    begin
        OnE2();
    end;

    procedure Fire3()
    begin
        OnE3();
    end;

    procedure Fire4()
    begin
        OnE4();
    end;

    procedure Fire5()
    begin
        OnE5();
    end;

    procedure Fire6()
    begin
        OnE6();
    end;

    procedure Fire0()
    begin
        OnE0();
    end;

    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE1()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE2()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE3()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE4()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE5()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE6()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnE0()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE1', '', false, false)]
    // line comment
    local procedure H1()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE2', '', false, false)]
    /* block comment */
    local procedure H2()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE3', '', false, false)]
#pragma warning disable AA0150
    local procedure H3()
    begin
        Commit();
    end;
#pragma warning restore AA0150

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE4', '', false, false)] // same-line comment
    local procedure H4()
    begin
        Commit();
    end;

#if not CLEAN99
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE5', '', false, false)]
#endif
    local procedure H5()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE6', '', false, false)]
#if X
    local procedure H6(I: Integer)
    begin
        Commit();
    end;
#else
    local procedure H6(T: Text)
    begin
        Commit();
    end;
#endif

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub", 'OnE0', '', false, false)]
    local procedure H0()
    begin
        Commit();
    end;

#if not X
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Pub",
#if X
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#endif
    local procedure HT()
    begin
        Commit();
    end;
}
"##;

/// The merge variant (A19's shape): `P` exists only under X and Y and reads
/// 'OnA' (no `Commit`); `H` is reached only by paths that read 'OnB'.
const MERGE: &str = r##"codeunit 50190 "Trivia Merge"
{
    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Trivia Merge",
#if X and Y
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#if X
#if Y
    local procedure P()
    begin
    end;
#endif
#endif
    local procedure H()
    begin
        Commit();
    end;
}
"##;

/// A12 (#80): every separator kind still binds the subscriber, so every `FireN`
/// reaches its handler's `Commit()`. The target variant binds `HT` to `OnB`
/// only, and the merge variant binds `H` to `OnB` only.
#[test]
fn digest_sees_a_subscriber_separated_by_trivia() {
    let fires = [
        "Fire0", "Fire1", "Fire2", "Fire3", "Fire4", "Fire5", "Fire6", "FireA", "FireB",
    ];
    let got = commit_by_routine(MAIN, &fires);
    let want: BTreeMap<String, bool> = fires
        .iter()
        .map(|f| (f.to_string(), *f != "FireA"))
        .collect();
    assert_eq!(got, want, "main workspace: routine -> has COMMIT");

    let got = commit_by_routine(MERGE, &["FireA", "FireB"]);
    let want: BTreeMap<String, bool> = [("FireA".to_string(), false), ("FireB".to_string(), true)]
        .into_iter()
        .collect();
    assert_eq!(got, want, "merge variant: routine -> has COMMIT");
}

// ---------------------------------------------------------------------------
// #103: one procedure declared in two `#if` arms with the same signature is one
// routine, and it subscribes to what EITHER arm subscribes to.
// ---------------------------------------------------------------------------

/// The issue's reproducer: `H` subscribes `OnA` under `XSYM` and `OnB` otherwise.
const IF_DUP: &str = r##"codeunit 50103 "Repro Pub"
{
    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;

#if XSYM
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnA', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnB', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
#endif
}
"##;

/// Both arms of `H` subscribe the SAME event.
const IF_DUP_SAME: &str = r##"codeunit 50103 "Repro Pub"
{
    procedure FireA()
    begin
        OnA();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

#if XSYM
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnA', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnA', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
#endif
}
"##;

/// The `ws3` shape: `OnA` is a plain procedure under `XSYM` and the publisher
/// otherwise; `H` subscribes to it unconditionally.
const IF_PUBNESS: &str = r##"codeunit 50103 "Repro Pub"
{
    procedure FireA()
    begin
        OnA();
    end;

#if XSYM
    local procedure OnA()
    begin
    end;
#else
    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;
#endif

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnA', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
}
"##;

/// The whole-object shape: ONE file declares the subscriber codeunit twice,
/// under a top-level `#if XSYM` / `#else`. Its `H` subscribes `OnA` in the
/// first declaration and `OnB` in the second.
const IF_DUP_OBJECT: &str = r##"codeunit 50103 "Repro Pub"
{
    procedure FireA()
    begin
        OnA();
    end;

    procedure FireB()
    begin
        OnB();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnA()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnB()
    begin
    end;
}

#if XSYM
codeunit 50104 "Repro Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnA', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
}
#else
codeunit 50104 "Repro Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Repro Pub", 'OnB', '', false, false)]
    local procedure H()
    begin
        Commit();
    end;
}
#endif
"##;

/// The program engine's call graph of a workspace holding `al`: every edge, and
/// the whole-program histogram.
fn program_edges(
    al: &str,
) -> (
    Vec<al_sem::program::resolve::full::ClassifiedEdge>,
    al_sem::program::resolve::edge::Histogram,
) {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let inputs =
        al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs());
    let (_, report, _) =
        al_sem::program::resolve::full::build_program_with_coverage(&inputs).expect("build");
    (report.edges, report.histogram)
}

/// The event-flow edges of `publisher_lc`, each as its outcome and its routine
/// targets (`name_lc`, source evidence).
fn event_flow(
    edges: &[al_sem::program::resolve::full::ClassifiedEdge],
    publisher_lc: &str,
) -> Vec<(String, Vec<(String, bool)>)> {
    use al_sem::program::resolve::edge::{EdgeKind, Evidence, RouteTarget, classify_obligation};
    edges
        .iter()
        .filter(|e| e.edge.kind == EdgeKind::EventFlow && &*e.edge.from.name_lc == publisher_lc)
        .map(|e| {
            let targets = e
                .edge
                .routes
                .iter()
                .filter_map(|r| match &r.target {
                    RouteTarget::Routine(t) => Some((
                        t.name_lc.to_string(),
                        matches!(r.evidence, Evidence::Source),
                    )),
                    _ => None,
                })
                .collect();
            (format!("{:?}", classify_obligation(&e.edge)), targets)
        })
        .collect()
}

/// The model's event graph of a workspace holding `al`: one
/// `(event, subscriber, resolution)` per edge, sorted, and the census.
fn model_event_edges(
    al: &str,
) -> (
    Vec<(String, String, String)>,
    al_sem::program::model::events::EventCensus,
) {
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let inputs =
        al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs());
    let m = assemble_and_resolve_workspace_program(&inputs, MODEL_INSTANCE_ID_DEFAULT, false)
        .expect("model");
    let name_of = |id: &str| {
        m.workspace
            .routines
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no model routine {id}"))
            .name
            .clone()
    };
    let mut out: Vec<(String, String, String)> = m
        .events
        .graph
        .edges
        .iter()
        .map(|e| {
            (
                e.event_id.rsplit('/').next().unwrap().to_string(),
                name_of(&e.subscriber_routine_id),
                e.resolution.clone(),
            )
        })
        .collect();
    out.sort();
    (out, m.events.census.clone())
}

/// A2: `FireB` raises `OnB`, which only `H`'s second arm subscribes to; the
/// digest of `FireB` must reach `H`'s `Commit()`, as `FireA`'s does.
#[test]
fn digest_sees_both_arms_of_an_if_duplicated_subscriber() {
    let got = commit_by_routine(IF_DUP, &["FireA", "FireB"]);
    let want: BTreeMap<String, bool> = [("FireA".to_string(), true), ("FireB".to_string(), true)]
        .into_iter()
        .collect();
    assert_eq!(got, want, "routine -> has COMMIT");
}

/// The whole-object `#if` duplicate (#103 final panel): the two declarations
/// of `H` share their file, so `FireB` reaches the second declaration's
/// `Commit()` too.
#[test]
fn digest_sees_both_declarations_of_an_if_duplicated_object() {
    let got = commit_by_routine(IF_DUP_OBJECT, &["FireA", "FireB"]);
    let want: BTreeMap<String, bool> = [("FireA".to_string(), true), ("FireB".to_string(), true)]
        .into_iter()
        .collect();
    assert_eq!(got, want, "routine -> has COMMIT");
}

/// A2b: the program engine's event-flow edge of `OnB` resolves to `H` from
/// source; on master it was classed `HonestEmpty` ("provably no callee").
#[test]
fn the_second_arms_event_flow_edge_resolves_from_source() {
    let (edges, histogram) = program_edges(IF_DUP);
    for event in ["ona", "onb"] {
        assert_eq!(
            event_flow(&edges, event),
            [("Resolved".to_string(), vec![("h".to_string(), true)])],
            "{event}'s event-flow edge"
        );
    }
    assert_eq!(histogram.honest_empty, 0, "{histogram:?}");
}

/// A3: a publisher declared in two `#if` arms, only one of which carries
/// `[IntegrationEvent]`, is a publisher: its event-flow edge reaches `H`, and
/// the model edge is `resolved`, not the `maybe` of an orphaned subscription.
#[test]
fn a_publisher_in_one_if_arm_is_a_publisher() {
    let (edges, _) = program_edges(IF_PUBNESS);
    assert_eq!(
        event_flow(&edges, "ona"),
        [("Resolved".to_string(), vec![("h".to_string(), true)])]
    );
    let (model, _) = model_event_edges(IF_PUBNESS);
    assert_eq!(
        model,
        [("ona".to_string(), "H".to_string(), "resolved".to_string())]
    );
}

/// A5: the two rows of an `#if`-duplicated subscriber are one routine id, so
/// the model has ONE edge per subscription, not one per row and subscription.
/// The census counts the routine once too: with both arms subscribing `OnA`,
/// `bound_model` is 1 (2 on master), while `model_subscribers` stays per row.
#[test]
fn the_model_has_one_edge_per_subscription_of_an_if_duplicate() {
    let resolved = |event: &str| (event.to_string(), "H".to_string(), "resolved".to_string());
    let (edges, _) = model_event_edges(IF_DUP);
    assert_eq!(edges, [resolved("ona"), resolved("onb")]);

    let (edges, census) = model_event_edges(IF_DUP_SAME);
    assert_eq!(edges, [resolved("ona")]);
    assert_eq!(
        (
            census.model_subscribers,
            census.unmapped_subscribers,
            census.bound_model
        ),
        (2, 0, 1),
        "{census:?}"
    );
}

// ---------------------------------------------------------------------------
// #104: one routine can carry several READINGS of one attribute (one per build
// path through an `#if` group inside the attribute's arguments, #80). Every
// consumer must read them all, merged like #103's `#if` arms.
// ---------------------------------------------------------------------------

/// The publisher side: `OnFoo`'s `IncludeSender` is `{first}` under `X` and
/// `{second}` otherwise; `H` captures the Sender. The `#pragma` group after the
/// attribute gives the routine both build paths.
fn sender_readings(first: &str, second: &str) -> String {
    format!(
        r##"codeunit 50100 "Pub"
{{
    [IntegrationEvent(
#if X
    {first},
#else
    {second},
#endif
    false)]
#if X
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
    procedure OnFoo()
    begin
    end;

    procedure Fire()
    begin
        OnFoo();
    end;
}}

codeunit 50101 "Sub"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnFoo', '', false, false)]
    local procedure H(var Sender: Codeunit "Pub")
    begin
        Commit();
    end;
}}
"##
    )
}

/// Plain publishers `OnFoo` and `OnBar`, plus a subscriber `G` whose ONE
/// `[EventSubscriber]` has the arguments `{args}` (an `#if Y` group inside).
fn subscriber_readings(args: &str) -> String {
    format!(
        r##"codeunit 50100 "Pub"
{{
    [IntegrationEvent(false, false)]
    procedure OnFoo()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnBar()
    begin
    end;

    procedure Fire()
    begin
        OnFoo();
        OnBar();
    end;
}}

codeunit 50101 "Sub"
{{
    [EventSubscriber({args})]
#if Y
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
    local procedure G()
    begin
        Commit();
    end;
}}
"##
    )
}

/// The `include_sender` of the program node named `name_lc`.
fn include_sender_of(al: &str, name_lc: &str) -> Option<bool> {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let inputs =
        al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs());
    let (ctx, _, _) =
        al_sem::program::resolve::full::build_program_with_coverage(&inputs).expect("build");
    ctx.graph()
        .routines
        .iter()
        .find(|r| &*r.id.name_lc == name_lc)
        .unwrap_or_else(|| panic!("no routine {name_lc}"))
        .include_sender
}

/// The event-flow routes of `publisher_lc`: one `(target name_lc, conditions)`
/// per routine route, over every event-flow edge of that publisher.
fn event_flow_routes(
    edges: &[al_sem::program::resolve::full::ClassifiedEdge],
    publisher_lc: &str,
) -> Vec<(String, Vec<al_sem::program::resolve::edge::Condition>)> {
    use al_sem::program::resolve::edge::{EdgeKind, RouteTarget};
    edges
        .iter()
        .filter(|e| e.edge.kind == EdgeKind::EventFlow && &*e.edge.from.name_lc == publisher_lc)
        .flat_map(|e| e.edge.routes.iter())
        .filter_map(|r| match &r.target {
            RouteTarget::Routine(t) => Some((t.name_lc.to_string(), r.conditions.clone())),
            _ => None,
        })
        .collect()
}

/// The model event graph's edges as `(event id after "/event/", subscriber,
/// resolution)`, sorted. Unlike `model_event_edges` it keeps an element suffix.
fn model_event_ids(al: &str) -> Vec<(String, String, String)> {
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
    use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), al);
    let inputs =
        al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs());
    let m = assemble_and_resolve_workspace_program(&inputs, MODEL_INSTANCE_ID_DEFAULT, false)
        .expect("model");
    let name_of = |id: &str| {
        m.workspace
            .routines
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no model routine {id}"))
            .name
            .clone()
    };
    let mut out: Vec<(String, String, String)> = m
        .events
        .graph
        .edges
        .iter()
        .map(|e| {
            (
                e.event_id
                    .split_once("/event/")
                    .map_or_else(|| e.event_id.clone(), |(_, t)| t.to_string()),
                name_of(&e.subscriber_routine_id),
                e.resolution.clone(),
            )
        })
        .collect();
    out.sort();
    out
}

/// A1 (#104): `IncludeSender` is `true` only in the SECOND reading. Some build
/// passes the Sender, so `H(var Sender)` is valid there: `OnFoo` must be
/// `include_sender == Some(true)`, its event flow must reach `h` from source,
/// and the model edge must be `resolved` (on master: `Some(false)`, `H`
/// orphaned).
#[test]
fn a1_sender_in_the_second_reading_binds_the_sender_subscriber() {
    let al = sender_readings("false", "true");
    assert_eq!(
        include_sender_of(&al, "onfoo"),
        Some(true),
        "OnFoo include_sender"
    );
    let (edges, _) = program_edges(&al);
    assert_eq!(
        event_flow(&edges, "onfoo"),
        [("Resolved".to_string(), vec![("h".to_string(), true)])],
        "OnFoo's event flow"
    );
    let (model, _) = model_event_edges(&al);
    assert_eq!(
        model,
        [("onfoo".to_string(), "H".to_string(), "resolved".to_string())]
    );
}

/// A2 (#104): the same with the Sender reading FIRST. CONTROL: holds on master
/// too (the first reading already wins there); it pins the other orientation.
#[test]
fn a2_control_sender_in_the_first_reading_binds_the_sender_subscriber() {
    let al = sender_readings("true", "false");
    assert_eq!(
        include_sender_of(&al, "onfoo"),
        Some(true),
        "OnFoo include_sender"
    );
    let (edges, _) = program_edges(&al);
    assert_eq!(
        event_flow(&edges, "onfoo"),
        [("Resolved".to_string(), vec![("h".to_string(), true)])],
        "OnFoo's event flow"
    );
    let (model, _) = model_event_edges(&al);
    assert_eq!(
        model,
        [("onfoo".to_string(), "H".to_string(), "resolved".to_string())]
    );
}

/// The A3/A3b assertions: `G` is wired to `OnFoo` ONCE (one route, with no
/// skip condition: a flag set in only some readings does not hold for the
/// edge), one model edge, counted once in `bound_model`.
fn assert_one_subscription_of_onfoo(al: &str) {
    let (edges, _) = program_edges(al);
    assert_eq!(
        event_flow_routes(&edges, "onfoo"),
        [("g".to_string(), vec![])],
        "OnFoo's event-flow routes (target, conditions)"
    );
    let (model, census) = model_event_edges(al);
    assert_eq!(
        model,
        [("onfoo".to_string(), "G".to_string(), "resolved".to_string())]
    );
    assert_eq!(census.bound_model, 1, "{census:?}");
}

/// A3 (#104): `SkipOnMissingLicense` is `true` under `Y` only. Both readings
/// name the same event, so they are ONE subscription (on master: two routes to
/// `g`, two model edges, `bound_model` 2).
#[test]
fn a3_two_readings_of_one_subscriber_are_one_subscription() {
    let al = subscriber_readings(
        "ObjectType::Codeunit, Codeunit::\"Pub\", 'OnFoo', '',\n#if Y\n    true,\n#else\n    false,\n#endif\n    false",
    );
    assert_one_subscription_of_onfoo(&al);
}

/// A3b (#104): one reading names the publisher by name, the other by number.
#[test]
fn a3b_readings_naming_the_publisher_by_name_and_by_number_are_one_subscription() {
    let al = subscriber_readings(
        "ObjectType::Codeunit,\n#if Y\n    Codeunit::\"Pub\",\n#else\n    50100,\n#endif\n    'OnFoo', '', false, false",
    );
    assert_one_subscription_of_onfoo(&al);
}

/// A3c (#104): readings naming two DIFFERENT events stay two subscriptions:
/// both `OnFoo`'s and `OnBar`'s flows reach `g`. CONTROL: expected to hold on
/// master as well.
#[test]
fn a3c_control_readings_naming_two_events_keep_both_subscriptions() {
    let al = subscriber_readings(
        "ObjectType::Codeunit, Codeunit::\"Pub\",\n#if Y\n    'OnFoo',\n#else\n    'OnBar',\n#endif\n    '', false, false",
    );
    let (edges, _) = program_edges(&al);
    for event in ["onfoo", "onbar"] {
        assert_eq!(
            event_flow_routes(&edges, event),
            [("g".to_string(), vec![])],
            "{event}'s event-flow routes"
        );
    }
}

/// A table `T` and a subscriber `H` of its `OnAfterValidateEvent` whose element
/// is 'a' under `Y` and 'b' otherwise. `publisher` is spliced into the table.
fn table_element_readings(publisher: &str) -> String {
    format!(
        r##"table 50100 "T"
{{
    fields
    {{
        field(1; a; Integer) {{ }}
        field(2; b; Integer) {{ }}
    }}
{publisher}
}}

codeunit 50101 "Sub"
{{
    [EventSubscriber(ObjectType::Table, Database::"T", 'OnAfterValidateEvent',
#if Y
    'a',
#else
    'b',
#endif
    false, false)]
#if Y
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
    local procedure H(var Rec: Record "T"; var xRec: Record "T"; CurrFieldNo: Integer)
    begin
        Commit();
    end;
}}
"##
    )
}

/// A3e (#104), platform half. CONTROL: a PLATFORM field event fires per
/// element, so the two readings are two subscriptions, one model edge each
/// (ids `/a` and `/b`). Expected to hold on master; the fix must keep it.
#[test]
fn a3e_control_platform_table_event_readings_keep_one_subscription_per_element() {
    let al = table_element_readings("");
    let edge = |id: &str| (id.to_string(), "H".to_string(), "resolved".to_string());
    assert_eq!(
        model_event_ids(&al),
        [
            edge("onaftervalidateevent/a"),
            edge("onaftervalidateevent/b")
        ]
    );
}

/// A3e (#104), codeunit half: a codeunit event never uses the element, so two
/// readings differing only in the element are ONE subscription: one route, one
/// model edge (on master: two of each).
#[test]
fn a3e_codeunit_event_readings_differing_only_in_element_are_one_subscription() {
    let al = subscriber_readings(
        "ObjectType::Codeunit, Codeunit::\"Pub\", 'OnFoo',\n#if Y\n    'a',\n#else\n    'b',\n#endif\n    false, false",
    );
    let (edges, _) = program_edges(&al);
    assert_eq!(
        event_flow_routes(&edges, "onfoo"),
        [("g".to_string(), vec![])],
        "OnFoo's event-flow routes"
    );
    assert_eq!(
        model_event_ids(&al),
        [("onfoo".to_string(), "G".to_string(), "resolved".to_string())]
    );
}

/// A3e (#104), real-publisher half (S5-1): the table declares a REAL
/// `[IntegrationEvent] OnAfterValidateEvent` whose arity admits `H`, so the
/// injector adds no platform publisher and `H` binds to the source routine
/// (not `Platform`). The model does not key that event by element: ONE
/// `Bound` subscription, ONE `resolved` model edge (on master: two).
#[test]
fn a3e_real_table_publisher_readings_differing_only_in_element_are_one_subscription() {
    use al_sem::program::resolve::event::PublisherKind;
    use al_sem::program::resolve::index::{SubscriberIndex, SubscriptionOutcome};
    let al = table_element_readings(
        r#"
    [IntegrationEvent(false, false)]
    procedure OnAfterValidateEvent(var Rec: Record "T"; var xRec: Record "T"; CurrFieldNo: Integer)
    begin
    end;
"#,
    );
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), &al);
    let inputs =
        al_sem::program::inputs::WorkspaceInputs::read(dir.path(), &crate::fingerprint_needs());
    let (ctx, _, _) =
        al_sem::program::resolve::full::build_program_with_coverage(&inputs).expect("build");
    let graph = ctx.graph();
    let index = SubscriberIndex::build(graph);
    let bound_kinds: Vec<Option<PublisherKind>> = index
        .subscriptions()
        .iter()
        .filter(|s| &*s.subscriber.name_lc == "h")
        .map(|s| match &s.outcome {
            SubscriptionOutcome::Bound(rid) => graph
                .routines
                .iter()
                .find(|r| &r.id == rid)
                .and_then(|r| r.publisher_kind),
            _ => panic!("H's subscription is not Bound"),
        })
        .collect();
    assert_eq!(
        bound_kinds,
        [Some(PublisherKind::Integration)],
        "H's subscriptions, by the bound publisher's kind"
    );
    assert_eq!(
        model_event_ids(&al),
        [(
            "onaftervalidateevent".to_string(),
            "H".to_string(),
            "resolved".to_string()
        )]
    );
}
