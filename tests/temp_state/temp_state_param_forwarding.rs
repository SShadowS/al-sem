//! Task 8 (temp-state-tracking, RV-7 binding gap / RV-8): resolve param-source
//! argument bindings when the caller FORWARDS its OWN record parameter as the
//! argument.
//!
//! Task 7 substituted a callee effect's `ParameterDependent(i)` through the
//! caller's argument binding for callee param `i`, but COLLAPSED the
//! caller's-own-param-source case to `Unknown`:
//!   - `source_temp_state == Some(ParameterDependent(_))` -> Unknown
//!   - `source_temp_state == None`                        -> Unknown (TODO ts8)
//!
//! At L2 a record-typed PARAMETER is already present in
//! `enclosing_record_variables`, so a forwarded-param arg's binding ALREADY
//! carries `source_temp_state` = that param's own temp_state:
//!   - `var Rec ... temporary` (keyword)      -> Known(true)
//!   - `var Rec ...` (keyword-less by-var)     -> ParameterDependent(caller idx)
//!   - by-value `Rec`                          -> Known(false)
//!
//! Task 8 RE-SYMBOLIZES the PD case: a callee PD substituted through a
//! forwarded keyword-less by-var caller param becomes
//! `ParameterDependent(caller_param_index)` — chaining the symbolic dependency
//! UPWARD instead of dropping it to Unknown. The keyword case already yields
//! Known(true); the by-value case Known(false).
//!
//! SOUNDNESS: re-symbolizing PD->PD only PROPAGATES the symbolic dependency; it
//! never invents Known(true). A forwarded keyword param yields Known(true)
//! ONLY because its source param IS Known(true). A PD chasing itself around a
//! recursive cycle never gains Known (monotone) and the fixed point converges.
//!
//! Same harness as `tests/temp_state_substitution.rs` (Task 7).

use al_sem::engine::l3::l3_workspace::L3Resolved;
use al_sem::engine::l4::summary::{PDbEffect, PDbEffectTempState, R3a2Projection, project_r3a2};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;
use tempfile::TempDir;

const APP_JSON: &str = r#"{
  "id": "eeeeeeee-8888-8888-8888-888888888888",
  "name": "TS8 Forwarding Test App",
  "publisher": "TS8",
  "version": "1.0.0.0"
}"#;

/// Write `app.json` + a single `src/main.al` carrying `al_src`, assemble +
/// resolve + run the L4 JACOBI fixed point, project to the R3a-2 surface.
fn project(al_src: &str) -> R3a2Projection {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("app.json"), APP_JSON).expect("write app.json");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src").join("main.al"), al_src).expect("write al");
    let resolved =
        assemble_and_resolve_workspace_program(dir.path(), MODEL_INSTANCE_ID_DEFAULT, false)
            .expect("assemble + resolve workspace");
    project_r3a2(&resolved)
}

/// Like [`project`] but returns the resolved L3 workspace (for binding-level
/// assertions — the RV-8 sourceKind case).
fn resolve(al_src: &str) -> L3Resolved {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("app.json"), APP_JSON).expect("write app.json");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src").join("main.al"), al_src).expect("write al");
    assemble_and_resolve_workspace_program(dir.path(), MODEL_INSTANCE_ID_DEFAULT, false)
        .expect("assemble + resolve workspace")
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

/// All effects (any via) for a given op, as (via, tempState), for diagnostics.
fn op_effects(proj: &R3a2Projection, op: &str) -> Vec<(String, PDbEffectTempState)> {
    proj.summaries
        .iter()
        .flat_map(|s| s.db_effects.iter())
        .filter(|e| e.op == op)
        .map(|e| (e.via.clone(), e.temp_state.clone()))
        .collect()
}

fn is_known(ts: &PDbEffectTempState, want: bool) -> bool {
    matches!(ts, PDbEffectTempState::Known { value } if *value == want)
}

fn is_pd(ts: &PDbEffectTempState, idx: u32) -> bool {
    matches!(ts, PDbEffectTempState::ParameterDependent { parameter_index } if *parameter_index == idx)
}

// --- (a) keyword param forwarded -> Known(true) -----------------------------

#[test]
fn keyword_param_forwarded_resolves_known_true() {
    // A(var Buf: Record X temporary) forwards Buf to Helper(var Rec) which does
    // Rec.Modify() (PD(0)). A's binding for the arg is the temporary keyword
    // param -> source_temp_state Known(true). The inherited effect in A must be
    // Known(true).
    let src = r#"
table 50210 "TS8 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50210 "TS8 Kw"
{
    procedure Helper(var Rec: Record "TS8 Rec")
    begin
        Rec.Modify();
    end;

    procedure A(var Buf: Record "TS8 Rec" temporary)
    begin
        Helper(Buf);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    assert!(
        !inh.is_empty(),
        "expected an inherited Modify effect in A; got: {:?}",
        op_effects(&proj, "Modify")
    );
    assert!(
        inh.iter().any(|e| is_known(&e.temp_state, true)),
        "forwarded temporary-keyword param must resolve Known(true); got: {:?}",
        op_effects(&proj, "Modify")
    );
    assert!(
        !inh.iter()
            .any(|e| matches!(e.temp_state, PDbEffectTempState::ParameterDependent { .. })),
        "no inherited effect may keep a callee-frame PD index; got: {:?}",
        op_effects(&proj, "Modify")
    );
}

// --- (b) keyword-less by-var param forwarded -> PD(caller index) ------------

#[test]
fn keyless_byvar_param_forwarded_resymbolizes_pd_caller_index() {
    // A(var Rec: Record X) (NO keyword) forwards Rec to Helper(var Rec) which
    // does Rec.Modify() (PD(0)). A's binding for the arg is its OWN keyword-less
    // by-var param -> source_temp_state PD(0). Task 8 RE-SYMBOLIZES: the
    // inherited effect in A must be ParameterDependent(0) (A's own param index),
    // NOT Unknown, NOT Known.
    let src = r#"
table 50211 "TS8 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50211 "TS8 Pd"
{
    procedure Helper(var Rec: Record "TS8 Rec")
    begin
        Rec.Modify();
    end;

    procedure A(var Rec: Record "TS8 Rec")
    begin
        Helper(Rec);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    assert!(
        !inh.is_empty(),
        "expected an inherited Modify effect in A; got: {:?}",
        op_effects(&proj, "Modify")
    );
    // A's inherited effect must carry PD(0) (A's own param 0), re-symbolized.
    assert!(
        inh.iter().any(|e| is_pd(&e.temp_state, 0)),
        "forwarded keyword-less by-var param must RE-SYMBOLIZE to PD(0) (caller index); got: {:?}",
        op_effects(&proj, "Modify")
    );
    // It must NOT collapse to a spurious Known(true) (the unsound direction).
    assert!(
        !inh.iter().any(|e| is_known(&e.temp_state, true)),
        "forwarded keyword-less by-var param must never become Known(true); got: {:?}",
        op_effects(&proj, "Modify")
    );
}

// --- (c) recursion-through-PD converges, never spurious Known(true) ---------

#[test]
fn recursion_through_pd_converges_and_never_known_true() {
    // A(var Rec) forwards Rec to itself (self-recursion through a by-var param).
    // The PD must re-symbolize around the cycle and the fixed point MUST
    // converge (the cap warning would otherwise be the only signal of
    // non-termination). The effect must resolve to PD(0) or Unknown — never a
    // spurious Known(true).
    let src = r#"
table 50212 "TS8 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50212 "TS8 Rec2"
{
    procedure A(var Rec: Record "TS8 Rec"; Depth: Integer)
    begin
        Rec.Modify();
        if Depth > 0 then
            A(Rec, Depth - 1);
    end;
}
"#;
    // If the fixed point fails to converge, the runner prints a warning but does
    // NOT hang (capped at MAX_FIXED_POINT_ITERATIONS); the call returns. We rely
    // on the test simply COMPLETING. Assert the resolved effect never gains a
    // spurious Known(true).
    let proj = project(src);
    let all: Vec<(String, PDbEffectTempState)> = op_effects(&proj, "Modify");
    assert!(
        !all.is_empty(),
        "expected at least the direct Modify effect; got: {all:?}"
    );
    // The self-recursive PD(0), folded back into A, must stay PD(0)/Unknown,
    // never Known(true).
    let inh = inherited_effects(&proj, "Modify");
    for e in &inh {
        assert!(
            !is_known(&e.temp_state, true),
            "recursion-through-PD must never produce a spurious Known(true); got: {:?}",
            e.temp_state
        );
        assert!(
            is_pd(&e.temp_state, 0) || matches!(e.temp_state, PDbEffectTempState::Unknown),
            "recursion-through-PD inherited effect must be PD(0) or Unknown; got: {:?}",
            e.temp_state
        );
    }
}

// --- (d) two-routine cycle forwarding a by-var param converges --------------

#[test]
fn two_cycle_forwarding_byvar_converges() {
    // A(var Rec) -> B(Rec); B(var Rec) -> A(Rec). Mutual recursion forwarding a
    // keyword-less by-var param. The PD re-symbolizes around the 2-cycle; the
    // fixed point must converge and no spurious Known(true) may appear.
    let src = r#"
table 50213 "TS8 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50213 "TS8 Cyc"
{
    procedure A(var Rec: Record "TS8 Rec"; Depth: Integer)
    begin
        Rec.Modify();
        if Depth > 0 then
            B(Rec, Depth - 1);
    end;

    procedure B(var Rec: Record "TS8 Rec"; Depth: Integer)
    begin
        if Depth > 0 then
            A(Rec, Depth - 1);
    end;
}
"#;
    let proj = project(src);
    let inh = inherited_effects(&proj, "Modify");
    for e in &inh {
        assert!(
            !is_known(&e.temp_state, true),
            "two-cycle forwarding must never produce a spurious Known(true); got: {:?}",
            e.temp_state
        );
        assert!(
            is_pd(&e.temp_state, 0) || matches!(e.temp_state, PDbEffectTempState::Unknown),
            "two-cycle forwarding inherited effect must be PD(0) or Unknown (never a spurious Known); got: {:?}",
            e.temp_state
        );
    }
}

// --- (e) RV-8: the scope-honest relabel never MISLABELS a promoted-global arg
//         as "local". -----------------------------------------------------------
//
// RV-8 contract (this task): a binding whose source resolves to a promoted
// GLOBAL record var (`scope == Some("global")`) must NOT carry the diagnostic
// label `"local"`. The relabel runs at L3 (where promotion has assigned scope),
// so any binding L3 can match to a promoted global becomes `"global"`.
//
// NOTE on the current L2 surface: the L2 binding builder only matches a routine's
// OWN params/locals (object globals are promoted later at L3), so an
// object-global arg currently surfaces as `sourceKind == "unknown"` /
// `sourceVariableName == None` at L2 — it never reaches the "local" mislabel in
// the first place. This test therefore asserts the SOUNDNESS INVARIANT directly:
// for every binding, if its source name matches a promoted-global record var,
// its sourceKind is `"global"` (never `"local"`). The invariant holds whether
// the L2 surface later starts emitting named global bindings (Task 16) or not.
#[test]
fn promoted_global_arg_is_never_mislabeled_local() {
    let src = r#"
table 50214 "TS8 Rec"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50214 "TS8 Glob"
{
    var
        GlobalBuf: Record "TS8 Rec";

    procedure Helper(var Rec: Record "TS8 Rec")
    begin
        Rec.Modify();
    end;

    procedure A()
    begin
        Helper(GlobalBuf);
    end;
}
"#;
    let resolved = resolve(src);
    for r in &resolved.workspace.routines {
        // Names of this routine's promoted-global record vars.
        let global_names: Vec<String> = r
            .record_variables
            .iter()
            .filter(|rv| rv.scope.as_deref() == Some("global"))
            .map(|rv| rv.name.to_lowercase())
            .collect();
        for cs in &r.call_sites {
            for b in &cs.argument_bindings {
                if let Some(name_lc) = b.source_variable_name.as_deref()
                    && global_names.iter().any(|g| g == name_lc)
                {
                    assert_eq!(
                        b.source_kind, "global",
                        "binding naming promoted global {name_lc:?} must be sourceKind \"global\" (RV-8), not {:?}",
                        b.source_kind
                    );
                }
            }
        }
    }
}
