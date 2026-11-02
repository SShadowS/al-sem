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
use crate::engine::resident::manifest::{Hashing, Manifest, check_consistent};
use crate::engine::resident::revision::Revision;
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

/// The model a revision builds: what every served command's view needs
/// (explain's rules and card text on top of the model).
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

/// A build that did not produce a model: its read and its failure.
pub struct FailedBuild {
    pub inputs: Arc<WorkspaceInputs>,
    pub failure: ViewFailure,
}

/// What the session publishes.
#[derive(Clone)]
pub enum Built {
    Revision(Arc<Revision>),
    Failed(Arc<FailedBuild>),
}

impl Built {
    /// The revision's identity, or `None` for a failed build.
    #[must_use]
    pub fn revision_id(&self) -> Option<RevisionId> {
        match self {
            Built::Revision(r) => Some(r.id.clone()),
            Built::Failed(_) => None,
        }
    }

    /// The read the published state was built from.
    #[must_use]
    pub fn inputs(&self) -> &Arc<WorkspaceInputs> {
        match self {
            Built::Revision(r) => &r.inputs,
            Built::Failed(f) => &f.inputs,
        }
    }
}

/// The published state and the manifest it was built from (`M_after`, with
/// its stats, the next check's cache).
struct Published {
    built: Built,
    manifest: Manifest,
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
}

/// Why no state was pinned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinError {
    /// The request stopped before a build began.
    Stopped(Stop),
    /// Every attempt's three observations disagreed.
    Unstable(String),
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
    /// The last freshness check's wall time, in microseconds.
    pub last_check_us: Option<u64>,
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
        }
    }

    /// The session with a build hook (tests).
    #[must_use]
    pub fn with_hook(mut self, hook: BuildHook) -> ResidentSession {
        self.hook = Some(hook);
        self
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
            let observed = Manifest::observe(&self.workspace, self.single_app, hashing);
            lock(&self.stats).last_check_us = Some(start.elapsed().as_micros() as u64);
            if observed == p.manifest {
                lock(&self.stats).fresh_hits += 1;
                self.clear_dirty(&dirty);
                return Ok(Pinned {
                    built: p.built.clone(),
                    stale: false,
                    changed_paths: Vec::new(),
                    waited_for_build_ms: 0,
                });
            }
            if mode == Freshness::LatestBuilt {
                let changed = p.manifest.changed_paths(&observed, &self.workspace);
                let me = Arc::clone(self);
                std::thread::spawn(move || {
                    // A build already running will publish; never queue a second.
                    if let Ok(mut b) = me.builder.try_lock() {
                        let _ = me.build_locked(&mut b, None);
                    }
                });
                return Ok(Pinned {
                    built: p.built.clone(),
                    stale: true,
                    changed_paths: changed,
                    waited_for_build_ms: 0,
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
                });
            }
            _ => before,
        };
        let built = self.build_locked(&mut b, before)?;
        Ok(Pinned {
            built,
            stale: false,
            changed_paths: Vec::new(),
            waited_for_build_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// The build itself, under the builder lock (see the module doc).
    fn build_locked(
        &self,
        b: &mut Builder,
        mut before: Option<Manifest>,
    ) -> Result<Built, PinError> {
        let start = Instant::now();
        let mut unstable = String::new();
        for attempt in 0..BUILD_ATTEMPTS {
            let dirty = lock(&self.dirty).clone();
            let prev = lock(&self.current).clone();
            let m_before = match before.take() {
                Some(m) => m,
                None => match &prev {
                    Some(p) => Manifest::observe(
                        &self.workspace,
                        self.single_app,
                        Hashing::Cached {
                            prev: &p.manifest,
                            dirty: &dirty,
                        },
                    ),
                    None => Manifest::observe(&self.workspace, self.single_app, Hashing::All),
                },
            };
            self.hook(BuildPoint::BeforeRead, attempt);
            let inputs = WorkspaceInputs::read(&self.workspace, &REVISION_READ);
            self.hook(BuildPoint::AfterRead, attempt);
            let consumed = Manifest::consumed(&inputs, self.single_app);
            let target = build_target(&mut b.workspace, &inputs, self.single_app);
            self.hook(BuildPoint::AfterBuild, attempt);
            let no_dirty = BTreeSet::new();
            let m_after = Manifest::observe(
                &self.workspace,
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
            let built = match target {
                Ok(target) => {
                    let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
                    let id = RevisionId {
                        engine_instance_id: self.instance_id.clone(),
                        generation,
                        inputs_stamp: consumed.stamp(),
                    };
                    Built::Revision(Arc::new(Revision::new(id, inputs, target)))
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
            }
            *lock(&self.current) = Some(Arc::new(Published {
                built: built.clone(),
                manifest: m_after,
            }));
            self.clear_dirty(&dirty);
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
/// failure the cold builder words), through the cold builder.
fn build_target(
    session: &mut WorkspaceSession,
    inputs: &Arc<WorkspaceInputs>,
    single_app: bool,
) -> Result<AnalysisTarget, ViewFailure> {
    if single_app || inputs.model_instance_id.is_none() {
        let key = ViewKey::new(&inputs.workspace, single_app, false);
        return build_view_model(inputs, &key, REVISION_NEEDS, |_| {})
            .map(|m| m.expect("a model view builds a model").target);
    }
    let _s = crate::engine::perf_trace::span("crossapp", "crossapp.base_total");
    session
        .refresh(inputs, false, REVISION_NEEDS.assembly())
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
