//! A resident revision (context-server design §4.1-§4.4): one built target
//! over one acquired workspace read, immutable, and its artifact store. A
//! request is answered off it in the design's order: resolve (the job's plan
//! part against the revision's model), [`Store::ensure_prepared`] (the ready
//! snapshot), the request's trees off that snapshot, then the job's compute
//! over a served [`View`] holding `Arc` clones of the revision's cells. The
//! answer is the CLI's exact output for the same inputs.
//!
//! **Artifacts** ([`ArtifactKey`], by phase): 0: `Ordering`, `Rules`,
//! `ExplainIndexes` (from the model alone), `Fold` (the one composition:
//! every derived row, the pd arena and the retained cones; no raw facts),
//! `L4` (the solve and its core summaries, never drained); 1: `Raw(subjects)`
//! (the raw facts of one view's subjects, primary or dependency, emitted on
//! demand from the fold's retained cones: plan 2026-11-03 L6), `Det` (the
//! detector artifacts, harvested from the lent `Fold` and `L4`, at a mask),
//! `TreeBase`; 2: `Run(key)`, `Tree(subjects)`; 3: `Witness`.
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
use crate::engine::l4::summary_runner::{L4Control, L4Reuse, SolveBaseline};
use crate::engine::l5::detector_context::{
    self as dc, CapabilityAnalysis, CapsInput, CoreInput, DetectorArtifacts, L4Input,
    OrderingFactsMap,
};
use crate::engine::l5::digest::DigestEntryResult;
use crate::engine::l5::digest_cli::DigestEffects;
use crate::engine::l5::explain::ExplainIndexes;
use crate::engine::l5::registry::{RunOutput, substrate};
use crate::engine::l5::rules_index::RulesIndex;
use crate::engine::resident::store::{
    Control, FillCx, PrepareError, Snapshot, Store, StoreKey, StoreStats, StoreValue,
};
use crate::engine::session::{
    JobId, Plan, RunKey, TreeRead, TreeReadOut, View, WitnessRequest, compose_cone_retained, key,
    relation_of, run_once, tree_apps, tree_groups,
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
    /// The raw facts of these subjects (internal ids): one view's, emitted
    /// on demand from the fold's retained cones (plan 2026-11-03 L6; the
    /// fold emits none). A subject another published `Raw` holds, or the
    /// previous revision's when its rows were not re-folded, is adopted.
    Raw(BTreeSet<String>),
    /// The tree store's whole sections (design §6.11): derived once per
    /// revision over the fold, shared by every view.
    TreeBase,
    /// One view's tree over these subjects (internal ids): the whole
    /// sections of `TreeBase` with the view's own sections. Every request
    /// whose tree group has these subjects reads it.
    Tree(BTreeSet<String>),
    Det,
    /// One run, with whether its plan filled ordering and rules (a run's
    /// context reads them when they are filled, `run_once`).
    Run {
        key: RunKey,
        ordering: bool,
        rules: bool,
    },
    /// One root's witnesses (plan 2026-11-03, item W): its
    /// `DigestEntryResult` before any read's own passes
    /// (`digest_cli::root_entries`), a function of the root (stable id) and
    /// `include_reads` alone over any tree holding it, so explain, prove and
    /// digest on the same root share it within the revision. Filled off the
    /// request's own `Tree(scope)`; never carried to the next revision.
    Witness {
        root: String,
        include_reads: bool,
    },
    /// Explain's indexes (plan 2026-11-03 W2): the model indexes every card
    /// reads and the cross-call rules' index, a function of the model alone,
    /// built once per revision and read by every explain over it. Immutable:
    /// each request walks it with a walk of its own. Never carried to the
    /// next revision.
    ExplainIndexes,
}

impl StoreKey for ArtifactKey {
    fn kind(&self) -> &'static str {
        ArtifactKey::kind(self)
    }
    fn phase(&self) -> usize {
        match self {
            ArtifactKey::Ordering
            | ArtifactKey::Rules
            | ArtifactKey::Fold
            | ArtifactKey::L4
            | ArtifactKey::ExplainIndexes => 0,
            ArtifactKey::Raw(_) | ArtifactKey::Det | ArtifactKey::TreeBase => 1,
            ArtifactKey::Run { .. } | ArtifactKey::Tree(_) => 2,
            ArtifactKey::Witness { .. } => 3,
        }
    }
}

/// The one composition a revision keeps.
pub struct Fold {
    pub(crate) caps: CapabilityAnalysis,
    /// The per-routine inputs its producer solve read (design §6.10): the
    /// next revision's fold re-solves only the producers Δ reaches and
    /// copies every other producer's states from `caps.pd`.
    pub(crate) pd_digests: std::collections::HashMap<String, [u8; 32]>,
    /// What its producer solve reused.
    pub pd_report: crate::engine::l4::pd_solve::PdSolveReport,
    /// Its per-SCC cone store (design §6.10): the next revision's fold
    /// re-folds only the SCCs whose inputs changed and copies the rest; and
    /// what a `Raw` fill emits a subject's raw facts from (plan 2026-11-03
    /// L6).
    pub(crate) cones: Option<crate::engine::l4::capability_cone::ConeStore>,
    /// What its walk re-folded and copied.
    pub cone_report: crate::engine::l4::capability_cone::ConeReport,
}

impl Fold {
    /// The fold's parts, for a byte census (`tools/census-probe resident`).
    #[doc(hidden)]
    pub fn into_census_parts(self) -> Vec<(&'static str, Box<dyn std::any::Any>)> {
        let CapabilityAnalysis {
            direct_full,
            coverage,
            derived,
            pd,
        } = self.caps;
        vec![
            ("fold.direct_full", Box::new(direct_full)),
            ("fold.coverage", Box::new(coverage)),
            ("fold.derived", Box::new(derived)),
            ("fold.pd", Box::new(pd)),
            ("fold.pd_digests", Box::new(self.pd_digests)),
            ("fold.cones", Box::new(self.cones)),
        ]
    }
}

/// The retained solve: never drained, so a det refill and the next seeded
/// solve read it (`CoreInput::Borrowed`, `L4Reuse::core`).
pub struct Solved {
    pub l4: Arc<L4Analysis>,
    pub core: Arc<CoreSummaries>,
    pub baseline: SolveBaseline,
}

/// The L4 seed a revision's solve may start from (design §6.9, stage 5):
/// the complete triple of an earlier revision's solve — its
/// [`SolveBaseline`], its bundle (inside [`L4Analysis`]) and its core
/// summaries — held as that revision's published `L4` artifact, and the
/// [`seed_key`] of the revision that solved it. The seeded solve compares
/// the baseline's digests with the current inputs' (the L4 host's R0-R3),
/// so a seed from ANY earlier revision with an equal key gives the cold
/// answer; a different key (a package, `app.json`, universe or engine
/// change) is a full solve.
#[derive(Clone)]
pub struct L4Seed {
    pub key: [u8; 32],
    /// An `Artifact` whose value is `ArtifactValue::L4`.
    pub artifact: Arc<Artifact>,
}

/// The half of the L4 host's R0 that the solve's own digests do not cover
/// (`incremental.rs`'s `host_global`): the universe, the packages the read
/// consumed (path and content hash, in closure order), the root `app.json`
/// bytes, the model-instance id, the source dependencies and the engine
/// version. Read from the revision's own acquired `inputs`, so the key and
/// the build cannot disagree.
#[must_use]
pub fn seed_key(inputs: &WorkspaceInputs, single_app: bool) -> [u8; 32] {
    let source_deps = inputs
        .source_dependencies
        .as_ref()
        .map(|d| crate::program::source_deps::digest(d));
    crate::engine::l4::summary_runner::sha256_of_debug(&(
        single_app,
        inputs.closure.consumed(),
        inputs.app_json.bytes.as_ref().ok(),
        &inputs.model_instance_id,
        &source_deps,
        &inputs.driver_version,
    ))
}

/// An artifact's value.
pub enum ArtifactValue {
    Ordering(OrderingFactsMap),
    Rules(RulesIndex),
    Fold(Box<Fold>),
    L4(Solved),
    Raw(InheritedFacts),
    TreeBase(Arc<crate::engine::l5::snapshot::TreeBase>),
    Tree(Arc<crate::engine::l5::snapshot::CapabilitySnapshot>),
    Det(Arc<DetectorArtifacts>),
    Run(Arc<RunOutput>),
    /// `None`: the root yields no entry (not reportable), stored so it is
    /// not asked again. A read finishes CLONES of it (prove's literal reach
    /// mutates its entries), never the stored value.
    Witness(Arc<Option<DigestEntryResult>>),
    ExplainIndexes(Arc<ExplainIndexes>),
}

/// A request's batch of witness entries (plan 2026-11-03, W item 3): the
/// store fills claimed keys one at a time, so the request's first `Witness`
/// fill computes every absent root of the request in one pass over its tree
/// (one index build, the roots in parallel) and leaves them here; the
/// request's later `Witness` fills return from it.
type WitnessScratch = std::sync::Mutex<BTreeMap<ArtifactKey, Arc<Option<DigestEntryResult>>>>;

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
    store: Store<ArtifactKey, Artifact>,
    answered: std::sync::atomic::AtomicU64,
    /// Fills per artifact kind (`status`, the T-order fill counters).
    fills: std::sync::Mutex<BTreeMap<&'static str, u64>>,
    /// [`seed_key`] of this revision's read.
    seed_key: [u8; 32],
    /// The seed this revision's `L4` fill starts from, until that fill
    /// publishes (then released: the revision's own solve is the next
    /// revision's seed).
    seed: std::sync::Mutex<Option<L4Seed>>,
    /// The last `L4` fill's report: `(seeded SCCs, total SCCs, full)`.
    l4_report: std::sync::Mutex<Option<(usize, usize, bool)>>,
    /// The previous revision's `Fold` artifact (an `ArtifactValue::Fold`),
    /// its producer states the seed of this revision's fold (§6.10), until
    /// that fold publishes. Same key rule as the L4 seed.
    fold_seed: std::sync::Mutex<Option<Arc<Artifact>>>,
    /// The raw facts emitted over that seed fold (its revision's `Raw`
    /// artifacts, plan 2026-11-03 L6): a `Raw` fill adopts a subject's
    /// entry from them when this revision's fold did not re-fold the
    /// subject's rows against the seed (`ConeStore::changed`), and emits it
    /// otherwise. Kept for the revision's life (a fold refilled without its
    /// seed re-folds everything, so nothing is adopted then); taken with the
    /// fold seed, so the two always belong together.
    raw_seed: Vec<Arc<Artifact>>,
    /// The `Raw` fills' subjects ([`Revision::raw_counts`]).
    raw_counts: RawCounts,
    /// The witness memo's counters ([`Revision::witness_counts`]).
    witness: WitnessCounts,
}

/// The subjects this revision's `Raw` fills emitted from the cones and the
/// ones they adopted (plan 2026-11-03 L6).
#[derive(Debug, Default)]
struct RawCounts {
    emitted: std::sync::atomic::AtomicU64,
    adopted: std::sync::atomic::AtomicU64,
}

/// The witness memo's counters (`status`): roots a request found published
/// (`hit`), roots a request computed (`miss`), and the passes that computed
/// them (`batches`: one tree index build per pass and rule-B mode).
#[derive(Debug, Default)]
struct WitnessCounts {
    hit: std::sync::atomic::AtomicU64,
    miss: std::sync::atomic::AtomicU64,
    batches: std::sync::atomic::AtomicU64,
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
            ArtifactKey::TreeBase => "tree_base",
            ArtifactKey::Tree(_) => "tree",
            ArtifactKey::Det => "det",
            ArtifactKey::Run { .. } => "run",
            ArtifactKey::Witness { .. } => "witness",
            ArtifactKey::ExplainIndexes => "explain_indexes",
        }
    }
}

impl Revision {
    /// A revision over `target`, built from `inputs`.
    #[must_use]
    pub fn new(id: RevisionId, inputs: Arc<WorkspaceInputs>, target: AnalysisTarget) -> Revision {
        Revision::seeded(id, inputs, target, None)
    }

    /// A revision whose `L4` fill starts from `prev`'s seed (design §6.9):
    /// `prev`'s published solve, else the seed `prev` itself still holds,
    /// when its [`seed_key`] equals this revision's; else a full solve.
    #[must_use]
    pub fn seeded(
        id: RevisionId,
        inputs: Arc<WorkspaceInputs>,
        target: AnalysisTarget,
        prev: Option<&Revision>,
    ) -> Revision {
        let rel = relation_of(&target);
        let dependency_calls = match &target {
            AnalysisTarget::CrossApp(b) => b.dependency_calls.clone(),
            AnalysisTarget::SingleApp(_) => None,
        };
        let single_app = matches!(target, AnalysisTarget::SingleApp(_));
        let seed_key = seed_key(&inputs, single_app);
        let seed = prev
            .and_then(Revision::l4_seed)
            .filter(|s| s.key == seed_key);
        let (fold_seed, raw_seed) = prev
            .filter(|p| p.seed_key == seed_key)
            .and_then(Revision::fold_seed_with_raws)
            .map_or((None, Vec::new()), |(f, r)| (Some(f), r));
        Revision {
            id,
            inputs,
            single_app,
            target: Arc::new(target),
            rel,
            dependency_calls,
            store: Store::new(),
            answered: std::sync::atomic::AtomicU64::new(0),
            fills: std::sync::Mutex::new(BTreeMap::new()),
            seed_key,
            seed: std::sync::Mutex::new(seed),
            l4_report: std::sync::Mutex::new(None),
            fold_seed: std::sync::Mutex::new(fold_seed),
            raw_seed,
            raw_counts: RawCounts::default(),
            witness: WitnessCounts::default(),
        }
    }

    /// The fold a NEXT revision's fold may start from: this revision's
    /// published `Fold`, else the seed it was given and has not used yet.
    #[must_use]
    pub fn fold_seed(&self) -> Option<Arc<Artifact>> {
        self.fold_seed_with_raws().map(|(f, _)| f)
    }

    /// [`Revision::fold_seed`] with the raw facts emitted over that fold
    /// (plan 2026-11-03 L6), read off one snapshot: this revision's
    /// published `Fold` and `Raw` artifacts, else the seed it was given and
    /// the raw seed that came with it.
    #[must_use]
    pub fn fold_seed_with_raws(&self) -> Option<(Arc<Artifact>, Vec<Arc<Artifact>>)> {
        let snap = self.store.load();
        match snap.get(&ArtifactKey::Fold) {
            Some(a) => {
                let raws = snap
                    .keys()
                    .filter(|k| matches!(k, ArtifactKey::Raw(_)))
                    .filter_map(|k| snap.get(k).cloned())
                    .collect();
                Some((Arc::clone(a), raws))
            }
            None => self
                .fold_seed
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .map(|f| (f, self.raw_seed.clone())),
        }
    }

    /// The published fold's cone-walk report, and its cone store's size
    /// `(SCCs, SCCs with a fact cone, fact-cone entries)`.
    #[must_use]
    pub fn cone_report(
        &self,
    ) -> Option<(
        crate::engine::l4::capability_cone::ConeReport,
        (usize, usize, usize),
    )> {
        match self.store.load().get(&ArtifactKey::Fold).map(|a| &a.value) {
            Some(ArtifactValue::Fold(f)) => Some((
                f.cone_report,
                f.cones.as_ref().map_or((0, 0, 0), |c| {
                    let (n, with) = c.len();
                    (n, with, c.cone_entries())
                }),
            )),
            _ => None,
        }
    }

    /// The published fold's producer-solve report.
    #[must_use]
    pub fn pd_report(&self) -> Option<crate::engine::l4::pd_solve::PdSolveReport> {
        match self.store.load().get(&ArtifactKey::Fold).map(|a| &a.value) {
            Some(ArtifactValue::Fold(f)) => Some(f.pd_report),
            _ => None,
        }
    }

    /// The seed a NEXT revision's solve may start from: this revision's
    /// published `L4`, else the seed it was given and has not used yet.
    #[must_use]
    pub fn l4_seed(&self) -> Option<L4Seed> {
        match self.store.load().get(&ArtifactKey::L4) {
            Some(a) => Some(L4Seed {
                key: self.seed_key,
                artifact: Arc::clone(a),
            }),
            None => self.seed.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        }
    }

    /// Whether this revision holds a seed its `L4` fill has not used yet.
    #[must_use]
    pub fn has_pending_seed(&self) -> bool {
        self.seed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// The last `L4` fill's `(seeded SCCs, total SCCs, full)`.
    #[must_use]
    pub fn l4_report(&self) -> Option<(usize, usize, bool)> {
        *self.l4_report.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The revision's target.
    #[must_use]
    pub fn target(&self) -> &AnalysisTarget {
        &self.target
    }

    /// The revision's parts in the order a byte census drops them
    /// (`tools/census-probe resident`): the artifact store, the relation,
    /// the dependency calls, the read, then the target (last, so the
    /// relation's and the calls' drop deltas are what only they held).
    #[doc(hidden)]
    pub fn into_census_parts(self) -> Vec<(&'static str, Box<dyn std::any::Any>)> {
        vec![
            ("revision.store", Box::new(self.store)),
            ("revision.rel", Box::new(self.rel)),
            ("revision.dependency_calls", Box::new(self.dependency_calls)),
            ("revision.inputs", Box::new(self.inputs)),
            ("revision.target", Box::new(self.target)),
        ]
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

    /// The artifact kinds eviction may drop, in the design's order (§7):
    /// the roots' witnesses (small, plan 2026-11-03 W), the views' raw
    /// facts (re-emitted from the fold's cones, plan L6), then the views'
    /// trees, runs and the det, then explain's
    /// indexes (plan W2: ~1 s to rebuild from the model, cheaper than any
    /// kind after it), then the tree store's whole sections and the fold
    /// (the cones), then ordering and rules. Never the model, the relation
    /// or the L4 seed.
    pub const EVICTION_ORDER: &'static [&'static str] = &[
        "witness",
        "raw",
        "tree",
        "run",
        "det",
        "explain_indexes",
        "tree_base",
        "fold",
        "ordering",
        "rules",
    ];

    /// The measured bytes of every evictable artifact the current snapshot
    /// holds (what their fills left resident).
    #[must_use]
    pub fn evictable_bytes(&self) -> u64 {
        let s = self.store.load();
        s.keys()
            .filter(|k| Self::EVICTION_ORDER.contains(&k.kind()))
            .map(|k| s.bytes(k))
            .sum()
    }

    /// Evict every unpinned artifact of `kind`; returns how many.
    pub fn evict_kind(&self, kind: &str) -> usize {
        let keys = self.keys_where(|k| k.kind() == kind);
        if keys.is_empty() {
            return 0;
        }
        self.store.evict(&keys)
    }

    /// Evict, for memory, every artifact of `kind` no request holds
    /// ([`Store::evict_unheld`]); returns how many.
    pub fn reclaim_kind(&self, kind: &str) -> usize {
        let keys = self.keys_where(|k| k.kind() == kind);
        if keys.is_empty() {
            return 0;
        }
        self.store.evict_unheld(&keys)
    }

    /// How many routine rows the revision's model holds (the estimate
    /// table's scale).
    #[must_use]
    pub fn routine_count(&self) -> u64 {
        self.target.resolved().workspace.routines.len() as u64
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
        if plan.reads_explain_indexes() {
            needs.push((ArtifactKey::ExplainIndexes, 0));
        }
        // The tree store (§6.11): the whole sections once per revision, one
        // tree per distinct view (its subjects), each over the fold and its
        // subjects' raw artifact (plan 2026-11-03 L6: the fold emits no raw
        // facts; a view's are emitted on demand, primary or dependency).
        let scopes = self.tree_scopes(plan);
        if !scopes.is_empty() {
            needs.push((ArtifactKey::TreeBase, 0));
        }
        for scope in scopes {
            let raw = ArtifactKey::Raw(scope.clone());
            if !needs.iter().any(|(k, _)| *k == raw) {
                needs.push((raw, 0));
            }
            let tree = ArtifactKey::Tree(scope);
            if !needs.iter().any(|(k, _)| *k == tree) {
                needs.push((tree, 0));
            }
        }
        // One witness per root of every effects read (plan 2026-11-03, W),
        // filled off that read's tree (an earlier phase).
        for (_, include_reads, roots) in self.witness_groups(plan) {
            for root in roots {
                let k = ArtifactKey::Witness {
                    root,
                    include_reads,
                };
                if !needs.iter().any(|(n, _)| *n == k) {
                    needs.push((k, 0));
                }
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

    /// Each effects read's tree group: its tree's subjects (the group's
    /// [`Revision::tree_scopes`] entry), its `include_reads`, and the
    /// distinct roots (stable ids) of its readers, sorted.
    fn witness_groups(&self, plan: &Plan) -> Vec<(BTreeSet<String>, bool, Vec<String>)> {
        let trees = plan.tree_demands();
        tree_groups(trees)
            .iter()
            .zip(self.tree_scopes(plan))
            .filter(|(g, _)| matches!(trees[g[0]].1.read, TreeRead::Effects))
            .map(|(g, scope)| {
                let roots: BTreeSet<String> = g
                    .iter()
                    .flat_map(|&m| trees[m].1.subjects.ids().iter().cloned())
                    .collect();
                // A group shares `include_reads` (`tree_groups`).
                (
                    scope,
                    trees[g[0]].1.include_reads,
                    roots.into_iter().collect(),
                )
            })
            .collect()
    }

    /// A `Witness` fill (plan 2026-11-03, W item 3): the key's entry from the
    /// request's `scratch`, or, on the request's first witness fill, every
    /// witness of the request absent from `snapshot` computed in one pass
    /// per tree group over that group's published tree, left in `scratch`.
    fn fill_witness(
        &self,
        plan: &Plan,
        key: &ArtifactKey,
        snapshot: &Snapshot<ArtifactKey, Artifact>,
        scratch: &WitnessScratch,
    ) -> Result<Arc<Option<DigestEntryResult>>, String> {
        use crate::engine::l5::digest_cli::{BoundaryMode, root_entries};
        // The key may already be in `snapshot`: a request that read the
        // snapshot before a concurrent request published the key claims it
        // once that fill is over (the store's step (a)), then fills off the
        // newer snapshot (step (c)). The batch below skips every key the
        // snapshot holds, this one too, so return the published entry: it is
        // this revision's value for the key.
        if let Some(ArtifactValue::Witness(e)) = snapshot.get(key).map(|a| &a.value) {
            return Ok(Arc::clone(e));
        }
        let mut held = scratch.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = held.get(key) {
            return Ok(Arc::clone(e));
        }
        let model = self.target.resolved();
        for (scope, include_reads, roots) in self.witness_groups(plan) {
            let absent: Vec<String> = roots
                .into_iter()
                .filter(|root| {
                    let k = ArtifactKey::Witness {
                        root: root.clone(),
                        include_reads,
                    };
                    snapshot.get(&k).is_none() && !held.contains_key(&k)
                })
                .collect();
            if absent.is_empty() {
                continue;
            }
            let Some(ArtifactValue::Tree(tree)) =
                snapshot.get(&ArtifactKey::Tree(scope)).map(|a| &a.value)
            else {
                return Err("a witness needs its tree, which is not published".into());
            };
            self.witness
                .batches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let entries = root_entries(tree, model, &absent, include_reads, BoundaryMode::On);
            for (root, entry) in absent.into_iter().zip(entries) {
                held.insert(
                    ArtifactKey::Witness {
                        root,
                        include_reads,
                    },
                    Arc::new(entry),
                );
            }
        }
        held.get(key)
            .cloned()
            .ok_or_else(|| format!("{key:?} is not a witness of the request's plan"))
    }

    /// One artifact's fill, off the snapshot holding its prerequisites; a
    /// run's detectors come from the request's `plan`; a witness's batch is
    /// the request's `scratch`.
    fn fill(
        &self,
        plan: &Plan,
        scratch: &WitnessScratch,
        cx: FillCx<'_, ArtifactKey, Artifact>,
    ) -> Result<Artifact, String> {
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
                ArtifactKey::Witness { .. } => {
                    ArtifactValue::Witness(self.fill_witness(plan, cx.key, cx.snapshot, scratch)?)
                }
                ArtifactKey::ExplainIndexes => {
                    let _s = crate::engine::perf_trace::span("revision", "explain.indexes");
                    ArtifactValue::ExplainIndexes(Arc::new(ExplainIndexes::build(model)))
                }
                ArtifactKey::Ordering => ArtifactValue::Ordering(dc::fill_ordering(model)),
                ArtifactKey::Rules => ArtifactValue::Rules(dc::fill_rules(model)),
                ArtifactKey::Fold => {
                    let _s = crate::engine::perf_trace::span("revision", "revision.fold");
                    // No raw facts (plan 2026-11-03 L6): a view's are
                    // emitted on demand from the retained cones (`Raw`).
                    let scope = ConeScope {
                        derived: true,
                        raw: crate::engine::l4::cone_derived::WitnessScope::None,
                    };
                    // The previous revision's fold (§6.10): its producer
                    // states are copied where Δ leaves them untouched.
                    let seed = self
                        .fold_seed
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    use crate::engine::l4::capability_cone::{ConeSeed, FoldSeed, PdRetain};
                    let fold_seed = seed.as_ref().and_then(|a| match &a.value {
                        ArtifactValue::Fold(f) => f.caps.pd.as_ref().map(|pd| FoldSeed {
                            arena: pd,
                            pd_digests: &f.pd_digests,
                            // The previous fold's cones and rows (§6.10).
                            cones: f.cones.as_ref().map(|store| ConeSeed {
                                store,
                                derived: &f.caps.derived,
                                coverage: &f.caps.coverage,
                            }),
                        }),
                        _ => None,
                    });
                    let (caps, _no_raw, kept) = compose_cone_retained(
                        model,
                        &self.rel,
                        scope,
                        PdRetain::Yes {
                            seed: fold_seed,
                            cones: true,
                        },
                    );
                    crate::engine::perf_trace::counter_delta(
                        "revision.pd_producers_reused",
                        kept.report.reused as u64,
                    );
                    crate::engine::perf_trace::counter_delta(
                        "revision.sccs_refolded",
                        kept.cones.refolded as u64,
                    );
                    drop(seed);
                    ArtifactValue::Fold(Box::new(Fold {
                        caps,
                        pd_digests: kept.digests,
                        pd_report: kept.report,
                        cones: kept.store,
                        cone_report: kept.cones,
                    }))
                }
                ArtifactKey::L4 => {
                    // A shared fill is never cancelled by a request (§4.6).
                    let never = || false;
                    // The seed (§6.9): the complete triple of an earlier
                    // revision's solve, compared digest by digest.
                    let seed = self.seed.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    let reuse = seed.as_ref().and_then(|s| match &s.artifact.value {
                        ArtifactValue::L4(prev) => Some(L4Reuse {
                            solve: &prev.baseline,
                            bundle: &prev.l4.bundle,
                            core: &prev.core.0,
                        }),
                        _ => None,
                    });
                    let req = L4Request {
                        extra_diagnostics: vec![],
                        control: Some(L4Control {
                            cancel: &never,
                            reuse,
                        }),
                    };
                    let cross = target.cross_app_inputs();
                    let (l4, core, outcome) =
                        build_l4_controlled(model, &self.rel, req, cross.as_ref())
                            .map_err(|_| "the solve was cancelled".to_string())?;
                    let outcome = outcome.ok_or("a controlled solve reports its outcome")?;
                    *self.l4_report.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some((outcome.seeded, outcome.total, outcome.full));
                    crate::engine::perf_trace::counter_delta(
                        "revision.l4_sccs_seeded",
                        outcome.seeded as u64,
                    );
                    drop(seed);
                    ArtifactValue::L4(Solved {
                        l4: Arc::new(l4),
                        core: Arc::new(core),
                        baseline: outcome.baseline,
                    })
                }
                ArtifactKey::Raw(subjects) => {
                    // Plan 2026-11-03 L6: emitted from the fold's retained
                    // cones. A subject another published `Raw` holds is
                    // adopted (it is this fold's entry); so is one the
                    // previous revision emitted, when this fold did not
                    // re-fold its rows against that revision's fold.
                    let Some(ArtifactValue::Fold(fold)) =
                        cx.snapshot.get(&ArtifactKey::Fold).map(|a| &a.value)
                    else {
                        return Err("raw facts need the fold, which is not published".into());
                    };
                    let store = fold.cones.as_ref().ok_or("the fold kept no cone store")?;
                    fn raws_of<'a>(
                        arts: impl Iterator<Item = &'a Arc<Artifact>>,
                    ) -> Vec<&'a InheritedFacts> {
                        arts.filter_map(|a| match &a.value {
                            ArtifactValue::Raw(r) => Some(r),
                            _ => None,
                        })
                        .collect()
                    }
                    let same = raws_of(
                        cx.snapshot
                            .keys()
                            .filter(|k| matches!(k, ArtifactKey::Raw(_)))
                            .filter_map(|k| cx.snapshot.get(k)),
                    );
                    let prev = raws_of(self.raw_seed.iter());
                    let reuse = |id: &str| {
                        same.iter()
                            .find(|r| r.contains_key(id))
                            .or_else(|| {
                                (!store.changed(id))
                                    .then(|| prev.iter().find(|r| r.contains_key(id)))
                                    .flatten()
                            })
                            .copied()
                    };
                    let (raw, report) = store.emit_raw(
                        &model.workspace.routines,
                        &fold.caps.direct_full,
                        fold.caps.pd.as_ref(),
                        subjects,
                        &reuse,
                    );
                    use std::sync::atomic::Ordering::Relaxed;
                    self.raw_counts.emitted.fetch_add(report.emitted, Relaxed);
                    self.raw_counts.adopted.fetch_add(report.adopted, Relaxed);
                    ArtifactValue::Raw(raw)
                }
                ArtifactKey::TreeBase => {
                    let Some(ArtifactValue::Fold(fold)) =
                        cx.snapshot.get(&ArtifactKey::Fold).map(|a| &a.value)
                    else {
                        return Err("the tree base needs the fold, which is not published".into());
                    };
                    let pd = fold.caps.pd.as_ref().ok_or("the fold kept no pd arena")?;
                    ArtifactValue::TreeBase(Arc::new(
                        crate::engine::l5::snapshot::derive_tree_base(
                            model,
                            self.rel.cone(),
                            self.dependency_calls(),
                            &fold.caps,
                            pd,
                            &tree_apps(self.cross_base()),
                        ),
                    ))
                }
                ArtifactKey::Tree(scope) => {
                    use crate::engine::l5::snapshot::{SubjectRaw, TreeRaw, compose_tree_over};
                    let Some(ArtifactValue::Fold(fold)) =
                        cx.snapshot.get(&ArtifactKey::Fold).map(|a| &a.value)
                    else {
                        return Err("a tree needs the fold, which is not published".into());
                    };
                    let Some(ArtifactValue::TreeBase(base)) =
                        cx.snapshot.get(&ArtifactKey::TreeBase).map(|a| &a.value)
                    else {
                        return Err("a tree needs the tree base, which is not published".into());
                    };
                    let pd = fold.caps.pd.as_ref().ok_or("the fold kept no pd arena")?;
                    let map = match cx
                        .snapshot
                        .get(&ArtifactKey::Raw(scope.clone()))
                        .map(|a| &a.value)
                    {
                        Some(ArtifactValue::Raw(r)) => r,
                        _ => return Err("a tree's raw facts are missing".into()),
                    };
                    ArtifactValue::Tree(Arc::new(compose_tree_over(
                        std::borrow::Cow::Borrowed(&**base),
                        model,
                        self.rel.cone(),
                        self.dependency_calls(),
                        &fold.caps,
                        TreeRaw::Borrow(SubjectRaw::new(map, scope)),
                        std::borrow::Cow::Borrowed(pd),
                        &tree_apps(self.cross_base()),
                    )))
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

    /// The calls dependency routines make (none single-app).
    fn dependency_calls(&self) -> &[CallEdge] {
        self.dependency_calls
            .as_deref()
            .map_or(&[][..], Vec::as_slice)
    }

    /// The plan's tree reads, each off its group's published tree (design
    /// §6.11: one tree per view, filled once per revision, shared by every
    /// request reading it), with the job lines a cold run prints for the
    /// trees: per group, the tree's composition (its base sections' lines,
    /// then its own), then the group's read. Stops between groups.
    fn trees(
        &self,
        plan: &Plan,
        snap: &ReadySnapshot,
        ctl: &Control<'_>,
    ) -> Result<(TreeReads, Vec<String>), PrepareError> {
        use crate::engine::session::read_tree_shared;
        let (mut effects, mut tree_reads) = (BTreeMap::new(), BTreeMap::new());
        let mut lines = Vec::new();
        let trees = plan.tree_demands();
        if trees.is_empty() {
            return Ok(((effects, tree_reads), lines));
        }
        let target = &*self.target;
        let base_lines = snap
            .get(&ArtifactKey::TreeBase)
            .map(|a| a.lines.clone())
            .unwrap_or_default();
        for (group, scope) in tree_groups(trees).iter().zip(self.tree_scopes(plan)) {
            if let Some(stop) = (ctl.stop)() {
                return Err(stop.into());
            }
            // An effects read finishes its roots' memoized witnesses: owned
            // CLONES, which its passes may mutate (prove's literal reach),
            // never the stored values.
            let entries = match trees[group[0]].1.read {
                TreeRead::Effects => {
                    let include_reads = trees[group[0]].1.include_reads;
                    let roots: BTreeSet<&String> = group
                        .iter()
                        .flat_map(|&m| trees[m].1.subjects.ids().iter())
                        .collect();
                    let mut entries = Vec::new();
                    for root in roots {
                        let k = ArtifactKey::Witness {
                            root: root.clone(),
                            include_reads,
                        };
                        match snap.get(&k).map(|a| &a.value) {
                            Some(ArtifactValue::Witness(e)) => {
                                entries.extend(Option::clone(e));
                            }
                            _ => {
                                return Err(PrepareError::Engine(format!(
                                    "{k:?} is not published"
                                )));
                            }
                        }
                    }
                    Some(entries)
                }
                _ => None,
            };
            let Some(artifact) = snap.get(&ArtifactKey::Tree(scope)) else {
                return Err(PrepareError::Engine("a tree is not published".into()));
            };
            let ArtifactValue::Tree(tree) = &artifact.value else {
                return Err(PrepareError::Engine(
                    "a tree key holds another value".into(),
                ));
            };
            lines.extend(base_lines.iter().cloned());
            lines.extend(artifact.lines.iter().cloned());
            let ((), read_lines) = capture(|| {
                read_tree_shared(
                    tree,
                    trees,
                    group,
                    target,
                    &mut effects,
                    &mut tree_reads,
                    entries,
                );
            });
            lines.extend(read_lines);
        }
        Ok(((effects, tree_reads), lines))
    }

    /// `job` (one of [`SERVED_COMMANDS`], made over [`Revision::inputs`])
    /// answered off this revision: the exact output a standalone run of it
    /// writes. `Err` only when the request stopped or a fill failed.
    pub fn answer(&self, job: &dyn Job, ctl: &Control<'_>) -> Result<JobOutput, PrepareError> {
        self.answer_timed(job, ctl).map(|(out, _)| out)
    }

    /// [`Revision::answer`] with where its time went: `(shared fill ms,
    /// compute ms)` — `ensure_prepared`, then everything after it.
    pub fn answer_timed(
        &self,
        job: &dyn Job,
        ctl: &Control<'_>,
    ) -> Result<(JobOutput, (u64, u64)), PrepareError> {
        let start = std::time::Instant::now();
        let mut prepare_ms = 0;
        let r = self.answer_inner(job, ctl, &mut prepare_ms);
        if r.is_ok() {
            self.answered
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let total = start.elapsed().as_millis() as u64;
        r.map(|out| (out, (prepare_ms, total.saturating_sub(prepare_ms))))
    }

    /// Fill `keys` (prerequisites first, each of phase 0: `Fold`, `L4`,
    /// `Ordering`, `Rules`; or `TreeBase`, after the fold) through
    /// [`Store::ensure_prepared`], as a request
    /// would: the measurement of "analysis prepared" (design §12.1) and the
    /// warm-up a front end may run after a refresh. Returns the wall ms.
    pub fn prepare_keys(
        &self,
        keys: &[ArtifactKey],
        ctl: &Control<'_>,
    ) -> Result<u64, PrepareError> {
        // The shared artifacts no request parameter shapes: phase 0, and the
        // tree store's whole sections (which read only the fold).
        if let Some(k) = keys
            .iter()
            .find(|k| StoreKey::phase(*k) != 0 && **k != ArtifactKey::TreeBase)
        {
            return Err(PrepareError::Engine(format!(
                "{k:?} is not a shared request-independent artifact"
            )));
        }
        let plan = Plan::merge([]);
        let needs: Vec<(ArtifactKey, u32)> = keys.iter().map(|k| (k.clone(), 0)).collect();
        let start = std::time::Instant::now();
        let scratch = WitnessScratch::default();
        let snap = self.store.ensure_prepared(
            &needs,
            &|_| 0,
            &|cx| self.fill(&plan, &scratch, cx),
            ctl,
        )?;
        self.release_used_seed(&snap);
        Ok(start.elapsed().as_millis() as u64)
    }

    /// Once this revision's own solve is published, the seed it started
    /// from is released (the solve is the next revision's seed).
    fn release_used_seed(&self, snap: &ReadySnapshot) {
        // Dropping a seed may free the last copy of the previous revision's
        // fold (its arena, cones, raw): timed apart.
        let _s = crate::engine::perf_trace::span("revision", "revision.seed_release");
        if snap.get(&ArtifactKey::L4).is_some() {
            self.seed.lock().unwrap_or_else(|p| p.into_inner()).take();
        }
        if snap.get(&ArtifactKey::Fold).is_some() {
            self.fold_seed
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
        }
    }

    /// How many requests this revision answered.
    #[must_use]
    pub fn answered(&self) -> u64 {
        self.answered.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The witness memo's counters `(hit, miss, batches)`: roots requests
    /// found published, roots they computed, and the passes that computed
    /// them (each one tree index build per rule-B mode).
    #[must_use]
    pub fn witness_counts(&self) -> (u64, u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            self.witness.hit.load(Relaxed),
            self.witness.miss.load(Relaxed),
            self.witness.batches.load(Relaxed),
        )
    }

    /// The `Raw` fills' subjects `(emitted, adopted)` (plan 2026-11-03 L6):
    /// emitted from the fold's cones, or adopted from another view's
    /// emission or the previous revision's.
    #[must_use]
    pub fn raw_counts(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            self.raw_counts.emitted.load(Relaxed),
            self.raw_counts.adopted.load(Relaxed),
        )
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
        prepare_ms: &mut u64,
    ) -> Result<(View, Vec<String>), PrepareError> {
        let needs = self.needs_of(plan);
        let start = std::time::Instant::now();
        // The request's witness batch: dropped with the request (the store
        // holds what it published).
        let scratch = WitnessScratch::default();
        let snap =
            self.store
                .ensure_prepared(&needs, &|_| 0, &|cx| self.fill(plan, &scratch, cx), ctl);
        *prepare_ms = start.elapsed().as_millis() as u64;
        let snap = snap?;
        self.release_used_seed(&snap);
        {
            // A needed witness this request computed is a miss; one it found
            // published (by an earlier or a concurrent request) is a hit.
            let computed = scratch.into_inner().unwrap_or_else(|p| p.into_inner());
            let (mut hit, mut miss) = (0, 0);
            for (k, _) in &needs {
                if matches!(k, ArtifactKey::Witness { .. }) {
                    if computed.contains_key(k) {
                        miss += 1;
                    } else {
                        hit += 1;
                    }
                }
            }
            use std::sync::atomic::Ordering::Relaxed;
            self.witness.hit.fetch_add(hit, Relaxed);
            self.witness.miss.fetch_add(miss, Relaxed);
            crate::engine::perf_trace::counter_delta("witness.hit", hit);
            crate::engine::perf_trace::counter_delta("witness.miss", miss);
        }
        if let Some(stop) = (ctl.stop)() {
            return Err(stop.into());
        }
        // The trees' working memory is reserved like a fill's (§4.5 "heavy
        // request"), and released once they are read.
        let guard = if plan.tree_demands().is_empty() {
            None
        } else {
            Some((ctl.reserve)("trees", 0).map_err(PrepareError::Budget)?)
        };
        let trees = self.trees(plan, &snap, ctl);
        if let Some(g) = guard {
            g.finish();
        }
        let ((effects, tree_reads), tree_lines) = trees?;
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
        let explain_indexes = match snap.get(&ArtifactKey::ExplainIndexes).map(|a| &a.value) {
            Some(ArtifactValue::ExplainIndexes(x)) if plan.reads_explain_indexes() => {
                Some(Arc::clone(x))
            }
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
            explain_indexes,
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

    fn answer_inner(
        &self,
        job: &dyn Job,
        ctl: &Control<'_>,
        prepare_ms: &mut u64,
    ) -> Result<JobOutput, PrepareError> {
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
        // The request's artifacts stay pinned through its trees and compute
        // (§4.2 step 4): a reclaim for its own trees, or another request's,
        // never evicts what it reads.
        let needed: Vec<ArtifactKey> = self.needs_of(&plan).into_iter().map(|(k, _)| k).collect();
        let _hold = self.store.hold(&needed);
        let (keys, has_tree) = (plan.keys(), plan.has_tree(JobId(0)));
        let (mut view, mut lines) = self.serve_plan(&plan, ctl, prepare_ms)?;
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

/// `job` (one view, one side) answered over a build that produced no model:
/// its own `view_failed` text and exit, as the batch driver writes it for a
/// standalone run (design §4.3 "Failed build").
pub fn answer_failed_build(
    job: &dyn Job,
    failure: &crate::engine::batch::job::ViewFailure,
) -> JobOutput {
    let d = job
        .demands()
        .into_iter()
        .next()
        .expect("a served job reads one view");
    let (side, lines) = capture(|| job.view_failed(d.side, failure));
    let doc = job.finalize(vec![side]);
    crate::engine::batch::serialize(0, job, doc, lines)
}

/// A job whose resolve failed, as the driver writes it.
fn serialize_failure(job: &dyn Job, f: JobFailure) -> JobOutput {
    let doc = job.finalize(vec![Err(f)]);
    crate::engine::batch::serialize(0, job, doc, vec![])
}

#[cfg(test)]
#[path = "revision_tests.rs"]
mod tests;
