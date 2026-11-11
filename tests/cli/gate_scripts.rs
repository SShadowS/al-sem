//! The CDO gate's scripts cannot pass on nothing (TLS-10): `scripts/cdo-speed`
//! fails when a timed call fails (a wrong workspace once made every call exit 1
//! in 0.01 s on both sides, and the phase printed PASS on the ratios of those
//! failures), and `scripts/cdo-gate` fails a test phase that ran no test.

/// The speed gate pointed at an empty workspace, both sides this build's
/// `alsem` (`--base-exe`/`--head-exe`: no build, no git): the first timed
/// call exits 1, and the gate fails (exit 1) naming the call, its exit code
/// and its stderr, and never prints PASS.
///
/// Mutation: drop the non-zero exit check in `run_round` -> every call is
/// timed, the ratios of the failures pass, PASS and exit 0.
#[cfg(target_os = "linux")]
#[test]
fn speed_gate_fails_when_a_timed_call_fails() {
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_alsem");
    let run = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(ws.path())
        .args(["--base-exe", exe, "--head-exe", exe, "--out"])
        .arg(out.path())
        .output()
        .expect("spawn python3 scripts/cdo-speed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(1), "{text}");
    assert!(
        text.contains("FAIL: r0 build (alsem_explain) on base exited 1"),
        "{text}"
    );
    assert!(text.contains("  | "), "the call's stderr: {text}");
    assert!(!text.contains("cdo-speed: PASS"), "{text}");
}

/// `cdo-gate --summary=<run>` on a run folder written by hand (no speed
/// phase): every test phase exited 0. With each phase passing tests, PASS and
/// exit 0 (the precondition); with `lsp` exiting 0 on "0 passed" (a filter
/// that matches nothing), the gate fails, naming it "no test ran".
///
/// Mutation: drop the zero-test rule in `summarize` -> `lsp` is ok, PASS.
#[cfg(unix)]
#[test]
fn cdo_gate_fails_a_phase_that_ran_no_test() {
    let phases = [
        "program_resolve_harness",
        "lsp",
        "l4_summary_differential",
        "lib_cdo",
        "output_cdo",
        "output_cdo_impact",
    ];
    let summary = |zero: Option<&str>| {
        let run = tempfile::tempdir().unwrap();
        // A run that ran the impact ratchets as their own phase (TLS-19).
        std::fs::write(run.path().join("split-impact"), "").unwrap();
        for p in phases {
            let n = if zero == Some(p) { 0 } else { 3 };
            std::fs::write(run.path().join(format!("{p}.status")), "0 1\n").unwrap();
            std::fs::write(
                run.path().join(format!("{p}.log")),
                format!(
                    "    1 test result: ok. {n} passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.01s\n"
                ),
            )
            .unwrap();
        }
        let out = std::process::Command::new("bash")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg(format!("--summary={}", run.path().display()))
            .output()
            .expect("spawn bash scripts/cdo-gate");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    let (code, text) = summary(None);
    assert_eq!(code, Some(0), "precondition: {text}");
    assert!(text.contains("cdo-gate: PASS"), "precondition: {text}");
    let (code, text) = summary(Some("lsp"));
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains(" lsp ") && l.contains("FAILED (no test ran)")),
        "{text}"
    );
    assert!(text.contains("cdo-gate: FAIL"), "{text}");
}

/// TLS-18: `cdo-speed --warm REV` (what `merge-and-push` runs after a landing)
/// fills exactly the cache entry a gate reads for its base: with that entry
/// present (a speed cache in a temporary folder, `ALSEM_SPEED_CACHE`, holding
/// a placeholder binary at `bin/<HEAD's sha>/alsem`), it finds it and builds
/// nothing. A `cargo` that always fails is first on `PATH`, so a warm that
/// looked anywhere else would try to build and fail.
///
/// Mutation: `--warm` keyed by anything but the commit's sha (as
/// `cached_binary` keys it) -> it builds, the stub `cargo` fails, exit 2.
#[cfg(target_os = "linux")]
#[test]
fn speed_warm_reads_the_gates_own_cache_entry() {
    use std::os::unix::fs::PermissionsExt;
    let repo = env!("CARGO_MANIFEST_DIR");
    let head = String::from_utf8(
        crate::git::reader(std::path::Path::new(repo))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let cache = tempfile::tempdir().unwrap();
    let bin = cache.path().join("bin").join(&head);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("alsem"), b"placeholder").unwrap();
    let stub = tempfile::tempdir().unwrap();
    let cargo = stub.path().join("cargo");
    std::fs::write(&cargo, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .args(["--warm", &head])
        .env("ALSEM_SPEED_CACHE", cache.path())
        .env("PATH", path)
        .output()
        .expect("spawn python3 scripts/cdo-speed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(0), "{text}");
    assert!(text.contains("cached binary"), "{text}");
    assert!(
        text.contains(&format!("warm: {head} = {}", &head[..10])),
        "{text}"
    );
}
