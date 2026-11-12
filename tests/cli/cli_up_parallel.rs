//! TLS-21: `alsem impact --part up` gives master's answer at any worker count.
//!
//! The reference, `tests/fixtures/up-parallel/up.master.golden.json`, was written ONCE by the
//! `release-fast` `alsem` of master `4f2459a6` (with TLS-21 step 0's trace-only spans, which do
//! not change the output), before the up walk ran roots in parallel. It is the outside
//! reference: the parallel and the serial path both share the new code, so comparing them with
//! each other would miss a bug they share. **Never regenerate it**; no `REGEN_TEMP_GOLDENS` path
//! writes it. A deliberate change to the up part's answer replaces it by hand, with the reason.
//! The three fields that measure the run (`cost.walkMs`, `cost.walksUs`, `cost.scratchBytes`)
//! are `null` in it and blanked here.
//!
//! Fixture (`tests/fixtures/up-parallel/ws`): six roots in a fixed order, the not-in-graph one in
//! the middle, a two-node root (`#if` arms subscribing differently) and a page-action caption.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

const APP: &str = "4b7e2d91-6c3a-4f58-a0d2-1e9c8b5f7a36:Codeunit:51400#";

/// The roots, in order: Alpha, Beta (two nodes), not in the graph, Gamma (caption), Delta
/// (over `RunObject`), Lonely.
const ROOT_HASHES: [&str; 6] = [
    "81c17c9a2aeee9d00bd7820fcaccb88e39c54eb5b6f812184c979dbe654005b3",
    "f6c9f8d45c19c37415ec3eb8d87823274ae3035d8c82c5175b9778646f614b1d",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "d800ba134870623f51844fbfbb72920fc06f06a84586ecd7ce57881918fe0a9f",
    "fe1aefe8188b436f7e9aa6de6bad5c41346e321d5f78210204a8c3afdffcb0a5",
    "c723a99e303f0e7dcb2865b635356ddc3fa2671ddf23757bce1cc40012216b88",
];

/// `alsem impact --part up` over the roots with `AL_SEM_REACH_WORKERS=workers`, the three
/// run-measuring fields blanked.
fn up_part(ws: &Path, workers: usize) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg("--part")
        .arg("up")
        .args(ROOT_HASHES.iter().map(|h| format!("--root-id={APP}{h}")))
        .arg(ws)
        .env("AL_SEM_REACH_WORKERS", workers.to_string())
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let cost = v["cost"].as_object_mut().expect("a cost block");
    for k in ["walkMs", "walksUs", "scratchBytes"] {
        assert!(cost.contains_key(k), "cost.{k}");
        cost.insert(k.to_string(), Value::Null);
    }
    v
}

#[test]
fn up_part_matches_master_at_1_and_8_workers() {
    let t = tempfile::tempdir().unwrap();
    copy_dir(
        &repo_root().join("tests/fixtures/up-parallel/ws"),
        &t.path().join("ws"),
    );
    std::fs::create_dir(t.path().join(".git")).unwrap();
    let ws = t.path().join("ws");
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(
            repo_root().join("tests/fixtures/up-parallel/up.master.golden.json"),
        )
        .unwrap(),
    )
    .unwrap();
    // The golden's own shape, so a wrong golden cannot pass by accident.
    let rows = golden["roots"].as_array().unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[1]["nodes"], 2, "Beta: two nodes");
    assert_eq!(rows[2]["status"], "not-in-graph");
    assert!(golden.to_string().contains("\"caption\":\"Run Gamma\""));
    for workers in [1, 8] {
        let got = up_part(&ws, workers);
        let ids: Vec<&str> = got["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["stableId"].as_str().unwrap())
            .collect();
        let want: Vec<String> = ROOT_HASHES.iter().map(|h| format!("{APP}{h}")).collect();
        assert_eq!(ids, want, "rows in root order at {workers} workers");
        assert_eq!(got, golden, "master's answer at {workers} workers");
    }
}
