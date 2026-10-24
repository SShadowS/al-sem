//! #277 D1: a command cut into the parts the batch driver calls. A job names
//! the views it reads (`demands`), states per view what it needs from the one
//! prepared plan (`resolve`, after the build, D4.2), computes off the prepared
//! view (`compute`), and turns its sides into one document (`finalize`) and
//! that document into ordered writes (`serialize`, D2.3).

use std::any::Any;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::run::{ModelFailure, TargetFailure};
use crate::engine::l5::digest_cli::DigestEffects;
use crate::engine::l5::registry::RunOutput;
use crate::engine::l5::snapshot::{SnapshotIdentityTable, derive_identity_table};
use crate::engine::session::{Cell, JobId, Plan, RunKey, View};
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::workspace::Model;
use crate::program::resolve::full::FreshCoverage;

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

impl ViewKey {
    pub fn new(workspace: &Path, single_app: bool, no_roots_config: bool) -> ViewKey {
        ViewKey {
            workspace: std::path::absolute(workspace).unwrap_or_else(|_| workspace.to_path_buf()),
            universe: if single_app {
                ViewUniverse::SingleApp
            } else {
                ViewUniverse::CrossApp
            },
            no_roots_config,
        }
    }
}

/// What a demand needs from its view's build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewNeeds {
    /// `alsem analyze`'s builder (its program coverage, `fresh`).
    pub analyze_extras: bool,
    /// The program, for a graph job (D9).
    pub graph: bool,
    pub workspace: WorkspaceNeeds,
}

/// One view a job reads.
pub struct ViewDemand {
    pub key: ViewKey,
    pub side: SideIx,
    pub needs: ViewNeeds,
    /// The workspace the command read. shortcut: each command still reads its
    /// own `WorkspaceInputs` (with `needs.workspace`); Task 6 deletes those
    /// reads and the driver reads one per workspace path.
    pub inputs: Arc<WorkspaceInputs>,
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

/// A job's failure on one side: its writes (a stderr line) and its exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobFailure {
    pub actions: Vec<WriteAction>,
    pub exit: u8,
}

impl JobFailure {
    /// The failure `line` on stderr, exit `exit`.
    pub fn line(line: &str, exit: u8) -> JobFailure {
        JobFailure {
            actions: vec![WriteAction::line(line)],
            exit,
        }
    }

    /// The failure's stderr text without its last newline (a library wrapper's
    /// `Err` message).
    pub fn message(&self) -> String {
        let mut text = String::new();
        for a in &self.actions {
            if let WriteAction::Stderr(b) = a {
                text.push_str(&String::from_utf8_lossy(b));
            }
        }
        text.strip_suffix('\n').unwrap_or(&text).to_string()
    }
}

impl From<JobFailure> for JobOutput {
    fn from(f: JobFailure) -> JobOutput {
        JobOutput {
            actions: f.actions,
            exit: f.exit,
        }
    }
}

/// One side's computed document; only its job reads it (no shared shape
/// across commands).
pub struct SideDoc(Box<dyn Any + Send>);

impl SideDoc {
    pub fn new<T: Any + Send>(doc: T) -> SideDoc {
        SideDoc(Box::new(doc))
    }

    pub fn take<T: Any>(self) -> T {
        *self.0.downcast().expect("a side document of its own job")
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

/// What `resolve` reads: the view's model, and its identity table, derived
/// once per view on the first ask and dropped when the resolve step ends
/// (D4.2 (3)).
pub struct ResolveCx<'a> {
    model: &'a Model,
    ids: OnceCell<SnapshotIdentityTable>,
}

impl<'a> ResolveCx<'a> {
    pub(crate) fn new(model: &'a Model) -> ResolveCx<'a> {
        ResolveCx {
            model,
            ids: OnceCell::new(),
        }
    }

    pub fn model(&self) -> &'a Model {
        self.model
    }

    pub fn ids(&self) -> &SnapshotIdentityTable {
        self.ids.get_or_init(|| derive_identity_table(self.model))
    }
}

/// Why a view built no model; each job words it as its own command does
/// (D4.1). shortcut: two builders (analyze's and `AnalysisTarget`'s) until
/// Task 6's one view builder.
#[derive(Debug)]
pub enum ViewFailure {
    /// `alsem analyze`'s builder, with its program coverage.
    Model {
        fresh: Result<FreshCoverage, String>,
        failure: ModelFailure,
    },
    Target(TargetFailure),
}

/// One demand's access to its prepared view. Releases go through the driver
/// (D6.1, D6.2): a cell or run that a later demand on the view reads stays.
pub struct ViewCx<'a> {
    pub(crate) view: &'a mut View,
    pub(crate) fresh: Option<&'a Result<FreshCoverage, String>>,
    pub(crate) job: JobId,
    pub(crate) keys: Vec<RunKey>,
    pub(crate) has_tree: bool,
    pub(crate) later_cells: Vec<Cell>,
    pub(crate) later_runs: Vec<RunKey>,
    pub(crate) carry: Option<Box<dyn Any + Send>>,
}

impl ViewCx<'_> {
    pub fn view(&self) -> &View {
        self.view
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

    /// The run's output; the view's own copy is dropped unless a later demand
    /// reads it.
    pub fn take_run(&mut self, key: &RunKey) -> Arc<RunOutput> {
        if self.later_runs.contains(key) {
            self.view.run(key)
        } else {
            self.view.take_run(key)
        }
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
    /// One per view the job reads; empty for a job with no view.
    fn demands(&self) -> Vec<ViewDemand>;
    /// Per view, after its build (D4.2): this demand's part of the plan.
    fn resolve(&self, side: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure>;
    /// Off the prepared view.
    fn compute(&self, side: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure>;
    /// The view did not build: this command's own text and exit.
    fn view_failed(&self, side: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure>;
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
