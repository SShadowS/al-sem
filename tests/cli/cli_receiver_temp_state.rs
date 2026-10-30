//! G49: a write whose temp state depends on the routine's own implicit `Rec`
//! (the reserved receiver index, `u32::MAX`) renders as
//! `parameter-dependent(rec)` on every user-facing surface, never as the raw
//! `parameter-dependent(4294967295)`.
//!
//! The fixture `tests/fixtures/g49-receiver` states the precondition in AL: a
//! table procedure `Touch` that calls `Modify()` on its own `Rec` (compiled clean
//! with alc 18.0). Each test drives a production entry point: `query effects`,
//! `digest` and `prove`, JSON and human text alike.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use al_sem::engine::l4::effect_query_cli::run_query_effects_pipeline;
use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use al_sem::engine::l5::prove::run_prove_pipeline;
use al_sem::program::inputs::WorkspaceInputs;
use serde_json::Value;

const REC: &str = "parameter-dependent(rec)";
const RAW: &str = "4294967295";

fn ws() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/g49-receiver")
}

fn inputs() -> Arc<WorkspaceInputs> {
    WorkspaceInputs::read(&ws(), &crate::fingerprint_needs())
}

/// Every string value under `v` (the JSON's rendered text).
fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// The text surface must name the receiver and never print the sentinel.
fn assert_rendered(label: &str, json_text: &str, human_text: &str, want_human: bool) {
    let doc: Value = serde_json::from_str(json_text).expect("json parses");
    let mut all = Vec::new();
    strings(&doc, &mut all);
    assert!(
        all.iter().any(|s| s.split(',').any(|p| p == REC)),
        "{label}: no `{REC}` string in the JSON:\n{json_text}"
    );
    assert!(
        !all.iter().any(|s| s.contains(RAW)),
        "{label}: a JSON string prints the raw receiver index:\n{json_text}"
    );
    if want_human {
        assert!(
            human_text.contains(REC),
            "{label}: human text lacks `{REC}`:\n{human_text}"
        );
    }
    assert!(
        !human_text.contains(RAW),
        "{label}: human text prints the raw receiver index:\n{human_text}"
    );
}

#[test]
fn query_effects_names_the_receiver() {
    for single_app in [true, false] {
        let r = run_query_effects_pipeline(&inputs(), "Rcv Row.Touch", "t", true, single_app)
            .expect("query effects runs");
        assert_eq!(r.exit_code, 0, "{}", r.json_text);
        assert_rendered(
            &format!("query effects single_app={single_app}"),
            &r.json_text,
            &r.human_text,
            true,
        );
    }
}

#[test]
fn digest_temp_state_names_the_receiver() {
    for single_app in [true, false] {
        let r = run_digest_pipeline(
            &inputs(),
            Some(vec!["src/Rcv.al".to_string()]),
            None,
            None,
            "t",
            true,
            None,
            single_app,
            false,
        )
        .expect("digest runs");
        let doc: Value = serde_json::from_str(&r.json_text).expect("digest json");
        let states: Vec<&str> = doc["payload"]["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter(|e| e["routine"]["display"] == "Touch")
            .flat_map(|e| e["effects"].as_array().into_iter().flatten())
            .filter_map(|x| x["detail"]["tempState"].as_str())
            .collect();
        assert_eq!(
            states,
            vec![REC],
            "digest single_app={single_app}: detail.tempState"
        );
        assert_rendered(
            &format!("digest single_app={single_app}"),
            &r.json_text,
            &r.human_text,
            false,
        );
    }
}

fn prove(routine: &str, single_app: bool) -> (Value, String, String) {
    let r = run_prove_pipeline(
        &inputs(),
        routine,
        "writes-table:Rcv Row",
        "t",
        true,
        single_app,
    )
    .expect("prove runs");
    let doc = serde_json::from_str(&r.json_text).expect("prove json");
    (doc, r.json_text, r.human_text)
}

/// `Touch` has a caller that passes a persistent record: `yes`, and the
/// evidence's `detail.tempState` names the receiver.
#[test]
fn prove_evidence_names_the_receiver() {
    for single_app in [true, false] {
        let (doc, json, human) = prove("Touch", single_app);
        let label = format!("prove Touch single_app={single_app}");
        let result = &doc["payload"]["result"];
        assert_eq!(result["answer"], "yes", "{label}: {json}");
        let states: Vec<&str> = result["evidence"]
            .as_array()
            .expect("evidence")
            .iter()
            .filter_map(|e| e["detail"]["tempState"].as_str())
            .collect();
        assert_eq!(states, vec![REC], "{label}: evidence tempState");
        assert_rendered(&label, &json, &human, false);
    }
}

/// `TouchAlone` has no caller: `unknown`, blocked by the caller-dependent
/// write, whose text names the implicit Rec rather than "a record parameter".
#[test]
fn prove_blocker_names_the_implicit_rec() {
    for single_app in [true, false] {
        let (doc, json, human) = prove("TouchAlone", single_app);
        let label = format!("prove TouchAlone single_app={single_app}");
        let result = &doc["payload"]["result"];
        assert_eq!(result["answer"], "unknown", "{label}: {json}");
        let details: Vec<&str> = result["blockedBy"]
            .as_array()
            .expect("blockedBy")
            .iter()
            .filter(|b| b["kind"] == "temp-state-depends-on-caller")
            .filter_map(|b| b["detail"].as_str())
            .collect();
        assert_eq!(
            details,
            vec![
                "DB_MODIFY on the implicit Rec: a table write only if the record the \
                 routine runs on is not temporary"
            ],
            "{label}"
        );
        assert!(!json.contains(RAW) && !human.contains(RAW), "{label}");
    }
}
