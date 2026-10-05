//! L2 control-context lattice + execution-order walker — port of al-sem
//! `src/index/control-context.ts` (`computeControlContexts`) plus the
//! `routine-indexer.ts:294-350` glue (IsHandled eligibility builder + the
//! error-call source-range post-pass).
//!
//! Consumes the validated R1a CFN skeleton (`features::PCFNNode`) PLUS routine
//! metadata (`attributesParsed`, `parameters`, derived `isHandledVars`); it does
//! NOT rebuild a control-flow representation. The branch-termination primitives
//! live in the shared [`super::control_flow`] module (R1c reuses them).
//!
//! Lattice (highest rank first — higher rank "wins" on max):
//!   unreachable > error-path > is-handled-guarded > loop-body > conditional > top-level
//!
//! `undefined` == unknown == ABSENCE: TryFunction / no-body / unwalked ids carry
//! no entry in the result maps (the caller omits the field).
//!
//! Never panics.

use super::control_flow::{Termination, branch_termination, else_termination, has_explicit_else};
use super::features::{PCFNNode, PCallSite, PFeatures, POperationSite};
use super::scope::ParameterSymbol;
use std::collections::HashMap;

// ============================================================================
// Lattice
// ============================================================================

/// The control-context lattice value (low → high rank).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlContext {
    TopLevel,
    Conditional,
    LoopBody,
    IsHandledGuarded,
    ErrorPath,
    Unreachable,
}

impl ControlContext {
    /// Numeric rank for lattice ordering. Higher = more restrictive.
    /// `unreachable` is the top; `top-level` is the bottom.
    pub fn rank(self) -> u8 {
        match self {
            ControlContext::TopLevel => 0,
            ControlContext::Conditional => 1,
            ControlContext::LoopBody => 2,
            ControlContext::IsHandledGuarded => 3,
            ControlContext::ErrorPath => 4,
            ControlContext::Unreachable => 5,
        }
    }

    /// The kebab-case lattice string used by al-sem's `controlContext` field
    /// (`control-context.ts` `ControlContext` union values). This is the exact
    /// serialization the L2 goldens carry.
    pub fn as_str(self) -> &'static str {
        match self {
            ControlContext::TopLevel => "top-level",
            ControlContext::Conditional => "conditional",
            ControlContext::LoopBody => "loop-body",
            ControlContext::IsHandledGuarded => "is-handled-guarded",
            ControlContext::ErrorPath => "error-path",
            ControlContext::Unreachable => "unreachable",
        }
    }
}

/// Return the more-restrictive (higher-rank) of two contexts.
pub fn max_ctx(a: ControlContext, b: ControlContext) -> ControlContext {
    if a.rank() >= b.rank() { a } else { b }
}

// ============================================================================
// IsHandled guard eligibility (decision 2)
// ============================================================================

/// IsHandled-guard eligibility for a routine. Names are lowercased.
#[derive(Debug, Clone, Default)]
pub struct GuardEligibility {
    /// Lowercased names of by-var Boolean parameters.
    pub by_var_bool_params: Vec<String>,
    /// Lowercased names of published / publish-eligible boolean variables.
    pub published_bool_vars: Vec<String>,
}

impl GuardEligibility {
    fn is_eligible(&self, identifier: &str) -> bool {
        self.by_var_bool_params.iter().any(|n| n == identifier)
            || self.published_bool_vars.iter().any(|n| n == identifier)
    }
}

/// Build the eligibility sets from parameters + the published boolean-var names.
/// `type_text` is matched case-insensitively against `boolean`.
fn build_guard_eligibility(
    parameters: &[ParameterSymbol],
    is_handled_vars: &[String],
) -> GuardEligibility {
    let mut by_var_bool_params = Vec::new();
    for p in parameters {
        if p.is_var && p.type_text.trim().to_lowercase() == "boolean" {
            let lc = p.name.to_lowercase();
            if !by_var_bool_params.contains(&lc) {
                by_var_bool_params.push(lc);
            }
        }
    }
    let mut published_bool_vars = Vec::new();
    for v in is_handled_vars {
        let lc = v.to_lowercase();
        if !published_bool_vars.contains(&lc) {
            published_bool_vars.push(lc);
        }
    }
    GuardEligibility {
        by_var_bool_params,
        published_bool_vars,
    }
}

// ============================================================================
// Walker accumulator
// ============================================================================

struct WalkResult {
    by_callsite: HashMap<String, ControlContext>,
    by_operation: HashMap<String, ControlContext>,
    guard: GuardEligibility,
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
// Block-level walker (continuation tracking)
// ============================================================================

/// Walk a "block" node's children in execution order. Returns the context that
/// applies to code AFTER this block (the final continuation context).
fn walk_block(
    node: &PCFNNode,
    ambient_context: ControlContext,
    result: &mut WalkResult,
) -> ControlContext {
    let mut continuation_ctx = ambient_context;
    let mut reachable = true;

    // A block walks its children; a non-block node walks itself as a single stmt.
    let singleton = [node.clone()];
    let stmts: &[PCFNNode] = if node.kind == "block" {
        children_of(node)
    } else {
        &singleton
    };

    for stmt in stmts {
        if !reachable {
            // Everything after an unconditional exit in this block is unreachable.
            collect_node(stmt, ControlContext::Unreachable, result);
            continue;
        }

        match stmt.kind.as_str() {
            "error" => {
                // Bare Error(): condition leaves at ambient, mark the leaf, then
                // everything after in this block is unreachable.
                apply_condition_leaves(stmt, continuation_ctx, result);
                assign_leaf(stmt, continuation_ctx, result);
                reachable = false;
            }
            "exit" => {
                apply_condition_leaves(stmt, continuation_ctx, result);
                assign_leaf(stmt, continuation_ctx, result);
                reachable = false;
            }
            "if" => {
                let next = walk_if_node(stmt, continuation_ctx, result);
                match next {
                    Some(ctx) => continuation_ctx = ctx,
                    None => reachable = false,
                }
            }
            "case" => {
                continuation_ctx = walk_case_node(stmt, continuation_ctx, result);
            }
            "while" | "for" | "foreach" | "repeat" => {
                walk_loop_node(stmt, continuation_ctx, result);
            }
            _ => {
                // call / op / other / try → collect at current context.
                collect_node(stmt, continuation_ctx, result);
            }
        }
    }

    continuation_ctx
}

/// Walk an `if` node, returning the continuation context for siblings AFTER it,
/// or `None` when every path through the `if` terminates (both arms exit/error).
fn walk_if_node(
    node: &PCFNNode,
    ambient: ControlContext,
    result: &mut WalkResult,
) -> Option<ControlContext> {
    // Condition leaves evaluate at ambient BEFORE branch selection.
    apply_condition_leaves(node, ambient, result);

    let then_body = children_of(node).first();
    let else_body = else_children_of(node).first();
    let then_term: Termination = then_body
        .map(branch_termination)
        .unwrap_or(Termination::Fallthrough);
    let else_term: Termination = else_termination(node);
    let branch_ctx = max_ctx(ambient, ControlContext::Conditional);

    // IsHandled guard recognition: positive/negative polarity on an eligible bool.
    let recognised: Option<&str> = node.condition_guard.as_ref().and_then(|g| {
        if result.guard.is_eligible(&g.identifier) {
            Some(g.polarity.as_str())
        } else {
            None
        }
    });
    let guarded_ctx = max_ctx(ambient, ControlContext::IsHandledGuarded);

    // Special case: single-arm error-only guard → body error-path, cont unchanged.
    if !has_explicit_else(node) && then_term == Termination::Error {
        if let Some(then_body) = then_body {
            collect_node(then_body, ControlContext::ErrorPath, result);
        }
        return Some(ambient);
    }

    // then-arm body context: negative-polarity IsHandled guards the BODY.
    let then_body_ctx = if then_term == Termination::Error {
        ControlContext::ErrorPath
    } else if recognised == Some("negative") {
        guarded_ctx
    } else {
        branch_ctx
    };
    if let Some(then_body) = then_body {
        collect_node(then_body, then_body_ctx, result);
    }
    // Walk explicit else-arm (implicit else has no body to walk).
    if let Some(else_body) = else_body {
        let else_ctx = if else_term == Termination::Error {
            ControlContext::ErrorPath
        } else {
            branch_ctx
        };
        collect_node(else_body, else_ctx, result);
    }

    // Continuation narrowing by fall-through counting (implicit else falls through).
    let then_falls = then_term == Termination::Fallthrough;
    let else_falls = else_term == Termination::Fallthrough;
    let fallthrough_count = (then_falls as u8) + (else_falls as u8);
    if fallthrough_count == 0 {
        return None; // unreachable continuation
    }
    if fallthrough_count == 1 {
        // One arm terminates → reaching the continuation is guarded. Positive-
        // polarity IsHandled guards the CONTINUATION.
        let narrowed = if recognised == Some("positive") {
            guarded_ctx
        } else {
            max_ctx(ambient, ControlContext::Conditional)
        };
        return Some(narrowed);
    }
    Some(ambient) // both arms fall through → no narrowing
}

/// Walk a `case` node, returning the continuation context for siblings after it.
fn walk_case_node(
    node: &PCFNNode,
    ambient: ControlContext,
    result: &mut WalkResult,
) -> ControlContext {
    apply_condition_leaves(node, ambient, result);
    let branch_ctx = max_ctx(ambient, ControlContext::Conditional);

    let mut any_terminates = false;
    for branch in children_of(node) {
        let body = children_of(branch).first();
        let term: Termination = body
            .map(branch_termination)
            .unwrap_or(Termination::Fallthrough);
        if term != Termination::Fallthrough {
            any_terminates = true;
        }
        let arm_ctx = if term == Termination::Error {
            ControlContext::ErrorPath
        } else {
            branch_ctx
        };
        for child in children_of(branch) {
            collect_node(child, arm_ctx, result);
        }
    }

    if any_terminates {
        max_ctx(ambient, ControlContext::Conditional)
    } else {
        ambient
    }
}

/// Walk a loop node's body. `repeat` bodies are a FLAT children array;
/// `while`/`for`/`foreach` wrap the body in a single block child.
fn walk_loop_node(node: &PCFNNode, ambient: ControlContext, result: &mut WalkResult) {
    // Condition / range / iterable leaves evaluate at ambient context.
    apply_condition_leaves(node, ambient, result);
    let body_ctx = max_ctx(ambient, ControlContext::LoopBody);

    if node.kind == "repeat" {
        // repeat body = all children (flat). Walk via a synthetic block so an
        // early exit/Error() inside the body is handled. We do NOT re-apply the
        // repeat node's own conditionLeaves (already applied above): the synthetic
        // block carries the children but no conditionLeaves of its own.
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
        walk_block(&synthetic, body_ctx, result);
        return;
    }

    // while/for/foreach: single block child wraps the body.
    if let Some(body) = children_of(node).first() {
        collect_node(body, body_ctx, result);
    }
}

// ============================================================================
// Node collector
// ============================================================================

/// Collect all ids in a node tree under the given context. For block nodes,
/// delegates to `walk_block` so continuation tracking still applies.
fn collect_node(node: &PCFNNode, ctx: ControlContext, result: &mut WalkResult) {
    match node.kind.as_str() {
        "block" => {
            walk_block(node, ctx, result);
        }
        "if" => {
            walk_if_node(node, ctx, result);
        }
        "case" => {
            walk_case_node(node, ctx, result);
        }
        "case-branch" => {
            for child in children_of(node) {
                collect_node(child, ctx, result);
            }
        }
        "while" | "for" | "foreach" | "repeat" => {
            walk_loop_node(node, ctx, result);
        }
        "try" => {
            for child in children_of(node) {
                collect_node(child, ctx, result);
            }
        }
        "op" | "call" | "error" | "exit" => {
            apply_condition_leaves(node, ctx, result);
            assign_leaf(node, ctx, result);
        }
        _ => {
            // "other" — apply condition leaves, recurse into children.
            apply_condition_leaves(node, ctx, result);
            for child in children_of(node) {
                collect_node(child, ctx, result);
            }
        }
    }
}

/// Process a node's conditionLeaves at the given context (BEFORE the node's own
/// effect / branch selection).
fn apply_condition_leaves(node: &PCFNNode, ctx: ControlContext, result: &mut WalkResult) {
    for leaf in condition_leaves_of(node) {
        collect_node(leaf, ctx, result);
    }
}

/// Assign the context for a leaf node's own id (op or call/error leaf).
fn assign_leaf(node: &PCFNNode, ctx: ControlContext, result: &mut WalkResult) {
    if let Some(op_id) = &node.operation_id {
        result.by_operation.insert(op_id.clone(), ctx);
    }
    if let Some(cs_id) = &node.callsite_id {
        result.by_callsite.insert(cs_id.clone(), ctx);
    }
}

// ============================================================================
// Public entry point (pure: skeleton + metadata → maps)
// ============================================================================

/// The control-context maps for a routine, keyed by callsite/operation id.
pub struct ControlContextMaps {
    pub by_callsite: HashMap<String, ControlContext>,
    pub by_operation: HashMap<String, ControlContext>,
    pub eligibility: GuardEligibility,
}

/// Walk a routine's CFN skeleton in execution order, assigning a ControlContext
/// to every callsiteId and operationId. TryFunction / no-tree → empty maps.
///
/// This does NOT apply the error-call post-pass (that needs the OperationSite
/// kinds + source anchors — see [`analyze_named_routine`]).
pub fn compute_control_contexts(
    statement_tree: Option<&PCFNNode>,
    attributes_parsed_names_lc: &[String],
    parameters: &[ParameterSymbol],
    is_handled_vars: &[String],
) -> ControlContextMaps {
    let mut result = WalkResult {
        by_callsite: HashMap::new(),
        by_operation: HashMap::new(),
        guard: build_guard_eligibility(parameters, is_handled_vars),
    };

    // TryFunction routines: all contexts → undefined (empty maps).
    let is_try_function = attributes_parsed_names_lc
        .iter()
        .any(|n| n == "tryfunction");
    if is_try_function {
        return ControlContextMaps {
            by_callsite: result.by_callsite,
            by_operation: result.by_operation,
            eligibility: result.guard,
        };
    }

    if let Some(tree) = statement_tree {
        // Root is always a "block" node.
        walk_block(tree, ControlContext::TopLevel, &mut result);
    }

    ControlContextMaps {
        by_callsite: result.by_callsite,
        by_operation: result.by_operation,
        eligibility: result.guard,
    }
}

// ============================================================================
// IsHandled-var derivation (routine-indexer.ts:302-318)
// ============================================================================

/// Derive the IsHandled-eligible boolean local/global var names (lowercased) per
/// `routine-indexer.ts:302-318`: a Boolean var (scope != "parameter") whose
/// lowercased name EQUALS some whole, trimmed callsite argument text.
pub fn derive_is_handled_vars(features: &PFeatures) -> Vec<String> {
    let mut call_arg_name_set: Vec<String> = Vec::new();
    for cs in &features.call_sites {
        for arg in &cs.argument_texts {
            let trimmed = arg.trim().to_lowercase();
            if !trimmed.is_empty() && !call_arg_name_set.contains(&trimmed) {
                call_arg_name_set.push(trimmed);
            }
        }
    }
    let mut out = Vec::new();
    for v in &features.variables {
        if v.scope != "parameter"
            && v.declared_type.trim().to_lowercase() == "boolean"
            && call_arg_name_set.contains(&v.name.to_lowercase())
        {
            let lc = v.name.to_lowercase();
            if !out.contains(&lc) {
                out.push(lc);
            }
        }
    }
    out
}

/// Apply L2 control-context to a routine's already-built `PFeatures`, IN PLACE.
///
/// This is the emitter-facing entry point (`l2_workspace.rs` → `aldump --l2`):
///   1. derive `isHandledVars` from the features (callsite arg texts × boolean vars),
///   2. run [`compute_control_contexts`] over the CFN skeleton + metadata,
///   3. apply the `error-call` source-range post-pass (`routine-indexer.ts:337-350`)
///      so `error-call` ops inherit their paired callsite's context,
///   4. write `control_context` (kebab-case string) onto every callsite/op that has
///      an entry; sites with NO entry keep `None` → the field is ABSENT in JSON.
///
/// `attr_names_lc` are the lowercased `attributesParsed` names (for TryFunction);
/// `parameters` feeds by-var Boolean guard eligibility.
pub fn apply_control_contexts(
    features: &mut PFeatures,
    attr_names_lc: &[String],
    parameters: &[ParameterSymbol],
) {
    let is_handled_vars = derive_is_handled_vars(features);
    let maps = compute_control_contexts(
        features.statement_tree.as_ref(),
        attr_names_lc,
        parameters,
        &is_handled_vars,
    );

    // error-call post-pass: error-call ops are NOT in by_operation (their CFN leaf
    // carries the paired callsite id). For each error-call op lacking a context,
    // inherit the context of the callsite whose source anchor (startLine/startColumn)
    // matches the op's.
    let mut by_operation = maps.by_operation;
    let call_anchors: Vec<(u32, u32, String)> = features
        .call_sites
        .iter()
        .map(|cs| {
            (
                cs.source_anchor.start_line,
                cs.source_anchor.start_column,
                cs.id.clone(),
            )
        })
        .collect();
    for op in &features.operation_sites {
        if op.kind == "error-call" && !by_operation.contains_key(&op.id) {
            let r = &op.source_anchor;
            if let Some((_, _, cs_id)) = call_anchors
                .iter()
                .find(|(l, c, _)| *l == r.start_line && *c == r.start_column)
                && let Some(ctx) = maps.by_callsite.get(cs_id).copied()
            {
                by_operation.insert(op.id.clone(), ctx);
            }
        }
    }

    // Populate the projection field (absent when no entry).
    for cs in &mut features.call_sites {
        cs.control_context = maps.by_callsite.get(&cs.id).map(|c| c.as_str().to_string());
    }
    for op in &mut features.operation_sites {
        op.control_context = by_operation.get(&op.id).map(|c| c.as_str().to_string());
    }
}

// ============================================================================
// Full-routine driver (with the error-call source-range post-pass)
// ============================================================================

/// The full control-context analysis of a single named routine, including the
/// error-call post-pass folded into `by_operation`.
pub struct RoutineControlContexts {
    pub by_callsite: HashMap<String, ControlContext>,
    /// Post-pass applied: error-call ops inherit their paired callsite's context.
    pub by_operation: HashMap<String, ControlContext>,
    pub eligibility: GuardEligibility,
    pub call_sites: Vec<PCallSite>,
    pub operation_sites: Vec<POperationSite>,
}

/// Drive the full L2 control-context computation for the named routine in a
/// single-file source: parse → R1a body walk → CFN skeleton → walker →
/// error-call post-pass. Returns `None` when the routine isn't found.
pub fn analyze_named_routine(
    source: &str,
    routine_name: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
) -> Option<RoutineControlContexts> {
    // Owned-IR body projection → features (CFN skeleton + call/op sites + vars) +
    // parameters + lowercased attribute names.
    let (features, parameters, attr_names_lc) =
        crate::program::body::l2_workspace::ir_features_for_named_routine(
            source,
            routine_name,
            app_guid,
            model_instance_id,
            source_unit_id,
        )?;

    let is_handled_vars = derive_is_handled_vars(&features);

    let maps = compute_control_contexts(
        features.statement_tree.as_ref(),
        &attr_names_lc,
        &parameters,
        &is_handled_vars,
    );

    // error-call post-pass (routine-indexer.ts:337-350): error-call ops are NOT
    // registered in by_operation (their CFN leaf carries the paired callsite id).
    // For each error-call op with no context, inherit the context of the callsite
    // whose source anchor (startLine/startColumn) matches the op's.
    let mut by_operation = maps.by_operation;
    for op in &features.operation_sites {
        if op.kind == "error-call" && !by_operation.contains_key(&op.id) {
            let r = &op.source_anchor;
            if let Some(paired) = features.call_sites.iter().find(|cs| {
                cs.source_anchor.start_line == r.start_line
                    && cs.source_anchor.start_column == r.start_column
            }) && let Some(ctx) = maps.by_callsite.get(&paired.id).copied()
            {
                by_operation.insert(op.id.clone(), ctx);
            }
        }
    }

    Some(RoutineControlContexts {
        by_callsite: maps.by_callsite,
        by_operation,
        eligibility: maps.eligibility,
        call_sites: features.call_sites,
        operation_sites: features.operation_sites,
    })
}
