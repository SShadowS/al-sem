//! Issue #100 acceptance tests: `TestField` is not a capability `read`, and the
//! `Rename`/`LockTable` table capability facts that keep d1/d2 honest.
//!
//! Rows (ledger `.agent/issue-100/ledger.md`, "Acceptance matrix"):
//! - A1: no capability `read` for `TestField`, through the full snapshot and the
//!   L2 workspace projection.
//! - A2: one op table serves both capability maps (agreement over the whole
//!   `RecordOpType` vocabulary) + the invariant `fact exists == db-touching class`.
//! - A2b: d1/d2 see `Rename`/`LockTable` callees (with and without `TestField`).
//! - A2c: the policy vocabulary accepts `rename` / `lock`.
//! - A2d: d2 is not starved by a known-temporary branch.
//! - A2e: a temporary record forwarded to a `var` parameter (case (e)).
//! - A2f: case (e) does not remove a FlowField path.
//!
//! Every test drives the real pipeline over AL stated in code: the snapshot
//! composer, the L2 workspace projection, the registered detectors, and
//! `policy check`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::gate::policy::pipeline::{PolicyCheckOptions, run_policy_check};
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::op_classification::{classify_op, is_db_touching_class};
use al_sem::engine::l5::registry::run_detectors;
use al_sem::engine::l5::snapshot_full::{
    FullSnapshotOptions, compose_full_snapshot, serialize_json,
};
use al_sem::program::body::l2_workspace::project_workspace;
use al_sem::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
};
use al_sem::program::model::workspace::Model;
use al_sem::record_ops::record_op_type;
use serde_json::Value;

const APP_GUID: &str = "11111111-0000-0000-0000-000000100100";

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// Write `files` + an `app.json` into a fresh temp dir (the on-disk workspace
/// the snapshot, L2 and policy pipelines read).
fn write_ws(files: &[(String, String)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp ws");
    let app = serde_json::json!({
        "id": APP_GUID, "name": "I100", "publisher": "I100", "version": "1.0.0.0",
    });
    std::fs::write(dir.path().join("app.json"), app.to_string()).unwrap();
    for (rel, src) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    dir
}

fn resolve(dir: &Path) -> Model {
    let model_id = compute_gate_model_instance_id(dir).expect("modelInstanceId");
    assemble_and_resolve_workspace_program(dir, &model_id, true).expect("resolve")
}

fn full_snapshot_json(dir: &Path, resolved: &Model) -> Value {
    let opts = FullSnapshotOptions {
        workspace_dir: dir,
        driver_version: "i100",
        deterministic: true,
        roots_config_ignored: false,
    };
    serde_json::from_str(&serialize_json(&compose_full_snapshot(resolved, &opts))).unwrap()
}

/// Every JSON object (anywhere in `v`) for which `pred` holds.
fn collect<'a>(v: &'a Value, pred: &dyn Fn(&Value) -> bool, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            if pred(v) {
                out.push(v);
            }
            m.values().for_each(|c| collect(c, pred, out));
        }
        Value::Array(a) => a.iter().for_each(|c| collect(c, pred, out)),
        _ => {}
    }
}

/// The snapshot's `capabilityFacts` array.
fn capability_facts(snap: &Value) -> Vec<&Value> {
    let mut arrays = Vec::new();
    fn find<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(m) => {
                for (k, c) in m {
                    if k == "capabilityFacts" {
                        out.push(c);
                    } else {
                        find(c, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|c| find(c, out)),
            _ => {}
        }
    }
    find(snap, &mut arrays);
    assert_eq!(arrays.len(), 1, "exactly one capabilityFacts array");
    arrays[0].as_array().unwrap().iter().collect()
}

fn stable_id(resolved: &Model, name: &str) -> String {
    let hits: Vec<_> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(hits.len(), 1, "one routine named {name}");
    hits[0].stable_routine_id.clone()
}

/// Map every record-operation id in the model to its `RecordOpType`.
fn op_type_by_id(resolved: &Model) -> BTreeMap<String, String> {
    resolved
        .workspace
        .routines
        .iter()
        .flat_map(|r| {
            r.record_operations
                .iter()
                .map(|o| (o.id.clone(), o.op.clone()))
        })
        .collect()
}

fn run_detector(name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == name)
        .collect();
    assert_eq!(selected.len(), 1, "{name} registered once");
    run_detectors(&resolved, &selected).findings
}

fn summarize(findings: &[Finding]) -> Vec<(String, String)> {
    findings
        .iter()
        .map(|f| (f.id.clone(), f.root_cause.clone()))
        .collect()
}

const ITEM_TABLE: &str = r#"
table 50300 "I100 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[50]) { }
        field(3; Amount; Decimal) { }
        field(4; "Line Count"; Integer)
        {
            FieldClass = FlowField;
            CalcFormula = count("I100 Line" where("Item No." = field("No.")));
        }
    }
    keys { key(PK; "No.") { } }
}
"#;

const LINE_TABLE: &str = r#"
table 50301 "I100 Line"
{
    fields
    {
        field(1; "Item No."; Code[20]) { }
        field(2; "Line No."; Integer) { }
    }
    keys { key(PK; "Item No.", "Line No.") { } }
}
"#;

// ===========================================================================
// A1 — TestField produces no capability read fact.
// ===========================================================================

const A1_CU: &str = r#"
codeunit 50310 "I100 TF"
{
    procedure OnlyTF()
    var
        Rec: Record "I100 Item";
    begin
        Rec.TestField(Name);
    end;

    procedure GetOnly()
    var
        Rec: Record "I100 Item";
    begin
        Rec.Get('A');
    end;

    procedure Mixed()
    var
        Rec: Record "I100 Item";
    begin
        Rec.Get('A');
        Rec.TestField(Name);
    end;

    procedure Caller()
    begin
        OnlyTF();
    end;

    procedure Both()
    begin
        OnlyTF();
        GetOnly();
    end;
}
"#;

const ITEM_TABLE_ID: &str = "11111111-0000-0000-0000-000000100100/table/50300";

/// A1, composed-snapshot half: the cone's direct + inherited facts and the
/// required `TableData R` rows.
#[test]
fn a1_testfield_is_not_a_capability_read_in_the_snapshot() {
    let dir = write_ws(&[
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("TF", A1_CU),
    ]);
    let resolved = resolve(dir.path());
    let snap = full_snapshot_json(dir.path(), &resolved);
    let ops = op_type_by_id(&resolved);
    let facts = capability_facts(&snap);

    // The negative is not vacuous: OnlyTF still RECORDS its TestField.
    let only_tf = resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "OnlyTF")
        .unwrap();
    assert!(
        only_tf
            .record_operations
            .iter()
            .any(|o| o.op == "TestField"),
        "OnlyTF must still record TestField: {:?}",
        only_tf.record_operations
    );

    // (subject, provenance) -> witness op types of its `read` facts on the table.
    let reads_of = |name: &str, provenance: &str| -> Vec<String> {
        let subject = stable_id(&resolved, name);
        facts
            .iter()
            .filter(|f| {
                f["subject"] == subject.as_str()
                    && f["op"] == "read"
                    && f["resourceKind"] == "table"
                    && f["resourceId"] == ITEM_TABLE_ID
                    && f["provenance"] == provenance
            })
            .map(|f| {
                let w = f["witnessOperationId"].as_str().unwrap_or("<none>");
                ops.get(w)
                    .cloned()
                    .unwrap_or_else(|| format!("<unknown {w}>"))
            })
            .collect()
    };
    let all_reads = |name: &str| -> Vec<String> {
        let mut v = reads_of(name, "direct");
        v.extend(reads_of(name, "inherited"));
        v
    };

    // Collected, so one run shows every broken expectation.
    let mut bad: Vec<String> = Vec::new();
    let mut expect = |what: &str, got: Vec<String>, want: &[&str]| {
        if got != want {
            bad.push(format!("{what}: got {got:?}, want {want:?}"));
        }
    };
    expect(
        "GetOnly direct reads",
        reads_of("GetOnly", "direct"),
        &["Get"],
    );
    // Mixed keeps exactly one read, witnessed by its Get (no TestField read).
    expect("Mixed direct reads", reads_of("Mixed", "direct"), &["Get"]);
    // Both keeps an inherited read whose witness is GetOnly's Get.
    expect(
        "Both inherited reads",
        reads_of("Both", "inherited"),
        &["Get"],
    );
    expect("OnlyTF reads", all_reads("OnlyTF"), &[]);
    expect("Caller reads", all_reads("Caller"), &[]);

    // No `TableData R` row for OnlyTF / Caller; GetOnly keeps one (control).
    let mut perms = Vec::new();
    collect(
        &snap,
        &|v| v["kind"] == "required" && v["targetKind"] == "TableData",
        &mut perms,
    );
    let has_r = |name: &str| -> bool {
        let subject = stable_id(&resolved, name);
        perms.iter().any(|p| {
            p["subject"] == subject.as_str()
                && p["rights"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|x| x == "R"))
        })
    };
    if !has_r("GetOnly") {
        bad.push(format!("control: GetOnly needs TableData R: {perms:#?}"));
    }
    for name in ["OnlyTF", "Caller"] {
        if has_r(name) {
            bad.push(format!("{name} must need no TableData R"));
        }
    }
    assert!(bad.is_empty(), "A1 snapshot:\n{}", bad.join("\n"));
}

/// A1, L2 half: the L2 workspace projection that fills `capabilityFactsDirect`
/// (the r1a projection).
#[test]
fn a1_testfield_is_not_a_capability_read_in_the_l2_projection() {
    let dir = write_ws(&[
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("TF", A1_CU),
    ]);
    let proj = project_workspace(dir.path()).expect("L2 projection");
    let routine = |name: &str| {
        proj.routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("L2 routine {name}"))
    };
    let table_reads = |name: &str| -> Vec<String> {
        routine(name)
            .capability_facts_direct
            .iter()
            .filter(|f| f.resource_kind == "table" && f.op == "read")
            .map(|f| {
                let v = serde_json::to_value(f).unwrap();
                v["extra"]["opSubtype"].as_str().unwrap_or("").to_string()
            })
            .collect()
    };
    assert!(
        routine("OnlyTF")
            .features
            .record_operations
            .iter()
            .any(|o| o.op == "TestField"),
        "L2 OnlyTF must still record TestField"
    );
    assert_eq!(table_reads("OnlyTF"), Vec::<String>::new());
    assert_eq!(table_reads("Mixed"), vec!["Get".to_string()]);
    assert_eq!(table_reads("GetOnly"), vec!["Get".to_string()]);
}

// ===========================================================================
// A2 — one op table serves both capability maps.
// ===========================================================================

/// The whole `RecordOpType` vocabulary, as (lower-case method, call text).
/// Hand-listed (the vocabulary is a `match`, not a list); each entry is checked
/// against `record_op_type`, so a typo here fails loudly.
const VOCAB: &[(&str, &str)] = &[
    ("findset", "Rec.FindSet()"),
    ("findfirst", "Rec.FindFirst()"),
    ("findlast", "Rec.FindLast()"),
    ("find", "Rec.Find('-')"),
    ("get", "Rec.Get('A')"),
    ("calcfields", "Rec.CalcFields(\"Line Count\")"),
    ("calcsums", "Rec.CalcSums(Amount)"),
    ("testfield", "Rec.TestField(Name)"),
    ("modify", "Rec.Modify()"),
    ("modifyall", "Rec.ModifyAll(Name, 'X')"),
    ("insert", "Rec.Insert()"),
    ("delete", "Rec.Delete()"),
    ("deleteall", "Rec.DeleteAll()"),
    ("rename", "Rec.Rename('B')"),
    ("setloadfields", "Rec.SetLoadFields(Name)"),
    ("addloadfields", "Rec.AddLoadFields(Name)"),
    ("setrange", "Rec.SetRange(Name, 'X')"),
    ("setfilter", "Rec.SetFilter(Name, 'X')"),
    ("setcurrentkey", "Rec.SetCurrentKey(Name)"),
    ("reset", "Rec.Reset()"),
    ("copy", "Rec.Copy(Other)"),
    ("transferfields", "Rec.TransferFields(Other)"),
    ("validate", "Rec.Validate(Name, 'X')"),
    ("init", "Rec.Init()"),
    ("next", "Rec.Next()"),
    ("count", "Rec.Count()"),
    ("countapprox", "Rec.CountApprox()"),
    ("isempty", "Rec.IsEmpty()"),
    ("locktable", "Rec.LockTable()"),
];

fn vocab_codeunit() -> String {
    let mut s = String::from("codeunit 50320 \"I100 Vocab\"\n{\n");
    for (lc, call) in VOCAB {
        s.push_str(&format!(
            "    procedure Op_{lc}()\n    var\n        Rec: Record \"I100 Item\";\n        \
             Other: Record \"I100 Item\";\n    begin\n        {call};\n    end;\n\n"
        ));
    }
    s.push_str("}\n");
    s
}

/// Per op type: the table-fact ops each consumer produced for its routine.
struct VocabRun {
    l2: BTreeMap<&'static str, BTreeSet<String>>,
    cone: BTreeMap<&'static str, BTreeSet<String>>,
}

fn vocab_run() -> VocabRun {
    let dir = write_ws(&[
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("Vocab", &vocab_codeunit()),
    ]);
    let proj = project_workspace(dir.path()).expect("L2");
    let resolved = resolve(dir.path());
    let snap = full_snapshot_json(dir.path(), &resolved);
    let facts = capability_facts(&snap);
    let mut l2 = BTreeMap::new();
    let mut cone = BTreeMap::new();
    for (lc, _) in VOCAB {
        let op_type = record_op_type(lc).unwrap_or_else(|| panic!("{lc} not in vocabulary"));
        let name = format!("Op_{lc}");
        let pr = proj
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("L2 {name}"));
        assert!(
            pr.features
                .record_operations
                .iter()
                .any(|o| o.op == op_type),
            "{name}: L2 must record {op_type} (non-vacuous): {:?}",
            pr.features
                .record_operations
                .iter()
                .map(|o| &o.op)
                .collect::<Vec<_>>()
        );
        l2.insert(
            op_type,
            pr.capability_facts_direct
                .iter()
                .filter(|f| f.resource_kind == "table")
                .map(|f| f.op.clone())
                .collect(),
        );
        let subject = stable_id(&resolved, &name);
        cone.insert(
            op_type,
            facts
                .iter()
                .filter(|f| {
                    f["subject"] == subject.as_str()
                        && f["provenance"] == "direct"
                        && f["resourceKind"] == "table"
                })
                .map(|f| f["op"].as_str().unwrap().to_string())
                .collect(),
        );
    }
    VocabRun { l2, cone }
}

/// A2: the L2 workspace facts and the cone's direct facts agree, for EVERY
/// name in the vocabulary, on whether (and as what op) it is a table fact.
/// Stated limit: no test proves that no second op table exists anywhere.
#[test]
fn a2_l2_and_cone_agree_over_the_whole_vocabulary() {
    let run = vocab_run();
    assert_eq!(
        run.l2, run.cone,
        "L2 vs cone direct table facts per op type"
    );
}

/// A2 / Design item 3 invariant, through the L2 and cone consumers: an op
/// yields a table capability fact exactly when the detectors' class table calls
/// it db-touching. Pins TestField (StateOnly -> no fact) and the new
/// `rename` / `lock` ops.
#[test]
fn a2_fact_exists_iff_db_touching_class() {
    let run = vocab_run();
    let mut bad = Vec::new();
    for (op_type, l2_ops) in &run.l2 {
        let touching = is_db_touching_class(classify_op(op_type));
        for (side, ops) in [("l2", l2_ops), ("cone", &run.cone[op_type])] {
            if ops.is_empty() == touching {
                bad.push(format!(
                    "{side}: {op_type} facts {ops:?} but db-touching = {touching}"
                ));
            }
        }
    }
    assert!(bad.is_empty(), "invariant broken:\n{}", bad.join("\n"));
    let one = |s: &str| BTreeSet::from([s.to_string()]);
    assert_eq!(run.l2["Rename"], one("rename"));
    assert_eq!(run.l2["LockTable"], one("lock"));
    assert_eq!(run.cone["Rename"], one("rename"));
    assert_eq!(run.cone["LockTable"], one("lock"));
}

// ===========================================================================
// A2b — d1/d2 see Rename/LockTable callees.
// ===========================================================================

/// The four callee shapes, all on a PHYSICAL local record.
const SHAPES: &[(&str, &str)] = &[
    (
        "tf_rename",
        "Rec.TestField(Name);\n        Rec.Rename('B');",
    ),
    ("tf_lock", "Rec.TestField(Name);\n        Rec.LockTable();"),
    ("rename_only", "Rec.Rename('B');"),
    ("lock_only", "Rec.LockTable();"),
];

fn callee_codeunit(body: &str) -> String {
    format!(
        r#"
codeunit 50330 "I100 Callee"
{{
    procedure Callee()
    var
        Rec: Record "I100 Item";
    begin
        {body}
    end;
}}
"#
    )
}

const D1_LOOP: &str = r#"
codeunit 50331 "I100 Loop"
{
    procedure LoopCaller()
    var
        Callee: Codeunit "I100 Callee";
        i: Integer;
    begin
        for i := 1 to 10 do
            Callee.Callee();
    end;
}
"#;

const D2_PUBLISHER: &str = r#"
codeunit 50332 "I100 Publisher"
{
    procedure RaiseInLoop()
    var
        i: Integer;
    begin
        for i := 1 to 10 do
            OnI100Event();
    end;

    [IntegrationEvent(false, false)]
    procedure OnI100Event()
    begin
    end;
}
"#;

const D2_SUBSCRIBER: &str = r#"
codeunit 50333 "I100 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Publisher", 'OnI100Event', '', true, true)]
    local procedure Handle()
    var
        Callee: Codeunit "I100 Callee";
    begin
        Callee.Callee();
    end;
}
"#;

#[test]
fn a2b_d1_sees_rename_and_locktable_callees() {
    let mut missing = Vec::new();
    for (shape, body) in SHAPES {
        let findings = run_detector(
            "d1-db-op-in-loop",
            &[
                al("Item", ITEM_TABLE),
                al("Line", LINE_TABLE),
                al("Callee", &callee_codeunit(body)),
                al("Loop", D1_LOOP),
            ],
        );
        if findings.is_empty() {
            missing.push(*shape);
        }
    }
    assert!(
        missing.is_empty(),
        "d1 found nothing for shapes {missing:?}"
    );
}

#[test]
fn a2b_d2_sees_rename_and_locktable_callees() {
    let mut missing = Vec::new();
    for (shape, body) in SHAPES {
        let findings = run_detector(
            "d2-event-fanout-in-loop",
            &[
                al("Item", ITEM_TABLE),
                al("Line", LINE_TABLE),
                al("Callee", &callee_codeunit(body)),
                al("Pub", D2_PUBLISHER),
                al("Sub", D2_SUBSCRIBER),
            ],
        );
        if findings.is_empty() {
            missing.push(*shape);
        }
    }
    assert!(
        missing.is_empty(),
        "d2 found nothing for shapes {missing:?}"
    );
}

// ===========================================================================
// A2c — the policy vocabulary accepts the new ops.
// ===========================================================================

const POLICY_CU: &str = r#"
codeunit 50340 "I100 Policy"
{
    procedure DoRename()
    var
        Rec: Record "I100 Item";
    begin
        Rec.TestField(Name);
        Rec.Rename('B');
    end;

    procedure DoLock()
    var
        Rec: Record "I100 Item";
    begin
        Rec.LockTable();
    end;

    procedure DoGet()
    var
        Rec: Record "I100 Item";
    begin
        Rec.Get('A');
    end;

    procedure DoModify()
    var
        Rec: Record "I100 Item";
    begin
        Rec.Modify();
    end;
}
"#;

fn policy_rule(id: &str, when: &str) -> String {
    format!(
        "version: 1\ndefaults:\n  onUnknown: fail-open\n  requireCoverage: any\nrules:\n  \
         - id: {id}\n    title: \"{id}\"\n    severity: high\n    when:\n{when}\n    facts: direct\n"
    )
}

/// Run `policy check` with one rule; return the names of the routines whose
/// declaration line a finding points at.
fn policy_hits(ws: &Path, yaml: &str) -> Result<BTreeSet<String>, String> {
    let policy = ws.join("policy.yaml");
    std::fs::write(&policy, yaml).unwrap();
    let policy = policy.to_string_lossy().to_string();
    let out = run_policy_check(&PolicyCheckOptions {
        workspace: ws,
        policy_path: Some(&policy),
        no_policy: false,
        format: "json",
        out: None,
        deterministic: true,
        strict: false,
        driver_version: "i100",
        single_app: true,
    });
    let Some(text) = out.text else {
        return Err(format!("exit {} {:?}", out.exit_code, out.stderr_lines));
    };
    let doc: Value = serde_json::from_str(&text).expect("policy json");
    let decl_line = |name: &str| -> u64 {
        POLICY_CU
            .lines()
            .position(|l| l.contains(&format!("procedure {name}()")))
            .unwrap() as u64
    };
    let mut hits = BTreeSet::new();
    for f in doc["findings"].as_array().expect("findings") {
        let line = f["primaryLocation"]["range"]["startLine"].as_u64().unwrap();
        for name in ["DoRename", "DoLock", "DoGet", "DoModify"] {
            if f["primaryLocation"]["sourceUnitId"] == "ws:src/Policy.al" && line == decl_line(name)
            {
                hits.insert(name.to_string());
            }
        }
    }
    Ok(hits)
}

#[test]
fn a2c_policy_vocabulary_accepts_rename_and_lock() {
    let dir = write_ws(&[
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("Policy", POLICY_CU),
    ]);
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();

    // (rule id, `when` body, expected routines). Collected, so one run shows
    // every broken expectation.
    let not_read = "      capability.resourceKind: table\n      not:\n        capability.op: [read, rename, lock]";
    let not_insert =
        "      capability.op: [modify, insert]\n      not:\n        capability.op: [insert]";
    let cases: &[(&str, &str, &[&str])] = &[
        // Controls: an existing op and a negated predicate compile and evaluate
        // as before (`read` matches the Get routine only: TestField is no read).
        ("r-read", "      capability.op: [read]", &["DoGet"]),
        ("r-not", not_insert, &["DoModify"]),
        // The new ops (also under a negation).
        ("r-not-new", not_read, &["DoModify"]),
        ("r-rename", "      capability.op: [rename]", &["DoRename"]),
        ("r-lock", "      capability.op: [lock]", &["DoLock"]),
    ];
    let mut bad = Vec::new();
    for (id, when, want) in cases {
        let got = policy_hits(dir.path(), &policy_rule(id, when));
        if got != Ok(set(want)) {
            bad.push(format!("{id}: got {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "A2c:\n{}", bad.join("\n"));
}

// ===========================================================================
// A2d — d2 is not starved by a known-temporary branch.
// ===========================================================================

/// The temp branch: a diamond `T0 -> {A0, B0} -> T1 -> ... -> T<layers>`. With
/// `layers = 8` the walk DEMANDS > 500 visits (2^i visits of `T_i`; ~1021 in
/// all) within depth 20 (`T8` sits at path length 18). Only the leaf has table
/// ops, each on its own LOCAL `Temporary` record. `leaf_ops` is that body.
fn a2d_workspace(leaf_ops: &str, physical_first: bool, layers: usize) -> Vec<(String, String)> {
    let mut cu = String::from("codeunit 50350 \"I100 Branch\"\n{\n");
    for i in 0..layers {
        cu.push_str(&format!(
            "    procedure T{i}()\n    begin\n        A{i}();\n        B{i}();\n    end;\n\n\
             \x20   procedure A{i}()\n    begin\n        T{n}();\n    end;\n\n\
             \x20   procedure B{i}()\n    begin\n        T{n}();\n    end;\n\n",
            n = i + 1
        ));
    }
    cu.push_str(&format!(
        "    procedure T{layers}()\n    var\n        TempRec: Record \"I100 Item\" temporary;\n    \
         begin\n        {leaf_ops}\n    end;\n\n\
         \x20   procedure Physical()\n    var\n        Rec: Record \"I100 Item\";\n    begin\n        \
         Rec.Modify();\n    end;\n}}\n"
    ));
    let calls = if physical_first {
        "Branch.Physical();\n        Branch.T0();"
    } else {
        "Branch.T0();\n        Branch.Physical();"
    };
    let sub = format!(
        r#"
codeunit 50351 "I100 Starve Sub"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Publisher", 'OnI100Event', '', true, true)]
    local procedure Handle()
    var
        Branch: Codeunit "I100 Branch";
    begin
        {calls}
    end;
}}
"#
    );
    vec![
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("Branch", &cu),
        al("Pub", D2_PUBLISHER),
        al("Sub", &sub),
    ]
}

fn d2_physical_terminal_found(findings: &[Finding]) -> bool {
    findings.iter().any(|f| {
        f.evidence_path
            .iter()
            .any(|s| s.note.starts_with("Modify on") && s.operation_id.is_some())
    })
}

/// A2d as the ledger states it: the temp branch holds only `Rename`/`LockTable`.
#[test]
fn a2d_d2_not_starved_by_temp_rename_lock_branch() {
    let findings = run_detector(
        "d2-event-fanout-in-loop",
        &a2d_workspace(
            "TempRec.Rename('B');\n        TempRec.LockTable();",
            false,
            8,
        ),
    );
    assert!(
        d2_physical_terminal_found(&findings),
        "d2 must reach Physical's Modify past a temp Rename/LockTable branch: {:#?}",
        summarize(&findings)
    );
}

/// A2d with a known-temp WRITE branch (the same mechanism; a temp `Modify` is a
/// known-temp NON-read, so the new bit is clear for the whole branch).
#[test]
fn a2d_d2_not_starved_by_temp_write_branch() {
    let findings = run_detector(
        "d2-event-fanout-in-loop",
        &a2d_workspace("TempRec.Modify();", false, 8),
    );
    assert!(
        d2_physical_terminal_found(&findings),
        "d2 must reach Physical's Modify past a temp Modify branch: {:#?}",
        summarize(&findings)
    );
}

/// Controls for A2d's precondition (green today and after): the same shape with
/// `Physical` called FIRST, and the temp-first shape with a SMALL diamond (2
/// layers, ~13 visits), both find the physical Modify. So a missing finding in
/// the 8-layer temp-first shape is the 500-visit budget, not the shape.
#[test]
fn a2d_controls_physical_first_and_small_diamond_are_found() {
    for (physical_first, layers) in [(true, 8), (false, 2)] {
        let findings = run_detector(
            "d2-event-fanout-in-loop",
            &a2d_workspace("TempRec.Modify();", physical_first, layers),
        );
        assert!(
            d2_physical_terminal_found(&findings),
            "control physical_first={physical_first} layers={layers}: {:#?}",
            summarize(&findings)
        );
    }
}

// ===========================================================================
// A2e / A2f — a temporary record forwarded to a `var` parameter.
// ===========================================================================

const FWD_PUBLISHER: &str = r#"
codeunit 50360 "I100 Fwd Publisher"
{
    procedure RaiseE1()
    var
        i: Integer;
    begin
        for i := 1 to 10 do
            OnE1();
    end;

    procedure RaiseE2()
    var
        i: Integer;
    begin
        for i := 1 to 10 do
            OnE2();
    end;

    procedure RaiseE3()
    var
        i: Integer;
    begin
        for i := 1 to 10 do
            OnE3();
    end;

    [IntegrationEvent(false, false)]
    procedure OnE1()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE2()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE3()
    begin
    end;
}
"#;

fn d2_findings_for_event(findings: &[Finding], event: &str) -> usize {
    let needle = format!("raises {event} inside a loop");
    findings
        .iter()
        .filter(|f| f.root_cause.contains(&needle))
        .count()
}

/// A2e: `E1 -> S1 -> A` (local temp) `-> B(var Rec)` `Rec.Modify()` gives no
/// d2 finding; `E2 -> S2 -> A2` (physical) `-> B` keeps it.
#[test]
fn a2e_temp_forwarded_to_var_param_write_is_pruned() {
    let cu = r#"
codeunit 50361 "I100 Fwd"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Fwd Publisher", 'OnE1', '', true, true)]
    local procedure S1()
    begin
        A();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Fwd Publisher", 'OnE2', '', true, true)]
    local procedure S2()
    begin
        A2();
    end;

    procedure A()
    var
        T: Record "I100 Item" temporary;
    begin
        B(T);
    end;

    procedure A2()
    var
        P: Record "I100 Item";
    begin
        B(P);
    end;

    procedure B(var Rec: Record "I100 Item")
    begin
        Rec.Modify();
    end;
}
"#;
    let findings = run_detector(
        "d2-event-fanout-in-loop",
        &[
            al("Item", ITEM_TABLE),
            al("Line", LINE_TABLE),
            al("FwdPub", FWD_PUBLISHER),
            al("Fwd", cu),
        ],
    );
    assert_eq!(
        d2_findings_for_event(&findings, "OnE2"),
        1,
        "E2 (physical record) keeps its finding: {:#?}",
        summarize(&findings)
    );
    assert_eq!(
        d2_findings_for_event(&findings, "OnE1"),
        0,
        "E1 (temp record forwarded to B) has no finding: {:#?}",
        summarize(&findings)
    );
}

/// A2e at the subscriber itself: `S1` owns the temp record and forwards it to
/// `B(var Rec)`. `S1`'s cone holds only the caller-frame known-temp `modify`, so
/// d2's own bit is clear and the subscriber pre-filter skips it. Without that
/// pre-filter `expand(S1 -> B)` admits `B` (its parameter-dependent `modify` sets
/// the bit) and `B`'s Modify becomes a terminal. Physical control: `OnE2`.
#[test]
fn a2e_subscriber_forwarding_its_own_temp_is_pruned_by_the_prefilter() {
    let cu = r#"
codeunit 50363 "I100 Fwd Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Fwd Publisher", 'OnE1', '', true, true)]
    local procedure S1()
    var
        T: Record "I100 Item" temporary;
    begin
        B(T);
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Fwd Publisher", 'OnE2', '', true, true)]
    local procedure S2()
    var
        P: Record "I100 Item";
    begin
        B(P);
    end;

    procedure B(var Rec: Record "I100 Item")
    begin
        Rec.Modify();
    end;
}
"#;
    let findings = run_detector(
        "d2-event-fanout-in-loop",
        &[
            al("Item", ITEM_TABLE),
            al("Line", LINE_TABLE),
            al("FwdPub", FWD_PUBLISHER),
            al("FwdSub", cu),
        ],
    );
    assert_eq!(
        d2_findings_for_event(&findings, "OnE2"),
        1,
        "E2 (physical record) keeps its finding: {:#?}",
        summarize(&findings)
    );
    assert_eq!(
        d2_findings_for_event(&findings, "OnE1"),
        0,
        "E1 (subscriber's own temp forwarded to B) has no finding: {:#?}",
        summarize(&findings)
    );
}

fn a2f_workspace(with_count: bool) -> Vec<(String, String)> {
    let count = if with_count {
        "n := T.Count();\n        "
    } else {
        ""
    };
    let cu = format!(
        r#"
codeunit 50362 "I100 Flow"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I100 Fwd Publisher", 'OnE3', '', true, true)]
    local procedure S3()
    begin
        A3();
    end;

    procedure A3()
    var
        T: Record "I100 Item" temporary;
        n: Integer;
    begin
        {count}C(T);
    end;

    procedure C(var Rec: Record "I100 Item")
    begin
        Rec.CalcFields("Line Count");
    end;
}}
"#
    );
    vec![
        al("Item", ITEM_TABLE),
        al("Line", LINE_TABLE),
        al("FwdPub", FWD_PUBLISHER),
        al("Flow", &cu),
    ]
}

/// A2f: a temp host's FlowField calculation forwarded to `C(var Rec)` keeps
/// the `E3` finding.
#[test]
fn a2f_temp_forwarded_calcfields_keeps_the_finding() {
    let findings = run_detector("d2-event-fanout-in-loop", &a2f_workspace(false));
    assert_eq!(
        d2_findings_for_event(&findings, "OnE3"),
        1,
        "{:#?}",
        summarize(&findings)
    );
}

/// A2f dedup variant: `A3` also does `T.Count()` on its temp before `C(T)`, so a
/// distance-1 temp read competes with the distance-2 `CalcFields` read under one
/// dedup key. Precondition asserted: `Count` wins `S3`'s inherited read witness.
#[test]
fn a2f_dedup_variant_count_wins_witness_and_finding_stays() {
    let files = a2f_workspace(true);
    let dir = write_ws(&files);
    let resolved = resolve(dir.path());
    let snap = full_snapshot_json(dir.path(), &resolved);
    let ops = op_type_by_id(&resolved);
    let s3 = stable_id(&resolved, "S3");
    let witnesses: Vec<String> = capability_facts(&snap)
        .iter()
        .filter(|f| {
            f["subject"] == s3.as_str()
                && f["op"] == "read"
                && f["resourceId"] == ITEM_TABLE_ID
                && f["provenance"] == "inherited"
        })
        .map(|f| {
            let w = f["witnessOperationId"].as_str().unwrap_or("<none>");
            ops.get(w)
                .cloned()
                .unwrap_or_else(|| format!("<unknown {w}>"))
        })
        .collect();
    assert_eq!(
        witnesses,
        vec!["Count".to_string()],
        "precondition: Count must win S3's inherited read witness"
    );

    let findings = run_detector("d2-event-fanout-in-loop", &files);
    assert_eq!(
        d2_findings_for_event(&findings, "OnE3"),
        1,
        "{:#?}",
        summarize(&findings)
    );
}
