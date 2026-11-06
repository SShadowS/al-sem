//! cli-b/b1 — the DIGEST CLI command: `alsem digest <ws>`.
//!
//! Ports:
//!   - `src/digest/changed-roots.ts`      → resolve_changed_roots
//!   - `src/digest/digest-query.ts`       → digest_query_full (order:false path)
//!   - `src/contracts/digest.ts`          → project_digest_document + serialize
//!   - `src/cli/digest.ts`                → format_digest_human + run_digest
//!
//! The full-document path adds:
//!   - conditionality (per-path + effect-level)
//!   - transactionContext for COMMIT effects
//!   - unresolved-cone BFS
//!   - guarantees / factId / scopedGuarantees (from ordering engine)
//!   - project_digest_document → JSON envelope (sorted, null-drop, 2-space, trailing \n)
//!   - formatDigestHuman → human text

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::gate::format_json::{
    serialize_normalized_value, sort_and_drop_nulls, sort_keys_keep_nulls,
};
use crate::engine::gate::run::analyzer_diagnostics_of_view;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l4::effect_query_cli::{OwnerIndex, temp_state_str};
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::routine_interner::SolveIx;
use crate::engine::l5::conditionality::EffectConditionality;
use crate::engine::l5::diff_parser::{DiffFileKind, HunkChange, parse_unified_diff};
pub use crate::engine::l5::digest::BoundaryMode;
use crate::engine::l5::digest::{
    DigestEntryResult, compute_digest_effects_cli_in, is_known_temp_state,
};
use crate::engine::l5::literal_reach::ResourceStatus;
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, SnapshotCoverageRecord, SnapshotIdentityTable,
};
use crate::engine::l5::transaction_spans::{SeedKind, TransactionSpan};
use crate::engine::l5::unresolved_cone::{
    DEFAULT_MAX_DEPTH, UnresolvedConeItem, UnresolvedTraversal, unresolved_cone,
};
use crate::engine::line_base::shown_opt;
use crate::engine::session::{Cell, RunKey};
use crate::program::model::workspace::Model;

// ---------------------------------------------------------------------------
// Shared envelope-diagnostics projection
// ---------------------------------------------------------------------------

/// Finishes digest's and prove's default run on a prepared view: reads the
/// envelope diagnostics and the tx map, then takes and drops the run and
/// releases `det`, the relation (`Cell::Rel`) and the ordering facts (#160
/// §4.4.4), each kept when a later job on the view reads it (#277 D6.1).
///
/// Shared so the two cannot desync. The envelope `diagnostics` is the JSON
/// array of the run `key` (each entry `{code: "DIAG-<stage>", message,
/// severity}`); the tx map is read off `det.transaction_spans`. Of the view's
/// cells only `l4` stays alive through snapshot composition. Cross-app, the
/// base still owns the relation (its graph and SCC): releasing `Cell::Rel`
/// drops only the view's `Arc`.
pub(crate) fn finish_default_run(
    cx: &mut ViewCx<'_>,
    key: &RunKey,
) -> (serde_json::Value, HashMap<String, TransactionContext>) {
    let view = cx.view();
    let diagnostics = diagnostics_json(&analyzer_diagnostics_of_view(view, key));
    let tx_ctx_map = tx_context_map(&view.det().transaction_spans);
    drop(cx.take_run(key));
    cx.release_all_but(&[Cell::L4]);
    (diagnostics, tx_ctx_map)
}

fn diagnostics_json(diag_vec: &[crate::engine::l5::registry::Diagnostic]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = diag_vec
        .iter()
        .map(|d| {
            let mut m = serde_json::Map::new();
            m.insert("code".into(), format!("DIAG-{}", d.stage).into());
            m.insert("message".into(), d.message.clone().into());
            m.insert("severity".into(), d.severity.clone().into());
            serde_json::Value::Object(m)
        })
        .collect();
    serde_json::Value::Array(arr)
}

// ---------------------------------------------------------------------------
// Changed-root resolution
// ---------------------------------------------------------------------------

/// The path after a `ws:` scheme: a workspace unit id, the one id form a file
/// input takes (#339).
fn ws_rest(p: &str) -> Option<&str> {
    p.strip_prefix("ws:")
}

/// Strip URI scheme prefix (`ws:`, [`ws_rest`], or `app:<guid>:`) from a
/// snapshot source path.
fn strip_scheme(p: &str) -> String {
    let app_rest = || {
        let rest = p.strip_prefix("app:")?;
        rest.find(':').map(|i| &rest[i + 1..])
    };
    ws_rest(p).or_else(app_rest).unwrap_or(p).replace('\\', "/")
}

/// Normalize an input path for comparison: strip scheme, forward-slash, a
/// lexical clean-up, lowercase. The clean-up drops every `.` segment and every
/// empty segment except the leading empty run of the raw split (at most 2), so
/// `./src/x.al`, `src/./x.al` and `src//x.al` all key as `src/x.al`, while
/// `/src/x.al` and `//srv/x.al` keep their `/` or `//` root. A path made only of
/// slashes keys as `/`, which names no file.
/// `..` is never cancelled (a symlinked `link/..` need not be the parent), and
/// a rooted path stays rooted, so neither matches a workspace file (#76).
fn normalize_input(p: &str) -> String {
    clean_segments(&strip_scheme(p)).to_lowercase()
}

/// The workspace-file keys a changed path may name under `root` (#77), as
/// written then below the git top level: [`normalize_input`]'s spelling, the
/// key of a workspace `.al` file's `rel_posix` ([`file_key`]). `alsem
/// impact`'s accounting reads paths exactly as the changed roots are read.
pub(crate) fn path_keys(root: &DiffRoot, path: &str) -> Vec<String> {
    path_readings(root, path).collect()
}

/// The key a workspace file (`rel_posix`) or a span's unit is read by.
pub(crate) fn file_key(path: &str) -> String {
    normalize_input(path)
}

/// Whether a diff path looks like an AL file (#353).
pub(crate) fn is_al_path(path: &str) -> bool {
    al_looking(path)
}

/// [`normalize_input`]'s lexical clean-up of a `/`-separated path, case kept
/// and with no scheme stripping: a file-system folder may be named `ws:Cloud`
/// (#77), so a [`DiffRoot`] prefix and the folders compared with it use this.
fn clean_segments(path: &str) -> String {
    let root = path.split('/').take_while(|s| s.is_empty()).count().min(2);
    let kept: Vec<&str> = std::iter::repeat_n("", root)
        .chain(path.split('/').filter(|s| !s.is_empty() && *s != "."))
        .collect();
    kept.join("/")
}

// ---------------------------------------------------------------------------
// --changed alias auto-detection (port of cli/digest.ts autoDetectChanged) (#12)
// ---------------------------------------------------------------------------

/// The category a `--changed <value>` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedAutoDetect {
    /// Existing file path → treat as --diff.
    Diff(String),
    /// Comma-list with any `.al` entry → treat as --changed-files.
    Files(Vec<String>),
    /// Else → treat as --changed-routines.
    Routines(Vec<String>),
}

/// `autoDetectChanged` (cli/digest.ts). A non-existent path that is not a `.al`
/// comma-list falls through to a routine selector (graceful miscategorization,
/// NOT an OS error). `exists` is injected so this stays a pure, testable function.
pub fn auto_detect_changed_with(value: &str, exists: impl Fn(&str) -> bool) -> ChangedAutoDetect {
    if exists(value) {
        return ChangedAutoDetect::Diff(value.to_string());
    }
    let parts: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.iter().any(|p| p.to_lowercase().ends_with(".al")) {
        ChangedAutoDetect::Files(parts)
    } else {
        ChangedAutoDetect::Routines(parts)
    }
}

/// Convenience wrapper that probes the real filesystem.
pub fn auto_detect_changed(value: &str) -> ChangedAutoDetect {
    auto_detect_changed_with(value, |p| std::path::Path::new(p).exists())
}

// ---------------------------------------------------------------------------
// Routine selector resolution (port of fingerprint-query.ts resolveSelector +
// indexes.ts normalizeDisplayKey / displayToStableIds). 5-form cascade. (#6)
//
// The index-builder and resolver live in `selector_index` — shared with
// fingerprint_query so the two CLIs cannot re-diverge on selector semantics
// or ordering (see that module's docs).
// ---------------------------------------------------------------------------

use crate::engine::l5::selector_index::{
    build_selector_indexes, narrow_to_owner, resolve_selector,
};

/// Changed-roots diagnostics (mirrors ChangedRootsDiagnostic).
#[derive(Debug, Clone)]
pub enum ChangedRootsDiagnostic {
    FileUnmatched {
        file: String,
    },
    /// A file input that names a workspace file both as workspace-relative and
    /// as git-top-relative (#339); it gives no roots.
    FileAmbiguous {
        file: String,
        candidates: Vec<String>,
    },
    SelectorUnmatched {
        selector: String,
        /// V3 §2.4: the nearest workspace names, or where the name is found
        /// only in dependency code.
        hints: crate::engine::l5::selector_index::SelectorHints,
    },
    SelectorAmbiguous {
        selector: String,
        candidates: Vec<String>,
        /// Output step 1b (fable #7): per candidate, the selector that names
        /// it (owner, routine and parameter types), where it is declared and
        /// its stable id; JSON `candidateRoutines`. Empty when the model has
        /// no row for a candidate.
        routines: Vec<String>,
        /// V3 §2.4: per candidate, its display with its complete parameter
        /// list, its declaration and the selector that picks it alone (JSON
        /// `candidateSelectors`). Empty when the model has no row for a
        /// candidate.
        listed: Vec<crate::engine::l5::selector_index::AmbiguousCandidate>,
    },
    DiffFileUnmatched {
        file: String,
    },
    /// A diff path that names a workspace file both as workspace-relative and
    /// as git-top-relative (#77); it gives no roots.
    DiffFileAmbiguous {
        file: String,
        candidates: Vec<String>,
    },
    HunkOutsideRoutines {
        file: String,
        start_line: i32,
        end_line: i32,
    },
    DiffFileDeleted {
        file: String,
    },
    /// A diff path that does not look like an AL file (#353): no roots.
    DiffFileNotAl {
        file: String,
    },
    DiffParseError {
        /// The file the error belongs to (a hunk-header error, #72); `None`
        /// for a truncated hunk or a header error whose section names no path.
        file: Option<String>,
        detail: String,
    },
}

impl ChangedRootsDiagnostic {
    /// The `kind` string, shared by the JSON and the human output (#340).
    fn kind(&self) -> &'static str {
        match self {
            Self::FileUnmatched { .. } => "file-unmatched",
            Self::FileAmbiguous { .. } => "file-ambiguous",
            Self::SelectorUnmatched { .. } => "selector-unmatched",
            Self::SelectorAmbiguous { .. } => "selector-ambiguous",
            Self::DiffFileUnmatched { .. } => "diff-file-unmatched",
            Self::DiffFileAmbiguous { .. } => "diff-file-ambiguous",
            Self::HunkOutsideRoutines { .. } => "hunk-outside-routines",
            Self::DiffFileDeleted { .. } => "diff-file-deleted",
            Self::DiffFileNotAl { .. } => "diff-file-not-al",
            Self::DiffParseError { .. } => "diff-parse-error",
        }
    }
}

pub struct ChangedRootsResult {
    pub roots: Vec<String>,
    pub diagnostics: Vec<ChangedRootsDiagnostic>,
    /// Per root id: indices into `spans.spans` of the declarations the input
    /// touched (the entry anchor is the last of them, #71).
    pub touched: HashMap<String, BTreeSet<usize>>,
    pub spans: RoutineSpanIndex,
}

pub struct ChangedInput<'a> {
    pub files: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub diff_text: Option<String>,
    pub diff_root: DiffRoot,
    /// The workspace's `.al` files (`WorkspaceInputs::al_files`): which
    /// readings of a changed path (a diff's or a file input's) name a FILE,
    /// routines or not (#77, #339).
    pub workspace_files: &'a [crate::program::inputs::WsFile],
}

/// What changed paths (a diff's and the file inputs, #77, #339) are relative
/// to, held as the workspace's path below that directory, `/`-joined with its
/// case kept (`Cloud`, never empty: a root that IS the workspace is
/// [`DiffRoot::Workspace`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DiffRoot {
    /// Paths are workspace-relative.
    #[default]
    Workspace,
    /// The enclosing git top level, found: a path reads either way, and one
    /// naming a workspace file both ways is ambiguous.
    GitTop(String),
    /// `--diff-root`: every path is relative to it.
    Explicit(String),
}

impl DiffRoot {
    /// The workspace's path below the root as `rel`'s `/`-joined components.
    pub fn below(rel: &std::path::Path, explicit: bool) -> DiffRoot {
        let prefix = clean_segments(
            &rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        );
        match (prefix.is_empty(), explicit) {
            (true, _) => DiffRoot::Workspace,
            (false, false) => DiffRoot::GitTop(prefix),
            (false, true) => DiffRoot::Explicit(prefix),
        }
    }
}

/// Whether two folder names name one folder: exactly equal, case included, on
/// every platform. Platform is not file-system identity (case-sensitive APFS
/// and NTFS folders exist), and an exact match can only leave a path
/// unmatched, never read it as a case-distinct sibling (#77).
fn same_folder(a: &str, b: &str) -> bool {
    a == b
}

/// The workspace key `path` names below `prefix`: its leading folders must be
/// `prefix`'s ([`same_folder`]), and only the rest keys like
/// [`normalize_input`]. `None` when it does not start with `prefix`. The
/// folders are file-system names, so no scheme is stripped ([`clean_segments`]).
fn strip_diff_prefix(prefix: &str, path: &str) -> Option<String> {
    let clean = clean_segments(&path.replace('\\', "/"));
    let mut parts = clean.split('/');
    for want in prefix.split('/') {
        if !same_folder(parts.next()?, want) {
            return None;
        }
    }
    let rest = parts.collect::<Vec<_>>().join("/");
    (!rest.is_empty()).then(|| rest.to_lowercase())
}

/// The workspace's `.al` files by [`normalize_input`] key, borrowing each
/// file's `rel_posix` (several spellings may share a key).
type FileKeys<'a> = HashMap<String, Vec<&'a str>>;

/// [`FileKeys`] of `files`, built at most once per [`resolve_changed_roots`]
/// call (only when a file input or a diff is read) and read by both steps.
fn workspace_file_keys(files: &[crate::program::inputs::WsFile]) -> FileKeys<'_> {
    let mut by_key = FileKeys::new();
    for f in files {
        by_key
            .entry(normalize_input(&f.rel_posix))
            .or_default()
            .push(&f.rel_posix);
    }
    by_key
}

/// The keys of the workspace FILES `path` names under `root` (#77), in order:
/// as written, then below the git top level. A file counts whether or not it
/// holds a routine, so a table-only reading still makes a path ambiguous.
fn diff_readings(files: &FileKeys<'_>, root: &DiffRoot, path: &str) -> Vec<String> {
    path_readings(root, path)
        .filter(|k| files.contains_key(k))
        .collect()
}

/// Every key `path` may name under `root`, file or not: as written, then below
/// the git top level.
fn path_readings(root: &DiffRoot, path: &str) -> impl Iterator<Item = String> {
    let as_written = || Some(normalize_input(path));
    let readings = match root {
        DiffRoot::Workspace => [as_written(), None],
        DiffRoot::Explicit(p) => [strip_diff_prefix(p, path), None],
        DiffRoot::GitTop(p) => [as_written(), strip_diff_prefix(p, path)],
    };
    readings.into_iter().flatten()
}

/// Every folder key that holds a workspace file (`src` for `src/x.al`), built
/// once per diff run, and only when an `Only in` path needs it (#353).
fn workspace_folders(files: &FileKeys<'_>) -> std::collections::HashSet<String> {
    let mut folders = std::collections::HashSet::new();
    for key in files.keys() {
        let mut end = key.len();
        while let Some(i) = key[..end].rfind('/') {
            if !folders.insert(key[..i].to_string()) {
                break; // its ancestors are in already
            }
            end = i;
        }
    }
    folders
}

/// Whether a workspace file lies below `path`, an `Only in` path that may be a
/// folder (#353): a reading is a workspace folder, or `path` is the diff
/// root's prefix or a folder above it (no reading exists there) and the
/// workspace has a file.
fn holds_workspace_files(
    folders: &std::collections::HashSet<String>,
    any_file: bool,
    root: &DiffRoot,
    path: &str,
) -> bool {
    path_readings(root, path).any(|r| folders.contains(&r))
        || (any_file && at_or_above_prefix(root, path))
}

/// Whether `path`'s folders are the first folders of the diff root's prefix
/// ([`same_folder`]): it is the prefix or a folder above it.
fn at_or_above_prefix(root: &DiffRoot, path: &str) -> bool {
    let (DiffRoot::GitTop(prefix) | DiffRoot::Explicit(prefix)) = root else {
        return false;
    };
    let mut want = prefix.split('/');
    clean_segments(&path.replace('\\', "/"))
        .split('/')
        .all(|seg| want.next().is_some_and(|w| same_folder(seg, w)))
}

/// Whether a diff path looks like an AL file (#353), as workspace discovery reads one.
fn al_looking(path: &str) -> bool {
    crate::source_text::has_al_extension(std::path::Path::new(path))
}

/// What a changed path (a diff's or a file input's) names under a [`DiffRoot`].
#[derive(Debug, PartialEq, Eq)]
enum PathMatch {
    /// One reading, a file holding a routine: its span-index key.
    One(String),
    /// Two readings: every unit of both (`ws:<rel>`), each reading's sorted, as
    /// written first.
    Ambiguous(Vec<String>),
    /// No reading, or one whose file holds no routine.
    None,
}

/// What `path` names under `root` (#77, #339). One reading: that file,
/// [`PathMatch::None`] when it holds no routine. Two: ambiguous, decided by the
/// workspace FILES ([`diff_readings`]).
fn match_path(
    spans: &RoutineSpanIndex,
    files: &FileKeys<'_>,
    root: &DiffRoot,
    path: &str,
) -> PathMatch {
    let readings = diff_readings(files, root, path);
    // A reading's units (`ws:<rel>`), sorted.
    let units = |key: &String| {
        let mut u: Vec<String> = files[key].iter().map(|f| format!("ws:{f}")).collect();
        u.sort();
        u.dedup();
        u
    };
    match &readings[..] {
        [key] if !spans.of_file(key).is_empty() => PathMatch::One(key.clone()),
        [_, _] => PathMatch::Ambiguous(readings.iter().flat_map(units).collect()),
        _ => PathMatch::None,
    }
}

/// The span-index key a diff path names under `root` ([`match_path`]), or
/// `diff-file-ambiguous` / `diff-file-unmatched`.
fn diff_key(
    spans: &RoutineSpanIndex,
    files: &FileKeys<'_>,
    root: &DiffRoot,
    path: &str,
) -> Result<String, ChangedRootsDiagnostic> {
    let file = path.to_string();
    match match_path(spans, files, root, path) {
        PathMatch::One(key) => Ok(key),
        PathMatch::Ambiguous(candidates) => {
            Err(ChangedRootsDiagnostic::DiffFileAmbiguous { file, candidates })
        }
        PathMatch::None => Err(ChangedRootsDiagnostic::DiffFileUnmatched { file }),
    }
}

/// The declarations (indices into `spans.spans`) a file input names under
/// `root` (#339), or `file-ambiguous` / `file-unmatched`. A `ws:` id names one
/// workspace FILE exactly (case kept, after the `\` and segment clean-up) under
/// every root, so a candidate copied from `file-ambiguous` picks its one file;
/// one holding no routine is `file-unmatched`. An id naming no workspace file
/// is read as a path ([`match_path`]): a folder may be named `ws:Cloud` (#77).
/// Every other input, `app:<guid>:…` included, is a path.
fn file_hits(
    spans: &RoutineSpanIndex,
    files: &FileKeys<'_>,
    root: &DiffRoot,
    path: &str,
) -> Result<Vec<usize>, ChangedRootsDiagnostic> {
    let file = path.to_string();
    if let Some(rest) = ws_rest(path) {
        let clean = |p: &str| clean_segments(&p.replace('\\', "/"));
        let exact = clean(rest);
        let key = normalize_input(path);
        if files
            .get(&key)
            .is_some_and(|fs| fs.iter().any(|f| clean(f) == exact))
        {
            let unit = format!("ws:{exact}");
            let ixs: Vec<usize> = spans
                .of_file(&key)
                .iter()
                .copied()
                .filter(|&i| spans.spans[i].source_unit_id == unit)
                .collect();
            if ixs.is_empty() {
                return Err(ChangedRootsDiagnostic::FileUnmatched { file });
            }
            return Ok(ixs);
        }
    }
    match match_path(spans, files, root, path) {
        PathMatch::One(key) => Ok(spans.of_file(&key).to_vec()),
        PathMatch::Ambiguous(candidates) => {
            Err(ChangedRootsDiagnostic::FileAmbiguous { file, candidates })
        }
        PathMatch::None => Err(ChangedRootsDiagnostic::FileUnmatched { file }),
    }
}

/// One routine DECLARATION's span (#71). Lines are 0-based and inclusive.
#[derive(Debug, Clone)]
pub struct RoutineSpan {
    /// Stable routine id.
    pub routine: String,
    /// The ORIGINAL unit, e.g. `ws:src/posting.al` (case kept), for the anchor.
    pub source_unit_id: String,
    /// First line of the declaration: the routine node or its first attribute.
    pub start: u32,
    /// Last line of the routine node.
    pub end: u32,
    /// `source_anchor.start_line`.
    pub anchor_line: u32,
    /// `source_anchor.start_column` (UTF-8 bytes).
    pub anchor_column: u32,
    /// The procedure name, set only for Interface / ControlAddIn procedure
    /// declarations.
    pub declaration_only_display: Option<String>,
}

/// Every workspace routine declaration's span, with lookups by normalized file
/// and by routine id (#71).
#[derive(Debug, Clone, Default)]
pub struct RoutineSpanIndex {
    /// One per declaration (never collapsed by id), in document order.
    pub spans: Vec<RoutineSpan>,
    by_file: HashMap<String, Vec<usize>>,
    by_routine: HashMap<String, Vec<usize>>,
}

impl RoutineSpan {
    /// Does a diff change touch this span? `Added(n)`: n in the span (1-based).
    /// `DeletedAfter(l)`: the gap between new lines l and l+1 is INTERIOR, so
    /// deleting a whole routine, or blank lines between routines, blames no one.
    /// `DeletedAttributesAfter(l)`: also the gap RIGHT ABOVE the span (`l + 1 ==
    /// first`), since deleted attribute lines there were the routine's own
    /// (deleting its first or only attribute).
    pub(crate) fn touched_by(&self, c: &HunkChange) -> bool {
        let (first, last) = (i64::from(self.start) + 1, i64::from(self.end) + 1);
        match *c {
            HunkChange::Added(n) => (first..=last).contains(&i64::from(n)),
            HunkChange::DeletedAfter(l) => first <= i64::from(l) && i64::from(l) < last,
            HunkChange::DeletedAttributesAfter(l) => {
                first - 1 <= i64::from(l) && i64::from(l) < last
            }
        }
    }

    fn anchor(&self) -> FullAnchor {
        FullAnchor {
            source_kind: "source",
            file: Some(self.source_unit_id.clone()),
            line: Some(self.anchor_line),
            column: Some(self.anchor_column),
            excerpt: None,
        }
    }
}

impl RoutineSpanIndex {
    /// An index over `spans` as given (already in document order per unit):
    /// a test's hand-stated spans.
    #[cfg(test)]
    pub(crate) fn from_spans(spans: Vec<RoutineSpan>) -> RoutineSpanIndex {
        RoutineSpanIndex {
            spans,
            ..RoutineSpanIndex::default()
        }
    }

    fn of_file(&self, normalized: &str) -> &[usize] {
        self.by_file.get(normalized).map_or(&[], Vec::as_slice)
    }

    fn of_routine(&self, id: &str) -> &[usize] {
        self.by_routine.get(id).map_or(&[], Vec::as_slice)
    }

    /// The declarations among `ixs` (one file's, from [`Self::of_file`]) that any
    /// of `changes` touches, possibly repeated. Per change a binary search, not a
    /// scan: within one unit `ixs` is in document order and spans never nest
    /// (the arms of an `#if`-split header share one span), so `end` ascends too.
    /// A normalized key can hold two units (paths differing in case only), so
    /// each unit's run is searched on its own.
    fn touched_in(&self, ixs: &[usize], changes: &[HunkChange]) -> Vec<usize> {
        let span = |i: usize| &self.spans[i];
        let mut hit = Vec::new();
        for unit in ixs.chunk_by(|&a, &b| span(a).source_unit_id == span(b).source_unit_id) {
            for c in changes {
                let n = i64::from(c.line());
                // Every touched span has `end + 1 >= n` and `start <= n`.
                let from = unit.partition_point(|&i| i64::from(span(i).end) + 1 < n);
                hit.extend(
                    unit[from..]
                        .iter()
                        .copied()
                        .take_while(|&i| i64::from(span(i).start) <= n)
                        .filter(|&i| span(i).touched_by(c)),
                );
            }
        }
        hit
    }
}

/// One span per routine DECLARATION (never collapsed by id), in document order,
/// from the model's workspace routines and the Interface / ControlAddIn
/// declaration-only procedures on its object rows. Only `ws:` units: dependency
/// source routines are `dep:<guid>:<path>`, ABI-only ones have an empty unit.
pub fn build_routine_span_index(resolved: &Model) -> RoutineSpanIndex {
    let model_routines = resolved.workspace.routines.iter().map(|r| RoutineSpan {
        routine: r.stable_routine_id.clone(),
        source_unit_id: r.source_anchor.source_unit_id.clone(),
        start: r.declaration_start_line,
        end: r.source_anchor.end_line,
        anchor_line: r.source_anchor.start_line,
        anchor_column: r.source_anchor.start_column,
        declaration_only_display: None,
    });
    let declaration_only = resolved
        .workspace
        .objects
        .iter()
        .flat_map(|o| &o.declaration_only_routines)
        .map(|d| RoutineSpan {
            routine: d.stable_routine_id.clone(),
            source_unit_id: d.source_anchor.source_unit_id.clone(),
            start: d.declaration_start_line,
            end: d.source_anchor.end_line,
            anchor_line: d.source_anchor.start_line,
            anchor_column: d.source_anchor.start_column,
            declaration_only_display: Some(d.name.clone()),
        });
    let mut spans: Vec<RoutineSpan> = model_routines
        .chain(declaration_only)
        .filter(|s| s.source_unit_id.starts_with("ws:"))
        .collect();
    // Stable: the arms of an `#if`-split header share one origin and keep model order.
    spans.sort_by(|a, b| {
        (&a.source_unit_id, a.anchor_line, a.anchor_column).cmp(&(
            &b.source_unit_id,
            b.anchor_line,
            b.anchor_column,
        ))
    });
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_routine: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in spans.iter().enumerate() {
        by_file
            .entry(normalize_input(&s.source_unit_id))
            .or_default()
            .push(i);
        by_routine.entry(s.routine.clone()).or_default().push(i);
    }
    RoutineSpanIndex {
        spans,
        by_file,
        by_routine,
    }
}

/// An entry's routine as a header and a summary row name it: the one
/// routine display (V3 §2.1, with the member's caption), else (the model
/// has no row for it) its owner-qualified name.
fn entry_display(
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    e: &DigestEntryFull,
) -> String {
    cx.names
        .display_with(&e.routine_id, true)
        .unwrap_or_else(|| {
            crate::engine::l4::effect_query_cli::owner_qualified(
                &e.object_display,
                &e.routine_display,
            )
        })
}

/// The workspace app's member triggers as a member-form selector matches
/// them (V3 §2.1, [`crate::engine::l5::selector_index::resolve_member_selector`]),
/// owners from `owners` (the model's, #78).
pub(crate) fn member_rows<'m>(
    resolved: &'m Model,
    owners: &'m OwnerIndex<'_>,
) -> impl Iterator<Item = crate::engine::l5::selector_index::MemberRow<'m>> + 'm {
    resolved
        .workspace
        .routines
        .iter()
        .filter(move |r| resolved.routine_role(r) == crate::program::model::role::Role::Primary)
        .filter_map(move |r| {
            let member = r.enclosing_member.as_deref()?;
            Some(crate::engine::l5::selector_index::MemberRow {
                id: &r.stable_routine_id,
                owner_display: owners.object_display(&r.stable_routine_id),
                member,
                member_kind: r.member_detail.as_ref().map_or_else(
                    || {
                        crate::engine::l5::routine_rollup::infer_member_kind(
                            &r.object_type,
                            &r.name,
                        )
                    },
                    |d| d.kind,
                ),
                routine: &r.name,
            })
        })
}

/// The model's primary app guid, the routine-selector scope of
/// [`resolve_changed_roots`].
pub fn primary_guid_of(resolved: &Model) -> Option<&str> {
    resolved.primary_app.as_ref().map(|a| a.app_guid.as_str())
}

/// `primary_app_guid` scopes routine selectors to the workspace app's routines: a
/// cross-app identity table (#66) also holds dependency routines. Files are
/// scoped by the span index's `ws:` filter ([`build_routine_span_index`]). Only
/// the routine selectors read `identities` (the model's, `derive_identity_table`,
/// built before digest's composition, #160 step 6 D3).
///
/// Files and diffs resolve by routine declaration span (#71); the result also
/// carries, per root, the declarations the input touched. `owners` narrows an
/// `<owner>::<routine>` selector and labels ambiguity candidates (#78).
pub fn resolve_changed_roots(
    identities: &SnapshotIdentityTable,
    resolved: &Model,
    input: &ChangedInput<'_>,
    primary_app_guid: Option<&str>,
    owners: &OwnerIndex<'_>,
) -> ChangedRootsResult {
    let mut roots: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut diagnostics: Vec<ChangedRootsDiagnostic> = Vec::new();

    let spans = build_routine_span_index(resolved);
    // Touched declaration indices; each one's id is a root (resolved below).
    let mut hits: Vec<usize> = Vec::new();

    // Selector indexes (5-form resolveSelector cascade).
    let selector_idx = build_selector_indexes(identities, primary_app_guid);

    // Both the file step and the diff step read paths by these (#339); built
    // on first use, so a routine-only run never builds them.
    let files = std::cell::LazyCell::new(|| workspace_file_keys(input.workspace_files));

    // 1. File-based matching: a file input reads like a diff path (#339).
    for file in input.files.iter().flatten() {
        match file_hits(&spans, &files, &input.diff_root, file) {
            Ok(ixs) => hits.extend(ixs),
            Err(d) => diagnostics.push(d),
        }
    }

    // V3 §2.4: the one display's names, for a failed selector's hints and
    // candidates; built on first need.
    let names = std::cell::LazyCell::new(|| {
        crate::engine::l5::routine_rollup::RoutineNames::new(resolved, &[])
    });

    // Output step 1b: stable id -> model row, for a selector's parameter list
    // and an ambiguity's candidate labels; built on first need.
    let rows = std::cell::LazyCell::new(|| {
        let mut m: HashMap<&str, &crate::program::model::workspace::ModelRoutine> = HashMap::new();
        for r in &resolved.workspace.routines {
            m.entry(r.stable_routine_id.as_str()).or_insert(r);
        }
        m
    });

    // 2. Routine selector matching (port of resolveSelector — 5-form cascade),
    // then (step 1b) an `(…)` parameter list picks among overloads.
    for selector in input.routines.iter().flatten() {
        let (base, wanted) = crate::engine::l5::selector_index::split_signature(selector);
        let (matches, matched_form) = resolve_selector(base, &selector_idx);
        let mut matches =
            narrow_to_owner(base, matched_form, matches, |id| owners.object_display(id));
        if let Some(wanted) = &wanted {
            matches.retain(|id| {
                rows.get(id.as_str()).is_some_and(|r| {
                    crate::engine::l5::selector_index::signature_matches(wanted, &r.parameters)
                })
            });
        }
        // V3 §2.1: a member form (`Object."Field"::Trigger`,
        // `Object.action(Name)::OnAction`), the display of a member trigger,
        // when nothing above matched (so no selector that resolved before
        // resolves differently).
        if matches.is_empty()
            && let Some(ids) = crate::engine::l5::selector_index::resolve_member_selector(
                base,
                member_rows(resolved, owners),
            )
        {
            matches = ids;
        }
        if matches.is_empty() {
            diagnostics.push(ChangedRootsDiagnostic::SelectorUnmatched {
                selector: selector.clone(),
                hints: crate::engine::l5::selector_index::selector_hints(
                    &names,
                    |id| owners.object_display(id),
                    selector,
                ),
            });
        } else if matches.len() >= 2 {
            // candidates = `<owner>::<display>` (display falls back to the id)
            // — deterministic (matches preserve identity-table / bucket order).
            let candidates: Vec<String> = matches
                .iter()
                .map(|id| {
                    let display = selector_idx
                        .routine_display_by_id
                        .get(id)
                        .map_or(id.as_str(), String::as_str);
                    owners.qualified(id, display)
                })
                .collect();
            let routines: Vec<String> = matches
                .iter()
                .zip(&candidates)
                .filter_map(|(id, label)| {
                    rows.get(id.as_str())
                        .map(|r| crate::engine::l5::selector_index::candidate_label(label, r))
                })
                .collect();
            let listed: Vec<crate::engine::l5::selector_index::AmbiguousCandidate> = matches
                .iter()
                .filter_map(|id| {
                    crate::engine::l5::selector_index::AmbiguousCandidate::of(&names, id)
                })
                .collect();
            diagnostics.push(ChangedRootsDiagnostic::SelectorAmbiguous {
                selector: selector.clone(),
                routines: if routines.len() == candidates.len() {
                    routines
                } else {
                    Vec::new()
                },
                listed: if listed.len() == candidates.len() {
                    listed
                } else {
                    Vec::new()
                },
                candidates,
            });
        } else {
            // Every declaration of the id is touched (a selector names no line).
            let id = matches.into_iter().next().unwrap();
            hits.extend_from_slice(spans.of_routine(&id));
            roots.insert(id);
        }
    }

    // 3. Unified diff matching
    if let Some(ref diff_text) = input.diff_text
        && !diff_text.trim().is_empty()
    {
        let parsed = parse_unified_diff(diff_text);
        for err in &parsed.errors {
            diagnostics.push(ChangedRootsDiagnostic::DiffParseError {
                file: None,
                detail: err.clone(),
            });
        }
        for fe in &parsed.file_errors {
            diagnostics.push(ChangedRootsDiagnostic::DiffParseError {
                file: Some(fe.path.clone()),
                detail: fe.detail.clone(),
            });
        }
        // A file with a hunk-header error gives no diff-derived roots, in ANY of
        // its records: the dropped hunk's changes are unknown, so whole-file roots
        // would hide the error (#72). Keyed by every file the errored path may
        // name (#77): an ambiguous one vetoes both, as its changes are unknown
        // for either.
        let poisoned: std::collections::HashSet<String> = parsed
            .file_errors
            .iter()
            .flat_map(|fe| diff_readings(&files, &input.diff_root, &fe.path))
            .collect();
        for diff_file in &parsed.files {
            // #353: a record whose path (a deletion's `---` path, else the new
            // one) is not AL-looking makes no roots. Renamed away from AL, it
            // deleted AL source; a copy to a non-AL name (also with an
            // `old_path`) deleted nothing. A deletion with no `---` path (the
            // `unknown` placeholder) stays `diff-file-deleted`.
            let placeholder =
                diff_file.kind == DiffFileKind::Deleted && diff_file.old_path.is_none();
            if !placeholder && !al_looking(&diff_file.path) {
                diagnostics.push(match diff_file.old_path.as_deref() {
                    Some(old) if diff_file.kind == DiffFileKind::Renamed && al_looking(old) => {
                        ChangedRootsDiagnostic::DiffFileDeleted {
                            file: old.to_string(),
                        }
                    }
                    _ => ChangedRootsDiagnostic::DiffFileNotAl {
                        file: diff_file.path.clone(),
                    },
                });
                continue;
            }
            if diff_file.kind == DiffFileKind::Deleted {
                diagnostics.push(ChangedRootsDiagnostic::DiffFileDeleted {
                    file: diff_file.path.clone(),
                });
                continue;
            }
            let key = match diff_key(&spans, &files, &input.diff_root, &diff_file.path) {
                Ok(key) => key,
                Err(d) => {
                    diagnostics.push(d);
                    continue;
                }
            };
            let ixs = spans.of_file(&key);
            if poisoned.contains(&key) {
                continue;
            }

            if diff_file.hunks.is_empty() {
                // Rename (or mode change) with no content — every declaration. A
                // file whose hunk header was rejected never gets here (poisoned above).
                hits.extend_from_slice(ixs);
                continue;
            }

            for hunk in &diff_file.hunks {
                // Only the hunk's CHANGED lines count; context never makes a root.
                let hit = spans.touched_in(ixs, &hunk.changes);
                if hit.is_empty() {
                    // endLine = newStart + max(newCount - 1, 0). When newCount == 0
                    // (pure deletion hunk) TS yields newStart, NOT newStart - 1 (#14).
                    diagnostics.push(ChangedRootsDiagnostic::HunkOutsideRoutines {
                        file: diff_file.path.clone(),
                        start_line: hunk.new_start,
                        end_line: hunk.new_start + (hunk.new_count - 1).max(0),
                    });
                }
                hits.extend(hit);
            }
        }
        // `diff -r`'s `Only in` files carry no hunk (#111). One that is a workspace
        // file (the new tree added it) is a whole-file hit, like a hunkless rename;
        // any other is reported, never dropped silently.
        let mut folders = None;
        for file in &parsed.only_in {
            // #353: a folder of workspace files keeps `diff-file-unmatched` (its
            // roots are lost, a stated limit).
            if !al_looking(file)
                && !holds_workspace_files(
                    folders.get_or_insert_with(|| workspace_folders(&files)),
                    !files.is_empty(),
                    &input.diff_root,
                    file,
                )
            {
                diagnostics.push(ChangedRootsDiagnostic::DiffFileNotAl { file: file.clone() });
                continue;
            }
            match diff_key(&spans, &files, &input.diff_root, file) {
                Err(d) => diagnostics.push(d),
                Ok(key) if !poisoned.contains(&key) => hits.extend_from_slice(spans.of_file(&key)),
                Ok(_) => {}
            }
        }
    }

    // Touched sets of one id from several inputs are unioned.
    let mut touched: HashMap<String, BTreeSet<usize>> = HashMap::new();
    for i in hits {
        let id = &spans.spans[i].routine;
        roots.insert(id.clone());
        touched.entry(id.clone()).or_default().insert(i);
    }

    // Sort + dedup
    let mut sorted: Vec<String> = roots.into_iter().collect();
    sorted.sort();

    ChangedRootsResult {
        roots: sorted,
        diagnostics,
        touched,
        spans,
    }
}

// ---------------------------------------------------------------------------
// Transaction context computation
// ---------------------------------------------------------------------------

pub type TransactionContext = &'static str;
const TX_SPAN_KNOWN_WRITES: TransactionContext = "span-has-known-writes";
const TX_SPAN_NO_WRITES: TransactionContext = "span-has-no-known-writes";
const TX_UNKNOWN: TransactionContext = "unknown";

/// The transaction-context map: `commit_operation_id → TransactionContext`, over
/// the spans of a `det` cell whose run demanded `TRANSACTION_SPANS` (digest and
/// prove's plan asserts it, #160 A2.10).
/// Mirrors TS: `computeTransactionSpans` + `transactionContextByOperationId` build.
/// "Known writes" are the physical writes PENDING at the Commit
/// (`pending_physical_tables`): a write after the Commit, or in a branch that
/// does not reach it, is not in the transaction it ends, and a temporary
/// record is in no transaction.
fn tx_context_map(spans: &[TransactionSpan]) -> HashMap<String, TransactionContext> {
    let mut map: HashMap<String, TransactionContext> = HashMap::new();
    for span in spans {
        if span.seed_kind != SeedKind::ExplicitCommit {
            continue;
        }
        let tc: TransactionContext = if !span.coverage_complete {
            TX_UNKNOWN
        } else if !span.pending_physical_tables.is_empty() {
            TX_SPAN_KNOWN_WRITES
        } else {
            TX_SPAN_NO_WRITES
        };
        map.insert(span.commit_operation_id.clone(), tc);
    }
    map
}

// ---------------------------------------------------------------------------
// Full digest query — extends digest_query with conditionality, tx-context,
// unresolved-cone, and guarantees.
// ---------------------------------------------------------------------------

pub struct DigestEntryFull {
    pub routine_id: String,
    pub routine_display: String,
    pub object_display: String,
    pub routine_anchor: Option<FullAnchor>,
    /// Every declaration's anchor in document order, for an id with more than one
    /// declaration (two `#if` arms); empty otherwise (#71).
    pub routine_anchors: Vec<FullAnchor>,
    pub coverage_status: String,
    pub coverage_reasons: Vec<String>,
    pub effects: Vec<DigestEffectFull>,
    pub unresolved: Vec<UnresolvedConeItem>,
    pub unresolved_traversal: UnresolvedTraversal,
    pub entry_diagnostics: Vec<EntryDiagnostic>,
    /// An Interface / ControlAddIn procedure declaration (#71): no body, no effects.
    pub declaration_only: bool,
}

pub struct DigestEffectFull {
    pub effect_type: String,
    pub detail: Vec<(String, String)>,
    pub provenance: &'static str,
    pub evidence: FullAnchor,
    pub via_paths: Vec<Vec<crate::engine::l5::digest::ProjectedHop>>,
    pub via_paths_truncated: bool,
    pub conditionality: EffectConditionality,
    pub transaction_context: TransactionContext,
    pub guarantees: Vec<String>,
    pub fact_id: String,
    pub scoped_guarantees: Vec<crate::engine::l5::ordering_engine::ScopedGuarantee>,
    /// A boundary group's full `(resource id, display)` set (#160 step 9
    /// D2 (6)); `detail` lists at most 10. Not serialized; empty off a group.
    pub group_resources: Vec<(String, String)>,
    /// G35: per group resource id, one example path inside the dependency
    /// ([`crate::engine::l5::group_path`]); `prove` shows the matched ones.
    pub group_resource_paths: Vec<(String, crate::engine::l5::group_path::GroupPathOutcome)>,
    /// W2-6: a resource-less group's terminal witness (`DigestEffectResult`'s
    /// field of the same name).
    pub group_terminal_path: Option<crate::engine::l5::group_path::GroupPathOutcome>,
    /// Final CDO output review, item 2: an own write row's witness when it
    /// keeps no listed path (`DigestEffectResult`'s field of the same name).
    pub own_path: Option<crate::engine::l5::group_path::GroupPathOutcome>,
    /// The operations the effect originates from, and its class's "known
    /// temporary" flag: what [`DbTempStates`] joins on, so `prove` can ask the
    /// same write in a CALLER's frame (G25). Not serialized.
    pub origin_operation_ids: Vec<String>,
    pub effect_is_kt: bool,
    /// G23: unreachable from the root once literal Boolean arguments are
    /// followed ([`crate::engine::l5::literal_reach`]); a MAY question does not
    /// count it. Not serialized.
    pub literal_unreachable: bool,
    /// W2-2: a boundary group's crossing (design §3.3); `None` off a group.
    /// The rollup groups rows by it; the 3.x digest serializes it once per
    /// entry (`entries[].crossings`).
    pub crossing: Option<Arc<crate::engine::l5::digest::Crossing>>,
}

pub struct FullAnchor {
    pub source_kind: &'static str, // "source" | "unavailable"
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
}

/// The [`EntryDiagnostic`] kind of an effect type whose site search stopped at
/// the depth bound (`DigestEntryResult::partial_site_types`): its rows may not
/// be every site the routine reaches.
pub const SITES_PARTIAL: &str = "sites-partial";

/// Whether `entry`'s rows of effect type `ty` may miss a site
/// ([`SITES_PARTIAL`]): a count of them is then "at least".
#[must_use]
pub fn sites_partial(entry: &DigestEntryFull, ty: &str) -> bool {
    entry
        .entry_diagnostics
        .iter()
        .any(|d| d.kind == SITES_PARTIAL && d.effect_type.as_deref() == Some(ty))
}

pub struct EntryDiagnostic {
    pub kind: &'static str,
    pub effect_type: Option<String>,
    pub fact_subject: Option<String>,
    /// `temp-state-incoherent` only (#69 D4): the effect's `factId`.
    pub fact_id: Option<String>,
    /// `temp-state-incoherent` only: the originating operation that failed the join.
    pub operation_id: Option<String>,
    /// `temp-state-incoherent` only: `no-l4-row` | `class-absent`.
    pub reason: Option<&'static str>,
}

/// Query-level diagnostics (mirrors `DigestQueryDiagnostic` in digest-query.ts).
/// These feed BOTH the envelope diagnostics channel AND rootsRequested (#5).
#[derive(Debug, Clone)]
pub enum DigestQueryDiagnostic {
    RootNotInSnapshot { routine_id: String },
    NoCoverageRecord { routine_id: String },
}

/// Result of the full digest query — entries plus query-level diagnostics.
pub struct DigestQueryFullResult {
    pub entries: Vec<DigestEntryFull>,
    pub query_diagnostics: Vec<DigestQueryDiagnostic>,
}

/// One `temp-state-incoherent` diagnostic per failed join, carrying the effect's
/// `factId` and the failed operation (#69 D4).
fn incoherence_diagnostics(
    effect_type: &str,
    fact_subject: &str,
    fact_id: &str,
    problems: Vec<(String, JoinProblem)>,
) -> Vec<EntryDiagnostic> {
    problems
        .into_iter()
        .map(|(op, problem)| EntryDiagnostic {
            kind: "temp-state-incoherent",
            effect_type: Some(effect_type.to_string()),
            fact_subject: Some(fact_subject.to_string()),
            fact_id: Some(fact_id.to_string()),
            operation_id: Some(op),
            reason: Some(problem.reason()),
        })
        .collect()
}

/// Why one originating operation failed the temp-state join (#69 D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinProblem {
    /// L4 has no row for the operation.
    NoL4Row,
    /// L4 has rows for the operation, but none in the effect's class.
    ClassAbsent,
}

impl JoinProblem {
    /// The `reason` of the `temp-state-incoherent` entry diagnostic.
    pub fn reason(self) -> &'static str {
        match self {
            JoinProblem::NoL4Row => "no-l4-row",
            JoinProblem::ClassAbsent => "class-absent",
        }
    }
}

/// One DB effect's L4 temp states, joined per originating operation (#69 D4).
#[derive(Debug, Clone, Default)]
pub struct TempJoin {
    pub states: BTreeSet<TempStateKind>,
    pub problems: Vec<(String, JoinProblem)>,
}

impl TempJoin {
    /// The `detail.tempState` field: absent when any operation has no L4 row (never
    /// invent a value) or nothing joined; else the distinct states rendered by
    /// `temp_state_str`, sorted by their text and `,`-joined.
    pub fn render(&self) -> Option<String> {
        if self.states.is_empty()
            || self
                .problems
                .iter()
                .any(|(_, p)| *p == JoinProblem::NoL4Row)
        {
            return None;
        }
        let text: BTreeSet<String> = self.states.iter().map(temp_state_str).collect();
        Some(text.into_iter().collect::<Vec<_>>().join(","))
    }
}

/// D4: per originating operation — no row → `NoL4Row`; rows but none in the
/// effect's class (kt = `known(true)`) → `ClassAbsent`, and ALL its rows join;
/// otherwise only its in-class rows join.
pub fn join_temp_states(
    effect_is_kt: bool,
    op_ids: &[String],
    rows: &[(&str, TempStateKind)],
) -> TempJoin {
    let mut j = TempJoin::default();
    for op in op_ids {
        let mine: Vec<&TempStateKind> = rows
            .iter()
            .filter(|(o, _)| o == op)
            .map(|(_, t)| t)
            .collect();
        if mine.is_empty() {
            j.problems.push((op.clone(), JoinProblem::NoL4Row));
            continue;
        }
        let in_class: Vec<&TempStateKind> = mine
            .iter()
            .copied()
            .filter(|t| (**t == TempStateKind::Known(true)) == effect_is_kt)
            .collect();
        if in_class.is_empty() {
            j.problems.push((op.clone(), JoinProblem::ClassAbsent));
            j.states.extend(mine.into_iter().cloned());
        } else {
            j.states.extend(in_class.into_iter().cloned());
        }
    }
    j
}

/// The L4 db-effect store joined for digest effects (#65): a DB_* effect's
/// `detail.tempState` is read from the SAME store and rendered by the SAME
/// function as `alsem query effects`, filtered per operation to the effect's
/// class (#69 D4, [`join_temp_states`]).
pub struct DbTempStates<'a> {
    bundle: &'a SummaryBundle,
    ix_by_stable: HashMap<&'a str, SolveIx>,
}

impl<'a> DbTempStates<'a> {
    pub fn new(resolved: &'a Model, bundle: &'a SummaryBundle) -> Self {
        let ix_by_stable = resolved
            .workspace
            .routines
            .iter()
            .filter_map(|r| Some((r.stable_routine_id.as_str(), bundle.routine_ix(&r.id)?)))
            .collect();
        DbTempStates {
            bundle,
            ix_by_stable,
        }
    }

    /// The root's L4 rows for these originating operations, joined for an effect
    /// of class `effect_is_kt` (a root L4 does not know has no rows).
    pub(crate) fn for_effect(
        &self,
        root_stable: &str,
        op_ids: &[String],
        effect_is_kt: bool,
    ) -> TempJoin {
        let rows: Vec<(&str, TempStateKind)> = self
            .ix_by_stable
            .get(root_stable)
            .into_iter()
            .flat_map(|&rix| self.bundle.db_effects(rix))
            .map(|e| (e.operation_id, e.temp_state.clone()))
            .collect();
        join_temp_states(effect_is_kt, op_ids, &rows)
    }
}

/// What digest's query reads of its tree (#160 step 6 D7 (5), A1-16): per
/// root its S4 entry and its unresolved cone, plus the tree's identity table
/// and coverage. Computed while the tree is alive; the tree is then released.
///
/// The identity table and the coverage are whole-program and the same for
/// every reader of one tree, so the readers of a shared tree share one copy
/// (G16: one copy per `prove` job, over a program holding Base Application's
/// source, held ~0.2 GB per job until the job ran).
#[derive(Default)]
pub struct DigestEffects {
    pub entries: Vec<DigestEntryResult>,
    pub unresolved: BTreeMap<String, UnresolvedCone>,
    pub coverage: Arc<Vec<SnapshotCoverageRecord>>,
    pub identities: Arc<SnapshotIdentityTable>,
    /// `--tests` (#254): the test-coverage graph over the same tree, one per
    /// tree, shared by every reader that asked for it.
    pub coverage_graph: Option<Arc<crate::engine::l5::test_coverage::CoverageGraph>>,
    /// Agent-tasks E1: a filtered prove's slice of its root, read off the
    /// same tree (`TreeDemand::slice`); `None` for every other reader.
    pub slice: Option<crate::engine::l5::prove_slice::SliceOut>,
    /// The model's background starts by call-site id
    /// (`ResolvedCalls::background_starts`): a `BACKGROUND_TASK` row at a start
    /// names the codeunit it starts (detail `starts`).
    pub background_starts: Arc<HashMap<String, crate::program::model::calls::BackgroundStart>>,
}

/// A root's unresolved cone: its items and the traversal block (depth,
/// truncation, visited count) the digest JSON serializes and prove's
/// obligations read.
pub struct UnresolvedCone {
    pub items: Vec<UnresolvedConeItem>,
    pub traversal: UnresolvedTraversal,
    /// The tree's analysis gaps whose subject the cone visits
    /// ([`count_gaps_in_cone`]), counted before the tree is dropped (#160
    /// step 9 D2 (5)): prove's `analysisGaps` obligation. `None` off prove's
    /// plan: digest never reads it, so it never counts it (final panel C1).
    pub gaps_in_cone: Option<usize>,
    /// The cone visits a routine with a record operation that raises the
    /// platform table events (`Insert`, `Modify`, `Delete`, `ModifyAll`,
    /// `DeleteAll`, `Rename`, `Validate`), which any app may subscribe to:
    /// prove's `loaded-subscribers` label. `None` off prove's plan, as
    /// `gaps_in_cone`.
    pub fires_table_events: Option<bool>,
}

/// Count analysis gaps whose subject intersects the visited cone.
///
/// A gap subject is EITHER:
///   - a StableRoutineId (parse-incomplete / body-unavailable gaps) → DIRECT match
///     against the visited set, OR
///   - a bare app GUID (symbol-only-boundary gaps, snapshot.rs:1860-1867) → matches
///     when ANY visited routine's stable id starts with `{appGuid}:` (stable ids use
///     ":" separators, NOT "/" — see digest-query.ts:722-728's documented soundness
///     fix). WITHOUT this app-level branch every symbol-only-boundary gap is invisible,
///     so a cone entering an opaque `.app` dep reports analysisGaps=0 → a FALSE absence
///     proof. This is the exact class of bug digest-query.ts documents.
fn count_gaps_in_cone(
    gap_subjects: &[&str],
    visited_ids: &std::collections::HashSet<String>,
    known_natives: &std::collections::HashSet<&str>,
) -> usize {
    let mut count = 0usize;
    for subject in gap_subjects {
        // Direct routine-subject match.
        if visited_ids.contains(*subject) {
            count += 1;
            continue;
        }
        // App-level (symbol-only-boundary) match: subject is a bare app GUID and some
        // visited routine has a stable id of the form `{appGuid}:Type:{n}#hash`. A
        // cataloged platform native (`platform_natives`) has known behaviour: it
        // does not put its app's gap in the cone.
        let prefix = format!("{}:", subject);
        if visited_ids
            .iter()
            .any(|v| v.starts_with(&prefix) && !known_natives.contains(v.as_str()))
        {
            count += 1;
        }
    }
    count
}

/// [`DigestEffects`] of `roots` off `tree`, one reader ([`digest_effects_each`]
/// with rule B as the tree states it).
#[cfg(test)]
pub(crate) fn digest_effects(
    tree: &CapabilitySnapshot,
    model: &Model,
    roots: &[String],
    tests: bool,
    gaps_in_cone: bool,
    include_reads: bool,
) -> DigestEffects {
    let mut out = digest_effects_each(
        tree,
        model,
        &[roots],
        tests,
        gaps_in_cone,
        include_reads,
        BoundaryMode::On,
    );
    out.pop().expect("one reader, one result")
}

/// [`DigestEffects`] for several readers of ONE tree, in order (G16): each
/// reader's S4 entries and unresolved cones off the same tree, and the tree's
/// whole-program parts shared by every reader as `Arc` clones: its identity
/// table and coverage, and with `tests` (#254) its test-coverage graph, built
/// ONCE per tree (context-server design r3 §6.12, §13 item 3). The tree is
/// only read: a second read of the same tree answers the same. Over a tree
/// whose subjects include a reader's roots, a reader's entries and cones are
/// those of a tree over its roots alone (`prove_scoped_equals_unscoped`,
/// `t3_digest_scoped_equals_unscoped`); the coverage graph reads only the
/// tree's whole-program sections (`CoverageGraph::from_snapshot`), so it is
/// the same over any subject set. `boundary`: rule B as the tree states it
/// (`On`; a root inside a dependency is still read with it off, G31) or off
/// for every root. `gaps_in_cone`: prove's plan, which counts each cone's
/// analysis gaps. `include_reads`: digest's `--include-reads` (#246).
pub(crate) fn digest_effects_each(
    tree: &CapabilitySnapshot,
    model: &Model,
    readers: &[&[String]],
    tests: bool,
    gaps_in_cone: bool,
    include_reads: bool,
    boundary: BoundaryMode,
) -> Vec<DigestEffects> {
    // One query over every reader's roots: the per-root work is independent
    // (one worker per root), and the per-call work (the tree's indexes, the
    // reportable roots) is whole-program, so a call per reader repeated it
    // (G16: ~1 s per `prove` job over Base Application's source). Each reader
    // then takes the entries of its own roots, in the query's sorted order.
    let all: Vec<String> = readers
        .iter()
        .flat_map(|r| r.iter().cloned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let entries = passed_root_entries(tree, model, &all, include_reads, boundary)
        .into_iter()
        .flatten()
        .collect();
    finish_effects(tree, model, readers, entries, tests, gaps_in_cone)
}

/// [`root_entries`] after the literal pass (W2-1: every effects reader reads
/// them after it; W2-1b: done HERE, once per batch of roots, so a resident
/// revision memoizes the passed entries and a warm ask pays nothing for the
/// pass). The pass is a function of the root over any tree holding it, as
/// the entry is: it reads the tree's edges and direct facts, the
/// whole-program sections every tree carries. One `LiteralReach` per call.
pub(crate) fn passed_root_entries(
    tree: &CapabilitySnapshot,
    model: &Model,
    roots: &[String],
    include_reads: bool,
    boundary: BoundaryMode,
) -> Vec<Option<DigestEntryResult>> {
    let mut entries = root_entries(tree, model, roots, include_reads, boundary);
    let present: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].is_some())
        .collect();
    let mut passed: Vec<DigestEntryResult> = entries.iter_mut().filter_map(Option::take).collect();
    literal_reach_pass(tree, model, &mut passed);
    for (i, e) in present.into_iter().zip(passed) {
        entries[i] = Some(e);
    }
    entries
}

/// The pure half of [`digest_effects_each`]: each root's entry off `tree`, in
/// the order of `roots` (sorted and distinct), `None` for a root that yields
/// no entry (not reportable). A root's entry is a function of the root and
/// `include_reads` alone over any tree whose subjects include it (Q1 of the
/// context-server plan 2026-11-03, pinned by `prove_scoped_equals_unscoped`
/// and `t3_digest_scoped_equals_unscoped`), so a resident revision memoizes
/// it per `(root, include_reads)` (`ArtifactKey::Witness`). Every other
/// input of the root pass is fixed here and is no parameter: no return
/// summaries and `ordering_witness_only` off
/// ([`compute_digest_effects_cli_in`]), the isolated event ids from the
/// model, and rule B from `boundary` and the root (a root inside a
/// dependency is read with it off, G31). The entries are the root pass's
/// output BEFORE any read's own passes ([`finish_effects`] mutates them):
/// a memo hands out clones.
pub(crate) fn root_entries(
    tree: &CapabilitySnapshot,
    model: &Model,
    roots: &[String],
    include_reads: bool,
    boundary: BoundaryMode,
) -> Vec<Option<DigestEntryResult>> {
    debug_assert!(
        roots.windows(2).all(|w| w[0] < w[1]),
        "the roots are sorted and distinct"
    );
    // G31: a root INSIDE a dependency. Rule B (spec 2026-10-23 §14.3.5) ends a
    // witness on entering dependency code and groups what lies beyond, which
    // is right for a workspace root (it bounds digest's cost) but leaves a
    // dependency root nothing: it starts past its own boundary, so every fact
    // ended at the root and none was an effect. Such a root is asked with rule
    // B off, so its witness paths follow the code that reaches each effect,
    // in every app.
    let is_dependency_root = |r: &String| {
        tree.boundary
            .as_ref()
            .is_some_and(|b| b.dependency_app.contains_key(r))
    };
    let (dependency_roots, workspace_roots): (Vec<String>, Vec<String>) = match boundary {
        BoundaryMode::On => roots.iter().cloned().partition(is_dependency_root),
        BoundaryMode::Off => (Vec::new(), roots.to_vec()),
    };
    let mut by_root: HashMap<String, DigestEntryResult> =
        compute_digest_effects_cli_in(tree, model, Some(&workspace_roots), include_reads, boundary)
            .into_iter()
            .map(|e| (e.routine_id.clone(), e))
            .collect();
    if !dependency_roots.is_empty() {
        by_root.extend(
            compute_digest_effects_cli_in(
                tree,
                model,
                Some(&dependency_roots),
                include_reads,
                BoundaryMode::Off,
            )
            .into_iter()
            .map(|e| (e.routine_id.clone(), e)),
        );
    }
    roots.iter().map(|r| by_root.remove(r)).collect()
}

/// The per-read half of [`digest_effects_each`]: `entries` (every reader's
/// roots' [`root_entries`], owned, sorted by routine id) finished for each
/// reader: prove's literal reach (`gaps_in_cone`), which MUTATES them, each
/// reader's own entries and unresolved cones, and the tree's shared parts.
pub(crate) fn finish_effects(
    tree: &CapabilitySnapshot,
    model: &Model,
    readers: &[&[String]],
    all_entries: Vec<DigestEntryResult>,
    tests: bool,
    gaps_in_cone: bool,
) -> Vec<DigestEffects> {
    FinishCx::new(tree, model, tests, gaps_in_cone).finish(readers, all_entries)
}

/// [`finish_effects`]' per-tree half: the coverage graph (`tests`), the gap
/// subjects, the platform natives and the table-event raisers, built once
/// and shared by every batch [`FinishCx::finish`] is given (a streamed read,
/// [`digest_effects_chunks`]).
pub(crate) struct FinishCx<'t> {
    tree: &'t CapabilitySnapshot,
    background_starts: Arc<HashMap<String, crate::program::model::calls::BackgroundStart>>,
    gaps_in_cone: bool,
    coverage_graph: Option<Arc<crate::engine::l5::test_coverage::CoverageGraph>>,
    gap_subjects: Vec<&'t str>,
    natives: std::collections::HashSet<&'t str>,
    fires: std::collections::HashSet<&'t str>,
}

impl<'t> FinishCx<'t> {
    pub(crate) fn new(
        tree: &'t CapabilitySnapshot,
        model: &'t Model,
        tests: bool,
        gaps_in_cone: bool,
    ) -> FinishCx<'t> {
        let coverage_graph = tests.then(|| {
            let _s = crate::engine::perf_trace::span("digest", "digest.coverage_graph");
            crate::engine::perf_trace::counter_delta("build.coverage_graph", 1);
            Arc::new(crate::engine::l5::test_coverage::CoverageGraph::from_snapshot(tree, model))
        });
        let gap_subjects: Vec<&str> = if gaps_in_cone {
            tree.analysis_gaps
                .iter()
                .map(|g| g.subject.as_str())
                .collect()
        } else {
            Vec::new()
        };
        // Cataloged platform natives: they do not bring their app's gap.
        let natives: std::collections::HashSet<&str> = model
            .workspace
            .routines
            .iter()
            .filter(|r| crate::program::model::platform_natives::pure_native(r).is_some())
            .map(|r| r.stable_routine_id.as_str())
            .collect();
        // Routines that raise platform table events (prove's `loaded-subscribers`).
        let fires: std::collections::HashSet<&str> = if gaps_in_cone {
            model
                .workspace
                .routines
                .iter()
                .filter(|r| {
                    r.record_operations.iter().any(|op| {
                        crate::program::resolve::full::can_fire_trigger(&op.op.to_ascii_lowercase())
                    })
                })
                .map(|r| r.stable_routine_id.as_str())
                .collect()
        } else {
            Default::default()
        };
        FinishCx {
            tree,
            background_starts: Arc::clone(&model.calls.background_starts),
            gaps_in_cone,
            coverage_graph,
            gap_subjects,
            natives,
            fires,
        }
    }

    /// `all_entries` (every reader's roots' entries, owned, sorted by
    /// routine id) finished for each reader (see [`finish_effects`]). One
    /// reader takes the entries themselves; several each take copies of
    /// their own.
    pub(crate) fn finish(
        &self,
        readers: &[&[String]],
        all_entries: Vec<DigestEntryResult>,
    ) -> Vec<DigestEffects> {
        debug_assert!(
            all_entries
                .windows(2)
                .all(|w| w[0].routine_id < w[1].routine_id),
            "the entries are sorted by routine id"
        );
        let (tree, gaps_in_cone) = (self.tree, self.gaps_in_cone);
        // W2-1 (design §3.1): every reader of the effects (digest, explain,
        // prove) reads them after the literal pass, so "does R write T" has one
        // answer. W2-1b: the entries arrive passed ([`passed_root_entries`]), so a
        // resident revision memoizes them passed.
        let unresolved_span = crate::engine::perf_trace::span("digest", "digest.unresolved_cones");
        let mut all_entries = Some(all_entries);
        let read: Vec<(Vec<DigestEntryResult>, BTreeMap<String, UnresolvedCone>)> = readers
            .iter()
            .map(|roots| {
                let own: std::collections::HashSet<&str> =
                    roots.iter().map(String::as_str).collect();
                // One reader: every entry is its own (the entries are its
                // roots'), so it takes them rather than a copy (one copy of a
                // large read's witnesses less at its peak).
                let entries: Vec<DigestEntryResult> = if readers.len() == 1 {
                    all_entries
                        .take()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|e| own.contains(e.routine_id.as_str()))
                        .collect()
                } else {
                    all_entries
                        .iter()
                        .flatten()
                        .filter(|e| own.contains(e.routine_id.as_str()))
                        .cloned()
                        .collect()
                };
                let unresolved = roots
                    .iter()
                    .map(|r| {
                        let c = unresolved_cone(tree, r);
                        (
                            r.clone(),
                            UnresolvedCone {
                                gaps_in_cone: gaps_in_cone.then(|| {
                                    count_gaps_in_cone(
                                        &self.gap_subjects,
                                        &c.visited_ids,
                                        &self.natives,
                                    )
                                }),
                                fires_table_events: gaps_in_cone.then(|| {
                                    c.visited_ids
                                        .iter()
                                        .any(|v| self.fires.contains(v.as_str()))
                                }),
                                items: c.items,
                                traversal: c.traversal,
                            },
                        )
                    })
                    .collect();
                (entries, unresolved)
            })
            .collect();
        drop(unresolved_span);
        read.into_iter()
            .map(|(entries, unresolved)| DigestEffects {
                entries,
                unresolved,
                coverage: Arc::clone(&tree.coverage),
                identities: Arc::clone(&tree.identities),
                coverage_graph: self.coverage_graph.clone(),
                slice: None,
                background_starts: Arc::clone(&self.background_starts),
            })
            .collect()
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: the batches [`digest_effects_chunks`] read on this thread.
    pub(crate) static STREAM_BATCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One reader's [`DigestEffects`] over `roots` (sorted, distinct), STREAMED:
/// `f` is handed each batch of at most `chunk` roots (and its effects) in
/// root order,
/// so no more than one batch's witnesses (5.6-7.9 MiB per root on CDO) is
/// alive at once, plus what `f` keeps. The per-tree work of
/// [`digest_effects_each`] (the tree's indexes, the carrier cache, the
/// literal reach and its suffix trees, the finishing sets) is built ONCE and
/// shared by every batch. Each batch's effects equal the ones
/// [`digest_effects_each`] gives for a reader of that batch's roots: an
/// entry and its unresolved cone are functions of the root over any tree
/// holding it (`prove_scoped_equals_unscoped`), and rule B is decided per
/// root as [`root_entries`] does (pinned by
/// `streamed_effects_equal_one_read`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn digest_effects_chunks(
    tree: &CapabilitySnapshot,
    model: &Model,
    roots: &[String],
    chunk: usize,
    tests: bool,
    gaps_in_cone: bool,
    include_reads: bool,
    mut f: impl FnMut(&[String], DigestEffects),
) {
    use crate::engine::l5::digest::{QueryCx, root_set, with_query_cx};
    debug_assert!(
        roots.windows(2).all(|w| w[0] < w[1]),
        "the roots are sorted and distinct"
    );
    let chunk = chunk.max(1);
    // `compute_digest_effects_cli_in`'s root set, once: the reportable roots
    // and the dependency routines a caller may name.
    let eligible = root_set(model);
    let isolated = crate::program::model::events::isolated_event_ids(&model.workspace.routines);
    let isolated = (!isolated.is_empty()).then_some(&isolated);
    // G31 (`root_entries`): a root inside a dependency is read with rule B off.
    let is_dependency_root = |r: &String| {
        tree.boundary
            .as_ref()
            .is_some_and(|b| b.dependency_app.contains_key(r))
    };
    let any_dependency = roots.iter().any(is_dependency_root);
    let finish = FinishCx::new(tree, model, tests, gaps_in_cone);
    let mut pass = LiteralPass::new(tree, model);
    let mut batches = |on: &QueryCx<'_, '_>, off: Option<&QueryCx<'_, '_>>| {
        for batch in roots.chunks(chunk) {
            let _s = crate::engine::perf_trace::span("digest", "digest.stream_batch");
            crate::engine::perf_trace::counter_delta("digest.stream_batches", 1);
            #[cfg(test)]
            STREAM_BATCHES.with(|n| n.set(n.get() + 1));
            let (dependency, workspace): (Vec<String>, Vec<String>) = batch
                .iter()
                .filter(|r| eligible.contains(r.as_str()))
                .cloned()
                .partition(is_dependency_root);
            let run = |cx: &QueryCx<'_, '_>, ids: &[String]| {
                cx.run(ids, None, isolated, false, include_reads, |e| e)
            };
            let mut by_root: HashMap<String, DigestEntryResult> = run(on, &workspace)
                .into_iter()
                .map(|e| (e.routine_id.clone(), e))
                .collect();
            if let Some(off) = off {
                by_root.extend(
                    run(off, &dependency)
                        .into_iter()
                        .map(|e| (e.routine_id.clone(), e)),
                );
            }
            let mut entries: Vec<DigestEntryResult> =
                batch.iter().filter_map(|r| by_root.remove(r)).collect();
            drop(by_root);
            pass.pass(&mut entries);
            let mut out = finish.finish(&[batch], entries);
            f(batch, out.pop().expect("one reader, one result"));
        }
    };
    with_query_cx(tree, BoundaryMode::On, |on| {
        if any_dependency {
            with_query_cx(tree, BoundaryMode::Off, |off| batches(on, Some(off)));
        } else {
            batches(on, None);
        }
    });
}

/// G23, prove's read only: effects a root cannot reach once literal Boolean
/// arguments are followed through recursive SCCs, where the cone drops
/// guards. It MUTATES `entries` (sets `literal_unreachable`,
/// `dead_group_resources` and `group_resource_paths`, prunes `via_paths`),
/// so it runs on a read's own copies, never on memoized ones.
/// Spans (context-server design r3 §13 item 6): prove's post-root work
/// (0.94 s measured for one root on CDO).
pub(crate) fn literal_reach_pass(
    tree: &CapabilitySnapshot,
    model: &Model,
    entries: &mut [DigestEntryResult],
) {
    LiteralPass::new(tree, model).pass(entries);
}

/// [`literal_reach_pass`]'s per-tree half: the literal reach, the group
/// paths and the suffix trees, built once and shared by every batch of
/// entries [`LiteralPass::pass`] is given (a streamed read,
/// [`digest_effects_chunks`]).
pub(crate) struct LiteralPass<'t> {
    reach: crate::engine::l5::literal_reach::LiteralReach<'t>,
    group_paths: crate::engine::l5::group_path::GroupPaths<'t>,
    suffixes: crate::engine::l5::literal_reach::SuffixCache<'t>,
}

impl<'t> LiteralPass<'t> {
    pub(crate) fn new(tree: &'t CapabilitySnapshot, model: &'t Model) -> LiteralPass<'t> {
        let reach = {
            let _s = crate::engine::perf_trace::span("digest", "prove.literal_reach");
            crate::engine::l5::literal_reach::LiteralReach::new(
                &tree.typed_edges,
                &tree.capability_facts,
                model,
            )
        };
        // G35: how each group resource is reached inside the dependency.
        let group_paths = {
            let _s = crate::engine::perf_trace::span("digest", "prove.group_paths");
            crate::engine::l5::group_path::GroupPaths::new(&tree.typed_edges, model)
        };
        LiteralPass {
            reach,
            group_paths,
            // W2-0b (design §3.4.3): the suffix trees, shared by every root,
            // crossing and resource of this read.
            suffixes: crate::engine::l5::literal_reach::SuffixCache::default(),
        }
    }

    /// The pass over `entries` (see [`literal_reach_pass`]).
    pub(crate) fn pass(&mut self, entries: &mut [DigestEntryResult]) {
        let LiteralPass {
            reach,
            group_paths,
            suffixes,
        } = self;
        literal_reach_entries(reach, group_paths, suffixes, entries);
    }
}

fn literal_reach_entries<'t>(
    reach: &crate::engine::l5::literal_reach::LiteralReach<'t>,
    group_paths: &crate::engine::l5::group_path::GroupPaths<'t>,
    suffixes: &mut crate::engine::l5::literal_reach::SuffixCache<'t>,
    entries: &mut [DigestEntryResult],
) {
    let _s = crate::engine::perf_trace::span("digest", "prove.reach_effects");
    // A shared fill is never cancelled by a request (context-server design
    // §4.6), so nothing here asks to stop; the check is wired for a reader
    // that owns its search.
    let never = || false;
    for entry in entries {
        // The root's guarded search, built once per root on first need
        // (`None` inside: it hit its cap).
        let mut root_tree = None;
        if entry.effects.iter().any(|e| e.boundary_group) {
            root_tree = Some(reach.live_tree_of(&entry.routine_id));
        }
        if let Some(Some(tree)) = &root_tree {
            // One crossing's entries serve each of its rows.
            let mut by_crossing: HashMap<
                String,
                crate::engine::l5::literal_reach::CrossingEntries,
            > = HashMap::new();
            for e in entry.effects.iter_mut().filter(|e| e.boundary_group) {
                let Some(c) = e.crossing.clone() else {
                    continue;
                };
                let entries = by_crossing
                    .entry(c.id.clone())
                    .or_insert_with(|| reach.crossing_entries(tree, &c));
                // W2-0c (design §3.3.1): per resource, the operations that
                // could write it: every direct fact of its kind in the group's
                // class, with the group's representatives. The path (G35,
                // W2-0b) and the status read the same set.
                // Only table writes are judged; an event group's resources
                // keep W2-0b's representatives (none: no path, as before).
                let kt = is_known_temp_state(e.temp_state.as_ref());
                // W2-1: table WRITES only; a `DB_READ` group (digest
                // `--include-reads`) is never pruned.
                let judged = matches!(
                    e.effect_type.as_str(),
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE"
                );
                let candidates: Vec<(String, Option<Vec<String>>)> = e
                    .group_resources
                    .iter()
                    .map(|(r, _)| {
                        let reps = e
                            .group_resource_ops
                            .iter()
                            .find(|(id, _)| id == r)
                            .and_then(|(_, ops)| ops.clone());
                        let ops = if judged {
                            reach.candidate_ops(
                                &e.effect_type,
                                r,
                                kt,
                                reps.as_deref().unwrap_or_default(),
                            )
                        } else {
                            reps
                        };
                        (r.clone(), ops)
                    })
                    .collect();
                e.group_resource_paths =
                    group_paths.paths(reach, tree, entries, e, &candidates, suffixes, &never);
                // W2-6 (2f): a resource-less group (COMMIT, HTTP, UI, …) gets
                // its own terminal witness: through the crossing to a live
                // state owning a direct fact of its type.
                if e.group_resources.is_empty()
                    && !e.effect_type.starts_with("DB_")
                    && e.effect_type != "EVENT_PUBLISH"
                {
                    let ops = vec![(String::new(), reach.terminal_ops(&e.effect_type))];
                    e.group_terminal_path = group_paths
                        .paths(reach, tree, entries, e, &ops, suffixes, &never)
                        .pop()
                        .map(|(_, o)| o);
                }
                // Each resource judged from THIS crossing's entry states.
                e.group_resource_status = candidates
                    .iter()
                    .map(|(r, ops)| {
                        use crate::engine::l5::literal_reach::Undecided;
                        let status = match ops {
                            _ if !judged => {
                                ResourceStatus::NotEstablished(Undecided::KindNotJudged)
                            }
                            Some(ops) => reach.resource_status(entries, ops, suffixes, &never),
                            None => ResourceStatus::NotEstablished(Undecided::NoOperation),
                        };
                        (r.clone(), status)
                    })
                    .collect();
            }
        }
        // A site-enumerated type's rows are judged one site at a time, unless
        // its search stopped at the depth bound (a site may be missing).
        let per_site: Vec<&str> = crate::engine::l5::digest::SITE_ENUMERATED
            .iter()
            .copied()
            .filter(|t| !entry.partial_site_types.iter().any(|p| p == t))
            .collect();
        let (dead, dead_resources) =
            reach.unreachable_effects(&entry.routine_id, &entry.effects, &per_site);
        for i in dead {
            entry.effects[i].literal_unreachable = true;
        }
        // G33: a group's resources reached only through code that cannot run.
        for (i, resources) in dead_resources {
            entry.effects[i].dead_group_resources = resources;
        }
        // W2-0c step 2 (design §3.3.1): and those this group's own crossing
        // does not reach. Only a completed search disproves; anything not
        // established keeps the resource. The raw list stays whole.
        for e in entry.effects.iter_mut().filter(|e| e.boundary_group) {
            for (r, s) in &e.group_resource_status {
                if *s == ResourceStatus::Disproved && !e.dead_group_resources.contains(r) {
                    e.dead_group_resources.push(r.clone());
                }
            }
        }
        // A listed witness path that cannot run is not evidence: drop it.
        // When none is left for a still-reachable effect, a live path exists
        // that was not listed.
        for e in entry.effects.iter_mut().filter(|e| !e.literal_unreachable) {
            let before = e.via_paths.len();
            e.via_paths.retain(|p| reach.path_live(p));
            if e.via_paths.len() < before && e.via_paths.is_empty() {
                e.via_paths_truncated = true;
            }
        }
        // Final CDO output review, item 2 (design §3.4.2): an own write row
        // left with no listed path still has a live route (it is not
        // literal-dead), so find it: the root's guarded search to the first
        // live state owning one of the row's own operations, its evidence
        // operation first. Before, such a row printed its anchor with no
        // `path:` and no reason (CDO: IssueReminder's `CDO Queue Entry`
        // Insert, whose three listed paths all pass a call its literal
        // arguments rule out). A row of the root's own body needs no search.
        // Rows with no source position (`unavailable`) are the no-path
        // section's (§4.1 section 13), and known-temporary rows print no
        // path; neither is searched.
        for e in entry.effects.iter_mut() {
            if e.boundary_group
                || e.literal_unreachable
                || e.via_paths.iter().any(|p| !p.is_empty())
                || !matches!(
                    e.effect_type.as_str(),
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE"
                )
                || e.evidence.source_kind == "unavailable"
                || is_known_temp_state(e.temp_state.as_ref())
            {
                continue;
            }
            let mut ops: Vec<String> = e.evidence_operation_id.iter().cloned().collect();
            for op in &e.origin_operation_ids {
                if !ops.contains(op) {
                    ops.push(op.clone());
                }
            }
            let tree = match ops
                .iter()
                .any(|op| reach.live_in_root(&entry.routine_id, op))
            {
                // The zero-hop witness: the root owns a live one.
                true => None,
                false => Some(
                    root_tree
                        .get_or_insert_with(|| reach.live_tree_of(&entry.routine_id))
                        .as_ref(),
                ),
            };
            e.own_path = Some(match tree {
                None => group_paths.own_path_in_root(reach, &entry.routine_id, &ops),
                Some(t) => group_paths.own_path(reach, t, &ops),
            });
        }
    }
}

/// The real entry point: given resolved workspace + a list of roots (their stableIds),
/// produce DigestEntryFull for each root (order:false path). `changed` supplies
/// the declaration spans and the touched declarations (#71); `roots` may be a
/// subset of `changed.roots` (prove passes one). `owners`: each root's owning
/// object, from the model's [`OwnerIndex`] (#78); `""` when absent.
pub fn run_digest_query_full_from_entries(
    effects: &mut DigestEffects,
    roots: &[String],
    tx_ctx_map: &HashMap<String, TransactionContext>,
    temp_states: &DbTempStates<'_>,
    changed: &ChangedRootsResult,
    owners: &OwnerIndex<'_>,
) -> DigestQueryFullResult {
    let (spans, touched) = (&changed.spans, &changed.touched);
    let mut query_diagnostics: Vec<DigestQueryDiagnostic> = Vec::new();
    let background_starts = Arc::clone(&effects.background_starts);

    // Coverage map
    let mut coverage_by_id: HashMap<&str, (&str, Vec<&str>)> = HashMap::new();
    for rec in effects.coverage.iter() {
        let reasons: Vec<&str> = rec.reasons.iter().map(|s| s.as_str()).collect();
        coverage_by_id.insert(
            rec.subject.as_str(),
            (rec.inherited_status.as_str(), reasons),
        );
    }

    // Identities
    let identities = &effects.identities;
    let mut display_by_id: HashMap<&str, &str> = HashMap::new();
    for i in 0..identities.stable_ids.len() {
        let id = identities
            .stable_ids
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        let nm = identities
            .display_names
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        if !id.is_empty() {
            display_by_id.insert(id, nm);
        }
    }

    // Build index of base entries by routineId, by position: each boundary
    // group's resource set is moved out of its entry below, not copied (final
    // panel C2).
    let base_by_rid: HashMap<String, usize> = effects
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.routine_id.clone(), i))
        .collect();

    let mut out: Vec<DigestEntryFull> = Vec::new();

    // The owning object comes from the model index, never the identity table
    // (#78); `""` when the id is absent, never an invented name.
    let owner_of = |rid: &str| owners.object_display(rid);

    for rid in roots {
        // Routine anchor at the declaration (#71): the LAST touched declaration in
        // document order (a single-declaration id: that one). See the ledger's
        // Entry anchor for why last, not first. `None` when the id has no `ws:` span.
        let decls = spans.of_routine(rid);
        let routine_anchor = touched
            .get(rid.as_str())
            .and_then(|t| t.iter().next_back())
            .map(|&i| spans.spans[i].anchor());
        let routine_anchors: Vec<FullAnchor> = if decls.len() > 1 {
            decls.iter().map(|&i| spans.spans[i].anchor()).collect()
        } else {
            Vec::new()
        };

        // Declaration-only roots (Interface / ControlAddIn methods) are not in the
        // snapshot: build them from the span index FIRST, so they push no
        // `root-not-in-snapshot` / `no-coverage-record` and count once (#71).
        if let Some(display) = decls
            .first()
            .and_then(|&i| spans.spans[i].declaration_only_display.as_ref())
        {
            out.push(DigestEntryFull {
                routine_id: rid.clone(),
                routine_display: display.clone(),
                object_display: owner_of(rid),
                routine_anchor,
                routine_anchors,
                coverage_status: "unknown".to_string(),
                coverage_reasons: vec!["declaration-only".to_string()],
                effects: Vec::new(),
                unresolved: Vec::new(),
                unresolved_traversal: UnresolvedTraversal {
                    truncated: false,
                    max_depth: DEFAULT_MAX_DEPTH,
                    visited_routines: 0,
                },
                entry_diagnostics: Vec::new(),
                declaration_only: true,
            });
            continue;
        }

        // Verify the routine exists in the snapshot. Missing display → emit
        // `root-not-in-snapshot` and skip (no entry). Mirrors digest-query.ts.
        let Some(rtn_display) = display_by_id.get(rid.as_str()).copied() else {
            query_diagnostics.push(DigestQueryDiagnostic::RootNotInSnapshot {
                routine_id: rid.clone(),
            });
            continue;
        };

        // Coverage. Missing record → emit `no-coverage-record` but STILL build the
        // entry with status "unknown" (TS: covRec === undefined still produces an entry).
        let (cov_status, cov_reasons) = match coverage_by_id.get(rid.as_str()) {
            Some((s, r)) => (*s, r.iter().map(|x| x.to_string()).collect::<Vec<_>>()),
            None => {
                query_diagnostics.push(DigestQueryDiagnostic::NoCoverageRecord {
                    routine_id: rid.clone(),
                });
                ("unknown", Vec::new())
            }
        };

        // Get base effects (from S4 ordering)
        let base_entry = base_by_rid
            .get(rid.as_str())
            .map(|&i| &mut effects.entries[i]);

        let mut full_effects: Vec<DigestEffectFull> = Vec::new();
        let mut entry_diagnostics: Vec<EntryDiagnostic> = Vec::new();

        if let Some(entry) = base_entry {
            // A site list the search could not finish (the depth bound): its
            // count is a lower bound, and the renderers say "at least".
            for ty in &entry.partial_site_types {
                entry_diagnostics.push(EntryDiagnostic {
                    kind: SITES_PARTIAL,
                    effect_type: Some(ty.clone()),
                    fact_subject: None,
                    fact_id: None,
                    operation_id: None,
                    reason: None,
                });
            }
            for eff in &mut entry.effects {
                // Conditionality — the per-path value computed in digest_query and
                // stored on the effect (#8 / #17). No recomputation here.
                let cond = eff.conditionality;

                // Transaction context for COMMIT
                let tx_ctx = if eff.effect_type == "COMMIT" {
                    if let Some(op_id) = &eff.evidence_operation_id {
                        tx_ctx_map
                            .get(op_id.as_str())
                            .copied()
                            .unwrap_or(TX_UNKNOWN)
                    } else {
                        TX_UNKNOWN
                    }
                } else {
                    TX_UNKNOWN
                };

                // Evidence anchor. G32: an event publication is the publisher
                // routine's own fact, with no operation to cite: its anchor is the
                // call into the publisher, the last hop of its shortest path.
                let publish_call = (eff.evidence.source_kind == "unavailable"
                    && eff.effect_type == "EVENT_PUBLISH")
                    .then(|| eff.via_paths.iter().min_by_key(|p| p.len()))
                    .flatten()
                    .and_then(|p| p.last())
                    .and_then(|h| Some((h.inner.anchor.as_ref()?, &h.inner)));
                let evidence = match publish_call {
                    Some((a, hop)) => FullAnchor {
                        source_kind: "source",
                        file: Some(a.file.clone()),
                        line: a.line,
                        column: a.column,
                        excerpt: hop.callee_display.clone(),
                    },
                    None => FullAnchor {
                        source_kind: eff.evidence.source_kind,
                        file: eff.evidence.file.clone(),
                        line: eff.evidence.line,
                        column: eff.evidence.column,
                        excerpt: eff.evidence.excerpt.clone(),
                    },
                };

                // evidence-unavailable diagnostic
                if evidence.source_kind == "unavailable" {
                    entry_diagnostics.push(EntryDiagnostic {
                        kind: "evidence-unavailable",
                        effect_type: Some(eff.effect_type.clone()),
                        fact_subject: Some(eff.fact_subject.clone()),
                        fact_id: None,
                        operation_id: None,
                        reason: None,
                    });
                }

                // Guarantees: unique labels from scopedGuarantees in first-occurrence order.
                // Mirrors TS guaranteesByIndex dedup: intra + root merged, duplicates suppressed
                // (a label may appear twice in scopedGuarantees with different scopes but only
                // once in the plain guarantees[] backward-compat array).
                let guarantees: Vec<String> = {
                    let mut seen = std::collections::HashSet::new();
                    eff.scoped_guarantees
                        .iter()
                        .filter_map(|sg| {
                            if seen.insert(sg.label) {
                                Some(sg.label.to_string())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                // #65: the record's temp state, CLI layer only (the core `detail`
                // feeds dedupe/factId and must not move). #69 D4: joined per
                // operation in the effect's class (its cone fact's class, frame-
                // correct after D1); every failed join is a diagnostic.
                let mut detail = eff.detail.clone();
                // W2-0c, raw versus refined (design §3.1): a group whose
                // refined membership is shorter than its raw one (resources
                // the root, or this group's crossing, cannot reach: G33,
                // W2-0c) describes the REFINED list: `resourceCount` counts it
                // and `resources` names its first ten; the raw count stays as
                // `resourcesOriginalCount`. Prove's read only: no other read
                // refines.
                if !eff.dead_group_resources.is_empty() {
                    let refined: Vec<&str> = eff
                        .group_resources
                        .iter()
                        .filter(|(id, _)| !eff.dead_group_resources.contains(id))
                        .map(|(_, d)| d.as_str())
                        .collect();
                    if refined.len() < eff.group_resources.len() {
                        for (k, v) in detail.iter_mut() {
                            match k.as_str() {
                                "resourceCount" => *v = refined.len().to_string(),
                                "resources" => {
                                    *v = refined
                                        .iter()
                                        .take(10)
                                        .copied()
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                }
                                _ => {}
                            }
                        }
                        detail.push((
                            "resourcesOriginalCount".to_string(),
                            eff.group_resources.len().to_string(),
                        ));
                    }
                }
                if matches!(
                    eff.effect_type.as_str(),
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" | "DB_READ"
                ) {
                    let kt = is_known_temp_state(eff.temp_state.as_ref());
                    let join = temp_states.for_effect(rid, &eff.origin_operation_ids, kt);
                    match join.render() {
                        Some(ts) => detail.push(("tempState".to_string(), ts)),
                        // G31: a root L4 holds no rows for (a dependency root:
                        // the summary bundle has none). The cone fact's own class
                        // is frame-correct (#69 D1), so a known-temporary write
                        // says so; any other class stays absent (not known).
                        None if kt && join.states.is_empty() => {
                            detail.push(("tempState".to_string(), "known(true)".to_string()));
                        }
                        None => {}
                    }
                    entry_diagnostics.extend(incoherence_diagnostics(
                        &eff.effect_type,
                        &eff.fact_subject,
                        &eff.fact_id,
                        join.problems,
                    ));
                }
                // F3: a background start names what it starts (CLI layer
                // only, as the temp state). A boundary group has no site.
                if eff.effect_type == "BACKGROUND_TASK"
                    && !eff.boundary_group
                    && let Some(start) = eff
                        .evidence_callsite_id
                        .as_deref()
                        .and_then(|cs| background_starts.get(cs))
                {
                    detail.push(("starts".to_string(), start.phrase()));
                }

                full_effects.push(DigestEffectFull {
                    effect_type: eff.effect_type.clone(),
                    detail,
                    provenance: eff.provenance,
                    evidence,
                    via_paths: eff.via_paths.clone(),
                    via_paths_truncated: eff.via_paths_truncated,
                    conditionality: cond,
                    transaction_context: tx_ctx,
                    guarantees,
                    fact_id: eff.fact_id.clone(),
                    scoped_guarantees: eff.scoped_guarantees.clone(),
                    // G33: a resource the root cannot reach is no member prove
                    // matches (`literal_reach`).
                    group_resources: std::mem::take(&mut eff.group_resources)
                        .into_iter()
                        .filter(|(id, _)| !eff.dead_group_resources.contains(id))
                        .collect(),
                    group_resource_paths: std::mem::take(&mut eff.group_resource_paths)
                        .into_iter()
                        .filter(|(id, _)| !eff.dead_group_resources.contains(id))
                        .collect(),
                    group_terminal_path: eff.group_terminal_path.take(),
                    own_path: eff.own_path.take(),
                    origin_operation_ids: eff.origin_operation_ids.clone(),
                    effect_is_kt: is_known_temp_state(eff.temp_state.as_ref()),
                    literal_unreachable: eff.literal_unreachable,
                    crossing: eff.crossing.clone(),
                });
            }
        }

        // Unresolved cone, computed with the effects. (gapsInCone — TS digestQuery
        // computes it onto the internal DigestEntryResult for the prove engine, but
        // it is NOT part of the digest JSON contract (contracts/digest.ts omits it)
        // and the CLI does not expose queryEntries, so it is intentionally not
        // computed here (#20).)
        // Moved out, not cloned (final panel F1-C6): the caller drops the
        // effects right after. `roots` holds each id once.
        let cone = effects
            .unresolved
            .remove(rid.as_str())
            .expect("digest_effects computes every root's cone");

        out.push(DigestEntryFull {
            routine_id: rid.clone(),
            routine_display: rtn_display.to_string(),
            object_display: owner_of(rid),
            routine_anchor,
            routine_anchors,
            coverage_status: cov_status.to_string(),
            coverage_reasons: cov_reasons,
            effects: full_effects,
            unresolved: cone.items,
            unresolved_traversal: cone.traversal,
            entry_diagnostics,
            declaration_only: false,
        });
    }

    // Sort by routineId (already sorted from resolve_changed_roots, but be safe)
    out.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    DigestQueryFullResult {
        entries: out,
        query_diagnostics,
    }
}

// ---------------------------------------------------------------------------
// JSON document projection
// ---------------------------------------------------------------------------

/// An anchor's JSON: `a.line` is the model's 0-based row, written 1-based
/// (`lineBase: 1`, [`crate::engine::line_base`]).
pub fn anchor_to_value(a: &FullAnchor) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("sourceKind".into(), a.source_kind.into());
    if let Some(ref f) = a.file {
        m.insert("file".into(), f.clone().into());
    }
    if let Some(l) = shown_opt(a.line) {
        m.insert("line".into(), l.into());
    }
    if let Some(c) = a.column {
        m.insert("column".into(), c.into());
    }
    if let Some(ref x) = a.excerpt {
        m.insert("excerpt".into(), x.clone().into());
    }
    serde_json::Value::Object(m)
}

fn hop_to_value(hop: &crate::engine::l5::digest::ProjectedHop) -> serde_json::Value {
    crate::engine::l5::digest::hop_to_json_value(&hop.inner)
}

pub(crate) fn scoped_guarantee_to_value(
    sg: &crate::engine::l5::ordering_engine::ScopedGuarantee,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("label".into(), sg.label.into());
    m.insert("scope".into(), sg.scope.into());
    if let Some(ref v) = sg.write_occurrence_id {
        m.insert("writeOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.commit_occurrence_id {
        m.insert("commitOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.io_occurrence_id {
        m.insert("ioOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.return_occurrence_id {
        m.insert("returnOccurrenceId".into(), v.clone().into());
    }
    // supportingEdgeIds — only emit when non-empty (mirrors TS contract: optional field).
    if !sg.supporting_edge_ids.is_empty() {
        let eids: Vec<serde_json::Value> = sg
            .supporting_edge_ids
            .iter()
            .map(|s| s.clone().into())
            .collect();
        m.insert("supportingEdgeIds".into(), serde_json::Value::Array(eids));
    }
    if let Some(ce) = sg.commit_effectiveness {
        m.insert("commitEffectiveness".into(), ce.into());
    }
    m.insert("interveningBoundary".into(), sg.intervening_boundary.into());
    m.insert("validForRefutation".into(), sg.valid_for_refutation.into());
    serde_json::Value::Object(m)
}

/// Whether a guarantee label of `eff` is valid for refutation: any of its
/// scoped guarantees is (output validation §9.8, F73). A label with none is
/// descriptive only: it describes an ordering, it cannot refute a claim.
#[must_use]
pub fn guarantee_refutable(eff: &DigestEffectFull, label: &str) -> bool {
    eff.scoped_guarantees
        .iter()
        .any(|sg| sg.label == label && sg.valid_for_refutation)
}

/// `refutableGuarantees` (F73, additive beside `guarantees`): the labels of
/// `guarantees` that are valid for refutation, in `guarantees` order,
/// written only when at least one is. THE RULE: a label in `guarantees`
/// that is not listed here is descriptive only (it describes an ordering;
/// it cannot refute a claim). So a reader never takes a descriptive
/// guarantee for a refutable fact, even where the folded document keeps no
/// trivial scoped guarantee to look the flag up in. `None`: no label is
/// refutable (every label descriptive only), the field is omitted.
pub(crate) fn refutable_guarantees(eff: &DigestEffectFull) -> Option<serde_json::Value> {
    let labels: Vec<serde_json::Value> = eff
        .guarantees
        .iter()
        .filter(|g| guarantee_refutable(eff, g))
        .map(|g| g.clone().into())
        .collect();
    (!labels.is_empty()).then_some(serde_json::Value::Array(labels))
}

pub fn effect_to_value(eff: &DigestEffectFull) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), eff.effect_type.clone().into());

    // detail: insertion-order map
    let mut detail_obj = serde_json::Map::new();
    for (k, v) in &eff.detail {
        detail_obj.insert(k.clone(), v.clone().into());
    }
    m.insert("detail".into(), serde_json::Value::Object(detail_obj));
    // MCP output plan 1d (D4): a boundary group's EVERY resource, in resource-id
    // order (`detail.resources` names the first ten); `detail.resourceCount`
    // equals its length.
    if !eff.group_resources.is_empty() {
        let all: Vec<serde_json::Value> = eff
            .group_resources
            .iter()
            .map(|(id, display)| serde_json::json!({"id": id, "display": display}))
            .collect();
        m.insert("resources".into(), serde_json::Value::Array(all));
    }

    m.insert("provenance".into(), eff.provenance.into());
    m.insert("evidence".into(), anchor_to_value(&eff.evidence));

    // viaPaths
    let via: Vec<serde_json::Value> = eff
        .via_paths
        .iter()
        .map(|path| {
            let hops: Vec<serde_json::Value> = path.iter().map(hop_to_value).collect();
            serde_json::Value::Array(hops)
        })
        .collect();
    m.insert("viaPaths".into(), serde_json::Value::Array(via));
    m.insert("viaPathsTruncated".into(), eff.via_paths_truncated.into());
    // W2-1, raw versus refined (design §3.1): a row the root cannot reach
    // under literal arguments stays (raw), with its status beside it, so a
    // literal-dead write never reads like a live one. Present only then.
    if eff.literal_unreachable {
        m.insert("literalUnreachable".into(), true.into());
    }
    m.insert("confidence".into(), "static".into());
    m.insert("conditionality".into(), eff.conditionality.into());
    m.insert("transactionContext".into(), eff.transaction_context.into());
    let guarantees: Vec<serde_json::Value> =
        eff.guarantees.iter().map(|g| g.clone().into()).collect();
    m.insert("guarantees".into(), serde_json::Value::Array(guarantees));
    if let Some(r) = refutable_guarantees(eff) {
        m.insert("refutableGuarantees".into(), r);
    }
    m.insert("factId".into(), eff.fact_id.clone().into());
    let sg: Vec<serde_json::Value> = eff
        .scoped_guarantees
        .iter()
        .map(scoped_guarantee_to_value)
        .collect();
    m.insert("scopedGuarantees".into(), serde_json::Value::Array(sg));

    serde_json::Value::Object(m)
}

pub(crate) fn unresolved_item_to_value(item: &UnresolvedConeItem) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(ref f) = item.callsite_file {
        // Build callsiteAnchor
        let mut anch = serde_json::Map::new();
        anch.insert("sourceKind".into(), "source".into());
        anch.insert("file".into(), f.clone().into());
        if let Some(l) = shown_opt(item.callsite_line) {
            anch.insert("line".into(), l.into());
        }
        if let Some(c) = item.callsite_column {
            anch.insert("column".into(), c.into());
        }
        m.insert("callsiteAnchor".into(), serde_json::Value::Object(anch));
    }
    m.insert("calleeDisplay".into(), item.callee_display.clone().into());
    m.insert("status".into(), item.status.clone().into());
    if let Some(ref cands) = item.candidates {
        let cv: Vec<serde_json::Value> = cands.iter().map(|c| c.clone().into()).collect();
        m.insert("candidates".into(), serde_json::Value::Array(cv));
    }
    if item.open_world == Some(true) {
        m.insert("openWorld".into(), true.into());
    }
    m.insert("owningRoutine".into(), item.owning_routine.clone().into());
    m.insert(
        "owningRoutineDisplay".into(),
        item.owning_routine_display.clone().into(),
    );
    serde_json::Value::Object(m)
}

fn entry_to_value(entry: &DigestEntryFull) -> serde_json::Value {
    let mut m = serde_json::Map::new();

    // coverage
    let mut cov = serde_json::Map::new();
    cov.insert("status".into(), entry.coverage_status.clone().into());
    let reasons: Vec<serde_json::Value> = entry
        .coverage_reasons
        .iter()
        .map(|r| r.clone().into())
        .collect();
    cov.insert("reasons".into(), serde_json::Value::Array(reasons));
    m.insert("coverage".into(), serde_json::Value::Object(cov));

    // effects
    let effects: Vec<serde_json::Value> = entry.effects.iter().map(effect_to_value).collect();
    m.insert("effects".into(), serde_json::Value::Array(effects));

    m.insert("entryDiagnostics".into(), entry_diagnostics_to_value(entry));
    m.insert("routine".into(), routine_to_value(entry));

    // unresolved
    let ur: Vec<serde_json::Value> = entry
        .unresolved
        .iter()
        .map(unresolved_item_to_value)
        .collect();
    m.insert("unresolved".into(), serde_json::Value::Array(ur));

    // unresolvedTraversal
    let mut trav = serde_json::Map::new();
    trav.insert(
        "maxDepth".into(),
        (entry.unresolved_traversal.max_depth as u64).into(),
    );
    trav.insert(
        "truncated".into(),
        entry.unresolved_traversal.truncated.into(),
    );
    trav.insert(
        "visitedRoutines".into(),
        (entry.unresolved_traversal.visited_routines as u64).into(),
    );
    m.insert(
        "unresolvedTraversal".into(),
        serde_json::Value::Object(trav),
    );

    serde_json::Value::Object(m)
}

/// An entry's `entryDiagnostics` list (both document shapes).
pub(crate) fn entry_diagnostics_to_value(entry: &DigestEntryFull) -> serde_json::Value {
    let ed: Vec<serde_json::Value> = entry
        .entry_diagnostics
        .iter()
        .map(|d| {
            let mut dm = serde_json::Map::new();
            dm.insert("kind".into(), d.kind.into());
            if let Some(ref et) = d.effect_type {
                dm.insert("effectType".into(), et.clone().into());
            }
            if let Some(ref fs) = d.fact_subject {
                dm.insert("factSubject".into(), fs.clone().into());
            }
            if let Some(ref id) = d.fact_id {
                dm.insert("factId".into(), id.clone().into());
            }
            if let Some(ref op) = d.operation_id {
                dm.insert("operationId".into(), op.clone().into());
            }
            if let Some(reason) = d.reason {
                dm.insert("reason".into(), reason.into());
            }
            serde_json::Value::Object(dm)
        })
        .collect();
    serde_json::Value::Array(ed)
}

/// An entry's `routine` block (both document shapes: bc-dev-mcp reads its
/// `anchor` and `anchors`).
pub(crate) fn routine_to_value(entry: &DigestEntryFull) -> serde_json::Value {
    let mut routine_m = serde_json::Map::new();
    routine_m.insert("stableId".into(), entry.routine_id.clone().into());
    routine_m.insert("display".into(), entry.routine_display.clone().into());
    routine_m.insert("objectDisplay".into(), entry.object_display.clone().into());
    if let Some(ref a) = entry.routine_anchor {
        routine_m.insert("anchor".into(), anchor_to_value(a));
    }
    if entry.declaration_only {
        routine_m.insert("declarationOnly".into(), true.into());
    }
    if !entry.routine_anchors.is_empty() {
        routine_m.insert(
            "anchors".into(),
            serde_json::Value::Array(entry.routine_anchors.iter().map(anchor_to_value).collect()),
        );
    }
    serde_json::Value::Object(routine_m)
}

/// One changed-roots diagnostic as digest's JSON writes it (`explain --diff`
/// writes the same objects).
pub(crate) fn changed_roots_diagnostic_to_value(d: &ChangedRootsDiagnostic) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), d.kind().into());
    match d {
        ChangedRootsDiagnostic::FileUnmatched { file }
        | ChangedRootsDiagnostic::DiffFileUnmatched { file }
        | ChangedRootsDiagnostic::DiffFileDeleted { file }
        | ChangedRootsDiagnostic::DiffFileNotAl { file } => {
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::SelectorUnmatched { selector, hints } => {
            m.insert("selector".into(), selector.clone().into());
            m.extend(crate::engine::l5::selector_index::hints_json(hints));
        }
        ChangedRootsDiagnostic::SelectorAmbiguous {
            selector,
            candidates,
            routines,
            listed,
        } => {
            m.insert("selector".into(), selector.clone().into());
            m.insert("candidates".into(), candidates.clone().into());
            if !routines.is_empty() {
                m.insert("candidateRoutines".into(), routines.clone().into());
            }
            if !listed.is_empty() {
                m.insert(
                    "candidateSelectors".into(),
                    listed.iter().map(|c| c.selector.clone()).collect(),
                );
            }
        }
        ChangedRootsDiagnostic::FileAmbiguous { file, candidates }
        | ChangedRootsDiagnostic::DiffFileAmbiguous { file, candidates } => {
            m.insert("file".into(), file.clone().into());
            m.insert("candidates".into(), candidates.clone().into());
        }
        ChangedRootsDiagnostic::HunkOutsideRoutines {
            file,
            start_line,
            end_line,
        } => {
            m.insert("file".into(), file.clone().into());
            m.insert("startLine".into(), (*start_line as u64).into());
            m.insert("endLine".into(), (*end_line as u64).into());
        }
        ChangedRootsDiagnostic::DiffParseError { file, detail } => {
            if let Some(file) = file {
                m.insert("file".into(), file.clone().into());
            }
            m.insert("detail".into(), detail.clone().into());
        }
    }
    serde_json::Value::Object(m)
}

/// One human line for a changed-roots diagnostic (#340):
/// `changed-roots: <kind> <subject>[ <detail>]`, every string escaped so the
/// diagnostic stays on one line.
pub(crate) fn changed_roots_diagnostic_human(d: &ChangedRootsDiagnostic) -> String {
    let list = |c: &[String]| {
        c.iter()
            .map(|s| escape_controls(s))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let rest = match d {
        ChangedRootsDiagnostic::FileUnmatched { file }
        | ChangedRootsDiagnostic::DiffFileUnmatched { file }
        | ChangedRootsDiagnostic::DiffFileDeleted { file }
        | ChangedRootsDiagnostic::DiffFileNotAl { file } => escape_controls(file),
        ChangedRootsDiagnostic::SelectorUnmatched { selector, hints } => {
            // V3 §2.4: the hints on the one line.
            let lines = crate::engine::l5::selector_index::hint_lines(hints, &[]);
            format!(
                "{} ({})",
                escape_controls(selector),
                escape_controls(&lines.join("; "))
            )
        }
        ChangedRootsDiagnostic::SelectorAmbiguous {
            selector, listed, ..
        } if !listed.is_empty() => {
            // V3 §2.4: each candidate's full display and declaration, and
            // the ready value of their selectors.
            let each: Vec<String> = listed
                .iter()
                .map(|c| match c.at.is_empty() {
                    true => c.display.clone(),
                    false => format!("{} at {}", c.display, c.at),
                })
                .collect();
            let value: Vec<String> = listed
                .iter()
                .map(|c| serde_json::Value::from(c.selector.as_str()).to_string())
                .collect();
            format!(
                "{} (candidates: {}); routines: [{}]",
                escape_controls(selector),
                list(&each),
                value.join(", ")
            )
        }
        ChangedRootsDiagnostic::SelectorAmbiguous {
            selector,
            candidates,
            routines,
            ..
        } => {
            let shown = if routines.is_empty() {
                candidates
            } else {
                routines
            };
            format!(
                "{} (candidates: {}){}",
                escape_controls(selector),
                list(shown),
                if routines.is_empty() {
                    ""
                } else {
                    "; pass one as <owner>::<routine>(<parameter types>) or its stable id"
                }
            )
        }
        ChangedRootsDiagnostic::FileAmbiguous { file, candidates } => format!(
            "{} (candidates: {}); pass one candidate as --file, or --diff-root <folder> to read paths relative to <folder>",
            escape_controls(file),
            list(candidates)
        ),
        ChangedRootsDiagnostic::DiffFileAmbiguous { file, candidates } => format!(
            "{} (candidates: {}); pass --diff-root <folder> to read diff paths relative to <folder>",
            escape_controls(file),
            list(candidates)
        ),
        ChangedRootsDiagnostic::HunkOutsideRoutines {
            file,
            start_line,
            end_line,
        } => format!(
            "{}:{}-{}",
            escape_controls(file),
            *start_line as u64,
            *end_line as u64
        ),
        ChangedRootsDiagnostic::DiffParseError {
            file: Some(file),
            detail,
        } => format!("{}: {}", escape_controls(file), escape_controls(detail)),
        ChangedRootsDiagnostic::DiffParseError { file: None, detail } => escape_controls(detail),
    };
    format!("changed-roots: {} {rest}", d.kind())
}

/// `s` with every control character, and every bidi or line-breaking format
/// character (U+200E/F, U+202A–E, U+2066–9, U+2028/9), in its `escape_default`
/// form (`\n`, `\t`, `\u{202e}`, …); every other character, `\` included, as
/// it is. It guards the `changed-roots:` lines only: the entry block's strings
/// (evidence, detail, reasons) are printed as before.
fn escape_controls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control()
            || matches!(
                c,
                '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                    | '\u{2028}' | '\u{2029}'
            )
        {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Project a query-level diagnostic into the envelope `{code, severity, message}`
/// shape. Mirrors `projectQueryDiagnostics` in contracts/digest.ts.
fn project_query_diagnostic(d: &DigestQueryDiagnostic) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    match d {
        DigestQueryDiagnostic::RootNotInSnapshot { routine_id } => {
            m.insert("code".into(), "DIAG-digest-root-not-in-snapshot".into());
            m.insert("severity".into(), "warning".into());
            m.insert(
                "message".into(),
                format!("Root routine '{routine_id}' not found in snapshot").into(),
            );
        }
        DigestQueryDiagnostic::NoCoverageRecord { routine_id } => {
            m.insert("code".into(), "DIAG-digest-no-coverage-record".into());
            m.insert("severity".into(), "warning".into());
            m.insert(
                "message".into(),
                format!("No coverage record for routine '{routine_id}'").into(),
            );
        }
    }
    serde_json::Value::Object(m)
}

/// Build the DocumentEnvelope<"digest", DigestPayload> as a serde_json::Value,
/// then serialize it like serialize_document_value (sorted keys, null-drop,
/// trailing \n). `test_coverage` (`--tests`, #254) is added after the null drop.
/// `include_reads` (#246) adds `payload.includeReads: true`; off, no key.
#[allow(clippy::too_many_arguments)] // document-envelope fields; grouping would obscure
pub fn project_digest_document(
    workspace_fp: &str,
    changed_input: &ChangedInputContract,
    changed_roots_result: &ChangedRootsResult,
    entries: &[DigestEntryFull],
    query_diagnostics: &[DigestQueryDiagnostic],
    diagnostics_json: serde_json::Value, // pre-built array from analyzer
    dependencies: &crate::program::resolve::full::LoadedDependencies,
    alsem_ver: &str,
    deterministic: bool,
    test_coverage: Option<serde_json::Value>,
    include_reads: bool,
    v3: Option<V3Inputs<'_>>,
) -> String {
    // payload.summary
    // rootsRequested = roots.length + queryResult.diagnostics.length (contracts/digest.ts:443).
    let roots_requested = changed_roots_result.roots.len() + query_diagnostics.len();
    let roots_resolved = entries.len();
    let total_effects: usize = entries.iter().map(|e| e.effects.len()).sum();
    let total_unresolved: usize = entries.iter().map(|e| e.unresolved.len()).sum();

    let mut summary = serde_json::Map::new();
    summary.insert("rootsRequested".into(), (roots_requested as u64).into());
    summary.insert("rootsResolved".into(), (roots_resolved as u64).into());
    summary.insert("totalEffects".into(), (total_effects as u64).into());
    summary.insert("totalUnresolved".into(), (total_unresolved as u64).into());

    // changed
    let mut changed = serde_json::Map::new();
    changed.insert("diffProvided".into(), changed_input.diff_provided.into());
    if let Some(ref files) = changed_input.files {
        let fv: Vec<serde_json::Value> = files.iter().map(|f| f.clone().into()).collect();
        changed.insert("files".into(), serde_json::Value::Array(fv));
    }
    if let Some(ref routines) = changed_input.routines {
        let rv: Vec<serde_json::Value> = routines.iter().map(|r| r.clone().into()).collect();
        changed.insert("routines".into(), serde_json::Value::Array(rv));
    }

    // entries: the 3.x projection (W2-2, design §4.4) unless `--schema 2`.
    let mut routines_table = None;
    let entries_val: Vec<serde_json::Value> = match &v3 {
        Some(v) => {
            let rt =
                crate::engine::l5::digest_v3::RoutineTable::build(entries, v.rollups, v.cx, v.opts);
            let out = entries
                .iter()
                .zip(v.rollups)
                .map(|(e, r)| crate::engine::l5::digest_v3::entry_v3(e, r, &rt, v.cx, v.opts))
                .collect();
            routines_table = Some(rt.table);
            out
        }
        None => entries.iter().map(entry_to_value).collect(),
    };

    // changedRootsDiagnostics
    let crd: Vec<serde_json::Value> = changed_roots_result
        .diagnostics
        .iter()
        .map(changed_roots_diagnostic_to_value)
        .collect();

    let mut payload = serde_json::Map::new();
    payload.insert("changed".into(), serde_json::Value::Object(changed));
    payload.insert(
        "changedRootsDiagnostics".into(),
        serde_json::Value::Array(crd),
    );
    payload.insert("dependencies".into(), dependencies.to_json());
    payload.insert("entries".into(), serde_json::Value::Array(entries_val));
    if include_reads {
        payload.insert("includeReads".into(), true.into());
    }
    // 2.0.0: every `line` in this document is 1-based.
    payload.insert(
        "lineBase".into(),
        crate::engine::line_base::LINE_BASE.into(),
    );
    payload.insert("summary".into(), serde_json::Value::Object(summary));
    payload.insert(
        "workspaceFingerprint".into(),
        workspace_fp.to_string().into(),
    );
    if let (Some(table), Some(v)) = (routines_table, &v3) {
        payload.insert("routines".into(), table);
        payload.insert("pathCap".into(), v.opts.path_cap.into());
        if v.opts.detail {
            payload.insert("detail".into(), true.into());
        }
    }

    // Envelope
    // makeEnvelope: deterministic ? pinned epoch : live ISO-8601 (#18 — was a dead
    // both-branches-identical block). The shared gate helper drives both cases.
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    // Envelope diagnostics = [...analyzerDiagnostics, ...projectQueryDiagnostics] (#5).
    let diagnostics_with_query = {
        let mut arr = match diagnostics_json {
            serde_json::Value::Array(a) => a,
            other => vec![other],
        };
        for qd in query_diagnostics {
            arr.push(project_query_diagnostic(qd));
        }
        serde_json::Value::Array(arr)
    };

    let mut env = serde_json::Map::new();
    env.insert("alsemVersion".into(), alsem_ver.into());
    env.insert("deterministic".into(), deterministic.into());
    env.insert("diagnostics".into(), diagnostics_with_query);
    env.insert("generatedAt".into(), generated_at.into());
    env.insert("kind".into(), "digest".into());
    env.insert("payload".into(), serde_json::Value::Object(payload));
    // 1.4.0: additive `detail.tempState` on DB_* effects (#65) and
    // `payload.dependencies` {mode, apps} (#66).
    // 1.5.0: additive routine.declarationOnly, routine.anchors and the
    // "declaration-only" coverage reason (#71).
    // 1.6.0: `detail.tempState` is a per-operation join in the effect's class;
    // additive `temp-state-incoherent` entry diagnostic with factId, operationId
    // and reason (#69 D4).
    // Still 1.6.0: `payload.testCoverage` is opt-in (`--tests`, #254) and additive.
    // Still 1.6.0: with `--include-reads` (#246, opt-in) a document may carry
    // `DB_READ` effects and `payload.includeReads: true`; without it, neither.
    // 1.7.0: additive default-on `diff-file-ambiguous` changed-roots diagnostic
    // with `file` and `candidates` (#77). Contract: a minor bump removes and
    // renames no field; it may add a field or kind, and (as 1.8.0 does) narrow
    // what an existing kind covers, so the same input can count differently;
    // `changedRootsDiagnostics[].kind` is an open set (a consumer treats an
    // unknown kind as a diagnostic, not a parse error); exit 0 does not mean
    // every diff file resolved.
    // 1.8.0: new `diff-file-not-al` changed-roots diagnostic with `file`
    // (#353): a diff path that is not AL-looking. NOT purely additive:
    // `diff-file-unmatched` and `diff-file-deleted` narrow to AL-looking paths
    // (the old side of an AL -> non-AL rename is `diff-file-deleted`). The one
    // exception: an `Only in` folder holding workspace files stays
    // `diff-file-unmatched`.
    // Still 1.8.0 (unreleased): new `file-ambiguous` changed-roots diagnostic
    // with `file` and `candidates` (#339): file inputs read paths as diffs do
    // (#77). Not purely additive on the file side either: a file input that
    // resolved as written may now be `file-ambiguous` and give no roots.
    // 2.0.0 (MCP output plan 1a): every `line` (evidence, routine anchors,
    // via-path hops, unresolved call sites, test coverage) is 1-based, and
    // `payload.lineBase: 1` says so. Major: a value's meaning changed.
    // 2.1.0 (MCP output plan 1d): a boundary group names its boundary
    // owner-qualified (`Codeunit X::OnRun`, was `<App>.OnRun`) and lists every
    // resource as `resources[]` ({id, display}; `detail.resourceCount` is its
    // length, `detail.resources` still the first ten); `unresolvedResources`
    // counts only DB writes, and a group of a type with no resources (COMMIT,
    // UI, ERROR, HTTP, FILE, BACKGROUND, ...) states no `resourceCount` /
    // `resources` (they were always "0" / "").
    // 2.2.0 (MCP output wave 2, W2-1, additive): a row the root cannot reach
    // under literal arguments carries `literalUnreachable: true`; a group
    // whose refined list is shorter than its raw one counts the refined list
    // and keeps `detail.resourcesOriginalCount`.
    // 3.0.0 (MCP output wave 2, W2-2, design §4.4): the rendering
    // redesign (`digest_v3`'s module doc); `--schema 2` keeps 2.2.0.
    // Still 3.0.0 (unreleased, round-1 c25): `testCoverage` with 0 tests
    // considered is `partial` (reason `no-test-code`, items `no-test-code`,
    // was `complete` / `no-path`), and sibling test apps digest does not load
    // add `test-apps-not-loaded` and `testAppsNotLoaded` (opt-in block).
    // 3.1.0 (output validation V3, §2.1, additive): every routine `display`
    // is the one display (member, AL quoting, the parameter types of an
    // overloaded name, a dependency object's number); routine rows and
    // summary rows gain `signature`, `member`, `selector` and `handle`, and
    // summary rows `stableId`.
    // 3.2.0 (output validation V4, F76, additive): `payload.result.scope`
    // (analysis, reasons, assumptions, limits, universe, scope).
    // 3.3.0 / 2.3.0 (output validation V6, F73, additive): an effect row's
    // `refutableGuarantees` (the labels valid for refutation, written only
    // when one is; a label not listed is descriptive only), and the summary
    // row's `guaranteeLabels`.
    let compact = v3.as_ref().is_some_and(|v| v.compact);
    let table_answers = v3.as_ref().map(|v| v.table_answers);
    env.insert(
        "schemaVersion".into(),
        if v3.is_some() { "3.3.0" } else { "2.3.0" }.into(),
    );

    // The block goes in after the null drop, so its `ratio: null` survives; the
    // keep-nulls sort then orders it (and the payload key it lands in).
    let mut doc = sort_and_drop_nulls(serde_json::Value::Object(env));
    if let Some(block) = test_coverage {
        doc["payload"]["testCoverage"] = block;
        doc = sort_keys_keep_nulls(doc);
    }
    // W2-2 gap (a): `--writes-table`, prove's own answer per entry.
    if let Some(answers) = table_answers.filter(|a| !a.is_empty()) {
        if let Some(es) = doc["payload"]["entries"].as_array_mut() {
            for e in es {
                let id = e["routine"]["stableId"].as_str().unwrap_or("").to_string();
                if let Some((values, _)) = answers.get(&id) {
                    e["tableAnswers"] = serde_json::json!(values);
                }
            }
        }
        doc = sort_keys_keep_nulls(doc);
    }
    // 3.2.0 (output validation V4, F76): `payload.result.scope`.
    if let Some(scope) = v3.as_ref().and_then(|v| v.scope.clone()) {
        doc["payload"]["result"] = serde_json::json!({"scope": scope});
        doc = sort_keys_keep_nulls(doc);
    }
    if compact {
        let mut s = serde_json::to_string(&doc).expect("serde_json serialization");
        s.push('\n');
        return s;
    }
    serialize_normalized_value(&doc)
}

/// What the 3.x projection reads beside the entries (W2-2).
pub struct V3Inputs<'a> {
    pub rollups: &'a [crate::engine::l5::routine_rollup::RoutineRollup],
    pub cx: &'a crate::engine::l5::routine_rollup::RollupCx<'a>,
    pub opts: crate::engine::l5::routine_rollup::RenderOpts,
    pub compact: bool,
    /// `--writes-table` (W2-2 gap (a)): `entries[].tableAnswers`.
    pub table_answers: &'a crate::engine::l5::prove::TableAnswers,
    /// Output validation V4 (F76): `payload.result.scope`.
    pub scope: Option<serde_json::Value>,
}

pub struct ChangedInputContract {
    pub files: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub diff_provided: bool,
}

// ---------------------------------------------------------------------------
// Human formatter (formatDigestHuman)
// ---------------------------------------------------------------------------

/// The human digest (W2-2, design §4.3): per root the explain card's effect
/// sections, rendered by the same functions (`routine_rollup`), then its
/// diagnostics as counts by kind and reason (each one, with its operation,
/// under `--detail`); then one line per changed-roots diagnostic in input
/// order, with no blank line between (#340).
pub fn format_digest_human_v3(
    entries: &[DigestEntryFull],
    rollups: &[crate::engine::l5::routine_rollup::RoutineRollup],
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    opts: crate::engine::l5::routine_rollup::RenderOpts,
    changed: &[ChangedRootsDiagnostic],
) -> String {
    use crate::engine::l5::routine_rollup as rr;
    let mut text = if entries.is_empty() {
        "digest: no changed roots resolved\n".to_string()
    } else {
        let mut lines: Vec<String> = Vec::new();
        // Output validation §5.2: with one root its summary line is the
        // answer's `answer:` line and the card keeps the rest; the analysis,
        // limits and scope are the header's third line (a card of several
        // prints its own analysis and limits only when they say more).
        let single = entries.len() == 1;
        for (entry, r) in entries.iter().zip(rollups) {
            lines.push(format!("--- {} ---", entry_display(cx, entry)));
            let facts = rr::header_facts(r, entry, &[]);
            // Several roots: each card's own summary line first, then the
            // lines a single card prints.
            let mut body: Vec<String> = Vec::new();
            if !single {
                body.push(format!("summary — {}", facts.answer()));
            }
            body.extend(facts.rest().iter().map(|l| (*l).to_string()));
            if !single {
                if facts.analysis != crate::engine::result_header::Analysis::Complete {
                    body.push(facts.analysis.human());
                }
                if !facts.limits.is_empty() {
                    body.push(format!("limits: {}", facts.limits.join("; ")));
                }
            }
            if entry.effects.is_empty() {
                body.push("effects: none".to_string());
            }
            // A section's shared path start, once (MCP output step 2).
            body.extend(rr::factor_prefix(rr::commit_lines(r, entry, cx, opts)));
            body.extend(rr::factor_prefix(rr::workspace_write_lines(
                r,
                entry,
                cx,
                opts,
                &|_| None,
            )));
            body.extend(rr::caller_dependent_lines(r, entry));
            body.extend(rr::beyond_lines(r, entry, cx, opts));
            body.extend(rr::factor_prefix(rr::no_path_lines(r, entry, cx, opts)));
            body.extend(rr::literal_dead_lines(r, opts));
            body.extend(rr::temporary_lines(r, opts));
            body.extend(rr::event_lines(r, cx, opts));
            body.extend(rr::kind_lines(r, entry, opts));
            body.extend(rr::workspace_gap_lines(r, entry, opts));
            body.extend(rr::dependency_gap_lines(r, opts));
            body.extend(diagnostic_lines(entry, opts));
            lines.extend(body.into_iter().map(|l| format!("  {l}")));
            lines.push(String::new());
        }
        lines.join("\n")
    };
    for d in changed {
        text.push_str(&changed_roots_diagnostic_human(d));
        text.push('\n');
    }
    text
}

/// The digest answer's four header lines (output validation §5.2): the
/// resolved roots, the answer (one root: its summary line), every root's
/// analysis and limits at once, the dependency depth and the dependencies
/// count, and what shows more.
#[allow(clippy::too_many_arguments)]
fn digest_header(
    entries: &[DigestEntryFull],
    rollups: &[crate::engine::l5::routine_rollup::RoutineRollup],
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    files: Option<&[String]>,
    routines: Option<&[String]>,
    diff: bool,
    render: crate::engine::l5::digest_v3::DigestRender,
    single_app: bool,
    deps: usize,
) -> crate::engine::result_header::Header {
    use crate::engine::follow_up::Input;
    use crate::engine::l5::routine_rollup as rr;
    use crate::engine::result_header::{Analysis, Header, dependencies_clause};
    let names: Vec<String> = entries.iter().map(|e| entry_display(cx, e)).collect();
    let listed = rr::fold(names.clone(), 3, "routines", false).joined();
    let roots = match names.len() {
        0 => "no root resolved".to_string(),
        1 => names[0].clone(),
        n => format!("{n} roots: {listed}"),
    };
    let mut modes: Vec<String> = Vec::new();
    if diff {
        modes.push("diff".to_string());
    }
    if let Some(f) = files {
        modes.push(format!("files {}", f.join(", ")));
    }
    if routines.is_some() && (diff || files.is_some()) {
        modes.push("routines".to_string());
    }
    let inputs = if modes.is_empty() {
        roots
    } else {
        format!("{} → {roots}", modes.join(" + "))
    };
    let mut h = Header::new("digest", inputs, single_app);
    let facts: Vec<rr::HeaderFacts> = entries
        .iter()
        .zip(rollups)
        .map(|(e, r)| rr::header_facts(r, e, &[]))
        .collect();
    h.answer = match &facts[..] {
        [] => "no answer — no changed root resolved".to_string(),
        [one] => one.answer().to_string(),
        many => format!(
            "{} roots, one card each; each card leads with its summary line",
            many.len()
        ),
    };
    let mut analysis = if facts.is_empty() {
        Analysis::NotAnalyzed("no changed root resolved".to_string())
    } else {
        Analysis::Complete
    };
    for f in &facts {
        analysis = analysis.and(f.analysis.clone());
        for l in &f.limits {
            if !h.limits.contains(l) {
                h.limits.push(l.clone());
            }
        }
    }
    h.analysis = analysis;
    let mut scope: Vec<String> = Vec::new();
    match &facts[..] {
        [] => {}
        [one] => scope.push(one.depth.clone()),
        _ => scope.push(
            "dependency code followed through every resolved call with literal arguments"
                .to_string(),
        ),
    }
    if !facts.is_empty() {
        scope.push("no lock facts".to_string());
    }
    scope.push(dependencies_clause(deps));
    h.scope = scope.join("; ");
    let s = render.surface;
    if !facts.is_empty() && !render.detail {
        h.more
            .push(format!("{} lists every list complete", Input::Detail.on(s)));
    }
    h.more
        .push(format!("{} for every effect row", Input::FormatJson.on(s)));
    h
}

/// `diagnostics: kind n; kind n (reason n)`, or each one with `detail`.
fn diagnostic_lines(
    entry: &DigestEntryFull,
    opts: crate::engine::l5::routine_rollup::RenderOpts,
) -> Vec<String> {
    let detail = opts.detail;
    if entry.entry_diagnostics.is_empty() {
        return Vec::new();
    }
    if detail {
        return entry
            .entry_diagnostics
            .iter()
            .map(|d| {
                format!(
                    "diagnostic: {}{}{}{}",
                    d.kind,
                    d.effect_type
                        .as_deref()
                        .map(|et| format!(" ({et})"))
                        .unwrap_or_default(),
                    d.reason.map(|r| format!(" reason={r}")).unwrap_or_default(),
                    d.operation_id
                        .as_deref()
                        .map(|o| format!(" op={o}"))
                        .unwrap_or_default()
                )
            })
            .collect();
    }
    let mut by: BTreeMap<&str, (usize, BTreeMap<&str, usize>)> = BTreeMap::new();
    for d in &entry.entry_diagnostics {
        let slot = by.entry(d.kind).or_default();
        slot.0 += 1;
        if let Some(r) = d.reason {
            *slot.1.entry(r).or_default() += 1;
        }
    }
    let parts: Vec<String> = by
        .into_iter()
        .map(|(k, (n, reasons))| {
            if reasons.is_empty() {
                format!("{k} {n}")
            } else {
                format!(
                    "{k} {n} ({})",
                    reasons
                        .iter()
                        .map(|(r, c)| format!("{r} {c}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        })
        .collect();
    vec![format!(
        "diagnostics: {} ({} lists each with its operation)",
        parts.join("; "),
        opts.detail_input()
    )]
}

// ---------------------------------------------------------------------------
// Top-level run_digest (used by the CLI command)
// ---------------------------------------------------------------------------

/// The changed roots as `digest --tests` counts them (#254): EVERY resolved root,
/// from its entry when the query built one, else `not-in-graph` (the query drops
/// a `root-not-in-snapshot` root, which must not leave the denominator).
fn coverage_changed_roots(
    roots: &[String],
    entries: &[DigestEntryFull],
    owners: &OwnerIndex<'_>,
) -> Vec<crate::engine::l5::test_coverage::ChangedRoutine> {
    use crate::engine::l5::test_coverage::{ChangedRoutine, ChangedShape};
    let by_id: HashMap<&str, &DigestEntryFull> =
        entries.iter().map(|e| (e.routine_id.as_str(), e)).collect();
    roots
        .iter()
        .map(|rid| match by_id.get(rid.as_str()) {
            Some(e) => ChangedRoutine {
                stable_id: rid.clone(),
                display: e.routine_display.clone(),
                object_display: e.object_display.clone(),
                anchor: e
                    .routine_anchor
                    .as_ref()
                    .and_then(|a| Some((a.file.clone()?, a.line?))),
                shape: if e.declaration_only {
                    ChangedShape::DeclarationOnly
                } else {
                    ChangedShape::Routine
                },
            },
            None => ChangedRoutine {
                stable_id: rid.clone(),
                display: rid.clone(),
                object_display: owners.object_display(rid),
                anchor: None,
                shape: ChangedShape::NotInGraph,
            },
        })
        .collect()
}

/// Result from run_digest_pipeline.
pub struct DigestRunResult {
    pub json_text: String,
    pub human_text: String,
    pub exit_code: u8, // 0 = OK, 2 = zero roots
    /// What the document says, in at most 8 KiB (MCP output plan
    /// 1b): [`digest_summary`]. The MCP server sends it beside the file the
    /// document goes to.
    pub summary: serde_json::Value,
}

#[cfg(test)]
use crate::engine::batch::output::SUMMARY_CAP_BYTES;

/// One root's summary row (output validation V4, §5.2): its identity (one
/// row per stable id, overloads apart by signature), its summary line, the
/// counts a reader decides on, its asked `tableAnswers` and its test status.
fn digest_entry_summary(
    e: &DigestEntryFull,
    r: &crate::engine::l5::routine_rollup::RoutineRollup,
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    answers: Option<&(Vec<serde_json::Value>, Vec<String>)>,
    test_status: &str,
) -> serde_json::Value {
    use serde_json::json;
    let mut by_type: BTreeMap<&str, u64> = BTreeMap::new();
    // §9.8 (F73): guarantee labels on effect rows, refutable and
    // descriptive-only counted apart.
    let (mut refutable, mut descriptive) = (0usize, 0usize);
    for f in &e.effects {
        *by_type.entry(f.effect_type.as_str()).or_default() += 1;
        for g in &f.guarantees {
            if guarantee_refutable(f, g) {
                refutable += 1;
            } else {
                descriptive += 1;
            }
        }
    }
    // V3 §2.1 (3.1.0): the one display, and the stable id, signature,
    // member and selector that tell overloads and member triggers apart.
    // Output validation V4 (§5.2, F42, F77): one compact row per stable id —
    // its summary line (`answer`), the counts a reader decides on, its test
    // status in the §8.1 words.
    let facts = crate::engine::l5::routine_rollup::header_facts(r, e, &[]);
    let mut v = json!({
        "routine": entry_display(cx, e),
        "stableId": e.routine_id,
        "analysis": facts.analysis.key(),
        "answer": facts.answer(),
        "effects": e.effects.len(),
        "effectsByType": by_type,
        // W2-2 (T-W2-13): the one RollupTotals serialization.
        "summary": r.totals.to_json(),
        "testStatus": test_status,
        // §9.8 (F73): guarantee labels over the effect rows; `refutable`
        // counts the labels the rows list in `refutableGuarantees`, and
        // `descriptiveOnly` the rest of `guarantees` (not listed there).
        "guaranteeLabels": {"refutable": refutable, "descriptiveOnly": descriptive},
    });
    for (k, x) in cx.names.identity_json(&e.routine_id) {
        if k != "display" {
            v[k] = x;
        }
    }
    // Output validation V2 (§5.2, F42): the asked-for `writesTables`
    // verdicts on the row, counts-only or not: what the caller asked for by
    // name is never only in the full document.
    if let Some((values, _)) = answers {
        v["tableAnswers"] = json!(
            values
                .iter()
                .map(|a| {
                    let blockers = a["blockedBy"].as_array().map_or(0, Vec::len);
                    let mut t = json!({
                        "table": {"name": a["table"]},
                        "answer": a.get("answer").cloned().unwrap_or(serde_json::Value::Null),
                        "blockers": blockers,
                    });
                    if let Some(b) = a["blockedBy"].as_array().and_then(|b| b.first()) {
                        t["firstBlocker"] = json!({"kind": b["kind"], "detail": b["detail"]});
                    }
                    if let Some(e) = a.get("error") {
                        t["error"] = e.clone();
                    }
                    t
                })
                .collect::<Vec<_>>()
        );
    }
    v
}

/// The digest's summary (MCP output plan 1b, D1/D2; output validation V4
/// §5.2): the document's totals and one row per root. Never the unresolved
/// call sites themselves: their counts. Kept under 8 KiB by
/// [`crate::engine::result_index::with_index`]: rows are dropped from the
/// end into `summaryTruncated`, and every row is in the result index.
pub fn digest_summary(
    entries: &[DigestEntryFull],
    rollups: &[crate::engine::l5::routine_rollup::RoutineRollup],
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    roots_requested: usize,
    changed_roots_diagnostics: usize,
) -> serde_json::Value {
    digest_summary_with(
        entries,
        rollups,
        cx,
        roots_requested,
        changed_roots_diagnostics,
        None,
    )
}

/// [`digest_summary`] with each row's `--writes-table` answers
/// (`tableAnswers`, output validation V2 §5.2).
pub(crate) fn digest_summary_with(
    entries: &[DigestEntryFull],
    rollups: &[crate::engine::l5::routine_rollup::RoutineRollup],
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    roots_requested: usize,
    changed_roots_diagnostics: usize,
    table_answers: Option<&crate::engine::l5::prove::TableAnswers>,
) -> serde_json::Value {
    digest_summary_tested(
        entries,
        rollups,
        cx,
        roots_requested,
        changed_roots_diagnostics,
        table_answers,
        None,
    )
}

/// Output validation V4 (§5.2, F42, F77): a root's test status in the §8.1
/// words, from the `--tests` coverage block (`not requested` without it).
fn root_test_status(
    id: &str,
    tc: Option<&crate::engine::l5::test_coverage::TestCoverage>,
) -> String {
    use crate::engine::l5::test_coverage::UncoveredReason as R;
    let Some(tc) = tc else {
        return "not requested".to_string();
    };
    if let Some(c) = tc.covered.iter().find(|c| c.stable_id == id) {
        return match c.reached_from.first() {
            Some(t) => format!("tests with a modeled path (nearest: {t})"),
            None => "tests with a modeled path".to_string(),
        };
    }
    match tc
        .uncovered
        .iter()
        .find(|u| u.stable_id == id)
        .map(|u| &u.reason)
    {
        Some(R::NotInGraph) => "not in projection",
        Some(R::NoTestCode | R::DeclarationOnly) => "not loaded",
        Some(R::PlatformEventSubscriber | R::DependencyEventSubscriber) => "analysis partial",
        Some(R::NoPath) if tc.coverage.status == "partial" => "analysis partial",
        Some(R::NoPath) => "no modeled path",
        None => "not in projection",
    }
    .to_string()
}

/// [`digest_summary_with`] with each row's test status (`--tests`): one
/// row per stable id (overloads never folded), the values several rows
/// share factored into `shared` (a row holds `{"shared": "sN"}`), the test
/// scope once; every row in the result index, as many in the summary as
/// fit its 8 KiB (`summaryTruncated` says which it left out).
pub(crate) fn digest_summary_tested(
    entries: &[DigestEntryFull],
    rollups: &[crate::engine::l5::routine_rollup::RoutineRollup],
    cx: &crate::engine::l5::routine_rollup::RollupCx<'_>,
    roots_requested: usize,
    changed_roots_diagnostics: usize,
    table_answers: Option<&crate::engine::l5::prove::TableAnswers>,
    test_coverage: Option<&crate::engine::l5::test_coverage::TestCoverage>,
) -> serde_json::Value {
    use serde_json::json;
    let total_unresolved: usize = entries.iter().map(|e| e.unresolved.len()).sum();
    let rows: Vec<serde_json::Value> = entries
        .iter()
        .zip(rollups)
        .map(|(e, r)| {
            digest_entry_summary(
                e,
                r,
                cx,
                table_answers.and_then(|t| t.get(&e.routine_id)),
                &root_test_status(&e.routine_id, test_coverage),
            )
        })
        .collect();
    // The values two or more rows share, once.
    const FACTORED: &[&str] = &["effectsByType", "summary"];
    let mut seen: Vec<(String, serde_json::Value, usize)> = Vec::new();
    for r in &rows {
        for k in FACTORED {
            let s = r[*k].to_string();
            match seen.iter_mut().find(|(t, _, _)| *t == s) {
                Some(x) => x.2 += 1,
                None => seen.push((s, r[*k].clone(), 1)),
            }
        }
    }
    let mut shared = serde_json::Map::new();
    let mut key_of: Vec<(String, String)> = Vec::new();
    for (s, v, n) in &seen {
        if *n > 1 {
            let key = format!("s{}", shared.len() + 1);
            shared.insert(key.clone(), v.clone());
            key_of.push((s.clone(), key));
        }
    }
    let listed: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut r = r.clone();
            for k in FACTORED {
                let s = r[*k].to_string();
                if let Some((_, key)) = key_of.iter().find(|(t, _)| *t == s) {
                    r[*k] = json!({"shared": key});
                }
            }
            r
        })
        .collect();
    let index_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut x = json!({});
            for k in [
                "routine",
                "stableId",
                "selector",
                "signature",
                "member",
                "analysis",
                "answer",
                "tableAnswers",
                "testStatus",
            ] {
                if let Some(v) = r.get(k) {
                    x[k] = v.clone();
                }
            }
            x
        })
        .collect();
    let mut v = json!({
        "kind": "digest",
        "rootsRequested": roots_requested,
        "rootsResolved": entries.len(),
        "totalEffects": entries.iter().map(|e| e.effects.len()).sum::<usize>(),
        "totalUnresolved": total_unresolved,
        "changedRootsDiagnostics": changed_roots_diagnostics,
        "routinesListed": listed.len(),
        "shared": shared,
        "routines": listed,
    });
    if let Some(tc) = test_coverage {
        v["testScope"] = json!({
            "status": tc.coverage.status,
            "reasons": tc.coverage.reasons,
            "testProceduresConsidered": tc.test_procedures_considered,
            "testAppsNotLoaded": tc.test_apps_not_loaded.iter().map(|a| a.name.clone())
                .collect::<Vec<_>>(),
        });
    }
    // Rows that do not all fit with their totals are listed compact (the
    // identity, summary line, verdicts and test status; the totals stay in
    // the document), so the summary names as many requested roots as it can
    // before it cuts any.
    let reserve = crate::engine::result_index::INDEX_RESERVE_BYTES;
    if v.to_string().len() + reserve > crate::engine::batch::output::SUMMARY_CAP_BYTES {
        v["shared"] = json!({});
        v["rowDetail"] = json!("compact: each root's totals are in the document");
        if let Some(rs) = v["routines"].as_array_mut() {
            for r in rs {
                if let Some(m) = r.as_object_mut() {
                    m.remove("summary");
                    m.remove("effectsByType");
                }
            }
        }
    }
    let mut v = crate::engine::result_index::with_index(v, "routines", "stableId", index_rows);
    v["routinesListed"] = json!(v["routines"].as_array().map_or(0, Vec::len));
    v
}

/// Run the full digest pipeline for a workspace.
///
/// `changed_files` are workspace-relative paths (e.g. "src/Foo.al").
/// `diff_text` is a unified diff string (for --diff).
/// `alsem_ver` is the version string (e.g. "cli-b-v1").
///
/// `single_app == false` (the default, #66) builds `alsem analyze`'s cross-app
/// base (`capability_cone::build_cross_app_base`): the workspace plus the
/// dependency code it demands, so a dependency table (Base Application's
/// Customer) resolves to `<appId>/table/N` with a name. `true` analyses the
/// workspace alone. With no dependency the two produce the same document apart
/// from `payload.dependencies.mode` (a contract test).
#[allow(clippy::too_many_arguments)] // CLI flags, one each
pub fn run_digest_pipeline(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    changed_files: Option<Vec<String>>,
    changed_routines: Option<Vec<String>>,
    diff_text: Option<String>,
    alsem_ver: &str,
    deterministic: bool,
    _max_paths_override: Option<usize>,
    single_app: bool,
    tests: bool,
) -> Result<DigestRunResult, String> {
    digest_pipeline_with(
        crate::engine::l5::detectors::default_detectors(),
        DigestRequest {
            inputs,
            changed_files,
            changed_routines,
            diff_text,
            alsem_ver,
            deterministic,
            single_app,
            tests,
            include_reads: false,
            diff_root: DiffRoot::Workspace,
        },
    )
}

/// [`run_digest_pipeline`] rendered as `render` says (the CLI's default is
/// `DigestRender::default()`, the 3.x document).
#[allow(clippy::too_many_arguments)] // CLI flags, one each
pub fn run_digest_pipeline_render(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    changed_files: Option<Vec<String>>,
    changed_routines: Option<Vec<String>>,
    diff_text: Option<String>,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
    render: crate::engine::l5::digest_v3::DigestRender,
) -> Result<DigestRunResult, String> {
    digest_pipeline_rendered(
        crate::engine::l5::detectors::default_detectors(),
        DigestRequest {
            inputs,
            changed_files,
            changed_routines,
            diff_text,
            alsem_ver,
            deterministic,
            single_app,
            tests: false,
            include_reads: false,
            diff_root: DiffRoot::Workspace,
        },
        render,
    )
}

/// The CLI inputs of one digest run ([`run_digest_pipeline`]'s flags).
/// `max_paths_override` is not here: the pipeline has never read it.
pub struct DigestRequest<'a> {
    /// Handed to the driver; compute reads `ViewCx::inputs` (#277 R7-1).
    pub inputs: &'a dyn WorkspaceRead,
    pub changed_files: Option<Vec<String>>,
    pub changed_routines: Option<Vec<String>>,
    pub diff_text: Option<String>,
    pub alsem_ver: &'a str,
    pub deterministic: bool,
    pub single_app: bool,
    /// `--tests`: add `payload.testCoverage` (#254).
    pub tests: bool,
    /// `--include-reads`: add `DB_READ` effects and `payload.includeReads` (#246).
    pub include_reads: bool,
    /// What `diff_text`'s and `changed_files`' paths are relative to (#77, #339).
    pub diff_root: DiffRoot,
}

/// [`run_digest_pipeline`] over a given detector set (#160 §4.4.4): the plan
/// is built from `detectors` by `digest_plan`, so a set without
/// `TRANSACTION_SPANS` makes `prepare` panic (A2.10).
pub(crate) fn digest_pipeline_with(
    detectors: Vec<crate::engine::l5::registry::Detector>,
    req: DigestRequest<'_>,
) -> Result<DigestRunResult, String> {
    // The in-process API keeps the 2.x document (`--schema 2`), which the
    // engine-fact tests read; the CLI's default is 3.x (W2-2).
    digest_pipeline_rendered(
        detectors,
        req,
        crate::engine::l5::digest_v3::DigestRender::schema2(),
    )
}

/// [`digest_pipeline_with`] rendered as `render` says (`--schema 2`,
/// `--detail`, `--compact`, `--max-paths`).
pub fn digest_pipeline_rendered(
    detectors: Vec<crate::engine::l5::registry::Detector>,
    req: DigestRequest<'_>,
    render: crate::engine::l5::digest_v3::DigestRender,
) -> Result<DigestRunResult, String> {
    let job = DigestJob {
        detectors,
        req,
        human: false,
        out: None,
        render,
        writes_tables: Vec::new(),
    };
    crate::engine::batch::run_doc(&job)
        .one::<Result<DigestRunResult, String>>()
        .unwrap_or_else(|f| Err(f.message()))
}

/// `alsem digest` as a batch job (#277 D1). Its side document is
/// `Result<DigestRunResult, String>`; the no-input failure (after the build,
/// before `prepare`) contributes nothing to the view's plan (D4.2).
pub struct DigestJob<'a> {
    detectors: Vec<crate::engine::l5::registry::Detector>,
    req: DigestRequest<'a>,
    /// `--format human`: the human text, else the JSON.
    human: bool,
    out: Option<&'a str>,
    /// `--schema 2`, `--detail`, `--compact`, `--max-paths` (design §5).
    render: crate::engine::l5::digest_v3::DigestRender,
    /// `--writes-table T` (W2-2 gap (a)): prove's answer per root and table.
    writes_tables: Vec<String>,
}

impl<'a> DigestJob<'a> {
    /// The default detectors over `req`, written as `--format` and `--out` say.
    pub fn new(req: DigestRequest<'a>, human: bool, out: Option<&'a str>) -> DigestJob<'a> {
        DigestJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            req,
            human,
            out,
            render: Default::default(),
            writes_tables: Vec::new(),
        }
    }

    /// `--writes-table T`, repeated (W2-2 gap (a)).
    #[must_use]
    pub fn with_writes_tables(mut self, tables: Vec<String>) -> Self {
        self.writes_tables = tables;
        self
    }

    /// The document's rendering (design §5).
    #[must_use]
    pub fn with_render(mut self, render: crate::engine::l5::digest_v3::DigestRender) -> Self {
        self.render = render;
        self
    }

    fn changed_input<'i>(
        &self,
        inputs: &'i crate::program::inputs::WorkspaceInputs,
    ) -> ChangedInput<'i> {
        ChangedInput {
            files: self.req.changed_files.clone(),
            routines: self.req.changed_routines.clone(),
            diff_text: self.req.diff_text.clone(),
            diff_root: self.req.diff_root.clone(),
            workspace_files: &inputs.al_files,
        }
    }
}

impl Job for DigestJob<'_> {
    fn name(&self) -> &'static str {
        "digest"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(
            self.req.inputs,
            self.req.single_app,
            false,
            ViewNeeds::MODEL,
        )
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        use crate::engine::session::digest_reads_plan;
        // The no-input check runs right after the build (an id or build error
        // still wins), before any detector run or solve (#160 §4.4.4).
        let has_input = self
            .req
            .changed_files
            .as_ref()
            .is_some_and(|f| !f.is_empty())
            || self
                .req
                .changed_routines
                .as_ref()
                .is_some_and(|r| !r.is_empty())
            || self
                .req
                .diff_text
                .as_ref()
                .is_some_and(|d| !d.trim().is_empty());
        if !has_input {
            return Err(JobFailure::line(
                "digest: at least one of changedFiles, changedRoutines, or diffText is required",
                1,
            ));
        }
        // D3: the roots are resolved before the composition, from the model
        // alone; the owner index dies here, the identity table with the
        // resolve step.
        let model = cx.model();
        let changed_roots = resolve_changed_roots(
            cx.ids(),
            model,
            &self.changed_input(cx.inputs()),
            primary_guid_of(model),
            &OwnerIndex::build(model),
        );
        // The plan composes the roots' raw facts, the tree over the cone graph
        // and their effects; no resolved root composes no tree (exit 2 below).
        let plan = digest_reads_plan(
            self.detectors.clone(),
            changed_roots.roots.iter().cloned().collect(),
            self.req.tests,
            self.req.include_reads,
        );
        // `--writes-table` reads the cones' gap counts (declared needs: only
        // then are they counted).
        let plan = if self.writes_tables.is_empty() {
            plan
        } else {
            crate::engine::session::counting_cone_gaps(plan)
        };
        Ok(PlanPart {
            plan,
            reads: vec![Cell::Det, Cell::L4, Cell::Effects],
            carry: Some(Box::new(changed_roots)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Ok::<_, String>(digest_view(
            cx,
            &self.req,
            self.render,
            &self.writes_tables,
        ))))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        // No model-instance id: `digest: could not compute modelInstanceId`.
        Ok(SideDoc::new(Err::<DigestRunResult, _>(
            failure.target.message("digest"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        match doc.one::<Result<DigestRunResult, String>>() {
            Err(f) => f,
            // A DigestPipelineError → the message verbatim (it already starts
            // with "digest:"), exit 1, mirroring cli/digest.ts's catch.
            Ok(r) => JobOutput::text_with_summary(
                self.out,
                r.map(|r| {
                    let text = if self.human {
                        r.human_text
                    } else {
                        r.json_text
                    };
                    (text, r.exit_code, r.summary)
                }),
            ),
        }
    }
}

/// Digest off its prepared view: the default run (the envelope diagnostics),
/// the tx map off its `det`, the effects off its tree and the L4 store off its
/// `l4` (#160 §4.4.4, step 6 D7).
fn digest_view(
    cx: &mut ViewCx<'_>,
    req: &DigestRequest<'_>,
    render: crate::engine::l5::digest_v3::DigestRender,
    writes_tables: &[String],
) -> DigestRunResult {
    let DigestRequest {
        changed_files,
        changed_routines,
        diff_text,
        alsem_ver,
        deterministic,
        tests,
        include_reads,
        ..
    } = req;
    let (alsem_ver, deterministic, tests) = (*alsem_ver, *deterministic, *tests);
    let changed_roots: ChangedRootsResult = cx.take_carry();
    let dependencies = cx.view().target().dependencies();
    let key = cx.keys()[0].clone();
    let (diagnostics_json, tx_ctx_map) = finish_default_run(cx, &key);
    let mut effects = if cx.has_tree() {
        cx.take_effects()
    } else {
        DigestEffects::default()
    };
    let resolved = cx.view().model();
    let workspace_fp =
        crate::engine::l5::snapshot_full::workspace_fingerprint_of(cx.inputs(), alsem_ver);

    // The L4 db-effect store, for DB_* `detail.tempState` (#65): the view's
    // `l4`, solved the way `query effects` solves it in the same mode (#66).
    let temp_states = DbTempStates::new(resolved, &cx.view().l4().bundle);
    // Built again (F1-C4): the first borrows the model, and `prepare` takes
    // the view mutably. It is small (one entry per primary routine).
    let owners = OwnerIndex::build(resolved);

    // The cones' facts a `--writes-table` answer reads, before the join
    // takes the cones.
    let cones = if writes_tables.is_empty() {
        HashMap::new()
    } else {
        crate::engine::l5::prove::cone_facts(&effects.unresolved)
    };
    // Run full digest query for the resolved roots
    let query_full = run_digest_query_full_from_entries(
        &mut effects,
        &changed_roots.roots,
        &tx_ctx_map,
        &temp_states,
        &changed_roots,
        &owners,
    );
    let entries = query_full.entries;
    // W2-2 gap (a): `--writes-table`, prove's own answer per root.
    let table_answers = crate::engine::l5::prove::table_answers(
        resolved,
        &temp_states,
        &entries,
        &cones,
        writes_tables,
    );
    let query_diagnostics = query_full.query_diagnostics;
    // W2-2: one rollup per root, read by the summary and the renderers.
    let rollup_cx = crate::engine::l5::routine_rollup::RollupCx::new(resolved, &dependencies.apps);
    let rollups: Vec<crate::engine::l5::routine_rollup::RoutineRollup> = entries
        .iter()
        .map(|e| crate::engine::l5::routine_rollup::RoutineRollup::build(e, &rollup_cx))
        .collect();
    // rootsRequested as `payload.summary` counts it (roots + query diagnostics).
    let roots_requested = changed_roots.roots.len() + query_diagnostics.len();
    // #254: the coverage block reads the graph the effects step built off the
    // tree (#160 step 6: the tree itself is released inside `prepare`). A
    // `--tests` plan always builds the tree, roots or not (`digest_plan`).
    let test_coverage = tests.then(|| {
        let changed = coverage_changed_roots(&changed_roots.roots, &entries, &owners);
        let mut tc = crate::engine::l5::test_coverage::compute_test_coverage(
            effects
                .coverage_graph
                .as_ref()
                .expect("a --tests plan builds the coverage graph"),
            &changed,
        );
        // The coverage graph holds the workspace app's own tests only. The
        // sibling test apps `alsem tests` would load (the same discovery,
        // files only, as explain's C-7) are named, and make the block
        // partial: a test there may reach what these do not (round-1 c25).
        let scope = crate::engine::test_select::scope::discover(
            &cx.inputs().workspace,
            &crate::engine::test_select::scope::ScopeRequest::default(),
        );
        tc.note_unloaded_test_apps(
            scope
                .apps
                .iter()
                .map(|a| crate::engine::l5::test_coverage::UnloadedTestApp {
                    name: a.name.clone(),
                    test_attribute_lines: a.test_attribute_lines,
                })
                .collect(),
        );
        tc
    });
    // Output validation V4 (§5.2, F42, F77): one row per root with its test
    // status, the rows also in the result index.
    let summary = digest_summary_tested(
        &entries,
        &rollups,
        &rollup_cx,
        roots_requested,
        changed_roots.diagnostics.len(),
        (!writes_tables.is_empty()).then_some(&table_answers),
        test_coverage.as_ref(),
    );
    // #69 D1.4: the query's inputs (the effects, the L4 store's join) are dead
    // here. Free them before the JSON projection. (The cells are released
    // after it: the 3.x projection names routines off the model.)
    drop(temp_states);
    drop(tx_ctx_map);
    drop(owners);
    drop(effects);

    let exit_code: u8 = if changed_roots.roots.is_empty() { 2 } else { 0 };

    let changed_input_contract = ChangedInputContract {
        files: changed_files.clone(),
        routines: changed_routines.clone(),
        diff_provided: diff_text.is_some(),
    };

    // Output validation §5.2: the header's facts, for the human answer's four
    // lines and the JSON's `payload.result.scope` (F76).
    let header = digest_header(
        &entries,
        &rollups,
        &rollup_cx,
        changed_files.as_deref(),
        changed_routines.as_deref(),
        diff_text.is_some(),
        render,
        req.single_app,
        dependencies.apps.len(),
    );

    // JSON
    let json_text = project_digest_document(
        &workspace_fp,
        &changed_input_contract,
        &changed_roots,
        &entries,
        &query_diagnostics,
        diagnostics_json,
        &dependencies,
        alsem_ver,
        deterministic,
        test_coverage.as_ref().map(|t| t.to_json_value()),
        *include_reads,
        (!render.schema2).then_some(V3Inputs {
            rollups: &rollups,
            cx: &rollup_cx,
            opts: render.opts(),
            compact: render.compact,
            table_answers: &table_answers,
            scope: Some(header.scope_json(serde_json::Map::new())),
        }),
    );

    // Human (W2-2, design §4.3): per root the explain card's effect sections,
    // rendered by the same functions. Output validation §5.3 (F32): the
    // dependencies are a count in the header's scope clause.
    let mut human_text = format_digest_human_v3(
        &entries,
        &rollups,
        &rollup_cx,
        render.opts(),
        &changed_roots.diagnostics,
    );
    // Output validation V2 (§4.7, F27): a change mode says first what it
    // computed. The digest does not compare the diff's lines with the
    // working tree (impact does), so it says so.
    if (diff_text.is_some() || changed_files.is_some()) && !entries.is_empty() {
        let impact = match render.surface {
            crate::engine::follow_up::Surface::Mcp => "alsem_impact with base",
            crate::engine::follow_up::Surface::Cli => "alsem impact --base",
        };
        human_text.insert_str(
            0,
            &format!(
                "computed: current effects of the {} {} touches, in the working tree ({}); no \
                 before/after computed — {impact} compares two revisions\n",
                crate::engine::l5::routine_rollup::plural(entries.len(), "routine", "routines"),
                if diff_text.is_some() {
                    "this diff"
                } else {
                    "these files"
                },
                if diff_text.is_some() {
                    "whether the diff's lines match it is not checked here"
                } else {
                    "every routine in the files"
                }
            ),
        );
    }
    if let Some(t) = &test_coverage {
        human_text.push_str(&t.human_line());
        human_text.push('\n');
    }
    // W2-2 gap (a): `--writes-table`, per root in entry order.
    for e in &entries {
        if let Some((_, lines)) = table_answers.get(&e.routine_id) {
            human_text.push_str(&format!(
                "table answers for {}:\n",
                entry_display(&rollup_cx, e)
            ));
            for l in lines {
                human_text.push_str(&format!("  {l}\n"));
            }
        }
    }
    human_text.insert_str(0, &header.human());
    // W2-7 (2g): readable dependency locations (the JSON keeps the guid).
    let human_text = dependencies.readable_locations(human_text);
    drop(rollup_cx);
    cx.release(Cell::L4);
    cx.release(Cell::Effects);

    DigestRunResult {
        json_text,
        human_text,
        exit_code,
        summary,
    }
}

/// A2.10's use test, shared by digest, prove and events: `pipeline` runs over
/// the default set minus every detector whose `requires` holds a bit of
/// `dropped`, on `tests/r0-corpus/<fixture>`, and must hit `prepare`'s panic
/// naming `missing`. Not `#[should_panic]`: a failed precondition assert would
/// also "pass" one.
#[cfg(test)]
pub(crate) fn assert_prepare_refuses_a_reduced_set(
    dropped: u32,
    missing: &str,
    fixture: &str,
    pipeline: impl FnOnce(
        Vec<crate::engine::l5::registry::Detector>,
        &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    ),
) {
    let set: Vec<_> = crate::engine::l5::detectors::default_detectors()
        .into_iter()
        .filter(|d| d.requires & dropped == 0)
        .collect();
    assert_eq!(
        set.iter().fold(0, |m, d| m | d.requires) & dropped,
        0,
        "precondition: the reduced union lacks the bits"
    );
    let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture);
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &ws,
        &crate::program::inputs::WorkspaceNeeds {
            fingerprint_rows: true,
            ..Default::default()
        },
    );
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pipeline(set, &inputs)));
    let Err(err) = r else {
        panic!("prepare must refuse a plan whose union lacks {missing}; the run completed")
    };
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    // `prepare`'s own `after_run` text: a bare `missing` would also match a
    // panic naming `CORE_SUMMARIES` when `missing` is `SUMMARIES`.
    assert!(msg.contains(&format!("reads {missing} off det")), "{msg}");
}

// #160 step 6 Task 2b-2 acceptance tests (digest on the cone graph, rule B).
#[cfg(test)]
#[path = "digest_rule_b_tests.rs"]
mod tests_rule_b;

#[cfg(test)]
mod tests {
    use super::*;

    /// A2.10: digest builds its plan from the set it is given.
    #[test]
    fn tx_context_without_d9() {
        assert_prepare_refuses_a_reduced_set(
            crate::engine::l5::registry::substrate::TRANSACTION_SPANS,
            "TRANSACTION_SPANS",
            "ws-d8-commit-in-tx",
            |set, inputs| {
                let _ = digest_pipeline_with(
                    set,
                    DigestRequest {
                        inputs,
                        changed_files: None,
                        changed_routines: Some(vec!["PostSalesDoc".to_string()]),
                        diff_text: None,
                        alsem_ver: "test",
                        deterministic: true,
                        single_app: false,
                        tests: false,
                        include_reads: false,
                        diff_root: DiffRoot::Workspace,
                    },
                );
            },
        );
    }

    /// #254 R4: a resolved root the query built no entry for (stated by hand: no
    /// entries at all) still reaches the coverage count, as `not-in-graph`.
    #[test]
    fn coverage_counts_roots_without_an_entry() {
        use crate::engine::l5::test_coverage::ChangedShape;
        let m = crate::engine::l5::test_support::empty_resolved();
        let owners = OwnerIndex::build(&m);
        let changed = coverage_changed_roots(&["gone".to_string()], &[], &owners);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].stable_id, "gone");
        assert_eq!(changed[0].shape, ChangedShape::NotInGraph);
    }

    // The #6 resolveSelector-cascade + normalize_display_key unit tests now live
    // in `selector_index::tests` (shared implementation, shared tests).

    // --- #76 normalize_input ----------------------------------------------

    /// #76 T2. `.` segments and non-leading empty segments go; the leading
    /// empty run (root, UNC) stays, at most 2; `..` is never cancelled.
    /// Mutation: cancel `..` -> `src/../x.al` becomes `x.al`.
    #[test]
    fn normalize_input_drops_dot_and_empty_segments() {
        for (input, expected) in [
            ("./src/X.al", "src/x.al"),
            ("src/./x.al", "src/x.al"),
            ("src//x.al", "src/x.al"),
            ("/src/x.al", "/src/x.al"),
            ("//srv/x.al", "//srv/x.al"),
            ("../x.al", "../x.al"),
            ("src/../x.al", "src/../x.al"),
            ("ws:src\\x.al", "src/x.al"),
            (".//src/x.al", "src/x.al"),
            ("///src/x.al", "//src/x.al"),
            ("src/x.al/", "src/x.al"),
            (".\\SRC\\x.al", "src/x.al"),
        ] {
            assert_eq!(normalize_input(input), expected, "{input}");
        }
    }

    // --- #77 diff prefixes --------------------------------------------------

    /// #77 A8b. Folder names compare exactly on every platform.
    /// Mutation: fold case (`eq_ignore_ascii_case`) -> `Cloud`/`cloud` match.
    #[test]
    fn same_folder_is_exact() {
        assert!(same_folder("Cloud", "Cloud"));
        assert!(!same_folder("Cloud", "cloud"));
    }

    /// #77 A24. The prefix is removed folder by folder, exactly; the rest keys
    /// like `normalize_input`; nothing left names nothing.
    /// Mutation: a string `starts_with` instead of the per-folder comparison ->
    /// `Cloudy/x.al` gives `y/x.al`.
    #[test]
    fn strip_diff_prefix_cases() {
        for (prefix, path, expected) in [
            ("Cloud", "Cloud/src/x.al", Some("src/x.al")),
            ("Cloud", "./Cloud/SRC/x.al", Some("src/x.al")),
            ("Cloud", "Cloudy/x.al", None),
            ("Cloud", "Cloud", None),
            ("Cloud", "Cloud/", None),
            ("Cloud", "cloud/src/x.al", None),
            ("a/b", "a/b/X.al", Some("x.al")),
            ("a/b", "a/x.al", None),
        ] {
            assert_eq!(
                strip_diff_prefix(prefix, path).as_deref(),
                expected,
                "{prefix} {path}"
            );
        }
    }

    // --- #12 autoDetectChanged --------------------------------------------

    #[test]
    fn auto_detect_existing_path_is_diff() {
        let r = auto_detect_changed_with("some.patch", |_| true);
        assert_eq!(r, ChangedAutoDetect::Diff("some.patch".to_string()));
    }

    #[test]
    fn auto_detect_al_comma_list_is_files() {
        let r = auto_detect_changed_with("src/A.al, src/B.al", |_| false);
        assert_eq!(
            r,
            ChangedAutoDetect::Files(vec!["src/A.al".into(), "src/B.al".into()])
        );
    }

    #[test]
    fn auto_detect_nonexistent_nonal_is_routines() {
        // A non-existent `--changed bad.patch` → NOT a file, not `.al` → routine
        // selector ["bad.patch"] (graceful miscategorization, not an OS error) (#12).
        let r = auto_detect_changed_with("bad.patch", |_| false);
        assert_eq!(r, ChangedAutoDetect::Routines(vec!["bad.patch".into()]));
    }

    #[test]
    fn auto_detect_routine_names() {
        let r = auto_detect_changed_with("MyCodeunit::DoWork, Other", |_| false);
        assert_eq!(
            r,
            ChangedAutoDetect::Routines(vec!["MyCodeunit::DoWork".into(), "Other".into()])
        );
    }

    // --- #69 D4: the per-operation temp-state join (acceptance row A4b) -----
    //
    // Hand-stated L4 rows `(operation id, temp state)`; no pipeline. Stated
    // limit: `NoL4Row` cannot be produced through the pipeline without a cone/L4
    // split, so this `--lib` test is its only pin.

    fn ops(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A4b. op1 has a row in the effect's class (kt) and one outside it; op2 has
    /// rows ONLY in the other class. states = op1's kt rows ∪ ALL of op2's rows,
    /// with a `ClassAbsent(op2)` problem. Mutation: union the ops before
    /// checking → "ClassAbsent not reported".
    #[test]
    fn temp_join_class_absent_contributes_all_rows() {
        let rows = [
            ("op1", TempStateKind::Known(true)),
            ("op1", TempStateKind::Known(false)),
            ("op2", TempStateKind::Known(false)),
            ("op2", TempStateKind::ParameterDependent(0)),
        ];
        let join = join_temp_states(true, &ops(&["op1", "op2"]), &rows);
        assert!(
            join.problems
                .iter()
                .any(|(op, p)| op == "op2" && *p == JoinProblem::ClassAbsent),
            "ClassAbsent not reported: {:?}",
            join.problems
        );
        assert!(
            !join.problems.iter().any(|(op, _)| op == "op1"),
            "op1 is in class, no problem expected: {:?}",
            join.problems
        );
        let want: BTreeSet<TempStateKind> = [
            TempStateKind::Known(true),
            TempStateKind::Known(false),
            TempStateKind::ParameterDependent(0),
        ]
        .into_iter()
        .collect();
        assert_eq!(join.states, want, "op1 filtered to kt, op2 all rows");
        assert!(
            !join.states.is_empty() && join.render().is_some(),
            "a ClassAbsent op still renders the field"
        );
    }

    /// A4b, precedence (R2-F05): op3 has no L4 row → the field is absent (never
    /// invent a value) and `NoL4Row(op3)` is reported, even though op1 joins.
    #[test]
    fn temp_join_no_l4_row_makes_the_field_absent() {
        let rows = [
            ("op1", TempStateKind::Known(true)),
            ("op2", TempStateKind::Known(false)),
        ];
        let join = join_temp_states(true, &ops(&["op1", "op2", "op3"]), &rows);
        assert!(
            join.problems
                .iter()
                .any(|(op, p)| op == "op3" && *p == JoinProblem::NoL4Row),
            "NoL4Row not reported: {:?}",
            join.problems
        );
        assert!(
            join.problems
                .iter()
                .any(|(op, p)| op == "op2" && *p == JoinProblem::ClassAbsent),
            "ClassAbsent not reported: {:?}",
            join.problems
        );
        assert_eq!(join.render(), None, "NoL4Row: the field must be absent");
    }

    fn entry_with(diagnostics: Vec<EntryDiagnostic>) -> DigestEntryFull {
        DigestEntryFull {
            routine_id: "r".to_string(),
            routine_display: "R".to_string(),
            object_display: String::new(),
            routine_anchor: None,
            routine_anchors: Vec::new(),
            coverage_status: "complete".to_string(),
            coverage_reasons: Vec::new(),
            effects: Vec::new(),
            unresolved: Vec::new(),
            unresolved_traversal: UnresolvedTraversal {
                truncated: false,
                max_depth: 64,
                visited_routines: 1,
            },
            entry_diagnostics: diagnostics,
            declaration_only: false,
        }
    }

    /// A digest effect of `ty` with `detail` and (for a boundary group) its
    /// whole resource set.
    fn effect(ty: &str, detail: &[(&str, &str)], group: &[(&str, &str)]) -> DigestEffectFull {
        DigestEffectFull {
            effect_type: ty.to_string(),
            detail: detail
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            provenance: "transitive",
            evidence: FullAnchor {
                source_kind: "unavailable",
                file: None,
                line: None,
                column: None,
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: crate::engine::l5::conditionality::UNKNOWN,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "f".to_string(),
            scoped_guarantees: vec![],
            group_resources: group
                .iter()
                .map(|(i, d)| (i.to_string(), d.to_string()))
                .collect(),
            group_resource_paths: vec![],
            group_terminal_path: None,
            own_path: None,
            origin_operation_ids: vec![],
            effect_is_kt: false,
            literal_unreachable: false,
            crossing: None,
        }
    }

    /// Output validation §8.2 (F53, c15) and §9.8 (F73): the summary row
    /// counts the live COMMIT sites and those the root's literal arguments
    /// rule out apart (3 COMMIT rows = 1 site + 2 more), and guarantee
    /// labels refutable and descriptive-only apart; an effect row lists its
    /// refutable labels in `refutableGuarantees` (only when one is; a label
    /// not listed is descriptive only). Hand-stated rows.
    ///
    /// DISCRIMINATION (2026-11-06): not counting own literal-dead COMMIT rows
    /// (the `commit_dead_sites += 1` in `RoutineRollup::build` removed) fails
    /// at "literalDead"; restored, passes. `guarantee_refutable` answering
    /// `false` for every label fails at "refutable row" (the row with
    /// WRITE_BEFORE_COMMIT lists nothing); restored, passes.
    #[test]
    fn summary_counts_dead_commits_and_guarantee_kinds_apart() {
        use crate::engine::l5::ordering_engine::ScopedGuarantee;
        let sg = |label: &'static str, refutable: bool| ScopedGuarantee {
            label,
            scope: "root",
            write_occurrence_id: None,
            commit_occurrence_id: None,
            io_occurrence_id: None,
            return_occurrence_id: None,
            supporting_edge_ids: vec![],
            commit_effectiveness: None,
            intervening_boundary: "none",
            valid_for_refutation: refutable,
        };
        let mut live = effect("COMMIT", &[], &[]);
        live.guarantees = vec!["COMMIT_REACHABLE".into(), "WRITE_BEFORE_COMMIT".into()];
        live.scoped_guarantees = vec![
            sg("COMMIT_REACHABLE", false),
            sg("WRITE_BEFORE_COMMIT", true),
        ];
        let mut dead1 = effect("COMMIT", &[], &[]);
        dead1.literal_unreachable = true;
        let mut dead2 = effect("COMMIT", &[], &[]);
        dead2.literal_unreachable = true;
        let mut e = entry_with(vec![]);
        e.effects = vec![live, dead1, dead2];
        assert_eq!(
            effect_to_value(&e.effects[0])["refutableGuarantees"],
            serde_json::json!(["WRITE_BEFORE_COMMIT"]),
            "refutable row"
        );
        assert!(
            effect_to_value(&e.effects[1])
                .get("refutableGuarantees")
                .is_none(),
            "no refutable label, no field"
        );
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        let s = digest_summary(&[e], &[r], &cx, 1, 0);
        let row = &s["routines"][0];
        assert_eq!(row["effectsByType"]["COMMIT"], 3, "{s}");
        let c = &row["summary"]["commits"];
        assert_eq!(c["ownSites"], 1, "{s}");
        assert_eq!(c["literalDead"]["sites"], 2, "literalDead: {s}");
        assert_eq!(
            row["guaranteeLabels"],
            serde_json::json!({"refutable": 1, "descriptiveOnly": 1}),
            "refutable 1: {s}"
        );
    }

    /// Output validation §8.2 (F53, c05): when no dependency call writes a
    /// record that may persist, `writes beyond dependency calls` is one
    /// absence verdict: `no (every candidate decided; …)` only when no
    /// dependency call is unresolved, else `none observed (…)`. Hand-stated
    /// rows: one crossing that only commits (writes no table), with and
    /// without an unresolved call in dependency code.
    ///
    /// DISCRIMINATION (2026-11-06): deciding the verdict without the
    /// dependency gaps (`r.totals.gaps_dependency == 0 &&` removed in
    /// `beyond_lines`) prints "no" with a gap: fails at "a gap is not a
    /// no"; restored, passes.
    #[test]
    fn beyond_with_no_writing_call_is_one_verdict() {
        let opts = crate::engine::l5::routine_rollup::RenderOpts {
            detail: false,
            path_cap: 1,
            surface: crate::engine::follow_up::Surface::Cli,
        };
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let line = |gap: bool| {
            let mut e = entry_with(vec![]);
            e.effects = vec![effect(
                "COMMIT",
                &[("boundary", "Codeunit Base Poster::OnRun")],
                &[],
            )];
            if gap {
                e.unresolved = vec![unresolved(
                    "Codeunit.Run(<dynamic>)",
                    "dynamic-target",
                    "dep:g:src/B.al",
                )];
            }
            let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
            crate::engine::l5::routine_rollup::beyond_lines(&r, &e, &cx, opts)
        };
        let clean = line(false);
        assert_eq!(clean.len(), 1, "{clean:?}");
        assert!(
            clean[0].starts_with("writes beyond dependency calls: no (every candidate decided; "),
            "{clean:?}"
        );
        let gap = line(true);
        assert!(
            gap[0].starts_with("writes beyond dependency calls: none observed ("),
            "a gap is not a no: {gap:?}"
        );
    }

    fn unresolved(callee: &str, status: &str, file: &str) -> UnresolvedConeItem {
        UnresolvedConeItem {
            callsite_id: format!("{file}/{callee}"),
            callsite_file: Some(file.to_string()),
            callsite_line: Some(3),
            callsite_column: Some(8),
            callee_display: callee.to_string(),
            status: status.to_string(),
            candidates: Some(vec!["a-very-long-candidate-stable-id".to_string(); 10]),
            open_world: None,
            closed_world_followable: false,
            publisher_scoped: false,
            owning_routine: "r".to_string(),
            owning_routine_display: "R".to_string(),
        }
    }

    /// MCP output plan 1b (D1, D2): the summary counts what decides whether to
    /// open the file (effects by type, physical tables written split workspace
    /// and dependency with a boundary group's WHOLE set, temporary tables,
    /// commits, boundaries) and gives the unresolved call sites as counts and
    /// top callees, never the items. Hand-stated: a workspace insert, a
    /// temporary delete, a 12-table dependency group, a commit group, three
    /// unresolved sites (two in the workspace).
    ///
    /// Discrimination (2026-11-04): embedding `e.unresolved`'s items in the
    /// entry summary fails at "no call-site item"; ignoring `group_resources`
    /// fails at the dependency count (0, not 12); restored, it passes.
    #[test]
    fn digest_summary_counts_and_names_without_the_unresolved_items() {
        let dep: Vec<(String, String)> = (1..=12)
            .map(|i| (format!("dep/table/{i}"), format!("Base Ledger {i:02}")))
            .collect();
        let dep_refs: Vec<(&str, &str)> =
            dep.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let mut e = entry_with(vec![]);
        e.object_display = "Codeunit Out Issue Document".to_string();
        e.routine_display = "IssueDoc".to_string();
        e.coverage_status = "partial".to_string();
        e.coverage_reasons = vec!["interface-open-world".to_string()];
        e.effects = vec![
            effect(
                "DB_INSERT",
                &[
                    ("resourceId", "ws/table/1"),
                    ("resourceDisplay", "Out Log"),
                    ("tempState", "known(false)"),
                ],
                &[],
            ),
            effect(
                "DB_DELETE",
                &[
                    ("resourceId", "ws/table/1"),
                    ("resourceDisplay", "Out Log"),
                    ("tempState", "known(true)"),
                ],
                &[],
            ),
            effect(
                "DB_INSERT",
                &[
                    ("boundary", "Codeunit Base Poster::OnRun"),
                    ("resources", "Base Ledger 01"),
                ],
                &dep_refs,
            ),
            effect(
                "COMMIT",
                &[("boundary", "Codeunit Base Poster::OnRun")],
                &[],
            ),
        ];
        e.unresolved = vec![
            unresolved("Codeunit.Run(<dynamic>)", "dynamic-target", "ws:src/A.al"),
            unresolved("Handler.Handle", "polymorphic", "ws:src/A.al"),
            unresolved(
                "Codeunit.Run(<dynamic>)",
                "dynamic-target",
                "dep:g:src/B.al",
            ),
        ];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        let s = digest_summary(&[e], &[r], &cx, 1, 0);
        let text = s.to_string();
        assert!(text.len() <= SUMMARY_CAP_BYTES, "{text}");
        assert!(
            !text.contains("callsiteAnchor") && !text.contains("a-very-long-candidate"),
            "no call-site item: {text}"
        );
        // Output validation V4 (§5.2): one compact row per root.
        let r = &s["routines"][0];
        assert_eq!(r["routine"], "Codeunit Out Issue Document::IssueDoc");
        assert_eq!(r["effectsByType"]["DB_INSERT"], 2, "{s}");
        // W2-2: the one RollupTotals serialization.
        let t = &r["summary"];
        assert_eq!(t["commits"]["answer"], "may", "{s}");
        let w = &t["mayWriteTables"];
        assert_eq!(
            (w["ownedByApp"].as_u64(), w["byDependencies"].as_u64()),
            (Some(1), Some(12)),
            "{s}"
        );
        assert_eq!(w["count"], 13);
        let g = &t["gaps"];
        assert_eq!(
            (g["workspace"].as_u64(), g["dependency"].as_u64()),
            (Some(2), Some(1)),
            "{s}"
        );
        assert_eq!(r["testStatus"], "not requested", "{s}");
        assert!(
            r["answer"]
                .as_str()
                .is_some_and(|a| a.starts_with("writes: ")),
            "{s}"
        );
        assert!(s.get("countsOnly").is_none(), "{s}");
        assert_eq!(s["indexRows"].as_array().map(Vec::len), Some(1), "{s}");
    }

    /// W2-2, T-W2-1: a rollup total is a union by identity, never a sum of
    /// section counts. Hand-stated: `Shared` written in workspace code AND
    /// listed by a group beyond `Codeunit Base Poster::OnRun`, `Other` listed by
    /// two groups (Insert and Modify) of that one crossing, and a temporary
    /// `Scratch` insert, and a caller-dependent `Arg` modify. Expected by hand:
    /// 2 may-persist tables (both owned by dependencies), 1 entry point,
    /// `Scratch` temporary-only, `Arg` caller-dependent and not may-persist.
    ///
    /// Discrimination (2026-11-04): computing `may_write_tables` as the sum of
    /// the workspace-code records and every crossing's table count fails (3,
    /// not 2); counting a caller-dependent contribution as physical (its arm
    /// adding to `live_tables`) fails too (3, not 2); each restored, it passes.
    #[test]
    fn rollup_totals_are_unions_by_identity() {
        let mut own = effect(
            "DB_INSERT",
            &[
                ("resourceId", "dep/table/1"),
                ("resourceDisplay", "Shared"),
                ("tempState", "known(false)"),
            ],
            &[],
        );
        own.evidence.source_kind = "source";
        own.evidence.file = Some("ws:src/A.al".to_string());
        own.evidence.line = Some(4);
        let mut temp = effect(
            "DB_INSERT",
            &[
                ("resourceId", "ws/table/9"),
                ("resourceDisplay", "Scratch"),
                ("tempState", "known(true)"),
            ],
            &[],
        );
        temp.evidence.source_kind = "source";
        let group = |ty: &str, res: &[(&str, &str)]| {
            effect(ty, &[("boundary", "Codeunit Base Poster::OnRun")], res)
        };
        let mut arg = effect(
            "DB_MODIFY",
            &[
                ("resourceId", "ws/table/5"),
                ("resourceDisplay", "Arg"),
                ("tempState", "parameter-dependent(0)"),
            ],
            &[],
        );
        arg.evidence.source_kind = "source";
        let mut e = entry_with(vec![]);
        e.effects = vec![
            own,
            temp,
            arg,
            group(
                "DB_INSERT",
                &[("dep/table/1", "Shared"), ("dep/table/2", "Other")],
            ),
            group("DB_MODIFY", &[("dep/table/2", "Other")]),
        ];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        assert_eq!(r.workspace_code.len(), 1, "one workspace-code record");
        assert_eq!(r.crossings.len(), 1, "one crossing, two group rows");
        let t = &r.totals;
        assert_eq!(t.may_write_tables, 2, "{t:?}");
        assert_eq!((t.owned_by_app, t.by_dependencies), (0, 2), "{t:?}");
        assert_eq!(t.entry_points, 1, "{t:?}");
        assert_eq!(t.temporary_only_tables, 1, "{t:?}");
        assert_eq!(
            r.crossings[0].by_type["DB_INSERT"].len(),
            2,
            "tables per kind"
        );
        // T-W2-20: the caller-dependent `Arg` write is its own section and
        // never a may-persist table.
        assert_eq!(t.caller_dependent_tables, 1, "{t:?}");
        assert_eq!(r.caller_dependent.len(), 1);
        assert!(!r.may_write.contains_key("ws/table/5"), "{t:?}");
    }

    /// W2-5 (2e): an effect type no kind family names is never dropped; it is
    /// the explicit other-kinds bucket, counted per place (an own site and a
    /// crossing) and printed by its type name. The type is stated by
    /// assignment: every type the digest maps today has a family, so no AL
    /// source makes one.
    ///
    /// Discrimination (2026-11-04): `family_of` returning `None` for an
    /// unknown type (the old drop) fails at "the own site is counted";
    /// restored, passes.
    #[test]
    fn an_effect_type_with_no_family_is_its_own_bucket() {
        use crate::engine::l5::routine_rollup::{OTHER_KINDS, kind_lines};
        let mut own = effect("FUTURE_KIND", &[], &[]);
        own.evidence.source_kind = "source";
        own.evidence.file = Some("ws:src/A.al".to_string());
        own.evidence.line = Some(4);
        let group = effect(
            "FUTURE_KIND",
            &[("boundary", "Codeunit Base Poster::OnRun")],
            &[],
        );
        let mut e = entry_with(vec![]);
        e.effects = vec![own, group];
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let r = crate::engine::l5::routine_rollup::RoutineRollup::build(&e, &cx);
        assert_eq!(
            r.kind_sites.get(OTHER_KINDS).map(Vec::len),
            Some(1),
            "the own site is counted: {:?}",
            r.kind_sites
        );
        assert_eq!(r.totals.kinds.get(OTHER_KINDS), Some(&2), "{:?}", r.totals);
        let lines = kind_lines(
            &r,
            &e,
            crate::engine::l5::routine_rollup::RenderOpts::new(false, None),
        );
        assert_eq!(
            lines,
            vec![
                "other effect kinds at 2 places: FUTURE_KIND at ws:src/A.al:5 (condition not resolved); \
                 FUTURE_KIND beyond Codeunit Base Poster::OnRun"
                    .to_string()
            ]
        );
    }

    /// A model whose primary app is `ws`.
    fn ws_model() -> Model {
        let mut m = crate::engine::l5::test_support::empty_resolved();
        m.primary_app = Some(crate::engine::gate::app_attribution::App {
            app_guid: "ws".to_string(),
            publisher: String::new(),
            name: String::new(),
            version: String::new(),
        });
        m
    }

    /// MCP output plan 1b: past [`SUMMARY_CAP_BYTES`] the summary degrades to
    /// counts (no names), then lists fewer routines; it never exceeds the cap.
    /// Hand-stated: 40 routines, each writing 12 long-named dependency tables.
    ///
    /// Discrimination (2026-11-04): returning `full` without the cap check
    /// fails at the size assertion (17,495 bytes); restored, it passes.
    #[test]
    fn digest_summary_degrades_to_counts_under_the_cap() {
        let dep: Vec<(String, String)> = (1..=12)
            .map(|i| {
                (
                    format!("dep/table/{i}"),
                    format!("A Rather Long Dependency Table Name {i:02}"),
                )
            })
            .collect();
        let dep_refs: Vec<(&str, &str)> =
            dep.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let entries: Vec<DigestEntryFull> = (0..40)
            .map(|i| {
                let mut e = entry_with(vec![]);
                e.object_display = format!("Codeunit A Long Object Name Number {i}");
                e.routine_display = format!("ALongRoutineName{i}");
                e.effects = vec![effect(
                    "DB_MODIFY",
                    &[("boundary", "Codeunit X::OnRun")],
                    &dep_refs,
                )];
                e
            })
            .collect();
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let rollups: Vec<_> = entries
            .iter()
            .map(|e| crate::engine::l5::routine_rollup::RoutineRollup::build(e, &cx))
            .collect();
        // Output validation V4 (§5.2): the rows past the cap go to the result
        // index (all 40), the summary says which it left out.
        let mut s = digest_summary(&entries, &rollups, &cx, 40, 0);
        let rows = s
            .as_object_mut()
            .unwrap()
            .remove(crate::engine::result_index::ROWS_KEY)
            .unwrap();
        let text = s.to_string();
        assert!(text.len() <= SUMMARY_CAP_BYTES, "{} bytes", text.len());
        assert_eq!(rows.as_array().map(Vec::len), Some(40), "{rows}");
        let listed = s["routinesListed"].as_u64().unwrap();
        assert!((1..40).contains(&listed), "{text}");
        assert_eq!(s["summaryTruncated"]["rowsListed"], listed, "{text}");
        assert_eq!(s["summaryTruncated"]["rowsTotal"], 40, "{text}");
        assert_eq!(s["rootsResolved"], 40);
    }

    /// #69 I3, construction: the failed join becomes one diagnostic per problem
    /// carrying the EFFECT's `factId` and the FAILED op. Mutation: swap
    /// `fact_id` and `operation_id` → "factId".
    #[test]
    fn incoherence_diagnostics_carry_fact_id_and_failed_op() {
        let d = incoherence_diagnostics(
            "DB_INSERT",
            "Subject",
            "fact-1",
            vec![
                ("op-a".to_string(), JoinProblem::NoL4Row),
                ("op-b".to_string(), JoinProblem::ClassAbsent),
            ],
        );
        assert_eq!(d.len(), 2, "one diagnostic per problem");
        assert_eq!(d[0].kind, "temp-state-incoherent");
        assert_eq!(d[0].effect_type.as_deref(), Some("DB_INSERT"));
        assert_eq!(d[0].fact_subject.as_deref(), Some("Subject"));
        assert_eq!(d[0].fact_id.as_deref(), Some("fact-1"), "factId");
        assert_eq!(d[0].operation_id.as_deref(), Some("op-a"), "operationId");
        assert_eq!(d[0].reason, Some("no-l4-row"));
        assert_eq!(d[1].operation_id.as_deref(), Some("op-b"));
        assert_eq!(d[1].reason, Some("class-absent"));
    }

    /// #69 I3, serialization: the three new keys reach the wire for
    /// `temp-state-incoherent` and are absent for `evidence-unavailable`.
    /// Mutation: delete the `dm.insert("factId" …)` arm (and likewise the
    /// `operationId` and `reason` arms) → "factId".
    #[test]
    fn entry_to_value_serializes_incoherence_fields() {
        let mut diags = incoherence_diagnostics(
            "DB_MODIFY",
            "Subject",
            "fact-1",
            vec![("op-a".to_string(), JoinProblem::ClassAbsent)],
        );
        diags.push(EntryDiagnostic {
            kind: "evidence-unavailable",
            effect_type: Some("DB_MODIFY".to_string()),
            fact_subject: Some("Subject".to_string()),
            fact_id: None,
            operation_id: None,
            reason: None,
        });
        let v = entry_to_value(&entry_with(diags));
        let ed = v["entryDiagnostics"].as_array().expect("entryDiagnostics");
        assert_eq!(ed[0]["factId"], "fact-1", "factId");
        assert_eq!(ed[0]["operationId"], "op-a", "operationId");
        assert_eq!(ed[0]["reason"], "class-absent", "reason");
        for key in ["factId", "operationId", "reason"] {
            assert!(
                ed[1].get(key).is_none(),
                "evidence-unavailable must not carry {key}: {}",
                ed[1]
            );
        }
    }

    /// #69 I2: the human output names the reason and the operation of a
    /// `temp-state-incoherent` diagnostic. Mutation: drop the `reason=` part →
    /// "reason=class-absent".
    #[test]
    fn human_output_shows_incoherence_reason_and_op() {
        let diags = incoherence_diagnostics(
            "DB_INSERT",
            "Subject",
            "fact-1",
            vec![("op-a".to_string(), JoinProblem::ClassAbsent)],
        );
        // W2-2: each diagnostic with its operation under --detail; counts by
        // kind and reason by default.
        let entries = [entry_with(diags)];
        let text = human(&entries, &[], true);
        assert!(
            text.contains(
                "diagnostic: temp-state-incoherent (DB_INSERT) reason=class-absent op=op-a"
            ),
            "reason=class-absent op=op-a missing: {text}"
        );
        let text = human(&entries, &[], false);
        assert!(
            text.contains("diagnostics: temp-state-incoherent 1 (class-absent 1)"),
            "{text}"
        );
    }

    /// The human digest of `entries` over a model whose primary app is `ws`.
    fn human(
        entries: &[DigestEntryFull],
        changed: &[ChangedRootsDiagnostic],
        detail: bool,
    ) -> String {
        let model = ws_model();
        let cx = crate::engine::l5::routine_rollup::RollupCx::new(&model, &[]);
        let rollups: Vec<_> = entries
            .iter()
            .map(|e| crate::engine::l5::routine_rollup::RoutineRollup::build(e, &cx))
            .collect();
        format_digest_human_v3(
            entries,
            &rollups,
            &cx,
            crate::engine::l5::routine_rollup::RenderOpts::new(detail, None),
            changed,
        )
    }

    /// #340 A4: with no changed-roots diagnostic the human text ends with the
    /// entry block, trailing newline included.
    #[test]
    fn human_output_unchanged_without_changed_roots_diagnostics() {
        assert_eq!(
            human(&[], &[], false),
            "digest: no changed roots resolved\n"
        );
        let text = human(&[entry_with(Vec::new())], &[], false);
        // Output validation §5.2: one root's summary line is the header's
        // `answer:`; the card starts with the rest of its summary.
        assert!(
            text.starts_with("--- R ---\n  transaction, UI and errors: "),
            "{text}"
        );
        assert!(text.contains("\n  effects: none\n"), "{text}");
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text}");
    }

    /// #340 A3: one line per changed-roots diagnostic, every variant, in input
    /// order, control characters escaped, after the entries with no blank line.
    /// Mutations: drop the escaping -> (a); add a blank line before the
    /// diagnostics -> (b).
    #[test]
    fn changed_roots_human_lines() {
        use ChangedRootsDiagnostic as D;
        let s = |v: &str| v.to_string();
        let changed = vec![
            D::FileUnmatched {
                file: s("src/a.al"),
            },
            D::SelectorUnmatched {
                selector: s("Foo"),
                hints: Default::default(),
            },
            D::SelectorAmbiguous {
                selector: s("Bar"),
                candidates: vec![s("c1"), s("c2")],
                routines: Vec::new(),
                listed: Vec::new(),
            },
            D::DiffFileUnmatched {
                file: s("src/b.al"),
            },
            D::DiffFileAmbiguous {
                file: s("x/y.al"),
                candidates: vec![s("ws:x/y.al"), s("ws:y.al")],
            },
            D::HunkOutsideRoutines {
                file: s("src/c.al"),
                start_line: 3,
                end_line: 5,
            },
            D::DiffFileDeleted {
                file: s("src/d.al"),
            },
            D::DiffFileNotAl {
                file: s("app.json"),
            },
            D::DiffParseError {
                file: Some(s("src/e.al")),
                detail: s("bad hunk header"),
            },
            D::DiffParseError {
                file: None,
                detail: s("truncated hunk"),
            },
            D::FileUnmatched {
                file: s("src/a\nb\t.al"),
            },
            // Bidi override and line separator are escaped; a backslash is not.
            D::FileUnmatched {
                file: s("src\\x\u{202e}y\u{2028}z.al"),
            },
            // #339: the file and a candidate are escaped too.
            D::FileAmbiguous {
                file: s("C\tl/a\u{202e}.al"),
                candidates: vec![s("ws:C\nl/a.al"), s("ws:a.al")],
            },
        ];
        let lines = concat!(
            "changed-roots: file-unmatched src/a.al\n",
            "changed-roots: selector-unmatched Foo (nearest names: no close match; selector forms: ",
            "Object::Routine, Object::Routine(<parameter types>), Object.\"Field\"::Trigger, ",
            "Object.action(Name)::OnAction, or a stable id)\n",
            "changed-roots: selector-ambiguous Bar (candidates: c1, c2)\n",
            "changed-roots: diff-file-unmatched src/b.al\n",
            "changed-roots: diff-file-ambiguous x/y.al (candidates: ws:x/y.al, ws:y.al); ",
            "pass --diff-root <folder> to read diff paths relative to <folder>\n",
            "changed-roots: hunk-outside-routines src/c.al:3-5\n",
            "changed-roots: diff-file-deleted src/d.al\n",
            "changed-roots: diff-file-not-al app.json\n",
            "changed-roots: diff-parse-error src/e.al: bad hunk header\n",
            "changed-roots: diff-parse-error truncated hunk\n",
            "changed-roots: file-unmatched src/a\\nb\\t.al\n",
            "changed-roots: file-unmatched src\\x\\u{202e}y\\u{2028}z.al\n",
            "changed-roots: file-ambiguous C\\tl/a\\u{202e}.al (candidates: ws:C\\nl/a.al, ws:a.al); ",
            "pass one candidate as --file, or --diff-root <folder> to read paths relative to <folder>\n",
        );
        // (a) no entries.
        assert_eq!(
            human(&[], &changed, false),
            format!("digest: no changed roots resolved\n{lines}")
        );
        // (b) one entry: its block, then the lines, no blank line between.
        let entries = [entry_with(Vec::new())];
        let block = human(&entries, &[], false);
        assert_eq!(human(&entries, &changed, false), format!("{block}{lines}"));
        assert!(!block.ends_with("\n\n"), "{block}");
    }

    // -- #2 gaps-in-cone app-level prefix oracle (moved from prove.rs, #160 step 9)

    /// The SOUNDNESS fix: a symbol-only-boundary gap whose subject is a bare app GUID
    /// is counted when ANY visited routine's stable id starts with `{appGuid}:`. A
    /// direct-match-only filter (the pre-fix bug) would report 0 → a false absence proof.
    #[test]
    fn gaps_in_cone_matches_app_level_prefix() {
        use std::collections::HashSet;
        // Visited routine inside an opaque dep app GUID "11112222...".
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert("11112222-0000-0000-0000-00000000aaaa:Codeunit:50000#deadbeef".to_string());
        visited.insert("33334444-0000-0000-0000-00000000bbbb:Codeunit:60000#cafef00d".to_string());

        // Gap subjects: one bare app GUID (symbol-only-boundary) that matches the prefix,
        // one bare app GUID that does NOT match any visited routine.
        let gap_subjects = vec![
            "11112222-0000-0000-0000-00000000aaaa", // matches via `{guid}:` prefix
            "99998888-0000-0000-0000-00000000cccc", // no visited routine in this app
        ];
        let count = count_gaps_in_cone(&gap_subjects, &visited, &Default::default());
        assert_eq!(
            count, 1,
            "exactly the app-level-matching gap is in cone (the prefix branch must fire)"
        );
    }

    /// Direct routine-subject gaps still match (the non-app branch).
    #[test]
    fn gaps_in_cone_matches_direct_routine_subject() {
        use std::collections::HashSet;
        let mut visited: HashSet<String> = HashSet::new();
        let rid = "aaaa1111-0000-0000-0000-00000000dddd:Codeunit:70000#beadfeed".to_string();
        visited.insert(rid.clone());

        // A parse-incomplete gap whose subject IS a visited StableRoutineId.
        let gap_subjects = vec![rid.as_str()];
        let count = count_gaps_in_cone(&gap_subjects, &visited, &Default::default());
        assert_eq!(count, 1, "direct routine-subject gap must match");
    }

    /// Negative: a gap subject neither directly present nor an app-prefix of any
    /// visited routine is NOT counted.
    #[test]
    fn gaps_in_cone_excludes_unrelated_subject() {
        use std::collections::HashSet;
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert("aaaa1111-0000-0000-0000-00000000dddd:Codeunit:70000#beadfeed".to_string());

        let gap_subjects = vec!["zzzz9999-0000-0000-0000-00000000eeee"];
        let count = count_gaps_in_cone(&gap_subjects, &visited, &Default::default());
        assert_eq!(count, 0, "unrelated gap subject must not be counted");
    }

    /// The transaction context of a Commit names the writes PENDING at it, not
    /// everything the span can reach: a span whose only writes run after the
    /// Commit (`writes_tables` set, `pending_physical_tables` empty, both
    /// stated by hand) has no known writes; one with a pending write has.
    #[test]
    fn tx_context_reads_the_writes_pending_at_the_commit() {
        let span = |op: &str, pending: &[&str]| TransactionSpan {
            seed_kind: SeedKind::ExplicitCommit,
            commit_operation_id: op.to_string(),
            seed_callsite_id: None,
            commit_routine_id: "r".to_string(),
            routines_in_span: vec!["r".to_string()],
            writes_tables: vec!["t/After".to_string()],
            writes_physical_tables_count: 1,
            publishes_events: vec![],
            span_roots: vec!["r".to_string()],
            coverage_complete: true,
            pending_physical_tables: pending.iter().map(|s| s.to_string()).collect(),
            pending_events: vec![],
            pending_physical_by_routine: Default::default(),
        };
        let map = tx_context_map(&[span("op/after", &[]), span("op/before", &["t/Before"])]);
        assert_eq!(map["op/after"], TX_SPAN_NO_WRITES);
        assert_eq!(map["op/before"], TX_SPAN_KNOWN_WRITES);
    }
}
