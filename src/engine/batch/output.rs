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
}

/// Write `out` on the real streams, in order (standalone). The first failed
/// action ends the list: a stdout, file or directory failure prints the write
/// error line; a stderr failure prints nothing (the stream is gone). Either
/// returns `write_failure_exit`; else `out.exit`.
pub fn replay(out: &JobOutput, write_failure_exit: u8) -> u8 {
    for action in &out.actions {
        let written = match action {
            WriteAction::Stdout(bytes) => write_stdout(bytes),
            WriteAction::Stderr(bytes) => write_stderr(bytes).map_err(|_| None),
            WriteAction::File(path, bytes) => write_file(path, bytes),
            WriteAction::Dir(path) => create_dir(path),
        };
        if let Err(failure) = written {
            if let Some((target, e)) = failure {
                write_error(&target, &e);
            }
            return write_failure_exit;
        }
    }
    out.exit
}

/// A failed write: its target and error, or `None` when stderr itself failed.
type Failure = Option<(String, std::io::Error)>;

/// Report a failed write of `target` on stderr.
fn write_error(target: &str, e: &std::io::Error) {
    let _ = write_stderr(format!("al-sem: write error: {target}: {e}\n").as_bytes());
}

fn write_stdout(bytes: &[u8]) -> Result<(), Failure> {
    let mut out = std::io::stdout().lock();
    out.write_all(bytes)
        .and_then(|()| out.flush())
        .map_err(|e| Some(("stdout".to_string(), e)))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|e| Some((path.display().to_string(), e)))
}

fn create_dir(path: &Path) -> Result<(), Failure> {
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|e| Some((path.display().to_string(), e)))
}

/// Write `bytes` to stderr and flush.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut err = std::io::stderr().lock();
    err.write_all(bytes).and_then(|()| err.flush())
}
