//! Task 7 (temp-state-tracking, G5 / RV-7): per-callsite substitution of
//! `ParameterDependent(i)` temp states at the L4 db-effects inheritance fold.
//!
//! A callee effect carrying `ParameterDependent(i)` ("tempness depends on the
//! i-th callee parameter") is, prior to this task, folded into the caller
//! VERBATIM — leaving a CALLEE-frame index `i` that is meaningless in the
//! caller's frame. This task substitutes `PD(i)` per-callsite using the
//! caller's argument binding for callee param `i`:
//!
//!   binding.source_temp_state == Some(Known(true))  -> Known(true)
//!   binding.source_temp_state == Some(Known(false)) -> Known(false)
//!   Some(Unknown) | Some(PD(_)) | None              -> Unknown (conservative)
//!   no callsite (event-dispatch) / non-binding edge  -> Unknown
//!
//! The substituted effect is re-keyed via `effect_key_of`, so identical
//! substitution results dedupe and divergent results (mixed callers) stay
//! distinct. Soundness: substitution only NARROWS symbolic -> binding-derived;
//! all uncertainty becomes Unknown (= fires). Never produce Known(true) except
//! from a binding source that is itself Known(true).
//!
//! Asserts on the composed caller summary's db_effects tempStates, obtained via
//! the same `assemble_and_resolve_workspace_default(...) -> project_r3a2(...)`
//! entry the R3a-2 differential uses, run over an on-disk synthetic workspace.

use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
use al_sem::engine::l4::summary::{PDbEffect, PDbEffectTempState, R3a2Projection, project_r3a2};
use tempfile::TempDir;

const APP_JSON: &str = r#"{
  "id": "dddddddd-7777-7777-7777-777777777777",
  "name": "TS7 Sub Test App",
  "publisher": "TS7",
  "version": "1.0.0.0"
}"#;

/// Write `app.json` + a single `src/main.al` carrying `al_src`, assemble + resolve
/// + run the L4 JACOBI fixed point, project to the R3a-2 comparison surface.
fn project(al_src: &str) -> R3a2Projection {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("app.json"), APP_JSON).expect("write app.json");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src").join("main.al"), al_src).expect("write al");
    let resolved =
        assemble_and_resolve_workspace_default(dir.path()).expect("assemble + resolve workspace");
    project_r3a2(&resolved)
}

/// All `Modify` db_effects across all summaries (the op the fixtures exercise),
/// returned with (via, tempState). Filtered to a single op so the assertions
/// read against the inherited-effect set directly.
fn modify_effects(proj: &R3a2Projection) -> Vec<(String, PDbEffectTempState)> {
    let mut out = Vec::new();
    for s in &proj.summaries {
        for e in &s.db_effects {
            if e.op == "Modify" {
                out.push((e.via.clone(), e.temp_state.clone()));
            }
        }
    }
    out
}

/// Inherited (via != "direct") effects for a given op across all summaries.
fn inherited_effects<'a>(proj: &'a R3a2Projection, op: &str) -> Vec<&'a PDbEffect> {
    let mut out = Vec::new();
    for s in &proj.summaries {
        for e in &s.db_effects {
            if e.op == op && e.via != "direct" {
                out.push(e);
            }
        }
    }
    out
}

fn is_known(ts: &PDbEffectTempState, want: bool) -> bool {
    matches!(ts, PDbEffectTempState::Known { value } if *value == want)
}

// --- (a) PD upgrade chain (LOCAL temp source) -> Known(true) ----------------

#[test]
fn pd_substitutes_to_known_true_for_local_temp_source() {
    // Helper(var Rec) -> Rec.Modify() is PD(0). Caller has a LOCAL temporary and
    // passes it; the inherited effect must resolve Known(true).
    let src = r#"
table 50200 "TS7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50200 "TS7 A"
{
    procedure Helper(var Rec: Record "TS7 Rec")
    begin
        Rec.Modify();
    end;

    procedure CallerTemp()
    var
        Buf: Record "TS7 Rec" temporary;
    begin
        Helper(Buf);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    assert!(
        !inh.is_empty(),
        "expected an inherited Modify effect in CallerTemp; got effects: {:?}",
        modify_effects(&proj)
    );
    assert!(
        inh.iter().any(|e| is_known(&e.temp_state, true)),
        "PD(0) substituted via a temporary local arg must be Known(true); got: {:?}",
        modify_effects(&proj)
    );
    assert!(
        !inh.iter()
            .any(|e| matches!(e.temp_state, PDbEffectTempState::ParameterDependent { .. })),
        "no inherited effect may keep a callee-frame ParameterDependent index; got: {:?}",
        modify_effects(&proj)
    );
}

// --- (b) PD physical (LOCAL physical source) -> Known(false) ----------------

#[test]
fn pd_substitutes_to_known_false_for_local_physical_source() {
    let src = r#"
table 50201 "TS7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50201 "TS7 B"
{
    procedure Helper(var Rec: Record "TS7 Rec")
    begin
        Rec.Modify();
    end;

    procedure CallerPhys()
    var
        Phys: Record "TS7 Rec";
    begin
        Helper(Phys);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    assert!(
        !inh.is_empty(),
        "expected an inherited Modify effect in CallerPhys; got: {:?}",
        modify_effects(&proj)
    );
    assert!(
        inh.iter().any(|e| is_known(&e.temp_state, false)),
        "PD(0) substituted via a physical local arg must be Known(false); got: {:?}",
        modify_effects(&proj)
    );
}

// --- (c) mixed callers -> TWO distinct inherited effects --------------------

#[test]
fn mixed_callers_produce_two_distinct_inherited_effects() {
    // Two callers to the same Helper op: one passes a temp local, one a physical
    // local. The inherited set must contain BOTH Known(true) and Known(false),
    // under DISTINCT effect_keys (the (op,tempState) dedup keeps them separate).
    let src = r#"
table 50202 "TS7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50202 "TS7 C"
{
    procedure Helper(var Rec: Record "TS7 Rec")
    begin
        Rec.Modify();
    end;

    procedure CallerTemp()
    var
        Buf: Record "TS7 Rec" temporary;
    begin
        Helper(Buf);
    end;

    procedure CallerPhys()
    var
        Phys: Record "TS7 Rec";
    begin
        Helper(Phys);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    assert!(
        inh.iter().any(|e| is_known(&e.temp_state, true)),
        "mixed-callers: expected a Known(true) inherited effect; got: {:?}",
        modify_effects(&proj)
    );
    assert!(
        inh.iter().any(|e| is_known(&e.temp_state, false)),
        "mixed-callers: expected a Known(false) inherited effect; got: {:?}",
        modify_effects(&proj)
    );
    // Distinct effect_keys (the tempfrag differs: t vs f).
    let keys: std::collections::BTreeSet<&str> =
        inh.iter().map(|e| e.effect_key.as_str()).collect();
    assert!(
        keys.len() >= 2,
        "mixed-callers: the two substituted effects must have distinct effect_keys; got keys: {keys:?}"
    );
}

// --- (d) event-subscriber PD maps to the publisher's parameter -------------

#[test]
fn event_dispatch_pd_maps_to_the_publishers_param() {
    // The event publisher inherits its subscriber's PD effect across the
    // event-dispatch edge. AL binds a subscriber's parameters to the publisher's
    // BY NAME, so the subscriber's `var Rec` (PD 0) becomes the publisher's own
    // `var Rec` (PD 0), and the raiser `DoWork`, which passes a physical local,
    // decides it: Known(false). (Engine-switch S8: it used to stop at Unknown.)
    let src = r#"
table 50203 "TS7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50203 "TS7 Publisher"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterDoWork(var Rec: Record "TS7 Rec")
    begin
    end;

    procedure DoWork()
    var
        Rec: Record "TS7 Rec";
    begin
        OnAfterDoWork(Rec);
    end;
}

codeunit 50204 "TS7 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"TS7 Publisher", 'OnAfterDoWork', '', false, false)]
    procedure HandleIt(var Rec: Record "TS7 Rec")
    begin
        Rec.Modify();
    end;
}
"#;
    let proj = project(src);
    // An event-dispatch edge folds with via "event-subscriber" (see
    // via_for_edge_kind).
    let dispatch_inherited: Vec<&PDbEffect> = proj
        .summaries
        .iter()
        .flat_map(|s| s.db_effects.iter())
        .filter(|e| e.op == "Modify" && e.via == "event-subscriber")
        .collect();
    assert!(
        !dispatch_inherited.is_empty(),
        "expected an event-dispatch inherited Modify effect; got: {:?}",
        modify_effects(&proj)
    );
    for e in &dispatch_inherited {
        assert!(
            matches!(
                e.temp_state,
                PDbEffectTempState::ParameterDependent { parameter_index: 0 }
            ),
            "event-dispatch PD inheritance must map to the publisher's param 0; got: {:?}",
            e.temp_state
        );
    }
    // The raiser passes a physical record: its inherited Modify is Known(false),
    // and no effect anywhere is Unknown.
    let all: Vec<&PDbEffect> = proj
        .summaries
        .iter()
        .flat_map(|s| s.db_effects.iter())
        .filter(|e| e.op == "Modify")
        .collect();
    assert!(
        all.iter().any(|e| is_known(&e.temp_state, false)),
        "the raiser's inherited Modify must be Known(false); got: {:?}",
        modify_effects(&proj)
    );
    assert!(
        !all.iter()
            .any(|e| matches!(e.temp_state, PDbEffectTempState::Unknown)),
        "no Modify may stay Unknown; got: {:?}",
        modify_effects(&proj)
    );
}

// --- (e) by-value record param is Known(false), not PD, not suppressed ------

#[test]
fn by_value_param_effect_is_known_false_and_not_touched() {
    // P(Rec) BY VALUE doing Rec.Insert(): at L2 a by-value record param is
    // Known(false) (NOT PD). The caller passes a temp local BY VALUE; the
    // inherited effect must stay Known(false) (substitution does not touch a
    // non-PD effect; passing a temp by value does NOT suppress).
    let src = r#"
table 50205 "TS7 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50205 "TS7 E"
{
    procedure P(Rec: Record "TS7 Rec")
    begin
        Rec.Insert();
    end;

    procedure CallerTempByValue()
    var
        Buf: Record "TS7 Rec" temporary;
    begin
        P(Buf);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Insert");
    assert!(
        !inh.is_empty(),
        "expected an inherited Insert effect; got insert effects: {:?}",
        proj.summaries
            .iter()
            .flat_map(|s| s.db_effects.iter())
            .filter(|e| e.op == "Insert")
            .map(|e| (e.via.clone(), e.temp_state.clone()))
            .collect::<Vec<_>>()
    );
    for e in &inh {
        assert!(
            is_known(&e.temp_state, false),
            "by-value temp-passed Insert must remain Known(false) (dangerous direction stays firing); got: {:?}",
            e.temp_state
        );
    }
}
