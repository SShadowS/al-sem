//! #160 step 2 (spec §4.4.3): the `View` a command builds its detector runs
//! and its L4 analysis on.
//!
//! A `View` owns its target and a few cells (`rel`, `l4`, `det`,
//! `ordering`), each filled at most once, by [`View::prepare`] alone, from a
//! [`Plan`] that only this module's factories build. No interior mutability:
//! every fill is on `&mut self`, every reader takes `&self` (rule 4).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::analysis::{L4Analysis, L4Request, build_l4};
use crate::engine::l4::combined_graph::PrimaryGraph;
use crate::engine::l4::exec_relation::ExecRelation;
use crate::engine::l4::summary_runner::SummarizeDiagnostic;
use crate::engine::l5::detector_context::{
    self as dc, DetectorArtifacts, DetectorContext, OrderingFactsMap,
};
use crate::engine::l5::registry::{self, Detector, RunOutput, substrate};
use crate::engine::perf_trace as pt;
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
    l4: Option<L4Analysis>,
    det: Option<DetectorArtifacts>,
    /// One bit per [`Cell`] released; a released cell is never refilled.
    released: u8,
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
    runs: Vec<Vec<Detector>>,
    scope_primary: bool,
    need_l4: bool,
    /// The substrate bits a post-run projection reads off `det`: digest and
    /// prove `TRANSACTION_SPANS` (the tx map), events `SUMMARIES` (the fanout
    /// report's coverage read). `prepare` asserts every run's union holds
    /// them, naming the missing bit (A2.10).
    after_run: u32,
}

/// The cells [`View::release`] drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Det,
    L4,
    Ordering,
    Rel,
}

impl Cell {
    const ALL: [Cell; 4] = [Cell::Det, Cell::L4, Cell::Ordering, Cell::Rel];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// `alsem analyze`: the selected detectors.
pub fn analyze_plan(detectors: Vec<Detector>, scope_primary: bool) -> Plan {
    Plan {
        runs: vec![detectors],
        scope_primary,
        need_l4: false,
        after_run: 0,
    }
}

/// `alsem digest` and `alsem prove`: the run, then the tx map off
/// `det.transaction_spans` and the query off `l4`.
pub fn digest_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![detectors],
        scope_primary: true,
        need_l4: true,
        after_run: substrate::TRANSACTION_SPANS,
    }
}

/// One run at primary scope, with nothing read after it: fingerprint,
/// `diff --strict` and policy check.
fn run_only_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![detectors],
        scope_primary: true,
        need_l4: false,
        after_run: 0,
    }
}

/// `alsem fingerprint`.
pub fn fingerprint_plan(detectors: Vec<Detector>) -> Plan {
    run_only_plan(detectors)
}

/// `alsem diff --strict`, per side.
pub fn diff_strict_plan(detectors: Vec<Detector>) -> Plan {
    run_only_plan(detectors)
}

/// `alsem events fanout` / `chains`: the run, then the report off
/// `det.summaries`.
pub fn events_plan(detectors: Vec<Detector>) -> Plan {
    Plan {
        runs: vec![detectors],
        scope_primary: true,
        need_l4: false,
        after_run: substrate::SUMMARIES,
    }
}

/// `alsem policy check`.
pub fn policy_plan(detectors: Vec<Detector>) -> Plan {
    run_only_plan(detectors)
}

/// `alsem query`: no run, `l4` alone.
pub fn query_plan() -> Plan {
    Plan {
        runs: vec![],
        scope_primary: true,
        need_l4: true,
        after_run: 0,
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
            runs,
            scope_primary,
            need_l4,
            after_run,
        }
    }

    /// The key of each run, in plan order.
    pub fn keys(&self) -> Vec<RunKey> {
        self.runs.iter().map(|r| self.key(r)).collect()
    }

    fn key(&self, run: &[Detector]) -> RunKey {
        RunKey {
            detectors: run.iter().map(|d| d.name.clone()).collect(),
            demanded: mask(run),
            scope_primary: self.scope_primary,
        }
    }

    fn union(&self) -> u32 {
        self.runs.iter().map(|r| mask(r)).fold(0, |a, m| a | m)
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
        substrate::RAW_INHERITED_FACTS => "RAW_INHERITED_FACTS",
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

impl View {
    pub fn new(target: AnalysisTarget) -> View {
        View {
            target,
            rel: None,
            l4: None,
            det: None,
            released: 0,
            ordering: None,
            runs: BTreeMap::new(),
        }
    }

    /// Fill every cell `plan` needs that is still empty, in order: ordering,
    /// rel, det (its cone, then `l4`, then its harvest), then the runs. A
    /// plan with no run: rel, then `l4` alone. Rule 1's asserts (a), (b),
    /// (c) and the `after_run` assertion hold in release builds too.
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
        let need_rel = need_det || need_l4;
        // (a) a released cell is never refilled: a refill after
        // `release(Det)` would find no `CoreSummaries`.
        for (cell, needed) in [
            (Cell::Ordering, need_ordering),
            (Cell::Rel, need_rel),
            (Cell::Det, need_det),
            (Cell::L4, need_l4),
        ] {
            assert!(
                !needed || self.released & cell.bit() == 0,
                "{cell:?} released; a View fills each cell once"
            );
        }
        let missing = plan.after_run & !union;
        assert!(
            missing == 0,
            "the plan reads {} off det after its run, but no run demands it",
            bit_names(missing)
        );
        assert!(
            union & substrate::RAW_INHERITED_FACTS == 0,
            "a det cell never carries RAW_INHERITED_FACTS"
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
            l4,
            det,
            ordering,
            runs,
            ..
        } = self;
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
            let (art, solved) =
                dc::build_detector_artifacts(model, union, r, req, target.cross_app_inputs());
            *det = Some(art);
            if need_l4 {
                *l4 = solved;
            }
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
        for run in &plan.runs {
            let key = plan.key(run);
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
            let out =
                registry::finish_run(model, &ctx, run, cross_base, plan.scope_primary, summarize);
            runs.insert(key, Arc::new(out));
        }
    }

    /// Drop a cell for good: a later `prepare` that needs it panics. Each
    /// release is one `view.release` trace instant naming the cell, so a CLI
    /// test can see when a command drops what.
    pub fn release(&mut self, cell: Cell) {
        pt::instant_lazy(
            "view",
            "view.release",
            || serde_json::json!({ "cell": format!("{cell:?}") }),
        );
        self.released |= cell.bit();
        match cell {
            Cell::Det => self.det = None,
            Cell::L4 => self.l4 = None,
            Cell::Ordering => self.ordering = None,
            Cell::Rel => self.rel = None,
        }
    }

    /// [`View::release`] every cell except `keep`: the one way a command
    /// drops what it no longer reads, so no cell (`Ordering` included) is
    /// kept alive by a forgotten line.
    pub fn release_all_but(&mut self, keep: &[Cell]) {
        for cell in Cell::ALL {
            if !keep.contains(&cell) {
                self.release(cell);
            }
        }
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
    /// `after_run: 0` in `digest_plan`, separately in `events_plan`, fails
    /// that case.
    #[test]
    fn command_plans_declare_after_run() {
        for (name, plan, bit) in [
            (
                "digest",
                digest_plan(vec![probe(0, noop)]),
                "TRANSACTION_SPANS",
            ),
            ("events", events_plan(vec![probe(0, noop)]), "SUMMARIES"),
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

    /// Rule 1: a det cell never carries the RAW bit. DISCRIMINATION: deleting
    /// the RAW assert fails this (no panic).
    #[test]
    #[should_panic(expected = "a det cell never carries RAW_INHERITED_FACTS")]
    fn a_det_cell_never_carries_raw_inherited_facts() {
        let plan = Plan::for_test(
            vec![vec![probe(
                substrate::SUMMARIES | substrate::RAW_INHERITED_FACTS,
                noop,
            )]],
            true,
            false,
            0,
        );
        view().prepare(&plan);
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
            Cell::Det.bit() | Cell::Ordering.bit() | Cell::Rel.bit()
        );
    }

    /// A2.10's "naming the missing bit" for every bit. DISCRIMINATION:
    /// deleting one arm of `substrate_bit_name` fails this.
    #[test]
    fn every_substrate_bit_has_a_name() {
        let all = substrate::ALL
            | substrate::RAW_INHERITED_FACTS
            | substrate::DB_EFFECT_REVERSE_INDEX
            | substrate::ORDERING_FACTS;
        assert_eq!(all.count_ones(), 7, "precondition: seven bits");
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
