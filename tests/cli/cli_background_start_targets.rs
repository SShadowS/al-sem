//! Callgraph F3, the effect side: explain's and digest's `BACKGROUND_TASK`
//! line names the codeunit a background start starts, for every start
//! `program::resolve::background::STARTERS` lists (`StartSession`,
//! `Session.StartSession`, `TaskScheduler.CreateTask` with its failure
//! codeunit, `CurrPage.EnqueueBackgroundTask`). A literal is named by the
//! object the program engine resolved it to (a number by the declared name); a
//! non-literal codeunit is "a codeunit decided at run time"; an id no object
//! declares keeps its number.
//!
//! `CurrPage.EnqueueBackgroundTask` had no `start` capability fact at all, so
//! its routine had no `BACKGROUND_TASK` line.
//!
//! DISCRIMINATION (2026-11-05): leaving `CurrPage.EnqueueBackgroundTask` out
//! of the starters `capability_cone::direct_facts_for_routine` reads (the old
//! receiver test) fails `page-task` (no line); not pushing the `starts` detail
//! in `run_digest_query_full_from_entries` fails `bare-name`; matching a
//! numbered literal by name in `Converter::background_start` fails `number`
//! (the line says `Codeunit 50502`); reading a failure codeunit of `0` as
//! dynamic in `background::start_target_of_info` fails `task-no-failure`
//! (`, on failure a codeunit decided at run time`).

use std::process::Command;

const SOURCE: &str = r#"codeunit 50501 "BS Job"
{
    trigger OnRun()
    begin
    end;
}
codeunit 50502 "BS Two"
{
    trigger OnRun()
    begin
    end;
}
codeunit 50503 "BS Fail"
{
    trigger OnRun()
    begin
    end;
}
codeunit 50504 BSBare
{
    trigger OnRun()
    begin
    end;
}
codeunit 50510 "BS Starter"
{
    procedure BareName()
    var
        S: Integer;
    begin
        StartSession(S, Codeunit::BSBare);
    end;

    procedure Quoted()
    var
        S: Integer;
    begin
        Session.StartSession(S, Codeunit::"BS Job");
    end;

    procedure Number()
    var
        S: Integer;
    begin
        Session.StartSession(S, 50502);
    end;

    procedure Task()
    begin
        TaskScheduler.CreateTask(Codeunit::"BS Job", Codeunit::"BS Fail");
    end;

    procedure TaskSameCodeunit()
    begin
        TaskScheduler.CreateTask(Codeunit::"BS Fail", 50503);
    end;

    procedure TaskNoFailure()
    begin
        TaskScheduler.CreateTask(50501, 0);
    end;

    procedure Dynamic(Id: Integer)
    begin
        TaskScheduler.CreateTask(Id, 50503);
    end;

    procedure DynamicSession(Id: Integer)
    var
        S: Integer;
    begin
        StartSession(S, Id);
    end;

    procedure Missing()
    var
        S: Integer;
    begin
        StartSession(S, 59999);
    end;
}
page 50520 "BS Page"
{
    trigger OnOpenPage()
    var
        T: Integer;
    begin
        CurrPage.EnqueueBackgroundTask(T, Codeunit::"BS Job");
    end;
}
"#;

fn workspace(root: &std::path::Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"35353535-0000-4000-8000-000000000035","name":"BS","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    ws
}

fn alsem(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(
        out.status.success(),
        "{args:?}: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// The `background tasks at …` line of a human explain or digest.
fn background_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("background tasks at"))
        .map(str::to_string)
}

#[test]
fn background_task_line_names_the_started_codeunit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let cases: &[(&str, &str, &str)] = &[
        ("bare-name", "BS Starter::BareName", "Codeunit BSBare"),
        ("quoted", "BS Starter::Quoted", "Codeunit BS Job"),
        ("number", "BS Starter::Number", "Codeunit BS Two"),
        (
            "task",
            "BS Starter::Task",
            "Codeunit BS Job, on failure Codeunit BS Fail",
        ),
        (
            "task-same-codeunit",
            "BS Starter::TaskSameCodeunit",
            "Codeunit BS Fail, on failure Codeunit BS Fail",
        ),
        (
            "task-no-failure",
            "BS Starter::TaskNoFailure",
            "Codeunit BS Job",
        ),
        (
            "dynamic",
            "BS Starter::Dynamic",
            "a codeunit decided at run time, on failure Codeunit BS Fail",
        ),
        (
            "dynamic-session",
            "BS Starter::DynamicSession",
            "a codeunit decided at run time",
        ),
        ("missing", "BS Starter::Missing", "Codeunit 59999"),
        ("page-task", "BS Page::OnOpenPage", "Codeunit BS Job"),
    ];
    for (tag, routine, starts) in cases {
        let explain = alsem(&["explain", w, "--routine", routine, "--format", "human"]);
        let line = background_line(&explain)
            .unwrap_or_else(|| panic!("{tag}: no background tasks line:\n{explain}"));
        assert!(
            line.starts_with("background tasks at 1 place: BACKGROUND_TASK at ws:src/a.al:"),
            "{tag}: {line}"
        );
        assert!(
            line.ends_with(&format!(", starts {starts}")),
            "{tag}: {line}"
        );
    }
    // Digest renders the same line, and its JSON carries the `starts` detail.
    let digest = alsem(&[
        "digest",
        w,
        "--routine",
        "BS Starter::Number",
        "--deterministic",
        "--format",
        "human",
    ]);
    let line = background_line(&digest).unwrap_or_else(|| panic!("digest:\n{digest}"));
    assert!(line.ends_with(", starts Codeunit BS Two"), "{line}");
    let json: serde_json::Value = crate::as_v3(
        serde_json::from_str(&alsem(&[
            "digest",
            w,
            "--routine",
            "BS Starter::Task",
            "--deterministic",
            "--format",
            "json",
        ]))
        .unwrap(),
    );
    let starts: Vec<&serde_json::Value> = json["payload"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .flat_map(|e| e["effects"].as_array().into_iter().flatten())
        .filter(|e| e["type"] == "BACKGROUND_TASK")
        .map(|e| &e["detail"]["starts"])
        .collect();
    assert_eq!(
        starts,
        vec![&serde_json::Value::from(
            "Codeunit BS Job, on failure Codeunit BS Fail"
        )],
        "{json}"
    );
}
