//! #160 steps 10 + 11a (spec `2026-10-22-issue-160-steps10-11a-siteix-routine-identity-design.md`
//! §4.1, §4.2): the key the model and the program engine name a call site by.
//!
//! The resolver stamps `ClassifiedEdge.site` when it is given a file; the body
//! walk records `ModelRoutine.file` and `ModelRoutine.site_exprs`.

use std::collections::HashMap;

use crate::program::graph::ProgramGraph;
use crate::program::node::AppRef;
use crate::snapshot::ParsedUnit;

/// A per-build number for one parsed file of one app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileIx(pub u32);

/// `SiteIx` (epic §5.3): a call expression in one file. `expr` is
/// `ExprId::index()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SiteIx {
    pub file: FileIx,
    pub expr: u32,
}

/// What a program call-site edge records about its site (§4.2, §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramSite {
    pub at: SiteIx,
    /// The site is a record op whose operation can fire a table trigger
    /// (`can_fire_trigger` of the extractor's op name).
    pub fires: bool,
}

/// The model's expression indices, parallel to `call_sites`,
/// `record_operations` and `operation_sites` (one entry each, same order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteExprs {
    pub calls: Vec<u32>,
    pub record_ops: Vec<u32>,
    pub op_sites: Vec<u32>,
}

/// Every parsed file of the build, numbered: the workspace unit's files, then the
/// dependency bodies' when the profile keeps them. Keyed by app AND path, so two
/// apps with the same virtual path get two numbers.
#[derive(Clone, Debug, Default)]
pub struct FileTable {
    by_key: HashMap<(AppRef, String), FileIx>,
}

impl FileTable {
    /// Number `parsed`, then `dep_bodies`, in that order. A unit whose app the
    /// graph does not know is skipped (as every other per-file table does).
    #[must_use]
    pub fn build(graph: &ProgramGraph, parsed: &[ParsedUnit], dep_bodies: &[ParsedUnit]) -> Self {
        let mut by_key = HashMap::new();
        for unit in parsed.iter().chain(dep_bodies) {
            let Some(app) = graph.apps.find(&unit.app) else {
                continue;
            };
            for pf in &unit.files {
                let next = FileIx(u32::try_from(by_key.len()).expect("fewer than 2^32 files"));
                by_key.entry((app, pf.virtual_path.clone())).or_insert(next);
            }
        }
        Self { by_key }
    }

    /// The number of `path` in `app`, if the build parsed it.
    #[must_use]
    pub fn get(&self, app: AppRef, path: &str) -> Option<FileIx> {
        self.by_key.get(&(app, path.to_string())).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The `program.file_table` census bucket (#160 §4.13): the entries and
    /// their path strings.
    pub(crate) fn census(&self) -> crate::engine::census::Tally {
        let mut t = crate::engine::census::Tally::default();
        t.buf::<((AppRef, String), FileIx)>(self.by_key.len())
            .strs(self.by_key.keys().map(|(_, p)| p));
        t
    }
}
