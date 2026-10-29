//! An undeclared `Rec`/`xRec` as a call argument is typed as the object's
//! implicit record, so an overload set picked on that argument resolves.
//! Continia DO: `LineAttachment.SetTemplateFilter(xRec)` in a table's
//! `OnDelete`, overloaded on two record types, stayed an ambiguous pair and
//! blocked `prove`.

use std::process::Command;

const SOURCE: &str = r#"table 50350 "RC Line"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    trigger OnDelete()
    var
        Att: Record "RC Attachment";
    begin
        Att.SetLineFilter(Rec);
    end;

    trigger OnModify()
    var
        Att: Record "RC Attachment";
    begin
        Att.SetLineFilter(xRec);
    end;
}
table 50351 "RC Other Line"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50352 "RC Attachment"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    procedure SetLineFilter(L: Record "RC Line")
    begin
        SetRange(Code, L.Code);
    end;

    procedure SetLineFilter(L: Record "RC Other Line")
    begin
        SetRange(Code, L.Code);
    end;
}
"#;

#[test]
fn an_implicit_rec_argument_picks_its_overload() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"35353535-0000-4000-8000-000000000035","name":"RC","publisher":"t","version":"1.0.0.0","runtime":"14.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["graph", "ws"])
        .current_dir(t.path())
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let rows: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // The `Record "RC Line"` overload (declared at line 30).
    let line_overload = rows
        .iter()
        .find(|r| {
            r["t"] == "node" && r["v"]["memberName"] == "SetLineFilter" && r["v"]["line"] == 30
        })
        .expect("precondition: the RC Line overload is a node")["v"]["id"]
        .clone();
    for (arg, line) in [("Rec", 10), ("xRec", 17)] {
        let edges: Vec<&serde_json::Value> = rows
            .iter()
            .filter(|r| r["t"] == "edge" && r["v"]["line"] == line)
            .map(|r| &r["v"])
            .collect();
        assert_eq!(edges.len(), 1, "{arg}: one target, not the pair: {edges:?}");
        assert_eq!(edges[0]["callee"], line_overload, "{arg}: {edges:?}");
        assert_eq!(edges[0]["confidence"], "Resolved", "{arg}: {edges:?}");
    }
}
