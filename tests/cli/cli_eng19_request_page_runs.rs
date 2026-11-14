//! ENG-19 (with ENG-21 merged into it, TLS-44): a report's request page.
//!
//! `Report.RunRequestPage` resolved as a plain built-in, so a write or commit
//! in the request page's triggers answered `no` through it (re-probed on master
//! `1bd755fc`: `may-commit no`). It runs `OnInitReport` and the request page,
//! then stops: no `OnPreReport`, no data item, no `OnPostReport`.
//!
//! A request page shown by `Run`/`RunModal` may be cancelled, which stops the
//! run before `OnPreReport`, yet `OnPreReport` read as `unconditional on
//! success` (master: `commits-on-success-path yes` for a `Commit()` there).
//! Measured on the BC 28.4 Linux sidecar, confirmed on Windows (engine limits
//! L20).

use std::process::Command;

const SOURCE: &str = r#"
report 53300 "E19 Rep"
{
    ProcessingOnly = true;
    requestpage
    {
        trigger OnOpenPage()
        begin
            Commit();
        end;
    }

    trigger OnPreReport()
    begin
        Error('pre');
    end;
}
report 53301 "E19 Quiet"
{
    ProcessingOnly = true;
    requestpage
    {
    }

    trigger OnPreReport()
    begin
        Commit();
    end;
}
report 53303 "E19 NoPage"
{
    ProcessingOnly = true;
    UseRequestPage = false;

    trigger OnPreReport()
    begin
        Commit();
    end;
}
codeunit 53302 "E19 Ops"
{
    procedure ViaRunRequestPage()
    var
        Params: Text;
    begin
        Params := Report.RunRequestPage(Report::"E19 Rep");
    end;

    procedure ViaRunRequestPageVar()
    var
        R: Report "E19 Rep";
        Params: Text;
    begin
        Params := R.RunRequestPage();
    end;

    procedure ViaRunQuiet()
    begin
        Report.Run(Report::"E19 Quiet");
    end;

    procedure ViaRunQuietVar()
    var
        R: Report "E19 Quiet";
    begin
        R.Run();
    end;

    procedure ViaRunQuietNoPage()
    begin
        Report.Run(Report::"E19 Quiet", false);
    end;

    procedure ViaRunNoPageProperty()
    begin
        Report.Run(Report::"E19 NoPage");
    end;

    procedure ViaExecuteQuiet()
    begin
        Report.Execute(Report::"E19 Quiet", '');
    end;

    procedure ViaRunRequestPageDynamic(Id: Integer)
    var
        Params: Text;
    begin
        Params := Report.RunRequestPage(Id);
    end;

    procedure RrpDataItem()
    begin
        Report.RunRequestPage(Report::"E19 DataItem");
    end;

    procedure RrpPost()
    begin
        Report.RunRequestPage(Report::"E19 Post");
    end;

    procedure RrpInit()
    begin
        Report.RunRequestPage(Report::"E19 Init");
    end;

    procedure RunQuietPost()
    begin
        Report.Run(Report::"E19 QuietPost");
    end;

    procedure RrpOverloaded()
    var
        R: Report "E19 Over";
        T: Text;
    begin
        T := R.RunRequestPage('');
    end;
}
report 53310 "E19 DataItem"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem(Num; Integer)
        {
            trigger OnAfterGetRecord()
            begin
                Error('row');
            end;
        }
    }
    requestpage
    {
    }
}
report 53311 "E19 Post"
{
    ProcessingOnly = true;
    requestpage
    {
    }

    trigger OnPostReport()
    begin
        Error('post');
    end;
}
report 53312 "E19 Init"
{
    ProcessingOnly = true;
    requestpage
    {
    }

    trigger OnInitReport()
    begin
        Commit();
    end;
}
report 53313 "E19 QuietPost"
{
    ProcessingOnly = true;
    requestpage
    {
    }

    trigger OnPostReport()
    begin
        Commit();
    end;
}
report 53314 "E19 Over"
{
    ProcessingOnly = true;
    requestpage
    {
        trigger OnOpenPage()
        begin
            Commit();
        end;
    }

    procedure RunRequestPage(P: Code[250]): Text
    begin
        exit('');
    end;
}
"#;

fn answer(ws: &std::path::Path, routine: &str, question: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            ws.to_str().unwrap(),
            &format!("Codeunit \"E19 Ops\"::{routine}"),
            question,
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    v["payload"]["result"]["answer"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e19e19e0-0000-4000-8000-0000000000dd","name":"E19","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/a.al"), SOURCE).unwrap();
    t
}

/// `RunRequestPage`, keyword and variable form, runs the request page (its
/// `OnOpenPage` commits) and not `OnPreReport` (it raises an error).
/// Breaks (2026-11-13): drop `"runrequestpage"` from `dataset::starts_object`
/// -> `may-commit no` for both (master's answer); skip
/// `retain_started_triggers` in `full.rs`'s keyword path -> `ViaRunRequestPage`
/// `throws-error yes` (OnPreReport followed); skip it in the resolver's
/// variable path -> `ViaRunRequestPageVar` `throws-error yes`. Each fails;
/// restored, it passes.
#[test]
fn run_request_page_runs_the_request_page_only() {
    let t = workspace();
    for routine in ["ViaRunRequestPage", "ViaRunRequestPageVar"] {
        assert_eq!(answer(t.path(), routine, "may-commit"), "yes", "{routine}");
        assert_eq!(answer(t.path(), routine, "throws-error"), "no", "{routine}");
    }
}

/// CDO's shape (Fable round 1): a run-time report id is an open run, so a
/// commit in some report's request page may run: `unknown`, never `no`
/// (master `1bd755fc` answered `no`: the call was a plain built-in).
/// Break (2026-11-13): drop `"runrequestpage"` from `dataset::starts_object`
/// -> `no`. Fails; restored, it passes.
#[test]
fn a_run_request_page_of_a_runtime_id_is_open() {
    let t = workspace();
    assert_eq!(
        answer(t.path(), "ViaRunRequestPageDynamic", "may-commit"),
        "unknown"
    );
}

/// Review round 1 (gpt-6.1-sol): `RunRequestPage` runs `OnInitReport` (a
/// commit there is reached) and no data item and no `OnPostReport` (an error
/// in either is not); a request page shown by `Run` may be cancelled before
/// `OnPostReport` too.
/// Breaks (2026-11-13): `start_runs_trigger` keeps member-level triggers ->
/// `RrpDataItem` `throws-error yes`; it keeps `OnPostReport` -> `RrpPost`
/// `throws-error yes`; `request_page_cancel_skips` takes `OnPreReport` only ->
/// `RunQuietPost` `commits-on-success-path yes`. Each fails; restored, it
/// passes.
#[test]
fn run_request_page_runs_init_and_no_dataitem_or_post_report() {
    let t = workspace();
    assert_eq!(answer(t.path(), "RrpDataItem", "throws-error"), "no");
    assert_eq!(answer(t.path(), "RrpPost", "throws-error"), "no");
    assert_eq!(answer(t.path(), "RrpInit", "may-commit"), "yes");
    assert_eq!(
        answer(t.path(), "RunQuietPost", "commits-on-success-path"),
        "unknown"
    );
}

/// A report that declares its own `RunRequestPage(Code[250])` (sol round 1):
/// `R.RunRequestPage('')` may bind to the built-in, so the request page's
/// commit is not ruled out; before `builtin_run_signatures` listed the
/// built-in's signatures, any declared overload hid it (`no`).
/// Break (2026-11-13): drop the `runrequestpage` arm of
/// `builtin_run_signatures` -> `no`. Fails; restored, it passes.
#[test]
fn a_declared_run_request_page_does_not_hide_the_builtin() {
    let t = workspace();
    assert_ne!(answer(t.path(), "RrpOverloaded", "may-commit"), "no");
}

/// TLS-52, on the wire: `alsem graph` sends a `RunRequestPage` (keyword and
/// variable form) to an `ObjectStart` node of the report (`start:
/// "runrequestpage"`), whose `Trigger` edges reach the request page's
/// `OnOpenPage` and not `OnPreReport`; a full run (`Run`) still goes to the
/// object node. The sited graph's own exclusions are pinned (with their
/// discrimination) by `impact::up`'s
/// `a_request_page_run_reaches_only_the_triggers_it_runs`.
/// Break (2026-11-14): the node row's kind back to `"Object"` -> "one
/// ObjectStart node" fails. Restored, it passes.
#[test]
fn the_graph_sends_a_request_page_run_to_a_start_node() {
    let t = workspace();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["graph", "."])
        .current_dir(t.path())
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let nodes = || lines.iter().filter(|v| v["t"] == "node").map(|v| &v["v"]);
    let edges = || lines.iter().filter(|v| v["t"] == "edge").map(|v| &v["v"]);
    let id_of = |pred: &dyn Fn(&serde_json::Value) -> bool| -> Vec<String> {
        nodes()
            .filter(|n| pred(n))
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    let starts = id_of(&|n| n["kind"] == "ObjectStart" && n["objectName"] == "E19 Rep");
    assert_eq!(starts.len(), 1, "one ObjectStart node: {starts:?}");
    let start = &starts[0];
    let start_node = nodes().find(|n| n["id"] == start.as_str()).unwrap();
    assert_eq!(start_node["start"], "runrequestpage");
    assert_eq!(start_node["objectType"], "Report");
    let object = id_of(&|n| n["kind"] == "Object" && n["objectName"] == "E19 Rep");
    assert_eq!(object.len(), 1, "the report's object node stays");
    let routine = |object: &str, member: &str| -> String {
        let ids = id_of(&|n| n["objectName"] == object && n["memberName"] == member);
        assert_eq!(ids.len(), 1, "{object}.{member}: {ids:?}");
        ids[0].clone()
    };
    let has = |caller: &str, callee: &str, kind: &str| {
        edges().any(|e| e["caller"] == caller && e["callee"] == callee && e["kind"] == kind)
    };
    for via in ["ViaRunRequestPage", "ViaRunRequestPageVar"] {
        let from = routine("E19 Ops", via);
        assert!(has(&from, start, "Run"), "{via} runs the start node");
        assert!(
            !has(&from, &object[0], "Run"),
            "{via} does not run the object"
        );
    }
    assert!(has(start, &routine("E19 Rep", "OnOpenPage"), "Trigger"));
    assert!(!has(start, &routine("E19 Rep", "OnPreReport"), "Trigger"));
    // `E19 Quiet` is only ever run in full (`Run`, `Execute`).
    assert!(
        id_of(&|n| n["kind"] == "ObjectStart" && n["objectName"] == "E19 Quiet").is_empty(),
        "a full run makes no start node"
    );
}

/// A request page shown by `Run` (keyword or variable) may be cancelled, so
/// the `Commit()` in `OnPreReport` is not on every success path; with the page
/// hidden (`RequestWindow` false, `UseRequestPage = false`) or a head-less
/// `Execute`, it is.
/// Breaks (2026-11-13): `request_page_cancel_skips` returns `false` ->
/// `ViaRunQuiet` answers `yes` (master's answer); `request_page_may_cancel`
/// ignores `UseRequestPage = false` -> `ViaRunNoPageProperty` answers
/// `unknown`; it ignores a literal `RequestWindow` -> `ViaRunQuietNoPage`
/// answers `unknown`. Each fails; restored, it passes.
#[test]
fn a_cancellable_request_page_makes_on_pre_report_conditional() {
    let t = workspace();
    for (routine, want) in [
        ("ViaRunQuiet", "unknown"),
        ("ViaRunQuietVar", "unknown"),
        ("ViaRunQuietNoPage", "yes"),
        ("ViaRunNoPageProperty", "yes"),
        ("ViaExecuteQuiet", "yes"),
    ] {
        assert_eq!(
            answer(t.path(), routine, "commits-on-success-path"),
            want,
            "{routine}"
        );
    }
}
