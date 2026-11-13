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

use al_syntax::IdentifierFoldExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use crate::program::attributes::{
    find_attribute, find_definite_attribute, has_attribute, has_definite_attribute, qualified_arg,
};
use crate::program::model::workspace::{ModelObject, ModelRoutine};

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
    /// True when the routine MAY carry `[TryFunction]` (some build has it):
    /// the cautious reading for a reader that degrades on a try boundary.
    pub has_try_function_boundary: bool,
    /// True when that `[TryFunction]` is only in some builds (engine limits
    /// B2, `AttributeInfo::conditional`). A reader that decides a definite
    /// answer from the boundary (`ordering_engine`'s proven-errors rule) needs
    /// `has_try_function_boundary && !try_function_conditional`.
    pub try_function_conditional: bool,
    /// True when the routine carries `[ErrorBehavior(ErrorBehavior::Collect)]`.
    pub has_error_behavior_collect: bool,
    /// "resolved" — body available and summary computed structurally.
    /// "partial"  — body unavailable or TryFunction (semantics opaque).
    pub coverage: &'static str,
    /// "normal" | "ignore" | "error" | "conditional" (only some builds declare
    /// `Ignore`/`Error`, see [`parse_commit_behavior`]).
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
fn walk_subtree(node: &crate::program::body::features::PCFNNode) -> SubtreeReach {
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
fn walk_subtree_list(nodes: &[crate::program::body::features::PCFNNode]) -> SubtreeReach {
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
///
/// Engine limits B2: only an attribute every build of the routine has decides
/// it. When only conditional ones (`AttributeInfo::conditional`) declare
/// `Ignore` or `Error`, the answer is `"conditional"`: some build may run
/// under it, another may not, so no reader may treat the routine as ignoring
/// (or erroring on) its commits for certain.
pub fn parse_commit_behavior(attrs: &[crate::program::attributes::AttributeInfo]) -> &'static str {
    if let Some(attr) = find_definite_attribute(attrs, "CommitBehavior") {
        return commit_behavior_value(attr);
    }
    let conditional = attrs.iter().any(|a| {
        !a.in_every_build()
            && a.name.eq_fold_identifier("CommitBehavior")
            && commit_behavior_value(a) != "normal"
    });
    if conditional { "conditional" } else { "normal" }
}

/// One `[CommitBehavior]` attribute's value: "normal" | "ignore" | "error".
pub fn commit_behavior_value(attr: &crate::program::attributes::AttributeInfo) -> &'static str {
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
///
/// Engine limits B2: `object_conditional` (`ModelObject::
/// inherent_commit_behavior_conditional`) says only some builds declare the
/// property, so a build without it runs its commits normally: an `ignore` or
/// `error` reading is then only `"conditional"`, which proves nothing either way.
pub fn merge_inherent_commit_behavior(
    routine_commit_behavior: &'static str,
    object_behavior: Option<&str>,
    object_conditional: bool,
) -> &'static str {
    if routine_commit_behavior != "normal" {
        return routine_commit_behavior;
    }
    match object_behavior {
        Some("ignore" | "error") if object_conditional => "conditional",
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
pub fn compute_routine_return_summary(routine: &ModelRoutine) -> RoutineReturnSummary {
    let attrs = &routine.attributes_parsed;
    // "May": a TryFunction in some build makes the body opaque (B2).
    let has_try_function_boundary = has_attribute(attrs, "TryFunction");
    let try_function_conditional =
        has_try_function_boundary && !has_definite_attribute(attrs, "TryFunction");
    let has_error_behavior_collect = parse_has_error_behavior_collect(attrs);
    let commit_behavior = parse_commit_behavior(attrs);

    // Symbol-only / no body: unknown.
    if !routine.body_available {
        return RoutineReturnSummary {
            has_normal_return_path: JsonValue::String("unknown".to_string()),
            all_paths_error: JsonValue::String("unknown".to_string()),
            has_try_function_boundary,
            try_function_conditional,
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
            try_function_conditional,
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
            try_function_conditional,
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
        try_function_conditional,
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
    routines: &[ModelRoutine],
    objects: Option<&[ModelObject]>,
) -> HashMap<String, RoutineReturnSummary> {
    // Each routine's DECLARING object, for its InherentCommitBehavior (#279: an
    // `#if`-split id has one row per arm).
    let declaring = objects.map(crate::program::model::declaring::DeclaringObjects::new);

    let mut result: HashMap<String, RoutineReturnSummary> = HashMap::new();

    for r in routines {
        let summary = compute_routine_return_summary(r);

        // Merge object-level InherentCommitBehavior if present and routine has no override.
        let effective_summary = if let Some(obj) = declaring.as_ref().and_then(|d| d.of(r)) {
            let merged_cb = merge_inherent_commit_behavior(
                summary.commit_behavior,
                obj.inherent_commit_behavior.as_deref(),
                obj.inherent_commit_behavior_conditional,
            );
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

        // Engine limits B2 (wave 2 final review): an id declared more than
        // once (`#if` arms, or two files) is one summary over every row; the
        // last row alone decided it.
        let effective_summary = match result.get(&r.id) {
            Some(earlier) => merge_alternative_summaries(earlier, &effective_summary),
            None => effective_summary,
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

/// The summary of one routine id whose rows `a` and `b` are alternative
/// builds (two `#if` arms, or two files; engine limits B2): a value they
/// disagree on is not known (`"unknown"`, and `"conditional"` for the commit
/// behavior), a "may" flag holds when either has it, and the coverage is
/// `partial` when either is.
#[must_use]
pub fn merge_alternative_summaries(
    a: &RoutineReturnSummary,
    b: &RoutineReturnSummary,
) -> RoutineReturnSummary {
    let agree = |x: &JsonValue, y: &JsonValue| {
        if x == y {
            x.clone()
        } else {
            JsonValue::String("unknown".to_string())
        }
    };
    RoutineReturnSummary {
        has_normal_return_path: agree(&a.has_normal_return_path, &b.has_normal_return_path),
        all_paths_error: agree(&a.all_paths_error, &b.all_paths_error),
        has_try_function_boundary: a.has_try_function_boundary || b.has_try_function_boundary,
        try_function_conditional: a.try_function_conditional
            || b.try_function_conditional
            || a.has_try_function_boundary != b.has_try_function_boundary,
        has_error_behavior_collect: a.has_error_behavior_collect || b.has_error_behavior_collect,
        coverage: if a.coverage == "partial" || b.coverage == "partial" {
            "partial"
        } else {
            a.coverage
        },
        commit_behavior: if a.commit_behavior == b.commit_behavior {
            a.commit_behavior
        } else {
            "conditional"
        },
    }
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
    resolved: &crate::program::model::workspace::Model,
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
    use crate::program::attributes::{AttributeArg, AttributeInfo};
    use crate::program::body::features::PCFNNode;
    use crate::program::model::workspace::RoutineVariables;

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
            conditional: false,
            another_declaration_lacks: false,
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
            conditional: false,
            another_declaration_lacks: false,
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
            path_hint: None,
            table_case: None,
            field_exit_guard: None,
            table_exit_guard: None,
            junction_guards: None,
            value_fact: None,
            flow: None,
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
            path_hint: None,
            table_case: None,
            field_exit_guard: None,
            table_exit_guard: None,
            junction_guards: None,
            value_fact: None,
            flow: None,
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
            path_hint: None,
            table_case: None,
            field_exit_guard: None,
            table_exit_guard: None,
            junction_guards: None,
            value_fact: None,
            flow: None,
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
            conditional: false,
            another_declaration_lacks: false,
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
        assert!(!summary.try_function_conditional);
    }

    /// Engine limits B2 (wave 2 final review, item 4): the model join and the
    /// summary merge read EVERY group of rows that share an id alike, a
    /// symbol-only group (an ABI overload collapse) included. Stated by
    /// assignment: two symbol-only rows (`abi:` unit) of one id, one carrying
    /// `[TryFunction]` with no `#if` at all; a caller bound to the id cannot
    /// tell which overload it reaches. After the join the attribute is
    /// "another declaration lacks it" (a "may" for the id), the summary says
    /// the boundary is conditional, and the row's own reading is still
    /// definite.
    #[test]
    fn a_symbol_only_group_is_joined_like_every_group() {
        let mut try_fn = attr_abi_bare("TryFunction", "x");
        try_fn.args.clear();
        let row = |attrs: Vec<AttributeInfo>| {
            let mut r = make_routine_with_attrs(attrs, false, None);
            r.source_anchor.source_unit_id = "abi:test".to_string();
            r
        };
        let mut ws = crate::program::model::workspace::ModelEntities {
            objects: Vec::new(),
            tables: Vec::new(),
            routines: vec![row(vec![try_fn]), row(vec![])],
        };
        assert!(
            ws.routines[0].attributes_parsed[0].in_every_build(),
            "precondition: a symbol attribute, definite before the join"
        );
        crate::program::model::workspace::join_alternative_declarations(
            &mut ws,
            &[],
            &Default::default(),
        );
        let a = &ws.routines[0].attributes_parsed[0];
        assert!(a.another_declaration_lacks && !a.conditional);
        let summaries = compute_return_summaries(&ws.routines, None);
        let s = &summaries[&ws.routines[0].id];
        assert!(s.has_try_function_boundary && s.try_function_conditional);
    }

    /// Engine limits B2: a `[TryFunction]` / `[CommitBehavior]` only some
    /// builds have is "may": the body stays opaque, but neither reads as a
    /// definite declaration (stated by assignment: `conditional: true`).
    #[test]
    fn a_conditional_declaration_is_may_not_definite() {
        let mut try_fn = attr_abi_bare("TryFunction", "x");
        try_fn.args.clear();
        try_fn.conditional = true;
        let mut ignore = attr_qualified("CommitBehavior", "CommitBehavior", "Ignore");
        ignore.conditional = true;
        let routine = make_routine_with_attrs(vec![try_fn, ignore], true, Some(cfn_leaf("error")));
        let summary = compute_routine_return_summary(&routine);
        assert!(summary.has_try_function_boundary, "opaque in some build");
        assert!(summary.try_function_conditional);
        assert_eq!(summary.coverage, "partial");
        assert_eq!(summary.commit_behavior, "conditional");
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
            merge_inherent_commit_behavior("normal", Some("ignore"), false),
            "ignore"
        );
    }

    /// Engine limits B2: a property only some builds declare proves nothing
    /// either way; a routine's own declaration still wins.
    #[test]
    fn merge_icb_conditional_object_is_conditional() {
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("ignore"), true),
            "conditional"
        );
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("error"), true),
            "conditional"
        );
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("allow"), true),
            "normal"
        );
        assert_eq!(
            merge_inherent_commit_behavior("ignore", Some("error"), true),
            "ignore"
        );
    }

    #[test]
    fn merge_icb_routine_error_object_ignore() {
        // routine "error" + object "ignore" → "error" (routine wins)
        assert_eq!(
            merge_inherent_commit_behavior("error", Some("ignore"), false),
            "error"
        );
    }

    #[test]
    fn merge_icb_routine_ignore_object_error() {
        // routine "ignore" + object "error" → "ignore" (routine wins)
        assert_eq!(
            merge_inherent_commit_behavior("ignore", Some("error"), false),
            "ignore"
        );
    }

    #[test]
    fn merge_icb_object_allow_stays_normal() {
        assert_eq!(
            merge_inherent_commit_behavior("normal", Some("allow"), false),
            "normal"
        );
    }

    #[test]
    fn merge_icb_object_none_stays_normal() {
        assert_eq!(
            merge_inherent_commit_behavior("normal", None, false),
            "normal"
        );
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
    // Helper to build a minimal ModelRoutine for testing.
    // -----------------------------------------------------------------------

    fn make_routine_with_attrs(
        attrs: Vec<AttributeInfo>,
        body_available: bool,
        statement_tree: Option<PCFNNode>,
    ) -> ModelRoutine {
        use crate::program::body::features::PAnchor;
        ModelRoutine {
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
            implicit_field_accesses: vec![],
            variables: RoutineVariables::default(),
            parameters: vec![],
            access_modifier: None,
            return_type: None,
            call_sites: vec![],
            operation_sites: vec![],
            statement_tree: statement_tree.map(std::sync::Arc::new),
            loops: vec![],
            file: None,
            site_exprs: Default::default(),
            source_anchor: PAnchor {
                source_unit_id: "ws:test.al".to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "procedure".to_string(),
            },
            declaration_start_line: 0,
            dead_marks_trusted: false,
            identifier_references: vec![],
            unreachable_statements: vec![],
            has_branching: false,
            var_assignments: vec![],
            condition_references: vec![],
            enclosing_member: None,
            member_detail: None,
            originating_object: None,
            enclosing_member_range: None,
            entry_temp_guard_receiver: None,
            table_event_exits: Default::default(),
            recordref_bindings: Vec::new(),
            field_ref_operations: Vec::new(),
            rules: None,
            rule_drafts: None,
            card_text: None,
            site_guards: None,
        }
    }

    // Minimal ModelObject matching make_routine_with_attrs's object_id, with a given ICB.
    fn make_object_with_icb(inherent_commit_behavior: Option<&str>) -> ModelObject {
        ModelObject {
            id: "test-guid/Codeunit/50000".to_string(),
            app_guid: "test-guid".to_string(),
            object_type: "Codeunit".to_string(),
            object_number: 50000,
            object_key: "50000".to_string(),
            name: "TestCodeunit".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: inherent_commit_behavior.map(str::to_string),
            inherent_commit_behavior_conditional: false,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: Vec::new(),
            run_actions: None,
            enum_impls: None,
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

    /// R12 (#279): each routine merges the ICB of its OWN declaring row. Two
    /// object rows share one id BY ASSIGNMENT (an `#if` split): lines 0-10
    /// with ICB "ignore", lines 11-20 with none; one routine in each. A
    /// last-row lookup makes `a` "normal", a first-row lookup makes `b`
    /// "ignore".
    #[test]
    fn bulk_icb_merge_reads_the_declaring_arm() {
        let span = |from: u32, to: u32| crate::program::body::features::PAnchor {
            source_unit_id: "ws:f.al".to_string(),
            start_line: from,
            start_column: 0,
            end_line: to,
            end_column: 0,
            syntax_kind: String::new(),
        };
        let mut first = make_object_with_icb(Some("ignore"));
        first.source_anchor = Some(span(0, 10));
        let mut second = make_object_with_icb(None);
        second.source_anchor = Some(span(11, 20));
        assert_eq!(first.id, second.id, "precondition: one id, two rows");
        let mut a = make_routine_with_attrs(vec![], true, Some(cfn_leaf("op")));
        a.id = "a".to_string();
        a.object_id = first.id.clone();
        a.source_anchor = span(2, 4);
        let mut b = a.clone();
        b.id = "b".to_string();
        b.source_anchor = span(12, 14);
        let map = compute_return_summaries(&[a, b], Some(&[first, second]));
        assert_eq!(map["a"].commit_behavior, "ignore");
        assert_eq!(map["b"].commit_behavior, "normal");
    }
}
