//! ENG-27: d56 reported a copy into a TEMPORARY buffer as "an extra SQL
//! round-trip", and twice when two copies preceded one write; d14 reported a
//! procedure once per `#if` arm under one id. Found in ENG-25's CDO triage
//! (`CDOSetupWizardUserSetup.Page.al:57`, Base App `PageManagement`).

use std::process::Command;

const SOURCE: &str = r#"
table 52800 "E27 Row"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; Code; Code[20]) { }
    }
    keys { key(PK; K) { } }
}
page 52801 "E27 Temp Page"
{
    PageType = List;
    SourceTable = "E27 Row";
    SourceTableTemporary = true;
    layout { area(Content) { repeater(G) { field(K; Rec.K) { } } } }

    procedure Fill()
    var
        Src: Record "E27 Row";
    begin
        if Src.FindSet() then
            repeat
                Rec := Src;
                if Rec.Find() then begin
                    if Rec.Code = '' then begin
                        Rec := Src;
                        Rec.Modify();
                    end;
                end else
                    Rec.Insert();
            until Src.Next() = 0;
    end;
}
page 52802 "E27 Real Page"
{
    PageType = List;
    SourceTable = "E27 Row";
    layout { area(Content) { repeater(G) { field(K; Rec.K) { } } } }

    procedure Fill()
    var
        Src: Record "E27 Row";
        Copy: Record "E27 Row";
    begin
        if Src.FindSet() then
            repeat
                Copy := Src;
                if Copy.Code = '' then begin
                    Copy := Src;
                    Copy.Modify();
                end;
            until Src.Next() = 0;
    end;
}
codeunit 52803 "E27 Temp Local"
{
    procedure Fill()
    var
        Src: Record "E27 Row";
        Buf: Record "E27 Row" temporary;
    begin
        if Src.FindSet() then
            repeat
                Buf := Src;
                Buf.Modify();
            until Src.Next() = 0;
    end;
}
codeunit 52805 "E27 Overwrite"
{
    procedure Fill()
    var
        Src: Record "E27 Row";
        TempBuf: Record "E27 Row" temporary;
        Copy: Record "E27 Row";
    begin
        if Src.FindSet() then
            repeat
                Copy := Src;
                Copy := TempBuf;
                Copy.Modify();
            until Src.Next() = 0;
    end;
}
codeunit 52804 "E27 Arms"
{
#if not CLEAN99
    local procedure SameClaim(X: Integer) Result: Integer
    begin
        Result := X;
    end;
#else
    local procedure SameClaim(X: Integer): Integer
    begin
        exit(X);
    end;
#endif

#if not CLEAN99
    local procedure TwoClaims(X: Integer): Integer
    begin
        exit(X);
    end;
#else
    internal procedure TwoClaims(X: Integer): Integer
    begin
        exit(X + 1);
    end;
#endif
}
"#;

fn findings(source: &str) -> Vec<serde_json::Value> {
    run(source, "json")["payload"]["findings"]
        .as_array()
        .unwrap()
        .clone()
}

/// The analyzer's output in `format` for a one-file workspace of `source`.
fn run(source: &str, format: &str) -> serde_json::Value {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e27e27e0-0000-4000-8000-0000000000aa","name":"E27","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "analyze",
            t.path().to_str().unwrap(),
            "--format",
            format,
            "--deterministic",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn of<'a>(fs: &'a [serde_json::Value], detector: &str, object: &str) -> Vec<&'a serde_json::Value> {
    fs.iter()
        .filter(|f| f["detector"] == detector && f["primaryLocation"]["objectName"] == object)
        .collect()
}

/// d56: a copy into a temporary buffer is no round-trip (the temp page's
/// `Rec`, a `temporary` local); on a real record two copies before one write
/// are ONE finding, naming the copy nearest the write.
/// An earlier copy that a later one overwrites before the write is not what
/// the write writes: `Copy := Src; Copy := TempBuf; Copy.Modify()` writes the
/// temporary copy, so nothing is reported.
/// Breaks (2026-11-13): drop the target-temp skip in `detect_d56` -> "E27
/// Temp Page" reports; drop the overwritten-copy skip -> "E27 Overwrite"
/// reports (and "E27 Real Page" would report 2: round 1's per-write map and
/// first-copy breaks failed there, and that map is now unreachable, so it is
/// gone). Each fails; restored, it passes.
#[test]
fn d56_skips_a_temporary_target_and_reports_one_finding_per_write() {
    let fs = findings(SOURCE);
    assert!(of(&fs, "d56-clone-before-write-in-loop", "E27 Temp Page").is_empty());
    assert!(of(&fs, "d56-clone-before-write-in-loop", "E27 Temp Local").is_empty());
    assert!(of(&fs, "d56-clone-before-write-in-loop", "E27 Overwrite").is_empty());
    let real = of(&fs, "d56-clone-before-write-in-loop", "E27 Real Page");
    assert_eq!(real.len(), 1, "{real:#?}");
    // The evidence's first step is the copy; only SARIF prints it.
    let sarif = run(SOURCE, "sarif");
    let d56: Vec<&serde_json::Value> = sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["ruleId"].as_str().is_some_and(|id| id.contains("d56")))
        .collect();
    assert_eq!(d56.len(), 1, "{d56:#?}");
    let copy_line = &d56[0]["codeFlows"][0]["threadFlows"][0]["locations"][0]["location"]["physicalLocation"]
        ["region"]["startLine"];
    // `Copy := Src;` inside the `if`, the copy the Modify writes.
    let nearest = SOURCE
        .lines()
        .position(|l| l.trim() == "Copy := Src;" && l.starts_with("                    "))
        .unwrap()
        + 1;
    assert_eq!(*copy_line, serde_json::json!(nearest), "{:#?}", d56[0]);
}

/// d14: `#if` arms of one procedure share its id. Arms that say the same thing
/// are one finding; arms that say different things (`local` / `internal`) are
/// two, `@arm1` and `@arm2`, keyed by order, never by line: blank lines above
/// change neither id nor fingerprint.
/// Breaks (2026-11-13): skip `one_finding_per_arm_claim` -> `SameClaim`
/// reports 2 with one id; collapse every same-id group -> `TwoClaims` reports
/// 1. Each fails; restored, it passes.
#[test]
fn d14_reports_one_finding_per_distinct_arm_claim() {
    let fs = findings(SOURCE);
    let named = |name: &str| -> Vec<(String, String)> {
        of(&fs, "d14-dead-routine", "E27 Arms")
            .into_iter()
            .filter(|f| f["primaryLocation"]["routineName"] == name)
            .map(|f| {
                (
                    f["id"].as_str().unwrap().to_string(),
                    f["fingerprint"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let same = named("SameClaim");
    assert_eq!(same.len(), 1, "{same:?}");
    let two = named("TwoClaims");
    assert_eq!(two.len(), 2, "{two:?}");
    assert!(
        two[0].0.ends_with("@arm1") && two[1].0.ends_with("@arm2"),
        "{two:?}"
    );
    assert_ne!(two[0].1, two[1].1);

    let moved = SOURCE.replace(
        "codeunit 52804 \"E27 Arms\"",
        "\n\n\ncodeunit 52804 \"E27 Arms\"",
    );
    assert_ne!(moved, SOURCE);
    let fs2 = findings(&moved);
    let mut ids: Vec<(String, String)> = of(&fs2, "d14-dead-routine", "E27 Arms")
        .into_iter()
        .map(|f| {
            (
                f["id"].as_str().unwrap().to_string(),
                f["fingerprint"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut before: Vec<(String, String)> = same.into_iter().chain(two).collect();
    ids.sort();
    before.sort();
    assert_eq!(ids, before);
}
