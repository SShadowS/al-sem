//! ENG-110: a persistent cache of parsed dependency IR (spec
//! `docs/superpowers/specs/2026-11-18-eng110-ir-cache-design.md`).
//!
//! Engine processes of one binary stop re-parsing dependency files: the first writes each
//! package's lowered IR to `<root>/ir/<image key>/<text digest>.bin`, later ones read, hash and
//! decode it. The key is the running image (`key`), the name ENG-113's verified text digest.
//! Only the `main`s of `alsem`, `aldump` and `al-call-hierarchy` install it
//! ([`install_from_env`]); tests and in-process binaries never see it.

pub(crate) mod codec;
pub(crate) mod counters;
pub(crate) mod envelope;
pub(crate) mod key;
pub(crate) mod pe;
pub(crate) mod prune;
pub(crate) mod report;
pub(crate) mod seam;
pub mod stamp;

pub use report::{REPORT_TAG, ReportGuard, report_end};

use counters::Counters;
use key::IrKey;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) use seam::parse_package;

/// The IR format, in every key and entry header. Bump it when the codec's bytes for an
/// unchanged IR change (the image key changes with any code change anyway).
pub(crate) const IR_FORMAT: u32 = 1;

/// One process's IR cache: where it lives, whether it reads and writes, and its counters.
pub struct IrCache {
    /// `false` (`ALSEM_IR_CACHE=off`): counts and reports only; every file `skipped_off`.
    enabled: bool,
    /// The folder holding the key folders (`<root>/ir`, or `ALSEM_IR_CACHE_DIR`).
    ir_dir: PathBuf,
    /// ENG-113's text cache directory, whose entries a fill parses.
    text_dir: PathBuf,
    /// `ALSEM_IR_CACHE_VERIFY=1`: every hit is also parsed and compared.
    verify: bool,
    report: Option<PathBuf>,
    key: OnceLock<Result<IrKey, key::NoKey>>,
    pub(crate) counters: Counters,
    started: AtomicBool,
    /// The report's last line for this process is written (`end`, or `no key`).
    ended: AtomicBool,
    /// Set at the first `last-used` publish of this process.
    touched: AtomicBool,
    warned: [AtomicBool; 4],
    /// An `ALSEM_IR_CACHE` value that is neither `on` nor `off`, warned at the first use (the
    /// logger is not up yet when a `main` installs the cache).
    bad_switch: Option<String>,
}

/// Whether the cache is on when `ALSEM_IR_CACHE` is unset or empty. **TLS-117: off on Windows
/// until the owner's acceptance of the Windows key passes** (positive keys on the shipped,
/// stamped binaries; the note's product acceptance); only an explicit `ALSEM_IR_CACHE=on` turns
/// it on there. One later commit flips this to `true` everywhere once that acceptance passes.
pub(crate) const DEFAULT_ON: bool = !cfg!(windows);

/// `ALSEM_IR_CACHE`: `on` or `off`, in any case; unset or empty is [`DEFAULT_ON`]. Anything else
/// is off, with the warning to give (sol/Fable r1: a typo never turns the cache on).
pub(crate) fn switch(v: Option<&std::ffi::OsStr>) -> (bool, Option<String>) {
    switch_with(v, DEFAULT_ON)
}

/// [`switch`] with the default given (tests check both defaults on any system).
pub(crate) fn switch_with(v: Option<&std::ffi::OsStr>, default_on: bool) -> (bool, Option<String>) {
    let Some(v) = v.filter(|v| !v.is_empty()) else {
        return (default_on, None);
    };
    match v.to_str().map(str::to_ascii_lowercase).as_deref() {
        Some("on") => (true, None),
        Some("off") => (false, None),
        _ => (
            false,
            Some(format!(
                "ALSEM_IR_CACHE={} is neither `on` nor `off`: the IR cache is off",
                v.to_string_lossy()
            )),
        ),
    }
}

/// Which one-time warning ([`IrCache::warn_once`]).
#[derive(Clone, Copy)]
pub(crate) enum Warn {
    RoundTrip = 0,
    Unwritable = 1,
    FolderFull = 2,
    NoKey = 3,
}

impl IrCache {
    pub(crate) fn new(
        enabled: bool,
        ir_dir: PathBuf,
        text_dir: PathBuf,
        verify: bool,
        report: Option<PathBuf>,
    ) -> Self {
        IrCache {
            enabled,
            ir_dir,
            text_dir,
            verify,
            report,
            key: OnceLock::new(),
            counters: Counters::default(),
            started: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            touched: AtomicBool::new(false),
            warned: Default::default(),
            bad_switch: None,
        }
    }

    /// A cache with its key given (tests: hashing the test binary per test is slow, and a
    /// test picks two keys at will).
    #[cfg(all(test, unix))]
    pub(crate) fn with_key(ir_dir: PathBuf, text_dir: PathBuf, verify: bool, key: IrKey) -> Self {
        let c = IrCache::new(true, ir_dir, text_dir, verify, None);
        let _ = c.key.set(Ok(key));
        c
    }

    /// The running image's key, hashed once, at the first use that needs it.
    pub(crate) fn key(&self) -> Option<IrKey> {
        self.key_result().as_ref().ok().copied()
    }

    /// Why there is no key (`None` when there is one).
    pub(crate) fn no_key(&self) -> Option<&key::NoKey> {
        self.key_result().as_ref().err()
    }

    fn key_result(&self) -> &Result<IrKey, key::NoKey> {
        // The warning is given at the first seam use (`first_use`): on Windows the key is made
        // in `main` before the logger is up.
        self.key.get_or_init(key::image_key)
    }

    pub(crate) fn warn_once(&self, w: Warn, msg: &str) {
        if !self.warned[w as usize].swap(true, Ordering::Relaxed) {
            log::warn!("{msg}");
        }
    }
}

static INSTALLED: OnceLock<IrCache> = OnceLock::new();
static SUSPENDED: AtomicUsize = AtomicUsize::new(0);

/// Install this process's cache from the environment: `ALSEM_IR_CACHE` (`on`, the default,
/// or `off`: count and report only), `ALSEM_IR_CACHE_DIR` (the `ir/` tree's folder),
/// `ALSEM_IR_CACHE_VERIFY=1`, `ALSEM_IR_CACHE_REPORT=<file>`. Called by the `main`s of
/// `alsem`, `aldump` and `al-call-hierarchy` only, never by `mint-goldens` or a test binary;
/// each holds the returned guard for its whole body, so every normal exit writes the report's
/// `end` line.
pub fn install_from_env() -> ReportGuard {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let (enabled, bad_switch) = switch(var("ALSEM_IR_CACHE").as_deref());
    // Paths only: nothing is made here (sol r1). A writer makes `ir/` at its first publish; a
    // cache-off or dependency-free run makes nothing, except `alsem batch`, whose check makes
    // the folders its caches would use before any job (TLS-116, `engine::batch::cache_guard`).
    let text_dir = crate::snapshot::cache::cache_dir_path();
    let ir_dir = var("ALSEM_IR_CACHE_DIR").map_or_else(|| text_dir.join("ir"), PathBuf::from);
    let verify = var("ALSEM_IR_CACHE_VERIFY").is_some_and(|v| v == "1");
    let report = var("ALSEM_IR_CACHE_REPORT").map(PathBuf::from);
    let mut cache = IrCache::new(enabled, ir_dir, text_dir, verify, report);
    cache.bad_switch = bad_switch;
    let _ = INSTALLED.set(cache);
    // TLS-117: on Windows the key is computed EAGERLY, here (aldump's and al-call-hierarchy's
    // first statement; in alsem's `main` after the perf-trace setup), so the time between the
    // image's load and its verification is milliseconds, not the hours a resident server could
    // wait before its first dependency parse. Commands that never use the cache skip it (they
    // would still get a lazy key at a first use). On Linux the key stays lazy (`/proc/self/exe`
    // is the running inode whenever it is read).
    #[cfg(windows)]
    if let Some(c) = INSTALLED.get().filter(|c| c.is_on())
        && eager_key_wanted(&std::env::args_os().collect::<Vec<_>>())
    {
        let _ = c.key();
    }
    ReportGuard::new()
}

/// Whether a process started with `args` (argv, the program first) may use the IR cache, so
/// its key is worth making at start (Fable N7): not for help or version output, the `mcp`
/// front end or `engine-ask` (their engine child keys itself). `alsem batch` uses the cache, in
/// the parent and every worker (TLS-116), so it keys. A cheap look at argv, never a parse: a
/// miss only makes the key lazy.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn eager_key_wanted(args: &[std::ffi::OsString]) -> bool {
    let sub = args.get(1).and_then(|a| a.to_str());
    !matches!(sub, Some("mcp" | "engine-ask" | "help"))
        && !args[1.min(args.len())..]
            .iter()
            .any(|a| matches!(a.to_str(), Some("--help" | "-h" | "--version" | "-V")))
}

/// The installed cache for the calling thread's seam use: `None` when none is installed, or
/// while a [`Suspend`] lives. A batch run uses it too (TLS-116): its write claims keep every
/// job write out of the cache folders ([`make_ir_dir`]).
pub(crate) fn installed() -> Option<&'static IrCache> {
    if SUSPENDED.load(Ordering::SeqCst) > 0 {
        return None;
    }
    INSTALLED.get()
}

/// Make `ir_dir` as a writer makes it: owner-only, its missing parents too. Best effort: a
/// failure leaves the folder missing or unusable, which a reader sees.
pub(crate) fn create_ir_dir(ir_dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(ir_dir);
    }
    // Windows (TLS-117): through the private walk: each component checked before anything is
    // made under it, so a refused parent gets no child (sol r1 P1-2).
    #[cfg(not(unix))]
    let _ = crate::snapshot::cache::create_cache_dir(ir_dir);
}

/// TLS-116: the installed IR cache's `ir/` folder, made now as its first writer would make it,
/// when this process would use it (installed, on, with a key); `None` otherwise. The caller
/// decides from what is there afterwards (`engine::batch::cache_guard`: a batch run's check,
/// which turns the cache off for the run when the folder is no directory).
pub fn make_ir_dir() -> Option<PathBuf> {
    let c = INSTALLED.get().filter(|c| c.is_on())?;
    c.key()?;
    create_ir_dir(&c.ir_dir);
    Some(c.ir_dir.clone())
}

/// Set when a batch run's snapshot turned the IR cache off for the run (TLS-116).
static BATCH_OFF: AtomicBool = AtomicBool::new(false);

/// TLS-116: turn the installed IR cache off for this process, as `ALSEM_IR_CACHE=off` does
/// (it still counts and reports): a batch run whose `ir/` folder is no directory after its
/// check made it, so the snapshot does not hold it.
pub fn disable() {
    BATCH_OFF.store(true, Ordering::SeqCst);
}

impl IrCache {
    /// Whether this cache reads and writes: on, and not turned off by a batch run.
    pub(crate) fn is_on(&self) -> bool {
        self.enabled && !BATCH_OFF.load(Ordering::SeqCst)
    }
}

/// TLS-116: the installed cache's report file (`ALSEM_IR_CACHE_REPORT`), which a batch run
/// registers as one of its writes.
pub fn report_path() -> Option<&'static std::path::Path> {
    INSTALLED.get()?.report.as_deref()
}

/// Turns the installed cache off while it lives (`aldump --session-refresh --verify`'s
/// cache-off reference build).
pub struct Suspend(());

impl Suspend {
    #[must_use]
    pub fn new() -> Self {
        SUSPENDED.fetch_add(1, Ordering::SeqCst);
        Suspend(())
    }
}

impl Default for Suspend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Suspend {
    fn drop(&mut self) {
        SUSPENDED.fetch_sub(1, Ordering::SeqCst);
    }
}
