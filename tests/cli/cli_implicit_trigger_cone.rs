//! #96 — implicit-trigger edges in the typed graph (design spec
//! `docs/superpowers/specs/2026-10-16-issue-96-implicit-trigger-typed-edges-design.md`,
//! "Acceptance matrix", rows A1-A6).
//!
//! A record operation that fires a table or field trigger (`T.Validate`,
//! `T.Insert(true)`, ...) reaches the trigger body through an `implicit-trigger`
//! edge. L4's db-effect store follows it; every typed-graph consumer (the
//! capability cone, the snapshot, digest, prove) must follow it too, and a
//! TEMPORARY receiver must stay non-physical across it.
//!
//! Every test drives a public entry point on an inline tempdir workspace
//! (the `temp_state_probe::write_workspace` pattern): `run_digest_pipeline` in
//! both modes, `run_prove_pipeline`, the detector context's
//! `ConeDerivedStore`, `compose_snapshot`, the combined graph and L4
//! `query effects`. Expected values (table ids, hop kind, anchor line/column,
//! conditionality) are literals; `fixture_is_where_the_literals_say` checks the
//! literals against the fixture text, never against the code under test. Each
//! `#[test]` collects every failing claim before it panics.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l4::combined_graph::build_combined_graph;
use al_sem::engine::l4::effect_query_cli::{QuerySubstrate, render_query_effects};
use al_sem::engine::l5::detector_context::build_detector_context;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use al_sem::engine::l5::prove::run_prove_pipeline;
use al_sem::engine::l5::registry::{run_detectors, substrate};
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
};
use al_sem::program::model::workspace::Model;
use serde_json::Value;

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

const APP_JSON: &str = r#"{
  "id": "96969696-0000-0000-0000-000000000096",
  "name": "Issue 96 Probe",
  "publisher": "RC",
  "version": "1.0.0.0"
}"#;

/// `Trig Row`'s triggers: field `Amount`'s `OnValidate` inserts `Log Row` and
/// modifies `Rec`; `OnInsert` inserts `Ins Log` and modifies `Rec`; `OnModify`
/// inserts `Mod Log`. `Rec.Modify()` has no RunTrigger argument, so it fires
/// nothing. Line numbers in the comments are 0-based (the digest anchor
/// convention); the `// @` markers are what `fixture_is_where_the_literals_say`
/// checks.
const SRC: &str = r#"table 50310 "Log Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }
table 50313 "Ins Log" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }
table 50314 "Mod Log" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }
table 50311 "Trig Row"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            var L: Record "Log Row";
            begin L.Id := 1; L.Insert(); Rec.Modify(); end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }

    trigger OnInsert()
    var I: Record "Ins Log";
    begin I.Insert(); Rec.Modify(); end;

    trigger OnModify()
    var M: Record "Mod Log";
    begin M.Insert(); end;
}

codeunit 50312 "Trig Caller"
{
    procedure ValidateIt()
    var T: Record "Trig Row";
    begin
        T.Validate(Amount, 1); // @validate-it
    end;

    procedure InsertTrue()
    var T: Record "Trig Row";
    begin
        T.Insert(true); // @insert-true
    end;

    procedure InsertVar(B: Boolean)
    var T: Record "Trig Row";
    begin
        T.Insert(B); // @insert-var
    end;

    procedure ModifyAllTrue()
    var T: Record "Trig Row";
    begin
        T.ModifyAll(Amount, 0, true); // @modifyall-true
    end;

    procedure CondValidate(X: Boolean)
    var T: Record "Trig Row";
    begin
        if X then
            T.Validate(Amount, 1); // @cond-validate
    end;

    procedure TempValidate()
    var TempT: Record "Trig Row" temporary;
    begin
        TempT.Validate(Amount, 1); // @temp-validate
    end;

    procedure TempInsert()
    var TempT: Record "Trig Row" temporary;
    begin
        TempT.Insert(true); // @temp-insert
    end;

    procedure MixedValidate()
    var
        T: Record "Trig Row";
        TempT: Record "Trig Row" temporary;
    begin
        T.Validate(Amount, 1); // @mixed-real-1
        TempT.Validate(Amount, 2); // @mixed-temp
        T.Validate(Amount, 3); // @mixed-real-2
    end;

    procedure V(var R: Record "Trig Row")
    begin
        R.Validate(Amount, 1); // @forward-v
    end;

    procedure ForwardTemp()
    var TempT: Record "Trig Row" temporary;
    begin
        V(TempT); // @forward-temp
    end;

    procedure ForwardReal()
    var T: Record "Trig Row";
    begin
        V(T); // @forward-real
    end;
}
"#;

/// The resource ids the engine gives the fixture's tables (`<app guid>/table/<id>`,
/// the form of a digest effect's `detail.resourceId`).
const LOG_ROW_ID: &str = "96969696-0000-0000-0000-000000000096/table/50310";
const TRIG_ROW_ID: &str = "96969696-0000-0000-0000-000000000096/table/50311";
const INS_LOG_ID: &str = "96969696-0000-0000-0000-000000000096/table/50313";
const MOD_LOG_ID: &str = "96969696-0000-0000-0000-000000000096/table/50314";

/// 0-based (line, column) of each record operation's receiver, stated
/// literally: `(marker, line, column, text at that position)`.
const SITES: &[(&str, u64, u64, &str)] = &[
    ("validate-it", 31, 8, "T.Validate(Amount, 1);"),
    ("insert-true", 37, 8, "T.Insert(true);"),
    ("insert-var", 43, 8, "T.Insert(B);"),
    ("modifyall-true", 49, 8, "T.ModifyAll(Amount, 0, true);"),
    ("cond-validate", 56, 12, "T.Validate(Amount, 1);"),
    ("temp-validate", 62, 8, "TempT.Validate(Amount, 1);"),
    ("temp-insert", 68, 8, "TempT.Insert(true);"),
    ("mixed-real-1", 76, 8, "T.Validate(Amount, 1);"),
    ("mixed-temp", 77, 8, "TempT.Validate(Amount, 2);"),
    ("mixed-real-2", 78, 8, "T.Validate(Amount, 3);"),
    ("forward-v", 83, 8, "R.Validate(Amount, 1);"),
];

fn write_workspace() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-96-")
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(root.join("src").join("T.al"), SRC).expect("write al source");
    tmp
}

/// The fixture literals above are where they say (a fixture self-check, so a
/// red assertion below is never a misplaced literal).
#[test]
fn fixture_is_where_the_literals_say() {
    let lines: Vec<&str> = SRC.lines().collect();
    for (marker, line, col, text) in SITES {
        let l = lines[*line as usize];
        assert!(
            l.ends_with(&format!("// @{marker}")),
            "{marker}: line {line} is {l:?}"
        );
        assert_eq!(
            &l[*col as usize..*col as usize + text.len()],
            *text,
            "{marker}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pipelines (each run once, shared by every test).
// ---------------------------------------------------------------------------

/// The command's one read of `ws` (#160).
fn read(ws: &Path) -> std::sync::Arc<al_sem::program::inputs::InputSet> {
    al_sem::program::inputs::InputSet::read(ws, &crate::fingerprint_needs())
}

fn digest(ws: &Path, file: &str, single_app: bool) -> Value {
    let result = run_digest_pipeline(
        &read(ws),
        Some(vec![file.to_string()]),
        None,
        None,
        "cli-b-v1",
        true,
        None,
        single_app,
    )
    .expect("digest runs");
    serde_json::from_str(&result.json_text).expect("digest json")
}

/// `[(mode label, digest document)]` for single-app and cross-app.
fn docs() -> &'static [(&'static str, Value)] {
    static DOCS: OnceLock<Vec<(&'static str, Value)>> = OnceLock::new();
    DOCS.get_or_init(|| {
        let tmp = write_workspace();
        vec![
            ("single-app", digest(tmp.path(), "src/T.al", true)),
            ("cross-app", digest(tmp.path(), "src/T.al", false)),
        ]
    })
}

fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        let tmp = write_workspace();
        let id = compute_gate_model_instance_id(tmp.path()).expect("model id");
        assemble_and_resolve_workspace_program(&read(tmp.path()), &id, false).expect("model")
    })
}

/// L4 `query effects` rows of `Trig Caller.<routine>`, single-app (the store
/// is the same in both modes here: no dependency).
fn query_effects(routine: &str) -> Vec<Value> {
    static WS: OnceLock<(tempfile::TempDir, QuerySubstrate)> = OnceLock::new();
    let (_, substrate) = WS.get_or_init(|| {
        let tmp = write_workspace();
        let id = compute_gate_model_instance_id(tmp.path()).expect("model id");
        let s = QuerySubstrate::build(&read(tmp.path()), &id, true).expect("query substrate");
        (tmp, s)
    });
    let q = render_query_effects(substrate, &format!("Trig Caller.{routine}"), "t", true);
    let doc: Value = serde_json::from_str(&q.json_text).expect("query json");
    doc["payload"]["effects"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn prove(routine: &str, question: &str) -> Value {
    let tmp = write_workspace();
    let r =
        run_prove_pipeline(&read(tmp.path()), routine, question, "t", true).expect("prove runs");
    serde_json::from_str(&r.json_text).expect("prove json")
}

/// Run `check` on both digest modes, with the shared no-incoherence check.
fn each_mode(check: impl Fn(&Value, &mut Vec<String>)) {
    let mut failures = Vec::new();
    for (mode, doc) in docs() {
        let mut f = Vec::new();
        no_incoherence(doc, &mut f);
        check(doc, &mut f);
        failures.extend(f.into_iter().map(|m| format!("[{mode}] {m}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Reading documents.
// ---------------------------------------------------------------------------

fn no_incoherence(doc: &Value, f: &mut Vec<String>) {
    for e in doc["payload"]["entries"].as_array().expect("entries") {
        let root = e["routine"]["display"].as_str().unwrap_or("?");
        for d in e["entryDiagnostics"].as_array().into_iter().flatten() {
            if d["kind"] == "temp-state-incoherent" {
                f.push(format!(
                    "{root}: temp-state-incoherent {}",
                    d["reason"].as_str().unwrap_or("?")
                ));
            }
        }
    }
}

fn entry<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["payload"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["routine"]["display"] == name)
        .unwrap_or_else(|| panic!("{name}: no entry"))
}

/// The entry's effects of type `ty` on resource `resource_id`.
fn effects_on<'a>(e: &'a Value, ty: &str, resource_id: &str) -> Vec<&'a Value> {
    e["effects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["type"] == ty && x["detail"]["resourceId"] == resource_id)
        .collect()
}

fn temp_state(eff: &Value) -> &str {
    eff["detail"]["tempState"].as_str().unwrap_or("<absent>")
}

/// A temp state with a part that is not `known(true)`: a physical write.
fn is_physical(eff: &Value) -> bool {
    temp_state(eff).split(',').any(|p| p != "known(true)")
}

fn via_paths(eff: &Value) -> Vec<&Vec<Value>> {
    eff["viaPaths"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_array).collect())
        .unwrap_or_default()
}

/// One line per effect of the entry, for failure messages.
fn summary(e: &Value) -> String {
    e["effects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["type"].as_str().unwrap_or("").starts_with("DB_"))
        .map(|x| {
            format!(
                "{} {} {} {} paths={}",
                x["type"].as_str().unwrap_or("?"),
                x["detail"]["resourceDisplay"].as_str().unwrap_or("?"),
                temp_state(x),
                x["conditionality"].as_str().unwrap_or("?"),
                x["viaPaths"]
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Is `hop` an `implicit-trigger` hop into a routine named `trigger`, anchored
/// at (line, column)?
fn is_trigger_hop(hop: &Value, trigger: &str, line: u64, column: u64) -> bool {
    hop["kind"] == "implicit-trigger"
        && hop["toDisplay"]
            .as_str()
            .is_some_and(|d| d.ends_with(trigger))
        && hop["anchor"]["line"] == line
        && hop["anchor"]["column"] == column
}

/// What a root's trigger effect must look like (see [`expect_trigger_effect`]).
struct Want<'a> {
    ty: &'a str,
    resource_id: &'a str,
    trigger: &'a str,
    at: (u64, u64),
    cond: &'a str,
}

/// `root` has a `want.ty` effect on `want.resource_id` with a viaPath whose
/// FIRST hop is an `implicit-trigger` hop into `want.trigger` anchored at
/// `want.at`, and the effect's conditionality is `want.cond`.
fn expect_trigger_effect(doc: &Value, f: &mut Vec<String>, root: &str, want: &Want) {
    let Want {
        ty,
        resource_id,
        trigger,
        at: (line, column),
        cond,
    } = *want;
    let e = entry(doc, root);
    let hits = effects_on(e, ty, resource_id);
    if hits.is_empty() {
        f.push(format!(
            "{root}: no {ty} on {resource_id} (no trigger effect); effects: [{}]",
            summary(e)
        ));
        return;
    }
    let via_trigger: Vec<&&Value> = hits
        .iter()
        .filter(|x| {
            via_paths(x).iter().any(|p| {
                p.first()
                    .is_some_and(|h| is_trigger_hop(h, trigger, line, column))
            })
        })
        .collect();
    if via_trigger.is_empty() {
        f.push(format!(
            "{root}: {ty} on {resource_id} has no viaPath whose first hop is implicit-trigger \
             to {trigger} at {line}:{column}; effects: [{}]",
            summary(e)
        ));
    }
    for x in via_trigger {
        if x["conditionality"] != cond {
            f.push(format!(
                "{root}: {ty} on {resource_id} conditionality {} != {cond}",
                x["conditionality"]
            ));
        }
    }
}

/// The unique routine id of `Trig Caller.<name>` (or `Trig Row`'s trigger
/// whose name ends with `name`).
fn rid(m: &Model, object: &str, name: &str) -> String {
    let ws = &m.workspace;
    let hits: Vec<&str> = ws
        .routines
        .iter()
        .filter(|r| {
            (r.name == name || r.name.ends_with(name))
                && ws
                    .objects
                    .iter()
                    .any(|o| o.id == r.object_id && o.name == object)
        })
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(hits.len(), 1, "one routine {object}.{name}, got {hits:?}");
    hits[0].to_string()
}

/// L4 rows of `routine` for `op` on table `table`, as `(via, tempState)`.
fn l4_rows(routine: &str, op: &str, table: &str) -> Vec<(String, String)> {
    query_effects(routine)
        .iter()
        .filter(|x| x["op"] == op && x["tableName"] == table)
        .map(|x| {
            (
                x["via"].as_str().unwrap_or("?").to_string(),
                x["tempState"].as_str().unwrap_or("<absent>").to_string(),
            )
        })
        .collect()
}

/// Every DB_MODIFY effect on `Trig Row` anywhere in a prove document.
fn prove_trig_row_modifies(doc: &Value) -> Vec<Value> {
    fn walk(v: &Value, out: &mut Vec<Value>) {
        match v {
            Value::Object(m) => {
                if m.get("type").is_some_and(|t| t == "DB_MODIFY")
                    && v["detail"]["resourceId"] == TRIG_ROW_ID
                {
                    out.push(v.clone());
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(&doc["payload"], &mut out);
    out
}

// ---------------------------------------------------------------------------
// A1 — digest reports the trigger's write, with the trigger hop; `fires`.
// ---------------------------------------------------------------------------

/// A1. `ValidateIt` (`T.Validate(Amount, 1)`): a `DB_INSERT` on "Log Row"
/// whose viaPath's first hop is `implicit-trigger` to `OnValidate`, anchored at
/// the Validate (31:8), unconditional. `fires`: `T.Insert(true)` gives an
/// unconditional `Ins Log` insert; `T.Insert(B)` a conditional one;
/// `T.ModifyAll(Amount, 0, true)` a conditional `Mod Log` insert; an
/// `if`-nested Validate a conditional `Log Row` insert.
#[test]
fn a1_digest_follows_the_implicit_trigger() {
    each_mode(|doc, f| {
        for (root, resource, trigger, at, cond) in [
            (
                "ValidateIt",
                LOG_ROW_ID,
                "OnValidate",
                (31, 8),
                "unconditional-on-success",
            ),
            (
                "InsertTrue",
                INS_LOG_ID,
                "OnInsert",
                (37, 8),
                "unconditional-on-success",
            ),
            ("InsertVar", INS_LOG_ID, "OnInsert", (43, 8), "conditional"),
            (
                "ModifyAllTrue",
                MOD_LOG_ID,
                "OnModify",
                (49, 8),
                "conditional",
            ),
            (
                "CondValidate",
                LOG_ROW_ID,
                "OnValidate",
                (56, 12),
                "conditional",
            ),
        ] {
            expect_trigger_effect(
                doc,
                f,
                root,
                &Want {
                    ty: "DB_INSERT",
                    resource_id: resource,
                    trigger,
                    at,
                    cond,
                },
            );
        }
    });
}

/// A1's precondition, true on master: L4 already sees the trigger writes A1
/// asks digest for, so A1's red on master is the typed graph, not the fixture.
///
/// Stated exception, on master (before #96): `ModifyAllTrue`. The program
/// engine had its `OnModify` edge (`alsem graph`: `via implicitTrigger`), but
/// the model adapter dropped it: master's `can_fire_trigger` listed neither
/// `modifyall` nor `deleteall`, so L4 had no `Mod Log` insert either, and A1's
/// bulk case was red for that reason too. #96 admits both in the adapter
/// (`src/program/model/program_calls.rs`, `can_fire_trigger`); this list still
/// leaves the bulk case out, because it states the master condition.
#[test]
fn a1_precondition_l4_sees_the_trigger_writes() {
    let mut f = Vec::new();
    for (routine, table) in [
        ("ValidateIt", "Log Row"),
        ("InsertTrue", "Ins Log"),
        ("InsertVar", "Ins Log"),
        ("CondValidate", "Log Row"),
    ] {
        let rows = l4_rows(routine, "Insert", table);
        if !rows.iter().any(|(via, _)| via == "implicit-trigger") {
            f.push(format!(
                "{routine}: L4 has no Insert on {table} via implicit-trigger: {rows:?}"
            ));
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A2 — the capability cone and prove.
// ---------------------------------------------------------------------------

/// A2. The cone's physical write set of `ValidateIt` contains "Log Row"
/// (50310), and `prove ValidateIt "writes-table:Log Row"` = yes. Precondition
/// (true today): `OnValidate`'s own set contains it.
#[test]
fn a2_cone_and_prove_see_the_trigger_write() {
    let m = model();
    let ctx = build_detector_context(m, substrate::ALL);
    let mut f = Vec::new();
    let on_validate = ctx
        .cone_derived
        .writes_physical_tables_of(&rid(m, "Trig Row", "OnValidate"));
    if !on_validate.iter().any(|t| t == LOG_ROW_ID) {
        f.push(format!(
            "precondition: OnValidate's physical writes {on_validate:?} lack {LOG_ROW_ID}"
        ));
    }
    let got = ctx
        .cone_derived
        .writes_physical_tables_of(&rid(m, "Trig Caller", "ValidateIt"));
    if !got.iter().any(|t| t == LOG_ROW_ID) {
        f.push(format!(
            "ValidateIt: cone physical writes {got:?} lack {LOG_ROW_ID}"
        ));
    }
    let p = prove("ValidateIt", "writes-table:Log Row");
    let answer = &p["payload"]["result"]["answer"];
    if answer != "yes" {
        f.push(format!(
            "prove ValidateIt writes-table:Log Row = {answer}, want yes"
        ));
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A3 — a temporary receiver stays non-physical, in L4 and in the cone.
// ---------------------------------------------------------------------------

/// A3. `TempT.Validate(Amount, 1)` and `TempT.Insert(true)` on a temporary
/// "Trig Row" whose `OnValidate`/`OnInsert` do `Rec.Modify()`:
/// - L4 `query effects` shows that Modify via implicit-trigger as `known(true)`;
/// - the cone has the write (temp-inclusive) but not as a physical write;
/// - digest has the DB_MODIFY on "Trig Row", and only as `known(true)`;
/// - prove's DB_MODIFY effects on "Trig Row" are all `known(true)` (prove's
///   `writes-table` is temp-blind, so the claim is on the effects it reports);
/// - the non-temporary twins (`ValidateIt`, `InsertTrue`) keep the physical
///   write in L4, the cone and digest.
#[test]
fn a3_temporary_receiver_stays_non_physical() {
    let m = model();
    let ctx = build_detector_context(m, substrate::ALL);
    let mut f = Vec::new();

    for (routine, want) in [
        ("TempValidate", "known(true)"),
        ("TempInsert", "known(true)"),
        ("ValidateIt", "known(false)"),
        ("InsertTrue", "known(false)"),
    ] {
        let rows = l4_rows(routine, "Modify", "Trig Row");
        let trig: Vec<_> = rows
            .iter()
            .filter(|(v, _)| v == "implicit-trigger")
            .collect();
        if trig.is_empty() || trig.iter().any(|(_, ts)| ts != want) {
            f.push(format!(
                "L4 {routine}: Modify on Trig Row via implicit-trigger want temp {want}, got {rows:?}"
            ));
        }
    }

    for (routine, physical) in [
        ("TempValidate", false),
        ("TempInsert", false),
        ("ValidateIt", true),
        ("InsertTrue", true),
    ] {
        let id = rid(m, "Trig Caller", routine);
        let all = ctx.cone_derived.writes_tables_of(&id);
        let phys = ctx.cone_derived.writes_physical_tables_of(&id);
        if !all.iter().any(|t| t == TRIG_ROW_ID) {
            f.push(format!(
                "cone {routine}: the trigger's Rec.Modify never reached the cone ({all:?})"
            ));
        }
        if phys.iter().any(|t| t == TRIG_ROW_ID) != physical {
            f.push(format!(
                "cone {routine}: physical writes {phys:?}, want Trig Row physical={physical}"
            ));
        }
    }

    for (mode, doc) in docs() {
        no_incoherence(doc, &mut f);
        for (routine, physical) in [
            ("TempValidate", false),
            ("TempInsert", false),
            ("ValidateIt", true),
            ("InsertTrue", true),
        ] {
            let e = entry(doc, routine);
            let mods = effects_on(e, "DB_MODIFY", TRIG_ROW_ID);
            if mods.is_empty() {
                f.push(format!(
                    "[{mode}] digest {routine}: no DB_MODIFY on Trig Row; effects: [{}]",
                    summary(e)
                ));
            }
            if mods.iter().any(|x| is_physical(x)) != physical {
                f.push(format!(
                    "[{mode}] digest {routine}: want physical DB_MODIFY on Trig Row = {physical}; \
                     effects: [{}]",
                    summary(e)
                ));
            }
        }
    }

    for routine in ["TempValidate", "TempInsert"] {
        let mods = prove_trig_row_modifies(&prove(routine, "writes-table:Trig Row"));
        if mods.is_empty() {
            f.push(format!(
                "prove {routine}: no DB_MODIFY on Trig Row reported"
            ));
        }
        for x in &mods {
            if is_physical(x) {
                f.push(format!(
                    "prove {routine}: physical DB_MODIFY on Trig Row ({})",
                    temp_state(x)
                ));
            }
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A4 — bindings and keys per operation.
// ---------------------------------------------------------------------------

/// A4. `MixedValidate`: real `T.Validate` (76:8), temporary `TempT.Validate`
/// (77:8), real `T.Validate` (78:8), all into one `OnValidate`.
/// - the combined graph and the snapshot each hold THREE implicit-trigger edges
///   from it (one per op), and the snapshot's PD arena links through all three;
/// - the cone has the physical write;
/// - digest's physical DB_MODIFY on "Trig Row" has EXACTLY ONE viaPath, its
///   first hop the implicit-trigger hop anchored at the first REAL site (76:8,
///   the lowest real op id), never the temporary site (77:8).
#[test]
fn a4_bindings_and_keys_are_per_operation() {
    let m = model();
    let mut f = Vec::new();
    let mixed = rid(m, "Trig Caller", "MixedValidate");

    let graph = build_combined_graph(&m.workspace, &m.calls, &m.events.graph);
    let ops: BTreeSet<Option<String>> = graph
        .typed_edges
        .iter()
        .filter(|t| t.kind == "implicit-trigger" && t.from == mixed)
        .map(|t| t.operation_id.clone())
        .collect();
    if ops.len() != 3 || ops.contains(&None) {
        f.push(format!(
            "combined graph: MixedValidate implicit-trigger typed edges by op {ops:?}, want 3 ops"
        ));
    }

    let snap = compose_snapshot(m);
    let stable = |display_suffix: &str| -> Option<String> {
        snap.identities
            .stable_ids
            .iter()
            .zip(&snap.identities.display_names)
            .find(|(_, d)| d.ends_with(display_suffix))
            .map(|(s, _)| s.clone())
    };
    let mixed_stable = stable("MixedValidate").expect("MixedValidate stable id");
    let edges: Vec<(usize, Value)> = snap
        .typed_edges
        .iter()
        .enumerate()
        .map(|(i, e)| (i, serde_json::to_value(e).expect("edge json")))
        .filter(|(_, e)| e["kind"] == "implicit-trigger" && e["from"] == mixed_stable.as_str())
        .collect();
    let edge_ids: BTreeSet<String> = edges
        .iter()
        .map(|(_, e)| e["edgeId"].as_str().unwrap_or("?").to_string())
        .collect();
    if edges.len() != 3 || edge_ids.len() != 3 {
        f.push(format!(
            "snapshot: MixedValidate implicit-trigger edges {:?}, want 3 distinct",
            edges.iter().map(|(_, e)| e).collect::<Vec<_>>()
        ));
    }
    let trigger_ix: BTreeSet<usize> = edges.iter().map(|(i, _)| *i).collect();
    let arena = &snap.pd_solution;
    let linked: BTreeSet<usize> = arena
        .by_routine
        .get(&mixed_stable)
        .into_iter()
        .flatten()
        .flat_map(|&s| arena.links[arena.states[s].links.clone()].iter())
        .map(|&(_, e)| e)
        .filter(|e| trigger_ix.contains(e))
        .collect();
    if linked.len() != 3 {
        f.push(format!(
            "PD arena: MixedValidate links through trigger edges {linked:?}, want 3 distinct"
        ));
    }

    let ctx = build_detector_context(m, substrate::ALL);
    let phys = ctx.cone_derived.writes_physical_tables_of(&mixed);
    if !phys.iter().any(|t| t == TRIG_ROW_ID) {
        f.push(format!(
            "cone MixedValidate: physical writes {phys:?} lack {TRIG_ROW_ID}"
        ));
    }

    for (mode, doc) in docs() {
        no_incoherence(doc, &mut f);
        let e = entry(doc, "MixedValidate");
        let phys: Vec<&Value> = effects_on(e, "DB_MODIFY", TRIG_ROW_ID)
            .into_iter()
            .filter(|x| is_physical(x))
            .collect();
        if phys.len() != 1 {
            f.push(format!(
                "[{mode}] digest MixedValidate: {} physical DB_MODIFY on Trig Row, want 1; \
                 effects: [{}]",
                phys.len(),
                summary(e)
            ));
            continue;
        }
        let paths = via_paths(phys[0]);
        let first_hops: Vec<&Value> = paths.iter().filter_map(|p| p.first()).collect();
        if paths.len() != 1
            || !first_hops
                .iter()
                .all(|h| is_trigger_hop(h, "OnValidate", 76, 8))
        {
            f.push(format!(
                "[{mode}] digest MixedValidate: physical DB_MODIFY viaPaths {}, want exactly one, \
                 first hop implicit-trigger at 76:8",
                phys[0]["viaPaths"]
            ));
        }
        if first_hops.iter().any(|h| h["anchor"]["line"] == 77) {
            f.push(format!(
                "[{mode}] digest MixedValidate: a physical viaPath is anchored at the temporary op (77)"
            ));
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A5 — a forwarded receiver is bound through a procedure parameter.
// ---------------------------------------------------------------------------

/// A5. `V(var R)` does `R.Validate(Amount, 1)`; `ForwardTemp` passes a
/// temporary record, `ForwardReal` a real one. The trigger's `Rec.Modify()`:
/// - is `parameter-dependent(0)` in `V` (L4 and digest);
/// - is `known(true)` in `ForwardTemp` (L4, digest; not physical in the cone);
/// - is physical in `ForwardReal` (digest and cone).
#[test]
fn a5_forwarded_receiver_binds_through_the_parameter() {
    let m = model();
    let ctx = build_detector_context(m, substrate::ALL);
    let mut f = Vec::new();

    for (routine, want) in [
        ("V", "parameter-dependent(0)"),
        ("ForwardTemp", "known(true)"),
        ("ForwardReal", "known(false)"),
    ] {
        let rows = l4_rows(routine, "Modify", "Trig Row");
        if rows.is_empty() || rows.iter().any(|(_, ts)| ts != want) {
            f.push(format!(
                "L4 {routine}: Modify on Trig Row want temp {want}, got {rows:?}"
            ));
        }
    }

    for (routine, physical) in [("ForwardTemp", false), ("ForwardReal", true)] {
        let id = rid(m, "Trig Caller", routine);
        let all = ctx.cone_derived.writes_tables_of(&id);
        let phys = ctx.cone_derived.writes_physical_tables_of(&id);
        if !all.iter().any(|t| t == TRIG_ROW_ID) {
            f.push(format!(
                "cone {routine}: the trigger's Rec.Modify never reached the cone ({all:?})"
            ));
        }
        if phys.iter().any(|t| t == TRIG_ROW_ID) != physical {
            f.push(format!(
                "cone {routine}: physical writes {phys:?}, want Trig Row physical={physical}"
            ));
        }
    }

    for (mode, doc) in docs() {
        no_incoherence(doc, &mut f);
        for (routine, want) in [
            ("V", "parameter-dependent(0)"),
            ("ForwardTemp", "known(true)"),
            ("ForwardReal", "known(false)"),
        ] {
            let e = entry(doc, routine);
            let mods = effects_on(e, "DB_MODIFY", TRIG_ROW_ID);
            if mods.is_empty() || mods.iter().any(|x| temp_state(x) != want) {
                f.push(format!(
                    "[{mode}] digest {routine}: DB_MODIFY on Trig Row want temp {want}; effects: [{}]",
                    summary(e)
                ));
            }
        }
        // V's own trigger hop is anchored at its Validate (83:8).
        expect_trigger_effect(
            doc,
            &mut f,
            "V",
            &Want {
                ty: "DB_MODIFY",
                resource_id: TRIG_ROW_ID,
                trigger: "OnValidate",
                at: (83, 8),
                cond: "unconditional-on-success",
            },
        );
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// A6 — a source-bearing dependency's trigger reaches the workspace caller.
// ---------------------------------------------------------------------------

const DEP_GUID: &str = "dddd9696-0000-0000-0000-000000000096";
const WS6_GUID: &str = "eeee9696-0000-0000-0000-000000000096";
const DEP_LOG_ID: &str = "dddd9696-0000-0000-0000-000000000096/table/70001";

const DEP_AL: &str = r#"table 70001 "Dep Log" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }
table 70000 "Dep Trig"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            var L: Record "Dep Log";
            begin L.Id := 1; L.Insert(); end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

codeunit 70002 "Dep Ops"
{
    procedure DoValidate()
    var D: Record "Dep Trig";
    begin
        D.Validate(Amount, 1);
    end;

    procedure DoInsert()
    begin
        InsertLog();
    end;

    local procedure InsertLog()
    var L: Record "Dep Log";
    begin
        L.Id := 2;
        L.Insert();
    end;
}
"#;

/// Line 6, column 8 (0-based) is `D.Validate(Amount, 1);`. `ValidateViaDep`
/// reaches the dependency's `OnValidate` through a DEPENDENCY-origin trigger
/// edge (`Dep Ops.DoValidate` fires it); `InsertViaDep` reaches "Dep Log"
/// through a dependency-origin CALL (`DoInsert` -> `InsertLog`), the
/// non-trigger control.
const WS6_AL: &str = r#"codeunit 50396 "Ws Caller"
{
    procedure ValidateDep()
    var
        D: Record "Dep Trig";
    begin
        D.Validate(Amount, 1); // @dep-validate
    end;

    procedure ValidateViaDep()
    var Ops: Codeunit "Dep Ops";
    begin
        Ops.DoValidate();
    end;

    procedure InsertViaDep()
    var Ops: Codeunit "Dep Ops";
    begin
        Ops.DoInsert();
    end;
}
"#;

fn write_cross_app_workspace(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("app.json"),
        format!(
            r#"{{"id":"{WS6_GUID}","name":"Ws96","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50300,"to":50399}}],"dependencies":[{{"id":"{DEP_GUID}","name":"Dep96","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(dir.join("src/Ws.al"), WS6_AL).unwrap();
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":70000,"Name":"Dep Trig","Fields":[{{"Id":1,"Name":"Id","TypeDefinition":{{"Name":"Integer"}}}},{{"Id":2,"Name":"Amount","TypeDefinition":{{"Name":"Decimal"}}}}]}},{{"Id":70001,"Name":"Dep Log","Fields":[{{"Id":1,"Name":"Id","TypeDefinition":{{"Name":"Integer"}}}}]}}],"Codeunits":[{{"Id":70002,"Name":"Dep Ops","Methods":[{{"Name":"DoValidate","Parameters":[]}},{{"Name":"DoInsert","Parameters":[]}}]}}],"AppId":"{DEP_GUID}","Name":"Dep96","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    let pkgs = dir.join(".alpackages");
    std::fs::create_dir_all(&pkgs).unwrap();
    crate::symbol_app::write_source_app(
        &pkgs.join("probe_Dep96_1.0.0.0.app"),
        DEP_GUID,
        "Dep96",
        "1.0.0.0",
        &symbols,
        &[("src/Dep.al", DEP_AL)],
        "",
    );
}

/// A6. Cross-app digest of `ValidateDep` (`D.Validate(Amount, 1)` on the
/// dependency's "Dep Trig", whose `OnValidate` inserts the dependency's
/// "Dep Log") reports the `DB_INSERT` on "Dep Log" with a first hop
/// `implicit-trigger` to `OnValidate` anchored at the Validate (6:8),
/// unconditional. Precondition (true today): cross-app L4 `query effects`
/// already has that insert via implicit-trigger, and the fixture line is where
/// the literal says.
#[test]
fn a6_dependency_trigger_reaches_the_workspace_caller() {
    assert!(
        WS6_AL
            .lines()
            .nth(6)
            .is_some_and(|l| l[8..].starts_with("D.Validate(Amount, 1);")),
        "fixture: line 6 col 8 is the Validate"
    );
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_cross_app_workspace(ws);
    let mut f = Vec::new();

    let model_id = compute_gate_model_instance_id(ws).expect("model id");
    let substrate = QuerySubstrate::build(&read(ws), &model_id, false).expect("query substrate");
    let q = render_query_effects(&substrate, "Ws Caller.ValidateDep", "t", true);
    let qdoc: Value = serde_json::from_str(&q.json_text).expect("query json");
    let l4: Vec<(String, String)> = qdoc["payload"]["effects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["op"] == "Insert" && x["tableName"] == "Dep Log")
        .map(|x| {
            (
                x["via"].as_str().unwrap_or("?").to_string(),
                x["tempState"].as_str().unwrap_or("?").to_string(),
            )
        })
        .collect();
    if !l4.iter().any(|(v, _)| v == "implicit-trigger") {
        f.push(format!(
            "precondition: cross-app L4 has no Insert on Dep Log via implicit-trigger: {l4:?}"
        ));
    }

    let doc = digest(ws, "src/Ws.al", false);
    no_incoherence(&doc, &mut f);
    expect_trigger_effect(
        &doc,
        &mut f,
        "ValidateDep",
        &Want {
            ty: "DB_INSERT",
            resource_id: DEP_LOG_ID,
            trigger: "OnValidate",
            at: (6, 8),
            cond: "unconditional-on-success",
        },
    );
    assert!(f.is_empty(), "{}", f.join("\n"));
}

/// A `facts: inherited` rule for an insert into the dependency's "Dep Log".
const DEP_LOG_INSERT_POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: dep-log-insert
    title: "Dep Log insert"
    severity: high
    when:
      capability.op: insert
      capability.resource.table.name: "Dep Log"
    facts: inherited
"#;

/// A6, final-panel S8: a trigger edge that starts in DEPENDENCY code.
/// `ValidateViaDep` calls the dependency's `Dep Ops.DoValidate`, which does
/// `D.Validate(Amount, 1)` on the dependency's own "Dep Trig"; its
/// `OnValidate` inserts "Dep Log". That trigger edge is not in the model's
/// `calls` (cut to workspace callers): it reaches the cone only as an
/// injected typed edge (`build_cross_app_base`'s `dep_calls`). The cross-app
/// detector context's cone must carry the insert to `ValidateViaDep`.
///
/// Observed through `policy check`, the public consumer of the cross-app
/// detector context's cone (`ConeDerivedStore` is crate-private there): a
/// `facts: inherited` rule on "Dep Log" inserts finds `ValidateViaDep`, like
/// the non-trigger control `InsertViaDep` (`DoInsert` -> `InsertLog`) and the
/// workspace-origin trigger `ValidateDep`. Single-app finds none of them
/// (precondition: the facts come from the dependency).
///
/// Digest is NOT asserted for the two dependency-origin shapes: the cross-app
/// digest snapshot is composed from the model's cut `calls`, so it misses
/// EVERY dependency-origin edge, a call as much as a trigger (pre-existing,
/// see `.agent/issue-96/t8.md`, "F1 root cause", and the ledger's discovery).
#[test]
fn a6_dependency_origin_trigger_reaches_the_workspace_cone() {
    use al_sem::engine::gate::policy::pipeline::{
        PolicyCheckOptions, policy_request, run_policy_check,
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    write_cross_app_workspace(&ws);
    let policy = tmp.path().join("policy.yaml");
    std::fs::write(&policy, DEP_LOG_INSERT_POLICY).unwrap();
    let policy = policy.to_string_lossy().to_string();

    let run = |single_app: bool| -> Vec<u64> {
        let out = run_policy_check(&PolicyCheckOptions {
            inputs: &al_sem::program::inputs::InputSet::read(
                &ws,
                &al_sem::program::inputs::InputNeeds {
                    policy: policy_request(Some(&policy), false),
                    ..Default::default()
                },
            ),
            no_policy: false,
            format: "json",
            out: None,
            deterministic: true,
            strict: false,
            driver_version: "t",
            single_app,
        });
        let text = out.text.unwrap_or_else(|| {
            panic!(
                "policy check: exit {} {:?}",
                out.exit_code, out.stderr_lines
            )
        });
        let doc: Value = serde_json::from_str(&text).expect("policy json");
        let mut lines: Vec<u64> = doc["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .filter(|x| x["primaryLocation"]["sourceUnitId"] == "ws:src/Ws.al")
            .map(|x| {
                x["primaryLocation"]["range"]["startLine"]
                    .as_u64()
                    .unwrap_or(u64::MAX)
            })
            .collect();
        lines.sort_unstable();
        lines
    };
    let decl = |name: &str| -> u64 {
        let needle = format!("procedure {name}()");
        WS6_AL
            .lines()
            .position(|l| l.contains(&needle))
            .unwrap_or_else(|| panic!("{name}: not in WS6_AL")) as u64
    };
    let want = vec![
        decl("ValidateDep"),
        decl("ValidateViaDep"),
        decl("InsertViaDep"),
    ];
    let mut f = Vec::new();
    let cross = run(false);
    if cross != want {
        f.push(format!(
            "cross-app: dep-log-insert findings at lines {cross:?}, want {want:?} \
             (ValidateDep, ValidateViaDep, InsertViaDep)"
        ));
    }
    let single = run(true);
    if !single.is_empty() {
        f.push(format!(
            "precondition: single-app finds {single:?}, want none"
        ));
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// The caller's guard on the firing op, and d1 across a trigger hop.
// ---------------------------------------------------------------------------

const INLINE_GUID: &str = "96969696-0000-0000-0000-0000000000a7";

fn inline_model(src: &str) -> Model {
    assemble_and_resolve_inline_program_default(
        &[("src/T7.al".to_string(), src.to_string())],
        INLINE_GUID,
    )
}

/// `Guarded` fires `OnValidate` only `if not Skip`. `OnValidate` inserts
/// "G Log" (a direct fact, the plain cone path) and modifies `Rec` (a PD fact
/// bound to the firing op, the PD solve's path).
const GUARD_SRC: &str = r#"table 50330 "G Log" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }
table 50331 "G Trig"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            var L: Record "G Log";
            begin L.Insert(); Rec.Modify(); end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

codeunit 50332 "G Caller"
{
    procedure Guarded(Skip: Boolean)
    var T: Record "G Trig";
    begin
        if not Skip then
            T.Validate(Amount, 1);
    end;

    procedure CallSkip()
    begin
        Guarded(true);
    end;

    procedure CallRun()
    begin
        Guarded(false);
    end;
}
"#;

/// #96 review CR3: the CALLER's guard on the op that fires a trigger applies
/// to everything the trigger does, on both cone paths: the plain path ("G Log",
/// `fact_cone_for_scc`) and the PD solve ("G Trig" through the trigger's `Rec`,
/// `pd_solve::solve`). `Guarded(true)` makes `if not Skip` false, so
/// `CallSkip` inherits neither write; `CallRun` inherits both.
#[test]
fn caller_guard_on_the_firing_op_reaches_the_trigger_facts() {
    let m = inline_model(GUARD_SRC);
    let ctx = build_detector_context(&m, substrate::ALL);
    let log = format!("{INLINE_GUID}/table/50330");
    let trig = format!("{INLINE_GUID}/table/50331");
    let writes = |name: &str| {
        let mut w = ctx
            .cone_derived
            .writes_physical_tables_of(&rid(&m, "G Caller", name));
        w.sort();
        w
    };
    let mut both = vec![log, trig];
    both.sort();
    let mut f = Vec::new();
    for (name, want) in [
        ("Guarded", both.clone()),
        ("CallRun", both.clone()),
        ("CallSkip", Vec::new()),
    ] {
        let got = writes(name);
        if got != want {
            f.push(format!("{name}: physical writes {got:?}, want {want:?}"));
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

/// d1-shaped: a loop calls a helper that fires `OnValidate`/`OnInsert`, whose
/// body does `Rec.Modify()` on the trigger's own record. `Rec` binds to the
/// helper's record op: real in `LoopReal`/`LoopInsert`, temporary in `LoopTemp`.
/// The temporary path uses its own table, "D1 TempTrig", so d1 never merges it
/// into a real path's finding (d1 reports one finding per terminal op).
const D1_SRC: &str = r#"table 50340 "D1 Trig"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            begin Rec.Modify(); end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }

    trigger OnInsert()
    begin Rec.Modify(); end;
}

table 50342 "D1 TempTrig"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            begin Rec.Modify(); end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

codeunit 50341 "D1 Loop"
{
    procedure LoopReal()
    var I: Integer;
    begin
        for I := 1 to 10 do
            ValidateReal();
    end;

    procedure LoopTemp()
    var I: Integer;
    begin
        for I := 1 to 10 do
            ValidateTemp();
    end;

    procedure LoopInsert()
    var I: Integer;
    begin
        for I := 1 to 10 do
            InsertReal();
    end;

    local procedure ValidateReal()
    var T: Record "D1 Trig";
    begin
        T.Validate(Amount, 1);
    end;

    local procedure ValidateTemp()
    var TempT: Record "D1 TempTrig" temporary;
    begin
        TempT.Validate(Amount, 1);
    end;

    local procedure InsertReal()
    var T: Record "D1 Trig";
    begin
        T.Insert(true);
    end;
}
"#;

/// #96 review CR2: d1 itself (the production fast path, `d1_temp::cross_hop`
/// and `d1_liveness`) binds a trigger's `Rec` to the firing op. A loop that
/// reaches the trigger's `Rec.Modify()` through a REAL record is a physical
/// finding with no "(temp state uncertain)"; through a TEMPORARY record it is
/// downgraded to info with the temporary-record note.
#[test]
fn d1_binds_the_trigger_rec_to_the_firing_op() {
    let m = inline_model(D1_SRC);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 is registered once");
    let findings = run_detectors(&m, &d1).findings;
    // The finding for `loop` reaching the trigger's `Rec.Modify()` on `table`.
    let modify_in = |lp: &str, table: &str, trigger: &str| -> Vec<(&str, &str)> {
        let head = format!("A loop in {lp} reaches Modify on {table} in {trigger},");
        findings
            .iter()
            .filter(|x| x.root_cause.starts_with(&head))
            .map(|x| (x.severity.as_str(), x.root_cause.as_str()))
            .collect()
    };
    let mut f = Vec::new();
    for (lp, trigger) in [("LoopReal", "OnValidate"), ("LoopInsert", "OnInsert")] {
        let got = modify_in(lp, "D1 Trig", trigger);
        let [(severity, rc)] = got.as_slice() else {
            f.push(format!("{lp}: want 1 Modify finding, got {got:?}"));
            continue;
        };
        if rc.contains("(temp state uncertain)") || rc.contains("(temporary record") {
            f.push(format!("{lp}: want a physical finding, got {rc:?}"));
        }
        if *severity == "info" {
            f.push(format!("{lp}: a physical finding is not info: {rc:?}"));
        }
    }
    let got = modify_in("LoopTemp", "D1 TempTrig", "OnValidate");
    match got.as_slice() {
        [("info", rc)] if rc.contains("(temporary record") => {}
        _ => f.push(format!(
            "LoopTemp: want one info temporary-record finding, got {got:?}"
        )),
    }
    let all: Vec<(&str, &str)> = findings
        .iter()
        .map(|x| (x.severity.as_str(), x.root_cause.as_str()))
        .collect();
    assert!(f.is_empty(), "{}\nall d1 findings: {all:#?}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// Ordering keeps the trigger hop in the call chain (final-panel N-F1).
// ---------------------------------------------------------------------------

/// `O Trig`'s `OnValidate` makes an HTTP call as its first operation. In
/// `LateIo` the trigger fires AFTER `DoCommit()`; in `EarlyIo` BEFORE it.
/// `Noop()` makes `DoCommit()`'s order in `LateIo` larger than the HTTP
/// call's order inside `OnValidate`, which is what the erased hop compared.
const ORDER_SRC: &str = r#"table 50351 "O Trig"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            var
                Client: HttpClient;
                Resp: HttpResponseMessage;
            begin
                Client.Get('https://example.test/t', Resp);
            end;
        }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

codeunit 50352 "O Caller"
{
    procedure Noop()
    begin
    end;

    procedure DoCommit()
    begin
        Commit();
    end;

    procedure LateIo()
    var T: Record "O Trig";
    begin
        Noop();
        DoCommit();
        T.Validate(Amount, 1);
    end;

    procedure EarlyIo()
    var T: Record "O Trig";
    begin
        T.Validate(Amount, 1);
        DoCommit();
    end;
}
"#;

/// Every label an effect carries (`guarantees` and `scopedGuarantees`).
fn labels(eff: &Value) -> Vec<String> {
    let plain = eff["guarantees"].as_array().into_iter().flatten();
    let scoped = eff["scopedGuarantees"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| &g["label"]);
    plain
        .chain(scoped)
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// #96 final-panel N-F1. A trigger hop has no call site, and the ordering
/// engine used to drop it from the call chain. The trigger's HTTP call then
/// looked root-local, and its order inside `OnValidate` was compared with
/// `DoCommit()`'s order in the root: a false "HTTP before commit" in `LateIo`.
/// The hop is now a call link keyed by the firing op (`T.Validate`), so
/// `LateIo` has no `EXTERNAL_IO_BEFORE_COMMIT`. `EarlyIo` keeps it (the HTTP
/// call really runs before the commit), which shows the label sees the
/// trigger's HTTP effect at all.
#[test]
fn ordering_keeps_the_trigger_hop_in_the_call_chain() {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-96-ord-")
        .tempdir()
        .expect("create scratch ws");
    std::fs::create_dir_all(tmp.path().join("src")).expect("create src dir");
    std::fs::write(tmp.path().join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(tmp.path().join("src").join("O.al"), ORDER_SRC).expect("write al source");

    let mut f = Vec::new();
    for single_app in [true, false] {
        let doc = digest(tmp.path(), "src/O.al", single_app);
        no_incoherence(&doc, &mut f);
        for (root, want) in [("LateIo", false), ("EarlyIo", true)] {
            let e = entry(&doc, root);
            let effects: Vec<&Value> = e["effects"].as_array().into_iter().flatten().collect();
            let http: Vec<&&Value> = effects
                .iter()
                .filter(|x| {
                    x["type"] == "HTTP"
                        && via_paths(x).iter().any(|p| {
                            p.first().is_some_and(|h| {
                                h["kind"] == "implicit-trigger"
                                    && h["toDisplay"]
                                        .as_str()
                                        .is_some_and(|d| d.ends_with("OnValidate"))
                            })
                        })
                })
                .collect();
            let commits = effects.iter().filter(|x| x["type"] == "COMMIT").count();
            if http.len() != 1 || commits != 1 {
                f.push(format!(
                    "[single_app={single_app}] {root}: precondition: want 1 HTTP via the \
                     trigger and 1 COMMIT, got {} and {commits}",
                    http.len()
                ));
                continue;
            }
            let has = effects
                .iter()
                .any(|x| labels(x).iter().any(|l| l == "EXTERNAL_IO_BEFORE_COMMIT"));
            if has != want {
                let all: Vec<(String, Vec<String>)> = effects
                    .iter()
                    .map(|x| (x["type"].as_str().unwrap_or("?").to_string(), labels(x)))
                    .collect();
                f.push(format!(
                    "[single_app={single_app}] {root}: EXTERNAL_IO_BEFORE_COMMIT {has}, \
                     want {want}; labels: {all:?}"
                ));
            }
        }
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}
