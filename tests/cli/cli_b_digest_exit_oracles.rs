//! cli-b/b1 — corpus-invisible exit-code + stderr oracles for `alsem digest`.
//!
//! The digest differential drives `run_digest_pipeline` directly, so the CLI
//! handler's exit codes and stderr messages (no-input, bad-format, zero-roots)
//! are NOT exercised by it. These oracles invoke the real `alsem` binary and
//! assert the exact byte behavior, mirroring the TS CLI (cli/digest.ts + index.ts):
//!
//!   - no changed input        → exit 1, message starts "digest: at least one of …"
//!   - invalid --format        → exit 1, message "al-sem digest: invalid --format …"
//!   - valid input, zero roots → exit 2, a VALID digest JSON document on stdout

use std::path::PathBuf;
use std::process::Command;

fn alsem_bin() -> &'static str {
    env!("CARGO_BIN_EXE_alsem")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(name)
}

#[test]
fn no_input_exits_1_with_cli_message() {
    let ws = fixture("ws-d8-commit-in-tx");
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());
    let out = Command::new(alsem_bin())
        .arg("digest")
        .arg(&ws)
        .output()
        .expect("run alsem digest");
    let code = out.status.code().expect("exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code, 1, "no-input must exit 1; stderr={stderr}");
    assert!(
        stderr.contains(
            "digest: at least one of --changed-files, --changed-routines, --diff, or --changed is required"
        ),
        "exact no-input message expected; got: {stderr:?}"
    );
    // No `al-sem:` prefix on the no-input message (TS writes it verbatim).
    assert!(
        !stderr.trim_start().starts_with("al-sem:"),
        "no-input message must NOT carry an al-sem: prefix: {stderr:?}"
    );
}

#[test]
fn invalid_format_exits_1_with_cli_message() {
    let ws = fixture("ws-d8-commit-in-tx");
    let out = Command::new(alsem_bin())
        .arg("digest")
        .arg(&ws)
        .arg("--file")
        .arg("src/Foo.al")
        .arg("--format")
        .arg("xml")
        .output()
        .expect("run alsem digest");
    let code = out.status.code().expect("exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(code, 1, "bad --format must exit 1; stderr={stderr}");
    assert!(
        stderr.contains("al-sem digest: invalid --format 'xml'. Expected: json | human"),
        "exact bad-format message expected; got: {stderr:?}"
    );
}

#[test]
fn zero_roots_exits_2_with_valid_document() {
    // A routine selector that matches nothing → zero roots → exit 2, but a VALID
    // digest document is still emitted (entries empty, a selector-unmatched diag).
    let ws = fixture("ws-d8-commit-in-tx");
    let out = Command::new(alsem_bin())
        .arg("digest")
        .arg(&ws)
        .arg("--routine")
        .arg("NoSuchRoutineNameXYZ")
        .arg("--deterministic")
        .output()
        .expect("run alsem digest");
    let code = out.status.code().expect("exit code");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(code, 2, "zero-roots must exit 2");
    // The stdout must be a valid digest envelope JSON.
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("zero-roots stdout must be valid JSON");
    assert_eq!(v["kind"], "digest", "envelope kind must be 'digest'");
    assert_eq!(
        v["payload"]["entries"].as_array().map(|a| a.len()),
        Some(0),
        "zero-roots → no entries"
    );
    // rootsRequested counts query diagnostics too (#5), and since round-2 n08
    // each routine selector that resolved to none or to several: the request
    // named one routine, so rootsRequested == 1 (it was 0) and rootsResolved
    // == 0.
    assert_eq!(v["payload"]["summary"]["rootsRequested"].as_u64(), Some(1));
    assert_eq!(v["payload"]["summary"]["rootsResolved"].as_u64(), Some(0));
    // A changed-roots selector-unmatched diagnostic is surfaced.
    let crd = v["payload"]["changedRootsDiagnostics"]
        .as_array()
        .expect("changedRootsDiagnostics array");
    assert!(
        crd.iter().any(|d| d["kind"] == "selector-unmatched"),
        "expected a selector-unmatched changed-roots diagnostic: {crd:?}"
    );
}
