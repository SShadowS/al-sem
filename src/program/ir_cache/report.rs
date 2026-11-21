//! The report file (`ALSEM_IR_CACHE_REPORT=<file>`, spec "Correctness checks"): a `start` line
//! at a process's first seam use and an `end` line with its counters at its exit, each one
//! `write` on an `O_APPEND` file, so concurrent processes never interleave a line. Off Linux,
//! or without a key, one `{"pid", "ir_cache": "unsupported" | "no key"}` line instead. A batch
//! run writes nothing (the seam never sees the cache there).

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
    let written = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
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
        let command: Vec<String> = std::env::args().take(4).collect();
        append(
            self,
            &serde_json::json!({
                "event": "start",
                "pid": pid,
                "command": command.join(" "),
                "enabled": self.enabled,
                "key": key.map(IrKey::hex),
            }),
        );
    }
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
    debug_assert!(
        c.counters.balanced(),
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
