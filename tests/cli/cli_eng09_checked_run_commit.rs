//! ENG-09 (engine limits L16 item 2): a checked `Codeunit.Run` commits.
//! Before the fix `prove may-commit` answered `no` for
//! `if Codeunit.Run(X) then;` when X has no `Commit()` of its own, and
//! `unknown` for a run-time id.
//!
//! Measured on the BC 28.4 Linux sidecar (ENG-09 probe): after a checked run
//! of a codeunit that inserts a row and never calls `Commit()`, a second
//! checked run succeeds, which BC refuses while a write is pending (the
//! control: an unchecked run, or an `Insert` of the caller's own, makes that
//! second run fail). So the checked run committed its write. It did so under
//! `[CommitBehavior(Ignore)]` and `(Error)` as well, without an error, and in
//! the variable form `Ok := W.Run()`. Learn: "the implicit commit by
//! `Codeunit.Run` isn't affected by `CommitBehavior`".
//!
//! A run that fails commits nothing and the caller carries on (the run
//! returns `false`), so the commit is never `unconditional on success`.

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 50900 "CR Row" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50910 "CR Writer"
{
    trigger OnRun()
    var
        R: Record "CR Row";
    begin
        R.K := 1;
        R.Insert();
    end;
}
codeunit 50920 "CR Caller"
{
    procedure Checked()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    procedure CheckedAssigned(): Boolean
    begin
        exit(Codeunit.Run(Codeunit::"CR Writer"));
    end;
    procedure Unchecked()
    begin
        Codeunit.Run(Codeunit::"CR Writer");
    end;
    procedure CheckedVar()
    var
        W: Codeunit "CR Writer";
    begin
        if W.Run() then;
    end;
    procedure UncheckedVar()
    var
        W: Codeunit "CR Writer";
    begin
        W.Run();
    end;
    procedure CheckedDynamic(Id: Integer)
    begin
        if Codeunit.Run(Id) then;
    end;
    procedure Outer()
    begin
        Checked();
    end;
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure UnderIgnore()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    [CommitBehavior(CommitBehavior::Error)]
    procedure UnderError()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e09e09e0-0000-4000-8000-0000000000aa","name":"ENG09","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/cr.al"), SOURCE).unwrap();
    t
}

fn ask(ws: &str, routine: &str, question: &str, mode: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(
            ["prove", ws, &format!("CR Caller::{routine}"), question]
                .into_iter()
                .chain(mode.iter().copied()),
        )
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

/// A checked run commits, in every form, and the commit is the run's call
/// site; an unchecked run does not.
#[test]
fn a_checked_run_commits_at_its_call_site() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, call) in [
        ("Checked", "Codeunit.Run("),
        ("CheckedAssigned", "Codeunit.Run("),
        ("CheckedVar", "W.Run()"),
        ("CheckedDynamic", "Codeunit.Run(Id)"),
    ] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let ev = &r["evidence"][0];
        assert_eq!(ev["type"], "COMMIT", "{routine}: {r}");
        assert_eq!(
            ev["evidence"]["line"].as_u64(),
            Some(line_of(call, &format!("procedure {routine}("))),
            "{routine}: {r}"
        );
    }
    for routine in ["Unchecked", "UncheckedVar"] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
    }
    // Through a plain call, and in the default (cross-app) mode.
    assert_eq!(
        ask(ws, "Outer", "may-commit", &["--single-app"])["answer"],
        "yes"
    );
    assert_eq!(ask(ws, "Checked", "may-commit", &[])["answer"], "yes");
}

/// A run that fails commits nothing, so the run's commit never makes
/// `commits-on-success-path` a definite yes.
#[test]
fn a_checked_runs_commit_is_never_unconditional() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["Checked", "CheckedVar", "Outer"] {
        let r = ask(ws, routine, "commits-on-success-path", &["--single-app"]);
        assert_eq!(r["answer"], "unknown", "{routine}: {r}");
        assert_eq!(
            r["blockedBy"][0]["kind"], "non-unconditional-effect-exists",
            "{routine}: {r}"
        );
    }
}

/// `[CommitBehavior]` does not change the run's commit: it still commits
/// under `Ignore` and raises no error under `Error`.
#[test]
fn commit_behavior_does_not_change_a_checked_runs_commit() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["UnderIgnore", "UnderError"] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let r = ask(ws, routine, "throws-error", &["--single-app"]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        // The `--site`/`--match` slice applies the same rule.
        let site = format!(
            "src/cr.al:{}",
            line_of("Codeunit.Run(", &format!("procedure {routine}("))
        );
        let r = ask(
            ws,
            routine,
            "may-commit",
            &["--site", &site, "--single-app"],
        );
        assert_eq!(r["answer"], "yes", "{routine} --site: {r}");
        let r = ask(
            ws,
            routine,
            "throws-error",
            &["--match", "Codeunit.Run", "--single-app"],
        );
        assert_eq!(r["answer"], "no", "{routine} --match: {r}");
        assert_eq!(r["otherSites"], serde_json::json!([]), "{routine}: {r}");
    }
}
