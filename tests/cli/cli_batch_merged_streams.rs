//! T20 (#277 spec D2.3): a standalone command writes in today's order when
//! stdout and stderr share one pipe (`2>&1`). Standalone is a one-job batch
//! (D1), so the driver's `JobOutput` replay must keep each command's order: a
//! degraded analyze prints its warning BEFORE the report; `policy check
//! --out` writes the file, THEN its analyzer diagnostics; a failed `--out`
//! stops the diagnostics that would follow it.
//!
//! The expected bytes are Task 1's base-binary records (`merged-streams/`),
//! read through the guarded reader and never written, `REGEN_TEMP_GOLDENS`
//! or not. The one normalization: the report's `"alsemVersion"` value, so a
//! version bump does not break a record.

use std::fs::File;
use std::process::{Command, Stdio};

use crate::batch_records::{batch_goldens, read_record};

/// Run `alsem args` from the repository root with stdout and stderr on ONE
/// file description (a dup'd file shares its offset, as `2>&1` does), and
/// return the merged bytes and the exit code.
fn merged(args: &[&str]) -> (Vec<u8>, i32) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("merged");
    let file = File::create(&path).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "off")
        .env_remove("ALSEM_TRACE")
        .env_remove("ALSEM_RETAINED_CENSUS")
        .stdin(Stdio::null())
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .status()
        .expect("spawn alsem");
    (
        std::fs::read(&path).unwrap(),
        status.code().expect("exit code"),
    )
}

/// `text` with every `"alsemVersion": "<v>"` value replaced by `<version>`.
fn normalize(text: &[u8]) -> String {
    const KEY: &str = "\"alsemVersion\": \"";
    let text = String::from_utf8(text.to_vec()).unwrap();
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find(KEY) {
        let after = &rest[i + KEY.len()..];
        let end = after.find('"').expect("closing quote");
        out.push_str(&rest[..i + KEY.len()]);
        out.push_str("<version>");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The record `name`'s merged bytes and exit code.
fn record(name: &str) -> (String, i32) {
    let dir = batch_goldens().join("merged-streams");
    let bytes = read_record(&dir.join(format!("{name}.merged.golden")));
    let exit = String::from_utf8(read_record(&dir.join(format!("{name}.exit")))).unwrap();
    (normalize(&bytes), exit.trim().parse().unwrap())
}

fn assert_record(name: &str, args: &[&str], stderr_part: &str) {
    let (want, want_exit) = record(name);
    assert!(
        want.contains(stderr_part),
        "precondition: the {name} record holds its stderr part {stderr_part:?}"
    );
    let (got, exit) = merged(args);
    assert_eq!(normalize(&got), want, "{name}: merged bytes");
    assert_eq!(exit, want_exit, "{name}: exit");
}

#[test]
fn write_order_under_merged_streams() {
    // Degraded analyze: the warning line first, then the report.
    assert_record(
        "degraded-analyze",
        &[
            "analyze",
            "tests/r0-corpus/ws-txn-d47-pos-file",
            "--deterministic",
        ],
        "al-sem: warning: analysis coverage degraded",
    );

    // Policy with `--out`: the file, then the diagnostic lines.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("p.json");
    let policy = |out: &str| {
        vec![
            "policy".to_string(),
            "check".to_string(),
            "tests/fixtures/cli-c-policy/ws-policy-commit-in-subscriber".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--deterministic".to_string(),
            "--out".to_string(),
            out.to_string(),
        ]
    };
    let args = policy(out.to_str().unwrap());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    assert_record("policy-diagnostics-out", &args, "warning: ");
    assert!(out.is_file(), "the --out file is written");

    // Policy whose `--out` fails: the write error, and no diagnostic after it.
    let args = policy("tests/gate-goldens/batch/merged-streams/no-such-dir/p.json");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    assert_record("policy-out-fails", &args, "al-sem: write error: ");
}
