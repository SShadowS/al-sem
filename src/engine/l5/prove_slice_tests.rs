//! Fixtures for prove's filters (agent-tasks design §9.5, E1). Each workspace
//! is written as AL source and answered through `alsem prove`'s own job (the
//! driver builds the cross-app program, the tree and the slice), so a test
//! pins the USE, not a helper. Every honesty rule of the module doc has a
//! test that fails when the rule is broken.

use crate::engine::l5::prove_slice::SliceRequest;

/// A one-app workspace (no dependency) with `files` under `src/`.
fn ws(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("app.json"),
        r#"{"id":"0a5e0000-0000-0000-0000-00000000e001","name":"SliceWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(tmp.path().join("src").join(name), text).unwrap();
    }
    tmp
}

fn job<'a>(
    inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    routine: &'a str,
    question: &'a str,
    slice: Option<SliceRequest>,
    human: bool,
) -> crate::engine::l5::prove::ProveJob<'a> {
    crate::engine::l5::prove::ProveJob {
        detectors: crate::engine::l5::detectors::default_detectors(),
        inputs,
        routine_selector: routine,
        question_text: question,
        alsem_ver: "test",
        deterministic: true,
        single_app: false,
        closed_world: false,
        assume_no_dependents: false,
        human,
        detail: true,
        out: None,
        slice,
        surface: Default::default(),
    }
}

fn inputs_of(ws: &std::path::Path) -> std::sync::Arc<crate::program::inputs::WorkspaceInputs> {
    // The binary's read for prove (`FINGERPRINT_ROWS`).
    let needs = crate::program::inputs::WorkspaceNeeds {
        build_only: false,
        fingerprint_rows: true,
    };
    crate::program::inputs::WorkspaceInputs::read(ws, &needs)
}

/// `alsem prove <ws> <routine> <question> [filters]`: the JSON and the human
/// text.
fn prove(
    ws: &std::path::Path,
    routine: &str,
    question: &str,
    m: Option<&str>,
    site: Option<&str>,
    op: Option<&str>,
    table: Option<&str>,
) -> (serde_json::Value, String) {
    let slice = SliceRequest::of(question, m, site, op, table, None).expect("a valid filter");
    let inputs = inputs_of(ws);
    let j = job(&inputs, routine, question, slice, false);
    let r = crate::engine::batch::run_doc(&j)
        .one::<Result<crate::engine::l5::prove::ProveRunResult, String>>()
        .unwrap()
        .unwrap_or_else(|e| panic!("prove failed: {e}"));
    assert_eq!(r.exit_code, 0, "answered: {}", r.human_text);
    (serde_json::from_str(&r.json_text).unwrap(), r.human_text)
}

fn matching(ws: &std::path::Path, routine: &str, text: &str) -> (serde_json::Value, String) {
    prove(ws, routine, "throws-error", Some(text), None, None, None)
}

fn answer(v: &serde_json::Value) -> &str {
    v["payload"]["result"]["answer"].as_str().unwrap()
}

fn evidence(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v["payload"]["result"]["evidence"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn blocker_kinds(v: &serde_json::Value) -> Vec<String> {
    v["payload"]["result"]["blockedBy"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| b["kind"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The guards of the terminal site of the first evidence row's route.
fn terminal_guards(v: &serde_json::Value) -> Vec<serde_json::Value> {
    let ev = evidence(v);
    let sites = ev[0]["route"]["sites"].as_array().unwrap().clone();
    sites.last().unwrap()["guards"].as_array().unwrap().clone()
}

const TABLE: &str = r#"table 50100 "Sl Rec"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Blocked; Boolean) { }
    }
}
"#;

/// One root per rule, each a local procedure of an internal codeunit (no
/// caller outside), every call resolved: the unfiltered obligations are
/// clear, so a filtered `no` is reachable and only the slice decides.
const CODE: &str = r#"codeunit 50101 "Sl Code"
{
    Access = Internal;

    var
        BlockedErr: Label 'The customer is blocked.';
        IssuedErr: Label 'Document %1 has already been issued.';

    procedure KnownOnly(var Rec: Record "Sl Rec")
    begin
        if Rec.Blocked then
            Error(BlockedErr);
    end;

    procedure WithComputed(var Rec: Record "Sl Rec")
    var
        Msg: Text;
    begin
        Msg := 'The customer is ' + Format(Rec.Blocked);
        if Rec.Blocked then
            Error(BlockedErr);
        Error(Msg);
    end;

    procedure WithPlaceholder(var Rec: Record "Sl Rec")
    begin
        Error(IssuedErr, Rec."No.");
    end;

    procedure ThenElse(var Rec: Record "Sl Rec")
    begin
        if Rec.Blocked then
            ThenSide()
        else
            ElseSide();
    end;

    local procedure ThenSide()
    begin
    end;

    local procedure ElseSide()
    begin
        Error('else side reached');
    end;

    procedure Loops(var Rec: Record "Sl Rec")
    begin
        while Rec.Blocked do
            WhileBody();
        repeat
            RepeatBody();
        until Rec."No." = '';
    end;

    local procedure WhileBody()
    begin
        Error('while body');
    end;

    local procedure RepeatBody()
    begin
        Error('repeat body');
    end;

    procedure Nested(var Rec: Record "Sl Rec"; A: Boolean; B: Boolean)
    begin
        if A then
            if B then
                if Rec.Blocked then
                    Error('three deep');
    end;

    procedure Checks(var Rec: Record "Sl Rec")
    begin
        Rec.TestField(Blocked);
        Error(BlockedErr);
    end;

    procedure Writes()
    var
        Rec: Record "Sl Rec";
    begin
        Rec.Insert();
        Rec.Modify();
    end;

    procedure WritesParam(var Rec: Record "Sl Rec")
    begin
        Rec.Insert();
    end;
}
"#;

fn fixture() -> tempfile::TempDir {
    ws(&[("Table.al", TABLE), ("Code.al", CODE)])
}

/// E-2: `--match` matches a Label's VALUE and a Label's NAME; the evidence
/// row carries the message text and its source. Break: match against the
/// site's anchor text (the `Error(BlockedErr)` excerpt) instead of the
/// message, and "is blocked" stops matching.
#[test]
fn e2_match_by_label_value_and_label_name() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "KnownOnly", "customer is BLOCKED");
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0]["message"]["text"], "The customer is blocked.");
    assert_eq!(ev[0]["message"]["source"], "label");
    assert_eq!(ev[0]["message"]["var"], "BlockedErr");
    assert!(
        human.contains("\"The customer is blocked.\" (Label BlockedErr)"),
        "{human}"
    );
    assert!(
        human.contains(crate::engine::l5::prove_slice::STATIC_LABEL),
        "{human}"
    );
    assert_eq!(v["schemaVersion"], "2.12.0");
    assert_eq!(v["payload"]["filter"]["match"], "customer is BLOCKED");
    let (v, _) = matching(tmp.path(), "KnownOnly", "blockederr");
    assert_eq!(answer(&v), "yes", "the label's name matches");
}

/// E-2 (second half): a site whose message has no static text never
/// matches, even when the text it computes would. `WithComputed`'s
/// `Error(Msg)` builds "The customer is …"; only the Label site matches.
/// Break: treat a computed message's text as its variable's literal
/// assignment, and two sites match.
#[test]
fn e2_a_site_without_static_text_never_matches() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "WithComputed", "The customer is");
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    assert_eq!(ev.len(), 1, "only the Label site matches: {ev:?}");
    assert_eq!(ev[0]["message"]["var"], "BlockedErr");
}

/// E-6: a cone with one computed-message error and no matching site answers
/// `unknown` (`message-unknown`), not `no`; the same filter on a cone whose
/// every message is static answers `no`. Break: treat an unknown text as
/// non-matching, and WithComputed answers `no`.
#[test]
fn e6_an_unknown_message_blocks_a_filtered_no() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "WithComputed", "no such text");
    assert_eq!(answer(&v), "unknown", "{human}");
    assert!(
        blocker_kinds(&v).contains(&"message-unknown".to_string()),
        "{:?}",
        blocker_kinds(&v)
    );
    assert_eq!(
        v["payload"]["slice"]["undecided"][0]["kind"],
        "message-unknown"
    );
    // Every message static: a proven `no`.
    let (v, human) = matching(tmp.path(), "KnownOnly", "no such text");
    assert_eq!(answer(&v), "no", "{human}");
}

/// A `%1` text that does not match blocks a `no`: the placeholder's value
/// may hold the text. One that matches its literal part is a `yes`. Break:
/// drop the placeholder rule, and the first answer is `no`.
#[test]
fn placeholders_block_a_filtered_no() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "WithPlaceholder", "D-1001");
    assert_eq!(answer(&v), "unknown", "{human}");
    assert!(
        blocker_kinds(&v).contains(&"message-placeholders".to_string()),
        "{:?}",
        blocker_kinds(&v)
    );
    let (v, _) = matching(tmp.path(), "WithPlaceholder", "already been issued");
    assert_eq!(answer(&v), "yes");
}

/// E-4: a route through the `else` branch prints `else`. Break: drop the
/// polarity (push both arms as `then`).
#[test]
fn e4_a_route_through_else_prints_else() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "ThenElse", "else side");
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    let first = &ev[0]["route"]["sites"][0]["guards"][0];
    assert_eq!(first["kind"], "else", "{ev:?}");
    assert_eq!(first["construct"], "if");
    assert_eq!(first["condition"], "Rec.Blocked");
    assert!(
        human.contains("else-branch of `if Rec.Blocked then`"),
        "{human}"
    );
}

/// E-7: `while C do R()` gives a loop-entry condition; `repeat R() until C`
/// a continuation condition, labelled apart. Break: label both the same.
#[test]
fn e7_loop_entry_and_continuation_are_labelled_apart() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "Loops", "while body");
    assert_eq!(answer(&v), "yes", "{human}");
    let g = &evidence(&v)[0]["route"]["sites"][0]["guards"][0];
    assert_eq!(g["kind"], "loop-entry", "{g}");
    assert_eq!(g["construct"], "while");
    assert!(
        human.contains("loop-entry condition of `while Rec.Blocked do`"),
        "{human}"
    );
    let (v, human) = matching(tmp.path(), "Loops", "repeat body");
    assert_eq!(answer(&v), "yes", "{human}");
    let g = &evidence(&v)[0]["route"]["sites"][0]["guards"][0];
    assert_eq!(g["kind"], "continuation", "{g}");
    assert_eq!(g["construct"], "repeat");
    assert!(
        human.contains("continuation condition of `repeat … until Rec.\"No.\" = ''`"),
        "{human}"
    );
}

/// E-8: the guards shown are the 2 innermost of 3 enclosing conditions, and
/// the answer says "2 of 3" and that they are selected context. Break: print
/// every guard as complete (no count), or keep the outermost two.
#[test]
fn e8_guards_are_selected_and_counted() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "Nested", "three deep");
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    let sites = ev[0]["route"]["sites"].as_array().unwrap();
    let last = sites.last().unwrap();
    assert_eq!(last["enclosingTotal"], 3, "{last}");
    let g = terminal_guards(&v);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[0]["condition"], "Rec.Blocked", "innermost first");
    assert_eq!(g[1]["condition"], "B");
    assert_eq!(
        ev[0]["route"]["guardsAre"],
        "selected context, not all necessary conditions"
    );
    assert!(
        human.contains(
            "guards shown (2 of 3 enclosing conditions; selected context, not all necessary \
             conditions;"
        ),
        "{human}"
    );
}

/// An implicit error matches `--match` only by its kind word, and says so;
/// its platform message blocks a `no` for any other text. Break: let a
/// platform message never match (TestField answers unknown), or let it
/// count as decided (the second answer is `no`).
#[test]
fn implicit_errors_match_by_kind_word_only() {
    let tmp = fixture();
    let (v, human) = matching(tmp.path(), "Checks", "TestField");
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    assert!(
        ev.iter()
            .any(|e| e["matchedBy"] == "kind-word" && e["op"] == "TestField"),
        "{ev:?}"
    );
    assert!(human.contains("(matched by its kind word only)"), "{human}");
    let (v, human) = matching(tmp.path(), "Checks", "something else");
    assert_eq!(answer(&v), "unknown", "{human}");
    assert!(blocker_kinds(&v).contains(&"message-unknown".to_string()));
}

/// `--site` keeps only the site at that line; `--op` and `--table` keep
/// only the operation and the table asked. Break: ignore the line in the
/// site match (both of Checks' errors match).
#[test]
fn site_op_and_table_filters() {
    let tmp = fixture();
    // `Error(BlockedErr)` in Checks: line 77 of Code.al (1-based).
    let src_line = CODE
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("Error(BlockedErr);"))
        .map(|(i, _)| i + 1)
        .last()
        .unwrap();
    let site = format!("Code.al:{src_line}");
    let (v, human) = prove(
        tmp.path(),
        "Checks",
        "throws-error",
        None,
        Some(&site),
        None,
        None,
    );
    assert_eq!(answer(&v), "yes", "{human}");
    let ev = evidence(&v);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0]["op"], "Error");
    assert_eq!(ev[0]["evidence"]["line"], src_line as u64);
    // `--op TestField --table "Sl Rec"`: the TestField only.
    let (v, _) = prove(
        tmp.path(),
        "Checks",
        "throws-error",
        None,
        None,
        Some("TestField"),
        Some("Sl Rec"),
    );
    assert_eq!(answer(&v), "yes");
    assert_eq!(evidence(&v).len(), 1);
    // No error of Checks is raised by a record operation on another table.
    let (v, human) = prove(
        tmp.path(),
        "Checks",
        "throws-error",
        None,
        None,
        None,
        Some("Other"),
    );
    assert_eq!(answer(&v), "no", "{human}");
    // Writes: `--op Insert` yes, `--op Delete` no (every write decided).
    let (v, human) = prove(
        tmp.path(),
        "Writes",
        "writes-table:Sl Rec",
        None,
        None,
        Some("insert"),
        None,
    );
    assert_eq!(answer(&v), "yes", "{human}");
    assert_eq!(evidence(&v)[0]["op"], "Insert");
    let (v, human) = prove(
        tmp.path(),
        "Writes",
        "writes-table:Sl Rec",
        None,
        None,
        Some("Delete"),
        None,
    );
    assert_eq!(answer(&v), "no", "{human}");
}

/// Output validation V2 (§4.3, F18): a filtered `no` names what it filtered
/// and the other reachable sites outside the filter, in the human answer and
/// in the JSON result, so a reader of `answer: "no"` sees what it is "no"
/// about and what else matched. Discrimination (2026-11-06): with
/// `prove_slice::human_lines` and `other_sites_value` filtering every site
/// out (`.filter(|_| false)`), this test fails on the missing other site;
/// restored, it passes.
#[test]
fn a_filtered_no_names_its_filter_and_the_other_sites() {
    let tmp = fixture();
    let (v, human) = prove(
        tmp.path(),
        "Writes",
        "writes-table:Sl Rec",
        None,
        None,
        Some("Delete"),
        None,
    );
    assert_eq!(answer(&v), "no", "{human}");
    let r = &v["payload"]["result"];
    assert_eq!(r["filter"]["op"], "Delete", "{r:#}");
    let others = r["otherSites"].as_array().unwrap();
    assert!(
        others.iter().any(|s| s["op"] == "Insert"),
        "the Insert outside the filter is listed: {r:#}"
    );
    assert!(
        human.contains(
            "\nanswer: no — no Delete site in the analyzed cone (every candidate decided"
        ),
        "{human}"
    );
    assert!(
        human.contains("other reachable write sites (outside the filter): ")
            && human.contains("Insert at "),
        "{human}"
    );
    // The CLI's words (§6.1); at `--surface=mcp` it reads `op: "Insert"`.
    assert!(
        human.contains("to see their routes: --op Insert"),
        "{human}"
    );
}

/// A matching write on a record its caller passes is temporary or not
/// depending on that caller (G17b): neither a `yes` nor a `no`. Break: count
/// a parameter-dependent record as a table write, and it answers `yes`.
#[test]
fn a_caller_dependent_write_is_undecided() {
    let tmp = fixture();
    let (v, human) = prove(
        tmp.path(),
        "WritesParam",
        "writes-table:Sl Rec",
        None,
        None,
        Some("Insert"),
        None,
    );
    assert_eq!(answer(&v), "unknown", "{human}");
    assert_eq!(
        blocker_kinds(&v),
        vec!["temp-state-depends-on-caller".to_string()]
    );
}

/// E-5: without a filter the job declares no slice need and its document
/// has no filter fields and the unfiltered version; with one it declares
/// the rules and the guard arms. Break: always declare `PROVE_SLICE` (the
/// first assertion fails), or forget it for a filter (the filtered run
/// panics "rules not built" / "site guards not built").
#[test]
fn e5_without_a_filter_nothing_changes() {
    use crate::engine::batch::job::{Job, ViewNeeds};
    let tmp = fixture();
    let inputs = inputs_of(tmp.path());
    let plain = job(&inputs, "KnownOnly", "throws-error", None, false);
    assert_eq!(plain.demands()[0].needs, ViewNeeds::MODEL);
    let filtered = job(
        &inputs,
        "KnownOnly",
        "throws-error",
        SliceRequest::of("throws-error", Some("x"), None, None, None, None).unwrap(),
        false,
    );
    let n = filtered.demands()[0].needs;
    assert!(n.rules && n.site_guards, "{n:?}");
    let r = crate::engine::batch::run_doc(&plain)
        .one::<Result<crate::engine::l5::prove::ProveRunResult, String>>()
        .unwrap()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&r.json_text).unwrap();
    assert_eq!(
        v["schemaVersion"],
        crate::engine::l5::prove::PROVE_SCHEMA_VERSION
    );
    assert!(v["payload"].get("filter").is_none() && v["payload"].get("slice").is_none());
    assert!(
        !r.human_text.contains("static candidates"),
        "{}",
        r.human_text
    );
}

/// A view that does not declare the guard arms holds none on any row, and
/// reading them fails loudly; one that declares them holds them on every
/// row. Pins the USE in the assembly.
#[test]
fn site_guards_are_built_only_when_declared() {
    use crate::engine::batch::job::{ViewKey, ViewNeeds};
    let tmp = fixture();
    let inputs = inputs_of(tmp.path());
    let key = ViewKey::new(&inputs.workspace, false, false);
    let view = crate::engine::gate::run::build_view_model(&inputs, &key, ViewNeeds::MODEL, |_| {})
        .unwrap_or_else(|_| panic!("view"))
        .expect("a model view");
    let rows = &view.target.resolved().workspace.routines;
    assert!(rows.iter().any(|r| r.name == "Nested"), "precondition");
    assert!(rows.iter().all(|r| r.site_guards.is_none()));
    let view =
        crate::engine::gate::run::build_view_model(&inputs, &key, ViewNeeds::PROVE_SLICE, |_| {})
            .unwrap_or_else(|_| panic!("view"))
            .expect("a model view");
    let rows = &view.target.resolved().workspace.routines;
    assert!(rows.iter().all(|r| r.site_guards.is_some()));
    let nested = rows.iter().find(|r| r.name == "Nested").unwrap();
    assert_eq!(nested.site_guards().arms.len(), 3);
}

#[test]
#[should_panic(expected = "site guards not built: declare ViewNeeds::site_guards")]
fn reading_site_guards_not_built_panics() {
    use crate::engine::batch::job::{ViewKey, ViewNeeds};
    let tmp = fixture();
    let inputs = inputs_of(tmp.path());
    let key = ViewKey::new(&inputs.workspace, false, false);
    let view = crate::engine::gate::run::build_view_model(&inputs, &key, ViewNeeds::MODEL, |_| {})
        .unwrap_or_else(|_| panic!("view"))
        .expect("a model view");
    let rows = &view.target.resolved().workspace.routines;
    let _ = rows
        .iter()
        .find(|r| r.name == "Nested")
        .unwrap()
        .site_guards();
}

/// Which filter goes with which question (design §12.1): refused with one
/// text, before the workspace is read.
#[test]
fn filters_refused_on_other_questions() {
    let e = SliceRequest::of("writes-table:T", Some("x"), None, None, None, None).unwrap_err();
    assert!(e.contains("--match goes with throws-error"), "{e}");
    let e = SliceRequest::of("may-commit", None, None, Some("Insert"), None, None).unwrap_err();
    assert!(
        e.contains("--op goes with throws-error or writes-table"),
        "{e}"
    );
    let e = SliceRequest::of("writes-table:T", None, None, None, Some("T"), None).unwrap_err();
    assert!(e.contains("--table goes with throws-error"), "{e}");
    let e = SliceRequest::of(
        "callers-intercept-errors",
        None,
        Some("a.al:1"),
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(e.contains("not 'callers-intercept-errors'"), "{e}");
    let e = SliceRequest::of("throws-error", None, Some("a.al"), None, None, None).unwrap_err();
    assert!(e.contains("is not <file>:<line>"), "{e}");
    assert!(
        SliceRequest::of("throws-error", None, None, None, None, None)
            .unwrap()
            .is_none()
    );
    // E2: `--routes` needs a filter and takes 1 to 10.
    let e = SliceRequest::of("throws-error", None, None, None, None, Some(3)).unwrap_err();
    assert!(e.contains("--routes goes with --match"), "{e}");
    for bad in [0, 11] {
        let e =
            SliceRequest::of("throws-error", Some("x"), None, None, None, Some(bad)).unwrap_err();
        assert!(e.contains("--routes takes 1 to 10"), "{e}");
    }
}

// ---------------------------------------------------------------------------
// E2: several distinct routes (design §11)
// ---------------------------------------------------------------------------

/// One root per route rule, each calling only local procedures (every call
/// resolved, so only the routes vary).
const ROUTES: &str = r#"codeunit 50102 "Rt Code"
{
    Access = Internal;

    procedure Reconverge()
    begin
        ViaA();
        ViaB();
    end;

    local procedure ViaA()
    begin
        Sink();
    end;

    local procedure ViaB()
    begin
        Sink();
    end;

    local procedure Sink()
    begin
        Error('reconverged sink');
    end;

    procedure Ordered()
    begin
        Long1();
        Short1();
    end;

    local procedure Long1()
    begin
        Long2();
    end;

    local procedure Long2()
    begin
        OrderSink();
    end;

    local procedure Short1()
    begin
        OrderSink();
    end;

    local procedure OrderSink()
    begin
        Error('ordered sink');
    end;

    procedure Three()
    begin
        P1();
        P2();
        P3();
    end;

    local procedure P1()
    begin
        ThreeSink();
    end;

    local procedure P2()
    begin
        ThreeSink();
    end;

    local procedure P3()
    begin
        ThreeSink();
    end;

    local procedure ThreeSink()
    begin
        Error('three sink');
    end;

    procedure Recurse()
    begin
        Again(true);
    end;

    local procedure Again(More: Boolean)
    begin
        if More then
            Again(false);
        RecSink();
    end;

    local procedure RecSink()
    begin
        Error('recursion sink');
    end;

    // `Depth` is unknown, so `Looper` is entered with nothing known (r3 E1
    // would bind a literal `3`, a state `N - 1` then leaves).
    procedure Loop(Depth: Integer)
    begin
        Looper(Depth);
    end;

    local procedure Looper(N: Integer)
    begin
        if N > 0 then
            Looper(N - 1);
        LoopSink();
    end;

    local procedure LoopSink()
    begin
        Error('loop sink');
    end;

    procedure DeadAlt()
    begin
        Gate(false);
        Open();
    end;

    local procedure Gate(Go: Boolean)
    begin
        if Go then
            AltSink();
    end;

    local procedure Open()
    begin
        AltSink();
    end;

    local procedure AltSink()
    begin
        Error('alt sink');
    end;

    procedure InItself()
    begin
        Error('in itself');
    end;
}
"#;

fn routes_fixture() -> tempfile::TempDir {
    ws(&[("Table.al", TABLE), ("Routes.al", ROUTES)])
}

/// `prove <routine> throws-error --match <text> --routes <n>`, with the
/// enumeration's edge budget `budget` (the production default when `None`).
fn routes_of(
    ws: &std::path::Path,
    routine: &str,
    text: &str,
    n: usize,
    budget: Option<usize>,
) -> (serde_json::Value, String) {
    let mut slice = SliceRequest::of("throws-error", Some(text), None, None, None, Some(n))
        .expect("a valid filter")
        .expect("a filter");
    if let Some(b) = budget {
        slice.route_budget = b;
    }
    let inputs = inputs_of(ws);
    let j = job(&inputs, routine, "throws-error", Some(slice), false);
    let r = crate::engine::batch::run_doc(&j)
        .one::<Result<crate::engine::l5::prove::ProveRunResult, String>>()
        .unwrap()
        .unwrap_or_else(|e| panic!("prove failed: {e}"));
    assert_eq!(r.exit_code, 0, "answered: {}", r.human_text);
    (serde_json::from_str(&r.json_text).unwrap(), r.human_text)
}

/// The one matching site's routes, each as its routine stable ids' display
/// (`hops`), and its `routeSearch` block.
fn site_routes(v: &serde_json::Value) -> (Vec<Vec<String>>, serde_json::Value) {
    let ev = evidence(v);
    assert_eq!(ev.len(), 1, "one matching site: {ev:?}");
    let routes = ev[0]["routes"]
        .as_array()
        .unwrap_or_else(|| panic!("no routes: {}", ev[0]))
        .iter()
        .map(|r| {
            r["hops"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| h.as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    (routes, ev[0]["routeSearch"].clone())
}

/// The last routine of each route (`Object.Routine` of its final hop).
fn via(route: &[String]) -> Vec<&str> {
    route
        .iter()
        .map(|h| h.rsplit(['.', ' ']).next().unwrap())
        .collect()
}

/// E-9: two paths reconverge on one state (`Sink` with no known values).
/// The search tree keeps one parent for it, so a walk of the tree finds one
/// route; the state graph gives both, and says the enumeration completed.
/// Break: enumerate over the tree (one parent per state): one route.
#[test]
fn e9_reconverging_paths_give_two_routes() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "Reconverge", "reconverged sink", 5, None);
    assert_eq!(answer(&v), "yes", "{human}");
    let (routes, search) = site_routes(&v);
    let mids: Vec<&str> = routes.iter().map(|r| via(r)[1]).collect();
    assert_eq!(routes.len(), 2, "{routes:?}\n{human}");
    assert!(
        mids.contains(&"ViaA") && mids.contains(&"ViaB"),
        "{routes:?}"
    );
    assert_eq!(search["complete"], true, "{search}");
    assert!(search["limit"].is_null(), "{search}");
    assert!(
        human.contains("enumeration complete: these are every route to this site"),
        "{human}"
    );
    assert!(human.contains("route 2 (2 hops)"), "{human}");
    // The two equal-length routes are ordered by their site ids.
    let ev = evidence(&v);
    let ids: Vec<Vec<String>> = ev[0]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["witness"]["hops"]
                .as_array()
                .map(|hs| {
                    hs.iter()
                        .map(|h| h["edgeId"].as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "equal hops: by site ids");
}

/// E-1: `routes: 3` on a site with exactly two routes lists exactly two,
/// fewest hops first (the 2-hop route although its call is written second),
/// with distinct site sequences. Break: tell routes apart by their terminal
/// site only (one route); or order by discovery (the 3-hop route first).
#[test]
fn e1_routes_are_distinct_and_shortest_first() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "Ordered", "ordered sink", 3, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 2, "{routes:?}\n{human}");
    assert_eq!(
        via(&routes[0]),
        ["Ordered", "Short1", "OrderSink"],
        "{routes:?}"
    );
    assert_eq!(
        via(&routes[1]),
        ["Ordered", "Long1", "Long2", "OrderSink"],
        "{routes:?}"
    );
    assert_eq!(search["complete"], true, "{search}");
    assert_eq!(search["listed"], 2);
    assert_eq!(search["requested"], 3);
    assert!(human.contains("route 1 (2 hops)"), "{human}");
    assert!(human.contains("route 2 (3 hops)"), "{human}");
    // E1's one route is still the evidence row's `route`.
    assert_eq!(
        evidence(&v)[0]["route"]["hops"].as_array().unwrap().len(),
        3
    );
}

/// Recursion: a routine may appear twice on a route with different known
/// values (`Again(true)` then `Again(false)`), but a route never returns to
/// a state already on it (`Looper` calling itself with nothing known): that
/// edge is counted as a repeat, not followed, and the enumeration completes
/// with the one route. Breaks: allow a state to repeat (`Loop` lists 3
/// routes, each one more `Looper` hop); forbid a routine to repeat at all
/// (`Recurse` lists 1 route).
#[test]
fn recursion_follows_new_values_and_never_repeats_a_state() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "Recurse", "recursion sink", 3, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 2, "{routes:?}\n{human}");
    assert_eq!(via(&routes[0]), ["Recurse", "Again", "RecSink"]);
    assert_eq!(via(&routes[1]), ["Recurse", "Again", "Again", "RecSink"]);
    assert_eq!(search["complete"], true, "{search}");
    // `Again(false)` calling `Again(false)`: its site needs `More = true`.
    assert!(
        search["edgesRejected"]["literalArgumentsAtSite"]
            .as_u64()
            .unwrap()
            >= 1,
        "{search}"
    );

    let (v, human) = routes_of(tmp.path(), "Loop", "loop sink", 3, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 1, "{routes:?}\n{human}");
    assert_eq!(via(&routes[0]), ["Loop", "Looper", "LoopSink"]);
    assert_eq!(search["complete"], true, "{search}");
    assert!(
        search["edgesRejected"]["repeatsState"].as_u64().unwrap() >= 1,
        "{search}"
    );
    assert!(
        human.contains("back to a state already on the route (recursion)"),
        "{human}"
    );
}

/// E-3: a route the literal arguments rule out (`Gate(false)` reaching
/// `if Go then AltSink()`) is not listed; the edge into `Gate(false)` is
/// counted as rejected by literal arguments further on (its callee reaches
/// the site's routine, but no live way does), not as a plain dead end, and
/// no count of ruled-out ROUTES is given. Breaks: enumerate over the graph
/// with literal arguments ignored (two routes); count every dead end as
/// `cannotReachSite` (the literal count is 0).
#[test]
fn e3_a_literal_dead_alternative_is_counted_not_listed() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "DeadAlt", "alt sink", 5, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 1, "{routes:?}\n{human}");
    assert_eq!(via(&routes[0]), ["DeadAlt", "Open", "AltSink"]);
    assert_eq!(search["complete"], true, "{search}");
    let rejected = &search["edgesRejected"];
    assert_eq!(rejected["literalArgumentsFurtherOn"], 1, "{search}");
    assert_eq!(rejected["cannotReachSite"], 0, "{search}");
    assert!(
        human.contains("rejected: 1 by literal arguments (0 at the edge's own site, 1 further on"),
        "{human}"
    );
    assert!(
        human.contains("(counts of edges, not of routes)"),
        "{human}"
    );
    assert!(!human.contains("routes ruled out"), "{human}");
}

/// E-10: a cap that fires says so. The route cap: `routes: 2` on a site with
/// three routes lists two and says more may exist. The edge budget: a
/// budget of 2 edges stops before any route is complete; the answer names
/// the cap, says routes may be missing and falls back to E1's route. Break:
/// stop silently (no limit, `complete: true`).
#[test]
fn e10_a_cap_gives_a_limit() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "Three", "three sink", 2, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 2, "{routes:?}\n{human}");
    assert_eq!(search["complete"], false, "{search}");
    assert_eq!(search["limit"], "route-cap", "{search}");
    assert!(
        search["partialRoutesLeft"].as_u64().unwrap() >= 1,
        "{search}"
    );
    assert!(human.contains("more routes may exist"), "{human}");
    assert!(!human.contains("enumeration complete"), "{human}");

    let (v, human) = routes_of(tmp.path(), "Reconverge", "reconverged sink", 5, Some(2));
    let (routes, search) = site_routes(&v);
    assert!(routes.is_empty(), "{routes:?}");
    assert_eq!(search["complete"], false, "{search}");
    assert_eq!(search["limit"], "edge-budget", "{search}");
    assert_eq!(search["edgesExamined"], 2, "{search}");
    assert!(
        human.contains("limit: the edge budget (2) was spent with"),
        "{human}"
    );
    assert!(human.contains("routes may be missing"), "{human}");
    assert!(
        human.contains("route off the search tree (the enumeration listed none) (2 hops)"),
        "{human}"
    );
    // The verdict does not depend on the routes.
    assert_eq!(answer(&v), "yes");
}

/// A site in the root itself: one zero-hop route, and no other (a longer
/// route would pass the site first). `--routes 1` is E1's answer byte for
/// byte, `--routes` above 1 only adds.
#[test]
fn a_site_in_the_root_and_routes_one_is_e1() {
    let tmp = routes_fixture();
    let (v, human) = routes_of(tmp.path(), "InItself", "in itself", 4, None);
    let (routes, search) = site_routes(&v);
    assert_eq!(routes.len(), 1, "{routes:?}\n{human}");
    assert_eq!(search["complete"], true);
    assert!(human.contains("route 1 (0 hops)"), "{human}");

    let (one, one_human) = routes_of(tmp.path(), "Ordered", "ordered sink", 1, None);
    let (none, none_human) = prove(
        tmp.path(),
        "Ordered",
        "throws-error",
        Some("ordered sink"),
        None,
        None,
        None,
    );
    assert_eq!(one, none, "--routes 1 is E1's JSON");
    assert_eq!(one_human, none_human, "--routes 1 is E1's human answer");
    assert!(evidence(&one)[0].get("routes").is_none());
    assert!(one["payload"]["filter"].get("routes").is_none());
    let (three, _) = routes_of(tmp.path(), "Ordered", "ordered sink", 3, None);
    assert_eq!(three["payload"]["filter"]["routes"], 3);
}

/// The enumeration checks its cancellation at its first edge (and every
/// 1,024 after): a cancelled one stops with the `cancelled` limit and the
/// partial routes it left, never claiming completion. Break: drop the check
/// (it completes with both routes).
#[test]
fn a_cancelled_enumeration_stops_with_its_limit() {
    use crate::engine::l5::literal_reach::LiteralReach;
    use crate::engine::l5::literal_reach::routes::RouteLimit;
    let tmp = routes_fixture();
    let (base, tree) = crate::engine::l5::digest::wave2_index_tests::tree_over(
        tmp.path(),
        crate::engine::l4::cone_derived::WitnessScope::All,
    );
    let model = &base.resolved;
    let reach = LiteralReach::new(&tree, model);
    let id = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap()
            .stable_routine_id
            .clone()
    };
    let (root, sink) = (id("Reconverge"), id("Sink"));
    let op = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Sink")
        .unwrap()
        .operation_sites
        .iter()
        .find(|o| o.kind == "error-call")
        .expect("Sink's Error")
        .id
        .clone();
    assert_eq!(reach.op_owner(&op), Some(sink.as_str()));
    let g = reach.state_graph(&root, &|| false).expect("a small graph");
    let all = reach.routes(&g, &op, 5, 1000, &|| false);
    assert_eq!((all.routes.len(), all.limit), (2, None), "precondition");
    let stopped = reach.routes(&g, &op, 5, 1000, &|| true);
    assert_eq!(stopped.limit, Some(RouteLimit::Cancelled));
    assert!(stopped.routes.is_empty());
    assert!(stopped.counts.unexplored >= 1);
}

/// r4 final (c05): one route, `GcRoot` → `GcStep` → `GcLeaf`, whose sites sit
/// under 8 enclosing conditions (3 + 3 + 2: a `for`, a `foreach` and an `if`
/// around the first call; an `if FindSet`, its `repeat` and an `else` around
/// the second; a `case` else and an `if` around both errors), more than a
/// `condition:` line shows (4) and more than the `--site` listing shows per
/// site (2). Two earlier `if … then exit` checks come before the calls.
const GUARD_COUNT_CODE: &str = r#"codeunit 50150 "Gc Root"
{
    procedure GcRoot()
    var
        Step: Codeunit "Gc Step";
        I: Integer;
        Names: List of [Text];
        Name: Text;
        Stop: Boolean;
    begin
        if Stop then
            exit;
        for I := 1 to 3 do
            foreach Name in Names do
                if Name <> '' then
                    Step.GcStep(Name, I);
    end;
}

codeunit 50151 "Gc Step"
{
    procedure GcStep(Name: Text; I: Integer)
    var
        Rec: Record "Sl Rec";
    begin
        if Name = 'skip' then
            exit;
        if Rec.FindSet() then
            repeat
                if I > 1 then
                    Rec.Init()
                else
                    GcLeaf(I, Name);
            until Rec.Next() = 0;
    end;

    local procedure GcLeaf(I: Integer; Name: Text)
    begin
        case I of
            1:
                exit;
            else
                if I < 100 then begin
                    Evaluate(I, Name);
                    Error('gc leaf error');
                end;
        end;
    end;
}
"#;

/// r4 final (c05): "N guards on the route" is the route's FULL count of
/// enclosing conditions, the population `prove --site`'s route listing
/// counts ("shown (k of M enclosing conditions …)"), never the subset a
/// line displays; earlier exits are counted apart; a `condition:` line that
/// lists some guards says how many it shows, of the same full count. Both
/// terminal kinds are pinned: an `Error` (an operation the statement tree
/// holds as its call) and an `Evaluate` implicit error (a call site).
///
/// DISCRIMINATION (2026-11-09), each break made with Edit, run, reverted:
/// - `route_condition_words` counting the displayed list again (the
///   innermost `ROUTE_GUARDS_SHOWN` = 4 guards): `4 guards on the route`,
///   fails;
/// - `GuardKind::encloses` true for an earlier exit too (the old single
///   population): `10 guards`, fails;
/// - `guards_of` without its `for`/`foreach`/`repeat` arm (loops not
///   counted, the c05 defect): `5 guards`, fails;
/// - `guards_at` without its call-site fallback: the `Error` row loses its
///   own site's 2 guards (`6 guards`), fails;
/// - `digest_cli` without the evidence call site among the terminal
///   operations: the `Evaluate` row loses its 2 (`6 guards`), fails.
#[test]
fn route_guard_counts_are_the_full_enclosing_count() {
    let tmp = ws(&[("Table.al", TABLE), ("Gc.al", GUARD_COUNT_CODE)]);
    let err_line = GUARD_COUNT_CODE
        .lines()
        .position(|l| l.contains("Error('gc leaf error');"))
        .unwrap()
        + 1;
    // The `--site` listing's count: its population, and the precondition
    // (more guards than any display shows).
    let (v, human) = prove(
        tmp.path(),
        "GcRoot",
        "throws-error",
        None,
        Some(&format!("Gc.al:{err_line}")),
        None,
        None,
    );
    assert_eq!(answer(&v), "yes", "{human}");
    let route = &evidence(&v)[0]["route"];
    assert_eq!(
        route["guardsTotal"], 8,
        "precondition: 8 enclosing: {route}"
    );
    assert_eq!(
        route["guardsShown"], 6,
        "precondition: the listing shows fewer: {route}"
    );
    assert!(
        human.contains("guards shown (6 of 8 enclosing conditions;"),
        "{human}"
    );
    // The rows without a filter: each error's words count the same 8.
    let (v, human) = prove(tmp.path(), "GcRoot", "throws-error", None, None, None, None);
    assert_eq!(answer(&v), "yes", "{human}");
    assert!(
        human.contains("(8 guards on the route, 0 evaluated; 2 earlier exits, 1 evaluated)"),
        "the answer line's words: {human}"
    );
    assert!(
        human.contains("… shown (4 of 8 enclosing conditions and 2 earlier exits)"),
        "{human}"
    );
    // The JSON lists every guard of each row's route: 8 enclosing, 2 exits,
    // for the `Evaluate` (a call site) and the `Error` (an operation) alike.
    let ev = evidence(&v);
    let mut lines: Vec<u64> = Vec::new();
    for e in &ev {
        let g = e["routeGuards"].as_array().cloned().unwrap_or_default();
        let exits = g.iter().filter(|g| g["kind"] == "exit").count();
        assert_eq!((g.len() - exits, exits), (8, 2), "{e}");
        lines.push(e["evidence"]["line"].as_u64().unwrap_or_default());
    }
    lines.sort_unstable();
    let err = err_line as u64;
    assert_eq!(lines, [err - 1, err], "the Evaluate and the Error: {ev:?}");
}
