//! A `Temp Blob` stream is in memory, not file access. `TempBlob.CreateOutStream`
//! was a FILE effect (`fileOp: "write-blob"`, ported from al-sem's
//! `file-blob.ts`), so `explain` printed "FILE at …" for it and `digest` counted
//! it under `kinds.file` (round-2 fact check n10:
//! `CDOeDocumentsDispatcher.Codeunit.al:286`, c25:
//! `CDOUniversigneSealService.Codeunit.al:218`). A write through the AL `File`
//! type is still a FILE effect.
//!
//! Through the shipped binary, so the body's capability extraction, the cone and
//! the digest and explain projections are all on the path.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50320 "Temp Blob"
{
    procedure CreateOutStream(var OutStream: OutStream)
    begin
    end;
}
codeunit 50321 "TB Writer"
{
    procedure ToBlob()
    var
        PdfTempBlob: Codeunit "Temp Blob";
        WriteStream: OutStream;
    begin
        PdfTempBlob.CreateOutStream(WriteStream);
    end;

    procedure ToFile()
    var
        F: File;
    begin
        F.Create('out.txt');
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"32323232-0000-4000-8000-000000000032","name":"TB","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50320,"to":50329}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/Probe.al"), SOURCE).unwrap();
    t
}

fn explain(ws: &Path, routine: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["explain", "--routine", routine])
        .arg(ws)
        .args(["--format", "human"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}");
    text
}

#[test]
fn a_temp_blob_stream_is_not_a_file_effect() {
    let t = workspace();
    let blob = explain(t.path(), "Codeunit \"TB Writer\"::ToBlob");
    assert!(!blob.contains("FILE"), "an in-memory stream: {blob}");
    // The positive control: a `File` write is file access.
    let file = explain(t.path(), "Codeunit \"TB Writer\"::ToFile");
    assert!(file.contains("FILE at ws:src/Probe.al:"), "{file}");
}

#[test]
fn digest_counts_no_file_effect_for_a_temp_blob_stream() {
    let t = workspace();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("digest")
        .arg(t.path())
        .args([
            "--file",
            "src/Probe.al",
            "--format",
            "json",
            "--deterministic",
        ])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let v: serde_json::Value = crate::as_v3(
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr))),
    );
    let text = v.to_string();
    // One FILE effect in the file: `ToFile`'s `File.Create`.
    assert_eq!(
        text.matches("\"type\":\"FILE\"").count(),
        1,
        "only the File write: {text}"
    );
}
