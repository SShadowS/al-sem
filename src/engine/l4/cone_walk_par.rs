//! TLS-29: the capability-cone walk over SCCs in parallel (spec
//! `docs/superpowers/specs/2026-11-13-parallel-cone-walk.md`).
//!
//! A ready queue: an SCC is ready when every successor (an SCC it calls) is
//! done, and a free worker takes the ready SCC with the lowest index (Tarjan
//! order, callees first). Each task builds its SCC's fact and coverage cones
//! and its members' derived rows and coverage records from its successors'
//! shared cones. A cone is freed when its last reader finishes. Results are
//! merged during the walk in SCC index order, so the output does not depend
//! on the worker count or on which task finishes first.
//!
//! Runs only for a walk with no cone store and no raw emission
//! ([`super::compose_inherited_cones`] picks it); everything else stays on
//! the serial loop.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use super::CapabilityFact;
use super::{
    ComposeStats, ConeFacts, ConeMap, CovMap, CoverageCone, CoverageRecord, GuardAnchor, MemoRows,
    RoutineDirectCoverage, RoutineDirectFacts, TypedEdgeGraph, coverage_cone_for_scc,
    coverage_record, fact_cone_for_scc, frame_bound, inherited_facts_for_singleton,
};
use crate::engine::l4::cone_derived::{
    ConeDerivedBuilder, ConeDerivedStore, ConeScope, DerivedFold, FoldedRow, ResInterner, RowFolder,
};
use crate::engine::l4::inherited_facts::InheritedBuilder;
use crate::engine::l4::scc::SccResult;
use crate::engine::l5::detector_context::cones_census as census;
use crate::program::model::workspace::ModelRoutine;

/// What the walk reads (all shared, read-only).
pub(super) struct Inputs<'a, 'g> {
    pub(super) g: &'g TypedEdgeGraph,
    pub(super) scc: &'a SccResult,
    pub(super) succ: &'a [BTreeSet<usize>],
    pub(super) direct: &'a RoutineDirectFacts,
    pub(super) direct_raw: &'a HashMap<String, Vec<CapabilityFact>>,
    pub(super) cov: &'a RoutineDirectCoverage,
    pub(super) routine_ids: &'a BTreeSet<String>,
    pub(super) routines_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
    pub(super) frames: &'a HashMap<String, crate::engine::l4::param_guard::FrameGuards>,
    pub(super) scope: &'a ConeScope,
    pub(super) resolved: &'a crate::engine::l4::pd_solve::ResolvedByRoutine<'g>,
}

impl Inputs<'_, '_> {
    fn own(&self, m: &str) -> &[crate::engine::l4::pd_solve::ResolvedEntry<'_>] {
        self.resolved.get(m).map_or(&[][..], Vec::as_slice)
    }
}

/// How the walk schedules: `threads` workers, and in tests the pop order and
/// an injected panic.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sched {
    pub(super) threads: usize,
    #[cfg(test)]
    pub(super) order: Order,
    /// The first task popped panics once every other worker is parked
    /// (guard 4).
    #[cfg(test)]
    pub(super) panic_first: bool,
}

/// Which ready SCC a free worker takes (tests; production takes the lowest).
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) enum Order {
    Lowest,
    Highest,
    Shuffled(u64),
}

impl Sched {
    pub(super) fn threads(threads: usize) -> Sched {
        Sched {
            threads: threads.max(1),
            #[cfg(test)]
            order: Order::Lowest,
            #[cfg(test)]
            panic_first: false,
        }
    }

    /// The pop priority of SCC `i` among `n` (lowest first).
    fn prio(&self, i: usize, n: usize) -> u64 {
        #[cfg(test)]
        match self.order {
            Order::Lowest => {}
            Order::Highest => return (n - 1 - i) as u64,
            Order::Shuffled(seed) => {
                // splitmix64 of (seed, i): a fixed, seed-dependent permutation.
                let mut z = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                return z ^ (z >> 31);
            }
        }
        let _ = n;
        i as u64
    }
}

#[cfg(test)]
thread_local! {
    /// A test's schedule for the walks this thread runs; it wins over
    /// `workers` (a walk with a cone store or raw members stays serial).
    pub(super) static TEST_SCHED: std::cell::Cell<Option<Sched>> =
        const { std::cell::Cell::new(None) };
}

/// The schedule of a walk that may run in parallel: `None` (the serial loop)
/// for one worker.
pub(super) fn sched_for(workers: usize) -> Option<Sched> {
    #[cfg(test)]
    if let Some(s) = TEST_SCHED.with(std::cell::Cell::get) {
        return Some(s);
    }
    (workers > 1).then(|| Sched::threads(workers))
}

/// What the walk produces, for `compose_inherited_cones`'s tail.
pub(super) struct Walked {
    pub(super) coverage: HashMap<String, CoverageRecord>,
    pub(super) derived: ConeDerivedBuilder,
    pub(super) stats: ComposeStats,
    /// Per SCC, under the census: its task's time and its kept cone's size.
    pub(super) scc_nanos: Vec<u64>,
    pub(super) cone_len: Vec<u64>,
}

/// A non-root SCC's cones, shared with its readers until the last one is done.
#[derive(Clone)]
struct Shared {
    fact: Arc<ConeFacts>,
    cov: Arc<CoverageCone>,
    /// The closed-set memo row (#160 step 7b §4.3), built by the first reader
    /// that wants it.
    memo: Arc<OnceLock<FoldedRow>>,
}

/// One SCC's results until the merge takes them.
struct SccOut {
    coverage: Vec<(String, CoverageRecord)>,
    rows: Vec<(String, FoldedRow)>,
    stats: ComposeStats,
    nanos: u64,
    cone_len: u64,
}

struct Slot {
    cone: Mutex<Option<Shared>>,
    /// Readers not finished; the cone is freed at 0.
    uses: AtomicUsize,
    /// Successors not done; the SCC is ready at 0.
    left: AtomicUsize,
    /// Set once the task stored its results. The start assert reads this,
    /// not `left` (the counter a readiness bug would corrupt).
    done: AtomicBool,
    out: Mutex<Option<SccOut>>,
}

struct Queue {
    ready: BinaryHeap<Reverse<(u64, usize)>>,
    finished: usize,
    abort: bool,
    /// Workers parked on the condvar (the panic test waits for them).
    waiting: usize,
}

struct Merge {
    store: ConeDerivedStore,
    coverage: HashMap<String, CoverageRecord>,
    stats: ComposeStats,
    /// The lowest SCC index not merged yet.
    next: usize,
    nanos: Vec<u64>,
    cone_len: Vec<u64>,
}

impl Merge {
    fn take(&mut self, i: usize, o: SccOut) {
        for (id, r) in o.coverage {
            self.coverage.insert(id, r);
        }
        for (id, row) in o.rows {
            self.store.push_row(id, &row);
        }
        self.stats.absorb(&o.stats);
        if !self.nanos.is_empty() {
            self.nanos[i] = o.nanos;
            self.cone_len[i] = o.cone_len;
        }
    }
}

struct Cx<'c, 'a, 'g> {
    x: &'c Inputs<'a, 'g>,
    sched: Sched,
    n: usize,
    readers: Vec<Vec<usize>>,
    slots: Vec<Slot>,
    queue: Mutex<Queue>,
    cv: Condvar,
    merge: Mutex<Merge>,
    interner: &'c ResInterner,
    /// Closed-set memo rows alive (the census's `memo_live_max`).
    live_memo: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

#[cfg(test)]
thread_local! {
    /// Parallel walks run by this thread, and the most tasks in flight at
    /// once in the last one (guards 1 and 3).
    pub(super) static PAR_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static LAST_MAX_IN_FLIGHT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Every resource id the walk can fold, interned before it: each fact of
/// every `direct_raw` key (keys sorted, facts in order). Cone entries are
/// `Arc` clones of `direct` reps (deduped from `direct_raw`), own entries are
/// producers' raw facts with only `temp_state` changed, and the SELF half
/// folds `direct_raw` itself. Not filtered by the fold's rules (a changed
/// `temp_state` can make a temp producer's table physical).
fn pre_intern(direct_raw: &HashMap<String, Vec<CapabilityFact>>) -> ResInterner {
    let mut keys: Vec<&String> = direct_raw.keys().collect();
    keys.sort();
    let mut interner = ResInterner::default();
    for k in keys {
        for f in &direct_raw[k] {
            if let Some(r) = f.resource_id.as_deref() {
                interner.intern(r);
            }
        }
    }
    interner
}

/// Run the walk (see the module doc).
pub(super) fn walk(x: &Inputs<'_, '_>, sched: Sched) -> Walked {
    let n = x.scc.sccs.len();
    let interner = pre_intern(x.direct_raw);
    let mut readers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, s) in x.succ.iter().enumerate() {
        for &y in s {
            readers[y].push(i);
        }
    }
    let slots: Vec<Slot> = (0..n)
        .map(|i| Slot {
            cone: Mutex::new(None),
            uses: AtomicUsize::new(readers[i].len()),
            left: AtomicUsize::new(x.succ[i].len()),
            done: AtomicBool::new(false),
            out: Mutex::new(None),
        })
        .collect();
    let ready = (0..n)
        .filter(|&i| x.succ[i].is_empty())
        .map(|i| Reverse((sched.prio(i, n), i)))
        .collect();
    let census_n = if census::enabled() { n } else { 0 };
    let cx = Cx {
        x,
        sched,
        n,
        readers,
        slots,
        queue: Mutex::new(Queue {
            ready,
            finished: 0,
            abort: false,
            waiting: 0,
        }),
        cv: Condvar::new(),
        merge: Mutex::new(Merge {
            store: ConeDerivedStore::default(),
            coverage: HashMap::new(),
            stats: ComposeStats::default(),
            next: 0,
            nanos: vec![0; census_n],
            cone_len: vec![0; census_n],
        }),
        interner: &interner,
        live_memo: AtomicUsize::new(0),
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    };
    std::thread::scope(|s| {
        for _ in 0..sched.threads {
            s.spawn(|| worker(&cx));
        }
    });
    #[cfg(test)]
    {
        PAR_WALKS.with(|c| c.set(c.get() + 1));
        LAST_MAX_IN_FLIGHT.with(|c| c.set(cx.max_in_flight.load(Ordering::Relaxed)));
    }
    let Cx {
        slots,
        merge,
        live_memo,
        ..
    } = cx;
    let m = merge.into_inner().expect("the merge lock is not poisoned");
    assert_eq!(m.next, n, "cone walk: {} of {n} SCCs merged", m.next);
    // ⟨C1 Task 4⟩ as the serial walk: every cone is freed by its last reader.
    debug_assert!(
        slots.iter().all(|s| {
            s.cone.lock().is_ok_and(|c| c.is_none()) && s.out.lock().is_ok_and(|o| o.is_none())
        }),
        "cone leak: a cone or a result survived the parallel SCC walk"
    );
    debug_assert_eq!(
        live_memo.into_inner(),
        0,
        "closed-set memo rows survived the parallel SCC walk"
    );
    Walked {
        coverage: m.coverage,
        derived: m.store.into_builder(interner),
        stats: m.stats,
        scc_nanos: m.nanos,
        cone_len: m.cone_len,
    }
}

/// Sets `abort` and wakes every worker when a task panics, so none waits
/// forever; the scope then re-raises the panic. A worker that later locks a
/// mutex the panic poisoned panics too: accepted, the first panic is the one
/// re-raised.
struct AbortOnPanic<'w, 'c, 'a, 'g>(&'w Cx<'c, 'a, 'g>);

impl Drop for AbortOnPanic<'_, '_, '_, '_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut q = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
            q.abort = true;
            drop(q);
            self.0.cv.notify_all();
        }
    }
}

fn worker(cx: &Cx<'_, '_, '_>) {
    loop {
        let i = {
            let mut q = cx.queue.lock().expect("the queue lock");
            loop {
                if q.abort || q.finished == cx.n {
                    return;
                }
                if let Some(Reverse((_, i))) = q.ready.pop() {
                    break i;
                }
                q.waiting += 1;
                q = cx.cv.wait(q).expect("the queue lock");
                q.waiting -= 1;
            }
        };
        let _abort = AbortOnPanic(cx);
        #[cfg(test)]
        if cx.sched.panic_first && cx.max_in_flight.load(Ordering::Relaxed) == 0 {
            // Guard 4: panic only once every other worker is parked.
            while cx.queue.lock().expect("the queue lock").waiting + 1 < cx.sched.threads {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("test: the task of SCC {i} panics");
        }
        let now = cx.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        cx.max_in_flight.fetch_max(now, Ordering::Relaxed);
        let (shared, out) = task(i, cx);
        cx.in_flight.fetch_sub(1, Ordering::Relaxed);
        let slot = &cx.slots[i];
        *slot.cone.lock().expect("a slot lock") = shared;
        *slot.out.lock().expect("a slot lock") = Some(out);
        slot.done.store(true, Ordering::Release);
        // Free and release before merging (review r1, sol and Opus): the merge
        // lock may wait behind a long prefix drain, and nothing on the
        // critical path should.
        // Free each successor's cone once its last reader (this one, maybe)
        // is done.
        for &y in &cx.x.succ[i] {
            if cx.slots[y].uses.fetch_sub(1, Ordering::AcqRel) == 1 {
                let taken = cx.slots[y].cone.lock().expect("a slot lock").take();
                if taken.is_some_and(|s| s.memo.get().is_some()) {
                    cx.live_memo.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
        let newly: Vec<usize> = cx.readers[i]
            .iter()
            .copied()
            .filter(|&p| cx.slots[p].left.fetch_sub(1, Ordering::AcqRel) == 1)
            .collect();
        let pushed = newly.len();
        let all_done = {
            let mut q = cx.queue.lock().expect("the queue lock");
            for p in newly {
                q.ready.push(Reverse((cx.sched.prio(p, cx.n), p)));
            }
            q.finished += 1;
            q.finished == cx.n
        };
        // One wakeup per SCC made ready; everyone at the end (the abort path
        // wakes everyone itself).
        if all_done {
            cx.cv.notify_all();
        } else {
            for _ in 0..pushed {
                cx.cv.notify_one();
            }
        }
        // Merge the finished prefix. Whoever stores `out[i]` then locks, so
        // `i` is merged by it or by an earlier holder; the scope joins every
        // worker and `walk` asserts `next == n` after it.
        let mut m = cx.merge.lock().expect("the merge lock");
        while m.next < cx.n {
            let next = m.next;
            let Some(o) = cx.slots[next].out.lock().expect("a slot lock").take() else {
                break;
            };
            m.take(next, o);
            m.next += 1;
        }
    }
}

/// A task's view of its successors' memo rows ([`MemoRows`]).
struct SuccMemo<'w> {
    rows: HashMap<usize, Arc<OnceLock<FoldedRow>>>,
    live: &'w AtomicUsize,
}

impl MemoRows for SuccMemo<'_> {
    fn row(&mut self, j: usize, build: impl FnOnce() -> FoldedRow) -> &FoldedRow {
        let cell = self
            .rows
            .get(&j)
            .unwrap_or_else(|| panic!("cone walk: no memo cell for callee SCC {j}"));
        let mut built = false;
        let row = cell.get_or_init(|| {
            built = true;
            build()
        });
        if built {
            // The peak from this increment itself (review r1, sol): a later
            // sample can miss it.
            let now = self.live.fetch_add(1, Ordering::Relaxed) + 1;
            if census::enabled() {
                census::max(&census::CLOSED_MEMO_LIVE_MAX, now as u64);
            }
        }
        row
    }

    fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }
}

/// SCC `i`'s work: the serial loop's body for a re-folded SCC with no cone
/// store and no raw member. Returns its shared cones (none for a root: no
/// reader) and its results.
fn task(i: usize, cx: &Cx<'_, '_, '_>) -> (Option<Shared>, SccOut) {
    let x = cx.x;
    let t_scc = census::start();
    let scc_entry = &x.scc.sccs[i];
    let succ_ids = &x.succ[i];
    let mut cones: ConeMap = HashMap::with_capacity(succ_ids.len());
    let mut covs: CovMap = HashMap::with_capacity(succ_ids.len());
    let mut memos: HashMap<usize, Arc<OnceLock<FoldedRow>>> =
        HashMap::with_capacity(succ_ids.len());
    for &y in succ_ids {
        assert!(
            cx.slots[y].done.load(Ordering::Acquire),
            "cone walk: SCC {i} started before its successor {y} was done"
        );
        let s = cx.slots[y]
            .cone
            .lock()
            .expect("a slot lock")
            .clone()
            .unwrap_or_else(|| panic!("cone walk: successor {y} of SCC {i} has no cone"));
        cones.insert(y, s.fact);
        covs.insert(y, s.cov);
        memos.insert(y, s.memo);
    }
    // A root (nothing calls it) keeps no cone: nobody would read it.
    let root_scc = cx.readers[i].is_empty();
    let mut stats = ComposeStats::default();
    let fcone: Option<ConeFacts> = (!root_scc).then(|| {
        let t = census::start();
        let anchor = match scc_entry.members.as_slice() {
            [m] if !scc_entry.recursive => Some(GuardAnchor {
                direct_raw: x.direct_raw.get(m).map(|v| v.as_slice()).unwrap_or(&[]),
                out_edges: x.g.outgoing.get(m).map(|v| v.as_slice()).unwrap_or(&[]),
                scc_id_by_routine: &x.scc.scc_id_by_routine,
                caller: x.routines_by_id.get(m.as_str()).copied(),
                frame: x.frames.get(m),
                routines_by_id: x.routines_by_id,
            }),
            _ => None,
        };
        let fcone = fact_cone_for_scc(&scc_entry.members, succ_ids, &cones, x.direct, anchor);
        census::add_gated(&census::FACTCONE_ENTRIES, fcone.len() as u64);
        census::add_since(&census::FACTCONE_NANOS, t);
        census::add(&census::FACTCONE_CALLS, 1);
        fcone
    });
    let t = census::start();
    let ccone = coverage_cone_for_scc(
        &scc_entry.members,
        succ_ids,
        &covs,
        x.cov,
        &x.g.unresolved_sources,
    );
    census::add_since(&census::COVERAGE_NANOS, t);
    census::max(&census::MAX_SCC_MEMBERS, scc_entry.members.len() as u64);
    let recursive = scc_entry.recursive;
    let emitted = if recursive {
        let n = scc_entry.members.len() as u64;
        let emitted = scc_entry
            .members
            .iter()
            .filter(|m| x.routine_ids.contains(*m))
            .count() as u64;
        stats.recursive_sccs += 1;
        stats.recursive_members_all += n;
        stats.recursive_members_emitted += emitted;
        stats.recursive_roots += u64::from(root_scc);
        stats.self_loops += u64::from(n == 1);
        stats.max_recursive_members = stats.max_recursive_members.max(n);
        stats.size_hist[match n {
            1 => 0,
            2 => 1,
            3..=9 => 2,
            10..=99 => 3,
            _ => 4,
        }] += 1;
        stats.with_successor += u64::from(!succ_ids.is_empty());
        if cfg!(test) || census::enabled() {
            stats.with_guarded_successor += u64::from(
                succ_ids
                    .iter()
                    .any(|y| cones.get(y).is_some_and(|c| c.values().any(frame_bound))),
            );
        }
        emitted
    } else {
        0
    };
    // §4.2 step 1: one set per recursive SCC with an emitted member. The
    // serial loop's `.any(|m| routine_ids.contains(m) && refold(m))` is
    // `emitted > 0` here: with no seed every member re-folds.
    let use_set = recursive && emitted > 0 && x.scope.derived;
    let t = use_set.then(census::start).flatten();
    let root_set = (use_set && root_scc)
        .then(|| fact_cone_for_scc(&scc_entry.members, succ_ids, &cones, x.direct, None));
    census::add_since(&census::BFS_WALK_NANOS, t);
    let scc_set: Option<&ConeFacts> = if !use_set {
        None
    } else if let Some(set) = &root_set {
        stats.scc_sets_built += 1;
        Some(set)
    } else {
        stats.scc_sets_borrowed += 1;
        Some(fcone.as_ref().expect("a non-root SCC has its fact cone"))
    };
    let mut folder = RowFolder::new(cx.interner);
    let set_row = scc_set.map(|set| {
        let t = census::start();
        let row = folder.fold_set(set.values().map(|e| &*e.rep));
        stats.scc_set_folds += 1;
        census::add_since(&census::BFS_FOLD_NANOS, t);
        row
    });
    let mut memo = SuccMemo {
        rows: memos,
        live: &cx.live_memo,
    };
    // Never written: no member here wants raw (the driver's condition).
    let mut raw = InheritedBuilder::default();
    let mut coverage: Vec<(String, CoverageRecord)> = Vec::new();
    let mut rows: Vec<(String, FoldedRow)> = Vec::new();
    for m in &scc_entry.members {
        if !x.routine_ids.contains(m) {
            continue;
        }
        let mode = x.scope.member(m);
        debug_assert!(
            !mode.wants_raw(),
            "the parallel walk got a raw member {m:?}"
        );
        let t = census::start();
        let record = coverage_record(m, x.cov, &ccone);
        census::add_since(&census::EMIT_NANOS, t);
        if !mode.wants_derived() {
            coverage.push((m.clone(), record));
            continue;
        }
        folder.begin_routine();
        let own = x.own(m);
        if recursive {
            stats.own_entries_recursive += own.len() as u64;
            stats.members_with_own_entry += u64::from(!own.is_empty());
            let t = census::start();
            if let (Some(set), Some(row)) = (scc_set, &set_row) {
                folder.fold_row(row);
                for entry in own {
                    folder.fold_fact(&entry.rep);
                }
                stats.scc_set_entries_folded += set.len() as u64;
            }
            census::add_since(&census::BFS_FOLD_NANOS, t);
            census::add(&census::BFS_CALLS, 1);
        } else {
            let t = census::start();
            let picks = inherited_facts_for_singleton(
                m,
                x.g,
                &x.scc.scc_id_by_routine,
                &cones,
                x.routines_by_id.get(m.as_str()).copied(),
                x.frames.get(m),
                x.routines_by_id,
                own,
                mode,
                &mut folder,
                &mut memo,
                &mut stats,
                &mut raw,
            );
            debug_assert!(picks.is_empty(), "a derived-only member emitted raw picks");
            census::add_since(&census::SINGLETON_NANOS, t);
            census::add(&census::SINGLETON_CALLS, 1);
        }
        // The SELF half: the routine's raw direct facts.
        let t = census::start();
        if let Some(own) = x.direct_raw.get(m) {
            for f in own {
                folder.fold_fact(f);
            }
        }
        rows.push((m.clone(), folder.finish_routine()));
        census::add_since(&census::DERIVED_NANOS, t);
        let t = census::start();
        coverage.push((m.clone(), record));
        census::add_since(&census::EMIT_NANOS, t);
    }
    // As the serial loop: the set row's and the root set's destruction is
    // recursive fold work. (A memo row's destruction is not charged here:
    // it drops with its cone's last clone, on whichever worker that is.)
    let t = census::start();
    drop(set_row);
    drop(root_set);
    census::add_since(&census::BFS_FOLD_NANOS, t);
    let cone_len = fcone.as_ref().map_or(0, |c| c.len() as u64);
    let shared = fcone.map(|f| Shared {
        fact: Arc::new(f),
        cov: Arc::new(ccone),
        memo: Arc::new(OnceLock::new()),
    });
    let nanos = t_scc.map_or(0, |t| t.elapsed().as_nanos() as u64);
    (
        shared,
        SccOut {
            coverage,
            rows,
            stats,
            nanos,
            cone_len,
        },
    )
}

#[cfg(test)]
#[path = "cone_walk_par_tests.rs"]
mod tests;
