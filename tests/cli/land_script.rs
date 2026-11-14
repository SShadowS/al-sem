//! ⟨TLS-59⟩ `scripts/land`: build a landing branch, gate it, push only on
//! request. Every test runs the script on a repository of its own, with stub
//! `scripts/ci-steps`, `scripts/cdo-gate` and `scripts/merge-and-push` that log
//! their calls, so nothing real is gated, merged or pushed.
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
    fn refs(&self) -> String {
        let out = git::run(
            &self.main,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn landing_branches(&self) -> Vec<String> {
        let out = git::run(
            &self.main,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/landing/",
            ],
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
    /// `scripts/land --worktree <gate> <args>` with the stubs' exits set by
    /// `stub_rc` (`[("CI_STEPS", 1)]`); (exit, stdout + stderr).
    fn land(&self, args: &[&str], stub_rc: &[(&str, i32)]) -> (Option<i32>, String) {
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
        for (stub, rc) in stub_rc {
            cmd.env(format!("STUB_{stub}_RC"), rc.to_string());
        }
        let out = cmd.output().expect("run scripts/land");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

/// A master with the three stub scripts, a CHANGELOG and `a.txt`; branches
/// `tools/tls-58-x` and `tools/tls-59-x` each add a CHANGELOG entry at the
/// same place (a CHANGELOG-only conflict between them) and a file of their
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
        write(
            &path,
            &format!(
                "#!/bin/sh\necho \"{stub} $*\" >> \"$STUB_LOG\"\nexit ${{STUB_{var}_RC:-0}}\n"
            ),
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let changelog =
        |entries: &str| format!("# Changelog\n\n## [Unreleased]\n\n{entries}### Old\n\n- old\n");
    write(&main.join("CHANGELOG.md"), &changelog(""));
    write(&main.join("a.txt"), "base\n");
    git::run(&main, &["add", "-A"]);
    git::run(&main, &["commit", "-qm", "base"]);
    for t in ["58", "59"] {
        git::run(
            &main,
            &["switch", "-qc", &format!("tools/tls-{t}-x"), "master"],
        );
        write(
            &main.join("CHANGELOG.md"),
            &changelog(&format!("### Added (TLS-{t})\n\n- entry {t}\n\n")),
        );
        write(&main.join(format!("f{t}.txt")), t);
        git::run(&main, &["add", "-A"]);
        git::run(&main, &["commit", "-qm", &format!("tls {t}")]);
    }
    git::run(&main, &["switch", "-qc", "tools/tls-60-bad", "master"]);
    write(&main.join("a.txt"), "branch\n");
    git::run(&main, &["commit", "-qam", "bad"]);
    git::run(&main, &["switch", "-q", "master"]);
    write(&main.join("a.txt"), "master\n");
    git::run(&main, &["commit", "-qam", "master moves"]);
    git::run(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            gate.to_str().unwrap(),
            "master",
        ],
    );
    Repo {
        _dir: dir,
        root,
        main,
        gate,
    }
}

/// A batch with a CHANGELOG-only conflict merges (both entries kept, a blank
/// line before each heading, no marker), runs ci then cdo-gate in that
/// order, and pushes nothing without `--push`. A red ci never reaches
/// cdo-gate (the incident behind the task); a red cdo-gate never pushes, even
/// with `--push`; a PASS with `--push` calls merge-and-push once. A taken
/// landing name gets `-2`. Every step leaves a start and an end line in the
/// timing log.
///
/// Mutations: ci's stop on a failure dropped -> the red-ci run calls
/// cdo-gate; `if ((!push))` made always false -> the plain run pushes.
#[test]
fn land_gates_in_order_and_pushes_only_on_request() {
    let r = repo();
    let (code, text) = r.land(&["--dry-run", "tools/tls-58-x", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("TLS-59: CHANGELOG.md conflicts: would union it"),
        "{text}"
    );
    assert!(
        r.calls().is_empty(),
        "a dry run gates nothing: {:?}",
        r.calls()
    );

    let (code, text) = r.land(&["tools/tls-58-x", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("land: PASS: landing/tls58+tls59 is gated"),
        "{text}"
    );
    assert_eq!(r.calls(), ["ci-steps all", "cdo-gate /cdo-ws"]);
    assert_eq!(r.landing_branches(), ["landing/tls58+tls59"]);
    let log = git::run(&r.gate, &["log", "--format=%s", "-2"]);
    assert_eq!(
        String::from_utf8(log.stdout).unwrap(),
        "landing: TLS-59\nlanding: TLS-58\n"
    );
    let changelog = std::fs::read_to_string(r.gate.join("CHANGELOG.md")).unwrap();
    assert_eq!(
        changelog,
        "# Changelog\n\n## [Unreleased]\n\n### Added (TLS-58)\n\n- entry 58\n\n\
         ### Added (TLS-59)\n\n- entry 59\n\n### Old\n\n- old\n"
    );

    r.reset_calls();
    let (code, text) = r.land(&["tools/tls-58-x", "tools/tls-59-x"], &[("CI_STEPS", 1)]);
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
    let (code, text) = r.land(&["--push", "tools/tls-58-x"], &[("CDO_GATE", 1)]);
    assert_eq!(code, Some(1), "{text}");
    assert_eq!(
        r.calls(),
        ["ci-steps all", "cdo-gate /cdo-ws"],
        "a red cdo-gate never pushes"
    );

    r.reset_calls();
    let (code, text) = r.land(&["--push", "tools/tls-59-x"], &[]);
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        r.calls(),
        [
            "ci-steps all",
            "cdo-gate /cdo-ws",
            "merge-and-push landing/tls59"
        ]
    );

    let flow = std::fs::read_to_string(r.root.join("flow.jsonl")).unwrap();
    let mut open = std::collections::BTreeMap::new();
    for line in flow.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect(line);
        assert_eq!(v["step"], "land", "{line}");
        let key = format!("{} {}", v["run_id"], v["phase"]);
        match v["event"].as_str() {
            Some("start") => assert!(open.insert(key, ()).is_none(), "{line}"),
            Some("end") => assert!(v["rc"].is_i64() && open.remove(&key).is_some(), "{line}"),
            _ => panic!("{line}"),
        }
    }
    assert!(open.is_empty(), "a step with no end: {open:?}");
}

/// Every refusal comes before anything changes. A conflict outside
/// CHANGELOG.md: the dry run predicts it (exit 1) and moves no ref; the real
/// run stops with the conflicting path, makes no landing branch, leaves the
/// worktree clean and gates nothing. Then: master as a branch, an unknown
/// branch, a branch given twice, a dirty worktree, a master behind
/// origin/master, a branch already in master (exit 1), and `--push` with
/// `--dry-run` (exit 2).
///
/// Mutation: create `landing/<name>` before the merges -> the refused run
/// leaves a branch.
#[test]
fn land_refuses_before_it_changes_anything() {
    let r = repo();
    let before = r.refs();
    let (code, text) = r.land(&["--dry-run", "tools/tls-60-bad"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("TLS-60: REFUSE: conflicts in a.txt"),
        "{text}"
    );
    assert_eq!(r.refs(), before, "a dry run moves no ref");

    let (code, text) = r.land(&["tools/tls-60-bad"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("conflicts in: a.txt; no landing branch was made"),
        "{text}"
    );
    assert!(
        r.landing_branches().is_empty(),
        "{:?}",
        r.landing_branches()
    );
    let status = git::run(&r.gate, &["status", "--porcelain"]);
    assert!(status.stdout.is_empty(), "{status:?}");
    assert!(r.calls().is_empty(), "{:?}", r.calls());

    for (args, says) in [
        (&["master"][..], "master is not a branch to land"),
        (&["nope"], "no local branch 'nope'"),
        (&["tools/tls-58-x", "tools/tls-58-x"], "given twice"),
    ] {
        let (code, text) = r.land(args, &[]);
        assert_eq!(code, Some(1), "{args:?}: {text}");
        assert!(text.contains(says), "{args:?}: {text}");
    }
    write(&r.gate.join("dirt"), "x");
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is not clean"), "{text}");
    std::fs::remove_file(r.gate.join("dirt")).unwrap();

    let ahead = git::run(&r.main, &["rev-parse", "tools/tls-60-bad"]);
    let ahead = String::from_utf8(ahead.stdout).unwrap();
    git::run(
        &r.main,
        &["update-ref", "refs/remotes/origin/master", ahead.trim()],
    );
    let (code, text) = r.land(&["tools/tls-58-x"], &[]);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("is behind (or off) origin/master"), "{text}");
    git::run(&r.main, &["update-ref", "-d", "refs/remotes/origin/master"]);

    let (code, text) = r.land(&["--push", "--dry-run", "tools/tls-58-x"], &[]);
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
