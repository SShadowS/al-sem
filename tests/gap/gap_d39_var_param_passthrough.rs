//! d39 must not flag a routine that only passes a record through from its own
//! `var` parameter.
//!
//! A `var` parameter hands the record back to the caller's caller. When the
//! helper leaves it Validate-dirty, the forwarder's own parameter is dirty at
//! exit too (the L4 walker composes var-to-var calls), so the dirt is not
//! discarded in the forwarder: d39 judges it one level up, at the routine that
//! owns the record. On CDO the B3 resolution of the obsolete `NewMail` /
//! `CreateAndSendMail` forwarders exposed this as 6 false d39 findings.
//!
//! The forwarder still fires when it discards the dirt itself (reloads the
//! record after the call), because then its own parameter is not dirty at exit.
//! Drives the REGISTERED d39 over inline workspaces.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000d39aa";
const DETECTOR: &str = "d39-record-left-dirty-across-chain";

const TABLE: &str = r#"
table 50391 "D39 Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}
"#;

/// Run the registered d39 and return the caller name each finding blames
/// (the first word of `root_cause`), sorted.
fn blamed(codeunit_body: &str) -> Vec<String> {
    blamed_by(DETECTOR, codeunit_body)
}

/// Same, for any registered detector.
fn blamed_by(detector: &str, codeunit_body: &str) -> Vec<String> {
    let src = format!("{TABLE}\ncodeunit 50390 \"D39 Chain\"\n{{\n{codeunit_body}\n}}\n");
    let files = vec![("src/D39Chain.al".to_string(), src)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == detector)
        .collect();
    assert_eq!(selected.len(), 1, "{detector} must be registered once");
    let findings: Vec<Finding> = run_detectors(&resolved, &selected).findings;
    let mut out: Vec<String> = findings
        .iter()
        .map(|f| f.root_cause.split(' ').next().unwrap_or("").to_string())
        .collect();
    out.sort();
    out
}

const LEAF: &str = r#"
    procedure Leaf(var Cust: Record "D39 Cust")
    begin
        Cust.Validate(Name, 'X');
    end;
"#;

/// The CDO shape: `Forward` only passes its `var` record through; `Outer`
/// owns the record and persists it. Nothing is discarded anywhere.
#[test]
fn passthrough_var_param_is_not_flagged() {
    let body = format!(
        r#"{LEAF}
    procedure Forward(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Forward(Cust);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// Two levels of pass-through; the owner never persists. Only the owner is
/// blamed, not either forwarder.
#[test]
fn two_level_passthrough_blames_only_the_owner() {
    let body = format!(
        r#"{LEAF}
    procedure Mid2(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
    end;

    procedure Mid1(var Cust: Record "D39 Cust")
    begin
        Mid2(Cust);
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid1(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Outer".to_string()]);
}

/// The forwarder also validates the `var` record itself and does not persist
/// it. The dirt still goes back to its caller, so the forwarder is not blamed;
/// an owner that persists clears it, an owner that does not is blamed.
#[test]
fn forwarder_writing_its_var_record_defers_to_the_owner() {
    let body = format!(
        r#"{LEAF}
    procedure Mid(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
        Cust.Validate(Name, 'Y');
    end;

    procedure OuterPersists()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust);
        Cust.Modify();
    end;

    procedure OuterDrops()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["OuterDrops".to_string()]);
}

/// The helper persists on one path only, so it is dirty at exit on the other
/// (the CDO `NewMail` shape: `Insert` inside a loop that may not run). The
/// forwarder's own parameter must inherit that dirt; it used to come out clean,
/// which blamed the forwarder and hid the owner.
#[test]
fn passthrough_of_helper_that_persists_on_one_path_blames_the_owner() {
    let body = r#"
    procedure Leaf(var Cust: Record "D39 Cust"; Flag: Boolean)
    begin
        Cust.Validate(Name, 'X');
        if Flag then
            Cust.Modify();
    end;

    procedure Forward(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust, false);
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Forward(Cust);
    end;
"#;
    assert_eq!(blamed(body), vec!["Outer".to_string()]);
}

/// The forwarder calls the helper inside `exit(...)` (the CDO
/// `CreateAndSendMail` shape). The walker must apply that call before it
/// records the exit state; it used to record the state from before the call.
#[test]
fn passthrough_inside_exit_expression_blames_the_owner() {
    let body = r#"
    procedure Leaf(var Cust: Record "D39 Cust"): Boolean
    begin
        Cust.Validate(Name, 'X');
        exit(true);
    end;

    procedure Forward(var Cust: Record "D39 Cust"): Boolean
    begin
        exit(Leaf(Cust));
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Forward(Cust);
    end;
"#;
    assert_eq!(blamed(body), vec!["Outer".to_string()]);
}

/// A local record really left dirty still fires.
#[test]
fn local_left_dirty_still_fires() {
    let body = format!(
        r#"{LEAF}
    procedure Owner()
    var
        Cust: Record "D39 Cust";
    begin
        Leaf(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Owner".to_string()]);
}

/// The forwarder discards the dirt itself by reloading its `var` record after
/// the call, so its parameter is clean at exit and the caller sees no dirt.
/// The forwarder is where the write is lost: it still fires.
#[test]
fn forwarder_that_reloads_after_the_call_still_fires() {
    let body = format!(
        r#"{LEAF}
    procedure Reloader(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
        Cust.Get(Cust."No.");
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Reloader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Reloader".to_string()]);
}

/// The forwarder reloads after the call and then validates the record itself,
/// so its parameter is still dirty at exit, but the helper's write was lost at
/// the reload. The forwarder is judged at the call site, not by its whole-routine
/// role: it fires, and the owner that persists stays clean.
#[test]
fn forwarder_that_reloads_then_dirties_again_still_fires() {
    let body = format!(
        r#"{LEAF}
    procedure Mid(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
        Cust.Get(Cust."No.");
        Cust.Validate(Name, 'Y');
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Mid".to_string()]);
}

/// A public `var` forwarder with NO caller in the workspace. d39 stays silent:
/// the dirt goes back to the (outside) caller, so "discarded here" would be
/// false whoever that caller is. d40 still fires on the same routine: its
/// finding is a requirement on the caller, and with the owner unjudged it is
/// reported at the forwarder (`Leaf` validates before loading).
#[test]
fn public_var_forwarder_without_caller_d39_silent_d40_fires() {
    let body = format!(
        r#"{LEAF}
    procedure Forward(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
    assert_eq!(
        blamed_by("d40-transitive-load-missing", &body),
        vec!["Forward".to_string()]
    );
}

/// A `local` forwarder with no caller cannot be called from outside: skipped.
#[test]
fn local_forwarder_without_caller_is_skipped() {
    let body = format!(
        r#"{LEAF}
    local procedure Forward(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// The forwarder reloads through a `var` helper after the call, then dirties the
/// record again itself: the helper reload drops `Leaf`'s write like an own `Get`.
#[test]
fn forwarder_that_reloads_through_a_helper_still_fires() {
    let body = format!(
        r#"{LEAF}
    procedure Loader(var Cust: Record "D39 Cust")
    begin
        Cust.Get('C1');
    end;

    procedure Mid(var Cust: Record "D39 Cust")
    begin
        Leaf(Cust);
        Loader(Cust);
        Cust.Validate(Name, 'Y');
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Mid".to_string()]);
}

const LOADER: &str = r#"
    procedure Loader(var Cust: Record "D39 Cust")
    begin
        Cust.Get('C1');
    end;
"#;

/// The CDO `CreateAndSendMail` shape: the call sits in one arm of an `if` and the
/// helper that loads the record sits in the other arm. Only one of them runs, so
/// the helper does not drop the call's dirt: the dirt goes back to the caller and
/// the forwarder is not blamed. The owner that persists stays clean.
#[test]
fn reload_in_the_other_branch_arm_does_not_count() {
    let body = format!(
        r#"{LEAF}{LOADER}
    procedure Mid(var Cust: Record "D39 Cust"; Flag: Boolean)
    begin
        if Flag then
            Leaf(Cust)
        else
            Loader(Cust);
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust, true);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// Same, with `case` arms and an own `Get`.
#[test]
fn reload_in_another_case_arm_does_not_count() {
    let body = format!(
        r#"{LEAF}
    procedure Mid(var Cust: Record "D39 Cust"; Mode: Integer)
    begin
        case Mode of
            1:
                Leaf(Cust);
            2:
                Cust.Get('C1');
        end;
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust, 1);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// The branch sits inside a loop: the reload in the other arm can run on the
/// next iteration, after the call, so it still counts and the forwarder fires.
#[test]
fn reload_in_the_other_arm_inside_a_loop_still_counts() {
    let body = format!(
        r#"{LEAF}{LOADER}
    procedure Mid(var Cust: Record "D39 Cust"; Flag: Boolean)
    var
        i: Integer;
    begin
        for i := 1 to 2 do
            if Flag then
                Leaf(Cust)
            else
                Loader(Cust);
    end;

    procedure Outer()
    var
        Cust: Record "D39 Cust";
    begin
        Mid(Cust, true);
        Cust.Modify();
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Mid".to_string()]);
}
