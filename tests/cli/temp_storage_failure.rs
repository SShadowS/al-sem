//! A failed temp-file write must never change results (FW2, review finding I-3).
//!
//! The engine decompresses a dependency's `SymbolReference.json`, and the nested
//! app of a Ready-to-Run package, into anonymous temp files. With the temp
//! directory's disk full, those writes failed, the dependency loader took the
//! failure for an unreadable `.app`, and the dependency silently vanished: same
//! binary, same input, exit 0, different call graph and different `analyze`
//! output (seen on CDO, 2026-10-04: 504 unknown edges instead of 0).
//!
//! The precondition is stated by the environment, not produced by the engine:
//! the child's temp directory is an existing regular FILE, so no temp file can
//! be created there on any OS. Each case runs the real binaries twice, healthy
//! and with temp storage broken, and requires byte-identical stdout.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXTURE: &str = "tests/r0-corpus/ws-baseapp-closure";
const DEP_APP: &str = "Microsoft_Base Application_24.0.0.0.app";

fn copy_fixture(to: &Path) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    for entry in walkdir::WalkDir::new(&from) {
        let entry = entry.unwrap();
        let dest = to.join(entry.path().strip_prefix(&from).unwrap());
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// Wrap `app` the way Microsoft ships a Ready-to-Run package: a NAVX header,
/// then a zip holding `readytorunappmanifest.json` and the real app as an entry.
fn wrap_ready_to_run(app: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let inner = "nested_24.0.0.0.app";
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("readytorunappmanifest.json", opts).unwrap();
    write!(zip, r#"{{"EmbeddedAppFileName":"{inner}"}}"#).unwrap();
    zip.start_file(inner, opts).unwrap();
    zip.write_all(app).unwrap();
    let mut out = b"NAVX".to_vec();
    out.resize(40, 0);
    out.extend_from_slice(zip.finish().unwrap().get_ref());
    out
}

/// Run `bin args`, with temp storage broken when `broken_temp` names a file.
fn run(bin: &str, args: &[&Path], broken_temp: Option<&Path>) -> Output {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    if let Some(file) = broken_temp {
        // Windows reads TMP then TEMP; Unix reads TMPDIR.
        for var in ["TMP", "TEMP", "TMPDIR"] {
            cmd.env(var, file);
        }
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn {bin}: {e}"));
    assert!(
        out.status.success(),
        "{bin} failed (broken temp: {}): {}",
        broken_temp.is_some(),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Healthy run vs broken-temp run of `bin`; returns the healthy stdout.
fn assert_same_with_broken_temp(bin: &str, args: &[&Path], broken: &Path) -> String {
    let healthy = run(bin, args, None);
    let degraded = run(bin, args, Some(broken));
    let healthy = String::from_utf8(healthy.stdout).unwrap();
    assert_eq!(
        healthy,
        String::from_utf8(degraded.stdout).unwrap(),
        "{bin}: a failed temp-file write changed the output"
    );
    healthy
}

fn check(ready_to_run: bool) {
    let dir = tempfile::tempdir().unwrap();
    let ws: PathBuf = dir.path().join("ws");
    copy_fixture(&ws);
    if ready_to_run {
        let app = ws.join(".alpackages").join(DEP_APP);
        let bytes = std::fs::read(&app).unwrap();
        std::fs::write(&app, wrap_ready_to_run(&bytes)).unwrap();
    }
    let broken = dir.path().join("temp-is-a-file");
    std::fs::write(&broken, b"").unwrap();

    let analyze = assert_same_with_broken_temp(
        env!("CARGO_BIN_EXE_alsem"),
        &[
            Path::new("analyze"),
            &ws,
            Path::new("--format"),
            Path::new("json"),
            Path::new("--deterministic"),
        ],
        &broken,
    );
    // Precondition: the dependency is really loaded in the healthy run, so the
    // comparison above would see it vanish.
    let json: serde_json::Value = serde_json::from_str(&analyze).unwrap();
    assert_eq!(
        json["payload"]["summary"]["opaqueApps"],
        serde_json::json!(["Base Application"]),
        "precondition: the healthy run loads the dependency"
    );

    let stats = assert_same_with_broken_temp(
        env!("CARGO_BIN_EXE_aldump"),
        &[Path::new("--program-call-graph-stats"), &ws],
        &broken,
    );
    let json: serde_json::Value = serde_json::from_str(&stats).unwrap();
    assert_eq!(
        json["primaryScoped"]["unknown"], 0,
        "precondition: the healthy call graph resolves every edge"
    );
}

#[test]
fn failed_temp_write_does_not_change_results() {
    check(false);
}

#[test]
fn failed_temp_write_does_not_change_results_ready_to_run() {
    check(true);
}
