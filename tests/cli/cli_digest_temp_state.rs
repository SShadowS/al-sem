//! #65 / #69 — `alsem digest`'s temp-state coherence.
//!
//! #65: a DB_* effect carries `detail.tempState`, read from the SAME L4
//! db-effect store `alsem query effects` reads.
//!
//! #69 (design spec
//! `docs/superpowers/specs/2026-10-14-issue-69-digest-temp-state-coherence-design.md`,
//! "Acceptance matrix"): every statement a digest makes about a write's temp
//! state agrees — the ordering engine's physical-write filter, the witness
//! (evidence, conditionality, viaPaths), the effect identity, and
//! `detail.tempState` — and `Validate`/`Copy`/`TransferFields` are not DB writes.
//!
//! Every test drives the production entry point `run_digest_pipeline` on the
//! shared probe workspace (`tests/common/temp_state_probe.rs`), in BOTH modes
//! (single-app and the cross-app default). Each routine there states its own
//! precondition. Assertion messages carry the matrix's named failure text, so a
//! discrimination proof can quote it. Each `#[test]` collects every failing
//! claim before it panics, so one red run shows all of them.
//!
//! Stated limit (#66): the probe has no dependency, so the two modes' stores are
//! equal here; nothing in this file tells the cross-app store (with dependency
//! leaves) from the single-app one.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l4::effect_query_cli::{QuerySubstrate, render_query_effects};
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use serde_json::Value;

use crate::temp_state_probe::write_workspace;

// ---------------------------------------------------------------------------
// Running the pipeline (once per mode, shared by every test).
// ---------------------------------------------------------------------------

fn run_mode(single_app: bool) -> Value {
    let tmp = write_workspace();
    let result = run_digest_pipeline(
        tmp.path(),
        Some(vec!["src/Probe.al".to_string()]),
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
        let v = vec![
            ("single-app", run_mode(true)),
            ("cross-app", run_mode(false)),
        ];
        if let Ok(dir) = std::env::var("ISSUE69_DUMP_DIR") {
            for (m, d) in &v {
                let p = std::path::Path::new(&dir).join(format!("{m}.json"));
                std::fs::write(p, serde_json::to_string_pretty(d).unwrap()).unwrap();
            }
        }
        v
    })
}

/// Run `check` on both modes; panic with every collected failure.
fn each_mode(check: impl Fn(&Value, &mut Vec<String>)) {
    let mut failures = Vec::new();
    for (mode, doc) in docs() {
        let mut f = Vec::new();
        check(doc, &mut f);
        failures.extend(f.into_iter().map(|m| format!("[{mode}] {m}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Reading the digest document.
// ---------------------------------------------------------------------------

fn entry<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["payload"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["routine"]["display"] == name)
        .unwrap_or_else(|| panic!("{name}: no entry"))
}

fn effects(e: &Value) -> &[Value] {
    e["effects"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn db_effects(e: &Value) -> Vec<&Value> {
    effects(e)
        .iter()
        .filter(|x| x["type"].as_str().unwrap_or("").starts_with("DB_"))
        .collect()
}

fn of_type<'a>(e: &'a Value, ty: &str) -> Vec<&'a Value> {
    effects(e).iter().filter(|x| x["type"] == ty).collect()
}

fn temp_state(eff: &Value) -> Option<&str> {
    eff["detail"]["tempState"].as_str()
}

/// The entry's UI_MESSAGE effects carry `WRITE_PENDING_AT_UI`.
fn has_label(e: &Value) -> bool {
    of_type(e, "UI_MESSAGE").iter().any(|m| {
        m["guarantees"]
            .as_array()
            .is_some_and(|g| g.iter().any(|l| l == "WRITE_PENDING_AT_UI"))
    })
}

fn summary(e: &Value) -> String {
    db_effects(e)
        .iter()
        .map(|x| {
            format!(
                "{}:{}",
                x["type"].as_str().unwrap_or("?"),
                temp_state(x).unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

// ---------------------------------------------------------------------------
// #65 — the original contract (A4: authority/subset), kept.
// ---------------------------------------------------------------------------

/// #65 values plus A4 (spec "Values come from `query effects`' store"): every
/// digest value is one `query effects` reports for the same routine, in the
/// same mode. Mutation: render "unknown" for every row in `for_effect` →
/// "TempOnly: digest says unknown".
#[test]
fn digest_db_effects_carry_query_effects_temp_state() {
    check_mode(true);
    check_mode(false);
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

fn check_mode(single_app: bool) {
    let doc = &docs()[if single_app { 0 } else { 1 }].1;
    assert_eq!(doc["schemaVersion"], "1.5.0");

    // Every DB_INSERT in the routine has the expected value.
    let expect_inserts = |name: &str, want: &str| {
        let e = entry(doc, name);
        let inserts = of_type(e, "DB_INSERT");
        assert!(
            !inserts.is_empty(),
            "{name}: no DB_INSERT in {}",
            summary(e)
        );
        for x in inserts {
            assert_eq!(temp_state(x), Some(want), "{name}: {}", summary(e));
        }
    };
    expect_inserts("TempOnly", "known(true)");
    expect_inserts("ViaHelperTemp", "known(true)");
    expect_inserts("ViaRecursionTemp", "known(true)");
    expect_inserts("RealWrite", "known(false)");
    expect_inserts("ViaHelperReal", "known(false)");
    expect_inserts("ViaRecursionReal", "known(false)");
    expect_inserts("Helper", "parameter-dependent(0)");
    expect_inserts("Forward", "parameter-dependent(0)");

    // Cross-command contract: every digest value is one `query effects` reports
    // for the same routine.
    let tmp = write_workspace();
    let ws = tmp.path();
    let model_id = compute_gate_model_instance_id(ws).expect("model id");
    let substrate = QuerySubstrate::build(ws, &model_id, single_app).expect("query substrate");
    let mut failures = Vec::new();
    for e in doc["payload"]["entries"].as_array().expect("entries") {
        let name = e["routine"]["display"].as_str().unwrap();
        let db = db_effects(e);
        if db.is_empty() {
            continue;
        }
        let q = render_query_effects(&substrate, &format!("Probe Writer.{name}"), "t", true);
        let qdoc: Value = serde_json::from_str(&q.json_text).expect("query json");
        let mut query_states = BTreeSet::new();
        collect_temp_states(&qdoc, &mut query_states);
        for x in db {
            for part in temp_state(x).iter().flat_map(|s| s.split(',')) {
                if !query_states.contains(part) {
                    failures.push(format!(
                        "{name}: digest says {part}, query effects says {query_states:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// A1 — the ordering filter, `detail.tempState` and the cone agree.
// ---------------------------------------------------------------------------

/// A1b. No `temp-state-incoherent` diagnostic anywhere in the document.
/// Mutation: put producers back into `fact_cone_for_scc` for recursive SCCs →
/// "RecUiTemp: temp-state-incoherent class-absent".
///
/// GREEN before #69: the diagnostic kind does not exist yet, so this test cannot
/// be red on the base; it pins the post-fix state (D4) and is the named red of
/// the mutation above.
#[test]
fn a1b_no_temp_state_incoherence() {
    each_mode(|doc, f| {
        for e in doc["payload"]["entries"].as_array().expect("entries") {
            let root = e["routine"]["display"].as_str().unwrap();
            for d in e["entryDiagnostics"].as_array().into_iter().flatten() {
                if d["kind"] == "temp-state-incoherent" {
                    f.push(format!(
                        "{root}: temp-state-incoherent {}",
                        d["reason"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// A3 — Validate / Copy / TransferFields are not DB writes (D5).
// ---------------------------------------------------------------------------

/// A3. State-only ops are not DB writes, on temporary and real records alike.
/// Replaces the #65 block that pinned "ValidateTemp emits a DB_MODIFY" (the
/// pinned behaviour changes, see CHANGELOG). Mutation: put each op back in
/// `modify`, one at a time → "<op routine>: DB_MODIFY present".
#[test]
fn a3_state_only_ops_are_not_db_writes() {
    each_mode(|doc, f| {
        for name in [
            "ValidateTemp",
            "ValidateReal",
            "CopyTemp",
            "CopyShareTemp",
            "CopyReal",
            "TransferTemp",
            "TransferReal",
            "ValidateRealUi",
            "CopyRealUi",
        ] {
            let e = entry(doc, name);
            for x in db_effects(e) {
                f.push(format!(
                    "{name}: {} present",
                    x["type"].as_str().unwrap_or("?")
                ));
            }
        }
        for name in ["ValidateRealUi", "CopyRealUi"] {
            if has_label(entry(doc, name)) {
                f.push(format!("{name}: label present"));
            }
        }
        f.sort();
        f.dedup();
    });
}
