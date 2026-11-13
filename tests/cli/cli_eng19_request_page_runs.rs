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
/// -> `may-commit no` for both (master's answer); drop the
/// `retain_started_triggers` calls -> `throws-error yes` (OnPreReport followed).
/// Each fails; restored, it passes.
#[test]
fn run_request_page_runs_the_request_page_only() {
    let t = workspace();
    for routine in ["ViaRunRequestPage", "ViaRunRequestPageVar"] {
        assert_eq!(answer(t.path(), routine, "may-commit"), "yes", "{routine}");
        assert_eq!(answer(t.path(), routine, "throws-error"), "no", "{routine}");
    }
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
