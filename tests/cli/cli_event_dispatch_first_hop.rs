//! #98 — an inherited fact whose FIRST hop is an event-dispatch edge (design:
//! `.agent/issue-98/ledger.md`, "Acceptance matrix", rows A2, A2b, A2c, A3).
//!
//! An event-dispatch edge runs from the PUBLISHER procedure to each
//! subscriber, so it is a first hop only when the digest root is the
//! publisher itself. Such a fact must get a witness path whose first hop is
//! that event-dispatch edge, and source evidence at the subscriber's write.
//!
//! Every test drives a public entry point (`run_digest_pipeline`,
//! `run_fingerprint_pipeline`) on an inline tempdir workspace. Expected values
//! (table id, event id, hop kinds, anchor line/column) are literals;
//! `fixture_is_where_the_literals_say` checks the anchor literals against the
//! fixture text, never against the code under test. The `control_*` and
//! `*_precondition*` tests hold today and show the fixtures are sound, so a
//! red acceptance test is the missing first hop and not a broken fixture.

use std::path::Path;

use al_sem::engine::l5::digest_cli::run_digest_pipeline;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
};
use al_sem::engine::l5::fingerprint_query::WitnessLimit;
use serde_json::Value;

// ---------------------------------------------------------------------------
// The fixtures.
// ---------------------------------------------------------------------------

const APP_JSON: &str = r#"{
  "id": "11111111-0000-0000-0000-000000009898",
  "name": "Issue 98 Probe",
  "publisher": "RC",
  "version": "1.0.0.0"
}"#;

/// Makes the publisher `OnX` a fingerprint root (an `[IntegrationEvent]`
/// procedure is not a `public-procedure` root by default).
const ROOTS_CONFIG: &str = r#"{
  "version": 1,
  "roots": [
    {
      "id": "onx-root",
      "target": {
        "objectId": "11111111-0000-0000-0000-000000009898/Codeunit/69801",
        "routineName": "OnX"
      },
      "kinds": ["job-queue-entrypoint"],
      "note": "i98: make the publisher a fingerprint root."
    }
  ]
}"#;

const LOG_AL: &str = r#"table 69800 "I98 Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
    keys { key(PK; "Entry No.") { } }
}
"#;

/// A2/A3: `Raise` calls `OnX`; `HandleOnX` subscribes and inserts.
const PUBLISHER_AL: &str = r#"codeunit 69801 "I98 Publisher"
{
    procedure Raise()
    begin
        OnX();
    end;

    [IntegrationEvent(false, false)]
    procedure OnX()
    begin
    end;
}
"#;

const SUBSCRIBER_AL: &str = r#"codeunit 69802 "I98 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I98 Publisher", 'OnX', '', false, false)]
    local procedure HandleOnX()
    var
        Log: Record "I98 Log";
    begin
        Log.Insert(); // @insert
    end;
}
"#;

/// A2b: the record is a `var` parameter of both the publisher and the
/// subscriber, with the SAME name (`event_pair` binds by name). The only write
/// in the workspace is then a raw parameter-dependent producer.
const PD_PUBLISHER_AL: &str = r#"codeunit 69801 "I98 Publisher"
{
    procedure Raise()
    var
        L: Record "I98 Log";
    begin
        OnX(L);
    end;

    [IntegrationEvent(false, false)]
    procedure OnX(var L: Record "I98 Log")
    begin
    end;
}
"#;

const PD_SUBSCRIBER_AL: &str = r#"codeunit 69802 "I98 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I98 Publisher", 'OnX', '', false, false)]
    local procedure HandleOnX(var L: Record "I98 Log")
    begin
        L.Insert(); // @pd-insert
    end;
}
"#;

/// A2c: two subscribers of `OnX`, both inserting into the table.
const FAN_SUBSCRIBER_AL: &str = r#"codeunit 69802 "I98 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I98 Publisher", 'OnX', '', false, false)]
    local procedure HandleOnX()
    var
        Log: Record "I98 Log";
    begin
        Log.Insert(); // @fan-insert-1
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"I98 Publisher", 'OnX', '', false, false)]
    local procedure HandleOnX2()
    var
        Log: Record "I98 Log";
    begin
        Log.Insert(); // @fan-insert-2
    end;
}
"#;

const LOG_ID: &str = "11111111-0000-0000-0000-000000009898/table/69800";
const ONX_EVENT_ID: &str = "11111111-0000-0000-0000-000000009898/Codeunit/69801/event/onx";
const SUBSCRIBER_FILE: &str = "ws:src/Subscriber.al";

/// `(marker, source, 0-based line, 0-based column, text at that position)`.
const SITES: &[(&str, &str, u64, u64, &str)] = &[
    ("insert", SUBSCRIBER_AL, 7, 8, "Log.Insert();"),
    ("pd-insert", PD_SUBSCRIBER_AL, 5, 8, "L.Insert();"),
    ("fan-insert-1", FAN_SUBSCRIBER_AL, 7, 8, "Log.Insert();"),
    ("fan-insert-2", FAN_SUBSCRIBER_AL, 15, 8, "Log.Insert();"),
];

fn write_workspace(publisher: &str, subscriber: &str) -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-98-")
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(root.join("roots.config.json"), ROOTS_CONFIG).expect("write roots config");
    for (name, text) in [
        ("Log.al", LOG_AL),
        ("Publisher.al", publisher),
        ("Subscriber.al", subscriber),
    ] {
        std::fs::write(root.join("src").join(name), text).expect("write al source");
    }
    tmp
}

#[test]
fn fixture_is_where_the_literals_say() {
    for (marker, src, line, col, text) in SITES {
        let l = src.lines().nth(*line as usize).expect("line exists");
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
// Pipelines and readers.
// ---------------------------------------------------------------------------

/// The digest document of `routine` (single-app: the fixture has no dependency).
fn digest(publisher: &str, subscriber: &str, routine: &str) -> Value {
    let tmp = write_workspace(publisher, subscriber);
    let result = run_digest_pipeline(
        &al_sem::program::inputs::InputSet::read(tmp.path(), &crate::fingerprint_needs()),
        None,
        Some(vec![routine.to_string()]),
        None,
        "cli-b-v1",
        true,
        None,
        true,
    )
    .expect("digest runs");
    serde_json::from_str(&result.json_text).expect("digest json")
}

fn fingerprint_witness(ws: &Path, routine: &str) -> Value {
    let opts = FingerprintOptions {
        inputs: &al_sem::program::inputs::InputSet::read(ws, &crate::fingerprint_needs()),
        driver_version: "cli-b-v1",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::All),
        roots: None,
        routine_selectors: vec![routine.to_string()],
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts).expect("fingerprint runs");
    match result.output {
        FingerprintOutput::Text(t) => serde_json::from_str(&t).expect("fingerprint json"),
        _ => panic!("expected Text from a json query"),
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

/// The entry's single `DB_INSERT` effect on the log table.
fn db_insert(e: &Value) -> &Value {
    let hits: Vec<&Value> = e["effects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["type"] == "DB_INSERT" && x["detail"]["resourceId"] == LOG_ID)
        .collect();
    assert_eq!(hits.len(), 1, "one DB_INSERT on {LOG_ID}: {hits:?}");
    hits[0]
}

/// A hop as `kind:from->to`, the form the assertions compare.
fn hop_str(h: &Value) -> String {
    format!(
        "{}:{}->{}",
        h["kind"].as_str().unwrap_or("?"),
        h["fromDisplay"].as_str().unwrap_or("?"),
        h["toDisplay"].as_str().unwrap_or("?")
    )
}

fn via_paths(eff: &Value) -> Vec<Vec<String>> {
    eff["viaPaths"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| p.as_array().into_iter().flatten().map(hop_str).collect())
        .collect()
}

/// Every `event-dispatch` hop of every viaPath carries the `OnX` event id.
fn event_hops_carry_onx_id(eff: &Value, f: &mut Vec<String>) {
    for p in eff["viaPaths"].as_array().into_iter().flatten() {
        for h in p.as_array().into_iter().flatten() {
            if h["kind"] == "event-dispatch" && h["eventId"] != ONX_EVENT_ID {
                f.push(format!(
                    "event hop eventId {} != {ONX_EVENT_ID}",
                    h["eventId"]
                ));
            }
        }
    }
}

/// The evidence is `source` at `(SUBSCRIBER_FILE, line, column)`.
fn evidence_at(eff: &Value, line: u64, column: u64, f: &mut Vec<String>) {
    let ev = &eff["evidence"];
    if ev["sourceKind"] != "source" {
        f.push(format!(
            "evidence.sourceKind is {} (want \"source\")",
            ev["sourceKind"]
        ));
        return;
    }
    if ev["file"] != SUBSCRIBER_FILE || ev["line"] != line || ev["column"] != column {
        f.push(format!(
            "evidence at {}:{}:{} (want {SUBSCRIBER_FILE}:{line}:{column})",
            ev["file"], ev["line"], ev["column"]
        ));
    }
}

fn evidence_unavailable_for(e: &Value, effect_type: &str) -> bool {
    e["entryDiagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|d| d["kind"] == "evidence-unavailable" && d["effectType"] == effect_type)
}

fn finish(f: Vec<String>) {
    assert!(f.is_empty(), "{}", f.join("\n"));
}

// ---------------------------------------------------------------------------
// Controls (green today).
// ---------------------------------------------------------------------------

/// The Raise-rooted path already exists: the event edge is the SECOND hop, so
/// the witness BFS walks it. This shows the fixture's event edge and write are
/// what the acceptance tests below expect.
#[test]
fn control_raise_rooted_path_crosses_the_event_edge() {
    let doc = digest(PUBLISHER_AL, SUBSCRIBER_AL, "Raise");
    let eff = db_insert(entry(&doc, "Raise"));
    let mut f = Vec::new();
    let want = vec![vec![
        "call:Raise->OnX".to_string(),
        "event-dispatch:OnX->HandleOnX".to_string(),
    ]];
    if via_paths(eff) != want {
        f.push(format!("viaPaths {:?} (want {want:?})", via_paths(eff)));
    }
    event_hops_carry_onx_id(eff, &mut f);
    evidence_at(eff, 7, 8, &mut f);
    finish(f);
}

/// The publisher's cone already holds the inherited insert: the bug is the
/// missing witness, not a missing fact.
#[test]
fn control_publisher_has_the_inherited_insert() {
    let doc = digest(PUBLISHER_AL, SUBSCRIBER_AL, "OnX");
    let eff = db_insert(entry(&doc, "OnX"));
    assert_eq!(eff["provenance"], "transitive", "{eff}");
}

// ---------------------------------------------------------------------------
// A2 / A3.
// ---------------------------------------------------------------------------

/// A2 + A3: the digest of the publisher gives the DB_INSERT one path, exactly
/// the event-dispatch hop, with source evidence at the subscriber's insert and
/// no `evidence-unavailable` diagnostic for DB_INSERT.
#[test]
fn a2_a3_publisher_digest_has_the_event_first_hop_and_source_evidence() {
    let doc = digest(PUBLISHER_AL, SUBSCRIBER_AL, "OnX");
    let e = entry(&doc, "OnX");
    let eff = db_insert(e);
    let mut f = Vec::new();
    let want = vec![vec!["event-dispatch:OnX->HandleOnX".to_string()]];
    if via_paths(eff) != want {
        f.push(format!("viaPaths {:?} (want {want:?})", via_paths(eff)));
    }
    event_hops_carry_onx_id(eff, &mut f);
    evidence_at(eff, 7, 8, &mut f);
    if evidence_unavailable_for(e, "DB_INSERT") {
        f.push("an evidence-unavailable entry diagnostic for DB_INSERT".to_string());
    }
    finish(f);
}

/// A2: the fingerprint witness of the publisher's inherited insert fact has
/// paths and no `first-hop-not-found`.
#[test]
fn a2_publisher_fingerprint_witness_has_paths() {
    let tmp = write_workspace(PUBLISHER_AL, SUBSCRIBER_AL);
    let doc = fingerprint_witness(tmp.path(), "OnX");
    let blocks = doc["payload"]["blocks"].as_array().expect("blocks");
    let block = blocks
        .iter()
        .find(|b| b["routine"]["display"] == "OnX")
        .expect("OnX block (rooted by roots.config.json)");
    let facts: Vec<&Value> = block["families"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|fam| fam["resources"].as_array().into_iter().flatten())
        .flat_map(|r| r["facts"].as_array().into_iter().flatten())
        .filter(|x| x["op"] == "insert" && x["resourceId"] == LOG_ID)
        .collect();
    assert_eq!(facts.len(), 1, "one insert fact on {LOG_ID}: {facts:?}");
    let w = &facts[0]["witness"];
    let mut f = Vec::new();
    if w["paths"].as_array().is_none_or(|p| p.is_empty()) {
        f.push(format!("witness.paths is empty: {w}"));
    }
    for d in w["diagnostics"].as_array().into_iter().flatten() {
        if d["kind"] == "first-hop-not-found" {
            f.push(format!("witness diagnostic first-hop-not-found: {d}"));
        }
    }
    finish(f);
}

// ---------------------------------------------------------------------------
// A2b: a parameter-dependent write through the event first hop (proofs).
// ---------------------------------------------------------------------------

/// The A2b precondition. The workspace's ONLY write is the subscriber's
/// `L.Insert()`, and it is a raw parameter-dependent producer, which the BFS
/// carrier predicate refuses (`is_bfs_carrier`: `!is_pd_snap`). So a path for
/// an inherited insert can come only from `enumerate_proofs`. The Raise-rooted
/// path, whose event edge is the second hop, shows the proofs already cross
/// the event edge (`event_pair` binds `L` to `L` by name).
#[test]
fn a2b_precondition_the_only_writer_is_a_producer_and_proofs_cross_the_event() {
    let mut f = Vec::new();
    let sub = digest(PD_PUBLISHER_AL, PD_SUBSCRIBER_AL, "HandleOnX");
    let direct = db_insert(entry(&sub, "HandleOnX"));
    if direct["provenance"] != "direct" {
        f.push(format!(
            "HandleOnX insert provenance {}",
            direct["provenance"]
        ));
    }
    if !direct["detail"]["tempState"]
        .as_str()
        .is_some_and(|t| t.starts_with("parameter-dependent"))
    {
        f.push(format!(
            "HandleOnX insert tempState {} (want parameter-dependent)",
            direct["detail"]["tempState"]
        ));
    }
    let raise = digest(PD_PUBLISHER_AL, PD_SUBSCRIBER_AL, "Raise");
    let eff = db_insert(entry(&raise, "Raise"));
    let want = vec![vec![
        "call:Raise->OnX".to_string(),
        "event-dispatch:OnX->HandleOnX".to_string(),
    ]];
    if via_paths(eff) != want {
        f.push(format!(
            "Raise viaPaths {:?} (want {want:?})",
            via_paths(eff)
        ));
    }
    let onx = digest(PD_PUBLISHER_AL, PD_SUBSCRIBER_AL, "OnX");
    let onx_eff = db_insert(entry(&onx, "OnX"));
    if onx_eff["provenance"] != "transitive" {
        f.push(format!("OnX insert provenance {}", onx_eff["provenance"]));
    }
    finish(f);
}

/// A2b: the publisher's parameter-dependent insert gets a proof path whose
/// first hop is the event-dispatch edge.
#[test]
fn a2b_pd_write_through_the_event_first_hop_has_a_proof_path() {
    let doc = digest(PD_PUBLISHER_AL, PD_SUBSCRIBER_AL, "OnX");
    let eff = db_insert(entry(&doc, "OnX"));
    let mut f = Vec::new();
    // Restated here so a green run can never come from a non-producer carrier.
    if !eff["detail"]["tempState"]
        .as_str()
        .is_some_and(|t| t.starts_with("parameter-dependent"))
    {
        f.push(format!(
            "precondition: OnX insert tempState {} (want parameter-dependent)",
            eff["detail"]["tempState"]
        ));
    }
    let want = vec!["event-dispatch:OnX->HandleOnX".to_string()];
    if !via_paths(eff).contains(&want) {
        f.push(format!("viaPaths {:?} has no {want:?}", via_paths(eff)));
    }
    event_hops_carry_onx_id(eff, &mut f);
    evidence_at(eff, 5, 8, &mut f);
    finish(f);
}

// ---------------------------------------------------------------------------
// A2c: two subscribers (the event id alone names the first hop, not `to`).
// ---------------------------------------------------------------------------

/// Control: from `Raise`, both subscribers' paths already exist.
#[test]
fn control_a2c_raise_rooted_paths_reach_both_subscribers() {
    let doc = digest(PUBLISHER_AL, FAN_SUBSCRIBER_AL, "Raise");
    let eff = db_insert(entry(&doc, "Raise"));
    let mut got = via_paths(eff);
    got.sort();
    let mut f = Vec::new();
    let want: Vec<Vec<String>> = ["HandleOnX", "HandleOnX2"]
        .iter()
        .map(|to| {
            vec![
                "call:Raise->OnX".to_string(),
                format!("event-dispatch:OnX->{to}"),
            ]
        })
        .collect();
    if got != want {
        f.push(format!("viaPaths (sorted) {got:?} (want exactly {want:?})"));
    }
    finish(f);
}

/// A2c: the publisher's insert lists BOTH event-dispatch first hops.
#[test]
fn a2c_publisher_insert_lists_both_subscriber_first_hops() {
    let doc = digest(PUBLISHER_AL, FAN_SUBSCRIBER_AL, "OnX");
    let eff = db_insert(entry(&doc, "OnX"));
    let mut got = via_paths(eff);
    got.sort();
    let mut f = Vec::new();
    let want: Vec<Vec<String>> = ["HandleOnX", "HandleOnX2"]
        .iter()
        .map(|to| vec![format!("event-dispatch:OnX->{to}")])
        .collect();
    if got != want {
        f.push(format!("viaPaths (sorted) {got:?} (want exactly {want:?})"));
    }
    event_hops_carry_onx_id(eff, &mut f);
    if eff["evidence"]["sourceKind"] != "source" {
        f.push(format!(
            "evidence.sourceKind is {} (want \"source\")",
            eff["evidence"]["sourceKind"]
        ));
    }
    finish(f);
}
