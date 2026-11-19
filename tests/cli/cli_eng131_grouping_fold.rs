//! ENG-131 (sol): detectors that group record operations by their variable's name group with
//! the identifier fold. `Aẞ` and `Aß` are two variables to the compiler (alc 18.0.41 keeps
//! `ẞ` apart from `ß`), where `str::to_lowercase` merged them: d4 read two different
//! variables' `Get`s as one repeated lookup, and d18 deduplicated two variables' filters into
//! one finding.

use serde_json::Value;
use std::process::Command;

fn analyze(src: &str) -> Value {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"13113113-0000-4000-8000-000000000132","name":"G131","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/G.al"), src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("analyze")
        .arg(t.path())
        .args(["--format", "json", "--deterministic", "--single-app"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn count(v: &Value, detector: &str) -> usize {
    distinct(v, detector).0
}

/// The `detector` findings' count, and whether their ids and their fingerprints are all
/// distinct (sol: two findings on one id let one baseline entry suppress both).
fn distinct(v: &Value, detector: &str) -> (usize, bool) {
    let fs: Vec<&Value> = v["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| f["detector"] == detector)
        .collect();
    let ids: std::collections::HashSet<String> = fs.iter().map(|f| f["id"].to_string()).collect();
    let fps: std::collections::HashSet<String> =
        fs.iter().map(|f| f["fingerprint"].to_string()).collect();
    (fs.len(), ids.len() == fs.len() && fps.len() == fs.len())
}

const TABLES: &str = "table 18 Customer { fields { field(1; \"No.\"; Code[20]) { } field(2; Status; Option) { OptionMembers = Open,Closed; } field(3; \"Fẞ\"; Integer) { } field(4; \"Fß\"; Integer) { } } }\ntable 37 \"Sales Line\" { fields { field(1; \"Document No.\"; Code[20]) { } } }\n";

#[test]
fn two_variables_spelled_with_sharp_s_and_capital_sharp_s_are_two() {
    assert_ne!(
        al_syntax::fold_identifier("Aẞ"),
        al_syntax::fold_identifier("Aß"),
        "precondition: two identifiers"
    );
    assert_eq!("Aẞ".to_lowercase(), "Aß".to_lowercase(), "precondition");
    // d4: one Get each, in one loop, on the same key: no repeated lookup.
    let d4 = analyze(&format!(
        "codeunit 50131 G\n{{\n    procedure Lookup()\n    var\n        SalesLine: Record \"Sales Line\";\n        Aẞ: Record Customer;\n        Aß: Record Customer;\n    begin\n        if SalesLine.FindSet() then\n            repeat\n                Aẞ.Get('X');\n                Aß.Get('X');\n            until SalesLine.Next() = 0;\n    end;\n}}\n{TABLES}"
    ));
    assert_eq!(count(&d4, "d4-repeated-lookup-in-loop"), 0, "d4: {d4:#}");
    // d18: each variable's constant filter in the loop is its own finding.
    let d18 = analyze(&format!(
        "codeunit 50131 G\n{{\n    procedure Filters()\n    var\n        Aẞ: Record Customer;\n        Aß: Record Customer;\n        i: Integer;\n    begin\n        for i := 1 to 10 do begin\n            Aẞ.SetRange(Status, Aẞ.Status::Open);\n            Aẞ.FindFirst();\n            Aß.SetRange(Status, Aß.Status::Open);\n            Aß.FindFirst();\n        end;\n    end;\n}}\n{TABLES}"
    ));
    assert_eq!(
        distinct(&d18, "d18-constant-filter-in-loop"),
        (2, true),
        "d18: two findings, distinct ids and fingerprints: {d18:#}"
    );
    // d4: each variable repeats its own lookup: two findings, never one id.
    let d4 = analyze(&format!(
        "codeunit 50131 G\n{{\n    procedure Lookup()\n    var\n        SalesLine: Record \"Sales Line\";\n        Aẞ: Record Customer;\n        Aß: Record Customer;\n    begin\n        if SalesLine.FindSet() then\n            repeat\n                Aẞ.Get('X');\n                Aẞ.Get('X');\n                Aß.Get('X');\n                Aß.Get('X');\n            until SalesLine.Next() = 0;\n    end;\n}}\n{TABLES}"
    ));
    assert_eq!(
        distinct(&d4, "d4-repeated-lookup-in-loop"),
        (2, true),
        "d4: two findings, distinct ids and fingerprints: {d4:#}"
    );
    // d18: one variable, two fields the compiler keeps apart: two findings.
    let fields = analyze(&format!(
        "codeunit 50131 G\n{{\n    procedure Filters()\n    var\n        C: Record Customer;\n        i: Integer;\n    begin\n        for i := 1 to 10 do begin\n            C.SetRange(\"Fẞ\", 1);\n            C.SetRange(\"Fß\", 1);\n            C.FindFirst();\n        end;\n    end;\n}}\n{TABLES}"
    ));
    assert_eq!(
        distinct(&fields, "d18-constant-filter-in-loop"),
        (2, true),
        "d18 fields: {fields:#}"
    );
}
