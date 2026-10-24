//! #277 D2.3: a job's output as an ordered list of write actions, and the one
//! way a standalone run writes it.
//!
//! One text and one rule for a failed write (#160 spec §4.2.5): `al-sem: write
//! error: <stdout|path>: <OS message>` on stderr, and the command's own
//! failure code (2 for analyze, 1 for every other command). A closed pipe never
//! aborts the process, and neither does a closed stderr under it: the message
//! goes through [`write_stderr`], never a panicking `eprintln!`.

use std::io::Write;
use std::path::{Path, PathBuf};

/// One write a command makes, in the order its standalone run makes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteAction {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    File(PathBuf, Vec<u8>),
    /// A directory the following `File`s go into, created when missing.
    Dir(PathBuf),
}

impl WriteAction {
    /// `bytes` to `--out` when given, else stdout.
    pub fn out(out: Option<&str>, bytes: impl Into<Vec<u8>>) -> WriteAction {
        match out {
            Some(path) => WriteAction::File(PathBuf::from(path), bytes.into()),
            None => WriteAction::Stdout(bytes.into()),
        }
    }

    /// One stderr line: `line` plus `\n`.
    pub fn line(line: &str) -> WriteAction {
        WriteAction::Stderr(format!("{line}\n").into_bytes())
    }
}

/// A job's writes and its exit code (#277 D2.3), held as bytes until written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobOutput {
    pub actions: Vec<WriteAction>,
    pub exit: u8,
}

impl JobOutput {
    /// Each of `lines` on stderr, then `exit`: an error a command reports
    /// without output.
    pub fn lines(lines: &[String], exit: u8) -> JobOutput {
        JobOutput {
            actions: lines.iter().map(|l| WriteAction::line(l)).collect(),
            exit,
        }
    }

    /// A text command's writes (query, prove, digest): `Ok((text, exit))` is
    /// the text to `--out` when given, else stdout; `Err(message)` the message
    /// on stderr, exit 1.
    pub fn text(out: Option<&str>, result: Result<(String, u8), String>) -> JobOutput {
        match result {
            Ok((text, exit)) => JobOutput {
                actions: vec![WriteAction::out(out, text)],
                exit,
            },
            Err(message) => JobOutput::line(&message, 1),
        }
    }

    /// One `line` on stderr, then `exit`.
    pub fn line(line: &str, exit: u8) -> JobOutput {
        JobOutput {
            actions: vec![WriteAction::line(line)],
            exit,
        }
    }

    /// The stderr text without its last newline (a library wrapper's `Err`
    /// message).
    pub fn message(&self) -> String {
        let mut text = String::new();
        for a in &self.actions {
            if let WriteAction::Stderr(b) = a {
                text.push_str(&String::from_utf8_lossy(b));
            }
        }
        text.strip_suffix('\n').unwrap_or(&text).to_string()
    }
}

/// Write `out` on the real streams, in order (standalone). The first failed
/// action ends the list with the write error line on stderr (after a stderr
/// failure that line is lost with the stream) and returns
/// `write_failure_exit`; else `out.exit`.
pub fn replay(out: &JobOutput, write_failure_exit: u8) -> u8 {
    let (mut stdout, mut stderr) = (std::io::stdout().lock(), std::io::stderr().lock());
    replay_on(
        out,
        write_failure_exit,
        Stream(&mut stdout, "stdout"),
        Stream(&mut stderr, "stderr"),
    )
    .0
}

/// A job's stream: where its bytes go and the target a write error names.
pub struct Stream<'a>(pub &'a mut dyn Write, pub &'a str);

impl Stream<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.0
            .write_all(bytes)
            .and_then(|()| self.0.flush())
            .map_err(|e| (self.1.to_string(), e))
    }
}

/// [`replay`] onto `stdout` and `stderr` (a batch job's two files, D2.1). A
/// failed write prints the write error line on the process's stderr (a
/// failed stderr names itself, which a closed stream drops). Returns the exit
/// and every file and directory written, in order.
pub fn replay_on(
    out: &JobOutput,
    write_failure_exit: u8,
    mut stdout: Stream<'_>,
    mut stderr: Stream<'_>,
) -> (u8, Vec<PathBuf>) {
    let mut written = Vec::new();
    for action in &out.actions {
        let done = match action {
            WriteAction::Stdout(bytes) => stdout.write(bytes),
            WriteAction::Stderr(bytes) => stderr.write(bytes),
            WriteAction::File(path, bytes) => write_file(path, bytes),
            WriteAction::Dir(path) => create_dir(path),
        };
        match (done, action) {
            (Err((target, e)), _) => {
                write_error(&target, &e);
                return (write_failure_exit, written);
            }
            (Ok(()), WriteAction::File(path, _) | WriteAction::Dir(path)) => {
                written.push(path.clone())
            }
            (Ok(()), _) => {}
        }
    }
    (out.exit, written)
}

/// A failed write: its target and error.
type Failure = (String, std::io::Error);

/// Report a failed write of `target` on stderr.
pub fn write_error(target: &str, e: &std::io::Error) {
    let _ = write_stderr(format!("al-sem: write error: {target}: {e}\n").as_bytes());
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|e| (path.display().to_string(), e))
}

fn create_dir(path: &Path) -> Result<(), Failure> {
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|e| (path.display().to_string(), e))
}

/// Write `bytes` to stderr and flush.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut err = std::io::stderr().lock();
    err.write_all(bytes).and_then(|()| err.flush())
}
