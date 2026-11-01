//! A typed-receiver call `Cu.Run(x)` with an argument binds to a procedure
//! `Run(…)` the codeunit declares, not to the built-in `Codeunit.Run` (which
//! runs `OnRun`).
//!
//! AL's rule (probed with alc 18.1.43.24897): `procedure Run()` with no
//! parameter is AL0440, so a call with no argument is always the built-in; once
//! the codeunit declares any `Run(…)`, a call with arguments binds among the
//! declared procedures only. Microsoft's System Application has such a
//! procedure (`"Azure AD User Mgmt. Impl.".Run(ForUserSecurityId: Guid)`).
//!
//! Before the fix the resolver sent every `Run` call of arity 0 or 1 to the
//! entry trigger. For a codeunit with no `OnRun` that is an empty run,
//! classified `honestEmpty` ("provably no callee"), so `prove may-commit`
//! answered `no` for a routine whose callee commits. The original report read
//! it as a cross-app demand gap (workspace → dependency `Start` → dependency
//! `Helper.Run`); the demand closure is transitive, and the same chain with
//! the routine renamed resolves (pinned below as the precondition).
//!
//! Every fixture is a workspace calling `StartCU.Start`, whose body calls
//! `Helper.Run(CreateGuid())`; `Helper.Run(G: Guid)` commits and `Helper` has
//! no `OnRun`. The dependency is given both ways the engine reads dependency
//! source: a folder named in `al-sem.dependencies.json`, and a `.app` package
//! with embedded source.

use std::path::Path;
use std::process::Command;

const WS_GUID: &str = "aaaa0991-0000-0000-0000-000000000991";
const DEP_GUID: &str = "bbbb0991-0000-0000-0000-000000000991";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn start_al(call: &str) -> String {
    format!(
        "codeunit 50000 StartCU\n{{\n    procedure Start()\n    var\n        H: Codeunit Helper;\n    begin\n        {call}\n    end;\n}}\n"
    )
}

/// `Helper`: `routine(G: Guid)` commits; an `OnRun` running `on_run` when it
/// is `Some`, no `OnRun` at all when `None`.
fn helper_al(routine: &str, on_run: Option<&str>) -> String {
    let trigger = on_run
        .map(|body| format!("    trigger OnRun()\n    begin\n        {body}\n    end;\n\n"))
        .unwrap_or_default();
    format!(
        "codeunit 50001 Helper\n{{\n{trigger}    procedure {routine}(G: Guid)\n    begin\n        Commit();\n    end;\n}}\n"
    )
}

const CALLER_AL: &str = "codeunit 60000 Caller\n{\n    procedure DoIt()\n    var\n        S: Codeunit StartCU;\n    begin\n        S.Start();\n    end;\n}\n";

#[derive(Clone, Copy)]
enum DepForm {
    /// The dependency's folder, named in `al-sem.dependencies.json`.
    SourceFolder,
    /// A `.app` in `.alpackages` with the source embedded.
    SourceApp,
}

/// A workspace whose dependency holds `StartCU` (calling `call`) and
/// `Helper` ([`helper_al`]).
fn two_app_fixture(root: &Path, form: DepForm, call: &str, routine: &str, on_run: Option<&str>) {
    let ws = root.join("ws");
    let dep_ref =
        format!(r#"{{"id":"{DEP_GUID}","name":"DepApp","publisher":"probe","version":"1.0.0.0"}}"#);
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"WsApp","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":60000,"to":60999}}],"dependencies":[{dep_ref}]}}"#
        ),
    );
    write(&ws.join("src/Caller.al"), CALLER_AL);
    let start = start_al(call);
    let helper = helper_al(routine, on_run);
    match form {
        DepForm::SourceFolder => {
            let dep = root.join("dep");
            write(
                &dep.join("app.json"),
                &format!(
                    r#"{{"id":"{DEP_GUID}","name":"DepApp","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50000,"to":50999}}],"dependencies":[]}}"#
                ),
            );
            write(&dep.join("src/StartCU.al"), &start);
            write(&dep.join("src/Helper.al"), &helper);
            write(
                &ws.join("al-sem.dependencies.json"),
                r#"{"sources":["../dep"]}"#,
            );
        }
        DepForm::SourceApp => {
            let symbols = format!(
                r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50000,"Name":"StartCU","Methods":[{{"Name":"Start","Parameters":[]}}]}},{{"Id":50001,"Name":"Helper","Methods":[{{"Name":"{routine}","Parameters":[{{"Name":"G","TypeDefinition":{{"Name":"Guid"}}}}]}}]}}],"AppId":"{DEP_GUID}","Name":"DepApp","Publisher":"probe","Version":"1.0.0.0"}}"#
            );
            crate::symbol_app::write_source_app(
                &ws.join(".alpackages/probe_DepApp_1.0.0.0.app"),
                DEP_GUID,
                "DepApp",
                "1.0.0.0",
                &symbols,
                &[("src/StartCU.al", &start), ("src/Helper.al", &helper)],
                "",
            );
        }
    }
}

/// `alsem prove <ws> DoIt may-commit`'s answer.
fn may_commit(ws: &Path, routine: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("prove")
        .arg(ws)
        .args([routine, "may-commit", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .expect("run alsem prove");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    v["payload"]["result"]["answer"]
        .as_str()
        .unwrap_or_else(|| panic!("no answer: {v}"))
        .to_string()
}

/// `aldump --dependency-bodies-stats <ws>`'s `all` histogram.
fn dependency_bodies(ws: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--dependency-bodies-stats")
        .arg(ws)
        .env("RUST_LOG", "off")
        .output()
        .expect("run aldump");
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    v["all"].clone()
}

fn check_two_app(form: DepForm, label: &str) {
    // Precondition: the same chain with the routine named `DoWork` reaches the
    // commit, so the dependency body and the demand closure work.
    let pre = tempfile::tempdir().expect("tempdir");
    two_app_fixture(pre.path(), form, "H.DoWork(CreateGuid());", "DoWork", None);
    let pre_ws = pre.path().join("ws");
    assert_eq!(
        may_commit(&pre_ws, "DoIt"),
        "yes",
        "precondition ({label}): the chain reaches Helper's commit"
    );

    let t = tempfile::tempdir().expect("tempdir");
    two_app_fixture(t.path(), form, "H.Run(CreateGuid());", "Run", None);
    let ws = t.path().join("ws");
    let h = dependency_bodies(&ws);
    // The dependency's three call sites: `H.Run(…)`, `CreateGuid()` and
    // Helper's `Commit()`.
    assert_eq!(h["total"], 3, "{label}: {h}");
    assert_eq!(
        h["honestEmpty"], 0,
        "{label}: `H.Run(G)` is not an empty run: {h}"
    );
    assert_eq!(
        h["resolvedSource"], 1,
        "{label}: `H.Run(G)` resolves to Helper.Run: {h}"
    );
    assert_eq!(
        may_commit(&ws, "DoIt"),
        "yes",
        "{label}: DoIt reaches Helper.Run's commit through StartCU.Start"
    );
}

#[test]
fn run_with_argument_binds_to_declared_procedure_source_folder_dependency() {
    check_two_app(DepForm::SourceFolder, "source folder");
}

#[test]
fn run_with_argument_binds_to_declared_procedure_source_app_dependency() {
    check_two_app(DepForm::SourceApp, "source .app");
}

/// A single app: the call site is the workspace's own, and its routine's
/// `prove` sees the commit.
#[test]
fn run_with_argument_binds_to_declared_procedure_single_app() {
    let t = tempfile::tempdir().expect("tempdir");
    let ws = t.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"WsApp","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50000,"to":60999}}],"dependencies":[]}}"#
        ),
    );
    write(
        &ws.join("src/StartCU.al"),
        &start_al("H.Run(CreateGuid());"),
    );
    write(&ws.join("src/Helper.al"), &helper_al("Run", None));
    assert_eq!(may_commit(ws, "Start"), "yes");
}

/// The other side of the rule: a call with NO argument is the built-in run
/// even when the codeunit declares `Run(G: Guid)` — it reaches `OnRun`, not
/// the procedure.
#[test]
fn run_without_argument_stays_the_entry_trigger() {
    let t = tempfile::tempdir().expect("tempdir");
    let ws = t.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"WsApp","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50000,"to":60999}}],"dependencies":[]}}"#
        ),
    );
    // OnRun is empty and only the procedure commits: `H.Run()` must not commit.
    write(&ws.join("src/StartCU.al"), &start_al("H.Run();"));
    write(&ws.join("src/Helper.al"), &helper_al("Run", Some("")));
    assert_eq!(may_commit(ws, "Start"), "no", "H.Run() reached Run(G)");
    // OnRun commits: `H.Run()` does.
    write(
        &ws.join("src/Helper.al"),
        &helper_al("Run", Some("Commit();")),
    );
    assert_eq!(may_commit(ws, "Start"), "yes", "H.Run() missed OnRun");
}

/// A Page's procedures include its extensions' (alc 18.1 compiles
/// `P.Run(CreateGuid())` against a `pageextension`'s `Run(G: Guid)`): the
/// call binds to the extension's procedure, not to the page's built-in run.
#[test]
fn page_run_with_argument_binds_to_extension_procedure() {
    let t = tempfile::tempdir().expect("tempdir");
    let ws = t.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"WsApp","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50000,"to":60999}}],"dependencies":[]}}"#
        ),
    );
    write(&ws.join("src/Pg.al"), "page 50003 Pg\n{\n}\n");
    write(
        &ws.join("src/PgExt.al"),
        "pageextension 50004 PgExt extends Pg\n{\n    procedure Run(G: Guid)\n    begin\n        Commit();\n    end;\n}\n",
    );
    write(
        &ws.join("src/Opener.al"),
        "codeunit 50005 Opener\n{\n    procedure Open()\n    var\n        P: Page Pg;\n    begin\n        P.Run(CreateGuid());\n    end;\n}\n",
    );
    assert_eq!(may_commit(ws, "Open"), "yes");
}
