//! The resident session (context-server design §4.3 "freshness → pin", §5,
//! §6): one workspace in one universe, its published revision (or the
//! failure of its last build), and the build that replaces it.
//!
//! **Pin** ([`ResidentSession::pin`]): per request, by its freshness mode:
//! - `fresh` (default): observe the manifest (stat-cached against the
//!   published one, 1-2 ms on CDO); equal → the published revision; else a
//!   refresh, and the answer comes from the new revision.
//! - `verify`: `fresh`, with every input hashed.
//! - `latest-built`: the published revision now, `stale` with the changed
//!   paths when the manifest differs, and a background refresh.
//!
//! **Build** (one at a time, under the builder lock): `M_before` (the
//! observation), the workspace read (each input read once into owned bytes),
//! `M_consumed` from that read, the target (cross-app through the stage-4
//! `WorkspaceSession`, which reuses what its rules allow; single-app cold),
//! `M_after`; the three must be equal over every entry, else the build is
//! discarded and retried, at most [`BUILD_ATTEMPTS`] times, then
//! `inputsUnstable`. A semantic build failure is published as the session's
//! state (`revision: null`): every request answers it through its job's own
//! `view_failed`, as the standalone CLI does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::engine::batch::job::{ViewFailure, ViewKey, ViewNeeds};
use crate::engine::gate::run::{AnalysisTarget, TargetFailure, build_view_model};
use crate::engine::resident::budget::{Budget, BudgetStatus, Case, Refusal, Reservation};
use crate::engine::resident::manifest::{Hashing, Manifest, check_consistent};
use crate::engine::resident::revision::{GraphRevision, Revision};
use crate::engine::resident::store::Stop;
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::session::WorkspaceSession;
use crate::program::session::classify::ChangeClass;
use crate::wire::RevisionId;

/// How many times a build whose inputs moved is retried (§5.2).
pub const BUILD_ATTEMPTS: usize = 3;

/// What a revision's read holds: everything any served command reads (the
/// snapshot's input rows for digest, prove and fingerprint included).
pub const REVISION_READ: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

/// The model a revision builds first: what most served commands' views need
/// (explain's rules and card text on top of the model). It does NOT cover
/// every demand: `prove` with a filter also reads the guard arms
/// (`ViewNeeds::PROVE_SLICE`). Such a job is refused by the revision with
/// its needs (`PrepareError::Needs`) and the engine host pins again with
/// [`ResidentSession::pin_needs`], which widens the session's needs for
/// every later build (engine limits B1). Guard arms stay out of this
/// constant so a session that never sees a filter does not pay for them.
pub const REVISION_NEEDS: ViewNeeds = ViewNeeds::EXPLAIN;

/// A request's freshness mode (§5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Freshness {
    #[default]
    #[serde(rename = "fresh")]
    Fresh,
    #[serde(rename = "latest-built")]
    LatestBuilt,
    #[serde(rename = "verify")]
    Verify,
}

/// A published state's input descriptor (agent-tasks design §1.2 M0-6): its
/// generation (`None` for a failed build), the manifest stamp, the universe
/// and the manifest it was built from, each file entry with the stat its
/// hash was taken at. A front end observes the disk against it
/// ([`Manifest::from_descriptor_entries`], `Hashing::Cached`) without asking
/// the child.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDescriptor {
    pub generation: Option<u64>,
    pub inputs_stamp: String,
    pub workspace: PathBuf,
    pub universe: String,
    pub manifest: Vec<crate::engine::resident::manifest::ManifestEntry>,
}

/// A build that did not produce a model: its read and its failure.
pub struct FailedBuild {
    pub inputs: Arc<WorkspaceInputs>,
    pub failure: ViewFailure,
}

/// What the session publishes.
#[derive(Clone)]
pub enum Built {
    Revision(Arc<Revision>),
    /// A graph-only session's revision (M0-2).
    Graph(Arc<GraphRevision>),
    Failed(Arc<FailedBuild>),
}

impl Built {
    /// The revision's identity, or `None` for a failed build.
    #[must_use]
    pub fn revision_id(&self) -> Option<RevisionId> {
        match self {
            Built::Revision(r) => Some(r.id.clone()),
            Built::Graph(g) => Some(g.id.clone()),
            Built::Failed(_) => None,
        }
    }

    /// The read the published state was built from.
    #[must_use]
    pub fn inputs(&self) -> &Arc<WorkspaceInputs> {
        match self {
            Built::Revision(r) => &r.inputs,
            Built::Graph(g) => &g.inputs,
            Built::Failed(f) => &f.inputs,
        }
    }
}

/// The published state and the manifest it was built from (`M_after`, with
/// its stats, the next check's cache).
struct Published {
    built: Built,
    manifest: Manifest,
    /// The assembly needs its model's rows were built with (M0-4).
    needs: crate::program::model::workspace::AssemblyNeeds,
}

/// One request's pin: the state it is answered from, held for the whole
/// request (T-pin).
pub struct Pinned {
    pub built: Built,
    /// `latest-built` only: the manifest differs from the published one.
    pub stale: bool,
    /// The inputs that differ (`latest-built` when stale).
    pub changed_paths: Vec<String>,
    /// How long the request waited for a build.
    pub waited_for_build_ms: u64,
    /// The rebuild this request caused for an artifact the published
    /// revision lacked (`"sited"`, [`ResidentSession::pin_sited`]).
    pub rebuilt_for: Option<&'static str>,
}

/// Why no state was pinned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinError {
    /// The request stopped before a build began.
    Stopped(Stop),
    /// Every attempt's three observations disagreed.
    Unstable(String),
    /// The build's reservation was refused (§4.5): nothing was read.
    Budget(Refusal),
}

/// What the session did so far (`session/status`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// Builds published (revisions and failed builds).
    pub builds: u64,
    /// Builds discarded by the three-way check.
    pub discarded_builds: u64,
    /// Freshness checks that found the published state current.
    pub fresh_hits: u64,
    /// The last build: its change class (`P`, `S`, `B` or `cold`), wall ms.
    pub last_build_class: Option<String>,
    pub last_build_ms: Option<u64>,
    /// When the last published build began, in milliseconds since the Unix
    /// epoch (`alsem_status`'s freshness block, MCP output plan 3c).
    pub last_build_started_unix_ms: Option<u64>,
    /// The last freshness check's wall time, in microseconds.
    pub last_check_us: Option<u64>,
    /// Revisions published with a sited graph, and the last one's build
    /// time and resident bytes (agent-tasks design §1.2 M0-1).
    pub sited_builds: u64,
    pub last_sited_ms: Option<u64>,
    pub last_sited_bytes: Option<u64>,
    /// Rebuilds a request whose model needs exceeded the published rows'
    /// caused ([`ResidentSession::pin_needs`], M0-4).
    pub needs_rebuilds: u64,
}

/// A point in a build where a test hook runs (T-acquire-*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPoint {
    /// `M_before` observed; the read is next.
    BeforeRead,
    /// The read acquired every input; the target build is next.
    AfterRead,
    /// The target is built; `M_after` is next.
    AfterBuild,
    /// [`ResidentSession::pin_sited`] is about to build the sited graph for
    /// the published revision, from a program over that revision's read (no
    /// workspace read, no new revision); the attempt number is 0.
    BeforeSited,
}

/// A test's hook: called at each [`BuildPoint`] with the attempt number.
pub type BuildHook = Box<dyn Fn(BuildPoint, usize) + Send + Sync>;

/// The session's builder: the stage-4 incremental model and its counters.
struct Builder {
    workspace: WorkspaceSession,
}

/// One workspace's resident session (see the module doc).
pub struct ResidentSession {
    workspace: PathBuf,
    single_app: bool,
    instance_id: String,
    builder: Mutex<Builder>,
    current: Mutex<Option<Arc<Published>>>,
    /// Paths a change notification named since the last build.
    dirty: Mutex<BTreeSet<PathBuf>>,
    generation: AtomicU64,
    stats: Mutex<SessionStats>,
    hook: Option<BuildHook>,
    /// The child's memory budget (stage 3); `None` admits everything.
    budget: Option<Arc<Budget>>,
    /// `--low-mem` ([`Self::with_low_mem`]).
    low_mem: bool,
    /// Whether each build makes the sited graph ([`Self::with_sited`],
    /// [`Self::builds_sited`]).
    sited: std::sync::atomic::AtomicBool,
    /// A graph-only session ([`Self::graph_only`], M0-2).
    graph_only: bool,
    /// The inputs every read and observation takes besides the workspace's
    /// own: a test universe's ([`Self::with_injected`], agent-tasks §4.3).
    injected: crate::program::inputs::Injected,
    /// The descriptor's universe name, when not the base one (a test
    /// universe's `tests:<key>`).
    universe_label: Option<String>,
    /// The model each build makes ([`REVISION_NEEDS`] unless
    /// [`Self::with_needs`] or [`Self::pin_needs`] set it, M0-4).
    needs: Mutex<ViewNeeds>,
}

impl ResidentSession {
    /// A session for `workspace` in its universe; nothing is built until the
    /// first pin.
    #[must_use]
    pub fn new(workspace: PathBuf, single_app: bool, instance_id: String) -> ResidentSession {
        ResidentSession {
            workspace,
            single_app,
            instance_id,
            builder: Mutex::new(Builder {
                workspace: WorkspaceSession::new(),
            }),
            current: Mutex::new(None),
            dirty: Mutex::new(BTreeSet::new()),
            generation: AtomicU64::new(0),
            stats: Mutex::new(SessionStats::default()),
            hook: None,
            budget: None,
            low_mem: false,
            sited: std::sync::atomic::AtomicBool::new(false),
            graph_only: false,
            injected: crate::program::inputs::Injected::default(),
            universe_label: None,
            needs: Mutex::new(REVISION_NEEDS),
        }
    }

    /// The session building its models with `needs` (a smaller model than
    /// [`REVISION_NEEDS`]: a request that needs more pins with
    /// [`Self::pin_needs`]).
    #[must_use]
    pub fn with_needs(self, needs: ViewNeeds) -> ResidentSession {
        *lock(&self.needs) = needs;
        self
    }

    /// The state a request whose model needs `needs` is answered from (M0-4):
    /// [`Self::pin`]; then, when the published model's rows were built with
    /// fewer needs, the session's needs are widened to cover them and the
    /// model is rebuilt once (`Pinned::rebuilt_for: "needs"`, class `needs`;
    /// the row cache misses every file whose recorded needs differ). A
    /// rebuild the budget refuses is `PinError::Budget`, with the reason.
    /// The engine host calls it when a revision refused a job for its needs
    /// (`PrepareError::Needs`, engine limits B1).
    pub fn pin_needs(
        self: &Arc<Self>,
        mode: Freshness,
        needs: crate::program::model::workspace::AssemblyNeeds,
        stop: &dyn Fn() -> Option<Stop>,
    ) -> Result<Pinned, PinError> {
        let pinned = self.pin(mode, stop)?;
        let covered = |s: &Self| {
            lock(&s.current)
                .as_ref()
                .is_some_and(|p| p.needs.covers(needs))
        };
        if self.graph_only || !matches!(pinned.built, Built::Revision(_)) || covered(self) {
            return Ok(pinned);
        }
        if let Some(s) = stop() {
            return Err(PinError::Stopped(s));
        }
        let start = Instant::now();
        let mut b = lock(&self.builder);
        {
            let mut n = lock(&self.needs);
            n.rules |= needs.rules;
            n.card_text |= needs.card_text;
            n.site_guards |= needs.site_guards;
        }
        if covered(self) {
            return Ok(Pinned {
                built: self.published().expect("published"),
                waited_for_build_ms: start.elapsed().as_millis() as u64,
                ..pinned
            });
        }
        let built = self.build_locked(&mut b, None, 1, &|| stop().is_some())?;
        {
            let mut s = lock(&self.stats);
            s.needs_rebuilds += 1;
            s.last_build_class = Some("needs".into());
        }
        Ok(Pinned {
            built,
            stale: false,
            changed_paths: Vec::new(),
            waited_for_build_ms: pinned.waited_for_build_ms + start.elapsed().as_millis() as u64,
            rebuilt_for: Some("needs"),
        })
    }

    /// A graph-only session (agent-tasks design §1.2 M0-2) for `workspace`,
    /// cross-app: each build makes the program and its sited graph and no
    /// model ([`Built::Graph`]); the workspace session keeps what its next
    /// refresh reuses (per-file parses and resolutions; under `--low-mem`, no
    /// dependency trees).
    #[must_use]
    pub fn graph_only(workspace: PathBuf, instance_id: String) -> ResidentSession {
        ResidentSession {
            graph_only: true,
            ..ResidentSession::new(workspace, false, instance_id)
        }
    }

    /// Whether this session is graph-only.
    #[must_use]
    pub fn is_graph_only(&self) -> bool {
        self.graph_only
    }

    /// The session reading `injected`'s inputs too (a test universe:
    /// the production app as a source folder, its package roots), in every
    /// build's read and every manifest observation, so a change there is a
    /// change of this session's inputs.
    #[must_use]
    pub fn with_injected(self, injected: crate::program::inputs::Injected) -> ResidentSession {
        ResidentSession { injected, ..self }
    }

    /// The session building the sited graph with every revision when
    /// `sited` (agent-tasks design §1.2 M0-1; `engine-host --graph`).
    #[must_use]
    pub fn with_sited(self, sited: bool) -> ResidentSession {
        self.sited.store(sited, Ordering::SeqCst);
        self
    }

    /// Whether each build makes the sited graph.
    #[must_use]
    pub fn builds_sited(&self) -> bool {
        self.sited.load(Ordering::SeqCst)
    }

    /// The session with a build hook (tests).
    #[must_use]
    pub fn with_hook(mut self, hook: BuildHook) -> ResidentSession {
        self.hook = Some(hook);
        self
    }

    /// The session in `--low-mem` mode when `low_mem` (design §7): the
    /// cross-app workspace session keeps no dependency tree between revisions
    /// ([`crate::program::session::MemoryMode::LowMem`]). A single-app
    /// session builds cold and holds none either way.
    #[must_use]
    pub fn with_low_mem(self, low_mem: bool) -> ResidentSession {
        let mode = if low_mem {
            crate::program::session::MemoryMode::LowMem
        } else {
            crate::program::session::MemoryMode::Default
        };
        *lock(&self.builder) = Builder {
            workspace: WorkspaceSession::with_mode(mode),
        };
        ResidentSession { low_mem, ..self }
    }

    /// Whether the session keeps no dependency tree between revisions.
    #[must_use]
    pub fn low_mem(&self) -> bool {
        self.low_mem
    }

    /// The session under a memory budget (§4.5): every build reserves first.
    #[must_use]
    pub fn with_budget(mut self, budget: Arc<Budget>) -> ResidentSession {
        self.budget = Some(budget);
        self
    }

    /// The memory budget, if any.
    #[must_use]
    pub fn budget(&self) -> Option<&Arc<Budget>> {
        self.budget.as_ref()
    }

    /// The budget's counters, with the retained floor brought up to date.
    #[must_use]
    pub fn memory_status(&self) -> Option<BudgetStatus> {
        self.update_floor();
        self.budget.as_ref().map(|b| b.status())
    }

    /// Set the retained floor (`status.retainedBytes`): the RSS less the
    /// measured bytes of every evictable artifact of the published revision
    /// a reclaim could evict now ([`Revision::reclaimable_bytes`]). An
    /// artifact a request holds is no room: counted as evictable, a request
    /// holding 2.1 GB of its own needs (an impact over 699 roots on CDO, its
    /// tree and raw facts) read a floor of 3.5 GB, so a reservation that
    /// could never fit was not called impossible: the reclaim evicted the
    /// session's dependency rows (10,881 entries, which the next build must
    /// make again), the RSS fell 0.18 GB, and it refused anyway.
    pub fn update_floor(&self) {
        let Some(b) = &self.budget else { return };
        let evictable = match self.published() {
            Some(Built::Revision(r)) => r.reclaimable_bytes(),
            _ => 0,
        };
        b.set_retained_floor(b.meter().rss().saturating_sub(evictable));
    }

    /// Reserve `estimate` for `case` (§4.5): at once when it fits; refused
    /// at once when it cannot fit even above the retained floor; else
    /// admission is closed, the in-flight releasers (holders beyond `mine`,
    /// fill workers) are waited for, unpinned artifacts are evicted in the
    /// design's order until it fits, and the reservation is tried once more.
    pub fn reserve_or_reclaim<'b>(
        &self,
        budget: &'b Budget,
        case: Case,
        estimate: u64,
        mine: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<Reservation<'b>, Refusal> {
        self.reserve_or_reclaim_sized(budget, case, 1, estimate, mine, stop)
    }

    /// [`Self::reserve_or_reclaim`] for work over `units` roots (its peak
    /// is observed in that size class, `budget::size_class`).
    pub fn reserve_or_reclaim_sized<'b>(
        &self,
        budget: &'b Budget,
        case: Case,
        units: u64,
        estimate: u64,
        mine: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<Reservation<'b>, Refusal> {
        self.update_floor();
        match budget.reserve_sized(case, units, estimate) {
            Ok(r) => return Ok(r),
            Err(f) if f.impossible => return Err(f),
            Err(_) => {}
        }
        let closed = budget.close();
        budget.wait_for_releasers(mine, stop);
        self.reclaim(budget, estimate);
        self.update_floor();
        let r = budget.reserve_sized(case, units, estimate);
        drop(closed);
        r
    }

    /// Evict until `estimate` fits (§7's order): the roots' witnesses,
    /// the views' raw facts, the dependency rows the workspace session keeps (when no build
    /// holds it), runs, the det, explain's indexes, the fold, ordering and rules; never the
    /// model, relation, tier or L4 seed. Returns how many entries went.
    pub fn reclaim(&self, budget: &Budget, estimate: u64) -> u64 {
        let rev = match self.published() {
            Some(Built::Revision(r)) => Some(r),
            _ => None,
        };
        let mut evicted = 0u64;
        let mut measured = 0u64;
        let rss_before = budget.meter().rss();
        // The RSS right after the evictions, before each collect: the record
        // states what the evictions alone gave back and what the collects
        // added.
        let (mut not_collected, mut collect_ms) = (0u64, 0u64);
        let collect = crate::release_alloc::collect_after_reclaim();
        // The design's order, with the session's dependency rows right after
        // the views' raw facts.
        for kind in Revision::EVICTION_ORDER
            .iter()
            .copied()
            .flat_map(|k| std::iter::once(k).chain((k == "raw").then_some("rows")))
        {
            if budget.fits(estimate) {
                break;
            }
            let n = match (kind, &rev) {
                ("rows", _) => match self.builder.try_lock() {
                    Ok(mut b) => b.workspace.evict_dependency_rows() as u64,
                    Err(_) => 0,
                },
                (k, Some(r)) => {
                    let before = r.reclaimable_bytes_of(k);
                    let n = r.reclaim_kind(k) as u64;
                    measured += before.saturating_sub(r.reclaimable_bytes_of(k));
                    n
                }
                (_, None) => 0,
            };
            if n > 0 {
                evicted += n;
                // What went is handed back before the next `fits` reads the
                // RSS: else freed pages still count, and every later kind is
                // evicted too.
                let rss = budget.meter().rss();
                let start = Instant::now();
                if collect {
                    crate::release_alloc::collect_freed();
                }
                collect_ms += start.elapsed().as_millis() as u64;
                not_collected += rss.saturating_sub(budget.meter().rss());
            }
        }
        budget.count_evicted(evicted);
        if evicted > 0 {
            let rss_collected = budget.meter().rss();
            budget.record_reclaim(crate::engine::resident::budget::ReclaimRecord {
                entries: evicted,
                measured_bytes: measured,
                rss_before,
                rss_evicted: rss_collected + not_collected,
                rss_collected,
                collect_ms,
            });
        }
        evicted
    }

    /// The workspace, as `session/open` named it.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    #[must_use]
    pub fn single_app(&self) -> bool {
        self.single_app
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> SessionStats {
        lock(&self.stats).clone()
    }

    /// The published state, if any.
    #[must_use]
    pub fn published(&self) -> Option<Built> {
        lock(&self.current).as_ref().map(|p| p.built.clone())
    }

    /// The published state's input descriptor (agent-tasks design §1.2
    /// M0-6): immutable per published state, so a front end that read it
    /// for one generation never asks again until the generation moves.
    #[must_use]
    pub fn descriptor(&self) -> Option<InputDescriptor> {
        let p = lock(&self.current).clone()?;
        Some(InputDescriptor {
            generation: p.built.revision_id().map(|r| r.generation),
            inputs_stamp: p.manifest.stamp(),
            workspace: self.workspace.clone(),
            universe: self.universe_name(),
            manifest: p.manifest.descriptor_entries(),
        })
    }

    /// The session's universe as the descriptor names it: a test universe
    /// is `tests:<key>` (its descriptor's key, [`Self::with_universe_label`]).
    #[must_use]
    pub fn universe_name(&self) -> String {
        match &self.universe_label {
            Some(l) => l.clone(),
            None => self.base_universe_name().to_string(),
        }
    }

    /// The session with its universe named `label` in its descriptor.
    #[must_use]
    pub fn with_universe_label(self, label: String) -> ResidentSession {
        ResidentSession {
            universe_label: Some(label),
            ..self
        }
    }

    fn base_universe_name(&self) -> &'static str {
        if self.graph_only {
            "graphOnly"
        } else if self.single_app {
            "singleApp"
        } else {
            "crossApp"
        }
    }

    /// The published state's inputs stamp.
    #[must_use]
    pub fn published_stamp(&self) -> Option<String> {
        lock(&self.current).as_ref().map(|p| p.manifest.stamp())
    }

    /// The last build's change report (cross-app).
    #[must_use]
    pub fn last_refresh(&self) -> Option<crate::program::session::RefreshReport> {
        lock(&self.builder).workspace.last_report().cloned()
    }

    /// Paths a change notification named (`session/notifyChanged`): each is
    /// hashed again on the next check, whatever its stat says.
    pub fn notify_changed(&self, paths: impl IntoIterator<Item = PathBuf>) {
        lock(&self.dirty).extend(paths);
    }

    /// The state a request is answered from, by its freshness mode (see the
    /// module doc). `stop` is read before a build begins; a build that began
    /// completes and is published for later requests.
    pub fn pin(
        self: &Arc<Self>,
        mode: Freshness,
        stop: &dyn Fn() -> Option<Stop>,
    ) -> Result<Pinned, PinError> {
        let current = lock(&self.current).clone();
        let dirty = lock(&self.dirty).clone();
        if let Some(p) = &current {
            let start = Instant::now();
            let hashing = match mode {
                Freshness::Verify => Hashing::All,
                _ => Hashing::Cached {
                    prev: &p.manifest,
                    dirty: &dirty,
                },
            };
            let observed =
                Manifest::observe_scoped(&self.workspace, &self.injected, self.single_app, hashing);
            lock(&self.stats).last_check_us = Some(start.elapsed().as_micros() as u64);
            if observed == p.manifest {
                lock(&self.stats).fresh_hits += 1;
                self.clear_dirty(&dirty);
                return Ok(Pinned {
                    built: p.built.clone(),
                    stale: false,
                    changed_paths: Vec::new(),
                    waited_for_build_ms: 0,
                    rebuilt_for: None,
                });
            }
            if mode == Freshness::LatestBuilt {
                let changed = p.manifest.changed_paths(&observed, &self.workspace);
                let me = Arc::clone(self);
                std::thread::spawn(move || {
                    // A build already running will publish; never queue a second.
                    if let Ok(mut b) = me.builder.try_lock() {
                        // No request holds this build: every holder is a
                        // releaser it may wait for.
                        let _ = me.build_locked(&mut b, None, 0, &|| false);
                    }
                });
                return Ok(Pinned {
                    built: p.built.clone(),
                    stale: true,
                    changed_paths: changed,
                    waited_for_build_ms: 0,
                    rebuilt_for: None,
                });
            }
            return self.build(Some(observed), stop);
        }
        self.build(None, stop)
    }

    fn clear_dirty(&self, seen: &BTreeSet<PathBuf>) {
        if !seen.is_empty() {
            let mut d = lock(&self.dirty);
            for p in seen {
                d.remove(p);
            }
        }
    }

    /// A build (or the one another request just published), waited for.
    fn build(
        &self,
        before: Option<Manifest>,
        stop: &dyn Fn() -> Option<Stop>,
    ) -> Result<Pinned, PinError> {
        if let Some(s) = stop() {
            return Err(PinError::Stopped(s));
        }
        let start = Instant::now();
        let mut b = lock(&self.builder);
        // Another request may have published while this one waited.
        let before = match (&before, lock(&self.current).as_ref()) {
            (Some(obs), Some(p)) if *obs == p.manifest => {
                return Ok(Pinned {
                    built: p.built.clone(),
                    stale: false,
                    changed_paths: Vec::new(),
                    waited_for_build_ms: start.elapsed().as_millis() as u64,
                    rebuilt_for: None,
                });
            }
            _ => before,
        };
        // The request waiting for this build is one admitted holder.
        let built = self.build_locked(&mut b, before, 1, &|| false)?;
        Ok(Pinned {
            built,
            stale: false,
            changed_paths: Vec::new(),
            waited_for_build_ms: start.elapsed().as_millis() as u64,
            rebuilt_for: None,
        })
    }

    /// The state a request that reads the sited graph is answered from
    /// (agent-tasks design §1.2 M0-1, owner answer 2026-11-05: the graph
    /// costs ~1 s per refresh on CDO, over the 300 ms bar, so it is built
    /// only once a question needs it). [`Self::pin`]; then, when the pinned
    /// revision has no sited graph, the session builds it with every revision
    /// from now on, and adds it to the pinned revision for this request
    /// (`Pinned::rebuilt_for` says `"sited"`): a program over that revision's
    /// own read (the workspace session's, which reuses every parse; single-
    /// app, the cold builder's), its graph, published into the revision's
    /// store beside every artifact already filled. No workspace read and no
    /// new revision: a rebuild here (as first built) published a new
    /// revision with an empty store, so the next digest or explain filled
    /// everything again (CDO: 9-13 s once the first impact or tests call had
    /// run). Only when the pinned revision is no longer the published one,
    /// or its program is not the session's to rebuild (cross-app with no
    /// model-instance id), does it rebuild once. `stop` also cancels the
    /// graph's build (polled every `CANCEL_EVERY` sites): the revision then
    /// stays without it, and the next such request builds it.
    pub fn pin_sited(
        self: &Arc<Self>,
        mode: Freshness,
        stop: &dyn Fn() -> Option<Stop>,
    ) -> Result<Pinned, PinError> {
        let pinned = self.pin(mode, stop)?;
        match &pinned.built {
            Built::Revision(r) if r.sited().is_none() => {}
            _ => return Ok(pinned),
        }
        self.sited.store(true, Ordering::SeqCst);
        if let Some(s) = stop() {
            return Err(PinError::Stopped(s));
        }
        let start = Instant::now();
        let mut b = lock(&self.builder);
        // Another request may have published a revision with it meanwhile.
        if let Some(Built::Revision(r)) = self.published()
            && r.sited().is_some()
        {
            return Ok(Pinned {
                built: Built::Revision(r),
                waited_for_build_ms: start.elapsed().as_millis() as u64,
                ..pinned
            });
        }
        if let (Some(Built::Revision(current)), Built::Revision(mine)) =
            (self.published(), &pinned.built)
            && Arc::ptr_eq(&current, mine)
            && (self.single_app || current.inputs.model_instance_id.is_some())
        {
            return self.add_sited_locked(&mut b, current, pinned, start, stop);
        }
        let built = self.build_locked(&mut b, None, 1, &|| stop().is_some())?;
        Ok(Pinned {
            built,
            stale: false,
            changed_paths: Vec::new(),
            waited_for_build_ms: pinned.waited_for_build_ms + start.elapsed().as_millis() as u64,
            rebuilt_for: Some("sited"),
        })
    }

    /// [`Self::pin_sited`]'s addition, under the builder lock: the sited
    /// graph of a program over `rev`'s own read, published into `rev`'s
    /// store. The reservation is a build's (the program is the incremental
    /// build's first half, the graph its addition, M0-5), released without
    /// recording an observation: this is not a build, and its smaller peak
    /// must not lower the next build's estimate.
    fn add_sited_locked(
        &self,
        b: &mut Builder,
        rev: Arc<Revision>,
        pinned: Pinned,
        start: Instant,
        stop: &dyn Fn() -> Option<Stop>,
    ) -> Result<Pinned, PinError> {
        let reservation = match &self.budget {
            Some(budget) => {
                let sizes = Default::default();
                let estimate = budget
                    .estimate(Case::Incremental, sizes)
                    .saturating_add(budget.estimate(Case::Fill("sited"), sizes));
                Some(
                    self.reserve_or_reclaim(budget, Case::Incremental, estimate, 1, &|| false)
                        .map_err(PinError::Budget)?,
                )
            }
            None => None,
        };
        self.hook(BuildPoint::BeforeSited, 0);
        let program = if self.single_app {
            let _s = crate::engine::perf_trace::span("revision", "revision.sited_program");
            crate::program::resolved::ResolvedProgram::build(
                &rev.inputs,
                crate::program::resolved::ProgramScope::Workspace,
            )
        } else {
            b.workspace.refresh_program(&rev.inputs)
        };
        let sited = program
            .ok()
            .and_then(|p| build_sited(&p, &|| stop().is_some()));
        drop(reservation);
        let waited = pinned.waited_for_build_ms + start.elapsed().as_millis() as u64;
        match sited {
            Some(s) => {
                {
                    let mut st = lock(&self.stats);
                    st.sited_builds += 1;
                    st.last_sited_ms = Some(s.ms);
                    st.last_sited_bytes = Some(s.graph.bytes());
                }
                rev.add_sited(Arc::new(s.graph));
                self.update_floor();
                Ok(Pinned {
                    built: Built::Revision(rev),
                    waited_for_build_ms: waited,
                    rebuilt_for: Some("sited"),
                    ..pinned
                })
            }
            // Stopped: the revision stays without the graph (the next such
            // request builds it). A program that did not build (the
            // revision's own did) leaves the answer to say the graph is
            // missing.
            None => match stop() {
                Some(s) => Err(PinError::Stopped(s)),
                None => Ok(Pinned {
                    waited_for_build_ms: waited,
                    ..pinned
                }),
            },
        }
    }

    /// The build itself, under the builder lock (see the module doc).
    /// `cancel_sited` stops the sited graph's build (the revision is then
    /// published without it).
    fn build_locked(
        &self,
        b: &mut Builder,
        mut before: Option<Manifest>,
        mine: u64,
        cancel_sited: &dyn Fn() -> bool,
    ) -> Result<Built, PinError> {
        let start = Instant::now();
        let started_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut unstable = String::new();
        for attempt in 0..BUILD_ATTEMPTS {
            let dirty = lock(&self.dirty).clone();
            let prev = lock(&self.current).clone();
            let m_before = match before.take() {
                Some(m) => m,
                None => match &prev {
                    Some(p) => Manifest::observe_scoped(
                        &self.workspace,
                        &self.injected,
                        self.single_app,
                        Hashing::Cached {
                            prev: &p.manifest,
                            dirty: &dirty,
                        },
                    ),
                    None => Manifest::observe_scoped(
                        &self.workspace,
                        &self.injected,
                        self.single_app,
                        Hashing::All,
                    ),
                },
            };
            // Reserve before anything is read (§4.5): the pre-model estimate
            // from the observed input sizes for a first build, the table's
            // scaled estimate beside a published revision.
            let reservation = match &self.budget {
                Some(budget) => {
                    let case = match prev.as_ref().map(|p| &p.built) {
                        Some(Built::Revision(_)) => Case::Incremental,
                        _ => Case::Cold,
                    };
                    let mut estimate = budget.estimate(case, m_before.input_sizes());
                    // The sited graph joins the build's reservation (M0-5):
                    // it is built inside the build, beside the lent program.
                    if self.graph_only || self.sited.load(Ordering::SeqCst) {
                        estimate += budget.estimate(Case::Fill("sited"), m_before.input_sizes());
                    }
                    Some(
                        self.reserve_or_reclaim(budget, case, estimate, mine, &|| false)
                            .map_err(PinError::Budget)?,
                    )
                }
                None => None,
            };
            // The published revision's trees (§6.11) seed nothing in the next
            // one, which rebuilds its whole sections and its views: free the
            // ones no request holds before the build, not after it, so they
            // are not part of its peak.
            if let Some(Built::Revision(r)) = prev.as_ref().map(|p| &p.built) {
                // Nor its witnesses (plan 2026-11-03 W: never carried), nor
                // explain's indexes (W2: the next model's are built anew).
                r.reclaim_kind("witness");
                r.reclaim_kind("explain_indexes");
                r.reclaim_kind("tree");
                r.reclaim_kind("tree_base");
            }
            self.hook(BuildPoint::BeforeRead, attempt);
            let inputs =
                WorkspaceInputs::read_scoped(&self.workspace, &REVISION_READ, &self.injected);
            self.hook(BuildPoint::AfterRead, attempt);
            let consumed = Manifest::consumed(&inputs, self.single_app);
            let want_sited = self.graph_only || self.sited.load(Ordering::SeqCst);
            let needs = *lock(&self.needs);
            let mut sited: Option<SitedBuild> = None;
            let target = if self.graph_only {
                // The program and its sited graph, no model (M0-2): the
                // workspace session still keeps what its next refresh reuses.
                match b.workspace.refresh_program(&inputs) {
                    Ok(p) => {
                        sited = build_sited(&p, &|| false);
                        Ok(None)
                    }
                    Err(e) => Err(ViewFailure {
                        target: TargetFailure::CrossApp(
                            crate::program::model::program_calls::CrossAppFailure::Build(e),
                        ),
                        analyze: None,
                        inputs: Arc::clone(&inputs),
                    }),
                }
            } else {
                build_target_with(
                    &mut b.workspace,
                    &inputs,
                    self.single_app,
                    needs,
                    &mut |p| {
                        if want_sited {
                            sited = build_sited(p, cancel_sited);
                        }
                    },
                )
                .map(Some)
            };
            self.hook(BuildPoint::AfterBuild, attempt);
            let no_dirty = BTreeSet::new();
            let m_after = Manifest::observe_scoped(
                &self.workspace,
                &self.injected,
                self.single_app,
                Hashing::Cached {
                    prev: &m_before,
                    dirty: &no_dirty,
                },
            );
            if let Err(e) = check_consistent(&m_before, &consumed, &m_after) {
                unstable = e.to_string();
                lock(&self.stats).discarded_builds += 1;
                continue;
            }
            let next_id = || RevisionId {
                engine_instance_id: self.instance_id.clone(),
                generation: self.generation.fetch_add(1, Ordering::SeqCst) + 1,
                inputs_stamp: consumed.stamp(),
            };
            let count_sited = |s: &SitedBuild| {
                let mut st = lock(&self.stats);
                st.sited_builds += 1;
                st.last_sited_ms = Some(s.ms);
                st.last_sited_bytes = Some(s.graph.bytes());
            };
            let built = match target {
                Ok(None) => match sited.take() {
                    Some(s) => {
                        count_sited(&s);
                        Built::Graph(Arc::new(GraphRevision {
                            id: next_id(),
                            inputs,
                            sited: Arc::new(s.graph),
                        }))
                    }
                    None => unreachable!("an uncancellable sited build finishes"),
                },
                Ok(Some(target)) => {
                    let id = next_id();
                    // The L4 seed of the last published revision (§6.9):
                    // the new revision's solve starts from it when the
                    // seed key matches.
                    let seed_from = match prev.as_ref().map(|p| &p.built) {
                        Some(Built::Revision(r)) => Some(&**r),
                        _ => None,
                    };
                    let mut rev = Revision::seeded(id, inputs, target, needs.assembly(), seed_from);
                    if let Some(s) = sited.take() {
                        count_sited(&s);
                        rev = rev.with_sited(Arc::new(s.graph));
                    }
                    Built::Revision(Arc::new(rev))
                }
                Err(failure) => Built::Failed(Arc::new(FailedBuild { inputs, failure })),
            };
            let class = if self.single_app {
                "cold".to_string()
            } else {
                b.workspace.last_report().map_or_else(
                    || "cold".to_string(),
                    |r| {
                        match r.class {
                            ChangeClass::BodyOnly => "B",
                            ChangeClass::Declaration => "S",
                            ChangeClass::Package => "P",
                        }
                        .to_string()
                    },
                )
            };
            {
                let mut s = lock(&self.stats);
                s.builds += 1;
                s.last_build_class = Some(class);
                s.last_build_ms = Some(start.elapsed().as_millis() as u64);
                s.last_build_started_unix_ms = Some(started_unix_ms);
            }
            *lock(&self.current) = Some(Arc::new(Published {
                built: built.clone(),
                manifest: m_after,
                needs: needs.assembly(),
            }));
            self.clear_dirty(&dirty);
            if let Some(r) = reservation {
                r.finish();
            }
            if let (Some(budget), Built::Revision(rev)) = (&self.budget, &built) {
                budget.estimates().routines = Some(rev.routine_count());
            }
            // The previous revision is released here (unless a request still
            // holds it); the floor is measured without it.
            drop(prev);
            self.update_floor();
            return Ok(built);
        }
        Err(PinError::Unstable(format!(
            "{unstable} (after {BUILD_ATTEMPTS} attempts)"
        )))
    }

    fn hook(&self, point: BuildPoint, attempt: usize) {
        if let Some(h) = &self.hook {
            h(point, attempt);
        }
    }
}

/// The target of a revision over `inputs` (the view builder's rules for a
/// model view with [`REVISION_NEEDS`]): cross-app through the workspace
/// session; single-app, or a workspace with no model-instance id (whose
/// failure the cold builder words), through the cold builder. Public for the
/// byte census (`tools/census-probe resident`).
#[doc(hidden)]
pub fn build_target(
    session: &mut WorkspaceSession,
    inputs: &Arc<WorkspaceInputs>,
    single_app: bool,
) -> Result<AnalysisTarget, ViewFailure> {
    build_target_with(session, inputs, single_app, REVISION_NEEDS, &mut |_| {})
}

/// What a revision's build made beside its target: the sited graph (agent-
/// tasks design §1.2 M0-1), when the session builds it, and its build's
/// wall time.
pub struct SitedBuild {
    pub graph: crate::program::impact_graph::SitedGraph,
    pub ms: u64,
}

/// The sited graph of `program`, timed, or `None` when `cancel` stopped it.
#[must_use]
pub fn build_sited(
    program: &crate::program::resolved::ResolvedProgram,
    cancel: &dyn Fn() -> bool,
) -> Option<SitedBuild> {
    let start = Instant::now();
    crate::program::impact_graph::sited_from_program(program, cancel)
        .ok()
        .map(|graph| SitedBuild {
            graph,
            ms: start.elapsed().as_millis() as u64,
        })
}

/// [`build_target`], lending the program to `on_program` between its build
/// and the model's (agent-tasks design §1.2 M0-1). A workspace with no
/// model-instance id builds no program here (its failure is the cold
/// builder's). `needs` is the model the revision builds (the session's,
/// [`REVISION_NEEDS`] unless a request widened them, M0-4).
pub fn build_target_with(
    session: &mut WorkspaceSession,
    inputs: &Arc<WorkspaceInputs>,
    single_app: bool,
    needs: ViewNeeds,
    on_program: &mut dyn FnMut(&crate::program::resolved::ResolvedProgram),
) -> Result<AnalysisTarget, ViewFailure> {
    if single_app {
        return crate::engine::gate::run::build_single_app_model_lending(inputs, needs, on_program);
    }
    if inputs.model_instance_id.is_none() {
        let key = ViewKey::new(&inputs.workspace, single_app, false);
        return build_view_model(inputs, &key, needs, |_| {})
            .map(|m| m.expect("a model view builds a model").target);
    }
    let _s = crate::engine::perf_trace::span("crossapp", "crossapp.base_total");
    session
        .refresh_lending(inputs, false, needs.assembly(), on_program)
        .and_then(crate::engine::l4::capability_cone::cross_app_base_of)
        .map(|base| AnalysisTarget::CrossApp(Box::new(base)))
        .map_err(|e| ViewFailure {
            target: TargetFailure::CrossApp(e),
            analyze: None,
            inputs: Arc::clone(inputs),
        })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "incr_tests.rs"]
mod incr_tests;

#[cfg(test)]
#[path = "cone_tests.rs"]
mod cone_tests;
