//! #272: an implicit `Rec`/`xRec` passed as a bare argument binds like a declared
//! record variable, so the cross-call detectors see it (spec: `## Design`,
//! revision 3, in the issue ledger; acceptance rows A2-A5).
//!
//! Every test drives the REAL analyze pipeline (`run_analyze_with_exit`) over a
//! committed fixture under `tests/fixtures/issue-272/` (not `tests/r0-corpus`: the
//! corpus is walked by the golden differentials, which would then demand goldens).
//! Each test asserts the EXACT set of `(detector, object, routine, line)` rows, so a
//! finding that moves between routines, even two of one name, fails as surely as a
//! missing one.
//! - `ws-owner`: d39 owner-rule cases added after the final panel (a page over a
//!   `TableType = Temporary` table, page OnDeleteRecord).
//!
//! - `ws-shapes`: implicit `Rec` in table / tableext / page / pageext routines, next
//!   to the same shape on a local record (the reference that fires before and after).
//! - `ws-readers`: the shared binding readers (d37's helper-persistence walk, d11/d21
//!   helper loads, d33 helper filters) with their controls.
//!
//! A6 (d40 keeps its C2(a) skip of the implicit `Rec`) is pinned by the r4
//! `ws-d40` golden; `shapes_d40_still_skips_the_implicit_rec` below states the same
//! rule on `ws-shapes` (only the local fires).

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issue-272")
        .join(name)
}

/// The findings of `detectors` (comma-separated full ids) over `ws`, default
/// (cross-app) mode, primary scope.
fn analyze(ws: &Path, detectors: &str) -> Vec<serde_json::Value> {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(detectors.to_string()),
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
        match run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "272-test") {
            Ok((out, ..)) => out,
            Err(e) => panic!("run_analyze failed: {e}"),
        };
    let json: serde_json::Value = serde_json::from_str(&out).expect("analyze JSON must parse");
    json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .clone()
}

fn s<'a>(v: &'a serde_json::Value, what: &str) -> &'a str {
    v.as_str()
        .unwrap_or_else(|| panic!("finding has no string {what}"))
}

/// `dNN` from a full detector id.
fn short(detector: &str) -> &str {
    detector.split('-').next().unwrap()
}

/// `"dNN\t<object name>\t<routine name>\tL<line>"` for each finding of
/// `detector_short`, sorted. The line (1-based, the finding's primary location: the
/// forwarded argument) tells apart routines of one name in one object, e.g. the
/// three `S Card` action OnAction triggers.
fn rows(fs: &[serde_json::Value], detector_short: &str) -> Vec<String> {
    let mut out: Vec<String> = fs
        .iter()
        .filter(|f| short(s(&f["detector"], "detector")) == detector_short)
        .map(|f| {
            let loc = &f["primaryLocation"];
            format!(
                "{detector_short}\t{}\t{}\tL{}",
                s(&loc["objectName"], "objectName"),
                s(&loc["routineName"], "routineName"),
                loc["line"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("finding has no line: {loc:#?}"))
            )
        })
        .collect();
    out.sort();
    out
}

fn sorted(expected: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = expected.iter().map(|r| r.replace(" | ", "\t")).collect();
    v.sort();
    v
}

/// The single finding of `detector_short` in `routine` of `object`.
fn finding<'a>(
    fs: &'a [serde_json::Value],
    detector_short: &str,
    object: &str,
    routine: &str,
) -> &'a serde_json::Value {
    let hits: Vec<_> = fs
        .iter()
        .filter(|f| {
            short(s(&f["detector"], "detector")) == detector_short
                && f["primaryLocation"]["objectName"] == object
                && f["primaryLocation"]["routineName"] == routine
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {detector_short} finding in {object}.{routine}: {hits:#?}"
    );
    hits[0]
}

const SHAPES_DETECTORS: &str = "d39-record-left-dirty-across-chain,\
d40-transitive-load-missing,d41-transitive-filter-loss,d42-cross-call-wrong-setloadfields";

fn shapes() -> Vec<serde_json::Value> {
    analyze(&fixture("ws-shapes"), SHAPES_DETECTORS)
}

/// A2: d41 reports the implicit `Rec` on a page OnAction (`S Card` P1), a pageext
/// OnAction (`S Card Ext` E1), a tableext procedure (`ExtD41ViaRec`), a table
/// procedure (`D41ViaRec`), bare `SetRange` + `Rec` (`D41ViaBareRec`) and a field
/// OnValidate `xRec` (`S Cust` OnValidate); also an undeclared `xRec` next to a
/// declared local `Rec` (`ShadowRecFwdXRec`), and a codeunit global `Rec`
/// (`ViaGlobalRec`): on the base it was mislabelled `implicit-rec`, which kept the
/// L3 global backfill from rebinding it. Unchanged: the local reference
/// (`D41ViaLocal`) and a declared local `xRec` (`ShadowDeclaredXRec`), the only two
/// that fire on the base.
#[test]
fn shapes_d41_reports_the_implicit_rec() {
    assert_eq!(
        rows(&shapes(), "d41"),
        sorted(&[
            "d41 | S Card | OnAction | L32",
            "d41 | S Card | ShadowDeclaredXRec | L96",
            "d41 | S Card | ShadowRecFwdXRec | L87",
            "d41 | S Card Ext | OnAction | L127",
            "d41 | S Cust | D41ViaBareRec | L53",
            "d41 | S Cust | D41ViaLocal | L44",
            "d41 | S Cust | D41ViaRec | L36",
            "d41 | S Cust | OnValidate | L17",
            "d41 | S Cust Ext | ExtD41ViaRec | L8",
            "d41 | S Global Rec | ViaGlobalRec | L48",
        ])
    );
}

/// A3: d42 reports the implicit `Rec` in a table procedure (`D42ViaRec`) and a page
/// OnAction (`S Card` P4) as it reports the local (`D42ViaLocal`); the table `Rec`
/// finding carries the same `looseFingerprint` as the local one (same object,
/// callee, parameter and caller table). Today only `D42ViaLocal` fires.
///
/// Limit: the page finding has no local twin in its own object, so its loose key is
/// not compared (the key includes the object).
#[test]
fn shapes_d42_reports_the_implicit_rec() {
    let fs = shapes();
    assert_eq!(
        rows(&fs, "d42"),
        sorted(&[
            "d42 | S Card | OnAction | L53",
            "d42 | S Cust | D42ViaLocal | L85",
            "d42 | S Cust | D42ViaRec | L77",
        ])
    );
    let via_rec = finding(&fs, "d42", "S Cust", "D42ViaRec");
    let via_local = finding(&fs, "d42", "S Cust", "D42ViaLocal");
    assert_eq!(
        s(&via_rec["looseFingerprint"], "looseFingerprint"),
        s(&via_local["looseFingerprint"], "looseFingerprint"),
        "the implicit Rec and the local share the caller table"
    );
}

/// A5: d39's owner rule. NOT reported (the caller or the platform persists `Rec`):
/// table procedure `D39ViaRec`, table field OnValidate, table OnModify, page
/// OnAction / field OnValidate / OnModifyRecord and procedure `PageProcDirty` on the
/// editable card, pageext OnAfterAction (`S Card Ext2`), tableext OnBeforeModify /
/// OnBeforeRename / OnAfterDelete. Reported: tableext OnAfterModify / OnAfterInsert /
/// OnAfterRename, `xRec` (`ExtXRec`), the `Editable = false` list OnAction (`S RO
/// List`), the `ModifyAllowed = false` card OnAction (`S NoMod Card`), the pageext
/// action on the read-only list (`S RO List Ext`), OnAfterGetRecord (`S AGR Card`),
/// a procedure that reloads `Rec` after the call (`ExtReloadAfter`), and the local
/// (`D39ViaLocal`, unchanged). Today only `D39ViaLocal` fires.
///
/// The second half of A5 is `gap_audit_b_table_triggers::
/// table_level_trigger_rec_ops_are_suppressed` (table OnModify forwarding `Rec` to a
/// dirtying helper), which must stay green once the binding resolves.
#[test]
fn shapes_d39_owner_rule() {
    assert_eq!(
        rows(&shapes(), "d39"),
        sorted(&[
            "d39 | S AGR Card | OnAfterGetRecord | L34",
            "d39 | S Cust | D39ViaLocal | L68",
            "d39 | S Cust Ext | OnAfterModify | L16",
            "d39 | S Cust Ext2 | ExtReloadAfter | L33",
            "d39 | S Cust Ext2 | ExtXRec | L69",
            "d39 | S Cust Ext2 | OnAfterInsert | L62",
            "d39 | S Cust Ext2 | OnAfterRename | L41",
            "d39 | S NoMod Card | OnAction | L17",
            "d39 | S RO List | OnAction | L17",
            "d39 | S RO List Ext | OnAction | L36",
        ])
    );
}

/// A5, final-panel cases over `ws-owner` (kept out of `ws-shapes`, whose summary
/// core A8 pins against the base):
/// - F1: a page over a `TableType = Temporary` table (no `SourceTableTemporary`)
///   hands a temporary `Rec`: d39's G-13 skip holds in OnAfterGetRecord, which the
///   owner rule would report, and in OnAction.
/// - F2: page OnDeleteRecord is exempt (the row is deleted; moot, not measured).
/// - FC4: a page PROCEDURE named `OnDeleteRecord` on an `Editable = false` page
///   (`O RO Card`) is not the trigger: it IS reported.
/// - Control: OnAfterGetRecord on the physical `O Card` reports, so the workspace
///   reaches d39 at all.
#[test]
fn owner_d39_temp_table_and_delete_record() {
    assert_eq!(
        rows(
            &analyze(&fixture("ws-owner"), "d39-record-left-dirty-across-chain"),
            "d39"
        ),
        sorted(&[
            "d39 | O Card | OnAfterGetRecord | L39",
            "d39 | O RO Card | OnDeleteRecord | L91",
        ])
    );
}

/// d40 keeps its C2(a) skip (A6's rule, golden-pinned on `ws-d40`): `D40ViaRec`
/// does not fire, the local does. Passes before and after #272.
#[test]
fn shapes_d40_still_skips_the_implicit_rec() {
    assert_eq!(
        rows(&shapes(), "d40"),
        sorted(&["d40 | S Cust | D40ViaLocal | L98"])
    );
}

/// A4: the other readers of argument bindings now see the implicit `Rec`.
/// - d37 is suppressed when the helper persists `Rec` by var (`D37Persists`), the
///   callee is unresolved (`D37Unresolved`) or the arity does not match
///   (`D37Arity`); it still fires for a read-only helper, a by-value helper, a
///   helper given another record and a helper that runs before the Validate.
/// - d11 is suppressed when a helper loads `Rec` by var (`D11Loaded`); it fires for
///   a by-value loader and a load after the Modify.
/// - d21 is suppressed when a helper loads `Rec` (`D21Loaded`); it fires when the
///   helper loads another record.
/// - d33 is suppressed when a helper filters `Rec` by var (`D33Filtered`); it fires
///   for a by-value filter and no filter.
/// - The `Local*` reference routines (the same shapes on a local record) fire
///   nothing, before and after.
///
/// Today `D37Persists`/`D37Unresolved`/`D37Arity` (d37), `D11Loaded` (d11),
/// `D21Loaded` (d21) and `D33Filtered` (d33) also fire.
#[test]
fn readers_see_the_implicit_rec() {
    let fs = analyze(
        &fixture("ws-readers"),
        "d11-modify-without-get,d21-read-without-load,d33-unfiltered-bulk-write,\
d37-validate-without-persist",
    );
    let mut got: Vec<String> = ["d11", "d21", "d33", "d37"]
        .iter()
        .flat_map(|d| rows(&fs, d))
        .collect();
    got.sort();
    assert_eq!(
        got,
        sorted(&[
            "d11 | R Cust | D11ByValue | L72",
            "d11 | R Cust | D11Order | L78",
            "d11 | R Cust | D37Arity | L42",
            "d11 | R Cust | D37ByValue | L28",
            "d11 | R Cust | D37NoPersist | L21",
            "d11 | R Cust | D37Order | L57",
            "d11 | R Cust | D37Persists | L14",
            "d11 | R Cust | D37Unresolved | L35",
            "d11 | R Cust | D37WrongVar | L49",
            "d21 | R Cust | D21WrongVar | L93",
            "d33 | R Cust | D33ByValue | L108",
            "d33 | R Cust | D33Unfiltered | L113",
            "d37 | R Cust | D37ByValue | L28",
            "d37 | R Cust | D37NoPersist | L21",
            "d37 | R Cust | D37Order | L57",
            "d37 | R Cust | D37WrongVar | L49",
        ])
    );
}
