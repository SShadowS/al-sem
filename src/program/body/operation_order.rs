//! L2 operation-order index walker — faithful port of al-sem
//! `src/index/operation-order.ts` (`computeOperationOrder`).
//!
//! Walks the R1a CFN skeleton (`features::PCFNNode`) in the SAME pre-order as
//! control-context (conditionLeaves BEFORE body children) and assigns an
//! `OperationOrder` (`orderId`, `frameId`, `onSuccessPath`, `dominatesSuccessReturn`)
//! to every callsiteId and operationId, plus the routine's `ScopeFrame[]` table.
//!
//! REUSES the shared branch-termination primitives in [`super::control_flow`]
//! (R1b) — does NOT re-derive them.
//!
//! This function is PURE over the CFN skeleton + the lowercased `attributesParsed`
//! names (for the TryFunction guard). The `error-call` source-range post-pass is
//! NOT here — it runs in the emitter layer (Task 3), where op/callsite anchors
//! exist. Never panics.
//!
//! Key rules (ported verbatim — do NOT re-derive or "simplify"):
//!  - `next_order_id` increments PER LEAF (in `assign_leaf`, BEFORE checking which
//!    ids are present). A leaf with BOTH ids clones the SAME `OperationOrder` into
//!    both maps; `exit`/`error` leaves consume an orderId even when not projected →
//!    emitted orderIds may have GAPS + ALIASES.
//!  - `on_success_path` = reachable AND not exclusively on an error-terminating arm.
//!    The check is `term != Error` (NOT `!terminates`): exit-arms ARE on the success
//!    path (exit is a normal return); error-arms are NOT.
//!  - `dominates_success_return` = true ONLY for a reachable root-block direct op
//!    before any normal-return-possible statement. The `"other"`/default wrapper
//!    PROPAGATES the caller's value to children; block/if/case/case-branch/loop/try
//!    + all conditionLeaves + error/exit leaves reset it to false.

use super::control_flow::{
    Termination, branch_termination, else_termination, has_explicit_else, terminates,
};
use super::features::{PCFNNode, PCallSite, PFeatures, POperationSite};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Result types (parity shape)
// ============================================================================

/// The execution-order fact assigned to an op/callsite leaf.
///
/// Serializes to al-sem's `POperationOrder` shape (camelCase, all four fields
/// always present) — `r1a-l2-projection.ts:projectOperationOrder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationOrder {
    #[serde(rename = "orderId")]
    pub order_id: u32,
    #[serde(rename = "frameId")]
    pub frame_id: i64,
    #[serde(rename = "onSuccessPath")]
    pub on_success_path: bool,
    #[serde(rename = "dominatesSuccessReturn")]
    pub dominates_success_return: bool,
}

/// A syntactic scope frame in the routine's frame table.
///
/// Branch frames (if-then / if-else / case-branch) ALWAYS carry
/// `branch_always_terminates` AND `branch_may_fall_through` (even when `false`),
/// and `branch_terminates_by` ONLY when they always-terminate. Root / loop / try
/// frames OMIT all three (`None`).
///
/// Serializes to al-sem's `PScopeFrame` shape — `r1a-l2-projection.ts:projectScopeFrame`.
/// CRITICAL: the branch flags use `skip_serializing_if = "Option::is_none"`, NOT
/// `is_false` — branch frames store `Some(false)`, which MUST emit as `false`,
/// while root/loop/try store `None`, which is omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeFrame {
    #[serde(rename = "frameId")]
    pub frame_id: i64,
    #[serde(rename = "parentFrameId")]
    pub parent_frame_id: i64,
    pub kind: String,
    // Field order MUST match al-sem operation-order.ts key-insertion order:
    // branchAlwaysTerminates (line 67), branchMayFallThrough (68), branchTerminatesBy
    // (70, LAST + conditional). A swap is byte-invisible until a branch frame carries
    // branchTerminatesBy (always-terminating branch with a known `by`).
    #[serde(
        rename = "branchAlwaysTerminates",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_always_terminates: Option<bool>,
    #[serde(
        rename = "branchMayFallThrough",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_may_fall_through: Option<bool>,
    #[serde(rename = "branchTerminatesBy", skip_serializing_if = "Option::is_none")]
    pub branch_terminates_by: Option<String>,
}

/// The operation-order maps + frame table for a routine.
pub struct OperationOrderResult {
    pub by_callsite: HashMap<String, OperationOrder>,
    pub by_operation: HashMap<String, OperationOrder>,
    pub scope_frames: Vec<ScopeFrame>,
}

// ============================================================================
// Walker accumulator
// ============================================================================

struct OrderWalkResult {
    by_callsite: HashMap<String, OperationOrder>,
    by_operation: HashMap<String, OperationOrder>,
    scope_frames: Vec<ScopeFrame>,
    /// Monotonic orderId counter.
    next_order_id: u32,
    /// Monotonic frameId counter.
    next_frame_id: i64,
}

fn children_of(node: &PCFNNode) -> &[PCFNNode] {
    node.children.as_deref().unwrap_or(&[])
}

fn else_children_of(node: &PCFNNode) -> &[PCFNNode] {
    node.else_children.as_deref().unwrap_or(&[])
}

fn condition_leaves_of(node: &PCFNNode) -> &[PCFNNode] {
    node.condition_leaves.as_deref().unwrap_or(&[])
}

// ============================================================================
// Frame stack helpers
// ============================================================================

/// Branch-termination info for a pushed branch frame.
struct BranchInfo {
    always_terminates: bool,
    by: Option<String>,
}

/// Push a new ScopeFrame onto the stack and return its frameId.
fn push_frame(
    result: &mut OrderWalkResult,
    parent_frame_id: i64,
    kind: &str,
    branch_info: Option<BranchInfo>,
) -> i64 {
    let frame_id = result.next_frame_id;
    result.next_frame_id += 1;

    let (branch_always_terminates, branch_terminates_by, branch_may_fall_through) =
        match branch_info {
            Some(info) => {
                let by = if info.always_terminates {
                    info.by
                } else {
                    None
                };
                (
                    Some(info.always_terminates),
                    by,
                    Some(!info.always_terminates),
                )
            }
            None => (None, None, None),
        };

    result.scope_frames.push(ScopeFrame {
        frame_id,
        parent_frame_id,
        kind: kind.to_string(),
        branch_always_terminates,
        branch_terminates_by,
        branch_may_fall_through,
    });
    frame_id
}

/// Convert an always-terminating `Termination` to the `branchTerminatesBy` value.
fn termination_kind(t: Termination) -> Option<String> {
    match t {
        Termination::Exit => Some("exit".to_string()),
        Termination::Error => Some("error".to_string()),
        Termination::Fallthrough => None,
    }
}

// ============================================================================
// Leaf assignment
// ============================================================================

/// Assign an OperationOrder to a leaf node (op or call/error/exit leaf).
/// Increments orderId ONCE, then records the entry into the present maps.
fn assign_leaf(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    dominates_success_return: bool,
    result: &mut OrderWalkResult,
) {
    let order_id = result.next_order_id;
    result.next_order_id += 1;
    let order = OperationOrder {
        order_id,
        frame_id,
        on_success_path,
        dominates_success_return,
    };
    if let Some(op_id) = &node.operation_id {
        result.by_operation.insert(op_id.clone(), order);
    }
    if let Some(cs_id) = &node.callsite_id {
        result.by_callsite.insert(cs_id.clone(), order);
    }
}

// ============================================================================
// Condition leaves
// ============================================================================

/// Walk a node's conditionLeaves in order. They are in expression position →
/// `dominates_success_return = false`.
fn walk_condition_leaves(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    result: &mut OrderWalkResult,
) {
    for leaf in condition_leaves_of(node) {
        collect_node(leaf, frame_id, on_success_path, false, result);
    }
}

// ============================================================================
// Block-level walker (postdominance tracking)
// ============================================================================

/// Whether a top-level statement can cause a NORMAL return (exit, or a
/// conditional that allows an exit on some arm) WITHOUT being an unconditional
/// terminator. Error-only arms do NOT count. Mirrors
/// `operation-order.ts:canNormalReturnBeforeAfter`.
fn can_normal_return_before_after(stmt: &PCFNNode) -> bool {
    if stmt.kind == "if" {
        let then_body = children_of(stmt).first();
        let then_term = then_body
            .map(branch_termination)
            .unwrap_or(Termination::Fallthrough);
        let else_term = else_termination(stmt);

        // Single-arm error-only guard: no normal return possible.
        if !has_explicit_else(stmt) && then_term == Termination::Error {
            return false;
        }
        // then-arm exits (no explicit else, else falls through): conditional
        // normal-return possible.
        if !has_explicit_else(stmt) && then_term == Termination::Exit {
            return true;
        }
        if has_explicit_else(stmt) {
            if then_term == Termination::Exit && else_term != Termination::Exit {
                return true;
            }
            if else_term == Termination::Exit && then_term != Termination::Exit {
                return true;
            }
            // Both exit: continuation unreachable (handled by reachable=false).
            // Neither exits: no normal-return via this if.
        }
        return false;
    }

    if stmt.kind == "case" {
        for branch in children_of(stmt) {
            let body = children_of(branch).first();
            let term = body
                .map(branch_termination)
                .unwrap_or(Termination::Fallthrough);
            if term == Termination::Exit {
                return true;
            }
        }
        return false;
    }

    // Loops, calls, ops, other — not a top-level conditional normal-return.
    false
}

/// Walk a "block" node's children in execution order. Returns whether control is
/// still reachable at the end of the block (false after an unconditional
/// exit/error).
///
/// `is_root_block`: true only for the routine's root block — enables
/// `dominates_success_return` tracking. Nested blocks pass false.
fn walk_block(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    is_root_block: bool,
    result: &mut OrderWalkResult,
) -> bool {
    let mut reachable = true;
    let mut normal_return_possible_before_here = false;

    let singleton = [node.clone()];
    let stmts: &[PCFNNode] = if node.kind == "block" {
        children_of(node)
    } else {
        &singleton
    };

    for stmt in stmts {
        if !reachable {
            // Everything after an unconditional exit is unreachable.
            collect_node(stmt, frame_id, false, false, result);
            continue;
        }

        // dominatesSuccessReturn for ops DIRECTLY in this root-block statement.
        let stmt_dominates = is_root_block && !normal_return_possible_before_here;

        match stmt.kind.as_str() {
            "error" => {
                walk_condition_leaves(stmt, frame_id, on_success_path, result);
                // Error never dominates a normal return.
                assign_leaf(stmt, frame_id, on_success_path, false, result);
                reachable = false;
            }
            "exit" => {
                walk_condition_leaves(stmt, frame_id, on_success_path, result);
                // exit IS the return, not an op before it.
                assign_leaf(stmt, frame_id, on_success_path, false, result);
                reachable = false;
            }
            "if" => {
                let still_reachable = walk_if_node(stmt, frame_id, on_success_path, result);
                if !still_reachable {
                    reachable = false;
                }
                // Update AFTER visiting: only LATER root statements see it.
                if is_root_block && can_normal_return_before_after(stmt) {
                    normal_return_possible_before_here = true;
                }
            }
            "case" => {
                walk_case_node(stmt, frame_id, on_success_path, result);
                if is_root_block && can_normal_return_before_after(stmt) {
                    normal_return_possible_before_here = true;
                }
            }
            "while" | "for" | "foreach" | "repeat" => {
                walk_loop_node(stmt, frame_id, on_success_path, result);
                // Loops are NOT treated as normal-return-possible (verbatim).
            }
            _ => {
                // call / op / other / try — collect at current frame.
                collect_node(stmt, frame_id, on_success_path, stmt_dominates, result);
            }
        }
    }

    reachable
}

// ============================================================================
// If walker
// ============================================================================

/// Walk an `if` node. Visits conditionLeaves (in the PARENT frame) first, then
/// each arm body in its own frame. Returns whether the continuation is still
/// reachable (false when both arms always terminate). Ops in arms NEVER dominate.
fn walk_if_node(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    result: &mut OrderWalkResult,
) -> bool {
    walk_condition_leaves(node, frame_id, on_success_path, result);

    let then_body = children_of(node).first().cloned();
    let else_body = else_children_of(node).first().cloned();

    let then_term = then_body
        .as_ref()
        .map(branch_termination)
        .unwrap_or(Termination::Fallthrough);
    let else_term = else_termination(node);

    // Single-arm error-only guard (no explicit else, then=error): body is
    // error-path (onSuccessPath=false), continuation unchanged.
    if !has_explicit_else(node) && then_term == Termination::Error {
        if let Some(then_body) = then_body {
            let then_frame_id = push_frame(
                result,
                frame_id,
                "if-then",
                Some(BranchInfo {
                    always_terminates: true,
                    by: Some("error".to_string()),
                }),
            );
            collect_node(&then_body, then_frame_id, false, false, result);
        }
        return true;
    }

    // then-arm.
    if let Some(then_body) = then_body {
        let then_always = terminates(then_term);
        let then_by = if then_always {
            termination_kind(then_term)
        } else {
            None
        };
        let then_frame_id = push_frame(
            result,
            frame_id,
            "if-then",
            Some(BranchInfo {
                always_terminates: then_always,
                by: then_by,
            }),
        );
        let then_on_success = if then_term != Termination::Error {
            on_success_path
        } else {
            false
        };
        collect_node(&then_body, then_frame_id, then_on_success, false, result);
    }

    // else-arm (explicit only).
    if let Some(else_body) = else_body {
        let else_always = terminates(else_term);
        let else_by = if else_always {
            termination_kind(else_term)
        } else {
            None
        };
        let else_frame_id = push_frame(
            result,
            frame_id,
            "if-else",
            Some(BranchInfo {
                always_terminates: else_always,
                by: else_by,
            }),
        );
        let else_on_success = if else_term != Termination::Error {
            on_success_path
        } else {
            false
        };
        collect_node(&else_body, else_frame_id, else_on_success, false, result);
    }

    // Continuation reachable iff at least one arm falls through.
    let then_falls = then_term == Termination::Fallthrough;
    let else_falls = else_term == Termination::Fallthrough;
    (then_falls as u8) + (else_falls as u8) > 0
}

// ============================================================================
// Case walker
// ============================================================================

/// Walk a `case` node. conditionLeaves first (in the parent frame), then each
/// branch body in its own frame. Ops in arms NEVER dominate.
fn walk_case_node(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    result: &mut OrderWalkResult,
) {
    walk_condition_leaves(node, frame_id, on_success_path, result);

    for branch in children_of(node) {
        let body = children_of(branch).first();
        let term = body
            .map(branch_termination)
            .unwrap_or(Termination::Fallthrough);
        let always = terminates(term);
        let by = if always { termination_kind(term) } else { None };

        let branch_frame_id = push_frame(
            result,
            frame_id,
            "case-branch",
            Some(BranchInfo {
                always_terminates: always,
                by,
            }),
        );
        let branch_on_success = if term != Termination::Error {
            on_success_path
        } else {
            false
        };
        for child in children_of(branch) {
            collect_node(child, branch_frame_id, branch_on_success, false, result);
        }
    }
}

// ============================================================================
// Loop walker
// ============================================================================

/// Walk a loop node. conditionLeaves walk in the PARENT frame, then a loop frame
/// is pushed AFTER them, then the body. Ops in loop bodies NEVER dominate.
fn walk_loop_node(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    result: &mut OrderWalkResult,
) {
    walk_condition_leaves(node, frame_id, on_success_path, result);

    let loop_frame_id = push_frame(result, frame_id, "loop", None);

    if node.kind == "repeat" {
        // repeat body = flat children. Walk via a synthetic block that carries the
        // body children but NO conditionLeaves of its own (already walked above) →
        // the synthetic block does NOT push its own frame and does NOT re-process
        // the repeat's conditionLeaves.
        let synthetic = PCFNNode {
            kind: "block".to_string(),
            operation_id: None,
            callsite_id: None,
            condition_guard: None,
            condition_leaves: None,
            children: node.children.clone(),
            else_children: None,
            is_case_else: false,
            source_range: None,
        };
        walk_block(&synthetic, loop_frame_id, on_success_path, false, result);
        return;
    }

    // while/for/foreach: single block child wraps the body.
    if let Some(body) = children_of(node).first() {
        collect_node(body, loop_frame_id, on_success_path, false, result);
    }
}

// ============================================================================
// Node collector
// ============================================================================

/// Collect all ids in a node tree at the given frame + success state.
///
/// `dominates_success_return`: only true for ops the caller proved are direct
/// root-block children before any normal-return. The `"other"`/default wrapper
/// PROPAGATES this value to its children; everything else resets it to false.
fn collect_node(
    node: &PCFNNode,
    frame_id: i64,
    on_success_path: bool,
    dominates_success_return: bool,
    result: &mut OrderWalkResult,
) {
    match node.kind.as_str() {
        "block" => {
            walk_block(node, frame_id, on_success_path, false, result);
        }
        "if" => {
            walk_if_node(node, frame_id, on_success_path, result);
        }
        "case" => {
            walk_case_node(node, frame_id, on_success_path, result);
        }
        "case-branch" => {
            for child in children_of(node) {
                collect_node(child, frame_id, on_success_path, false, result);
            }
        }
        "while" | "for" | "foreach" | "repeat" => {
            walk_loop_node(node, frame_id, on_success_path, result);
        }
        "try" => {
            let try_frame_id = push_frame(result, frame_id, "try", None);
            for child in children_of(node) {
                collect_node(child, try_frame_id, on_success_path, false, result);
            }
        }
        "op" => {
            walk_condition_leaves(node, frame_id, on_success_path, result);
            assign_leaf(
                node,
                frame_id,
                on_success_path,
                dominates_success_return,
                result,
            );
        }
        "call" => {
            walk_condition_leaves(node, frame_id, on_success_path, result);
            assign_leaf(
                node,
                frame_id,
                on_success_path,
                dominates_success_return,
                result,
            );
        }
        "error" => {
            walk_condition_leaves(node, frame_id, on_success_path, result);
            // Error never dominates a normal return.
            assign_leaf(node, frame_id, on_success_path, false, result);
        }
        "exit" => {
            walk_condition_leaves(node, frame_id, on_success_path, result);
            assign_leaf(node, frame_id, on_success_path, false, result);
        }
        _ => {
            // "other" — apply condition leaves, recurse into children, PROPAGATING
            // dominates_success_return (do NOT reset it for "other").
            walk_condition_leaves(node, frame_id, on_success_path, result);
            for child in children_of(node) {
                collect_node(
                    child,
                    frame_id,
                    on_success_path,
                    dominates_success_return,
                    result,
                );
            }
        }
    }
}

// ============================================================================
// Public entry point (pure: skeleton + attrs → maps + frames)
// ============================================================================

/// Walk a routine's CFN skeleton in execution order, assigning an
/// `OperationOrder` to every callsiteId and operationId, plus the routine's
/// `ScopeFrame[]`.
///
/// PURE over the CFN skeleton + the lowercased `attributes_parsed_names_lc`.
///
/// Root frame: TryFunction → empty (no frames). `statement_tree` None → no root
/// frame. A present-but-empty tree → the root frame STILL exists. Never panics.
pub fn compute_operation_order(
    statement_tree: Option<&PCFNNode>,
    attributes_parsed_names_lc: &[String],
) -> OperationOrderResult {
    let mut result = OrderWalkResult {
        by_callsite: HashMap::new(),
        by_operation: HashMap::new(),
        scope_frames: Vec::new(),
        next_order_id: 0,
        next_frame_id: 0,
    };

    // TryFunction routines: empty maps + NO frames.
    let is_try_function = attributes_parsed_names_lc
        .iter()
        .any(|n| n == "tryfunction");
    if is_try_function {
        return OperationOrderResult {
            by_callsite: result.by_callsite,
            by_operation: result.by_operation,
            scope_frames: result.scope_frames,
        };
    }

    let Some(tree) = statement_tree else {
        return OperationOrderResult {
            by_callsite: result.by_callsite,
            by_operation: result.by_operation,
            scope_frames: result.scope_frames,
        };
    };

    // Push the root frame (parentFrameId = -1, kind "block").
    let root_frame_id = push_frame(&mut result, -1, "block", None);

    // Root is always a "block" node; isRootBlock = true enables dominance tracking.
    walk_block(tree, root_frame_id, true, true, &mut result);

    OperationOrderResult {
        by_callsite: result.by_callsite,
        by_operation: result.by_operation,
        scope_frames: result.scope_frames,
    }
}

// ============================================================================
// Emitter entry point (apply to a built `PFeatures`, in place)
// ============================================================================

/// Apply L2 operation-order to a routine's already-built `PFeatures`, IN PLACE.
///
/// This is the emitter-facing glue (`l2_workspace.rs` → `aldump --l2`):
///   1. run [`compute_operation_order`] over the CFN skeleton + the lowercased
///      `attributesParsed` names (the TryFunction guard),
///   2. apply the `error-call` source-range post-pass (mirrors al-sem
///      `routine-indexer.ts:370-381` + R1b's controlContext post-pass): an
///      `error-call` op is NOT registered in `by_operation` (its CFN leaf carries
///      the paired callsite id), so for each `error-call` op lacking an order, find
///      the callsite whose `sourceAnchor.{startLine,startColumn}` matches and COPY
///      its full `OperationOrder` verbatim — allocate NO new orderId, infer NO
///      frame, recompute nothing,
///   3. write `order` onto every callsite/op that has an entry (sites with no entry
///      keep `None` → the field is ABSENT in JSON),
///   4. set the routine's `scope_frames` (PRESENT — carrying the root frame — when
///      a body tree exists, EMPTY for TryFunction / no body).
///
/// `attr_names_lc` are the lowercased `attributesParsed` names.
pub fn apply_operation_order(features: &mut PFeatures, attr_names_lc: &[String]) {
    let order = compute_operation_order(features.statement_tree.as_ref(), attr_names_lc);

    // error-call source-range post-pass (over the op/callsite RECORDS, which carry
    // source_anchor — the CFN skeleton dropped anchors). COPY the paired callsite's
    // full order verbatim into the error-call op.
    let mut by_operation = order.by_operation;
    for op in &features.operation_sites {
        if op.kind == "error-call" && !by_operation.contains_key(&op.id) {
            let r = &op.source_anchor;
            if let Some(paired) = features.call_sites.iter().find(|cs| {
                cs.source_anchor.start_line == r.start_line
                    && cs.source_anchor.start_column == r.start_column
            }) && let Some(ord) = order.by_callsite.get(&paired.id).copied()
            {
                by_operation.insert(op.id.clone(), ord);
            }
        }
    }

    // Populate `order` on each record (absent when no entry).
    for cs in &mut features.call_sites {
        cs.order = order.by_callsite.get(&cs.id).copied();
    }
    for op in &mut features.operation_sites {
        op.order = by_operation.get(&op.id).copied();
    }

    // Set the frame table (present-with-root when a body tree exists, empty otherwise).
    features.scope_frames = order.scope_frames;
}

// ============================================================================
// Full-routine driver (with the error-call source-range post-pass)
// ============================================================================

/// The full operation-order analysis of a single named routine, including the
/// error-call source-range post-pass folded into `by_operation`. This mirrors the
/// emitter (Task 3) glue and is the test-facing entry point — `compute_operation_order`
/// itself stays pure.
pub struct RoutineOperationOrder {
    pub by_callsite: HashMap<String, OperationOrder>,
    /// Post-pass applied: error-call ops inherit their paired callsite's order.
    pub by_operation: HashMap<String, OperationOrder>,
    pub scope_frames: Vec<ScopeFrame>,
    pub call_sites: Vec<PCallSite>,
    pub operation_sites: Vec<POperationSite>,
}

/// Drive the full L2 operation-order computation for the named routine in a
/// single-file source: parse → R1a body walk → CFN skeleton → `compute_operation_order`
/// → error-call source-range post-pass. Returns `None` when the routine isn't found.
pub fn analyze_named_routine_order(
    source: &str,
    routine_name: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
) -> Option<RoutineOperationOrder> {
    let (features, _parameters, attr_names_lc) =
        crate::program::body::l2_workspace::ir_features_for_named_routine(
            source,
            routine_name,
            app_guid,
            model_instance_id,
            source_unit_id,
        )?;

    let order = compute_operation_order(features.statement_tree.as_ref(), &attr_names_lc);

    // error-call post-pass (mirrors the controlContext post-pass): error-call ops
    // are NOT registered in by_operation (their CFN leaf carries the paired
    // callsite id). For each error-call op with no order, COPY the full order of
    // the callsite whose source anchor (startLine/startColumn) matches.
    let mut by_operation = order.by_operation;
    for op in &features.operation_sites {
        if op.kind == "error-call" && !by_operation.contains_key(&op.id) {
            let r = &op.source_anchor;
            if let Some(paired) = features.call_sites.iter().find(|cs| {
                cs.source_anchor.start_line == r.start_line
                    && cs.source_anchor.start_column == r.start_column
            }) && let Some(ord) = order.by_callsite.get(&paired.id).copied()
            {
                by_operation.insert(op.id.clone(), ord);
            }
        }
    }

    Some(RoutineOperationOrder {
        by_callsite: order.by_callsite,
        by_operation,
        scope_frames: order.scope_frames,
        call_sites: features.call_sites,
        operation_sites: features.operation_sites,
    })
}
