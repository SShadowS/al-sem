//! G15: `alsem prove <ws> <routine> reaches-ui` on every way AL code opens UI.
//!
//! It answered `no` with full coverage claimed for Continia DO's
//! `CDO Signature FactBox` field trigger `OnDrillDown`, whose body is a
//! `Page.Run(Page::"…", Signature)`: `prove` counted only `UI_MESSAGE`,
//! `UI_CONFIRM` and `UI_ERROR`, so the `UI_WINDOW_OPEN` effect the digest had
//! was ignored. A `Run()` on a Page-typed variable produced no effect at all,
//! and `Hyperlink` was typed `FILE`. A confident wrong `no` is the worst answer
//! for a verifier: it marks a correct claim as contradicted.
//!
//! Each routine of one temp workspace is asked through the real binary; the
//! evidence must name the call. `NoUi` is the control: a complete cone with no
//! UI still answers `no`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50150 "G15 T"
{
    fields { field(1; Code; Code[20]) { } }
}
page 50151 "G15 List"
{
    SourceTable = "G15 T";
}
codeunit 50152 "G15 Shapes"
{
    procedure ObjectRunWithRecord()
    var
        T: Record "G15 T";
    begin
        Page.Run(Page::"G15 List", T);
    end;

    procedure PageVarRun()
    var
        P: Page "G15 List";
    begin
        P.Run();
    end;

    procedure PageVarRunModal()
    var
        P: Page "G15 List";
    begin
        P.RunModal();
    end;

    procedure ReportRun()
    begin
        Report.Run(50199);
    end;

    procedure HyperlinkCall()
    begin
        Hyperlink('https://example.com');
    end;

    procedure NoUi(var T: Record "G15 T")
    begin
        T.Code := 'X';
    end;
}
page 50153 "G15 FactBox"
{
    PageType = CardPart;
    SourceTable = "G15 T";
    layout
    {
        area(Content)
        {
            field("Count"; Rec.Code)
            {
                trigger OnDrillDown()
                var
                    T: Record "G15 T";
                begin
                    T.SetRange(Code, Rec.Code);
                    Page.Run(Page::"G15 List", T);
                end;
            }
        }
    }
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn every_ui_opening_call_reaches_ui() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"15151515-0000-4000-8000-000000000015","name":"G15","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();

    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &["prove", "ws", &id_of(name), "reaches-ui", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };

    for (routine, excerpt) in [
        ("ObjectRunWithRecord", "Page.Run"),
        ("PageVarRun", "P.Run"),
        ("PageVarRunModal", "P.RunModal"),
        ("ReportRun", "Report.Run"),
        ("HyperlinkCall", "Hyperlink"),
        ("OnDrillDown", "Page.Run"),
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let evidence: Vec<&str> = r["evidence"]
            .as_array()
            .unwrap_or_else(|| panic!("{routine}: no evidence: {r}"))
            .iter()
            .filter_map(|e| e["evidence"]["excerpt"].as_str())
            .collect();
        assert!(
            evidence.iter().any(|x| x.starts_with(excerpt)),
            "{routine}: the evidence must name {excerpt:?}: {evidence:?}"
        );
    }

    let control = prove("NoUi");
    assert_eq!(control["answer"], "no", "the control has no UI: {control}");
    assert_eq!(
        control["obligations"]["coverageComplete"], true,
        "precondition: the control's cone is complete, so `no` is a real answer"
    );
}
