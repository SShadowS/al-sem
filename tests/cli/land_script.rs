//! ⟨TLS-59⟩ `scripts/land`: build a landing branch, gate it, push only on
//! request. Every test runs the script on a repository of its own, with stub
//! `scripts/ci-steps`, `scripts/cdo-gate` and `scripts/merge-and-push` that log
//! their calls (and, for ci, run a hook), so nothing real is gated, merged or
//! pushed.
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
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(LAND).arg("--worktree").arg(&self.gate).args(args);
        for v in al_sem::git_env::LOCAL_ENV_VARS {
            cmd.env_remove(v);
        }
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("LAND_LOGS", self.root.join("logs"))
            .env("FLOW_LOG", self.root.join("flow.jsonl"))
            .env("LAND_MEM_MIN_GB", "0")
            // A CDO slot of its own (TLS-60), held for this session.
            .env("CDO_SLOT_DIR", self.root.join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CDO_SLOT_POLL_S", "1")
            .env("CLAUDE_CODE_SESSION_ID", "land-test")
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
            .env("CDO_WS", self.root.join("cdo-ws"))
            .env("STUB_LOG", self.root.join("calls"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run scripts/land");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
    /// `scripts/cdo-slot <args>` on this repo's slot, as Claude session
    /// `session` (no Claude PID: never stale); (exit, stdout + stderr).
    fn slot(&self, session: &str, args: &[&str]) -> (Option<i32>, String) {
        let out = std::process::Command::new("bash")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/cdo-slot"))
            .args(args)
            .env("CDO_SLOT_DIR", self.root.join("slot"))
            .env("CDO_SLOT_MEM_MIN_GB", "0")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .env_remove("CLAUDE_PID")
            .env_remove("CDO_SLOT_TOKEN")
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
            std::fs::write(self.main.join(path), bytes).unwrap();
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
/// refuses that session's `cargo test`; the cdo-gate land runs shares the hold
/// (its acquire answers "already yours", exit 4); after the run the slot is
/// free again.
///
/// Mutations: land without its acquire -> the other session sees `free` and
/// its `cargo test` passes the hook; land without the token export -> the
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
         \"tool_input\":{{\"command\":\"cargo test\"}}}}' \
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
        std::process::Command::new("bash")
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
        for v in al_sem::git_env::LOCAL_ENV_VARS {
            cmd.env_remove(v);
        }
        let out = cmd
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
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
