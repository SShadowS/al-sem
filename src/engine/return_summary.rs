//! Per-routine returnability summary — Rust port of al-sem's
//! `src/engine/return-summary.ts` (spec §J5).
//!
//! Answers: does this routine have a normal exit path (fall-off-end or `exit`)?
//! Or does every path end in `Error()`?
//!
//! The public entry-point is [`compute_return_summaries`], which dual-keys its
//! output by internal `RoutineId` (slash form) AND `StableRoutineId` (colon#hash
//! form). `project_r4f_return_summaries` converts that map to the
//! golden-shaped stable projection.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use crate::engine::l3::l3_workspace::{L3Object, L3Routine};
use crate::program::attributes::{find_attribute, has_attribute, qualified_arg};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-routine returnability summary (spec §J5).
///
/// The bool | "unknown" union is represented as a `serde_json::Value`
/// (serialises as JSON true / false / "unknown") to byte-match al-sem.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutineReturnSummary {
    /// true  — at least one normal-return path exists.
    /// false — every path ends in Error() (always-error routine).
    /// "unknown" — body unavailable or structure opaque.
    pub has_normal_return_path: JsonValue, // true | false | "unknown"
    /// true  — every reachable path terminates via Error().
    /// false — at least one path reaches a normal exit.
    /// "unknown" — body unavailable or indeterminate.
    pub all_paths_error: JsonValue, // true | false | "unknown"
    /// True when the routine carries `[TryFunction]`.
    pub has_try_function_boundary: bool,
    /// True when the routine carries `[ErrorBehavior(ErrorBehavior::Collect)]`.
    pub has_error_behavior_collect: bool,
    /// "resolved" — body available and summary computed structurally.
    /// "partial"  — body unavailable or TryFunction (semantics opaque).
    pub coverage: &'static str,
    /// "normal" | "ignore" | "error"
    pub commit_behavior: &'static str,
}

// ---------------------------------------------------------------------------
// The stable golden-output struct
// ---------------------------------------------------------------------------

/// One row in the return-summaries stable projection — matches al-sem's golden
/// `routineReturnSummaries` shape EXACTLY (field order is the serialization
/// order that serde_json::to_string_pretty produces).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StableReturnSummary {
    /// StableRoutineId (`appGuid:ObjectType:ObjectNumber#sigHash`).
    pub routine_id: String,
    /// true | false | "unknown"
    pub has_normal_return_path: JsonValue,
    /// true | false | "unknown"
    pub all_paths_error: JsonValue,
    pub has_try_function_boundary: bool,
    pub has_error_behavior_collect: bool,
    pub coverage: String,
    pub commit_behavior: String,
}

/// The return-summaries stable projection document (matches the al-sem golden).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct R4FReturnSummaryProjection {
    pub fixture_name: String,
    pub summary_count: usize,
    pub summaries: Vec<StableReturnSummary>,
}

// ---------------------------------------------------------------------------
// Internal CFG-walker types
// ---------------------------------------------------------------------------

/// Reachability of a CFN subtree: can control flow past the end (`has_normal`)?
/// Do ALL paths end in Error() (`all_error`)?
#[derive(Debug, Clone, Copy)]
struct SubtreeReach {
    has_normal: bool,
    all_error: bool,
}

/// Walk a `PCFNNode` subtree and return its reachability.
///
/// EXACTLY mirrors al-sem's `walkSubtree` function, case by case.
fn walk_subtree(node: &crate::engine::l2::features::PCFNNode) -> SubtreeReach {
    match node.kind.as_str() {
        "error" => SubtreeReach {
            has_normal: false,
            all_error: true,
        },

        "exit" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },

        "op" | "call" | "other" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },

        "block" => {
            let children = node.children.as_deref().unwrap_or(&[]);
            if children.is_empty() {
                return SubtreeReach {
                    has_normal: true,
                    all_error: false,
                };
            }
            for child in children {
                let child_reach = walk_subtree(child);
                if child_reach.all_error {
                    return SubtreeReach {
                        has_normal: false,
                        all_error: true,
                    };
                }
            }
            SubtreeReach {
                has_normal: true,
                all_error: false,
            }
        }

        "if" => {
            let then_children = node.children.as_deref().unwrap_or(&[]);
            let else_children = node.else_children.as_deref().unwrap_or(&[]);

            let then_reach = if !then_children.is_empty() {
                walk_subtree_list(then_children)
            } else {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            };

            let else_reach = if !else_children.is_empty() {
                walk_subtree_list(else_children)
            } else {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            };

            SubtreeReach {
                has_normal: then_reach.has_normal || else_reach.has_normal,
                all_error: then_reach.all_error && else_reach.all_error,
            }
        }

        "case" => {
            let branches = node.children.as_deref().unwrap_or(&[]);
            if branches.is_empty() {
                return SubtreeReach {
                    has_normal: true,
                    all_error: false,
                };
            }

            let has_else_branch = node
                .else_children
                .as_ref()
                .map(|v| !v.is_empty())
                .unwrap_or(false);

            let mut all_branches_error = true;
            let mut some_normal = false;

            for branch in branches {
                let reach = walk_subtree(branch);
                if reach.has_normal {
                    some_normal = true;
                }
                if !reach.all_error {
                    all_branches_error = false;
                }
            }

            if has_else_branch {
                let else_reach = walk_subtree_list(node.else_children.as_deref().unwrap_or(&[]));
                if else_reach.has_normal {
                    some_normal = true;
                }
                if !else_reach.all_error {
                    all_branches_error = false;
                }
            } else {
                // No else → no-match path falls through normally.
                some_normal = true;
                all_branches_error = false;
            }

            SubtreeReach {
                has_normal: some_normal,
                all_error: all_branches_error,
            }
        }

        "case-branch" => {
            let body = node.children.as_deref().unwrap_or(&[]);
            walk_subtree_list(body)
        }

        "while" | "for" | "foreach" => {
            // Loop body may execute 0 times → conservatively falls through.
            SubtreeReach {
                has_normal: true,
                all_error: false,
            }
        }

        "repeat" => {
            // Repeat-until: body always executes at least once.
            let body = node.children.as_deref().unwrap_or(&[]);
            let body_reach = walk_subtree_list(body);
            if body_reach.all_error {
                SubtreeReach {
                    has_normal: false,
                    all_error: true,
                }
            } else {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            }
        }

        "try" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },

        _ => {
            // Unknown / unrecognised node kind — conservative fall-through.
            SubtreeReach {
                has_normal: true,
                all_error: false,
            }
        }
    }
}

/// Walk a list of siblings as a sequential block. Mirrors `walkSubtreeList`.
fn walk_subtree_list(nodes: &[crate::engine::l2::features::PCFNNode]) -> SubtreeReach {
    if nodes.is_empty() {
        return SubtreeReach {
            has_normal: true,
            all_error: false,
        };
    }
    for node in nodes {
        let reach = walk_subtree(node);
        if reach.all_error {
            return SubtreeReach {
                has_normal: false,
                all_error: true,
            };
        }
    }
    SubtreeReach {
        has_normal: true,
        all_error: false,
    }
}

// ---------------------------------------------------------------------------
// Attribute parsers
// ---------------------------------------------------------------------------

/// Parse `[CommitBehavior]` from `attributes_parsed`. Returns "normal" | "ignore"
/// | "error". Mirrors al-sem's `parseCommitBehavior` with the dual-shape fallback
/// (native qualified_enum_value → `member`; dep ABI bare-Value → `value`/`text`).
pub fn parse_commit_behavior(attrs: &[crate::program::attributes::AttributeInfo]) -> &'static str {
    let Some(attr) = find_attribute(attrs, "CommitBehavior") else {
        return "normal";
    };

    // Native shape: qualified_enum_value → read the member.
    let mut member: Option<String> = qualified_arg(attr, 0).map(|qa| qa.member);

    // Dep ABI shape: bare identifier value (e.g. "Ignore"/"Error"). Fall back to
    // the arg's value/text when the qualified read did not apply.
    if member.is_none()
        && let Some(arg0) = attr.args.first()
    {
        member = arg0.value.clone().or_else(|| Some(arg0.text.clone()));
    }

    let Some(m) = member else {
        return "normal";
    };
    match m.to_lowercase().as_str() {
        "ignore" => "ignore",
        "error" => "error",
        _ => "normal",
    }
}

/// Parse `[ErrorBehavior]` and return true when the routine has
/// `ErrorBehavior::Collect`. Mirrors `parseHasErrorBehaviorCollect`.
pub fn parse_has_error_behavior_collect(
    attrs: &[crate::program::attributes::AttributeInfo],
) -> bool {
    let Some(attr) = find_attribute(attrs, "ErrorBehavior") else {
        return false;
    };

    // Native shape: qualified_enum_value → read the member.
    let mut member: Option<String> = qualified_arg(attr, 0).map(|qa| qa.member);

    // Dep ABI shape: bare identifier value (e.g. "Collect"). Fall back to
    // the arg's value/text when the qualified read did not apply.
    if member.is_none()
        && let Some(arg0) = attr.args.first()
    {
        member = arg0.value.clone().or_else(|| Some(arg0.text.clone()));
    }

    member
        .map(|m| m.to_lowercase() == "collect")
        .unwrap_or(false)
}

/// Merge object-level `InherentCommitBehavior` into a routine's `commitBehavior`.
///
/// Routine-level `[CommitBehavior]` WINS: only apply the object property when the
/// routine has no explicit override (i.e., routineCB == "normal"). Mirrors
/// `mergeInherentCommitBehavior`.
pub fn merge_inherent_commit_behavior(
    routine_commit_behavior: &'static str,
    object_behavior: Option<&str>,
) -> &'static str {
    if routine_commit_behavior != "normal" {
        return routine_commit_behavior;
    }
    match object_behavior {
        Some("ignore") => "ignore",
        Some("error") => "error",
        // "allow" or None → no override; stays "normal".
        _ => "normal",
    }
}

// ---------------------------------------------------------------------------
// Per-routine summary
// ---------------------------------------------------------------------------

/// Compute the `RoutineReturnSummary` for one routine. Mirrors
/// `computeRoutineReturnSummary`.
pub fn compute_routine_return_summary(routine: &L3Routine) -> RoutineReturnSummary {
    let attrs = &routine.attributes_parsed;
    let has_try_function_boundary = has_attribute(attrs, "TryFunction");
    let has_error_behavior_collect = parse_has_error_behavior_collect(attrs);
    let commit_behavior = parse_commit_behavior(attrs);

    // Symbol-only / no body: unknown.
    if !routine.body_available {
        return RoutineReturnSummary {
            has_normal_return_path: JsonValue::String("unknown".to_string()),
            all_paths_error: JsonValue::String("unknown".to_string()),
            has_try_function_boundary,
            has_error_behavior_collect,
            coverage: "partial",
            commit_behavior,
        };
    }

    // TryFunction: body exists but semantics are opaque to the caller.
    if has_try_function_boundary {
        return RoutineReturnSummary {
            has_normal_return_path: JsonValue::String("unknown".to_string()),
            all_paths_error: JsonValue::String("unknown".to_string()),
            has_try_function_boundary: true,
            has_error_behavior_collect,
            coverage: "partial",
            commit_behavior,
        };
    }

    // Body available but no statement tree (parse incomplete or empty body):
    // treat as fall-off-end → normal return, not all-error.
    let Some(ref tree) = routine.statement_tree else {
        return RoutineReturnSummary {
            has_normal_return_path: JsonValue::Bool(true),
            all_paths_error: JsonValue::Bool(false),
            has_try_function_boundary,
            has_error_behavior_collect,
            coverage: "partial",
            commit_behavior,
        };
    };

    let reach = walk_subtree(tree);

    RoutineReturnSummary {
        has_normal_return_path: JsonValue::Bool(reach.has_normal),
        all_paths_error: JsonValue::Bool(reach.all_error),
        has_try_function_boundary,
        has_error_behavior_collect,
        coverage: "resolved",
        commit_behavior,
    }
}

// ---------------------------------------------------------------------------
// Bulk computation
// ---------------------------------------------------------------------------

/// Compute return summaries for all routines.
///
/// Returns a `HashMap` dual-keyed by:
///  - internal `RoutineId` (slash form: `appGuid/ObjectType/ObjectNumber/…`)
///  - `StableRoutineId` (colon#hash form: `appGuid:ObjectType:ObjectNumber#sigHash`)
///
/// Object-level `InherentCommitBehavior` is merged when `objects` is supplied
/// (routine-level `[CommitBehavior]` wins over the object property). Skip the
/// stable key when the sig hash is missing. Use for LOOKUP only — never iterate
/// into output (determinism requires the sorted stable projection below).
pub fn compute_return_summaries(
    routines: &[L3Routine],
    objects: Option<&[L3Object]>,
) -> HashMap<String, RoutineReturnSummary> {
    // Build objectId → inherentCommitBehavior lookup when objects are supplied.
    let mut object_icb_map: HashMap<String, Option<String>> = HashMap::new();
    if let Some(objs) = objects {
        for obj in objs {
            // Only store when the field is present (matches al-sem: `if (obj.inherentCommitBehavior !== undefined)`).
            object_icb_map.insert(obj.id.clone(), obj.inherent_commit_behavior.clone());
        }
    }

    let mut result: HashMap<String, RoutineReturnSummary> = HashMap::new();

    for r in routines {
        let summary = compute_routine_return_summary(r);

        // Merge object-level InherentCommitBehavior if present and routine has no override.
        let effective_summary = if let Some(icb_opt) = object_icb_map.get(&r.object_id) {
            let merged_cb =
                merge_inherent_commit_behavior(summary.commit_behavior, icb_opt.as_deref());
            if merged_cb != summary.commit_behavior {
                RoutineReturnSummary {
                    commit_behavior: merged_cb,
                    ..summary.clone()
                }
            } else {
                summary.clone()
            }
        } else {
            summary.clone()
        };

        // Key by internal RoutineId (slash format).
        result.insert(r.id.clone(), effective_summary.clone());

        // Key by StableRoutineId (colon#hash format) — skip when hash missing.
        if !r.normalized_signature_hash.is_empty() {
            result.insert(r.stable_routine_id.clone(), effective_summary);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Stable projection
// ---------------------------------------------------------------------------

/// Project the return-summaries map to the golden-shaped stable projection.
///
/// Only workspace-SOURCE routines with a non-empty `stable_routine_id` are
/// emitted — matches al-sem's `computeReturnSummaries` semantics (skip stable key
/// when sig hash missing). Sorted by stable routineId ascending.
pub fn project_r4f_return_summaries(
    resolved: &crate::engine::l3::l3_workspace::L3Resolved,
    fixture_name: &str,
) -> R4FReturnSummaryProjection {
    let summaries_map = compute_return_summaries(
        &resolved.workspace.routines,
        Some(&resolved.workspace.objects),
    );

    // Collect stable-keyed entries: one per routine with a non-empty stable id.
    let mut stable: Vec<StableReturnSummary> = Vec::new();
    for r in &resolved.workspace.routines {
        if r.normalized_signature_hash.is_empty() {
            continue;
        }
        let stable_id = &r.stable_routine_id;
        if stable_id.is_empty() {
            continue;
        }
        // Look up by internal id to get the effective summary (with ICB merged).
        let Some(summary) = summaries_map.get(&r.id) else {
            continue;
        };
        stable.push(StableReturnSummary {
            routine_id: stable_id.clone(),
            has_normal_return_path: summary.has_normal_return_path.clone(),
            all_paths_error: summary.all_paths_error.clone(),
            has_try_function_boundary: summary.has_try_function_boundary,
            has_error_behavior_collect: summary.has_error_behavior_collect,
            coverage: summary.coverage.to_string(),
            commit_behavior: summary.commit_behavior.to_string(),
        });
    }

    stable.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    R4FReturnSummaryProjection {
        fixture_name: fixture_name.to_string(),
        summary_count: stable.len(),
        summaries: stable,
    }
}

// ---------------------------------------------------------------------------
// Native oracle tests (CRITICAL — exercises branches the differential cannot reach)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l2::features::PCFNNode;
    use crate::engine::l3::l3_workspace::RoutineVariables;
    use crate::program::attributes::{AttributeArg, AttributeInfo};

    // Helper: build a minimal AttributeInfo for a given name + one qualified arg.
    fn attr_qualified(name: &str, qualifier: &str, member: &str) -> AttributeInfo {
        AttributeInfo {
            name: name.to_string(),
            args: vec![AttributeArg {
                kind: "qualified_enum_value".to_string(),
                text: format!("{qualifier}::{member}"),
                value: Some(member.to_string()),
                qualifier: Some(qualifier.to_string()),
                member: Some(member.to_string()),
            }],
            raw: format!("[{name}({qualifier}::{member})]"),
        }
    }

    // Helper: build a minimal AttributeInfo for a dep-ABI bare-Value arg.
    fn attr_abi_bare(name: &str, value: &str) -> AttributeInfo {
        AttributeInfo {
            name: name.to_string(),
            args: vec![AttributeArg {
                kind: "identifier".to_string(),
                text: value.to_string(),
                value: Some(value.to_string()),
                qualifier: None,
                member: None,
            }],
            raw: format!("[{name}({value})]"),
        }
    }

    // Helper: build a simple CFN leaf node.
    fn cfn_leaf(kind: &str) -> PCFNNode {
        PCFNNode {
            kind: kind.to_string(),
            operation_id: None,
            callsite_id: None,
            condition_guard: None,
            condition_leaves: None,
            children: None,
            else_children: None,
            is_case_else: false,
            source_range: None,
        }
    }

    fn cfn_with_children(kind: &str, children: Vec<PCFNNode>) -> PCFNNode {
        PCFNNode {
            kind: kind.to_string(),
            operation_id: None,
            callsite_id: None,
            condition_guard: None,
            condition_leaves: None,
            children: Some(children),
            else_children: None,
            is_case_else: false,
            source_range: None,
        }
    }

    fn cfn_with_else(
        kind: &str,
        children: Vec<PCFNNode>,
        else_children: Vec<PCFNNode>,
    ) -> PCFNNode {
        PCFNNode {
            kind: kind.to_string(),
            operation_id: None,
            callsite_id: None,
            condition_guard: None,
            condition_leaves: None,
            children: Some(children),
            else_children: Some(else_children),
            is_case_else: false,
            source_range: None,
        }
    }

    // -----------------------------------------------------------------------
    // (a) CommitBehavior parsing — native qualified + dep ABI bare-Value shapes.
    // -----------------------------------------------------------------------

    #[test]
    fn commit_behavior_ignore_qualified() {
        let attrs = vec![attr_qualified("CommitBehavior", "CommitBehavior", "Ignore")];
        assert_eq!(parse_commit_behavior(&attrs), "ignore");
    }

    #[test]
    fn commit_behavior_error_qualified() {
        let attrs = vec![attr_qualified("CommitBehavior", "CommitBehavior", "Error")];
        assert_eq!(parse_commit_behavior(&attrs), "error");
    }

    #[test]
    fn commit_behavior_abi_bare_ignore() {
        // Dep ABI shape: bare identifier with value="Ignore", no member.
        let attrs = vec![attr_abi_bare("CommitBehavior", "Ignore")];
        assert_eq!(parse_commit_behavior(&attrs), "ignore");
    }

    #[test]
    fn commit_behavior_absent() {
        assert_eq!(parse_commit_behavior(&[]), "normal");
    }

    // -----------------------------------------------------------------------
    // (b) TryFunction → hasNormalReturnPath "unknown" + coverage "partial".
    // -----------------------------------------------------------------------

    #[test]
    fn try_function_boundary_opaque() {
        let attrs = vec![AttributeInfo {
            name: "TryFunction".to_string(),
            args: vec![],
            raw: "[TryFunction]".to_string(),
        }];
        // Simulate body_available = true, statement_tree = Some(block with error)
        // but TryFunction should short-circuit to unknown/partial.
        let dummy_routine = make_routine_with_attrs(attrs, true, Some(cfn_leaf("error")));
        let summary = compute_routine_return_summary(&dummy_routine);
        assert_eq!(
            summary.has_normal_return_path,
            JsonValue::String("unknown".to_string()),
            "TryFunction should produce unknown hasNormalReturnPath"
        );
        assert_eq!(
            summary.all_paths_error,
            JsonValue::String("unknown".to_string())
        );
        assert_eq!(summary.coverage, "partial");
        assert!(summary.has_try_function_boundary);
    }

    // -----------------------------------------------------------------------
    // (c) ErrorBehavior(Collect) → hasErrorBehaviorCollect true.
    // -----------------------------------------------------------------------

    #[test]
    fn error_behavior_collect_qualified() {
        let attrs = vec![attr_qualified("ErrorBehavior", "ErrorBehavior", "Collect")];
        assert!(parse_has_error_behavior_collect(&attrs));
    }

    #[test]
    fn error_behavior_collect_abi_bare() {
        let attrs = vec![attr_abi_bare("ErrorBehavior", "Collect")];
        assert!(parse_has_error_behavior_collect(&attrs));
    }

    #[test]
    fn error_behavior_absent() {
        assert!(!parse_has_error_behavior_collect(&[]));
    }

    // -----------------------------------------------------------------------
    // (d) !bodyAvailable → both "unknown" + partial.
    // -----------------------------------------------------------------------

    #[test]
    fn body_unavailable_is_unknown_partial() {
        let routine = make_routine_with_attrs(vec![], false, None);
        let summary = compute_routine_return_summary(&routine);
        assert_eq!(
            summary.has_normal_return_path,
            JsonValue::String("unknown".to_string())
        );
        assert_eq!(
            summary.all_paths_error,
            JsonValue::String("unknown".to_string())
        );
        assert_eq!(summary.coverage, "partial");
    }

    // -----------------------------------------------------------------------
    // (e) statement_tree None but bodyAvailable → {true, false} + partial.
    // -----------------------------------------------------------------------

    #[test]
    fn body_available_no_tree_is_normal_partial() {
        let routine = make_routine_with_attrs(vec![], true, None);
        let summary = compute_routine_return_summary(&routine);
        assert_eq!(summary.has_normal_return_path, JsonValue::Bool(true));
        assert_eq!(summary.all_paths_error, JsonValue::Bool(false));
        assert_eq!(summary.coverage, "partial");
    }

    // -----------------------------------------------------------------------
    // (f) merge_inherent_commit_behavior precedence.
    // -----------------------------------------------------------------------

    #[test]
    fn merge_icb_routine_normal_object_ignore() {
        // routine "normal" + object "ignore" → "ignore"
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("ignore")),
            "ignore"
        );
    }

    #[test]
    fn merge_icb_routine_error_object_ignore() {
        // routine "error" + object "ignore" → "error" (routine wins)
        assert_eq!(
            merge_inherent_commit_behavior("error", Some("ignore")),
            "error"
        );
    }

    #[test]
    fn merge_icb_routine_ignore_object_error() {
        // routine "ignore" + object "error" → "ignore" (routine wins)
        assert_eq!(
            merge_inherent_commit_behavior("ignore", Some("error")),
            "ignore"
        );
    }

    #[test]
    fn merge_icb_object_allow_stays_normal() {
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("allow")),
            "normal"
        );
    }

    #[test]
    fn merge_icb_object_none_stays_normal() {
        assert_eq!(merge_inherent_commit_behavior("normal", None), "normal");
    }

    // -----------------------------------------------------------------------
    // (g) walk_subtree structural cases.
    // -----------------------------------------------------------------------

    /// if where both branches error → allError true.
    #[test]
    fn walk_if_both_branches_error() {
        // if { error } else { error }
        let node = cfn_with_else("if", vec![cfn_leaf("error")], vec![cfn_leaf("error")]);
        let reach = walk_subtree(&node);
        assert!(reach.all_error, "both if-branches error → allError true");
        assert!(!reach.has_normal);
    }

    /// if where then errors but no else → has_normal true (else fallthrough).
    #[test]
    fn walk_if_no_else_has_normal() {
        let node = cfn_with_children("if", vec![cfn_leaf("error")]);
        let reach = walk_subtree(&node);
        assert!(reach.has_normal, "if with no else: fallthrough path exists");
        assert!(!reach.all_error);
    }

    /// case with no else → someNormal true (no-match path).
    #[test]
    fn walk_case_no_else_has_normal() {
        let branch = cfn_with_children("case-branch", vec![cfn_leaf("error")]);
        let node = cfn_with_children("case", vec![branch]);
        // No else_children → no-match path.
        let reach = walk_subtree(&node);
        assert!(
            reach.has_normal,
            "case without else: no-match path always falls through"
        );
        assert!(!reach.all_error);
    }

    /// case with else where all branches (incl. else) error → allBranchesError true.
    #[test]
    fn walk_case_with_else_all_error() {
        let branch = cfn_with_children("case-branch", vec![cfn_leaf("error")]);
        let mut node = cfn_with_children("case", vec![branch]);
        node.else_children = Some(vec![cfn_leaf("error")]);
        let reach = walk_subtree(&node);
        assert!(
            reach.all_error,
            "case with else where all branches error → allError true"
        );
        assert!(!reach.has_normal);
    }

    /// repeat whose body errors → {false, true}.
    #[test]
    fn walk_repeat_body_errors() {
        let node = cfn_with_children("repeat", vec![cfn_leaf("error")]);
        let reach = walk_subtree(&node);
        assert!(
            !reach.has_normal,
            "repeat with all-error body: first iteration always errors"
        );
        assert!(reach.all_error);
    }

    /// repeat whose body is normal → {true, false}.
    #[test]
    fn walk_repeat_body_normal() {
        let node = cfn_with_children("repeat", vec![cfn_leaf("op")]);
        let reach = walk_subtree(&node);
        assert!(reach.has_normal);
        assert!(!reach.all_error);
    }

    /// block with an error child early terminates the block.
    #[test]
    fn walk_block_early_terminate() {
        let node = cfn_with_children(
            "block",
            vec![cfn_leaf("op"), cfn_leaf("error"), cfn_leaf("op")],
        );
        let reach = walk_subtree(&node);
        assert!(!reach.has_normal);
        assert!(reach.all_error);
    }

    /// while/for/foreach always fallthrough (0-iteration).
    #[test]
    fn walk_loops_always_fallthrough() {
        for kind in ["while", "for", "foreach"] {
            let node = cfn_with_children(kind, vec![cfn_leaf("error")]);
            let reach = walk_subtree(&node);
            assert!(
                reach.has_normal,
                "{kind} with error body still falls through (0 iterations)"
            );
            assert!(!reach.all_error);
        }
    }

    /// exit node → hasNormal true, allError false.
    #[test]
    fn walk_exit_is_normal() {
        let reach = walk_subtree(&cfn_leaf("exit"));
        assert!(reach.has_normal);
        assert!(!reach.all_error);
    }

    /// error node → hasNormal false, allError true.
    #[test]
    fn walk_error_is_all_error() {
        let reach = walk_subtree(&cfn_leaf("error"));
        assert!(!reach.has_normal);
        assert!(reach.all_error);
    }

    // -----------------------------------------------------------------------
    // Helper to build a minimal L3Routine for testing.
    // -----------------------------------------------------------------------

    fn make_routine_with_attrs(
        attrs: Vec<AttributeInfo>,
        body_available: bool,
        statement_tree: Option<PCFNNode>,
    ) -> L3Routine {
        use crate::engine::l2::features::PAnchor;
        L3Routine {
            id: "test-guid/Codeunit/50000/test-hash".to_string(),
            stable_routine_id: "test-guid:Codeunit:50000#test-hash".to_string(),
            object_id: "test-guid/Codeunit/50000".to_string(),
            object_type: "Codeunit".to_string(),
            name: "TestProcedure".to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: attrs,
            app_guid: "test-guid".to_string(),
            object_number: 50000,
            normalized_signature_hash: "test-hash".to_string(),
            body_available,
            parse_incomplete: false,
            record_variables: vec![],
            record_operations: vec![],
            field_accesses: vec![],
            variables: RoutineVariables::default(),
            parameters: vec![],
            access_modifier: None,
            return_type: None,
            call_sites: vec![],
            operation_sites: vec![],
            statement_tree,
            loops: vec![],
            source_anchor: PAnchor {
                source_unit_id: "ws:test.al".to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "procedure".to_string(),
            },
            identifier_references: vec![],
            unreachable_statements: vec![],
            has_branching: false,
            var_assignments: vec![],
            condition_references: vec![],
            enclosing_member: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
        }
    }

    // Minimal L3Object matching make_routine_with_attrs's object_id, with a given ICB.
    fn make_object_with_icb(inherent_commit_behavior: Option<&str>) -> L3Object {
        L3Object {
            id: "test-guid/Codeunit/50000".to_string(),
            app_guid: "test-guid".to_string(),
            object_type: "Codeunit".to_string(),
            object_number: 50000,
            name: "TestCodeunit".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: inherent_commit_behavior.map(str::to_string),
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
        }
    }

    // -----------------------------------------------------------------------
    // Summary-level threading of CommitBehavior / ErrorBehavior — the corpus
    // never carries these attributes, so prove the parse→summary wiring here.
    // -----------------------------------------------------------------------

    #[test]
    fn commit_behavior_threads_into_summary() {
        let routine = make_routine_with_attrs(
            vec![attr_qualified("CommitBehavior", "CommitBehavior", "Ignore")],
            true,
            Some(cfn_leaf("op")),
        );
        assert_eq!(
            compute_routine_return_summary(&routine).commit_behavior,
            "ignore"
        );
    }

    #[test]
    fn error_behavior_collect_threads_into_summary() {
        let routine = make_routine_with_attrs(
            vec![attr_qualified("ErrorBehavior", "ErrorBehavior", "Collect")],
            true,
            Some(cfn_leaf("op")),
        );
        assert!(compute_routine_return_summary(&routine).has_error_behavior_collect);
    }

    // -----------------------------------------------------------------------
    // Bulk ICB-merge end-to-end (compute_return_summaries) — object-level
    // InherentCommitBehavior merged in; routine-level [CommitBehavior] wins.
    // -----------------------------------------------------------------------

    #[test]
    fn bulk_icb_merge_object_level_applies() {
        // Routine with NO [CommitBehavior]; object declares InherentCommitBehavior=Ignore.
        let routine = make_routine_with_attrs(vec![], true, Some(cfn_leaf("op")));
        let obj = make_object_with_icb(Some("ignore"));
        let map = compute_return_summaries(
            std::slice::from_ref(&routine),
            Some(std::slice::from_ref(&obj)),
        );
        // Both the internal and stable keys carry the merged summary.
        assert_eq!(map.get(&routine.id).unwrap().commit_behavior, "ignore");
        assert_eq!(
            map.get(&routine.stable_routine_id).unwrap().commit_behavior,
            "ignore"
        );
    }

    #[test]
    fn bulk_icb_merge_routine_attr_wins_over_object() {
        // Routine [CommitBehavior(Error)] + object Ignore → routine wins → "error".
        let routine = make_routine_with_attrs(
            vec![attr_qualified("CommitBehavior", "CommitBehavior", "Error")],
            true,
            Some(cfn_leaf("op")),
        );
        let obj = make_object_with_icb(Some("ignore"));
        let map = compute_return_summaries(
            std::slice::from_ref(&routine),
            Some(std::slice::from_ref(&obj)),
        );
        assert_eq!(map.get(&routine.id).unwrap().commit_behavior, "error");
    }
}
