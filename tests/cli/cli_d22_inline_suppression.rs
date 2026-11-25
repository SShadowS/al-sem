//! ENG-101 sol r1: an inline `// al-sem-ignore d22-flowfield-without-calcfields`
//! keeps suppressing per READ after d22 merged the reads of one FlowField into one
//! finding. The merged finding is suppressed only when a directive covers EVERY read
//! it lists; a directive above some reads keeps the finding (all its reads listed).
//! Run through the real binary, so the analyze pipeline's use is pinned, not the
//! helper.
//!
//! Precondition, stated in the workspace below: `OnlyFirstIgnored` reads
//! `Customer."Balance (LCY)"` twice (a `Get` between), with a directive above the
//! first read only; `EveryReadIgnored` reads it twice with a directive above each.
//!
//! Discrimination (2026-11-25): `run.rs` passing no merged-site lines to
//! `apply_inline_suppressions` (the primary-line rule alone) -> "only
//! OnlyFirstIgnored stays" fails (`OnlyFirstIgnored` is suppressed by the directive
//! above its first read). Restored, green.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const CODEUNIT: &str = r#"codeunit 50890 "D22 Inline"
{
    procedure OnlyFirstIgnored()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        // al-sem-ignore d22-flowfield-without-calcfields: the default value is wanted here
        Amount := Customer."Balance (LCY)";
        Customer.Get('C0002');
        Amount := Customer."Balance (LCY)";
    end;

    procedure EveryReadIgnored()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        // al-sem-ignore d22-flowfield-without-calcfields: wanted
        Amount := Customer."Balance (LCY)";
        // al-sem-ignore d22-flowfield-without-calcfields: wanted
        Amount += Customer."Balance (LCY)";
    end;
}
"#;

#[test]
fn a_merged_d22_finding_is_suppressed_only_when_every_read_is() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d22");
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("src")).expect("mkdir");
    std::fs::copy(corpus.join("app.json"), ws.join("app.json")).expect("app.json");
    std::fs::copy(corpus.join("src/Tables.al"), ws.join("src/Tables.al")).expect("tables");
    std::fs::write(ws.join("src/Codeunit.al"), CODEUNIT).expect("codeunit");

    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("analyze")
        .arg(ws)
        .args([
            "--detector",
            "d22-flowfield-without-calcfields",
            "--format",
            "json",
        ])
        .output()
        .expect("run alsem");
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("json ({e}): {}", String::from_utf8_lossy(&out.stderr)));
    let flagged: BTreeSet<String> = doc["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rootCause"].as_str())
        .filter_map(|t| t.split(' ').next())
        .map(str::to_string)
        .collect();
    assert_eq!(
        flagged,
        BTreeSet::from(["OnlyFirstIgnored".to_string()]),
        "only OnlyFirstIgnored stays"
    );
}
