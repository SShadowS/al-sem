//! G30: an interface call on an object GLOBAL of a workspace codeunit, where
//! the codeunit itself is the only writer, dispatches over the codeunits its
//! writes can store. Continia DO: `CDO Template Variant Mgt` holds
//! `FieldChainEvaluator: Interface "CDO IField Chain Evaluator"`, written only
//! by `internal procedure SetFieldChainEvaluator(Evaluator: …)`, whose one
//! caller passes a `Codeunit "CDO Field Chain Evaluator"` variable.
//!
//! Two steps, because the second needs every call edge of the program:
//! 1. per interface call site ([`holder_sources`], from `full.rs`'s member
//!    arm): every write to the global, in every routine of the codeunit, is a
//!    codeunit-typed variable ([`HolderSource::Codeunit`]) or a parameter of
//!    the writing routine ([`HolderSource::Param`]);
//! 2. after phase 1 ([`close_holder_sites`]): every call of a writing routine
//!    passes, at that parameter, a codeunit-typed variable
//!    ([`argument_codeunits`], recorded on each call site). The site is then
//!    closed over those codeunits, if each implements the interface and is
//!    the object of one of the edge's real routes.
//!
//! Every doubt fails OPEN (the site keeps its open-world edge):
//! - the global is not of a codeunit, is a `protected var`, or is shadowed at
//!   the call; the codeunit implements an interface (a call of its methods
//!   through it may come from code that is not loaded);
//! - the file did not parse cleanly, or the lowerer recorded an issue inside
//!   the codeunit;
//! - a routine reads the global other than as a member-call receiver (passing
//!   it on, `Clear`, copying it) or writes it from any other value;
//! - a writing routine is not `local` or `internal`, is an event subscriber,
//!   or the workspace names `internalsVisibleTo` friends (their calls are not
//!   resolved here);
//! - one call of a writing routine passes anything but a codeunit variable.
//!
//! An unassigned interface variable raises a runtime error when called; that
//! is no dispatch target, so it does not open the site.

use std::collections::HashMap;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{ExprId, ExprKind, ObjectKind, ParseStatus, RoutineDecl, VarDecl};

use crate::program::graph::ProgramGraph;
use crate::program::node::{ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{Access, fold_ref_name};
use crate::program::resolve::closed_enum::{
    ClosedEnumCx, admitted_writes, name_of, overlaps, unparen,
};
use crate::program::resolve::edge::{Evidence, RouteTarget, SetCompleteness};
use crate::program::resolve::full::{ClassifiedEdge, ObligationId, SiteFacts};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolve::receiver::{
    CallerScopeSymbol, ParsedType, caller_scope_symbol, classify_type_text, parse_object_kind_type,
};

/// One value a write can store in the global.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderSource {
    /// A variable declared `Codeunit X`.
    Codeunit(ObjectNodeId),
    /// The `n`th parameter of the writing routine.
    Param(RoutineNodeId, usize),
}

/// Step 1: the values every write to the object global `receiver` names
/// stores, or `None` (see the module doc). At least one write.
pub(crate) fn holder_sources(
    cx: &ClosedEnumCx,
    receiver: ExprId,
    interface_lc: &str,
) -> Option<Vec<HolderSource>> {
    let obj = cx.obj;
    if obj.kind != ObjectKind::Codeunit
        || !obj.implements.is_empty()
        || cx.file.parse_status != ParseStatus::Clean
        || cx
            .file
            .issues
            .iter()
            .any(|i| overlaps(&i.origin.byte, &obj.origin.byte))
    {
        return None;
    }
    let v_lc = name_of(&cx.file.ir, receiver)?;
    let named = |n: &str| n.fold_identifier() == v_lc;
    let is_i = |ty: Option<&str>| matches!(ty.map(classify_type_text), Some(ParsedType::Interface { name }) if name == interface_lc);
    let mut globals = obj.globals.iter().filter(|g| named(&g.name)).peekable();
    if globals.peek().is_none()
        || !globals.all(|g| is_i(g.ty.as_deref()))
        || obj.protected_globals.iter().any(|g| named(g))
        || shadows(cx.routine, &v_lc)
    {
        return None;
    }
    let mut out = Vec::new();
    for routine in &obj.routines {
        if shadows(routine, &v_lc) {
            continue;
        }
        if routine.parse_incomplete {
            return None;
        }
        let Some(body) = routine.body else {
            continue;
        };
        for value in admitted_writes(&cx.file.ir, body, &v_lc)? {
            out.push(write_source(cx, routine, value)?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Whether a local, parameter or the named return value of `routine` is
/// named `v_lc` (it hides the global there).
fn shadows(routine: &RoutineDecl, v_lc: &str) -> bool {
    let named = |n: &str| n.fold_identifier() == v_lc;
    routine.params.iter().any(|p| named(&p.name))
        || routine.locals.iter().any(|l| named(&l.name))
        || routine.return_name.as_deref().is_some_and(named)
}

/// One written value: a parameter of `routine`, or a codeunit variable.
fn write_source(cx: &ClosedEnumCx, routine: &RoutineDecl, value: ExprId) -> Option<HolderSource> {
    let ir = &cx.file.ir;
    let n = name_of(ir, unparen(ir, value))?;
    if let Some(i) = routine
        .params
        .iter()
        .position(|p| p.name.fold_identifier() == n)
    {
        let rid =
            crate::program::sig_fp::source_routine_node_id(cx.from_object.id.clone(), routine);
        return Some(HolderSource::Param(rid, i));
    }
    codeunit_of(
        cx.graph,
        cx.index,
        &cx.from_object.id,
        &n,
        routine,
        &cx.obj.globals,
    )
    .map(HolderSource::Codeunit)
}

/// The codeunit X of the variable `name_lc` declared `Codeunit X` in scope at
/// `routine` (X resolved from `from`, unique in the whole program), or `None`.
fn codeunit_of(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    from: &ObjectNodeId,
    name_lc: &str,
    routine: &RoutineDecl,
    globals: &[VarDecl],
) -> Option<ObjectNodeId> {
    let CallerScopeSymbol::Found(Some(ty)) = caller_scope_symbol(name_lc, routine, globals) else {
        return None;
    };
    let (head, rest) = ty.trim().split_once(char::is_whitespace)?;
    if !head.eq_ignore_ascii_case("codeunit") {
        return None;
    }
    let ParsedType::Object { object_ref, .. } = parse_object_kind_type(ObjectKind::Codeunit, rest)
    else {
        return None;
    };
    match index.resolve_object_ref_strict(graph, from.clone(), ObjectKind::Codeunit, &object_ref) {
        ObjectRefResolution::Unique(id) => Some(id),
        _ => None,
    }
}

/// Step 2's input, recorded on a call site whose one target is a source
/// routine with an `Interface` parameter: per argument, the codeunit of a
/// codeunit-typed variable passed there (`None` for any other argument).
/// Empty when the target has no interface parameter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn argument_codeunits(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    from: &ObjectNodeId,
    routine: &RoutineDecl,
    globals: &[VarDecl],
    ir: &al_syntax::ir::Ir,
    args: &[ExprId],
    target: &RoutineNodeId,
) -> Vec<Option<ObjectNodeId>> {
    let Ok(i) = graph.routines.binary_search_by(|r| r.id.cmp(target)) else {
        return Vec::new();
    };
    let has_interface_param = graph.routines[i]
        .param_sig_key
        .split('|')
        .any(|t| t.trim_start().starts_with("interface"));
    if !has_interface_param {
        return Vec::new();
    }
    args.iter()
        .map(|&a| {
            let n = name_of(ir, unparen(ir, a))?;
            match &ir.expr(unparen(ir, a)).kind {
                ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) => {
                    codeunit_of(graph, index, from, &n, routine, globals)
                }
                _ => None,
            }
        })
        .collect()
}

/// Step 2: close every holder site whose writing routines' callers all pass a
/// codeunit variable (see the module doc). `edges` are the workspace's phase-1
/// edges, `facts` their site facts.
pub(crate) fn close_holder_sites(
    graph: &ProgramGraph,
    edges: &mut [ClassifiedEdge],
    facts: &mut HashMap<ObligationId, SiteFacts>,
) {
    if !graph.workspace_friendless {
        return;
    }
    let mut callers: HashMap<&RoutineNodeId, Vec<&ObligationId>> = HashMap::new();
    for e in edges.iter() {
        for r in &e.edge.routes {
            if let RouteTarget::Routine(rid) = &r.target {
                callers.entry(rid).or_default().push(&e.obligation_id);
            }
        }
    }
    let mut closed: Vec<(ObligationId, Vec<ObjectNodeId>)> = Vec::new();
    for e in edges.iter() {
        let Some(f) = facts.get(&e.obligation_id) else {
            continue;
        };
        let (Some(sources), Some(interface_lc)) = (&f.interface_holder, &f.interface) else {
            continue;
        };
        if !f.interface_closed_over.is_empty() {
            continue;
        }
        let Some(mut over) = sources
            .iter()
            .map(|s| source_codeunits(graph, &callers, facts, s))
            .collect::<Option<Vec<Vec<ObjectNodeId>>>>()
            .map(|v| v.concat())
        else {
            continue;
        };
        over.sort();
        over.dedup();
        let real = e.edge.routes.iter().all(|r| {
            !matches!(r.target, RouteTarget::Unresolved)
                && !matches!(r.evidence, Evidence::Unknown(_))
        });
        let reached: Vec<ObjectNodeId> = e
            .edge
            .routes
            .iter()
            .filter_map(|r| r.target_object())
            .collect();
        let implements = |id: &ObjectNodeId| {
            graph.object(id).is_some_and(|o| {
                o.implements
                    .iter()
                    .any(|i| fold_ref_name(i) == *interface_lc)
            })
        };
        if real && !over.is_empty() && over.iter().all(|c| implements(c) && reached.contains(c)) {
            closed.push((e.obligation_id.clone(), over));
        }
    }
    let at: HashMap<&ObligationId, usize> = edges
        .iter()
        .enumerate()
        .map(|(i, e)| (&e.obligation_id, i))
        .collect();
    let patches: Vec<(usize, ObligationId, Vec<ObjectNodeId>)> = closed
        .into_iter()
        .map(|(o, over)| (at[&o], o, over))
        .collect();
    // The receiver holds one of `over` (or nothing: a runtime error), so the
    // routes to every other implementer cannot run: dropped. The model adapter
    // does not list those implementers as unreached (`DispatchMeta`).
    for (i, o, over) in patches {
        let edge = &mut edges[i].edge;
        edge.completeness = SetCompleteness::Complete;
        edge.routes
            .retain(|r| r.target_object().is_some_and(|t| over.contains(&t)));
        if let Some(f) = facts.get_mut(&o) {
            f.interface_closed_over = over;
        }
    }
}

/// The codeunits one source stores: itself, or what every call of the writing
/// routine passes at that parameter. `None` when the routine may be called
/// from code that is not resolved, or one call passes anything else.
fn source_codeunits(
    graph: &ProgramGraph,
    callers: &HashMap<&RoutineNodeId, Vec<&ObligationId>>,
    facts: &HashMap<ObligationId, SiteFacts>,
    source: &HolderSource,
) -> Option<Vec<ObjectNodeId>> {
    let (rid, p) = match source {
        HolderSource::Codeunit(c) => return Some(vec![c.clone()]),
        HolderSource::Param(rid, p) => (rid, *p),
    };
    let i = graph.routines.binary_search_by(|r| r.id.cmp(rid)).ok()?;
    let node = &graph.routines[i];
    if !matches!(node.access, Access::Local | Access::Internal)
        || !node.event_subscribers.is_empty()
    {
        return None;
    }
    callers
        .get(rid)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|c| facts.get(*c)?.argument_codeunits.get(p)?.clone())
        .collect()
}
