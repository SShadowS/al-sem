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
use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

fn build(src: &str, guid: &str) -> Model {
    assemble_and_resolve_inline_program_default(&[("src/T.al".to_string(), src.to_string())], guid)
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

// ---------------------------------------------------------------------------
// TODAY-FORMS (stages C and A; spec §5.3 "through C and A: today's
// behaviour"). The same fixtures as the stage-B tests of the same names, which
// stay held in `held-tests/B/` and replace these at stage B. Each pins the
// compat projection's rule on its input, so swapping one compat rule fails it.
// ---------------------------------------------------------------------------

/// T1 (§5.1), today-form: through C and A the projected direct facts are arm
/// B's (compat LAST, R2a), the summary's direct half is arm B's, the edges keep
/// both callees (R1 is a union) and the solve's `db_effects` hold arm B's
/// direct write and both callees' writes, not arm A's direct write. Break: the
/// R2a compat rule LAST -> FIRST -> "the direct facts are arm B's" fails.
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
    assert_eq!(
        direct, &b_direct,
        "the direct facts are arm B's (the LAST row, R2a)"
    );

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
    assert_eq!(
        summary.capability_facts_direct, b_direct,
        "the summary's direct half is arm B's"
    );
    let bundle = &owned.l4().expect("CORE_SUMMARIES").bundle;
    let ix = bundle.routine_ix(&id).expect("Work in the solve");
    let tables: BTreeSet<String> = bundle
        .db_effects(ix)
        .map(|e| e.table_id.to_string())
        .collect();
    for t in ["CR B", "CR X", "CR Z"] {
        assert!(
            tables.contains(&table_id(&m, t)),
            "the solve's db_effects hold {t}: {tables:?}"
        );
    }
    assert!(
        !tables.contains(&table_id(&m, "CR A")),
        "arm A's direct write is not in the solve (LAST base summary): {tables:?}"
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

/// T1f-neg (§5.3, decision 6, R1-03), today-form: arm A passes a PHYSICAL
/// record and arm B (the last row) a temporary one, at the same `{id}/cs0`.
/// Through C and A the raises are read through the LAST-row string index
/// (#17's today rule), so both read arm B's temporary record: the proof holds
/// and the rewrite applies to the LAST `Handle` row only. Break: read the
/// FIRST row's raise -> the proof fails and no row is rewritten.
#[test]
fn event_param_temp_mixed_raise_is_not_proven() {
    let m = build(
        &event_temp_src("Record \"EP T\""),
        "22222222-0000-0000-0000-00000011b013",
    );
    let (raise, rows) = collided(&m, "Raise");
    let (_, handle) = collided(&m, "Handle");
    let cs0 = |ri: usize| m.workspace.routines[ri].call_sites[0].id.clone();
    assert_eq!(
        cs0(rows[0]),
        cs0(rows[1]),
        "precondition: both raises sit at one call-site id"
    );
    assert!(cs0(rows[0]).starts_with(&raise), "precondition: {raise}");
    assert_eq!(
        handle_param_temp(&m),
        [(handle[0], false), (handle[1], true)],
        "today: proven from the LAST row's raise, the LAST subscriber row rewritten"
    );
}

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

/// T6b (§5.6, decision 2, R1-04), today-form: the per-site consumers that
/// bypass R9 read `site_pick`, whose compat form is today's mix: the FIRST
/// edge with the call-site id in the id's unioned list (arm A's `WriteIt`,
/// which persists) with the LAST row's bindings. So d37 on `Work` follows
/// `WriteIt` and suppresses the `Validate` (no finding), and
/// `compose_roles_only`, handed arm B's row (the solve's LAST row), never
/// visits `cs4`: `Work`'s `var` parameter `R` does not persist. A single-row
/// multi-implementer case (`Solo`) reads the first implementer's role. Break:
/// `site_pick` reads the LAST matching edge (arm B's `ReadIt`) -> d37 flags
/// the `Validate` -> fails.
#[test]
fn per_site_consumers_read_one_pick() {
    use crate::engine::l4::effect_lattice::EffectPresence;
    let m = build(FIVE_FOUR_SRC, FIVE_FOUR_GUID);
    let (id, _) = five_four(&m);
    let write_it = id_named(&m, "WriteIt");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let first_cs0 = ctx.graph.edges_by_from[&id]
        .iter()
        .find(|e| e.callsite_id.as_deref() == Some(format!("{id}/cs0").as_str()))
        .map(|e| e.to.clone());
    assert_eq!(
        first_cs0,
        Some(write_it),
        "precondition: the unioned list's first cs0 edge is arm A's (WriteIt)"
    );
    let out = crate::engine::l5::detectors::d37::detect_d37(&m, &ctx).expect("d37");
    assert!(
        !out.findings
            .iter()
            .any(|f| f.id.starts_with(&format!("d37/{id}/"))),
        "today's d37 follows arm A's WriteIt (persists): Work's Validate is suppressed"
    );
    let role = |rid: &str| {
        ctx.parameter_roles_by_routine
            .get(rid)
            .and_then(|v| v.iter().find(|r| r.parameter_index == 0))
            .map(|r| r.persists_current_record)
    };
    assert_eq!(
        role(&id),
        Some(EffectPresence::No),
        "compose_roles_only is handed arm B's row (no cs4): R does not persist"
    );
    // Measured today (stage C head): an interface site's bindings are marked
    // ambiguous by the adapter (`mark_bindings_ambiguous`), so
    // `compose_roles_only` skips the site whatever edge it picks, and `Solo`
    // keeps its base role (no write in its own body).
    assert_eq!(
        role(&id_named(&m, "Solo")),
        Some(EffectPresence::No),
        "single-row Solo: the interface site's bindings are ambiguous, so its base role"
    );
}

/// `site_pick`'s compat form (spec §4.6, stage A; added because T6b's fixture
/// cannot discriminate it: its `cs1` interface call hands `L` with ambiguous
/// bindings, so d37 suppresses the `Validate` before `cs0`'s edge matters). A
/// collided `Work` whose only call site `cs0` hands the validated `L` to
/// `WriteIt` (persists) in arm A and to `ReadIt` (reads only) in arm B. Today
/// d37 reads the FIRST `cs0` edge of the id's unioned list with the LAST row's
/// bindings, on every row: the `Validate` is flagged exactly when that first
/// edge's callee does not persist. Break: `site_pick` reads the LAST matching
/// edge -> the verdict flips.
#[test]
fn site_pick_compat_reads_the_first_unioned_edge() {
    let src = r#"
table 50884 "SP T"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; "No.") { } }
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
    procedure Work()
    var
        L: Record "SP T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        WriteIt(L);
    end;
#else
    procedure Work()
    var
        L: Record "SP T";
    begin
        L.Get('x');
        L.Validate(Name, 'n');
        ReadIt(L, 1);
    end;
#endif
}
"#;
    let m = build(src, "22222222-0000-0000-0000-00000011b021");
    let (id, _) = collided(&m, "Work");
    let (write_it, read_it) = (id_named(&m, "WriteIt"), id_named(&m, "ReadIt"));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let cs0 = format!("{id}/cs0");
    let targets: Vec<&str> = ctx.graph.edges_by_from[&id]
        .iter()
        .filter(|e| e.callsite_id.as_deref() == Some(cs0.as_str()))
        .map(|e| e.to.as_str())
        .collect();
    assert_eq!(
        targets.len(),
        2,
        "precondition: both arms' cs0 edges are in the unioned list: {targets:?}"
    );
    assert!(
        targets.contains(&write_it.as_str()) && targets.contains(&read_it.as_str()),
        "precondition: one cs0 edge per arm: {targets:?}"
    );
    let flagged = crate::engine::l5::detectors::d37::detect_d37(&m, &ctx)
        .expect("d37")
        .findings
        .iter()
        .any(|f| f.id.starts_with(&format!("d37/{id}/")));
    assert_eq!(
        flagged,
        targets[0] == read_it,
        "d37 follows the FIRST unioned cs0 edge ({}): flagged iff it does not persist",
        targets[0]
    );
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
