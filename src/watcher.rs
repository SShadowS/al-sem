//! One LSP root's file system watch (#221).
//!
//! The watch covers every input the LSP build and the server read: the root
//! (recursive), the root's `app.json` and workspace config, and the ancestor
//! `.alpackages` folders the dependency loader reads
//! ([`crate::dependencies::package_cache_scope`]). Ancestors the walk visited
//! are watched non-recursively, so a cache folder appearing, vanishing or
//! being replaced is seen; each existing cache folder outside the root is
//! watched non-recursively too.
//!
//! [`map_event`] is the pure mapping of one raw event onto the updater's
//! [`ChangeEvent`]s; [`WatchState::handle_raw`] is the one per-event step
//! the production loop runs (re-sync the cache registrations, reload the
//! config, then send). The updater reconciles every path from disk, so no
//! rule here depends on rename pairing or on file/folder typing.
//!
//! Stated limits: an edit to the global `~/.al-sem/config.json` and a `.git`
//! created or removed at the root or an ancestor need a server restart. The
//! watch thread has no stop signal: it notices that the updater has gone
//! only when it next tries to send, so it lingers until the next event that
//! maps to one.

use log::{info, warn};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use crate::config::ConfigHandle;
use crate::dependencies::{CacheScope, Role, cache_scope_role, package_cache_scope};
use crate::lsp::updater::{ChangeEvent, rel_under_root};
use crate::protocol::normalize_path;

/// The two calls the watch state makes on a notify backend.
pub trait WatchBackend: Send {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()>;
    fn unwatch(&mut self, path: &Path) -> notify::Result<()>;
}

impl WatchBackend for RecommendedWatcher {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        Watcher::watch(self, path, mode)
    }
    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        Watcher::unwatch(self, path)
    }
}

/// Makes a root's backend, handing it the raw-event sender.
pub type BackendFactory = Arc<
    dyn Fn(Sender<notify::Result<Event>>) -> notify::Result<Box<dyn WatchBackend>> + Send + Sync,
>;

/// The production factory.
pub fn recommended_backend() -> BackendFactory {
    Arc::new(|tx| {
        Ok(Box::new(RecommendedWatcher::new(tx, Config::default())?) as Box<dyn WatchBackend>)
    })
}

/// What one root's watcher watches, computed once at start (the `.git`
/// boundary is frozen then; later only WHICH candidates exist is checked).
#[derive(Debug, Clone)]
pub struct WatchScope {
    pub root: PathBuf,
    pub cache: CacheScope,
}

impl WatchScope {
    pub fn compute(root: &Path) -> Self {
        WatchScope {
            root: root.to_path_buf(),
            cache: package_cache_scope(root),
        }
    }

    /// The candidate cache folders not strictly inside the root (the root's
    /// own `.alpackages` is covered by the recursive root watch).
    fn outside_candidates(&self) -> impl Iterator<Item = &PathBuf> {
        self.cache
            .candidates
            .iter()
            .filter(|c| !self.strictly_inside(c))
    }

    fn strictly_inside(&self, p: &Path) -> bool {
        rel_under_root(&self.root, p).is_some_and(|r| !r.as_os_str().is_empty())
    }

    /// The outside candidate `p` names (folder parts compared normalized).
    fn outside_candidate(&self, p: &Path) -> Option<PathBuf> {
        let p = normalize_path(p);
        self.outside_candidates()
            .find(|c| normalize_path(c) == p)
            .cloned()
    }
}

/// `Create`, `Remove` and renames say an entry appeared, vanished or moved;
/// every other kind only says something was touched (ON1).
fn is_structural(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

/// One raw event to the updater's events (Design "Event mapping", rules
/// 1-9, first match wins per path). Pure: an outside-root cache folder maps
/// to nothing here; [`WatchState::handle_raw`] re-syncs for it.
pub fn map_event(scope: &WatchScope, event: &Event) -> Vec<ChangeEvent> {
    use notify::event::ModifyKind;
    if event.need_rescan() {
        return vec![ChangeEvent::Overflow];
    }
    // The loader's own reads produce `Access` on cache paths (O8).
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    let structural = is_structural(&event.kind);
    let mut out = Vec::new();
    for p in &event.paths {
        let rel = rel_under_root(&scope.root, p);
        let rel_is = |name: &str| rel.as_deref() == Some(Path::new(name));
        if rel_is(".al-sem.json") || rel_is(".al-call-hierarchy.json") {
            out.push(ChangeEvent::ConfigChanged);
            continue;
        }
        match cache_scope_role(&scope.cache, p) {
            Some(Role::AppFile) => {
                out.push(ChangeEvent::DepsChanged);
                continue;
            }
            Some(Role::CacheFolder) => {
                if scope.strictly_inside(p) && structural {
                    out.push(ChangeEvent::DepsChanged);
                }
                continue;
            }
            None => {}
        }
        let Some(rel) = rel else { continue };
        if rel.as_os_str().is_empty() {
            if structural {
                out.push(ChangeEvent::Overflow);
            }
            continue;
        }
        if rel == Path::new("app.json") {
            out.push(ChangeEvent::DepsChanged);
            continue;
        }
        // Git rewrites `index.lock` on every `git status` (O3N1).
        if rel.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        let data_write = matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_))
        );
        if data_write && !crate::source_text::has_al_extension(p) {
            continue;
        }
        out.push(ChangeEvent::PathChanged(p.clone()));
    }
    out
}

/// One root's registrations and the per-event step.
pub struct WatchState {
    scope: WatchScope,
    registered: BTreeMap<PathBuf, RecursiveMode>,
    /// Per outside candidate: whether it existed as a folder at the last check.
    cache_seen: BTreeMap<PathBuf, bool>,
    /// Registrations that failed, retried by [`Self::retry_failed`].
    failed: BTreeMap<PathBuf, RecursiveMode>,
}

impl WatchState {
    pub fn new(scope: WatchScope) -> Self {
        WatchState {
            scope,
            registered: BTreeMap::new(),
            cache_seen: BTreeMap::new(),
            failed: BTreeMap::new(),
        }
    }

    /// Register the root and the walked dirs FIRST, then stat the candidates
    /// and register the existing ones: a cache folder created between the
    /// two steps is seen by its parent's watch (S7). The root is recursive,
    /// everything else not; a root that is itself a candidate stays
    /// recursive, because `resync` skips a registered path.
    pub fn start(&mut self, backend: &mut dyn WatchBackend) {
        let root = self.scope.root.clone();
        self.register(backend, &root, RecursiveMode::Recursive);
        for d in self.scope.cache.walked_dirs.clone() {
            self.register(backend, &d, RecursiveMode::NonRecursive);
        }
        self.resync(backend);
    }

    fn register(&mut self, backend: &mut dyn WatchBackend, p: &Path, mode: RecursiveMode) -> bool {
        match backend.watch(p, mode) {
            Ok(()) => {
                self.failed.remove(p);
                self.registered.insert(p.to_path_buf(), mode);
                true
            }
            Err(e) => {
                warn!("Failed to watch {} (retried every 5 s): {e}", p.display());
                self.failed.insert(p.to_path_buf(), mode);
                false
            }
        }
    }

    /// The one per-event step the production loop runs. Re-syncs the cache
    /// registrations BEFORE any `DepsChanged` is sent (S4), reloads the config
    /// before `ConfigChanged` is sent. Returns `false` once the updater's
    /// channel is closed.
    pub fn handle_raw(
        &mut self,
        backend: &mut dyn WatchBackend,
        raw: notify::Result<Event>,
        config: &ConfigHandle,
        tx: &Sender<ChangeEvent>,
    ) -> bool {
        let event = match raw {
            Ok(e) => e,
            Err(e) => {
                warn!("Watch error: {e:?}");
                return true;
            }
        };
        let mut out = Vec::new();
        let cache_folders: Vec<PathBuf> = event
            .paths
            .iter()
            .filter_map(|p| self.scope.outside_candidate(p))
            .collect();
        if !cache_folders.is_empty()
            && !event.need_rescan()
            && !matches!(event.kind, EventKind::Access(_))
        {
            if self.resync(backend) {
                out.push(ChangeEvent::DepsChanged);
            } else if is_structural(&event.kind) {
                // A replaced folder keeps its name: the existence diff
                // misses it, so re-watch it.
                for c in cache_folders {
                    if self.registered.remove(&c).is_some() {
                        let _ = backend.unwatch(&c);
                        self.register(backend, &c, RecursiveMode::NonRecursive);
                    }
                }
                out.push(ChangeEvent::DepsChanged);
            }
        }
        out.extend(map_event(&self.scope, &event));
        for ev in out {
            match ev {
                ChangeEvent::ConfigChanged => config.reload_from_disk(),
                ChangeEvent::DepsChanged => {
                    self.resync(backend);
                }
                _ => {}
            }
            if tx.send(ev).is_err() {
                return false;
            }
        }
        true
    }

    /// Re-stat every outside candidate: watch each that now exists, unwatch
    /// each that is gone (and stop retrying it). Returns whether any
    /// existence changed.
    pub fn resync(&mut self, backend: &mut dyn WatchBackend) -> bool {
        let candidates: Vec<PathBuf> = self.scope.outside_candidates().cloned().collect();
        let mut changed = false;
        for c in candidates {
            let exists = c.is_dir();
            changed |= self.cache_seen.insert(c.clone(), exists) != Some(exists);
            let registered = self.registered.contains_key(&c);
            if exists && !registered {
                self.register(backend, &c, RecursiveMode::NonRecursive);
            } else if !exists {
                self.failed.remove(&c);
                if registered {
                    let _ = backend.unwatch(&c);
                    self.registered.remove(&c);
                }
            }
        }
        changed
    }

    /// Retry the failed registrations (the loop's 5 s tick). After a success:
    /// resync, then `Overflow` (the root came back) or `DepsChanged`.
    /// Returns `false` once the updater's channel is closed.
    pub fn retry_failed(
        &mut self,
        backend: &mut dyn WatchBackend,
        tx: &Sender<ChangeEvent>,
    ) -> bool {
        let mut any = false;
        let mut root = false;
        for (p, mode) in std::mem::take(&mut self.failed) {
            if self.register(backend, &p, mode) {
                any = true;
                root |= p == self.scope.root;
            }
        }
        if !any {
            return true;
        }
        self.resync(backend);
        let ev = if root {
            ChangeEvent::Overflow
        } else {
            ChangeEvent::DepsChanged
        };
        tx.send(ev).is_ok()
    }
}

/// Start `root`'s watch: the backend is created and every registration made
/// on the CALLING thread, so all watches exist before the build reads (S7).
/// A thread then forwards raw events until the updater's channel closes.
pub fn start_watching(
    root: &Path,
    factory: &BackendFactory,
    config: Arc<ConfigHandle>,
    tx: Sender<ChangeEvent>,
) {
    let (raw_tx, raw_rx) = channel();
    let mut backend = match factory(raw_tx) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to create file watcher for {}: {e}", root.display());
            return;
        }
    };
    let mut state = WatchState::new(WatchScope::compute(root));
    state.start(backend.as_mut());
    info!("Watching for file changes in: {}", root.display());
    std::thread::spawn(move || {
        let mut last_retry = Instant::now();
        loop {
            let alive = match raw_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(raw) => state.handle_raw(backend.as_mut(), raw, &config, &tx),
                Err(RecvTimeoutError::Timeout) => true,
                Err(RecvTimeoutError::Disconnected) => false,
            };
            let alive = alive
                && (last_retry.elapsed() < Duration::from_secs(5) || {
                    last_retry = Instant::now();
                    state.retry_failed(backend.as_mut(), &tx)
                });
            if !alive {
                info!("Updater channel found closed on a send; stopping the file watcher thread");
                return;
            }
        }
    });
}

/// A recording [`WatchBackend`] for tests (`watcher.rs` and `server.rs`).
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::mpsc::Receiver;

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Rec {
        Watch(PathBuf, RecursiveMode),
        Unwatch(PathBuf),
        /// An event that reached the change channel.
        Sent(ChangeEvent),
    }

    /// Records every call. Before recording, it drains the change channel
    /// (`events`) into the same log, so the log shows the true order of
    /// watch calls and sent events.
    #[derive(Clone, Default)]
    pub(crate) struct FakeBackend {
        pub(crate) log: Arc<Mutex<Vec<Rec>>>,
        /// `watch` on these paths fails.
        pub(crate) refuse: Arc<Mutex<HashSet<PathBuf>>>,
        pub(crate) events: Option<Arc<Mutex<Receiver<ChangeEvent>>>>,
    }

    impl FakeBackend {
        pub(crate) fn drain(&self) {
            if let Some(rx) = &self.events {
                let rx = rx.lock().unwrap();
                let mut log = self.log.lock().unwrap();
                log.extend(rx.try_iter().map(Rec::Sent));
            }
        }

        /// Drain, then take and clear the log.
        pub(crate) fn take_log(&self) -> Vec<Rec> {
            self.drain();
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl WatchBackend for FakeBackend {
        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            self.drain();
            if self.refuse.lock().unwrap().contains(path) {
                return Err(notify::Error::generic("fake backend: watch refused"));
            }
            self.log
                .lock()
                .unwrap()
                .push(Rec::Watch(path.to_path_buf(), mode));
            Ok(())
        }

        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.drain();
            self.log
                .lock()
                .unwrap()
                .push(Rec::Unwatch(path.to_path_buf()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeBackend, Rec};
    use super::*;
    use crate::dependencies::{Role, cache_scope_role};
    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, Flag, ModifyKind, RemoveKind, RenameMode,
    };
    use std::fs;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn ev(kind: EventKind, paths: &[&Path]) -> Event {
        paths
            .iter()
            .fold(Event::new(kind), |e, p| e.add_path(p.to_path_buf()))
    }

    // ── T10: event mapping, one row per rule (hand-stated scope) ──────────

    fn t10_scope() -> WatchScope {
        let root = PathBuf::from("/p/ws");
        WatchScope {
            cache: CacheScope {
                candidates: vec![root.join(".alpackages"), PathBuf::from("/p/.alpackages")],
                walked_dirs: vec![PathBuf::from("/p")],
            },
            root,
        }
    }

    fn map(kind: EventKind, paths: &[&str]) -> Vec<ChangeEvent> {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        map_event(&t10_scope(), &ev(kind, &refs))
    }

    fn changed(p: &str) -> ChangeEvent {
        ChangeEvent::PathChanged(PathBuf::from(p))
    }

    #[test]
    fn t10_rescan_flag_is_overflow() {
        let e = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(map_event(&t10_scope(), &e), vec![ChangeEvent::Overflow]);
    }

    #[test]
    fn t10_access_on_a_cache_app_is_dropped() {
        let kind = EventKind::Access(AccessKind::Open(AccessMode::Any));
        assert_eq!(map(kind, &["/p/ws/.alpackages/x.app"]), vec![]);
    }

    #[test]
    fn t10_al_outside_the_root_is_dropped() {
        let kind = EventKind::Create(CreateKind::File);
        assert_eq!(map(kind, &["/elsewhere/x.al"]), vec![]);
    }

    #[test]
    fn t10_app_created_in_the_root_cache_is_deps_changed() {
        let kind = EventKind::Create(CreateKind::File);
        assert_eq!(
            map(kind, &["/p/ws/.alpackages/x.app"]),
            vec![ChangeEvent::DepsChanged]
        );
    }

    #[test]
    fn t10_rename_both_yields_one_path_changed_per_path() {
        let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
        assert_eq!(
            map(kind, &["/p/ws/a.al", "/p/ws/b.al"]),
            vec![changed("/p/ws/a.al"), changed("/p/ws/b.al")]
        );
    }

    #[test]
    fn t10_modify_any_on_a_folder_is_path_changed() {
        let kind = EventKind::Modify(ModifyKind::Any);
        assert_eq!(map(kind, &["/p/ws/src"]), vec![changed("/p/ws/src")]);
    }

    #[test]
    fn t10_event_kind_any_on_a_folder_is_path_changed() {
        assert_eq!(
            map(EventKind::Any, &["/p/ws/src"]),
            vec![changed("/p/ws/src")]
        );
    }

    #[test]
    fn t10_git_paths_are_dropped() {
        let touched = EventKind::Modify(ModifyKind::Any);
        assert_eq!(map(touched, &["/p/ws/.git"]), vec![]);
        let created = EventKind::Create(CreateKind::File);
        assert_eq!(map(created, &["/p/ws/.git/index.lock"]), vec![]);
    }

    #[test]
    fn t10_structural_event_on_the_root_cache_folder_is_deps_changed() {
        let kind = EventKind::Remove(RemoveKind::Folder);
        assert_eq!(
            map(kind, &["/p/ws/.alpackages"]),
            vec![ChangeEvent::DepsChanged]
        );
    }

    #[test]
    fn t10_touched_root_cache_folder_is_nothing() {
        let kind = EventKind::Modify(ModifyKind::Any);
        assert_eq!(map(kind, &["/p/ws/.alpackages"]), vec![]);
    }

    #[test]
    fn t10_data_write_on_a_non_al_file_is_dropped() {
        let kind = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        assert_eq!(map(kind, &["/p/ws/README.md"]), vec![]);
    }

    #[test]
    fn t10_root_entry_removed_is_overflow() {
        let kind = EventKind::Remove(RemoveKind::Folder);
        assert_eq!(map(kind, &["/p/ws"]), vec![ChangeEvent::Overflow]);
    }

    #[test]
    fn t10_root_entry_touched_is_nothing() {
        let kind = EventKind::Modify(ModifyKind::Any);
        assert_eq!(map(kind, &["/p/ws"]), vec![]);
    }

    #[test]
    fn t10_root_app_json_is_deps_changed() {
        let kind = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        assert_eq!(
            map(kind, &["/p/ws/app.json"]),
            vec![ChangeEvent::DepsChanged]
        );
    }

    #[test]
    fn t10_workspace_config_is_config_changed() {
        let kind = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        assert_eq!(
            map(kind, &["/p/ws/.al-sem.json"]),
            vec![ChangeEvent::ConfigChanged]
        );
        assert_eq!(
            map(kind, &["/p/ws/.al-call-hierarchy.json"]),
            vec![ChangeEvent::ConfigChanged]
        );
    }

    #[test]
    fn t10_app_under_a_nested_alpackages_is_path_changed() {
        let kind = EventKind::Create(CreateKind::File);
        assert_eq!(
            map(kind, &["/p/ws/sub/.alpackages/z.app"]),
            vec![changed("/p/ws/sub/.alpackages/z.app")]
        );
    }

    #[test]
    fn t10_upper_case_app_extension_is_not_deps_changed() {
        let kind = EventKind::Create(CreateKind::File);
        let got = map(kind, &["/p/ws/.alpackages/X.APP"]);
        assert!(
            !got.contains(&ChangeEvent::DepsChanged),
            "T10: the loader reads only `*.app` (case-sensitive); got {got:?}"
        );
    }

    /// Windows lowercases the root (`protocol::normalize_path`); a raw event
    /// keeps the disk's case. Runs only on Windows.
    #[cfg(windows)]
    #[test]
    fn t10_windows_root_in_another_case_is_still_under_the_root() {
        let root = PathBuf::from(r"c:\users\me\ws");
        let scope = WatchScope {
            cache: CacheScope {
                candidates: vec![root.join(".alpackages")],
                walked_dirs: vec![],
            },
            root,
        };
        let raw = PathBuf::from(r"C:\Users\Me\ws\Src\A.al");
        let e = ev(EventKind::Create(CreateKind::File), &[&raw]);
        assert_eq!(
            map_event(&scope, &e),
            vec![ChangeEvent::PathChanged(raw.clone())]
        );
    }

    // ── T11: watch wiring through `WatchState::handle_raw` (fake backend) ──

    /// `<tmp>/parent/ws`: `parent` holds `.git`, so the walk stops there and
    /// `parent/.alpackages` is the one ancestor candidate.
    struct Rig {
        _tmp: tempfile::TempDir,
        parent: PathBuf,
        ws: PathBuf,
        fake: FakeBackend,
        tx: Sender<ChangeEvent>,
        config: ConfigHandle,
    }

    impl Rig {
        fn cache(&self) -> PathBuf {
            self.parent.join(".alpackages")
        }

        fn state(&self) -> WatchState {
            WatchState::new(WatchScope::compute(&self.ws))
        }

        fn raw(&mut self, st: &mut WatchState, kind: EventKind, path: &Path) {
            st.handle_raw(
                &mut self.fake,
                Ok(ev(kind, &[path])),
                &self.config,
                &self.tx,
            );
        }
    }

    fn rig(with_parent_cache: bool) -> Rig {
        let tmp = tempdir().unwrap();
        let parent = tmp.path().join("parent");
        let ws = parent.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir(parent.join(".git")).unwrap();
        if with_parent_cache {
            fs::create_dir(parent.join(".alpackages")).unwrap();
        }
        assert!(!ws.join(".git").exists(), "precondition: no .git in ws");
        let (tx, rx) = channel();
        let fake = FakeBackend {
            events: Some(Arc::new(Mutex::new(rx))),
            ..FakeBackend::default()
        };
        let config = ConfigHandle::new(ws.clone(), false);
        Rig {
            _tmp: tmp,
            parent,
            ws,
            fake,
            tx,
            config,
        }
    }

    fn pos(log: &[Rec], rec: &Rec) -> usize {
        log.iter()
            .position(|r| r == rec)
            .unwrap_or_else(|| panic!("{rec:?} missing from {log:?}"))
    }

    #[test]
    fn t11a_start_registers_root_and_parents_before_the_candidates() {
        let mut r = rig(true);
        let mut st = r.state();
        st.start(&mut r.fake);
        let log = r.fake.take_log();
        let root = pos(&log, &Rec::Watch(r.ws.clone(), RecursiveMode::Recursive));
        let parent = pos(
            &log,
            &Rec::Watch(r.parent.clone(), RecursiveMode::NonRecursive),
        );
        let cand = pos(&log, &Rec::Watch(r.cache(), RecursiveMode::NonRecursive));
        assert!(
            cand > root && cand > parent,
            "T11(a): candidates come last; {log:?}"
        );
    }

    #[test]
    fn t11b_new_cache_folder_is_watched_before_deps_changed_is_sent() {
        let mut r = rig(false);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        fs::create_dir(r.cache()).unwrap();
        let cache = r.cache();
        r.raw(&mut st, EventKind::Create(CreateKind::Folder), &cache);
        let log = r.fake.take_log();
        let watch = pos(&log, &Rec::Watch(cache, RecursiveMode::NonRecursive));
        let sent = pos(&log, &Rec::Sent(ChangeEvent::DepsChanged));
        assert!(watch < sent, "T11(b): resync before send; {log:?}");
    }

    #[test]
    fn t11c_touched_unchanged_cache_folder_sends_nothing() {
        let mut r = rig(true);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        let cache = r.cache();
        r.raw(&mut st, EventKind::Modify(ModifyKind::Any), &cache);
        assert_eq!(r.fake.take_log(), vec![], "T11(c)");
    }

    #[test]
    fn t11d_replaced_cache_folder_is_rewatched_then_deps_changed() {
        let mut r = rig(true);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        let cache = r.cache();
        fs::remove_dir(&cache).unwrap();
        fs::create_dir(&cache).unwrap();
        r.raw(&mut st, EventKind::Create(CreateKind::Folder), &cache);
        assert_eq!(
            r.fake.take_log(),
            vec![
                Rec::Unwatch(cache.clone()),
                Rec::Watch(cache, RecursiveMode::NonRecursive),
                Rec::Sent(ChangeEvent::DepsChanged),
            ],
            "T11(d)"
        );
    }

    #[test]
    fn t11e_retry_after_a_failed_parent_watch_resyncs_before_sending() {
        let mut r = rig(false);
        r.fake.refuse.lock().unwrap().insert(r.parent.clone());
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        fs::create_dir(r.cache()).unwrap();
        r.fake.refuse.lock().unwrap().clear();
        st.retry_failed(&mut r.fake, &r.tx);
        let log = r.fake.take_log();
        let parent = pos(
            &log,
            &Rec::Watch(r.parent.clone(), RecursiveMode::NonRecursive),
        );
        let cand = pos(&log, &Rec::Watch(r.cache(), RecursiveMode::NonRecursive));
        let sent = pos(&log, &Rec::Sent(ChangeEvent::DepsChanged));
        assert!(
            parent < sent && cand < sent,
            "T11(e): resync before send; {log:?}"
        );

        let app = r.cache().join("y.app");
        fs::write(&app, b"").unwrap();
        r.raw(&mut st, EventKind::Create(CreateKind::File), &app);
        assert!(
            r.fake
                .take_log()
                .contains(&Rec::Sent(ChangeEvent::DepsChanged)),
            "T11(e): the retried cache folder is live"
        );
    }

    #[test]
    fn t11f_raw_app_create_in_ancestor_cache_is_deps_changed() {
        let mut r = rig(true);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        let app = r.cache().join("x.app");
        fs::write(&app, b"").unwrap();
        r.raw(&mut st, EventKind::Create(CreateKind::File), &app);
        assert!(
            r.fake
                .take_log()
                .contains(&Rec::Sent(ChangeEvent::DepsChanged)),
            "T11(f)"
        );
    }

    /// The root is itself a candidate folder (an app inside a folder named
    /// `.alpackages`): planned twice, so `Recursive` must win. Scope
    /// hand-stated.
    #[test]
    fn t11g_root_that_is_a_candidate_is_registered_once_recursive() {
        let tmp = tempdir().unwrap();
        let parent = tmp.path().join("parent");
        let root = parent.join(".alpackages");
        fs::create_dir_all(&root).unwrap();
        let scope = WatchScope {
            cache: CacheScope {
                candidates: vec![root.join(".alpackages"), root.clone()],
                walked_dirs: vec![parent],
            },
            root: root.clone(),
        };
        let mut fake = FakeBackend::default();
        WatchState::new(scope).start(&mut fake);
        let root_watches: Vec<Rec> = fake
            .take_log()
            .into_iter()
            .filter(|rec| matches!(rec, Rec::Watch(p, _) if *p == root))
            .collect();
        assert_eq!(
            root_watches,
            vec![Rec::Watch(root, RecursiveMode::Recursive)],
            "T11(g)"
        );
    }

    #[test]
    fn t11h_cache_scope_role_follows_the_loader() {
        let r = rig(true);
        let scope = package_cache_scope(&r.ws);
        let c = r.cache();
        assert_eq!(
            cache_scope_role(&scope, &c.join("x.app")),
            Some(Role::AppFile)
        );
        assert_eq!(cache_scope_role(&scope, &c), Some(Role::CacheFolder));
        assert_eq!(cache_scope_role(&scope, &c.join("x.al")), None);
        assert_eq!(cache_scope_role(&scope, &c.join("n").join("y.app")), None);
        let nested = r.ws.join("sub").join(".alpackages").join("z.app");
        assert_eq!(cache_scope_role(&scope, &nested), None);
    }

    #[test]
    fn t11i_root_cache_folder_removed_is_deps_changed_without_rewatch() {
        let mut r = rig(false);
        let own = r.ws.join(".alpackages");
        fs::create_dir(&own).unwrap();
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        fs::remove_dir(&own).unwrap();
        r.raw(&mut st, EventKind::Remove(RemoveKind::Folder), &own);
        assert_eq!(
            r.fake.take_log(),
            vec![Rec::Sent(ChangeEvent::DepsChanged)],
            "T11(i): the root watch owns that folder"
        );
    }

    /// F2: a cache folder whose registration failed and which is then
    /// deleted is no longer retried.
    #[test]
    fn t11k_deleted_cache_folder_stops_being_retried() {
        let mut r = rig(true);
        r.fake.refuse.lock().unwrap().insert(r.cache());
        let mut st = r.state();
        st.start(&mut r.fake);
        assert!(st.failed.contains_key(&r.cache()), "precondition: failed");
        r.fake.take_log();
        fs::remove_dir(r.cache()).unwrap();
        st.resync(&mut r.fake);
        r.fake.refuse.lock().unwrap().clear();
        st.retry_failed(&mut r.fake, &r.tx);
        assert_eq!(r.fake.take_log(), vec![], "T11(k)");
    }

    /// Review focus: VS Code runs `git status` on every save, which rewrites
    /// `.git/index.lock`: no rebuild, no reconcile, no watch call.
    #[test]
    fn t11j_git_lock_churn_sends_nothing() {
        let mut r = rig(false);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        let lock = r.ws.join(".git").join("index.lock");
        r.raw(&mut st, EventKind::Create(CreateKind::File), &lock);
        assert_eq!(r.fake.take_log(), vec![], "T11(j)");
    }

    #[test]
    fn test_watcher_detects_al_file() {
        // #221: on the fake backend. The old version passed when no event
        // arrived at all (`if let Some(..)`).
        let mut r = rig(false);
        let mut st = r.state();
        st.start(&mut r.fake);
        r.fake.take_log();
        let al_file = r.ws.join("Test.al");
        fs::write(&al_file, "codeunit 50000 Test {}").unwrap();
        r.raw(&mut st, EventKind::Create(CreateKind::File), &al_file);
        assert_eq!(
            r.fake.take_log(),
            vec![Rec::Sent(ChangeEvent::PathChanged(al_file))]
        );
    }
}
