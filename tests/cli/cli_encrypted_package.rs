//! G3: an encrypted `.app` in the workspace's `.alpackages` is named, not just
//! failed on. Every package of Continia's public 29.0 release is one (2026-10):
//! the usual 40-byte NAVX header, then `.NEA` and an encrypted payload where a
//! zip would start. Before, the loader logged "Failed to open .app as ZIP
//! archive" and `analyze` listed nothing.
//!
//! Each test drives the REAL analyze pipeline (`run_analyze_with_exit`, JSON) over a
//! temp copy of `tests/fixtures/analyze-json`; the package bytes are written at test
//! time with the layout of the real packages (hand-built here, so the precondition
//! does not depend on al-sem's own test builder).

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

const PACKAGE: &str = "Core 29.0.0.221304 - 29.0 (BC 2026 Wave 2).app";

/// An encrypted package: NAVX, header size 40, version 2, a GUID, the payload
/// length, closing NAVX; then `.NEA` and bytes holding no zip signature.
fn encrypted_app() -> Vec<u8> {
    let payload: Vec<u8> = b".NEA\0\0\0\x01"
        .iter()
        .copied()
        .chain((0..2048u32).map(|i| (i * 37 % 251) as u8))
        .collect();
    let mut out = b"NAVX".to_vec();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[0x47; 16]);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(b"NAVX");
    out.extend_from_slice(&payload);
    out
}

fn workspace(with_package: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    crate::copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/analyze-json"),
        tmp.path(),
    );
    if with_package {
        let dir = tmp.path().join(".alpackages");
        std::fs::create_dir_all(&dir).expect("mkdir .alpackages");
        std::fs::write(dir.join(PACKAGE), encrypted_app()).expect("write package");
    }
    tmp
}

fn analyze(ws: &Path) -> serde_json::Value {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        single_app: false,
        with_evidence: false,
    };
    let (out, _, _, _) =
        run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "g3-test")
            .unwrap_or_else(|e| panic!("run_analyze failed: {e}"));
    serde_json::from_str(&out).expect("analyze JSON must parse")
}

fn diagnostics(json: &serde_json::Value) -> Vec<String> {
    json["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| d["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn an_encrypted_package_is_listed_and_named_encrypted() {
    let ws = workspace(true);
    let json = analyze(ws.path());
    assert_eq!(
        json["payload"]["summary"]["encryptedPackages"],
        serde_json::json!([format!(".alpackages/{PACKAGE}")]),
    );
    let diags = diagnostics(&json);
    assert!(
        diags.iter().any(|m| m.contains(PACKAGE)
            && m.contains(
                "encrypted .app package: its manifest, symbols and source cannot be read"
            )),
        "a dependency diagnostic must name the package as encrypted: {diags:#?}"
    );
}

/// The key is always there, empty when no package is encrypted.
#[test]
fn no_encrypted_package_gives_an_empty_list() {
    let ws = workspace(false);
    let json = analyze(ws.path());
    assert_eq!(
        json["payload"]["summary"]["encryptedPackages"],
        serde_json::json!([])
    );
}
