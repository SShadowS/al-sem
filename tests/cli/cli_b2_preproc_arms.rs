//! Engine limits B2 (`docs/engine-limits.md` §1): the arms of a statement-position
//! `#if` were run one after another. An `exit` or `Error` in one arm then cut off
//! the code after the group in EVERY configuration, so `prove` answered a wrong
//! `no (every candidate decided)` and a wrong `unconditional on success`. Each case
//! is pinned through the CLI answer that printed it.
//!
//! The workspace states its precondition literally: `app.json` defines `CLEAN99`,
//! and the engine chooses no configuration (a later L22 step), so every arm is a
//! build that may ship. Each routine's source says which arm a build takes.

use std::process::Command;

use serde_json::Value;

const SOURCE: &str = r#"table 50171 "B2 Log B"
{
    fields { field(1; "No."; Integer) { } }
}

codeunit 50170 "B2 Preproc"
{
    // A build without CLEAN99 exits; a build with it runs the write.
    procedure IfExit()
    var
        LogB: Record "B2 Log B";
    begin
#if not CLEAN99
        exit;
#endif
        LogB.Insert();
    end;

    // The error exists only in a build without CLEAN99.
    procedure IfError()
    begin
#if not CLEAN99
        Error('old build only');
#endif
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b2b2b2b2-0000-4000-8000-0000000000b2","name":"B2","publisher":"t","version":"1.0.0.0","dependencies":[],"preprocessorSymbols":["CLEAN99"]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/b2.al"), SOURCE).unwrap();
    t
}

fn prove(ws: &str, routine: &str, question: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove", ws, routine, question, "--single-app"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    v["payload"]["result"].clone()
}

/// The 1-based line of the first line holding `text` after the line holding
/// `after`.
fn line_of(text: &str, after: &str) -> u64 {
    let start = SOURCE.lines().position(|l| l.contains(after)).expect(after);
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.contains(text))
        .expect(text);
    (start + at + 1) as u64
}

/// An `exit` inside a `#if` arm with no `#else` ends that arm only: the write
/// after the group runs in the build that skips the arm. Before B2 the arm was
/// spliced into the routine's statements, the write was unreachable, and prove
/// said `no (every candidate decided)`.
#[test]
fn an_exit_in_one_arm_does_not_hide_the_code_after_the_group() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Preproc::IfExit", "writes-table:B2 Log B");
    assert_eq!(r["answer"], "yes", "{r:#}");
    let e = &r["evidence"][0];
    assert_eq!(e["type"], "DB_INSERT", "{r:#}");
    assert_eq!(
        e["evidence"]["line"],
        line_of("LogB.Insert();", "procedure IfExit()"),
        "{r:#}"
    );
    // Some build exits first: the write is not on every success path.
    assert_ne!(e["conditionality"], "unconditional-on-success", "{r:#}");
}

/// An `Error` that only one build compiles is no unconditional error of the
/// routine: before B2 throws-error said `unconditional on success`.
#[test]
fn an_error_in_one_arm_is_not_unconditional() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Preproc::IfError", "throws-error");
    assert_eq!(r["answer"], "yes", "{r:#}");
    let e = &r["evidence"][0];
    assert_eq!(e["type"], "ERROR_THROW", "{r:#}");
    assert_ne!(e["conditionality"], "unconditional-on-success", "{r:#}");
}
