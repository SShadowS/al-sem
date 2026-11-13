//! ENG-23: `CbGraph::of_tree` leaves a checked `Codeunit.Run`'s commit out of
//! the `[CommitBehavior]` carriers (`commit_scope::is_run_commit`), and nothing
//! tested it. The one shape where it changes an answer (probe, 2026-11-13): a
//! routine declared in two `#if` arms, one `[CommitBehavior(Error)]` arm holding
//! only a checked run, the other `[CommitBehavior(Ignore)]` holding only a
//! `Commit()`. With the run's commit counted as the Error arm's carrier, the
//! Ignore arm's `Commit()` takes the Error behavior and `prove … throws-error`
//! answers `yes`; it is ignored, so the answer is `no`.
//!
//! The same shape showed a label defect: the ignored commit's reason named the
//! merged declaration's word ("CommitBehavior Error on …X") instead of the
//! behavior asked about (`Declared::reason_for`).

use std::process::Command;

const SOURCE: &str = r#"
table 53100 "E23 Row"
{
    fields
    {
        field(1; K; Integer) { }
    }
    keys { key(PK; K) { } }
}
codeunit 53101 "E23 Worker"
{
    trigger OnRun()
    var
        R: Record "E23 Row";
    begin
        R.K := 1;
        R.Insert();
    end;
}
codeunit 53102 "E23 Ops"
{
    procedure Root()
    begin
        X();
    end;

#if A
    [CommitBehavior(CommitBehavior::Error)]
    local procedure X()
    begin
        if Codeunit.Run(Codeunit::"E23 Worker") then;
    end;
#else
    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure X()
    begin
        Commit();
    end;
#endif
}
codeunit 53103 "E23 Plain"
{
    procedure RootPlain()
    begin
        Y();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    local procedure Y()
    begin
        if Codeunit.Run(Codeunit::"E23 Worker") then;
    end;
}
"#;

fn run(args: &[&str]) -> serde_json::Value {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e23e23e0-0000-4000-8000-0000000000bb","name":"E23","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    let ws = t.path().to_str().unwrap().to_string();
    let mut full: Vec<&str> = vec![args[0], &ws];
    full.extend_from_slice(&args[1..]);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&full)
        .args(["--format", "json"])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// The checked run's commit carries no `[CommitBehavior]`: the Ignore arm's
/// `Commit()` stays ignored and raises nothing, the run's own error is not
/// raised (nothing pending), so `Root` cannot throw.
/// Break (2026-11-13): drop `&& !is_run_commit(f)` in `CbGraph::of_tree`
/// (commit_scope.rs) -> `yes`, its row the Ignore arm's `Commit()` (line 36)
/// read as an error ("CommitBehavior Error on …X"). Fails; restored, it passes.
/// Control (Fable round 1): `RootPlain`, one `[CommitBehavior(Error)]` routine
/// holding only a checked run, answers `no` with and without the exclusion
/// (run under the same break: still `no`), so the arms shape is the one that
/// reaches it.
#[test]
fn a_checked_runs_commit_carries_no_commit_behavior() {
    let v = run(&["prove", "Codeunit \"E23 Ops\"::Root", "throws-error"]);
    assert_eq!(v["payload"]["result"]["answer"], "no", "{v}");
    let p = run(&["prove", "Codeunit \"E23 Plain\"::RootPlain", "throws-error"]);
    assert_eq!(p["payload"]["result"]["answer"], "no", "{p}");
}

/// The ignored commit's reason names the behavior it is ignored under
/// (Ignore), not the merged arms' word (Error).
/// Break (2026-11-13): `digest.rs` `why` took the merged declaration's word
/// (the old `Declared::reason`, since removed) instead of `reason_for(bit, …)`
/// -> "CommitBehavior Error on …X". Fails; restored, it passes.
#[test]
fn an_ignored_commits_reason_names_ignore() {
    let v = run(&["digest", "--routine", "Root"]);
    let mut found = Vec::new();
    fn walk(v: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(serde_json::Value::Array(a)) = m.get("suppressedCommits") {
                    out.extend(a.iter().cloned());
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(&v, &mut found);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["behavior"], "Ignore");
    assert_eq!(
        found[0]["reason"], "CommitBehavior Ignore on Codeunit \"E23 Ops\"::X",
        "{found:?}"
    );
}
