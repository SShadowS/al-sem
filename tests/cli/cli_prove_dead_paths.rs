//! G23: `prove … writes-table` on a write behind `if P then` that every call
//! path binds `P` to `false`.
//!
//! Continia DO's `DocSave` calls `RunReport(false, …)`, which passes `UseQueue`
//! on to `RunReportID`, where `QueueMail` (the `CDO Queue Entry` insert) sits
//! under `if UseQueue then`. Outside a recursive SCC the cone's
//! `param_guard` already drops it (`DocSave` here); inside one it did not,
//! because a recursive SCC shares one cone (`DocSaveRecursive`, where
//! `RunReportRec` and `RunReportIDRec` call each other, as DO's 489-routine
//! SCC does): `literal_reach` follows the literals per root.
//!
//! The controls keep every case that can still run a `yes`: a literal `true`
//! (also in the recursive SCC), the root's own parameter, a reassigned
//! parameter, one passed to a `var`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50230 "G23 Queue"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50231 "G23 Handler"
{
    procedure DocSave()
    begin
        RunReport(false);
    end;

    // As on DO: through a recursive SCC (where the cone keeps no guard), the
    // dead path is short and the live one long, so the witness search lists
    // the dead one first.
    procedure DocBoth()
    begin
        RunReportRec(false, 0); // the dead call
        Hop1();
    end;

    local procedure Hop1()
    begin
        Hop2();
    end;

    local procedure Hop2()
    begin
        Hop3();
    end;

    local procedure Hop3()
    begin
        RunReportRec(true, 0);
    end;

    procedure DocSaveRecursive()
    begin
        RunReportRec(false, 0);
    end;

    procedure DocQueueRecursive()
    begin
        RunReportRec(true, 0);
    end;

    local procedure RunReportRec(UseQueue: Boolean; Depth: Integer)
    begin
        RunReportIDRec(UseQueue, Depth);
    end;

    local procedure RunReportIDRec(UseQueue: Boolean; Depth: Integer)
    begin
        if Depth < 3 then
            RunReportRec(UseQueue, Depth + 1);
        if UseQueue then
            QueueMail();
    end;

    procedure DocQueue()
    begin
        RunReport(true);
    end;

    procedure DocEither(UseQueue: Boolean)
    begin
        RunReport(UseQueue);
    end;

    procedure DocReassigned()
    begin
        RunReportReassigning(false);
    end;

    procedure DocVarPassed()
    begin
        RunReportVarPassing(false);
    end;

    local procedure RunReport(UseQueue: Boolean)
    begin
        RunReportID(UseQueue);
    end;

    local procedure RunReportID(UseQueue: Boolean)
    begin
        if UseQueue then
            QueueMail();
    end;

    local procedure RunReportReassigning(UseQueue: Boolean)
    begin
        UseQueue := true;
        if UseQueue then
            QueueMail();
    end;

    local procedure RunReportVarPassing(UseQueue: Boolean)
    begin
        Flip(UseQueue);
        if UseQueue then
            QueueMail();
    end;

    local procedure Flip(var B: Boolean)
    begin
        B := not B;
    end;

    local procedure QueueMail()
    var
        Q: Record "G23 Queue";
    begin
        Q.Code := 'X';
        Q.Insert();
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

#[test]
fn a_write_behind_a_literal_false_guard_is_not_a_write() {
    dead_path_answers(SOURCE);
}

/// TLS-43: the same answers when the flag carries a non-ASCII capital
/// (`ÆUseQueue`). The literal-value pass (`literal_values::fold`) and the cone's
/// `param_guard` match the parameter's name against the L2 rows (guards,
/// assignments, argument bindings), which fold with `fold_identifier`.
///
/// Discrimination (2026-11-13): `literal_values::fold` back on
/// `to_ascii_lowercase` fails this test (`DocVarPassed` answers a wrong `no`);
/// restored, it passes. **Limit:** `param_guard::frame_guards`' two compares
/// back on the ASCII fold, alone, still pass: on these shapes the literal-value
/// pass decides the same answers. Both broken together fail as above.
#[test]
fn a_write_behind_a_nordic_flag_is_read_alike() {
    let nordic = SOURCE.replace("UseQueue", "ÆUseQueue");
    assert_ne!(nordic, SOURCE, "precondition: the names were renamed");
    dead_path_answers(&nordic);
}

fn dead_path_answers(source: &str) {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000023","name":"G23","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), source).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &id_of(name),
                "writes-table:G23 Queue",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };

    for routine in ["DocSave", "DocSaveRecursive"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine}: the only path is dead: {r}");
        assert_eq!(r["obligations"]["coverageComplete"], true, "{r}");
    }

    for routine in [
        "DocQueue",
        "DocQueueRecursive",
        "DocEither",
        "DocReassigned",
        "DocVarPassed",
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine}: the path can run: {r}");
    }

    // The evidence lists only paths that can run, never one through the dead
    // `RunReportRec(false, 0)` call; with the live path left unlisted (the
    // witness search took the short dead one), it says so (`viaPathsTruncated`).
    let dead_line = source
        .lines()
        .position(|l| l.contains("// the dead call"))
        .expect("the dead call is in SOURCE");
    let both = prove("DocBoth");
    assert_eq!(both["answer"], "yes", "{both}");
    let evidence = both["evidence"]
        .as_array()
        .unwrap_or_else(|| panic!("evidence: {both}"));
    let first_hop_lines: Vec<u64> = evidence
        .iter()
        .flat_map(|e| e["viaPaths"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| p[0]["anchor"]["line"].as_u64())
        .collect();
    assert!(
        evidence
            .iter()
            .all(|e| e["viaPaths"].as_array().is_some_and(|p| !p.is_empty())
                || e["viaPathsTruncated"] == true),
        "an effect with no listed path says a live one went unlisted: {both}"
    );
    assert!(
        !first_hop_lines.contains(&(dead_line as u64)),
        "no listed path starts at the dead call (0-based line {dead_line}): {first_hop_lines:?}"
    );
}

/// MCP output wave 2, W2-1 (design §3.1): digest reads its effects after
/// the same literal pass as prove, and keeps a literal-dead row (raw) marked
/// `literalUnreachable` (refined): the `G23 Queue` insert of
/// `DocSaveRecursive` (prove: `no`) is listed, marked, and is no table its
/// 1b summary counts; `DocQueueRecursive`'s live one is unmarked.
/// Break: dropping the pass from digest's read (the row loses its mark).
#[test]
fn digest_marks_a_literal_dead_write_as_prove_judges_it() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000023","name":"G23","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
    let digest = |routine: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "digest",
                "ws",
                "--routine",
                routine,
                "--format",
                "json",
                "--deterministic",
                // W2-2: the 2.x document's fields.
                "--schema",
                "2",
            ],
        )
    };
    let queue_inserts = |d: &serde_json::Value| -> Vec<serde_json::Value> {
        d["payload"]["entries"][0]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "DB_INSERT" && e["detail"]["resourceDisplay"] == "G23 Queue")
            .cloned()
            .collect()
    };
    let dead = queue_inserts(&digest("DocSaveRecursive"));
    assert!(!dead.is_empty(), "precondition: the cone lists the insert");
    assert!(
        dead.iter().all(|e| e["literalUnreachable"] == true),
        "every listed insert is marked: {dead:?}"
    );
    let live = queue_inserts(&digest("DocQueueRecursive"));
    assert!(!live.is_empty(), "precondition");
    assert!(
        live.iter().all(|e| e.get("literalUnreachable").is_none()),
        "{live:?}"
    );
}

/// Output step 1b (astra top 5): `query effects` keeps every L4 row and its
/// `via` (spec §12.2; ConTinAI reads them), and marks each row's route as
/// the cone judges it after the literal pass (`literalRoute`):
/// - `DocSaveRecursive`: the insert is literal-dead in the cone (recursive
///   SCC, `literal_reach`): `ruledOut`;
/// - `DocSave`: the cone's parameter guard drops it (singleton SCCs), so no
///   cone effect originates from it: `notInCone`;
/// - `DocBoth`: the index's first hop is the dead `RunReportRec(false, 0)`
///   call, the cone's live route enters through `Hop1`: `otherRoute`;
/// - `DocQueue`, `DocQueueRecursive`, `DocEither`: the cone's route enters
///   through the row's first hop: `coneRoute`, no human mark.
///
/// Discrimination (2026-11-05), each break in `ConeRoutes::judge`, each
/// restored: dropping the `literal_unreachable` check fails at
/// `DocSaveRecursive` (`coneRoute`); dropping the `NotInCone` check fails at
/// `DocSave` (not judged, `null`); comparing the witness's first step as
/// always equal fails at `DocBoth` (`coneRoute`).
#[test]
fn query_effects_marks_routes_the_literal_arguments_rule_out() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000023","name":"G23","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
    let query = |routine: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "query",
                "effects",
                "ws",
                "--routine",
                routine,
                "--format",
                "json",
                "--deterministic",
            ],
        )["payload"]
            .clone()
    };
    let human = |routine: &str| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(["query", "effects", "ws", "--routine", routine])
            .current_dir(t.path())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for (routine, status, hop, cone_hop) in [
        ("DocSaveRecursive", "ruledOut", "RunReportRec", None),
        ("DocSave", "notInCone", "RunReport", None),
        (
            "DocBoth",
            "otherRoute",
            "RunReportRec",
            Some("Codeunit \"G23 Handler\"::Hop1"),
        ),
        ("DocQueue", "coneRoute", "RunReport", None),
        ("DocQueueRecursive", "coneRoute", "RunReportRec", None),
        ("DocEither", "coneRoute", "RunReport", None),
    ] {
        let p = query(routine);
        let rows = p["effects"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{routine}: the row is kept: {p}");
        let row = &rows[0];
        assert_eq!(row["op"], "Insert", "{routine}: {row}");
        assert_eq!(row["via"], "inherited", "{routine}: `via` unchanged: {row}");
        assert_eq!(row["firstHop"]["display"], hop, "{routine}: {row}");
        assert_eq!(row["literalRoute"]["status"], status, "{routine}: {row}");
        assert_eq!(
            row["literalRoute"]["coneFirstHop"]["display"].as_str(),
            cone_hop,
            "{routine}: {row}"
        );
        let text = human(routine);
        let marked = text.contains("ruled out by the root's literal arguments")
            || text.contains("the cone keeps no live route through it");
        assert_eq!(marked, status != "coneRoute", "{routine}: {text}");
    }
}
