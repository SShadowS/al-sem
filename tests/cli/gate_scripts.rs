//! The CDO gate's scripts cannot pass on nothing (TLS-10): `scripts/cdo-speed`
//! fails when a timed call fails (a wrong workspace once made every call exit 1
//! in 0.01 s on both sides, and the phase printed PASS on the ratios of those
//! failures), and `scripts/cdo-gate` fails a test phase that ran no test.

/// ⟨TLS-89⟩ A cdo-speed run takes the CDO slot timed before it times: give it a
/// slot of its own under `dir` (never the tentacle's, never an inherited hold's
/// token), no memory floor, and build names no real process has (a peer lane's
/// build must not hold it up).
fn private_slot<'a>(
    cmd: &'a mut std::process::Command,
    dir: &std::path::Path,
) -> &'a mut std::process::Command {
    cmd.env("CDO_SLOT_DIR", dir.join("slot"))
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("CDO_SLOT_POLL_S", "1")
        .env("CDO_SLOT_BUILD_NAMES", "tls89-none")
        .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
        // TLS-83: no instruction counting unless a test asks for it (a real
        // perf on the stub servers adds 0.2 s a call and reads nothing useful).
        .env("CDO_SPEED_INSTR", "0")
}

/// The speed gate pointed at an empty workspace, both sides this build's
/// `alsem` (`--base-exe`/`--head-exe`: no build, no git): the first timed
/// call exits 1, and the gate fails (exit 1) naming the call, its exit code
/// and its stderr, and never prints PASS.
///
/// Mutation: drop the non-zero exit check in `run_round` -> every call is
/// timed, the ratios of the failures pass, PASS and exit 0.
///
/// TLS-89: with no hold to share it takes the (private) slot itself, timed.
/// Mutation (2026-11-18): `timed_hold` returning early with no CDO_SLOT_TOKEN
/// (its first form) -> no "taken by cdo-speed" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_gate_fails_when_a_timed_call_fails() {
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_alsem");
    let run = private_slot(&mut crate::clean_env::command("python3"), out.path())
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(ws.path())
        .args(["--base-exe", exe, "--head-exe", exe, "--out"])
        .arg(out.path())
        .env("ALSEM_SPEED_CACHE", out.path().join("cache"))
        // TLS-65/TLS-76: no wait for a calm machine (the test's own load is no issue).
        .env("CDO_SPEED_CALM_PRESSURE", "100")
        .output()
        .expect("spawn python3 scripts/cdo-speed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(1), "{text}");
    assert!(
        text.contains("FAIL: r0.1 build (alsem_explain) on base exited 1"),
        "{text}"
    );
    assert!(text.contains("  | "), "the call's stderr: {text}");
    assert!(!text.contains("cdo-speed: PASS"), "{text}");
    // TLS-89 (review r1, sol): with no hold to share, it takes the slot itself, timed.
    assert!(
        text.contains("cdo-slot: taken by cdo-speed (cdo-speed timing)")
            && text.contains("cdo-slot: timed by cdo-speed (cdo-speed timing)"),
        "{text}"
    );
}

/// ⟨TLS-89⟩ Inside a hold (its token), cdo-speed marks it timed before its first
/// timed call; a build still running when the wait ends stops it there: exit 2,
/// "timed nothing", no server started. The stand-in build is bash through a symlink
/// named `t89g<test pid>` (unique: `pgrep -x` sees the whole machine), matched by
/// `cdo-slot`'s `pgrep -x` (CDO_SLOT_BUILD_NAMES).
///
/// Mutation (2026-11-18): the `timed_hold(repo)` call dropped from `main` -> the
/// warm-up starts beside the build (exit 1 on the empty workspace, "round 0") (red).
#[cfg(target_os = "linux")]
#[test]
fn speed_gate_times_nothing_beside_a_build() {
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_alsem");
    let slot = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
    let fake_name = format!("t89g{}", std::process::id());
    let fake = out.path().join(&fake_name);
    // A symlink (its process name is the link's): a copy can fail to exec with
    // "Text file busy" while another test thread forks.
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    // Reaped on any exit, a panic included (a leaked stand-in would hold up
    // later runs).
    // One process: the builtin `read` waits on a pipe nobody writes, so killing
    // it leaves no `sleep` child holding the test's output (a nextest leak).
    let build = crate::cdo_slot::Reap(
        crate::clean_env::command(&fake)
            .args(["-c", "read -t 600 _ || true"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let holder = crate::cdo_slot::Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let held = crate::clean_env::command("bash")
        .args([slot, "acquire", "gate", "test hold", "--bind"])
        .arg(holder.0.id().to_string())
        .env("CDO_SLOT_DIR", out.path().join("slot"))
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("CDO_SLOT_TOKEN", "tok-speed")
        .output()
        .unwrap();
    let run = crate::clean_env::command("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(ws.path())
        .args(["--base-exe", exe, "--head-exe", exe, "--out"])
        .arg(out.path())
        .env("ALSEM_SPEED_CACHE", out.path().join("cache"))
        .env("CDO_SPEED_CALM_PRESSURE", "100")
        .env("CDO_SLOT_DIR", out.path().join("slot"))
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("CDO_SLOT_POLL_S", "1")
        .env("CDO_SLOT_TOKEN", "tok-speed")
        .env("CDO_SLOT_BUILD_NAMES", &fake_name)
        .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
        .env("CDO_SLOT_TIMED_WAIT_S", "1")
        .output()
        .expect("spawn python3 scripts/cdo-speed");
    drop(build);
    drop(holder);
    assert!(held.status.success(), "{held:?}");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(2), "{text}");
    assert!(text.contains("timed nothing"), "{text}");
    assert!(!text.contains("round 0"), "{text}");
}

/// TLS-25: `--rounds` outside 1 to 5 (there are 5 round routines) is refused
/// at once, as a usage error naming the limit, before any server starts. The
/// same setup as above: with a valid count the gate would start both servers
/// and fail the warm-up (exit 1, "round 0"), so exit 2 and no "round 0" show
/// the refusal came first. `--rounds 5` is the precondition: it starts.
///
/// Mutation (2026-11-12): drop the range check in `main` -> `--rounds 6`
/// starts the servers, fails the warm-up on the empty workspace (exit 1);
/// FAIL. Restored, passes.
#[cfg(target_os = "linux")]
#[test]
fn speed_gate_refuses_more_rounds_than_routines() {
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_alsem");
    let run = |rounds: &str| {
        let r = private_slot(&mut crate::clean_env::command("python3"), out.path())
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
            .arg(ws.path())
            .args([
                "--base-exe",
                exe,
                "--head-exe",
                exe,
                "--rounds",
                rounds,
                "--out",
            ])
            .arg(out.path())
            .env("ALSEM_SPEED_CACHE", out.path().join("cache"))
            // TLS-65 (Fable r1)/TLS-76: no wait for a calm machine, which on a busy
            // box would hold the valid run for 10 min.
            .env("CDO_SPEED_CALM_PRESSURE", "100")
            .output()
            .expect("spawn python3 scripts/cdo-speed");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&r.stdout),
            String::from_utf8_lossy(&r.stderr)
        );
        (r.status.code(), text)
    };
    let (code, text) = run("5");
    assert_eq!(code, Some(1), "precondition: 5 rounds start: {text}");
    assert!(text.contains("round 0"), "precondition: {text}");
    for bad in ["6", "0"] {
        let (code, text) = run(bad);
        assert_eq!(code, Some(2), "--rounds {bad}: {text}");
        assert!(text.contains("give 1 to 5"), "--rounds {bad}: {text}");
        assert!(
            !text.contains("round 0"),
            "--rounds {bad}: nothing started: {text}"
        );
    }
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
        let out = crate::clean_env::command("bash")
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

/// TLS-58: `cdo-gate --only <phases>`, the pre-submit subset.
/// - `--dry-run` names exactly the subset's build targets and phases, and
///   never the speed check or the impact ratchets;
/// - an unknown phase (`impact`, `speed`, a typo) and a repeated one stop it
///   with exit 2 (a typo must not run a smaller check than asked for);
/// - the summary of a subset run judges only its phases (the run folder's
///   `only` file): with statuses for those two alone it PASSes, and its
///   verdict says SUBSET; the same folder without the `only` file FAILs on
///   the full gate's missing phases (the precondition that the list decides).
///
/// Mutation: `summarize` ignoring the `only` file -> the subset run FAILs
/// ("missing" for the phases it never ran).
#[cfg(unix)]
#[test]
fn cdo_gate_only_runs_and_judges_just_its_subset() {
    let gate = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate");
    let run = |args: &[&str]| {
        let out = crate::clean_env::command("bash")
            .arg(gate)
            .args(args)
            .output()
            .expect("spawn bash scripts/cdo-gate");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    let (code, text) = run(&["--only", "harness,l4", "--dry-run"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains(
            "build: cargo test --profile cdo --no-run --test program_resolve_harness --test l4_summary_differential\n"
        ),
        "only the subset's targets: {text}"
    );
    assert!(
        text.contains(
            "phases, one after another: program_resolve_harness l4_summary_differential\n"
        ),
        "{text}"
    );
    // Every phase name builds the target its phase runs (sol r1): `lib` the
    // lib (`-p al-sem --lib`, as `lib_tests`' cargo calls), `output` the cli.
    let (code, text) = run(&["--only", "output,lib,lsp,l4,harness", "--dry-run"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains(
            "build: cargo test --profile cdo --no-run --test cli -p al-sem --lib --test lsp \
             --test l4_summary_differential --test program_resolve_harness\n"
        ),
        "{text}"
    );
    for bad in [
        // `read` would drop a trailing empty field or a second line (sol r1).
        &["--only", "harness,"][..],
        &["--only", "harness\nspeed"],
        &["--only", "harness,impact"],
        &["--only", "speed"],
        &["--only", "l4,l4"],
        &["--only"],
        // An empty list must not fall through to the full gate (review r1).
        &["--only="],
        &["--only", ""],
        &["--only", "harness", "--only", "l4"],
    ] {
        let (code, text) = run(bad);
        assert_eq!(code, Some(2), "{bad:?} refused: {text}");
        // Refused by `--only`'s own check, not by a later one (no workspace
        // also exits 2, after a fall-through to the full gate).
        assert!(text.contains("cdo-gate: error: --only"), "{bad:?}: {text}");
    }
    let folder = |with_only: bool| {
        let dir = tempfile::tempdir().unwrap();
        if with_only {
            std::fs::write(
                dir.path().join("only"),
                "harness,l4\nprogram_resolve_harness\nl4_summary_differential\n",
            )
            .unwrap();
        }
        for p in ["program_resolve_harness", "l4_summary_differential"] {
            std::fs::write(dir.path().join(format!("{p}.status")), "0 1\n").unwrap();
            std::fs::write(
                dir.path().join(format!("{p}.log")),
                "    1 test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.01s\n",
            )
            .unwrap();
        }
        dir
    };
    let full = folder(false);
    let (code, text) = run(&[&format!("--summary={}", full.path().display())]);
    assert_eq!(
        code,
        Some(1),
        "precondition: the full gate's phases are missing: {text}"
    );
    let subset = folder(true);
    let (code, text) = run(&[&format!("--summary={}", subset.path().display())]);
    assert_eq!(code, Some(0), "the subset judges its own phases: {text}");
    assert!(
        text.contains("cdo-gate: PASS (SUBSET --only harness,l4: not a landing gate"),
        "{text}"
    );
    assert!(
        !text.contains("SPEED AND MEMORY WERE NOT CHECKED"),
        "{text}"
    );
    // An empty or cut-off list judges nothing, so it never passes (review r1).
    for cut in [
        "",
        "harness,l4\n",
        // Cut part-way: one of the two phases (sol r1).
        "harness,l4\nprogram_resolve_harness\n",
    ] {
        std::fs::write(subset.path().join("only"), cut).unwrap();
        let (code, text) = run(&[&format!("--summary={}", subset.path().display())]);
        assert_eq!(code, Some(1), "only file {cut:?}: {text}");
        assert!(text.contains("cdo-gate: FAIL (SUBSET"), "{text}");
    }
    // `--help` goes to stdout, so `cdo-gate --help | less` shows it.
    let help = crate::clean_env::command("bash")
        .args([gate, "--help"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("--only harness,l4"),
        "{help:?}"
    );
}

/// ⟨TLS-87⟩ A passing `--only` run records its phases under the tree it ran on,
/// in the shared git directory (`<common dir>/cdo-presubmit/<tree>`), so
/// `scripts/land` can check a branch had its pre-submit subset. The gate runs
/// from a throwaway git repository holding a copy of the scripts it needs, with
/// a stub `cargo` that passes every phase (or fails, or commits during the run):
/// it records nothing on a work tree with an untracked file (it would not name
/// exactly HEAD's tree), nothing for a FAILING run, nothing when run from
/// another checkout (cargo builds the current folder, not the script's), and
/// nothing when a commit lands during the run; clean, it records `harness,l4`;
/// a second run (`lsp`) on the same tree adds to the record.
///
/// Mutations (2026-11-18): the `record_presubmit` call dropped -> no record
/// (red); the clean-tree check dropped from `record_presubmit` -> the dirty run
/// records (red). Review r1 (Fable): the `rc == 0` condition dropped -> the
/// failing run records (red); the checkout check dropped from
/// `presubmit_clean_tree` -> the run from another checkout records (red); the
/// same-tree comparison dropped -> the run with a commit during it records its
/// starting tree (red). Each restored, green. Review r1 (sol): the
/// `presubmit_touched` check dropped -> the run whose test phase changed and
/// restored a tracked file records (red).
#[cfg(unix)]
#[test]
fn cdo_gate_only_pass_records_the_tree_it_ran_on() {
    use std::os::unix::fs::PermissionsExt;
    let repo = tempfile::tempdir().unwrap();
    let stub = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let scripts = repo.path().join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    for f in [
        "cdo-gate",
        "cdo-slot",
        // cdo-slot sources it (TLS-105).
        "awake-clock",
        "require-rttime",
        "cdo-speed",
        "host-suspended",
        "ir-cache-report-check",
    ] {
        let from = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join(f);
        std::fs::copy(from, scripts.join(f)).unwrap();
    }
    crate::git::run(repo.path(), &["init", "-q", "-b", "master"]);
    crate::git::run(repo.path(), &["add", "scripts"]);
    crate::git::run(repo.path(), &["commit", "-q", "-m", "scripts"]);
    let tree =
        String::from_utf8(crate::git::run(repo.path(), &["rev-parse", "HEAD^{tree}"]).stdout)
            .unwrap()
            .trim()
            .to_string();
    // STUB_FAIL: every test phase fails. STUB_COMMIT=<repo>: the first test
    // phase commits a new file there (a commit landing during the run).
    // STUB_SUSPEND=<n> (TLS-104): a test phase sets the host's suspended-seconds
    // reading (CDO_HOST_SUSPENDED_FILE) to n: the host "slept" during it.
    std::fs::write(
        stub.path().join("cargo"),
        "#!/bin/sh\n\
         case \" $* \" in *' --no-run '*) ;; *)\n\
           if [ -n \"$STUB_SUSPEND\" ]; then echo \"$STUB_SUSPEND\" > \"$CDO_HOST_SUSPENDED_FILE\"; fi\n\
           if [ -n \"$STUB_FAIL\" ]; then\n\
             echo 'test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s'; exit 101\n\
           fi\n\
           if [ -n \"$STUB_TOUCH\" ]; then\n\
             cp \"$STUB_TOUCH\" \"$STUB_TOUCH.bak\"; echo x >> \"$STUB_TOUCH\"\n\
             cp \"$STUB_TOUCH.bak\" \"$STUB_TOUCH\"; rm \"$STUB_TOUCH.bak\"\n\
           fi\n\
           if [ -n \"$STUB_COMMIT\" ] && [ ! -e \"$STUB_COMMIT/late.txt\" ]; then\n\
             echo x > \"$STUB_COMMIT/late.txt\"\n\
             git -C \"$STUB_COMMIT\" add late.txt\n\
             git -C \"$STUB_COMMIT\" -c user.name=t -c user.email=t@t commit -qm late\n\
           fi\n\
           if [ -n \"$ALSEM_IR_CACHE_REPORT\" ]; then\n\
             echo '{\"event\":\"start\",\"pid\":7}' >> \"$ALSEM_IR_CACHE_REPORT\"\n\
             echo '{\"event\":\"end\",\"pid\":7,\"counters\":{\"eligible_files\":3,\"published_packages\":1}}' >> \"$ALSEM_IR_CACHE_REPORT\"\n\
           fi ;;\n\
         esac\n\
         echo 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s'\n",
    )
    .unwrap();
    std::fs::set_permissions(
        stub.path().join("cargo"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        stub.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let other = tempfile::tempdir().unwrap();
    crate::git::run(other.path(), &["init", "-q", "-b", "master"]);
    // <subset>, the folder it runs from, extra environment, the exit it must give.
    let gate = |only: &str, cwd: &std::path::Path, env: &[(&str, &str)], rc: i32| {
        let mut cmd = crate::clean_env::command("bash");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd
            .current_dir(cwd)
            .arg(scripts.join("cdo-gate"))
            .args(["--only", only])
            .arg(ws.path())
            .env("PATH", &path)
            // Stub tools: a HOME of its own (TLS-115).
            .env("HOME", stub.path())
            .env("CARGO_TARGET_DIR", target.path())
            .env("CDO_SLOT_DIR", target.path().join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .output()
            .expect("spawn bash scripts/cdo-gate");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(rc), "{text}");
        text
    };
    let here = repo.path();
    let records = repo.path().join(".git/cdo-presubmit");
    let record = records.join(&tree);

    // Not clean (an untracked file): no record.
    std::fs::write(repo.path().join("scratch.txt"), "x").unwrap();
    let text = gate("harness,l4", here, &[], 0);
    assert!(text.contains("no pre-submit record"), "{text}");
    assert!(!record.exists(), "a dirty run recorded a pass");
    std::fs::remove_file(repo.path().join("scratch.txt")).unwrap();

    // A failing run: no record.
    gate("harness,l4", here, &[("STUB_FAIL", "1")], 1);
    assert!(!record.exists(), "a failing run recorded a pass");

    // Run from another checkout (cargo would build that one): no record.
    let text = gate("harness,l4", other.path(), &[], 0);
    assert!(text.contains("no pre-submit record"), "{text}");
    assert!(
        !record.exists(),
        "a run from another checkout recorded this tree"
    );

    // Review r1 (sol): a tracked file changed during the run and restored
    // before its end (clean at both ends): no record.
    let touched = scripts.join("require-rttime");
    let touch = [("STUB_TOUCH", touched.to_str().unwrap())];
    let text = gate("harness,l4", here, &touch, 0);
    assert!(text.contains("modified during the run"), "{text}");
    assert!(!record.exists(), "a run on changed code recorded the tree");

    // Clean, from its own checkout: recorded.
    let text = gate("harness,l4", here, &[], 0);
    assert!(text.contains("pre-submit record for tree"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "l4_summary_differential\nprogram_resolve_harness\n"
    );

    // Another subset on the same tree adds its phases.
    gate("lsp", here, &[], 0);
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "l4_summary_differential\nlsp\nprogram_resolve_harness\n"
    );

    // A commit lands during the run: neither tree gets this run's phase.
    let commit = [("STUB_COMMIT", repo.path().to_str().unwrap())];
    let text = gate("output", here, &commit, 0);
    assert!(text.contains("no pre-submit record"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "l4_summary_differential\nlsp\nprogram_resolve_harness\n",
        "the starting tree got a phase it was not tested on to the end"
    );
    assert_eq!(
        std::fs::read_dir(&records).unwrap().count(),
        1,
        "the new tree got a record"
    );

    // ⟨TLS-104⟩ The host suspends 30 s during the first test phase (the stub
    // sets the reading from 100 to 130): that phase's line and the summary say
    // so; the next phase (reading unchanged) says nothing. The control: no
    // suspend, no note. Mutation (2026-11-20): `suspend_note` always returning
    // early -> no note (red); restored, green.
    let reading = target.path().join("suspended");
    let reading_s = reading.to_str().unwrap().to_string();
    std::fs::write(&reading, "100").unwrap();
    let env = [
        ("CDO_HOST_SUSPENDED_FILE", reading_s.as_str()),
        ("STUB_SUSPEND", "130"),
    ];
    let text = gate("harness,l4", other.path(), &env, 0);
    let noted: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("the host was suspended 30 s during this phase"))
        .collect();
    assert_eq!(noted.len(), 1, "one phase saw the suspend: {text}");
    assert!(
        noted[0].starts_with("cdo-gate: [program_resolve_harness]"),
        "{text}"
    );
    assert!(
        text.contains("cdo-gate:   host suspended 30 s during program_resolve_harness (its seconds include that time)"),
        "{text}"
    );
    std::fs::write(&reading, "100").unwrap();
    let text = gate(
        "harness,l4",
        other.path(),
        &[("CDO_HOST_SUSPENDED_FILE", reading_s.as_str())],
        0,
    );
    assert!(!text.contains("suspended"), "{text}");
}

/// TLS-58 review r1: a real `--only harness,l4,lsp,output` run, with a stub
/// `cargo` that logs each call. It builds just those targets, runs just those
/// phases, each with the full gate's command and `ENFORCE_CDO_WS=1` and with
/// the memory sampler running (where the container's cgroup is readable),
/// never the speed check or the impact ratchets, writes `only` (not
/// `split-impact`), and its verdict is SUBSET. (`lib`'s run needs a libtest
/// listing a stub cannot fake; its build selector is pinned by the dry run.)
///
/// Mutations: drop `speed=0` from the `--only` branch -> the speed phase runs
/// and the run is no longer the subset it claims; start `memory_sampler` only
/// outside a subset -> `SAMPLED=no`; TLS-74: `gate_tree_rss` without its
/// `engines=N` field, or counting nothing (`engines=0` written as is) -> no
/// sample line matches -> `SAMPLED=no`.
#[cfg(unix)]
#[test]
fn cdo_gate_only_run_builds_and_runs_just_its_phases() {
    use std::os::unix::fs::PermissionsExt;
    let stub = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let calls = stub.path().join("calls");
    // The sampler starts once the build is done, and writes its first sample
    // at once; each phase call waits (up to 10 s) to see it. TLS-74: the phase
    // call runs a stand-in `alsem` (bash under that name; `; :` keeps bash from
    // replacing itself with `sleep`) meanwhile, so the
    // sample it waits for must count one (`engines=` 1 or more).
    let sampler = std::fs::read_to_string("/sys/fs/cgroup/memory.stat").is_ok();
    std::os::unix::fs::symlink("/bin/bash", stub.path().join("alsem")).unwrap();
    std::fs::write(
        stub.path().join("cargo"),
        format!(
            "#!/bin/sh\ns=''\n\
             case \" $* \" in *' --no-run '*) ;; *)\n\
               s=' SAMPLED=no'\n\
               if [ '{sampler}' = true ]; then '{}' -c 'sleep 12; :' & a=$!; i=0; while [ $i -lt 100 ]; do\n\
                 if grep -qsE '^[0-9]+ [0-9]+ [0-9]+ engines=[1-9][0-9]*' \"$CARGO_TARGET_DIR\"/cdo-gate-logs/*/memory.timeline; then s=' SAMPLED=yes'; break; fi\n\
                 sleep 0.1; i=$((i + 1)); done; kill $a 2>/dev/null; else s=''; fi ;;\n\
             esac\n\
             echo \"ENFORCE=$ENFORCE_CDO_WS$s IR=$ALSEM_IR_CACHE $*\" >> '{}'\n\
             if [ -n \"$ALSEM_IR_CACHE_REPORT\" ]; then\n\
               echo '{{\"event\":\"start\",\"pid\":7}}' >> \"$ALSEM_IR_CACHE_REPORT\"\n\
               echo '{{\"event\":\"end\",\"pid\":7,\"counters\":{{\"eligible_files\":3,\"published_packages\":1}}}}' >> \"$ALSEM_IR_CACHE_REPORT\"\n\
             fi\n\
             CLAUDE_CODE_SESSION_ID=other CDO_SLOT_TOKEN= bash '{}' status >> '{}' 2>&1\n\
             echo 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s'\n",
            stub.path().join("alsem").display(),
            calls.display(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot"),
            stub.path().join("slot-seen").display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        stub.path().join("cargo"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        stub.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .args(["--only", "harness,l4,lsp,output"])
        .arg(ws.path())
        .env("PATH", path)
        .env("HOME", stub.path())
        .env("CARGO_TARGET_DIR", target.path())
        // A slot of its own (TLS-60): never the tentacle's real one.
        .env("CDO_SLOT_DIR", target.path().join("slot"))
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        // TLS-87: a stubbed pass must never record a pre-submit pass for the
        // real tree (it would on a clean checkout).
        .env("CDO_PRESUBMIT_DIR", target.path().join("presubmit"))
        // The `.cargo/config.toml` `[env]` default a real cargo gives each test;
        // the stub cargo does not apply it (TLS-115: no longer inherited).
        .env("ALSEM_IR_CACHE", "off")
        .output()
        .expect("spawn bash scripts/cdo-gate");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("cdo-gate: PASS (SUBSET --only harness,l4,lsp,output: not a landing gate"),
        "{text}"
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    let lines: Vec<&str> = calls.lines().collect();
    let s = if sampler { " SAMPLED=yes" } else { "" };
    assert_eq!(
        lines,
        [
            // ENG-110: the IR cache on for output_cdo only; the others keep the
            // test's own (cargo `[env]`) off.
            "ENFORCE=1 IR=off test --profile cdo --no-run --test program_resolve_harness \
             --test l4_summary_differential --test lsp --test cli"
                .to_string(),
            format!(
                "ENFORCE=1{s} IR=off test --profile cdo --test program_resolve_harness -- --test-threads=4"
            ),
            format!("ENFORCE=1{s} IR=off test --profile cdo --test l4_summary_differential"),
            format!(
                "ENFORCE=1{s} IR=off test --profile cdo --test lsp -- program_graph:: snapshot_robustness::"
            ),
            format!(
                "ENFORCE=1{s} IR=on test --profile cdo --test cli output_cdo:: -- --skip output_cdo::cdo_impact"
            ),
        ],
        "{text}"
    );
    // ENG-110: the output phase's report was judged (the stub wrote a start and an end
    // line) and the summary says so.
    assert!(
        text.contains("cdo-gate:   IR cache output_cdo: IR: PASS output"),
        "{text}"
    );
    let runs: Vec<_> = std::fs::read_dir(target.path().join("cdo-gate-logs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].join("only").is_file());
    assert!(!runs[0].join("split-impact").exists());
    assert!(!runs[0].join("speed.status").exists());
    // A tree with no product change takes the TLS-20 speed skip instead, which
    // would hide a subset that tried the speed check (Fable r2).
    assert!(!runs[0].join("speed-skipped").exists());
    // TLS-60: the gate held the CDO slot through every cargo call, as another
    // session saw it (mutation: cdo-gate without its acquire -> `free`).
    let seen = std::fs::read_to_string(stub.path().join("slot-seen")).unwrap();
    assert_eq!(seen.lines().count(), 5, "{seen}");
    for line in seen.lines() {
        assert!(
            line.starts_with("held by cdo-gate (cdo-gate --only harness,l4,lsp,output) since "),
            "{seen}"
        );
    }
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
    let run = crate::clean_env::command("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .args(["--warm", &head])
        .env("ALSEM_SPEED_CACHE", cache.path())
        .env("PATH", path)
        .env("HOME", stub.path())
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

/// TLS-33: the speed check builds the working tree in ci-steps' optimized
/// build folder (`<target>/gate-release`), where `ci-steps all` already built
/// the same tree, not in the main target folder (where it rebuilt al-sem). A
/// stub `cargo` first on `PATH` records the `CARGO_TARGET_DIR` it is given and
/// leaves a binary there; the base is a given binary, so nothing else builds.
///
/// Mutation (2026-11-13): `build_working_tree` without its `CARGO_TARGET_DIR`
/// -> the stub builds in the main target folder, `cdo-speed` finds no binary
/// in `gate-release` and exits 2 (the exit-code assert fails).
#[cfg(target_os = "linux")]
#[test]
fn speed_build_uses_the_gate_release_folder() {
    use std::os::unix::fs::PermissionsExt;
    let target = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let stub = tempfile::tempdir().unwrap();
    let record = stub.path().join("target-dir");
    let cargo = stub.path().join("cargo");
    std::fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s' \"$CARGO_TARGET_DIR\" > '{}'\n\
             mkdir -p \"$CARGO_TARGET_DIR/release-fast\" && : > \"$CARGO_TARGET_DIR/release-fast/alsem\"\n",
            record.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let base = stub.path().join("base-alsem");
    std::fs::write(&base, b"placeholder").unwrap();
    let path = format!(
        "{}:{}",
        stub.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = crate::clean_env::command("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(ws.path())
        .args(["--build-only", "--base-exe"])
        .arg(&base)
        .arg("--out")
        .arg(target.path().join("out"))
        .env("CARGO_TARGET_DIR", target.path())
        .env("ALSEM_SPEED_CACHE", target.path().join("speed-cache"))
        .env("PATH", path)
        .env("HOME", stub.path())
        .output()
        .expect("spawn python3 scripts/cdo-speed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.status.code(), Some(0), "{text}");
    assert_eq!(
        std::fs::read_to_string(&record).expect("the stub cargo ran"),
        target.path().join("gate-release").display().to_string(),
        "the working tree is built in ci-steps' folder: {text}"
    );
}

/// TLS-77: `scripts/profile` builds the `profiling` profile and records it with
/// samply, and a profile of a failed command is not reported as a result.
/// - `--dry-run`: the exact build and record commands, arguments quoted;
/// - usage errors (another binary, `-o` with no file) exit 2;
/// - a real run against stub `cargo` (it builds a fake binary that exits
///   `STUB_RC`) and `samply` (it runs what follows `--`, as samply does, and exits
///   0 whatever it did): exit 0 and "wrote" for a clean command; the command's own
///   exit code and a warning for a failed one (review TLS-77, Fable: samply's 0
///   made a usage error look like a profile);
/// - the profile in Cargo.toml keeps what samply reads: symbols and line tables.
///
/// Mutations (2026-11-15): the exit-code check after `samply record` removed ->
/// the failed command's run exits 0 and the test fails; the missing-status
/// check removed -> the never-run command reads as "wrote", exit 0.
#[cfg(unix)]
#[test]
fn profile_script_builds_symbols_and_passes_the_exit_code() {
    use std::os::unix::fs::PermissionsExt;
    let root = env!("CARGO_MANIFEST_DIR");
    let stub = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let log = stub.path().join("log");
    for (name, body) in [
        (
            "cargo",
            "printf 'cargo %s\\n' \"$*\" >> \"$STUB_LOG\"\n\
             mkdir -p \"$CARGO_TARGET_DIR/profiling\"\n\
             printf '#!/usr/bin/env bash\\nexit \"$STUB_RC\"\\n' > \"$CARGO_TARGET_DIR/profiling/alsem\"\n\
             chmod +x \"$CARGO_TARGET_DIR/profiling/alsem\"",
        ),
        (
            "samply",
            "printf 'samply %s\\n' \"$*\" >> \"$STUB_LOG\"\n\
             [ \"${STUB_NORUN:-}\" = 1 ] && exit 0\n\
             while [ \"$1\" != -- ]; do shift; done; shift\n\
             \"$@\"; exit 0",
        ),
    ] {
        let p = stub.path().join(name);
        std::fs::write(&p, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run = |args: &[&str], rc: &str| {
        let _ = std::fs::remove_file(&log);
        let out = crate::clean_env::command("bash")
            .arg(format!("{root}/scripts/profile"))
            .args(args)
            .current_dir(stub.path())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    stub.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("HOME", stub.path())
            .env("STUB_LOG", &log)
            .env("STUB_RC", rc)
            // "norun": samply exits 0 without running the command.
            .env("STUB_NORUN", if rc == "norun" { "1" } else { "0" })
            .env("CARGO_TARGET_DIR", target.path())
            .output()
            .expect("spawn bash scripts/profile");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    let exe = format!("{}/profiling/alsem", target.path().display());

    let (code, text) = run(
        &["--dry-run", "-o", "my out.gz", "alsem", "--x", "a b"],
        "0",
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains(&format!(
            "build: cargo build --manifest-path {root}/Cargo.toml --profile profiling --bin alsem\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "record: samply record --save-only --unstable-presymbolicate -o my\\ out.gz -- {exe} --x a\\ b\n"
        )),
        "{text}"
    );
    assert!(!log.exists(), "a dry run runs nothing");
    let (code, text) = run(&["--dry-run", "cargo"], "0");
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("usage: scripts/profile"), "{text}");
    let (code, text) = run(&["-o"], "0");
    assert_eq!(code, Some(2), "{text}");

    let (code, text) = run(&["-o", "p.json.gz", "alsem", "fingerprint", "ws"], "0");
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("scripts/profile: wrote p.json.gz"), "{text}");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains(&format!(
            "cargo build --manifest-path {root}/Cargo.toml --profile profiling --bin alsem\n"
        )),
        "{calls}"
    );
    assert!(
        calls.contains("samply record --save-only --unstable-presymbolicate -o p.json.gz -- "),
        "{calls}"
    );
    assert!(calls.contains(&format!("{exe} fingerprint ws")), "{calls}");

    let (code, text) = run(&["-o", "p.json.gz", "alsem", "fingerprint", "ws"], "3");
    assert_eq!(code, Some(3), "{text}");
    assert!(
        text.contains("the profiled command exited 3: p.json.gz profiles a failed run"),
        "{text}"
    );
    assert!(!text.contains("wrote p.json.gz"), "{text}");

    // The command never ran (no exit status written): not a result either
    // (review r2, sol).
    let (code, text) = run(&["-o", "p.json.gz", "alsem", "fingerprint", "ws"], "norun");
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("exit status is unavailable"), "{text}");
    assert!(!text.contains("wrote p.json.gz"), "{text}");

    // The profile itself keeps what samply reads: symbols and line tables.
    let cargo_toml =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let section = cargo_toml
        .split("[profile.profiling]")
        .nth(1)
        .expect("a [profile.profiling] section")
        .split("\n[")
        .next()
        .unwrap();
    for want in [
        "inherits = \"release-fast\"",
        "debug = \"line-tables-only\"",
        "strip = false",
    ] {
        assert!(section.contains(want), "{want}: {section}");
    }
}

/// TLS-74: the summary's per-phase memory, from a hand-written run folder: each
/// phase's peak is the highest container sample in [start, end) with that
/// sample's gate MiB and `alsem` count; a sample at a phase's end second belongs
/// to the next phase; a timeline from before TLS-74 (no `engines=`) shows "?";
/// a phase that ended before the first sample prints nothing; peaks compare as
/// numbers (1000 over 900).
///
/// Mutation (2026-11-15): the window's end made inclusive (`<=`) -> phase a's
/// peak becomes b's first sample (900 MiB, 4 processes) and the test fails.
#[cfg(unix)]
#[test]
fn cdo_gate_summary_names_each_phases_peak_memory() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path();
    std::fs::write(
        logs.join("phases.times"),
        "90 start z\n95 end z\n100 start a\n110 end a\n110 start b\n130 end b\n130 start c\n140 end c\n",
    )
    .unwrap();
    std::fs::write(
        logs.join("memory.timeline"),
        "100 300 200 engines=1 alsem/1:150\n\
         104 500 450 engines=2 alsem/1:200 alsem/2:200\n\
         108 400 380 engines=2 alsem/1:190\n\
         110 900 880 engines=4 alsem/3:220\n\
         120 1000 950 engines=3 alsem/3:210\n\
         132 600 550 x/9:500\n",
    )
    .unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(logs.join(format!("{name}.status")), "0 10\n").unwrap();
    }
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg(format!("--summary={}", logs.display()))
        .output()
        .expect("spawn bash scripts/cdo-gate --summary");
    let text = String::from_utf8_lossy(&out.stdout);
    let line = |phase: &str| {
        text.lines()
            .find(|l| l.contains(&format!("memory {phase} ")))
            .unwrap_or_else(|| panic!("no memory line for {phase}: {text}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(
        line("a").ends_with(
            "peak 500 MiB (container, unreclaimable), the gate 450 MiB, alsem processes 2"
        ),
        "{text}"
    );
    assert!(
        line("b").ends_with(
            "peak 1000 MiB (container, unreclaimable), the gate 950 MiB, alsem processes 3"
        ),
        "{text}"
    );
    assert!(
        line("c").ends_with(
            "peak 600 MiB (container, unreclaimable), the gate 550 MiB, alsem processes ?"
        ),
        "{text}"
    );
    assert!(!text.contains("memory z "), "{text}");
}

/// TLS-32: the test phases' layout (`cdo-gate --layout` prints what
/// `plan_layout` decides, the decision the gate makes after `output_cdo`).
/// Lane 2 runs beside the impact phase at the most threads (4 down to 1) whose
/// need (impact's own worst 16,100 + lane 2's worst: lib_cdo's own at that count,
/// 17,900, 13,600 or 9,100 since TLS-35, but never under the 8,400 of the phases
/// before it + 4,096 MiB) fits BOTH the host's MemAvailable and the
/// container's free memory (`memory.max` minus what cannot be reclaimed);
/// otherwise lane 2 runs after it; `CDO_GATE_SERIAL=1` wins over all. The host
/// is a hand-written meminfo (`CDO_GATE_MEMINFO`) and the container a
/// hand-written cgroup folder (`CDO_GATE_CGROUP`).
///
/// Mutations (2026-11-13): `plan_layout`'s container test dropped (host only)
/// -> the busy container is called parallel at 2 threads; an unreadable
/// container reported `unlimited` instead of `unknown` (fail open) -> "no
/// memory.stat" is called parallel; `container_free_mib` without its numeric
/// check -> `anon garbage` reads as 0 and is called parallel at 2 threads.
/// TLS-35 (2026-11-15): the thread loop back at `2 1` -> the free container
/// is called parallel at 2 threads, not 4; lane 2's 8,400 floor dropped (the
/// need from lib_cdo alone) -> 27,500 free MiB is called parallel at 1 thread.
#[cfg(unix)]
#[test]
fn cdo_gate_runs_lane_two_beside_impact_only_with_the_memory() {
    const MIB: u64 = 1024 * 1024;
    let dir = tempfile::tempdir().unwrap();
    // host MemAvailable and container use in MiB (`None`: no container limit).
    let layout = |host_mib: u64, used_mib: Option<u64>, serial: bool| {
        let case = dir.path().join(format!("{host_mib}-{used_mib:?}-{serial}"));
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(
            case.join("meminfo"),
            format!(
                "MemTotal: 98000000 kB\nMemAvailable: {} kB\n",
                host_mib * 1024
            ),
        )
        .unwrap();
        let max = used_mib.map_or("max".to_string(), |_| (49_152 * MIB).to_string());
        std::fs::write(case.join("memory.max"), format!("{max}\n")).unwrap();
        // 1 GiB of reclaimable slab on top, which must not count.
        std::fs::write(
            case.join("memory.stat"),
            format!(
                "anon {}\nshmem 0\nkernel {}\nslab_reclaimable {}\n",
                used_mib.unwrap_or(0) * MIB,
                1024 * MIB,
                1024 * MIB
            ),
        )
        .unwrap();
        let mut cmd = crate::clean_env::command("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", case.join("meminfo"))
            .env("CDO_GATE_CGROUP", &case)
            .env("CDO_GATE_LAYOUT", "parallel");
        if serial {
            cmd.env("CDO_GATE_SERIAL", "1");
        }
        let out = cmd.output().expect("spawn bash scripts/cdo-gate --layout");
        assert_eq!(out.status.code(), Some(0));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let check = |host: u64, used: Option<u64>, serial: bool, want: &str| {
        let got = layout(host, used, serial);
        assert!(
            got.starts_with(want),
            "host {host} MiB, container use {used:?} MiB, serial {serial}: want {want:?}, got {got}"
        );
    };
    let at = |n: u32| format!("parallel: lane 2 beside the impact phase, lib_cdo at {n} thread(s)");
    // Needs: 4 threads 38,096 MiB, 3 33,796, 2 29,296, 1 28,596 (lane 2's
    // phases before lib_cdo, 8,400, outweigh lib_cdo at 1 thread).
    check(60_000, Some(2_000), false, &at(4)); // container free 47,152
    check(60_000, Some(12_000), false, &at(3)); // container free 37,152
    check(60_000, Some(18_000), false, &at(2)); // container free 31,152
    check(60_000, Some(20_200), false, &at(1)); // container free 28,952
    check(60_000, Some(21_652), false, "sequential:"); // container free 27,500
    check(31_000, Some(2_000), false, &at(2)); // the host is short for 3
    check(20_000, Some(2_000), false, "sequential:"); // the host is short for 1
    check(60_000, None, false, &at(4)); // no container limit: the host decides
    check(60_000, Some(2_000), true, "serial:");
    // An unknown container is no room (review r2, sol: fail closed): a
    // `memory.stat` that cannot be read, or a `memory.max` that is no number.
    let broken = dir.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("memory.max"), format!("{}\n", 49_152 * MIB)).unwrap();
    let meminfo = broken.join("meminfo");
    std::fs::write(&meminfo, "MemAvailable: 62914560 kB\n").unwrap();
    let unknown = |dir: &std::path::Path| {
        let out = crate::clean_env::command("bash")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", &meminfo)
            .env("CDO_GATE_CGROUP", dir)
            .env("CDO_GATE_LAYOUT", "parallel")
            .output()
            .expect("spawn bash scripts/cdo-gate --layout");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let got = unknown(&broken); // no memory.stat
    assert!(got.starts_with("sequential:"), "no memory.stat: {got}");
    std::fs::write(broken.join("memory.stat"), "anon 0\nkernel 0\n").unwrap();
    std::fs::write(broken.join("memory.max"), "garbage\n").unwrap();
    let got = unknown(&broken);
    assert!(
        got.starts_with("sequential:"),
        "a garbage memory.max: {got}"
    );
    // A malformed or partial memory.stat (review r3, sol: awk reads garbage as 0).
    std::fs::write(broken.join("memory.max"), format!("{}\n", 49_152 * MIB)).unwrap();
    for stat in ["anon garbage\nshmem 0\nkernel 0\n", "anon 0\nkernel 0\n"] {
        std::fs::write(broken.join("memory.stat"), stat).unwrap();
        let got = unknown(&broken);
        assert!(
            got.starts_with("sequential:"),
            "memory.stat {stat:?}: {got}"
        );
    }
    // Exactly at the need is enough; one MiB short is not.
    check(60_000, Some(49_152 - 38_096), false, &at(4));
    check(60_000, Some(49_152 - 38_096 + 1), false, &at(3));
    check(38_096, Some(2_000), false, &at(4));
    check(38_095, Some(2_000), false, &at(3));
}

/// TLS-32 review r3-r4 (sol): a TERM sent to the gate's PID alone, during its
/// build phase, stops the gate at once (exit 130) and the phase's processes
/// with it. A stub `cargo` first on `PATH` sleeps (as a long build would) and
/// writes its PID; the gate runs `--no-speed` on an empty folder.
///
/// Mutation (2026-11-13): `run_phase` back to a foreground pipeline -> the
/// trap waits for the build, and the gate misses its deadline.
///
/// ⟨TLS-64⟩ The waits are 120 s, the stub's build 600 s: a gate that stops at
/// once takes about a second idle, but a 20 s deadline failed a landing gate at
/// host load ~500 (TLS-58's, 2026-11-14). A gate that waits for the build
/// still misses 120 s by far, and so does a build process that survives it.
#[cfg(target_os = "linux")]
#[test]
fn cdo_gate_stops_at_once_on_term_during_its_build() {
    use std::os::unix::fs::PermissionsExt;
    let stub = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let pid_file = stub.path().join("cargo.pid");
    std::fs::write(
        stub.path().join("cargo"),
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 600\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        stub.path().join("cargo"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        stub.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    use std::os::unix::process::CommandExt;
    let mut gate = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg("--no-speed")
        .arg(ws.path())
        .env("PATH", path)
        .env("HOME", stub.path())
        .env("CARGO_TARGET_DIR", target.path())
        // A slot of its own (TLS-60): never the tentacle's real one.
        .env("CDO_SLOT_DIR", target.path().join("slot"))
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Its own process group, so the guard below can end all of it.
        .process_group(0)
        .spawn()
        .expect("spawn bash scripts/cdo-gate");
    let gate_pid = gate.id() as i32;
    // Whatever way this test ends, nothing of the gate outlives it (a panic
    // would otherwise leave the gate calling the 600 s stub phase after phase,
    // Fable r1). The TERM below still goes to the gate's PID alone.
    struct KillGroup(i32);
    impl Drop for KillGroup {
        fn drop(&mut self) {
            // SAFETY: SIGKILL to the process group this test created.
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
        }
    }
    let _group = KillGroup(gate_pid);
    // Every wait below; far under the stub's 600 s build.
    let limit = std::time::Duration::from_secs(120);
    // The build phase is running when the stub's PID file reads as a number
    // (the shell creates the file before it writes it). A gate that ends
    // before its build fails at once, with its exit.
    let started = std::time::Instant::now();
    let build_pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            break pid;
        }
        if let Some(s) = gate.try_wait().unwrap() {
            panic!("the gate ended before its build phase: {s}");
        }
        assert!(
            started.elapsed() < limit,
            "the gate never reached its build phase within {limit:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // SAFETY: plain kill(2) on a child process this test owns.
    assert_eq!(unsafe { libc::kill(gate_pid, libc::SIGTERM) }, 0);
    let termed = std::time::Instant::now();
    let deadline = termed + limit;
    let status = loop {
        if let Some(s) = gate.try_wait().unwrap() {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the gate did not stop within {limit:?} of a TERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(130));
    // The build's process dies of the gate's TERM (a zombie counts as dead);
    // under load that can take a while, so poll instead of one fixed pause.
    let alive = || {
        // SAFETY: signal 0 only probes whether the process exists.
        let exists = unsafe { libc::kill(build_pid, 0) } == 0;
        exists
            && std::fs::read_to_string(format!("/proc/{build_pid}/stat"))
                .is_ok_and(|s| !s.contains(") Z "))
    };
    while alive() {
        assert!(
            termed.elapsed() < limit,
            "the build phase's process was still alive {limit:?} after the TERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!("gate stopped {:?} after the TERM", termed.elapsed());
}

/// TLS-34 review r1 (sol): no test here may write the real speed cache
/// (`~/.cache/al-sem/cdo-speed`). A test that builds or warms through
/// `cdo-speed` would leave a stub binary there, and a later gate could time
/// against it as its base. Every `cdo-speed` run in this file sets
/// `ALSEM_SPEED_CACHE` before it runs (`.output()` / `.spawn()`).
///
/// Mutation (2026-11-13): drop `ALSEM_SPEED_CACHE` from
/// `speed_build_uses_the_gate_release_folder` -> named here.
#[test]
fn speed_tests_never_use_the_real_cache() {
    let text = include_str!("gate_scripts.rs");
    let needle = concat!("\"/scripts/", "cdo-speed\"");
    let mut missing = Vec::new();
    for (i, part) in text.split(needle).enumerate().skip(1) {
        let run = part.split([';']).next().unwrap_or(part);
        // Only a statement that runs the script (a copy of it is no run).
        let Some(until) = [".output()", ".spawn()"]
            .iter()
            .filter_map(|end| run.find(end))
            .min()
        else {
            continue;
        };
        if !run[..until].contains("ALSEM_SPEED_CACHE") {
            missing.push(format!(
                "run #{i}: {}",
                run.lines().take(3).collect::<Vec<_>>().join(" ")
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "cdo-speed runs without ALSEM_SPEED_CACHE:\n{}",
        missing.join("\n")
    );
}

/// A throwaway checkout for TLS-34's tests: a git repo holding a copy of
/// `scripts/cdo-speed` (which finds its repo from its own path) and one file,
/// committed; a grammar clone (another git repo, `TREE_SITTER_AL_PATH`); a stub
/// `cargo` that builds a placeholder `alsem` (or fails); an empty workspace; a
/// base binary. Runs never touch the real checkout or cache.
#[cfg(target_os = "linux")]
struct Throwaway {
    dir: tempfile::TempDir,
}

#[cfg(target_os = "linux")]
impl Throwaway {
    fn new() -> Throwaway {
        use std::os::unix::fs::PermissionsExt;
        let t = Throwaway {
            dir: tempfile::tempdir().unwrap(),
        };
        for repo in ["repo", "grammar"] {
            std::fs::create_dir_all(t.path(repo)).unwrap();
            t.git(repo, &["init", "-q"]);
            std::fs::write(t.path(repo).join("a.txt"), "one\n").unwrap();
        }
        std::fs::create_dir_all(t.path("repo/scripts")).unwrap();
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"),
            t.path("repo/scripts/cdo-speed"),
        )
        .unwrap();
        for repo in ["repo", "grammar"] {
            t.commit(repo);
        }
        std::fs::create_dir_all(t.path("ws")).unwrap();
        std::fs::create_dir_all(t.path("bin-ok")).unwrap();
        std::fs::create_dir_all(t.path("bin-fail")).unwrap();
        std::fs::write(
            t.path("bin-ok/cargo"),
            // `EDIT_DURING_BUILD`: a file the "build" changes (the sources
            // moving under a build).
            "#!/bin/sh\n[ -n \"$EDIT_DURING_BUILD\" ] && echo edit >> \"$EDIT_DURING_BUILD\"\nmkdir -p \"$CARGO_TARGET_DIR/release-fast\" && printf built > \"$CARGO_TARGET_DIR/release-fast/alsem\"\n",
        )
        .unwrap();
        std::fs::write(t.path("bin-fail/cargo"), "#!/bin/sh\nexit 1\n").unwrap();
        for c in ["bin-ok/cargo", "bin-fail/cargo"] {
            std::fs::set_permissions(t.path(c), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(t.path("base-alsem"), b"placeholder").unwrap();
        t
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }

    /// Through `crate::git::run` (scratch git: no hook variable, no system or
    /// user config, a fixed identity), as every test-made repo must (review
    /// r1, Opus; `tests/common/git.rs`).
    fn git(&self, repo: &str, args: &[&str]) -> String {
        let out = crate::git::run(&self.path(repo), args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn commit(&self, repo: &str) {
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "-q", "-m", "c"]);
    }

    /// The cache key a kept binary of the repo's HEAD gets: tree + grammar.
    fn key(&self) -> String {
        let tree = self.git("repo", &["rev-parse", "HEAD^{tree}"]);
        let grammar = self.git("grammar", &["rev-parse", "HEAD"]);
        format!("{tree}-g{}", &grammar[..12])
    }

    /// `cdo-speed <args>` in the throwaway repo with `cache`, the stub cargo
    /// (`ok` or failing), and `extra` env: (exit code, output).
    fn speed(
        &self,
        args: &[&str],
        cache: &std::path::Path,
        ok: bool,
        extra: &[(&str, &str)],
    ) -> (Option<i32>, String) {
        let stub = self.path(if ok { "bin-ok" } else { "bin-fail" });
        let mut cmd = std::process::Command::new("python3");
        // A clean environment: an inherited build override (RUSTFLAGS, a
        // profile override, CFLAGS) would make every key `None` (review r2, sol).
        // Stricter than `crate::clean_env` (its own HOME, no toolchain homes).
        cmd.arg(self.path("repo/scripts/cdo-speed"))
            .args(args)
            // From the throwaway folder: a relative `TREE_SITTER_AL_PATH` of
            // `grammar` then names a real clone for git (not for the build).
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("ALSEM_SPEED_CACHE", cache)
            .env("CARGO_TARGET_DIR", self.path("target"))
            .env("TREE_SITTER_AL_PATH", self.path("grammar"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    stub.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            );
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let run = cmd.output().expect("spawn python3 cdo-speed");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        (run.status.code(), text)
    }

    /// The working tree's build (`--build-only`); whether it kept a binary
    /// under the current key in `cache`.
    fn build_keeps(&self, cache: &std::path::Path, extra: &[(&str, &str)]) -> bool {
        let ws = self.path("ws");
        let base = self.path("base-alsem");
        let out = self.path("out");
        let (code, text) = self.speed(
            &[
                ws.to_str().unwrap(),
                "--build-only",
                "--base-exe",
                base.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
            cache,
            true,
            extra,
        );
        assert_eq!(code, Some(0), "{text}");
        // The script's own word (the key's format is pinned once, in the clean
        // case of `speed_build_keeps_a_binary_only_for_a_clean_tree_and_grammar`).
        text.contains("kept the working tree's binary")
    }
}

/// TLS-34: the working tree's build keeps its binary under (tree, grammar)
/// only when nothing else went into it: a clean work tree, an unmodified
/// grammar clone, no Rust flags. Each case in a throwaway checkout.
///
/// Mutations (2026-11-13): `working_tree_key` without its status check -> the
/// tracked change keeps a binary; the grammar's status with
/// `--untracked-files=no` -> the untracked `scanner.c` keeps one; `build_key`
/// without its absolute-path check -> the relative grammar path keeps one
/// (the run's folder is the throwaway's, so git finds that clone);
/// `CFLAGS` out of `BUILD_ENV_EXACT` -> the C-flags case keeps one; the key
/// after the build not compared (`key = key_before`) -> the edited-during-build
/// case keeps one; `record_tree_binary` not catching `OSError` -> the unwritable
/// cache exits 1 (a speed failure) and `build_keeps`' exit assert fails.
#[cfg(target_os = "linux")]
#[test]
fn speed_build_keeps_a_binary_only_for_a_clean_tree_and_grammar() {
    let t = Throwaway::new();
    let cache = |name: &str| t.path(&format!("cache-{name}"));
    assert!(
        t.build_keeps(&cache("clean"), &[]),
        "precondition: a clean tree keeps"
    );
    assert_eq!(
        std::fs::read(cache("clean").join("trees").join(t.key()).join("alsem")).unwrap(),
        b"built"
    );
    // A tracked change.
    std::fs::write(t.path("repo/a.txt"), "two\n").unwrap();
    assert!(
        !t.build_keeps(&cache("tracked"), &[]),
        "a tracked change kept a binary"
    );
    t.git("repo", &["checkout", "-q", "--", "a.txt"]);
    // An untracked file outside .gitignore.
    std::fs::write(t.path("repo/new.txt"), "x\n").unwrap();
    assert!(
        !t.build_keeps(&cache("untracked"), &[]),
        "an untracked file kept a binary"
    );
    std::fs::remove_file(t.path("repo/new.txt")).unwrap();
    // A modified grammar clone.
    std::fs::write(t.path("grammar/a.txt"), "two\n").unwrap();
    assert!(
        !t.build_keeps(&cache("grammar"), &[]),
        "a modified grammar kept a binary"
    );
    t.git("grammar", &["checkout", "-q", "--", "a.txt"]);
    // Rust flags or a profile override.
    assert!(!t.build_keeps(&cache("flags"), &[("RUSTFLAGS", "-Ctarget-cpu=native")]));
    assert!(!t.build_keeps(
        &cache("profile"),
        &[("CARGO_PROFILE_RELEASE_FAST_LTO", "off")]
    ));
    // C flags (the grammar's parser is C), a target's Rust flags (review r2, sol).
    assert!(!t.build_keeps(&cache("cflags"), &[("CFLAGS", "-O0")]));
    // cc-rs's host spelling (review r3, sol); `HOSTNAME` alone is no override.
    assert!(!t.build_keeps(&cache("host-cflags"), &[("HOST_CFLAGS", "-O0")]));
    assert!(t.build_keeps(&cache("hostname"), &[("HOSTNAME", "box")]));
    assert!(!t.build_keeps(
        &cache("target-flags"),
        &[(
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
            "-Copt-level=0"
        )]
    ));
    // An untracked file in the grammar clone (an added scanner.c is compiled).
    std::fs::write(t.path("grammar/scanner.c"), "int x;\n").unwrap();
    assert!(
        !t.build_keeps(&cache("grammar-untracked"), &[]),
        "an untracked grammar file kept a binary"
    );
    std::fs::remove_file(t.path("grammar/scanner.c")).unwrap();
    // A relative grammar path (git and the build would resolve it differently).
    assert!(
        !t.build_keeps(&cache("relative"), &[("TREE_SITTER_AL_PATH", "grammar")]),
        "a relative grammar path kept a binary"
    );
    // Another toolchain than the tree's pin (review r1, Opus).
    assert!(!t.build_keeps(&cache("toolchain"), &[("RUSTUP_TOOLCHAIN", "nightly")]));
    // The sources changed during the build: the key after is not the key before.
    let a = t.path("repo/a.txt");
    assert!(
        !t.build_keeps(
            &cache("during"),
            &[("EDIT_DURING_BUILD", a.to_str().unwrap())]
        ),
        "a build whose sources changed under it kept a binary"
    );
    t.git("repo", &["checkout", "-q", "--", "a.txt"]);
    // A cache that cannot be written only skips the keeping: the speed check
    // still exits 0 (`build_keeps` asserts it; review r1, Opus).
    std::fs::write(t.path("not-a-folder"), "x").unwrap();
    assert!(!t.build_keeps(&t.path("not-a-folder"), &[]));
    // Clean again: keeps.
    assert!(
        t.build_keeps(&cache("again"), &[]),
        "a clean tree keeps again"
    );
}

/// TLS-34: `cdo-speed --warm REV` seeds REV's entry from a binary kept for
/// REV's tree and grammar and builds nothing (the stub cargo fails); with a
/// new grammar commit the kept binary does not match, so it builds (and the
/// stub fails).
///
/// Mutations (2026-11-13): `cached_binary` without its seeding step -> the
/// first warm builds, the stub fails, the exit-code assert fails; the key
/// without its grammar part (`f"{tree}"`) -> the moved grammar is seeded and
/// the `assert_ne` fails.
#[cfg(target_os = "linux")]
#[test]
fn speed_warm_seeds_a_commit_from_its_trees_gate_binary() {
    let t = Throwaway::new();
    let cache = t.path("cache");
    assert!(
        t.build_keeps(&cache, &[]),
        "precondition: the gate kept its binary"
    );
    let head = t.git("repo", &["rev-parse", "HEAD"]);
    let (code, text) = t.speed(&["--warm", &head], &cache, false, &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("seeded from a gate's build"), "{text}");
    assert_eq!(
        std::fs::read(cache.join("bin").join(&head).join("alsem")).unwrap(),
        b"built"
    );
    // The grammar moved: the kept binary is not this build's.
    let t2 = Throwaway::new();
    let cache2 = t2.path("cache");
    assert!(t2.build_keeps(&cache2, &[]));
    std::fs::write(t2.path("grammar/a.txt"), "next\n").unwrap();
    t2.commit("grammar");
    let head2 = t2.git("repo", &["rev-parse", "HEAD"]);
    let (code, text) = t2.speed(&["--warm", &head2], &cache2, false, &[]);
    assert_ne!(code, Some(0), "{text}");
    assert!(text.contains("building alsem (once per commit)"), "{text}");
}

/// TLS-36: beside the impact phase, lane 2 is pinned by core type, read from a
/// fake host: the CPU mask (`CDO_GATE_PROC_STATUS` for `/proc/self/status`) and
/// sysfs (`CDO_GATE_SYS_CPU`: `online` and each CPU's `cpuinfo_max_freq`).
/// - The host is a Core Ultra 9 185H as server1: CPUs 0-11 performance
///   (4.8-5.1 GHz), 12-19 efficiency (3.8), 20-21 low-power (2.5).
/// - The default takes every efficiency core (12-19); a count the last N
///   non-low-power CPUs (`08` is eight); a list as given; 0 none.
/// - Offline CPUs are left out. A CPU with no efficiency cores falls back to
///   the last 6, and there is no pin when the impact phase would keep fewer
///   than 8.
///
/// Mutations (2026-11-13): the low-power cut dropped (`mhz < 0.7 * top` never
/// true) -> the default takes 12-21 (the low-power cores too) and fails; the
/// efficiency band's upper edge dropped (every core counts as efficiency) ->
/// the impact phase would keep fewer than 8, no pin, and the default fails.
#[cfg(target_os = "linux")]
#[test]
fn cdo_gate_pins_lane_two_by_core_type() {
    // Without taskset the gate pins nothing (review r2, Opus): nothing to test.
    let has_taskset = crate::clean_env::command("sh")
        .args(["-c", "command -v taskset"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_taskset {
        eprintln!("cdo_gate_pins_lane_two_by_core_type: skipped, no taskset");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("meminfo"), "MemAvailable: 62914560 kB\n").unwrap();
    std::fs::write(dir.path().join("memory.max"), "max\n").unwrap();
    // A fake host: (CPU, max kHz) for each CPU, online list, CPU mask.
    let host = |name: &str, cpus: &[(u32, u32)], online: &str, mask: &str| {
        let sys = dir.path().join(name);
        for &(c, khz) in cpus {
            let f = sys.join(format!("cpu{c}/cpufreq"));
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("cpuinfo_max_freq"), format!("{khz}\n")).unwrap();
        }
        std::fs::write(sys.join("online"), format!("{online}\n")).unwrap();
        std::fs::write(
            sys.join("status"),
            format!("Name:\tbash\nCpus_allowed_list:\t{mask}\n"),
        )
        .unwrap();
        sys
    };
    let layout = |sys: &std::path::Path, want: Option<&str>| {
        let mut cmd = crate::clean_env::command("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", dir.path().join("meminfo"))
            .env("CDO_GATE_CGROUP", dir.path())
            .env("CDO_GATE_LAYOUT", "parallel")
            .env("CDO_GATE_PROC_STATUS", sys.join("status"))
            .env("CDO_GATE_SYS_CPU", sys);
        if let Some(w) = want {
            cmd.env("CDO_GATE_LANE2_CPUS", w);
        }
        let out = cmd.output().expect("spawn bash scripts/cdo-gate --layout");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let check = |sys: &std::path::Path, want: Option<&str>, cpus: &str| {
        let got = layout(sys, want);
        assert!(
            got.contains(&format!("lane 2 on CPUs {cpus} ")),
            "{want:?}: want CPUs {cpus}, got {got}"
        );
    };
    let ultra: Vec<(u32, u32)> = (0..22)
        .map(|c| {
            let khz = match c {
                0 | 5..=11 => 4_800_000,
                1..=4 => 5_100_000,
                12..=19 => 3_800_000,
                _ => 2_500_000,
            };
            (c, khz)
        })
        .collect();
    let server1 = host("server1", &ultra, "0-21", "0-21");
    check(&server1, None, "12,13,14,15,16,17,18,19");
    check(&server1, Some("6"), "14,15,16,17,18,19");
    check(&server1, Some("08"), "12,13,14,15,16,17,18,19");
    check(&server1, Some("2-5"), "2-5");
    check(&server1, Some("0"), "any");
    // CPUs 18-21 offline: the efficiency cores left are 12-17.
    let offline = host("offline", &ultra, "0-17", "0-21");
    check(&offline, None, "12,13,14,15,16,17");
    // No efficiency cores: the last 6; too few CPUs: no pin.
    let even: Vec<(u32, u32)> = (0..22).map(|c| (c, 4_800_000)).collect();
    check(
        &host("even", &even, "0-21", "0-21"),
        None,
        "16,17,18,19,20,21",
    );
    let small: Vec<(u32, u32)> = (0..10).map(|c| (c, 4_800_000)).collect();
    check(&host("small", &small, "0-9", "0-9"), None, "any");
}

/// TLS-36: every lane 2 command runs with the pin (`"${lane2_pin[@]}"`): the
/// harness, lsp and l4 phases and both of lib_cdo's runs. The pin decided in
/// `plan_layout` would otherwise not reach a phase that forgot it.
///
/// Mutation (2026-11-13): `"${lane2_pin[@]}"` dropped from `phase_lsp` ->
/// named here.
#[test]
fn cdo_gate_runs_every_lane_two_command_pinned() {
    let script = include_str!("../../scripts/cdo-gate");
    let body = |head: &str| {
        let start = script
            .find(head)
            .unwrap_or_else(|| panic!("scripts/cdo-gate has no `{head}`"));
        let rest = &script[start..];
        &rest[..rest.find("\n}").expect("a function's end")]
    };
    for (f, runs) in [
        ("phase_harness() {", 1),
        ("phase_lsp() {", 1),
        ("phase_l4() {", 1),
        ("lib_tests() {", 2),
    ] {
        let b = body(f);
        // A `--list` (lib_tests' listing of the phase's tests) runs no test.
        assert_eq!(
            b.lines()
                .filter(|l| l.contains("cargo test") && !l.contains("--list"))
                .count(),
            runs,
            "`{f}`: the cargo runs this test knows"
        );
        assert_eq!(
            b.matches("\"${lane2_pin[@]}\"").count(),
            runs,
            "`{f}`: every cargo run must carry \"${{lane2_pin[@]}}\""
        );
    }
}

/// TLS-36: lib_cdo's long-test run must account for every long test by name.
/// A long test renamed to a longer name (`..._v2`) would match no `--exact`
/// name and still be skipped by the second run's substring `--skip`, so it
/// would run nowhere (review r1, sol and Opus). `lib_tests` from the script,
/// with a stub `cargo` that reports how many tests the long run found.
///
/// Mutations (2026-11-13): `lib_tests` without its count check -> the run that
/// finds 4 of 5 passes, and the assert fails; without its listing check (the
/// `orphans`) -> the sibling `cdo_crossing_census_v2` passes unrun, and the
/// assert fails; a failed listing not returning -> it reads as "no orphans",
/// the phase passes, and the assert fails.
#[cfg(unix)]
#[test]
fn lib_cdo_fails_when_a_long_test_is_not_found_by_name() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cargo"),
        // `--list`: one plain test, and (`SIBLING`) a test whose name holds a
        // long test's; else a run's result line.
        "#!/bin/sh\nif echo \"$*\" | grep -q -- --list; then\n\
         [ -n \"$LIST_FAILS\" ] && { echo 'error: could not compile'; exit 101; }\n\
         echo 'snapshot::snapshot::tests::builds_snapshot_over_cdo_workspace: test'\n\
         [ -n \"$SIBLING\" ] && echo 'engine::l5::digest::wave2_index_tests::cdo_crossing_census_v2: test'\n\
         exit 0\nfi\n\
         if echo \"$*\" | grep -q -- --exact; then n=${LONG_FOUND:-5}; else n=12; fi\n\
         echo \"test result: ok. $n passed; 0 failed; 0 ignored; 0 measured; 3300 filtered out; finished in 1.00s\"\n",
    )
    .unwrap();
    std::fs::set_permissions(
        dir.path().join("cargo"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    // `extra`: "", "SIBLING" (the listing names a sibling) or "LIST_FAILS".
    let run = |found: &str, extra: &str| {
        let out = crate::clean_env::command("bash")
            .arg("-c")
            .arg(concat!(
                "logs=\"$1\"; profile=(); lane2_nice=(); lane2_pin=(); lib_threads=2; ",
                "source <(sed -n '/^lib_long_tests=(/,/^)/p; /^lib_tests()/,/^}/p' \"$2\"); ",
                "lib_tests"
            ))
            .arg("bash")
            .arg(dir.path())
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .env("HOME", dir.path())
            .env("LONG_FOUND", found)
            .env("SIBLING", if extra == "SIBLING" { "1" } else { "" })
            .env("LIST_FAILS", if extra == "LIST_FAILS" { "1" } else { "" })
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .expect("spawn bash");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (code, text) = run("5", "");
    assert_eq!(code, Some(0), "precondition, all five found: {text}");
    let (code, text) = run("4", "");
    assert_ne!(code, Some(0), "4 of 5 long tests found must fail: {text}");
    assert!(text.contains("found 4 of 5"), "{text}");
    // A sibling added beside a long test (review r2, sol): the long run still
    // finds 5, but the second run's substring --skip would drop the sibling.
    let (code, text) = run("5", "SIBLING");
    assert_ne!(code, Some(0), "a test in neither run must fail: {text}");
    assert!(text.contains("cdo_crossing_census_v2"), "{text}");
    // A listing that fails says nothing about siblings (review r3, sol).
    let (code, text) = run("5", "LIST_FAILS");
    assert_ne!(code, Some(0), "a failed listing must fail: {text}");
    assert!(text.contains("could not list"), "{text}");
}

/// TLS-36: serial is the default, even with every byte of memory free; the
/// parallel layout only when `CDO_GATE_LAYOUT=parallel` asks. Measured on
/// server1 on one tree back to back: serial 875 s, parallel 1,131 s.
///
/// Mutation (2026-11-13): `plan_layout` without its `CDO_GATE_LAYOUT` check
/// -> plenty of memory gives "parallel:" and the assert fails.
#[cfg(unix)]
#[test]
fn cdo_gate_is_serial_unless_parallel_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("meminfo"), "MemAvailable: 94371840 kB\n").unwrap();
    std::fs::write(dir.path().join("memory.max"), "max\n").unwrap();
    let layout = |asked: Option<&str>| {
        let mut cmd = crate::clean_env::command("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", dir.path().join("meminfo"))
            .env("CDO_GATE_CGROUP", dir.path());
        if let Some(a) = asked {
            cmd.env("CDO_GATE_LAYOUT", a);
        }
        let out = cmd.output().expect("spawn bash scripts/cdo-gate --layout");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let got = layout(None);
    assert!(
        got.starts_with("sequential:") && got.contains("the default"),
        "{got}"
    );
    let got = layout(Some("parallel"));
    assert!(got.starts_with("parallel:"), "{got}");
    let got = layout(Some("Parallel")); // only the exact word asks
    assert!(got.starts_with("sequential:"), "{got}");
}

/// TLS-71: `ci-steps all` runs the agentflow suite (pytest, from `scripts/`)
/// and fails when it fails. No gate ran it before, and two tests sat red on
/// master. Run in a temp repo with `cargo`, `python3` and the helper scripts
/// stubbed, so only the step wiring is under test.
///
/// Mutation (2026-11-15): `agentflow` taken out of `all`'s step loop -> the red
/// suite gives exit 0 and the test fails.
#[cfg(unix)]
#[test]
fn ci_steps_all_runs_and_judges_the_agentflow_suite() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (repo, bin, grammar) = (
        dir.path().join("repo"),
        dir.path().join("bin"),
        dir.path().join("grammar"),
    );
    let exe = |p: std::path::PathBuf, body: &str| {
        std::fs::write(&p, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    std::fs::create_dir_all(repo.join("scripts")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(grammar.join("src")).unwrap();
    std::fs::write(grammar.join("src/node-types.json"), "[]").unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ci-steps"),
        repo.join("scripts/ci-steps"),
    )
    .unwrap();
    // ENG-110: `build` runs check-ir-image on the binary; the stub build makes none.
    for s in [
        "tree-stamp",
        "require-rttime",
        "nextest-priority-check",
        "check-ir-image",
    ] {
        exe(repo.join("scripts").join(s), "echo stub");
    }
    // TLS-108: `perf-bounds` asks `cargo test --no-run` for its test binary (the
    // JSON artifact line); the stub names `/bin/true`.
    // TLS-113: the stand-in lists the one timing test the temp repo's
    // tests/perf_bounds.rs declares, as `perf-bounds` now checks before it runs.
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("tests/perf_bounds.rs"),
        "mod release_checks {\n    #[test]\n    fn t() {}\n}\n",
    )
    .unwrap();
    let perf = bin.join("perf-bounds-stand-in");
    exe(
        perf.clone(),
        r#"[ "$1" = --list ] && [ "$4" != --ignored ] && echo "release_checks::t: test"; exit 0"#,
    );
    exe(
        bin.join("cargo"),
        &format!(
            r#"case " $* " in *" --no-run "*) echo '{{"profile":{{"test":true}},"target":{{"name":"perf_bounds"}},"executable":"{}"}}' ;; esac; exit 0"#,
            perf.display()
        ),
    );
    exe(
        bin.join("python3"),
        r#"[ "$3" = --version ] && exit 0; echo "$PWD $*" >> "$PY_LOG"; exit "${PY_RC:-0}""#,
    );
    crate::git::run(&repo, &["init", "-q"]);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let log = dir.path().join("py.log");
    let run = |rc: &str| {
        let _ = std::fs::remove_file(&log);
        let out = crate::clean_env::command("bash")
            .arg(repo.join("scripts/ci-steps"))
            .arg("all")
            .current_dir(&repo)
            .env("PATH", &path)
            .env("HOME", dir.path())
            .env("TREE_SITTER_AL_PATH", &grammar)
            .env("CARGO_TARGET_DIR", dir.path().join("target"))
            .env("PY_LOG", &log)
            .env("PY_RC", rc)
            // TLS-108: the temp repo has no cdo-slot; a slot folder that cannot
            // be made takes perf-bounds' no-slot path (said in its line).
            .env("CDO_SLOT_DIR", "/proc/no-slot-here")
            .output()
            .expect("spawn bash scripts/ci-steps all");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    let (code, text) = run("0");
    assert_eq!(code, Some(0), "{text}");
    let called = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        called
            .trim_end()
            .ends_with("/scripts -m pytest -q -p no:cacheprovider agentflow/tests"),
        "{called}"
    );
    let (code, text) = run("1");
    assert_eq!(
        code,
        Some(1),
        "a red agentflow suite must fail `all`: {text}"
    );
    assert!(text.contains("FAILED at 'agentflow'"), "{text}");
}

/// A warm build folder written by hand: one dependency, plus the two folders
/// `scripts/seed-target` leaves out.
fn warm_target(src: &tempfile::TempDir) {
    std::fs::create_dir_all(src.path().join("debug/deps")).unwrap();
    std::fs::write(src.path().join("debug/deps/a.rlib"), b"x").unwrap();
    std::fs::create_dir_all(src.path().join("flycheck0")).unwrap();
    std::fs::create_dir_all(src.path().join("tmp")).unwrap();
}

fn seed_target(wt: &std::path::Path, src: &std::path::Path) -> std::process::Output {
    crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/seed-target"))
        .arg(wt)
        .arg("--from")
        .arg(src)
        .output()
        .expect("spawn bash scripts/seed-target")
}

/// TLS-08: `scripts/seed-target` copies a warm build folder into a new
/// worktree, minus `flycheck0` and `tmp`, or leaves nothing behind (a half
/// copy would make cargo trust stale units). Two cases:
/// - source and worktree in the same temp folder: where it has reflinks the
///   copy holds the dependency and not the skipped folders, and a second run
///   refuses the existing `target/`; where it has none, as below.
/// - source on `/dev/shm` (its own tmpfs), so a reflink copy always fails:
///   exit 1 and no `target/`.
///
/// Mutation (2026-11-11): drop the `rm -rf "$dst"` on a failed copy -> the
/// `/dev/shm` case leaves an empty `target/` and fails. (The first version of
/// this test had only the same-folder case; that break passed it, because
/// this machine's `/tmp` has reflinks.)
#[cfg(unix)]
#[test]
fn seed_target_copies_all_or_nothing() {
    let src = tempfile::tempdir().unwrap();
    warm_target(&src);
    let wt = tempfile::tempdir().unwrap();
    let dst = wt.path().join("target");
    let out = seed_target(wt.path(), src.path());
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    match out.status.code() {
        Some(0) => {
            assert!(dst.join("debug/deps/a.rlib").is_file(), "{text}");
            assert!(!dst.join("flycheck0").exists(), "{text}");
            assert!(!dst.join("tmp").exists(), "{text}");
            let again = seed_target(wt.path(), src.path());
            assert_eq!(again.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&again.stderr).contains("exists"));
        }
        Some(1) => assert!(!dst.exists(), "a failed seed left {dst:?}: {text}"),
        other => panic!("exit {other:?}: {text}"),
    }

    let shm = std::path::Path::new("/dev/shm");
    if !shm.is_dir() {
        return;
    }
    let src = tempfile::tempdir_in(shm).unwrap();
    warm_target(&src);
    let wt = tempfile::tempdir().unwrap();
    let dst = wt.path().join("target");
    let out = seed_target(wt.path(), src.path());
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(!dst.exists(), "a failed seed left {dst:?}: {text}");
}

/// `cdo-gate --product-paths` (the rule alone): the changed paths that can
/// reach the `alsem` binary.
fn product_paths(paths: &[&str]) -> Vec<String> {
    use std::io::Write;
    let mut child = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg("--product-paths")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn bash scripts/cdo-gate");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{}\n", paths.join("\n")).as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// TLS-20: `cdo-gate` skips its speed phase only when the diff touches no
/// product code, by an allow-list of paths never compiled into `alsem`.
/// Both ways: tests, docs, scripts, the nextest config and the CHANGELOG
/// select none (the phase is skipped); `src/`, `crates/`, `Cargo.*`,
/// `scripts/cdo-speed` itself and an unknown top-level file each select
/// themselves (the phase runs). And a run that skipped it says so in its
/// summary, never as a plain PASS.
///
/// Mutation: `src/*` added to the allow-list -> the `src/` path is not
/// selected and the test fails.
#[cfg(unix)]
#[test]
fn cdo_gate_skips_speed_only_when_no_product_code_changed() {
    let never = [
        "tests/cli/x.rs",
        "tests/r4-goldens/a.json",
        "docs/superpowers/notes/n.md",
        "scripts/ci-steps",
        ".config/nextest.toml",
        "CHANGELOG.md",
    ];
    assert!(
        product_paths(&never).is_empty(),
        "{:?}",
        product_paths(&never)
    );
    for p in [
        "src/engine/l5/digest.rs",
        "crates/al-syntax/src/lower/mod.rs",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        ".cargo/config.toml",
        "scripts/cdo-speed",
        "README.md",
    ] {
        let mut mixed: Vec<&str> = never.to_vec();
        mixed.push(p);
        assert_eq!(product_paths(&mixed), vec![p.to_string()], "{p}");
    }
    // A run folder that skipped the phase (written by hand, as the gate does).
    let run = tempfile::tempdir().unwrap();
    std::fs::write(run.path().join("split-impact"), "").unwrap();
    std::fs::write(
        run.path().join("speed-skipped"),
        "diff touches no product code\n",
    )
    .unwrap();
    for p in [
        "program_resolve_harness",
        "lsp",
        "l4_summary_differential",
        "lib_cdo",
        "output_cdo",
        "output_cdo_impact",
    ] {
        std::fs::write(run.path().join(format!("{p}.status")), "0 1\n").unwrap();
        std::fs::write(
            run.path().join(format!("{p}.log")),
            "    1 test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
        )
        .unwrap();
    }
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg(format!("--summary={}", run.path().display()))
        .output()
        .expect("spawn bash scripts/cdo-gate");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("speed: skipped, diff touches no product code"),
        "{text}"
    );
    assert!(
        text.contains("PASS (behaviour only: speed skipped, diff touches no product code)"),
        "{text}"
    );
    assert!(
        !text.contains("cdo-gate: PASS\n"),
        "never a plain PASS: {text}"
    );
}

/// A stub `alsem` MCP server for cdo-speed: it answers every call at once,
/// except its first (`build`, the model build), which burns `STUB_BUILD_S`
/// seconds of CPU, 50 % more when it is the second server of its pair to
/// build (a shared counter in `STUB_DIR`): the "second side builds slower
/// under load" bias of TLS-65, made deterministic. ⟨TLS-111⟩ It publishes user
/// cycles beside instructions (3e9 per CPU second burned, at a steady clock);
/// STUB_SLOW_HEAD, STUB_CPI_HEAD, STUB_SYS_HEAD, STUB_SECOND_FACTOR,
/// STUB_BUILD_HEAD_BY_PASS, STUB_TESTS_HEAD_BY_PASS and
/// STUB_TESTS_HEAD_SLEEP_BY_PASS shape head's cost, STUB_SLEEP_S both sides'
/// (see the code).
#[cfg(target_os = "linux")]
const SPEED_STUB: &str = r#"#!/usr/bin/env python3
import fcntl, json, os, sys, time
# ENG-110 r2: STUB_IR_DEAD: a server started for cdo-speed's IR rows (the cache on) reads
# `initialize`, closes its stdin and answers it, so cdo-speed's next write hits a closed pipe
# (BrokenPipeError, an OSError that is neither CallFailed nor SystemExit).
if os.environ.get("STUB_IR_DEAD") and os.environ.get("ALSEM_IR_CACHE") == "on":
    first = json.loads(sys.stdin.readline())
    os.close(0)
    print(json.dumps({"jsonrpc": "2.0", "id": first["id"], "result": {}}), flush=True)
    time.sleep(5)
    sys.exit(0)
calls = 0
def burn(seconds):
    # User time (TLS-111: the clock is cycles per USER second): a pure loop
    # between reads, since reading the CPU clock is a system call.
    end = os.times().user + seconds
    while os.times().user < end:
        for _ in range(20000):
            pass
def made_before(name):
    # How many servers made this call before this one (n % 2 == 1: this server
    # is the second of its pair; n // 2: the warm-up pass, for warm-tests).
    with open(os.path.join(os.environ["STUB_DIR"], name), "a+") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.seek(0)
        n = len(f.read())
        f.write("x")
    return n
head = os.path.basename(sys.argv[0]) == "alsem-head"
# TLS-111: STUB_SLOW_HEAD: head's clock is lower (more CPU, the same cycles);
# STUB_CPI_HEAD: head needs more cycles (more CPU and cycles, the same clock).
slow = float(os.environ.get("STUB_SLOW_HEAD", "1")) if head else 1.0
# STUB_SYS_HEAD's read buffer, 4 MiB touched in every server (equal peak RSS).
ballast = bytearray(b"\x01") * (1 << 22)
cpi = float(os.environ.get("STUB_CPI_HEAD", "1")) if head else 1.0
live = os.path.join(os.environ["STUB_DIR"], "live")
os.makedirs(live, exist_ok=True)
me = os.path.join(live, str(os.getpid()))
def another_alive():
    # TLS-79: another server holds a built model (it registers itself once its
    # own build is done, review r1, sol: a started server holds none yet).
    # A marker whose process is gone (killed) does not count (review r1, Fable).
    return any(p != str(os.getpid()) and os.path.exists(f"/proc/{p}") for p in os.listdir(live))
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    if msg.get("method") == "tools/call":
        calls += 1
        # The warm-up's two judged calls: the model build (1st), 50 % slower
        # while another server is alive (the real cause, TLS-79), and the test
        # universe (6th, `warm-tests`), 50 % slower when it is the second of
        # its pair (a positional factor the flipped passes cancel, TLS-65).
        # TLS-111: the call's CPU seconds at a steady clock; its user cycles
        # follow them at 3 GHz (1e9 for a call that burns nothing).
        burned = 0.0
        # Wall-only seconds this call declares (`stubSleptS`): cdo-speed's stub
        # clock adds them to the call's CPU, so wall is exact under any host load.
        slept = 0.0
        work = 1.0  # head's extra work this call (instructions and cycles both)
        zero_instr = False
        if calls == 1:
            # TLS-111: STUB_BUILD_HEAD_BY_PASS=f1,f2,...: head's build does f<pass>
            # times the work in that warm-up pass (3 builds a pass: two timed,
            # one again, untimed).
            n = made_before("build")
            by_pass = os.environ.get("STUB_BUILD_HEAD_BY_PASS")
            if head and by_pass:
                work = float(by_pass.split(",")[n // 3])
            # STUB_INSTR_ZERO_BUILD_PASS=p: head's build in warm-up pass p
            # (0-based) retires no instruction (that one call goes uncounted).
            zero_instr = head and str(n // 3) == os.environ.get("STUB_INSTR_ZERO_BUILD_PASS")
            # TLS-111 review r5: STUB_BUILD_HEAD_PASS=slow:cpi:sys,... sets head's
            # build factors (as STUB_SLOW_HEAD, STUB_CPI_HEAD and STUB_SYS_HEAD) per
            # warm-up pass.
            b_slow, b_cpi = slow, cpi
            b_sys = float(os.environ.get("STUB_SYS_HEAD", "0"))
            spec = os.environ.get("STUB_BUILD_HEAD_PASS")
            if head and spec:
                # An extra pair (passes 3 and 4) repeats passes 1 and 2.
                passes = spec.split(",")
                b_slow, b_cpi, b_sys = map(float, passes[(n // 3) % len(passes)].split(":"))
            burned = (float(os.environ["STUB_BUILD_S"]) * (1.5 if another_alive() else 1.0)
                      * work * b_cpi)
            burn(burned * b_slow)
            # STUB_HEAD_SLEEP_S (TLS-103): head's build waits this long, wall time
            # with no CPU, as a loaded host stretches it.
            if head:
                time.sleep(float(os.environ.get("STUB_HEAD_SLEEP_S", "0")))
                # TLS-111: STUB_SYS_HEAD=x: head's build also spends x times its
                # CPU seconds in the kernel (reading /dev/zero: no user time).
                sys_s = burned * b_sys
                if sys_s > 0:
                    # Into `ballast`, which every server holds: a buffer of
                    # head's own raised its peak RSS (memory x1.35), and 64 KiB
                    # reads spent ~15 % in user time (a false CLOCK).
                    fd = os.open("/dev/zero", os.O_RDONLY)
                    end = os.times().system + sys_s
                    while os.times().system < end:
                        os.readv(fd, [ballast])
                    os.close(fd)
            open(me, "w").close()
        if calls == 6:
            n = made_before("tests")
            # STUB_SECOND_FACTOR: the positional factor (default 1.5).
            factor = float(os.environ.get("STUB_SECOND_FACTOR", "1.5")) if n % 2 == 1 else 1.0
            # TLS-111: STUB_TESTS_HEAD_BY_PASS=f1,f2,...: head's warm-tests does
            # f<pass> times the work (CPU, cycles AND instructions: a real
            # slowdown, so no CLOCK/CPI label makes it advisory) in that pass.
            by_pass = os.environ.get("STUB_TESTS_HEAD_BY_PASS")
            if head and by_pass:
                work = float(by_pass.split(",")[n // 2])
            burned = float(os.environ["STUB_BUILD_S"]) * factor * work * cpi
            burn(burned * slow)
            # STUB_TESTS_HEAD_SLEEP_BY_PASS=s1,s2,...: head's warm-tests then
            # waits s<pass> seconds more (wall time only).
            sleeps = os.environ.get("STUB_TESTS_HEAD_SLEEP_BY_PASS")
            if head and sleeps:
                slept += float(sleeps.split(",")[n // 2])
        # TLS-111 review r3: STUB_ROUND_BURN_S=s: every round call (the 8th call
        # on: both servers of the last pass have made 7) burns s seconds, so the
        # round total (6 calls) is judged; STUB_ROUND_HEAD=slow:cpi,... gives
        # head's factors per round (as STUB_SLOW_HEAD and STUB_CPI_HEAD).
        if calls >= 8 and os.environ.get("STUB_ROUND_BURN_S"):
            r_slow = r_cpi = 1.0
            spec = os.environ.get("STUB_ROUND_HEAD")
            if head and spec:
                r_slow, r_cpi = map(float, spec.split(",")[(calls - 8) // 6].split(":"))
            burned = float(os.environ["STUB_ROUND_BURN_S"]) * r_cpi * cpi
            burn(burned * r_slow * slow)
        # STUB_UNCOVER_CALL=N: the stub perf's counters ran for half of head's
        # N-th call (per server): that call's count does not cover it.
        uncover = head and str(calls) == os.environ.get("STUB_UNCOVER_CALL")
        with open(os.path.join(os.environ["STUB_DIR"], f"cover.{os.getpid()}"), "w") as f:
            f.write("0.5" if uncover else "1")
        # TLS-130 review r3: STUB_PARTIAL_CALL=N: the stub perf prints head's
        # N-th call (per server) with its running % summed to 90 (counts whole).
        partial = head and str(calls) == os.environ.get("STUB_PARTIAL_CALL")
        with open(os.path.join(os.environ["STUB_DIR"], f"partial.{os.getpid()}"), "w") as f:
            f.write("1" if partial else "")
        # STUB_SLEEP_S: both sides' judged calls wait this long (wall time only),
        # so a wall ratio stays steady when load stretches the short CPU burn.
        if calls in (1, 6):
            slept += float(os.environ.get("STUB_SLEEP_S", "0"))
        cyc = os.path.join(os.environ["STUB_DIR"], f"cycles.{os.getpid()}")
        done = int(open(cyc).read()) if os.path.exists(cyc) else 0
        with open(cyc, "w") as f:
            f.write(str(done + int(burned * 3e9 if burned else 1e9)))
        # TLS-83: this process's user-mode instructions so far, for the stub perf
        # (SPEED_PERF_STUB): 1e9 a call, times STUB_INSTR_HEAD on the head side.
        factor = 1.0
        if head:
            factor = float(os.environ.get("STUB_INSTR_HEAD", "1"))
            # STUB_INSTR_ZERO_CALL=N[,M...]: head's N-th call (per server) retires none.
            if str(calls) in os.environ.get("STUB_INSTR_ZERO_CALL", "").split(",") or zero_instr:
                factor = 0.0
            # TLS-104: STUB_SUSPEND_CALL=N: the host "sleeps" STUB_SUSPEND_S
            # during head's N-th call (per server): the reading cdo-speed takes
            # (CDO_HOST_SUSPENDED_FILE) rises while the call runs.
            if str(calls) == os.environ.get("STUB_SUSPEND_CALL"):
                path = os.environ["CDO_HOST_SUSPENDED_FILE"]
                now = float(open(path).read().split()[0])
                with open(path, "w") as f:
                    f.write(str(now + float(os.environ["STUB_SUSPEND_S"])))
        counter = os.path.join(os.environ["STUB_DIR"], f"instr.{os.getpid()}")
        done = int(open(counter).read()) if os.path.exists(counter) else 0
        with open(counter, "w") as f:
            f.write(str(done + int(1e9 * factor * work)))
        # STUB_PERF_EARLY (TLS-111 review r2): answer only once the stub perf
        # attached to this call has ended (gone, or a zombie its parent has not
        # reaped), so cdo-speed always finds it ended at the stop: deterministic.
        if os.environ.get("STUB_PERF_EARLY"):
            mark = os.path.join(os.environ["STUB_DIR"], f"perfpid.{os.getpid()}")
            end = time.monotonic() + 10
            while time.monotonic() < end and os.path.exists(mark):
                try:
                    pid = open(mark).read().strip()
                    state = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[0]
                except (OSError, IndexError):
                    break
                if state in ("Z", "X"):
                    break
                time.sleep(0.01)
        result = {"structuredContent": {"exitCode": 0, "stdoutText": "ok", "stubSleptS": slept}}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
if os.path.exists(me):
    os.remove(me)
"#;

/// ⟨TLS-83⟩ A stand-in `perf` (cdo-speed's CDO_SPEED_PERF test hook): the
/// probe (`... -- true`) reads a count; attached (`-p <pids> -o <file>`), it
/// writes, on SIGINT, how far the stub servers' published counters
/// (`$STUB_DIR/instr.<pid>`, and `cycles.<pid>` since TLS-111) moved since it
/// started, as `perf stat -x,` does on
/// a hybrid CPU (one core type `<not counted>`). It speaks perf's control FIFO
/// (`-D -1 --control fifo:<ctl>,<ack>`): it counts from `enable` and answers
/// `ack`; STUB_PERF_SLOW=<s> delays its counters between `enable` and `ack`, as
/// perf's event setup does. STUB_PERF_NONE: every attach
/// reads `<not counted>` on both. STUB_PERF_VANISH: it deletes itself after the
/// probe, so starting it for a call fails. STUB_PERF_NOACK: it never answers
/// `ack`.
const SPEED_PERF_STUB: &str = r#"#!/usr/bin/env python3
import json, os, signal, sys, time
a = sys.argv[1:]
def total(pids, kind="instr"):
    s = 0
    for p in pids:
        f = os.path.join(os.environ["STUB_DIR"], f"{kind}.{p}")
        if os.path.exists(f):
            s += int(open(f).read())
    return s
def cpu_ns(pids):
    # The attached processes' CPU time (user + system) from /proc, in ns.
    s = 0
    for p in pids:
        try:
            f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()
            s += int(f[11]) + int(f[12])
        except (OSError, IndexError, ValueError):
            pass
    return s * 1e9 / os.sysconf("SC_CLK_TCK")
def lines(n, c, run_ns=1e6, partial=False):
    # TLS-111: instructions and cycles per PMU, with perf's running time (ns:
    # the attached tasks' CPU time while counted, split over the PMUs, equal for
    # a PMU's events when nothing multiplexes) and running % (as real perf on a
    # hybrid split: whole percents whose sum passes 100). Attached (not the
    # probe): STUB_PERF_SPLIT=<share> puts that share of each count on
    # cpu_atom; STUB_PERF_MUX: cpu_core's cycles ran 10 % shorter than its
    # instructions (multiplexed); STUB_PERF_MUX_ATTACH=<n>: only on the run's
    # n-th attach (TLS-130). A PMU never run on reads `<not counted>` at 0 ns
    # and 0.00 %; STUB_PERF_PINNED_LOST: cpu_core's pinned group got no
    # counters (another pinned user): `<not counted>` at 0 ns and 100.00 %, as
    # the real perf prints it (TLS-130 review r1).
    probe = "--" in a
    if os.environ.get("STUB_PERF_NONE") and not probe:
        n = c = "<not counted>"
    split = 0.0 if probe else float(os.environ.get("STUB_PERF_SPLIT", "0"))
    mux = not probe and (os.environ.get("STUB_PERF_MUX")
                         or os.environ.get("STUB_PERF_MUX_ATTACH") == str(attach_no))
    out = ""
    for pmu, share in (("cpu_core", 1 - split), ("cpu_atom", split)):
        for kind, v in (("instructions", n), ("cycles", c)):
            lost = not probe and os.environ.get("STUB_PERF_PINNED_LOST") and pmu == "cpu_core"
            nc = share == 0 or isinstance(v, str) or lost
            val = "<not counted>" if nc else int(round(v * share))
            mux_f = 0.9 if mux and pmu == "cpu_core" and kind == "cycles" else 1
            ns = 0 if nc else int(run_ns * share * mux_f)
            pct = 100 if lost else 0 if nc else 100 if share == 1 else int(120 * share)
            if partial and not nc:
                pct = 90 if share == 1 else int(90 * share)  # summed: 90 (STUB_PARTIAL_CALL)
            out += f"{val},,{pmu}/{kind}/u,{ns},{pct:.2f},,\n"
    return out
if "--" in a:
    sys.stderr.write(lines(1000, 2000))
    if os.environ.get("STUB_PERF_VANISH"):
        os.remove(sys.argv[0])  # gone after the probe: every attach then fails to start
    sys.exit(0)
out = a[a.index("-o") + 1]
pids = a[a.index("-p") + 1].split(",")
with open(os.path.join(os.environ["STUB_DIR"], "perf-attaches"), "a") as f:
    f.write(pids[0] + "\n")  # the server's pid (cdo-speed lists it first)
with open(os.path.join(os.environ["STUB_DIR"], "perf-attaches")) as f:
    attach_no = len(f.read().splitlines())
# Every attach's arguments, one JSON list a line (TLS-130 review r1: what
# `Server._instr_start` really passes, `-e` included).
with open(os.path.join(os.environ["STUB_DIR"], "perf-argv"), "a") as f:
    f.write(json.dumps(a) + "\n")
# This perf's pid, for the server's STUB_PERF_EARLY wait.
with open(os.path.join(os.environ["STUB_DIR"], f"perfpid.{pids[0]}"), "w") as f:
    f.write(str(os.getpid()))
# perf's `-D -1 --control fifo:<ctl>,<ack>`: counting starts at `enable`, then
# `ack` (cdo-speed waits for it before the call). STUB_PERF_SLOW: slow counters.
assert a[a.index("-D") + 1] == "-1", a
ctl, ack = a[a.index("--control") + 1].split(":", 1)[1].split(",")
start = cstart = tstart = 0
def stop(*_):
    with open(out, "w") as f:
        # STUB_PERF_EARLY writes a fixed positive count: one measured right
        # after the ack can be 0, which a reader may not take for a count, so a
        # break that reads the early output could pass (TLS-83 r3, sol).
        if os.environ.get("STUB_PERF_EARLY"):
            f.write(lines(1000000, 2000000))
        else:
            # TLS-111 review r2: the counters ran for the tasks' CPU time (at
            # least 1 ms: a count never runs 0 ns); STUB_PERF_COVER=<f>: only f
            # of it (counters that missed part of the call).
            ran = (cpu_ns(pids) - tstart) * float(os.environ.get("STUB_PERF_COVER", "1"))
            # The server's own factor for this call (STUB_UNCOVER_CALL).
            cover = os.path.join(os.environ["STUB_DIR"], f"cover.{pids[0]}")
            if os.path.exists(cover):
                ran *= float(open(cover).read() or "1")
            # The server's STUB_PARTIAL_CALL mark for this call (TLS-130 r3).
            mark = os.path.join(os.environ["STUB_DIR"], f"partial.{pids[0]}")
            partial = os.path.exists(mark) and open(mark).read() == "1"
            f.write(lines(total(pids) - start, total(pids, "cycles") - cstart, max(ran, 1e6),
                          partial))
    sys.exit(0)
# Installed before the `ack` (review r1, sol): a fast call may SIGINT us at once.
# STUB_PERF_NOSTOP: SIGINT does nothing (a perf hanging at its stop).
signal.signal(signal.SIGINT, signal.SIG_IGN if os.environ.get("STUB_PERF_NOSTOP") else stop)
with open(ctl) as c:
    assert c.readline().strip() == "enable"
    # Like perf, the counters come up some time after `enable`; only the `ack`
    # says they run. STUB_PERF_NOACK: never.
    time.sleep(float(os.environ.get("STUB_PERF_SLOW", "0")))
    while os.environ.get("STUB_PERF_NOACK"):
        time.sleep(0.05)
    start, cstart, tstart = total(pids), total(pids, "cycles"), cpu_ns(pids)
    with open(ack, "w") as k:
        k.write("ack\n")
# STUB_PERF_EARLY: it ends right after the `ack`, with valid output (a count of
# only part of the call).
if os.environ.get("STUB_PERF_EARLY"):
    stop()
while True:
    time.sleep(0.05)
"#;

#[cfg(target_os = "linux")]
fn speed_with_stub(
    dir: &std::path::Path,
    load: &str,
    extra: &[&str],
    env: &[(&str, &str)],
) -> (Option<i32>, String) {
    use std::os::unix::fs::PermissionsExt;
    let stub = dir.join("alsem");
    std::fs::write(&stub, SPEED_STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    // TLS-83: head runs under its own name, so the stub can tell the sides apart.
    let head = dir.join("alsem-head");
    if !head.exists() {
        std::os::unix::fs::symlink(&stub, &head).unwrap();
    }
    let state = dir.join(format!("state-{}", extra.join("-")));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(dir.join("loadavg"), load).unwrap();
    // A calm machine unless a test passes its own pressure file.
    std::fs::write(
        dir.join("pressure"),
        "some avg10=1.00 avg60=1.00 avg300=1.00 total=0\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
    )
    .unwrap();
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let mut cmd = crate::clean_env::command("python3");
    private_slot(&mut cmd, dir);
    cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(&ws)
        .args(["--base-exe"])
        .arg(&stub)
        .arg("--head-exe")
        .arg(&head)
        .args(["--rounds", "1", "--max-rounds", "1", "--out"])
        .arg(state.join("out"))
        .args(extra)
        .env("ALSEM_SPEED_CACHE", dir.join("cache"))
        .env("STUB_DIR", &state)
        .env("STUB_BUILD_S", "0.4")
        .env("CDO_SPEED_WARM_MIN_SECONDS", "0.2")
        .env("CDO_SPEED_LOADAVG_FILE", dir.join("loadavg"))
        .env("CDO_SPEED_PRESSURE_FILE", dir.join("pressure"))
        .env("CDO_SPEED_HOST_PRESSURE_FILE", dir.join("pressure"))
        .env("CDO_SPEED_CALM_PRESSURE", "25")
        // TLS-103: a stub clock. The stub servers burn exact CPU, but their
        // wall time is the host's: under load a stub build read x1.78 against
        // the 1.6 wall limit and failed a test about instruction counts. With
        // wall taken as CPU, a stub run's verdict no longer depends on the
        // host. (A test may still pass its own value in `env`.)
        .env("CDO_SPEED_TEST_WALL_IS_CPU", "1");
    // An empty value removes the variable (a test turning a default off).
    for (k, v) in env {
        if v.is_empty() {
            cmd.env_remove(k);
        } else {
            cmd.env(k, v);
        }
    }
    let out = cmd.output().expect("spawn python3 scripts/cdo-speed");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), text)
}

/// ⟨TLS-83⟩ cdo-speed counts each call's user-mode instructions (a stub perf,
/// SPEED_PERF_STUB, reading what the stub servers publish) and reports the
/// instruction ratio per call, and on its verdict line for the model build and
/// the round total. The limit (x1.02) is ADVISORY: a head doing 5 % more work
/// is reported ("ADVISORY (not failing)") and still PASSes. When perf cannot
/// count, the run says so and judges CPU and wall only, as before.
///
/// Mutations (2026-11-18): the advisory appended to `failed` instead -> the 5 %
/// head FAILs (red); the instruction summary dropped from the verdict line ->
/// no `instructions: build x1.0500` there (red); `instr_probe` treating a perf
/// that cannot run as counting -> the run without perf breaks instead of
/// falling back (red). Each restored, green. Review r1 (Fable): the OSError
/// guard in `instr_start` narrowed away -> the vanished perf breaks the run
/// (red); `instr_stop` returning a 0 count -> `round-total x0.8333` with one
/// uncounted call (red; the all-calls-uncounted case alone stayed GREEN under
/// this break, both sides' totals being 0, hence the one-call case). Each
/// restored, green. TLS-92's gate (2026-11-18): the wait for perf's `ack` skipped
/// -> "no call fully counted" under a stand-in perf whose counters come up 0.5 s
/// after `enable` (red; with the delay BEFORE the FIFO open instead, the break
/// stayed GREEN: the open itself synchronized, so the stub now lags where real
/// perf does). Restored, green. TLS-111 (2026-11-21): the verdict saying "no
/// call fully counted" whatever the count -> "...; 37 of 40 calls counted)"
/// beside it (red); restored, green. Review r2 (sol): the STUB_PERF_EARLY case is
/// deterministic (each stub server answers once its perf has ended) and checks
/// every record; `instr_stop` reading an early-ended perf's output anyway ->
/// "a perf that ended early was counted" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_reports_instructions_and_their_limit_is_advisory() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let perf = bin.path().join("perf");
    std::fs::write(&perf, SPEED_PERF_STUB).unwrap();
    std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
    let perf = perf.to_str().unwrap();
    let verdict = |text: &str| -> String {
        text.lines()
            .rfind(|l| l.contains("cdo-speed: PASS") || l.contains("cdo-speed: FAIL"))
            .unwrap_or_else(|| panic!("a verdict line: {text}"))
            .to_string()
    };

    // Head does 5 % more work: reported, advisory, still a PASS.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_INSTR_HEAD", "1.05"),
        ],
    );
    assert_eq!(code, Some(0), "an advisory must not fail the run: {text}");
    let v = verdict(&text);
    assert!(
        v.contains("cdo-speed: PASS")
            && v.contains(
                "instructions: build x1.0500, round-total x1.0500; advisory limit x1.02, ADVISORY: "
            ),
        "{text}"
    );
    assert!(
        text.contains("ADVISORY (not failing): build: instructions x1.0500"),
        "{text}"
    );

    // The same work: x1.0000, no advisory. The stand-in perf starts 0.5 s late
    // (longer than the old fixed 0.2 s attach wait, which then counted the call
    // from part-way, a 0: TLS-92's gate): every call is still counted whole,
    // round total included, since the call waits for perf's `ack`.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_PERF_SLOW", "0.5"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        verdict(&text)
            .contains("(instructions: build x1.0000, round-total x1.0000; advisory limit x1.02)"),
        "{text}"
    );
    assert!(!text.contains("ADVISORY"), "{text}");

    // No perf: said, and CPU and wall only.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", "/nonexistent/perf"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("instructions: NOT counted, CPU and wall only"),
        "{text}"
    );
    assert!(
        verdict(&text).contains("(instructions: not counted, CPU and wall only)"),
        "{text}"
    );

    // Review r1 (Fable): perf gone after the probe (starting it for a call
    // fails): the calls are not counted, and the run still passes.
    let gone = tempfile::tempdir().unwrap();
    let gone_perf = gone.path().join("perf");
    std::fs::write(&gone_perf, SPEED_PERF_STUB).unwrap();
    std::fs::set_permissions(&gone_perf, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", gone_perf.to_str().unwrap()),
            ("STUB_PERF_VANISH", "1"),
        ],
    );
    assert_eq!(code, Some(0), "counting broke the run: {text}");
    assert!(
        verdict(&text).contains("(instructions: no call fully counted; advisory limit x1.02)"),
        "{text}"
    );

    // Review r1 (Fable): calls with no instruction counted on either core type
    // are not counted (a 0 pulled the round total down).
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_PERF_NONE", "1"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        verdict(&text).contains("(instructions: no call fully counted; advisory limit x1.02)"),
        "{text}"
    );

    // ... and ONE such call among counted ones (head's 9th call on the rounds'
    // server, a round call) leaves that call and the round total uncounted, not
    // a total pulled down by a 0.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_INSTR_ZERO_CALL", "9"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    let v = verdict(&text);
    assert!(
        v.contains("(instructions: build x1.0000; advisory limit x1.02; ")
            && v.contains(" calls counted)")
            && !v.contains(" 0 of "),
        "the round total took a 0, or the count of counted calls is missing: {text}"
    );

    // TLS-111 (an A/A run counted 55 of 64 calls and said "no call fully
    // counted"): with head's builds and a round call uncounted, neither summary
    // ratio exists, and the verdict says how many calls were counted.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_INSTR_ZERO_CALL", "1,9"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    let v = verdict(&text);
    assert!(
        v.contains(
            "(instructions: build and round-total not fully counted; advisory limit x1.02; "
        ) && v.contains(" calls counted)")
            && !v.contains("no call fully counted"),
        "{text}"
    );

    // Review r1 (sol): a perf that never acks costs each server at most
    // INSTR_GIVE_UP waits, then its calls go uncounted, said once.
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_PERF_NOACK", "1"),
            ("CDO_SPEED_INSTR_ATTACH_S", "0.3"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    let gave_up = text
        .matches("perf failed to attach 3 times in a row")
        .count();
    assert!(gave_up > 0, "no give-up: {text}");
    // No server tried more than 3 times (a server with fewer calls tries fewer
    // times), and those that gave up tried exactly 3. Counted from cdo-speed's
    // own line per failed attach, by server pid (TLS-103: the stub perf's log
    // missed an attempt whose stub was still starting when the 0.3 s ran out,
    // under load). The stub's log still bounds the attaches it saw. Break: the
    // give-up's early return dropped -> a server tries on every call (red; the
    // message alone stayed GREEN under that break, since it is printed at the
    // third failure either way).
    let mut per_server = std::collections::BTreeMap::<String, usize>::new();
    for l in text.lines().filter(|l| l.contains("perf attach failed (")) {
        let pid = l.split("(pid ").nth(1).unwrap().split(')').next().unwrap();
        *per_server.entry(pid.to_string()).or_default() += 1;
    }
    assert!(
        per_server.values().all(|&n| n <= 3),
        "attaches went on after the give-up: {per_server:?}"
    );
    assert_eq!(
        per_server.values().filter(|&&n| n == 3).count(),
        gave_up,
        "{per_server:?}"
    );
    let mut stub_saw = std::collections::BTreeMap::<String, usize>::new();
    for pid in std::fs::read_to_string(dir.path().join("state-/perf-attaches"))
        .unwrap_or_default()
        .lines()
    {
        *stub_saw.entry(pid.to_string()).or_default() += 1;
    }
    assert!(
        stub_saw.values().all(|&n| n <= 3),
        "the stub saw attaches after the give-up: {stub_saw:?}"
    );
    assert!(
        verdict(&text).contains("(instructions: no call fully counted; advisory limit x1.02)"),
        "{text}"
    );

    // Review r2 (sol): a perf that acks but never stops ends its server's
    // counting at the first stop timeout. Break: no give-up on a stop timeout
    // (each ack resets the attach streak: every call tries again).
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_PERF_NOSTOP", "1"),
            ("CDO_SPEED_INSTR_STOP_S", "0.3"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("perf did not stop within 0.3 s"), "{text}");
    let mut per_server = std::collections::BTreeMap::<String, usize>::new();
    for pid in std::fs::read_to_string(dir.path().join("state-/perf-attaches"))
        .unwrap()
        .lines()
    {
        *per_server.entry(pid.to_string()).or_default() += 1;
    }
    assert!(
        per_server.values().all(|&n| n == 1),
        "a server kept counting after perf hung at its stop: {per_server:?}"
    );

    // Review r2 (sol): a perf that ended before cdo-speed stopped it (valid
    // output, part of the call) gives no count. Break: its output read anyway
    // (every call counted, x1.0000).
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf),
            ("STUB_PERF_EARLY", "1"),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    // TLS-111 review r2 (sol): deterministic now (each stub server answers only
    // once its perf has ended), and checked per call: no record holds a count,
    // not only no summary ratio (a count can make a CPU failure advisory).
    assert!(
        verdict(&text).contains("(instructions: no call fully counted; advisory limit x1.02)"),
        "a perf that ended early was counted: {text}"
    );
    let json = speed_json(dir.path());
    let counted: Vec<_> = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| !r["instr"].is_null() || !r["counts"].is_null())
        .map(|r| format!("{} {} r{}", r["call"], r["side"], r["round"]))
        .collect();
    assert!(
        counted.is_empty(),
        "early-ended calls with a count: {counted:?}"
    );
}

/// ⟨TLS-83⟩ The REAL perf counts the stub servers' calls: one binary twice, so
/// every ratio is about 1 and no line says "not counted". This pins perf's
/// actual output format (event names, `-x,` columns, `<not counted>`), which the
/// stub perf only imitates. Skipped, saying so, where perf cannot count
/// user-mode instructions (CI, a host whose perf sysctls reset).
#[cfg(target_os = "linux")]
#[test]
fn speed_counts_instructions_with_the_real_perf() {
    // Review r2 (sol): the preflight is cdo-speed's own probe, so it counts
    // through the same pinned groups (`perf_event_args`) the attaches use.
    let probe = r#"
import importlib.machinery, importlib.util, sys
loader = importlib.machinery.SourceFileLoader("cdo_speed", sys.argv[1])
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("cdo_speed", loader))
loader.exec_module(m)
events, why = m.instr_probe()
print("can" if events else f"cannot: {why}")
"#;
    let out = crate::clean_env::command("python3")
        .args(["-c", probe])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .env("ALSEM_SPEED_CACHE", "/nonexistent/cdo-speed-cache")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    if !said.starts_with("can") {
        eprintln!("skipped: perf cannot count through cdo-speed's pinned groups here: {said}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &[("CDO_SPEED_INSTR", "1")]);
    assert_eq!(code, Some(0), "{text}");
    let v = text
        .lines()
        .rfind(|l| l.contains("cdo-speed: PASS"))
        .unwrap_or_else(|| panic!("{text}"))
        .to_string();
    assert!(text.contains("instructions: counted per call"), "{text}");
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("state-/out/speed.json")).unwrap(),
    )
    .unwrap();
    // TLS-130 removed the systematic refusal (a PMU's events enabled and read
    // apart while a server thread ran; pinned by
    // `speed_perf_groups_keep_a_pmus_running_times_equal`). Refusals outside the
    // run's control stay legitimate under the suite's load, any number of them
    // (review r1, sol): an attach whose ack takes over CDO_SPEED_INSTR_ATTACH_S
    // (5 s), a short call whose /proc CPU ticks miss `covers`' band, a pinned
    // group that lost its counters. Each uncounted TIMED record must carry its
    // own reason (`uncounted`), so a refusal printed by an untimed rebuild
    // (`head2-again` in the TLS-130 log) cannot stand in for it. The verdict
    // with ratios missing is pinned deterministically by
    // `speed_prints_each_uncounted_calls_refusal`.
    let records = json["records"].as_array().unwrap();
    let calls: Vec<_> = records.iter().filter(|r| r["total"] != true).collect();
    let uncounted: Vec<_> = calls
        .iter()
        .filter(|r| r["instr"].is_null() || r["cycles"].is_null())
        .collect();
    let silent: Vec<_> = uncounted
        .iter()
        .filter(|r| !r["uncounted"].is_string())
        .map(|r| {
            format!(
                "{} {} r{}.{}",
                r["call"], r["side"], r["round"], r["warm_pass"]
            )
        })
        .collect();
    assert!(
        silent.is_empty(),
        "an uncounted call without its reason: {silent:?}\n{text}"
    );
    // Review r2 (Fable): the count in the log, so a drift towards refusing most
    // calls is visible; review r3: at least one counted call (a second buys
    // nothing, and more refusals under load are no regression).
    eprintln!(
        "real perf: {} of {} calls counted",
        calls.len() - uncounted.len(),
        calls.len()
    );
    assert!(
        uncounted.len() < calls.len(),
        "no call counted under the real perf (reasons: {:?}); a real perf format \
         change refuses every call, as does another pinned perf user holding the counters \
         (`pgrep -a -x perf`)\n{text}",
        uncounted
            .iter()
            .map(|r| &r["uncounted"])
            .collect::<Vec<_>>()
    );
    if uncounted.is_empty() {
        assert!(
            v.contains("(instructions: build x") && v.contains("round-total x"),
            "{text}"
        );
    } else {
        let counted = records
            .iter()
            .filter(|r| r["instr"].as_f64().is_some_and(|i| i > 0.0))
            .count();
        assert!(
            v.contains(&format!("; {counted} of {} calls counted)", records.len())),
            "{text}"
        );
    }
    // TLS-111: user cycles too, under the real perf's running-% columns (the
    // unpinned stub servers move between core types, so a PMU's share alone is
    // below 100 while the kind's sum is 100).
    // The longest counted call (a short one can have 0 user seconds: no clock).
    let counted = calls
        .iter()
        .filter(|r| !r["instr"].is_null())
        .max_by(|a, b| a["cpu"].as_f64().partial_cmp(&b["cpu"].as_f64()).unwrap())
        .unwrap_or_else(|| panic!("a counted record: {json}"));
    // A clock only where the call had user seconds (review r2, sol).
    assert!(
        counted["cycles"].as_f64().is_some_and(|c| c > 0.0)
            && (counted["user"].as_f64().is_none_or(|u| u <= 0.0)
                || counted["clock"].as_f64().is_some_and(|c| c > 0.0)),
        "the real perf's cycles were not counted: {counted}"
    );
}

/// ⟨TLS-130⟩ The REAL perf, started and stopped while the attached tasks run
/// (12 spinning processes, as a server's threads can be at a call's start or
/// end under load), with cdo-speed's own events (`perf_event_args`), twice:
/// every stop's output is taken as a count. A PMU's events apart are enabled
/// and read one after the other, so one of them ran longer: on server1 (perf
/// 7.0.14, 2026-11-25, no other perf user;
/// /work/.al-sem/tools-notes/tls130/probe-2026-11-25.log) 10 of 10 such stops
/// were refused ("multiplexed"), 0 of 10 grouped, so two stops pin it. Then
/// (review r1, hybrid only) a third stop with the spinners on the P-cores and
/// 16 ordinary cpu_core events beside ours: the pinned (`:D`) group must read
/// 100.00 % running (unpinned it read 49-50 %, probe-pinned-2026-11-25.log).
/// Skipped, saying so, where perf cannot count. Review r1 (sol): the third
/// stop uses only the P-cores in this process's CPU mask
/// (`os.sched_getaffinity`; none: that stop is skipped, saying so), every
/// spinner must be alive and inside that mask before perf attaches, and every
/// exit path reaps perf, the spinners and the pipes. Review r2 (Fable): the
/// test waits (up to 10 s) until each spinner's mask EQUALS the wanted one
/// (before `taskset` applies it, a spinner still has this process's mask);
/// spinners are registered one by one as each starts, and each ends by itself
/// after 120 s; perf ending before its ack prints its stderr; a refusal with a
/// `<not counted>` row at 100.00 % names the pinned-rival cause.
///
/// Mutation (2026-11-25, review r1): the P-core list back to the machine-wide
/// `cpu_core/cpus`, the test run under `taskset -c 12-21` (the E-cores, as a
/// cpuset without P-cores) -> "a spinner ended or runs outside this process's
/// CPUs" (red); restored, the same run -> "pinned stop skipped: no P-core",
/// green. Redone after review r2's wait for the wanted mask (2026-11-26): ->
/// "a spinner ended, or runs outside this process's CPUs [12, ..., 21] or the
/// wanted [0, ..., 11]" (red); restored, the same run green (below).
///
/// Mutation (2026-11-25): `perf_event_args` back to one `-e` per event ->
/// "stop 1 refused" with `cpu_core/instructions/u` at 2968286980 ns against its
/// cycles at 2968141108, `cpu_atom` 2729111466 against 2729080424 (red; also
/// "not one perf group per PMU" in `speed_perf_parsing_and_warm_judging`).
/// Restored, green. Redone at two stops (review r1): "stop 1 refused",
/// `cpu_core` instructions 2434296526 ns against cycles 2434751329 (the second
/// event longer this time; red). Restored, green. Review r1: `:D` dropped ->
/// "the pinned group was time-sliced" (now "did not run 100.00 %"), cpu_core
/// at 33.00 % (red). Restored,
/// green.
#[cfg(target_os = "linux")]
#[test]
fn speed_perf_groups_keep_a_pmus_running_times_equal() {
    let py = r#"
import importlib.machinery, importlib.util, os, select, signal, subprocess, sys, time
loader = importlib.machinery.SourceFileLoader("cdo_speed", sys.argv[1])
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("cdo_speed", loader))
loader.exec_module(m)
events, why = m.instr_probe()
if not events:
    print(f"skipped: {why}")
    sys.exit(0)
# The CPUs this process may run on (review r1, sol: a container's cpuset can
# leave out the P-cores, where `taskset` to them fails).
allowed = os.sched_getaffinity(0)
RIVAL = ("another CPU-wide pinned perf user on the host may hold the counters (a real "
         "signal, not a flake: cdo-speed refuses such counts too). Check with "
         "`pgrep -a -x perf` and `pgrep -a -x samply` (look for `perf stat -a`/`record -a` "
         "or `:D` events), and the NMI watchdog: /proc/sys/kernel/nmi_watchdog")
# Each spinner ends by itself after 120 s, even if this script is killed.
SPIN = "import time\nend = time.monotonic() + 120\nwhile time.monotonic() < end: pass"
def stop_once(mask=None, extra=()):
    spin, perf, fds = [], None, []
    wanted = mask or allowed
    pre = ("taskset", "-c", ",".join(map(str, sorted(mask)))) if mask else ()
    # Every exit path reaps perf, the spinners and the pipes, each registered as
    # soon as it exists (reviews r1 and r2).
    try:
        for _ in range(12):
            spin.append(subprocess.Popen([*pre, sys.executable, "-c", SPIN]))
        # Review r2 (Fable): wait (bounded) until `taskset` has applied each mask;
        # before, a spinner still has this process's mask.
        deadline = time.monotonic() + 10
        while any(p.poll() is None and os.sched_getaffinity(p.pid) != wanted for p in spin):
            if time.monotonic() > deadline:
                break
            time.sleep(0.02)
        for p in spin:
            if (p.poll() is not None or os.sched_getaffinity(p.pid) != wanted
                    or not os.sched_getaffinity(p.pid) <= allowed):
                sys.exit(f"a spinner ended, or runs outside this process's CPUs "
                         f"{sorted(allowed)} or the wanted {sorted(wanted)}")
        # Counting from `enable`'s ack, as cdo-speed does: no stop before perf runs.
        ctl_r, ctl_w = os.pipe()
        fds.append(ctl_r)
        fds.append(ctl_w)
        ack_r, ack_w = os.pipe()
        fds.append(ack_r)
        fds.append(ack_w)
        perf = subprocess.Popen([m.PERF, "stat", "--no-scale", "-x,", *extra,
                                 *m.perf_event_args(events),
                                 "-D", "-1", "--control", f"fd:{ctl_r},{ack_w}",
                                 "-p", ",".join(str(p.pid) for p in spin)],
                                pass_fds=(ctl_r, ack_w), stderr=subprocess.PIPE, text=True)
        os.write(ctl_w, b"enable\n")
        deadline = time.monotonic() + 30
        got = b""
        while b"ack" not in got:
            if perf.poll() is not None:
                sys.exit(f"perf ended before its ack (exit {perf.returncode}): "
                         f"{perf.stderr.read()}")
            left = deadline - time.monotonic()
            if left <= 0:
                sys.exit("perf gave no ack in 30 s")
            if select.select([ack_r], [], [], min(left, 0.5))[0]:
                got += os.read(ack_r, 64)
        time.sleep(0.5)
        perf.send_signal(signal.SIGINT)
        text = perf.communicate(timeout=30)[1]
    finally:
        if perf is not None and perf.poll() is None:
            perf.kill()
            perf.wait()
        for p in spin:
            p.kill()
            p.wait()
        for fd in fds:
            os.close(fd)
    if m.perf_parse(text, events) is None:
        # A pinned group that got no counters reads `<not counted>` at 100.00 %.
        # Both on one event's row (review r3, sol).
        lost = any(ln.startswith("<not counted>,") and ln.split(",")[4:5] == ["100.00"]
                   for ln in text.splitlines())
        sys.exit(f"refused{': ' + RIVAL if lost else ''}:\n{text}")
    return text
def cpu_list(text):
    out = set()
    for part in text.strip().split(","):
        lo, _, hi = part.partition("-")
        out.update(range(int(lo), int(hi or lo) + 1))
    return out
for stop in range(2):
    stop_once()
# Pinned (review r1): spinners on the P-cores beside 16 ordinary cpu_core
# events (two groups of 8; 8 general counters): unpinned, our group would run
# about half the time; pinned, all of it. Only on the P-cores this process may
# use; none: that stop is skipped, saying so.
p_cores = set()
if events == m.HYBRID_EVENTS:
    p_cores = cpu_list(open("/sys/bus/event_source/devices/cpu_core/cpus").read()) & allowed
    if not p_cores:
        print(f"pinned stop skipped: no P-core in this process's CPUs {sorted(allowed)}")
if p_cores:
    eight = ("cpu_core/branches/u,cpu_core/branch-misses/u,cpu_core/cache-misses/u,"
             "cpu_core/cache-references/u,cpu_core/L1-dcache-loads/u,cpu_core/LLC-loads/u,"
             "cpu_core/LLC-load-misses/u,cpu_core/dTLB-loads/u")
    text = stop_once(p_cores, ("-e", "{" + eight + "}", "-e", "{" + eight + "}"))
    ours = [ln for ln in text.splitlines() if ",cpu_core/instructions/u," in ln
            or ",cpu_core/cycles/u," in ln]
    if len(ours) != 2 or any(ln.split(",")[4] != "100.00" for ln in ours):
        # Every spinner was on the P-cores (checked), so a share under 100 % is a
        # time-sliced group: `:D` is missing.
        sys.exit(f"the pinned group did not run 100.00 %: `:D` is missing.\n{text}")
print("ok")
"#;
    let out = crate::clean_env::command("python3")
        .args(["-c", py])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .env("ALSEM_SPEED_CACHE", "/nonexistent/cdo-speed-cache")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && (text.contains("ok") || text.contains("skipped: ")),
        "{text}"
    );
    eprintln!("{text}");
}

/// cdo-speed's lines that say why a call went uncounted (one per call; an
/// attach give-up then stops printing, after 3 failed attaches).
#[cfg(target_os = "linux")]
fn speed_refusals(text: &str) -> Vec<&str> {
    const WHY: [&str; 5] = [
        "perf output not taken as a count",
        "call not counted",
        "perf attach failed",
        "NOT COVERED",
        "perf did not stop",
    ];
    text.lines()
        .filter(|l| WHY.iter().any(|w| l.contains(w)))
        .collect()
}

/// ⟨TLS-130⟩ An uncounted call prints why, and the verdict says how many
/// calls were counted: the stub perf multiplexes only the run's first attach
/// (STUB_PERF_MUX_ATTACH 1: cpu_core's cycles ran 10 % shorter). Exactly one
/// call record has no count, exactly one refusal line is printed, naming that
/// call's side's server with perf's rows, and the verdict's "N of M calls
/// counted" matches speed.json. The other silent paths (perf ending early, an
/// unreadable output, no instruction at all) print a refusal too; perf ending
/// early is checked here as well (STUB_PERF_EARLY: every call).
///
/// Mutations (2026-11-25): the "perf output not taken as a count" print
/// dropped -> "one refusal line for the one uncounted call" (red); the
/// early-end print dropped -> "an early-ended call without its refusal" (red).
/// Each restored, green. A pinned group without counters (STUB_PERF_PINNED_LOST,
/// split 70/30) is refused as perf output on every call. Review r1: the
/// `<not counted>`-at-0.00-% rule dropped from `perf_parse` -> "a pinned group
/// without counters was counted: 0 of 40 refused" (red). Restored, green.
/// Review r1 (sol), 2026-11-26: the record's reason dropped in `timed_call`
/// (`refusal = None`) -> "the refusal's reason on its own record" (red).
/// Restored, green. Only `Server._instr_start`'s perf call changed to one `-e`
/// per event (`perf_event_args` and the probe left as they are) -> "every
/// attach with one pinned group per PMU: 46 attaches, wrong: [[..., "-e",
/// "cpu_core/instructions/u", "-e", "cpu_core/cycles/u", ...]]" (red).
/// Restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_prints_each_uncounted_calls_refusal() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        let mut env = vec![("CDO_SPEED_INSTR", "1"), ("CDO_SPEED_PERF", perf.as_str())];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        (dir, code, text)
    };

    let (dir, code, text) = run(&[("STUB_PERF_MUX_ATTACH", "1")]);
    assert_eq!(code, Some(0), "{text}");
    let json = speed_json(dir.path());
    let records = json["records"].as_array().unwrap();
    let uncounted: Vec<_> = records
        .iter()
        .filter(|r| r["total"] != true && r["instr"].is_null())
        .collect();
    assert_eq!(uncounted.len(), 1, "one call muxed: {uncounted:?}\n{text}");
    let side = uncounted[0]["side"].as_str().unwrap();
    let refusals = speed_refusals(&text);
    assert!(
        refusals.len() == 1
            && refusals[0].contains(&format!("cdo-speed: {side}"))
            && refusals[0].contains("perf output not taken as a count")
            && refusals[0].contains(",cpu_core/cycles/u,"),
        "one refusal line for the one uncounted call ({side}): {refusals:?}\n{text}"
    );
    let counted = records
        .iter()
        .filter(|r| r["instr"].as_f64().is_some_and(|i| i > 0.0))
        .count();
    assert!(
        speed_verdict(&text).contains(&format!("; {counted} of {} calls counted)", records.len())),
        "the verdict's counted calls: {counted} of {}\n{text}",
        records.len()
    );
    // Review r1 (sol): the refusal is tied to its timed record, and the verdict
    // with a ratio missing is deterministic: the muxed call is the run's first
    // timed call (base's build, warm-up pass 1), so the build has no ratio and
    // the round total keeps its own.
    assert!(
        uncounted[0]["call"] == "build"
            && uncounted[0]["uncounted"] == "perf output not taken as a count"
            && records
                .iter()
                .filter(|r| r["total"] != true && !r["instr"].is_null())
                .all(|r| r["uncounted"].is_null()),
        "the refusal's reason on its own record: {uncounted:?}\n{text}"
    );
    let v = speed_verdict(&text);
    assert!(
        v.contains("(instructions: round-total x") && !v.contains("build x"),
        "the verdict with the build's ratio missing: {v}"
    );
    // Review r1 (sol): the arguments of every real attach (`-p`, from
    // `Server._instr_start`), spelled out, not taken from `perf_event_args`.
    let argv = std::fs::read_to_string(dir.path().join("state-/perf-argv")).unwrap();
    let attaches: Vec<Vec<String>> = argv
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let want = [
        "-e",
        "{cpu_core/instructions/u,cpu_core/cycles/u}:D",
        "-e",
        "{cpu_atom/instructions/u,cpu_atom/cycles/u}:D",
    ];
    let bad: Vec<_> = attaches
        .iter()
        .filter(|a| {
            let events: Vec<&str> = a
                .iter()
                .enumerate()
                .filter(|(i, x)| *x == "-e" || (*i > 0 && a[i - 1] == "-e"))
                .map(|(_, x)| x.as_str())
                .collect();
            events != want || !a.iter().any(|x| x == "-p")
        })
        .collect();
    assert!(
        // At least one attach per timed call (untimed rebuilds attach too).
        attaches.len() >= records.iter().filter(|r| r["total"] != true).count() && bad.is_empty(),
        "every attach with one pinned group per PMU: {} attaches, wrong: {bad:?}",
        attaches.len()
    );

    let (_dir, code, text) = run(&[("STUB_PERF_EARLY", "1")]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.lines()
            .filter(|l| l.contains("perf ended before the call did; call not counted"))
            .count()
            >= 2,
        "an early-ended call without its refusal: {text}"
    );

    // Review r1: cpu_core's pinned group got no counters (`<not counted>` at
    // 0 ns, 100.00 %) on calls that ran 70/30 over the PMUs: refused as perf
    // output, before coverage would see it.
    let (dir, code, text) = run(&[("STUB_PERF_PINNED_LOST", "1"), ("STUB_PERF_SPLIT", "0.3")]);
    assert_eq!(code, Some(0), "{text}");
    let records = speed_json(dir.path())["records"]
        .as_array()
        .unwrap()
        .clone();
    let calls = records.iter().filter(|r| r["total"] != true).count();
    let lost = text
        .lines()
        .filter(|l| {
            l.contains("perf output not taken as a count")
                && l.contains("<not counted>,,cpu_core/instructions/u,0,100.00,,")
        })
        .count();
    assert!(
        calls > 0
            && lost >= calls
            && records.iter().all(|r| r["instr"].is_null())
            && speed_verdict(&text).contains("(instructions: no call fully counted"),
        "a pinned group without counters was counted: {lost} of {calls} refused\n{text}"
    );
}

/// A stand-in perf (SPEED_PERF_STUB) in its own folder, executable.
#[cfg(target_os = "linux")]
fn stub_perf(dir: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let perf = dir.join("perf");
    std::fs::write(&perf, SPEED_PERF_STUB).unwrap();
    std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
    perf.to_str().unwrap().to_string()
}

/// The verdict line (PASS, FAIL or HOST UNSTABLE) of a cdo-speed run.
#[cfg(target_os = "linux")]
fn speed_verdict(text: &str) -> String {
    text.lines()
        .rfind(|l| {
            l.contains("cdo-speed: PASS")
                || l.contains("cdo-speed: FAIL")
                || l.contains("cdo-speed: HOST UNSTABLE")
        })
        .unwrap_or_else(|| panic!("a verdict line: {text}"))
        .to_string()
}

#[cfg(target_os = "linux")]
fn speed_json(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("state-/out/speed.json")).unwrap())
        .unwrap()
}

/// ⟨TLS-111⟩ cdo-speed counts user cycles beside instructions, per PMU, and
/// labels a judged row whose sides' clock (user cycles / user s) or cycles per
/// instruction differ beyond x1.15, never changing the verdict:
/// - counts split 70/30 over cpu_core and cpu_atom (running 70 % and 30 %, as
///   an unpinned call reads): parsed per PMU and summed, no label;
/// - the same split with cpu_core's cycles running 10 % shorter than its
///   instructions (multiplexed; real perf, forced: 241.0 against 261.7 ms): no
///   call counted. The stub's running % sums past 100, as real perf's does on
///   a hybrid split, and counts;
/// - head's clock x0.83 (STUB_SLOW_HEAD 1.2: more CPU, the same cycles): PASS
///   with `CLOCK` and no `CPI`;
/// - head's cycles per instruction x1.2 (STUB_CPI_HEAD): PASS with `CPI` and no
///   `CLOCK`.
///
/// The stub burns USER time: its first burn loop read the CPU clock, a system
/// call, so ~70 % of its CPU was system time and the clock (cycles per user
/// second) read 9-10 GHz, with false CLOCK labels from the user/system split.
///
/// Mutations (2026-11-21): the running-share check in `perf_counts` disabled ->
/// the 95 % run counted ("a multiplexed count was taken"; red; round 2 replaced
/// that rule, see `speed_perf_parsing_and_warm_judging`); the CLOCK flags
/// appended to `failed` -> "a label failed the run" (red); `clock_cpi_flags`
/// testing `r > band` instead of either way -> no CLOCK on the x0.83 run (red);
/// `kind_total` summing `cpu_core` only -> the cycles-sum assertion (red). Each
/// restored, green. The clock's divisor (user, not user + system seconds) is
/// pinned by `speed_cpu_failure_is_advisory_only_with_equal_instructions_and_a_label`.
#[cfg(target_os = "linux")]
#[test]
fn speed_counts_cycles_and_labels_clock_and_cpi() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        let mut env = vec![("CDO_SPEED_INSTR", "1"), ("CDO_SPEED_PERF", perf.as_str())];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        (dir, code, text)
    };

    // Counts on both PMUs: parsed per PMU, summed.
    let (dir, code, text) = run(&[("STUB_PERF_SPLIT", "0.3")]);
    assert_eq!(code, Some(0), "{text}");
    let v = speed_verdict(&text);
    assert!(v.contains("(instructions: build x1.0000"), "{text}");
    assert!(!v.contains(" CLOCK:") && !v.contains(" CPI:"), "{text}");
    let json = speed_json(dir.path());
    let build = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["call"] == "build" && r["side"] == "base")
        .unwrap()
        .clone();
    let count = |e: &str| build["counts"][e].as_f64().unwrap_or(0.0);
    assert!(
        count("cpu_core/instructions/u") > 0.0
            && count("cpu_atom/instructions/u") > 0.0
            && count("cpu_core/cycles/u") > 0.0
            && count("cpu_atom/cycles/u") > 0.0,
        "per PMU: {build}"
    );
    let cycles = build["cycles"].as_f64().unwrap();
    assert_eq!(
        cycles,
        count("cpu_core/cycles/u") + count("cpu_atom/cycles/u"),
        "{build}"
    );
    assert_eq!(
        build["instr"].as_f64().unwrap(),
        count("cpu_core/instructions/u") + count("cpu_atom/instructions/u"),
        "{build}"
    );
    let (user, sys) = (
        build["user"].as_f64().unwrap(),
        build["sys"].as_f64().unwrap(),
    );
    assert!(
        (user + sys - build["cpu"].as_f64().unwrap()).abs() < 1e-6,
        "{build}"
    );
    let clock = build["clock"].as_f64().unwrap();
    assert!((cycles / user / 1e9 - clock).abs() < 1e-9, "{build}");
    assert!(
        (2.0..4.5).contains(&clock),
        "the stub's ~3 GHz: {clock} {build}"
    );
    assert!(build["cpi"].as_f64().is_some_and(|c| c > 0.0), "{build}");
    assert_eq!(json["clock_band"], 1.15, "{json}");

    // cpu_core's cycles ran shorter than its instructions: multiplexed, no count.
    let (_dir, code, text) = run(&[("STUB_PERF_SPLIT", "0.3"), ("STUB_PERF_MUX", "1")]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        speed_verdict(&text)
            .contains("(instructions: no call fully counted; advisory limit x1.02)"),
        "a multiplexed count was taken: {text}"
    );

    // A lower clock on head: labelled CLOCK, still a PASS.
    let (dir, code, text) = run(&[("STUB_SLOW_HEAD", "1.2")]);
    assert_eq!(code, Some(0), "a label failed the run: {text}");
    let v = speed_verdict(&text);
    assert!(
        v.contains("cdo-speed: PASS CLOCK: the sides' clocks (user cycles / user s) differ beyond x1.15 on build x0.8"),
        "{text}"
    );
    assert!(!v.contains(" CPI:"), "{text}");
    let json = speed_json(dir.path());
    assert!(
        !json["clock_labels"].as_array().unwrap().is_empty()
            && json["cpi_labels"].as_array().unwrap().is_empty(),
        "{json}"
    );

    // More cycles per instruction on head: labelled CPI, still a PASS.
    let (_dir, code, text) = run(&[("STUB_CPI_HEAD", "1.2")]);
    assert_eq!(code, Some(0), "a label failed the run: {text}");
    let v = speed_verdict(&text);
    assert!(
        v.contains(" CPI: the sides' cycles per instruction differ beyond x1.15 on build x1.2"),
        "{text}"
    );
    assert!(!v.contains(" CLOCK:"), "{text}");
}

/// ⟨TLS-111⟩ cdo-speed's perf parsing and warm-up judging, called directly
/// (the script imported):
/// - only `instructions` and `cycles`, once each per PMU, reach perf (both are
///   fixed counters, so nothing multiplexes), as one perf group per PMU
///   (TLS-130); any other event, or one twice, is refused;
/// - `perf_counts` on REAL perf output (server1, perf 7.0.14): a hybrid split
///   whose running shares sum to 116 %, a pinned call whose atom rows are
///   `<not counted>` (0 ns, 0.00), and `true`'s truncated 93 + 6 count; a
///   multiplexed run (cpu_core's cycles ran 241.0 ms, its instructions 261.7)
///   and an event's row given twice do not, nor (review r2, sol) a positive
///   count at 0 ns or a `<not counted>` row at a running time; on one PMU, equal
///   running times count and unequal ones do not;
/// - `judge_warm` keeps a row judged after pass 2 judged when the extra pair's
///   samples pull its median base CPU under the cutoff (review r1, sol);
/// - `covers` is one-sided with a cap: real CDO calls (A/A run of f7da9a93)
///   reading +2.6-2.7 % (build 184.41 s against 179.63 s CPU) count, as do short
///   calls 25 ms short or 22 ms over; 5.4 % or 40 % short and +11 % (over the
///   x1.10 cap) do not.
///
/// Mutations (2026-11-21): the kind check dropped from `perf_event_args` ->
/// `cpu_core/cache-misses/u` accepted (red). Round 2: the equal-running-time
/// check off -> "multiplexed: counted" (red); the duplicate-row check off -> "a
/// row twice: counted" (red); `judge_warm` ignoring `keep_judged` -> "a row
/// judged after pass 2 lost its judgment" (red). Each restored, green. Round 3:
/// the 0-ns check off -> "a count at 0 ns: counted" (red); the `<not counted>`
/// check off -> "<not counted> at a running time: counted" (red). Each
/// restored, green. Round 4: the old two-sided 2 % band -> "covers(184.41,
/// 179.63) is False" (red); the cap dropped -> "covers(200.0, 179.63) is True"
/// (red). Each restored, green. TLS-130 review r1 (2026-11-25): `:D` dropped
/// from `perf_event_args` -> "not one pinned perf group per PMU" (red); the
/// `<not counted>`-at-0.00-% rule dropped -> "a pinned group without counters:
/// counted" (red). Each restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_perf_parsing_and_warm_judging() {
    let py = r#"
import importlib.machinery, importlib.util, sys
loader = importlib.machinery.SourceFileLoader("cdo_speed", sys.argv[1])
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("cdo_speed", loader))
loader.exec_module(m)
if m.perf_event_args(m.HYBRID_EVENTS) != [
        "-e", "{cpu_core/instructions/u,cpu_core/cycles/u}:D",
        "-e", "{cpu_atom/instructions/u,cpu_atom/cycles/u}:D"]:
    sys.exit(f"not one pinned perf group per PMU: {m.perf_event_args(m.HYBRID_EVENTS)}")
if m.perf_event_args(m.PLAIN_EVENTS) != ["-e", "{instructions:u,cycles:u}:D"]:
    sys.exit(f"not one pinned perf group: {m.perf_event_args(m.PLAIN_EVENTS)}")
for bad in (m.HYBRID_EVENTS + ("cpu_core/cache-misses/u",),
            ("cpu_core/cycles/u", "cpu_core/cycles/u"),
            ("instructions:u", "cycles:u", "branches:u")):
    try:
        m.perf_event_args(bad)
    except ValueError:
        continue
    sys.exit(f"accepted {bad}")

# Real `perf stat --no-scale -x,` output, perf 7.0.14 on server1 (2026-11-21).
split = (  # attached (-p) to a process starting threads; shares sum to 116 %
    "9296421848,,cpu_core/instructions/u,1025384631,85.00,,\n"
    "2696136676,,cpu_core/cycles/u,1025384631,85.00,,\n"
    "1784288134,,cpu_atom/instructions/u,240884042,31.00,,\n"
    "539680691,,cpu_atom/cycles/u,240884042,31.00,,\n")
pinned = (  # on the P-cores only: the atom rows not counted
    "519833560432,,cpu_core/instructions/u,134502078026,100.00,,\n"
    "234607557207,,cpu_core/cycles/u,134502078026,100.00,,\n"
    "<not counted>,,cpu_atom/instructions/u,0,0.00,,\n"
    "<not counted>,,cpu_atom/cycles/u,0,0.00,,\n")
truncated = (  # `true`, 93.7 % and 6.3 % printed 93 and 6 (summing to 99)
    "145690,,cpu_core/instructions/u,2590713,93.00,,\n"
    "362574,,cpu_core/cycles/u,2590713,93.00,,\n"
    "0,,cpu_atom/instructions/u,173406,6.00,,\n"
    "0,,cpu_atom/cycles/u,173406,6.00,,\n")
muxed = (  # 16 more events on cpu_core force multiplexing: its two differ
    "2283271690,,cpu_core/instructions/u,261738976,47.00,,\n"
    "615183361,,cpu_core/cycles/u,240993765,43.00,,\n"
    "977709388,,cpu_atom/instructions/u,131670406,27.00,,\n"
    "298829742,,cpu_atom/cycles/u,131670406,27.00,,\n")
# TLS-130 review r1, perf 7.0.14 on server1 (2026-11-25, probe-pinned-2026-11-25.log):
# attached beside a competing pinned group, cpu_core's pinned group got no counters.
pinned_lost = (
    "<not counted>,,cpu_core/instructions/u,0,100.00,,\n"
    "<not counted>,,cpu_core/cycles/u,0,100.00,,\n"
    "1841531994,,cpu_atom/instructions/u,669085616,62.00,,\n"
    "540300573,,cpu_atom/cycles/u,669085616,62.00,,\n")
cases = [
    ("hybrid split", split, True),
    ("pinned, atom not counted", pinned, True),
    ("a pinned group without counters", pinned_lost, False),
    ("whole-percent truncation", truncated, True),
    ("multiplexed", muxed, False),
    ("a row twice", split + "5,,cpu_core/cycles/u,1025384631,85.00,,\n", False),
    ("a count at 0 ns", split.replace(",1025384631,", ",0,"), False),
    ("<not counted> at a running time",
     pinned.replace("<not counted>,,cpu_atom/instructions/u,0,", "<not counted>,,cpu_atom/instructions/u,5,")
           .replace("<not counted>,,cpu_atom/cycles/u,0,", "<not counted>,,cpu_atom/cycles/u,5,"), False),
]
for name, text, ok in cases:
    got = m.perf_counts(text, m.HYBRID_EVENTS)
    if (got is not None) != ok:
        sys.exit(f"{name}: {'refused' if ok else 'counted'}: {text!r}")
if m.perf_counts(split, m.HYBRID_EVENTS)["cpu_atom/cycles/u"] != 539680691:
    sys.exit("per-PMU count misread")
plain = "10,,instructions:u,{a},100.00,,\n20,,cycles:u,{b},100.00,,\n"
if m.perf_counts(plain.format(a=500, b=500), m.PLAIN_EVENTS) is None:
    sys.exit("one PMU, equal running times, refused")
if m.perf_counts(plain.format(a=500, b=450), m.PLAIN_EVENTS) is not None:
    sys.exit("one PMU, unequal running times, counted")

# judge_warm: base CPU 30, 30, then 5, 5 (median 17.5 under the 20 s cutoff).
recs = []
for p, (b, h) in enumerate([(30, 40), (30, 20), (5, 6), (5, 4)]):
    for side, cpu in (("base", b), ("head", h)):
        recs.append(dict(call="warm-tests", side=side, round=0, warm_pass=p, wall=cpu,
                         cpu=cpu, sha="", load=1.0))
two = m.judge_warm(recs[:4])
if not two[0]["judged"]:
    sys.exit("judged after pass 2: precondition")
if m.judge_warm(recs)[0]["judged"]:
    sys.exit("precondition: the cutoff unjudges it on four passes")
if not m.judge_warm(recs, {"warm-tests"})[0]["judged"]:
    sys.exit("a row judged after pass 2 lost its judgment on the extra samples")

# covers(running s, CPU s): real CDO calls (A/A run of f7da9a93, aa3), and the
# one-sided band's edges.
for running, cpu, ok in [
    (184.41, 179.63, True),   # build: +2.7 %, read on every multi-threaded call
    (136.91, 133.38, True),   # warm-tests: +2.6 %
    (0.475, 0.5, True),       # a short call 25 ms short (ticks)
    (0.522, 0.5, True),       # 22 ms over
    (170.0, 179.63, False),   # 5.4 % short: counters missed part of the call
    (0.24, 0.4, False),       # 40 % short (the stub's STUB_PERF_COVER 0.6)
    (200.0, 179.63, False),   # +11 %: over the sanity cap
    (None, 1.0, False),
]:
    if m.covers(running, cpu) != ok:
        sys.exit(f"covers({running}, {cpu}) is {not ok}")
print("ok")
"#;
    let out = crate::clean_env::command("python3")
        .args(["-c", py])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        // Only imported (nothing runs), but the rule above holds for every use.
        .env("ALSEM_SPEED_CACHE", "/nonexistent/cdo-speed-cache")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success() && text.contains("ok"), "{text}");
}

/// ⟨TLS-111, note §5.2⟩ A judged warm-up row that fails while its two passes
/// disagree beyond x1.25 gets ONE more flipped pair. The stub's positional
/// factor is off (STUB_SECOND_FACTOR 1), and head's warm-tests costs a set
/// factor per pass (STUB_TESTS_HEAD_BY_PASS):
/// - 1.0, 2.0, then 1.0, 1.7: x1.41 fails with passes x1.0/x2.0; the added pair's
///   same-order passes agree (x1.0/x1.0, x2.0/x1.7): judged on all four (x1.36),
///   FAIL, the verdict saying why the pair was added; never a fifth pass;
/// - 1.0, 2.0, then 1.0, 1.0: passes 2 and 4 still disagree: HOST UNSTABLE,
///   exit 2, no rounds; speed.json says `verdict` HOST UNSTABLE and lists it in
///   `failed` (a `not failed` reader never sees a pass); a `FAIL:` line names
///   it; cdo-gate's summary shows a FAILED speed row, which alone fails the gate
///   with every other phase green;
/// - 1.5, 1.5: a failure both passes agree on adds nothing and fails.
///
/// Mutations (2026-11-21): `contradicted` ignoring the spread -> the agreeing
/// failure starts "warm-up 3 of 4" (red); `same_order_disagree` comparing
/// passes i and i + 1 instead of i and i + 2 -> the agreeing re-measure reads
/// HOST UNSTABLE, exit 2 (red); HOST UNSTABLE's `return 2` dropped -> the
/// still-disagreeing run PASSes, exit 0, on its four-pass mean x1.19 (red); the
/// extra pair allowed after pass 4 too (`p == passes >= 2`, `passes += 2`) ->
/// "warm-up 5 of 6" (red). Round 2: HOST UNSTABLE left out of `failed` -> the
/// `failed` assertion (red); `verdict` from `failed` alone -> "FAIL" (red); the
/// `FAIL:` lines printed only after the HOST UNSTABLE return -> no "FAIL: HOST
/// UNSTABLE:" line (red). Each restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_adds_one_flipped_pair_on_a_contradicted_failure() {
    let run = |by_pass: &str| {
        let dir = tempfile::tempdir().unwrap();
        let (code, text) = speed_with_stub(
            dir.path(),
            "1.0",
            &[],
            &[
                ("STUB_SECOND_FACTOR", "1.0"),
                ("STUB_TESTS_HEAD_BY_PASS", by_pass),
            ],
        );
        (dir, code, text)
    };

    // Contradicted, then the same orders agree: judged on four passes.
    let (dir, code, text) = run("1.0,2.0,1.0,1.7");
    assert_eq!(code, Some(1), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: FAIL")
            && v.contains("(one more flipped warm-up pair: a failure its passes contradicted beyond x1.25: warm-tests cpu x")
            && v.contains("the same-order passes agree, judged on 4 passes)"),
        "{text}"
    );
    assert!(
        text.contains("FAIL: warm-tests: CPU time x1.3") && text.contains("warm-up, 4 pass(es))"),
        "{text}"
    );
    assert!(
        text.contains("warm-up 4 of 4") && !text.contains("warm-up 5"),
        "exactly one pair added: {text}"
    );
    let json = speed_json(dir.path());
    assert_eq!(json["warm_samples"], 4, "{json}");
    assert_eq!(json["warm_samples_asked"], 2, "{json}");
    assert_eq!(json["verdict"], "FAIL", "{json}");
    let tests = json["warm_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["call"] == "warm-tests")
        .unwrap()
        .clone();
    let rs: Vec<f64> = tests["cpu"]["ratios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    assert_eq!(rs.len(), 4, "{tests}");
    let want = rs.iter().product::<f64>().powf(0.25);
    assert!(
        (tests["cpu"]["ratio"].as_f64().unwrap() - want).abs() < 1e-9,
        "the geometric mean of all four: {tests}"
    );
    assert!(text.contains("round 1:"), "the rounds ran: {text}");

    // Still contradicted: HOST UNSTABLE, exit 2, never a PASS.
    let (dir, code, text) = run("1.0,2.0,1.0,1.0");
    assert_eq!(code, Some(2), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: HOST UNSTABLE")
            && v.contains("the same-order passes still disagree: warm-tests cpu passes 2 and 4"),
        "{text}"
    );
    assert!(
        !text.contains("cdo-speed: PASS") && !text.contains("round 1:"),
        "{text}"
    );
    let json = speed_json(dir.path());
    assert!(
        !json["host_unstable"].as_array().unwrap().is_empty(),
        "{json}"
    );
    // Never green to a reader of `failed` (review r1, sol and Fable).
    assert_eq!(json["verdict"], "HOST UNSTABLE", "{json}");
    assert!(
        json["failed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().starts_with("HOST UNSTABLE")),
        "{json}"
    );
    assert!(text.contains("cdo-speed: FAIL: HOST UNSTABLE: "), "{text}");
    // cdo-gate's summary: a failed speed row naming it, with every other phase
    // green: the speed phase alone fails the gate.
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("speed.status"), "2 600\n").unwrap();
    std::fs::write(logs.join("speed.log"), text.as_bytes()).unwrap();
    for phase in [
        "program_resolve_harness",
        "lsp",
        "l4_summary_differential",
        "lib_cdo",
        "output_cdo",
    ] {
        std::fs::write(logs.join(format!("{phase}.status")), "0 10\n").unwrap();
        std::fs::write(
            logs.join(format!("{phase}.log")),
            "test result: ok. 3 passed; 0 failed; 0 ignored\n",
        )
        .unwrap();
    }
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg(format!("--summary={}", logs.display()))
        .output()
        .unwrap();
    let summary = String::from_utf8_lossy(&out.stdout);
    let speed_row = summary
        .lines()
        .find(|l| l.starts_with("cdo-gate:   speed "))
        .unwrap_or_else(|| panic!("a speed row: {summary}"));
    assert!(
        speed_row.contains("FAILED (2)") && speed_row.contains("HOST UNSTABLE"),
        "{speed_row}"
    );
    assert_eq!(
        summary
            .lines()
            .filter(|l| l.starts_with("cdo-gate:   ") && l.contains(" ok "))
            .count(),
        5,
        "precondition: every other phase green: {summary}"
    );
    assert!(!out.status.success(), "{summary}");

    // Both passes agree on the failure: nothing added, FAIL.
    let (_dir, code, text) = run("1.5,1.5");
    assert_eq!(code, Some(1), "{text}");
    assert!(
        speed_verdict(&text).starts_with("cdo-speed: FAIL")
            && !text.contains("one more flipped")
            && !text.contains("warm-up 3"),
        "{text}"
    );
    assert!(text.contains("warm-up, 2 pass(es))"), "{text}");
}

/// ENG-110 r2: an `OSError` in cdo-speed's IR rows (a dead pipe: the stub's IR server
/// closes its stdin), after the judged timing, labels the verdict and never fails or stops
/// the run. Mutation: the catch narrowed to `(CallFailed, SystemExit)` -> red.
#[cfg(target_os = "linux")]
#[test]
fn speed_ir_rows_never_fail_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &[("STUB_IR_DEAD", "1")]);
    assert_eq!(code, Some(0), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: PASS") && v.contains("IR ROWS NOT RUN (BrokenPipeError"),
        "{text}"
    );
}

/// ⟨TLS-111 review r1⟩ The extra pair judges EVERY failure in side order, and
/// may clear one:
/// - cross-row: warm-tests triggers (1.0, 2.0) and settles (0.9, 1.8), while the
///   build failed after pass 2 with agreeing passes (x1.4, x1.4) and reads x0.9
///   in the extra pair: its four-pass mean (x1.12) would pass it, but its
///   same-order passes disagree (x1.4/x0.9): HOST UNSTABLE naming the build;
/// - side order: the stub's positional factor ON (x1.5 for the second of a
///   pair), head's warm-tests x1.4 in every pass: per-pass ratios ~x2.1, ~x0.93,
///   ~x2.1, ~x0.93, so passes 3 and 4 agree with 1 and 2 only in the same side
///   order: FAIL judged on four passes, never HOST UNSTABLE;
/// - agree, then PASS: 0.9, 2.0 fails (x1.34) with contradicting passes; 0.8, 1.7
///   agree with them; the four-pass mean x1.25 PASSes, saying the pair was added.
///
/// Mutations (2026-11-21, round 2): the same-order check over the triggering
/// failures only (round 1's code) -> the cross-row run exits 1, not 2 (red);
/// passes 3 and 4 in the wrong side order (`first` base in passes 1 and 4, the
/// flip in passes 2 and 3) -> the side-order run reads HOST UNSTABLE, passes 1
/// and 3 x0.933/x2.073 (red). Each restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_extra_pair_judges_every_failure_in_side_order() {
    let run = |env: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], env);
        (dir, code, text)
    };

    // Cross-row: the build's failure must agree too.
    let (_dir, code, text) = run(&[
        ("STUB_SECOND_FACTOR", "1.0"),
        ("STUB_TESTS_HEAD_BY_PASS", "1.0,2.0,0.9,1.8"),
        ("STUB_BUILD_HEAD_BY_PASS", "1.4,1.4,0.9,0.9"),
    ]);
    assert_eq!(code, Some(2), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: HOST UNSTABLE") && v.contains("build cpu passes 1 and 3"),
        "{text}"
    );

    // Side order: passes 3 and 4 repeat passes 1 and 2's.
    let (_dir, code, text) = run(&[("STUB_TESTS_HEAD_BY_PASS", "1.4,1.4,1.4,1.4")]);
    assert_eq!(code, Some(1), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: FAIL")
            && v.contains("the same-order passes agree, judged on 4 passes)"),
        "{text}"
    );

    // Agree, then PASS on four passes.
    let (dir, code, text) = run(&[
        ("STUB_SECOND_FACTOR", "1.0"),
        ("STUB_TESTS_HEAD_BY_PASS", "0.9,2.0,0.8,1.7"),
    ]);
    assert_eq!(code, Some(0), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: PASS")
            && v.contains("a failure its passes contradicted beyond x1.25: warm-tests cpu")
            && v.contains("the same-order passes agree, judged on 4 passes)"),
        "{text}"
    );
    assert_eq!(speed_json(dir.path())["verdict"], "PASS");
}

/// ⟨TLS-111 review r1⟩ A wall-time failure adds the pair too, and one warm-up
/// pass never does:
/// - wall (the stub clock: wall is each call's CPU plus the seconds the stub
///   declares, `stubSleptS`; real sleeps here once read 3.92 s for 1.4 s under a
///   peer's CDO gate): both sides' judged calls declare 1 s, and head's
///   warm-tests 4 s more in passes 2 and 4 only, CPU equal: wall x3.9 against
///   x1 fails with contradicting passes; the pair agrees: FAIL on wall, four
///   passes;
/// - `--warm-samples 1`, head's warm-tests x2.0: FAIL, no pair (one pass has no
///   second to contradict it).
///
/// Mutations (2026-11-21, round 2): `warm_failures` judging CPU only -> no pair
/// on the wall failure (red); restored, green; redone with declared sleeps
/// (round 6), red again, restored, green. The one-pass case has no proof
/// of its own: the `p == passes == 2` guard is redundant there (one ratio has
/// no spread, so `contradicted` adds nothing), so no single break turns it red.
#[cfg(target_os = "linux")]
#[test]
fn speed_extra_pair_on_wall_and_never_with_one_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &[],
        &[
            ("STUB_SECOND_FACTOR", "1.0"),
            ("STUB_SLEEP_S", "1"),
            ("STUB_TESTS_HEAD_SLEEP_BY_PASS", "0,4,0,4"),
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.contains("contradicted beyond x1.25: warm-tests wall x")
            && !v.contains("warm-tests cpu x")
            && v.contains("the same-order passes agree, judged on 4 passes)"),
        "{text}"
    );
    assert!(
        text.contains("FAIL: warm-tests: wall time x") && text.contains("warm-up, 4 pass(es); "),
        "{text}"
    );

    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "1.0",
        &["--warm-samples", "1"],
        &[
            ("STUB_SECOND_FACTOR", "1.0"),
            ("STUB_TESTS_HEAD_BY_PASS", "2.0"),
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("FAIL: warm-tests: CPU time x")
            && !text.contains("one more flipped")
            && !text.contains("warm-up 2"),
        "{text}"
    );
}

/// ⟨TLS-111⟩ Owner decision (kraken, q-20261121T025802-abcd2da8), narrowed in
/// review r3 (Fable): a CPU-time failure is ADVISORY only when head's LOWER
/// clock explains it (CPU ratio x clock ratio within the limit), with equal
/// instructions (within x1.02 either way) and wall passing; otherwise it stays
/// a failure.
/// - head's clock x0.67 (STUB_SLOW_HEAD 1.5: x1.5 CPU, the same cycles): CPU
///   x1.5, x1.0 at equal clocks: PASS, "CPU ADVISORY" printing both ratios and
///   the clock, `advisory` in speed.json; warm-tests' contradicted passes get the
///   extra pair first;
/// - head needs x1.5 cycles per instruction (STUB_CPI_HEAD), clock equal: CPU
///   x1.5 with a CPI label is what a memory-stall regression looks like: FAIL;
/// - head's clock FASTER (x1.25, STUB_SLOW_HEAD 0.8) with x1.8 cycles per
///   instruction: CPU x1.44 and a CLOCK label in head's favour: FAIL (x1.8 at
///   equal clocks);
/// - the lower clock with x1.1 or x0.9 instructions: FAIL (the work changed);
/// - (TLS-130 review r3, sol) the lower clock with no contradiction
///   (STUB_SECOND_FACTOR 1): the build is waived (control); the same with
///   head's build counted partial (STUB_PARTIAL_CALL 1: running % summed to 89,
///   counts whole): FAIL saying why, `partial` on the record and the warm row;
/// - a clean clock: head's build spends 0.5x its CPU in the kernel
///   (STUB_SYS_HEAD; user time, cycles and instructions unchanged): CPU x1.5,
///   no label: FAIL. This also pins the clock's divisor: user + system seconds
///   would read its clock x0.67, a CLOCK label, and an advisory PASS.
///
/// The kernel time is read into a 4 MiB buffer every stub server holds: a
/// buffer of head's own raised its peak memory x1.35 (a FAIL whatever the
/// CPU), and 64 KiB reads spent ~15 % in user time (a false CLOCK, x0.85).
///
/// Mutations (2026-11-21, round 4, review r3): the round-1 rule restored (any
/// CLOCK or CPI label, no clock correction) -> "a CPI excess was waived" (red);
/// CLOCK only but no clock correction -> "a faster head clock waived a CPU
/// failure" (red); the lower instruction bound `1 / INSTR_LIMIT` dropped ->
/// "instructions x0.9 were waived" (red). Each restored, green. Earlier (round
/// 2, on the CPI-based cases then): the instruction band, the label condition
/// and the clock's divisor each red; round 3: contradiction first red (the
/// first run's assertion, unchanged). TLS-130 review r3 (2026-11-27):
/// `timed_call` dropping the call's reason (`partial = None`) -> "a partial
/// build count was waived (record false, row false)" (red; the control passed
/// first). Restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_cpu_failure_is_advisory_only_when_a_lower_clock_explains_it() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        // Both PMUs counted: a `<not counted>` PMU is partial (TLS-130 r2).
        let mut env = vec![
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf.as_str()),
            ("STUB_PERF_SPLIT", "0.3"),
        ];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        (dir, code, text)
    };
    let fails_with_no_advisory = |code: Option<i32>, text: &str| {
        code == Some(1)
            && text.contains("FAIL: build: CPU time x1.")
            && !text.contains("CPU ADVISORY")
    };

    let (dir, code, text) = run(&[("STUB_SLOW_HEAD", "1.5")]);
    assert_eq!(code, Some(0), "{text}");
    let v = speed_verdict(&text);
    assert!(
        v.starts_with("cdo-speed: PASS")
            && v.contains("CPU ADVISORY (")
            && v.contains("owner decision q-20261121T025802-abcd2da8")
            && v.contains("build: CPU time x1.")
            && v.contains("at equal clocks (CLOCK x0.6")
            && v.contains("with instructions x1.0000"),
        "{text}"
    );
    // Review r2 (sol): contradiction first. warm-tests' CPU failure would
    // qualify for the advisory, but the stub's positional factor makes its
    // passes disagree (x2.25/x1.0): the extra pair runs before any advisory.
    assert!(
        v.contains("(one more flipped warm-up pair: a failure its passes contradicted beyond x1.25: warm-tests cpu x")
            && text.contains("warm-up 4 of 4"),
        "a qualifying contradicted failure skipped the extra pair: {text}"
    );
    let json = speed_json(dir.path());
    assert!(
        !json["advisory"].as_array().unwrap().is_empty() && json["verdict"] == "PASS",
        "{json}"
    );

    // TLS-130 review r3 (sol), end to end: the same lower clock with no
    // contradiction waives the build (control); with head's first call on each
    // server (its build) counted partial (STUB_PARTIAL_CALL: running % summed
    // to 89, counts whole), the build's failure stays a FAIL, says why, and
    // speed.json's record and warm row carry `partial`.
    let waivable = [("STUB_SLOW_HEAD", "1.5"), ("STUB_SECOND_FACTOR", "1.0")];
    let (_dir, code, text) = run(&waivable);
    assert!(
        code == Some(0) && text.contains("ADVISORY (not failing): build: CPU time x1."),
        "control: the build was not waived: {text}"
    );
    let (dir, code, text) = run(&[waivable[0], waivable[1], ("STUB_PARTIAL_CALL", "1")]);
    let json = speed_json(dir.path());
    let partial_record = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["call"] == "build" && r["side"] == "head" && r["partial"].is_string());
    let partial_row = json["warm_rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["call"] == "build" && r["partial"].is_string());
    assert!(
        code == Some(1)
            && text.lines().any(|l| l.contains("FAIL: build: CPU time x1.")
                && l.contains("(no CPU advisory: a count may be partial: head build r"))
            && !text.contains("ADVISORY (not failing): build:")
            && partial_record
            && partial_row,
        "a partial build count was waived (record {partial_record}, row {partial_row}): {text}"
    );

    // More cycles per instruction (a stall regression's shape): never waived.
    let (_dir, code, text) = run(&[("STUB_CPI_HEAD", "1.5"), ("STUB_SECOND_FACTOR", "1.0")]);
    assert!(
        fails_with_no_advisory(code, &text) && speed_verdict(&text).contains(" CPI: "),
        "a CPI excess was waived: {text}"
    );

    // A clock in head's favour never waives.
    let (_dir, code, text) = run(&[
        ("STUB_CPI_HEAD", "1.8"),
        ("STUB_SLOW_HEAD", "0.8"),
        ("STUB_SECOND_FACTOR", "1.0"),
    ]);
    assert!(
        fails_with_no_advisory(code, &text) && speed_verdict(&text).contains(" CLOCK: "),
        "a faster head clock waived a CPU failure: {text}"
    );

    // Unequal instructions, either way.
    for instr in ["1.1", "0.9"] {
        let (_dir, code, text) = run(&[
            ("STUB_SLOW_HEAD", "1.5"),
            ("STUB_INSTR_HEAD", instr),
            ("STUB_SECOND_FACTOR", "1.0"),
        ]);
        assert!(
            fails_with_no_advisory(code, &text),
            "instructions x{instr} were waived: {text}"
        );
    }

    let (_dir, code, text) = run(&[("STUB_SYS_HEAD", "0.5")]);
    assert_eq!(code, Some(1), "a clean clock stays a failure: {text}");
    let v = speed_verdict(&text);
    assert!(
        text.contains("FAIL: build: CPU time x1.")
            && !text.contains("FAIL: peak memory")
            && !v.contains("CPU ADVISORY")
            && !v.contains(" CLOCK:"),
        "{text}"
    );
}

/// ⟨TLS-111 review r2, sol⟩ The advisory needs counts that cover the call on
/// both sides in every pass, and a CPU-ONLY failure. Each case has head's clock
/// lower with equal instructions and cycles (STUB_SLOW_HEAD; the advisory's
/// case since review r3), and:
/// - head's build uncounted in pass 1 only (STUB_INSTR_ZERO_BUILD_PASS 0): the
///   build row has no instruction ratio: its CPU failure stays a failure;
/// - CPU x1.8 (STUB_SLOW_HEAD 1.8; x1.0 at equal clocks) under the stub clock:
///   wall x1.8 fails too (limit 1.6): nothing is made advisory;
/// - the counters ran for 0.6 of each call's CPU seconds (STUB_PERF_COVER):
///   each burning call's record keeps its counts but reads `covered: false`,
///   with no instruction count (TLS-130) and no clock, so no build ratio, no
///   label and no advisory: FAIL.
///
/// The positional factor is off (STUB_SECOND_FACTOR 1) in the last two, so no
/// extra pair runs; in the first it runs (its warm-tests is contradicted).
///
/// Mutations (2026-11-21, round 3): `covers` always true -> the partly covered
/// run PASSes, exit 0 (red); `cpu_advisory` without its wall guard -> "CPU
/// ADVISORY" on the CPU-and-wall run (red); `judge` taking the clock, CPI and
/// instruction ratios from the passes that were counted (`any` pass, not
/// `all`) -> the build is made advisory from pass 2 alone and the run PASSes,
/// exit 0 (red). Each restored, green. Round 4 (the cases moved from more cycles
/// per instruction to a lower clock): all three proofs redone, each red again
/// (the partly covered run exit 0; "CPU ADVISORY" on the CPU-and-wall run; the
/// build advisory from pass 2 alone, x1.47 and x1.00 at equal clocks, exit 0).
/// TLS-130 (2026-11-25): `timed_call` keeping an uncovered count's `instr` ->
/// "uncovered counts kept for diagnostics, with no instruction count or clock"
/// (red). Restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_cpu_advisory_needs_covering_counts_and_a_cpu_only_failure() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        // Both PMUs counted, so only the guard under test withholds (TLS-130 r2).
        let mut env = vec![
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf.as_str()),
            ("STUB_PERF_SPLIT", "0.3"),
        ];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        (dir, code, text)
    };
    // The build's CPU failure is a FAIL line, and no advisory names it (the
    // verdict line carries the advisories).
    let no_build_advisory = |text: &str| {
        text.contains("FAIL: build: CPU time x1.")
            && !speed_verdict(text).contains("build: CPU time x")
    };

    // One side uncounted in one pass.
    let (_dir, code, text) = run(&[
        ("STUB_SLOW_HEAD", "1.5"),
        ("STUB_INSTR_ZERO_BUILD_PASS", "0"),
    ]);
    assert_eq!(code, Some(1), "{text}");
    assert!(no_build_advisory(&text), "{text}");

    // CPU and wall fail on one row.
    let (_dir, code, text) = run(&[("STUB_SLOW_HEAD", "1.8"), ("STUB_SECOND_FACTOR", "1.0")]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("FAIL: build: CPU time x1.")
            && text.contains("FAIL: build: wall time x1.")
            && !text.contains("CPU ADVISORY"),
        "{text}"
    );

    // Counters that cover only part of each call.
    let (dir, code, text) = run(&[
        ("STUB_SLOW_HEAD", "1.5"),
        ("STUB_SECOND_FACTOR", "1.0"),
        ("STUB_PERF_COVER", "0.6"),
    ]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("FAIL: build: CPU time x1.") && !text.contains("CPU ADVISORY"),
        "{text}"
    );
    let json = speed_json(dir.path());
    let builds: Vec<_> = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["call"] == "build")
        .collect();
    assert!(
        !builds.is_empty()
            && builds.iter().all(|r| r["covered"] == false
                && r["counts"]["cpu_core/instructions/u"]
                    .as_f64()
                    .is_some_and(|i| i > 0.0)
                && r["instr"].is_null()
                && r["clock"].is_null()),
        "uncovered counts kept for diagnostics, with no instruction count or clock: {builds:?}"
    );
    // TLS-130: so no instruction ratio either (a multiplexed perf group loses
    // the same time on both its events; only coverage shows it).
    assert!(
        !speed_verdict(&text).contains("build x"),
        "an uncovered count reached an instruction ratio: {text}"
    );
}

/// ⟨TLS-111 review r3, S2⟩ A rounds row's CPU failure is advisory only when
/// EVERY round over the limit is explained by itself: equal instructions, its
/// counts covering the call, its wall within WALL_LIMIT (1.3) and its own CPU
/// ratio at equal clocks within CPU_LIMIT (1.2). Every round call burns 0.4 s
/// (STUB_ROUND_BURN_S), so the round total (2.4 s a side, over MIN_SECONDS 2)
/// is judged; three rounds; head's factors per round in STUB_ROUND_HEAD
/// (slow:cpi). The four runs go in parallel (each its own folder, slot and
/// stub servers; the stub clock makes wall its CPU, so contention moves no
/// ratio): 258 s alone (206 s in the suite) became 30 s alone, with burns of
/// 0.4 s for 0.5 s (0.4 is the smallest that keeps the round total judged).
/// - all three at a lower clock (1.25:1): CPU x1.25, x1.0 at equal clocks, wall
///   x1.25: the round total is advisory, PASS;
/// - round 1 at more cycles per instruction (1:1.25), rounds 2 and 3 at a lower
///   clock: the medians (CPU x1.25, clock x0.8) would combine to x1.0, but round
///   1 is x1.25 at equal clocks: FAIL;
/// - all three at x1.35 (1.35:1): wall x1.35 over WALL_LIMIT (it would pass the
///   warm-up's 1.6): no advisory, FAIL;
/// - all three at a lower clock, but head's 9th call (a round call) counted for
///   half its CPU (STUB_UNCOVER_CALL): the round total is not covered, so no
///   clock: FAIL;
/// - (TLS-130 review r3, sol) all three at a lower clock, but head's 9th call
///   counted partial (STUB_PARTIAL_CALL: its running % summed to 89, counts
///   whole): no advisory, FAIL saying why; the call's record, head's
///   round-total record and the round-total row carry `partial`.
///
/// Mutations (2026-11-21, round 4): the rounds row judged on its medians (one
/// sample, as a warm-up row) -> "medians of different rounds combined" (red);
/// the rounds' advisory wall limit taken as the warm-up's 1.6 -> "CPU
/// ADVISORY" on the x1.35 wall run (red); the round total's `covered` from
/// `any` record instead of `all` -> "an uncovered round call let the round
/// total be advisory" (red). Each restored, green. Round 5 (parallel runs, 0.4 s
/// burns): all three redone, each red again, restored, green. TLS-130 review
/// r3 (2026-11-27), each alone: `timed_call` dropping the call's reason
/// (`partial = None`) -> "a partial round call let the round total be advisory
/// (call false, total false, row false)" (red); the round total's join of its
/// calls' reasons off (`partial=None`) -> "(call true, total false, row
/// false)" (red). Each restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_cpu_advisory_judges_every_round_by_itself() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |head: &str, extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        let mut env = vec![
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf.as_str()),
            ("STUB_SECOND_FACTOR", "1.0"),
            ("STUB_ROUND_BURN_S", "0.4"),
            ("STUB_ROUND_HEAD", head),
            // Both PMUs counted: a `<not counted>` PMU is partial (TLS-130 r2).
            ("STUB_PERF_SPLIT", "0.3"),
        ];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(
            dir.path(),
            "1.0",
            &["--rounds", "3", "--max-rounds", "3"],
            &env,
        );
        (dir, code, text)
    };
    let fails_with_no_advisory = |code: Option<i32>, text: &str| {
        code == Some(1)
            && text.contains("FAIL: round-total: CPU time x1.")
            && !text.contains("CPU ADVISORY")
    };

    let cases: [(&str, &[(&str, &str)]); 5] = [
        ("1.25:1,1.25:1,1.25:1", &[]),
        ("1:1.25,1.25:1,1.25:0.8", &[]),
        ("1.35:1,1.35:1,1.35:1", &[]),
        ("1.25:1,1.25:1,1.25:1", &[("STUB_UNCOVER_CALL", "9")]),
        ("1.25:1,1.25:1,1.25:1", &[("STUB_PARTIAL_CALL", "9")]),
    ];
    let results: Vec<_> = std::thread::scope(|s| {
        let runs: Vec<_> = cases
            .iter()
            .map(|&(head, extra)| s.spawn(move || run(head, extra)))
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let mut results = results.into_iter();

    let (dir, code, text) = results.next().unwrap();
    assert_eq!(code, Some(0), "{text}");
    assert!(
        speed_verdict(&text).contains("CPU ADVISORY (1: ")
            && speed_verdict(&text).contains("round-total: CPU time x1.2"),
        "{text}"
    );
    // The stub's state folder is named after the extra arguments.
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dir.path()
                .join("state---rounds-3---max-rounds-3/out/speed.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(json["verdict"], "PASS", "{json}");

    let (_dir, code, text) = results.next().unwrap();
    assert!(
        fails_with_no_advisory(code, &text),
        "medians of different rounds combined: {text}"
    );

    let (_dir, code, text) = results.next().unwrap();
    assert!(
        fails_with_no_advisory(code, &text) && text.contains("FAIL: round-total: wall time x1."),
        "{text}"
    );

    let (_dir, code, text) = results.next().unwrap();
    assert!(
        fails_with_no_advisory(code, &text) && text.contains("NOT COVERED"),
        "an uncovered round call let the round total be advisory: {text}"
    );

    // TLS-130 review r3 (sol): the first case's waived round total, with
    // head's 9th call (a round call) counted partial (STUB_PARTIAL_CALL): a
    // FAIL that says why; the call's record, head's round-total record and the
    // round-total row carry `partial`.
    let (dir, code, text) = results.next().unwrap();
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dir.path()
                .join("state---rounds-3---max-rounds-3/out/speed.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let records = json["records"].as_array().unwrap();
    let call = records.iter().any(|r| {
        r["total"] != true && r["side"] == "head" && r["round"] != 0 && r["partial"].is_string()
    });
    let total = records
        .iter()
        .any(|r| r["total"] == true && r["side"] == "head" && r["partial"].is_string());
    let row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["call"] == "round-total" && r["partial"].is_string());
    assert!(
        fails_with_no_advisory(code, &text)
            && text
                .lines()
                .any(|l| l.contains("FAIL: round-total: CPU time x1.")
                    && l.contains("(no CPU advisory: a count may be partial: head round-total r"))
            && call
            && total
            && row,
        "a partial round call let the round total be advisory (call {call}, total {total}, \
         row {row}): {text}"
    );
}

/// ⟨TLS-111 review r4⟩ The CPU ratio at equal clocks scales head's USER time
/// only (the clock is user cycles per user second), and its size matters, not
/// only its sign. Under the stub clock both sides' judged calls declare 2 s of
/// wall-only time (STUB_SLEEP_S, `stubSleptS`), so wall stays far under 1.6
/// whatever the CPU and whatever the host's load; positional factor off; all
/// must FAIL:
/// - head's clock x0.5 (STUB_SLOW_HEAD 2) plus kernel time of 0.45x its base
///   work on the build (STUB_SYS_HEAD; sol's case): CPU x2.45; scaling user and
///   system by the clock would read x1.23 and waive; user only reads x1.45;
/// - head's clock x0.83 (STUB_SLOW_HEAD 1.2) with x1.4 cycles per instruction:
///   CPU x1.68, x1.4 at equal clocks: a lower clock that explains only part of
///   the excess. (Fable suggested 1.17 and 1.35: 1/1.17 is x0.855, under 2 % from
///   the 15 % CLOCK band, so the label itself could flicker on /proc ticks.)
/// - review r5 (sol), passes that mix: head's build at CPU x1.4 in both passes,
///   its clock x0.6 with 95 % user time in pass 1 and x3.27 with 8 % user time in
///   pass 2 (STUB_BUILD_HEAD_PASS slow:cpi:sys; a 1 s base build so pass 2's
///   0.11 s of user time reads its clock to about 10 %): the aggregate clock
///   reads above 1, in head's FAVOUR (x1.25 measured), while the CPU at equal
///   clocks reads x1.21 (x1.42 CPU): it must not waive.
///
/// Mutations (2026-11-21, round 5): head's user and system time both scaled by
/// the clock (review r4's blocker) -> "user time only: the build was waived",
/// that case alone (red); a sign-only check (`clock ratio < 1` in place of the
/// CPU ratio at equal clocks within the limit) -> both cases waived, "the size"
/// included (red). Each restored, green. Round 6 (review r5): the `clock ratio
/// < 1` condition dropped -> "mixed passes: the build was waived" alone, as
/// "build: CPU time x1.42, x1.21 at equal clocks (CLOCK x1.248)" (red); and
/// again under the declared-sleep stub clock, x1.41, x1.20, CLOCK x1.295 (red);
/// restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_cpu_advisory_scales_user_time_only_and_by_size() {
    let bin = tempfile::tempdir().unwrap();
    let perf = stub_perf(bin.path());
    let run = |extra: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        let mut env = vec![
            ("CDO_SPEED_INSTR", "1"),
            ("CDO_SPEED_PERF", perf.as_str()),
            ("STUB_SECOND_FACTOR", "1.0"),
            ("STUB_SLEEP_S", "2"),
            // Both PMUs counted, so only the guard under test withholds (TLS-130 r2).
            ("STUB_PERF_SPLIT", "0.3"),
        ];
        env.extend_from_slice(extra);
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        (dir, code, text)
    };
    let cases: [&[(&str, &str)]; 3] = [
        &[("STUB_SLOW_HEAD", "2.0"), ("STUB_SYS_HEAD", "0.45")],
        &[("STUB_SLOW_HEAD", "1.2"), ("STUB_CPI_HEAD", "1.4")],
        &[
            ("STUB_BUILD_S", "1.0"),
            (
                "STUB_BUILD_HEAD_PASS",
                "1.667:0.798:0.0877,0.306:0.366:3.52",
            ),
        ],
    ];
    let results: Vec<_> = std::thread::scope(|s| {
        let runs: Vec<_> = cases
            .iter()
            .map(|&extra| s.spawn(move || run(extra)))
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    // Every case is checked before the test fails, so a break shows each one.
    let waived: Vec<String> = ["user time only", "the size", "mixed passes"]
        .iter()
        .zip(&results)
        .filter(|(_, (_dir, code, text))| {
            let v = speed_verdict(text);
            !(*code == Some(1)
                && text.contains("FAIL: build: CPU time x")
                && !text.contains("FAIL: build: wall")
                && v.contains(" CLOCK: ")
                && !v.contains("build: CPU time x"))
        })
        .map(|(what, (_dir, _code, text))| {
            format!("{what}: the build was waived (or the case did not hold): {text}")
        })
        .collect();
    assert!(waived.is_empty(), "{}", waived.join("\n\n"));
}

/// ⟨TLS-130 reviews r1 and r2, sol and Fable⟩ A count that may be partial never
/// makes a CPU failure advisory: no bound on what it lost is sound (the sides'
/// normal running-time excess differs, and lost time need not have run at the
/// call's average rate), so `cpu_advisory` gives none for a row where either
/// side's count is partial (`partial_count`). Through real `perf stat -x,`
/// rows (`perf_parse`, `partial_count`), `judge_warm` and `cpu_advisory`, on a
/// warm-up build, base 40 s CPU, all user time. Each counterexample's MEASURED
/// ratios pass every other condition (checked), so the old rules waived it:
/// - sol r2: head CPU and wall x1.54, measured instructions x1.019 and x1.298
///   at equal clocks (CLOCK x0.843); head's `cpu_atom` group `<not counted>`
///   (a pinned group lost after its tasks ran there), and the same with base's;
/// - Fable r2: head CPU and wall x1.45, measured instructions x0.985, clock
///   x0.8648 (the label only from the loss), x1.254 at equal clocks; head's
///   running % summed to 99 over the PMUs (80 + 19: review r3, sol, the case a
///   truncation allowance let through).
///
/// Each partial row is not waived and says why (`row["partial"]`). Controls:
/// the same measured numbers from whole counts (both PMUs counted, 84 % and
/// 36 %) are waived, so only the partial rule withholds. `partial_count` itself:
/// a hybrid split and a plain 100 % are whole; a `<not counted>` PMU, 80 + 19,
/// the real `true` line truncated to 93 + 6 (taken as partial: it only withholds
/// an advisory) and a plain 98 % are partial. The end-to-end path (a stub run's
/// verdict and speed.json) is pinned in
/// `speed_cpu_failure_is_advisory_only_when_a_lower_clock_explains_it` and
/// `speed_cpu_advisory_judges_every_round_by_itself`.
///
/// Mutation (2026-11-26, review r2): `row.get("partial") or` dropped from
/// `cpu_advisory` -> "a partial count waived a FAIL: sol r2 (head), sol r2
/// (base), Fable r2" (red). Restored, green. Review r3 (2026-11-27), each
/// break alone, each restored, then green: `partial_count`'s `<not counted>`
/// arm off (`lost = []`) -> "sol r2 (head): the row does not say why (no
/// `partial`)" (red); its running-% arm off (`total < 0`) -> "Fable r2: the row
/// does not say why" (red); `judge`'s coverage arm off -> "Fable r3 (head not
/// covered): the row does not say why" (red); `cpu_advisory` withholding on a
/// partial count in any round (`any(partial)`) -> "a partial count in a round
/// under the limit withheld the rounds row's advisory" (red).
#[cfg(target_os = "linux")]
#[test]
fn speed_cpu_advisory_never_on_a_partial_count() {
    let py = r#"
import importlib.machinery, importlib.util, sys
loader = importlib.machinery.SourceFileLoader("cdo_speed", sys.argv[1])
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("cdo_speed", loader))
loader.exec_module(m)
E = m.HYBRID_EVENTS

def csv(instr, cycles, ns, form):
    # `perf stat --no-scale -x,` rows, the real shapes: "whole" split 70/30 at
    # 84 % and 36 %; "atom lost" cpu_atom `<not counted>`; "short" 80 % and 19 %.
    if form == "atom lost":
        return (f"{int(instr)},,cpu_core/instructions/u,{int(ns)},100.00,,\n"
                f"{int(cycles)},,cpu_core/cycles/u,{int(ns)},100.00,,\n"
                "<not counted>,,cpu_atom/instructions/u,0,0.00,,\n"
                "<not counted>,,cpu_atom/cycles/u,0,0.00,,\n")
    pc, pa = {"whole": ("84.00", "36.00"), "short": ("80.00", "19.00")}[form]
    out = ""
    for pmu, f, pct in (("cpu_core", 0.7, pc), ("cpu_atom", 0.3, pa)):
        out += (f"{int(instr * f)},,{pmu}/instructions/u,{int(ns * f)},{pct},,\n"
                f"{int(cycles * f)},,{pmu}/cycles/u,{int(ns * f)},{pct},,\n")
    return out

def rec(side, p, cpu, instr, cycles, form):
    # "uncovered" (review r3, Fable): every PMU counted, running time 90 % of
    # the CPU seconds (under `covers`' band). `timed_call` drops such a count;
    # here its instructions and clock are kept BY HAND, so only `judge`'s
    # coverage arm can withhold the advisory.
    uncovered = form == "uncovered"
    ns = cpu * 1e9 * (0.9 if uncovered else 1.0)
    parsed = m.perf_parse(csv(instr, cycles, ns, "whole" if uncovered else form), E)
    if parsed is None:
        sys.exit(f"precondition: {form} parses")
    i = m.kind_total(parsed[0], "instructions")
    c = m.kind_total(parsed[0], "cycles")
    running = m.kind_total(parsed[1], "instructions") / 1e9
    covered = m.covers(running, cpu)
    if covered == uncovered:
        sys.exit(f"precondition: {form} {'covers' if covered else 'does not cover'}")
    return dict(call="build", side=side, round=0, warm_pass=p, wall=cpu, cpu=cpu, sha="",
                load=1.0, instr=i, cycles=c, user=cpu, sys=0.0, running_s=running,
                covered=covered, partial=m.partial_count(parsed),
                **m.clock_cpi(c, cpu, i, True))

def build_row(cpu_ratio, ir, cr, head_form, base_form="whole"):
    # Measured: head CPU and wall x cpu_ratio, instructions x ir, clock x cr.
    recs = []
    for p in range(2):
        hc = 40.0 * cpu_ratio
        recs.append(rec("base", p, 40.0, 1e11, 1.2e11, base_form))
        recs.append(rec("head", p, hc, 1e11 * ir, 1.2e11 * cr * hc / 40, head_form))
    rows = m.judge_warm(recs)
    if len(rows) != 1 or not rows[0]["judged"]:
        sys.exit(f"precondition: one judged build row: {rows}")
    return rows[0]

cases = [  # (name, CPU ratio, instructions, clock, head form, base form)
    ("sol r2 (head)", 1.54, 1.019, 1.298 / 1.54, "atom lost", "whole"),
    ("sol r2 (base)", 1.54, 1.019, 1.298 / 1.54, "whole", "atom lost"),
    ("Fable r2", 1.45, 0.985, 0.8648, "short", "whole"),
    ("Fable r3 (head not covered)", 1.54, 1.019, 1.298 / 1.54, "uncovered", "whole"),
]
waived = []
for name, cpu, ir, cr, hf, bf in cases:
    control = build_row(cpu, ir, cr, "whole")
    if m.cpu_advisory(control) is None:
        sys.exit(f"control ({name}): the same numbers from whole counts not waived")
    row = build_row(cpu, ir, cr, hf, bf)
    if not row.get("partial"):
        sys.exit(f"{name}: the row does not say why (no `partial`)")
    if m.cpu_advisory(row) is not None:
        waived.append(name)
if waived:
    sys.exit(f"a partial count waived a FAIL: {', '.join(waived)}")

# A rounds row (review r3, Fable): head CPU x1.25 in rounds 1 and 3 (over the
# 1.2 limit), x1.1 in round 2, each explained by a lower clock. A partial count
# in round 2 (not judged) leaves the advisory; one in round 1 withholds it.
def rounds_row(partial_round):
    recs = []
    for rnd, ratio in ((1, 1.25), (2, 1.1), (3, 1.25)):
        for side, cpu, cr in (("base", 10.0, 1.0), ("head", 10.0 * ratio, 1 / ratio)):
            form = "atom lost" if side == "head" and rnd == partial_round else "whole"
            r = rec(side, 0, cpu, 1e10, 1.2e10 * cr * cpu / 10, form)
            recs.append(dict(r, call="query", round=rnd))
    return next(r for r in m.judge(recs)[0] if r["call"] == "query")
if m.cpu_advisory(rounds_row(2)) is None:
    sys.exit("a partial count in a round under the limit withheld the rounds row's advisory")
if m.cpu_advisory(rounds_row(1)) is not None:
    sys.exit("a partial count in an over-limit round waived the rounds row")

def parsed(text, events=E):
    p = m.perf_parse(text, events)
    if p is None:
        sys.exit(f"precondition: parses: {text!r}")
    return p
truncated = ("145690,,cpu_core/instructions/u,2590713,93.00,,\n"
             "362574,,cpu_core/cycles/u,2590713,93.00,,\n"
             "0,,cpu_atom/instructions/u,173406,6.00,,\n"
             "0,,cpu_atom/cycles/u,173406,6.00,,\n")
plain = "10,,instructions:u,500,{p},,\n20,,cycles:u,500,{p},,\n"
for name, text, events, whole in [
    ("hybrid split", csv(1e9, 2e9, 1e9, "whole"), E, True),
    ("plain 100 %", plain.format(p="100.00"), m.PLAIN_EVENTS, True),
    ("a PMU not counted", csv(1e9, 2e9, 1e9, "atom lost"), E, False),
    ("80 + 19 (review r3)", csv(1e9, 2e9, 1e9, "short"), E, False),
    ("the real `true` truncated to 93 + 6", truncated, E, False),
    ("plain 98 %", plain.format(p="98.00"), m.PLAIN_EVENTS, False),
]:
    if (m.partial_count(parsed(text, events)) is None) != whole:
        sys.exit(f"partial_count: {name} read {'partial' if whole else 'whole'}")
print("ok")
"#;
    let out = crate::clean_env::command("python3")
        .args(["-c", py])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .env("ALSEM_SPEED_CACHE", "/nonexistent/cdo-speed-cache")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success() && text.contains("ok"), "{text}");
}

/// ⟨TLS-130 review r3, Fable⟩ `scripts/ir-cache-acceptance` counts through
/// cdo-speed's `perf_parse`: its `run_counted` runs a command under a stub perf
/// that writes a fixed hybrid count (70/30, 1000 instructions, 2000 cycles)
/// and must read it. `perf_parse` gaining a third value made that script's
/// `counts, running = parsed` crash every counted run while every test stayed
/// green; this test runs the script's own parse path.
///
/// Mutation (2026-11-26): the unpacking restored -> "ValueError: too many
/// values to unpack" (red). Restored, green.
#[cfg(target_os = "linux")]
#[test]
fn ir_cache_acceptance_counts_through_cdo_speeds_parse() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let perf = bin.path().join("perf");
    std::fs::write(
        &perf,
        r#"#!/usr/bin/env python3
import os, sys
a = sys.argv[1:]
with open(a[a.index("-o") + 1], "w") as f:
    f.write("700,,cpu_core/instructions/u,7000,84.00,,\n"
            "1400,,cpu_core/cycles/u,7000,84.00,,\n"
            "300,,cpu_atom/instructions/u,3000,36.00,,\n"
            "600,,cpu_atom/cycles/u,3000,36.00,,\n")
cmd = a[a.index("--") + 1:]
os.execvp(cmd[0], cmd)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
    let py = r#"
import importlib.machinery, importlib.util, os, sys
loader = importlib.machinery.SourceFileLoader("ir_cache_acceptance", sys.argv[1])
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("ir_cache_acceptance", loader))
loader.exec_module(m)
instr, cycles, cpu, wall, covered = m.run_counted(["true"], dict(os.environ),
                                                  m.speed.HYBRID_EVENTS)
if (instr, cycles) != (1000, 2000):
    sys.exit(f"run_counted read {instr} instructions, {cycles} cycles")
print("ok")
"#;
    let out = crate::clean_env::command("python3")
        .args(["-c", py])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/ir-cache-acceptance"
        ))
        .env("CDO_SPEED_PERF", &perf)
        .env("ALSEM_SPEED_CACHE", "/nonexistent/cdo-speed-cache")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success() && text.contains("ok"), "{text}");
}

/// ⟨TLS-104⟩ A host suspend during a timed call is labelled, never hidden: the
/// stub "sleeps" 5 s (the reading `CDO_HOST_SUSPENDED_FILE` rises) during head's
/// 2nd call. The run logs `host suspended 5 s during ...`, its verdict line
/// carries ` SUSPENDED:`, `speed.json` lists it, and the verdict itself (PASS,
/// exit 0) is the same limits' (label, never weaken). The control: the same run
/// with no suspend has no label and an empty list.
///
/// Mutation (2026-11-20): `Suspends.note` never recording a rise -> no
/// "host suspended" line, no label (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_labels_a_host_suspend() {
    let run = |suspend: bool| {
        let dir = tempfile::tempdir().unwrap();
        let reading = dir.path().join("suspended");
        std::fs::write(&reading, "100").unwrap();
        let reading = reading.to_str().unwrap().to_string();
        let mut env = vec![("CDO_HOST_SUSPENDED_FILE", reading.as_str())];
        if suspend {
            env.extend([("STUB_SUSPEND_CALL", "2"), ("STUB_SUSPEND_S", "5")]);
        }
        let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &env);
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("state-/out/speed.json")).unwrap(),
        )
        .unwrap();
        (code, text, json)
    };
    let (code, text, json) = run(true);
    assert_eq!(code, Some(0), "the same limits judge it: {text}");
    assert!(text.contains("host suspended 5 s during r0.1"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("cdo-speed: PASS SUSPENDED: the host slept")),
        "{text}"
    );
    // Each suspend is attributed to the head call it happened in (review: not to
    // the call after it), and that call's record carries it.
    let seen = json["suspended"].as_array().expect("suspended list");
    assert!(
        !seen.is_empty()
            && seen.iter().all(|s| s["seconds"] == 5.0
                && s["where"]
                    .as_str()
                    .is_some_and(|w| w.starts_with("during ") && w.ends_with(" head"))),
        "{json}"
    );
    let records = json["records"].as_array().expect("records");
    assert!(
        records.iter().any(|r| r["side"] == "head"
            && r["suspended_during"] == 5.0
            && r["suspended_before"] == 0),
        "{json}"
    );

    let (code, text, json) = run(false);
    assert_eq!(code, Some(0), "{text}");
    // (The hooks line names `CDO_HOST_SUSPENDED_FILE`: look for the label.)
    assert!(
        !text.contains(" SUSPENDED:") && !text.contains("host suspended"),
        "{text}"
    );
    assert_eq!(json["suspended"], serde_json::json!([]), "{json}");
}

/// ⟨TLS-104⟩ `scripts/host-suspended` reads the host's suspended seconds (a
/// whole number on a Linux host), and its test hook instead when set; an
/// unreadable hook file prints nothing and exits 1. cdo-slot's `status` says how
/// much of a hold's age the host spent suspended: the hold is taken at reading
/// 100, the reading moves to 160, and the status line names 60 s; the control
/// (unchanged reading) names none.
///
/// The hook files are written with no final newline: `host-suspended`'s first
/// form (`read ... || exit 1`) refused them, and this test found it.
///
/// Mutation (2026-11-20): `suspended_since` always printing `)` -> no "60 s of
/// it with the host suspended" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn host_suspend_reading_and_slot_status() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let real = crate::clean_env::command("bash")
        .arg(script)
        .output()
        .unwrap();
    assert!(real.status.success(), "{real:?}");
    let text = String::from_utf8_lossy(&real.stdout);
    assert!(text.trim().parse::<u64>().is_ok(), "{text:?}");
    let dir = tempfile::tempdir().unwrap();
    let reading = dir.path().join("suspended");
    let missing = crate::clean_env::command("bash")
        .arg(script)
        .env("CDO_HOST_SUSPENDED_FILE", &reading)
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());

    let slot = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
    let holder = crate::cdo_slot::Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let cmd = |args: &[&str]| {
        crate::clean_env::command("bash")
            .arg(slot)
            .args(args)
            .env("CDO_SLOT_DIR", dir.path().join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_HOST_SUSPENDED_FILE", &reading)
            .output()
            .unwrap()
    };
    std::fs::write(&reading, "100").unwrap();
    let pid = holder.0.id().to_string();
    let held = cmd(&["acquire", "t104", "test hold", "--bind", &pid]);
    assert!(held.status.success(), "{held:?}");
    let status = String::from_utf8_lossy(&cmd(&["status"]).stdout).into_owned();
    assert!(
        status.starts_with("held by t104") && !status.contains("suspended"),
        "{status}"
    );
    std::fs::write(&reading, "160").unwrap();
    let status = String::from_utf8_lossy(&cmd(&["status"]).stdout).into_owned();
    assert!(
        status.contains("60 s of it with the host suspended)"),
        "{status}\n{}",
        std::fs::read_to_string(dir.path().join("slot/status")).unwrap_or_default()
    );
}

/// ⟨TLS-103⟩ A stub run's verdict does not depend on the host's load: under
/// `speed_with_stub`'s stub clock (CDO_SPEED_TEST_WALL_IS_CPU) a call's wall
/// time is its CPU time. Here head's model build waits 1 s with no CPU
/// (STUB_HEAD_SLEEP_S), as a loaded host stretches it: about x3.5 in wall
/// against the 1.6 limit, the shape that failed TLS-102's stress run (x1.78).
/// The run still PASSes, judged on CPU alone.
///
/// The control (review, sol): the same delay with the stub clock off FAILs on
/// wall time, so the PASS above is the hook's doing. And the hook never judges
/// a real run: any value but `1`, or a run without `--base-exe` and
/// `--head-exe` (a gate builds its own binaries), is refused before anything
/// starts.
///
/// Mutations (2026-11-19): the hook dropped from `Server.call` (wall measured
/// again) -> "FAIL: build: wall time" (red); restored, green. The refusal
/// dropped from argument parsing -> the run without stub binaries is not
/// refused (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn speed_stub_verdicts_ignore_the_hosts_wall_time() {
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(dir.path(), "1.0", &[], &[("STUB_HEAD_SLEEP_S", "1")]);
    assert_eq!(
        code,
        Some(0),
        "a stretched wall time failed a stub run: {text}"
    );
    assert!(
        text.contains("test hooks set") && text.contains("CDO_SPEED_TEST_WALL_IS_CPU=1"),
        "{text}"
    );

    // The control: the stub clock off (an empty value removes it). 3 s, not 1:
    // the ratio (0.4k + 3) / 0.4k stays over 1.6 until load stretches the 0.4 s
    // burn twelvefold (k = 12.5), where 1 s gave out at k = 4.2 (review, Fable).
    let off = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        off.path(),
        "1.0",
        &[],
        &[
            ("STUB_HEAD_SLEEP_S", "3"),
            ("CDO_SPEED_TEST_WALL_IS_CPU", ""),
        ],
    );
    // TLS-111: the host's wall time makes the two passes' ratios differ (x7.97
    // against x5.70 under load), so an extra pair may follow and still disagree:
    // HOST UNSTABLE, exit 2, which prints the wall failure too. Never a pass.
    assert!(code == Some(1) || code == Some(2), "{text}");
    assert!(text.contains("FAIL: build: wall time"), "{text}");

    // Never a real run: a value but `1`, or no stub binaries.
    let other = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(
        other.path(),
        "1.0",
        &[],
        &[("CDO_SPEED_TEST_WALL_IS_CPU", "0")],
    );
    assert_eq!(code, Some(2), "{text}");
    assert!(
        text.contains("CDO_SPEED_TEST_WALL_IS_CPU is a test hook") && !text.contains("outputs in"),
        "refused before anything starts: {text}"
    );
    // One stub binary only (both are needed), and `--no-build` into a private
    // target: a refusal that regressed ends at "no binary at", never in cargo.
    let out = crate::clean_env::command("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(dir.path().join("ws"))
        .arg("--base-exe")
        .arg(dir.path().join("alsem"))
        .arg("--no-build")
        .env("CDO_SPEED_TEST_WALL_IS_CPU", "1")
        .env("CARGO_TARGET_DIR", dir.path().join("target"))
        .env("ALSEM_SPEED_CACHE", dir.path().join("cache"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(
        text.contains("only with --base-exe and --head-exe"),
        "{text}"
    );
}

/// TLS-65 and TLS-79: the warm-up is timed twice, on fresh server pairs, the
/// second pass with every call's side order flipped, and judged on the
/// geometric mean of the two ratios (TLS-65); and each side's model build is
/// timed with the other side's server closed (TLS-79). The stub's model build
/// is 50 % slower while another server is alive (the measured cause) and its
/// test universe 50 % slower when second in its pair (a positional factor).
/// - One pass: the build reads ~x1.00 (no other server alive while it is
///   timed); warm-tests shows the whole positional factor, x0.67 (base is
///   second for it in pass 1, its order flipped per call);
/// - two passes: the build ~x1.00 in each; warm-tests one ratio each way (> 1.3
///   and < 0.8) with exactly their geometric mean, near 1.
///
/// Mutations: the second pass not flipped -> warm-tests x1.50 both times ->
/// FAIL; the warm-up judged on one pass (`round=1`) -> one ratio (Fable r1);
/// TLS-79 (2026-11-16): the original scheduling restored (both servers started,
/// then `run_round(0)` with the build first: the second side builds while the
/// first holds its model) -> the one-pass build reads x1.50 and fails.
#[cfg(target_os = "linux")]
#[test]
fn speed_warm_up_cancels_the_second_side_penalty() {
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = speed_with_stub(dir.path(), "0.5", &["--warm-samples", "1"], &[]);
    assert!(code == Some(0) || code == Some(1), "{text}");
    assert!(!text.contains("FAIL: build: CPU"), "{text}");
    let one: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("state---warm-samples-1/out/speed.json")).unwrap(),
    )
    .unwrap();
    let row = |call: &str| {
        one["warm_rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["call"] == call)
            .unwrap_or_else(|| panic!("{call} in warm_rows"))["cpu"]["ratio"]
            .as_f64()
            .unwrap()
    };
    // The precondition: one pass shows the whole positional factor (x1/1.5).
    let t = row("warm-tests");
    assert!(
        (0.6..0.73).contains(&t),
        "precondition: one pass reads warm-tests at ~x0.67: x{t}"
    );
    let r = row("build");
    assert!(
        (0.9..1.1).contains(&r),
        "the build is timed with no other server alive: x{r}"
    );

    let (code, text) = speed_with_stub(dir.path(), "0.5", &[], &[]);
    // The CPU judgment is the deterministic part (the stub burns exact CPU);
    // only a CPU failure may not appear. (Wall equals CPU here under the stub
    // clock, TLS-103; it was the host's when sol r1 wrote this.)
    assert!(
        !text.contains("FAIL: build: CPU") && !text.contains("FAIL: warm-tests: CPU"),
        "two flipped samples pass the CPU limit: {text}"
    );
    assert!(code == Some(0) || code == Some(1), "{text}");
    assert!(text.contains("by pass; load 0.5-0.5)"), "{text}");
    assert!(
        text.contains("head builds first"),
        "the second pass flips: {text}"
    );
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("state-/out/speed.json")).unwrap(),
    )
    .unwrap();
    for call in ["build", "warm-tests"] {
        let row = json["warm_rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["call"] == call)
            .unwrap_or_else(|| panic!("{call} in warm_rows"));
        let ratios: Vec<f64> = row["cpu"]["ratios"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect();
        assert_eq!(ratios.len(), 2, "{call}: one ratio per pass: {ratios:?}");
        let hi = ratios.iter().cloned().fold(f64::MIN, f64::max);
        let lo = ratios.iter().cloned().fold(f64::MAX, f64::min);
        if call == "build" {
            assert!(
                hi < 1.1 && lo > 0.9,
                "build: no penalty in either pass: {ratios:?}"
            );
        } else {
            assert!(
                hi > 1.3 && lo < 0.8,
                "{call}: one pass each way: {ratios:?}"
            );
        }
        let ratio = row["cpu"]["ratio"].as_f64().unwrap();
        assert!(
            (0.9..1.1).contains(&ratio),
            "{call}: the geometric mean cancels: {ratio}"
        );
        // Exactly the geometric mean, for CPU and wall (sol r1: the plain
        // mean, x1.083 here, would also fall in the range above).
        for m in ["cpu", "wall"] {
            let rs: Vec<f64> = row[m]["ratios"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap())
                .collect();
            let want = (rs[0] * rs[1]).sqrt();
            let got = row[m]["ratio"].as_f64().unwrap();
            assert!(
                (got - want).abs() < 1e-9,
                "{call} {m}: ratio {got}, geometric mean of {rs:?} is {want}"
            );
        }
    }
}

/// TLS-76 (replaces TLS-65's stop on a loaded machine). Admission waits on the
/// host's pressure (avg60); the label comes from the stall share this container's
/// own tasks saw over each timed block (`some total=`). Three cases, from
/// hand-written PSI files:
/// - the host stays at 70 % for the whole wait: the run goes ahead (never
///   skipped, never exit 2), the verdict says "UNDER LOAD" naming both passes,
///   speed.json lists them, and cdo-gate's summary shows it on the speed row;
/// - the host is calm but a writer thread grows the container's stall total at
///   ~80 % of wall time: the label names the blocks whose share passed 25 %
///   (reviews r1, Fable and sol: a pre-pass reading alone sees an idle cgroup);
/// - calm (both files flat at 1 %), with a 1-min load of 50 (TLS-65 refused
///   that): no label.
///
/// Mutations (2026-11-15): `wait_calm`'s timeout returning "not under load" ->
/// no label in case 1; `under_load_label` ignoring the stall shares -> no label
/// in case 2.
#[cfg(target_os = "linux")]
#[test]
fn speed_runs_under_load_and_says_so() {
    // 1. The host stays loaded.
    let dir = tempfile::tempdir().unwrap();
    let host = dir.path().join("host");
    std::fs::write(&host, "some avg10=80.00 avg60=70.00 avg300=60.00 total=0\n").unwrap();
    let (code, text) = speed_with_stub(
        dir.path(),
        "50.0",
        &[],
        &[
            ("CDO_SPEED_HOST_PRESSURE_FILE", host.to_str().unwrap()),
            ("CDO_SPEED_CALM_WAIT_S", "1"),
            ("CDO_SPEED_CALM_POLL_S", "0.2"),
        ],
    );
    let verdict_of = |text: &str| -> String {
        text.lines()
            .rfind(|l| l.contains("cdo-speed: PASS") || l.contains("cdo-speed: FAIL"))
            .unwrap_or_else(|| panic!("a verdict line: {text}"))
            .to_string()
    };
    let verdict = verdict_of(&text);
    assert!(
        verdict.contains(
            "UNDER LOAD: host pressure above 25.0 % for 1 s before warm-up pass(es) 1, 2"
        ),
        "{text}"
    );
    assert!(code == Some(0) || code == Some(1), "{text}");
    assert!(text.contains("round 0"), "the warm-up ran: {text}");
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("state-/out/speed.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        json["admitted_by_timeout"],
        serde_json::json!([1, 2]),
        "{json}"
    );
    assert_eq!(json["under_load"], true, "{json}");

    // cdo-gate's summary of such a run: the label is on the speed row.
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("speed.status"), "0 600\n").unwrap();
    std::fs::write(logs.join("speed.log"), text.as_bytes()).unwrap();
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg(format!("--summary={}", logs.display()))
        .output()
        .unwrap();
    let summary = String::from_utf8_lossy(&out.stdout);
    let speed_row = summary
        .lines()
        .find(|l| l.starts_with("cdo-gate:   speed "))
        .unwrap_or_else(|| panic!("a speed row: {summary}"));
    assert!(speed_row.contains("UNDER LOAD"), "{speed_row}");

    // 2. The host is calm; this container's own tasks stall ~80 % of the time.
    let stalled = tempfile::tempdir().unwrap();
    let own = stalled.path().join("own");
    std::fs::write(&own, "some avg10=1.00 avg60=1.00 avg300=1.00 total=0\n").unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (own, stop) = (own.clone(), stop.clone());
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let us = (t0.elapsed().as_micros() as f64 * 0.8) as u64;
                let tmp = own.with_extension("tmp");
                std::fs::write(
                    &tmp,
                    format!("some avg10=1.00 avg60=1.00 avg300=1.00 total={us}\n"),
                )
                .unwrap();
                std::fs::rename(&tmp, &own).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })
    };
    let (_, text) = speed_with_stub(
        stalled.path(),
        "50.0",
        &[],
        &[("CDO_SPEED_PRESSURE_FILE", own.to_str().unwrap())],
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().unwrap();
    let verdict = verdict_of(&text);
    assert!(
        // The warm-up passes only: the stub's rounds may end between two writer
        // updates (20 ms) and read 0 % (review r2, sol).
        verdict.contains("UNDER LOAD: stall share above 25.0 % in warm-up 1, warm-up 2"),
        "{text}"
    );
    assert!(!verdict.contains("host pressure"), "{verdict}");

    // 3. Calm (1 %), with the same 1-min load of 50: no label.
    let calm = tempfile::tempdir().unwrap();
    let (_, text) = speed_with_stub(calm.path(), "50.0", &[], &[]);
    assert!(!text.contains("UNDER LOAD"), "{text}");
    assert!(
        text.contains("cdo-speed: PASS") || text.contains("cdo-speed: FAIL"),
        "{text}"
    );
}

/// ⟨TLS-108⟩ `ci-steps perf-bounds` with a stand-in test binary (`body`, a bash
/// script it runs instead of building `perf_bounds`) on a private slot, plus
/// `extra` environment. Returns (exit code, stdout+stderr).
#[cfg(target_os = "linux")]
fn perf_bounds_step(
    dir: &std::path::Path,
    body: &str,
    extra: &[(&str, &std::ffi::OsStr)],
) -> (Option<i32>, String) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("perf-bounds-stand-in");
    // ⟨TLS-113⟩ The step first asks the binary for its tests (`--list`): the
    // stand-in lists as many `release_checks::` tests as tests/perf_bounds.rs
    // declares (STUB_LISTED overrides it) and STUB_IGNORED ignored ones.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/perf_bounds.rs"))
        .unwrap();
    let declared = src
        .split_once("\nmod release_checks")
        .expect("tests/perf_bounds.rs has `mod release_checks`")
        .1
        .lines()
        .filter(|l| l.trim() == "#[test]")
        .count();
    std::fs::write(
        &bin,
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = --list ]; then\n\
             case \" $* \" in *\" --ignored \"*) n=${{STUB_IGNORED:-0}} ;; *) n=${{STUB_LISTED:-{declared}}} ;; esac\n\
             for ((i = 0; i < n; i++)); do echo \"release_checks::t$i: test\"; done; exit 0\n\
             fi\n{body}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = crate::clean_env::command("bash");
    private_slot(&mut cmd, dir)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ci-steps"))
        .arg("perf-bounds")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CI_STEPS_PERF_BOUNDS_BIN", &bin);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn ci-steps perf-bounds");
    (
        out.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// The stand-in body that prints the slot's status from inside the timed run.
#[cfg(target_os = "linux")]
fn status_from_inside() -> String {
    // `status` exits 1 while the slot is held; the stand-in's exit is the step's.
    format!(
        "bash '{}/scripts/cdo-slot' status || true",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// ⟨TLS-108⟩ The perf-bounds step times its test binary under a TIMED slot hold:
/// with no hold to share it takes the slot itself, marks it timed, then runs the
/// binary (which sees the timed mark: the status it prints from inside says
/// "timed by perf-bounds since"), and prints the timing line. The binary's exit
/// is the step's.
///
/// Mutation (2026-11-20): the `cdo-slot acquire ... --timed` call dropped from
/// `run_perf_bounds` -> no "timed by perf-bounds" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_runs_under_a_timed_slot_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (code, text) = perf_bounds_step(dir.path(), &status_from_inside(), &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("cdo-slot: taken by perf-bounds (perf-bounds timing)")
            && text.contains("cdo-slot: timed by perf-bounds (perf-bounds timing)"),
        "{text}"
    );
    assert!(
        text.contains("timed by perf-bounds since"),
        "the binary ran inside the timed hold: {text}"
    );
    assert!(
        text.contains("ci-steps: perf-bounds timing: ") && text.contains("exit 0;"),
        "{text}"
    );
    let (code, text) = perf_bounds_step(dir.path(), "exit 3", &[]);
    assert_eq!(code, Some(3), "the binary's exit is the step's: {text}");
}

/// ⟨TLS-113⟩ The step fails, before it takes the slot or runs anything, when
/// the binary does not hold every timing test: built with debug assertions
/// (an inherited `CARGO_PROFILE_RELEASE_FAST_DEBUG_ASSERTIONS=true` compiles
/// `release_checks` out, and the two tests left passed; review r2, sol), one
/// short, or a test ignored. A real dev-profile binary lists 0 of the 12 and a
/// real release-fast one all 12
/// (`/work/.al-sem/tools-notes/tls113/r3-perf-bounds-real-binaries.log`).
///
/// Proof (2026-11-21): the `perf_bounds_incomplete` call removed from
/// `run_perf_bounds` -> FAILED (the stand-in listing 0 ran and passed);
/// restored -> passed.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_refuses_a_binary_without_its_timing_tests() {
    let dir = tempfile::tempdir().unwrap();
    for (env, says) in [
        (
            ("STUB_LISTED", "0"),
            "the timing tests would not run: the binary has 0 of the ",
        ),
        (("STUB_LISTED", "11"), "the binary has 11 of the "),
        (("STUB_IGNORED", "1"), "1 of its tests are ignored"),
    ] {
        let (code, text) = perf_bounds_step(
            dir.path(),
            "echo stand-in ran",
            &[(env.0, std::ffi::OsStr::new(env.1))],
        );
        assert_eq!(code, Some(1), "{env:?}: {text}");
        assert!(text.contains(says), "{env:?}: {text}");
        assert!(!text.contains("stand-in ran"), "{env:?}: {text}");
        assert!(!text.contains("cdo-slot: taken"), "{env:?}: {text}");
    }
}

/// ⟨TLS-108⟩ A build still running when the timing is to begin: the step times
/// nothing (the stand-in binary never runs) and fails. The stand-in build is
/// bash through a symlink named `t108b<test pid>`, matched by cdo-slot's
/// `pgrep -x` (CDO_SLOT_BUILD_NAMES).
///
/// Mutation (2026-11-20): exit 6 accepted like 0 (the `6)` arm merged into
/// `0 | 4`) -> the binary runs beside the build (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_times_nothing_beside_a_build() {
    let dir = tempfile::tempdir().unwrap();
    let fake_name = format!("t108b{}", std::process::id());
    let fake = dir.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let build = crate::cdo_slot::Reap(
        crate::clean_env::command(&fake)
            .args(["-c", "read -t 600 _ || true"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ran = dir.path().join("ran");
    let (code, text) = perf_bounds_step(
        dir.path(),
        &format!("touch '{}'", ran.display()),
        &[
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_ref()),
            ("CDO_SLOT_TIMED_WAIT_S", "1".as_ref()),
        ],
    );
    drop(build);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("timed nothing"), "{text}");
    assert!(
        text.contains(&format!("({fake_name})")),
        "the refusal names the build: {text}"
    );
    assert!(!ran.exists(), "the binary ran beside a build: {text}");
}

/// ⟨TLS-108⟩ Where no slot can be held (a GitHub Actions runner: CI runs this
/// step), the step runs the binary with no hold and says so (review r1, Fable and
/// sol: holding it there failed or hung CI).
///
/// Mutation (2026-11-20): `perf_bounds_no_slot` always empty -> the hold is
/// taken on the runner ("taken by perf-bounds", red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_holds_no_slot_on_a_ci_runner() {
    let dir = tempfile::tempdir().unwrap();
    let ran = dir.path().join("ran");
    let (code, text) = perf_bounds_step(
        dir.path(),
        &format!("touch '{}'", ran.display()),
        &[("GITHUB_ACTIONS", "true".as_ref())],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(ran.exists(), "{text}");
    assert!(
        text.contains("(no CDO slot held: a GitHub Actions runner)"),
        "{text}"
    );
    assert!(!text.contains("taken by perf-bounds"), "{text}");
}

/// ⟨TLS-108⟩ Inside a hold (land's token) the step shares it: it marks it timed
/// without taking the slot, and the binary runs inside the timed mark.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_shares_the_hold_it_runs_in() {
    let dir = tempfile::tempdir().unwrap();
    let holder = crate::cdo_slot::Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut acquire = crate::clean_env::command("bash");
    let held = private_slot(&mut acquire, dir.path())
        .args([
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot"),
            "acquire",
            "land",
            "test landing",
            "--bind",
        ])
        .arg(holder.0.id().to_string())
        .env("CDO_SLOT_TOKEN", "tok-108")
        .output()
        .unwrap();
    assert!(held.status.success(), "{held:?}");
    let (code, text) = perf_bounds_step(
        dir.path(),
        &status_from_inside(),
        &[("CDO_SLOT_TOKEN", "tok-108".as_ref())],
    );
    drop(holder);
    assert_eq!(code, Some(0), "{text}");
    assert!(!text.contains("taken by perf-bounds"), "it shares: {text}");
    assert!(text.contains("timed by perf-bounds since"), "{text}");
}

/// ⟨TLS-108⟩ A `ci-steps` parked in `acquire --wait` (a lane's own gate queued
/// behind a landing) is not a running build: the landing's timed step inside its
/// hold does not wait for it (review r1, Fable and sol: the two waited on each
/// other until CDO_SLOT_TIMED_WAIT_S, then the landing failed). The parked script
/// is named `t108p<test pid>` and listed as a build script, so only the fix in
/// `foreign_builds` (ancestors of a queued waiter are skipped) lets the timing go.
///
/// Mutation (2026-11-20): the `parked` skip dropped from `foreign_builds` -> the
/// step is refused, exit 1, "timed nothing" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_does_not_wait_for_a_parked_gate() {
    let dir = tempfile::tempdir().unwrap();
    let slot = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
    let holder = crate::cdo_slot::Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut acquire = crate::clean_env::command("bash");
    let held = private_slot(&mut acquire, dir.path())
        .args([slot, "acquire", "land", "test landing", "--bind"])
        .arg(holder.0.id().to_string())
        .env("CDO_SLOT_TOKEN", "tok-108p")
        .output()
        .unwrap();
    assert!(held.status.success(), "{held:?}");
    let script_name = format!("t108p{}", std::process::id());
    let script = dir.path().join(&script_name);
    std::fs::write(
        &script,
        format!("bash '{slot}' acquire lane 'queued gate' --wait --bind $$\nsleep 600\n"),
    )
    .unwrap();
    let mut park = crate::clean_env::command("bash");
    let parked = crate::cdo_slot::Reap(
        private_slot(&mut park, dir.path())
            .arg(&script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Its ticket in the queue: it is parked.
    let queue = dir.path().join("slot").join("queue");
    let start = std::time::Instant::now();
    while std::fs::read_dir(&queue).map_or(true, |mut d| d.next().is_none()) {
        assert!(start.elapsed().as_secs() < 30, "the waiter never queued");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let (code, text) = perf_bounds_step(
        dir.path(),
        "true",
        &[
            ("CDO_SLOT_TOKEN", "tok-108p".as_ref()),
            ("CDO_SLOT_BUILD_SCRIPTS", script_name.as_ref()),
            ("CDO_SLOT_TIMED_WAIT_S", "2".as_ref()),
        ],
    );
    drop(parked);
    drop(holder);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("timed by perf-bounds"), "{text}");
}

/// The stall-total line of a cpu.pressure file.
#[cfg(target_os = "linux")]
fn pressure_line(total: u64) -> String {
    format!(
        "some avg10=0.00 avg60=0.00 avg300=0.00 total={total}\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"
    )
}

/// ⟨TLS-108⟩ The step's run with the stall total moved by `stalled_us` while the
/// stand-in sleeps `run_s` s, and the suspended reading moved by `slept_s`.
#[cfg(target_os = "linux")]
fn perf_bounds_with_load(stalled_us: u64, slept_s: u64, run_s: u64) -> (Option<i32>, String) {
    let dir = tempfile::tempdir().unwrap();
    let pressure = dir.path().join("cpu.pressure");
    let suspended = dir.path().join("suspended");
    std::fs::write(&pressure, pressure_line(1000)).unwrap();
    std::fs::write(&suspended, "100").unwrap();
    let body = format!(
        "sleep {run_s}\nprintf '%s' '{}' > '{}'\necho {} > '{}'",
        pressure_line(1000 + stalled_us),
        pressure.display(),
        100 + slept_s,
        suspended.display()
    );
    perf_bounds_step(
        dir.path(),
        &body,
        &[
            ("CI_STEPS_CPU_PRESSURE_FILE", pressure.as_os_str()),
            ("CDO_HOST_SUSPENDED_FILE", suspended.as_os_str()),
        ],
    )
}

/// ⟨TLS-108⟩ A run under load is labelled, never hidden and never excused: 600 ms
/// stalled in about 1 s is above 15 % (UNDER LOAD); 10 ms is below (the share is
/// printed, no label); the binary's exit (0) stays the verdict either way.
///
/// Mutation (2026-11-20): the share compared the wrong way (`<`) -> no "UNDER
/// LOAD" at 600 ms (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_labels_load() {
    let (code, text) = perf_bounds_with_load(600_000, 0, 1);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("exit 0; UNDER LOAD: this cgroup's CPU stall share was "),
        "{text}"
    );
    let (code, text) = perf_bounds_with_load(10_000, 0, 1);
    assert_eq!(code, Some(0), "{text}");
    assert!(!text.contains("UNDER LOAD"), "{text}");
    assert!(text.contains("(this cgroup's CPU stall share "), "{text}");
}

/// ⟨TLS-108⟩ A suspend during the run is labelled SUSPENDED (as cdo-speed labels
/// it), and the stall share is of AWAKE time: 250 ms stalled in a 2 s run with
/// 1 s asleep is 25 % of the awake second (UNDER LOAD), though only ~12 % of the
/// wall time. A suspend covering the whole run says so instead of a share.
///
/// Mutation (2026-11-20): the share taken over wall time (`awake_us` =
/// `wall_us`) -> no "UNDER LOAD" (red); restored, green.
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_labels_a_suspend() {
    let (code, text) = perf_bounds_with_load(250_000, 1, 2);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("SUSPENDED: the host slept 1 s during the timing"),
        "{text}"
    );
    assert!(text.contains("UNDER LOAD: "), "share of awake time: {text}");
    let (code, text) = perf_bounds_with_load(0, 5, 1);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("(load not checked: the host slept through the run)")
            && text.contains("SUSPENDED: the host slept 5 s during the timing"),
        "{text}"
    );
}

/// ⟨TLS-108⟩ The parked skip cannot hide a real build (review r2, sol and Fable):
/// - a queue ticket whose PID is alive but whose start time is not that
///   process's (a dead waiter's reused PID) parks nothing: the build script
///   under that PID still counts;
/// - a parked waiter whose own process has an exact build NAME (as cargo or
///   rustc would) still counts: only build scripts can be parked.
///
/// Both refuse the timing (exit 1, "timed nothing").
///
/// Mutations (2026-11-20, each restored, green): the ticket's start time not
/// checked -> the first case times (red); the script-only condition dropped ->
/// the second case times (red).
#[cfg(target_os = "linux")]
#[test]
fn perf_bounds_parked_skip_hides_no_real_build() {
    let dir = tempfile::tempdir().unwrap();
    let slot = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
    let holder = crate::cdo_slot::Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut acquire = crate::clean_env::command("bash");
    let held = private_slot(&mut acquire, dir.path())
        .args([slot, "acquire", "land", "test landing", "--bind"])
        .arg(holder.0.id().to_string())
        .env("CDO_SLOT_TOKEN", "tok-108s")
        .output()
        .unwrap();
    assert!(held.status.success(), "{held:?}");

    // 1. A live build script under a forged ticket with the wrong start time.
    let script_name = format!("t108s{}", std::process::id());
    let script = dir.path().join(&script_name);
    std::fs::write(&script, "read -t 600 _ || true\n").unwrap();
    let live = crate::cdo_slot::Reap(
        crate::clean_env::command("bash")
            .arg(&script)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let queue = dir.path().join("slot").join("queue");
    std::fs::create_dir_all(&queue).unwrap();
    let forged = queue.join(format!("00000000000000000001.{}", live.0.id()));
    std::fs::write(&forged, format!("{} 1 \n", live.0.id())).unwrap();
    let (code, text) = perf_bounds_step(
        dir.path(),
        "true",
        &[
            ("CDO_SLOT_TOKEN", "tok-108s".as_ref()),
            ("CDO_SLOT_BUILD_SCRIPTS", script_name.as_ref()),
            ("CDO_SLOT_TIMED_WAIT_S", "1".as_ref()),
        ],
    );
    drop(live);
    let _ = std::fs::remove_file(&forged);
    assert_eq!(code, Some(1), "a stale ticket parked a live script: {text}");
    assert!(text.contains(&format!("({script_name})")), "{text}");

    // 2. A parked waiter whose own process has an exact build name.
    let fake_name = format!("t108n{}", std::process::id());
    let fake = dir.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let mut park = crate::clean_env::command(&fake);
    let parked = crate::cdo_slot::Reap(
        private_slot(&mut park, dir.path())
            .args([
                "-c",
                &format!("bash '{slot}' acquire lane 'queued' --wait --bind $$; sleep 600"),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Its ticket in the queue (the forged one is gone): it is parked.
    let start = std::time::Instant::now();
    while std::fs::read_dir(&queue).map_or(true, |mut d| d.next().is_none()) {
        assert!(start.elapsed().as_secs() < 30, "the waiter never queued");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let (code, text) = perf_bounds_step(
        dir.path(),
        "true",
        &[
            ("CDO_SLOT_TOKEN", "tok-108s".as_ref()),
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_ref()),
            ("CDO_SLOT_TIMED_WAIT_S", "1".as_ref()),
        ],
    );
    drop(parked);
    drop(holder);
    assert_eq!(code, Some(1), "a parked build name was skipped: {text}");
    assert!(text.contains(&format!("({fake_name})")), "{text}");
}

/// Set in the child `scripts_see_none_of_the_callers_environment` starts.
#[cfg(target_os = "linux")]
const ENV_PROBE: &str = "TLS115_ENV_PROBE";

/// ⟨TLS-115⟩ The scripts these tests run see none of the caller's environment
/// (`crate::clean_env`): this test runs `env_probe_child` again in a child of
/// this test binary with a polluted environment (as a landing's or a
/// developer's shell would be): `CDO_SLOT_OWNER=leaked`,
/// `CDO_GATE_LANE2_CPUS=99` and a `BASH_ENV` that appends to `<itself>.ran`
/// (its own path from `BASH_SOURCE`: no path written into shell source).
/// The child first shows the marker works (a plain `bash -c true` with the
/// inherited environment writes it: the positive control), removes it, then
/// runs two script calls the way the tests here do: `ci-steps perf-bounds`
/// (its slot owner must be its own `perf-bounds`) and `cdo-gate --layout` on a
/// fake host of 22 equal CPUs with a stub `taskset` first on PATH (so it runs
/// on any host; lane 2 must get the default last 6, not "any" from a count of
/// 99). Last, no bash may have read the `BASH_ENV`.
///
/// Proofs (2026-11-23), each one name added to `crate::clean_env`'s pass list
/// so only its own assert can go red, restored -> passed:
/// - `"BASH_ENV"` -> FAILED only at the marker assert ("a script's bash read
///   the caller's BASH_ENV");
/// - `"CDO_SLOT_OWNER"` -> FAILED at "the caller's CDO_SLOT_OWNER reached
///   ci-steps" (taken by leaked);
/// - `"CDO_GATE_LANE2_CPUS"` -> FAILED at "the caller's CDO_GATE_LANE2_CPUS
///   reached cdo-gate" (lane 2 on CPUs any).
///
/// And a real test: `speed_gate_fails_when_a_timed_call_fails` with its
/// `crate::clean_env::command("python3")` made a plain `Command::new` (its form
/// before TLS-115) and `CDO_SLOT_OWNER=leaked` exported -> FAILED ("cdo-slot:
/// taken by leaked (cdo-speed timing)"); restored, it passes.
#[cfg(target_os = "linux")]
#[test]
fn scripts_see_none_of_the_callers_environment() {
    let dir = tempfile::tempdir().unwrap();
    let bash_env = dir.path().join("bash-env");
    std::fs::write(&bash_env, "echo leaked >> \"${BASH_SOURCE[0]}.ran\"\n").unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "gate_scripts::env_probe_child",
            "--exact",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(ENV_PROBE, "1")
        .env("CDO_SLOT_OWNER", "leaked")
        .env("CDO_GATE_LANE2_CPUS", "99")
        .env("BASH_ENV", &bash_env)
        .output()
        .expect("spawn the test binary");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "the child ran the probe: {text}");
    assert!(text.contains("env probe: ran"), "{text}");
}

/// The child half of `scripts_see_none_of_the_callers_environment`; nothing
/// outside it (no `TLS115_ENV_PROBE`).
#[cfg(target_os = "linux")]
#[test]
fn env_probe_child() {
    if std::env::var_os(ENV_PROBE).is_none() {
        return;
    }
    let bash_env = std::env::var_os("BASH_ENV").expect("the parent sets BASH_ENV");
    let mut marker = bash_env.clone();
    marker.push(".ran");
    let marker = std::path::PathBuf::from(marker);
    let dir = tempfile::tempdir().unwrap();

    // Positive control: bash with the fixture's BASH_ENV writes the marker. A
    // clean environment plus that one variable: an inherited POSIXLY_CORRECT
    // makes bash skip BASH_ENV (review r2, sol).
    let control = crate::clean_env::command("bash")
        .env("BASH_ENV", &bash_env)
        .args(["-c", "true"])
        .status()
        .unwrap();
    assert!(control.success());
    assert!(marker.exists(), "control: the BASH_ENV wrote no marker");
    std::fs::remove_file(&marker).unwrap();

    let (code, text) = perf_bounds_step(dir.path(), &status_from_inside(), &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("cdo-slot: taken by perf-bounds (perf-bounds timing)"),
        "the caller's CDO_SLOT_OWNER reached ci-steps: {text}"
    );

    // `--layout` only asks whether `taskset` exists (`command -v`): a stub
    // makes the layout half run on a host without one.
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("taskset"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(bin.join("taskset"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let sys = dir.path().join("sys");
    for c in 0..22 {
        let f = sys.join(format!("cpu{c}/cpufreq"));
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("cpuinfo_max_freq"), "4800000\n").unwrap();
    }
    std::fs::write(sys.join("online"), "0-21\n").unwrap();
    std::fs::write(
        sys.join("status"),
        "Name:\tbash\nCpus_allowed_list:\t0-21\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("meminfo"), "MemAvailable: 62914560 kB\n").unwrap();
    std::fs::write(dir.path().join("memory.max"), "max\n").unwrap();
    let out = crate::clean_env::command("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg("--layout")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", dir.path())
        .env("CDO_GATE_MEMINFO", dir.path().join("meminfo"))
        .env("CDO_GATE_CGROUP", dir.path())
        .env("CDO_GATE_LAYOUT", "parallel")
        .env("CDO_GATE_PROC_STATUS", sys.join("status"))
        .env("CDO_GATE_SYS_CPU", &sys)
        .output()
        .expect("spawn bash scripts/cdo-gate --layout");
    let got = String::from_utf8_lossy(&out.stdout);
    assert!(
        got.contains("lane 2 on CPUs 16,17,18,19,20,21 "),
        "the caller's CDO_GATE_LANE2_CPUS reached cdo-gate: {got}"
    );
    assert!(
        !marker.exists(),
        "a script's bash read the caller's BASH_ENV: {:?}",
        std::fs::read_to_string(&marker)
    );
    println!("env probe: ran");
}
