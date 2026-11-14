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

impl Repo {
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
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
            .env("CDO_WS", "/cdo-ws")
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
    let (code, text) = r.land(&["--push", "tools/tls-58-x"], &[("STUB_CDO_GATE_RC", "1")]);
    assert_eq!(code, Some(1), "{text}");
    assert_eq!(
        r.calls(),
        ["ci-steps all", "cdo-gate /cdo-ws"],
        "a red cdo-gate never pushes"
    );

    r.reset_calls();
    let (code, text) = r.land(
        &["--push", "tools/tls-59-x"],
        &[("STUB_MERGE_AND_PUSH_RC", "1")],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("FAIL at push"), "{text}");

    r.reset_calls();
    let (code, text) = r.land(&["--push", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        r.calls(),
        [
            "ci-steps all",
            "cdo-gate /cdo-ws",
            "merge-and-push landing/tls59-2"
        ]
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
    let lock =
        PathBuf::from(r.git(&r.gate, &["rev-parse", "--absolute-git-dir"])).join("land.lock");
    let held = std::fs::File::create(&lock).unwrap();
    use std::os::fd::AsRawFd;
    // SAFETY: flock(2) on a file this test opened and keeps open.
    assert_eq!(
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("another scripts/land run holds"), "{text}");
    assert!(r.calls().is_empty() && r.landing_branches().is_empty());
    drop(held);
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
    let (code, text) = r.land(&["tools/tls-58-x"], &[("LAND_MEM_MIN_GB", "3.5")]);
    assert_eq!(code, Some(2), "{text}");

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
