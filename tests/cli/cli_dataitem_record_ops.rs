//! A report or xmlport dataitem is a record variable in every routine of its
//! object, so `"Job Ledger Entry".Modify()` from a nested dataitem's trigger,
//! or `ICOutboxTrans.Insert()` on an xmlport table element from a procedure,
//! is a record operation on the dataitem's table. The body pass took only
//! globals, parameters and locals as record receivers, so these became calls
//! the model could not type: `digest` listed each as
//! `unresolved-receiver-type`, and `prove` blocked on it (Base Application's
//! Update Job Item Cost, IC Outbox Imp/Exp). The resolver already typed them.
//! Each write names its table (a quoted receiver, dataitem or local, missed
//! its variable in the record typing).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50290 "DI Ledger Entry"
{
    fields { field(1; K; Integer) { } field(2; "Unit Price"; Decimal) { } }
    keys { key(PK; K) { } }
}
table 50291 "DI Planning Line"
{
    fields { field(1; K; Integer) { } field(2; "Unit Price"; Decimal) { } }
    keys { key(PK; K) { } }
}
report 50292 "DI Report"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem("DI Ledger Entry"; "DI Ledger Entry")
        {
            dataitem("DI Planning Line"; "DI Planning Line")
            {
                trigger OnAfterGetRecord()
                begin
                    "DI Ledger Entry".Validate("Unit Price", "Unit Price");
                    "DI Ledger Entry".Modify();
                end;
            }
        }
    }
}
xmlport 50293 "DI Port"
{
    schema
    {
        textelement(Root)
        {
            tableelement(DIOutbox; "DI Planning Line")
            {
            }
            tableelement(DITempBox; "DI Temp")
            {
                UseTemporary = true;
            }
        }
    }

    procedure SetOutbox()
    begin
        DIOutbox.DeleteAll();
        DIOutbox.Insert();
        DITempBox.Insert();
    end;
}
table 50296 "DI Temp"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50294 "DI Quoted"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50295 "DI Quoted Writer"
{
    procedure Write()
    var
        "My Rec": Record "DI Quoted";
    begin
        "My Rec".Insert();
    end;
}
"#;

fn digest(ws: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("digest")
        .arg(ws)
        .args(["--file", "src/Probe.al", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// Every `{calleeDisplay, status}` row anywhere in the digest.
fn unresolved_rows(v: &serde_json::Value, out: &mut Vec<(String, String)>) {
    match v {
        serde_json::Value::Object(m) => {
            if let (Some(c), Some(s)) = (m.get("calleeDisplay"), m.get("status")) {
                out.push((
                    c.as_str().unwrap_or("").into(),
                    s.as_str().unwrap_or("").into(),
                ));
                return;
            }
            m.values().for_each(|x| unresolved_rows(x, out));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| unresolved_rows(x, out)),
        _ => {}
    }
}

#[test]
fn dataitem_record_ops_are_record_operations() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"29292929-0000-4000-8000-000000000029","name":"DI","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50290,"to":50299}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/Probe.al"), SOURCE).unwrap();
    let d = digest(t.path());
    let mut rows = Vec::new();
    unresolved_rows(&d, &mut rows);
    assert!(
        rows.iter().all(|(_, s)| s != "unresolved-receiver-type"),
        "{rows:?}"
    );
    let text = d.to_string();
    // The quoted local's write too: an operation keeps its receiver's raw
    // text, which missed the variable's logical name.
    // A table element's `UseTemporary = true` makes its record temporary; one
    // without it is the physical table.
    let temp_state = |id: u32| {
        let key = format!(
            "\"resourceId\":\"29292929-0000-4000-8000-000000000029/table/{id}\",\"tempState\":\""
        );
        let at = text.find(&key).map(|i| i + key.len())?;
        Some(text[at..].split('"').next()?.to_string())
    };
    assert_eq!(temp_state(50296).as_deref(), Some("known(true)"), "{text}");
    assert_eq!(temp_state(50291).as_deref(), Some("known(false)"), "{text}");
    for table in ["DI Ledger Entry", "DI Planning Line", "DI Quoted"] {
        assert!(
            text.contains(&format!("\"resourceDisplay\":\"{table}\"")),
            "a write to {table}: {text}"
        );
    }
}
