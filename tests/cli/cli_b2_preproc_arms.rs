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
    t
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
    for routine in ["NestedArmExit", "NestedIfExit", "LoopExit", "ExitOrError"] {
        assert_eq!(
            insert_conditionality(&ws, routine, "B2 Log B", "LogB.Insert();"),
            "conditional",
            "{routine}"
        );
    }
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
