//! `LspSnapshot` (T3 Task 8): the immutable, batch-built, owned-derived-index
//! snapshot the migrated LSP server serves queries from — the arc's
//! structural centerpiece.
//!
//! [`LspSnapshot::build_full`] composes the engine primitives landed by
//! earlier T3 tasks (`SnapshotBuilder` → `parse_snapshot` → `build_dep_layer`/
//! `assemble_program_graph` [Task 5] → per-file `resolve_file_obligations`
//! [Task 6] → `def_surface_fingerprint` [Task 7] → `emit_event_flow_edges`)
//! into one self-contained, `Arc`-shareable value: every field is OWNED data
//! (never a borrow into another field), so the whole snapshot can be handed
//! to a query thread as `Arc<LspSnapshot>` without any lifetime entanglement.
//!
//! # Ownership law (spec §3 / H-10 lesson) — AMENDED (Tier-2 latency wave, Task 1)
//!
//! `ResolveIndex`/`DeclSurface`/the `ObjectNodeId → &ObjectNode` map all BORROW
//! `graph`/`parsed` and are built TRANSIENTLY inside [`LspSnapshot::build_full`]
//! — they never appear as fields on `LspSnapshot` itself (that would make the
//! struct self-referential).
//!
//! [`build_incoming`]/[`build_decl_by_id`] are the two DERIVED indexes stored
//! on the snapshot. The ORIGINAL H-10 law (T3 Task 9) required both to be
//! rebuilt WHOLESALE on every generation, with no exception — reacting to a
//! real staleness bug the law was written to rule out. **That law is now
//! amended, not repealed**, licensed by a permanent parity gate (see below):
//! [`LspSnapshot::build_full`] and every RUNG-2/RUNG-3 rebuild
//! (`Updater::apply_rung2`/`apply_rung3`) still rebuild `incoming`/
//! `decl_by_id` WHOLESALE, from scratch, via [`build_incoming`]/
//! [`build_decl_by_id`] — the wholesale path is untouched and remains the
//! ONLY path for any rebuild that can add/remove a workspace file or change
//! a routine's identity/signature. RUNG 1 (`apply_rung1_core`,
//! `src/lsp/updater.rs`) is the sole exception: because a rung-1 batch is,
//! by construction, a set of `DefSurface`-fingerprint-UNCHANGED body edits to
//! ALREADY-KNOWN files, its effect on both indexes is providably confined to
//! the touched file(s)' own contribution — so rung 1 instead PATCHES
//! `incoming`/`decl_by_id` (clone-then-mutate only the touched files'
//! entries, using a `RoutineNodeId` decl-multiplicity refcount to stay
//! duplicate-safe — see `Updater`'s own doc) rather than re-deriving either
//! index from the WHOLE workspace. `publisher_fanout` is Arc-forwarded
//! unchanged at rung 1 (it depends only on `event_edges`, which rung 1 never
//! touches). The permanent gate this amendment is licensed by:
//! `tests/lsp_incremental_parity.rs`'s index-equality assertions (a rung-1
//! patched `decl_by_id`/`incoming` must always equal a fresh
//! [`build_decl_by_id`]/[`build_incoming`] recomputed from the SAME
//! post-edit `decls_by_file`/`edges_by_file`) plus its cross-file-duplicate-
//! `RoutineNodeId` fixture — any future change that breaks that gate breaks
//! the license, not just a test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::AlFile;
use rayon::prelude::*;

use crate::lsp::def_surface::{DefSurface, def_surface_fingerprint};
use crate::lsp::encoding::{ColOut, LineIndex, LineTable};
use crate::program::dep_cache::DepCache;
use crate::program::dep_cache::DepLspTier;
use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::ObjectNode;
use crate::program::profile::BuildProfile;
use crate::program::resolve::decl_surface::{DeclSurface, DepMeta};
use crate::program::resolve::edge::{Edge, RouteTarget};
use crate::program::resolve::emit_event_flow_edges;
use crate::program::resolve::full::{
    ClassifiedEdge, ObligationId, ProgramContext, app_object_map, build_context_with,
};
use crate::program::resolve::index::ResolveIndex;
use crate::program::sig_fp::source_routine_node_id;
use crate::program::{DepLayer, ProgramGraph};
use crate::snapshot::DependencySource;
use crate::snapshot::{AppSetSnapshot, ParsedFile, ParsedUnit};

/// Reference to one edge: (virtual_path, index into `edges_by_file[path]`).
/// Index-based — never a borrow — so [`LspSnapshot`] stays self-contained and
/// `Arc`-shareable.
///
/// `file: Arc<str>` (Tier-2 latency wave, Task 1 / item F5): was `String`.
/// [`push_edge_targets`] used to allocate a FRESH `String` (`file.to_string()`)
/// per `EdgeRef` pushed — the single hottest allocation in [`build_incoming`]
/// (~9-13ms best-of-7 on CDO's 17,973 workspace edges, almost entirely this
/// alloc). An `Arc<str>` lets every `EdgeRef` for the SAME file share one
/// allocation (`Arc::clone`, a refcount bump — no allocation at all), built
/// once per file per `build_incoming`/patch call. Compare via `&*r.file ==
/// "some/path"` or `r.file.as_ref() == "..."` — `Arc<str>` has no direct
/// `PartialEq<str>`/`PartialEq<&str>` impl the way `String` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRef {
    pub file: Arc<str>,
    pub idx: u32,
}

/// Reserved `EdgeRef.file` key for [`LspSnapshot::event_edges`] — a
/// NUL-prefixed string no real AL `virtual_path` can ever collide with (a
/// `virtual_path` is built from real filesystem-derived path segments, none
/// of which can embed `\0`), so `EdgeRef` stays uniform (always plainly
/// `(file, idx)`) without needing a separate enum-tagged variant just for
/// event-flow edges.
pub const EVENT_EDGES_KEY: &str = "\u{0}events";

/// [`LspSnapshot::dep_lines`]'s map type.
pub(crate) use crate::program::dep_cache::DepLines;

/// One routine declaration's identity + LSP-facing spans, owned (never
/// borrowing the `AlFile` it was read from — `Origin` is plain data).
#[derive(Clone, Debug)]
pub struct DeclEntry {
    pub id: RoutineNodeId,
    /// Raw casing, for display (`RoutineNodeId::name_lc` is lowercased).
    pub name: String,
    /// Whole declaration span (`CallHierarchyItem.range`).
    pub origin: al_syntax::ir::Origin,
    /// Name-token span (`CallHierarchyItem.selectionRange`).
    pub name_origin: al_syntax::ir::Origin,
    pub virtual_path: String,
}

/// A borrowed, source-agnostic view of one routine declaration's LSP-facing
/// data — the common shape of a workspace [`DeclEntry`] and a dependency
/// [`RoutineMeta`] (`dep_meta` tier), so [`LspSnapshot::decl_and_line_table`] can
/// serve BOTH without materializing a second owned map for dependencies
/// (the old `dep_decl_by_id` duplicated ~103 MB of `dep_meta`'s data on a
/// CDO-scale workspace, plus an O(all-dep-decls) build pass at every rung-3).
#[derive(Clone, Copy, Debug)]
pub struct DeclView<'a> {
    pub id: &'a RoutineNodeId,
    /// Raw casing, for display (`RoutineNodeId::name_lc` is lowercased).
    pub name: &'a str,
    /// Whole declaration span (`CallHierarchyItem.range`).
    pub origin: &'a al_syntax::ir::Origin,
    /// Name-token span (`CallHierarchyItem.selectionRange`).
    pub name_origin: &'a al_syntax::ir::Origin,
    pub virtual_path: &'a str,
}

impl<'a> DeclView<'a> {
    #[must_use]
    pub fn from_entry(e: &'a DeclEntry) -> Self {
        DeclView {
            id: &e.id,
            name: &e.name,
            origin: &e.origin,
            name_origin: &e.name_origin,
            virtual_path: &e.virtual_path,
        }
    }
}

/// One parsed file's owned data: the `AlFile` IR, its source text, and its
/// definition-surface fingerprint (Task 7) — everything a query needs
/// without re-reading disk or re-parsing.
pub struct ParsedFileEntry {
    /// `Arc`-shared with the updater's working-state `ParsedFile.file` (perf
    /// safe-wins Task 2) — see that field's sharing soundness doc.
    pub file: Arc<AlFile>,
    /// Shares the workspace `SourceFile.text` allocation (perf safe-wins Task 1).
    pub text: Arc<str>,
    pub virtual_path: String,
    pub surface: DefSurface,
    /// Snapshot-scoped [`LineTable`] cache (Tier-2 latency wave follow-up,
    /// `docs/OUTSTANDING.md`'s "Snapshot-scoped LineTable cache" item) — see
    /// [`Self::line_table`]. Populated lazily on first access via
    /// `OnceLock::get_or_init`, never eagerly: most files in a workspace are
    /// never queried in a given session, so building every file's
    /// `LineTable` up front at snapshot-construction time would waste the
    /// exact O(workspace-bytes) cost this cache exists to avoid paying more
    /// than once per file.
    ///
    /// Sound to memoize here (rather than needing separate invalidation
    /// bookkeeping) because `ParsedFileEntry` is immutable once constructed
    /// and a NEW one (with a fresh, empty `OnceLock`) is built only when
    /// `text` actually changes: `LspSnapshot::from_context`, rung 2
    /// (`Updater::apply_rung2`, rebuilds every file), and rung 1's
    /// touched-file insert (`apply_rung1_core`) all construct a brand-new
    /// `ParsedFileEntry` exactly when a file's text is recomputed. Every
    /// file rung 1 did NOT touch is instead forwarded by `Arc::clone` (`let
    /// mut parsed_files = cur.parsed.clone();`) — the SAME `Arc
    /// <ParsedFileEntry>`, hence the SAME `OnceLock`, survives the swap, so
    /// an unrelated save doesn't even cost that file's cache — it stays
    /// warm across generations for as long as the file itself is untouched.
    /// `OnceLock` (rather than a `Mutex<Option<LineTable>>`) also means two
    /// threads racing to warm the SAME file's cache (the main request loop
    /// and the updater thread's post-swap diagnostics recompute run
    /// concurrently — see `server.rs`'s module doc) never contend on an
    /// unrelated file's slot, and a losing racer's redundant build is simply
    /// discarded, never a panic or a stale write.
    line_table: OnceLock<LineTable>,
}

impl ParsedFileEntry {
    /// Construct a fresh entry — ALWAYS with an empty `line_table` cache
    /// slot. The one constructor `LspSnapshot::from_context` and both of
    /// `Updater`'s rung-1/rung-2 rebuild paths (`src/lsp/updater.rs`) go
    /// through, so "a brand-new `ParsedFileEntry` always starts with a
    /// fresh cache" can never accidentally drift at one of the three
    /// call sites (the field itself is private specifically to force this).
    #[must_use]
    pub fn new(
        file: Arc<AlFile>,
        text: Arc<str>,
        virtual_path: String,
        surface: DefSurface,
    ) -> Self {
        ParsedFileEntry {
            file,
            text,
            virtual_path,
            surface,
            line_table: OnceLock::new(),
        }
    }

    /// The cached [`LineTable`] for this file's CURRENT text — built once
    /// (lazily, on first access) and reused by every subsequent handler call
    /// against the SAME snapshot generation. See [`Self::line_table`]
    /// field's doc for the invalidation argument (a new generation that
    /// changes this file's text always gets a brand-new `ParsedFileEntry`,
    /// hence a fresh empty cache; an untouched file's `Arc<ParsedFileEntry>`
    /// — and its warmed cache — is forwarded unchanged).
    #[must_use]
    pub fn line_table(&self) -> &LineTable {
        self.line_table
            .get_or_init(|| LineTable::new(Arc::clone(&self.text)))
    }
}

/// The immutable, batch-built LSP snapshot: a whole-program resolve pass
/// frozen into owned, `Arc`-shareable data. See the module doc for the
/// composition [`LspSnapshot::build_full`] runs and the ownership law that
/// keeps every field self-contained.
pub struct LspSnapshot {
    /// Monotonic build counter. `build_full` always produces generation `0`
    /// (a full batch build has no prior generation to count from) — a future
    /// incremental updater (Task 9) bumps this on each rung-1/rung-2 apply.
    /// Excluded from cross-build equivalence checks (see this module's tests).
    pub generation: u64,
    /// `Arc`-shared (T3 Task 9): rung 1 (body-only edit) and rung 2
    /// (workspace-layer rebuild reusing the cached dep layer) both need to
    /// hand an UNCHANGED-or-rebuilt graph to a fresh `LspSnapshot` value
    /// without deep-cloning `ProgramGraph`'s node arrays (`ObjectIndex`
    /// carries no `Clone` impl, and cloning tens of thousands of
    /// `ObjectNode`/`RoutineNode` entries on every rung-1 save would itself
    /// blow the <100ms budget) — mirrors `dep_layer`'s existing pattern.
    pub graph: Arc<ProgramGraph>,
    pub dep_layer: Arc<DepLayer>,
    /// Identity/roots for rebuilds. `Arc`-shared for the same reason as
    /// `graph` above: `AppSetSnapshot` carries the workspace's source TEXT
    /// (`AppUnit::source`), so a plain `.clone()` on every incremental swap
    /// would copy text neither rung 1 nor rung 2 ever touches. Dependency
    /// apps keep their `source` (tier, content hash) but NO files: the LSP
    /// keeps no dependency text once [`Self::dep_lines`] is built (engine-
    /// switch S10.1; see [`without_dependency_text`]).
    pub snap: Arc<AppSetSnapshot>,
    /// `virtual_path` → file+text+`DefSurface`, workspace files ONLY (mirrors
    /// `edges_by_file`'s workspace scoping — a dependency's own source is
    /// never queried by the LSP surface).
    pub parsed: HashMap<String, Arc<ParsedFileEntry>>,
    /// Workspace-scoped: holds ONLY Phase-1 (workspace-caller) `Call`/`Run`/
    /// `ImplicitTrigger` edge buckets, keyed by `virtual_path`.
    pub edges_by_file: HashMap<String, Arc<Vec<ClassifiedEdge>>>,
    /// Phase-2 `EventFlow` edges (whole-program: subscribed publishers in
    /// every app, not just the workspace) — kept in ONE flat bucket rather
    /// than per-file, addressed via the reserved [`EVENT_EDGES_KEY`].
    ///
    /// Holds only links with at least one route. This is NOT the full
    /// publisher list: a publisher nobody subscribes to has no entry here.
    /// The program report (`resolve_full_program`) keeps the route-less links.
    pub event_edges: Arc<Vec<ClassifiedEdge>>,
    /// DERIVED — see [`build_incoming`]'s doc. Rebuilt WHOLESALE at rung 2/3
    /// (and by [`LspSnapshot::build_full`]); PATCHED (touched-file-local) at
    /// rung 1 by `apply_rung1_core` — see this module's amended ownership-law
    /// doc above.
    pub incoming: HashMap<RoutineNodeId, Vec<EdgeRef>>,
    /// DERIVED, precomputed in the SAME O(E) pass [`build_incoming`] makes
    /// over `event_edges` (t3 whole-branch review, blocker fix): for every
    /// routine `P` that is the `from` (publisher) of at least one
    /// `event_edges` entry, the sum of that entry's `routes.len()` — the
    /// REAL resolved-subscriber count [`crate::lsp::lens::
    /// effective_incoming_count`] needs for its "as-publisher fan-out" term.
    /// Before this field existed, that function computed the identical value
    /// by scanning ALL of `event_edges` on EVERY call — O(E) per query,
    /// called once per declaration by `compute_all` on every diagnostics
    /// recompute (itself run on every snapshot swap, including a rung-1
    /// single-file body edit), making a full diagnostics pass O(decls ×
    /// event_edges) — quadratic in workspace size. Precomputing it here
    /// keeps `effective_incoming_count` O(1) per call, matching `incoming`'s
    /// own precomputed-index pattern; rebuilt wholesale alongside `incoming`
    /// at rung 2/3. `Arc`-WRAPPED (Tier-2 latency wave, Task 1): this field
    /// is derived ONLY from `event_edges`, which rung 1 NEVER changes (rung 1
    /// touches only workspace `Call`/`Run`/`ImplicitTrigger` edges) — so
    /// `apply_rung1_core` forwards it via `Arc::clone` instead of
    /// recomputing (it used to be recomputed anyway, wastefully, alongside
    /// `incoming` — see `apply_rung1_core`'s own doc for the fix).
    pub publisher_fanout: Arc<HashMap<RoutineNodeId, usize>>,
    /// Sorted by `origin.byte.start` within each file. `Arc`-wrapped per file
    /// (T3 Task 9) so an incremental rung-1/rung-2 rebuild can share every
    /// UNCHANGED file's decl list via a cheap `Arc::clone` instead of
    /// deep-cloning the whole `HashMap<String, Vec<DeclEntry>>` (every
    /// `DeclEntry`'s `String` fields would otherwise be re-heap-allocated on
    /// every save, across the WHOLE workspace, just to replace one file).
    pub decls_by_file: HashMap<String, Arc<Vec<DeclEntry>>>,
    /// DERIVED — like [`Self::incoming`], rebuilt WHOLESALE from
    /// `decls_by_file` (see [`build_decl_by_id`]) at rung 2/3; PATCHED
    /// (touched-file-local, duplicate-`RoutineNodeId`-safe via
    /// `Updater::decl_multiplicity`) at rung 1 — see this module's amended
    /// ownership-law doc above and `apply_rung1_core`'s doc.
    pub decl_by_id: HashMap<RoutineNodeId, DeclEntry>,
    /// A text-free [`LineIndex`] for every file contributing an entry to
    /// [`Self::dep_meta`], keyed `(app, virtual_path)` — a
    /// dependency's `virtual_path` is only unique WITHIN its own app (two
    /// different deps can each have their own "Codeunit1.al"), unlike
    /// `Self::parsed`'s workspace-only, plain-`String`-keyed map. It turns a
    /// dependency-source item's positions into editor columns (the role
    /// [`ParsedFileEntry::line_table`] plays for workspace files). The LSP keeps
    /// no dependency TEXT (engine-switch S10.1): nothing it serves displays it.
    /// Look both maps up together via [`Self::decl_and_line_table`].
    pub dep_lines: Arc<DepLines>,
    /// The frozen dependency tier of the owned `DeclSurface` (T3 Task 12):
    /// every non-primary routine's `RoutineMeta` projection (name, origins,
    /// `parse_incomplete`, param `ty`/`by_ref` — never the body), built with
    /// the dependency nodes (`DepNodes::dep_meta`, shared by every root on
    /// the same dependency tier) and forwarded
    /// by `Arc::clone` across rungs 1/2 (sound for the same reason
    /// `dep_lines` is: dependency source cannot change on those rungs — see
    /// its doc). Rung 1/2 rebuild a workspace-only `DeclSurface` via
    /// [`DeclSurface::with_frozen`], composing it with this tier rather than
    /// re-deriving it, so no rung ever needs a dependency parse tree (under
    /// the LSP's `LIGHT` profile those trees die during the parse, right
    /// after each file is summarized). ALSO doubles as the `RouteTarget::Routine(id)`
    /// counterpart of [`Self::decl_by_id`] for every NON-primary (dependency)
    /// app — the design doc's §5 promise that "a dep with embedded source
    /// gets REAL navigable spans (legacy never could)". `make_routine_route`
    /// (the resolver) only ever constructs `RouteTarget::Routine(id)` when
    /// the SAME `DeclSurface` this tier is built from just answered `Some`
    /// for `id` — so any `id` an edge carries as a `Routine` target is
    /// guaranteed to be found in EITHER `decl_by_id` (workspace) or here,
    /// never neither. Served (as a borrowed [`DeclView`]) via
    /// [`Self::decl_and_line_table`] rather than a dedicated owned map — the
    /// old `dep_decl_by_id` duplicated this exact data.
    pub dep_meta: Arc<DepMeta>,
    /// The workspace root every `virtual_path` in this snapshot is relative
    /// to, normalized via [`crate::protocol::normalize_path`] (T3 Task 11) —
    /// so a handler can turn an inbound `textDocument` URI into the SAME
    /// `virtual_path` key `decls_by_file`/`parsed` use, via `uri_to_path`
    /// (which ALSO normalizes) + `strip_prefix`, without either side's
    /// casing silently mismatching on Windows. `Arc`-wrapped like `snap`/
    /// `dep_layer`: identical across every rung (the workspace root a
    /// running server watches never changes mid-session).
    pub workspace_root: Arc<PathBuf>,
}

impl LspSnapshot {
    /// Full batch build — snapshot → dep layer → assemble → resolve per file
    /// → derive indexes. Returns `None` when the underlying snapshot/program
    /// context build fails (fail-closed, mirrors
    /// [`crate::program::resolve::full::resolve_full_program`]).
    #[must_use]
    pub fn build_full(workspace_root: &Path) -> Option<LspSnapshot> {
        Self::build_full_with(workspace_root, DependencySource::default())
    }

    /// [`Self::build_full`] with an explicit [`DependencySource`].
    #[must_use]
    pub fn build_full_with(
        workspace_root: &Path,
        dependency_source: DependencySource,
    ) -> Option<LspSnapshot> {
        Self::build_full_with_cache(workspace_root, dependency_source, &DepCache::default())
    }

    /// [`Self::build_full_with`], sharing the dependency tier through
    /// `dep_cache` with every other root that loads the same dependencies.
    #[must_use]
    pub fn build_full_with_cache(
        workspace_root: &Path,
        dependency_source: DependencySource,
        dep_cache: &DepCache,
    ) -> Option<LspSnapshot> {
        let ctx = build_context_with(
            workspace_root,
            dependency_source,
            BuildProfile::LIGHT,
            dep_cache,
        )?;
        Some(Self::from_context(ctx, workspace_root).0)
    }

    /// As [`Self::build_full`], but ALSO returns the ONE workspace
    /// [`ParsedUnit`] for T3 Task 9's incremental updater
    /// (`src/lsp/updater.rs`) to own as its mutable working state.
    ///
    /// `ctx.parsed` holds only the workspace unit: under the LSP's `LIGHT`
    /// profile each dependency tree is dropped right after it is summarized,
    /// during the parse (the dependency tier supplies `dep_meta` and
    /// `dep_lines`). So the updater's
    /// steady state never retains dependency parse arenas — see the design
    /// spec (`docs/superpowers/specs/2026-07-13-owned-decl-surface-design.md`).
    /// `ParsedFile.file`/`.text` are `Arc`-shared (perf safe-wins Task 2),
    /// so the published snapshot's `ParsedFileEntry`s hold `Arc::clone`s of
    /// the SAME workspace allocations this returns — sound because nothing
    /// mutates an `AlFile` after `al_syntax::parse` returns; every update
    /// REPLACES whole `ParsedFile`/`ParsedUnit` values (rung-1 `pending`
    /// splice / rung-2 `splice_file` / rung-3 wholesale — see updater.rs),
    /// so two owners of the same `Arc<AlFile>` can never observe a torn or
    /// stale-relative-to-each-other view.
    ///
    /// `pub` (T3 Task 10, widened from `pub(crate)`): the permanent
    /// incremental-vs-batch differential gate (`tests/lsp_incremental_parity.rs`)
    /// is an external integration-test crate — it needs this to construct an
    /// [`Updater`](crate::lsp::updater::Updater) exactly as `main.rs`/
    /// `server.rs` eventually will, so this is the arc's real future public
    /// server-construction surface, not test-only scaffolding.
    #[must_use]
    pub fn build_full_with_parsed(workspace_root: &Path) -> Option<(LspSnapshot, ParsedUnit)> {
        Self::build_full_with_parsed_with(workspace_root, DependencySource::default())
    }

    /// [`Self::build_full_with_parsed`] with an explicit [`DependencySource`]
    /// — what the server's first build and the updater's full rebuild use.
    #[must_use]
    pub fn build_full_with_parsed_with(
        workspace_root: &Path,
        dependency_source: DependencySource,
    ) -> Option<(LspSnapshot, ParsedUnit)> {
        Self::build_full_with_parsed_with_cache(
            workspace_root,
            dependency_source,
            &DepCache::default(),
        )
    }

    /// [`Self::build_full_with_parsed_with`], sharing the dependency tier
    /// through `dep_cache` — what the server's per-root builds use.
    #[must_use]
    pub fn build_full_with_parsed_with_cache(
        workspace_root: &Path,
        dependency_source: DependencySource,
        dep_cache: &DepCache,
    ) -> Option<(LspSnapshot, ParsedUnit)> {
        let ctx = build_context_with(
            workspace_root,
            dependency_source,
            BuildProfile::LIGHT,
            dep_cache,
        )?;
        Some(Self::from_context(ctx, workspace_root))
    }

    /// The composition shared by [`Self::build_full`]/
    /// [`Self::build_full_with_parsed`]: dep layer → assemble → resolve per
    /// file → derive indexes, given an already-built [`ProgramContext`].
    ///
    /// `pub(crate)` (T3 Task 11): `handlers.rs`'s own tests construct a
    /// two-app (workspace + embedded-source dependency) [`ProgramContext`]
    /// by hand — mirroring `program::build`'s in-memory layer-split fixture
    /// pattern — and call this directly, the same way `build_full`/
    /// `build_full_with_parsed` do, rather than re-implementing this
    /// composition a second time just to exercise it without disk I/O.
    ///
    /// Returns the `LspSnapshot` alongside the ONE workspace [`ParsedUnit`]
    /// (`ctx.parsed` holds nothing else): `dep_meta` and `dep_lines` come from
    /// the dependency tier.
    /// `ParsedFile.file`/`.text` are `Arc`-shared (perf
    /// safe-wins Task 2), so the published snapshot's workspace
    /// `ParsedFileEntry`s hold `Arc::clone`s rather than consuming the
    /// workspace unit by value; `build_full` just drops the returned
    /// workspace unit too (it never needed it).
    pub(crate) fn from_context(
        ctx: ProgramContext,
        workspace_root: &Path,
    ) -> (LspSnapshot, ParsedUnit) {
        let ProgramContext {
            snap,
            graph,
            mut parsed,
            primary_app_ref,
            ws_file_set,
            dep_layer,
            profile: _,
        } = ctx;

        // Locate the ONE primary (workspace) `ParsedUnit` — `snap.apps` is
        // GUID-deduped upstream, so at most one can match (mirrors
        // `build_context`'s own find-or-synthesize, but a workspace with zero
        // source files never reaches here anyway: `ws_file_set` would be
        // empty and every loop below is a no-op).
        let primary_unit_idx = parsed.iter().position(|u| u.app == snap.workspace_app);

        // ── Transient borrow phase: index/surface borrow `graph`/`parsed`,
        // and per the module's ownership law must never survive into
        // `LspSnapshot` — everything they produce is copied into owned data
        // (or, for `pf.file`/`pf.text`, `Arc::clone`d in the sharing phase
        // below — perf safe-wins Task 2 — rather than moved, since `parsed`
        // must survive intact for the caller).
        let mut edges_by_file: HashMap<String, Arc<Vec<ClassifiedEdge>>> = HashMap::new();
        let mut surfaces_by_file: HashMap<String, DefSurface> = HashMap::new();
        let mut decls_by_file: HashMap<String, Arc<Vec<DeclEntry>>> = HashMap::new();
        let event_edges: Arc<Vec<ClassifiedEdge>>;
        let dep_lines: Arc<DepLines>;
        let dep_meta: Arc<DepMeta>;

        {
            let obj_node_map = app_object_map(&graph, primary_app_ref);
            let index = ResolveIndex::build(&graph);
            // The rung-1 construction: workspace decls over the dependency
            // tier's frozen `dep_meta` (built with the dependency nodes, so
            // a shared-tier hit — which parsed only the workspace — has it
            // too). Roots sharing a dependency tier share its `dep_lines`;
            // the first root to get here publishes them, indexed from the
            // snapshot's source files (no parse needed).
            let ws = primary_unit_idx.map_or(&[][..], |i| std::slice::from_ref(&parsed[i]));
            let surface = DeclSurface::build(&graph, ws)
                .with_frozen(Arc::clone(&dep_layer.dep_nodes.dep_meta));
            let tier = dep_layer.dep_nodes.lsp.get_or_init(|| {
                Arc::new(DepLspTier {
                    dep_lines: Arc::new(build_dep_lines(&snap, &graph.apps, primary_app_ref)),
                })
            });
            dep_meta = Arc::clone(&dep_layer.dep_nodes.dep_meta);
            dep_lines = Arc::clone(&tier.dep_lines);
            crate::census_hook::mark("5.index+surface+dep_meta+dep_lines");

            if let Some(idx) = primary_unit_idx {
                // T3 Task 3 (F7): same ordered-collect-then-`par_iter` shape as
                // `resolve_full_program_from_parts`'s Phase-1 loop — `files`
                // preserves `parsed[idx].files`' original order, `recompute_file`
                // reads only immutable shared borrows, and the indexed
                // `par_iter`/`collect()` keeps `results` in that same order, so
                // the sequential `insert`s below are byte-identical to the old
                // serial loop. Big-stack pool for the same reason as
                // `snapshot::parse::parse_snapshot`: the resolver's
                // receiver/extraction walk recurses over the AL expression tree.
                let files: Vec<&ParsedFile> = parsed[idx]
                    .files
                    .iter()
                    .filter(|pf| ws_file_set.contains(&pf.virtual_path))
                    .collect();
                let results: Vec<(Vec<ClassifiedEdge>, DefSurface, Vec<DeclEntry>)> =
                    crate::big_stack::big_stack_pool().install(|| {
                        files
                            .par_iter()
                            .map(|pf| {
                                recompute_file(
                                    pf,
                                    primary_app_ref,
                                    &graph,
                                    &index,
                                    &surface,
                                    &obj_node_map,
                                )
                            })
                            .collect()
                    });
                for (pf, (edges, surface, decls)) in files.iter().zip(results) {
                    edges_by_file.insert(pf.virtual_path.clone(), Arc::new(edges));
                    surfaces_by_file.insert(pf.virtual_path.clone(), surface);
                    decls_by_file.insert(pf.virtual_path.clone(), Arc::new(decls));
                }
            }
            crate::census_hook::mark("6.resolve_workspace_files");

            let raw_event_edges = emit_event_flow_edges(&graph, &surface);
            // Links without routes have no LSP reader (no incoming ref, no
            // fan-out, no outgoing item); the program report keeps them
            // (spec §2, §6 2b).
            event_edges = Arc::new(
                raw_event_edges
                    .into_iter()
                    .filter(|edge| !edge.routes.is_empty())
                    .map(|edge| ClassifiedEdge {
                        obligation_id: ObligationId::Publisher(edge.from.clone()),
                        edge,
                    })
                    .collect(),
            );
            crate::census_hook::mark("7.event_edges");

            // `index`/`surface`/`obj_node_map` drop here, at the end of this
            // block — their borrows of `graph`/`parsed` end before the
            // sharing phase below needs to (immutably) re-borrow `parsed`.
        }

        let (incoming, publisher_fanout) = build_incoming(&edges_by_file, &event_edges);
        let decl_by_id = build_decl_by_id(&decls_by_file);
        crate::census_hook::mark("8.incoming+decl_by_id");

        // ── Sharing phase (perf safe-wins Task 2): `AlFile`/text are
        // `Arc`-shared, so the published snapshot CLONES the `Arc`s and
        // leaves `parsed`'s workspace entries intact for the extraction
        // below.
        let mut parsed_files: HashMap<String, Arc<ParsedFileEntry>> = HashMap::new();
        if let Some(idx) = primary_unit_idx {
            for pf in &parsed[idx].files {
                if !ws_file_set.contains(&pf.virtual_path) {
                    continue;
                }
                let surface = surfaces_by_file
                    .remove(&pf.virtual_path)
                    .expect("a surface was computed for every ws_file_set member above");
                parsed_files.insert(
                    pf.virtual_path.clone(),
                    Arc::new(ParsedFileEntry::new(
                        Arc::clone(&pf.file),
                        Arc::clone(&pf.text),
                        pf.virtual_path.clone(),
                        surface,
                    )),
                );
            }
        }

        let snapshot = LspSnapshot {
            generation: 0,
            graph: Arc::new(graph),
            dep_layer: Arc::new(dep_layer),
            snap: Arc::new(without_dependency_text(snap)),
            parsed: parsed_files,
            edges_by_file,
            event_edges,
            incoming,
            publisher_fanout: Arc::new(publisher_fanout),
            decls_by_file,
            decl_by_id,
            dep_lines,
            dep_meta,
            workspace_root: Arc::new(crate::protocol::normalize_path(workspace_root)),
        };

        // `parsed` holds only the workspace unit (dependency trees never
        // reach a `ProgramContext`), so this returns it.
        let ws_pos = parsed
            .iter()
            .position(|u| u.app == snapshot.snap.workspace_app);
        let workspace_unit = match ws_pos {
            Some(i) => parsed.swap_remove(i),
            None => ParsedUnit {
                app: snapshot.snap.workspace_app.clone(),
                files: vec![],
            },
        };
        crate::census_hook::mark("9.publish_snapshot");
        (snapshot, workspace_unit)
    }

    /// Position lookup: file + 0-based line + UTF-8 byte col → routine whose
    /// `name_origin` or whole-decl `origin` contains it (name hit preferred).
    ///
    /// `line`/`byte_col` share [`al_syntax::ir::Point`]'s own semantics
    /// (`column` is a UTF-8 byte column within the line) — no encoding
    /// conversion needed; compare directly against `Origin.start`/`.end`.
    #[must_use]
    pub fn decl_at(&self, virtual_path: &str, line: u32, byte_col: u32) -> Option<&DeclEntry> {
        let decls = self.decls_by_file.get(virtual_path)?;
        let pos = (line, byte_col);

        // Name hit, preferred: an exact click on the symbol's own name token.
        if let Some(d) = decls.iter().find(|d| point_in_origin(pos, &d.name_origin)) {
            return Some(d);
        }
        // Whole-decl (body) hit fallback.
        decls.iter().find(|d| point_in_origin(pos, &d.origin))
    }

    /// Look up one classified edge by its [`EdgeRef`].
    #[must_use]
    pub fn edge(&self, r: &EdgeRef) -> &ClassifiedEdge {
        if &*r.file == EVENT_EDGES_KEY {
            &self.event_edges[r.idx as usize]
        } else {
            &self.edges_by_file[r.file.as_ref()][r.idx as usize]
        }
    }

    /// Resolve ANY `RoutineNodeId` — workspace OR dependency — to its live
    /// decl data plus the column converter for its file. The one lookup
    /// handlers.rs uses for every position-bearing `RouteTarget::Routine(id)`
    /// surface, so a caller never needs to know whether `id` is served from
    /// [`Self::decl_by_id`] (workspace) or [`Self::dep_meta`] (dependency).
    /// A workspace decl's converter is its file's cached
    /// [`ParsedFileEntry::line_table`] (memoized — repeat callers against the
    /// SAME snapshot generation, e.g. `incoming`'s per-distinct-caller loop,
    /// reuse it); a dependency decl's is its file's [`LineIndex`] in
    /// [`Self::dep_lines`] (built once per shared tier). Returns `None` for a
    /// stale id (not in either map) — the fail-closed "never guess" contract
    /// every handler built on this must honor.
    #[must_use]
    pub fn decl_and_line_table(&self, id: &RoutineNodeId) -> Option<(DeclView<'_>, &dyn ColOut)> {
        if let Some(d) = self.decl_by_id.get(id) {
            let entry = self.parsed.get(&d.virtual_path)?;
            return Some((DeclView::from_entry(d), entry.line_table()));
        }
        let (key, m) = self.dep_meta.get_key_value(id)?;
        let index = self
            .dep_lines
            .get(&(id.object.app, m.virtual_path.clone()))?;
        Some((
            DeclView {
                id: key,
                name: &m.name,
                origin: &m.origin,
                name_origin: &m.name_origin,
                virtual_path: &m.virtual_path,
            },
            index,
        ))
    }
}

/// `true` when the half-open span `[origin.start, origin.end)` — compared as
/// `(row, column)` tuples, matching source-span containment (a later line
/// always sorts after an earlier one; same-line spans compare by column) —
/// contains `pos`.
fn point_in_origin(pos: (u32, u32), origin: &al_syntax::ir::Origin) -> bool {
    let start = (origin.start.row, origin.start.column);
    let end = (origin.end.row, origin.end.column);
    pos >= start && pos < end
}

/// O(E) wholesale rebuild — used by [`LspSnapshot::build_full`] and every
/// rung-2/3 rebuild. Rung 1 instead PATCHES `incoming` for the touched
/// file(s) only (`apply_rung1_core`, `src/lsp/updater.rs`) — see this
/// module's amended ownership-law doc for the licensing gate.
///
/// `Incoming(S)` gets: every `Call`/`Run`/`ImplicitTrigger` edge with a route
/// `RouteTarget::Routine(S)` (from `edges_by_file`), AND every `EventFlow`
/// edge from publisher `P` with a route targeting `S` (from `event_edges` —
/// event direction: `P` calls `S`). Both populations are scanned uniformly:
/// every route on every edge (matching `Edge::all_routes`'s RESOLUTION-context
/// semantics, not a reachability filter — an LSP "incoming calls" view is
/// meant to show every statically-possible caller, including one gated behind
/// `ManualBinding`/`AmbiguousDispatch`, not just the unconditionally-firing
/// subset `Edge::default_reachable_routes` would give).
///
/// Returns `(incoming, publisher_fanout)` — see [`LspSnapshot::publisher_fanout`]'s
/// doc for why the second map is precomputed HERE, in the SAME loop over
/// `event_edges` this function already runs, rather than via a separate pass
/// (t3 whole-branch review, blocker fix): `publisher_fanout[P]` is the sum of
/// `routes.len()` over every `event_edges` entry whose `edge.from == P` —
/// the REAL resolved-subscriber count, never mere edge presence (an
/// `emit_event_flow_edges` publisher entry always exists even with zero
/// subscribers (the LSP snapshot drops route-less ones), so counting entries
/// rather than summing routes would overcount an unsubscribed publisher as
/// "used").
///
/// Builds ONE `Arc<str>` per file (Tier-2 latency wave, Task 1 / F5) — every
/// `EdgeRef` for that file's edges `Arc::clone`s it, replacing the OLD
/// per-`EdgeRef` `file.to_string()` allocation (the single hottest cost in
/// this function).
#[must_use]
pub fn build_incoming(
    edges_by_file: &HashMap<String, Arc<Vec<ClassifiedEdge>>>,
    event_edges: &[ClassifiedEdge],
) -> (
    HashMap<RoutineNodeId, Vec<EdgeRef>>,
    HashMap<RoutineNodeId, usize>,
) {
    let mut incoming: HashMap<RoutineNodeId, Vec<EdgeRef>> = HashMap::new();

    for (file, edges) in edges_by_file {
        let file_arc: Arc<str> = Arc::from(file.as_str());
        for (idx, ce) in edges.iter().enumerate() {
            push_edge_targets(&mut incoming, &ce.edge, &file_arc, idx as u32);
        }
    }

    let mut publisher_fanout: HashMap<RoutineNodeId, usize> = HashMap::new();
    let event_key: Arc<str> = Arc::from(EVENT_EDGES_KEY);
    for (idx, ce) in event_edges.iter().enumerate() {
        push_edge_targets(&mut incoming, &ce.edge, &event_key, idx as u32);
        if !ce.edge.routes.is_empty() {
            *publisher_fanout.entry(ce.edge.from.clone()).or_insert(0) += ce.edge.routes.len();
        }
    }

    (incoming, publisher_fanout)
}

/// Push one [`EdgeRef`] per DISTINCT `RouteTarget::Routine` target `edge`
/// resolves to (T3 Task 9 review carry-over from Task 8: a single edge can
/// carry >1 route to the exact SAME target — e.g. a pathological
/// ambiguous-overload candidate set where two routes happen to name the
/// same routine — and without this per-edge dedup guard, `incoming[target]`
/// would carry the IDENTICAL `EdgeRef` more than once: pure noise for a
/// consumer, e.g. `incomingCalls`' `fromRanges` showing the same call site
/// twice for no reason). Routes from a DIFFERENT edge naming the same
/// target are NOT deduplicated — those are genuinely distinct callers (a
/// different `idx`), never touched by this guard.
///
/// `pub(crate)` (Tier-2 latency wave, Task 1): the rung-1 `incoming` patch
/// (`apply_rung1_core`, `src/lsp/updater.rs`) reuses this exact function to
/// add the touched file's NEW edges — the ONE place "which targets does this
/// edge push to" is defined, so the wholesale and patched paths can never
/// disagree about the per-edge dedup rule.
pub(crate) fn push_edge_targets(
    incoming: &mut HashMap<RoutineNodeId, Vec<EdgeRef>>,
    edge: &Edge,
    file: &Arc<str>,
    idx: u32,
) {
    let mut seen_this_edge: Vec<&RoutineNodeId> = Vec::new();
    for route in &edge.routes {
        if let RouteTarget::Routine(target) = &route.target {
            if seen_this_edge.contains(&target) {
                continue;
            }
            seen_this_edge.push(target);
            incoming.entry(target.clone()).or_default().push(EdgeRef {
                file: Arc::clone(file),
                idx,
            });
        }
    }
}

/// The DISTINCT `RouteTarget::Routine` targets `edge` pushes to — the same
/// per-edge dedup rule [`push_edge_targets`] applies, exposed standalone so
/// the rung-1 `incoming` patch (`apply_rung1_core`) can compute "which
/// targets did this OLD edge contribute to" when REMOVING a touched file's
/// stale entries, without needing a `file`/`idx` to construct a throwaway
/// [`EdgeRef`] just to discard it.
pub(crate) fn edge_targets(edge: &Edge) -> Vec<&RoutineNodeId> {
    let mut seen: Vec<&RoutineNodeId> = Vec::new();
    for route in &edge.routes {
        if let RouteTarget::Routine(target) = &route.target
            && !seen.contains(&target)
        {
            seen.push(target);
        }
    }
    seen
}

/// One workspace file's contribution to a snapshot: its resolved edge list,
/// definition-surface fingerprint, and (sorted) decl list. Shared by
/// [`LspSnapshot::from_context`]'s whole-batch build loop and the
/// incremental updater's rung-1 (one file) / rung-2 (every file) per-file
/// recompute (`src/lsp/updater.rs`) — the ONE place "what a file
/// contributes to a snapshot" is defined, so the batch and incremental paths
/// can never drift apart.
#[must_use]
pub(crate) fn recompute_file(
    pf: &ParsedFile,
    primary_app_ref: AppRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    obj_node_map: &HashMap<ObjectNodeId, &ObjectNode>,
) -> (Vec<ClassifiedEdge>, DefSurface, Vec<DeclEntry>) {
    let file_res = crate::program::resolve::full::resolve_file_obligations(
        pf,
        primary_app_ref,
        graph,
        index,
        surface,
        obj_node_map,
    );
    let def_surface = def_surface_fingerprint(pf);

    let mut decls: Vec<DeclEntry> = Vec::new();
    for obj in &pf.file.objects {
        let obj_key = match obj.id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(obj.name.fold_identifier().into()),
        };
        let obj_node_id = ObjectNodeId {
            app: primary_app_ref,
            kind: obj.kind,
            key: obj_key,
        };
        for routine in &obj.routines {
            let id = source_routine_node_id(obj_node_id.clone(), routine);
            decls.push(DeclEntry {
                id,
                name: routine.name.clone(),
                origin: routine.origin.clone(),
                name_origin: routine.name_origin.clone(),
                virtual_path: pf.virtual_path.clone(),
            });
        }
    }
    decls.sort_by_key(|d| d.origin.byte.start);

    (file_res.edges, def_surface, decls)
}

/// DERIVED index (see [`LspSnapshot::decl_by_id`]'s doc): every `DeclEntry`
/// across every file, keyed by its `RoutineNodeId`. Rebuilt WHOLESALE from
/// `decls_by_file` at [`LspSnapshot::build_full`] and every rung-2/3 rebuild;
/// rung 1 instead PATCHES this index for the touched file(s) only
/// (`apply_rung1_core`) — see this module's amended ownership-law doc.
///
/// **Duplicate-`RoutineNodeId` winner is UNSPECIFIED** (`decls_by_file` can
/// hold the SAME id declared in >1 file — the
/// `dedup_routines_preserving_genuine_overloads` population — and this
/// function, iterating a `HashMap`, lets whichever file's entry is visited
/// LAST win, which is already nondeterministic across builds/platforms). The
/// only invariant callers may rely on: every id present in the UNION of
/// `decls_by_file` maps to ONE of its declaring files' own entries. The
/// rung-1 patch (`Updater::decl_multiplicity`) preserves this exact
/// invariant rather than any specific winner — see `apply_rung1_core`'s doc.
#[must_use]
pub fn build_decl_by_id(
    decls_by_file: &HashMap<String, Arc<Vec<DeclEntry>>>,
) -> HashMap<RoutineNodeId, DeclEntry> {
    let mut decl_by_id = HashMap::new();
    for decls in decls_by_file.values() {
        for d in decls.iter() {
            decl_by_id.insert(d.id.clone(), d.clone());
        }
    }
    decl_by_id
}

/// Count of DISTINCT declaring FILES per `RoutineNodeId` across
/// `decls_by_file` (Tier-2 latency wave, Task 1) — the companion index
/// `Updater::decl_multiplicity` maintains alongside `decl_by_id` so a rung-1
/// patch can tell whether an id disappearing from ONE file's decl list is
/// gone from the workspace entirely (multiplicity reaches 0 — evict from
/// `decl_by_id`) or merely loses ONE of several declaring files (still >0 —
/// `decl_by_id`'s entry survives, re-derived from a surviving file if the
/// evicted file happened to be the current winner). An id declared TWICE
/// within the SAME file (a pathological, not-expected-in-practice case)
/// still counts as ONE declaring file here — `decls_by_file`'s own
/// per-file dedup, mirrored via the `seen` set below, keeps this a
/// per-FILE count, not a raw per-decl occurrence count.
#[must_use]
pub fn build_decl_multiplicity(
    decls_by_file: &HashMap<String, Arc<Vec<DeclEntry>>>,
) -> HashMap<RoutineNodeId, u32> {
    let mut mult: HashMap<RoutineNodeId, u32> = HashMap::new();
    for decls in decls_by_file.values() {
        let mut seen: std::collections::HashSet<&RoutineNodeId> = std::collections::HashSet::new();
        for d in decls.iter() {
            if seen.insert(&d.id) {
                *mult.entry(d.id.clone()).or_insert(0) += 1;
            }
        }
    }
    mult
}

/// `snap` with every dependency app's source files dropped, for
/// [`LspSnapshot::snap`]. `source` stays `Some` with its tier and content
/// hash, so "this app ships source" keeps its meaning; only the files go. No
/// LSP reader needs them after the build: positions come from
/// [`LspSnapshot::dep_lines`], `dependencyDocumentSymbol` and `al-preview://`
/// read the ABI, rungs 1 and 2 reuse the dependency tier, and rung 3 reloads
/// from disk.
fn without_dependency_text(mut snap: AppSetSnapshot) -> AppSetSnapshot {
    let workspace = snap.workspace_app.clone();
    for unit in snap.apps.iter_mut().filter(|u| u.id != workspace) {
        if let Some(source) = unit.source.as_mut() {
            source.files = Arc::new(Vec::new());
        }
    }
    snap
}

/// Build [`LspSnapshot::dep_lines`]: a [`LineIndex`] of every dependency file
/// by `(app, virtual path)`, which [`LspSnapshot::decl_and_line_table`] pairs
/// with [`LspSnapshot::dep_meta`] for a dependency decl's position conversion.
/// Reads the snapshot's source files (no parse), first file winning on a
/// repeated key. It runs once per live shared dependency tier:
/// [`LspSnapshot::from_context`] calls it inside the tier's `get_or_init`, so
/// every root on that tier shares the result.
#[must_use]
pub(crate) fn build_dep_lines(
    snap: &AppSetSnapshot,
    apps: &AppRegistry,
    primary: AppRef,
) -> DepLines {
    // A source without its text (S10.1b) would index no file of its app, and
    // every root on the tier would share that silently.
    assert!(
        !snap.has_deferred_dependency_text(),
        "dependency line indexes built from a snapshot without dependency text"
    );
    let mut dep_lines = DepLines::new();
    for unit in &snap.apps {
        let (Some(app_ref), Some(source)) = (apps.find(&unit.id), unit.source.as_ref()) else {
            continue;
        };
        if app_ref == primary {
            continue;
        }
        for f in source.files.iter() {
            dep_lines
                .entry((app_ref, f.virtual_path.as_str().into()))
                .or_insert_with(|| LineIndex::new(&f.text));
        }
    }
    dep_lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::resolve::edge::{Edge, EdgeKind};
    use crate::program::resolve::full::resolve_full_program;

    /// A fixture workspace exercising: a cross-file call (Alpha.DoWork calls
    /// Beta.Process via a declared `Codeunit "Beta"` local var), a same-name
    /// overload pair (`Alpha.Calc(Integer)` / `Alpha.Calc(Text)`), an event
    /// publisher/subscriber pair (`Beta.OnAfterProcess` / `Gamma.
    /// HandleAfterProcess`), and a non-ASCII (Danish) identifier (`Løbenr`) —
    /// per the task brief's Step-1 fixture requirements.
    fn write_fixture_workspace(dir: &std::path::Path) {
        std::fs::write(
            dir.join("app.json"),
            r#"{
    "id": "33333333-0000-0000-0000-000000000008",
    "name": "Task8 LspSnapshot Fixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .expect("write app.json");

        std::fs::write(
            dir.join("Alpha.al"),
            r#"codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
    begin
        Beta.Process();
        Calc(1);
        Calc('x');
    end;

    procedure Calc(X: Integer)
    begin
    end;

    procedure Calc(X: Text)
    begin
    end;

    procedure Løbenr()
    begin
    end;
}
"#,
        )
        .expect("write Alpha.al");

        std::fs::write(
            dir.join("Beta.al"),
            r#"codeunit 50101 "Beta"
{
    procedure Process()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterProcess()
    begin
    end;

    // A publisher nobody subscribes to: its event link has no routes.
    [IntegrationEvent(false, false)]
    procedure OnNobodyListens()
    begin
    end;
}
"#,
        )
        .expect("write Beta.al");

        std::fs::write(
            dir.join("Gamma.al"),
            r#"codeunit 50102 "Gamma"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Beta", 'OnAfterProcess', '', false, false)]
    local procedure HandleAfterProcess()
    begin
    end;
}
"#,
        )
        .expect("write Gamma.al");
    }

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture_workspace(dir.path());
        dir
    }

    // ── build_full: union equals a direct resolve_full_program run ────────

    #[test]
    fn build_full_edges_match_resolve_full_program() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");

        let mut got: Vec<Edge> = snap
            .edges_by_file
            .values()
            .flat_map(|v| v.iter().map(|ce| ce.edge.clone()))
            .collect();
        got.extend(snap.event_edges.iter().map(|ce| ce.edge.clone()));
        got.sort();

        // The LSP keeps only event links with routes; the report keeps all.
        let all: Vec<Edge> = report.edges.into_iter().map(|ce| ce.edge).collect();
        let is_empty_link = |e: &Edge| e.kind == EdgeKind::EventFlow && e.routes.is_empty();
        assert!(
            all.iter().any(is_empty_link),
            "the report must still hold route-less event links (not lost)"
        );
        let mut want: Vec<Edge> = all.into_iter().filter(|e| !is_empty_link(e)).collect();
        want.sort();

        assert_eq!(
            got, want,
            "build_full's edges_by_file + event_edges union must equal a \
             direct resolve_full_program run minus route-less event links \
             (order-insensitive)"
        );
        assert!(
            snap.event_edges.iter().all(|ce| !ce.edge.routes.is_empty()),
            "no route-less event link is stored in the LSP snapshot"
        );
        assert!(!got.is_empty(), "fixture must produce real edges");
    }

    // ── determinism across two builds (generation excluded) ───────────────

    #[test]
    fn build_full_is_deterministic_across_two_builds() {
        let dir = fixture_dir();
        let s1 = LspSnapshot::build_full(dir.path()).expect("build 1");
        let s2 = LspSnapshot::build_full(dir.path()).expect("build 2");

        let mut files1: Vec<_> = s1.decls_by_file.keys().cloned().collect();
        let mut files2: Vec<_> = s2.decls_by_file.keys().cloned().collect();
        files1.sort();
        files2.sort();
        assert_eq!(files1, files2, "same workspace file set");
        for f in &files1 {
            let ids1: Vec<_> = s1.decls_by_file[f].iter().map(|d| d.id.clone()).collect();
            let ids2: Vec<_> = s2.decls_by_file[f].iter().map(|d| d.id.clone()).collect();
            assert_eq!(ids1, ids2, "file {f}: same decl ids in the same order");
        }

        let mut ef1: Vec<_> = s1.edges_by_file.keys().cloned().collect();
        let mut ef2: Vec<_> = s2.edges_by_file.keys().cloned().collect();
        ef1.sort();
        ef2.sort();
        assert_eq!(ef1, ef2);
        for f in &ef1 {
            let mut a: Vec<Edge> = s1.edges_by_file[f]
                .iter()
                .map(|ce| ce.edge.clone())
                .collect();
            let mut b: Vec<Edge> = s2.edges_by_file[f]
                .iter()
                .map(|ce| ce.edge.clone())
                .collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "file {f}: same edge set");
        }

        let mut e1: Vec<Edge> = s1.event_edges.iter().map(|ce| ce.edge.clone()).collect();
        let mut e2: Vec<Edge> = s2.event_edges.iter().map(|ce| ce.edge.clone()).collect();
        e1.sort();
        e2.sort();
        assert_eq!(e1, e2, "same event-edge set");

        let mut inc1: Vec<_> = s1
            .incoming
            .iter()
            .map(|(k, v)| {
                let mut v = v.clone();
                v.sort_by(|a, b| (a.file.as_ref(), a.idx).cmp(&(b.file.as_ref(), b.idx)));
                (k.clone(), v)
            })
            .collect();
        let mut inc2: Vec<_> = s2
            .incoming
            .iter()
            .map(|(k, v)| {
                let mut v = v.clone();
                v.sort_by(|a, b| (a.file.as_ref(), a.idx).cmp(&(b.file.as_ref(), b.idx)));
                (k.clone(), v)
            })
            .collect();
        inc1.sort_by(|a, b| a.0.cmp(&b.0));
        inc2.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(inc1, inc2, "same incoming index (generation excluded)");
    }

    // ── decl_at: name hit, body-fallback hit, and none ─────────────────────

    #[test]
    fn decl_at_hits_name_then_falls_back_to_whole_decl_then_none() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let alpha_decls = snap
            .decls_by_file
            .get("Alpha.al")
            .expect("Alpha.al must be indexed");
        let lobenr = alpha_decls
            .iter()
            .find(|d| d.name == "Løbenr")
            .expect("Løbenr decl must be present (non-ASCII identifier fixture)");

        // Name hit: querying exactly at the name token's start must resolve
        // to Løbenr's own DeclEntry.
        let hit = snap
            .decl_at(
                "Alpha.al",
                lobenr.name_origin.start.row,
                lobenr.name_origin.start.column,
            )
            .expect("name-position hit");
        assert_eq!(hit.id, lobenr.id);

        // Whole-decl (body) hit: `origin.start` precedes `name_origin.start`
        // (the `procedure` keyword comes before the name token), so this
        // point is inside `origin` but outside `name_origin` — exercising the
        // fallback arm specifically.
        assert!(
            (lobenr.origin.start.row, lobenr.origin.start.column)
                < (
                    lobenr.name_origin.start.row,
                    lobenr.name_origin.start.column
                ),
            "fixture assumption: origin must start before name_origin"
        );
        let body_hit = snap
            .decl_at(
                "Alpha.al",
                lobenr.origin.start.row,
                lobenr.origin.start.column,
            )
            .expect("whole-decl-position hit");
        assert_eq!(body_hit.id, lobenr.id);

        // None: a position far past the end of the file, and an unknown file.
        assert!(snap.decl_at("Alpha.al", 9_999, 0).is_none());
        assert!(snap.decl_at("NoSuchFile.al", 0, 0).is_none());
    }

    // ── build_incoming: cross-file caller + event subscriber's publisher ──

    #[test]
    fn build_incoming_finds_cross_file_caller_and_event_subscriber_publisher() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let beta_process = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("Beta.Process decl")
            .id
            .clone();
        let incoming_process = snap
            .incoming
            .get(&beta_process)
            .expect("Beta.Process must have an incoming caller");
        assert!(
            incoming_process.iter().any(|r| &*r.file == "Alpha.al"),
            "Alpha.DoWork's cross-file call must be indexed as incoming to \
             Beta.Process; got {incoming_process:?}"
        );
        for r in incoming_process.iter().filter(|r| &*r.file == "Alpha.al") {
            let ce = snap.edge(r);
            assert!(
                ce.edge.routes.iter().any(
                    |route| matches!(&route.target, RouteTarget::Routine(t) if *t == beta_process)
                ),
                "the referenced edge must actually route to Beta.Process"
            );
        }

        let gamma_sub = snap.decls_by_file["Gamma.al"]
            .iter()
            .find(|d| d.name == "HandleAfterProcess")
            .expect("Gamma.HandleAfterProcess decl")
            .id
            .clone();
        let incoming_sub = snap
            .incoming
            .get(&gamma_sub)
            .expect("the subscriber must have an incoming publisher edge");
        assert!(
            incoming_sub.iter().any(|r| &*r.file == EVENT_EDGES_KEY),
            "the event edge must be indexed under the reserved event-edges \
             key; got {incoming_sub:?}"
        );
        for r in incoming_sub.iter().filter(|r| &*r.file == EVENT_EDGES_KEY) {
            let ce = snap.edge(r);
            assert_eq!(ce.edge.kind, EdgeKind::EventFlow);
        }
    }

    // ── publisher_fanout: precomputed, O(1)-lookupable (t3 whole-branch ───
    // ── review blocker fix — see LspSnapshot::publisher_fanout's doc) ──────

    #[test]
    fn publisher_fanout_counts_real_routes_and_omits_unpublished_routines() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        // Beta.OnAfterProcess is a real publisher with exactly one real
        // subscriber (Gamma.HandleAfterProcess, per the fixture) — its
        // publisher_fanout entry must equal 1, matching the OLD
        // effective_incoming_count's `event_edges.iter().filter(from ==
        // id).map(routes.len()).sum()` computation exactly (same value, now
        // precomputed instead of scanned per call).
        let beta_on_after_process = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "OnAfterProcess")
            .expect("Beta.OnAfterProcess decl")
            .id
            .clone();
        assert_eq!(
            snap.publisher_fanout.get(&beta_on_after_process).copied(),
            Some(1),
            "a publisher with exactly one real subscriber must have \
             publisher_fanout == 1"
        );

        // A routine that is NEVER a publisher (Beta.Process, an ordinary
        // procedure) must have NO publisher_fanout entry at all — never a
        // spurious Some(0) that would silently inflate a future consumer's
        // sum by an extra hashmap probe for no reason.
        let beta_process = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("Beta.Process decl")
            .id
            .clone();
        assert_eq!(
            snap.publisher_fanout.get(&beta_process),
            None,
            "an ordinary (non-publisher) routine must have no publisher_fanout entry"
        );
    }

    #[test]
    fn publisher_fanout_omits_a_publisher_with_zero_real_subscribers() {
        // emit_event_flow_edges emits ONE ClassifiedEdge per publisher
        // declaration UNCONDITIONALLY, even with zero subscribers — mere
        // edge PRESENCE must never count as fan-out (mirrors
        // effective_incoming_count's own "as-publisher fan-out" doc).
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("app.json"),
            r#"{
    "id": "33333333-0000-0000-0000-00000000000f",
    "name": "PublisherFanoutZeroFixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .expect("write app.json");
        std::fs::write(
            dir.path().join("Lonely.al"),
            r#"codeunit 50100 "Lonely"
{
    [IntegrationEvent(false, false)]
    procedure OnNobodyListens()
    begin
    end;
}
"#,
        )
        .expect("write Lonely.al");

        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let publisher = snap.decls_by_file["Lonely.al"]
            .iter()
            .find(|d| d.name == "OnNobodyListens")
            .expect("OnNobodyListens decl")
            .id
            .clone();
        assert_eq!(
            snap.publisher_fanout.get(&publisher),
            None,
            "a publisher with ZERO real subscribers must have no \
             publisher_fanout entry — edge presence alone is never fan-out"
        );
        // Its route-less link is not stored at all, and the lens count and
        // the incoming index are unchanged (zero).
        assert!(
            snap.event_edges.iter().all(|ce| ce.edge.from != publisher),
            "a route-less event link must not be stored in the LSP snapshot"
        );
        assert_eq!(
            crate::lsp::lens::effective_incoming_count(&snap, &publisher),
            0
        );
        assert!(!snap.incoming.contains_key(&publisher));
    }

    // ── build_incoming: one edge, 2 routes to the SAME target → 1 EdgeRef ──
    // (T3 Task 9 review carry-over from Task 8: a pathological
    // ambiguous-overload-style edge whose routes list happens to name the
    // same target twice must not produce a duplicate incoming entry.)

    #[test]
    fn build_incoming_dedups_one_edges_repeated_route_to_the_same_target() {
        use crate::program::node::{AppRef, ObjKey, ObjectNodeId};
        use crate::program::resolve::edge::{
            CanonicalSpan, DispatchShape, Evidence, Route, RouteTarget, SetCompleteness, SiteId,
            SourcePos, Witness,
        };
        use al_syntax::ir::ObjectKind;

        fn rid(name: &str) -> RoutineNodeId {
            RoutineNodeId {
                object: ObjectNodeId {
                    app: AppRef(0),
                    kind: ObjectKind::Codeunit,
                    key: ObjKey::Id(1),
                },
                name_lc: name.into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            }
        }

        fn dup_route(target: &RoutineNodeId) -> Route {
            Route {
                target: RouteTarget::Routine(target.clone()),
                evidence: Evidence::Source,
                conditions: vec![],
                witness: Witness::None,
                receiver_tier: None,
            }
        }

        let target = rid("target");
        let caller = rid("caller");
        let edge = Edge {
            from: caller.clone(),
            site: SiteId {
                caller,
                span: CanonicalSpan {
                    unit: "F.al".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 2 },
                },
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape: DispatchShape::AmbiguousOverload,
            completeness: SetCompleteness::Complete,
            // Pathological: the SAME target named twice in one edge's routes.
            routes: vec![dup_route(&target), dup_route(&target)],
        };

        let mut edges_by_file: HashMap<String, Arc<Vec<ClassifiedEdge>>> = HashMap::new();
        edges_by_file.insert(
            "F.al".to_string(),
            Arc::new(vec![ClassifiedEdge {
                obligation_id: ObligationId::CallSite {
                    caller: edge.from.clone(),
                    span: edge.site.span.clone(),
                    callee_fp: edge.site.callee_fingerprint,
                },
                edge,
            }]),
        );

        let (incoming, _fanout) = build_incoming(&edges_by_file, &[]);
        let refs = incoming
            .get(&target)
            .expect("target must have an incoming entry");
        assert_eq!(
            refs.len(),
            1,
            "one edge with 2 routes to the SAME target must produce exactly 1 \
             EdgeRef, not one per route; got {refs:?}"
        );
    }

    // ── build_incoming: TWO DIFFERENT edges to the same target → 2 EdgeRefs ──
    // (review fix-wave item 4: the per-edge dedup guard above must never
    // collapse genuinely distinct callers — only a repeated route WITHIN one
    // edge is deduplicated.)

    #[test]
    fn build_incoming_keeps_two_different_edges_to_the_same_target_as_2_edgerefs() {
        use crate::program::node::{AppRef, ObjKey, ObjectNodeId};
        use crate::program::resolve::edge::{
            CanonicalSpan, DispatchShape, Evidence, Route, RouteTarget, SetCompleteness, SiteId,
            SourcePos, Witness,
        };
        use al_syntax::ir::ObjectKind;

        fn rid(name: &str) -> RoutineNodeId {
            RoutineNodeId {
                object: ObjectNodeId {
                    app: AppRef(0),
                    kind: ObjectKind::Codeunit,
                    key: ObjKey::Id(1),
                },
                name_lc: name.into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            }
        }

        fn single_route_edge(
            caller: RoutineNodeId,
            target: &RoutineNodeId,
            callee_fp: u64,
        ) -> Edge {
            Edge {
                from: caller.clone(),
                site: SiteId {
                    caller,
                    span: CanonicalSpan {
                        unit: "F.al".into(),
                        start: SourcePos { line: 1, col: 1 },
                        end: SourcePos { line: 1, col: 2 },
                    },
                    callee_fingerprint: callee_fp,
                },
                kind: EdgeKind::Call,
                shape: DispatchShape::Exact,
                completeness: SetCompleteness::Complete,
                routes: vec![Route {
                    target: RouteTarget::Routine(target.clone()),
                    evidence: Evidence::Source,
                    conditions: vec![],
                    witness: Witness::None,
                    receiver_tier: None,
                }],
            }
        }

        let target = rid("target");
        let caller_a = rid("caller_a");
        let caller_b = rid("caller_b");
        let edge_a = single_route_edge(caller_a, &target, 1);
        let edge_b = single_route_edge(caller_b, &target, 2);

        let mut edges_by_file: HashMap<String, Arc<Vec<ClassifiedEdge>>> = HashMap::new();
        edges_by_file.insert(
            "F.al".to_string(),
            Arc::new(vec![
                ClassifiedEdge {
                    obligation_id: ObligationId::CallSite {
                        caller: edge_a.from.clone(),
                        span: edge_a.site.span.clone(),
                        callee_fp: edge_a.site.callee_fingerprint,
                    },
                    edge: edge_a,
                },
                ClassifiedEdge {
                    obligation_id: ObligationId::CallSite {
                        caller: edge_b.from.clone(),
                        span: edge_b.site.span.clone(),
                        callee_fp: edge_b.site.callee_fingerprint,
                    },
                    edge: edge_b,
                },
            ]),
        );

        let (incoming, _fanout) = build_incoming(&edges_by_file, &[]);
        let refs = incoming
            .get(&target)
            .expect("target must have incoming entries");
        assert_eq!(
            refs.len(),
            2,
            "two DIFFERENT edges naming the same target must NOT be deduped \
             against each other — got {refs:?}"
        );
        assert_ne!(
            refs[0].idx, refs[1].idx,
            "the two EdgeRefs must point at two distinct edge indices"
        );
    }

    // ── snapshot-scoped LineTable cache (docs/OUTSTANDING.md item) ─────────

    #[test]
    fn parsed_file_entry_line_table_is_memoized() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let entry = snap.parsed.get("Alpha.al").expect("Alpha.al entry");

        let first: *const LineTable = entry.line_table();
        let second: *const LineTable = entry.line_table();
        assert!(
            std::ptr::eq(first, second),
            "two calls to `line_table()` against the SAME `ParsedFileEntry` \
             must return the identical cached instance, not rebuild it — \
             this is the whole point of the snapshot-scoped cache"
        );
    }

    #[test]
    fn parsed_file_entry_line_table_matches_fresh_construction() {
        use crate::lsp::encoding::PositionEncoding;

        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let entry = snap.parsed.get("Alpha.al").expect("Alpha.al entry");

        let cached = entry.line_table();
        let fresh = LineTable::new(Arc::clone(&entry.text));

        let lobenr = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "Løbenr")
            .expect("Løbenr decl — a non-ASCII name exercises real utf-16 math");

        for enc in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            assert_eq!(
                cached.col_out(
                    lobenr.name_origin.start.row,
                    lobenr.name_origin.start.column,
                    enc
                ),
                fresh.col_out(
                    lobenr.name_origin.start.row,
                    lobenr.name_origin.start.column,
                    enc
                ),
                "the cached table must agree with an independently-built one ({enc:?})"
            );
        }
    }
}
