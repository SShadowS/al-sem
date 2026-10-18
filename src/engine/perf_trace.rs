//! Permanent, env-gated performance tracing (spec `docs/superpowers/specs/2026-07-18-tracing-infra.md`).
//!
//! This is a bespoke tracer (NOT the `tracing` crate): the measurement waves
//! need exactly five primitives, deterministic Chrome-Trace output, Windows
//! RSS via direct FFI, and crash-safe checkpointing — a subscriber ecosystem
//! buys none of that and adds a process-global surface. Fully separate from
//! product telemetry: a local side file only, no upload, no source text.
//!
//! ## Disabled-path contract
//! `ALSEM_TRACE` absent / `0` / `off`: a single `OnceLock` test at every coarse
//! site returns `None`; no files, no clocks, no K32 calls, no JSON, no
//! allocation. `enabled(Detail)` is the one cheap gate.
//!
//! ## Crash safety
//! The trace file is kept as an ALWAYS-CLOSED JSON array: creation writes `[]`,
//! and each append seeks over the trailing `]`, writes the event (plus a
//! leading comma after the first) and a fresh `]`, then flushes. So the `B`
//! event is durable the instant a span opens, and a cap-killed / crashed run
//! leaves a file that still parses with the active span visible. Trace-write
//! failures fail OPEN — the writer is dropped and analysis proceeds untouched.
//!
//! ## Threading
//! Spans may close on a different thread than they opened (rayon in the
//! pipeline), so the writer lives behind a `Mutex` and every span captures a
//! stable synthetic tid at open time and re-uses it at close, keeping the
//! `B`/`E` pair matched. This is coarse-event-only by design; hot loops use
//! [`LocalCounters`] (plain `u64`, flushed once) so there is never per-node
//! lock contention.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use serde_json::{Value, json};

const MB: u64 = 1_048_576;

// ---------------------------------------------------------------------------
// Detail tiers (cumulative)
// ---------------------------------------------------------------------------

/// Cumulative verbosity tiers. `Stages` < `Hot`; a configured tier enables every
/// tier at or below it (`enabled(Detail::Stages)` is true whenever tracing is on
/// at all). (The intermediate `Jacobi` tier was retired with the old Jacobi
/// db-effects solver — its only instrumentation lived inside that solver.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    Stages,
    Hot,
}

impl Detail {
    fn parse(s: &str) -> Detail {
        match s.trim().to_ascii_lowercase().as_str() {
            "hot" => Detail::Hot,
            _ => Detail::Stages,
        }
    }
}

// ---------------------------------------------------------------------------
// Config (pure parse — no global state, unit-testable in isolation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Config {
    detail: Detail,
    /// Explicit `ALSEM_TRACE_FILE`; `None` means "derive `alsem-trace-<pid>.json`".
    file: Option<PathBuf>,
    sample_every: u64,
    scc_min: u64,
    stderr: bool,
    exit_after: Option<String>,
}

impl Config {
    /// Parse a config from an env reader. Returns `None` (tracing disabled) when
    /// `ALSEM_TRACE` is absent, `0`, `off`, or anything other than `1`/`chrome`.
    /// Pure: no global state, no I/O — this is the unit-test seam.
    fn from_reader(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
        let raw = get("ALSEM_TRACE").unwrap_or_default();
        let on = matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "chrome");
        if !on {
            return None;
        }
        let detail = get("ALSEM_TRACE_DETAIL")
            .map(|s| Detail::parse(&s))
            .unwrap_or(Detail::Stages);
        let file = get("ALSEM_TRACE_FILE")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        let sample_every = parse_u64_or(get("ALSEM_TRACE_SAMPLE_EVERY"), 64);
        let scc_min = parse_u64_or(get("ALSEM_TRACE_SCC_MIN"), 100);
        let stderr = get("ALSEM_TRACE_STDERR").as_deref().map(str::trim) == Some("1");
        let exit_after = get("ALSEM_TRACE_EXIT_AFTER").filter(|s| !s.trim().is_empty());
        Some(Config {
            detail,
            file,
            sample_every,
            scc_min,
            stderr,
            exit_after,
        })
    }
}

fn parse_u64_or(v: Option<String>, default: u64) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// RSS via direct K32 FFI (PROCESS_MEMORY_COUNTERS_EX)
// ---------------------------------------------------------------------------

/// A two-instant RSS observation (bytes). Documented as an OS snapshot, not
/// live-allocation accounting.
#[derive(Debug, Clone, Copy)]
struct Rss {
    working_set: u64,
    peak_working_set: u64,
    private_usage: u64,
}

#[cfg(windows)]
fn read_rss() -> Option<Rss> {
    // PROCESS_MEMORY_COUNTERS_EX — same layout as PROCESS_MEMORY_COUNTERS plus a
    // trailing PrivateUsage (SIZE_T). `cb` is set to size_of this EX struct so
    // K32GetProcessMemoryInfo fills PrivateUsage too.
    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    // Rust 2024: extern blocks must be marked `unsafe`.
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCountersEx,
            cb: u32,
        ) -> i32;
    }
    unsafe {
        let mut c: ProcessMemoryCountersEx = std::mem::zeroed();
        c.cb = std::mem::size_of::<ProcessMemoryCountersEx>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            Some(Rss {
                working_set: c.working_set_size as u64,
                peak_working_set: c.peak_working_set_size as u64,
                private_usage: c.private_usage as u64,
            })
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
fn read_rss() -> Option<Rss> {
    None
}

// ---------------------------------------------------------------------------
// Chrome-Trace writer — always-closed JSON array, flush per event
// ---------------------------------------------------------------------------

struct TraceWriter {
    /// `None` once the writer has failed (fail-open) or was never opened.
    out: Option<BufWriter<File>>,
    first: bool,
}

impl TraceWriter {
    fn create(path: &std::path::Path) -> std::io::Result<TraceWriter> {
        let file = File::create(path)?;
        let mut out = BufWriter::new(file);
        out.write_all(b"[]")?;
        out.flush()?;
        Ok(TraceWriter {
            out: Some(out),
            first: true,
        })
    }

    /// A no-op writer (file open failed): keeps the tracer alive so the stderr
    /// mirror still works, but drops every event.
    fn dead() -> TraceWriter {
        TraceWriter {
            out: None,
            first: true,
        }
    }

    /// Append one event, keeping the on-disk file a valid closed array at all
    /// times. Seeks over the trailing `]`, writes `[,]event]`, flushes. Any I/O
    /// error fails OPEN: the underlying file is dropped and later events no-op.
    fn append(&mut self, ev: &Value) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let payload = ev.to_string();
        let first = self.first;
        let res = (|| -> std::io::Result<()> {
            out.seek(SeekFrom::End(-1))?; // position over the closing ']'
            if !first {
                out.write_all(b",")?;
            }
            out.write_all(payload.as_bytes())?;
            out.write_all(b"]")?;
            out.flush()?;
            Ok(())
        })();
        match res {
            Ok(()) => self.first = false,
            Err(_) => self.out = None, // fail open — stop touching the file
        }
    }

    fn flush(&mut self) {
        if let Some(out) = self.out.as_mut() {
            let _ = out.flush();
        }
    }
}

// ---------------------------------------------------------------------------
// Tracer — the process-global state, built once from env
// ---------------------------------------------------------------------------

struct Tracer {
    config: Config,
    start: Instant,
    pid: u32,
    writer: Mutex<TraceWriter>,
    counters: Mutex<HashMap<&'static str, u64>>,
    /// The running phase-peak sampler ([`mark_phase_start`]), if any.
    phase: Mutex<Option<PhaseSampler>>,
}

/// A background thread that samples the working set every 10 ms into `max`
/// (bytes) until `stop` is set (#160 spec §4.1.4).
struct PhaseSampler {
    max: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl PhaseSampler {
    fn start() -> PhaseSampler {
        let max = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        // One synchronous sample, so a phase shorter than one interval is never 0.
        sample_working_set(&max);
        let (m, s) = (Arc::clone(&max), Arc::clone(&stop));
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                sample_working_set(&m);
            }
        });
        PhaseSampler { max, stop, handle }
    }

    /// Stop and join the thread, take one last synchronous sample, return the
    /// phase maximum in bytes.
    fn finish(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
        sample_working_set(&self.max);
        self.max.load(Ordering::Relaxed)
    }
}

fn sample_working_set(max: &AtomicU64) {
    if let Some(r) = read_rss() {
        max.fetch_max(r.working_set, Ordering::Relaxed);
    }
}

impl Tracer {
    fn init(config: Config) -> Tracer {
        let pid = std::process::id();
        let path = config
            .file
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("alsem-trace-{pid}.json")));
        let writer = match TraceWriter::create(&path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!(
                    "perf_trace: cannot create {}: {e}; file tracing disabled",
                    path.display()
                );
                TraceWriter::dead()
            }
        };
        Tracer {
            config,
            start: Instant::now(),
            pid,
            writer: Mutex::new(writer),
            counters: Mutex::new(HashMap::new()),
            phase: Mutex::new(None),
        }
    }

    /// Stop the phase sampler (if one runs) and return its maximum in bytes.
    fn finish_phase(&self) -> u64 {
        let sampler = self.phase.lock().ok().and_then(|mut p| p.take());
        sampler.map_or(0, PhaseSampler::finish)
    }

    fn ts(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn append(&self, ev: &Value) {
        if let Ok(mut w) = self.writer.lock() {
            w.append(ev);
        }
    }

    fn emit_process_meta(&self, run_name: &str) {
        self.append(&json!({
            "ph": "M", "name": "process_name",
            "pid": self.pid, "tid": synthetic_tid(),
            "args": { "name": run_name }
        }));
    }

    fn emit_begin(&self, cat: &'static str, name: &str, tid: u32) {
        self.append(&json!({
            "ph": "B", "cat": cat, "name": name,
            "pid": self.pid, "tid": tid, "ts": self.ts()
        }));
    }

    fn emit_end(
        &self,
        cat: &'static str,
        name: &str,
        tid: u32,
        begin: Option<Rss>,
        end: Option<Rss>,
    ) {
        let mut args = serde_json::Map::new();
        if let Some(r) = end {
            args.insert("rss_mb".into(), (r.working_set / MB).into());
            args.insert("peak_mb".into(), (r.peak_working_set / MB).into());
            args.insert("private_mb".into(), (r.private_usage / MB).into());
            if let Some(b) = begin {
                let delta = (r.working_set as i64 - b.working_set as i64) / MB as i64;
                args.insert("rss_delta_mb".into(), delta.into());
            }
        }
        self.append(&json!({
            "ph": "E", "cat": cat, "name": name,
            "pid": self.pid, "tid": tid, "ts": self.ts(),
            "args": Value::Object(args)
        }));
    }

    fn emit_instant(&self, cat: &'static str, name: &'static str, val: Value) {
        self.append(&json!({
            "ph": "i", "cat": cat, "name": name,
            "pid": self.pid, "tid": synthetic_tid(), "ts": self.ts(),
            "s": "t", "args": val
        }));
    }

    fn emit_counter(&self, name: &'static str, args: Value) {
        self.append(&json!({
            "ph": "C", "name": name,
            "pid": self.pid, "tid": synthetic_tid(), "ts": self.ts(),
            "args": args
        }));
    }

    fn set_counter(&self, name: &'static str, v: u64) {
        if let Ok(mut m) = self.counters.lock() {
            m.insert(name, v);
        }
        self.emit_counter(name, json!({ name: v }));
    }

    fn add_counter(&self, name: &'static str, dv: u64) {
        let mut total = dv;
        if let Ok(mut m) = self.counters.lock() {
            let e = m.entry(name).or_insert(0);
            *e += dv;
            total = *e;
        }
        self.emit_counter(name, json!({ name: total }));
    }

    /// Open a span: capture a stable tid + begin RSS, write `B` immediately, and
    /// (opt-in) mirror a Wave-1 `STAGE …` line to stderr.
    fn open(&self, cat: &'static str, name: String) -> ActiveSpan {
        let tid = synthetic_tid();
        let rss_begin = read_rss();
        self.emit_begin(cat, &name, tid);
        if self.config.stderr {
            let (ws, peak) = rss_begin
                .map(|r| (r.working_set / MB, r.peak_working_set / MB))
                .unwrap_or((0, 0));
            eprintln!(
                "STAGE {name} t={:.2}s rss_mb={ws} peak_mb={peak}",
                self.start.elapsed().as_secs_f64()
            );
        }
        ActiveSpan {
            cat,
            name,
            tid,
            rss_begin,
        }
    }

    fn close(&self, s: &ActiveSpan) {
        let end = read_rss();
        self.emit_end(s.cat, &s.name, s.tid, s.rss_begin, end);
    }
}

// One synthetic tid per OS thread, assigned lazily and monotonically. Keeps the
// timeline stable and lets a span opened on thread A close on thread B while
// still pairing its B/E under one tid.
static NEXT_TID: AtomicU32 = AtomicU32::new(1);
thread_local! {
    static TID: u32 = NEXT_TID.fetch_add(1, Ordering::Relaxed);
}
fn synthetic_tid() -> u32 {
    TID.with(|t| *t)
}

fn tracer() -> Option<&'static Tracer> {
    static TRACER: OnceLock<Option<Tracer>> = OnceLock::new();
    TRACER
        .get_or_init(|| Config::from_reader(|k| std::env::var(k).ok()).map(Tracer::init))
        .as_ref()
}

// ---------------------------------------------------------------------------
// Span identity + guards
// ---------------------------------------------------------------------------

/// A span name: a `&'static str` (the common coarse-site case, no alloc) or an
/// owned `String` (dynamic names like `detector.<name>` or per-SCC spans).
pub enum TraceName {
    Static(&'static str),
    Owned(String),
}

impl TraceName {
    fn into_string(self) -> String {
        match self {
            TraceName::Static(s) => s.to_string(),
            TraceName::Owned(s) => s,
        }
    }
}

impl From<&'static str> for TraceName {
    fn from(s: &'static str) -> Self {
        TraceName::Static(s)
    }
}

impl From<String> for TraceName {
    fn from(s: String) -> Self {
        TraceName::Owned(s)
    }
}

struct ActiveSpan {
    cat: &'static str,
    name: String,
    tid: u32,
    rss_begin: Option<Rss>,
}

/// Top-level process span (from [`run`]). Its `Drop` closes the span.
pub struct RunGuard {
    span: Option<ActiveSpan>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Some(s) = self.span.take()
            && let Some(t) = tracer()
        {
            // #160 §4.1.4: the phase peak and the OS lifetime peak, before the
            // run span closes. Both are 0 where `read_rss()` is `None`.
            let post_attach = t.finish_phase() / MB;
            let process = read_rss().map_or(0, |r| r.peak_working_set / MB);
            t.set_counter("peak.process_mb", process);
            t.set_counter("peak.post_attach_mb", post_attach);
            if t.config.stderr {
                eprintln!(
                    "[perf-trace] peak.process_mb={process} peak.post_attach_mb={post_attach}"
                );
            }
            t.close(&s);
        }
    }
}

/// A nested timeline span (from [`span`]). Its `Drop` writes the `E` event with
/// RSS args, then honors `ALSEM_TRACE_EXIT_AFTER`.
pub struct SpanGuard {
    span: Option<ActiveSpan>,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if let Some(s) = self.span.take() {
            if let Some(t) = tracer() {
                t.close(&s);
            }
            maybe_exit_after(&s.name);
        }
    }
}

// ---------------------------------------------------------------------------
// Local (hot-loop) counters — plain u64, flushed once
// ---------------------------------------------------------------------------

/// A cheap, lock-free accumulator for hot loops. Increment plain `u64` fields
/// (or `add`/`set` named slots) with NO atomics, clocks, or per-node
/// allocation, then `flush(category)` once at scope end to emit a single
/// multi-series counter event. The hot-loop rule: test `enabled(Detail::Hot)`
/// ONCE outside the loop and thread `Option<&mut LocalCounters>` down.
#[derive(Debug, Default)]
pub struct LocalCounters {
    entries: Vec<(&'static str, u64)>,
}

impl LocalCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `dv` to the named slot (creating it at 0 on first touch).
    pub fn add(&mut self, name: &'static str, dv: u64) {
        for (n, v) in &mut self.entries {
            if *n == name {
                *v += dv;
                return;
            }
        }
        self.entries.push((name, dv));
    }

    /// Set the named slot to `v` (e.g. carrying a plain stack `u64` out at end).
    pub fn set(&mut self, name: &'static str, v: u64) {
        for (n, slot) in &mut self.entries {
            if *n == name {
                *slot = v;
                return;
            }
        }
        self.entries.push((name, v));
    }

    pub fn get(&self, name: &str) -> u64 {
        self.entries
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(0, |(_, v)| *v)
    }

    /// Emit all slots as one Chrome `C` counter event named `category`.
    pub fn flush(self, category: &'static str) {
        if self.entries.is_empty() {
            return;
        }
        if let Some(t) = tracer() {
            let mut args = serde_json::Map::new();
            for (n, v) in &self.entries {
                args.insert((*n).to_string(), (*v).into());
            }
            t.emit_counter(category, Value::Object(args));
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// True when tracing is enabled at or above `d`'s tier. The one cheap gate:
/// a single `OnceLock` read on the disabled path.
pub fn enabled(d: Detail) -> bool {
    tracer().is_some_and(|t| t.config.detail >= d)
}

/// Per-walk timing sample rate (`ALSEM_TRACE_SAMPLE_EVERY`, default 64). Read by
/// the Detail::Hot walk instrumentation to sample expensive per-walk clocks.
pub fn sample_every() -> u64 {
    tracer().map_or(64, |t| t.config.sample_every)
}

/// Per-SCC detail threshold (`ALSEM_TRACE_SCC_MIN`, default 100). A generic
/// tracing knob: per-SCC instrumentation (when added) emits one span per SCC at
/// or above this size. (Its previous reader, the old Jacobi solver's per-SCC
/// span, retired with that solver.)
pub fn scc_min() -> u64 {
    tracer().map_or(100, |t| t.config.scc_min)
}

/// Open the top-level process span. Writes process metadata + a `B` event; the
/// returned guard closes it on drop.
pub fn run(name: &'static str) -> RunGuard {
    let Some(t) = tracer() else {
        return RunGuard { span: None };
    };
    t.emit_process_meta(name);
    RunGuard {
        span: Some(t.open("process", name.to_string())),
    }
}

/// Open a nested span. `B` is written (and flushed) immediately; the guard emits
/// `E` + RSS args on drop. No allocation on the disabled path.
pub fn span(cat: &'static str, name: impl Into<TraceName>) -> SpanGuard {
    let Some(t) = tracer() else {
        return SpanGuard { span: None };
    };
    let name = name.into().into_string();
    SpanGuard {
        span: Some(t.open(cat, name)),
    }
}

/// Start sampling a phase's working-set peak (#160 spec §4.1.4): one sample now,
/// then one every 10 ms on a background thread until the [`run`] guard drops,
/// which reports the maximum as `peak.post_attach_mb`. A second call restarts
/// the sampler: the maximum is reset, so the last phase's maximum is reported.
/// With tracing off it is one `OnceLock` read.
pub fn mark_phase_start(name: &'static str) {
    let Some(t) = tracer() else {
        return;
    };
    t.finish_phase();
    let sampler = PhaseSampler::start();
    if let Ok(mut p) = t.phase.lock() {
        *p = Some(sampler);
    }
    t.emit_instant("phase", "phase.start", json!({ "name": name }));
}

/// Emit an absolute counter value.
pub fn counter(name: &'static str, v: u64) {
    if let Some(t) = tracer() {
        t.set_counter(name, v);
    }
}

/// Emit a counter after adding `dv` to its running total.
pub fn counter_delta(name: &'static str, dv: u64) {
    if let Some(t) = tracer() {
        t.add_counter(name, dv);
    }
}

/// Emit a one-shot instant event whose `args` are built lazily — `build` runs
/// ONLY when tracing is enabled (so struct-shaped payloads cost nothing off).
pub fn instant_lazy(
    cat: &'static str,
    name: &'static str,
    build: impl FnOnce() -> serde_json::Value,
) {
    let Some(t) = tracer() else {
        return;
    };
    let val = build();
    t.emit_instant(cat, name, val);
}

/// Generalizes the old throwaway early-exit: if `ALSEM_TRACE_EXIT_AFTER` names
/// `span_name`, flush the trace and exit the process cleanly (0). Called from
/// `SpanGuard::drop`, and available as an explicit checkpoint.
pub fn maybe_exit_after(span_name: &str) {
    if let Some(t) = tracer()
        && t.config.exit_after.as_deref() == Some(span_name)
    {
        if let Ok(mut w) = t.writer.lock() {
            w.flush();
        }
        // This exit skips `main`'s own census print (#160 §4.1.7).
        crate::engine::l5::detector_context::report_cones_census();
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------------
// Tests (TDD — written to the spec's four groups)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// An env reader backed by a fixed table (no process-global env mutation, so
    /// these tests never race each other).
    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(a, _)| *a == k)
                .map(|(_, v)| v.to_string())
        }
    }

    fn events(path: &std::path::Path) -> Vec<Value> {
        let txt = std::fs::read_to_string(path).unwrap();
        // Must ALWAYS be valid JSON — that is the crash-safety contract.
        let v: Value = serde_json::from_str(&txt)
            .unwrap_or_else(|e| panic!("trace file is not valid JSON: {e}\n{txt}"));
        v.as_array().expect("trace root must be an array").clone()
    }

    fn test_cfg(file: PathBuf, detail: Detail) -> Config {
        Config {
            detail,
            file: Some(file),
            sample_every: 64,
            scc_min: 100,
            stderr: false,
            exit_after: None,
        }
    }

    // --- Group (a): env parsing --------------------------------------------

    #[test]
    fn env_off_by_default() {
        assert!(Config::from_reader(|_| None).is_none());
        assert!(Config::from_reader(env(&[("ALSEM_TRACE", "0")])).is_none());
        assert!(Config::from_reader(env(&[("ALSEM_TRACE", "off")])).is_none());
        assert!(Config::from_reader(env(&[("ALSEM_TRACE", "")])).is_none());
    }

    #[test]
    fn env_enable_variants_and_defaults() {
        for val in ["1", "chrome", "CHROME", " 1 "] {
            let c = Config::from_reader(env(&[("ALSEM_TRACE", val)]))
                .unwrap_or_else(|| panic!("{val} should enable"));
            assert_eq!(c.detail, Detail::Stages, "default detail = stages");
            assert_eq!(c.sample_every, 64, "default sample_every = 64");
            assert_eq!(c.scc_min, 100, "default scc_min = 100");
            assert!(c.file.is_none(), "no ALSEM_TRACE_FILE => derived default");
            assert!(!c.stderr);
            assert!(c.exit_after.is_none());
        }
    }

    #[test]
    fn env_detail_tiers_are_cumulative() {
        let hot = Config::from_reader(env(&[("ALSEM_TRACE", "1"), ("ALSEM_TRACE_DETAIL", "hot")]))
            .unwrap();
        assert_eq!(hot.detail, Detail::Hot);
        // Hot enables every lower tier.
        assert!(hot.detail >= Detail::Stages);
        assert!(hot.detail >= Detail::Hot);

        // The retired `jacobi` tier now parses as the base `Stages` tier (the
        // `_ => Detail::Stages` fallback), and never enables `Hot`.
        let retired = Config::from_reader(env(&[
            ("ALSEM_TRACE", "1"),
            ("ALSEM_TRACE_DETAIL", "jacobi"),
        ]))
        .unwrap();
        assert_eq!(retired.detail, Detail::Stages);
        assert!(retired.detail < Detail::Hot, "stages does NOT enable hot");

        let stages = Config::from_reader(env(&[("ALSEM_TRACE", "1")])).unwrap();
        assert_eq!(stages.detail, Detail::Stages);
        assert!(stages.detail < Detail::Hot, "stages does NOT enable hot");
    }

    #[test]
    fn env_sample_scc_file_stderr_exit_overrides() {
        let c = Config::from_reader(env(&[
            ("ALSEM_TRACE", "1"),
            ("ALSEM_TRACE_SAMPLE_EVERY", "128"),
            ("ALSEM_TRACE_SCC_MIN", "250"),
            ("ALSEM_TRACE_FILE", "custom.json"),
            ("ALSEM_TRACE_STDERR", "1"),
            ("ALSEM_TRACE_EXIT_AFTER", "l3.resolve"),
        ]))
        .unwrap();
        assert_eq!(c.sample_every, 128);
        assert_eq!(c.scc_min, 250);
        assert_eq!(c.file, Some(PathBuf::from("custom.json")));
        assert!(c.stderr);
        assert_eq!(c.exit_after.as_deref(), Some("l3.resolve"));

        // Invalid numerics fall back to the defaults, not to 0.
        let bad = Config::from_reader(env(&[
            ("ALSEM_TRACE", "1"),
            ("ALSEM_TRACE_SAMPLE_EVERY", "abc"),
            ("ALSEM_TRACE_SCC_MIN", "0"),
        ]))
        .unwrap();
        assert_eq!(bad.sample_every, 64);
        assert_eq!(bad.scc_min, 100);
    }

    // --- Group (b): Chrome JSON validity via the real writer ---------------

    #[test]
    fn chrome_json_is_valid_and_well_shaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.json");
        let t = Tracer::init(test_cfg(path.clone(), Detail::Hot));
        t.emit_process_meta("analyze");

        let s = t.open("stage", "l3.resolve".to_string());

        // Immediate-B contract: the B event is durable the moment the span opens,
        // BEFORE it closes.
        let mid = events(&path);
        assert!(
            mid.iter()
                .any(|e| e["ph"] == "B" && e["name"] == "l3.resolve"),
            "B must be written immediately: {mid:?}"
        );
        assert!(
            !mid.iter().any(|e| e["ph"] == "E"),
            "no E before the span closes"
        );

        t.close(&s);
        t.set_counter("nodes_visited", 5);
        t.add_counter("memo_misses", 3);
        t.emit_instant("scc", "anatomy", json!({ "largest": 10 }));

        let evs = events(&path);
        let ph = |p: &str| evs.iter().filter(|e| e["ph"] == p).count();
        assert_eq!(ph("M"), 1, "process metadata");
        assert_eq!(ph("B"), 1);
        assert_eq!(ph("E"), 1);
        assert_eq!(ph("C"), 2, "one absolute + one delta counter");
        assert_eq!(ph("i"), 1);

        let b = evs.iter().find(|e| e["ph"] == "B").unwrap();
        assert_eq!(b["cat"], "stage");
        assert_eq!(b["name"], "l3.resolve");
        assert!(b["ts"].is_number());
        assert!(b["pid"].is_number());
        assert!(b["tid"].is_number());

        let e = evs.iter().find(|e| e["ph"] == "E").unwrap();
        assert_eq!(e["name"], "l3.resolve");
        assert!(e["args"].is_object(), "E carries RSS args object");
        assert_eq!(e["tid"], b["tid"], "B/E share the captured tid");
    }

    #[test]
    fn counters_track_absolute_and_running_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let t = Tracer::init(test_cfg(path.clone(), Detail::Hot));
        t.add_counter("edges", 10);
        t.add_counter("edges", 5);
        let evs = events(&path);
        let last = evs.iter().rfind(|e| e["ph"] == "C").unwrap();
        assert_eq!(last["args"]["edges"], 15, "delta counters accumulate");
    }

    // --- Group (c): killed-run checkpoint ----------------------------------

    #[test]
    fn killed_run_leaves_valid_json_with_open_span() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("killed.json");
        {
            let t = Tracer::init(test_cfg(path.clone(), Detail::Stages));
            t.emit_process_meta("analyze");
            let _open = t.open("stage", "l4_l5.run_detectors".to_string());
            // Simulate a cap-kill: drop the tracer WITHOUT closing the span.
            drop(_open); // ActiveSpan is a plain value here; closing is the guard's job
            drop(t);
        }
        // In the real pipeline the SpanGuard would close on drop; here we model a
        // process killed while the span is live by never emitting E.
        let evs = events(&path); // panics if not valid JSON
        assert!(
            evs.iter()
                .any(|e| e["ph"] == "B" && e["name"] == "l4_l5.run_detectors"),
            "open span must be visible in a killed run"
        );
        assert!(
            !evs.iter().any(|e| e["ph"] == "E"),
            "a killed-mid-span run has no matching E"
        );
    }

    // --- Group (d): disabled path emits nothing / no file ------------------

    #[test]
    fn disabled_path_creates_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("should-not-exist.json");

        // The full gated flow: a disabled env yields no Config, so no Tracer and
        // no file is ever opened.
        let cfg = Config::from_reader(env(&[
            ("ALSEM_TRACE", "0"),
            ("ALSEM_TRACE_FILE", path.to_str().unwrap()),
        ]));
        assert!(cfg.is_none(), "ALSEM_TRACE=0 => disabled");
        assert!(!path.exists(), "disabled path must never create a file");
    }

    #[test]
    fn local_counters_arithmetic() {
        let mut lc = LocalCounters::new();
        lc.add("hits", 3);
        lc.add("hits", 4);
        lc.set("misses", 9);
        lc.add("misses", 1);
        assert_eq!(lc.get("hits"), 7);
        assert_eq!(lc.get("misses"), 10);
        assert_eq!(lc.get("absent"), 0);
    }

    #[test]
    fn trace_name_conversions() {
        let s: TraceName = "static".into();
        assert_eq!(s.into_string(), "static");
        let o: TraceName = String::from("owned").into();
        assert_eq!(o.into_string(), "owned");
    }

    // --- Group (e): Windows RSS smoke (task 4 acceptance gate) -------------

    /// Direct FFI smoke test: `read_rss` must return real, nonzero counters with
    /// peak >= current on Windows (the platform the K32 probe targets; `None`
    /// off-Windows is covered structurally by the `#[cfg(not(windows))]` stub
    /// above and isn't a "smoke" claim to test).
    #[cfg(windows)]
    #[test]
    fn rss_probe_returns_nonzero_values_with_peak_at_least_current() {
        // Touch real memory first so the working set is unambiguously nonzero
        // even on a freshly-started test process.
        let keep_alive: Vec<u8> = vec![7u8; 8 * 1024 * 1024];
        let r = read_rss().expect("K32GetProcessMemoryInfo must succeed on Windows");
        assert!(r.working_set > 0, "working_set must be nonzero: {r:?}");
        assert!(
            r.peak_working_set > 0,
            "peak_working_set must be nonzero: {r:?}"
        );
        assert!(
            r.peak_working_set >= r.working_set,
            "peak_working_set must be >= working_set (two-instant OS snapshot): {r:?}"
        );
        assert!(r.private_usage > 0, "private_usage must be nonzero: {r:?}");
        std::hint::black_box(&keep_alive);
    }

    /// Integration-shaped smoke test: a real span's `E` event (the shape every
    /// production span actually emits) carries nonzero `rss_mb`/`peak_mb` with
    /// `peak_mb >= rss_mb`, proving the FFI reading is correctly wired through
    /// `Tracer::open`/`close`/`emit_end`, not just callable in isolation.
    #[cfg(windows)]
    #[test]
    fn span_close_embeds_nonzero_rss_args_with_peak_at_least_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rss.json");
        let t = Tracer::init(test_cfg(path.clone(), Detail::Stages));
        let s = t.open("stage", "rss.smoke".to_string());
        t.close(&s);

        let evs = events(&path);
        let e = evs.iter().find(|e| e["ph"] == "E").unwrap();
        let rss_mb = e["args"]["rss_mb"]
            .as_u64()
            .expect("rss_mb must be present");
        let peak_mb = e["args"]["peak_mb"]
            .as_u64()
            .expect("peak_mb must be present");
        assert!(rss_mb > 0, "rss_mb must be nonzero: {e:?}");
        assert!(peak_mb >= rss_mb, "peak_mb must be >= rss_mb: {e:?}");
    }
}
