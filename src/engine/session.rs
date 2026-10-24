//! #160 step 2 (spec §4.4.3): the `View` a command builds its detector runs
//! and its L4 analysis on.
//!
//! A `View` owns its target and a few cells (`rel`, `caps`, `raw`, `l4`,
//! `det`, `ordering`), each filled at most once, by [`View::prepare`] alone,
//! from a [`Plan`] that only this module's factories build. No interior
//! mutability: every fill is on `&mut self`, every reader takes `&self` (rule 4).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::analysis::{L4Analysis, L4Request, build_l4};
use crate::engine::l4::capability_cone::CapabilityFact;
use crate::engine::l4::combined_graph::PrimaryGraph;
use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
use crate::engine::l4::exec_relation::ExecRelation;
use crate::engine::l4::summary_runner::SummarizeDiagnostic;
use crate::engine::l5::detector_context::{
    self as dc, CapabilityAnalysis, DetectorArtifacts, DetectorContext, OrderingFactsMap,
};
use crate::engine::l5::digest_cli::DigestEffects;
use crate::engine::l5::registry::{self, Detector, RunOutput, substrate};
use crate::engine::l5::snapshot::CapabilitySnapshot;
use crate::engine::perf_trace as pt;
use crate::program::model::role::Role;
use crate::program::model::workspace::Model;

/// One command's model and the cells built over it (spec §4.4.3).
pub struct View {
    /// Private: the cells describe this target, so it never changes after
    /// `new` (read it through [`View::target`]).
    target: AnalysisTarget,
    /// One execution relation per view (#160 step 4): single-app built by
    /// `prepare`, cross-app `Arc::clone(&base.rel)`. It holds the SCC too, so
    /// single-app the SCC lives from the relation's build, before the det
    /// fill's cone, until `release(Cell::Rel)` (spec §4.4, bounded in §7).
    rel: Option<Arc<ExecRelation>>,
    /// The view's ONE composition over `rel.cone()` (#160 step 6 D2): filled
    /// right after `rel`, consumed by the det fill, released by `prepare`.
    caps: Option<CapabilityAnalysis>,
    /// The witness subjects' raw inherited facts, from the same composition;
    /// filled only under a [`WitnessRequest`].
    raw: Option<HashMap<String, Vec<CapabilityFact>>>,
    /// The tree being read (#160 step 6 D4; #277 D6.5): one at a time, filled
    /// from `caps` and `raw`, released by `prepare` once its read is done.
    tree: Option<CapabilitySnapshot>,
    /// Each tree job's per-root effects (D7 (5); #277 D5.4), read by its
    /// query after the run.
    effects: BTreeMap<JobId, DigestEffects>,
    /// Each fingerprint tree job's owned read (#160 step 9 D5; #277 D5.4),
    /// read off its tree inside `prepare`; taken by [`View::take_tree_read`].
    tree_reads: BTreeMap<JobId, TreeReadOut>,
    l4: Option<L4Analysis>,
    det: Option<DetectorArtifacts>,
    /// One bit per [`Cell`] released; a released cell is never refilled.
    released: u16,
    ordering: Option<OrderingFactsMap>,
    runs: BTreeMap<RunKey, Arc<RunOutput>>,
}

/// A run's identity: its detectors, their mask and its scope.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RunKey {
    pub detectors: Vec<String>,
    pub demanded: u32,
    pub scope_primary: bool,
}

/// What a command needs from its [`View`]. The fields are private: a plan is
/// built only by the factories below (spec §4.4.4).
pub struct Plan {
    /// Each run's detectors and its own scope (#277 D5.5): `true` keeps the
    /// primary app's findings only.
    runs: Vec<(Vec<Detector>, bool)>,
    need_l4: bool,
    /// The substrate bits a post-run projection reads off `det`: digest and
    /// prove `TRANSACTION_SPANS` (the tx map), events `SUMMARIES` (the fanout
    /// report's coverage read). `prepare` asserts every run's union holds
    /// them, naming the missing bit (A2.10).
    after_run: u32,
    /// Whose raw inherited facts the view's composition keeps (#160 step 6
    /// D3), fixed before it runs: the union of the jobs' requests (#277 D5).
    witness: WitnessRequest,
    /// The tree jobs (#277 D5.2), each composed over its own subjects and
    /// read inside `prepare`.
    trees: Vec<(JobId, TreeDemand)>,
}

/// A job's index in its batch (#277).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u32);

/// One job's tree over the cone graph (#277 D5.2): composed over `subjects`,
/// read by `read`, then dropped, all inside `prepare`.
#[derive(Debug)]
pub struct TreeDemand {
    pub subjects: WitnessRequest,
    /// Digest's `--tests` (#254): the effects keep the tree's test-coverage
    /// graph.
    pub tests: bool,
    /// Prove's tree only (#160 step 9 final panel C1): each root's unresolved
    /// cone also counts the tree's analysis gaps it visits. Per job, so a
    /// digest on the same view never gets the count.
    pub gaps_in_cone: bool,
    pub read: TreeRead,
}

/// What a job reads off its tree (#277 D5.2; #160 step 9 D5): the read runs
/// right after the tree's composition, before `det`, and only its owned
/// result outlives `prepare`.
pub enum TreeRead {
    /// Digest's (and prove's) per-root effects (#160 step 6 D7 (5)), into
    /// `Cell::Effects`.
    Effects,
    /// `fingerprint --routine`'s query result, into `Cell::TreeRead`.
    FingerprintQuery(QueryRead),
    /// `fingerprint --inventory-only`'s five keys, into `Cell::TreeRead`.
    Inventory(InventoryRead),
}

impl std::fmt::Debug for TreeRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TreeRead::Effects => "Effects",
            TreeRead::FingerprintQuery(_) => "FingerprintQuery",
            TreeRead::Inventory(_) => "Inventory",
        })
    }
}

/// The routines a plan wants raw inherited facts for (#160 step 6 D3; #277
/// D5.1): every row whose stable id is in `ids`, plus every `Role::Primary`
/// row if `primary`. An empty request keeps no raw facts; it never means
/// "no composition" ([`Plan::composes`], D5.1a).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WitnessRequest {
    ids: BTreeSet<String>,
    primary: bool,
}

impl WitnessRequest {
    /// Digest's changed roots, by stable id: every row of each id is a subject
    /// (a collided id's rows share its cone).
    pub fn roots(ids: BTreeSet<String>) -> Self {
        WitnessRequest {
            ids,
            primary: false,
        }
    }

    /// Every `Role::Primary` routine (policy check).
    pub fn primary() -> Self {
        WitnessRequest {
            ids: BTreeSet::new(),
            primary: true,
        }
    }

    pub fn empty() -> Self {
        WitnessRequest::default()
    }

    pub fn union(&self, other: &Self) -> Self {
        WitnessRequest {
            ids: self.ids.union(&other.ids).cloned().collect(),
            primary: self.primary || other.primary,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && !self.primary
    }

    /// Whether every subject `other` names is named here too.
    fn covers(&self, other: &Self) -> bool {
        other.ids.is_subset(&self.ids) && (self.primary || !other.primary)
    }

    /// The composition's raw scope over `model`. `pub(crate)` so the
    /// scoped-equals-unscoped tests (#160 step 9 T17) compose with the view's
    /// own mapping, not a copy of it.
    pub(crate) fn scope(&self, model: &Model) -> WitnessScope {
        WitnessScope::Subjects(self.subjects(model))
    }

    /// The internal ids of every row this request names in `model`.
    fn subjects(&self, model: &Model) -> BTreeSet<String> {
        model
            .workspace
            .routines
            .iter()
            .filter(|r| {
                self.ids.contains(&r.stable_routine_id)
                    || (self.primary && model.routine_role(r) == Role::Primary)
            })
            .map(|r| r.id.clone())
            .collect()
    }
}

/// The fingerprint query's tree-read inputs (#160 step 9 D3): its filters and
/// whether a `roots.config.json` was skipped. The roles come from the model.
pub struct QueryRead {
    pub filters: crate::engine::l5::fingerprint_query::FingerprintFilters,
    pub roots_config_ignored: bool,
}

/// The inventory's tree-read inputs (#160 step 9 D4): the snapshot options
/// `assemble_full_snapshot` takes, owned (the universe comes from the target).
pub struct InventoryRead {
    pub inputs: Arc<crate::program::inputs::InputSet>,
    pub driver_version: String,
    pub deterministic: bool,
    pub roots_config_ignored: bool,
}

/// A fingerprint tree read's owned result: nothing in it borrows the tree.
pub enum TreeReadOut {
    FingerprintQuery(crate::engine::l5::fingerprint_query::FingerprintQueryResult),
    Inventory(indexmap::IndexMap<String, crate::engine::gate::cbor::CborValue>),
}

/// The fingerprint query's read off the tree (#160 step 9 D3): the query with
/// the model's owners and roles, over the pool (the plan's roots: the routines
/// the selectors named). Its result is owned (S9-16).
fn read_fingerprint_query(
    tree: &CapabilitySnapshot,
    read: &QueryRead,
    pool: &BTreeSet<String>,
    model: &Model,
) -> crate::engine::l5::fingerprint_query::FingerprintQueryResult {
    let owners = crate::engine::l4::effect_query_cli::OwnerIndex::build(model);
    let primary = crate::engine::l5::fingerprint_query::primary_stable_ids(model);
    crate::engine::l5::fingerprint_query::fingerprint_query(
        tree,
        &read.filters,
        Some(pool),
        read.roots_config_ignored,
        &owners,
        &|id| primary.contains(id),
    )
}

/// The inventory's read off the tree (#160 step 9 D4): the assembly half of
/// the full snapshot over the tree, its five inventory keys only. The tree is
/// composed over `Roots(∅)`, so it holds no raw facts and the permission
/// facts are empty (§4.4 item 3).
fn read_inventory(
    tree: CapabilitySnapshot,
    read: &InventoryRead,
    target: &AnalysisTarget,
) -> indexmap::IndexMap<String, crate::engine::gate::cbor::CborValue> {
    use crate::engine::l5::snapshot_full::{
        FullSnapshotOptions, SnapshotKeys, SnapshotUniverse, assemble_full_snapshot,
    };
    let opts = FullSnapshotOptions {
        inputs: &read.inputs,
        driver_version: &read.driver_version,
        deterministic: read.deterministic,
        roots_config_ignored: read.roots_config_ignored,
        universe: SnapshotUniverse::of(target),
    };
    assemble_full_snapshot(
        target.resolved(),
        tree,
        crate::engine::gate::cbor::CborValue::Array(vec![]),
        &opts,
        SnapshotKeys::Inventory,
    )
}

/// The cells [`View::release`] drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Det,
    L4,
    Ordering,
    Rel,
    Caps,
    Raw,
    Tree,
    Effects,
    TreeRead,
}

impl Cell {
    /// Every cell a command can hold past `prepare` and `finish_default_run`
    /// drops: `Caps` and `Tree` never outlive `prepare`, and digest keeps
    /// `Effects` (with `L4`) for its query, then releases both. `TreeRead` is
    /// here as `Raw` is: released whether filled or not (a fingerprint tree
    /// read is usually taken, and so released, first).
    const ALL: [Cell; 6] = [
        Cell::Det,
        Cell::L4,
        Cell::Ordering,
        Cell::Rel,
        Cell::Raw,
        Cell::TreeRead,
    ];

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// `alsem analyze`: the selected detectors.
pub fn analyze_plan(detectors: Vec<Detector>, scope_primary: bool) -> Plan {
    Plan {
        runs: vec![(detectors, scope_primary)],
        need_l4: false,
        after_run: 0,
        witness: WitnessRequest::empty(),
        trees: vec![],
    }
}

/// `alsem digest` and `alsem prove`: the run, then the tx map off
/// `det.transaction_spans` and the query off `l4`. Digest passes its changed
/// roots (stable ids, resolved before the composition, #160 step 6 D3): the
/// view composes their raw facts, builds the tree over the cone graph and
/// keeps their effects ([`View::effects`]); prove passes its one root (#160
/// step 9 D2). No root (a digest or prove that resolved none): neither. `tests`
/// (`digest --tests`, #254): the effects also keep the tree's test-coverage
/// graph, which covers the whole workspace, so a tree is built even with no
/// root (an empty request: no raw facts; the tree demand composes, D5.1a).
/// Standalone, the tree is job 0's.
pub fn digest_plan(detectors: Vec<Detector>, roots: BTreeSet<String>, tests: bool) -> Plan {
    let witness = WitnessRequest::roots(roots);
    let trees = if tests || !witness.is_empty() {
        vec![(
            JobId(0),
            TreeDemand {
                subjects: witness.clone(),
                tests,
                gaps_in_cone: false,
                read: TreeRead::Effects,
            },
        )]
    } else {
        vec![]
    };
    Plan {
        runs: vec![(detectors, true)],
        need_l4: true,
        after_run: substrate::TRANSACTION_SPANS,
        witness,
        trees,
    }
}

/// `alsem prove` (#160 step 9 D2): digest's plan over its one root, or none,
/// whose unresolved cone also counts the analysis gaps it visits (C1).
pub fn prove_plan(detectors: Vec<Detector>, root: Option<String>) -> Plan {
    let mut plan = digest_plan(detectors, root.into_iter().collect(), false);
    for (_, tree) in &mut plan.trees {
        tree.gaps_in_cone = true;
    }
    plan
}

/// One run at primary scope, with nothing read after it: fingerprint and
/// `diff --strict`.
fn run_only_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![(detectors, true)],
        need_l4: false,
        after_run: 0,
        witness: WitnessRequest::empty(),
        trees: vec![],
    }
}

/// [`run_only_plan`] with job 0's tree over `subjects`, read by `read`: the
/// fingerprint tree reads (#160 step 9 D3, D4).
fn run_and_tree_plan(detectors: Vec<Detector>, subjects: WitnessRequest, read: TreeRead) -> Plan {
    Plan {
        witness: subjects.clone(),
        trees: vec![(
            JobId(0),
            TreeDemand {
                subjects,
                tests: false,
                gaps_in_cone: false,
                read,
            },
        )],
        ..run_only_plan(detectors)
    }
}

/// `alsem fingerprint`.
pub fn fingerprint_plan(detectors: Vec<Detector>) -> Plan {
    run_only_plan(detectors)
}

/// `alsem fingerprint --routine` (#160 step 9 D3): fingerprint's run, the raw
/// facts of `pool` (the selectors' routines, stable ids), and the query read
/// off the view's tree inside `prepare`.
pub fn fingerprint_query_plan(
    detectors: Vec<Detector>,
    pool: BTreeSet<String>,
    read: QueryRead,
) -> Plan {
    run_and_tree_plan(
        detectors,
        WitnessRequest::roots(pool),
        TreeRead::FingerprintQuery(read),
    )
}

/// `alsem fingerprint --inventory-only` (#160 step 9 D4): fingerprint's run and
/// the inventory read off a tree over an empty request: a composition with no
/// raw facts (digest `--tests`' shape; the tree demand composes, #277 D5.1a).
pub fn fingerprint_inventory_plan(detectors: Vec<Detector>, read: InventoryRead) -> Plan {
    run_and_tree_plan(
        detectors,
        WitnessRequest::empty(),
        TreeRead::Inventory(read),
    )
}

/// `alsem diff --strict`, per side.
pub fn diff_strict_plan(detectors: Vec<Detector>) -> Plan {
    run_only_plan(detectors)
}

/// `alsem events fanout` / `chains`: the run, then the report off
/// `det.summaries`.
pub fn events_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![(detectors, true)],
        need_l4: false,
        after_run: substrate::SUMMARIES,
        witness: WitnessRequest::empty(),
        trees: vec![],
    }
}

/// `alsem policy check`: the run, then the rule engine off `det.summaries`
/// and the view's raw facts of every primary routine (`View::raw`; #160
/// step 6 Task 3).
pub fn policy_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![(detectors, true)],
        need_l4: false,
        after_run: substrate::SUMMARIES,
        witness: WitnessRequest::primary(),
        trees: vec![],
    }
}

/// `alsem query`: no run, `l4` alone.
pub fn query_plan() -> Plan {
    Plan {
        runs: vec![],
        need_l4: true,
        after_run: 0,
        witness: WitnessRequest::empty(),
        trees: vec![],
    }
}

impl Plan {
    #[cfg(test)]
    pub(crate) fn for_test(
        runs: Vec<Vec<Detector>>,
        scope_primary: bool,
        need_l4: bool,
        after_run: u32,
    ) -> Plan {
        Plan {
            runs: runs.into_iter().map(|r| (r, scope_primary)).collect(),
            need_l4,
            after_run,
            witness: WitnessRequest::empty(),
            trees: vec![],
        }
    }

    /// The plan with `witness`.
    #[cfg(test)]
    pub(crate) fn with_witness(self, witness: WitnessRequest) -> Plan {
        Plan { witness, ..self }
    }

    /// The plan with `trees`.
    #[cfg(test)]
    pub(crate) fn with_trees(self, trees: Vec<(JobId, TreeDemand)>) -> Plan {
        Plan { trees, ..self }
    }

    /// Whether `prepare` composes `job`'s tree and fills `Cell::Effects`: the
    /// one answer a caller reads before `View::effects` (final panel F1-C5).
    pub fn has_tree(&self, job: JobId) -> bool {
        self.trees.iter().any(|(j, _)| *j == job)
    }

    /// Whether the view's composition keeps raw facts: a request or a tree
    /// demand (a tree takes `raw`, empty under an empty request).
    fn keeps_raw(&self) -> bool {
        !self.witness.is_empty() || !self.trees.is_empty()
    }

    /// Whether `prepare` composes (#277 D5.1a): a request, a tree demand
    /// whatever its request's size, or a run that needs derived rows when
    /// `det` is not filled. Never inferred from an empty request alone.
    pub fn composes(&self, det_filled: bool) -> bool {
        self.keeps_raw() || (!det_filled && !self.runs.is_empty() && self.needs_derived())
    }

    /// The derived rows are the det fill's (TRANSACTION_SPANS folds over the
    /// summaries); the raw half is the witness request's.
    fn needs_derived(&self) -> bool {
        self.union() & (substrate::SUMMARIES | substrate::TRANSACTION_SPANS) != 0
    }

    /// The key of each run, in plan order.
    pub fn keys(&self) -> Vec<RunKey> {
        self.runs.iter().map(key).collect()
    }

    fn union(&self) -> u32 {
        self.runs.iter().map(|(r, _)| mask(r)).fold(0, |a, m| a | m)
    }
}

/// A run's key, its scope its own (#277 D5.5).
fn key((run, scope_primary): &(Vec<Detector>, bool)) -> RunKey {
    RunKey {
        detectors: run.iter().map(|d| d.name.clone()).collect(),
        demanded: mask(run),
        scope_primary: *scope_primary,
    }
}

fn mask(run: &[Detector]) -> u32 {
    run.iter().fold(0, |a, d| a | d.requires)
}

/// The constant's name of one `substrate` bit, for panic messages.
fn substrate_bit_name(bit: u32) -> &'static str {
    match bit {
        substrate::SUMMARIES => "SUMMARIES",
        substrate::CORE_SUMMARIES => "CORE_SUMMARIES",
        substrate::TRANSACTION_SPANS => "TRANSACTION_SPANS",
        substrate::CLOSED_WORLD_TEMP => "CLOSED_WORLD_TEMP",
        substrate::DB_EFFECT_REVERSE_INDEX => "DB_EFFECT_REVERSE_INDEX",
        substrate::ORDERING_FACTS => "ORDERING_FACTS",
        _ => "an unnamed substrate bit",
    }
}

/// Every set bit of `bits`, by name, `|`-joined.
fn bit_names(bits: u32) -> String {
    (0..u32::BITS)
        .map(|i| 1u32 << i)
        .filter(|b| bits & b != 0)
        .map(substrate_bit_name)
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Mark `cell` released: one `view.release` trace instant naming it, so a
/// CLI test can see when a command drops what.
fn note_release(released: &mut u16, cell: Cell) {
    pt::instant_lazy(
        "view",
        "view.release",
        || serde_json::json!({ "cell": format!("{cell:?}") }),
    );
    *released |= cell.bit();
}

impl View {
    pub fn new(target: AnalysisTarget) -> View {
        View {
            target,
            rel: None,
            caps: None,
            raw: None,
            tree: None,
            effects: BTreeMap::new(),
            tree_reads: BTreeMap::new(),
            l4: None,
            det: None,
            released: 0,
            ordering: None,
            runs: BTreeMap::new(),
        }
    }

    /// Fill every cell `plan` needs that is still empty, in order (#160 step 6
    /// D7): ordering, rel, ONE composition into `caps` and `raw`; per tree
    /// demand, in job order, its tree (#277 D5.3: its subjects' raw entries
    /// drained or borrowed, `caps.pd` lent or, for the last tree, taken; `Raw`
    /// is released after the last tree unless a later reader asks it) and its
    /// read off it, the effects or a `TreeRead` result (then `Tree` is
    /// released, step 9 D5); det (it
    /// consumes `caps`, then `l4`, then its harvest; `Caps` is released), then
    /// the runs. A plan with no run: rel, then `l4` alone. Rule 1's asserts
    /// (a), (b), (c), the one-witness assertion and the `after_run` assertion
    /// hold in release builds too. A view composes once, so a witness request
    /// comes with its first composing `prepare` or not at all.
    pub fn prepare(&mut self, plan: &Plan) {
        self.prepare_inner(plan, vec![]);
    }

    /// [`View::prepare`] with `L4Request.extra_diagnostics = diags` (§8.4):
    /// no fixture makes the solver report one.
    #[cfg(test)]
    pub(crate) fn prepare_with_diagnostics(
        &mut self,
        plan: &Plan,
        diags: Vec<SummarizeDiagnostic>,
    ) {
        self.prepare_inner(plan, diags);
    }

    fn prepare_inner(&mut self, plan: &Plan, mut extra_diagnostics: Vec<SummarizeDiagnostic>) {
        // Rule 2: no fill from inside a detector.
        assert!(
            !registry::in_detector_loop(),
            "View::prepare inside the detector loop"
        );
        let union = plan.union();
        let need_det = !plan.runs.is_empty();
        let core = union & substrate::CORE_SUMMARIES != 0;
        let need_l4 = plan.need_l4 || core;
        let need_ordering = union & substrate::ORDERING_FACTS != 0;
        let need_derived = plan.needs_derived();
        let need_raw = plan.keeps_raw();
        let need_caps = plan.composes(self.det.is_some());
        let need_rel = need_det || need_l4 || need_raw;
        // D3: the subjects are fixed once, before the one composition: a
        // second request, or a request after the view composed without one.
        assert!(
            !need_raw
                || (self.raw.is_none()
                    && self.released & (Cell::Raw.bit() | Cell::Caps.bit()) == 0),
            "a View takes at most one witness request, in its one composing prepare"
        );
        assert!(
            plan.trees
                .iter()
                .all(|(_, t)| plan.witness.covers(&t.subjects)),
            "every tree demand's subjects are inside the view's request"
        );
        // (a) a released cell is never refilled: a refill after
        // `release(Det)` would find no `CoreSummaries`.
        for (cell, needed) in [
            (Cell::Ordering, need_ordering),
            (Cell::Rel, need_rel),
            (Cell::Det, need_det),
            (Cell::L4, need_l4),
            (Cell::Caps, need_caps),
        ] {
            // D2 (final panel F1-C3): every composition releases `Caps` in its
            // own prepare, so a second one (a det fill needing derived rows
            // after a raw-only composition) breaks the one-composition rule.
            assert!(
                !needed || self.released & cell.bit() == 0,
                "{}",
                if matches!(cell, Cell::Caps) {
                    "a View composes once, in its first composing prepare: a det fill that \
                     needs derived rows comes with that prepare"
                        .to_string()
                } else {
                    format!("{cell:?} released; a View fills each cell once")
                }
            );
        }
        let missing = plan.after_run & !union;
        assert!(
            missing == 0,
            "the plan reads {} off det after its run, but no run demands it",
            bit_names(missing)
        );
        // (b) a filled det covers the new plan, so output never depends on
        // fill state (A2.5).
        if let Some(det) = &self.det {
            let missing = union & !det.demanded;
            assert!(
                missing == 0,
                "Det was filled without {}; a View never widens a cell",
                bit_names(missing)
            );
        }

        let View {
            target,
            rel,
            caps,
            raw,
            tree,
            effects,
            tree_reads,
            l4,
            det,
            released,
            ordering,
            runs,
        } = self;
        // Final panel F1-C2: the dependency calls (cross-app) are read only by
        // a tree; they leave the base in the first prepare and die at the
        // tree's composition below; a plan with no tree drops them here.
        let dependency_calls = match &mut *target {
            AnalysisTarget::CrossApp(b) => b.dependency_calls.take(),
            AnalysisTarget::SingleApp(_) => None,
        }
        .filter(|_| !plan.trees.is_empty());
        let target = &*target;
        let model = target.resolved();
        let cross_base = match target {
            AnalysisTarget::SingleApp(_) => None,
            AnalysisTarget::CrossApp(b) => Some(&**b),
        };
        // The det fill keeps the span the context build had before the View
        // (`run_detectors` / `run_detectors_cross_app`), so its unspanned
        // stretches stay out of the caller's self time.
        let fill_span = (need_det && det.is_none()).then(|| match cross_base {
            None => pt::span("context", "context.build_total"),
            Some(_) => pt::span("crossapp", "crossapp.context_build"),
        });
        // Ordering FIRST: it reads the model alone, and the deadlock rule
        // wants it before the runs.
        if need_ordering && ordering.is_none() {
            *ordering = Some(dc::fill_ordering(model));
        }
        if need_rel && rel.is_none() {
            // Single-app the view builds it (counted in `ExecRelation::new`,
            // its `rel.*` census buckets printed by `single_app`); cross-app it
            // is the base's, built, counted and census-printed with the base.
            *rel = Some(match cross_base {
                Some(b) => Arc::clone(&b.rel),
                None => Arc::new(ExecRelation::single_app(model)),
            });
        }
        // D2: ONE composition over `rel.cone()`, the det cone's inputs.
        if need_caps {
            let r = rel.as_ref().expect("rel filled above");
            let scope = ConeScope {
                derived: need_derived,
                raw: if need_raw {
                    plan.witness.scope(model)
                } else {
                    WitnessScope::None
                },
            };
            let _s = pt::span("context", "context.capability_cones");
            let (c, w) =
                dc::compose_capabilities(model, r.cone(), dc::det_cone_inputs(model, r), scope);
            *caps = Some(c);
            if need_raw {
                *raw = Some(w);
            }
        }
        // D7 (4)-(5), #277 D5.3: per tree demand, in job order, its tree over
        // its own subjects, its read off it, then the tree is released (one
        // at a time, D6.5), all before `det` takes `caps`. A tree drains the
        // entries it is the last reader of; with a later reader (a later
        // tree, or policy's rule engine at (8)) it borrows them. The pd arena
        // is lent to every tree but the last; the dependency calls die after
        // the last.
        if !plan.trees.is_empty() {
            use crate::engine::l5::snapshot::{SubjectRaw, TreeRaw, compose_tree};
            use std::borrow::Cow;
            let mut dependency_calls = dependency_calls;
            let r = rel.as_ref().expect("rel filled above");
            let app_names = cross_base.map(|b| b.loaded_app_names()).unwrap_or_default();
            // D9 (#160 step 9): the dependency apps with no source; none single-app.
            let source_less = cross_base
                .map(|b| crate::engine::l5::snapshot::source_less_apps(&b.dependency_apps))
                .unwrap_or_default();
            let mut map = raw.take().expect("raw filled above");
            let scopes: Vec<BTreeSet<String>> = plan
                .trees
                .iter()
                .map(|(_, t)| t.subjects.subjects(model))
                .collect();
            // The raw readers after the trees: the part of the request no
            // tree asks. Policy, the one such reader, asks `primary` alone.
            // shortcut: a tree that asks `primary` (step 9's inventory) keeps
            // raw alive to the end; upgrade when `Plan` carries the non-tree
            // jobs' own request.
            let rest = WitnessRequest {
                ids: plan
                    .witness
                    .ids
                    .iter()
                    .filter(|id| !plan.trees.iter().any(|(_, t)| t.subjects.ids.contains(*id)))
                    .cloned()
                    .collect(),
                primary: plan.witness.primary,
            };
            let rest_scope = rest.subjects(model);
            for (i, ((job, demand), scope)) in plan.trees.iter().zip(&scopes).enumerate() {
                let last_tree = i + 1 == plan.trees.len();
                let later = |id: &str| {
                    rest_scope.contains(id) || scopes[i + 1..].iter().any(|s| s.contains(id))
                };
                let calls: &[crate::program::model::calls::CallEdge] =
                    match (cross_base, dependency_calls.as_deref()) {
                        (None, _) => &[],
                        (Some(_), Some(c)) => c,
                        (Some(_), None) => panic!(
                            "the dependency calls left the base in an earlier prepare: \
                             digest's tree composes in the view's first prepare"
                        ),
                    };
                let c = caps.as_mut().expect("caps filled above");
                let pd = if last_tree {
                    Cow::Owned(c.pd.take().unwrap_or_default())
                } else {
                    c.pd.as_ref()
                        .map_or_else(|| Cow::Owned(Default::default()), Cow::Borrowed)
                };
                let drain = scope.iter().all(|id| !map.contains_key(id) || !later(id));
                let (tree_raw, done) = if !drain {
                    let acc = SubjectRaw::new(&map, scope);
                    let done: Vec<String> = acc
                        .subjects()
                        .iter()
                        .filter(|id| !later(id))
                        .cloned()
                        .collect();
                    (TreeRaw::Borrow(acc), done)
                } else if map.keys().all(|id| scope.contains(id)) {
                    // A one-tree view with no later reader: the whole map, as
                    // a standalone digest always did.
                    (TreeRaw::Drain(std::mem::take(&mut map)), vec![])
                } else {
                    let own = scope.iter().filter_map(|id| map.remove_entry(id)).collect();
                    (TreeRaw::Drain(own), vec![])
                };
                *tree = Some(compose_tree(
                    model,
                    r.cone(),
                    calls,
                    c,
                    tree_raw,
                    pd,
                    &app_names,
                    &source_less,
                ));
                for id in done {
                    map.remove(&id);
                }
                if last_tree {
                    if rest.is_empty() {
                        note_release(released, Cell::Raw);
                    } else {
                        *raw = Some(std::mem::take(&mut map));
                    }
                    dependency_calls = None;
                }
                // No tree read asks `primary`: digest, prove and the query ask
                // their roots, inventory nothing (#277 D5.1's table).
                debug_assert!(!demand.subjects.primary);
                let roots = &demand.subjects.ids;
                match &demand.read {
                    TreeRead::Effects => {
                        let t = tree.as_mut().expect("tree filled above");
                        let roots: Vec<String> = roots.iter().cloned().collect();
                        let out = crate::engine::l5::digest_cli::digest_effects(
                            t,
                            model,
                            &roots,
                            demand.tests,
                            demand.gaps_in_cone,
                        );
                        effects.insert(*job, out);
                    }
                    TreeRead::FingerprintQuery(read) => {
                        let t = tree.as_ref().expect("tree filled above");
                        let out = read_fingerprint_query(t, read, roots, model);
                        tree_reads.insert(*job, TreeReadOut::FingerprintQuery(out));
                    }
                    TreeRead::Inventory(read) => {
                        // The assembly consumes the tree (its key classes filter it).
                        let t = tree.take().expect("tree filled above");
                        let out = read_inventory(t, read, target);
                        tree_reads.insert(*job, TreeReadOut::Inventory(out));
                    }
                }
                *tree = None;
                note_release(released, Cell::Tree);
            }
        }
        if need_det && det.is_none() {
            let r = rel.as_ref().expect("rel filled above");
            // (c) `CoreSummaries` is a local of the det fill's own solve, so a
            // det fill that needs `l4` builds it in the same call.
            assert!(
                !need_l4 || l4.is_none(),
                "L4 already filled; a det fill that needs l4 builds it in the same call"
            );
            let req = need_l4.then(|| L4Request {
                extra_diagnostics: std::mem::take(&mut extra_diagnostics),
            });
            let (art, solved) = dc::build_detector_artifacts(
                model,
                union,
                r,
                req,
                target.cross_app_inputs(),
                caps.take(),
            );
            *det = Some(art);
            if need_l4 {
                *l4 = solved;
            }
        }
        // D7 (6): nothing reads the composition past the det fill.
        if need_caps {
            *caps = None;
            note_release(released, Cell::Caps);
        }
        drop(fill_span);
        // A plan with no run (query) fills `l4` alone.
        if need_l4 && l4.is_none() {
            let r = rel.as_ref().expect("rel filled above");
            let req = L4Request { extra_diagnostics };
            // The `CoreSummaries` die here: nothing harvests them.
            let (solved, _core) = build_l4(model, r, req, target.cross_app_inputs().as_ref());
            *l4 = Some(solved);
        }
        if !need_det {
            return;
        }
        let art = det.as_ref().expect("det filled above");
        // The live-cell checkpoint (spec §4.7, A6.18): what is alive at the
        // first run. No tree, no pd arena and no raw facts of digest's (D7).
        if plan.runs.iter().any(|run| !runs.contains_key(&key(run))) {
            let pd = caps.as_ref().is_some_and(|c| c.pd.is_some()) || tree.is_some();
            let live: Vec<&str> = [
                ("Rel", rel.is_some()),
                ("Caps", caps.is_some()),
                ("PdArena", pd),
                ("Raw", raw.is_some()),
                ("Tree", tree.is_some()),
                ("Effects", !effects.is_empty()),
                ("TreeRead", !tree_reads.is_empty()),
                ("L4", l4.is_some()),
                ("Det", true),
                ("Ordering", ordering.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, filled)| filled.then_some(name))
            .collect();
            if crate::engine::census::enabled() {
                eprintln!("[retained-census] view.live cells={}", live.join(","));
            }
            pt::instant_lazy("view", "view.live", || serde_json::json!({ "cells": live }));
        }
        for entry @ (run, scope_primary) in &plan.runs {
            let key = key(entry);
            if runs.contains_key(&key) {
                continue;
            }
            // One context per run (its borrowed maps print per construction).
            let ctx: DetectorContext<'_> = dc::detector_context(model, art, ordering.as_ref());
            // Rule 3: a run's summarize diagnostics come from ITS mask.
            let summarize: &[SummarizeDiagnostic] = if key.demanded & substrate::CORE_SUMMARIES != 0
            {
                &l4.as_ref()
                    .expect("l4 filled with CORE_SUMMARIES")
                    .summarize_diagnostics
            } else {
                &[]
            };
            let out = registry::finish_run(model, &ctx, run, cross_base, *scope_primary, summarize);
            runs.insert(key, Arc::new(out));
        }
    }

    /// Drop a cell for good: a later `prepare` that needs it panics. Each
    /// release is one `view.release` trace instant naming the cell, so a CLI
    /// test can see when a command drops what.
    pub fn release(&mut self, cell: Cell) {
        note_release(&mut self.released, cell);
        match cell {
            Cell::Det => self.det = None,
            Cell::L4 => self.l4 = None,
            Cell::Ordering => self.ordering = None,
            Cell::Rel => self.rel = None,
            Cell::Caps => self.caps = None,
            Cell::Raw => self.raw = None,
            Cell::Tree => self.tree = None,
            Cell::Effects => self.effects.clear(),
            Cell::TreeRead => self.tree_reads.clear(),
        }
    }

    /// [`View::release`] every cell except `keep` and those already released:
    /// the one way a command drops what it no longer reads, so no cell
    /// (`Ordering` included) is kept alive by a forgotten line.
    pub fn release_all_but(&mut self, keep: &[Cell]) {
        for cell in Cell::ALL {
            if !keep.contains(&cell) && self.released & cell.bit() == 0 {
                self.release(cell);
            }
        }
    }

    /// `job`'s tree read, moved out (#160 step 9 D5; #277 D5.4); the cell is
    /// released once no job's read is left in it.
    pub fn take_tree_read(&mut self, job: JobId) -> TreeReadOut {
        let out = self
            .tree_reads
            .remove(&job)
            .expect("no tree read: the plan asked for none");
        if self.tree_reads.is_empty() {
            note_release(&mut self.released, Cell::TreeRead);
        }
        out
    }

    /// The output-time census (#160 step 9 §4.5): what is alive when a command
    /// serializes, printed under `ALSEM_RETAINED_CENSUS`.
    pub fn census_output(&self) {
        if crate::engine::census::enabled() {
            eprintln!("{}", self.census_output_line());
        }
    }

    /// The filled cells, the relation's strong count (the target's own handle
    /// included) and whether a tree is alive.
    fn census_output_line(&self) -> String {
        let cells: Vec<&str> = [
            ("Rel", self.rel.is_some()),
            ("Caps", self.caps.is_some()),
            ("Raw", self.raw.is_some()),
            ("Tree", self.tree.is_some()),
            ("Effects", !self.effects.is_empty()),
            ("TreeRead", !self.tree_reads.is_empty()),
            ("L4", self.l4.is_some()),
            ("Det", self.det.is_some()),
            ("Ordering", self.ordering.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, filled)| filled.then_some(name))
        .collect();
        let rel_strong = match &self.target {
            AnalysisTarget::CrossApp(b) => Arc::strong_count(&b.rel),
            AnalysisTarget::SingleApp(_) => self.rel.as_ref().map_or(0, Arc::strong_count),
        };
        format!(
            "[retained-census] view.output cells={} rel_strong={rel_strong} tree={}",
            cells.join(","),
            u8::from(self.tree.is_some())
        )
    }

    pub fn target(&self) -> &AnalysisTarget {
        &self.target
    }

    pub fn model(&self) -> &Model {
        self.target.resolved()
    }

    pub fn rel(&self) -> &Arc<ExecRelation> {
        self.rel.as_ref().expect("rel cell not filled")
    }

    pub fn l4(&self) -> &L4Analysis {
        self.l4.as_ref().expect("l4 cell not filled")
    }

    pub fn det(&self) -> &DetectorArtifacts {
        self.det.as_ref().expect("det cell not filled")
    }

    /// The witness subjects' raw inherited facts (#160 step 6 D2).
    pub fn raw(&self) -> &HashMap<String, Vec<CapabilityFact>> {
        self.raw.as_ref().expect("raw cell not filled")
    }

    /// `job`'s effects (#160 step 6 D7 (5); #277 D5.4), read by its query at
    /// (8).
    pub fn effects(&self, job: JobId) -> &DigestEffects {
        self.effects
            .get(&job)
            .unwrap_or_else(|| panic!("effects of {job:?} not filled"))
    }

    /// `job`'s effects, moved out: digest's query consumes their unresolved
    /// cones (final panel F1-C6). The entry is gone after; the caller still
    /// `release`s `Cell::Effects` when it drops them.
    pub fn take_effects(&mut self, job: JobId) -> DigestEffects {
        self.effects
            .remove(&job)
            .unwrap_or_else(|| panic!("effects of {job:?} not filled"))
    }

    /// The stored output of a run `prepare` ran. An `Arc`: a caller may
    /// `release(..)` while it still holds the output.
    pub fn run(&self, key: &RunKey) -> Arc<RunOutput> {
        Arc::clone(
            self.runs
                .get(key)
                .unwrap_or_else(|| panic!("run {key:?} not prepared")),
        )
    }

    /// Remove a run's stored output, so it dies with the caller's `Arc`:
    /// digest, prove, fingerprint and `diff --strict` drop it before snapshot
    /// composition (§4.4.4).
    pub fn take_run(&mut self, key: &RunKey) -> Arc<RunOutput> {
        self.runs
            .remove(key)
            .unwrap_or_else(|| panic!("run {key:?} not prepared or already taken"))
    }

    /// Take the view apart into what `alsem query` reads once its plan is
    /// prepared: the model, the relation's graph and the `l4` analysis.
    /// Cross-app, the rest of the base (its cone inputs, its own `rel`
    /// handle) drops here, so it is not live through the query and the
    /// render, as at `1524b1fd`.
    pub fn into_query_parts(self) -> (Model, Arc<PrimaryGraph>, L4Analysis) {
        let graph = Arc::clone(self.rel.expect("rel cell not filled").primary());
        let l4 = self.l4.expect("l4 cell not filled");
        let model = match self.target {
            AnalysisTarget::SingleApp(m) => *m,
            AnalysisTarget::CrossApp(b) => b.resolved,
        };
        (model, graph, l4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::summary_runner::SummarizeDiagnostic;
    use crate::engine::l5::detector_context::DetectorContext;
    use crate::engine::l5::registry::{
        Detector, DetectorError, DetectorOutput, DetectorStats, Diagnostic, substrate,
    };
    use crate::program::model::workspace::Model;

    type RunFn = fn(&Model, &DetectorContext) -> Result<DetectorOutput, DetectorError>;

    fn probe(requires: u32, run: RunFn) -> Detector {
        Detector {
            name: "probe".to_string(),
            run,
            requires,
            rule_name: "Probe",
            short_description: "test probe",
        }
    }

    fn noop(_: &Model, _: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        Ok(DetectorOutput::no_diag(
            vec![],
            DetectorStats::new("probe", 0, 0),
        ))
    }

    /// The registry tests' `ordering_output`: a state reported as a diagnostic.
    fn ordering_output(name: &str) -> DetectorOutput {
        DetectorOutput {
            findings: vec![],
            stats: DetectorStats::new(name, 0, 0),
            diagnostics: vec![Diagnostic {
                severity: "warning".to_string(),
                stage: "detect".to_string(),
                message: name.to_string(),
            }],
            d1_cohort_index: None,
        }
    }

    fn ordering_state(_: &Model, c: &DetectorContext) -> Result<DetectorOutput, DetectorError> {
        Ok(ordering_output(if c.ordering.is_some() {
            "filled"
        } else {
            "empty"
        }))
    }

    fn view() -> View {
        View::new(AnalysisTarget::SingleApp(Box::new(
            crate::engine::l5::test_support::empty_resolved(),
        )))
    }

    fn panic_text(what: &str, r: std::thread::Result<()>) -> String {
        let Err(err) = r else {
            panic!("{what}: expected a panic, none came")
        };
        err.downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    /// DISCRIMINATION: removing `prepare`'s `after_run` assertion makes this
    /// fail (no panic).
    #[test]
    #[should_panic(expected = "TRANSACTION_SPANS")]
    fn after_run_bits_must_be_in_the_union() {
        let plan = Plan::for_test(
            vec![vec![probe(0, noop)]],
            true,
            false,
            substrate::TRANSACTION_SPANS,
        );
        view().prepare(&plan);
    }

    /// Pins the factories, not their use (spec §8.4). DISCRIMINATION:
    /// `after_run: 0` in `digest_plan`, separately in `events_plan` and in
    /// `policy_plan`, fails that case.
    #[test]
    fn command_plans_declare_after_run() {
        for (name, plan, bit) in [
            (
                "digest",
                digest_plan(vec![probe(0, noop)], BTreeSet::new(), false),
                "TRANSACTION_SPANS",
            ),
            ("events", events_plan(vec![probe(0, noop)]), "SUMMARIES"),
            ("policy", policy_plan(vec![probe(0, noop)]), "SUMMARIES"),
        ] {
            let msg = panic_text(
                name,
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| view().prepare(&plan))),
            );
            assert!(
                msg.contains(&format!("reads {bit} off det")),
                "{name}: {msg}"
            );
        }
    }

    /// Rule 4 (A2.9). Stated limit: a lazy fill inside a type defined in
    /// another module is not caught here.
    #[test]
    fn no_lazy_fill_in_this_file() {
        let src: String = include_str!("session.rs")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in [
            concat!("Once", "Lock"),
            concat!("Once", "Cell"),
            concat!("Lazy", "Lock"),
            concat!("Lazy", "<"),
            concat!("Cell", "<"),
            concat!("Mut", "ex"),
            concat!("Rw", "Lock"),
            concat!("Ato", "mic"),
            concat!("thread", "_local"),
            concat!("static", " mut"),
            // A `use std::cell::RefCell as R;` alias hides `Cell<`.
            concat!("cell", "::"),
        ] {
            assert!(!src.contains(needle), "session.rs contains {needle}");
        }
    }

    /// The probe reads `c.ordering` from inside the detector loop.
    /// DISCRIMINATION: moving the ordering fill after the runs gives `["empty"]`.
    #[test]
    fn prepare_fills_ordering_before_the_runs() {
        let plan = Plan::for_test(
            vec![vec![probe(substrate::ORDERING_FACTS, ordering_state)]],
            true,
            false,
            0,
        );
        let mut v = view();
        v.prepare(&plan);
        let run = v.run(&plan.keys()[0]);
        let states: Vec<&str> = run.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(states, ["filled"]);
    }

    /// Rule 1 (a) and (b). DISCRIMINATION: deleting assert (a) fails the
    /// first case; deleting assert (b) the second.
    #[test]
    fn a_second_prepare_never_refills_or_widens() {
        let narrow = Plan::for_test(
            vec![vec![probe(substrate::SUMMARIES, noop)]],
            true,
            false,
            0,
        );
        let wide = Plan::for_test(
            vec![vec![probe(
                substrate::SUMMARIES | substrate::TRANSACTION_SPANS,
                noop,
            )]],
            true,
            false,
            0,
        );
        let msg = panic_text(
            "(a) refill after release",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut v = view();
                v.prepare(&narrow);
                v.release(Cell::Det);
                v.prepare(&narrow);
            })),
        );
        assert!(msg.contains("Det released"), "{msg}");
        let msg = panic_text(
            "(b) a wider plan",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut v = view();
                v.prepare(&narrow);
                v.prepare(&wide);
            })),
        );
        assert!(msg.contains("TRANSACTION_SPANS"), "{msg}");
    }

    /// Rule 1 (c): a det fill that needs `l4` while `l4` is filled panics,
    /// with CORE and without it (spec §4.4.3 rule 1). DISCRIMINATION:
    /// deleting assert (c) fails the CORE case (no panic); the non-CORE case
    /// was red against Task 22's narrower `!core || l4.is_none()`.
    #[test]
    fn a_det_fill_needing_l4_never_finds_l4_filled() {
        for (name, requires, need_l4) in [
            ("CORE", substrate::CORE_SUMMARIES, false),
            ("need_l4 without CORE", substrate::SUMMARIES, true),
        ] {
            let plan = Plan::for_test(vec![vec![probe(requires, noop)]], true, need_l4, 0);
            let msg = panic_text(
                name,
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut v = view();
                    v.prepare(&query_plan());
                    v.prepare(&plan);
                })),
            );
            assert!(msg.contains("L4 already filled"), "{name}: {msg}");
        }
    }

    /// #160 step 6 D2/D7: the view composes once, `det` consumes it and
    /// `prepare` releases `Caps`; `Raw` is filled only under a witness request.
    /// DISCRIMINATION: skipping the `Caps` release fails the first assertion;
    /// filling `Raw` whatever the request fails the second.
    #[test]
    fn caps_die_with_the_det_fill_and_raw_needs_a_witness_request() {
        let run = || vec![vec![probe(substrate::SUMMARIES, noop)]];
        let mut v = view();
        v.prepare(&Plan::for_test(run(), true, false, 0));
        assert!(
            v.caps.is_none() && v.released & Cell::Caps.bit() != 0,
            "Caps released after the det fill"
        );
        assert!(v.raw.is_none(), "no witness request, no raw");
        let mut v = view();
        v.prepare(&Plan::for_test(run(), true, false, 0).with_witness(WitnessRequest::primary()));
        assert!(v.raw().is_empty(), "the empty model's primary raw is empty");
    }

    /// #160 step 6 D3: one witness request per view. DISCRIMINATION: deleting
    /// the assertion fails this (no panic).
    #[test]
    #[should_panic(expected = "a View takes at most one witness request")]
    fn a_view_takes_one_witness_request() {
        let plan = || {
            Plan::for_test(
                vec![vec![probe(substrate::SUMMARIES, noop)]],
                true,
                false,
                0,
            )
            .with_witness(WitnessRequest::primary())
        };
        let mut v = view();
        v.prepare(&plan());
        v.prepare(&plan());
    }

    /// #160 step 6 D3 (Task 2b-1 review): a witness request after the view
    /// composed WITHOUT one names the witness rule, not the generic refill.
    /// DISCRIMINATION: dropping `Cell::Caps` from the assertion's released
    /// bits gives the refill loop's `Caps` text and fails this.
    #[test]
    fn a_witness_request_after_a_composition_names_the_rule() {
        let plain = Plan::for_test(
            vec![vec![probe(substrate::SUMMARIES, noop)]],
            true,
            false,
            0,
        );
        let msg = panic_text(
            "witness after a composition",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut v = view();
                v.prepare(&plain);
                let with =
                    Plan::for_test(vec![], true, false, 0).with_witness(WitnessRequest::primary());
                v.prepare(&with);
            })),
        );
        assert!(
            msg.contains("a View takes at most one witness request, in its one composing prepare"),
            "{msg}"
        );
    }

    /// Final panel F1-C3: a det fill that needs derived rows after a raw-only
    /// composition names the one-composition rule, not the generic refill.
    #[test]
    fn a_det_fill_after_a_raw_only_composition_names_the_rule() {
        let msg = panic_text(
            "det after a raw-only composition",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut v = view();
                v.prepare(
                    &Plan::for_test(vec![], true, false, 0).with_witness(WitnessRequest::primary()),
                );
                v.prepare(&Plan::for_test(
                    vec![vec![probe(substrate::SUMMARIES, noop)]],
                    true,
                    false,
                    0,
                ));
            })),
        );
        assert!(
            msg.contains("a View composes once, in its first composing prepare"),
            "{msg}"
        );
    }

    /// #160 step 6 D7 (Task 2b-1 review): a composition that only a witness
    /// request asked for (no det fill consumes it) still dies in `prepare`.
    /// DISCRIMINATION: deleting `*caps = None` fails the first assertion.
    #[test]
    fn a_raw_only_composition_is_released_by_prepare() {
        let mut v = view();
        v.prepare(&Plan::for_test(vec![], true, false, 0).with_witness(WitnessRequest::primary()));
        assert!(v.caps.is_none(), "Caps dropped");
        assert!(v.released & Cell::Caps.bit() != 0, "Caps released");
        assert!(v.raw.is_some(), "precondition: the raw half was filled");
    }

    /// Final panel C1: only prove's plan counts each cone's analysis gaps, read
    /// off the effects `prepare` leaves; digest's cone carries no count.
    /// Precondition: both cones exist. DISCRIMINATION: passing `true` in
    /// place of `plan.gaps_in_cone` at `prepare`'s `digest_effects` call
    /// fails the digest arm.
    #[test]
    fn only_proves_plan_counts_the_gaps_in_its_cone() {
        let root = "app:Codeunit:1#a".to_string();
        let run = || vec![probe(substrate::TRANSACTION_SPANS, noop)];
        for (name, plan, want) in [
            (
                "digest",
                digest_plan(run(), [root.clone()].into(), false),
                None,
            ),
            ("prove", prove_plan(run(), Some(root.clone())), Some(0)),
        ] {
            let mut v = view();
            v.prepare(&plan);
            let effects = v.take_effects(JobId(0));
            let cone = effects.unresolved.get(&root);
            assert!(cone.is_some(), "{name}: precondition, the root's cone");
            assert_eq!(cone.and_then(|c| c.gaps_in_cone), want, "{name}");
        }
    }

    /// #277 D5.1a: a tree demand composes whatever its request's size, with
    /// no run to force it (step 9's `--inventory-only` shape: an empty
    /// request and a tree). No standalone command shows this: digest's own
    /// run composes anyway. DISCRIMINATION: dropping the tree term from
    /// `Plan::composes` leaves the tree no composition, and `prepare` panics.
    #[test]
    fn a_tree_demand_composes_with_an_empty_request() {
        let plan = Plan::for_test(vec![], true, false, 0).with_trees(vec![(
            JobId(0),
            TreeDemand {
                subjects: WitnessRequest::empty(),
                tests: false,
                gaps_in_cone: false,
                read: TreeRead::Effects,
            },
        )]);
        assert!(
            plan.witness.is_empty() && plan.runs.is_empty(),
            "precondition: no request, no run"
        );
        let mut v = view();
        v.prepare(&plan);
        assert!(
            v.effects.contains_key(&JobId(0)),
            "the tree was composed and read"
        );
        assert!(
            v.released & Cell::Caps.bit() != 0,
            "the one composition was released"
        );
    }

    fn effects_tree(subjects: WitnessRequest) -> TreeDemand {
        TreeDemand {
            subjects,
            tests: false,
            gaps_in_cone: false,
            read: TreeRead::Effects,
        }
    }

    /// #277 R3-1: a tree demand naming a subject the view's request does not
    /// is refused. DISCRIMINATION: `|| true` on the assertion → no panic.
    #[test]
    #[should_panic(expected = "inside the view's request")]
    fn a_tree_outside_the_request_is_refused() {
        let plan = Plan::for_test(vec![], true, false, 0)
            .with_witness(WitnessRequest::primary())
            .with_trees(vec![(
                JobId(0),
                effects_tree(WitnessRequest::roots(["x".to_string()].into())),
            )]);
        view().prepare(&plan);
    }

    /// #277 R3-1, the passing half: a `primary` request covers a tree with
    /// no subject.
    #[test]
    fn a_primary_request_covers_an_empty_tree() {
        let plan = Plan::for_test(vec![], true, false, 0)
            .with_witness(WitnessRequest::primary())
            .with_trees(vec![(JobId(0), effects_tree(WitnessRequest::empty()))]);
        let mut v = view();
        v.prepare(&plan);
        assert!(v.effects.contains_key(&JobId(0)));
    }

    /// Every field of `e` as one comparable string.
    fn effects_text(e: &DigestEffects) -> String {
        let unresolved: Vec<String> = e
            .unresolved
            .iter()
            .map(|(k, c)| format!("{k}: {:?} {:?}", c.items, c.traversal))
            .collect();
        format!(
            "{:?}\n{unresolved:?}\n{:?}\n{:?}",
            e.entries, e.coverage, e.identities
        )
    }

    /// #277 Task 4 (D5.3, D5.4): two tree jobs with disjoint roots and a
    /// policy-shaped later raw reader (`primary`) on ONE view, off one
    /// composition. Each job's effects equal its one-tree plan's; the raw
    /// facts outlive the trees for the later reader. Rule B's fixture:
    /// `FillAndPost`'s temp `Fill` insert is a producer only a proof off the
    /// pd arena witnesses, so the second tree reads the arena.
    /// DISCRIMINATION: `caps.pd.take()` per tree (the first tree takes it) →
    /// job 1's effects differ.
    #[test]
    fn two_trees_one_pass() {
        let tmp = tempfile::tempdir().unwrap();
        crate::engine::l5::test_support::write_fixture_rule_b(tmp.path());
        let inputs = crate::program::inputs::WorkspaceInputs::read(tmp.path(), &Default::default());
        let new_view = || View::new(AnalysisTarget::build(&inputs, false, "digest").expect("base"));
        let root = |v: &View, name: &str| -> String {
            v.model()
                .workspace
                .routines
                .iter()
                .find(|r| r.name == name && v.model().routine_role(r) == Role::Primary)
                .unwrap_or_else(|| panic!("precondition: {name}"))
                .stable_routine_id
                .clone()
        };
        let probe_view = new_view();
        let (a, b) = (
            root(&probe_view, "CallWrite"),
            root(&probe_view, "FillAndPost"),
        );
        drop(probe_view);
        let tree = |id: &str| effects_tree(WitnessRequest::roots([id.to_string()].into()));
        let plan = |trees: Vec<(JobId, TreeDemand)>, policy: bool| {
            let mut w = trees
                .iter()
                .fold(WitnessRequest::empty(), |w, (_, t)| w.union(&t.subjects));
            if policy {
                w = w.union(&WitnessRequest::primary());
            }
            Plan::for_test(vec![], true, false, 0)
                .with_witness(w)
                .with_trees(trees)
        };
        let standalone = |id: &str| {
            let mut v = new_view();
            v.prepare(&plan(vec![(JobId(0), tree(id))], false));
            v.take_effects(JobId(0))
        };
        let (sa, sb) = (standalone(&a), standalone(&b));
        assert!(
            sb.entries.iter().flat_map(|e| &e.effects).any(|e| {
                e.detail
                    .iter()
                    .any(|(k, v)| k == "boundary" && v == "XRec.Fill")
                    && crate::engine::l5::digest::is_known_temp_state(e.temp_state.as_ref())
            }),
            "precondition: FillAndPost's standalone effects carry the proof-witnessed Fill insert"
        );

        // R4-1, no later reader (run first, so its breaks fail here): (a)
        // disjoint roots, each tree drains its own entries out of the shared
        // map; (b) one root twice, the first tree borrows and the last
        // drains. DISCRIMINATION: (a) the first tree drains the whole map →
        // "(a) disjoint: job 1"; (b) the borrow tree removes every subject, a
        // later reader or not → "(b) same root: job 1".
        for (name, x, y, sx, sy) in [
            ("(a) disjoint", &a, &b, &sa, &sb),
            ("(b) same root", &b, &b, &sb, &sb),
        ] {
            let mut v = new_view();
            v.prepare(&plan(vec![(JobId(0), tree(x)), (JobId(1), tree(y))], false));
            assert_eq!(
                effects_text(v.effects(JobId(0))),
                effects_text(sx),
                "{name}: job 0"
            );
            assert_eq!(
                effects_text(v.effects(JobId(1))),
                effects_text(sy),
                "{name}: job 1"
            );
            assert!(
                v.raw.is_none() && v.released & Cell::Raw.bit() != 0,
                "{name}: raw released"
            );
        }

        // With a policy-shaped later reader both trees borrow.
        let mut v = new_view();
        v.prepare(&plan(
            vec![(JobId(0), tree(&a)), (JobId(1), tree(&b))],
            true,
        ));
        assert_eq!(
            effects_text(v.effects(JobId(0))),
            effects_text(&sa),
            "job 0"
        );
        assert_eq!(
            effects_text(v.effects(JobId(1))),
            effects_text(&sb),
            "job 1"
        );
        assert!(v.tree.is_none(), "no tree outlives prepare");
        assert!(
            v.released & Cell::Caps.bit() != 0,
            "the one composition was released"
        );
        let fill_and_post: Vec<&str> = v
            .model()
            .workspace
            .routines
            .iter()
            .filter(|r| r.stable_routine_id == b)
            .map(|r| r.id.as_str())
            .collect();
        assert!(
            !fill_and_post.is_empty() && fill_and_post.iter().all(|id| v.raw().contains_key(*id)),
            "the later reader still finds the trees' primary subjects"
        );
    }

    /// #160 step 6 D3, #277 D5.2: digest's plan carries its roots as a
    /// request and job 0's tree demand over them; a digest with no root
    /// (prove, or none resolved) has neither. A roots request scopes every
    /// row of each stable id.
    #[test]
    fn digest_plan_asks_a_tree_only_for_roots() {
        let roots: BTreeSet<String> = ["a:Codeunit:1#x".to_string()].into();
        // Job 0's effects tree over `subjects`, with no gap count (`TreeRead`
        // carries no `PartialEq`: a query read's filters have none).
        let is_tree = |p: &Plan, subjects: WitnessRequest, tests: bool| {
            matches!(&p.trees[..], [(JobId(0), t)] if t.subjects == subjects
                && t.tests == tests
                && !t.gaps_in_cone
                && matches!(t.read, TreeRead::Effects))
        };
        let p = digest_plan(vec![probe(0, noop)], roots.clone(), false);
        assert_eq!(p.witness, WitnessRequest::roots(roots.clone()));
        assert!(
            is_tree(&p, WitnessRequest::roots(roots), false),
            "{:?}",
            p.trees
        );
        let p = digest_plan(vec![probe(0, noop)], BTreeSet::new(), false);
        assert!(p.witness.is_empty() && p.trees.is_empty());
        // `--tests` covers the workspace: a tree with no root (#254), over an
        // empty request (D5.1a).
        let p = digest_plan(vec![probe(0, noop)], BTreeSet::new(), true);
        assert!(p.witness.is_empty());
        assert!(is_tree(&p, WitnessRequest::empty(), true), "{:?}", p.trees);
        let mut m = crate::engine::l5::test_support::empty_resolved();
        for id in ["r1", "r2", "r3"] {
            let mut r = crate::engine::l5::test_support::routine(id, "procedure");
            r.stable_routine_id = if id == "r3" {
                "other#"
            } else {
                "a:Codeunit:1#x"
            }
            .into();
            m.workspace.routines.push(r);
        }
        assert_eq!(
            WitnessRequest::roots(["a:Codeunit:1#x".to_string()].into()).scope(&m),
            WitnessScope::Subjects(["r1".to_string(), "r2".to_string()].into())
        );
    }

    /// #277 T14 (D5.1): the union of two requests scopes exactly the union of
    /// their scopes. Rows stated by assignment: `p1`, `p2` in the primary app,
    /// `d1` in a dependency. DISCRIMINATION: `primary: a.primary && b.primary`
    /// in `union` fails the (roots, primary) case.
    #[test]
    fn witness_request_union() {
        let mut m = crate::engine::l5::test_support::empty_resolved();
        m.primary_app = Some(crate::engine::gate::app_attribution::App {
            app_guid: "app".to_string(),
            publisher: String::new(),
            name: String::new(),
            version: String::new(),
        });
        for (id, app) in [("p1", "app"), ("p2", "app"), ("d1", "dep")] {
            let mut r = crate::engine::l5::test_support::routine(id, "procedure");
            r.app_guid = app.to_string();
            r.stable_routine_id = format!("s:{id}");
            m.workspace.routines.push(r);
        }
        let subjects = |w: &WitnessRequest| match w.scope(&m) {
            WitnessScope::Subjects(s) => s,
            _ => panic!("a request scopes its subjects"),
        };
        let ids = |xs: &[&str]| -> BTreeSet<String> { xs.iter().map(|x| x.to_string()).collect() };
        assert_eq!(
            subjects(&WitnessRequest::primary()),
            ids(&["p1", "p2"]),
            "precondition: primary scopes the primary rows only"
        );
        for (name, a, b) in [
            (
                "(roots, roots)",
                WitnessRequest::roots(ids(&["s:d1"])),
                WitnessRequest::roots(ids(&["s:p1"])),
            ),
            (
                "(roots, primary)",
                WitnessRequest::roots(ids(&["s:d1"])),
                WitnessRequest::primary(),
            ),
            (
                "(empty, primary)",
                WitnessRequest::empty(),
                WitnessRequest::primary(),
            ),
            (
                "(empty, empty)",
                WitnessRequest::empty(),
                WitnessRequest::empty(),
            ),
        ] {
            let mut want = subjects(&a);
            want.extend(subjects(&b));
            assert_eq!(subjects(&a.union(&b)), want, "{name}");
        }
        assert!(
            WitnessRequest::empty()
                .union(&WitnessRequest::empty())
                .is_empty()
        );
    }

    /// `take_run` removes the stored output: a second take, or a `run` of the
    /// key, panics naming it. DISCRIMINATION: `take_run` returning a clone
    /// (`self.run(key)`) without removing the entry fails both cases.
    #[test]
    fn a_taken_run_is_gone() {
        type After = fn(&mut View, &RunKey);
        let plan = Plan::for_test(vec![vec![probe(0, noop)]], true, false, 0);
        let key = plan.keys()[0].clone();
        let cases: [(&str, After); 2] = [
            ("a second take", |v, k| drop(v.take_run(k))),
            ("run", |v, k| drop(v.run(k))),
        ];
        for (name, after) in cases {
            let msg = panic_text(
                name,
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut v = view();
                    v.prepare(&plan);
                    drop(v.take_run(&key));
                    after(&mut v, &key);
                })),
            );
            assert!(msg.contains("\"probe\""), "{name}: {msg}");
        }
    }

    /// `release_all_but` drops every filled cell but the kept ones, Ordering
    /// included. DISCRIMINATION: leaving `Cell::Ordering` out of `Cell::ALL`
    /// fails the ordering assertion.
    #[test]
    fn release_all_but_keeps_only_the_named_cells() {
        let plan = Plan::for_test(
            vec![vec![probe(substrate::ORDERING_FACTS, noop)]],
            true,
            true,
            0,
        );
        let mut v = view();
        v.prepare(&plan);
        assert!(
            v.ordering.is_some() && v.det.is_some() && v.l4.is_some() && v.rel.is_some(),
            "precondition: all four cells filled"
        );
        v.release_all_but(&[Cell::L4]);
        assert!(v.ordering.is_none(), "ordering released");
        assert!(v.det.is_none() && v.rel.is_none(), "det and rel released");
        assert!(v.l4.is_some(), "the kept cell stays");
        assert_eq!(
            v.released,
            Cell::Det.bit()
                | Cell::Ordering.bit()
                | Cell::Rel.bit()
                | Cell::Raw.bit()
                | Cell::TreeRead.bit()
        );
    }

    /// A query read with no filter (every root, witnesses off).
    fn query_read() -> QueryRead {
        QueryRead {
            filters: crate::engine::l5::fingerprint_query::FingerprintFilters {
                roots: None,
                routine_selectors: vec![],
                include_inherited: true,
                witness_limit: crate::engine::l5::fingerprint_query::WitnessLimit::Disabled,
            },
            roots_config_ignored: false,
        }
    }

    /// An inventory read over an empty workspace's input set.
    fn inventory_read() -> InventoryRead {
        let tmp = tempfile::tempdir().unwrap();
        InventoryRead {
            inputs: crate::program::inputs::InputSet::read(tmp.path(), &Default::default()),
            driver_version: "t".to_string(),
            deterministic: true,
            roots_config_ignored: false,
        }
    }

    /// A det-running plan with job 0's tree, read by `read`, over an empty
    /// request.
    fn tree_plan(read: TreeRead) -> Plan {
        Plan::for_test(
            vec![vec![probe(substrate::SUMMARIES, noop)]],
            true,
            false,
            0,
        )
        .with_trees(vec![(
            JobId(0),
            TreeDemand {
                subjects: WitnessRequest::empty(),
                tests: false,
                gaps_in_cone: false,
                read,
            },
        )])
    }

    /// #160 step 9 T20 (spec §5): the tree-read lifecycle. (a) a
    /// `FingerprintQuery` read leaves a result `take_tree_read` returns once;
    /// (b) a `TreeRead` cell, filled and untaken or never filled, dies in
    /// `release_all_but(&[])`;
    /// (c) after `prepare` with any tree read the tree is gone, though `det`
    /// ran after it (E18); (d) `take_tree_read` after an `Effects` plan panics
    /// with its message; (e) `Effects` still fills the effects; (f) every
    /// `Cell` bit is non-zero and distinct (the `u16` mask). The output-time
    /// census line is read field by field beside (b).
    /// DISCRIMINATION: B1, skipping the tree drop after a `FingerprintQuery`
    /// read, fails (c); B2, giving `Cell::TreeRead` the bit of `Cell::Det`,
    /// fails (f).
    #[test]
    fn tree_read_lifecycle() {
        // (a) and (c), FingerprintQuery.
        let mut v = view();
        v.prepare(&tree_plan(TreeRead::FingerprintQuery(query_read())));
        assert!(
            v.det.is_some(),
            "(c) precondition: det ran in the same prepare"
        );
        assert!(
            v.tree.is_none() && v.released & Cell::Tree.bit() != 0,
            "(c) the FingerprintQuery tree died inside prepare"
        );
        assert!(v.effects.is_empty(), "(a) a query read fills no effects");
        let TreeReadOut::FingerprintQuery(r) = v.take_tree_read(JobId(0)) else {
            panic!("(a) a FingerprintQuery read leaves a query result")
        };
        assert!(r.blocks.is_empty() && r.diagnostics.is_empty());
        assert!(
            v.tree_reads.is_empty() && v.released & Cell::TreeRead.bit() != 0,
            "(a) the take empties and releases the cell"
        );
        let msg = panic_text(
            "(a) a second take",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drop(v.take_tree_read(JobId(0)))
            })),
        );
        assert!(msg.contains("no tree read"), "(a) {msg}");

        // (c) Inventory, then (b) and the census line.
        let mut v = view();
        v.prepare(&tree_plan(TreeRead::Inventory(inventory_read())));
        assert!(
            v.tree.is_none() && v.released & Cell::Tree.bit() != 0,
            "(c) the Inventory tree died inside prepare"
        );
        assert!(
            matches!(v.tree_reads.get(&JobId(0)), Some(TreeReadOut::Inventory(_))),
            "(b) precondition: an Inventory result is filled"
        );
        assert_eq!(
            v.census_output_line(),
            "[retained-census] view.output cells=Rel,TreeRead,Det rel_strong=1 tree=0"
        );
        v.release_all_but(&[]);
        assert!(v.tree_reads.is_empty(), "(b) the untaken result is dropped");
        assert!(v.released & Cell::TreeRead.bit() != 0, "(b) and released");
        assert_eq!(
            v.census_output_line(),
            "[retained-census] view.output cells= rel_strong=0 tree=0"
        );

        // (d) and (e), Effects.
        let mut v = view();
        let plan = tree_plan(TreeRead::Effects);
        assert!(
            plan.has_tree(JobId(0)),
            "(e) an Effects plan has job 0's tree"
        );
        v.prepare(&plan);
        assert!(v.tree.is_none(), "(e) the Effects tree died inside prepare");
        assert!(
            v.tree_reads.is_empty(),
            "(e) Effects fills no TreeRead cell"
        );
        drop(v.take_effects(JobId(0)));
        v.release_all_but(&[]);
        assert_ne!(
            v.released & Cell::TreeRead.bit(),
            0,
            "(b) an unfilled TreeRead is released too, as an unfilled Raw is"
        );
        let msg = panic_text(
            "(d) take_tree_read after Effects",
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drop(v.take_tree_read(JobId(0)))
            })),
        );
        assert!(
            msg.contains("no tree read: the plan asked for none"),
            "(d) {msg}"
        );
        // (e)'s converse, that only `Effects` fills the effects, is (a)'s "a
        // query read fills no effects" (#277: no `TreeRead::None`; no tree
        // demand is no tree).

        // (f) Every variant, listed by an exhaustive match so a new one is listed too.
        let all = [
            Cell::Det,
            Cell::L4,
            Cell::Ordering,
            Cell::Rel,
            Cell::Caps,
            Cell::Raw,
            Cell::Tree,
            Cell::Effects,
            Cell::TreeRead,
        ];
        for c in all {
            match c {
                Cell::Det
                | Cell::L4
                | Cell::Ordering
                | Cell::Rel
                | Cell::Caps
                | Cell::Raw
                | Cell::Tree
                | Cell::Effects
                | Cell::TreeRead => assert_ne!(c.bit(), 0, "(f) {c:?} has a bit"),
            }
        }
        let mask = all.iter().fold(0u16, |m, c| m | c.bit());
        assert_eq!(
            mask.count_ones() as usize,
            all.len(),
            "(f) every Cell bit is distinct"
        );
    }

    /// A2.10's "naming the missing bit" for every bit. DISCRIMINATION:
    /// deleting one arm of `substrate_bit_name` fails this.
    #[test]
    fn every_substrate_bit_has_a_name() {
        let all = substrate::ALL | substrate::DB_EFFECT_REVERSE_INDEX | substrate::ORDERING_FACTS;
        assert_eq!(all.count_ones(), 6, "precondition: six bits");
        for i in 0..u32::BITS {
            let bit = 1u32 << i;
            if all & bit != 0 {
                let name = substrate_bit_name(bit);
                assert!(!name.contains("unnamed"), "bit {i}: {name}");
            }
        }
    }

    /// Rule 3. `d` is hand-stated: no fixture makes the solver cap (§4.4.7),
    /// so a real solve would leave both lists empty. DISCRIMINATION: taking
    /// the run's diagnostics from `det.demanded` instead of the key fails the
    /// empty assertion.
    #[test]
    fn run_diagnostics_follow_the_run_mask() {
        let d = SummarizeDiagnostic {
            severity: "warning".to_string(),
            stage: "summarize".to_string(),
            message: "x".to_string(),
        };
        let plan = Plan::for_test(
            vec![
                vec![probe(substrate::CORE_SUMMARIES, noop)],
                vec![probe(substrate::SUMMARIES, noop)],
            ],
            true,
            false,
            0,
        );
        let mut v = view();
        v.prepare_with_diagnostics(&plan, vec![d.clone()]);
        let k = plan.keys();
        let converted = Diagnostic {
            severity: d.severity,
            stage: d.stage,
            message: d.message,
        };
        // The seam reached the run (non-vacuity).
        assert_eq!(v.run(&k[0]).summarize_diagnostics, [converted]);
        assert!(v.run(&k[1]).summarize_diagnostics.is_empty());
    }
}
