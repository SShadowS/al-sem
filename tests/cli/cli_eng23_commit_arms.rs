//! ENG-23: `CbGraph::of_tree` leaves a checked `Codeunit.Run`'s commit out of
//! the `[CommitBehavior]` carriers (`commit_scope::is_run_commit`), and nothing
//! tested it. It changes an answer where a routine declared in two `#if` arms
//! has a `[CommitBehavior(Error)]` arm holding only a checked run beside an arm
//! holding a `Commit()` (with `[CommitBehavior(Ignore)]` or no attribute; the
//! second shape found by gpt-6.1-sol). With the run's commit counted as the
//! Error arm's carrier, the other arm's `Commit()` takes the Error behavior and
//! `prove … throws-error` answers `yes`; it is not an error, so the answer is
//! `no`. A single Error routine and a dependency crossing are controls.
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

/// Review round 1 (gpt-6.1-sol): more shapes the exclusion and the reason meet.
const ROUND2: &str = r#"
table 53100 "E23 Row"
{
    fields { field(1; K; Integer) { } }
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
codeunit 53110 "E23 Inh"
{
    InherentCommitBehavior = Ignore;

#if A
    [CommitBehavior(CommitBehavior::Error)]
    procedure X()
    begin
        if Codeunit.Run(Codeunit::"E23 Worker") then;
    end;
#else
    procedure X()
    begin
        Y();
    end;
#endif

    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure Y()
    begin
        Commit();
    end;
}
codeunit 53112 "E23 NoAttr"
{
#if A
    [CommitBehavior(CommitBehavior::Error)]
    procedure X2()
    begin
        if Codeunit.Run(Codeunit::"E23 Worker") then;
    end;
#else
    procedure X2()
    begin
        Commit();
    end;
#endif
}
codeunit 53111 "E23 Outer"
{
    procedure RootInh()
    var
        I: Codeunit "E23 Inh";
    begin
        I.X();
    end;

    procedure RootNoAttr()
    var
        N: Codeunit "E23 NoAttr";
    begin
        N.X2();
    end;

    procedure RootDep()
    var
        D: Codeunit "E23 Dep Entry";
    begin
        D.Go();
    end;
}
"#;

/// A dependency with source (rule B's crossing): its entry calls an
/// Error-scoped helper holding only a checked run, then an Ignore-scoped one
/// holding a `Commit()`.
const DEP: &str = r#"
codeunit 70981 "E23 Dep Worker"
{
    trigger OnRun()
    begin
    end;
}
codeunit 70980 "E23 Dep Entry"
{
    procedure Go()
    begin
        ErrH();
        IgnH();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    local procedure ErrH()
    begin
        if Codeunit.Run(Codeunit::"E23 Dep Worker") then;
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure IgnH()
    begin
        Commit();
    end;
}
"#;

fn round2_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e23e23e0-0000-4000-8000-0000000000bc","name":"E23 R2","publisher":"t","version":"1.0.0.0","dependencies":[{"id":"22222222-0000-0000-0000-0000000000e3","name":"E23 Dep","publisher":"t","version":"1.0.0.0"}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), ROUND2).unwrap();
    let symbols = r#"{"RuntimeVersion":"13.0","AppId":"22222222-0000-0000-0000-0000000000e3","Name":"E23 Dep","Publisher":"t","Version":"1.0.0.0","Codeunits":[{"Id":70980,"Name":"E23 Dep Entry","Methods":[{"Name":"Go","Parameters":[]}]},{"Id":70981,"Name":"E23 Dep Worker","Methods":[]}]}"#;
    crate::symbol_app::write_source_app(
        &t.path().join(".alpackages/t_E23 Dep_1.0.0.0.app"),
        "22222222-0000-0000-0000-0000000000e3",
        "E23 Dep",
        "1.0.0.0",
        symbols,
        &[("src/Dep.al", DEP)],
        "",
    );
    t
}

fn run_in(ws: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut full: Vec<&str> = vec![args[0], ws.to_str().unwrap()];
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

/// The exclusion in two more shapes (sol round 1: "one reachable shape" was
/// false): an arm with no attribute holding the `Commit()` (`RootNoAttr`:
/// cannot throw), and a dependency entry whose Error helper only runs a
/// checked run and whose Ignore helper commits (`RootDep`, rule B's
/// crossing: the run commits, under no behavior, so `may-commit yes`).
/// Break (2026-11-13): drop `&& !is_run_commit(f)` in `CbGraph::of_tree` ->
/// `RootNoAttr` answers `throws-error yes` (the arm's normal commit read under
/// the Error arm). Fails; restored, it passes. `RootDep` is a control: under
/// the same break it answered `may-commit yes`, `commits-on-success-path
/// unknown` and `throws-error yes`, as with the exclusion (its `throws-error`
/// is the run's own error inside dependency code, which the trap pass does not
/// rule, engine limits L28), so no answer of this crossing shape was found to
/// depend on the exclusion.
#[test]
fn the_exclusion_holds_without_an_attribute_and_across_a_dependency() {
    let t = round2_workspace();
    let ask = |routine: &str, q: &str| {
        let v = run_in(
            t.path(),
            &["prove", &format!("Codeunit \"E23 Outer\"::{routine}"), q],
        );
        (
            v["payload"]["result"]["answer"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            v,
        )
    };
    let (a, v) = ask("RootNoAttr", "throws-error");
    assert_eq!(a, "no", "{v}");
    let (a, v) = ask("RootDep", "may-commit");
    assert_eq!(a, "yes", "{v}");
}

/// An ignored commit's reason names the source of the Ignore it is ignored
/// under: X's Error is an attribute of one arm, its Ignore the object's
/// `InherentCommitBehavior` (the other arm declares nothing), so the reason
/// is the object property's, never "CommitBehavior Ignore on …X" (sol round
/// 1: the merged declaration had lost which arm set which behavior).
/// Break (2026-11-13): `reason_for` reads the merged `source` instead of the
/// bit's (`ignore_source`) -> "CommitBehavior Ignore on Codeunit \"E23
/// Inh\"::X". Fails; restored, it passes.
#[test]
fn an_ignored_commits_reason_names_its_own_source() {
    let t = round2_workspace();
    let v = run_in(t.path(), &["digest", "--routine", "RootInh"]);
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
    assert_eq!(
        found[0]["reason"],
        "InherentCommitBehavior Ignore on the object of Codeunit \"E23 Inh\"::X (may apply)",
        "{found:?}"
    );
}
