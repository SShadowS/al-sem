//! A resident revision (context-server design §4.1-§4.4): one built target
//! over one acquired workspace read, immutable, and its artifact store. A
//! request is answered off it in the design's order: resolve (the job's plan
//! part against the revision's model), [`Store::ensure_prepared`] (the ready
//! snapshot), the request's trees off that snapshot, then the job's compute
//! over a served [`View`] holding `Arc` clones of the revision's cells. The
//! answer is the CLI's exact output for the same inputs.
//!
//! **Artifacts** ([`ArtifactKey`], by phase): 0: `Ordering`, `Rules` (from
//! the model alone), `Fold` (the one composition: every derived row, the pd
//! arena, and the primary routines' raw facts), `L4` (the solve and its core
//! summaries, never drained); 1: `Raw(subjects)` (the raw facts of subjects
//! outside the primary emission: a dependency root), `Det` (the detector
//! artifacts, harvested from the lent `Fold` and `L4`, at a mask); 2:
//! `Run(key)`. Trees are the request's own (stage 5 retains their sections).
//!
//! **Job lines.** A cold run prints the lines `prepare` printed (the solve's
//! non-convergence line, …) only when its plan solves `l4`. Each artifact
//! keeps the lines its fill printed, and an answer replays them in the order
//! a cold `prepare` prints them, whether or not this request filled them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::engine::batch::job::{Job, JobFailure, LaterReads, ResolveCx, ViewCx};
use crate::engine::batch::output::{JobOutput, capture};
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::analysis::{CoreSummaries, L4Analysis, L4Request, build_l4_controlled};
use crate::engine::l4::cone_derived::ConeScope;
use crate::engine::l4::exec_relation::ExecRelation;
use crate::engine::l4::inherited_facts::InheritedFacts;
use crate::engine::l4::summary_runner::{L4Control, SolveBaseline};
use crate::engine::l5::detector_context::{
    self as dc, CapabilityAnalysis, CapsInput, CoreInput, DetectorArtifacts, L4Input,
    OrderingFactsMap,
};
use crate::engine::l5::digest_cli::DigestEffects;
use crate::engine::l5::registry::{RunOutput, substrate};
use crate::engine::l5::rules_index::RulesIndex;
use crate::engine::resident::store::{
    Control, FillCx, PrepareError, Snapshot, Store, StoreKey, StoreStats, StoreValue,
};
use crate::engine::session::{
    JobId, Plan, RunKey, TreeReadOut, View, WitnessRequest, compose_cone, emit_raw_ids, key,
    read_tree, relation_of, run_once, tree_apps, tree_groups,
};
use crate::program::inputs::WorkspaceInputs;
use crate::program::model::calls::CallEdge;
use crate::wire::RevisionId;

/// The commands a revision answers off its retained cells: one view, the
/// model the revision built ([`crate::engine::batch::job::ViewNeeds::EXPLAIN`]
/// covers each), no analyze extras, no program. Every other command runs the
/// one-shot job path over the revision's inputs.
pub const SERVED_COMMANDS: &[&str] = &["query", "digest", "explain", "prove"];

/// A key of a revision's artifact store (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactKey {
    Ordering,
    Rules,
    Fold,
    L4,
    /// The raw facts of these subjects (internal ids), outside the primary
    /// emission.
    Raw(BTreeSet<String>),
    Det,
    /// One run, with whether its plan filled ordering and rules (a run's
    /// context reads them when they are filled, `run_once`).
    Run {
        key: RunKey,
        ordering: bool,
        rules: bool,
    },
}

impl StoreKey for ArtifactKey {
    fn phase(&self) -> usize {
        match self {
            ArtifactKey::Ordering | ArtifactKey::Rules | ArtifactKey::Fold | ArtifactKey::L4 => 0,
            ArtifactKey::Raw(_) | ArtifactKey::Det => 1,
            ArtifactKey::Run { .. } => 2,
        }
    }
}

/// The one composition a revision keeps.
pub struct Fold {
    pub(crate) caps: CapabilityAnalysis,
    /// The primary routines' raw facts.
    pub(crate) raw: InheritedFacts,
}

/// The retained solve: never drained, so a det refill and the next seeded
/// solve read it (`CoreInput::Borrowed`, `L4Reuse::core`).
pub struct Solved {
    pub l4: Arc<L4Analysis>,
    pub core: Arc<CoreSummaries>,
    pub baseline: SolveBaseline,
}

/// An artifact's value.
pub enum ArtifactValue {
    Ordering(OrderingFactsMap),
    Rules(RulesIndex),
    Fold(Fold),
    L4(Solved),
    Raw(InheritedFacts),
    Det(Arc<DetectorArtifacts>),
    Run(Arc<RunOutput>),
}

/// An artifact: its value and the job lines its fill printed.
pub struct Artifact {
    pub value: ArtifactValue,
    pub lines: Vec<String>,
}

impl StoreValue for Artifact {
    /// The det's mask (0 for every other key).
    type Req = u32;
    fn satisfies(&self, mask: &u32) -> bool {
        match &self.value {
            ArtifactValue::Det(d) => mask & !d.demanded == 0,
            _ => true,
        }
    }
}

/// A request's tree reads: each tree job's effects and fingerprint reads.
type TreeReads = (BTreeMap<JobId, DigestEffects>, BTreeMap<JobId, TreeReadOut>);

/// A revision's ready snapshot.
pub type ReadySnapshot = Arc<Snapshot<ArtifactKey, Artifact>>;

/// One built revision (see the module doc).
pub struct Revision {
    pub id: RevisionId,
    /// The acquired read the revision was built from; every job answered off
    /// the revision is made over it (the request's `PlanCx::inputs`).
    pub inputs: Arc<WorkspaceInputs>,
    pub single_app: bool,
    target: Arc<AnalysisTarget>,
    rel: Arc<ExecRelation>,
    dependency_calls: Option<Arc<Vec<CallEdge>>>,
    /// The internal ids of the primary routines (the `Fold`'s raw subjects).
    primary: BTreeSet<String>,
    store: Store<ArtifactKey, Artifact>,
    answered: std::sync::atomic::AtomicU64,
    /// Fills per artifact kind (`status`, the T-order fill counters).
    fills: std::sync::Mutex<BTreeMap<&'static str, u64>>,
}

impl ArtifactKey {
    /// The kind's name (fill counters).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            ArtifactKey::Ordering => "ordering",
            ArtifactKey::Rules => "rules",
            ArtifactKey::Fold => "fold",
            ArtifactKey::L4 => "l4",
            ArtifactKey::Raw(_) => "raw",
            ArtifactKey::Det => "det",
            ArtifactKey::Run { .. } => "run",
        }
    }
}

impl Revision {
    /// A revision over `target`, built from `inputs`.
    #[must_use]
    pub fn new(id: RevisionId, inputs: Arc<WorkspaceInputs>, target: AnalysisTarget) -> Revision {
        let rel = relation_of(&target);
        let dependency_calls = match &target {
            AnalysisTarget::CrossApp(b) => b.dependency_calls.clone(),
            AnalysisTarget::SingleApp(_) => None,
        };
        let primary = WitnessRequest::primary().subjects(target.resolved());
        Revision {
            id,
            inputs,
            single_app: matches!(target, AnalysisTarget::SingleApp(_)),
            target: Arc::new(target),
            rel,
            dependency_calls,
            primary,
            store: Store::new(),
            answered: std::sync::atomic::AtomicU64::new(0),
            fills: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// The revision's target.
    #[must_use]
    pub fn target(&self) -> &AnalysisTarget {
        &self.target
    }

    /// The artifact store's counters.
    #[must_use]
    pub fn stats(&self) -> StoreStats {
        self.store.stats()
    }

    /// The current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ReadySnapshot {
        self.store.load()
    }

    /// Evict artifacts (§4.2 "Eviction"); keys pinned by a request in
    /// flight stay. Returns how many were evicted.
    pub fn evict(&self, keys: &[ArtifactKey]) -> usize {
        self.store.evict(keys)
    }

    /// Every key the current snapshot holds of `pred`'s kind.
    #[must_use]
    pub fn keys_where(&self, pred: impl Fn(&ArtifactKey) -> bool) -> Vec<ArtifactKey> {
        self.store
            .load()
            .keys()
            .filter(|k| pred(k))
            .cloned()
            .collect()
    }

    /// The closure of `plan`'s needs, every prerequisite listed, with the
    /// det's mask.
    fn needs_of(&self, plan: &Plan) -> Vec<(ArtifactKey, u32)> {
        let union = plan.union();
        let own = plan.own_bits();
        let ordering = (union | own) & substrate::ORDERING_FACTS != 0;
        let rules = (union | own) & substrate::RULES != 0;
        let det = !plan.run_entries().is_empty();
        let fold = plan.keeps_raw() || (det && plan.needs_derived());
        let mut needs = Vec::new();
        if ordering {
            needs.push((ArtifactKey::Ordering, 0));
        }
        if rules {
            needs.push((ArtifactKey::Rules, 0));
        }
        if fold {
            needs.push((ArtifactKey::Fold, 0));
        }
        if plan.builds_l4() {
            needs.push((ArtifactKey::L4, 0));
        }
        for scope in self.tree_scopes(plan) {
            if !scope.is_subset(&self.primary) {
                needs.push((ArtifactKey::Raw(scope), 0));
            }
        }
        if det {
            needs.push((ArtifactKey::Det, union));
        }
        for entry in plan.run_entries() {
            needs.push((
                ArtifactKey::Run {
                    key: key(entry),
                    ordering,
                    rules,
                },
                0,
            ));
        }
        needs
    }

    /// Each tree group's subjects (internal ids), in group order.
    fn tree_scopes(&self, plan: &Plan) -> Vec<BTreeSet<String>> {
        let model = self.target.resolved();
        let trees = plan.tree_demands();
        tree_groups(trees)
            .iter()
            .map(|g| {
                g.iter()
                    .fold(WitnessRequest::empty(), |w, &i| {
                        w.union(&trees[i].1.subjects)
                    })
                    .subjects(model)
            })
            .collect()
    }

    /// One artifact's fill, off the snapshot holding its prerequisites; a
    /// run's detectors come from the request's `plan`.
    fn fill(&self, plan: &Plan, cx: FillCx<'_, ArtifactKey, Artifact>) -> Result<Artifact, String> {
        let target = &*self.target;
        let model = target.resolved();
        *self
            .fills
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(cx.key.kind())
            .or_default() += 1;
        let (value, lines) = capture(|| -> Result<ArtifactValue, String> {
            Ok(match cx.key {
                ArtifactKey::Ordering => ArtifactValue::Ordering(dc::fill_ordering(model)),
                ArtifactKey::Rules => ArtifactValue::Rules(dc::fill_rules(model)),
                ArtifactKey::Fold => {
                    let primary = WitnessRequest::primary();
                    let scope = ConeScope {
                        derived: true,
                        raw: primary.scope(model),
                    };
                    let (caps, raw) = compose_cone(model, &self.rel, scope);
                    ArtifactValue::Fold(Fold { caps, raw })
                }
                ArtifactKey::L4 => {
                    // A shared fill is never cancelled by a request (§4.6).
                    let never = || false;
                    let req = L4Request {
                        extra_diagnostics: vec![],
                        control: Some(L4Control {
                            cancel: &never,
                            reuse: None,
                        }),
                    };
                    let cross = target.cross_app_inputs();
                    let (l4, core, outcome) =
                        build_l4_controlled(model, &self.rel, req, cross.as_ref())
                            .map_err(|_| "the solve was cancelled".to_string())?;
                    ArtifactValue::L4(Solved {
                        l4: Arc::new(l4),
                        core: Arc::new(core),
                        baseline: outcome
                            .ok_or("a controlled solve reports its outcome")?
                            .baseline,
                    })
                }
                ArtifactKey::Raw(subjects) => {
                    ArtifactValue::Raw(emit_raw_ids(model, &self.rel, subjects.clone()))
                }
                ArtifactKey::Det => {
                    // A det refill keeps what the det it replaces covered
                    // when the snapshot holds what that wider mask reads.
                    let wider = cx.existing.and_then(|a| match &a.value {
                        ArtifactValue::Det(d) => Some(d.demanded | cx.req),
                        _ => None,
                    });
                    let readable = |mask: u32| {
                        (mask & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) == 0
                            || cx.snapshot.get(&ArtifactKey::Fold).is_some())
                            && (mask & substrate::CORE_SUMMARIES == 0
                                || cx.snapshot.get(&ArtifactKey::L4).is_some())
                    };
                    let mask = wider.filter(|m| readable(*m)).unwrap_or(*cx.req);
                    let caps = if mask & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) != 0
                    {
                        match cx.snapshot.get(&ArtifactKey::Fold).map(|a| &a.value) {
                            Some(ArtifactValue::Fold(f)) => Some(CapsInput::Borrowed(&f.caps)),
                            _ => {
                                return Err("the det needs the fold, which is not published".into());
                            }
                        }
                    } else {
                        None
                    };
                    let l4 = if mask & substrate::CORE_SUMMARIES != 0 {
                        match cx.snapshot.get(&ArtifactKey::L4).map(|a| &a.value) {
                            Some(ArtifactValue::L4(s)) => {
                                L4Input::Solved(&s.l4, CoreInput::Borrowed(&s.core))
                            }
                            _ => return Err("the det needs l4, which is not published".into()),
                        }
                    } else {
                        L4Input::None
                    };
                    let (art, _) = dc::harvest_detector_artifacts(
                        model,
                        mask,
                        &self.rel,
                        caps,
                        l4,
                        target.cross_app_inputs(),
                    );
                    ArtifactValue::Det(Arc::new(art))
                }
                ArtifactKey::Run {
                    key: run_key,
                    ordering,
                    rules,
                } => {
                    let det = match cx.snapshot.get(&ArtifactKey::Det).map(|a| &a.value) {
                        Some(ArtifactValue::Det(d)) => d,
                        _ => return Err("a run needs the det, which is not published".into()),
                    };
                    let ordering = match cx.snapshot.get(&ArtifactKey::Ordering) {
                        Some(a) if *ordering => match &a.value {
                            ArtifactValue::Ordering(o) => Some(o),
                            _ => None,
                        },
                        _ => None,
                    };
                    let rules = match cx.snapshot.get(&ArtifactKey::Rules) {
                        Some(a) if *rules => match &a.value {
                            ArtifactValue::Rules(r) => Some(r),
                            _ => None,
                        },
                        _ => None,
                    };
                    let l4 = match cx.snapshot.get(&ArtifactKey::L4).map(|a| &a.value) {
                        Some(ArtifactValue::L4(s)) => Some(&*s.l4),
                        _ => None,
                    };
                    let entry = plan
                        .run_entries()
                        .iter()
                        .find(|e| key(e) == *run_key)
                        .ok_or("a run the request's plan does not name")?;
                    ArtifactValue::Run(Arc::new(run_once(
                        model,
                        det,
                        ordering,
                        rules,
                        l4,
                        entry,
                        run_key,
                        self.cross_base(),
                    )))
                }
            })
        });
        Ok(Artifact {
            value: value?,
            lines,
        })
    }

    fn cross_base(&self) -> Option<&crate::engine::l4::capability_cone::R3a5CrossAppBase> {
        match &*self.target {
            AnalysisTarget::SingleApp(_) => None,
            AnalysisTarget::CrossApp(b) => Some(b),
        }
    }

    /// The plan's trees, each over the ready snapshot's composition: raw
    /// borrowed from the primary emission or a `Raw` artifact, the pd arena
    /// and the dependency calls lent. Stops between groups.
    fn trees(
        &self,
        plan: &Plan,
        snap: &ReadySnapshot,
        ctl: &Control<'_>,
    ) -> Result<TreeReads, PrepareError> {
        use crate::engine::l5::snapshot::{SubjectRaw, TreeRaw, compose_tree};
        let (mut effects, mut tree_reads) = (BTreeMap::new(), BTreeMap::new());
        let trees = plan.tree_demands();
        if trees.is_empty() {
            return Ok((effects, tree_reads));
        }
        let target = &*self.target;
        let model = target.resolved();
        let apps = tree_apps(self.cross_base());
        let Some(ArtifactValue::Fold(fold)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
            return Err(PrepareError::Engine("a tree without the fold".into()));
        };
        let pd = fold
            .caps
            .pd
            .as_ref()
            .ok_or_else(|| PrepareError::Engine("the fold kept no pd arena".into()))?;
        let calls = self
            .dependency_calls
            .as_deref()
            .map_or(&[][..], Vec::as_slice);
        for (group, scope) in tree_groups(trees).iter().zip(self.tree_scopes(plan)) {
            if let Some(stop) = (ctl.stop)() {
                return Err(stop.into());
            }
            let map = if scope.is_subset(&self.primary) {
                &fold.raw
            } else {
                match snap.get(&ArtifactKey::Raw(scope.clone())).map(|a| &a.value) {
                    Some(ArtifactValue::Raw(r)) => r,
                    _ => {
                        return Err(PrepareError::Engine(
                            "a tree's raw facts are missing".into(),
                        ));
                    }
                }
            };
            let tree = compose_tree(
                model,
                self.rel.cone(),
                calls,
                &fold.caps,
                TreeRaw::Borrow(SubjectRaw::new(map, &scope)),
                std::borrow::Cow::Borrowed(pd),
                &apps,
            );
            read_tree(
                &mut Some(tree),
                trees,
                group,
                target,
                &mut effects,
                &mut tree_reads,
            );
        }
        Ok((effects, tree_reads))
    }

    /// `job` (one of [`SERVED_COMMANDS`], made over [`Revision::inputs`])
    /// answered off this revision: the exact output a standalone run of it
    /// writes. `Err` only when the request stopped or a fill failed.
    pub fn answer(&self, job: &dyn Job, ctl: &Control<'_>) -> Result<JobOutput, PrepareError> {
        let r = self.answer_inner(job, ctl);
        if r.is_ok() {
            self.answered
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        r
    }

    /// How many requests this revision answered.
    #[must_use]
    pub fn answered(&self) -> u64 {
        self.answered.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How often each artifact kind was filled on this revision.
    #[must_use]
    pub fn fill_counts(&self) -> BTreeMap<&'static str, u64> {
        self.fills.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// `plan` prepared and served as a view (the request's trees read off
    /// the ready snapshot), with the job lines `prepare` would have printed
    /// before and after the trees.
    pub(crate) fn serve_plan(
        &self,
        plan: &Plan,
        ctl: &Control<'_>,
    ) -> Result<(View, Vec<String>), PrepareError> {
        let needs = self.needs_of(plan);
        let snap = self
            .store
            .ensure_prepared(&needs, &|_| 0, &|cx| self.fill(plan, cx), ctl)?;
        if let Some(stop) = (ctl.stop)() {
            return Err(stop.into());
        }
        let (trees, tree_lines) = capture(|| self.trees(plan, &snap, ctl));
        let (effects, tree_reads) = trees?;
        let runs: BTreeMap<RunKey, Arc<RunOutput>> = needs
            .iter()
            .filter_map(|(k, _)| match (k, snap.get(k).map(|a| &a.value)) {
                (ArtifactKey::Run { key, .. }, Some(ArtifactValue::Run(r))) => {
                    Some((key.clone(), Arc::clone(r)))
                }
                _ => None,
            })
            .collect();
        let l4 = match snap.get(&ArtifactKey::L4).map(|a| &a.value) {
            Some(ArtifactValue::L4(s)) if plan.builds_l4() => Some(Arc::clone(&s.l4)),
            _ => None,
        };
        let det = match snap.get(&ArtifactKey::Det).map(|a| &a.value) {
            Some(ArtifactValue::Det(d)) if !plan.run_entries().is_empty() => Some(Arc::clone(d)),
            _ => None,
        };
        let view = View::served(
            Arc::clone(&self.target),
            Arc::clone(&self.rel),
            l4,
            det,
            effects,
            tree_reads,
            runs,
        );
        // A cold run prints `prepare`'s lines only when its plan solves l4,
        // in `prepare`'s order: ordering, rules, the composition (and any
        // scoped emission), the trees, the solve, the det, the runs.
        let mut lines = Vec::new();
        if plan.builds_l4() {
            let of = |k: &ArtifactKey| snap.get(k).map(|a| a.lines.clone()).unwrap_or_default();
            let wanted = |k: &ArtifactKey| needs.iter().any(|(n, _)| n == k);
            for k in [ArtifactKey::Ordering, ArtifactKey::Rules, ArtifactKey::Fold] {
                if wanted(&k) {
                    lines.extend(of(&k));
                }
            }
            for (k, _) in &needs {
                if matches!(k, ArtifactKey::Raw(_)) {
                    lines.extend(of(k));
                }
            }
            lines.extend(tree_lines);
            lines.extend(of(&ArtifactKey::L4));
            if wanted(&ArtifactKey::Det) {
                lines.extend(of(&ArtifactKey::Det));
            }
            for (k, _) in &needs {
                if matches!(k, ArtifactKey::Run { .. }) {
                    lines.extend(of(k));
                }
            }
        }
        Ok((view, lines))
    }

    fn answer_inner(&self, job: &dyn Job, ctl: &Control<'_>) -> Result<JobOutput, PrepareError> {
        let mut demands = job.demands();
        if demands.len() != 1 {
            return Err(PrepareError::Engine(format!(
                "'{}' reads {} views; a revision serves one",
                job.name(),
                demands.len()
            )));
        }
        let d = demands.pop().expect("one demand");
        // One pinned revision serves resolve, prepare and compute (T-pin):
        // the job was made over this revision's own read.
        if !Arc::ptr_eq(&d.inputs, &self.inputs) {
            return Err(PrepareError::Engine(
                "the job was made over another read than the pinned revision's".into(),
            ));
        }
        if (d.key.universe == crate::engine::batch::job::ViewUniverse::SingleApp) != self.single_app
            || d.key.no_roots_config
            || d.needs.analyze_extras
            || d.needs.graph
        {
            return Err(PrepareError::Engine(format!(
                "'{}' needs a view this revision did not build",
                job.name()
            )));
        }
        let model = self.target.resolved();
        let part = match job.resolve(d.side, &ResolveCx::new(model, &d.inputs)) {
            Ok(p) => p,
            Err(f) => return Ok(serialize_failure(job, f)),
        };
        let plan = Plan::merge([(JobId(0), part.plan)]);
        let (keys, has_tree) = (plan.keys(), plan.has_tree(JobId(0)));
        let (mut view, mut lines) = self.serve_plan(&plan, ctl)?;
        if let Some(stop) = (ctl.stop)() {
            return Err(stop.into());
        }
        let mut handed = Vec::new();
        let mut cx = ViewCx {
            view: &mut view,
            inputs: &d.inputs,
            fresh: None,
            job: JobId(0),
            keys,
            has_tree,
            later: LaterReads::EVERYTHING,
            carry: part.carry,
            handed: &mut handed,
        };
        let (side, own) = capture(|| job.compute(d.side, &mut cx));
        drop(view);
        lines.extend(own);
        let doc = job.finalize(vec![side]);
        Ok(crate::engine::batch::serialize(0, job, doc, lines))
    }
}

/// A job whose resolve failed, as the driver writes it.
fn serialize_failure(job: &dyn Job, f: JobFailure) -> JobOutput {
    let doc = job.finalize(vec![Err(f)]);
    crate::engine::batch::serialize(0, job, doc, vec![])
}

#[cfg(test)]
#[path = "revision_tests.rs"]
mod tests;
