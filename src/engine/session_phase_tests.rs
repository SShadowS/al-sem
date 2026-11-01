//! Context-server design r3 §4.2 / r4 §11 (stage 1, T-phase): the borrowed
//! phase path. [`Prepared`] is the smallest consumer of the phase functions
//! that keeps what it fills, as stage 2's revision will: one target, one
//! relation, one composition (derived rows and the primary routines' raw
//! facts, its pd arena retained), one solve with its `CoreSummaries` behind an
//! `Arc`, a det harvested from them at any mask and refillable, and runs
//! cached by key. A request is served as a [`View::served`] view that a real
//! job computes on, so each warm answer is compared byte for byte with the
//! job's cold standalone run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::*;
use crate::engine::batch::job::{Job, LaterReads, ResolveCx, ViewCx, ViewKey, ViewNeeds};
use crate::engine::batch::output::{JobOutput, capture};
use crate::engine::l4::analysis::build_l4_controlled;
use crate::engine::l4::summary_runner::{L4Reuse, SolveBaseline};
use crate::engine::l5::detector_context::{CapsInput, CoreInput, L4Input};
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};

/// How often each shared phase ran on one [`Prepared`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Fills {
    compose: usize,
    emit_raw: usize,
    l4: usize,
    harvest: usize,
}

/// The retained solve: the analysis, its core summaries (never consumed)
/// and the baseline the next seeded solve reads.
struct Solved {
    l4: Arc<L4Analysis>,
    core: Arc<CoreSummaries>,
    baseline: SolveBaseline,
    lines: Vec<String>,
}

/// One prepared state over one target (see the module doc).
struct Prepared {
    /// Lent to each served view and taken back after its compute.
    target: Option<AnalysisTarget>,
    rel: Arc<ExecRelation>,
    /// The base's handle, cloned, never taken (cross-app; none single-app).
    dependency_calls: Option<Arc<Vec<crate::program::model::calls::CallEdge>>>,
    caps: Option<CapabilityAnalysis>,
    /// The primary routines' raw facts and their internal ids.
    raw: Option<(InheritedFacts, BTreeSet<String>)>,
    solved: Option<Solved>,
    det: Option<Arc<DetectorArtifacts>>,
    ordering: Option<OrderingFactsMap>,
    rules: Option<RulesIndex>,
    runs: BTreeMap<RunKey, Arc<RunOutput>>,
    fills: Fills,
    /// The break hook of the discrimination proofs: `true` hands the harvest
    /// the core summaries OWNED, drained out of the retained `Arc`, the way
    /// the one-shot path drains its own.
    drain_core: bool,
}

/// The needs every served command's view takes (explain's: rules and card
/// text on top of the model), so one model serves them all.
const SERVED: ViewNeeds = ViewNeeds {
    rules: true,
    card_text: true,
    ..ViewNeeds::MODEL
};

impl Prepared {
    fn new(inputs: &Arc<WorkspaceInputs>, single_app: bool) -> Prepared {
        let key = ViewKey::new(&inputs.workspace, single_app, false);
        let built = crate::engine::gate::run::build_view_model(inputs, &key, SERVED, |_| {})
            .unwrap_or_else(|f| panic!("the fixture builds: {f:?}"))
            .expect("a model view");
        let target = built.target;
        let rel = relation_of(&target);
        let dependency_calls = match &target {
            AnalysisTarget::CrossApp(b) => b.dependency_calls.clone(),
            AnalysisTarget::SingleApp(_) => None,
        };
        Prepared {
            target: Some(target),
            rel,
            dependency_calls,
            caps: None,
            raw: None,
            solved: None,
            det: None,
            ordering: None,
            rules: None,
            runs: BTreeMap::new(),
            fills: Fills::default(),
            drain_core: false,
        }
    }

    fn target(&self) -> &AnalysisTarget {
        self.target.as_ref().expect("the target is home")
    }

    fn evict_det(&mut self) {
        self.det = None;
    }

    /// The one composition, on first need: every derived row and the
    /// primary routines' raw facts.
    fn compose(&mut self) {
        if self.caps.is_some() {
            return;
        }
        let target = self.target.as_ref().expect("home");
        let model = target.resolved();
        let primary = WitnessRequest::primary();
        let scope = ConeScope {
            derived: true,
            raw: primary.scope(model),
        };
        let (caps, raw) = compose_cone(model, &self.rel, scope);
        self.caps = Some(caps);
        self.raw = Some((raw, primary.subjects(model)));
        self.fills.compose += 1;
    }

    fn solve(&mut self) {
        if self.solved.is_some() {
            return;
        }
        let target = self.target.as_ref().expect("home");
        let cancel = || false;
        let req = L4Request {
            extra_diagnostics: vec![],
            control: Some(crate::engine::l4::summary_runner::L4Control {
                cancel: &cancel,
                reuse: None,
            }),
        };
        let cross = target.cross_app_inputs();
        let (r, lines) =
            capture(|| build_l4_controlled(target.resolved(), &self.rel, req, cross.as_ref()));
        let (l4, core, outcome) = r.expect("never cancelled");
        self.solved = Some(Solved {
            l4: Arc::new(l4),
            core: Arc::new(core),
            baseline: outcome.expect("a controlled solve").baseline,
            lines,
        });
        self.fills.l4 += 1;
    }

    /// Fill what `plan` reads and hand it out as a served view, with the
    /// job lines a standalone run of the plan prints.
    fn serve(&mut self, plan: &Plan) -> (View, Vec<String>) {
        let union = plan.union();
        let need_det = !plan.runs.is_empty();
        if (union | plan.own) & substrate::ORDERING_FACTS != 0 && self.ordering.is_none() {
            self.ordering = Some(dc::fill_ordering(self.target().resolved()));
        }
        if (union | plan.own) & substrate::RULES != 0 && self.rules.is_none() {
            self.rules = Some(dc::fill_rules(self.target().resolved()));
        }
        if !plan.trees.is_empty() || (need_det && plan.needs_derived()) {
            self.compose();
        }
        if plan.builds_l4() {
            self.solve();
        }
        let ((effects, tree_reads), mut lines) = capture(|| self.trees(plan));
        if need_det {
            self.harvest(union);
        }
        let (runs, run_lines) = capture(|| self.runs(plan));
        lines.extend(run_lines);
        if plan.builds_l4() {
            let mut l4_lines = self.solved.as_ref().expect("solved").lines.clone();
            l4_lines.extend(lines);
            lines = l4_lines;
        }
        let view = View::served(
            self.target.take().expect("home"),
            Arc::clone(&self.rel),
            self.solved
                .as_ref()
                .filter(|_| plan.builds_l4())
                .map(|s| Arc::clone(&s.l4)),
            self.det.clone().filter(|_| need_det),
            effects,
            tree_reads,
            runs,
        );
        (view, lines)
    }

    /// The plan's trees, each over the retained composition: raw borrowed
    /// (from the primary emission, or a scoped emission for subjects outside
    /// it), the pd arena and the dependency calls lent.
    fn trees(
        &mut self,
        plan: &Plan,
    ) -> (BTreeMap<JobId, DigestEffects>, BTreeMap<JobId, TreeReadOut>) {
        use crate::engine::l5::snapshot::{SubjectRaw, TreeRaw, compose_tree};
        let (mut effects, mut tree_reads) = (BTreeMap::new(), BTreeMap::new());
        let target = self.target.as_ref().expect("home");
        let model = target.resolved();
        let cross_base = match target {
            AnalysisTarget::SingleApp(_) => None,
            AnalysisTarget::CrossApp(b) => Some(&**b),
        };
        let apps = tree_apps(cross_base);
        for group in tree_groups(&plan.trees) {
            let request = group.iter().fold(WitnessRequest::empty(), |w, &i| {
                w.union(&plan.trees[i].1.subjects)
            });
            let scope = request.subjects(model);
            let (retained, primary) = self.raw.as_ref().expect("composed");
            let emitted;
            let map = if scope.is_subset(primary) {
                retained
            } else {
                emitted = emit_raw(model, &self.rel, &request);
                self.fills.emit_raw += 1;
                &emitted
            };
            let caps = self.caps.as_ref().expect("composed");
            let pd = caps.pd.as_ref().expect("the retained pd arena");
            let calls = self
                .dependency_calls
                .as_deref()
                .map_or(&[][..], Vec::as_slice);
            let tree = compose_tree(
                model,
                self.rel.cone(),
                calls,
                caps,
                TreeRaw::Borrow(SubjectRaw::new(map, &scope)),
                std::borrow::Cow::Borrowed(pd),
                &apps,
            );
            read_tree(
                &mut Some(tree),
                &plan.trees,
                &group,
                target,
                &mut effects,
                &mut tree_reads,
            );
        }
        (effects, tree_reads)
    }

    /// A det covering `union`, harvested from the lent composition and the
    /// retained solve; a det that does not cover it is refilled at the union
    /// of both masks.
    fn harvest(&mut self, union: u32) {
        if self.det.as_ref().is_some_and(|d| union & !d.demanded == 0) {
            return;
        }
        let mask = union | self.det.as_ref().map_or(0, |d| d.demanded);
        if mask & substrate::CORE_SUMMARIES != 0 {
            self.solve();
        }
        let drained;
        let target = self.target.as_ref().expect("home");
        let l4 = match &mut self.solved {
            None => L4Input::None,
            Some(s) if self.drain_core => {
                drained = CoreSummaries(std::mem::take(
                    &mut Arc::get_mut(&mut s.core).expect("one handle").0,
                ));
                L4Input::Solved(&s.l4, CoreInput::Owned(drained))
            }
            Some(s) => L4Input::Solved(&s.l4, CoreInput::Borrowed(&s.core)),
        };
        let (art, solved) = dc::harvest_detector_artifacts(
            target.resolved(),
            mask,
            &self.rel,
            self.caps.as_ref().map(CapsInput::Borrowed),
            l4,
            target.cross_app_inputs(),
        );
        assert!(solved.is_none(), "a lent solve is not solved again");
        self.det = Some(Arc::new(art));
        self.fills.harvest += 1;
    }

    fn runs(&mut self, plan: &Plan) -> BTreeMap<RunKey, Arc<RunOutput>> {
        let target = self.target.as_ref().expect("home");
        let cross_base = match target {
            AnalysisTarget::SingleApp(_) => None,
            AnalysisTarget::CrossApp(b) => Some(&**b),
        };
        let mut out = BTreeMap::new();
        for entry in &plan.runs {
            let k = key(entry);
            let run = match self.runs.get(&k) {
                Some(r) => Arc::clone(r),
                None => {
                    let r = Arc::new(run_once(
                        target.resolved(),
                        self.det.as_ref().expect("harvested"),
                        self.ordering.as_ref(),
                        self.rules.as_ref(),
                        self.solved.as_ref().map(|s| &*s.l4),
                        entry,
                        &k,
                        cross_base,
                    ));
                    self.runs.insert(k.clone(), Arc::clone(&r));
                    r
                }
            };
            out.insert(k, run);
        }
        out
    }

    /// `job` answered off this state, as the driver answers a one-demand job.
    fn answer(&mut self, job: &dyn Job) -> JobOutput {
        let d = job.demands().into_iter().next().expect("a job with a view");
        let part = match job.resolve(d.side, &ResolveCx::new(self.target().resolved(), &d.inputs)) {
            Ok(p) => p,
            Err(f) => {
                let doc = job.finalize(vec![Err(f)]);
                return crate::engine::batch::serialize(0, job, doc, vec![]);
            }
        };
        let plan = Plan::merge([(JobId(0), part.plan)]);
        let (keys, has_tree) = (plan.keys(), plan.has_tree(JobId(0)));
        let (mut view, mut lines) = self.serve(&plan);
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
        self.target = Some(view.into_target());
        lines.extend(own);
        let doc = job.finalize(vec![side]);
        crate::engine::batch::serialize(0, job, doc, lines)
    }
}

/// One served command, built over `inputs`.
#[derive(Clone, Copy, Debug)]
enum Cmd {
    Query,
    Digest,
    Explain,
    Prove,
}

struct Fixture {
    inputs: Arc<WorkspaceInputs>,
    single_app: bool,
    /// The routine every command names.
    routine: &'static str,
    _dir: Option<tempfile::TempDir>,
}

/// The binary's read for digest and prove (`FINGERPRINT_ROWS`), which the
/// other two commands' answers do not depend on.
const READ: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

fn r0(ws: &str, routine: &'static str) -> Fixture {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws);
    Fixture {
        inputs: WorkspaceInputs::read(&path, &READ),
        single_app: true,
        routine,
        _dir: None,
    }
}

/// The rule-B fixture: a workspace over a source-bearing dependency, cross-app
/// (dependency calls, temporary records, events and commits).
fn rule_b(routine: &'static str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_rule_b(dir.path());
    Fixture {
        inputs: WorkspaceInputs::read(dir.path(), &READ),
        single_app: false,
        routine,
        _dir: Some(dir),
    }
}

fn job(cmd: Cmd, f: &Fixture) -> Box<dyn Job + '_> {
    use crate::engine::l5::digest_cli::{DiffRoot, DigestJob, DigestRequest};
    use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    match cmd {
        Cmd::Query => Box::new(crate::engine::l4::effect_query_cli::QueryJob::effects(
            &f.inputs,
            f.routine,
            "test",
            true,
            f.single_app,
        )),
        Cmd::Digest => Box::new(DigestJob::new(
            DigestRequest {
                inputs: &f.inputs,
                changed_files: None,
                changed_routines: Some(vec![f.routine.to_string()]),
                diff_text: None,
                alsem_ver: "test",
                deterministic: true,
                single_app: f.single_app,
                tests: false,
                include_reads: false,
                diff_root: DiffRoot::Workspace,
            },
            false,
            None,
        )),
        Cmd::Explain => Box::new(ExplainJob::new(
            ExplainRequest {
                inputs: &f.inputs,
                mode: ExplainMode::Routines(vec![f.routine.to_string()]),
                single_app: f.single_app,
            },
            false,
            None,
        )),
        Cmd::Prove => Box::new(crate::engine::l5::prove::ProveJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            inputs: &f.inputs,
            routine_selector: f.routine,
            question_text: "may-commit",
            alsem_ver: "test",
            deterministic: true,
            single_app: f.single_app,
            closed_world: false,
            human: false,
            out: None,
        }),
    }
}

fn cold(cmd: Cmd, f: &Fixture) -> JobOutput {
    crate::engine::batch::standalone_output(&*job(cmd, f))
}

/// `order` answered in sequence off ONE prepared state, each answer equal to
/// its cold standalone run. Returns the state's fill counts.
fn assert_order_equals_cold(f: &Fixture, order: &[Cmd]) -> Fills {
    let mut p = Prepared::new(&f.inputs, f.single_app);
    for &cmd in order {
        let warm = p.answer(&*job(cmd, f));
        let cold = cold(cmd, f);
        assert_eq!(
            warm.exit, cold.exit,
            "{order:?}: {cmd:?} on {}: exit",
            f.routine
        );
        assert_eq!(warm, cold, "{order:?}: {cmd:?} on {}", f.routine);
        assert!(
            warm.exit == 0,
            "precondition: {cmd:?} answers on {} ({:?})",
            f.routine,
            warm.message()
        );
    }
    p.fills
}

/// T-phase (r3 §11, stage 1): mixed-order sequences over one prepared state
/// — query→digest, digest→query, explain→prove, prove→explain — each answer
/// byte-equal to its cold one-shot run, single-app and cross-app. The state
/// composes once and solves once whatever the order.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-01): the lent
/// harvest arm handing the det an EMPTY derived store instead of sharing
/// the composition's fails `[Query, Digest]: Digest on DepWriteThenCommit`
/// (its commit's transaction context); the served trees lent an empty pd
/// arena instead of the retained one fails `[Query, Digest]: Digest on
/// ForwardedAfterValidate`.
#[test]
fn mixed_orders_over_one_prepared_state_equal_cold_runs() {
    use Cmd::*;
    // Rule B's `DepWriteThenCommit` commits after a callee's write: its
    // commit's transaction context reads the callee's derived rows (through
    // `det.transaction_spans`), so a lent harvest that lost them answers
    // digest differently.
    let fixtures = [
        r0("ws-d37", "ForwardedAfterValidate"),
        rule_b("WriteThenCommit"),
        rule_b("DepWriteThenCommit"),
    ];
    for f in &fixtures {
        for order in [
            &[Query, Digest][..],
            &[Digest, Query],
            &[Explain, Prove],
            &[Prove, Explain],
        ] {
            let fills = assert_order_equals_cold(f, order);
            assert_eq!(
                fills.compose,
                usize::from(order.iter().any(|c| !matches!(c, Query)))
            );
            assert_eq!(
                fills.l4,
                usize::from(order.iter().any(|c| matches!(c, Query | Digest | Prove)))
            );
            assert_eq!(fills.emit_raw, 0, "every root is primary");
        }
    }
}

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

/// A root inside a dependency (rule B's `Dep Rec Lib.Raise`; no CLI selector
/// names one today, so the plan is built by hand, stage 2's dependency-root
/// request): the retained composition emitted raw facts for the primary
/// routines only, so its tree borrows a scoped emission ([`emit_raw`]), and
/// its effects equal a cold one-shot view's over the same plan, after a
/// primary request on the same state.
///
/// DISCRIMINATION: borrowing the retained primary map for the dependency
/// root (no emission) leaves its tree without the root's raw facts: the
/// effects differ (empty against the inherited insert). With `Write` as the
/// root the same break passed the effects (a callee-less routine inherits
/// nothing), so the root is `Raise`.
#[test]
fn a_dependency_root_borrows_a_scoped_emission() {
    let f = rule_b("WriteThenCommit");
    let mut p = Prepared::new(&f.inputs, f.single_app);
    assert_eq!(p.answer(&*job(Cmd::Explain, &f)), cold(Cmd::Explain, &f));
    let write = p
        .target()
        .resolved()
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Raise" && p.target().resolved().routine_role(r) == Role::Dependency)
        .expect("precondition: the dependency's Raise is demanded")
        .stable_routine_id
        .clone();
    let plan = || Plan::merge([(JobId(0), explain_plan(BTreeSet::from([write.clone()])))]);
    let (view, _) = p.serve(&plan());
    let warm = effects_text(view.effects(JobId(0)));
    p.target = Some(view.into_target());
    let mut cold_view = View::new(
        crate::engine::gate::run::build_view_model(
            &f.inputs,
            &ViewKey::new(&f.inputs.workspace, false, false),
            SERVED,
            |_| {},
        )
        .unwrap()
        .unwrap()
        .target,
    );
    cold_view.prepare(&plan());
    let cold = effects_text(cold_view.effects(JobId(0)));
    assert!(
        cold.contains("Inherited") || cold.contains("inherited") || cold.contains("ransitive"),
        "precondition: the root's effects carry an inherited fact (its raw half): {cold}"
    );
    assert_eq!(warm, cold);
    assert_eq!(p.fills.compose, 1);
    assert_eq!(
        p.fills.emit_raw, 1,
        "one scoped emission for the dependency root"
    );
}

/// The state's det, its core-summary harvest only (the fields a drained
/// `CoreSummaries` empties), comparable across harvests.
fn core_harvest(det: &DetectorArtifacts) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let unc = det
        .uncertainties_by_node
        .iter()
        .map(|(id, set)| {
            let values: Vec<&_> = det
                .uncertainties
                .elements(*set)
                .iter()
                .map(|u| det.uncertainties.value(*u))
                .collect();
            (id.clone(), format!("{values:?}"))
        })
        .collect();
    let roles = det
        .parameter_roles_by_routine
        .iter()
        .map(|(id, r)| (id.clone(), format!("{r:?}")))
        .collect();
    (unc, roles)
}

/// r3 §4.1 / r4 §11: the det cell is evicted and refilled from the retained
/// `l4` and `CoreSummaries`, any number of times, and every refill equals
/// the first harvest and the cold answers.
///
/// DISCRIMINATION: `drain_core = true` (the harvest takes the core
/// summaries OWNED, drained out of the retained `Arc`, as the one-shot path
/// drains its own) leaves the refill an empty map: it fails at "refill 0:
/// the core harvest" (the parameter roles are gone).
#[test]
fn det_evict_then_refill_reads_the_retained_core_summaries() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let mut p = Prepared::new(&f.inputs, f.single_app);
    let first = p.answer(&*job(Cmd::Prove, &f));
    assert_eq!(first, cold(Cmd::Prove, &f));
    let harvest = core_harvest(p.det.as_ref().unwrap());
    // ws-d37's `Helper(var Customer)` has a core parameter role; it has no
    // summary uncertainty (no to-less call), so the roles carry the proof.
    assert!(
        !harvest.1.is_empty(),
        "precondition: the core summaries carry parameter roles"
    );
    for round in 0..2 {
        p.evict_det();
        let again = p.answer(&*job(Cmd::Digest, &f));
        assert_eq!(
            core_harvest(p.det.as_ref().unwrap()),
            harvest,
            "refill {round}: the core harvest"
        );
        assert_eq!(again, cold(Cmd::Digest, &f), "refill {round}: digest");
        let again = p.answer(&*job(Cmd::Prove, &f));
        assert_eq!(again, first, "refill {round}: prove");
    }
    assert_eq!(p.fills.harvest, 3, "one harvest and two refills");
    assert_eq!(p.fills.l4, 1, "one solve");
    assert_eq!(p.fills.compose, 1, "one composition");
}

/// r4 §11: a digest followed by a seeded L4 update over the same revision
/// reads the retained `CoreSummaries` (`L4Reuse::core`): every SCC is
/// seeded, and the seeded solve's projection equals a cold full solve's.
///
/// DISCRIMINATION: `drain_core = true` empties the retained map in the
/// digest's harvest, and the seeded solve then panics ("seeded …: no
/// baseline core summary").
#[test]
fn digest_then_seeded_l4_update_reuses_the_retained_core() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let mut p = Prepared::new(&f.inputs, f.single_app);
    assert_eq!(p.answer(&*job(Cmd::Digest, &f)), cold(Cmd::Digest, &f));
    let s = p.solved.as_ref().expect("digest solved");
    let target = p.target();
    let cancel = || false;
    let req = L4Request {
        extra_diagnostics: vec![],
        control: Some(crate::engine::l4::summary_runner::L4Control {
            cancel: &cancel,
            reuse: Some(L4Reuse {
                solve: &s.baseline,
                bundle: &s.l4.bundle,
                core: &s.core.0,
            }),
        }),
    };
    let cross = target.cross_app_inputs();
    let (l4, core, outcome) =
        build_l4_controlled(target.resolved(), &p.rel, req, cross.as_ref()).expect("not cancelled");
    let outcome = outcome.expect("a controlled solve");
    assert!(outcome.total > 0, "precondition: SCCs to seed");
    assert_eq!(outcome.seeded, outcome.total, "every SCC seeded");
    let seeded = crate::engine::incremental::L4Projection::of(&l4, &core.0);
    let (cold_l4, cold_core) = build_l4(
        target.resolved(),
        &relation_of(target),
        L4Request {
            extra_diagnostics: vec![],
            control: None,
        },
        cross.as_ref(),
    );
    let cold = crate::engine::incremental::L4Projection::of(&cold_l4, &cold_core.0);
    assert!(
        cold.per_id.values().any(|(s, _)| s.is_some()),
        "precondition: the cold solve has core summaries"
    );
    assert_eq!(seeded, cold);
}

/// The payload a lent harvest clones, cloned the same way: each distinct
/// id's direct facts and coverage (when it has direct facts) and each
/// distinct id's core uncertainties and parameter roles, plus the roles of
/// ids no row reaches.
fn clone_payloads(model: &Model, caps: &CapabilityAnalysis, core: &CoreSummaries) -> usize {
    let mut kept = 0usize;
    let mut seen = std::collections::HashSet::new();
    for r in &model.workspace.routines {
        if !seen.insert(r.id.as_str()) {
            continue;
        }
        if let Some(d) = caps.direct_full.get(&r.id) {
            let pair = (d.clone(), caps.coverage.get(&r.id).cloned());
            kept += pair.0.len();
            std::hint::black_box(pair);
        }
        if let Some(s) = core.0.get(&r.id) {
            let pair = (s.uncertainties.clone(), s.parameter_roles.clone());
            kept += pair.0.len() + pair.1.len();
            std::hint::black_box(pair);
        }
    }
    for (id, s) in &core.0 {
        if !seen.contains(id.as_str()) && !s.parameter_roles.is_empty() {
            std::hint::black_box((id.clone(), s.parameter_roles.clone()));
        }
    }
    kept
}

/// T-phase-alloc (r4 §11, as corrected by review r3 finding 14): the OWNED
/// harvest arm (the one-shot path's) moves the composition's payloads and
/// drains the core summaries; it never copies them. Measured with the lib's
/// thread-gated counting allocator, on one thread: the lent harvest's
/// allocations exceed the owned harvest's by AT LEAST the payload clones
/// themselves (measured by cloning exactly what the lent arm clones), in
/// bytes and in count. The intended `Arc` wrapper is counted explicitly: the
/// owned harvest moves the composition's ONE `Arc<ConeDerivedStore>` into the
/// artifacts (the same allocation, no new wrapper, no copy); the lent one
/// shares it (two handles, one allocation).
///
/// DISCRIMINATION: cloning `direct_full` in the owned arm (`get(..).cloned()`
/// in place of `remove`) raises the owned bytes by the direct facts' clone,
/// and the byte margin falls below the payload ("owned 48498 B, lent 50670
/// B, payload 14897 B: the owned arm copied"; unbroken: owned 35773 B).
#[test]
fn owned_harvest_moves_payloads_and_adds_only_the_counted_arc() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        let target = AnalysisTarget::build(&f.inputs, true, "t-phase-alloc").unwrap();
        let model = target.resolved();
        let rel = relation_of(&target);
        let scope = || ConeScope {
            derived: true,
            raw: WitnessScope::None,
        };
        let (owned_caps, _) = compose_cone(model, &rel, scope());
        let (lent_caps, _) = compose_cone(model, &rel, scope());
        let req = || L4Request {
            extra_diagnostics: vec![],
            control: None,
        };
        let (owned_l4, owned_core) = build_l4(model, &rel, req(), None);
        let (lent_l4, lent_core) = build_l4(model, &rel, req(), None);
        let mask = digest_plan(
            crate::engine::l5::detectors::default_detectors(),
            BTreeSet::new(),
            false,
        )
        .union();
        assert!(mask & substrate::CORE_SUMMARIES != 0 && mask & substrate::SUMMARIES != 0);

        let (kept, n_payload, b_payload) =
            crate::test_alloc::measure(|| clone_payloads(model, &lent_caps, &lent_core));
        assert!(
            kept > 0 && b_payload > 0,
            "precondition: the fixture has payloads to copy"
        );

        let derived_ptr = Arc::as_ptr(&owned_caps.derived);
        let ((owned, _), n_owned, b_owned) = crate::test_alloc::measure(|| {
            dc::harvest_detector_artifacts(
                model,
                mask,
                &rel,
                Some(CapsInput::Owned(owned_caps)),
                L4Input::Solved(&owned_l4, CoreInput::Owned(owned_core)),
                None,
            )
        });
        let ((lent, _), n_lent, b_lent) = crate::test_alloc::measure(|| {
            dc::harvest_detector_artifacts(
                model,
                mask,
                &rel,
                Some(CapsInput::Borrowed(&lent_caps)),
                L4Input::Solved(&lent_l4, CoreInput::Borrowed(&lent_core)),
                None,
            )
        });
        // The counted Arc: moved, never re-wrapped or copied.
        assert_eq!(Arc::as_ptr(&owned.cone_derived), derived_ptr);
        assert_eq!(Arc::strong_count(&owned.cone_derived), 1);
        assert!(Arc::ptr_eq(&lent.cone_derived, &lent_caps.derived));
        assert_eq!(Arc::strong_count(&lent_caps.derived), 2);
        // No payload copy in the owned arm.
        assert!(
            b_lent >= b_owned + b_payload,
            "owned {b_owned} B, lent {b_lent} B, payload {b_payload} B: the owned arm copied"
        );
        assert!(
            n_lent >= n_owned + n_payload,
            "owned {n_owned}, lent {n_lent}, payload {n_payload} allocations"
        );
        eprintln!(
            "[t-phase-alloc] owned={n_owned} allocs/{b_owned} B lent={n_lent}/{b_lent} B \
             payload={n_payload}/{b_payload} B"
        );
        assert_eq!(
            core_harvest(&owned),
            core_harvest(&lent),
            "same harvest either way"
        );
    });
}
