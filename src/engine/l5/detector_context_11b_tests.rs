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
