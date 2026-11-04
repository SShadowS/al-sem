//! T-prepared (design §11, the store's publication invariants with cold
//! roots): `ensure_prepared` over a store of small fake artifacts, each test
//! stating its precondition by construction (which request holds which
//! claim when) and its outcome.

use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

/// A two-phase key set: `A` and `X`/`Y` in phase 0, `B` (reads `A`) in 1.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum K {
    A,
    X,
    Y,
    B,
}

impl StoreKey for K {
    fn phase(&self) -> usize {
        match self {
            K::A | K::X | K::Y => 0,
            K::B => 1,
        }
    }
}

/// A value: which fill made it (a count) and, for `B`, the `A` it read.
#[derive(Debug, PartialEq, Eq)]
struct V {
    made: usize,
    read_a: Option<usize>,
}

impl StoreValue for V {
    type Req = ();
    fn satisfies(&self, (): &()) -> bool {
        true
    }
}

fn needs(keys: &[K]) -> Vec<(K, ())> {
    keys.iter().map(|k| (k.clone(), ())).collect()
}

/// A fill that numbers its values and requires `A` before `B`.
fn numbered(counter: &AtomicUsize) -> impl Fn(FillCx<'_, K, V>) -> Result<V, String> + Sync + '_ {
    move |cx| {
        let made = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let read_a = match cx.key {
            K::B => Some(
                cx.snapshot
                    .get(&K::A)
                    .ok_or("B computed before its prerequisite A was published")?
                    .made,
            ),
            _ => None,
        };
        Ok(V { made, read_a })
    }
}

fn no_estimate(_: &K) -> usize {
    0
}

/// A gate a fill waits at. The test opens it by dropping its [`Opener`], so
/// a failed assertion (under a discrimination break) never leaves a fill
/// blocked and the test thread joined on it forever.
#[derive(Default)]
struct Gate(std::sync::atomic::AtomicBool);

impl Gate {
    fn wait(&self) {
        while !self.0.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn opener(&self) -> Opener<'_> {
        Opener(self)
    }
}

struct Opener<'a>(&'a Gate);

impl Drop for Opener<'_> {
    fn drop(&mut self) {
        self.0.0.store(true, Ordering::SeqCst);
    }
}

/// The first request on an empty store fills every key in phase order and
/// returns a snapshot holding them; a second request fills nothing.
#[test]
fn first_request_on_an_empty_snapshot_fills_in_phase_order() {
    let store: Store<K, V> = Store::new();
    assert!(store.load().is_empty(), "precondition: an empty store");
    let n = AtomicUsize::new(0);
    let s = store
        .ensure_prepared(
            &needs(&[K::B, K::A]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(s.get(&K::A).unwrap().made, 1);
    assert_eq!(s.get(&K::B).unwrap().read_a, Some(1));
    let again = store
        .ensure_prepared(
            &needs(&[K::B]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    assert!(Arc::ptr_eq(
        again.get(&K::B).unwrap(),
        s.get(&K::B).unwrap()
    ));
    assert_eq!(store.stats().fills, 2);
}

/// After an eviction the key is refilled at its new generation; a reader
/// that holds the old ready snapshot keeps its value (a paused reader keeps
/// its snapshot).
#[test]
fn after_eviction_the_key_is_refilled_and_a_paused_reader_keeps_its_snapshot() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let first = store
        .ensure_prepared(
            &needs(&[K::A]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(store.evict(&[K::A]), 1);
    assert_eq!(store.generation(&K::A), 1);
    assert!(
        store.load().get(&K::A).is_none(),
        "evicted from the current snapshot"
    );
    assert_eq!(
        first.get(&K::A).unwrap().made,
        1,
        "the paused reader's snapshot is intact"
    );
    let second = store
        .ensure_prepared(
            &needs(&[K::A]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(second.get(&K::A).unwrap().made, 2);
}

/// Two requests claiming disjoint keys fill them at the same time (each
/// fill waits for the other to start) and publish without losing each
/// other's entry.
///
/// DISCRIMINATION (2026-11-02): publishing the request's own values as the
/// whole new snapshot (a plain store, `entries` built empty instead of from
/// the current snapshot) fails: the second publication drops the first
/// request's key, so that request's ready snapshot lacks it (the store's
/// ready-snapshot assertion fires in its thread and the join unwrap fails).
#[test]
fn disjoint_keys_published_concurrently_both_survive() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let barrier = std::sync::Barrier::new(2);
    let fill = |cx: FillCx<'_, K, V>| {
        barrier.wait();
        numbered(&n)(cx)
    };
    std::thread::scope(|s| {
        let a = s.spawn(|| {
            store.ensure_prepared(&needs(&[K::X]), &no_estimate, &fill, &Control::unreserved())
        });
        let b = s.spawn(|| {
            store.ensure_prepared(&needs(&[K::Y]), &no_estimate, &fill, &Control::unreserved())
        });
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    let s = store.load();
    assert!(
        s.get(&K::X).is_some() && s.get(&K::Y).is_some(),
        "both entries survive: {:?}",
        s.keys().collect::<Vec<_>>()
    );
}

/// A fill and an eviction race on one key: the eviction lands while the fill
/// computes, so the filled value (made at the old generation) is discarded,
/// never published, and the key is claimed again and refilled at the new
/// generation.
///
/// DISCRIMINATION (2026-11-02): publishing without the generation check
/// (`if true` in place of `e.generation == generation`) publishes the
/// tombstoned value: "the discarded value is not published" fails (made 1).
#[test]
fn tombstone_rejected_publication_is_refilled_at_the_new_generation() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let gate = Gate::default();
    let fill = |cx: FillCx<'_, K, V>| {
        let v = numbered(&n)(cx);
        if v.as_ref().is_ok_and(|v| v.made == 1) {
            let _ = started_tx.send(());
            gate.wait();
        }
        v
    };
    let s = std::thread::scope(|sc| {
        let open = gate.opener();
        let h = sc.spawn(|| {
            store.ensure_prepared(&needs(&[K::A]), &no_estimate, &fill, &Control::unreserved())
        });
        started_rx.recv().unwrap();
        // The fill is in flight; its own request pinned A, so only a
        // tombstone (not a pressure eviction) can move its generation.
        store.tombstone(&[K::A]);
        drop(open);
        h.join().unwrap().unwrap()
    });
    let a = s.get(&K::A).unwrap();
    assert_eq!(a.made, 2, "the discarded value is not published");
    assert_eq!(store.stats().discards, 1);
    assert_eq!(store.load().entries[&K::A].generation, 1);
}

/// A joined prerequisite plus an independently claimed dependent: request 2
/// needs `A` and `B`; request 1 holds `A`'s fill. Request 2 joins `A` and may
/// claim and compute `B` only after `A` is published: its `B` reads request
/// 1's `A`.
///
/// DISCRIMINATION (2026-11-02): ending a phase that holds only keys other
/// workers fill without waiting for them (`if claims.keys.is_empty() {
/// break; }` before `wait_for`) lets request 2 go on to phase 1 with `A`
/// unpublished: its `B` fill fails "B computed before its prerequisite A was
/// published", and the test fails at request 2's unwrap. (Skipping only the
/// wait does NOT break it: the phase loop then spins until `A` is published,
/// because a phase ends only when the snapshot satisfies it.)
#[test]
fn a_joined_prerequisite_is_published_before_the_dependent_is_computed() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let gate = Gate::default();
    let fill = |cx: FillCx<'_, K, V>| {
        if *cx.key == K::A {
            let _ = started_tx.send(());
            gate.wait();
        }
        numbered(&n)(cx)
    };
    std::thread::scope(|sc| {
        let open = gate.opener();
        let r1 = sc.spawn(|| {
            store.ensure_prepared(&needs(&[K::A]), &no_estimate, &fill, &Control::unreserved())
        });
        started_rx.recv().unwrap();
        let r2 = sc.spawn(|| {
            store.ensure_prepared(
                &needs(&[K::A, K::B]),
                &no_estimate,
                &fill,
                &Control::unreserved(),
            )
        });
        // Request 2 has joined `A` before it is released.
        while store.stats().joins == 0 {
            std::thread::yield_now();
        }
        drop(open);
        let s1 = r1.join().unwrap().unwrap();
        let s2 = r2.join().unwrap().unwrap();
        let a = s1.get(&K::A).unwrap().made;
        assert_eq!(
            s2.get(&K::A).unwrap().made,
            a,
            "request 2 used request 1's A"
        );
        assert_eq!(s2.get(&K::B).unwrap().read_a, Some(a));
    });
    assert_eq!(store.stats().fills, 2, "A once, B once");
}

/// A request never waits while it holds a claim (the module doc): it
/// publishes what it claimed before it waits for what it joined. Request C
/// fills `Y` and is held at a gate for the whole test; request 1 needs `X`
/// and `Y`, so it claims `X` and joins `Y`; request 2 needs only `X`.
/// Request 2 answers while `Y` is still being filled, with request 1's `X`.
///
/// Before 2026-11-04 request 1 waited first, holding `X`, so request 2
/// waited on request 1 until `Y` was published. With a third request that
/// claimed the joined key again (it read the snapshot before the key was
/// published) and joined one of the waiter's claims, that was a deadlock:
/// `concurrent_requests_and_evictions_answer_the_cold_bytes` hung in 2 of
/// 300 runs on a loaded machine, its two heavy requests both in `wait_for`.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-04): waiting
/// for the joined keys before computing the claims (`self.wait_for(&joined,
/// worker, ctl)?;` moved back above the claimed keys' fills) fails "request
/// 2 answered while Y was filled" with `Deadline` (its watchdog: request 2
/// waits on request 1, which waits on the gate); reverted, it passes.
#[test]
fn a_request_publishes_its_claims_before_it_waits_for_a_joined_key() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let gate = Gate::default();
    let y_fills = AtomicUsize::new(0);
    let fill = |cx: FillCx<'_, K, V>| {
        if *cx.key == K::Y {
            y_fills.fetch_add(1, Ordering::SeqCst);
            let _ = started_tx.send(());
            gate.wait();
        }
        numbered(&n)(cx)
    };
    // Request 2's watchdog: a wait that never ends gives up with `deadline`.
    let start = std::time::Instant::now();
    let watchdog = || (start.elapsed() > Duration::from_secs(10)).then_some(Stop::Deadline);
    let waiter = Control {
        stop: &watchdog,
        reserve: &unreserved_guard,
    };
    std::thread::scope(|sc| {
        let open = gate.opener();
        let c = sc.spawn(|| {
            store.ensure_prepared(&needs(&[K::Y]), &no_estimate, &fill, &Control::unreserved())
        });
        started_rx.recv().unwrap();
        let r1 = sc.spawn(|| {
            store.ensure_prepared(
                &needs(&[K::X, K::Y]),
                &no_estimate,
                &fill,
                &Control::unreserved(),
            )
        });
        // Request 1 claimed `X` and joined `Y` in one registry section.
        while store.stats().joins == 0 {
            std::thread::yield_now();
        }
        assert!(
            store
                .registry()
                .get(&K::X)
                .is_some_and(|e| e.state != State::Absent),
            "precondition: request 1 claimed X"
        );
        let r2 = store.ensure_prepared(&needs(&[K::X]), &no_estimate, &fill, &waiter);
        let y_filling = matches!(
            store.registry().get(&K::Y).map(|e| &e.state),
            Some(State::Filling { .. })
        );
        drop(open);
        let s2 = r2.expect("request 2 answered while Y was filled");
        assert!(y_filling, "precondition: Y was still being filled");
        let s1 = r1.join().unwrap().unwrap();
        c.join().unwrap().unwrap();
        assert!(
            Arc::ptr_eq(s2.get(&K::X).unwrap(), s1.get(&K::X).unwrap()),
            "both requests read the published X"
        );
    });
    // Until 2026-11-04 `X` could be filled twice here (request 2 read the
    // snapshot just before request 1 published `X`, then claimed it); a
    // request now reads the snapshot under the registry lock, so it either
    // joins request 1's fill or sees the published `X`.
    assert_eq!(y_fills.load(Ordering::SeqCst), 1, "Y filled once, by C");
    assert_eq!(store.stats().fills, 2, "X filled once, Y once");
}

/// A request judges what it lacks against the snapshot it reads under the
/// registry lock, so it never claims a key that is already published.
/// Request 1 needs `X` and is held (the store's test hook) at the top of
/// its claim section, before the registry lock; request 2 then fills and
/// publishes `X`; request 1 goes on, finds `X` published and fills nothing.
///
/// Before 2026-11-04 request 1 read the snapshot BEFORE the hook's point
/// (before the lock): the stale snapshot lacked `X`, the registry said
/// `Published` (not `Filling`, so nothing to join), and request 1 claimed
/// `X` and filled it a second time: a duplicate L4 solve or fold in the
/// real store, and the stale read behind b41a3b01's witness failure. The
/// interleaving is a window between two of the store's own statements with
/// no callback in it, so only the hook can hold a request there.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-04): the
/// snapshot read (`let s = self.load();` and `missing`) moved back above
/// the hook, outside the registry lock, fails "X filled once" (2 fills);
/// reverted, it passes.
#[test]
fn a_request_never_claims_a_key_published_after_it_read_the_snapshot() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let calls = Arc::new(AtomicUsize::new(0));
    // Request 1's first claim section: it says it is held there, then waits
    // until the test opens `published` (dropped on every path, so a failed
    // assertion never leaves request 1 held and the scope joined on it).
    let (held_tx, held_rx) = mpsc::channel::<()>();
    let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct OpenOnDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for OpenOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    {
        let (calls, held_tx, published) = (calls.clone(), Mutex::new(held_tx), published.clone());
        let hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            // Only the first call (request 1's, spawned first) is held.
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = held_tx.lock().unwrap().send(());
                while !published.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
        assert!(store.before_claim.set(hook).is_ok());
    }
    let (s1, s2) = std::thread::scope(|sc| {
        let open = OpenOnDrop(published.clone());
        let r1 = sc.spawn(|| {
            store.ensure_prepared(
                &needs(&[K::X]),
                &no_estimate,
                &numbered(&n),
                &Control::unreserved(),
            )
        });
        held_rx.recv().unwrap();
        assert!(
            store
                .registry()
                .get(&K::X)
                .is_none_or(|e| e.state == State::Absent),
            "precondition: request 1 has not claimed X"
        );
        let s2 = store
            .ensure_prepared(
                &needs(&[K::X]),
                &no_estimate,
                &numbered(&n),
                &Control::unreserved(),
            )
            .unwrap();
        assert_eq!(
            store.registry().get(&K::X).map(|e| &e.state),
            Some(&State::Published),
            "precondition: X is published before request 1 goes on"
        );
        drop(open);
        (r1.join().unwrap().unwrap(), s2)
    });
    assert_eq!(store.stats().fills, 1, "X filled once");
    assert_eq!(store.stats().joins, 0, "nothing joined: X was published");
    assert!(
        Arc::ptr_eq(s1.get(&K::X).unwrap(), s2.get(&K::X).unwrap()),
        "request 1 reads request 2's X"
    );
}

/// A failed reservation with a waiter: request 1 claims `A` and its
/// reservation fails (after request 2 joined); request 1 answers `budget`,
/// its claim is released, and request 2 is woken, claims `A` itself and
/// fills it.
///
/// DISCRIMINATION (2026-11-02): not releasing the claims on the budget path
/// (`claims.keys.clear()` before the `return Err(Budget)`) leaves `A`
/// `Filling` by a gone worker: request 2 is never woken and its watchdog
/// ends it with `deadline` ("request 2 was woken and filled A": Deadline).
#[test]
fn a_failed_reservation_wakes_its_waiter_which_fills_the_key() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let first = AtomicUsize::new(0);
    let reserve = |_: &'static str, _: usize| -> Result<Guard<'_>, String> {
        if first.fetch_add(1, Ordering::SeqCst) == 0 {
            // Fail only once request 2 is waiting on this claim.
            while store.stats().joins == 0 {
                std::thread::yield_now();
            }
            return Err("no room".to_string());
        }
        Ok(Box::new(()))
    };
    let ctl = Control {
        stop: &|| None,
        reserve: &reserve,
    };
    // Request 2's watchdog: a waiter never woken gives up with `deadline`.
    let start = std::time::Instant::now();
    let watchdog = || (start.elapsed() > Duration::from_secs(10)).then_some(Stop::Deadline);
    let waiter = Control {
        stop: &watchdog,
        reserve: &reserve,
    };
    std::thread::scope(|sc| {
        let r1 =
            sc.spawn(|| store.ensure_prepared(&needs(&[K::A]), &no_estimate, &numbered(&n), &ctl));
        while store
            .registry()
            .get(&K::A)
            .is_none_or(|e| e.state == State::Absent)
        {
            std::thread::yield_now();
        }
        let r2 = sc
            .spawn(|| store.ensure_prepared(&needs(&[K::A]), &no_estimate, &numbered(&n), &waiter));
        assert_eq!(
            r1.join().unwrap().err(),
            Some(PrepareError::Budget("no room".into()))
        );
        let s2 = r2
            .join()
            .unwrap()
            .expect("request 2 was woken and filled A");
        assert_eq!(s2.get(&K::A).unwrap().made, 1);
    });
}

/// The last waiter cancelled while the fill publishes: the waiter answers
/// `cancelled` at once; the fill goes on and publishes for later requests.
#[test]
fn a_cancelled_waiter_leaves_and_the_fill_still_publishes() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let gate = Gate::default();
    let fill = |cx: FillCx<'_, K, V>| {
        let _ = started_tx.send(());
        gate.wait();
        numbered(&n)(cx)
    };
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let stop = || cancelled.load(Ordering::SeqCst).then_some(Stop::Cancelled);
    let waiter = Control {
        stop: &stop,
        reserve: &unreserved_guard,
    };
    std::thread::scope(|sc| {
        let open = gate.opener();
        let r1 = sc.spawn(|| {
            store.ensure_prepared(&needs(&[K::A]), &no_estimate, &fill, &Control::unreserved())
        });
        started_rx.recv().unwrap();
        let r2 = sc.spawn(|| store.ensure_prepared(&needs(&[K::A]), &no_estimate, &fill, &waiter));
        while store.stats().joins == 0 {
            std::thread::yield_now();
        }
        cancelled.store(true, Ordering::SeqCst);
        assert_eq!(r2.join().unwrap().err(), Some(PrepareError::Cancelled));
        drop(open);
        r1.join().unwrap().unwrap();
    });
    assert_eq!(store.load().get(&K::A).unwrap().made, 1);
    assert_eq!(store.stats().fills, 1);
}

/// A fill that fails (or panics) marks its key `Failed` and fails its
/// request `engineError`; nothing partial is published; the next request
/// claims the key again.
#[test]
fn a_failed_fill_publishes_nothing_and_is_claimed_again() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let fail_once = AtomicUsize::new(0);
    let fill = |cx: FillCx<'_, K, V>| {
        if *cx.key == K::B && fail_once.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("the B fill blew up");
        }
        numbered(&n)(cx)
    };
    let err = store
        .ensure_prepared(
            &needs(&[K::A, K::B]),
            &no_estimate,
            &fill,
            &Control::unreserved(),
        )
        .err()
        .unwrap();
    assert!(
        matches!(&err, PrepareError::Engine(m) if m.contains("the B fill blew up")),
        "{err:?}"
    );
    assert!(store.load().get(&K::B).is_none(), "no partial B");
    let s = store
        .ensure_prepared(
            &needs(&[K::A, K::B]),
            &no_estimate,
            &fill,
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(s.get(&K::B).unwrap().read_a, Some(1), "A kept, B refilled");
}

/// A key evicted during every fill churns: after `MAX_DISCARDS` discards the
/// request fails "artifact churn" (bounded termination) and the key is
/// left claimable.
#[test]
fn a_key_evicted_during_every_fill_churns_and_terminates() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let fill = |cx: FillCx<'_, K, V>| {
        store.tombstone(&[K::A]);
        numbered(&n)(cx)
    };
    let err = store
        .ensure_prepared(&needs(&[K::A]), &no_estimate, &fill, &Control::unreserved())
        .err()
        .unwrap();
    assert!(
        matches!(&err, PrepareError::Engine(m) if m.contains("churn")),
        "{err:?}"
    );
    assert_eq!(store.stats().discards, MAX_DISCARDS as u64);
    assert_eq!(store.registry()[&K::A].state, State::Absent);
}

/// An eviction skips keys pinned for assembly by a request in flight.
#[test]
fn eviction_skips_keys_pinned_for_assembly() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    store
        .ensure_prepared(
            &needs(&[K::A]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    let evicted = AtomicUsize::new(usize::MAX);
    let fill = |cx: FillCx<'_, K, V>| {
        // While B fills, A is pinned by this request.
        if *cx.key == K::B {
            evicted.store(store.evict(&[K::A]), Ordering::SeqCst);
        }
        numbered(&n)(cx)
    };
    let s = store
        .ensure_prepared(
            &needs(&[K::A, K::B]),
            &no_estimate,
            &fill,
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(evicted.load(Ordering::SeqCst), 0, "A was pinned");
    assert!(s.get(&K::A).is_some());
}

/// An eviction that evicts nothing (every key pinned) publishes nothing: a
/// request reading the current snapshot stays its only reader, so a later
/// reclaim can still free an unpinned value. Found by the tree store (stage
/// 5b): a reclaim tried the request's own pinned tree first, which used to
/// republish an equal snapshot, so every value had two owners and the next
/// kind's unpinned run could not be evicted.
///
/// DISCRIMINATION (2026-11-03): removing `evict`'s early return for an empty
/// eviction fails "the ready snapshot is still the current one" (and the
/// budget test `a_fill_that_does_not_fit_evicts_in_order...`, whose reclaim
/// then evicts nothing); reverted, green.
#[test]
fn an_empty_eviction_publishes_nothing() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    let ready = store
        .ensure_prepared(
            &needs(&[K::A, K::X]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    let held = store.hold(&[K::A]);
    assert_eq!(store.evict_unheld(&[K::A]), 0, "A is pinned");
    assert!(
        Arc::ptr_eq(&ready, &store.load()),
        "the ready snapshot is still the current one"
    );
    // The request still reads `ready`, which IS the current snapshot: X has
    // one owner and a reclaim frees it.
    assert_eq!(store.evict_unheld(&[K::X]), 1, "then the unpinned X goes");
    drop((ready, held));
}

/// A reclaim evicts only what no request holds: a value in a ready
/// snapshot a request still computes over stays (evicting it would free
/// nothing until the request ends); once released it goes. Found on CDO:
/// a request's own fold was evicted by its trees' reservation, which then
/// read the floor as the whole RSS and refused as impossible.
///
/// DISCRIMINATION (2026-11-02): `evict_unheld` filtering on `>= 1` instead
/// of `== 1` fails "an older snapshot holds X"; `hold` adding no pin fails
/// "a held value stays".
#[test]
fn a_reclaim_skips_values_a_ready_snapshot_holds() {
    let store: Store<K, V> = Store::new();
    let n = AtomicUsize::new(0);
    store
        .ensure_prepared(
            &needs(&[K::A]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    // A request computing over A holds it; an older snapshot keeps X's.
    let held = store.hold(&[K::A]);
    assert_eq!(store.evict_unheld(&[K::A]), 0, "a held value stays");
    assert!(store.load().get(&K::A).is_some());
    store
        .ensure_prepared(
            &needs(&[K::X]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    let older = store.load();
    store
        .ensure_prepared(
            &needs(&[K::Y]),
            &no_estimate,
            &numbered(&n),
            &Control::unreserved(),
        )
        .unwrap();
    assert_eq!(store.evict_unheld(&[K::X]), 0, "an older snapshot holds X");
    drop((held, older));
    assert_eq!(store.evict_unheld(&[K::A]), 1, "released, it goes");
    assert!(store.load().get(&K::A).is_none());
}
