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
//! - T6: two d60 findings in one routine on record variables `P1` and `P2` (key
//!   `d60/{rid}/{loopId}/{var}`) keep different loose keys: the variable after the
//!   loop segment is not a site id to strip (final review CR1).
//!
//! T3 (the detector part) is a unit test in `src/engine/l5/fingerprint.rs`: on the
//! production path the detector part is redundant with the key's `dNN/` prefix.
//!
//! #252: the detectors that state a typed loose subject get a test per row of the
//! spec's acceptance matrix (`docs/superpowers/specs/2026-10-24-issue-252-loose-subjects-design.md`):
//! a move keeps the key, and the subject's parts (callee, caller table, events,
//! loop object, …) keep two findings apart. The `d45` dependency-publisher case is
//! `d45_dep_anchor_switch` in `tests/r4/r4_cross_app_program.rs`.

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

/// `app.json` plus each `(relative path, text)` file.
fn write_workspace(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir.join("src")).expect("create src dir");
    std::fs::write(dir.join("app.json"), APP_JSON).expect("write app.json");
    for (path, text) in files {
        std::fs::write(dir.join(path), text).expect("write workspace file");
    }
}

fn temp_workspace(tag: &str, codeunit: &str) -> (tempfile::TempDir, PathBuf) {
    temp_workspace_files(
        tag,
        &[("src/Codeunit.al", codeunit), ("src/Tables.al", TABLES)],
    )
}

fn temp_workspace_files(tag: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-247-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path().to_path_buf();
    write_workspace(&root, files);
    (tmp, root)
}

fn analyze_json(ws: &Path) -> serde_json::Value {
    analyze_json_with(ws, None, false)
}

/// `detector`: the `--detector` selection (`None` = the default set).
/// `with_evidence`: emit `evidencePath` (read by the #252 move preconditions).
fn analyze_json_with(ws: &Path, detector: Option<&str>, with_evidence: bool) -> serde_json::Value {
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
        with_evidence,
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
    let json = analyze_json_with(tmp.path(), Some(d64), false);
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

/// T6: one upgrade trigger, two row-by-row loops on `P1` and `P2` (same table).
const T6: &str = r#"
codeunit 50248 Loose247Upgrade
{
    Subtype = Upgrade;

    trigger OnUpgradePerCompany()
    var
        P1: Record Item;
        P2: Record Item;
    begin
        if P1.FindSet() then
            repeat
                P1."No." := 'X';
                P1.Modify();
            until P1.Next() = 0;
        if P2.FindSet() then
            repeat
                P2."No." := 'Y';
                P2.Modify();
            until P2.Next() = 0;
    end;
}
"#;

/// T6 (final review CR1/CR8): a variable named like a site id (`p1`, `p2`) after
/// the loop segment of d60's key is the subject, so the two findings keep different
/// loose keys. The old chain walk stripped `/loop0/p1` and `/loop1/p2` alike.
#[test]
fn t6_variable_after_the_loop_segment_is_kept() {
    let (_g, ws) = temp_workspace("t6", T6);
    // d60 is opt-in (the `bcquality` preset), so select it.
    let d60 = "d60-upgrade-loop-should-be-datatransfer";
    let fs = findings_of(&analyze_json_with(&ws, Some(d60), false), d60);
    assert_eq!(fs.len(), 2, "two d60 findings expected: {fs:#?}");
    // Precondition: one routine, same tables.
    assert_eq!(
        fs[0]["primaryLocation"]["routineId"],
        fs[1]["primaryLocation"]["routineId"]
    );
    assert_eq!(fs[0]["affectedTables"], fs[1]["affectedTables"]);
    assert_ne!(
        str_of(&fs[0], "looseFingerprint"),
        str_of(&fs[1], "looseFingerprint"),
    );
}

// ---------------------------------------------------------------------------
// #252: each detector states its loose subject.
// ---------------------------------------------------------------------------

/// The #252 tables: `Customer` and `Item`, each with `Name` and `Address`.
const TABLES_252: &str = r#"
table 18 Customer
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
        field(3; Address; Text[50]) { }
    }
}

table 27 Item
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
        field(3; Address; Text[50]) { }
    }
}
"#;

/// One #252 workspace: `code` plus [`TABLES_252`].
fn ws_252(tag: &str, code: &str) -> (tempfile::TempDir, PathBuf) {
    temp_workspace_files(tag, &[("src/Code.al", code), ("src/Tables.al", TABLES_252)])
}

/// The findings of `detector` in `code`, with evidence.
fn run_252(tag: &str, code: &str, detector: &str) -> Vec<serde_json::Value> {
    let (_g, ws) = ws_252(tag, code);
    findings_of(&analyze_json_with(&ws, Some(detector), true), detector)
}

fn loc<'a>(f: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    &f["primaryLocation"][key]
}

fn step_routine(f: &serde_json::Value, i: usize) -> &str {
    f["evidencePath"][i]["routineId"]
        .as_str()
        .unwrap_or_else(|| panic!("no evidencePath[{i}].routineId: {f:#}"))
}

/// The object part (`{app}:{type}:{number}`) of a stable routine id.
fn object_of_routine(rid: &str) -> &str {
    rid.split('#').next().unwrap()
}

fn loose(f: &serde_json::Value) -> &str {
    str_of(f, "looseFingerprint")
}

/// A move: one finding per run, same object, `fingerprint` differs,
/// `looseFingerprint` equal.
fn assert_move(a: &serde_json::Value, b: &serde_json::Value) {
    assert_eq!(
        loc(a, "objectId"),
        loc(b, "objectId"),
        "precondition: one object"
    );
    assert_ne!(str_of(a, "fingerprint"), str_of(b, "fingerprint"));
    assert_eq!(loose(a), loose(b), "a move keeps looseFingerprint");
}

// --- d39-d42: the called routine, its parameter position, the caller's table ---

/// A d39-d42 shape: the caller body (`{v}` the record variable, `{h}` the helper)
/// and the helper body (on its record parameter `R`).
struct Dnn {
    detector: &'static str,
    caller: &'static str,
    helper: &'static str,
}

const D39: Dnn = Dnn {
    detector: "d39-record-left-dirty-across-chain",
    caller: "{v}.Get('A');\n        {h}({v});",
    helper: "R.Validate(Name, 'X');\n        exit;",
};
const D40: Dnn = Dnn {
    detector: "d40-transitive-load-missing",
    caller: "{h}({v});",
    helper: "R.Modify();",
};
const D41: Dnn = Dnn {
    detector: "d41-transitive-filter-loss",
    caller: "{v}.SetRange(\"No.\", 'A');\n        {h}({v});\n        {v}.DeleteAll();",
    helper: "R.Reset();",
};
const D42: Dnn = Dnn {
    detector: "d42-cross-call-wrong-setloadfields",
    caller: "{v}.SetLoadFields(\"No.\", Name);\n        {v}.FindFirst();\n        {h}({v});",
    helper: "Message(R.Address);",
};

/// A caller procedure: `(name, Some((var, table, helper)))`, or `None` for an empty one.
type Caller<'a> = (&'a str, Option<(&'a str, &'a str, &'a str)>);

/// One codeunit. `helpers`: `(name, parameter type)`.
fn dnn_code(d: &Dnn, callers: &[Caller], helpers: &[(&str, &str)]) -> String {
    let mut s = String::from("codeunit 50252 L252\n{\n");
    for (name, body) in callers {
        match body {
            Some((v, table, h)) => {
                let code = d.caller.replace("{v}", v).replace("{h}", h);
                s.push_str(&format!(
                    "    procedure {name}()\n    var {v}: Record {table};\n    begin\n        {code}\n    end;\n\n"
                ));
            }
            None => s.push_str(&format!("    procedure {name}()\n    begin\n    end;\n\n")),
        }
    }
    for (name, ty) in helpers {
        s.push_str(&format!(
            "    local procedure {name}(var R: {ty})\n    begin\n        {}\n    end;\n\n",
            d.helper
        ));
    }
    s.push_str("}\n");
    s
}

/// The callee a d39-d42 finding names. d39/d40/d42 put it at `evidencePath[1]`;
/// d41's evidence stays in the caller, so its callee is read from that step's note
/// (`forwards {v} to {callee}, which calls Reset`).
fn dnn_callee(d: &Dnn, f: &serde_json::Value) -> String {
    if d.detector == D41.detector {
        let note = str_of(&f["evidencePath"][1], "note");
        let after = note
            .split(" to ")
            .nth(1)
            .expect("d41 note names the callee");
        after.split(',').next().unwrap().to_string()
    } else {
        step_routine(f, 1).to_string()
    }
}

/// `dNN_move_rename`: the caller code moves from `A` to `B` and its variable is
/// renamed; the anchor (the caller) moves with it.
fn dnn_move_rename(d: &Dnn) {
    let helpers = [("Helper", "Record Customer")];
    let a = dnn_code(
        d,
        &[("A", Some(("Cust", "Customer", "Helper"))), ("B", None)],
        &helpers,
    );
    let b = dnn_code(
        d,
        &[("A", None), ("B", Some(("C", "Customer", "Helper")))],
        &helpers,
    );
    let fa = run_252("mv-a", &a, d.detector);
    let fb = run_252("mv-b", &b, d.detector);
    assert_eq!((fa.len(), fb.len()), (1, 1), "{fa:#?}\n{fb:#?}");
    assert_eq!(loc(&fa[0], "routineName"), "A");
    assert_eq!(loc(&fb[0], "routineName"), "B");
    assert_move(&fa[0], &fb[0]);
}

/// `dNN_callees_distinct`: two callers, two different callees, same table and index.
fn dnn_callees_distinct(d: &Dnn) {
    let code = dnn_code(
        d,
        &[
            ("A", Some(("C", "Customer", "HelperA"))),
            ("B", Some(("C", "Customer", "HelperB"))),
        ],
        &[
            ("HelperA", "Record Customer"),
            ("HelperB", "Record Customer"),
        ],
    );
    let fs = run_252("callees", &code, d.detector);
    assert_eq!(fs.len(), 2, "{fs:#?}");
    assert_ne!(
        dnn_callee(d, &fs[0]),
        dnn_callee(d, &fs[1]),
        "precondition: two callees"
    );
    assert_eq!(fs[0]["affectedTables"], fs[1]["affectedTables"]);
    assert_ne!(loose(&fs[0]), loose(&fs[1]));
}

/// `dNN_same_callee_tables_distinct`: one callee, called with a `Customer` and with
/// an `Item` at the same index. `param` is the callee's parameter type.
///
/// With `Record Customer` this code is COMPILE-INVALID ON PURPOSE: the AL compiler
/// rejects passing an `Item` to a `var Record Customer` parameter, but the file
/// parses cleanly and the engine still builds both findings. That is exactly the
/// shape that defeats callee + index alone, so the caller's table must be in the key.
/// `Record` (no table, not valid AL either; reached through parser recovery) is the
/// P5 shape, which only d41 reports.
fn dnn_same_callee_tables_distinct(d: &Dnn, param: &str) {
    let code = dnn_code(
        d,
        &[
            ("CallerC", Some(("C", "Customer", "Helper"))),
            ("CallerI", Some(("I", "Item", "Helper"))),
        ],
        &[("Helper", param)],
    );
    let fs = run_252("tables", &code, d.detector);
    assert_eq!(fs.len(), 2, "{fs:#?}");
    assert_eq!(
        dnn_callee(d, &fs[0]),
        dnn_callee(d, &fs[1]),
        "precondition: one callee"
    );
    assert_ne!(loose(&fs[0]), loose(&fs[1]));
}

#[test]
fn d39_move_rename() {
    dnn_move_rename(&D39);
}
#[test]
fn d40_move_rename() {
    dnn_move_rename(&D40);
}
#[test]
fn d41_move_rename() {
    dnn_move_rename(&D41);
}
#[test]
fn d42_move_rename() {
    dnn_move_rename(&D42);
}
#[test]
fn d39_callees_distinct() {
    dnn_callees_distinct(&D39);
}
#[test]
fn d40_callees_distinct() {
    dnn_callees_distinct(&D40);
}
#[test]
fn d41_callees_distinct() {
    dnn_callees_distinct(&D41);
}
#[test]
fn d42_callees_distinct() {
    dnn_callees_distinct(&D42);
}
#[test]
fn d39_same_callee_tables_distinct() {
    dnn_same_callee_tables_distinct(&D39, "Record Customer");
}
#[test]
fn d40_same_callee_tables_distinct() {
    dnn_same_callee_tables_distinct(&D40, "Record Customer");
}
#[test]
fn d41_same_callee_tables_distinct() {
    dnn_same_callee_tables_distinct(&D41, "Record Customer");
}
#[test]
fn d42_same_callee_tables_distinct() {
    dnn_same_callee_tables_distinct(&D42, "Record Customer");
}
/// The P5 generic-`Record` callee (extra d41 case).
#[test]
fn d41_generic_record_tables_distinct() {
    dnn_same_callee_tables_distinct(&D41, "Record");
}

// --- d7: the events and objects of the cycle ---

const D7: &str = "d7-recursive-event-expansion";

/// The `ws-d7-event-cycle` shape with `SubB` named `sub_b`.
fn d7_one_codeunit(sub_b: &str) -> String {
    format!(
        r#"codeunit 50117 D7EventCycle
{{
    [IntegrationEvent(false, false)]
    procedure OnA() begin end;

    procedure RaiseA() begin OnA(); end;

    [IntegrationEvent(false, false)]
    procedure OnB() begin end;

    procedure RaiseB() begin OnB(); end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::D7EventCycle, 'OnA', '', true, true)]
    local procedure SubA() begin RaiseB(); end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::D7EventCycle, 'OnB', '', true, true)]
    local procedure {sub_b}() begin RaiseA(); end;
}}
"#
    )
}

/// `d7_move`: `SubB` renamed `SubB2` (its code moves to another routine).
#[test]
fn d7_move() {
    let fa = run_252("d7-a", &d7_one_codeunit("SubB"), D7);
    let fb = run_252("d7-b", &d7_one_codeunit("SubB2"), D7);
    assert_eq!((fa.len(), fb.len()), (1, 1), "{fa:#?}\n{fb:#?}");
    assert_ne!(
        str_of(&fa[0], "id"),
        str_of(&fb[0], "id"),
        "precondition: raw id differs"
    );
    assert_move(&fa[0], &fb[0]);
}

/// The P6 two-object cycle; `D7Y`'s subscriber to `D7X.OnA` is named `sub`.
fn d7_two_objects(sub: &str) -> String {
    format!(
        r#"codeunit 50260 D7X
{{
    [IntegrationEvent(false, false)]
    procedure OnA() begin end;

    procedure RaiseA() begin OnA(); end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::D7Y, 'OnB', '', true, true)]
    local procedure SubB() begin RaiseA(); end;
}}
codeunit 50261 D7Y
{{
    [IntegrationEvent(false, false)]
    procedure OnB() begin end;

    procedure RaiseB() begin OnB(); end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::D7X, 'OnA', '', true, true)]
    local procedure {sub}() begin RaiseB(); end;
}}
"#
    )
}

/// `d7_anchor_object_switch`: renaming `SubA` to `Q3` inside `D7Y` moves the anchor
/// (the first member by internal id) from `D7X` to `D7Y` (P6). The loose key follows
/// the cycle's objects, not the anchor. Internal routine ids hash the app id, so the
/// pair holds for P6's app id only: the test uses that `app.json`.
#[test]
fn d7_anchor_object_switch() {
    const P6_APP: &str = r#"{"id":"11111111-0000-0000-0000-000000000252","name":"P252 d60a","publisher":"PT","version":"1.0.0.0","dependencies":[]}"#;
    let run = |tag: &str, sub: &str| {
        let code = d7_two_objects(sub);
        let files = [("app.json", P6_APP), ("src/Code.al", code.as_str())];
        let (_g, ws) = temp_workspace_files(tag, &files);
        findings_of(&analyze_json_with(&ws, Some(D7), true), D7)
    };
    let fa = run("d7x-a", "SubA");
    let fb = run("d7x-b", "Q3");
    assert_eq!((fa.len(), fb.len()), (1, 1), "{fa:#?}\n{fb:#?}");
    assert_ne!(
        loc(&fa[0], "objectId"),
        loc(&fb[0], "objectId"),
        "precondition: the anchor moves to the other object"
    );
    assert_eq!(loose(&fa[0]), loose(&fb[0]));
}

/// `d7_events_distinct`: one codeunit, two cycles over different events.
#[test]
fn d7_events_distinct() {
    let mut code = String::from("codeunit 50117 C\n{\n");
    for (x, y) in [("A", "B"), ("C", "D")] {
        code.push_str(&format!(
            "    [IntegrationEvent(false, false)]\n    procedure On{x}() begin end;\n    procedure Raise{x}() begin On{x}(); end;\n    [IntegrationEvent(false, false)]\n    procedure On{y}() begin end;\n    procedure Raise{y}() begin On{y}(); end;\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::C, 'On{x}', '', true, true)]\n    local procedure Sub{x}() begin Raise{y}(); end;\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::C, 'On{y}', '', true, true)]\n    local procedure Sub{y}() begin Raise{x}(); end;\n"
        ));
    }
    code.push_str("}\n");
    let fs = run_252("d7-two", &code, D7);
    assert_eq!(fs.len(), 2, "{fs:#?}");
    assert_eq!(loc(&fs[0], "objectId"), loc(&fs[1], "objectId"));
    assert_ne!(loose(&fs[0]), loose(&fs[1]));
}

// --- d45: the publisher's events, on the publisher's object ---

/// `d45_publishers_distinct` (P2): two primary integration events of one codeunit,
/// each with a subscriber inserting `Customer`. Master stripped both publishers to
/// `d45/~|{table}`.
#[test]
fn d45_publishers_distinct() {
    let code = r#"
codeunit 50254 P252Pub
{
    [IntegrationEvent(false, false)]
    procedure OnFirst()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnSecond()
    begin
    end;
}

codeunit 50255 P252Sub
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::P252Pub, 'OnFirst', '', false, false)]
    local procedure HandleFirst()
    var C: Record Customer;
    begin
        C."No." := 'A';
        C.Insert();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::P252Pub, 'OnSecond', '', false, false)]
    local procedure HandleSecond()
    var C: Record Customer;
    begin
        C."No." := 'B';
        C.Insert();
    end;
}
"#;
    let d45 = "d45-event-transitive-table-exposure";
    let fs = run_252("d45", code, d45);
    assert_eq!(fs.len(), 2, "{fs:#?}");
    assert_eq!(loc(&fs[0], "objectId"), loc(&fs[1], "objectId"));
    assert_eq!(fs[0]["affectedTables"], fs[1]["affectedTables"]);
    assert_ne!(loc(&fs[0], "routineId"), loc(&fs[1], "routineId"));
    assert_ne!(loose(&fs[0]), loose(&fs[1]));
}

// --- d48: the loop's object, the IO kind and method ---

const D48: &str = "d48-io-in-loop";

/// Codeunit `H` with `Send()`, which does one HTTP send.
const D48_H: &str = r#"
codeunit 50300 H
{
    procedure Send()
    var
        Client: HttpClient;
        Req: HttpRequestMessage;
        Resp: HttpResponseMessage;
    begin
        Client.Send(Req, Resp);
    end;
}
"#;

/// A procedure `name` whose loop calls `H.Send`.
fn d48_loop(name: &str) -> String {
    format!(
        "    procedure {name}()\n    var H: Codeunit H; i: Integer;\n    begin\n        repeat i += 1; H.Send(); until i >= 10;\n    end;\n"
    )
}

/// `d48_move_transitive`: the loop calling `H.Send` moves from `A.P` to `A.Q`. The
/// anchor (the IO in `H`) stays; the loop routine (`evidencePath[0]`) moves.
#[test]
fn d48_move_transitive() {
    let empty = |n: &str| format!("    procedure {n}()\n    begin\n    end;\n");
    let a = format!(
        "{D48_H}codeunit 50301 A\n{{\n{}{}}}\n",
        d48_loop("P"),
        empty("Q")
    );
    let b = format!(
        "{D48_H}codeunit 50301 A\n{{\n{}{}}}\n",
        empty("P"),
        d48_loop("Q")
    );
    let fa = run_252("d48-mv-a", &a, D48);
    let fb = run_252("d48-mv-b", &b, D48);
    assert_eq!((fa.len(), fb.len()), (1, 1), "{fa:#?}\n{fb:#?}");
    assert_eq!(
        loc(&fa[0], "routineId"),
        loc(&fb[0], "routineId"),
        "precondition: anchor stays"
    );
    assert_ne!(
        step_routine(&fa[0], 0),
        step_routine(&fb[0], 0),
        "precondition: loop moves"
    );
    assert_move(&fa[0], &fb[0]);
}

/// `d48_direct_to_transitive`: `A.P`'s loop sends itself vs calls `A.Helper`, which
/// sends. Anchors `A.P` vs `A.Helper`, one object.
#[test]
fn d48_direct_to_transitive() {
    let vars = "var Client: HttpClient; Req: HttpRequestMessage; Resp: HttpResponseMessage;";
    let direct = format!(
        "codeunit 50301 A\n{{\n    procedure P()\n    {vars} i: Integer;\n    begin\n        repeat i += 1; Client.Send(Req, Resp); until i >= 10;\n    end;\n}}\n"
    );
    let transitive = format!(
        "codeunit 50301 A\n{{\n    procedure P()\n    var i: Integer;\n    begin\n        repeat i += 1; Helper(); until i >= 10;\n    end;\n\n    local procedure Helper()\n    {vars}\n    begin\n        Client.Send(Req, Resp);\n    end;\n}}\n"
    );
    let fa = run_252("d48-dir", &direct, D48);
    let fb = run_252("d48-tr", &transitive, D48);
    assert_eq!((fa.len(), fb.len()), (1, 1), "{fa:#?}\n{fb:#?}");
    assert_eq!(loc(&fa[0], "routineName"), "P");
    assert_eq!(loc(&fb[0], "routineName"), "Helper");
    assert_move(&fa[0], &fb[0]);
}

/// `d48_loop_objects_distinct`: codeunits `A` and `B` each loop calling `H.Send`;
/// both findings anchor on the IO in `H`.
#[test]
fn d48_loop_objects_distinct() {
    let code = format!(
        "{D48_H}codeunit 50301 A\n{{\n{}}}\ncodeunit 50302 B\n{{\n{}}}\n",
        d48_loop("P"),
        d48_loop("P")
    );
    let fs = run_252("d48-two", &code, D48);
    assert_eq!(fs.len(), 2, "{fs:#?}");
    assert_eq!(
        loc(&fs[0], "routineId"),
        loc(&fs[1], "routineId"),
        "precondition: one anchor"
    );
    assert_ne!(
        object_of_routine(step_routine(&fs[0], 0)),
        object_of_routine(step_routine(&fs[1], 0)),
        "precondition: loops in two objects"
    );
    assert_ne!(loose(&fs[0]), loose(&fs[1]));
}
