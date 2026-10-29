//! G4: a dependency app given as a SOURCE FOLDER (its own `app.json` and `.al`
//! files, no `.app`), named in the workspace's `al-sem.dependencies.json`.
//!
//! Every test runs a real binary over a temp tree laid out like the case that
//! motivated it (Continia DO against a BC.History checkout): the workspace `ws/`,
//! and BESIDE it `deps/D/` in BC.History's shape. The app's own folder is
//! `deps/D/Source/D`, which holds a MODULE folder with an `app.json` of its own
//! (System Application has 110 of them, compiled into the one app); its test
//! app is a separate folder, `deps/D/Test/D Tests`, beside the app's `Source`.

use std::path::Path;
use std::process::Command;

const WS_GUID: &str = "aaaaaaaa-0000-4000-8000-0000000000a4";
const DEP_GUID: &str = "dddddddd-0000-4000-8000-0000000000d4";

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The app's own folder, as a config entry relative to `ws`.
const DEP: &str = "../deps/D/Source/D";

/// `root/ws` calls `"D Helper".DoWork` (in the app), `"D Module Helper".Assist`
/// (in its module folder) and `"D Test Only".Probe` (only in its test app).
/// `config`: the workspace's `al-sem.dependencies.json` text, if any.
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
        ModuleHelper: Codeunit "D Module Helper";
        TestOnly: Codeunit "D Test Only";
    begin
        Helper.DoWork();
        ModuleHelper.Assist();
        TestOnly.Probe();
    end;
}
"#,
    );
    let app = root.join("deps/D/Source/D");
    write(
        &app.join("app.json"),
        &format!(r#"{{"id":"{DEP_GUID}","name":"D","publisher":"t","version":"1.0.0.0"}}"#),
    );
    write(
        &app.join("src/Helper.Codeunit.al"),
        r#"codeunit 50200 "D Helper"
{
    procedure DoWork()
    begin
    end;
}
"#,
    );
    write(
        &app.join("Module/app.json"),
        r#"{"id":"cccccccc-0000-4000-8000-0000000000c4","name":"D Module","publisher":"t","version":"1.0.0.0"}"#,
    );
    write(
        &app.join("Module/src/ModuleHelper.Codeunit.al"),
        r#"codeunit 50250 "D Module Helper"
{
    procedure Assist()
    begin
    end;
}
"#,
    );
    write(
        &root.join("deps/D/Test/D Tests/app.json"),
        r#"{"id":"eeeeeeee-0000-4000-8000-0000000000e4","name":"D Tests","publisher":"t","version":"1.0.0.0"}"#,
    );
    write(
        &root.join("deps/D/Test/D Tests/src/TestOnly.Codeunit.al"),
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

/// The calls into the app and into its module folder resolve to source; the
/// call into its test app (beside it, not named) stays unknown. Without the
/// config all three are unknown: the comparison is what the config does.
/// G21: the module folder's own `app.json` made G4's first walk skip it.
#[test]
fn a_source_folder_dependency_resolves_with_its_modules_not_its_tests() {
    let without = tempfile::tempdir().unwrap();
    tree(without.path(), None);
    let base = stats(without.path());
    assert_eq!(
        base["unknown"], 3,
        "precondition: without the config every call is unknown: {base}"
    );

    let with = tempfile::tempdir().unwrap();
    tree(with.path(), Some(&format!(r#"{{ "sources": ["{DEP}"] }}"#)));
    let got = stats(with.path());
    assert_eq!(
        got["unknown"], 1,
        "only the test app's call stays unknown: {got}"
    );
    assert_eq!(
        got["resolvedSource"].as_u64().unwrap(),
        base["resolvedSource"].as_u64().unwrap() + 2,
        "the app's and its module's calls resolve to source: before {base}, after {got}"
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
            r#"{ "sources": ["../deps/D/Source/D", "../deps/D/Source/D"] }"#,
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
    tree(t.path(), Some(&format!(r#"{{ "sources": ["{DEP}"] }}"#)));
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
    let writer = job(&t.path().join("deps/D/Source/D/Module/src/Out.al"));
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
