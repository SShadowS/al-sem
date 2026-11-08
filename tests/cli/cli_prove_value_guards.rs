//! r3 E1 (spec `2026-11-08-output-validation-r3.md` §1): Integer, Option and
//! Enum literal values in the literal reach (n10).
//!
//! Continia DO's `DocSave` passes `OutputAction::SaveAsPDF` (a member of a
//! GLOBAL `Option` variable with blank members) into `RunReport`'s Integer
//! parameter, which `RunReportID` reads as `case OutputActionInt of
//! OutputAction::Print: … OutputAction::SaveAsPDF: …`. `EH Handler` is that
//! shape: the arms write different tables, so `prove … writes-table:<T>`
//! says which arms a root can run.
//!
//! Each test states its precondition in `SOURCE` (literal arguments, the
//! parameter's type and uses) and asks `prove`; the controls keep every
//! route that can still run a `yes`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50240 "EH Print" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50241 "EH Save" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50242 "EH Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50243 "EH Not Print" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50244 "EH In List" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50245 "EH Exit Guarded" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50246 "EH Range" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50247 "EH Range First" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50248 "EH Range Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50249 "EH Alpha" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50250 "EH Beta" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50251 "EH Kind Else" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50252 "EH Other Enum" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50253 "EH Other Beta" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50254 "EH Opt A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50255 "EH Opt B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50256 "EH Ctl Var" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50257 "EH Ctl Assigned" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50258 "EH Ctl Passed" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50259 "EH Ctl Loop" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50260 "EH Ctl Unknown" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
enum 50261 "EH Kind"
{
    Extensible = true;
    value(0; Alpha) { }
    value(1; Beta) { }
}
enumextension 50262 "EH Kind Ext" extends "EH Kind"
{
    value(50; Gamma) { }
}
enum 50263 "EH Other"
{
    value(0; Alpha) { }
    value(1; Beta) { }
}
codeunit 50264 "EH Handler"
{
    var
        OutputAction: Option Print,EMailOpen,,,SaveAsPDF;

    // ---- an Integer parameter against a global Option's labels (n10) ----
    procedure DocSave()
    begin
        RunReport(OutputAction::SaveAsPDF);
    end;

    procedure DocPrint()
    begin
        RunReport(OutputAction::Print);
    end;

    procedure DocEMail()
    begin
        RunReport(OutputAction::EMailOpen);
    end;

    // 4 is SaveAsPDF's ordinal only when the two blank members count.
    procedure DocFour()
    begin
        RunReport(4);
    end;

    procedure DocEither(A: Integer)
    begin
        RunReport(A);
    end;

    // Two routes into RunReport: a literal one and an unknown one.
    procedure DocJoin(A: Integer)
    begin
        RunReport(OutputAction::SaveAsPDF);
        RunReport(A);
    end;

    procedure DocComputed()
    begin
        RunReport(ComputeAction());
    end;

    local procedure ComputeAction(): Integer
    begin
        exit(OutputAction::Print);
    end;

    local procedure RunReport(OutputActionInt: Integer)
    begin
        RunReportID(OutputActionInt);
    end;

    local procedure RunReportID(OutputActionInt: Integer)
    begin
        case OutputActionInt of
            OutputAction::Print:
                WriteT(1);
            OutputAction::SaveAsPDF:
                WriteT(2);
            else
                WriteT(3);
        end;
        if OutputActionInt <> OutputAction::Print then
            WriteT(4);
        if OutputActionInt in [OutputAction::Print, OutputAction::EMailOpen] then
            WriteT(5);
        RunExitGuard(OutputActionInt);
    end;

    local procedure RunExitGuard(P: Integer)
    begin
        if P <> OutputAction::Print then
            exit;
        WriteT(6);
    end;

    // ---- an unresolved label keeps its arm and the else live ----
    procedure RangePrint()
    begin
        RunRange(OutputAction::Print);
    end;

    procedure RangeSave()
    begin
        RunRange(OutputAction::SaveAsPDF);
    end;

    local procedure RunRange(P: Integer)
    begin
        case P of
            OutputAction::Print:
                WriteT(8);
            10 .. 12:
                WriteT(7);
            else
                WriteT(9);
        end;
    end;

    // ---- an Enum parameter ----
    procedure KindAlpha()
    begin
        RunKind(Enum::"EH Kind"::Alpha);
    end;

    // A known extension member passed explicitly is one value.
    procedure KindGamma()
    begin
        RunKind(Enum::"EH Kind"::Gamma);
    end;

    // An unknown value of an extensible enum is never narrowed to the
    // members loaded today.
    procedure KindUnknown(K: Enum "EH Kind")
    begin
        RunKind(K);
    end;

    local procedure RunKind(Kind: Enum "EH Kind")
    begin
        case Kind of
            Kind::Alpha:
                WriteT(10);
            Kind::Beta:
                WriteT(11);
            else
                WriteT(12);
        end;
    end;

    // A label of ANOTHER enum (alc would reject the comparison): a
    // conversion not modeled, so its arm stays live, though its value name
    // differs from the bound one.
    procedure KindOther()
    begin
        RunKindOther(Enum::"EH Kind"::Alpha);
    end;

    local procedure RunKindOther(Kind: Enum "EH Kind")
    begin
        case Kind of
            Enum::"EH Other"::Beta:
                WriteT(13);
            Kind::Beta:
                WriteT(14);
        end;
    end;

    // ---- an Option parameter, called with a local Option's label ----
    procedure OptCallB()
    var
        M: Option A,B,C;
    begin
        RunOpt(M::B);
    end;

    local procedure RunOpt(Mode: Option A,B,C)
    begin
        if Mode = Mode::A then
            WriteT(15);
        if Mode = Mode::B then
            WriteT(16);
    end;

    // ---- controls: parameters whose value is not the caller's argument ----
    procedure CtlVar()
    var
        X: Integer;
    begin
        X := 2;
        RunVar(X);
    end;

    local procedure RunVar(var P: Integer)
    begin
        if P = 1 then
            WriteT(17);
    end;

    procedure CtlAssigned()
    begin
        RunAssigned(2);
    end;

    local procedure RunAssigned(P: Integer)
    begin
        P += 1;
        if P = 3 then
            WriteT(18);
    end;

    procedure CtlPassed()
    begin
        RunPassed(2);
    end;

    local procedure RunPassed(P: Integer)
    begin
        Bump(P);
        if P = 1 then
            WriteT(19);
    end;

    local procedure Bump(var X: Integer)
    begin
        X := 1;
    end;

    procedure CtlLoop()
    begin
        RunLoop(5);
    end;

    local procedure RunLoop(P: Integer)
    begin
        for P := 1 to 2 do
            WriteNothing();
        if P = 1 then
            WriteT(20);
    end;

    procedure CtlUnknown()
    begin
        RunUnknownCallee(2);
    end;

    local procedure RunUnknownCallee(P: Integer)
    begin
        NoSuchProcedure(P);
        if P = 1 then
            WriteT(21);
    end;

    local procedure WriteNothing()
    begin
    end;

    local procedure WriteT(Which: Integer)
    var
        TPrint: Record "EH Print";
        TSave: Record "EH Save";
        TElse: Record "EH Else";
        TNotPrint: Record "EH Not Print";
        TInList: Record "EH In List";
        TExit: Record "EH Exit Guarded";
        TRange: Record "EH Range";
        TRangeFirst: Record "EH Range First";
        TRangeElse: Record "EH Range Else";
        TAlpha: Record "EH Alpha";
        TBeta: Record "EH Beta";
        TKindElse: Record "EH Kind Else";
        TOther: Record "EH Other Enum";
        TOtherBeta: Record "EH Other Beta";
        TOptA: Record "EH Opt A";
        TOptB: Record "EH Opt B";
        TVar: Record "EH Ctl Var";
        TAssigned: Record "EH Ctl Assigned";
        TPassed: Record "EH Ctl Passed";
        TLoop: Record "EH Ctl Loop";
        TUnknown: Record "EH Ctl Unknown";
    begin
        case Which of
            1:
                TPrint.Insert();
            2:
                TSave.Insert();
            3:
                TElse.Insert();
            4:
                TNotPrint.Insert();
            5:
                TInList.Insert();
            6:
                TExit.Insert();
            7:
                TRange.Insert();
            8:
                TRangeFirst.Insert();
            9:
                TRangeElse.Insert();
            10:
                TAlpha.Insert();
            11:
                TBeta.Insert();
            12:
                TKindElse.Insert();
            13:
                TOther.Insert();
            14:
                TOtherBeta.Insert();
            15:
                TOptA.Insert();
            16:
                TOptB.Insert();
            17:
                TVar.Insert();
            18:
                TAssigned.Insert();
            19:
                TPassed.Insert();
            20:
                TLoop.Insert();
            21:
                TUnknown.Insert();
        end;
    end;
}
codeunit 50265 "EH Events"
{
    var
        Mode: Option Short,Long;

    // The event is raised two calls down a SHORT route the literal rules out
    // (the witness search lists it) and down a LONG live one.
    procedure EvRoot()
    begin
        EvDispatch(Mode::Long);
    end;

    // Raises the event itself: an extension point.
    procedure EvDirect()
    begin
        OnEhSomething();
    end;

    local procedure EvDispatch(M: Integer)
    begin
        case M of
            Mode::Short:
                EvRaise();
            Mode::Long:
                EvHop1();
        end;
    end;

    local procedure EvHop1()
    begin
        EvHop2();
    end;

    local procedure EvHop2()
    begin
        EvHop3();
    end;

    local procedure EvHop3()
    begin
        EvRaise();
    end;

    local procedure EvRaise()
    begin
        OnEhSomething();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnEhSomething()
    begin
    end;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    crate::as_v3(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    }))
}

/// The workspace, and `prove <routine> writes-table:<table>`'s answer.
struct Ws {
    dir: tempfile::TempDir,
    ids: Vec<(String, String)>,
}

impl Ws {
    fn new() -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            r#"{"id":"e1e1e1e1-0000-4000-8000-0000000000e1","name":"E1","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
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
                    r["routineName"].as_str().unwrap().to_string(),
                    r["stableRoutineId"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        Ws { dir, ids }
    }

    fn answer(&self, routine: &str, table: &str) -> String {
        let id = &self
            .ids
            .iter()
            .find(|(n, _)| n == routine)
            .unwrap_or_else(|| panic!("precondition: routine {routine} is in the inventory"))
            .1;
        let r = alsem(
            self.dir.path(),
            &[
                "prove",
                "ws",
                id,
                &format!("writes-table:{table}"),
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone();
        r["answer"].as_str().unwrap_or_default().to_string()
    }

    /// `explain --format json` of `routine`'s payload.
    fn explain(&self, routine: &str) -> serde_json::Value {
        alsem(
            self.dir.path(),
            &[
                "explain",
                "ws",
                "--routine",
                &format!("EH Events::{routine}"),
                "--format",
                "json",
            ],
        )
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
}

/// n10's shape: an Integer parameter, bound from a global Option's label,
/// prunes the `case` arms it does not match, and a forwarded parameter keeps
/// the value (`RunReport` → `RunReportID`). Break: dropping the arm
/// requirement (`DocSave` reaches `EH Print`).
#[test]
fn case_arms_follow_an_option_label_through_an_integer_parameter() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Print", "no"),
        ("DocSave", "EH Save", "yes"),
        ("DocPrint", "EH Print", "yes"),
        ("DocPrint", "EH Save", "no"),
    ]);
}

/// Option ordinals count blank members: `OutputAction::SaveAsPDF` is 4 (two
/// blank members before it), so the integer literal 4 runs the `SaveAsPDF`
/// arm. Break: skipping blank members (SaveAsPDF becomes 2).
#[test]
fn an_option_ordinal_counts_blank_members() {
    let ws = Ws::new();
    ws.expect(&[("DocFour", "EH Save", "yes"), ("DocFour", "EH Print", "no")]);
}

/// The `else` is dead only when every bound value matches a resolved label
/// of some arm. Break: no requirement on the `else` (`DocSave` reaches
/// `EH Else`).
#[test]
fn case_else_runs_only_for_an_unlisted_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Else", "no"),
        ("DocPrint", "EH Else", "no"),
        ("DocEMail", "EH Else", "yes"),
    ]);
}

/// `if P <> L`, `if P in [..]` and `if P <> L then exit;` prune as `=` does.
/// Break: flipping the test's polarity.
#[test]
fn if_comparisons_and_an_exit_guard_prune() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocSave", "EH Not Print", "yes"),
        ("DocPrint", "EH Not Print", "no"),
        ("DocSave", "EH In List", "no"),
        ("DocEMail", "EH In List", "yes"),
        ("DocPrint", "EH In List", "yes"),
        ("DocSave", "EH Exit Guarded", "no"),
        ("DocPrint", "EH Exit Guarded", "yes"),
    ]);
}

/// A label the engine cannot resolve (a range) keeps its arm AND the `else`
/// live, even for a value a resolved arm matches; a resolved arm that does not
/// match is still dead. Break: taking the `else` requirement from the resolved
/// arms only (`RangePrint` loses `EH Range Else`).
#[test]
fn an_unresolved_label_keeps_its_arm_and_the_else_live() {
    let ws = Ws::new();
    ws.expect(&[
        ("RangePrint", "EH Range First", "yes"),
        ("RangePrint", "EH Range", "yes"),
        ("RangePrint", "EH Range Else", "yes"),
        ("RangeSave", "EH Range First", "no"),
        ("RangeSave", "EH Range", "yes"),
        ("RangeSave", "EH Range Else", "yes"),
    ]);
}

/// Each route keeps its own binding: `DocJoin` reaches `RunReport` with the
/// `SaveAsPDF` literal and with its own unknown parameter, so the Print arm
/// stays live through the second route; an unknown root parameter or a
/// computed argument binds nothing. Break: keying the search's states by
/// routine only (the unknown route is folded into the literal one).
#[test]
fn an_unknown_route_never_inherits_another_routes_literal() {
    let ws = Ws::new();
    ws.expect(&[
        ("DocJoin", "EH Print", "yes"),
        ("DocEither", "EH Print", "yes"),
        ("DocComputed", "EH Print", "yes"),
    ]);
}

/// An Enum parameter compares by (enum, value): a known value (an extension
/// member included) runs only its arm, or the `else`; an unknown value of an
/// extensible enum runs every arm and the `else`.
#[test]
fn an_enum_parameter_compares_by_enum_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("KindAlpha", "EH Alpha", "yes"),
        ("KindAlpha", "EH Beta", "no"),
        ("KindAlpha", "EH Kind Else", "no"),
        ("KindGamma", "EH Alpha", "no"),
        ("KindGamma", "EH Beta", "no"),
        ("KindGamma", "EH Kind Else", "yes"),
        ("KindUnknown", "EH Alpha", "yes"),
        ("KindUnknown", "EH Beta", "yes"),
        ("KindUnknown", "EH Kind Else", "yes"),
    ]);
}

/// A label of another enum is a conversion not modeled: its arm stays live
/// (its value name `Beta` differs from the bound `Alpha`); the other arm is
/// still judged. Break: dropping the enum's identity from the comparison (the
/// `EH Other` arm becomes dead).
#[test]
fn another_enums_label_is_not_the_same_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("KindOther", "EH Other Enum", "yes"),
        ("KindOther", "EH Other Beta", "no"),
    ]);
}

/// An Option parameter, bound from a LOCAL Option variable's label, compared
/// with its own labels.
#[test]
fn an_option_parameter_binds_from_a_local_options_label() {
    let ws = Ws::new();
    ws.expect(&[
        ("OptCallB", "EH Opt A", "no"),
        ("OptCallB", "EH Opt B", "yes"),
    ]);
}

/// The usable rule: a `var` parameter, an assigned one (compound), one passed
/// `var` to a helper, a `for` variable, and one passed to a call the resolver
/// did not resolve all hold something other than the caller's argument, so
/// nothing is pruned. One break each: removing that clause of the rule.
#[test]
fn a_parameter_whose_value_may_change_binds_nothing() {
    let ws = Ws::new();
    ws.expect(&[
        ("CtlVar", "EH Ctl Var", "yes"),
        ("CtlAssigned", "EH Ctl Assigned", "yes"),
        ("CtlPassed", "EH Ctl Passed", "yes"),
        ("CtlLoop", "EH Ctl Loop", "yes"),
        ("CtlUnknown", "EH Ctl Unknown", "yes"),
    ]);
}

/// An event raised only down calls is no extension point of the root, even
/// when its listed (shortest) path cannot run under the literal arguments and
/// is dropped: the row's empty path is not a zero-hop one. `EvRoot` raises
/// `OnEhSomething` four calls down (the two-call route is literal-dead);
/// `EvDirect` raises it itself. Break: reading an empty path as zero hops
/// (`EvRoot` lists the extension point, the CDO IssueReminder explain grew by
/// 60 such lines).
#[test]
fn an_event_whose_listed_paths_were_dropped_is_no_extension_point() {
    let ws = Ws::new();
    let points = |r: &str| -> Vec<String> {
        let v = ws.explain(r);
        let card = v
            .pointer("/payload/cards/0")
            .or_else(|| v.pointer("/payload"))
            .cloned()
            .unwrap_or(v.clone());
        let mut out = Vec::new();
        let mut stack = vec![&card];
        while let Some(x) = stack.pop() {
            match x {
                serde_json::Value::Object(m) => {
                    if let Some(serde_json::Value::Array(eps)) = m.get("extensionPoints") {
                        out.extend(
                            eps.iter()
                                .filter_map(|e| e["event"].as_str().map(String::from)),
                        );
                    }
                    stack.extend(m.values());
                }
                serde_json::Value::Array(a) => stack.extend(a.iter()),
                _ => {}
            }
        }
        out
    };
    assert_eq!(
        points("EvDirect"),
        ["OnEhSomething"],
        "precondition: a raise of its own is one"
    );
    assert!(points("EvRoot").is_empty(), "{:?}", points("EvRoot"));
}
