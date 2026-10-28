//! G4: a dependency app given as a SOURCE FOLDER (its own `app.json` and `.al`
//! files, no `.app`), named in the workspace's `al-sem.dependencies.json`.
//!
//! Every test runs a real binary over a temp tree laid out like the case that
//! motivated it (Continia DO against a BC.History checkout of Base Application):
//! the workspace `ws/`, and the dependency `dep/` BESIDE it, with a nested test
//! app `dep/Test/` (its own `app.json`) that must not count as part of `dep`.

use std::path::Path;
use std::process::Command;

const WS_GUID: &str = "aaaaaaaa-0000-4000-8000-0000000000a4";
const DEP_GUID: &str = "dddddddd-0000-4000-8000-0000000000d4";

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// `root/ws` calls `"D Helper".DoWork` (in `dep`) and `"D Test Only".Probe`
/// (only in the nested `dep/Test` app). `config`: the workspace's
/// `al-sem.dependencies.json` text, if any.
fn tree(root: &Path, config: Option<&str>) {
    write(
        &root.join("ws/app.json"),
        &format!(
            r#"{{"id":"{WS_GUID}","name":"W","publisher":"t","version":"1.0.0.0",
                "dependencies":[{{"id":"{DEP_GUID}","name":"D","publisher":"t","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &root.join("ws/src/Caller.Codeunit.al"),
        r#"codeunit 50100 "W Caller"
{
    procedure Run()
    var
        Helper: Codeunit "D Helper";
        TestOnly: Codeunit "D Test Only";
    begin
        Helper.DoWork();
        TestOnly.Probe();
    end;
}
"#,
    );
    write(
        &root.join("dep/app.json"),
        &format!(r#"{{"id":"{DEP_GUID}","name":"D","publisher":"t","version":"1.0.0.0"}}"#),
    );
    write(
        &root.join("dep/src/Helper.Codeunit.al"),
        r#"codeunit 50200 "D Helper"
{
    procedure DoWork()
    begin
    end;
}
"#,
    );
    write(
        &root.join("dep/Test/app.json"),
        r#"{"id":"eeeeeeee-0000-4000-8000-0000000000e4","name":"D Tests","publisher":"t","version":"1.0.0.0"}"#,
    );
    write(
        &root.join("dep/Test/src/TestOnly.Codeunit.al"),
        r#"codeunit 50300 "D Test Only"
{
    procedure Probe()
    begin
    end;
}
"#,
    );
    if let Some(c) = config {
        write(&root.join("ws/al-sem.dependencies.json"), c);
    }
}

/// `aldump --program-call-graph-stats ws`'s `primaryScoped` histogram.
fn stats(root: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(root.join("ws"))
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    v["primaryScoped"].clone()
}

/// The call into the source folder resolves to its source; the call into the
/// nested test app stays unknown (that app is not part of the dependency).
/// Without the config both are unknown: the comparison is what the config does.
#[test]
fn a_source_folder_dependency_resolves_and_its_nested_app_does_not() {
    let without = tempfile::tempdir().unwrap();
    tree(without.path(), None);
    let base = stats(without.path());
    assert_eq!(
        base["unknown"], 2,
        "precondition: without the config both calls are unknown: {base}"
    );

    let with = tempfile::tempdir().unwrap();
    tree(with.path(), Some(r#"{ "sources": ["../dep"] }"#));
    let got = stats(with.path());
    assert_eq!(
        got["unknown"], 1,
        "only the nested test app's call stays unknown: {got}"
    );
    assert_eq!(
        got["resolvedSource"].as_u64().unwrap(),
        base["resolvedSource"].as_u64().unwrap() + 1,
        "the dependency call resolves to source: before {base}, after {got}"
    );
}

/// A configuration the build cannot honour fails the build with its reason
/// (al-call-hierarchy --project prints it; G2's plumbing).
#[test]
fn a_bad_source_folder_names_the_reason() {
    for (config, reason) in [
        (r#"{ "sources": ["../nope"] }"#, "source \"../nope\":"),
        (r#"{ "source": [] }"#, "unknown field `source`"),
        (r#"{ "sources": ["src"] }"#, "is inside the workspace"),
        (
            r#"{ "sources": ["../dep/Test", "../dep/Test"] }"#,
            "are both app",
        ),
    ] {
        let t = tempfile::tempdir().unwrap();
        tree(t.path(), Some(config));
        let out = Command::new(env!("CARGO_BIN_EXE_al-call-hierarchy"))
            .arg("--project")
            .arg(t.path().join("ws"))
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{config}: the build must fail");
        assert!(
            stderr.contains("al-sem.dependencies.json") && stderr.contains(reason),
            "{config}: stderr must name the config and {reason:?}: {stderr}"
        );
    }
}

/// `alsem batch` refuses a job that writes a file a source folder holds: the
/// workspace read takes it, as it takes the workspace's own files.
#[test]
fn batch_refuses_a_write_into_a_source_folder() {
    let t = tempfile::tempdir().unwrap();
    tree(t.path(), Some(r#"{ "sources": ["../dep"] }"#));
    let ws = t.path().join("ws");
    let manifest = t.path().join("m.json");
    let job = |stdout: &Path| {
        serde_json::json!({
            "argv": ["events", "fanout", ws.to_str().unwrap(), "--format", "json"],
            "stdout": stdout.to_str().unwrap(),
            "stderr": t.path().join("e.txt").to_str().unwrap(),
        })
    };
    let reader = serde_json::json!({
        "argv": ["events", "fanout", ws.to_str().unwrap(), "--format", "json"],
        "stdout": t.path().join("r.json").to_str().unwrap(),
        "stderr": t.path().join("r.err").to_str().unwrap(),
    });
    let writer = job(&t.path().join("dep/src/Out.al"));
    std::fs::write(
        &manifest,
        serde_json::json!({ "jobs": [reader, writer] }).to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("batch")
        .arg(&manifest)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("reads the workspace") && stderr.contains("Out.al"),
        "the write into the source folder must be refused: {stderr}"
    );
}
