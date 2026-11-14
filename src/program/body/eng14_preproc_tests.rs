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

const SOURCE: &str = r#"codeunit 50340 "Eng14 Split"
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

#[test]
fn eng14_a3_split_closing() {
    let m = model();
    assert_live(&m, "SplitClosing", "Call8b");
}

#[test]
fn eng14_a4_unordered_runs() {
    let m = model();
    assert_live(&m, "CondOnlyHead", "Foo");
    assert_live(&m, "TwoHeads", "Bar");
    for cs in routine(&m, "SplitCaseEnd").call_sites.iter() {
        assert!(
            !is_dead(cs),
            "SplitCaseEnd: {} is not unreachable",
            cs.callee_text
        );
    }
    assert!(
        d20_lines(&m, "SplitCaseEnd").is_empty(),
        "SplitCaseEnd: no d20"
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
    ] {
        assert_live(&m, name, "Foo");
        assert!(
            !dominates(&m, name, "Bar"),
            "{name}: Bar is skipped in a build, so it does not dominate the success return"
        );
    }
    assert!(
        !dominates(&m, "SlotNoArm", "Bar"),
        "SlotNoArm: in the build with no arm Bar is the then-body"
    );
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
}
