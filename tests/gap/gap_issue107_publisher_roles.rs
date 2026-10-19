//! Issue #107 acceptance tests: an event publisher's parameter roles compose
//! what its subscribers do to that parameter, and every caller that raises the
//! event sees it through the existing call-site composition.
//!
//! Spec: `docs/superpowers/specs/2026-10-19-issue-107-publisher-roles-subscribers-design.md`,
//! "Acceptance matrix" (unit rows 1..12b) and the ledger's "Plan notes".
//!
//! Every test states its AL in code and reads the PUBLISHED roles through the
//! production paths: the detector context `alsem analyze` builds
//! (`build_detector_context`, `parameter_roles_by_routine`), the summary solver
//! itself (`compute_summaries_v2`) where a precondition can only be stated as a
//! hand-built graph edge or a bodyless routine, and the registered detectors.
//! Rows 5c (edge metadata), 13 (`event_publisher_param`), the forced roles cap
//! and the dependency-publisher rows live in the lib (they need crate-private
//! symbols).

use std::collections::HashMap;

use al_sem::engine::l4::combined_graph::{CombinedGraph, build_combined_graph};
use al_sem::engine::l4::scc::{SccInputGraph, tarjan_scc};
use al_sem::engine::l4::summary::{FieldList, RecordRoleSummary};
use al_sem::engine::l4::summary_runner::{FieldIndex, compute_summaries_v2};
use al_sem::engine::l5::detector_context::build_detector_context;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::registry::{run_detectors, substrate};
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;
use al_sem::program::model::workspace::{Model, ModelRoutine};

const APP_GUID: &str = "11111111-0000-0000-0000-000000107107";

const TABLE: &str = r#"
table 50100 Cust
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// The presence facts, in `RecordRoleSummary` order.
const PRESENCE: [&str; 8] = [
    "persistsCurrentRecord",
    "setBasedDbWrites",
    "validatesParam",
    "copiesIntoParam",
    "resetsFiltersOnParam",
    "loadsFromDbParam",
    "initialisesParam",
    "mutatesParam",
];

/// The five path-sensitive facts (the walker's output).
const PATH: [&str; 5] = [
    "requiresLoadedAtEntry",
    "requiredLoadedFieldsAtEntry",
    "mutatesBeforeLoad",
    "dirtyAtExit",
    "currentLoadedFieldsAtExit",
];

fn model_of(files: &[(&str, String)]) -> Model {
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(n, s)| (format!("src/{n}.al"), s.clone()))
        .collect();
    assemble_and_resolve_inline_program_default(&files, APP_GUID)
}

/// The table plus `code` as one codeunit file each (the caller writes whole
/// objects).
fn model(objects: &[&str]) -> Model {
    let mut files: Vec<(&str, String)> = vec![("Tab", TABLE.to_string())];
    let names = ["O0", "O1", "O2", "O3", "O4", "O5", "O6", "O7"];
    for (i, o) in objects.iter().enumerate() {
        files.push((names[i], o.to_string()));
    }
    model_of(&files)
}

/// The one routine called `name` (panics on 0 or several: the precondition).
fn rid(m: &Model, name: &str) -> String {
    let hits: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(hits.len(), 1, "precondition: one routine {name}");
    hits[0].id.clone()
}

/// Routine name -> roles, through the detector context `alsem analyze` builds.
fn ctx_roles(m: &Model) -> HashMap<String, Vec<RecordRoleSummary>> {
    let ctx = build_detector_context(m, substrate::ALL);
    let mut out = HashMap::new();
    for r in &m.workspace.routines {
        if let Some(v) = ctx.parameter_roles_by_routine.get(&r.id) {
            out.insert(r.id.clone(), v.clone());
        }
    }
    out
}

/// The summary solver over `routines` and `graph` (the inputs the detector
/// context uses), so a test can hand-state an edge or a bodyless routine.
fn solve(
    m: &Model,
    routines: &[ModelRoutine],
    graph: &CombinedGraph,
) -> HashMap<String, Vec<RecordRoleSummary>> {
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &adjacency,
    });
    let mut fields: FieldIndex = HashMap::new();
    for t in &m.workspace.tables {
        for f in &t.fields {
            fields
                .entry((t.id.clone(), f.name.to_lowercase()))
                .or_insert_with(|| f.id.clone());
        }
    }
    let (map, _) = compute_summaries_v2(routines, graph, &scc, &m.calls.upgraded_bindings, &fields);
    map.into_iter()
        .map(|(k, v)| (k, v.parameter_roles))
        .collect()
}

fn graph_of(m: &Model) -> CombinedGraph {
    build_combined_graph(&m.workspace, &m.calls, &m.events.graph)
}

/// The role of parameter `idx` of the routine called `name`.
fn role<'a>(
    m: &Model,
    roles: &'a HashMap<String, Vec<RecordRoleSummary>>,
    name: &str,
    idx: u32,
) -> &'a RecordRoleSummary {
    roles
        .get(&rid(m, name))
        .and_then(|v| v.iter().find(|r| r.parameter_index == idx))
        .unwrap_or_else(|| panic!("precondition: {name} has a role for parameter {idx}"))
}

/// One fact of a role, as its Debug text (`Yes`/`No`/`Unknown`, or a field
/// list: `Unknown`/`Full`/`Known([..])`).
fn fact(r: &RecordRoleSummary, f: &str) -> String {
    match f {
        "persistsCurrentRecord" => format!("{:?}", r.persists_current_record),
        "setBasedDbWrites" => format!("{:?}", r.set_based_db_writes),
        "validatesParam" => format!("{:?}", r.validates_param),
        "copiesIntoParam" => format!("{:?}", r.copies_into_param),
        "resetsFiltersOnParam" => format!("{:?}", r.resets_filters_on_param),
        "loadsFromDbParam" => format!("{:?}", r.loads_from_db_param),
        "initialisesParam" => format!("{:?}", r.initialises_param),
        "mutatesParam" => format!("{:?}", r.mutates_param),
        "requiresLoadedAtEntry" => format!("{:?}", r.requires_loaded_at_entry),
        "requiredLoadedFieldsAtEntry" => format!("{:?}", r.required_loaded_fields_at_entry),
        "mutatesBeforeLoad" => format!("{:?}", r.mutates_before_load),
        "dirtyAtExit" => format!("{:?}", r.dirty_at_exit),
        "currentLoadedFieldsAtExit" => format!("{:?}", r.current_loaded_fields_at_exit),
        other => panic!("unknown fact {other}"),
    }
}

/// Record every `(fact, want)` that `r` does not have.
fn expect(failures: &mut Vec<String>, label: &str, r: &RecordRoleSummary, want: &[(&str, &str)]) {
    for (f, w) in want {
        let got = fact(r, f);
        if &got != w {
            failures.push(format!("{label}.{f}: want {w}, got {got}"));
        }
    }
}

/// Record every fact in `fields` of `r` that is not `want`.
fn expect_all(
    failures: &mut Vec<String>,
    label: &str,
    r: &RecordRoleSummary,
    fields: &[&str],
    want: &str,
) {
    for f in fields {
        let got = fact(r, f);
        if got != want {
            failures.push(format!("{label}.{f}: want {want}, got {got}"));
        }
    }
}

fn done(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The findings of one registered detector, as the first word of each
/// `root_cause` (the blamed routine), sorted.
fn blamed(m: &Model, detector: &str) -> Vec<String> {
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == detector)
        .collect();
    assert_eq!(selected.len(), 1, "{detector} must be registered once");
    let mut out: Vec<String> = run_detectors(m, &selected)
        .findings
        .iter()
        .map(|f| f.root_cause.split(' ').next().unwrap_or("").to_string())
        .collect();
    out.sort();
    out
}

/// What a DIRECT call to a `Rec.Modify()` helper gives its caller (probe a'):
/// the oracle for "the publisher reflects its `Modify` subscriber".
const MODIFY_ORACLE: [(&str, &str); 5] = [
    ("persistsCurrentRecord", "Yes"),
    ("mutatesParam", "Yes"),
    ("requiresLoadedAtEntry", "Yes"),
    ("mutatesBeforeLoad", "Yes"),
    ("dirtyAtExit", "No"),
];

// ---------------------------------------------------------------------------
// Acceptance item 1 + rows 1, 2: a Modify / a Validate subscriber.
// ---------------------------------------------------------------------------

const PUB_1_2: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnRecModify(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnRecValidate(var Rec: Record Cust)
    begin
    end;

    procedure ForwardModify(var Rec: Record Cust)
    begin
        OnRecModify(Rec);
    end;

    procedure ForwardValidate(var Rec: Record Cust)
    begin
        OnRecValidate(Rec);
    end;

    procedure ControlDirect(var Rec: Record Cust)
    begin
        DoModify(Rec);
    end;

    procedure DoModify(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;

const SUB_1_2: &str = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnRecModify', '', false, false)]
    local procedure HandleModify(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnRecValidate', '', false, false)]
    local procedure HandleValidate(var Rec: Record Cust)
    begin
        Rec.Validate(Name, 'x');
    end;
}
"#;

/// Row 1 (and acceptance item 1, workspace): one `Modify` subscriber. The
/// publisher's role for `Rec` equals what a direct call to the subscriber gives
/// its caller (`ControlDirect`, probe a'), and `ForwardModify` inherits it.
#[test]
fn row1_modify_subscriber_equals_a_direct_call() {
    let m = model(&[PUB_1_2, SUB_1_2]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    // The oracle itself, so a change to the call-site rules shows here first.
    expect(
        &mut f,
        "ControlDirect",
        role(&m, &roles, "ControlDirect", 0),
        &MODIFY_ORACLE,
    );
    for name in ["OnRecModify", "ForwardModify"] {
        expect(&mut f, name, role(&m, &roles, name, 0), &MODIFY_ORACLE);
    }
    done(f);
}

/// Row 2: one `Validate` subscriber -> the publisher is `dirtyAtExit yes`,
/// `validatesParam yes`; `ForwardValidate` inherits `dirtyAtExit yes`.
#[test]
fn row2_validate_subscriber_dirties_the_publisher() {
    let m = model(&[PUB_1_2, SUB_1_2]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnRecValidate",
        role(&m, &roles, "OnRecValidate", 0),
        &[
            ("validatesParam", "Yes"),
            ("mutatesParam", "Yes"),
            ("dirtyAtExit", "Yes"),
        ],
    );
    expect(
        &mut f,
        "ForwardValidate",
        role(&m, &roles, "ForwardValidate", 0),
        &[("validatesParam", "Yes"), ("dirtyAtExit", "Yes")],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Row 3: two subscribers, every order persists.
// ---------------------------------------------------------------------------

/// Row 3: subscribers `A = Validate; Modify` and `B = Modify`. Every order ends
/// persisted, so `dirtyAtExit no` (the round-1 closure gave `yes`), while the
/// MAY facts join (`validatesParam yes`, `persistsCurrentRecord yes`).
#[test]
fn row3_two_subscribers_every_order_persists() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnTwo(var Rec: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnTwo', '', false, false)]
    local procedure HandleA(var Rec: Record Cust)
    begin
        Rec.Validate(Name, 'x');
        Rec.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnTwo', '', false, false)]
    local procedure HandleB(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnTwo",
        role(&m, &roles, "OnTwo", 0),
        &[
            ("persistsCurrentRecord", "Yes"),
            ("validatesParam", "Yes"),
            ("mutatesParam", "Yes"),
            ("dirtyAtExit", "No"),
        ],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Rows 4, 4b: unconditional loaders.
// ---------------------------------------------------------------------------

/// Row 4: two unconditional `Get` subscribers -> the publisher
/// `loadsFromDbParam yes`, and the walker's `loaded` stays `Yes` after the
/// dispatch: a caller `F(var R) { OnLoad2(R); R.Modify() }` does not require a
/// loaded record. Adding a no-op subscriber (`OnLoad3`) changes no role fact.
#[test]
fn row4_two_unconditional_loaders() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnLoad2(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnLoad3(var Rec: Record Cust)
    begin
    end;

    procedure F2(var R: Record Cust)
    begin
        OnLoad2(R);
        R.Modify();
    end;

    procedure F3(var R: Record Cust)
    begin
        OnLoad3(R);
        R.Modify();
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLoad2', '', false, false)]
    local procedure HandleLoad2A(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLoad2', '', false, false)]
    local procedure HandleLoad2B(var Rec: Record Cust)
    begin
        Rec.Get('B');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLoad3', '', false, false)]
    local procedure HandleLoad3A(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLoad3', '', false, false)]
    local procedure HandleLoad3B(var Rec: Record Cust)
    begin
        Rec.Get('B');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnLoad3', '', false, false)]
    local procedure HandleLoad3NoOp(var Rec: Record Cust)
    begin
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for p in ["OnLoad2", "OnLoad3"] {
        expect(
            &mut f,
            p,
            role(&m, &roles, p, 0),
            &[("loadsFromDbParam", "Yes")],
        );
    }
    for c in ["F2", "F3"] {
        expect(
            &mut f,
            c,
            role(&m, &roles, c, 0),
            &[("requiresLoadedAtEntry", "No")],
        );
    }
    let (r2, r3) = (
        role(&m, &roles, "OnLoad2", 0),
        role(&m, &roles, "OnLoad3", 0),
    );
    for name in PRESENCE.iter().chain(PATH.iter()) {
        if fact(r2, name) != fact(r3, name) {
            f.push(format!(
                "a no-op subscriber moved {name}: OnLoad2 {} vs OnLoad3 {}",
                fact(r2, name),
                fact(r3, name)
            ));
        }
    }
    done(f);
}

/// Row 4b (mixed `U`/`O`): an unconditional `Get` subscriber plus a MANUAL
/// no-op subscriber. Every run executes the `Get`, so the incoming unloaded
/// state is never an outcome: the publisher `loadsFromDbParam yes`, and a
/// caller `F(var R) { OnMixed(R); R.Modify() }` gets `requiresLoadedAtEntry no`.
#[test]
fn row4b_unconditional_loader_plus_manual_noop() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnMixed(var Rec: Record Cust)
    begin
    end;

    procedure F(var R: Record Cust)
    begin
        OnMixed(R);
        R.Modify();
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnMixed', '', false, false)]
    local procedure HandleMixedGet(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;
    let manual = r#"
codeunit 50102 ManualSub
{
    EventSubscriberInstance = Manual;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnMixed', '', false, false)]
    local procedure HandleMixedNoOp(var Rec: Record Cust)
    begin
    end;
}
"#;
    let m = model(&[pubs, subs, manual]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnMixed",
        role(&m, &roles, "OnMixed", 0),
        &[("loadsFromDbParam", "Yes")],
    );
    expect(
        &mut f,
        "F",
        role(&m, &roles, "F", 0),
        &[("requiresLoadedAtEntry", "No")],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Rows 5, 5b, 5c, 5d': optional subscribers mask the load trio.
// ---------------------------------------------------------------------------

/// Row 5: a MANUAL (`EventSubscriberInstance = Manual`) `Get` subscriber is
/// masked (`loadsFromDbParam no`, `optional_load`), so a caller
/// `V(var R) { R.Validate(..); OnOptLoad(R) }` gains no d39 load point (the
/// load-point reader is `puts_in_loaded_state` on the callee role).
/// Control in the same fixture: an UNCONDITIONAL `Get` subscriber makes its
/// publisher `loadsFromDbParam yes` (this part is red until the dispatch is
/// composed; the mask part guards the `optional_load` rule afterwards).
#[test]
fn row5_manual_loader_is_masked() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnOptLoad(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnUncondLoad(var Rec: Record Cust)
    begin
    end;

    procedure V(var R: Record Cust)
    begin
        R.Validate(Name, 'x');
        OnOptLoad(R);
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnUncondLoad', '', false, false)]
    local procedure HandleUncondLoad(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;
    let manual = r#"
codeunit 50102 ManualSub
{
    EventSubscriberInstance = Manual;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOptLoad', '', false, false)]
    local procedure HandleOptLoad(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;
    let m = model(&[pubs, subs, manual]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnUncondLoad (control)",
        role(&m, &roles, "OnUncondLoad", 0),
        &[("loadsFromDbParam", "Yes")],
    );
    let opt = role(&m, &roles, "OnOptLoad", 0);
    expect(&mut f, "OnOptLoad", opt, &[("loadsFromDbParam", "No")]);
    if opt.puts_in_loaded_state() {
        f.push("OnOptLoad is a load point (puts_in_loaded_state)".to_string());
    }
    expect(
        &mut f,
        "V",
        role(&m, &roles, "V", 0),
        &[("dirtyAtExit", "Yes"), ("loadsFromDbParam", "No")],
    );
    done(f);
}

/// Row 5b (through the CALLER): `F(var R) { R.Get('A'); OnOptionalGet(R);
/// R.Modify() }` with a manual `Get; Modify` subscriber. The masked trio is the
/// identity in the caller's walker, so the earlier `Loaded::Yes` survives the
/// dispatch: `F`'s `requiresLoadedAtEntry no` (with `unknown` it would be
/// `yes`), and its caller `CallerC() { F(X) }` with an unloaded `X` gets no d40
/// finding. The subscriber's `Modify` is a visible MAY fact: the publisher
/// `persistsCurrentRecord yes` (red until the dispatch is composed).
#[test]
fn row5b_manual_loader_does_not_destroy_the_callers_load() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnOptionalGet(var Rec: Record Cust)
    begin
    end;

    procedure F(var R: Record Cust)
    begin
        R.Get('A');
        OnOptionalGet(R);
        R.Modify();
    end;

    procedure CallerC()
    var
        X: Record Cust;
    begin
        F(X);
    end;
}
"#;
    let manual = r#"
codeunit 50102 ManualSub
{
    EventSubscriberInstance = Manual;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOptionalGet', '', false, false)]
    local procedure HandleOptionalGet(var Rec: Record Cust)
    begin
        Rec.Get('A');
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, manual]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnOptionalGet",
        role(&m, &roles, "OnOptionalGet", 0),
        &[("persistsCurrentRecord", "Yes"), ("loadsFromDbParam", "No")],
    );
    expect(
        &mut f,
        "F",
        role(&m, &roles, "F", 0),
        &[("requiresLoadedAtEntry", "No")],
    );
    let d40 = blamed(&m, "d40-transitive-load-missing");
    if d40.iter().any(|b| b == "CallerC") {
        f.push(format!("d40 blames CallerC: {d40:?}"));
    }
    done(f);
}

/// Row 5c (behaviour half; the `Subscription.conditional_any` / edge
/// `conditional` half is a lib test): a subscriber with
/// `SkipOnMissingLicense = true` in one `#if` reading and `false` in the other
/// is optional (`conditional_any`), so its `Get` is masked: the publisher's
/// trio is `no`. Its `Modify` is a visible MAY fact (`persistsCurrentRecord
/// yes`). Control: the same subscriber without the `#if` gives
/// `loadsFromDbParam yes`.
#[test]
fn row5c_mixed_readings_mask_the_load() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnMixRead(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnPlainRead(var Rec: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
#if CLEAN
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnMixRead', '', true, false)]
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnMixRead', '', false, false)]
#endif
    local procedure HandleMixRead(var Rec: Record Cust)
    begin
        Rec.Get('A');
        Rec.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnPlainRead', '', false, false)]
    local procedure HandlePlainRead(var Rec: Record Cust)
    begin
        Rec.Get('A');
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnPlainRead (control)",
        role(&m, &roles, "OnPlainRead", 0),
        &[
            ("loadsFromDbParam", "Yes"),
            ("persistsCurrentRecord", "Yes"),
        ],
    );
    expect(
        &mut f,
        "OnMixRead",
        role(&m, &roles, "OnMixRead", 0),
        &[
            ("loadsFromDbParam", "No"),
            ("initialisesParam", "No"),
            ("copiesIntoParam", "No"),
            ("persistsCurrentRecord", "Yes"),
        ],
    );
    done(f);
}

const COPY_PUB: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnManualCopy(var Rec: Record Cust)
    begin
    end;

    procedure NoOp(var R: Record Cust)
    begin
    end;

    procedure OpaqueHelper(var R: Record Cust)
    begin
    end;

    procedure Fwd(var R: Record Cust)
    begin
        OnManualCopy(R);
    end;

    procedure FwdTwoLevels(var R: Record Cust)
    begin
        Fwd(R);
    end;

    procedure FwdThenNoOp(var R: Record Cust)
    begin
        OnManualCopy(R);
        NoOp(R);
    end;

    procedure NoOpThenFwd(var R: Record Cust)
    begin
        NoOp(R);
        OnManualCopy(R);
    end;

    procedure FwdThenOpaque(var R: Record Cust)
    begin
        OnManualCopy(R);
        OpaqueHelper(R);
    end;
}
"#;

const COPY_MANUAL: &str = r#"
codeunit 50102 ManualSub
{
    EventSubscriberInstance = Manual;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnManualCopy', '', false, false)]
    local procedure HandleManualCopy(var Rec: Record Cust)
    var
        Other: Record Cust;
    begin
        Rec.Copy(Other);
    end;
}
"#;

/// Row 5d (and the plan-notes `OnManualCopy` regression): a sole MANUAL
/// subscriber doing `Rec.Copy(Other)`. The mask touches only the published
/// trio: the publisher `copiesIntoParam no` (masked), `mutatesParam yes`
/// (unmasked), `dirtyAtExit yes` (the run branch is `DirtyV`). A forwarder
/// `Fwd(var R) { OnManualCopy(R) }` and a second level `FwdTwoLevels(var R) {
/// Fwd(R) }`: `mutatesParam yes`, `copiesIntoParam no`, `dirtyAtExit yes`, no
/// load point.
#[test]
fn row5d_manual_copy_mutates_without_loading() {
    let m = model(&[COPY_PUB, COPY_MANUAL]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for name in ["OnManualCopy", "Fwd", "FwdTwoLevels"] {
        let r = role(&m, &roles, name, 0);
        expect(
            &mut f,
            name,
            r,
            &[
                ("copiesIntoParam", "No"),
                ("mutatesParam", "Yes"),
                ("dirtyAtExit", "Yes"),
            ],
        );
        if r.puts_in_loaded_state() {
            f.push(format!("{name} is a load point (puts_in_loaded_state)"));
        }
    }
    done(f);
}

/// Rows 5d' and 5d'': c1b ACCUMULATES `mutates_param` across call sites.
/// `FwdThenNoOp(var R) { OnManualCopy(R); NoOp(R) }` and the reversed order
/// `NoOpThenFwd` both keep `mutatesParam yes` (an overwrite per call site
/// loses the first call's `yes` in 5d' only).
#[test]
fn row5d_prime_mutates_accumulates_in_either_order() {
    let m = model(&[COPY_PUB, COPY_MANUAL]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for name in ["FwdThenNoOp", "NoOpThenFwd"] {
        expect(
            &mut f,
            name,
            role(&m, &roles, name, 0),
            &[("mutatesParam", "Yes"), ("copiesIntoParam", "No")],
        );
    }
    done(f);
}

/// Plan notes: `OnManualCopy(R); OpaqueHelper(R)` with `OpaqueHelper` bodyless
/// (hand-stated: `body_available = false`, so c1b takes its opaque branch). The
/// opaque call joins `unknown` into every presence fact, but the first call's
/// unmasked `mutatesParam yes` survives: `FwdThenOpaque.mutatesParam yes`.
#[test]
fn plan_manual_copy_then_opaque_helper_keeps_the_mutation() {
    let m = model(&[COPY_PUB, COPY_MANUAL]);
    let opaque = rid(&m, "OpaqueHelper");
    let mut routines = m.workspace.routines.clone();
    for r in routines.iter_mut().filter(|r| r.id == opaque) {
        r.body_available = false;
    }
    let graph = graph_of(&m);
    let roles = solve(&m, &routines, &graph);
    let mut f = Vec::new();
    expect(
        &mut f,
        "FwdThenOpaque",
        role(&m, &roles, "FwdThenOpaque", 0),
        &[("mutatesParam", "Yes")],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Rows 6, 6b: isolated events.
// ---------------------------------------------------------------------------

const ISO_PUB: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false, true)]
    procedure OnIso(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false, true)]
    procedure OnIsoValidate(var Rec: Record Cust)
    begin
    end;

    procedure Fwd(var R: Record Cust)
    begin
        OnIso(R);
    end;

    procedure FwdValidate(var R: Record Cust)
    begin
        OnIsoValidate(R);
    end;
}
"#;

const ISO_SUB: &str = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnIso', '', false, false)]
    local procedure HandleIso(var Rec: Record Cust)
    begin
        Rec.Validate(Name, 'x');
        Rec.Modify();
        Error('fail');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnIsoValidate', '', false, false)]
    local procedure HandleIsoValidate(var Rec: Record Cust)
    begin
        Rec.Validate(Name, 'x');
        Error('fail');
    end;
}
"#;

/// Row 6: an isolated event whose subscriber does `Validate; Modify; Error`.
/// Presence is visible MAY information (`persistsCurrentRecord yes`,
/// `validatesParam yes`); the trio is masked (`loadsFromDbParam no`); the five
/// path facts are `unknown` (a failure rolls the database work back). A
/// forwarder `Fwd(var R) { OnIso(R) }` gets `dirtyAtExit no` (persist rule
/// first, the stated leniency).
#[test]
fn row6_isolated_validate_modify_error() {
    let m = model(&[ISO_PUB, ISO_SUB]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    let p = role(&m, &roles, "OnIso", 0);
    expect(
        &mut f,
        "OnIso",
        p,
        &[
            ("persistsCurrentRecord", "Yes"),
            ("validatesParam", "Yes"),
            ("loadsFromDbParam", "No"),
        ],
    );
    expect_all(&mut f, "OnIso", p, &PATH, "Unknown");
    expect(
        &mut f,
        "Fwd",
        role(&m, &roles, "Fwd", 0),
        &[("dirtyAtExit", "No"), ("persistsCurrentRecord", "Yes")],
    );
    done(f);
}

/// Row 6b: an isolated `Validate; Error` subscriber -> the publisher
/// `validatesParam yes`, `persistsCurrentRecord no`; `FwdValidate` gets
/// `dirtyAtExit yes` (validated, then failed: fields changed and unsaved).
#[test]
fn row6b_isolated_validate_error() {
    let m = model(&[ISO_PUB, ISO_SUB]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnIsoValidate",
        role(&m, &roles, "OnIsoValidate", 0),
        &[("validatesParam", "Yes"), ("persistsCurrentRecord", "No")],
    );
    expect(
        &mut f,
        "FwdValidate",
        role(&m, &roles, "FwdValidate", 0),
        &[("dirtyAtExit", "Yes")],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Rows 7, 7b, 7c: event-id collisions and duplicate edges.
// ---------------------------------------------------------------------------

/// Row 7: an ambiguous subscription to an overloaded publisher (same object,
/// same event name, so the same event id) is an `ambiguous` dispatch edge, an
/// `Opaque` binding: the dispatch of `Rec` is unseen, every presence and path
/// fact of the publisher `unknown`, and no `yes` from the other (resolved,
/// `Validate` / `Get`) subscribers survives next to it.
///
/// Measured while writing this test: `HandleOverAmbiguous(var Rec)` and
/// `HandleOverQty(var Rec; Qty)` are both bound `resolved`, and because both
/// overloads share one event id, EACH overload carries a `resolved` dispatch
/// edge to BOTH of them. Only the parameterless `HandleOverNone()` gets the
/// `ambiguous` edge this row needs.
#[test]
fn row7_event_id_collision_is_unseen() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnOver(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnOver(var Rec: Record Cust; Qty: Integer)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOver', '', false, false)]
    local procedure HandleOverAmbiguous(var Rec: Record Cust)
    begin
        Rec.Validate(Name, 'x');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOver', '', false, false)]
    local procedure HandleOverQty(var Rec: Record Cust; Qty: Integer)
    begin
        Rec.Get('A');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOver', '', false, false)]
    local procedure HandleOverNone()
    begin
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let graph = graph_of(&m);
    // Precondition: the publisher(s) carry an `ambiguous` dispatch edge.
    let overloads: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "OnOver")
        .collect();
    assert_eq!(overloads.len(), 2, "precondition: two OnOver overloads");
    let with_ambiguous: Vec<&ModelRoutine> = overloads
        .iter()
        .copied()
        .filter(|r| {
            graph.edges_by_from.get(&r.id).is_some_and(|es| {
                es.iter()
                    .any(|e| e.kind == "event-dispatch" && e.resolution == "ambiguous")
            })
        })
        .collect();
    assert!(
        !with_ambiguous.is_empty(),
        "precondition: an ambiguous dispatch edge from an OnOver overload; edges: {:?}",
        overloads
            .iter()
            .map(|r| graph.edges_by_from.get(&r.id))
            .collect::<Vec<_>>()
    );
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for r in with_ambiguous {
        let role = roles
            .get(&r.id)
            .and_then(|v| v.iter().find(|x| x.parameter_index == 0))
            .expect("precondition: a role for Rec");
        let label = format!("OnOver/{}", r.parameters.len());
        expect_all(&mut f, &label, role, &PRESENCE, "Unknown");
        expect_all(&mut f, &label, role, &PATH, "Unknown");
    }
    done(f);
}

const SIMPLE_SUB: &str = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSimple', '', false, false)]
    local procedure HandleSimple(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;

/// Row 7b: one resolved and one ambiguous edge to the SAME subscriber
/// (hand-stated: the ambiguous copy is pushed onto the solver's graph). The
/// `Opaque` copy absorbs the `Exact` one: the dispatch is unseen, every
/// presence and path fact `unknown`.
#[test]
fn row7b_opaque_copy_absorbs_exact_edge() {
    let m = model(&[SIMPLE, SIMPLE_SUB]);
    let mut graph = graph_of(&m);
    let mut e = simple_edge(&m, &graph);
    e.resolution = "ambiguous".to_string();
    graph.edges_by_from.get_mut(&e.from).unwrap().push(e);
    let roles = solve(&m, &m.workspace.routines, &graph);
    let r = role(&m, &roles, "OnSimple", 0);
    let mut f = Vec::new();
    expect_all(&mut f, "OnSimple", r, &PRESENCE, "Unknown");
    expect_all(&mut f, "OnSimple", r, &PATH, "Unknown");
    done(f);
}

/// Row 7c: a duplicate resolved (subscriber, q) edge counts once (n = 1,
/// exact): `OnSimple` equals the direct-call oracle.
#[test]
fn row7c_duplicate_resolved_edge_counts_once() {
    let m = model(&[SIMPLE, SIMPLE_SUB]);
    let mut graph = graph_of(&m);
    let e = simple_edge(&m, &graph);
    graph.edges_by_from.get_mut(&e.from).unwrap().push(e);
    let roles = solve(&m, &m.workspace.routines, &graph);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnSimple",
        role(&m, &roles, "OnSimple", 0),
        &MODIFY_ORACLE,
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Row 7d: the overload collision (step-6 Ruling).
// ---------------------------------------------------------------------------

/// Ruling (step 6): two overloads share one event id, so `OnPair(var Rec)`
/// also gets a RESOLVED edge to `HandlePairQty(var Rec; Qty)`, whose `Qty`
/// binds no parameter of that overload: opaque there, exact on the other.
#[test]
fn row7d_overload_collision_resolved_edge_is_opaque() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnPair(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnPair(var Rec: Record Cust; Qty: Integer)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnPair', '', false, false)]
    local procedure HandlePairQty(var Rec: Record Cust; Qty: Integer)
    begin
        Rec.Validate(Name, 'x');
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let graph = graph_of(&m);
    let handler = rid(&m, "HandlePairQty");
    let overloads: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "OnPair")
        .collect();
    assert_eq!(overloads.len(), 2, "precondition: two OnPair overloads");
    for r in &overloads {
        let es: Vec<_> = graph
            .edges_by_from
            .get(&r.id)
            .map(|v| v.iter().filter(|e| e.kind == "event-dispatch").collect())
            .unwrap_or_default();
        assert!(
            es.len() == 1 && es[0].to == handler && es[0].resolution == "resolved",
            "precondition: OnPair/{} has one resolved dispatch edge to HandlePairQty: {es:?}",
            r.parameters.len()
        );
    }
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for r in overloads {
        let role = roles
            .get(&r.id)
            .and_then(|v| v.iter().find(|x| x.parameter_index == 0))
            .expect("precondition: a role for Rec");
        let label = format!("OnPair/{}", r.parameters.len());
        match r.parameters.len() {
            1 => {
                expect_all(&mut f, &label, role, &PRESENCE, "Unknown");
                expect_all(&mut f, &label, role, &PATH, "Unknown");
            }
            _ => expect(
                &mut f,
                &label,
                role,
                &[("validatesParam", "Yes"), ("mutatesParam", "Yes")],
            ),
        }
    }
    done(f);
}

/// Ruling (step 6, revised): overloads share one event id, so a subscriber
/// resolved to ONE overload gets a dispatch edge from EVERY overload, and a
/// smaller subscriber's names (`Rec`) all bind the larger overload too. The
/// engine does not keep which overload the subscription selected, so every
/// binding of a publisher whose event id another publisher shares is optional
/// and path-opaque: `HandlePair`'s `Get` is no proven load on either overload
/// (`loadsFromDbParam no`). Control: the same subscriber on a single,
/// unshared `OnSimple` is a proven `yes` (`review_isolated_loader_is_masked`).
#[test]
fn row7e_overload_shared_event_id_bindings_are_never_exact() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnPair(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnPair(var Rec: Record Cust; Qty: Integer)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnPair', '', false, false)]
    local procedure HandlePair(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let graph = graph_of(&m);
    let handler = rid(&m, "HandlePair");
    let overloads: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "OnPair")
        .collect();
    assert_eq!(overloads.len(), 2, "precondition: two OnPair overloads");
    for r in &overloads {
        let es: Vec<_> = graph
            .edges_by_from
            .get(&r.id)
            .map(|v| v.iter().filter(|e| e.kind == "event-dispatch").collect())
            .unwrap_or_default();
        assert!(
            es.len() == 1
                && es[0].to == handler
                && es[0].resolution == "resolved"
                && es[0]
                    .dispatch
                    .as_ref()
                    .is_some_and(|d| !d.conditional && !d.isolated),
            "precondition: OnPair/{} has one plain resolved dispatch edge to HandlePair: {es:?}",
            r.parameters.len()
        );
    }
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for r in overloads {
        let role = roles
            .get(&r.id)
            .and_then(|v| v.iter().find(|x| x.parameter_index == 0))
            .expect("precondition: a role for Rec");
        let label = format!("OnPair/{}", r.parameters.len());
        expect(&mut f, &label, role, &[("loadsFromDbParam", "No")]);
    }
    done(f);
}

// ---------------------------------------------------------------------------
// Row 9: trigger raisers (non-goal pinned).
// ---------------------------------------------------------------------------

/// Row 9: a table trigger raising `OnBeforeCustModify(Rec)` passes its implicit
/// `Rec`, which is not a declared record parameter: the trigger gains no role.
/// Control: the publisher itself does compose its `Validate` subscriber.
#[test]
fn row9_trigger_raiser_gains_no_role() {
    let table = r#"
table 50100 Cust
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
    }
    keys { key(PK; "No.") { } }

    trigger OnModify()
    begin
        OnBeforeCustModify(Rec);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeCustModify(var Cust: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Table, Database::Cust, 'OnBeforeCustModify', '', false, false)]
    local procedure HandleBeforeCustModify(var Cust: Record Cust)
    begin
        Cust.Validate(Name, 'x');
    end;
}
"#;
    let m = model_of(&[("Tab", table.to_string()), ("Sub", subs.to_string())]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnBeforeCustModify (control)",
        role(&m, &roles, "OnBeforeCustModify", 0),
        &[("validatesParam", "Yes"), ("dirtyAtExit", "Yes")],
    );
    let trigger = rid(&m, "OnModify");
    let trigger_roles = roles.get(&trigger).cloned().unwrap_or_default();
    if !trigger_roles.is_empty() {
        f.push(format!("the OnModify trigger has roles: {trigger_roles:?}"));
    }
    done(f);
}

// ---------------------------------------------------------------------------
// Row 10: a publisher-subscriber cycle (recursive SCC).
// ---------------------------------------------------------------------------

/// Row 10 (`ws-cycle`): `HandleCyc` re-raises `OnCyc`, a recursive SCC
/// `{OnCyc, HandleCyc}`; `HandleCyc2` loads. It converges without a cap (no
/// summarize diagnostic), with `HandleCyc`'s `Modify` in `OnCyc`'s role and
/// `HandleCyc2`'s unconditional `Get` as `loadsFromDbParam yes`.
#[test]
fn row10_cycle_converges_with_the_subscribers_effects() {
    let cyc = r#"
codeunit 50102 Cyc
{
    [IntegrationEvent(false, false)]
    procedure OnCyc(var Rec: Record Cust; Depth: Integer)
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Cyc, 'OnCyc', '', false, false)]
    local procedure HandleCyc(var Rec: Record Cust; Depth: Integer)
    begin
        if Depth > 0 then
            OnCyc(Rec, Depth - 1);
        Rec.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Cyc, 'OnCyc', '', false, false)]
    local procedure HandleCyc2(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;
    let m = model(&[cyc]);
    let ctx = build_detector_context(&m, substrate::ALL);
    let mut f = Vec::new();
    if !ctx.summarize_diagnostics.is_empty() {
        f.push(format!("roles capped: {:?}", ctx.summarize_diagnostics));
    }
    let roles = ctx_roles(&m);
    expect(
        &mut f,
        "OnCyc",
        role(&m, &roles, "OnCyc", 0),
        &[
            ("persistsCurrentRecord", "Yes"),
            ("mutatesParam", "Yes"),
            ("loadsFromDbParam", "Yes"),
        ],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Review Focus 1, 2.
// ---------------------------------------------------------------------------

/// Review Focus 1: `Sender` binds no publisher parameter and is not counted.
#[test]
fn review_sender_parameter_keeps_the_binding_exact() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(true, false)]
    procedure OnSend(var Rec: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSend', '', false, false)]
    local procedure HandleSend(Sender: Codeunit Pub; var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let graph = graph_of(&m);
    let es: Vec<_> = graph.edges_by_from[&rid(&m, "OnSend")]
        .iter()
        .filter(|e| e.kind == "event-dispatch")
        .collect();
    assert!(
        es.len() == 1 && es[0].to == rid(&m, "HandleSend") && es[0].resolution == "resolved",
        "precondition: one resolved dispatch edge OnSend -> HandleSend: {es:?}"
    );
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnSend",
        role(&m, &roles, "OnSend", 0),
        &[("persistsCurrentRecord", "Yes"), ("mutatesParam", "Yes")],
    );
    done(f);
}

/// Review Focus 2: only the parameter the subscriber declares composes.
#[test]
fn review_partial_binding_leaves_the_other_parameter_alone() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnTwoRecs(var A: Record Cust; var B: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnTwoRecs', '', false, false)]
    local procedure HandleB(var B: Record Cust)
    begin
        B.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnTwoRecs/B",
        role(&m, &roles, "OnTwoRecs", 1),
        &[("persistsCurrentRecord", "Yes")],
    );
    expect_all(
        &mut f,
        "OnTwoRecs/A",
        role(&m, &roles, "OnTwoRecs", 0),
        &PRESENCE,
        "No",
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Task 4 review fix: the branches of the presence fold.
// ---------------------------------------------------------------------------

const SIMPLE: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnSimple(var Rec: Record Cust)
    begin
    end;
}
"#;

const SIMPLE_ISOLATED: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false, true)]
    procedure OnSimple(var Rec: Record Cust)
    begin
    end;
}
"#;

const GET_SUB: &str = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSimple', '', false, false)]
    local procedure HandleSimple(var Rec: Record Cust)
    begin
        Rec.Get('A');
    end;
}
"#;

/// `OnSimple`'s single dispatch edge (precondition: one, resolved).
fn simple_edge(
    m: &Model,
    graph: &CombinedGraph,
) -> al_sem::engine::l4::combined_graph::CombinedEdge {
    let p = rid(m, "OnSimple");
    let es: Vec<_> = graph.edges_by_from[&p]
        .iter()
        .filter(|e| e.kind == "event-dispatch")
        .cloned()
        .collect();
    assert_eq!(es.len(), 1, "precondition: one dispatch edge from OnSimple");
    assert_eq!(es[0].resolution, "resolved", "precondition: resolved");
    es[0].clone()
}

/// An isolated subscriber may not run to completion (its work is rolled back),
/// so its load is masked in the publisher's presence facts: `loadsFromDbParam
/// no`. The same subscriber on a plain event is a proven `yes` (control).
#[test]
fn review_isolated_loader_is_masked() {
    let plain = model(&[SIMPLE, GET_SUB]);
    let iso = model(&[SIMPLE_ISOLATED, GET_SUB]);
    let g = graph_of(&iso);
    let e = simple_edge(&iso, &g);
    assert!(
        e.dispatch.as_ref().is_some_and(|d| d.isolated),
        "precondition: the dispatch edge is isolated"
    );
    let mut f = Vec::new();
    expect(
        &mut f,
        "plain OnSimple",
        role(&plain, &ctx_roles(&plain), "OnSimple", 0),
        &[("loadsFromDbParam", "Yes")],
    );
    expect(
        &mut f,
        "isolated OnSimple",
        role(&iso, &ctx_roles(&iso), "OnSimple", 0),
        &[("loadsFromDbParam", "No")],
    );
    done(f);
}

/// Two edges to one subscriber (hand-stated: the second is conditional) are one
/// binding. It is optional only if EVERY edge may skip it, so the unconditional
/// edge keeps the load: `loadsFromDbParam yes`. OR-ing `optional` would mask it
/// (`no`). (`path_opaque` is OR-ed: an isolated edge on any copy isolates it.)
#[test]
fn review_duplicate_edges_merge_their_flags() {
    let m = model(&[SIMPLE, GET_SUB]);
    let mut graph = graph_of(&m);
    let mut e = simple_edge(&m, &graph);
    e.dispatch = Some(al_sem::engine::l4::combined_graph::DispatchMeta {
        conditional: true,
        isolated: false,
    });
    graph.edges_by_from.get_mut(&e.from).unwrap().push(e);
    let roles = solve(&m, &m.workspace.routines, &graph);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnSimple",
        role(&m, &roles, "OnSimple", 0),
        &[("loadsFromDbParam", "Yes")],
    );
    done(f);
}

/// A plain (unconditional) edge and an isolated edge to one `Get` subscriber
/// (hand-stated: the isolated copy is pushed) are one binding, and it is
/// isolated. An isolated subscriber may not run to completion, so it is
/// optional too (`path_opaque => optional`): `loadsFromDbParam no`, never the
/// plain edge's proven `yes`.
#[test]
fn review_duplicate_plain_and_isolated_edges_mask_the_load() {
    let m = model(&[SIMPLE, GET_SUB]);
    let mut graph = graph_of(&m);
    let mut e = simple_edge(&m, &graph);
    e.dispatch = Some(al_sem::engine::l4::combined_graph::DispatchMeta {
        conditional: false,
        isolated: true,
    });
    graph.edges_by_from.get_mut(&e.from).unwrap().push(e);
    let roles = solve(&m, &m.workspace.routines, &graph);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnSimple",
        role(&m, &roles, "OnSimple", 0),
        &[("loadsFromDbParam", "No")],
    );
    done(f);
}

fn four_subs() -> String {
    let mut s = String::from("codeunit 50101 Sub\n{\n");
    for i in 0..4 {
        s.push_str(&format!(
            "    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSimple', '', false, false)]\n    local procedure Handle{i}(var Rec: Record Cust)\n    begin\n        Rec.Modify();\n    end;\n\n"
        ));
    }
    s.push_str("}\n");
    s
}

/// The bound is `EXACT_DISPATCH_MAX` = 4: exactly four subscribers stay exact,
/// and a duplicate edge to one of them (5 edges, 4 subscribers) is still four.
#[test]
fn review_four_subscribers_stay_exact_even_with_a_duplicate_edge() {
    let subs = four_subs();
    let m = model(&[SIMPLE, &subs]);
    let mut graph = graph_of(&m);
    let p = rid(&m, "OnSimple");
    let dispatch: Vec<_> = graph.edges_by_from[&p]
        .iter()
        .filter(|e| e.kind == "event-dispatch")
        .cloned()
        .collect();
    assert_eq!(dispatch.len(), 4, "precondition: four dispatch edges");
    let mut f = Vec::new();
    let want = [("persistsCurrentRecord", "Yes"), ("mutatesParam", "Yes")];
    expect(
        &mut f,
        "four",
        role(&m, &solve(&m, &m.workspace.routines, &graph), "OnSimple", 0),
        &want,
    );
    graph
        .edges_by_from
        .get_mut(&p)
        .unwrap()
        .push(dispatch[0].clone());
    expect(
        &mut f,
        "four + duplicate",
        role(&m, &solve(&m, &m.workspace.routines, &graph), "OnSimple", 0),
        &want,
    );
    done(f);
}

/// A dispatch edge whose subscriber is not in the routine set (hand-stated)
/// leaves the dispatch unseen: every presence fact `unknown`.
#[test]
fn review_unknown_subscriber_routine_is_unseen() {
    let m = model(&[SIMPLE, GET_SUB]);
    let mut graph = graph_of(&m);
    let mut e = simple_edge(&m, &graph);
    e.to = "no-such-routine".to_string();
    graph.edges_by_from.get_mut(&e.from).unwrap().push(e);
    let roles = solve(&m, &m.workspace.routines, &graph);
    let mut f = Vec::new();
    expect_all(
        &mut f,
        "OnSimple",
        role(&m, &roles, "OnSimple", 0),
        &PRESENCE,
        "Unknown",
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Row 11: a subscriber that omits the parameter / takes it by value.
// ---------------------------------------------------------------------------

/// Row 11: `HandleOmitted(Flag)` cannot touch `Rec` (the publisher's presence
/// facts stay `no`); `HandleByValue(Rec)` doing `Modify` adds the entry
/// requirement only (`requiresLoadedAtEntry yes`; exit effects need `var` on
/// both sides: `persistsCurrentRecord no`, `mutatesParam no`, `dirtyAtExit no`).
#[test]
fn row11_omitted_and_by_value_subscribers() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnRecByValue(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnRecOmitted(var Rec: Record Cust; Flag: Boolean)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnRecByValue', '', false, false)]
    local procedure HandleByValue(Rec: Record Cust)
    begin
        Rec.Modify();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnRecOmitted', '', false, false)]
    local procedure HandleOmitted(Flag: Boolean)
    begin
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect_all(
        &mut f,
        "OnRecOmitted",
        role(&m, &roles, "OnRecOmitted", 0),
        &PRESENCE,
        "No",
    );
    expect(
        &mut f,
        "OnRecByValue",
        role(&m, &roles, "OnRecByValue", 0),
        &[
            ("requiresLoadedAtEntry", "Yes"),
            ("persistsCurrentRecord", "No"),
            ("mutatesParam", "No"),
            ("dirtyAtExit", "No"),
        ],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Rows 12, 12b: more than EXACT_DISPATCH_MAX bindings.
// ---------------------------------------------------------------------------

const FIVE_PUB: &str = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnFive(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnFiveVal(Rec: Record Cust)
    begin
    end;

    procedure Fwd(var R: Record Cust)
    begin
        OnFive(R);
    end;

    procedure G(var R: Record Cust)
    var
        X: Code[20];
    begin
        R.SetLoadFields(Name);
        OnFive(R);
        R.Get('A');
        X := R."No.";
    end;

    procedure H(var R: Record Cust)
    begin
        R.Validate(Name, 'x');
        OnFive(R);
    end;

    procedure GVal(var R: Record Cust)
    var
        X: Code[20];
    begin
        R.SetLoadFields(Name);
        OnFiveVal(R);
        R.Get('A');
        X := R."No.";
    end;

    procedure GControl(var R: Record Cust)
    var
        X: Code[20];
    begin
        R.SetLoadFields(Name);
        R.Get('A');
        X := R."No.";
    end;
}
"#;

/// Five subscribers of `OnFive` (one `Get`, one `Validate`, three no-ops) and
/// five of the by-value `OnFiveVal`, all binding `Rec` by name.
fn five_subs() -> String {
    let mut s = String::from("codeunit 50101 Sub\n{\n");
    for (event, var) in [("OnFive", "var "), ("OnFiveVal", "")] {
        for i in 0..5 {
            let body = match i {
                0 => "        Rec.Get('A');\n",
                1 => "        Rec.Validate(Name, 'x');\n",
                _ => "",
            };
            s.push_str(&format!(
                "    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, '{event}', '', false, false)]\n    local procedure Handle{event}{i}({var}Rec: Record Cust)\n    begin\n{body}    end;\n\n"
            ));
        }
    }
    s.push_str("}\n");
    s
}

/// Row 12: five subscribers (over the exact bound), one `Get` and one
/// `Validate` -> unseen dispatch: every presence and path fact of the
/// publisher `unknown` (no `loadsFromDbParam yes`, no `validatesParam yes`).
/// Through `Fwd(var R) { OnFive(R) }`: no proven load, no `dirtyAtExit yes`
/// from the subscribers, no load point. By-value publisher parameter
/// (`OnFiveVal(Rec)`, plan notes): no presence change (all `no`), and its
/// caller `GVal` keeps its loaded state and narrowing (loaded fields equal to
/// the same body without the call, `GControl`).
#[test]
fn row12_five_subscribers_are_unseen() {
    let subs = five_subs();
    let m = model(&[FIVE_PUB, &subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    let p = role(&m, &roles, "OnFive", 0);
    expect_all(&mut f, "OnFive", p, &PRESENCE, "Unknown");
    expect_all(&mut f, "OnFive", p, &PATH, "Unknown");
    let fwd = role(&m, &roles, "Fwd", 0);
    for (name, bad) in [("loadsFromDbParam", "Yes"), ("dirtyAtExit", "Yes")] {
        if fact(fwd, name) == bad {
            f.push(format!("Fwd.{name}: {bad} from the subscribers"));
        }
    }
    if fwd.puts_in_loaded_state() {
        f.push("Fwd is a load point (puts_in_loaded_state)".to_string());
    }
    expect_all(
        &mut f,
        "OnFiveVal",
        role(&m, &roles, "OnFiveVal", 0),
        &PRESENCE,
        "No",
    );
    let (gv, gc) = (role(&m, &roles, "GVal", 0), role(&m, &roles, "GControl", 0));
    if gv.current_loaded_fields_at_exit != gc.current_loaded_fields_at_exit {
        f.push(format!(
            "GVal lost its narrowing: {:?} vs GControl {:?}",
            gv.current_loaded_fields_at_exit, gc.current_loaded_fields_at_exit
        ));
    }
    done(f);
}

/// Row 12b: `G(var R) { R.SetLoadFields(Name); OnFive(R); R.Get('A'); X :=
/// R."No." }` -> after the unseen dispatch the narrowing is `Unknown` (not
/// kept), so `G`'s loaded fields after the `Get` are `Unknown` (with the
/// narrowing kept they would be `[Name]`). `H(var R) { R.Validate(..);
/// OnFive(R) }` keeps its own `DirtyV`: `dirtyAtExit yes` from `H`'s own
/// `Validate`, not from the subscribers.
#[test]
fn row12b_unseen_dispatch_drops_the_callers_narrowing() {
    let subs = five_subs();
    let m = model(&[FIVE_PUB, &subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    let g = role(&m, &roles, "G", 0);
    if g.current_loaded_fields_at_exit != FieldList::Unknown {
        f.push(format!(
            "G.currentLoadedFieldsAtExit: want Unknown, got {:?}",
            g.current_loaded_fields_at_exit
        ));
    }
    expect(
        &mut f,
        "H",
        role(&m, &roles, "H", 0),
        &[("dirtyAtExit", "Yes")],
    );
    done(f);
}

// ---------------------------------------------------------------------------
// Plan notes: a subscriber that raises another event.
// ---------------------------------------------------------------------------

/// Plan notes: `HandleOuter` (a subscriber of `OnOuter`) raises `OnInner`, whose
/// subscriber does `Modify`. The effect travels two dispatches: `OnOuter` and
/// its forwarder equal the direct-call oracle.
#[test]
fn plan_subscriber_forwarding_another_event() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnOuter(var Rec: Record Cust)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnInner(var Rec: Record Cust)
    begin
    end;

    procedure ForwardOuter(var R: Record Cust)
    begin
        OnOuter(R);
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnOuter', '', false, false)]
    local procedure HandleOuter(var Rec: Record Cust)
    var
        P: Codeunit Pub;
    begin
        P.OnInner(Rec);
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnInner', '', false, false)]
    local procedure HandleInner(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    for name in ["OnInner", "HandleOuter", "OnOuter", "ForwardOuter"] {
        expect(&mut f, name, role(&m, &roles, name, 0), &MODIFY_ORACLE);
    }
    done(f);
}

// ---------------------------------------------------------------------------
// Review Focus 3, 5.
// ---------------------------------------------------------------------------

/// Review Focus 3: a resolved subscriber without a body (hand-stated:
/// `body_available = false`) is visible but opaque: the publisher's facts are
/// `unknown`, never `no`, and so are its required entry fields (the opaque
/// subscriber may read any field, as in an unseen dispatch).
#[test]
fn review_bodyless_subscriber_is_opaque() {
    let m = model(&[SIMPLE, SIMPLE_SUB]);
    let graph = graph_of(&m);
    simple_edge(&m, &graph);
    let handler = rid(&m, "HandleSimple");
    let mut routines = m.workspace.routines.clone();
    for r in routines.iter_mut().filter(|r| r.id == handler) {
        r.body_available = false;
    }
    let roles = solve(&m, &routines, &graph);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnSimple",
        role(&m, &roles, "OnSimple", 0),
        &[
            ("persistsCurrentRecord", "Unknown"),
            ("requiresLoadedAtEntry", "Unknown"),
            ("requiredLoadedFieldsAtEntry", "Unknown"),
        ],
    );
    done(f);
}

/// Review Focus 5: a by-value publisher parameter composes the entry
/// requirement only (c1a), no presence fact.
#[test]
fn review_by_value_publisher_param_composes_entry_only() {
    let pubs = r#"
codeunit 50100 Pub
{
    [IntegrationEvent(false, false)]
    procedure OnVal(Rec: Record Cust)
    begin
    end;
}
"#;
    let subs = r#"
codeunit 50101 Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnVal', '', false, false)]
    local procedure HandleVal(var Rec: Record Cust)
    begin
        Rec.Modify();
    end;
}
"#;
    let m = model(&[pubs, subs]);
    let roles = ctx_roles(&m);
    let mut f = Vec::new();
    expect(
        &mut f,
        "OnVal",
        role(&m, &roles, "OnVal", 0),
        &[
            ("persistsCurrentRecord", "No"),
            ("mutatesParam", "No"),
            ("requiresLoadedAtEntry", "Yes"),
        ],
    );
    done(f);
}
