//! Constant-argument guards (engine-switch S8, triage D gap 2).
//!
//! A write or `Commit` inside `if UpdateCache then ...` runs only when the
//! caller passes `UpdateCache = true`. Continia Core's activation code reaches
//! its cache writes only through `GetActivationState(..., UpdateCache, ...)`,
//! and every route from a workspace subscriber passes a literal `false`; one
//! other Core caller passes `true`, so no per-routine fact can prune the write.
//! The answer depends on the call path, exactly like a `var` record parameter's
//! temp state, and is carried the same way: a capability-cone entry records the
//! parameter values it REQUIRES ([`Req`]), in the frame of the routine whose
//! cone holds it, and each call edge substitutes the caller's argument
//! ([`across_edge`]): a literal that contradicts drops the fact, a literal that
//! agrees drops the requirement, and a forwarded parameter re-anchors it.
//!
//! A requirement is only ever DROPPED when it cannot be tracked, which makes the
//! fact unconditional again: the pre-guard behaviour. Only a literal that
//! contradicts removes a fact.
//!
//! A parameter is USABLE as a guard only when its value is the caller's
//! argument for the whole body: a by-value `Boolean`, never an assignment
//! target, and never passed to a `var` parameter (or to a callee whose
//! parameters are not known). Guards come from `if` nodes whose
//! `condition_guard` names a usable parameter (`P`, `not P`, `P = false`), and
//! from the early exit `if <guard> then exit;`, which guards the statements
//! that follow it in the same block.

use std::collections::HashMap;

use crate::engine::l3::l3_workspace::L3Routine;
use crate::program::body::features::{PCFNNode, PCallSite};

/// `(parameter index, required value)` in one routine's frame.
pub(crate) type Req = (u32, bool);

/// Longer requirement lists are dropped (the fact becomes unconditional):
/// keeps the number of distinct cone keys per fact small.
const MAX_REQS: usize = 4;

/// One routine's guard frame.
#[derive(Debug, Default)]
pub(crate) struct FrameGuards {
    /// Usable guard parameters: index set.
    pub usable: Vec<u32>,
    /// Operation id / call-site id -> the requirements that must hold for it to
    /// run. Sites with no requirement are absent.
    pub by_site: HashMap<String, Vec<Req>>,
}

/// Sorted, conflict-free union of two requirement lists. `None` when they
/// require opposite values of one parameter (the site can never run).
pub(crate) fn conjoin(a: &[Req], b: &[Req]) -> Option<Vec<Req>> {
    let mut out: Vec<Req> = a.to_vec();
    for &(i, v) in b {
        match out.iter().find(|(j, _)| *j == i) {
            Some(&(_, w)) if w != v => return None,
            Some(_) => {}
            None => out.push((i, v)),
        }
    }
    out.sort_unstable();
    if out.len() > MAX_REQS {
        out.clear();
    }
    Some(out)
}

/// The guard frame of `r`. `passes_by_value(callsite_id, argument_index)` says
/// whether the call passes that argument to a by-value parameter of a known
/// callee. `None` when the routine has no usable guard parameter.
pub(crate) fn frame_guards(
    r: &L3Routine,
    passes_by_value: impl Fn(&str, u32) -> bool,
) -> Option<FrameGuards> {
    let mut usable: Vec<(String, u32)> = Vec::new();
    for p in &r.parameters {
        if p.is_var || !p.type_text.trim().eq_ignore_ascii_case("boolean") {
            continue;
        }
        let name = p.name.trim().trim_matches('"');
        if r.var_assignments.iter().any(|a| {
            a.lhs_name
                .trim()
                .trim_matches('"')
                .eq_ignore_ascii_case(name)
        }) {
            continue;
        }
        let passed_unsafely = r.call_sites.iter().any(|cs| {
            cs.argument_bindings.iter().any(|b| {
                b.source_kind == "parameter"
                    && b.source_parameter_index == Some(p.index)
                    && !passes_by_value(&cs.id, b.parameter_index)
            })
        });
        if passed_unsafely {
            continue;
        }
        usable.push((name.to_ascii_lowercase(), p.index));
    }
    if usable.is_empty() {
        return None;
    }
    let mut by_site: HashMap<String, Vec<Req>> = HashMap::new();
    if let Some(tree) = &r.statement_tree {
        walk(tree, &[], &usable, &mut by_site);
    }
    Some(FrameGuards {
        usable: usable.iter().map(|(_, i)| *i).collect(),
        by_site,
    })
}

/// [`frame_guards`] over a call graph: `callees_at(callsite_id)` lists the
/// routines a call site reaches. A parameter passed to a call stays usable when
/// every reached callee takes it by value, or when the call reaches nothing (a
/// platform method) other than `Evaluate` and `Clear`, which write their
/// argument.
pub(crate) fn frame_guards_over<'a>(
    r: &L3Routine,
    callees_at: impl Fn(&str) -> Vec<&'a str>,
    routines_by_id: &HashMap<&str, &L3Routine>,
) -> Option<FrameGuards> {
    use crate::program::body::features::PCallee;
    let passes_by_value = |cs_id: &str, arg: u32| -> bool {
        let targets = callees_at(cs_id);
        if targets.is_empty() {
            let callee_lc = r
                .call_sites
                .iter()
                .find(|c| c.id == cs_id)
                .map(|c| match &c.callee {
                    PCallee::Bare { name } => name.to_ascii_lowercase(),
                    PCallee::Member { method, .. } => method.to_ascii_lowercase(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            // The platform methods that write a Boolean argument.
            return !matches!(callee_lc.as_str(), "evaluate" | "clear");
        }
        targets.iter().all(|to| {
            routines_by_id
                .get(to)
                .and_then(|callee| callee.parameters.iter().find(|p| p.index == arg))
                .is_some_and(|p| !p.is_var)
        })
    };
    frame_guards(r, passes_by_value)
}

/// The requirement an `if` node's simple guard states for its THEN branch.
fn then_req(node: &PCFNNode, usable: &[(String, u32)]) -> Option<Req> {
    let g = node.condition_guard.as_ref()?;
    let (_, i) = usable.iter().find(|(n, _)| *n == g.identifier)?;
    Some((*i, g.polarity == "positive"))
}

/// Whether a branch is exactly one `exit` (through any `block` wrappers: a
/// branch is built as one block node).
fn is_exit_only(children: Option<&Vec<PCFNNode>>) -> bool {
    match children.map(Vec::as_slice) {
        Some([n]) if n.kind == "exit" => true,
        Some([n]) if n.kind == "block" && n.condition_leaves.is_none() => {
            is_exit_only(n.children.as_ref())
        }
        _ => false,
    }
}

fn record(node: &PCFNNode, reqs: &[Req], by_site: &mut HashMap<String, Vec<Req>>) {
    if reqs.is_empty() {
        return;
    }
    for id in [&node.operation_id, &node.callsite_id]
        .into_iter()
        .flatten()
    {
        by_site.insert(id.clone(), reqs.to_vec());
    }
}

/// Walk one statement list under `reqs`.
fn walk_list(
    nodes: &[PCFNNode],
    reqs: &[Req],
    usable: &[(String, u32)],
    by_site: &mut HashMap<String, Vec<Req>>,
) {
    let mut reqs: Vec<Req> = reqs.to_vec();
    for n in nodes {
        walk(n, &reqs, usable, by_site);
        // `if <guard> then exit;` (no else): what follows runs only when the
        // guard is false.
        if n.kind == "if"
            && n.else_children.is_none()
            && is_exit_only(n.children.as_ref())
            && let Some((i, v)) = then_req(n, usable)
        {
            match conjoin(&reqs, &[(i, !v)]) {
                Some(r) => reqs = r,
                None => return, // the rest of the block cannot run
            }
        }
    }
}

fn walk(
    node: &PCFNNode,
    reqs: &[Req],
    usable: &[(String, u32)],
    by_site: &mut HashMap<String, Vec<Req>>,
) {
    record(node, reqs, by_site);
    // Calls and ops inside a condition run before the branch is chosen.
    if let Some(leaves) = &node.condition_leaves {
        for l in leaves {
            walk(l, reqs, usable, by_site);
        }
    }
    let req = if node.kind == "if" {
        then_req(node, usable)
    } else {
        None
    };
    let branch = |extra: Option<Req>| -> Option<Vec<Req>> {
        match extra {
            Some(r) => conjoin(reqs, &[r]),
            None => Some(reqs.to_vec()),
        }
    };
    if let Some(children) = &node.children
        && let Some(r) = branch(req)
    {
        walk_list(children, &r, usable, by_site);
    }
    if let Some(children) = &node.else_children
        && let Some(r) = branch(req.map(|(i, v)| (i, !v)))
    {
        walk_list(children, &r, usable, by_site);
    }
}

/// Carry a callee-frame requirement list across one call into the caller's
/// frame. `None`: an argument contradicts a requirement, so the fact cannot
/// happen through this call. A requirement whose argument is neither a boolean
/// literal nor a usable caller parameter is dropped (unconditional).
pub(crate) fn across_edge(
    reqs: &[Req],
    cs: &PCallSite,
    caller_frame: Option<&FrameGuards>,
) -> Option<Vec<Req>> {
    let mut out: Vec<Req> = Vec::new();
    for &(i, v) in reqs {
        let text = cs
            .argument_texts
            .get(i as usize)
            .map(|t| t.trim().to_ascii_lowercase());
        match text.as_deref() {
            Some("true") | Some("false") => {
                if (text.as_deref() == Some("true")) != v {
                    return None;
                }
            }
            _ => {
                let j = cs
                    .argument_bindings
                    .iter()
                    .find(|b| b.parameter_index == i && b.source_kind == "parameter")
                    .and_then(|b| b.source_parameter_index);
                if let (Some(j), Some(f)) = (j, caller_frame)
                    && f.usable.contains(&j)
                {
                    out = conjoin(&out, &[(j, v)])?;
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conjoin_merges_and_detects_conflict() {
        assert_eq!(
            conjoin(&[(1, true)], &[(0, false)]),
            Some(vec![(0, false), (1, true)])
        );
        assert_eq!(conjoin(&[(1, true)], &[(1, true)]), Some(vec![(1, true)]));
        assert_eq!(conjoin(&[(1, true)], &[(1, false)]), None);
        let many: Vec<Req> = (0..5).map(|i| (i, true)).collect();
        assert_eq!(conjoin(&many, &[]), Some(vec![]), "over the cap: dropped");
    }
}
