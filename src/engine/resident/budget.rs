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
//! table value.
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
//! Linux; elsewhere none, and admission then reads the estimates alone) or a
//! test's.

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
/// explain there; 6 GiB holds CDO's measured peak (5.4 GB) with margin.
pub const DEFAULT_BUDGET_CROSS_APP: u64 = 6 * GIB;

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
            Case::Fill("raw") => 600,
            Case::Fill("det") => 350,
            Case::Fill("l4") => 250,
            Case::Fill("run") => 120,
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
    /// The last three observed peaks per case.
    observed: BTreeMap<Case, VecDeque<u64>>,
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

    /// The estimate of `case`: the largest of its last three observed peaks,
    /// else the table's, scaled by routine count (the pre-model formula for
    /// a cold build).
    #[must_use]
    pub fn of(&self, case: Case, sizes: InputSizes) -> u64 {
        if let Some(max) = self.observed.get(&case).and_then(|q| q.iter().max()) {
            return *max;
        }
        match (case, self.routines) {
            (Case::Cold, _) | (Case::Incremental, None) => self.pre_model(sizes),
            (_, None) => case.table_mib() * MIB,
            (_, Some(r)) => (case.table_mib() * MIB).saturating_mul(r.max(1)) / TABLE_ROUTINES,
        }
    }

    /// Record an observed peak of `case`.
    pub fn observe(&mut self, case: Case, peak: u64) {
        let q = self.observed.entry(case).or_default();
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
/// window reset through `/proc/self/clear_refs` (Linux ≥ 4.0). Elsewhere
/// every reading is 0 and admission reads the estimates alone.
#[derive(Debug, Default)]
pub struct ProcMeter;

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
            self.budget
                .estimates()
                .observe(self.case, peak.saturating_sub(self.rss_before));
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

    /// The estimate of `case` over inputs of `sizes`.
    #[must_use]
    pub fn estimate(&self, case: Case, sizes: InputSizes) -> u64 {
        self.estimates().of(case, sizes)
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
