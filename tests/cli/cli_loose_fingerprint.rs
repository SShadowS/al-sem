//! #247: `looseFingerprint` on `alsem analyze --format json` findings — a key that
//! stays equal when the same problem moves to another routine of the same object,
//! while `fingerprint` keeps the routine.
//!
//! Every test drives the REAL analyze pipeline (`run_analyze_with_exit`) over a
//! scratch workspace in a temp dir (not an r0-corpus fixture: the corpus is walked by
//! the golden differentials, which would then demand goldens).
//!
//! - T1: the d11 problem moved from `BadNoGet` to `Other` (behind one more record op,
//!   so its `/op{N}` ordinal differs) keeps `looseFingerprint` and changes `fingerprint`.
//! - T2: two d11 findings in one object on different tables have different loose keys.
//! - T4: the loose key is deterministic across runs and workspace paths (pinned literal).
//! - T5: an object-level finding (d64, no routine anchor) has `looseFingerprint ==
//!   fingerprint`.
//!
//! T3 (the detector part) is a unit test in `src/engine/l5/fingerprint.rs`: on the
//! production path the detector part is redundant with the key's `dNN/` prefix.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

const APP_JSON: &str = r#"{
    "id": "11111111-0000-0000-0000-000000000247",
    "name": "Loose 247",
    "publisher": "PT",
    "version": "1.0.0.0",
    "dependencies": []
}
"#;

const TABLES: &str = r#"
table 18 Customer
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}

table 27 Item
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}
"#;

/// T1-A: the bad `Modify` (never loaded) sits in `BadNoGet`; `Other` is empty.
const T1_A: &str = r#"
codeunit 50247 Loose247
{
    procedure BadNoGet()
    var Customer: Record Customer;
    begin
        Customer."No." := 'X';
        Customer.Modify();
    end;

    procedure Other()
    begin
    end;
}
"#;

/// T1-B: the same bad code moved into `Other`, behind one more record op on a
/// separate variable, so the moved op's walk-order ordinal differs.
const T1_B: &str = r#"
codeunit 50247 Loose247
{
    procedure BadNoGet()
    begin
    end;

    procedure Other()
    var Loaded: Record Customer;
        Customer: Record Customer;
    begin
        Loaded.Get('Y');
        Customer."No." := 'X';
        Customer.Modify();
    end;
}
"#;

/// T2: one object, a d11 finding on `Customer` and one on `Item`.
const T2: &str = r#"
codeunit 50247 Loose247
{
    procedure BadCustomer()
    var Customer: Record Customer;
    begin
        Customer."No." := 'X';
        Customer.Modify();
    end;

    procedure BadItem()
    var Item: Record Item;
    begin
        Item."No." := 'X';
        Item.Modify();
    end;
}
"#;

/// T4's pinned `looseFingerprint` of T1-A's d11 finding:
/// `sha256("d11-modify-without-get|Codeunit/50247||<app>/table/18|d11/~/~")[..16]`.
const T1_A_LOOSE: &str = "406a88f5e0b896a6";

fn write_workspace(dir: &Path, codeunit: &str) {
    std::fs::create_dir_all(dir.join("src")).expect("create src dir");
    std::fs::write(dir.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(dir.join("src/Codeunit.al"), codeunit).expect("write codeunit");
    std::fs::write(dir.join("src/Tables.al"), TABLES).expect("write tables");
}

fn temp_workspace(tag: &str, codeunit: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-247-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path().to_path_buf();
    write_workspace(&root, codeunit);
    (tmp, root)
}

fn analyze_json(ws: &Path) -> serde_json::Value {
    analyze_json_with(ws, None)
}

/// `detector`: the `--detector` selection (`None` = the default set).
fn analyze_json_with(ws: &Path, detector: Option<&str>) -> serde_json::Value {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(str::to_string),
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
    let out = match run_analyze_with_exit(&args.read_inputs(), &args, "247-test") {
        Ok((out, _, _)) => out,
        Err(e) => panic!("run_analyze failed: {e}"),
    };
    serde_json::from_str(&out).expect("analyze JSON must parse")
}

fn findings_of(json: &serde_json::Value, detector: &str) -> Vec<serde_json::Value> {
    json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"] == detector)
        .cloned()
        .collect()
}

fn only_finding(json: &serde_json::Value, detector: &str) -> serde_json::Value {
    let fs = findings_of(json, detector);
    assert_eq!(
        fs.len(),
        1,
        "expected exactly one {detector} finding: {fs:#?}"
    );
    fs.into_iter().next().unwrap()
}

fn str_of<'a>(f: &'a serde_json::Value, key: &str) -> &'a str {
    f[key]
        .as_str()
        .unwrap_or_else(|| panic!("finding has no string `{key}`: {f:#}"))
}

fn last_segment(id: &str) -> &str {
    id.rsplit('/').next().unwrap()
}

const D11: &str = "d11-modify-without-get";

/// T1 (Acceptance 2): a moved finding keeps `looseFingerprint`, changes `fingerprint`.
#[test]
fn t1_moved_finding_keeps_loose_fingerprint() {
    let (_a_guard, a) = temp_workspace("t1a", T1_A);
    let (_b_guard, b) = temp_workspace("t1b", T1_B);
    let fa = only_finding(&analyze_json(&a), D11);
    let fb = only_finding(&analyze_json(&b), D11);

    // Precondition: the move landed in another routine at another op ordinal.
    assert_eq!(fa["primaryLocation"]["routineName"], "BadNoGet");
    assert_eq!(fb["primaryLocation"]["routineName"], "Other");
    let (oa, ob) = (
        last_segment(str_of(&fa, "id")),
        last_segment(str_of(&fb, "id")),
    );
    assert!(oa.starts_with("op") && ob.starts_with("op"), "{oa} / {ob}");
    assert_ne!(
        oa, ob,
        "precondition: the two raw ids end in different /op{{N}}"
    );

    assert_ne!(
        str_of(&fa, "fingerprint"),
        str_of(&fb, "fingerprint"),
        "fingerprint keeps the routine, so a move changes it"
    );
    assert_eq!(
        str_of(&fa, "looseFingerprint"),
        str_of(&fb, "looseFingerprint"),
        "looseFingerprint ignores the routine, so a move keeps it"
    );
}

/// T2 (Acceptance 3): same detector, same object, different tables → loose keys differ.
#[test]
fn t2_different_table_same_object_differs() {
    let (_g, ws) = temp_workspace("t2", T2);
    let json = analyze_json(&ws);
    let fs = findings_of(&json, D11);
    assert_eq!(fs.len(), 2, "two d11 findings expected: {fs:#?}");
    // Precondition: one object, two different tables.
    assert_eq!(
        fs[0]["primaryLocation"]["objectId"],
        fs[1]["primaryLocation"]["objectId"]
    );
    assert_ne!(fs[0]["affectedTables"], fs[1]["affectedTables"]);
    assert_ne!(
        str_of(&fs[0], "looseFingerprint"),
        str_of(&fs[1], "looseFingerprint"),
    );
}

/// T4 (Acceptance 4): deterministic across runs and workspace paths, and pinned.
#[test]
fn t4_loose_fingerprint_is_deterministic_and_pinned() {
    let (_g1, ws1) = temp_workspace("t4a", T1_A);
    let (_g2, ws2) = temp_workspace("t4b-other-path", T1_A);
    let run1 = only_finding(&analyze_json(&ws1), D11);
    let run2 = only_finding(&analyze_json(&ws1), D11);
    let copy = only_finding(&analyze_json(&ws2), D11);
    let loose = str_of(&run1, "looseFingerprint");
    assert_eq!(loose, str_of(&run2, "looseFingerprint"));
    assert_eq!(loose, str_of(&copy, "looseFingerprint"));
    assert_eq!(loose.len(), 16);
    assert!(loose.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(loose, T1_A_LOOSE, "pinned looseFingerprint of T1-A");
}

/// T5 (Acceptance 5): an object-level finding (d64 anchors on the page object, not a
/// routine) strips nothing and has no routine name, so `looseFingerprint == fingerprint`.
#[test]
fn t5_object_level_finding_loose_equals_fingerprint() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d64");
    let tmp = tempfile::Builder::new()
        .prefix("alsem-247-t5-")
        .tempdir()
        .expect("create scratch ws");
    crate::copy_dir(&fixture, tmp.path());
    // d64 is opt-in (the `bcquality` preset), so select it.
    let d64 = "d64-api-page-write-surface";
    let json = analyze_json_with(tmp.path(), Some(d64));
    let fs = findings_of(&json, d64);
    assert!(!fs.is_empty(), "the d64 fixture must yield findings");
    for f in &fs {
        assert!(
            f["primaryLocation"]["routineName"].is_null(),
            "precondition: object-level anchor: {f:#}"
        );
        assert_eq!(
            str_of(f, "looseFingerprint"),
            str_of(f, "fingerprint"),
            "{f:#}"
        );
    }
}
