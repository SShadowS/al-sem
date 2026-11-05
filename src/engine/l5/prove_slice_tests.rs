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
    let slice = SliceRequest::of(question, m, site, op, table).expect("a valid filter");
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
    assert_eq!(v["schemaVersion"], "2.5.0");
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
        SliceRequest::of("throws-error", Some("x"), None, None, None).unwrap(),
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
    let e = SliceRequest::of("writes-table:T", Some("x"), None, None, None).unwrap_err();
    assert!(e.contains("--match goes with throws-error"), "{e}");
    let e = SliceRequest::of("may-commit", None, None, Some("Insert"), None).unwrap_err();
    assert!(
        e.contains("--op goes with throws-error or writes-table"),
        "{e}"
    );
    let e = SliceRequest::of("writes-table:T", None, None, None, Some("T")).unwrap_err();
    assert!(e.contains("--table goes with throws-error"), "{e}");
    let e =
        SliceRequest::of("callers-intercept-errors", None, Some("a.al:1"), None, None).unwrap_err();
    assert!(e.contains("not 'callers-intercept-errors'"), "{e}");
    let e = SliceRequest::of("throws-error", None, Some("a.al"), None, None).unwrap_err();
    assert!(e.contains("is not <file>:<line>"), "{e}");
    assert!(
        SliceRequest::of("throws-error", None, None, None, None)
            .unwrap()
            .is_none()
    );
}
