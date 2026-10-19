//! The resolved program as a value (#160 step 1b, spec §4.3): one program build,
//! split into what outlives the body ([`ProgramMeta`]) and the body itself
//! ([`ProgramBody`]: the context and the resolver's per-site output).
//!
//! The two edge sets stay apart. `edges` is the resolver's report (the primary
//! app's routines plus the event-flow edges of every publisher in every app);
//! `dependency_body_edges` is one edge per dependency call site. A dependency
//! publisher can sit in both, so the order `edges` then `dependency_body_edges` is
//! a contract (the graph projection keeps the first edge's `via` on a `conf` tie,
//! the adapter takes the first edge at a span). [`ProgramBody::all_edges`] and
//! [`EdgeView::all_edges`] are the only ways a reader walks both.

use std::collections::HashMap;
use std::sync::Arc;

use crate::program::inputs::InputSet;
use crate::program::node::AppRef;
use crate::program::profile::{BuildProfile, DependencyBodies};
use crate::program::resolve::edge::Histogram;
use crate::program::resolve::full::{
    BuiltinDispatchAudit, ClassifiedEdge, Coverage, FreshCoverage, ObligationId, ParenlessCalls,
    ProgramContext, ProgramReport, SiteFacts,
};

/// One program build: [`ProgramMeta`] and, until a reader takes it, the body.
pub struct ResolvedProgram {
    pub meta: ProgramMeta,
    pub body: Option<ProgramBody>,
}

/// What a step 0-2 reader reads of a build besides the body (spec §4.3.1).
pub struct ProgramMeta {
    /// The set the build read (shared with the command's own `Arc`).
    pub inputs: Arc<InputSet>,
    pub primary_app_ref: AppRef,
    pub recovered_files: Vec<String>,
    /// Carries `files_without_object` (§4.2.4).
    pub coverage: FreshCoverage,
    /// Over `edges` only, as the resolver computed them.
    pub histogram: Histogram,
    pub primary_histogram: Histogram,
    /// The resolver's coverage contract (`ProgramReport::coverage`).
    pub coverage_contract: Coverage,
    /// [`crate::program::resolve::full::loaded_apps`] of this build.
    pub loaded_apps: Vec<serde_json::Value>,
}

/// The context and the resolver's per-site output. Dropped after the adapter.
pub struct ProgramBody {
    /// Graph, parsed units, dependency trees (per profile).
    pub ctx: ProgramContext,
    /// The resolver's `ProgramReport::edges`: every workspace routine's edges plus
    /// the event-flow edges of publishers in every app.
    pub edges: Vec<ClassifiedEdge>,
    /// `ProgramContext::resolve_dependency_bodies().edges`: one edge per dependency
    /// call site. Empty under `DependencyBodies::Summary`.
    pub dependency_body_edges: Vec<ClassifiedEdge>,
    /// Both sets' site facts, merged.
    pub site_facts: HashMap<ObligationId, SiteFacts>,
    /// Both sets' parens-less calls, merged.
    pub parenless_calls: ParenlessCalls,
    pub builtin_dispatch_audit: BuiltinDispatchAudit,
    pub event_flow_dual_publisher_alias_skips: usize,
}

impl ProgramBody {
    /// `edges`, then `dependency_body_edges` (the order is a contract, see the
    /// module doc).
    pub fn all_edges(&self) -> impl Iterator<Item = &ClassifiedEdge> + Clone {
        self.edges.iter().chain(&self.dependency_body_edges)
    }

    /// The adapter family's view of this body.
    #[must_use]
    pub fn edge_view(&self, primary_app_ref: AppRef) -> EdgeView<'_> {
        EdgeView {
            primary_app_ref,
            edges: &self.edges,
            dependency_body_edges: &self.dependency_body_edges,
            site_facts: &self.site_facts,
        }
    }
}

/// What the adapter family reads of a resolved program: the primary app, both
/// edge sets and the merged site facts. Borrowed, `Copy`.
#[derive(Clone, Copy)]
pub struct EdgeView<'r> {
    pub primary_app_ref: AppRef,
    pub edges: &'r [ClassifiedEdge],
    pub dependency_body_edges: &'r [ClassifiedEdge],
    pub site_facts: &'r HashMap<ObligationId, SiteFacts>,
}

impl<'r> EdgeView<'r> {
    /// Same order as [`ProgramBody::all_edges`].
    pub fn all_edges(self) -> impl Iterator<Item = &'r ClassifiedEdge> + Clone {
        self.edges.iter().chain(self.dependency_body_edges)
    }
}

impl ProgramReport {
    /// A report straight from the resolver: `edges` = `report.edges`, no
    /// dependency bodies.
    #[must_use]
    pub fn edge_view(&self) -> EdgeView<'_> {
        EdgeView {
            primary_app_ref: self.primary_app_ref,
            edges: &self.edges,
            dependency_body_edges: &[],
            site_facts: &self.site_facts,
        }
    }
}

impl ResolvedProgram {
    /// The program build (`build_program_with_coverage_profiled`) plus, under
    /// `DependencyBodies::Keep`, every source-bearing dependency body resolved
    /// from its own app, its edges kept apart and its site facts and parens-less
    /// calls merged.
    ///
    /// It stops at the edges. The demand filter
    /// (`program::resolve::demand::cross_app_demand`) is the detector model's
    /// choice; the call-graph export must not apply it (it would remove dependency
    /// code reachable only through interface implementations, extension triggers
    /// or run hubs).
    pub fn build(inputs: &Arc<InputSet>, profile: BuildProfile) -> Result<ResolvedProgram, String> {
        use crate::engine::perf_trace as pt;
        let keep = profile.dependency_bodies == DependencyBodies::Keep;
        let (ctx, mut report, coverage) = {
            let _s = keep.then(|| pt::span("crossapp", "crossapp.program_build_full"));
            crate::program::resolve::full::build_program_with_coverage_profiled(inputs, profile)?
        };
        let dependency_body_edges = if keep {
            let dependency = {
                let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
                ctx.resolve_dependency_bodies()
            };
            report.site_facts.extend(dependency.site_facts);
            report.parenless_calls.extend(dependency.parenless_calls);
            dependency.edges
        } else {
            Vec::new()
        };
        let loaded_apps = crate::program::resolve::full::loaded_apps(&ctx, &coverage);
        let ProgramReport {
            edges,
            coverage: coverage_contract,
            histogram,
            primary_histogram,
            abi_integrity: _,
            primary_app_ref,
            event_flow_dual_publisher_alias_skips,
            recovered_files,
            builtin_dispatch_audit,
            site_facts,
            parenless_calls,
        } = report;
        Ok(ResolvedProgram {
            meta: ProgramMeta {
                inputs: Arc::clone(inputs),
                primary_app_ref,
                recovered_files,
                coverage,
                histogram,
                primary_histogram,
                coverage_contract,
                loaded_apps,
            },
            body: Some(ProgramBody {
                ctx,
                edges,
                dependency_body_edges,
                site_facts,
                parenless_calls,
                builtin_dispatch_audit,
                event_flow_dual_publisher_alias_skips,
            }),
        })
    }
}

#[cfg(test)]
impl ProgramBody {
    /// A real body (a context needs a build), for tests that then state its edge
    /// sets by assignment.
    fn for_test() -> ProgramBody {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let inputs = InputSet::read(&ws, &Default::default());
        ResolvedProgram::build(&inputs, BuildProfile::FULL)
            .expect("fixture builds")
            .body
            .expect("built once")
    }
}

/// A copy of the body's first edge whose caller is told apart by `params_count`.
#[cfg(test)]
fn edge_for_test(body: &ProgramBody, params_count: usize) -> ClassifiedEdge {
    let first = body.all_edges().next().expect("fixture has an edge");
    let mut edge = first.edge.clone();
    edge.from.params_count = params_count;
    ClassifiedEdge {
        obligation_id: first.obligation_id.clone(),
        edge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §4.3.2 / §8.3: `all_edges` yields `edges`, then
    /// `dependency_body_edges` (the order the projection's `conf` tie and the
    /// adapter's first-edge rule read), both on the body and on its view. The two
    /// sets are hand-stated by assignment.
    #[test]
    fn all_edges_is_edges_then_dependency_bodies() {
        let mut body = ProgramBody::for_test();
        let a = edge_for_test(&body, 101);
        let b = edge_for_test(&body, 202);
        let (a_from, b_from) = (a.edge.from.clone(), b.edge.from.clone());
        body.edges = vec![a];
        body.dependency_body_edges = vec![b];
        let order = |it: &mut dyn Iterator<Item = &ClassifiedEdge>| {
            it.map(|e| e.edge.from.clone()).collect::<Vec<_>>()
        };
        assert_eq!(
            order(&mut body.all_edges()),
            [a_from.clone(), b_from.clone()]
        );
        assert_eq!(
            order(&mut body.edge_view(AppRef(0)).all_edges()),
            [a_from, b_from]
        );
    }
}
