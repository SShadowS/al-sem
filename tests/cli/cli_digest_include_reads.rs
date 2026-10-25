//! #246: `alsem digest --include-reads` adds `DB_READ` effects (table reads).
//!
//! Every case spawns `CARGO_BIN_EXE_alsem`, so the clap arm, its default and
//! the batch view plumbing are pinned, not only the library pipeline.
//! Design: `.agent/issue-246/ledger.md` `## Design` (T2-T5).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const VERSION_OVERRIDE: &str = "cli-reads-v1";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `alsem digest <ws> <args> --deterministic --format json` -> the document.
fn digest(ws: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("digest")
        .arg(ws)
        .args(args)
        .args(["--deterministic", "--format", "json"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "digest {args:?} failed: {text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn entries(doc: &Value) -> &Vec<Value> {
    doc["payload"]["entries"].as_array().expect("entries")
}

fn entry<'a>(doc: &'a Value, routine: &str) -> &'a Value {
    entries(doc)
        .iter()
        .find(|e| e["routine"]["display"] == routine)
        .unwrap_or_else(|| panic!("no {routine} entry: {doc}"))
}

fn effects_of<'a>(doc: &'a Value, routine: &str, ty: &str) -> Vec<&'a Value> {
    entry(doc, routine)["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["type"] == ty)
        .collect()
}

fn count_reads(doc: &Value) -> usize {
    entries(doc)
        .iter()
        .flat_map(|e| e["effects"].as_array().unwrap())
        .filter(|f| f["type"] == "DB_READ")
        .count()
}

/// `from>to` per hop, joined by `,`, per via path.
fn via(f: &Value) -> Vec<String> {
    f["viaPaths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p.as_array()
                .unwrap()
                .iter()
                .map(|h| {
                    format!(
                        "{}>{}",
                        h["fromDisplay"].as_str().unwrap_or(""),
                        h["toDisplay"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

// ---------------------------------------------------------------------------
// T2: flag-on minus its reads equals flag-off, over the digest corpus.
// ---------------------------------------------------------------------------

/// The Design's four-step normalization of a flag-on document; returns how
/// many `DB_READ` effects it removed.
fn strip_reads(doc: &mut Value) -> usize {
    let mut removed = 0;
    for e in doc["payload"]["entries"].as_array_mut().expect("entries") {
        let effects = e["effects"].as_array_mut().expect("effects");
        let before = effects.len();
        effects.retain(|f| f["type"] != "DB_READ");
        removed += before - effects.len();
        if let Some(d) = e["entryDiagnostics"].as_array_mut() {
            d.retain(|d| d["effectType"] != "DB_READ");
        }
    }
    let total = doc["payload"]["summary"]["totalEffects"]
        .as_u64()
        .expect("totalEffects");
    doc["payload"]["summary"]["totalEffects"] = (total - removed as u64).into();
    doc["payload"]
        .as_object_mut()
        .expect("payload")
        .remove("includeReads");
    removed
}

/// T2 (A1): over `DIGEST_CORPUS`, `--include-reads` with its reads removed
/// (effects, their diagnostics, their `totalEffects` share and the
/// `includeReads` echo) is the default document. The corpus must carry reads,
/// and the three fixtures that read in source must each yield one.
#[test]
fn include_reads_adds_only_db_read() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut total = 0;
    let mut per_fixture = std::collections::BTreeMap::new();
    for fixture in crate::cli_b_digest_differential::DIGEST_CORPUS {
        let ws = corpus.join(fixture);
        let files = crate::cli_b_digest_differential::collect_al_files(&ws);
        let mut args: Vec<&str> = Vec::new();
        for f in &files {
            args.extend(["--file", f.as_str()]);
        }
        let off = digest(&ws, &args);
        assert!(
            off["payload"].get("includeReads").is_none(),
            "{fixture}: flag-off echoes includeReads"
        );
        assert_eq!(count_reads(&off), 0, "{fixture}: flag-off has DB_READ");
        args.push("--include-reads");
        let mut on = digest(&ws, &args);
        assert_eq!(
            on["payload"]["includeReads"],
            Value::Bool(true),
            "{fixture}: no includeReads echo"
        );
        let removed = strip_reads(&mut on);
        total += removed;
        per_fixture.insert(*fixture, removed);
        assert_eq!(
            on, off,
            "{fixture}: flag-on minus its reads differs from flag-off"
        );
    }
    assert!(total > 0, "the corpus yields no DB_READ: {per_fixture:?}");
    for f in ["ws-d1-multi-caller", "ws-txn-d46-pos", "ws-d8-commit-in-tx"] {
        assert!(
            per_fixture[f] > 0,
            "{f} reads in source but yields no DB_READ: {per_fixture:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// T3/T4: a workspace with its own Customer and Vendor tables.
// ---------------------------------------------------------------------------

const WS_GUID: &str = "aaaa2460-0000-0000-0000-000000000246";

const WORK_AL: &str = r#"table 50100 Customer
{
    fields
    {
        field(1; "No."; Code[20])
        {
        }
    }
}

table 50101 Vendor
{
    fields
    {
        field(1; "No."; Code[20])
        {
        }
    }
}

codeunit 50110 "Reads Work"
{
    procedure OneRead()
    var
        Customer: Record Customer;
    begin
        Customer.Get('A');
    end;

    procedure TwoReads()
    var
        Customer: Record Customer;
        Vendor: Record Vendor;
    begin
        Customer.Get('A');
        Vendor.FindFirst();
    end;

    procedure TempRead()
    var
        TempCust: Record Customer temporary;
    begin
        TempCust.FindFirst();
    end;

    procedure Caller()
    begin
        Helper();
    end;

    local procedure Helper()
    var
        Customer: Record Customer;
    begin
        Customer.Get('H');
    end;

    procedure ReadCommit()
    var
        Customer: Record Customer;
    begin
        Customer.Get('A');
        Customer.Modify();
        Message('x');
        Commit();
    end;
}
"#;

fn reads_workspace(dir: &Path) {
    write(
        &dir.join("app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"Reads246","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}]}}"#
        ),
    );
    write(&dir.join("src/Work.al"), WORK_AL);
}

/// The evidence line (0-based, as `t1` reads it) of the first `WORK_AL` line
/// whose trim is `text`.
fn line_of(text: &str) -> u64 {
    WORK_AL
        .lines()
        .position(|l| l.trim() == text)
        .unwrap_or_else(|| panic!("fixture: no line {text}")) as u64
}

/// Both modes of the reads workspace: (flag-off, flag-on).
fn reads_docs() -> (Value, Value) {
    let dir = tempfile::tempdir().unwrap();
    reads_workspace(dir.path());
    let off = digest(dir.path(), &["--file", "src/Work.al"]);
    let on = digest(dir.path(), &["--file", "src/Work.al", "--include-reads"]);
    (off, on)
}

/// T3 (A2): one read gives one `DB_READ`, two tables give two, each with its
/// table; a physical and a temporary read carry their `tempState`; a read in a
/// callee reaches the caller as a transitive effect witnessed through it.
/// Without the flag: no `DB_READ`, no echo.
#[test]
fn reads_are_effects_with_table_temp_state_and_witness() {
    let (off, on) = reads_docs();
    let table = |n: u32| format!("{WS_GUID}/table/{n}");

    assert_eq!(count_reads(&off), 0, "{off}");
    assert!(off["payload"].get("includeReads").is_none(), "{off}");

    let one = effects_of(&on, "OneRead", "DB_READ");
    assert_eq!(one.len(), 1, "OneRead: {on}");
    assert_eq!(s(&one[0]["detail"]["resourceId"]), table(50100));
    assert_eq!(s(&one[0]["detail"]["resourceDisplay"]), "Customer");
    assert_eq!(s(&one[0]["detail"]["tempState"]), "known(false)", "{on}");
    assert_eq!(s(&one[0]["provenance"]), "direct");

    let mut two: Vec<(String, String)> = effects_of(&on, "TwoReads", "DB_READ")
        .iter()
        .map(|f| {
            (
                s(&f["detail"]["resourceId"]).to_string(),
                s(&f["detail"]["resourceDisplay"]).to_string(),
            )
        })
        .collect();
    two.sort();
    assert_eq!(
        two,
        vec![
            (table(50100), "Customer".to_string()),
            (table(50101), "Vendor".to_string()),
        ],
        "TwoReads: {on}"
    );

    let temp = effects_of(&on, "TempRead", "DB_READ");
    assert_eq!(temp.len(), 1, "TempRead: {on}");
    assert_eq!(s(&temp[0]["detail"]["tempState"]), "known(true)", "{on}");

    let inherited = effects_of(&on, "Caller", "DB_READ");
    assert_eq!(inherited.len(), 1, "Caller: {on}");
    let f = inherited[0];
    assert_eq!(s(&f["provenance"]), "transitive", "{f}");
    assert_eq!(s(&f["detail"]["resourceId"]), table(50100));
    assert_eq!(via(f), vec!["Caller>Helper".to_string()], "{f}");
    assert_eq!(
        f["evidence"]["line"].as_u64(),
        Some(line_of("Customer.Get('H');")),
        "{f}"
    );
}

/// T4 (Design item 4): `DB_READ` is not ordering-graded (`guarantees` and
/// `scopedGuarantees` empty) while the routine's commit would grade it, and
/// every other effect keeps its flag-off labels (a read sorts before
/// `UI_MESSAGE`, so a shifted remap moves one).
#[test]
fn reads_are_not_ordering_graded_and_move_no_label() {
    let (off, on) = reads_docs();
    let read = effects_of(&on, "ReadCommit", "DB_READ");
    assert_eq!(read.len(), 1, "ReadCommit: {on}");
    assert_eq!(read[0]["guarantees"], Value::Array(vec![]), "{on}");
    assert_eq!(read[0]["scopedGuarantees"], Value::Array(vec![]), "{on}");
    for ty in ["COMMIT", "DB_MODIFY", "UI_MESSAGE"] {
        let a = effects_of(&off, "ReadCommit", ty);
        let b = effects_of(&on, "ReadCommit", ty);
        assert_eq!(a.len(), 1, "{ty}: {off}");
        assert_eq!(b.len(), 1, "{ty}: {on}");
        assert_eq!(a[0]["guarantees"], b[0]["guarantees"], "{ty} guarantees");
        assert_eq!(
            a[0]["scopedGuarantees"], b[0]["scopedGuarantees"],
            "{ty} scopedGuarantees"
        );
    }
    // The precondition that makes the remap break visible: UI_MESSAGE is graded.
    let ui = effects_of(&off, "ReadCommit", "UI_MESSAGE");
    assert!(
        !ui[0]["guarantees"].as_array().unwrap().is_empty(),
        "fixture: UI_MESSAGE carries labels: {off}"
    );
}

// ---------------------------------------------------------------------------
// T5: a read behind a dependency call is a rule-B group.
// ---------------------------------------------------------------------------

/// T5 (A3): cross-app, `CallChain` calls the dependency's `Outer`, whose
/// `Inner` reads Customer. The read is one `DB_READ` group that enters
/// `XBase.Outer`, names Customer, and stops there: the via path is the root's
/// own call (empty), with no hop inside the dependency.
#[test]
fn dependency_read_is_a_boundary_group() {
    let dir = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own_inner(dir.path(), "", "Cust.Get('A');");
    let off = digest(dir.path(), &["--routine", "CallChain"]);
    assert_eq!(count_reads(&off), 0, "{off}");
    let on = digest(dir.path(), &["--routine", "CallChain", "--include-reads"]);
    let reads = effects_of(&on, "CallChain", "DB_READ");
    assert_eq!(reads.len(), 1, "{on}");
    let g = reads[0];
    assert_eq!(s(&g["detail"]["boundary"]), "XBase.Outer", "{g}");
    assert_eq!(s(&g["detail"]["resourceCount"]), "1", "{g}");
    assert_eq!(s(&g["detail"]["resources"]), "Customer", "{g}");
    assert_eq!(s(&g["conditionality"]), "unknown", "{g}");
    assert_eq!(via(g), vec![String::new()], "{g}");
    assert!(
        !via(g).iter().any(|p| p.contains("Inner")),
        "the witness enters the dependency: {g}"
    );
}
