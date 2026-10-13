//! #65 — `alsem digest`'s DB_* effects carry `detail.tempState`, read from the
//! SAME L4 db-effect store `alsem query effects` reads.
//!
//! Drives the production entry point `run_digest_pipeline` on a hand-written
//! workspace whose routines each state their own precondition: a temporary
//! record written directly, through a `var` helper, through a recursive cycle;
//! a real record written the same ways; and a `Validate` (a DB_MODIFY the L4
//! store has no row for, so the field must be absent, not invented).
//!
//! The cases that matter are the ones the capability cone gets wrong:
//! `ViaHelperReal` and `ViaRecursionTemp` have no parameters, yet the cone
//! keeps the helper's `parameter-dependent(0)` for them.

use std::collections::{BTreeMap, BTreeSet};

use al_sem::engine::l4::effect_query_cli::{QuerySubstrate, render_query_effects};
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use serde_json::Value;

const APP_JSON: &str = r#"{
  "id": "65656565-0000-0000-0000-000000000065",
  "name": "Issue 65 Probe",
  "publisher": "RC",
  "version": "1.0.0.0"
}"#;

const PROBE_AL: &str = r#"table 50300 "Probe Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { Clustered = true; } } }

codeunit 50301 "Probe Writer"
{
    procedure TempOnly()
    var Buffer: Record "Probe Row" temporary;
    begin Buffer.Id := 1; Buffer.Insert(); end;

    procedure RealWrite()
    var Row: Record "Probe Row";
    begin Row.Id := 1; Row.Insert(); end;

    procedure Helper(var R: Record "Probe Row")
    begin R.Insert(); end;

    procedure ViaHelperTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Helper(Buffer); end;

    procedure ViaHelperReal()
    var Row: Record "Probe Row";
    begin Helper(Row); end;

    procedure Forward(var X: Record "Probe Row")
    begin Helper(X); end;

    procedure ValidateTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Buffer.Validate(Id, 1); end;

    procedure Rec1(var R: Record "Probe Row"; N: Integer)
    begin if N > 0 then Rec2(R, N - 1); R.Insert(); end;

    procedure Rec2(var R: Record "Probe Row"; N: Integer)
    begin Rec1(R, N); end;

    procedure ViaRecursionTemp()
    var Buffer: Record "Probe Row" temporary;
    begin Rec1(Buffer, 2); end;
}
"#;

fn write_workspace() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-65-")
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(root.join("src").join("Probe.al"), PROBE_AL).expect("write al source");
    tmp
}

/// routine display -> [(effect type, detail.tempState)] for every DB_* effect.
type DbEffects = BTreeMap<String, Vec<(String, Option<String>)>>;

fn db_effects(doc: &Value) -> DbEffects {
    let mut out = DbEffects::new();
    for entry in doc["payload"]["entries"].as_array().expect("entries") {
        let name = entry["routine"]["display"].as_str().unwrap().to_string();
        let list = out.entry(name).or_default();
        for eff in entry["effects"].as_array().unwrap() {
            let ty = eff["type"].as_str().unwrap_or_default();
            if ty.starts_with("DB_") {
                let ts = eff["detail"]["tempState"].as_str().map(str::to_string);
                list.push((ty.to_string(), ts));
            }
        }
    }
    out
}

fn collect_temp_states(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "tempState"
                    && let Some(s) = x.as_str()
                {
                    out.insert(s.to_string());
                }
                collect_temp_states(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_temp_states(x, out)),
        _ => {}
    }
}

#[test]
fn digest_db_effects_carry_query_effects_temp_state() {
    let tmp = write_workspace();
    let ws = tmp.path();
    let result = run_digest_pipeline(
        ws,
        Some(vec!["src/Probe.al".to_string()]),
        None,
        None,
        "cli-b-v1",
        true,
        None,
    )
    .expect("digest runs");
    let doc: Value = serde_json::from_str(&result.json_text).expect("digest json");
    assert_eq!(doc["schemaVersion"], "1.4.0");
    let got = db_effects(&doc);

    // Every DB_INSERT in the routine has the expected value.
    let expect_inserts = |name: &str, want: &str| {
        let effs = got.get(name).unwrap_or_else(|| panic!("{name}: no entry"));
        let inserts: Vec<_> = effs.iter().filter(|(t, _)| t == "DB_INSERT").collect();
        assert!(!inserts.is_empty(), "{name}: no DB_INSERT in {effs:?}");
        for (_, ts) in inserts {
            assert_eq!(ts.as_deref(), Some(want), "{name}: {effs:?}");
        }
    };
    expect_inserts("TempOnly", "known(true)");
    expect_inserts("ViaHelperTemp", "known(true)");
    expect_inserts("ViaRecursionTemp", "known(true)");
    expect_inserts("RealWrite", "known(false)");
    expect_inserts("ViaHelperReal", "known(false)");
    expect_inserts("Helper", "parameter-dependent(0)");
    expect_inserts("Forward", "parameter-dependent(0)");

    // `Validate` has no L4 db-effect row: the field is absent, never invented.
    let validate = &got["ValidateTemp"];
    assert!(
        validate.iter().any(|(t, _)| t == "DB_MODIFY"),
        "precondition: ValidateTemp emits a DB_MODIFY ({validate:?})"
    );
    assert!(
        validate.iter().all(|(_, ts)| ts.is_none()),
        "ValidateTemp: {validate:?}"
    );

    // Cross-command contract: every digest value is one `query effects` reports
    // for the same routine.
    let model_id = al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id(ws)
        .expect("model id");
    let resolved = assemble_and_resolve_workspace_program(ws, &model_id, false).expect("resolves");
    let substrate = QuerySubstrate::from_resolved(resolved);
    for (name, effs) in &got {
        let q = render_query_effects(&substrate, &format!("Probe Writer.{name}"), "t", true);
        let qdoc: Value = serde_json::from_str(&q.json_text).expect("query json");
        let mut query_states = BTreeSet::new();
        collect_temp_states(&qdoc, &mut query_states);
        for (_, ts) in effs {
            for part in ts.iter().flat_map(|s| s.split(',')) {
                assert!(
                    query_states.contains(part),
                    "{name}: digest says {part}, query effects says {query_states:?}"
                );
            }
        }
    }
}
