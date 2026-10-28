//! `this.Global` as a call argument is typed as the object global it names, so
//! an overload set picked on that argument resolves. Continia CDN:
//! `this.Utilities.IsPurchaseDocument(this.CreationFactory)`, overloaded on
//! `Codeunit "CTS-CDN eDoc. Creation Factory"` against `Integer`, stayed an
//! ambiguous pair (and blocked `prove`) while the bare `Factory` resolved.

use std::process::Command;

const SOURCE: &str = r#"codeunit 50320 "OV Factory"
{
    procedure Number(): Integer
    begin
        exit(1);
    end;
}
codeunit 50321 "OV Util"
{
    internal procedure IsPurchase(F: Codeunit "OV Factory"): Boolean
    begin
        exit(IsPurchase(F.Number()));
    end;

    internal procedure IsPurchase(N: Integer): Boolean
    begin
        exit(N = 1);
    end;
}
codeunit 50322 "OV Caller"
{
    var
        Factory: Codeunit "OV Factory";
        Util: Codeunit "OV Util";

    procedure Plain(): Boolean
    begin
        exit(Util.IsPurchase(Factory));
    end;

    procedure WithThis(): Boolean
    begin
        exit(this.Util.IsPurchase(this.Factory));
    end;
}
"#;

#[test]
fn a_this_global_argument_picks_its_overload() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"31313131-0000-4000-8000-000000000031","name":"OV","publisher":"t","version":"1.0.0.0","runtime":"14.0"}"#,
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
    // The Codeunit-parameter overload (declared at line 10).
    let codeunit_overload = rows
        .iter()
        .find(|r| r["t"] == "node" && r["v"]["memberName"] == "IsPurchase" && r["v"]["line"] == 10)
        .expect("precondition: the Codeunit overload is a node")["v"]["id"]
        .clone();
    let edges_at = |line: u64| -> Vec<&serde_json::Value> {
        rows.iter()
            .filter(|r| r["t"] == "edge" && r["v"]["line"] == line)
            .map(|r| &r["v"])
            .collect()
    };
    // Precondition: the bare argument resolves to that overload.
    let plain = edges_at(28);
    assert_eq!(plain.len(), 1, "precondition: {plain:?}");
    assert_eq!(plain[0]["callee"], codeunit_overload, "{plain:?}");

    let this = edges_at(33);
    assert_eq!(
        this.len(),
        1,
        "one target, not the ambiguous pair: {this:?}"
    );
    assert_eq!(this[0]["callee"], codeunit_overload, "{this:?}");
    assert_eq!(this[0]["confidence"], "Resolved", "{this:?}");
}
