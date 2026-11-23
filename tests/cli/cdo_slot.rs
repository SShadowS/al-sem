//! ⟨TLS-60⟩ `scripts/cdo-slot` (the one CDO slot) and its PreToolUse hook
//! `scripts/claude-hooks/cdo-slot-guard`. Every test uses a slot folder of its
//! own (`CDO_SLOT_DIR`), never the tentacle's.
//!
//! ⟨TLS-89⟩ Only kernel locks decide; files describe: the hold is a flock its
//! holder keeps, a timing is a shared flock its timer keeps, and a process that
//! dies drops its locks. Many tests here kill a holder or a timer outright and
//! check that the next probe sees exactly what the locks say.

const SLOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot");
const GUARD: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/claude-hooks/cdo-slot-guard"
);

/// A slot folder. When it drops, every process whose environment names it
/// (holders, keepers, waiters) is killed. Its watchdog does the same after
/// WATCHDOG, so a hung test fails instead of holding the CDO slot for hours
/// (TLS-89: one did, 2h43m).
struct Slot(
    tempfile::TempDir,
    // Never read: held for its Drop, which stops the watchdog.
    #[allow(dead_code)] Watchdog,
);

// 600 s: it catches hangs, and a landing's load stretched the slowest test past
// 300 s twice in TLS-102's stress run (that test now runs its calls 8 at once).
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(600);

/// `cmd` without any slot or Claude setting this test process inherited (a
/// landing runs the suite inside its own hold: its token, its tuning, its
/// session), so a slot test gives the same result inside a hold and outside.
fn scrubbed(mut cmd: std::process::Command) -> std::process::Command {
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CDO_SLOT_") || k.starts_with("CLAUDE_") || k == "CDO_WS" {
            cmd.env_remove(k);
        }
    }
    cmd
}

/// SIGKILL to every process whose environment names `dir` as CDO_SLOT_DIR.
fn kill_users(dir: &std::path::Path) -> Vec<u32> {
    let want = format!("CDO_SLOT_DIR={}", dir.display());
    let mut killed = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let env = std::fs::read(e.path().join("environ")).unwrap_or_default();
        if env.split(|b| *b == 0).any(|v| v == want.as_bytes()) {
            // SAFETY: SIGKILL to a process of this test's own slot folder.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            killed.push(pid);
        }
    }
    killed
}

struct Watchdog {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Watchdog {
    fn new(dir: std::path::PathBuf) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let until = std::time::Instant::now() + WATCHDOG;
            while std::time::Instant::now() < until {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let killed = kill_users(&dir);
            eprintln!("cdo_slot watchdog: the test hung for {WATCHDOG:?}; killed {killed:?}");
        });
        Watchdog { stop }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The environment every cdo-slot call of these tests gets: no memory floor,
/// a 1 s poll, no grace wait (a test sets it to test it), and no build name or
/// script that any real process on this machine could have.
const BASE_ENV: [(&str, &str); 5] = [
    ("CDO_SLOT_MEM_MIN_GB", "0"),
    ("CDO_SLOT_POLL_S", "1"),
    ("CDO_SLOT_TIMED_GRACE_S", "0"),
    ("CDO_SLOT_BUILD_NAMES", "tls89-none"),
    ("CDO_SLOT_BUILD_SCRIPTS", "tls89-none"),
];

impl Slot {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dog = Watchdog::new(tmp.path().join("slot"));
        Slot(tmp, dog)
    }
    fn dir(&self) -> std::path::PathBuf {
        self.0.path().join("slot")
    }
    /// A `cdo-slot <args>` command as Claude session `session`, with BASE_ENV
    /// and then `env` (`CLAUDE_PID` and `CDO_SLOT_TOKEN` only when `env` gives them).
    fn cmd(&self, session: &str, args: &[&str], env: &[(&str, &str)]) -> std::process::Command {
        self.cmd_at(SLOT.as_ref(), session, args, env)
    }
    /// [`Slot::cmd`] with the cdo-slot at `script` (one in a [`link_folder`]).
    fn cmd_at(
        &self,
        script: &std::path::Path,
        session: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> std::process::Command {
        let mut cmd = scrubbed(std::process::Command::new("bash"));
        cmd.arg(script)
            .args(args)
            .env("CDO_SLOT_DIR", self.dir())
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN");
        for (k, v) in BASE_ENV.iter().chain(env) {
            cmd.env(k, v);
        }
        cmd
    }
    /// Wait (up to 20 s) until a test pause hook's `paused.<pid>` file appears,
    /// then remove it, so a later wait sees only a new one.
    fn wait_paused(&self) {
        until("a pause hook was reached", &|| {
            let found: Vec<_> = std::fs::read_dir(self.dir())
                .map(|d| {
                    d.flatten()
                        .filter(|e| e.file_name().to_string_lossy().starts_with("paused."))
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default();
            for p in &found {
                let _ = std::fs::remove_file(p);
            }
            !found.is_empty()
        });
    }
    /// Ends the window of the process a pause hook holds (it waits for this
    /// file, at most its hook's seconds), once the test's steps in it are done.
    fn resume(&self) {
        std::fs::write(self.dir().join("resume"), "").unwrap();
    }
    /// `cdo-slot <args>`, run to its end; (exit, stdout + stderr).
    fn run(&self, session: &str, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
        let out = self.cmd(session, args, env).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
    /// `cdo-slot <args>` in the background, its output discarded.
    fn bg(&self, session: &str, args: &[&str], env: &[(&str, &str)]) -> Reap {
        Reap(
            self.cmd(session, args, env)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    /// `cdo-slot probe`'s decision line.
    fn probe(&self) -> String {
        let (_, text) = self.run("probe", &["probe"], &[]);
        text.lines().next().unwrap_or_default().to_string()
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
        let mut cmd = scrubbed(std::process::Command::new(GUARD));
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
    /// The holder PID the status names (a keeper, or a run).
    fn holder(&self) -> Option<i32> {
        std::fs::read_to_string(self.dir().join("status"))
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("holder_pid="))?
            .parse()
            .ok()
    }
    /// Polls (up to POLL_BUDGET) until `status` says free.
    fn wait_free(&self) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < POLL_BUDGET {
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
        kill_users(&self.dir());
    }
}

/// How long a poll for an expected state may take. Generous on purpose: only a
/// failing run waits it out, and a landing's full suite slows every step
/// (TLS-102: three slot tests failed only inside landings).
const POLL_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// Polls `ok` every 50 ms; panics naming `what` after POLL_BUDGET.
fn until(what: &str, ok: &dyn Fn() -> bool) {
    let start = std::time::Instant::now();
    while !ok() {
        assert!(start.elapsed() < POLL_BUDGET, "never: {what}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The PIDs of this slot's keepers of one kind (`b"_keep\0timed\0"` or
/// `b"_keep\0hold\0"`). Only the keeper itself: each `$(...)` it runs forks a
/// subshell with the same command line and environment for a moment, and a
/// count by command line alone saw two (TLS-102's landing).
fn keepers(s: &Slot, kind: &[u8]) -> Vec<i32> {
    let want = format!("CDO_SLOT_DIR={}", s.dir().display());
    let is_keeper = |p: i32| {
        let cmd = std::fs::read(format!("/proc/{p}/cmdline")).unwrap_or_default();
        let env = std::fs::read(format!("/proc/{p}/environ")).unwrap_or_default();
        cmd.ends_with(kind) && env.split(|b| *b == 0).any(|v| v == want.as_bytes())
    };
    let parent = |p: i32| -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/{p}/stat")).ok()?;
        stat.rsplit_once(") ")?.1.split(' ').nth(1)?.parse().ok()
    };
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<i32>().ok())
        .filter(|&p| is_keeper(p) && !parent(p).is_some_and(is_keeper))
        .collect()
}

/// The hook's answers for `cmds` from session `who`, in order, 8 calls at a
/// time: each call is its own python and bash, and one after another the guard
/// test's ~400 passed its watchdog under a landing's load (TLS-102).
fn guard_many(s: &Slot, who: &str, cmds: &[&str]) -> Vec<(Option<i32>, String)> {
    let size = cmds.len().div_ceil(8).max(1);
    std::thread::scope(|sc| {
        let parts: Vec<_> = cmds
            .chunks(size)
            .map(|part| sc.spawn(move || part.iter().map(|c| s.guard(who, c)).collect::<Vec<_>>()))
            .collect();
        parts.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// A process that sleeps 600 s: a stand-in for a bound process.
fn sleeper() -> Reap {
    Reap(
        std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap(),
    )
}

/// A stand-in process of slot `s` (a build, a build script, a lock holder): its
/// output goes nowhere and its environment names the slot folder, so the
/// slot's cleanup ends it and every child it started (a killed `bash -c
/// "sleep 600; true"` leaves its `sleep`, which nextest reported as a leak).
fn stand_in(s: &Slot, program: impl AsRef<std::ffi::OsStr>, args: &[&std::ffi::OsStr]) -> Reap {
    Reap(
        scrubbed(std::process::Command::new(program))
            .args(args)
            .env("CDO_SLOT_DIR", s.dir())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

/// ⟨TLS-90⟩ A spawned process (a waiter, a stand-in) that is killed and reaped
/// when it drops, so a failing assertion cannot leak it. (A test process killed
/// outright runs no `Drop`, so that case can still leak; the slot's watchdog
/// covers it.)
pub(crate) struct Reap(pub(crate) std::process::Child);

/// ⟨TLS-90⟩ The one way the slot tests spawn a waiter: `acquire <owner> <reason>
/// --wait` as session `session`, polling every `poll` seconds with memory floor
/// `mem` GB, held in [`Reap`].
fn waiter(s: &Slot, owner: &str, reason: &str, session: &str, poll: &str, mem: &str) -> Reap {
    s.bg(
        session,
        &["acquire", owner, reason, "--wait"],
        &[("CDO_SLOT_MEM_MIN_GB", mem), ("CDO_SLOT_POLL_S", poll)],
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

/// The slot frees itself, because the kernel drops a dead process's locks:
/// - A hand hold whose Claude process is gone: its keeper ends itself and the
///   slot is free (there is no STALE state any more: nothing has to guess).
/// - A hold bound to a process (`--bind`, which land and cdo-gate use) ends
///   when that process is killed outright, even before it is reaped.
/// - `run` holds the slot in its own process: free after its command succeeds
///   or fails, and when its whole process group is killed.
/// - Too little memory refuses.
///
/// Mutations: the keeper's Claude-PID watch made a sleep -> the dead session's
/// hold never frees (red); the keeper's bind watch made a sleep -> the slot
/// outlives the killed process (red).
#[test]
fn cdo_slot_frees_itself() {
    let s = Slot::new();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let dead = gone.id().to_string();
    let (code, text) = s.run("C", &["acquire", "cron", "z"], &[("CLAUDE_PID", &dead)]);
    assert_eq!(code, Some(0), "{text}");
    assert!(s.wait_free(), "a dead session's hold stayed");

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
    // Its own process group: SIGKILL to the group ends `run` and its `sleep`.
    use std::os::unix::process::CommandExt;
    let mut run = s.cmd("A", &["run", "tools", "k", "--", "sleep", "600"], &[]);
    let mut run = run
        .stdout(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let group = run.id() as i32;
    until("run took the slot", &|| s.holder() == Some(group));
    // SAFETY: SIGKILL to the process group this test created.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    run.wait().unwrap();
    assert_eq!(
        s.run("B", &["status"], &[]).0,
        Some(0),
        "a killed run left the slot held"
    );

    let (code, text) = s.run(
        "A",
        &["acquire", "tools", "m"],
        &[("CDO_SLOT_MEM_MIN_GB", "999999")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("MemAvailable below 999999 GB"), "{text}");
}

/// ⟨TLS-89⟩ A `run`'s hold and timing are its command's too (owner decision
/// 2026-11-19: inherit): the run's own process killed outright while its
/// command lives leaves the slot held and timed, since that command still runs
/// (and still uses the memory); it frees the moment the command ends. A keeper
/// killed outright frees its hold at once, with no wait for anyone to notice.
///
/// Mutations (2026-11-19): `run` taking its hold in a keeper (`acquire ... 0`)
/// -> free while the orphaned command runs (red); `state` deciding from the
/// status file (`[[ -s $status_file ]]` for the lock probe) -> held after the
/// keeper's SIGKILL (red). Each restored, green.
#[test]
fn cdo_slot_a_hold_lives_exactly_as_long_as_its_processes() {
    let s = Slot::new();
    let mut run = s.bg(
        "A",
        &["run", "tools", "orphan", "--timed", "--", "sleep", "600"],
        &[],
    );
    let rpid = run.id() as i32;
    // The run's command (its `sleep 600`, which carries the slot folder in its
    // environment).
    let command = || -> Vec<i32> {
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().parse::<i32>().ok())
            .filter(|p| {
                std::fs::read(format!("/proc/{p}/cmdline"))
                    .map(|c| c == b"sleep\x00600\x00")
                    .unwrap_or(false)
                    && std::fs::read(format!("/proc/{p}/environ"))
                        .map(|e| {
                            let want = format!("CDO_SLOT_DIR={}", s.dir().display());
                            e.split(|b| *b == 0).any(|v| v == want.as_bytes())
                        })
                        .unwrap_or(false)
            })
            .collect()
    };
    // Killed only once its command runs (TLS-89's 3x-under-load check: the
    // timing lock is taken before the build wait, so `timed=1` alone could
    // let the kill land before the command started, and nothing inherited).
    until("the run's command runs, timed", &|| {
        command().len() == 1 && s.probe() == "held=1 own=0 timed=1"
    });
    // SAFETY: SIGKILL to the run's bash only; its `sleep` lives on.
    unsafe { libc::kill(rpid, libc::SIGKILL) };
    run.0.wait().unwrap();
    for _ in 0..10 {
        assert_eq!(
            s.probe(),
            "held=1 own=0 timed=1",
            "the hold ended while its command ran"
        );
    }
    let kids = command();
    assert_eq!(kids.len(), 1, "precondition: the orphaned command lives");
    // SAFETY: SIGKILL to the orphaned `sleep` of this test's run.
    unsafe { libc::kill(kids[0], libc::SIGKILL) };
    until("the hold ended with its command", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });

    let (code, text) = s.run("A", &["acquire", "tools", "keeper"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let keeper = s.holder().expect("the status names the keeper");
    // SAFETY: SIGKILL to the keeper this test's acquire started.
    unsafe { libc::kill(keeper, libc::SIGKILL) };
    until("the keeper is gone", &|| {
        !std::path::Path::new(&format!("/proc/{keeper}")).exists()
            || std::fs::read_to_string(format!("/proc/{keeper}/stat"))
                .map(|t| t.contains(") Z "))
                .unwrap_or(true)
    });
    assert_eq!(
        s.run("B", &["status"], &[]),
        (Some(0), "free\n".into()),
        "a killed keeper's hold outlived it"
    );
}

/// The hook: while a timing runs (TLS-89), every session's builds, tests, gates
/// and CDO runs are refused (exit 2, naming the timing and the holder), the
/// holder's own included, in every shape a command line takes. While A holds
/// the slot untimed, B's gates and CDO runs are refused (naming the holder) and
/// B's builds and tests pass, as do all of A's. Reading a script, git and a
/// fixture `aldump` always pass; with the slot free everything passes; input it
/// cannot read passes (a hook bug must not stop every session).
///
/// TLS-89 mutations (2026-11-18): `BUILDS = False` dropped from the untimed
/// path -> B's `cargo test --test cli` refused under the untimed hold (red);
/// the timed branch dropped -> A's own `cargo test --test cli` passes under the
/// timing (red). Both restored, green.
///
/// Mutations: the wrapper skipping dropped -> `nice`, `timeout -k`, `taskset`
/// shapes pass; the first-word rule dropped (every word checked) -> `grep ...
/// scripts/cdo-gate` is refused; the quote-aware reading dropped (word by
/// word) -> commit messages and greps naming `cargo test` are refused; the
/// heredoc skip dropped -> a heredoc line `cargo test` is refused; the
/// information modes dropped -> `land --dry-run` is refused; the own-session
/// check dropped -> A is refused; TLS-61: the `cdo-before-after` entry of
/// `INFO_ANYWHERE` dropped -> its `--dry-run` and `--help` are refused.
/// TLS-89 review r7 (2026-11-19): the `cdo-slot` branch of `script_guarded`
/// dropped -> `scripts/cdo-slot run ... -- cargo build` passes the timing (red).
/// Review r8: `run`'s own words parsed as "the first `--`" again -> the
/// `'--'` reason hides the build (red); `cdo-slot` dropped from the PATH names
/// -> the bare `cdo-slot run ... -- cargo build` passes (red).
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
        // Review r3 (sol): a python option's separate value is no script name;
        // agentflow from PATH.
        "python3 -X dev scripts/cdo-before-after master",
        "agentflow run --name ci -- true",
        // Review r4 (sol): clustered options ending in X take the next word.
        "python3 -IX dev scripts/cdo-before-after master",
        // Review r5 (sol): an attached value may itself end in W or X.
        "python3 -Wignore:W scripts/cdo-before-after master",
        // Review r7 (sol): the slot's own `run` wraps a command.
        "scripts/cdo-slot run lane rebuild -- cargo build",
        "bash /work/alsem/scripts/cdo-slot run lane r --timed -- timeout 60 cargo test",
        // Review r8 (sol): from PATH, and a reason that is itself `--`.
        "cdo-slot run lane x -- cargo build",
        "scripts/cdo-slot run lane '--' -- cargo build",
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
        "python3 -X dev scripts/cdo-before-after master",
        "python3 -IX dev scripts/cdo-before-after master",
        "python3 -Wignore:W scripts/cdo-before-after master",
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
        // Review r7 (sol): the slot itself, and its `run` of a harmless command.
        "scripts/cdo-slot status",
        "bash scripts/cdo-slot run lane x -- git status",
        "scripts/cdo-slot run lane '--' --timeout 5 -- git status",
    ];
    // The hook's calls run 8 at a time (`guard_many`): one after another, the
    // ~400 calls passed the 300 s watchdog under a landing's load (TLS-102).
    let both: Vec<&str> = guarded.iter().chain(&free).copied().collect();
    for (c, (code, text)) in both.iter().zip(guard_many(&s, "B", &both)) {
        assert_eq!(code, Some(0), "slot free: {c}: {text}");
    }
    // Timed (bound to a stand-in timing process; no build name can match).
    let mut timing = sleeper();
    let tpid = timing.id().to_string();
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
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    for who in ["A", "B"] {
        for (c, (code, text)) in guarded.iter().zip(guard_many(&s, who, &guarded)) {
            assert_eq!(code, Some(2), "timed, {who}: {c}: {text}");
            assert!(
                text.contains("timed by orchestrator since")
                    && text.contains("(speed tls89)")
                    && text.contains("Not run:"),
                "{c}: {text}"
            );
        }
    }
    for (c, (code, text)) in free.iter().zip(guard_many(&s, "B", &free)) {
        assert_eq!(code, Some(0), "timed: {c}: {text}");
    }
    timing.kill().unwrap();
    timing.wait().unwrap();
    assert!(s.wait_free());

    // Untimed: only another session's gates and CDO runs.
    let (code, text) = s.run("A", &["acquire", "orchestrator", "landing tls60"], &[]);
    assert_eq!(code, Some(0), "{text}");
    let (from_b, from_a) = (guard_many(&s, "B", &guarded), guard_many(&s, "A", &guarded));
    for ((c, (code, text)), (a_code, a_text)) in guarded.iter().zip(from_b).zip(from_a) {
        if not_builds.contains(c) {
            assert_eq!(code, Some(2), "{c}: {text}");
            assert!(
                text.contains("held by orchestrator (landing tls60)"),
                "{c}: {text}"
            );
        } else {
            assert_eq!(code, Some(0), "a build beside an untimed hold: {c}: {text}");
        }
        assert_eq!(a_code, Some(0), "the holder's own: {c}: {a_text}");
    }
    for (c, (code, text)) in free.iter().zip(guard_many(&s, "B", &free)) {
        assert_eq!(code, Some(0), "{c}: {text}");
    }
    let (code, text) = s.guard_raw("not json");
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("(allowed)"), "{text}");
    let other_tool = r#"{"tool_name":"Read","session_id":"B","tool_input":{"file_path":"x"}}"#;
    assert_eq!(s.guard_raw(other_tool).0, Some(0));
    let no_session = r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;
    assert_eq!(s.guard_raw(no_session).0, Some(0), "it cannot tell: allow");
    // A probe that fails (a `flock` that errors, first on PATH; not file
    // permissions, which a privileged user ignores: review r2, sol) is no
    // holder: allow (review r1, sol). Mutation: any probe failure read as
    // held -> refused.
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
    let mut land = sleeper();
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
/// - 8 acquirers at once on a free slot: exactly one wins, and the status names
///   a live holder.
/// - A status left behind that names an unrelated live process (a reused PID)
///   and a hold id nobody holds: the slot reads free, is taken, and that process
///   is never killed. A status of the LIVE hold edited to name the stranger:
///   B is refused, `release` ends the real keeper (found by its lock), and the
///   stranger lives.
/// - A second acquire by the same session while its own hold is bound to a
///   process (a second `land`) does not share it: refused.
/// - A bound hold whose Claude process is gone lives as long as its process.
/// - An acquire with no session, no token and no --bind is refused (nobody
///   could release it).
/// - A reason with backslashes is stored as written.
///
/// Mutations: the flock dropped from `take_hold` -> several winners; `release`
/// ending the status's holder_pid -> the stranger is killed (and the real
/// keeper lives); bound holds shared by session -> the second acquire gets 4;
/// the no-session refusal dropped -> acquired; `echo` for `printf` in
/// `write_atomic` -> the reason's `\n` writes a second line.
#[test]
fn cdo_slot_is_truthful_under_races_and_bad_records() {
    let s = Slot::new();
    let racers: Vec<_> = (0..8)
        .map(|i| s.bg(&format!("S{i}"), &["acquire", "o", "race"], &[]))
        .collect();
    let wins = racers
        .into_iter()
        .map(|mut c| c.0.wait().unwrap().code())
        .filter(|c| *c == Some(0))
        .count();
    assert_eq!(wins, 1);
    let holder = s.holder().expect("a status names the winner");
    // SAFETY: signal 0 only probes the process.
    assert_eq!(
        unsafe { libc::kill(holder, 0) },
        0,
        "the named holder lives"
    );
    drop(s);

    // A status left behind naming a live stranger and a dead hold id.
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let mut stranger = sleeper();
    std::fs::write(
        s.dir().join("status"),
        format!(
            "owner=old\nreason=x\nsince=2026-01-01T00:00:00Z\nsession=old\nclaude_pid=\n\
             bound=\nholder_pid={}\nholder_kind=keeper\ntoken=t\nhid=0123456789abcdef\n",
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
    // The live hold's status edited to name the stranger.
    let (code, text) = s.run("old", &["acquire", "old", "real"], &[]);
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
    assert_eq!(code, Some(1), "{text}");
    let (code, text) = s.run("old", &["release"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        stranger.try_wait().unwrap().is_none(),
        "release killed the stranger the status named"
    );
    assert_eq!(
        s.run("B", &["status"], &[]),
        (Some(0), "free\n".into()),
        "release left the real keeper"
    );

    // The same session, its hold bound to a running process; and a bound hold
    // whose Claude process is gone.
    let mut first = sleeper();
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
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let dead = gone.id().to_string();
    let mut bound = sleeper();
    let pid = bound.id().to_string();
    let (code, text) = s.run(
        "C",
        &["acquire", "land", "cron", "--bind", &pid],
        &[("CLAUDE_PID", &dead)],
    );
    assert_eq!(code, Some(0), "{text}");
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert_eq!(
        s.run("B", &["status"], &[]).0,
        Some(1),
        "live bound hold: kept"
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

/// ⟨TLS-89⟩ A process stuck on the timing lock (here a `flock -x` on it that
/// sleeps, as a stopped probe would be) blocks nobody for long: a timing gives
/// up after CDO_SLOT_LOCK_WAIT_S with exit 5, naming the PID that has it, and
/// takes nothing; `status` answers at once (and says a timing runs: the lock is
/// taken, the safe reading). (The hang behind this test: a SIGSTOPped waiter
/// held a lock and `release` waited 2h43m.)
///
/// Mutation (2026-11-19): `take_timed` waiting without `-w` -> the timing is
/// still waiting at the test's 10 s deadline (red); restored, green.
///
/// The keeper that reports the refusal writes its "fail <why>" and ends; the
/// acquire must still read it once it sees the keeper gone (TLS-89's
/// 3x-under-load check: the full suite's copy of this test lost the PID
/// that way). The CDO_SLOT_TEST_PAUSE_SPAWN_S hook makes it happen every time.
/// Mutation (2026-11-19): the ready re-check after the keeper is gone dropped
/// -> "the timing lock was not taken", no PID (red); restored, green.
#[test]
fn cdo_slot_a_stuck_lock_fails_loudly_and_readers_go_on() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    // -o: its `sleep` does not keep the lock once flock is killed.
    let lock = s.dir().join("timed.lock");
    let stuck = stand_in(
        &s,
        "flock",
        &[
            "-x".as_ref(),
            "-o".as_ref(),
            lock.as_os_str(),
            "sleep".as_ref(),
            "60".as_ref(),
        ],
    );
    until("the stand-in has the lock", &|| {
        s.probe().ends_with("timed=1")
    });
    let started = std::time::Instant::now();
    let (code, text) = s.run("B", &["status"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        // 15 s: well past a loaded host's status call, well short of the lock
        // wait a reader that took the lock would sit in (none, or 30 s).
        started.elapsed() < std::time::Duration::from_secs(15),
        "a reader waited for the lock"
    );
    let t = sleeper();
    let tpid = t.id().to_string();
    let mut timing = s.cmd(
        "A",
        &["acquire", "lane", "stuck", "--bind", &tpid, "--timed"],
        // The SPAWN pause lets the timing keeper write its "fail <why>" and end
        // between the acquire's ready check and its liveness check.
        &[
            ("CDO_SLOT_LOCK_WAIT_S", "2"),
            ("CDO_SLOT_TEST_PAUSE_SPAWN_S", "3"),
        ],
    );
    let mut timing = Reap(
        timing
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    // 20 s: the 2 s lock wait and the 3 s pause on a loaded host; the break
    // (no -w) waits for ever.
    let until_t = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let code = loop {
        if let Some(st) = timing.0.try_wait().unwrap() {
            break st.code();
        }
        assert!(
            std::time::Instant::now() < until_t,
            "a timing still waits for a stuck lock after 20 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(&mut timing.0.stderr.take().unwrap(), &mut err).unwrap();
    assert_eq!(code, Some(5), "{err}");
    assert!(
        err.contains("timing lock") && err.contains(&format!("PID {}", stuck.id())),
        "{err}"
    );
}

/// ⟨TLS-89⟩ The holder writes its status whole (write, then rename), and puts
/// the hold id last: a reader polling while the write is half way (the
/// CDO_SLOT_TEST_WRITE_PAUSE_S hook writes the first third, pauses, then the
/// rest) never prints a part of it, only "still writing" or the whole hold.
///
/// Mutation (2026-11-19): both guards broken (the status written in place, and
/// `hid` moved to the first line) -> "held by  () since" (red). Breaking only
/// one stays green, recorded as a property: a part without the id line names
/// no hold the reader can trust, and a renamed file is never partial.
#[test]
fn cdo_slot_a_reader_never_sees_a_half_written_status() {
    let s = Slot::new();
    let mut writer = s.bg(
        "A",
        &["acquire", "lane", "halfway"],
        &[("CDO_SLOT_TEST_WRITE_PAUSE_S", "3")],
    );
    s.wait_paused();
    let until_t = std::time::Instant::now() + std::time::Duration::from_millis(2000);
    let mut reads = 0;
    while std::time::Instant::now() < until_t {
        let (_, text) = s.run("B", &["status"], &[]);
        // The whole hold ends with its last described field (the session); a
        // part read as a hold would end in an empty one.
        assert!(
            text.starts_with("held (its holder is still writing its status)")
                || (text.starts_with("held by lane (halfway) since")
                    && text.trim_end().ends_with("session A")),
            "a reader saw a half-written status: {text}"
        );
        reads += 1;
    }
    assert!(reads > 0);
    assert_eq!(writer.0.wait().unwrap().code(), Some(0));
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(text.starts_with("held by lane (halfway) since"), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-89⟩ Concurrent takers: six sessions at once run timed and untimed
/// `run`s, bound timed acquires whose process then ends, and `status`, while a
/// checker reads `status` throughout:
/// - every `status` exits 0 or 1 and shows no half-read line (an empty owner,
///   reason or time);
/// - every command ends with an expected code (0 ran, 1 held, 4 shared, 6 a
///   timed refusal), never a crash;
/// - some timing really ran (a run of only refusals proves nothing; review r6,
///   sol);
/// - at the end the slot is free and no timing runs.
#[test]
fn cdo_slot_state_holds_under_concurrent_takers() {
    let s = Slot::new();
    let run = |session: &str, args: &[&str]| -> (Option<i32>, String) {
        s.run(session, args, &[("CDO_SLOT_TIMED_WAIT_S", "5")])
    };
    let bad_line = |l: &str| {
        l.contains("held by  (")
            || l.contains("() since")
            || l.contains("timed by  since")
            || l.contains("since  (")
            || l.ends_with("()")
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    let seen_bad = std::sync::Mutex::new(Vec::<String>::new());
    let unexpected = std::sync::Mutex::new(Vec::<String>::new());
    let timed_ran = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        sc.spawn(|| {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let (code, text) = run("checker", &["status"]);
                if !matches!(code, Some(0 | 1)) {
                    seen_bad.lock().unwrap().push(format!("{code:?}: {text}"));
                }
                for l in text.lines().filter(|l| bad_line(l)) {
                    seen_bad.lock().unwrap().push(l.to_string());
                }
            }
        });
        let workers: Vec<_> = (0..6)
            .map(|w| {
                let (run, unexpected, timed_ran) = (&run, &unexpected, &timed_ran);
                sc.spawn(move || {
                    let session = format!("S{w}");
                    for i in 0..4 {
                        let kind = (w + i) % 4;
                        let (code, text) = match kind {
                            0 => run(&session, &["run", "lane", "t", "--timed", "--", "true"]),
                            1 => run(&session, &["run", "lane", "u", "--", "true"]),
                            2 => {
                                let mut t = Reap(
                                    std::process::Command::new("sleep")
                                        .arg("1")
                                        .spawn()
                                        .unwrap(),
                                );
                                let tpid = t.id().to_string();
                                let r = run(
                                    &session,
                                    &["acquire", "lane", "b", "--bind", &tpid, "--timed", "--wait"],
                                );
                                let _ = t.0.wait();
                                r
                            }
                            _ => run(&session, &["status"]),
                        };
                        if (kind == 0 || kind == 2) && matches!(code, Some(0 | 4)) {
                            timed_ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        if !matches!(code, Some(0 | 1 | 4 | 6)) {
                            unexpected.lock().unwrap().push(format!("{code:?}: {text}"));
                        }
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let bad = seen_bad.into_inner().unwrap();
    assert!(bad.is_empty(), "bad status reads: {bad:?}");
    let unexpected = unexpected.into_inner().unwrap();
    assert!(unexpected.is_empty(), "unexpected outcomes: {unexpected:?}");
    assert!(timed_ran.into_inner() > 0, "no timing ever ran");
    until("the slot came free", &|| {
        run("checker", &["status"]) == (Some(0), "free\n".into())
    });
}

/// ⟨TLS-89, found by TLS-92's gate⟩ A reader takes every field from ONE read of
/// the status. Read field by field from the file, a status replaced between
/// `session` and `bound` made another run's BOUND hold read as this session's
/// own unbound one ("already yours" with empty fields; cdo-speed's timed hold
/// then failed). Here session A holds the slot bound to a sleeper; a second
/// acquire by A pauses between reading `session` and `bound` (the
/// CDO_SLOT_TEST_PAUSE_S hook) while the test removes the status file. It must
/// still see a hold that is not its own: refused (exit 1), not exit 4.
///
/// Mutation (2026-11-18): `field` reading the status FILE again instead of the
/// loaded copy -> "already yours", exit 4 (red); restored, green.
#[test]
fn cdo_slot_reads_the_status_once_per_check() {
    let s = Slot::new();
    let held = sleeper();
    let pid = held.id().to_string();
    let (code, text) = s.run("A", &["acquire", "land", "bound hold", "--bind", &pid], &[]);
    assert_eq!(code, Some(0), "{text}");
    let second = s
        .cmd(
            "A",
            &["acquire", "lane", "second"],
            &[("CDO_SLOT_TEST_PAUSE_S", "60")],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Remove it only once the reader is inside the window (review r3, sol).
    s.wait_paused();
    std::fs::remove_file(s.dir().join("status")).unwrap();
    s.resume();
    let out = second.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    drop(held);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(!text.contains("already yours"), "{text}");
}

/// ⟨TLS-89 review r6, sol: the P2s of the file-and-liveness design⟩ Each
/// decision is a lock probe, so:
/// - #1: a timing T1 and a second timing T2 that joined it: T1 killed outright,
///   every probe after it still says timed (T2's lock), never a gap.
/// - #3: a run that took the slot ends while a token-sharing timing it started
///   runs on: still held (the sharer inherited the hold) and timed, until the
///   sharer ends; then free.
/// - #4: an `acquire --timed` killed outright while it waits for a build: its
///   hold and its timing go with it (both keepers watch it until it says go),
///   though the bound process lives.
///
/// Mutations (2026-11-19): `timed_now` deciding from the first timing file's
/// PID (`alive $(field pid)`) -> #1 reads untimed after T1's kill (red; the
/// files sort by random id, so this break shows only when T1's file sorts
/// first: the proof run did); the keeper's writer watch dropped (wait for go
/// only) -> #4 stays held (red); `state`'s mine test written `a || b && c`
/// (bash groups it `(a || b) && c`, so a token share of a run's bound hold is
/// not "mine") -> #3's sharer waits and nothing is timed (red). Each restored,
/// green.
#[test]
fn cdo_slot_decides_from_locks_alone() {
    let s = Slot::new();
    // #1.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t1 = sleeper();
    let t2 = sleeper();
    let (p1, p2) = (t1.id().to_string(), t2.id().to_string());
    for (p, r) in [(&p1, "first"), (&p2, "second")] {
        let (code, text) = s.run("A", &["acquire", "lane", r, "--bind", p, "--timed"], &[]);
        assert_eq!(code, Some(4), "{text}");
    }
    drop(t1);
    for _ in 0..15 {
        assert!(
            s.probe().ends_with("timed=1"),
            "a probe read untimed while the second timing ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(text.contains("(second)"), "{text}");
    drop(t2);
    until("both timings ended", &|| {
        s.probe() == "held=1 own=0 timed=0"
    });
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // #3: the run took the slot, started a sharer (its token), and ends first.
    // Its output away: the sharer must not keep the run's output pipe open.
    // The taker ends only once the sharer's timing runs (review r7, sol: a fixed
    // sleep could end it first). The sharer runs 20 s: far longer than the
    // taker's end and the check below take on a loaded host (it was 4 s).
    let inner = format!(
        "bash {SLOT} run lane sharer --timed -- sleep 20 >/dev/null 2>&1 & \
         until bash {SLOT} probe | grep -q timed=1; do sleep 0.1; done"
    );
    let (code, text) = s.run(
        "A",
        &["run", "lane", "taker", "--", "bash", "-c", &inner],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        s.probe(),
        "held=1 own=0 timed=1",
        "the hold or the timing ended with the run while its sharer timed"
    );
    until("free once the sharer ended", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });

    // #4.
    let fake_name = format!("t89d{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let b = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let bound = sleeper();
    let bpid = bound.id().to_string();
    let mut waiting = s.bg(
        "A",
        &["acquire", "lane", "killed", "--bind", &bpid, "--timed"],
        &[
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "60"),
        ],
    );
    until("held and timed while it waits", &|| {
        s.probe() == "held=1 own=0 timed=1"
    });
    waiting.0.kill().unwrap();
    waiting.0.wait().unwrap();
    until("nothing left behind by the killed acquire", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });
    drop(b);
    drop(bound);
}

/// ⟨TLS-89⟩ A timing never starts beside a build: `run --timed` takes the timing
/// lock, waits for the build processes already running, and starts once they
/// end; still running at `CDO_SLOT_TIMED_WAIT_S` it refuses (exit 6, naming
/// their PIDs), runs nothing and frees the slot. Its own ancestors (the cargo
/// test that runs a timed script) are not waited for. An untimed run does not
/// wait.
///
/// The stand-in build is bash run through a symlink named `t89s<test pid>`
/// (matched by `pgrep -x`; unique, as `pgrep -x` sees the whole machine),
/// running two commands so it does not exec into `sleep`.
///
/// Mutations (2026-11-18): the timeout's refusal made `break` -> the run starts
/// beside the build (red); the ancestor exclusion dropped from `foreign_builds`
/// -> the run under the stand-in build refuses itself (red). Each restored,
/// green.
#[test]
fn cdo_slot_timed_waits_for_builds_and_refuses_beside_one() {
    let s = Slot::new();
    let fake_name = format!("t89s{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    // A symlink, not a copy: a copy still open for writing when another test
    // thread forks fails to exec ("Text file busy").
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let names = ("CDO_SLOT_BUILD_NAMES", fake_name.as_str());
    let ran = s.0.path().join("ran");
    let touch = ran.to_str().unwrap();

    // A build runs: refused after the wait, nothing run, the slot free.
    let mut b = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
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
    // run's first scan on a loaded machine).
    let mut timed = s.cmd(
        "A",
        &["run", "lane", "timing", "--timed", "--", "touch", touch],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "60")],
    );
    let mut timed = Reap(
        timed
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
    // status names the timing.
    let inner = format!("bash {SLOT} run lane inner --timed -- bash -c 'bash {SLOT} status; true'");
    let mut under = scrubbed(std::process::Command::new(&fake));
    under
        .args(["-c", &format!("{inner}; true")])
        .env("CDO_SLOT_DIR", s.dir())
        .env("CLAUDE_CODE_SESSION_ID", "A")
        .env_remove("CLAUDE_PID")
        .env_remove("CDO_SLOT_TOKEN");
    for (k, v) in BASE_ENV {
        under.env(k, v);
    }
    let out = under
        .env("CDO_SLOT_BUILD_NAMES", &fake_name)
        .env("CDO_SLOT_TIMED_WAIT_S", "2")
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

    // Review r3 (sol): the wait never sleeps past its deadline. A build still
    // running at the 1 s deadline refuses at once, even with a 15 s poll.
    // Break: sleeping a whole poll -> the refusal comes after 15 s (red on the
    // time). The build runs until the test ends it: one that ended by itself at
    // 3 s could end before a loaded host's first scan, which then rightly saw
    // nothing and started (TLS-102's stress run).
    let b7 = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let started = std::time::Instant::now();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "deadline", "--timed", "--", "true"],
        &[
            names,
            ("CDO_SLOT_TIMED_WAIT_S", "1"),
            ("CDO_SLOT_POLL_S", "15"),
        ],
    );
    drop(b7);
    assert_eq!(code, Some(6), "started beside a build: {text}");
    // 14 s: the break sleeps a whole 15 s poll; a loaded host's refusal at the
    // 1 s deadline stays far below.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(14),
        "slept past the deadline: {:?}",
        started.elapsed()
    );

    // Review r1 (sol): a reason with a CR (which text-mode readers break into a
    // second line, where `timed by` lives) is refused.
    let (code, text) = s.run("A", &["acquire", "lane", "probe\rtimed by x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("one-line reason"), "{text}");
}

/// ⟨TLS-104⟩ A host suspend does not use up `run --timed`'s wait for builds: the
/// wait is wall-clock time, which keeps counting while a laptop sleeps (review,
/// Fable). A build runs and the wait is 10 s; once the run says it waits, the
/// host "sleeps" 60 s (`CDO_HOST_SUSPENDED_FILE` moves from 100 to 160). The
/// run says so and waits on awake time; the build is ended only 12 s after the
/// run started, past the 10 s wall-clock deadline (ending it at once passed even
/// with the wait on wall time: a first proof of this test came back green), and
/// the timed command runs (exit 0).
///
/// The budgets (ENG-168's task gate, 4 failures in 4 runs with 2 s and 5 s): on
/// a loaded machine the wait's first scan of the process table can itself take
/// over 2 s, so a 2 s wait refused before it printed the line the suspend is
/// injected on. 10 s leaves the scan room; the 60 s suspend puts the awake
/// deadline about 70 s out, far past the kill at 12 s.
///
/// Mutation (2026-11-20): the wait on wall time again (`slot_clocks` taking
/// `EPOCHREALTIME` for the awake clock) -> refused at 10 s, exit 6 (red);
/// restored, green. (TLS-105 moved `slot_clocks` into scripts/awake-clock's
/// `awake_read`, shared by every awake deadline.)
#[test]
fn cdo_slot_timed_wait_survives_a_host_suspend() {
    let s = Slot::new();
    let fake_name = format!("t104s{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let ran = s.0.path().join("ran");
    let touch = ran.to_str().unwrap();
    let reading = s.0.path().join("suspended");
    std::fs::write(&reading, "100").unwrap();
    let reading_s = reading.to_str().unwrap();
    let mut b = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let bpid = b.0.id();
    let mut timed = Reap(
        s.cmd(
            "A",
            &["run", "lane", "timing", "--timed", "--", "touch", touch],
            &[
                ("CDO_SLOT_BUILD_NAMES", fake_name.as_str()),
                ("CDO_SLOT_TIMED_WAIT_S", "10"),
                ("CDO_HOST_SUSPENDED_FILE", reading_s),
            ],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let started = std::time::Instant::now();
    let mut text = String::new();
    {
        use std::io::BufRead;
        let lines = std::io::BufReader::new(timed.0.stdout.take().unwrap()).lines();
        for line in lines {
            let line = line.unwrap();
            if line.contains(&format!("waiting for build processes to end: PID {bpid}")) {
                std::fs::write(&reading, "160").unwrap();
            }
            if line.contains("the host slept 60 s during this wait") {
                // Past the 10 s wall-clock deadline, inside the awake one.
                let at = std::time::Duration::from_secs(12);
                std::thread::sleep(at.saturating_sub(started.elapsed()));
                b.0.kill().unwrap();
                b.0.wait().unwrap();
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    let mut err = String::new();
    std::io::Read::read_to_string(&mut timed.0.stderr.take().unwrap(), &mut err).unwrap();
    let code = timed.0.wait().unwrap().code();
    assert_eq!(code, Some(0), "{text}{err}");
    assert!(
        text.contains("the host slept 60 s during this wait"),
        "{text}"
    );
    assert!(ran.exists(), "{text}{err}");
}

/// ⟨TLS-105⟩ `acquire --wait --timeout` counts awake time: a host suspend does
/// not use up the timeout (review of TLS-104, Fable). B waits 2 s for A's hold;
/// once B says it waits (its clock is started by then), the host "sleeps" 10 s
/// (`CDO_HOST_SUSPENDED_FILE` 100 -> 110). B says so, still gives up (a real
/// awake timeout expires), but only about 12 s after it started: not at the 2 s
/// wall-clock deadline.
///
/// Mutation (2026-11-23): the timeout check on wall time again (`awake_us`
/// plus `awake_slept` seconds) -> gave up 2.2 s after the suspend (red);
/// restored, green. Review r1: `awake_slept` not updated after a report (in
/// scripts/awake-clock) -> the suspend line said twice (red); restored, green.
#[test]
fn cdo_slot_acquire_timeout_survives_a_host_suspend() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let reading = s.0.path().join("suspended");
    std::fs::write(&reading, "100").unwrap();
    let reading_s = reading.to_str().unwrap();
    let mut b = Reap(
        s.cmd(
            "B",
            &["acquire", "b", "late", "--wait", "--timeout", "2"],
            &[("CDO_HOST_SUSPENDED_FILE", reading_s)],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let mut text = String::new();
    let mut slept_at = None;
    {
        use std::io::BufRead;
        let lines = std::io::BufReader::new(b.0.stdout.take().unwrap()).lines();
        for line in lines {
            let line = line.unwrap();
            if line.contains("waiting: held by a (hold)") && slept_at.is_none() {
                std::fs::write(&reading, "110").unwrap();
                slept_at = Some(std::time::Instant::now());
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    let mut err = String::new();
    std::io::Read::read_to_string(&mut b.0.stderr.take().unwrap(), &mut err).unwrap();
    let code = b.0.wait().unwrap().code();
    let after = slept_at.expect("B never said it waits").elapsed();
    assert_eq!(code, Some(1), "{text}{err}");
    assert!(err.contains("not taken after 2 s"), "{text}{err}");
    // Said exactly once (review r1, sol): each later read sees the same sleep.
    let said =
        "cdo-slot: --timeout: the host slept 10 s during this wait: it goes on that much longer";
    assert_eq!(text.matches(said).count(), 1, "{text}{err}");
    // The awake deadline is ~12 s out; the wall one passed 2 s after the start.
    assert!(
        after >= std::time::Duration::from_secs(8),
        "gave up {after:?} after the suspend: the suspend used up the timeout"
    );
}

/// ⟨TLS-105⟩ `go` (a keeper's go, CDO_SLOT_LOCK_WAIT_S) counts awake time. A
/// hold keeper is stopped before its go (as in the review r8 case below); once
/// the go file exists (its clock is started by then), the host "sleeps" 10 s.
/// The acquire says so once its 2 s wall deadline has passed, is still waiting
/// 5 s after the suspend, and when the keeper goes on then (SIGCONT) it takes
/// its go: the hold is taken (exit 0). That a real awake timeout still expires
/// is the review r8 case in `cdo_slot_acquire_windows_hold_against_a_timing`
/// (a stopped keeper, given up on after 2 s). (A first version waited for that
/// expiry here; its break came back only 7.4 s after the suspend, the keeper's
/// 5 s end included, too close to the 8 s bound to tell apart under load.)
///
/// Mutation (2026-11-23): `go`'s check on wall time again (`awake_us` plus
/// `awake_slept` seconds) -> it had already given up when the keeper went on
/// (red); restored, green.
#[test]
fn cdo_slot_go_survives_a_host_suspend() {
    let s = Slot::new();
    let reading = s.0.path().join("suspended");
    std::fs::write(&reading, "100").unwrap();
    let reading_s = reading.to_str().unwrap();
    let mut held = Reap(
        s.cmd(
            "A",
            &["acquire", "lane", "stopped"],
            &[
                ("CDO_SLOT_TEST_PAUSE_HOLDGO_S", "60"),
                ("CDO_SLOT_LOCK_WAIT_S", "2"),
                ("CDO_HOST_SUSPENDED_FILE", reading_s),
            ],
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    let k = keepers(&s, b"_keep\0hold\0");
    assert_eq!(k.len(), 1, "precondition: one hold keeper");
    // SAFETY: SIGSTOP to this test's own hold keeper.
    unsafe { libc::kill(k[0], libc::SIGSTOP) };
    s.resume();
    until("the go file", &|| {
        std::fs::read_dir(s.dir())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("go."))
    });
    std::fs::write(&reading, "110").unwrap();
    let slept_at = std::time::Instant::now();
    let said = "cdo-slot: go: the host slept 10 s during this wait: it goes on that much longer";
    let mut err = String::new();
    let mut waiting_at_5s = None;
    {
        use std::io::BufRead;
        let lines = std::io::BufReader::new(held.0.stderr.take().unwrap()).lines();
        for line in lines {
            let line = line.unwrap();
            if line.contains(said) && waiting_at_5s.is_none() {
                // Past the 2 s wall deadline, inside the ~12 s awake one.
                let at = std::time::Duration::from_secs(5);
                std::thread::sleep(at.saturating_sub(slept_at.elapsed()));
                waiting_at_5s = Some(held.0.try_wait().unwrap().is_none());
                // SAFETY: SIGCONT to this test's own hold keeper.
                unsafe { libc::kill(k[0], libc::SIGCONT) };
            }
            err.push_str(&line);
            err.push('\n');
        }
    }
    let code = held.0.wait().unwrap().code();
    assert_eq!(err.matches(said).count(), 1, "said once: {err}");
    assert_eq!(waiting_at_5s, Some(true), "gave up within 5 s: {err}");
    assert_eq!(code, Some(0), "the go was refused: {err}");
    assert_eq!(s.probe(), "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// A folder of links to the real scripts named in `with`, for a cdo-slot run
/// from there: plus, with `stub`, a bash `host-suspended` of that body (a clock
/// that fails, a slow one, an old one), so no test hook reaches the run. A
/// script left out of `with` is missing there (a partial copy).
fn link_folder(s: &Slot, with: &[&str], stub: Option<&str>) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = s.0.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    for f in with {
        std::os::unix::fs::symlink(real.join(f), bin.join(f)).unwrap();
    }
    if let Some(body) = stub {
        let path = bin.join("host-suspended");
        std::fs::write(&path, format!("#!/bin/bash\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

/// ⟨TLS-105 review r1⟩ cdo-slot loads scripts/awake-clock only for a wait with
/// a deadline (sol P1, Fable 2). A copy without it still answers `probe` and
/// `status` (the guard's probe: one that failed would let work run beside a
/// timing) and `release`; an `acquire` says the helper is missing and takes
/// nothing.
///
/// Mutation (2026-11-23): the helper sourced at the top again (as in e0d333e8)
/// -> `probe` from the copy fails (red); restored, green.
#[test]
fn cdo_slot_without_its_clock_helper_still_probes() {
    let s = Slot::new();
    let copy = link_folder(&s, &["cdo-slot", "host-suspended"], None).join("cdo-slot");
    let run = |session: &str, args: &[&str]| {
        let out = s.cmd_at(&copy, session, args, &[]).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };
    assert_eq!(run("P", &["status"]), (Some(0), "free\n".into()));
    let (code, text) = run("P", &["probe"]);
    assert_eq!(
        (code, text.lines().next()),
        (Some(0), Some("held=0 own=0 timed=0")),
        "{text}"
    );
    let (code, text) = run("A", &["acquire", "lane", "no helper"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("awake-clock is missing"), "{text}");
    assert!(text.contains("nothing was taken"), "{text}");
    assert_eq!(s.probe(), "held=0 own=0 timed=0", "it took nothing");
    // A hold taken by the full scripts: the copy sees it and releases it.
    assert_eq!(s.run("A", &["acquire", "lane", "full"], &[]).0, Some(0));
    let (code, text) = run("P", &["probe"]);
    assert_eq!(
        (code, text.lines().next()),
        (Some(0), Some("held=1 own=0 timed=0")),
        "{text}"
    );
    let (code, text) = run("A", &["release"]);
    assert_eq!(code, Some(0), "{text}");
    until("released", &|| s.probe() == "held=0 own=0 timed=0");
}

/// ⟨TLS-105 review r1, Fable 1⟩ A clock that stops reading does not make a wait
/// endless. `host-suspended` reads once (the wait's start), then fails: the
/// wait keeps the last awake time for two failed reads, falls back to wall time
/// from the last good reading at the third, says so once, and gives up.
///
/// Mutation (2026-11-23): the fall back dropped (the third failure kept the
/// last awake time like the first two) -> still waiting after 20 s (red);
/// restored, green.
#[test]
fn cdo_slot_a_clock_that_stops_reading_falls_back_to_wall_time() {
    let s = Slot::new();
    let real = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let bin = link_folder(
        &s,
        &["cdo-slot", "awake-clock"],
        Some(&format!(
            "d=$(dirname \"$0\")\n[[ -e \"$d/read-once\" ]] && exit 1\n: >\"$d/read-once\"\nexec '{real}' \"$@\""
        )),
    );
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let mut b = Reap(
        s.cmd_at(
            &bin.join("cdo-slot"),
            "B",
            &["acquire", "b", "late", "--wait", "--timeout", "2"],
            &[],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let started = std::time::Instant::now();
    let code = loop {
        if let Some(st) = b.0.try_wait().unwrap() {
            break st.code();
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "still waiting after 20 s: a clock that stopped reading made the wait endless"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut text = String::new();
    std::io::Read::read_to_string(&mut b.0.stdout.take().unwrap(), &mut text).unwrap();
    std::io::Read::read_to_string(&mut b.0.stderr.take().unwrap(), &mut text).unwrap();
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("not taken after 2 s"), "{text}");
    let said =
        "cdo-slot: --timeout: the awake clock failed 3 reads in a row: this wait counts wall time";
    assert_eq!(text.matches(said).count(), 1, "{text}");
}

/// ⟨TLS-105 review r1⟩ The keeper took its go while the clocks were read at the
/// deadline (sol P2, Fable 5): that is a go, not a stopped keeper. The keeper
/// is stopped; at go's first deadline read, the stand-in `host-suspended`
/// lets it go on and waits until it has taken the go, then answers (real
/// clocks: past the 1 s deadline). The acquire takes the hold (exit 0).
///
/// Mutation (2026-11-23): the go file's second check on expiry dropped -> the
/// acquire refuses the go and ends the keeper, exit 1 (red); restored, green.
#[test]
fn cdo_slot_go_taken_during_the_deadline_read_is_a_go() {
    let s = Slot::new();
    let real = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let slot = s.dir();
    let stub = format!(
        "d=$(dirname \"$0\")\n\
         if [[ -e \"$d/cont\" ]]; then\n\
           p=$(<\"$d/cont\"); rm -f \"$d/cont\"; kill -CONT \"$p\"\n\
           for i in $(seq 400); do compgen -G '{}/go.*' >/dev/null || break; sleep 0.05; done\n\
         fi\n\
         exec '{real}' \"$@\"",
        slot.display()
    );
    let bin = link_folder(&s, &["cdo-slot", "awake-clock"], Some(&stub));
    let mut held = Reap(
        s.cmd_at(
            &bin.join("cdo-slot"),
            "A",
            &["acquire", "lane", "ack"],
            &[
                ("CDO_SLOT_TEST_PAUSE_HOLDGO_S", "60"),
                ("CDO_SLOT_LOCK_WAIT_S", "1"),
            ],
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    let k = keepers(&s, b"_keep\0hold\0");
    assert_eq!(k.len(), 1, "precondition: one hold keeper");
    // SAFETY: SIGSTOP to this test's own hold keeper.
    unsafe { libc::kill(k[0], libc::SIGSTOP) };
    s.resume();
    // After go's first clock read (its start), before its deadline read.
    until("the go file", &|| {
        std::fs::read_dir(&slot)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("go."))
    });
    std::fs::write(bin.join("cont"), k[0].to_string()).unwrap();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut held.0.stderr.take().unwrap(), &mut err).unwrap();
    let code = held.0.wait().unwrap().code();
    assert!(
        !bin.join("cont").exists(),
        "precondition: the deadline read let the keeper go on"
    );
    assert_eq!(code, Some(0), "an acknowledged go was refused: {err}");
    assert_eq!(s.probe(), "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-105 review r1, sol P4⟩ An old `host-suspended` (the suspended seconds
/// only, no `--clocks` fields) leaves the wait on wall time: a suspend is then
/// counted in the wait, and the line says so instead of "it goes on that much
/// longer". (Wording only: no discrimination proof, by the owner's rule.)
#[test]
fn cdo_slot_an_old_clock_says_the_wall_clock_counts_the_suspend() {
    let s = Slot::new();
    let bin = link_folder(
        &s,
        &["cdo-slot", "awake-clock"],
        Some("cat \"$(dirname \"$0\")/suspended\""),
    );
    let reading = bin.join("suspended");
    std::fs::write(&reading, "100").unwrap();
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let mut b = Reap(
        s.cmd_at(
            &bin.join("cdo-slot"),
            "B",
            &["acquire", "b", "late", "--wait", "--timeout", "3"],
            &[],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let mut text = String::new();
    {
        use std::io::BufRead;
        let lines = std::io::BufReader::new(b.0.stdout.take().unwrap()).lines();
        for line in lines {
            let line = line.unwrap();
            if line.contains("waiting: held by a (hold)") {
                std::fs::write(&reading, "110").unwrap();
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    std::io::Read::read_to_string(&mut b.0.stderr.take().unwrap(), &mut text).unwrap();
    assert_eq!(b.0.wait().unwrap().code(), Some(1), "{text}");
    let said = "cdo-slot: --timeout: the host slept 10 s during this wait: the wall clock counts it in this wait (no awake clock)";
    assert_eq!(text.matches(said).count(), 1, "{text}");
    assert!(!text.contains("goes on that much longer"), "{text}");
}

/// ⟨TLS-89⟩ The build scripts a timing waits for, and the grace wait:
/// - Review r1 (Fable): a build SCRIPT between its cargo steps (no build-named
///   process) counts; review r1 (sol): run with an option operand before it
///   (`bash -o pipefail <script>`).
/// - Review r2 (sol): a python build driver with an attached option value
///   holding an `m` (`-Xfrozen_modules=off`: no `-m` option) counts.
/// - The grace wait (owner decision 2026-11-19): a build that starts during
///   the first CDO_SLOT_TIMED_GRACE_S seconds after the timing lock was taken
///   (one the guard allowed a moment before) is still waited for.
///
/// Mutations: `script_builds` dropped from `foreign_builds` -> the run starts
/// beside the script (red); the `-o`/`-O` operand skip dropped from
/// `script_operand` -> missed (red); the word-wide `*[cm]*` check put back ->
/// the driver is missed (red). (2026-11-19) the grace counting made `break` on
/// the first empty scan -> the late build is missed and the timing starts (red).
#[test]
fn cdo_slot_timed_waits_for_build_scripts_and_late_builds() {
    let s = Slot::new();
    let fake_name = format!("t89g{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let names = ("CDO_SLOT_BUILD_NAMES", fake_name.as_str());

    let script_name = format!("ci-steps-t89-{}", std::process::id());
    let script = s.0.path().join(&script_name);
    std::fs::write(&script, "sleep 600\ntrue\n").unwrap();
    let sb = stand_in(
        &s,
        "bash",
        &["-o".as_ref(), "pipefail".as_ref(), script.as_os_str()],
    );
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
        text.contains(&format!("PID {} ({script_name})", sb.id())),
        "{text}"
    );
    drop(sb);

    let py_name = format!("check-goldens-t89-{}", std::process::id());
    let py = s.0.path().join(&py_name);
    std::fs::write(&py, "import time\ntime.sleep(600)\n").unwrap();
    let pb = stand_in(
        &s,
        "python3",
        &["-Xfrozen_modules=off".as_ref(), py.as_os_str()],
    );
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "true"],
        &[
            names,
            ("CDO_SLOT_BUILD_SCRIPTS", py_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "2"),
        ],
    );
    assert_eq!(
        code,
        Some(6),
        "the timing started beside a python driver: {text}"
    );
    assert!(
        text.contains(&format!("PID {} ({py_name})", pb.id())),
        "{text}"
    );
    drop(pb);

    // The grace wait: the build starts 1 s after the timing lock is taken. The
    // timed command sleeps first, so a timing that started without waiting is
    // still seen holding its lock (and is caught by the assertion, not by the
    // precondition).
    let ran = s.0.path().join("ran");
    let timed_cmd = format!("sleep 2; touch {}", ran.display());
    let mut timing = s.cmd(
        "A",
        &[
            "run", "lane", "grace", "--timed", "--", "bash", "-c", &timed_cmd,
        ],
        // The GRACE hook holds the timing right after its first empty scan
        // (inside the grace) until the late build runs; a fixed 1 s sleep
        // could outlast a 3 s grace on a loaded host (TLS-102's audit).
        &[
            names,
            ("CDO_SLOT_TIMED_GRACE_S", "3"),
            ("CDO_SLOT_TIMED_WAIT_S", "6"),
            ("CDO_SLOT_TEST_PAUSE_GRACE_S", "60"),
        ],
    );
    let mut timing = Reap(
        timing
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    s.wait_paused();
    let late = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    until("the late build runs", &|| {
        std::path::Path::new(&format!("/proc/{}", late.id())).exists()
    });
    s.resume();
    let code = timing.0.wait().unwrap().code();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut timing.0.stderr.take().unwrap(), &mut err).unwrap();
    assert_eq!(
        code,
        Some(6),
        "the timing started beside a late build: {err}"
    );
    assert!(err.contains(&format!("PID {}", late.id())), "{err}");
    assert!(!ran.exists());
}

/// ⟨TLS-89⟩ Timings by `acquire --bind --timed`, in a hold taken by hand:
/// - A timing ends with its bound process; the hold stays.
/// - Review r1 (Fable): a second timing in that hold waits for a build too,
///   and keeps its own timing when the first one ends; a refused `acquire
///   --bind --timed` with no session or token frees the slot it took.
/// - Review r1 (sol): the hold it shares is released while it waits for a
///   build -> refused once the build ends, never a start with no hold.
/// - A hold released while a timing in it runs leaves the timing on, and no
///   other session can take the slot until the timing ends (owner decision
///   2026-11-19: a timing keeps every new hold out).
/// - Review r3/r4 (sol): the hold it took or shared is replaced in the window
///   right after the acquire (the test hook pauses there) -> refused, and the
///   replacement is left alone and untimed.
/// - Review r3/r4 (sol): a refused second timing bound to the SAME process (a
///   retry), also one that refuses concurrently with a retry that starts,
///   leaves the other timing on.
/// - Review r5 (sol): a timing that TOOK the slot and is refused while a
///   token-sharing timing runs: the sharer's timing stays on.
/// - A `run`'s hold is not released by `release`: it ends with the run.
///
/// Mutations (2026-11-19): the hold probe after the drain dropped -> the
/// released hold's timing starts (red); the `timed_now` check dropped from
/// `acquire` -> B takes the slot beside the timing (red); `refuse` ending
/// every timing keeper instead of its own -> the retry's timing ends (red).
/// Each restored, green. (2026-11-18): the refused acquire's keeper end
/// dropped -> the slot stays held (red).
#[test]
fn cdo_slot_timings_in_a_hand_hold() {
    let s = Slot::new();
    let fake_name = format!("t89h{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let names = ("CDO_SLOT_BUILD_NAMES", fake_name.as_str());
    let build = || stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let timed_line = || s.run("B", &["status"], &[]).1;

    // A timing ends with its bound process; the hold stays.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let timing = sleeper();
    let tpid = timing.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "measure", "--bind", &tpid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "shared: {text}");
    assert!(timed_line().contains("timed by lane since"));
    drop(timing);
    until("the timing ended with its process", &|| {
        s.probe() == "held=1 own=0 timed=0"
    });

    // Two timings: the second waits for a build too, and keeps its own.
    let t1 = sleeper();
    let t2 = sleeper();
    let (t1pid, t2pid) = (t1.id().to_string(), t2.id().to_string());
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "first", "--bind", &t1pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    let b2 = build();
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
    drop(b2);
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    drop(t1);
    until("only the second timing left", &|| {
        let t = timed_line();
        t.contains("(second)") && !t.contains("(first)")
    });
    assert!(
        s.probe().ends_with("timed=1"),
        "the second timing lost its cover"
    );
    drop(t2);
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A refused `acquire --bind --timed` (no session, no token) frees what it took.
    let b3 = build();
    let t3 = sleeper();
    let t3pid = t3.id().to_string();
    let (code, text) = s.run(
        "",
        &["acquire", "cron", "measure", "--bind", &t3pid, "--timed"],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "1")],
    );
    assert_eq!(code, Some(6), "{text}");
    assert_eq!(
        s.run("B", &["status"], &[]),
        (Some(0), "free\n".into()),
        "the refused acquire kept the slot"
    );
    drop(t3);

    // The hold it shares is released while it waits for a build.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t4 = sleeper();
    let t4pid = t4.id().to_string();
    let mut waiting = s.bg(
        "A",
        &["acquire", "lane", "drained", "--bind", &t4pid, "--timed"],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "60")],
    );
    until("the timing waits", &|| s.probe().ends_with("timed=1"));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    // A timing keeps every new hold out, its own hold gone or not.
    let (code, text) = s.run("B", &["acquire", "other", "beside"], &[]);
    assert_eq!(code, Some(1), "a hold was taken beside a timing: {text}");
    assert!(text.contains("a timing runs"), "{text}");
    drop(b3);
    assert_eq!(
        waiting.0.wait().unwrap().code(),
        Some(6),
        "a timing started after its hold ended"
    );
    assert_eq!(s.run("B", &["status"], &[]), (Some(0), "free\n".into()));
    drop(t4);

    // The hold it shared is replaced in the window right after the acquire.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t5 = sleeper();
    let t5pid = t5.id().to_string();
    let mut window = s.bg(
        "A",
        &["acquire", "lane", "window", "--bind", &t5pid, "--timed"],
        &[names, ("CDO_SLOT_TEST_PAUSE_ACQUIRED_S", "60")],
    );
    s.wait_paused();
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    assert_eq!(
        s.run("B", &["acquire", "other", "replacement"], &[]).0,
        Some(0)
    );
    s.resume();
    assert_eq!(window.0.wait().unwrap().code(), Some(6));
    let (_, text) = s.run("D", &["status"], &[]);
    assert!(
        text.contains("held by other (replacement)") && !text.contains("timed by"),
        "the refused timing touched another session's hold: {text}"
    );
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
    drop(t5);

    // A refused retry bound to the same process leaves the first timing on;
    // so does one that refuses while the retry starts (the hook pauses it).
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t7 = sleeper();
    let t7pid = t7.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "kept", "--bind", &t7pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    let b6 = build();
    let mut first = s.bg(
        "A",
        &["acquire", "lane", "refused", "--bind", &t7pid, "--timed"],
        &[
            names,
            ("CDO_SLOT_TIMED_WAIT_S", "1"),
            ("CDO_SLOT_TEST_PAUSE_REFUSE_S", "60"),
        ],
    );
    s.wait_paused();
    drop(b6);
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "retry", "--bind", &t7pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    s.resume();
    assert_eq!(first.0.wait().unwrap().code(), Some(6));
    let text = timed_line();
    assert!(
        text.contains("timed by lane since") && !text.contains("(refused)"),
        "{text}"
    );
    drop(t7);
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A timing that TOOK the slot is refused while a token-sharing timing runs.
    let t11 = sleeper();
    let t12 = sleeper();
    let (t11pid, t12pid) = (t11.id().to_string(), t12.id().to_string());
    let b11 = build();
    let mut taker = s.bg(
        "A",
        &["acquire", "lane", "taker", "--bind", &t11pid, "--timed"],
        &[
            names,
            ("CDO_SLOT_TIMED_WAIT_S", "1"),
            ("CDO_SLOT_TEST_PAUSE_REFUSE_S", "60"),
            ("CDO_SLOT_TOKEN", "tls89-shared"),
        ],
    );
    s.wait_paused();
    drop(b11);
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "sharer", "--bind", &t12pid, "--timed"],
        &[names, ("CDO_SLOT_TOKEN", "tls89-shared")],
    );
    assert_eq!(code, Some(4), "{text}");
    s.resume();
    assert_eq!(taker.0.wait().unwrap().code(), Some(6));
    assert!(
        s.probe().ends_with("timed=1") && timed_line().contains("(sharer)"),
        "the refused taker ended the sharer's timing"
    );
    drop(t12);
    drop(t11);
    until("free", &|| s.probe() == "held=0 own=0 timed=0");

    // A `run`'s hold is the run's: `release` refuses it. The hold lock is taken
    // (`held=1`) a moment before the run writes its status (TLS-92's landing:
    // a `release` in that moment refused, honestly, with "its holder is still
    // writing its status", and the test expected "taken by a run"). The
    // PUBLISH hook holds that moment open (2 s, no resume): a `release` inside
    // it refuses and ends nothing; the test then waits for the published
    // status, not for `held=1`.
    // Mutation (2026-11-19): the wait for the published status dropped (the
    // second `release` goes in the same moment) -> "still writing its status"
    // instead of "taken by a run" (red); restored, green.
    let mut running = s.bg(
        "E",
        &["run", "lane", "took", "--", "sleep", "600"],
        &[("CDO_SLOT_TEST_PAUSE_PUBLISH_S", "2")],
    );
    s.wait_paused();
    let (code, text) = s.run("E", &["release"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("still writing its status"), "{text}");
    assert!(
        s.probe().starts_with("held=1"),
        "a refused release ended the hold"
    );
    until("the run's status is published", &|| {
        s.run("D", &["status"], &[])
            .1
            .contains("held by lane (took)")
    });
    let (code, text) = s.run("E", &["release"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("taken by a run"), "{text}");
    running.0.kill().unwrap();
}

/// ⟨TLS-89 review r7, sol⟩ The two acquire-side windows:
/// - A hold taken in the window after the acquire's checks: B's keeper pauses
///   before the hold lock (CDO_SLOT_TEST_PAUSE_TAKE_S) while A takes a hold,
///   starts a timing in it and releases the hold, the timing still on. B must
///   not take the slot beside that timing.
/// - A timing keeper killed after it took its locks and before its go (the
///   CDO_SLOT_TEST_PAUSE_GO_S hook pauses the acquire there): the acquire must
///   not report a timing it no longer has (exit 6), and frees the slot it took.
///
/// Mutations (2026-11-19): the timing recheck under the hold lock dropped ->
/// B takes the slot beside the timing (red); `go` without the acknowledgement
/// (only `: >go.<id>`) -> the acquire reports success with no timing (red).
/// Each restored, green. Review r8 (2026-11-19): the timing probe after both
/// go's dropped -> the acquire whose timing keeper died between them exits 0
/// (red); `go`'s time bound dropped -> it waits for the stopped keeper past the
/// test's 20 s (red). Each restored, green. (The go file's creation check has
/// no proof: no test here can make that one write fail.)
#[test]
fn cdo_slot_acquire_windows_hold_against_a_timing() {
    let s = Slot::new();
    let mut b = s.bg(
        "B",
        &["acquire", "other", "late"],
        // Held until the test makes `resume` (60 s at most): a fixed 3 s pause
        // ended before the steps below under load (TLS-89's gate).
        &[("CDO_SLOT_TEST_PAUSE_TAKE_S", "60")],
    );
    s.wait_paused();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t = sleeper();
    let tpid = t.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "timing", "--bind", &tpid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    assert_eq!(s.probe(), "held=0 own=0 timed=1", "precondition");
    s.resume();
    assert_eq!(
        b.0.wait().unwrap().code(),
        Some(1),
        "a hold was taken beside a timing"
    );
    assert_eq!(s.probe(), "held=0 own=0 timed=1");
    drop(t);
    until("no timing", &|| s.probe() == "held=0 own=0 timed=0");

    let t = sleeper();
    let tpid = t.id().to_string();
    let mut timed = s.bg(
        "A",
        &["acquire", "lane", "orphaned", "--bind", &tpid, "--timed"],
        // KEEPER_FORK: each keeper keeps a subshell of itself until `fork-done`,
        // as a `$(...)` does for a moment: the count must still be one.
        // Mutation (2026-11-19): `keepers` without its parent check -> two
        // (red), the landing's failure; restored, green.
        &[
            ("CDO_SLOT_TEST_PAUSE_GO_S", "60"),
            ("CDO_SLOT_TEST_KEEPER_FORK", "1"),
        ],
    );
    s.wait_paused();
    let k = keepers(&s, b"_keep\0timed\0");
    std::fs::write(s.dir().join("fork-done"), "").unwrap();
    assert_eq!(k.len(), 1, "precondition: one timing keeper");
    // SAFETY: SIGKILL to this test's own timing keeper.
    unsafe { libc::kill(k[0], libc::SIGKILL) };
    s.resume();
    assert_eq!(
        timed.0.wait().unwrap().code(),
        Some(6),
        "the acquire reported a timing whose keeper was killed"
    );
    assert_eq!(s.probe(), "held=0 own=0 timed=0", "it freed what it took");
    drop(t);

    // Review r8 (sol): the timing keeper killed AFTER its go, while the acquire
    // gives the hold keeper its go (the HOLDGO hook pauses there): refused, and
    // the hold it took is freed.
    let t = sleeper();
    let tpid = t.id().to_string();
    let mut timed = s.bg(
        "A",
        &["acquire", "lane", "between", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_HOLDGO_S", "60")],
    );
    s.wait_paused();
    let k = keepers(&s, b"_keep\0timed\0");
    assert_eq!(k.len(), 1, "precondition: one timing keeper");
    // SAFETY: SIGKILL to this test's own timing keeper.
    unsafe { libc::kill(k[0], libc::SIGKILL) };
    s.resume();
    assert_eq!(
        timed.0.wait().unwrap().code(),
        Some(6),
        "the acquire reported a timing that ended before both go's"
    );
    until("it freed the hold", &|| s.probe() == "held=0 own=0 timed=0");
    drop(t);

    // Review r8 (sol): a hold keeper STOPPED before its go: the acquire gives up
    // after CDO_SLOT_LOCK_WAIT_S, ends the keeper and says nothing is held.
    // SCAN_ONCE: only the first keeper scan finds it, as when a later scan
    // missed it under load (TLS-102's stress run left it stopped and holding).
    // Mutation (2026-11-19): `end_keeper` trusting a fresh scan at each step
    // again -> the stopped keeper is never killed: "never: the stopped keeper
    // was ended" (red); restored, green.
    let mut held = s
        .cmd(
            "A",
            &["acquire", "lane", "stopped"],
            &[
                ("CDO_SLOT_TEST_PAUSE_HOLDGO_S", "60"),
                ("CDO_SLOT_LOCK_WAIT_S", "2"),
                ("CDO_SLOT_TEST_SCAN_ONCE", "1"),
            ],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    s.wait_paused();
    let k = keepers(&s, b"_keep\0hold\0");
    assert_eq!(k.len(), 1, "precondition: one hold keeper");
    // SAFETY: SIGSTOP to this test's own hold keeper.
    unsafe { libc::kill(k[0], libc::SIGSTOP) };
    s.resume();
    let started = std::time::Instant::now();
    let code = loop {
        if let Some(st) = held.try_wait().unwrap() {
            break st.code();
        }
        // 30 s: the 2 s go bound plus the keeper's end (TERM, then KILL after
        // 5 s) on a loaded host; the break waits for ever.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the go waited for a stopped keeper"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(&mut held.stderr.take().unwrap(), &mut err).unwrap();
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("nothing is held"), "{err}");
    until("the stopped keeper was ended", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });
}

/// ⟨TLS-89's landing⟩ `release` finds its keeper among every process that has
/// the hold's id file open, other ones included (a probe has it open for an
/// instant). The landing's full nextest run failed the queue test this way:
/// `D release` exited 1 when a probe listed after the keeper had the file
/// open, because `keepers_of`'s last `grep && echo` left status 1 and `set -e`
/// ended the script at `ps=$(keepers_of ...)`. Here a stand-in started after the
/// keeper (so listed after it) keeps the file open for the whole release.
///
/// Mutation (2026-11-19): `keepers_of` back to `grep -q ... && echo "$p"` ->
/// `release` exits 1 (red); restored, green.
#[test]
fn cdo_slot_release_ignores_other_openers_of_the_hold_file() {
    let s = Slot::new();
    assert_eq!(s.run("D", &["acquire", "d", "held"], &[]).0, Some(0));
    let status = std::fs::read_to_string(s.dir().join("status")).unwrap();
    let hid = status
        .lines()
        .find_map(|l| l.strip_prefix("hid="))
        .expect("the status names its hold id")
        .to_string();
    let file = s.dir().join("id").join(format!("hold.{hid}"));
    let opener = stand_in(
        &s,
        "bash",
        &[
            "-c".as_ref(),
            "exec 3<\"$0\"; sleep 600".as_ref(),
            file.as_os_str(),
        ],
    );
    let opener_pid = opener.id();
    until("the stand-in has the hold file open", &|| {
        std::fs::read_dir(format!("/proc/{opener_pid}/fd"))
            .map(|d| {
                d.flatten()
                    .any(|e| std::fs::read_link(e.path()).ok() == Some(file.clone()))
            })
            .unwrap_or(false)
    });
    let (code, text) = s.run("D", &["release"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(s.run("B", &["status"], &[]), (Some(0), "free\n".into()));
    drop(opener);
}

/// ⟨TLS-89's manual landing⟩ The real slot folder keeps what older versions
/// wrote: a `timed` FILE (TLS-89's first design kept a timing mark there), a
/// `meta` lock file, a status with no hold id. In it, cdo-gate's hold (`acquire
/// --wait --bind <its pid>`, its token exported) and cdo-speed's timed share
/// (`acquire --wait --bind <its pid> --timed`, the same token) must work: the
/// landing's speed phase got "--timed: the timing lock was not taken" (exit 5),
/// its keeper dead at `mkdir -p .../timed` on that file. And a folder the
/// timing cannot make is a refusal that names it, never a silent keeper death.
///
/// Mutations (2026-11-19): the timing folder named `timed` again -> the share
/// exits 5 (red); the `mkdir` check in `take_timed` dropped (the keeper dies
/// under `set -e`) -> the refusal names no folder, only the keeper's end (red).
/// Each restored, green.
#[test]
fn cdo_slot_works_in_a_folder_an_older_version_left() {
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    std::fs::write(
        s.dir().join("timed"),
        "owner=cdo-speed\nreason=cdo-speed timing\nsince=2026-11-18T02:36:11Z\npid=2141252\n",
    )
    .unwrap();
    std::fs::write(s.dir().join("meta"), "").unwrap();
    std::fs::write(
        s.dir().join("status"),
        "owner=old\nreason=x\nsince=2026-11-18T00:00:00Z\nsession=old\nholder_pid=1\ntoken=t\n",
    )
    .unwrap();
    let token = ("CDO_SLOT_TOKEN", "tls89-gate");
    let gate = sleeper();
    let gpid = gate.id().to_string();
    let (code, text) = s.run(
        "G",
        &["acquire", "cdo-gate", "cdo-gate", "--wait", "--bind", &gpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");
    let speed = sleeper();
    let spid = speed.id().to_string();
    let (code, text) = s.run(
        "G",
        &[
            "acquire",
            "cdo-speed",
            "cdo-speed timing",
            "--wait",
            "--bind",
            &spid,
            "--timed",
        ],
        &[token],
    );
    assert_eq!(
        code,
        Some(4),
        "the speed phase's timed share failed: {text}"
    );
    assert!(text.contains("timed by cdo-speed"), "{text}");
    assert_eq!(s.probe(), "held=1 own=0 timed=1");
    drop(speed);
    drop(gate);
    until("free", &|| s.probe() == "held=0 own=0 timed=0");

    // The timing folder's name taken by a file (a slot of its own): a refusal
    // that names it.
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    std::fs::write(s.dir().join("timings"), "").unwrap();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t = sleeper();
    let tpid = t.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "timing", "--bind", &tpid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(5), "{text}");
    assert!(text.contains("cannot make the folders"), "{text}");
    assert_eq!(s.probe(), "held=1 own=0 timed=0", "nothing timed");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}
