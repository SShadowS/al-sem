//! `alsem` and `aldump` print `warn!` lines on stderr (FW3).
//!
//! Before, neither binary installed a logger, so a dependency the loader
//! dropped was reported only by a `warn!` that went nowhere (the FW2 temp-disk
//! bug). The precondition is stated by the input: a `.alpackages` file that is
//! not a zip, which the loader warns about and skips. `RUST_LOG` still sets the
//! level, and stdout never carries log lines.

use std::path::Path;
use std::process::Command;

fn copy_fixture(to: &Path) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-e2e");
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

/// Run `bin args` with `RUST_LOG` set to `level` (`None` = unset); returns
/// (stdout, stderr).
fn run(bin: &str, args: &[&Path], level: Option<&str>) -> (String, String) {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    match level {
        Some(l) => cmd.env("RUST_LOG", l),
        None => cmd.env_remove("RUST_LOG"),
    };
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn {bin}: {e}"));
    assert!(out.status.success(), "{bin} failed: {out:?}");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn dropped_dependency_warning_reaches_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    copy_fixture(&ws);
    std::fs::create_dir_all(ws.join(".alpackages")).unwrap();
    std::fs::write(ws.join(".alpackages/Junk_Junk_1.0.0.0.app"), b"not a zip").unwrap();

    let cases: [(&str, Vec<&Path>); 2] = [
        (
            env!("CARGO_BIN_EXE_alsem"),
            vec![
                Path::new("analyze"),
                &ws,
                Path::new("--format"),
                Path::new("json"),
                Path::new("--deterministic"),
            ],
        ),
        (
            env!("CARGO_BIN_EXE_aldump"),
            vec![Path::new("--program-call-graph-stats"), &ws],
        ),
    ];
    for (bin, args) in &cases {
        let (out, err) = run(bin, args, None);
        assert!(
            err.contains("WARN") && err.contains("Junk_Junk_1.0.0.0.app"),
            "{bin}: the skipped .app must be reported on stderr, got: {err:?}"
        );
        assert!(!out.contains("WARN"), "{bin}: log lines on stdout");
        // RUST_LOG overrides the default level; stdout does not change.
        let (quiet_out, quiet_err) = run(bin, args, Some("error"));
        assert_eq!(quiet_err, "", "{bin}: RUST_LOG=error must silence warnings");
        assert_eq!(quiet_out, out, "{bin}: the log level changed stdout");
    }
}
