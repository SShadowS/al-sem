//! #277 T11 (D2.1, D2.2; Review Focus 3, 5): a manifest that cannot run is a
//! usage error before anything runs: exit 3, ONE stderr line naming the batch
//! and job and the reason (clap's own first line for a bad flag), no job line
//! on stdout, no build.

use serde_json::{Value, json};

use crate::cli_build_counters::{count, last_counters, spawn_trace_events, ws_a};

/// `alsem batch` over `manifest`: exit, stdout, stderr, `build.program`.
fn run(manifest: &Value) -> (Option<i32>, String, String, u64) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["batch", path.to_str().unwrap()],
        &[("RUST_LOG", "off")],
    );
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        count(&last_counters(&events), "build.program"),
    )
}

#[test]
fn batch_manifest_errors() {
    let ws = ws_a();
    let good = json!({ "argv": ["analyze", ws, "--format", "json"] });
    // (the second job, its argv, what the one line must say)
    let bad_jobs: [(Value, &str); 5] = [
        (json!(["batch", "m.json"]), "'batch' cannot run in a batch"),
        (
            json!(["policy", "explain", "some-rule"]),
            "'policy explain' cannot run in a batch",
        ),
        (
            json!(["cache", "prune"]),
            "'cache prune' cannot run in a batch",
        ),
        (json!([]), "no command"),
        (
            json!(["analyze", ws, "--no-such-flag"]),
            "error: unexpected argument '--no-such-flag' found",
        ),
    ];
    for (argv, reason) in bad_jobs {
        let manifest = json!({ "batches": [{ "jobs": [good, { "argv": argv }] }] });
        let (exit, stdout, stderr, program) = run(&manifest);
        let what = format!("{argv}: {stderr:?}");
        assert_eq!(exit, Some(3), "{what}");
        assert_eq!(stdout, "", "{what}");
        assert_eq!(stderr.lines().count(), 1, "one line: {what}");
        assert!(
            stderr.starts_with("al-sem batch: batch 0 job 1: ") && stderr.contains(reason),
            "{what}"
        );
        assert_eq!(program, 0, "nothing runs: {what}");
    }
    // Empty manifests (plan clarification): nothing to run is a usage error.
    for (manifest, reason) in [
        (json!({ "jobs": [] }), "batch 0: no job"),
        (json!({ "batches": [] }), "no batch"),
        (
            json!({ "batches": [{ "jobs": [good] }, { "jobs": [] }] }),
            "batch 1: no job",
        ),
    ] {
        let (exit, stdout, stderr, program) = run(&manifest);
        let what = format!("{manifest}: {stderr:?}");
        assert_eq!(exit, Some(3), "{what}");
        assert_eq!(stdout, "", "{what}");
        assert_eq!(stderr.lines().count(), 1, "{what}");
        assert!(stderr.contains(reason), "{what}");
        assert_eq!(program, 0, "{what}");
    }
}
