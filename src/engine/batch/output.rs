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

    /// The bytes its actions hold (the `batch.held` census, D6.3).
    pub fn bytes(&self) -> u64 {
        let len = |a: &WriteAction| match a {
            WriteAction::Stdout(b) | WriteAction::Stderr(b) => b.len(),
            WriteAction::File(p, b) => p.as_os_str().len() + b.len(),
            WriteAction::Dir(p) => p.as_os_str().len(),
        };
        self.actions.iter().map(|a| len(a) as u64).sum()
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
        &mut Streams {
            stdout: Stream(&mut stdout, "stdout"),
            stderr: Stream(&mut stderr, "stderr"),
        },
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

/// Where a job's writes go (context-server design r3 §10, §13 item 4): its
/// two streams, its files and directories, and the write error line of the
/// first failed write. [`Streams`] is the CLI's (and a batch job's): real
/// streams, the file system, the error line on the process's stderr. A
/// server's sink captures all of it, the error line included, so a failed
/// write reaches its answer instead of its own stderr.
pub trait Sink {
    fn stdout(&mut self, bytes: &[u8]) -> Result<(), Failure>;
    fn stderr(&mut self, bytes: &[u8]) -> Result<(), Failure>;
    fn file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure>;
    fn dir(&mut self, path: &Path) -> Result<(), Failure>;
    /// The failed write's line (`al-sem: write error: <target>: <message>`).
    fn write_error(&mut self, target: &str, e: &std::io::Error);
}

/// The CLI's sink: `stdout` and `stderr` (the process's, or a batch job's two
/// files, D2.1), files and directories on the file system, and a failed
/// write's line on the process's stderr (a failed stderr names itself, which
/// a closed stream drops).
pub struct Streams<'a> {
    pub stdout: Stream<'a>,
    pub stderr: Stream<'a>,
}

impl Sink for Streams<'_> {
    fn stdout(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.stdout.write(bytes)
    }
    fn stderr(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.stderr.write(bytes)
    }
    fn file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        write_file(path, bytes)
    }
    fn dir(&mut self, path: &Path) -> Result<(), Failure> {
        create_dir(path)
    }
    fn write_error(&mut self, target: &str, e: &std::io::Error) {
        write_error(target, e);
    }
}

/// [`replay`] into `sink`, in order. The first failed action ends the list
/// with the sink's write error line and returns `write_failure_exit`; else
/// `out.exit`. Returns the exit and every file and directory written, in
/// order.
pub fn replay_on(
    out: &JobOutput,
    write_failure_exit: u8,
    sink: &mut dyn Sink,
) -> (u8, Vec<PathBuf>) {
    let mut written = Vec::new();
    for action in &out.actions {
        let done = match action {
            WriteAction::Stdout(bytes) => sink.stdout(bytes),
            WriteAction::Stderr(bytes) => sink.stderr(bytes),
            WriteAction::File(path, bytes) => sink.file(path, bytes),
            WriteAction::Dir(path) => sink.dir(path),
        };
        match (done, action) {
            (Err((target, e)), _) => {
                sink.write_error(&target, &e);
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
pub type Failure = (String, std::io::Error);

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

thread_local! {
    /// The job lines printed by the driver step running on this thread, while
    /// the driver captures them (#277 D7).
    static JOB_LINES: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// A library job line (#277 D7): the L4 non-convergence line, graph's
/// coverage warning. Inside a driver step it goes to the driver, which
/// replays it in the stderr of each job whose standalone run prints it;
/// anywhere else (a test, `aldump`, a rayon worker: F11) it is printed.
pub fn job_line(line: String) {
    let unclaimed = JOB_LINES.with(|c| match c.borrow_mut().as_mut() {
        Some(lines) => {
            lines.push(line);
            None
        }
        None => Some(line),
    });
    if let Some(line) = unclaimed {
        eprintln!("{line}");
    }
}

/// `f`'s result and the job lines it printed on this thread.
pub(crate) fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    /// Restores the outer capture, also when `f` panics.
    struct Restore(Option<Option<Vec<String>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(outer) = self.0.take() {
                JOB_LINES.with(|c| *c.borrow_mut() = outer);
            }
        }
    }
    let mut restore = Restore(Some(JOB_LINES.with(|c| c.replace(Some(Vec::new())))));
    let t = f();
    let outer = restore.0.take().expect("restored once");
    let lines = JOB_LINES.with(|c| c.replace(outer)).unwrap_or_default();
    (t, lines)
}

/// Write `bytes` to stderr and flush.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut err = std::io::stderr().lock();
    err.write_all(bytes).and_then(|()| err.flush())
}

/// A sink that keeps every write in memory, the failed write's line in its
/// stderr: what a server answers with ([`crate::wire`]). Files are kept as
/// bytes; creating them is the server's output policy (stage 2), not this.
#[derive(Default, Debug)]
pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

impl Sink for Captured {
    fn stdout(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.stdout.extend_from_slice(bytes);
        Ok(())
    }
    fn stderr(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.stderr.extend_from_slice(bytes);
        Ok(())
    }
    fn file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        self.files.push((path.to_path_buf(), bytes.to_vec()));
        Ok(())
    }
    fn dir(&mut self, _: &Path) -> Result<(), Failure> {
        Ok(())
    }
    fn write_error(&mut self, target: &str, e: &std::io::Error) {
        self.stderr
            .extend_from_slice(format!("al-sem: write error: {target}: {e}\n").as_bytes());
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    /// [`Captured`] whose file writes fail.
    struct NoFiles(Captured);

    impl Sink for NoFiles {
        fn stdout(&mut self, bytes: &[u8]) -> Result<(), Failure> {
            self.0.stdout(bytes)
        }
        fn stderr(&mut self, bytes: &[u8]) -> Result<(), Failure> {
            self.0.stderr(bytes)
        }
        fn file(&mut self, path: &Path, _: &[u8]) -> Result<(), Failure> {
            Err((path.display().to_string(), std::io::Error::other("refused")))
        }
        fn dir(&mut self, path: &Path) -> Result<(), Failure> {
            self.0.dir(path)
        }
        fn write_error(&mut self, target: &str, e: &std::io::Error) {
            self.0.write_error(target, e);
        }
    }

    /// r3 §10: a failed write's line goes through the sink, so a capturing
    /// sink holds it (with the write-failure exit), and the writes after the
    /// failure are not made.
    #[test]
    fn a_failed_write_is_captured_by_the_sink() {
        let out = JobOutput {
            actions: vec![
                WriteAction::line("first"),
                WriteAction::File("o.json".into(), b"{}".to_vec()),
                WriteAction::Stdout(b"never".to_vec()),
            ],
            exit: 0,
        };
        let mut sink = NoFiles(Captured::default());
        let (exit, written) = replay_on(&out, 2, &mut sink);
        assert_eq!((exit, written), (2, vec![]));
        assert_eq!(
            String::from_utf8_lossy(&sink.0.stderr),
            "first\nal-sem: write error: o.json: refused\n"
        );
        assert!(sink.0.stdout.is_empty());
        // Everything succeeds: in order, files kept, the job's exit.
        let mut all = Captured::default();
        assert_eq!(replay_on(&out, 2, &mut all).0, 0);
        assert_eq!(all.stdout, b"never");
        assert_eq!(all.files, [(PathBuf::from("o.json"), b"{}".to_vec())]);
    }
}
