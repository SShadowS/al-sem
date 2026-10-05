//! FW1 (final review C-1): the program engine and L2/L3 must see the SAME
//! workspace source, in the same files with the same text.
//!
//! Text: a workspace `.al` file that is not valid UTF-8 (a Windows-1252 byte,
//! common in code that came from NAV through txt2al) used to fail the program
//! build while L3 assembled, so analyze exited with an error and no output.
//! Both now read through one lossy decoder (`al_sem::source_text`).
//!
//! Files: both now pick files with one walk (`source_text::discover_al_files`):
//! a directory named `X.al` is walked, the `.al` extension is matched without
//! case, `.alpackages` / `.snapshots` / `node_modules` are skipped at any case,
//! and symbolic links are followed. Before, the program engine and L3 differed
//! on each of these. A file only L3 saw kept L3's edges for its own call sites,
//! but a call INTO it from a file both saw took the program engine's answer,
//! which had no target: the callee looked dead (the extension-case test shows
//! that silent finding change).
//!
//! Each test asserts on d14 (dead routine) findings. That both engines see the
//! same TEXT (so the adapter's byte-span join stays exact past an invalid
//! byte) is pinned by `program_calls::tests::non_utf8_byte_before_call_pairs`;
//! these tests do not detect a join drift (proved: a program-side-only decode
//! change left them green).

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

const APP_JSON: &str = r#"{"id":"f1f1f1f1-0000-0000-0000-000000000001","name":"FW1","publisher":"T","version":"1.0.0.0","dependencies":[]}"#;

const NORMAL: &str =
    "codeunit 50100 \"Fw1 Normal\"\n{\n    procedure P()\n    begin\n    end;\n}\n";

/// 0xE6 is "æ" in Windows-1252 and invalid on its own in UTF-8. It sits on
/// the same line as, and before, the `LiveHelper()` call.
const WIN1252: &[u8] = b"codeunit 50101 \"Fw1 Latin\"\n{\n    trigger OnRun()\n    begin\n        /* K\xE6re */ LiveHelper();\n    end;\n\n    local procedure LiveHelper() begin end;\n\n    local procedure DeadHelper() begin end;\n}\n";

/// Calls `LiveInternal` on [`CALLEE`] from another file.
const CALLER: &str = "codeunit 50102 \"Fw1 Caller\"\n{\n    trigger OnRun()\n    var\n        C: Codeunit \"Fw1 Callee\";\n    begin\n        C.LiveInternal();\n    end;\n}\n";

/// `LiveInternal` is called from [`CALLER`]; `DeadInternal` from nowhere.
const CALLEE: &str = "codeunit 50103 \"Fw1 Callee\"\n{\n    internal procedure LiveInternal() begin end;\n\n    internal procedure DeadInternal() begin end;\n}\n";

/// A temp workspace with a root `app.json` and `files` (relative path, bytes).
fn workspace(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.json"), APP_JSON).unwrap();
    for (rel, bytes) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    dir
}

/// Run analyze (d14 only, JSON) and return the routine names of the d14
/// findings located in `file` (a `ws:`-prefixed unit id).
fn d14_routines_in(ws: &Path, file: &str) -> Vec<String> {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some("d14-dead-routine".to_string()),
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
        with_evidence: false,
    };
    let (out, _, _) = run_analyze_with_exit(&args, "test").expect("analyze must not fail");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json output");
    let findings = v["payload"]["findings"].as_array().expect("findings array");
    findings
        .iter()
        .filter(|f| f["primaryLocation"]["file"].as_str() == Some(file))
        .filter_map(|f| f["primaryLocation"]["routineName"].as_str())
        .map(str::to_string)
        .collect()
}

/// The callee file is analyzed with the caller's call resolved into it:
/// exactly `DeadInternal` is dead.
fn assert_callee_judged(ws: &Path, callee_file: &str) {
    let got = d14_routines_in(ws, callee_file);
    assert_eq!(got, ["DeadInternal"], "d14 in {callee_file}");
}

#[test]
fn analyze_reads_a_non_utf8_file() {
    // `black_box` keeps rustc's `invalid_from_utf8` lint from rejecting a
    // check it can already see is always true.
    assert!(
        std::str::from_utf8(std::hint::black_box(WIN1252)).is_err(),
        "precondition: the fixture is not UTF-8"
    );
    let ws = workspace(&[
        ("src/A.Codeunit.al", NORMAL.as_bytes()),
        ("src/B.Codeunit.al", WIN1252),
    ]);
    let got = d14_routines_in(ws.path(), "ws:src/B.Codeunit.al");
    assert!(
        got.iter().any(|r| r == "DeadHelper"),
        "no d14 finding for DeadHelper in the non-UTF-8 file: {got:?}"
    );
}

#[test]
fn analyze_walks_a_directory_named_dot_al() {
    let rel = "src/Odd.al/C.Codeunit.al";
    assert!(
        Path::new(rel).parent().unwrap().extension() == Some("al".as_ref()),
        "precondition: the parent directory's name ends in .al"
    );
    let ws = workspace(&[("src/A.Codeunit.al", NORMAL.as_bytes()), (rel, WIN1252)]);
    let got = d14_routines_in(ws.path(), &format!("ws:{rel}"));
    assert!(
        got.iter().any(|r| r == "DeadHelper"),
        "no d14 finding for DeadHelper under the `.al` directory: {got:?}"
    );
}

/// `Callee.Codeunit.AL`: L2 matched the extension without case, the program
/// engine did not, so the call into it had no program target.
#[test]
fn analyze_reads_an_upper_case_extension() {
    let ws = workspace(&[
        ("src/Caller.Codeunit.al", CALLER.as_bytes()),
        ("src/Callee.Codeunit.AL", CALLEE.as_bytes()),
    ]);
    assert_callee_judged(ws.path(), "ws:src/Callee.Codeunit.AL");
}

/// `.snapshots` (the program engine skipped it, L2 did not) and
/// `Node_Modules` (both skipped only the exact lower-case name) hold no
/// workspace source.
#[test]
fn analyze_skips_dependency_and_output_dirs_at_any_case() {
    let dead = "codeunit 50104 \"Fw1 Skipped\"\n{\n    local procedure Dead() begin end;\n}\n";
    let ws = workspace(&[
        ("src/Caller.Codeunit.al", CALLER.as_bytes()),
        ("src/Callee.Codeunit.al", CALLEE.as_bytes()),
        (".snapshots/S.Codeunit.al", dead.as_bytes()),
        ("Node_Modules/N.Codeunit.al", dead.as_bytes()),
    ]);
    assert_callee_judged(ws.path(), "ws:src/Callee.Codeunit.al");
    for f in [".snapshots/S.Codeunit.al", "Node_Modules/N.Codeunit.al"] {
        let got = d14_routines_in(ws.path(), &format!("ws:{f}"));
        assert!(got.is_empty(), "{f} was analyzed: {got:?}");
    }
}

/// A file whose callee lives outside the workspace, reached through a link
/// at `src/<name>`; the caller is in `src/Caller.Codeunit.al`.
fn linked_workspace(outside: &Path) -> tempfile::TempDir {
    std::fs::create_dir(outside.join("dir")).unwrap();
    std::fs::write(outside.join("Callee.Codeunit.al"), CALLEE).unwrap();
    std::fs::write(outside.join("dir/Callee.Codeunit.al"), CALLEE).unwrap();
    workspace(&[("src/Caller.Codeunit.al", CALLER.as_bytes())])
}

/// A symbolic link to a FILE (the program engine read it, L2 did not). Needs
/// the right to create symbolic links (Windows: Developer Mode or admin); it
/// returns early, and says so, without it.
#[test]
fn analyze_follows_a_file_link() {
    let outside = tempfile::tempdir().unwrap();
    let ws = linked_workspace(outside.path());
    let (target, at) = (
        outside.path().join("Callee.Codeunit.al"),
        ws.path().join("src/Callee.Codeunit.al"),
    );
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&target, &at);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target, &at);
    if let Err(e) = made {
        eprintln!("SKIPPED analyze_follows_a_file_link: cannot create a file link: {e}");
        return;
    }
    assert_callee_judged(ws.path(), "ws:src/Callee.Codeunit.al");
}

/// A link to a DIRECTORY (neither engine walked it). On Windows this is a
/// junction (`mklink /J`), which needs no special right.
#[test]
fn analyze_follows_a_directory_link() {
    let outside = tempfile::tempdir().unwrap();
    let ws = linked_workspace(outside.path());
    // `cmd` wants backslashes: join one component at a time.
    let (target, at) = (
        outside.path().join("dir"),
        ws.path().join("src").join("linked"),
    );
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&at)
            .arg(&target)
            .output()
            .expect("run mklink");
        assert!(out.status.success(), "mklink /J failed: {out:?}");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &at).unwrap();
    assert_callee_judged(ws.path(), "ws:src/linked/Callee.Codeunit.al");
}
