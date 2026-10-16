//! The `diff` CLI orchestrator. Port of al-sem `src/cli/diff.ts`: input-kind
//! detection, snapshot/workspace loading, the `--strict` analyzer-diagnostic gate
//! (workspace-mode sides only, before the rename overlay; #89), the rename
//! overlay, format dispatch, `--out`, the ws-mode stderr note, and the exit gates
//! (`--fail-on`, strict-coverage).

use std::path::Path;

use crate::engine::gate::cbor::CborValue;
use crate::engine::gate::snapshot_deserialize::{SnapshotFormat, deserialize_snapshot};

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

/// Compose a full snapshot tree by reanalyzing a workspace directory (ws-mode).
/// Under `strict` it also returns that side's analyzer diagnostics as
/// `<severity>: <message>` lines, the list `fingerprint --strict` gates on (#89);
/// otherwise the lines are empty and no detector runs. `diff` stays single-app
/// (#70). The model is dropped before the next side loads, so at most one is alive.
fn load_snapshot_from_workspace(
    dir: &str,
    driver_version: &str,
    deterministic: bool,
    strict: bool,
) -> Result<(CborValue, Vec<String>), String> {
    use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
    use crate::engine::gate::run::{AnalysisTarget, analyzer_diagnostic_lines};
    use crate::engine::l5::snapshot_full::{FullSnapshotOptions, compose_full_snapshot};
    use crate::program::model::program_calls::assemble_and_resolve_workspace_program;

    let ws = Path::new(dir);
    let model_id = compute_gate_model_instance_id(ws)
        .ok_or_else(|| format!("could not compute modelInstanceId for workspace '{dir}'"))?;
    // Engine-switch S6.4: the program-backed model (program calls and events).
    let resolved = assemble_and_resolve_workspace_program(ws, &model_id, false)
        .ok_or_else(|| format!("workspace '{dir}' did not resolve"))?;
    let target = AnalysisTarget::SingleApp(Box::new(resolved));
    let diag_lines = if strict {
        analyzer_diagnostic_lines(ws, &target)
    } else {
        Vec::new()
    };
    let opts = FullSnapshotOptions {
        workspace_dir: ws,
        driver_version,
        deterministic,
        roots_config_ignored: false,
    };
    Ok((compose_full_snapshot(target.resolved(), &opts), diag_lines))
}

/// Load one side: a workspace (reanalyzed, with its strict diagnostic lines) or
/// a snapshot file (no analyzer diagnostics survive deserialization).
fn load_side(
    arg: &str,
    kind: InputKind,
    opts: &DiffCliOptions,
) -> Result<(CborValue, Vec<String>), String> {
    match kind {
        InputKind::Workspace => {
            load_snapshot_from_workspace(arg, opts.driver_version, opts.deterministic, opts.strict)
        }
        _ => load_snapshot_from_path(arg).map(|s| (s, Vec::new())),
    }
}

/// Run the diff command end-to-end. Returns a `DiffRunOutcome`; the bin maps it to
/// stdout/stderr writes + a process exit code.
pub fn run_diff(opts: &DiffCliOptions) -> DiffRunOutcome {
    let mut stderr_lines: Vec<String> = Vec::new();
    let mut workspace_mode = false;

    // Input-kind detection.
    let old_kind = match detect_input_kind(opts.old_arg) {
        Ok(k) => k,
        Err(msg) => return early_error(msg, 1),
    };
    let new_kind = match detect_input_kind(opts.new_arg) {
        Ok(k) => k,
        Err(msg) => return early_error(msg, 1),
    };

    // Reject .app input cleanly (CONFIG_ERROR exit 2). al-sem detects `.app` as a
    // distinct kind and routes it to loadSnapshotFromApp; the Rust engine does not
    // port the .app snapshot path, so it rejects it.
    if old_kind == InputKind::App || new_kind == InputKind::App {
        return early_error(
            "diff: .app input is not supported by the Rust engine; persist a snapshot with 'al-sem fingerprint --format cbor.gz' and diff that".to_string(),
            2,
        );
    }

    if old_kind == InputKind::Workspace || new_kind == InputKind::Workspace {
        workspace_mode = true;
    }

    // Load both snapshots.
    let (old_snap, old_diag_lines) = match load_side(opts.old_arg, old_kind, opts) {
        Ok(s) => s,
        Err(msg) => return early_error(format!("failed to load snapshot: {msg}"), 1),
    };
    let (new_snap, new_diag_lines) = match load_side(opts.new_arg, new_kind, opts) {
        Ok(s) => s,
        Err(msg) => return early_error(format!("failed to load snapshot: {msg}"), 1),
    };

    // --strict (#89): an error-severity analyzer diagnostic on EITHER workspace
    // side → its lines to stderr, no output, exit 1, and no ws-mode note, as
    // `fingerprint`/`events`/`policy` print only the diagnostics. It runs before
    // the rename overlay, the engine and every exit gate. Old side first; a
    // new-side line already printed for the old side is dropped, repeats within
    // one side are kept. Without an error the run continues unchanged and the
    // warning/info lines are not printed. In snapshot-input mode both lists are
    // empty, so `--strict` is a no-op there.
    let any_error = old_diag_lines
        .iter()
        .chain(&new_diag_lines)
        .any(|l| l.starts_with("error:"));
    if any_error {
        let mut lines = old_diag_lines.clone();
        lines.extend(
            new_diag_lines
                .into_iter()
                .filter(|l| !old_diag_lines.contains(l)),
        );
        return DiffRunOutcome {
            output: None,
            stderr_lines: lines,
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
    if opts.coverage_policy == CoveragePolicy::Strict && strict_coverage_failed {
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
