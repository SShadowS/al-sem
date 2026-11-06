//! Output validation V1 (spec 2026-11-06-output-validation-r1 §1), F15 and
//! F16, through the shipped `alsem query effects` on
//! `tests/fixtures/query-effects-buckets/ws`. Hand-stated from the fixture text:
//! `QB Copier::CopyDoc` does `ToRow.Get` and `ToRow.Modify` on its third
//! parameter `ToRow` (caller-dependent) and `TempRow.Get` on a temporary
//! record. Only the Modify is a write.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn ws() -> PathBuf {
    // The app sits in `ws/` (no root app.json), so `ids_parent_baseline`
    // does not enumerate it.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-effects-buckets/ws")
}

fn effects(extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["query", "effects"])
        .arg(ws())
        .args(["--routine", "QB Copier.CopyDoc"])
        .args(extra)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// F15: a caller-dependent `Get` and a temporary `Get` are reads, never
/// writes. The caller-dependent writes are the one Modify; the reads are both
/// Gets, with the note saying which of them are caller-dependent / temporary.
///
/// Discrimination (2026-11-06): `bucket()` returning `callerDependent` for
/// every op of the caller-dependent class (and `temporary` for every op of
/// the temporary class), as before the fix, fails at "caller-dependent
/// writes 1 on 1 table" (the summary said 2, and "temporary records 1");
/// breaking only the temporary arm fails at the "temporary records 1"
/// check; restored, passes.
#[test]
fn reads_are_never_counted_as_writes() {
    let human = effects(&[]);
    assert!(
        human.contains("caller-dependent writes 1 on 1 table"),
        "{human}"
    );
    assert!(human.contains("reads 2 on 1 table"), "{human}");
    // No temporary-record WRITE bucket (summary part or its own line).
    assert!(!human.contains("temporary records 1"), "{human}");
    assert!(!human.contains("  temporary records:"), "{human}");
    assert!(
        human.contains("    QB Row: Modify 1 (caller-dependent: parameter 3 (ToRow))"),
        "{human}"
    );
    assert!(
        human.contains("  reads: 2 on 1 table (Get 2; 1 caller-dependent, 1 on temporary records)"),
        "{human}"
    );

    let v: Value = serde_json::from_str(&effects(&["--format", "json"])).expect("json");
    let s = &v["payload"]["summary"];
    assert_eq!(s["callerDependent"]["operations"], 1, "{s:#}");
    assert_eq!(s["reads"]["operations"], 2, "{s:#}");
    assert_eq!(s["temporary"]["operations"], 0, "{s:#}");
    let by_table = s["byTable"].as_array().unwrap();
    assert_eq!(by_table.len(), 1, "{s:#}");
    assert_eq!(
        by_table[0]["ops"],
        serde_json::json!({"Modify": 1}),
        "{s:#}"
    );
}

/// F16: one answer, one base. The table row and the `--all` row both name
/// parameter 3 (1-based) with its name; the JSON keeps the 0-based
/// `tempState` and adds `parameterPosition` and `parameterName`.
///
/// Discrimination (2026-11-06): the `--all` row printing the raw
/// `temp parameter-dependent(2)` again fails at the `[Get]` row's
/// "parameter 3 (ToRow)" check; restored, passes.
#[test]
fn a_parameter_has_one_number() {
    let human = effects(&["--all"]);
    let rows: Vec<&str> = human
        .lines()
        .filter(|l| l.contains("on QB Row via direct"))
        .collect();
    assert_eq!(rows.len(), 3, "{human}");
    for op in ["[Get]", "[Modify]"] {
        assert!(
            rows.iter().any(|r| r.starts_with(&format!("    {op} "))
                && r.contains("caller-dependent: parameter 3 (ToRow)")),
            "{op}: {human}"
        );
    }
    assert!(
        !human.contains("parameter-dependent(2)"),
        "no 0-based index: {human}"
    );

    let v: Value =
        crate::as_v3(serde_json::from_str(&effects(&["--all", "--format", "json"])).expect("json"));
    let rows: Vec<&Value> = v["payload"]["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["tempState"] == "parameter-dependent(2)")
        .collect();
    assert_eq!(rows.len(), 2, "{v:#}");
    for r in rows {
        assert_eq!(r["parameterPosition"], 3, "{r:#}");
        assert_eq!(r["parameterName"], "ToRow", "{r:#}");
    }
}

fn explain() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("explain")
        .arg(ws())
        .args(["--routine", "QB Copier.CopyDoc"])
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// Output validation V2 (§4.1, F14): a routine whose only write is
/// caller-dependent is never led by a "no": explain's summary and query
/// effects' summary both lead with the conditional write, its operation and
/// place, and the condition, then the absence of any other write. Hand-stated
/// from the fixture: the one write is `ToRow.Modify()` at line 26, on the
/// third parameter `ToRow`.
///
/// Discrimination (2026-11-06): see the commit message.
#[test]
fn a_conditional_write_leads_the_summary() {
    // Output validation V4 (§5.2): one card's summary line is the header's
    // `answer:` line, as query effects' summary line is.
    let card = explain();
    let writes = card
        .lines()
        .find(|l| l.starts_with("answer: writes: "))
        .unwrap_or_else(|| panic!("{card}"));
    assert!(
        writes.contains(
            "writes: may modify persistent QB Row records (Modify at :26) when the record its \
             caller passes (parameter 3) is not temporary; otherwise it changes a temporary \
             record; no other write"
        ),
        "{card}"
    );
    let human = effects(&[]);
    let summary = human
        .lines()
        .find(|l| l.starts_with("answer: "))
        .unwrap_or_else(|| panic!("{human}"));
    assert!(
        summary.contains(
            "may modify persistent QB Row records (Modify at ws:src/Copier.al:26) when the record \
             its caller passes (parameter 3, ToRow) is not temporary; otherwise it changes a \
             temporary record; no other write that may persist: none observed"
        ),
        "{human}"
    );
}

/// Output validation V2 (§9.2, F57; §5.2, F61): `locks:` prints even at
/// zero, with its scope, and the JSON states each operation family's depth
/// and inventory. Wording and a structured echo of a fixed scope: no
/// discrimination proof (CLAUDE.md "Scope of the proof").
#[test]
fn locks_and_reads_always_print_with_their_scope() {
    let human = effects(&[]);
    assert!(
        human.contains(
            "locks: none observed (one dependency routine deep; deeper dependency locks are not \
             inventoried)"
        ),
        "{human}"
    );
    let v: Value = serde_json::from_str(&effects(&["--format", "json"])).expect("json");
    let f = &v["payload"]["result"]["scope"]["families"];
    assert_eq!(f["locks"]["depth"], "oneDependencyRoutine", "{f:#}");
    assert_eq!(f["locks"]["inventory"], "notExhaustive", "{f:#}");
    assert_eq!(v["schemaVersion"], "4.0.0");
}
