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
//! - the parse (`Arc<AlFile>`, compared by identity: the session reuses a
//!   tree only for byte-equal text, and a dependency's tree belongs to the
//!   tier, which lives as long as `DepKey` is unchanged) and the text (by
//!   bytes, unless it is the same allocation);
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
/// revision.
pub(crate) struct ProjectionInputs<'a> {
    pub(crate) tree: &'a Arc<al_syntax::ir::AlFile>,
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
    tree: Arc<al_syntax::ir::AlFile>,
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
            tree: Arc::clone(i.tree),
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
    pub(crate) fn matches(&self, i: &ProjectionInputs<'_>) -> bool {
        Arc::ptr_eq(&self.tree, i.tree)
            && (Arc::ptr_eq(&self.text, i.text) || *self.text == **i.text)
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
}

/// The raw rows kept for the next revision (see the module doc).
#[derive(Default)]
pub struct RowCache {
    pub(crate) workspace: HashMap<String, CachedFile>,
    pub(crate) dependency: HashMap<(AppRef, String), CachedFile>,
    pub(crate) abi: Option<CachedAbi>,
    pub(crate) last: RowCacheStats,
}

impl RowCache {
    /// What the last assembly through this cache reused.
    #[must_use]
    pub fn last_stats(&self) -> RowCacheStats {
        self.last
    }
}
