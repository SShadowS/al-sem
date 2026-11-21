//! The report file (`ALSEM_IR_CACHE_REPORT=<file>`, spec "Correctness checks"): a `start` line
//! at a process's first seam use and an `end` line with its counters at its exit, each one
//! `write` on an `O_APPEND` file, so concurrent processes never interleave a line. Off Linux,
//! or without a key, one `{"pid", "ir_cache": "unsupported" | "no key"}` line instead. In a
//! batch run (TLS-116) the parent registers the file as one of the run's writes and every
//! process of the run appends only to that file ([`crate::engine::batch::claim::open_shared`]).

/// The report's tag in a batch run's claims.
pub const REPORT_TAG: &str = "ir-cache-report";

use super::IrCache;
use super::key::IrKey;
use std::io::Write;
use std::sync::atomic::Ordering;

/// Held for a `main`'s whole body ([`super::install_from_env`]): its drop writes the `end`
/// line when a `start` was written.
#[must_use = "hold it for main's whole body: its drop writes the report's end line"]
pub struct ReportGuard(());

impl ReportGuard {
    pub(crate) fn new() -> Self {
        ReportGuard(())
    }
}

impl Drop for ReportGuard {
    fn drop(&mut self) {
        report_end();
    }
}

fn append(c: &IrCache, v: &serde_json::Value) {
    let Some(path) = &c.report else {
        return;
    };
    let mut line = v.to_string();
    line.push('\n');
    let written = crate::engine::batch::claim::open_shared(path, REPORT_TAG)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        log::warn!("IR cache report {} not written ({e})", path.display());
    }
}

impl IrCache {
    /// The first seam use of this process: the `start` line (or the one `unsupported` /
    /// `no key` line). `key` is `None` when the cache is off (`ALSEM_IR_CACHE=off`) or the
    /// image has no key.
    pub(crate) fn first_use(&self, key: Option<&IrKey>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(w) = &self.bad_switch {
            log::warn!("{w}");
        }
        // No sink: nothing to format (sol r1: reading the command line here once panicked on
        // a non-UTF-8 argument, cache off or not).
        if self.report.is_none() {
            return;
        }
        let pid = std::process::id();
        if self.enabled && key.is_none() {
            let why = if cfg!(target_os = "linux") {
                "no key"
            } else {
                "unsupported"
            };
            self.ended.store(true, Ordering::SeqCst); // no end line follows this one
            append(self, &serde_json::json!({"pid": pid, "ir_cache": why}));
            return;
        }
        append(
            self,
            &serde_json::json!({
                "event": "start",
                "pid": pid,
                "argv": argv_json(std::env::args_os()),
                "enabled": self.enabled,
                "key": key.map(IrKey::hex),
            }),
        );
    }
}

/// The whole command line, losslessly (Fable/sol r2: a run's mode, `--low-mem` say, must be
/// provable from its report): each argument as its string when it is UTF-8, else as
/// `{"hex": "<its bytes>"}`.
fn argv_json(args: impl Iterator<Item = std::ffi::OsString>) -> serde_json::Value {
    args.map(|a| match a.into_string() {
        Ok(s) => serde_json::Value::String(s),
        Err(raw) => {
            #[cfg(unix)]
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(raw.as_os_str()).to_vec();
            #[cfg(not(unix))]
            let bytes = raw.to_string_lossy().into_owned().into_bytes();
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            serde_json::json!({ "hex": hex })
        }
    })
    .collect()
}

/// Write the installed cache's `end` line, once, when its `start` line was written (a
/// `main`'s exit: [`ReportGuard`]'s drop, or an exit that skips it, such as
/// `perf_trace`'s `exit_after`).
pub fn report_end() {
    let Some(c) = super::INSTALLED.get() else {
        return;
    };
    if !c.started.load(Ordering::SeqCst) || c.ended.swap(true, Ordering::SeqCst) {
        return;
    }
    // Not while unwinding: a panic in `Drop` during an unwind aborts the process (Fable r1),
    // and the counts of an interrupted run need not balance.
    debug_assert!(
        std::thread::panicking() || c.counters.balanced(),
        "IR cache counters do not balance: {}",
        c.counters.snapshot()
    );
    append(
        c,
        &serde_json::json!({
            "event": "end",
            "pid": std::process::id(),
            "counters": c.counters.snapshot(),
        }),
    );
}
