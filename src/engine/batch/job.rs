//! #277 D1: a command cut into the parts the batch driver calls. A job names
//! the views it reads (`demands`), states per view what it needs from the one
//! prepared plan (`resolve`, after the build, D4.2), computes off the prepared
//! view (`compute`), and turns its sides into one document (`finalize`) and
//! that document into ordered writes (`serialize`, D2.3).

use std::any::Any;
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use crate::engine::batch::output::JobOutput;
use crate::engine::gate::run::{ModelFailure, TargetFailure};
use crate::engine::l5::digest_cli::DigestEffects;
use crate::engine::l5::registry::RunOutput;
use crate::engine::l5::snapshot::{SnapshotIdentityTable, derive_identity_table};
use crate::engine::session::{Cell, JobId, Plan, RunKey, TreeReadOut, View};
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::workspace::Model;
use crate::program::resolve::full::FreshCoverage;
use crate::program::resolved::ResolvedProgram;

/// A job's side: its index among the job's demands (diff: old 0, new 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideIx(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewUniverse {
    CrossApp,
    SingleApp,
}

/// One view per key per batch (D4.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewKey {
    /// `std::path::absolute`, not canonical: two spellings that absolutise
    /// differently are two views (stated).
    pub workspace: PathBuf,
    pub universe: ViewUniverse,
    pub no_roots_config: bool,
}

/// A workspace path as a view key and the reader spell it (D4.1):
/// `std::path::absolute`, never canonical.
fn absolute(workspace: &Path) -> PathBuf {
    std::path::absolute(workspace).unwrap_or_else(|_| workspace.to_path_buf())
}

impl ViewKey {
    pub fn new(workspace: &Path, single_app: bool, no_roots_config: bool) -> ViewKey {
        ViewKey {
            workspace: absolute(workspace),
            universe: if single_app {
                ViewUniverse::SingleApp
            } else {
                ViewUniverse::CrossApp
            },
            no_roots_config,
        }
    }
}

/// What a demand needs from its view's ONE builder (D4.1); a view's needs are
/// the union of its demands'.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewNeeds {
    /// `alsem analyze`'s extras: the program coverage (`fresh`), the physical
    /// rows and the object-fact census.
    pub analyze_extras: bool,
    /// The program, lent before the model is built (D9): `alsem graph`.
    pub graph: bool,
    /// The model: every job but graph.
    pub model: bool,
}

impl ViewNeeds {
    pub const MODEL: ViewNeeds = ViewNeeds {
        analyze_extras: false,
        graph: false,
        model: true,
    };
    pub const ANALYZE: ViewNeeds = ViewNeeds {
        analyze_extras: true,
        ..ViewNeeds::MODEL
    };
    pub const GRAPH: ViewNeeds = ViewNeeds {
        analyze_extras: false,
        graph: true,
        model: false,
    };

    pub fn union(self, other: ViewNeeds) -> ViewNeeds {
        ViewNeeds {
            analyze_extras: self.analyze_extras || other.analyze_extras,
            graph: self.graph || other.graph,
            model: self.model || other.model,
        }
    }
}

/// One view a job reads.
pub struct ViewDemand {
    pub key: ViewKey,
    pub side: SideIx,
    pub needs: ViewNeeds,
    /// The workspace's one read ([`WorkspaceReader`]); every demand on a view
    /// holds the same read.
    pub inputs: Arc<WorkspaceInputs>,
}

impl ViewDemand {
    pub fn new(
        key: ViewKey,
        side: SideIx,
        needs: ViewNeeds,
        inputs: Arc<WorkspaceInputs>,
    ) -> ViewDemand {
        ViewDemand {
            key,
            side,
            needs,
            inputs,
        }
    }

    /// The demands of a job of one side (#277 E12-C8): side 0 on the view of
    /// its workspace read, in its universe. The read is handed to the driver
    /// ([`WorkspaceRead::hand_off`]).
    pub fn one(
        inputs: &dyn WorkspaceRead,
        single_app: bool,
        no_roots_config: bool,
        needs: ViewNeeds,
    ) -> Vec<ViewDemand> {
        let inputs = inputs.hand_off();
        let key = ViewKey::new(&inputs.workspace, single_app, no_roots_config);
        vec![ViewDemand::new(key, SideIx(0), needs, inputs)]
    }
}

/// A job's hold on its workspace read (#277 R7-1, D6.3). A job derives what
/// it needs from the read when it is made ([`get`](Self::get)), hands the
/// read to the driver in `demands` ([`hand_off`](Self::hand_off)), and from
/// then on reads it only through [`ViewCx::inputs`]. The driver drops each
/// read after its last view, so in a batch over several workspaces a read
/// does not outlive its views.
pub trait WorkspaceRead {
    /// The read, while the job holds it.
    fn get(&self) -> Arc<WorkspaceInputs>;
    /// The read, for the job's demand.
    fn hand_off(&self) -> Arc<WorkspaceInputs>;
}

/// A library caller's read: the caller keeps its own `Arc`, as a standalone
/// run of one workspace may.
impl WorkspaceRead for Arc<WorkspaceInputs> {
    fn get(&self) -> Arc<WorkspaceInputs> {
        Arc::clone(self)
    }
    fn hand_off(&self) -> Arc<WorkspaceInputs> {
        Arc::clone(self)
    }
}

/// The binary's read: given up to the driver by `hand_off`, so nothing but
/// the driver holds it once the batch runs.
pub struct HandOff(RefCell<Option<Arc<WorkspaceInputs>>>);

impl HandOff {
    pub fn new(inputs: Arc<WorkspaceInputs>) -> HandOff {
        HandOff(RefCell::new(Some(inputs)))
    }
}

impl WorkspaceRead for HandOff {
    fn get(&self) -> Arc<WorkspaceInputs> {
        self.0
            .borrow()
            .clone()
            .expect("a job reads its workspace through ViewCx::inputs once the driver holds it")
    }
    fn hand_off(&self) -> Arc<WorkspaceInputs> {
        self.0
            .borrow_mut()
            .take()
            .expect("a job hands its read off once")
    }
}

/// One `WorkspaceInputs` per workspace path (D3; plan Task 7 Step 2b): every
/// job of a batch reads its workspace through this, so jobs on one path share
/// one `Arc`, read with the union of every job's needs on that path, stated
/// before the first read. A read happens on the first ask, so a standalone
/// command reads where it always did. The reader keeps each read alive only
/// while the jobs are made, so a job that gives its read up early (one that
/// fails before its view) never makes the next job read it again (#277 final
/// r2 R2-5); after [`made`](Self::made) the jobs and then the driver hold
/// them, and the driver drops each after its last view (#277 E12-C2).
#[derive(Default)]
pub struct WorkspaceReader {
    planned: BTreeMap<PathBuf, WorkspaceNeeds>,
    read: RefCell<BTreeMap<PathBuf, (WorkspaceNeeds, Weak<WorkspaceInputs>)>>,
    making: RefCell<Vec<Arc<WorkspaceInputs>>>,
}

impl WorkspaceReader {
    /// A reader for the jobs that will read each `(workspace, needs)`.
    pub fn new(reads: impl IntoIterator<Item = (PathBuf, WorkspaceNeeds)>) -> WorkspaceReader {
        let mut planned = BTreeMap::new();
        for (path, needs) in reads {
            planned
                .entry(absolute(&path))
                .and_modify(|n: &mut WorkspaceNeeds| *n = n.union(needs))
                .or_insert(needs);
        }
        WorkspaceReader {
            planned,
            ..WorkspaceReader::default()
        }
    }

    /// Every job is made: the reader gives its reads up to them.
    pub fn made(&self) {
        self.making.borrow_mut().clear();
    }

    /// `workspace`'s one read, holding at least `needs`.
    pub fn read(&self, workspace: &Path, needs: WorkspaceNeeds) -> Arc<WorkspaceInputs> {
        let key = absolute(workspace);
        let mut read = self.read.borrow_mut();
        if let Some((held, inputs)) = read.get(&key)
            && let Some(inputs) = inputs.upgrade()
        {
            assert_eq!(
                held.union(needs),
                *held,
                "a workspace read without a job's needs: {}",
                key.display()
            );
            return inputs;
        }
        let needs = self.planned.get(&key).map_or(needs, |p| p.union(needs));
        let inputs = WorkspaceInputs::read(workspace, &needs);
        read.insert(key, (needs, Arc::downgrade(&inputs)));
        self.making.borrow_mut().push(Arc::clone(&inputs));
        inputs
    }
}

/// What one demand needs from its view's ONE plan (D5): its own plan (a
/// factory's; `Plan::merge` merges the view's parts), the cells it reads after
/// `prepare` (D6.1: a cell a later demand reads is not released before it),
/// and what `resolve` hands to `compute`.
pub struct PlanPart {
    pub plan: Plan,
    pub reads: Vec<Cell>,
    pub carry: Option<Box<dyn Any + Send>>,
}

/// A job's failure on one side: the writes (a stderr line) and the exit it
/// ends with, a [`JobOutput`] (#277 E12-C7: one shape, not two).
pub type JobFailure = JobOutput;

/// One side's computed document; only its job reads it (no shared shape
/// across commands). `bytes`: what it holds, for the `batch.held` census
/// (D6.3); only a side that can wait for a later view (diff's) states it.
pub struct SideDoc {
    doc: Box<dyn Any + Send>,
    bytes: u64,
}

impl SideDoc {
    pub fn new<T: Any + Send>(doc: T) -> SideDoc {
        SideDoc::held(doc, 0)
    }

    /// A side document of `bytes` bytes.
    pub fn held<T: Any + Send>(doc: T, bytes: u64) -> SideDoc {
        SideDoc {
            doc: Box::new(doc),
            bytes,
        }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn take<T: Any>(self) -> T {
        *self.doc.downcast().expect("a side document of its own job")
    }
}

/// A finished job's document, before it is serialized.
pub struct Doc(Box<dyn Any + Send>);

impl Doc {
    pub fn new<T: Any + Send>(doc: T) -> Doc {
        Doc(Box::new(doc))
    }

    pub fn take<T: Any>(self) -> T {
        *self.0.downcast().expect("a document of its own job")
    }

    /// A one-side job's document ([`Job::finalize`]'s default): its side, or
    /// the side's failure.
    pub fn one<T: Any>(self) -> Result<T, JobFailure> {
        self.take::<Result<SideDoc, JobFailure>>()
            .map(SideDoc::take)
    }
}

/// What `resolve` reads: the view's model and workspace read, and its
/// identity table, derived once per view on the first ask and dropped when
/// the resolve step ends (D4.2 (3)).
pub struct ResolveCx<'a> {
    model: &'a Model,
    inputs: &'a Arc<WorkspaceInputs>,
    ids: OnceCell<SnapshotIdentityTable>,
}

impl<'a> ResolveCx<'a> {
    pub(crate) fn new(model: &'a Model, inputs: &'a Arc<WorkspaceInputs>) -> ResolveCx<'a> {
        ResolveCx {
            model,
            inputs,
            ids: OnceCell::new(),
        }
    }

    pub fn model(&self) -> &'a Model {
        self.model
    }

    /// The view's workspace read (#277 R7-1: the job handed its own off).
    pub fn inputs(&self) -> &'a Arc<WorkspaceInputs> {
        self.inputs
    }

    pub fn ids(&self) -> &SnapshotIdentityTable {
        self.ids.get_or_init(|| derive_identity_table(self.model))
    }
}

/// Why a view built no model; each job words it as its own command does
/// (D4.1): `target` as `AnalysisTarget`'s callers word it, `analyze` as
/// `alsem analyze` classifies it, with its program coverage (`Some` on a view
/// with analyze's extras). `inputs`: the view's read, which analyze's empty
/// report reads (the job handed its own off, #277 R7-1).
pub struct ViewFailure {
    pub target: TargetFailure,
    pub analyze: Option<Box<(Result<FreshCoverage, String>, ModelFailure)>>,
    pub inputs: Arc<WorkspaceInputs>,
}

impl std::fmt::Debug for ViewFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewFailure")
            .field("target", &self.target)
            .field("analyze", &self.analyze)
            .field("workspace", &self.inputs.workspace)
            .finish()
    }
}

/// One demand's access to its prepared view. Releases go through the driver
/// (D6.1, D6.2): a cell or run that a later demand on the view reads stays.
pub struct ViewCx<'a> {
    pub(crate) view: &'a mut View,
    pub(crate) inputs: &'a Arc<WorkspaceInputs>,
    pub(crate) fresh: Option<&'a Result<FreshCoverage, String>>,
    pub(crate) job: JobId,
    pub(crate) keys: Vec<RunKey>,
    pub(crate) has_tree: bool,
    pub(crate) later_cells: Vec<Cell>,
    pub(crate) later_runs: Vec<RunKey>,
    pub(crate) carry: Option<Box<dyn Any + Send>>,
    /// Every run handed to a job on the view, under `ALSEM_RETAINED_CENSUS=1`
    /// only (D11): `Weak`, so the allocation's address stays unique while the
    /// view's census counts each distinct one once.
    pub(crate) handed: &'a mut Vec<HandedRun>,
}

/// One run handed to a job (D11): the job, the allocation, its bytes.
pub(crate) type HandedRun = (JobId, Weak<RunOutput>, usize);

impl ViewCx<'_> {
    pub fn view(&self) -> &View {
        self.view
    }

    /// The view's workspace read.
    pub fn inputs(&self) -> &Arc<WorkspaceInputs> {
        self.inputs
    }

    /// The program coverage of an analyze view.
    pub fn fresh(&self) -> &Result<FreshCoverage, String> {
        self.fresh.expect("the analyze builder's view")
    }

    /// This demand's run keys, in its plan's order.
    pub fn keys(&self) -> &[RunKey] {
        &self.keys
    }

    /// Whether `prepare` composed this job's tree.
    pub fn has_tree(&self) -> bool {
        self.has_tree
    }

    /// What this job's `resolve` handed on.
    pub fn take_carry<T: Any>(&mut self) -> T {
        *self
            .carry
            .take()
            .expect("resolve's carry")
            .downcast()
            .expect("the carry of its own job")
    }

    /// This job's tree effects (D5.4).
    pub fn take_effects(&mut self) -> DigestEffects {
        self.view.take_effects(self.job)
    }

    /// This job's tree read (#160 step 9 D5; #277 D5.4).
    pub fn take_tree_read(&mut self) -> TreeReadOut {
        self.view.take_tree_read(self.job)
    }

    /// The run's output; the view's own copy is dropped unless a later demand
    /// reads it. Always the view's one allocation, never a copy (D11).
    pub fn take_run(&mut self, key: &RunKey) -> Arc<RunOutput> {
        let run = if self.later_runs.contains(key) {
            self.view.run(key)
        } else {
            self.view.take_run(key)
        };
        if crate::engine::census::enabled() {
            let bytes = run.census_tally().bytes;
            self.handed.push((self.job, Arc::downgrade(&run), bytes));
        }
        run
    }

    /// [`View::release`], unless a later demand reads `cell`.
    pub fn release(&mut self, cell: Cell) {
        if !self.later_cells.contains(&cell) {
            self.view.release(cell);
        }
    }

    /// [`View::release_all_but`], keeping what a later demand reads too.
    pub fn release_all_but(&mut self, keep: &[Cell]) {
        let keep: Vec<Cell> = keep.iter().chain(&self.later_cells).copied().collect();
        self.view.release_all_but(&keep);
    }
}

/// A command as the driver runs it (D1).
pub trait Job {
    /// The command, for the job's trace spans (`job.<i>.<name>`, D10).
    fn name(&self) -> &'static str;
    /// A span a standalone run opens around its whole driver run (analyze's
    /// `analyze.total`, which `scripts/trace_summary.py` reads); none in a
    /// batch, where it would cover the other jobs.
    fn standalone_span(&self) -> Option<&'static str> {
        None
    }
    /// One per view the job reads; empty for a job with no view.
    fn demands(&self) -> Vec<ViewDemand>;
    /// Per view, after its build (D4.2): this demand's part of the plan.
    fn resolve(&self, side: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure>;
    /// Off the prepared view.
    fn compute(&self, side: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure>;
    /// The view did not build: this command's own text and exit.
    fn view_failed(&self, side: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure>;
    /// A graph demand (D9): off the view's program, or its build error, lent
    /// before the model is built. Its side is done here.
    fn program(
        &self,
        _side: SideIx,
        _program: Result<&ResolvedProgram, &str>,
    ) -> Result<SideDoc, JobFailure> {
        unreachable!("a job that reads no program")
    }
    /// After the job's last view. The default: a job of at most one side.
    fn finalize(&self, sides: Vec<Result<SideDoc, JobFailure>>) -> Doc {
        let mut sides = sides.into_iter();
        let side = sides.next().expect("a one-side job's side");
        assert!(sides.next().is_none(), "a one-side job");
        Doc::new(side)
    }
    fn serialize(&self, doc: Doc) -> JobOutput;
    /// The exit of a failed write (analyze 2, every other command 1).
    fn write_failure_exit(&self) -> u8 {
        1
    }
}

/// A job that failed its own validation: no view, its output already made,
/// and its command's write-failure exit (analyze 2, every other command 1).
pub struct Failed {
    pub output: JobOutput,
    pub write_failure_exit: u8,
}

impl Job for Failed {
    fn name(&self) -> &'static str {
        "failed"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        vec![]
    }
    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        unreachable!("a job with no view")
    }
    fn compute(&self, _: SideIx, _: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        unreachable!("a job with no view")
    }
    fn view_failed(&self, _: SideIx, _: &ViewFailure) -> Result<SideDoc, JobFailure> {
        unreachable!("a job with no view")
    }
    fn finalize(&self, _: Vec<Result<SideDoc, JobFailure>>) -> Doc {
        Doc::new(())
    }
    fn serialize(&self, _: Doc) -> JobOutput {
        self.output.clone()
    }
    fn write_failure_exit(&self) -> u8 {
        self.write_failure_exit
    }
}
