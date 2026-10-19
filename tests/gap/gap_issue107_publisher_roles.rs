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
use al_sem::engine::l4::summary::RecordRoleSummary;
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
// Row 6b: isolated events.
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
            1 => expect_all(&mut f, &label, role, &PRESENCE, "Unknown"),
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
