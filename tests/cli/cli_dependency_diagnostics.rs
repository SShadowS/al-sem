//! #91: every command that reports the analyzer's diagnostics reports the
//! program build's dependency-ledger warnings too, as `alsem analyze` does.
//!
//! Each case drives the real `alsem` binary (`RUST_LOG=off`, so no logger line
//! reaches stderr) over two tempdir fixtures:
//!   - (a) `tests/r0-corpus/ws-codeunit-rec`, with its `CodeunitRecLibA`
//!     dependency declared at `2.0.0.0` while `.alpackages` holds `1.0.0.0`
//!     (the issue's reproducer: "is present only at version 1.0.0.0");
//!   - (b) `tests/r0-corpus/ws-preflight-missingdep`, whose one dependency has
//!     no package ("is missing").
//!
//! Every row first asserts the EXACT expected message is there (a shared break
//! that drops it from analyze too would still pass an equality check), then that
//! the command's dependency messages equal analyze's in the SAME mode:
//! `fingerprint` and `prove` are single-app (vs `analyze --single-app`); `digest`,
//! `events` and `policy` default to cross-app (vs plain `analyze`).

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

struct Fixture {
    corpus_dir: &'static str,
    /// The stable id of a routine in the fixture, for `prove`.
    prove_routine: &'static str,
    /// A workspace-relative `.al` path, for `digest --file`.
    digest_file: &'static str,
    /// The dependency message the fixture's ledger must produce.
    message: &'static str,
}

const OLDER: Fixture = Fixture {
    corpus_dir: "ws-codeunit-rec",
    prove_routine: "dddd4444-0000-0000-0000-000000000004:Table:50970#72bb1a2604161dd744550476e8f33571d9f0d4fae870e5db1f9dc92ff9e2ce71",
    digest_file: "src/Item.Table.al",
    message: "dependency \"CodeunitRecLibA\" (eeee5555-0000-0000-0000-000000000005), declared by \"CodeunitRec Probe\" at version 2.0.0.0, is present only at version 1.0.0.0",
};

const MISSING: Fixture = Fixture {
    corpus_dir: "ws-preflight-missingdep",
    prove_routine: "pppppppp-0000-0000-0000-000000000001:Codeunit:63200#a15fbfb887bf1826e3f04ea76d6346e88cbd57b0a934f4d9a6a5b5385f360d7e",
    digest_file: "src/caller.al",
    message: "dependency \"PFMissingDep\" (pppppppp-0000-0000-0000-000000000099), declared by \"Preflight Missing Dep Primary\" at version 1.0.0.0, is missing",
};

const FIXTURES: [&Fixture; 2] = [&OLDER, &MISSING];

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

/// The fixture copied into a tempdir. For (a) the `CodeunitRecLibA` dependency
/// is set to exactly `2.0.0.0`.
fn workspace(f: &Fixture) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join(f.corpus_dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("r0-corpus")
            .join(f.corpus_dir),
        &ws,
    );
    if f.corpus_dir == OLDER.corpus_dir {
        let path = ws.join("app.json");
        let mut app: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read app.json"))
                .expect("parse app.json");
        let dep = app["dependencies"]
            .as_array_mut()
            .expect("dependencies")
            .iter_mut()
            .find(|d| d["name"] == "CodeunitRecLibA")
            .expect("CodeunitRecLibA dependency");
        dep["version"] = Value::from("2.0.0.0");
        std::fs::write(&path, app.to_string()).expect("write app.json");
    }
    (tmp, ws)
}

fn alsem(ws: &Path, args: &[&str]) -> Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"));
    // A subcommand group (`events fanout`, `policy check`) is two words.
    let (head, tail) = args.split_at(match args[0] {
        "events" | "policy" => 2,
        _ => 1,
    });
    cmd.args(head)
        .arg(ws)
        .args(tail)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem")
}

/// The `DIAG-dependencies` messages of a JSON envelope's top-level `diagnostics`.
fn json_dependency_messages(out: &Output, what: &str) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{what}: no JSON envelope ({e}); stderr {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    doc["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("{what}: no top-level diagnostics array"))
        .iter()
        .filter(|d| d["code"] == "DIAG-dependencies")
        .map(|d| d["message"].as_str().expect("message").to_string())
        .collect()
}

/// The dependency messages of the `warning: <message>` lines on stderr.
fn stderr_dependency_messages(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter_map(|l| l.trim_end_matches('\r').strip_prefix("warning: "))
        .filter(|m| m.starts_with("dependency "))
        .map(str::to_string)
        .collect()
}

fn analyze_messages(ws: &Path, single_app: bool) -> Vec<String> {
    let mut args = vec!["analyze", "--format", "json"];
    if single_app {
        args.push("--single-app");
    }
    json_dependency_messages(&alsem(ws, &args), "analyze")
}

/// The exact-message check, then equality with analyze in the same mode.
fn assert_row(f: &Fixture, ws: &Path, what: &str, got: &[String], single_app: bool) {
    assert!(
        got.iter().any(|m| m == f.message),
        "{what} on {}: missing the dependency warning {:?}; got {got:?}",
        f.corpus_dir,
        f.message
    );
    assert_eq!(
        got,
        analyze_messages(ws, single_app).as_slice(),
        "{what} on {}: dependency messages differ from analyze's (single_app={single_app})",
        f.corpus_dir
    );
}

#[test]
fn analyze_reports_the_dependency_warning_in_both_modes() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        for single_app in [true, false] {
            let got = analyze_messages(&ws, single_app);
            assert!(
                got.iter().any(|m| m == f.message),
                "analyze (single_app={single_app}) on {}: got {got:?}",
                f.corpus_dir
            );
        }
    }
}

#[test]
fn fingerprint_json_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let out = alsem(&ws, &["fingerprint", "--format", "json"]);
        let got = json_dependency_messages(&out, "fingerprint");
        assert_row(f, &ws, "fingerprint", &got, true);
    }
}

#[test]
fn prove_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let out = alsem(&ws, &["prove", f.prove_routine, "may-commit"]);
        let got = json_dependency_messages(&out, "prove");
        assert_row(f, &ws, "prove", &got, true);
    }
}

#[test]
fn digest_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let out = alsem(
            &ws,
            &["digest", "--file", f.digest_file, "--format", "json"],
        );
        let got = json_dependency_messages(&out, "digest");
        assert_row(f, &ws, "digest", &got, false);
    }
}

#[test]
fn events_fanout_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let got = stderr_dependency_messages(&alsem(&ws, &["events", "fanout"]));
        assert_row(f, &ws, "events fanout", &got, false);
    }
}

#[test]
fn events_chains_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let got = stderr_dependency_messages(&alsem(&ws, &["events", "chains"]));
        assert_row(f, &ws, "events chains", &got, false);
    }
}

#[test]
fn policy_check_reports_the_dependency_warning() {
    for f in FIXTURES {
        let (_tmp, ws) = workspace(f);
        let got = stderr_dependency_messages(&alsem(&ws, &["policy", "check"]));
        assert_row(f, &ws, "policy check", &got, false);
    }
}
