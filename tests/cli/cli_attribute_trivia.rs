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
        dir.path(),
        None,
        Some(fires.iter().map(|s| s.to_string()).collect()),
        None,
        "issue-80",
        true,  // deterministic
        None,  // max_paths
        false, // cross-app, the CLI default (#66)
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
    let (_, report, _) =
        al_sem::program::resolve::full::build_program_with_coverage(dir.path()).expect("build");
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
    let m = assemble_and_resolve_workspace_program(dir.path(), MODEL_INSTANCE_ID_DEFAULT, false)
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
