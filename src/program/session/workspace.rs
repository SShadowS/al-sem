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
//!
//! **Memory modes** ([`MemoryMode`], context-server design §7). Both modes
//! keep each source-bearing dependency's extracted file list (the list the
//! snapshot shares, so the next snapshot reads the same text through the
//! session's `DepCache` at the same `.app` stamp and nothing is extracted
//! again until a package changes, which is P). The default keeps every
//! dependency tree in the tier too (fastest; the trees hold the same texts).
//! `LowMem` builds with `BuildProfile::ON_DEMAND`: the tier keeps no tree, so
//! the kept text is all it holds of the dependency source. A revision parses
//! dependency files only where a step
//! reads their trees: P parses them all (the tier is built from that parse);
//! S parses them all again (`ProgramContext::ensure_dep_bodies`) because every
//! dependency body is re-resolved; B parses none, except the files whose kept
//! rows or routine ids miss in the model assembly (a demand expansion, the
//! needs changing, the rows evicted), which parses exactly those. The trees
//! die with the revision's build context. Every answer is the same: a tree is
//! `al_syntax::parse` of the file's text, and the text is the one the
//! snapshot read from the acquired package image at the stamp the tier and the
//! row cache are keyed by (a changed package is P, which drops all of it).

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
use crate::program::session::dep_record::{self, DepRecord};
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
    /// What each dependency file's resolution read (the S rule,
    /// `session::dep_record`); `None` when no record could be kept.
    dep_record: Option<Arc<DepRecord>>,
    /// Each source-bearing dependency's extracted file list (texts), as the
    /// snapshot held it, in both modes. Held so the next snapshot shares it
    /// through the session's `DepCache` (same `.app` stamp, content hash
    /// included) instead of extracting the text again; under `LowMem` a
    /// file whose tree is needed is parsed from it. In the default mode the
    /// tier's trees hold the same texts, so only the lists are extra.
    _dependency_sources: Vec<Arc<Vec<crate::snapshot::embedded::SourceFile>>>,
}

/// What a workspace session keeps between revisions (context-server design
/// §7; see the module doc).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryMode {
    /// Every dependency tree kept in the tier: the fastest refresh.
    #[default]
    Default,
    /// No dependency tree kept between revisions (`--low-mem`): a revision
    /// parses the dependency files a step needs, from their kept text.
    LowMem,
}

impl MemoryMode {
    /// The build profile of a session in this mode.
    #[must_use]
    pub fn profile(self) -> BuildProfile {
        match self {
            MemoryMode::Default => BuildProfile::FULL,
            MemoryMode::LowMem => BuildProfile::ON_DEMAND,
        }
    }
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
    /// Whether any dependency file was resolved again.
    pub dependency_bodies_resolved: bool,
    /// Source-bearing dependency files, and how many of them were resolved
    /// in this revision (all on P or a whole fallback; under S those whose
    /// recorded answers changed, `session::dep_record`; none under B).
    pub dependency_files: usize,
    pub dependency_files_resolved: usize,
    /// Under S: whether the finer rule ran (`false`: everything resolved,
    /// for want of a trusted record).
    pub dependency_rule: bool,
    /// Dependency files the program build parsed (every one on a tier miss;
    /// under `LowMem` also every one for an S re-resolution). The model
    /// assembly's own re-parses are in `rows.dependency_parsed`.
    pub dependency_files_parsed: usize,
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
    mode: MemoryMode,
    dep_cache: DepCache,
    retained: Option<Retained>,
    /// The raw model rows of the last revision (§6.8 L3/L4); dropped with the
    /// retained state on a P change, so no kept row outlives its packages.
    rows: RowCache,
    last: Option<RefreshReport>,
    /// Tests only: a `Weak` to every dependency tree a refresh's program
    /// build held (`probe_dependency_trees`).
    #[cfg(test)]
    probe: Option<Vec<std::sync::Weak<al_syntax::ir::AlFile>>>,
}

impl Default for WorkspaceSession {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSession {
    /// A session in the default memory mode (every dependency tree kept).
    #[must_use]
    pub fn new() -> Self {
        Self::with_mode(MemoryMode::Default)
    }

    /// A session in `mode` (see [`MemoryMode`]).
    #[must_use]
    pub fn with_mode(mode: MemoryMode) -> Self {
        WorkspaceSession {
            mode,
            dep_cache: DepCache::default(),
            retained: None,
            rows: RowCache::default(),
            last: None,
            #[cfg(test)]
            probe: None,
        }
    }

    /// The session's memory mode.
    #[must_use]
    pub fn mode(&self) -> MemoryMode {
        self.mode
    }

    /// Tests: from now on, record a `Weak` to every dependency tree a refresh
    /// builds or parses (the program build's and the model assembly's).
    #[cfg(test)]
    pub(crate) fn probe_dependency_trees(&mut self) {
        self.probe = Some(Vec::new());
        self.rows.probe = Some(Vec::new());
    }

    /// Tests: how many probed dependency trees there were, and how many are
    /// still alive (held by anything, anywhere).
    #[cfg(test)]
    pub(crate) fn probed_dependency_trees(&self) -> (usize, usize) {
        let all = self.probe.iter().chain(self.rows.probe.iter()).flatten();
        let (mut seen, mut alive) = (0, 0);
        for w in all {
            seen += 1;
            alive += usize::from(w.strong_count() > 0);
        }
        (seen, alive)
    }

    /// What the last refresh did.
    #[must_use]
    pub fn last_report(&self) -> Option<&RefreshReport> {
        self.last.as_ref()
    }

    /// Drop the dependency files' kept rows (context-server design §7, the
    /// first eviction): the next refresh projects them again, so it is slower
    /// and its answer is the same. Returns how many files' rows were dropped.
    pub fn evict_dependency_rows(&mut self) -> usize {
        self.rows.evict_dependency()
    }

    /// The retained parts moved out one at a time, in the order a byte census
    /// drops them (`tools/census-probe resident`): each part's drop delta is
    /// the bytes no earlier-dropped part shared with it. Leaves the session
    /// empty (the next refresh is a P).
    #[doc(hidden)]
    pub fn take_census_parts(&mut self) -> Vec<(&'static str, Box<dyn std::any::Any>)> {
        let mut parts: Vec<(&'static str, Box<dyn std::any::Any>)> = Vec::new();
        let rows = std::mem::take(&mut self.rows);
        parts.push(("rows.abi", Box::new(rows.abi)));
        parts.extend(RowCache::split_dependency_for_census(rows.dependency));
        parts.push(("rows.dependency_ids", Box::new(rows.dependency_ids)));
        parts.push(("rows.workspace", Box::new(rows.workspace)));
        if let Some(r) = self.retained.take() {
            let mut resolutions = Vec::new();
            let mut trees = Vec::new();
            for (_, f) in r.files {
                resolutions.push(f.resolution);
                trees.push((f.tree, f.text));
            }
            parts.push(("retained.file_resolutions", Box::new(resolutions)));
            parts.push(("retained.file_trees", Box::new(trees)));
            parts.push(("retained.dependency_resolution", Box::new(r.dependency)));
            parts.push(("retained.dependency_record", Box::new(r.dep_record)));
            parts.push((
                "retained.dependency_sources",
                Box::new(r._dependency_sources),
            ));
            parts.push(("retained.tier", Box::new(r._tier)));
        }
        parts.push(("dep_cache", Box::new(std::mem::take(&mut self.dep_cache))));
        parts
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
        self.refresh_lending(inputs, skip_roots_config, needs, &mut |_| {})
    }

    /// [`Self::refresh`], lending the program to `on_program` between its
    /// build and the model's (agent-tasks design §1.2 M0-1: the sited graph
    /// is built there, the one place the program exists). Not called when
    /// the program failed.
    pub fn refresh_lending(
        &mut self,
        inputs: &Arc<WorkspaceInputs>,
        skip_roots_config: bool,
        needs: AssemblyNeeds,
        on_program: &mut dyn FnMut(&ResolvedProgram),
    ) -> Result<CrossAppProgram, CrossAppFailure> {
        let start = std::time::Instant::now();
        let _s = pt::span("revision", "revision.refresh");
        let program = self.refresh_program(inputs);
        if let Ok(p) = &program {
            on_program(p);
        }
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
        let profile = self.mode.profile();
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
            #[cfg(test)]
            let probe = self.rows.probe.take();
            self.rows = RowCache::default();
            #[cfg(test)]
            {
                self.rows.probe = probe;
            }
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

        // The dependency bodies: reused across B; under S each file is
        // resolved again only when an answer it recorded changed
        // (`session::dep_record`); resolved whole otherwise.
        let reuse_dependency = class == ChangeClass::BodyOnly;
        let mut dependency_edges_copied = false;
        let mut dependency_rule = false;
        let whole = |ctx: &f::ProgramContext| {
            // `LowMem` on a tier hit: every dependency file is parsed again.
            ctx.ensure_dep_bodies();
            let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
            let (d, rec) = dep_record::resolve_whole(ctx);
            let n = rec
                .as_ref()
                .map_or_else(|| ctx.dependency_files().len(), |r| r.files.len());
            (d, rec.map(Arc::new), n)
        };
        let (dependency, dependency_arc, dep_rec, dependency_resolved) =
            match (reuse_dependency, retained) {
                (false, Some(r)) if class == ChangeClass::Declaration && r.dep_record.is_some() => {
                    let rec = r.dep_record.as_deref().expect("checked");
                    match dep_record::resolve_by_rule(&ctx, &r.dependency, rec) {
                        Some((d, rec, outcome)) => {
                            dependency_rule = true;
                            pt::counter("deps.rule_queries", outcome.queries as u64);
                            pt::counter("deps.rule_changed", outcome.changed as u64);
                            pt::counter("deps.files_resolved", outcome.resolved as u64);
                            let arc = Arc::new(d.clone());
                            let rec = match rec {
                                dep_record::RecordAfter::Unchanged => r.dep_record.clone(),
                                dep_record::RecordAfter::New(rec) => rec.map(Arc::new),
                            };
                            (d, arc, rec, outcome.resolved)
                        }
                        None => {
                            let (d, rec, n) = whole(&ctx);
                            let arc = Arc::new(d.clone());
                            (d, arc, rec, n)
                        }
                    }
                }
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
                        // ENG-92: the facts point into the copy, so the old edges are freed.
                        d.site_facts = Arc::new(d.site_facts.with_edges(Arc::clone(&d.edges)));
                    }
                    if current.is_some() {
                        // The edges' ranges and the answers they were resolved
                        // against are unchanged: the record carries over.
                        let arc = Arc::new(d.clone());
                        (d, arc, r.dep_record.clone(), 0)
                    } else {
                        // §6.5: a cross-file reference with no current
                        // declaration is an invariant failure; release redoes
                        // the resolution against the current semantics.
                        debug_assert!(false, "a reused dependency witness has no declaration");
                        let (d, rec, n) = whole(&ctx);
                        let arc = Arc::new(d.clone());
                        (d, arc, rec, n)
                    }
                }
                _ => {
                    let (d, rec, n) = whole(&ctx);
                    let arc = Arc::new(d.clone());
                    (d, arc, rec, n)
                }
            };
        let dependency_files = dep_rec
            .as_ref()
            .map_or(dependency_resolved, |r| r.files.len());
        if let Some(r) = &dep_rec {
            pt::counter("deps.record_bytes", r.approx_bytes() as u64);
        }

        // What the next revision reuses.
        let tier = Arc::clone(&ctx.dep_layer.dep_nodes);
        let dependency_files_parsed = ctx.dep_files_parsed();
        // Both modes: the next snapshot shares this text through the
        // session's `DepCache` (same `.app` stamp, which carries the image's
        // content hash) instead of extracting it again. In the default mode
        // the tier's trees hold the same `Arc<str>` texts, so this costs only
        // the lists.
        let dependency_sources: Vec<_> = ctx
            .snap
            .apps
            .iter()
            .filter(|u| u.id != ctx.snap.workspace_app)
            .filter_map(|u| u.source.as_ref().map(|s| Arc::clone(&s.files)))
            .collect();
        #[cfg(test)]
        if let (Some(p), Some(bodies)) = (self.probe.as_mut(), ctx.dep_bodies()) {
            p.extend(
                bodies
                    .iter()
                    .flat_map(|u| u.files.iter())
                    .map(|pf| Arc::downgrade(&pf.file)),
            );
        }
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
            dep_record: dep_rec,
            _dependency_sources: dependency_sources,
        });
        self.last = Some(RefreshReport {
            class,
            changed,
            parsed,
            resolved: resolved_now,
            dependency_bodies_resolved: dependency_resolved > 0,
            dependency_files,
            dependency_files_resolved: dependency_resolved,
            dependency_rule,
            dependency_files_parsed,
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
    facts.sort_by(|a, b| a.0.cmp(&b.0));
    for f in facts {
        writeln!(w, "{f:?}")?;
    }
    // ENG-92: each edge's own lookup, in edge order. The dependency facts find their key
    // through an edge index, so a refresh that kept a stale index answers a miss here.
    let hits: String = b
        .all_edges()
        .map(|e| {
            if b.site_facts.get(&e.obligation_id()).is_some() {
                '1'
            } else {
                '0'
            }
        })
        .collect();
    writeln!(w, "-- site facts by edge {hits}")?;
    let mut parenless: Vec<_> = b
        .parenless_calls
        .iter()
        .map(|(k, v)| {
            let mut ids: Vec<usize> = v.parenless.iter().map(|e| e.index()).collect();
            ids.sort_unstable();
            // ENG-80: the bare-call verdicts are part of what a refresh must match.
            let mut bare: Vec<(usize, String)> = v
                .bare
                .iter()
                .map(|(e, b)| (e.index(), format!("{b:?}")))
                .collect();
            bare.sort_unstable();
            (k.clone(), ids, bare)
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

/// One part of the model's text: printed whole, or element by element (a long
/// list; TLS-80: [`model_fingerprint`] hashes its elements in parallel).
enum Part<'a> {
    Text(Box<WritePart<'a>>),
    Seq(&'static str, Box<dyn SeqItems + 'a>),
}

/// A part printed whole.
type WritePart<'a> = dyn Fn(&mut dyn std::fmt::Write) -> std::fmt::Result + Sync + 'a;

/// A list printed one `{:#?}` element at a time.
trait SeqItems: Sync {
    fn len(&self) -> usize;
    fn write_item(&self, i: usize, w: &mut dyn std::fmt::Write) -> std::fmt::Result;
}

impl<T: std::fmt::Debug + Sync> SeqItems for &[T] {
    fn len(&self) -> usize {
        <[T]>::len(self)
    }
    fn write_item(&self, i: usize, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        writeln!(w, "{:#?},", self[i])
    }
}

impl<T: std::fmt::Debug + Sync> SeqItems for Vec<T> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn write_item(&self, i: usize, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        writeln!(w, "{:#?},", self[i])
    }
}

/// The cross-app model in parts, in a fixed order: every field of the model,
/// its rows, calls and events. Each struct is destructured without `..`, so a
/// field added to any of them does not compile until it is placed here
/// (TLS-81: `ResolvedCalls`' `background_starts` and `loaded_only_tables`
/// were left out). Unordered maps and sets are sorted first.
/// [`write_model`] and [`model_fingerprint`] both read these, so they cover the
/// same fields.
fn model_parts(x: &CrossAppProgram) -> Vec<Part<'_>> {
    use crate::program::model::calls::ResolvedCalls;
    use crate::program::model::events::{EventGraph, ProgramEvents};
    use crate::program::model::workspace::{Model, ModelEntities};
    let CrossAppProgram {
        resolved,
        declared_dependencies,
        dependency_apps,
        coverage,
        friends,
        loaded_apps,
    } = x;
    let Model {
        workspace,
        root_classifications,
        primary_app,
        infra_diagnostics,
        workspace_diagnostics,
        dependency_diagnostics,
        calls,
        events,
        page_ui_blocked,
    } = resolved;
    let ModelEntities {
        objects,
        tables,
        routines,
    } = workspace;
    let ResolvedCalls {
        edges,
        upgraded_bindings,
        diagnostics,
        external_targets,
        background_starts,
        loaded_only_tables,
    } = &**calls;
    let ProgramEvents { graph, census } = &**events;
    let EventGraph {
        events: event_symbols,
        edges: event_edges,
    } = graph;
    let mut bindings: Vec<_> = upgraded_bindings.iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(b.0));
    let mut starts: Vec<_> = background_starts.iter().collect();
    starts.sort_by(|a, b| a.0.cmp(b.0));
    let mut loaded_only: Vec<_> = loaded_only_tables.iter().collect();
    loaded_only.sort();
    let mut friends: Vec<_> = friends.iter().collect();
    friends.sort();
    vec![
        Part::Seq("workspace.objects", Box::new(&objects[..])),
        Part::Seq("workspace.tables", Box::new(&tables[..])),
        Part::Seq("workspace.routines", Box::new(&routines[..])),
        Part::Text(Box::new(move |w| writeln!(w, "{root_classifications:#?}"))),
        Part::Text(Box::new(move |w| {
            writeln!(
                w,
                "{primary_app:?}\n{infra_diagnostics:?}\n{workspace_diagnostics:?}\n{dependency_diagnostics:?}"
            )
        })),
        Part::Seq("calls.edges", Box::new(&edges[..])),
        Part::Seq("calls.upgraded_bindings", Box::new(bindings)),
        Part::Text(Box::new(move |w| {
            writeln!(w, "{diagnostics:?}\n{external_targets:?}")
        })),
        Part::Seq("calls.background_starts", Box::new(starts)),
        Part::Text(Box::new(move |w| writeln!(w, "{loaded_only:?}"))),
        Part::Seq("events.graph.events", Box::new(&event_symbols[..])),
        Part::Seq("events.graph.edges", Box::new(&event_edges[..])),
        Part::Text(Box::new(move |w| writeln!(w, "{census:#?}"))),
        Part::Text(Box::new(move |w| writeln!(w, "{page_ui_blocked:?}"))),
        Part::Text(Box::new(move |w| {
            writeln!(
                w,
                "{declared_dependencies:?}\n{dependency_apps:?}\n{coverage:?}\n{friends:?}\n{loaded_apps:?}"
            )
        })),
    ]
}

/// The cross-app model as text ([`model_parts`], in order).
#[doc(hidden)]
pub fn write_model(x: &CrossAppProgram, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
    for part in model_parts(x) {
        match part {
            Part::Text(f) => f(w)?,
            Part::Seq(name, items) => {
                writeln!(w, "{name} [{}]", items.len())?;
                for i in 0..items.len() {
                    items.write_item(i, w)?;
                }
            }
        }
    }
    Ok(())
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

/// Elements per hashed chunk of a long part (2 in tests, so a small fixture's
/// lists span several chunks: review TLS-80, Fable).
const FINGERPRINT_CHUNK: usize = if cfg!(test) { 2 } else { 512 };

/// A fingerprint of [`write_model`]'s content: each of [`model_parts`] hashed
/// on its own, a long list in parallel chunks of [`FINGERPRINT_CHUNK`] elements
/// (each element's `{:#?}` text, as `write_model` prints it), and the part
/// hashes combined in their fixed order with each list's name and length. Two
/// fingerprints are equal only when every part's text is (TLS-80: hashing the
/// one text on one thread took ~100 s per CDO model, twice per refresh step).
#[doc(hidden)]
#[must_use]
pub fn model_fingerprint(x: &CrossAppProgram) -> String {
    use rayon::prelude::*;
    let text_hash = |f: &WritePart<'_>| {
        let mut h = HashSink(blake3::Hasher::new());
        f(&mut h).expect("hashing cannot fail");
        h.0.finalize()
    };
    let parts = model_parts(x);
    let hashes: Vec<blake3::Hash> = parts
        .par_iter()
        .map(|part| match part {
            Part::Text(f) => text_hash(f.as_ref()),
            Part::Seq(name, items) => {
                let n = items.len();
                let chunks: Vec<blake3::Hash> = (0..n.div_ceil(FINGERPRINT_CHUNK))
                    .into_par_iter()
                    .map(|c| {
                        let mut h = HashSink(blake3::Hasher::new());
                        for i in c * FINGERPRINT_CHUNK..((c + 1) * FINGERPRINT_CHUNK).min(n) {
                            items.write_item(i, &mut h).expect("hashing cannot fail");
                        }
                        h.0.finalize()
                    })
                    .collect();
                let mut h = blake3::Hasher::new();
                h.update(name.as_bytes());
                h.update(&(n as u64).to_le_bytes());
                for c in &chunks {
                    h.update(c.as_bytes());
                }
                h.finalize()
            }
        })
        .collect();
    let mut all = blake3::Hasher::new();
    all.update(b"al-sem-model-fingerprint-v2");
    for h in &hashes {
        all.update(h.as_bytes());
    }
    all.finalize().to_hex().to_string()
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
    // ENG-91: each site's rare facts are a box (an allocation) of their own.
    for _ in r.site_facts.iter().filter(|(_, f)| f.has_rare()) {
        t.buf::<crate::program::resolve::full::RareSiteFacts>(1);
    }
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
        t.buf::<al_syntax::ir::ExprId>(ids.parenless.len());
        t.buf::<(
            al_syntax::ir::ExprId,
            crate::program::resolve::full::BareVerdict,
        )>(ids.bare.len());
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
