//! G13: one id and declared line bases across `alsem graph`, `fingerprint
//! --inventory-only` and `prove`, so a consumer can join them.
//!
//! - Graph nodes carry the StableRoutineId the other commands use, minted from
//!   the graph's own declaration data; it must equal the inventory's (model-
//!   minted) id for every routine, overloads and a field trigger included.
//! - Inventory rows carry `arity` and `sourceAnchor`, the same anchor `prove`
//!   reports as `routine.anchor`.
//! - Each document says its line base: graph `meta.lineBase` 1, prove and
//!   fingerprint `payload.lineBase` 0; a graph node's `line` is the inventory
//!   anchor's line + 1.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50130 "G13 T"
{
    fields
    {
        field(1; Code; Code[20])
        {
            trigger OnValidate()
            begin
            end;
        }
    }
    keys { key(PK; Code) { } }
}
codeunit 50131 "G13 C"
{
    procedure P(X: Integer)
    begin
    end;

    procedure P(X: Text)
    begin
    end;

    procedure Q(var R: Record "G13 T"): Boolean
    begin
    end;
}
"#;

fn run(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn graph_inventory_and_prove_share_ids_and_declare_line_bases() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"13131313-0000-4000-8000-000000000013","name":"G13","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/S.al"), SOURCE).unwrap();

    let inv: serde_json::Value =
        serde_json::from_str(&run(t.path(), &["fingerprint", "ws", "--inventory-only"])).unwrap();
    assert_eq!(inv["payload"]["lineBase"], 0);
    let rows = inv["payload"]["routineInventory"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "P, P, Q and the field trigger: {rows:?}");
    let by_id: HashMap<&str, &serde_json::Value> = rows
        .iter()
        .map(|r| (r["stableRoutineId"].as_str().unwrap(), r))
        .collect();

    let graph = run(t.path(), &["graph", "ws"]);
    let mut lines = graph
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap());
    let meta = lines.next().unwrap();
    assert_eq!(meta["t"], "meta");
    assert_eq!(meta["v"]["lineBase"], 1);
    let routine_nodes: Vec<serde_json::Value> = lines
        .filter(|l| {
            l["t"] == "node" && matches!(l["v"]["kind"].as_str(), Some("Method" | "Trigger"))
        })
        .map(|l| l["v"].clone())
        .collect();
    assert_eq!(routine_nodes.len(), 4, "{routine_nodes:?}");
    for n in &routine_nodes {
        let id = n["stableRoutineId"]
            .as_str()
            .unwrap_or_else(|| panic!("a source node has an id: {n}"));
        let row = by_id
            .get(id)
            .unwrap_or_else(|| panic!("the graph id is the inventory's: {n}"));
        assert_eq!(n["arity"], row["arity"], "{n} vs {row}");
        assert_eq!(
            n["line"].as_u64().unwrap(),
            row["sourceAnchor"]["line"].as_u64().unwrap() + 1,
            "graph lines are 1-based, inventory anchors 0-based: {n} vs {row}"
        );
    }

    for row in rows {
        let id = row["stableRoutineId"].as_str().unwrap();
        let doc: serde_json::Value = serde_json::from_str(&run(
            t.path(),
            &["prove", "ws", id, "may-commit", "--deterministic"],
        ))
        .unwrap();
        assert_eq!(doc["payload"]["lineBase"], 0);
        assert_eq!(
            doc["payload"]["routine"]["stableId"],
            row["stableRoutineId"]
        );
        assert_eq!(
            doc["payload"]["routine"]["anchor"], row["sourceAnchor"],
            "the inventory anchor is prove's"
        );
    }
}
