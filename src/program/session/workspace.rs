//! The workspace session (context-server design §6, stage 4 prefix): given the
//! state retained from the previous revision and the next revision's inputs, it
//! produces the next revision's program and model, equal to a cold build of the
//! same inputs.
//!
//! What is retained (§4.1's left column, the part this prefix implements):
//! - the dependency tier (`Arc<DepNodes>`, held strongly, so the session's
//!   [`DepCache`] keeps hitting it while `DepKey` is unchanged);
//! - each workspace file's parse (L0, reused for byte-equal text), its
//!   declaration projection (L1, §6.1) and its pre-refinement
//!   `FileResolution` (L2, §6.6);
//! - the dependency bodies' resolution.
//!
//! What every revision reruns whole, exactly as the cold build runs it: the
//! snapshot, the graph assembly from the (partly reused) parse, the
//! whole-program refinement `close_holder_sites` and the event-flow edges
//! (L2b), the demand (L2c), the model assembly, adapter, event graph and the
//! three final passes (L3-L8): `cross_app_program_from`, the cold code itself.
//!
//! The rules (§6.1, §6.6):
//! - **P** (no retained state, `DepKey` changed, the root `app.json` changed,
//!   or a source dependency's content changed): a cold build; nothing reused
//!   but the parse of byte-equal files.
//! - **S** (a projection changed, or a file was added, removed or renamed):
//!   every workspace file re-resolved, every dependency body re-resolved.
//! - **B** (every changed file kept its projection): only the changed files
//!   are re-resolved; the other files' and the dependency bodies' retained
//!   resolutions are reused, their source witnesses re-materialized against
//!   the current declaration surface (§6.5).

use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::perf_trace as pt;
use crate::program::dep_cache::{DepCache, DepKey, DepNodes};
use crate::program::inputs::WorkspaceInputs;
use crate::program::model::program_calls::{CrossAppFailure, CrossAppProgram};
use crate::program::model::row_cache::{RowCache, RowCacheStats};
use crate::program::model::workspace::AssemblyNeeds;
use crate::program::profile::BuildProfile;
use crate::program::resolve::full::{DependencyBodyResolution, FileResolution};
use crate::program::resolved::{ProgramScope, ResolvedProgram};
use crate::program::session::classify::{ChangeClass, DeclProjection, classify_files};
use crate::snapshot::{AppSetSnapshot, DependencySource, SnapshotBuilder};

/// One workspace file as the last revision left it.
struct RetainedFile {
    text: Arc<str>,
    tree: Arc<al_syntax::ir::AlFile>,
    projection: DeclProjection,
    /// The pre-refinement resolution (before `close_holder_sites`).
    resolution: Arc<FileResolution>,
}

/// What the session keeps between revisions.
struct Retained {
    /// Held so the session's cache keeps the tier alive.
    _tier: Arc<DepNodes>,
    dep_key: DepKey,
    identity: blake3::Hash,
    files: HashMap<String, RetainedFile>,
    dependency: Arc<DependencyBodyResolution>,
}

/// What one refresh did (the `revision.refresh` record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshReport {
    pub class: ChangeClass,
    /// Workspace files whose text changed, plus added and removed ones, sorted.
    pub changed: Vec<String>,
    /// Workspace files parsed in this revision (the others reused their tree).
    pub parsed: usize,
    /// Workspace files resolved in this revision (the others reused theirs).
    pub resolved: usize,
    /// Whether the dependency bodies were resolved again.
    pub dependency_bodies_resolved: bool,
    /// Under B: whether the reused dependency edges had to be copied (a
    /// witness into a moved workspace declaration changed); otherwise they
    /// are shared with the previous revision.
    pub dependency_edges_copied: bool,
    /// Bytes copied out of the retained state into this revision (the
    /// `model.copied_bytes` counter): the reused resolutions' buffers.
    pub copied_bytes: u64,
    /// The program part ([`WorkspaceSession::refresh_program`]).
    pub program_ms: u128,
    /// The whole refresh, inputs given (the `revision.refresh` span); equal to
    /// `program_ms` after a `refresh_program` alone.
    pub wall_ms: u128,
    /// What the model assembly took from the row cache (`None` until
    /// [`WorkspaceSession::refresh_model`] ran for this revision).
    pub rows: Option<RowCacheStats>,
}

/// The incremental cross-app workspace model.
pub struct WorkspaceSession {
    dep_cache: DepCache,
    retained: Option<Retained>,
    /// The raw model rows of the last revision (§6.8 L3/L4); dropped with the
    /// retained state on a P change, so no kept row outlives its packages.
    rows: RowCache,
    last: Option<RefreshReport>,
}

impl Default for WorkspaceSession {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSession {
    #[must_use]
    pub fn new() -> Self {
        WorkspaceSession {
            dep_cache: DepCache::default(),
            retained: None,
            rows: RowCache::default(),
            last: None,
        }
    }

    /// What the last refresh did.
    #[must_use]
    pub fn last_report(&self) -> Option<&RefreshReport> {
        self.last.as_ref()
    }

    /// The next revision's cross-app model from `inputs` (the whole refresh,
    /// under the `revision.refresh` span): the program
    /// ([`Self::refresh_program`]), then the cold model pipeline over it.
    pub fn refresh(
        &mut self,
        inputs: &Arc<WorkspaceInputs>,
        skip_roots_config: bool,
        needs: AssemblyNeeds,
    ) -> Result<CrossAppProgram, CrossAppFailure> {
        let start = std::time::Instant::now();
        let _s = pt::span("revision", "revision.refresh");
        let program = self.refresh_program(inputs);
        let out = self.refresh_model(program, inputs, skip_roots_config, needs);
        if let Some(r) = self.last.as_mut() {
            r.wall_ms = start.elapsed().as_millis();
            pt::counter("model.copied_bytes", r.copied_bytes);
        }
        out
    }

    /// The cross-app model over `program` (the one [`Self::refresh_program`]
    /// just returned): the cold model pipeline, with every file's raw rows
    /// taken from the session's row cache when their inputs are unchanged
    /// (§6.8 L3/L4, `model::row_cache`). Equal to
    /// `cross_app_program_from(program, …)`.
    pub fn refresh_model(
        &mut self,
        program: Result<ResolvedProgram, String>,
        inputs: &Arc<WorkspaceInputs>,
        skip_roots_config: bool,
        needs: AssemblyNeeds,
    ) -> Result<CrossAppProgram, CrossAppFailure> {
        let out = crate::program::model::program_calls::cross_app_program_from_cached(
            program,
            inputs,
            skip_roots_config,
            needs,
            &mut self.rows,
        );
        if let Some(r) = self.last.as_mut() {
            r.rows = Some(self.rows.last_stats());
        }
        out
    }

    /// The next revision's program (L0-L2b plus the dependency bodies) from
    /// `inputs`, reusing what the rules allow (see the module doc). Equal to
    /// `ResolvedProgram::build(inputs, ProgramScope::CrossApp)`.
    pub fn refresh_program(
        &mut self,
        inputs: &Arc<WorkspaceInputs>,
    ) -> Result<ResolvedProgram, String> {
        use crate::program::resolve::full as f;
        let start = std::time::Instant::now();
        pt::counter_delta("build.program", 1);
        let profile = BuildProfile::FULL;
        let (snap, load) = {
            let _s = pt::span("preflight", "preflight.snapshot_build");
            (SnapshotBuilder {
                workspace_root: inputs.workspace.clone(),
                local_providers: vec![],
            })
            .build_with_inputs(inputs, DependencySource::default(), &self.dep_cache)
            .map_err(|e| format!("snapshot build failed: {e:#}"))?
        };
        let dep_key = DepKey::of(&snap, profile);
        let identity = identity_of(&snap, inputs);
        let package_change = self
            .retained
            .as_ref()
            .is_none_or(|r| r.dep_key != dep_key || r.identity != identity);
        if package_change {
            // P: a full rebuild. The old tier is released; the cache misses on
            // the new key and builds it from the packages.
            self.retained = None;
            self.rows = RowCache::default();
        }

        // L0: the parse, reusing the tree of every byte-equal file.
        let retained = self.retained.as_ref();
        let reuse = |path: &str, text: &str| -> Option<Arc<al_syntax::ir::AlFile>> {
            let r = retained?.files.get(path)?;
            (*r.text == *text).then(|| Arc::clone(&r.tree))
        };
        let ctx =
            f::build_context_from_snapshot_reusing(snap, profile, &self.dep_cache, Some(&reuse))?;
        let primary = ctx.primary_app_ref;

        // L1: the projections, and the class (§6.1).
        let ws_files: Vec<&crate::snapshot::ParsedFile> = ctx
            .parsed
            .iter()
            .filter(|u| ctx.graph.apps.find(&u.app) == Some(primary))
            .flat_map(|u| u.files.iter())
            .filter(|pf| ctx.ws_file_set.contains(&pf.virtual_path))
            .collect();
        let mut parsed = 0usize;
        let mut changed: Vec<String> = Vec::new();
        let projections: Vec<(String, DeclProjection)> = ws_files
            .iter()
            .map(|pf| {
                let kept = retained
                    .and_then(|r| r.files.get(&pf.virtual_path))
                    .filter(|r| Arc::ptr_eq(&r.tree, &pf.file));
                let p = match kept {
                    Some(r) => r.projection,
                    None => {
                        parsed += 1;
                        changed.push(pf.virtual_path.clone());
                        crate::program::session::classify::decl_projection(
                            primary,
                            pf.provenance.tier,
                            &pf.virtual_path,
                            &pf.file,
                        )
                    }
                };
                (pf.virtual_path.clone(), p)
            })
            .collect();
        let class = match retained {
            None => ChangeClass::Package,
            Some(r) => {
                for path in r.files.keys() {
                    if !ctx.ws_file_set.contains(path) {
                        changed.push(path.clone());
                    }
                }
                let old: Vec<(&str, DeclProjection)> = r
                    .files
                    .iter()
                    .map(|(p, f)| (p.as_str(), f.projection))
                    .collect();
                let new: Vec<(&str, DeclProjection)> =
                    projections.iter().map(|(p, d)| (p.as_str(), *d)).collect();
                classify_files(&old, &new)
            }
        };
        changed.sort();

        // L2 + L2b: the workspace resolution, reusing every unchanged file's
        // retained resolution under B.
        let reuse_files: HashMap<String, Arc<FileResolution>> = match (class, retained) {
            (ChangeClass::BodyOnly, Some(r)) => ws_files
                .iter()
                .filter_map(|pf| {
                    let rf = r.files.get(&pf.virtual_path)?;
                    Arc::ptr_eq(&rf.tree, &pf.file)
                        .then(|| (pf.virtual_path.clone(), Arc::clone(&rf.resolution)))
                })
                .collect(),
            _ => HashMap::new(),
        };
        let mut copied = crate::engine::census::Tally::default();
        for r in reuse_files.values() {
            tally_resolution(&mut copied, r);
        }
        let (report, kept) = {
            let _s = pt::span("preflight", "preflight.resolve_full");
            f::resolve_full_program_reusing(
                &ctx,
                Some(&reuse_files),
                true,
                crate::program::resolve::abi_check::AbiIntegrity::Skip,
            )
        };
        let kept = kept.expect("asked to keep the resolutions");
        let resolved_now = ws_files.len() - reuse_files.len() + kept.rematerialize_failures;
        let coverage = f::reduce_fresh_coverage(&ctx, &report, &load.unreadable, &inputs.workspace);

        // The dependency bodies: reused across B, resolved whole otherwise.
        let reuse_dependency = class == ChangeClass::BodyOnly;
        let mut dependency_edges_copied = false;
        let (dependency, dependency_arc, dependency_resolved) = match (reuse_dependency, retained) {
            (true, Some(r)) => {
                // The edges and site facts are shared by `Arc`; only the
                // parens-less calls are copied, and the edges only when a
                // witness into a moved workspace declaration must change.
                let mut d = (*r.dependency).clone();
                tally_parenless(&mut copied, &d);
                let surface = ctx.decl_surface();
                let current = f::witnesses_current(&d.edges, &surface);
                if current == Some(false) {
                    dependency_edges_copied = true;
                    let edges = Arc::make_mut(&mut d.edges);
                    tally_edges(&mut copied, edges);
                    let ok = f::rematerialize_witnesses(edges, &surface);
                    debug_assert!(ok, "checked by witnesses_current");
                }
                if current.is_some() {
                    let arc = Arc::new(d.clone());
                    (d, arc, false)
                } else {
                    // §6.5: a cross-file reference with no current
                    // declaration is an invariant failure; release redoes
                    // the resolution against the current semantics.
                    debug_assert!(false, "a reused dependency witness has no declaration");
                    let d = {
                        let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
                        ctx.resolve_dependency_bodies()
                    };
                    let arc = Arc::new(d.clone());
                    (d, arc, true)
                }
            }
            _ => {
                let d = {
                    let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
                    ctx.resolve_dependency_bodies()
                };
                let arc = Arc::new(d.clone());
                (d, arc, true)
            }
        };

        // What the next revision reuses.
        let tier = Arc::clone(&ctx.dep_layer.dep_nodes);
        let proj: HashMap<String, DeclProjection> = projections.into_iter().collect();
        let mut files = HashMap::new();
        let trees: HashMap<&str, (&Arc<al_syntax::ir::AlFile>, &Arc<str>)> = ws_files
            .iter()
            .map(|pf| (pf.virtual_path.as_str(), (&pf.file, &pf.text)))
            .collect();
        for (path, resolution) in kept.files {
            let (tree, text) = trees[path.as_str()];
            files.insert(
                path.clone(),
                RetainedFile {
                    text: Arc::clone(text),
                    tree: Arc::clone(tree),
                    projection: proj[&path],
                    resolution,
                },
            );
        }
        drop(trees);
        drop(ws_files);
        let program = ResolvedProgram::from_parts(
            inputs,
            ProgramScope::CrossApp,
            ctx,
            report,
            coverage,
            Some(dependency),
        );
        self.retained = Some(Retained {
            _tier: tier,
            dep_key,
            identity,
            files,
            dependency: dependency_arc,
        });
        self.last = Some(RefreshReport {
            class,
            changed,
            parsed,
            resolved: resolved_now,
            dependency_bodies_resolved: dependency_resolved,
            dependency_edges_copied,
            copied_bytes: copied.bytes as u64,
            program_ms: start.elapsed().as_millis(),
            wall_ms: start.elapsed().as_millis(),
            rows: None,
        });
        Ok(program)
    }
}

/// The program as text, for the cold-oracle comparison (stage 4's equality
/// contract): both edge sets in order with their witnesses, the site facts
/// and parens-less calls sorted, the meta. Two builds of one revision write
/// the same text exactly when a consumer of the program can tell no
/// difference.
#[doc(hidden)]
pub fn write_program(p: &ResolvedProgram, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
    let b = p.body();
    for e in b.all_edges() {
        writeln!(w, "{e:?}")?;
    }
    writeln!(w, "-- dependency edges {}", b.dependency_body_edges().len())?;
    let mut facts: Vec<_> = b.site_facts.iter().collect();
    facts.sort_by(|a, b| a.0.cmp(b.0));
    for f in facts {
        writeln!(w, "{f:?}")?;
    }
    let mut parenless: Vec<_> = b
        .parenless_calls
        .iter()
        .map(|(k, v)| {
            let mut ids: Vec<usize> = v.iter().map(|e| e.index()).collect();
            ids.sort_unstable();
            (k.clone(), ids)
        })
        .collect();
    parenless.sort();
    writeln!(w, "{parenless:?}")?;
    let m = p.meta();
    writeln!(
        w,
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        m.recovered_files, m.coverage, m.histogram, m.primary_histogram, m.coverage_contract
    )
}

/// The cross-app model as text: every field a consumer reads (rows, calls,
/// bindings sorted by call site, diagnostics, external targets, the event
/// graph, the declared and loaded dependencies, the coverage, the friends).
#[doc(hidden)]
pub fn write_model(x: &CrossAppProgram, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
    let m = &x.resolved;
    let mut bindings: Vec<_> = m.calls.upgraded_bindings.iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(b.0));
    let mut friends: Vec<_> = x.friends.iter().collect();
    friends.sort();
    writeln!(w, "{:#?}", m.workspace)?;
    writeln!(w, "{:#?}", m.root_classifications)?;
    writeln!(
        w,
        "{:?}\n{:?}\n{:?}\n{:?}",
        m.primary_app, m.infra_diagnostics, m.workspace_diagnostics, m.dependency_diagnostics
    )?;
    writeln!(w, "{:#?}", m.calls.edges)?;
    writeln!(w, "{bindings:#?}")?;
    writeln!(
        w,
        "{:?}\n{:?}",
        m.calls.diagnostics, m.calls.external_targets
    )?;
    writeln!(w, "{:#?}", m.events)?;
    writeln!(
        w,
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        x.declared_dependencies, x.dependency_apps, x.coverage, friends, x.loaded_apps
    )
}

/// A `fmt::Write` that hashes instead of storing (a CDO model's text runs to
/// gigabytes).
struct HashSink(blake3::Hasher);

impl std::fmt::Write for HashSink {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.update(s.as_bytes());
        Ok(())
    }
}

/// blake3 of [`write_model`]'s text.
#[doc(hidden)]
#[must_use]
pub fn model_fingerprint(x: &CrossAppProgram) -> String {
    let mut h = HashSink(blake3::Hasher::new());
    write_model(x, &mut h).expect("hashing cannot fail");
    h.0.finalize().to_hex().to_string()
}

/// The P inputs (§6.1): the root `app.json` bytes and every dependency's
/// source content hash (a source-folder dependency has no package stamp in
/// `DepKey`). Any change rebuilds.
fn identity_of(snap: &AppSetSnapshot, inputs: &WorkspaceInputs) -> blake3::Hash {
    let mut h = blake3::Hasher::new();
    h.update(b"al-sem-session-identity-v1");
    match &inputs.app_json.bytes {
        Ok(b) => {
            h.update(&[1]);
            h.update(&(b.len() as u64).to_le_bytes());
            h.update(b);
        }
        Err(e) => {
            h.update(&[0]);
            h.update(e.to_string().as_bytes());
        }
    }
    for unit in snap.apps.iter().skip(1) {
        let hash = unit.source.as_ref().map_or("", |s| s.content_hash.as_str());
        h.update(&(hash.len() as u64).to_le_bytes());
        h.update(hash.as_bytes());
        h.update(&[u8::from(unit.app_path.is_some())]);
    }
    h.finalize()
}

/// The buffers a clone of `r` copies (shallow: each vector's elements).
fn tally_resolution(t: &mut crate::engine::census::Tally, r: &FileResolution) {
    use crate::program::resolve::edge::Route;
    use crate::program::resolve::full::{ClassifiedEdge, ObligationId, SiteFacts};
    t.buf::<ClassifiedEdge>(r.edges.len())
        .buf::<(ObligationId, SiteFacts)>(r.site_facts.len())
        .buf::<al_syntax::ir::ExprId>(r.parenless.len());
    for e in &r.edges {
        t.buf::<Route>(e.edge.routes.len());
    }
}

/// The buffers a copy of the dependency bodies' edges copies.
fn tally_edges(
    t: &mut crate::engine::census::Tally,
    edges: &[crate::program::resolve::full::ClassifiedEdge],
) {
    use crate::program::resolve::edge::Route;
    use crate::program::resolve::full::ClassifiedEdge;
    t.buf::<ClassifiedEdge>(edges.len());
    for e in edges {
        t.buf::<Route>(e.edge.routes.len());
    }
}

/// The buffers a clone of the dependency bodies' parens-less calls copies
/// (the edges and site facts are shared by `Arc`).
fn tally_parenless(t: &mut crate::engine::census::Tally, d: &DependencyBodyResolution) {
    for ids in d.parenless_calls.values() {
        t.buf::<al_syntax::ir::ExprId>(ids.len());
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
