//! The raw model rows a workspace session keeps between revisions
//! (context-server design §6.8, layers L3 and L4).
//!
//! The model assembly projects every file it holds into rows (`project_ir`)
//! and every symbol-only dependency's ABI into rows. Both are pure functions
//! of their inputs, so a revision may take the previous revision's rows for a
//! file exactly when every input is equal. The cache stores, per file, the
//! rows as `project_ir` pushed them (the raw rows, before `finish_resolved`
//! writes record types, merged fields and root classifications into the
//! model's own copy), with every input they were projected from:
//!
//! - the text (by bytes, unless it is the same allocation), which stands
//!   for the parse: a tree is `al_syntax::parse` of the text and nothing
//!   else, so equal text is an equal tree. No tree is kept here: a session
//!   that holds no dependency tree between revisions (`--low-mem`,
//!   `DependencyBodies::OnDemand`) parses again only the files whose rows
//!   miss, and a reused row never needs its tree;
//! - the population (which objects and routines are projected: the program
//!   graph's physical rows for a workspace file, the demanded slice for a
//!   dependency file), the file's parens-less calls, the assembly needs, the
//!   app guid and the source unit id.
//!
//! The file's `FileIx` is the one input kept symbolically: `project_ir` only
//! writes it into each routine row's `file`, and a file added before this one
//! (in the workspace, or any workspace file for a dependency file, numbered
//! after them) shifts it. A reused copy is re-stamped with the current index
//! (`CachedFile::rows_for`).
//!
//! The ABI block (every symbol-only dependency's rows, in package-path order)
//! is kept with its required-dependency list, the apps' `AppRef`s and the
//! needs. Its other input, the packages' `SymbolReference`, can change only
//! with `DepKey`; the session drops the whole cache on a P change (the cache
//! lives in its retained state), so a kept block never outlives its packages.
//!
//! Each row's program node is NOT cached: the assembly recomputes it every
//! revision from the current graph (a workspace row's from the physical rows,
//! a dependency row's from its declaration), so a node id that depends on
//! another file's declaration can never go stale.
//!
//! The order of the rows is the cold assembly's: the assembly walks files in
//! the same order and appends each file's rows, reused or fresh, in place.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::workspace::{AbiRowIds, AssemblyNeeds, FilePopulation, ModelEntities};
use crate::program::node::{AppRef, RoutineNodeId};
use crate::program::site_key::FileIx;

/// Everything `project_ir` reads for one file, borrowed from the current
/// revision. Its tree is the text's parse (see the module doc).
pub(crate) struct ProjectionInputs<'a> {
    pub(crate) text: &'a Arc<str>,
    pub(crate) population: &'a FilePopulation,
    pub(crate) parenless: &'a HashSet<al_syntax::ir::ExprId>,
    pub(crate) file: Option<FileIx>,
    pub(crate) needs: AssemblyNeeds,
    pub(crate) app_guid: &'a str,
    pub(crate) source_unit_id: &'a str,
}

/// One file's raw rows and the inputs they were projected from.
pub(crate) struct CachedFile {
    text: Arc<str>,
    population: FilePopulation,
    parenless: HashSet<al_syntax::ir::ExprId>,
    file: Option<FileIx>,
    needs: AssemblyNeeds,
    app_guid: String,
    source_unit_id: String,
    pub(crate) rows: ModelEntities,
    /// `project_ir`'s `(object, routine)` position of each routine row.
    pub(crate) positions: Vec<(usize, usize)>,
}

impl CachedFile {
    pub(crate) fn new(
        i: &ProjectionInputs<'_>,
        rows: ModelEntities,
        positions: Vec<(usize, usize)>,
    ) -> Self {
        CachedFile {
            text: Arc::clone(i.text),
            population: i.population.clone(),
            parenless: i.parenless.clone(),
            file: i.file,
            needs: i.needs,
            app_guid: i.app_guid.to_string(),
            source_unit_id: i.source_unit_id.to_string(),
            rows,
            positions,
        }
    }

    /// Whether these rows, re-stamped with `i.file` ([`Self::rows_for`]), are
    /// what `project_ir` would push for `i`: every input equal but the
    /// `FileIx`, which `project_ir` only writes into each routine row's
    /// `file` (a per-build number: a file added before this one shifts it).
    /// The text stands for the tree (its parse).
    pub(crate) fn matches(&self, i: &ProjectionInputs<'_>) -> bool {
        (Arc::ptr_eq(&self.text, i.text) || *self.text == **i.text)
            && self.population == *i.population
            && self.parenless == *i.parenless
            && self.needs == i.needs
            && self.app_guid == i.app_guid
            && self.source_unit_id == i.source_unit_id
    }

    /// A copy of the rows for a revision in which the file is numbered
    /// `file`: every routine row's `file` is the stamp `project_ir` wrote
    /// (asserted), replaced by the current one.
    pub(crate) fn rows_for(&self, file: Option<FileIx>) -> ModelEntities {
        let mut rows = self.rows.clone();
        if file != self.file {
            for r in &mut rows.routines {
                debug_assert_eq!(r.file, self.file, "project_ir stamps every row");
                r.file = file;
            }
        }
        rows
    }
}

/// The symbol-only dependencies' rows (one block, as the cold assembly
/// appends it) and what they were built from.
pub(crate) struct CachedAbi {
    /// Each symbol-only required dependency in assembly order: its app guid
    /// as the snapshot spells it, its package path and its graph `AppRef`.
    pub(crate) apps: Vec<(String, Option<std::path::PathBuf>, Option<AppRef>)>,
    pub(crate) needs: AssemblyNeeds,
    pub(crate) rows: ModelEntities,
    pub(crate) node_of: Vec<Option<RoutineNodeId>>,
    pub(crate) ids: AbiRowIds,
}

/// What one assembly took from the cache and what it projected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowCacheStats {
    /// Workspace files projected / whose rows were reused.
    pub workspace_projected: usize,
    pub workspace_reused: usize,
    /// Source-bearing dependency files projected / reused.
    pub dependency_projected: usize,
    pub dependency_reused: usize,
    /// Whether the ABI block was reused.
    pub abi_reused: bool,
    /// Dependency files parsed again by this assembly because their rows or
    /// routine ids missed and the build held no dependency tree
    /// (`DependencyBodies::OnDemand`, a body-only revision): exactly the
    /// files projected or whose ids were computed. Always 0 when the build
    /// holds the trees.
    pub dependency_parsed: usize,
}

/// The raw rows kept for the next revision (see the module doc).
#[derive(Default)]
pub struct RowCache {
    pub(crate) workspace: HashMap<String, CachedFile>,
    pub(crate) dependency: HashMap<(AppRef, String), CachedFile>,
    /// Each dependency file's routine ids per object (`dependency_routine_ids`,
    /// a pure function of the app and the parse), with the text they were
    /// built from (it stands for the parse): the demanded slice and the rows'
    /// nodes are looked up.
    pub(crate) dependency_ids: HashMap<(AppRef, String), DependencyIds>,
    pub(crate) abi: Option<CachedAbi>,
    pub(crate) last: RowCacheStats,
    /// Tests only: a `Weak` to every dependency tree an assembly parsed on
    /// demand (`WorkspaceSession`'s tree probe).
    #[cfg(test)]
    pub(crate) probe: Option<Vec<std::sync::Weak<al_syntax::ir::AlFile>>>,
}

/// One dependency file's routine ids and the text they were built from.
pub(crate) struct DependencyIds {
    text: Arc<str>,
    ids: Arc<Vec<Vec<RoutineNodeId>>>,
}

impl RowCache {
    /// The kept routine ids of dependency file `path` of `app`, when they
    /// were built from this text. The text check is defensive: a dependency
    /// text changes only with its package, a P change that drops the whole
    /// cache (a discrimination break that skips it stays green, 2026-11-02).
    pub(crate) fn kept_dependency_ids(
        &self,
        app: AppRef,
        path: &str,
        text: &Arc<str>,
    ) -> Option<Arc<Vec<Vec<RoutineNodeId>>>> {
        let d = self.dependency_ids.get(&(app, path.to_string()))?;
        (Arc::ptr_eq(&d.text, text) || *d.text == **text).then(|| Arc::clone(&d.ids))
    }

    /// The routine ids of dependency file `path` of `app`: the kept ones when
    /// they were built from this text, else `compute()`'s, kept.
    pub(crate) fn dependency_ids(
        &mut self,
        app: AppRef,
        path: &str,
        text: &Arc<str>,
        compute: impl FnOnce() -> Vec<Vec<RoutineNodeId>>,
    ) -> Arc<Vec<Vec<RoutineNodeId>>> {
        if let Some(ids) = self.kept_dependency_ids(app, path, text) {
            return ids;
        }
        let ids = Arc::new(compute());
        self.dependency_ids.insert(
            (app, path.to_string()),
            DependencyIds {
                text: Arc::clone(text),
                ids: Arc::clone(&ids),
            },
        );
        ids
    }

    /// The kept rows of dependency file `key`, when they match `inputs`.
    pub(crate) fn kept_dependency_rows(
        &self,
        key: &(AppRef, String),
        inputs: &ProjectionInputs<'_>,
    ) -> Option<&CachedFile> {
        self.dependency.get(key).filter(|f| f.matches(inputs))
    }

    /// What the last assembly through this cache reused.
    #[must_use]
    pub fn last_stats(&self) -> RowCacheStats {
        self.last
    }

    /// Drop every dependency file's kept rows and routine ids (an eviction,
    /// context-server design §7: the next assembly projects them again).
    /// Returns how many files' rows were dropped.
    pub(crate) fn evict_dependency(&mut self) -> usize {
        let n = self.dependency.len();
        self.dependency = HashMap::new();
        self.dependency_ids = HashMap::new();
        n
    }

    /// The dependency files' entries split into their parts, for a byte
    /// census (`WorkspaceSession::take_census_parts`): routine rows, the
    /// other rows, the populations and parens-less sets, then the texts.
    pub(crate) fn split_dependency_for_census(
        dependency: HashMap<(AppRef, String), CachedFile>,
    ) -> Vec<(&'static str, Box<dyn std::any::Any>)> {
        let mut routines = Vec::new();
        let mut other = Vec::new();
        let mut inputs = Vec::new();
        let mut texts = Vec::new();
        for (k, f) in dependency {
            routines.push(f.rows.routines);
            other.push((f.rows.objects, f.rows.tables, f.positions));
            inputs.push((k, f.population, f.parenless, f.app_guid, f.source_unit_id));
            texts.push(f.text);
        }
        vec![
            ("rows.dependency.routines", Box::new(routines)),
            ("rows.dependency.objects_tables", Box::new(other)),
            ("rows.dependency.inputs", Box::new(inputs)),
            ("rows.dependency.texts", Box::new(texts)),
        ]
    }
}
