//! The `diff` CLI orchestrator. Port of al-sem `src/cli/diff.ts`: input-kind
//! detection, snapshot/workspace loading, the `--strict` analyzer-diagnostic gate
//! (workspace-mode sides only, before the rename overlay; #89), the rename
//! overlay, format dispatch, `--out`, the ws-mode stderr note, and the exit gates
//! (a fatal preflight first, #220; then strict-coverage and `--fail-on`).

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewKey, ViewNeeds,
};
use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::cbor::CborValue;
use crate::engine::gate::run::{TargetFailure, analyzer_diagnostics_of_view, diagnostic_lines};
use crate::engine::gate::snapshot_deserialize::{SnapshotFormat, deserialize_snapshot};
use crate::engine::l5::detectors::default_detectors;
use crate::engine::l5::snapshot_full::{FullSnapshotOptions, compose_full_snapshot};
use crate::engine::session::diff_plan;
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};

use super::format::format_diff;
use super::renames::parse_rename_overlay;
use super::{CoveragePolicy, DiffEngineOptions, Severity, run_diff_engine};

/// The outcome of a diff run: the text to write (stdout or --out), the lines to
/// write to stderr (analyzer diagnostics + the ws-mode note), and the exit code.
pub struct DiffRunOutcome {
    pub output: Option<String>,
    pub stderr_lines: Vec<String>,
    pub exit_code: u8,
    /// When set, an early error: print to stderr, no stdout, exit `exit_code`.
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Snapshot,
    Workspace,
    App,
}

/// The ws-mode stderr note (al-sem diff.ts), byte-exact.
pub const WORKSPACE_MODE_NOTE: &str = "note: workspace-mode reanalyzes both sides; for CI, persist snapshots with 'al-sem fingerprint --format cbor.gz' and diff those instead.";

pub struct DiffCliOptions<'a> {
    pub old_arg: &'a str,
    pub new_arg: &'a str,
    pub format: &'a str, // already validated: human | json | sarif
    pub out: Option<&'a str>,
    pub coverage_policy: CoveragePolicy,
    pub renames_path: Option<&'a str>,
    pub fail_on: Option<Severity>,
    /// `--strict`: exit 1 on an error-severity analyzer diagnostic of either
    /// workspace-mode side, before any output (#89). A no-op for snapshot inputs.
    pub strict: bool,
    pub deterministic: bool,
    pub driver_version: &'a str,
}

fn detect_input_kind(arg: &str) -> Result<InputKind, String> {
    let p = Path::new(arg);
    if !p.exists() {
        return Err(format!("input not found: {arg}"));
    }
    if p.is_dir() {
        return Ok(InputKind::Workspace);
    }
    if arg.to_lowercase().ends_with(".app") {
        return Ok(InputKind::App);
    }
    Ok(InputKind::Snapshot)
}

/// Load a snapshot from a file path → deserialized CborValue tree. The format hint
/// comes from the extension (al-sem `loadSnapshotFromPath`).
fn load_snapshot_from_path(path: &str) -> Result<CborValue, String> {
    let lower = path.to_lowercase();
    let hint = if lower.ends_with(".json") {
        Some(SnapshotFormat::Json)
    } else if lower.ends_with(".cbor.gz") || lower.ends_with(".gz") {
        Some(SnapshotFormat::CborGz)
    } else if lower.ends_with(".cbor") {
        Some(SnapshotFormat::Cbor)
    } else {
        None
    };
    let bytes =
        std::fs::read(path).map_err(|e| format!("could not read snapshot '{path}': {e}"))?;
    deserialize_snapshot(&bytes, hint)
}

/// A loaded side: its snapshot tree and, for a `--strict` workspace side, its
/// analyzer diagnostic lines. A snapshot file contributes no lines, even when
/// its envelope carried some (deserialization keeps only the payload).
type Side = (CborValue, Vec<String>);

/// A snapshot-file side, loaded.
fn file_side(arg: &str) -> Result<Side, JobFailure> {
    load_snapshot_from_path(arg)
        .map(|s| (s, Vec::new()))
        .map_err(load_failure)
}

/// "failed to load snapshot: <msg>", exit 1.
fn load_failure(msg: String) -> JobFailure {
    JobFailure::line(&format!("failed to load snapshot: {msg}"), 1)
}

/// `alsem diff` as a batch job (#277 D8). A workspace side is a demand on its
/// single-app view (#70), which composes the side's snapshot there; two sides
/// on one workspace share one view (`diff A A`: one build). The old snapshot
/// file is loaded before any build, the new one in `finalize`, so a side
/// loads only when standalone `diff` would load it: a failed old side stops
/// the run (the driver skips the new side's view). Its document is the
/// [`DiffRunOutcome`].
pub struct DiffJob<'a> {
    opts: &'a DiffCliOptions<'a>,
    kinds: [InputKind; 2],
    /// Each workspace side's read; one read when both name one view.
    inputs: [Option<Arc<WorkspaceInputs>>; 2],
    /// The old snapshot file, loaded at construction, taken by `finalize`.
    old_file: RefCell<Option<CborValue>>,
}

impl<'a> DiffJob<'a> {
    /// The job, or its early outcome: an input that does not exist, an
    /// `.app` input, or an old snapshot file that does not load. Nothing is
    /// built for any of them.
    pub fn new(opts: &'a DiffCliOptions<'a>) -> Result<DiffJob<'a>, DiffRunOutcome> {
        // Input-kind detection.
        let old_kind = detect_input_kind(opts.old_arg).map_err(|m| early_error(m, 1))?;
        let new_kind = detect_input_kind(opts.new_arg).map_err(|m| early_error(m, 1))?;

        // Reject .app input cleanly (CONFIG_ERROR exit 2). al-sem detects `.app` as a
        // distinct kind and routes it to loadSnapshotFromApp; the Rust engine does not
        // port the .app snapshot path, so it rejects it.
        if old_kind == InputKind::App || new_kind == InputKind::App {
            return Err(early_error(
                "diff: .app input is not supported by the Rust engine; persist a snapshot with 'al-sem fingerprint --format cbor.gz' and diff that".to_string(),
                2,
            ));
        }
        let old_file = match old_kind {
            InputKind::Workspace => None,
            _ => match file_side(opts.old_arg) {
                Ok((snapshot, _)) => Some(snapshot),
                Err(f) => return Err(DiffRunOutcome::failed(f)),
            },
        };
        // #160 §4.2.6: one read per side's workspace; both sides on one view
        // share it.
        let read = |arg: &str| {
            WorkspaceInputs::read(
                Path::new(arg),
                &WorkspaceNeeds {
                    fingerprint_rows: true,
                    ..WorkspaceNeeds::default()
                },
            )
        };
        let old = (old_kind == InputKind::Workspace).then(|| read(opts.old_arg));
        let new = (new_kind == InputKind::Workspace).then(|| match &old {
            Some(o) if view_key(opts.old_arg) == view_key(opts.new_arg) => Arc::clone(o),
            _ => read(opts.new_arg),
        });
        Ok(DiffJob {
            opts,
            kinds: [old_kind, new_kind],
            inputs: [old, new],
            old_file: RefCell::new(old_file),
        })
    }

    fn arg(&self, side: SideIx) -> &str {
        match side.0 {
            0 => self.opts.old_arg,
            _ => self.opts.new_arg,
        }
    }
}

/// A workspace side's view (`diff` stays single-app, #70).
fn view_key(arg: &str) -> ViewKey {
    ViewKey::new(Path::new(arg), true, false)
}

impl Job for DiffJob<'_> {
    fn demands(&self) -> Vec<ViewDemand> {
        (0..2u8)
            .filter_map(|i| {
                let inputs = self.inputs[usize::from(i)].as_ref()?;
                Some(ViewDemand {
                    key: view_key(self.arg(SideIx(i))),
                    side: SideIx(i),
                    needs: ViewNeeds::MODEL,
                    inputs: Arc::clone(inputs),
                })
            })
            .collect()
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        Ok(PlanPart {
            plan: diff_plan(default_detectors(), self.opts.strict),
            reads: vec![],
            carry: None,
        })
    }

    /// Compose the side's full snapshot tree off its view (ws-mode). Under
    /// `--strict` the side's analyzer diagnostics come first, as
    /// `<severity>: <message>` lines, the list `fingerprint --strict` gates on
    /// (#89); the run and its cells are released before the composition.
    /// Without `--strict` no detector runs and nothing is filled.
    fn compute(&self, side: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let diag_lines = if self.opts.strict {
            let key = cx.keys()[0].clone();
            let lines = diagnostic_lines(&analyzer_diagnostics_of_view(cx.view(), &key));
            drop(cx.take_run(&key));
            cx.release_all_but(&[]);
            lines
        } else {
            Vec::new()
        };
        let opts = FullSnapshotOptions {
            inputs: self.inputs[usize::from(side.0)]
                .as_ref()
                .expect("a workspace side"),
            driver_version: self.opts.driver_version,
            deterministic: self.opts.deterministic,
            roots_config_ignored: false,
            universe: crate::engine::l5::snapshot_full::SnapshotUniverse::SingleApp,
        };
        let snapshot = compose_full_snapshot(cx.view().model(), &opts);
        Ok(SideDoc::new((snapshot, diag_lines)))
    }

    fn view_failed(&self, side: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        let dir = self.arg(side);
        Err(load_failure(match failure.target {
            TargetFailure::NoModelInstanceId => {
                format!("could not compute modelInstanceId for workspace '{dir}'")
            }
            _ => format!("workspace '{dir}' did not resolve"),
        }))
    }

    /// Today's order: the old side's load error, the new side's, the
    /// `--strict` gate, the rename overlay, the engine, the render.
    fn finalize(&self, sides: Vec<Result<SideDoc, JobFailure>>) -> Doc {
        let mut ws = sides.into_iter();
        let mut side = |i: usize| -> Result<Side, JobFailure> {
            match self.kinds[i] {
                InputKind::Workspace => ws.next().expect("a workspace side").map(SideDoc::take),
                _ if i == 0 => Ok((
                    self.old_file.take().expect("the old file, loaded once"),
                    Vec::new(),
                )),
                _ => file_side(self.opts.new_arg),
            }
        };
        let old = match side(0) {
            Ok(old) => old,
            Err(f) => return Doc::new(DiffRunOutcome::failed(f)),
        };
        let new = match side(1) {
            Ok(new) => new,
            Err(f) => return Doc::new(DiffRunOutcome::failed(f)),
        };
        Doc::new(finish_diff(self.opts, self.kinds, old, new))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        doc.take::<DiffRunOutcome>().into_output(self.opts.out)
    }
}

/// The strict gate's stderr (#89): the old side's lines, then the new side's in
/// full, with no text dedupe across sides, because a path-free message shared by
/// two revisions must still show the new side has it. When both sides are the
/// same workspace, its list prints once.
fn strict_gate_lines(old: Vec<String>, new: Vec<String>, same_workspace: bool) -> Vec<String> {
    if same_workspace {
        return old;
    }
    let mut lines = old;
    lines.extend(new);
    lines
}

/// Both arguments name one workspace directory, so its diagnostics would print
/// twice under `--strict` (#89). Compared canonically; an unresolvable path is
/// never "the same".
fn same_workspace(old: &str, old_kind: InputKind, new: &str, new_kind: InputKind) -> bool {
    old_kind == InputKind::Workspace
        && new_kind == InputKind::Workspace
        && matches!(
            (std::fs::canonicalize(old), std::fs::canonicalize(new)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Run the diff command end-to-end, through the batch driver. Returns a
/// `DiffRunOutcome`; the bin maps it to stdout/stderr writes + a process exit
/// code.
pub fn run_diff(opts: &DiffCliOptions) -> DiffRunOutcome {
    match DiffJob::new(opts) {
        Err(early) => early,
        Ok(job) => crate::engine::batch::run_doc(&job).take(),
    }
}

/// Both sides loaded: the `--strict` gate, the rename overlay, the engine,
/// the render and the exit gates.
fn finish_diff(
    opts: &DiffCliOptions,
    [old_kind, new_kind]: [InputKind; 2],
    (old_snap, old_diag_lines): Side,
    (new_snap, new_diag_lines): Side,
) -> DiffRunOutcome {
    let mut stderr_lines: Vec<String> = Vec::new();
    let workspace_mode = old_kind == InputKind::Workspace || new_kind == InputKind::Workspace;

    // --strict (#89): an error-severity analyzer diagnostic on EITHER workspace
    // side → its lines to stderr, no output, exit 1, and no ws-mode note, as
    // `fingerprint`/`events`/`policy` print only the diagnostics. It runs before
    // the rename overlay, the engine and every exit gate. Old side first, then
    // the new side in full: a path-free message the two revisions share must
    // still show the new side has it. Only when both arguments are the SAME
    // workspace is its list printed once. Without an error the run continues
    // unchanged and the warning/info lines are not printed. Diff reads no
    // diagnostics from a snapshot file, so `--strict` is a no-op for that side.
    let any_error = old_diag_lines
        .iter()
        .chain(&new_diag_lines)
        .any(|l| l.starts_with("error:"));
    if any_error {
        let same = same_workspace(opts.old_arg, old_kind, opts.new_arg, new_kind);
        return DiffRunOutcome {
            output: None,
            stderr_lines: strict_gate_lines(old_diag_lines, new_diag_lines, same),
            exit_code: 1,
            error_message: None,
        };
    }

    // Optional rename overlay.
    let rename_overlay = match opts.renames_path {
        None => None,
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => match parse_rename_overlay(&text) {
                Ok(o) => Some(o),
                Err(e) => return early_error(format!("failed to load rename overlay: {e}"), 1),
            },
            Err(e) => {
                return early_error(format!("failed to load rename overlay: {e}"), 1);
            }
        },
    };

    // The envelope diagnostics channel stays empty in both modes, matching the
    // snapshot-input and ws-mode goldens (`projectDiagnostics([])`); the `--strict`
    // gate above reads the analyzer diagnostics, the envelope does not (#89).
    let analyzer_diagnostics: Vec<CborValue> = Vec::new();

    // Run the engine.
    let engine_opts = DiffEngineOptions {
        coverage_policy: opts.coverage_policy,
        deterministic: opts.deterministic,
        rename_overlay,
    };
    let result = run_diff_engine(&old_snap, &new_snap, &engine_opts);

    // Render.
    let text = format_diff(
        &result,
        opts.format,
        opts.driver_version,
        opts.deterministic,
        &analyzer_diagnostics,
    );

    // ws-mode stderr note (always appended when workspace mode).
    if workspace_mode {
        stderr_lines.push(WORKSPACE_MODE_NOTE.to_string());
    }

    // Exit gates.
    let strict_coverage_failed = result
        .diagnostics
        .iter()
        .any(|d| d.kind == "coverage-incomplete");
    let mut exit_code: u8 = 0;
    // #220 D5.5: a fatal preflight (the diff did not run) exits 1, first.
    if result.fatal || (opts.coverage_policy == CoveragePolicy::Strict && strict_coverage_failed) {
        exit_code = 1;
    } else if let Some(threshold) = opts.fail_on {
        let t = threshold.rank();
        if result.findings.iter().any(|f| f.severity.rank() <= t) {
            exit_code = 1;
        }
    }

    DiffRunOutcome {
        output: Some(text),
        stderr_lines,
        exit_code,
        error_message: None,
    }
}

fn early_error(msg: String, exit_code: u8) -> DiffRunOutcome {
    DiffRunOutcome {
        output: None,
        stderr_lines: Vec::new(),
        exit_code,
        error_message: Some(msg),
    }
}

impl DiffRunOutcome {
    /// A side's failure as the early error it is.
    fn failed(f: JobFailure) -> DiffRunOutcome {
        early_error(f.message(), f.exit)
    }

    /// The writes `diff` makes (#277 D2.3): an early error's line alone; else
    /// the output (to `out` or stdout), then the stderr lines.
    pub fn into_output(self, out: Option<&str>) -> JobOutput {
        if let Some(msg) = self.error_message {
            return JobOutput::lines(&[msg], self.exit_code);
        }
        let mut actions: Vec<WriteAction> = self
            .output
            .map(|text| WriteAction::out(out, text))
            .into_iter()
            .collect();
        actions.extend(self.stderr_lines.iter().map(|l| WriteAction::line(l)));
        JobOutput {
            actions,
            exit: self.exit_code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// #89: two different workspaces that share a path-free message both print
    /// it, so the reader sees the NEW side has the error too. Mutation: drop
    /// new-side lines equal to an old-side line -> the second assert fails.
    #[test]
    fn strict_gate_lines_keep_a_shared_line_from_two_workspaces() {
        let old = v(&["error: cap hit", "warning: w"]);
        let new = v(&["error: cap hit"]);
        assert_eq!(
            strict_gate_lines(old.clone(), new.clone(), true),
            v(&["error: cap hit", "warning: w"])
        );
        assert_eq!(
            strict_gate_lines(old, new, false),
            v(&["error: cap hit", "warning: w", "error: cap hit"])
        );
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).expect("mkdir");
        for entry in std::fs::read_dir(src).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let to = dst.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                copy_dir(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).expect("copy");
            }
        }
    }

    /// #91: a workspace side's strict lines carry the program build's
    /// dependency-ledger warnings, as `alsem analyze` reports them. diff prints
    /// these lines only on a failing gate, so the loader is where they show.
    /// The fixture is `tests/r0-corpus/ws-codeunit-rec` with its
    /// `CodeunitRecLibA` dependency declared at 2.0.0.0 against the packaged
    /// 1.0.0.0. Mutation: drop source (2) from `analyzer_diagnostics_of_run`, or
    /// give the single-app model an empty `dependency_diagnostics` -> this fails.
    #[test]
    fn workspace_side_strict_lines_carry_the_dependency_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path().join("ws-codeunit-rec");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-codeunit-rec"),
            &ws,
        );
        let app_json = ws.join("app.json");
        let mut app: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&app_json).expect("read app.json"))
                .expect("parse app.json");
        let dep = app["dependencies"]
            .as_array_mut()
            .expect("dependencies")
            .iter_mut()
            .find(|d| d["name"] == "CodeunitRecLibA")
            .expect("CodeunitRecLibA dependency");
        dep["version"] = serde_json::Value::from("2.0.0.0");
        std::fs::write(&app_json, app.to_string()).expect("write app.json");

        /// The diff job with its sides kept as computed, through the driver.
        struct Sides<'a>(DiffJob<'a>);
        impl Job for Sides<'_> {
            fn demands(&self) -> Vec<ViewDemand> {
                self.0.demands()
            }
            fn resolve(&self, s: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
                self.0.resolve(s, cx)
            }
            fn compute(&self, s: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
                self.0.compute(s, cx)
            }
            fn view_failed(&self, s: SideIx, f: &ViewFailure) -> Result<SideDoc, JobFailure> {
                self.0.view_failed(s, f)
            }
            fn finalize(&self, sides: Vec<Result<SideDoc, JobFailure>>) -> Doc {
                Doc::new(sides)
            }
            fn serialize(&self, _: Doc) -> JobOutput {
                unreachable!()
            }
        }
        let arg = ws.to_str().expect("utf-8 path");
        let opts = DiffCliOptions {
            old_arg: arg,
            new_arg: arg,
            format: "json",
            out: None,
            coverage_policy: CoveragePolicy::Loose,
            renames_path: None,
            fail_on: None,
            strict: true,
            deterministic: true,
            driver_version: "t",
        };
        let job = Sides(DiffJob::new(&opts).unwrap_or_else(|_| panic!("inputs exist")));
        let sides = crate::engine::batch::run_doc(&job).take::<Vec<Result<SideDoc, JobFailure>>>();
        let (_, lines) = sides
            .into_iter()
            .next()
            .expect("the old side")
            .expect("workspace loads")
            .take::<Side>();
        let want = "warning: dependency \"CodeunitRecLibA\" (eeee5555-0000-0000-0000-000000000005), \
                    declared by \"CodeunitRec Probe\" at version 2.0.0.0, is present only at \
                    version 1.0.0.0";
        assert!(
            lines.iter().any(|l| l == want),
            "missing {want:?}; got {lines:?}"
        );
    }
}
