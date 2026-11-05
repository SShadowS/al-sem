//! `alsem query touches --table T --field F` (agent-tasks design §7.8,
//! D-1..D-8), through the shipped binary so the clap arm, the job's plan and
//! the renderer are all pinned. The fixture is `tests/fixtures/field-touches`
//! (its header holds the hand counts); D-4 builds a workspace with an
//! embedded-source dependency.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/field-touches/ws")
}

/// `alsem query touches <ws> <args> --format json --deterministic`.
fn touches(ws: &Path, args: &[&str]) -> (Value, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("query")
        .arg("touches")
        .arg(ws)
        .args(args)
        .args(["--format", "json", "--deterministic"])
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{e}: {text}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (v, out.status.code().unwrap_or(-1))
}

fn amount() -> Value {
    let (v, code) = touches(&fixture(), &["--table", "FT Item", "--field", "Amount"]);
    assert_eq!(code, 0, "{v}");
    v["payload"].clone()
}

/// The 1-based line of the fixture that holds `snippet` (exactly one).
fn line_of(snippet: &str) -> u64 {
    let text = std::fs::read_to_string(fixture().join("src/FTFields.al")).unwrap();
    let hits: Vec<u64> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(snippet))
        .map(|(i, _)| i as u64 + 1)
        .collect();
    assert_eq!(hits.len(), 1, "fixture line {snippet:?}: {hits:?}");
    hits[0]
}

/// The sites at `line`, each as its uses.
fn sites_at(p: &Value, line: u64) -> Vec<Vec<String>> {
    p["fieldTouches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["site"]["line"].as_u64() == Some(line))
        .map(|s| {
            s["uses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

fn site_at(p: &Value, line: u64) -> Value {
    let all: Vec<&Value> = p["fieldTouches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["site"]["line"].as_u64() == Some(line))
        .collect();
    assert_eq!(all.len(), 1, "one site at line {line}: {all:?}");
    all[0].clone()
}

/// D-1: every kind, temporary and persistent, each site once with its kind
/// and uses; the totals are the fixture header's hand counts.
#[test]
fn every_kind_once_with_hand_counted_totals() {
    let p = amount();
    let s = &p["fieldSummary"];
    assert_eq!(s["sites"], 23, "{s}");
    assert_eq!(s["routines"], 5, "{s}");
    assert_eq!(s["objects"], 3, "{s}");
    assert_eq!(s["candidates"], 3, "{s}");
    let uses: BTreeMap<String, u64> = s["uses"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
        .collect();
    let want: BTreeMap<String, u64> = [
        ("read", 4),
        ("write", 5),
        ("validate", 1),
        ("filter", 3),
        ("key-use", 1),
        ("load", 1),
        ("calc", 1),
        ("rule", 3),
        ("metadata", 1),
        ("copy-candidate", 2),
        ("by-number", 2),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_string(), n))
    .collect();
    assert_eq!(uses, want);
    // One site per kind-bearing line, with its kind.
    for (snippet, kind) in [
        ("Item.Validate(Amount, 5)", "validate"),
        ("Item.SetRange(Amount, 0, 10)", "filter"),
        ("Item.SetFilter(Amount", "filter"),
        ("Item.SetCurrentKey(Amount", "key-use"),
        ("Item.SetLoadFields(", "load"),
        ("Item.CalcSums(Amount)", "calc"),
        ("Item.TestField(Amount)", "rule"),
        ("Item.FieldError(Amount)", "rule"),
        ("Item.FieldCaption(Amount)", "metadata"),
        ("Item.ModifyAll(Amount, 0)", "write"),
    ] {
        assert_eq!(
            sites_at(&p, line_of(snippet)),
            [vec![kind.to_string()]],
            "{snippet}"
        );
    }
    // Temporary and persistent consumers are both listed.
    assert_eq!(
        site_at(&p, line_of("Tmp.Amount := 1"))["tempState"],
        "temporary"
    );
    assert_eq!(
        site_at(&p, line_of("if Amount > 0"))["tempState"],
        "persistent"
    );
    assert_eq!(
        site_at(&p, line_of("if Amount > 0"))["source"],
        "implicit-access"
    );
    // The table trigger's bare field: a write and a read on one line, and
    // its TestField as a rule.
    let mut trigger = sites_at(&p, line_of("Amount := Amount + 1"));
    trigger.sort();
    assert_eq!(
        trigger,
        [vec!["read".to_string()], vec!["write".to_string()]]
    );
    // The declared key.
    assert_eq!(
        p["declaredKeys"][0]["fields"],
        serde_json::json!(["Amount", "Unit Price"])
    );
}

/// D-2: `Item.Amount += 1` is one site with uses read and write.
#[test]
fn compound_assignment_is_one_site_with_two_uses() {
    let p = amount();
    assert_eq!(
        sites_at(&p, line_of("Item.Amount += 1;")),
        [vec!["read".to_string(), "write".to_string()]]
    );
    assert_eq!(p["fieldSummary"]["readWriteSites"], 1);
}

/// D-3: `Next(2)` and `Get('X')` take a step and a key value, not a field.
#[test]
fn next_and_get_arguments_are_not_fields() {
    let p = amount();
    for snippet in ["Item.Next(2);\n", "Item.Get('X')"] {
        let snippet = snippet.trim_end();
        // `Item.Next(2);` appears twice (Kinds and OtherFieldOnly).
        let text = std::fs::read_to_string(fixture().join("src/FTFields.al")).unwrap();
        for (i, l) in text.lines().enumerate() {
            if l.contains(snippet) {
                assert!(
                    sites_at(&p, i as u64 + 1).is_empty(),
                    "line {} ({snippet}) is no consumer",
                    i + 1
                );
            }
        }
    }
    assert!(
        p["fieldTouches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["routine"]["display"] != "OtherFieldOnly"),
        "OtherFieldOnly uses only Unit Price and Next(2)"
    );
    // A local named `Amount` as Next's step and Get's key value: no site.
    assert!(
        p["fieldTouches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["routine"]["display"] != "ValuesNotFields"),
        "Next(Amount) / Get(Amount) pass a local, not the field"
    );
}

/// D-4: a dependency routine touching a dependency table's field is listed
/// (the scan is not restricted to the primary app, as `field_writes` is).
#[test]
fn a_dependency_routine_is_listed() {
    let tmp = tempfile::tempdir().unwrap();
    dd::write_own_inner(
        tmp.path(),
        "",
        "Cust.\"No.\" := 'A';\n        Cust.Modify();",
    );
    let (v, code) = touches(tmp.path(), &["--table", "Customer", "--field", "No."]);
    assert_eq!(code, 0, "{v}");
    let p = &v["payload"];
    let dep: Vec<&Value> = p["fieldTouches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["role"] == "dependency")
        .collect();
    assert_eq!(dep.len(), 1, "{p}");
    assert_eq!(dep[0]["routine"]["display"], "Inner");
    assert_eq!(dep[0]["uses"], serde_json::json!(["write"]));
    assert!(p["searched"]["dependencyRoutineBodies"].as_u64().unwrap() > 0);
    // Single-app: no dependency code, and the answer says so.
    // (The dependency's Customer is no table of the workspace alone, so the
    // single-app question is about the workspace's own table.)
    let (v, code) = touches(
        tmp.path(),
        &["--table", "Ws Flag", "--field", "Entry No.", "--single-app"],
    );
    assert_eq!(code, 0, "{v}");
    let kinds: Vec<&str> = v["payload"]["searched"]["notSearched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"dependency-code"), "{kinds:?}");
}

/// D-5: `TransferFields` is a copy candidate on both sides; one whose other
/// table has no field with F's number is not listed.
#[test]
fn transfer_fields_is_a_copy_candidate_on_both_sides() {
    let p = amount();
    let src = site_at(&p, line_of("Copy.TransferFields(Item)"));
    assert_eq!(src["uses"], serde_json::json!(["copy-candidate"]));
    assert_eq!(src["copySide"], "source");
    let recv = site_at(&p, line_of("Item.TransferFields(Copy)"));
    assert_eq!(recv["copySide"], "receiver");
    assert!(sites_at(&p, line_of("NoMatch.TransferFields(Item)")).is_empty());
}

/// D-6: a field access on a record of an unresolved table is a candidate,
/// not a match; another table's field of the same name is neither.
#[test]
fn an_unknown_table_access_is_a_candidate() {
    let p = amount();
    let s = site_at(&p, line_of("Missing.Amount := 1"));
    assert_eq!(s["candidate"], "table-not-resolved");
    assert_eq!(
        site_at(&p, line_of("Missing.SetRange(Amount)"))["candidate"],
        "table-not-resolved"
    );
    assert!(sites_at(&p, line_of("Copy.Amount := 2")).is_empty());
    // A matched site carries no candidate.
    assert!(site_at(&p, line_of("Item.Validate(Amount, 5)"))["candidate"].is_null());
}

/// D-7: `FieldIndex(2)` is the second field, never field 2; `Field(2)` on a
/// RecordRef whose table the routine decides is a match, on a parameter a
/// candidate.
#[test]
fn field_index_never_matches_and_field_number_needs_a_table() {
    let p = amount();
    assert!(sites_at(&p, line_of("RecRef.FieldIndex(2)")).is_empty());
    assert!(sites_at(&p, line_of("RecRef.Field(3)")).is_empty());
    assert_eq!(p["fieldSummary"]["fieldIndexSites"], 1);
    assert_eq!(p["fieldSummary"]["byNumberNotLiteral"], 1);
    let text = std::fs::read_to_string(fixture().join("src/FTFields.al")).unwrap();
    let lines: Vec<u64> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("RecRef.Field(2)"))
        .map(|(i, _)| i as u64 + 1)
        .collect();
    assert_eq!(lines.len(), 2);
    let known = site_at(&p, lines[0]);
    assert_eq!(known["uses"], serde_json::json!(["by-number"]));
    assert!(known["candidate"].is_null(), "{known}");
    assert_eq!(
        site_at(&p, lines[1])["candidate"],
        "recordref-table-unknown"
    );
}

/// D-8: the kinds v1 does not search are stated, and an answer with no
/// site is "no indexed consumer observed", never no.
#[test]
fn what_is_not_searched_is_stated() {
    let p = amount();
    let kinds: Vec<&str> = p["searched"]["notSearched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["kind"].as_str().unwrap())
        .collect();
    for k in ["calcformula", "tablerelation", "columns"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    let (v, code) = touches(&fixture(), &["--table", "FT NoMatch", "--field", "Unused"]);
    assert_eq!(code, 0, "{v}");
    let a = &v["payload"]["absence"];
    assert_eq!(a["answer"], "unknown", "{v}");
    assert_eq!(a["statement"], "no indexed consumer observed");
    assert_eq!(
        a["reasons"][0]["kind"],
        "declaration-consumers-not-searched"
    );
}
