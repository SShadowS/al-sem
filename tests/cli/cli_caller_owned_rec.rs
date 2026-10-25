//! #299: d11 and d37 skip the receiver `Rec` of a Table / TableExtension routine,
//! because the caller (or the platform) owns that record (spec: `## Design`,
//! revision 3, in the issue ledger).
//!
//! The test drives the REAL analyze pipeline (`run_analyze_with_exit`) over the
//! committed fixture `tests/fixtures/issue-299/ws-owned` (not `tests/r0-corpus`,
//! whose walkers would demand goldens) and asserts the EXACT set of
//! `(detector, object, routine, line)` rows. Every precondition is written in the
//! fixture itself; its comments state each routine's expected rows.
//!
//! What the rows pin:
//! - The receiver skip: no row for `ProcModify`, `ProcValidate`, the field
//!   `OnLookup`, `ProcValidateThenReset` (`Reset` keeps field values),
//!   `MemberRecAssign` (a member target `X.Rec := v` is not a reset), `TempModify`
//!   (a temporary table keeps the flag) or `ExtProc`.
//! - d37 still fires when the routine puts `Rec` in a new state after the
//!   `Validate` (`ProcValidateThenNext`, `ProcValidateThenHelperLoad`,
//!   `ProcValidateThenAssign`), when the assignment is in the other arm of the
//!   branch (`ProcValidateOppositeBranchAssign`, a stated conservative limit), and
//!   in a tableext after-write trigger (`OnAfterModify`).
//! - d11 still fires when the routine replaced `Rec` itself (bare `Rec := Other`)
//!   before the op (`ProcAssignThenModify`), not when the replacement comes after
//!   (`ProcModifyThenAssign`).
//! - A stated limit, pinned: `Validate; Clear(Rec)` (`ProcValidateThenClear`) has
//!   no row. d37's helper rule suppresses it before the receiver rule runs (an
//!   unresolved call that takes `Rec`), as it does for any record.
//! - Fail closed: a named return `Rec` (`ProcNamedReturn`) is not the receiver.
//! - Controls: a local record in a table (`ProcLocal`) and in a codeunit
//!   (`LocalControl`), and the page copies of #272's reader shapes (`O Card`),
//!   which keep the d37 helper walk and d11 helper load on an implicit `Rec`
//!   covered now that the table copies in `ws-readers` no longer fire.
//!
//! `ProcShadow` (a local `Rec: Record`) and `ProcShadowRecRef` (a local
//! `Rec: RecordRef`) fire, but they do NOT guard the flag: a declared `Rec` gets no
//! seeded row, so no break of the flag can reach them.

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

/// `"dNN | <object> | <routine> | L<line>"` for every d11 / d37 finding over
/// `ws-owned`, default (cross-app) mode, primary scope, sorted.
fn owned_rows() -> Vec<String> {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issue-299/ws-owned");
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some("d11-modify-without-get,d37-validate-without-persist".to_string()),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        single_app: false,
        with_evidence: false,
    };
    let out =
        match run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "299-test") {
            Ok((out, ..)) => out,
            Err(e) => panic!("run_analyze failed: {e}"),
        };
    let json: serde_json::Value = serde_json::from_str(&out).expect("analyze JSON must parse");
    let mut rows: Vec<String> = json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|f| {
            let loc = &f["primaryLocation"];
            format!(
                "{} | {} | {} | L{}",
                f["detector"].as_str().unwrap().split('-').next().unwrap(),
                loc["objectName"].as_str().unwrap(),
                loc["routineName"].as_str().unwrap(),
                loc["line"].as_u64().unwrap()
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn table_receiver_rec_is_caller_owned() {
    let mut expected = vec![
        "d11 | O Card | D11ByValue | L193",
        "d11 | O Card | D11Order | L199",
        "d11 | O Card | D37Arity | L164",
        "d11 | O Card | D37ByValue | L150",
        "d11 | O Card | D37NoPersist | L143",
        "d11 | O Card | D37Order | L179",
        "d11 | O Card | D37Persists | L136",
        "d11 | O Card | D37Unresolved | L157",
        "d11 | O Card | D37WrongVar | L171",
        "d11 | O Cust | ProcLocal | L67",
        "d11 | O Cust | ProcLocal | L68",
        "d11 | O Cust | ProcNamedReturn | L61",
        "d11 | O Cust | ProcShadow | L55",
        "d11 | O Cust | ProcShadow | L56",
        "d11 | O Cust | ProcShadowRecRef | L75",
        "d11 | O H | LocalControl | L234",
        "d11 | O H | LocalControl | L235",
        "d11 | O Repl | ProcAssignThenModify | L248",
        "d37 | O Card | D37ByValue | L150",
        "d37 | O Card | D37NoPersist | L143",
        "d37 | O Card | D37Order | L179",
        "d37 | O Card | D37WrongVar | L171",
        "d37 | O Cust Ext | OnAfterModify | L122",
        "d37 | O Cust | ProcNamedReturn | L61",
        "d37 | O Cust | ProcValidateOppositeBranchAssign | L82",
        "d37 | O Cust | ProcValidateThenAssign | L42",
        "d37 | O Cust | ProcValidateThenHelperLoad | L35",
        "d37 | O Cust | ProcValidateThenNext | L28",
    ];
    expected.sort();
    assert_eq!(owned_rows(), expected);
}
