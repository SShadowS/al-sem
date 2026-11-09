//! Engine limits B8 (1): `alsem fingerprint` prints AL's own TableData
//! rights. AL spells them `R`, `I`, `M`, `D` (`tabledata X = RIMD`); the
//! human and query output once printed `W` for an insert, while the full
//! snapshot's `permissionFacts` said `I`. Both now read one mapping
//! (`engine::l5::table_rights`).
//!
//! The fixture states its precondition literally: one event subscriber (a
//! fingerprint root) that reads, inserts, modifies and deletes one physical
//! table, so each of the four rights is required exactly once.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

const APP_GUID: &str = "b8b8b8b8-0000-4000-8000-0000000000b8";

fn run(cwd: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--deterministic")
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn fixture() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"{APP_GUID}","name":"B8 Rights","publisher":"t","version":"1.0.0.0","dependencies":[]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/rights.al"),
        r#"table 50140 "B8 Entry"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Note; Text[50]) { }
    }
}

codeunit 50140 "B8 Publisher"
{
    [IntegrationEvent(false, false)]
    procedure OnDone()
    begin
    end;
}

codeunit 50141 "B8 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"B8 Publisher", 'OnDone', '', true, true)]
    local procedure HandleDone()
    var
        E: Record "B8 Entry";
    begin
        E.Get(1);
        E.Init();
        E."Entry No." := 2;
        E.Insert();
        E.Note := 'x';
        E.Modify();
        E.Delete();
    end;
}
"#,
    )
    .unwrap();
    t
}

/// Every `rights` string of the query output's `requiredPermissions` rows
/// for `table`, anywhere in `v`.
fn query_rights(v: &Value, table: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if m.get("targetId").and_then(Value::as_str) == Some(table)
                && let Some(r) = m.get("rights").and_then(Value::as_str)
            {
                out.push(r.to_string());
            }
            m.values().for_each(|x| query_rights(x, table, out));
        }
        Value::Array(a) => a.iter().for_each(|x| query_rights(x, table, out)),
        _ => {}
    }
}

/// Every right the full snapshot's `permissionFacts` require on a target
/// that names table 50140.
fn snapshot_rights(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Array(facts)) = m.get("permissionFacts") {
                for f in facts {
                    if f["target"].as_str().is_some_and(|t| t.ends_with(":50140")) {
                        for r in f["rights"].as_array().into_iter().flatten() {
                            out.insert(r.as_str().unwrap_or("").to_string());
                        }
                    }
                }
            }
            m.values().for_each(|x| snapshot_rights(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| snapshot_rights(x, out)),
        _ => {}
    }
}

#[test]
fn fingerprint_prints_al_table_rights() {
    let t = fixture();
    let table = format!("{APP_GUID}/table/50140");

    // The human answer (the query path).
    let (human, err, code) = run(t.path(), &["fingerprint", "ws"]);
    assert_eq!(code, 0, "{err}");
    // The `permissions:` section's line (the `writes:` and `reads:` summary
    // lines name the table too, with no rights).
    let line = human
        .lines()
        .find(|l| {
            l.trim_start()
                .starts_with(&format!("TableData \"{table}\""))
        })
        .unwrap_or_else(|| panic!("precondition: a permission line for {table}:\n{human}"));
    assert!(line.ends_with(" RIMD"), "AL's rights in AL's order: {line}");

    // The query JSON (`--witness 0` takes the query path).
    let (json, err, code) = run(
        t.path(),
        &["fingerprint", "ws", "--format", "json", "--witness", "0"],
    );
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&json).expect("json");
    let mut rights = Vec::new();
    query_rights(&v, &table, &mut rights);
    assert!(
        !rights.is_empty(),
        "precondition: a requiredPermissions row"
    );
    assert!(rights.iter().all(|r| r == "RIMD"), "{rights:?}");

    // The full snapshot (the default JSON) requires the same four rights.
    let (json, err, code) = run(t.path(), &["fingerprint", "ws", "--format", "json"]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&json).expect("json");
    let mut snap = BTreeSet::new();
    snapshot_rights(&v, &mut snap);
    let want: BTreeSet<String> = ["R", "I", "M", "D"].map(String::from).into();
    assert_eq!(snap, want, "the two surfaces read one mapping");
}
