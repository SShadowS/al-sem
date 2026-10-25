//! #222: the incremental L4 host (spec
//! `docs/superpowers/specs/2026-10-25-issue-222-incremental-l4-host-design.md`
//! §4.1). It rebuilds the front end per revision through the step-12 builder
//! (`WorkspaceInputs::read`, `AnalysisTarget::build`, a `View` filled with
//! `rel` and `l4` only) and seeds every SCC the invalidation rule (§4.3)
//! leaves clean from the last committed revision. There is no second engine.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::engine::batch::job::ViewUniverse;
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::analysis::{CoreSummaries, L4Analysis};
use crate::engine::l4::effect_store::{EffectSetId, SummaryBundle};
use crate::engine::l4::scc::SccResult;
use crate::engine::l4::summary::{DbEffect, RoutineSummary};
use crate::engine::l4::summary_runner::{
    Cancelled, L4Control, L4Reuse, SolveBaseline, SummarizeDiagnostic, sha256_of_debug,
};
use crate::engine::session::View;
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::workspace::Model;
use crate::program::resolve::semantic_golden::dependency_closure_digest;

/// The `cancel` checks a refresh makes before the solve's first SCC turn:
/// after the inputs, after the target, after the relation (§4.2).
#[doc(hidden)]
pub const PHASE_CHECKS: usize = 3;

/// The incremental L4 host of one workspace root, in one universe.
pub struct IncrementalHost {
    workspace: PathBuf,
    universe: ViewUniverse,
    /// The last COMMITTED revision only.
    last: Option<Baseline>,
    /// A completed refresh, until [`Refresher::commit`].
    staged: Option<Baseline>,
}

/// What the host keeps from a revision (§4.4): no model, program or relation.
struct Baseline {
    /// §4.3 R0, host half.
    host_global: [u8; 32],
    solve: SolveBaseline,
    l4: L4Analysis,
    core: CoreSummaries,
}

/// Why a refresh stopped. The committed baseline is untouched in both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshError {
    Cancelled,
    Build(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshKind {
    Full(FullReason),
    Incremental,
}

/// Which half of R0 made a refresh Full (or that there was no baseline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullReason {
    NoBaseline,
    HostGlobal,
    SolveGlobal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub kind: RefreshKind,
    /// The re-solved SCCs, each as its members' sorted signature labels
    /// (`Object.[Member.]Routine(var T1; T2): R`).
    pub resolved: BTreeSet<Vec<String>>,
    pub seeded: usize,
    pub total: usize,
}

/// What the worker drives (§4.2); the host implements it.
pub trait Refresher {
    /// Build the next revision into `staged`. `cancel` is polled between
    /// phases and between SCCs. On cancel or error, the committed baseline is
    /// untouched and `staged` is cleared.
    fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError>;
    /// Move `staged` into the committed baseline.
    fn commit(&mut self);
}

impl IncrementalHost {
    pub fn new(workspace: &Path, universe: ViewUniverse) -> Self {
        IncrementalHost {
            workspace: workspace.to_path_buf(),
            universe,
            last: None,
            staged: None,
        }
    }

    /// `refresh` then `commit`: what the oracle tests call.
    pub fn refresh_committed(
        &mut self,
        cancel: &dyn Fn() -> bool,
    ) -> Result<RefreshReport, RefreshError> {
        let report = self.refresh(cancel)?;
        self.commit();
        Ok(report)
    }

    /// The committed baseline's projection (§4.7).
    pub fn projection(&self) -> Option<L4Projection> {
        self.last
            .as_ref()
            .map(|b| L4Projection::of(&b.l4, &b.core.0))
    }

    /// sha256 over the committed baseline's host global, solve global and
    /// row digests (T3).
    #[doc(hidden)]
    pub fn baseline_fingerprint(&self) -> Option<[u8; 32]> {
        self.last.as_ref().map(|b| {
            let rows: BTreeMap<_, _> = b.solve.rows.iter().collect();
            sha256_of_debug(&(b.host_global, b.solve.solve_global, rows))
        })
    }
}

impl Refresher for IncrementalHost {
    fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError> {
        self.staged = None;
        let cancelled = |_: Cancelled| RefreshError::Cancelled;
        // §4.1 step 1: before the build, so a `.app` change that slips in
        // makes the NEXT digest differ, never a false reuse.
        let closure = dependency_closure_digest(&self.workspace).map_err(RefreshError::Build)?;
        let inputs = WorkspaceInputs::read(&self.workspace, &WorkspaceNeeds::default());
        let host_global = sha256_of_debug(&(
            self.universe,
            &closure,
            inputs.app_json.bytes.as_ref().ok(),
            &inputs.model_instance_id,
        ));
        if cancel() {
            return Err(RefreshError::Cancelled);
        }
        let single_app = self.universe == ViewUniverse::SingleApp;
        let target =
            AnalysisTarget::build(&inputs, single_app, "l4-host").map_err(RefreshError::Build)?;
        if cancel() {
            return Err(RefreshError::Cancelled);
        }
        let mut view = View::new(target);
        let base = self.last.as_ref().filter(|b| b.host_global == host_global);
        let control = L4Control {
            cancel,
            reuse: base.map(|b| L4Reuse {
                solve: &b.solve,
                bundle: &b.l4.bundle,
                core: &b.core.0,
            }),
        };
        let (core, outcome) = view.prepare_l4_seeded(control).map_err(cancelled)?;
        let kind = match (&self.last, base) {
            (None, _) => RefreshKind::Full(FullReason::NoBaseline),
            (Some(_), None) => RefreshKind::Full(FullReason::HostGlobal),
            (Some(_), Some(_)) if outcome.full => RefreshKind::Full(FullReason::SolveGlobal),
            _ => RefreshKind::Incremental,
        };
        let label = labels(view.model());
        let resolved = outcome
            .resolved
            .iter()
            .map(|key| {
                let mut l: Vec<String> = key
                    .iter()
                    .map(|id| {
                        label
                            .get(id.as_str())
                            .cloned()
                            .unwrap_or_else(|| id.clone())
                    })
                    .collect();
                l.sort();
                l
            })
            .collect();
        let l4 = view.take_l4();
        drop(view);
        self.staged = Some(Baseline {
            host_global,
            solve: outcome.baseline,
            l4,
            core,
        });
        Ok(RefreshReport {
            kind,
            resolved,
            seeded: outcome.seeded,
            total: outcome.total,
        })
    }

    fn commit(&mut self) {
        if let Some(b) = self.staged.take() {
            self.last = Some(b);
        }
    }
}

/// Each routine id's signature label, `Object.[Member.]Routine(var T1; T2): R`:
/// the fields `compute_routine_id` hashes, so two overloads never share one.
/// A collided id takes its first row's.
fn labels(model: &Model) -> HashMap<&str, String> {
    let ws = &model.workspace;
    let object: HashMap<&str, &str> = ws
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();
    let mut out = HashMap::new();
    for r in &ws.routines {
        out.entry(r.id.as_str()).or_insert_with(|| {
            let params: Vec<String> = r
                .parameters
                .iter()
                .map(|p| format!("{}{}", if p.is_var { "var " } else { "" }, p.type_text))
                .collect();
            format!(
                "{}.{}{}({}){}",
                object.get(r.object_id.as_str()).copied().unwrap_or("?"),
                r.enclosing_member
                    .as_ref()
                    .map(|m| format!("{m}."))
                    .unwrap_or_default(),
                r.name,
                params.join("; "),
                r.return_type
                    .as_ref()
                    .map(|t| format!(": {t}"))
                    .unwrap_or_default(),
            )
        });
    }
    out
}

/// §4.7's projection: per id the core summary and its owned db effects
/// (`None` without a bundle row), the `terminal_base` partition as sorted id
/// groups, the diagnostics and warnings, and the SCC list with its flags. No
/// `EffectId` or `EffectSetId` number appears in it.
#[derive(Debug, PartialEq)]
pub struct L4Projection {
    pub(crate) per_id: BTreeMap<String, (Option<RoutineSummary>, Option<Vec<DbEffect>>)>,
    terminal_groups: Vec<Vec<String>>,
    diagnostics: Vec<SummarizeDiagnostic>,
    warnings: Vec<String>,
    sccs: Vec<(Vec<String>, bool)>,
}

impl L4Projection {
    pub fn of(l4: &L4Analysis, core: &HashMap<String, RoutineSummary>) -> L4Projection {
        let mut p = l4_projection(&l4.bundle, core, &l4.summarize_diagnostics, &l4.scc);
        p.warnings = l4.warnings.clone();
        p
    }

    /// The projection without the core summaries: only the ids with a bundle
    /// row, each with its db effects (T0b's comparison with a `query` plan
    /// view, whose core summaries `View::prepare` drops).
    pub fn without_core(&self) -> L4Projection {
        L4Projection {
            per_id: self
                .per_id
                .iter()
                .filter(|(_, (_, e))| e.is_some())
                .map(|(id, (_, e))| (id.clone(), (None, e.clone())))
                .collect(),
            terminal_groups: self.terminal_groups.clone(),
            diagnostics: self.diagnostics.clone(),
            warnings: self.warnings.clone(),
            sccs: self.sccs.clone(),
        }
    }
}

/// [`L4Projection`] of a bare solve (no warnings: `build_l4` renders them).
pub(crate) fn l4_projection(
    bundle: &SummaryBundle,
    core: &HashMap<String, RoutineSummary>,
    diagnostics: &[SummarizeDiagnostic],
    scc: &SccResult,
) -> L4Projection {
    let row_of = |id: &str| bundle.routine_ix(id).filter(|&ix| bundle.has_row(ix));
    let mut ids: BTreeSet<String> = core.keys().cloned().collect();
    ids.extend(
        bundle
            .routines_with_rows()
            .map(|ix| bundle.routine_id(ix).to_string()),
    );
    let per_id = ids
        .into_iter()
        .map(|id| {
            let effects =
                row_of(&id).map(|ix| bundle.db_effects(ix).map(|e| e.to_owned()).collect());
            let summary = core.get(&id).cloned();
            (id, (summary, effects))
        })
        .collect();
    let mut groups: BTreeMap<EffectSetId, Vec<String>> = BTreeMap::new();
    for ix in bundle.routines_with_rows() {
        let set = bundle.terminal_base(ix).expect("a row has a terminal base");
        groups
            .entry(set)
            .or_default()
            .push(bundle.routine_id(ix).to_string());
    }
    let mut terminal_groups: Vec<Vec<String>> = groups
        .into_values()
        .map(|mut g| {
            g.sort();
            g
        })
        .collect();
    terminal_groups.sort();
    L4Projection {
        per_id,
        terminal_groups,
        diagnostics: diagnostics.to_vec(),
        warnings: Vec::new(),
        sccs: scc
            .sccs
            .iter()
            .map(|s| (s.members.clone(), s.recursive))
            .collect(),
    }
}
