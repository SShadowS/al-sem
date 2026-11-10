//! The finer dependency re-resolution rule for S (plan
//! `2026-11-03-context-server-remaining.md` item S; context-server design §6.6).
//!
//! Every dependency file is resolved through a [`Recorder`] (the reads facade,
//! `resolve::reads`), which logs each query its resolution made of the
//! whole-program state. The session keeps, per dependency file, its edge range
//! and its queries, and per query the digest of its answer ([`eval`]). Under S
//! it asks every kept query again against the new revision's state: a file
//! whose answers all have the same digest is resolved by the same steps to the
//! same edges (resolution is a deterministic function of the file's own parse,
//! which a dependency never changes outside P, and of those answers), so its
//! edges are kept; the others are resolved again, recorded again.
//!
//! **The answer digest** ([`eval`]) is blake3 over the declaration projection's
//! encoding (`session::classify::Classify`) of everything the query hands out:
//! ids by value; a node (`ObjectNode`, `RoutineNode`) or a run of them by the
//! whole node's content, so a read the resolution makes through the returned
//! node later is covered too; a `RoutineMeta` likewise. Two exceptions, both
//! exact: a node of the dependency tier's shared part is named by its position
//! in that part, and a frozen dependency `RoutineMeta` by its address, which
//! is sound because [`DepRecord::tier`] pins the identity of those immutable
//! parts (a record of another tier is never trusted). The content digest
//! leaves out what the declaration projection leaves out (positional origins,
//! `RoutineNode.source_unit`): resolution reads origins only to build source
//! witnesses, which the session re-materializes from the current surface for
//! every kept edge, exactly as it does under B (§6.5).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use rayon::prelude::*;

use crate::engine::perf_trace as pt;
use crate::program::graph::ProgramGraph;
use crate::program::node::AppRef;
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::full::{
    ClassifiedEdge, DependencyBodyResolution, FileResolution, ObligationId, ParenlessCalls,
    ProgramContext, SiteFacts,
};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolve::reads::{FACADE_VERSION, GraphReads, IndexReads, Query};
use crate::program::session::classify::{Classify, ProjectionHasher};
use crate::program::site_key::FileIx;
use crate::snapshot::ParsedFile;

/// An answer's digest.
pub(crate) type Digest = [u8; 32];

/// The state a query is answered against: one revision's graph, index and
/// surface.
pub(crate) struct EvalState<'a> {
    pub graph: &'a ProgramGraph,
    pub index: &'a ResolveIndex,
    pub surface: &'a DeclSurface,
}

impl EvalState<'_> {
    /// The identity of the immutable dependency parts the digests name by
    /// position or address (see the module doc), with the facade version.
    pub(crate) fn tier(&self) -> [usize; 4] {
        [
            FACADE_VERSION as usize,
            std::sync::Arc::as_ptr(self.graph.objects.shared()) as usize,
            std::sync::Arc::as_ptr(self.graph.routines.shared()) as usize,
            self.surface.frozen_identity(),
        ]
    }
}

/// The position of `x` in `part`, when it is an element of it.
fn position_in<T>(part: &[T], x: &T) -> Option<usize> {
    let size = std::mem::size_of::<T>();
    let base = part.as_ptr() as usize;
    let p = std::ptr::from_ref(x) as usize;
    (size > 0 && p >= base && p < base + std::mem::size_of_val(part)).then(|| (p - base) / size)
}

fn object(h: &mut ProjectionHasher, s: &EvalState, o: Option<&ObjectNode>) {
    match o {
        None => h.tag(0),
        Some(o) => match position_in(s.graph.objects.shared(), o) {
            Some(i) => {
                h.tag(1);
                h.u64(i as u64);
            }
            None => {
                h.tag(2);
                o.classify(h);
            }
        },
    }
}

fn routine(h: &mut ProjectionHasher, s: &EvalState, r: Option<&RoutineNode>) {
    match r {
        None => h.tag(0),
        Some(r) => match position_in(s.graph.routines.shared(), r) {
            Some(i) => {
                h.tag(1);
                h.u64(i as u64);
            }
            None => {
                h.tag(2);
                r.classify(h);
            }
        },
    }
}

fn run<'g>(
    h: &mut ProjectionHasher,
    s: &EvalState,
    it: impl ExactSizeIterator<Item = &'g RoutineNode>,
) {
    h.u64(it.len() as u64);
    for r in it {
        routine(h, s, Some(r));
    }
}

fn resolution(h: &mut ProjectionHasher, r: &ObjectRefResolution) {
    match r {
        ObjectRefResolution::Unique(id) => {
            h.tag(1);
            id.classify(h);
        }
        ObjectRefResolution::Ambiguous => h.tag(2),
        ObjectRefResolution::OutOfClosure => h.tag(3),
        ObjectRefResolution::Unresolved => h.tag(4),
    }
}

/// The digest of `q`'s answer against `s` (see the module doc): the answer
/// each facade method gives, by the same plain implementation.
pub(crate) fn eval(q: &Query, s: &EvalState) -> Digest {
    let g: &dyn GraphReads = s.graph;
    let ix: &dyn IndexReads = s.index;
    let mut h = ProjectionHasher::with_domain(b"al-sem-dep-query-answer");
    match q {
        Query::Object(id) => object(&mut h, s, g.object(id)),
        Query::ObjectFirst(id) => object(&mut h, s, g.object_first(id)),
        Query::Routine(rid) => routine(&mut h, s, g.routine(rid)),
        Query::RoutinesWithId(rid) => run(&mut h, s, g.routines_with_id(rid)),
        Query::RoutinesOf(id) => run(&mut h, s, g.routines_of(id)),
        Query::RoutinesNamed(id, name) => run(&mut h, s, g.routines_named(id, name)),
        Query::DeclarationsOf(rid) => g.declarations_of(rid).to_vec().classify(&mut h),
        Query::ResolveObject(app, kind, name) => {
            object(&mut h, s, g.resolve_object(*app, *kind, name));
        }
        Query::ResolveObjectByNumber(app, kind, n) => {
            object(&mut h, s, g.resolve_object_by_number(*app, *kind, *n));
        }
        Query::Closure(app) => {
            let c: Vec<AppRef> = g.closure(*app).into_iter().collect();
            c.classify(&mut h);
        }
        Query::IsFriend(a, b) => g.is_friend(*a, *b).classify(&mut h),
        Query::WorkspaceFriendless => g.workspace_friendless().classify(&mut h),
        Query::ClosedInternalInterface(id) => g.is_closed_internal_interface(id).classify(&mut h),
        Query::LookupPages(id) => g.lookup_pages(id).map(<[String]>::to_vec).classify(&mut h),
        Query::ObjectByNumber(app, kind, n) => {
            ix.object_by_number(g, *app, *kind, *n).classify(&mut h);
        }
        Query::ResolveObjectRef(from, kind, r) => {
            resolution(
                &mut h,
                &ix.resolve_object_ref(g, from.clone(), *kind, &r.to_ref()),
            );
        }
        Query::ResolveObjectRefStrict(from, kind, r) => {
            resolution(
                &mut h,
                &ix.resolve_object_ref_strict(g, from.clone(), *kind, &r.to_ref()),
            );
        }
        Query::NameIsUnique(kind, name) => ix.name_is_unique(*kind, name).classify(&mut h),
        Query::UniqueObject(kind, name) => ix.unique_object(*kind, name).cloned().classify(&mut h),
        Query::TableExtensionsOf(name) => ix.table_extensions_of(name).classify(&mut h),
        Query::PageExtensionsOf(name) => ix.page_extensions_of(name).classify(&mut h),
        Query::ReportExtensionsOf(name) => ix.report_extensions_of(name).classify(&mut h),
        Query::ImplementersOf(name) => ix.implementers_of(name).classify(&mut h),
        Query::UnmappedEnumImplementers(name) => {
            ix.unmapped_enum_implementers(g, name).classify(&mut h);
        }
        // The `from_object` accessors read only its app (`ResolveIndex`'s
        // `*_from` forms, to which the object forms delegate).
        Query::FieldWithOwnerInTable(app, base, field) => s
            .index
            .field_with_owner_in_table_from(s.graph, *app, base, field)
            .classify(&mut h),
        Query::TableScopeHasRoutine(app, base, name) => s
            .index
            .table_scope_has_routine_from(s.graph, *app, base, name)
            .classify(&mut h),
        Query::TableScopeMayHaveField(app, base, field) => s
            .index
            .table_scope_may_have_field_from(s.graph, *app, base, field)
            .classify(&mut h),
        Query::UntargetedTableExtensionsIn(app) => {
            let ids: Vec<_> = s
                .index
                .untargeted_table_extensions_from(s.graph, *app)
                .cloned()
                .collect();
            ids.classify(&mut h);
        }
        Query::ObjectExtends(from, target) => ix.object_extends(g, from, target).classify(&mut h),
        Query::SurfaceGet(rid) => match s.surface.get_local(rid) {
            Some(m) => {
                h.tag(2);
                m.classify(&mut h);
            }
            None => match crate::program::resolve::reads::SurfaceReads::get(s.surface, rid) {
                // The frozen tier's entry: immutable, pinned by `tier`.
                Some(m) => {
                    h.tag(1);
                    h.u64(std::ptr::from_ref(m) as usize as u64);
                }
                None => h.tag(0),
            },
        },
    }
    h.finish_bytes()
}

/// One dependency file of the record.
pub(crate) struct DepFile {
    pub app: AppRef,
    pub path: String,
    /// Its edges in the flat dependency edge list.
    pub edges: Range<usize>,
    /// The `FileIx` its edges' sites carry.
    pub file_ix: Option<FileIx>,
    /// Indices into [`DepRecord::queries`].
    pub queries: Box<[u32]>,
}

/// What the session keeps of the last dependency-body resolution (see the
/// module doc).
pub(crate) struct DepRecord {
    /// [`EvalState::tier`] of the state the digests were taken against.
    pub tier: [usize; 4],
    /// In resolution order.
    pub files: Vec<DepFile>,
    pub queries: Vec<Query>,
    /// Per query, its answer's digest against the record's state.
    pub digests: Vec<Digest>,
}

/// Interns queries into one table, keeping the digests known for them.
#[derive(Default)]
pub(crate) struct Interner {
    ids: HashMap<Query, u32>,
    queries: Vec<Query>,
    digests: Vec<Option<Digest>>,
}

impl Interner {
    pub(crate) fn intern(&mut self, q: Query, known: Option<Digest>) -> u32 {
        if let Some(&i) = self.ids.get(&q) {
            return i;
        }
        let i = u32::try_from(self.queries.len()).expect("fewer than 2^32 queries");
        self.ids.insert(q.clone(), i);
        self.queries.push(q);
        self.digests.push(known);
        i
    }

    /// The table, every digest not known yet taken against `s`.
    pub(crate) fn finish(self, s: &EvalState) -> (Vec<Query>, Vec<Digest>) {
        let digests = self
            .queries
            .par_iter()
            .zip(self.digests.into_par_iter())
            .map(|(q, d)| d.unwrap_or_else(|| eval(q, s)))
            .collect();
        (self.queries, digests)
    }
}

impl DepRecord {
    /// The record's memory: its tables, shallow (each query's strings
    /// counted by length), for the census and the CHANGELOG's number.
    pub(crate) fn approx_bytes(&self) -> usize {
        let files: usize = self
            .files
            .iter()
            .map(|f| std::mem::size_of::<DepFile>() + f.path.len() + f.queries.len() * 4)
            .sum();
        let queries: usize = self
            .queries
            .iter()
            .map(|q| std::mem::size_of::<Query>() + query_heap(q))
            .sum();
        files + queries + self.digests.len() * 32
    }
}

fn query_heap(q: &Query) -> usize {
    match q {
        Query::RoutinesNamed(_, s)
        | Query::ResolveObject(_, _, s)
        | Query::NameIsUnique(_, s)
        | Query::UniqueObject(_, s)
        | Query::TableExtensionsOf(s)
        | Query::PageExtensionsOf(s)
        | Query::ReportExtensionsOf(s)
        | Query::ImplementersOf(s)
        | Query::UnmappedEnumImplementers(s)
        | Query::FieldWithOwnerInTable(_, _, s)
        | Query::TableScopeHasRoutine(_, _, s)
        | Query::TableScopeMayHaveField(_, _, s) => s.len(),
        Query::ResolveObjectRef(
            _,
            _,
            crate::program::resolve::reads::RefKey::Name { raw, normalized_lc },
        )
        | Query::ResolveObjectRefStrict(
            _,
            _,
            crate::program::resolve::reads::RefKey::Name { raw, normalized_lc },
        ) => raw.len() + normalized_lc.len(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Resolving the dependency bodies, whole or by the rule
// ---------------------------------------------------------------------------

/// A [`DependencyBodyResolution`] built file by file in resolution order, with
/// its record.
struct Assembly {
    edges: Vec<ClassifiedEdge>,
    site_facts: HashMap<ObligationId, SiteFacts>,
    parenless: ParenlessCalls,
    files: Vec<DepFile>,
    interner: Interner,
    /// Two files' site facts share an obligation id: a kept file's facts can
    /// then not be told from the other file's, so no record is kept.
    collision: bool,
}

impl Assembly {
    fn new(edges: usize) -> Self {
        Assembly {
            edges: Vec::with_capacity(edges),
            site_facts: HashMap::new(),
            parenless: ParenlessCalls::new(),
            files: Vec::new(),
            interner: Interner::default(),
            collision: false,
        }
    }

    fn facts(&mut self, facts: impl IntoIterator<Item = (ObligationId, SiteFacts)>) {
        let mut own: HashSet<ObligationId> = HashSet::new();
        for (k, v) in facts {
            let first_here = own.insert(k.clone());
            if self.site_facts.insert(k, v).is_some() && first_here {
                self.collision = true;
            }
        }
    }

    /// A file resolved in this revision.
    fn fresh(
        &mut self,
        app: AppRef,
        path: &str,
        file_ix: Option<FileIx>,
        r: FileResolution,
        log: HashSet<Query>,
    ) {
        let start = self.edges.len();
        self.edges.extend(r.edges.into_iter().map(|mut e| {
            al_syntax::ir::exact_size(&mut e.edge.routes);
            e
        }));
        self.facts(r.site_facts);
        if !r.parenless.is_empty() {
            self.parenless
                .insert((app, path.to_string()), r.parenless.into_iter().collect());
        }
        let queries = log
            .into_iter()
            .map(|q| self.interner.intern(q, None))
            .collect();
        self.files.push(DepFile {
            app,
            path: path.to_string(),
            edges: start..self.edges.len(),
            file_ix,
            queries,
        });
    }

    /// A file whose recorded answers all held: its edges, site facts and
    /// parens-less calls taken from `old`, its sites re-stamped with the
    /// file's current `FileIx`.
    fn kept(
        &mut self,
        old: &DependencyBodyResolution,
        rec: &DepRecord,
        f: &DepFile,
        digests: &[Digest],
        file_ix: Option<FileIx>,
    ) {
        let start = self.edges.len();
        self.edges
            .extend(old.edges[f.edges.clone()].iter().cloned().map(|mut e| {
                if file_ix != f.file_ix {
                    e.site = match (e.site, file_ix) {
                        (Some(mut s), Some(ix)) => {
                            s.at.file = ix;
                            Some(s)
                        }
                        _ => None,
                    };
                }
                e
            }));
        let facts: Vec<(ObligationId, SiteFacts)> = old.edges[f.edges.clone()]
            .iter()
            .filter_map(|e| {
                old.site_facts
                    .get(&e.obligation_id)
                    .map(|v| (e.obligation_id.clone(), v.clone()))
            })
            .collect();
        self.facts(facts);
        let key = (f.app, f.path.clone());
        if let Some(p) = old.parenless_calls.get(&key) {
            self.parenless.insert(key, p.clone());
        }
        let queries = f
            .queries
            .iter()
            .map(|&q| {
                let q = q as usize;
                self.interner
                    .intern(rec.queries[q].clone(), Some(digests[q]))
            })
            .collect();
        self.files.push(DepFile {
            app: f.app,
            path: f.path.clone(),
            edges: start..self.edges.len(),
            file_ix,
            queries,
        });
    }

    fn finish(self, s: &EvalState) -> (DependencyBodyResolution, Option<DepRecord>) {
        let resolution = DependencyBodyResolution {
            edges: Arc::new(self.edges),
            site_facts: Arc::new(self.site_facts),
            parenless_calls: self.parenless,
        };
        if self.collision {
            return (resolution, None);
        }
        let (queries, digests) = self.interner.finish(s);
        let record = DepRecord {
            tier: s.tier(),
            files: self.files,
            queries,
            digests,
        };
        (resolution, Some(record))
    }
}

/// Resolve `files[i]` for every `i` in `which`, each through its recorder.
fn resolve_recorded(
    ctx: &ProgramContext,
    files: &[(AppRef, &ParsedFile)],
    which: &[usize],
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> Vec<(FileResolution, HashSet<Query>)> {
    let maps = ctx.dependency_object_maps(files);
    crate::big_stack::big_stack_pool().install(|| {
        which
            .par_iter()
            .map(|&i| {
                let (app, pf) = files[i];
                let (r, log, _) =
                    ctx.resolve_dependency_file_recorded(app, pf, index, surface, &maps[&app]);
                (r, log)
            })
            .collect()
    })
}

/// Every dependency body resolved, each file through its recorder: equal to
/// [`ProgramContext::resolve_dependency_bodies`], with the record (`None`
/// when two files' site facts share an obligation id). Needs the dependency
/// trees (`ensure_dep_bodies` first under `OnDemand`).
pub(crate) fn resolve_whole(ctx: &ProgramContext) -> (DependencyBodyResolution, Option<DepRecord>) {
    let files = ctx.dependency_files();
    let index = ResolveIndex::build(ctx.graph());
    let surface = ctx.decl_surface();
    let all: Vec<usize> = (0..files.len()).collect();
    let results = resolve_recorded(ctx, &files, &all, &index, &surface);
    let total = results.iter().map(|(r, _)| r.edges.len()).sum();
    let mut a = Assembly::new(total);
    for ((app, pf), (r, log)) in files.iter().zip(results) {
        a.fresh(
            *app,
            &pf.virtual_path,
            ctx.file_ix(*app, &pf.virtual_path),
            r,
            log,
        );
    }
    let state = EvalState {
        graph: ctx.graph(),
        index: &index,
        surface: &surface,
    };
    let _s = pt::span("crossapp", "crossapp.dep_record_digests");
    a.finish(&state)
}

/// What [`resolve_by_rule`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RuleOutcome {
    /// Distinct queries asked again.
    pub queries: usize,
    /// Queries whose answer changed.
    pub changed: usize,
    /// Dependency files resolved again.
    pub resolved: usize,
}

/// The S rule: every recorded query asked again against `ctx`'s state; the
/// files with a changed answer resolved again (recorded), the others' edges
/// kept from `old`. `None` when the record cannot be trusted (another tier
/// or facade version, a changed file list, a kept witness with no
/// declaration): the caller resolves everything.
pub(crate) fn resolve_by_rule(
    ctx: &ProgramContext,
    old: &DependencyBodyResolution,
    rec: &DepRecord,
) -> Option<(DependencyBodyResolution, RecordAfter, RuleOutcome)> {
    let index = ResolveIndex::build(ctx.graph());
    let surface = ctx.decl_surface();
    let state = EvalState {
        graph: ctx.graph(),
        index: &index,
        surface: &surface,
    };
    if state.tier() != rec.tier {
        return None;
    }
    let digests: Vec<Digest> = {
        let _s = pt::span("crossapp", "crossapp.dep_record_check");
        rec.queries.par_iter().map(|q| eval(q, &state)).collect()
    };
    let changed: Vec<bool> = digests
        .iter()
        .zip(&rec.digests)
        .map(|(a, b)| a != b)
        .collect();
    let file_ixs: Vec<Option<FileIx>> = rec
        .files
        .iter()
        .map(|f| ctx.file_ix(f.app, &f.path))
        .collect();
    let dirty: Vec<usize> = rec
        .files
        .iter()
        .zip(&file_ixs)
        .enumerate()
        .filter(|(_, (f, ix))| {
            f.queries.iter().any(|&q| changed[q as usize]) || (f.file_ix.is_none() && ix.is_some())
        })
        .map(|(i, _)| i)
        .collect();
    let outcome = RuleOutcome {
        queries: rec.queries.len(),
        changed: changed.iter().filter(|c| **c).count(),
        resolved: dirty.len(),
    };
    // Nothing to resolve and no file renumbered: the old resolution is this
    // revision's, shared as under B (its source witnesses re-materialized
    // when a callee moved), and the record stands as it is.
    if dirty.is_empty()
        && rec
            .files
            .iter()
            .zip(&file_ixs)
            .all(|(f, ix)| f.file_ix == *ix)
    {
        let mut d = old.clone();
        match crate::program::resolve::full::witnesses_current(&d.edges, &surface) {
            None => return None,
            Some(false) => {
                let ok = crate::program::resolve::full::rematerialize_witnesses(
                    Arc::make_mut(&mut d.edges).as_mut_slice(),
                    &surface,
                );
                debug_assert!(ok, "checked by witnesses_current");
            }
            Some(true) => {}
        }
        return Some((d, RecordAfter::Unchanged, outcome));
    }
    let mut fresh: HashMap<usize, (FileResolution, HashSet<Query>)> = HashMap::new();
    if !dirty.is_empty() {
        ctx.ensure_dep_bodies();
        let files = ctx.dependency_files();
        let same_files = files.len() == rec.files.len()
            && files
                .iter()
                .zip(&rec.files)
                .all(|((app, pf), f)| *app == f.app && pf.virtual_path == f.path);
        if !same_files {
            return None;
        }
        let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
        let results = resolve_recorded(ctx, &files, &dirty, &index, &surface);
        fresh = dirty.iter().copied().zip(results).collect();
    }
    let mut a = Assembly::new(old.edges.len());
    for (i, f) in rec.files.iter().enumerate() {
        match fresh.remove(&i) {
            Some((r, log)) => a.fresh(f.app, &f.path, file_ixs[i], r, log),
            None => a.kept(old, rec, f, &digests, file_ixs[i]),
        }
    }
    // §6.5: a kept edge's source witness names its callee's current span.
    match crate::program::resolve::full::witnesses_current(&a.edges, &surface) {
        None => return None,
        Some(false) => {
            let ok = crate::program::resolve::full::rematerialize_witnesses(&mut a.edges, &surface);
            debug_assert!(ok, "checked by witnesses_current");
        }
        Some(true) => {}
    }
    let (resolution, record) = {
        let _s = pt::span("crossapp", "crossapp.dep_record_digests");
        a.finish(&state)
    };
    Some((resolution, RecordAfter::New(record), outcome))
}

/// The record after [`resolve_by_rule`].
pub(crate) enum RecordAfter {
    /// Every answer held and nothing moved: the kept record stands.
    Unchanged,
    /// Rebuilt (`None`: none can be kept, see [`resolve_whole`]).
    New(Option<DepRecord>),
}

#[cfg(test)]
#[path = "dep_record_tests.rs"]
mod tests;
