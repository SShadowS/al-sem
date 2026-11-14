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
