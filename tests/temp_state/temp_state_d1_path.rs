//! Task 10 (temp-state-tracking, Component 3 / RV-6): d1 consumes the PATH-RESOLVED
//! temp state + the worst-severity merge-tie with a dual-verdict note.
//!
//! These drive the REAL d1 detector over inline AL workspaces (via
//! `assemble_and_resolve_inline_program_default` + `run_detectors`) and inspect the emitted
//! findings. The unit-level edge-kind allowlist guard (case c) lives in
//! `tests/temp_state_path.rs` (`edge_kind_guard_dynamic_hop_resolves_unknown`);
//! these cover the end-to-end detector behaviour:
//!   (a) mixed callers — per-path severity then worst-severity merge-tie + dual note;
//!   (b) PD resolves to info on a pure temp-caller path (was "(temp state uncertain)");
//!   (d) a non-PD terminal already Known(true) still downgrades to info (regression).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000d1abc";

/// Run d1 in isolation over an inline workspace and return its emitted findings.
fn run_d1(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    run_detectors(&resolved, &d1).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// --- (a) mixed callers: per-path severity then worst-severity merge-tie ------

/// Helper `H(var Cust)` does `Cust.Modify()` → terminal op temp_state = PD(0).
/// Caller A loops calling `H(TempCust)` (a TEMPORARY local) → that path resolves
/// Known(true) → info/temporary. Caller B loops calling `H(PhysCust)` (a PHYSICAL
/// local) → that path resolves Known(false) → high/physical. Both paths share the
/// SAME terminal op so the terminal-centric assembly collapses them to ONE
/// finding with one `LoopContext` per caller loop. The WINNER (the physical
/// route — higher verdict quality) drives severity (worst = high) and the note;
/// BOTH verdicts are surfaced structurally across `contexts[]` (the redesign
/// replaced the old dual-verdict prose note — verdicts now live in the contexts).
#[test]
fn mixed_callers_worst_severity_and_context_verdicts() {
    let src = r#"
table 50111 "MT Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50111 "MT D1 Mixed"
{
    procedure ModifyHelper(var Cust: Record "MT Cust")
    begin
        Cust.Modify();
    end;

    procedure CallerTemp()
    var TempCust: Record "MT Cust" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            ModifyHelper(TempCust);
    end;

    procedure CallerPhysical()
    var PhysCust: Record "MT Cust"; i: Integer;
    begin
        for i := 1 to 10 do
            ModifyHelper(PhysCust);
    end;
}
"#;
    let findings = run_d1(&[al("MTD1Mixed", src)]);

    // The two paths collapse to ONE finding on the shared terminal Modify.
    assert_eq!(
        findings.len(),
        1,
        "the two callers' paths must merge to one finding. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.severity, &f.root_cause))
            .collect::<Vec<_>>()
    );
    let f = &findings[0];

    // Worst severity wins: the physical path fires at "high" (Modify-in-loop), the
    // temp path would be "info". The winner (physical) drives the finding severity.
    assert_eq!(
        f.severity, "high",
        "worst severity must win (physical 'high' over temp 'info'). rootCause: {}",
        f.root_cause
    );

    // Both callers' verdicts are surfaced per VERDICT CLASS in cohortContexts[]
    // (the C6 cutover: one cohort per class, winner first — the two single-caller
    // loops fall into distinct physical/temporary classes).
    let ccs = f
        .cohort_contexts
        .as_ref()
        .expect("d1 findings carry cohort_contexts");
    assert_eq!(
        ccs.len(),
        2,
        "one cohort per verdict class. cohortContexts: {ccs:#?}"
    );
    // The winner (cohortContexts[0]) is the physical route (higher verdict quality).
    assert_eq!(
        ccs[0].verdict, "physical",
        "the physical route wins on verdict quality. cohortContexts: {ccs:#?}"
    );
    let verdicts: std::collections::BTreeSet<&str> =
        ccs.iter().map(|c| c.verdict.as_str()).collect();
    assert_eq!(
        verdicts,
        ["physical", "temporary"].into_iter().collect(),
        "both verdicts surfaced across cohorts. cohortContexts: {ccs:#?}"
    );
    // The winner is physical, so NO temp note on the rootCause; and the deleted
    // dual-verdict prose note must not reappear.
    assert!(
        !f.root_cause.contains("temp state varies by caller"),
        "the dual-verdict prose note is gone (verdicts live in contexts). rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains("temporary record"),
        "a physical winner carries no temporary note. rootCause: {}",
        f.root_cause
    );
}

// --- (b) PD resolves to info on a pure temp-caller path ----------------------

/// Single caller passing a TEMPORARY local to `H(var Cust)` whose `Cust.Modify()` is
/// PD(0). The one path resolves Known(true) → d1 downgrades to info (PREVIOUSLY this
/// fell to "(temp state uncertain)" at normal severity because the raw op state was
/// PD/unknown).
#[test]
fn pd_resolves_to_info_on_temp_caller_path() {
    let src = r#"
table 50112 "PT Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50112 "PT D1 TempOnly"
{
    procedure ModifyHelper(var Cust: Record "PT Cust")
    begin
        Cust.Modify();
    end;

    procedure OnlyCaller()
    var TempCust: Record "PT Cust" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            ModifyHelper(TempCust);
    end;
}
"#;
    let findings = run_d1(&[al("PTD1TempOnly", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "info",
        "a PD terminal reached only via a temp-caller path resolves Known(true) → info. \
         rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains("temporary record"),
        "info finding must carry the temporary-record note. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains("temp state uncertain"),
        "the PD must no longer fall to '(temp state uncertain)'. rootCause: {}",
        f.root_cause
    );
}

// --- (d) non-PD terminal already Known(true) still downgrades to info ---------

/// A DIRECT in-loop temp op (a temporary LOCAL var, op temp_state already
/// Known(true), no caller hop) resolves Known(true) with NO stepping → still
/// downgraded to info, exactly as before Task 10 (regression guard).
#[test]
fn direct_known_temp_still_info_unchanged() {
    let src = r#"
table 50113 "DT Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50113 "DT D1 Direct"
{
    procedure DirectTemp()
    var TempCust: Record "DT Cust" temporary; i: Integer;
    begin
        for i := 1 to 10 do
            TempCust.Modify();
    end;
}
"#;
    let findings = run_d1(&[al("DTD1Direct", src)]);
    assert_eq!(
        findings.len(),
        1,
        "one finding expected. findings: {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "info",
        "a direct Known(true) temp op stays info (no stepping). rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains("temporary record"),
        "direct temp op keeps the temporary-record note. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains("temp state varies by caller"),
        "a single-path finding must NOT get a dual-verdict note. rootCause: {}",
        f.root_cause
    );
}
