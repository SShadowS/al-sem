//! ENG-08 (engine limits L20): an XmlPort import can fail, and a statement
//! that starts one raises that error. Before the fix `prove throws-error`
//! answered `no` for `Xmlport.Import(X, IS)`, `Xmlport.Run` of an import and a
//! variable's `X.Import()`.
//!
//! Measured on the BC 28.4 Linux sidecar (ENG-08 probe, XML format, statement
//! form): an import onto an existing key, or a key the file holds twice,
//! fails ("The record … already exists"), unless `AutoUpdate`/`AutoReplace`
//! say otherwise; a value that does not evaluate ("The value "abc" can't be
//! evaluated into type Integer.") and malformed XML fail whatever they say.
//! `Ok := Xmlport.Import(…)` returned `false` instead of raising. So the error
//! belongs to the call that starts the import, in statement position. A
//! literal direction the XmlPort's `Direction` refuses (`Xmlport.Run(X, false,
//! false)` on a `Direction = Import` port) fails too, before any trigger:
//! "The XMLPort 'X' is designed for import only."
//!
//! Each routine below does nothing but the one call, so only that call can
//! make the answer `yes`; every answer is read through the CLI.

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 50900 "XI Row" { fields { field(1; K; Integer) { } field(2; Amount; Integer) { } } keys { key(PK; K) { } } }
xmlport 50910 "XI Import"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "XI Row")
            {
                fieldattribute(K; Row.K) { }
                fieldattribute(Amount; Row.Amount) { }
            }
        }
    }
}
xmlport 50911 "XI Update"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "XI Row")
            {
                AutoUpdate = true;
                fieldattribute(K; Row.K) { }
            }
        }
    }
}
xmlport 50912 "XI NoSave"
{
    Direction = Import;
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "XI Row")
            {
                AutoSave = false;
                fieldattribute(K; Row.K) { }
            }
        }
    }
}
xmlport 50913 "XI Export"
{
    Direction = Export;
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "XI Row")
            {
                fieldattribute(K; Row.K) { }
            }
        }
    }
}
xmlport 50914 "XI Both"
{
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "XI Row")
            {
                fieldattribute(K; Row.K) { }
            }
        }
    }
}
codeunit 50920 "XI Caller"
{
    procedure VarRunBoth()
    var
        X: XmlPort "XI Both";
    begin
        X.Run();
    end;
    procedure VarRunExport()
    var
        X: XmlPort "XI Export";
    begin
        X.Run();
    end;
    procedure ImportStmt(IS: InStream)
    begin
        Xmlport.Import(Xmlport::"XI Import", IS);
    end;
    procedure ImportUpdateStmt(IS: InStream)
    begin
        Xmlport.Import(Xmlport::"XI Update", IS);
    end;
    procedure ImportNoSaveStmt(IS: InStream)
    begin
        Xmlport.Import(Xmlport::"XI NoSave", IS);
    end;
    procedure RunImport()
    begin
        Xmlport.Run(Xmlport::"XI Import", false, true);
    end;
    procedure VarImport(IS: InStream)
    var
        X: XmlPort "XI Import";
    begin
        X.SetSource(IS);
        X.Import();
    end;
    procedure ImportOk(IS: InStream) Ok: Boolean
    begin
        Ok := Xmlport.Import(Xmlport::"XI Import", IS);
    end;
    procedure ImportIf(IS: InStream)
    begin
        if Xmlport.Import(Xmlport::"XI Import", IS) then;
    end;
    procedure ImportExit(IS: InStream): Boolean
    begin
        exit(Xmlport.Import(Xmlport::"XI Import", IS));
    end;
    procedure RunExportOnly()
    begin
        Xmlport.Run(Xmlport::"XI Export", false);
    end;
    procedure RunExportArg()
    begin
        Xmlport.Run(Xmlport::"XI Import", false, false);
    end;
    procedure RunImportArgOnExport()
    begin
        Xmlport.Run(Xmlport::"XI Export", false, true);
    end;
    procedure RunExportArgOnBoth()
    begin
        Xmlport.Run(Xmlport::"XI Both", false, false);
    end;
    procedure ImportDynamic(Id: Integer; IS: InStream)
    begin
        Xmlport.Import(Id, IS);
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e08e08e0-0000-4000-8000-0000000000aa","name":"ENG08","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/xi.al"), SOURCE).unwrap();
    t
}

fn throws(ws: &str, routine: &str) -> Value {
    throws_in(ws, routine, &["--single-app"])
}

fn throws_in(ws: &str, routine: &str, mode: &[&str]) -> Value {
    ask(ws, routine, "throws-error", mode)
}

fn ask(ws: &str, routine: &str, question: &str, mode: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(
            ["prove", ws, &format!("XI Caller::{routine}"), question]
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

/// A statement that starts an import raises its error, at the call, whatever
/// `AutoUpdate` or `AutoSave` say (bad input fails any import).
#[test]
fn a_statement_import_may_raise_an_error() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, call) in [
        ("ImportStmt", "Xmlport.Import("),
        ("ImportUpdateStmt", "Xmlport.Import("),
        ("ImportNoSaveStmt", "Xmlport.Import("),
        ("RunImport", "Xmlport.Run("),
        ("VarImport", "X.Import()"),
        // No `Direction`: `Both`, so the run may import.
        ("VarRunBoth", "X.Run()"),
        // A direction the XmlPort refuses fails before any trigger runs
        // (measured: "The XMLPort 'X' is designed for import only.").
        ("RunExportArg", "Xmlport.Run("),
        ("RunImportArgOnExport", "Xmlport.Run("),
    ] {
        let r = throws(ws, routine);
        assert_eq!(r["answer"], "yes", "{routine}: {r:#}");
        let ev = r["evidence"].as_array().expect("evidence");
        assert_eq!(ev.len(), 1, "{routine}: {r:#}");
        assert_eq!(ev[0]["type"], "ERROR_IMPLICIT", "{routine}: {r:#}");
        assert_eq!(
            ev[0]["evidence"]["line"],
            line_of(call, &format!("procedure {routine}(")),
            "{routine}: {r:#}"
        );
    }
}

/// A used result catches the error (`Ok :=` and `if` measured; `exit(…)` uses
/// the value too), and a run that only exports imports nothing.
#[test]
fn a_caught_import_or_an_export_raises_none() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in [
        "ImportOk",
        "ImportIf",
        "ImportExit",
        "RunExportOnly",
        "RunExportArgOnBoth",
        "VarRunExport",
    ] {
        let r = throws(ws, routine);
        assert_eq!(r["answer"], "no", "{routine}: {r:#}");
    }
}

/// A direction the XmlPort refuses runs nothing, so it writes nothing: the
/// platform's import insert is not claimed for an export-only XmlPort run as
/// an import, while an import-capable one still writes.
#[test]
fn a_refused_direction_writes_nothing() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(
        ws,
        "RunImportArgOnExport",
        "writes-table:XI Row",
        &["--single-app"],
    );
    assert_eq!(r["answer"], "no", "{r:#}");
    let r = ask(ws, "RunImport", "writes-table:XI Row", &["--single-app"]);
    assert_eq!(r["answer"], "yes", "{r:#}");
}

/// The default (cross-app) build marks the import too: the mark is set where
/// both builds attach their calls.
#[test]
fn a_statement_import_raises_in_the_default_mode_too() {
    let t = workspace();
    let r = throws_in(t.path().to_str().unwrap(), "ImportStmt", &[]);
    assert_eq!(r["answer"], "yes", "{r:#}");
}

/// An import of an XmlPort not known here (a run-time id) gets no mark, but
/// its run is unresolved, so the answer stays `unknown`, never `no`.
#[test]
fn an_import_of_an_xmlport_not_known_is_unknown() {
    let t = workspace();
    let r = throws(t.path().to_str().unwrap(), "ImportDynamic");
    assert_eq!(r["answer"], "unknown", "{r:#}");
}
