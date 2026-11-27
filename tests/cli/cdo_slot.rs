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

/// `cmd` without any slot, Claude or other setting this test process inherited
/// (a landing runs the suite inside its own hold: its token, its tuning, its
/// session), so a slot test gives the same result inside a hold and outside.
/// TLS-115: `crate::clean_env`, so a `BASH_ENV` or gate knob is gone too.
fn scrubbed(mut cmd: std::process::Command) -> std::process::Command {
    crate::clean_env::clean_env(&mut cmd);
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
        Self::new_in(&std::env::temp_dir())
    }
    /// ⟨TLS-135 review r12, Fable⟩ A slot under the repository's `target/`, on
    /// the repository's own filesystem (btrfs on server1, where `/tmp` is
    /// overlay): a lock's key there differs from `stat`'s, and a slot under
    /// `/tmp` never showed it.
    fn on_repo_fs() -> Self {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/cdo-slot-tests");
        std::fs::create_dir_all(&base).unwrap();
        Self::new_in(&base)
    }
    fn new_in(base: &std::path::Path) -> Self {
        let tmp = tempfile::tempdir_in(base).unwrap();
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
            .env("CLAUDE_CODE_SESSION_ID", session);
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
///
/// ⟨TLS-127⟩ "Still writing" once ended 5 s after the lock was taken, and this
/// test's pause plus reads passed that on a loaded host ("held by an unknown
/// holder"). The writer now keeps the publish lock until its status is in, so
/// "still writing" lasts exactly as long as the write; see
/// `cdo_slot_a_slow_take_is_still_writing_not_unknown`.
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

/// ⟨TLS-127⟩ A holder between its hold lock and its status is "still writing"
/// however long that takes. It used to be judged by the lock file's age (under
/// 5 s), so a take slowed past 5 s (host load 150, a suspend, or the test
/// above, whose 3 s write pause and 2 s of reads start after the lock is taken)
/// read as "held by an unknown holder". Now the holder keeps the publish lock
/// (shared) from before its hold lock until its status is renamed in, and a
/// reader asks that lock. Here the lock file is aged by 60 s to stand for the
/// slow take. A lock held by a process that never writes a status is still an
/// unknown holder.
///
/// Mutation (2026-11-24): `describe` without the publish-lock check (the old
/// age rule back) -> "a slow take read as: held by an unknown holder (no status
/// names it) since" (red); restored, green.
#[test]
fn cdo_slot_a_slow_take_is_still_writing_not_unknown() {
    let s = Slot::new();
    let age_lock = || {
        let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(s.dir().join("lock"))
            .unwrap()
            .set_modified(aged)
            .unwrap();
    };
    // A status of an older hold, naming a dead id, as on every real take.
    assert_eq!(s.run("A", &["acquire", "lane", "first"], &[]).0, Some(0));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    let mut taker = s.bg(
        "A",
        &["acquire", "lane", "slow"],
        &[("CDO_SLOT_TEST_PAUSE_PUBLISH_S", "60")],
    );
    s.wait_paused();
    age_lock();
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(
        text.starts_with("held (its holder is still writing its status)"),
        "a slow take read as: {text}"
    );
    s.resume();
    assert_eq!(taker.0.wait().unwrap().code(), Some(0));
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(text.starts_with("held by lane (slow) since"), "{text}");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A holder that never writes a status (not a cdo-slot take) stays unknown.
    // --no-fork: the process Reap kills is the one that has the lock.
    let lock = s.dir().join("lock");
    let _stranger = stand_in(
        &s,
        "flock",
        &[
            "--no-fork".as_ref(),
            "-x".as_ref(),
            lock.as_os_str(),
            "sleep".as_ref(),
            "600".as_ref(),
        ],
    );
    until("the stranger holds the lock", &|| {
        s.probe().starts_with("held=1")
    });
    age_lock();
    let (_, text) = s.run("B", &["status"], &[]);
    assert!(
        text.starts_with("held by an unknown holder (no status names it)"),
        "{text}"
    );
}

/// ⟨TLS-127⟩ A reader whose first status read came before a take renamed its
/// status in, and whose publish probe comes after the take let the publish lock
/// go, reads the status once more and names the new holder. The READ hook holds
/// the reader between the two steps while the take finishes in that window.
///
/// Mutation (2026-11-24): the second read in `state` dropped -> "the finished
/// take read as: held by an unknown holder (no status names it)" (red);
/// restored, green.
#[test]
fn cdo_slot_a_take_that_finishes_mid_read_is_named() {
    let s = Slot::new();
    // A status of an older hold, naming a dead id: the reader's first read.
    assert_eq!(s.run("A", &["acquire", "lane", "first"], &[]).0, Some(0));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
    let mut taker = s.bg(
        "A",
        &["acquire", "lane", "late"],
        &[("CDO_SLOT_TEST_PAUSE_PUBLISH_S", "60")],
    );
    s.wait_paused();
    let reader = s
        .cmd("B", &["status"], &[("CDO_SLOT_TEST_PAUSE_READ_S", "60")])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    s.wait_paused();
    // The take finishes: status renamed in, publish lock let go.
    s.resume();
    assert_eq!(taker.0.wait().unwrap().code(), Some(0));
    std::fs::write(s.dir().join("resume-read"), "").unwrap();
    let out = reader.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("held by lane (late) since"),
        "the finished take read as: {text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-127 review r1, Fable⟩ A hold released after a reader saw the hold lock
/// held, and before it read the status, leaves no live id and no publish lock.
/// The reader probes the hold lock once more and reads the slot as free: a
/// `status` says "free", and an `acquire` without --wait takes it, instead of
/// "held by an unknown holder" with an empty holder list. The HELD hook holds
/// each reader in that window while the test releases. A third case keeps the
/// publish lock taken (shared, as a take attempt would) during the release: the
/// reader must still read free, not "still writing" (review r2, Fable).
///
/// Mutation (2026-11-24): the hold-lock re-probe in `state` dropped -> "a
/// released hold read as: held by an unknown holder (no status names it)"
/// (red); restored, green.
/// Mutation (2026-11-24, review r2): the first publish probe's hold-lock
/// re-probe dropped (`writing=1` at once) -> "a released hold read as: held
/// (its holder is still writing its status)" (red); restored, green.
#[test]
fn cdo_slot_a_release_during_a_read_reads_free() {
    let s = Slot::new();
    let held = [("CDO_SLOT_TEST_PAUSE_HELD_S", "60")];
    for (args, publish_taken) in [
        (&["status"][..], false),
        (&["acquire", "lane", "next"][..], false),
        (&["status"][..], true),
    ] {
        assert_eq!(s.run("A", &["acquire", "lane", "first"], &[]).0, Some(0));
        let publish = s.dir().join("publish.lock");
        let _publish = publish_taken.then(|| {
            stand_in(
                &s,
                "flock",
                &[
                    "--no-fork".as_ref(),
                    "-s".as_ref(),
                    publish.as_os_str(),
                    "sleep".as_ref(),
                    "600".as_ref(),
                ],
            )
        });
        if publish_taken {
            until("the publish lock is taken", &|| {
                std::process::Command::new("flock")
                    .args(["-x", "-n"])
                    .arg(s.dir().join("publish.lock"))
                    .arg("true")
                    .status()
                    .unwrap()
                    .code()
                    == Some(1)
            });
        }
        let reader = s
            .cmd("B", args, &held)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        s.wait_paused();
        assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
        s.resume();
        let out = reader.wait_with_output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "a released hold read as: {text}"
        );
        if args[0] == "status" {
            assert_eq!(text.trim_end(), "free", "a released hold read as: {text}");
        } else {
            let (_, now) = s.run("B", &["status"], &[]);
            assert!(now.starts_with("held by lane (next) since"), "{now}");
            assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
        }
    }
}

/// ⟨TLS-127 review, sol⟩ A publish lock that cannot be probed does not undo a
/// confirmed hold: a stranger holds the hold lock, `publish.lock` is a regular
/// file nobody may read (mode 000: flock cannot open it), and the guard still
/// refuses another session's CDO command. The publish probe is best effort: a
/// failed one used to end `probe` (exit 5) before its decision line, and the
/// guard then let the command run.
///
/// Mutation (2026-11-25): `publish_busy` calling `probe` outside its subshell
/// (its exit 5 ends the script) -> "the guard let it run beside a held slot:
/// cdo-slot-guard: ... probe failed" (red, exit 0); restored, green. (Then with
/// a socket as `publish.lock`; since review r2 a socket is refused before any
/// open, so the test uses an unreadable regular file to keep pinning this.)
#[test]
fn cdo_slot_an_unprobeable_publish_lock_keeps_the_hold() {
    use std::os::unix::fs::PermissionsExt;
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let publish = s.dir().join("publish.lock");
    std::fs::write(&publish, "").unwrap();
    std::fs::set_permissions(&publish, std::fs::Permissions::from_mode(0o000)).unwrap();
    // As root the mode would not stop the open: the test would prove nothing.
    assert!(
        std::fs::File::open(&publish).is_err(),
        "precondition: publish.lock cannot be opened"
    );
    let lock = s.dir().join("lock");
    let _stranger = stand_in(
        &s,
        "flock",
        &[
            "--no-fork".as_ref(),
            "-x".as_ref(),
            lock.as_os_str(),
            "sleep".as_ref(),
            "600".as_ref(),
        ],
    );
    // Asked of the lock itself, not of `probe` (the code under test).
    until("the stranger holds the lock", &|| {
        lock.exists()
            && std::process::Command::new("flock")
                .args(["-s", "-n"])
                .arg(&lock)
                .arg("true")
                .status()
                .unwrap()
                .code()
                == Some(1)
    });
    let (code, text) = s.guard("B", "scripts/cdo-gate --only harness");
    assert_eq!(
        code,
        Some(2),
        "the guard let it run beside a held slot: {text}"
    );
}

/// ⟨TLS-127 review r2, sol⟩ A `publish.lock` that is not a regular file is
/// never opened: opening a FIFO with no writer blocks, so the guard's probe hung
/// until the guard cut it (30 s) and then let the command run. With a stranger
/// holding the hold lock and a FIFO as `publish.lock`, the guard refuses at
/// once, and a take refuses ("not a regular file") instead of blocking.
///
/// Mutation (2026-11-26): the regular-file check in `publish_busy` dropped ->
/// "the guard let it run beside a held slot" after the guard's 30 s cut (red);
/// restored, green. Since review r3 the reader also opens the file read-write,
/// which never blocks, so the check is one of two guards: the mutation now drops
/// the check AND makes the open read-only (`exec 9<`); each alone stays green.
#[test]
fn cdo_slot_a_fifo_publish_lock_is_never_opened() {
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let publish = s.dir().join("publish.lock");
    let status = std::process::Command::new("mkfifo")
        .arg(&publish)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo");
    let lock = s.dir().join("lock");
    let stranger = stand_in(
        &s,
        "flock",
        &[
            "--no-fork".as_ref(),
            "-x".as_ref(),
            lock.as_os_str(),
            "sleep".as_ref(),
            "600".as_ref(),
        ],
    );
    until("the stranger holds the lock", &|| {
        lock.exists()
            && std::process::Command::new("flock")
                .args(["-s", "-n"])
                .arg(&lock)
                .arg("true")
                .status()
                .unwrap()
                .code()
                == Some(1)
    });
    let started = std::time::Instant::now();
    let (code, text) = s.guard("B", "scripts/cdo-gate --only harness");
    assert_eq!(
        code,
        Some(2),
        "the guard let it run beside a held slot: {text}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the guard's probe waited on the FIFO: {:?}",
        started.elapsed()
    );
    drop(stranger);
    until("the stranger let go", &|| s.probe().starts_with("held=0"));
    let started = std::time::Instant::now();
    let (code, text) = s.run("A", &["run", "lane", "fifo", "--", "true"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is not a regular file"), "{text}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the take waited on the FIFO"
    );
}

/// ⟨TLS-127 review, sol⟩ A take waits for the publish lock only so long, and
/// never holds the hold lock meanwhile: with a stuck exclusive holder of
/// `publish.lock` (a stopped reader's probe), a `run` and a keeper-backed
/// `acquire --wait` each fail after CDO_SLOT_LOCK_WAIT_S (2 s) with a message
/// that names the publish lock, and the hold lock reads free throughout. Under a
/// 60 s lock wait, an `acquire --timeout 2` waits for the publish lock what is
/// left of its 2 s at the attempt (the message's "held for X s", X <= 2), and
/// an `acquire --timeout 0` does not wait at all ("held for 0.000000 s").
///
/// Mutations (2026-11-25): the hold lock taken before the publish lock in
/// `take_hold` -> "the hold lock was taken while waiting for the publish lock"
/// (red); the publish wait not cut to --timeout -> "the publish wait outlasted
/// --timeout" (red, after 60 s); each restored, green.
/// Mutation (2026-11-26, review r2): the wait rounded up to whole seconds with
/// at least 1 s (the r1 rule) -> "--timeout 0 still waited: ... held for 1 s"
/// (red); restored, green.
#[test]
fn cdo_slot_a_stuck_publish_lock_refuses_takes_in_bounds() {
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let publish = s.dir().join("publish.lock");
    std::fs::write(&publish, "").unwrap();
    let _stuck = stand_in(
        &s,
        "flock",
        &[
            "--no-fork".as_ref(),
            "-x".as_ref(),
            publish.as_os_str(),
            "sleep".as_ref(),
            "600".as_ref(),
        ],
    );
    until("the stand-in has the publish lock", &|| {
        std::process::Command::new("flock")
            .args(["-s", "-n"])
            .arg(&publish)
            .arg("true")
            .status()
            .unwrap()
            .code()
            == Some(1)
    });
    // Each take, polled until it ends: (exit, output, seconds, hold lock ever held).
    let take = |args: &[&str], env: &[(&str, &str)]| {
        let started = std::time::Instant::now();
        let mut child = s
            .cmd("A", args, env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut held = false;
        while child.try_wait().unwrap().is_none() {
            held |= s.probe().starts_with("held=1");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let out = child.wait_with_output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text, started.elapsed(), held)
    };
    let two = [("CDO_SLOT_LOCK_WAIT_S", "2")];
    for args in [
        &["run", "lane", "ran", "--", "true"][..],
        &["acquire", "lane", "kept", "--wait"][..],
    ] {
        let (code, text, _, held) = take(args, &two);
        assert_eq!(code, Some(1), "{args:?}: {text}");
        assert!(
            text.contains("the publish lock") && text.contains("held for 2 s"),
            "{args:?}: {text}"
        );
        assert!(
            !held,
            "{args:?}: the hold lock was taken while waiting for the publish lock"
        );
        assert_eq!(s.probe(), "held=0 own=0 timed=0", "{args:?}");
    }
    // The seconds the take waited, from its message "held for X s".
    let waited = |text: &str| -> f64 {
        let x = text
            .split("held for ")
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .unwrap_or_else(|| panic!("no wait in: {text}"));
        x.parse()
            .unwrap_or_else(|_| panic!("wait {x:?} in: {text}"))
    };
    let sixty = [("CDO_SLOT_LOCK_WAIT_S", "60")];
    let (code, text, took, _) = take(
        &["acquire", "lane", "timed-out", "--wait", "--timeout", "2"],
        &sixty,
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        waited(&text) <= 2.0 && took < std::time::Duration::from_secs(20),
        "the publish wait outlasted --timeout ({took:?}): {text}"
    );
    let (code, text, _, _) = take(
        &["acquire", "lane", "no-time", "--wait", "--timeout", "0"],
        &sixty,
    );
    assert_eq!(code, Some(1), "{text}");
    assert_eq!(waited(&text), 0.0, "--timeout 0 still waited: {text}");
}

/// ⟨TLS-127 review r3, sol⟩ A host suspend between an acquire's handoff and its
/// keeper's publish-lock attempt does not use up the --timeout: the deadline
/// the keeper gets is on CLOCK_MONOTONIC, which stops in a suspend (the wall
/// clock is only the fall back). The keeper is held in the TAKE hook for 6 s,
/// all of them "suspended" (`CDO_HOST_SUSPENDED_FILE` 100 -> 106), so of a
/// 10 s --timeout it still waits most for the stuck publish lock (over 5 s, at
/// most 10 s); on the wall clock it would have had at most 4 s left.
///
/// Mutation (2026-11-26): the take using the wall-clock deadline only ->
/// "the suspend used up the publish wait: ... held for 3.9... s" (red);
/// restored, green.
#[test]
fn cdo_slot_a_suspend_during_the_handoff_keeps_the_publish_wait() {
    let s = Slot::new();
    std::fs::create_dir_all(s.dir()).unwrap();
    let publish = s.dir().join("publish.lock");
    std::fs::write(&publish, "").unwrap();
    let _stuck = stand_in(
        &s,
        "flock",
        &[
            "--no-fork".as_ref(),
            "-x".as_ref(),
            publish.as_os_str(),
            "sleep".as_ref(),
            "600".as_ref(),
        ],
    );
    until("the stand-in has the publish lock", &|| {
        std::process::Command::new("flock")
            .args(["-s", "-n"])
            .arg(&publish)
            .arg("true")
            .status()
            .unwrap()
            .code()
            == Some(1)
    });
    let reading = s.0.path().join("suspended");
    std::fs::write(&reading, "100").unwrap();
    let reading_s = reading.to_str().unwrap();
    let taker = s
        .cmd(
            "A",
            &["acquire", "lane", "handoff", "--wait", "--timeout", "10"],
            &[
                ("CDO_SLOT_LOCK_WAIT_S", "60"),
                ("CDO_SLOT_TEST_PAUSE_TAKE_S", "60"),
                ("CDO_HOST_SUSPENDED_FILE", reading_s),
            ],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    s.wait_paused();
    // Six seconds pass in the keeper's window, all of them suspended.
    let paused_at = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_secs(6));
    let slept = paused_at.elapsed().as_secs();
    std::fs::write(&reading, format!("{}", 100 + slept)).unwrap();
    s.resume();
    let out = taker.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{text}");
    let x: f64 = text
        .split("held for ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no wait in: {text}"));
    assert!(
        x > 5.0 && x <= 10.0,
        "the suspend used up the publish wait: {text}"
    );
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
/// first: the proof run did; since TLS-135 r8 #1 runs its two timings one
/// after the other, so this break no longer applies); the keeper's writer watch dropped (wait for go
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
    // One timing at a time (TLS-135 r8): the second starts once the first,
    // killed with its bound process, has ended; probes read timed throughout.
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "first", "--bind", &p1, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    drop(t1);
    until("the first timing ended", &|| {
        s.probe() == "held=1 own=0 timed=0"
    });
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &p2, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
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
        .env("CLAUDE_CODE_SESSION_ID", "A");
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

/// `acquire <args>` from the cdo-slot at `script` as session B, run to its end
/// or killed after `limit`; (exit, stdout + stderr, ended in time, seconds from
/// before its start to its end).
fn run_bounded(
    s: &Slot,
    script: &std::path::Path,
    args: &[&str],
    limit: std::time::Duration,
) -> (Option<i32>, String, bool, std::time::Duration) {
    run_bounded_as(s, script, "B", args, limit)
}

/// [`run_bounded`] as session `session`.
fn run_bounded_as(
    s: &Slot,
    script: &std::path::Path,
    session: &str,
    args: &[&str],
    limit: std::time::Duration,
) -> (Option<i32>, String, bool, std::time::Duration) {
    let spawned = std::time::Instant::now();
    let mut b = Reap(
        s.cmd_at(script, session, args, &[])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = std::time::Instant::now();
    let (code, in_time) = loop {
        if let Some(st) = b.0.try_wait().unwrap() {
            break (st.code(), true);
        }
        if started.elapsed() >= limit {
            b.0.kill().unwrap();
            // Its children too (a hung stand-in clock keeps the output open).
            kill_users(&s.dir());
            break (b.0.wait().unwrap().code(), false);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let took = spawned.elapsed();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut b.0.stdout.take().unwrap(), &mut text).unwrap();
    std::io::Read::read_to_string(&mut b.0.stderr.take().unwrap(), &mut text).unwrap();
    (code, text, in_time, took)
}

/// ⟨TLS-105 review r2, sol⟩ The fall back to wall time is anchored to the wall
/// clock read AFTER the last good clock reading, so a suspend inside that
/// reading is not charged to the wait. The stand-in clock's second read takes
/// 4 s (under the reads' 5 s cut) in which the host "sleeps" (the suspended
/// reading 100 -> 104, so awake time does not move), then every read fails.
/// With a 6 s timeout the wait falls back at the fifth read, about 7 s of wall
/// time in, of which 4 s were the suspend: it must give up only after 6 s of
/// awake time, so at least 10 s of wall time after its start (the bug gave up
/// at the fifth read, ~7 s). A first version counted reads (at least 6), which
/// a loaded host broke: its slow iterations rightly used up the awake time by
/// the fifth read (a full suite's run). Load only makes this wait longer.
///
/// Mutation (2026-11-23): the wall clock read before the clocks again (as in
/// 4b059aa6) -> gave up at the fifth read (red); restored, green. Measure
/// changed to the time: re-proved, red (gave up 7.6 s after its start);
/// restored, green.
#[test]
fn cdo_slot_fall_back_does_not_charge_a_suspend_in_the_last_good_read() {
    let s = Slot::new();
    let real = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let bin = link_folder(
        &s,
        &["cdo-slot", "awake-clock"],
        Some(&format!(
            "d=$(dirname \"$0\")\nexport CDO_HOST_SUSPENDED_FILE=\"$d/suspended\"\n\
             if [[ \"$1\" == --clocks ]]; then\n\
               n=$(( $(cat \"$d/n\" 2>/dev/null || echo 0) + 1 )); echo $n > \"$d/n\"\n\
               case $n in 1) ;; 2) sleep 4; echo 104 >\"$d/suspended\" ;; *) exit 1 ;; esac\n\
             fi\n\
             exec '{real}' \"$@\""
        )),
    );
    std::fs::write(bin.join("suspended"), "100").unwrap();
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let (code, text, in_time, took) = run_bounded(
        &s,
        &bin.join("cdo-slot"),
        &["acquire", "b", "late", "--wait", "--timeout", "6"],
        std::time::Duration::from_secs(60),
    );
    assert!(in_time, "never gave up: {text}");
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("not taken after 6 s"), "{text}");
    assert!(
        text.contains("the host slept 4 s during this wait: it goes on that much longer"),
        "precondition: the suspend was in the last good read: {text}"
    );
    assert!(
        text.contains("the awake clock failed 3 reads in a row"),
        "precondition: it fell back: {text}"
    );
    assert!(
        took >= std::time::Duration::from_millis(9500),
        "gave up {took:?} after its start: the suspend in the last good read was charged to the wait: {text}"
    );
}

/// ⟨TLS-105 review r2, sol⟩ A clock read that hangs is a failed read: each is
/// cut after 5 s (`timeout -k 1 5`), so after three the wait falls back to wall
/// time and gives up, instead of hanging with the clock. Review r3 (sol): the
/// hung read ignores TERM, so only the KILL a second later ends it.
///
/// Mutation (2026-11-23): the `timeout 5` dropped -> still waiting after 40 s
/// (red); restored, green. Review r3: the `-k 1` dropped (TERM only, which
/// the read ignores) -> still waiting after 40 s (red); restored, green.
#[test]
fn cdo_slot_a_clock_read_that_hangs_is_a_failed_read() {
    let s = Slot::new();
    let real = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let bin = link_folder(
        &s,
        &["cdo-slot", "awake-clock"],
        Some(&format!(
            "d=$(dirname \"$0\")\n\
             if [[ \"$1\" == --clocks && -e \"$d/read-once\" ]]; then trap '' TERM; exec sleep 600; fi\n\
             [[ \"$1\" == --clocks ]] && : >\"$d/read-once\"\n\
             exec '{real}' \"$@\""
        )),
    );
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let (code, text, in_time, _) = run_bounded(
        &s,
        &bin.join("cdo-slot"),
        &["acquire", "b", "late", "--wait", "--timeout", "2"],
        std::time::Duration::from_secs(40),
    );
    assert!(
        in_time,
        "still waiting after 40 s: a hung clock read hung the wait: {text}"
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("not taken after 2 s"), "{text}");
    let said = "the awake clock failed 3 reads in a row";
    assert_eq!(text.matches(said).count(), 1, "{text}");
}

/// ⟨TLS-105 review r3, sol⟩ cdo-slot's plain suspended reading (`status` and
/// `probe` print it for a hold; an acquire's waiting line too) is cut like the
/// clock reads: a stand-in whose plain read hangs and ignores TERM (its
/// `--clocks` read works) leaves `probe` and `status` answering within 20 s
/// (the guard's probe: one that hung would let work run), and `acquire
/// --timeout 2` giving up within 20 s.
///
/// Mutation (2026-11-23): `host_suspended` without its `timeout -k 1 5` ->
/// `probe` still running after 20 s (red); restored, green.
#[test]
fn cdo_slot_a_plain_suspended_read_that_hangs_is_cut() {
    let s = Slot::new();
    let real = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/host-suspended");
    let bin = link_folder(
        &s,
        &["cdo-slot", "awake-clock"],
        Some(&format!(
            "[[ \"$1\" == --clocks ]] || {{ trap '' TERM; exec sleep 600; }}\nexec '{real}' \"$@\""
        )),
    );
    let copy = bin.join("cdo-slot");
    // A hold whose description reads the suspended seconds.
    assert_eq!(s.run("A", &["acquire", "a", "hold"], &[]).0, Some(0));
    let limit = std::time::Duration::from_secs(20);
    let (code, text, in_time, _) = run_bounded_as(&s, &copy, "P", &["probe"], limit);
    assert!(in_time, "probe still running after 20 s: {text}");
    assert_eq!(code, Some(0), "{text}");
    assert!(text.starts_with("held=1 own=0 timed=0"), "{text}");
    let (code, text, in_time, _) = run_bounded_as(&s, &copy, "P", &["status"], limit);
    assert!(in_time, "status still running after 20 s: {text}");
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("held by a (hold)"), "{text}");
    let (code, text, in_time, _) = run_bounded_as(
        &s,
        &copy,
        "B",
        &["acquire", "b", "late", "--wait", "--timeout", "2"],
        limit,
    );
    assert!(in_time, "acquire still waiting after 20 s: {text}");
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("not taken after 2 s"), "{text}");
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
/// - One timing at a time (TLS-135 review r8, replacing TLS-89's "a second
///   timing in that hold waits for a build too"): a second timing is refused
///   at once while the first runs, and starts once it ended; a refused `acquire
///   --bind --timed` with no session or token frees the slot it took.
/// - Review r1 (sol): the hold it shares is released while it waits for a
///   build -> refused once the build ends, never a start with no hold.
/// - A hold released while a timing in it runs leaves the timing on, and no
///   other session can take the slot until the timing ends (owner decision
///   2026-11-19: a timing keeps every new hold out).
/// - Review r3/r4 (sol): the hold it took or shared is replaced in the window
///   right after the acquire (the test hook pauses there) -> refused, and the
///   replacement is left alone and untimed.
/// - Review r3/r4 (sol): a second timing bound to the SAME process (a retry)
///   leaves the first timing on and takes none (since TLS-135 r10: that
///   process owns the timing by binding, and is told "already timed ... a
///   duplicate request", exit 4, before it waits for any hold).
/// - Review r5 (sol): a timing that TOOK the slot waits for a build; a
///   token-sharing timing is refused at once (TLS-135 r8), and the taker,
///   refused for its build, frees the slot.
/// - A `run`'s hold is not released by `release`: it ends with the run.
///
/// Mutations (2026-11-19): the hold probe after the drain dropped -> the
/// released hold's timing starts (red); the `timed_now` check dropped from
/// `acquire` -> B takes the slot beside the timing (red); `refuse` ending
/// every timing keeper instead of its own -> the retry's timing ends (red; a
/// break that needs two timings at once, which TLS-135 r8 no longer allows).
/// Each restored, green. (2026-11-18): the refused acquire's keeper end
/// dropped -> the slot stays held (red). (TLS-135 r8) the gate's check
/// dropped -> the second timing beside the first exits 4 (red). (TLS-135 r10)
/// the binding owner dropped from `timing_scan` -> the retry bound to the same
/// process is refused as foreign (exit 6, red); restored, green.
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

    // One timing at a time (TLS-135 review r8): a second one is refused at
    // once while the first runs, and starts once it ended.
    let t1 = sleeper();
    let t2 = sleeper();
    let (t1pid, t2pid) = (t1.id().to_string(), t2.id().to_string());
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "first", "--bind", &t1pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(6), "a second timing beside the first: {text}");
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    assert!(
        timed_line().contains("(first)"),
        "the refusal ended the first"
    );
    drop(t1);
    until("the first timing ended", &|| {
        s.probe() == "held=1 own=0 timed=0"
    });
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
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

    // A retry bound to the same process (its owner by binding) takes no second
    // timing and leaves the first on: "already timed", a duplicate.
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t7 = sleeper();
    let t7pid = t7.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "kept", "--bind", &t7pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "refused", "--bind", &t7pid, "--timed"],
        &[names],
    );
    assert_eq!(code, Some(4), "{text}");
    assert!(
        text.contains("already timed") && text.contains("a duplicate request"),
        "{text}"
    );
    let text = timed_line();
    assert!(
        text.contains("(kept)") && !text.contains("(refused)"),
        "{text}"
    );
    drop(t7);
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));

    // A timing that TOOK the slot and waits for a build: a token-sharing
    // timing is refused at once (one timing at a time), and the taker, refused
    // for its build, frees the slot.
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
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "sharer", "--bind", &t12pid, "--timed"],
        &[names, ("CDO_SLOT_TOKEN", "tls89-shared")],
    );
    assert_eq!(code, Some(6), "{text}");
    drop(b11);
    s.resume();
    assert_eq!(taker.0.wait().unwrap().code(), Some(6));
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

/// ⟨TLS-135⟩ A folder of links to the real gate scripts, with `ci-steps` linked
/// under a name no real process has (`ci-steps-t135-<pid>-<n>`, for
/// CDO_SLOT_BUILD_SCRIPTS), and a stub `cargo` first on PATH. The stub appends
/// `beside` to `<tmp>/cargo-log` while a timing of the test runs (`<tmp>/on`
/// exists, `<tmp>/off` does not), else `after`; with TLS135_LEAK set it also
/// leaves a `sleep 600` behind (an orphan once the gate ends); with TLS135_HOLD
/// set, `cargo fmt` (or `cargo <TLS135_HOLD_CMD>`) touches `<tmp>/<cmd>-running`
/// and waits until that file exists. Stub `tree-stamp`, `nextest-priority-check`, `python3` and
/// `check-ir-image` let `ci-steps task` and `all` run. Returns (folder, script
/// name).
fn gate_folder(s: &Slot) -> (std::path::PathBuf, String) {
    use std::os::unix::fs::PermissionsExt;
    let bin = s.0.path().join("gate");
    std::fs::create_dir_all(&bin).unwrap();
    let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    // One name per folder: tests run side by side in one process, and a
    // timing's drain must not count another test's gate as its build.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("ci-steps-t135-{}-{n}", std::process::id());
    std::os::unix::fs::symlink(real.join("ci-steps"), bin.join(&name)).unwrap();
    for f in [
        "cdo-slot",
        "awake-clock",
        "host-suspended",
        "build-entry",
        "check-goldens",
        "require-rttime",
    ] {
        std::os::unix::fs::symlink(real.join(f), bin.join(f)).unwrap();
    }
    let t = s.0.path().display().to_string();
    let cargo = bin.join("cargo");
    std::fs::write(
        &cargo,
        format!(
            "#!/bin/bash\n[ \"$2\" = --version ] && exit 0\n\
             if [ -n \"$TLS135_HOLD\" ] && [ \"$1\" = \"${{TLS135_HOLD_CMD:-fmt}}\" ]; then touch {t}/$1-running; while [ ! -e \"$TLS135_HOLD\" ]; do sleep 0.05; done; fi\n\
             if [ -e {t}/on ] && [ ! -e {t}/off ]; then echo beside; else echo after; fi >>{t}/cargo-log\n\
             [ -z \"$TLS135_LEAK\" ] || (sleep 600 </dev/null >/dev/null 2>&1 &)\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (f, body) in [
        (
            "tree-stamp",
            format!("[ \"$1\" = --path ] && echo {t}/stamp || echo stamp"),
        ),
        ("nextest-priority-check", "exit 0".to_string()),
        // `ci-steps all`'s agentflow step (pytest) and build check.
        ("python3", "exit 0".to_string()),
        ("check-ir-image", "exit 0".to_string()),
        // `ci-steps task`'s locale step (TLS-141).
        ("check-parse-path-locale", "exit 0".to_string()),
    ] {
        std::fs::write(bin.join(f), format!("#!/bin/bash\n{body}\n")).unwrap();
        std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // `ci-steps task`'s gate-env step runs in the repository's `scripts/`
    // (its pytest is the python3 stub above), for a test that runs in `repo/`.
    std::fs::create_dir_all(s.0.path().join("repo/scripts")).unwrap();
    (bin, name)
}

/// A new, empty log file opened for appending: stdout and an appending stderr
/// on the same file then never write over each other.
fn log_file(p: &std::path::Path) -> std::fs::File {
    std::fs::File::create(p).unwrap();
    std::fs::OpenOptions::new().append(true).open(p).unwrap()
}

/// `<wrap...> bash <gate>/<script> fmt` (its `cargo fmt` is the stub) as a
/// command of slot `s`, with BASE_ENV, the stub first on PATH, then `env`.
/// `script` may carry its own arguments after a space (`check-goldens --regen`);
/// then no `fmt` is added.
fn gate_cmd(
    s: &Slot,
    gate: &std::path::Path,
    script: &str,
    wrap: &[&std::ffi::OsStr],
    env: &[(&str, &str)],
) -> std::process::Command {
    let bash: &std::ffi::OsStr = "bash".as_ref();
    let (prog, pre) = wrap.split_first().unwrap_or((&bash, &[]));
    let mut cmd = scrubbed(std::process::Command::new(prog));
    cmd.args(pre);
    if !wrap.is_empty() {
        cmd.arg("bash");
    }
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut words = script.split(' ');
    cmd.arg(gate.join(words.next().unwrap()));
    let rest: Vec<&str> = words.collect();
    cmd.args(if rest.is_empty() { vec!["fmt"] } else { rest })
        .env("PATH", path)
        .env("CDO_SLOT_DIR", s.dir());
    for (k, v) in BASE_ENV.iter().chain(env) {
        cmd.env(k, v);
    }
    cmd
}

/// A timing of slot `s` by `run --timed` as session A: it touches `<tmp>/on`,
/// waits until `<tmp>/end` exists, then touches `<tmp>/off`.
fn test_timing(s: &Slot, env: &[(&str, &str)]) -> Reap {
    let t = s.0.path().display().to_string();
    let body = format!("touch {t}/on; while [ ! -e {t}/end ]; do sleep 0.05; done; touch {t}/off");
    s.bg(
        "A",
        &[
            "run", "lane", "timing", "--timed", "--", "bash", "-c", &body,
        ],
        env,
    )
}

/// ⟨TLS-135⟩ The landing that failed on 2026-11-25: a gate queued behind a lock
/// (`flock <lock> bash scripts/ci-steps ...`) before a timed hold began passed
/// the guard then, and started its build when the lock came free, in the middle
/// of the timing. Now ci-steps (and check-goldens) check at their own start and
/// wait for the timing to end, saying so; the wait is bounded and refuses.
///
/// Reproduction and mutation (2026-11-25): `build_entry ci-steps` commented out
/// in ci-steps (the master behaviour) -> "the queued gate built beside the
/// timing", cargo-log `beside` (red); restored, green.
#[test]
fn cdo_slot_a_gate_queued_before_a_timing_waits_for_it() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let lock = t.join("task-gate.lock");
    let log = t.join("gate.log");
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let log_has = |w: &str| {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(w)
    };

    // The earlier gate holds the lock; ours queues behind it.
    let release = t.join("release");
    let body = format!("while [ ! -e {} ]; do sleep 0.05; done", release.display());
    let _earlier = stand_in(
        &s,
        "flock",
        &[
            lock.as_os_str(),
            "bash".as_ref(),
            "-c".as_ref(),
            body.as_ref(),
        ],
    );
    until("the earlier gate holds the lock", &|| {
        !std::process::Command::new("flock")
            .args(["-n".as_ref(), lock.as_os_str(), "true".as_ref()])
            .status()
            .unwrap()
            .success()
    });
    let mut queued = Reap(
        gate_cmd(
            &s,
            &gate,
            &script,
            &["flock".as_ref(), lock.as_os_str()],
            &[],
        )
        .stdout(log_file(&log))
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap(),
    );

    let mut timing = test_timing(&s, &[]);
    until("the timing runs", &|| t.join("on").exists());
    std::fs::write(&release, "").unwrap();
    until("the queued gate builds, or waits for the timing", &|| {
        !cargo_log().is_empty() || log_has("waiting for the CDO timing to end")
    });
    assert_eq!(cargo_log(), "", "the queued gate built beside the timing");
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
    assert_eq!(queued.wait().unwrap().code(), Some(0));
    assert_eq!(cargo_log(), "after\n");
    assert!(log_has("the CDO timing ended"));
}

/// ⟨TLS-135⟩ What the entry check lets through, and its limit:
/// - A gate run by the timing's own command builds at once (perf-bounds' and
///   cdo-speed's builds run under their own timing).
/// - A step of a recorded run (even a quiet one) is checked like any start
///   (review r2, sol: the earlier "a run admits its steps" let a step launched
///   beside a timing build); an inherited `build_entry_admitted` skips nothing.
/// - Any other gate under a timing waits at most CDO_SLOT_ENTRY_WAIT_S, then
///   refuses (exit 1) and builds nothing; check-goldens the same.
///
/// Mutations (2026-11-25), each red, restored, green: `foreign_timing`'s
/// skip of its own line's timing dropped -> the timing's own gate exits 1;
/// `build_entry` returning 0 on the wait's exit 6 -> ci-steps exits 0 and
/// builds; check-goldens' `build_entry` before its run commented out -> it
/// exits 0. (2026-11-26, review r1) its `--regen` guard commented out -> the
/// regen builds (cargo-log `beside`). (review r2) the admission walk put back
/// in `enter_check` (an ancestor with a live record admits) -> the run's step
/// builds beside the timing; `build_entry`'s once-per-process check reading
/// any value (`[ -n ... ]`) -> the step with an inherited value builds.
#[test]
fn cdo_slot_gate_entry_passes_its_own_line_and_refuses_after_its_wait() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let short = ("CDO_SLOT_ENTRY_WAIT_S", "1");

    let (code, text) = s.run(
        "A",
        &[
            "run",
            "lane",
            "own",
            "--timed",
            "--",
            "bash",
            &gate_script,
            "fmt",
        ],
        &[("PATH", &path), short],
    );
    assert_eq!(code, Some(0), "the timing's own gate did not run: {text}");
    assert!(!text.contains("waiting for the CDO timing"), "{text}");
    assert_eq!(cargo_log(), "after\n");

    let mut timing = test_timing(&s, &[]);
    until("the timing runs", &|| t.join("on").exists());
    // Review r2 (sol): no run admits its steps. The outer shell is a recorded,
    // quiet entry (as `enter` and `build_step` leave it); its step checks for
    // itself and waits (here: refuses after 1 s). And an inherited
    // `build_entry_admitted` (not this process's) skips no check.
    let nested = format!(
        "{RECORD_SELF} bash '{SLOT}' quiet on; \
         build_entry_admitted=1 bash '{gate_script}' fmt; rc=$?; exit $rc"
    );
    let out = {
        let mut c = scrubbed(std::process::Command::new("bash"));
        c.args(["-c", &nested])
            .env("PATH", &path)
            .env("CDO_SLOT_DIR", s.dir());
        for (k, v) in BASE_ENV.iter().chain(&[short]) {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "a run's step: {text}");
    assert_eq!(cargo_log(), "after\n", "a run's step built: {text}");

    let out = gate_cmd(&s, &gate, &script, &[], &[short])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("refused: a CDO timing still runs after 1 s"),
        "{text}"
    );
    assert_eq!(cargo_log(), "after\n", "it built: {text}");
    // check-goldens the same (its stamp skip off: it would build nothing).
    let mut c = gate_cmd(&s, &gate, "check-goldens", &[], &[short]);
    let out = c.env("CHECK_GOLDENS_FORCE", "1").output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("check-goldens: refused: a CDO timing still runs"),
        "{text}"
    );
    assert_eq!(cargo_log(), "after\n", "check-goldens built: {text}");
    // Its regen guard too (review r1, sol): `--regen` builds before the run's.
    let out = gate_cmd(&s, &gate, "check-goldens --regen", &[], &[short])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("check-goldens: refused: a CDO timing still runs"),
        "{text}"
    );
    assert_eq!(
        cargo_log(),
        "after\n",
        "check-goldens --regen built: {text}"
    );
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
}

/// Bash that records its own shell as a build entry, as `cdo-slot enter`
/// writes it (`$CDO_SLOT_DIR/entries/<pid>.<start time>`).
const RECORD_SELF: &str = "mkdir -p \"$CDO_SLOT_DIR/entries\"; \
    st=$(awk '{print $22}' /proc/$$/stat); echo \"$$ $st\" >\"$CDO_SLOT_DIR/entries/$$.$st\";";

/// ⟨TLS-135⟩ The check and a timed acquire cannot miss each other:
/// - A gate that recorded itself and has not yet checked (the ENTER hook holds
///   it there) when a timing begins is a build to that timing: it waits for
///   it, by the record alone (no build name or script matches), and refuses
///   at CDO_SLOT_TIMED_WAIT_S.
/// - A gate parked in its wait (its record taken back) is no build to the
///   timing it parks behind, still draining, though its script name is a build
///   script there: counting it made the two wait on each other.
///
/// Mutations (2026-11-25), each red, restored, green: the entry records left
/// out of `foreign_builds` -> the timing starts beside the paused gate (exit
/// 0); `parked/` left out of its parked loop -> the timing is refused (exit
/// 6); `enter_check` not taking its record back on exit 3 -> the parked
/// gate's record refuses the timing (exit 6). (r8: re-proved on the drained
/// timing, one timing at a time.) (2026-11-26, review r1,
/// Fable) the order: the record written after the decision (the ENTER hook
/// kept just before the write) -> the timing starts beside the paused gate
/// (exit 0).
#[test]
fn cdo_slot_gate_entry_and_a_timing_never_miss_each_other() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let log = t.join("gate.log");
    let log_has = |w: &str| {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(w)
    };

    let mut early = Reap(
        gate_cmd(
            &s,
            &gate,
            &script,
            &[],
            &[("CDO_SLOT_TEST_PAUSE_ENTER_S", "60")],
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "true"],
        &[("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(code, Some(6), "the timing started beside a gate: {text}");
    assert!(
        text.contains(&format!("PID {} (entry)", early.id())),
        "{text}"
    );
    s.resume();
    assert_eq!(early.wait().unwrap().code(), Some(0));

    // A timing drains (it waits for a stand-in build); a gate that starts then
    // sees the published timing and parks behind it. The drain must not count
    // the parked gate (its script name is a build script here), or the two
    // wait on each other. (One timing at a time, TLS-135 r8: the parked gate is
    // met by the timing it parks behind, not by a second timing.)
    let fake_name = format!("t135p{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let build = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t1 = sleeper();
    let t1pid = t1.id().to_string();
    let mut timing = s.bg(
        "A",
        &["acquire", "lane", "first", "--bind", &t1pid, "--timed"],
        &[
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_str()),
            ("CDO_SLOT_BUILD_SCRIPTS", script.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "20"),
        ],
    );
    until("the timing is published", &|| {
        s.run("probe", &["status"], &[]).1.contains("(first)")
    });
    let mut parked = Reap(
        gate_cmd(&s, &gate, &script, &[], &[])
            .stdout(log_file(&log))
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the gate waits", &|| {
        log_has("waiting for the CDO timing to end")
    });
    drop(build);
    assert_eq!(
        timing.0.wait().unwrap().code(),
        Some(4),
        "the parked gate held up the timing"
    );
    drop(t1);
    assert_eq!(parked.wait().unwrap().code(), Some(0));
    assert!(log_has("the CDO timing ended"));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135, the orchestrator's review of the first design⟩ A finished gate
/// that left a child behind (an orphan with the gate's environment and open
/// descriptors) holds no timing up: only the live entry process counts. The
/// first design counted every holder of an inherited `building` descriptor, so
/// such an orphan stalled a timing for CDO_SLOT_TIMED_WAIT_S. And a record whose
/// PID now names another process (its start time differs) is no entry.
///
/// Mutations (2026-11-25), each red, restored, green: the first design's
/// scripts (447eef658's cdo-slot and build-entry) -> the orphan refuses the
/// timing (exit 6); the record's start-time check dropped from
/// `foreign_builds` -> the reused-PID record refuses the timing (exit 6).
/// (2026-11-26, review r1, sol) a dead record's cleanup removing every
/// record of its PID (`entries/<pid>.*`) -> the live record is gone and the
/// timing starts (exit 0).
#[test]
fn cdo_slot_a_finished_gates_orphan_holds_no_timing_up() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let mut leaky = gate_cmd(&s, &gate, &script, &[], &[("TLS135_LEAK", "1")]);
    let gate_run = leaky
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(Reap(gate_run).wait().unwrap().success());
    let want = format!("CDO_SLOT_DIR={}", s.dir().display());
    let orphan = std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .find(|p| {
            let comm = std::fs::read_to_string(format!("/proc/{p}/comm")).unwrap_or_default();
            let env = std::fs::read(format!("/proc/{p}/environ")).unwrap_or_default();
            comm.trim() == "sleep" && env.split(|b| *b == 0).any(|v| v == want.as_bytes())
        });
    assert!(orphan.is_some(), "precondition: the gate left an orphan");
    let (code, text) = s.run(
        "A",
        &["run", "lane", "after", "--timed", "--", "true"],
        &[("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(
        code,
        Some(0),
        "a finished gate's orphan held up a timing: {text}"
    );

    // A record naming a live process that is not the entry it recorded.
    let other = sleeper();
    let entries = s.dir().join("entries");
    std::fs::create_dir_all(&entries).unwrap();
    let stale = entries.join(format!("{}.1", other.id()));
    std::fs::write(&stale, format!("{} 1\n", other.id())).unwrap();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "reused", "--timed", "--", "true"],
        &[("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(
        code,
        Some(0),
        "a reused PID's record held up a timing: {text}"
    );

    // Review r1 (sol): removing the stale record leaves the live record of the
    // same PID (its own generation) in place, and that one still counts.
    std::fs::write(&stale, format!("{} 1\n", other.id())).unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", other.id())).unwrap();
    let start = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split(' ')
        .nth(19)
        .unwrap()
        .to_string();
    let live = entries.join(format!("{}.{start}", other.id()));
    std::fs::write(&live, format!("{} {start}\n", other.id())).unwrap();
    let (code, text) = s.run(
        "A",
        &["run", "lane", "live", "--timed", "--", "true"],
        &[("CDO_SLOT_TIMED_WAIT_S", "1")],
    );
    assert_eq!(code, Some(6), "the live record did not count: {text}");
    assert!(
        text.contains(&format!("PID {} (entry)", other.id())),
        "{text}"
    );
    assert!(!stale.exists(), "the stale record stayed");
    assert!(
        live.exists(),
        "the stale record's cleanup took the live one"
    );
}

/// ⟨TLS-135 review r1, sol and Fable⟩ TLS-108's deadlock by the record: a
/// lane's real `ci-steps perf-bounds` (a stand-in test binary, no build) is
/// admitted while a landing holds the slot, records itself, and queues for the
/// slot. The landing's timed acquire must not wait for it (the gate waits for
/// the landing's slot): the step is quiet while it waits (`build_step` around
/// its acquire). The same for a run that runs `perf-bounds` as a step (`ci-steps
/// all`; review r2, Fable): quiet while its step runs. And quiet is a state an
/// entry enters, never one read from the process tree (review r2, sol): an
/// entry whose waiter runs in the background is free to run on and stays a
/// build.
///
/// Mutations (2026-11-26), each red, restored, green: `build_step` around
/// perf-bounds' acquire replaced by a plain call -> the landing's timing is
/// refused, exit 6; `build_step` running its step without `quiet on` -> the
/// first timing is refused, exit 6 (that one break covers both the step's
/// acquire and the nested run's step: they share `build_step`, so the nested
/// part has no break of its own); `foreign_builds` skipping every
/// entry that is an ancestor of a waiter (r1's rule without its quiescence
/// test) -> the background waiter's entry is not counted (exit 4).
#[test]
fn cdo_slot_a_queued_entry_does_not_hold_up_the_timing_it_waits_for() {
    use std::os::unix::fs::PermissionsExt;
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/perf_bounds.rs"))
        .unwrap();
    let declared = src
        .split_once("\nmod release_checks")
        .unwrap()
        .1
        .lines()
        .filter(|l| l.trim() == "#[test]")
        .count();
    let bin = t.join("perf-bounds-stand-in");
    std::fs::write(
        &bin,
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = --list ]; then\n\
             case \" $* \" in *\" --ignored \"*) exit 0 ;; esac\n\
             for ((i = 0; i < {declared}; i++)); do echo \"release_checks::t$i: test\"; done; exit 0\n\
             fi\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let token = ("CDO_SLOT_TOKEN", "tls135-land");
    let holder = sleeper();
    let hpid = holder.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "landing", "--bind", &hpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");

    let queue = s.dir().join("queue");
    let queued = || {
        std::fs::read_dir(&queue)
            .map(|d| d.flatten().count())
            .unwrap_or(0)
    };
    let bin_s = bin.display().to_string();
    // Ended with the slot folder's processes when the test ends.
    let _lane = Reap(
        gate_cmd(
            &s,
            &gate,
            &format!("{script} perf-bounds"),
            &[],
            &[("CI_STEPS_PERF_BOUNDS_BIN", bin_s.as_str())],
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap(),
    );
    until("the lane's perf-bounds queues for the slot", &|| {
        queued() == 1
    });
    let t1 = sleeper();
    let t1pid = t1.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "timing", "--bind", &t1pid, "--timed"],
        &[
            token,
            ("CDO_SLOT_BUILD_SCRIPTS", script.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "3"),
        ],
    );
    assert_eq!(
        code,
        Some(4),
        "the queued gate held up the landing's timing: {text}"
    );
    drop(t1);
    until("the landing's timing ended", &|| {
        s.probe().ends_with("timed=0")
    });

    // The nested path (review r2, Fable): a run that runs `perf-bounds` as a
    // step (`ci-steps all`), through the real build_entry and build_step.
    let entry_lib = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/build-entry");
    let gate_script = gate.join(&script).display().to_string();
    let parent = format!(
        "source '{entry_lib}'; build_entry all || exit 9; \
         build_step bash '{gate_script}' perf-bounds; exit $?"
    );
    let probe_env = gate_cmd(
        &s,
        &gate,
        &script,
        &[],
        &[("CI_STEPS_PERF_BOUNDS_BIN", bin_s.as_str())],
    );
    let mut nested = scrubbed(std::process::Command::new("bash"));
    nested
        .args(["-c", &parent])
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    for (k, v) in probe_env.get_envs() {
        if let Some(v) = v {
            nested.env(k, v);
        }
    }
    let _nested = Reap(
        nested
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the nested perf-bounds queues", &|| queued() == 2);
    let tn = sleeper();
    let tnpid = tn.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "timing-n", "--bind", &tnpid, "--timed"],
        &[
            token,
            ("CDO_SLOT_BUILD_SCRIPTS", script.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "3"),
        ],
    );
    assert_eq!(
        code,
        Some(4),
        "the nested run's queued step held up the landing's timing: {text}"
    );
    drop(tn);
    until("the landing's timing ended", &|| {
        s.probe().ends_with("timed=0")
    });

    // Review r2 (sol): an entry whose waiter runs in the background is free to
    // run on; it did not say quiet, so it stays a build.
    let slot = SLOT;
    let body = format!(
        "{RECORD_SELF} bash '{slot}' acquire lane queued --wait --bind $$ & sleep 600; wait"
    );
    let _branchy = stand_in(&s, "bash", &["-c".as_ref(), body.as_ref()]);
    until("the third entry queues", &|| queued() == 3);
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "timing2", "--bind", &t2pid, "--timed"],
        &[token, ("CDO_SLOT_TIMED_WAIT_S", "2")],
    );
    assert_eq!(
        code,
        Some(6),
        "an entry with a step beside its waiter was not counted: {text}"
    );
    assert!(text.contains("(entry)"), "{text}");
    drop(t2);
}

/// ⟨TLS-135 review r1, sol⟩ An ancestor that a running timing does not count
/// (it is that timing's own ancestor) admits no new step: a gate its recorded
/// entry launches beside the timing, after the timer's scan, must check like
/// a new run, and waits (here: refuses after its 1 s). Review r2 (sol): the
/// same for a quiet entry a timing skipped: a step it launches after the
/// timing began checks for itself.
///
/// Mutations (2026-11-26), each red, restored, green: (r1) the walk in
/// `enter_check` no longer stopping at a timing's own line -> the gate is
/// admitted and builds beside the timing. (r2, no run admits its steps any
/// more) an admission walk put back in `enter_check` (an ancestor with a live
/// record admits) -> the first gate builds beside its timing (the second part
/// is not reached; the same break, so no separate proof).
#[test]
fn cdo_slot_a_timers_own_ancestor_admits_no_step_beside_it() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let gate_script = gate.join(&script).display().to_string();
    let td = t.display().to_string();
    let body = format!(
        "{RECORD_SELF} \
         bash '{SLOT}' run lane timing --timed -- bash -c 'touch {td}/on; while [ ! -e {td}/end ]; do sleep 0.05; done; touch {td}/off' & \
         until [ -e {td}/on ]; do sleep 0.05; done; \
         bash '{gate_script}' fmt; echo $? >{td}/gate-rc; touch {td}/end; wait"
    );
    let out = gate_cmd(&s, &gate, &script, &[], &[("CDO_SLOT_ENTRY_WAIT_S", "1")]);
    let mut c = scrubbed(std::process::Command::new("bash"));
    c.args(["-c", &body]);
    for (k, v) in out.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    let out = c.output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let rc = std::fs::read_to_string(t.join("gate-rc")).unwrap_or_default();
    assert_eq!(rc.trim(), "1", "the gate beside the timing: {text}");
    assert_eq!(cargo_log(), "", "it built beside the timing: {text}");
    for f in ["on", "off", "end"] {
        let _ = std::fs::remove_file(t.join(f));
    }

    // A quiet entry the timing skipped launches a gate after the timing began.
    let body = format!(
        "{RECORD_SELF} bash '{SLOT}' quiet on || exit 9; touch {td}/quiet; \
         until [ -e {td}/on ]; do sleep 0.05; done; \
         bash '{gate_script}' fmt; echo $? >{td}/gate-rc2"
    );
    let env = gate_cmd(&s, &gate, &script, &[], &[("CDO_SLOT_ENTRY_WAIT_S", "1")]);
    let mut c = scrubbed(std::process::Command::new("bash"));
    c.args(["-c", &body]);
    for (k, v) in env.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    let mut quiet = Reap(
        c.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the entry is quiet", &|| t.join("quiet").exists());
    let mut timing = test_timing(&s, &[]);
    until("the timing runs beside the quiet entry", &|| {
        t.join("on").exists()
    });
    assert!(quiet.wait().unwrap().success());
    let rc = std::fs::read_to_string(t.join("gate-rc2")).unwrap_or_default();
    assert_eq!(rc.trim(), "1", "the quiet entry's new step");
    assert_eq!(cargo_log(), "", "the quiet entry's new step built");
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
}

/// ⟨TLS-135 review r1, sol⟩ The wait is one deadline per gate: when one timing
/// ends and another begins before the gate's next decision, the gate's second
/// round counts from the first round's start, not from zero. And no sleep
/// passes the deadline (a 15 s poll does not stretch a 1 s wait).
///
/// Mutations (2026-11-26), each red, restored, green: `awake_start` moved into
/// `enter_wait`'s round loop -> the gate waits its whole 8 s again after the
/// second timing begins; the nap no longer capped to the time left -> the 1 s
/// wait takes its 15 s poll.
#[test]
fn cdo_slot_gate_entry_waits_one_deadline_in_all() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let log = t.join("gate.log");
    let log_has = |w: &str| {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(w)
    };

    // No sleep past the deadline.
    let mut timing = test_timing(&s, &[]);
    until("the timing runs", &|| t.join("on").exists());
    let started = std::time::Instant::now();
    let out = gate_cmd(
        &s,
        &gate,
        &script,
        &[],
        &[("CDO_SLOT_ENTRY_WAIT_S", "1"), ("CDO_SLOT_POLL_S", "15")],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the wait slept past its deadline: {:?}",
        started.elapsed()
    );
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
    for f in ["on", "off", "end"] {
        std::fs::remove_file(t.join(f)).unwrap();
    }

    // Timing A, the gate waits; A ends, and at the gate's next decision (the
    // ENTER hook holds it there) timing B begins.
    let mut a = test_timing(&s, &[]);
    until("timing A runs", &|| t.join("on").exists());
    let mut g = Reap(
        gate_cmd(
            &s,
            &gate,
            &script,
            &[],
            &[
                ("CDO_SLOT_ENTRY_WAIT_S", "8"),
                ("CDO_SLOT_TEST_PAUSE_ENTER_S", "60"),
            ],
        )
        .stdout(log_file(&log))
        .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    s.resume();
    until("the gate waits", &|| {
        log_has("waiting for the CDO timing to end")
    });
    std::thread::sleep(std::time::Duration::from_millis(2500));
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(a.wait().unwrap().code(), Some(0));
    let gid = g.id();
    until("the gate's next decision is held", &|| {
        let stat = std::fs::read_to_string(format!("/proc/{gid}/stat")).unwrap_or_default();
        if stat
            .rsplit_once(") ")
            .is_none_or(|(_, r)| r.starts_with('Z'))
        {
            panic!(
                "the gate ended before its next decision: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::fs::read_dir(s.dir())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("paused."))
    });
    s.wait_paused();
    // B's timing lock is taken at once; its build wait then sees the gate's
    // record until the gate parks again.
    let _b = s.bg(
        "A",
        &["run", "lane", "b", "--timed", "--", "sleep", "600"],
        &[],
    );
    until("timing B begins", &|| {
        s.run("probe", &["status"], &[])
            .1
            .contains("timed by lane since")
    });
    let resumed = std::time::Instant::now();
    s.resume();
    assert_eq!(g.wait().unwrap().code(), Some(1), "the gate under timing B");
    assert!(
        resumed.elapsed() < std::time::Duration::from_millis(6500),
        "the second round waited from zero: {:?}",
        resumed.elapsed()
    );
}

/// ⟨TLS-135 review r1, sol⟩ What the entry check cannot check, it refuses:
/// - A record that cannot be made (here `entries` is a file) is no admission
///   on a host that can hold the slot: exit 1, nothing built.
/// - A timing whose file names an ancestor of the gate, but with another start
///   time (the PID now names another process), is not the gate's own: the
///   gate waits, here refusing after 1 s.
///
/// Mutations (2026-11-26), each red, restored, green: `enter_check` returning 0
/// where its record cannot be made -> the gate builds; `build_entry` going on
/// after an `enter` error -> the gate builds; `foreign_timing`'s start-time
/// comparison dropped -> the gate takes the timing as its own and builds.
/// (Since TLS-135 r13 the record carries `kept=1` and its lock's real key, so
/// the case reaches the binding rule's start-time check in `scan_once`; a
/// record with no key is `unknown` and refuses at once.)
#[test]
fn cdo_slot_gate_entry_refuses_what_it_cannot_check() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    std::fs::create_dir_all(s.dir()).unwrap();
    std::fs::write(s.dir().join("entries"), "").unwrap();
    let out = gate_cmd(&s, &gate, &script, &[], &[]).output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("cannot record this run"), "{text}");
    assert_eq!(cargo_log(), "", "it built without a record: {text}");
    std::fs::remove_file(s.dir().join("entries")).unwrap();

    // A timing of a reused PID: its locks held by stand-ins, its file naming
    // the gate's parent shell with start time 1.
    let d = s.dir().display().to_string();
    let gate_script = gate.join(&script).display().to_string();
    let body = format!(
        "mkdir -p {d}/id {d}/timings; \
         flock -s {d}/timed.lock sleep 600 & l1=$!; flock -x {d}/id/timed.feedfeedfeedfeed sleep 600 & l2=$!; \
         sleep 0.5; \
         key=$(grep -h '^lock:' /proc/$l2/fdinfo/* | awk '{{print $7}}' | head -n 1); \
         printf 'owner=old\\nreason=r\\nsince=2026-11-26T00:00:00Z\\npid=%s\\npid_start=1\\nkept=1\\nlock_key=%s\\n' $$ \"$key\" >{d}/timings/feedfeedfeedfeed; \
         bash '{gate_script}' fmt; rc=$?; pkill -P $l1; pkill -P $l2; kill $l1 $l2; exit $rc"
    );
    let env = gate_cmd(&s, &gate, &script, &[], &[("CDO_SLOT_ENTRY_WAIT_S", "1")]);
    let mut c = scrubbed(std::process::Command::new("bash"));
    c.args(["-c", &body]);
    for (k, v) in env.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    let out = c.output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("refused: a CDO timing still runs"), "{text}");
    assert_eq!(
        cargo_log(),
        "",
        "it took a reused PID's timing as its own: {text}"
    );
}

/// The env of a `gate_cmd` (BASE_ENV, the stub PATH, the slot) plus `env`, on
/// `bash -c <body>`.
fn gate_shell(
    s: &Slot,
    gate: &std::path::Path,
    script: &str,
    body: &str,
    env: &[(&str, &str)],
) -> std::process::Command {
    let base = gate_cmd(s, gate, script, &[], env);
    let mut c = scrubbed(std::process::Command::new("bash"));
    c.args(["-c", body]);
    for (k, v) in base.get_envs() {
        if let Some(v) = v {
            c.env(k, v);
        }
    }
    c
}

/// ⟨TLS-135 review r2, sol⟩ The production run: a real `ci-steps task` (its
/// steps stubbed) is in its `fmt` step when a foreign timing begins. The
/// timing waits for the step (its record), not for the run, which is quiet
/// while its step runs; after the step the run is admitted afresh and waits
/// for the timing (review r3): it pauses between its steps, builds nothing
/// beside the timing, and ends green.
///
/// Mutation (2026-11-26): `build_step` running its step without `quiet on` ->
/// the timing waits for the run while the run's next step waits for the timing:
/// the timing is refused (red); restored, green.
#[test]
fn cdo_slot_a_run_pauses_between_its_steps_for_a_timing() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let repo = t.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    crate::git::run(&repo, &["init", "-q"]);
    let grammar = t.join("grammar");
    std::fs::create_dir_all(grammar.join("src")).unwrap();
    std::fs::write(grammar.join("src/node-types.json"), "[]").unwrap();
    let go = t.join("go").display().to_string();
    let gs = grammar.display().to_string();
    let log = t.join("task.log");
    let mut task = Reap(
        gate_cmd(
            &s,
            &gate,
            &format!("{script} task"),
            &[],
            &[
                ("TLS135_HOLD", go.as_str()),
                ("TREE_SITTER_AL_PATH", gs.as_str()),
            ],
        )
        .current_dir(&repo)
        .stdout(log_file(&log))
        .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap())
        .spawn()
        .unwrap(),
    );
    until("the run's fmt step builds", &|| {
        t.join("fmt-running").exists()
    });
    let mut timing = test_timing(&s, &[("CDO_SLOT_TIMED_WAIT_S", "15")]);
    until("the timing waits for the step", &|| {
        s.run("probe", &["status"], &[])
            .1
            .contains("timed by lane since")
    });
    std::fs::write(&go, "").unwrap();
    let tid = timing.id();
    until("the timing begins between the run's steps", &|| {
        let st = std::fs::read_to_string(format!("/proc/{tid}/stat")).unwrap_or_default();
        if st.rsplit_once(") ").is_none_or(|(_, r)| r.starts_with('Z')) {
            panic!(
                "the timing ended without beginning: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        t.join("on").exists()
    });
    assert_eq!(cargo_log(), "after\n", "a step built beside the timing");
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
    let code = task.wait().unwrap().code();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(code, Some(0), "{text}");
    // No step built while the timing ran (each would log `beside`): the run
    // paused between its steps.
    assert!(!cargo_log().contains("beside"), "{}", cargo_log());
}

/// ⟨TLS-135 review r2, sol and Fable⟩ A timing that scans while a gate writes
/// its record (CDO_SLOT_TEST_WRITE_PAUSE_S holds the write half done) neither
/// removes the record's temporary file (only exact `<pid>.<start>` names are
/// judged) nor lets the gate build beside it: the record is complete before
/// the decision, whatever the order of the lines around the ENTER hook.
///
/// Mutations (2026-11-26), each red, restored, green: the scan judging every
/// name in `entries/` again -> the temporary file is removed, the gate cannot
/// write its record and refuses (exit 1); the decision moved before the record
/// write, the ENTER hook left after both (Fable's mutant) -> the gate builds
/// beside the timing.
#[test]
fn cdo_slot_a_timing_scan_during_a_record_write_neither_breaks_nor_misses_it() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let log = t.join("gate.log");
    let mut g = Reap(
        gate_cmd(
            &s,
            &gate,
            &script,
            &[],
            &[("CDO_SLOT_TEST_WRITE_PAUSE_S", "4")],
        )
        .stdout(log_file(&log))
        .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    let mut timing = test_timing(&s, &[]);
    until("the timing runs", &|| t.join("on").exists());
    until("the gate decided", &|| {
        let l = std::fs::read_to_string(&log).unwrap_or_default();
        l.contains("waiting for the CDO timing")
            || l.contains("refusing")
            || !cargo_log().is_empty()
    });
    assert_eq!(cargo_log(), "", "the gate built beside the timing");
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
    let code = g.wait().unwrap().code();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(cargo_log(), "after\n", "{text}");
}

/// ⟨TLS-135 review r2, sol⟩ The deadline holds when timings hand over between
/// the wait's probe and its decision, round after round: each decision finds a
/// new timing (it began at the ENTER hook), and each ends before the next
/// probe (at the RETRY hook). The gate refuses at its 3 s, not never.
///
/// Mutation (2026-11-26): the deadline check after a decision that found a
/// timing dropped -> the gate never refuses over 12 handovers (red); restored,
/// green.
#[test]
fn cdo_slot_gate_entry_deadline_holds_over_repeated_handovers() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    // The first decision finds timing 0, which then ends; then, each round:
    // ENTER (start timing i), RETRY (end it).
    let start_timing = |i: usize| {
        let tn = t.display().to_string();
        let body = format!("while [ ! -e {tn}/end{i} ]; do sleep 0.05; done");
        let r = s.bg(
            "A",
            &[
                "run",
                "lane",
                &format!("t{i}"),
                "--timed",
                "--",
                "bash",
                "-c",
                &body,
            ],
            &[],
        );
        until("the timing has begun", &|| {
            s.run("probe", &["status"], &[])
                .1
                .contains(&format!("(t{i})"))
        });
        r
    };
    let mut timings = vec![start_timing(0)];
    let mut g = Reap(
        gate_cmd(
            &s,
            &gate,
            &script,
            &[],
            &[
                ("CDO_SLOT_ENTRY_WAIT_S", "3"),
                ("CDO_SLOT_TEST_PAUSE_ENTER_S", "60"),
                ("CDO_SLOT_TEST_PAUSE_RETRY_S", "60"),
            ],
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap(),
    );
    // The first `enter`'s decision: timing 0 runs.
    s.wait_paused();
    s.resume();
    let gid = g.id();
    let gate_alive = || {
        std::fs::read_to_string(format!("/proc/{gid}/stat"))
            .ok()
            .and_then(|st| st.rsplit_once(") ").map(|(_, r)| !r.starts_with('Z')))
            .unwrap_or(false)
    };
    let paused_or_done = || {
        !gate_alive()
            || std::fs::read_dir(s.dir())
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("paused."))
    };
    // Once the gate waits (parked), timing 0 ends: the wait's probe sees none,
    // and the gate decides again.
    until("the gate waits", &|| {
        std::fs::read_dir(s.dir().join("parked"))
            .map(|d| d.flatten().count() > 0)
            .unwrap_or(false)
    });
    std::fs::write(t.join("end0"), "").unwrap();
    assert_eq!(timings[0].wait().unwrap().code(), Some(0));
    let mut ended = false;
    for i in 1..=12 {
        // ENTER: the next decision; timing i begins first.
        until("the gate's next window", &paused_or_done);
        if !gate_alive() {
            ended = true;
            break;
        }
        s.wait_paused();
        timings.push(start_timing(i));
        s.resume();
        // RETRY: the decision found timing i; it ends before the next probe.
        until("the gate's next window", &paused_or_done);
        if !gate_alive() {
            ended = true;
            break;
        }
        s.wait_paused();
        std::fs::write(t.join(format!("end{i}")), "").unwrap();
        assert_eq!(timings[i].wait().unwrap().code(), Some(0));
        s.resume();
    }
    assert!(ended, "the gate never refused over 12 handovers");
    assert_eq!(g.wait().unwrap().code(), Some(1));
    assert_eq!(cargo_log(), "", "it built");
    for (i, _) in timings.iter().enumerate() {
        let _ = std::fs::write(t.join(format!("end{i}")), "");
    }
}

/// ⟨TLS-135 review r3, sol and Fable⟩ A real `ci-steps all` (its steps
/// stubbed) waits for its background build quietly. The order that deadlocked:
/// the build records itself and is held (the ENTER hook, for `ci-steps build`
/// only) while the run's `test` step ends and the run is admitted again and
/// reaches its join; a timing begins (it counts the held build); the build then
/// sees the timing and parks. The timing must begin (the run is quiet at its
/// join), the build then runs after it, and the run ends green.
///
/// Mutation (2026-11-26): the join a plain `wait "$build_pid"` again -> the
/// timing counts the run, which waits for the build, which waits for the
/// timing: the timing is refused (red); restored, green. (r4: with the join
/// marker the same break is red at "the run is quiet at its join".) (r7,
/// Fable) the ending keeper timing not skipped (then `timing_ending`, now
/// `timing_releasing`) -> after its perf-bounds
/// step the run waits for the step's ending timing ("waiting for the CDO
/// timing", red); restored, green.
#[test]
fn cdo_slot_ci_steps_all_waits_for_its_build_quietly() {
    use std::os::unix::fs::PermissionsExt;
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("scripts")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("tests/perf_bounds.rs"),
        "mod release_checks {\n    #[test]\n    fn t() {}\n}\n",
    )
    .unwrap();
    crate::git::run(&repo, &["init", "-q"]);
    let grammar = t.join("grammar");
    std::fs::create_dir_all(grammar.join("src")).unwrap();
    std::fs::write(grammar.join("src/node-types.json"), "[]").unwrap();
    let bin = t.join("perf-bounds-stand-in");
    std::fs::write(
        &bin,
        "#!/bin/bash\nif [ \"$1\" = --list ]; then\n\
         case \" $* \" in *\" --ignored \"*) exit 0 ;; esac\n\
         echo 'release_checks::t: test'; exit 0\nfi\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (gs, bs, td) = (
        grammar.display().to_string(),
        bin.display().to_string(),
        t.join("target").display().to_string(),
    );
    let log = t.join("all.log");
    let go = t.join("go").display().to_string();
    let mut all = Reap(
        gate_cmd(
            &s,
            &gate,
            &format!("{script} all"),
            &[],
            &[
                ("TREE_SITTER_AL_PATH", gs.as_str()),
                ("CI_STEPS_PERF_BOUNDS_BIN", bs.as_str()),
                ("CARGO_TARGET_DIR", td.as_str()),
                ("CDO_SLOT_TEST_PAUSE_ENTER_S", "60"),
                ("CDO_SLOT_TEST_PAUSE_ENTER_NAME", "ci-steps build"),
                ("TLS135_HOLD", go.as_str()),
                ("TLS135_HOLD_CMD", "test"),
            ],
        )
        .current_dir(&repo)
        .stdout(log_file(&log))
        .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap())
        .spawn()
        .unwrap(),
    );
    // The build is held after its record. The run's test step is held at its
    // last cargo call; then it ends, the run is admitted afresh (its record is
    // written again: a new file) and goes quiet at its join (review r4: a
    // marker, not a sleep).
    s.wait_paused();
    until("the run's test step runs its last cargo call", &|| {
        t.join("test-running").exists()
    });
    let pid = all.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split(' ')
        .nth(19)
        .unwrap()
        .to_string();
    let name = format!("{pid}.{start}");
    let record = s.dir().join("entries").join(&name);
    let ino = |p: &std::path::Path| {
        std::fs::metadata(p)
            .map(|m| std::os::unix::fs::MetadataExt::ino(&m))
            .ok()
    };
    let before = ino(&record);
    assert!(before.is_some(), "precondition: the run is recorded");
    std::fs::write(t.join("go"), "").unwrap();
    until("the run is admitted afresh after its test step", &|| {
        ino(&record).is_some_and(|i| Some(i) != before)
    });
    until("the run is quiet at its join", &|| {
        s.dir().join("quiet").join(&name).exists()
    });
    let mut timing = test_timing(&s, &[("CDO_SLOT_TIMED_WAIT_S", "10")]);
    until("the timing has its lock", &|| {
        s.run("probe", &["status"], &[])
            .1
            .contains("timed by lane since")
    });
    s.resume();
    let tid = timing.id();
    until(
        "the timing begins while the run waits for its build",
        &|| {
            let st = std::fs::read_to_string(format!("/proc/{tid}/stat")).unwrap_or_default();
            if st.rsplit_once(") ").is_none_or(|(_, r)| r.starts_with('Z')) {
                panic!(
                    "the timing ended without beginning: {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            t.join("on").exists()
        },
    );
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
    // The build's next decision passes the ENTER hook again: let it through.
    s.resume();
    let code = all.wait().unwrap().code();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(code, Some(0), "{text}");
    assert!(!cargo_log().contains("beside"), "{}", cargo_log());
    // Review r7 (Fable): after its perf-bounds step the run does not wait for
    // the step's own ending timing (its keeper lets go within 2 s).
    let after = text
        .split("=== ci-steps: perf-bounds ===")
        .nth(1)
        .unwrap_or("");
    // No wait at all, not even one that ends at once ("the CDO timing ended").
    assert!(
        !after.contains("waiting for the CDO timing") && !after.contains("the CDO timing ended"),
        "the run waited for its perf-bounds step's ending timing: {after}"
    );
}

/// ⟨TLS-135 review r3, sol⟩ After a quiet step a run is admitted afresh before
/// it works on in its own shell: a timing may have skipped it while it was
/// quiet. A run whose step ends under such a timing waits (here: refuses after
/// 1 s) and builds nothing.
///
/// Mutation (2026-11-26): the fresh admission dropped from `build_step` -> the
/// run's own cargo runs beside the timing (red); restored, green.
#[test]
fn cdo_slot_a_run_is_admitted_afresh_after_a_quiet_step() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    let td = t.display().to_string();
    let entry_lib = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/build-entry");
    let body = format!(
        "source '{entry_lib}'; build_entry run || exit 9; \
         build_step bash -c 'touch {td}/stepping; until [ -e {td}/go ]; do sleep 0.05; done'; \
         cargo fmt; echo done >{td}/worked-on"
    );
    let mut run = Reap(
        gate_shell(&s, &gate, &script, &body, &[("CDO_SLOT_ENTRY_WAIT_S", "1")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the run's step runs", &|| t.join("stepping").exists());
    let mut timing = test_timing(&s, &[]);
    until("the timing begins beside the quiet run", &|| {
        t.join("on").exists()
    });
    std::fs::write(t.join("go"), "").unwrap();
    assert_eq!(run.wait().unwrap().code(), Some(1));
    assert!(!t.join("worked-on").exists(), "the run worked on");
    assert_eq!(cargo_log(), "", "the run built beside the timing");
    std::fs::write(t.join("end"), "").unwrap();
    assert_eq!(timing.wait().unwrap().code(), Some(0));
}

/// ⟨TLS-135 review r3, Fable⟩ `build_step` leaves the quiet state after its
/// step: a run alive after a step is counted by a timing again. And a run whose
/// quiet state cannot be removed stops (exit 1) rather than work on.
///
/// Mutations (2026-11-26), each red, restored, green: `quiet off` not run (its
/// result taken as success) -> the run stays skipped, the timing begins (exit
/// 0); the `exit 1` after a failed `quiet off` dropped -> the run works on.
#[test]
fn cdo_slot_build_step_leaves_the_quiet_state() {
    use std::os::unix::fs::PermissionsExt;
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let entry_lib = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/build-entry");
    let body = format!(
        "source '{entry_lib}'; build_entry run || exit 9; build_step true; \
         touch {td}/ready; sleep 600"
    );
    let run = Reap(
        gate_shell(&s, &gate, &script, &body, &[])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    until("the run is past its step", &|| t.join("ready").exists());
    let (code, text) = s.run(
        "A",
        &["run", "lane", "timing", "--timed", "--", "true"],
        &[("CDO_SLOT_TIMED_WAIT_S", "1")],
    );
    assert_eq!(code, Some(6), "the run stayed quiet after its step: {text}");
    assert!(text.contains("(entry)"), "{text}");
    drop(run);

    // `quiet off` cannot remove the state (its folder is read-only).
    let body = format!(
        "source '{entry_lib}'; build_entry run2 || exit 9; \
         build_step bash -c 'chmod a-w \"$CDO_SLOT_DIR/quiet\"'; echo on >{td}/worked-on"
    );
    let out = gate_shell(&s, &gate, &script, &body, &[]).output().unwrap();
    let quiet = s.dir().join("quiet");
    std::fs::set_permissions(&quiet, std::fs::Permissions::from_mode(0o755)).unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("could not leave the quiet state"), "{text}");
    assert!(!t.join("worked-on").exists(), "the run worked on: {text}");
}

/// ⟨TLS-135 review r4, sol⟩ A run that cannot go quiet (here `quiet` is a file,
/// so its folder cannot be made) does not run its step: counted while it waits
/// for the step, it could wait on the timing that waits for it. It stops with
/// exit 1 before the step.
///
/// Mutation (2026-11-26): a failed `quiet on` only said, the step run anyway
/// (r3's behaviour) -> the step runs (red); restored, green.
#[test]
fn cdo_slot_build_step_does_not_run_a_step_it_cannot_wait_for_quietly() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    std::fs::create_dir_all(s.dir()).unwrap();
    std::fs::write(s.dir().join("quiet"), "").unwrap();
    let entry_lib = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/build-entry");
    let body = format!(
        "source '{entry_lib}'; build_entry run || exit 9; build_step touch {td}/stepped; exit 0"
    );
    let out = gate_shell(&s, &gate, &script, &body, &[]).output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("could not go quiet"), "{text}");
    assert!(!t.join("stepped").exists(), "the step ran: {text}");
}

/// A stand-in `perf_bounds` binary for `ci-steps perf-bounds`
/// (CI_STEPS_PERF_BOUNDS_BIN): it lists as many timing tests as
/// tests/perf_bounds.rs declares, and its run sleeps 1 s.
fn perf_bounds_stand_in(s: &Slot) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/perf_bounds.rs"))
        .unwrap();
    let declared = src
        .split_once("\nmod release_checks")
        .unwrap()
        .1
        .lines()
        .filter(|l| l.trim() == "#[test]")
        .count();
    let bin = s.0.path().join("perf-bounds-stand-in");
    std::fs::write(
        &bin,
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = --list ]; then\n\
             case \" $* \" in *\" --ignored \"*) exit 0 ;; esac\n\
             for ((i = 0; i < {declared}; i++)); do echo \"release_checks::t$i: test\"; done; exit 0\n\
             fi\nsleep 1\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// ⟨TLS-135 review r4, r6, r7⟩ Two real `ci-steps perf-bounds` (a stand-in
/// test binary) sharing one hold (its token): each counted the other's
/// `ci-steps` as a build and both were refused after CDO_SLOT_TIMED_WAIT_S.
/// Now one times, and the other is refused at once ("another timing runs in
/// this slot"). Forced (review r7, Fable): the first is held in the timing gate
/// after its check (the TIMED_GATE hook) while the second waits at the gate;
/// only then does the first publish.
///
/// Mutations (2026-11-26), each red, restored, green: the gate not passed
/// (r6: `entry_turn` not called) -> both refused after 20 s, (1, 1); (r7) the
/// check made before the gate is taken -> both checked before either
/// published (red); (r7) `timing_gate` not called -> the hold in the gate is
/// never reached (red).
#[test]
fn cdo_slot_two_perf_bounds_in_one_hold_one_is_refused() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let bin = perf_bounds_stand_in(&s);
    let holder = sleeper();
    let hpid = holder.id().to_string();
    let token = ("CDO_SLOT_TOKEN", "tls135-shared");
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "landing", "--bind", &hpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");
    let bin_s = bin.display().to_string();
    let lane = |i: usize| {
        Reap(
            gate_cmd(
                &s,
                &gate,
                &format!("{script} perf-bounds"),
                &[],
                &[
                    ("CI_STEPS_PERF_BOUNDS_BIN", bin_s.as_str()),
                    token,
                    ("CDO_SLOT_BUILD_SCRIPTS", script.as_str()),
                    ("CDO_SLOT_TIMED_WAIT_S", "20"),
                    ("CDO_SLOT_TEST_PAUSE_TIMED_GATE_S", "60"),
                ],
            )
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(log_file(&t.join(format!("pb{i}.log"))))
            .stderr(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(t.join(format!("pb{i}.log")))
                    .unwrap(),
            )
            .spawn()
            .unwrap(),
        )
    };
    let mut a = lane(1);
    let mut b = lane(2);
    // One is held in the gate after its check; the other has the gate open
    // (it waits for it). Both acquires carry the reason in their argv.
    let gate_lock = s.dir().join("timing-gate.lock");
    let at_gate = || {
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok())
            .filter(|e| {
                let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
                String::from_utf8_lossy(&cmd).contains("perf-bounds timing")
                    && std::fs::read_dir(e.path().join("fd")).is_ok_and(|d| {
                        d.flatten()
                            .any(|f| std::fs::read_link(f.path()).is_ok_and(|l| l == gate_lock))
                    })
            })
            .count()
    };
    s.wait_paused();
    until("the other waits at the gate", &|| at_gate() >= 2);
    s.resume();
    // A second hold of the hook (only if the check came before the gate: the
    // mutant) is let through too, so the test ends either way.
    let offered = std::time::Instant::now();
    while offered.elapsed().as_secs() < 3 {
        let found = std::fs::read_dir(s.dir())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("paused."));
        if found {
            s.wait_paused();
            s.resume();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (ca, cb) = (a.wait().unwrap().code(), b.wait().unwrap().code());
    let logs = format!(
        "{}\n---\n{}",
        std::fs::read_to_string(t.join("pb1.log")).unwrap_or_default(),
        std::fs::read_to_string(t.join("pb2.log")).unwrap_or_default()
    );
    // Waiting on each other ends in two refusals (exit 1) at the wait: the
    // exits and the prompt refusal's message say which happened.
    let mut codes = [ca, cb];
    codes.sort();
    assert_eq!(codes, [Some(0), Some(1)], "{logs}");
    assert!(
        logs.contains("refused: another timing runs in this slot"),
        "{logs}"
    );
    drop(holder);
}

/// ⟨TLS-135 review r5, sol and Fable⟩ A `ci-steps perf-bounds` run under its
/// own outer timing (`cdo-slot run ... --timed -- ci-steps perf-bounds`) does
/// not wait for that timing to end: it waits for this command. It shares the
/// hold and times as before.
///
/// Mutation (2026-11-26): the start's check (then `entry_turn`, now the timing
/// gate's) waiting for any timing (`timed_now`, as in r4) -> it waits for the
/// outer timing and is refused after its 20 s (red);
/// restored, green. (r6) its first check by `timed_now` -> refused at once,
/// exit 1 (red); restored, green.
#[test]
fn cdo_slot_perf_bounds_under_its_own_timed_run_does_not_wait_for_it() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let bin = perf_bounds_stand_in(&s);
    let bin_s = bin.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let out = s
        .cmd(
            "A",
            &[
                "run",
                "lane",
                "outer",
                "--timed",
                "--",
                "bash",
                &gate_script,
                "perf-bounds",
            ],
            &[
                ("PATH", &path),
                ("CI_STEPS_PERF_BOUNDS_BIN", &bin_s),
                ("CDO_SLOT_TIMED_WAIT_S", "20"),
            ],
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("ci-steps: perf-bounds timing:"), "{text}");
}

/// ⟨TLS-135 review r6, sol⟩ sol's interleaving: in one hold (its token), a
/// foreign `ci-steps perf-bounds` F is past its acquire (the ACQUIRED hook holds
/// it) when an outer timed `run` begins; F then meets the outer timing; only
/// after that does the outer command start its own `perf-bounds` N. Under r5, F
/// waited for the outer timing keeping the turn, N needed the turn before it
/// could see the outer timing was its own, and the outer timing waited for N:
/// N was refused after its wait. Now F is refused at once and keeps nothing;
/// N times under its own outer timing and the run ends green. And (r7, sol) F,
/// refused, ends before the outer timing does: no fresh admission parks it
/// behind the timing that refused it.
///
/// Mutations (2026-11-26), each red, restored, green: r5's `scripts/cdo-slot`
/// (cd829e9a1: F waits keeping the turn) -> N is refused, the outer run exits 1;
/// (r7) a refused perf-bounds re-admitted (`build_quiet_end` for
/// `build_quiet_stop`) -> F waits behind the outer timing past 15 s.
#[test]
fn cdo_slot_a_foreign_entry_timing_keeps_no_turn_from_a_nested_one() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let bin = perf_bounds_stand_in(&s);
    let bin_s = bin.display().to_string();
    let token = ("CDO_SLOT_TOKEN", "tls135-nested");
    let holder = sleeper();
    let hpid = holder.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "landing", "--bind", &hpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");
    let flog = t.join("foreign.log");
    let mut foreign = Reap(
        gate_cmd(
            &s,
            &gate,
            &format!("{script} perf-bounds"),
            &[],
            &[
                ("CI_STEPS_PERF_BOUNDS_BIN", bin_s.as_str()),
                token,
                ("CDO_SLOT_TEST_PAUSE_ACQUIRED_S", "60"),
                ("CDO_SLOT_TIMED_WAIT_S", "20"),
                ("CDO_SLOT_ENTRY_WAIT_S", "60"),
            ],
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(log_file(&flog))
        .stderr(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&flog)
                .unwrap(),
        )
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    // The outer timed run; its command waits for `go`, then runs N.
    let gate_script = gate.join(&script).display().to_string();
    let body = format!(
        "touch {td}/outer-on; until [ -e {td}/go ]; do sleep 0.05; done; bash '{gate_script}' perf-bounds"
    );
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let olog = t.join("outer.log");
    let mut outer = Reap(
        s.cmd(
            "L",
            &["run", "lane", "outer", "--timed", "--", "bash", "-c", &body],
            &[
                token,
                ("PATH", &path),
                ("CI_STEPS_PERF_BOUNDS_BIN", &bin_s),
                ("CDO_SLOT_TIMED_WAIT_S", "20"),
            ],
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(log_file(&olog))
        .stderr(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&olog)
                .unwrap(),
        )
        .spawn()
        .unwrap(),
    );
    until("the outer timing runs", &|| t.join("outer-on").exists());
    // F goes on and meets the outer timing: it is refused.
    s.resume();
    until("F met the outer timing", &|| {
        std::fs::read_to_string(&flog)
            .unwrap_or_default()
            .contains("another timing runs in this slot")
    });
    // Review r7 (sol): F, refused, ends at once: before the outer timing ends
    // (no fresh admission that parks behind it).
    let fid = foreign.id();
    let live = || {
        std::fs::read_to_string(format!("/proc/{fid}/stat"))
            .ok()
            .and_then(|st| st.rsplit_once(") ").map(|(_, r)| !r.starts_with('Z')))
            .unwrap_or(false)
    };
    let refused = std::time::Instant::now();
    while live() && refused.elapsed() < std::time::Duration::from_secs(15) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        !live(),
        "the refused perf-bounds waited behind the outer timing"
    );
    std::fs::write(t.join("go"), "").unwrap();
    let code = outer.wait().unwrap().code();
    let text = std::fs::read_to_string(&olog).unwrap_or_default();
    assert_eq!(code, Some(0), "the nested perf-bounds: {text}");
    assert!(text.contains("ci-steps: perf-bounds timing:"), "{text}");
    assert_eq!(foreign.wait().unwrap().code(), Some(1));
    let f = std::fs::read_to_string(&flog).unwrap_or_default();
    assert!(
        f.contains("refused: another timing runs in this slot"),
        "{f}"
    );
    drop(holder);
}

/// ⟨TLS-135 review r7, sol⟩ Every timed start passes one gate: a real `ci-steps
/// perf-bounds` and a plain `cdo-slot run --timed` in one hold (its token), in
/// both orders, each held at the publication point (the TIMED_GATE hook) while
/// the other starts. The first one's timing runs; the second is refused at once
/// ("another timing runs in this slot"), never published beside it.
///
/// Mutations (2026-11-26, on r7's rule, each red, restored, green): the check
/// ignoring a running perf-bounds timing when the new one is not an entry ->
/// in order 1 the run publishes beside the perf-bounds; the check ignoring a
/// running plain timing when the new one is an entry -> in order 2 the
/// perf-bounds times beside the run. (r8: one rule for all, any foreign timing
/// refuses; the gate's check dropped -> order 1 red.)
#[test]
fn cdo_slot_every_timed_start_passes_one_gate_in_both_orders() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let bin = perf_bounds_stand_in(&s);
    let bin_s = bin.display().to_string();
    let token = ("CDO_SLOT_TOKEN", "tls135-gate");
    let holder = sleeper();
    let hpid = holder.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "landing", "--bind", &hpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");
    let perf_bounds = |log: &std::path::Path, hook: &str, secs: &str| {
        Reap(
            gate_cmd(
                &s,
                &gate,
                &format!("{script} perf-bounds"),
                &[],
                &[
                    ("CI_STEPS_PERF_BOUNDS_BIN", bin_s.as_str()),
                    token,
                    (hook, secs),
                ],
            )
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(log_file(log))
            .stderr(std::fs::OpenOptions::new().append(true).open(log).unwrap())
            .spawn()
            .unwrap(),
        )
    };
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
    // A process whose command line holds `marker` has the gate open (it waits
    // for it: the other one holds it, paused).
    let gate_lock = s.dir().join("timing-gate.lock");
    let at_gate = |marker: &str| {
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok())
            .any(|e| {
                let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
                String::from_utf8_lossy(&cmd).contains(marker)
                    && std::fs::read_dir(e.path().join("fd")).is_ok_and(|d| {
                        d.flatten()
                            .any(|f| std::fs::read_link(f.path()).is_ok_and(|l| l == gate_lock))
                    })
            })
    };

    // Order 1: the perf-bounds is at its publication when the run starts.
    let plog = t.join("pb1.log");
    let mut pb = perf_bounds(&plog, "CDO_SLOT_TEST_PAUSE_TIMED_GATE_S", "60");
    s.wait_paused();
    let run = s
        .cmd(
            "L",
            &["run", "lane", "plain", "--timed", "--", "true"],
            &[token],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The run waits for the gate; the perf-bounds then publishes.
    until("the run waits at the gate", &|| at_gate("plain"));
    s.resume();
    let out = run.wait_with_output().unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(6),
        "the run beside the perf-bounds: {text}\nperf-bounds: {}",
        read(&plog)
    );
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    assert_eq!(pb.wait().unwrap().code(), Some(0), "{}", read(&plog));

    // Order 2: the run is at its publication when the perf-bounds starts.
    let td = t.display().to_string();
    let body = format!("touch {td}/run-on; sleep 8");
    let mut run = Reap(
        s.cmd(
            "L",
            &[
                "run", "lane", "plain2", "--timed", "--", "bash", "-c", &body,
            ],
            &[token, ("CDO_SLOT_TEST_PAUSE_TIMED_GATE_S", "60")],
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap(),
    );
    s.wait_paused();
    let plog = t.join("pb2.log");
    let mut pb = perf_bounds(&plog, "CDO_SLOT_TEST_PAUSE_TIMED_GATE_S", "");
    // The perf-bounds is admitted (nothing is published yet) and waits for the
    // gate; then the run publishes.
    until("the perf-bounds waits at the gate", &|| {
        at_gate("perf-bounds timing")
    });
    s.resume();
    let code = pb.wait().unwrap().code();
    let text = read(&plog);
    assert_eq!(code, Some(1), "the perf-bounds beside the run: {text}");
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    // Checked once the run has ended (review r8, sol: the refusal may come
    // before the run's drain or command).
    assert_eq!(run.wait().unwrap().code(), Some(0));
    assert!(
        t.join("run-on").exists(),
        "precondition: the run's timing began"
    );

    // Order 3 (review r7, Fable's cycle): the perf-bounds already has its
    // timing (drained, held before its go) when the run starts.
    let plog = t.join("pb3.log");
    let mut pb = perf_bounds(&plog, "CDO_SLOT_TEST_PAUSE_GO_S", "60");
    s.wait_paused();
    let out = s
        .cmd(
            "L",
            &["run", "lane", "plain3", "--timed", "--", "true"],
            &[token],
        )
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(6),
        "the run beside the perf-bounds' timing: {text}"
    );
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    s.resume();
    assert_eq!(pb.wait().unwrap().code(), Some(0), "{}", read(&plog));
    drop(holder);
}

/// ⟨TLS-135 review r7, Fable⟩ The timing gate covers the check and the
/// publication only, never the drain: a timing whose drain waits for a build
/// does not keep a second timing out of the gate. The second gets the gate at
/// once and is refused there at once (r8: one timing at a time), long before
/// the first's drain ends.
///
/// Mutation (2026-11-26): the gate closed at the end of `timed_start` (after
/// the drain) instead of right after the publication -> the second timing gets
/// exit 5, "the timing gate ... stayed held" (red; on r7's rule); restored,
/// green.
#[test]
fn cdo_slot_the_timing_gate_is_not_held_across_a_drain() {
    let s = Slot::new();
    let fake_name = format!("t135d{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let names = ("CDO_SLOT_BUILD_NAMES", fake_name.as_str());
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let build = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let t1 = sleeper();
    let t1pid = t1.id().to_string();
    // The first timing drains, waiting for the build.
    let first = s.bg(
        "A",
        &["acquire", "lane", "first", "--bind", &t1pid, "--timed"],
        &[names, ("CDO_SLOT_TIMED_WAIT_S", "30")],
    );
    until("the first timing is published", &|| {
        s.run("probe", &["status"], &[]).1.contains("(first)")
    });
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let asked = std::time::Instant::now();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[
            names,
            ("CDO_SLOT_LOCK_WAIT_S", "2"),
            ("CDO_SLOT_TIMED_WAIT_S", "3"),
        ],
    );
    assert_eq!(code, Some(6), "{text}");
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(20),
        "the gate was held across the first timing's drain: {:?}",
        asked.elapsed()
    );
    drop(build);
    drop(first);
    drop(t1);
    drop(t2);
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r8, sol⟩ Two sibling `cdo-slot run --timed` in one hold
/// (its token), each of whose command runs a gate (`ci-steps fmt`): under r7
/// both timings were allowed, each gate parked behind the other's timing, and
/// each timing waited for its own command. One timing at a time: the second
/// run is refused at once, and the first's gate runs under its own timing.
///
/// Mutation (2026-11-27): the timing gate's check dropped -> both timings run
/// and the second sibling is not refused (exit 0, red); restored, green.
#[test]
fn cdo_slot_two_sibling_timed_runs_never_wait_on_each_other() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let token = ("CDO_SLOT_TOKEN", "tls135-siblings");
    let holder = sleeper();
    let hpid = holder.id().to_string();
    let (code, text) = s.run(
        "L",
        &["acquire", "land", "landing", "--bind", &hpid],
        &[token],
    );
    assert_eq!(code, Some(0), "{text}");
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    // Each command: say it runs, wait for `go`, run its gate, keep its result.
    let sibling = |i: usize| {
        let body = format!(
            "touch {td}/on{i}; until [ -e {td}/go ]; do sleep 0.05; done; \
             bash '{gate_script}' fmt; echo $? >{td}/rc{i}"
        );
        let log = t.join(format!("sib{i}.log"));
        Reap(
            s.cmd(
                "L",
                &[
                    "run",
                    "lane",
                    &format!("sib{i}"),
                    "--timed",
                    "--",
                    "bash",
                    "-c",
                    &body,
                ],
                &[token, ("PATH", &path), ("CDO_SLOT_ENTRY_WAIT_S", "3")],
            )
            .stdout(log_file(&log))
            .stderr(std::fs::OpenOptions::new().append(true).open(&log).unwrap())
            .spawn()
            .unwrap(),
        )
    };
    let mut a = sibling(1);
    until("the first sibling's timing runs", &|| {
        t.join("on1").exists()
    });
    let mut b = sibling(2);
    // Refused at once; given 10 s (a second timing that ran instead waits for
    // `go`, which comes after this, so the test ends either way).
    let asked = std::time::Instant::now();
    while b.0.try_wait().unwrap().is_none() && asked.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::fs::write(t.join("go"), "").unwrap();
    let code_b = b.wait().unwrap().code();
    let text_b = std::fs::read_to_string(t.join("sib2.log")).unwrap_or_default();
    let code_a = a.wait().unwrap().code();
    let rc1 = std::fs::read_to_string(t.join("rc1")).unwrap_or_default();
    assert_eq!(code_b, Some(6), "the second sibling's timing: {text_b}");
    assert!(
        text_b.contains("refused: another timing runs in this slot"),
        "{text_b}"
    );
    assert_eq!(
        rc1.trim(),
        "0",
        "the first sibling's gate under its own timing"
    );
    assert_eq!(code_a, Some(0));
    drop(holder);
}

/// ⟨TLS-135 review r8, sol⟩ A `run --timed` whose wrapper is killed while its
/// command lives: the command keeps the timing (it inherited the locks), and
/// a gate it then starts owns that timing (it has the timing's id lock file
/// open, inherited: review r10, sol), so it runs instead of waiting for
/// itself.
///
/// Mutation (2026-11-27, r8): the `cmd_pid` own-line test dropped from
/// `timing_scan` -> the gate waits for its own timing and refuses after 2 s
/// (red); restored, green. (r10) the open-file owner dropped from
/// `timing_scan` -> the same (red); restored, green.
#[test]
fn cdo_slot_a_timed_runs_command_outlives_its_killed_wrapper() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let body = format!(
        "touch {td}/on; until [ -e {td}/go ]; do sleep 0.05; done; \
         bash '{gate_script}' fmt; echo $? >{td}/rc"
    );
    let mut wrapper = s.bg(
        "A",
        &["run", "lane", "outer", "--timed", "--", "bash", "-c", &body],
        &[("PATH", &path), ("CDO_SLOT_ENTRY_WAIT_S", "2")],
    );
    until("the timed command runs", &|| t.join("on").exists());
    wrapper.0.kill().unwrap();
    wrapper.0.wait().unwrap();
    assert!(
        s.probe().ends_with("timed=1"),
        "precondition: the command keeps the timing"
    );
    std::fs::write(t.join("go"), "").unwrap();
    until("the command's gate ended", &|| t.join("rc").exists());
    let rc = std::fs::read_to_string(t.join("rc")).unwrap_or_default();
    assert_eq!(rc.trim(), "0", "the gate waited for its own timing");
}

/// ⟨TLS-135 review r8, sol⟩ A keeper never has the timing gate, not even in its
/// first moment (the KEEPER hook holds it there, before it closes its caller's
/// descriptors): one stopped there kept every later timed start out.
///
/// Mutation (2026-11-27, r8): the gate's close dropped from spawn_keeper's
/// launch -> the held keeper has `timing-gate.lock` open (red); restored,
/// green. (r10: the keeper now starts after timed_start_now closed the gate)
/// that close moved after the keeper's start -> the same (red); restored,
/// green.
#[test]
fn cdo_slot_a_keeper_is_launched_without_the_timing_gate() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut timed = s.bg(
        "A",
        &["acquire", "lane", "keeper", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_KEEPER_S", "60")],
    );
    let paused = || -> Option<u32> {
        std::fs::read_dir(s.dir()).ok()?.flatten().find_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("paused.")?
                .parse()
                .ok()
        })
    };
    until("the keeper is held in its first moment", &|| {
        paused().is_some()
    });
    let k = paused().unwrap();
    let gate_lock = s.dir().join("timing-gate.lock");
    let has_gate = std::fs::read_dir(format!("/proc/{k}/fd"))
        .unwrap()
        .flatten()
        .any(|f| std::fs::read_link(f.path()).is_ok_and(|l| l == gate_lock));
    s.wait_paused();
    s.resume();
    assert!(
        !has_gate,
        "the keeper was launched with the timing gate open"
    );
    assert_eq!(timed.0.wait().unwrap().code(), Some(4));
    drop(tm);
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r10, sol⟩ No keeper publishes a timing: its acquire takes
/// and publishes it under the timing gate, and the keeper only inherits it.
/// Here the acquire is killed while its keeper is held in its first moment:
/// the timing stays on (the keeper holds it), a timed start after the kill is
/// refused, and once the keeper sees its acquire gone the timing ends. At no
/// time do two timings run. (Before, the keeper published after its pause,
/// beside a timing taken meanwhile.)
///
/// Mutation (2026-11-27): the acquire closing its copies before the keeper's
/// start (the keeper inherits nothing) -> no timing runs after the kill
/// (red); restored, green.
#[test]
fn cdo_slot_a_killed_acquires_keeper_never_publishes_late() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut killed = s.bg(
        "A",
        &["acquire", "lane", "killed", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_KEEPER_S", "60")],
    );
    s.wait_paused();
    killed.0.kill().unwrap();
    killed.0.wait().unwrap();
    assert!(
        s.probe().ends_with("timed=1"),
        "the keeper holds the timing its acquire published"
    );
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "later", "--bind", &t2pid, "--timed"],
        &[],
    );
    assert_eq!(
        code,
        Some(6),
        "a timing beside the killed acquire's: {text}"
    );
    s.resume();
    until("the keeper ends the timing", &|| {
        s.probe() == "held=1 own=0 timed=0"
    });
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "after", "--bind", &t2pid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    drop(t2);
    drop(tm);
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r10, sol⟩ A retry by the process a timing is bound to is
/// answered at once, before it waits for any hold: with a fresh hold of its
/// own (bound, no token to share it) it waited for ever for a hold that ends
/// only with it. Both a fresh hold and a shared one: exit 4, "already timed
/// ... a duplicate request", no second timing, the first still on.
///
/// Mutation (2026-11-27): the `already_owned` check dropped from `acquire`
/// -> the retry in the fresh hold waits (killed after 10 s, red); restored,
/// green.
#[test]
fn cdo_slot_a_retry_of_its_own_timing_never_waits() {
    let s = Slot::new();
    let slot = std::path::Path::new(SLOT);
    // A fresh hold, bound and unshared.
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let (code, text) = s.run(
        "B",
        &["acquire", "lane", "first", "--bind", &tpid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    let (code, text, in_time, _) = run_bounded(
        &s,
        slot,
        &[
            "acquire", "lane", "retry", "--wait", "--bind", &tpid, "--timed",
        ],
        std::time::Duration::from_secs(10),
    );
    assert!(in_time, "the retry waited for its own hold: {text}");
    assert_eq!(code, Some(4), "{text}");
    assert!(text.contains("a duplicate request"), "{text}");
    assert!(s.probe().ends_with("timed=1"), "the first timing ended");
    drop(tm);
    until("free", &|| s.probe() == "held=0 own=0 timed=0");

    // A shared hold (this session's, by hand).
    assert_eq!(s.run("B", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let (code, text) = s.run(
        "B",
        &["acquire", "lane", "first", "--bind", &t2pid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    let (code, text, in_time, _) = run_bounded(
        &s,
        slot,
        &[
            "acquire", "lane", "retry", "--wait", "--bind", &t2pid, "--timed",
        ],
        std::time::Duration::from_secs(10),
    );
    assert!(in_time, "{text}");
    assert_eq!(code, Some(4), "{text}");
    assert!(text.contains("a duplicate request"), "{text}");
    drop(t2);
    until("no timing", &|| s.probe().ends_with("timed=0"));
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r10, sol⟩ A worker left behind by a timed `run` (the run
/// and its command have ended) owns the timing it inherited: a `ci-steps`
/// it starts itself runs at once, instead of waiting for a lock it holds.
///
/// Mutation (2026-11-27): the open-file owner dropped from `timing_scan` ->
/// the worker's gate waits for its own timing and refuses after 2 s (red);
/// restored, green. (r13, its slot on btrfs: `Slot::on_repo_fs`) the lock key
/// taken from `stat` instead of the kernel's lock line, its self-check
/// skipped -> the same (red; the nested test too, while a test on `/tmp`
/// stayed green); restored, green.
#[test]
fn cdo_slot_an_orphan_worker_owns_its_timing() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let worker = format!(
        "( until [ -e {td}/go ]; do sleep 0.05; done; \
         bash '{gate_script}' fmt >{td}/fmt.log 2>&1; echo $? >{td}/rc ) \
         </dev/null >/dev/null 2>&1 & exit 0"
    );
    let (code, text) = s.run(
        "A",
        &[
            "run", "lane", "worker", "--timed", "--", "bash", "-c", &worker,
        ],
        &[("PATH", &path), ("CDO_SLOT_ENTRY_WAIT_S", "2")],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        s.probe().ends_with("timed=1"),
        "precondition: the worker keeps the timing"
    );
    std::fs::write(t.join("go"), "").unwrap();
    until("the worker's gate ended", &|| t.join("rc").exists());
    let rc = std::fs::read_to_string(t.join("rc")).unwrap_or_default();
    let fmt = std::fs::read_to_string(t.join("fmt.log")).unwrap_or_default();
    assert_eq!(
        rc.trim(),
        "0",
        "the worker waited for its own timing: {fmt}"
    );
}

/// The stdout and stderr of an ended (or killed and reaped) child spawned
/// with both piped.
fn piped_text(c: &mut std::process::Child) -> String {
    use std::io::Read;
    let mut text = String::new();
    if let Some(mut o) = c.stdout.take() {
        let _ = o.read_to_string(&mut text);
    }
    if let Some(mut e) = c.stderr.take() {
        let _ = e.read_to_string(&mut text);
    }
    text
}

/// [`Slot::cmd`] spawned with stdout and stderr piped.
fn spawn_piped(s: &Slot, session: &str, args: &[&str], env: &[(&str, &str)]) -> Reap {
    Reap(
        s.cmd(session, args, env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

/// Wait up to `secs` for `c` to end (killed and reaped after); (exit, ended in
/// time, its output).
fn end_within(c: &mut Reap, secs: u64) -> (Option<i32>, bool, String) {
    let asked = std::time::Instant::now();
    while c.0.try_wait().unwrap().is_none() && asked.elapsed().as_secs() < secs {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let ended = c.0.try_wait().unwrap();
    if ended.is_none() {
        let _ = c.0.kill();
    }
    let st = c.0.wait().unwrap();
    (st.code(), ended.is_some(), piped_text(&mut c.0))
}

/// ⟨TLS-135 review r11, r12, sol⟩ A timing is provisional until its acquire
/// finished its drain and handoff (its commit point). A retry by the same
/// bound process meanwhile is refused at once ("still starting", exit 6): it
/// is not told "already timed" (the first may still fail, or drain for a
/// build beside it), and it does not wait (a wait could cycle with that
/// drain). Once the first is final, the retry is a duplicate (exit 4).
///
/// Mutation (2026-11-27): every live timing read as final (`scan_once` not
/// asking `commit_final`) -> the retry is told "already timed" (exit 4, red);
/// restored, green.
#[test]
fn cdo_slot_a_retry_of_a_starting_timing_is_refused_at_once() {
    let s = Slot::new();
    let slot = std::path::Path::new(SLOT);
    assert_eq!(s.run("B", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut first = s.bg(
        "B",
        &["acquire", "lane", "first", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_GRACE_S", "60")],
    );
    s.wait_paused();
    let (code, text, in_time, _) = run_bounded(
        &s,
        slot,
        &[
            "acquire", "lane", "retry", "--wait", "--bind", &tpid, "--timed",
        ],
        std::time::Duration::from_secs(10),
    );
    assert!(in_time, "the retry waited: {text}");
    assert_eq!(code, Some(6), "{text}");
    assert!(text.contains("still starting"), "{text}");
    s.resume();
    assert_eq!(first.0.wait().unwrap().code(), Some(4));
    let (code, text) = s.run(
        "B",
        &["acquire", "lane", "retry", "--bind", &tpid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    assert!(text.contains("a duplicate request"), "{text}");
    drop(tm);
    until("no timing", &|| s.probe().ends_with("timed=0"));
    assert_eq!(s.run("B", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r11, sol⟩ A retry queued BEFORE its first acquire
/// published (both bound to one process, no token: the first's fresh hold is
/// not the retry's) ends promptly once the first publishes: `acquire` asks
/// request_verdict at every poll, so it never waits for a hold that ends only
/// with its own process.
///
/// Mutation (2026-11-27): the per-poll request_verdict dropped from
/// `acquire`'s loop -> the retry waits for the first's hold (killed after
/// 15 s, red); restored, green.
#[test]
fn cdo_slot_a_concurrent_retry_ends_once_its_timing_runs() {
    let s = Slot::new();
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut first = s.bg(
        "B",
        &["acquire", "lane", "first", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_ACQUIRED_S", "60")],
    );
    s.wait_paused();
    let queue = s.dir().join("queue");
    let mut retry = spawn_piped(
        &s,
        "B",
        &[
            "acquire", "lane", "retry", "--wait", "--bind", &tpid, "--timed",
        ],
        &[("CDO_SLOT_POLL_S", "1")],
    );
    until("the retry queues behind the first's hold", &|| {
        std::fs::read_dir(&queue)
            .map(|d| d.flatten().count())
            .unwrap_or(0)
            == 1
    });
    s.resume();
    assert_eq!(first.0.wait().unwrap().code(), Some(0));
    let (code, in_time, out) = end_within(&mut retry, 15);
    assert!(in_time, "the retry waited for its own hold: {out}");
    assert!(
        code == Some(4) || code == Some(6),
        "the retry: {code:?} {out}"
    );
    drop(tm);
    until("free", &|| s.probe() == "held=0 own=0 timed=0");
}

/// ⟨TLS-135 review r11, r12, sol⟩ Only a descriptor that carries the flock
/// holds a timing: under a timed `run`, a `dup` of the id lock descriptor,
/// with the original closed, still holds it (the command's gate runs at
/// once); an independent open of the same file by an unrelated process holds
/// nothing (its gate waits for the timing; here: refuses after 1 s). Fork
/// inheritance: `cdo_slot_an_orphan_worker_owns_its_timing`.
///
/// Mutation (2026-11-27): holds_lock matching any descriptor on the file's
/// inode (`ino:`), lock or not -> the independent open's gate runs (rc 0,
/// red); restored, green.
#[test]
fn cdo_slot_only_a_descriptor_with_the_lock_holds_a_timing() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let body = format!(
        "tid=$(ls \"$CDO_SLOT_DIR/timings\"); id=$(readlink -f \"$CDO_SLOT_DIR/id/timed.$tid\"); \
         for f in /proc/$$/fd/*; do [ \"$(readlink \"$f\")\" = \"$id\" ] && n=${{f##*/}}; done; \
         eval \"exec 9>&$n $n>&-\"; \
         bash '{gate_script}' fmt >{td}/dup.log 2>&1; echo $? >{td}/duprc; \
         echo \"$id\" >{td}/id; until [ -e {td}/done ]; do sleep 0.05; done"
    );
    let mut run = s.bg(
        "A",
        &["run", "lane", "dup", "--timed", "--", "bash", "-c", &body],
        &[("PATH", &path)],
    );
    until("the dup's gate ran", &|| t.join("id").exists());
    let duprc = std::fs::read_to_string(t.join("duprc")).unwrap_or_default();
    let dup = std::fs::read_to_string(t.join("dup.log")).unwrap_or_default();
    assert_eq!(duprc.trim(), "0", "a dup of the lock descriptor: {dup}");
    let id = std::fs::read_to_string(t.join("id")).unwrap();
    let open = format!("exec 8<'{}'; bash '{gate_script}' fmt", id.trim());
    let out = gate_shell(&s, &gate, &script, &open, &[("CDO_SLOT_ENTRY_WAIT_S", "1")])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "an independent open held the timing: {text}"
    );
    std::fs::write(t.join("done"), "").unwrap();
    run.0.wait().unwrap();
}

/// ⟨TLS-135 review r11, r12, sol⟩ A process whose ANCESTOR holds a timing
/// (here: the exact bound process of a keeper's timing) is related to it but
/// cannot reuse it: it would outlive it. Its timed start is refused at once,
/// before it queues (with no token it would wait for its parent's own hold,
/// which ends only with that parent: a cycle while the parent waits for it).
///
/// Mutation (2026-11-27): request_verdict treating `ancestor` as unrelated
/// -> the child queues behind its parent's hold (no answer in 10 s, red);
/// restored, green.
#[test]
fn cdo_slot_a_child_of_a_timings_process_cannot_reuse_it() {
    let s = Slot::new();
    let t = s.0.path();
    let td = t.display().to_string();
    let parent = format!(
        "until [ -e {td}/go ]; do sleep 0.05; done; \
         bash -c 'exec bash \"$0\" acquire lane nested --wait --bind $$ --timed' '{SLOT}' \
         >{td}/child.log 2>&1; echo $? >{td}/rc; sleep 600"
    );
    let env_of = s.cmd("B", &["status"], &[("CDO_SLOT_POLL_S", "1")]);
    let mut p = scrubbed(std::process::Command::new("bash"));
    p.args(["-c", &parent]);
    for (k, v) in env_of.get_envs() {
        if let Some(v) = v {
            p.env(k, v);
        }
    }
    let p = Reap(
        p.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ppid = p.0.id().to_string();
    let (code, text) = s.run(
        "B",
        &["acquire", "lane", "outer", "--bind", &ppid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    std::fs::write(t.join("go"), "").unwrap();
    let asked = std::time::Instant::now();
    while !t.join("rc").exists() && asked.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let log = std::fs::read_to_string(t.join("child.log")).unwrap_or_default();
    let rc = std::fs::read_to_string(t.join("rc")).unwrap_or_default();
    assert_eq!(rc.trim(), "6", "the child's timed start: {log}");
    assert!(log.contains("cannot reuse"), "{log}");
    drop(p);
    until("free", &|| s.probe() == "held=0 own=0 timed=0");
}

/// ⟨TLS-135 review r11, sol⟩ A refused timing's record stays while its lock is
/// still held (here a child of the refusing `run` keeps the descriptors: the
/// REFUSE_HOLDER hook): `status` still names it, and it goes once the lock
/// is free. A record removed under a held lock hid a live timing from every
/// scan.
///
/// Mutation (2026-11-27): forget_timed removing the record without its lock
/// probe -> `timed=1` with no record (red); restored, green.
#[test]
fn cdo_slot_a_refused_timing_keeps_its_record_while_held() {
    let s = Slot::new();
    let fake_name = format!("t135r{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let _build = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let (code, text) = s.run(
        "A",
        &["run", "lane", "refused", "--timed", "--", "true"],
        &[
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_str()),
            ("CDO_SLOT_TIMED_WAIT_S", "0"),
            ("CDO_SLOT_TEST_REFUSE_HOLDER", "30"),
        ],
    );
    assert_eq!(code, Some(6), "{text}");
    assert!(
        s.probe().ends_with("timed=1"),
        "precondition: the lock is held"
    );
    let (_, status) = s.run("B", &["status"], &[]);
    assert!(
        status.contains("timed by lane"),
        "the held timing lost its record: {status}"
    );
    let holder = std::fs::read_to_string(s.dir().join("holder.pid")).unwrap();
    std::process::Command::new("kill")
        .arg(holder.trim())
        .status()
        .unwrap();
    until("the timing ends", &|| s.probe().ends_with("timed=0"));
}

/// ⟨TLS-135 review r11, sol; Fable⟩ The process to time is checked again right
/// before the publication, under the gate (its start time from --bind's one
/// read), and on every round of the drain: one that ended meanwhile is
/// refused, never timed. Two windows: ended while the gate was held (its own
/// refusal line), and ended while the drain waited for a build.
///
/// Mutations (2026-11-27): the recheck after the gate's pause dropped -> the
/// drain's line instead of the gate's (red); the drain's per-round check
/// dropped -> the acquire waits for the build (no answer in 15 s, red); each
/// restored, green.
#[test]
fn cdo_slot_a_timed_process_that_ends_is_never_timed() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    // Ended in the gate's window.
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut gated = spawn_piped(
        &s,
        "A",
        &["acquire", "lane", "gated", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_TIMED_GATE_S", "60")],
    );
    s.wait_paused();
    drop(tm);
    s.resume();
    let (code, in_time, text) = end_within(&mut gated, 30);
    assert!(in_time, "{text}");
    assert_eq!(code, Some(6), "{text}");
    assert!(text.contains("the process to bind (PID"), "{text}");
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");

    // Ended while the drain waited for a build.
    let fake_name = format!("t135d{}", std::process::id());
    let fake = s.0.path().join(&fake_name);
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    let _build = stand_in(&s, &fake, &["-c".as_ref(), "sleep 600; true".as_ref()]);
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut drained = spawn_piped(
        &s,
        "A",
        &["acquire", "lane", "drained", "--bind", &tpid, "--timed"],
        &[
            ("CDO_SLOT_BUILD_NAMES", fake_name.as_str()),
            ("CDO_SLOT_POLL_S", "1"),
        ],
    );
    until("the timing drains", &|| s.probe().ends_with("timed=1"));
    drop(tm);
    let (code, in_time, text) = end_within(&mut drained, 15);
    assert!(in_time, "the drain went on for a dead process: {text}");
    assert_eq!(code, Some(6), "{text}");
    assert!(text.contains("the process it was to time"), "{text}");
    until("no timing", &|| s.probe() == "held=1 own=0 timed=0");
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r12, sol⟩ ONE commit point for a timed start's hold and
/// timing. Its keepers take their go, then keep only once the acquire wrote
/// its commit mark: an acquire that dies before it leaves nothing held or
/// timed (each keeper reads the mark again after it saw the death); one that
/// dies after it leaves both kept, to end with the bound process; a commit
/// mark that cannot be written ends both (exit 5).
///
/// Mutations (2026-11-27): the keepers keeping without the mark (their
/// `exit 0` on no commit dropped) -> after the death before the commit the
/// slot stays held (red); the commit write's failure ignored -> exit 0 and
/// held (red); each restored, green.
#[test]
fn cdo_slot_one_commit_point_for_the_hold_and_the_timing() {
    let s = Slot::new();
    // The acquire dies before its commit.
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let mut a = s.bg(
        "B",
        &["acquire", "lane", "before", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_COMMIT_S", "60")],
    );
    s.wait_paused();
    assert_eq!(s.probe(), "held=1 own=0 timed=1", "precondition");
    a.0.kill().unwrap();
    a.0.wait().unwrap();
    until("both keepers abort", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });

    // The acquire dies after its commit.
    let mut a = s.bg(
        "B",
        &["acquire", "lane", "after", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_COMMITTED_S", "60")],
    );
    s.wait_paused();
    a.0.kill().unwrap();
    a.0.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert_eq!(
        s.probe(),
        "held=1 own=0 timed=1",
        "the committed start was dropped"
    );
    drop(tm);
    until("they end with the process", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });

    // The commit mark cannot be written.
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let (code, text) = s.run(
        "B",
        &["acquire", "lane", "failed", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_COMMIT_FAIL", "1")],
    );
    assert_eq!(code, Some(5), "{text}");
    assert!(text.contains("could not write its commit mark"), "{text}");
    until("nothing held or timed", &|| {
        s.probe() == "held=0 own=0 timed=0"
    });
    drop(tm);
}

/// ⟨TLS-135 review r12, sol⟩ The commit point commits only what still stands:
/// the hold released, or the timing's keeper killed, in the window right
/// before the mark -> the start is refused (exit 6, saying which), and
/// nothing is left timed.
///
/// Mutation (2026-11-27): commit_standing always true -> the start reports
/// success (exit 4) for a hold that ended (red); restored, green.
#[test]
fn cdo_slot_a_commit_checks_what_it_commits() {
    let s = Slot::new();
    let tm = sleeper();
    let tpid = tm.id().to_string();
    // The hold released in the window.
    assert_eq!(s.run("C", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let mut a = spawn_piped(
        &s,
        "C",
        &["acquire", "lane", "released", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_COMMIT_S", "60")],
    );
    s.wait_paused();
    assert_eq!(s.run("C", &["release"], &[]).0, Some(0));
    s.resume();
    let (code, in_time, text) = end_within(&mut a, 30);
    assert!(in_time, "{text}");
    assert_eq!(code, Some(6), "{text}");
    assert!(text.contains("its hold ended"), "{text}");
    until("nothing timed", &|| s.probe().ends_with("timed=0"));

    // The timing's keeper killed in the window.
    assert_eq!(s.run("C", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let mut a = spawn_piped(
        &s,
        "C",
        &["acquire", "lane", "killed", "--bind", &tpid, "--timed"],
        &[("CDO_SLOT_TEST_PAUSE_COMMIT_S", "60")],
    );
    s.wait_paused();
    let k = keepers(&s, b"_keep\0timed\0");
    assert_eq!(k.len(), 1, "precondition: one timing keeper");
    std::process::Command::new("kill")
        .args(["-9", &k[0].to_string()])
        .status()
        .unwrap();
    until("its lock is free", &|| s.probe().ends_with("timed=0"));
    s.resume();
    let (code, in_time, text) = end_within(&mut a, 30);
    assert!(in_time, "{text}");
    assert_eq!(code, Some(6), "{text}");
    assert!(text.contains("its timing ended"), "{text}");
    assert_eq!(s.probe(), "held=1 own=0 timed=0");
    assert_eq!(s.run("C", &["release"], &[]).0, Some(0));
    drop(tm);
}

/// ⟨TLS-135 review r12, sol⟩ The id locks decide whether a timing runs, not the
/// aggregate `timed.lock`: a timed `run` whose command closes its copy of
/// `timed.lock`, leaves a worker with the id lock only and exits keeps the
/// timing live (`probe` says timed=1; a foreign gate refuses after its wait;
/// a second timing is refused).
///
/// Mutation (2026-11-27): timed_now asking `timed.lock` alone -> probe says
/// timed=0 while the id lock is held (red); restored, green.
#[test]
fn cdo_slot_a_worker_with_only_the_id_lock_keeps_the_timing() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let body = "tl=$(readlink -f \"$CDO_SLOT_DIR/timed.lock\"); \
        for f in /proc/$$/fd/*; do [ \"$(readlink \"$f\")\" = \"$tl\" ] && eval \"exec ${f##*/}>&-\"; done; \
        sleep 600 </dev/null >/dev/null 2>&1 & exit 0";
    let (code, text) = s.run(
        "A",
        &["run", "lane", "worker", "--timed", "--", "bash", "-c", body],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        s.probe().ends_with("timed=1"),
        "the id lock alone did not keep the timing"
    );
    let out = gate_cmd(&s, &gate, &script, &[], &[("CDO_SLOT_ENTRY_WAIT_S", "1")])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "a gate beside it: {text}");
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(6), "a second timing beside it: {text}");
    drop(t2);
    // The slot's cleanup ends the worker.
}

/// ⟨TLS-135 review r12, sol⟩ holds_lock reads each descriptor on its own: one
/// it cannot read (the FDINFO hook marks descriptor 0 so, everywhere) does not
/// hide a later one that carries the lock (the timed command's own gate runs),
/// and leaves a "no" incomplete, which is unknown (an unrelated gate refuses at
/// once, "cannot tell").
///
/// Mutations (2026-11-27): an unread descriptor ending the read at once as
/// unknown -> the timed command's own gate refuses (red); the incomplete "no"
/// taken as a plain "no" -> the unrelated gate parks and refuses after its
/// wait instead (red); each restored, green.
#[test]
fn cdo_slot_an_unread_descriptor_keeps_a_match_and_makes_a_no_unknown() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let body = format!(
        "CDO_SLOT_TEST_FDINFO_UNREADABLE=0 bash '{gate_script}' fmt >{td}/own.log 2>&1; \
         echo $? >{td}/ownrc; until [ -e {td}/done ]; do sleep 0.05; done"
    );
    let mut run = s.bg(
        "A",
        &["run", "lane", "own", "--timed", "--", "bash", "-c", &body],
        &[("PATH", &path)],
    );
    until("the command's gate ran", &|| t.join("ownrc").exists());
    let ownrc = std::fs::read_to_string(t.join("ownrc")).unwrap_or_default();
    let own = std::fs::read_to_string(t.join("own.log")).unwrap_or_default();
    assert_eq!(ownrc.trim(), "0", "its own gate: {own}");
    let wrapped = format!("CDO_SLOT_TEST_FDINFO_UNREADABLE=0 bash '{gate_script}' fmt");
    let out = gate_shell(
        &s,
        &gate,
        &script,
        &wrapped,
        &[("CDO_SLOT_ENTRY_WAIT_S", "1")],
    )
    .output()
    .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("cannot tell"), "{text}");
    std::fs::write(t.join("done"), "").unwrap();
    run.0.wait().unwrap();
}

/// ⟨TLS-135 review r12, sol⟩ A build entry whose line has a process of our
/// user it cannot read, started after the timing's taker (it could have
/// inherited the lock: the UNREADABLE hook marks the gate's wrapper shell so),
/// cannot tell whether the running timing is its own, and refuses at once
/// ("cannot tell"), rather than taking it for foreign or own. One started
/// before the taker (here: the test process) is skipped: it cannot hold it.
///
/// Mutations (2026-11-27): `unknown` read as `none` -> the wrapper's gate
/// parks and refuses after its wait instead (red); the started-before-taker
/// skip dropped -> the gate under the unreadable test process refuses with
/// "cannot tell" instead of parking (red); each restored, green.
#[test]
fn cdo_slot_an_unreadable_line_refuses_its_entry() {
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "timing", "--bind", &tpid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(4), "{text}");
    let gate_script = gate.join(&script).display().to_string();
    let wrapped = format!("CDO_SLOT_TEST_UNREADABLE_PID=$$ bash '{gate_script}' fmt");
    let out = gate_shell(
        &s,
        &gate,
        &script,
        &wrapped,
        &[("CDO_SLOT_ENTRY_WAIT_S", "1")],
    )
    .output()
    .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("cannot tell"), "{text}");
    let me = std::process::id().to_string();
    let out = gate_cmd(
        &s,
        &gate,
        &script,
        &[],
        &[
            ("CDO_SLOT_ENTRY_WAIT_S", "1"),
            ("CDO_SLOT_TEST_UNREADABLE_PID", me.as_str()),
        ],
    )
    .output()
    .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("still r") && !text.contains("cannot tell"),
        "an older unreadable process was taken as a possible holder: {text}"
    );
    drop(tm);
    until("no timing", &|| s.probe().ends_with("timed=0"));
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r9, sol⟩ A timing is live while its id lock is held,
/// whatever its PIDs: a timed `run` whose command backgrounds a worker and
/// exits leaves the worker holding the timing (it inherited the locks) after
/// the run and its command are gone. A gate then waits for it (here: refuses
/// after 1 s) and a second timing is refused; neither goes on beside it.
///
/// Mutation (2026-11-27): r8's ending rule put back (a `run`'s timing skipped
/// once its run and command are dead) -> the gate is admitted and builds
/// beside the live timing (red); restored, green.
#[test]
fn cdo_slot_a_timing_is_live_while_its_lock_is_held() {
    let s = Slot::new();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let cargo_log = || std::fs::read_to_string(t.join("cargo-log")).unwrap_or_default();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let (code, text) = s.run(
        "A",
        &[
            "run",
            "lane",
            "worker",
            "--timed",
            "--",
            "bash",
            "-c",
            "sleep 600 </dev/null >/dev/null 2>&1 & exit 0",
        ],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        s.probe().ends_with("timed=1"),
        "precondition: the worker keeps the timing"
    );
    let out = gate_cmd(&s, &gate, &script, &[], &[("CDO_SLOT_ENTRY_WAIT_S", "1")])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the gate beside the live timing: {text}"
    );
    assert_eq!(cargo_log(), "", "it built beside the live timing: {text}");
    let t2 = sleeper();
    let t2pid = t2.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "second", "--bind", &t2pid, "--timed"],
        &[],
    );
    assert_eq!(code, Some(6), "a second timing beside the live one: {text}");
    drop(t2);
    // The slot's cleanup ends the worker (its environment names the folder).
}

/// ⟨TLS-135 review r9, sol⟩ A new timing's own line is that of the process it
/// will be bound to, not of its launcher: a command under timing A that starts
/// `acquire --timed --bind PID` for a process outside A's line is refused at
/// once (that timing would be foreign to A).
///
/// Mutation (2026-11-27): the gate judging the own line from the launcher
/// (`foreign_timing` with no root) -> the acquire passes as A's own and
/// publishes a second timing (exit 4, red); restored, green.
#[test]
fn cdo_slot_a_timing_is_judged_by_the_process_it_binds() {
    let s = Slot::new();
    let t = s.0.path();
    let td = t.display().to_string();
    let other = sleeper();
    let opid = other.id();
    let body = format!("bash '{SLOT}' acquire lane bound --bind {opid} --timed; echo $? >{td}/rc");
    let (code, text) = s.run(
        "A",
        &["run", "lane", "outer", "--timed", "--", "bash", "-c", &body],
        &[],
    );
    assert_eq!(code, Some(0), "{text}");
    let rc = std::fs::read_to_string(t.join("rc")).unwrap_or_default();
    assert_eq!(
        rc.trim(),
        "6",
        "a timing bound outside the line passed: {text}"
    );
    assert!(
        text.contains("refused: another timing runs in this slot"),
        "{text}"
    );
    drop(other);
}

/// ⟨TLS-135 review r9, sol⟩ A keeper that never says it started (here it
/// stops itself right after it has the timing: since r10 it inherits the
/// acquire's) is killed and reaped before the acquire goes on: a stopped one
/// kept its locks, and TERM does not wake it. The acquire drops its own copy,
/// refuses (exit 5) and nothing is left timed.
///
/// Mutation (2026-11-27): the timeout sending TERM and returning (r8) -> the
/// stopped keeper keeps the timing ("timed=1" after the refusal, red);
/// restored, green.
#[test]
fn cdo_slot_a_keeper_that_never_starts_is_ended() {
    let s = Slot::new();
    assert_eq!(s.run("A", &["acquire", "lane", "by hand"], &[]).0, Some(0));
    let tm = sleeper();
    let tpid = tm.id().to_string();
    let (code, text) = s.run(
        "A",
        &["acquire", "lane", "stopped", "--bind", &tpid, "--timed"],
        &[
            ("CDO_SLOT_TEST_KEEPER_STOP", "1"),
            ("CDO_SLOT_TEST_SPAWN_STEPS", "40"),
        ],
    );
    assert_eq!(code, Some(5), "{text}");
    assert!(text.contains("its keeper did not start"), "{text}");
    assert_eq!(
        s.probe(),
        "held=1 own=0 timed=0",
        "the stopped keeper kept its timing"
    );
    drop(tm);
    assert_eq!(s.run("A", &["release"], &[]).0, Some(0));
}

/// ⟨TLS-135 review r9 Fable, r10 sol⟩ Under one timed `run`, a nested
/// perf-bounds owns the run's timing (it inherited the id lock file): it is
/// told "already timed" and times under that timing, taking no second one,
/// and a sibling step started meanwhile owns it too and runs (before r10 the
/// nested perf-bounds took a timing of its own, and the sibling was refused).
///
/// Mutation (2026-11-27): the open-file owner dropped from `timing_scan` ->
/// the nested perf-bounds is refused (another timing runs) and times nothing
/// (red); restored, green.
#[test]
fn cdo_slot_a_nested_timed_start_runs_under_its_runs_timing() {
    use std::os::unix::fs::PermissionsExt;
    // On the repository's filesystem: the lock key there is not `stat`'s
    // (review r12, Fable).
    let s = Slot::on_repo_fs();
    let (gate, script) = gate_folder(&s);
    let t = s.0.path();
    let td = t.display().to_string();
    // A perf-bounds stand-in that times 5 s.
    let bin1 = perf_bounds_stand_in(&s);
    let body = std::fs::read_to_string(&bin1)
        .unwrap()
        .replace("sleep 1", "sleep 5");
    let bin = t.join("perf-bounds-5s");
    std::fs::write(&bin, body).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin_s = bin.display().to_string();
    let path = format!(
        "{}:{}",
        gate.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let gate_script = gate.join(&script).display().to_string();
    let inner = format!(
        "{{ bash '{gate_script}' perf-bounds >{td}/pb.log 2>&1; echo $? >{td}/pbrc; }} & \
         timeout 20 bash -c 'until grep -qs \"already timed\" {td}/pb.log; do sleep 0.05; done'; \
         bash '{gate_script}' fmt >{td}/fmt.log 2>&1; echo $? >{td}/rc; wait"
    );
    let out = s
        .cmd(
            "A",
            &[
                "run", "lane", "outer", "--timed", "--", "bash", "-c", &inner,
            ],
            &[
                ("PATH", &path),
                ("CI_STEPS_PERF_BOUNDS_BIN", &bin_s),
                ("CDO_SLOT_ENTRY_WAIT_S", "30"),
            ],
        )
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let fmt = std::fs::read_to_string(t.join("fmt.log")).unwrap_or_default();
    let rc = std::fs::read_to_string(t.join("rc")).unwrap_or_default();
    let pb = std::fs::read_to_string(t.join("pb.log")).unwrap_or_default();
    let pbrc = std::fs::read_to_string(t.join("pbrc")).unwrap_or_default();
    assert!(pb.contains("already timed"), "{pb} {text}");
    assert_eq!(pbrc.trim(), "0", "{pb}");
    assert!(
        pb.contains("ci-steps: perf-bounds timing:"),
        "the nested perf-bounds timed: {pb}"
    );
    assert_eq!(rc.trim(), "0", "the sibling step: {fmt} {text}");
    let second = std::fs::read_dir(s.dir().join("timings"))
        .unwrap()
        .flatten()
        .filter(|e| {
            std::fs::read_to_string(e.path()).is_ok_and(|c| c.contains("perf-bounds timing"))
        })
        .count();
    assert_eq!(second, 0, "a second timing was taken");
}
