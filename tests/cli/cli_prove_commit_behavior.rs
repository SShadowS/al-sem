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

    // The shortest route (IgnoreScope, 2 hops) is ignored; a longer one
    // (H1 -> H2 -> OnlyCommit, 3 hops) commits normally.
    procedure LongNormal()
    begin
        IgnoreScope();
        H1();
    end;

    procedure H1()
    begin
        H2();
    end;

    procedure H2()
    begin
        OnlyCommit();
    end;

    // A checked run inside an Ignore scope: the run commits at its end.
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreCheckedRun()
    begin
        if Codeunit.Run(Codeunit::"CB Runner") then;
    end;

    // A checked run inside an Error scope: an error in the run is trapped
    // by it, and its implicit commit is not affected.
    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorCheckedRun()
    begin
        if Codeunit.Run(Codeunit::"CB Runner") then;
    end;

    // An unchecked run inside an Ignore scope.
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreUncheckedRun()
    begin
        Codeunit.Run(Codeunit::"CB Runner");
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
fn a_codeunit_run_inside_an_error_scope_may_error() {
    // Learn does not say whether the scope follows into a codeunit started
    // with an unchecked `Codeunit.Run`: the commit may raise an error (yes)
    // and may run (no definite `no`).
    let ws = workspace();
    let r = prove(&ws, "CallsRunUnderError", "throws-error");
    assert_eq!(r["answer"], "yes", "{r}");
    assert_eq!(
        prove(&ws, "CallsRunUnderError", "may-commit")["answer"],
        "yes"
    );
    let r = prove(&ws, "IgnoreUncheckedRun", "may-commit");
    assert_eq!(r["answer"], "yes", "{r}");
}

#[test]
fn a_checked_run_inside_an_ignore_scope_still_commits() {
    // Learn ("Codeunit.Run" method): a checked run commits at its end, and
    // that implicit commit "isn't affected by CommitBehavior". Before the
    // fix of this test's finding the branch answered `no` with a false
    // "ignored" reason.
    let ws = workspace();
    for q in ["may-commit", "commits-on-success-path"] {
        let r = prove(&ws, "IgnoreCheckedRun", q);
        assert_ne!(r["answer"], "no", "{q}: {r}");
        assert!(r.get("suppressedCommits").is_none(), "{q}: {r}");
    }
    assert_eq!(
        prove(&ws, "IgnoreCheckedRun", "may-commit")["answer"],
        "yes"
    );
    // Under Error, Learn does not say whether the scope follows into the
    // run. If it does, the commit raises an error, the run traps it (it
    // returns false and rolls back) and nothing commits; if it does not, the
    // run commits. So the commit stays possible (yes), is not certain on
    // success (before this round's fix: "unconditional on success"), and no
    // commit error escapes the run.
    assert_eq!(prove(&ws, "ErrorCheckedRun", "may-commit")["answer"], "yes");
    let must = prove(&ws, "ErrorCheckedRun", "commits-on-success-path");
    assert_eq!(must["answer"], "unknown", "{must}");
    let e = prove(&ws, "ErrorCheckedRun", "throws-error");
    assert!(
        evidence(&e).iter().all(|(_, cb)| cb.is_none()),
        "no commit error row past a checked run: {e}"
    );
}

#[test]
fn a_longer_normal_route_keeps_a_commit_the_listed_routes_ignore() {
    // Precondition: the witness search lists only the shortest route
    // (LongNormal -> IgnoreScope -> OnlyCommit), which is ignored; the
    // normal route is one hop longer.
    let ws = workspace();
    let r = prove(&ws, "LongNormal", "may-commit");
    assert_eq!(r["answer"], "yes", "{r}");
    assert!(r.get("suppressedCommits").is_none(), "{r}");
    let via = r["evidence"][0]["viaPaths"].to_string();
    assert!(via.contains("H2"), "the found normal route is shown: {r}");
}

#[test]
fn a_longer_normal_prefix_to_a_crossing_keeps_the_commit_beyond() {
    // Root reaches the crossing (`Ws Work.CallChain` -> `Dep Lib.Outer`,
    // which commits) by a short prefix through an Ignore routine and by a
    // longer plain one. The witness lists only the short prefix.
    use crate::dep_dispatch_fixture as dd;
    let extra = r#"codeunit 50175 "Ws Scope"
{
    procedure Root()
    begin
        IgnoreW();
        A1();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreW()
    var
        W: Codeunit "Ws Work";
    begin
        W.CallChain();
    end;

    procedure A1()
    begin
        A2();
    end;

    procedure A2()
    var
        W: Codeunit "Ws Work";
    begin
        W.CallChain();
    end;
}
"#;
    let dir = tempfile::tempdir().unwrap();
    dd::write_own(dir.path(), extra);
    let ask = |routine: &str| {
        json(
            dir.path(),
            &["prove", ".", routine, "may-commit", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    // Control: through the Ignore routine alone the commit is ignored.
    let ignored = ask("Codeunit \"Ws Scope\"::IgnoreW");
    assert_eq!(ignored["answer"], "no", "precondition: {ignored}");
    let r = ask("Codeunit \"Ws Scope\"::Root");
    assert_eq!(r["answer"], "yes", "{r}");
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

/// One workspace (`ws/`, app `CB2`) of `source`, for the tests below.
fn workspace_of(source: &str) -> Ws {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"20202020-0000-4000-8000-0000000002b4","name":"CB2","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Probe.al"), source).unwrap();
    let root = t.path().to_path_buf();
    Ws { _t: t, root }
}

/// Every way of starting an object, each inside an Ignore scope, each
/// reaching a commit only through the started object. The objects' commits
/// sit in their run triggers (a report's `OnPreReport`, a page's
/// `OnOpenPage`, a query's `OnBeforeOpen`, a codeunit's `OnRun`) or, for an
/// XmlPort import, in the `OnInsert` trigger of the table it imports into.
const STARTS: &str = r#"table 50340 "ST"
{
    fields { field(1; K; Integer) { } }
    trigger OnInsert()
    begin
        Commit();
    end;
}

report 50340 "SR"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(D; "ST") { }
    }
    trigger OnPreReport()
    begin
        Commit();
    end;
}

page 50340 "SP"
{
    SourceTable = "ST";
    trigger OnOpenPage()
    begin
        Commit();
    end;
}

query 50340 "SQ"
{
    elements
    {
        dataitem(D; "ST") { column(K; K) { } }
    }
    trigger OnBeforeOpen()
    begin
        Commit();
    end;
}

xmlport 50340 "SX"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(T; "ST")
            {
                fieldelement(K; T.K) { }
            }
        }
    }
}

codeunit 50341 "SC"
{
    trigger OnRun()
    begin
        Commit();
    end;

    procedure Plain()
    begin
        Commit();
    end;
}

codeunit 50342 "Starts"
{
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportRun()
    begin
        Report.Run(Report::"SR");
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportRunModal()
    begin
        Report.RunModal(Report::"SR");
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportSaveAsPdf()
    begin
        Report.SaveAsPdf(Report::"SR", 'x.pdf');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportExecute()
    begin
        Report.Execute(Report::"SR", '');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportVarRun()
    var
        Rep: Report "SR";
    begin
        Rep.Run();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportVarSaveAsPdf()
    var
        Rep: Report "SR";
    begin
        Rep.SaveAsPdf('x.pdf');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ReportVarPrint()
    var
        Rep: Report "SR";
    begin
        Rep.Print('');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure PageRun()
    begin
        Page.Run(Page::"SP");
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure PageVarRunModal()
    var
        P: Page "SP";
    begin
        P.RunModal();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure QueryOpen()
    var
        Q: Query "SQ";
    begin
        Q.Open();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure QuerySaveAsCsv(O: OutStream)
    begin
        Query.SaveAsCsv(Query::"SQ", O);
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure XmlportImport(S: InStream)
    begin
        Xmlport.Import(Xmlport::"SX", S);
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CodeunitVarRun()
    var
        C: Codeunit "SC";
    begin
        C.Run();
    end;

    // Controls: a procedure call is no start, so the scope applies for
    // certain; so does a table trigger reached by the routine's own insert.
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure PlainCall()
    var
        C: Codeunit "SC";
    begin
        C.Plain();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure OwnInsert()
    var
        R: Record "ST";
    begin
        R.Insert(true);
    end;
}
"#;

#[test]
fn every_way_of_starting_an_object_lets_the_scope_only_may_apply() {
    // B4 review, finding B: before the fix only `PCallee::ObjectRun` and a
    // codeunit variable's `.Run` into `OnRun` were runs; a report variable's
    // `.Run()`, the report saves and the query opens B6 routes into their
    // triggers were plain calls, so Ignore gave a definite (wrong) `no`.
    // Learn does not say whether the scope follows into a started object,
    // so a commit there stays possible and is never certain on success.
    let ws = workspace_of(STARTS);
    let ask = |routine: &str, q: &str| prove(&ws, &format!("Codeunit \"Starts\"::{routine}"), q);
    for routine in [
        "ReportRun",
        "ReportRunModal",
        "ReportSaveAsPdf",
        "ReportExecute",
        "ReportVarRun",
        "ReportVarSaveAsPdf",
        "ReportVarPrint",
        "PageRun",
        "PageVarRunModal",
        "QueryOpen",
        "QuerySaveAsCsv",
        "XmlportImport",
        "CodeunitVarRun",
    ] {
        let may = ask(routine, "may-commit");
        assert_eq!(may["answer"], "yes", "{routine}: {may}");
        assert!(may.get("suppressedCommits").is_none(), "{routine}: {may}");
        let must = ask(routine, "commits-on-success-path");
        assert_ne!(must["answer"], "yes", "{routine}: {must}");
    }
    // Controls (the precondition that the scope is read at all).
    for routine in ["PlainCall", "OwnInsert"] {
        let r = ask(routine, "may-commit");
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        assert_eq!(suppressed(&r).len(), 1, "{routine}: {r}");
    }
}

/// The `XBase` dependency with a codeunit run: `Dep Runner` (12), whose
/// `OnRun` inserts and commits.
const DEP_RUNNER: &str = r#"codeunit 10 "Dep Lib"
{
    procedure Outer()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
        Commit();
    end;

    procedure Outer2()
    begin
        Codeunit.Run(Codeunit::"Dep Runner");
    end;

    procedure Outer3()
    begin
        if Codeunit.Run(Codeunit::"Dep Runner") then;
    end;

    procedure Outer4()
    var
        R: Codeunit "Dep Runner";
    begin
        R.Run();
    end;
}

codeunit 12 "Dep Runner"
{
    trigger OnRun()
    var
        Cust: Record Customer;
    begin
        Cust.Insert();
        Commit();
    end;
}
"#;

const DEP_RUN_CALLERS: &str = r#"codeunit 50176 "Ws Runs"
{
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreCheckedDepRun()
    begin
        if Codeunit.Run(Codeunit::"Dep Runner") then;
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreUncheckedDepRun()
    begin
        Codeunit.Run(Codeunit::"Dep Runner");
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreCheckedDepMember()
    var
        R: Codeunit "Dep Runner";
    begin
        if R.Run() then;
    end;

    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorCheckedDepRun()
    begin
        if Codeunit.Run(Codeunit::"Dep Runner") then;
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnorePlainDep()
    var
        L: Codeunit "Dep Lib";
    begin
        L.Outer();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreNestedDepRun()
    var
        L: Codeunit "Dep Lib";
    begin
        L.Outer2();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreNestedDepChecked()
    var
        L: Codeunit "Dep Lib";
    begin
        L.Outer3();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure IgnoreNestedDepMember()
    var
        L: Codeunit "Dep Lib";
    begin
        L.Outer4();
    end;
}
"#;

/// The workspace `DEP_RUN_CALLERS` over a dependency app `XBase` built from
/// source (`DEP_RUNNER`): the cross-app model loads the dependency's bodies.
fn dep_run_workspace() -> tempfile::TempDir {
    use crate::dep_dispatch_fixture as dd;
    let dir = tempfile::tempdir().unwrap();
    dd::write_workspace(dir.path(), DEP_RUN_CALLERS);
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"Codeunits":[{{"Id":10,"Name":"Dep Lib","Methods":[{{"Name":"Outer","Parameters":[]}},{{"Name":"Outer2","Parameters":[]}},{{"Name":"Outer3","Parameters":[]}},{{"Name":"Outer4","Parameters":[]}}]}},{{"Id":12,"Name":"Dep Runner","Methods":[]}}],"AppId":"{}","Name":"XBase","Publisher":"probe","Version":"1.0.0.0"}}"#,
        dd::DEP_GUID
    );
    let customer = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
    crate::symbol_app::write_source_app(
        &dir.path().join(".alpackages").join(dd::DEP_APP_FILE),
        dd::DEP_GUID,
        "XBase",
        "1.0.0.0",
        &symbols,
        &[("src/Customer.al", customer), ("src/Lib.al", DEP_RUNNER)],
        "",
    );
    dir
}

/// `prove <Ws Runs::routine> <q>` over the workspace at `dir`: its result.
fn ask_ws_runs(dir: &Path, routine: &str, q: &str) -> serde_json::Value {
    json(
        dir,
        &[
            "prove",
            ".",
            &format!("Codeunit \"Ws Runs\"::{routine}"),
            q,
            "--deterministic",
        ],
    )["payload"]["result"]
        .clone()
}

#[test]
fn an_object_run_into_a_dependency_keeps_its_commit() {
    // B4 review, finding A: the crossing edge into a dependency was composed
    // as a plain call whatever its step, so a run of a dependency codeunit
    // under Ignore gave a definite (wrong) `no`. Learn ("Codeunit.Run"): a
    // checked run commits at its end and that commit "isn't affected by
    // CommitBehavior"; an unchecked run only MAY carry the scope.
    let dir = dep_run_workspace();
    let ask = |routine: &str, q: &str| ask_ws_runs(dir.path(), routine, q);
    // Control: a plain call into the dependency is ignored for certain.
    let plain = ask("IgnorePlainDep", "may-commit");
    assert_eq!(plain["answer"], "no", "precondition: {plain}");
    for routine in [
        "IgnoreCheckedDepRun",
        "IgnoreUncheckedDepRun",
        "IgnoreCheckedDepMember",
        "ErrorCheckedDepRun",
    ] {
        let r = ask(routine, "may-commit");
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        assert!(r.get("suppressedCommits").is_none(), "{routine}: {r}");
    }
    // A checked run traps the commit's error: no commit error row escapes.
    let e = ask("ErrorCheckedDepRun", "throws-error");
    assert!(
        evidence(&e).iter().all(|(_, cb)| cb.is_none()),
        "no commit error row past a checked run: {e}"
    );
}

#[test]
fn an_object_run_inside_dependency_code_keeps_its_commit() {
    // B4 round-2 review: the run is INSIDE the dependency's own code
    // (`Dep Lib::Outer2/3/4` start `Dep Runner`, which commits), reached by a
    // plain call from an Ignore scope. The cross-app tree keeps the
    // dependency code's calls apart from the model's (`dependency_calls`),
    // and the run steps were read from the model's calls alone, so the run
    // edge was a plain step and the commit beyond read as ignored: a definite
    // (wrong) `no`. The scope only MAY follow into a started object (Learn
    // does not say it does), and a checked run's commit is not affected by
    // CommitBehavior (Learn, "Codeunit.Run"), so the commit stays possible.
    let dir = dep_run_workspace();
    let ask = |routine: &str, q: &str| ask_ws_runs(dir.path(), routine, q);
    // Precondition (control): a plain call into the dependency, whose own
    // commit is not behind a run, is ignored for certain.
    let plain = ask("IgnorePlainDep", "may-commit");
    assert_eq!(plain["answer"], "no", "precondition: {plain}");
    for routine in [
        "IgnoreNestedDepRun",
        "IgnoreNestedDepChecked",
        "IgnoreNestedDepMember",
    ] {
        let r = ask(routine, "may-commit");
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        assert!(r.get("suppressedCommits").is_none(), "{routine}: {r}");
        // On one reading only: never a definite answer either way.
        let s = ask(routine, "commits-on-success-path");
        assert_eq!(s["answer"], "unknown", "{routine}: {s}");
        // The digest reads the same rule.
        let out = run(
            dir.path(),
            &[
                "digest",
                ".",
                "--routine",
                &format!("Codeunit \"Ws Runs\"::{routine}"),
                "--format",
                "human",
            ],
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("commits: may"), "{routine}: {text}");
        assert!(!text.contains("are ignored"), "{routine}: {text}");
    }
}

const TRAPS: &str = r#"codeunit 50350 "CB Traps"
{
    [CommitBehavior(CommitBehavior::Error)]
    procedure ErrorTry()
    begin
        if TryCommit() then;
    end;

    [TryFunction]
    procedure TryCommit()
    begin
        Commit();
    end;

    procedure PlainTry()
    begin
        if TryError() then;
    end;

    [TryFunction]
    procedure TryError()
    begin
        Error('boom');
    end;
}
"#;

#[test]
fn the_filtered_slice_traps_a_commit_turned_into_an_error() {
    // B4 review, finding C: the slice ruled the trap by the site's own row
    // type (COMMIT), so a commit turned into an error inside a used
    // TryFunction read `yes` with `--site`, `--match` and `--op`, while the
    // unfiltered answer and the digest said `no` (caught).
    let ws = workspace_of(TRAPS);
    let ask = |routine: &str, extra: &[&str]| {
        let selector = format!("Codeunit \"CB Traps\"::{routine}");
        let mut args = vec!["prove", "ws", &selector, "throws-error", "--deterministic"];
        args.extend_from_slice(extra);
        json(&ws.root, &args)["payload"]["result"].clone()
    };
    // ENG-10 (measured on BC 28 on premises): the `Commit()` inside the used
    // try call is refused, an error the try call does not catch, raised if
    // the server refuses it: the commit's own error is caught, the refusal
    // is not, unfiltered and filtered alike.
    let unfiltered = ask("ErrorTry", &[]);
    assert_eq!(unfiltered["answer"], "yes", "precondition: {unfiltered}");
    let escaping: Vec<&serde_json::Value> =
        unfiltered["evidence"].as_array().unwrap().iter().collect();
    assert!(
        escaping
            .iter()
            .all(|e| e["detail"]["resourceId"] == "write-in-try"
                && e["trap"]["verdict"] == "escapes-if-write-refused"),
        "only the try call's refusal escapes: {unfiltered}"
    );
    for filter in [
        &["--site", "ws:src/Probe.al:12"][..],
        &["--match", "commit"][..],
        &["--op", "Commit"][..],
    ] {
        let r = ask("ErrorTry", filter);
        assert_eq!(r["answer"], "yes", "{filter:?}: {r}");
    }
    // Control: a plain error in a used TryFunction, asked with --site.
    let control = ask("PlainTry", &["--site", "ws:src/Probe.al:23"]);
    assert_eq!(control["answer"], "no", "{control}");
}

/// A report whose run trigger and own `Print`/`SaveAsWord` procedures all
/// commit, called from Ignore scopes by a report variable.
const BINDINGS: &str = r#"table 50390 "BBT" { fields { field(1; K; Integer) { } } }

report 50390 "BBR"
{
    ProcessingOnly = true;
    dataset { dataitem(D; "BBT") { } }
    trigger OnPreReport()
    begin
        Commit();
    end;

    procedure Print()
    begin
        Commit();
    end;

    procedure SaveAsWord(T: Code[250])
    begin
        Commit();
    end;
}

codeunit 50391 "BB Calls"
{
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallPrint()
    var
        R: Report "BBR";
    begin
        R.Print();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallUndecided()
    var
        R: Report "BBR";
    begin
        R.SaveAsWord('x.docx');
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure CallBuiltin()
    var
        R: Report "BBR";
    begin
        R.SaveAsPdf('x.pdf');
    end;
}
"#;

#[test]
fn a_start_follows_the_overload_binding_and_an_undecided_one_gives_no_no() {
    // Integration of B4 (one rule for "this call starts its object", read by
    // the resolver and the commit scope) with B6 (a report's Print/Execute/
    // SaveAs* bind by AL's overload rule). `R.Print()` binds to the report's
    // own `procedure Print()` (alc 18.0.43.1464, B6): a plain call, so the
    // Ignore scope applies for certain. B4's earlier arity rule called it a
    // start, and the scope only MAY have followed: a wrong `yes`. An
    // undecided binding keeps its unresolved route: never a definite `no`.
    let ws = workspace_of(BINDINGS);
    let ask = |routine: &str, q: &str| prove(&ws, &format!("Codeunit \"BB Calls\"::{routine}"), q);
    let print = ask("CallPrint", "may-commit");
    assert_eq!(print["answer"], "no", "CallPrint: {print}");
    assert_eq!(suppressed(&print).len(), 1, "CallPrint: {print}");
    // The undecided call stays unresolved (B6's OverloadAmbiguous route):
    // `unknown`, neither the procedure's plain call (`no`) nor a start.
    let undecided = ask("CallUndecided", "may-commit");
    assert_eq!(undecided["answer"], "unknown", "CallUndecided: {undecided}");
    let builtin = ask("CallBuiltin", "may-commit");
    assert_eq!(builtin["answer"], "yes", "CallBuiltin: {builtin}");
    let must = ask("CallBuiltin", "commits-on-success-path");
    assert_ne!(must["answer"], "yes", "CallBuiltin: {must}");
}
