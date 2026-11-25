//! ⟨TLS-59⟩ `scripts/land`: build a landing branch, gate it, push only on
//! request. Every test runs the script on a repository of its own, with stub
//! `scripts/ci-steps`, `scripts/cdo-gate` and `scripts/merge-and-push` that log
//! their calls (and, for ci, run a hook), so nothing real is gated, merged or
//! pushed.
use crate::clean_env::clean_env;
use crate::git;
use std::path::{Path, PathBuf};

const LAND: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/land");

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    main: PathBuf,
    gate: PathBuf,
}

/// Whatever CDO slot holder a test leaves in its slot folder (a failing test,
/// a mutation run) ends with it.
impl Drop for Repo {
    fn drop(&mut self) {
        let holder = std::fs::read_to_string(self.root.join("slot/status"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("holder_pid="))
                    .and_then(|p| p.parse::<i32>().ok())
            });
        if let Some(pid) = holder {
            // SAFETY: SIGKILL to the holder this test's slot started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

impl Repo {
    /// The stubs' calls, the temporary root cut out of each (`CDO_WS` is
    /// `<root>/cdo-ws`, so cdo-gate's call reads `cdo-gate /cdo-ws`).
    fn calls(&self) -> Vec<String> {
        let root = self.root.to_str().unwrap();
        std::fs::read_to_string(self.root.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.replace(root, ""))
            .collect()
    }
    fn reset_calls(&self) {
        let _ = std::fs::remove_file(self.root.join("calls"));
    }
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        String::from_utf8(git::run(dir, args).stdout)
            .unwrap()
            .trim()
            .to_string()
    }
    fn refs(&self) -> String {
        self.git(
            &self.main,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        )
    }
    fn landing_branches(&self) -> Vec<String> {
        self.git(
            &self.main,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/landing/",
            ],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }
    /// `scripts/land --worktree <gate> <args>` with extra environment `env`
    /// (a stub's exit: `("STUB_CI_STEPS_RC", "1")`; ci's hook:
    /// `("STUB_CI_STEPS_HOOK", "<sh>")`); (exit, stdout + stderr).
    fn land(&self, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
        let out = self
            .land_cmd(Path::new(LAND), args, env)
            .output()
            .expect("run scripts/land");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
    /// The command `land` runs, with the land script `script`.
    fn land_cmd(
        &self,
        script: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> std::process::Command {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(script).arg("--worktree").arg(&self.gate).args(args);
        clean_env(&mut cmd)
            // Stub scripts: a HOME of its own (TLS-115).
            .env("HOME", &self.root)
            .env("LAND_LOGS", self.root.join("logs"))
            .env("FLOW_LOG", self.root.join("flow.jsonl"))
            .env("LAND_MEM_MIN_GB", "0")
            // A CDO slot of its own (TLS-60), held for this session.
            .env("CDO_SLOT_DIR", self.root.join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            .env("CLAUDE_CODE_SESSION_ID", "land-test")
            .env("CDO_WS", self.root.join("cdo-ws"))
            .env("STUB_LOG", self.root.join("calls"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }
    /// `scripts/cdo-slot <args>` on this repo's slot, as Claude session
    /// `session` (no Claude PID: never stale); (exit, stdout + stderr).
    fn slot(&self, session: &str, args: &[&str]) -> (Option<i32>, String) {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot"))
            .args(args);
        let out = clean_env(&mut cmd)
            .env("CDO_SLOT_DIR", self.root.join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
    /// A branch from master that writes `files` and commits.
    fn branch(&self, name: &str, files: &[(&str, &[u8])]) {
        git::run(&self.main, &["switch", "-qc", name, "master"]);
        for (path, bytes) in files {
            let path = self.main.join(path);
            // A switch removes a folder its branch emptied.
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        git::run(&self.main, &["add", "-A"]);
        git::run(&self.main, &["commit", "-qm", name]);
        git::run(&self.main, &["switch", "-q", "master"]);
    }
}

fn changelog(entries: &str, old: &str) -> String {
    format!("# Changelog\n\n## [Unreleased]\n\n{entries}### Old\n\n{old}\n")
}

/// A master with the three stub scripts, a CHANGELOG and `a.txt`; branches
/// `tools/tls-58-x` and `tools/tls-59-x` each add a CHANGELOG entry at the
/// same place (a CHANGELOG-only conflict of additions) and a file of their
/// own; `tools/tls-60-bad` changes `a.txt`, which master changes too after
/// it. The gate worktree sits on a detached master.
fn repo() -> Repo {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let main = root.join("main");
    let gate = root.join("gate");
    std::fs::create_dir_all(main.join("scripts")).unwrap();
    std::fs::create_dir_all(root.join("cdo-ws")).unwrap();
    git::run(&main, &["init", "-q", "-b", "master"]);
    for (stub, var) in [
        ("ci-steps", "CI_STEPS"),
        ("cdo-gate", "CDO_GATE"),
        ("merge-and-push", "MERGE_AND_PUSH"),
    ] {
        let path = main.join("scripts").join(stub);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{stub} $*\" >> \"$STUB_LOG\"\n\
                 [ -z \"$STUB_{var}_HOOK\" ] || eval \"$STUB_{var}_HOOK\"\n\
                 exit ${{STUB_{var}_RC:-0}}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(main.join("CHANGELOG.md"), changelog("", "- old")).unwrap();
    std::fs::write(main.join("a.txt"), "base\n").unwrap();
    git::run(&main, &["add", "-A"]);
    git::run(&main, &["commit", "-qm", "base"]);
    let r = Repo {
        _dir: dir,
        root,
        main,
        gate,
    };
    for t in ["58", "59"] {
        let entry = changelog(&format!("### Added (TLS-{t})\n\n- entry {t}\n\n"), "- old");
        r.branch(
            &format!("tools/tls-{t}-x"),
            &[
                ("CHANGELOG.md", entry.as_bytes()),
                (&format!("f{t}.txt"), t.as_bytes()),
            ],
        );
    }
    r.branch("tools/tls-60-bad", &[("a.txt", b"branch\n")]);
    std::fs::write(r.main.join("a.txt"), "master\n").unwrap();
    git::run(&r.main, &["commit", "-qam", "master moves"]);
    git::run(
        &r.main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            r.gate.to_str().unwrap(),
            "master",
        ],
    );
    r
}

/// A batch with a CHANGELOG-only conflict of additions merges (both entries
/// kept, master's first, a blank line between), prints the merge order and a
/// verdict per step, runs ci, mem-wait, then cdo-gate in that order, and
/// pushes nothing without `--push`. A red ci never reaches cdo-gate (the
/// incident behind the task); a mem-wait that runs out never reaches it
/// either; a red cdo-gate never pushes, even with `--push`; a PASS with
/// `--push` calls merge-and-push once, and its failure fails the run. A taken
/// landing name gets `-2`. Every step leaves a start and an end line in the
/// timing log, and a line in the run's `verdict` file.
///
/// Mutations: ci's stop on a failure dropped -> the red-ci run calls
/// cdo-gate; `if ((!push))` made always false -> the plain run pushes; the
/// mem-wait block removed -> the run that cannot get its memory gates on.
#[test]
fn land_gates_in_order_and_pushes_only_on_request() {
    let r = repo();
    let (code, text) = r.land(&["--dry-run", "tools/tls-58-x", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("TLS-59: CHANGELOG.md conflicts: would keep both sides' entries"),
        "{text}"
    );
    assert!(
        r.calls().is_empty(),
        "a dry run gates nothing: {:?}",
        r.calls()
    );

    let (code, text) = r.land(&["tools/tls-58-x", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    for line in [
        "land:   1. tools/tls-58-x (",
        "land:   2. tools/tls-59-x (",
        "land: [merge] ok",
        "land: [ci] ok",
        "land: [mem-wait] ok",
        "land: [cdo] ok",
        "land: PASS: landing/tls58+tls59 is gated",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
    assert_eq!(r.calls(), ["ci-steps all", "cdo-gate /cdo-ws"]);
    assert_eq!(r.landing_branches(), ["landing/tls58+tls59"]);
    assert_eq!(
        r.git(&r.gate, &["log", "--format=%s", "-2"]),
        "landing: TLS-59\nlanding: TLS-58"
    );
    assert_eq!(
        std::fs::read_to_string(r.gate.join("CHANGELOG.md")).unwrap(),
        changelog(
            "### Added (TLS-58)\n\n- entry 58\n\n### Added (TLS-59)\n\n- entry 59\n\n",
            "- old"
        )
    );
    let verdicts: Vec<String> = std::fs::read_dir(r.root.join("logs"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path().join("verdict")).unwrap())
        .collect();
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    let steps: Vec<String> = verdicts[0]
        .lines()
        .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(steps, ["merge 0", "ci 0", "mem-wait 0", "cdo 0"]);

    r.reset_calls();
    let (code, text) = r.land(
        &["tools/tls-58-x", "tools/tls-59-x"],
        &[("STUB_CI_STEPS_RC", "1")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("FAIL at ci: cdo-gate not run"), "{text}");
    assert_eq!(r.calls(), ["ci-steps all"], "a red ci stops the run");
    assert!(
        r.landing_branches()
            .contains(&"landing/tls58+tls59-2".to_string()),
        "a taken name gets -2: {:?}",
        r.landing_branches()
    );

    r.reset_calls();
    let (code, text) = r.land(
        &["tools/tls-58-x"],
        &[("LAND_MEM_MIN_GB", "999999"), ("LAND_MEM_TIMEOUT_S", "0")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("FAIL at mem-wait: cdo-gate not run"),
        "{text}"
    );
    assert_eq!(r.calls(), ["ci-steps all"], "no memory, no cdo-gate");

    r.reset_calls();
    // The gate's own verdict lines are shown under land's (the orchestrator's
    // wish after land's first real run): no need to open cdo.log.
    let (code, text) = r.land(
        &["--push", "tools/tls-58-x"],
        &[
            ("STUB_CDO_GATE_RC", "1"),
            (
                "STUB_CDO_GATE_HOOK",
                "echo 'cdo-gate:   speed   FAIL   build CPU x1.36'; echo 'noise'; echo 'cdo-gate: FAIL'",
            ),
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    assert_eq!(
        r.calls(),
        ["ci-steps all", "cdo-gate /cdo-ws"],
        "a red cdo-gate never pushes"
    );
    assert!(text.contains("land: [cdo] FAILED, exit 1 ("), "{text}");
    assert!(
        text.contains(
            "land:   | cdo-gate:   speed   FAIL   build CPU x1.36\nland:   | cdo-gate: FAIL\n"
        ),
        "the gate's FAIL lines, in order, without its noise: {text}"
    );

    r.reset_calls();
    let (code, text) = r.land(
        &["--push", "tools/tls-59-x"],
        &[("STUB_MERGE_AND_PUSH_RC", "1")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("FAIL at push"), "{text}");

    // merge-and-push leaves a background job behind (its speed-check warm-up);
    // the job must not hold the lock, or the next landing is refused (review
    // r2, Fable). The job lives until this test ends it (sol r1: no timer to
    // beat). merge-and-push is told the master the gates saw.
    struct Kill(PathBuf);
    impl Drop for Kill {
        fn drop(&mut self) {
            if let Some(pid) = std::fs::read_to_string(&self.0)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
            {
                // SAFETY: SIGKILL to the stub's background sleep, by its PID.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
    let holder = Kill(r.root.join("holder.pid"));
    let master = r.git(&r.main, &["rev-parse", "master"]);
    r.reset_calls();
    let hook = format!(
        "echo \"expect $MERGE_EXPECT_MASTER $MERGE_EXPECT_SOURCE\" >> \"$STUB_LOG\"; \
         sleep 600 >/dev/null 2>&1 & echo $! > '{}'",
        holder.0.display()
    );
    let (code, text) = r.land(
        &["--push", "tools/tls-59-x"],
        &[("STUB_MERGE_AND_PUSH_HOOK", &hook)],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        r.calls(),
        [
            "ci-steps all".to_string(),
            "cdo-gate /cdo-ws".to_string(),
            "merge-and-push landing/tls59-2".to_string(),
            format!(
                "expect {master} {}",
                r.git(&r.main, &["rev-parse", "landing/tls59-2"])
            ),
        ]
    );
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(0), "the next landing right after: {text}");
    drop(holder);

    // Three lanes that each add a CHANGELOG entry at the same place: the dry
    // run builds each prediction on the resolved CHANGELOG, so it agrees with
    // the real run (sol r1).
    let entry = changelog("### Added (TLS-57)\n\n- entry 57\n\n", "- old");
    r.branch(
        "tools/tls-57-x",
        &[
            ("CHANGELOG.md", entry.as_bytes()),
            ("f57.txt", b"57".as_slice()),
        ],
    );
    let three = ["tools/tls-57-x", "tools/tls-58-x", "tools/tls-59-x"];
    let (code, text) = r.land(&[&["--dry-run"][..], &three].concat(), &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        text.matches("would keep both sides' entries").count(),
        2,
        "{text}"
    );
    let (code, text) = r.land(&three, &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        std::fs::read_to_string(r.gate.join("CHANGELOG.md")).unwrap(),
        changelog(
            "### Added (TLS-57)\n\n- entry 57\n\n### Added (TLS-58)\n\n- entry 58\n\n\
             ### Added (TLS-59)\n\n- entry 59\n\n",
            "- old"
        )
    );

    let flow = std::fs::read_to_string(r.root.join("flow.jsonl")).unwrap();
    let mut open = std::collections::BTreeSet::new();
    for line in flow.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect(line);
        assert_eq!(v["step"], "land", "{line}");
        let key = format!("{} {}", v["run_id"], v["phase"]);
        match v["event"].as_str() {
            Some("start") => assert!(open.insert(key), "{line}"),
            Some("end") => assert!(v["rc"].is_i64() && open.remove(&key), "{line}"),
            _ => panic!("{line}"),
        }
    }
    assert!(open.is_empty(), "a step with no end: {open:?}");
}

/// ⟨TLS-105⟩ The mem-wait (LAND_MEM_TIMEOUT_S) counts awake time: a host
/// suspend does not use it up (review of TLS-104, Fable). The memory is never
/// enough (999999 GB) and the wait 40 s of awake time; the host "sleeps" 20 s
/// during land's first nap. land says so once (on the terminal and in
/// mem-wait.log), naps 15 s (the cap), 15 s, then the 10 s left, and gives up
/// at the first reading at or past 40 s of awake time.
///
/// ⟨TLS-129⟩ The test sets the whole timeline, so neither load nor when the
/// test thread runs can change it (sol r1). land runs from a folder of links
/// to the real scripts, first on PATH, with three stand-ins on one virtual
/// clock (`clock`: CLOCK_MONOTONIC in us, the suspended seconds):
/// `host-suspended` reports it; land's `sleep` records its argument, moves
/// the clock on by it (no real sleep) and, in the first nap, adds the 20 s
/// suspend; land's MemAvailable `awk` moves it on by `delay_s` (the loop's
/// own work, made certain). The test asserts what land slept and the awake
/// time it gave up at against the values this timeline gives.
///
/// History: the old check, "gives up within 14 s of wall time after the
/// suspend" (a 2 s wait, a 10 s suspend written by the test thread), measured
/// the loop's work and the host's scheduling: it failed at host load 381
/// (14.58 s, TLS-127's task gate) and with a 3 s delay in each clock read
/// after the suspend, land unchanged (15.18 s, 2026-11-25). It is removed.
///
/// Mutation (2026-11-23): the mem-wait on wall time again (`awake_us` plus
/// `awake_slept` seconds) -> gave up 3.1 s after the suspend (red); restored,
/// green. Re-proved on the virtual clock, and the rest, in the next test.
#[test]
fn land_mem_wait_survives_a_host_suspend() {
    land_mem_wait_across_a_suspend(0, &[15, 15, 10], 40);
}

/// ⟨TLS-129⟩ The same wait with 6 s of loop work before each clock reading (a
/// loaded host): each nap is cut to the time left as of the reading before
/// it, so land gives up at the reading after the deadline, 6 s late (46 s):
/// the stated limit, from land's last reading, not a hard wake-up bound.
///
/// Proof (2026-11-25, sol r1), each break in land red in both tests, then
/// restored, green: the deadline shifted (`mem_timeout * 1000000 + 15000000`)
/// -> slept 15, 15, 15, 10 / 15, 15, 7; the mem-wait on wall time (`awake_us`
/// plus `awake_slept` seconds) -> slept 15, 5 / 15; a fixed 15 s sleep
/// (`awake_nap 15000000`) -> 15, 15, 15 / 15, 15; no 15 s cap (`awake_nap
/// $left_us`) -> 40 / 34.
#[test]
fn land_mem_wait_judges_its_naps_not_the_load() {
    land_mem_wait_across_a_suspend(6, &[15, 13], 46);
}

/// The body of the two tests above: a 40 s wait with `delay_s` seconds of loop
/// work before each reading; land must nap `naps_s` and give up at `gave_up_s`
/// of awake time.
fn land_mem_wait_across_a_suspend(delay_s: u64, naps_s: &[u64], gave_up_s: u64) {
    use std::os::unix::fs::PermissionsExt;
    let r = repo();
    let bin = link_scripts(
        &r,
        &[
            "land",
            "changelog-merge",
            "cdo-slot",
            "awake-clock",
            "agentflow",
        ],
    );
    // Stand-ins on the virtual clock `$d/clock` ("<monotonic us> <suspended
    // s>"). `sleep` and `awk` act only for land's own shell (its command line
    // names bin/land; cdo-slot's sleeps are real); `${PATH#*:}` drops `bin`
    // (first on PATH) to reach the real tool.
    let head = "d=$(dirname \"$0\")\nread m s <\"$d/clock\"\n\
                land() { case \"$(tr '\\0' ' ' </proc/$PPID/cmdline)\" in \
                */bin/land\\ *) return 0 ;; esac; return 1; }";
    for (name, body) in [
        (
            "host-suspended",
            "if [ \"${1:-}\" = --clocks ]; then echo \"$s $((m / 1000000 + s)) $m\"; \
             else echo \"$s\"; fi"
                .to_string(),
        ),
        (
            "sleep",
            "if land; then\n\
             echo \"$1\" >>\"$d/sleeps\"\n\
             [ -e \"$d/slept\" ] || { s=$((s + 20)); : >\"$d/slept\"; }\n\
             echo \"$((m + ${1%.*} * 1000000 + 1${1#*.} - 1000000)) $s\" >\"$d/clock\"\n\
             exit 0\nfi\n\
             PATH=${PATH#*:} exec sleep \"$@\""
                .to_string(),
        ),
        (
            "awk",
            format!(
                "case \"$*\" in *MemAvailable*) \
                 ! land || echo \"$((m + {delay_s} * 1000000)) $s\" >\"$d/clock\" ;; esac\n\
                 PATH=${{PATH#*:}} exec awk \"$@\""
            ),
        ),
    ] {
        let stub = bin.join(name);
        std::fs::write(&stub, format!("#!/bin/sh\n{head}\n{body}\n")).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(bin.join("clock"), "1000000000 100\n").unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (code, text) = run_out(r.land_cmd(
        &bin.join("land"),
        &["tools/tls-58-x"],
        &[
            ("LAND_MEM_MIN_GB", "999999"),
            ("LAND_MEM_TIMEOUT_S", "40"),
            ("PATH", &path),
        ],
    ));
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("FAIL at mem-wait"), "{text}");
    let said = "land: mem-wait: the host slept 20 s during this wait: it goes on that much longer";
    assert_eq!(text.matches(said).count(), 1, "{text}");
    assert_eq!(
        text.matches("land: mem-wait: MemAvailable").count(),
        1,
        "{text}"
    );
    let logs: Vec<String> = std::fs::read_dir(r.root.join("logs"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path().join("mem-wait.log")).unwrap())
        .collect();
    assert!(
        logs.len() == 1 && logs[0].matches(said).count() == 1,
        "{logs:?}"
    );
    // What land slept ("S.UUUUUU" seconds) and the awake time it gave up at.
    let sleeps = std::fs::read_to_string(bin.join("sleeps")).unwrap_or_default();
    let want: Vec<String> = naps_s.iter().map(|s| format!("{s}.000000")).collect();
    assert_eq!(sleeps.lines().collect::<Vec<_>>(), want, "{logs:?}");
    let gave_up = format!("gave up after {} us of awake time:", gave_up_s * 1_000_000);
    assert!(logs[0].contains(&gave_up), "{gave_up}: {logs:?}");
    assert_eq!(r.calls(), ["ci-steps all"], "no memory, no cdo-gate");
}

/// A folder of links to the real scripts named in `with` (land's test copies:
/// a stand-in `host-suspended` beside them, or a script left out).
fn link_scripts(r: &Repo, with: &[&str]) -> PathBuf {
    let bin = r.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    for f in with {
        std::os::unix::fs::symlink(real.join(f), bin.join(f)).unwrap();
    }
    bin
}

/// ⟨TLS-105 review r1⟩ land loads scripts/awake-clock only for a run that waits
/// for memory (sol P1): `--help` and `--dry-run` work without it, and a run
/// that needs it says so before it does anything (no landing branch made).
///
/// Mutation (2026-11-23): the helper sourced at the top again (as in e0d333e8)
/// -> `--help` fails (red); restored, green.
#[test]
fn land_without_its_clock_helper_still_helps_and_predicts() {
    let r = repo();
    let bin = link_scripts(&r, &["land", "changelog-merge", "cdo-slot", "agentflow"]);
    let land = bin.join("land");
    let (code, text) = run_out(r.land_cmd(&land, &["--help"], &[]));
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("usage: scripts/land"), "{text}");
    let (code, text) = run_out(r.land_cmd(&land, &["--dry-run", "tools/tls-58-x"], &[]));
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = run_out(r.land_cmd(&land, &["tools/tls-58-x"], &[("LAND_MEM_MIN_GB", "1")]));
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("awake-clock is missing"), "{text}");
    assert!(
        r.landing_branches().is_empty(),
        "{:?}",
        r.landing_branches()
    );
    assert!(r.calls().is_empty(), "{:?}", r.calls());
}

/// A command's (exit, stdout + stderr).
fn run_out(mut cmd: std::process::Command) -> (Option<i32>, String) {
    let out = cmd.output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), text)
}

/// What the gates ran on must be what merge-and-push lands (review r1,
/// Fable H1/H2). If, while the gates run, the gate worktree leaves the
/// landing commit (a second run, a hand checkout) or master moves (a side
/// merge), the run stops before cdo-gate and pushes nothing, even with
/// `--push`. A second run on a worktree a first one holds is refused at once.
///
/// Mutations: drop the master check from `recheck` -> the moved-master run
/// pushes; drop the HEAD check -> the moved-worktree run goes on to cdo-gate;
/// drop the lock -> the second run is not refused.
#[test]
fn land_pushes_only_the_tree_its_gates_saw() {
    let r = repo();
    let side = format!(
        "git -C '{}' commit -q --allow-empty -m side",
        r.main.display()
    );
    let (code, text) = r.land(
        &["--push", "tools/tls-58-x"],
        &[("STUB_CI_STEPS_HOOK", &side)],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("master moved during the gates"), "{text}");
    assert_eq!(r.calls(), ["ci-steps all"], "nothing after the move");

    let r = repo();
    let away = format!("git -C '{}' switch -q --detach master", r.gate.display());
    let (code, text) = r.land(
        &["--push", "tools/tls-58-x"],
        &[("STUB_CI_STEPS_HOOK", &away)],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("moved off the landing commit"), "{text}");
    assert_eq!(r.calls(), ["ci-steps all"], "nothing after the move");

    let r = repo();
    // Another session holds the CDO slot (TLS-60): land waits for it, and with
    // a limit gives up before it changes anything.
    // Bound to a child this test owns, so the slot ends with it however the
    // test ends.
    struct Owned(std::process::Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let anchor = Owned(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let pid = anchor.0.id().to_string();
    let other = r.slot(
        "other",
        &["acquire", "engine", "a CDO measure", "--bind", &pid],
    );
    assert_eq!(other.0, Some(0), "{}", other.1);
    let (code, text) = r.land(&["tools/tls-58-x"], &[("LAND_SLOT_TIMEOUT_S", "2")]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("waiting: held by engine (a CDO measure)"),
        "{text}"
    );
    assert!(text.contains("the CDO slot was not taken"), "{text}");
    assert!(r.calls().is_empty() && r.landing_branches().is_empty());
    assert_eq!(r.slot("other", &["release"]).0, Some(0));
    drop(anchor);

    // A tag with the landing name would win over the branch when
    // merge-and-push merges the name unqualified: the name counts as taken
    // (sol r1).
    git::run(&r.main, &["tag", "landing/tls58", "tools/tls-60-bad"]);
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(r.landing_branches(), ["landing/tls58-2"]);
    // A branch under the name (`landing/foo/child`) blocks `landing/foo` too
    // (sol r2): it is skipped, not tried and failed after the merges.
    git::run(&r.main, &["branch", "landing/foo/child", "master"]);
    let (code, text) = r.land(&["--name", "foo", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("landing branch: landing/foo-2"), "{text}");
}

/// TLS-60: land holds the CDO slot from its start to its end. While its ci
/// runs, another session sees the slot held by land and the cdo-slot-guard hook
/// refuses that session's `scripts/cdo-gate` (TLS-89: land's hold is untimed,
/// so a `cargo test` would pass; only cdo-speed's timing inside it is timed);
/// the cdo-gate land runs shares the hold (its acquire answers "already yours",
/// exit 4); after the run the slot is free again.
///
/// Mutations: land without its acquire -> the other session sees `free` and
/// its `scripts/cdo-gate` passes the hook; land without the token export -> the
/// nested acquire waits on land's own hold.
#[test]
fn land_holds_the_cdo_slot_from_start_to_end() {
    let r = repo();
    let slot = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
    let guard = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/claude-hooks/cdo-slot-guard"
    );
    let ci = format!(
        "CLAUDE_CODE_SESSION_ID=other CDO_SLOT_TOKEN= bash {slot} status >> \"$STUB_LOG\" 2>&1; \
         printf '%s' '{{\"tool_name\":\"Bash\",\"session_id\":\"other\",\
         \"tool_input\":{{\"command\":\"scripts/cdo-gate /ws\"}}}}' \
         | {guard} 2>/dev/null; echo \"guard $?\" >> \"$STUB_LOG\""
    );
    let cdo = format!(
        // No session id: only land's exported token can make it "yours" (a
        // land started outside Claude, by cron, has no session).
        "CLAUDE_CODE_SESSION_ID= bash {slot} acquire cdo-gate nested --wait --timeout 3 >/dev/null 2>&1; \
         echo \"nested $?\" >> \"$STUB_LOG\""
    );
    let (code, text) = r.land(
        &["tools/tls-58-x"],
        &[("STUB_CI_STEPS_HOOK", &ci), ("STUB_CDO_GATE_HOOK", &cdo)],
    );
    assert_eq!(code, Some(0), "{text}");
    let calls = r.calls();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert_eq!(calls[0], "ci-steps all");
    assert!(
        calls[1].starts_with("held by land (landing tools/tls-58-x) since "),
        "{calls:?}"
    );
    assert!(
        calls[1].contains("session land-test, for PID "),
        "bound to the land run: {calls:?}"
    );
    assert_eq!(calls[2], "guard 2", "the hook refuses the other session");
    assert_eq!(calls[3], "cdo-gate /cdo-ws");
    assert_eq!(
        calls[4], "nested 4",
        "the nested acquire shares land's hold"
    );
    let (code, text) = r.slot("other", &["status"]);
    assert_eq!((code, text.trim()), (Some(0), "free"));
}

/// Review r1 (sol): land needs the slot to itself, and only the slot decides
/// who gates. A slot its own session already holds by hand (which it would
/// share) is refused before anything changes: two lands could otherwise run
/// on one worktree. A cdo-gate process seen while land waits for memory (one
/// queued behind this very landing) no longer holds land up.
///
/// Mutations: land accepting a shared hold (exit 4) -> the run goes on; the
/// mem-wait's process check restored -> the run gives up at mem-wait.
#[test]
fn land_needs_the_slot_to_itself_and_waits_only_for_memory() {
    let r = repo();
    let (code, text) = r.slot("land-test", &["acquire", "tools", "by hand"]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("land needs it to itself"), "{text}");
    assert!(r.calls().is_empty() && r.landing_branches().is_empty());
    assert_eq!(r.slot("land-test", &["release"]).0, Some(0));

    // A process whose command line is `bash <dir>/scripts/cdo-gate`.
    let fake = r.root.join("other/scripts");
    std::fs::create_dir_all(&fake).unwrap();
    // No `exec`: the bash process keeps the command line `bash <dir>/scripts/
    // cdo-gate` (review r2, sol). Its own process group, which the guard ends
    // whole (bash and its `sleep`), with no output shared with the test.
    std::fs::write(fake.join("cdo-gate"), "sleep 60\n").unwrap();
    struct Group(std::process::Child);
    impl Drop for Group {
        fn drop(&mut self) {
            // SAFETY: SIGKILL to the process group this test created.
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }
    use std::os::unix::process::CommandExt;
    let gate = Group(
        crate::clean_env::command("bash")
            .arg(fake.join("cdo-gate"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    // Ready: the process's command line names the gate.
    let cmdline = format!("/proc/{}/cmdline", gate.0.id());
    let start = std::time::Instant::now();
    while !std::fs::read_to_string(&cmdline).is_ok_and(|s| s.contains("scripts/cdo-gate")) {
        assert!(
            start.elapsed().as_secs() < 10,
            "the fake gate never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (code, text) = r.land(
        &["tools/tls-58-x"],
        &[("LAND_MEM_MIN_GB", "1"), ("LAND_MEM_TIMEOUT_S", "0")],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(r.calls(), ["ci-steps all", "cdo-gate /cdo-ws"]);
}

/// Every refusal leaves nothing behind. A conflict outside CHANGELOG.md: the
/// dry run predicts it (exit 1) and moves no ref, HEAD or file; the real run
/// stops with the conflicting path, makes no landing branch, leaves the
/// worktree clean and gates nothing. A CHANGELOG conflict that is not
/// additions on both sides (a line both sides reworded; a side that deleted
/// the file; content `git merge-file` cannot merge) is refused the same way,
/// in the dry run and the real run alike. Then: master as a branch, an
/// unknown branch, a branch given twice, a dirty worktree, a master behind
/// origin/master, a branch already in master (exit 1), and `--push` with
/// `--dry-run` or a LAND_MEM_MIN_GB that is no number (exit 2).
///
/// Mutations: create `landing/<name>` before the merges -> the refused run
/// leaves a branch; the additions-only rule dropped (base part ignored) ->
/// the reworded line merges; the dry run's stage check dropped -> it says
/// a deleted CHANGELOG would merge.
#[test]
fn land_refuses_and_leaves_nothing_behind() {
    let r = repo();
    let before = r.refs();
    let head = r.git(&r.gate, &["rev-parse", "HEAD"]);
    let (code, text) = r.land(&["--dry-run", "tools/tls-60-bad"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("TLS-60: REFUSE: conflicts in a.txt"),
        "{text}"
    );
    assert_eq!(r.refs(), before, "a dry run moves no ref");
    assert_eq!(r.git(&r.gate, &["rev-parse", "HEAD"]), head);
    assert_eq!(r.git(&r.gate, &["status", "--porcelain"]), "");

    let refused = |branch: &str, dry: &str, real: &str| {
        let (code, text) = r.land(&["--dry-run", branch], &[]);
        assert_eq!(code, Some(1), "dry {branch}: {text}");
        assert!(text.contains(dry), "dry {branch}: {text}");
        let (code, text) = r.land(&[branch], &[]);
        assert_eq!(code, Some(1), "{branch}: {text}");
        assert!(text.contains(real), "{branch}: {text}");
        assert!(
            text.contains("no landing branch was made"),
            "{branch}: {text}"
        );
        assert!(
            r.landing_branches().is_empty(),
            "{:?}",
            r.landing_branches()
        );
        assert_eq!(r.git(&r.gate, &["status", "--porcelain"]), "", "{branch}");
        assert!(r.calls().is_empty(), "{branch}: {:?}", r.calls());
    };
    refused(
        "tools/tls-60-bad",
        "REFUSE: conflicts in a.txt",
        "conflicts in: a.txt",
    );

    // CHANGELOG cases: master rewords `- old`; one branch rewords it too, one
    // deletes the file, one writes content git cannot merge as text.
    r.branch(
        "tools/tls-61-reword",
        &[("CHANGELOG.md", changelog("", "- old, branch").as_bytes())],
    );
    git::run(&r.main, &["switch", "-qc", "tools/tls-62-del", "master"]);
    git::run(&r.main, &["rm", "-q", "CHANGELOG.md"]);
    git::run(&r.main, &["commit", "-qm", "delete"]);
    git::run(&r.main, &["switch", "-q", "master"]);
    r.branch(
        "tools/tls-63-bin",
        &[("CHANGELOG.md", b"# Changelog\0binary\n")],
    );
    std::fs::write(r.main.join("CHANGELOG.md"), changelog("", "- old, master")).unwrap();
    git::run(&r.main, &["commit", "-qam", "master rewords"]);
    git::run(&r.gate, &["switch", "-q", "--detach", "master"]);
    let line = "changes a line both sides had";
    refused("tools/tls-61-reword", line, line);
    refused(
        "tools/tls-62-del",
        "CHANGELOG.md is missing on one side",
        "CHANGELOG.md is missing on one side",
    );
    let bin = "cannot merge CHANGELOG.md";
    refused("tools/tls-63-bin", bin, bin);

    for (args, says) in [
        (&["master"][..], "master is not a branch to land"),
        (&["nope"], "no local branch 'nope'"),
        (&["tools/tls-58-x", "tools/tls-58-x"], "given twice"),
    ] {
        let (code, text) = r.land(args, &[]);
        assert_eq!(code, Some(1), "{args:?}: {text}");
        assert!(text.contains(says), "{args:?}: {text}");
    }
    std::fs::write(r.gate.join("dirt"), "x").unwrap();
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is not clean"), "{text}");
    std::fs::remove_file(r.gate.join("dirt")).unwrap();

    let ahead = r.git(&r.main, &["rev-parse", "tools/tls-60-bad"]);
    git::run(
        &r.main,
        &["update-ref", "refs/remotes/origin/master", &ahead],
    );
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is behind (or off) origin/master"), "{text}");
    git::run(&r.main, &["update-ref", "-d", "refs/remotes/origin/master"]);

    let (code, text) = r.land(&["--push", "--dry-run", "tools/tls-58-x"], &[]);
    assert_eq!(code, Some(2), "{text}");
    // Not a number, octal-looking (`08` is no bash number, `035` is 29), or a
    // CDO_WS cdo-gate would read as an option and exit 0 with nothing gated.
    for env in [
        ("LAND_MEM_MIN_GB", "3.5"),
        ("LAND_MEM_MIN_GB", "08"),
        ("LAND_MEM_TIMEOUT_S", "035"),
        ("CDO_WS", "--help"),
        ("CDO_WS", "relative/ws"),
    ] {
        let (code, text) = r.land(&["tools/tls-58-x"], &[env]);
        assert_eq!(code, Some(2), "{env:?}: {text}");
    }

    git::run(&r.main, &["merge", "-q", "--no-edit", "tools/tls-58-x"]);
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is already in master"), "{text}");
    assert!(
        r.calls().is_empty(),
        "no refusal gated anything: {:?}",
        r.calls()
    );
}

/// ⟨TLS-113⟩ `--no-speed=<reason>`: cdo-gate gets `--no-speed` on its command
/// line, the reason is printed, heads cdo.log, is in the verdict and in the
/// PASS line, and no step sees CDO_GATE_NO_SPEED. Each bad reason is a usage
/// error (exit 2) with its own message before anything runs: a bare
/// `--no-speed` (the spelling that let a forgotten reason swallow a branch),
/// an empty, blank or option-like reason, one given twice, one with a control
/// character, one naming an existing path or a branch. CDO_GATE_NO_SPEED in
/// land's own environment is refused (exit 2), even empty: an exported
/// variable reached every step of ENG-159's landing and failed `ci-steps
/// agentflow`. A declared test hook (ci-steps', cdo-speed's) is refused the
/// same way, and no step runs a BASH_ENV or ENV startup file (review r1, sol).
///
/// Discrimination proofs (2026-11-21, each break made with Edit in
/// scripts/land, `cargo test --test cli land_script::land_no_speed`, reverted):
/// - the CDO_GATE_NO_SPEED refusal removed -> FAILED (the run with it set
///   exited 0 and gated); reverted -> passed.
/// - the reason check removed -> FAILED (an empty reason exited 0); reverted
///   -> passed.
/// - `"${cdo_flags[@]}"` dropped from the cdo-gate call -> FAILED (calls read
///   `cdo-gate /cdo-ws`); reverted -> passed.
/// - the verdict line removed -> FAILED (no `cdo no-speed:` line); reverted
///   -> passed.
/// - (r2) the branch check removed -> FAILED (the branch taken for a reason
///   exited 0); reverted -> passed.
/// - (r2) the twice check removed -> FAILED; the control-character check
///   removed -> FAILED; the path check removed -> FAILED; each reverted ->
///   passed.
/// - (r2) the test-hook refusal removed -> FAILED (CI_STEPS_PERF_BOUNDS_BIN
///   exited 0); reverted -> passed.
/// - (r2) `unset BASH_ENV ENV` removed -> FAILED (the steps logged `startup`
///   lines); reverted -> passed.
/// - (r3) the build-override refusal removed -> FAILED (the
///   CARGO_PROFILE_RELEASE_FAST_DEBUG_ASSERTIONS run exited 0); reverted ->
///   passed.
/// - (r4) the engine-seam refusal removed -> FAILED (the ALSEM_TEST_WORKERS
///   run exited 0); reverted -> passed.
#[test]
fn land_no_speed_takes_a_reason_and_passes_the_flag() {
    let r = repo();
    let why = "owner-approved: ENG-159 speed skip";
    let reason: &str = &format!("--no-speed={why}");
    let a_path: &str = &format!("--no-speed={}", r.root.display());
    for (args, says) in [
        (
            &["--no-speed", why, "tools/tls-58-x"][..],
            "--no-speed needs a reason: --no-speed=<reason>",
        ),
        (&["--no-speed=", "tools/tls-58-x"], "needs a reason"),
        (&["--no-speed=  ", "tools/tls-58-x"], "needs a reason"),
        (&["--no-speed=--push", "tools/tls-58-x"], "needs a reason"),
        (
            &[reason, "--no-speed=other", "tools/tls-58-x"],
            "--no-speed given twice",
        ),
        (
            &["--no-speed=a\rb", "tools/tls-58-x"],
            "the reason has a control character",
        ),
        (
            &["--no-speed=a\nb", "tools/tls-58-x"],
            "the reason has a control character",
        ),
        (
            &[a_path, "tools/tls-58-x"],
            "names an existing path, not a reason",
        ),
        (
            &["--no-speed=tools/tls-58-x", "tools/tls-59-x"],
            "'tools/tls-58-x' names a branch, not a reason",
        ),
    ] {
        let (code, text) = r.land(args, &[]);
        assert_eq!(code, Some(2), "{args:?}: {text}");
        assert!(text.contains(says), "{args:?}: {says}: {text}");
    }
    for env in [("CDO_GATE_NO_SPEED", "1"), ("CDO_GATE_NO_SPEED", "")] {
        let (code, text) = r.land(&[reason, "tools/tls-58-x"], &[env]);
        assert_eq!(code, Some(2), "{env:?}: {text}");
        assert!(text.contains("CDO_GATE_NO_SPEED is set"), "{env:?}: {text}");
    }
    for env in [
        ("CI_STEPS_PERF_BOUNDS_BIN", "/bin/true"),
        ("CDO_SPEED_PERF", "/bin/true"),
    ] {
        let (code, text) = r.land(&["tools/tls-58-x"], &[env]);
        assert_eq!(code, Some(2), "{env:?}: {text}");
        assert!(
            text.contains(&format!(
                "test hooks are set, which no real landing may pass to its gates: {}",
                env.0
            )),
            "{env:?}: {text}"
        );
    }
    // (r2, sol) Build-shape overrides: one would compile perf_bounds' timing
    // tests out, or build another tree than the gates think.
    for env in [
        ("CARGO_PROFILE_RELEASE_FAST_DEBUG_ASSERTIONS", "true"),
        ("CARGO_BUILD_JOBS", "1"),
        ("RUSTFLAGS", "-Cdebug-assertions"),
        ("RUSTDOCFLAGS", "x"),
        ("CARGO_ENCODED_RUSTFLAGS", "x"),
        ("CARGO_TARGET_DIR", "/tmp/elsewhere"),
    ] {
        let (code, text) = r.land(&["tools/tls-58-x"], &[env]);
        assert_eq!(code, Some(2), "{env:?}: {text}");
        assert!(
            text.contains(&format!(
                "build overrides are set, which no landing may pass to its builds: {}",
                env.0
            )),
            "{env:?}: {text}"
        );
    }
    // (r4) The engine's own test seams, read in Rust by the suite ci-steps runs.
    for env in [
        ("ALSEM_TEST_WORKERS", "1"),
        ("ALSEM_TEST_WORKER_DIES", "1"),
        ("ALSEM_BATCH_CLAIMS", "/tmp/claims"),
    ] {
        let (code, text) = r.land(&["tools/tls-58-x"], &[env]);
        assert_eq!(code, Some(2), "{env:?}: {text}");
        assert!(
            text.contains(&format!(
                "engine test seams are set, which no landing may pass to its gates: {}",
                env.0
            )),
            "{env:?}: {text}"
        );
    }
    assert!(
        r.calls().is_empty(),
        "a refusal gates nothing: {:?}",
        r.calls()
    );

    let (code, text) = r.land(&["--dry-run", reason, "tools/tls-58-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("; cdo-gate --no-speed /"), "{text}");

    // Each stub logs whether it sees the variable: no step may. A startup
    // file logs every bash but land's own (its bash ran it before line 1).
    let seen = "echo \"env ${CDO_GATE_NO_SPEED-unset}\" >> \"$STUB_LOG\"";
    let startup = r.root.join("startup.sh");
    std::fs::write(
        &startup,
        "case \"$0\" in */land) ;; *) echo \"startup $0\" >> \"$STUB_LOG\" ;; esac\n",
    )
    .unwrap();
    let startup = startup.to_str().unwrap();
    let (code, text) = r.land(
        &[reason, "tools/tls-58-x"],
        &[
            ("STUB_CI_STEPS_HOOK", seen),
            ("STUB_CDO_GATE_HOOK", seen),
            ("BASH_ENV", startup),
            ("ENV", startup),
        ],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        r.calls(),
        [
            "ci-steps all",
            "env unset",
            "cdo-gate --no-speed /cdo-ws",
            "env unset"
        ]
    );
    assert!(
        text.contains(&format!(
            "land: cdo-gate runs with --no-speed (SPEED AND MEMORY NOT CHECKED): {why}"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "land: PASS (cdo-gate --no-speed: {why}): landing/tls58 is gated"
        )),
        "{text}"
    );
    let run = std::fs::read_dir(r.root.join("logs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let verdict = std::fs::read_to_string(run.join("verdict")).unwrap();
    assert!(
        verdict.contains(&format!("\ncdo no-speed: {why}\n")),
        "{verdict}"
    );
    let cdo_log = std::fs::read_to_string(run.join("cdo.log")).unwrap();
    assert!(
        cdo_log.starts_with(&format!("land: cdo-gate --no-speed: {why}\n")),
        "{cdo_log}"
    );
}

/// The real `scripts/merge-and-push`, with a bare origin and the landing
/// branch `scripts/land` built (stub gates). Told what the gates saw
/// (`MERGE_EXPECT_MASTER`, `MERGE_EXPECT_SOURCE`), it merges and pushes
/// nothing when the branch is another commit, when the merge would not give
/// the gated tree (a branch not built on master), or when master moved after
/// the gates (sol r1, r2). With all of them right it merges the branch's
/// commit, not a same-named tag (sol r2), and
/// pushes that merge.
///
/// Mutations: drop the source check, the tree check or the master check ->
/// that case merges or pushes; merge the bare name again -> the tag wins and
/// nothing is merged.
#[test]
fn merge_and_push_lands_only_what_the_gates_saw() {
    let r = repo();
    let origin = r.root.join("origin.git");
    git::run(
        &r.root,
        &[
            "clone",
            "-q",
            "--bare",
            r.main.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git::run(
        &r.main,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git::run(&r.main, &["fetch", "-q", "origin"]);
    let tmp = r.root.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let push = |branch: &str, master: &str, source: &str| {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/merge-and-push"
        ))
        .arg(branch)
        .current_dir(&r.main);
        let out = clean_env(&mut cmd)
            .env("STUB_LOG", r.root.join("calls"))
            .env("TMPDIR", &tmp)
            .env("MERGE_EXPECT_MASTER", master)
            .env("MERGE_EXPECT_SOURCE", source)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    let rev = |dir: &Path, what: &str| r.git(dir, &["rev-parse", what]);

    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let m1 = rev(&r.main, "master");
    let src = rev(&r.main, "landing/tls58");
    let refused = |what: &str, code: Option<i32>, text: &str| {
        assert_eq!(code, Some(1), "{what}: {text}");
        assert_eq!(rev(&r.main, "master"), m1, "{what}: master unchanged");
        assert_eq!(rev(&origin, "master"), m1, "{what}: nothing pushed");
    };
    let other = rev(&r.main, "tools/tls-59-x");
    let (code, text) = push("landing/tls58", &m1, &other);
    refused("another source", code, &text);
    assert!(
        text.contains("as the gates expected; nothing merged"),
        "{text}"
    );
    let unbuilt = rev(&r.main, "tools/tls-58-x");
    let (code, text) = push("tools/tls-58-x", &m1, &unbuilt);
    refused("not built on master", code, &text);
    assert!(text.contains("is not the gated tree"), "{text}");

    // A side commit lands on master after the gates saw m1.
    git::run(&r.main, &["commit", "-q", "--allow-empty", "-m", "side"]);
    let m2 = rev(&r.main, "master");
    let (code, text) = push("landing/tls58", &m1, &src);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("master is at"), "{text}");
    assert_eq!(rev(&r.main, "master"), m2);
    assert_eq!(rev(&origin, "master"), m1);

    // Landed again on m2; then a tag named like the branch appears (made after
    // land chose the name: merge-and-push's `fetch origin master` brings no
    // tags, checked by hand 2026-11-14, so it would come from elsewhere). A
    // bare name would resolve to the tag, which git prefers over the branch.
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let src2 = rev(&r.main, "landing/tls58-2");
    git::run(&r.main, &["tag", "landing/tls58-2", &m1]);
    let (code, text) = push("landing/tls58-2", &m2, &src2);
    assert_eq!(code, Some(0), "{text}");
    let pushed = rev(&origin, "master");
    assert_eq!(pushed, rev(&r.main, "master"));
    assert_eq!(rev(&r.main, "master^1"), m2);
    assert_eq!(rev(&r.main, "master^2"), src2, "the branch, not the tag");

    // Told what land's gates saw, a source already in master is no landing to
    // push (sol r4: that path skipped the tree check).
    let m3 = rev(&r.main, "master");
    let (code, text) = push("landing/tls58-2", &m3, &src2);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("which the gates did not expect"), "{text}");
    assert_eq!(rev(&r.main, "master"), m3);

    // The plain path (no expect variables), as run by hand: it merges the
    // branch's commit, not a same-named tag, and pushes that merge (Fable r3).
    r.branch("tools/tls-65-plain", &[("f65.txt", b"65".as_slice())]);
    let plain = rev(&r.main, "tools/tls-65-plain");
    git::run(&r.main, &["tag", "tools/tls-65-plain", &m1]);
    let (code, text) = push("tools/tls-65-plain", "", "");
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(rev(&r.main, "master^1"), m3);
    assert_eq!(rev(&r.main, "master^2"), plain, "the branch, not the tag");
    assert_eq!(rev(&origin, "master"), rev(&r.main, "master"));
}

/// ⟨TLS-87⟩ land checks each branch's pre-submit CDO record (scripts/cdo-gate
/// writes it for a passing `--only` run, under the tree it ran on, in the
/// shared git directory). A branch changing product code (src/, Cargo.toml)
/// needs `harness,l4,output`; one changing only the subset's own goldens needs
/// their phases; docs, scripts, top-level Markdown and other tests need none.
/// No record is a loud warning naming the missing phases and the command to
/// run; under LAND_REQUIRE_PRESUBMIT=1 a refusal. A record for its head tree
/// satisfies it (hand-written here: the precondition stated, not produced). A
/// file moved out of src/ still counts (`--no-renames`). In a multi-branch
/// landing only the branch without its record is named.
///
/// Mutations (2026-11-18): the LAND_REQUIRE_PRESUBMIT refusal dropped -> the
/// branch with no record passes under it (red); `output_cdo` dropped from the
/// product need -> the src branch with only harness,l4 passes (red). Review r1
/// (Fable): `--no-renames` dropped -> the moved-out file counts at its new,
/// exempt path only and the branch passes (red); the exempt list widened to
/// all of tests/ -> the l4-golden branch passes with no record (red). Each
/// restored, green. Review r1 (sol): the merge by branch name, not the pinned
/// commit -> the landing's second parent is the moved tip (red); `cli_closure`
/// returning nothing -> the
/// output-helper branch passes (red); `core.quotePath=false` dropped -> the
/// docs branch with `docs/café.md` is refused (red).
#[test]
fn land_checks_the_pre_submit_cdo_record() {
    let r = repo();
    // (No docs/ folder here: `branch` makes the folders of the files it writes.)
    for d in ["src/engine", "tests/l4-summary-baseline", "tests/cli"] {
        std::fs::create_dir_all(r.main.join(d)).unwrap();
    }
    // A file on master that a branch moves out of src/.
    std::fs::write(r.main.join("src/engine/moved.rs"), "// moved\n".repeat(20)).unwrap();
    // The output phase's test and a helper it imports (review r1, sol).
    std::fs::write(
        r.main.join("tests/cli/output_cdo.rs"),
        "use crate::output_invariants::check;\n",
    )
    .unwrap();
    std::fs::write(r.main.join("tests/cli/output_invariants.rs"), "// v1\n").unwrap();
    git::run(
        &r.main,
        &[
            "add",
            "src/engine/moved.rs",
            "tests/cli/output_cdo.rs",
            "tests/cli/output_invariants.rs",
        ],
    );
    git::run(&r.main, &["commit", "-qm", "engine file"]);
    r.branch(
        "tools/tls-66-outhelper",
        &[("tests/cli/output_invariants.rs", b"// v2\n")],
    );
    r.branch("tools/tls-61-src", &[("src/lib.rs", b"// x\n")]);
    r.branch("tools/tls-62-cargo", &[("Cargo.toml", b"[package]\n")]);
    // Review r2 (sol): test data the harness reads.
    r.branch(
        "tools/tls-68-fixture",
        &[("tests/fixtures/events/x.al", b"// x\n")],
    );
    r.branch(
        "tools/tls-63-docs",
        &[
            ("docs/n.md", b"x\n"),
            // Review r1 (sol): git quotes a non-ASCII path by default, which
            // would hide the docs/ prefix.
            ("docs/café.md", b"x\n"),
            // Review r2 (sol): a tab is quoted even with core.quotePath=false.
            ("docs/tab\there.md", b"x\n"),
            ("scripts/extra", b"x\n"),
            ("README.md", b"x\n"),
            ("tests/cli/other.rs", b"// x\n"),
        ],
    );
    r.branch(
        "tools/tls-64-l4gold",
        &[("tests/l4-summary-baseline/x.json", b"{}\n")],
    );
    git::run(&r.main, &["switch", "-qc", "tools/tls-65-move", "master"]);
    // (A switch back to master removed the emptied folder.)
    std::fs::create_dir_all(r.main.join("tests/cli")).unwrap();
    git::run(
        &r.main,
        &["mv", "src/engine/moved.rs", "tests/cli/moved.rs"],
    );
    git::run(&r.main, &["commit", "-qm", "move"]);
    git::run(&r.main, &["switch", "-q", "master"]);
    let require = [("LAND_REQUIRE_PRESUBMIT", "1")];
    let record = |branch: &str, phases: &str| {
        let tree = r.git(&r.main, &["rev-parse", &format!("{branch}^{{tree}}")]);
        let dir = r.main.join(".git/cdo-presubmit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(tree), phases).unwrap();
    };

    let all = "l4_summary_differential\noutput_cdo\nprogram_resolve_harness\n";

    // No record: a loud warning, the run goes on.
    let (code, text) = r.land(&["--dry-run", "tools/tls-61-src"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("WARNING: 'tools/tls-61-src' has no passing pre-submit CDO record")
            && text.contains("missing program_resolve_harness l4_summary_differential output_cdo")
            && text.contains("scripts/cdo-gate --only harness,l4,output <CDO_WS>"),
        "{text}"
    );
    // ... and a refusal when required.
    let (code, text) = r.land(&["--dry-run", "tools/tls-61-src"], &require);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("no pre-submit CDO record for tools/tls-61-src"),
        "{text}"
    );

    // harness,l4 alone is not enough for product code.
    record(
        "tools/tls-61-src",
        "l4_summary_differential\nprogram_resolve_harness\n",
    );
    let (code, text) = r.land(&["--dry-run", "tools/tls-61-src"], &require);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("missing output_cdo"), "{text}");
    // With the full record for its head tree: accepted.
    record("tools/tls-61-src", all);
    let (code, text) = r.land(&["--dry-run", "tools/tls-61-src"], &require);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("'tools/tls-61-src' pre-submit CDO record: program_resolve_harness l4_summary_differential output_cdo"),
        "{text}"
    );

    // Cargo.toml is product code too.
    let (code, text) = r.land(&["--dry-run", "tools/tls-62-cargo"], &require);
    assert_eq!(code, Some(1), "{text}");

    // An l4 golden re-mint needs harness,l4 (no output).
    let (code, text) = r.land(&["--dry-run", "tools/tls-64-l4gold"], &require);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("missing program_resolve_harness l4_summary_differential")
            && !text.contains("output_cdo"),
        "{text}"
    );
    record(
        "tools/tls-64-l4gold",
        "l4_summary_differential\nprogram_resolve_harness\n",
    );
    let (code, text) = r.land(&["--dry-run", "tools/tls-64-l4gold"], &require);
    assert_eq!(code, Some(0), "{text}");

    // A file moved out of src/ into an exempt place still counts.
    let (code, text) = r.land(&["--dry-run", "tools/tls-65-move"], &require);
    assert_eq!(code, Some(1), "a move out of src/ was exempt: {text}");

    // Docs, scripts, top-level Markdown, other tests: no record needed.
    let (code, text) = r.land(&["--dry-run", "tools/tls-63-docs"], &require);
    assert_eq!(code, Some(0), "{text}");
    assert!(!text.contains("pre-submit"), "{text}");

    // Review r1 (sol): the branch gains a commit after the checks (here: moved
    // to one adding a source file): the landing merges the commit that was
    // checked, not the new tip. Break: the merge by branch name.
    let rev = |what: &str| r.git(&r.main, &["rev-parse", what]);
    r.branch("tools/tls-67-late", &[("docs/late-ok.md", b"x\n")]);
    let checked = rev("tools/tls-67-late");
    git::run(&r.main, &["switch", "-q", "tools/tls-67-late"]);
    std::fs::write(r.main.join("src/late.rs"), "// late\n").unwrap();
    git::run(&r.main, &["add", "src/late.rs"]);
    git::run(&r.main, &["commit", "-qm", "late source"]);
    let late = rev("tools/tls-67-late");
    git::run(&r.main, &["switch", "-q", "master"]);
    git::run(&r.main, &["branch", "-qf", "tools/tls-67-late", &checked]);
    let main = r.main.to_str().unwrap().to_string();
    let hook = format!("git -C '{main}' branch -qf tools/tls-67-late {late}");
    let (code, text) = r.land(
        &["tools/tls-67-late"],
        &[("LAND_TEST_AFTER_CHECK", hook.as_str())],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(rev("tools/tls-67-late"), late, "precondition: it moved");
    assert_eq!(
        rev("landing/tls67^2"),
        checked,
        "the landing merged a commit no check saw"
    );

    // Review r2 (sol): a fixture change needs the record too. Break: tests/
    // fixtures/ left exempt with the other tests.
    let (code, text) = r.land(&["--dry-run", "tools/tls-68-fixture"], &require);
    assert_eq!(code, Some(1), "a fixture change was exempt: {text}");
    assert!(
        text.contains("missing program_resolve_harness l4_summary_differential output_cdo"),
        "{text}"
    );

    // Review r1 (sol): a helper the output test imports is output's input too.
    let (code, text) = r.land(&["--dry-run", "tools/tls-66-outhelper"], &require);
    assert_eq!(code, Some(1), "an output helper was exempt: {text}");
    assert!(
        text.contains("missing program_resolve_harness l4_summary_differential output_cdo"),
        "{text}"
    );

    // Several branches: only the one without its record is named.
    let (code, text) = r.land(
        &["--dry-run", "tools/tls-61-src", "tools/tls-62-cargo"],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("WARNING: 'tools/tls-62-cargo'")
            && !text.contains("WARNING: 'tools/tls-61-src'"),
        "{text}"
    );
}
