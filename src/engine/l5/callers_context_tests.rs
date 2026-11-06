//! Fixtures for the callers' context questions (agent-tasks design §6.8).
//! Each workspace is written as AL source and answered through `alsem
//! prove`'s own job (the driver builds the cross-app program, its sited graph
//! and the model), so a test pins the USE, not a helper.

/// A one-app workspace (no dependency) with `files` under `src/`.
fn ws(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("app.json"),
        r#"{"id":"0a5e0000-0000-0000-0000-00000000a001","name":"CallersWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(tmp.path().join("src").join(name), text).unwrap();
    }
    tmp
}

/// `alsem prove <ws> <routine> <question> --format json`, parsed.
fn prove_json(
    ws: &std::path::Path,
    routine: &str,
    question: &str,
    no_deps: bool,
) -> serde_json::Value {
    let (json, _) = prove_both(ws, routine, question, no_deps);
    json
}

fn prove_both(
    ws: &std::path::Path,
    routine: &str,
    question: &str,
    no_deps: bool,
) -> (serde_json::Value, String) {
    // The binary's read for prove (`FINGERPRINT_ROWS`).
    let needs = crate::program::inputs::WorkspaceNeeds {
        build_only: false,
        fingerprint_rows: true,
    };
    let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &needs);
    let job = crate::engine::l5::prove::ProveJob {
        detectors: crate::engine::l5::detectors::default_detectors(),
        inputs: &inputs,
        routine_selector: routine,
        question_text: question,
        alsem_ver: "test",
        deterministic: true,
        single_app: false,
        closed_world: false,
        assume_no_dependents: no_deps,
        human: false,
        detail: true,
        out: None,
        slice: None,
    };
    let r = crate::engine::batch::run_doc(&job)
        .one::<Result<crate::engine::l5::prove::ProveRunResult, String>>()
        .unwrap()
        .unwrap_or_else(|e| panic!("prove failed: {e}"));
    assert_eq!(r.exit_code, 0, "answered: {}", r.human_text);
    (serde_json::from_str(&r.json_text).unwrap(), r.human_text)
}

fn answer(v: &serde_json::Value) -> &str {
    v["payload"]["result"]["answer"].as_str().unwrap()
}

fn kinds(v: &serde_json::Value) -> Vec<String> {
    v["payload"]["result"]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

fn blocked(v: &serde_json::Value) -> Vec<String> {
    v["payload"]["result"]["blockedByCounts"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

fn evidence_lines(v: &serde_json::Value) -> Vec<u64> {
    v["payload"]["result"]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["evidence"]["line"].as_u64())
        .collect()
}

const TABLE: &str = r#"table 50100 "Pw Rec"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}
"#;

const TARGET: &str = r#"codeunit 50101 "Pw Target"
{
    procedure R()
    begin
    end;
}
"#;

/// A-1, A-1b, A-1c, A-2, A-3, A-3b: one caller per rule, each a local
/// procedure of an internal codeunit, so nothing outside blocks a `no`.
const CALLERS: &str = r#"codeunit 50102 "Pw Callers"
{
    Access = Internal;

    local procedure WriteThenCall()
    var
        Rec: Record "Pw Rec";
        Target: Codeunit "Pw Target";
    begin
        Rec.Modify();
        Target.R();
    end;

    local procedure CallThenWrite()
    var
        Rec: Record "Pw Rec";
        Target: Codeunit "Pw Target";
    begin
        Target.R();
        Rec.Modify();
    end;
}
"#;

#[test]
fn pending_write_before_the_call_is_a_witness_after_is_not() {
    // A-1: the write before the call is the evidence (line 10); the caller
    // that writes after the call gives none.
    let w = ws(&[
        ("Rec.al", TABLE),
        ("Target.al", TARGET),
        ("Callers.al", CALLERS),
    ]);
    let v = prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    assert_eq!(answer(&v), "yes", "{v:#}");
    let ev = &v["payload"]["result"]["evidence"];
    let writes: Vec<&serde_json::Value> = ev
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "pending-write")
        .collect();
    assert_eq!(writes.len(), 1, "only WriteThenCall pends: {ev:#}");
    assert_eq!(writes[0]["evidence"]["line"], 10, "{ev:#}");
    assert!(
        writes[0]["path"][0]["display"]
            .as_str()
            .unwrap()
            .ends_with("WriteThenCall"),
        "{ev:#}"
    );
}

/// An internal codeunit `name` (id `id`) holding `body` (procedures and
/// globals), so its local procedures have no external caller.
fn cu(id: u32, name: &str, body: &str) -> String {
    format!("codeunit {id} \"{name}\"\n{{\n    Access = Internal;\n{body}\n}}\n")
}

/// One local procedure `P` with a `Rec` and a `Target` variable.
fn caller(id: u32, name: &str, attrs: &str, stmts: &str) -> String {
    cu(
        id,
        name,
        &format!(
            "    {attrs}\n    local procedure P()\n    var\n        Rec: Record \"Pw Rec\";\n        \
             Target: Codeunit \"Pw Target\";\n    begin\n{stmts}\n    end;\n"
        ),
    )
}

fn pending(files: &[(&str, &str)]) -> serde_json::Value {
    let mut all = vec![("Rec.al", TABLE), ("Target.al", TARGET)];
    all.extend_from_slice(files);
    let w = ws(&all);
    prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    )
}

#[test]
fn a_caller_that_writes_only_after_the_call_answers_no() {
    // A-1 (no side): the whole cone is seen (no_deps), nothing pends.
    let c = caller(
        50110,
        "C1",
        "",
        "        Target.R();\n        Rec.Modify();",
    );
    let v = pending(&[("C.al", &c)]);
    assert_eq!(answer(&v), "no", "{v:#}");
}

#[test]
fn a_loop_counts_a_write_after_the_call_in_an_earlier_iteration() {
    // A-1b.
    let c = caller(
        50110,
        "C1",
        "",
        "        repeat\n            Target.R();\n            Rec.Modify();\n        until Rec.Next() = 0;",
    );
    let v = pending(&[("C.al", &c)]);
    assert_eq!(answer(&v), "yes", "{v:#}");
    assert_eq!(kinds(&v), ["pending-write"], "{v:#}");
}

#[test]
fn a_commit_between_the_write_and_the_call_clears_it() {
    // A-1c: Modify; Commit; R -> nothing pending from this caller.
    let c = caller(
        50110,
        "C1",
        "",
        "        Rec.Modify();\n        Commit();\n        Target.R();",
    );
    let v = pending(&[("C.al", &c)]);
    assert_eq!(answer(&v), "no", "{v:#}");
    assert_eq!(v["payload"]["result"]["stopped"]["commit"], 1, "{v:#}");
}

#[test]
fn a_tryfunction_caller_blocks_no_on_its_own_row() {
    // A-1d: the tree walk would say nothing pends (the write is after); the
    // owner's rule says a TryFunction caller is not modelled.
    let c = cu(
        50110,
        "C1",
        "    [TryFunction]\n    local procedure P()\n    var\n        Rec: Record \"Pw Rec\";\n        \
         Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n        Rec.Modify();\n    \
         end;\n",
    );
    let (v, human) = prove_both(
        ws(&[("Rec.al", TABLE), ("Target.al", TARGET), ("C.al", &c)]).path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert_eq!(blocked(&v), ["tryfunction-caller"], "{v:#}");
    assert!(human.contains("TryFunction callers: 1"), "{human}");
}

#[test]
fn a_callee_before_the_call_that_writes_counts_one_after_does_not() {
    // A-2: `W(); R();` pends through W's cone; `R(); W();` does not.
    let w = cu(
        50111,
        "Pw Writer",
        "    procedure W()\n    var\n        Rec: Record \"Pw Rec\";\n    begin\n        Rec.Modify();\n    end;\n",
    );
    let before = caller(50110, "C1", "", "        Writer.W();\n        Target.R();").replace(
        "Target: Codeunit \"Pw Target\";",
        "Target: Codeunit \"Pw Target\";\n        Writer: Codeunit \"Pw Writer\";",
    );
    let v = pending(&[("W.al", &w), ("C.al", &before)]);
    assert_eq!(answer(&v), "yes", "{v:#}");
    let ev = &v["payload"]["result"]["evidence"][0];
    assert_eq!(ev["evidence"]["line"], 11, "the call to W: {v:#}");
    let after = before.replace(
        "        Writer.W();\n        Target.R();",
        "        Target.R();\n        Writer.W();",
    );
    let v = pending(&[("W.al", &w), ("C.al", &after)]);
    assert_eq!(answer(&v), "no", "{v:#}");
}

/// A table whose `OnModify` calls R.
const TRIGGER_TABLE: &str = r#"table 50100 "Pw Rec"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    trigger OnModify()
    var
        Target: Codeunit "Pw Target";
    begin
        Target.R();
    end;
}
"#;

#[test]
fn a_trigger_of_a_physical_write_is_inside_write_a_temporary_one_is_not() {
    // A-3.
    let phys = caller(50110, "C1", "", "        Rec.Modify(true);");
    let w = ws(&[
        ("Rec.al", TRIGGER_TABLE),
        ("Target.al", TARGET),
        ("C.al", &phys),
    ]);
    let v = prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    assert_eq!(answer(&v), "yes", "{v:#}");
    assert!(kinds(&v).contains(&"inside-write".to_string()), "{v:#}");
    let temp = phys.replace("Record \"Pw Rec\";", "Record \"Pw Rec\" temporary;");
    let w = ws(&[
        ("Rec.al", TRIGGER_TABLE),
        ("Target.al", TARGET),
        ("C.al", &temp),
    ]);
    let v = prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    assert!(
        !kinds(&v).contains(&"inside-write".to_string()),
        "a temporary record's trigger proves no transaction: {v:#}"
    );
    assert_ne!(answer(&v), "yes", "{v:#}");
}

#[test]
fn a_checked_codeunit_run_between_the_writer_and_r_stops_the_path() {
    // A-3b: C writes, then runs `Pw Runner` checked; its OnRun calls R.
    let runner = "codeunit 50112 \"Pw Runner\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
                  Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n    end;\n}\n";
    let c = caller(
        50110,
        "C1",
        "",
        "        Rec.Modify();\n        if Codeunit.Run(Codeunit::\"Pw Runner\") then;",
    );
    let v = pending(&[("Runner.al", runner), ("C.al", &c)]);
    assert_ne!(answer(&v), "yes", "{v:#}");
    assert_eq!(v["payload"]["result"]["stopped"]["checked-run"], 1, "{v:#}");
}

// ---------------------------------------------------------------------------
// callers-intercept-errors
// ---------------------------------------------------------------------------

/// R raises; `T` is a TryFunction calling R.
const TRY: &str = "codeunit 50113 \"Pw Try\"\n{\n    Access = Internal;\n    [TryFunction]\n    \
                   procedure T()\n    var\n        Target: Codeunit \"Pw Target\";\n    begin\n        \
                   Target.R();\n    end;\n}\n";

fn intercept(files: &[(&str, &str)], no_deps: bool) -> (serde_json::Value, String) {
    let mut all = vec![("Rec.al", TABLE), ("Target.al", TARGET)];
    all.extend_from_slice(files);
    let w = ws(&all);
    prove_both(
        w.path(),
        "Pw Target::R",
        "callers-intercept-errors",
        no_deps,
    )
}

#[test]
fn only_the_consumed_tryfunction_call_intercepts() {
    // A-4: `if T() then` (line 8) traps; `T();` (line 9) is an ordinary call.
    let c = cu(
        50110,
        "C1",
        "    local procedure P()\n    var\n        Tr: Codeunit \"Pw Try\";\n    begin\n        \
         if Tr.T() then;\n        Tr.T();\n    end;\n",
    );
    let (v, human) = intercept(&[("Try.al", TRY), ("C.al", &c)], true);
    assert_eq!(answer(&v), "yes", "{v:#}");
    let traps: Vec<u64> = evidence_lines(&v);
    assert_eq!(kinds(&v), ["catch-and-return-false"], "{v:#}");
    assert_eq!(traps, [8], "{v:#}");
    assert!(
        human.contains("not intercepted: Tr.T called as a statement"),
        "{human}"
    );
    assert!(
        human.contains("does not roll back database changes"),
        "{human}"
    );
}

#[test]
fn a_call_in_a_tryfunction_calls_arguments_is_not_trapped_by_it() {
    // A-5: `if Tr.T2(Target.RB()) then`: RB runs in C, before T2.
    let target = "codeunit 50101 \"Pw Target\"\n{\n    procedure R()\n    begin\n    end;\n\n    \
                  procedure RB(): Boolean\n    begin\n        R();\n    end;\n}\n";
    let try2 = "codeunit 50113 \"Pw Try\"\n{\n    Access = Internal;\n    [TryFunction]\n    \
                procedure T2(B: Boolean)\n    begin\n    end;\n}\n";
    let c = cu(
        50110,
        "C1",
        "    local procedure P()\n    var\n        Tr: Codeunit \"Pw Try\";\n        Target: Codeunit \
         \"Pw Target\";\n    begin\n        if Tr.T2(Target.RB()) then;\n    end;\n",
    );
    let w = ws(&[
        ("Rec.al", TABLE),
        ("Target.al", target),
        ("Try.al", try2),
        ("C.al", &c),
    ]);
    let (v, _) = prove_both(w.path(), "Pw Target::RB", "callers-intercept-errors", true);
    assert!(
        !kinds(&v).contains(&"catch-and-return-false".to_string()),
        "{v:#}"
    );
    assert_eq!(answer(&v), "no", "{v:#}");
}

#[test]
fn an_error_behavior_collect_caller_is_collectible_not_a_catch() {
    // A-6.
    let c = cu(
        50110,
        "C1",
        "    [ErrorBehavior(ErrorBehavior::Collect)]\n    local procedure P()\n    var\n        \
         Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n    end;\n",
    );
    let (v, _) = intercept(&[("C.al", &c)], true);
    assert_eq!(kinds(&v), ["collectible"], "{v:#}");
}

#[test]
fn asserterror_intercepts() {
    let c = "codeunit 50114 \"Pw Test\"\n{\n    Subtype = Test;\n    [Test]\n    procedure TestR()\n    \
             var\n        Target: Codeunit \"Pw Target\";\n    begin\n        asserterror Target.R();\n    \
             end;\n}\n";
    let (v, _) = intercept(&[("T.al", c)], true);
    assert_eq!(kinds(&v), ["asserterror"], "{v:#}");
}

#[test]
fn a_trap_above_a_background_start_does_not_intercept() {
    // A-7: C runs `Pw Starter` checked; Starter starts `Pw Bg` in a new
    // session; Bg's OnRun calls R. Since callgraph F3 the sited graph has
    // the `StartSession` site's edge to Bg (a `Background` site), so the
    // walk reaches the starter and the stop is what keeps C's trap out.
    // Discrimination proof (2026-11-05, F3): with the stop disabled this
    // test fails (a `catch-and-return-false` witness from C); restored, it
    // passes. Bg's `background-start` entry is R0's, from that site.
    let starter = "codeunit 50115 \"Pw Starter\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
                   S: Integer;\n    begin\n        StartSession(S, Codeunit::\"Pw Bg\");\n    end;\n}\n";
    let bg = "codeunit 50116 \"Pw Bg\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
              Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n    end;\n}\n";
    let c = cu(
        50110,
        "C1",
        "    local procedure P()\n    begin\n        if Codeunit.Run(Codeunit::\"Pw Starter\") then;\n    \
         end;\n",
    );
    let (v, _) = intercept(&[("S.al", starter), ("Bg.al", bg), ("C.al", &c)], true);
    assert!(
        !kinds(&v).contains(&"catch-and-return-false".to_string()),
        "{v:#}"
    );
    assert_eq!(
        v["payload"]["result"]["entries"]["background-start"], 1,
        "{v:#}"
    );
}

// ---------------------------------------------------------------------------
// callers-may-run-noninteractive
// ---------------------------------------------------------------------------

/// An install codeunit calling `RIn` under `if GuiAllowed`, `RElse` in the
/// else branch, `RShape` under a condition of another shape. (Not `RUn`: a
/// codeunit variable's `.RUn()` is `Codeunit.Run`.)
const INSTALL: &str = r#"codeunit 50117 "Pw Install"
{
    Subtype = Install;
    Access = Internal;

    trigger OnInstallAppPerCompany()
    var
        Target: Codeunit "Pw Gui Target";
        Flag: Boolean;
    begin
        if GuiAllowed then
            Target.RIn();
        if GuiAllowed() then
            Flag := true
        else
            Target.RElse();
        if Flag or GuiAllowed() then
            Target.RShape();
    end;
}
"#;

const GUI_TARGET: &str = r#"codeunit 50118 "Pw Gui Target"
{
    Access = Internal;
    procedure RIn()
    begin
    end;
    procedure RElse()
    begin
    end;
    procedure RShape()
    begin
    end;
}
"#;

fn noninteractive(files: &[(&str, &str)], routine: &str, config: bool) -> serde_json::Value {
    let w = ws(files);
    if config {
        std::fs::write(
            w.path().join("roots.config.json"),
            r#"{"version":1,"roots":[]}"#,
        )
        .unwrap();
    }
    prove_json(w.path(), routine, "callers-may-run-noninteractive", true)
}

#[test]
fn gui_allowed_guards_place_the_call() {
    // A-8.
    let files = [("I.al", INSTALL), ("G.al", GUI_TARGET)];
    let v = noninteractive(&files, "Pw Gui Target::RIn", true);
    assert!(kinds(&v).is_empty(), "inside GuiAllowed: {v:#}");
    assert_eq!(answer(&v), "no", "{v:#}");
    assert_eq!(v["payload"]["result"]["stopped"]["gui-allowed"], 1, "{v:#}");
    let v = noninteractive(&files, "Pw Gui Target::RElse", true);
    assert_eq!(kinds(&v), ["non-interactive-entry"], "else branch: {v:#}");
    let v = noninteractive(&files, "Pw Gui Target::RShape", true);
    assert_eq!(answer(&v), "yes", "{v:#}");
    let unrecognized = v["payload"]["result"]["unrecognizedGuards"]
        .as_array()
        .unwrap();
    assert_eq!(unrecognized.len(), 1, "{v:#}");
    // The place is the condition's line (`if Flag or GuiAllowed() then`,
    // line 17 of I.al), not the guarded call's (line 18; c02 printed the
    // call's line as the condition's).
    let place = unrecognized[0].as_str().unwrap();
    assert!(place.ends_with("I.al:17"), "{place}");
}

#[test]
fn a_literal_start_session_target_is_non_interactive_a_variable_one_blocks_no() {
    // A-9.
    let starter = "codeunit 50115 \"Pw Starter\"\n{\n    Access = Internal;\n    local procedure Go()\n    \
                   var\n        S: Integer;\n    begin\n        StartSession(S, Codeunit::\"Pw Bg\");\n    \
                   end;\n}\n";
    let bg = "codeunit 50116 \"Pw Bg\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
              Target: Codeunit \"Pw Gui Target\";\n    begin\n        Target.RIn();\n    end;\n}\n";
    let v = noninteractive(
        &[("S.al", starter), ("Bg.al", bg), ("G.al", GUI_TARGET)],
        "Pw Gui Target::RIn",
        true,
    );
    assert_eq!(answer(&v), "yes", "{v:#}");
    assert_eq!(
        v["payload"]["result"]["evidence"][0]["entry"]["kind"], "background-start",
        "{v:#}"
    );
    // A variable target anywhere blocks a `no` for an unrelated routine.
    let var_starter = starter
        .replace("Codeunit::\"Pw Bg\"", "Id")
        .replace("S: Integer;", "S: Integer;\n        Id: Integer;");
    let v = noninteractive(
        &[("S.al", &var_starter), ("G.al", GUI_TARGET)],
        "Pw Gui Target::RElse",
        true,
    );
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert_eq!(blocked(&v), ["unresolved-background-target"], "{v:#}");
}

#[test]
fn no_roots_config_blocks_no() {
    // A-10.
    let files = [("G.al", GUI_TARGET)];
    let v = noninteractive(&files, "Pw Gui Target::RIn", false);
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert_eq!(blocked(&v), ["publication-unknown"], "{v:#}");
    let v = noninteractive(&files, "Pw Gui Target::RIn", true);
    assert_eq!(answer(&v), "no", "{v:#}");
}

// ---------------------------------------------------------------------------
// Shared: external and hidden callers
// ---------------------------------------------------------------------------

#[test]
fn a_public_routine_is_open_world_unless_assume_no_dependents() {
    // A-11: R is public on a public codeunit.
    let w = ws(&[("Rec.al", TABLE), ("Target.al", TARGET)]);
    let v = prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        false,
    );
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert_eq!(blocked(&v), ["external-callers"], "{v:#}");
    let v = prove_json(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    assert_eq!(answer(&v), "no", "{v:#}");
    assert_eq!(
        v["payload"]["result"]["assumptions"],
        serde_json::json!(["assume-no-dependents"]),
        "{v:#}"
    );
}

#[test]
fn an_internal_routine_of_an_app_with_a_friend_is_external() {
    // A-11 (friend): internal codeunit, app names an internalsVisibleTo friend.
    let target = "codeunit 50101 \"Pw Target\"\n{\n    Access = Internal;\n    procedure R()\n    begin\n    \
                  end;\n}\n";
    let w = ws(&[("Target.al", target)]);
    let v = prove_json(w.path(), "Pw Target::R", "callers-intercept-errors", false);
    assert_eq!(answer(&v), "no", "no friend: {v:#}");
    let app = std::fs::read_to_string(w.path().join("app.json")).unwrap();
    let with_friend = app.replace(
        r#""runtime""#,
        r#""internalsVisibleTo":[{"id":"0a5e0000-0000-0000-0000-00000000f00d","name":"Friend","publisher":"probe"}],"runtime""#,
    );
    assert_ne!(app, with_friend, "precondition: the friend was added");
    std::fs::write(w.path().join("app.json"), with_friend).unwrap();
    let v = prove_json(w.path(), "Pw Target::R", "callers-intercept-errors", false);
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert_eq!(blocked(&v), ["external-callers"], "{v:#}");
}

#[test]
fn a_hidden_caller_in_the_cone_blocks_no() {
    // A-12: R's only caller subscribes to an event nobody raises.
    let p = "codeunit 50119 \"Pw Pub\"\n{\n    Access = Internal;\n    [IntegrationEvent(false, false)]\n    \
             procedure OnSomething()\n    begin\n    end;\n}\n";
    let s = "codeunit 50120 \"Pw Sub\"\n{\n    Access = Internal;\n    \
             [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Pw Pub\", 'OnSomething', '', false, false)]\n    \
             local procedure Handle()\n    var\n        Target: Codeunit \"Pw Target\";\n    begin\n        \
             Target.R();\n    end;\n}\n";
    let (v, _) = intercept(&[("P.al", p), ("S.al", s)], true);
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert!(blocked(&v).contains(&"hidden-callers".to_string()), "{v:#}");
}

#[test]
fn a_page_opened_after_a_write_runs_its_action_in_a_later_user_event() {
    // C writes, then opens `Pw Page`; the page's action calls R when the
    // user clicks it, after C has finished: C's write is not pending at R.
    let page = "page 50121 \"Pw Page\"\n{\n    actions\n    {\n        area(Processing)\n        {\n            \
                action(Go)\n            {\n                trigger OnAction()\n                var\n                    \
                Target: Codeunit \"Pw Target\";\n                begin\n                    Target.R();\n                \
                end;\n            }\n        }\n    }\n}\n";
    let c = caller(
        50110,
        "C1",
        "",
        "        Rec.Modify();\n        Page.Run(Page::\"Pw Page\");",
    );
    let v = pending(&[("Page.al", page), ("C.al", &c)]);
    assert_eq!(v["payload"]["result"]["stopped"]["page-event"], 1, "{v:#}");
    assert_ne!(answer(&v), "yes", "{v:#}");
}

#[test]
fn a_witness_only_through_a_hub_is_listed_but_not_a_yes() {
    // C writes, then runs a codeunit by a variable id: the run hub stands for
    // every codeunit's OnRun, `Pw Runner`'s too, which calls R. The pending
    // write is real only if the id is that codeunit's: an open witness.
    let runner = "codeunit 50112 \"Pw Runner\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
                  Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n    end;\n}\n";
    let c = cu(
        50110,
        "C1",
        "    local procedure P(Id: Integer)\n    var\n        Rec: Record \"Pw Rec\";\n    begin\n        \
         Rec.Modify();\n        Codeunit.Run(Id);\n    end;\n",
    );
    let v = pending(&[("Runner.al", runner), ("C.al", &c)]);
    assert!(
        v["payload"]["result"]["evidenceThroughOpenSites"]
            .as_u64()
            .unwrap()
            >= 1,
        "precondition: the witness goes through the run hub: {v:#}"
    );
    assert_eq!(answer(&v), "unknown", "{v:#}");
}

/// Output validation V2 (§3.1, F10): a witness that passes a hub is a
/// candidate, never evidence. The human answer prints `established
/// witnesses: none` before it, labels the candidate count, starts the
/// witness line with its tier and names the hub with its site; the JSON
/// counts the two classes apart. Discrimination (2026-11-06): counting every
/// witness as established in `format_human` (`let established =
/// a.witness_count;`) fails this test; restored, it passes.
#[test]
fn a_hub_witness_is_a_candidate_never_an_established_witness() {
    let runner = "codeunit 50112 \"Pw Runner\"\n{\n    Access = Internal;\n    trigger OnRun()\n    var\n        \
                  Target: Codeunit \"Pw Target\";\n    begin\n        Target.R();\n    end;\n}\n";
    let c = cu(
        50110,
        "C1",
        "    local procedure P(Id: Integer)\n    var\n        Rec: Record \"Pw Rec\";\n    begin\n        \
         Rec.Modify();\n        Codeunit.Run(Id);\n    end;\n",
    );
    let w = ws(&[
        ("Rec.al", TABLE),
        ("Target.al", TARGET),
        ("Runner.al", runner),
        ("C.al", &c),
    ]);
    let (v, human) = prove_both(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        true,
    );
    let r = &v["payload"]["result"];
    let candidates = r["candidateWitnesses"]["count"].as_u64().unwrap();
    assert!(candidates >= 1, "precondition: a hub witness: {r:#}");
    assert_eq!(r["establishedWitnesses"], 0, "{r:#}");
    assert_eq!(r["evidence"][0]["pathTier"], "via-hub", "{r:#}");
    assert!(human.contains("  established witnesses: none\n"), "{human}");
    assert!(
        human.contains(&format!("  candidates over a hub: {candidates}")),
        "{human}"
    );
    assert!(human.contains("    [via-hub] pending-write: "), "{human}");
    assert!(
        human.contains("without a hub was established in the analyzed graph")
            && human.contains("[hub Codeunit.Run of a variable at ws:src/C.al:"),
        "{human}"
    );
    assert!(!human.contains("onlyOpen"), "{human}");
}

/// Output validation V2 (§4.4, F19): an `unknown` stays `unknown` and names
/// the input that removes a blocker kind, saying `removes every blocker of
/// this answer` only when it does; otherwise it names what stays. R is
/// public (external callers) and its one caller subscribes to an event
/// nobody raises (hidden callers): `assumeNoDependents` removes the first
/// kind only. Discrimination (2026-11-06): printing `removes every blocker`
/// whatever stays (`if true` in `reanalysis_line`) fails the second half;
/// restored, it passes.
#[test]
fn an_unknown_names_the_assumption_and_what_it_removes() {
    let w = ws(&[("Rec.al", TABLE), ("Target.al", TARGET)]);
    let (v, human) = prove_both(
        w.path(),
        "Pw Target::R",
        "callers-have-pending-writes",
        false,
    );
    assert_eq!(answer(&v), "unknown", "{v:#}");
    assert!(
        human.contains("answer:   unknown (blocked by: external callers 1)"),
        "{human}"
    );
    assert!(human.contains("  plain reading: no pending write was found in the analyzed callers"));
    assert!(human.contains("  not established: that no caller has a write pending"));
    assert!(
        human.contains(
            "re-analysis under an assumption: assumeNoDependents: true — assumes no other app \
             calls this routine; removes every blocker of this answer (external callers 1)"
        ),
        "{human}"
    );
    // Two kinds: the assumption removes one, the other stays.
    let p = "codeunit 50119 \"Pw Pub\"\n{\n    Access = Internal;\n    [IntegrationEvent(false, false)]\n    \
             procedure OnSomething()\n    begin\n    end;\n}\n";
    let s = "codeunit 50120 \"Pw Sub\"\n{\n    Access = Internal;\n    \
             [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Pw Pub\", 'OnSomething', '', false, false)]\n    \
             local procedure Handle()\n    var\n        Target: Codeunit \"Pw Target\";\n    begin\n        \
             Target.R();\n    end;\n}\n";
    let (v, human) = intercept(&[("P.al", p), ("S.al", s)], false);
    assert_eq!(answer(&v), "unknown", "{v:#}");
    let b = blocked(&v);
    assert!(
        b.contains(&"external-callers".to_string()) && b.contains(&"hidden-callers".to_string()),
        "precondition: two kinds: {b:?}"
    );
    assert!(
        human.contains("removes external callers 1; stay: hidden callers 1"),
        "{human}"
    );
    assert!(!human.contains("removes every blocker"), "{human}");
    assert!(
        human.contains("  not established: that no caller turns this error into false"),
        "{human}"
    );
}

#[test]
fn a_started_codeunits_commit_ui_and_error_are_not_the_starters() {
    // F3 decision: effects beyond a background start run in another session,
    // so they are never the starter's own commits, UI or errors (the start
    // itself stays a BACKGROUND_TASK effect). The model's calls do not carry
    // the start's edge. LIMIT (discrimination proof, 2026-11-05): no
    // single-line break leaks it, because three independent layers keep the
    // edge out of the model's calls: its `Background` obligation (the join
    // reads `CallSite` ones only), no site key, and the first program edge
    // per key wins (the catalog call comes first). The test pins the answer.
    let starter = "codeunit 50115 \"Pw Starter\"\n{\n    Access = Internal;\n    local procedure Go()\n    \
                   var\n        S: Integer;\n    begin\n        StartSession(S, Codeunit::\"Pw Bg\");\n    \
                   end;\n}\n";
    let bg = "codeunit 50116 \"Pw Bg\"\n{\n    Access = Internal;\n    trigger OnRun()\n    begin\n        \
              Commit();\n        Message('x');\n        Error('y');\n    end;\n}\n";
    let w = ws(&[("S.al", starter), ("Bg.al", bg)]);
    for q in ["may-commit", "reaches-ui", "throws-error"] {
        let v = prove_json(w.path(), "Pw Starter::Go", q, true);
        assert_eq!(answer(&v), "no", "{q}: {v:#}");
        let v = prove_json(w.path(), "Pw Bg::OnRun", q, true);
        assert_eq!(answer(&v), "yes", "precondition: Bg itself does it ({q})");
    }
}
