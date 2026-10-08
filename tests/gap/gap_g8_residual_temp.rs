//! Gap G-8 (docs/engine-gaps.md): residual global / by-var-param temp
//! resolution gaps reported by the CDO triage — INVESTIGATION + regression guard.
//!
//! Two shapes were reported still resolving "uncertain" after the temp-state
//! epoch:
//!
//! 1. **Case A — codeunit-global temporary record** (CDO eDocuments Dispatcher
//!    `TempErrors: Record "Error Message" temporary;`): ops on the global
//!    (Insert / FindSet / Next in loops) flagged temp-uncertain. The epoch's
//!    Task-3 object-global promotion (`l3_workspace.rs`) + pass-2a rebind
//!    (`record_types.rs`) should already resolve these `Known(true)` — note the
//!    referenced table ("Error Message") is a BASE-APP table that is NOT in the
//!    workspace, so the resolution must come from the `temporary` keyword on the
//!    var declaration alone, never from table lookup.
//!
//! 2. **Case B — keyword-temp by-var param** (CDO Aut. Statement Upgrade Mgt
//!    `GetUpgradeData(var Temp...: Record ... temporary)` from a Page caller):
//!    a by-var param whose `record_type` carries the `temporary_keyword` is
//!    `Known(true)` by contract-trust (Task 8 / RV-3) — the caller is
//!    irrelevant. Only a by-var param WITHOUT the keyword is PD(i), and a PD
//!    op whose path roots in its own routine (no caller hop) correctly stays
//!    Unknown → "uncertain" (out-of-model per-path truth, NOT a bug).
//!
//! Asserts both the L3 resolution and the real d1 detector pipeline, mirroring
//! tests/gap_g2_runtime_temp.rs.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g8abc";

fn run_gap_detectors(files: &[(String, String)], wanted: &[&str]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        wanted.len(),
        "each wanted detector must be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// =============================================================================
// Case A — codeunit-global `temporary` record (the eDocuments Dispatcher shape)
// =============================================================================

/// The reported shape: a codeunit-level GLOBAL `temporary` record over a table
/// that is NOT in the workspace (like base-app "Error Message" in CDO), with
/// in-loop Insert and a FindSet/Next drain loop.
const DISPATCHER_SRC: &str = r#"
codeunit 50980 "G8 Dispatcher"
{
    var
        TempErrors: Record "Error Message" temporary;

    procedure CollectErrors()
    var
        I: Integer;
    begin
        I := 0;
        repeat
            TempErrors.Insert();
            I := I + 1;
        until I > 10;
    end;

    procedure DrainErrors()
    begin
        if TempErrors.FindSet() then
            repeat
            until TempErrors.Next() = 0;
    end;
}
"#;

/// CONTROL: the same shape WITHOUT the `temporary` keyword — must stay
/// Known(false) (physical) and d1 must keep firing above info.
const DISPATCHER_CONTROL_SRC: &str = r#"
codeunit 50981 "G8 Dispatcher Plain"
{
    var
        Errors: Record "Error Message";

    procedure CollectErrors()
    var
        I: Integer;
    begin
        I := 0;
        repeat
            Errors.Insert();
            I := I + 1;
        until I > 10;
    end;
}
"#;

#[test]
fn case_a_global_temporary_record_ops_resolve_known_true() {
    let files = [al("G8Dispatcher", DISPATCHER_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let collect = resolved
        .routine_by_name("CollectErrors")
        .expect("CollectErrors must be resolved");
    assert_eq!(
        collect.record_var_scope("TempErrors").as_deref(),
        Some("global"),
        "TempErrors must be the PROMOTED object-global record var",
    );
    assert_eq!(
        collect.record_var_temp_known("TempErrors"),
        Some(true),
        "a codeunit-global `Record \"Error Message\" temporary` var must resolve \
         Known(true) — from the `temporary` keyword alone (the table is NOT in \
         the workspace)",
    );
    assert_eq!(
        collect.first_record_op_temp_known("TempErrors"),
        Some(true),
        "`TempErrors.Insert()` in a loop must resolve Known(true) via the \
         Task-3 promotion + pass-2a rebind",
    );

    let drain = resolved
        .routine_by_name("DrainErrors")
        .expect("DrainErrors must be resolved");
    assert_eq!(
        drain.first_record_op_temp_known("TempErrors"),
        Some(true),
        "`TempErrors.FindSet()` / `.Next()` must also resolve Known(true) — \
         the promotion reaches EVERY routine of the object",
    );
}

#[test]
fn case_a_d1_downgrades_global_temp_in_loop_insert_to_info() {
    let files = [al("G8Dispatcher", DISPATCHER_SRC)];
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    let non_info: Vec<_> = findings.iter().filter(|f| f.severity != "info").collect();
    assert!(
        non_info.is_empty(),
        "d1 must downgrade in-loop ops on the codeunit-global temporary record \
         to info (Known(true) tempness); got: {:?}",
        non_info
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}

#[test]
fn case_a_control_non_temp_global_stays_physical_and_d1_fires() {
    let files = [al("G8DispatcherPlain", DISPATCHER_CONTROL_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let collect = resolved
        .routine_by_name("CollectErrors")
        .expect("CollectErrors must be resolved");
    assert_eq!(
        collect.first_record_op_temp_known("Errors"),
        Some(false),
        "a codeunit-global record WITHOUT `temporary` must resolve Known(false)",
    );

    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    assert!(
        findings.iter().any(|f| f.severity != "info"),
        "d1 must keep firing (above info) on the non-temp global's in-loop Insert",
    );
}

/// The FORWARDED-global shape (the suspected real CDO finding): the temp global
/// is passed BY-VAR into a local helper whose keyword-less by-var param does the
/// op, with the CALL inside the caller's loop. The d1 transitive path is
/// `DispatchAll(loop) → LogError → Errors.Insert()`; the terminal op is PD(0)
/// and per-path resolution must chase the hop binding back to the PROMOTED
/// global's Known(true).
const FORWARDING_SRC: &str = r#"
codeunit 50985 "G8 Fwd Dispatcher"
{
    var
        TempErrors: Record "Error Message" temporary;

    procedure DispatchAll()
    var
        I: Integer;
    begin
        I := 0;
        repeat
            LogError(TempErrors);
            I := I + 1;
        until I > 10;
    end;

    local procedure LogError(var Errors: Record "Error Message")
    begin
        Errors.Insert();
    end;
}
"#;

/// CONTROL: identical forwarding shape but the global is NOT temporary — the
/// transitive finding must keep firing above info.
const FORWARDING_CONTROL_SRC: &str = r#"
codeunit 50986 "G8 Fwd Dispatcher Plain"
{
    var
        Errors: Record "Error Message";

    procedure DispatchAll()
    var
        I: Integer;
    begin
        I := 0;
        repeat
            LogError(Errors);
            I := I + 1;
        until I > 10;
    end;

    local procedure LogError(var Errors2: Record "Error Message")
    begin
        Errors2.Insert();
    end;
}
"#;

#[test]
fn case_a_forwarded_global_temp_resolves_info_along_path() {
    let files = [al("G8FwdDispatcher", FORWARDING_SRC)];
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    assert!(
        !findings.is_empty(),
        "the transitive loop→call→Insert path must produce a d1 finding"
    );
    let non_info: Vec<_> = findings.iter().filter(|f| f.severity != "info").collect();
    assert!(
        non_info.is_empty(),
        "the by-var-forwarded GLOBAL temp record must resolve Known(true) along \
         the path (promoted-global binding → PD substitution) and downgrade to \
         info; got: {:?}",
        non_info
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}

#[test]
fn case_a_control_forwarded_non_temp_global_keeps_firing() {
    let files = [al("G8FwdDispatcherPlain", FORWARDING_CONTROL_SRC)];
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    assert!(
        findings.iter().any(|f| f.severity != "info"),
        "forwarding a NON-temp global into the helper must keep the transitive \
         d1 finding above info; all: {:?}",
        findings
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}

// =============================================================================
// Case B — by-var params (the GetUpgradeData shape)
// =============================================================================

/// A keyword-temp by-var param (`var TempBuf: Record X temporary`) is
/// Known(true) by contract-trust (Task 8 / RV-3) — no caller needed. The
/// keyword-LESS by-var twin is PD and stays unproven in isolation. The table is
/// in-workspace and NOT TableType=Temporary, so no table-level override can
/// mask the param-level resolution.
const UPGRADE_SRC: &str = r#"
table 50982 "G8 Upgrade Buffer"
{
    fields { field(1; Id; Integer) { } }
}

codeunit 50983 "G8 Upgrade Mgt"
{
    procedure GetUpgradeData(var TempBuf: Record "G8 Upgrade Buffer" temporary)
    var
        I: Integer;
    begin
        I := 0;
        repeat
            TempBuf.Id := I;
            TempBuf.Insert();
            I := I + 1;
        until I > 10;
    end;

    procedure GetUpgradeDataPlain(var Buf: Record "G8 Upgrade Buffer")
    var
        I: Integer;
    begin
        I := 0;
        repeat
            Buf.Id := I;
            Buf.Insert();
            I := I + 1;
        until I > 10;
    end;
}
"#;

/// A Page caller passing a temp LOCAL into the keyword-less by-var param —
/// the batch-7 caller shape. The op inside `GetUpgradeDataPlain` is PD(0);
/// d1's DIRECT finding (loop and op in the same routine) has no caller hop on
/// its path, so per-path resolution correctly stays Unknown → "uncertain".
const PAGE_CALLER_SRC: &str = r#"
page 50984 "G8 Upgrade Page"
{
    actions
    {
        area(Processing)
        {
            action(RunUpgrade)
            {
                trigger OnAction()
                var
                    TempLocal: Record "G8 Upgrade Buffer" temporary;
                    UpgradeMgt: Codeunit "G8 Upgrade Mgt";
                begin
                    UpgradeMgt.GetUpgradeDataPlain(TempLocal);
                end;
            }
        }
    }
}
"#;

#[test]
fn case_b_keyword_temp_by_var_param_resolves_known_true() {
    let files = [al("G8UpgradeMgt", UPGRADE_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let routine = resolved
        .routine_by_name("GetUpgradeData")
        .expect("GetUpgradeData must be resolved");
    assert_eq!(
        routine.record_var_temp_known("TempBuf"),
        Some(true),
        "a `var TempBuf: Record X temporary` param must be Known(true) by \
         contract-trust — the caller is irrelevant",
    );
    assert_eq!(
        routine.first_record_op_temp_known("TempBuf"),
        Some(true),
        "`TempBuf.Insert()` must resolve Known(true) from the param keyword",
    );
}

#[test]
fn case_b_d1_downgrades_keyword_temp_param_loop_insert_to_info() {
    // Only the keyword-temp routine in scope: every d1 finding must be info.
    let single = r#"
table 50982 "G8 Upgrade Buffer"
{
    fields { field(1; Id; Integer) { } }
}

codeunit 50983 "G8 Upgrade Mgt"
{
    procedure GetUpgradeData(var TempBuf: Record "G8 Upgrade Buffer" temporary)
    var
        I: Integer;
    begin
        I := 0;
        repeat
            TempBuf.Id := I;
            TempBuf.Insert();
            I := I + 1;
        until I > 10;
    end;
}
"#;
    let files = [al("G8UpgradeMgt", single)];
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    let non_info: Vec<_> = findings.iter().filter(|f| f.severity != "info").collect();
    assert!(
        non_info.is_empty(),
        "d1 must downgrade the in-loop Insert on a keyword-temp by-var param to \
         info; got: {:?}",
        non_info
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}

#[test]
fn case_b_keywordless_by_var_param_is_pd_and_stays_uncertain_per_path() {
    // OUT-OF-MODEL DOCUMENTATION (not a bug): the keyword-LESS by-var param is
    // PD(0). The d1 DIRECT finding's path roots in GetUpgradeDataPlain itself —
    // no caller hop to chase — so per-path resolution collapses to Unknown and
    // the finding fires "uncertain" EVEN IF the only in-workspace caller (the
    // page) passes a temp local. That caller-side truth belongs to the
    // transitive finding rooted at the caller, not to the direct one.
    let files = [
        al("G8UpgradeMgt", UPGRADE_SRC),
        al("G8UpgradePage", PAGE_CALLER_SRC),
    ];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let plain = resolved
        .routine_by_name("GetUpgradeDataPlain")
        .expect("GetUpgradeDataPlain must be resolved");
    assert_ne!(
        plain.record_var_temp_known("Buf"),
        Some(true),
        "a by-var param WITHOUT the temporary keyword must NOT be Known(true) \
         in isolation (it is parameter-dependent)",
    );
    assert_ne!(
        plain.first_record_op_temp_known("Buf"),
        Some(true),
        "`Buf.Insert()` must stay parameter-dependent/unknown — only a caller \
         path can bind it",
    );

    // The direct d1 finding inside GetUpgradeDataPlain keeps firing above info
    // (uncertain — NOT downgraded by the page's temp local).
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    let above_info: Vec<_> = findings.iter().filter(|f| f.severity != "info").collect();
    assert!(
        !above_info.is_empty(),
        "the PD in-loop Insert must keep firing above info somewhere (uncertain \
         per-path is the conservative, correct verdict); all findings: {:?}",
        findings
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}
