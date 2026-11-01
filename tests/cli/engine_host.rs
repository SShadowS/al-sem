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

fn ask(argv: &[&str]) -> AskParams {
    AskParams {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        ..AskParams::default()
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
        Stderr::Lines(l) => al_sem::wire::stderr_bytes(&l),
        Stderr::File(f) => std::fs::read(&f.path).expect("the spilled stderr"),
    };
    (stdout, stderr, exit_code)
}

/// Stage-2 exit: front end → engine-host → explain / prove / digest / query
/// on a fixture answers the CLI's stdout byte for byte (decoded
/// `stdoutText`, its trailing newline included), its stderr and its exit; a
/// stdout over the request's inline cap comes back as `stdoutFile` with the
/// same bytes. One child answers every ask, and shuts down cleanly.
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
    for argv in commands {
        let expected = cli(argv);
        assert!(
            expected.0.ends_with(b"\n"),
            "{argv:?}: a CLI output ends its line"
        );
        let answer = front.ask(ask(argv));
        assert_eq!(answer.revision, None, "no resident revision in stage 2");
        let Outcome::Cli { stdout, .. } = &answer.outcome else {
            panic!("{argv:?}: {:?}", answer.outcome)
        };
        assert!(
            matches!(stdout, Stdout::Text(_)),
            "{argv:?}: inline under the cap"
        );
        assert_eq!(cli_answer(answer.outcome), expected, "{argv:?}");
        // Over the inline cap: the same bytes, as a file.
        let mut small = ask(argv);
        small.max_inline_bytes = Some(16);
        let answer = front.ask(small);
        let Outcome::Cli { stdout, .. } = &answer.outcome else {
            panic!("{argv:?}: {:?}", answer.outcome)
        };
        assert!(matches!(stdout, Stdout::File(_)), "{argv:?}: spilled");
        assert_eq!(cli_answer(answer.outcome), expected, "{argv:?} (file)");
    }
    let stats = front.stats();
    assert_eq!((stats.spawns, stats.deaths, stats.completed), (1, 0, 8));
    let report = front.shutdown();
    assert!(report.graceful, "{report:?}");
    assert!(report.status.is_some_and(|s| s.success()), "{report:?}");
}

/// `alsem engine-ask -- <argv>` (the front end as a process: it decodes,
/// decides the diff root and reads the diff from its own stdin) prints what
/// `alsem <argv>` prints, for a `--diff -` request and a clap error.
#[test]
fn engine_ask_prints_what_the_cli_prints() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let diff = "--- a/src/post.al\n+++ b/src/post.al\n@@ -14,5 +14,5 @@\n         if IsHandled then\n             exit;\n-        Order.Posted := false;\n+        Order.Posted := true;\n         Order.Validate(Status, Order.Status::Posted);\n         Order.Modify(true);\n";
    let run = |argv: &[&str], stdin: &str| {
        use std::io::Write;
        let mut child = Command::new(ALSEM)
            .args(argv)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (out.stdout, out.stderr, out.status.code())
    };
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
}
