//! ENG-165: `alsem prove <routine> commit-between --after A --before B`
//! (design `docs/superpowers/specs/2026-11-25-eng165-commit-between-design.md`
//! §8). One routine per shape; every expected answer was written down in the
//! spec before the code ran. `Send()` inserts into an outbox (A leaves a
//! pending write); B is the log `Insert`. The AL compiles clean with alc
//! 18.0.41 (the same shapes, `/work/.al-sem/engine-notes/eng165/fx2`).
//!
//! The two `Do*` shapes are public twins of the two places the engine finds
//! in the DO workspace that match the owner's description (not confirmed by
//! the owner): commit, deliver, commit on success, then an error back in the
//! caller before the log (DO-1); `Commit` + a checked run inside the send,
//! then a `Modify` before the log (DO-2).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const TABLES: &str = r#"table 50600 "EB Log"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}

table 50601 "EB Outbox"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}

table 50602 "EB Doc"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnInsert()
    begin
        Commit();
    end;
}

table 50603 "EB Log Committing"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnInsert()
    begin
        Commit();
    end;
}
"#;

/// Each `// <TAG>` marks a site the tests name by its line.
const SHAPES: &str = r#"codeunit 50610 "EB Shapes"
{
    procedure S1()
    begin
        Send(); // S1A
        Commit();
        L.Insert(); // S1B
    end;

    procedure S2()
    begin
        Send(); // S2A
        L.Insert(); // S2B
        Commit();
    end;

    procedure S3()
    begin
        Send(); // S3A
        CommitHelper();
        L.Insert(); // S3B
    end;

    procedure S4(Flag: Boolean)
    begin
        Send(); // S4A
        if Flag then
            Commit();
        L.Insert(); // S4B
    end;

    procedure S5(N: Integer)
    var
        I: Integer;
    begin
        for I := 1 to N do begin
            Send(); // S5A
            L.Insert(); // S5B
            Commit();
        end;
    end;

    procedure S5b(N: Integer)
    begin
        repeat
            L.Insert(); // S5bB
            Send(); // S5bA
            Commit();
            N -= 1;
        until N <= 0;
    end;

    procedure S5c(N: Integer; Flag: Boolean)
    var
        I: Integer;
    begin
        for I := 1 to N do begin
            Send(); // S5cA
            if Flag then
                L.Insert(); // S5cB
            Commit();
        end;
    end;

    procedure S5d(Flag: Boolean)
    begin
        Send(); // S5dA
        MaybeLog(Flag);
        Commit();
        Log();
    end;

    procedure S5f(N: Integer)
    var
        I: Integer;
    begin
        for I := 1 to N do begin
            Send(); // S5fA
            if TryLog() then;
            Commit();
        end;
    end;

    procedure S6()
    begin
        Send(); // S6A
        OnAfterSend();
        L.Insert(); // S6B
    end;

    procedure S7(Amount: Decimal)
    begin
        Send(); // S7A
        Commit();
        if Amount < 0 then
            Error('negative'); // S7E
        L.Insert(); // S7B
    end;

    procedure S8()
    begin
        Send(); // S8A
        if Codeunit.Run(Codeunit::"EB Worker") then;
        L.Insert(); // S8B
    end;

    procedure S9()
    begin
        L.Insert(); // S9B
        Send(); // S9A
        Commit();
    end;

    procedure S10(Flag: Boolean)
    begin
        Send(); // S10A
        if Flag then begin
            Commit();
            exit;
        end;
        L.Insert(); // S10B
    end;

    procedure S11(Flag: Boolean)
    begin
        if Flag then
            Send() // S11A
        else
            Commit();
        L.Insert(); // S11B
    end;

    procedure S12(X: Boolean)
    begin
        Send(); // S12A
        if X then begin
            Commit();
            Error('x');
        end;
        L.Insert(); // S12B
    end;

    procedure S13(Id: Integer)
    begin
        Send(); // S13A
        Codeunit.Run(Id);
        L.Insert(); // S13B
    end;

    procedure S13b(Runner: Interface "EB Runner")
    begin
        Send(); // S13bA
        Runner.Go();
        L.Insert(); // S13bB
    end;

    procedure S14()
    begin
        Send(); // S14A
        IgnoredCommit();
        L.Insert(); // S14B
    end;

    procedure S14e()
    begin
        Send(); // S14eA
        ErrorCommit();
        L.Insert(); // S14eB
    end;

    procedure S15()
    begin
        SendThenCommit();
        L.Insert(); // S15B
    end;

    procedure S16()
    begin
        Send(); // S16A
        CommitThenLog();
    end;

    procedure S17()
    begin
        Via1();
        Via2();
    end;

    procedure S18()
    begin
        Send(); // S18A
        OnIsolatedCommit();
        L.Insert(); // S18B
    end;

    procedure S20()
    begin
        Send(); // S20A
        if Codeunit.Run(Codeunit::"EB Committing Worker") then;
        L.Insert(); // S20B
    end;

    procedure S21()
    begin
        Send(); // S21A
        Page.RunModal(Page::"EB Log Card");
        L.Insert(); // S21B
    end;

    procedure S22()
    var
        D: Record "EB Doc";
    begin
        Send(); // S22A
        D.Insert(true);
        L.Insert(); // S22B
    end;

    procedure S23()
    var
        C: Record "EB Log Committing";
    begin
        Send(); // S23A
        C.Insert(true); // S23B
    end;

    procedure S24()
    begin
        Send(); // S24A
        Commit();
        if TryCheck() then;
        L.Insert(); // S24B
    end;

    procedure S25()
    begin
        if Codeunit.Run(Codeunit::"EB Worker 25") then;
    end;

    procedure S26()
    begin
        Log2(Send(), Helper()); // S26
        Log2(Helper(), Send()); // S26R
    end;

    procedure S27()
    begin
        Send(); Send(); // S27
        L.Insert(); // S27B
    end;

    procedure S30(Bad: Boolean)
    begin
        if Codeunit.Run(Codeunit::"EB Sender 30") then;
        if Bad then
            Error('bad'); // S30E
        L.Insert(); // S30B
    end;

    procedure S31()
    begin
        if TrySend() then;
        L.Insert(); // S31B
    end;

    procedure Do1(Bad: Boolean)
    begin
        Deliver();
        if Bad then
            Error('bad'); // DO1E
        L.Insert(); // DO1B
    end;

    procedure Do2()
    var
        D: Record "EB Doc";
    begin
        PostMessage(); // DO2A
        D.Modify(); // DO2M
        L.Insert(); // DO2B
    end;

    local procedure Deliver()
    var
        D: Record "EB Doc";
    begin
        D.Modify();
        Commit();
        if Transmit() then // DO1A
            Commit();
    end;

    local procedure Transmit(): Boolean
    begin
        exit(true);
    end;

    local procedure PostMessage()
    begin
        Commit();
        if Codeunit.Run(Codeunit::"EB Dispatcher") then;
    end;

    local procedure Via1()
    begin
        SendX();
        LogX();
    end;

    local procedure Via2()
    begin
        SendX();
        Commit();
        LogX();
    end;

    local procedure SendX()
    begin
        Send(); // S17A
    end;

    local procedure LogX()
    begin
        L.Insert(); // S17B
    end;

    local procedure SendThenCommit()
    begin
        Send(); // S15A
        Commit();
    end;

    local procedure CommitThenLog()
    begin
        Commit();
        L.Insert(); // S16B
    end;

    local procedure MaybeLog(Flag: Boolean)
    begin
        if Flag then
            Log();
    end;

    local procedure Log()
    begin
        L.Insert(); // LOGB
    end;

    [TryFunction]
    local procedure TryLog()
    begin
        L.Insert(); // TRYLOGB
    end;

    [TryFunction]
    local procedure TryCheck()
    begin
        Error('checked'); // S24E
    end;

    [TryFunction]
    local procedure TrySend()
    begin
        Send(); // S31A
        Commit();
        Error('after'); // S31E
    end;

    local procedure CommitHelper()
    begin
        Commit();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure IgnoredCommit()
    begin
        Commit();
    end;

    [CommitBehavior(CommitBehavior::Error)]
    local procedure ErrorCommit()
    begin
        Commit();
    end;

    local procedure Helper(): Boolean
    begin
        Commit();
        exit(true);
    end;

    local procedure Log2(X: Boolean; Y: Boolean)
    begin
    end;

    procedure Send(): Boolean
    var
        O: Record "EB Outbox";
    begin
        O.Insert();
        exit(true);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterSend()
    begin
    end;

    [IntegrationEvent(false, false, true)]
    local procedure OnIsolatedCommit()
    begin
    end;

    var
        L: Record "EB Log";
}
"#;

const OTHERS: &str = r#"interface "EB Runner"
{
    procedure Go();
}

codeunit 50620 "EB Runner Impl" implements "EB Runner"
{
    procedure Go()
    begin
    end;
}

codeunit 50621 "EB Subscribers"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EB Shapes", 'OnAfterSend', '', false, false)]
    local procedure AfterSend()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EB Shapes", 'OnIsolatedCommit', '', false, false)]
    local procedure IsolatedCommit()
    begin
        Commit();
    end;
}

codeunit 50622 "EB Worker"
{
    trigger OnRun()
    var
        O: Record "EB Outbox";
    begin
        O.Insert();
    end;
}

codeunit 50623 "EB Committing Worker"
{
    trigger OnRun()
    begin
        Commit();
    end;
}

codeunit 50624 "EB Worker 25"
{
    trigger OnRun()
    var
        L: Record "EB Log";
        S: Codeunit "EB Shapes";
    begin
        S.Send(); // S25A
        Commit();
        Check();
        L.Insert(); // S25B
    end;

    local procedure Check()
    begin
        Error('check'); // S25E
    end;
}

codeunit 50625 "EB Sender 30"
{
    trigger OnRun()
    var
        S: Codeunit "EB Shapes";
    begin
        S.Send(); // S30A
    end;
}

codeunit 50626 "EB Dispatcher"
{
    trigger OnRun()
    var
        S: Codeunit "EB Shapes";
    begin
        S.Send(); // DO2DEEP
    end;
}

page 50627 "EB Log Card"
{
    PageType = Card;
    SourceTable = "EB Log";

    layout
    {
        area(Content)
        {
            field(No; Rec."No.") { ApplicationArea = All; }
        }
    }
}
"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"16516516-0000-4000-8000-0000000e0165","name":"EB","publisher":"t","version":"1.0.0.0","runtime":"15.0","idRanges":[{"from":50600,"to":50699}]}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Tables.al"), TABLES).unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SHAPES).unwrap();
    std::fs::write(ws.join("src/Others.al"), OTHERS).unwrap();
    ws
}

/// `file:line` of the line marked `// <tag>` (1-based).
fn at(tag: &str) -> String {
    for (file, src) in [("Shapes.al", SHAPES), ("Others.al", OTHERS)] {
        let marker = format!("// {tag}");
        if let Some(i) = src.lines().position(|l| l.trim_end().ends_with(&marker)) {
            return format!("{file}:{}", i + 1);
        }
    }
    panic!("no line marked {tag}");
}

/// `file:line` of the marked line and the 1-based column of `needle` on it.
fn at_col(tag: &str, needle: &str) -> String {
    let marker = format!("// {tag}");
    let (i, line) = SHAPES
        .lines()
        .enumerate()
        .find(|(_, l)| l.trim_end().ends_with(&marker))
        .expect("marked line");
    format!("Shapes.al:{}:{}", i + 1, line.find(needle).unwrap() + 1)
}

fn line_of(tag: &str) -> u64 {
    at(tag).rsplit(':').next().unwrap().parse().unwrap()
}

fn prove(
    ws: &Path,
    routine: &str,
    after: &str,
    before: &str,
    extra: &[&str],
) -> (i32, Value, String) {
    let mut args = vec![
        "prove".to_string(),
        ws.to_str().unwrap().to_string(),
        routine.to_string(),
        "commit-between".to_string(),
        "--after".to_string(),
        after.to_string(),
        "--before".to_string(),
        before.to_string(),
        "--single-app".to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let v = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    (
        out.status.code().unwrap_or(-1),
        v,
        format!("{stdout}{stderr}"),
    )
}

struct Answer {
    answer: String,
    slice: Value,
    raw: String,
}

fn ask(ws: &Path, routine: &str, a: &str, b: &str) -> Answer {
    ask_with(ws, routine, a, b, &[])
}

fn ask_with(ws: &Path, routine: &str, a: &str, b: &str, extra: &[&str]) -> Answer {
    let (code, v, raw) = prove(ws, routine, a, b, extra);
    assert_eq!(code, 0, "{routine}: {raw}");
    Answer {
        answer: v["payload"]["result"]["answer"]
            .as_str()
            .unwrap_or("?")
            .to_string(),
        slice: v["payload"]["slice"].clone(),
        raw,
    }
}

fn rows<'v>(s: &'v Value, list: &str) -> Vec<&'v Value> {
    s[list]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn has_label(row: &Value, label: &str) -> bool {
    row["labels"].as_array().is_some_and(|l| {
        l.iter()
            .any(|x| x.as_str().is_some_and(|s| s.starts_with(label)))
    })
}

fn lines_of(list: &[&Value]) -> Vec<u64> {
    let mut v: Vec<u64> = list.iter().filter_map(|r| r["line"].as_u64()).collect();
    v.sort_unstable();
    v
}

fn r(name: &str) -> String {
    format!("EB Shapes::{name}")
}

/// One fixture for the whole file: each test asks its own shape.
fn ws() -> &'static Path {
    static WS: std::sync::OnceLock<(tempfile::TempDir, std::path::PathBuf)> =
        std::sync::OnceLock::new();
    let (_, p) = WS.get_or_init(|| {
        let t = tempfile::tempdir().unwrap();
        let p = workspace(t.path());
        (t, p)
    });
    p
}

#[test]
fn s1_straight_commit_is_between() {
    let a = ask(ws(), &r("S1"), &at("S1A"), &at("S1B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert_eq!(rows(&a.slice, "between").len(), 1, "{}", a.raw);
}

#[test]
fn s2_commit_after_b_is_no() {
    let a = ask(ws(), &r("S2"), &at("S2A"), &at("S2B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
}

#[test]
fn s3_commit_in_a_callee_is_between() {
    let a = ask(ws(), &r("S3"), &at("S3A"), &at("S3B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "through-callee"),
        "{}",
        a.raw
    );
}

#[test]
fn s4_commit_on_one_branch_is_between() {
    let a = ask(ws(), &r("S4"), &at("S4A"), &at("S4B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s5_sure_b_cuts_the_loop() {
    let a = ask(ws(), &r("S5"), &at("S5A"), &at("S5B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
}

#[test]
fn s5b_the_next_b_is_over_the_back_edge() {
    let a = ask(ws(), &r("S5b"), &at("S5bA"), &at("S5bB"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "loop-carried"),
        "{}",
        a.raw
    );
}

#[test]
fn s5c_a_maybe_skipped_b_is_no_cut() {
    let a = ask(ws(), &r("S5c"), &at("S5cA"), &at("S5cB"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "loop-carried"),
        "{}",
        a.raw
    );
}

#[test]
fn s5d_a_b_a_helper_may_reach_is_no_cut() {
    let a = ask(ws(), &r("S5d"), &at("S5dA"), &at("LOGB"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s5f_no_cut_at_a_b_inside_a_trap() {
    let a = ask(ws(), &r("S5f"), &at("S5fA"), &at("TRYLOGB"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s6_commit_in_a_subscriber_is_between() {
    let a = ask(ws(), &r("S6"), &at("S6A"), &at("S6B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "through-event"),
        "{}",
        a.raw
    );
}

#[test]
fn s7_the_error_after_the_commit_is_listed() {
    let a = ask(ws(), &r("S7"), &at("S7A"), &at("S7B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let c = rows(&a.slice, "between")[0];
    let after: Vec<&Value> = c["errorsAfterCommit"].as_array().unwrap().iter().collect();
    assert!(lines_of(&after).contains(&line_of("S7E")), "{}", a.raw);
    assert!(
        a.raw.contains("DANGEROUS") || a.slice.is_object(),
        "{}",
        a.raw
    );
}

#[test]
fn s8_a_checked_run_with_a_pending_write_is_an_error_not_a_commit() {
    let a = ask(ws(), &r("S8"), &at("S8A"), &at("S8B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    let errs = rows(&a.slice, "errorsBetween");
    assert!(
        errs.iter().any(|e| has_label(e, "run-error-rolls-back-a")),
        "{}",
        a.raw
    );
}

#[test]
fn s9_b_never_after_a() {
    let a = ask(ws(), &r("S9"), &at("S9A"), &at("S9B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert_eq!(a.slice["relation"], "b-never-after-a", "{}", a.raw);
}

#[test]
fn s10_commit_then_exit_is_without_b() {
    let a = ask(ws(), &r("S10"), &at("S10A"), &at("S10B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    let w = rows(&a.slice, "commitsWithoutB");
    assert_eq!(w.len(), 1, "{}", a.raw);
    assert!(
        w[0]["ends"].as_array().unwrap().iter().any(|e| e == "exit"),
        "{}",
        a.raw
    );
}

#[test]
fn s11_commit_in_the_other_arm_is_no() {
    let a = ask(ws(), &r("S11"), &at("S11A"), &at("S11B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
}

#[test]
fn s12_commit_then_error_is_without_b() {
    let a = ask(ws(), &r("S12"), &at("S12A"), &at("S12B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    let w = rows(&a.slice, "commitsWithoutB");
    assert_eq!(w.len(), 1, "{}", a.raw);
    assert!(
        w[0]["ends"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "error"),
        "{}",
        a.raw
    );
}

#[test]
fn s13_a_dynamic_run_between_is_unknown() {
    let a = ask(ws(), &r("S13"), &at("S13A"), &at("S13B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
}

#[test]
fn s13b_an_open_interface_call_is_unknown_unless_closed_world() {
    let a = ask(ws(), &r("S13b"), &at("S13bA"), &at("S13bB"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    let c = ask_with(
        ws(),
        &r("S13b"),
        &at("S13bA"),
        &at("S13bB"),
        &["--closed-world"],
    );
    assert_eq!(c.answer, "no", "{}", c.raw);
}

#[test]
fn s14_commit_behavior_ignore_and_error() {
    let a = ask(ws(), &r("S14"), &at("S14A"), &at("S14B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    let e = ask(ws(), &r("S14e"), &at("S14eA"), &at("S14eB"));
    assert_eq!(e.answer, "no", "{}", e.raw);
    assert!(
        rows(&e.slice, "errorsBetween")
            .iter()
            .any(|x| has_label(x, "commit-behavior-error")),
        "{}",
        e.raw
    );
}

#[test]
fn s15_commit_in_a_tail() {
    let a = ask(ws(), &r("S15"), &at("S15A"), &at("S15B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert_eq!(rows(&a.slice, "between")[0]["part"], "A-tail", "{}", a.raw);
}

#[test]
fn s16_commit_in_b_head() {
    let a = ask(ws(), &r("S16"), &at("S16A"), &at("S16B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert_eq!(rows(&a.slice, "between")[0]["part"], "B-head", "{}", a.raw);
}

#[test]
fn s17_every_route_pair_is_checked() {
    let a = ask(ws(), &r("S17"), &at("S17A"), &at("S17B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(a.slice["pairsExamined"].as_u64().unwrap() >= 2, "{}", a.raw);
}

#[test]
fn s18_commit_in_an_isolated_subscriber_is_class_1() {
    let a = ask(ws(), &r("S18"), &at("S18A"), &at("S18B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "isolated-subscriber"),
        "{}",
        a.raw
    );
}

#[test]
fn s20_commit_inside_a_checked_run_is_class_2() {
    let a = ask(ws(), &r("S20"), &at("S20A"), &at("S20B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert!(
        !rows(&a.slice, "ownTransactionCommits").is_empty(),
        "{}",
        a.raw
    );
    assert!(
        rows(&a.slice, "errorsBetween")
            .iter()
            .any(|e| has_label(e, "run-error-rolls-back-a")),
        "{}",
        a.raw
    );
}

#[test]
fn s21_page_run_modal_with_a_pending_write_is_an_error_row() {
    let a = ask(ws(), &r("S21"), &at("S21A"), &at("S21B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert!(
        rows(&a.slice, "errorsBetween")
            .iter()
            .any(|e| has_label(e, "page-run") || has_label(e, "runmodal")),
        "{}",
        a.raw
    );
}

#[test]
fn s22_a_trigger_that_commits_is_between() {
    let a = ask(ws(), &r("S22"), &at("S22A"), &at("S22B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        has_label(rows(&a.slice, "between")[0], "through-trigger"),
        "{}",
        a.raw
    );
}

#[test]
fn s23_commit_inside_b() {
    let a = ask(ws(), &r("S23"), &at("S23A"), &at("S23B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert_eq!(rows(&a.slice, "commitsInsideB").len(), 1, "{}", a.raw);
}

#[test]
fn s24_an_error_trapped_below_the_region_is_left_out() {
    let a = ask(ws(), &r("S24"), &at("S24A"), &at("S24B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let after: Vec<&Value> = rows(&a.slice, "between")[0]["errorsAfterCommit"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert!(!lines_of(&after).contains(&line_of("S24E")), "{}", a.raw);
}

#[test]
fn s25_an_error_trapped_above_the_meet_routine_is_swallowed() {
    let a = ask(ws(), &r("S25"), &at("S25A"), &at("S25B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let after: Vec<&Value> = rows(&a.slice, "between")[0]["errorsAfterCommit"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    let e = after
        .iter()
        .find(|e| e["line"].as_u64() == Some(line_of("S25E")))
        .unwrap_or_else(|| panic!("{}", a.raw));
    assert!(has_label(e, "swallowed-above-meet"), "{}", a.raw);
}

#[test]
fn s26_nested_calls_run_left_to_right() {
    let a = ask(
        ws(),
        &r("S26"),
        &at_col("S26", "Send"),
        &at_col("S26", "Log2"),
    );
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let b = ask(
        ws(),
        &r("S26"),
        &at_col("S26R", "Send"),
        &at_col("S26R", "Log2"),
    );
    assert_eq!(b.answer, "no", "{}", b.raw);
}

#[test]
fn s27_an_ambiguous_line_is_refused() {
    let (code, _, raw) = prove(ws(), &r("S27"), &at("S27"), &at("S27B"), &[]);
    assert_eq!(code, 1, "{raw}");
    assert!(raw.contains("add the column"), "{raw}");
}

#[test]
fn s30_a_checked_run_on_a_route_makes_a_durable() {
    let a = ask(ws(), &r("S30"), &at("S30A"), &at("S30B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let c = rows(&a.slice, "between")
        .into_iter()
        .find(|c| c["kind"] == "run-commit-of-A")
        .unwrap_or_else(|| panic!("{}", a.raw));
    let after: Vec<&Value> = c["errorsAfterCommit"].as_array().unwrap().iter().collect();
    assert!(lines_of(&after).contains(&line_of("S30E")), "{}", a.raw);
}

#[test]
fn s31_a_trap_on_a_route_catches_the_tail_error() {
    let a = ask(ws(), &r("S31"), &at("S31A"), &at("S31B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let all: Vec<&Value> = rows(&a.slice, "between")
        .iter()
        .flat_map(|c| c["errorsAfterCommit"].as_array().unwrap().iter())
        .collect();
    assert!(!lines_of(&all).contains(&line_of("S31E")), "{}", a.raw);
}

#[test]
fn do1_twin_commit_in_a_tail_then_an_error() {
    let a = ask(ws(), &r("Do1"), &at("DO1A"), &at("DO1B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let c = rows(&a.slice, "between")[0];
    assert_eq!(c["part"], "A-tail", "{}", a.raw);
    let after: Vec<&Value> = c["errorsAfterCommit"].as_array().unwrap().iter().collect();
    assert!(lines_of(&after).contains(&line_of("DO1E")), "{}", a.raw);
}

#[test]
fn do2_twin_commits_inside_a_then_a_modify() {
    let a = ask(ws(), &r("Do2"), &at("DO2A"), &at("DO2B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    let inside = rows(&a.slice, "commitsInsideA");
    let classes: Vec<u64> = inside.iter().filter_map(|c| c["class"].as_u64()).collect();
    assert!(classes.contains(&1) && classes.contains(&2), "{}", a.raw);
    let c1 = inside.iter().find(|c| c["class"] == 1).unwrap();
    let after: Vec<&Value> = c1["errorsAfterCommit"].as_array().unwrap().iter().collect();
    assert!(lines_of(&after).contains(&line_of("DO2M")), "{}", a.raw);
    assert!(
        lines_of(&rows(&a.slice, "errorsBetween")).contains(&line_of("DO2M")),
        "{}",
        a.raw
    );
    // Named deep, inside the checked run: the run's commit makes A durable.
    let d = ask(ws(), &r("Do2"), &at("DO2DEEP"), &at("DO2B"));
    assert_eq!(d.answer, "yes", "{}", d.raw);
    let c = rows(&d.slice, "between")
        .into_iter()
        .find(|c| c["kind"] == "run-commit-of-A")
        .unwrap_or_else(|| panic!("{}", d.raw));
    let after: Vec<&Value> = c["errorsAfterCommit"].as_array().unwrap().iter().collect();
    assert!(lines_of(&after).contains(&line_of("DO2M")), "{}", d.raw);
}

#[test]
fn may_commit_contract_every_listed_commit_is_a_may_commit_site() {
    // §5.6 on the fixture: every commit row's place is among may-commit's
    // evidence places or inside its cone (a row's route starts at the root).
    for (routine, a, b) in [
        ("S1", "S1A", "S1B"),
        ("S7", "S7A", "S7B"),
        ("S16", "S16A", "S16B"),
    ] {
        let x = ask(ws(), &r(routine), &at(a), &at(b));
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args([
                "prove",
                ws().to_str().unwrap(),
                &r(routine),
                "may-commit",
                "--single-app",
            ])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let places: Vec<(String, u64)> = v["payload"]["result"]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "COMMIT")
            .filter_map(|e| {
                Some((
                    e["evidence"]["file"].as_str()?.to_string(),
                    e["evidence"]["line"].as_u64()?,
                ))
            })
            .collect();
        for c in rows(&x.slice, "between") {
            let p = (
                c["file"].as_str().unwrap().to_string(),
                c["line"].as_u64().unwrap(),
            );
            assert!(places.contains(&p), "{routine}: {p:?} not in {places:?}");
        }
    }
}
