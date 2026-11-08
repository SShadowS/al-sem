//! Round-4 literal routes (fact check c15, p01, p09): what the literal
//! arguments decide beyond `case` / `if` branches.
//!
//! - An interface local assigned from a known enum value dispatches only to
//!   that value's implementation (c15: `ModuleLicense := ModuleType` with
//!   `ModuleType` bound to `"CDO Module Type"::Essential`).
//! - An unresolved call site no route can reach blocks nothing (c15: the
//!   `GetModuleName` dispatch reached only under `if ShowError`, every caller
//!   passing `false`).
//! - A `Variant` parameter whose caller passes a variable of known static
//!   type decides the parameter's type tests (p01: `RecordIsTemporary(Rec)`
//!   behind `if not VariantRecord.IsRecord then Error(…)`).
//!
//! Each case states its precondition in `SOURCE` and asks `prove …
//! writes-table:<T>` (one table per guarded write); the controls keep every
//! write that can still run a `yes`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50400 "LR T1" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50401 "LR T2" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50402 "LR T3" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50403 "LR T4" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50404 "LR T5" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50405 "LR T6" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50406 "LR T7" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
interface "LR Strategy"
{
    procedure Run();
}
enum 50410 "LR Kind" implements "LR Strategy"
{
    Extensible = true;
    DefaultImplementation = "LR Strategy" = "LR Default";
    value(0; Fast) { Implementation = "LR Strategy" = "LR Fast"; }
    value(1; Slow) { Implementation = "LR Strategy" = "LR Slow"; }
    value(2; Plain) { }
    value(3; Ghost) { Implementation = "LR Strategy" = "LR Not Loaded"; }
}
enumextension 50415 "LR Kind Ext" extends "LR Kind"
{
    value(10; Extra) { Implementation = "LR Strategy" = "LR Ext Impl"; }
}
codeunit 50411 "LR Fast" implements "LR Strategy"
{
    procedure Run()
    var
        T: Record "LR T1";
    begin
        T.Insert();
    end;
}
codeunit 50412 "LR Slow" implements "LR Strategy"
{
    procedure Run()
    var
        T: Record "LR T2";
    begin
        T.Insert();
    end;
}
codeunit 50413 "LR Default" implements "LR Strategy"
{
    procedure Run()
    var
        T: Record "LR T3";
    begin
        T.Insert();
    end;
}
codeunit 50414 "LR Ext Impl" implements "LR Strategy"
{
    procedure Run()
    var
        T: Record "LR T4";
    begin
        T.Insert();
    end;
}
codeunit 50420 "LR Iface"
{
    // ---- an enum literal bound to a parameter, then to an interface local ----
    procedure FastLiteral()
    begin
        Dispatch("LR Kind"::Fast);
    end;

    procedure FastQualified()
    begin
        Dispatch(Enum::"LR Kind"::Fast);
    end;

    // ---- the value's implementation is the enum's default ----
    procedure PlainValue()
    begin
        Dispatch("LR Kind"::Plain);
    end;

    // ---- an enum extension's value: its own implementation ----
    procedure ExtraValue()
    begin
        Dispatch("LR Kind"::Extra);
    end;

    // ---- an implementation that is not loaded: every one stays ----
    procedure GhostValue()
    begin
        Dispatch("LR Kind"::Ghost);
    end;

    // ---- an unknown value: every implementation ----
    procedure UnknownValue(K: Enum "LR Kind")
    begin
        Dispatch(K);
    end;

    // ---- a literal assigned to the interface local in the routine ----
    procedure LocalLiteral()
    var
        S: Interface "LR Strategy";
    begin
        S := "LR Kind"::Slow;
        S.Run();
    end;

    local procedure Dispatch(Kind: Enum "LR Kind")
    var
        S: Interface "LR Strategy";
    begin
        S := Kind;
        S.Run();
    end;
}
codeunit 50430 "LR Variant"
{
    // ---- a Variant type test decided by the argument's static type ----
    procedure RecordPassed()
    var
        R: Record "LR T5";
    begin
        Check(R);
    end;

    procedure CallResultPassed()
    begin
        Check(Rec5());
    end;

    procedure CodeunitPassed()
    var
        C: Codeunit "LR Fast";
    begin
        Check(C);
    end;

    procedure TextPassed()
    var
        X: Text;
    begin
        Check(X);
    end;

    local procedure Rec5(): Boolean
    begin
        exit(true);
    end;

    local procedure Check(V: Variant)
    var
        T: Record "LR T6";
    begin
        if not V.IsRecord then
            T.Insert();
    end;
}
codeunit 50440 "LR Blocker"
{
    // ---- an unresolved site behind a branch the literals rule out ----
    procedure ShowFalse()
    begin
        Inner(false);
    end;

    procedure ShowTrue()
    begin
        Inner(true);
    end;

    local procedure Inner(Show: Boolean)
    var
        T: Record "LR T7";
        S: Interface "LR Strategy";
    begin
        if Show then
            S.Run();
        T.Get('X');
    end;
}
codeunit 50445 "LR Commit"
{
    // ---- a commit only a route the literals rule out reaches (p09) ----
    // A local (not a literal argument), so the cone keeps the commit and
    // only the literal reach rules it out.
    procedure NoCommit()
    var
        Flag: Boolean;
    begin
        Flag := false;
        Maybe(Flag);
    end;

    local procedure Maybe(C: Boolean)
    begin
        if C then
            Commit();
    end;
}
"#;

/// A table whose trigger passes its own `Rec` to a `Variant` (p01's shape):
/// `OnInsert` → `NextNo` → `IsTemp(Rec)`, whose `Error` runs only when the
/// argument is not a record.
const P01: &str = r#"table 50450 "LR Attach"
{
    fields { field(1; "Entry No."; Integer) { } }
    keys { key(PK; "Entry No.") { } }

    trigger OnInsert()
    begin
        "Entry No." := NextNo();
    end;

    procedure NextNo(): Integer
    begin
        if IsTemp(Rec) then
            exit(1);
        exit(2);
    end;

    procedure NextNoText(): Integer
    var
        X: Text;
    begin
        if IsTemp(X) then
            exit(1);
        exit(2);
    end;

    local procedure IsTemp(VariantRecord: Variant) ReturnValue: Boolean
    var
        RecRef: RecordRef;
    begin
        if not VariantRecord.IsRecord then
            Error('You can only use IsTemp with record.');
        RecRef.GetTable(VariantRecord);
        ReturnValue := RecRef.IsTemporary;
    end;
}
"#;

fn alsem_raw(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = alsem_raw(cwd, args);
    crate::as_v3(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    }))
}

/// The workspace and its routines' stable ids.
struct Ws {
    dir: tempfile::TempDir,
    ids: Vec<(String, String, String)>,
}

impl Ws {
    fn with_source(source: &str) -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            r#"{"id":"e4e4e4e4-0000-4000-8000-0000000000e4","name":"LR","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(ws.join("src/Routes.al"), source).unwrap();
        let inv = alsem(
            dir.path(),
            &["fingerprint", "ws", "--inventory-only", "--format", "json"],
        );
        let ids = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["objectName"].as_str().unwrap_or_default().to_string(),
                    r["routineName"].as_str().unwrap().to_string(),
                    r["stableRoutineId"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        Ws { dir, ids }
    }

    fn id(&self, routine: &str) -> &str {
        &self
            .ids
            .iter()
            .find(|(_, n, _)| n == routine)
            .unwrap_or_else(|| panic!("precondition: routine {routine} is in the inventory"))
            .2
    }

    fn prove(&self, routine: &str, question: &str) -> serde_json::Value {
        alsem(
            self.dir.path(),
            &["prove", "ws", self.id(routine), question, "--deterministic"],
        )["payload"]["result"]
            .clone()
    }

    fn prove_human(&self, routine: &str, question: &str) -> String {
        let out = alsem_raw(
            self.dir.path(),
            &[
                "prove",
                "ws",
                self.id(routine),
                question,
                "--format",
                "human",
            ],
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn answer(&self, routine: &str, table: &str) -> String {
        self.prove(routine, &format!("writes-table:{table}"))["answer"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Every `(routine, table, answer)` holds; all mismatches in one message.
    fn expect(&self, cases: &[(&str, &str, &str)]) {
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(r, t, want)| {
                let got = self.answer(r, t);
                (got != *want).then(|| format!("{r} writes {t}: got {got}, want {want}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    fn explain_human(&self, selector: &str) -> String {
        let out = alsem_raw(
            self.dir.path(),
            &["explain", "--routine", selector, "--format", "human", "ws"],
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

/// c15's shape: a literal enum value bound through a parameter to an
/// interface local reaches only that value's implementation, written as
/// `"Enum"::Value` or `Enum::"Enum"::Value`; an unknown value reaches every
/// one. (The interface is open world, so a `no` reads `unknown`: the
/// implementations ruled out leave the evidence, which is what is asked.)
/// Break: `dispatch_excluded` always false (every implementation stays).
#[test]
fn an_enum_literal_selects_its_implementation() {
    let ws = Ws::with_source(SOURCE);
    for root in ["FastLiteral", "FastQualified"] {
        assert_eq!(ws.answer(root, "LR T1"), "yes", "{root}: Fast runs");
        assert_ne!(ws.answer(root, "LR T2"), "yes", "{root}: Slow cannot run");
        assert_ne!(
            ws.answer(root, "LR T3"),
            "yes",
            "{root}: Default cannot run"
        );
    }
    ws.expect(&[
        ("UnknownValue", "LR T1", "yes"),
        ("UnknownValue", "LR T2", "yes"),
    ]);
}

/// A value with no `Implementation` takes the enum's `DefaultImplementation`;
/// an enum extension's value takes its own. Break: the default not read (the
/// value then counts as unresolved and every implementation stays).
#[test]
fn a_default_and_an_extension_value_select_their_implementation() {
    let ws = Ws::with_source(SOURCE);
    assert_eq!(ws.answer("PlainValue", "LR T3"), "yes");
    assert_ne!(ws.answer("PlainValue", "LR T1"), "yes");
    assert_eq!(ws.answer("ExtraValue", "LR T4"), "yes");
    assert_ne!(ws.answer("ExtraValue", "LR T1"), "yes");
}

/// An implementation that names no loaded codeunit narrows nothing (it is
/// not resolved), and a literal assigned to the interface local in the
/// routine selects as a bound parameter does. Break: an unresolved
/// implementation treated as resolved (every edge ruled out).
#[test]
fn an_unresolved_implementation_keeps_every_one_and_a_local_literal_selects() {
    let ws = Ws::with_source(SOURCE);
    ws.expect(&[
        ("GhostValue", "LR T1", "yes"),
        ("GhostValue", "LR T2", "yes"),
        ("LocalLiteral", "LR T2", "yes"),
    ]);
    assert_ne!(ws.answer("LocalLiteral", "LR T1"), "yes");
}

/// p01's rule: a `Variant` parameter fed a record makes `not V.IsRecord`
/// false, so the write behind it cannot run; a codeunit makes it true, and
/// a Text (a type the engine does not decide) keeps it. Break:
/// `variant_test` answering `None` for every method.
#[test]
fn a_variant_type_test_follows_the_arguments_static_type() {
    let ws = Ws::with_source(SOURCE);
    ws.expect(&[
        ("RecordPassed", "LR T6", "no"),
        ("CodeunitPassed", "LR T6", "yes"),
        ("TextPassed", "LR T6", "yes"),
        // `Check(Rec5())` passes a call's result: unknown, kept.
        ("CallResultPassed", "LR T6", "yes"),
    ]);
}

/// c15's blocker: an unresolved interface call reached only under `if Show`
/// is no blocker when every route passes `Show = false`; the answer says once
/// how many were ruled out. With `true` it still blocks. Break: the cone not
/// filtered by `literal_dead_sites` (the `ShowFalse` answer is `unknown`).
#[test]
fn an_unresolved_site_the_literals_rule_out_blocks_nothing() {
    let ws = Ws::with_source(SOURCE);
    let f = ws.prove("ShowFalse", "writes-table:LR T7");
    assert_eq!(f["answer"], "no", "{f:#}");
    assert_eq!(f["obligations"]["literalDeadSites"], 1, "{f:#}");
    let t = ws.prove("ShowTrue", "writes-table:LR T7");
    assert_eq!(t["answer"], "unknown", "{t:#}");
    assert!(t["obligations"].get("literalDeadSites").is_none(), "{t:#}");
    let h = ws.prove_human("ShowFalse", "writes-table:LR T7");
    assert_eq!(
        h.matches("ruled out by literal arguments: 1 call site")
            .count(),
        1,
        "{h}"
    );
}

/// p09: a commit site no route can reach stays listed as
/// `literalUnreachable`, without the kept witness (it passes the branch the
/// literals rule out), as the human text shows none. Break: `mark_dead`
/// keeping `via`.
#[test]
fn a_dead_rows_route_is_not_shown() {
    let ws = Ws::with_source(SOURCE);
    let out = alsem_raw(
        ws.dir.path(),
        &[
            "explain",
            "--routine",
            "Codeunit \"LR Commit\"::NoCommit",
            "--format",
            "json",
            "ws",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("{e}: {}", String::from_utf8_lossy(&out.stderr));
    });
    let sites = v["payload"]["cards"][0]["effects"]["commitSites"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            v["cards"][0]["effects"]["commitSites"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        });
    let dead: Vec<&serde_json::Value> = sites
        .iter()
        .filter(|s| s["literalUnreachable"] == true)
        .collect();
    assert_eq!(dead.len(), 1, "precondition: one dead commit row: {v:#}");
    assert!(dead[0].get("via").is_none(), "{:#}", dead[0]);
}

/// p01: a table trigger passing its own `Rec` to a `Variant` neither
/// inherits the "not a record" rule nor counts its error; a Text argument
/// keeps both. Break: `variant_across` answering `Unknown` for every type
/// (the rule is inherited again).
#[test]
fn p01_a_record_argument_rules_out_the_not_a_record_error() {
    let ws = Ws::with_source(P01);
    let insert = ws.explain_human("Table \"LR Attach\"::OnInsert");
    assert!(
        !insert.contains("VariantRecord.IsRecord"),
        "no inherited rule: {insert}"
    );
    assert!(!insert.contains("ERROR_THROW"), "no error: {insert}");
    let text = ws.explain_human("Table \"LR Attach\"::NextNoText");
    assert!(
        text.contains("rejects when not VariantRecord.IsRecord"),
        "a Text keeps the rule: {text}"
    );
    assert!(text.contains("ERROR_THROW"), "and the error: {text}");
}
