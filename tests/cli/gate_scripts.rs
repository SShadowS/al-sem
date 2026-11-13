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
        .env("ALSEM_SPEED_CACHE", out.path().join("cache"))
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
        let r = std::process::Command::new("python3")
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
    let run = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-speed"))
        .arg(ws.path())
        .args(["--build-only", "--base-exe"])
        .arg(&base)
        .arg("--out")
        .arg(target.path().join("out"))
        .env("CARGO_TARGET_DIR", target.path())
        .env("ALSEM_SPEED_CACHE", target.path().join("speed-cache"))
        .env("PATH", path)
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

/// TLS-32: the test phases' layout (`cdo-gate --layout` prints what
/// `plan_layout` decides, the decision the gate makes after `output_cdo`).
/// Lane 2 runs beside the impact phase at the most threads (2, then 1) whose
/// need (impact's own worst 16,100 + lib_cdo's own worst at that count, 23,300
/// or 18,700, + 4,096 MiB) fits BOTH the host's MemAvailable and the
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
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", case.join("meminfo"))
            .env("CDO_GATE_CGROUP", &case)
            .env("CDO_GATE_LAYOUT", "parallel")
            .env_remove("CDO_GATE_SERIAL");
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
    let two = "parallel: lane 2 beside the impact phase, lib_cdo at 2 thread(s)";
    let one = "parallel: lane 2 beside the impact phase, lib_cdo at 1 thread(s)";
    // Needs: 2 threads 43,496 MiB, 1 thread 38,896 MiB.
    check(60_000, Some(2_000), false, two); // container free 47,152
    check(60_000, Some(8_000), false, one); // container free 41,152
    check(60_000, Some(12_000), false, "sequential:"); // container free 37,152
    check(41_000, Some(2_000), false, one); // the host is short for 2
    check(30_000, Some(2_000), false, "sequential:"); // the host is short for 1
    check(60_000, None, false, two); // no container limit: the host decides
    check(60_000, Some(2_000), true, "serial:");
    // An unknown container is no room (review r2, sol: fail closed): a
    // `memory.stat` that cannot be read, or a `memory.max` that is no number.
    let broken = dir.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("memory.max"), format!("{}\n", 49_152 * MIB)).unwrap();
    let meminfo = broken.join("meminfo");
    std::fs::write(&meminfo, "MemAvailable: 62914560 kB\n").unwrap();
    let unknown = |dir: &std::path::Path| {
        let out = std::process::Command::new("bash")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", &meminfo)
            .env("CDO_GATE_CGROUP", dir)
            .env("CDO_GATE_LAYOUT", "parallel")
            .env_remove("CDO_GATE_SERIAL")
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
    check(60_000, Some(49_152 - 43_496), false, two);
    check(60_000, Some(49_152 - 43_496 + 1), false, one);
    check(43_496, Some(2_000), false, two);
    check(43_495, Some(2_000), false, one);
}

/// TLS-32 review r3-r4 (sol): a TERM sent to the gate's PID alone, during its
/// build phase, stops the gate at once (exit 130) and the phase's processes
/// with it. A stub `cargo` first on `PATH` sleeps (as a long build would) and
/// writes its PID; the gate runs `--no-speed` on an empty folder.
///
/// Mutation (2026-11-13): `run_phase` back to a foreground pipeline -> the
/// trap waits for the 60 s build, and the gate misses the 20 s deadline.
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
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 60\n",
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
    let mut gate = std::process::Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
        .arg("--no-speed")
        .arg(ws.path())
        .env("PATH", path)
        .env("CARGO_TARGET_DIR", target.path())
        .env_remove("CDO_GATE_SERIAL")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bash scripts/cdo-gate");
    // The build phase is running when the stub has written its PID.
    let started = std::time::Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the gate never reached its build phase"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let build_pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let gate_pid = gate.id() as i32;
    // SAFETY: plain kill(2) on a child process this test owns.
    assert_eq!(unsafe { libc::kill(gate_pid, libc::SIGTERM) }, 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(s) = gate.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = gate.kill();
            // SAFETY: as above; clean up the stub's sleep.
            unsafe { libc::kill(build_pid, libc::SIGKILL) };
            panic!("the gate did not stop within 20 s of a TERM");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(130));
    std::thread::sleep(std::time::Duration::from_millis(300));
    // SAFETY: signal 0 only probes whether the process exists.
    let alive = unsafe { libc::kill(build_pid, 0) } == 0
        && std::fs::read_to_string(format!("/proc/{build_pid}/stat"))
            .is_ok_and(|s| !s.contains(") Z "));
    assert!(!alive, "the build phase's process survived the gate");
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
    assert!(text.contains("building (once per commit)"), "{text}");
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
    let has_taskset = std::process::Command::new("sh")
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
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", dir.path().join("meminfo"))
            .env("CDO_GATE_CGROUP", dir.path())
            .env("CDO_GATE_LAYOUT", "parallel")
            .env("CDO_GATE_PROC_STATUS", sys.join("status"))
            .env("CDO_GATE_SYS_CPU", sys)
            .env_remove("CDO_GATE_SERIAL")
            .env_remove("CDO_GATE_LANE2_CPUS");
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
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(concat!(
                "logs=\"$1\"; profile=(); lane2_nice=(); lane2_pin=(); lib_threads=2; ",
                "source <(sed -n '/^lib_long_tests=(/,/^)/p; /^lib_tests()/,/^}/p' \"$2\"); ",
                "lib_tests"
            ))
            .arg("bash")
            .arg(dir.path())
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
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
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-gate"))
            .arg("--layout")
            .env("CDO_GATE_MEMINFO", dir.path().join("meminfo"))
            .env("CDO_GATE_CGROUP", dir.path())
            .env_remove("CDO_GATE_SERIAL")
            .env_remove("CDO_GATE_LAYOUT");
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

/// A warm build folder written by hand: one dependency, plus the two folders
/// `scripts/seed-target` leaves out.
fn warm_target(src: &tempfile::TempDir) {
    std::fs::create_dir_all(src.path().join("debug/deps")).unwrap();
    std::fs::write(src.path().join("debug/deps/a.rlib"), b"x").unwrap();
    std::fs::create_dir_all(src.path().join("flycheck0")).unwrap();
    std::fs::create_dir_all(src.path().join("tmp")).unwrap();
}

fn seed_target(wt: &std::path::Path, src: &std::path::Path) -> std::process::Output {
    std::process::Command::new("bash")
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
    let mut child = std::process::Command::new("bash")
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
    let out = std::process::Command::new("bash")
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
