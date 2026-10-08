//! d40 must not blame a caller whose record is loaded by a helper, nor a routine
//! that only forwards its own parameter.
//!
//! On CDO the B3 resolution exposed both shapes: `EMailTemplateLine.GetV2Line(Line)`
//! loads `Line` with `Line.Get(...)` before the call d40 flagged, and obsolete
//! forwarders pass their own `var Line` parameter to the next overload. The load
//! is the caller's job there, and the L4 walker composes the callee's entry
//! requirement into the forwarder's parameter role, so d40 judges the forwarder's
//! callers instead. Drives the REGISTERED d40 over inline workspaces.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000d40aa";
const DETECTOR: &str = "d40-transitive-load-missing";

const TABLE: &str = r#"
table 50401 "D40 Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}
"#;

/// Run the registered d40 and return the routine each finding blames (the first
/// word of `root_cause`), sorted.
fn blamed(codeunit_body: &str) -> Vec<String> {
    let src = format!("{TABLE}\ncodeunit 50400 \"D40 Chain\"\n{{\n{codeunit_body}\n}}\n");
    let files = vec![("src/D40Chain.al".to_string(), src)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(selected.len(), 1, "{DETECTOR} must be registered once");
    let findings: Vec<Finding> = run_detectors(&resolved, &selected).findings;
    let mut out: Vec<String> = findings
        .iter()
        .map(|f| f.root_cause.split(' ').next().unwrap_or("").to_string())
        .collect();
    out.sort();
    out
}

/// `Reader` reads a field of the record without loading it.
const READER: &str = r#"
    procedure Reader(var Cust: Record "D40 Cust"): Text
    begin
        exit(Cust.Name);
    end;

    procedure Loader(var Cust: Record "D40 Cust"): Boolean
    begin
        exit(Cust.Get('C1'));
    end;
"#;

/// The CDO `GetV2Line` shape: a helper loads the record through its `var`
/// parameter before the call.
#[test]
fn record_loaded_by_helper_is_not_flagged() {
    let body = format!(
        r#"{READER}
    procedure Caller()
    var
        Cust: Record "D40 Cust";
    begin
        if not Loader(Cust) then
            exit;
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// The control: nothing loads the record, so the caller is blamed.
#[test]
fn record_never_loaded_still_fires() {
    let body = format!(
        r#"{READER}
    procedure Caller()
    var
        Cust: Record "D40 Cust";
    begin
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Caller".to_string()]);
}

/// The helper loads a DIFFERENT variable, so `Cust` is still unloaded.
#[test]
fn helper_loading_another_variable_still_fires() {
    let body = format!(
        r#"{READER}
    procedure Caller()
    var
        Cust: Record "D40 Cust";
        Other: Record "D40 Cust";
    begin
        Loader(Other);
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Caller".to_string()]);
}

/// A helper call AFTER the read does not count as a load before it.
#[test]
fn helper_load_after_the_call_still_fires() {
    let body = format!(
        r#"{READER}
    procedure Caller()
    var
        Cust: Record "D40 Cust";
    begin
        Reader(Cust);
        Loader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Caller".to_string()]);
}

/// A by-value helper parameter loads only the helper's copy.
#[test]
fn by_value_helper_does_not_load_the_caller_record() {
    let body = format!(
        r#"{READER}
    procedure ValueLoader(Cust: Record "D40 Cust"): Boolean
    begin
        exit(Cust.Get('C1'));
    end;

    procedure Caller()
    var
        Cust: Record "D40 Cust";
    begin
        ValueLoader(Cust);
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Caller".to_string()]);
}

/// The helper loads on one path only. d40's caller-side check is lenient: an
/// own `Get` on one branch already counts as loaded (pinned by the sibling
/// below), so a helper that loads on one path counts the same way.
#[test]
fn helper_loading_on_one_path_counts_like_an_own_branch_load() {
    let helper = format!(
        r#"{READER}
    procedure MaybeLoader(var Cust: Record "D40 Cust"; Flag: Boolean)
    begin
        if Flag then
            Cust.Get('C1');
    end;

    procedure Caller(Flag: Boolean)
    var
        Cust: Record "D40 Cust";
    begin
        MaybeLoader(Cust, Flag);
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&helper), Vec::<String>::new());

    let own_branch = format!(
        r#"{READER}
    procedure Caller(Flag: Boolean)
    var
        Cust: Record "D40 Cust";
    begin
        if Flag then
            Cust.Get('C1');
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&own_branch), Vec::<String>::new());
}

/// The CDO forwarder shape: `Forward` passes its own `var` parameter on. The
/// owner that loads is clean; the owner that does not is blamed, not `Forward`.
#[test]
fn forwarder_of_its_var_parameter_defers_to_the_owner() {
    let body = format!(
        r#"{READER}
    procedure Forward(var Cust: Record "D40 Cust"): Text
    begin
        exit(Reader(Cust));
    end;

    procedure OwnerLoads()
    var
        Cust: Record "D40 Cust";
    begin
        Cust.Get('C1');
        Forward(Cust);
    end;

    procedure OwnerSkips()
    var
        Cust: Record "D40 Cust";
    begin
        Forward(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["OwnerSkips".to_string()]);
}

/// Same for a by-value parameter: the copy carries the caller's loaded state.
#[test]
fn forwarder_of_its_by_value_parameter_defers_to_the_owner() {
    let body = format!(
        r#"{READER}
    procedure Forward(Cust: Record "D40 Cust"): Text
    begin
        exit(Reader(Cust));
    end;

    procedure OwnerSkips()
    var
        Cust: Record "D40 Cust";
    begin
        Forward(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["OwnerSkips".to_string()]);
}

/// Two levels of helper: `Mid` hands its `var` record to `Loader`. L4 composes
/// the load into `Mid`'s role, so the caller of `Mid` counts as loaded.
#[test]
fn two_level_helper_load_is_not_flagged() {
    let body = format!(
        r#"{READER}
    procedure Mid(var Cust: Record "D40 Cust")
    begin
        Loader(Cust);
    end;

    procedure Caller()
    var
        Cust: Record "D40 Cust";
    begin
        Mid(Cust);
        Reader(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// A public forwarder with NO caller in the workspace: its owner is outside, so
/// nobody would be judged one level up. The finding stays on the forwarder.
#[test]
fn public_forwarder_without_workspace_caller_still_fires() {
    let body = format!(
        r#"{READER}
    procedure Forward(var Cust: Record "D40 Cust"): Text
    begin
        exit(Reader(Cust));
    end;
"#
    );
    assert_eq!(blamed(&body), vec!["Forward".to_string()]);
}

/// A `local` forwarder with no caller cannot be called from outside: nothing to
/// judge, nothing to blame.
#[test]
fn local_forwarder_without_caller_is_skipped() {
    let body = format!(
        r#"{READER}
    local procedure Forward(var Cust: Record "D40 Cust"): Text
    begin
        exit(Reader(Cust));
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// A by-value parameter loaded by a `var` helper inside the routine: the walker
/// must see the local copy loaded, so `F` does not require it loaded at entry
/// and its caller is not blamed.
#[test]
fn by_value_parameter_loaded_by_var_helper_needs_no_load_at_entry() {
    let body = format!(
        r#"{READER}
    procedure F(Cust: Record "D40 Cust"): Text
    begin
        Loader(Cust);
        exit(Reader(Cust));
    end;

    procedure Owner()
    var
        Cust: Record "D40 Cust";
    begin
        F(Cust);
    end;
"#
    );
    assert_eq!(blamed(&body), Vec::<String>::new());
}

/// A forwarder passing its own `var` record to a callee with no role for that
/// parameter (a `var Variant`) takes L4's opaque branch: every composed fact,
/// `loads_from_db_param` / `initialises_param` included, becomes Unknown (not
/// Yes, so d40 does not count it as a load). Dormant on CDO (no such site in the
/// digest graph: zero `copies_into_param: Unknown` before and after).
#[test]
fn forwarding_to_an_opaque_callee_makes_load_facts_unknown() {
    use al_sem::engine::l4::effect_lattice::EffectPresence;
    use al_sem::engine::l5::detector_context::build_detector_context;
    use al_sem::engine::l5::registry::substrate;

    let src = format!(
        r#"{TABLE}
codeunit 50402 "D40 Opaque"
{{
    procedure TakesVariant(var V: Variant)
    begin
    end;

    procedure Forward(var Cust: Record "D40 Cust")
    begin
        TakesVariant(Cust);
    end;
}}
"#
    );
    let files = vec![("src/D40Opaque.al".to_string(), src)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    let ctx = build_detector_context(&resolved, substrate::ALL);
    let forward = resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Forward")
        .expect("Forward routine");
    let role = &ctx.parameter_roles_by_routine[&forward.id][0];
    assert_eq!(role.loads_from_db_param, EffectPresence::Unknown);
    assert_eq!(role.initialises_param, EffectPresence::Unknown);
    assert!(!role.puts_in_loaded_state());
}
