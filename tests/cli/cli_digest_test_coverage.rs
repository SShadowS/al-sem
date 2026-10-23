//! #254: `alsem digest … --tests` adds `payload.testCoverage`: which changed
//! production routines some primary-app `[Test]` procedure reaches over the
//! resolved call graph (calls, implicit triggers, resolved object runs, interface
//! candidates, event dispatch, `[HandlerFunctions]`), and why the rest are not.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem` on `tests/fixtures/test-selection-graph/`
//! (or the no-tests fixture). The expected `covered` / `uncovered` sets are
//! hand-stated from the fixture's AL source, never computed by production code.
//! Stable ids are hashes, so items are compared by `display` / `objectDisplay`;
//! the one id-valued field, `reachedFrom`, is checked against the lowest stable
//! id among the tests the source says reach the routine (the witness rule), with
//! the tests' ids read from a plain digest of those test procedures.
//! Ledger: `.agent/issue-254/ledger.md` (Design, Acceptance matrix rows 1-7).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const VERSION_OVERRIDE: &str = "cli-test-coverage-v1";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn graph_ws() -> PathBuf {
    fixtures().join("test-selection-graph")
}

fn no_tests_ws() -> PathBuf {
    fixtures().join("digest-test-coverage-no-tests")
}

fn diff_file(name: &str) -> PathBuf {
    fixtures().join("digest-test-coverage").join(name)
}

/// `alsem digest <ws> <args> --format json --deterministic` -> (raw stdout, doc).
/// Panics with stderr when the run fails (today: clap rejects `--tests`).
fn digest_raw(ws: &Path, args: &[&str]) -> (String, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("digest")
        .arg(ws)
        .args(args)
        .args(["--format", "json", "--deterministic"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(
        out.status.success(),
        "alsem digest {args:?} exited {:?}: stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    (stdout, doc)
}

/// `--routine` args for every name.
fn routines(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .flat_map(|n| ["--routine".to_string(), n.to_string()])
        .collect()
}

fn with_tests(ws: &Path, args: &[String]) -> (String, Value) {
    let mut a: Vec<&str> = args.iter().map(String::as_str).collect();
    a.push("--tests");
    digest_raw(ws, &a)
}

/// The `testCoverage` block, checked against the invariants every run must hold
/// (matrix row 5): covered + uncovered == changedRoutines, disjoint, each sorted
/// by stableId, `coveredChangedRoutines` == |covered|, `ratio` = covered/changed
/// (or null when changed is 0), every covered item has exactly one witness.
fn block(doc: &Value) -> Value {
    let b = doc["payload"]
        .get("testCoverage")
        .unwrap_or_else(|| panic!("payload.testCoverage missing: {doc}"))
        .clone();
    let ids = |k: &str| -> Vec<String> {
        b[k].as_array()
            .unwrap_or_else(|| panic!("`{k}` is not an array: {b}"))
            .iter()
            .map(|i| i["stableId"].as_str().unwrap().to_string())
            .collect()
    };
    let (cov, unc) = (ids("covered"), ids("uncovered"));
    for (k, v) in [("covered", &cov), ("uncovered", &unc)] {
        let mut s = v.clone();
        s.sort();
        assert_eq!(v, &s, "`{k}` not sorted by stableId: {b}");
        let set: BTreeSet<&String> = v.iter().collect();
        assert_eq!(set.len(), v.len(), "duplicate in `{k}`: {b}");
    }
    let c: BTreeSet<&String> = cov.iter().collect();
    assert!(
        unc.iter().all(|u| !c.contains(u)),
        "covered/uncovered overlap: {b}"
    );
    let changed = b["changedRoutines"].as_u64().unwrap();
    assert_eq!((cov.len() + unc.len()) as u64, changed, "partition: {b}");
    assert_eq!(
        b["coveredChangedRoutines"].as_u64().unwrap(),
        cov.len() as u64
    );
    if changed == 0 {
        assert!(b["ratio"].is_null(), "ratio must be null at 0 changed: {b}");
    } else {
        let r = b["ratio"].as_f64().unwrap_or_else(|| panic!("ratio: {b}"));
        assert!(
            (r - cov.len() as f64 / changed as f64).abs() < 1e-9,
            "ratio: {b}"
        );
    }
    for i in b["covered"].as_array().unwrap() {
        assert_eq!(i["reachedFrom"].as_array().map(Vec::len), Some(1), "{i}");
        assert!(i["viaEvent"].is_boolean(), "{i}");
    }
    b
}

/// (display, reachedFrom id, viaEvent) of every covered item, sorted.
fn covered(b: &Value) -> Vec<(String, String, bool)> {
    let mut v: Vec<_> = b["covered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["display"].as_str().unwrap().to_string(),
                i["reachedFrom"][0].as_str().unwrap().to_string(),
                i["viaEvent"].as_bool().unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

/// (display, objectDisplay, reason) of every uncovered item, sorted.
fn uncovered(b: &Value) -> Vec<(String, String, String)> {
    let mut v: Vec<_> = b["uncovered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["display"].as_str().unwrap().to_string(),
                i["objectDisplay"].as_str().unwrap().to_string(),
                i["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

/// The stable id of the single digest entry a `--routine <name>` selects
/// (a plain digest, no `--tests`).
fn stable_id(name: &str) -> String {
    let (_, doc) = digest_raw(&graph_ws(), &["--routine", name]);
    let e = doc["payload"]["entries"].as_array().unwrap();
    assert_eq!(e.len(), 1, "`{name}` selects one routine: {doc}");
    e[0]["routine"]["stableId"].as_str().unwrap().to_string()
}

/// The witness rule: the lowest stable id among the tests that reach a routine.
fn witness(tests: &[&str]) -> String {
    tests.iter().map(|t| stable_id(t)).min().unwrap()
}

fn cov(display: &str, tests: &[&str], via_event: bool) -> (String, String, bool) {
    (display.to_string(), witness(tests), via_event)
}

fn unc(display: &str, object: &str, reason: &str) -> (String, String, String) {
    (display.to_string(), object.to_string(), reason.to_string())
}

/// The production routines of the main selection (rows 1, 2, 3, 5, 6, 7).
const MAIN: &[&str] = &[
    "TaxAmount",
    "PointsFor",
    "ApplyDecay",
    "CashRoundOnInvoiceTotal",
    "RoundToNickel",
    "AddBonusOnCalcPoints",
    "Ping",
    "ExportLine",
    "ReachedOnlyFromNowhere",
    "OnAfterInsertLedger",
];

/// The fixture's 11 `[Test]` procedures (Tests.al). `CardHandler` is a
/// `[ModalPageHandler]`, not a root.
const TEST_COUNT: u64 = 11;

/// Rows 1, 2, 3, 5, 7: the exact covered / uncovered sets of the main selection.
#[test]
fn main_selection_exact_sets() {
    let (_, doc) = with_tests(&graph_ws(), &routines(MAIN));
    let b = block(&doc);
    assert_eq!(
        b["testProceduresConsidered"].as_u64(),
        Some(TEST_COUNT),
        "{b}"
    );
    assert_eq!(b["excludedTestCode"].as_u64(), Some(0), "{b}");
    assert_eq!(b["changedRoutines"].as_u64(), Some(10), "{b}");

    let mut want_cov = vec![
        // Row 1, direct call: Tests.al:33 GermanTax -> TaxAmount; InvoiceTotalRounds
        // reaches it too (Invoice.al:10 -> Tax.al:36).
        cov("TaxAmount", &["GermanTax", "InvoiceTotalRounds"], false),
        // Tests.al:10 BonusAboveThousand -> PointsFor; NightlyRecalcDecays via OnRun (Loyalty.al:47).
        cov(
            "PointsFor",
            &["BonusAboveThousand", "NightlyRecalcDecays"],
            false,
        ),
        // Row 7, two-hop: RecalcNightly -> Codeunit.Run(Acc Nightly Recalc) -> OnRun -> ApplyDecay.
        cov("ApplyDecay", &["NightlyRecalcDecays"], false),
        // Row 2, pub -> sub: InvoiceTotal raises OnAfterInvoiceTotal (Invoice.al:11),
        // the only way into the Invoice.al subscriber and its callee.
        cov("CashRoundOnInvoiceTotal", &["InvoiceTotalRounds"], true),
        cov("RoundToNickel", &["InvoiceTotalRounds"], true),
        // PointsFor raises OnAfterCalcPoints (Loyalty.al:8); both loyalty tests reach it.
        cov(
            "AddBonusOnCalcPoints",
            &["BonusAboveThousand", "NightlyRecalcDecays"],
            true,
        ),
    ];
    want_cov.sort();
    assert_eq!(covered(&b), want_cov, "covered: {b}");

    let mut want_unc = vec![
        // Row 3: Ping is reached only through `V.Ping()` on a Variant (Dynamic.al:10,
        // ledger status dynamic-target), which is not followed.
        unc("Ping", "Codeunit Sel Pingable", "no-path"),
        // Row 7: LegacyExport.al, called by nothing.
        unc("ExportLine", "Codeunit Acc Legacy Export", "no-path"),
        // Dynamic.al:39: called only by a subscriber to an event that does not exist.
        unc(
            "ReachedOnlyFromNowhere",
            "Codeunit Sel Unbound Subscriber",
            "no-path",
        ),
        // Ledger.al:164: subscribes to the table's platform OnAfterInsertEvent; no
        // publisher routine exists.
        unc(
            "OnAfterInsertLedger",
            "Codeunit Sel Ledger Subscriber",
            "platform-event-subscriber",
        ),
    ];
    want_unc.sort();
    assert_eq!(uncovered(&b), want_unc, "uncovered: {b}");

    // Each uncovered item carries its digest entry's own anchor {file, line}.
    let entries = doc["payload"]["entries"].as_array().unwrap();
    for u in b["uncovered"].as_array().unwrap() {
        let e = entries
            .iter()
            .find(|e| e["routine"]["stableId"] == u["stableId"])
            .unwrap_or_else(|| panic!("no entry for {u}"));
        assert_eq!(u["anchor"]["file"], e["routine"]["anchor"]["file"], "{u}");
        assert_eq!(u["anchor"]["line"], e["routine"]["anchor"]["line"], "{u}");
    }
}

/// Row 3: the traversal is incomplete, and the block says so. The reach of all
/// 11 tests holds: `Codeunit.Run(Id)` (Dynamic.al:5: dynamic-target and an
/// unresolved object run), `V.Ping()` (Dynamic.al:10: dynamic-target), the
/// `TestPage` local and `Card.OpenView` (Tests.al:59-61: test-page,
/// unresolved-receiver-type), the `TestPage` parameter of the handler
/// (Tests.al:75: test-page) and `Page.RunModal` (Ledger.al:158: an unresolved
/// object run and unfetched-dependency).
#[test]
fn traversal_incomplete_is_partial() {
    let (_, doc) = with_tests(&graph_ws(), &routines(MAIN));
    let c = &block(&doc)["coverage"];
    assert_eq!(c["status"], "partial", "{c}");
    // No `interface-open-world`: `TaxRate.RatePct()` (Tax.al:31) dispatches over a
    // closed set of three workspace candidates, which is followed.
    assert_eq!(
        c["reasons"],
        serde_json::json!([
            "dynamic-target",
            "object-run-unresolved",
            "test-page",
            "unfetched-dependency",
            "unresolved-receiver-type",
        ]),
        "{c}"
    );
    // Incomplete rows: CardShowsRecord, OpenCardModally (test-page; also the
    // unresolved object run under it), DynamicRun, VariantPing.
    assert_eq!(c["testsWithUnresolvedTraversal"].as_u64(), Some(4), "{c}");
    // Dependency boundary: only OpenCardModally's Page.RunModal row.
    assert_eq!(c["testsCallingDependencyCode"].as_u64(), Some(1), "{c}");
}

/// Row 4: a `[Test]` procedure and a helper of a `Subtype = Test` codeunit are
/// removed from the count; the production routine stays.
#[test]
fn test_code_is_excluded() {
    let (_, doc) = with_tests(
        &graph_ws(),
        &routines(&["BonusAboveThousand", "CardHandler", "ExportLine"]),
    );
    let b = block(&doc);
    assert_eq!(b["excludedTestCode"].as_u64(), Some(2), "{b}");
    assert_eq!(b["changedRoutines"].as_u64(), Some(1), "{b}");
    assert_eq!(covered(&b), vec![], "{b}");
    assert_eq!(
        uncovered(&b),
        vec![unc("ExportLine", "Codeunit Acc Legacy Export", "no-path")],
        "{b}"
    );
}

/// Row 7: only test code selected -> changedRoutines 0 and `"ratio": null`, the
/// key present in the emitted text (the serializer otherwise drops nulls).
#[test]
fn only_test_code_gives_null_ratio() {
    let (raw, doc) = with_tests(
        &graph_ws(),
        &routines(&["BonusAboveThousand", "CardHandler"]),
    );
    let b = block(&doc);
    assert_eq!(b["changedRoutines"].as_u64(), Some(0), "{b}");
    assert_eq!(b["excludedTestCode"].as_u64(), Some(2), "{b}");
    assert!(
        b.as_object().unwrap().contains_key("ratio"),
        "ratio key absent: {b}"
    );
    assert!(
        raw.contains("\"ratio\": null"),
        "raw text lacks `\"ratio\": null`: {raw}"
    );
}

/// Row 5: an interface method declaration (Tax.al:3) stays in the count and is
/// uncovered `declaration-only`; the implementation it dispatches to (Tax.al:44,
/// `Acc Tax DK`) is covered through the interface candidates of Tax.al:31.
/// Declaration-only roots come from `--diff` (a `--routine` selector names only
/// the three implementations).
#[test]
fn interface_declaration_is_declaration_only() {
    let diff = diff_file("tax-interface.diff");
    let args = vec!["--diff".to_string(), diff.display().to_string()];
    let (_, doc) = with_tests(&graph_ws(), &args);
    let b = block(&doc);
    assert_eq!(b["changedRoutines"].as_u64(), Some(2), "{b}");
    assert_eq!(
        covered(&b),
        vec![cov("RatePct", &["GermanTax", "InvoiceTotalRounds"], false)],
        "{b}"
    );
    assert_eq!(
        uncovered(&b),
        vec![unc("RatePct", "Interface Acc Tax Rate", "declaration-only")],
        "{b}"
    );
}

/// The consumer's shape: `--diff` touching two production routines (ApplyDecay,
/// ExportLine) and one test procedure (GermanTax).
#[test]
fn diff_input_exact_sets() {
    let diff = diff_file("test-selection-graph.diff");
    let args = vec!["--diff".to_string(), diff.display().to_string()];
    let (_, doc) = with_tests(&graph_ws(), &args);
    let b = block(&doc);
    assert_eq!(b["changedRoutines"].as_u64(), Some(2), "{b}");
    assert_eq!(b["excludedTestCode"].as_u64(), Some(1), "{b}");
    assert_eq!(
        b["testProceduresConsidered"].as_u64(),
        Some(TEST_COUNT),
        "{b}"
    );
    assert_eq!(
        covered(&b),
        vec![cov("ApplyDecay", &["NightlyRecalcDecays"], false)],
        "{b}"
    );
    assert_eq!(
        uncovered(&b),
        vec![unc("ExportLine", "Codeunit Acc Legacy Export", "no-path")],
        "{b}"
    );
}

/// Row 7: a workspace with no tests -> 0 tests considered, everything uncovered,
/// ratio 0, nothing traversed so the block is complete.
#[test]
fn no_tests_workspace() {
    let (_, doc) = with_tests(&no_tests_ws(), &routines(&["NetPrice", "Discount"]));
    let b = block(&doc);
    assert_eq!(b["testProceduresConsidered"].as_u64(), Some(0), "{b}");
    assert_eq!(b["changedRoutines"].as_u64(), Some(2), "{b}");
    assert_eq!(b["ratio"].as_f64(), Some(0.0), "{b}");
    assert_eq!(covered(&b), vec![], "{b}");
    assert_eq!(
        uncovered(&b),
        vec![
            unc("Discount", "Codeunit Nt Pricing", "no-path"),
            unc("NetPrice", "Codeunit Nt Pricing", "no-path"),
        ],
        "{b}"
    );
    let c = &b["coverage"];
    assert_eq!(c["status"], "complete", "{c}");
    assert_eq!(c["reasons"], serde_json::json!([]), "{c}");
    assert_eq!(c["testsWithUnresolvedTraversal"].as_u64(), Some(0), "{c}");
    assert_eq!(c["testsCallingDependencyCode"].as_u64(), Some(0), "{c}");
}

/// Row 6: two `--deterministic` runs are byte-identical, on a selection where
/// several tests reach the same routine (TaxAmount, PointsFor,
/// AddBonusOnCalcPoints: the witness ordering is exercised).
#[test]
fn deterministic_runs_are_byte_identical() {
    let (a, _) = with_tests(&graph_ws(), &routines(MAIN));
    let (b, _) = with_tests(&graph_ws(), &routines(MAIN));
    assert_eq!(a, b, "two --deterministic runs differ");
}

/// Without `--tests` the document has no `testCoverage` key, and the block is the
/// flag's only addition: removing it gives the flag-off document exactly.
#[test]
fn without_flag_no_block() {
    let args = routines(MAIN);
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let (raw_off, off) = digest_raw(&graph_ws(), &a);
    assert!(
        !raw_off.contains("testCoverage"),
        "flag off emits testCoverage: {raw_off}"
    );
    let (_, mut on) = with_tests(&graph_ws(), &args);
    assert!(
        on["payload"]
            .as_object_mut()
            .unwrap()
            .remove("testCoverage")
            .is_some(),
        "flag on lacks payload.testCoverage: {on}"
    );
    assert_eq!(on, off, "--tests changed more than payload.testCoverage");
}

/// `alsem digest <ws> <args> --format human` -> stdout. Panics when the run fails.
fn digest_human(ws: &Path, args: &[String]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("digest")
        .arg(ws)
        .args(args)
        .args(["--format", "human", "--deterministic"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "alsem digest {args:?} exited {:?}: stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).replace('\r', "")
}

/// Review Focus 5: human output with `--tests` adds exactly one summary line, in
/// the spec's wording; without the flag there is none.
#[test]
fn human_line_with_tests() {
    let mut args = routines(MAIN);
    let off = digest_human(&graph_ws(), &args);
    assert!(!off.contains("test coverage:"), "flag off: {off}");
    args.push("--tests".to_string());
    let on = digest_human(&graph_ws(), &args);
    let lines: Vec<&str> = on
        .lines()
        .filter(|l| l.starts_with("test coverage:"))
        .collect();
    assert_eq!(
        lines,
        vec![
            "test coverage: 6 of 10 changed routines reached; 11 [Test] procedures \
             considered (partial: dynamic-target, object-run-unresolved, test-page, \
             unfetched-dependency, unresolved-receiver-type)"
        ],
        "{on}"
    );
    // The line is the flag's only addition to the human text.
    assert_eq!(on.replace(&format!("{}\n", lines[0]), ""), off, "{on}");
}

/// Matrix row 4: a routine that `roots.config.json` declares with kind
/// `test-procedure` but that has no `[Test]` attribute is not a test root. It
/// stays in the count (uncovered: nothing calls it), and only the one `[Test]`
/// procedure is considered.
#[test]
fn configured_test_kind_is_not_a_root() {
    let ws = fixtures().join("digest-test-coverage-configured-root");
    let (_, doc) = with_tests(&ws, &routines(&["Configured", "Charged"]));
    let b = block(&doc);
    assert_eq!(b["testProceduresConsidered"].as_u64(), Some(1), "{b}");
    assert_eq!(b["excludedTestCode"].as_u64(), Some(0), "{b}");
    assert_eq!(b["changedRoutines"].as_u64(), Some(2), "{b}");
    assert_eq!(
        uncovered(&b),
        vec![unc("Configured", "Codeunit Cr Billing", "no-path")],
        "{b}"
    );
    assert_eq!(b["covered"][0]["display"], "Charged", "{b}");
}
