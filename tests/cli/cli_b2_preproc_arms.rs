//! Engine limits B2 (`docs/engine-limits.md` §1): the arms of a statement-position
//! `#if` were run one after another. An `exit` or `Error` in one arm then cut off
//! the code after the group in EVERY configuration, so `prove` answered a wrong
//! `no (every candidate decided)` and a wrong `unconditional on success`. Each case
//! is pinned through the CLI answer that printed it.
//!
//! The workspace states its precondition literally: `app.json` defines `CLEAN99`,
//! and the engine chooses no configuration (a later L22 step), so every arm is a
//! build that may ship. Each routine's source says which arm a build takes.

use std::process::Command;

use serde_json::Value;

const SOURCE: &str = r#"table 50171 "B2 Log B"
{
    fields { field(1; "No."; Integer) { } }
}

table 50172 "B2 Log A"
{
    fields { field(1; "No."; Integer) { } }
}

codeunit 50170 "B2 Preproc"
{
    // A build without CLEAN99 exits; a build with it runs the write.
    procedure IfExit()
    var
        LogB: Record "B2 Log B";
    begin
#if not CLEAN99
        exit;
#endif
        LogB.Insert();
    end;

    // The error exists only in a build without CLEAN99.
    procedure IfError()
    begin
#if not CLEAN99
        Error('old build only');
#endif
    end;

    // A build without CLEAN99 returns early when b holds: no commit then.
    procedure ArmMayExitCommit(b: Boolean)
    begin
#if not CLEAN99
        if b then exit;
#endif
        Commit();
    end;

    // Nested groups: a build without CLEAN99 that defines B99 exits first.
    procedure NestedArmExit()
    var
        LogB: Record "B2 Log B";
    begin
#if not CLEAN99
#if B99
        exit;
#endif
        LogB.Modify();
#endif
        LogB.Insert();
    end;

    // Plain AL, the same shape: a and b both true return before the Insert.
    procedure NestedIfExit(a: Boolean; b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        if a then begin
            if b then exit;
            LogB.Modify();
        end;
        LogB.Insert();
    end;

    // An exit inside a loop leaves the routine, not just the loop.
    procedure LoopExit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        while b do begin
            if b then exit;
        end;
        LogB.Insert();
    end;

    // The arm always ends (exit or Error), and c and d both true return
    // successfully before the Insert.
    procedure ExitOrError(c: Boolean; d: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        if c then begin
            if d then exit;
            Error('c without d');
        end;
        LogB.Insert();
    end;

    // A bare `begin … end` that may exit: b true returns before the Insert.
    procedure BareBlockExit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        begin
            if b then exit;
        end;
        LogB.Insert();
    end;

    // A bare `begin … end` that always exits: the Insert never runs.
    procedure AlwaysExitBlock()
    var
        LogB: Record "B2 Log B";
    begin
        begin
            exit;
        end;
        LogB.Insert();
    end;

    // The control: nothing returns early, so the Insert is on every success
    // path.
    procedure NoExit(c: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        if c then
            Error('c');
        LogB.Insert();
    end;

    // One routine, declared in two arms: a build runs one body or the other.
#if CLEAN99
    procedure TwoArms()
    var
        LogA: Record "B2 Log A";
    begin
        LogA.Insert();
    end;
#else
    procedure TwoArms()
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
    end;
#endif

    procedure CallsTwoArms()
    begin
        TwoArms();
    end;
}

// The ordering facts: a COMMIT that dominates the root's return. Each routine
// writes, may return early (or not, the controls), then commits.
codeunit 50173 "B2 Order"
{
    procedure Control()
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
        Commit();
    end;

    procedure ArmMayExitCommit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
#if not CLEAN99
        if b then exit;
#endif
        Commit();
    end;

    procedure NestedGroupCommit()
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
#if not CLEAN99
#if B99
        exit;
#endif
        LogB.Modify();
#endif
        Commit();
    end;

    procedure NestedIfCommit(a: Boolean; b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
        if a then begin
            if b then exit;
        end;
        Commit();
    end;

    procedure LoopExitCommit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
        while b do
            exit;
        Commit();
    end;

    procedure BareBlockCommit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
        begin
            if b then exit;
        end;
        Commit();
    end;

    procedure DoCommit()
    begin
        Commit();
    end;

    procedure CallsCommitControl()
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
        DoCommit();
    end;

    procedure CallsCommitAfterArmExit(b: Boolean)
    var
        LogB: Record "B2 Log B";
    begin
        LogB.Insert();
#if not CLEAN99
        if b then exit;
#endif
        DoCommit();
    end;
}
"#;

/// Engine limits B2, second review: attributes and properties that only some
/// builds have. Each `Arm…`/`…Maybe` object has its declaration inside an
/// `#if CLEAN99` arm the routine (or the object's other members) is not in, so
/// a build without `CLEAN99` (a possible build: the engine chooses none) does
/// not have it. Each has a control that declares the same thing in every build.
const DECL_SOURCE: &str = r#"table 50175 "B2 Temp Maybe"
{
#if CLEAN99
    TableType = Temporary;
#endif
    fields { field(1; "No."; Integer) { } }
}

table 50176 "B2 Temp Always"
{
    TableType = Temporary;
    fields { field(1; "No."; Integer) { } }
}

table 50179 "B2 Xml T"
{
    fields { field(1; "No."; Integer) { } }
}

codeunit 50177 "B2 Decl"
{
#if CLEAN99
    [CommitBehavior(CommitBehavior::Ignore)]
#endif
    procedure ArmIgnore()
    begin
        Commit();
    end;

    [CommitBehavior(CommitBehavior::Ignore)]
    procedure AlwaysIgnore()
    begin
        Commit();
    end;

#if CLEAN99
    [CommitBehavior(CommitBehavior::Error)]
#endif
    procedure ArmError()
    begin
        Commit();
    end;

    procedure CallsArmIgnore()
    begin
        ArmIgnore();
    end;

    // A build with CLEAN99 attaches the attribute to TakesItInArm, a build
    // without it to TakesLeftover.
    [CommitBehavior(CommitBehavior::Ignore)]
#if CLEAN99
    procedure TakesItInArm()
    begin
        Commit();
    end;
#endif
    procedure TakesLeftover()
    begin
        Commit();
    end;

    procedure IoThenArmIgnore()
    var
        C: HttpClient;
        Resp: HttpResponseMessage;
    begin
        C.Get('https://example.com', Resp);
        ArmIgnore();
    end;


#if CLEAN99
    [TryFunction]
#endif
    procedure ArmTry()
    begin
        Error('fails');
    end;

    [TryFunction]
    procedure AlwaysTry()
    begin
        Error('fails');
    end;

    procedure TestsArmTry()
    begin
        if ArmTry() then;
    end;

    procedure TestsAlwaysTry()
    begin
        if AlwaysTry() then;
    end;

    procedure WritesTempMaybe()
    var
        R: Record "B2 Temp Maybe";
    begin
        R.Insert();
    end;

    procedure WritesTempAlways()
    var
        R: Record "B2 Temp Always";
    begin
        R.Insert();
    end;

    procedure RunsArmExport()
    begin
        Xmlport.Run(Xmlport::"B2 Arm Export");
    end;

    procedure RunsExport()
    begin
        Xmlport.Run(Xmlport::"B2 Export");
    end;
}

codeunit 50192 "B2 ICB"
{
#if CLEAN99
    InherentCommitBehavior = Ignore;
#else
    InherentCommitBehavior = Error;
#endif

    procedure Commits()
    begin
        Commit();
    end;
}

xmlport 50180 "B2 Arm Export"
{
#if CLEAN99
    Direction = Export;
#endif
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "B2 Xml T")
            {
                fieldelement(No; Row."No.") { }
            }
        }
    }
}

xmlport 50181 "B2 Export"
{
    Direction = Export;
    schema
    {
        textelement(Root)
        {
            tableelement(Row; "B2 Xml T")
            {
                fieldelement(No; Row."No.") { }
            }
        }
    }
}

table 50187 "B2 Sub Log"
{
    fields { field(1; "No."; Integer) { } }
}

// Runs for a database record only: a temporary page `Rec` skips it.
codeunit 50186 "B2 Xml Sub"
{
    [EventSubscriber(ObjectType::Table, Database::"B2 Xml T", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnInsert(var Rec: Record "B2 Xml T"; RunTrigger: Boolean)
    var
        Log: Record "B2 Sub Log";
    begin
        if Rec.IsTemporary then
            exit;
        Log.Insert();
    end;
}

page 50182 "B2 Page Maybe"
{
    SourceTable = "B2 Xml T";
#if CLEAN99
    SourceTableTemporary = true;
#endif

    procedure InsertRec()
    begin
        Rec.Insert();
    end;
}

page 50183 "B2 Page Temp"
{
    SourceTable = "B2 Xml T";
    SourceTableTemporary = true;

    procedure InsertRec()
    begin
        Rec.Insert();
    end;
}

report 50184 "B2 Rep Maybe"
{
    dataset
    {
        dataitem(Item; "B2 Xml T")
        {
#if CLEAN99
            UseTemporary = true;
#endif
        }
    }

    procedure InsertItem()
    begin
        Item.Insert();
    end;
}

report 50185 "B2 Rep Temp"
{
    dataset
    {
        dataitem(Item; "B2 Xml T")
        {
            UseTemporary = true;
        }
    }

    procedure InsertItem()
    begin
        Item.Insert();
    end;
}
"#;

/// Third review of B2: one routine declared in two `#if` arms, an attribute
/// in one arm only; an `#if`-only page permission; an event isolated in some
/// builds only. Each has a control written in every build.
const ARMS_SOURCE: &str = r#"codeunit 50193 "B2 Split"
{
    // A split header: a build without CLEAN99 takes the attribute.
#if CLEAN99
    procedure ElseIgn(a: Integer)
#else
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure ElseIgn(a: Integer)
#endif
    begin
        Commit();
    end;

    // The same attribute in both arms: every build has it.
#if CLEAN99
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure BothIgn(a: Integer)
#else
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure BothIgn(a: Integer)
#endif
    begin
        Commit();
    end;

    // Two whole declarations; only the CLEAN99 one ignores its commit.
#if CLEAN99
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure TwoBodies()
    begin
        Commit();
    end;
#else
    procedure TwoBodies()
    begin
        Commit();
    end;
#endif

    procedure CallTwo()
    begin
        TwoBodies();
    end;

#if CLEAN99
    [TryFunction]
    procedure SplitTry()
#else
    procedure SplitTry()
#endif
    begin
        Error('x');
    end;

    procedure CallTry()
    begin
        if SplitTry() then;
    end;

#if CLEAN99
    [TryFunction]
    procedure BothTry()
#else
    [TryFunction]
    procedure BothTry()
#endif
    begin
        Error('x');
    end;

    procedure CallBothTry()
    begin
        if BothTry() then;
    end;
}

table 50194 "B2 UI T"
{
    fields { field(1; "No."; Integer) { } }
    trigger OnDelete()
    begin
        Error('no delete');
    end;
}

page 50195 "B2 UI Armed"
{
    SourceTable = "B2 UI T";
#if CLEAN99
    DeleteAllowed = false;
#endif
}

page 50196 "B2 UI Closed"
{
    SourceTable = "B2 UI T";
    DeleteAllowed = false;
}

codeunit 50197 "B2 Iso Pub"
{
#if CLEAN99
    [IntegrationEvent(false, false, true)]
#else
    [IntegrationEvent(false, false, false)]
#endif
    procedure OnArmIso()
    begin
    end;

    [IntegrationEvent(false, false,
#if CLEAN99
    true
#else
    false
#endif
    )]
    procedure OnArgIso()
    begin
    end;

    [IntegrationEvent(false, false, true)]
    procedure OnIso()
    begin
    end;
}

codeunit 50198 "B2 Iso Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"B2 Iso Pub", 'OnArmIso', '', false, false)]
    local procedure SubArm()
    begin
        Error('arm');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"B2 Iso Pub", 'OnArgIso', '', false, false)]
    local procedure SubArg()
    begin
        Error('arg');
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"B2 Iso Pub", 'OnIso', '', false, false)]
    local procedure SubIso()
    begin
        Error('iso');
    end;
}

codeunit 50199 "B2 Iso Raise"
{
    procedure RaiseArm()
    var
        Pub: Codeunit "B2 Iso Pub";
    begin
        Pub.OnArmIso();
    end;

    procedure RaiseArg()
    var
        Pub: Codeunit "B2 Iso Pub";
    begin
        Pub.OnArgIso();
    end;

    procedure RaiseIso()
    var
        Pub: Codeunit "B2 Iso Pub";
    begin
        Pub.OnIso();
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b2b2b2b2-0000-4000-8000-0000000000b2","name":"B2","publisher":"t","version":"1.0.0.0","dependencies":[],"preprocessorSymbols":["CLEAN99"]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/b2.al"), SOURCE).unwrap();
    std::fs::write(t.path().join("src/b2decl.al"), DECL_SOURCE).unwrap();
    std::fs::write(t.path().join("src/b2arms.al"), ARMS_SOURCE).unwrap();
    t
}

/// `alsem digest <ws> --routine <routine> --single-app --detail` as JSON: the
/// one entry's effects.
fn digest_effects(ws: &str, routine: &str) -> Vec<Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "digest",
            ws,
            "--routine",
            routine,
            "--single-app",
            "--detail",
            "--compact=false",
        ])
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
    let entries = v["payload"]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "{v:#}");
    entries[0]["effects"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The scoped guarantee labels of a routine's digest (root-scoped ones only
/// when `root_only`), with the commit effectiveness each carries.
fn scoped_guarantees(ws: &str, routine: &str, root_only: bool) -> Vec<(String, Option<String>)> {
    digest_effects(ws, routine)
        .iter()
        .flat_map(|e| {
            e["scopedGuarantees"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|g| !root_only || g["scope"] == "root")
        .map(|g| {
            (
                g["label"].as_str().unwrap_or_default().to_string(),
                g["commitEffectiveness"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn prove(ws: &str, routine: &str, question: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove", ws, routine, question, "--single-app"])
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

/// An `exit` inside a `#if` arm with no `#else` ends that arm only: the write
/// after the group runs in the build that skips the arm. Before B2 the arm was
/// spliced into the routine's statements, the write was unreachable, and prove
/// said `no (every candidate decided)`.
#[test]
fn an_exit_in_one_arm_does_not_hide_the_code_after_the_group() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Preproc::IfExit", "writes-table:B2 Log B");
    assert_eq!(r["answer"], "yes", "{r:#}");
    let e = &r["evidence"][0];
    assert_eq!(e["type"], "DB_INSERT", "{r:#}");
    assert_eq!(
        e["evidence"]["line"],
        line_of("LogB.Insert();", "procedure IfExit()"),
        "{r:#}"
    );
    // Some build exits first: the write is not on every success path.
    assert_ne!(e["conditionality"], "unconditional-on-success", "{r:#}");
}

/// An `Error` that only one build compiles is no unconditional error of the
/// routine: before B2 throws-error said `unconditional on success`.
#[test]
fn an_error_in_one_arm_is_not_unconditional() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Preproc::IfError", "throws-error");
    assert_eq!(r["answer"], "yes", "{r:#}");
    let e = &r["evidence"][0];
    assert_eq!(e["type"], "ERROR_THROW", "{r:#}");
    assert_ne!(e["conditionality"], "unconditional-on-success", "{r:#}");
}

/// The `DB_INSERT` evidence of `routine`'s `writes-table:{table}`: asserts the
/// answer is `yes`, cites the `Insert` line after `procedure {routine}(`, and
/// returns its conditionality.
fn insert_conditionality(ws: &str, routine: &str, table: &str, insert: &str) -> String {
    let r = prove(
        ws,
        &format!("B2 Preproc::{routine}"),
        &format!("writes-table:{table}"),
    );
    assert_eq!(r["answer"], "yes", "{routine}: {r:#}");
    let e = r["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "DB_INSERT")
        .unwrap_or_else(|| panic!("{routine}: no DB_INSERT: {r:#}"));
    assert_eq!(
        e["evidence"]["line"],
        line_of(insert, &format!("procedure {routine}(")),
        "{routine}: {r:#}"
    );
    e["conditionality"].as_str().unwrap().to_string()
}

/// Review of B2: an arm that only MAY exit (`#if not CLEAN99 if b then exit;
/// #endif`) still keeps some successful runs from the code after the group.
/// Before the fix the group's `case` made the continuation conditional only for
/// an arm that ALWAYS exits, so `commits-on-success-path` said a wrong `yes`
/// (master, which spliced the arm, said `unknown`).
#[test]
fn an_arm_that_may_exit_keeps_the_commit_after_it_off_the_success_path() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(
        &ws,
        "B2 Preproc::ArmMayExitCommit",
        "commits-on-success-path",
    );
    assert_ne!(r["answer"], "yes", "{r:#}");
    assert_eq!(r["answer"], "unknown", "{r:#}");
    let e = &r["evidence"][0];
    assert_eq!(e["type"], "COMMIT", "{r:#}");
    assert_eq!(e["conditionality"], "conditional", "{r:#}");
}

/// Review of B2: the same gap for nested groups, a pure preprocessor shape,
/// and for the plain-AL shapes it shares a cause with (an `exit` nested in an
/// `if` arm, in a loop, or next to an `Error` in an arm that always ends). Each
/// read `unconditional on success` for the `Insert` after the construct.
#[test]
fn a_nested_exit_makes_the_code_after_the_construct_conditional() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    // Every shape at once, so a failure names each one that moved.
    let shapes = [
        "NestedArmExit",
        "NestedIfExit",
        "LoopExit",
        "ExitOrError",
        "BareBlockExit",
    ];
    let got: Vec<(&str, String)> = shapes
        .iter()
        .map(|&r| {
            (
                r,
                insert_conditionality(&ws, r, "B2 Log B", "LogB.Insert();"),
            )
        })
        .collect();
    let want: Vec<(&str, String)> = shapes
        .iter()
        .map(|&r| (r, "conditional".to_string()))
        .collect();
    assert_eq!(got, want);
    // The control: with no exit, a failing `Error` guard leaves the Insert on
    // every SUCCESS path, so the fix does not blur every guarded write.
    assert_eq!(
        insert_conditionality(&ws, "NoExit", "B2 Log B", "LogB.Insert();"),
        "unconditional-on-success"
    );
}

/// Review of B2: a routine declared in two arms is one routine whose build runs
/// one arm's body. Each arm's write read `unconditional on success` through the
/// caller, though the other build never makes it.
#[test]
fn each_arm_of_a_two_arm_routine_is_conditional() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for table in ["B2 Log A", "B2 Log B"] {
        let r = prove(
            &ws,
            "B2 Preproc::CallsTwoArms",
            &format!("writes-table:{table}"),
        );
        assert_eq!(r["answer"], "yes", "{table}: {r:#}");
        let e = &r["evidence"][0];
        assert_eq!(e["type"], "DB_INSERT", "{table}: {r:#}");
        assert_eq!(e["conditionality"], "conditional", "{table}: {r:#}");
    }
}

/// A bare `begin exit; end;` always leaves: the Insert after it is dead in
/// every build. The block's continuation used to be dropped (it went through
/// the generic statement arm), so the Insert read as reachable.
#[test]
fn a_block_that_always_exits_ends_the_routine() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Preproc::AlwaysExitBlock", "writes-table:B2 Log B");
    assert_eq!(r["answer"], "no", "{r:#}");
}

/// Second review of B2, blocking 1: the ordering facts read "can this
/// statement return early" with the one may-exit rule. An `#if` arm that only
/// MAY exit, a nested `if`, a loop and a bare block each may return before the
/// COMMIT, so the COMMIT does not dominate the root's return and no write is
/// committed before every return. Before, `operation_order` looked one arm
/// deep and skipped loops: digest claimed `COMMIT_DOMINATES_RETURN` (scope
/// root, valid for refutation) and `WRITE_COMMITTED_BEFORE_RETURN` for all of
/// them. The same holds through a call (`ordering_inter`'s
/// `exactly_once_on_success`).
#[test]
fn a_statement_that_may_exit_keeps_the_commit_from_dominating_the_return() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    // Every scope: the commit sits in the root itself. Through a call, the
    // callee's own COMMIT_DOMINATES_RETURN (scope owning-routine) holds, so
    // only the root scope is read.
    let labels = |routine: &str, root_only: bool| -> Vec<String> {
        scoped_guarantees(&ws, &format!("B2 Order::{routine}"), root_only)
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    };
    // The controls: nothing returns early.
    let control = labels("Control", false);
    for label in ["COMMIT_DOMINATES_RETURN", "WRITE_COMMITTED_BEFORE_RETURN"] {
        assert!(control.iter().any(|l| l == label), "{label}: {control:?}");
    }
    let through_call = labels("CallsCommitControl", true);
    assert!(
        through_call.iter().any(|l| l == "COMMIT_DOMINATES_RETURN"),
        "{through_call:?}"
    );
    // Every shape at once, so a failure names each one that moved.
    let wrong: Vec<(&str, String)> = [
        ("ArmMayExitCommit", false),
        ("NestedGroupCommit", false),
        ("NestedIfCommit", false),
        ("LoopExitCommit", false),
        ("BareBlockCommit", false),
        ("CallsCommitAfterArmExit", true),
    ]
    .into_iter()
    .flat_map(|(routine, root_only)| {
        labels(routine, root_only)
            .into_iter()
            .filter(|l| l == "COMMIT_DOMINATES_RETURN" || l == "WRITE_COMMITTED_BEFORE_RETURN")
            .map(move |l| (routine, l))
    })
    .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// The answer's first evidence row's conditionality for `question`.
fn first_conditionality(r: &Value) -> String {
    r["evidence"][0]["conditionality"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Second review of B2, blocking 2: a `[CommitBehavior]` inside an `#if` arm
/// the routine is not in was read as declared in every build. `may-commit`
/// said a definite `no` for the routine and its caller, though a build
/// without `CLEAN99` commits (master said `yes`). It only MAY apply now: the
/// commit stays, conditional, and `commits-on-success-path` is `unknown`.
#[test]
fn a_commit_behavior_only_some_builds_declare_may_apply() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for routine in ["B2 Decl::ArmIgnore", "B2 Decl::CallsArmIgnore"] {
        let r = prove(&ws, routine, "may-commit");
        assert_eq!(r["answer"], "yes", "{routine}: {r:#}");
        assert_eq!(first_conditionality(&r), "conditional", "{routine}: {r:#}");
        let r = prove(&ws, routine, "commits-on-success-path");
        assert_eq!(r["answer"], "unknown", "{routine}: {r:#}");
    }
    // An `Error` only some builds declare: the commit may raise an error, and
    // may run.
    let r = prove(&ws, "B2 Decl::ArmError", "throws-error");
    assert_eq!(r["answer"], "yes", "{r:#}");
    assert_eq!(first_conditionality(&r), "conditional", "{r:#}");
    assert_eq!(
        prove(&ws, "B2 Decl::ArmError", "may-commit")["answer"],
        "yes"
    );
    // The control: every build declares it, so the commit is ignored.
    assert_eq!(
        prove(&ws, "B2 Decl::AlwaysIgnore", "may-commit")["answer"],
        "no"
    );
}

/// The path an attribute takes past an `#if` wrapper decides whether every
/// build of its routine has it. Before a wrapper whose arm holds a routine,
/// the attribute belongs to that routine in every build of it (it exists only
/// in that arm), and to the routine after the wrapper only in the builds that
/// skip the arm.
#[test]
fn an_attribute_a_routine_in_an_arm_takes_is_conditional_for_the_next() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_eq!(
        prove(&ws, "B2 Decl::TakesItInArm", "may-commit")["answer"],
        "no"
    );
    let r = prove(&ws, "B2 Decl::TakesLeftover", "may-commit");
    assert_eq!(r["answer"], "yes", "{r:#}");
    assert_eq!(first_conditionality(&r), "conditional", "{r:#}");
}

/// The commit effectiveness `EXTERNAL_IO_BEFORE_COMMIT` carries in a root's
/// digest.
fn io_commit_effectiveness(ws: &str, routine: &str) -> Vec<String> {
    scoped_guarantees(ws, routine, true)
        .into_iter()
        .filter(|(l, _)| l == "EXTERNAL_IO_BEFORE_COMMIT")
        .filter_map(|(_, e)| e)
        .collect()
}

/// The ordering engine graded a commit under an `#if`-only `Ignore` as
/// `proven_suppressed` (its return summary read the attribute as declared). A
/// declaration only some builds have proves nothing either way. (An
/// `#if`-only `[TryFunction]` gets the same rule, `try_function_conditional`;
/// no CLI shape reaches it: a try routine's own ordering is opaque.)
#[test]
fn a_commit_under_a_conditional_declaration_is_not_proven() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    assert_eq!(
        io_commit_effectiveness(&ws, "B2 Decl::IoThenArmIgnore"),
        ["assumed_effective"]
    );
}

/// A `[TryFunction]` only some builds declare traps nothing for certain: a
/// build without it lets the error escape. Before, `throws-error` said `no`.
#[test]
fn a_try_function_only_some_builds_declare_traps_nothing_for_certain() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 Decl::TestsArmTry", "throws-error");
    assert_eq!(r["answer"], "yes", "{r:#}");
    assert_eq!(
        prove(&ws, "B2 Decl::TestsAlwaysTry", "throws-error")["answer"],
        "no"
    );
}

/// `InherentCommitBehavior` with one value per arm: the strictest (`Error`)
/// may apply. Before, the first arm's `Ignore` was read and `throws-error`
/// said `no`.
#[test]
fn an_inherent_commit_behavior_per_arm_keeps_the_strictest() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let r = prove(&ws, "B2 ICB::Commits", "throws-error");
    assert_eq!(r["answer"], "yes", "{r:#}");
    assert_eq!(prove(&ws, "B2 ICB::Commits", "may-commit")["answer"], "yes");
}

/// Second review of B2, blocking 3, and the same rule for every property that
/// decides a definite `no`: a value only some `#if` arms write leaves the
/// property's default possible. An `#if`-only `Direction = Export` keeps the
/// default `Both`, so the XmlPort may import; an `#if`-only `TableType =
/// Temporary`, `SourceTableTemporary = true` or `UseTemporary = true` leaves a
/// build that writes the database. Each said a definite `no` before; each
/// control (written in every build) still does.
#[test]
fn a_property_only_some_builds_write_leaves_its_default_possible() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    // (routine, table, expected answer). Every case at once, so a failure
    // names each one that moved.
    let cases = [
        ("B2 Decl::RunsArmExport", "B2 Xml T", "yes"),
        ("B2 Decl::RunsExport", "B2 Xml T", "no"),
        ("B2 Decl::WritesTempMaybe", "B2 Temp Maybe", "yes"),
        ("B2 Decl::WritesTempAlways", "B2 Temp Always", "no"),
        ("B2 Page Maybe::InsertRec", "B2 Xml T", "yes"),
        ("B2 Page Temp::InsertRec", "B2 Xml T", "no"),
        // The table event's subscriber exits for a temporary record.
        ("B2 Page Maybe::InsertRec", "B2 Sub Log", "yes"),
        ("B2 Page Temp::InsertRec", "B2 Sub Log", "no"),
        ("B2 Rep Maybe::InsertItem", "B2 Xml T", "yes"),
        ("B2 Rep Temp::InsertItem", "B2 Xml T", "no"),
    ];
    let got: Vec<(&str, String)> = cases
        .iter()
        .map(|&(routine, table, _)| {
            let r = prove(&ws, routine, &format!("writes-table:{table}"));
            (
                routine,
                r["answer"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want: Vec<(&str, String)> = cases
        .iter()
        .map(|&(routine, _, answer)| (routine, answer.to_string()))
        .collect();
    assert_eq!(got, want);
}

/// Third review of B2, blocking A: a routine declared in two `#if` arms is one
/// row per arm, and an attribute in ONE arm's row was read as present in every
/// build (its own row has it in every build of that row). `may-commit` said a
/// definite `no` for `ElseIgn`, `TwoBodies` and its caller, and `throws-error`
/// said `no` for `CallTry`, though the build that takes the other arm commits
/// or lets the error escape (the pre-B2 master said `yes`). The same attribute
/// in BOTH arms is in every build: the controls keep their `no`.
#[test]
fn an_attribute_only_one_arm_of_a_routine_declares_is_conditional() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let cases = [
        ("B2 Split::ElseIgn", "may-commit", "yes"),
        ("B2 Split::TwoBodies", "may-commit", "yes"),
        ("B2 Split::CallTwo", "may-commit", "yes"),
        ("B2 Split::CallTry", "throws-error", "yes"),
        ("B2 Split::BothIgn", "may-commit", "no"),
        ("B2 Split::CallBothTry", "throws-error", "no"),
    ];
    let got: Vec<(&str, String)> = cases
        .iter()
        .map(|&(routine, question, _)| {
            let r = prove(&ws, routine, question);
            (
                routine,
                r["answer"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want: Vec<(&str, String)> = cases
        .iter()
        .map(|&(routine, _, answer)| (routine, answer.to_string()))
        .collect();
    assert_eq!(got, want);
}

/// Third review of B2, blocking B: an `#if`-only `DeleteAllowed = false`
/// blocked deletes through the page in every build, so `explain` dropped the
/// page from the table's `OnDelete` UI surfaces. A build without `CLEAN99` has
/// the default, `true` (Microsoft Learn, "DeleteAllowed property": "The
/// default value is true"). The control, written in every build, still blocks.
#[test]
fn a_page_permission_only_some_builds_write_blocks_nothing() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "explain",
            &ws,
            "--routine",
            "\"B2 UI T\"::OnDelete",
            "--single-app",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let surfaces: Vec<&str> = v["cards"][0]["runBy"]["potentialUiSurfaces"]
        .as_array()
        .unwrap_or_else(|| panic!("no surfaces: {v:#}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(surfaces, ["Page \"B2 UI Armed\""], "{v:#}");
}

/// Third review of B2, non-blocking: an event isolated in some builds only
/// (its isolating attribute in one arm, or an `#if` in its arguments) was a
/// trap on the raiser's route, worded "otherwise caught by the isolated
/// event", and the subscriber's `throws-error` said the error does not
/// escape the event unless a write is pending. A build that does not isolate
/// the event lets it escape. The control, isolated in every build, keeps both.
#[test]
fn an_event_isolated_in_some_builds_only_is_no_trap() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for (routine, trapped) in [
        ("B2 Iso Raise::RaiseArm", false),
        ("B2 Iso Raise::RaiseArg", false),
        ("B2 Iso Raise::RaiseIso", true),
    ] {
        let r = prove(&ws, routine, "throws-error");
        assert_eq!(r["answer"], "yes", "{routine}: {r:#}");
        assert_eq!(
            r["evidence"][0]["trap"]["kind"] == "isolated-event",
            trapped,
            "{routine}: {r:#}"
        );
    }
    let human = |routine: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args([
                "prove",
                &ws,
                routine,
                "throws-error",
                "--single-app",
                "--format",
                "human",
            ])
            .env("RUST_LOG", "off")
            .output()
            .expect("spawn alsem");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for routine in ["B2 Iso Sub::SubArm", "B2 Iso Sub::SubArg"] {
        let text = human(routine);
        assert!(
            text.contains("not established (the event is isolated in some builds only"),
            "{routine}: {text}"
        );
    }
    let text = human("B2 Iso Sub::SubIso");
    assert!(
        text.contains("OnIso: no, unless the raising transaction has a write pending"),
        "{text}"
    );
}

/// Run `alsem <args…> --single-app` and parse its JSON stdout.
fn alsem_json(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg("--single-app")
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("json")
}

/// The 1-based lines of `TwoArms`' two inserts: arm A's `LogA.Insert();` and
/// arm B's `LogB.Insert();` (the first after the first `TwoArms` header is arm
/// B's: arm A writes only Log A).
fn two_arm_lines() -> (u64, u64) {
    let a = line_of("LogA.Insert();", "procedure TwoArms()");
    let b = line_of("LogB.Insert();", "procedure TwoArms()");
    assert!(a < b, "precondition: arm A comes first");
    (a, b)
}

/// Engine limits B2, per-arm ids: a routine declared in two `#if` arms cites
/// EACH ARM'S OWN line for its facts. Both arms used to mint the same site ids
/// (`…/op0`), and readers resolve an id to one row (the last), so `prove`
/// cited arm B's line for arm A's write. Each answer stays `conditional`: a
/// build runs one arm. Break: number every arm from 0 (the model builder's
/// `SiteIdBase`) -> Log A's insert cites arm B's line.
#[test]
fn a_two_arm_routine_cites_each_arms_own_line_in_prove() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let (a, b) = two_arm_lines();
    for (table, line) in [("B2 Log A", a), ("B2 Log B", b)] {
        let r = prove(
            &ws,
            "B2 Preproc::CallsTwoArms",
            &format!("writes-table:{table}"),
        );
        assert_eq!(r["answer"], "yes", "{table}: {r:#}");
        let e = &r["evidence"][0];
        assert_eq!(e["type"], "DB_INSERT", "{table}: {r:#}");
        assert_eq!(e["evidence"]["line"], line, "{table}: {r:#}");
        assert_eq!(e["conditionality"], "conditional", "{table}: {r:#}");
    }
}

/// The same through `explain` (its writes card) and `digest` (its effects):
/// each table's write cites its own arm's line, `conditional` in explain.
#[test]
fn a_two_arm_routine_cites_each_arms_own_line_in_explain_and_digest() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let (a, b) = two_arm_lines();
    let want = [("B2 Log A", a), ("B2 Log B", b)];

    let ex = alsem_json(&[
        "explain",
        &ws,
        "--routine",
        "B2 Preproc::CallsTwoArms",
        "--format",
        "json",
    ]);
    let writes = ex["cards"][0]["effects"]["writes"]
        .as_array()
        .unwrap_or_else(|| panic!("explain writes: {ex:#}"));
    for (table, line) in want {
        let w: Vec<&Value> = writes.iter().filter(|w| w["table"] == table).collect();
        assert_eq!(w.len(), 1, "explain {table}: {ex:#}");
        assert_eq!(w[0]["anchor"]["line"], line, "explain {table}: {ex:#}");
        assert_eq!(
            w[0]["conditionality"], "conditional",
            "explain {table}: {ex:#}"
        );
    }

    let dg = alsem_json(&["digest", &ws, "--routine", "B2 Preproc::CallsTwoArms"]);
    let effects = dg["payload"]["effects"]
        .as_object()
        .unwrap_or_else(|| panic!("digest effects: {dg:#}"));
    for (table, line) in want {
        let lines: Vec<&Value> = effects
            .values()
            .filter(|e| e["type"] == "DB_INSERT" && e["detail"]["resourceDisplay"] == table)
            .map(|e| &e["evidence"]["line"])
            .collect();
        assert_eq!(lines, vec![&Value::from(line)], "digest {table}: {dg:#}");
    }
}
