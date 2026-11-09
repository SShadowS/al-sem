//! B4/L16: `[CommitBehavior]` in the forward commit and error answers.
//!
//! Microsoft Learn ("CommitBehavior attribute"): under `Ignore` an explicit
//! `Commit()` does nothing, under `Error` it raises an error, for the
//! attributed method's whole scope (every method it calls, the subscribers of
//! an event it publishes), and only a stricter behavior can be set inside
//! (`Error` over `Ignore` stays `Error`). Before the fix `throws-error`
//! answered `no (every candidate decided)` for a commit under `Error`, and
//! `may-commit` / `commits-on-success-path` answered `yes` for a commit no
//! route runs.
//!
//! Each routine of one temp workspace is asked through the real binary, so
//! the production path (cone → digest split → prove/digest/explain) is what
//! the asserts read. Every precondition is stated in the source below.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50300 "CB Probe"
{
    procedure OnlyCommit()
    begin
        Commit();
    end;

    // A commit whose only route passes an Error scope.
    procedure CallsErrorScope()
    begin
        ErrorScope();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorScope()
    begin
        OnlyCommit();
    end;

    // A commit whose only route passes an Ignore scope.
    procedure CallsIgnoreScope()
    begin
        IgnoreScope();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreScope()
    begin
        OnlyCommit();
    end;

    // Error over Ignore stays Error (Learn: only a stricter behavior).
    procedure CallsErrorOverIgnore()
    begin
        ErrorOverIgnore();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorOverIgnore()
    begin
        IgnoreScope();
    end;

    // Two routes of equal length to one commit: the unconditional one is
    // ignored, the plain one is conditional.
    procedure Mixed(Flag: Boolean)
    begin
        if Flag then
            Helper();
        IgnoreScope();
    end;

    procedure Helper()
    begin
        OnlyCommit();
    end;

    // The shortest route commits; a longer one passes an Error scope.
    procedure Both()
    begin
        OnlyCommit();
        ErrorScope();
    end;

    // An event publisher with Ignore: its subscriber's commit is ignored.
    procedure RaisesIgnoredEvent()
    begin
        OnIgnoredEvent();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    [IntegrationEvent(false, false)]
    procedure OnIgnoredEvent()
    begin
    end;

    // A codeunit run inside an Error scope.
    procedure CallsRunUnderError()
    begin
        RunUnderError();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    procedure RunUnderError()
    begin
        Codeunit.Run(Codeunit::"CB Runner");
    end;

    procedure CallsPlain()
    begin
        OnlyCommit();
    end;
}

codeunit 50301 "CB Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"CB Probe", 'OnIgnoredEvent', '', false, false)]
    local procedure HandleIgnoredEvent()
    begin
        Commit();
    end;
}

codeunit 50302 "CB Runner"
{
    trigger OnRun()
    begin
        Commit();
    end;
}

codeunit 50303 "CB Inherent Error"
{
    InherentCommitBehavior = Error;

    procedure CommitsUnderProperty()
    begin
        Commit();
    end;
}
"#;

fn run(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

fn json(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(cwd, args);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

struct Ws {
    _t: tempfile::TempDir,
    root: std::path::PathBuf,
}

fn workspace() -> Ws {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"20202020-0000-4000-8000-0000000000b4","name":"CB","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Probe.al"), SOURCE).unwrap();
    let root = t.path().to_path_buf();
    Ws { _t: t, root }
}

fn prove(ws: &Ws, routine: &str, question: &str) -> serde_json::Value {
    let selector = if routine.contains("::") {
        routine.to_string()
    } else {
        format!("Codeunit \"CB Probe\"::{routine}")
    };
    json(
        &ws.root,
        &["prove", "ws", &selector, question, "--deterministic"],
    )["payload"]["result"]
        .clone()
}

/// The evidence rows' (type, `commitBehavior` detail).
fn evidence(r: &serde_json::Value) -> Vec<(String, Option<String>)> {
    r["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            (
                e["type"].as_str().unwrap_or("").to_string(),
                e["detail"]["commitBehavior"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn suppressed(r: &serde_json::Value) -> Vec<(String, String)> {
    r["suppressedCommits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            (
                s["behavior"].as_str().unwrap_or("").to_string(),
                s["reason"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn a_commit_under_error_is_an_error_and_no_commit() {
    let ws = workspace();
    let err = prove(&ws, "CallsErrorScope", "throws-error");
    assert_eq!(err["answer"], "yes", "{err}");
    assert_eq!(
        evidence(&err),
        [(
            "ERROR_IMPLICIT".to_string(),
            Some("CommitBehavior Error on Codeunit \"CB Probe\"::ErrorScope".to_string())
        )],
        "{err}"
    );
    for q in ["may-commit", "commits-on-success-path"] {
        let r = prove(&ws, "CallsErrorScope", q);
        assert_eq!(r["answer"], "no", "{q}: {r}");
        assert_eq!(
            suppressed(&r),
            [(
                "Error".to_string(),
                "CommitBehavior Error on Codeunit \"CB Probe\"::ErrorScope".to_string()
            )],
            "{q}: {r}"
        );
    }
}

#[test]
fn a_commit_under_ignore_is_no_commit() {
    let ws = workspace();
    for routine in ["CallsIgnoreScope", "IgnoreScope"] {
        for q in ["may-commit", "commits-on-success-path"] {
            let r = prove(&ws, routine, q);
            assert_eq!(r["answer"], "no", "{routine} {q}: {r}");
            assert_eq!(
                suppressed(&r),
                [(
                    "Ignore".to_string(),
                    "CommitBehavior Ignore on Codeunit \"CB Probe\"::IgnoreScope".to_string()
                )],
                "{routine} {q}: {r}"
            );
        }
        let e = prove(&ws, routine, "throws-error");
        assert_eq!(
            e["answer"], "no",
            "{routine}: an ignored commit raises nothing: {e}"
        );
    }
}

#[test]
fn error_over_ignore_stays_error() {
    let ws = workspace();
    let r = prove(&ws, "CallsErrorOverIgnore", "throws-error");
    assert_eq!(r["answer"], "yes", "{r}");
    assert_eq!(
        evidence(&r),
        [(
            "ERROR_IMPLICIT".to_string(),
            Some("CommitBehavior Error on Codeunit \"CB Probe\"::ErrorOverIgnore".to_string())
        )],
        "{r}"
    );
    assert_eq!(
        prove(&ws, "CallsErrorOverIgnore", "may-commit")["answer"],
        "no"
    );
}

#[test]
fn an_ignored_route_does_not_make_a_commit_unconditional() {
    let ws = workspace();
    // Precondition: both routes are listed (equal length), and before the
    // fix the ignored one made the commit `unconditional on success`.
    let may = prove(&ws, "Mixed", "may-commit");
    assert_eq!(may["answer"], "yes", "{may}");
    let must = prove(&ws, "Mixed", "commits-on-success-path");
    assert_eq!(
        must["answer"], "unknown",
        "only the conditional route commits: {must}"
    );
}

#[test]
fn a_longer_route_through_error_adds_an_error_and_keeps_the_commit() {
    let ws = workspace();
    let commit = prove(&ws, "Both", "commits-on-success-path");
    assert_eq!(commit["answer"], "yes", "{commit}");
    let err = prove(&ws, "Both", "throws-error");
    assert_eq!(err["answer"], "yes", "{err}");
    assert!(
        evidence(&err).iter().any(|(t, cb)| t == "ERROR_IMPLICIT"
            && cb.as_deref() == Some("CommitBehavior Error on Codeunit \"CB Probe\"::ErrorScope")),
        "{err}"
    );
    // Its route passes the Error scope (the shortest route does not).
    let hops = err["evidence"][0]["viaPaths"].to_string();
    assert!(hops.contains("ErrorScope"), "{err}");
}

#[test]
fn an_event_publisher_scope_covers_its_subscribers() {
    let ws = workspace();
    let r = prove(&ws, "RaisesIgnoredEvent", "may-commit");
    assert_eq!(r["answer"], "no", "{r}");
    assert_eq!(
        suppressed(&r),
        [(
            "Ignore".to_string(),
            "CommitBehavior Ignore on Codeunit \"CB Probe\"::OnIgnoredEvent".to_string()
        )],
        "{r}"
    );
}

#[test]
fn a_codeunit_run_inside_an_error_scope_errors() {
    let ws = workspace();
    let r = prove(&ws, "CallsRunUnderError", "throws-error");
    assert_eq!(r["answer"], "yes", "{r}");
    assert_eq!(
        prove(&ws, "CallsRunUnderError", "may-commit")["answer"],
        "no"
    );
}

#[test]
fn an_object_property_only_may_apply() {
    // `InherentCommitBehavior` is not on Microsoft Learn: the commit may run
    // (a `yes` stays) and may raise an error (no definite `no`).
    let ws = workspace();
    let r = "Codeunit \"CB Inherent Error\"::CommitsUnderProperty";
    assert_eq!(prove(&ws, r, "may-commit")["answer"], "yes");
    assert_eq!(prove(&ws, r, "throws-error")["answer"], "yes");
}

#[test]
fn a_plain_commit_is_unchanged() {
    let ws = workspace();
    let r = prove(&ws, "CallsPlain", "commits-on-success-path");
    assert_eq!(r["answer"], "yes", "{r}");
    assert!(r.get("suppressedCommits").is_none(), "{r}");
    assert_eq!(prove(&ws, "CallsPlain", "throws-error")["answer"], "no");
}

/// The `XBase` dependency of `dep_dispatch_fixture` whose `Outer` (the
/// routine the workspace enters, `Ws Work.CallChain → Dep Lib.Outer →
/// Inner`, where `Inner` inserts and commits) declares `behavior`.
fn dep_lib(behavior: &str) -> String {
    format!(
        r#"codeunit 10 "Dep Lib"
{{
    [CommitBehavior(CommitBehavior::{behavior})]
    procedure Outer()
    begin
        Inner();
    end;

    procedure Inner()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
        Commit();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}}
"#
    )
}

#[test]
fn a_commit_beyond_a_crossing_follows_the_dependency_scope() {
    use crate::dep_dispatch_fixture as dd;
    let call_chain = "Codeunit \"Ws Work\"::CallChain";
    let prove_x = |ws: &Path, question: &str| {
        json(ws, &["prove", ".", call_chain, question, "--deterministic"])["payload"]["result"]
            .clone()
    };
    // Control: the plain dependency commits beyond the crossing.
    let plain = tempfile::tempdir().unwrap();
    dd::write_own(plain.path(), "");
    let r = prove_x(plain.path(), "may-commit");
    assert_eq!(r["answer"], "yes", "precondition: a group commit: {r}");

    let ignore = tempfile::tempdir().unwrap();
    dd::write_own_lib(ignore.path(), "", &dep_lib("Ignore"));
    let r = prove_x(ignore.path(), "may-commit");
    assert_eq!(r["answer"], "no", "{r}");
    let s = &r["suppressedCommits"][0];
    assert_eq!(s["behavior"], "Ignore", "{r}");
    // The crossing as every boundary row names it.
    assert_eq!(s["beyond"], "Codeunit Dep Lib::Outer", "{r}");
    assert_eq!(
        s["reason"], "CommitBehavior Ignore on Codeunit \"Dep Lib\"::Outer",
        "{r}"
    );

    let error = tempfile::tempdir().unwrap();
    dd::write_own_lib(error.path(), "", &dep_lib("Error"));
    let r = prove_x(error.path(), "throws-error");
    assert_eq!(r["answer"], "yes", "{r}");
    let rows = r["evidence"].as_array().cloned().unwrap_or_default();
    let commit_row = rows
        .iter()
        .find(|e| {
            e["detail"]["commitBehavior"] == "CommitBehavior Error on Codeunit \"Dep Lib\"::Outer"
        })
        .unwrap_or_else(|| panic!("a commit error row beyond the crossing: {r}"));
    // Its witness ends on the commit (Lib.al line 14), not on the insert's
    // implicit error (line 13), whose own row stays beside it.
    assert_eq!(commit_row["groupWitness"]["terminal"]["line"], 14, "{r}");
    assert!(
        rows.iter()
            .any(|e| e["detail"].get("commitBehavior").is_none()
                && e["factId"] != commit_row["factId"]),
        "the insert's error row keeps its own id: {r}"
    );
    assert_eq!(prove_x(error.path(), "may-commit")["answer"], "no");
}

#[test]
fn the_callers_question_reads_the_same_declarations() {
    // The callers' question (an agent about to add a COMMIT to OnlyCommit)
    // lists the declarations on its callers' paths through the one rule
    // (`commit_scope::declared`) the forward answers apply.
    let ws = workspace();
    let r = prove(&ws, "OnlyCommit", "callers-have-pending-writes");
    let rows: Vec<&str> = r["commitBehavior"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for want in [
        "Error on Codeunit \"CB Probe\"::ErrorScope (an added COMMIT below it is an error)",
        "Ignore on Codeunit \"CB Probe\"::IgnoreScope (an added COMMIT below it is ignored)",
    ] {
        assert!(rows.contains(&want), "{want}: {rows:?}");
    }
}

#[test]
fn the_filtered_slice_applies_the_same_rule() {
    let ws = workspace();
    let at = |routine: &str, question: &str| {
        json(
            &ws.root,
            &[
                "prove",
                "ws",
                &format!("Codeunit \"CB Probe\"::{routine}"),
                question,
                "--site",
                "ws:src/Probe.al:5",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };
    // An ignored commit is no commit site.
    let ignored = at("CallsIgnoreScope", "may-commit");
    assert_eq!(ignored["answer"], "no", "{ignored}");
    // A commit under Error is an error site.
    let error = at("CallsErrorScope", "throws-error");
    assert_eq!(error["answer"], "yes", "{error}");
    // Control: a plain commit stays a commit site and no error site.
    assert_eq!(at("CallsPlain", "may-commit")["answer"], "yes");
    assert_ne!(at("CallsPlain", "throws-error")["answer"], "yes");
}

#[test]
fn digest_and_explain_say_why() {
    let ws = workspace();
    for cmd in ["digest", "explain"] {
        let out = run(
            &ws.root,
            &[
                cmd,
                "ws",
                "--routine",
                "Codeunit \"CB Probe\"::CallsErrorScope",
                "--format",
                "human",
            ],
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains(
                "commit behavior: COMMIT at ws:src/Probe.al:5 raises an error instead \
                 (CommitBehavior Error on Codeunit \"CB Probe\"::ErrorScope)"
            ),
            "{cmd}: {text}"
        );
        assert!(
            text.contains(
                "a COMMIT that raises an error: CommitBehavior Error on Codeunit \"CB Probe\"::ErrorScope"
            ),
            "{cmd}: {text}"
        );
    }
}
