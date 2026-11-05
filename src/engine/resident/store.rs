//! Artifact snapshots and `ensure_prepared` (context-server design §4.2).
//!
//! A revision's analysis artifacts live in immutable [`Snapshot`]s. A request
//! obtains a **ready** snapshot holding everything it needs before it
//! computes: [`Store::ensure_prepared`] fills what is missing, phase by
//! phase, and returns the snapshot it published or found. Fills and
//! evictions publish NEW snapshots; a snapshot a request holds is never
//! modified, so a request computes over one consistent set of artifacts
//! whatever happens to the store meanwhile.
//!
//! The registry keeps per key: a monotonic **generation** (an eviction
//! increments it), a state (`Absent`, `Filling`, `Published`, `Failed`) and
//! the number of requests that pinned the key for assembly (eviction skips
//! pinned keys).
//!
//! **Publication** is one transaction per phase, under the registry lock and
//! then the snapshot lock (always in that order): a value whose key's
//! generation moved since its claim (an eviction happened) is discarded, its
//! claim released and the key claimed again on the next iteration (at most
//! [`MAX_DISCARDS`] times per key, then "artifact churn"); the other values
//! are merged into the CURRENT snapshot, so two requests publishing disjoint
//! keys never lose each other's entries. Only after the swap are the keys
//! marked `Published` and the waiters woken.
//!
//! **A request never waits while it holds a claim.** Each iteration of a
//! phase claims what nobody fills, joins what another request fills,
//! computes and publishes its claims, and only then waits for the joined
//! keys. (Waiting first let two requests each wait on a key the other held
//! claimed: a joined key's fill ended and a request that read the snapshot
//! before it was published claimed it again, then joined one of the
//! waiter's claims.) A waiter holds nothing, so no wait is ever on a
//! waiter, and no cycle of waits can form.
//!
//! **A request judges what it lacks against the snapshot read under the
//! registry lock.** Every writer of the snapshot holds that lock, so there
//! the snapshot and the keys' states agree: a key that is `Published` is in
//! the snapshot read. (Read before the lock, the snapshot could predate a
//! publication, and the request claimed and filled a published key again.)
//!
//! **Reservation** (§4.5, stage 3): each claimed key's fill reserves through
//! [`Control::reserve`] just before it computes and holds the guard until
//! its value is in hand; the guard's `finish` gives the bytes the fill left
//! resident, kept with the published entry ([`Snapshot::bytes`]) as the
//! eviction's sizes. A refused reservation releases the claims (below).
//!
//! **Every failure path releases the claims and wakes the waiters**: a
//! cancelled or expired request, a failed reservation, a fill that failed or
//! panicked, artifact churn. A waiter that is woken by a released claim
//! re-enters its own loop and claims the key itself.
//!
//! Stated difference from the design's text: stage 2 has no append-only
//! tables yet (the cone and tree stores of stage 5 bring them), so a snapshot
//! carries no `lens` and publication has no epoch branch. The rebase loop of
//! §4.2 is a lock here: publication merges into the current snapshot under
//! the snapshot lock, which is the loop's result without its retries.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How often a discarded value's key is claimed again before the request
/// fails with "artifact churn" (§4.2 step 3d).
pub const MAX_DISCARDS: usize = 3;

/// How long a waiter sleeps between looks at its cancel flag.
const WAIT_SLICE: Duration = Duration::from_millis(20);

/// A key of the store: its phase fixes the order of fills (a key's
/// prerequisites are in earlier phases).
pub trait StoreKey: Clone + Ord + Hash + Debug + Send + Sync {
    /// 0-based; every prerequisite of a key is in a lower phase.
    fn phase(&self) -> usize;
    /// The key's kind, as a reservation names its case (`budget::Case`).
    fn kind(&self) -> &'static str {
        "artifact"
    }
}

/// A held reservation (§4.5): released when dropped. `finish` is called
/// once the fill it covers has its value, and returns the bytes the fill
/// left resident (recorded with the published entry, the eviction sizes).
pub trait Held: Send {
    fn finish(self: Box<Self>) -> u64 {
        0
    }
}

impl Held for () {}

/// A reservation's guard.
pub type Guard<'a> = Box<dyn Held + 'a>;

/// A value of the store, and what a request requires of it (a det of a
/// wide enough mask, for example).
pub trait StoreValue: Send + Sync {
    type Req: Clone + Debug + Send + Sync;
    /// Whether this published value satisfies `req`. Called under the
    /// store's registry lock (a request judges its claims there), so it
    /// must be cheap and must not touch the store.
    fn satisfies(&self, req: &Self::Req) -> bool;
}

/// One immutable set of published artifacts.
pub struct Snapshot<K, V> {
    entries: BTreeMap<K, Published<V>>,
}

/// A published value, the generation it was filled at, and the bytes its
/// fill left resident (0 when unmeasured).
pub struct Published<V> {
    pub value: Arc<V>,
    pub generation: u64,
    pub bytes: u64,
}

impl<V> Clone for Published<V> {
    fn clone(&self) -> Self {
        Published {
            value: Arc::clone(&self.value),
            generation: self.generation,
            bytes: self.bytes,
        }
    }
}

impl<K: StoreKey, V: StoreValue> Snapshot<K, V> {
    fn empty() -> Self {
        Snapshot {
            entries: BTreeMap::new(),
        }
    }

    /// The value published for `key`.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<&Arc<V>> {
        self.entries.get(key).map(|p| &p.value)
    }

    /// Whether the snapshot holds a value for `key` satisfying `req`.
    #[must_use]
    pub fn satisfies(&self, key: &K, req: &V::Req) -> bool {
        self.entries
            .get(key)
            .is_some_and(|p| p.value.satisfies(req))
    }

    /// The keys present.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    /// The bytes `key`'s fill left resident (0 when absent or unmeasured).
    #[must_use]
    pub fn bytes(&self, key: &K) -> u64 {
        self.entries.get(key).map_or(0, |p| p.bytes)
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the snapshot is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A key's state in the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Absent,
    Filling { worker: u64 },
    Published,
    Failed(String),
}

#[derive(Debug)]
struct Entry {
    generation: u64,
    state: State,
    /// Requests that pinned the key for assembly.
    pins: usize,
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            generation: 0,
            state: State::Absent,
            pins: 0,
        }
    }
}

/// Why `ensure_prepared` returned no snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareError {
    /// The request was cancelled (while waiting or between phases).
    Cancelled,
    /// The request's deadline passed.
    Deadline,
    /// The reservation for a fill failed (§4.5).
    Budget(String),
    /// A fill failed, or a key churned past [`MAX_DISCARDS`].
    Engine(String),
}

/// Why a request stops: its flag, read by the store between steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    Deadline,
}

impl From<Stop> for PrepareError {
    fn from(s: Stop) -> Self {
        match s {
            Stop::Cancelled => PrepareError::Cancelled,
            Stop::Deadline => PrepareError::Deadline,
        }
    }
}

/// A request's controls over its preparation.
pub struct Control<'a> {
    /// `Some` once the request must stop.
    pub stop: &'a (dyn Fn() -> Option<Stop> + Sync),
    /// The reservation before a fill, or before a request's trees (§4.5):
    /// the key's kind (`"trees"` for trees) and the work's size in roots (0
    /// when unsized: a revision-wide artifact), by which the reserver sizes
    /// its estimate (`budget::size_class`); `Err(why)` refuses it.
    pub reserve: &'a (dyn Fn(&'static str, usize) -> Result<Guard<'a>, String> + Sync),
}

/// A reservation that always succeeds and holds nothing.
pub fn unreserved_guard<'a>(_: &'static str, _: usize) -> Result<Guard<'a>, String> {
    Ok(Box::new(()))
}

impl Control<'static> {
    /// A request that never stops and whose reservations always succeed.
    #[must_use]
    pub fn unreserved() -> Control<'static> {
        Control {
            stop: &|| None,
            reserve: &unreserved_guard,
        }
    }
}

/// What a fill reads: the key, the requirement, the value it replaces (a det
/// refilled at a wider mask), and the snapshot its prerequisites are in.
pub struct FillCx<'a, K, V: StoreValue> {
    pub key: &'a K,
    pub req: &'a V::Req,
    pub existing: Option<&'a Arc<V>>,
    pub snapshot: &'a Snapshot<K, V>,
}

/// A store's fill: a claimed key's value off the snapshot holding its
/// prerequisites, or why it failed.
pub type Fill<'f, K, V> = dyn Fn(FillCx<'_, K, V>) -> Result<V, String> + Sync + 'f;

/// The store's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub fills: u64,
    pub joins: u64,
    pub discards: u64,
    pub evictions: u64,
    pub failures: u64,
    pub publications: u64,
}

/// A revision's artifact store (see the module doc).
pub struct Store<K: StoreKey, V: StoreValue> {
    snapshot: Mutex<Arc<Snapshot<K, V>>>,
    registry: Mutex<HashMap<K, Entry>>,
    changed: Condvar,
    stats: Mutex<StoreStats>,
    next_worker: AtomicU64,
    /// Tests only: called at the top of every claim section, before the
    /// registry lock is taken (and so before the snapshot the claims are
    /// judged against is read). It lets a test hold a request exactly
    /// there while another request publishes, the interleaving that made a
    /// stale snapshot reclaim a published key (2026-11-04). No callback
    /// runs at that point otherwise, so a test cannot reach it any other
    /// way; the field does not exist outside `cfg(test)`.
    #[cfg(test)]
    before_claim: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl<K: StoreKey, V: StoreValue> Default for Store<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// The assembly pins of one request, released on drop (every path).
struct Pins<'a, K: StoreKey, V: StoreValue> {
    store: &'a Store<K, V>,
    keys: Vec<K>,
}

impl<K: StoreKey, V: StoreValue> Drop for Pins<'_, K, V> {
    fn drop(&mut self) {
        let mut reg = self.store.registry();
        for k in &self.keys {
            if let Some(e) = reg.get_mut(k) {
                e.pins = e.pins.saturating_sub(1);
            }
        }
    }
}

/// The claims of one phase iteration, released on drop unless published or
/// failed (every early return releases them and wakes the waiters).
struct Claims<'a, K: StoreKey, V: StoreValue> {
    store: &'a Store<K, V>,
    worker: u64,
    /// Each claimed key and the generation it was claimed at.
    keys: Vec<(K, u64)>,
}

impl<K: StoreKey, V: StoreValue> Drop for Claims<'_, K, V> {
    fn drop(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        let mut reg = self.store.registry();
        for (k, _) in &self.keys {
            if let Some(e) = reg.get_mut(k)
                && e.state
                    == (State::Filling {
                        worker: self.worker,
                    })
            {
                e.state = State::Absent;
            }
        }
        drop(reg);
        self.store.changed.notify_all();
    }
}

impl<K: StoreKey, V: StoreValue> Store<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Store {
            snapshot: Mutex::new(Arc::new(Snapshot::empty())),
            registry: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
            stats: Mutex::new(StoreStats::default()),
            next_worker: AtomicU64::new(1),
            #[cfg(test)]
            before_claim: std::sync::OnceLock::new(),
        }
    }

    fn registry(&self) -> MutexGuard<'_, HashMap<K, Entry>> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn cell(&self) -> MutexGuard<'_, Arc<Snapshot<K, V>>> {
        self.snapshot.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn count(&self, f: impl FnOnce(&mut StoreStats)) {
        f(&mut self.stats.lock().unwrap_or_else(|p| p.into_inner()));
    }

    /// The current snapshot.
    #[must_use]
    pub fn load(&self) -> Arc<Snapshot<K, V>> {
        Arc::clone(&self.cell())
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> StoreStats {
        self.stats.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The generation of `key` (0 until first evicted).
    #[must_use]
    pub fn generation(&self, key: &K) -> u64 {
        self.registry().get(key).map_or(0, |e| e.generation)
    }

    /// Evict `keys` (§4.2 "Eviction"): each key's generation is incremented,
    /// so a fill in flight cannot publish its value, and a snapshot without
    /// the entries is published. Keys pinned for assembly are skipped.
    /// Returns how many keys were evicted.
    pub fn evict(&self, keys: &[K]) -> usize {
        let mut reg = self.registry();
        let mut gone = Vec::new();
        for k in keys {
            let e = reg.entry(k.clone()).or_default();
            if e.pins > 0 {
                continue;
            }
            e.generation += 1;
            if matches!(e.state, State::Published | State::Failed(_)) {
                e.state = State::Absent;
            }
            gone.push(k.clone());
        }
        // Nothing to evict (every key pinned): publish nothing. A new
        // snapshot with the same entries would only add a second owner to
        // every value while a request holds the old one, so no later reclaim
        // could free any of them (`evict_unheld` counts owners).
        if gone.is_empty() {
            return 0;
        }
        let mut cell = self.cell();
        let mut entries = cell.entries.clone();
        for k in &gone {
            entries.remove(k);
        }
        *cell = Arc::new(Snapshot { entries });
        drop(cell);
        drop(reg);
        self.count(|s| s.evictions += gone.len() as u64);
        self.changed.notify_all();
        gone.len()
    }

    /// Publish `value` for `key` outside a fill (an artifact made where its
    /// inputs existed: the sited graph, agent-tasks design §1.2 M0-1, seeded
    /// by the revision's build or added later from a program over the
    /// revision's own read). It is `Published` like a filled key, with
    /// `bytes` as its measured size, merged into the current snapshot (every
    /// other entry kept). Panics if `key` is already present or filling: a
    /// seeded key is never filled, and its seeder checks it is absent.
    pub fn seed(&self, key: K, value: V, bytes: u64) {
        let mut reg = self.registry();
        let e = reg.entry(key.clone()).or_default();
        assert!(
            e.state == State::Absent,
            "{key:?} is seeded once, and only while absent"
        );
        e.state = State::Published;
        let generation = e.generation;
        let mut cell = self.cell();
        let mut entries = cell.entries.clone();
        entries.insert(
            key,
            Published {
                value: Arc::new(value),
                generation,
                bytes,
            },
        );
        *cell = Arc::new(Snapshot { entries });
        drop(cell);
        drop(reg);
        self.count(|s| s.publications += 1);
    }

    /// Pin `keys` for as long as the returned guard lives: a request holds
    /// its needs through its compute (§4.2 step 4, "keep it pinned for the
    /// compute"), so a reclaim never evicts what it reads.
    pub fn hold(&self, keys: &[K]) -> impl Drop + '_ {
        let mut reg = self.registry();
        for k in keys {
            reg.entry(k.clone()).or_default().pins += 1;
        }
        Pins {
            store: self,
            keys: keys.to_vec(),
        }
    }

    /// Evict for memory (§4.5 reclaim): [`Store::evict`] of the keys whose
    /// value no request holds — not pinned (for assembly or a compute,
    /// [`Store::hold`]), and held by no older snapshot. Evicting a value a
    /// request still reads frees nothing until that request ends, and would
    /// only cost the next request its refill. Returns how many.
    pub fn evict_unheld(&self, keys: &[K]) -> usize {
        let free: Vec<K> = {
            let cell = self.cell();
            keys.iter()
                .filter(|k| {
                    cell.entries
                        .get(*k)
                        .is_some_and(|p| Arc::strong_count(&p.value) == 1)
                })
                .cloned()
                .collect()
        };
        if free.is_empty() {
            return 0;
        }
        self.evict(&free)
    }

    /// Invalidate `keys` whatever their pins (a tombstone): each generation
    /// is incremented, so a fill in flight is discarded at publication and
    /// refilled, and the entries leave the current snapshot. A snapshot a
    /// request already holds keeps its values. Eviction under memory
    /// pressure is [`Store::evict`], which never touches pinned keys; this is
    /// for a value that must not be published (its inputs are gone).
    pub fn tombstone(&self, keys: &[K]) {
        let mut reg = self.registry();
        for k in keys {
            let e = reg.entry(k.clone()).or_default();
            e.generation += 1;
            if matches!(e.state, State::Published | State::Failed(_)) {
                e.state = State::Absent;
            }
        }
        let mut cell = self.cell();
        let mut entries = cell.entries.clone();
        for k in keys {
            entries.remove(k);
        }
        *cell = Arc::new(Snapshot { entries });
        drop(cell);
        drop(reg);
        self.changed.notify_all();
    }

    /// A ready snapshot satisfying every `(key, requirement)` of `needs`
    /// (the closure of the request's needs: every prerequisite listed), or
    /// why there is none (§4.2). `units` is the size in roots of one key's
    /// fill, handed to its reservation ([`Control::reserve`]); `fill`
    /// computes a claimed key's value off the snapshot holding its
    /// prerequisites.
    pub fn ensure_prepared(
        &self,
        needs: &[(K, V::Req)],
        units: &dyn Fn(&K) -> usize,
        fill: &Fill<'_, K, V>,
        ctl: &Control<'_>,
    ) -> Result<Arc<Snapshot<K, V>>, PrepareError> {
        let worker = self.next_worker.fetch_add(1, Ordering::Relaxed);
        // (2) Pin every key of the plan for assembly.
        let _pins = {
            let mut reg = self.registry();
            for (k, _) in needs {
                reg.entry(k.clone()).or_default().pins += 1;
            }
            Pins {
                store: self,
                keys: needs.iter().map(|(k, _)| k.clone()).collect(),
            }
        };
        let last_phase = needs.iter().map(|(k, _)| k.phase()).max();
        let mut discards: HashMap<K, usize> = HashMap::new();
        // (3) Phase by phase.
        for phase in 0..=last_phase.unwrap_or(0) {
            let mine: Vec<&(K, V::Req)> =
                needs.iter().filter(|(k, _)| k.phase() == phase).collect();
            if mine.is_empty() {
                continue;
            }
            loop {
                if let Some(stop) = (ctl.stop)() {
                    return Err(stop.into());
                }
                // (a) What the current snapshot lacks: claim or join it. The
                // snapshot is read UNDER the registry lock: every writer of
                // the snapshot (publish, evict, tombstone) holds that lock,
                // so the snapshot and the keys' states agree here. Read
                // before the lock, a snapshot could predate a publication
                // whose key is already `Published`, and the request claimed
                // and filled that key again for nothing (a duplicate L4
                // solve or fold; the same stale read made b41a3b01's
                // witness fill fail).
                #[cfg(test)]
                if let Some(hook) = self.before_claim.get() {
                    hook();
                }
                let mut claims = Claims {
                    store: self,
                    worker,
                    keys: Vec::new(),
                };
                let mut joined: Vec<K> = Vec::new();
                let missing: Vec<&(K, V::Req)>;
                {
                    let mut reg = self.registry();
                    let s = self.load();
                    missing = mine
                        .iter()
                        .copied()
                        .filter(|(k, r)| !s.satisfies(k, r))
                        .collect();
                    if missing.is_empty() {
                        break;
                    }
                    for (k, _) in &missing {
                        let e = reg.entry(k.clone()).or_default();
                        match &e.state {
                            State::Filling { worker: w } if *w != worker => joined.push(k.clone()),
                            _ => {
                                e.state = State::Filling { worker };
                                claims.keys.push((k.clone(), e.generation));
                            }
                        }
                    }
                }
                if !joined.is_empty() {
                    self.count(|s| s.joins += joined.len() as u64);
                }
                // (b) Reserve, then compute every claimed key off the current
                // snapshot (its prerequisites were published by earlier
                // phases; a joined key is of this phase, so no claimed key
                // reads one), and (c) publish them, BEFORE waiting for any
                // joined key: a request never waits while it holds a claim.
                // Waiting first deadlocked: this request waits on a joined
                // key, holding its claims; the key's fill ends and a request
                // that read the snapshot before it was published claims it
                // again (or widens it, a det), then joins one of this
                // request's claims; each waits on the other forever.
                if !claims.keys.is_empty() {
                    let s = self.load();
                    let mut values: Vec<(K, u64, V, u64)> = Vec::new();
                    for (k, generation) in claims.keys.clone() {
                        let (k, generation) = (&k, &generation);
                        // Each fill holds its reservation while it computes (a
                        // fill worker), released once its value is in hand.
                        let guard = match (ctl.reserve)(k.kind(), units(k)) {
                            Ok(g) => g,
                            // `claims` drops: released, waiters woken.
                            Err(why) => return Err(PrepareError::Budget(why)),
                        };
                        let req = &missing
                            .iter()
                            .find(|(m, _)| m == k)
                            .expect("a claimed key is missing")
                            .1;
                        let cx = FillCx {
                            key: k,
                            req,
                            existing: s.get(k),
                            snapshot: &s,
                        };
                        let filled =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill(cx)))
                                .unwrap_or_else(|p| Err(panic_text(&p)));
                        match filled {
                            Ok(v) => values.push((k.clone(), *generation, v, guard.finish())),
                            Err(why) => {
                                self.fail(k, worker, &why);
                                claims.keys.retain(|(c, _)| c != k);
                                return Err(PrepareError::Engine(format!(
                                    "filling {k:?} failed: {why}"
                                )));
                            }
                        }
                        if let Some(stop) = (ctl.stop)() {
                            // A computed value is still published below only
                            // if the request goes on; a stopped request
                            // releases.
                            return Err(stop.into());
                        }
                    }
                    drop(s);
                    self.count(|st| st.fills += values.len() as u64);
                    // Publish as one transaction.
                    let churned = self.publish(values, worker, &mut discards);
                    claims.keys.clear();
                    if let Some(k) = churned {
                        return Err(PrepareError::Engine(format!(
                            "artifact churn: {k:?} was evicted {MAX_DISCARDS} times while filled"
                        )));
                    }
                }
                // (d) Wait for every joined key, holding no claim (the fill
                // goes on for others whatever this request does).
                drop(claims);
                self.wait_for(&joined, worker, ctl)?;
                // (e) Reload and repeat until the phase is satisfied.
            }
        }
        // (4) The ready snapshot.
        let s = self.load();
        debug_assert!(needs.iter().all(|(k, r)| s.satisfies(k, r)));
        if let Some((k, _)) = needs.iter().find(|(k, r)| !s.satisfies(k, r)) {
            // Evicted after its phase: pinned keys cannot be, so this is a
            // store invariant failure.
            return Err(PrepareError::Engine(format!(
                "{k:?} missing from the ready snapshot"
            )));
        }
        Ok(s)
    }

    /// Wait until no key of `joined` is being filled by another worker.
    fn wait_for(&self, joined: &[K], worker: u64, ctl: &Control<'_>) -> Result<(), PrepareError> {
        if joined.is_empty() {
            return Ok(());
        }
        let mut reg = self.registry();
        loop {
            let busy = joined.iter().any(|k| {
                matches!(reg.get(k).map(|e| &e.state),
                    Some(State::Filling { worker: w }) if *w != worker)
            });
            if !busy {
                return Ok(());
            }
            if let Some(stop) = (ctl.stop)() {
                return Err(stop.into());
            }
            reg = self
                .changed
                .wait_timeout(reg, WAIT_SLICE)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// A fill failed: the key is `Failed`, its waiters woken.
    fn fail(&self, key: &K, worker: u64, why: &str) {
        let mut reg = self.registry();
        if let Some(e) = reg.get_mut(key)
            && e.state == (State::Filling { worker })
        {
            e.state = State::Failed(why.to_string());
        }
        drop(reg);
        self.count(|s| s.failures += 1);
        self.changed.notify_all();
    }

    /// Publish `values` (each with the generation its claim saw) as one
    /// transaction; a value whose key's generation moved is discarded and its
    /// key released. Returns a key that churned past [`MAX_DISCARDS`].
    fn publish(
        &self,
        values: Vec<(K, u64, V, u64)>,
        worker: u64,
        discards: &mut HashMap<K, usize>,
    ) -> Option<K> {
        let mut reg = self.registry();
        let mut kept: Vec<(K, Published<V>)> = Vec::new();
        let mut churned = None;
        for (k, generation, v, bytes) in values {
            let e = reg.entry(k.clone()).or_default();
            if e.generation == generation {
                kept.push((
                    k,
                    Published {
                        value: Arc::new(v),
                        generation,
                        bytes,
                    },
                ));
                continue;
            }
            // An eviction happened since the claim: discard (§4.2 3d(i)).
            if e.state == (State::Filling { worker }) {
                e.state = State::Absent;
            }
            let n = discards.entry(k.clone()).or_default();
            *n += 1;
            self.count(|s| s.discards += 1);
            if *n >= MAX_DISCARDS {
                churned = Some(k);
            }
        }
        if !kept.is_empty() {
            let mut cell = self.cell();
            let mut entries = cell.entries.clone();
            for (k, p) in &kept {
                entries.insert(k.clone(), p.clone());
            }
            *cell = Arc::new(Snapshot { entries });
            drop(cell);
            // Only after the swap: Published, waiters woken.
            for (k, _) in &kept {
                if let Some(e) = reg.get_mut(k) {
                    e.state = State::Published;
                }
            }
            self.count(|s| s.publications += 1);
        }
        drop(reg);
        self.changed.notify_all();
        churned
    }
}

/// A panic payload's text.
fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a fill panicked".to_string())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
