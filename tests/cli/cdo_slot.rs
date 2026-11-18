//! ⟨TLS-60⟩ `scripts/cdo-slot` (the one CDO slot) and its PreToolUse hook
//! `scripts/claude-hooks/cdo-slot-guard`. Every test uses a slot folder of its
//! own (`CDO_SLOT_DIR`), never the tentacle's.

const SLOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
const GUARD: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/claude-hooks/cdo-slot-guard"
);

/// A slot folder; whatever holder a test leaves is killed when it drops.
struct Slot(tempfile::TempDir);

impl Slot {
    fn new() -> Self {
        Slot(tempfile::tempdir().unwrap())
    }
    fn dir(&self) -> std::path::PathBuf {
        self.0.path().join("slot")
    }
    /// `cdo-slot <args>` as Claude session `session` with `env` added
    /// (`CLAUDE_PID` only when `env` gives one); (exit, stdout + stderr).
    fn run(&self, session: &str, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(SLOT)
            .args(args)
            .env("CDO_SLOT_DIR", self.dir())
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            // TLS-89: no real build on this machine may hold up a timed hold.
            .env("CDO_SLOT_BUILD_NAMES", "tls89-none")
            .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
    /// The hook, for a Bash call `command` from session `session`.
    fn guard(&self, session: &str, command: &str) -> (Option<i32>, String) {
        let input = serde_json::json!({
            "tool_name": "Bash",
            "session_id": session,
            "tool_input": { "command": command },
        });
        self.guard_raw(&input.to_string())
    }
    fn guard_raw(&self, input: &str) -> (Option<i32>, String) {
        self.guard_raw_env(input, &[])
    }
    fn guard_raw_env(&self, input: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
        use std::io::Write;
        let mut cmd = std::process::Command::new(GUARD);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .env("CDO_SLOT_DIR", self.dir())
            .env_remove("CDO_WS")
            .env_remove("CDO_SLOT_TOKEN")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
    fn holder(&self) -> Option<i32> {
        std::fs::read_to_string(self.dir().join("status"))
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("holder_pid="))?
            .parse()
            .ok()
    }
    /// Polls (up to 20 s) until `status` says free.
    fn wait_free(&self) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(20) {
            if self.run("probe", &["status"], &[]).0 == Some(0) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(pid) = self.holder() {
            // SAFETY: SIGKILL to the holder this test's slot started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// ⟨TLS-90⟩ A spawned `cdo-slot acquire --wait` waiter that is killed and reaped
/// when it drops, so a failing assertion cannot leak it. `Slot`'s `Drop` kills
/// only the slot's holder. The TLS-88 breaks left waiters polling their temporary
/// folders for hours, and some then took their test's slot once its holder was
/// killed, leaving a holder that never ends. (A test process killed outright runs
/// no `Drop`, so that case can still leak.)
pub(crate) struct Reap(pub(crate) std::process::Child);

/// ⟨TLS-90⟩ The one way the slot tests spawn a waiter: `acquire <owner> <reason>
/// --wait` as session `session`, polling every `poll` seconds with memory floor
/// `mem` GB, held in [`Reap`].
fn waiter(s: &Slot, owner: &str, reason: &str, session: &str, poll: &str, mem: &str) -> Reap {
    Reap(
        std::process::Command::new("bash")
            .args([SLOT, "acquire", owner, reason, "--wait"])
            .env("CDO_SLOT_DIR", s.dir())
            .env("CDO_SLOT_MEM_MIN_GB", mem)
            .env("CDO_SLOT_POLL_S", poll)
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

impl std::ops::Deref for Reap {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Reap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// ⟨TLS-90⟩ A waiter from [`waiter`] (the helper every slot test uses) is gone
/// after a panic in the test that spawned it.
///
/// Mutation: `Reap::drop` without the kill, and `try_wait` for `wait` (a plain
/// `wait` would block on the live waiter) -> "the waiter outlived the panic" (red).
#[test]
fn reap_ends_a_waiter_when_the_test_panics() {
    let s = Slot::new();
    let (code, text) = s.run("A", &["acquire", "a", "hold"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let pid = std::sync::Mutex::new(0u32);
    let r = std::panic::catch_unwind(|| {
        let w = waiter(&s, "w", "leak", "B", "1", "0");
        *pid.lock().unwrap() = w.id();
        panic!("a failing assertion");
    });
    assert!(r.is_err(), "precondition: the closure panicked");
    let pid = *pid.lock().unwrap();
    assert!(pid != 0, "precondition: the waiter was spawned");
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the waiter {pid} outlived the panic"
    );
}

/// One holder at a time. A takes the slot; A again is "already yours" (exit 4);
/// B sees it held, is refused at once without --wait, waits and times out with
/// it, and may not release it; A releases it and it is free. A script's token
/// (CDO_SLOT_TOKEN) makes a nested acquire from another session share the hold.
///
/// Mutations: the session match dropped from `state` -> A's second acquire
/// waits instead of exit 4; the holder check dropped from `release` -> B frees
/// A's slot.
#[test]
fn cdo_slot_holds_one_session_at_a_time() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["status"], &[]), (Some(0), "free\n".into()));
    let (code, text) = s.run("A", &["acquire", "tools", "measure x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = s.run("A", &["acquire", "tools", "again"], &[]);
    assert_eq!(code, Some(4), "{text}");
    let (code, text) = s.run("B", &["status"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.starts_with("held by tools (measure x) since "),
        "{text}"
    );
    assert!(text.trim_end().ends_with("session A"), "{text}");
    let (code, text) = s.run("B", &["acquire", "engine", "y"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("use --wait"), "{text}");
    let (code, text) = s.run(
        "B",
        &["acquire", "engine", "y", "--wait", "--timeout", "1"],
        &[],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("waiting: held by tools"), "{text}");
    let (code, text) = s.run("B", &["release"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("not yours"), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    assert_eq!(s.run("B", &["status"], &[]).0, Some(0));

    // A script's token: the hold it took is shared with what it starts.
    let (code, text) = s.run("A", &["acquire", "land", "x"], &[("CDO_SLOT_TOKEN", "t1")]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = s.run(
        "",
        &["acquire", "cdo-gate", "y"],
        &[("CDO_SLOT_TOKEN", "t1")],
    );
    assert_eq!(code, Some(4), "{text}");
    assert_eq!(
        s.run("", &["release"], &[("CDO_SLOT_TOKEN", "t1")]).0,
        Some(0)
    );
    assert_eq!(s.run("B", &["status"], &[]).0, Some(0));
}

/// The slot frees itself. A holder whose Claude process is gone is STALE (exit
/// 3) and the next acquire ends it and takes over. A holder bound to a process
/// (`--bind`, which land and cdo-gate use) ends when that process is killed
/// outright. `run` releases after its command succeeds or fails, and when it is
/// itself killed with SIGKILL. Too little memory refuses.
///
/// Mutations: the Claude-liveness check dropped -> the stale holder blocks B;
/// the bound holder's watch loop dropped -> the slot outlives the killed
/// process; `run`'s release dropped -> held after the command.
#[test]
fn cdo_slot_frees_itself() {
    let s = Slot::new();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let dead = gone.id().to_string();
    let (code, text) = s.run("C", &["acquire", "cron", "z"], &[("CLAUDE_PID", &dead)]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = s.run("B", &["status"], &[]);
    assert_eq!(code, Some(3), "{text}");
    assert!(text.trim_end().ends_with("STALE"), "{text}");
    let (code, text) = s.run("B", &["acquire", "engine", "y"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("is STALE"), "{text}");
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));

    let mut bound = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let pid = bound.id().to_string();
    let (code, text) = s.run("A", &["acquire", "tools", "b", "--bind", &pid], &[]);
    assert_eq!(code, Some(0), "{text}");
    bound.kill().unwrap();
    // Not reaped yet: a zombie is a dead process (review r1, sol).
    let freed = s.wait_free();
    bound.wait().unwrap();
    assert!(freed, "the bound slot outlived its process (a zombie)");

    for (cmd, rc) in [("true", 0), ("false", 1)] {
        let (code, text) = s.run("A", &["run", "tools", "r", "--", cmd], &[]);
        assert_eq!(code, Some(rc), "{cmd}: {text}");
        assert_eq!(s.run("B", &["status"], &[]).0, Some(0), "{cmd}: released");
    }
    // Its own process group: SIGKILL to the group ends `run` and its `sleep`
    // (and nothing of any other test); the holder is in a session of its own.
    use std::os::unix::process::CommandExt;
    let mut run = std::process::Command::new("bash")
        .args([SLOT, "run", "tools", "k", "--", "sleep", "600"])
        .env("CDO_SLOT_DIR", s.dir())
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("CLAUDE_CODE_SESSION_ID", "A")
        .env_remove("CLAUDE_PID")
        .env_remove("CDO_SLOT_TOKEN")
        .stdout(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let group = run.id() as i32;
    let start = std::time::Instant::now();
    while s.holder().is_none() {
        assert!(start.elapsed().as_secs() < 20, "run never took the slot");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // SAFETY: SIGKILL to the process group this test created.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    run.wait().unwrap();
    assert!(s.wait_free(), "a killed run left the slot held");

    let (code, text) = s.run(
        "A",
        &["acquire", "tools", "m"],
        &[("CDO_SLOT_MEM_MIN_GB", "999999")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("MemAvailable below 999999 GB"), "{text}");
}

/// The hook: while session A holds the slot TIMED (TLS-89), every session's
/// builds, tests, gates and CDO runs are refused (exit 2, naming the timed run
/// and the holder), A's own included, in every shape a command line takes.
/// While A holds it untimed, B's gates and CDO runs are refused (naming the
/// holder) and B's builds and tests pass, as do all of A's. Reading a script,
/// git and a fixture `aldump` always pass; with the slot free everything
/// passes; input it cannot read passes (a hook bug must not stop every session).
///
/// TLS-89 mutations (2026-11-18): `BUILDS = False` dropped from the untimed
/// path -> B's `cargo test --test cli` refused under the untimed hold (red);
/// the timed branch dropped -> A's own `cargo test --test cli` passes under the
/// timed hold (red). Both restored, green.
///
/// Mutations: the wrapper skipping dropped -> `nice`, `timeout -k`, `taskset`
/// shapes pass; the first-word rule dropped (every word checked) -> `grep ...
/// scripts/cdo-gate` is refused; the quote-aware reading dropped (word by
/// word) -> commit messages and greps naming `cargo test` are refused; the
/// heredoc skip dropped -> a heredoc line `cargo test` is refused; the
/// information modes dropped -> `land --dry-run` is refused; the own-session
/// check dropped -> A is refused; TLS-61: the `cdo-before-after` entry of
/// `INFO_ANYWHERE` dropped -> its `--dry-run` and `--help` are refused.
#[test]
fn cdo_slot_guard_refuses_other_sessions_while_held() {
    let s = Slot::new();
    let guarded = [
        "cargo test --test cli",
        "cd /work/x && nice -n 10 cargo +nightly build --release",
        "TREE_SITTER_AL_PATH=/x cargo clippy",
        "bash scripts/ci-steps task",
        "scripts/cdo-gate --only harness",
        "bash /work/alsem/scripts/check-goldens",
        "timeout 60 cargo nextest run",
        "bash -c \"cargo check\"",
        "env FOO=1 setsid nohup cargo bench",
        "target/cdo/aldump --program-call-graph-stats /work/src/cdo-baseline/Cloud",
        "alsem analyze \"$CDO_WS\"",
        "bash scripts/land --push tools/x",
        "echo hi; cargo run --bin aldump",
        // Review r1 (Fable): aliases, cargo's flags, shell keywords, wrappers.
        "cargo t --test cli",
        "cargo -q test",
        "cargo --locked build",
        "if ! cargo build; then echo no; fi",
        "for t in r3 r4; do cargo test --test $t; done",
        "{ cargo test; } > log 2>&1",
        "RUSTFLAGS=\"-C a -C b\" cargo build",
        "timeout -k 5 600 cargo test",
        "nice -n10 cargo build",
        "taskset -c 0-5 cargo build",
        "/usr/bin/time -v target/release-fast/aldump --snapshot-memory /work/src/cdo-baseline/Cloud",
        "cd /work/src/cdo-baseline/Cloud && aldump --stats .",
        "python3 scripts/cdo-speed --warm HEAD",
        "cd scripts && ./cdo-gate /ws",
        "cargo \\\n  test",
        // Review r2 (Fable): option clusters, substitutions, `#` in words,
        // here-strings, process substitution, rustup/watch, agentflow run.
        "bash -lc \"cargo test\"",
        "out=\"$(cargo test 2>&1)\"",
        "x=`cargo test 2>&1`",
        "if [ $# -gt 0 ]; then cargo test; fi",
        "n=${#arr[@]}; cargo test",
        "jq . <<< x\ncargo test",
        "diff <(cargo test) expected",
        "rustup run nightly cargo test",
        "watch -n 5 cargo check",
        "scripts/agentflow run --name ci -- true",
        // Review r1 (sol): a quoted `#` is no comment; an unquoted heredoc's
        // body runs its substitutions; `'<<EOF'` in quotes starts no heredoc.
        "rg '# TODO' src && cargo test",
        "cat <<EOF\n$(cargo test)\nEOF",
        "echo '<<EOF'; cargo test",
        // Review r2 (sol): quoted operators are words; `$(...)` is matched
        // with its quotes; a whole heredoc delimiter; command-string wrappers.
        "echo '>' && cargo test",
        "out=\"$(printf ')'; cargo test)\"",
        "cat <<EOF-TEXT\ndoc\nEOF-TEXT\ncargo test",
        "watch -n 5 'cargo check'",
        "env -S 'cargo test'",
        // Review r3 (sol): information modes only where the script reads them;
        // shell options with operands before `-c`.
        "bash scripts/ci-steps build --dry-run",
        "bash scripts/ci-steps all --help",
        "bash -euo pipefail -c 'cargo test'",
        "bash -o pipefail -c 'cargo test 2>&1 | tee /tmp/t.log'",
        // TLS-61: a before/after run builds and measures.
        "scripts/cdo-before-after master",
        "python3 scripts/cdo-before-after --census master~1 master",
        // TLS-89: cargo naming the CDO workspace is a CDO run, timed or not.
        "cargo run --release --bin aldump -- --stats /work/src/cdo-baseline/Cloud",
    ];
    // TLS-89: the guarded commands that are NOT only builds or tests (gates, CDO
    // runs): refused under an untimed hold too. Every other `guarded` entry is a
    // cargo build/test, ci-steps, check-goldens or agentflow run.
    let not_builds = [
        "scripts/cdo-gate --only harness",
        "target/cdo/aldump --program-call-graph-stats /work/src/cdo-baseline/Cloud",
        "alsem analyze \"$CDO_WS\"",
        "bash scripts/land --push tools/x",
        "/usr/bin/time -v target/release-fast/aldump --snapshot-memory /work/src/cdo-baseline/Cloud",
        "cd /work/src/cdo-baseline/Cloud && aldump --stats .",
        "python3 scripts/cdo-speed --warm HEAD",
        "cd scripts && ./cdo-gate /ws",
        "scripts/cdo-before-after master",
        "python3 scripts/cdo-before-after --census master~1 master",
        "cargo run --release --bin aldump -- --stats /work/src/cdo-baseline/Cloud",
    ];
    assert!(not_builds.iter().all(|c| guarded.contains(c)));
    let free = [
        "grep -n x scripts/cdo-gate",
        "cat scripts/land",
        "git status",
        "target/cdo/aldump --stats tests/r0-corpus/x",
        "echo cargo test",
        "cargo --version",
        "ls target",
        // Review r1 (Fable): text that names a build is not a build.
        "git commit -m \"fix: slot; cargo test green\"",
        "git commit -m \"$(cat <<'EOF'\nfix\n\ncargo test --test cli\nEOF\n)\"",
        "rg \"(cargo build|cargo test)\" docs",
        "echo \"(scripts/cdo-gate --only x)\"",
        "scripts/land --dry-run tools/x",
        "scripts/cdo-gate --layout",
        "bash scripts/ci-steps fmt",
        "# cargo test later",
        "cat > notes.md <<'EOF'\ncargo test --test cli\nEOF",
        // Review r1 (sol): single quotes and a quoted heredoc run nothing.
        "git commit -m 'document $(cargo test)'",
        "printf '%s\\n' '$(cargo test)'",
        "cat <<'EOF'\n$(cargo test)\nEOF",
        // Review r2 (sol): a quoted `;` is a word; a commit message with an
        // apostrophe and parentheses in a heredoc inside `$(...)`.
        "printf '%s\\n' ';' cargo test",
        // Review r4 (sol): cdo-speed's argparse help builds nothing.
        "scripts/cdo-speed --help",
        "python3 scripts/cdo-speed --help",
        // Fable r3: a syntax check runs nothing.
        "bash -n scripts/land && bash -n scripts/cdo-gate",
        // TLS-61: its --dry-run and --help, wherever given, run nothing.
        "scripts/cdo-before-after master --dry-run",
        "python3 scripts/cdo-before-after --help",
        "git commit -m \"$(cat <<'EOF'\nfix: don't (really) run cargo test\n\nCo-Authored-By: x\nEOF\n)\"",
    ];
    for c in guarded.iter().chain(&free) {
        assert_eq!(s.guard("B", c).0, Some(0), "slot free: {c}");
    }
    // Timed (bound to a stand-in timing process; no build name can match).
    let mut timing = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let tpid = timing.id().to_string();
    let no_builds = [("CDO_SLOT_BUILD_NAMES", "tls89-none")];
    let (code, text) = s.run(
        "A",
        &[
            "acquire",
            "orchestrator",
            "speed tls89",
            "--bind",
            &tpid,
            "--timed",
        ],
        &no_builds,
    );
    assert_eq!(code, Some(0), "{text}");
    for c in guarded {
        for who in ["A", "B"] {
            let (code, text) = s.guard(who, c);
            assert_eq!(code, Some(2), "timed, {who}: {c}: {text}");
            assert!(
                text.contains("timed by orchestrator since")
                    && text.contains("(speed tls89)")
                    && text.contains("Not run:"),
                "{c}: {text}"
            );
        }
    }
    for c in free {
        assert_eq!(s.guard("B", c).0, Some(0), "timed: {c}");
    }
    timing.kill().unwrap();
    timing.wait().unwrap();
    assert!(s.wait_free());

    // Untimed: only another session's gates and CDO runs.
    let (code, text) = s.run("A", &["acquire", "orchestrator", "landing tls60"], &[]);
    assert_eq!(code, Some(0), "{text}");
    for c in guarded {
        let (code, text) = s.guard("B", c);
        if not_builds.contains(&c) {
            assert_eq!(code, Some(2), "{c}: {text}");
            assert!(
                text.contains("held by orchestrator (landing tls60)"),
                "{c}: {text}"
            );
        } else {
            assert_eq!(code, Some(0), "a build beside an untimed hold: {c}: {text}");
        }
        assert_eq!(s.guard("A", c).0, Some(0), "the holder's own: {c}");
    }
    for c in free {
        assert_eq!(s.guard("B", c).0, Some(0), "{c}");
    }
    let (code, text) = s.guard_raw("not json");
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("(allowed)"), "{text}");
    let other_tool = r#"{"tool_name":"Read","session_id":"B","tool_input":{"file_path":"x"}}"#;
    assert_eq!(s.guard_raw(other_tool).0, Some(0));
    let no_session = r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;
    assert_eq!(s.guard_raw(no_session).0, Some(0), "it cannot tell: allow");
    // A lock the probe cannot open is no holder: allow (review r1, sol).
    // Mutation: any probe failure read as held -> refused.
    // A probe that fails (a `flock` that errors, first on PATH; not file
    // permissions, which a privileged user ignores: review r2, sol).
    let broken = Slot::new();
    std::fs::create_dir_all(broken.dir()).unwrap();
    std::fs::write(broken.dir().join("lock"), "").unwrap();
    let bin = broken.0.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("flock"), "#!/bin/sh\nexit 64\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("flock"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let input = serde_json::json!({
        "tool_name": "Bash",
        "session_id": "B",
        "tool_input": { "command": "cargo test" },
    })
    .to_string();
    let (code, text) = broken.guard_raw_env(&input, &[("PATH", &path)]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("(allowed)"), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A's own BOUND hold (its land or cdo-gate): A's commands still pass, B's
    // do not (review r2: only another session's hold stops a session).
    // Mutation: the hook's own-session check limited to unbound holds -> A
    // is refused here.
    let mut land = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let pid = land.id().to_string();
    let (code, text) = s.run("A", &["acquire", "land", "landing x", "--bind", &pid], &[]);
    assert_eq!(code, Some(0), "{text}");
    let a = s.guard("A", "scripts/cdo-gate --only harness");
    let b = s.guard("B", "scripts/cdo-gate --only harness");
    land.kill().unwrap();
    land.wait().unwrap();
    assert_eq!(a.0, Some(0), "A under its own bound hold: {}", a.1);
    assert_eq!(b.0, Some(2), "B: {}", b.1);
    assert!(s.wait_free());
}

/// Review r1 (Fable): the slot stays truthful under races and bad records.
/// - 8 acquirers at once on a free slot, and 8 at once on a STALE one: exactly
///   one wins, and the status names a live holder.
/// - A status left behind that names an unrelated live process (a reused PID):
///   the slot reads free, is taken, and that process is never killed.
/// - A second acquire by the same session while its own hold is bound to a
///   process (a second `land`) does not share it: refused.
/// - A hold whose Claude process is gone but whose bound process lives is not
///   stale.
/// - An acquire with no session, no token and no --bind is refused (nobody
///   could release it).
/// - A reason with backslashes is stored as written.
///
/// Mutations: the flock dropped from the holder -> several winners; the
/// is-holder check dropped -> the unrelated process is killed; bound holds
/// shared by session -> the second acquire gets 4; the bound check dropped
/// from STALE -> the live bound hold reads STALE; the no-session refusal
/// dropped -> acquired; `echo` for `printf` in the holder -> the reason's `\n`
/// writes a second line.
#[test]
fn cdo_slot_is_truthful_under_races_and_bad_records() {
    let spawn = |s: &Slot, session: &str, extra: &[(&str, &str)]| {
        let mut cmd = std::process::Command::new("bash");
        cmd.args([SLOT, "acquire", "o", "race"])
            .env("CDO_SLOT_DIR", s.dir())
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.spawn().unwrap()
    };
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let dead = gone.id().to_string();
    for stale_first in [false, true] {
        let s = Slot::new();
        if stale_first {
            let (code, text) = s.run("old", &["acquire", "old", "gone"], &[("CLAUDE_PID", &dead)]);
            assert_eq!(code, Some(0), "{text}");
        }
        let racers: Vec<_> = (0..8).map(|i| spawn(&s, &format!("S{i}"), &[])).collect();
        let wins = racers
            .into_iter()
            .map(|mut c| c.wait().unwrap().code())
            .filter(|c| *c == Some(0))
            .count();
        assert_eq!(wins, 1, "stale first: {stale_first}");
        let holder = s.holder().expect("a status names the winner");
        // SAFETY: signal 0 only probes the process.
        assert_eq!(
            unsafe { libc::kill(holder, 0) },
            0,
            "the named holder lives"
        );
    }

    // A status left behind naming a live stranger.
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let mut stranger = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    std::fs::write(
        s.dir().join("status"),
        format!(
            "owner=old\nreason=x\nsince=2026-01-01T00:00:00Z\nsession=old\nclaude_pid={dead}\n\
             bound=\nholder_pid={}\ntoken=t\n",
            stranger.id()
        ),
    )
    .unwrap();
    assert_eq!(s.run("B", &["status"], &[]), (Some(0), "free\n".into()));
    let (code, text) = s.run("B", &["acquire", "engine", "y"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        stranger.try_wait().unwrap().is_none(),
        "the stranger was killed"
    );
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
    // The lock really held (a stale holder), its status rewritten to name the
    // stranger: still nobody kills the stranger; B is refused.
    let (code, text) = s.run("old", &["acquire", "old", "gone"], &[("CLAUDE_PID", &dead)]);
    assert_eq!(code, Some(0), "{text}");
    let real = s.holder().unwrap();
    let status = std::fs::read_to_string(s.dir().join("status")).unwrap();
    std::fs::write(
        s.dir().join("status"),
        status.replace(
            &format!("holder_pid={real}"),
            &format!("holder_pid={}", stranger.id()),
        ),
    )
    .unwrap();
    let (code, text) = s.run("B", &["acquire", "engine", "z"], &[]);
    let stranger_lives = stranger.try_wait().unwrap().is_none();
    // SAFETY: SIGKILL to the real holder this test started (before any assert,
    // so a failing run leaves no holder behind).
    unsafe { libc::kill(real, libc::SIGKILL) };
    assert_eq!(code, Some(1), "{text}");
    assert!(stranger_lives, "the stranger was killed");
    stranger.kill().unwrap();
    stranger.wait().unwrap();
    assert!(s.wait_free());

    // The same session, its hold bound to a running process; and a bound hold
    // whose Claude process is gone.
    let mut first = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let pid = first.id().to_string();
    let (code, text) = s.run("A", &["acquire", "land", "one", "--bind", &pid], &[]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = s.run("A", &["acquire", "land", "two"], &[]);
    assert_eq!(
        code,
        Some(1),
        "a bound hold is not shared by session: {text}"
    );
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(s.wait_free());
    let mut bound = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let pid = bound.id().to_string();
    let (code, text) = s.run(
        "C",
        &["acquire", "land", "cron", "--bind", &pid],
        &[("CLAUDE_PID", &dead)],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        s.run("B", &["status"], &[]).0,
        Some(1),
        "live bound hold: not stale"
    );
    bound.kill().unwrap();
    bound.wait().unwrap();
    assert!(s.wait_free());

    // No session, no token, no bind.
    let (code, text) = s.run("", &["acquire", "x", "y"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("outside a Claude session"), "{text}");
    assert_eq!(s.run("B", &["status"], &[]).0, Some(0));

    // A reason with backslashes.
    let reason = r"measure\nsession=B\cx";
    let (code, text) = s.run("A", &["acquire", "tools", reason], &[]);
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = s.run("B", &["status"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains(&format!("({reason})")), "{text}");
    assert!(!text.contains("(yours)"), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-88⟩ The queue's two other rules, each with the slot free and a waiter
/// held still:
/// - An acquire without --wait refuses while a waiter that can run waits. The
///   waiter is stopped (SIGSTOP) once queued: alive, so its ticket counts, but
///   it cannot take the slot itself after A's release.
/// - A waiter short of memory (its floor 999999 GB) marks its ticket and steps
///   aside: a no-wait acquire with no floor takes the slot past it, and the
///   waiter keeps its ticket (its place).
///
/// Mutations: `ahead` counting nothing when we hold no ticket -> the no-wait
/// acquire takes the slot (red); `ahead` counting `mem` tickets -> the acquire
/// past the memory-blocked waiter is refused (red); `ahead` removing a `mem`
/// ticket -> M re-enqueues under a new name (red).
#[test]
fn cdo_slot_queue_refuses_jumps_and_steps_aside_for_memory() {
    let s = Slot::new();
    let waiter =
        |session: &str, poll: &str, mem: &str| waiter(&s, "w", "queue", session, poll, mem);
    let ticket_text = || {
        std::fs::read_dir(s.dir().join("queue"))
            .ok()
            .and_then(|mut d| d.next())
            .and_then(|e| std::fs::read_to_string(e.ok()?.path()).ok())
    };
    let until = |what: &str, ok: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !ok() {
            assert!(start.elapsed().as_secs() < 20, "never: {what}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };

    // A waiter that can run: a no-wait acquire may not jump it.
    let (code, text) = s.run("A", &["acquire", "a", "hold"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let mut w = waiter("B", "30", "0");
    until("W queued", &|| ticket_text().is_some());
    // Stopped, W is alive (its ticket counts) but cannot take the slot itself.
    // SAFETY: SIGSTOP to the waiter this test spawned.
    unsafe { libc::kill(w.id() as i32, libc::SIGSTOP) };
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    let (code, text) = s.run("D", &["acquire", "d", "jump"], &[]);
    assert_eq!(code, Some(1), "a no-wait acquire jumped a waiter: {text}");
    assert!(text.contains("came first"), "{text}");
    w.kill().unwrap();
    w.wait().unwrap();

    // A waiter short of memory steps aside.
    let mut m = waiter("C", "1", "999999");
    let names = || -> Vec<std::ffi::OsString> {
        std::fs::read_dir(s.dir().join("queue"))
            .map(|d| d.filter_map(|e| Some(e.ok()?.file_name())).collect())
            .unwrap_or_default()
    };
    // W's dead ticket goes at M's first read of the queue; wait for both.
    until("only M waits, marked mem", &|| {
        names().len() == 1 && ticket_text().is_some_and(|t| t.trim_end().ends_with(" mem"))
    });
    let before = names();
    assert_eq!(before.len(), 1, "precondition: only M waits");
    let (code, text) = s.run("D", &["acquire", "d", "past"], &[]);
    assert_eq!(
        code,
        Some(0),
        "a memory-blocked waiter held up the slot: {text}"
    );
    assert_eq!(s.run("D", &["release"], &[]).0, Some(0));
    // M keeps its place: past its 1 s poll, its ticket is the same one.
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert_eq!(names(), before, "M lost its place in the queue");
    m.kill().unwrap();
    m.wait().unwrap();
}

/// ⟨TLS-88⟩ Waiters are served in arrival order. While A holds the slot, W1 then
/// W2 wait. When A releases, A's own immediate acquire goes behind them and
/// times out. W1 takes the slot and W2 keeps waiting. W2 is then killed outright
/// (no trap removes its ticket), and its dead ticket must not block the next
/// acquire.
///
/// Mutations: the queue check dropped from `acquire` -> A re-takes the slot
/// ahead of W1 (red); `ahead` keeping dead tickets -> the acquire after W2's
/// SIGKILL waits forever (red).
#[test]
fn cdo_slot_serves_waiters_in_arrival_order() {
    let s = Slot::new();
    let waiter = |session: &str, owner: &str| waiter(&s, owner, "fifo", session, "1", "0");
    let tickets = || {
        std::fs::read_dir(s.dir().join("queue"))
            .map(|d| d.count())
            .unwrap_or(0)
    };
    let until = |what: &str, ok: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !ok() {
            assert!(start.elapsed().as_secs() < 20, "never: {what}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    let owner = || {
        std::fs::read_to_string(s.dir().join("status"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("owner=").map(str::to_string))
            })
    };

    let (code, text) = s.run("A", &["acquire", "a", "first"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let mut w1 = waiter("B", "w1");
    until("W1 queued", &|| tickets() == 1);
    let mut w2 = waiter("C", "w2");
    until("W2 queued", &|| tickets() == 2);

    // A releases and at once asks again: it is behind W1 and W2 now.
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    let (code, text) = s.run(
        "A",
        &["acquire", "a", "again", "--wait", "--timeout", "3"],
        &[],
    );
    assert_eq!(
        code,
        Some(1),
        "A re-took the slot ahead of the waiters: {text}"
    );
    assert_eq!(w1.wait().unwrap().code(), Some(0), "W1 took the slot");
    assert_eq!(owner().as_deref(), Some("w1"));
    assert_eq!(tickets(), 1, "W2 still waiting");

    // W2 dies outright while waiting; W1 releases; the dead ticket is skipped.
    w2.kill().unwrap();
    w2.wait().unwrap();
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
    let (code, text) = s.run(
        "D",
        &["acquire", "d", "after", "--wait", "--timeout", "10"],
        &[],
    );
    assert_eq!(
        code,
        Some(0),
        "a dead waiter's ticket blocked the slot: {text}"
    );
    assert_eq!(tickets(), 0, "every ticket gone");
    assert_eq!(s.run("D", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-89⟩ A timed hold never starts beside a build: `run --timed` marks the
/// hold, waits for the build processes already running, and starts once they
/// end; still running at `CDO_SLOT_TIMED_WAIT_S` it refuses (exit 6, naming
/// their PIDs), runs nothing and frees the slot. Its own ancestors (the cargo
/// test that runs a timed script) are not waited for. An untimed run does not
/// wait. The mark lives only as long as its bound process.
///
/// Review r1 (sol): a script run with an option operand (`bash -o pipefail
/// <script>`) still counts; a timing whose shared hold is released while it
/// waits refuses (exit 6) rather than start with no hold; a reason with a CR is
/// refused (a text-mode reader would see a fake second status line).
///
/// Review r1 (Fable): a build SCRIPT (ci-steps, between its cargo steps) counts
/// too; a second timing in a shared hold waits for builds as well (it got
/// "timed already" at once before) and keeps its own mark when the first
/// timing ends; a refused `acquire --bind --timed` with no session or token
/// frees the slot it took.
///
/// The stand-in build is bash run through a symlink named `t89s<test pid>`
/// (matched by `pgrep -x`; unique, as `pgrep -x` sees the whole machine),
/// running two commands so it does not exec into `sleep`. The stand-in build
/// script is a file `ci-steps-t89-<test pid>` run by bash.
///
/// Mutations (2026-11-18): the timeout's `return 6` made `break` -> the run
/// starts beside the build (red); the ancestor exclusion dropped from
/// `foreign_builds` -> the run under the stand-in build refuses itself (red);
/// the liveness check dropped from `timed_valid` -> the mark outlives its
/// killed process (red). Each restored, green. Review r1 (2026-11-18):
/// `script_builds` dropped from `foreign_builds` -> the run starts beside the
/// script (red); the old early return on "timed already" restored -> the
/// second timing starts beside the build (red); the `end_holder` on a refused
/// acquire dropped -> the slot stays held (red). Each restored, green.
/// Review r1 (sol, 2026-11-18): the `-o`/`-O` operand skip dropped from
/// `script_operand` -> `bash -o pipefail <script>` is missed and the timing
/// starts (red); the recheck after the drain made `if false` -> a timing starts
/// after its hold was released (red); the CR refusal dropped from the reason
/// check -> the reason is taken (red). Each restored, green.
/// Review r2 (sol): a timing that took the slot and is refused after its hold
/// changed hands leaves the new holder alone; a python driver with an attached
/// option value holding an `m` (`-Xfrozen_modules=off`) counts. Mutations
/// (2026-11-18): the cleanup ending whatever holder is current (`state;
/// end_holder "$holder_pid"`) -> the replacement hold is killed, status free
/// (red); the word-wide `*[cm]*` check put back -> the driver is missed (red;
/// with `-Xdev` it stayed green, so the test now uses a value with an `m`).
/// Each restored, green.
#[test]
fn cdo_slot_timed_waits_for_builds_and_refuses_beside_one() {
    let s = Slot::new();
    let fake_name = format!("t89s{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    // A symlink, not a copy: a copy still open for writing when another test
    // thread forks fails to exec ("Text file busy"). Its process name is the
    // link's name.
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let names = ("CDO_SLOT_BUILD_NAMES", fake_name.as_str());
    let build = || {
        Reap(
            std::process::Command::new(&fake)
                .args(["-c", "sleep 600; true"])
                .spawn()
                .unwrap(),
        )
    };
    let ran = s.0.path().join("ran");
    let touch = ran.to_str().unwrap();

    // A build runs: refused after the wait, nothing run, the slot free.
    let mut b = build();
    let bpid = b.id();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "touch", touch],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(code, Some(6), "{text}");
    assert!(
        text.contains("--timed refused") && text.contains(&format!("PID {bpid} ({fake_name})")),
        "{text}"
    );
    assert!(!ran.exists(), "the timed command ran beside a build");
    assert_eq!(
        s.run("A", &["status"], &[]).0,
        Some(0),
        "the refused run freed the slot"
    );

    // An untimed run does not wait for it.
    let (code, text) = s.run(
        "A",
        &["run", "lane", "plain", "--", "touch", touch],
        &[names],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(ran.exists());
    std::fs::remove_file(&ran).unwrap();

    // The build ends during the wait: the timed run starts then. The build is
    // ended only once the run says it waits for it (a fixed delay raced the
    // run's first scan on a loaded machine: the build was gone before it).
    let mut timed = Reap(
        std::process::Command::new("bash")
            .args([
                SLOT, "run", "lane", "timing", "--timed", "--", "touch", touch,
            ])
            .env("CDO_SLOT_DIR", s.dir())
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            .env("CDO_SLOT_TIMED_WAIT_S", "60")
            .env("CDO_SLOT_BUILD_NAMES", &fake_name)
            .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
            .env("CLAUDE_CODE_SESSION_ID", "A")
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let want = format!("waiting for build processes to end: PID {bpid}");
    let mut text = String::new();
    {
        use std::io::BufRead;
        let lines = std::io::BufReader::new(timed.0.stdout.take().unwrap()).lines();
        let mut ended = false;
        for line in lines {
            let line = line.unwrap();
            if line.contains(&want) && !ended {
                b.kill().unwrap();
                b.wait().unwrap();
                ended = true;
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    let code = timed.0.wait().unwrap().code();
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains(&want), "{text}");
    assert!(ran.exists());

    // Run under a build process (its ancestor): not waited for; inside, the
    // status names the timed run.
    let inner = format!("bash {SLOT} run lane inner --timed -- bash -c 'bash {SLOT} status; true'");
    let out = std::process::Command::new(&fake)
        .args(["-c", &format!("{inner}; true")])
        .env("CDO_SLOT_DIR", s.dir())
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("CDO_SLOT_POLL_S", "1")
        .env("CDO_SLOT_TIMED_WAIT_S", "2")
        .env("CDO_SLOT_BUILD_NAMES", &fake_name)
        .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
        .env("CLAUDE_CODE_SESSION_ID", "A")
        .env_remove("CLAUDE_PID")
        .env_remove("CDO_SLOT_TOKEN")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("timed by lane since") && text.contains("(inner)"),
        "{text}"
    );
    assert!(!text.contains("refused"), "{text}");

    // A timed mark on a hold held by hand ends with its bound process; the hold stays.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let mut timing = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let tpid = timing.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "measure", "--bind", &tpid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "shared: {text}");
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(text.contains("timed by lane since"), "{text}");
    timing.kill().unwrap();
    timing.wait().unwrap();
    let (code, text) = s.run("B", &["status"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        !text.contains("timed by"),
        "the mark outlived its process: {text}"
    );

    // Two timings in that hold: the second waits for a build too, and keeps
    // its own mark when the first ends.
    let mut t1 = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut t2 = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let (t1pid, t2pid) = (t1.id().to_string(), t2.id().to_string());
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "first", "--bind", &t1pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    let mut b2 = build();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(
        code,
        Some(6),
        "the second timing started beside a build: {text}"
    );
    b2.kill().unwrap();
    b2.wait().unwrap();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    t1.kill().unwrap();
    t1.wait().unwrap();
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(
        text.contains("timed by lane since") && text.contains("(second)"),
        "the second timing lost its cover: {text}"
    );
    t2.kill().unwrap();
    t2.wait().unwrap();
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A build script between its cargo steps (no build-named process) counts.
    let script_name = format!("ci-steps-t89-{}", std::process::id());
    let script = s.0.path().join(&script_name);
    std::fs::write(&script, "sleep 600\ntrue\n").unwrap();
    // Run with an option operand before the script (review r1, sol): `pipefail`
    // is no script name.
    let mut sb = Reap(
        std::process::Command::new("bash")
            .args(["-o", "pipefail"])
            .arg(&script)
            .spawn()
            .unwrap(),
    );
    let sbpid = sb.id();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "true"],
        &[
            names,
            ("CDO_SLOT_BUILD_SCRIPTS", script_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "2"),
        ],
    );
    assert_eq!(
        code,
        Some(6),
        "the timing started beside a build script: {text}"
    );
    assert!(
        text.contains(&format!("PID {sbpid} ({script_name})")),
        "{text}"
    );

    // A refused `acquire --bind --timed` (no session, no token) frees what it took.
    let mut t3 = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let t3pid = t3.id().to_string();
    let (code, text) = s.run(
        "",
        &["acquire", "cron", "measure", "--bind", &t3pid, "--timed"],
        &[
            names,
            ("CDO_SLOT_BUILD_SCRIPTS", script_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "1"),
        ],
    );
    sb.kill().unwrap();
    sb.wait().unwrap();
    assert_eq!(code, Some(6), "{text}");
    assert_eq!(
        s.run("B", &["status"], &[]).0,
        Some(0),
        "the refused acquire kept the slot"
    );
    t3.kill().unwrap();
    t3.wait().unwrap();

    // Review r1 (sol): the hold it shares ends while it waits for a build ->
    // refused once the build ends, not a start with no hold.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let mut b3 = build();
    let mut t4 = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut waiting = Reap(
        std::process::Command::new("bash")
            .args([SLOT, "acquire", "lane", "drained", "--bind"])
            .arg(t4.id().to_string())
            .arg("--timed")
            .env("CDO_SLOT_DIR", s.dir())
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            .env("CDO_SLOT_TIMED_WAIT_S", "60")
            .env("CDO_SLOT_BUILD_NAMES", &fake_name)
            .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
            .env("CLAUDE_CODE_SESSION_ID", "A")
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    b3.kill().unwrap();
    b3.wait().unwrap();
    assert_eq!(
        waiting.wait().unwrap().code(),
        Some(6),
        "a timing started after its hold ended"
    );
    t4.kill().unwrap();
    t4.wait().unwrap();

    // Review r2 (sol): a timing that TOOK the slot, whose session releases it
    // while it waits, and another session takes it -> refused, and the other
    // session's hold is left alone.
    let mut b4 = build();
    let mut t5 = Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    );
    let mut took = Reap(
        std::process::Command::new("bash")
            .args([SLOT, "acquire", "lane", "took", "--bind"])
            .arg(t5.id().to_string())
            .arg("--timed")
            .env("CDO_SLOT_DIR", s.dir())
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            .env("CDO_SLOT_TIMED_WAIT_S", "60")
            .env("CDO_SLOT_BUILD_NAMES", &fake_name)
            .env("CDO_SLOT_BUILD_SCRIPTS", "tls89-none")
            .env("CLAUDE_CODE_SESSION_ID", "C")
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(s.run("C", &["release"], &[]).0, Some(0));
    let (code, text) = s.run("B", &["acquire", "other", "replacement"], &[]);
    assert_eq!(code, Some(0), "{text}");
    b4.kill().unwrap();
    b4.wait().unwrap();
    assert_eq!(took.wait().unwrap().code(), Some(6));
    let (code, text) = s.run("D", &["status"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("held by other (replacement)"),
        "the refused timing ended another session's hold: {text}"
    );
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
    t5.kill().unwrap();
    t5.wait().unwrap();

    // Review r2 (sol): a python build driver with an attached option value
    // whose value holds an `m` (`-Xfrozen_modules=off`: no `-m` option) still
    // counts. (`-Xdev` proved nothing: its value has neither `c` nor `m`, and
    // the old word-wide check passed it too.)
    let py_name = format!("check-goldens-t89-{}", std::process::id());
    let py = s.0.path().join(&py_name);
    std::fs::write(&py, "import time\ntime.sleep(600)\n").unwrap();
    let mut pb = Reap(
        std::process::Command::new("python3")
            .arg("-Xfrozen_modules=off")
            .arg(&py)
            .spawn()
            .unwrap(),
    );
    let pbpid = pb.id();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "true"],
        &[
            names,
            ("CDO_SLOT_BUILD_SCRIPTS", py_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "2"),
        ],
    );
    pb.kill().unwrap();
    pb.wait().unwrap();
    assert_eq!(
        code,
        Some(6),
        "the timing started beside a python driver: {text}"
    );
    assert!(text.contains(&format!("PID {pbpid} ({py_name})")), "{text}");

    // Review r1 (sol): a reason with a CR (which text-mode readers break into a
    // second line, where `timed by` lives) is refused.
    let (code, text) = s.run("A", &["acquire", "lane", "probe\rtimed by x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("one-line reason"), "{text}");
}
