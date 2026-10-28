//! A LOCAL RecordRef whose every `Open` is `Open(_, true)` holds a temporary
//! table on every path, also in a routine that branches: its record ops are
//! `known(true)`. Continia DO's `TryMatchTypedFilter` opens `CDO Typed Value
//! Buffer` temporary under a `case`, and its `RecRef.Insert` read `unknown`.
//! The rule needs that nothing else binds it: an `Open` without `true`, a
//! `GetTable`, an assignment, or passing it on leaves it `unknown`.

use std::process::Command;

fn source(extra: &str) -> String {
    format!(
        r#"table 50360 "RR Buffer"
{{
    fields {{ field(1; Code; Code[20]) {{ }} }}
    keys {{ key(PK; Code) {{ }} }}
}}
codeunit 50361 "RR Cases"
{{
    procedure Stage(Kind: Integer)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(Database::"RR Buffer", true);
        case Kind of
            1:
                RecRef.Field(1).Value := 'A';
            2:
                RecRef.Field(1).Value := 'B';
        end;
{extra}
        RecRef.Insert(false);
    end;
}}
"#
    )
}

/// The `tempState` of `Stage`'s `RecRef.Insert` effect.
fn insert_temp_state(extra: &str) -> String {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"36363636-0000-4000-8000-000000000036","name":"RR","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), source(extra)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["digest", "ws", "--routine", "Stage", "--format", "json"])
        .current_dir(t.path())
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let effects: Vec<&serde_json::Value> = doc["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| e["effects"].as_array().unwrap())
        .filter(|f| f["type"] == "DB_INSERT")
        .collect();
    assert_eq!(effects.len(), 1, "precondition: one insert: {effects:?}");
    effects[0]["detail"]["tempState"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

#[test]
fn a_local_recordref_opened_temporary_is_temporary_under_branching() {
    assert_eq!(insert_temp_state(""), "known(true)");
}

#[test]
fn anything_else_binding_it_leaves_it_unknown() {
    for (case, extra) in [
        (
            "an Open without true",
            "        RecRef.Open(Database::\"RR Buffer\");",
        ),
        ("a GetTable", "        RecRef.GetTable(RecRef);"),
    ] {
        assert_eq!(insert_temp_state(extra), "unknown", "{case}");
    }
}
