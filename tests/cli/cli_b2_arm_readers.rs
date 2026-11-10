//! Engine limits B2 (per-arm ids): a routine declared in two `#if` arms is two
//! rows of one routine id, each numbering its own sites. These pin, through
//! the CLI answers, the readers that take a routine's row by id and then look
//! a SITE up in it: each must read the row (the arm) that holds the site.
//! The arm a reader would pick by itself is the OTHER one (the FIRST row for
//! the readers keyed by stable id, the LAST for those keyed by internal id),
//! so a reader that does not follow the site reads the wrong arm.

use std::process::Command;

use serde_json::Value;

const SOURCE: &str = r#"table 50370 "RT A"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

table 50371 "RT B"
{
    fields { field(1; "No."; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50370 "RT Work"
{
    local procedure Touch()
    begin
    end;

    local procedure WriteA()
    var
        A: Record "RT A";
    begin
        A.Insert();
    end;

    local procedure WriteB()
    var
        B: Record "RT B";
    begin
        B.Insert();
    end;

#if A
    local procedure Mid(X: Boolean)
    begin
        if X then
            WriteA();
    end;
#else
    local procedure Mid(X: Boolean)
    begin
        Touch();
        if X then
            WriteB();
    end;
#endif

    procedure Root()
    begin
        Mid(true);
    end;

    procedure RootF()
    begin
        Mid(false);
    end;

    procedure Target()
    begin
    end;

#if A
    procedure Caller()
    begin
        Touch();
        Target();
    end;
#else
    procedure Caller()
    var
        A: Record "RT A";
    begin
        A.Modify();
        Target();
    end;
#endif

    local procedure Store(var R: Record "RT A")
    begin
        R.Insert();
    end;

#if A
    local procedure Stage()
    var
        T: Record "RT A" temporary;
    begin
        Store(T);
    end;
#else
    local procedure Stage()
    begin
        Touch();
    end;
#endif

    procedure TempRoot()
    begin
        Stage();
    end;

#if A
    local procedure W2()
    begin
        Touch();
    end;
#else
    local procedure W2()
    var
        B: Record "RT B";
    begin
        B.Insert();
    end;
#endif

    procedure Root2()
    begin
        W2();
    end;

    local procedure Fail()
    begin
        Error('fails');
    end;

#if A
    procedure Guarded()
    begin
        Touch();
    end;
#else
    procedure Guarded()
    begin
        asserterror Fail();
    end;
#endif
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"b2b2b2b2-0000-4000-8000-0000000000d2","name":"B2R","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/rt.al"), SOURCE).unwrap();
    t
}

fn alsem(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
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

/// The 1-based line of the first line holding `text` after the line holding
/// `after` (the n-th such line when `after` repeats: `nth` 0 or 1).
fn line_of(text: &str, after: &str, nth: usize) -> u64 {
    let start = SOURCE
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(after))
        .nth(nth)
        .map(|(i, _)| i)
        .expect(after);
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.contains(text))
        .expect(text);
    (start + at + 1) as u64
}

/// `prove Root writes-table:"RT B" --op insert --routes 1`: the route runs
/// through `Mid`'s arm B (the SECOND row). Its witness hop cites arm B's call
/// line, and the route's guard there is arm B's `if X then`. Breaks: the
/// witness step (`group_path::step`) reads the routine's first row -> the hop
/// has no line; the route sites (`prove_slice::route_sites`) read the first
/// row -> arm B's guard is not found.
#[test]
fn a_route_through_a_second_arm_cites_that_arm() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let v = alsem(&[
        "prove",
        &ws,
        "RT Work::Root",
        "writes-table:RT B",
        "--single-app",
        "--op",
        "insert",
        "--routes",
        "1",
    ]);
    let r = &v["payload"]["result"];
    assert_eq!(r["answer"], "yes", "{r:#}");
    let e = &r["evidence"][0];
    let call = line_of("WriteB();", "procedure Mid(", 1);
    let cond = line_of("if X then", "procedure Mid(", 1);
    let hops = e["route"]["witness"]["hops"]
        .as_array()
        .unwrap_or_else(|| panic!("hops: {e:#}"));
    assert_eq!(hops[1]["line"], call, "the hop cites arm B's call: {e:#}");
    let site = &e["route"]["sites"][1];
    assert_eq!(site["line"], call, "{e:#}");
    assert_eq!(
        site["guards"][0]["line"], cond,
        "the guard is arm B's `if X then`: {e:#}"
    );
}

/// `RootF` calls `Mid(false)`: in both arms the write sits under `if X then`,
/// so no build writes `RT A` (arm A, the FIRST row) or `RT B`. Break: the
/// literal search keeps one guard frame per routine (the last row's) -> arm
/// A's call has no requirement and `RT A` reads as written.
#[test]
fn a_literal_guard_in_each_arm_rules_out_its_write() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    for table in ["RT A", "RT B"] {
        let v = alsem(&[
            "prove",
            &ws,
            "RT Work::RootF",
            &format!("writes-table:{table}"),
            "--single-app",
        ]);
        assert_eq!(v["payload"]["result"]["answer"], "no", "{table}: {v:#}");
    }
}

/// `callers-have-pending-writes` on `Target`: its caller `Caller` is two arms,
/// and arm B (the SECOND row) modifies `RT A` before its call. The witness
/// cites arm B's call. Break: the callers' context reads the routine's first
/// row in the file -> arm B's call is not in it, and the order is unknown.
#[test]
fn a_callers_question_reads_the_arm_that_holds_the_call() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let v = alsem(&[
        "prove",
        &ws,
        "RT Work::Target",
        "callers-have-pending-writes",
    ]);
    let r = &v["payload"]["result"];
    assert_eq!(r["answer"], "yes", "{r:#}");
    let text = r.to_string();
    let call = line_of("Target();", "procedure Caller(", 1);
    assert!(
        text.contains(&format!("\"line\":{call}")),
        "the witness cites arm B's call (line {call}): {r:#}"
    );
    assert!(!text.contains("order-unknown"), "{r:#}");
}

/// `prove Root2 writes-table:"RT B" --op insert --routes 1`: the insert is
/// `W2`'s arm B (the SECOND row) own operation; the evidence, the route's
/// last site and the witness terminal cite its line. Breaks: the slice reads
/// the owner's first row for the site (`prove_slice`, the evidence) -> no
/// line; the route's terminal anchor reads the first row -> no line; the
/// witness terminal (`group_path::terminal`) reads the first row -> no line.
#[test]
fn a_site_in_a_second_arm_cites_its_line() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let v = alsem(&[
        "prove",
        &ws,
        "RT Work::Root2",
        "writes-table:RT B",
        "--single-app",
        "--op",
        "insert",
        "--routes",
        "1",
    ]);
    let e = &v["payload"]["result"]["evidence"][0];
    let line = line_of("B.Insert();", "procedure W2(", 1);
    assert_eq!(e["evidence"]["line"], line, "evidence: {v:#}");
    let sites = e["route"]["sites"]
        .as_array()
        .unwrap_or_else(|| panic!("sites: {v:#}"));
    assert_eq!(sites.last().unwrap()["line"], line, "route terminal: {v:#}");
    assert_eq!(
        e["route"]["witness"]["terminal"]["line"], line,
        "witness terminal: {v:#}"
    );
}

/// `prove Guarded throws-error`: `Guarded`'s arm B (the SECOND row) calls
/// `Fail` under `asserterror`, so the error is caught there: the answer is
/// `no`, and the trap is placed at arm B's `asserterror` line. Breaks: the
/// trap edge reads the call in the routine's first row (`edge_trap`) -> no
/// trap, the error escapes (`yes`); the trap's place reads the first row
/// (`trap_at`) -> no line.
#[test]
fn a_trap_in_a_second_arm_is_read_in_that_arm() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "prove",
            &ws,
            "RT Work::Guarded",
            "throws-error",
            "--single-app",
            "--op",
            "error",
            "--format",
            "human",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("answer: no"), "{text}");
    let at = line_of("asserterror Fail();", "procedure Guarded(", 1);
    assert!(
        text.contains(&format!("by the asserterror at ws:src/rt.al:{at}")),
        "the trap sits at arm B's asserterror (line {at}): {text}"
    );
}

/// `TempRoot` → `Stage` (two arms; arm A, the FIRST row, passes a TEMPORARY
/// record to `Store`, which inserts it; arm B calls nothing). The write is
/// temporary, so no build writes the physical table, and `query effects`
/// says the insert's record is temporary. Break: the summary solver's
/// parameter substitution (`db_effect_solver::edge_caller`) reads the call in
/// the id's last row (arm B) -> no binding, the record's state is not
/// resolved, and `prove` answers `yes`.
#[test]
fn a_temporary_argument_in_an_arm_is_temporary() {
    let t = workspace();
    let ws = t.path().to_string_lossy().into_owned();
    let v = alsem(&[
        "prove",
        &ws,
        "RT Work::TempRoot",
        "writes-table:RT A",
        "--single-app",
    ]);
    assert_eq!(v["payload"]["result"]["answer"], "no", "{v:#}");
    let q = alsem(&[
        "query",
        "effects",
        &ws,
        "--routine",
        "RT Work.TempRoot",
        "--single-app",
        "--format",
        "json",
    ]);
    let states: Vec<&Value> = q["payload"]["effects"]
        .as_array()
        .unwrap_or_else(|| panic!("effects: {q:#}"))
        .iter()
        .map(|e| &e["tempState"])
        .collect();
    assert!(
        !states.is_empty() && states.iter().all(|s| *s == "temporary"),
        "the insert's record is temporary: {q:#}"
    );
}
