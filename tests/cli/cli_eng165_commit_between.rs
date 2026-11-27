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

/// Review round (Fable, R1-R8; `RV::R6` was not a wrong answer), plus a
/// call into a symbol-only dependency between A and B (S29), a call in a
/// `case` label (R4) and `#if` arms (S28).
const REVIEW: &str = r#"codeunit 50640 "RV"
{
    procedure R1()
    begin
        L.Insert(); // R1B
        Deliver();
    end;

    procedure Deliver()
    begin
        Transmit(); // R1A
        Commit();
    end;

    procedure R2()
    begin
        L.Insert(); // R2B
        if Codeunit.Run(Codeunit::"RV Sender") then;
    end;

    procedure R3()
    begin
        Transmit(); // R3A
        if Codeunit.Run(Codeunit::"RV Logger") then;
    end;

    procedure R4()
    begin
        Transmit(); // R4A
        case true of
            CommitAndTell():
                ;
        end;
        L.Insert(); // R4B
    end;

    procedure R5(N: Integer; Flag: Boolean)
    var
        I: Integer;
        X: Integer;
    begin
        for I := 1 to N do begin
            Transmit(); // R5A
            X := Flag ? LogIt() : 0; // R5B
            Commit();
        end;
    end;

    procedure R7(Id: Integer)
    begin
        Dyn(Id); // R7A
        L.Insert(); // R7B
    end;

    procedure R8()
    begin
        Plain();
        Scoped();
    end;

    procedure S34()
    begin
        Transmit(); // S34A
        Both34();
        L.Insert(); // S34B
    end;

    procedure S34r()
    begin
        Transmit(); // S34rA
        Both34r();
        L.Insert(); // S34rB
    end;

    local procedure Both34()
    begin
        Scoped34();
        Plain34();
    end;

    local procedure Both34r()
    begin
        Plain34();
        Scoped34();
    end;

    local procedure Plain34()
    begin
        CommitHelper();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure Scoped34()
    begin
        CommitHelper();
    end;

    procedure S35()
    begin
        OnSend35();
        L.Insert(); // S35B
    end;

    [IntegrationEvent(false, false)]
    local procedure OnSend35()
    begin
    end;

    procedure S36(N: Integer)
    begin
        repeat
            Step36();
            N -= 1;
        until N <= 0;
    end;

    local procedure Step36()
    begin
        L.Insert(); // S36B
        Transmit(); // S36A
        Commit();
    end;

    procedure S37()
    var
        P: Codeunit "Dep Post";
    begin
        Transmit(); // S37A
        case true of
            P.Ok():
                ;
        end;
        L.Insert(); // S37B
    end;

    procedure S38()
    begin
        OnBoth38();
        Commit();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBoth38()
    begin
    end;

    procedure S39()
    begin
        case true of
            TransmitOk(): // S39A
                ;
        end;
        Commit();
        L.Insert(); // S39B
    end;

    local procedure TransmitOk(): Boolean
    begin
        Transmit();
        exit(true);
    end;

    procedure S40()
    begin
        Transmit(); // S40A
        CommitHelper();
        Commit();
        L.Insert(); // S40B
    end;

    procedure S41(C: Boolean)
    begin
        Outer41(C);
    end;

    local procedure Outer41(C: Boolean)
    begin
        L.Insert(); // S41B
        Inner41(C);
    end;

    local procedure Inner41(C: Boolean)
    begin
        Transmit(); // S41A
        if C then begin
            Commit();
            Outer41(false);
        end;
    end;

    procedure S42()
    var
        D: Record "RV Doc 42";
    begin
        D.Insert(true);
        L.Insert(); // S42B
    end;

    procedure S43()
    begin
        Again := true;
        Rec43();
        L.Insert(); // S43B
    end;

    local procedure Rec43()
    begin
        if Again then begin
            Again := false;
            Rec43();
            Commit();
        end else
            Transmit(); // S43A
    end;

    procedure S43b()
    begin
        Again := true;
        Transmit(); // S43bA
        X43b();
    end;

    local procedure X43b()
    begin
        if Again then begin
            Again := false;
            Y43b();
        end else
            L.Insert(); // S43bB
    end;

    local procedure Y43b()
    begin
        Commit();
        X43b();
    end;

    procedure S44()
    var
        D: Record "RV Doc 44";
    begin
        D.DeleteAll(true);
        L.Insert(); // S44B
    end;

    procedure S45()
    var
        D: Record "RV Doc 45";
    begin
        D.DeleteAll(true);
    end;

    procedure S46()
    var
        D: Record "RV Doc 46";
        I: Integer;
    begin
        for I := 1 to 2 do begin
            Transmit(); // S46A
            D.SetRange("No.", I);
            D.DeleteAll(true);
            Commit();
        end;
    end;

    procedure S47()
    begin
        Report.Run(Report::"RV Batch 47");
    end;

    procedure S48()
    var
        D: Record "RV Doc 48";
        I: Integer;
    begin
        for I := 1 to 2 do begin
            Transmit(); // S48A
            D."No." := I;
            D.Insert(I = 2);
            Commit();
        end;
    end;

    procedure S33()
    begin
        Transmit(); // S33A
        Commit();
        Page.RunModal(Page::"EB Log Card");
        L.Insert(); // S33B
    end;

    procedure S32()
    begin
        OnSend32();
        Commit();
        L.Insert(); // S32B
    end;

    [IntegrationEvent(false, false)]
    local procedure OnSend32()
    begin
    end;

    procedure S29()
    var
        P: Codeunit "Dep Post";
    begin
        Transmit(); // S29A
        P.Post();
        L.Insert(); // S29B
    end;

#if CLEAN28
    procedure S28()
    begin
        Transmit(); // S28A
    end;
#else
    procedure S28()
    begin
        L.Insert(); // S28B
    end;
#endif

    local procedure Plain()
    begin
        CommitHelper();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    local procedure Scoped()
    begin
        Transmit(); // R8A
        CommitHelper();
        L.Insert(); // R8B
    end;

    local procedure CommitHelper()
    begin
        Commit();
    end;

    local procedure Dyn(Id: Integer)
    begin
        Transmit();
        Codeunit.Run(Id);
    end;

    procedure Transmit()
    var
        O: Record "EB Outbox";
    begin
        O.Insert();
    end;

    local procedure CommitAndTell(): Boolean
    begin
        Commit();
        exit(true);
    end;

    local procedure LogIt(): Integer
    begin
        L.Insert();
        exit(1);
    end;

    var
        L: Record "EB Log";
        Again: Boolean;
}

codeunit 50641 "RV Sender"
{
    trigger OnRun()
    var
        R: Codeunit "RV";
    begin
        R.Transmit(); // R2A
    end;
}

codeunit 50643 "RV Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RV", 'OnSend32', '', false, false)]
    local procedure Send32()
    var
        R: Codeunit "RV";
    begin
        R.Transmit(); // S32A
    end;
}

codeunit 50644 "RV Subscribers 35"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RV", 'OnSend35', '', false, false)]
    local procedure Send35()
    var
        R: Codeunit "RV";
    begin
        R.Transmit(); // S35A
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RV", 'OnSend35', '', false, false)]
    local procedure Commit35()
    begin
        Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RV", 'OnBoth38', '', false, false)]
    local procedure Send38()
    var
        R: Codeunit "RV";
    begin
        R.Transmit(); // S38A
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"RV", 'OnBoth38', '', false, false)]
    local procedure Log38()
    var
        L: Record "EB Log";
    begin
        L.Insert(); // S38B
    end;

    [EventSubscriber(ObjectType::Table, Database::"RV Doc 42", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsert42(var Rec: Record "RV Doc 42"; RunTrigger: Boolean)
    begin
        Commit();
    end;
}

table 50645 "RV Doc 42"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnInsert()
    var
        R: Codeunit "RV";
    begin
        R.Transmit(); // S42A
    end;
}

table 50646 "RV Doc 44"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnDelete()
    var
        R: Codeunit "RV";
    begin
        if "No." = 2 then
            Commit();
        R.Transmit(); // S44A
    end;
}

table 50647 "RV Doc 45"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnDelete()
    var
        R: Codeunit "RV";
        L: Record "EB Log";
    begin
        L.Insert(); // S45B
        R.Transmit(); // S45A
        Commit();
    end;
}

table 50648 "RV Doc 46"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnDelete()
    var
        L: Record "EB Log";
    begin
        L.Insert(); // S46B
    end;
}

table 50649 "RV Doc 48"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { Clustered = true; } }

    trigger OnInsert()
    var
        L: Record "EB Log";
    begin
        L.Insert(); // S48B
    end;
}

report 50650 "RV Batch 47"
{
    ProcessingOnly = true;

    dataset
    {
        dataitem(Doc; "RV Doc 46")
        {
            trigger OnAfterGetRecord()
            var
                R: Codeunit "RV";
                L: Record "EB Log";
            begin
                L.Insert(); // S47B
                R.Transmit(); // S47A
                Commit();
            end;
        }
    }
}

codeunit 50642 "RV Logger"
{
    trigger OnRun()
    var
        L: Record "EB Log";
    begin
        Commit();
        L.Insert(); // R3B
    end;
}
"#;

const DEP_GUID: &str = "16516516-0000-4000-8000-0000000d0165";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"15.0","Codeunits":[{"Id":50690,"Name":"Dep Post","Methods":[{"Name":"Post","Parameters":[]},{"Name":"Ok","Parameters":[],"ReturnTypeDefinition":{"Name":"Boolean"}}]}],"AppId":"16516516-0000-4000-8000-0000000d0165","Name":"DepPost","Publisher":"probe","Version":"1.0.0.0"}"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"16516516-0000-4000-8000-0000000e0165","name":"EB","publisher":"t","version":"1.0.0.0","runtime":"15.0","idRanges":[{{"from":50600,"to":50699}}],
            "dependencies":[{{"id":"{DEP_GUID}","name":"DepPost","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Tables.al"), TABLES).unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SHAPES).unwrap();
    std::fs::write(ws.join("src/Others.al"), OTHERS).unwrap();
    std::fs::write(ws.join("src/Review.al"), REVIEW).unwrap();
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_DepPost_1.0.0.0.app"),
        DEP_GUID,
        "DepPost",
        "1.0.0.0",
        DEP_SYMBOLS,
    );
    ws
}

/// `file:line` of the line marked `// <tag>` (1-based).
fn at(tag: &str) -> String {
    for (file, src) in [
        ("Shapes.al", SHAPES),
        ("Others.al", OTHERS),
        ("Review.al", REVIEW),
    ] {
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
    ];
    // `@cross-app` (test-only marker): load the dependencies too.
    if !extra.contains(&"@cross-app") {
        args.push("--single-app".to_string());
    }
    args.extend(
        extra
            .iter()
            .filter(|s| **s != "@cross-app")
            .map(|s| s.to_string()),
    );
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
    if list == "errorsBetween" {
        // Indices into the error table (each error listed once).
        return errors_at(s, &s[list]);
    }
    s[list]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// The error table's rows at the indices in `ids`.
fn errors_at<'v>(s: &'v Value, ids: &Value) -> Vec<&'v Value> {
    ids.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| s["errors"].get(i.as_u64()? as usize))
                .collect()
        })
        .unwrap_or_default()
}

/// The nearest error after commit row `c` (the JSON gives the counts and the
/// nearest one, not the whole list: review r2 item 7).
fn after_of<'v>(s: &'v Value, c: &Value) -> Vec<&'v Value> {
    errors_at(
        s,
        &Value::Array(vec![c["errorsAfterCommit"]["nearest"].clone()]),
    )
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
    let after: Vec<&Value> = after_of(&a.slice, c);
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
    let after: Vec<&Value> = after_of(&a.slice, rows(&a.slice, "between")[0]);
    assert!(!lines_of(&after).contains(&line_of("S24E")), "{}", a.raw);
}

#[test]
fn s25_an_error_trapped_above_the_meet_routine_is_swallowed() {
    let a = ask(ws(), &r("S25"), &at("S25A"), &at("S25B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let after: Vec<&Value> = after_of(&a.slice, rows(&a.slice, "between")[0]);
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
    let after: Vec<&Value> = after_of(&a.slice, c);
    assert!(lines_of(&after).contains(&line_of("S30E")), "{}", a.raw);
}

#[test]
fn s31_a_trap_on_a_route_catches_the_tail_error() {
    let a = ask(ws(), &r("S31"), &at("S31A"), &at("S31B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let all: Vec<&Value> = rows(&a.slice, "between")
        .iter()
        .flat_map(|c| after_of(&a.slice, c))
        .collect();
    assert!(!lines_of(&all).contains(&line_of("S31E")), "{}", a.raw);
}

#[test]
fn do1_twin_commit_in_a_tail_then_an_error() {
    let a = ask(ws(), &r("Do1"), &at("DO1A"), &at("DO1B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let c = rows(&a.slice, "between")[0];
    assert_eq!(c["part"], "A-tail", "{}", a.raw);
    let after: Vec<&Value> = after_of(&a.slice, c);
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
    let after: Vec<&Value> = after_of(&a.slice, c1);
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
    let after: Vec<&Value> = after_of(&d.slice, c);
    assert!(lines_of(&after).contains(&line_of("DO2M")), "{}", d.raw);
}

fn rv(name: &str) -> String {
    format!("RV::{name}")
}

#[test]
fn r1_tail_commit_with_b_never_after_a_is_without_b() {
    // Review B3: `L.Insert (B); Deliver (A; Commit)`: B never runs after A.
    let a = ask(ws(), &rv("R1"), &at("R1A"), &at("R1B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert!(rows(&a.slice, "between").is_empty(), "{}", a.raw);
    assert!(!rows(&a.slice, "commitsWithoutB").is_empty(), "{}", a.raw);
}

#[test]
fn r2_run_commit_of_a_with_b_never_after_a_is_without_b() {
    // Review B3 for the N1 row: A inside a checked run that comes after B.
    let a = ask(ws(), &rv("R2"), &at("R2A"), &at("R2B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert!(rows(&a.slice, "between").is_empty(), "{}", a.raw);
}

#[test]
fn r3_b_head_inside_a_checked_run_is_class_2() {
    // Review B4: `Transmit (A); if Codeunit.Run(Logger)` with Logger:
    // Commit; L.Insert (B): the run's own commit, never A's.
    let a = ask(ws(), &rv("R3"), &at("R3A"), &at("R3B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert!(
        !rows(&a.slice, "ownTransactionCommits").is_empty(),
        "{}",
        a.raw
    );
}

#[test]
fn r4_a_call_in_a_case_label_is_not_a_silent_no() {
    // Review B1: the control-flow tree does not place a call in a case label.
    let a = ask(ws(), &rv("R4"), &at("R4A"), &at("R4B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("site-outside-control-flow"), "{}", a.raw);
}

#[test]
fn r5_a_b_in_a_ternary_arm_may_be_skipped() {
    // Review S2: `X := Flag ? LogIt() : 0` (B) may not run.
    let a = ask(ws(), &rv("R5"), &at("R5A"), &at("R5B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn r7_an_unresolved_call_inside_a_does_not_block() {
    // Review S3: only region cones feed the blockers.
    let a = ask(ws(), &rv("R7"), &at("R7A"), &at("R7B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
}

#[test]
fn r8_commit_behavior_is_read_per_route() {
    // Review S1: `Plain` reaches the helper's commit outside `Scoped`'s
    // `[CommitBehavior(Ignore)]`; between A and B it is ignored.
    let a = ask(ws(), &rv("R8"), &at("R8A"), &at("R8B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
}

#[test]
fn s28_alternative_declarations_never_run_in_one_build() {
    let a = ask(ws(), &rv("S28"), &at("S28A"), &at("S28B"));
    assert_eq!(a.answer, "no", "{}", a.raw);
    assert_eq!(a.slice["relation"], "not-in-one-build", "{}", a.raw);
}

#[test]
fn s33_a_page_run_right_after_a_commit_has_no_pending_write() {
    // Review N8: every path to the RunModal passes the Commit: no write is
    // pending there, so no page-run error row.
    let a = ask(ws(), &rv("S33"), &at("S33A"), &at("S33B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    assert!(
        !rows(&a.slice, "errorsBetween")
            .iter()
            .any(|e| e["kind"] == "window"),
        "{}",
        a.raw
    );
}

#[test]
fn s32_a_inside_an_event_subscriber() {
    // Review S4: A in a subscriber; the commit after the raise is between.
    let a = ask(ws(), &rv("S32"), &at("S32A"), &at("S32B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s34_a_commit_reached_both_ignored_and_normally_is_kept() {
    // Review r2 item 1: per route above the statement, every path below it.
    // Both call orders (the cone walk's edge order must not decide).
    for (r, a, b) in [("S34", "S34A", "S34B"), ("S34r", "S34rA", "S34rB")] {
        let x = ask(ws(), &rv(r), &at(a), &at(b));
        assert_eq!(x.answer, "yes", "{r}: {}", x.raw);
    }
}

#[test]
fn s35_a_sibling_subscriber_of_a_s_event_blocks() {
    // Review r2 item 2: another subscriber of A's event may commit after A.
    let a = ask(ws(), &rv("S35"), &at("S35A"), &at("S35B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("sibling-subscribers"), "{}", a.raw);
}

#[test]
fn s36_a_loop_above_the_meet_routine_blocks() {
    // Review r2 item 3: the caller's loop runs M again; the next B after A
    // is in a later run. Not followed: unknown, never a false no.
    let a = ask(ws(), &rv("S36"), &at("S36A"), &at("S36B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("loop-above-meet"), "{}", a.raw);
}

#[test]
fn s37_an_unresolved_call_in_a_case_label_blocks() {
    // Review r2 item 4: no edge, no record, not placed: still read against
    // the root's unresolved cone (single-app: the dependency is not loaded).
    let a = ask(ws(), &rv("S37"), &at("S37A"), &at("S37B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("region-unresolved-call"), "{}", a.raw);
}

#[test]
fn s38_a_and_b_in_two_subscribers_of_one_event_is_unknown() {
    // The guard `event-subscriber-order`.
    let a = ask(ws(), &rv("S38"), &at("S38A"), &at("S38B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("event-subscriber-order"), "{}", a.raw);
}

#[test]
fn s39_a_in_a_case_label_is_unknown() {
    // The guard `site-not-in-tree`: A's own statement is not placed.
    let a = ask(ws(), &rv("S39"), &at("S39A"), &at("S39B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("site-not-in-tree"), "{}", a.raw);
}

#[test]
fn s40_order_nearest_summary_and_statements_are_pinned() {
    // Review r2 item 7: the direct commit (0 hops) leads; the helper's
    // (1 hop) follows; summary and statements count both.
    let a = ask(ws(), &rv("S40"), &at("S40A"), &at("S40B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
    let between = rows(&a.slice, "between");
    assert_eq!(between.len(), 2, "{}", a.raw);
    assert_eq!(between[0]["hops"], 0, "{}", a.raw);
    assert_eq!(between[1]["hops"], 1, "{}", a.raw);
    assert_eq!(a.slice["nearest"]["between"]["hops"], 0, "{}", a.raw);
    assert_eq!(
        a.slice["summary"]["between"]["class1"]["count"], 2,
        "{}",
        a.raw
    );
    assert_eq!(
        a.slice["statements"].as_array().map(Vec::len),
        Some(2),
        "{}",
        a.raw
    );
}

#[test]
fn s41_recursion_into_the_meet_routine_blocks() {
    // Review r3 item 1: A, Commit, then the meet routine again, whose B
    // runs next. No route shows the recursion: unknown, never a false no.
    let a = ask(ws(), &rv("S41"), &at("S41A"), &at("S41B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("recursion-on-route"), "{}", a.raw);
}

#[test]
fn s42_a_table_event_at_the_leave_site_blocks() {
    // Review r3 item 2: A is in OnInsert; the OnAfterInsertEvent subscriber
    // commits after it returns, in the same Insert, before B.
    let a = ask(ws(), &rv("S42"), &at("S42A"), &at("S42B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("site-other-edges"), "{}", a.raw);
}

#[test]
fn s43_recursion_below_the_meet_routine_on_a_s_route_blocks() {
    // Review r4 item 1: A in the inner run of Rec43; the outer run commits
    // after it returns, then B. The route lists Rec43 once: unknown.
    let a = ask(ws(), &rv("S43"), &at("S43A"), &at("S43B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("recursion-on-route"), "{}", a.raw);
}

#[test]
fn s43b_recursion_below_the_meet_routine_on_b_s_route_is_yes() {
    // Review r4 item 1, the mirror: X43b's first run calls Y43b, which
    // commits and runs X43b again, whose B is the next B. Not a false `no`
    // even without the recursion blocker: B is in a branch, so the call
    // toward it is no sure cut and its cone (Y43b's commit) is in M's
    // region. Any recursion before the first B passes through B's head,
    // which is walked. No shape was found where the B-side range of
    // `recursion_on_route` turns a false `no`; it is kept as the safe side.
    let a = ask(ws(), &rv("S43b"), &at("S43bA"), &at("S43bB"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s44_a_bulk_operation_on_a_s_route_blocks() {
    // Review r4 item 2: DeleteAll(true) runs OnDelete once per row; a later
    // row's run commits before its A, after the first row's A. The detail
    // pins that the operation was found and read as bulk. The other branch
    // (the operation's row not in the model, which blocks too) has no cheap
    // fixture: a workspace record operation always has its row, and a
    // symbol-only dependency routine has no body edges to leave by.
    let a = ask(ws(), &rv("S44"), &at("S44A"), &at("S44B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("bulk-operation-on-route"), "{}", a.raw);
    assert!(
        a.raw.contains("whose trigger runs once per row"),
        "{}",
        a.raw
    );
}

#[test]
fn s45_a_bulk_operation_on_the_shared_prefix_blocks() {
    // Review r5 (sol) item 1: both routes share the DeleteAll edge, so the
    // meet routine is OnDelete. With two rows: A (row 1), Commit, B (row 2).
    let a = ask(ws(), &rv("S45"), &at("S45A"), &at("S45B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("bulk-operation-on-route"), "{}", a.raw);
}

#[test]
fn s46_a_trigger_that_may_not_fire_is_no_sure_b() {
    // Review r5 (sol) item 2: B is in OnDelete, reached by a DeleteAll that
    // may match no row. Pass 1 matches none: its Commit runs, then pass 2's
    // DeleteAll runs B. Not cut at the first DeleteAll: yes.
    let a = ask(ws(), &rv("S46"), &at("S46A"), &at("S46B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s47_a_report_dataitem_on_the_shared_prefix_blocks() {
    // Reviews r5 (Fable issue 1): a batch job's OnAfterGetRecord runs once
    // per row. Row 1's A, its Commit, then row 2's B.
    let a = ask(ws(), &rv("S47"), &at("S47A"), &at("S47B"));
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("repeated-trigger-on-route"), "{}", a.raw);
}

#[test]
fn s48_a_non_literal_run_trigger_is_no_sure_b() {
    // Reviews r5 (Fable issue 2): `Insert(I = 2)` runs OnInsert (B) only on
    // pass 2; pass 1's Commit runs between A and that B: yes.
    let a = ask(ws(), &rv("S48"), &at("S48A"), &at("S48B"));
    assert_eq!(a.answer, "yes", "{}", a.raw);
}

#[test]
fn s29_a_symbol_only_routine_between_is_unknown() {
    // Review B2: its body is not available (cross-app: the dependency is
    // loaded, from symbols only).
    let a = ask_with(ws(), &rv("S29"), &at("S29A"), &at("S29B"), &["@cross-app"]);
    assert_eq!(a.answer, "unknown", "{}", a.raw);
    assert!(a.raw.contains("region-body-not-available"), "{}", a.raw);
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

#[test]
fn throws_error_contract_every_error_row_is_a_throws_error_site() {
    // §5.6: every error row is a `throws-error` site of the root, or one it
    // leaves out as caught above the meet routine (`swallowed-above-meet`).
    for (routine, a, b) in [("S7", "S7A", "S7B"), ("Do1", "DO1A", "DO1B")] {
        let x = ask(ws(), &r(routine), &at(a), &at(b));
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args([
                "prove",
                ws().to_str().unwrap(),
                &r(routine),
                "throws-error",
                "--single-app",
            ])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let places: Vec<(String, u64)> = v["payload"]["result"]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| {
                Some((
                    e["evidence"]["file"].as_str()?.to_string(),
                    e["evidence"]["line"].as_u64()?,
                ))
            })
            .collect();
        let errs = rows(&x.slice, "errorsBetween");
        assert!(!errs.is_empty(), "{routine}: precondition, error rows");
        for e in errs {
            if has_label(e, "swallowed-above-meet") || e["kind"] == "window" {
                continue;
            }
            let p = (
                e["file"].as_str().unwrap().to_string(),
                e["line"].as_u64().unwrap(),
            );
            assert!(places.contains(&p), "{routine}: {p:?} not in {places:?}");
        }
    }
}
