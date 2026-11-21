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
#[cfg(unix)]
pub(crate) mod prune;
pub(crate) mod report;
pub(crate) mod seam;

pub use report::{ReportGuard, report_end};

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
    key: OnceLock<Option<IrKey>>,
    pub(crate) counters: Counters,
    started: AtomicBool,
    /// The report's last line for this process is written (`end`, or `no key`).
    ended: AtomicBool,
    /// Set at the first `last-used` publish of this process.
    touched: AtomicBool,
    warned: [AtomicBool; 4],
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
        }
    }

    /// A cache with its key given (tests: hashing the test binary per test is slow, and a
    /// test picks two keys at will).
    #[cfg(test)]
    pub(crate) fn with_key(ir_dir: PathBuf, text_dir: PathBuf, verify: bool, key: IrKey) -> Self {
        let c = IrCache::new(true, ir_dir, text_dir, verify, None);
        let _ = c.key.set(Some(key));
        c
    }

    /// The running image's key, hashed once, at the first use that needs it.
    pub(crate) fn key(&self) -> Option<IrKey> {
        *self.key.get_or_init(|| {
            let k = key::image_key();
            if k.is_none() {
                self.warn_once(
                    Warn::NoKey,
                    "IR cache: no image key (not Linux, unreadable or unmarked image): off",
                );
            }
            k
        })
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
    let enabled = var("ALSEM_IR_CACHE").is_none_or(|v| v != "off");
    let ir_dir = var("ALSEM_IR_CACHE_DIR").map_or_else(
        || crate::snapshot::cache::cache_dir().join("ir"),
        PathBuf::from,
    );
    let text_dir = crate::snapshot::cache::cache_dir();
    let verify = var("ALSEM_IR_CACHE_VERIFY").is_some_and(|v| v == "1");
    let report = var("ALSEM_IR_CACHE_REPORT").map(PathBuf::from);
    let _ = INSTALLED.set(IrCache::new(enabled, ir_dir, text_dir, verify, report));
    ReportGuard::new()
}

/// The installed cache for the calling thread's seam use: `None` when none is installed,
/// while a [`Suspend`] lives, or in a batch run (`alsem batch`, TLS-116: off).
pub(crate) fn installed() -> Option<&'static IrCache> {
    if SUSPENDED.load(Ordering::SeqCst) > 0 || crate::engine::batch::claim::dir().is_some() {
        return None;
    }
    INSTALLED.get()
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
