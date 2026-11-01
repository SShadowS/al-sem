//! One request's planning context (context-server design r3 §4.3 and §13,
//! stage 0; review r2 #16): everything a command's planning reads from
//! outside its argv, owned by the request instead of the process.
//!
//! Planning used to read three process globals of the `alsem` binary: the
//! stdin text a `--diff -` reads (a `OnceLock`, so every later request of the
//! process got the FIRST request's diff), the plan's file reads and `exists`
//! answers (one map for the process), and the paths the plan being made
//! touched (one list, so two plans made at once mixed their paths). It also
//! decided the diff root itself (`metadata`, `canonicalize`, `.git` probes).
//!
//! The order a request is planned in (r3 §4.3): **decode** the argv (pure:
//! clap); choose the workspace read that will answer (a resident server pins
//! a revision; a CLI process reads its own); **decide** the request's
//! file-system questions in the caller (the diff root, `digest --changed`'s
//! file-or-list) through this context's [`PlanFs`]; **construct** the
//! [`PlanCx`]; then **plan** (`make_job`: validate, read the request's files
//! through the context, never probe the file system), resolve and compute
//! against that same read.
//!
//! [`PlanReads`] is what several requests of ONE run share: the stdin read
//! (a batch's `--diff -` jobs all get the run's one read, #277 D3) and each
//! file read once (#277 final r2 R2-1). A [`PlanCx`] is one request's: its
//! touched paths, its diff-root decision and, when its caller holds one, the
//! workspace read its job uses instead of reading the workspace itself.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::worker::{PlanFiles, StdinRead};
use crate::engine::l5::digest_cli::DiffRoot;
use crate::program::inputs::WorkspaceInputs;

/// The file-system questions planning asks. [`ProcessFs`] answers them from
/// the process (the CLI); a server answers from what its request captured.
pub trait PlanFs: Send + Sync {
    /// The bytes at `path`.
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> bool;
    /// `path` made canonical (symlinks resolved).
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// Whether `path` is a folder (`Err`: its metadata could not be read).
    fn is_dir(&self, path: &Path) -> std::io::Result<bool>;
    /// `path` made absolute against the current directory, not resolved.
    fn absolute(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// The whole of stdin, read once; `None`: the read failed.
    fn read_stdin(&self) -> Option<String>;
}

/// The process's own answers: the file system now, and the real stdin.
pub struct ProcessFs;

/// How many times [`ProcessFs`] read the process's stdin: the test seam that
/// shows a request planned with given stdin text never reads the real one.
pub static PROCESS_STDIN_READS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl PlanFs for ProcessFs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn is_dir(&self, path: &Path) -> std::io::Result<bool> {
        std::fs::metadata(path).map(|m| m.is_dir())
    }

    fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::path::absolute(path)
    }

    fn read_stdin(&self) -> Option<String> {
        use std::io::Read;
        PROCESS_STDIN_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok().map(|_| buf)
    }
}

/// What the requests of one run share: the file system they ask, the stdin
/// read (given, or made once on the first ask) and each file read and
/// `exists` answer, made once.
pub struct PlanReads {
    fs: Box<dyn PlanFs>,
    stdin: OnceLock<Option<String>>,
    files: Mutex<PlanFiles>,
}

impl PlanReads {
    /// The CLI's: the process's file system, stdin read on the first
    /// `--diff -`.
    #[must_use]
    pub fn process() -> Arc<PlanReads> {
        PlanReads::new(Box::new(ProcessFs), None, PlanFiles::default())
    }

    /// A batch worker's: its parent's stdin read (when a job made one) and
    /// file reads, so it neither reads its own stdin (its batch input) nor a
    /// file again.
    #[must_use]
    pub fn seeded(stdin: Option<StdinRead>, files: PlanFiles) -> Arc<PlanReads> {
        PlanReads::new(Box::new(ProcessFs), stdin, files)
    }

    /// A request's: its stdin is `stdin` (the diff text it carries; `None`: it
    /// carries none, and a `--diff -` fails as an unreadable stdin does), its
    /// file-system questions `fs` answers. The process's stdin is never read.
    #[must_use]
    pub fn request(stdin: Option<String>, fs: Box<dyn PlanFs>) -> Arc<PlanReads> {
        let read = match stdin {
            Some(text) => StdinRead::Text(text),
            None => StdinRead::Failed,
        };
        PlanReads::new(fs, Some(read), PlanFiles::default())
    }

    fn new(fs: Box<dyn PlanFs>, stdin: Option<StdinRead>, files: PlanFiles) -> Arc<PlanReads> {
        let cell = OnceLock::new();
        if let Some(read) = stdin {
            let _ = cell.set(match read {
                StdinRead::Text(text) => Some(text),
                StdinRead::Failed => None,
            });
        }
        Arc::new(PlanReads {
            fs,
            stdin: cell,
            files: Mutex::new(files),
        })
    }

    /// The stdin read made so far, if any: what a batch's parent hands its
    /// workers.
    #[must_use]
    pub fn stdin_read(&self) -> Option<StdinRead> {
        self.stdin.get().map(|read| match read {
            Some(text) => StdinRead::Text(text.clone()),
            None => StdinRead::Failed,
        })
    }

    /// Every file read and `exists` answer so far: what a batch's parent
    /// hands its workers.
    #[must_use]
    pub fn files(&self) -> PlanFiles {
        self.files.lock().expect("plan files").clone()
    }
}

/// One request's planning context: the run's shared reads, the paths this
/// request's planning touched, its diff-root decision (made by the caller,
/// before planning) and, when the caller holds one, the workspace read its
/// job uses. `Sync`: two requests planned at once each hold their own.
pub struct PlanCx {
    reads: Arc<PlanReads>,
    touched: Mutex<Vec<PathBuf>>,
    /// `digest` / `explain`: what the changed paths are relative to, decided
    /// by the caller (`decide_diff_root` through this context, or shipped by
    /// a batch's parent or a front end); `None` for every other command.
    /// Planning never decides it itself.
    pub diff_root_decided: Option<Result<DiffRoot, String>>,
    /// The workspace read the request's job uses (a resident revision's);
    /// `None`: the job reads its workspace through the run's reader, as the
    /// CLI always has. An argv naming another workspace, or needing more than
    /// the read holds, is refused at planning.
    pub inputs: Option<Arc<WorkspaceInputs>>,
}

impl PlanCx {
    /// A request of the run whose shared reads are `reads`.
    #[must_use]
    pub fn new(reads: &Arc<PlanReads>) -> PlanCx {
        PlanCx {
            reads: Arc::clone(reads),
            touched: Mutex::new(Vec::new()),
            diff_root_decided: None,
            inputs: None,
        }
    }

    /// The stdin text, read on the first ask of the run (`None`: the read
    /// failed). Every `--diff -` of the run gets the same text (#277 D3).
    pub fn stdin_text(&self) -> Option<&str> {
        let reads = &*self.reads;
        reads.stdin.get_or_init(|| reads.fs.read_stdin()).as_deref()
    }

    /// Record that this request's planning probed `path`, whatever the
    /// outcome.
    pub fn touch(&self, path: &Path) {
        self.touched
            .lock()
            .expect("plan touches")
            .push(path.to_path_buf());
    }

    /// The paths this request touched so far, in order; the list is left
    /// empty (a batch's parent adds them to the job's reads, D2.4).
    pub fn take_touched(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.touched.lock().expect("plan touches"))
    }

    /// `path`'s bytes as a plan reads them: read once per run. The error
    /// keeps its text, not its kind.
    pub fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.touch(path);
        let mut files = self.reads.files.lock().expect("plan files");
        let read = files
            .reads
            .entry(path.to_path_buf())
            .or_insert_with(|| self.reads.fs.read(path).map_err(|e| e.to_string()));
        read.clone().map_err(std::io::Error::other)
    }

    /// Whether `path` exists, as a plan asks it: answered once per run.
    pub fn exists(&self, path: &Path) -> bool {
        self.touch(path);
        let mut files = self.reads.files.lock().expect("plan files");
        *files
            .exists
            .entry(path.to_path_buf())
            .or_insert_with(|| self.reads.fs.exists(path))
    }

    /// Whether `path` exists, asked of the file system now and neither
    /// recorded nor kept: `digest --changed`'s file-or-list decision, which
    /// the caller pins once on the decoded command (`changed_is_file`).
    pub fn probe_exists(&self, path: &Path) -> bool {
        self.reads.fs.exists(path)
    }

    /// `path` made canonical, through the run's file system.
    pub fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.reads.fs.canonicalize(path)
    }

    /// Whether `path` is a folder, through the run's file system.
    pub fn is_dir(&self, path: &Path) -> std::io::Result<bool> {
        self.reads.fs.is_dir(path)
    }

    /// `path` made absolute, through the run's file system.
    pub fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.reads.fs.absolute(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file system with no file at all: every question has a fixed answer.
    struct Empty;

    impl PlanFs for Empty {
        fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
            Ok(path.to_string_lossy().into_owned().into_bytes())
        }
        fn exists(&self, _: &Path) -> bool {
            false
        }
        fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
            Ok(path.to_path_buf())
        }
        fn is_dir(&self, _: &Path) -> std::io::Result<bool> {
            Ok(false)
        }
        fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
            Ok(path.to_path_buf())
        }
        fn read_stdin(&self) -> Option<String> {
            panic!("a request context never reads stdin")
        }
    }

    /// Two requests made one after the other, each with its own stdin text,
    /// answer their own text: the old process `OnceLock` answered the first
    /// text to both.
    #[test]
    fn two_requests_get_their_own_stdin() {
        let a = PlanCx::new(&PlanReads::request(Some("diff A".into()), Box::new(Empty)));
        assert_eq!(a.stdin_text(), Some("diff A"));
        let b = PlanCx::new(&PlanReads::request(Some("diff B".into()), Box::new(Empty)));
        assert_eq!(b.stdin_text(), Some("diff B"));
        let none = PlanCx::new(&PlanReads::request(None, Box::new(Empty)));
        assert_eq!(none.stdin_text(), None);
    }

    /// Touches recorded by two requests of ONE run on two threads at once
    /// stay apart, while their file reads are shared.
    #[test]
    fn concurrent_requests_keep_their_touches() {
        let reads = PlanReads::request(None, Box::new(Empty));
        let barrier = std::sync::Barrier::new(2);
        let run = |tag: &str| {
            let cx = PlanCx::new(&reads);
            barrier.wait();
            for i in 0..200 {
                let _ = cx.exists(Path::new(&format!("{tag}/{i}")));
            }
            cx.take_touched()
        };
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| run("a"));
            let b = s.spawn(|| run("b"));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!(a.len(), 200);
        assert_eq!(b.len(), 200);
        assert!(a.iter().all(|p| p.starts_with("a")), "{a:?}");
        assert!(b.iter().all(|p| p.starts_with("b")), "{b:?}");
        assert_eq!(reads.files().exists.len(), 400, "one run, one answer map");
    }
}
