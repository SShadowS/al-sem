//! T23 (#277 spec D2.4, S17): `--update-baseline`'s write left compute and is
//! now the bin's first write, under the one write-failure rule.
//!
//! The precondition is Task 1's base-binary record (`update-baseline/`, read
//! through the guarded reader, never regenerated): at `1134bed1` a failed write
//! printed `al-sem: failed to write baseline '<path>': <OS message>` with exit 3
//! and an empty stdout. The head prints `al-sem: write error: <path>: <OS
//! message>` with exit 2, stdout still empty.
//!
//! Normalization: the record was minted on Linux, so its OS message is Linux's.
//! The compare replaces the OS message with `<OS message>` on both sides (the
//! record's from its own text, the head's from the OS's error for the same
//! write), so the test runs on any platform. The record holds no version field.

use std::path::Path;
use std::process::{Command, Output};

use crate::batch_records::{batch_goldens, read_record};

const BASELINE: &str = "tests/gate-goldens/batch/update-baseline/no-such-dir/b.json";

fn analyze(baseline: &str, update: bool) -> Output {
    let mut args = vec![
        "analyze",
        "tests/r0-corpus/ws-d1",
        "--deterministic",
        "--baseline",
        baseline,
    ];
    if update {
        args.push("--update-baseline");
    }
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem")
}

/// `text` with the OS message after `head` replaced by `<OS message>`.
fn normalize(text: &str, head: &str) -> String {
    let rest = text
        .strip_prefix(head)
        .unwrap_or_else(|| panic!("{text:?} does not start with {head:?}"));
    assert!(
        rest.ends_with('\n') && rest.len() > 1,
        "no OS message: {text:?}"
    );
    format!("{head}<OS message>\n")
}

#[test]
fn update_baseline_write_failure() {
    let dir = batch_goldens().join("update-baseline");
    // Precondition: the base behaviour, from the record.
    let base_err = String::from_utf8(read_record(&dir.join("base.stderr.golden"))).unwrap();
    assert_eq!(
        normalize(
            &base_err,
            &format!("al-sem: failed to write baseline '{BASELINE}': ")
        ),
        format!("al-sem: failed to write baseline '{BASELINE}': <OS message>\n")
    );
    assert!(read_record(&dir.join("base.stdout.golden")).is_empty());
    assert_eq!(read_record(&dir.join("base.exit")), b"3\n");

    // The head.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !root.join(BASELINE).parent().unwrap().exists(),
        "precondition: the baseline's folder does not exist"
    );
    let os = std::fs::write(root.join(BASELINE), b"").unwrap_err();
    let out = analyze(BASELINE, true);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err, format!("al-sem: write error: {BASELINE}: {os}\n"));
    assert_eq!(
        normalize(&err, &format!("al-sem: write error: {BASELINE}: ")),
        format!("al-sem: write error: {BASELINE}: <OS message>\n")
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout unwritten");
}

/// The success case: the baseline file is written, and stdout equals a plain
/// `--baseline` run over the file just written (the set applied is the set
/// written, as before the move).
#[test]
fn update_baseline_write_success() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("b.json");
    let path = path.to_str().unwrap();
    let updated = analyze(path, true);
    assert_eq!(
        updated.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    let written = std::fs::read_to_string(path).expect("baseline written");
    let v: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert!(
        !v["fingerprints"].as_array().unwrap().is_empty(),
        "precondition: ws-d1 has findings"
    );
    let plain = analyze(path, false);
    assert_eq!(plain.status.code(), Some(0));
    assert_eq!(updated.stdout, plain.stdout);
    assert_eq!(std::fs::read_to_string(path).unwrap(), written);
}
