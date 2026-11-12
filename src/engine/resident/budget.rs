//! Admission and reservation (context-server design §4.5, §7; stage 3).
//!
//! **Reservation before work.** A build, a fill and a request's trees each
//! reserve their estimated peak first: [`Budget::reserve`] succeeds iff
//! `rss + reserved + estimate <= budget`, where `reserved` sums every
//! reservation still held (a fill in flight holds its own until it has
//! published: fill workers are counted). A reservation is released when its
//! guard drops, on every path.
//!
//! **Estimates** ([`Estimates`]): before the first build, from the
//! workspace's `.al` bytes and its packages' bytes, never below a nonzero
//! floor (cross-app 1.5 GiB, single-app 0.5 GiB); after it, a per-case table
//! measured on CDO, scaled by the revision's routine count. Once a case has
//! been observed, the largest of its last three observed peaks replaces the
//! table value, per size class: a request's trees and its witness batch are
//! observed and estimated by the number of roots they cover ([`size_class`]),
//! so one large request's peak is never a small request's estimate.
//!
//! **Floor** ([`Budget::retained_floor`], `status.retainedBytes`): the bytes
//! of the complete retained set, the part no eviction can free: the RSS less
//! the measured bytes of every evictable artifact the published revision
//! holds. A reservation whose estimate does not fit even above the floor is
//! impossible: it fails at once with the estimate, the budget and the floor
//! ([`Refusal`]), and is re-evaluated on every request (so a size-reducing
//! edit, or a larger budget, lifts it).
//!
//! **Overrun.** Work is never aborted for memory. A finished piece of work
//! whose RSS passed the budget counts an overrun, and its observed peak
//! raises its case's estimate; no new reservation fits until the RSS is back
//! under the budget.
//!
//! The RSS and peak come from a [`Meter`]: the process's own (`/proc` on
//! Linux; the working set on Windows, whose peak window is known only when
//! the lifetime peak rose in it, [`window_peak`]; elsewhere none, and
//! admission then reads the estimates alone) or a test's.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// 2^30 bytes.
pub const GIB: u64 = 1 << 30;

/// 2^20 bytes.
pub const MIB: u64 = 1 << 20;

/// The default `--memory-budget` of a cross-app child. The design's 4 GiB
/// (§7, an open point of §13) is below what CDO retains after one build
/// (3.7 GB RSS, stage 3 measurement), so it would refuse every first
/// explain there. Stage 3 set 6 GiB for CDO's then peak (5.4 GB); stage 5b's
/// cone and tree stores raised the resident default-mode figure to about
/// 6.0 GB after a B edit (peak 6.34 GB), right at 6 GiB, so trees were
/// evicted under normal use. 8 GiB (owner decision, 2026-11-03) keeps them
/// with margin; `--low-mem` keeps its own 6 GiB.
pub const DEFAULT_BUDGET_CROSS_APP: u64 = 8 * GIB;

/// The default `--memory-budget` of a single-app child (CDO single-app
/// one-shot peak 0.75-0.9 GB).
pub const DEFAULT_BUDGET_SINGLE_APP: u64 = 2 * GIB;

/// The default `--memory-budget` of a cross-app child under `--low-mem`: the
/// same 6 GiB, set by measurement (CDO, 2026-11-02, the four asks then a B,
/// an S and an S edit, each followed by the four asks). `--low-mem` keeps the
/// RSS about 1 GB lower (3.7 GB after the four asks, 4.4 GB after the edits,
/// against 4.7 and 5.4 GB), but a lower budget refuses real work: an S
/// refresh parses every dependency file again, its observed peak becomes its
/// estimate, and the retained floor creeps up 0.2-0.3 GB per refresh
/// (allocator retention). At 4.5 GiB the asks after the first S were refused,
/// at 5 GiB the second S was refused as impossible; at 6 GiB nothing was
/// refused or evicted. The saving is headroom (fewer evictions, room under
/// `--total-budget` for a second child), not a smaller default.
pub const DEFAULT_BUDGET_CROSS_APP_LOW_MEM: u64 = 6 * GIB;

/// The default `--memory-budget` of a child: by universe, and for a
/// cross-app child by memory mode (`--low-mem` keeps no dependency tree;
/// a single-app child never holds one, so the mode changes nothing there).
#[must_use]
pub fn default_budget(single_app: bool, low_mem: bool) -> u64 {
    match (single_app, low_mem) {
        (true, _) => DEFAULT_BUDGET_SINGLE_APP,
        (false, false) => DEFAULT_BUDGET_CROSS_APP,
        (false, true) => DEFAULT_BUDGET_CROSS_APP_LOW_MEM,
    }
}

/// The `--memory-budget` defaults in the words `--help` prints
/// (`8G cross-app, 6G cross-app with --low-mem, 2G single-app`), from the
/// constants above, so the help cannot drift from them again (engine limits
/// B8: it said `6G cross-app` after the default became 8 GiB).
#[must_use]
pub fn default_budget_text() -> String {
    format!(
        "{} cross-app, {} cross-app with --low-mem, {} single-app",
        size_text(DEFAULT_BUDGET_CROSS_APP),
        size_text(DEFAULT_BUDGET_CROSS_APP_LOW_MEM),
        size_text(DEFAULT_BUDGET_SINGLE_APP)
    )
}

/// `bytes` with the largest of G, M or K that divides it (the suffixes
/// `--memory-budget` takes), else in bytes.
fn size_text(bytes: u64) -> String {
    for (unit, suffix) in [(GIB, "G"), (MIB, "M"), (1 << 10, "K")] {
        if bytes >= unit && bytes.is_multiple_of(unit) {
            return format!("{}{suffix}", bytes / unit);
        }
    }
    bytes.to_string()
}

/// The pre-model estimate's floor, cross-app (§4.5).
pub const ESTIMATE_FLOOR_CROSS_APP: u64 = 3 * GIB / 2;

/// The pre-model estimate's floor, single-app (§4.5).
pub const ESTIMATE_FLOOR_SINGLE_APP: u64 = GIB / 2;

/// The default `--idle-timeout` (§7).
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

/// The routine count the per-case table was measured at (CDO, cross-app).
pub const TABLE_ROUTINES: u64 = 33_287;

/// What a reservation is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Case {
    /// A build with no revision to reuse (the first, or after a failure).
    Cold,
    /// A build beside the published revision (B, S; a P is caught by the
    /// observed peak).
    Incremental,
    /// A fill of an artifact kind (`ArtifactKey::kind`).
    Fill(&'static str),
    /// A request's trees (explain, prove, digest).
    Trees,
}

impl Case {
    /// The case's name (`status`, refusal messages).
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Case::Cold => "cold build".into(),
            Case::Incremental => "incremental build".into(),
            Case::Fill(k) => format!("{k} fill"),
            Case::Trees => "trees".into(),
        }
    }

    /// The measured CDO peak over the RSS before the work (stage 3, MiB),
    /// scaled by routine count; `Cold` is the pre-model formula's.
    fn table_mib(self) -> u64 {
        match self {
            Case::Cold => 3_800,
            Case::Incremental => 1_600,
            Case::Fill("fold") => 900,
            // A view's raw facts (plan 2026-11-03 L6): emitted from the
            // fold's retained cones, no longer a raw-only composition (the
            // old 600). One root on CDO: 14 ms, 0.32 MiB left resident
            // (`census-probe incr`, `PROBE_RAW_BYTES=1`); the default's
            // 60 MiB until the fill's peak is measured apart.
            Case::Fill("raw") => 60,
            Case::Fill("det") => 350,
            Case::Fill("l4") => 250,
            Case::Fill("run") => 120,
            // The tree store (stage 5): a view's tree and the whole sections,
            // the `Trees` case's composition half; not measured apart.
            Case::Fill("tree") => 900,
            Case::Fill("tree_base") => 450,
            // A root's witnesses (plan 2026-11-03 W): 5.6-7.9 MiB resident
            // per root on CDO (`census-probe incr --witness`); a request's
            // first witness fill computes all its roots and the tree's
            // indexes (~0.1 s of work), so the default's 60 MiB stands until
            // the fill's peak is measured apart.
            Case::Fill("witness") => 60,
            // Explain's indexes (plan 2026-11-03 W2): the fill's peak over
            // the live heap before it, +29.0 MiB on CDO, 25.7 MiB left
            // resident (`census-probe incr --witness`, 2026-11-03).
            Case::Fill("explain_indexes") => 30,
            // The sited graph (agent-tasks M0-1/M0-5), built inside the
            // revision's build: 46 MiB left resident on CDO cross-app
            // (`aldump --session-refresh --sited`, 2026-11-05), plus its
            // build's resolve index, source facts and collapse map, not
            // measured apart.
            Case::Fill("sited") => 120,
            Case::Fill(_) => 60,
            Case::Trees => 900,
        }
    }
}

/// The sizes a pre-model estimate reads (from the manifest's stats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputSizes {
    pub al_bytes: u64,
    pub package_bytes: u64,
}

/// TLS-22: the literal pass's extra peak per extra worker, at CDO's size
/// (scaled by routine count like the table): the one-shot 699-root
/// `impact --diff` peaked 153 MiB higher at 8 workers than at 1 (about
/// 22 MiB per extra worker), `--base` 149 MiB (2026-11-12,
/// `docs/superpowers/notes/2026-11-12-parallel-literal-pass.md`).
pub const REACH_WORKER_MIB: u64 = 25;

/// Bytes of peak per `.al` byte (CDO single-app: 0.8 GB over 4.74 MiB).
pub const PER_AL_BYTE: u64 = 180;

/// Bytes of peak per package byte (CDO cross-app: 2.9 GB over 70 MiB).
pub const PER_PACKAGE_BYTE: u64 = 45;

/// The estimator (see the module doc).
#[derive(Clone, Debug)]
pub struct Estimates {
    pub single_app: bool,
    /// The pre-model floor.
    pub floor: u64,
    pub per_al_byte: u64,
    pub per_package_byte: u64,
    /// The published revision's routine count (`None` before the first).
    pub routines: Option<u64>,
    /// TLS-22: how many roots the literal pass runs at once in this session
    /// (1 under `--low-mem`): a table estimate of a pass-running case adds
    /// [`REACH_WORKER_MIB`] per extra worker ([`Estimates::of_sized`]).
    pub reach_workers: u64,
    /// The last three observed peaks per case and size class
    /// ([`size_class`]).
    observed: BTreeMap<(Case, u32), VecDeque<u64>>,
}

/// The size class of a piece of work over `units` roots (0 or 1: class 0;
/// then one class per factor of 4: 2-4, 5-16, 17-64, 65-256, ...). A
/// request's trees and its witness batch grow with their roots: one 699-root
/// impact's trees peaked at 4.2 GB on CDO, a one-root digest's at a few
/// hundred MiB. An estimate is read from its own class's observations, so a
/// large request's peak never becomes the next small request's estimate
/// (it refused every later one-routine ask: 4174 MiB over a 5.7 GB RSS).
#[must_use]
pub fn size_class(units: u64) -> u32 {
    let mut class = 0;
    let mut top = 1u64;
    while units > top {
        top = top.saturating_mul(4);
        class += 1;
    }
    class
}

impl Estimates {
    /// The design's estimator for a universe.
    #[must_use]
    pub fn new(single_app: bool) -> Estimates {
        Estimates {
            single_app,
            floor: if single_app {
                ESTIMATE_FLOOR_SINGLE_APP
            } else {
                ESTIMATE_FLOOR_CROSS_APP
            },
            per_al_byte: PER_AL_BYTE,
            per_package_byte: PER_PACKAGE_BYTE,
            routines: None,
            reach_workers: 1,
            observed: BTreeMap::new(),
        }
    }

    /// The pre-model estimate of a cold build over inputs of `sizes`.
    #[must_use]
    pub fn pre_model(&self, sizes: InputSizes) -> u64 {
        let packages = if self.single_app {
            0
        } else {
            sizes.package_bytes.saturating_mul(self.per_package_byte)
        };
        sizes
            .al_bytes
            .saturating_mul(self.per_al_byte)
            .saturating_add(packages)
            .max(self.floor)
    }

    /// The estimate of `case` for work of one unit ([`Estimates::of_sized`]).
    #[must_use]
    pub fn of(&self, case: Case, sizes: InputSizes) -> u64 {
        self.of_sized(case, sizes, 1)
    }

    /// The estimate of `case` for work over `units` roots: the largest of
    /// the last three peaks observed in its size class ([`size_class`]);
    /// else the table's, scaled by routine count (the pre-model formula for
    /// a cold build), or the largest peak a SMALLER class observed when that
    /// is larger. A larger class's peaks are never read: a 699-root
    /// request's is no estimate of a one-root one.
    #[must_use]
    pub fn of_sized(&self, case: Case, sizes: InputSizes, units: u64) -> u64 {
        let class = size_class(units);
        if let Some(max) = self
            .observed
            .get(&(case, class))
            .and_then(|q| q.iter().max())
        {
            return *max;
        }
        // TLS-22: a case that runs the literal pass holds up to one root's
        // working set per extra worker at once; an observed peak includes
        // it, the table must add it.
        let workers = match case {
            Case::Trees | Case::Fill("witness") => {
                self.reach_workers.min(units).saturating_sub(1) * REACH_WORKER_MIB
            }
            _ => 0,
        };
        let mib = case.table_mib() + workers;
        let table = match (case, self.routines) {
            (Case::Cold, _) | (Case::Incremental, None) => self.pre_model(sizes),
            (_, None) => mib * MIB,
            (_, Some(r)) => (mib * MIB).saturating_mul(r.max(1)) / TABLE_ROUTINES,
        };
        let smaller = self
            .observed
            .range((case, 0)..(case, class))
            .flat_map(|(_, q)| q.iter().copied())
            .max()
            .unwrap_or(0);
        table.max(smaller)
    }

    /// Record an observed peak of `case` for work of one unit.
    pub fn observe(&mut self, case: Case, peak: u64) {
        self.observe_sized(case, 1, peak);
    }

    /// Record an observed peak of `case` for work over `units` roots.
    pub fn observe_sized(&mut self, case: Case, units: u64, peak: u64) {
        let q = self.observed.entry((case, size_class(units))).or_default();
        q.push_back(peak);
        while q.len() > 3 {
            q.pop_front();
        }
    }
}

/// The process's memory as the OS reports it.
pub trait Meter: Send + Sync {
    /// Resident bytes now (0 when unknown).
    fn rss(&self) -> u64;
    /// The peak resident bytes since the last [`Meter::reset_peak`].
    fn peak(&self) -> u64;
    /// Start a new peak window.
    fn reset_peak(&self);
}

/// This process's memory: `/proc/self/status` (`VmRSS`, `VmHWM`), the peak
/// window reset through `/proc/self/clear_refs` (Linux ≥ 4.0). On Windows
/// the working set (`K32GetProcessMemoryInfo`), with the peak window
/// [`window_peak`] derives from the process's lifetime peak. Elsewhere every
/// reading is 0 and admission reads the estimates alone.
#[derive(Debug, Default)]
pub struct ProcMeter;

/// A peak window's top on a platform that keeps only the process's lifetime
/// peak and cannot reset it (Windows' `PeakWorkingSetSize`): `lifetime` when
/// it rose above `at_reset` (the lifetime peak when the window opened), which
/// is then exactly the window's top; else 0, unknown (the window's top lies
/// somewhere at or below the old peak), which records no observation, so the
/// case keeps its table or earlier estimate.
#[must_use]
pub fn window_peak(lifetime: u64, at_reset: u64) -> u64 {
    if lifetime > at_reset { lifetime } else { 0 }
}

/// The process's (working set, lifetime peak working set) in bytes; 0s when
/// the call fails.
#[cfg(windows)]
fn working_set() -> (u64, u64) {
    crate::engine::perf_trace::working_set().unwrap_or((0, 0))
}

/// The lifetime peak when the current peak window opened (Windows).
#[cfg(windows)]
static PEAK_AT_RESET: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
impl Meter for ProcMeter {
    fn rss(&self) -> u64 {
        working_set().0
    }
    fn peak(&self) -> u64 {
        window_peak(working_set().1, PEAK_AT_RESET.load(Ordering::SeqCst))
    }
    fn reset_peak(&self) {
        PEAK_AT_RESET.store(working_set().1, Ordering::SeqCst);
    }
}

#[cfg(not(windows))]
impl ProcMeter {
    fn status_kib(key: &str) -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with(key))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .unwrap_or(0)
    }
}

#[cfg(not(windows))]
impl Meter for ProcMeter {
    fn rss(&self) -> u64 {
        Self::status_kib("VmRSS:") * 1024
    }
    fn peak(&self) -> u64 {
        Self::status_kib("VmHWM:") * 1024
    }
    fn reset_peak(&self) {
        let _ = std::fs::write("/proc/self/clear_refs", "5");
    }
}

/// Why a reservation was refused (the `budget` outcome's fields).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub case: Case,
    pub estimate: u64,
    pub budget: u64,
    pub rss: u64,
    pub reserved: u64,
    pub retained_floor: u64,
    /// The estimate does not fit even above the retained floor: no
    /// eviction or wait could make room.
    pub impossible: bool,
}

impl Refusal {
    /// The answer's `estimateKiB`, `budgetKiB`, `retainedFloorKiB`.
    #[must_use]
    pub fn figures(&self) -> crate::wire::BudgetFigures {
        crate::wire::BudgetFigures {
            estimate_kib: self.estimate / 1024,
            budget_kib: self.budget / 1024,
            retained_floor_kib: self.retained_floor / 1024,
        }
    }

    /// The outcome's message.
    #[must_use]
    pub fn message(&self) -> String {
        let mib = |b: u64| b / MIB;
        format!(
            "memory budget: the {} needs about {} MiB; budget {} MiB, resident {} MiB, \
             reserved {} MiB, retained floor {} MiB{}",
            self.case.name(),
            mib(self.estimate),
            mib(self.budget),
            mib(self.rss),
            mib(self.reserved),
            mib(self.retained_floor),
            if self.impossible {
                " (it cannot fit above the retained floor: raise --memory-budget)"
            } else {
                ""
            }
        )
    }
}

/// What one reclaim did: the entries it evicted and their measured bytes,
/// and the RSS before it, after the evictions and after the freed memory
/// was handed back to the OS ([`crate::release_alloc::collect_freed`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReclaimRecord {
    pub entries: u64,
    /// The evicted artifacts' measured bytes (the session's dependency rows
    /// are not measured: 0 for them).
    pub measured_bytes: u64,
    pub rss_before: u64,
    pub rss_evicted: u64,
    pub rss_collected: u64,
    pub collect_ms: u64,
}

/// The counters `status` reports.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetStatus {
    pub budget_bytes: u64,
    pub rss_bytes: u64,
    pub reserved_bytes: u64,
    pub retained_bytes: u64,
    pub fill_workers: u64,
    pub holders: u64,
    pub admission_closed: bool,
    pub reservations: u64,
    pub refusals: u64,
    pub impossible: u64,
    pub overruns: u64,
    pub reclaims: u64,
    pub evicted: u64,
    /// The last reclaim that evicted anything.
    pub last_reclaim: Option<ReclaimRecord>,
}

#[derive(Debug, Default)]
struct State {
    reserved: u64,
    fill_workers: u64,
    holders: u64,
    admission_closed: bool,
    reservations: u64,
    refusals: u64,
    impossible: u64,
    overruns: u64,
    reclaims: u64,
    evicted: u64,
    last_reclaim: Option<ReclaimRecord>,
    /// The bytes no eviction can free (see the module doc).
    retained_floor: u64,
}

/// A child's memory budget (see the module doc).
pub struct Budget {
    budget: AtomicU64,
    meter: Arc<dyn Meter>,
    state: Mutex<State>,
    changed: std::sync::Condvar,
    estimates: Mutex<Estimates>,
}

/// A held reservation; released on drop. A fill's counts as a fill worker
/// while held.
pub struct Reservation<'a> {
    budget: &'a Budget,
    case: Case,
    /// The work's size in roots ([`size_class`]): its peak is observed in
    /// that class.
    units: u64,
    bytes: u64,
    /// The RSS when it was taken (an overrun and an observed peak are
    /// measured from it).
    rss_before: u64,
}

impl Reservation<'_> {
    /// The reserved bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The work is done: record its observed peak (the peak window's top
    /// over the RSS it started at) and whether it overran the budget.
    /// Returns the bytes the work left resident (RSS now less RSS before).
    pub fn finish(self) -> u64 {
        let meter = &self.budget.meter;
        let (peak, rss) = (meter.peak(), meter.rss());
        if peak > 0 {
            self.budget.estimates().observe_sized(
                self.case,
                self.units,
                peak.saturating_sub(self.rss_before),
            );
        }
        if rss > self.budget.budget() {
            self.budget.state().overruns += 1;
        }
        rss.saturating_sub(self.rss_before)
    }
}

impl crate::engine::resident::store::Held for Reservation<'_> {
    fn finish(self: Box<Self>) -> u64 {
        Reservation::finish(*self)
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut s = self.budget.state();
        s.reserved = s.reserved.saturating_sub(self.bytes);
        if matches!(self.case, Case::Fill(_)) {
            s.fill_workers = s.fill_workers.saturating_sub(1);
        }
        drop(s);
        self.budget.changed.notify_all();
    }
}

/// An admitted request (a holder); released on drop.
pub struct Admitted<'a> {
    budget: &'a Budget,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        let mut s = self.budget.state();
        s.holders = s.holders.saturating_sub(1);
        drop(s);
        self.budget.changed.notify_all();
    }
}

/// Admission is closed while a reclaim runs; the closer reopens it on drop.
pub struct Closed<'a> {
    budget: &'a Budget,
}

impl Drop for Closed<'_> {
    fn drop(&mut self) {
        self.budget.state().admission_closed = false;
        self.budget.changed.notify_all();
    }
}

impl Budget {
    /// A budget of `bytes` over `meter`, with `estimates`.
    #[must_use]
    pub fn new(bytes: u64, meter: Arc<dyn Meter>, estimates: Estimates) -> Budget {
        Budget {
            budget: AtomicU64::new(bytes),
            meter,
            state: Mutex::new(State::default()),
            changed: std::sync::Condvar::new(),
            estimates: Mutex::new(estimates),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The estimator.
    pub fn estimates(&self) -> MutexGuard<'_, Estimates> {
        self.estimates.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The budget in bytes.
    #[must_use]
    pub fn budget(&self) -> u64 {
        self.budget.load(Ordering::Relaxed)
    }

    /// Change the budget (impossibility is re-evaluated on the next request).
    pub fn set_budget(&self, bytes: u64) {
        self.budget.store(bytes, Ordering::Relaxed);
        self.changed.notify_all();
    }

    /// The meter.
    #[must_use]
    pub fn meter(&self) -> &dyn Meter {
        &*self.meter
    }

    /// The retained floor last set ([`Budget::set_retained_floor`]).
    #[must_use]
    pub fn retained_floor(&self) -> u64 {
        self.state().retained_floor
    }

    /// Set the retained floor: the RSS less the evictable bytes.
    pub fn set_retained_floor(&self, bytes: u64) {
        self.state().retained_floor = bytes;
    }

    /// Count `n` evicted entries (status).
    pub fn count_evicted(&self, n: u64) {
        self.state().evicted += n;
    }

    /// Record a reclaim that evicted entries (status `lastReclaim`).
    pub fn record_reclaim(&self, r: ReclaimRecord) {
        self.state().last_reclaim = Some(r);
    }

    /// The estimate of `case` over inputs of `sizes`.
    #[must_use]
    pub fn estimate(&self, case: Case, sizes: InputSizes) -> u64 {
        self.estimates().of(case, sizes)
    }

    /// The estimate of `case` for work over `units` roots
    /// ([`Estimates::of_sized`]).
    #[must_use]
    pub fn estimate_sized(&self, case: Case, sizes: InputSizes, units: u64) -> u64 {
        self.estimates().of_sized(case, sizes, units)
    }

    /// Admit a request as a holder: waits while a reclaim has admission
    /// closed (each wait slice re-reads `stop`; a stopped request leaves
    /// with `false`). The reclaim that closed it reopens it after one
    /// attempt, so the wait is bounded.
    pub fn admit(&self, stop: &dyn Fn() -> bool) -> Option<Admitted<'_>> {
        let mut s = self.state();
        while s.admission_closed {
            if stop() {
                return None;
            }
            s = self
                .changed
                .wait_timeout(s, std::time::Duration::from_millis(20))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        s.holders += 1;
        Some(Admitted { budget: self })
    }

    /// Whether `estimate` fits now: `rss + reserved + estimate <= budget`.
    #[must_use]
    pub fn fits(&self, estimate: u64) -> bool {
        let rss = self.meter.rss();
        rss.saturating_add(self.state().reserved)
            .saturating_add(estimate)
            <= self.budget()
    }

    /// Whether `estimate` can ever fit: above the retained floor, within the
    /// budget.
    #[must_use]
    pub fn possible(&self, estimate: u64) -> bool {
        self.retained_floor().saturating_add(estimate) <= self.budget()
    }

    /// Reserve `estimate` bytes for `case` if `rss + reserved + estimate <=
    /// budget`; else the refusal. The peak window starts here.
    pub fn reserve(&self, case: Case, estimate: u64) -> Result<Reservation<'_>, Refusal> {
        self.reserve_sized(case, 1, estimate)
    }

    /// [`Budget::reserve`] for work over `units` roots: its peak is
    /// observed in that size class ([`size_class`]).
    pub fn reserve_sized(
        &self,
        case: Case,
        units: u64,
        estimate: u64,
    ) -> Result<Reservation<'_>, Refusal> {
        let rss = self.meter.rss();
        let budget = self.budget();
        let mut s = self.state();
        if rss.saturating_add(s.reserved).saturating_add(estimate) > budget {
            s.refusals += 1;
            let impossible = s.retained_floor.saturating_add(estimate) > budget;
            if impossible {
                s.impossible += 1;
            }
            return Err(Refusal {
                case,
                estimate,
                budget,
                rss,
                reserved: s.reserved,
                retained_floor: s.retained_floor,
                impossible,
            });
        }
        s.reserved += estimate;
        s.reservations += 1;
        if matches!(case, Case::Fill(_)) {
            s.fill_workers += 1;
        }
        drop(s);
        self.meter.reset_peak();
        Ok(Reservation {
            budget: self,
            case,
            units,
            bytes: estimate,
            rss_before: rss,
        })
    }

    /// Close admission for a reclaim (§4.5): no new holder is admitted until
    /// the guard drops.
    pub fn close(&self) -> Closed<'_> {
        let mut s = self.state();
        s.admission_closed = true;
        s.reclaims += 1;
        Closed { budget: self }
    }

    /// Wait, with admission closed, until no holder but `mine` and no fill
    /// worker remains (the in-flight releasers), or `stop`. Never waits when
    /// there is no releaser in flight.
    pub fn wait_for_releasers(&self, mine: u64, stop: &dyn Fn() -> bool) {
        let mut s = self.state();
        while s.holders > mine || s.fill_workers > 0 {
            if stop() {
                return;
            }
            s = self
                .changed
                .wait_timeout(s, std::time::Duration::from_millis(20))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// The counters.
    #[must_use]
    pub fn status(&self) -> BudgetStatus {
        let rss = self.meter.rss();
        let s = self.state();
        BudgetStatus {
            budget_bytes: self.budget(),
            rss_bytes: rss,
            reserved_bytes: s.reserved,
            retained_bytes: s.retained_floor,
            fill_workers: s.fill_workers,
            holders: s.holders,
            admission_closed: s.admission_closed,
            reservations: s.reservations,
            refusals: s.refusals,
            impossible: s.impossible,
            overruns: s.overruns,
            reclaims: s.reclaims,
            evicted: s.evicted,
            last_reclaim: s.last_reclaim.clone(),
        }
    }
}

/// A `--memory-budget` value: bytes, or with a `K`, `M`, `G` suffix (binary
/// units, optionally followed by `iB` or `B`).
pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let stripped = lower
        .strip_suffix("ib")
        .or_else(|| lower.strip_suffix('b'))
        .unwrap_or(&lower);
    let (num, unit) = match stripped.chars().last() {
        Some('k') => (&stripped[..stripped.len() - 1], 1u64 << 10),
        Some('m') => (&stripped[..stripped.len() - 1], 1 << 20),
        Some('g') => (&stripped[..stripped.len() - 1], 1 << 30),
        _ => (stripped, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("not a size: '{s}' (bytes, or a number with K, M or G)"))?;
    if !n.is_finite() || n <= 0.0 {
        return Err(format!("not a positive size: '{s}'"));
    }
    Ok((n * unit as f64) as u64)
}

/// A meter whose readings a test assigns.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeMeter {
    pub(crate) rss: AtomicU64,
    pub(crate) peak: AtomicU64,
}

#[cfg(test)]
impl Meter for FakeMeter {
    fn rss(&self) -> u64 {
        self.rss.load(Ordering::SeqCst)
    }
    fn peak(&self) -> u64 {
        self.peak.load(Ordering::SeqCst)
    }
    fn reset_peak(&self) {
        self.peak.store(self.rss(), Ordering::SeqCst);
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
