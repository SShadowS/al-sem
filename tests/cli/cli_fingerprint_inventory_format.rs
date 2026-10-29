//! G12: `fingerprint --inventory-only` without `--format`.
//!
//! Its help says it implies `--format=json`, but an omitted format resolved to
//! `human` and the command failed with "--inventory-only requires
//! --format=json (got Human)". An omitted format is now json; an explicit
//! non-json one is still refused.

use std::process::Command;

fn run(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

#[test]
fn inventory_only_implies_json() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"12121212-0000-4000-8000-000000000012","name":"G12","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        ws.join("src/C.al"),
        "codeunit 50120 \"G12 C\" { procedure P() begin end; }",
    )
    .unwrap();

    let out = run(t.path(), &["fingerprint", "ws", "--inventory-only"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json on stdout");
    assert!(
        doc["payload"]["routineInventory"].is_array(),
        "the inventory projection: {doc}"
    );

    let out = run(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "human"],
    );
    assert!(
        !out.status.success(),
        "an explicit non-json format is refused"
    );
}
