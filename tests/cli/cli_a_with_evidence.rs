//! E3 — `analyze --with-evidence` opt-in (evidencePath + position-derived member
//! discriminator).
//!
//! These tests drive the REAL gate `analyze` pipeline (`run_analyze_with_exit`) over an
//! inline scratch workspace written to a temp dir (NOT an r0-corpus fixture — the corpus
//! is enumerated by the byte-parity differential, which would then demand goldens).
//!
//! Coverage (spec Revision-2 clauses):
//!   (1) RE-1/RE-2 — a TWO-FIELD table where each field carries a distinct member trigger
//!       (`OnValidate` on field 1, `OnLookup` on field 2), each body triggering a finding
//!       (d1 db-op-in-loop). Under `--with-evidence` EACH finding carries its OWN
//!       POSITION-derived `enclosingMember` (the member-WRAPPER range containing its
//!       primaryLocation, smallest-range), proving the discriminator is PER-FINDING and
//!       wrapper-range-derived — NOT looked up from the routine map (where two field
//!       triggers' internal ids collapse — RE-1). `evidencePath` present; schema `1.1.0`.
//!       (Two SAME-named field triggers collapse to one model routine and so cannot yield
//!       two findings via the native detector; distinct trigger kinds keep both routines,
//!       giving two findings in two disjoint field wrappers — the observable per-finding
//!       position-derivation surface.)
//!   (2) RE-8 — the SAME fixture under PLAIN `analyze --format json` is BYTE-IDENTICAL to
//!       the with-evidence run with every new key stripped, and `schemaVersion "1.0.0"`
//!       (no `evidencePath` / `enclosingMember` / `originatingObject` keys anywhere).
//!   (3) A finding OUTSIDE any member trigger (an object-level/procedure finding) gets NO
//!       `enclosingMember`.
//!   (4) A page_field trigger finding ALSO gets a position-derived `enclosingMember`.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

const APP_JSON: &str = r#"{
    "id": "11111111-0000-0000-0000-0000000000e3",
    "name": "E3 With Evidence",
    "publisher": "PT",
    "version": "1.0.0.0",
    "dependencies": []
}
"#;

/// Write a unique scratch workspace (`app.json` + `src/<name>`) under the temp dir.
/// Returns the guard and the workspace root; the `TempDir` removes the tree on drop
/// (panics included), so keep it alive for the whole test.
fn write_workspace(tag: &str, al_name: &str, al_src: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-e3-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(root.join("src").join(al_name), al_src).expect("write al source");
    (tmp, root)
}

/// Build the analyze args for a workspace at `ws`, with `with_evidence` toggled.
fn args_for(ws: &Path, with_evidence: bool) -> AnalyzeArgs {
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
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
        with_evidence,
    }
}

fn run_json(ws: &Path, with_evidence: bool) -> String {
    let args = args_for(ws, with_evidence);
    match run_analyze_with_exit(&args, "e3-test") {
        Ok((out, _, _)) => out,
        Err(e) => panic!("run_analyze failed: {e}"),
    }
}

fn parse(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("analyze JSON must parse")
}

// ---------------------------------------------------------------------------
// Two-field table — field 1 OnValidate, field 2 OnLookup; each body triggers a
// d1 db-op-in-loop finding. The findings sit inside DISJOINT field wrappers, so
// the per-finding POSITION discriminator must yield DISTINCT enclosingMember
// values (derived from the containing wrapper range, not the routine map).
// ---------------------------------------------------------------------------

const TWO_FIELD_TABLE: &str = r#"
table 50100 "E3 Two Field"
{
    fields
    {
        field(1; "First Field"; Integer)
        {
            trigger OnValidate()
            var
                Cust: Record "E3 Two Field";
                i: Integer;
            begin
                Cust.FindSet();
                for i := 1 to 10 do
                    Cust.Get(i);
            end;
        }
        field(2; "Second Field"; Integer)
        {
            trigger OnLookup()
            var
                Cust: Record "E3 Two Field";
                i: Integer;
            begin
                Cust.FindSet();
                for i := 1 to 10 do
                    Cust.Get(i);
            end;
        }
    }
    keys { key(PK; "First Field") { } }
}
"#;

/// Collect the d1 findings (one per field OnValidate) from the parsed analyze JSON.
fn d1_findings(doc: &serde_json::Value) -> Vec<&serde_json::Value> {
    doc["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"].as_str() == Some("d1-db-op-in-loop"))
        .collect()
}

#[test]
fn two_field_with_evidence_distinct_member_per_finding() {
    // Reader-side env serialization: the produced doc embeds `alsemVersion`
    // from the env-reading `driver_version()`, so hold the crate ENV_LOCK
    // against the `cli_a_*` writers of `ALCH_DRIVER_VERSION_OVERRIDE`.
    let _env = crate::env_guard();
    let (_tmp, ws) = write_workspace("twofield-we", "twofield.al", TWO_FIELD_TABLE);
    let doc = parse(&run_json(&ws, true));

    // schemaVersion bumps to 1.1.0 under the flag (RE-8).
    assert_eq!(
        doc["schemaVersion"].as_str(),
        Some("1.1.0"),
        "with-evidence envelope schemaVersion must be 1.1.0"
    );

    let findings = d1_findings(&doc);
    assert_eq!(
        findings.len(),
        2,
        "exactly two d1 findings (one per field OnValidate); got {findings:?}"
    );

    // Each finding carries its OWN position-derived enclosingMember (RE-1/RE-2).
    let mut members: Vec<String> = Vec::new();
    for f in &findings {
        let pl = &f["primaryLocation"];
        let member = pl["enclosingMember"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("each finding's primaryLocation must carry enclosingMember; got {pl:?}")
            })
            .to_string();
        members.push(member);

        // originatingObject = the StableObjectId of the declaring table (:-form).
        let oo = pl["originatingObject"]
            .as_str()
            .expect("originatingObject present");
        assert!(
            oo.contains(":Table:50100"),
            "originatingObject must be the declaring table StableObjectId, got {oo}"
        );

        // evidencePath present (the stable-projected call chain), at least one step,
        // each step routineId in :-form.
        let ep = f["evidencePath"]
            .as_array()
            .expect("evidencePath array present");
        assert!(
            !ep.is_empty(),
            "evidencePath must be non-empty for a d1 finding"
        );
        let first_rid = ep[0]["routineId"].as_str().expect("step routineId");
        assert!(
            first_rid.contains(':'),
            "evidencePath step routineId must be in :-form (StableRoutineId), got {first_rid}"
        );
    }

    members.sort();
    members.dedup();
    assert_eq!(
        members,
        vec!["First Field".to_string(), "Second Field".to_string()],
        "the two findings must carry DISTINCT, position-derived members (RE-1/RE-2 — derived from the containing wrapper range, NOT a routine-id lookup)"
    );
}

#[test]
fn default_output_byte_identical_minus_evidence_keys() {
    // Reader-side env serialization (see two_field test): both runs must see
    // the same `ALCH_DRIVER_VERSION_OVERRIDE` state or `alsemVersion` differs
    // and the byte-identical assertion below flakes.
    let _env = crate::env_guard();
    let (_tmp, ws) = write_workspace("twofield-default", "twofield.al", TWO_FIELD_TABLE);
    let plain = run_json(&ws, false);
    let evid = run_json(&ws, true);

    // (RE-8) default schemaVersion stays 1.0.0; no opt-in keys anywhere.
    let plain_doc = parse(&plain);
    assert_eq!(
        plain_doc["schemaVersion"].as_str(),
        Some("1.0.0"),
        "default (no flag) schemaVersion must stay 1.0.0"
    );
    assert!(
        !plain.contains("evidencePath"),
        "default output must not contain evidencePath"
    );
    assert!(
        !plain.contains("enclosingMember"),
        "default output must not contain enclosingMember"
    );
    assert!(
        !plain.contains("originatingObject"),
        "default output must not contain originatingObject"
    );

    // Stronger: the with-evidence doc, with every additive key recursively stripped and
    // schemaVersion reset to 1.0.0, must be BYTE-identical to the default doc. This proves
    // the augmentation is purely additive (RE-8 — default byte-identical).
    let mut evid_doc = parse(&evid);
    strip_evidence_keys(&mut evid_doc);
    evid_doc["schemaVersion"] = serde_json::Value::String("1.0.0".to_string());

    assert_eq!(
        evid_doc, plain_doc,
        "with-evidence doc minus the additive keys (+ schemaVersion reset) must equal the default doc"
    );
}

/// Recursively remove the opt-in additive keys from a parsed analyze doc.
fn strip_evidence_keys(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.remove("evidencePath");
            map.remove("enclosingMember");
            map.remove("originatingObject");
            for (_, child) in map.iter_mut() {
                strip_evidence_keys(child);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                strip_evidence_keys(child);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// A finding OUTSIDE any member trigger (an object-level codeunit procedure) gets
// NO enclosingMember (degrades to None — engine never throws).
// ---------------------------------------------------------------------------

const PROCEDURE_ONLY: &str = r#"
table 50101 "E3 Proc Table"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50101 "E3 Proc"
{
    procedure DoWork()
    var
        Rec: Record "E3 Proc Table";
        i: Integer;
    begin
        Rec.FindSet();
        for i := 1 to 10 do
            Rec.Get(i);
    end;
}
"#;

#[test]
fn finding_outside_member_trigger_has_no_member() {
    // Reader-side env serialization (see two_field test).
    let _env = crate::env_guard();
    let (_tmp, ws) = write_workspace("proc-only", "proc.al", PROCEDURE_ONLY);
    let doc = parse(&run_json(&ws, true));

    let findings = d1_findings(&doc);
    assert!(
        !findings.is_empty(),
        "expected at least one d1 finding in the procedure body"
    );
    for f in &findings {
        let pl = &f["primaryLocation"];
        assert!(
            pl.get("enclosingMember").is_none(),
            "a procedure-body finding must NOT carry enclosingMember (no member wrapper contains it), got {pl:?}"
        );
        assert!(
            pl.get("originatingObject").is_none(),
            "a procedure-body finding must NOT carry originatingObject, got {pl:?}"
        );
        // evidencePath is still present under the flag.
        assert!(
            f["evidencePath"].is_array(),
            "evidencePath must still be present (flag is on)"
        );
    }
}

// ---------------------------------------------------------------------------
// A page_field trigger finding ALSO gets a position-derived enclosingMember
// (E1 review noted page_field lacked coverage).
// ---------------------------------------------------------------------------

const PAGE_FIELD_TRIGGER: &str = r#"
table 50102 "E3 Page Table"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Amount; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}

page 50102 "E3 Card"
{
    PageType = Card;
    SourceTable = "E3 Page Table";
    layout
    {
        area(content)
        {
            field(AmountFld; Rec.Amount)
            {
                ApplicationArea = All;
                trigger OnValidate()
                var
                    Cust: Record "E3 Page Table";
                    i: Integer;
                begin
                    Cust.FindSet();
                    for i := 1 to 10 do
                        Cust.Get(i);
                end;
            }
        }
    }
}
"#;

#[test]
fn page_field_trigger_finding_has_member() {
    // Reader-side env serialization (see two_field test).
    let _env = crate::env_guard();
    let (_tmp, ws) = write_workspace("page-field", "page.al", PAGE_FIELD_TRIGGER);
    let doc = parse(&run_json(&ws, true));

    let findings = d1_findings(&doc);
    assert!(
        !findings.is_empty(),
        "expected a d1 finding inside the page_field OnValidate trigger"
    );
    let f = findings[0];
    let pl = &f["primaryLocation"];
    let member = pl["enclosingMember"].as_str().unwrap_or_else(|| {
        panic!("page_field trigger finding must carry enclosingMember; got {pl:?}")
    });
    assert_eq!(
        member, "AmountFld",
        "page_field member = the page field control name"
    );
}
