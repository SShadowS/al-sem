//! r3 E2 (spec `2026-11-08-output-validation-r3.md` §1 E2): values inside a
//! routine and `var` parameters passed to an event publisher (n01).
//!
//! Continia DO's `CreateAndSendMail` takes `UseSMTP` by value, may set it,
//! passes it `var` to the publisher `OnBeforeCreateAndSendMailV2`, derives
//! `IsEmailPreview := not UseSMTP`, and reaches a COMMIT only under `if
//! IsEmailPreview`. `FV Mail` is that shape. Each other case states its
//! precondition in `SOURCE` and asks `prove … writes-table:<T>` (one table per
//! guarded write); the controls keep every write that can still run a `yes`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50300 "FV T1" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50301 "FV T2" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50302 "FV T3" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50303 "FV T4" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50304 "FV T5" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50305 "FV T6" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50306 "FV T7" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50307 "FV T8" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50308 "FV T9" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50309 "FV T10" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50310 "FV T11" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50311 "FV T12" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50312 "FV T13" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50313 "FV T14" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50314 "FV T15" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50315 "FV T16" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50316 "FV T17" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50317 "FV T18" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50318 "FV T19" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50319 "FV T20" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50320 "FV T21" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50321 "FV T22" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50322 "FV T23" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50323 "FV T24" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50324 "FV T25" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50325 "FV T26" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
codeunit 50330 "FV Flow"
{
    var
        GFlag: Boolean;

    // ---- a literal assigned to a local (not its default value) ----
    procedure LitDead()
    var
        X: Boolean;
    begin
        X := true;
        if not X then
            W(1);
    end;

    // ---- a value derived with `not` from a reassigned parameter (n01) ----
    procedure NotFromTrue()
    begin
        Derived(true, false);
    end;

    procedure NotFromFalse()
    begin
        Derived(false, false);
    end;

    procedure NotFromHandle()
    begin
        Derived(false, true);
    end;

    local procedure Derived(UseSmtp: Boolean; Handle: Boolean)
    var
        Preview: Boolean;
    begin
        if Handle then
            UseSmtp := true;
        Preview := not UseSmtp;
        if Preview then
            W(2);
    end;

    // ---- branches join ----
    procedure JoinSame()
    var
        X: Boolean;
    begin
        if Unknown() then
            X := true
        else
            X := true;
        if not X then
            W(3);
    end;

    procedure JoinDiffer()
    var
        X: Boolean;
    begin
        // The first branch's value alone would rule the write out.
        if Unknown() then
            X := false
        else
            X := true;
        if X then
            W(4);
    end;

    // ---- a loop-carried value ----
    procedure LoopCarried()
    var
        X: Boolean;
    begin
        X := false;
        repeat
            if X then
                W(5);
            X := true;
        until Unknown();
    end;

    procedure LoopUnwritten()
    var
        X: Boolean;
    begin
        X := false;
        repeat
            if X then
                W(6);
        until Unknown();
    end;

    // ---- a var parameter from its first assignment ----
    procedure VarAssigned()
    var
        V: Boolean;
    begin
        VarFirst(V);
    end;

    local procedure VarFirst(var V: Boolean)
    begin
        if V then
            W(7);
        V := false;
        if V then
            W(8);
    end;

    // ---- two var formals called with the same actual ----
    procedure SameActual()
    var
        X: Boolean;
    begin
        TwoFormals(X, X);
    end;

    local procedure TwoFormals(var A: Boolean; var B: Boolean)
    begin
        A := true;
        B := false;
        if not A then
            W(9);
    end;

    // ---- a var formal naming a global the routine writes ----
    procedure GlobalActual()
    begin
        FormalAndGlobal(GFlag);
    end;

    local procedure FormalAndGlobal(var V: Boolean)
    begin
        V := true;
        GFlag := false;
        if not V then
            W(10);
    end;

    // ---- a var formal across a call ----
    procedure CallActual()
    var
        X: Boolean;
    begin
        FormalAcrossCall(X);
    end;

    local procedure FormalAcrossCall(var V: Boolean)
    begin
        V := true;
        Touch();
        if not V then
            W(11);
    end;

    local procedure Touch()
    begin
        GFlag := true;
    end;

    // ---- a global is never tracked ----
    procedure GlobalNever()
    begin
        GFlag := false;
        if GFlag then
            W(12);
    end;

    // ---- E1's binding from a caller's tracked local ----
    procedure FromLocal()
    var
        K: Integer;
    begin
        K := 2;
        Pick(K);
    end;

    local procedure Pick(N: Integer)
    begin
        case N of
            1:
                W(13);
            2:
                W(14);
        end;
    end;

    // ---- publishers ----
    procedure PubNone()
    var
        U: Boolean;
    begin
        U := true;
        OnNoSubscriber(U);
        if not U then
            W(15);
    end;

    procedure PubAssigning()
    var
        U: Boolean;
    begin
        U := true;
        OnAssigned(U);
        if not U then
            W(16);
    end;

    procedure PubForwarding()
    var
        U: Boolean;
    begin
        U := true;
        OnForwarded(U);
        if not U then
            W(17);
    end;

    procedure PubTwoReaders()
    var
        U: Boolean;
    begin
        U := true;
        OnTwoReaders(U);
        if not U then
            W(18);
    end;

    procedure PubReaderAndWriter()
    var
        U: Boolean;
    begin
        U := true;
        OnReaderAndWriter(U);
        if not U then
            W(19);
    end;

    // The loaded subscriber takes the parameter by value: it cannot change it.
    procedure PubByValueSubscriber()
    var
        U: Boolean;
    begin
        U := true;
        OnByValueSubscriber(U);
        if not U then
            W(20);
    end;

    // ---- a guard the flow cannot evaluate stays in the condition ----
    procedure Unevaluated(Flag: Boolean)
    var
        On: Boolean;
        Both: Boolean;
    begin
        On := true;
        // Both may hold either value: known values, but not one.
        if Unknown() then
            Both := true
        else
            Both := false;
        if On then
            if Flag then
                if Both then
                    W(21);
    end;

    // ---- a value passed to Evaluate is written ----
    procedure EvaluateWrites()
    var
        X: Boolean;
    begin
        X := false;
        Evaluate(X, 'Yes');
        if X then
            W(22);
    end;

    // ---- a call not resolved may write a variable passed to it ----
    procedure UnresolvedWrites()
    var
        X: Boolean;
    begin
        X := false;
        NoSuchProcedure(X);
        if X then
            W(23);
    end;

    // ---- an exit guard on a derived value ----
    procedure ExitGuardTrue()
    begin
        ExitGuard(true);
    end;

    local procedure ExitGuard(Smtp: Boolean)
    var
        Preview: Boolean;
    begin
        Preview := not Smtp;
        if not Preview then
            exit;
        W(24);
    end;

    // ---- a for loop's counter is written ----
    procedure ForCounter()
    var
        I: Integer;
    begin
        I := 0;
        for I := 1 to 3 do
            if I <> 0 then
                W(25);
    end;

    local procedure Unknown(): Boolean
    var
        T: Record "FV T26";
    begin
        exit(T.FindFirst());
    end;

    [IntegrationEvent(false, false)]
    local procedure OnNoSubscriber(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAssigned(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnForwarded(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnTwoReaders(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnReaderAndWriter(var Value: Boolean)
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnByValueSubscriber(var Value: Boolean)
    begin
    end;

    local procedure W(Which: Integer)
    var
        T1: Record "FV T1";
        T2: Record "FV T2";
        T3: Record "FV T3";
        T4: Record "FV T4";
        T5: Record "FV T5";
        T6: Record "FV T6";
        T7: Record "FV T7";
        T8: Record "FV T8";
        T9: Record "FV T9";
        T10: Record "FV T10";
        T11: Record "FV T11";
        T12: Record "FV T12";
        T13: Record "FV T13";
        T14: Record "FV T14";
        T15: Record "FV T15";
        T16: Record "FV T16";
        T17: Record "FV T17";
        T18: Record "FV T18";
        T19: Record "FV T19";
        T20: Record "FV T20";
        T21: Record "FV T21";
        T22: Record "FV T22";
        T23: Record "FV T23";
        T24: Record "FV T24";
        T25: Record "FV T25";
    begin
        case Which of
            1:
                T1.Insert();
            2:
                T2.Insert();
            3:
                T3.Insert();
            4:
                T4.Insert();
            5:
                T5.Insert();
            6:
                T6.Insert();
            7:
                T7.Insert();
            8:
                T8.Insert();
            9:
                T9.Insert();
            10:
                T10.Insert();
            11:
                T11.Insert();
            12:
                T12.Insert();
            13:
                T13.Insert();
            14:
                T14.Insert();
            15:
                T15.Insert();
            16:
                T16.Insert();
            17:
                T17.Insert();
            18:
                T18.Insert();
            19:
                T19.Insert();
            20:
                T20.Insert();
            21:
                T21.Insert();
            22:
                T22.Insert();
            23:
                T23.Insert();
            24:
                T24.Insert();
            25:
                T25.Insert();
        end;
    end;
}
codeunit 50331 "FV Subscribers"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnAssigned', '', false, false)]
    local procedure AssignIt(var Value: Boolean)
    begin
        Value := false;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnForwarded', '', false, false)]
    local procedure ForwardIt(var Value: Boolean)
    begin
        SetIt(Value);
    end;

    local procedure SetIt(var V: Boolean)
    begin
        V := false;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnTwoReaders', '', false, false)]
    local procedure ReadOne(var Value: Boolean)
    var
        T: Record "FV T26";
    begin
        if Value then
            T.Insert();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnTwoReaders', '', false, false)]
    local procedure ReadTwo(var Value: Boolean)
    var
        T: Record "FV T26";
    begin
        if not Value then
            T.Insert();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnReaderAndWriter', '', false, false)]
    local procedure ReadThree(var Value: Boolean)
    var
        T: Record "FV T26";
    begin
        if Value then
            T.Insert();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnReaderAndWriter', '', false, false)]
    local procedure WriteThree(var Value: Boolean)
    begin
        Value := false;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FV Flow", 'OnByValueSubscriber', '', false, false)]
    local procedure ByValue(Value: Boolean)
    var
        T: Record "FV T26";
    begin
        if Value then
            T.Insert();
    end;
}
codeunit 50332 "FV Mail"
{
    // n01's shape: the COMMIT runs only in the preview branch, and a caller
    // passing UseSMTP = true cannot reach it unless a subscriber changes it.
    procedure SendNow()
    begin
        CreateAndSend(true, false);
    end;

    procedure SendPreview()
    begin
        CreateAndSend(false, false);
    end;

    local procedure CreateAndSend(UseSMTP: Boolean; UseHandleDoc: Boolean)
    var
        IsHandled: Boolean;
        IsEmailPreview: Boolean;
    begin
        if UseHandleDoc then
            UseSMTP := true;
        OnBeforeCreateAndSend(UseSMTP, UseHandleDoc, IsHandled);
        if IsHandled then
            exit;
        IsEmailPreview := not UseSMTP;
        if IsEmailPreview then
            OpenMail(Unknown());
    end;

    local procedure OpenMail(Modal: Boolean)
    begin
        if Modal then
            Commit();
    end;

    local procedure Unknown(): Boolean
    var
        T: Record "FV T26";
    begin
        exit(T.FindFirst());
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeCreateAndSend(var UseSMTP: Boolean; UseHandleDoc: Boolean; var IsHandled: Boolean)
    begin
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
    fn new() -> Ws {
        Ws::with_source(SOURCE)
    }

    fn with_source(source: &str) -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            r#"{"id":"e2e2e2e2-0000-4000-8000-0000000000e2","name":"E2","publisher":"t","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::write(ws.join("src/Flow.al"), source).unwrap();
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
}

/// A literal assigned to a local decides its `if`. Break: a local starting
/// unknown and an assignment not evaluated (no value is ever known).
#[test]
fn a_literal_assigned_to_a_local_decides_its_branch() {
    let ws = Ws::new();
    ws.expect(&[("LitDead", "FV T1", "no")]);
}

/// n01's derivation: `Preview := not UseSmtp` after `if Handle then UseSmtp
/// := true` (a by-value parameter that changes, which E1 cannot bind).
/// Break: `not` evaluated as the identity.
#[test]
fn a_value_derived_with_not_follows_a_reassigned_parameter() {
    let ws = Ws::new();
    ws.expect(&[
        ("NotFromTrue", "FV T2", "no"),
        ("NotFromFalse", "FV T2", "yes"),
        ("NotFromHandle", "FV T2", "no"),
    ]);
}

/// Every feasible branch joins: two branches assigning `true` leave `true`,
/// two different values leave both. Break: the join keeps the first
/// branch's value only.
#[test]
fn branches_join() {
    let ws = Ws::new();
    ws.expect(&[("JoinSame", "FV T3", "no"), ("JoinDiffer", "FV T4", "yes")]);
}

/// A variable assigned anywhere in a loop is unknown at every read inside it,
/// before the assignment too (the spec's `X := false; repeat if X then …; X
/// := true; until …`), while one the loop never writes keeps its value.
/// Break: no loop widening.
#[test]
fn a_loop_carried_value_is_unknown_in_the_loop() {
    let ws = Ws::new();
    ws.expect(&[
        ("LoopCarried", "FV T5", "yes"),
        ("LoopUnwritten", "FV T6", "no"),
    ]);
}

/// A `var` parameter is tracked from its first assignment only: before it,
/// its value is the caller's. Break: a `var` formal starting at its type's
/// default (`false`).
#[test]
fn a_var_parameter_is_tracked_from_its_first_assignment() {
    let ws = Ws::new();
    ws.expect(&[
        ("VarAssigned", "FV T7", "yes"),
        ("VarAssigned", "FV T8", "no"),
    ]);
}

/// Alias safety: `TwoFormals(X, X)` makes `B := false` write `A` too, a `var`
/// formal naming a global the routine writes changes with it, and a call may
/// write the actual. One break each: no reset at a write to another formal;
/// at a write to a compatible global; at a call.
#[test]
fn a_var_formal_may_alias_another_formal_a_global_or_a_callee() {
    let ws = Ws::new();
    ws.expect(&[
        ("SameActual", "FV T9", "yes"),
        ("GlobalActual", "FV T10", "yes"),
        ("CallActual", "FV T11", "yes"),
    ]);
}

/// A global is never tracked. Break: tracking globals as locals.
#[test]
fn a_global_is_never_tracked() {
    let ws = Ws::new();
    ws.expect(&[("GlobalNever", "FV T12", "yes")]);
}

/// r3 E1's last rule, wired by E2: a caller's tracked local binds a callee's
/// Integer parameter. Break: no binding from the caller's flow.
#[test]
fn a_callers_tracked_local_binds_a_value_parameter() {
    let ws = Ws::new();
    ws.expect(&[
        ("FromLocal", "FV T13", "no"),
        ("FromLocal", "FV T14", "yes"),
    ]);
}

/// Publisher `var` parameters keep a value only when every loaded subscriber
/// is proven unable to change it: no subscriber, two that only read it, or
/// one taking it by value keep it; one that assigns it, one that forwards it
/// `var` to a helper, or one writer among readers do not. Breaks: a publisher
/// pass always keeping the value (the writers' rows turn `no`); never keeping
/// it (the readers' rows turn `yes`).
#[test]
fn a_publisher_keeps_a_value_only_when_no_loaded_subscriber_changes_it() {
    let ws = Ws::new();
    ws.expect(&[
        ("PubNone", "FV T15", "no"),
        ("PubAssigning", "FV T16", "yes"),
        ("PubForwarding", "FV T17", "yes"),
        ("PubTwoReaders", "FV T18", "no"),
        ("PubReaderAndWriter", "FV T19", "yes"),
        ("PubByValueSubscriber", "FV T20", "no"),
    ]);
}

/// A `no` that rests on the publisher keep holds only in the
/// loaded-subscribers world: it says so, and names the subscribers
/// considered. Break: the strict world counting the row as ruled out too
/// (the answer is no longer labelled).
#[test]
fn a_no_that_rests_on_loaded_subscribers_states_it() {
    let ws = Ws::new();
    let r = ws.prove("PubNone", "writes-table:FV T15");
    assert_eq!(r["answer"], "no", "{r:#}");
    assert_eq!(
        r["assumptions"],
        serde_json::json!(["loaded-subscribers"]),
        "{r:#}"
    );
    let a = &r["subscriberAssumptions"][0];
    assert_eq!(a["parameter"], "Value", "{r:#}");
    assert!(
        a["publisher"]
            .as_str()
            .unwrap_or_default()
            .ends_with("::OnNoSubscriber"),
        "{r:#}"
    );
    assert_eq!(a["subscribers"], serde_json::json!([]), "{r:#}");
    let r = ws.prove("PubTwoReaders", "writes-table:FV T18");
    let subs = &r["subscriberAssumptions"][0]["subscribers"];
    assert_eq!(subs.as_array().map(Vec::len), Some(2), "{r:#}");
}

/// Other writes: `Evaluate`, a call not resolved, and a `for` counter make a
/// value unknown. One break each.
#[test]
fn evaluate_an_unresolved_call_and_a_for_counter_write_a_value() {
    let ws = Ws::new();
    ws.expect(&[
        ("EvaluateWrites", "FV T22", "yes"),
        ("UnresolvedWrites", "FV T23", "yes"),
        ("ForCounter", "FV T25", "yes"),
    ]);
}

/// `if not Preview then exit;` on a derived value guards what follows.
#[test]
fn an_exit_guard_on_a_derived_value_prunes_what_follows() {
    let ws = Ws::new();
    ws.expect(&[("ExitGuardTrue", "FV T24", "no")]);
}

/// n01: the COMMIT behind `if IsEmailPreview` cannot run from `SendNow`
/// (UseSMTP = true, no loaded subscriber of the publisher), and the answer
/// names the assumption; from `SendPreview` it can, and its condition names
/// the route's guards: the evaluated ones by value, the others not evaluated
/// (never the `IsHandled pattern` class label).
#[test]
fn n01_the_preview_commit_follows_the_publishers_var_parameter() {
    let ws = Ws::new();
    let now = ws.prove("SendNow", "may-commit");
    assert_eq!(now["answer"], "no", "{now:#}");
    assert_eq!(
        now["assumptions"],
        serde_json::json!(["loaded-subscribers"]),
        "{now:#}"
    );
    let preview = ws.prove("SendPreview", "may-commit");
    assert_eq!(preview["answer"], "yes", "{preview:#}");
    let text = ws.prove_human("SendPreview", "may-commit");
    let cond = text
        .lines()
        .find(|l| l.trim_start().starts_with("condition:"))
        .unwrap_or_else(|| panic!("a condition line: {text}"));
    // `IsHandled` is a local passed `var` to a publisher no loaded subscriber
    // changes: still `false`.
    for want in [
        "IsHandled: false, evaluated",
        "IsEmailPreview: true, evaluated",
        "Modal: must be true, not evaluated",
    ] {
        assert!(cond.contains(want), "{want}: {cond}");
    }
    assert!(!cond.contains("IsHandled pattern"), "{cond}");
    let now_text = ws.prove_human("SendNow", "may-commit");
    assert!(
        now_text
            .contains("OnBeforeCreateAndSend has no loaded subscriber, so UseSMTP keeps its value"),
        "{now_text}"
    );
}

/// A guard the flow cannot evaluate stays in the condition as not evaluated;
/// one it evaluates is printed by value; one whose value may be either is
/// not evaluated. Break: a guard counted evaluated when its value merely
/// may let the route run (`Both`).
#[test]
fn an_unevaluated_guard_stays_in_the_condition() {
    let ws = Ws::new();
    let r = ws.prove("Unevaluated", "writes-table:FV T21");
    assert_eq!(r["answer"], "yes", "{r:#}");
    let text = ws.prove_human("Unevaluated", "writes-table:FV T21");
    let cond = text
        .lines()
        .find(|l| l.trim_start().starts_with("condition:"))
        .unwrap_or_else(|| panic!("a condition line: {text}"));
    assert!(cond.contains("On: true, evaluated"), "{cond}");
    assert!(cond.contains("Flag: must be true, not evaluated"), "{cond}");
    // A guard whose value may be either is not evaluated either.
    assert!(cond.contains("Both: must be true, not evaluated"), "{cond}");
}
