//! ENG-14 part 1 (#310 remainder; spec `docs/superpowers/specs/2026-11-14-eng14-310-remainder-design.md`
//! §4.2, acceptance A1-A5): a `#if` shape whose arm boundaries the lowerer cannot keep, or that sits in a
//! one-statement slot, must never let an `exit` in one build mark code dead, or let later code dominate
//! the success return, in a build that runs differently.
//!
//! Every case goes through production assembly (`assemble_and_resolve_inline_program_default`). The
//! workspace defines no preprocessor symbol and the engine chooses no configuration (L22), so every arm
//! is a build that may ship. Each routine's comment says which build runs the call named "live".

use crate::program::body::features::PCallSite;
use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;
use crate::program::model::workspace::{Model, ModelRoutine};

const SOURCE: &str = r#"table 50340 "Eng14 T"
{
    fields { field(1; "No."; Integer) { } }
}

codeunit 50340 "Eng14 Split"
{
    // A1: a split procedure body. Under not-X the body is `begin Call5(); end`: Call5 live.
    procedure SplitBodyExit()
#if X
    begin
        exit;
#else
    begin
#endif
        Call5();
    end;

    // A1: the shared tail's own exit still kills: Call6b dead in every build.
    procedure SplitBodyTailExit()
#if X
    var
        I: Integer;
    begin
        I := 1;
#else
    begin
#endif
        exit;
        Call6b();
    end;

    // A1: a split complete body. CallA is dead in the X build; CallB runs in the other.
    procedure SplitComplete()
#if X
    begin
        exit;
        CallA();
    end;
#else
    begin
        CallB();
    end;
#endif

    // A2: under X the tail is `if C then exit; Call7();`: Call7 live.
    procedure SplitBodyGuard(C: Boolean)
#if X
    begin
        if C then
#else
    begin
#endif
        exit;
        Call7();
    end;

    // A3: a split closing. Under X the then-arm closes `end else begin Call8b(); end`: Call8b live.
    procedure SplitClosing(C: Boolean)
    begin
        if C then begin
            exit;
#if X
        end else begin
            Call8b();
        end;
#else
        end;
#endif
        Call8();
    end;

    // A4: a cond-only head split. Under not-X the block is `Work(); Foo();`: Foo live.
    procedure CondOnlyHead(C: Boolean; D: Boolean)
    begin
#if X
        if C then begin
            exit;
#else
        if D then begin
            Work();
#endif
            Foo();
        end;
        Call9();
    end;

    // A4: a split if-else with two heads inside a then-arm: Bar live.
    procedure TwoHeads(Q: Boolean; C: Boolean; D: Boolean)
    begin
        if Q then begin
#if X
            if C then
                exit
            else
#else
            if D then
                Work()
            else
#endif
                exit;
        end else
            exit;
        Bar();
    end;

    // A4: a split case closing. Under X `case I of 1: exit; end; Foo();`: Foo live (I <> 1).
    procedure SplitCaseEnd(I: Integer)
    begin
        case I of
            1:
#if X
                exit;
        end;
        Foo();
#else
                Work();
        end;
        Foo();
#endif
    end;

    // A5: a multi-statement arm in a then-slot. Under X `if C then exit; Foo();`: Foo live;
    // Bar does not dominate the success return.
    procedure SlotThen(C: Boolean)
    begin
        if C then
#if X
            exit;
            Foo();
#endif
        Bar();
    end;

    // A5: the same in an else-slot.
    procedure SlotElse(C: Boolean)
    begin
        if C then
            Work()
        else
#if X
            exit;
            Foo();
#endif
        Bar();
    end;

    // A5: the same in a loop body.
    procedure SlotLoop(C: Boolean)
    begin
        while C do
#if X
            exit;
            Foo();
#endif
        Bar();
    end;

    // A5: the same in a case-branch body.
    procedure SlotCaseBranch(I: Integer)
    begin
        case I of
            1:
#if X
                exit;
                Foo();
#endif
        end;
        Bar();
    end;

    // A5: no arm in the build that takes none, so there `if C then Bar();`: Bar does not
    // dominate the success return.
    procedure SlotNoArm(C: Boolean)
    begin
        if C then
#if not X
            Foo();
#endif
        Bar();
    end;

    // A5: nested slots: Foo live; Bar not dominating.
    procedure SlotNested(C: Boolean; D: Boolean)
    begin
        if D then
            if C then
#if X
                exit;
                Foo();
#endif
        Bar();
    end;

    // A5 (review): a broken slot in a split body's shared tail. In the build that takes no
    // arm Bar is the then-body: Bar does not dominate the success return.
    procedure SplitBodySlot(C: Boolean)
#if X
    begin
#else
    begin
#endif
        if C then
#if not A
            Foo();
#endif
        Bar();
    end;

    // A5: a `with` slot and an `asserterror` slot holding a multi-statement arm.
    procedure SlotWith(var R: Record "Eng14 T")
    begin
        with R do
#if X
            Init();
            Foo();
#endif
        Bar();
    end;

    procedure SlotAssertError()
    begin
        asserterror
#if X
            Error('a');
            Foo();
#endif
        Bar();
    end;

    // A5 (sol): a broken slot inside a statement `#if` arm. Under A without B the `exit`
    // is the then-body, so Bar runs when C is false.
    procedure ArmSlot(C: Boolean)
    begin
#if A
        if C then
#if B
            Foo();
#endif
        exit;
        Bar();
#endif
    end;

    // A5 (sol): the slot's second statement runs whatever Flag is, under X.
    procedure GuardCommit(Flag: Boolean)
    begin
        if Flag then
#if X
            exit;
            Commit();
#endif
        Work();
    end;

    // A5: a well-formed slot (one statement in every build) keeps its tree.
    procedure SlotWellFormed(C: Boolean)
    begin
        if C then
#if X
            Foo();
#else
            Work();
#endif
        Bar();
    end;

    // I2: a call in the split body's guard condition.
    procedure SplitBodyGuardCall()
#if X
    begin
        if Check() then
#else
    begin
#endif
        exit;
        Call7();
    end;

    // A4 (review): an `else` that exists in one build only. With X unset the case falls
    // through when I <> 1, so R may reach Modify without a Get.
    procedure CaseCondElse(var R: Record "Eng14 T"; I: Integer)
    begin
        case I of
            1:
                R.Get();
#if X
            else
                R.Get();
#endif
        end;
        R.Modify();
    end;

    // Not a `#if` shape (CDO measure): a comment after a case `else` makes the lowerer read
    // the else's `statement_block` through its recovery path; that is still a real
    // sequence, so the exit kills Work, as on master.
    procedure ElseComment(I: Integer)
    begin
        case I of
            1:
                Foo();
            else // a comment
                exit;
                Work();
        end;
    end;

    // Control for the dominance helper: Bar runs on every success path.
    procedure Plain()
    begin
        Foo();
        Bar();
    end;

    local procedure Call5() begin end;
    local procedure Call6b() begin end;
    local procedure CallA() begin end;
    local procedure CallB() begin end;
    local procedure Call7() begin end;
    local procedure Call8() begin end;
    local procedure Call8b() begin end;
    local procedure Call9() begin end;
    local procedure Foo() begin end;
    local procedure Bar() begin end;
    local procedure Work() begin end;
    local procedure Check(): Boolean begin end;
}
"#;

fn model() -> Model {
    assemble_and_resolve_inline_program_default(
        &[("Eng14.Codeunit.al".to_string(), SOURCE.to_string())],
        "e1400000-0000-4000-8000-000000000014",
    )
}

fn routine<'a>(m: &'a Model, name: &str) -> &'a ModelRoutine {
    let rows: Vec<_> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: one row");
    rows[0]
}

fn site<'a>(m: &'a Model, name: &str, callee: &str) -> &'a PCallSite {
    let found: Vec<_> = routine(m, name)
        .call_sites
        .iter()
        .filter(|c| c.callee_text == callee)
        .collect();
    assert_eq!(found.len(), 1, "{name}: one call to {callee}");
    found[0]
}

fn is_dead(cs: &PCallSite) -> bool {
    cs.control_context.as_deref() == Some("unreachable")
}

/// Whether the call to `callee` in `name` dominates the success return. The Model does not
/// store operation order (its readers rebuild it from the tree), so this rebuilds it the same
/// way; a site with no order entry fails the test instead of reading `false`.
fn dominates(m: &Model, name: &str, callee: &str) -> bool {
    let r = routine(m, name);
    let cs = site(m, name, callee);
    let order = crate::program::body::operation_order::compute_operation_order(
        r.statement_tree.as_deref(),
        &[],
    );
    order
        .by_callsite
        .get(&cs.id)
        .unwrap_or_else(|| panic!("{name}: no order entry for {callee}"))
        .dominates_success_return
}

/// The lines d20 reports as unreachable in `name`.
fn d20_lines(m: &Model, name: &str) -> Vec<u32> {
    routine(m, name)
        .unreachable_statements
        .iter()
        .map(|u| u.unreachable_anchor.start_line)
        .collect()
}

fn assert_live(m: &Model, name: &str, callee: &str) {
    let cs = site(m, name, callee);
    assert!(
        !is_dead(cs),
        "{name}: {callee} runs in some build, so it is not unreachable: {:?}",
        cs.control_context
    );
    assert!(
        !d20_lines(m, name).contains(&cs.source_anchor.start_line),
        "{name}: no d20 at {callee}"
    );
}

#[test]
fn eng14_a1_split_bodies() {
    let m = model();
    assert_live(&m, "SplitBodyExit", "Call5");
    assert_live(&m, "SplitComplete", "CallB");
    // The guard: a real exit in the shared tail still kills what follows it.
    let tail = site(&m, "SplitBodyTailExit", "Call6b");
    assert!(
        is_dead(tail),
        "SplitBodyTailExit: Call6b is dead in every build"
    );
    assert!(d20_lines(&m, "SplitBodyTailExit").contains(&tail.source_anchor.start_line));
    // And an exit in one complete-body arm kills that arm's own code.
    assert!(
        is_dead(site(&m, "SplitComplete", "CallA")),
        "SplitComplete: CallA dead"
    );
}

#[test]
fn eng14_a2_split_body_guard() {
    let m = model();
    assert_live(&m, "SplitBodyGuard", "Call7");
}

/// A routine whose `#if` arm boundaries were lost fails closed (spec §8.1): no control tree,
/// so no dead mark, d20, guard or dominance comes from it. Its calls keep their sites.
fn assert_untrusted(m: &Model, name: &str) {
    let r = routine(m, name);
    assert!(r.body_available, "{name}: the body is there");
    assert!(
        r.statement_tree.is_none(),
        "{name}: arm boundaries lost, so no control tree"
    );
    assert!(r.unreachable_statements.is_empty(), "{name}: no d20");
    assert!(r.has_branching, "{name}: never read as straight-line");
    assert!(
        !r.call_sites.is_empty(),
        "{name}: its calls keep their sites"
    );
    for cs in &r.call_sites {
        assert!(
            !is_dead(cs),
            "{name}: {} is not unreachable",
            cs.callee_text
        );
    }
}

#[test]
fn eng14_a3_split_closing() {
    let m = model();
    assert_untrusted(&m, "SplitClosing");
}

#[test]
fn eng14_a4_lost_arm_boundaries() {
    let m = model();
    for name in ["CondOnlyHead", "TwoHeads", "SplitCaseEnd"] {
        assert_untrusted(&m, name);
    }
    // A non-`#if` node on the recovery path is a real sequence, not a lost shape.
    assert!(
        is_dead(site(&m, "ElseComment", "Work")),
        "ElseComment: the exit in the else kills Work in every build"
    );
}

#[test]
fn eng14_a5_slots() {
    let m = model();
    // Control: the helper can say yes. With no `#if`, Bar runs on every success path.
    assert!(dominates(&m, "Plain", "Bar"), "Plain: Bar dominates");
    for name in [
        "SlotThen",
        "SlotElse",
        "SlotLoop",
        "SlotCaseBranch",
        "SlotNested",
        "SlotNoArm",
        "SplitBodySlot",
        "SlotWith",
        "SlotAssertError",
        "ArmSlot",
    ] {
        assert_untrusted(&m, name);
    }
    // A well-formed slot (one statement in every build) stays precise: the tree is kept.
    assert!(
        routine(&m, "SlotWellFormed").statement_tree.is_some(),
        "SlotWellFormed: one statement per build keeps the tree"
    );
}

/// sol's part-1 review: `if Flag then #if X exit; Commit(); #endif`. Under X, `Commit`
/// runs whatever `Flag` is, so no guard may put it behind `Flag = true`.
#[test]
fn eng14_a5_a_broken_slot_invents_no_guard() {
    let m = model();
    let r = routine(&m, "GuardCommit");
    assert!(
        crate::engine::l4::param_guard::frame_guards(r, |_, _| true).is_none(),
        "GuardCommit: no guard frame from a slot whose arm boundaries are lost"
    );
}

/// I2 (Fable's part-1 review): a call in a split body's guard condition has a call site.
#[test]
fn eng14_a2_a_guard_condition_calls_have_sites() {
    let m = model();
    let _ = site(&m, "SplitBodyGuardCall", "Check");
}

#[test]
fn eng14_a4_a_conditional_case_else_may_not_run() {
    // `cfg_walker` reads a `case` with an `else` branch (`is_case_else`) as exhaustive: no
    // path skips every branch. An `else` in one `#if` arm only must not make it so.
    let m = model();
    let owned = crate::engine::l5::detector_context::build_detector_context(
        &m,
        crate::engine::l5::registry::substrate::ALL,
    );
    let ctx = owned.ctx();
    let id = &routine(&m, "CaseCondElse").id;
    let role = ctx
        .parameter_roles_by_routine
        .get(id)
        .and_then(|v| v.iter().find(|r| r.parameter_index == 0))
        .expect("a role for R");
    assert_ne!(
        format!("{:?}", role.requires_loaded_at_entry),
        "No",
        "CaseCondElse: with X unset the case has no else, so R may reach Modify unloaded"
    );
}

/// ENG-14 census on CDO (Fable's part-1 review, I1): how many routines lose their control tree,
/// by cause and by app (workspace or dependency). A measurement, not a ratchet: it prints and
/// asserts only that the model was built. Run with
/// `CDO_WS=… cargo test -p al-sem --lib eng14_cdo_untrusted_census -- --nocapture`.
#[test]
fn eng14_cdo_untrusted_census() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let x = crate::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &inputs,
        false,
        crate::program::model::workspace::AssemblyNeeds::BASE,
    )
    .unwrap_or_else(|_| panic!("no cross-app model for {}", ws.display()));
    let m = &x.resolved;
    let primary = m
        .primary_app
        .as_ref()
        .expect("a primary app")
        .app_guid
        .clone();
    // (workspace?, parse_incomplete?) -> rows with a body and no tree
    let mut by: std::collections::BTreeMap<(bool, bool), usize> = Default::default();
    let (mut bodies, mut ws_bodies) = (0usize, 0usize);
    let mut examples: Vec<String> = Vec::new();
    for r in &m.workspace.routines {
        if !r.body_available {
            continue;
        }
        let in_ws = r.app_guid == primary;
        bodies += 1;
        ws_bodies += usize::from(in_ws);
        if r.statement_tree.is_none() {
            *by.entry((in_ws, r.parse_incomplete)).or_default() += 1;
            if examples.len() < 20 {
                examples.push(format!(
                    "{} {}::{} parse_incomplete={}",
                    if in_ws { "ws " } else { "dep" },
                    r.object_id,
                    r.name,
                    r.parse_incomplete
                ));
            }
        }
    }
    println!("ENG14-CENSUS rows with a body: {bodies} (workspace {ws_bodies})");
    for ((in_ws, pi), n) in &by {
        println!(
            "ENG14-CENSUS untrusted {} {}: {n}",
            if *in_ws { "workspace" } else { "dependency" },
            if *pi {
                "parse-incomplete"
            } else {
                "unlowered statement"
            }
        );
    }
    for e in &examples {
        println!("ENG14-CENSUS example {e}");
    }
    assert!(bodies > 0, "the CDO model has bodies");
}

/// A6 sources: two routines whose control tree cannot be trusted. `Broken` has a parse
/// error inside an expression (every statement is still modelled, so only
/// `parse_incomplete` makes it untrusted); `Deep` nests more `if` levels than the lowering
/// depth budget, so its innermost statements are lowered as `Unknown`. Each would claim something if it kept its tree:
/// a dead mark after `exit`, `all_paths_error` from the final `Error`, a `Flag` guard.
fn untrusted_source() -> String {
    let mut deep = String::new();
    for _ in 0..140 {
        deep.push_str("if Flag then begin ");
    }
    deep.push_str("Foo(); ");
    for _ in 0..140 {
        deep.push_str("end; ");
    }
    format!(
        r#"codeunit 50341 "Eng14 Untrusted"
{{
    procedure Broken(Flag: Boolean)
    var
        X: Integer;
    begin
        if Flag then
            exit;
        X := (1 + );
        exit;
        Bar();
        Error('x');
    end;

    procedure Deep(Flag: Boolean)
    begin
        if Flag then
            exit;
        {deep}
        exit;
        Bar();
        Error('x');
    end;

    // No branch in its source: only the untrusted rule makes it read as branching.
    procedure Straight()
    var
        X: Integer;
    begin
        X := (1 + );
        Foo();
    end;

    local procedure Foo() begin end;
    local procedure Bar() begin end;
}}
"#
    )
}

#[test]
fn eng14_a6_untrusted_routines_claim_nothing() {
    let m = assemble_and_resolve_inline_program_default(
        &[("Untrusted.Codeunit.al".to_string(), untrusted_source())],
        "e1400000-0000-4000-8000-000000000015",
    );
    for name in ["Broken", "Deep"] {
        let r = routine(&m, name);
        assert!(r.body_available, "{name}: the body is there");
        assert!(
            r.statement_tree.is_none(),
            "{name}: an untrusted routine has no control tree"
        );
        assert!(
            r.unreachable_statements.is_empty(),
            "{name}: no d20 entry from an untrusted body"
        );
        let bar = site(&m, name, "Bar");
        assert_eq!(
            bar.control_context, None,
            "{name}: Bar has no context, never `unreachable`"
        );
        let summary = crate::engine::return_summary::compute_routine_return_summary(r);
        assert_eq!(
            summary.all_paths_error,
            serde_json::Value::Bool(false),
            "{name}: no all-paths-error claim"
        );
        assert!(
            crate::engine::l4::param_guard::frame_guards(r, |_, _| true).is_none(),
            "{name}: no parameter guard frame"
        );
    }
    // Fable (part-1 review): `has_branching: false` is a straight-line proof for
    // record_types, event_flow and d61; an untrusted body never gives it.
    assert!(
        routine(&m, "Straight").has_branching,
        "Straight: an untrusted routine is never read as straight-line"
    );
}
