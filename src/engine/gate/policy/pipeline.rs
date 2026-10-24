//! Policy CLI pipeline — ports al-sem `src/cli/policy.ts` (`runPolicyCheck` /
//! `runPolicyExplain`) and the `runPolicy` wrapper in `policy-engine.ts`.
//!
//! Drives the workspace analysis (same path as `events`: assemble → resolve →
//! detector context), builds the policy model-input view, runs the engine, and
//! formats the result. Policy resolution mirrors the CLI:
//!   1. `--no-policy` → "disabled" (policyVersion 0).
//!   2. `--policy <path>` → "explicit:<workspace-relative path>".
//!   3. auto-detect `al-sem.policy.yaml` / `.yml` in the workspace →
//!      "auto:<workspace-relative path>".
//!   4. bundled default → "default".
//!
//! `policySource`'s path component is ALWAYS relative to the analyzed workspace
//! root (forward slashes on every platform, no drive letters, no `.`/`..`
//! segments) — never an absolute machine path, which would be a reproducibility
//! defect in product output. A policy file outside the workspace root falls back
//! to its bare filename. See `workspace_relative` below.

use std::path::{Path, PathBuf};

use crate::engine::gate::policy::format_policy::{
    format_policy_human, format_policy_json, format_policy_sarif,
};
use crate::engine::gate::policy::policy_engine::{PolicyModel, PolicyRunResult, run_policy_engine};
use crate::engine::gate::policy::policy_loader::{
    BUNDLED_DEFAULT_POLICY_YAML, LoadResult, load_policy_from_file, load_policy_from_input,
    load_policy_from_string,
};
use crate::engine::gate::policy::policy_types::{PolicyDoc, predicate_to_json};
use crate::engine::gate::run::{AnalysisTarget, analyzer_diagnostics_of_view, diagnostic_lines};
use crate::engine::l5::detectors::default_detectors;
use crate::engine::root_classification::classify_roots;
use crate::engine::session::{Cell, View, policy_plan};
use crate::program::inputs::{PolicyOrigin, PolicyRequest, RequestInputs, WorkspaceInputs};
use crate::program::model::workspace::ModelRoutine;

/// How the effective policy was resolved.
enum ResolvedPolicy {
    /// A loaded policy with a `policySource` string.
    Loaded { policy: PolicyDoc, source: String },
    /// `--no-policy`: no policy, source "disabled", version 0.
    Disabled,
    /// A load error (stderr lines already prefixed).
    Error(Vec<String>),
}

/// The policy file `policy check` reads (#160): `--no-policy` reads none,
/// `--policy <path>` that path, else the auto-detected workspace file. The
/// command's `RequestInputs` is read with it, and [`resolve_policy_check`] then
/// reads only those.
#[must_use]
pub fn policy_request(policy_path: Option<&str>, no_policy: bool) -> PolicyRequest {
    if no_policy {
        PolicyRequest::Disabled
    } else if let Some(p) = policy_path {
        PolicyRequest::Path(p.into())
    } else {
        PolicyRequest::AutoDetect
    }
}

/// Auto-detect / explicit / default policy resolution for `check`. Mirrors
/// `runPolicyCheck`'s resolution block. The explicit-or-auto choice (and the
/// exists-first auto-detect rule) was applied when the `RequestInputs` was read
/// ([`policy_request`]); this loads the bytes it holds.
fn resolve_policy_check(workspace: &Path, request: &RequestInputs) -> ResolvedPolicy {
    if request.policy_request == PolicyRequest::Disabled {
        return ResolvedPolicy::Disabled;
    }
    if let Some(p) = &request.policy_file {
        let ws_abs = absolutize(workspace);
        let label = match p.origin {
            PolicyOrigin::Explicit => "explicit",
            PolicyOrigin::Auto => "auto",
        };
        return match load_policy_from_input(&p.file) {
            LoadResult::Ok { policy, .. } => ResolvedPolicy::Loaded {
                policy,
                source: format!("{label}:{}", workspace_relative(&ws_abs, &p.file.path)),
            },
            LoadResult::Err { errors, .. } => ResolvedPolicy::Error(
                errors
                    .into_iter()
                    .map(|e| format!("policy load error: {e}"))
                    .collect(),
            ),
        };
    }
    // Bundled default (embedded — byte-identical to the vendored yaml).
    match load_policy_from_string(BUNDLED_DEFAULT_POLICY_YAML) {
        LoadResult::Ok { policy, .. } => ResolvedPolicy::Loaded {
            policy,
            source: "default".to_string(),
        },
        LoadResult::Err { errors, .. } => ResolvedPolicy::Error(
            errors
                .into_iter()
                .map(|e| format!("bundled default policy load error: {e}"))
                .collect(),
        ),
    }
}

/// Resolve an absolute path (al-sem `resolve(path)` semantics — relative to cwd).
pub(crate) fn absolutize<P: AsRef<Path>>(p: P) -> PathBuf {
    let path = p.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Lexically collapse `.` and `..` components without touching the filesystem
/// (`absolutize(".")` leaves a trailing `CurDir` component that would otherwise
/// defeat a component-wise `strip_prefix` comparison).
fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The policy file's path relative to the analyzed workspace root — the tail of
/// the `policySource` field (`auto:<rel>` / `explicit:<rel>`). Forward slashes on
/// all platforms, no drive letters, no `.`/`..` segments. A policy file outside
/// the workspace root falls back to its bare filename.
fn workspace_relative(workspace: &Path, abs: &Path) -> String {
    let ws = normalize_lexical(workspace);
    let target = normalize_lexical(abs);
    match target.strip_prefix(&ws) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_string_lossy().replace('\\', "/")),
    }
}

/// The outcome of a `policy check` run (stdout text + exit + stderr lines).
pub struct PolicyCheckOutcome {
    pub text: Option<String>,
    pub exit_code: u8,
    pub stderr_lines: Vec<String>,
    /// When set, the text should be written to this `--out` path instead of stdout.
    pub out_path: Option<String>,
}

/// Options for `run_policy_check`.
pub struct PolicyCheckOptions<'a> {
    /// The command's one read of the workspace (#160).
    pub inputs: &'a std::sync::Arc<WorkspaceInputs>,
    /// The command's request files (#277 D3), read with [`policy_request`]: the
    /// `--policy` / auto-detected file and the request itself (`--no-policy`
    /// included).
    pub request: &'a RequestInputs,
    pub format: &'a str, // "human" | "json" | "sarif"
    pub out: Option<&'a str>,
    pub deterministic: bool,
    pub strict: bool,
    pub driver_version: &'a str,
    /// `--single-app`: the workspace alone; default (#70) the cross-app base.
    pub single_app: bool,
}

/// `runPolicyCheck`. Exit: 0 on success ALWAYS (no fail-on gate); non-zero only on
/// invalid format / strict analyzer error / policy-load error / write failure.
pub fn run_policy_check(opts: &PolicyCheckOptions) -> PolicyCheckOutcome {
    // A set read without a policy request holds no policy file even when the
    // workspace has one: the check would silently run the bundled default.
    assert!(
        opts.request.policy_request != PolicyRequest::None,
        "policy check: RequestInputs read without a policy request (use policy_request)"
    );
    // Resolve the effective policy before any build (#277 D3, Decision 7): a
    // bad policy is reported even when the build would fail, or `--strict`
    // would stop on an analyzer diagnostic.
    let (policy_opt, source): (Option<PolicyDoc>, String) =
        match resolve_policy_check(&opts.inputs.workspace, opts.request) {
            ResolvedPolicy::Loaded { policy, source } => (Some(policy), source),
            ResolvedPolicy::Disabled => (None, "disabled".to_string()),
            ResolvedPolicy::Error(lines) => {
                return PolicyCheckOutcome {
                    text: None,
                    exit_code: 1,
                    stderr_lines: lines,
                    out_path: None,
                };
            }
        };
    // Build the model (assemble → resolve → detector context).
    // #70: the cross-app base by default (rules see a dependency's table names and
    // the facts its code adds), the workspace alone under `--single-app`. No
    // model-instance id is `al-sem policy check: could not compute
    // modelInstanceId`, exit 1 (`AnalysisTarget::build`).
    let target = match AnalysisTarget::build(opts.inputs, opts.single_app, "al-sem policy check") {
        Ok(t) => t,
        Err(e) => {
            return PolicyCheckOutcome {
                text: None,
                exit_code: 1,
                stderr_lines: vec![e],
                out_path: None,
            };
        }
    };
    // #160 §4.4.4: one `View`. analyzeWorkspace diagnostics come off its run
    // of the default detector set, so detector-stage diagnostics surface
    // exactly as al-sem does; the run is taken once read.
    let mut view = View::new(target);
    let plan = policy_plan(default_detectors());
    let key = plan.keys().remove(0);
    view.prepare(&plan);
    let diag_lines = diagnostic_lines(&analyzer_diagnostics_of_view(&view, &key));
    drop(view.take_run(&key));

    // --strict: any error-severity diagnostic → exit 1 (stderr the diagnostics).
    if opts.strict && diag_lines.iter().any(|l| l.starts_with("error:")) {
        return PolicyCheckOutcome {
            text: None,
            exit_code: 1,
            stderr_lines: diag_lines,
            out_path: None,
        };
    }

    // #160 step 6 Task 3 (D7 (7)): the rule engine reads `det.summaries` and
    // the view's witness pass (`Cell::Raw`: every primary routine's raw
    // inherited facts); nothing else is read after the run.
    view.release_all_but(&[Cell::Det, Cell::Raw]);
    let resolved = view.model();
    let target = view.target();

    // Run the engine (runPolicy wrapper: disabled → empty result, version 0).
    let result: PolicyRunResult = match &policy_opt {
        None => PolicyRunResult {
            policy_source: source,
            policy_version: 0,
            rule_summaries: Vec::new(),
            findings: Vec::new(),
            diagnostics: Vec::new(),
        },
        Some(policy) => {
            let det = view.det();
            let root_classifications = classify_roots(&resolved.workspace);
            let ws = &resolved.workspace;
            // #70: rules evaluate WORKSPACE routines only; the dependency routines
            // stay in the model for lookup (tables, objects, inherited facts).
            let routines: std::borrow::Cow<[ModelRoutine]> =
                if ws.routines.iter().all(|r| target.is_primary_routine(r)) {
                    std::borrow::Cow::Borrowed(&ws.routines)
                } else {
                    ws.routines
                        .iter()
                        .filter(|r| target.is_primary_routine(r))
                        .cloned()
                        .collect()
                };
            let model = PolicyModel::new(
                &routines,
                &ws.objects,
                &ws.tables,
                &det.event_graph.events,
                &root_classifications,
                &det.summaries,
                view.raw(),
            );
            let out = run_policy_engine(&model, policy);
            PolicyRunResult {
                policy_source: source,
                policy_version: policy.version,
                rule_summaries: out.rule_summaries,
                findings: out.findings,
                diagnostics: out.diagnostics,
            }
        }
    };
    let dependencies = target.dependencies();
    // The rule engine was the last reader of `det` and the raw facts.
    view.release_all_but(&[]);

    let text = match opts.format {
        "json" => format_policy_json(
            &result,
            &dependencies,
            opts.driver_version,
            opts.deterministic,
        ),
        "sarif" => format_policy_sarif(&result, opts.driver_version),
        _ => dependencies.prefix_human(format_policy_human(&result)),
    };

    // Exit 0 always; stderr = analyzer diagnostics (after writing output).
    PolicyCheckOutcome {
        text: Some(text),
        exit_code: 0,
        stderr_lines: diag_lines,
        out_path: opts.out.map(|s| s.to_string()),
    }
}

// ---------------------------------------------------------------------------
// policy explain
// ---------------------------------------------------------------------------

/// The outcome of `policy explain`.
pub struct PolicyExplainOutcome {
    pub stdout: Option<String>,
    pub exit_code: u8,
    pub stderr_lines: Vec<String>,
}

/// Options for `run_policy_explain`.
pub struct PolicyExplainOptions<'a> {
    pub workspace: &'a Path,
    pub rule_id: &'a str,
    pub policy_path: Option<&'a str>,
}

/// `runPolicyExplain`. `--format json` is IGNORED (always human). Resolution mirrors
/// `runPolicyCheck` MINUS `--no-policy` (explain has no disabled mode).
pub fn run_policy_explain(opts: &PolicyExplainOptions) -> PolicyExplainOutcome {
    let ws_abs = absolutize(opts.workspace);
    // Resolve policy (explicit → auto → default).
    let (policy, source): (PolicyDoc, String) = if let Some(p) = opts.policy_path {
        let abs = absolutize(p);
        match load_policy_from_file(&abs) {
            LoadResult::Ok { policy, .. } => (
                policy,
                format!("explicit:{}", workspace_relative(&ws_abs, &abs)),
            ),
            LoadResult::Err { errors, .. } => {
                return PolicyExplainOutcome {
                    stdout: None,
                    exit_code: 1,
                    stderr_lines: errors
                        .into_iter()
                        .map(|e| format!("policy load error: {e}"))
                        .collect(),
                };
            }
        }
    } else {
        // Auto-detect.
        let mut found: Option<(PolicyDoc, String)> = None;
        for name in ["al-sem.policy.yaml", "al-sem.policy.yml"] {
            let candidate = opts.workspace.join(name);
            if candidate.exists() {
                let abs = absolutize(candidate.to_string_lossy().as_ref());
                match load_policy_from_file(&abs) {
                    LoadResult::Ok { policy, .. } => {
                        found = Some((
                            policy,
                            format!("auto:{}", workspace_relative(&ws_abs, &abs)),
                        ));
                        break;
                    }
                    LoadResult::Err { errors, .. } => {
                        return PolicyExplainOutcome {
                            stdout: None,
                            exit_code: 1,
                            stderr_lines: errors
                                .into_iter()
                                .map(|e| format!("policy load error: {e}"))
                                .collect(),
                        };
                    }
                }
            }
        }
        match found {
            Some(x) => x,
            None => match load_policy_from_string(BUNDLED_DEFAULT_POLICY_YAML) {
                LoadResult::Ok { policy, .. } => (policy, "default".to_string()),
                LoadResult::Err { errors, .. } => {
                    return PolicyExplainOutcome {
                        stdout: None,
                        exit_code: 1,
                        stderr_lines: errors
                            .into_iter()
                            .map(|e| format!("bundled default policy load error: {e}"))
                            .collect(),
                    };
                }
            },
        }
    };

    // Find the rule.
    let Some(rule) = policy.rules.iter().find(|r| r.id == opts.rule_id) else {
        return PolicyExplainOutcome {
            stdout: None,
            exit_code: 1,
            stderr_lines: vec![format!(
                "al-sem policy explain: rule '{}' not found in effective policy ({})",
                opts.rule_id, source
            )],
        };
    };

    // Render the rule-level summary + Normalized AST block.
    let coverage = rule
        .require_coverage
        .as_deref()
        .or_else(|| {
            policy
                .defaults
                .as_ref()
                .and_then(|d| d.require_coverage.as_deref())
        })
        .unwrap_or("any");
    let on_unknown = rule
        .on_unknown
        .as_deref()
        .or_else(|| {
            policy
                .defaults
                .as_ref()
                .and_then(|d| d.on_unknown.as_deref())
        })
        .unwrap_or("fail-closed");

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Rule: {}", rule.id));
    if let Some(t) = &rule.title {
        lines.push(format!("Title: {t}"));
    }
    lines.push(format!("Severity: {}", rule.severity));
    lines.push(format!("Coverage gate: {coverage}"));
    lines.push(format!("On unknown: {on_unknown}"));
    lines.push(format!("Effective policy: {source}"));
    lines.push(String::new());
    lines.push("Normalized AST:".to_string());
    lines.push(predicate_to_json(&rule.when));
    if let Some(except) = &rule.except {
        lines.push("Except:".to_string());
        lines.push(predicate_to_json(except));
    }

    PolicyExplainOutcome {
        stdout: Some(format!("{}\n", lines.join("\n"))),
        exit_code: 0,
        stderr_lines: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_relative;
    use std::path::PathBuf;

    // Paths below are built with `PathBuf::join` rather than embedded-separator
    // string literals (e.g. `r"U:\ws"`), so separators are native to whatever OS
    // runs the test (backslash on Windows, forward slash on Unix) and
    // `strip_prefix`'s component-wise comparison is exercised identically on
    // both — a raw `r"U:\ws"` literal parses as a single opaque filename
    // component on Unix (no drive-letter/backslash-separator concept there),
    // which previously made 5 of these 6 tests Windows-only despite having no
    // `#[cfg(windows)]` marking them as such.

    #[test]
    fn inside_workspace_is_forward_slash_relative() {
        let ws = PathBuf::from("root").join("ws");
        let abs = ws.join("al-sem.policy.yaml");
        assert_eq!(workspace_relative(&ws, &abs), "al-sem.policy.yaml");
    }

    #[test]
    fn nested_subdir_joins_with_forward_slashes() {
        let ws = PathBuf::from("root").join("ws");
        let abs = ws.join("sub").join("dir").join("al-sem.policy.yaml");
        assert_eq!(workspace_relative(&ws, &abs), "sub/dir/al-sem.policy.yaml");
    }

    #[test]
    fn outside_workspace_falls_back_to_bare_filename() {
        let ws = PathBuf::from("root").join("ws");
        let abs = PathBuf::from("root").join("other").join("p.yaml");
        assert_eq!(workspace_relative(&ws, &abs), "p.yaml");
    }

    #[test]
    fn sibling_workspace_name_is_not_treated_as_a_prefix_match() {
        // `ws-foo` contains "ws" as a string prefix but is a component-wise
        // SIBLING of `ws`, not a descendant — `strip_prefix` compares path
        // components, not strings, so this must still fall back to the bare
        // filename rather than being (wrongly) treated as inside the workspace.
        let ws = PathBuf::from("root").join("ws");
        let abs = PathBuf::from("root").join("ws-foo").join("p.yaml");
        assert_eq!(workspace_relative(&ws, &abs), "p.yaml");
    }

    #[cfg(windows)]
    #[test]
    fn windows_backslash_input_normalizes_to_forward_slashes() {
        // Backslash-embedded literals and a drive-letter prefix have no
        // equivalent on Unix (`\` is an ordinary filename character there, and
        // there is no drive-letter/Prefix component), so this case is
        // inherently Windows-only and cannot be ported to native construction.
        use std::path::Path;
        let ws = Path::new(r"U:\ws\root");
        let abs = Path::new(r"U:\ws\root\a\b\al-sem.policy.yaml");
        assert_eq!(workspace_relative(ws, abs), "a/b/al-sem.policy.yaml");
    }

    #[test]
    fn leading_dot_workspace_is_normalized_away() {
        // `normalize_lexical`'s `CurDir` arm is reachable only when a path's
        // OWN first component is a literal `.` (e.g. a workspace root of ".").
        // A `.` in the middle or at the end of a path is already elided by
        // `Path::components()` itself before `normalize_lexical` ever sees it
        // (verified: `PathBuf::from("root").join("ws").join(".")` yields the
        // components `[Normal("root"), Normal("ws")]` with no `CurDir` at
        // all) — so a leading dot is the one shape that actually exercises
        // that arm, on every platform.
        let ws = PathBuf::from(".");
        let abs = PathBuf::from(".").join("al-sem.policy.yaml");
        assert_eq!(workspace_relative(&ws, &abs), "al-sem.policy.yaml");
    }
}
