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

/// The hook: while session A holds the slot, session B's builds, tests, gates
/// and CDO runs are refused (exit 2, naming the holder), in every shape a
/// command line takes; reading a script, git, a fixture `aldump` and A's own
/// commands pass; with the slot free everything passes; input it cannot read
/// passes (a hook bug must not stop every session).
///
/// Mutations: the wrapper skipping dropped -> `nice`, `timeout -k`, `taskset`
/// shapes pass; the first-word rule dropped (every word checked) -> `grep ...
/// scripts/cdo-gate` is refused; the quote-aware reading dropped (word by
/// word) -> commit messages and greps naming `cargo test` are refused; the
/// heredoc skip dropped -> a heredoc line `cargo test` is refused; the
/// information modes dropped -> `land --dry-run` is refused; the own-session
/// check dropped -> A is refused.
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
    ];
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
        "git commit -m \"$(cat <<'EOF'\nfix: don't (really) run cargo test\n\nCo-Authored-By: x\nEOF\n)\"",
    ];
    for c in guarded.iter().chain(&free) {
        assert_eq!(s.guard("B", c).0, Some(0), "slot free: {c}");
    }
    let (code, text) = s.run("A", &["acquire", "orchestrator", "landing tls60"], &[]);
    assert_eq!(code, Some(0), "{text}");
    for c in guarded {
        let (code, text) = s.guard("B", c);
        assert_eq!(code, Some(2), "{c}: {text}");
        assert!(
            text.contains("held by orchestrator (landing tls60)"),
            "{c}: {text}"
        );
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
    let a = s.guard("A", "cargo test --test cli");
    let b = s.guard("B", "cargo test --test cli");
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
