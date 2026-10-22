//! #160 step 11b acceptance tests (spec
//! `docs/superpowers/specs/2026-10-22-issue-160-step11b-routine-rekeying-design.md`
//! §5): the projection's rules for rows that share one string id. Every
//! collision is made the natural way where the source can make it (one
//! procedure declared in an `#if A` / `#else` pair: two rows, one id, one node;
//! the FIRST row is the `#if` arm, the LAST the `#else` arm), and each test
//! asserts its precondition from the CURRENT rows first.
//!
//! Each test names the stage that turns it green (C, A or B, §4.1). Through C
//! and A the projection reproduces today's rules, so the B tests are red until
//! stage B; T7, T-guard and T1j hold today by design.

use super::*;
use crate::engine::l5::registry::substrate;
use crate::program::model::program_calls::{
    assemble_and_resolve_inline_program_default,
    assemble_and_resolve_inline_program_reversed_groups,
};

fn build(src: &str, guid: &str) -> Model {
    assemble_and_resolve_inline_program_default(&[("src/T.al".to_string(), src.to_string())], guid)
}

fn build_reversed(src: &str, guid: &str) -> Model {
    assemble_and_resolve_inline_program_reversed_groups(
        &[("src/T.al".to_string(), src.to_string())],
        guid,
    )
}

/// The rows named `name`, by position (`RoutineIx`).
fn rows_named(m: &Model, name: &str) -> Vec<usize> {
    m.workspace
        .routines
        .iter()
        .enumerate()
        .filter(|(_, r)| r.name.eq_ignore_ascii_case(name))
        .map(|(ri, _)| ri)
        .collect()
}

/// The collided id of `name`: asserts the group precondition (exactly two rows,
/// one id) and returns it.
fn collided(m: &Model, name: &str) -> (String, Vec<usize>) {
    let rows = rows_named(m, name);
    assert_eq!(rows.len(), 2, "precondition: {name} is two rows");
    let id = m.workspace.routines[rows[0]].id.clone();
    assert_eq!(
        m.workspace.routines[rows[1]].id, id,
        "precondition: {name}'s rows share one id"
    );
    (id, rows)
}

fn id_named(m: &Model, name: &str) -> String {
    let rows = rows_named(m, name);
    assert_eq!(rows.len(), 1, "precondition: {name} is one row");
    m.workspace.routines[rows[0]].id.clone()
}

fn table_id(m: &Model, name: &str) -> String {
    m.workspace
        .tables
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("precondition: table {name}"))
        .id
        .clone()
}

/// The source line (0-based) of a row's declaration: tells the `#if` arm (the
/// smaller line) from the `#else` arm.
fn line(m: &Model, ri: usize) -> u32 {
    m.workspace.routines[ri].source_anchor.start_line
}

// ---------------------------------------------------------------------------
// T1 (§5.1): the collision fixture flips. Stage B.
// ---------------------------------------------------------------------------

/// T1's fixture: `Work` in two complete arms whose direct facts differ (arm A
/// inserts `CR A` and calls `TouchA`, which inserts `CR X`; arm B modifies
/// `CR B` and calls `TouchB`, which inserts `CR Z`).
const WORK_SRC: &str = r#"
table 50813 "CR A"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50814 "CR B"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50815 "CR X"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50816 "CR Z"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50813 "CR Work"
{
#if A
    procedure Work()
    var
        A: Record "CR A";
    begin
        A.Insert();
        TouchA();
    end;
#else
    procedure Work()
    var
        B: Record "CR B";
    begin
        B.Modify();
        TouchB();
    end;
#endif

    local procedure TouchA()
    var
        X: Record "CR X";
    begin
        X.Insert();
    end;

    local procedure TouchB()
    var
        Z: Record "CR Z";
    begin
        Z.Insert();
    end;
}
"#;

/// T1 (§5.1, rebuilt for R1-06; replaces the hand-stated 11a fixture of
/// `tests::collision_rules_hand_stated`, which is deleted at stage B): a
/// natural `#if` collision whose arms can tell the rules apart. At B the
/// projected direct facts are A ∪ B (A's insert AND B's modify), the edges keep
/// both callees, and the solve's `db_effects` hold both arms' direct writes.
/// Breaks: projection first-row -> "B's modify" fails; last-row -> "A's insert"
/// fails.
#[test]
fn collision_rules_hand_stated() {
    use crate::engine::l4::capability_cone::direct_facts_for_routine;
    let m = build(WORK_SRC, "22222222-0000-0000-0000-00000011b001");
    let (id, rows) = collided(&m, "Work");
    assert!(
        line(&m, rows[0]) < line(&m, rows[1]),
        "precondition: arm A first"
    );
    let facts = |ri: usize| direct_facts_for_routine(&m.workspace.routines[ri], &[]).0;
    let (a_direct, b_direct) = (facts(rows[0]), facts(rows[1]));
    assert!(
        !a_direct.is_empty() && !b_direct.is_empty() && a_direct != b_direct,
        "precondition: the arms' direct facts differ and are both non-empty"
    );
    let (touch_a, touch_b) = (id_named(&m, "TouchA"), id_named(&m, "TouchB"));

    let inputs = det_cone_inputs(&m);
    let direct = inputs.direct_full.get(&id).expect("direct facts of Work");
    for f in &a_direct {
        assert!(direct.contains(f), "A's insert in the direct facts: {f:?}");
    }
    for f in &b_direct {
        assert!(direct.contains(f), "B's modify in the direct facts: {f:?}");
    }

    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let to: BTreeSet<&str> = ctx
        .graph
        .edges_by_from
        .get(&id)
        .map(|v| v.iter().map(|e| e.to.as_str()).collect())
        .unwrap_or_default();
    assert!(
        to.contains(touch_a.as_str()) && to.contains(touch_b.as_str()),
        "both arms' callees: {to:?}"
    );
    let summary = ctx.summaries.get(&id).expect("a summary");
    for f in a_direct.iter().chain(&b_direct) {
        assert!(
            summary.capability_facts_direct.contains(f),
            "the summary's direct half is A ∪ B: {f:?}"
        );
    }
    let bundle = &owned.l4().expect("CORE_SUMMARIES").bundle;
    let ix = bundle.routine_ix(&id).expect("Work in the solve");
    let tables: BTreeSet<String> = bundle
        .db_effects(ix)
        .map(|e| e.table_id.to_string())
        .collect();
    for t in ["CR A", "CR B", "CR X", "CR Z"] {
        assert!(
            tables.contains(&table_id(&m, t)),
            "the solve's db_effects hold {t}: {tables:?}"
        );
    }
}

/// T1-cov (§5.1, decision 7): coverage takes the LEAST complete status and the
/// sorted union of reasons. Arm A is set `parse_incomplete`, arm B
/// `body_available = false`, by assignment (the stated precondition). Through
/// the production consumer (`build_detector_context`'s coverage record) the
/// routine's direct coverage is `unknown` with reasons
/// `["opaque-dependency", "parse-incomplete"]`. Break: LAST reasons -> one
/// reason missing. Stage B.
#[test]
fn collided_coverage_is_the_least_complete_union() {
    let mut m = build(WORK_SRC, "22222222-0000-0000-0000-00000011b002");
    let (id, rows) = collided(&m, "Work");
    m.workspace.routines[rows[0]].parse_incomplete = true;
    m.workspace.routines[rows[1]].body_available = false;
    let owned = build_detector_context(&m, substrate::SUMMARIES);
    let ctx = owned.ctx();
    let cov = ctx
        .summaries
        .get(&id)
        .and_then(|s| s.coverage.as_ref())
        .expect("a coverage record");
    assert_eq!(cov.direct_status, "unknown", "{cov:?}");
    for r in ["opaque-dependency", "parse-incomplete"] {
        assert!(
            cov.reasons.iter().any(|x| x == r),
            "the reasons are the sorted union, {r} missing: {cov:?}"
        );
    }
    let mut sorted = cov.reasons.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(cov.reasons, sorted, "reasons sorted and unique");
}

// ---------------------------------------------------------------------------
// T2 / T2u (§5.2): order independence of the set-valued surfaces. Stage B.
// ---------------------------------------------------------------------------

/// T2's fixture: the arms agree on `cs0`-`cs2` (the same three calls) and on
/// their operations; arm B alone owns `cs3` (a call to `WriteZ`), a `Commit`
/// and a raise of the published `OnDone`.
const SWAP_SRC: &str = r#"
table 50817 "SW X"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50818 "SW Z"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50817 "SW Work"
{
#if A
    procedure Work()
    var
        X: Record "SW X";
    begin
        X.Insert();
        Helper1();
        Helper2();
        Missing3();
    end;
#else
    procedure Work()
    var
        X: Record "SW X";
    begin
        X.Insert();
        Helper1();
        Helper2();
        Missing3();
        WriteZ();
        Commit();
        OnDone();
    end;
#endif

    local procedure Helper1()
    begin
    end;

    local procedure Helper2()
    begin
    end;

    local procedure WriteZ()
    var
        Z: Record "SW Z";
    begin
        Z.Insert();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnDone()
    begin
    end;
}
"#;

/// Every set-valued surface §5.2 compares, for `id`, as text (each payload's
/// `Debug`), in the order the production code holds it.
fn swap_surfaces(m: &Model, id: &str) -> Vec<(&'static str, Vec<String>)> {
    let dbg = |v: &dyn std::fmt::Debug| format!("{v:?}");
    let inputs = det_cone_inputs(m);
    let owned = build_detector_context(m, substrate::ALL);
    let ctx = owned.ctx();
    let bundle = &owned.l4().expect("CORE_SUMMARIES").bundle;
    let ix = bundle.routine_ix(id).expect("Work in the solve");
    let mut reverse: Vec<String> = ctx
        .reverse_call_graph
        .values()
        .flatten()
        .filter(|e| e.from == id)
        .map(|e| dbg(e))
        .collect();
    reverse.sort();
    let snapshot = crate::engine::l5::snapshot::compose_snapshot(m);
    let stable = m
        .workspace
        .routines
        .iter()
        .find(|r| r.id == id)
        .expect("Work")
        .stable_routine_id
        .clone();
    let facts: Vec<String> = snapshot
        .capability_facts
        .iter()
        .filter(|f| f.subject == stable)
        .map(|f| dbg(f))
        .collect();
    let mut once = facts.clone();
    once.sort();
    once.dedup();
    assert_eq!(
        once.len(),
        facts.len(),
        "the snapshot lists each capability fact of the id once: {facts:#?}"
    );
    vec![
        (
            "det_cone_inputs.direct_full",
            inputs.direct_full[id].iter().map(|f| dbg(f)).collect(),
        ),
        (
            "det_cone_inputs.coverage_in",
            vec![dbg(&inputs.coverage_in[id])],
        ),
        (
            "solve db_effects",
            bundle.db_effects(ix).map(|e| dbg(&e.to_owned())).collect(),
        ),
        (
            "uncertainties_by_node",
            ctx.uncertainty_view()
                .values_of(id)
                .map(|u| dbg(u))
                .collect(),
        ),
        (
            "rel.primary().edges_by_from",
            ctx.graph.edges_by_from[id].iter().map(|e| dbg(e)).collect(),
        ),
        (
            "uncertainty_edges_by_from",
            ctx.uncertainty_edges_by_from
                .get(id)
                .map(|v| v.iter().map(|u| dbg(u)).collect())
                .unwrap_or_default(),
        ),
        ("reverse_call_graph", reverse),
        (
            "transaction_spans",
            ctx.transaction_spans
                .iter()
                .filter(|s| s.commit_routine_id == id)
                .map(|s| dbg(s))
                .collect(),
        ),
        ("snapshot capability_facts", facts),
        (
            "snapshot callsite_index",
            snapshot
                .callsite_index
                .iter()
                .filter(|c| c.routine == stable)
                .map(|c| dbg(c))
                .collect(),
        ),
    ]
}

/// T2 (§5.2, R1-08, R2-03): the same source assembled with the rows of every
/// collision group in REVERSED order gives the same set-valued surfaces
/// (classes P, R and G are excluded by design: they follow row order on
/// purpose). Every source anchor in these payloads is the arm's own, so the
/// arms' anchors need no normalization here: a surface differs only if a rule
/// picks a row. Before B the two orders differ (A's compat rules pick by row).
/// Break: revert any one union to a first-row pick -> that surface differs.
/// Stage B.
#[test]
fn set_valued_surfaces_do_not_depend_on_row_order() {
    let guid = "22222222-0000-0000-0000-00000011b003";
    let (m, r) = (build(SWAP_SRC, guid), build_reversed(SWAP_SRC, guid));
    let (id, rows) = collided(&m, "Work");
    let (rid, rrows) = collided(&r, "Work");
    assert_eq!(id, rid, "precondition: one id in both orders");
    assert!(
        line(&m, rows[0]) < line(&m, rows[1]) && line(&r, rrows[0]) > line(&r, rrows[1]),
        "precondition: the reversed model has the arms in the other order"
    );
    let callees = |m: &Model, ri: usize| -> Vec<String> {
        m.workspace.routines[ri]
            .call_sites
            .iter()
            .take(3)
            .map(|cs| cs.callee_text.clone())
            .collect()
    };
    assert_eq!(
        callees(&m, rows[0]),
        callees(&m, rows[1]),
        "precondition: the arms' shared call sites have equal callee text"
    );
    let (a, b) = (swap_surfaces(&m, &id), swap_surfaces(&r, &id));
    for ((name, left), (_, right)) in a.iter().zip(&b) {
        assert!(
            !left.is_empty() && !right.is_empty(),
            "precondition: surface {name} is non-empty in both orders"
        );
        assert_eq!(left, right, "surface {name} differs between row orders");
    }
}

/// T2u (§5.2, R1-01), the projection helper: hand-stated items with an equal
/// sort key and different payloads, in both input orders: the outputs are
/// equal and keep BOTH items. Two call edges `kind|{id}/cs0|to` differing in
/// `resolution`; two uncertainties differing in `interface_name`; two publisher
/// symbols differing in `signature_hash`. Break: dedup by sort key -> one item
/// lost; insertion order -> the two orders differ. Stage B.
#[test]
fn union_keeps_unequal_items_in_one_order() {
    use crate::engine::l4::combined_graph::{CombinedEdge, edge_sort_key};
    use crate::program::model::projection::union_by_value;
    let edge = |resolution: &str| CombinedEdge {
        from: "w".to_string(),
        to: "h".to_string(),
        kind: "direct".to_string(),
        callsite_id: Some("w/cs0".to_string()),
        operation_id: Some("w/op0".to_string()),
        event_id: None,
        subscriber_app_id: None,
        resolution: resolution.to_string(),
        dispatch: None,
    };
    let (e1, e2) = (edge("resolved"), edge("maybe"));
    assert_eq!(
        edge_sort_key(&e1),
        edge_sort_key(&e2),
        "precondition: one sort key"
    );
    let ab = union_by_value(vec![vec![e1.clone()], vec![e2.clone()]], edge_sort_key);
    let ba = union_by_value(vec![vec![e2.clone()], vec![e1.clone()]], edge_sort_key);
    assert_eq!(ab.len(), 2, "both edges kept: {ab:?}");
    assert_eq!(ab, ba, "edges: the union does not depend on row order");

    let unc = |iface: &str| Uncertainty {
        kind: "interface-open-world".to_string(),
        callsite_id: Some("w/cs0".to_string()),
        operation_id: None,
        routine_id: None,
        interface_name: Some(iface.to_string()),
    };
    let key = |u: &Uncertainty| format!("{}|{:?}", u.kind, u.callsite_id);
    let (u1, u2) = (unc("IA"), unc("IB"));
    let ab = union_by_value(vec![vec![u1.clone()], vec![u2.clone()]], key);
    let ba = union_by_value(vec![vec![u2.clone()], vec![u1.clone()]], key);
    assert_eq!(ab.len(), 2, "both uncertainties kept: {ab:?}");
    assert_eq!(
        ab, ba,
        "uncertainties: the union does not depend on row order"
    );

    let m = build(PUB_SRC, "22222222-0000-0000-0000-00000011b004");
    let sym = m
        .events
        .graph
        .events
        .iter()
        .find(|s| s.publisher_routine_id.is_some())
        .expect("precondition: a publisher symbol")
        .clone();
    let mut s2 = sym.clone();
    s2.signature_hash = format!("{}-other", sym.signature_hash);
    let skey = |s: &crate::program::model::events::EventSymbol| s.id.clone();
    let ab = union_by_value(vec![vec![sym.clone()], vec![s2.clone()]], skey);
    let ba = union_by_value(vec![vec![s2.clone()], vec![sym.clone()]], skey);
    assert_eq!(ab.len(), 2, "both symbols kept");
    assert_eq!(
        ab.iter().map(|s| &s.signature_hash).collect::<Vec<_>>(),
        ba.iter().map(|s| &s.signature_hash).collect::<Vec<_>>(),
        "symbols: the union does not depend on row order"
    );
}

/// T2u through production (§5.2, decision 1): two collided rows' call edges at
/// one call-site id and one target, made to differ in `resolution` by
/// assignment on `model.calls` (arm A's edge only), in both row orders, built
/// into the production relation (`ExecRelation::single_app`).
/// `rel.primary().edges_by_from[id]` is equal in both orders and holds both
/// items; today's stable sort keeps emission (row) order. A single-row id with
/// an internal duplicate is passed through untouched (emission order). Break:
/// dedup by sort key -> one item lost; insertion order -> the orders differ.
/// Stage B.
#[test]
fn collided_edge_list_is_order_free_in_production() {
    use crate::program::model::taxonomy::Resolution;
    let guid = "22222222-0000-0000-0000-00000011b005";
    let prepare = |mut m: Model, arm_a_first: bool| -> (Model, String) {
        let (id, _) = collided(&m, "Work");
        let helper = id_named(&m, "Helper1");
        let calls = std::sync::Arc::make_mut(&mut m.calls);
        let at: Vec<usize> = calls
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == id && e.to.as_deref() == Some(helper.as_str()))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 2, "precondition: one Helper1 edge per arm");
        assert_eq!(
            calls.edges[at[0]].callsite_id, calls.edges[at[1]].callsite_id,
            "precondition: one call-site id"
        );
        // The adapter emits edges in row order: arm A's edge is the first in
        // the normal order and the second in the reversed one.
        let a = if arm_a_first { at[0] } else { at[1] };
        assert_eq!(calls.edges[a].resolution, Resolution::Resolved);
        calls.edges[a].resolution = Resolution::Maybe;
        (m, id)
    };
    let (m, id) = prepare(build(SWAP_SRC, guid), true);
    let (r, _) = prepare(build_reversed(SWAP_SRC, guid), false);
    let list = |m: &Model| -> Vec<String> {
        let rel = crate::engine::l4::exec_relation::ExecRelation::single_app(m);
        rel.primary().edges_by_from[&id]
            .iter()
            .map(|e| format!("{e:?}"))
            .collect()
    };
    let (left, right) = (list(&m), list(&r));
    assert_eq!(
        left.iter()
            .filter(|e| e.contains("Maybe") || e.contains("maybe"))
            .count(),
        1,
        "precondition: arm A's edge differs: {left:?}"
    );
    let helper = id_named(&m, "Helper1");
    assert_eq!(
        left.iter()
            .filter(|e| e.contains(&format!("to: \"{helper}\"")))
            .count(),
        2,
        "both arms' unequal Helper1 edges are kept: {left:?}"
    );
    assert_eq!(
        left, right,
        "the collided list does not depend on row order"
    );
}

/// T2u, the uncertainty half through production (§5.2, R3-S2): R1b keeps ONE
/// interface uncertainty per call-site key; for a collided id it is the LAST
/// OWNING row's. Two collided rows whose interface uncertainties at one
/// call-site key differ in `interface_name` (arm A calls through `IA`, arm B
/// through `IB`): `uncertainty_edges_by_from[id]` holds exactly arm B's (the
/// last owning row's) uncertainty in the normal order. Stage B.
#[test]
fn collided_interface_uncertainty_is_the_last_owning_rows() {
    let src = r#"
interface "UI A"
{
    procedure Go();
}

interface "UI B"
{
    procedure Go();
}

codeunit 50819 "UI Work"
{
#if A
    procedure Work()
    var
        I: Interface "UI A";
    begin
        I.Go();
    end;
#else
    procedure Work()
    var
        I: Interface "UI B";
    begin
        I.Go();
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b006");
    let (id, rows) = collided(&m, "Work");
    assert!(
        line(&m, rows[0]) < line(&m, rows[1]),
        "precondition: arm A first"
    );
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let names: Vec<Option<String>> = ctx
        .uncertainty_edges_by_from
        .get(&id)
        .map(|v| {
            v.iter()
                .filter(|u| u.kind == "interface-open-world")
                .map(|u| u.interface_name.clone())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names.len(),
        1,
        "one interface uncertainty per key: {names:?}"
    );
    assert!(
        names[0].as_deref().is_some_and(|n| n.contains("UI B")),
        "the LAST owning row's uncertainty (arm B, `UI B`): {names:?}"
    );
}

// ---------------------------------------------------------------------------
// T1b-T1j (§5.3). Stage B unless stated.
// ---------------------------------------------------------------------------

/// T1b's fixture: the publisher `OnP` declared in two equal arms, a raiser and
/// one subscriber.
const PUB_SRC: &str = r#"
codeunit 50860 "PB Pub"
{
#if A
    [IntegrationEvent(false, false)]
    procedure OnP()
    begin
    end;
#else
    [IntegrationEvent(false, false)]
    procedure OnP()
    begin
    end;
#endif

    procedure Raise()
    begin
        OnP();
    end;
}

codeunit 50861 "PB Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"PB Pub", 'OnP', '', false, false)]
    local procedure Handle()
    begin
    end;
}
"#;

/// T1b (§5.3, production witness for R4b): equal publisher symbols of a
/// collided publisher merge by value: ONE `EventSymbol` (production
/// `program_event_graph`) and ONE event-dispatch edge per subscriber (production
/// relation, `rel.primary().edges_by_from[publisher]`). No per-row publisher
/// facts are asserted (R1-17). Break: push a symbol per row -> two dispatch
/// edges. Stage B.
#[test]
fn collided_publisher_symbols_merge_by_value() {
    let m = build(PUB_SRC, "22222222-0000-0000-0000-00000011b007");
    let (id, _) = collided(&m, "OnP");
    let handle = id_named(&m, "Handle");
    let symbols = m
        .events
        .graph
        .events
        .iter()
        .filter(|s| s.publisher_routine_id.as_deref() == Some(id.as_str()))
        .count();
    assert_eq!(symbols, 1, "one EventSymbol for equal arms");
    let rel = crate::engine::l4::exec_relation::ExecRelation::single_app(&m);
    let dispatch = rel
        .primary()
        .edges_by_from
        .get(&id)
        .map(|v| {
            v.iter()
                .filter(|e| e.kind == "event-dispatch" && e.to == handle)
                .count()
        })
        .unwrap_or(0);
    assert_eq!(dispatch, 1, "one event-dispatch edge per subscriber");
}

/// T1b's variant (§5.3): the publisher's arms differ in a parameter NAME (one
/// id, two different `EventSymbol.parameters`): both symbols are kept, in one
/// order whatever the row order (production event graph, both row orders).
/// Stage B.
#[test]
fn collided_unequal_publisher_symbols_are_both_kept_in_one_order() {
    let src = PUB_SRC
        .replacen("procedure OnP()", "procedure OnP(X: Integer)", 1)
        .replacen("procedure OnP()", "procedure OnP(Y: Integer)", 1)
        .replace("OnP();", "OnP(1);");
    let guid = "22222222-0000-0000-0000-00000011b008";
    let (m, r) = (build(&src, guid), build_reversed(&src, guid));
    let (id, _) = collided(&m, "OnP");
    let syms = |m: &Model| -> Vec<String> {
        m.events
            .graph
            .events
            .iter()
            .filter(|s| s.publisher_routine_id.as_deref() == Some(id.as_str()))
            .map(|s| format!("{:?}", s.parameters))
            .collect()
    };
    let (left, right) = (syms(&m), syms(&r));
    assert!(
        left.len() == 2 && left[0] != left[1],
        "precondition and assertion: both differing symbols kept: {left:?}"
    );
    assert_eq!(
        left, right,
        "the symbols' order does not depend on row order"
    );
}

/// T1c (§5.3): both arms make one interface call through `IC I`, which has two
/// implementers. The Cone payload's interface-dispatch edges from the collided
/// id carry `candidate_count` = 2, one edge per implementer (today the arms'
/// candidates are counted together: 4). Break: union without dedup. Stage B.
#[test]
fn collided_interface_call_counts_each_implementer_once() {
    let src = r#"
interface "IC I"
{
    procedure Go();
}

codeunit 50871 "IC Impl1" implements "IC I"
{
    procedure Go()
    begin
    end;
}

codeunit 50872 "IC Impl2" implements "IC I"
{
    procedure Go()
    begin
    end;
}

codeunit 50870 "IC Work"
{
#if A
    procedure Work()
    var
        I: Interface "IC I";
    begin
        I.Go();
    end;
#else
    procedure Work()
    var
        I: Interface "IC I";
    begin
        I.Go();
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b009");
    let (id, _) = collided(&m, "Work");
    let rel = crate::engine::l4::exec_relation::ExecRelation::single_app(&m);
    let dispatch: Vec<(Option<usize>, String)> = rel
        .cone()
        .iter()
        .filter(|e| e.kind == "interface-dispatch" && e.from == id)
        .map(|e| (e.candidate_count, e.to.clone().unwrap_or_default()))
        .collect();
    assert!(
        !dispatch.is_empty(),
        "precondition: interface dispatch edges"
    );
    assert!(
        dispatch.iter().all(|(c, _)| *c == Some(2)),
        "candidate_count = the number of implementers: {dispatch:?}"
    );
    assert_eq!(dispatch.len(), 2, "one edge per implementer: {dispatch:?}");
}

/// T1d (§5.3): a collided caller (arm A calls `HelperA`, arm B `HelperB`) is
/// ONE entry: the reverse call graph and r3a1's `combined_edges` list each of
/// its edges once (today the k-fold `nodes` walk lists them k times). Break:
/// walk duplicate `nodes`. Stage B.
#[test]
fn collided_id_is_one_reverse_entry() {
    let src = r#"
codeunit 50873 "RV Work"
{
#if A
    procedure Work()
    begin
        HelperA();
    end;
#else
    procedure Work()
    begin
        HelperB();
    end;
#endif

    local procedure HelperA()
    begin
    end;

    local procedure HelperB()
    begin
    end;
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b010");
    let (id, _) = collided(&m, "Work");
    let owned = build_detector_context(&m, substrate::SUMMARIES);
    let ctx = owned.ctx();
    for helper in ["HelperA", "HelperB"] {
        let h = id_named(&m, helper);
        let from_work = ctx
            .reverse_call_graph
            .get(&h)
            .map(|v| v.iter().filter(|e| e.from == id).count())
            .unwrap_or(0);
        assert_eq!(from_work, 1, "reverse_call_graph[{helper}] lists Work once");
    }
    let stable = m.workspace.routines[rows_named(&m, "Work")[0]]
        .stable_routine_id
        .clone();
    let p = m.project_r3a1_combined_graph();
    let from_work = p.combined_edges.iter().filter(|e| e.from == stable).count();
    assert_eq!(from_work, 2, "r3a1 lists each of Work's two edges once");
}

/// T1e (§5.3): a primary routine whose arm A commits (after inserting `SP X`)
/// and whose arm B (the last row) only modifies `SP Z`. The span seeds from
/// the union of commits, one span per call-site id, and the seed row is a row
/// that owns the commit (arm A): its pending physical tables are `SP X`. Break:
/// seed `routine_by_id[id]` (the last row). Stage B.
#[test]
fn collided_commit_seeds_from_both_arms() {
    let src = r#"
table 50874 "SP X"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50875 "SP Z"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50874 "SP Work"
{
#if A
    procedure Work()
    var
        X: Record "SP X";
    begin
        X.Insert();
        Commit();
    end;
#else
    procedure Work()
    var
        Z: Record "SP Z";
    begin
        Z.Modify();
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b011");
    let (id, rows) = collided(&m, "Work");
    let commits = |ri: usize| {
        m.workspace.routines[ri]
            .operation_sites
            .iter()
            .filter(|o| o.kind == "commit")
            .count()
    };
    assert_eq!(
        (commits(rows[0]), commits(rows[1])),
        (1, 0),
        "precondition: arm A commits, arm B (the last row) does not"
    );
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| s.commit_routine_id == id)
        .collect();
    assert_eq!(spans.len(), 1, "one span per commit: {spans:#?}");
    assert_eq!(
        spans[0].pending_physical_tables,
        vec![table_id(&m, "SP X")],
        "the seed row is the committing arm: {:#?}",
        spans[0]
    );
}

/// T1f/T1f-neg's fixture: a `local` event `OnX(var Rec)` raised by the
/// collided `Raise` (one raise per arm, at the same `{id}/cs0`), subscribed to
/// by the collided `Handle`. `ARM_A_REC` is arm A's record declaration.
fn event_temp_src(arm_a_rec: &str) -> String {
    format!(
        r#"
table 50876 "EP T"
{{
    fields {{ field(1; "No."; Code[20]) {{ }} }}
    keys {{ key(PK; "No.") {{ }} }}
}}

codeunit 50876 "EP Pub"
{{
    [IntegrationEvent(false, false)]
    local procedure OnX(var Rec: Record "EP T")
    begin
    end;

#if A
    procedure Raise()
    var
        Rec: {arm_a_rec};
    begin
        OnX(Rec);
    end;
#else
    procedure Raise()
    var
        Rec: Record "EP T" temporary;
    begin
        OnX(Rec);
    end;
#endif
}}

codeunit 50877 "EP Sub"
{{
#if A
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EP Pub", 'OnX', '', false, false)]
    local procedure Handle(var Rec: Record "EP T")
    begin
        Rec.Insert();
    end;
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EP Pub", 'OnX', '', false, false)]
    local procedure Handle(var Rec: Record "EP T")
    begin
        Rec.Modify();
    end;
#endif
}}
"#
    )
}

/// `(row, is Rec known temporary)` for every `Handle` row.
fn handle_param_temp(m: &Model) -> Vec<(usize, bool)> {
    rows_named(m, "Handle")
        .into_iter()
        .map(|ri| {
            let r = &m.workspace.routines[ri];
            let temp = r
                .record_variables
                .iter()
                .filter(|rv| rv.is_parameter)
                .all(|rv| {
                    crate::program::body::features::temp_state_suppresses(Some(&rv.temp_state))
                });
            (ri, temp)
        })
        .collect()
}

/// T1f (§5.3, decision 6): both arms of the collided raiser pass a temporary
/// record, so every applicable raise proves temporary and the rewrite applies
/// to EVERY subscriber row of the id (today: the last row only). Break: rewrite
/// the last row only. Stage B.
#[test]
fn event_param_temp_proves_over_every_raise() {
    let m = build(
        &event_temp_src("Record \"EP T\" temporary"),
        "22222222-0000-0000-0000-00000011b012",
    );
    collided(&m, "Raise");
    collided(&m, "Handle");
    assert!(
        !m.events.graph.edges.is_empty(),
        "precondition: Handle subscribes to OnX"
    );
    let got = handle_param_temp(&m);
    assert!(
        got.iter().any(|&(_, t)| t),
        "precondition: the proof holds (some row is rewritten): {got:?}"
    );
    assert!(
        got.iter().all(|&(_, t)| t),
        "every subscriber row gets the temp rewrite: {got:?}"
    );
}

/// T1f-neg (§5.3, decision 6, R1-03): arm A passes a PHYSICAL record and arm B
/// (the last row) a temporary one, at the same `{id}/cs0`. Not every raise
/// proves temporary, so no subscriber row is rewritten (today: proven, because
/// both raises are read from the last row). Break: inspect raises through the
/// LAST-row string index. Stage B.
#[test]
fn event_param_temp_mixed_raise_is_not_proven() {
    let m = build(
        &event_temp_src("Record \"EP T\""),
        "22222222-0000-0000-0000-00000011b013",
    );
    let (raise, rows) = collided(&m, "Raise");
    collided(&m, "Handle");
    let cs0 = |ri: usize| m.workspace.routines[ri].call_sites[0].id.clone();
    assert_eq!(
        cs0(rows[0]),
        cs0(rows[1]),
        "precondition: both raises sit at one call-site id"
    );
    assert!(cs0(rows[0]).starts_with(&raise), "precondition: {raise}");
    let got = handle_param_temp(&m);
    assert!(
        got.iter().all(|&(_, t)| !t),
        "a mixed physical/temporary raise proves nothing: {got:?}"
    );
}

/// T1h (§5.3): an `OnRun` trigger declared in two arms. Root classification
/// gives one classification per distinct value (today one per row), and a
/// roots config naming the id resolves it (`resolved`, not `ambiguous`).
/// Breaks: keep per-row copies; count rows instead of ids. Stage B.
#[test]
fn collided_routine_classifies_once() {
    use crate::engine::root_classification::compute_root_classifications;
    let src = r#"
codeunit 50878 "RC Run"
{
#if A
    trigger OnRun()
    begin
        Message('a');
    end;
#else
    trigger OnRun()
    begin
        Message('b');
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b014");
    let (id, _) = collided(&m, "OnRun");
    let (ast, _) = compute_root_classifications(&m.workspace, None);
    let mine: Vec<_> = ast.iter().filter(|c| c.routine_id == id).collect();
    assert!(!mine.is_empty(), "precondition: OnRun is a root");
    assert_eq!(
        mine.len(),
        1,
        "one classification per distinct value: {mine:?}"
    );
    let config = format!(
        r#"{{"version":1,"roots":[{{"id":"E1","target":{{"routineId":"{id}"}},"kinds":["onrun-codeunit"]}}]}}"#
    );
    let (with_config, _) =
        compute_root_classifications(&m.workspace, Some(&Ok(config.into_bytes())));
    let mine: Vec<_> = with_config.iter().filter(|c| c.routine_id == id).collect();
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(
        mine[0].resolution_status.as_deref(),
        Some("resolved"),
        "two rows of one id are not ambiguous: {mine:?}"
    );
}

/// T1h's `resolve_target` half (§4.6): T1h's config names the routine by id,
/// which `resolve_target` matches once whatever the rows. Named by
/// `objectId + routineName` instead, both rows of the collided `OnRun` match;
/// they are one id, so the entry resolves (`resolved`, not `ambiguous`).
/// Break: drop the dedup by id -> `ambiguous`. Stage B.
#[test]
fn collided_routine_resolves_by_object_and_name() {
    use crate::engine::root_classification::compute_root_classifications;
    let src = r#"
codeunit 50878 "RC Run"
{
#if A
    trigger OnRun()
    begin
        Message('a');
    end;
#else
    trigger OnRun()
    begin
        Message('b');
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b023");
    let (id, rows) = collided(&m, "OnRun");
    let object = &m.workspace.routines[rows[0]].object_id;
    let config = format!(
        r#"{{"version":1,"roots":[{{"id":"E1","target":{{"objectId":"{object}","routineName":"OnRun"}},"kinds":["onrun-codeunit"]}}]}}"#
    );
    let (with_config, _) =
        compute_root_classifications(&m.workspace, Some(&Ok(config.into_bytes())));
    let mine: Vec<_> = with_config.iter().filter(|c| c.routine_id == id).collect();
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(
        mine[0].config_entry_id.as_deref(),
        Some("E1"),
        "precondition: the config entry applied: {mine:?}"
    );
    assert_eq!(
        mine[0].resolution_status.as_deref(),
        Some("resolved"),
        "two rows of one id are one match, not ambiguous: {mine:?}"
    );
}

/// T1j (§5.3, R2-01): a parameter's role is ONE record per index, the LAST
/// row's, never a union. `Helper`'s arms give its parameter conflicting roles
/// (arm A modifies the record, arm B, the last row, only reads it). The
/// projected `parameter_roles` of `Helper` hold one record for the index, arm
/// B's (`persists_current_record = No`); the composed role of the caller `Pass`
/// (through `compose_roles_only`) and d37's verdict on `Work` (a `Validate`
/// handed to `Helper`) both follow it. Holds today (`base_summaries` is
/// last-wins) and at every stage. Break (at B): union the role records -> two
/// records for the index.
#[test]
fn collided_parameter_role_is_one_record() {
    use crate::engine::l4::effect_lattice::EffectPresence;
    let src = r#"
table 50879 "PR T"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50879 "PR Work"
{
#if A
    procedure Helper(var R: Record "PR T")
    begin
        R.Modify();
    end;
#else
    procedure Helper(var R: Record "PR T")
    begin
        if R.Name = '' then
            exit;
    end;
#endif

    procedure Pass(var R: Record "PR T")
    begin
        Helper(R);
    end;

    procedure Work()
    var
        L: Record "PR T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        Helper(L);
    end;
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b015");
    let (helper, _) = collided(&m, "Helper");
    let pass = id_named(&m, "Pass");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let roles = ctx
        .parameter_roles_by_routine
        .get(&helper)
        .expect("Helper's parameter roles");
    let at0: Vec<_> = roles.iter().filter(|r| r.parameter_index == 0).collect();
    assert_eq!(at0.len(), 1, "one record for the index: {roles:#?}");
    assert_eq!(
        at0[0].persists_current_record,
        EffectPresence::No,
        "the LAST row's (arm B reads only)"
    );
    let pass_role = ctx
        .parameter_roles_by_routine
        .get(&pass)
        .and_then(|v| v.iter().find(|r| r.parameter_index == 0))
        .expect("Pass's role");
    assert_eq!(
        pass_role.persists_current_record,
        EffectPresence::No,
        "the caller's composed role follows that record"
    );
    let out = crate::engine::l5::detectors::d37::detect_d37(&m, &ctx).expect("d37");
    let work = id_named(&m, "Work");
    assert!(
        out.findings
            .iter()
            .any(|f| f.id.starts_with(&format!("d37/{work}/"))),
        "d37 follows the record: Helper does not persist, so Work's Validate is flagged"
    );
}

// ---------------------------------------------------------------------------
// T6 / T6b (§5.6): call-site id collisions, the CDO 5/4 shape. Stage B.
// ---------------------------------------------------------------------------

/// The 5/4 fixture: arm A has 5 call sites, arm B (the last row) 4. `cs0`
/// calls a different routine in each arm (`WriteIt(L)` / `ReadIt(L, 1)`:
/// different parameters, so different bindings; one persists the record, one
/// only reads it); `cs1` is an interface call with two implementers in both
/// arms; `cs2`/`cs3` call `Extra`; `cs4` exists only in arm A and hands the
/// `var` parameter `R` to `WriteIt`.
const FIVE_FOUR_SRC: &str = r#"
table 50880 "CS T"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
}

interface "CS I"
{
    procedure Go(var R: Record "CS T");
}

codeunit 50881 "CS Impl1" implements "CS I"
{
    procedure Go(var R: Record "CS T")
    begin
        R.Modify();
    end;
}

codeunit 50882 "CS Impl2" implements "CS I"
{
    procedure Go(var R: Record "CS T")
    begin
    end;
}

codeunit 50880 "CS Work"
{
    local procedure WriteIt(var R: Record "CS T")
    begin
        R.Modify();
    end;

    local procedure ReadIt(var R: Record "CS T"; N: Integer)
    begin
        if R.Name = '' then
            exit;
    end;

    local procedure Extra()
    begin
    end;

#if A
    procedure Work(var R: Record "CS T")
    var
        I: Interface "CS I";
        L: Record "CS T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        WriteIt(L);
        I.Go(L);
        Extra();
        Extra();
        WriteIt(R);
    end;
#else
    procedure Work(var R: Record "CS T")
    var
        I: Interface "CS I";
        L: Record "CS T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        ReadIt(L, 1);
        I.Go(L);
        Extra();
        Extra();
    end;
#endif

    procedure Solo(var R: Record "CS T")
    var
        I: Interface "CS I";
    begin
        I.Go(R);
    end;
}
"#;

const FIVE_FOUR_GUID: &str = "22222222-0000-0000-0000-00000011b016";

/// The 5/4 fixture's preconditions; returns `(work id, rows)`.
fn five_four(m: &Model) -> (String, Vec<usize>) {
    let (id, rows) = collided(m, "Work");
    let sites = |ri: usize| m.workspace.routines[ri].call_sites.len();
    assert_eq!(
        (sites(rows[0]), sites(rows[1])),
        (5, 4),
        "precondition: arm A has 5 call sites, arm B (the last row) 4"
    );
    (id, rows)
}

/// T6 (§5.6, decision 4, R1-04): the LAST OWNING row per call-site key, every
/// piece of a key from that row. `{id}/cs0`: the resolved edge (R9) AND the
/// bindings (R5) are arm B's (`ReadIt`, two bindings), i.e. from one row.
/// `{id}/cs4`: arm A's, the only owner. `{id}/cs1`: the FIRST targeted
/// implementer inside the chosen site (the within-row policy). Breaks: pick
/// the id's last row without the owning filter -> `cs4` loses its edge; edge
/// FIRST row and bindings LAST row (today's mix) -> the same-row assertion
/// fails; take the last implementer -> `cs1` fails. Stage B.
#[test]
fn collided_call_site_takes_the_last_owning_row() {
    let m = build(FIVE_FOUR_SRC, FIVE_FOUR_GUID);
    let (id, _) = five_four(&m);
    let (write_it, read_it, extra) = (
        id_named(&m, "WriteIt"),
        id_named(&m, "ReadIt"),
        id_named(&m, "Extra"),
    );
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let edge_to = |cs: &str| {
        ctx.resolved_call_edge_by_callsite
            .get(&format!("{id}/{cs}"))
            .and_then(|e| e.to.clone())
    };
    let bindings = |cs: &str| {
        ctx.upgraded_bindings_by_callsite
            .get(&format!("{id}/{cs}"))
            .map(Vec::len)
    };
    assert_eq!(
        (edge_to("cs0"), bindings("cs0")),
        (Some(read_it.clone()), Some(2)),
        "cs0: edge and bindings both arm B's (ReadIt, 2 bindings); WriteIt is {write_it}"
    );
    assert_eq!(
        (edge_to("cs4"), bindings("cs4")),
        (Some(write_it.clone()), Some(1)),
        "cs4: arm A's, its only owner"
    );
    assert_ne!(
        edge_to("cs4"),
        Some(extra),
        "precondition: cs4 is arm A's WriteIt(R)"
    );
    let mut impls: Vec<String> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "Go" && r.body_available)
        .map(|r| r.id.clone())
        .collect();
    impls.sort();
    assert_eq!(impls.len(), 2, "precondition: two implementers");
    assert_eq!(
        edge_to("cs1"),
        Some(impls[0].clone()),
        "cs1: the first targeted implementer of the chosen site"
    );
}

/// T6b's fixture (redesigned at stage B, see the test): `Work(var R; var S)` in
/// two arms. `cs0` hands `R` and `cs1` the validated local `L` to `ReadIt`
/// (reads only) in arm A and to `WriteIt` (persists) in arm B, with plain,
/// unambiguous bindings that differ per arm (two bindings against one); `cs2`
/// exists only in arm A and hands `S` to `WriteIt`. `Single` (one row) hands
/// its `var` parameter to `WriteIt`; `Solo` (one row) to an interface with two
/// implementers. (Arm A's callee is `ReadIt` because its id sorts first, so the
/// unioned edge list's first edge at `cs0`/`cs1` is the WRONG arm's.)
const PICK_SRC: &str = r#"
table 50884 "SP T"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
}

interface "SP I"
{
    procedure Go(var R: Record "SP T");
}

codeunit 50885 "SP Impl1" implements "SP I"
{
    procedure Go(var R: Record "SP T")
    begin
        R.Modify();
    end;
}

codeunit 50886 "SP Impl2" implements "SP I"
{
    procedure Go(var R: Record "SP T")
    begin
    end;
}

codeunit 50884 "SP Work"
{
    local procedure WriteIt(var R: Record "SP T")
    begin
        R.Modify();
    end;

    local procedure ReadIt(var R: Record "SP T"; N: Integer)
    begin
        if R.Name = '' then
            exit;
    end;

#if A
    procedure Work(var R: Record "SP T"; var S: Record "SP T")
    var
        L: Record "SP T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        ReadIt(R, 1);
        ReadIt(L, 1);
        WriteIt(S);
    end;
#else
    procedure Work(var R: Record "SP T"; var S: Record "SP T")
    var
        L: Record "SP T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        WriteIt(R);
        WriteIt(L);
    end;
#endif

    procedure Single(var R: Record "SP T")
    begin
        WriteIt(R);
    end;

    procedure Solo(var R: Record "SP T")
    var
        I: Interface "SP I";
    begin
        I.Go(R);
    end;
}
"#;

/// T6b (§5.6, decision 2, R1-04; fixture REDESIGNED at stage B against the
/// spec's 5/4 form, whose `cs1` interface site has ambiguous bindings, so d37
/// suppressed before `cs0`'s edge mattered and `compose_roles_only` skipped
/// it): the per-site consumers that bypass R9 read one pick, the LAST owning
/// row's edge WITH that row's bindings, over the id's owned keys.
/// - d37 on `Work`: `L` is validated, then handed over at `cs1`. Arm B owns
///   `cs1` last (`WriteIt`, persists), so the `Validate` is suppressed. The
///   unioned list's first `cs1` edge is arm A's `ReadIt`: read with arm B's
///   (R5) bindings, it flags.
/// - `compose_roles_only` on `Work`: `R` (index 0) is handed over at `cs0`,
///   arm B's `WriteIt` -> `R` persists (the first unioned edge, `ReadIt`, would
///   not). `S` (index 1) is handed to `WriteIt` only at `cs2`, arm A's only:
///   the id's owned keys include it -> `S` persists (one row's sites, arm B's,
///   would miss it).
/// - Single-row ids are untouched: `Single` persists through its one edge, and
///   `Solo` keeps its base role (its interface site's bindings are ambiguous),
///   both as before stage B.
///
/// Breaks: read `edges_by_from[id]` directly again -> d37 flags and `R` does not
/// persist; iterate one row's call sites -> `S` does not persist. Stage B.
#[test]
fn per_site_consumers_read_one_pick() {
    use crate::engine::l4::effect_lattice::EffectPresence;
    let m = build(PICK_SRC, "22222222-0000-0000-0000-00000011b021");
    let (id, rows) = collided(&m, "Work");
    let read_it = id_named(&m, "ReadIt");
    let sites = |ri: usize| m.workspace.routines[ri].call_sites.len();
    assert_eq!(
        (sites(rows[0]), sites(rows[1])),
        (3, 2),
        "precondition: arm A owns cs0-cs2, arm B (the last row) cs0-cs1"
    );
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    for cs in ["cs0", "cs1"] {
        let key = format!("{id}/{cs}");
        let first = ctx.graph.edges_by_from[&id]
            .iter()
            .find(|e| e.callsite_id.as_deref() == Some(key.as_str()))
            .map(|e| e.to.clone());
        assert_eq!(
            first.as_ref(),
            Some(&read_it),
            "precondition: the unioned list's first {cs} edge is arm A's ReadIt"
        );
        let b = &ctx.upgraded_bindings_by_callsite[&key];
        assert!(
            b.len() == 1 && b[0].binding_resolution == "resolved" && b[0].callee_parameter_is_var,
            "precondition: {cs}'s bindings are arm B's (WriteIt: one), resolved and var: {b:?}"
        );
    }
    let validates = |ri: usize| {
        m.workspace.routines[ri]
            .record_operations
            .iter()
            .filter(|o| o.op == "Validate")
            .count()
    };
    assert_eq!(
        (validates(rows[0]), validates(rows[1])),
        (1, 1),
        "precondition: each arm validates L once"
    );
    let out = crate::engine::l5::detectors::d37::detect_d37(&m, &ctx).expect("d37");
    assert!(
        !out.findings
            .iter()
            .any(|f| f.id.starts_with(&format!("d37/{id}/"))),
        "d37 follows arm B's WriteIt at cs1 (persists): Work's Validate is suppressed"
    );
    let role = |rid: &str, ix: u32| {
        ctx.parameter_roles_by_routine
            .get(rid)
            .and_then(|v| v.iter().find(|r| r.parameter_index == ix))
            .map(|r| r.persists_current_record)
    };
    assert_eq!(
        role(&id, 0),
        Some(EffectPresence::Yes),
        "compose_roles_only follows arm B's WriteIt at cs0: R persists"
    );
    assert_eq!(
        role(&id, 1),
        Some(EffectPresence::Yes),
        "compose_roles_only visits cs2 (arm A's WriteIt(S)): S persists"
    );
    assert_eq!(
        role(&id_named(&m, "Single"), 0),
        Some(EffectPresence::Yes),
        "single-row Single: its one edge, as before stage B"
    );
    assert_eq!(
        role(&id_named(&m, "Solo"), 0),
        Some(EffectPresence::No),
        "single-row Solo: the interface site's bindings are ambiguous, so its base role, as before stage B"
    );
}

/// The walker's call transfer (`cfg_walker::apply_call`, reached through
/// `compose_roles_only` -> `walk_param`) is a third per-site consumer: it reads
/// the site's callee through `site_pick` too. `Work(var R)` hands `R` at `cs0`
/// to `KeepsR` in arm A and to `DirtiesR` in arm B (the walked row, the LAST
/// owning row). The unioned list's first `cs0` edge is arm A's (`KeepsR`'s id
/// sorts first). `Work`'s path
/// facts for `R` (entry requirement, dirt, loaded fields) equal those of the
/// single-row `RefB`, which makes arm B's call, and differ from `RefA`'s (arm A's
/// call). Break: take the first `cs0` edge of `edges_by_from[id]` again ->
/// `Work` follows `RefA`.
#[test]
fn walker_reads_the_last_owning_rows_callee() {
    let src = r#"
table 50887 "WK T"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50887 "WK Work"
{
    local procedure DirtiesR(var R: Record "WK T")
    begin
        R.Name := 'x';
    end;

    local procedure KeepsR(var R: Record "WK T")
    begin
    end;

#if A
    procedure Work(var R: Record "WK T")
    begin
        KeepsR(R);
    end;
#else
    procedure Work(var R: Record "WK T")
    begin
        DirtiesR(R);
    end;
#endif

    procedure RefA(var R: Record "WK T")
    begin
        KeepsR(R);
    end;

    procedure RefB(var R: Record "WK T")
    begin
        DirtiesR(R);
    end;
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b024");
    let (id, _) = collided(&m, "Work");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let first = ctx.graph.edges_by_from[&id]
        .iter()
        .find(|e| e.callsite_id.as_deref() == Some(format!("{id}/cs0").as_str()))
        .map(|e| e.to.clone());
    assert_eq!(
        first,
        Some(id_named(&m, "KeepsR")),
        "precondition: the unioned list's first cs0 edge is arm A's (KeepsR)"
    );
    let path = |rid: &str| {
        let r = ctx
            .parameter_roles_by_routine
            .get(rid)
            .and_then(|v| v.iter().find(|r| r.parameter_index == 0))
            .expect("a role for R");
        format!(
            "{:?} {:?} {:?} {:?} {:?}",
            r.requires_loaded_at_entry,
            r.required_loaded_fields_at_entry,
            r.mutates_before_load,
            r.dirty_at_exit,
            r.current_loaded_fields_at_exit
        )
    };
    let (a, b) = (path(&id_named(&m, "RefA")), path(&id_named(&m, "RefB")));
    assert_ne!(
        a, b,
        "precondition: the two calls give different path facts"
    );
    assert_eq!(
        path(&id),
        b,
        "the walker follows arm B's DirtiesR (the last owning row), as RefB does"
    );
}

/// d1's seeds (`d1_graph::build_d1_graph`) are spec §3.2 class K: only EQUAL
/// copies collapse, unequal ones stay. `Work`'s two arms loop over a call at
/// the same `cs0`, to `WriteA` in arm A and `WriteB` in arm B: the two seeds are
/// unequal (different rows, loops and callees), so both are kept, each with its
/// own row's loop and callee (the callee-name guard keeps the other arm's edge
/// out). A one-seed-per-key (class P) rule would drop arm A's genuine in-loop
/// chain, the case `gap_g18_transitive_loop::hand_stated_collision_still_fires_a_genuine_inloop_chain`
/// pins. Break: seed each call-site key once, from its last owning row -> arm
/// A's seed is missing.
#[test]
fn d1_seeds_a_collided_call_site_once() {
    let src = r#"
table 50888 "D1 T"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50888 "D1 Work"
{
    local procedure WriteA()
    var
        T: Record "D1 T";
    begin
        T.Insert();
    end;

    local procedure WriteB()
    var
        T: Record "D1 T";
    begin
        T.Modify();
    end;

#if A
    procedure Work()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            WriteA();
    end;
#else
    procedure Work()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            WriteB();
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b025");
    let (id, rows) = collided(&m, "Work");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let mut memo = HashMap::new();
    let (graph, seeds) = crate::engine::l5::d1_graph::build_d1_graph(&ctx, &m.workspace, &mut memo);
    let cs0 = format!("{id}/cs0");
    let mine: Vec<(usize, &str)> = seeds
        .iter()
        .filter(|s| s.callsite.id == cs0)
        .map(|s| {
            let row = rows
                .iter()
                .copied()
                .find(|&ri| std::ptr::eq(s.loop_routine, &m.workspace.routines[ri]))
                .expect("the seed's loop routine is a row of Work");
            (row, graph.node_ids[s.entry as usize])
        })
        .collect();
    let (write_a, write_b) = (id_named(&m, "WriteA"), id_named(&m, "WriteB"));
    assert_eq!(
        mine,
        vec![(rows[0], write_a.as_str()), (rows[1], write_b.as_str())],
        "the arms' unequal seeds are both kept, each with its own row's loop and callee"
    );
}

// ---------------------------------------------------------------------------
// T7 (§5.7), T-guard (decision 5), T9 (R2-02).
// ---------------------------------------------------------------------------

/// T7 (§5.7): a collided caller appears ONCE in `edges_from_order`, at its first
/// appearance, as today. Holds today; stays at every stage. Break (at A/B): no
/// dedup -> the id appears twice.
#[test]
fn from_order_lists_a_collided_caller_once() {
    let m = build(WORK_SRC, "22222222-0000-0000-0000-00000011b017");
    let (id, _) = collided(&m, "Work");
    let rel = crate::engine::l4::exec_relation::ExecRelation::single_app(&m);
    let order = &rel.primary().edges_from_order;
    assert_eq!(
        order.iter().filter(|f| **f == id).count(),
        1,
        "Work once in edges_from_order: {order:?}"
    );
}

/// T-guard's fixture: `Guarded(DoWrite)` declared in two arms whose guards
/// require opposite values (`ARM_A_GUARD` is arm A's condition; arm B, the last
/// row, writes only when `not DoWrite`), called with `false` by `Work`.
/// `Plain(DoWrite)` (one row, `if DoWrite`) called with `false` by `Probe` is
/// the precondition that guards prune at all.
fn guard_src(arm_a: &str) -> String {
    format!(
        r#"
table 50883 "GD X"
{{
    fields {{ field(1; "No."; Code[20]) {{ }} }}
    keys {{ key(PK; "No.") {{ }} }}
}}

codeunit 50883 "GD Work"
{{
#if A
    local procedure Guarded(DoWrite: Boolean)
    var
        X: Record "GD X";
    begin
        {arm_a}
    end;
#else
    local procedure Guarded(DoWrite: Boolean)
    var
        X: Record "GD X";
    begin
        if not DoWrite then
            X.Insert();
    end;
#endif

    procedure Work()
    begin
        Guarded(false);
    end;

    local procedure Plain(DoWrite: Boolean)
    var
        X: Record "GD X";
    begin
        if DoWrite then
            X.Insert();
    end;

    procedure Probe()
    begin
        Plain(false);
    end;
}}
"#
    )
}

/// T-guard (decision 5): guard frames keep today's rule at every stage, the
/// LAST row's frame per id. Case 1: arm A requires `DoWrite`, arm B (last)
/// requires `not DoWrite`; `Work` calls with `false`, so with B's frame the
/// insert runs and `Work`'s cone touches `GD X`. Case 2: arm A is unguarded,
/// arm B (the last row) requires `DoWrite`; with B's frame and `false` the
/// insert is pruned, as today. Holds today by design. Break: union the arms'
/// requirements -> case 1's opposite requirements never run.
#[test]
fn guard_frames_keep_the_last_rows_frame() {
    let check = |arm_a: &str, guid: &str, want_touch: bool| {
        let src = guard_src(arm_a);
        let src = if want_touch {
            src
        } else {
            src.replace(
                "if not DoWrite then\n            X.Insert();",
                "if DoWrite then\n            X.Insert();",
            )
        };
        let m = build(&src, guid);
        collided(&m, "Guarded");
        let owned = build_detector_context(&m, substrate::SUMMARIES);
        let ctx = owned.ctx();
        assert!(
            !ctx.cone_derived.touches_table(&id_named(&m, "Probe")),
            "precondition: a constant false argument prunes a guarded insert"
        );
        assert_eq!(
            ctx.cone_derived.touches_table(&id_named(&m, "Work")),
            want_touch,
            "Work's cone reads the LAST row's frame (arm A: {arm_a})"
        );
    };
    check(
        "if DoWrite then\n            X.Insert();",
        "22222222-0000-0000-0000-00000011b018",
        true,
    );
    check("X.Insert();", "22222222-0000-0000-0000-00000011b019", false);
}

/// T9 (§4.3, R2-02): the production projection checks that the rows of every
/// collision group share one role. A natural fixture reads 0; then a group is
/// forced across two roles by assignment (arm B's `source_unit_id` set to a
/// dependency unit) and `build_detector_context` runs the production projection
/// on that model: in a debug build its `debug_assert!` fires, in a release build
/// the census counter `groups_mixed_role` reads 1. Break: remove the
/// projection's call to the check -> no panic (debug) and 0 (release). Stage A.
#[test]
#[cfg_attr(
    debug_assertions,
    should_panic(expected = "collision group mixes roles")
)]
fn collided_rows_share_one_role() {
    use crate::program::model::projection::groups_mixed_role;
    let mut m = build(WORK_SRC, "22222222-0000-0000-0000-00000011b020");
    let (_, rows) = collided(&m, "Work");
    drop(build_detector_context(&m, substrate::ALL));
    assert_eq!(groups_mixed_role(), 0, "a natural group has one role");
    m.workspace.routines[rows[1]].source_anchor.source_unit_id =
        "dep:dddddddd-0000-0000-0000-00000011b020:src/T.al".to_string();
    drop(build_detector_context(&m, substrate::ALL));
    assert_eq!(groups_mixed_role(), 1, "the mixed group is counted");
}

// ---------------------------------------------------------------------------
// R2b / R2c (§4.6): the other set-valued producers of direct facts. Stage B.
// ---------------------------------------------------------------------------

/// R2c (§4.6, the r3a3 builders): on T1's fixture, the source-only r3a3 base's
/// `direct_full` and `project_r3a3`'s direct facts of the collided `Work` hold
/// BOTH arms' direct facts (A's insert and B's modify). Break: the builder's
/// projection back to a last-row pick -> A's insert is missing.
#[test]
fn r3a3_builders_union_a_collided_id() {
    use crate::engine::l4::capability_cone::{
        build_r3a3_source_only_base_over, direct_facts_for_routine, project_r3a3,
    };
    use crate::engine::l4::combined_graph::build_combined_graph;
    let m = build(WORK_SRC, "22222222-0000-0000-0000-00000011b022");
    let (id, rows) = collided(&m, "Work");
    let facts = |ri: usize| direct_facts_for_routine(&m.workspace.routines[ri], &[]).0;
    let (a, b) = (facts(rows[0]), facts(rows[1]));
    assert!(
        !a.is_empty() && !b.is_empty() && a != b,
        "precondition: the arms' direct facts differ and are both non-empty"
    );
    let base = build_r3a3_source_only_base_over(
        &m,
        build_combined_graph(&m.workspace, &m.calls, &m.events.graph),
    );
    for f in a.iter().chain(&b) {
        assert!(
            base.direct_full[&id].contains(f),
            "the source-only base holds both arms' direct facts: {f:?}"
        );
    }
    let stable = &m.workspace.routines[rows[1]].stable_routine_id;
    let projection = project_r3a3(&m);
    let ops: BTreeSet<&str> = projection
        .summaries
        .iter()
        .find(|s| &s.routine_id == stable)
        .expect("Work in the r3a3 projection")
        .capability_facts_direct
        .iter()
        .map(|f| f.op.as_str())
        .collect();
    assert!(
        ops.contains("insert") && ops.contains("modify"),
        "project_r3a3's direct facts hold A's insert and B's modify: {ops:?}"
    );
}

/// R2b (§4.6, the cross-app base): a source-bearing dependency whose `Work`
/// (no calls, so a fixed leaf) is declared in two arms, arm A inserting and
/// arm B modifying `Dep T`. The base's `leaf_summaries[Work].db_effects` and
/// `direct_full[Work]` hold both arms' writes. Break: the base's projection
/// back to a last-row pick -> arm A's insert is missing.
#[test]
fn cross_app_base_unions_a_collided_dependency_leaf() {
    use std::io::Write;
    const WS: &str = "aaaa11b0-0000-0000-0000-0000000011b0";
    const DEP: &str = "bbbb11b0-0000-0000-0000-0000000011b0";
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{WS}","name":"XWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50200,"to":50299}}],"dependencies":[{{"id":"{DEP}","name":"XDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/Go.al"),
        "codeunit 50201 \"Ws Go\"\n{\n    procedure Go()\n    var\n        D: Codeunit \"Dep Ord\";\n    begin\n        D.Work();\n    end;\n}\n",
    )
    .unwrap();
    let dep_al = r#"table 50172 "Dep T"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50172 "Dep Ord"
{
#if XSYM
    procedure Work()
    var
        T: Record "Dep T";
    begin
        T.Insert();
    end;
#else
    procedure Work()
    var
        T: Record "Dep T";
    begin
        T.Modify();
    end;
#endif
}
"#;
    let manifest = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP}" Name="XDep" Publisher="probe" Version="1.0.0.0" Runtime="13.0" /></Package>"#
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","AppId":"{DEP}","Name":"XDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, text) in [
            ("NavxManifest.xml", manifest.as_str()),
            ("SymbolReference.json", symbols.as_str()),
            ("src/Ord.al", dep_al),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    let pkgs = ws.join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    let mut out = std::fs::File::create(pkgs.join("probe_XDep_1.0.0.0.app")).unwrap();
    out.write_all(&[0u8; 40]).unwrap();
    out.write_all(bytes.get_ref()).unwrap();

    let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
    let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs, "r0")
        .expect("cross-app base");
    let rows: Vec<&ModelRoutine> = base
        .resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "Work")
        .collect();
    assert_eq!(rows.len(), 2, "precondition: Work is two rows");
    let id = rows[0].id.clone();
    assert_eq!(rows[1].id, id, "precondition: one id");
    let leaf_ops: BTreeSet<String> = base
        .leaf_summaries
        .get(&id)
        .expect("precondition: Work is a fixed leaf")
        .db_effects
        .iter()
        .map(|e| e.op.to_ascii_lowercase())
        .collect();
    let direct_ops: BTreeSet<String> = base.direct_full[&id]
        .iter()
        .map(|f| f.op.to_string())
        .collect();
    for (what, ops) in [("leaf db_effects", &leaf_ops), ("direct_full", &direct_ops)] {
        assert!(
            ops.contains("insert") && ops.contains("modify"),
            "the cross-app base's {what} hold both arms' writes: {ops:?}"
        );
    }
}
