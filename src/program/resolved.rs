//! The resolved program as a value (#160 step 1b, spec §4.3): one program build,
//! split into what outlives the body ([`ProgramMeta`]) and the body itself
//! ([`ProgramBody`]: the context and the resolver's per-site output).
//!
//! The body is not optional. A reader of the whole build takes
//! `&ResolvedProgram` (`alsem graph`, [`crate::program::impact_graph::export_from_program`]);
//! the model builder consumes the `ResolvedProgram` and hands back its
//! [`ProgramMeta`] ([`crate::program::model::program_calls::attach_program_calls`]).
//! So a shared program can be read any number of times before the one consumer
//! releases its body, and a read after the release does not compile.
//!
//! The two edge sets stay apart. `edges` is the resolver's report (the primary
//! app's routines plus the event-flow edges of every publisher in every app);
//! `dependency_body_edges` is one edge per dependency call site. A dependency
//! publisher can sit in both, so the order `edges` then `dependency_body_edges` is
//! a contract (the graph projection keeps the first edge's `via` on a `conf` tie,
//! the adapter takes the first edge at a span). Both fields are private:
//! [`ProgramBody::all_edges`] and [`EdgeView::all_edges`] are the only ways a
//! reader walks both.

use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::census::{Kind, Tally};
use crate::program::inputs::InputSet;
use crate::program::node::AppRef;
use crate::program::profile::{BuildProfile, DependencyBodies};
use crate::program::resolve::edge::Histogram;
use crate::program::resolve::full::{
    ClassifiedEdge, Coverage, FreshCoverage, ObligationId, ParenlessCalls, ProgramContext,
    ProgramReport, SiteFacts,
};

/// What a build resolves. The scope is a meaning, not a retention choice: it
/// fixes both the build profile and whether the dependency bodies' call sites
/// get edges, so no caller can ask for one without the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramScope {
    /// The workspace's routines: `DependencyBodies::Summary`, no dependency
    /// call-site edges (`dependency_body_edges` is empty). The single-app model.
    Workspace,
    /// The workspace plus every source-bearing dependency body, each resolved
    /// from its own app: `BuildProfile::FULL` and
    /// `ProgramContext::resolve_dependency_bodies`. The cross-app model and
    /// `alsem graph`.
    CrossApp,
}

impl ProgramScope {
    fn profile(self) -> BuildProfile {
        match self {
            ProgramScope::Workspace => BuildProfile {
                dependency_bodies: DependencyBodies::Summary,
            },
            ProgramScope::CrossApp => BuildProfile::FULL,
        }
    }
}

/// One program build: [`ProgramMeta`] and the body. Both are private and read
/// through [`ResolvedProgram::meta`] / [`ResolvedProgram::body`], so the meta that
/// describes a body (its scope above all) cannot be swapped onto another build's
/// body (final panel F2-3):
///
/// ```compile_fail,E0616
/// use al_sem::program::resolved::ResolvedProgram;
/// fn swap(a: &mut ResolvedProgram, b: &mut ResolvedProgram) {
///     std::mem::swap(&mut a.meta, &mut b.meta);
/// }
/// ```
pub struct ResolvedProgram {
    meta: ProgramMeta,
    body: ProgramBody,
}

/// What a step 0-2 reader reads of a build besides the body (spec §4.3.1), and
/// what the model builder hands back when it releases the body.
pub struct ProgramMeta {
    /// The set the build read (shared with the command's own `Arc`).
    pub inputs: Arc<InputSet>,
    /// What the build resolved. Private: only [`ResolvedProgram::build`] sets it,
    /// so a reader that gates on it (the call-graph export) cannot be talked past.
    scope: ProgramScope,
    pub recovered_files: Vec<String>,
    /// Carries `files_without_object` (§4.2.4).
    pub coverage: FreshCoverage,
    /// Over `edges` only, as the resolver computed them.
    pub histogram: Histogram,
    pub primary_histogram: Histogram,
    /// The resolver's coverage contract (`ProgramReport::coverage`).
    pub coverage_contract: Coverage,
}

/// The context and the resolver's per-site output. Dropped after the adapter.
pub struct ProgramBody {
    /// Graph, parsed units, dependency trees (per scope). Its `primary_app_ref`
    /// is the build's one primary app.
    pub ctx: ProgramContext,
    /// The resolver's `ProgramReport::edges`: every workspace routine's edges plus
    /// the event-flow edges of publishers in every app.
    edges: Vec<ClassifiedEdge>,
    /// `ProgramContext::resolve_dependency_bodies().edges`: one edge per dependency
    /// call site. Empty under [`ProgramScope::Workspace`].
    dependency_body_edges: Vec<ClassifiedEdge>,
    /// Both sets' site facts, merged.
    pub site_facts: HashMap<ObligationId, SiteFacts>,
    /// Both sets' parens-less calls, merged.
    pub parenless_calls: ParenlessCalls,
}

impl ProgramMeta {
    /// What the build resolved. A caller cannot change it, even on a meta it owns
    /// (final panel F2-3):
    ///
    /// ```compile_fail,E0616
    /// use al_sem::program::resolved::{ProgramMeta, ProgramScope};
    /// fn widen(m: &mut ProgramMeta) {
    ///     m.scope = ProgramScope::CrossApp;
    /// }
    /// ```
    #[must_use]
    pub fn scope(&self) -> ProgramScope {
        self.scope
    }
}

impl ProgramBody {
    /// `edges`, then `dependency_body_edges` (the order is a contract, see the
    /// module doc).
    pub fn all_edges(&self) -> impl Iterator<Item = &ClassifiedEdge> + Clone {
        self.edges.iter().chain(&self.dependency_body_edges)
    }

    /// The dependency bodies' call-site edges alone (every one a `CallSite`
    /// from a dependency app, checked when built).
    #[must_use]
    pub fn dependency_body_edges(&self) -> &[ClassifiedEdge] {
        &self.dependency_body_edges
    }

    /// Drop the body: the resolver's per-site output first, then the context
    /// (the `b3.report_drop` and `b3.ctx_drop` spans).
    pub(crate) fn release(self) {
        use crate::engine::perf_trace as pt;
        let ProgramBody {
            ctx,
            edges,
            dependency_body_edges,
            site_facts,
            parenless_calls,
        } = self;
        {
            let _s = pt::span("b3", "b3.report_drop");
            drop((edges, dependency_body_edges, site_facts, parenless_calls));
        }
        let _s = pt::span("b3", "b3.ctx_drop");
        drop(ctx);
    }

    /// The adapter family's view of this body, for the context's primary app.
    #[must_use]
    pub fn edge_view(&self) -> EdgeView<'_> {
        EdgeView {
            primary_app_ref: self.ctx.primary_app_ref,
            edges: &self.edges,
            dependency_body_edges: &self.dependency_body_edges,
            site_facts: &self.site_facts,
        }
    }
}

/// What the adapter family reads of a resolved program: the primary app, both
/// edge sets and the merged site facts. Borrowed, `Copy`. Built only by
/// [`ProgramBody::edge_view`] and [`ProgramReport::edge_view`].
#[derive(Clone, Copy)]
pub struct EdgeView<'r> {
    pub primary_app_ref: AppRef,
    edges: &'r [ClassifiedEdge],
    dependency_body_edges: &'r [ClassifiedEdge],
    pub site_facts: &'r HashMap<ObligationId, SiteFacts>,
}

impl<'r> EdgeView<'r> {
    /// Same order as [`ProgramBody::all_edges`].
    pub fn all_edges(self) -> impl Iterator<Item = &'r ClassifiedEdge> + Clone {
        self.edges.iter().chain(self.dependency_body_edges)
    }

    /// A view over two hand-stated edge sets, in the contract's order.
    #[cfg(test)]
    pub(crate) fn for_test(
        primary_app_ref: AppRef,
        edges: &'r [ClassifiedEdge],
        dependency_body_edges: &'r [ClassifiedEdge],
        site_facts: &'r HashMap<ObligationId, SiteFacts>,
    ) -> EdgeView<'r> {
        EdgeView {
            primary_app_ref,
            edges,
            dependency_body_edges,
            site_facts,
        }
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
    /// The program build (`build_program_with_coverage_profiled`) at `scope`'s
    /// profile plus, under [`ProgramScope::CrossApp`], every source-bearing
    /// dependency body resolved from its own app, its edges kept apart and its
    /// site facts and parens-less calls merged.
    ///
    /// It stops at the edges. The demand filter
    /// (`program::resolve::demand::cross_app_demand`) is the detector model's
    /// choice; the call-graph export must not apply it (it would remove dependency
    /// code reachable only through interface implementations, extension triggers
    /// or run hubs).
    pub fn build(inputs: &Arc<InputSet>, scope: ProgramScope) -> Result<ResolvedProgram, String> {
        use crate::engine::perf_trace as pt;
        let cross_app = scope == ProgramScope::CrossApp;
        let (ctx, mut report, coverage) = {
            let _s = cross_app.then(|| pt::span("crossapp", "crossapp.program_build_full"));
            crate::program::resolve::full::build_program_with_coverage_profiled(
                inputs,
                scope.profile(),
            )?
        };
        let dependency_body_edges = if cross_app {
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
        // What `ProgramBody::dependency_body_edges` promises its readers (the
        // graph's `dependencyBodies` histogram reads the slice as is).
        debug_assert!(dependency_body_edges.iter().all(|ce| {
            ce.edge.from.object.app != ctx.primary_app_ref
                && matches!(ce.obligation_id, ObligationId::CallSite { .. })
        }));
        // Nothing reads the dispatch audit, the alias-skip count or the ABI
        // integrity of this build: they drop here.
        let ProgramReport {
            edges,
            coverage: coverage_contract,
            histogram,
            primary_histogram,
            abi_integrity: _,
            primary_app_ref: _,
            event_flow_dual_publisher_alias_skips: _,
            recovered_files,
            builtin_dispatch_audit: _,
            site_facts,
            parenless_calls,
        } = report;
        Ok(ResolvedProgram {
            meta: ProgramMeta {
                inputs: Arc::clone(inputs),
                scope,
                recovered_files,
                coverage,
                histogram,
                primary_histogram,
                coverage_contract,
            },
            body: ProgramBody {
                ctx,
                edges,
                dependency_body_edges,
                site_facts,
                parenless_calls,
            },
        })
    }

    /// What the build read and resolved, besides the body.
    #[must_use]
    pub fn meta(&self) -> &ProgramMeta {
        &self.meta
    }

    /// The context and the resolver's per-site output.
    #[must_use]
    pub fn body(&self) -> &ProgramBody {
        &self.body
    }

    /// The meta and the body, apart: for the one consumer that releases the body
    /// and keeps the meta (`attach_program_calls`), and for a caller that is done
    /// with the build.
    pub(crate) fn into_parts(self) -> (ProgramMeta, ProgramBody) {
        (self.meta, self.body)
    }

    /// [`crate::program::resolve::full::loaded_apps`] of this build (it reads
    /// the context, so only while the body is alive).
    #[must_use]
    pub fn loaded_apps(&self) -> Vec<serde_json::Value> {
        crate::program::resolve::full::loaded_apps(&self.body.ctx, &self.meta.coverage)
    }

    /// #160 §4.1.5: the `program.snapshot_texts`, `program.report` and `inputs`
    /// buckets, printed; #160 steps 10 + 11a (§4.13): `program.edge_site` (the
    /// edges' site keys, a part of `program.report`) and `program.file_table`.
    pub(crate) fn census(&self) {
        self.snapshot_texts_tally()
            .emit("program.snapshot_texts", Kind::Exact);
        self.report_tally().emit("program.report", Kind::Shallow);
        let b = &self.body;
        let edges = b.edges.len() + b.dependency_body_edges.len();
        Tally {
            bytes: edges * std::mem::size_of::<Option<crate::program::site_key::ProgramSite>>(),
            allocs: 0,
        }
        .emit("program.edge_site", Kind::Exact);
        b.ctx.files.census().emit("program.file_table", Kind::Exact);
        self.meta.inputs.census().emit("inputs", Kind::Exact);
    }

    /// `program.snapshot_texts`: every source text of every DEPENDENCY app in
    /// the snapshot, each shared `Arc<str>` once (exact). The workspace texts are
    /// the `inputs` bucket's, which shares them by `Arc`.
    fn snapshot_texts_tally(&self) -> Tally {
        let mut seen = std::collections::HashSet::new();
        let mut texts = Tally::default();
        let snap = self.body.ctx.snapshot();
        for app in snap.apps.iter().filter(|u| u.id != snap.workspace_app) {
            for f in app.source.iter().flat_map(|s| s.files.iter()) {
                if seen.insert(Arc::as_ptr(&f.text).cast::<u8>()) {
                    // `ArcInner<str>`: strong + weak counts, then the bytes, padded
                    // to the counts' alignment — the requested allocation size.
                    texts.bytes += (2 * std::mem::size_of::<usize>() + f.text.len())
                        .next_multiple_of(std::mem::align_of::<usize>());
                    texts.allocs += 1;
                }
            }
        }
        texts
    }

    /// `program.report` (shallow): the resolver's per-site output (both edge
    /// sets, site facts, parens-less calls) and what of its report the meta
    /// keeps (the coverage contract's `missing`/`extra`, the recovered files).
    fn report_tally(&self) -> Tally {
        let (b, m) = (&self.body, &self.meta);
        let mut r = Tally::default();
        r.buf::<ClassifiedEdge>(b.edges.len())
            .buf::<ClassifiedEdge>(b.dependency_body_edges.len())
            .buf::<(ObligationId, SiteFacts)>(b.site_facts.len())
            .buf::<(
                (AppRef, String),
                std::collections::HashSet<al_syntax::ir::ExprId>,
            )>(b.parenless_calls.len())
            .strs(b.parenless_calls.keys().map(|(_, f)| f))
            .buf::<ObligationId>(m.coverage_contract.missing.len())
            .buf::<ObligationId>(m.coverage_contract.extra.len())
            .buf::<String>(m.recovered_files.len())
            .strs(&m.recovered_files);
        for ids in b.parenless_calls.values() {
            r.buf::<al_syntax::ir::ExprId>(ids.len());
        }
        r
    }
}

#[cfg(test)]
impl ResolvedProgram {
    /// A real program (a context needs a build), for tests that then state its
    /// edge sets by assignment.
    fn for_test(scope: ProgramScope) -> ResolvedProgram {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let inputs = InputSet::read(&ws, &Default::default());
        ResolvedProgram::build(&inputs, scope).expect("fixture builds")
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
        site: first.site,
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
        let mut body = ResolvedProgram::for_test(ProgramScope::CrossApp).body;
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
        assert_eq!(order(&mut body.edge_view().all_edges()), [a_from, b_from]);
    }

    /// Code review CR3: `program.report` charges what the meta keeps of the
    /// resolver's report. Hand-stated: one recovered file and one missing and
    /// one extra obligation, set by assignment on a real build, add exactly their
    /// buffers and the file name's bytes.
    #[test]
    fn report_tally_counts_the_meta_rows() {
        let mut p = ResolvedProgram::for_test(ProgramScope::Workspace);
        p.meta.recovered_files.clear();
        p.meta.coverage_contract.missing.clear();
        p.meta.coverage_contract.extra.clear();
        let before = p.report_tally();
        let id = edge_for_test(&p.body, 1).obligation_id;
        p.meta.recovered_files = vec!["App::Recovered.al".to_string()];
        p.meta.coverage_contract.missing = vec![id.clone()];
        p.meta.coverage_contract.extra = vec![id];
        let after = p.report_tally();
        let size = std::mem::size_of::<ObligationId>();
        assert_eq!(
            after.bytes - before.bytes,
            2 * size + std::mem::size_of::<String>() + "App::Recovered.al".len()
        );
        assert_eq!(after.allocs - before.allocs, 4);
    }

    /// Code review CR11 (the function; the use is pinned by
    /// `cli_build_counters::retained_census_changes_no_output`):
    /// `program.snapshot_texts` leaves the workspace texts to the `inputs`
    /// bucket. The fixture has no dependency source, so the bucket is empty
    /// while the workspace has text.
    #[test]
    fn snapshot_texts_exclude_the_workspace() {
        let p = ResolvedProgram::for_test(ProgramScope::CrossApp);
        assert!(
            p.meta
                .inputs
                .al_files
                .iter()
                .any(|f| f.text.as_ref().is_ok_and(|t| !t.is_empty())),
            "the precondition: the workspace has text"
        );
        let t = p.snapshot_texts_tally();
        assert_eq!((t.bytes, t.allocs), (0, 0));
    }
}
