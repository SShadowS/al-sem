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

use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
use crate::engine::gate::policy::format_policy::{
    format_policy_human, format_policy_json, format_policy_sarif,
};
use crate::engine::gate::policy::policy_engine::{PolicyModel, PolicyRunResult, run_policy_engine};
use crate::engine::gate::policy::policy_loader::{
    BUNDLED_DEFAULT_POLICY_YAML, LoadResult, load_policy_from_file, load_policy_from_string,
};
use crate::engine::gate::policy::policy_types::{PolicyDoc, predicate_to_json};
use crate::engine::gate::run::compute_analyzer_diagnostics;
use crate::engine::l3::program_calls::assemble_and_resolve_workspace_program;
use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::digest_cli::DEFAULT_DETECTOR_NAMES;
use crate::engine::root_classification::classify_roots;

/// How the effective policy was resolved.
enum ResolvedPolicy {
    /// A loaded policy with a `policySource` string.
    Loaded { policy: PolicyDoc, source: String },
    /// `--no-policy`: no policy, source "disabled", version 0.
    Disabled,
    /// A load error (stderr lines already prefixed).
    Error(Vec<String>),
}

/// Auto-detect / explicit / default policy resolution for `check`. Mirrors
/// `runPolicyCheck`'s resolution block.
fn resolve_policy_check(
    workspace: &Path,
    policy_path: Option<&str>,
    no_policy: bool,
) -> ResolvedPolicy {
    if no_policy {
        return ResolvedPolicy::Disabled;
    }
    let ws_abs = absolutize(workspace);
    if let Some(p) = policy_path {
        let abs = absolutize(p);
        return match load_policy_from_file(&abs) {
            LoadResult::Ok { policy, .. } => ResolvedPolicy::Loaded {
                policy,
                source: format!("explicit:{}", workspace_relative(&ws_abs, &abs)),
            },
            LoadResult::Err { errors, .. } => ResolvedPolicy::Error(
                errors
                    .into_iter()
                    .map(|e| format!("policy load error: {e}"))
                    .collect(),
            ),
        };
    }
    // Auto-detect.
    for name in ["al-sem.policy.yaml", "al-sem.policy.yml"] {
        let candidate = workspace.join(name);
        if candidate.exists() {
            let abs = absolutize(candidate.to_string_lossy().as_ref());
            return match load_policy_from_file(&abs) {
                LoadResult::Ok { policy, .. } => ResolvedPolicy::Loaded {
                    policy,
                    source: format!("auto:{}", workspace_relative(&ws_abs, &abs)),
                },
                LoadResult::Err { errors, .. } => ResolvedPolicy::Error(
                    errors
                        .into_iter()
                        .map(|e| format!("policy load error: {e}"))
                        .collect(),
                ),
            };
        }
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
fn absolutize<P: AsRef<Path>>(p: P) -> PathBuf {
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
    pub workspace: &'a Path,
    pub policy_path: Option<&'a str>,
    pub no_policy: bool,
    pub format: &'a str, // "human" | "json" | "sarif"
    pub out: Option<&'a str>,
    pub deterministic: bool,
    pub strict: bool,
    pub driver_version: &'a str,
}

/// `runPolicyCheck`. Exit: 0 on success ALWAYS (no fail-on gate); non-zero only on
/// invalid format / strict analyzer error / policy-load error / write failure.
pub fn run_policy_check(opts: &PolicyCheckOptions) -> PolicyCheckOutcome {
    // Build the model (assemble → resolve → detector context).
    let model_id = match compute_gate_model_instance_id(opts.workspace) {
        Some(id) => id,
        None => {
            return PolicyCheckOutcome {
                text: None,
                exit_code: 1,
                stderr_lines: vec![
                    "al-sem policy check: could not compute modelInstanceId".to_string(),
                ],
                out_path: None,
            };
        }
    };
    // Engine-switch S6.6: the program-backed model (program calls and events).
    let resolved = match assemble_and_resolve_workspace_program(opts.workspace, &model_id, false) {
        Some(r) => r,
        None => {
            return PolicyCheckOutcome {
                text: None,
                exit_code: 1,
                stderr_lines: vec!["al-sem policy check: workspace did not resolve".to_string()],
                out_path: None,
            };
        }
    };

    // analyzeWorkspace diagnostics (run the default detector set so detector-stage
    // diagnostics surface exactly as al-sem does).
    let diag_lines = analyze_workspace_diagnostic_lines(opts.workspace, &resolved);

    // --strict: any error-severity diagnostic → exit 1 (stderr the diagnostics).
    if opts.strict && diag_lines.iter().any(|l| l.starts_with("error:")) {
        return PolicyCheckOutcome {
            text: None,
            exit_code: 1,
            stderr_lines: diag_lines,
            out_path: None,
        };
    }

    // Resolve the effective policy.
    let (policy_opt, source): (Option<PolicyDoc>, String) =
        match resolve_policy_check(opts.workspace, opts.policy_path, opts.no_policy) {
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
            // ⟨C1 Task 3 — R2⟩ The ONE call site that ORs in `RAW_INHERITED_FACTS`.
            // `policy_engine::select_facts` iterates the real inherited
            // `CapabilityFact` objects for `facts: inherited | any` rules, so this
            // path must compose the cone under `ConeOutput::Both`. The bit is
            // deliberately NOT in `substrate::ALL` — every other `ALL` caller
            // (`gate/events.rs`, `l5/digest_cli.rs`, `l5/prove.rs`) reads only
            // coverage / spans / the derived substrate, and folding it into `ALL`
            // would re-materialize ~10.9 GB of cone Vecs on all of them.
            let ctx = build_detector_context(
                &resolved,
                crate::engine::l5::registry::substrate::ALL
                    | crate::engine::l5::registry::substrate::RAW_INHERITED_FACTS,
            );
            let root_classifications = classify_roots(&resolved.workspace);
            let ws = &resolved.workspace;
            let model = PolicyModel::new(
                &ws.routines,
                &ws.objects,
                &ws.tables,
                &ctx.event_graph.events,
                &root_classifications,
                &ctx.summaries,
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

    let text = match opts.format {
        "json" => format_policy_json(&result, opts.driver_version, opts.deterministic),
        "sarif" => format_policy_sarif(&result, opts.driver_version),
        _ => format_policy_human(&result),
    };

    // Exit 0 always; stderr = analyzer diagnostics (after writing output).
    PolicyCheckOutcome {
        text: Some(text),
        exit_code: 0,
        stderr_lines: diag_lines,
        out_path: opts.out.map(|s| s.to_string()),
    }
}

/// `analyzeWorkspace`-equivalent diagnostic lines (mirrors events.rs).
fn analyze_workspace_diagnostic_lines(
    workspace: &Path,
    resolved: &crate::engine::l3::l3_workspace::L3Resolved,
) -> Vec<String> {
    let default_detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| DEFAULT_DETECTOR_NAMES.contains(&d.name.as_str()))
        .collect();
    compute_analyzer_diagnostics(workspace, resolved, &default_detectors)
        .iter()
        .map(|d| format!("{}: {}", d.severity, d.message))
        .collect()
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
