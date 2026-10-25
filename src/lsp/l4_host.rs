//! #222 §4.2: the incremental L4 host's worker thread.
//!
//! `HostHandle::request` bumps `requested` under the one lock and sends a
//! wake. The worker drains queued wakes, reads `g = requested`, and refreshes
//! with `cancel = requested != g`. A complete refresh is always committed as
//! the baseline (OQ4), but published to `status` only if `g` is still the
//! newest request, checked under the same lock a bump takes (S3).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};

use log::debug;

use crate::engine::batch::job::ViewUniverse;
use crate::engine::incremental::{IncrementalHost, RefreshError, RefreshReport, Refresher};

/// The request counter and the published status, behind ONE lock (S3).
#[derive(Default)]
pub(crate) struct Shared {
    requested: u64,
    status: Option<HostStatus>,
}

/// The last published refresh and the generation it answered.
#[derive(Clone, Debug)]
pub struct HostStatus {
    pub generation: u64,
    pub report: RefreshReport,
}

/// The server's side of the worker. The worker ends when every clone drops.
#[derive(Clone)]
pub struct HostHandle {
    shared: Arc<Mutex<Shared>>,
    wake: mpsc::Sender<()>,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().expect("l4-host: lock poisoned")
}

impl HostHandle {
    /// Ask for a refresh of the current revision. Never waits on a refresh:
    /// the lock is held only for the bump.
    pub fn request(&self) {
        lock(&self.shared).requested += 1;
        // A send fails only once the worker is gone; there is nothing to wake.
        let _ = self.wake.send(());
    }

    pub fn status(&self) -> Option<HostStatus> {
        lock(&self.shared).status.clone()
    }
}

/// A handle, and the worker's ends of it.
fn channel() -> (HostHandle, Arc<Mutex<Shared>>, mpsc::Receiver<()>) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (wake, rx) = mpsc::channel();
    let handle = HostHandle {
        shared: Arc::clone(&shared),
        wake,
    };
    (handle, shared, rx)
}

/// The loop (§4.2), generic over its `Refresher` so tests can script it.
/// `after_drain` runs between the drain and the read of `g` (a no-op in
/// production; T4b's hook).
pub(crate) fn run_worker(
    shared: Arc<Mutex<Shared>>,
    wake: mpsc::Receiver<()>,
    mut host: impl Refresher,
    after_drain: impl Fn(),
) {
    // The generation of the last complete refresh. A wake whose request was
    // already read into it (sent after the drain) needs no second refresh:
    // every new request bumps `requested` past it (final r1 FX-12).
    let mut done = None;
    while wake.recv().is_ok() {
        // Drain BEFORE reading `g`: a request after the read keeps its wake.
        while wake.try_recv().is_ok() {}
        after_drain();
        let g = lock(&shared).requested;
        if done == Some(g) {
            continue;
        }
        match host.refresh(&|| lock(&shared).requested != g) {
            Ok(report) => {
                // A complete refresh is a settled revision: always the baseline (OQ4).
                host.commit();
                done = Some(g);
                let (kind, seeded, total) = (report.kind, report.seeded, report.total);
                let published = {
                    let mut s = lock(&shared);
                    let current = s.requested == g;
                    if current {
                        s.status = Some(HostStatus {
                            generation: g,
                            report,
                        });
                    }
                    current
                };
                // Logged after the lock is released: a blocked stderr must
                // never block `request()` (final r1 FX-1).
                if published {
                    debug!(
                        "l4-host: published generation {g}: {kind:?}, seeded {seeded} of {total}"
                    );
                }
            }
            Err(RefreshError::Build(e)) => debug!("l4-host: generation {g}: build error: {e}"),
            Err(RefreshError::Cancelled) => {}
        }
    }
}

/// Spawn the worker on `root` with a real `IncrementalHost`, and request the
/// first revision.
pub fn spawn_l4_host(root: PathBuf, universe: ViewUniverse) -> (HostHandle, JoinHandle<()>) {
    let (handle, shared, wake) = channel();
    let join = thread::Builder::new()
        .name("l4-host".into())
        .spawn(move || {
            guarded(
                || {
                    // Debug-only seam for the spawned-binary test of FX-13.
                    #[cfg(debug_assertions)]
                    if std::env::var_os("AL_SEM_L4_HOST_TEST_PANIC").is_some() {
                        panic!("AL_SEM_L4_HOST_TEST_PANIC");
                    }
                    run_worker(shared, wake, IncrementalHost::new(&root, universe), || {})
                },
                end_the_server,
            )
        })
        .expect("spawn the l4-host thread");
    handle.request();
    (handle, join)
}

/// Run `work`; a panic in it goes to `on_panic` with its message. In an
/// unwinding build (dev, test, `--release`) a panic would otherwise end only
/// the worker thread, silently (final r1 FX-13).
fn guarded(work: impl FnOnce(), on_panic: impl FnOnce(&str)) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("a non-string panic");
        on_panic(msg);
    }
}

/// A host crash ends the server, as the help text says, in every profile.
fn end_the_server(msg: &str) {
    eprintln!("l4-host: panicked: {msg}; ending the server");
    std::process::abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::incremental::{RefreshError, RefreshKind, RefreshReport, Refresher};
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(10);

    fn report(kind: RefreshKind) -> RefreshReport {
        RefreshReport {
            kind,
            resolved: Default::default(),
            seeded: 0,
            total: 0,
        }
    }

    /// Spin (no fixed sleep) until `status()` names `generation`, or time out.
    fn reaches(handle: &HostHandle, generation: u64) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if handle.status().is_some_and(|s| s.generation == generation) {
                return true;
            }
            std::thread::yield_now();
        }
        false
    }

    /// Run `run_worker` on its own thread with `host` and `after_drain`.
    fn start(
        parts: (Arc<Mutex<Shared>>, Receiver<()>),
        host: impl Refresher + Send + 'static,
        after_drain: impl Fn() + Send + 'static,
    ) -> JoinHandle<()> {
        let (shared, wake) = parts;
        thread::spawn(move || run_worker(shared, wake, host, after_drain))
    }

    /// Wraps a real host; after `host.refresh` returns (past its last cancel
    /// check) it reports the result and waits for a release.
    struct Paused {
        host: IncrementalHost,
        paused: Sender<Option<RefreshReport>>,
        release: Receiver<()>,
    }

    impl Refresher for Paused {
        fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError> {
            let r = self.host.refresh(cancel);
            self.paused.send(r.as_ref().ok().cloned()).unwrap();
            self.release.recv_timeout(TIMEOUT).expect("released");
            r
        }
        fn commit(&mut self) {
            self.host.commit();
        }
    }

    /// T1a's shape: `A→B→L`, `L` inserts `Cust`.
    fn t1a(dir: &std::path::Path) {
        let files = [
            (
                "app.json",
                r#"{"id":"22222222-0000-0000-0000-000000000224","name":"Host222","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{"from":50100,"to":50199}]}"#,
            ),
            (
                "src/Tables.al",
                "table 50100 Cust\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n",
            ),
            (
                "src/A.al",
                "codeunit 50100 A\n{\n    procedure A()\n    var\n        CB: Codeunit B;\n    begin\n        CB.B();\n    end;\n}\n",
            ),
            (
                "src/B.al",
                "codeunit 50101 B\n{\n    procedure B()\n    var\n        CL: Codeunit L;\n    begin\n        CL.L();\n    end;\n}\n",
            ),
            (
                "src/L.al",
                "codeunit 50102 L\n{\n    procedure L()\n    var\n        C: Record Cust;\n    begin\n        C.Insert();\n    end;\n}\n",
            ),
        ];
        for (rel, text) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    /// T4a (S3, S4, O20): refresh #1 completes, then request 2 arrives
    /// before `run_worker` sees the result. #1 is committed (so #2 is
    /// Incremental with nothing re-solved) but not published (`status()` is
    /// `None` during #2's pause). Breaks: (i) no publish guard → generation
    /// 1 is visible; (ii) commit only when current → #2 is Full(NoBaseline).
    #[test]
    fn t4a_stale_complete_unpublished_retained() {
        let tmp = tempfile::tempdir().unwrap();
        t1a(tmp.path());
        let (handle, shared, wake) = channel();
        let (paused_tx, paused) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let host = Paused {
            host: IncrementalHost::new(tmp.path(), ViewUniverse::CrossApp),
            paused: paused_tx,
            release: release_rx,
        };
        let join = start((shared, wake), host, || {});
        handle.request();
        let r1 = paused.recv_timeout(TIMEOUT).expect("#1 pauses");
        assert_eq!(
            r1.map(|r| r.kind),
            Some(RefreshKind::Full(
                crate::engine::incremental::FullReason::NoBaseline
            )),
            "#1 completes"
        );
        handle.request();
        release.send(()).unwrap();
        let r2 = paused.recv_timeout(TIMEOUT).expect("#2 pauses");
        assert!(
            handle.status().is_none(),
            "the stale #1 is not published: {:?}",
            handle.status()
        );
        let r2 = r2.expect("#2 completes");
        assert_eq!(r2.kind, RefreshKind::Incremental, "#1 was committed");
        assert!(r2.resolved.is_empty(), "nothing re-solved: {r2:?}");
        release.send(()).unwrap();
        assert!(reaches(&handle, 2), "status reaches generation 2");
        drop(handle);
        drop(release);
        join.join().unwrap();
    }

    /// A refresh that checks `cancel` once, then completes.
    struct CheckOnce;

    impl Refresher for CheckOnce {
        fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError> {
            if cancel() {
                return Err(RefreshError::Cancelled);
            }
            Ok(report(RefreshKind::Incremental))
        }
        fn commit(&mut self) {}
    }

    /// T4b: a request made between the drain and the read of `g` (the
    /// `after_drain` hook) is served. Break: read `g` before the drain → the
    /// drain eats the hook's wake, the refresh for the older `g` is
    /// cancelled, and the worker blocks; the wait times out.
    #[test]
    fn t4b_no_stranded_request() {
        let (handle, shared, wake) = channel();
        let hook = Mutex::new(Some(handle.clone()));
        let join = start((shared, wake), CheckOnce, move || {
            if let Some(h) = hook.lock().unwrap().take() {
                h.request();
            }
        });
        handle.request();
        assert!(reaches(&handle, 2), "status: {:?}", handle.status());
        drop(handle);
        join.join().unwrap();
    }

    /// [`CheckOnce`] that counts its refreshes.
    struct Counted(Arc<std::sync::atomic::AtomicUsize>);

    impl Refresher for Counted {
        fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            CheckOnce.refresh(cancel)
        }
        fn commit(&mut self) {}
    }

    /// Final r1 FX-12 (T4b's harness): the hook's request (made after the
    /// drain) is read into `g = 2` and served by ONE refresh; its wake,
    /// left in the channel, causes no second refresh of generation 2. A
    /// later request is still served. Break: drop the `done == Some(g)`
    /// skip → three refreshes.
    #[test]
    fn t4d_leftover_wake_refreshes_nothing() {
        let (handle, shared, wake) = channel();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook = Mutex::new(Some(handle.clone()));
        let (drained, drains) = mpsc::channel();
        let join = start((shared, wake), Counted(Arc::clone(&count)), move || {
            if let Some(h) = hook.lock().unwrap().take() {
                h.request();
            }
            let _ = drained.send(());
        });
        handle.request();
        assert!(reaches(&handle, 2), "status: {:?}", handle.status());
        // The leftover wake's drain: past it, the skip (or the refresh) is
        // done before the next `recv`.
        for _ in 0..2 {
            drains.recv_timeout(TIMEOUT).expect("a drain");
        }
        handle.request();
        assert!(reaches(&handle, 3), "status: {:?}", handle.status());
        drop(handle);
        join.join().unwrap();
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one refresh per generation"
        );
    }

    /// Holds refresh #1 until released; never calls `cancel`.
    struct Held {
        entered: Sender<()>,
        release: Receiver<()>,
    }

    impl Refresher for Held {
        fn refresh(&mut self, _: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError> {
            self.entered.send(()).unwrap();
            // A release or a closed channel lets the refresh end; a timeout
            // means the test never got to release it, which fails (FX-2): a
            // refresh that just ended would let a blocked `request()` return
            // inside its own wait.
            if let Err(mpsc::RecvTimeoutError::Timeout) = self.release.recv_timeout(TIMEOUT) {
                panic!("refresh #1 was never released");
            }
            Ok(report(RefreshKind::Incremental))
        }
        fn commit(&mut self) {}
    }

    /// T4c: with refresh #1 held, `request()` returns (asserted before the
    /// release). Break: the worker holds the lock for the whole refresh →
    /// `request()` blocks and the wait times out.
    #[test]
    fn t4c_request_never_waits() {
        let (handle, shared, wake) = channel();
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let held = Held {
            entered: entered_tx,
            release: release_rx,
        };
        let join = start((shared, wake), held, || {});
        handle.request();
        entered.recv_timeout(TIMEOUT).expect("#1 is running");
        let (done_tx, done) = mpsc::channel();
        let h = handle.clone();
        thread::spawn(move || {
            h.request();
            done_tx.send(()).unwrap();
        });
        assert!(
            done.recv_timeout(TIMEOUT).is_ok(),
            "request() returns while a refresh runs"
        );
        // Release #1; the closed channel lets #2 run straight through.
        release.send(()).unwrap();
        drop(release);
        assert!(reaches(&handle, 2), "status: {:?}", handle.status());
        drop(handle);
        join.join().unwrap();
    }

    /// Review Focus 2: dropping the last `HostHandle` ends the worker
    /// thread. Break: the spawned thread keeps a sender → it never ends.
    #[test]
    fn worker_exits_when_handles_drop() {
        let tmp = tempfile::tempdir().unwrap();
        t1a(tmp.path());
        let (handle, join) = spawn_l4_host(tmp.path().to_path_buf(), ViewUniverse::CrossApp);
        assert!(reaches(&handle, 1), "the first revision is published");
        let other = handle.clone();
        drop(handle);
        drop(other);
        let deadline = Instant::now() + TIMEOUT;
        while !join.is_finished() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(join.is_finished(), "the worker thread ends");
        join.join().unwrap();
    }
}
