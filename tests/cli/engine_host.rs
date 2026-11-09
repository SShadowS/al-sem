//! The engine child and its front end end to end (context-server design r5
//! §3, §4.3, §8.3; stage-2 plumbing): a front end ([`EngineFront`]) spawns
//! `alsem engine-host`, asks it commands, and every answer is the CLI's own
//! bytes; a child that misbehaves (an oversized or malformed frame, no hello,
//! a wrong protocol, a crash mid-request) gives a defined failure and is
//! restarted with backoff; a request that fails its planning answers the
//! CLI's outcome without reading the workspace.
//!
//! The fault tests need the child's test hooks (`ALSEM_ENGINE_TEST_HOOK`),
//! compiled into debug builds and into any build with the
//! `engine-test-hooks` feature (so they also run against a `release-ship`
//! binary built with that feature).

use std::path::{Path, PathBuf};
use std::process::Command;

use al_sem::engine::host::AskParams;
use al_sem::engine::host::front::{EngineFront, FrontConfig};
use al_sem::wire::{FailedOutcome, Outcome, Stderr, Stdout};

const ALSEM: &str = env!("CARGO_BIN_EXE_alsem");

fn ws_explain() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-explain")
}

/// `alsem <argv>` as a process: (stdout, stderr, exit).
fn cli(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let out = Command::new(ALSEM).args(argv).output().expect("alsem runs");
    let code = u8::try_from(out.status.code().expect("an exit code")).unwrap();
    (out.stdout, out.stderr, code)
}

fn front(workspace: &Path) -> EngineFront {
    EngineFront::new(FrontConfig::new(ALSEM, workspace)).expect("front end")
}

/// The params a real front end sends for `argv`: its decisions and request
/// files made by `alsem engine-ask --params` (the binary's own decoder).
fn ask(argv: &[&str]) -> AskParams {
    let out = Command::new(ALSEM)
        .args(["engine-ask", "--params", "--"])
        .args(argv)
        .output()
        .expect("alsem runs");
    assert!(
        out.status.success(),
        "{argv:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the params decode")
}

/// The bare params: the argv alone, no decisions, no files.
fn bare(argv: &[&str]) -> AskParams {
    AskParams {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        ..AskParams::default()
    }
}

/// A temporary copy of a corpus workspace (edits never touch the corpus).
fn copy_ws(name: &str) -> tempfile::TempDir {
    fn copy(from: &Path, to: &Path) {
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&t).unwrap();
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), &t).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus")
            .join(name),
        dir.path(),
    );
    dir
}

fn failed_outcome(o: &Outcome) -> Option<FailedOutcome> {
    match o {
        Outcome::Failed { outcome, .. } => Some(*outcome),
        Outcome::Cli { .. } => None,
    }
}

/// A CLI answer's (stdout bytes, stderr bytes, exit), reading a spilled
/// stdout from its file (checked against its length and hash).
fn cli_answer(outcome: Outcome) -> (Vec<u8>, Vec<u8>, u8) {
    let Outcome::Cli {
        exit_code,
        stdout,
        stderr,
        ..
    } = outcome
    else {
        panic!("a cli answer expected, got {outcome:?}")
    };
    let stdout = match stdout {
        Stdout::Text(t) => t.into_bytes(),
        Stdout::File(f) => {
            let b = std::fs::read(&f.path).expect("the spilled stdout");
            assert_eq!(b.len() as u64, f.bytes);
            assert_eq!(al_sem::engine::ids::sha256_bytes_hex(&b), f.sha256);
            b
        }
    };
    let stderr = match stderr {
        Stderr::Lines(l) => l.bytes().to_vec(),
        Stderr::File(f) => std::fs::read(&f.path).expect("the spilled stderr"),
    };
    (stdout, stderr, exit_code)
}

/// Stage-2 exit (T-order at the protocol): front end → engine-host →
/// explain / prove / digest / query on a fixture, all off ONE resident
/// revision, answers the CLI's stdout byte for byte (decoded `stdoutText`,
/// its trailing newline included), its stderr and its exit; a stdout over
/// the request's inline cap comes back as `stdoutFile` with the same bytes.
/// One child answers every ask from one build (one workspace read), every
/// answer names revision generation 1, and it shuts down cleanly.
#[test]
fn answers_are_byte_identical_to_the_cli() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut front = front(&ws);
    let commands: [&[&str]; 4] = [
        &["explain", w, "--routine", "Archive", "--format", "json"],
        &["prove", w, "PostOrder", "may-commit", "--deterministic"],
        &["digest", w, "--routine", "PostOrder", "--deterministic"],
        &["query", "touches", w, "--table", "50400", "--deterministic"],
    ];
    let mut stamp = None;
    for argv in commands {
        let expected = cli(argv);
        assert!(
            expected.0.ends_with(b"\n"),
            "{argv:?}: a CLI output ends its line"
        );
        let answer = front.ask(ask(argv));
        let revision = answer
            .revision
            .clone()
            .expect("a resident revision answers");
        assert_eq!(revision.generation, 1, "{argv:?}: one revision");
        assert!(revision.inputs_stamp.starts_with("al-sem-manifest-v1:"));
        assert_eq!(
            *stamp.get_or_insert(revision.inputs_stamp.clone()),
            revision.inputs_stamp
        );
        assert_eq!(answer.meta.universe.as_deref(), Some("crossApp"));
        assert!(answer.meta.timing.is_some());
        let Outcome::Cli { stdout, .. } = &answer.outcome else {
            panic!("{argv:?}: {:?}", answer.outcome)
        };
        assert!(
            matches!(stdout, Stdout::Text(_)),
            "{argv:?}: inline under the cap"
        );
        assert_eq!(cli_answer(answer.outcome), expected, "{argv:?}");
        // Over the inline cap: the same bytes, as a file (off Unix there is
        // no output directory: inline-only, `faults::without_an_output_
        // directory_answers_stay_inline`).
        let mut small = ask(argv);
        small.max_inline_bytes = Some(16);
        let answer = front.ask(small);
        let Outcome::Cli { stdout, .. } = &answer.outcome else {
            panic!("{argv:?}: {:?}", answer.outcome)
        };
        assert_eq!(
            matches!(stdout, Stdout::File(_)),
            cfg!(unix),
            "{argv:?}: spilled exactly where a directory exists"
        );
        assert_eq!(cli_answer(answer.outcome), expected, "{argv:?} (file)");
    }
    let stats = front.stats();
    assert_eq!((stats.spawns, stats.deaths, stats.completed), (1, 0, 8));
    let status = front.status().unwrap();
    assert_eq!(status.workspace_reads, 1, "one build served every ask");
    let fills = status.fills.expect("the revision's fill counts");
    assert_eq!(fills["byKind"]["fold"], 1, "{fills}");
    assert_eq!(fills["byKind"]["l4"], 1, "{fills}");
    let report = front.shutdown();
    assert!(report.graceful, "{report:?}");
    assert!(report.status.is_some_and(|s| s.success()), "{report:?}");
}

/// Engine limits B1: `prove` with a filter (`--match`, `--table`, `--op`,
/// `--site`, `--routes`) reads each routine's guard arms
/// (`ViewNeeds::PROVE_SLICE`), which the revision's own needs
/// (`REVISION_NEEDS`, explain's) leave out. Before the fix the first such ask
/// panicked in the child ("site guards not built") and killed the engine.
///
/// Precondition, stated: the first ask is an UNFILTERED prove, so the
/// published revision exists and was built without guard arms when the first
/// filtered ask arrives (checked: no needs rebuild yet). Then every filter,
/// in each universe, answers the one-shot CLI's bytes, through one child that
/// never dies; the revision is widened exactly once (`needsRebuilds` 1, class
/// `needs`), and every later filtered ask is served off the widened revision.
///
/// DISCRIMINATION (2026-11-09): turning off the engine host's retry on
/// `PrepareError::Needs` (`if false && needs_more.is_none()`) fails this test
/// at the first filter ("a revision answers": an engine error instead);
/// deleting the revision's `covers` check fails it the same way (the child
/// panics and dies). Each restored, it passes.
#[test]
fn every_prove_filter_is_answered_off_a_widened_revision() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let filtered: [&[&str]; 8] = [
        &[
            "prove",
            w,
            "PostOrder",
            "throws-error",
            "--match",
            "not released",
        ],
        &[
            "prove",
            w,
            "PostOrder",
            "throws-error",
            "--table",
            "Explain Order",
        ],
        &["prove", w, "PostOrder", "throws-error", "--op", "TestField"],
        &[
            "prove",
            w,
            "PostOrder",
            "writes-table:Explain Order",
            "--op",
            "Modify",
        ],
        &[
            "prove",
            w,
            "PostOrder",
            "throws-error",
            "--site",
            "src/post.al:11",
        ],
        &[
            "prove",
            w,
            "Archive",
            "may-commit",
            "--site",
            "src/post.al:27",
        ],
        &[
            "prove",
            w,
            "PostOrder",
            "reaches-ui",
            "--site",
            "src/post.al:19",
        ],
        &[
            "prove",
            w,
            "PostOrder",
            "throws-error",
            "--match",
            "released",
            "--routes",
            "2",
        ],
    ];
    for universe in [
        al_sem::engine::host::Universe::CrossApp,
        al_sem::engine::host::Universe::SingleApp,
    ] {
        let single = universe == al_sem::engine::host::Universe::SingleApp;
        let with_universe = |argv: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            v.push("--deterministic".into());
            if single {
                v.push("--single-app".into());
            }
            v
        };
        let mut cfg = FrontConfig::new(ALSEM, &ws);
        cfg.universe = universe;
        let mut front = EngineFront::new(cfg).expect("front end");
        let needs_rebuilds = |front: &mut EngineFront| {
            front
                .status()
                .unwrap()
                .session
                .expect("the session's stats")["needsRebuilds"]
                .as_u64()
                .expect("needsRebuilds")
        };
        let plain = with_universe(&["prove", w, "PostOrder", "may-commit"]);
        let plain: Vec<&str> = plain.iter().map(String::as_str).collect();
        let answer = front.ask(ask(&plain));
        assert_eq!(cli_answer(answer.outcome), cli(&plain), "{plain:?}");
        assert_eq!(
            needs_rebuilds(&mut front),
            0,
            "{universe:?}: no guard arms yet"
        );
        for argv in filtered {
            let argv = with_universe(argv);
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            let expected = cli(&argv);
            assert_eq!(expected.2, 0, "{argv:?}: the CLI answers");
            let answer = front.ask(ask(&argv));
            assert!(answer.revision.is_some(), "{argv:?}: a revision answers");
            assert_eq!(cli_answer(answer.outcome), expected, "{argv:?}");
        }
        let stats = front.stats();
        assert_eq!(
            (stats.spawns, stats.deaths),
            (1, 0),
            "{universe:?}: {stats:?}"
        );
        let status = front.status().unwrap();
        let session = status.session.expect("the session's stats");
        assert_eq!(session["needsRebuilds"], 1, "{universe:?}: {session}");
        assert_eq!(
            session["lastBuildClass"], "needs",
            "{universe:?}: {session}"
        );
        // The unfiltered question still answers the CLI's bytes off the
        // widened revision.
        let answer = front.ask(ask(&plain));
        assert_eq!(
            cli_answer(answer.outcome),
            cli(&plain),
            "{plain:?} (widened)"
        );
        let report = front.shutdown();
        assert!(report.graceful, "{report:?}");
    }
}

/// `alsem <argv>` as a process fed `stdin`: (stdout, stderr, exit).
///
/// A process may exit without reading its stdin (a clap error exits before
/// any command runs). Writing to it then fails with a broken pipe, at a
/// moment that depends on the scheduler: that is the process's outcome, not
/// a failure of this helper, so a broken pipe ends the write and the exit
/// and streams are compared as usual. Any other write error fails.
fn run_with_stdin(argv: &[&str], stdin: &[u8]) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    use std::io::Write;
    let mut child = Command::new(ALSEM)
        .args(argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Dropped at the end of the statement: the child sees its input end.
    match child.stdin.take().unwrap().write_all(stdin) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("write stdin: {e}"),
    }
    let out = child.wait_with_output().unwrap();
    (out.stdout, out.stderr, out.status.code())
}

/// The `--no-such-flag` case of the test below, made deterministic: a stdin
/// far over any pipe buffer cannot be written before the process exits, so
/// the write always meets a closed pipe (it used to do so only when the
/// process won the race, failing the test). Both `alsem <argv>` and `alsem
/// engine-ask -- <argv>` exit on the clap error without reading stdin; the
/// write's broken pipe is their outcome and the two still agree.
#[test]
fn a_process_that_exits_before_reading_its_stdin_is_compared() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let stdin = vec![b'x'; 4 << 20];
    let argv = ["explain", w, "--routine", "Archive", "--no-such-flag"];
    let direct = run_with_stdin(&argv, &stdin);
    assert_eq!(direct.2, Some(2), "a clap error: {direct:?}");
    let mut via = vec!["engine-ask", "--"];
    via.extend(argv);
    assert_eq!(run_with_stdin(&via, &stdin), direct);
}

/// `alsem engine-ask -- <argv>` (the front end as a process: it decodes,
/// decides the diff root and reads the diff from its own stdin) prints what
/// `alsem <argv>` prints, for a `--diff -` request and a clap error.
#[test]
fn engine_ask_prints_what_the_cli_prints() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let diff = "--- a/src/post.al\n+++ b/src/post.al\n@@ -14,5 +14,5 @@\n         if IsHandled then\n             exit;\n-        Order.Posted := false;\n+        Order.Posted := true;\n         Order.Validate(Status, Order.Status::Posted);\n         Order.Modify(true);\n";
    let run = |argv: &[&str], stdin: &str| run_with_stdin(argv, stdin.as_bytes());
    for argv in [
        vec!["explain", w, "--diff", "-", "--format", "json"],
        vec!["explain", w, "--routine", "Archive", "--no-such-flag"],
    ] {
        let direct = run(&argv, diff);
        let mut via = vec!["engine-ask", "--"];
        via.extend(&argv);
        let engine = run(&via, diff);
        let text = |o: &(Vec<u8>, Vec<u8>, Option<i32>)| {
            (
                String::from_utf8_lossy(&o.0).into_owned(),
                String::from_utf8_lossy(&o.1).into_owned(),
                o.2,
            )
        };
        assert_eq!(text(&engine), text(&direct), "{argv:?}");
    }
}

/// r5 §4.3: a request that fails its planning (an invalid format, an
/// invalid question, an unreadable diff file) answers the CLI's own text and
/// exit as a `kind: "cli"` answer, and the child reads no workspace for it.
/// A valid request on a workspace that does not build answers the job's own
/// failure text (a semantic build failure is `kind: "cli"` too); an invalid
/// format on that broken workspace still answers the planning error.
#[test]
fn planning_failures_keep_cli_outcomes_and_read_nothing() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut front = front(&ws);
    for argv in [
        vec!["explain", w, "--routine", "Archive", "--format", "bogus"],
        vec!["prove", w, "PostOrder", "bogus"],
        vec!["digest", w, "--diff", "/nonexistent/x.diff"],
    ] {
        let expected = cli(&argv);
        assert_ne!(expected.2, 0, "{argv:?}: precondition, the CLI fails it");
        assert_eq!(
            cli_answer(front.ask(ask(&argv)).outcome),
            expected,
            "{argv:?}"
        );
    }
    let status = front.status().expect("status");
    assert_eq!(status.asks, 3);
    assert_eq!(
        status.workspace_reads, 0,
        "no planning failure reads the workspace"
    );

    let broken = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(broken.path().join("src")).unwrap();
    std::fs::write(broken.path().join("src/a.al"), "codeunit 50100 A { }").unwrap();
    let b = broken.path().to_str().unwrap();
    let mut front = self::front(broken.path());
    for (argv, reads) in [
        (vec!["explain", b, "--routine", "A", "--format", "json"], 1),
        (vec!["explain", b, "--routine", "A", "--format", "bogus"], 1),
    ] {
        let expected = cli(&argv);
        assert_ne!(expected.2, 0, "{argv:?}: precondition, the CLI fails it");
        assert_eq!(
            cli_answer(front.ask(ask(&argv)).outcome),
            expected,
            "{argv:?}"
        );
        assert_eq!(front.status().unwrap().workspace_reads, reads, "{argv:?}");
    }
}

/// A request naming another workspace than the session's is refused,
/// `notThisWorkspace`, before any read.
#[test]
fn another_workspace_is_refused() {
    let ws = ws_explain();
    let other = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-rules");
    let mut front = front(&ws);
    let answer = front.ask(ask(&["explain", other.to_str().unwrap(), "--routine", "X"]));
    assert!(
        matches!(
            answer.outcome,
            Outcome::Failed {
                outcome: FailedOutcome::NotThisWorkspace,
                ..
            }
        ),
        "{:?}",
        answer.outcome
    );
    assert_eq!(front.status().unwrap().workspace_reads, 0);
}

/// T-fresh / T-manifest at the protocol: asks with no edit between them
/// reuse the revision (no second read); a body edit is seen by the next ask
/// (generation 2, a new stamp) whose answer equals a cold CLI run of the
/// edited workspace; `latest-built` answers at once from the published
/// revision, `stale` with the changed path; `verify` re-hashes everything.
#[test]
fn an_edit_between_asks_is_answered_from_a_refreshed_revision() {
    let dir = copy_ws("ws-explain");
    let w = dir.path().to_str().unwrap();
    let argv = ["prove", w, "Archive", "may-commit", "--deterministic"];
    let mut front = front(dir.path());
    let first = front.ask(ask(&argv));
    assert_eq!(cli_answer(first.outcome.clone()), cli(&argv));
    let r1 = first.revision.unwrap();
    let again = front.ask(ask(&argv));
    assert_eq!(
        again.revision.as_ref(),
        Some(&r1),
        "no edit: the same revision"
    );
    assert_eq!(front.status().unwrap().workspace_reads, 1, "no rebuild");
    let post = dir.path().join("src/post.al");
    let text = std::fs::read_to_string(&post).unwrap();
    assert!(
        text.contains("        Order.Modify();\n        Commit();"),
        "precondition"
    );
    std::fs::write(
        &post,
        text.replace(
            "        Order.Modify();\n        Commit();",
            "        Order.Modify(true);\n        Commit();",
        ),
    )
    .unwrap();
    // latest-built: the old revision at once, stale, the path named.
    let mut stale = ask(&argv);
    stale.freshness = al_sem::engine::resident::session::Freshness::LatestBuilt;
    let s = front.ask(stale);
    assert!(s.meta.stale, "{:?}", s.meta);
    let Outcome::Cli { changed_paths, .. } = &s.outcome else {
        panic!("{:?}", s.outcome)
    };
    assert!(
        changed_paths.iter().any(|p| p.ends_with("post.al")),
        "{changed_paths:?}"
    );
    // fresh (a background refresh may have published already).
    let fresh = front.ask(ask(&argv));
    let r2 = fresh.revision.clone().unwrap();
    assert_eq!(r2.generation, 2, "the edit is seen");
    assert_ne!(r2.inputs_stamp, r1.inputs_stamp);
    assert!(!fresh.meta.stale);
    assert_eq!(
        cli_answer(fresh.outcome),
        cli(&argv),
        "refreshed equals cold"
    );
    let mut verify = ask(&argv);
    verify.freshness = al_sem::engine::resident::session::Freshness::Verify;
    assert_eq!(
        front.ask(verify).revision,
        Some(r2),
        "verify: nothing changed"
    );
}

/// The child's allocator hands freed large blocks back to the OS
/// (`al_sem::release_alloc`, design §7's per-refresh RSS creep): `alsem
/// engine-host` turns the release on at `RELEASE_MIN` and its status says
/// so; after a refresh (the previous revision freed) blocks were released
/// (Linux, the one platform that releases), transparent huge pages are off
/// in a `--low-mem` child only, and the answers stay the CLI's bytes.
/// `ALSEM_RELEASE_MIN=0` turns both off: nothing is released.
#[test]
fn the_child_releases_freed_large_blocks() {
    let dir = copy_ws("ws-explain");
    let w = dir.path().to_str().unwrap().to_string();
    let argv = ["prove", &w, "Archive", "may-commit", "--deterministic"];
    let post = dir.path().join("src/post.al");
    let text = std::fs::read_to_string(&post).unwrap();
    let edited = text.replace(
        "        Order.Modify();\n        Commit();",
        "        Order.Modify(true);\n        Commit();",
    );
    assert_ne!(edited, text, "precondition: the edit applies");
    for (off, low_mem) in [(false, false), (false, true), (true, true)] {
        std::fs::write(&post, &text).unwrap();
        let mut cfg = FrontConfig::new(ALSEM, dir.path());
        if off {
            cfg.env.push(("ALSEM_RELEASE_MIN".into(), "0".into()));
        }
        if low_mem {
            cfg.args.push("--low-mem".into());
        }
        let mut front = EngineFront::new(cfg).expect("front end");
        assert_eq!(cli_answer(front.ask(ask(&argv)).outcome), cli(&argv));
        std::fs::write(&post, &edited).unwrap();
        let fresh = front.ask(ask(&argv));
        assert_eq!(fresh.revision.as_ref().map(|r| r.generation), Some(2));
        assert_eq!(
            cli_answer(fresh.outcome),
            cli(&argv),
            "refreshed equals cold"
        );
        let memory = front.status().unwrap().memory.expect("memory status");
        let want = if off {
            0
        } else {
            al_sem::release_alloc::RELEASE_MIN as u64
        };
        assert_eq!(memory["releaseFromBytes"], want, "{memory}");
        let released = memory["releasedBlocks"].as_u64().expect("releasedBlocks");
        let on_linux = !off && cfg!(target_os = "linux");
        if on_linux {
            assert!(released > 0, "the refresh released freed blocks: {memory}");
        } else {
            assert_eq!(released, 0, "{memory}");
        }
        assert_eq!(memory["thpDisabled"], on_linux && low_mem, "{memory}");
        front.shutdown();
    }
}

/// `alsem impact`'s production parts are answered OFF the resident revision
/// (its retained artifacts and sited graph), never by the one-shot job path
/// over its inputs, and a warm repeat fills nothing again: the main part
/// (digest's plan over the roots) and the up part (the sited graph) each
/// count as an answer of the revision, the CLI's part answer either way (the
/// main part's bytes; the up part's but for its wall-clock `walkMs`); the
/// up part's sited graph is added to the revision the main part filled (no
/// second read, no new revision, nothing refilled); a second round reads no
/// workspace, keeps the revision, adds two answers and no fill. Fixture:
/// `tests/fixtures/test-universe/App`, copied with a `.git` marker (as
/// `mcp_impact` does).
///
/// DISCRIMINATION (2026-11-05): `"impact"` and `"impact-up"` removed from
/// `SERVED_COMMANDS` (the state B1 shipped, where every CDO part rebuilt the
/// program, 24-96 s per main part warm): fails at "the main part is
/// answered off the revision" (answered 0); restored, passes. `pin_sited`
/// rebuilding as first built (its `add_sited_locked` branch removed): fails
/// at "the up part is answered off the same revision" (`(2, 2, 1, 0)`: a
/// second read, generation 2, an empty store, whose warm main part then
/// filled 8 artifacts again); restored, passes.
#[test]
fn impact_parts_are_answered_off_the_revision_and_a_repeat_fills_nothing() {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), &t).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-universe"),
        dir.path(),
    );
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let ws = dir.path().join("App");
    let w = ws.to_str().unwrap().to_string();
    let main = [
        "impact",
        "--part",
        "main",
        "--routine=TU Core::Post",
        "--",
        &w,
    ];
    let mut front = front(&ws);
    // A part's answer as JSON, its wall-clock fields (the up part's
    // `cost.walkMs` and `cost.walksUs`) blanked: the rest is the CLI's to
    // the byte.
    let part = |out: (Vec<u8>, Vec<u8>, u8)| -> (serde_json::Value, Vec<u8>, u8) {
        let mut v: serde_json::Value = serde_json::from_slice(&out.0).unwrap();
        if let Some(c) = v.get_mut("cost").and_then(|c| c.as_object_mut()) {
            c.remove("walkMs");
            c.remove("walksUs");
        }
        (v, out.1, out.2)
    };
    let answered = |front: &mut EngineFront| -> (u64, u64, u64, u64) {
        let s = front.status().unwrap();
        let f = s.fills.expect("the revision's fill counts");
        (
            s.workspace_reads,
            s.revision.expect("a revision").generation,
            f["answered"].as_u64().unwrap(),
            f["fills"].as_u64().unwrap(),
        )
    };

    let a = front.ask(ask(&main));
    let main_out = cli_answer(a.outcome);
    assert_eq!(main_out, cli(&main), "the main part's CLI bytes");
    let doc: serde_json::Value = serde_json::from_slice(&main_out.0).unwrap();
    let root = doc["roots"][0]["stableId"].as_str().unwrap().to_string();
    let (reads, generation, main_answered, fills) = answered(&mut front);
    assert_eq!(
        main_answered, 1,
        "the main part is answered off the revision"
    );
    assert_eq!((reads, generation), (1, 1));
    assert!(fills > 0, "precondition: the main part filled artifacts");

    let root_arg = format!("--root-id={root}");
    let up = ["impact", "--part", "up", &root_arg, "--", &w];
    let a = front.ask(ask(&up));
    assert_eq!(
        part(cli_answer(a.outcome)),
        part(cli(&up)),
        "the up part's CLI answer"
    );
    // The up part reads the sited graph the revision did not have: it is
    // added to that revision (`pin_sited`), which keeps its artifacts.
    assert_eq!(
        answered(&mut front),
        (1, 1, 2, fills),
        "the up part is answered off the same revision, nothing refilled"
    );

    // Warm repeat: the same revision answers both, filling nothing.
    for argv in [&main[..], &up[..]] {
        let a = front.ask(ask(argv));
        assert_eq!(a.revision.as_ref().map(|r| r.generation), Some(1));
        assert_eq!(part(cli_answer(a.outcome)), part(cli(argv)), "{argv:?}");
    }
    assert_eq!(
        answered(&mut front),
        (1, 1, 4, fills),
        "a warm repeat reads nothing, keeps the revision, fills nothing"
    );
    assert!(front.shutdown().graceful);
}

/// T-exit3 / T-plan: a planning failure with exit 3 (`digest --order`, a
/// refused flag) is the CLI's own `kind: "cli"` answer, exit 3, with
/// `revision: null` and no workspace read; so is an engine-only refusal
/// (a command the engine does not run).
#[test]
fn exit_three_is_preserved() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut front = front(&ws);
    let argv = ["digest", w, "--routine", "PostOrder", "--order"];
    let expected = cli(&argv);
    assert_eq!(expected.2, 3, "precondition: the CLI exits 3");
    let answer = front.ask(ask(&argv));
    assert_eq!(answer.revision, None);
    assert_eq!(cli_answer(answer.outcome), expected);
    let refused = front.ask(bare(&["batch", "x.json"]));
    let (_, _, exit) = cli_answer(refused.outcome);
    assert_eq!(exit, 3);
    assert_eq!(front.status().unwrap().workspace_reads, 0);
}

/// The front end's decisions are mandatory when a command reads paths: an
/// ask without them is refused `protocol` (the child never decides); with
/// them and its diff file shipped as `requestFiles` (the child never reads
/// it), the answer equals the CLI's.
///
/// DISCRIMINATION (2026-11-02): dropping the child's check (`if false &&
/// reads_paths && …` in `decided`) answers the bare ask as a CLI outcome:
/// "left: None, right: Some(Protocol)".
#[test]
fn decisions_and_request_files_come_from_the_front_end() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let diff_dir = tempfile::tempdir().unwrap();
    let diff = diff_dir.path().join("change.diff");
    std::fs::write(
        &diff,
        "--- a/src/post.al\n+++ b/src/post.al\n@@ -24,5 +24,5 @@\n     begin\n         Order.Status := Order.Status::Archived;\n-        Order.Modify(true);\n+        Order.Modify();\n         Commit();\n     end;\n",
    )
    .unwrap();
    let d = diff.to_str().unwrap();
    let argv = ["digest", w, "--diff", d, "--deterministic"];
    let mut front = front(&ws);
    let refused = front.ask(bare(&argv));
    assert_eq!(
        failed_outcome(&refused.outcome),
        Some(FailedOutcome::Protocol)
    );
    let params = ask(&argv);
    assert!(
        params.request_files.is_some(),
        "precondition: the diff is shipped"
    );
    let expected = cli(&argv);
    // The child must not read the file: remove it before asking.
    std::fs::remove_file(&diff).unwrap();
    assert_eq!(cli_answer(front.ask(params).outcome), expected);
}

/// The child enforces its universe: a `--single-app` ask to a cross-app
/// session is refused `notThisWorkspace`; a single-app session answers it.
#[test]
fn the_other_universe_is_refused() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let argv = ["query", "touches", w, "--table", "50400", "--single-app"];
    let mut front = front(&ws);
    let answer = front.ask(ask(&argv));
    assert_eq!(
        failed_outcome(&answer.outcome),
        Some(FailedOutcome::NotThisWorkspace)
    );
    let mut cfg = FrontConfig::new(ALSEM, &ws);
    cfg.universe = al_sem::engine::host::Universe::SingleApp;
    let mut single = EngineFront::new(cfg).unwrap();
    let answer = single.ask(ask(&argv));
    assert_eq!(answer.meta.universe.as_deref(), Some("singleApp"));
    assert_eq!(cli_answer(answer.outcome), cli(&argv));
}

/// A `deadlineMs` that passes before the ask runs answers `deadline` with no
/// work done; a cancel through the front end's watch answers `cancelled`.
#[test]
fn deadline_and_cancel_through_the_front_end() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut front = front(&ws);
    let mut p = ask(&["explain", w, "--routine", "Archive"]);
    p.deadline_ms = Some(0);
    let a = front.ask(p);
    assert_eq!(failed_outcome(&a.outcome), Some(FailedOutcome::Deadline));
    assert_eq!(a.meta.work.as_ref().map(|w| w.work_done_ms), Some(0));
    let flag = std::sync::atomic::AtomicBool::new(true);
    let mut watch = al_sem::engine::host::front::Watch {
        cancel: Some(&flag),
        progress: None,
    };
    let a = front.ask_with(ask(&["prove", w, "PostOrder", "may-commit"]), &mut watch);
    // Cancelled before or during its run; never a CLI answer.
    assert_eq!(
        failed_outcome(&a.outcome),
        Some(FailedOutcome::Cancelled),
        "{:?}",
        a.outcome
    );
}

#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
mod faults {
    use super::*;
    use al_sem::engine::host::test_hooks::ENV;
    use al_sem::wire::MAX_FRAME_BYTES;
    use std::time::{Duration, Instant};

    /// A front end whose child runs `hook`, with short clocks.
    fn hooked(hook: &str) -> EngineFront {
        let mut cfg = FrontConfig::new(ALSEM, ws_explain());
        cfg.env.push((ENV.into(), hook.into()));
        cfg.hello_deadline = Duration::from_millis(1500);
        cfg.backoff_initial = Duration::from_millis(200);
        cfg.backoff_max = Duration::from_secs(2);
        EngineFront::new(cfg).expect("front end")
    }

    fn explain_argv() -> Vec<String> {
        let w = ws_explain();
        [
            "explain",
            w.to_str().unwrap(),
            "--routine",
            "Archive",
            "--format",
            "json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn explain() -> AskParams {
        AskParams {
            argv: explain_argv(),
            ..AskParams::default()
        }
    }

    fn failure(outcome: &Outcome) -> (FailedOutcome, &str) {
        match outcome {
            Outcome::Failed {
                outcome, message, ..
            } => (*outcome, message.as_str()),
            other => panic!("a failed answer expected, got {other:?}"),
        }
    }

    /// A child frame over 16 MiB is refused when the reader has read the
    /// limit plus one byte (never the whole line), and the child is killed:
    /// `protocol`, one death, no child running.
    #[test]
    fn an_oversized_child_frame_is_refused_and_kills_the_child() {
        let mut front = hooked("oversized-frame-on-ask");
        let started = Instant::now();
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::Protocol, "{message}");
        assert!(
            message.contains(&format!("frame of {} bytes exceeds", MAX_FRAME_BYTES + 1)),
            "read stopped at the limit plus one byte: {message}"
        );
        assert!(front.hello().is_none(), "the child was killed");
        assert_eq!(front.stats().deaths, 1);
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// Wait (bounded) until the reader has ended the running child.
    fn wait_for_exit(front: &EngineFront) {
        let started = Instant::now();
        while front.child_exited() != Some(true) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the child was not ended: {:?}",
                front.queue_stats()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Review r6 NEW 5 / design r7 §3: a child that floods small frames
    /// (valid responses, distinct notifications, garbage lines) while the
    /// front end reads nothing (started, no ask waiting) never grows the
    /// queue past its bounds (1,024 frames, 64 MiB): the READER ends the
    /// child with no ask involved and records why; the next request (here
    /// `status`) fails with that reason as `protocol`, the death counted.
    #[test]
    fn a_flooding_child_is_ended_with_no_ask_waiting() {
        use al_sem::engine::host::{QUEUE_BYTES, QUEUE_FRAMES};
        for kind in ["response", "notify", "garbage"] {
            let mut front = hooked(&format!("flood-after-open:{kind}:200000"));
            front.ensure_started().expect("started");
            wait_for_exit(&front);
            let stats = front.queue_stats().expect("the child's queue");
            assert!(stats.peak_frames <= QUEUE_FRAMES, "{kind}: {stats:?}");
            assert!(stats.peak_bytes <= QUEUE_BYTES, "{kind}: {stats:?}");
            let why = stats.failure.clone().expect("the reader recorded why");
            assert!(why.starts_with("engine protocol error"), "{kind}: {why}");
            if kind != "garbage" {
                assert_eq!(
                    stats.peak_frames, QUEUE_FRAMES,
                    "{kind}: filled to the bound"
                );
                assert!(why.contains("queue bound"), "{kind}: {why}");
            }
            let err = front
                .status()
                .expect_err("the ended child fails the request");
            assert!(err.contains(&why), "{kind}: {err}");
            assert_eq!(
                front.last_failure().map(|(o, _)| o),
                Some(FailedOutcome::Protocol),
                "{kind}"
            );
            assert_eq!(front.stats().deaths, 1, "{kind}");
            assert!(front.hello().is_none(), "{kind}");
        }
    }

    /// Progress notifications are coalesced per request: a child that sends
    /// 200,000 of them for one request while the front end reads nothing
    /// holds one queued frame, is not ended, and answers the next ask.
    #[test]
    fn a_progress_flood_is_coalesced_and_the_child_lives() {
        let mut front = hooked("flood-after-open:progress:200000");
        front.ensure_started().expect("started");
        let started = Instant::now();
        while front.queue_stats().expect("running").frames == 0 {
            assert!(started.elapsed() < Duration::from_secs(30), "no flood");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        // Unread: one frame, however many came (the peak may also count the
        // `session/open` response, taken just after the first progress).
        let stats = front.queue_stats().expect("running");
        assert_eq!(stats.frames, 1, "{stats:?}");
        assert!(stats.peak_frames <= 2, "{stats:?}");
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let expected = cli(&argv);
        let answer = front.ask(explain());
        assert_eq!(cli_answer(answer.outcome), expected);
        // At most the newest progress beside the ask's own response.
        let stats = front.queue_stats().expect("running");
        assert!(stats.peak_frames <= 2, "{stats:?}");
        assert_eq!(stats.failure, None);
        assert_eq!(front.stats().deaths, 0);
    }

    /// A stray print to the child's standard output (anywhere in the engine)
    /// lands in its drained stderr, never in the protocol stream: the ask
    /// answers the CLI's bytes and the child lives (Unix: fd 1 moved onto
    /// stderr; Windows: the standard output handle moved,
    /// `child::protocol_stdout`).
    ///
    /// DISCRIMINATION (2026-11-02, Linux): leaving fd 1 in place (the
    /// `dup2(2, 1)` dropped from `protocol_stdout`) fails the ask
    /// `protocol` ("engine protocol error"): the CLI-bytes assertion panics.
    #[test]
    fn a_stray_print_never_reaches_the_protocol_stream() {
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let expected = cli(&argv);
        let mut front = hooked("stray-stdout-on-ask");
        assert_eq!(cli_answer(front.ask(explain()).outcome), expected);
        assert_eq!(front.stats().deaths, 0);
        let started = Instant::now();
        while !front.stderr_tail().contains("a stray print to stdout") {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the print reached stderr: {}",
                front.stderr_tail()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Inline-only (design §10, Windows fail-closed; the `no-output-dir`
    /// hook takes that path on any platform): the child makes no output
    /// directory, and an answer over the ask's inline cap still comes back
    /// inline with the CLI's bytes (the cap raised to `MAX_INLINE_BYTES`).
    ///
    /// DISCRIMINATION (2026-11-02): not raising the cap (`ask_inline_only`
    /// asking with the request's own params) answers `refusedPath` ("...
    /// spilling stdout: no output directory on this platform: inline-only"):
    /// the `Outcome::Cli` pattern panics.
    #[test]
    fn without_an_output_directory_answers_stay_inline() {
        let w = ws_explain();
        let w = w.to_str().unwrap();
        let argv = ["query", "touches", w, "--table", "50400", "--deterministic"];
        let expected = cli(&argv);
        let mut front = hooked("no-output-dir");
        let mut small = ask(&argv);
        small.max_inline_bytes = Some(16);
        let answer = front.ask(small);
        let Outcome::Cli { stdout, .. } = &answer.outcome else {
            panic!("a cli answer: {:?}", answer.outcome)
        };
        assert!(matches!(stdout, Stdout::Text(_)), "inline: {stdout:?}");
        assert_eq!(cli_answer(answer.outcome), expected);
        assert_eq!(front.status().expect("status").output_dir, None);
    }

    /// A child frame that is not JSON kills the child: `protocol`.
    #[test]
    fn a_malformed_child_frame_kills_the_child() {
        let mut front = hooked("garbage-frame-on-ask");
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::Protocol, "{message}");
        assert!(message.contains("engine protocol error"), "{message}");
        assert!(front.hello().is_none());
        assert_eq!(front.stats().deaths, 1);
    }

    /// A child that sends no hello within the deadline is killed and the ask
    /// fails `engineError` "engine start timeout", at the deadline.
    #[test]
    fn a_child_with_no_hello_times_out() {
        let mut front = hooked("no-hello");
        let started = Instant::now();
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::EngineError, "{message}");
        assert!(message.contains("engine start timeout"), "{message}");
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(1500) && took < Duration::from_secs(10),
            "{took:?}"
        );
        assert!(front.hello().is_none());
    }

    /// A hello with a protocol version the front end does not support kills
    /// the child: `protocol`, "protocol mismatch".
    #[test]
    fn an_unknown_protocol_version_is_refused() {
        let mut front = hooked("protocol:99");
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::Protocol, "{message}");
        assert!(message.contains("protocol mismatch"), "{message}");
        assert!(message.contains("99"), "{message}");
    }

    /// A child that crashes mid-request (an abort, as a panic is under
    /// `release-ship`) fails that ask with `engineError` naming the signal;
    /// the next ask restarts it after the backoff delay and answers the CLI's
    /// bytes; that completed ask resets the backoff.
    #[test]
    fn a_crash_mid_request_fails_the_ask_and_the_next_ask_respawns() {
        let marker = tempfile::tempdir().unwrap();
        let hook = format!(
            "abort-on-ask-once:{}",
            marker.path().join("crashed").display()
        );
        let mut front = hooked(&hook);
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::EngineError, "{message}");
        assert!(message.contains("engine died"), "{message}");
        #[cfg(unix)]
        assert!(message.contains("signal 6"), "an abort: {message}");
        assert_eq!(
            front.current_backoff(),
            Duration::from_millis(400),
            "doubled"
        );
        let started = Instant::now();
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let expected = cli(&argv);
        let answer = front.ask(explain());
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "waited the backoff"
        );
        assert_eq!(cli_answer(answer.outcome), expected);
        let stats = front.stats();
        assert_eq!((stats.spawns, stats.deaths, stats.completed), (2, 1, 1));
        assert_eq!(
            front.current_backoff(),
            Duration::from_millis(200),
            "a completed ask resets the backoff"
        );
    }

    /// A child that exits before reading the request: the front end's write
    /// of the request fails with a broken pipe, deterministically (a request
    /// far over any pipe buffer cannot be written to a reader that is gone),
    /// and that failure is an answer, `engineError` naming the failed write,
    /// with the death recorded, never a crash of the front end.
    #[test]
    fn a_child_gone_before_the_request_write_fails_the_ask() {
        let mut front = hooked("exit-after-open");
        let answer = front.ask(AskParams {
            diff_text: Some("x".repeat(4 << 20)),
            ..explain()
        });
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::EngineError, "{message}");
        assert!(
            message.contains("writing to the engine failed"),
            "{message}"
        );
        let stats = front.stats();
        assert_eq!((stats.spawns, stats.deaths, stats.completed), (1, 1, 0));
        assert_eq!(front.child_exited(), None, "no child running");
    }

    /// A child that dies on every ask: each restart waits longer (the
    /// backoff doubles, a hello alone never resets it), and the third death
    /// within the window fails every later ask at once until reset.
    #[test]
    fn repeated_deaths_back_off_then_fail_until_reset() {
        let mut front = hooked("abort-on-ask");
        let mut waits = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
            let (outcome, _) = failure(&front.ask(explain()).outcome);
            assert_eq!(outcome, FailedOutcome::EngineError);
            waits.push(started.elapsed());
        }
        assert!(waits[1] >= Duration::from_millis(200), "{waits:?}");
        assert!(waits[2] >= Duration::from_millis(400), "{waits:?}");
        assert_eq!(front.stats().spawns, 3);
        let started = Instant::now();
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::EngineError);
        assert!(message.contains("engine failed: 3 deaths"), "{message}");
        assert!(started.elapsed() < Duration::from_millis(100), "no spawn");
        assert_eq!(front.stats().spawns, 3);
        front.reset();
        let _ = front.ask(explain());
        assert_eq!(front.stats().spawns, 4, "a reset restarts");
    }

    /// T-panic (design §11, every profile): a real Rust panic while an ask
    /// is answered ends the child (under the abort profile the panic
    /// aborts; under an unwinding one the worker's guard aborts), the ask
    /// fails `engineError` naming the abort, and the next ask is answered
    /// by a new child with the CLI's bytes.
    ///
    /// DISCRIMINATION (2026-11-02): removing the worker's `catch_unwind`
    /// guard in `child::work` (an unwinding debug build) fails it: the
    /// panicking worker thread dies, the reader keeps the child alive, and
    /// the ask is never answered (the test's front end then waits forever;
    /// it was stopped by `timeout` after 120 s).
    #[test]
    fn a_panic_mid_request_kills_the_child_and_the_next_ask_respawns() {
        let marker = tempfile::tempdir().unwrap();
        let hook = format!(
            "panic-on-ask-once:{}",
            marker.path().join("panicked").display()
        );
        let mut front = hooked(&hook);
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::EngineError, "{message}");
        assert!(message.contains("engine died"), "{message}");
        #[cfg(unix)]
        assert!(message.contains("signal 6"), "an abort: {message}");
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(cli_answer(front.ask(explain()).outcome), cli(&argv));
        let stats = front.stats();
        assert_eq!((stats.spawns, stats.deaths, stats.completed), (2, 1, 1));
    }

    /// `--idle-timeout` (design §7): a child with nothing to do announces
    /// its idle exit and ends after the grace; the front end's next ask
    /// starts a new child without counting a death or waiting a backoff,
    /// and is answered with the CLI's bytes.
    ///
    /// DISCRIMINATION (2026-11-02): ignoring the notice in
    /// `EngineFront::ensure_running` (`let idle = false && …`) fails it: the
    /// next ask meets the closed pipe and fails `engineError` ("engine died
    /// (writing to the engine failed: Broken pipe …; exit code 0)").
    #[test]
    fn an_idle_child_ends_and_the_next_ask_starts_a_new_one() {
        let mut cfg = FrontConfig::new(ALSEM, ws_explain());
        cfg.args.extend(
            ["--idle-timeout", "1", "--idle-grace-ms", "200"]
                .iter()
                .map(Into::into),
        );
        let mut front = EngineFront::new(cfg).expect("front end");
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let expected = cli(&argv);
        assert_eq!(cli_answer(front.ask(explain()).outcome), expected);
        let started = Instant::now();
        while front.child_exited() != Some(true) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the idle child never ended"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(cli_answer(front.ask(explain()).outcome), expected);
        let stats = front.stats();
        assert_eq!(
            (stats.spawns, stats.deaths, stats.retired),
            (2, 0, 1),
            "no death: {stats:?}"
        );
    }

    /// T-budget through the child: a workspace whose pre-model estimate
    /// exceeds `--memory-budget` is refused `budget` before it is read,
    /// with the estimate, budget and floor; `status` reports the budget.
    ///
    /// DISCRIMINATION (2026-11-02): opening the session without its budget
    /// (`ResidentSession::new(..)` with no `.with_budget`) fails "cold build":
    /// the model is built and read, and only the first fill is refused.
    #[test]
    fn a_workload_over_the_child_budget_is_refused_with_its_figures() {
        let mut cfg = FrontConfig::new(ALSEM, ws_explain());
        cfg.args.extend(
            [
                "--memory-budget",
                "64M",
                "--estimate-per-al-byte",
                "1000000",
            ]
            .iter()
            .map(Into::into),
        );
        let mut front = EngineFront::new(cfg).expect("front end");
        let answer = front.ask(explain());
        let (outcome, message) = failure(&answer.outcome);
        assert_eq!(outcome, FailedOutcome::Budget, "{message}");
        assert!(message.contains("cold build"), "{message}");
        let figures = answer.meta.budget.expect("the budget figures");
        assert_eq!(figures.budget_kib, 64 * 1024);
        assert!(figures.estimate_kib > figures.budget_kib);
        let status = front.status().expect("status");
        assert_eq!(status.workspace_reads, 0, "refused before the read");
        let memory = status.memory.expect("memory status");
        assert_eq!(memory["budgetBytes"], 64 * 1024 * 1024);
        assert_eq!(memory["impossible"], 1);
    }

    /// `--low-mem` reaches the child (design §7): its status says so, its
    /// default budget is the low-mem one, and its answers are the CLI's
    /// bytes. A child without the flag reports the default mode.
    #[test]
    fn a_low_mem_child_says_so_and_answers_the_cli_bytes() {
        use al_sem::engine::resident::budget::default_budget;
        let argv = explain_argv();
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let expected = cli(&argv);
        for low_mem in [false, true] {
            let mut cfg = FrontConfig::new(ALSEM, ws_explain());
            if low_mem {
                cfg.args.push("--low-mem".into());
            }
            let mut front = EngineFront::new(cfg).expect("front end");
            assert_eq!(
                cli_answer(front.ask(explain()).outcome),
                expected,
                "{low_mem}"
            );
            let status = front.status().expect("status");
            assert_eq!(status.low_mem, low_mem);
            let memory = status.memory.expect("memory status");
            assert_eq!(memory["budgetBytes"], default_budget(false, low_mem));
        }
    }
}
