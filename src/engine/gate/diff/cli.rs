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
    use crate::engine::gate::run::{AnalysisTarget, analyzer_diagnostic_lines};
    use crate::engine::l5::snapshot_full::{FullSnapshotOptions, compose_full_snapshot};
    use crate::program::inputs::{InputNeeds, InputSet};
    use crate::program::model::program_calls::assemble_and_resolve_workspace_program;

    let ws = Path::new(dir);
    // #160 §4.2.6: one read of this side's workspace.
    let inputs = InputSet::read(
        ws,
        &InputNeeds {
            fingerprint_rows: true,
            ..InputNeeds::default()
        },
    );
    let model_id = inputs
        .model_instance_id
        .as_deref()
        .ok_or_else(|| format!("could not compute modelInstanceId for workspace '{dir}'"))?;
    // Engine-switch S6.4: the program-backed model (program calls and events).
    let resolved = assemble_and_resolve_workspace_program(&inputs, model_id, false)
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
    let snapshot = compose_full_snapshot(target.resolved(), &opts);
    drop(target);
    // #160 §4.1.4: this side's model is gone, so the other side's build is
    // not post-attach.
    crate::engine::perf_trace::end_phase("post_attach");
    Ok((snapshot, diag_lines))
}

/// Load one side: a workspace (reanalyzed, with its strict diagnostic lines) or
/// a snapshot file. Deserialization keeps only the snapshot payload, so a
/// snapshot side contributes no analyzer diagnostics even when the envelope
/// carried some.
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

        let (_, lines) =
            load_snapshot_from_workspace(ws.to_str().expect("utf-8 path"), "t", true, true)
                .expect("workspace loads");
        let want = "warning: dependency \"CodeunitRecLibA\" (eeee5555-0000-0000-0000-000000000005), \
                    declared by \"CodeunitRec Probe\" at version 2.0.0.0, is present only at \
                    version 1.0.0.0";
        assert!(
            lines.iter().any(|l| l == want),
            "missing {want:?}; got {lines:?}"
        );
    }
}
