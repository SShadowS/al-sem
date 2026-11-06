//! The field writers read bare `F := …` writes through the implicit record,
//! and `alsem explain --field` agrees with `alsem query touches --field` on
//! every write site (one definition: `ModelRoutine::field_access_sites`).
//!
//! Through the shipped binary, so the production use is pinned, not the
//! helper. The fixture is `tests/fixtures/field-writers-implicit` (its header
//! holds the hand count; each writer's line carries `// writer`). The bug it
//! guards: Continia DO's `CDO Setup."Document Folder"` answered "0 writers"
//! while its own OnValidate assigns `"Document Folder" := … + '\'`, a site
//! `query touches` listed as an `implicit-access` write.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/field-writers-implicit/ws")
}

fn run_json(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "alsem {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// The 1-based lines of the fixture marked `// writer`.
fn marked_lines() -> BTreeSet<u64> {
    let text = std::fs::read_to_string(fixture().join("src/FWFields.al")).unwrap();
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.trim_end().ends_with("// writer"))
        .map(|(i, _)| i as u64 + 1)
        .collect()
}

/// `explain --field`'s writers: (routine, kind, line) per site.
fn explain_writers() -> BTreeSet<(String, String, u64)> {
    let ws = fixture();
    let v = run_json(&[
        "explain",
        "--table",
        "FW Item",
        "--field",
        "Folder",
        "--format",
        "json",
        "--",
        ws.to_str().unwrap(),
    ]);
    let mut out = BTreeSet::new();
    for w in v["field"]["writers"].as_array().expect("writers") {
        let routine = w["routine"].as_str().unwrap().to_string();
        for s in w["sites"].as_array().unwrap() {
            out.insert((
                routine.clone(),
                s["kind"].as_str().unwrap().to_string(),
                s["anchor"]["line"].as_u64().unwrap(),
            ));
        }
    }
    out
}

/// `query touches --field`'s write sites that are matches (not candidates).
fn touches_write_lines() -> BTreeSet<u64> {
    let ws = fixture();
    let v = run_json(&[
        "query",
        "touches",
        ws.to_str().unwrap(),
        "--table",
        "FW Item",
        "--field",
        "Folder",
        "--format",
        "json",
        "--deterministic",
    ]);
    v["payload"]["fieldTouches"]
        .as_array()
        .expect("fieldTouches")
        .iter()
        .filter(|s| s["candidate"].is_null())
        .filter(|s| s["uses"].as_array().unwrap().iter().any(|u| u == "write"))
        .map(|s| s["site"]["line"].as_u64().unwrap())
        .collect()
}

#[test]
fn bare_writes_through_the_implicit_record_are_writers() {
    let marked = marked_lines();
    assert_eq!(marked.len(), 5, "the fixture's hand count");
    let writers = explain_writers();
    let lines: BTreeSet<u64> = writers.iter().map(|(_, _, l)| *l).collect();
    assert_eq!(lines, marked, "explain --field writers: {writers:?}");
    assert!(writers.iter().all(|(_, k, _)| k == "assign"), "{writers:?}");
    let routines: BTreeSet<&str> = writers.iter().map(|(r, _, _)| r.as_str()).collect();
    for name in [
        "OnValidate",
        "Normalize",
        "ExtSet",
        "OnAction",
        "Codeunit \"FW Code\"::Qualified",
    ] {
        assert!(
            routines.iter().any(|r| r.contains(name)),
            "{name} missing from {routines:?}"
        );
    }
    assert!(
        !routines
            .iter()
            .any(|r| r.contains("Shadowed") || r.contains("ReadOnly") || r.contains("OtherField")),
        "{routines:?}"
    );
}

#[test]
fn explain_writers_and_query_touches_writes_agree() {
    let touches = touches_write_lines();
    let explain: BTreeSet<u64> = explain_writers().into_iter().map(|(_, _, l)| l).collect();
    assert_eq!(touches, explain);
    assert_eq!(touches, marked_lines());
}
